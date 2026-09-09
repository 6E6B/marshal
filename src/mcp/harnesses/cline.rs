use super::*;
use codec::Fields;

pub struct Cline;

const CLINE_KEYS: &[&str] = &[
    "type",
    "command",
    "args",
    "cwd",
    "env",
    "url",
    "headers",
    "autoApprove",
    "disabled",
];

impl HarnessAdapter for Cline {
    fn id(&self) -> &'static str {
        "cline"
    }

    fn display_name(&self) -> &'static str {
        "Cline"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["cline"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        let mut targets = Vec::new();
        let ide_path = p.home.join(".cline/data/settings/cline_mcp_settings.json");
        targets.push(Target::new(
            self.id(),
            ide_path,
            Scope::User,
            Format::Json,
            "mcpServers",
        ));

        let cli_path = p.home.join(".cline/mcp.json");
        if cli_path.exists() {
            targets.push(Target::new(
                self.id(),
                cli_path,
                Scope::User,
                Format::Json,
                "mcpServers",
            ));
        }

        if let Some(proj) = &p.project {
            targets.push(Target::new(
                self.id(),
                proj.join(".cline/mcp.json"),
                Scope::Project,
                Format::Json,
                "mcpServers",
            ));
        }

        targets
    }

    fn fields(&self) -> Fields {
        Fields {
            http_type: Some("streamableHttp"),
            sse_type: Some("sse"),
            ..Default::default()
        }
    }

    fn connection_keys(&self) -> &'static [&'static str] {
        CLINE_KEYS
    }
}
