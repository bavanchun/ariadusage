// Ported from CodexBar Tests/CodexBarTests/PiProcessEnvironmentTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/ProcessOwnershipReaperTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]
use std::fs;

use ariadusage_engine::brokers::process::{parse_marker_environment, read_marker_environment};
use tempfile::tempdir;

const MARKER: &str = "ARIADUSAGE_PROBE_OWNER";

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
// CodexBar: PiProcessEnvironmentTests.swift:63, ProcessOwnershipReaperTests.swift:171
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
// CodexBar: ProcessOwnershipReaperTests.swift:187, :205
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

    let procfs_process = procfs::process::all_processes_with_root(root.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(procfs_process.pid, 101);
    assert_eq!(procfs_process.status().unwrap().ruid, 1000);
    assert_eq!(procfs_process.stat().unwrap().starttime, 987654);

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
