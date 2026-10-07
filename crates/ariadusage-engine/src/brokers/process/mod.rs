mod buffers;
mod command;
pub mod drain;
mod env;
mod error;
#[cfg(target_os = "linux")]
mod holders;
mod procscan;
pub mod pty;
#[cfg(target_os = "linux")]
mod reaper;
mod registry;
pub mod rpc;
mod run;
pub mod scan;
mod signal;
#[cfg(target_os = "linux")]
mod teardown;

pub use buffers::{BoundedLineBuffer, BoundedOutputBuffer, LineDrain};
pub use command::{AbsolutePath, Command, LaunchMode, Output, StdinSpec, StreamPolicy};
pub use drain::DrainClassification;
pub use env::ProcessEnv;
pub use error::{OutputStream, ProcessError};
pub use procscan::{
    ProcessIdentity, fd_targets, parse_marker_environment, process_children, process_descendants,
    process_group, process_identity, process_state, process_uid, read_marker_environment,
    same_uid_processes,
};
pub use pty::{
    PtyCompletionReason, PtyError, PtyScript, PtySession, PtySize, PtyTranscript, SendOnSubstring,
    SubstringSource,
};
pub use registry::ProcessRegistry;
pub use rpc::{RpcError, RpcSession};
#[cfg(all(target_os = "linux", feature = "test-hooks"))]
pub use run::retry_spawn_for_test;
pub use run::run;
pub use scan::StreamScanBuffer;
pub use signal::{ProcessSignal, signal, signal_group};

pub const DEFAULT_OUTPUT_CAP: usize = 1024 * 1024;

#[cfg(target_os = "linux")]
fn session_target(
    pid: u32,
    pipe_fds: &[std::os::fd::RawFd],
) -> Result<teardown::ProcessTarget, ProcessError> {
    use std::path::Path;

    use rustix::process::{getpgrp, getpid, getuid};

    let Ok(pid) = i32::try_from(pid) else {
        return Err(ProcessError::LaunchFailed);
    };
    if pid <= 1 || pid == getpid().as_raw_pid() {
        return Err(ProcessError::LaunchFailed);
    }
    let proc_root = Path::new("/proc");
    let identity = process_identity(proc_root, pid).ok_or(ProcessError::LaunchFailed)?;
    let pgid = process_group(proc_root, pid).ok_or(ProcessError::LaunchFailed)?;
    let uid = process_uid(proc_root, pid).ok_or(ProcessError::LaunchFailed)?;
    if pgid <= 1 || pgid != pid || pgid == getpgrp().as_raw_pid() || uid != getuid().as_raw() {
        return Err(ProcessError::LaunchFailed);
    }
    let pipes = pipe_fds
        .iter()
        .filter_map(|fd| {
            let target = std::fs::read_link(format!("/proc/self/fd/{fd}")).ok()?;
            let text = target.to_string_lossy();
            (text.starts_with("pipe:[") && text.ends_with(']')).then_some(target)
        })
        .collect();
    let target = teardown::ProcessTarget::new(identity, pgid, uid, pipes);
    target.refresh_descendants();
    Ok(target)
}
