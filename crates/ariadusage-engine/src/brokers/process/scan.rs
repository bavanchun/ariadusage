// Ported from CodexBar Sources/CodexBarCore/Host/PTY/StreamScanBuffer.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use zeroize::{Zeroize, Zeroizing};

/// Keeps only the overlap needed to find a needle split between read chunks.
pub struct StreamScanBuffer {
    max_needle: usize,
    tail: Zeroizing<Vec<u8>>,
}

impl StreamScanBuffer {
    pub fn new(max_needle: usize) -> Self {
        Self {
            max_needle,
            tail: Zeroizing::new(Vec::new()),
        }
    }

    /// Returns the previous tail plus this chunk and retains at most `max_needle - 1` bytes.
    pub fn append(&mut self, bytes: &[u8]) -> Zeroizing<Vec<u8>> {
        if bytes.is_empty() {
            return Zeroizing::new(Vec::new());
        }
        let mut combined = Zeroizing::new(std::mem::take(&mut *self.tail));
        combined.extend_from_slice(bytes);
        let retained = self.max_needle.saturating_sub(1).min(combined.len());
        self.tail
            .extend_from_slice(&combined[combined.len() - retained..]);
        combined
    }

    pub fn reset(&mut self) {
        self.tail.zeroize();
    }

    /// Lowercases ASCII bytes while leaving all non-ASCII bytes unchanged.
    pub fn lowercased_ascii(bytes: &[u8]) -> Zeroizing<Vec<u8>> {
        let mut lowered = Zeroizing::new(bytes.to_vec());
        for byte in lowered.iter_mut() {
            if byte.is_ascii_uppercase() {
                *byte = byte.to_ascii_lowercase();
            }
        }
        lowered
    }
}

impl std::fmt::Debug for StreamScanBuffer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StreamScanBuffer")
            .field("max_needle", &self.max_needle)
            .field("retained_bytes", &self.tail.len())
            .finish()
    }
}
