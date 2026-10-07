#[cfg(target_os = "linux")]
use std::future::{Future, pending};
#[cfg(target_os = "linux")]
use std::io;
#[cfg(target_os = "linux")]
use std::pin::Pin;
#[cfg(target_os = "linux")]
use std::process::Stdio;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
#[cfg(target_os = "linux")]
use tokio::task::JoinSet;
#[cfg(target_os = "linux")]
use tokio::time::{self, Instant};
#[cfg(target_os = "linux")]
use zeroize::Zeroizing;

use crate::brokers::call::BrokerCall;

#[cfg(target_os = "linux")]
use super::buffers::BoundedOutputBuffer;
use super::command::{Command, Output};
#[cfg(target_os = "linux")]
use super::command::{LaunchMode, StreamPolicy};
#[cfg(target_os = "linux")]
use super::error::OutputStream;
use super::error::ProcessError;
#[cfg(target_os = "linux")]
use super::procscan::{ProcessIdentity, process_group, process_identity, process_uid};
#[cfg(target_os = "linux")]
use super::signal::{ProcessSignal, signal_group};

#[cfg(target_os = "linux")]
const ETXTBSY: i32 = 26;
#[cfg(target_os = "linux")]
const SPAWN_RETRY_LIMIT: usize = 3;
#[cfg(target_os = "linux")]
const SPAWN_RETRY_DELAY: Duration = Duration::from_millis(10);
#[cfg(target_os = "linux")]
const CLEANUP_WAIT: Duration = Duration::from_secs(1);

pub async fn run(call: BrokerCall, command: Command) -> Result<Output, ProcessError> {
    #[cfg(target_os = "linux")]
    {
        run_linux(call, command).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (call, command);
        Err(ProcessError::Unsupported)
    }
}

#[cfg(target_os = "linux")]
async fn run_linux(call: BrokerCall, command: Command) -> Result<Output, ProcessError> {
    use process_wrap::tokio::{
        ChildWrapper, CommandWrap, ProcessGroup, ProcessSession, ResetSigmask,
    };

    if call.cancel.is_cancelled() {
        return Err(ProcessError::Cancelled);
    }

    let spawn = || {
        let mut wrapped = CommandWrap::with_new(command.program.as_path(), |child| {
            child
                .args(&command.args)
                .env_clear()
                .envs(command.env.iter())
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
        return Err(ProcessError::LaunchFailed);
    };
    let proc_root = std::path::Path::new("/proc");
    let (Some(identity), Some(pgid), Some(uid)) = (
        process_identity(proc_root, pid),
        process_group(proc_root, pid),
        process_uid(proc_root, pid),
    ) else {
        let _ = child.inner_mut().start_kill();
        let _ = child.inner_mut().wait().await;
        return Err(ProcessError::LaunchFailed);
    };
    if pgid <= 1 || pgid != pid || uid != rustix::process::getuid().as_raw() {
        let _ = child.inner_mut().start_kill();
        let _ = child.inner_mut().wait().await;
        return Err(ProcessError::LaunchFailed);
    }
    let mut guard = ProcessGroupGuard {
        identity,
        pgid,
        active: true,
    };

    let (stdout, stderr, stdin) = {
        let child = child.inner_mut();
        (
            child.stdout().take(),
            child.stderr().take(),
            child.stdin().take(),
        )
    };
    let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
        guard.kill();
        return Err(ProcessError::LaunchFailed);
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
    let status = tokio::select! {
        result = child.inner_mut().wait() => match result {
            Ok(status) => status,
            Err(_) => {
                guard.kill();
                tasks.abort_all();
                return Err(ProcessError::Io);
            }
        },
        _ = call.cancel.cancelled() => {
            return interrupt_run(&mut child, &mut guard, &mut tasks, ProcessError::Cancelled).await;
        },
        _ = &mut timeout => {
            return interrupt_run(&mut child, &mut guard, &mut tasks, ProcessError::TimedOut).await;
        },
        Some(event) = event_rx.recv() => {
            return interrupt_run(&mut child, &mut guard, &mut tasks, event.into_error()).await;
        }
    };

    let mut stdout_result = None;
    let mut stderr_result = None;
    let mut pending_tasks = 2 + usize::from(stdin_pending);
    while pending_tasks > 0 {
        tokio::select! {
            joined = tasks.join_next() => {
                let Some(joined) = joined else {
                    guard.kill();
                    return Err(ProcessError::Io);
                };
                pending_tasks -= 1;
                match joined {
                    Ok(TaskOutput::Stream(OutputStream::Stdout, result)) => stdout_result = Some(result),
                    Ok(TaskOutput::Stream(OutputStream::Stderr, result)) => stderr_result = Some(result),
                    Ok(TaskOutput::Stdin(result)) => {
                        if result.is_err() {
                            guard.kill();
                            tasks.abort_all();
                            return Err(ProcessError::Io);
                        }
                    }
                    Err(_) => {
                        guard.kill();
                        tasks.abort_all();
                        return Err(ProcessError::Io);
                    }
                }
            },
            _ = call.cancel.cancelled() => {
                return interrupt_run(&mut child, &mut guard, &mut tasks, ProcessError::Cancelled).await;
            },
            _ = &mut timeout => {
                return interrupt_run(&mut child, &mut guard, &mut tasks, ProcessError::TimedOut).await;
            },
            Some(event) = event_rx.recv() => {
                return interrupt_run(&mut child, &mut guard, &mut tasks, event.into_error()).await;
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
        guard.kill();
        return Err(ProcessError::OutputTooLarge {
            stream: OutputStream::Stdout,
            cap: stdout.cap,
        });
    }
    if stderr.exceeded {
        guard.kill();
        return Err(ProcessError::OutputTooLarge {
            stream: OutputStream::Stderr,
            cap: stderr.cap,
        });
    }
    guard.active = false;
    Ok(Output {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
    })
}

#[cfg(target_os = "linux")]
async fn interrupt_run(
    child: &mut Box<dyn process_wrap::tokio::ChildWrapper>,
    guard: &mut ProcessGroupGuard,
    tasks: &mut JoinSet<TaskOutput>,
    error: ProcessError,
) -> Result<Output, ProcessError> {
    guard.kill();
    let _ = time::timeout(CLEANUP_WAIT, child.inner_mut().wait()).await;
    tasks.abort_all();
    Err(error)
}

#[cfg(target_os = "linux")]
enum RunEvent {
    OutputTooLarge { stream: OutputStream, cap: usize },
    Io,
}

#[cfg(target_os = "linux")]
impl RunEvent {
    fn into_error(self) -> ProcessError {
        match self {
            Self::OutputTooLarge { stream, cap } => ProcessError::OutputTooLarge { stream, cap },
            Self::Io => ProcessError::Io,
        }
    }
}

#[cfg(target_os = "linux")]
enum TaskOutput {
    Stream(OutputStream, io::Result<Captured>),
    Stdin(io::Result<()>),
}

#[cfg(target_os = "linux")]
struct Captured {
    bytes: Zeroizing<Vec<u8>>,
    cap: usize,
    exceeded: bool,
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
struct ProcessGroupGuard {
    identity: ProcessIdentity,
    pgid: i32,
    active: bool,
}

#[cfg(target_os = "linux")]
impl ProcessGroupGuard {
    fn kill(&self) {
        let _ = signal_group(self.identity, self.pgid, ProcessSignal::Kill);
    }
}

#[cfg(target_os = "linux")]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if self.active {
            self.kill();
        }
    }
}

#[cfg(target_os = "linux")]
fn timeout_future(duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    match Instant::now().checked_add(duration) {
        Some(deadline) => Box::pin(time::sleep_until(deadline)),
        None => Box::pin(pending()),
    }
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
fn is_text_busy(error: &io::Error) -> bool {
    error.raw_os_error() == Some(ETXTBSY)
}

#[cfg(all(target_os = "linux", feature = "test-hooks"))]
pub async fn retry_spawn_for_test<T>(
    enabled: bool,
    spawn: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    retry_spawn(enabled, spawn).await
}
