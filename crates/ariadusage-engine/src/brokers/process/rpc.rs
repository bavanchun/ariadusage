// Ported from CodexBar Sources/CodexBarCore/UsageFetcher.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Host/Process/RPCRequestTimeout.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Host/Process/RPCChildProcessTeardown.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::fmt;
use std::time::Duration;

use serde_json::value::RawValue;
use thiserror::Error;

use super::ProcessError;

#[cfg(target_os = "linux")]
const MAX_RPC_LINE: usize = 1024 * 1024;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RpcError {
    #[error("RPC request was cancelled")]
    Cancelled,
    #[error("RPC request timed out")]
    TimedOut,
    #[error("RPC line exceeded the configured limit")]
    LineTooLong,
    #[error("RPC request failed")]
    RequestFailed,
    #[error("RPC response was malformed")]
    Malformed,
    #[error("RPC session is closed")]
    Closed,
    #[error("process operation failed: {0}")]
    Process(ProcessError),
}

impl From<ProcessError> for RpcError {
    fn from(error: ProcessError) -> Self {
        Self::Process(error)
    }
}

/// A stdio JSON-RPC session with id-based response multiplexing.
pub struct RpcSession {
    #[cfg(target_os = "linux")]
    inner: std::sync::Arc<linux::RpcInner>,
}

impl RpcSession {
    pub async fn spawn(
        call: crate::brokers::call::BrokerCall,
        command: super::Command,
        registry: &super::ProcessRegistry,
        gate: &ariadusage_core::gates::launch::LaunchGate,
    ) -> Result<Self, RpcError> {
        #[cfg(target_os = "linux")]
        {
            Ok(Self {
                inner: linux::spawn(call, command, registry, gate).await?,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (call, command, registry, gate);
            Err(RpcError::Process(ProcessError::Unsupported))
        }
    }

    pub async fn request(
        &self,
        method: &str,
        params: &RawValue,
        timeout: Duration,
    ) -> Result<Box<RawValue>, RpcError> {
        #[cfg(target_os = "linux")]
        {
            self.inner.request(method, params, timeout).await
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (method, params, timeout);
            Err(RpcError::Process(ProcessError::Unsupported))
        }
    }

    pub async fn shutdown(&self) {
        #[cfg(target_os = "linux")]
        self.inner.shutdown().await;
    }
}

impl fmt::Debug for RpcSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RpcSession")
            .field("state", &"redacted")
            .finish()
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::future::{Future, pending};
    use std::io;
    use std::os::fd::AsRawFd;
    use std::pin::Pin;
    use std::process::Stdio;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, Weak};
    use std::time::Duration;

    use bytes::BytesMut;
    use process_wrap::tokio::{
        ChildWrapper, CommandWrap, ProcessGroup, ProcessSession, ResetSigmask,
    };
    use serde::Deserialize;
    use serde::Serialize;
    use serde::de::Deserializer;
    use serde_json::value::RawValue;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::{Mutex as AsyncMutex, oneshot};
    use tokio::task::JoinHandle;
    use tokio::time::{self, Instant};
    use tokio_util::codec::{Decoder, LinesCodec};
    use zeroize::{Zeroize, Zeroizing};

    use ariadusage_core::gates::launch::{LaunchFailureKind, LaunchGate};

    use crate::brokers::call::BrokerCall;

    use super::{MAX_RPC_LINE, ProcessError, RpcError};
    use crate::brokers::process::command::{Command, LaunchMode};
    use crate::brokers::process::procscan::PROCESS_MARKER_ENV;
    use crate::brokers::process::reaper::ReapGuard;
    use crate::brokers::process::registry::{LaunchPermit, ProcessRegistry};
    use crate::brokers::process::session_target;
    use crate::brokers::process::teardown::{ProcessTarget, terminate, terminate_sync};
    use crate::brokers::process::{
        holders::reap_output_holders, holders::reap_output_holders_sync,
    };

    static NEXT_MARKER: AtomicU64 = AtomicU64::new(1);

    #[derive(Serialize)]
    struct RequestBody<'a> {
        id: u64,
        method: &'a str,
        params: &'a RawValue,
    }

    #[derive(Deserialize)]
    struct Response {
        id: Option<u64>,
        #[serde(default, deserialize_with = "present")]
        method: bool,
        #[serde(default, deserialize_with = "raw_value")]
        result: Option<Box<RawValue>>,
        #[serde(default, deserialize_with = "present")]
        error: bool,
    }

    fn present<'de, D>(deserializer: D) -> Result<bool, D::Error>
    where
        D: Deserializer<'de>,
    {
        let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
        Ok(true)
    }

    fn raw_value<'de, D>(deserializer: D) -> Result<Option<Box<RawValue>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Box::<RawValue>::deserialize(deserializer).map(Some)
    }

    type PendingResult = Result<Box<RawValue>, RpcError>;
    type PendingMap = HashMap<u64, oneshot::Sender<PendingResult>>;

    pub(super) struct RpcInner {
        writer: AsyncMutex<Option<tokio::process::ChildStdin>>,
        child: AsyncMutex<Option<Box<dyn ChildWrapper>>>,
        reader: AsyncMutex<Option<JoinHandle<()>>>,
        stderr: AsyncMutex<Option<JoinHandle<()>>>,
        tracker: AsyncMutex<Option<JoinHandle<()>>>,
        guard: Mutex<Option<SessionGuard>>,
        permit: Mutex<Option<LaunchPermit>>,
        pending: Mutex<PendingMap>,
        next_id: AtomicU64,
        stderr_bytes: AtomicU64,
        cancel: tokio_util::sync::CancellationToken,
        tracker_stop: tokio_util::sync::CancellationToken,
        shutdown_lock: AsyncMutex<()>,
        closed: AtomicBool,
    }

    struct SessionGuard {
        target: ProcessTarget,
        reaper: Option<ReapGuard>,
        active: bool,
    }

    impl SessionGuard {
        async fn terminate(&mut self) {
            if !self.active {
                return;
            }
            terminate(&self.target).await;
            reap_output_holders(&self.target).await;
            if let Some(mut reaper) = self.reaper.take() {
                reaper.finish().await;
            }
            self.active = false;
        }
    }

    impl Drop for SessionGuard {
        fn drop(&mut self) {
            if self.active {
                terminate_sync(&self.target);
                reap_output_holders_sync(&self.target);
                self.active = false;
            }
        }
    }

    pub(super) async fn spawn(
        call: BrokerCall,
        command: Command,
        registry: &ProcessRegistry,
        gate: &LaunchGate,
    ) -> Result<Arc<RpcInner>, RpcError> {
        let binary = command.program.as_path().to_string_lossy().into_owned();
        if let Err(until) = gate.check(&binary, call.interaction) {
            return Err(ProcessError::LaunchSuppressed { until }.into());
        }
        let mut permit = registry.register()?;
        let marker = command.reap_marker.then(|| {
            let sequence = NEXT_MARKER.fetch_add(1, Ordering::Relaxed);
            format!("{}-{sequence:016x}", std::process::id())
        });
        let env = marker.as_ref().map_or_else(
            || command.env.clone(),
            |marker| {
                command
                    .env
                    .clone()
                    .with_internal(PROCESS_MARKER_ENV, marker.clone())
            },
        );
        let mut wrapped = CommandWrap::with_new(command.program.as_path(), |child| {
            child
                .args(&command.args)
                .env_clear()
                .envs(env.iter())
                .kill_on_drop(true)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if let Some(cwd) = &command.cwd {
                child.current_dir(cwd);
            }
        });
        wrapped.wrap(ResetSigmask);
        match command.launch {
            LaunchMode::Group => {
                wrapped.wrap(ProcessGroup::leader());
            }
            LaunchMode::Session => {
                wrapped.wrap(ProcessSession);
            }
        };
        let mut child = match wrapped.spawn() {
            Ok(child) => child,
            Err(_) => {
                let _ = gate.record_failure(&binary, LaunchFailureKind::Process);
                return Err(ProcessError::LaunchFailed.into());
            }
        };
        let Some(pid) = child.inner().id() else {
            let _ = child.inner_mut().start_kill();
            let _ = child.inner_mut().wait().await;
            let _ = gate.record_failure(&binary, LaunchFailureKind::Process);
            return Err(ProcessError::LaunchFailed.into());
        };
        let (stdin, stdout, stderr) = {
            let inner = child.inner_mut();
            (
                inner.stdin().take(),
                inner.stdout().take(),
                inner.stderr().take(),
            )
        };
        let (Some(stdin), Some(stdout), Some(stderr)) = (stdin, stdout, stderr) else {
            let _ = child.inner_mut().start_kill();
            let _ = child.inner_mut().wait().await;
            let _ = gate.record_failure(&binary, LaunchFailureKind::Process);
            return Err(ProcessError::LaunchFailed.into());
        };
        let target = match session_target(pid, &[stdout.as_raw_fd(), stderr.as_raw_fd()]) {
            Ok(target) => target,
            Err(error) => {
                let _ = child.inner_mut().start_kill();
                let _ = child.inner_mut().wait().await;
                let _ = gate.record_failure(&binary, LaunchFailureKind::Process);
                return Err(error.into());
            }
        };
        permit.attach(target.clone());
        let tracker_target = target.clone();
        let tracker_stop = tokio_util::sync::CancellationToken::new();
        let inner = Arc::new(RpcInner {
            writer: AsyncMutex::new(Some(stdin)),
            child: AsyncMutex::new(Some(child)),
            reader: AsyncMutex::new(None),
            stderr: AsyncMutex::new(None),
            tracker: AsyncMutex::new(None),
            guard: Mutex::new(Some(SessionGuard {
                target,
                reaper: Some(ReapGuard::new(marker, pid as i32)),
                active: true,
            })),
            permit: Mutex::new(Some(permit)),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            stderr_bytes: AtomicU64::new(0),
            cancel: call.cancel,
            tracker_stop: tracker_stop.clone(),
            shutdown_lock: AsyncMutex::new(()),
            closed: AtomicBool::new(false),
        });
        let reader_inner = Arc::downgrade(&inner);
        let reader = tokio::spawn(read_responses(stdout, reader_inner));
        *inner.reader.lock().await = Some(reader);
        let stderr_inner = Arc::downgrade(&inner);
        let stderr_task = tokio::spawn(drain_stderr(stderr, stderr_inner));
        *inner.stderr.lock().await = Some(stderr_task);
        let tracker = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tracker_stop.cancelled() => break,
                    _ = time::sleep(Duration::from_millis(20)) => tracker_target.refresh_descendants(),
                }
            }
        });
        *inner.tracker.lock().await = Some(tracker);
        Ok(inner)
    }

    impl RpcInner {
        pub(super) async fn request(
            self: &Arc<Self>,
            method: &str,
            params: &RawValue,
            timeout: Duration,
        ) -> Result<Box<RawValue>, RpcError> {
            if self.closed.load(Ordering::Acquire) {
                return Err(RpcError::Closed);
            }
            if self.cancel.is_cancelled() {
                return Err(RpcError::Cancelled);
            }
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let (sender, receiver) = oneshot::channel();
            lock(&self.pending).insert(id, sender);
            let mut bytes = Zeroizing::new(Vec::new());
            if serde_json::to_writer(&mut *bytes, &RequestBody { id, method, params }).is_err() {
                lock(&self.pending).remove(&id);
                return Err(RpcError::Malformed);
            }
            bytes.push(b'\n');
            let write_result = {
                let mut writer = self.writer.lock().await;
                match writer.as_mut() {
                    Some(writer) => writer.write_all(&bytes).await,
                    None => Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed")),
                }
            };
            bytes.zeroize();
            if write_result.is_err() {
                lock(&self.pending).remove(&id);
                let inner = Arc::clone(self);
                tokio::spawn(async move { inner.shutdown().await });
                return Err(RpcError::Closed);
            }
            let mut timeout = timeout_future(timeout);
            tokio::select! {
                biased;
                _ = &mut timeout => {
                    lock(&self.pending).remove(&id);
                    self.shutdown().await;
                    Err(RpcError::TimedOut)
                },
                _ = self.cancel.cancelled() => {
                    lock(&self.pending).remove(&id);
                    Err(RpcError::Cancelled)
                },
                result = receiver => {
                    let result = result.unwrap_or(Err(RpcError::Closed));
                    if matches!(&result, Err(RpcError::LineTooLong)) {
                        self.shutdown().await;
                    }
                    result
                },
            }
        }

        pub(super) async fn shutdown(self: &Arc<Self>) {
            let _lock = self.shutdown_lock.lock().await;
            if self.closed.swap(true, Ordering::AcqRel) {
                return;
            }
            self.fail_pending(RpcError::Closed);
            self.tracker_stop.cancel();
            drop(self.writer.lock().await.take());
            let mut guard = lock(&self.guard).take();
            if let Some(guard) = guard.as_mut() {
                guard.terminate().await;
            }
            if let Some(child) = self.child.lock().await.as_mut() {
                let _ = time::timeout(Duration::from_secs(1), child.inner_mut().wait()).await;
            }
            if let Some(reader) = self.reader.lock().await.take() {
                reader.abort();
            }
            if let Some(stderr) = self.stderr.lock().await.take() {
                stderr.abort();
            }
            if let Some(tracker) = self.tracker.lock().await.take() {
                tracker.abort();
            }
            drop(guard);
            lock(&self.permit).take();
        }

        fn fail_pending(&self, error: RpcError) {
            let pending = std::mem::take(&mut *lock(&self.pending));
            for (_, sender) in pending {
                let _ = sender.send(Err(error.clone()));
            }
        }
    }

    impl Drop for RpcInner {
        fn drop(&mut self) {
            self.tracker_stop.cancel();
        }
    }

    async fn read_responses<R>(mut stdout: R, inner: Weak<RpcInner>)
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let mut codec = LinesCodec::new_with_max_length(MAX_RPC_LINE);
        let mut framed = BytesMut::with_capacity(8 * 1024);
        let mut chunk = [0_u8; 8 * 1024];
        loop {
            loop {
                match codec.decode(&mut framed) {
                    Ok(Some(line)) => {
                        if let Some(inner) = inner.upgrade() {
                            process_line(&inner, Zeroizing::new(line));
                        }
                    }
                    Ok(None) => break,
                    Err(_) => {
                        if let Some(inner) = inner.upgrade() {
                            inner.fail_pending(RpcError::LineTooLong);
                            tokio::spawn(async move { inner.shutdown().await });
                        }
                        return;
                    }
                }
            }
            match stdout.read(&mut chunk).await {
                Ok(0) => {
                    if let Ok(Some(line)) = codec.decode_eof(&mut framed)
                        && let Some(inner) = inner.upgrade()
                    {
                        process_line(&inner, Zeroizing::new(line));
                    }
                    if let Some(inner) = inner.upgrade() {
                        inner.fail_pending(RpcError::Closed);
                        tokio::spawn(async move { inner.shutdown().await });
                    }
                    return;
                }
                Ok(count) => {
                    framed.extend_from_slice(&chunk[..count]);
                    chunk[..count].zeroize();
                }
                Err(_) => {
                    if let Some(inner) = inner.upgrade() {
                        inner.fail_pending(RpcError::Closed);
                        tokio::spawn(async move { inner.shutdown().await });
                    }
                    return;
                }
            }
        }
    }

    fn process_line(inner: &RpcInner, line: Zeroizing<String>) {
        let Ok(message) = serde_json::from_str::<Response>(&line) else {
            return;
        };
        let Some(id) = message.id else {
            return;
        };
        let Some(sender) = lock(&inner.pending).remove(&id) else {
            return;
        };
        let result = if message.error {
            Err(RpcError::RequestFailed)
        } else if message.method {
            Err(RpcError::Malformed)
        } else if let Some(result) = message.result {
            Ok(result)
        } else {
            Err(RpcError::Malformed)
        };
        let _ = sender.send(result);
    }

    async fn drain_stderr<R>(mut stderr: R, inner: Weak<RpcInner>)
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let mut bytes = [0_u8; 8 * 1024];
        loop {
            match stderr.read(&mut bytes).await {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if let Some(inner) = inner.upgrade() {
                        inner
                            .stderr_bytes
                            .fetch_add(count as u64, Ordering::Relaxed);
                    }
                    bytes[..count].zeroize();
                }
            }
        }
        bytes.zeroize();
    }

    fn timeout_future(duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        match Instant::now().checked_add(duration) {
            Some(deadline) => Box::pin(time::sleep_until(deadline)),
            None => Box::pin(pending()),
        }
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
