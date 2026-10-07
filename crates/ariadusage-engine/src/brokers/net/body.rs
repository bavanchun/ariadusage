use std::fmt;

use zeroize::Zeroizing;

/// Request payload, with secret payloads zeroized when the broker releases them.
#[derive(Default)]
pub enum NetBody {
    #[default]
    Empty,
    Plain(Vec<u8>),
    Secret(Zeroizing<Vec<u8>>),
}

impl NetBody {
    pub fn secret(bytes: impl Into<Vec<u8>>) -> Self {
        Self::Secret(Zeroizing::new(bytes.into()))
    }

    pub(crate) fn into_request_body(self) -> Option<reqwest::Body> {
        match self {
            Self::Empty => None,
            Self::Plain(bytes) => Some(reqwest::Body::from(bytes)),
            Self::Secret(bytes) => Some(reqwest::Body::from(bytes::Bytes::from_owner(
                SecretBodyOwner(bytes),
            ))),
        }
    }

    fn debug_summary(&self) -> (&'static str, usize) {
        match self {
            Self::Empty => ("empty", 0),
            Self::Plain(bytes) => ("plain", bytes.len()),
            Self::Secret(bytes) => ("secret", bytes.len()),
        }
    }
}

struct SecretBodyOwner(Zeroizing<Vec<u8>>);

impl AsRef<[u8]> for SecretBodyOwner {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl fmt::Debug for NetBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, len) = self.debug_summary();
        f.debug_struct("NetBody")
            .field("kind", &kind)
            .field("len", &len)
            .finish()
    }
}
