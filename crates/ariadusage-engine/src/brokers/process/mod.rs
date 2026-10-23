mod buffers;
mod command;
pub mod drain;
mod env;
mod error;
#[cfg(unix)]
mod holders;
pub mod procscan;
pub mod pty;
#[cfg(unix)]
mod reaper;
mod registry;
pub mod rpc;
mod run;
pub mod scan;
mod signal;
#[cfg(unix)]
mod teardown;

pub use buffers::{BoundedLineBuffer, BoundedOutputBuffer, LineDrain};
pub use command::{AbsolutePath, Command, LaunchMode, Output, StdinSpec, StreamPolicy};
pub use drain::DrainClassification;
pub use env::ProcessEnv;
pub use error::{OutputStream, ProcessError};
pub use procscan::{
    ProcessIdentity, fd_targets, parse_marker_environment, parse_procargs2_arguments,
    parse_procargs2_environment, parse_procargs2_marker, process_children, process_descendants,
    process_group, process_identity, process_net_ns, process_state, process_uid,
    read_marker_environment, same_uid_processes,
};
pub use pty::{
    PtyCompletionReason, PtyError, PtyScript, PtySession, PtySize, PtyTranscript, SendOnSubstring,
    SubstringSource,
};
pub use registry::ProcessRegistry;
pub use rpc::{RpcError, RpcSession};
#[cfg(all(unix, feature = "test-hooks"))]
pub use run::retry_spawn_for_test;
pub use run::run;
pub use scan::StreamScanBuffer;
pub use signal::{ProcessSignal, signal, signal_group};

pub const DEFAULT_OUTPUT_CAP: usize = 1024 * 1024;

#[cfg(unix)]
pub(super) fn output_pipe_targets(fd: std::os::fd::RawFd) -> Vec<std::path::PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let Ok(target) = std::fs::read_link(format!("/proc/self/fd/{fd}")) else {
            return Vec::new();
        };
        let text = target.to_string_lossy();
        if text.starts_with("pipe:[") && text.ends_with(']') {
            vec![target]
        } else {
            Vec::new()
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::path::PathBuf;

        use proc_pidinfo::{Fd, PipeFdInfo, proc_pidfdinfo_self};

        let mut targets = Vec::new();
        if let Ok(Some(pipe)) = proc_pidfdinfo_self::<PipeFdInfo>(Fd(fd)) {
            let handle = pipe.pipe_info.pipe_handle;
            if handle != 0 {
                targets.push(PathBuf::from(format!("pipe:[{handle:x}]")));
            }
        }
        targets
    }
}

#[cfg(unix)]
pub(super) fn capture_output_pipes(
    pipe_fds: &[std::os::fd::BorrowedFd<'_>],
) -> (Vec<std::path::PathBuf>, Vec<std::os::fd::OwnedFd>) {
    use std::os::fd::AsRawFd;

    let mut pipes = Vec::new();
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut holds = Vec::new();
    for &fd in pipe_fds {
        #[cfg(target_os = "macos")]
        if let Ok(dup_fd) = rustix::io::fcntl_dupfd_cloexec(fd, 3) {
            holds.push(dup_fd);
        }
        pipes.extend(output_pipe_targets(fd.as_raw_fd()));
    }
    pipes.sort();
    pipes.dedup();
    (pipes, holds)
}

#[cfg(unix)]
fn session_target(
    pid: u32,
    pipe_fds: &[std::os::fd::BorrowedFd<'_>],
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
    let identity = match process_identity(proc_root, pid) {
        Some(identity) => identity,
        None if process_state(proc_root, pid) == Some('Z') => ProcessIdentity {
            pid,
            start_ticks: 0,
        },
        None => return Err(ProcessError::LaunchFailed),
    };
    let pgid = process_group(proc_root, pid).ok_or(ProcessError::LaunchFailed)?;
    let uid = process_uid(proc_root, pid).ok_or(ProcessError::LaunchFailed)?;
    if pgid <= 1 || pgid != pid || pgid == getpgrp().as_raw_pid() || uid != getuid().as_raw() {
        return Err(ProcessError::LaunchFailed);
    }
    let (pipes, pipe_holds) = capture_output_pipes(pipe_fds);
    let target = teardown::ProcessTarget::with_pipe_holds(identity, pgid, uid, pipes, pipe_holds);
    target.refresh_descendants();
    Ok(target)
}
