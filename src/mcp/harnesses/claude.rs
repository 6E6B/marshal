use super::*;
use codec::{Expansion, Fields};

pub struct Claude;

impl HarnessAdapter for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["claude"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        let cli = p.executable(self.binaries());
        let user_path = p.home_override("CLAUDE_CONFIG_DIR", ".claude.json");

        let mut targets = vec![Target {
            adapter_id: self.id().into(),
            path: user_path,
            scope: Scope::User,
            format: Format::Json,
            root: vec!["mcpServers".into()],
            array: false,
            strategy: cli
                .as_ref()
                .map(|bin| WriteStrategy::ClaudeCli {
                    executable: bin.clone(),
                    scope: "user",
                    cwd: p.home.clone(),
                })
                .unwrap_or(WriteStrategy::File),
        }];

        if let Some(proj) = &p.project {
            targets.push(Target {
                adapter_id: self.id().into(),
                path: proj.join(".mcp.json"),
                scope: Scope::Project,
                format: Format::Json,
                root: vec!["mcpServers".into()],
                array: false,
                strategy: cli
                    .as_ref()
                    .map(|bin| WriteStrategy::ClaudeCli {
                        executable: bin.clone(),
                        scope: "project",
                        cwd: proj.clone(),
                    })
                    .unwrap_or(WriteStrategy::File),
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
}
