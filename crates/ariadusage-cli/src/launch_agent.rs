//! macOS per-user LaunchAgent management for the AriadUsage engine daemon.
//!
//! Writes, loads (`bootstrap gui/<uid>`), unloads (`bootout gui/<uid>`), and removes
//! `~/Library/LaunchAgents/io.github.bavanchun.ariadusage.daemon.plist`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ariadusage_engine::paths::{PathError, resolve_state_dir};
use ariadusage_engine::private_file::{WriteHooks, write_private};

/// Reverse-DNS LaunchAgent label for the AriadUsage engine daemon.
pub const LAUNCH_AGENT_LABEL: &str = "io.github.bavanchun.ariadusage.daemon";

/// Plist filename under `~/Library/LaunchAgents/`.
pub const LAUNCH_AGENT_PLIST_NAME: &str = "io.github.bavanchun.ariadusage.daemon.plist";

/// Absolute path to macOS `launchctl`.
pub const LAUNCHCTL_BIN: &str = "/bin/launchctl";

/// Errors encountered while managing the macOS LaunchAgent.
#[derive(Debug, PartialEq, Eq)]
pub enum LaunchAgentError {
    UnsupportedPlatform,
    Path(PathError),
    NonAbsoluteBinaryPath(PathBuf),
    InvalidPathCharacters,
    WriteFailed(String),
    LaunchctlFailed(String),
}

impl std::fmt::Display for LaunchAgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                write!(f, "LaunchAgent management is supported only on macOS")
            }
            Self::Path(e) => write!(f, "{e}"),
            Self::NonAbsoluteBinaryPath(p) => {
                write!(f, "daemon binary path must be absolute: {}", p.display())
            }
            Self::InvalidPathCharacters => {
                write!(f, "path contains invalid control characters or non-UTF-8")
            }
            Self::WriteFailed(msg) => write!(f, "failed to write LaunchAgent plist: {msg}"),
            Self::LaunchctlFailed(msg) => write!(f, "launchctl failed: {msg}"),
        }
    }
}

impl std::error::Error for LaunchAgentError {}

impl From<PathError> for LaunchAgentError {
    fn from(value: PathError) -> Self {
        Self::Path(value)
    }
}

/// Resolved paths for the per-user LaunchAgent plist and daemon log files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchAgentPaths {
    pub plist_path: PathBuf,
    pub log_dir: PathBuf,
    pub stdout_log_path: PathBuf,
    pub stderr_log_path: PathBuf,
}

/// Resolves the LaunchAgent plist path and log paths from an injected environment lookup and `home`.
pub fn resolve_launch_agent_paths(
    env: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Result<LaunchAgentPaths, LaunchAgentError> {
    let home = home.ok_or(PathError::NoHomeDirectory)?;
    if !home.is_absolute() {
        return Err(LaunchAgentError::Path(PathError::NonAbsoluteHome(
            home.to_path_buf(),
        )));
    }
    let state_base = resolve_state_dir(env, Some(home))?;
    let log_dir = state_base.join("ariadusage");
    let plist_path = home
        .join("Library")
        .join("LaunchAgents")
        .join(LAUNCH_AGENT_PLIST_NAME);

    Ok(LaunchAgentPaths {
        plist_path,
        stdout_log_path: log_dir.join("daemon.out.log"),
        stderr_log_path: log_dir.join("daemon.err.log"),
        log_dir,
    })
}

fn xml_escape(input: &str) -> Result<String, LaunchAgentError> {
    if input
        .chars()
        .any(|c| c == '\0' || (c.is_control() && c != '\t'))
    {
        return Err(LaunchAgentError::InvalidPathCharacters);
    }
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    Ok(out)
}

/// Generates the deterministic XML property list for `io.github.bavanchun.ariadusage.daemon`.
///
/// Enforces:
/// - `Label`: `io.github.bavanchun.ariadusage.daemon`
/// - `ProgramArguments`: `["<binary_path>", "daemon"]`
/// - `RunAtLoad`: `true`
/// - `KeepAlive`: `{ "SuccessfulExit": false }` (restart on crash only)
/// - `StandardOutPath` / `StandardErrorPath` under `<state_dir>/ariadusage/`
/// - No `EnvironmentVariables` or secrets.
pub fn render_launch_agent_plist(
    binary_path: &Path,
    paths: &LaunchAgentPaths,
) -> Result<String, LaunchAgentError> {
    if !binary_path.is_absolute() {
        return Err(LaunchAgentError::NonAbsoluteBinaryPath(
            binary_path.to_path_buf(),
        ));
    }
    let bin_str = binary_path
        .to_str()
        .ok_or(LaunchAgentError::InvalidPathCharacters)?;
    let stdout_str = paths
        .stdout_log_path
        .to_str()
        .ok_or(LaunchAgentError::InvalidPathCharacters)?;
    let stderr_str = paths
        .stderr_log_path
        .to_str()
        .ok_or(LaunchAgentError::InvalidPathCharacters)?;

    let escaped_bin = xml_escape(bin_str)?;
    let escaped_stdout = xml_escape(stdout_str)?;
    let escaped_stderr = xml_escape(stderr_str)?;

    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCH_AGENT_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{escaped_bin}</string>
        <string>daemon</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>StandardOutPath</key>
    <string>{escaped_stdout}</string>
    <key>StandardErrorPath</key>
    <string>{escaped_stderr}</string>
</dict>
</plist>
"#
    ))
}

/// Injectable runner for invoking `/bin/launchctl` by absolute path with an argv array.
pub trait LaunchctlRunner {
    fn run(&self, program: &Path, args: &[String]) -> Result<(), LaunchAgentError>;
}

/// Production [`LaunchctlRunner`] that executes `/bin/launchctl` directly with an argv array.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemLaunchctlRunner;

impl LaunchctlRunner for SystemLaunchctlRunner {
    fn run(&self, program: &Path, args: &[String]) -> Result<(), LaunchAgentError> {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|e| LaunchAgentError::LaunchctlFailed(e.to_string()))?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let detail = if stderr.is_empty() {
                format!("exit status {}", output.status)
            } else {
                stderr
            };
            Err(LaunchAgentError::LaunchctlFailed(detail))
        }
    }
}

/// Writes the LaunchAgent plist file atomically with private (`0600`) permissions and ensures the
/// daemon log directory exists with mode `0700`.
pub fn write_launch_agent_plist(
    binary_path: &Path,
    paths: &LaunchAgentPaths,
) -> Result<(), LaunchAgentError> {
    let xml = render_launch_agent_plist(binary_path, paths)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        builder.mode(0o700);
        builder
            .create(&paths.log_dir)
            .map_err(|e| LaunchAgentError::WriteFailed(e.to_string()))?;
        if let Some(parent) = paths.plist_path.parent() {
            builder
                .create(parent)
                .map_err(|e| LaunchAgentError::WriteFailed(e.to_string()))?;
        }
    }

    write_private(&paths.plist_path, xml.as_bytes(), &WriteHooks::default())
        .map_err(|e| LaunchAgentError::WriteFailed(e.to_string()))
}

/// Loads the LaunchAgent into `gui/<uid>` via `/bin/launchctl bootstrap gui/<uid> <plist_path>`.
pub fn load_launch_agent(
    uid: u32,
    paths: &LaunchAgentPaths,
    runner: &dyn LaunchctlRunner,
) -> Result<(), LaunchAgentError> {
    let plist_str = paths
        .plist_path
        .to_str()
        .ok_or(LaunchAgentError::InvalidPathCharacters)?;
    let domain = format!("gui/{uid}");
    let args = vec!["bootstrap".to_string(), domain, plist_str.to_string()];
    runner.run(Path::new(LAUNCHCTL_BIN), &args)
}

/// Unloads the LaunchAgent from `gui/<uid>` via `/bin/launchctl bootout gui/<uid> <plist_path>`.
pub fn unload_launch_agent(
    uid: u32,
    paths: &LaunchAgentPaths,
    runner: &dyn LaunchctlRunner,
) -> Result<(), LaunchAgentError> {
    let plist_str = paths
        .plist_path
        .to_str()
        .ok_or(LaunchAgentError::InvalidPathCharacters)?;
    let domain = format!("gui/{uid}");
    let args = vec!["bootout".to_string(), domain, plist_str.to_string()];
    runner.run(Path::new(LAUNCHCTL_BIN), &args)
}

/// Unloads (if loaded) and removes the LaunchAgent plist file from disk.
pub fn remove_launch_agent(
    uid: u32,
    paths: &LaunchAgentPaths,
    runner: &dyn LaunchctlRunner,
) -> Result<(), LaunchAgentError> {
    if paths.plist_path.exists() {
        let _ = unload_launch_agent(uid, paths, runner);
        std::fs::remove_file(&paths.plist_path)
            .map_err(|e| LaunchAgentError::WriteFailed(e.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct FakeLaunchctlRunner {
        calls: RefCell<Vec<(PathBuf, Vec<String>)>>,
    }

    impl LaunchctlRunner for FakeLaunchctlRunner {
        fn run(&self, program: &Path, args: &[String]) -> Result<(), LaunchAgentError> {
            self.calls
                .borrow_mut()
                .push((program.to_path_buf(), args.to_vec()));
            Ok(())
        }
    }

    #[test]
    fn resolves_launch_agent_paths_and_renders_plist_without_secrets() {
        let temp_home = tempfile::tempdir().expect("tempdir");
        let env_map: HashMap<String, OsString> = HashMap::new();
        let paths = resolve_launch_agent_paths(|k| env_map.get(k).cloned(), Some(temp_home.path()))
            .expect("resolve paths");

        assert_eq!(
            paths.plist_path,
            temp_home
                .path()
                .join("Library/LaunchAgents/io.github.bavanchun.ariadusage.daemon.plist")
        );
        assert_eq!(
            paths.stdout_log_path,
            temp_home
                .path()
                .join(".local/state/ariadusage/daemon.out.log")
        );
        assert_eq!(
            paths.stderr_log_path,
            temp_home
                .path()
                .join(".local/state/ariadusage/daemon.err.log")
        );

        let bin = Path::new("/usr/local/bin/ariadusage");
        let xml = render_launch_agent_plist(bin, &paths).expect("render plist");
        assert!(xml.contains("<string>io.github.bavanchun.ariadusage.daemon</string>"));
        assert!(!xml.contains("codexbar"));
        assert!(!xml.contains("CodexBar"));
        assert!(!xml.contains("EnvironmentVariables"));
        assert!(xml.contains("<string>/usr/local/bin/ariadusage</string>"));
        assert!(xml.contains("<string>daemon</string>"));
        assert!(xml.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert!(
            xml.contains(
                "<key>KeepAlive</key>\n    <dict>\n        <key>SuccessfulExit</key>\n        <false/>\n    </dict>"
            )
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn write_load_unload_and_remove_use_fake_runner_and_temp_home() {
        let temp_home = tempfile::tempdir().expect("tempdir");
        let paths = resolve_launch_agent_paths(|_| None, Some(temp_home.path())).expect("paths");
        let bin = Path::new("/opt/ariadusage/bin/ariadusage");

        write_launch_agent_plist(bin, &paths).expect("write plist");
        assert!(paths.plist_path.exists());
        assert!(paths.log_dir.is_dir());

        let runner = FakeLaunchctlRunner::default();
        load_launch_agent(501, &paths, &runner).expect("load");
        unload_launch_agent(501, &paths, &runner).expect("unload");
        remove_launch_agent(501, &paths, &runner).expect("remove");
        assert!(!paths.plist_path.exists());

        let calls = runner.calls.borrow();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].0, PathBuf::from("/bin/launchctl"));
        assert_eq!(
            calls[0].1,
            vec![
                "bootstrap".to_string(),
                "gui/501".to_string(),
                paths.plist_path.to_str().unwrap().to_string()
            ]
        );
        assert_eq!(calls[1].0, PathBuf::from("/bin/launchctl"));
        assert_eq!(
            calls[1].1,
            vec![
                "bootout".to_string(),
                "gui/501".to_string(),
                paths.plist_path.to_str().unwrap().to_string()
            ]
        );
    }
}
