use super::*;

pub struct Windsurf;

impl HarnessAdapter for Windsurf {
    fn id(&self) -> &'static str {
        "windsurf"
    }

    fn display_name(&self) -> &'static str {
        "Windsurf Cascade"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["windsurf"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        vec![Target::new(
            self.id(),
            p.home.join(".codeium/windsurf/mcp_config.json"),
            Scope::User,
            Format::Json,
            "mcpServers",
        )]
    }
}
