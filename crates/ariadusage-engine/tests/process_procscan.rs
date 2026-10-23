// Ported from CodexBar Tests/CodexBarTests/DarwinProcessEnumeratorTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/PiProcessEnvironmentTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/ProcessOwnershipReaperTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(unix)]
use std::collections::BTreeMap;
use std::fs;

use ariadusage_engine::brokers::process::{
    parse_marker_environment, parse_procargs2_arguments, parse_procargs2_environment,
    parse_procargs2_marker, read_marker_environment,
};
use tempfile::tempdir;

const MARKER: &str = "ARIADUSAGE_PROBE_OWNER";
const SELECTOR_ENV: &[&str] = &["HOME", "OMP_PROFILE"];

#[test]
// CodexBar: PiProcessEnvironmentTests.swift:19, :50
fn nul_environment_parser_is_strict_and_keeps_only_the_marker() {
    let payload = b"HOME=/synthetic/home\0ARIADUSAGE_PROBE_OWNER=fixture\0OTHER=value\0";
    assert_eq!(
        parse_marker_environment(payload, MARKER).map(|value| value.to_vec()),
        Some(b"fixture".to_vec())
    );
    assert_eq!(parse_marker_environment(b"", MARKER), None);
    assert_eq!(
        parse_marker_environment(b"HOME=/synthetic/home\0", MARKER),
        None
    );
    assert_eq!(parse_marker_environment(b"OTHER=fixture\0", MARKER), None);
    assert_eq!(
        parse_marker_environment(b"ARIADUSAGE_PROBE_OWNER=fixture", MARKER),
        None
    );
    assert_eq!(
        parse_marker_environment(
            b"ARIADUSAGE_PROBE_OWNER=fixture\0ARIADUSAGE_PROBE_OWNER=other\0",
            MARKER
        ),
        None
    );
}

#[test]
// CodexBar: PiProcessEnvironmentTests.swift:63, ProcessOwnershipReaperTests.swift:172
fn marker_reader_uses_the_exact_entry_and_fake_proc_root() {
    let root = tempdir().unwrap();
    let process = root.path().join("101");
    fs::create_dir_all(&process).unwrap();
    fs::write(
        process.join("environ"),
        b"OTHER=ARIADUSAGE_PROBE_OWNER=not-the-marker\0ARIADUSAGE_PROBE_OWNER=fixture\0",
    )
    .unwrap();

    assert_eq!(
        read_marker_environment(root.path(), 101, MARKER).map(|value| value.to_vec()),
        Some(b"fixture".to_vec())
    );
    fs::write(process.join("environ"), b"ARIADUSAGE_PROBE_OWNER=fixture").unwrap();
    assert_eq!(read_marker_environment(root.path(), 101, MARKER), None);
    assert_eq!(read_marker_environment(root.path(), 102, MARKER), None);
    assert_eq!(read_marker_environment(root.path(), 0, MARKER), None);
    assert_eq!(read_marker_environment(root.path(), -1, MARKER), None);
}

#[test]
// CodexBar: ProcessOwnershipReaperTests.swift:188, :206
fn marker_parser_bounds_unrelated_values_and_rejects_malformed_records() {
    let unrelated = vec![b'x'; 4096];
    let mut payload = Vec::new();
    for _ in 0..128 {
        payload.extend_from_slice(b"UNRELATED=");
        payload.extend_from_slice(&unrelated);
        payload.push(0);
    }
    payload.extend_from_slice(b"ARIADUSAGE_PROBE_OWNER=fixture\0");

    assert_eq!(
        parse_marker_environment(&payload, MARKER).map(|value| value.to_vec()),
        Some(b"fixture".to_vec())
    );
    assert_eq!(
        parse_marker_environment(&payload[..payload.len() - 1], MARKER),
        None
    );
    let mut malformed = payload;
    malformed.extend_from_slice(b"broken-record\0");
    assert_eq!(parse_marker_environment(&malformed, MARKER), None);
    assert_eq!(
        parse_marker_environment(&vec![0; 1024 * 1024 + 1], MARKER),
        None
    );
}

#[test]
fn proc_scanner_uses_an_injected_root_for_uid_identity_and_descriptor_targets() {
    use std::os::unix::fs::symlink;

    use ariadusage_engine::brokers::process::{fd_targets, same_uid_processes};

    let root = tempdir().unwrap();
    let process = root.path().join("101");
    let descriptors = process.join("fd");
    fs::create_dir_all(&descriptors).unwrap();
    fs::write(
        process.join("status"),
        "Name:\tfixture\nState:\tS\nTgid:\t101\nPid:\t101\nPPid:\t1\nTracerPid:\t0\nUid:\t1000\t1000\t1000\t1000\nGid:\t1000\t1000\t1000\t1000\nFDSize:\t1\nGroups:\t1000\nThreads:\t1\nSigQ:\t0/0\nSigPnd:\t0000000000000000\nShdPnd:\t0000000000000000\nSigBlk:\t0000000000000000\nSigIgn:\t0000000000000000\nSigCgt:\t0000000000000000\nCapInh:\t0000000000000000\nCapPrm:\t0000000000000000\nCapEff:\t0000000000000000\n",
    )
    .unwrap();
    let mut fields = vec!["0"; 50];
    fields[0] = "S";
    fields[1] = "1";
    fields[2] = "101";
    fields[3] = "101";
    fields[19] = "987654";
    fs::write(
        process.join("stat"),
        format!("101 (fixture) {}\n", fields.join(" ")),
    )
    .unwrap();
    symlink("pipe:[123]", descriptors.join("0")).unwrap();

    #[cfg(target_os = "linux")]
    {
        let procfs_process = procfs::process::all_processes_with_root(root.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(procfs_process.pid, 101);
        assert_eq!(procfs_process.status().unwrap().ruid, 1000);
        assert_eq!(procfs_process.stat().unwrap().starttime, 987654);
    }

    let identities = same_uid_processes(root.path(), 1000);
    assert!(
        identities.contains(&ariadusage_engine::brokers::process::ProcessIdentity {
            pid: 101,
            start_ticks: 987654,
        })
    );
    assert!(
        fd_targets(root.path(), 101)
            .iter()
            .any(|target| target == "pipe:[123]")
    );
    assert!(same_uid_processes(root.path(), 9999).is_empty());
}

#[test]
// CodexBar: PiProcessEnvironmentTests.swift:55
fn marker_environment_reader_rejects_invalid_utf8_only_when_it_is_the_marker() {
    let invalid_marker = b"ARIADUSAGE_PROBE_OWNER=\xff\0";
    assert_eq!(
        parse_marker_environment(invalid_marker, MARKER)
            .unwrap()
            .as_slice(),
        b"\xff"
    );
    let unrelated_invalid = b"UNRELATED=\xff\0";
    assert_eq!(parse_marker_environment(unrelated_invalid, MARKER), None);
}

#[test]
// CodexBar: DarwinProcessEnumeratorTests.swift:11, :18, :25, :30, :83, :90
fn proc_args2_parser_preserves_argv_and_excludes_environment() {
    let data = proc_args_data(&["/usr/bin/tool", "--flag", "value"], &[]);
    assert_eq!(
        parse_procargs2_arguments(&data),
        Some(vec![
            "/usr/bin/tool".to_owned(),
            "--flag".to_owned(),
            "value".to_owned()
        ])
    );

    let zero_argc = proc_args_data(&[], &[]);
    assert_eq!(parse_procargs2_arguments(&zero_argc), Some(Vec::new()));

    assert_eq!(parse_procargs2_arguments(&[2, 0, 0]), None);

    let with_env = proc_args_data(&["/usr/bin/tool", "--flag"], &["SECRET=value", "HOME=/tmp"]);
    let command = parse_procargs2_arguments(&with_env).unwrap().join(" ");
    assert_eq!(command, "/usr/bin/tool --flag");
    assert!(!command.contains("SECRET"));
    assert!(!command.contains("HOME"));

    let embedded_empty = proc_args_data(&["/usr/bin/tool", "", "value"], &[]);
    assert_eq!(
        parse_procargs2_arguments(&embedded_empty),
        Some(vec![
            "/usr/bin/tool".to_owned(),
            String::new(),
            "value".to_owned()
        ])
    );

    let mut oversized_argc = proc_args_data(&["/usr/bin/tool", "value"], &[]);
    oversized_argc[..4].copy_from_slice(&3i32.to_le_bytes());
    assert_eq!(parse_procargs2_arguments(&oversized_argc), None);
}

#[test]
// CodexBar: DarwinProcessEnumeratorTests.swift:42, :58, :73
fn proc_args2_environment_and_marker_respect_terminators_and_apple_vectors() {
    let mut data = proc_args_data(
        &["/usr/local/bin/omp", "", "--profile", "work"],
        &[
            "HOME=/synthetic/home",
            "OMP_PROFILE=work",
            "ARIADUSAGE_PROBE_OWNER=fixture",
            "UNRELATED=value",
        ],
    );
    data.extend_from_slice(&[0, 0, 0]);

    assert_eq!(
        parse_procargs2_arguments(&data),
        Some(vec![
            "/usr/local/bin/omp".to_owned(),
            String::new(),
            "--profile".to_owned(),
            "work".to_owned()
        ])
    );
    assert_eq!(
        parse_procargs2_environment(&data, SELECTOR_ENV),
        Some(BTreeMap::from([
            ("HOME".to_owned(), "/synthetic/home".to_owned()),
            ("OMP_PROFILE".to_owned(), "work".to_owned()),
        ]))
    );
    assert_eq!(
        parse_procargs2_marker(&data, MARKER).map(|v| v.to_vec()),
        Some(b"fixture".to_vec())
    );

    let empty = proc_args_data(&["pi"], &[]);
    assert_eq!(parse_procargs2_environment(&empty, SELECTOR_ENV), None);
    assert_eq!(parse_procargs2_marker(&empty, MARKER), None);

    let mut padded_empty = empty.clone();
    padded_empty.extend_from_slice(&[0, 0]);
    assert_eq!(
        parse_procargs2_environment(&padded_empty, SELECTOR_ENV),
        None
    );

    let known_empty = proc_args_data(&["pi"], &["UNRELATED=value"]);
    assert_eq!(
        parse_procargs2_environment(&known_empty, SELECTOR_ENV),
        Some(BTreeMap::new())
    );

    let mut truncated = proc_args_data(&["pi"], &["HOME=/synthetic/home"]);
    truncated.pop();
    assert_eq!(
        parse_procargs2_arguments(&truncated),
        Some(vec!["pi".to_owned()])
    );
    assert_eq!(parse_procargs2_environment(&truncated, SELECTOR_ENV), None);

    let mut with_apple_vector = proc_args_data(
        &["pi"],
        &["HOME=/synthetic/process", "ARIADUSAGE_PROBE_OWNER=real"],
    );
    with_apple_vector.push(0);
    with_apple_vector.extend_from_slice(
        b"HOME=/synthetic/apple-vector\0ARIADUSAGE_PROBE_OWNER=forged\0ptr_munge=ignored\0",
    );
    assert_eq!(
        parse_procargs2_environment(&with_apple_vector, SELECTOR_ENV),
        Some(BTreeMap::from([(
            "HOME".to_owned(),
            "/synthetic/process".to_owned()
        )]))
    );
    assert_eq!(
        parse_procargs2_marker(&with_apple_vector, MARKER).map(|v| v.to_vec()),
        Some(b"real".to_vec())
    );
}

#[cfg(target_os = "macos")]
#[test]
// CodexBar: DarwinProcessEnumeratorTests.swift:150, :174
fn darwin_live_procscan_inspects_current_and_child_processes() {
    use std::io::{BufRead, BufReader};
    use std::path::Path;
    use std::process::{Command, Stdio};

    use ariadusage_engine::brokers::process::{
        fd_targets, process_children, process_group, process_identity, process_state, process_uid,
        same_uid_processes,
    };

    let proc_root = Path::new("/proc");
    let self_pid = std::process::id() as i32;
    let uid = rustix::process::getuid().as_raw();
    let self_identity = process_identity(proc_root, self_pid).expect("self identity");
    assert_eq!(self_identity.pid, self_pid);
    assert!(self_identity.start_ticks > 0);
    assert_eq!(process_uid(proc_root, self_pid), Some(uid));
    assert_eq!(
        process_group(proc_root, self_pid),
        Some(rustix::process::getpgrp().as_raw_pid())
    );
    assert!(matches!(
        process_state(proc_root, self_pid),
        Some('R' | 'S' | 'I')
    ));

    let mut child = Command::new(env!("CARGO_BIN_EXE_ariadusage-test-child"))
        .arg("ready-wait")
        .env(MARKER, "darwin-live-marker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn test child");
    let _child_stdin = child.stdin.take().expect("child stdin");
    let mut child_stdout = BufReader::new(child.stdout.take().expect("child stdout"));
    let mut line = String::new();
    child_stdout.read_line(&mut line).expect("read ready line");
    assert_eq!(line.trim(), "READY", "unexpected ready line: {line}");

    let child_pid = child.id() as i32;
    let child_identity = process_identity(proc_root, child_pid).expect("child identity");
    assert_eq!(child_identity.pid, child_pid);
    assert!(same_uid_processes(proc_root, uid).contains(&child_identity));
    assert!(process_children(proc_root, self_pid).contains(&child_pid));
    assert!(!fd_targets(proc_root, child_pid).is_empty());
    assert_eq!(
        read_marker_environment(proc_root, child_pid, MARKER).map(|v| v.to_vec()),
        Some(b"darwin-live-marker".to_vec())
    );

    let _ = child.kill();
    let _ = child.wait();
}

fn proc_args_data(arguments: &[&str], environment: &[&str]) -> Vec<u8> {
    let mut data = Vec::from((arguments.len() as i32).to_le_bytes());
    data.extend_from_slice(b"/usr/bin/exec\0\0\0");
    for value in arguments.iter().chain(environment.iter()) {
        data.extend_from_slice(value.as_bytes());
        data.push(0);
    }
    data
}
