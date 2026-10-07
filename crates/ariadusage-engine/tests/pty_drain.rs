// Ported from CodexBar Sources/CodexBarCore/Host/PTY/StreamScanBuffer.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Host/PTY/TTYCommandRunner.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StreamScanBufferTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/TTYCommandRunnerTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]

use std::io::{self, Read};

use ariadusage_engine::brokers::process::{DrainClassification, StreamScanBuffer, drain};

#[test]
// CodexBar: StreamScanBufferTests.swift:7
fn split_markers_remain_searchable_across_read_chunks() {
    let mut scan = StreamScanBuffer::new(3);
    let first = scan.append(b"abc");
    assert!(!first.windows(3).any(|window| window == b"cde"));
    let second = scan.append(b"def");
    assert!(second.windows(3).any(|window| window == b"cde"));
}

#[test]
// CodexBar: StreamScanBufferTests.swift:22
fn scan_buffer_retains_only_max_needle_overlap() {
    let mut scan = StreamScanBuffer::new(4);
    let _ = scan.append(b"012345");
    assert!(scan.append(&[]).is_empty());
    let next = scan.append(b"xyz");
    assert_eq!(next.as_slice(), b"345xyz");
    assert!(format!("{scan:?}").contains("retained_bytes: 3"));
}

#[test]
// CodexBar: TTYCommandRunnerTests.swift:460
fn drain_classifies_read_data() {
    assert_eq!(drain::classify(Ok(17)), DrainClassification::Data(17));
}

#[test]
// CodexBar: TTYCommandRunnerTests.swift:488
fn drain_classifies_zero_as_closed() {
    assert_eq!(drain::classify(Ok(0)), DrainClassification::Closed);
}

#[test]
// CodexBar: TTYCommandRunnerTests.swift:517
fn drain_classifies_would_block_as_retryable() {
    assert_eq!(
        drain::classify(Err(io::Error::from(io::ErrorKind::WouldBlock))),
        DrainClassification::WouldBlock
    );
}

#[test]
// CodexBar: TTYCommandRunnerTests.swift:533
fn drain_classifies_eagain_as_retryable() {
    assert_eq!(
        drain::classify(Err(io::Error::from_raw_os_error(11))),
        DrainClassification::WouldBlock
    );
}

#[test]
// CodexBar: TTYCommandRunnerTests.swift:561
fn drain_classifies_interrupted_as_retryable() {
    assert_eq!(
        drain::classify(Err(io::Error::from(io::ErrorKind::Interrupted))),
        DrainClassification::WouldBlock
    );
}

#[test]
// CodexBar: TTYCommandRunnerTests.swift:588
fn drain_classifies_eio_as_closed() {
    assert_eq!(
        drain::classify(Err(io::Error::from_raw_os_error(5))),
        DrainClassification::Closed
    );
}

#[test]
// CodexBar: TTYCommandRunnerTests.swift:598
fn drain_classifies_other_errors_as_closed() {
    assert_eq!(
        drain::classify(Err(io::Error::other("synthetic reader failure"))),
        DrainClassification::Closed
    );
}

#[test]
// CodexBar: TTYCommandRunnerTests.swift:653
fn lowercase_only_changes_ascii_letters() {
    let lowered = StreamScanBuffer::lowercased_ascii(&[b'A', b'Z', 0xc3, 0x89]);
    assert_eq!(lowered.as_slice(), &[b'a', b'z', 0xc3, 0x89]);
}

#[test]
fn fake_reader_results_follow_the_drain_classifier() {
    let mut reader = io::Cursor::new(b"pty-data");
    let mut bytes = [0_u8; 16];
    let count = reader.read(&mut bytes).unwrap();
    assert_eq!(drain::classify(Ok(count)), DrainClassification::Data(8));
    let eof = reader.read(&mut bytes).unwrap();
    assert_eq!(drain::classify(Ok(eof)), DrainClassification::Closed);
}
