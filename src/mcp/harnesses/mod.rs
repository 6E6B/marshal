//! Adapters own the public config contract. Shared codecs implement field-level
//! mechanics; the registry and transaction engine never inspect client schemas.
mod amp;
mod claude;
mod cline;
mod codec;
mod codex;
mod continue_client;
mod copilot_cli;
mod cursor;
mod devin;
mod gemini;
mod goose;
mod kiro;
mod opencode;
mod pi;
mod roo;
mod vscode;
mod windsurf;
mod zed;

use super::{
    canonical::McpServer,
    detection::{Installation, Platform, Scope},
    formats::Format,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::PathBuf;

use self::codec::{Fields, STANDARD_KEYS};

#[derive(Clone, Debug)]
pub enum WriteStrategy {
    File,
    ClaudeCli {
        executable: PathBuf,
        scope: &'static str,
        cwd: PathBuf,
    },
}
#[derive(Clone, Debug)]
pub struct Target {
    pub adapter_id: String,
    pub path: PathBuf,
    pub scope: Scope,
    pub format: Format,
    pub root: Vec<String>,
    pub array: bool,
    pub strategy: WriteStrategy,
}
impl Target {
    pub fn new(id: &str, path: PathBuf, scope: Scope, format: Format, root: &str) -> Self {
        Self {
            adapter_id: id.into(),
            path,
            scope,
            format,
            root: vec![root.into()],
            array: false,
            strategy: WriteStrategy::File,
        }
    }
    pub fn label(&self) -> String {
        format!("{} · {}", self.scope.label(), self.path.display())
    }
}

pub trait HarnessAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn binaries(&self) -> &'static [&'static str];
    fn targets(&self, platform: &Platform) -> Vec<Target>;
    fn fields(&self) -> Fields {
        Fields::default()
    }
    fn encode(&self, server: &McpServer) -> Result<Value> {
        self.fields().encode(server)
    }
    fn decode(&self, name: &str, entry: &Value) -> Result<McpServer> {
        self.fields().decode(name, entry)
    }
    /// Only these connection fields may be replaced. Unknown tool/trust controls
    /// and OAuth state remain owned by the harness.
    fn connection_keys(&self) -> &'static [&'static str] {
        STANDARD_KEYS
    }
    fn notice(&self) -> Option<String> {
        None
    }
    fn detect(&self, p: &Platform) -> Option<Installation> {
        let targets = self.targets(p);
        let executable = p.executable(self.binaries());
        let config = targets.iter().any(|t| t.path.is_file());
        let app_data = targets.iter().filter(|t| t.scope == Scope::User).any(|t| {
            t.path
                .parent()
                .is_some_and(|d| d != p.home && d != p.config && d.is_dir())
        });
        if executable.is_none() && !config && !app_data {
            return None;
        }
        Some(Installation {
            adapter_id: self.id().into(),
            executable,
            confidence: if config { "likely" } else { "possible" },
            notice: self.notice(),
        })
    }
    fn entries(&self, target: &Target, config: &Value) -> Result<Vec<(String, Value)>> {
        let mut node = config;
        for key in &target.root {
            let Some(child) = node.get(key) else {
                return Ok(vec![]);
            };
            node = child;
        }
        if target.array {
            let array = node.as_array().context("MCP section must be an array")?;
            let mut names = std::collections::HashSet::new();
            array
                .iter()
                .map(|v| {
                    let name = v["name"].as_str().context(
                        "MCP array entry has no name (package references are not editable)",
                    )?;
                    if !names.insert(name) {
                        bail!("Duplicate MCP server name");
                    }
                    Ok((name.into(), v.clone()))
                })
                .collect()
        } else {
            Ok(node
                .as_object()
                .context("MCP section must be an object")?
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect())
        }
    }
    fn replace(
        &self,
        target: &Target,
        config: &Value,
        name: &str,
        server: Option<&McpServer>,
    ) -> Result<Value> {
        let entries = self.entries(target, config)?;
        let old = entries.iter().find(|(n, _)| n == name).map(|(_, v)| v);
        let entry = if let Some(server) = server {
            server.validate()?;
            let encoded = self.encode(server)?;
            let mut merged = old.cloned().unwrap_or_else(|| json!({}));
            let obj = merged
                .as_object_mut()
                .context("Existing MCP entry is not an object")?;
            for key in self.connection_keys() {
                obj.remove(*key);
            }
            obj.extend(
                encoded
                    .as_object()
                    .context("Adapter did not encode an object")?
                    .clone(),
            );
            // Preserve policy fields even when an encoder supplies safe defaults.
            if let Some(old) = old.and_then(Value::as_object) {
                for (k, v) in old {
                    if !self.connection_keys().contains(&k.as_str()) {
                        obj.insert(k.clone(), v.clone());
                    }
                }
            }
            Some(merged)
        } else {
            None
        };
        let mut out = config.clone();
        let mut node = &mut out;
        for (i, key) in target.root.iter().enumerate() {
            let obj = node
                .as_object_mut()
                .context("MCP parent must be an object")?;
            node = obj.entry(key).or_insert_with(|| {
                if target.array && i + 1 == target.root.len() {
                    json!([])
                } else {
                    json!({})
                }
            });
        }
        if target.array {
            let array = node
                .as_array_mut()
                .context("MCP section must be an array")?;
            let index = array.iter().position(|e| e["name"].as_str() == Some(name));
            match (index, entry) {
                (Some(i), Some(mut v)) => {
                    v["name"] = name.into();
                    array[i] = v;
                }
                (None, Some(mut v)) => {
                    v["name"] = name.into();
                    array.push(v);
                }
                (Some(i), None) => {
                    array.remove(i);
                }
                _ => {}
            }
        } else {
            let obj = node
                .as_object_mut()
                .context("MCP section must be an object")?;
            if let Some(v) = entry {
                obj.insert(name.into(), v);
            } else {
                obj.remove(name);
            }
        }
        Ok(out)
    }
}

pub fn all() -> Vec<Box<dyn HarnessAdapter>> {
    vec![
        Box::new(claude::Claude),
        Box::new(codex::Codex),
        Box::new(gemini::Gemini),
        Box::new(copilot_cli::CopilotCli),
        Box::new(cursor::Cursor),
        Box::new(vscode::VsCode),
        Box::new(opencode::OpenCode),
        Box::new(amp::Amp),
        Box::new(devin::Devin),
        Box::new(windsurf::Windsurf),
        Box::new(kiro::Kiro),
        Box::new(zed::Zed),
        Box::new(cline::Cline),
        Box::new(roo::Roo),
        Box::new(continue_client::Continue),
        Box::new(goose::Goose),
        Box::new(pi::Pi),
    ]
}
pub(super) fn pair(
    p: &Platform,
    id: &str,
    user: PathBuf,
    project: &str,
    format: Format,
    root: &str,
) -> Vec<Target> {
    let mut targets = vec![Target::new(id, user, Scope::User, format, root)];
    if let Some(proj) = &p.project {
        targets.push(Target::new(
            id,
            proj.join(project),
            Scope::Project,
            format,
            root,
        ));
    }
    targets
}
