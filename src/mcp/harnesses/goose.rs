use super::*;
use codec::{Expansion, Fields};

pub struct Goose;

const GOOSE_KEYS: &[&str] = &["name", "type", "cmd", "args", "envs", "url", "enabled"];

impl HarnessAdapter for Goose {
    fn id(&self) -> &'static str {
        "goose"
    }

    fn display_name(&self) -> &'static str {
        "Goose"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["goose"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        vec![Target::new(
            self.id(),
            p.config.join("goose/config.yaml"),
            Scope::User,
            Format::Yaml,
            "extensions",
        )]
    }

    fn fields(&self) -> Fields {
        Fields {
            stdio_type: Some("stdio"),
            http_type: Some("streamable-http"),
            sse_type: Some("sse"),
            http_key: "url",
            sse_key: Some("url"),
            command_key: "cmd",
            command_array: false,
            env_key: "envs",
            cwd: false,
            expansion: Expansion::Dollar,
            remote: true,
        }
    }

    fn encode(&self, server: &McpServer) -> Result<Value> {
        let mut v = self.fields().encode(server)?;
        v["name"] = server.name.clone().into();
        v["enabled"] = true.into();
        Ok(v)
    }

    fn connection_keys(&self) -> &'static [&'static str] {
        GOOSE_KEYS
    }
}
