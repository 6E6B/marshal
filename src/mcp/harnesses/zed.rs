use super::*;
use codec::{Expansion, Fields};

pub struct Zed;

impl HarnessAdapter for Zed {
    fn id(&self) -> &'static str {
        "zed"
    }

    fn display_name(&self) -> &'static str {
        "Zed"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["zed", "zed-editor"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        pair(
            p,
            self.id(),
            p.config.join("zed/settings.json"),
            ".zed/settings.json",
            Format::Jsonc,
            "context_servers",
        )
    }

    fn fields(&self) -> Fields {
        Fields {
            cwd: false,
            expansion: Expansion::Dollar,
            ..Default::default()
        }
    }
}
