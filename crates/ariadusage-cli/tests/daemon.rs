#[cfg(unix)]
mod unix_tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use assert_cmd::cargo::cargo_bin;

    #[test]
    fn daemon_serves_snapshot_over_socket_and_cleans_up_on_sigterm() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let home_dir = temp_dir.path().join("home");
        let config_dir = temp_dir.path().join("config");
        let state_dir = temp_dir.path().join("state");
        let run_dir = temp_dir.path().join("run");
        for dir in [&home_dir, &config_dir, &state_dir, &run_dir] {
            std::fs::create_dir_all(dir).expect("create dir");
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                .expect("set 0700");
        }

        let sock_path = run_dir.join("ariadusage").join("engine.sock");
        let bin = cargo_bin("ariadusage");

        let mut daemon = Command::new(&bin)
            .args(["daemon", "--socket", sock_path.to_str().unwrap()])
            .env("HOME", &home_dir)
            .env("XDG_CONFIG_HOME", &config_dir)
            .env("XDG_STATE_HOME", &state_dir)
            .env("XDG_RUNTIME_DIR", &run_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn daemon");

        // Wait for the socket file to appear (up to 5s).
        let deadline = Instant::now() + Duration::from_secs(5);
        while !sock_path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(sock_path.exists(), "daemon socket was not created");

        // Verify 0700 parent directory and 0600 socket file permissions.
        let parent_mode = std::fs::metadata(sock_path.parent().unwrap())
            .expect("parent metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(parent_mode, 0o700);
        let sock_mode = std::fs::metadata(&sock_path)
            .expect("socket metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(sock_mode, 0o600);

        // Query the running daemon with `ariadusage status --socket <path> --json`.
        let status_out = Command::new(&bin)
            .args(["status", "--socket", sock_path.to_str().unwrap(), "--json"])
            .env("HOME", &home_dir)
            .env("XDG_CONFIG_HOME", &config_dir)
            .env("XDG_STATE_HOME", &state_dir)
            .env("XDG_RUNTIME_DIR", &run_dir)
            .output()
            .expect("run status --json");
        assert!(
            status_out.status.success(),
            "status --json failed: {}",
            String::from_utf8_lossy(&status_out.stderr)
        );
        let stdout_str = String::from_utf8(status_out.stdout).expect("utf8 stdout");
        assert!(stdout_str.contains("\"schemaVersion\": 1"));
        assert!(stdout_str.contains("\"claude\""));
        assert!(stdout_str.contains("\"codex\""));
        assert!(stdout_str.contains("\"antigravity\""));

        // Send SIGTERM to the daemon and verify clean exit + socket removal.
        let pid = rustix::process::Pid::from_raw(daemon.id() as i32).expect("valid pid");
        rustix::process::kill_process(pid, rustix::process::Signal::TERM).expect("send SIGTERM");

        let status = daemon.wait().expect("wait daemon");
        assert!(
            status.success(),
            "daemon did not exit 0 on SIGTERM: {status}"
        );
        assert!(
            !sock_path.exists(),
            "daemon must remove its socket file on clean shutdown"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn daemon_write_creates_private_plist_in_temporary_home() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let home_dir = temp_dir.path().join("home");
        let state_dir = temp_dir.path().join("state");
        std::fs::create_dir_all(&home_dir).expect("create home");
        std::fs::create_dir_all(&state_dir).expect("create state");

        let bin = cargo_bin("ariadusage");
        let out = Command::new(&bin)
            .args(["daemon", "write", "--binary", "/usr/local/bin/ariadusage"])
            .env("HOME", &home_dir)
            .env("XDG_STATE_HOME", &state_dir)
            .output()
            .expect("daemon write");
        assert!(
            out.status.success(),
            "daemon write failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let plist_path: PathBuf = home_dir
            .join("Library")
            .join("LaunchAgents")
            .join("io.github.bavanchun.ariadusage.daemon.plist");
        assert!(plist_path.exists(), "plist was not written to temp HOME");
        let mode = std::fs::metadata(&plist_path)
            .expect("plist metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        let contents = std::fs::read_to_string(&plist_path).expect("read plist");
        assert!(contents.contains("<string>io.github.bavanchun.ariadusage.daemon</string>"));
        assert!(contents.contains("<string>/usr/local/bin/ariadusage</string>"));
        assert!(contents.contains("<string>daemon</string>"));
        assert!(!contents.contains("EnvironmentVariables"));
    }
}
