use super::{ProcessError, ProcessIdentity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSignal {
    Term,
    Kill,
}

pub fn signal(identity: ProcessIdentity, signal: ProcessSignal) -> Result<bool, ProcessError> {
    #[cfg(target_os = "linux")]
    {
        use rustix::process::{Pid, PidfdFlags, getpid, getuid, pidfd_open, pidfd_send_signal};

        if identity.pid <= 1 || identity.pid == getpid().as_raw_pid() {
            return Ok(false);
        }
        let Some(pid) = Pid::from_raw(identity.pid) else {
            return Ok(false);
        };
        let pidfd = match pidfd_open(pid, PidfdFlags::empty()) {
            Ok(pidfd) => pidfd,
            Err(error) => return Err(map_pidfd_error(error)),
        };
        let proc_root = Path::new("/proc");
        if super::process_identity(proc_root, identity.pid) != Some(identity)
            || super::process_uid(proc_root, identity.pid) != Some(getuid().as_raw())
        {
            return Ok(false);
        }
        pidfd_send_signal(&pidfd, rustix_signal(signal)).map_err(map_pidfd_error)?;
        Ok(true)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (identity, signal);
        Err(ProcessError::Unsupported)
    }
}

pub fn signal_group(
    identity: ProcessIdentity,
    pgid: i32,
    signal: ProcessSignal,
) -> Result<bool, ProcessError> {
    #[cfg(target_os = "linux")]
    {
        use rustix::process::{
            Pid, PidfdFlags, getpgrp, getpid, getuid, kill_process_group, pidfd_open,
        };

        if identity.pid <= 1
            || identity.pid == getpid().as_raw_pid()
            || pgid <= 1
            || pgid == getpgrp().as_raw_pid()
        {
            return Ok(false);
        }
        let (Some(pid), Some(group)) = (Pid::from_raw(identity.pid), Pid::from_raw(pgid)) else {
            return Ok(false);
        };
        let pidfd = match pidfd_open(pid, PidfdFlags::empty()) {
            Ok(pidfd) => pidfd,
            Err(error) => return Err(map_pidfd_error(error)),
        };
        let proc_root = Path::new("/proc");
        if super::process_identity(proc_root, identity.pid) != Some(identity)
            || super::process_uid(proc_root, identity.pid) != Some(getuid().as_raw())
            || super::process_group(proc_root, identity.pid) != Some(pgid)
        {
            return Ok(false);
        }
        let _keep_pidfd_open_until_signal_is_sent = pidfd;
        kill_process_group(group, rustix_signal(signal)).map_err(map_pidfd_error)?;
        Ok(true)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (identity, pgid, signal);
        Err(ProcessError::Unsupported)
    }
}

#[cfg(target_os = "linux")]
fn rustix_signal(signal: ProcessSignal) -> rustix::process::Signal {
    match signal {
        ProcessSignal::Term => rustix::process::Signal::TERM,
        ProcessSignal::Kill => rustix::process::Signal::KILL,
    }
}

#[cfg(target_os = "linux")]
fn map_pidfd_error(error: rustix::io::Errno) -> ProcessError {
    if error == rustix::io::Errno::NOSYS {
        ProcessError::Unsupported
    } else {
        ProcessError::Io
    }
}

#[cfg(target_os = "linux")]
use std::path::Path;
