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
use ariadusage_protocol::ids::ProviderId;
use ariadusage_protocol::ipc::{
    ClientInfo, ClientMessage, PROTOCOL_V1, ServerMessage, Topic, parse_server_message,
};
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

    fn send(&self, message: &ClientMessage) {
        let mut guard = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(stream) = guard.as_mut()
            && write_message(stream, message).is_err()
        {
            let _ = stream.shutdown(Shutdown::Both);
            *guard = None;
        }
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
            topics: vec![Topic::Snapshot],
            id: None,
        },
    )?;
    write_message(&mut write_half, &ClientMessage::GetSnapshot { id: None })?;
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
            // Responses, notices and settings pushes are not used yet.
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
