use super::*;
use codec::{Expansion, Fields};

pub struct Cursor;

impl HarnessAdapter for Cursor {
    fn id(&self) -> &'static str {
        "cursor"
    }

    fn display_name(&self) -> &'static str {
        "Cursor"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["cursor"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        pair(
            p,
            self.id(),
            p.home.join(".cursor/mcp.json"),
            ".cursor/mcp.json",
            Format::Json,
            "mcpServers",
        )
    }

    fn fields(&self) -> Fields {
        Fields {
            cwd: true,
            expansion: Expansion::Colon,
            ..Default::default()
        }
    }
}
