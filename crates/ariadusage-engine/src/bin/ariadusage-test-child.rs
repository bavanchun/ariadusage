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
        "grandchild"
            | "grandchild-no-session"
            | "hold-pipe"
            | "hold-pipe-stream"
            | "hold-detached-pipe"
            | "term-spawn-grandchild"
    );
    if !inherited {
        let _ =
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::TERM));
    }

    match mode.as_str() {
        "fake-shell" => run_fake_shell(&mode, remaining).await?,
        _ if mode.starts_with('-') => run_fake_shell(&mode, remaining).await?,
        "print-sid" => {
            let sid = rustix::process::getsid(None)?;
            println!("{}", sid.as_raw_pid());
        }
        "marker-present" => {
            println!(
                "{}",
                std::env::var_os("ARIADUSAGE_PROCESS_MARKER").is_some()
            );
        }
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
        "ignore-term" | "ignore-term-ready" => {
            if mode == "ignore-term-ready"
                && let Some(path) = remaining.get(1)
            {
                std::fs::write(path, std::process::id().to_string())?;
            }
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
        "rpc-echo" => run_rpc_echo(remaining.first().and_then(|value| value.to_str()))?,
        "rpc-ignore-term" => {
            let mut terminate = signal(SignalKind::terminate())?;
            loop {
                let _ = terminate.recv().await;
            }
        }
        "pty-prompt" => run_pty_prompt(remaining.first().and_then(|value| value.to_str()))?,
        "pty-flood" => {
            write_pattern("stdout", 2 * 1024 * 1024)?;
            time::sleep(Duration::from_secs(30)).await;
        }
        "setsid" | "grandchild" => {
            let sid = rustix::process::setsid()?;
            if let Some(path) = remaining.get(1) {
                std::fs::write(path, std::process::id().to_string())?;
            } else {
                println!("{}", sid.as_raw_pid());
            }
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
        "spawn-group-no-pipe" => {
            let millis = bounded_millis(remaining.first());
            let child = Command::new(std::env::current_exe()?)
                .arg("grandchild-no-session")
                .arg(millis.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            println!("{}", child.id());
        }
        "spawn-detached-wait" | "spawn-detached-failure" => {
            let millis = bounded_millis(remaining.first());
            let child = Command::new(std::env::current_exe()?)
                .arg("grandchild")
                .arg(millis.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            if let Some(path) = remaining.get(1) {
                std::fs::write(path, child.id().to_string())?;
            }
            println!("{}", child.id());
            if mode == "spawn-detached-failure" {
                return Err(Box::new(std::io::Error::other("synthetic child failure")));
            }
            time::sleep(Duration::from_secs(30)).await;
        }
        "spawn-detached" | "spawn-detached-unmarked" => {
            let millis = bounded_millis(remaining.first());
            let mut child = Command::new(std::env::current_exe()?);
            child
                .arg("grandchild")
                .arg(millis.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if mode == "spawn-detached-unmarked" {
                child.env_remove("ARIADUSAGE_PROCESS_MARKER");
            }
            let child = child.spawn()?;
            println!("{}", child.id());
        }
        "spawn-detached-pair" => {
            let millis = bounded_millis(remaining.first());
            let child = Command::new(std::env::current_exe()?)
                .args(["grandchild", &millis.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            let marked_pid = child.id();
            let child = Command::new(std::env::current_exe()?)
                .arg("grandchild")
                .arg(millis.to_string())
                .env_remove("ARIADUSAGE_PROCESS_MARKER")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            println!("{} {}", marked_pid, child.id());
        }
        "spawn-detached-on-term" => {
            let path = remaining
                .first()
                .ok_or_else(|| std::io::Error::other("missing helper pid path"))?;
            let mut terminate = signal(SignalKind::terminate())?;
            println!("READY");
            terminate.recv().await;
            let child = Command::new(std::env::current_exe()?)
                .args(["grandchild", "50000"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            std::fs::write(path, child.id().to_string())?;
        }
        "hold-pipe" => {
            let millis = bounded_millis(remaining.first());
            let child = Command::new(std::env::current_exe()?)
                .arg("grandchild-no-session")
                .arg(millis.to_string())
                .spawn()?;
            println!("{}", child.id());
        }
        "hold-pipe-stream" => {
            let _ = rustix::process::setsid();
            tokio::spawn(async {
                if let Ok(mut hup) = signal(SignalKind::hangup()) {
                    while hup.recv().await.is_some() {}
                }
            });
            tokio::spawn(async {
                if let Ok(mut term) = signal(SignalKind::terminate()) {
                    while term.recv().await.is_some() {}
                }
            });
            if let Some(path) = remaining.get(1) {
                std::fs::write(path, std::process::id().to_string())?;
            }
            time::sleep(Duration::from_secs(300)).await;
        }
        "hold-detached-pipe" => {
            let millis = bounded_millis(remaining.first());
            let child = Command::new(std::env::current_exe()?)
                .arg("grandchild")
                .arg(millis.to_string())
                .spawn()?;
            println!("{}", child.id());
        }
        "hold-detached-term-spawn" => {
            let mut command = Command::new(std::env::current_exe()?);
            command.arg("term-spawn-grandchild");
            if let Some(path) = remaining.first() {
                command.arg(path);
            }
            let child = command.spawn()?;
            time::sleep(Duration::from_millis(100)).await;
            println!("{}", child.id());
        }
        "term-spawn-grandchild" => {
            let _ = rustix::process::setsid()?;
            let mut terminate = signal(SignalKind::terminate())?;
            println!("READY");
            terminate.recv().await;
            let child = Command::new(std::env::current_exe()?)
                .args(["grandchild-no-session", "50000"])
                .spawn()?;
            if let Some(path) = remaining.first() {
                std::fs::write(path, child.id().to_string())?;
            }
            time::sleep(Duration::from_secs(30)).await;
        }
        "spawn-clear-env-wait" => {
            if let Some(path) = remaining.first() {
                let _child = Command::new(std::env::current_exe()?)
                    .arg("ready-wait")
                    .arg(path)
                    .env_clear()
                    .spawn()?;
                time::sleep(Duration::from_secs(30)).await;
            }
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
fn run_rpc_echo(mode: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{BufRead, Write};

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut reversed = Vec::new();
    let mut sent_noise = false;
    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(request) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(id) = request.get("id").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        if !sent_noise {
            stdout.write_all(b"not-json\n{\"method\":\"notice\",\"params\":{}}\n")?;
            sent_noise = true;
        }
        if mode == Some("error") {
            let message = ["synthetic", "rpc", "private", "value"].join("-");
            writeln!(
                stdout,
                "{{\"id\":{id},\"error\":{{\"message\":{}}}}}",
                serde_json::to_string(&message)?
            )?;
            stdout.flush()?;
            continue;
        }
        if mode == Some("oversize") {
            stdout.write_all(&vec![b'x'; 1024 * 1024 + 1])?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
            continue;
        }
        if mode == Some("missing-result") {
            writeln!(stdout, "{{\"id\":{id}}}")?;
            stdout.flush()?;
            continue;
        }
        let params = request
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let response = serde_json::json!({"id": id, "result": params});
        if mode == Some("reverse") {
            reversed.push(response);
            if reversed.len() == 2 {
                for response in reversed.drain(..).rev() {
                    serde_json::to_writer(&mut stdout, &response)?;
                    stdout.write_all(b"\n")?;
                }
                stdout.flush()?;
            }
        } else {
            serde_json::to_writer(&mut stdout, &response)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
            if mode == Some("exit") {
                return Ok(());
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn run_pty_prompt(mode: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{BufRead, Write};
    use std::time::Duration;

    let mut stdout = std::io::stdout();
    match mode.unwrap_or("prompt") {
        "prompt" => {
            stdout.write_all(b"\x1b[31mready>\x1b[0m")?;
            stdout.flush()?;
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line)?;
            writeln!(stdout, "ack:{line}")?;
        }
        "wait-exit" => {
            stdout.write_all(b"waiting-exit")?;
            stdout.flush()?;
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line)?;
            if line.trim() == "/exit" {
                stdout.write_all(b"exit-received")?;
            }
        }
        "idle" => {
            stdout.write_all(b"ready")?;
            stdout.flush()?;
            std::thread::sleep(Duration::from_secs(3));
        }
        "split-stop" => {
            stdout.write_all(b"NEE")?;
            stdout.flush()?;
            std::thread::sleep(Duration::from_millis(150));
            stdout.write_all(b"DLE")?;
            stdout.flush()?;
            std::thread::sleep(Duration::from_secs(3));
        }
        "split-url" => {
            stdout.write_all(b"https://")?;
            stdout.flush()?;
            std::thread::sleep(Duration::from_millis(150));
            stdout.write_all(b"example.invalid")?;
            stdout.flush()?;
            std::thread::sleep(Duration::from_secs(3));
        }
        "text-window" => {
            stdout.write_all(&vec![b'x'; 9_000])?;
            stdout.write_all(b"Question\r? ")?;
            stdout.flush()?;
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line)?;
            stdout.write_all(b"accepted")?;
            stdout.flush()?;
        }
        "enter-url" => {
            stdout.write_all(b"waiting")?;
            stdout.flush()?;
            let mut line = String::new();
            let stdin = std::io::stdin();
            let mut input = stdin.lock();
            for _ in 0..2 {
                line.clear();
                input.read_line(&mut line)?;
            }
            stdout.write_all(b" http://example.invalid")?;
            stdout.flush()?;
            std::thread::sleep(Duration::from_secs(3));
        }
        "clean-exit" => {
            stdout.write_all(b"buffered-before-exit")?;
            stdout.flush()?;
        }
        "env" => {
            let current_dir = std::env::current_dir()?;
            for name in [
                "TERM",
                "COLORTERM",
                "LANG",
                "CI",
                "HOME",
                "PWD",
                "UNLISTED_PROCESS_TEST",
            ] {
                let value = std::env::var(name).unwrap_or_else(|_| "<missing>".to_owned());
                writeln!(stdout, "{name}={value}")?;
            }
            writeln!(stdout, "CWD={}", current_dir.display())?;
            stdout.flush()?;
        }
        "signal-mask" => {
            use nix::sys::signal::{SigSet, SigmaskHow, Signal, pthread_sigmask};

            let mut current = SigSet::empty();
            pthread_sigmask(SigmaskHow::SIG_SETMASK, None, Some(&mut current))?;
            writeln!(stdout, "{}", current.contains(Signal::SIGTERM))?;
            stdout.flush()?;
        }
        _ => return Err(Box::new(std::io::Error::other("unknown PTY helper mode"))),
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

#[cfg(target_os = "linux")]
async fn run_fake_shell(
    mode: &str,
    remaining: Vec<std::ffi::OsString>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut all_args = Vec::new();
    if mode != "fake-shell" {
        all_args.push(std::ffi::OsString::from(mode));
    }
    all_args.extend(remaining);

    let mut command_str = String::new();
    let mut iter = all_args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "-c" {
            if let Some(cmd) = iter.next() {
                command_str = cmd.to_string_lossy().into_owned();
            }
            break;
        }
    }

    if let Ok(holders_file) = std::env::var("FAKE_SHELL_HOLDERS_PID_FILE") {
        let stdout_file = format!("{holders_file}.stdout");
        let stderr_file = format!("{holders_file}.stderr");
        let _child1 = Command::new(std::env::current_exe()?)
            .args(["hold-pipe-stream", "1", &stdout_file])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let _child2 = Command::new(std::env::current_exe()?)
            .args(["hold-pipe-stream", "2", &stderr_file])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()?;
        for _ in 0..100 {
            if std::path::Path::new(&stdout_file).exists()
                && std::path::Path::new(&stderr_file).exists()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
        return Ok(());
    }

    if command_str.contains("trap '' HUP TERM") || std::env::var_os("FAKE_SHELL_BG_CHILD").is_some()
    {
        if let Ok(marker) = std::env::var("FAKE_SHELL_MARKER") {
            let _ = std::fs::write(&marker, b"marker");
        }
        let child = Command::new(std::env::current_exe()?)
            .args(["grandchild", "50000"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        println!("{}", child.id());
        return Ok(());
    }

    if command_str.contains("noisy") || std::env::var_os("FAKE_SHELL_NOISY").is_some() {
        for i in 0..4000 {
            println!("out-{i:04}");
            eprintln!("err-{i:04}");
        }
        println!("__CODEXBAR_DONE__");
        return Ok(());
    }

    if command_str.contains("head -c") || std::env::var_os("FAKE_SHELL_ZERO_BYTES").is_some() {
        if command_str.contains(">&2") || std::env::var_os("FAKE_SHELL_STDERR_BYTES").is_some() {
            let stderr_bytes = std::env::var("FAKE_SHELL_STDERR_BYTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8 * 1024 * 1024);
            let chunk = vec![0u8; 64 * 1024];
            let mut remaining = stderr_bytes;
            let mut stderr = std::io::stderr();
            while remaining > 0 {
                let to_write = std::cmp::min(remaining, chunk.len());
                stderr.write_all(&chunk[..to_write])?;
                remaining -= to_write;
            }
            stderr.flush()?;
            if command_str.contains("printf") {
                print!("/synthetic/bin");
                std::io::stdout().flush()?;
            }
            return Ok(());
        }

        let count: usize = std::env::var("FAKE_SHELL_ZERO_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .or_else(|| {
                let parts: Vec<&str> = command_str.split_whitespace().collect();
                if let Some(pos) = parts.iter().position(|&p| p == "-c") {
                    parts.get(pos + 1).and_then(|s| s.parse().ok())
                } else {
                    None
                }
            })
            .unwrap_or(1024);
        let chunk = vec![0u8; 64 * 1024];
        let mut remaining = count;
        let mut stdout = std::io::stdout();
        while remaining > 0 {
            let to_write = std::cmp::min(remaining, chunk.len());
            stdout.write_all(&chunk[..to_write])?;
            remaining -= to_write;
        }
        stdout.flush()?;
        return Ok(());
    }

    if command_str.contains("__ARIADUSAGE_PATH__") {
        if std::env::var_os("FAKE_SHELL_OVERFLOW").is_some() {
            print!(
                "__ARIADUSAGE_PATH__{}__ARIADUSAGE_PATH__",
                "a".repeat(1500 * 1024)
            );
        } else if let Ok(path) = std::env::var("FAKE_SHELL_PATH") {
            print!("__ARIADUSAGE_PATH__{path}__ARIADUSAGE_PATH__");
        } else if let Ok(path) = std::env::var("PATH") {
            print!("__ARIADUSAGE_PATH__{path}__ARIADUSAGE_PATH__");
        } else {
            print!("__ARIADUSAGE_PATH__/usr/bin:/bin__ARIADUSAGE_PATH__");
        }
        std::io::stdout().flush()?;
        return Ok(());
    }

    if command_str.contains("command -v") {
        if let Ok(val) = std::env::var("FAKE_SHELL_COMMAND_V") {
            println!("{val}");
        } else {
            let tool = command_str.split("command -v").nth(1).unwrap_or("").trim();
            if let Ok(val) = std::env::var(format!("FAKE_SHELL_COMMAND_V_{}", tool.to_uppercase()))
            {
                println!("{val}");
            } else {
                println!("/shell/bin/{tool}");
            }
        }
        return Ok(());
    }

    if command_str.contains("alias") || command_str.contains("type -a") {
        if let Ok(val) = std::env::var("FAKE_SHELL_ALIAS") {
            println!("{val}");
        } else {
            let tool = if command_str.contains("alias ") {
                command_str
                    .split("alias ")
                    .nth(1)
                    .unwrap_or("")
                    .split_whitespace()
                    .next()
                    .unwrap_or("claude")
            } else {
                "claude"
            };
            if let Ok(val) = std::env::var(format!("FAKE_SHELL_ALIAS_{}", tool.to_uppercase())) {
                println!("{val}");
            } else {
                println!("alias {tool}='/fakehome/.claude/local/bin/{tool}'");
            }
        }
        return Ok(());
    }

    Ok(())
}
