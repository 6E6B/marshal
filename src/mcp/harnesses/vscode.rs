use super::*;
use codec::{Expansion, Fields};

pub struct VsCode;

impl HarnessAdapter for VsCode {
    fn id(&self) -> &'static str {
        "vscode"
    }

    fn display_name(&self) -> &'static str {
        "VS Code"
    }

    fn binaries(&self) -> &'static [&'static str] {
        &["code", "code-insiders", "codium"]
    }

    fn targets(&self, p: &Platform) -> Vec<Target> {
        let mut targets = Vec::new();
        let default_user = p.config.join("Code/User/mcp.json");
        targets.push(Target::new(
            self.id(),
            default_user.clone(),
            Scope::User,
            Format::Jsonc,
            "servers",
        ));

        for user_dir in p.editor_users() {
            let mcp_path = user_dir.join("mcp.json");
            if mcp_path != default_user && mcp_path.exists() {
                targets.push(Target::new(
                    self.id(),
                    mcp_path,
                    Scope::User,
                    Format::Jsonc,
                    "servers",
                ));
            }
        }

        if let Some(proj) = &p.project {
            targets.push(Target::new(
                self.id(),
                proj.join(".vscode/mcp.json"),
                Scope::Project,
                Format::Jsonc,
                "servers",
            ));
        }

        targets
    }

    fn fields(&self) -> Fields {
        Fields {
            stdio_type: Some("stdio"),
            http_type: Some("http"),
            sse_type: Some("sse"),
            http_key: "url",
            sse_key: Some("url"),
            cwd: true,
            expansion: Expansion::Colon,
            ..Default::default()
        }
    }
}
