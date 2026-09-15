use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    Stdio,
    StreamableHttp,
    Sse,
}

// Deliberately no Debug: a literal may contain a credential.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ConfigValue {
    Literal {
        value: String,
    },
    Env {
        name: String,
    },
    /// Required for Authorization: Bearer <environment reference>.
    EnvTemplate {
        name: String,
        prefix: String,
        suffix: String,
    },
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct McpServer {
    pub name: String,
    pub transport: Transport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, ConfigValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, ConfigValue>,
}

impl McpServer {
    pub fn empty(name: &str, transport: Transport) -> Self {
        Self {
            name: name.into(),
            transport,
            command: None,
            args: vec![],
            cwd: None,
            env: BTreeMap::new(),
            url: None,
            headers: BTreeMap::new(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty()
            || self.name.len() > 200
            || self.name.contains(['\0', '\n', '\r'])
        {
            bail!("Enter a nonempty server name without control characters (up to 200 bytes)");
        }
        match self.transport {
            Transport::Stdio => {
                if self
                    .command
                    .as_ref()
                    .is_none_or(|s| s.is_empty() || s.contains('\0'))
                {
                    bail!("A stdio server needs an executable");
                }
                if self.url.is_some() || !self.headers.is_empty() {
                    bail!("Stdio cannot use URL or headers");
                }
            }
            _ => {
                let url = reqwest::Url::parse(self.url.as_deref().unwrap_or_default())
                    .map_err(|_| anyhow::anyhow!("Invalid endpoint URL"))?;
                if !["http", "https"].contains(&url.scheme())
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    bail!("Use an HTTP(S) URL without embedded credentials");
                }
                if self.command.is_some()
                    || !self.args.is_empty()
                    || self.cwd.is_some()
                    || !self.env.is_empty()
                {
                    bail!(
                        "Remote servers cannot use executable, arguments, directory, or process environment"
                    );
                }
            }
        }
        for (key, value) in self.env.iter().chain(&self.headers) {
            if key.is_empty() || key.contains(['\0', '\n', '\r', '=']) {
                bail!("Invalid environment/header name");
            }
            match value {
                ConfigValue::Env { name } | ConfigValue::EnvTemplate { name, .. } => {
                    if name.is_empty()
                        || !name.bytes().enumerate().all(|(i, b)| {
                            b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit())
                        })
                    {
                        bail!("Invalid environment reference name");
                    }
                }
                ConfigValue::Literal { value } if value.contains('\0') => {
                    bail!("NUL is not allowed in configuration values")
                }
                _ => {}
            }
        }
        if self.args.iter().any(|s| s.contains('\0'))
            || self.cwd.as_ref().is_some_and(|s| s.contains('\0'))
        {
            bail!("NUL is not allowed in launch arguments");
        }
        Ok(())
    }
    pub fn equivalent(&self, other: &Self) -> bool {
        let mut other = other.clone();
        other.name.clone_from(&self.name);
        self == &other
    }
    /// Safe for conflict previews: connection arguments, headers, values and URL query are omitted.
    pub fn summary(&self) -> String {
        match self.transport {
            Transport::Stdio => format!(
                "stdio · {} · {} arguments",
                self.command
                    .as_deref()
                    .and_then(|s| std::path::Path::new(s).file_name())
                    .unwrap_or_default()
                    .to_string_lossy(),
                self.args.len()
            ),
            _ => self
                .url
                .as_ref()
                .and_then(|s| reqwest::Url::parse(s).ok())
                .map(|u| {
                    format!(
                        "{:?} · {}://{}",
                        self.transport,
                        u.scheme(),
                        u.host_str().unwrap_or_default()
                    )
                })
                .unwrap_or_else(|| "Remote endpoint".into()),
        }
    }
    pub fn from_managed(s: &crate::config::Server) -> Self {
        use crate::config::Connection;
        let mut out = match &s.connection {
            Connection::Stdio {
                executable,
                arguments,
                directory,
            } => {
                let mut out = Self::empty(&s.name, Transport::Stdio);
                out.command = Some(executable.clone());
                out.args = arguments.clone();
                out.cwd = (!directory.is_empty()).then(|| directory.clone());
                out.env = s
                    .environment
                    .iter()
                    .map(|(k, v)| (k.clone(), ConfigValue::Literal { value: v.clone() }))
                    .collect();
                out
            }
            Connection::Http {
                url,
                headers,
                secret_headers,
            } => {
                let mut out = Self::empty(&s.name, Transport::StreamableHttp);
                out.url = Some(url.clone());
                out.headers = headers
                    .iter()
                    .map(|(k, v)| (k.clone(), ConfigValue::Literal { value: v.clone() }))
                    .collect();
                // Placeholder: the keyring value is resolved before writing to a
                // client; without it only inequality can be detected.
                for name in secret_headers {
                    out.headers
                        .insert(name.clone(), ConfigValue::Env { name: name.clone() });
                }
                out
            }
        };
        if out.transport == Transport::Stdio {
            for key in &s.secrets {
                out.env
                    .insert(key.clone(), ConfigValue::Env { name: key.clone() });
            }
        }
        out
    }

    pub fn to_managed(&self) -> crate::config::Server {
        use crate::config::Connection;
        let connection = match self.transport {
            Transport::Stdio => Connection::Stdio {
                executable: self.command.clone().unwrap_or_default(),
                arguments: self.args.clone(),
                directory: self.cwd.clone().unwrap_or_default(),
            },
            Transport::StreamableHttp | Transport::Sse => {
                let mut headers = BTreeMap::new();
                for (k, v) in &self.headers {
                    match v {
                        ConfigValue::Literal { value } => {
                            headers.insert(k.clone(), value.clone());
                        }
                        ConfigValue::Env { name } => {
                            if let Ok(value) = std::env::var(name) {
                                headers.insert(k.clone(), value);
                            }
                        }
                        ConfigValue::EnvTemplate {
                            name,
                            prefix,
                            suffix,
                        } => {
                            if let Ok(value) = std::env::var(name) {
                                headers.insert(k.clone(), format!("{prefix}{value}{suffix}"));
                            }
                        }
                    }
                }
                Connection::Http {
                    url: self.url.clone().unwrap_or_default(),
                    headers,
                    secret_headers: vec![],
                }
            }
        };
        let mut environment = BTreeMap::new();
        let mut secrets = Vec::new();
        for (k, v) in &self.env {
            match v {
                ConfigValue::Literal { value } => {
                    environment.insert(k.clone(), value.clone());
                }
                ConfigValue::Env { name } | ConfigValue::EnvTemplate { name, .. } => {
                    secrets.push(name.clone());
                }
            }
        }
        crate::config::Server {
            id: uuid::Uuid::new_v4().to_string(),
            name: self.name.clone(),
            connection,
            environment,
            secrets,
            auto_start: false,
            restart_on_failure: false,
            remote: Default::default(),
            commands: vec![],
            bridge_port: None,
        }
    }

    pub fn from_managed_bridge(s: &crate::config::Server, bridge_url: &str) -> Self {
        let mut out = Self::empty(&s.name, Transport::StreamableHttp);
        out.url = Some(bridge_url.to_string());
        out
    }
}

impl std::fmt::Debug for ConfigValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigValue::Literal { .. } => write!(f, "Literal(***)"),
            ConfigValue::Env { name } => write!(f, "Env({name})"),
            ConfigValue::EnvTemplate { name, .. } => write!(f, "EnvTemplate({name})"),
        }
    }
}

impl std::fmt::Debug for McpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServer")
            .field("name", &self.name)
            .field("transport", &self.transport)
            .field("command", &self.command)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("env_count", &self.env.len())
            .field("url", &self.url)
            .field("headers_count", &self.headers.len())
            .finish()
    }
}
