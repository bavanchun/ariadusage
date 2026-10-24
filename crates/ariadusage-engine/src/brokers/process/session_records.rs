// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityCLISession.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Persistent ownership records for broker-managed warm sessions.
//!
//! This module owns filesystem I/O and lock coordination. Provider modules use
//! the traits only and remain policy/state-machine code.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::process_identity;
#[cfg(unix)]
use super::{ProcessSignal, read_marker_environment, signal_group};

pub type ProcessFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub pid: u32,
    pub requested_binary_path: PathBuf,
    pub executable_path: String,
    pub start_epoch: f64,
    pub process_group: Option<u32>,
    pub owner_pid: Option<u32>,
    pub owner_executable_path: Option<String>,
    pub owner_start_epoch: Option<f64>,
    #[serde(default)]
    pub marker: Option<String>,
}

impl std::fmt::Debug for SessionRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionRecord")
            .field("pid", &"<redacted>")
            .field("requested_binary_path", &"<redacted>")
            .field("executable_path", &"<redacted>")
            .field("start_epoch", &"<redacted>")
            .field("process_group", &self.process_group.map(|_| "<redacted>"))
            .field("owner_pid", &self.owner_pid.map(|_| "<redacted>"))
            .field(
                "owner_executable_path",
                &self.owner_executable_path.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "owner_start_epoch",
                &self.owner_start_epoch.map(|_| "<redacted>"),
            )
            .field("marker", &self.marker.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

pub trait SessionRecordStore: Send + Sync {
    fn load(&self) -> Result<Vec<SessionRecord>, String>;
    fn save(&self, record: &SessionRecord) -> Result<(), String>;
    fn remove(&self, record: &SessionRecord) -> Result<(), String>;
}

pub trait SessionLaunchLock: Send + Sync {
    fn acquire(&self) -> ProcessFuture<Result<(), String>>;
    fn release(&self);
}

pub trait SessionRecordIdentity: Send + Sync {
    fn identity(&self, pid: u32) -> Option<(String, f64)>;
    fn current_pid(&self) -> u32;
    fn current_identity(&self) -> Option<(String, f64)>;
    fn terminate_record(&self, record: SessionRecord) -> ProcessFuture<()>;
}

/// Production identity adapter. Signals only the recorded group after exact
/// PID start identity, uid, group and broker marker checks.
pub struct ProcSessionRecordIdentity;

impl SessionRecordIdentity for ProcSessionRecordIdentity {
    fn identity(&self, pid: u32) -> Option<(String, f64)> {
        let pid_i32 = i32::try_from(pid).ok()?;
        let root = Path::new("/proc");
        let identity = process_identity(root, pid_i32)?;
        let executable =
            crate::brokers::local_probe::LocalProbe::new().executable_path(identity)?;
        Some((
            executable.to_string_lossy().into_owned(),
            identity.start_ticks as f64,
        ))
    }

    fn current_pid(&self) -> u32 {
        std::process::id()
    }

    fn current_identity(&self) -> Option<(String, f64)> {
        self.identity(self.current_pid())
    }

    fn terminate_record(&self, record: SessionRecord) -> ProcessFuture<()> {
        #[cfg(not(unix))]
        {
            let _ = record;
            Box::pin(async {})
        }
        #[cfg(unix)]
        {
            Box::pin(async move {
                let Ok(pid) = i32::try_from(record.pid) else {
                    return;
                };
                let root = Path::new("/proc");
                let Some(identity) = process_identity(root, pid) else {
                    return;
                };
                if (identity.start_ticks as f64 - record.start_epoch).abs() >= 0.001 {
                    return;
                }
                let Some(group) = record
                    .process_group
                    .and_then(|group| i32::try_from(group).ok())
                else {
                    return;
                };
                let Some(marker) = record.marker.as_deref() else {
                    return;
                };
                let marker_matches =
                    read_marker_environment(root, pid, super::procscan::PROCESS_MARKER_ENV)
                        .is_some_and(|current| current.as_slice() == marker.as_bytes());
                if !marker_matches {
                    return;
                }
                if !signal_group(identity, group, ProcessSignal::Term).unwrap_or(false) {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
                if process_identity(root, pid) != Some(identity)
                    || !read_marker_environment(root, pid, super::procscan::PROCESS_MARKER_ENV)
                        .is_some_and(|current| current.as_slice() == marker.as_bytes())
                {
                    return;
                }
                let _ = signal_group(identity, group, ProcessSignal::Kill);
            })
        }
    }
}

pub struct FileSessionRecordStore {
    path: PathBuf,
}

impl FileSessionRecordStore {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn same_owner(lhs: &SessionRecord, rhs: &SessionRecord) -> bool {
        match (
            lhs.owner_pid,
            rhs.owner_pid,
            lhs.owner_executable_path.as_deref(),
            rhs.owner_executable_path.as_deref(),
            lhs.owner_start_epoch,
            rhs.owner_start_epoch,
        ) {
            (Some(lp), Some(rp), Some(le), Some(re), Some(ls), Some(rs)) => {
                lp == rp && le == re && (ls - rs).abs() < 0.001
            }
            _ => {
                lhs.pid == rhs.pid
                    && lhs.executable_path == rhs.executable_path
                    && (lhs.start_epoch - rhs.start_epoch).abs() < 0.001
            }
        }
    }
}

impl SessionRecordStore for FileSessionRecordStore {
    fn load(&self) -> Result<Vec<SessionRecord>, String> {
        const RECORD_CAP: u64 = 1024 * 1024;
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let file = std::fs::File::open(&self.path).map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        file.take(RECORD_CAP + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > RECORD_CAP {
            return Err("session record exceeds size cap".to_owned());
        }
        serde_json::from_slice::<Vec<SessionRecord>>(&bytes)
            .or_else(|_| serde_json::from_slice::<SessionRecord>(&bytes).map(|record| vec![record]))
            .map_err(|error| error.to_string())
    }

    fn save(&self, record: &SessionRecord) -> Result<(), String> {
        let mut records = self.load().unwrap_or_default();
        records.retain(|existing| !Self::same_owner(existing, record));
        records.push(record.clone());
        let bytes = serde_json::to_vec(&records).map_err(|error| error.to_string())?;
        crate::private_file::write_private(
            &self.path,
            &bytes,
            &crate::private_file::WriteHooks::default(),
        )
        .map_err(|error| error.to_string())
    }

    fn remove(&self, record: &SessionRecord) -> Result<(), String> {
        let mut records = self.load()?;
        records.retain(|existing| {
            !(existing.pid == record.pid
                && existing.executable_path == record.executable_path
                && (existing.start_epoch - record.start_epoch).abs() < 0.001)
        });
        if records.is_empty() {
            if self.path.exists() {
                std::fs::remove_file(&self.path).map_err(|error| error.to_string())?;
            }
            return Ok(());
        }
        let bytes = serde_json::to_vec(&records).map_err(|error| error.to_string())?;
        crate::private_file::write_private(
            &self.path,
            &bytes,
            &crate::private_file::WriteHooks::default(),
        )
        .map_err(|error| error.to_string())
    }
}

pub struct FileSessionLaunchLock {
    #[cfg(unix)]
    path: PathBuf,
    held: Arc<AtomicBool>,
    file: Arc<std::sync::Mutex<Option<std::fs::File>>>,
}

struct LockAcquireGuard {
    held: Arc<AtomicBool>,
    committed: bool,
}
#[cfg(unix)]
impl LockAcquireGuard {
    fn commit(mut self) {
        self.committed = true;
    }
}
impl Drop for LockAcquireGuard {
    fn drop(&mut self) {
        if !self.committed {
            self.held.store(false, Ordering::Release);
        }
    }
}
impl FileSessionLaunchLock {
    #[must_use]
    pub fn new(path: PathBuf) -> Arc<Self> {
        #[cfg(not(unix))]
        let _ = path;
        Arc::new(Self {
            #[cfg(unix)]
            path,
            held: Arc::new(AtomicBool::new(false)),
            file: Arc::new(std::sync::Mutex::new(None)),
        })
    }
}
impl SessionLaunchLock for FileSessionLaunchLock {
    fn acquire(&self) -> ProcessFuture<Result<(), String>> {
        #[cfg(unix)]
        let path = self.path.clone();
        #[cfg(unix)]
        let file_slot = self.file.clone();
        let held = self.held.clone();
        if held
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Box::pin(async { Err("session launch lock is already held".into()) });
        }
        Box::pin(async move {
            let _acquire_guard = LockAcquireGuard {
                held,
                committed: false,
            };
            #[cfg(not(unix))]
            {
                Err("session launch locking is unsupported on this platform".into())
            }
            #[cfg(unix)]
            {
                use rustix::fs::{FlockOperation, flock};
                use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
                if let Some(parent) = path.parent() {
                    let mut builder = std::fs::DirBuilder::new();
                    builder
                        .recursive(true)
                        .mode(0o700)
                        .create(parent)
                        .map_err(|_| "could not create session lock directory".to_owned())?;
                }
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .mode(0o600)
                    .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
                    .open(&path)
                    .map_err(|_| "could not open session launch lock".to_owned())?;
                loop {
                    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
                        Ok(()) => {
                            *file_slot
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(file);
                            _acquire_guard.commit();
                            return Ok(());
                        }
                        Err(rustix::io::Errno::WOULDBLOCK) => {
                            tokio::time::sleep(Duration::from_millis(10)).await
                        }
                        Err(_) => return Err("could not acquire session launch lock".into()),
                    }
                }
            }
        })
    }
    fn release(&self) {
        // Closing the file releases the kernel lock. Retain its inode so a
        // waiting host cannot acquire a different lock file after an unlink.
        self.file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.held.store(false, Ordering::Release);
    }
}
