#[cfg(target_os = "linux")]
fn main() {
    if install_watchdog().is_err() {
        std::process::exit(2);
    }
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("list-fds")) {
        if list_fds().is_err() {
            std::process::exit(2);
        }
        return;
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|_| std::process::exit(2));
    if runtime.block_on(run()).is_err() {
        std::process::exit(2);
    }
}

#[cfg(target_os = "linux")]
fn install_watchdog() -> Result<(), std::io::Error> {
    use std::time::Duration;

    std::thread::Builder::new()
        .name("ariadusage-test-child-deadline".into())
        .spawn(|| {
            std::thread::sleep(Duration::from_secs(50));
            let _ = rustix::process::kill_process(
                rustix::process::getpid(),
                rustix::process::Signal::KILL,
            );
            std::process::exit(2);
        })?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use nix::sys::signal::{SigSet, SigmaskHow, Signal, pthread_sigmask};
    use tokio::signal::unix::{SignalKind, signal};
    use tokio::time;

    let mut args = std::env::args_os().skip(1);
    let mode = args
        .next()
        .and_then(|value| value.into_string().ok())
        .unwrap_or_default();
    let remaining = args.collect::<Vec<_>>();
    let inherited = matches!(
        mode.as_str(),
        "grandchild" | "grandchild-no-session" | "hold-pipe"
    );
    if !inherited {
        let _ =
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::TERM));
    }

    match mode.as_str() {
        "sleep" => {
            let millis = bounded_millis(remaining.first());
            time::sleep(Duration::from_millis(millis)).await;
        }
        "stream" => {
            let deadline = time::sleep(Duration::from_secs(30));
            tokio::pin!(deadline);
            let mut stdout = std::io::stdout();
            let chunk = vec![b'x'; 4096];
            loop {
                stdout.write_all(&chunk)?;
                stdout.flush()?;
                tokio::select! {
                    _ = &mut deadline => break,
                    _ = time::sleep(Duration::from_millis(2)) => {},
                }
            }
        }
        "print" => {
            let destination = remaining
                .first()
                .and_then(|value| value.to_str())
                .unwrap_or("stdout");
            let count = bounded_count(remaining.get(1));
            write_pattern(destination, count)?;
        }
        "stderr-exit" => {
            let _ignored = read_stdin_bounded().await?;
            let message = remaining
                .first()
                .and_then(|value| value.to_str())
                .unwrap_or("synthetic-error");
            let _ = writeln!(std::io::stderr(), "{message}");
            return Err(Box::new(std::io::Error::other("synthetic child exit")));
        }
        "stderr-large" => {
            std::io::stdout().write_all(b"x")?;
            write_pattern("stderr", bounded_count(remaining.first()))?;
        }
        "stdin-echo" => {
            let bytes = read_stdin_bounded().await?;
            std::io::stdout().write_all(&bytes)?;
        }
        "ready-wait" => {
            if let Some(path) = remaining.first() {
                std::fs::write(path, std::process::id().to_string())?;
            }
            println!("READY");
            time::sleep(Duration::from_secs(60)).await;
        }
        "ignore-term" => {
            let mut terminate = signal(SignalKind::terminate())?;
            let deadline = time::sleep(Duration::from_millis(bounded_millis(remaining.first())));
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    _ = &mut deadline => break,
                    _ = terminate.recv() => {},
                }
            }
        }
        "signal-mask" => {
            let mut current = SigSet::empty();
            pthread_sigmask(SigmaskHow::SIG_SETMASK, None, Some(&mut current))?;
            println!("{}", current.contains(Signal::SIGTERM));
        }
        "setsid" | "grandchild" => {
            let sid = rustix::process::setsid()?;
            println!("{}", sid.as_raw_pid());
            time::sleep(Duration::from_millis(bounded_millis(remaining.first()))).await;
        }
        "grandchild-no-session" => {
            time::sleep(Duration::from_millis(bounded_millis(remaining.first()))).await;
        }
        "spawn-grandchild" => {
            let millis = bounded_millis(remaining.first());
            let child = Command::new(std::env::current_exe()?)
                .arg("grandchild")
                .arg(millis.to_string())
                .stdin(Stdio::null())
                .spawn()?;
            println!("{}", child.id());
        }
        "hold-pipe" => {
            let millis = bounded_millis(remaining.first());
            let child = Command::new(std::env::current_exe()?)
                .arg("grandchild-no-session")
                .arg(millis.to_string())
                .spawn()?;
            println!("{}", child.id());
        }
        "clear-env" => {
            let status = Command::new(std::env::current_exe()?)
                .arg("inspect-env")
                .env_clear()
                .status()?;
            if !status.success() {
                return Err(Box::new(std::io::Error::other("synthetic re-exec failed")));
            }
        }
        "inspect-env" => {
            println!("{}", std::env::vars_os().next().is_none());
        }
        "list-env-names" => {
            let mut names = std::env::vars_os()
                .map(|(name, _)| name)
                .collect::<Vec<_>>();
            names.sort_unstable();
            let mut stdout = std::io::stdout();
            for name in names {
                writeln!(stdout, "{}", name.to_string_lossy())?;
            }
            stdout.flush()?;
        }
        "env-value" => {
            let name = remaining
                .first()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if let Some(value) = std::env::var_os(name) {
                std::io::stdout().write_all(value.to_string_lossy().as_bytes())?;
            }
        }
        "marker" => {
            if let Some((_, value)) =
                std::env::vars_os().find(|(key, _)| key == "ARIADUSAGE_PROBE_OWNER")
            {
                std::io::stdout().write_all(value.to_string_lossy().as_bytes())?;
            }
        }
        "split-utf8" => {
            let ascii_count = bounded_count(remaining.first());
            let mut stdout = std::io::stdout();
            let chunk = vec![b'x'; ascii_count.min(16 * 1024)];
            let mut written = 0;
            while written < ascii_count {
                let count = (ascii_count - written).min(chunk.len());
                stdout.write_all(&chunk[..count])?;
                written += count;
            }
            stdout.write_all(&[0xe2, 0x82, 0xac])?;
            stdout.flush()?;
        }
        _ => return Err(Box::new(std::io::Error::other("unknown helper mode"))),
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn list_fds() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;

    rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::TERM))?;
    let mut stdout = std::io::stdout();
    writeln!(stdout, "PID {}", std::process::id())?;
    stdout.flush()?;
    for entry in std::fs::read_dir("/proc/self/fd")? {
        let entry = entry?;
        if let Ok(target) = std::fs::read_link(entry.path()) {
            writeln!(
                stdout,
                "{} {}",
                entry.file_name().to_string_lossy(),
                target.display()
            )?;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
async fn read_stdin_bounded() -> Result<zeroize::Zeroizing<Vec<u8>>, std::io::Error> {
    use std::io::{self, Read};
    use std::os::fd::AsFd;
    use std::time::Duration;

    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};

    const MAX_STDIN: usize = 1024 * 1024;
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let flags = fcntl_getfl(reader.as_fd())?;
    fcntl_setfl(reader.as_fd(), flags | OFlags::NONBLOCK)?;
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    let mut buffer = [0_u8; 8192];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(bytes),
            Ok(count) => {
                if bytes.len() + count > MAX_STDIN {
                    return Err(io::Error::other("synthetic stdin exceeded its bound"));
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {},
                    _ = tokio::time::sleep_until(deadline) => {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "synthetic stdin timed out"));
                    }
                }
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(target_os = "linux")]
fn write_pattern(destination: &str, count: usize) -> Result<(), std::io::Error> {
    use std::io::Write;

    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let chunk = vec![b'x'; 16 * 1024];
    let mut written = 0;
    while written < count {
        let size = (count - written).min(chunk.len());
        match destination {
            "stderr" => stderr.write_all(&chunk[..size])?,
            "both" => {
                stdout.write_all(&chunk[..size])?;
                stderr.write_all(&chunk[..size])?;
            }
            _ => stdout.write_all(&chunk[..size])?,
        }
        written += size;
    }
    stdout.flush()?;
    stderr.flush()?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn bounded_millis(value: Option<&std::ffi::OsString>) -> u64 {
    value
        .and_then(|value| value.to_str())
        .and_then(|value| value.parse().ok())
        .unwrap_or(1000)
        .min(60_000)
}

#[cfg(target_os = "linux")]
fn bounded_count(value: Option<&std::ffi::OsString>) -> usize {
    value
        .and_then(|value| value.to_str())
        .and_then(|value| value.parse().ok())
        .unwrap_or(1)
        .min(16 * 1024 * 1024)
}
