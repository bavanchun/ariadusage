//! Frame codec enforcing 1 MiB framing boundaries in both directions.

use std::fmt;

use tokio_util::bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder, LinesCodec, LinesCodecError};

/// Maximum allowed length of a single frame in bytes (1 MiB).
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Errors produced during frame encoding or decoding.
#[derive(Debug)]
pub enum IpcCodecError {
    /// Inbound frame exceeded the maximum allowed size.
    MaxLineLengthExceeded,
    /// Outbound frame exceeded the maximum allowed size before sending.
    FrameTooLarge { size: usize, max: usize },
    /// Underlying I/O error.
    Io(std::io::Error),
}

impl fmt::Display for IpcCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MaxLineLengthExceeded => {
                write!(
                    f,
                    "inbound frame exceeded maximum allowed size of {MAX_FRAME_BYTES} bytes"
                )
            }
            Self::FrameTooLarge { size, max } => {
                write!(
                    f,
                    "outbound frame of {size} bytes exceeds maximum allowed size of {max} bytes"
                )
            }
            Self::Io(err) => write!(f, "i/o error: {err}"),
        }
    }
}

impl std::error::Error for IpcCodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<LinesCodecError> for IpcCodecError {
    fn from(err: LinesCodecError) -> Self {
        match err {
            LinesCodecError::MaxLineLengthExceeded => Self::MaxLineLengthExceeded,
            LinesCodecError::Io(err) => Self::Io(err),
        }
    }
}

impl From<std::io::Error> for IpcCodecError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// Newline-delimited JSON frame codec enforcing a strict 1 MiB cap on both decode and encode.
#[derive(Debug, Clone)]
pub struct IpcCodec {
    lines: LinesCodec,
    max_length: usize,
}

impl Default for IpcCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl IpcCodec {
    /// Construct a new [`IpcCodec`] capped at [`MAX_FRAME_BYTES`].
    pub fn new() -> Self {
        Self::new_with_max_length(MAX_FRAME_BYTES)
    }

    /// Construct a new [`IpcCodec`] with a custom max length (primarily for testing).
    pub fn new_with_max_length(max_length: usize) -> Self {
        Self {
            lines: LinesCodec::new_with_max_length(max_length),
            max_length,
        }
    }

    /// Return the maximum allowed frame length in bytes.
    #[must_use]
    pub fn max_length(&self) -> usize {
        self.max_length
    }
}

impl Decoder for IpcCodec {
    type Item = String;
    type Error = IpcCodecError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.lines.decode(src).map_err(IpcCodecError::from)
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.lines.decode_eof(src).map_err(IpcCodecError::from)
    }
}

impl<T: AsRef<str>> Encoder<T> for IpcCodec {
    type Error = IpcCodecError;

    fn encode(&mut self, item: T, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let s = item.as_ref();
        if s.len() > self.max_length {
            return Err(IpcCodecError::FrameTooLarge {
                size: s.len(),
                max: self.max_length,
            });
        }
        self.lines.encode(s, dst).map_err(IpcCodecError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_normal_frame() {
        let mut codec = IpcCodec::new();
        let mut buf = BytesMut::new();

        codec.encode(r#"{"type":"getSnapshot"}"#, &mut buf).unwrap();
        assert_eq!(buf.as_ref(), b"{\"type\":\"getSnapshot\"}\n");

        let decoded = codec.decode(&mut buf).unwrap();
        assert_eq!(decoded.as_deref(), Some(r#"{"type":"getSnapshot"}"#));
        assert!(buf.is_empty());
    }

    #[test]
    fn encoder_refuses_frame_over_cap() {
        let max = 64;
        let mut codec = IpcCodec::new_with_max_length(max);
        let mut buf = BytesMut::new();

        // Exact cap succeeds
        let exact = "a".repeat(max);
        codec.encode(&exact, &mut buf).unwrap();
        assert_eq!(buf.len(), max + 1); // + newline
        buf.clear();

        // One byte over cap fails and writes no bytes to buffer
        let oversized = "a".repeat(max + 1);
        let err = codec.encode(&oversized, &mut buf).unwrap_err();
        assert!(matches!(
            err,
            IpcCodecError::FrameTooLarge { size: 65, max: 64 }
        ));
        assert!(buf.is_empty());
    }

    #[test]
    fn decoder_rejects_oversized_inbound_and_recovers() {
        let max = 32;
        let mut codec = IpcCodec::new_with_max_length(max);
        let mut buf = BytesMut::new();

        // Feed an oversized line followed by a valid line
        let mut stream = "x".repeat(max + 10);
        stream.push('\n');
        stream.push_str("{\"type\":\"valid\"}\n");
        buf.extend_from_slice(stream.as_bytes());

        // First line produces MaxLineLengthExceeded
        let res = codec.decode(&mut buf);
        assert!(matches!(res, Err(IpcCodecError::MaxLineLengthExceeded)));

        // Stream remains usable for subsequent frames
        let res2 = codec.decode(&mut buf).unwrap();
        assert_eq!(res2.as_deref(), Some("{\"type\":\"valid\"}"));
    }
}
