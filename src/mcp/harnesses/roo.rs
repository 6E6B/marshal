use super::*;
use codec::Fields;

pub struct Roo;

const ROO_KEYS: &[&str] = &[
    "type",
    "command",
    "args",
    "cwd",
    "env",
    "url",
    "headers",
    "alwaysAllow",
    "disabledTools",
    "disabled",
];

impl HarnessAdapter for Roo {
    fn id(&self) -> &'static str {
        "roo"
    }

    fn display_name(&self) -> &'static str {
        "Roo Code"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["roo", "roocode"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        pair(
            p,
            self.id(),
            p.home.join(".roo/mcp_settings.json"),
            ".roo/mcp.json",
            Format::Json,
            "mcpServers",
        )
    }

    fn fields(&self) -> Fields {
        Fields {
            http_type: Some("streamable-http"),
            sse_type: Some("sse"),
            ..Default::default()
        }
    }

    fn connection_keys(&self) -> &'static [&'static str] {
        ROO_KEYS
    }
}
