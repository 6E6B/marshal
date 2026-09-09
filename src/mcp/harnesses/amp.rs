use super::*;
use codec::{Expansion, Fields};

pub struct Amp;

impl HarnessAdapter for Amp {
    fn id(&self) -> &'static str {
        "amp"
    }

    fn display_name(&self) -> &'static str {
        "Amp"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["amp"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        pair(
            p,
            self.id(),
            p.config.join("amp/settings.json"),
            ".amp/settings.json",
            Format::Jsonc,
            "amp.mcpServers",
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
