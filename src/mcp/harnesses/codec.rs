use super::*;
use crate::mcp::canonical::{ConfigValue, Transport};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub enum Expansion {
    None,
    Dollar,
    Colon,
    OpenCode,
}
impl Expansion {
    fn delimiters(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::None => None,
            Self::Dollar => Some(("${", "}")),
            Self::Colon => Some(("${env:", "}")),
            Self::OpenCode => Some(("{env:", "}")),
        }
    }
    pub fn render(self, v: &ConfigValue) -> Result<String> {
        match v {
            ConfigValue::Literal { value } => {
                if let Some((start, _)) = self.delimiters()
                    && value.contains(start)
                {
                    bail!(
                        "Literal contains interpolation syntax; use an explicit environment reference"
                    );
                }
                Ok(value.clone())
            }
            ConfigValue::Env { name } | ConfigValue::EnvTemplate { name, .. } => {
                let (start,end) = self.delimiters().context("This client cannot safely represent an environment reference; no secret was resolved or copied")?;
                let token = format!("{start}{name}{end}");
                if let ConfigValue::EnvTemplate { prefix, suffix, .. } = v {
                    Ok(format!("{prefix}{token}{suffix}"))
                } else {
                    Ok(token)
                }
            }
        }
    }
    pub fn read(self, v: &str) -> Result<ConfigValue> {
        if let Some((start, end)) = self.delimiters()
            && let Some((prefix, tail)) = v.split_once(start)
        {
            let (name, suffix) = tail
                .split_once(end)
                .context("Unclosed environment reference")?;
            if suffix.contains(start) || name.is_empty() || name.contains([':', '$', '{']) {
                bail!("Complex environment expression is not representable");
            }
            return Ok(if prefix.is_empty() && suffix.is_empty() {
                ConfigValue::Env { name: name.into() }
            } else {
                ConfigValue::EnvTemplate {
                    name: name.into(),
                    prefix: prefix.into(),
                    suffix: suffix.into(),
                }
            });
        }
        Ok(ConfigValue::Literal { value: v.into() })
    }
}
pub fn values(v: &BTreeMap<String, ConfigValue>, expansion: Expansion) -> Result<Value> {
    Ok(Value::Object(
        v.iter()
            .map(|(k, v)| Ok((k.clone(), Value::String(expansion.render(v)?))))
            .collect::<Result<_>>()?,
    ))
}
pub fn read_values(
    v: Option<&Value>,
    expansion: Expansion,
) -> Result<BTreeMap<String, ConfigValue>> {
    match v {
        None => Ok(BTreeMap::new()),
        Some(v) => v
            .as_object()
            .context("Expected environment/header object")?
            .iter()
            .map(|(k, v)| {
                Ok((
                    k.clone(),
                    expansion.read(
                        v.as_str()
                            .context("Environment/header value must be a string")?,
                    )?,
                ))
            })
            .collect(),
    }
}

/// A reusable field codec, selected explicitly by each independently versioned
/// adapter. It neither chooses paths nor edits files.
pub struct Fields {
    pub stdio_type: Option<&'static str>,
    pub http_type: Option<&'static str>,
    pub sse_type: Option<&'static str>,
    pub http_key: &'static str,
    pub sse_key: Option<&'static str>,
    pub command_key: &'static str,
    pub command_array: bool,
    pub env_key: &'static str,
    pub cwd: bool,
    pub expansion: Expansion,
    pub remote: bool,
}
impl Default for Fields {
    fn default() -> Self {
        Self {
            stdio_type: None,
            http_type: None,
            sse_type: None,
            http_key: "url",
            sse_key: None,
            command_key: "command",
            command_array: false,
            env_key: "env",
            cwd: false,
            expansion: Expansion::None,
            remote: true,
        }
    }
}
impl Fields {
    pub fn encode(&self, s: &McpServer) -> Result<Value> {
        s.validate()?;
        let mut v = json!({});
        match s.transport {
            Transport::Stdio => {
                if let Some(t) = self.stdio_type {
                    v["type"] = t.into();
                }
                if self.command_array {
                    v[self.command_key] =
                        std::iter::once(s.command.clone().expect("validated stdio command"))
                            .chain(s.args.clone())
                            .collect::<Vec<_>>()
                            .into();
                } else {
                    v[self.command_key] =
                        s.command.clone().expect("validated stdio command").into();
                    if !s.args.is_empty() {
                        v["args"] = s.args.clone().into();
                    }
                }
                if !s.env.is_empty() {
                    v[self.env_key] = values(&s.env, self.expansion)?;
                }
                if let Some(cwd) = &s.cwd {
                    if !self.cwd {
                        bail!("This client has no verified working-directory field");
                    }
                    v["cwd"] = cwd.clone().into();
                }
            }
            Transport::StreamableHttp | Transport::Sse => {
                if !self.remote {
                    bail!(
                        "Remote transport is not verified for this adapter; use the client's MCP settings"
                    );
                }
                let (key, ty) = if s.transport == Transport::Sse {
                    (
                        self.sse_key.context(
                            "Explicit SSE transport is not representable by this client",
                        )?,
                        self.sse_type,
                    )
                } else {
                    (self.http_key, self.http_type)
                };
                if let Some(t) = ty {
                    v["type"] = t.into();
                }
                v[key] = s.url.clone().expect("validated remote URL").into();
                if !s.headers.is_empty() {
                    v["headers"] = values(&s.headers, self.expansion)?;
                }
            }
        }
        Ok(v)
    }
    pub fn decode(&self, name: &str, v: &Value) -> Result<McpServer> {
        if !v.is_object() {
            bail!("MCP entry must be an object");
        }
        // These alter connection semantics and cannot be dropped during import.
        for key in [
            "envFile",
            "headersHelper",
            "authProviderType",
            "bearer_token_env_var",
            "oauth",
            "auth",
        ] {
            if v.get(key).is_some() {
                bail!(
                    "Connection uses client-owned authentication or expansion; inspect in the client"
                );
            }
        }
        let has_command = v.get(self.command_key).is_some();
        let has_remote =
            v.get(self.http_key).is_some() || self.sse_key.is_some_and(|k| v.get(k).is_some());
        if has_command == has_remote {
            bail!("Ambiguous or missing MCP transport");
        }
        let ty = v.get("type").and_then(Value::as_str);
        let mut s = if has_command {
            if ty.is_some() && ty != self.stdio_type {
                bail!("Unrecognized stdio transport type");
            }
            let mut s = McpServer::empty(name, Transport::Stdio);
            if self.command_array {
                let a: Vec<String> = serde_json::from_value(v[self.command_key].clone())
                    .context("Expected command array")?;
                s.command = a.first().cloned();
                s.args = a.into_iter().skip(1).collect();
            } else {
                s.command = Some(
                    v[self.command_key]
                        .as_str()
                        .context("Expected executable string")?
                        .into(),
                );
                if let Some(a) = v.get("args") {
                    s.args =
                        serde_json::from_value(a.clone()).context("Expected argument array")?;
                }
            }
            s.env = read_values(v.get(self.env_key), self.expansion)?;
            if let Some(c) = v.get("cwd") {
                if !self.cwd {
                    bail!("Unverified working-directory field");
                }
                s.cwd = Some(c.as_str().context("Expected directory string")?.into());
            }
            s
        } else {
            if !self.remote {
                bail!("Remote transport is not verified for this adapter");
            }
            let sse = self.sse_key.is_some_and(|k| {
                v.get(k).is_some() && (k != self.http_key || ty == self.sse_type && ty.is_some())
            });
            let expected = if sse { self.sse_type } else { self.http_type };
            if ty.is_some() && ty != expected {
                bail!("Unrecognized remote transport type");
            }
            let mut s = McpServer::empty(
                name,
                if sse {
                    Transport::Sse
                } else {
                    Transport::StreamableHttp
                },
            );
            s.url = Some(
                v[if sse {
                    self.sse_key.expect("SSE key selected")
                } else {
                    self.http_key
                }]
                .as_str()
                .context("Expected URL string")?
                .into(),
            );
            s.headers = read_values(v.get("headers"), self.expansion)?;
            s
        };
        if s.cwd.as_deref() == Some("") {
            s.cwd = None;
        }
        s.validate()?;
        Ok(s)
    }
}

pub const STANDARD_KEYS: &[&str] = &[
    "type",
    "command",
    "args",
    "cwd",
    "env",
    "url",
    "httpUrl",
    "serverUrl",
    "headers",
];
