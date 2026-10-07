use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

use zeroize::Zeroizing;

use super::{DEFAULT_OUTPUT_CAP, ProcessEnv, ProcessError};

#[derive(Clone)]
pub struct AbsolutePath(PathBuf);

impl AbsolutePath {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, ProcessError> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(ProcessError::LaunchFailed);
        }
        let Ok(metadata) = std::fs::metadata(path) else {
            return Err(ProcessError::LaunchFailed);
        };
        if !metadata.is_file() || !is_executable(&metadata) {
            return Err(ProcessError::LaunchFailed);
        }
        Ok(Self(path.to_path_buf()))
    }

    pub(crate) fn as_path(&self) -> &Path {
        &self.0
    }
}

impl fmt::Debug for AbsolutePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AbsolutePath(<redacted>)")
    }
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    metadata.is_file()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    Group,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamPolicy {
    Capture { cap: usize },
    Discard,
}

pub enum StdinSpec {
    Null,
    Bytes(Vec<u8>),
    Secret(Zeroizing<Vec<u8>>),
}

impl StdinSpec {
    pub(crate) fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    pub(crate) fn into_bytes(self) -> Option<Zeroizing<Vec<u8>>> {
        match self {
            Self::Null => None,
            Self::Bytes(bytes) => Some(Zeroizing::new(bytes)),
            Self::Secret(bytes) => Some(bytes),
        }
    }
}

impl fmt::Debug for StdinSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, len) = match self {
            Self::Null => ("null", 0),
            Self::Bytes(bytes) => ("bytes", bytes.len()),
            Self::Secret(bytes) => ("secret", bytes.len()),
        };
        formatter
            .debug_struct("StdinSpec")
            .field("kind", &kind)
            .field("byte_len", &len)
            .finish()
    }
}

pub struct Command {
    pub(crate) program: AbsolutePath,
    pub(crate) args: Vec<OsString>,
    pub(crate) env: ProcessEnv,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) stdin: StdinSpec,
    pub(crate) stdout: StreamPolicy,
    pub(crate) stderr: StreamPolicy,
    pub(crate) timeout: Duration,
    pub(crate) launch: LaunchMode,
    pub(crate) reap_marker: bool,
    pub(crate) retry_text_busy: bool,
}

impl Command {
    pub fn new(program: AbsolutePath, args: Vec<OsString>) -> Self {
        Self {
            program,
            args,
            env: ProcessEnv::empty(),
            cwd: None,
            stdin: StdinSpec::Null,
            stdout: StreamPolicy::Capture {
                cap: DEFAULT_OUTPUT_CAP,
            },
            stderr: StreamPolicy::Capture {
                cap: DEFAULT_OUTPUT_CAP,
            },
            timeout: Duration::from_secs(30),
            launch: LaunchMode::Group,
            reap_marker: false,
            retry_text_busy: false,
        }
    }

    pub fn env(mut self, env: ProcessEnv) -> Self {
        self.env = env;
        self
    }

    pub fn with_env(mut self, env: ProcessEnv) -> Self {
        self.env = env;
        self
    }

    pub fn set_env(&mut self, env: ProcessEnv) -> &mut Self {
        self.env = env;
        self
    }

    pub fn set_cwd(&mut self, cwd: Option<PathBuf>) -> &mut Self {
        self.cwd = cwd;
        self
    }

    pub fn set_stdin(&mut self, stdin: StdinSpec) -> &mut Self {
        self.stdin = stdin;
        self
    }

    pub fn set_stdout(&mut self, stdout: StreamPolicy) -> &mut Self {
        self.stdout = stdout;
        self
    }

    pub fn set_stderr(&mut self, stderr: StreamPolicy) -> &mut Self {
        self.stderr = stderr;
        self
    }

    pub fn set_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.timeout = timeout;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn set_launch_mode(&mut self, launch: LaunchMode) -> &mut Self {
        self.launch = launch;
        self
    }

    pub fn set_reap_marker(&mut self, enabled: bool) -> &mut Self {
        self.reap_marker = enabled;
        self
    }

    pub fn set_retry_text_busy(&mut self, enabled: bool) -> &mut Self {
        self.retry_text_busy = enabled;
        self
    }
}

impl fmt::Debug for Command {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Command")
            .field("program", &self.program)
            .field("args_len", &self.args.len())
            .field("env", &self.env)
            .field("cwd_set", &self.cwd.is_some())
            .field("stdin", &self.stdin)
            .field("stdout", &self.stdout)
            .field("stderr", &self.stderr)
            .field("timeout", &self.timeout)
            .field("launch", &self.launch)
            .field("reap_marker", &self.reap_marker)
            .field("retry_text_busy", &self.retry_text_busy)
            .finish()
    }
}

pub struct Output {
    pub status: ExitStatus,
    pub stdout: Zeroizing<Vec<u8>>,
    pub stderr: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for Output {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Output")
            .field("status", &"<exit status>")
            .field("stdout_len", &self.stdout.len())
            .field("stderr_len", &self.stderr.len())
            .finish()
    }
}
