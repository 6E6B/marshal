use super::*;
use codec::{Expansion, Fields};

pub struct CopilotCli;

impl HarnessAdapter for CopilotCli {
    fn id(&self) -> &'static str {
        "copilot-cli"
    }

    fn display_name(&self) -> &'static str {
        "GitHub Copilot CLI"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["copilot"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        let copilot_home = p.home_override("COPILOT_HOME", ".copilot");
        let user_path = if copilot_home.extension().is_some_and(|e| e == "json") {
            copilot_home
        } else {
            copilot_home.join("mcp-config.json")
        };
        let mut targets = vec![Target::new(
            self.id(),
            user_path,
            Scope::User,
            Format::Json,
            "mcpServers",
        )];

        if let Some(proj) = &p.project {
            let project_path = if proj.join(".github/mcp.json").exists() {
                proj.join(".github/mcp.json")
            } else {
                proj.join(".mcp.json")
            };
            targets.push(Target::new(
                self.id(),
                project_path,
                Scope::Project,
                Format::Json,
                "mcpServers",
            ));
        }

        targets
    }

    fn fields(&self) -> Fields {
        Fields {
            expansion: Expansion::Dollar,
            ..Default::default()
        }
    }
}
