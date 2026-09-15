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
        #[serde(default)]
        headers: BTreeMap<String, String>,
        /// Header names whose values live in the keyring as "header:{name}".
        #[serde(default)]
        secret_headers: Vec<String>,
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

    /// Keyring key under which the value of a secret HTTP header is stored.
    pub fn header_secret_name(header: &str) -> String {
        format!("header:{header}")
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
            Connection::Http { url, .. } => url.clone(),
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
            Connection::Http {
                url,
                headers,
                secret_headers,
            } => {
                let parsed =
                    reqwest::Url::parse(url).context("Enter a complete HTTP or HTTPS URL")?;
                if !["http", "https"].contains(&parsed.scheme()) || parsed.host_str().is_none() {
                    bail!("Use an HTTP or HTTPS endpoint");
                }
                if !parsed.username().is_empty() || parsed.password().is_some() {
                    bail!("Credentials cannot be stored in an endpoint URL");
                }
                for (key, value) in headers {
                    validate_header_name(key)?;
                    if value.contains(['\0', '\r', '\n']) {
                        bail!("Invalid value for header {key}");
                    }
                }
                let mut seen = std::collections::HashSet::new();
                for key in secret_headers {
                    validate_header_name(key)?;
                    if headers.contains_key(key) || !seen.insert(key) {
                        bail!("Duplicate header: {key}");
                    }
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
            if key.is_empty() || key.contains(['=', '\0', ':']) {
                bail!("Invalid environment variable name: {key}");
            }
        }
        Ok(())
    }
}

/// RFC 7230 token characters; header names are matched case-insensitively, so
/// keep them in the form the user entered and only reject invalid bytes.
fn validate_header_name(key: &str) -> Result<()> {
    let valid = !key.is_empty()
        && key.bytes().all(|b| {
            b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
        });
    if !valid {
        bail!("Invalid header name: {key}");
    }
    Ok(())
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

/// A named set of servers that can be started together.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub servers: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSettings {
    #[serde(default)]
    pub auto_start_login: bool,
    #[serde(default = "default_true")]
    pub run_in_background_on_close: bool,
    #[serde(default)]
    pub run_in_background_on_startup: bool,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    /// Desktop notifications for crashes and remote access events.
    #[serde(default = "default_true")]
    pub notifications: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            auto_start_login: false,
            run_in_background_on_close: true,
            run_in_background_on_startup: false,
            profiles: vec![],
            notifications: true,
        }
    }
}

impl AppSettings {
    pub fn can_run_in_background(&self) -> bool {
        self.run_in_background_on_close || self.run_in_background_on_startup
    }
}

pub struct SettingsRegistry {
    path: PathBuf,
}

impl SettingsRegistry {
    #[cfg(test)]
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn new() -> Self {
        let path = directories::ProjectDirs::from("io", "marshal", "Marshal")
            .expect("Configuration directory")
            .config_dir()
            .join("settings.json");
        Self { path }
    }

    pub fn load(&self) -> Result<AppSettings> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .context("Cannot read application settings; the original file has been preserved"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AppSettings::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, settings: &AppSettings) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let dir = self.path.parent().expect("settings.json path has a parent");
        std::fs::create_dir_all(dir)?;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        let temp = self
            .path
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(settings)?)?;
        file.sync_all()?;
        std::fs::rename(&temp, &self.path)?;
        let _ = std::fs::File::open(dir).and_then(|f| f.sync_all());
        Ok(())
    }
}

/// Directory where per-server logs persist across restarts.
pub fn logs_dir() -> PathBuf {
    directories::ProjectDirs::from("io", "marshal", "Marshal")
        .map(|dirs| dirs.data_dir().join("logs"))
        .unwrap_or_else(|| std::env::temp_dir().join("marshal-logs"))
}

pub fn autostart_dir() -> Option<PathBuf> {
    if crate::host::in_flatpak() {
        if let Some(host_config) = crate::host::host_env("XDG_CONFIG_HOME") {
            return Some(PathBuf::from(host_config).join("autostart"));
        }
        if let Some(host_home) = crate::host::host_env("HOME") {
            return Some(PathBuf::from(host_home).join(".config").join("autostart"));
        }
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config").join("autostart"))
    } else if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
        Some(PathBuf::from(xdg).join("autostart"))
    } else {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config").join("autostart"))
    }
}

pub fn autostart_desktop_path() -> Option<PathBuf> {
    autostart_dir().map(|dir| dir.join("io.github._6E6B.marshal.desktop"))
}

pub fn autostart_exec_command(run_in_background: bool) -> String {
    let bg_flag = if run_in_background { " --background" } else { "" };
    if crate::host::in_flatpak() {
        format!("flatpak run io.github._6E6B.marshal{bg_flag}")
    } else if crate::host::is_command_in_path("marshal") {
        format!("marshal{bg_flag}")
    } else if let Ok(exe) = std::env::current_exe() {
        format!("{}{bg_flag}", exe.display())
    } else {
        format!("marshal{bg_flag}")
    }
}

pub fn is_autostart_file_enabled(path: &PathBuf) -> bool {
    if !path.is_file() {
        return false;
    }
    if let Ok(content) = std::fs::read_to_string(path) {
        if content.lines().any(|l| {
            let t = l.trim();
            t == "Hidden=true" || t == "X-GNOME-Autostart-enabled=false"
        }) {
            return false;
        }
        return true;
    }
    false
}

pub fn is_autostart_enabled() -> bool {
    let Some(path) = autostart_desktop_path() else {
        return false;
    };
    is_autostart_file_enabled(&path)
}

pub fn sync_autostart(settings: &AppSettings) -> Result<()> {
    let Some(path) = autostart_desktop_path() else {
        bail!("Could not determine autostart directory");
    };
    sync_autostart_to_path(&path, settings)
}

pub fn sync_autostart_to_path(path: &PathBuf, settings: &AppSettings) -> Result<()> {
    if settings.auto_start_login {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let exec = autostart_exec_command(settings.run_in_background_on_startup);
        let content = format!(
            "[Desktop Entry]\nType=Application\nName=Marshal\nComment=Manage MCP servers\nExec={exec}\nIcon=io.github._6E6B.marshal\nTerminal=false\nCategories=Development;GTK;\nStartupNotify=false\nX-GNOME-Autostart-enabled=true\n"
        );
        std::fs::write(path, content)?;
    } else if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
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

    #[test]
    fn app_settings_defaults_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let registry = SettingsRegistry::at(dir.path().join("settings.json"));
        let default_settings = registry.load().unwrap();
        assert_eq!(default_settings, AppSettings::default());

        let custom = AppSettings {
            auto_start_login: true,
            run_in_background_on_close: false,
            run_in_background_on_startup: true,
            ..Default::default()
        };
        registry.save(&custom).unwrap();
        assert_eq!(registry.load().unwrap(), custom);

        // Deserializing empty JSON should use default values (especially run_in_background_on_close: true)
        let deserialized: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(deserialized, AppSettings::default());
    }

    #[test]
    fn autostart_sync_creation_and_removal() {
        let dir = tempfile::tempdir().unwrap();
        let autostart_file = dir.path().join("io.github._6E6B.marshal.desktop");

        let mut settings = AppSettings {
            auto_start_login: true,
            run_in_background_on_startup: true,
            ..Default::default()
        };
        sync_autostart_to_path(&autostart_file, &settings).unwrap();
        assert!(autostart_file.exists());
        let content = std::fs::read_to_string(&autostart_file).unwrap();
        assert!(content.contains("--background"));
        assert!(is_autostart_file_enabled(&autostart_file));

        // When run_in_background_on_startup is disabled
        settings.run_in_background_on_startup = false;
        sync_autostart_to_path(&autostart_file, &settings).unwrap();
        let content2 = std::fs::read_to_string(&autostart_file).unwrap();
        assert!(!content2.contains("--background"));

        // When disabled, file is removed
        settings.auto_start_login = false;
        sync_autostart_to_path(&autostart_file, &settings).unwrap();
        assert!(!autostart_file.exists());
        assert!(!is_autostart_file_enabled(&autostart_file));
    }
}
