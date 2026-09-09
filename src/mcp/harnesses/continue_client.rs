use super::*;
use codec::{Expansion, Fields};

pub struct Continue;

const CONTINUE_KEYS: &[&str] = &[
    "name", "type", "command", "args", "cwd", "env", "url", "headers",
];

impl HarnessAdapter for Continue {
    fn id(&self) -> &'static str {
        "continue"
    }

    fn display_name(&self) -> &'static str {
        "Continue"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["continue"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        let mut targets = vec![Target {
            adapter_id: self.id().into(),
            path: p.home.join(".continue/config.yaml"),
            scope: Scope::User,
            format: Format::Yaml,
            root: vec!["mcpServers".into()],
            array: true,
            strategy: WriteStrategy::File,
        }];

        if let Some(proj) = &p.project {
            targets.push(Target {
                adapter_id: self.id().into(),
                path: proj.join(".continue/config.yaml"),
                scope: Scope::Project,
                format: Format::Yaml,
                root: vec!["mcpServers".into()],
                array: true,
                strategy: WriteStrategy::File,
            });
        }

        targets
    }

    fn fields(&self) -> Fields {
        Fields {
            expansion: Expansion::Dollar,
            ..Default::default()
        }
    }

    fn encode(&self, server: &McpServer) -> Result<Value> {
        let mut v = self.fields().encode(server)?;
        v["name"] = server.name.clone().into();
        Ok(v)
    }

    fn connection_keys(&self) -> &'static [&'static str] {
        CONTINUE_KEYS
    }
}
