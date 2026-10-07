// Ported from CodexBar Tests/CodexBarTests/BoundedOutputBufferTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
use ariadusage_engine::brokers::process::{BoundedLineBuffer, BoundedOutputBuffer};

#[test]
// CodexBar: BoundedOutputBufferTests.swift:7
fn output_buffer_rejects_data_beyond_its_byte_limit() {
    let mut buffer = BoundedOutputBuffer::new(4);

    assert!(buffer.append(b"abcd"));
    assert!(!buffer.append(b"e"));
    assert_eq!(buffer.as_slice(), b"abcd");
}

#[test]
// CodexBar: BoundedOutputBufferTests.swift:19
fn line_buffer_rejects_an_unterminated_line_beyond_its_byte_limit() {
    let mut buffer = BoundedLineBuffer::new(4);

    let first = buffer.append_and_drain_lines(b"abcd");
    let overflow = buffer.append_and_drain_lines(b"e");

    assert!(first.lines.is_empty());
    assert!(!first.did_exceed_limit);
    assert!(overflow.lines.is_empty());
    assert!(overflow.did_exceed_limit);
}

#[test]
// CodexBar: BoundedOutputBufferTests.swift:32
fn line_buffer_frees_completed_lines_before_accepting_more_output() {
    let mut buffer = BoundedLineBuffer::new(4);

    let first = buffer.append_and_drain_lines(b"a\n");
    let second = buffer.append_and_drain_lines(b"bcde");

    assert_eq!(
        first
            .lines
            .iter()
            .map(|line| line.as_slice())
            .collect::<Vec<_>>(),
        [b"a"]
    );
    assert!(!first.did_exceed_limit);
    assert!(!second.did_exceed_limit);
}

#[test]
// CodexBar: BoundedOutputBufferTests.swift:44
fn line_buffer_drains_a_completed_line_before_limiting_the_same_chunk_tail() {
    let mut buffer = BoundedLineBuffer::new(4);

    let partial = buffer.append_and_drain_lines(b"abc");
    let completed = buffer.append_and_drain_lines(b"d\nxy");

    assert!(!partial.did_exceed_limit);
    assert_eq!(
        completed
            .lines
            .iter()
            .map(|line| line.as_slice())
            .collect::<Vec<_>>(),
        [b"abcd"]
    );
    assert!(!completed.did_exceed_limit);
}

#[test]
// CodexBar: BoundedOutputBufferTests.swift:56
fn line_buffer_rejects_an_oversized_line_even_when_newline_arrives() {
    let mut buffer = BoundedLineBuffer::new(4);

    let _ = buffer.append_and_drain_lines(b"abc");
    let overflow = buffer.append_and_drain_lines(b"de\n");

    assert!(overflow.lines.is_empty());
    assert!(overflow.did_exceed_limit);
}
