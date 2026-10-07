use zeroize::{Zeroize, Zeroizing};

#[derive(Default)]
pub struct BoundedOutputBuffer {
    bytes: Zeroizing<Vec<u8>>,
    max_bytes: usize,
}

impl BoundedOutputBuffer {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            bytes: Zeroizing::new(Vec::with_capacity(max_bytes.min(64 * 1024))),
            max_bytes,
        }
    }

    pub fn append(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() > self.max_bytes.saturating_sub(self.bytes.len()) {
            return false;
        }
        self.bytes.extend_from_slice(bytes);
        true
    }

    pub fn as_slice(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    pub fn into_bytes(self) -> Zeroizing<Vec<u8>> {
        self.bytes
    }
}

#[derive(Default)]
pub struct BoundedLineBuffer {
    pending: Zeroizing<Vec<u8>>,
    max_bytes: usize,
}

pub struct LineDrain {
    pub lines: Vec<Zeroizing<Vec<u8>>>,
    pub did_exceed_limit: bool,
}

impl BoundedLineBuffer {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            pending: Zeroizing::new(Vec::with_capacity(max_bytes.min(64 * 1024))),
            max_bytes,
        }
    }

    pub fn append_and_drain_lines(&mut self, bytes: &[u8]) -> LineDrain {
        let mut lines = Vec::new();
        let mut start = 0;

        for (index, byte) in bytes.iter().enumerate() {
            if *byte != b'\n' {
                continue;
            }
            let segment = &bytes[start..index];
            if segment.len() > self.max_bytes.saturating_sub(self.pending.len()) {
                self.pending.zeroize();
                return LineDrain {
                    lines,
                    did_exceed_limit: true,
                };
            }
            self.pending.extend_from_slice(segment);
            lines.push(Zeroizing::new(std::mem::take(&mut *self.pending)));
            start = index + 1;
        }

        let tail = &bytes[start..];
        if tail.len() > self.max_bytes.saturating_sub(self.pending.len()) {
            self.pending.zeroize();
            return LineDrain {
                lines,
                did_exceed_limit: true,
            };
        }
        self.pending.extend_from_slice(tail);
        LineDrain {
            lines,
            did_exceed_limit: false,
        }
    }
}
