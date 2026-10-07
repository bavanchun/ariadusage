// Ported from CodexBar Sources/CodexBarCore/PathEnvironment.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

#[cfg(target_os = "linux")]
use std::ffi::OsString;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use ariadusage_core::gates::launch::LaunchGate;
#[cfg(target_os = "linux")]
use tokio::sync::{Mutex, watch};

use super::call::BrokerCall;
#[cfg(target_os = "linux")]
use super::exec_resolver::RealResolverFs;
use super::exec_resolver::ResolverFs;
#[cfg(target_os = "linux")]
use super::process::{
    AbsolutePath, Command, LaunchMode, Output, ProcessEnv, ProcessError, ProcessRegistry,
    StreamPolicy,
};

#[cfg(target_os = "linux")]
struct CacheState {
    captured: Option<Vec<PathBuf>>,
    in_flight: Option<watch::Receiver<Option<Option<Vec<PathBuf>>>>>,
}

#[derive(Clone)]
pub struct LoginShell {
    #[cfg(target_os = "linux")]
    env: ProcessEnv,
    #[cfg(target_os = "linux")]
    registry: Arc<ProcessRegistry>,
    #[cfg(target_os = "linux")]
    gate: Arc<LaunchGate>,
    #[cfg(target_os = "linux")]
    cache: Arc<Mutex<CacheState>>,
}

#[cfg(target_os = "linux")]
impl LoginShell {
    pub fn new(env: ProcessEnv) -> Self {
        Self::with_registry_and_gate(
            env,
            Arc::new(ProcessRegistry::new()),
            Arc::new(LaunchGate::default()),
        )
    }

    pub fn with_registry_and_gate(
        env: ProcessEnv,
        registry: Arc<ProcessRegistry>,
        gate: Arc<LaunchGate>,
    ) -> Self {
        Self {
            env,
            registry,
            gate,
            cache: Arc::new(Mutex::new(CacheState {
                captured: None,
                in_flight: None,
            })),
        }
    }

    pub async fn current(&self) -> Option<Vec<PathBuf>> {
        let state = self.cache.lock().await;
        state.captured.clone()
    }

    pub async fn capture_path(&self, call: &BrokerCall) -> Option<Vec<PathBuf>> {
        let mut rx = {
            let mut state = self.cache.lock().await;
            if let Some(captured) = &state.captured {
                return Some(captured.clone());
            }
            if let Some(rx) = &state.in_flight {
                rx.clone()
            } else {
                let (tx, rx) = watch::channel(None);
                state.in_flight = Some(rx);
                drop(state);

                let result = self.execute_capture_path(call).await;

                let mut state = self.cache.lock().await;
                if let Some(paths) = &result {
                    state.captured = Some(paths.clone());
                }
                state.in_flight = None;
                let _ = tx.send(Some(result.clone()));
                return result;
            }
        };

        while rx.borrow().is_none() {
            if rx.changed().await.is_err() {
                break;
            }
        }
        rx.borrow().clone().flatten()
    }

    async fn execute_capture_path(&self, call: &BrokerCall) -> Option<Vec<PathBuf>> {
        let shell_val = self
            .env
            .get("SHELL")
            .and_then(|s| s.to_str())
            .unwrap_or("/bin/zsh");
        let shell_prog = AbsolutePath::new(shell_val).ok()?;
        let is_ci = self
            .env
            .get("CI")
            .and_then(|s| s.to_str())
            .is_some_and(|s| s == "1" || s.eq_ignore_ascii_case("true"));

        let marker = "__ARIADUSAGE_PATH__";
        let cmd_str = format!("printf '{marker}%s{marker}' \"$PATH\"");
        let args: Vec<OsString> = if is_ci {
            vec!["-c".into(), cmd_str.into()]
        } else {
            vec!["-l".into(), "-i".into(), "-c".into(), cmd_str.into()]
        };

        let mut command = Command::new(shell_prog, args);
        command
            .set_env(self.env.clone())
            .set_stdout(StreamPolicy::Capture { cap: 1024 * 1024 })
            .set_stderr(StreamPolicy::Discard)
            .set_timeout(Duration::from_secs(6))
            .set_launch_mode(LaunchMode::Session)
            .set_reap_marker(true);

        let output = super::process::run(call.clone(), command, &self.registry, &self.gate)
            .await
            .ok()?;
        if !output.status.success() {
            return None;
        }

        let text = std::str::from_utf8(&output.stdout).ok()?;
        let trimmed = text.trim();
        let extracted = if let (Some(start), Some(end)) =
            (trimmed.find(marker), trimmed.rfind(marker))
            && start < end
        {
            &trimmed[start + marker.len()..end]
        } else {
            trimmed
        };

        let value = extracted.trim();
        if value.is_empty() {
            return None;
        }

        let paths: Vec<PathBuf> = std::env::split_paths(value)
            .filter(|p| !p.as_os_str().is_empty())
            .collect();
        if paths.is_empty() { None } else { Some(paths) }
    }

    pub async fn command_v(&self, call: &BrokerCall, name: &str) -> Option<PathBuf> {
        let shell_val = self
            .env
            .get("SHELL")
            .and_then(|s| s.to_str())
            .unwrap_or("/bin/zsh");
        let shell_prog = AbsolutePath::new(shell_val).ok()?;
        let is_ci = self
            .env
            .get("CI")
            .and_then(|s| s.to_str())
            .is_some_and(|s| s == "1" || s.eq_ignore_ascii_case("true"));

        let cmd_str = format!("command -v {name}");
        let args: Vec<OsString> = if is_ci {
            vec!["-c".into(), cmd_str.into()]
        } else {
            vec!["-l".into(), "-i".into(), "-c".into(), cmd_str.into()]
        };

        let mut command = Command::new(shell_prog, args);
        command
            .set_env(self.env.clone())
            .set_stdout(StreamPolicy::Capture { cap: 1024 * 1024 })
            .set_stderr(StreamPolicy::Discard)
            .set_timeout(Duration::from_secs(2))
            .set_launch_mode(LaunchMode::Session)
            .set_reap_marker(true);

        let output = super::process::run(call.clone(), command, &self.registry, &self.gate)
            .await
            .ok()?;
        if !output.status.success() {
            return None;
        }

        let text = std::str::from_utf8(&output.stdout).ok()?;
        for line in text.lines().rev() {
            let trimmed = line.trim();
            if trimmed.starts_with('/') {
                let path = PathBuf::from(trimmed);
                if RealResolverFs.is_executable(&path) {
                    return Some(path);
                }
            }
        }

        None
    }

    pub async fn alias(&self, call: &BrokerCall, name: &str) -> Option<PathBuf> {
        let shell_val = self
            .env
            .get("SHELL")
            .and_then(|s| s.to_str())
            .unwrap_or("/bin/zsh");
        let shell_prog = AbsolutePath::new(shell_val).ok()?;
        let is_ci = self
            .env
            .get("CI")
            .and_then(|s| s.to_str())
            .is_some_and(|s| s == "1" || s.eq_ignore_ascii_case("true"));

        let cmd_str = format!("alias {name} 2>/dev/null; type -a {name} 2>/dev/null");
        let args: Vec<OsString> = if is_ci {
            vec!["-c".into(), cmd_str.into()]
        } else {
            vec!["-l".into(), "-i".into(), "-c".into(), cmd_str.into()]
        };

        let mut command = Command::new(shell_prog, args);
        command
            .set_env(self.env.clone())
            .set_stdout(StreamPolicy::Capture { cap: 1024 * 1024 })
            .set_stderr(StreamPolicy::Discard)
            .set_timeout(Duration::from_secs(2))
            .set_launch_mode(LaunchMode::Session)
            .set_reap_marker(true);

        let output = super::process::run(call.clone(), command, &self.registry, &self.gate)
            .await
            .ok()?;
        if !output.status.success() {
            return None;
        }

        let text = std::str::from_utf8(&output.stdout).ok()?;
        let home = self.env.get("HOME").and_then(|h| h.to_str()).unwrap_or("");
        parse_alias_and_type_a(text, name, home, &RealResolverFs)
    }

    pub async fn run_shell_command(
        &self,
        call: &BrokerCall,
        args: Vec<OsString>,
        timeout: Duration,
        stdout_cap: usize,
    ) -> Result<Output, ProcessError> {
        let shell_val = self
            .env
            .get("SHELL")
            .and_then(|s| s.to_str())
            .unwrap_or("/bin/zsh");
        let shell_prog = AbsolutePath::new(shell_val)?;

        let mut command = Command::new(shell_prog, args);
        command
            .set_env(self.env.clone())
            .set_stdout(StreamPolicy::Capture { cap: stdout_cap })
            .set_stderr(StreamPolicy::Discard)
            .set_timeout(timeout)
            .set_launch_mode(LaunchMode::Session)
            .set_reap_marker(true);

        super::process::run(call.clone(), command, &self.registry, &self.gate).await
    }
}

#[cfg(not(target_os = "linux"))]
impl LoginShell {
    pub async fn capture_path(&self, _call: &BrokerCall) -> Option<Vec<PathBuf>> {
        None
    }

    pub async fn command_v(&self, _call: &BrokerCall, _name: &str) -> Option<PathBuf> {
        None
    }

    pub async fn alias(&self, _call: &BrokerCall, _name: &str) -> Option<PathBuf> {
        None
    }
}

pub fn parse_alias_and_type_a(
    text: &str,
    tool: &str,
    home: &str,
    fs: &dyn ResolverFs,
) -> Option<PathBuf> {
    let lines: Vec<&str> = text.lines().map(|l| l.trim()).collect();

    // 1) parse alias path
    let alias_prefix = format!("alias {tool}=");
    for line in &lines {
        if line.starts_with(&alias_prefix) {
            let value = &line[alias_prefix.len()..];
            if let Some(path) = extract_alias_expansion(value, home)
                && fs.is_executable(&path)
            {
                return Some(path);
            }
        }
        if line.to_ascii_lowercase().contains("aliased to")
            && let Some(idx) = line.to_ascii_lowercase().find("aliased to")
        {
            let value = line[idx + "aliased to".len()..].trim();
            if let Some(path) = extract_alias_expansion(value, home)
                && fs.is_executable(&path)
            {
                return Some(path);
            }
        }
    }

    // 2) extract path candidate from type -a lines
    for line in lines {
        if let Some(path) = extract_path_candidate(line, tool, home)
            && fs.is_executable(&path)
        {
            return Some(path);
        }
    }

    None
}

fn extract_alias_expansion(raw: &str, home: &str) -> Option<PathBuf> {
    let trimmed =
        raw.trim_matches(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '`');
    if trimmed.is_empty() {
        return None;
    }
    let first = trimmed.split_whitespace().next()?;
    let expanded = expand_path(first, home);
    if expanded.starts_with('/') {
        Some(PathBuf::from(expanded))
    } else {
        None
    }
}

fn extract_path_candidate(line: &str, tool: &str, home: &str) -> Option<PathBuf> {
    for token in line.split_whitespace() {
        let expanded = expand_path(token, home);
        if expanded.starts_with('/') {
            let path = PathBuf::from(&expanded);
            if path.file_name().and_then(|f| f.to_str()) == Some(tool) {
                return Some(path);
            }
        }
    }
    None
}

fn expand_path(raw: &str, home: &str) -> String {
    if raw == "~" {
        home.to_owned()
    } else if let Some(stripped) = raw.strip_prefix("~/") {
        if home.ends_with('/') {
            format!("{home}{stripped}")
        } else {
            format!("{home}/{stripped}")
        }
    } else {
        raw.to_owned()
    }
}

pub async fn capture_path(shell: &LoginShell, call: &BrokerCall) -> Option<Vec<PathBuf>> {
    shell.capture_path(call).await
}

pub async fn command_v(shell: &LoginShell, call: &BrokerCall, name: &str) -> Option<PathBuf> {
    shell.command_v(call, name).await
}

pub async fn alias(shell: &LoginShell, call: &BrokerCall, name: &str) -> Option<PathBuf> {
    shell.alias(call, name).await
}
