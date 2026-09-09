use super::*;
use codec::Fields;

pub struct Kiro;

impl HarnessAdapter for Kiro {
    fn id(&self) -> &'static str {
        "kiro"
    }

    fn display_name(&self) -> &'static str {
        "Kiro"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["kiro", "kiro-cli"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        pair(
            p,
            self.id(),
            p.home.join(".kiro/settings/mcp.json"),
            ".kiro/settings/mcp.json",
            Format::Json,
            "mcpServers",
        )
    }

    fn fields(&self) -> Fields {
        Fields {
            cwd: true,
            ..Default::default()
        }
    }
}
