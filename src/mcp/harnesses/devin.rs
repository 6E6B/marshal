use super::*;
use codec::{Expansion, Fields};

pub struct Devin;

impl HarnessAdapter for Devin {
    fn id(&self) -> &'static str {
        "devin"
    }

    fn display_name(&self) -> &'static str {
        "Devin"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["devin"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        let mut targets = vec![Target::new(
            self.id(),
            p.config.join("devin/mcp_config.json"),
            Scope::User,
            Format::Json,
            "mcpServers",
        )];

        if let Some(proj) = &p.project {
            targets.push(Target::new(
                self.id(),
                proj.join(".devin/mcp_config.json"),
                Scope::Project,
                Format::Json,
                "mcpServers",
            ));
            targets.push(Target::new(
                self.id(),
                proj.join(".devin/mcp_config.local.json"),
                Scope::ProjectLocal,
                Format::Json,
                "mcpServers",
            ));
        }

        targets
    }

    fn fields(&self) -> Fields {
        Fields {
            cwd: true,
            expansion: Expansion::Dollar,
            ..Default::default()
        }
    }
}
