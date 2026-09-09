use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    User,
    Project,
    ProjectLocal,
}
impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "User",
            Self::Project => "Project",
            Self::ProjectLocal => "Private project",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Installation {
    pub adapter_id: String,
    pub executable: Option<PathBuf>,
    pub confidence: &'static str,
    pub notice: Option<String>,
}

/// All OS/path conventions live here. Tests always inject an isolated environment.
#[derive(Clone)]
pub struct Platform {
    pub home: PathBuf,
    pub config: PathBuf,
    pub path: Vec<PathBuf>,
    pub project: Option<PathBuf>,
    pub overrides: BTreeMap<String, PathBuf>,
}
impl Platform {
    pub fn current(project: Option<PathBuf>) -> Result<Self> {
        if !cfg!(target_os = "linux") {
            bail!("Harness discovery currently supports Linux only");
        }
        let home = directories::BaseDirs::new()
            .context("Home directory unavailable")?
            .home_dir()
            .to_owned();
        let config = if crate::host::in_flatpak() {
            // Flatpak sets XDG_CONFIG_HOME to the sandbox data dir. Client
            // configs live in the real ~/.config on the host.
            home.join(".config")
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(".config"))
        };
        let project = match project {
            Some(p) => {
                let p = p.canonicalize()?;
                if !p.is_dir() {
                    bail!("Select a project directory");
                }
                Some(p)
            }
            None => {
                // If no explicit project was provided, only recognize a project
                // if cwd is inside a git repository that is not the user's home directory.
                std::env::current_dir()
                    .ok()
                    .and_then(|cwd| cwd.canonicalize().ok())
                    .and_then(|cwd| {
                        cwd.ancestors()
                            .find(|p| p.join(".git").exists() && *p != home)
                            .map(|p| p.to_owned())
                    })
            }
        };
        let overrides = [
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "COPILOT_HOME",
            "OPENCODE_CONFIG",
            "PI_CODING_AGENT_DIR",
        ]
        .into_iter()
        .filter_map(|k| {
            std::env::var_os(k)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|v| (k.into(), v))
        })
        .collect();
        Ok(Self {
            home,
            config,
            project,
            overrides,
            path: crate::host::host_path(),
        })
    }
    pub fn executable(&self, names: &[&str]) -> Option<PathBuf> {
        names
            .iter()
            .flat_map(|n| self.path.iter().map(move |p| p.join(n)))
            .find(|p| crate::host::is_executable(p))
    }
    pub fn home_override(&self, key: &str, default: &str) -> PathBuf {
        self.overrides
            .get(key)
            .cloned()
            .unwrap_or_else(|| self.home.join(default))
    }
    pub fn editor_users(&self) -> Vec<PathBuf> {
        let mut result = Vec::new();
        for editor in ["Code", "Code - Insiders", "VSCodium", "Cursor", "Windsurf"] {
            let user = self.config.join(editor).join("User");
            if user.is_dir() {
                result.push(user.clone());
            }
            for profile in bounded_dirs(&user.join("profiles"), 100) {
                result.push(profile);
            }
        }
        result
    }
}
pub fn bounded_dirs(path: &Path, limit: usize) -> Vec<PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .take(limit)
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    entries.sort();
    entries
}
