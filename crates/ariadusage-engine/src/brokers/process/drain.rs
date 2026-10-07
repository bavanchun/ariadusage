// Ported from CodexBar Sources/CodexBarCore/Host/PTY/TTYCommandRunner.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainClassification {
    Data(usize),
    WouldBlock,
    Closed,
}

/// Classifies one non-blocking PTY read result without retaining its contents.
pub fn classify(result: io::Result<usize>) -> DrainClassification {
    match result {
        Ok(0) => DrainClassification::Closed,
        Ok(count) => DrainClassification::Data(count),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            DrainClassification::WouldBlock
        }
        Err(error) if error.raw_os_error() == Some(5) => DrainClassification::Closed,
        Err(_) => DrainClassification::Closed,
    }
}
