use super::*;
use codec::{Expansion, Fields};

pub struct Gemini;

impl HarnessAdapter for Gemini {
    fn id(&self) -> &'static str {
        "gemini"
    }

    fn display_name(&self) -> &'static str {
        "Google Antigravity"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["agy", "gemini", "antigravity"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        let user_path = p.home.join(".gemini/config/mcp_config.json");
        pair(
            p,
            self.id(),
            user_path,
            ".agent/mcp_config.json",
            Format::Json,
            "mcpServers",
        )
    }

    fn fields(&self) -> Fields {
        Fields {
            http_key: "serverUrl",
            sse_key: Some("serverUrl"),
            expansion: Expansion::Dollar,
            ..Default::default()
        }
    }

    fn decode(&self, name: &str, entry: &Value) -> Result<McpServer> {
        let mut fields = self.fields();
        if entry.get("serverUrl").is_none() {
            if entry.get("httpUrl").is_some() {
                fields.http_key = "httpUrl";
                fields.sse_key = Some("httpUrl");
            } else if entry.get("url").is_some() {
                fields.http_key = "url";
                fields.sse_key = Some("url");
            }
        }
        fields.decode(name, entry)
    }
}
