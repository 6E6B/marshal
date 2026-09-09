use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum Connection {
    Stdio {
        executable: String,
        arguments: Vec<String>,
        directory: String,
    },
    Http {
        url: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct RemoteConfig {
    pub provider: String,
    pub tunnel_id: String,
    pub domain: String,
    #[serde(default)]
    pub start_with_server: bool,
}

impl RemoteConfig {
    pub fn credential_name(provider: &str) -> Option<&'static str> {
        match provider {
            "openai" => Some("remote-token-openai"),
            "ngrok" => Some("remote-token-ngrok"),
            _ => None,
        }
    }

    pub fn is_credential_name(name: &str) -> bool {
        matches!(
            name,
            "remote-token" | "remote-token-openai" | "remote-token-ngrok"
        )
    }

    pub fn validate_openai_tunnel_id(id: &str) -> Result<()> {
        if !id.starts_with("tunnel_")
            || id.len() != 39
            || !id[7..].chars().all(|c| c.is_ascii_hexdigit())
        {
            bail!("Enter the tunnel ID from OpenAI Platform settings");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CustomCommand {
    pub id: String,
    pub name: String,
    pub command: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub connection: Connection,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub restart_on_failure: bool,
    #[serde(default)]
    pub remote: RemoteConfig,
    #[serde(default)]
    pub commands: Vec<CustomCommand>,
    #[serde(default)]
    pub bridge_port: Option<u16>,
}

impl Server {
    pub fn command(text: &str) -> Result<Connection> {
        let words = shell_words::split(text).context("Check the quotes in the command")?;
        if words.is_empty() || words[0].is_empty() {
            bail!("Enter a launch command");
        }
        if words
            .iter()
            .any(|w| ["|", "||", "&&", ";", ">", "<", "&"].contains(&w.as_str()))
        {
            bail!(
                "Enter one executable and its arguments. Shell pipelines and redirection are not supported."
            );
        }
        Ok(Connection::Stdio {
            executable: words[0].clone(),
            arguments: words[1..].to_vec(),
            directory: String::new(),
        })
    }
    pub fn command_text(&self) -> String {
        match &self.connection {
            Connection::Stdio {
                executable,
                arguments,
                ..
            } => {
                if executable.is_empty() && arguments.is_empty() {
                    String::new()
                } else {
                    shell_words::join(std::iter::once(executable).chain(arguments))
                }
            }
            Connection::Http { url } => url.clone(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("Enter a server name");
        }
        let mut ids = std::collections::HashSet::new();
        for command in &self.commands {
            if command.id.is_empty() || !ids.insert(&command.id) {
                bail!("Custom commands must have unique IDs");
            }
            if command.name.trim().is_empty() || command.command.trim().is_empty() {
                bail!("Enter a name and command for each custom command");
            }
        }
        match &self.connection {
            Connection::Http { url } => {
                let parsed =
                    reqwest::Url::parse(url).context("Enter a complete HTTP or HTTPS URL")?;
                if !["http", "https"].contains(&parsed.scheme()) || parsed.host_str().is_none() {
                    bail!("Use an HTTP or HTTPS endpoint");
                }
                if !parsed.username().is_empty() || parsed.password().is_some() {
                    bail!("Credentials cannot be stored in an endpoint URL");
                }
            }
            Connection::Stdio {
                executable,
                directory,
                ..
            } => {
                if executable.is_empty() {
                    bail!("Enter an executable");
                }
                if !directory.is_empty() && !std::path::Path::new(directory).is_dir() {
                    bail!("The working directory does not exist");
                }
            }
        }
        for key in self.environment.keys().chain(self.secrets.iter()) {
            if key.is_empty() || key.contains(['=', '\0']) {
                bail!("Invalid environment variable name: {key}");
            }
        }
        Ok(())
    }
}

pub struct ServerRegistry {
    path: PathBuf,
}
impl ServerRegistry {
    #[cfg(test)]
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn new() -> Self {
        let path = directories::ProjectDirs::from("io", "marshal", "Marshal")
            .expect("Configuration directory")
            .config_dir()
            .join("servers.json");
        Self { path }
    }
    pub fn load(&self) -> Result<Vec<Server>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .context("Cannot read server definitions; the original file has been preserved"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(e) => Err(e.into()),
        }
    }
    pub fn save(&self, servers: &[Server]) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let dir = self.path.parent().expect("servers.json path has a parent");
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let temp = self
            .path
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(servers)?)?;
        file.sync_all()?;
        std::fs::rename(&temp, &self.path)?;
        std::fs::File::open(dir)?.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_credentials_are_provider_specific() {
        assert_eq!(
            RemoteConfig::credential_name("openai"),
            Some("remote-token-openai")
        );
        assert_eq!(
            RemoteConfig::credential_name("ngrok"),
            Some("remote-token-ngrok")
        );
        assert_ne!(
            RemoteConfig::credential_name("openai"),
            RemoteConfig::credential_name("ngrok")
        );
        assert!(RemoteConfig::is_credential_name("remote-token"));
        assert!(!RemoteConfig::is_credential_name("DATABASE_URL"));
        assert!(
            RemoteConfig::validate_openai_tunnel_id("tunnel_0123456789abcdef0123456789abcdef")
                .is_ok()
        );
        assert!(RemoteConfig::validate_openai_tunnel_id("tunnel_short").is_err());
    }

    #[test]
    fn older_remote_configuration_does_not_start_automatically() {
        let remote: RemoteConfig = serde_json::from_value(serde_json::json!({
            "provider": "ngrok",
            "tunnel_id": "",
            "domain": ""
        }))
        .unwrap();
        assert!(!remote.start_with_server);
    }

    #[test]
    fn parses_quoted_arguments_without_shell_expansion() {
        let Connection::Stdio {
            executable,
            arguments,
            ..
        } = Server::command("npx -y server '/a path' '$HOME' '$(touch /tmp/no)' ").unwrap()
        else {
            panic!()
        };
        assert_eq!(executable, "npx");
        assert_eq!(
            arguments,
            ["-y", "server", "/a path", "$HOME", "$(touch /tmp/no)"]
        );
        assert!(Server::command("a && b").is_err());
        assert!(Server::command("'unterminated").is_err());
    }
    #[test]
    fn legacy_servers_load_and_custom_commands_validate() {
        let mut server: Server = serde_json::from_value(serde_json::json!({
            "id": "legacy", "name": "Legacy", "connection": {
                "transport": "http", "url": "http://localhost:1234/mcp"
            }
        }))
        .unwrap();
        assert!(server.commands.is_empty());
        server.commands.push(CustomCommand {
            id: "test".into(),
            name: "Test".into(),
            command: "echo hello | cat".into(),
        });
        server.validate().unwrap();
        server.commands.push(server.commands[0].clone());
        assert!(server.validate().is_err());
        server.commands.pop();
        server.commands[0].command = " ".into();
        assert!(server.validate().is_err());
    }

    #[test]
    fn registry_round_trip_and_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let registry = ServerRegistry {
            path: dir.path().join("servers.json"),
        };
        assert!(registry.load().unwrap().is_empty());
        registry.save(&[]).unwrap();
        assert!(registry.load().unwrap().is_empty());
        std::fs::write(&registry.path, "broken").unwrap();
        assert!(registry.load().is_err());
        assert_eq!(std::fs::read_to_string(&registry.path).unwrap(), "broken");
    }
}
