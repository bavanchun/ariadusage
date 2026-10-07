#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserError {
    Suppressed,
    NoBrowserSession,
    Locked,
    Dismissed,
    Unsupported,
    Io,
    Malformed,
}

impl std::fmt::Display for BrowserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Suppressed => "browser access suppressed",
            Self::NoBrowserSession => "no browser session found",
            Self::Locked => "browser key is locked",
            Self::Dismissed => "browser key access was dismissed",
            Self::Unsupported => "browser import unsupported on this platform",
            Self::Io => "browser data could not be read",
            Self::Malformed => "browser data is malformed",
        })
    }
}

impl std::error::Error for BrowserError {}

impl From<std::io::Error> for BrowserError {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}

impl From<rusqlite::Error> for BrowserError {
    fn from(error: rusqlite::Error) -> Self {
        match error.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseCorrupt)
            | Some(rusqlite::ErrorCode::NotADatabase) => Self::Malformed,
            _ => Self::Io,
        }
    }
}
