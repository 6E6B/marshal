use super::*;
use codec::{Expansion, Fields};

pub struct OpenCode;

impl HarnessAdapter for OpenCode {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn display_name(&self) -> &'static str {
        "OpenCode"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["opencode"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        let user_path = p.home_override("OPENCODE_CONFIG", ".config/opencode/opencode.json");
        pair(
            p,
            self.id(),
            user_path,
            "opencode.json",
            Format::Jsonc,
            "mcp",
        )
    }

    fn fields(&self) -> Fields {
        Fields {
            stdio_type: Some("local"),
            http_type: Some("remote"),
            sse_type: Some("remote"),
            command_array: true,
            expansion: Expansion::OpenCode,
            ..Default::default()
        }
    }
}
