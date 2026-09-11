//! Run user commands on the host when Marshal is inside a Flatpak sandbox.
//!
//! MCP servers, custom commands, tunnel clients, and agent CLIs are installed
//! on the host. The sandbox cannot see those binaries on PATH, so they are
//! started with `flatpak-spawn --host`. Extra environment variables must be
//! passed with `--env=` because the host process inherits the session helper
//! environment, not Marshal’s sandbox or the graphical login session.

use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Stdio,
    sync::OnceLock,
};

pub fn in_flatpak() -> bool {
    Path::new("/.flatpak-info").exists()
}

pub fn tokio_command(program: impl AsRef<OsStr>) -> Command {
    Command::new(program)
}

pub fn std_command(program: impl AsRef<OsStr>) -> StdCommand {
    StdCommand::new(program)
}

pub struct Command {
    inner: tokio::process::Command,
    program: OsString,
    args: Vec<OsString>,
    in_flatpak: bool,
    sealed: bool,
}

impl Command {
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self::create(in_flatpak(), program)
    }

    fn create(in_flatpak: bool, program: impl AsRef<OsStr>) -> Self {
        if in_flatpak {
            let mut inner = tokio::process::Command::new("flatpak-spawn");
            inner.arg("--host").arg("--watch-bus");
            apply_session_env(|flag| {
                inner.arg(flag);
            });
            Self {
                inner,
                program: program.as_ref().into(),
                args: Vec::new(),
                in_flatpak: true,
                sealed: false,
            }
        } else {
            Self {
                inner: tokio::process::Command::new(&program),
                program: program.as_ref().into(),
                args: Vec::new(),
                in_flatpak: false,
                sealed: true,
            }
        }
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        if self.in_flatpak {
            self.args.push(arg.as_ref().into());
        } else {
            self.inner.arg(arg);
        }
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.arg(arg);
        }
        self
    }

    pub fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        if self.in_flatpak {
            self.inner.arg(env_flag(key.as_ref(), value.as_ref()));
        } else {
            self.inner.env(key, value);
        }
        self
    }

    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        for (key, value) in vars {
            self.env(key, value);
        }
        self
    }

    pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        if self.in_flatpak {
            let mut flag = OsString::from("--directory=");
            flag.push(dir.as_ref());
            self.inner.arg(flag);
        } else {
            self.inner.current_dir(dir);
        }
        self
    }

    pub fn kill_on_drop(&mut self, kill: bool) -> &mut Self {
        self.inner.kill_on_drop(kill);
        self
    }

    pub fn process_group(&mut self, group: i32) -> &mut Self {
        self.inner.process_group(group);
        self
    }

    pub fn stdin(&mut self, cfg: Stdio) -> &mut Self {
        self.inner.stdin(cfg);
        self
    }

    pub fn stdout(&mut self, cfg: Stdio) -> &mut Self {
        self.inner.stdout(cfg);
        self
    }

    pub fn stderr(&mut self, cfg: Stdio) -> &mut Self {
        self.inner.stderr(cfg);
        self
    }

    /// # Safety
    /// Same contract as [`std::os::unix::process::CommandExt::pre_exec`].
    pub unsafe fn pre_exec<F>(&mut self, f: F) -> &mut Self
    where
        F: FnMut() -> std::io::Result<()> + Send + Sync + 'static,
    {
        unsafe {
            self.inner.pre_exec(f);
        }
        self
    }

    pub fn spawn(&mut self) -> std::io::Result<tokio::process::Child> {
        self.seal();
        self.inner.spawn()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn as_std(&self) -> &std::process::Command {
        self.inner.as_std()
    }

    fn seal(&mut self) {
        if self.in_flatpak && !self.sealed {
            self.inner.arg("--").arg(&self.program);
            self.inner.args(&self.args);
            self.sealed = true;
        }
    }
}

pub struct StdCommand {
    inner: std::process::Command,
    program: OsString,
    args: Vec<OsString>,
    in_flatpak: bool,
    sealed: bool,
}

impl StdCommand {
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self::create(in_flatpak(), program)
    }

    fn create(in_flatpak: bool, program: impl AsRef<OsStr>) -> Self {
        if in_flatpak {
            let mut inner = std::process::Command::new("flatpak-spawn");
            inner.arg("--host").arg("--watch-bus");
            apply_session_env(|flag| {
                inner.arg(flag);
            });
            Self {
                inner,
                program: program.as_ref().into(),
                args: Vec::new(),
                in_flatpak: true,
                sealed: false,
            }
        } else {
            Self {
                inner: std::process::Command::new(&program),
                program: program.as_ref().into(),
                args: Vec::new(),
                in_flatpak: false,
                sealed: true,
            }
        }
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        if self.in_flatpak {
            self.args.push(arg.as_ref().into());
        } else {
            self.inner.arg(arg);
        }
        self
    }

    pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        if self.in_flatpak {
            let mut flag = OsString::from("--directory=");
            flag.push(dir.as_ref());
            self.inner.arg(flag);
        } else {
            self.inner.current_dir(dir);
        }
        self
    }

    pub fn output(&mut self) -> std::io::Result<std::process::Output> {
        self.seal();
        self.inner.output()
    }

    fn seal(&mut self) {
        if self.in_flatpak && !self.sealed {
            self.inner.arg("--").arg(&self.program);
            self.inner.args(&self.args);
            self.sealed = true;
        }
    }
}

fn env_flag(key: &OsStr, value: &OsStr) -> OsString {
    let mut flag = OsString::from("--env=");
    flag.push(key);
    flag.push("=");
    flag.push(value);
    flag
}

/// Host processes do not inherit the sandbox. Copy the graphical session from
/// Marshal and PATH/D-Bus/runtime from the host so browsers and Secret Service
/// work the same as a native launch.
fn apply_session_env(mut add: impl FnMut(OsString)) {
    if !in_flatpak() {
        return;
    }
    for (key, value) in session_env() {
        add(env_flag(&key, &value));
    }
}

fn session_env() -> Vec<(OsString, OsString)> {
    static ENV: OnceLock<Vec<(OsString, OsString)>> = OnceLock::new();
    ENV.get_or_init(load_session_env).clone()
}

fn load_session_env() -> Vec<(OsString, OsString)> {
    let host = host_environ();
    let mut vars = Vec::new();
    for key in [
        "PATH",
        "HOME",
        "USER",
        "SHELL",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "XAUTHORITY",
        "SSH_AUTH_SOCK",
    ] {
        if let Some(value) = host.get(OsStr::new(key)).filter(|value| !value.is_empty()) {
            vars.push((OsString::from(key), value.clone()));
        }
    }
    vars.extend(graphical_session_env());
    if !vars.iter().any(|(key, _)| key == "XAUTHORITY")
        && let Some(value) = std::env::var_os("XAUTHORITY").filter(|value| !value.is_empty())
    {
        vars.push((OsString::from("XAUTHORITY"), value));
    }
    vars
}

fn graphical_session_env() -> Vec<(OsString, OsString)> {
    [
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_TYPE",
        "XDG_SESSION_DESKTOP",
    ]
    .into_iter()
    .filter_map(|key| {
        std::env::var_os(key)
            .filter(|value| !value.is_empty())
            .map(|value| (OsString::from(key), value))
    })
    .collect()
}

fn host_environ() -> HashMap<OsString, OsString> {
    static ENV: OnceLock<HashMap<OsString, OsString>> = OnceLock::new();
    ENV.get_or_init(|| {
        if !in_flatpak() {
            return HashMap::new();
        }
        std::process::Command::new("flatpak-spawn")
            .args(["--host", "--", "printenv"])
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|text| parse_environ(&text))
            .unwrap_or_default()
    })
    .clone()
}

fn parse_environ(text: &str) -> HashMap<OsString, OsString> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            if key.is_empty() {
                return None;
            }
            Some((OsString::from(key), OsString::from(value)))
        })
        .collect()
}

pub fn host_path() -> Vec<PathBuf> {
    static PATH: OnceLock<Vec<PathBuf>> = OnceLock::new();
    PATH.get_or_init(load_host_path).clone()
}

fn load_host_path() -> Vec<PathBuf> {
    let raw = if in_flatpak() {
        std::process::Command::new("flatpak-spawn")
            .args(["--host", "--", "printenv", "PATH"])
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|value| OsString::from(value.trim()))
    } else {
        std::env::var_os("PATH")
    };
    std::env::split_paths(&raw.unwrap_or_default())
        .filter(|path| path.is_absolute())
        .collect()
}

pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if path
        .metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    {
        return true;
    }
    if !in_flatpak() {
        return false;
    }
    std::process::Command::new("flatpak-spawn")
        .args(["--host", "--", "test", "-x"])
        .arg(path)
        .status()
        .is_ok_and(|status| status.success())
}

pub fn host_env(key: &str) -> Option<OsString> {
    if in_flatpak() {
        host_environ()
            .get(OsStr::new(key))
            .cloned()
            .or_else(|| std::env::var_os(key))
    } else {
        std::env::var_os(key)
    }
}

pub fn is_command_in_path(cmd: &str) -> bool {
    host_path().iter().any(|dir| dir.join(cmd).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_direct_outside_flatpak() {
        assert!(!in_flatpak());
        assert_eq!(tokio_command("npx").as_std().get_program(), "npx");
        let mut claude = std_command("claude");
        claude.arg("mcp");
        assert_eq!(claude.inner.get_program(), "claude");
    }

    #[test]
    fn parse_environ_reads_host_assignments() {
        let env = parse_environ("PATH=/usr/bin:/bin\nHOME=/home/nick\nEMPTY=\n=skip\n");
        assert_eq!(
            env.get(OsStr::new("PATH")).map(OsString::as_os_str),
            Some(OsStr::new("/usr/bin:/bin"))
        );
        assert_eq!(
            env.get(OsStr::new("HOME")).map(OsString::as_os_str),
            Some(OsStr::new("/home/nick"))
        );
        assert_eq!(
            env.get(OsStr::new("EMPTY")).map(OsString::as_os_str),
            Some(OsStr::new(""))
        );
        assert!(!env.contains_key(OsStr::new("")));
    }

    #[test]
    fn graphical_session_env_matches_process_display_variables() {
        const KEYS: [&str; 5] = [
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XDG_CURRENT_DESKTOP",
            "XDG_SESSION_TYPE",
            "XDG_SESSION_DESKTOP",
        ];
        let vars = graphical_session_env();
        for (key, value) in &vars {
            let key = key.to_str().expect("graphical session keys are UTF-8");
            assert!(KEYS.contains(&key));
            assert!(!value.is_empty());
            assert_eq!(std::env::var_os(key).as_deref(), Some(value.as_os_str()));
        }
        for key in KEYS {
            match std::env::var_os(key) {
                Some(value) if !value.is_empty() => {
                    assert!(vars.iter().any(|(copied, copied_value)| {
                        copied == key && copied_value == &value
                    }));
                }
                _ => {
                    assert!(vars.iter().all(|(copied, _)| copied != key));
                }
            }
        }
    }

    #[test]
    fn flatpak_env_and_directory_are_flags_before_program() {
        let mut command = Command::create(true, "tunnel-client");
        command
            .env("CONTROL_PLANE_API_KEY", "secret")
            .current_dir("/tmp")
            .arg("run");
        command.seal();
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--host",
                "--watch-bus",
                "--env=CONTROL_PLANE_API_KEY=secret",
                "--directory=/tmp",
                "--",
                "tunnel-client",
                "run",
            ]
        );
        assert_eq!(command.as_std().get_program(), "flatpak-spawn");
    }
}
