#[cfg(unix)]
use std::future::{Future, pending};
#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::os::fd::AsFd;
#[cfg(unix)]
use std::pin::Pin;
#[cfg(unix)]
use std::process::Stdio;
#[cfg(unix)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::task::JoinSet;
#[cfg(unix)]
use tokio::time::{self, Instant};
#[cfg(unix)]
use zeroize::Zeroizing;

#[cfg(unix)]
use ariadusage_core::gates::launch::LaunchFailureKind;
use ariadusage_core::gates::launch::LaunchGate;

use crate::brokers::call::BrokerCall;

#[cfg(unix)]
use super::buffers::BoundedOutputBuffer;
use super::command::{Command, Output};
#[cfg(unix)]
use super::command::{LaunchMode, StreamPolicy};
#[cfg(unix)]
use super::error::OutputStream;
use super::error::ProcessError;
#[cfg(unix)]
use super::holders::{reap_output_holders, reap_output_holders_sync};
#[cfg(unix)]
use super::procscan::{
    PROCESS_MARKER_ENV, ProcessIdentity, process_group, process_identity, process_state,
    process_uid,
};
#[cfg(unix)]
use super::reaper::ReapGuard;
#[cfg(unix)]
use super::registry::LaunchPermit;
use super::registry::ProcessRegistry;
#[cfg(unix)]
use super::signal::{ProcessSignal, signal};
#[cfg(unix)]
use super::teardown::{ProcessTarget, terminate, terminate_sync};

#[cfg(unix)]
const ETXTBSY: i32 = 26;
#[cfg(unix)]
const SPAWN_RETRY_LIMIT: usize = 3;
#[cfg(unix)]
const SPAWN_RETRY_DELAY: Duration = Duration::from_millis(10);
#[cfg(unix)]
const CLEANUP_WAIT: Duration = Duration::from_secs(1);
#[cfg(unix)]
static NEXT_PROCESS_MARKER: AtomicU64 = AtomicU64::new(1);

pub async fn run(
    call: BrokerCall,
    command: Command,
    registry: &ProcessRegistry,
    gate: &LaunchGate,
) -> Result<Output, ProcessError> {
    #[cfg(unix)]
    {
        let binary = command.program.as_path().to_string_lossy().into_owned();
        if let Err(until) = gate.check(&binary, call.interaction) {
            return Err(ProcessError::LaunchSuppressed { until });
        }
        let permit = registry.register()?;
        run_unix(call, command, permit, gate, &binary).await
    }
    #[cfg(not(unix))]
    {
        let _ = (call, command, registry, gate);
        Err(ProcessError::Unsupported)
    }
}

#[cfg(unix)]
async fn run_unix(
    call: BrokerCall,
    command: Command,
    mut permit: LaunchPermit,
    gate: &LaunchGate,
    binary: &str,
) -> Result<Output, ProcessError> {
    let marker = command.reap_marker.then(next_process_marker);
    let mut reaper = ReapGuard::new(marker.clone(), 0);
    let result = run_unix_inner(call, command, marker, &mut permit, &mut reaper).await;
    reaper.finish().await;
    if matches!(result, Err(ProcessError::LaunchFailed)) {
        let _ = gate.record_failure(binary, LaunchFailureKind::Process);
    }
    permit.finish();
    result
}

#[cfg(unix)]
fn next_process_marker() -> String {
    let sequence = NEXT_PROCESS_MARKER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{sequence:016x}", std::process::id())
}

#[cfg(unix)]
async fn run_unix_inner(
    call: BrokerCall,
    command: Command,
    marker: Option<String>,
    permit: &mut LaunchPermit,
    reaper: &mut ReapGuard,
) -> Result<Output, ProcessError> {
    use process_wrap::tokio::{
        ChildWrapper, CommandWrap, ProcessGroup, ProcessSession, ResetSigmask,
    };

    if call.cancel.is_cancelled() {
        return Err(ProcessError::Cancelled);
    }

    let child_env = marker.as_ref().map_or_else(
        || command.env.clone(),
        |marker| {
            command
                .env
                .clone()
                .with_internal(PROCESS_MARKER_ENV, marker.clone())
        },
    );

    let spawn = || {
        let mut wrapped = CommandWrap::with_new(command.program.as_path(), |child| {
            child
                .args(&command.args)
                .env_clear()
                .envs(child_env.iter())
                .stdin(if command.stdin.is_null() {
                    Stdio::null()
                } else {
                    Stdio::piped()
                })
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
        }
        let result: io::Result<Box<dyn ChildWrapper>> = wrapped.spawn();
        result
    };
    let mut child = retry_spawn(command.retry_text_busy, spawn)
        .await
        .map_err(|_| ProcessError::LaunchFailed)?;
    let Some(pid) = child.inner().id().map(|pid| pid as i32) else {
        let _ = child.inner_mut().start_kill();
        let _ = child.inner_mut().wait().await;
        return Err(ProcessError::LaunchFailed);
    };
    reaper.set_pgid(pid);
    let proc_root = std::path::Path::new("/proc");
    let identity = match process_identity(proc_root, pid) {
        Some(identity) => identity,
        None if process_state(proc_root, pid) == Some('Z') => ProcessIdentity {
            pid,
            start_ticks: 0,
        },
        None => {
            let _ = child.inner_mut().start_kill();
            let _ = child.inner_mut().wait().await;
            return Err(ProcessError::LaunchFailed);
        }
    };
    let (Some(pgid), Some(uid)) = (process_group(proc_root, pid), process_uid(proc_root, pid))
    else {
        let _ = signal(identity, ProcessSignal::Kill);
        let _ = child.inner_mut().wait().await;
        return Err(ProcessError::LaunchFailed);
    };
    if pgid <= 1
        || pgid != pid
        || pgid == rustix::process::getpid().as_raw_pid()
        || pgid == rustix::process::getpgrp().as_raw_pid()
        || uid != rustix::process::getuid().as_raw()
    {
        let _ = signal(identity, ProcessSignal::Kill);
        let _ = child.inner_mut().wait().await;
        return Err(ProcessError::LaunchFailed);
    }
    let (stdout, stderr, stdin) = {
        let child = child.inner_mut();
        (
            child.stdout().take(),
            child.stderr().take(),
            child.stdin().take(),
        )
    };
    let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
        let target = ProcessTarget::new(identity, pgid, uid, Vec::new());
        let mut guard = ProcessGroupGuard {
            target,
            active: true,
        };
        guard.terminate().await;
        let _ = child.inner_mut().wait().await;
        return Err(ProcessError::LaunchFailed);
    };
    let (pipes, pipe_holds) = super::capture_output_pipes(&[stdout.as_fd(), stderr.as_fd()]);
    let target = ProcessTarget::with_pipe_holds(identity, pgid, uid, pipes, pipe_holds);
    target.refresh_descendants();
    permit.attach(target.clone());
    let mut guard = ProcessGroupGuard {
        target,
        active: true,
    };
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tasks = JoinSet::new();
    tasks.spawn(drain_stream(
        stdout,
        command.stdout,
        OutputStream::Stdout,
        event_tx.clone(),
    ));
    tasks.spawn(drain_stream(
        stderr,
        command.stderr,
        OutputStream::Stderr,
        event_tx.clone(),
    ));
    let stdin_pending = match (stdin, command.stdin.into_bytes()) {
        (Some(stdin), Some(bytes)) => {
            tasks.spawn(write_stdin(stdin, bytes, event_tx.clone()));
            true
        }
        _ => false,
    };
    drop(event_tx);

    let mut timeout = timeout_future(command.timeout);
    let mut tracker = time::interval(Duration::from_millis(20));
    tracker.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    let status = loop {
        tokio::select! {
            biased;
            Some(event) = event_rx.recv() => {
                return interrupt_run(&mut child, &mut guard, &mut tasks, event.into_error()).await;
            },
            _ = call.cancel.cancelled() => {
                return interrupt_run(&mut child, &mut guard, &mut tasks, ProcessError::Cancelled).await;
            },
            _ = &mut timeout => {
                return interrupt_run(&mut child, &mut guard, &mut tasks, ProcessError::TimedOut).await;
            },
            _ = tracker.tick() => guard.target.refresh_descendants(),
            result = child.inner_mut().wait() => match result {
                Ok(status) => break status,
                Err(_) => {
                    guard.terminate().await;
                    tasks.abort_all();
                    return Err(ProcessError::Io);
                }
            }
        }
    };

    if let Ok(event) = event_rx.try_recv() {
        return interrupt_run(&mut child, &mut guard, &mut tasks, event.into_error()).await;
    }
    {
        let holder_target = guard.target.clone();
        let mut holder_cleanup = Box::pin(reap_output_holders(&holder_target));
        loop {
            tokio::select! {
                biased;
                Some(event) = event_rx.recv() => {
                    return interrupt_run(&mut child, &mut guard, &mut tasks, event.into_error()).await;
                },
                _ = call.cancel.cancelled() => {
                    return interrupt_run(&mut child, &mut guard, &mut tasks, ProcessError::Cancelled).await;
                },
                _ = &mut timeout => {
                    return interrupt_run(&mut child, &mut guard, &mut tasks, ProcessError::TimedOut).await;
                },
                _ = tracker.tick() => guard.target.refresh_descendants(),
                _ = &mut holder_cleanup => break,
            }
        }
    }

    let mut stdout_result = None;
    let mut stderr_result = None;
    let mut pending_tasks = 2 + usize::from(stdin_pending);
    while pending_tasks > 0 {
        tokio::select! {
            biased;
            Some(event) = event_rx.recv() => {
                return interrupt_run(&mut child, &mut guard, &mut tasks, event.into_error()).await;
            },
            _ = call.cancel.cancelled() => {
                return interrupt_run(&mut child, &mut guard, &mut tasks, ProcessError::Cancelled).await;
            },
            _ = &mut timeout => {
                return interrupt_run(&mut child, &mut guard, &mut tasks, ProcessError::TimedOut).await;
            },
            _ = tracker.tick() => guard.target.refresh_descendants(),
            joined = tasks.join_next() => {
                let Some(joined) = joined else {
                    guard.terminate().await;
                    return Err(ProcessError::Io);
                };
                pending_tasks -= 1;
                match joined {
                    Ok(TaskOutput::Stream(OutputStream::Stdout, result)) => stdout_result = Some(result),
                    Ok(TaskOutput::Stream(OutputStream::Stderr, result)) => stderr_result = Some(result),
                    Ok(TaskOutput::Stdin(result)) => {
                        if result.is_err() {
                            guard.terminate().await;
                            tasks.abort_all();
                            return Err(ProcessError::Io);
                        }
                    }
                    Err(_) => {
                        guard.terminate().await;
                        tasks.abort_all();
                        return Err(ProcessError::Io);
                    }
                }
            }
        }
    }

    let stdout = stdout_result
        .ok_or(ProcessError::Io)?
        .map_err(|_| ProcessError::Io)?;
    let stderr = stderr_result
        .ok_or(ProcessError::Io)?
        .map_err(|_| ProcessError::Io)?;
    if stdout.exceeded {
        guard.terminate().await;
        return Err(ProcessError::OutputTooLarge {
            stream: OutputStream::Stdout,
            cap: stdout.cap,
        });
    }
    if stderr.exceeded {
        guard.terminate().await;
        return Err(ProcessError::OutputTooLarge {
            stream: OutputStream::Stderr,
            cap: stderr.cap,
        });
    }
    guard.terminate().await;
    Ok(Output {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
    })
}

#[cfg(unix)]
async fn interrupt_run(
    child: &mut Box<dyn process_wrap::tokio::ChildWrapper>,
    guard: &mut ProcessGroupGuard,
    tasks: &mut JoinSet<TaskOutput>,
    error: ProcessError,
) -> Result<Output, ProcessError> {
    guard.terminate().await;
    let _ = time::timeout(CLEANUP_WAIT, child.inner_mut().wait()).await;
    tasks.abort_all();
    Err(error)
}

#[cfg(unix)]
enum RunEvent {
    OutputTooLarge { stream: OutputStream, cap: usize },
    Io,
}

#[cfg(unix)]
impl RunEvent {
    fn into_error(self) -> ProcessError {
        match self {
            Self::OutputTooLarge { stream, cap } => ProcessError::OutputTooLarge { stream, cap },
            Self::Io => ProcessError::Io,
        }
    }
}

#[cfg(unix)]
enum TaskOutput {
    Stream(OutputStream, io::Result<Captured>),
    Stdin(io::Result<()>),
}

#[cfg(unix)]
struct Captured {
    bytes: Zeroizing<Vec<u8>>,
    cap: usize,
    exceeded: bool,
}

#[cfg(unix)]
async fn drain_stream<R>(
    mut reader: R,
    policy: StreamPolicy,
    stream: OutputStream,
    events: tokio::sync::mpsc::UnboundedSender<RunEvent>,
) -> TaskOutput
where
    R: AsyncRead + Unpin,
{
    let cap = match policy {
        StreamPolicy::Capture { cap } => cap,
        StreamPolicy::Discard => 0,
    };
    let mut output = BoundedOutputBuffer::new(cap);
    let mut exceeded = false;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(count) => count,
            Err(_) => {
                let _ = events.send(RunEvent::Io);
                return TaskOutput::Stream(stream, Err(io::Error::other("stream read failed")));
            }
        };
        if let StreamPolicy::Capture { .. } = policy
            && !exceeded
            && !output.append(&buffer[..count])
        {
            exceeded = true;
            let _ = events.send(RunEvent::OutputTooLarge { stream, cap });
        }
    }
    let bytes = match policy {
        StreamPolicy::Capture { .. } => output.into_bytes(),
        StreamPolicy::Discard => Zeroizing::new(Vec::new()),
    };
    TaskOutput::Stream(
        stream,
        Ok(Captured {
            bytes,
            cap,
            exceeded,
        }),
    )
}

#[cfg(unix)]
async fn write_stdin(
    mut stdin: tokio::process::ChildStdin,
    bytes: Zeroizing<Vec<u8>>,
    events: tokio::sync::mpsc::UnboundedSender<RunEvent>,
) -> TaskOutput {
    let result = stdin.write_all(&bytes).await.map_err(|_| {
        let _ = events.send(RunEvent::Io);
        io::Error::other("stdin write failed")
    });
    drop(stdin);
    TaskOutput::Stdin(result)
}

#[cfg(unix)]
struct ProcessGroupGuard {
    target: ProcessTarget,
    active: bool,
}

#[cfg(unix)]
impl ProcessGroupGuard {
    async fn terminate(&mut self) {
        if self.active {
            terminate(&self.target).await;
            self.target.release_pipes();
            self.active = false;
        }
    }
}

#[cfg(unix)]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if self.active {
            terminate_sync(&self.target);
            reap_output_holders_sync(&self.target);
            self.active = false;
        }
    }
}

#[cfg(unix)]
fn timeout_future(duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    match Instant::now().checked_add(duration) {
        Some(deadline) => Box::pin(time::sleep_until(deadline)),
        None => Box::pin(pending()),
    }
}

#[cfg(unix)]
async fn retry_spawn<T>(enabled: bool, mut spawn: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let max_attempts = if enabled { SPAWN_RETRY_LIMIT } else { 1 };
    for attempt in 0..max_attempts {
        match spawn() {
            Ok(value) => return Ok(value),
            Err(error) if enabled && is_text_busy(&error) && attempt + 1 < max_attempts => {
                time::sleep(SPAWN_RETRY_DELAY).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other("process launch failed"))
}

#[cfg(unix)]
fn is_text_busy(error: &io::Error) -> bool {
    error.raw_os_error() == Some(ETXTBSY)
}

#[cfg(all(unix, feature = "test-hooks"))]
pub async fn retry_spawn_for_test<T>(
    enabled: bool,
    spawn: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    retry_spawn(enabled, spawn).await
}
