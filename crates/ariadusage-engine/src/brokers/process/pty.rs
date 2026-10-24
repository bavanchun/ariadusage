// Ported from CodexBar Sources/CodexBarCore/Host/PTY/TTYCommandRunner.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::fmt;
use std::time::Duration;

use thiserror::Error;
use zeroize::Zeroizing;

use super::{Command, ProcessError};

const DEFAULT_ROWS: u16 = 50;
const DEFAULT_COLS: u16 = 160;
#[cfg(unix)]
const OUTPUT_CAP: usize = 1024 * 1024;
#[cfg(unix)]
const RECENT_TEXT_CAP: usize = 8 * 1024;
#[cfg(unix)]
const READ_POLL: Duration = Duration::from_millis(60);
#[cfg(unix)]
const EXIT_DRAIN: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyCompletionReason {
    ProcessExited,
    IdleTimeout,
    OutputCondition,
    DeadlineExceeded,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PtyError {
    #[error("PTY operation was cancelled")]
    Cancelled,
    #[error("PTY operation timed out")]
    TimedOut,
    #[error("PTY output exceeded its configured limit")]
    OutputTooLarge,
    #[error("PTY session is closed")]
    Closed,
    #[error("PTY operation failed")]
    Io,
    #[error("PTY could not be opened or configured")]
    Infrastructure,
    #[error("PTY could not be opened (errno {errno}: {description})")]
    OpenFailed { errno: i32, description: String },
    #[error("PTY operation is unsupported on this platform")]
    Unsupported,
    #[error("process operation failed: {0}")]
    Process(ProcessError),
}

impl From<ProcessError> for PtyError {
    fn from(error: ProcessError) -> Self {
        Self::Process(error)
    }
}

impl PtyError {
    /// A PTY allocation failure carrying the captured POSIX error and its description.
    pub fn open_failed(errno: i32) -> Self {
        let text = std::io::Error::from_raw_os_error(errno).to_string();
        let description = text
            .strip_suffix(&format!(" (os error {errno})"))
            .map_or_else(|| text.clone(), str::to_owned);
        Self::OpenFailed { errno, description }
    }
}

/// Makes the next PTY allocation in this process fail with `errno`, so tests can
/// observe how allocation failures are reported.
#[cfg(all(unix, feature = "test-hooks"))]
pub fn fail_next_open_for_test(errno: i32) {
    unix::FAIL_NEXT_OPEN.store(errno, std::sync::atomic::Ordering::SeqCst);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
}

impl PtySize {
    pub const fn new(rows: u16, cols: u16) -> Self {
        Self { rows, cols }
    }
}

impl Default for PtySize {
    fn default() -> Self {
        Self::new(DEFAULT_ROWS, DEFAULT_COLS)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubstringSource {
    RawBytes,
    RecentText,
}

pub struct SendOnSubstring {
    pub needle: Vec<u8>,
    pub send: Zeroizing<Vec<u8>>,
    pub source: SubstringSource,
}

impl SendOnSubstring {
    pub fn new(
        needle: impl Into<Vec<u8>>,
        send: impl Into<Vec<u8>>,
        source: SubstringSource,
    ) -> Self {
        Self {
            needle: needle.into(),
            send: Zeroizing::new(send.into()),
            source,
        }
    }
}

impl fmt::Debug for SendOnSubstring {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SendOnSubstring")
            .field("needle_len", &self.needle.len())
            .field("send_len", &self.send.len())
            .field("source", &self.source)
            .finish()
    }
}

pub struct PtyScript {
    pub input: Zeroizing<Vec<u8>>,
    pub send_on_substrings: Vec<SendOnSubstring>,
    pub stop_needles: Vec<Vec<u8>>,
    pub stop_on_url: bool,
    pub idle_timeout: Option<Duration>,
    pub send_enter_every: Option<Duration>,
    pub settle: Duration,
    pub deadline: Duration,
}

impl PtyScript {
    pub fn new(input: impl Into<Vec<u8>>, deadline: Duration) -> Self {
        Self {
            input: Zeroizing::new(input.into()),
            send_on_substrings: Vec::new(),
            stop_needles: Vec::new(),
            stop_on_url: false,
            idle_timeout: None,
            send_enter_every: None,
            settle: Duration::from_millis(250),
            deadline,
        }
    }
}

impl fmt::Debug for PtyScript {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PtyScript")
            .field("input_len", &self.input.len())
            .field("send_count", &self.send_on_substrings.len())
            .field("stop_count", &self.stop_needles.len())
            .field("stop_on_url", &self.stop_on_url)
            .field("idle_timeout", &self.idle_timeout)
            .field("send_enter_every", &self.send_enter_every)
            .field("settle", &self.settle)
            .field("deadline", &self.deadline)
            .finish()
    }
}

pub struct PtyTranscript {
    pub bytes: Zeroizing<Vec<u8>>,
    pub text: Zeroizing<String>,
    pub reason: PtyCompletionReason,
}

impl fmt::Debug for PtyTranscript {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PtyTranscript")
            .field("byte_len", &self.bytes.len())
            .field("text_len", &self.text.len())
            .field("reason", &self.reason)
            .finish()
    }
}

pub struct PtySession {
    #[cfg(unix)]
    inner: unix::PtyInner,
}

impl PtySession {
    pub async fn spawn(
        call: crate::brokers::call::BrokerCall,
        command: Command,
        size: PtySize,
        registry: &super::ProcessRegistry,
        gate: &ariadusage_core::gates::launch::LaunchGate,
    ) -> Result<Self, PtyError> {
        #[cfg(unix)]
        {
            Ok(Self {
                inner: unix::spawn(call, command, size, registry, gate).await?,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (call, command, size, registry, gate);
            Err(PtyError::Unsupported)
        }
    }

    pub async fn run_script(&mut self, script: PtyScript) -> Result<PtyTranscript, PtyError> {
        #[cfg(unix)]
        {
            self.inner.run_script(script).await
        }
        #[cfg(not(unix))]
        {
            let _ = script;
            Err(PtyError::Unsupported)
        }
    }

    pub async fn write(&mut self, bytes: &[u8]) -> Result<(), PtyError> {
        #[cfg(unix)]
        {
            self.inner.write(bytes).await
        }
        #[cfg(not(unix))]
        {
            let _ = bytes;
            Err(PtyError::Unsupported)
        }
    }

    pub async fn close(&mut self) {
        #[cfg(unix)]
        self.inner.close().await;
    }

    /// Returns the root child PID while the PTY process is still alive.
    pub fn pid(&self) -> Option<u32> {
        #[cfg(unix)]
        {
            self.inner.pid()
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// Whether the broker-owned child has exited.
    pub fn is_running(&self) -> bool {
        #[cfg(unix)]
        {
            !self.inner.has_exited()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    /// Returns the executable path and stable process start identity.
    #[cfg(unix)]
    pub fn process_identity(&self) -> Option<(std::path::PathBuf, u64)> {
        self.inner.process_identity()
    }

    #[cfg(not(unix))]
    pub fn process_identity(&self) -> Option<(std::path::PathBuf, u64)> {
        None
    }

    /// Returns the broker ownership marker assigned to this PTY child.
    pub fn marker(&self) -> Option<String> {
        #[cfg(unix)]
        {
            self.inner.marker()
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    #[cfg(unix)]
    pub fn process_group(&self) -> Option<u32> {
        self.inner.process_group()
    }

    #[cfg(not(unix))]
    pub fn process_group(&self) -> Option<u32> {
        None
    }

    /// Closes a warm PTY without sending interactive input.
    pub async fn terminate(&mut self) {
        #[cfg(unix)]
        self.inner.terminate_without_input().await;
    }

    /// Drains currently available PTY bytes, bounded to 64 × 8 KiB per call.
    pub async fn drain_output(&mut self) -> Result<Vec<u8>, PtyError> {
        #[cfg(unix)]
        {
            return self.inner.drain_output().await;
        }
        #[cfg(not(unix))]
        {
            Err(PtyError::Unsupported)
        }
    }

    pub(crate) async fn read_once(
        &mut self,
        buffer: &mut [u8],
        timeout: Duration,
    ) -> Result<super::DrainClassification, PtyError> {
        #[cfg(unix)]
        {
            self.inner.read_once(buffer, timeout).await
        }
        #[cfg(not(unix))]
        {
            let _ = (buffer, timeout);
            Err(PtyError::Unsupported)
        }
    }

    pub(crate) fn has_exited(&self) -> bool {
        #[cfg(unix)]
        {
            self.inner.has_exited()
        }
        #[cfg(not(unix))]
        {
            true
        }
    }

    /// Waits up to `timeout` for the session's exit watcher to observe that the child exited.
    #[cfg(all(unix, feature = "test-hooks"))]
    pub async fn wait_for_exit_for_test(&mut self, timeout: Duration) -> bool {
        self.inner.wait_for_exit(timeout).await
    }
}

impl fmt::Debug for PtySession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PtySession")
            .field("state", &"redacted")
            .finish()
    }
}

#[cfg(all(unix, feature = "test-hooks"))]
pub async fn drain_bounded_for_test(source: Vec<u8>) -> Result<(Vec<u8>, usize), PtyError> {
    unix::drain_bounded_for_test(source).await
}

#[cfg(unix)]
mod unix {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    #[cfg(not(target_os = "macos"))]
    use pty_process::Command as PtyCommand;
    use pty_process::{Size, open};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::watch;
    use tokio::time::{self, Instant};
    use zeroize::{Zeroize, Zeroizing};

    use ariadusage_core::gates::launch::LaunchGate;

    use crate::brokers::call::BrokerCall;

    use super::{
        EXIT_DRAIN, OUTPUT_CAP, PtyCompletionReason, PtyError, PtyScript, PtySize, PtyTranscript,
        READ_POLL, RECENT_TEXT_CAP, SendOnSubstring, SubstringSource,
    };
    use crate::brokers::process::buffers::BoundedOutputBuffer;
    use crate::brokers::process::command::Command;
    use crate::brokers::process::procscan::PROCESS_MARKER_ENV;
    use crate::brokers::process::reaper::ReapGuard;
    use crate::brokers::process::registry::ProcessRegistry;
    use crate::brokers::process::teardown::{ProcessTarget, terminate, terminate_sync};
    use crate::brokers::process::{DrainClassification, drain, scan, session_target};
    use crate::brokers::process::{
        holders::reap_output_holders, holders::reap_output_holders_sync,
    };

    static NEXT_MARKER: AtomicU64 = AtomicU64::new(1);
    #[cfg(feature = "test-hooks")]
    pub(super) static FAIL_NEXT_OPEN: std::sync::atomic::AtomicI32 =
        std::sync::atomic::AtomicI32::new(0);

    fn open_pty() -> pty_process::Result<(pty_process::Pty, pty_process::Pts)> {
        #[cfg(feature = "test-hooks")]
        {
            let errno = FAIL_NEXT_OPEN.swap(0, Ordering::SeqCst);
            if errno != 0 {
                return Err(pty_process::Error::Io(std::io::Error::from_raw_os_error(
                    errno,
                )));
            }
        }
        open()
    }

    /// Reports the POSIX error behind a PTY allocation failure when there is one.
    fn open_failed(error: pty_process::Error) -> PtyError {
        let errno = match &error {
            pty_process::Error::Io(error) => error.raw_os_error(),
            pty_process::Error::Rustix(errno) => Some(errno.raw_os_error()),
            _ => None,
        };
        errno.map_or(PtyError::Infrastructure, PtyError::open_failed)
    }

    trait DrainReader {
        fn read_chunk<'a>(
            &'a mut self,
            chunk: &'a mut [u8],
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<DrainClassification, PtyError>> + Send + 'a,
            >,
        >;
    }

    async fn drain_bounded<R: DrainReader + ?Sized>(reader: &mut R) -> Result<Vec<u8>, PtyError> {
        let mut output = Vec::new();
        let mut chunk = [0_u8; 8192];
        for _ in 0..64 {
            match reader.read_chunk(&mut chunk).await? {
                DrainClassification::Data(count) => output.extend_from_slice(&chunk[..count]),
                DrainClassification::WouldBlock | DrainClassification::Closed => break,
            }
        }
        Ok(output)
    }

    pub(super) struct PtyInner {
        master: Option<pty_process::Pty>,
        marker: Option<String>,
        call: BrokerCall,
        guard: Option<PtyGuard>,
        exit_rx: watch::Receiver<bool>,
        ran_script: bool,
    }

    struct PtyGuard {
        target: ProcessTarget,
        reaper: Option<ReapGuard>,
        active: bool,
    }

    impl PtyGuard {
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

    impl Drop for PtyGuard {
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
        size: PtySize,
        registry: &ProcessRegistry,
        gate: &LaunchGate,
    ) -> Result<PtyInner, PtyError> {
        let binary_path = command.program.as_path().to_path_buf();
        let binary = binary_path.to_string_lossy().into_owned();
        if let Err(until) = gate.check(&binary, call.interaction) {
            return Err(super::PtyError::Process(
                crate::brokers::process::ProcessError::LaunchSuppressed { until },
            ));
        }
        let mut permit = registry.register()?;
        let (master, slave) = open_pty().map_err(open_failed)?;
        master
            .resize(Size::new(size.rows, size.cols))
            .map_err(open_failed)?;
        #[cfg(target_os = "macos")]
        let slave_hold = {
            use std::os::fd::AsFd;
            rustix::io::fcntl_dupfd_cloexec(slave.as_fd(), 3)
                .map_err(|_| PtyError::Infrastructure)?
        };
        let marker = command.reap_marker.then(|| {
            let sequence = NEXT_MARKER.fetch_add(1, Ordering::Relaxed);
            format!("{}-{sequence:016x}", std::process::id())
        });
        let mut env = command.env.clone();
        if let Some(home) = command.env.get("HOME").cloned() {
            env = env.with("HOME", home);
        }
        if env.get("TERM").is_none_or(|value| value.is_empty()) {
            env = env.with("TERM", "xterm-256color");
        }
        if env.get("COLORTERM").is_none_or(|value| value.is_empty()) {
            env = env.with("COLORTERM", "truecolor");
        }
        if env.get("LANG").is_none_or(|value| value.is_empty()) {
            env = env.with("LANG", "en_US.UTF-8");
        }
        if env.get("CI").is_none() {
            env = env.with("CI", "0");
        }
        if let Some(cwd) = &command.cwd {
            env = env.with("PWD", cwd.as_os_str());
        }
        if let Some(marker) = &marker {
            env = env.with_internal(PROCESS_MARKER_ENV, marker.clone());
        }
        #[cfg(not(target_os = "macos"))]
        let mut child = {
            let mut pty_command = PtyCommand::new(command.program.as_path()).kill_on_drop(true);
            pty_command = pty_command.args(&command.args).env_clear().envs(env.iter());
            if let Some(cwd) = &command.cwd {
                pty_command = pty_command.current_dir(cwd);
            }
            pty_command
                .spawn(slave)
                .map_err(|_| PtyError::Infrastructure)?
        };
        #[cfg(target_os = "macos")]
        let mut child = {
            use std::os::fd::AsFd;
            use std::process::Stdio;
            let stdin_fd = slave
                .as_fd()
                .try_clone_to_owned()
                .map_err(|_| PtyError::Infrastructure)?;
            let stdout_fd = slave
                .as_fd()
                .try_clone_to_owned()
                .map_err(|_| PtyError::Infrastructure)?;
            let stderr_fd = slave
                .as_fd()
                .try_clone_to_owned()
                .map_err(|_| PtyError::Infrastructure)?;
            drop(slave);
            let mut pty_command = tokio::process::Command::new(command.program.as_path());
            pty_command
                .kill_on_drop(true)
                .process_group(0)
                .args(&command.args)
                .env_clear()
                .envs(env.iter())
                .stdin(Stdio::from(stdin_fd))
                .stdout(Stdio::from(stdout_fd))
                .stderr(Stdio::from(stderr_fd));
            if let Some(cwd) = &command.cwd {
                pty_command.current_dir(cwd);
            }
            pty_command.spawn().map_err(|_| PtyError::Infrastructure)?
        };
        let Some(pid) = child.id() else {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(PtyError::Infrastructure);
        };
        let target = match session_target(pid, &[]) {
            Ok(target) => target,
            Err(error) => {
                #[cfg(target_os = "macos")]
                drop(slave_hold);
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(error.into());
            }
        };
        #[cfg(target_os = "macos")]
        target.set_tty_hold(slave_hold);
        permit.attach(target.clone());
        let (exit_tx, exit_rx) = watch::channel(false);
        tokio::spawn(async move {
            let _ = child.wait().await;
            drop(permit);
            exit_tx.send_replace(true);
        });
        Ok(PtyInner {
            master: Some(master),
            marker: marker.clone(),
            call,
            guard: Some(PtyGuard {
                target,
                reaper: Some(ReapGuard::new(marker, pid as i32)),
                active: true,
            }),
            exit_rx,
            ran_script: false,
        })
    }

    impl DrainReader for PtyInner {
        fn read_chunk<'a>(
            &'a mut self,
            chunk: &'a mut [u8],
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<DrainClassification, PtyError>> + Send + 'a,
            >,
        > {
            Box::pin(self.read_once(chunk, Duration::from_millis(1)))
        }
    }

    impl PtyInner {
        pub async fn write(&mut self, bytes: &[u8]) -> Result<(), PtyError> {
            let Some(master) = self.master.as_mut() else {
                return Err(PtyError::Closed);
            };
            master.write_all(bytes).await.map_err(|_| PtyError::Io)?;
            master.flush().await.map_err(|_| PtyError::Io)
        }

        pub async fn run_script(
            &mut self,
            mut script: PtyScript,
        ) -> Result<PtyTranscript, PtyError> {
            if self.ran_script || self.master.is_none() {
                return Err(PtyError::Closed);
            }
            self.ran_script = true;
            if !script.input.is_empty() {
                let mut input = Zeroizing::new(std::mem::take(&mut *script.input));
                input.push(b'\r');
                if self.write(&input).await.is_err() {
                    self.close().await;
                    return Err(PtyError::Io);
                }
                input.zeroize();
            }

            let max_needle = script
                .stop_needles
                .iter()
                .map(Vec::len)
                .chain(
                    script
                        .send_on_substrings
                        .iter()
                        .map(|item| item.needle.len()),
                )
                .chain([b"http://".len(), b"https://".len(), b"\x1b[6n".len()])
                .max()
                .unwrap_or(1);
            let mut scanner = scan::StreamScanBuffer::new(max_needle);
            let mut output = BoundedOutputBuffer::new(OUTPUT_CAP);
            let mut recent_text = Zeroizing::new(Vec::new());
            let mut triggered = vec![false; script.send_on_substrings.len()];
            let start = Instant::now();
            let deadline = start.checked_add(script.deadline).unwrap_or(start);
            let mut last_output = None;
            let mut last_enter = start;
            let mut next_cursor = start;
            let mut url_seen = false;
            let mut closed = false;
            let mut early_reason = None;
            let mut chunk = [0_u8; 8 * 1024];

            loop {
                if self.call.cancel.is_cancelled() {
                    self.close().await;
                    return Err(PtyError::Cancelled);
                }
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                if let Some(timeout) = script.idle_timeout
                    && !output.as_slice().is_empty()
                    && last_output.is_some_and(|last| now.duration_since(last) >= timeout)
                {
                    early_reason = Some(PtyCompletionReason::IdleTimeout);
                    break;
                }
                if !url_seen
                    && let Some(every) = script.send_enter_every
                    && now.duration_since(last_enter) >= every
                {
                    let _ = self.write(b"\r").await;
                    last_enter = now;
                }

                let read_wait = deadline.saturating_duration_since(now).min(READ_POLL);
                match self.read_once(&mut chunk, read_wait).await? {
                    DrainClassification::Data(count) => {
                        if !output.append(&chunk[..count]) {
                            chunk[..count].zeroize();
                            self.release_slave_hold();
                            self.master.take();
                            self.shutdown_process().await;
                            return Err(PtyError::OutputTooLarge);
                        }
                        last_output = Some(Instant::now());
                        let scanned = scanner.append(&chunk[..count]);
                        update_recent_text(&mut recent_text, &chunk[..count]);
                        let now = Instant::now();
                        if now >= next_cursor
                            && scanned.windows(4).any(|window| window == b"\x1b[6n")
                        {
                            let _ = self.write(b"\x1b[1;1R").await;
                            next_cursor = now + Duration::from_secs(1);
                        }
                        for (index, item) in script.send_on_substrings.iter().enumerate() {
                            if !triggered[index] && substring_matches(item, &scanned, &recent_text)
                            {
                                let _ = self.write(&item.send).await;
                                triggered[index] = true;
                            }
                        }
                        if !url_seen && contains_url_prefix(&scanned) {
                            url_seen = true;
                            if script.stop_on_url {
                                early_reason = Some(PtyCompletionReason::OutputCondition);
                            }
                        }
                        if early_reason.is_none()
                            && script
                                .stop_needles
                                .iter()
                                .any(|needle| contains(&scanned, needle))
                        {
                            early_reason = Some(PtyCompletionReason::OutputCondition);
                        }
                        chunk[..count].zeroize();
                        if early_reason.is_some() {
                            break;
                        }
                    }
                    DrainClassification::WouldBlock => {}
                    DrainClassification::Closed => {
                        closed = true;
                        break;
                    }
                }
                if self.has_exited() {
                    break;
                }
            }

            if let Some(reason) = early_reason {
                let settle = script
                    .settle
                    .min(deadline.saturating_duration_since(Instant::now()));
                let settle_until = Instant::now()
                    .checked_add(settle)
                    .unwrap_or_else(Instant::now);
                while Instant::now() < settle_until {
                    if self.call.cancel.is_cancelled() {
                        self.close().await;
                        return Err(PtyError::Cancelled);
                    }
                    let wait = settle_until
                        .saturating_duration_since(Instant::now())
                        .min(READ_POLL);
                    match self.read_once(&mut chunk, wait).await? {
                        DrainClassification::Data(count) => {
                            if !output.append(&chunk[..count]) {
                                chunk[..count].zeroize();
                                self.release_slave_hold();
                                self.master.take();
                                self.shutdown_process().await;
                                return Err(PtyError::OutputTooLarge);
                            }
                            let scanned = scanner.append(&chunk[..count]);
                            if Instant::now() >= next_cursor
                                && scanned.windows(4).any(|window| window == b"\x1b[6n")
                            {
                                let _ = self.write(b"\x1b[1;1R").await;
                                next_cursor = Instant::now() + Duration::from_secs(1);
                            }
                            chunk[..count].zeroize();
                        }
                        DrainClassification::WouldBlock => {}
                        DrainClassification::Closed => break,
                    }
                }
                let reason = if self.has_exited() {
                    self.release_slave_hold();
                    PtyCompletionReason::ProcessExited
                } else {
                    reason
                };
                return Ok(transcript(output, reason));
            }

            let exited = self.has_exited();
            if closed && !exited {
                if self.wait_for_exit(EXIT_DRAIN).await {
                    self.release_slave_hold();
                    return Ok(transcript(output, PtyCompletionReason::ProcessExited));
                }
                self.close().await;
                return Err(PtyError::TimedOut);
            }
            if exited {
                #[cfg(target_os = "macos")]
                if self.has_slave_hold() {
                    loop {
                        match self
                            .read_once(&mut chunk, Duration::from_millis(10))
                            .await?
                        {
                            DrainClassification::Data(count) => {
                                if !output.append(&chunk[..count]) {
                                    chunk[..count].zeroize();
                                    self.release_slave_hold();
                                    self.master.take();
                                    self.shutdown_process().await;
                                    return Err(PtyError::OutputTooLarge);
                                }
                                chunk[..count].zeroize();
                            }
                            DrainClassification::WouldBlock => break,
                            DrainClassification::Closed => {
                                closed = true;
                                break;
                            }
                        }
                    }
                    self.release_slave_hold();
                }
                let drain_deadline = Instant::now() + EXIT_DRAIN;
                while !closed && Instant::now() < drain_deadline {
                    let wait = drain_deadline
                        .saturating_duration_since(Instant::now())
                        .min(READ_POLL);
                    match self.read_once(&mut chunk, wait).await? {
                        DrainClassification::Data(count) => {
                            if !output.append(&chunk[..count]) {
                                chunk[..count].zeroize();
                                self.master.take();
                                self.shutdown_process().await;
                                return Err(PtyError::OutputTooLarge);
                            }
                            chunk[..count].zeroize();
                        }
                        DrainClassification::WouldBlock => {}
                        DrainClassification::Closed => closed = true,
                    }
                }
                if !closed {
                    self.close().await;
                    return Err(PtyError::TimedOut);
                }
                return Ok(transcript(output, PtyCompletionReason::ProcessExited));
            }

            let drain_for = script
                .settle
                .clamp(Duration::from_millis(200), Duration::from_millis(500));
            let drain_deadline = Instant::now() + drain_for;
            while !closed && Instant::now() < drain_deadline {
                let wait = drain_deadline
                    .saturating_duration_since(Instant::now())
                    .min(READ_POLL);
                match self.read_once(&mut chunk, wait).await? {
                    DrainClassification::Data(count) => {
                        if !output.append(&chunk[..count]) {
                            chunk[..count].zeroize();
                            self.release_slave_hold();
                            self.master.take();
                            self.shutdown_process().await;
                            return Err(PtyError::OutputTooLarge);
                        }
                        chunk[..count].zeroize();
                    }
                    DrainClassification::WouldBlock => {}
                    DrainClassification::Closed => closed = true,
                }
            }
            let reason = if self.has_exited() {
                self.release_slave_hold();
                PtyCompletionReason::ProcessExited
            } else {
                PtyCompletionReason::DeadlineExceeded
            };
            Ok(transcript(output, reason))
        }

        #[cfg(target_os = "macos")]
        fn has_slave_hold(&self) -> bool {
            self.guard
                .as_ref()
                .is_some_and(|guard| guard.target.has_tty_hold())
        }

        fn release_slave_hold(&self) {
            if let Some(guard) = self.guard.as_ref() {
                guard.target.release_tty_hold();
            }
        }

        pub(super) async fn read_once(
            &mut self,
            buffer: &mut [u8],
            timeout: Duration,
        ) -> Result<DrainClassification, PtyError> {
            let Some(master) = self.master.as_mut() else {
                return Ok(DrainClassification::Closed);
            };
            let result = time::timeout(timeout, master.read(buffer)).await;
            Ok(match result {
                Ok(result) => drain::classify(result),
                Err(_) => DrainClassification::WouldBlock,
            })
        }

        pub(super) fn has_exited(&self) -> bool {
            *self.exit_rx.borrow()
        }

        pub(super) fn pid(&self) -> Option<u32> {
            self.guard
                .as_ref()
                .map(|guard| guard.target.root.pid as u32)
        }

        pub(super) fn process_identity(&self) -> Option<(std::path::PathBuf, u64)> {
            let guard = self.guard.as_ref()?;
            let identity = guard.target.root;
            #[cfg(target_os = "linux")]
            {
                let current = crate::brokers::process::procscan::process_identity(
                    std::path::Path::new("/proc"),
                    identity.pid,
                )?;
                if current != identity {
                    return None;
                }
                let executable = std::fs::read_link(format!("/proc/{}/exe", identity.pid)).ok()?;
                Some((executable, identity.start_ticks))
            }
            #[cfg(not(target_os = "linux"))]
            {
                let executable =
                    crate::brokers::local_probe::LocalProbe::new().executable_path(identity)?;
                Some((executable, identity.start_ticks))
            }
        }

        pub(super) fn process_group(&self) -> Option<u32> {
            self.guard.as_ref().map(|guard| guard.target.pgid as u32)
        }

        pub(super) fn marker(&self) -> Option<String> {
            self.marker.clone()
        }

        pub(super) async fn wait_for_exit(&mut self, timeout: Duration) -> bool {
            if self.has_exited() {
                return true;
            }
            time::timeout(timeout, self.exit_rx.changed()).await.is_ok() && self.has_exited()
        }

        pub async fn close(&mut self) {
            let running = !self.has_exited();
            if running && let Some(master) = self.master.as_mut() {
                let _ = master.write_all(b"/exit\n").await;
                let _ = master.flush().await;
            }
            self.release_slave_hold();
            self.master.take();
            self.shutdown_process().await;
        }

        pub async fn terminate_without_input(&mut self) {
            self.release_slave_hold();
            self.master.take();
            self.shutdown_process().await;
        }

        pub async fn drain_output(&mut self) -> Result<Vec<u8>, PtyError> {
            drain_bounded(self).await
        }

        async fn shutdown_process(&mut self) {
            self.release_slave_hold();
            if let Some(mut guard) = self.guard.take() {
                guard.terminate().await;
            }
            let _ = self.wait_for_exit(Duration::from_secs(1)).await;
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) async fn drain_bounded_for_test(
        source: Vec<u8>,
    ) -> Result<(Vec<u8>, usize), PtyError> {
        struct Fixture {
            source: Vec<u8>,
            offset: usize,
        }

        impl DrainReader for Fixture {
            fn read_chunk<'a>(
                &'a mut self,
                chunk: &'a mut [u8],
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<DrainClassification, PtyError>>
                        + Send
                        + 'a,
                >,
            > {
                let count = (self.source.len() - self.offset).min(chunk.len());
                chunk[..count].copy_from_slice(&self.source[self.offset..self.offset + count]);
                self.offset += count;
                Box::pin(async move {
                    if count == 0 {
                        Ok(DrainClassification::Closed)
                    } else {
                        Ok(DrainClassification::Data(count))
                    }
                })
            }
        }

        let mut reader = Fixture { source, offset: 0 };
        let output = drain_bounded(&mut reader).await?;
        Ok((output, reader.offset))
    }

    impl Drop for PtyInner {
        fn drop(&mut self) {
            self.release_slave_hold();
            self.master.take();
            self.guard.take();
            // The guard performs bounded cleanup; the exit monitor reaps the child and releases
            // its launch permit even when the session is dropped without close().
        }
    }

    fn transcript(output: BoundedOutputBuffer, reason: PtyCompletionReason) -> PtyTranscript {
        let bytes = output.into_bytes();
        let text = strip_ansi(bytes.as_slice());
        PtyTranscript {
            bytes,
            text,
            reason,
        }
    }

    fn update_recent_text(window: &mut Zeroizing<Vec<u8>>, chunk: &[u8]) {
        let Ok(text) = std::str::from_utf8(chunk) else {
            return;
        };
        window.extend_from_slice(text.as_bytes());
        if window.len() > RECENT_TEXT_CAP {
            let excess = window.len() - RECENT_TEXT_CAP;
            window.copy_within(excess.., 0);
            window[RECENT_TEXT_CAP..].zeroize();
            window.truncate(RECENT_TEXT_CAP);
        }
    }

    fn substring_matches(item: &SendOnSubstring, scanned: &[u8], recent: &[u8]) -> bool {
        if item.needle.is_empty() {
            return false;
        }
        match item.source {
            SubstringSource::RawBytes => contains(scanned, &item.needle),
            SubstringSource::RecentText => {
                contains(recent, &item.needle)
                    || contains(
                        &recent
                            .iter()
                            .copied()
                            .filter(|byte| *byte != b'\r')
                            .collect::<Vec<_>>(),
                        &item.needle,
                    )
            }
        }
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        !needle.is_empty()
            && haystack
                .windows(needle.len())
                .any(|window| window == needle)
    }

    fn contains_url_prefix(bytes: &[u8]) -> bool {
        contains(bytes, b"https://") || contains(bytes, b"http://")
    }

    fn strip_ansi(bytes: &[u8]) -> Zeroizing<String> {
        #[derive(Clone, Copy)]
        enum State {
            Text,
            Escape,
            Csi,
            Osc,
            OscEscape,
        }
        let mut state = State::Text;
        let mut plain = Zeroizing::new(Vec::with_capacity(bytes.len()));
        for byte in bytes {
            match state {
                State::Text if *byte == 0x1b => state = State::Escape,
                State::Text => plain.push(*byte),
                State::Escape if *byte == b'[' => state = State::Csi,
                State::Escape if *byte == b']' => state = State::Osc,
                State::Escape => state = State::Text,
                State::Csi if (0x40..=0x7e).contains(byte) => state = State::Text,
                State::Csi => {}
                State::Osc if *byte == 0x07 => state = State::Text,
                State::Osc if *byte == 0x1b => state = State::OscEscape,
                State::Osc => {}
                State::OscEscape if *byte == b'\\' => state = State::Text,
                State::OscEscape => state = State::Osc,
            }
        }
        let text = String::from_utf8_lossy(&plain).into_owned();
        Zeroizing::new(text)
    }
    #[cfg(test)]
    mod drain_tests {
        use super::drain_bounded_for_test;

        #[tokio::test]
        async fn bounded_drain_preserves_exact_bytes_and_source_offset() {
            let source = vec![1_u8; 8192 * 65];
            let (output, offset) = drain_bounded_for_test(source.clone()).await.unwrap();
            assert_eq!(output, source[..8192 * 64]);
            assert_eq!(offset, 8192 * 64);
        }
    }
}
