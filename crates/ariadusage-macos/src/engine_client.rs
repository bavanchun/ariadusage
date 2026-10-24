//! Connection to the engine's Unix socket over IPC v1.
//!
//! A background thread owns the read side: it connects, performs the
//! handshake, subscribes to snapshots and forwards every pushed snapshot to
//! the caller. When the engine goes away it reconnects with the same backoff
//! the Omarchy plugin uses. Intents such as refresh are written from the
//! caller's thread through a shared write handle.

use std::io::{self, BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use ariadusage_protocol::MAX_FRAME_BYTES;
use ariadusage_protocol::ids::{ActionId, ProviderId, SettingId};
use ariadusage_protocol::ipc::{
    ClientInfo, ClientMessage, PROTOCOL_V1, ServerMessage, Topic, parse_server_message,
};
use ariadusage_protocol::settings::{SettingsPage, SettingsScope};
use ariadusage_protocol::snapshot::EngineSnapshot;

/// Name this client reports in its handshake.
pub const CLIENT_NAME: &str = "ariadusage-macos";

/// Delays between reconnect attempts; the last one repeats.
pub const RECONNECT_BACKOFF: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
];

/// Timeout for outbound client writes so a stalled engine cannot freeze the UI.
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// What the engine connection reports to the app.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    /// The handshake succeeded.
    Connected { engine_version: String },
    /// The engine pushed a new snapshot.
    Snapshot(Box<EngineSnapshot>),
    /// The engine returned a settings page.
    Settings(Box<SettingsPage>),
    /// The connection failed or closed; a reconnect is scheduled.
    Disconnected,
}

/// Handle used to send intents to the engine.
#[derive(Clone, Default)]
pub struct EngineClient {
    writer: Arc<Mutex<Option<UnixStream>>>,
}

impl EngineClient {
    /// Starts the connection thread. `on_event` runs on that thread.
    pub fn spawn<F>(socket_path: PathBuf, on_event: F) -> io::Result<Self>
    where
        F: Fn(EngineEvent) + Send + 'static,
    {
        Self::spawn_with_backoff(socket_path, &RECONNECT_BACKOFF, on_event)
    }

    /// Starts the connection thread with a custom reconnect backoff schedule.
    /// An empty slice defaults to [`RECONNECT_BACKOFF`].
    pub fn spawn_with_backoff<F>(
        socket_path: PathBuf,
        backoff: &'static [Duration],
        on_event: F,
    ) -> io::Result<Self>
    where
        F: Fn(EngineEvent) + Send + 'static,
    {
        let backoff = if backoff.is_empty() {
            &RECONNECT_BACKOFF[..]
        } else {
            backoff
        };
        let client = Self::default();
        let writer = Arc::clone(&client.writer);
        thread::Builder::new()
            .name("engine-client".into())
            .spawn(move || run(&socket_path, &writer, backoff, &on_event))?;
        Ok(client)
    }

    /// Asks the engine to refresh one provider, or all when `provider` is
    /// `None`. Does nothing while disconnected.
    pub fn refresh(&self, provider: Option<ProviderId>) {
        self.send(&ClientMessage::Refresh { provider, id: None });
    }

    /// Requests the settings descriptors for `scope`.
    pub fn get_settings(&self, scope: SettingsScope) {
        self.send(&ClientMessage::GetSettings { scope, id: None });
    }

    /// Updates a non-secret setting in the engine.
    pub fn set_setting(&self, id: SettingId, value: serde_json::Value) {
        self.send(&ClientMessage::SetSetting { id, value });
    }

    /// Runs a maintenance or provider action in the engine.
    pub fn run_action(&self, id: ActionId, confirm: Option<bool>) {
        self.send(&ClientMessage::RunAction { id, confirm });
    }

    fn send(&self, message: &ClientMessage) {
        send_to_writer(&self.writer, message);
    }
}

fn send_to_writer(writer: &Mutex<Option<UnixStream>>, message: &ClientMessage) {
    let mut guard = writer.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(stream) = guard.as_mut()
        && write_message(stream, message).is_err()
    {
        let _ = stream.shutdown(Shutdown::Both);
        *guard = None;
    }
}

fn run<F>(
    socket_path: &PathBuf,
    writer: &Mutex<Option<UnixStream>>,
    backoff: &[Duration],
    on_event: &F,
) where
    F: Fn(EngineEvent),
{
    let mut attempt = 0usize;
    loop {
        let mut got_snapshot = false;
        let _ = session(socket_path, writer, on_event, &mut got_snapshot);
        // A session that got as far as a snapshot resets the backoff, even if
        // it later ended with an I/O error.
        if got_snapshot {
            attempt = 0;
        }
        *writer.lock().unwrap_or_else(PoisonError::into_inner) = None;
        on_event(EngineEvent::Disconnected);
        let delay = backoff[attempt.min(backoff.len() - 1)];
        attempt += 1;
        thread::sleep(delay);
    }
}

/// Runs one connection until it closes. Sets `*got_snapshot` when any
/// snapshot arrives.
fn session<F>(
    socket_path: &PathBuf,
    writer: &Mutex<Option<UnixStream>>,
    on_event: &F,
    got_snapshot: &mut bool,
) -> io::Result<()>
where
    F: Fn(EngineEvent),
{
    let stream = UnixStream::connect(socket_path)?;
    let mut write_half = stream.try_clone()?;
    write_half.set_write_timeout(Some(WRITE_TIMEOUT))?;
    write_message(&mut write_half, &hello())?;
    write_message(
        &mut write_half,
        &ClientMessage::Subscribe {
            topics: vec![Topic::Snapshot, Topic::Settings],
            id: None,
        },
    )?;
    write_message(&mut write_half, &ClientMessage::GetSnapshot { id: None })?;
    write_message(
        &mut write_half,
        &ClientMessage::GetSettings {
            scope: SettingsScope::App,
            id: None,
        },
    )?;
    *writer.lock().unwrap_or_else(PoisonError::into_inner) = Some(write_half);

    let mut reader = BufReader::new(stream);
    while let Some(frame) = read_frame(&mut reader, MAX_FRAME_BYTES)? {
        let Ok(text) = std::str::from_utf8(&frame) else {
            continue;
        };
        match parse_server_message(text) {
            Ok(ServerMessage::Welcome { engine_version, .. }) => {
                on_event(EngineEvent::Connected { engine_version });
            }
            Ok(ServerMessage::Snapshot { snapshot }) => {
                *got_snapshot = true;
                on_event(EngineEvent::Snapshot(snapshot));
            }
            Ok(ServerMessage::SettingsChanged { scope }) => {
                send_to_writer(writer, &ClientMessage::GetSettings { scope, id: None });
            }
            Ok(ServerMessage::Response {
                ok: Some(true),
                payload: Some(payload),
                ..
            }) => {
                if let Ok(page) = serde_json::from_value::<SettingsPage>(payload) {
                    on_event(EngineEvent::Settings(Box::new(page)));
                }
            }
            // Other responses and notices are not used yet.
            Ok(_) | Err(_) => {}
        }
    }
    Ok(())
}

fn hello() -> ClientMessage {
    ClientMessage::Hello {
        protocols: vec![PROTOCOL_V1.to_string()],
        client: ClientInfo::new(CLIENT_NAME, env!("CARGO_PKG_VERSION")),
        id: None,
    }
}

fn write_message(stream: &mut UnixStream, message: &ClientMessage) -> io::Result<()> {
    let mut line = serde_json::to_vec(message).map_err(io::Error::other)?;
    line.push(b'\n');
    stream.write_all(&line)
}

/// Reads one newline-terminated frame whose content is at most `max` bytes
/// (excluding `\r` and `\n`). An oversized frame is skipped up to its newline.
/// Returns `None` at end of stream.
pub fn read_frame<R: BufRead>(reader: &mut R, max: usize) -> io::Result<Option<Vec<u8>>> {
    let raw_cap = max.saturating_add(2);
    loop {
        let mut frame = Vec::new();
        let mut oversized = false;
        loop {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                // End of stream: a partial last line is not a frame.
                return Ok(None);
            }
            let newline = available.iter().position(|&b| b == b'\n');
            let take = newline.map_or(available.len(), |i| i + 1);
            if !oversized {
                if frame.len() + take > raw_cap {
                    oversized = true;
                    frame = Vec::new();
                } else {
                    frame.extend_from_slice(&available[..take]);
                }
            }
            reader.consume(take);
            if newline.is_some() {
                break;
            }
        }
        if oversized {
            continue;
        }
        frame.pop();
        if frame.last() == Some(&b'\r') {
            frame.pop();
        }
        if frame.len() > max {
            continue;
        }
        return Ok(Some(frame));
    }
}

/// Maximum byte length of a Unix domain socket `sun_path` buffer on macOS (104 bytes including NUL).
pub const SUN_PATH_MAX_BYTES: usize = 104;

/// Relative path of the engine IPC socket inside the runtime directory.
pub const ENGINE_SOCKET_REL_PATH: &str = "ariadusage/engine.sock";

fn canonicalize_macos_var_prefix(path: &std::path::Path) -> PathBuf {
    if path == std::path::Path::new("/var") {
        return PathBuf::from("/private/var");
    }
    if let Ok(rest) = path.strip_prefix("/var") {
        return std::path::Path::new("/private/var").join(rest);
    }
    path.to_path_buf()
}

/// Resolves the default macOS engine socket path (`<runtime_dir>/ariadusage/engine.sock`) from an
/// optional `XDG_RUNTIME_DIR` string and a `confstr(_CS_DARWIN_USER_TEMP_DIR)` fallback, enforcing
/// the `< 104`-byte `sun_path` limit.
pub fn resolve_default_socket_path_with(
    xdg_runtime_dir: Option<&str>,
    darwin_temp_dir: impl FnOnce() -> Option<PathBuf>,
) -> Result<PathBuf, String> {
    let raw_dir = xdg_runtime_dir
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            let dir = darwin_temp_dir()?;
            let trimmed = dir.to_str().map(|s| s.trim_end_matches('/'));
            let normalized = match trimmed {
                Some(s) if !s.is_empty() => PathBuf::from(s),
                _ => dir,
            };
            normalized.is_absolute().then_some(normalized)
        })
        .ok_or_else(|| "could not resolve macOS runtime directory".to_string())?;

    let runtime_dir = canonicalize_macos_var_prefix(&raw_dir);
    let sock = runtime_dir.join(ENGINE_SOCKET_REL_PATH);

    use std::os::unix::ffi::OsStrExt;
    let len = sock.as_os_str().as_bytes().len();
    if len >= SUN_PATH_MAX_BYTES {
        return Err(format!(
            "engine socket path is {len} bytes, which is not strictly less than the {SUN_PATH_MAX_BYTES}-byte sun_path limit: {}",
            sock.display()
        ));
    }
    Ok(sock)
}

/// Resolves the default macOS engine socket path using `XDG_RUNTIME_DIR` or
/// `confstr(_CS_DARWIN_USER_TEMP_DIR)` (via `/usr/bin/getconf DARWIN_USER_TEMP_DIR`).
pub fn default_socket_path() -> Result<PathBuf, String> {
    let xdg = std::env::var("XDG_RUNTIME_DIR").ok();
    resolve_default_socket_path_with(xdg.as_deref(), || {
        let output = std::process::Command::new("/usr/bin/getconf")
            .arg("DARWIN_USER_TEMP_DIR")
            .env_clear()
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let raw = std::str::from_utf8(&output.stdout).ok()?.trim();
        let trimmed = raw.trim_end_matches('/');
        if trimmed.is_empty() || trimmed == "undefined" {
            return None;
        }
        Some(PathBuf::from(trimmed))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_default_socket_path_canonicalizes_var_and_checks_sun_path() {
        let sock =
            resolve_default_socket_path_with(None, || Some(PathBuf::from("/var/folders/ab/cd/T/")))
                .expect("resolve darwin temp dir");
        assert_eq!(
            sock,
            PathBuf::from("/private/var/folders/ab/cd/T/ariadusage/engine.sock")
        );

        let xdg_sock = resolve_default_socket_path_with(Some("/var/folders/xy/z/T"), || None)
            .expect("resolve xdg");
        assert_eq!(
            xdg_sock,
            PathBuf::from("/private/var/folders/xy/z/T/ariadusage/engine.sock")
        );

        let long_dir = format!("/{}", "a".repeat(90));
        let err = resolve_default_socket_path_with(Some(&long_dir), || None)
            .expect_err("oversized socket path must fail");
        assert!(err.contains("sun_path limit"));
    }
}
