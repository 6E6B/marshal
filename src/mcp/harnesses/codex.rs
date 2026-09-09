use super::*;
use codec::{Expansion, Fields};

pub struct Codex;

impl HarnessAdapter for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn display_name(&self) -> &'static str {
        "OpenAI Codex"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["codex"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        let codex_home = p.home_override("CODEX_HOME", ".codex");
        let user_path = if codex_home.extension().is_some_and(|e| e == "toml") {
            codex_home
        } else {
            codex_home.join("config.toml")
        };
        pair(
            p,
            self.id(),
            user_path,
            ".codex/config.toml",
            Format::Toml,
            "mcp_servers",
        )
    }

    fn fields(&self) -> Fields {
        Fields {
            cwd: true,
            expansion: Expansion::Dollar,
            ..Default::default()
        }
    }
}
