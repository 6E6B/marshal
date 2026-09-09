use crate::{
    backend::{Manager, ProcessGroup, pump},
    config::{RemoteConfig, Server},
    secrets::SecretStore,
};
use anyhow::{Context, Result, bail};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub trait TunnelProvider {
    fn command(
        &self,
        server: &Server,
        endpoint: &str,
        token: &str,
        health: &str,
    ) -> Result<crate::host::Command>;
    fn health_path(&self) -> &str;
}
pub struct OpenAITunnelProvider;
pub struct NgrokProvider;
impl TunnelProvider for OpenAITunnelProvider {
    fn command(
        &self,
        server: &Server,
        endpoint: &str,
        token: &str,
        health: &str,
    ) -> Result<crate::host::Command> {
        RemoteConfig::validate_openai_tunnel_id(&server.remote.tunnel_id)?;
        let mut command = crate::host::tokio_command("tunnel-client");
        command
            .arg("run")
            .env("CONTROL_PLANE_API_KEY", token)
            .env("CONTROL_PLANE_TUNNEL_ID", &server.remote.tunnel_id)
            .env("CONTROL_PLANE_BASE_URL", "https://api.openai.com")
            .env("MCP_SERVER_URL", endpoint)
            .env("HEALTH_LISTEN_ADDR", health)
            .env("LOG_FORMAT", "json")
            .env("LOG_LEVEL", "info");
        Ok(command)
    }
    fn health_path(&self) -> &str {
        "/readyz"
    }
}
impl TunnelProvider for NgrokProvider {
    fn command(
        &self,
        server: &Server,
        endpoint: &str,
        token: &str,
        _health: &str,
    ) -> Result<crate::host::Command> {
        let url = reqwest::Url::parse(endpoint)?;
        let mut command = crate::host::tokio_command("ngrok");
        command
            .args([
                "http",
                &url.origin().ascii_serialization(),
                "--log",
                "stdout",
                "--log-format",
                "json",
            ])
            .env("NGROK_AUTHTOKEN", token);
        if !server.remote.domain.trim().is_empty() {
            command.arg("--url").arg(&server.remote.domain);
        }
        Ok(command)
    }
    fn health_path(&self) -> &str {
        "/api/tunnels"
    }
}

pub async fn run(manager: Arc<Manager>, server: &Server, cancel: CancellationToken) -> Result<()> {
    let provider: Box<dyn TunnelProvider + Send + Sync> = match server.remote.provider.as_str() {
        "openai" => Box::new(OpenAITunnelProvider),
        "ngrok" => Box::new(NgrokProvider),
        _ => bail!("Choose a tunnel provider"),
    };
    let credential_name = crate::config::RemoteConfig::credential_name(&server.remote.provider)
        .context("Choose a tunnel provider")?;
    let token = if SecretStore::contains(&server.id, credential_name).await? {
        SecretStore::get(&server.id, credential_name).await
    } else {
        // Configurations created before provider-specific credentials used this key.
        SecretStore::get(&server.id, "remote-token").await
    }
    .context("Save this provider’s credential in the keyring first")?;
    let endpoint = manager.bridge(&server.id, cancel.clone()).await?;
    // Provider agents require a fixed health port. Reserve it until immediately before spawn.
    let health_socket = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = health_socket.local_addr()?.to_string();
    let mut command = provider.command(server, &endpoint, &token, &address)?;
    // ngrok exposes web_addr through its agent configuration, not a CLI flag.
    // Keep the file under the app cache so a host ngrok can read it from Flatpak.
    let cache = directories::ProjectDirs::from("io", "marshal", "Marshal")
        .map(|dirs| dirs.cache_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&cache)?;
    let config_dir = tempfile::Builder::new()
        .prefix("ngrok-")
        .tempdir_in(&cache)?;
    if server.remote.provider == "ngrok" {
        let config = config_dir.path().join("ngrok.yml");
        tokio::fs::write(
            &config,
            format!("version: \"3\"\nagent:\n  web_addr: {address}\n"),
        )
        .await?;
        command.arg("--config").arg(config);
    }
    command
        .kill_on_drop(true)
        .process_group(0)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Drop handlers do not run if Marshal crashes or is killed. Do not leave a
    // second tunnel polling OpenAI with a bridge that no longer exists.
    #[cfg(target_os = "linux")]
    unsafe {
        let parent = libc::getpid();
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                libc::_exit(1);
            }
            Ok(())
        });
    }
    drop(health_socket);
    let mut child = command.spawn().context("Could not launch the tunnel client. Install tunnel-client or ngrok and make it available in PATH.")?;
    let pid = child.id().context("Tunnel process has no PID")?;
    let _group = ProcessGroup(pid);
    let stdout = tokio::spawn(pump(
        manager.clone(),
        server.id.clone(),
        "Tunnel",
        child.stdout.take().unwrap(),
        None,
        vec![token.clone()],
    ));
    let stderr = tokio::spawn(pump(
        manager.clone(),
        server.id.clone(),
        "Tunnel",
        child.stderr.take().unwrap(),
        None,
        vec![token],
    ));
    // Aborting these readers when the task is cancelled prevents detached readers keeping pipes open.
    struct Readers(tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>);
    impl Drop for Readers {
        fn drop(&mut self) {
            self.0.abort();
            self.1.abort();
        }
    }
    let _readers = Readers(stdout, stderr);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let mut timer = tokio::time::interval(Duration::from_secs(3));
    loop {
        tokio::select! {
            status = child.wait() => bail!("Tunnel client exited: {}", status?),
            _ = cancel.cancelled() => return Ok(()),
            _ = timer.tick() => {
                let response = client.get(format!("http://{address}{}", provider.health_path())).send().await;
                match response {
                    Ok(response) if response.status().is_success() => {
                        let remote = if server.remote.provider == "openai" { Some(server.remote.tunnel_id.clone()) }
                        else {
                            let data: serde_json::Value = response.json().await?;
                            data["tunnels"].as_array().and_then(|tunnels| tunnels.iter().find_map(|t| t["public_url"].as_str().filter(|url| url.starts_with("https://")).map(str::to_owned)))
                                .map(|url| format!("{}{}", url.trim_end_matches('/'), reqwest::Url::parse(&endpoint).unwrap().path()))
                        };
                        manager.update(&server.id, |s| { s.remote_state = if remote.is_some() { "Connected" } else { "Connecting" }.into(); s.endpoint = remote; });
                    }
                    _ => manager.update(&server.id, |s| { s.remote_state = "Waiting for Tunnel".into(); s.endpoint = None; }),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_commands_keep_credentials_out_of_arguments() {
        let mut server: Server = serde_json::from_value(serde_json::json!({
            "id": "fixture", "name": "Fixture",
            "connection": { "transport": "http", "url": "http://localhost:3000/mcp" }
        }))
        .unwrap();
        let endpoint = "http://127.0.0.1:1234/private-route/mcp";
        assert!(
            OpenAITunnelProvider
                .command(&server, endpoint, "secret", "127.0.0.1:1235")
                .is_err()
        );
        server.remote.tunnel_id = "tunnel_0123456789abcdef0123456789abcdef".into();
        let openai = OpenAITunnelProvider
            .command(&server, endpoint, "secret", "127.0.0.1:1235")
            .unwrap();
        let openai = openai.as_std();
        assert_eq!(openai.get_program(), "tunnel-client");
        assert_eq!(openai.get_args().collect::<Vec<_>>(), vec!["run"]);
        assert!(
            openai.get_envs().any(|(key, value)| key == "MCP_SERVER_URL"
                && value == Some(std::ffi::OsStr::new(endpoint)))
        );
        assert!(
            openai
                .get_envs()
                .any(|(key, value)| key == "CONTROL_PLANE_API_KEY"
                    && value == Some(std::ffi::OsStr::new("secret")))
        );
        let ngrok = NgrokProvider
            .command(&server, endpoint, "secret", "127.0.0.1:1235")
            .unwrap();
        let args: Vec<_> = ngrok.as_std().get_args().collect();
        assert_eq!(args[1], "http://127.0.0.1:1234");
        assert!(!args.iter().any(|arg| *arg == "secret"));
        assert!(!args.iter().any(|arg| *arg == "--web-addr"));
    }
}
