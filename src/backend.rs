use crate::{
    config::{Connection, RemoteConfig, Server, ServerRegistry},
    host,
    secrets::SecretStore,
};
use anyhow::{Context, Result, anyhow, bail};
use rmcp::{
    RoleClient, RoleServer, Service, ServiceExt,
    model::*,
    service::{NotificationContext, Peer, RequestContext},
    transport::StreamableHttpClientTransport,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub state: String,
    pub error: Option<String>,
    pub pid: Option<u32>,
    pub since: Option<Instant>,
    pub exit_code: Option<i32>,
    pub info: Option<Value>,
    pub tools: Vec<Value>,
    pub resources: Vec<Value>,
    pub templates: Vec<Value>,
    pub prompts: Vec<Value>,
    pub logs: VecDeque<String>,
    pub revision: u64,
    pub remote_state: String,
    pub endpoint: Option<String>,
    pub local_endpoint: Option<String>,
    pub needs_restart: bool,
    pub commands: HashMap<String, String>,
}
impl Snapshot {
    pub fn active(&self) -> bool {
        ["Starting", "Running", "Restarting"].contains(&self.state.as_str())
    }
}
struct Job {
    cancel: CancellationToken,
    done: CancellationToken,
}
impl Job {
    fn new() -> Self {
        Self {
            cancel: CancellationToken::new(),
            done: CancellationToken::new(),
        }
    }
    async fn stop(self) {
        self.cancel.cancel();
        self.done.cancelled().await;
    }
}
async fn stop_job<K, Q>(jobs: &Mutex<HashMap<K, Job>>, key: &Q)
where
    K: Eq + Hash + std::borrow::Borrow<Q>,
    Q: Eq + Hash + ?Sized,
{
    let job = jobs.lock().unwrap().remove(key);
    if let Some(job) = job {
        job.stop().await;
    }
}
pub struct Manager {
    pub runtime: Arc<tokio::runtime::Runtime>,
    registry: ServerRegistry,
    definitions: Mutex<Vec<Server>>,
    snapshots: Mutex<HashMap<String, Snapshot>>,
    peers: Mutex<HashMap<String, Peer<RoleClient>>>,
    jobs: Mutex<HashMap<String, Job>>,
    tunnels: Mutex<HashMap<String, Job>>,
    local_bridges: Mutex<HashMap<String, (String, CancellationToken)>>,
    command_jobs: Mutex<HashMap<(String, String), Job>>,
    operations: tokio::sync::Mutex<()>,
    pub load_error: Option<String>,
}
impl Manager {
    pub fn new(runtime: Arc<tokio::runtime::Runtime>) -> Arc<Self> {
        Self::with_registry(runtime, ServerRegistry::new())
    }
    pub(crate) fn with_registry(
        runtime: Arc<tokio::runtime::Runtime>,
        registry: ServerRegistry,
    ) -> Arc<Self> {
        let loaded = registry.load();
        let error = loaded.as_ref().err().map(ToString::to_string);
        Arc::new(Self {
            runtime,
            registry,
            definitions: Mutex::new(loaded.unwrap_or_default()),
            snapshots: Mutex::new(HashMap::new()),
            peers: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(HashMap::new()),
            local_bridges: Mutex::new(HashMap::new()),
            command_jobs: Mutex::new(HashMap::new()),
            operations: tokio::sync::Mutex::new(()),
            load_error: error,
        })
    }
    pub fn servers(&self) -> Vec<Server> {
        self.definitions.lock().unwrap().clone()
    }
    pub fn server(&self, id: &str) -> Result<Server> {
        self.definitions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .ok_or_else(|| anyhow!("Server was removed"))
    }
    pub fn is_active(&self, id: &str) -> bool {
        self.snapshots
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(Snapshot::active)
    }
    pub fn snapshot(&self, id: &str) -> Snapshot {
        self.snapshots
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_else(|| Snapshot {
                state: "Stopped".into(),
                remote_state: "Local Only".into(),
                ..Default::default()
            })
    }
    pub(crate) fn update(&self, id: &str, f: impl FnOnce(&mut Snapshot)) {
        let mut states = self.snapshots.lock().unwrap();
        let state = states.entry(id.into()).or_insert_with(|| Snapshot {
            state: "Stopped".into(),
            remote_state: "Local Only".into(),
            ..Default::default()
        });
        f(state);
        state.revision += 1;
    }
    pub(crate) fn log(&self, id: &str, source: &str, message: &str) {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            % 86400;
        self.update(id, |s| {
            for line in message.lines().take(32) {
                s.logs.push_back(format!(
                    "{:02}:{:02}:{:02}  {source}  {}",
                    stamp / 3600,
                    stamp / 60 % 60,
                    stamp % 60,
                    line.chars().take(4096).collect::<String>()
                ));
            }
            while s.logs.len() > 1000 {
                s.logs.pop_front();
            }
        });
    }
    pub async fn save(&self, server: Server, secret_changes: Vec<(String, String)>) -> Result<()> {
        let _guard = self.operations.lock().await;
        if self.load_error.is_some() {
            bail!("Resolve the configuration file error before saving");
        }
        server.validate()?;
        let secrets_changed = secret_changes
            .iter()
            .any(|(key, _)| !RemoteConfig::is_credential_name(key));
        for (key, value) in secret_changes {
            SecretStore::set(&server.id, &key, &value)
                .await
                .context("Could not save to Secret Service")?;
        }
        let mut definitions = self.servers();
        let removed: Vec<_> = definitions
            .iter()
            .find(|s| s.id == server.id)
            .map(|s| {
                s.secrets
                    .iter()
                    .filter(|key| !server.secrets.contains(key))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let server_id = server.id.clone();
        if let Some(old) = definitions.iter_mut().find(|s| s.id == server.id) {
            let live_port = old.bridge_port;
            let changed = old.connection != server.connection
                || old.environment != server.environment
                || old.secrets != server.secrets
                || secrets_changed;
            if changed && self.snapshot(&server.id).active() {
                self.update(&server.id, |s| s.needs_restart = true);
            }
            *old = server;
            old.bridge_port = live_port;
        } else {
            definitions.push(server);
        }
        self.registry.save(&definitions)?;
        *self.definitions.lock().unwrap() = definitions;
        for key in removed {
            SecretStore::delete(&server_id, &key).await.context(
                "Configuration saved, but an old secret could not be removed from the keyring",
            )?;
        }
        Ok(())
    }
    pub async fn remove(self: &Arc<Self>, id: &str) -> Result<()> {
        let _guard = self.operations.lock().await;
        if self.load_error.is_some() {
            bail!("The configuration file could not be loaded");
        }
        self.stop_commands(id).await;
        self.stop_inner(id).await;
        let server = self.server(id)?;
        let mut definitions = self.servers();
        definitions.retain(|s| s.id != id);
        self.registry.save(&definitions)?;
        *self.definitions.lock().unwrap() = definitions;
        for key in server.secrets.iter().map(String::as_str).chain([
            "remote-token",
            "remote-token-openai",
            "remote-token-ngrok",
        ]) {
            if let Err(e) = SecretStore::delete(id, key).await {
                self.log(id, "Keyring", &e.to_string());
            }
        }
        Ok(())
    }
    pub async fn run_command(self: &Arc<Self>, id: &str, command_id: &str) -> Result<()> {
        let _guard = self.operations.lock().await;
        let server = self.server(id)?;
        let command = server
            .commands
            .iter()
            .find(|c| c.id == command_id)
            .cloned()
            .ok_or_else(|| anyhow!("Command was removed"))?;
        let key = (id.to_owned(), command_id.to_owned());
        let job = Job::new();
        let cancel = job.cancel.clone();
        let done = job.done.clone();
        {
            let mut jobs = self.command_jobs.lock().unwrap();
            if jobs.get(&key).is_some_and(|job| !job.done.is_cancelled()) {
                bail!("This command is already running");
            }
            jobs.insert(key, job);
        }
        self.update(id, |s| {
            s.commands.insert(command_id.into(), "Running".into());
        });
        let manager = self.clone();
        self.runtime.spawn(async move {
            let result = manager.execute_command(&server, &command, &cancel).await;
            let state = match result {
                Ok(state) => state,
                Err(error) => format!("Failed: {error:#}"),
            };
            manager.log(&server.id, &command.name, &state);
            manager.update(&server.id, |s| {
                s.commands.insert(command.id, state);
            });
            done.cancel();
        });
        Ok(())
    }

    pub async fn stop_command(&self, id: &str, command_id: &str) -> Result<()> {
        let _guard = self.operations.lock().await;
        stop_job(&self.command_jobs, &(id.to_owned(), command_id.to_owned())).await;
        Ok(())
    }

    async fn stop_commands(&self, id: &str) {
        let jobs: Vec<_> = {
            let mut jobs = self.command_jobs.lock().unwrap();
            let keys: Vec<_> = jobs
                .keys()
                .filter(|(server, _)| server == id)
                .cloned()
                .collect();
            keys.into_iter()
                .filter_map(|key| jobs.remove(&key))
                .collect()
        };
        for job in jobs {
            job.stop().await;
        }
    }

    async fn execute_command(
        self: &Arc<Self>,
        server: &Server,
        custom: &crate::config::CustomCommand,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let mut command = host::tokio_command("/bin/sh");
        command
            .args(["-c", &custom.command])
            .envs(&server.environment)
            .kill_on_drop(true)
            .process_group(0)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Connection::Stdio { directory, .. } = &server.connection
            && !directory.is_empty()
        {
            command.current_dir(directory);
        }
        let mut redactions = vec![];
        for key in &server.secrets {
            let value = tokio::select! {
                _ = cancel.cancelled() => return Ok("Stopped".into()),
                value = SecretStore::get(&server.id, key) => value?,
            };
            command.env(key, &value);
            redactions.push(value);
        }
        if cancel.is_cancelled() {
            return Ok("Stopped".into());
        }
        let mut child = command.spawn().context("Could not launch custom command")?;
        let pid = child.id().context("Child process has no PID")?;
        let group = ProcessGroup(pid);
        self.log(&server.id, &custom.name, "Started");
        // Keep readers in this future so cancellation drops them with the process.
        let stdout = pump(
            self.clone(),
            server.id.clone(),
            format!("{} stdout", custom.name),
            child.stdout.take().unwrap(),
            None,
            redactions.clone(),
        );
        let stderr = pump(
            self.clone(),
            server.id.clone(),
            format!("{} stderr", custom.name),
            child.stderr.take().unwrap(),
            None,
            redactions,
        );
        let wait = async {
            let status = tokio::select! {
                status = child.wait() => status?,
                _ = cancel.cancelled() => {
                    group.kill();
                    child.wait().await?;
                    return Ok::<_, anyhow::Error>("Stopped".to_owned());
                }
            };
            group.kill();
            Ok(if status.success() {
                "Completed".into()
            } else {
                format!("Failed: {status}")
            })
        };
        let (result, (), ()) = tokio::join!(wait, stdout, stderr);
        result
    }

    pub async fn start(self: &Arc<Self>, id: &str) -> Result<()> {
        let _guard = self.operations.lock().await;
        self.start_inner(id).await
    }
    async fn start_inner(self: &Arc<Self>, id: &str) -> Result<()> {
        let server = self.server(id)?;
        if self
            .jobs
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|j| !j.done.is_cancelled())
        {
            return Ok(());
        }
        let job = Job::new();
        let cancel = job.cancel.clone();
        let done = job.done.clone();
        self.jobs.lock().unwrap().insert(id.into(), job);
        self.update(id, |s| {
            s.state = "Starting".into();
            s.error = None;
            s.info = None;
            s.tools.clear();
            s.resources.clear();
            s.templates.clear();
            s.prompts.clear();
            s.needs_restart = false;
        });
        let manager = self.clone();
        tokio::spawn(async move {
            let mut attempts = 0;
            loop {
                let result = manager.run(&server, &cancel).await;
                manager.peers.lock().unwrap().remove(&server.id);
                manager.stop_tunnel_inner(&server.id).await;
                manager.stop_local_bridge(&server.id);
                if cancel.is_cancelled() {
                    manager.update(&server.id, |s| {
                        s.state = "Stopped".into();
                        s.error = None;
                        s.pid = None;
                        s.since = None;
                    });
                    break;
                }
                let message = result
                    .err()
                    .map(|e| format!("{e:#}"))
                    .unwrap_or_else(|| "The MCP connection closed".into());
                manager.log(&server.id, "MCP", &message);
                manager.update(&server.id, |s| {
                    s.state = "Failed".into();
                    s.error = Some(message);
                    s.pid = None;
                    s.since = None;
                });
                if !server.restart_on_failure
                    || matches!(server.connection, Connection::Http { .. })
                    || attempts >= 5
                {
                    break;
                }
                attempts += 1;
                manager.update(&server.id, |s| s.state = "Restarting".into());
                tokio::select! {
                    _ = cancel.cancelled() => {
                        manager.update(&server.id, |s| s.state = "Stopped".into());
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(2u64.pow(attempts))) => {}
                }
            }
            done.cancel();
        });
        Ok(())
    }
    pub async fn stop(self: &Arc<Self>, id: &str) -> Result<()> {
        let _guard = self.operations.lock().await;
        self.stop_inner(id).await;
        Ok(())
    }
    async fn stop_inner(&self, id: &str) {
        self.stop_tunnel_inner(id).await;
        self.stop_local_bridge(id);
        stop_job(&self.jobs, id).await;
    }
    pub async fn restart(self: &Arc<Self>, id: &str) -> Result<()> {
        let _guard = self.operations.lock().await;
        self.stop_inner(id).await;
        self.start_inner(id).await
    }
    pub async fn shutdown(self: &Arc<Self>) -> Result<()> {
        let _guard = self.operations.lock().await;
        for s in self.servers() {
            self.stop_commands(&s.id).await;
            self.stop_inner(&s.id).await;
        }
        Ok(())
    }
    async fn run(self: &Arc<Self>, server: &Server, cancel: &CancellationToken) -> Result<()> {
        let id = &server.id;
        let mut child = None;
        let mut process_group = None;
        let mut pumps = Vec::new();
        let session_result = async {
            let session = match &server.connection {
                Connection::Stdio {
                    executable,
                    arguments,
                    directory,
                } => {
                    let mut command = host::tokio_command(executable);
                    command
                        .args(arguments)
                        .envs(&server.environment)
                        .kill_on_drop(true)
                        .process_group(0)
                        .stdin(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped());
                    if !directory.is_empty() {
                        command.current_dir(directory);
                    }
                    let mut redactions = vec![];
                    for key in &server.secrets {
                        let value = SecretStore::get(id, key)
                            .await
                            .context("Could not read a server secret")?;
                        command.env(key, &value);
                        redactions.push(value);
                    }
                    let mut process = command.spawn().with_context(|| format!("Could not launch {executable}. Check that it is installed and available in PATH."))?;
                    let pid = process.id().context("Child process has no PID")?;
                    process_group = Some(ProcessGroup(pid));
                    self.update(id, |s| {
                        s.pid = Some(pid);
                        s.exit_code = None;
                    });
                    self.log(id, "Manager", &format!("Started process {pid}"));
                    let stdin = process.stdin.take().unwrap();
                    let stdout = process.stdout.take().unwrap();
                    let stderr = process.stderr.take().unwrap();
                    child = Some(process);
                    let (read, write) = tokio::io::duplex(65536);
                    pumps.push(tokio::spawn(pump(
                        self.clone(),
                        id.clone(),
                        "stdout",
                        stdout,
                        Some(write),
                        redactions.clone(),
                    )));
                    pumps.push(tokio::spawn(pump(
                        self.clone(),
                        id.clone(),
                        "stderr",
                        stderr,
                        None,
                        redactions,
                    )));
                    ().serve((read, stdin))
                        .await
                        .context("MCP initialization failed")?
                }
                Connection::Http { url } => ()
                    .serve(StreamableHttpClientTransport::from_uri(url.clone()))
                    .await
                    .context("Could not connect to the HTTP MCP endpoint")?,
            };
            Ok::<_, anyhow::Error>(session)
        };
        let session = tokio::select! {
            _ = cancel.cancelled() => Err(anyhow!("Cancelled")),
            result = tokio::time::timeout(Duration::from_secs(45), session_result) => result.context("Initialization timed out after 45 seconds").and_then(|r| r),
        };
        let result = match session {
            Err(e) => Err(e),
            Ok(session) => {
                self.peers
                    .lock()
                    .unwrap()
                    .insert(id.clone(), session.peer().clone());
                self.update(id, |s| {
                    s.info = session
                        .peer_info()
                        .and_then(|v| serde_json::to_value(v).ok());
                    s.state = "Running".into();
                    s.error = None;
                    s.since = Some(Instant::now());
                });
                let bridge_cancel = cancel.child_token();
                let manager = self.clone();
                let bridge_id = id.clone();
                tokio::spawn(async move {
                    match manager.start_local_bridge(&bridge_id, bridge_cancel).await {
                        Ok(endpoint) => {
                            manager.log(
                                &bridge_id,
                                "Bridge",
                                &format!("Local bridge available at {endpoint}"),
                            );
                        }
                        Err(e) => {
                            manager.log(
                                &bridge_id,
                                "Bridge",
                                &format!("Could not start local bridge: {e:#}"),
                            );
                        }
                    }
                });
                if server.remote.start_with_server {
                    // The bridge needs the initialized peer, so remote access follows a
                    // successful MCP connection rather than the process launch itself.
                    // Run this outside the server job so Stop can keep exclusive ownership
                    // of the lifecycle lock while it waits for this job to finish.
                    let manager = self.clone();
                    let id = id.clone();
                    tokio::spawn(async move {
                        let _ = manager.start_tunnel(&id).await;
                    });
                }
                let discovery = tokio::select! { _ = cancel.cancelled() => Ok(()), result = self.discover(id) => result };
                if let Err(e) = discovery {
                    self.log(id, "Discovery", &e.to_string());
                    self.update(id, |s| s.error = Some(format!("Discovery incomplete: {e}")));
                }
                let token = session.cancellation_token();
                tokio::select! {
                    _ = cancel.cancelled() => { token.cancel(); Ok(()) },
                    result = session.waiting() => { Err(anyhow!("MCP disconnected: {:?}", result)) },
                    exit = async { if let Some(p) = child.as_mut() { p.wait().await } else { std::future::pending().await } } => {
                        token.cancel();
                        if let Ok(status) = &exit { self.update(id, |s| s.exit_code = status.code()); }
                        Err(anyhow!("Process exited: {:?}", exit))
                    }
                }
            }
        };
        if let Some(mut process) = child {
            if let Some(group) = &process_group {
                group.terminate();
            }
            let status = match tokio::time::timeout(Duration::from_secs(3), process.wait()).await {
                Ok(status) => status,
                Err(_) => {
                    if let Some(group) = &process_group {
                        group.kill();
                    }
                    process.wait().await
                }
            };
            if let Ok(status) = status {
                self.update(id, |s| s.exit_code = status.code());
            }
        }
        drop(process_group);
        for pump in pumps {
            pump.abort();
        }
        self.log(id, "Manager", "Connection closed");
        result
    }
    pub async fn request(&self, id: &str, method: &str, params: Value) -> Result<Value> {
        let peer = self
            .peers
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("Start the server to use this feature"))?;
        let request: ClientRequest =
            serde_json::from_value(json!({"method": method, "params": params}))?;
        let result = tokio::time::timeout(Duration::from_secs(60), peer.send_request(request))
            .await
            .context("MCP request timed out")??;
        Ok(serde_json::to_value(result)?)
    }
    pub async fn discover(&self, id: &str) -> Result<()> {
        let snapshot = self.snapshot(id);
        let capabilities = &snapshot
            .info
            .as_ref()
            .ok_or_else(|| anyhow!("Not connected"))?["capabilities"];
        for (capability, method, field) in [
            ("tools", "tools/list", "tools"),
            ("resources", "resources/list", "resources"),
            ("resources", "resources/templates/list", "resourceTemplates"),
            ("prompts", "prompts/list", "prompts"),
        ] {
            if capabilities.get(capability).is_none() {
                continue;
            }
            let mut items = Vec::new();
            let mut cursor = None;
            let mut seen = std::collections::HashSet::new();
            loop {
                let result = self
                    .request(
                        id,
                        method,
                        cursor.as_ref().map_or(json!({}), |c| json!({"cursor": c})),
                    )
                    .await?;
                if let Some(values) = result[field].as_array() {
                    items.extend(values.iter().cloned());
                }
                cursor = result["nextCursor"].as_str().map(str::to_owned);
                if cursor.is_none() {
                    break;
                }
                if items.len() > 10000 || !seen.insert(cursor.clone()) {
                    bail!("Invalid or excessive discovery pagination");
                }
            }
            self.update(id, |s| match field {
                "tools" => s.tools = items,
                "resources" => s.resources = items,
                "resourceTemplates" => s.templates = items,
                _ => s.prompts = items,
            });
        }
        self.log(id, "Manager", "Capabilities discovered");
        Ok(())
    }

    pub async fn start_tunnel(self: &Arc<Self>, id: &str) -> Result<()> {
        let _guard = self.operations.lock().await;
        self.stop_tunnel_inner(id).await;
        let server = self.server(id)?;
        if !self.snapshot(id).active() || !self.peers.lock().unwrap().contains_key(id) {
            bail!("Start the server before enabling remote access");
        }
        let job = Job::new();
        let cancel = job.cancel.clone();
        let done = job.done.clone();
        self.tunnels.lock().unwrap().insert(id.into(), job);
        self.update(id, |s| {
            s.remote_state = "Starting".into();
            s.endpoint = None;
        });
        let manager = self.clone();
        tokio::spawn(async move {
            let result = tokio::select! { _ = cancel.cancelled() => Ok(()), result = crate::tunnel::run(manager.clone(), &server, cancel.clone()) => result };
            if let Err(e) = result {
                manager.log(&server.id, "Tunnel", &format!("{e:#}"));
                manager.update(&server.id, |s| {
                    s.remote_state = format!("Failed: {e:#}");
                    s.endpoint = None;
                });
            } else {
                manager.update(&server.id, |s| {
                    s.remote_state = "Local Only".into();
                    s.endpoint = None;
                });
            }
            cancel.cancel();
            done.cancel();
        });
        Ok(())
    }
    pub async fn stop_tunnel(&self, id: &str) -> Result<()> {
        let _guard = self.operations.lock().await;
        self.stop_tunnel_inner(id).await;
        Ok(())
    }
    async fn stop_tunnel_inner(&self, id: &str) {
        stop_job(&self.tunnels, id).await;
    }
    pub async fn bridge_with_port(
        self: &Arc<Self>,
        id: &str,
        preferred_port: Option<u16>,
        path: &str,
        cancel: CancellationToken,
    ) -> Result<String> {
        use rmcp::transport::streamable_http_server::{
            StreamableHttpService, session::local::LocalSessionManager,
        };
        let peer = self
            .peers
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("MCP is not connected"))?;

        let listener = if let Some(port) = preferred_port
            && port != 0
        {
            let mut bound = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await;
            if bound.is_err() {
                tokio::time::sleep(Duration::from_millis(150)).await;
                bound = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await;
            }
            match bound {
                Ok(l) => l,
                Err(_) => tokio::net::TcpListener::bind("127.0.0.1:0").await?,
            }
        } else {
            tokio::net::TcpListener::bind("127.0.0.1:0").await?
        };

        let address = listener.local_addr()?;
        let service = StreamableHttpService::new(
            move || Ok(Bridge { peer: peer.clone() }),
            Arc::new(LocalSessionManager::default()),
            Default::default(),
        );
        let router = axum::Router::new().nest(
            path,
            axum::Router::new()
                .fallback_service(service)
                .layer(axum::middleware::from_fn(bridge_compatibility)),
        );
        tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(cancel.cancelled_owned())
                .await;
        });
        Ok(format!("http://{address}{path}"))
    }

    pub async fn bridge(self: &Arc<Self>, id: &str, cancel: CancellationToken) -> Result<String> {
        let path = format!("/{}/mcp", uuid::Uuid::new_v4());
        self.bridge_with_port(id, None, &path, cancel).await
    }

    pub async fn start_local_bridge(
        self: &Arc<Self>,
        id: &str,
        cancel: CancellationToken,
    ) -> Result<String> {
        let server = self.server(id)?;
        let path = format!("/{}/mcp", server.id);
        let endpoint = self
            .bridge_with_port(id, server.bridge_port, &path, cancel.clone())
            .await?;
        let port = reqwest::Url::parse(&endpoint)?.port();
        self.remember_bridge_port(id, port);
        self.local_bridges
            .lock()
            .unwrap()
            .insert(id.to_string(), (endpoint.clone(), cancel));
        self.update(id, |s| s.local_endpoint = Some(endpoint.clone()));
        Ok(endpoint)
    }

    pub async fn ensure_local_bridge(self: &Arc<Self>, id: &str) -> Result<String> {
        if let Some((url, cancel)) = self.local_bridges.lock().unwrap().get(id)
            && !cancel.is_cancelled()
        {
            return Ok(url.clone());
        }
        self.stop_local_bridge(id);
        let cancel = CancellationToken::new();
        self.start_local_bridge(id, cancel).await
    }

    fn remember_bridge_port(self: &Arc<Self>, id: &str, port: Option<u16>) {
        {
            let mut defs = self.definitions.lock().unwrap();
            let Some(server) = defs.iter_mut().find(|s| s.id == id) else {
                return;
            };
            if server.bridge_port == port {
                return;
            }
            server.bridge_port = port;
        }
        let manager = self.clone();
        let id = id.to_owned();
        self.runtime.spawn(async move {
            let _guard = manager.operations.lock().await;
            let mut defs = manager.servers();
            let Some(server) = defs.iter_mut().find(|s| s.id == id) else {
                return;
            };
            server.bridge_port = port;
            if let Err(e) = manager.registry.save(&defs) {
                manager.log(
                    &id,
                    "Bridge",
                    &format!("Could not persist bridge port: {e:#}"),
                );
                return;
            }
            *manager.definitions.lock().unwrap() = defs;
        });
    }

    pub fn stop_local_bridge(&self, id: &str) {
        if let Some((_, cancel)) = self.local_bridges.lock().unwrap().remove(id) {
            cancel.cancel();
        }
        self.update(id, |s| s.local_endpoint = None);
    }
}

async fn bridge_compatibility(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::{http::StatusCode, response::IntoResponse};
    if req.headers().contains_key("origin") {
        return StatusCode::FORBIDDEN.into_response();
    }
    if req.method() != axum::http::Method::POST {
        return next.run(req).await;
    }
    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, 32 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    // Modern clients probe before initializing. rmcp 0.8 only supports the
    // legacy lifecycle; a JSON-RPC method-not-found response allows fallback.
    // Tunnel probes may omit/mislabel Content-Type, so recognize this one
    // method before rmcp's MIME validation without relaxing other requests.
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && value["jsonrpc"] == "2.0"
        && value["method"] == "server/discover"
        && (value["id"].is_string() || value["id"].is_number())
    {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({
                "jsonrpc": "2.0", "id": value["id"],
                "error": {"code": -32601, "message": "Method not found"}
            })),
        )
            .into_response();
    }
    next.run(axum::extract::Request::from_parts(parts, bytes.into()))
        .await
}

pub(crate) struct ProcessGroup(pub u32);
impl ProcessGroup {
    pub fn terminate(&self) {
        if self.0 == 0 {
            return;
        }
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGTERM);
        }
    }
    pub fn kill(&self) {
        if self.0 == 0 {
            return;
        }
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
    }
}

pub(crate) async fn pump<R: tokio::io::AsyncRead + Unpin>(
    manager: Arc<Manager>,
    id: String,
    source: impl AsRef<str>,
    mut reader: R,
    mut forward: Option<tokio::io::DuplexStream>,
    secrets: Vec<String>,
) {
    let mut bytes = [0; 8192];
    let mut pending = Vec::new();
    loop {
        let count = match reader.read(&mut bytes).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if let Some(writer) = &mut forward
            && writer.write_all(&bytes[..count]).await.is_err()
        {
            break;
        }
        pending.extend_from_slice(&bytes[..count]);
        manager.log(
            &id,
            source.as_ref(),
            &redact_chunk(&mut pending, &secrets, false),
        );
    }
    if !pending.is_empty() {
        manager.log(
            &id,
            source.as_ref(),
            &redact_chunk(&mut pending, &secrets, true),
        );
    }
}

fn redact_chunk(pending: &mut Vec<u8>, secrets: &[String], eof: bool) -> String {
    let keep = secrets.iter().map(String::len).max().unwrap_or(0);
    let safe_end = if eof {
        pending.len()
    } else {
        pending.len().saturating_sub(keep)
    };
    let mut output = Vec::new();
    let mut i = 0;
    while i < safe_end {
        if let Some(secret) = secrets
            .iter()
            .filter(|s| !s.is_empty())
            .filter(|s| pending[i..].starts_with(s.as_bytes()))
            .max_by_key(|s| s.len())
        {
            output.extend_from_slice(b"[redacted]");
            i += secret.len();
        } else {
            output.push(pending[i]);
            i += 1;
        }
    }
    pending.drain(..i);
    String::from_utf8_lossy(&output).into_owned()
}

#[derive(Clone)]
struct Bridge {
    peer: Peer<RoleClient>,
}
impl Service<RoleServer> for Bridge {
    async fn handle_request(
        &self,
        request: ClientRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ServerResult, rmcp::ErrorData> {
        if let ClientRequest::InitializeRequest(request) = request {
            context.peer.set_peer_info(request.params);
            return Ok(ServerResult::InitializeResult(self.get_info()));
        }
        tokio::time::timeout(Duration::from_secs(60), self.peer.send_request(request))
            .await
            .map_err(|_| rmcp::ErrorData::internal_error("Upstream timed out", None))?
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))
    }
    async fn handle_notification(
        &self,
        _notification: ClientNotification,
        _context: NotificationContext<RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        Ok(())
    }
    fn get_info(&self) -> ServerInfo {
        let mut info = self.peer.peer_info().cloned().unwrap_or_default();
        // The shared upstream has no per-client roots, subscriptions, sampling or elicitation.
        if let Some(tools) = info.capabilities.tools.as_mut() {
            tools.list_changed = None;
        }
        if let Some(resources) = info.capabilities.resources.as_mut() {
            resources.list_changed = None;
            resources.subscribe = None;
        }
        if let Some(prompts) = info.capabilities.prompts.as_mut() {
            prompts.list_changed = None;
        }
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secrets_are_redacted_across_read_boundaries() {
        let secrets = vec!["private-token".to_owned(), "private-token-long".to_owned()];
        for split in 0..=30 {
            let input = b"before private-token-long after";
            let mut pending = input[..split].to_vec();
            let mut output = redact_chunk(&mut pending, &secrets, false);
            pending.extend_from_slice(&input[split..]);
            output.push_str(&redact_chunk(&mut pending, &secrets, true));
            assert_eq!(output, "before [redacted] after");
            assert!(pending.is_empty());
        }
    }
    fn setup() -> (Arc<Manager>, tempfile::TempDir, Server) {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
        let m = Arc::new(Manager {
            runtime,
            registry: ServerRegistry::at(dir.path().join("servers.json")),
            definitions: Mutex::new(vec![]),
            snapshots: Mutex::new(HashMap::new()),
            peers: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
            tunnels: Mutex::new(HashMap::new()),
            local_bridges: Mutex::new(HashMap::new()),
            command_jobs: Mutex::new(HashMap::new()),
            operations: tokio::sync::Mutex::new(()),
            load_error: None,
        });
        let server = Server {
            id: "fixture".into(),
            name: "Fixture".into(),
            connection: Connection::Stdio {
                executable: "python3".into(),
                arguments: vec![format!(
                    "{}/tests/fixtures/mcp_server.py",
                    env!("CARGO_MANIFEST_DIR")
                )],
                directory: String::new(),
            },
            environment: Default::default(),
            secrets: vec![],
            auto_start: false,
            restart_on_failure: false,
            remote: Default::default(),
            commands: vec![],
            bridge_port: None,
        };
        (m, dir, server)
    }
    async fn ready(m: &Manager, id: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let s = m.snapshot(id);
                assert_ne!(s.state, "Failed", "{:?}", s.error);
                if s.tools.len() == 2 && s.prompts.len() == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("Discovery did not finish");
    }
    async fn command_finished(m: &Manager, id: &str, cid: &str) -> String {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(state) = m.snapshot(id).commands.get(cid)
                    && state != "Running"
                {
                    return state.clone();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("command should finish")
    }

    #[test]
    fn custom_commands_finish_fail_stream_and_cancel() {
        let (m, dir, mut server) = setup();
        server
            .environment
            .insert("CUSTOM_VALUE".into(), "inherited".into());
        if let Connection::Stdio { directory, .. } = &mut server.connection {
            *directory = dir.path().to_string_lossy().into_owned();
        }
        server.commands = [
            (
                "quick",
                "printf '%s' \"$CUSTOM_VALUE\"; pwd; printf error-output >&2",
            ),
            ("fail", "exit 7"),
            (
                "long",
                "printf ready; sleep 120 & echo $! > child.pid; wait",
            ),
        ]
        .into_iter()
        .map(|(id, command)| crate::config::CustomCommand {
            id: id.into(),
            name: id.into(),
            command: command.into(),
        })
        .collect();
        m.runtime.block_on(async {
            m.save(server.clone(), vec![]).await.unwrap();
            assert_eq!(m.registry.load().unwrap()[0].commands, server.commands);
            m.run_command(&server.id, "quick").await.unwrap();
            assert_eq!(command_finished(&m, &server.id, "quick").await, "Completed");
            let logs = m
                .snapshot(&server.id)
                .logs
                .into_iter()
                .collect::<Vec<_>>()
                .join("\n");
            assert!(logs.contains("inherited"));
            assert!(logs.contains(&dir.path().to_string_lossy().to_string()));
            assert!(logs.contains("error-output"));
            m.run_command(&server.id, "fail").await.unwrap();
            assert!(command_finished(&m, &server.id, "fail").await.contains('7'));
            m.run_command(&server.id, "long").await.unwrap();
            assert!(m.run_command(&server.id, "long").await.is_err());
            tokio::time::timeout(Duration::from_secs(5), async {
                while !dir.path().join("child.pid").exists()
                    || !m
                        .snapshot(&server.id)
                        .logs
                        .iter()
                        .any(|l| l.contains("ready"))
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            m.stop_command(&server.id, "long").await.unwrap();
            assert_eq!(command_finished(&m, &server.id, "long").await, "Stopped");
            let pid = std::fs::read_to_string(dir.path().join("child.pid")).unwrap();
            let process = std::fs::read_to_string(format!("/proc/{}/stat", pid.trim()));
            assert!(process.is_err() || process.unwrap().split_whitespace().nth(2) == Some("Z"));
            m.run_command(&server.id, "long").await.unwrap();
            m.shutdown().await.unwrap();
            assert_eq!(command_finished(&m, &server.id, "long").await, "Stopped");
            assert!(!m.snapshot(&server.id).active());
            m.run_command(&server.id, "quick").await.unwrap();
            assert_eq!(command_finished(&m, &server.id, "quick").await, "Completed");
        });
    }

    #[test]
    fn process_discovery_invocation_restart_and_bridge() {
        let (m, _dir, server) = setup();
        m.runtime.block_on(async {
            m.save(server.clone(), vec![]).await.unwrap();
            m.start(&server.id).await.unwrap();
            ready(&m, &server.id).await;
            let pid = m.snapshot(&server.id).pid.unwrap();
            m.start(&server.id).await.unwrap();
            assert_eq!(m.snapshot(&server.id).pid, Some(pid));

            let result = m
                .request(
                    &server.id,
                    "tools/call",
                    json!({"name": "echo", "arguments": {"text": "hello"}}),
                )
                .await
                .unwrap();
            assert_eq!(result["content"][0]["text"], "hello");
            let result = m
                .request(
                    &server.id,
                    "resources/read",
                    json!({"uri": "fixture://readme"}),
                )
                .await
                .unwrap();
            assert_eq!(result["contents"][0]["text"], "Fixture content");
            let result = m
                .request(
                    &server.id,
                    "prompts/get",
                    json!({"name": "greet", "arguments": {"name": "GNOME"}}),
                )
                .await
                .unwrap();
            assert_eq!(result["messages"][0]["content"]["text"], "Hello GNOME");

            let cancellation = CancellationToken::new();
            let endpoint = m.bridge(&server.id, cancellation.clone()).await.unwrap();
            let http = reqwest::Client::new();
            let discovery = json!({
                "jsonrpc": "2.0",
                "id": "openai-mcp-discover",
                "method": "server/discover",
                "params": {}
            });
            for content_type in [
                None,
                Some("text/plain"),
                Some("application/json"),
                Some("application/json; charset=utf-8"),
            ] {
                let mut request = http
                    .post(&endpoint)
                    .header("accept", "application/json, text/event-stream")
                    .body(discovery.to_string());
                if let Some(content_type) = content_type {
                    request = request.header("content-type", content_type);
                }
                let response = request.send().await.unwrap();
                assert_eq!(response.status(), 404);
                assert_eq!(response.headers()["content-type"], "application/json");
                let response: serde_json::Value = response.json().await.unwrap();
                assert_eq!(response["id"], "openai-mcp-discover");
                assert_eq!(response["error"]["code"], -32601);
            }
            let browser = http
                .post(&endpoint)
                .header("origin", "https://example.com")
                .json(&discovery)
                .send()
                .await
                .unwrap();
            assert_eq!(browser.status(), 403);
            let unknown_path = http
                .post(format!(
                    "{}/not-the-bridge",
                    endpoint.trim_end_matches("/mcp")
                ))
                .json(&discovery)
                .send()
                .await
                .unwrap();
            assert_eq!(unknown_path.status(), 404);
            assert!(unknown_path.json::<serde_json::Value>().await.is_err());
            let invalid_type = http
                .post(&endpoint)
                .header("accept", "application/json, text/event-stream")
                .header("content-type", "text/plain")
                .body(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}).to_string())
                .send()
                .await
                .unwrap();
            assert_eq!(invalid_type.status(), 415);

            let a =
                ().serve(StreamableHttpClientTransport::from_uri(endpoint.clone()))
                    .await
                    .unwrap();
            let b =
                ().serve(StreamableHttpClientTransport::from_uri(endpoint.clone()))
                    .await
                    .unwrap();
            let request = |text: &str| {
                serde_json::from_value::<ClientRequest>(json!({
                    "method": "tools/call",
                    "params": {"name": "echo", "arguments": {"text": text}}
                }))
                .unwrap()
            };
            let (left, right) = tokio::join!(
                a.send_request(request("client A")),
                b.send_request(request("client B"))
            );
            assert_eq!(
                serde_json::to_value(left.unwrap()).unwrap()["content"][0]["text"],
                "client A"
            );
            assert_eq!(
                serde_json::to_value(right.unwrap()).unwrap()["content"][0]["text"],
                "client B"
            );
            assert_eq!(m.snapshot(&server.id).pid, Some(pid));
            let browser = reqwest::Client::new()
                .post(endpoint)
                .header("origin", "https://example.com")
                .body("{}")
                .send()
                .await
                .unwrap();
            assert_eq!(browser.status(), 403);
            a.cancel().await.unwrap();
            b.cancel().await.unwrap();
            cancellation.cancel();

            m.restart(&server.id).await.unwrap();
            ready(&m, &server.id).await;
            assert_ne!(m.snapshot(&server.id).pid, Some(pid));
            m.stop(&server.id).await.unwrap();
            assert_eq!(m.snapshot(&server.id).state, "Stopped");
            assert!(
                m.request(&server.id, "tools/list", json!({}))
                    .await
                    .is_err()
            );
        });
    }

    #[test]
    fn local_bridge_persists_port_and_serves_requests() {
        let (m, _dir, server) = setup();
        m.runtime.block_on(async {
            m.save(server.clone(), vec![]).await.unwrap();
            m.start(&server.id).await.unwrap();
            ready(&m, &server.id).await;

            // Wait briefly for local bridge to become available in snapshot
            tokio::time::timeout(Duration::from_secs(5), async {
                while m.snapshot(&server.id).local_endpoint.is_none() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .unwrap();

            let ep = m.snapshot(&server.id).local_endpoint.unwrap();
            assert!(ep.contains(&format!("/{}/mcp", server.id)));

            // Connect over HTTP via bridge
            let client =
                ().serve(StreamableHttpClientTransport::from_uri(ep.clone()))
                    .await
                    .unwrap();
            let req = serde_json::from_value::<ClientRequest>(json!({
                "method": "tools/call",
                "params": {"name": "echo", "arguments": {"text": "through-bridge"}}
            }))
            .unwrap();
            let res = client.send_request(req).await.unwrap();
            assert_eq!(
                serde_json::to_value(res).unwrap()["content"][0]["text"],
                "through-bridge"
            );

            // Port is persisted to Server definition
            let saved = m.server(&server.id).unwrap();
            assert!(saved.bridge_port.is_some());

            client.cancel().await.unwrap();
            m.stop(&server.id).await.unwrap();
            assert!(m.snapshot(&server.id).local_endpoint.is_none());
        });
    }

    #[test]
    fn cancellation_during_initialization_and_unexpected_exit() {
        let (m, _dir, mut server) = setup();
        m.runtime.block_on(async {
            if let Connection::Stdio { arguments, .. } = &mut server.connection {
                arguments.push("--hang".into());
            }
            m.save(server.clone(), vec![]).await.unwrap();
            m.start(&server.id).await.unwrap();
            tokio::time::sleep(Duration::from_millis(150)).await;
            tokio::time::timeout(Duration::from_secs(5), m.stop(&server.id))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(m.snapshot(&server.id).state, "Stopped");
            if let Connection::Stdio { arguments, .. } = &mut server.connection {
                arguments.pop();
            }
            m.save(server.clone(), vec![]).await.unwrap();
            m.start(&server.id).await.unwrap();
            ready(&m, &server.id).await;
            assert!(
                m.request(&server.id, "tools/call", json!({"name":"crash"}))
                    .await
                    .is_err()
            );
            tokio::time::timeout(Duration::from_secs(5), async {
                while m.snapshot(&server.id).state != "Failed" {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(m.snapshot(&server.id).exit_code, Some(7));
            m.shutdown().await.unwrap();
        });
    }
    #[test]
    fn missing_executable_fails_without_losing_definition() {
        let (m, _dir, mut server) = setup();
        m.runtime.block_on(async {
            if let Connection::Stdio { executable, .. } = &mut server.connection {
                *executable = "/not/a/real/executable".into();
            }
            m.save(server.clone(), vec![]).await.unwrap();
            m.start(&server.id).await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                while m.snapshot(&server.id).state != "Failed" {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
            assert!(
                m.snapshot(&server.id)
                    .error
                    .unwrap()
                    .contains("Could not launch")
            );
            assert_eq!(m.servers().len(), 1);
            m.shutdown().await.unwrap();
        });
    }

    #[test]
    fn remote_access_is_started_after_the_mcp_connection() {
        let (m, _dir, mut server) = setup();
        server.remote.provider = "test-invalid-provider".into();
        server.remote.start_with_server = true;
        m.runtime.block_on(async {
            m.save(server.clone(), vec![]).await.unwrap();
            m.start(&server.id).await.unwrap();
            ready(&m, &server.id).await;
            tokio::time::timeout(Duration::from_secs(5), async {
                while !m.snapshot(&server.id).remote_state.starts_with("Failed:") {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("Remote access was not started");
            assert_eq!(m.snapshot(&server.id).state, "Running");
            m.shutdown().await.unwrap();
        });
    }
}
