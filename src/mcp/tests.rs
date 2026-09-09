use super::{
    canonical::{ConfigValue, McpServer, Transport},
    detection::{Platform, Scope},
    formats::{self, Format},
    harnesses::{self, Target},
    operations::{ConflictResolution, TargetActionTaken, TargetStatus},
    registry::HarnessRegistry,
};
use serde_json::json;
use std::collections::BTreeMap;
use tempfile::TempDir;

fn make_platform(temp: &TempDir) -> Platform {
    let home = temp.path().join("home");
    let config = home.join(".config");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&project).unwrap();

    Platform {
        home,
        config,
        path: vec![],
        project: Some(project),
        overrides: BTreeMap::new(),
    }
}

fn sample_stdio_server(name: &str) -> McpServer {
    let mut server = McpServer::empty(name, Transport::Stdio);
    server.command = Some("npx".into());
    server.args = vec!["-y".into(), "@upstash/context7-mcp".into()];
    server.env.insert(
        "API_KEY".into(),
        ConfigValue::Literal {
            value: "secret123".into(),
        },
    );
    server.env.insert(
        "TOKEN".into(),
        ConfigValue::Env {
            name: "MY_TOKEN".into(),
        },
    );
    server
}

fn sample_http_server(name: &str) -> McpServer {
    let mut server = McpServer::empty(name, Transport::StreamableHttp);
    server.url = Some("https://mcp.context7.com/mcp".into());
    server.headers.insert(
        "Authorization".into(),
        ConfigValue::EnvTemplate {
            name: "AUTH_TOKEN".into(),
            prefix: "Bearer ".into(),
            suffix: "".into(),
        },
    );
    server
}

#[test]
fn test_canonical_validation() {
    let valid = sample_stdio_server("valid-server");
    assert!(valid.validate().is_ok());

    let mut invalid_name = valid.clone();
    invalid_name.name = "".into();
    assert!(invalid_name.validate().is_err());

    let mut null_name = valid.clone();
    null_name.name = "test\0bad".into();
    assert!(null_name.validate().is_err());

    let mut stdio_with_url = valid.clone();
    stdio_with_url.url = Some("https://example.com".into());
    assert!(stdio_with_url.validate().is_err());

    let http_valid = sample_http_server("remote");
    assert!(http_valid.validate().is_ok());

    let mut http_with_command = http_valid.clone();
    http_with_command.command = Some("node".into());
    assert!(http_with_command.validate().is_err());
}

#[test]
fn test_canonical_debug_redaction() {
    let server = sample_stdio_server("secret-server");
    let debug_repr = format!("{:?}", server);
    assert!(
        !debug_repr.contains("secret123"),
        "Debug representation leaked secret literal"
    );
}

#[test]
fn test_jsonc_lossless_edit() {
    let input = r#"{
    // Top-level comment
    "mcpServers": {
        // First server comment
        "first": {
            "command": "node",
            "args": ["server.js"]
        }
    }
}
"#;
    let mut val = formats::parse(input, Format::Jsonc).unwrap();
    val["mcpServers"]["second"] = json!({
        "command": "python",
        "args": ["app.py"]
    });

    let edited = formats::edit(input, Format::Jsonc, &val).unwrap();
    assert!(edited.contains("// Top-level comment"));
    assert!(edited.contains("// First server comment"));
    assert!(edited.contains("\"second\""));
}

#[test]
fn test_toml_lossless_edit() {
    let input = r#"# Main Codex configuration
[general]
model = "gpt-5"

# MCP Servers section
[mcp_servers.existing]
command = "npx"
args = ["-y", "mcp-test"]
"#;
    let mut val = formats::parse(input, Format::Toml).unwrap();
    val["mcp_servers"]["new_server"] = json!({
        "command": "cargo",
        "args": ["run"]
    });

    let edited = formats::edit(input, Format::Toml, &val).unwrap();
    assert!(edited.contains("# Main Codex configuration"));
    assert!(edited.contains("# MCP Servers section"));
    assert!(edited.contains("new_server"));
    assert!(edited.contains("model = \"gpt-5\""));
}

#[test]
fn test_yaml_lossless_edit_continue_sequence() {
    let input = r#"# Continue Configuration
name: My Project
mcpServers:
  # Existing server
  - name: existing
    command: node
    args:
      - index.js
"#;
    let mut val = formats::parse(input, Format::Yaml).unwrap();
    val["mcpServers"].as_array_mut().unwrap().push(json!({
        "name": "added",
        "command": "python",
        "args": ["main.py"]
    }));

    let edited = formats::edit(input, Format::Yaml, &val).unwrap();
    assert!(edited.contains("# Existing server"));
    assert!(edited.contains("added"));
}

#[test]
fn test_vscode_adapter_servers_key_and_types() {
    let temp = TempDir::new().unwrap();
    let platform = make_platform(&temp);
    let adapter = harnesses::all()
        .into_iter()
        .find(|a| a.id() == "vscode")
        .unwrap();

    let stdio = sample_stdio_server("test_stdio");
    let encoded_stdio = adapter.encode(&stdio).unwrap();
    assert_eq!(encoded_stdio["type"], "stdio");
    assert_eq!(encoded_stdio["command"], "npx");
    // Verify colon expansion for VS Code: ${env:MY_TOKEN}
    assert_eq!(encoded_stdio["env"]["TOKEN"], "${env:MY_TOKEN}");

    let decoded = adapter.decode("test_stdio", &encoded_stdio).unwrap();
    assert_eq!(decoded.name, "test_stdio");
    assert_eq!(decoded.transport, Transport::Stdio);

    let targets = adapter.targets(&platform);
    assert!(targets.iter().all(|t| t.root == vec!["servers"]));
}

#[test]
fn test_opencode_adapter_array_command_and_mcp_root() {
    let temp = TempDir::new().unwrap();
    let platform = make_platform(&temp);
    let adapter = harnesses::all()
        .into_iter()
        .find(|a| a.id() == "opencode")
        .unwrap();

    let stdio = sample_stdio_server("ctx7");
    let encoded = adapter.encode(&stdio).unwrap();
    assert_eq!(encoded["type"], "local");
    assert!(encoded["command"].is_array());
    assert_eq!(encoded["command"][0], "npx");
    assert_eq!(encoded["command"][1], "-y");
    assert_eq!(encoded["command"][2], "@upstash/context7-mcp");
    // Verify OpenCode expansion: {env:MY_TOKEN}
    assert_eq!(encoded["env"]["TOKEN"], "{env:MY_TOKEN}");

    let decoded = adapter.decode("ctx7", &encoded).unwrap();
    assert_eq!(decoded.name, "ctx7");
    assert_eq!(decoded.command.as_deref(), Some("npx"));
    assert_eq!(decoded.args, vec!["-y", "@upstash/context7-mcp"]);

    let targets = adapter.targets(&platform);
    assert!(targets.iter().all(|t| t.root == vec!["mcp"]));
}

#[test]
fn test_zed_adapter_context_servers() {
    let temp = TempDir::new().unwrap();
    let platform = make_platform(&temp);
    let adapter = harnesses::all()
        .into_iter()
        .find(|a| a.id() == "zed")
        .unwrap();

    let targets = adapter.targets(&platform);
    assert!(targets.iter().all(|t| t.root == vec!["context_servers"]));
}

#[test]
fn test_amp_adapter_mcp_servers_key() {
    let temp = TempDir::new().unwrap();
    let platform = make_platform(&temp);
    let adapter = harnesses::all()
        .into_iter()
        .find(|a| a.id() == "amp")
        .unwrap();

    let targets = adapter.targets(&platform);
    assert!(targets.iter().all(|t| t.root == vec!["amp.mcpServers"]));
}

#[test]
fn test_gemini_adapter_http_key_distinction() {
    let adapter = harnesses::all()
        .into_iter()
        .find(|a| a.id() == "gemini")
        .unwrap();

    let http_server = sample_http_server("gemini-http");
    let encoded = adapter.encode(&http_server).unwrap();
    assert!(encoded.get("serverUrl").is_some());

    let decoded = adapter.decode("gemini-http", &encoded).unwrap();
    assert_eq!(decoded.name, "gemini-http");
    assert_eq!(decoded.url, http_server.url);
}

#[test]
fn test_goose_adapter_extensions_and_cmd() {
    let adapter = harnesses::all()
        .into_iter()
        .find(|a| a.id() == "goose")
        .unwrap();
    let stdio = sample_stdio_server("goose-test");
    let encoded = adapter.encode(&stdio).unwrap();

    assert_eq!(encoded["name"], "goose-test");
    assert_eq!(encoded["cmd"], "npx");
    assert_eq!(encoded["type"], "stdio");
    assert_eq!(encoded["enabled"], true);
    assert!(encoded.get("envs").is_some());
}

#[test]
fn test_all_17_adapters_registered() {
    let registry = HarnessRegistry::new();
    assert_eq!(registry.adapters().len(), 17);

    let expected = [
        "claude",
        "codex",
        "gemini",
        "copilot-cli",
        "cursor",
        "vscode",
        "opencode",
        "amp",
        "devin",
        "windsurf",
        "kiro",
        "zed",
        "cline",
        "roo",
        "continue",
        "goose",
        "pi",
    ];

    for id in expected {
        assert!(registry.adapter(id).is_some(), "Missing adapter for {id}");
    }
}

#[test]
fn test_install_and_idempotency_and_drift() {
    let temp = TempDir::new().unwrap();
    let platform = make_platform(&temp);
    let registry = HarnessRegistry::new();

    let target_cursor = Target::new(
        "cursor",
        platform.project.as_ref().unwrap().join(".cursor/mcp.json"),
        Scope::Project,
        Format::Json,
        "mcpServers",
    );
    let target_vscode = Target::new(
        "vscode",
        platform.project.as_ref().unwrap().join(".vscode/mcp.json"),
        Scope::Project,
        Format::Jsonc,
        "servers",
    );
    let target_codex = Target::new(
        "codex",
        platform
            .project
            .as_ref()
            .unwrap()
            .join(".codex/config.toml"),
        Scope::Project,
        Format::Toml,
        "mcp_servers",
    );

    let targets = vec![
        target_cursor.clone(),
        target_vscode.clone(),
        target_codex.clone(),
    ];
    let server = sample_stdio_server("context7");

    // First install
    let results = registry
        .install_server(&server, &targets, ConflictResolution::Overwrite)
        .unwrap();
    assert_eq!(results.len(), 3);
    for r in &results {
        assert!(matches!(r.action, TargetActionTaken::Installed));
    }

    // Verify files exist
    assert!(target_cursor.path.exists());
    assert!(target_vscode.path.exists());
    assert!(target_codex.path.exists());

    // Second install (idempotency check)
    let results_second = registry
        .install_server(&server, &targets, ConflictResolution::Overwrite)
        .unwrap();
    for r in &results_second {
        assert!(matches!(r.action, TargetActionTaken::AlreadyUpToDate));
    }

    // Conflict detection check
    let mut modified = server.clone();
    modified.args.push("--extra-arg".into());
    let status = registry.check_status(&modified, &target_cursor).unwrap();
    assert!(matches!(status, TargetStatus::Conflict { .. }));

    // Inventory scan check
    let inventory = registry.scan_inventory(&platform).unwrap();
    let entry = inventory.iter().find(|e| e.name == "context7").unwrap();
    assert_eq!(entry.installations.len(), 3);
    assert!(!entry.has_drift);

    // Remove server
    let remove_results = registry.remove_server("context7", &targets).unwrap();
    for r in &remove_results {
        assert!(matches!(r.action, TargetActionTaken::Removed));
    }

    let status_after = registry.check_status(&server, &target_cursor).unwrap();
    assert!(matches!(status_after, TargetStatus::NotInstalled));
}

#[test]
fn test_targets_without_project_only_emit_user_scope() {
    let temp = TempDir::new().unwrap();
    let mut platform = make_platform(&temp);
    platform.project = None;

    let registry = HarnessRegistry::new();
    for adapter in registry.adapters() {
        let targets = adapter.targets(&platform);
        for target in targets {
            assert_eq!(
                target.scope,
                Scope::User,
                "Adapter {} emitted non-User target {:?} when platform.project is None",
                adapter.id(),
                target
            );
        }
    }
}

#[test]
fn test_install_bridge_server_to_codex() {
    let temp = TempDir::new().unwrap();
    let platform = make_platform(&temp);
    let registry = HarnessRegistry::new();

    let target_codex = Target::new(
        "codex",
        platform
            .project
            .as_ref()
            .unwrap()
            .join(".codex/config.toml"),
        Scope::Project,
        Format::Toml,
        "mcp_servers",
    );

    let managed = crate::config::Server {
        id: "3d35b34d-9b42-4742-89cc-11c23b7d15f1".into(),
        name: "Brightspace".into(),
        connection: crate::config::Connection::Stdio {
            executable: "npx".into(),
            arguments: vec!["-y".into(), "brightspace-mcp-server@latest".into()],
            directory: String::new(),
        },
        environment: Default::default(),
        secrets: vec!["D2L_BASE_URL".into()],
        auto_start: false,
        restart_on_failure: false,
        remote: Default::default(),
        commands: vec![],
        bridge_port: Some(52401),
    };

    let bridge_server = McpServer::from_managed_bridge(
        &managed,
        "http://127.0.0.1:52401/3d35b34d-9b42-4742-89cc-11c23b7d15f1/mcp",
    );
    let results = registry
        .install_server(
            &bridge_server,
            std::slice::from_ref(&target_codex),
            ConflictResolution::Overwrite,
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].action, TargetActionTaken::Installed));

    let content = std::fs::read_to_string(&target_codex.path).unwrap();
    assert!(content.contains("http://127.0.0.1:52401/3d35b34d-9b42-4742-89cc-11c23b7d15f1/mcp"));
    assert!(!content.contains("${D2L_BASE_URL}"));
}

#[test]
fn test_multiple_servers_codex_toml() {
    let temp = TempDir::new().unwrap();
    let platform = make_platform(&temp);
    let registry = HarnessRegistry::new();

    let target_codex = Target::new(
        "codex",
        platform
            .project
            .as_ref()
            .unwrap()
            .join(".codex/config.toml"),
        Scope::Project,
        Format::Toml,
        "mcp_servers",
    );

    let initial_toml = r#"
[mcp_servers]
Brightspace = { url = "http://127.0.0.1:40115/3d35b34d-9b42-4742-89cc-11c23b7d15f1/mcp" }

[mcp_servers.heroui-react]
enabled = false
"#;
    std::fs::create_dir_all(target_codex.path.parent().unwrap()).unwrap();
    std::fs::write(&target_codex.path, initial_toml).unwrap();

    let firefox = crate::config::Server {
        id: "61db6354-9754-4217-bb5d-c2241442eb1e".into(),
        name: "Firefox Devtools".into(),
        connection: crate::config::Connection::Stdio {
            executable: "npx".into(),
            arguments: vec!["@mozilla/firefox-devtools-mcp@latest".into()],
            directory: String::new(),
        },
        environment: Default::default(),
        secrets: vec![],
        auto_start: false,
        restart_on_failure: false,
        remote: Default::default(),
        commands: vec![],
        bridge_port: Some(41161),
    };

    let bridge_firefox = McpServer::from_managed_bridge(
        &firefox,
        "http://127.0.0.1:41161/61db6354-9754-4217-bb5d-c2241442eb1e/mcp",
    );

    let res = registry
        .install_server(
            &bridge_firefox,
            std::slice::from_ref(&target_codex),
            ConflictResolution::Overwrite,
        )
        .unwrap();
    assert_eq!(res.len(), 1);

    let content = std::fs::read_to_string(&target_codex.path).unwrap();
    assert!(content.contains("Brightspace = { url = \"http://127.0.0.1:40115/3d35b34d-9b42-4742-89cc-11c23b7d15f1/mcp\" }"));
    assert!(content.contains("\"Firefox Devtools\" = { url = \"http://127.0.0.1:41161/61db6354-9754-4217-bb5d-c2241442eb1e/mcp\" }"));
}
