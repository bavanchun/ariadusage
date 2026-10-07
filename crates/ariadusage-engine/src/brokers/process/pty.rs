// Ported from CodexBar Sources/CodexBarCore/Host/PTY/TTYCommandRunner.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::fmt;
use std::time::Duration;

use thiserror::Error;
use zeroize::Zeroizing;

use super::{Command, ProcessError};

const DEFAULT_ROWS: u16 = 50;
const DEFAULT_COLS: u16 = 160;
#[cfg(target_os = "linux")]
const OUTPUT_CAP: usize = 1024 * 1024;
#[cfg(target_os = "linux")]
const RECENT_TEXT_CAP: usize = 8 * 1024;
#[cfg(target_os = "linux")]
const READ_POLL: Duration = Duration::from_millis(60);
#[cfg(target_os = "linux")]
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
    #[cfg(target_os = "linux")]
    inner: linux::PtyInner,
}

impl PtySession {
    pub async fn spawn(
        call: crate::brokers::call::BrokerCall,
        command: Command,
        size: PtySize,
        registry: &super::ProcessRegistry,
        gate: &ariadusage_core::gates::launch::LaunchGate,
    ) -> Result<Self, PtyError> {
        #[cfg(target_os = "linux")]
        {
            Ok(Self {
                inner: linux::spawn(call, command, size, registry, gate).await?,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (call, command, size, registry, gate);
            Err(PtyError::Unsupported)
        }
    }

    pub async fn run_script(&mut self, script: PtyScript) -> Result<PtyTranscript, PtyError> {
        #[cfg(target_os = "linux")]
        {
            self.inner.run_script(script).await
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = script;
            Err(PtyError::Unsupported)
        }
    }

    pub async fn write(&mut self, bytes: &[u8]) -> Result<(), PtyError> {
        #[cfg(target_os = "linux")]
        {
            self.inner.write(bytes).await
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = bytes;
            Err(PtyError::Unsupported)
        }
    }

    pub async fn close(&mut self) {
        #[cfg(target_os = "linux")]
        self.inner.close().await;
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

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use pty_process::{Command as PtyCommand, Size, open};
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

    pub(super) struct PtyInner {
        master: Option<pty_process::Pty>,
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
        let binary = command.program.as_path().to_string_lossy().into_owned();
        if let Err(until) = gate.check(&binary, call.interaction) {
            return Err(super::PtyError::Process(
                crate::brokers::process::ProcessError::LaunchSuppressed { until },
            ));
        }
        let mut permit = registry.register()?;
        let (master, slave) = open().map_err(|_| PtyError::Infrastructure)?;
        master
            .resize(Size::new(size.rows, size.cols))
            .map_err(|_| PtyError::Infrastructure)?;
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
        let mut pty_command = PtyCommand::new(command.program.as_path()).kill_on_drop(true);
        pty_command = pty_command.args(&command.args).env_clear().envs(env.iter());
        if let Some(cwd) = &command.cwd {
            pty_command = pty_command.current_dir(cwd);
        }
        let mut child = pty_command
            .spawn(slave)
            .map_err(|_| PtyError::Infrastructure)?;
        let Some(pid) = child.id() else {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(PtyError::Infrastructure);
        };
        let target = match session_target(pid, &[]) {
            Ok(target) => target,
            Err(error) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(error.into());
            }
        };
        permit.attach(target.clone());
        let (exit_tx, exit_rx) = watch::channel(false);
        tokio::spawn(async move {
            let _ = child.wait().await;
            drop(permit);
            exit_tx.send_replace(true);
        });
        Ok(PtyInner {
            master: Some(master),
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
                    PtyCompletionReason::ProcessExited
                } else {
                    reason
                };
                return Ok(transcript(output, reason));
            }

            let exited = self.has_exited();
            if closed && !exited {
                if self.wait_for_exit(EXIT_DRAIN).await {
                    return Ok(transcript(output, PtyCompletionReason::ProcessExited));
                }
                self.close().await;
                return Err(PtyError::TimedOut);
            }
            if exited {
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
                PtyCompletionReason::ProcessExited
            } else {
                PtyCompletionReason::DeadlineExceeded
            };
            Ok(transcript(output, reason))
        }

        async fn read_once(
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

        fn has_exited(&self) -> bool {
            *self.exit_rx.borrow()
        }

        async fn wait_for_exit(&mut self, timeout: Duration) -> bool {
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
            self.master.take();
            self.shutdown_process().await;
        }

        async fn shutdown_process(&mut self) {
            if let Some(mut guard) = self.guard.take() {
                guard.terminate().await;
            }
            let _ = self.wait_for_exit(Duration::from_secs(1)).await;
        }
    }

    impl Drop for PtyInner {
        fn drop(&mut self) {
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
}
