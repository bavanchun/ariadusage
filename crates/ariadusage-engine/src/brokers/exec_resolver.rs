// Ported from CodexBar Sources/CodexBarCore/PathEnvironment.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/CodexExecutableResolver.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::process::{AbsolutePath, ProcessEnv};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Claude,
    Codex,
    Antigravity,
}

impl Tool {
    pub fn binary_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Antigravity => "agy",
        }
    }

    pub fn override_env_key(self) -> &'static str {
        match self {
            Self::Claude => "CLAUDE_CLI_PATH",
            Self::Codex => "CODEX_CLI_PATH",
            Self::Antigravity => "ANTIGRAVITY_CLI_PATH",
        }
    }

    pub fn well_known_paths(self, home: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        match self {
            Self::Claude => {
                paths.push(home.join(".local/bin/claude"));
                paths.push(home.join(".claude/local/claude"));
                paths.push(home.join(".claude/bin/claude"));
                paths.push(PathBuf::from("/opt/homebrew/bin/claude"));
                paths.push(PathBuf::from("/usr/local/bin/claude"));
                paths.push(home.join(".local/share/mise/shims/claude"));
                paths.push(home.join(".asdf/shims/claude"));
            }
            Self::Antigravity => {
                paths.push(home.join(".local/bin/agy"));
                paths.push(PathBuf::from("/opt/homebrew/bin/agy"));
                paths.push(PathBuf::from("/usr/local/bin/agy"));
                paths.push(home.join(".local/share/mise/shims/agy"));
                paths.push(home.join(".asdf/shims/agy"));
            }
            Self::Codex => {
                paths.push(home.join(".local/share/mise/shims/codex"));
                paths.push(home.join(".asdf/shims/codex"));
            }
        }
        paths
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    #[error("executable not found")]
    NotFound,
    #[error("executable override is invalid")]
    OverrideInvalid,
    #[error("executable candidate is untrusted")]
    Untrusted,
    #[error("executable resolver is unsupported on this platform")]
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileOwnerAndMode {
    pub uid: u32,
    pub mode: u32,
}

pub trait ResolverFs: Send + Sync {
    fn is_executable(&self, path: &Path) -> bool;
    fn owner_and_mode(&self, path: &Path) -> Option<FileOwnerAndMode>;

    fn current_uid(&self) -> u32 {
        #[cfg(target_os = "linux")]
        {
            rustix::process::getuid().as_raw()
        }
        #[cfg(not(target_os = "linux"))]
        {
            1000
        }
    }

    fn is_script(&self, path: &Path) -> bool {
        let Ok(mut file) = std::fs::File::open(path) else {
            return false;
        };
        use std::io::Read;
        let mut magic = [0u8; 2];
        file.read_exact(&mut magic).is_ok() && &magic == b"#!"
    }
}

pub struct RealResolverFs;

impl ResolverFs for RealResolverFs {
    fn is_executable(&self, path: &Path) -> bool {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(path)
                .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        }
        #[cfg(not(target_os = "linux"))]
        {
            std::fs::metadata(path)
                .map(|meta| meta.is_file())
                .unwrap_or(false)
        }
    }

    fn owner_and_mode(&self, path: &Path) -> Option<FileOwnerAndMode> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = std::fs::metadata(path).ok()?;
            Some(FileOwnerAndMode {
                uid: meta.uid(),
                mode: meta.mode(),
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            None
        }
    }
}

pub type ShellLookupFn<'a> = &'a (dyn Fn(&str) -> Option<PathBuf> + Send + Sync);

pub struct ResolverInputs<'a> {
    pub env: &'a ProcessEnv,
    pub home: &'a Path,
    pub fs: &'a dyn ResolverFs,
    pub login_path: Option<&'a [PathBuf]>,
    pub command_v: Option<ShellLookupFn<'a>>,
    pub alias: Option<ShellLookupFn<'a>>,
}

impl<'a> ResolverInputs<'a> {
    pub fn new(env: &'a ProcessEnv, home: &'a Path, fs: &'a dyn ResolverFs) -> Self {
        Self {
            env,
            home,
            fs,
            login_path: None,
            command_v: None,
            alias: None,
        }
    }

    pub fn with_login_path(mut self, login_path: &'a [PathBuf]) -> Self {
        self.login_path = Some(login_path);
        self
    }

    pub fn with_command_v(mut self, command_v: ShellLookupFn<'a>) -> Self {
        self.command_v = Some(command_v);
        self
    }

    pub fn with_alias(mut self, alias: ShellLookupFn<'a>) -> Self {
        self.alias = Some(alias);
        self
    }
}

fn is_group_or_world_writable_without_sticky(mode: u32) -> bool {
    let group_writable = (mode & 0o020) != 0;
    let world_writable = (mode & 0o002) != 0;
    let sticky = (mode & 0o1000) != 0;
    (group_writable || world_writable) && !sticky
}

pub fn check_trust(fs: &dyn ResolverFs, candidate: &Path) -> Result<(), ResolveError> {
    let current_uid = fs.current_uid();

    // Check candidate file
    let file_stat = fs
        .owner_and_mode(candidate)
        .ok_or(ResolveError::Untrusted)?;
    if file_stat.uid != current_uid && file_stat.uid != 0 {
        return Err(ResolveError::Untrusted);
    }
    if is_group_or_world_writable_without_sticky(file_stat.mode) {
        return Err(ResolveError::Untrusted);
    }

    // Check parent directory
    let parent = candidate.parent().ok_or(ResolveError::Untrusted)?;
    let parent_stat = fs.owner_and_mode(parent).ok_or(ResolveError::Untrusted)?;
    if parent_stat.uid != current_uid && parent_stat.uid != 0 {
        return Err(ResolveError::Untrusted);
    }
    if is_group_or_world_writable_without_sticky(parent_stat.mode) {
        return Err(ResolveError::Untrusted);
    }

    Ok(())
}

pub fn effective_path(login: Option<&[PathBuf]>, current: Option<&std::ffi::OsStr>) -> OsString {
    let mut parts = Vec::new();

    if let Some(login_paths) = login {
        for path in login_paths {
            if path.is_absolute() {
                parts.push(path.clone());
            }
        }
    }

    if let Some(current_path) = current {
        for path in std::env::split_paths(current_path) {
            if path.is_absolute() {
                parts.push(path);
            }
        }
    }

    if parts.is_empty() {
        parts = vec![
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
            PathBuf::from("/usr/sbin"),
            PathBuf::from("/sbin"),
        ];
    }

    let mut seen = HashSet::new();
    let mut unique_parts = Vec::new();
    for path in parts {
        if seen.insert(path.clone()) {
            unique_parts.push(path);
        }
    }

    std::env::join_paths(unique_parts).unwrap_or_default()
}

pub fn resolve(tool: Tool, inputs: &ResolverInputs<'_>) -> Result<AbsolutePath, ResolveError> {
    // 1) Override variable
    if tool == Tool::Antigravity {
        if let Some(override_val) = inputs.env.get(tool.override_env_key()) {
            let val_str = override_val.to_str().unwrap_or("");
            let trimmed = val_str.trim();
            if trimmed.is_empty() || !val_str.starts_with('/') {
                return Err(ResolveError::OverrideInvalid);
            }
            let path = PathBuf::from(val_str);
            if !inputs.fs.is_executable(&path) {
                return Err(ResolveError::OverrideInvalid);
            }
            check_trust(inputs.fs, &path)?;
            return AbsolutePath::from_absolute_path(path)
                .map_err(|_| ResolveError::OverrideInvalid);
        }
    } else if let Some(override_val) = inputs.env.get(tool.override_env_key()) {
        let val_str = override_val.to_str().unwrap_or("");
        if val_str.starts_with('/') {
            let path = PathBuf::from(val_str);
            if inputs.fs.is_executable(&path) && check_trust(inputs.fs, &path).is_ok() {
                return AbsolutePath::from_absolute_path(path)
                    .map_err(|_| ResolveError::OverrideInvalid);
            }
        }
        // Non-executable or untrusted falls through
    }

    // 2) Login-shell PATH
    if let Some(login_paths) = inputs.login_path {
        for dir in login_paths {
            if !dir.is_absolute() {
                continue;
            }
            let candidate = dir.join(tool.binary_name());
            if inputs.fs.is_executable(&candidate) {
                check_trust(inputs.fs, &candidate)?;
                return AbsolutePath::from_absolute_path(candidate)
                    .map_err(|_| ResolveError::NotFound);
            }
        }
    }

    // 3) Existing PATH
    if let Some(current_path) = inputs.env.get("PATH") {
        for dir in std::env::split_paths(current_path) {
            if !dir.is_absolute() {
                continue;
            }
            let candidate = dir.join(tool.binary_name());
            if inputs.fs.is_executable(&candidate) {
                check_trust(inputs.fs, &candidate)?;
                return AbsolutePath::from_absolute_path(candidate)
                    .map_err(|_| ResolveError::NotFound);
            }
        }
    }

    // 4) Well-known installation paths
    for candidate in tool.well_known_paths(inputs.home) {
        if inputs.fs.is_executable(&candidate) {
            check_trust(inputs.fs, &candidate)?;
            return AbsolutePath::from_absolute_path(candidate).map_err(|_| ResolveError::NotFound);
        }
    }

    // 5) Interactive login shell lookup (command -v)
    if let Some(command_v) = inputs.command_v
        && let Some(hit) = command_v(tool.binary_name())
        && hit.is_absolute()
        && inputs.fs.is_executable(&hit)
    {
        check_trust(inputs.fs, &hit)?;
        return AbsolutePath::from_absolute_path(hit).map_err(|_| ResolveError::NotFound);
    }

    // 5b) Alias fallback
    if let Some(alias) = inputs.alias
        && let Some(hit) = alias(tool.binary_name())
        && hit.is_absolute()
        && inputs.fs.is_executable(&hit)
    {
        check_trust(inputs.fs, &hit)?;
        return AbsolutePath::from_absolute_path(hit).map_err(|_| ResolveError::NotFound);
    }

    // 6) Minimal fallback
    for dir in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        let candidate = Path::new(dir).join(tool.binary_name());
        if inputs.fs.is_executable(&candidate) {
            check_trust(inputs.fs, &candidate)?;
            return AbsolutePath::from_absolute_path(candidate).map_err(|_| ResolveError::NotFound);
        }
    }

    Err(ResolveError::NotFound)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexResolution {
    pub executable: AbsolutePath,
    pub login_path: Option<Vec<PathBuf>>,
}

pub fn resolve_codex_for_rpc(
    inputs: &ResolverInputs<'_>,
    executable: &str,
    capture_login_path: impl FnOnce() -> Option<Vec<PathBuf>>,
) -> Result<CodexResolution, ResolveError> {
    // 1) Explicit override CODEX_CLI_PATH
    if let Some(override_val) = inputs.env.get("CODEX_CLI_PATH") {
        let val_str = override_val.to_str().unwrap_or("");
        if val_str.starts_with('/') {
            let path = PathBuf::from(val_str);
            if inputs.fs.is_executable(&path) && check_trust(inputs.fs, &path).is_ok() {
                let is_script = inputs.fs.is_script(&path);
                let login_path = if is_script {
                    capture_login_path()
                } else {
                    None
                };
                let abs_path = AbsolutePath::from_absolute_path(path)
                    .map_err(|_| ResolveError::OverrideInvalid)?;
                return Ok(CodexResolution {
                    executable: abs_path,
                    login_path,
                });
            }
        }
    }

    // 2) General resolver with captured login PATH
    let captured_login = capture_login_path();
    let inputs_with_login = ResolverInputs {
        login_path: captured_login.as_deref(),
        ..*inputs
    };

    match resolve(Tool::Codex, &inputs_with_login) {
        Ok(resolved) => Ok(CodexResolution {
            executable: resolved,
            login_path: captured_login,
        }),
        Err(ResolveError::NotFound) => {
            if executable.contains('/') {
                let explicit_path = if Path::new(executable).is_absolute() {
                    PathBuf::from(executable)
                } else {
                    std::env::current_dir()
                        .unwrap_or_else(|_| PathBuf::from("/"))
                        .join(executable)
                };
                if inputs.fs.is_executable(&explicit_path)
                    && check_trust(inputs.fs, &explicit_path).is_ok()
                {
                    let abs_path = AbsolutePath::from_absolute_path(explicit_path)
                        .map_err(|_| ResolveError::NotFound)?;
                    return Ok(CodexResolution {
                        executable: abs_path,
                        login_path: captured_login,
                    });
                }
            }
            Err(ResolveError::NotFound)
        }
        Err(err) => Err(err),
    }
}
