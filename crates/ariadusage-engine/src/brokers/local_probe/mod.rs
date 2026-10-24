// Ported from CodexBar Sources/CodexBarCore/DarwinProcessEnumerator.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityStatusProbe.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityStatusProbe+PortDetection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
//! LocalProbe broker for same-user process and listening TCP port discovery.

pub mod error;
#[cfg(target_os = "linux")]
mod lsof;
pub mod proc_net;

use std::fmt;
#[cfg(unix)]
use std::io::Read;
use std::path::Path;
#[cfg(any(unix, feature = "test-hooks"))]
use std::path::PathBuf;
use std::time::Duration;

use crate::brokers::call::BrokerCall;
use crate::brokers::process::ProcessIdentity;
#[cfg(unix)]
use crate::brokers::process::procscan;

pub use error::{ListenerCommandError, ProbeError};
pub use proc_net::{parse_tcp_table, socket_inode};

/// Non-owning view of a process's command-line arguments seen by the filter predicate.
pub struct ArgvView<'a> {
    argv: &'a [String],
}

impl<'a> ArgvView<'a> {
    pub fn new(argv: &'a [String]) -> Self {
        Self { argv }
    }

    pub fn argv(&self) -> &[String] {
        self.argv
    }

    /// Joins arguments with spaces so regexes written against `ps` output match unchanged.
    pub fn command_line(&self) -> String {
        self.argv.join(" ")
    }
}

impl fmt::Debug for ArgvView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArgvView")
            .field("argc", &self.argv.len())
            .finish()
    }
}

/// Redacted wrapper around process command-line arguments retained after matching.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretArgv {
    argv: Vec<String>,
}

impl SecretArgv {
    pub fn new(argv: Vec<String>) -> Self {
        Self { argv }
    }

    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    pub fn into_inner(self) -> Vec<String> {
        self.argv
    }

    pub fn command_line(&self) -> String {
        self.argv.join(" ")
    }
}

impl fmt::Debug for SecretArgv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretArgv")
            .field("argc", &self.argv.len())
            .finish()
    }
}

/// A matched same-user process in the engine's network namespace.
#[derive(Clone, PartialEq, Eq)]
pub struct ProbeProcess {
    pub identity: ProcessIdentity,
    pub argv: SecretArgv,
}

impl fmt::Debug for ProbeProcess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProbeProcess")
            .field("identity", &self.identity)
            .field("argv", &self.argv)
            .finish()
    }
}

/// Parses NUL-separated `/proc/<pid>/cmdline` bytes into individual argument strings.
pub fn parse_cmdline(bytes: &[u8]) -> Vec<String> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let slice = if bytes.ends_with(&[0]) {
        &bytes[..bytes.len() - 1]
    } else {
        bytes
    };
    if slice.is_empty() {
        return Vec::new();
    }
    slice
        .split(|&b| b == 0)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect()
}

/// The LocalProbe broker discovery interface.
#[derive(Clone, Debug)]
pub struct LocalProbe {
    #[cfg(target_os = "linux")]
    listener_command: lsof::ListenerCommand,
    #[cfg(unix)]
    proc_root: PathBuf,
    #[cfg(unix)]
    uid: u32,
    #[cfg(unix)]
    own_net_ns: Option<String>,
}

#[cfg(unix)]
fn proc_source_available(proc_root: &Path) -> bool {
    #[cfg(target_os = "macos")]
    if procscan::is_live_proc_root(proc_root) {
        return true;
    }
    proc_root.exists()
}

#[cfg(unix)]
fn read_process_argv(proc_root: &Path, pid: i32) -> Option<Vec<String>> {
    #[cfg(target_os = "macos")]
    if procscan::is_live_proc_root(proc_root) {
        return procscan::darwin_process_argv(pid);
    }
    let cmdline_path = proc_root.join(pid.to_string()).join("cmdline");
    let file = std::fs::File::open(cmdline_path).ok()?;
    let mut bytes = Vec::new();
    file.take(1024 * 1024).read_to_end(&mut bytes).ok()?;
    Some(parse_cmdline(&bytes))
}

#[cfg(target_os = "linux")]
fn same_effective_user(proc_root: &Path, pid: i32, uid: u32) -> bool {
    let Ok(status) = std::fs::read_to_string(proc_root.join(pid.to_string()).join("status")) else {
        return false;
    };
    let Some(line) = status.lines().find_map(|line| line.strip_prefix("Uid:")) else {
        return false;
    };
    let mut uids = line.split_whitespace().map(str::parse::<u32>);
    uids.next() == Some(Ok(uid)) && uids.next() == Some(Ok(uid))
}

impl LocalProbe {
    /// Creates a LocalProbe with system defaults.
    pub fn new() -> Self {
        #[cfg(unix)]
        {
            let proc_root = PathBuf::from("/proc");
            let uid = rustix::process::getuid().as_raw();
            let own_net_ns = procscan::process_net_ns(&proc_root, std::process::id() as i32);
            Self {
                proc_root,
                uid,
                own_net_ns,
                #[cfg(target_os = "linux")]
                listener_command: lsof::ListenerCommand::default(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {}
        }
    }

    /// Creates a LocalProbe with injected proc root, uid, and own network namespace identifier.
    #[cfg(feature = "test-hooks")]
    pub fn with_hooks(proc_root: PathBuf, uid: u32, own_net_ns: Option<String>) -> Self {
        #[cfg(unix)]
        {
            let resolved_ns = own_net_ns.or_else(|| {
                std::fs::read_link(proc_root.join("self/ns/net"))
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
                    .or_else(|| {
                        procscan::process_net_ns(Path::new("/proc"), std::process::id() as i32)
                    })
            });
            Self {
                proc_root,
                uid,
                own_net_ns: resolved_ns,
                #[cfg(target_os = "linux")]
                listener_command: lsof::ListenerCommand::with_candidates(Vec::new()),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (proc_root, uid, own_net_ns);
            Self {}
        }
    }

    /// Overrides the lsof candidate for invented executable/proc fixtures.
    #[cfg(all(target_os = "linux", feature = "test-hooks"))]
    pub fn with_listener_executable(mut self, executable: Option<PathBuf>) -> Self {
        self.listener_command =
            lsof::ListenerCommand::with_candidates(executable.into_iter().collect());
        self
    }

    #[cfg(target_os = "linux")]
    fn listener_identity(&self, pid: i32) -> Option<ProcessIdentity> {
        if !same_effective_user(&self.proc_root, pid, self.uid)
            || self.own_net_ns.is_none()
            || procscan::process_net_ns(&self.proc_root, pid) != self.own_net_ns
        {
            return None;
        }
        procscan::process_identity(&self.proc_root, pid)
    }

    /// Accessor for the configured proc root path.
    pub fn proc_root(&self) -> &Path {
        #[cfg(unix)]
        {
            &self.proc_root
        }
        #[cfg(not(unix))]
        {
            Path::new("/proc")
        }
    }

    /// Scans processes owned by the user in the engine's network namespace.
    ///
    /// The caller's `keep` predicate sees an `ArgvView`. The command-line arguments of
    /// non-matching processes are dropped immediately. Kept arguments are wrapped in `SecretArgv`.
    pub async fn same_user_processes<F>(
        &self,
        call: &BrokerCall,
        keep: F,
    ) -> Result<Vec<ProbeProcess>, ProbeError>
    where
        F: FnMut(&ArgvView<'_>) -> bool + Send + 'static,
    {
        #[cfg(not(unix))]
        {
            let _ = (call, keep);
            Err(ProbeError::Unsupported)
        }
        #[cfg(unix)]
        {
            if call.cancel.is_cancelled() {
                return Err(ProbeError::Cancelled);
            }
            if !proc_source_available(&self.proc_root) {
                return Err(ProbeError::ProcUnavailable);
            }
            let Some(own_net_ns) = self.own_net_ns.clone() else {
                return Err(ProbeError::ProcUnavailable);
            };

            let proc_root = self.proc_root.clone();
            let uid = self.uid;
            let cancel = call.cancel.clone();

            let scan =
                tokio::task::spawn_blocking(move || -> Result<Vec<ProbeProcess>, ProbeError> {
                    let mut keep = keep;
                    let identities = procscan::same_uid_processes(&proc_root, uid);
                    let mut matched = Vec::new();
                    for identity in identities {
                        if cancel.is_cancelled() {
                            return Err(ProbeError::Cancelled);
                        }
                        #[cfg(target_os = "linux")]
                        if !same_effective_user(&proc_root, identity.pid, uid) {
                            continue;
                        }
                        let net_ns = procscan::process_net_ns(&proc_root, identity.pid);
                        if net_ns.as_ref() != Some(&own_net_ns) {
                            continue;
                        }
                        let Some(argv) = read_process_argv(&proc_root, identity.pid) else {
                            continue;
                        };
                        let view = ArgvView::new(&argv);
                        if keep(&view) {
                            matched.push(ProbeProcess {
                                identity,
                                argv: SecretArgv::new(argv),
                            });
                        }
                    }
                    Ok(matched)
                });

            tokio::select! {
                _ = call.cancel.cancelled() => Err(ProbeError::Cancelled),
                res = scan => match res {
                    Ok(result) => {
                        if call.cancel.is_cancelled() {
                            Err(ProbeError::Cancelled)
                        } else {
                            result
                        }
                    }
                    Err(_) => Err(ProbeError::ProcUnavailable),
                }
            }
        }
    }

    pub fn owned_process_identity(&self, pid: i32) -> Option<ProcessIdentity> {
        #[cfg(unix)]
        {
            if procscan::process_uid(&self.proc_root, pid) == Some(self.uid) {
                procscan::process_identity(&self.proc_root, pid)
            } else {
                None
            }
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            None
        }
    }

    /// Returns kernel executable identity only for the same owned process generation.
    pub fn executable_path(&self, identity: ProcessIdentity) -> Option<std::path::PathBuf> {
        #[cfg(unix)]
        {
            if procscan::process_uid(&self.proc_root, identity.pid) != Some(self.uid)
                || procscan::process_identity(&self.proc_root, identity.pid) != Some(identity)
            {
                return None;
            }
            #[cfg(target_os = "macos")]
            let path = if procscan::is_live_proc_root(&self.proc_root) {
                use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, UpdateKind};
                let pid = Pid::from_u32(u32::try_from(identity.pid).ok()?);
                let mut system = sysinfo::System::new();
                system.refresh_processes_specifics(
                    ProcessesToUpdate::Some(&[pid]),
                    true,
                    ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
                );
                system
                    .process(pid)
                    .and_then(|p| p.exe())
                    .map(std::path::Path::to_path_buf)
            } else {
                std::fs::read_link(self.proc_root.join(identity.pid.to_string()).join("exe")).ok()
            };
            #[cfg(not(target_os = "macos"))]
            let path =
                std::fs::read_link(self.proc_root.join(identity.pid.to_string()).join("exe")).ok();
            if procscan::process_identity(&self.proc_root, identity.pid) == Some(identity) {
                path
            } else {
                None
            }
        }
        #[cfg(not(unix))]
        {
            let _ = identity;
            None
        }
    }

    /// Discovers listening TCP ports for the specified `pid` from its open socket descriptors.
    pub async fn listening_tcp_ports(
        &self,
        call: &BrokerCall,
        pid: i32,
        timeout: Duration,
    ) -> Result<Vec<u16>, ProbeError> {
        #[cfg(not(unix))]
        {
            let _ = (call, pid, timeout);
            Err(ProbeError::Unsupported)
        }
        #[cfg(unix)]
        {
            if call.cancel.is_cancelled() {
                return Err(ProbeError::Cancelled);
            }
            if timeout.is_zero() {
                return Err(ProbeError::TimedOut);
            }
            if pid <= 0 {
                return Err(ProbeError::NoListeningPorts);
            }
            if !proc_source_available(&self.proc_root) {
                return Err(ProbeError::ProcUnavailable);
            }

            let started = std::time::Instant::now();
            #[cfg(target_os = "linux")]
            let identity = self
                .listener_identity(pid)
                .ok_or(ProbeError::NoListeningPorts)?;
            #[cfg(target_os = "linux")]
            let pending = {
                let result = self.listener_command.ports(call, pid, timeout).await;
                if call.cancel.is_cancelled() {
                    return Err(ProbeError::Cancelled);
                }
                if matches!(
                    &result,
                    Err(ProbeError::ListenerCommand(_)
                        | ProbeError::Cancelled
                        | ProbeError::TimedOut)
                ) {
                    return result;
                }
                if self.listener_identity(pid) != Some(identity) {
                    return Err(ProbeError::NoListeningPorts);
                }
                match result {
                    Ok(ports) if !ports.is_empty() => return Ok(ports),
                    Ok(_) => None,
                    Err(error @ ProbeError::ListenerDiscoveryPending(_)) => Some(error),
                    Err(error) => return Err(error),
                }
            };
            let timeout = timeout.saturating_sub(started.elapsed());
            if timeout.is_zero() {
                return Err(ProbeError::TimedOut);
            }
            let proc_root = self.proc_root.clone();
            let cancel = call.cancel.clone();

            let scan = tokio::task::spawn_blocking(move || -> Vec<u16> {
                if cancel.is_cancelled() {
                    return Vec::new();
                }
                proc_net::proc_listening_ports(&proc_root, pid)
            });

            tokio::select! {
                _ = call.cancel.cancelled() => Err(ProbeError::Cancelled),
                res = tokio::time::timeout(timeout, scan) => match res {
                    Ok(Ok(ports)) => {
                        if call.cancel.is_cancelled() {
                            return Err(ProbeError::Cancelled);
                        }
                        #[cfg(target_os = "linux")]
                        if self.listener_identity(pid) != Some(identity) {
                            return Err(ProbeError::NoListeningPorts);
                        }
                        if ports.is_empty() {
                            #[cfg(target_os = "linux")]
                            if let Some(error) = pending { return Err(error); }
                            Err(ProbeError::NoListeningPorts)
                        } else {
                            Ok(ports)
                        }
                    }
                    Ok(Err(_join_err)) => Err(ProbeError::ProcUnavailable),
                    Err(_elapsed) => Err(ProbeError::TimedOut),
                }
            }
        }
    }

    /// Rechecks process identity, network namespace, and whether a LISTEN socket for `port`
    /// is still held by `identity.pid`.
    pub async fn listener_still_owned(
        &self,
        call: &BrokerCall,
        identity: ProcessIdentity,
        port: u16,
    ) -> Result<bool, ProbeError> {
        #[cfg(not(unix))]
        {
            let _ = (call, identity, port);
            Err(ProbeError::Unsupported)
        }
        #[cfg(unix)]
        {
            if call.cancel.is_cancelled() {
                return Err(ProbeError::Cancelled);
            }
            if identity.pid <= 0 {
                return Ok(false);
            }
            if !proc_source_available(&self.proc_root) {
                return Err(ProbeError::ProcUnavailable);
            }

            #[cfg(target_os = "linux")]
            if self.listener_identity(identity.pid) != Some(identity) {
                return Ok(false);
            }
            let proc_root = self.proc_root.clone();
            let uid = self.uid;
            let own_net_ns = self.own_net_ns.clone();
            let cancel = call.cancel.clone();

            let check = tokio::task::spawn_blocking(move || -> Result<bool, ProbeError> {
                if cancel.is_cancelled() {
                    return Err(ProbeError::Cancelled);
                }
                // 1. Recheck process identity
                let current_ident = procscan::process_identity(&proc_root, identity.pid);
                if current_ident != Some(identity) {
                    return Ok(false);
                }
                // 2. Recheck process uid
                let current_uid = procscan::process_uid(&proc_root, identity.pid);
                if current_uid != Some(uid) {
                    return Ok(false);
                }
                #[cfg(target_os = "linux")]
                if !same_effective_user(&proc_root, identity.pid, uid) {
                    return Ok(false);
                }
                // 3. Recheck network namespace
                let current_ns = procscan::process_net_ns(&proc_root, identity.pid);
                if current_ns.is_none() || own_net_ns.is_none() || current_ns != own_net_ns {
                    return Ok(false);
                }
                // 4. Recheck listening socket ownership
                let ports = proc_net::proc_listening_ports(&proc_root, identity.pid);
                if !ports.contains(&port)
                    || procscan::process_identity(&proc_root, identity.pid) != Some(identity)
                {
                    return Ok(false);
                }
                #[cfg(target_os = "linux")]
                if !same_effective_user(&proc_root, identity.pid, uid)
                    || procscan::process_net_ns(&proc_root, identity.pid) != own_net_ns
                {
                    return Ok(false);
                }
                Ok(true)
            });

            tokio::select! {
                _ = call.cancel.cancelled() => Err(ProbeError::Cancelled),
                res = check => match res {
                    Ok(result) => {
                        if call.cancel.is_cancelled() {
                            Err(ProbeError::Cancelled)
                        } else {
                            result
                        }
                    }
                    Err(_) => Err(ProbeError::ProcUnavailable),
                }
            }
        }
    }
}

impl Default for LocalProbe {
    fn default() -> Self {
        Self::new()
    }
}

/// Standalone entry point: scans same-user processes matching `keep` in the engine's network namespace.
pub async fn same_user_processes<F>(
    call: &BrokerCall,
    keep: F,
) -> Result<Vec<ProbeProcess>, ProbeError>
where
    F: FnMut(&ArgvView<'_>) -> bool + Send + 'static,
{
    LocalProbe::default().same_user_processes(call, keep).await
}

/// Standalone entry point: discovers listening TCP ports owned by `pid`.
pub async fn listening_tcp_ports(
    call: &BrokerCall,
    pid: i32,
    timeout: Duration,
) -> Result<Vec<u16>, ProbeError> {
    LocalProbe::default()
        .listening_tcp_ports(call, pid, timeout)
        .await
}

/// Standalone entry point: rechecks that `identity.pid` still owns a LISTEN socket for `port`.
pub async fn listener_still_owned(
    call: &BrokerCall,
    identity: ProcessIdentity,
    port: u16,
) -> Result<bool, ProbeError> {
    LocalProbe::default()
        .listener_still_owned(call, identity, port)
        .await
}
