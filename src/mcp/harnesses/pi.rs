use super::*;
use codec::{Expansion, Fields};

pub struct Pi;

impl HarnessAdapter for Pi {
    fn id(&self) -> &'static str {
        "pi"
    }

    fn display_name(&self) -> &'static str {
        "Pi"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["pi"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        let user_path = p
            .home_override("PI_CODING_AGENT_DIR", ".pi/agent")
            .join("mcp.json");
        pair(
            p,
            self.id(),
            user_path,
            ".pi/mcp.json",
            Format::Json,
            "mcpServers",
        )
    }

    fn notice(&self) -> Option<String> {
        Some("Pi — MCP support via pi-mcp-extension".into())
    }

    fn fields(&self) -> Fields {
        Fields {
            http_type: Some("streamable-http"),
            sse_type: Some("sse"),
            expansion: Expansion::Dollar,
            ..Default::default()
        }
    }
}
