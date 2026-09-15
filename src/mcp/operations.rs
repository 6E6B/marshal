use super::{
    canonical::{ConfigValue, McpServer, Transport},
    formats,
    harnesses::{HarnessAdapter, Target, WriteStrategy},
};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Debug)]
pub enum TargetStatus {
    NotInstalled,
    Identical,
    Conflict {
        existing: Box<McpServer>,
        requested: Box<McpServer>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConflictResolution {
    Overwrite,
    Skip,
    Rename(String),
}

#[derive(Clone, Debug)]
pub enum TargetActionTaken {
    Installed,
    AlreadyUpToDate,
    Replaced,
    Skipped,
    #[cfg_attr(not(test), allow(dead_code))]
    Removed,
}

#[derive(Clone, Debug)]
pub struct TargetOpResult {
    #[cfg_attr(not(test), allow(dead_code))]
    pub target: Target,
    pub action: TargetActionTaken,
}

/// Reads the existing raw text of a target configuration, or empty string if file does not exist.
pub fn read_target_raw(path: &Path) -> Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }
    std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read target configuration at {}", path.display()))
}

/// Reads and parses the existing target configuration into a JSON value.
pub fn read_target_value(target: &Target) -> Result<Value> {
    let raw = read_target_raw(&target.path)?;
    formats::parse(&raw, target.format)
        .with_context(|| format!("Failed to parse configuration at {}", target.path.display()))
}

/// Writes content to a file atomically via a temporary file in the same directory.
pub fn atomic_write(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory {}", parent.display()))?;
        let mut temp = tempfile::NamedTempFile::new_in(parent).with_context(|| {
            format!(
                "Failed to create temporary file in directory {}",
                parent.display()
            )
        })?;
        use std::io::Write;
        temp.write_all(content.as_bytes())
            .with_context(|| format!("Failed to write to temporary file for {}", path.display()))?;
        temp.flush()
            .with_context(|| format!("Failed to flush temporary file for {}", path.display()))?;
        temp.persist(path).map_err(|e| {
            anyhow::anyhow!(
                "Failed to atomically persist {} to {}: {}",
                e.file.path().display(),
                path.display(),
                e.error
            )
        })?;
    } else {
        std::fs::write(path, content)
            .with_context(|| format!("Failed to write file {}", path.display()))?;
    }
    Ok(())
}

/// Checks the status of a specific server in a target.
pub fn check_target_status(
    adapter: &dyn HarnessAdapter,
    target: &Target,
    server: &McpServer,
) -> Result<TargetStatus> {
    let config = read_target_value(target)?;
    let entries = adapter.entries(target, &config)?;
    if let Some((_, entry)) = entries.iter().find(|(n, _)| n == &server.name) {
        match adapter.decode(&server.name, entry) {
            Ok(existing) => {
                if existing.equivalent(server) {
                    Ok(TargetStatus::Identical)
                } else {
                    Ok(TargetStatus::Conflict {
                        existing: Box::new(existing),
                        requested: Box::new(server.clone()),
                    })
                }
            }
            Err(_) => {
                // If existing configuration cannot be decoded into standard canonical model,
                // treat as conflict so user can decide.
                let mut fallback = McpServer::empty(&server.name, Transport::Stdio);
                fallback.command = Some("<unrecognized connection format>".into());
                Ok(TargetStatus::Conflict {
                    existing: Box::new(fallback),
                    requested: Box::new(server.clone()),
                })
            }
        }
    } else {
        Ok(TargetStatus::NotInstalled)
    }
}

/// Writes a server definition into a target, using the Claude CLI when configured
/// and falling back to lossless file edits.
pub fn apply_target_write(
    adapter: &dyn HarnessAdapter,
    target: &Target,
    name: &str,
    server: Option<&McpServer>,
) -> Result<()> {
    match &target.strategy {
        WriteStrategy::ClaudeCli {
            executable,
            scope,
            cwd,
        } => {
            let cli = match server {
                Some(server) => claude_cli_add(executable, scope, cwd, server),
                None => claude_cli_remove(executable, scope, cwd, name),
            };
            if cli.is_err() {
                apply_file_write(adapter, target, name, server)?;
            }
        }
        WriteStrategy::File => apply_file_write(adapter, target, name, server)?,
    }
    Ok(())
}

/// Writes a server definition into a target file using lossless CST editing.
pub fn apply_file_write(
    adapter: &dyn HarnessAdapter,
    target: &Target,
    name: &str,
    server: Option<&McpServer>,
) -> Result<()> {
    let raw = read_target_raw(&target.path)?;
    let before = formats::parse(&raw, target.format)?;
    let after = adapter.replace(target, &before, name, server)?;
    let updated = formats::edit(&raw, target.format, &after)?;
    atomic_write(&target.path, &updated)
}

/// Runs Claude CLI to add a server.
pub fn claude_cli_add(
    executable: &Path,
    scope: &str,
    cwd: &Path,
    server: &McpServer,
) -> Result<()> {
    let mut cmd = crate::host::std_command(executable);
    cmd.current_dir(cwd);
    cmd.arg("mcp").arg("add").arg("--scope").arg(scope);

    match server.transport {
        Transport::Stdio => {
            for (key, val) in &server.env {
                match val {
                    ConfigValue::Literal { value } => {
                        cmd.arg("-e").arg(format!("{key}={value}"));
                    }
                    ConfigValue::Env { name } => {
                        cmd.arg("-e").arg(format!("{key}=${name}"));
                    }
                    ConfigValue::EnvTemplate {
                        name,
                        prefix,
                        suffix,
                    } => {
                        cmd.arg("-e").arg(format!("{key}={prefix}${name}{suffix}"));
                    }
                }
            }
            cmd.arg(&server.name);
            if let Some(command) = &server.command {
                cmd.arg(command);
            }
            for arg in &server.args {
                cmd.arg(arg);
            }
        }
        Transport::StreamableHttp => {
            cmd.arg("--transport").arg("http");
            cmd.arg(&server.name);
            cmd.arg(server.url.as_deref().unwrap_or_default());
        }
        Transport::Sse => {
            cmd.arg("--transport").arg("sse");
            cmd.arg(&server.name);
            cmd.arg(server.url.as_deref().unwrap_or_default());
        }
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to execute Claude CLI: {}", executable.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Claude CLI failed with exit code {:?}: {}",
            output.status.code(),
            stderr.trim()
        );
    }
    Ok(())
}

/// Runs Claude CLI to remove a server.
pub fn claude_cli_remove(
    executable: &Path,
    scope: &str,
    cwd: &Path,
    server_name: &str,
) -> Result<()> {
    let mut cmd = crate::host::std_command(executable);
    cmd.current_dir(cwd);
    cmd.arg("mcp")
        .arg("remove")
        .arg("--scope")
        .arg(scope)
        .arg(server_name);

    let output = cmd
        .output()
        .with_context(|| format!("Failed to execute Claude CLI: {}", executable.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Claude CLI remove failed with exit code {:?}: {}",
            output.status.code(),
            stderr.trim()
        );
    }
    Ok(())
}
