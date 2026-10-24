//! IPC v1 Unix domain socket server for Linux and macOS.
//!
//! Enforces:
//! - Socket path `< 104` bytes (`SUN_PATH_MAX_BYTES`).
//! - Parent directory mode `0700`, owned by the current effective UID, not a symlink.
//! - Socket file mode `0600`.
//! - Peer credential verification on every accepted stream (`SO_PEERCRED` on Linux,
//!   `getpeereid` / `LOCAL_PEEREPID` on macOS via `tokio::net::UnixStream::peer_cred`),
//!   rejecting any peer whose UID does not match the engine's effective UID.
//! - 1 MiB framing boundary in both directions via [`IpcCodec`].

#[cfg(unix)]
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
#[cfg(unix)]
use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::path::{Path, PathBuf};

#[cfg(unix)]
use ariadusage_protocol::codec::{IpcCodec, IpcCodecError};
use ariadusage_protocol::ids::{ActionId, ProviderId, SettingId};
#[cfg(unix)]
use ariadusage_protocol::ipc::{
    ClientMessage, ErrorCode, Topic, negotiate_protocol, parse_client_message,
};
use ariadusage_protocol::ipc::{IpcError, ServerMessage};
use ariadusage_protocol::settings::{SettingsPage, SettingsScope};
use ariadusage_protocol::snapshot::EngineSnapshot;
#[cfg(unix)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;
#[cfg(unix)]
use tokio::sync::watch;
#[cfg(unix)]
use tokio::task::JoinSet;
#[cfg(unix)]
use tokio_util::bytes::BytesMut;
#[cfg(unix)]
use tokio_util::codec::{Decoder, Encoder};

use crate::paths::PathError;
#[cfg(unix)]
use crate::paths::validate_socket_path_len;

/// Errors encountered while starting or authenticating on the IPC v1 Unix socket server.
#[derive(Debug, thiserror::Error)]
pub enum IpcServerError {
    #[error(transparent)]
    Path(#[from] PathError),

    #[error("socket path has no parent directory: {0}")]
    MissingParentDirectory(std::path::PathBuf),

    #[error("socket parent directory failed ownership, permission, or symlink check: {0}")]
    UntrustedParentDirectory(std::path::PathBuf),

    #[error("rejected IPC peer with UID {peer_uid} (expected engine EUID {expected_euid})")]
    UntrustedPeerUid { peer_uid: u32, expected_euid: u32 },

    #[error("i/o error on IPC socket server: {0}")]
    Io(#[from] std::io::Error),
}

/// Verified Unix domain socket peer identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerIdentity {
    pub uid: u32,
    pub gid: u32,
    pub pid: Option<i32>,
}

/// Verifies that `peer_uid` matches `expected_euid`, rejecting connections from any other UID.
pub const fn verify_peer_uid(peer_uid: u32, expected_euid: u32) -> Result<(), IpcServerError> {
    if peer_uid == expected_euid {
        Ok(())
    } else {
        Err(IpcServerError::UntrustedPeerUid {
            peer_uid,
            expected_euid,
        })
    }
}

/// Extracts OS peer credentials from `stream` (`SO_PEERCRED` on Linux, `getpeereid` /
/// `LOCAL_PEEREPID` on macOS) and verifies that the peer UID matches `expected_euid`.
#[cfg(unix)]
pub fn verify_unix_peer_with_euid(
    stream: &UnixStream,
    expected_euid: u32,
) -> Result<PeerIdentity, IpcServerError> {
    let cred = stream.peer_cred()?;
    let peer_uid = cred.uid();
    verify_peer_uid(peer_uid, expected_euid)?;
    Ok(PeerIdentity {
        uid: peer_uid,
        gid: cred.gid(),
        pid: cred.pid(),
    })
}

/// Verifies that the peer of `stream` has the same UID as the current process's effective UID.
#[cfg(unix)]
pub fn verify_unix_peer(stream: &UnixStream) -> Result<PeerIdentity, IpcServerError> {
    let expected_euid = rustix::process::geteuid().as_raw();
    verify_unix_peer_with_euid(stream, expected_euid)
}

/// Snapshot and intent backend serving the IPC v1 server.
///
/// Keeping the server generic over this trait allows the IPC server and daemon to be tested
/// independently and wired directly to the engine runtime store.
pub trait IpcBackend: Send + Sync + 'static {
    /// Subscribe to asynchronous server push events (`Snapshot`, `SettingsChanged`, `Notice`).
    fn subscribe_pushes(&self) -> broadcast::Receiver<ServerMessage>;

    /// Return the current [`EngineSnapshot`].
    fn current_snapshot(&self) -> Pin<Box<dyn Future<Output = EngineSnapshot> + Send + '_>>;

    /// Trigger a user-initiated refresh for `provider` (or all providers when `None`) and return
    /// the resulting [`EngineSnapshot`].
    fn refresh(
        &self,
        provider: Option<ProviderId>,
    ) -> Pin<Box<dyn Future<Output = Result<EngineSnapshot, IpcError>> + Send + '_>>;

    /// Return the [`SettingsPage`] for `scope`.
    fn get_settings(
        &self,
        scope: SettingsScope,
    ) -> Pin<Box<dyn Future<Output = Result<SettingsPage, IpcError>> + Send + '_>>;

    /// Update a non-secret setting and return the affected [`SettingsScope`].
    fn set_setting(
        &self,
        id: SettingId,
        value: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<SettingsScope, IpcError>> + Send + '_>>;

    /// Execute a provider or maintenance action.
    fn run_action(
        &self,
        id: ActionId,
        confirm: Option<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<Option<serde_json::Value>, IpcError>> + Send + '_>>;
}

/// Handle to a running IPC v1 Unix socket server.
#[cfg(unix)]
pub struct IpcServerHandle {
    socket_path: PathBuf,
    shutdown_tx: watch::Sender<bool>,
    join_handle: Option<tokio::task::JoinHandle<()>>,
}

#[cfg(unix)]
impl IpcServerHandle {
    /// Returns the filesystem path of the bound Unix domain socket.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Gracefully stops the IPC server, waits for all active client tasks to finish, and removes
    /// the socket file.
    pub async fn stop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.await;
        }
        remove_socket_file(&self.socket_path);
    }
}

#[cfg(unix)]
impl Drop for IpcServerHandle {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        remove_socket_file(&self.socket_path);
    }
}

#[cfg(unix)]
fn remove_socket_file(socket_path: &Path) {
    if let Ok(meta) = std::fs::symlink_metadata(socket_path)
        && !meta.is_dir()
    {
        let _ = std::fs::remove_file(socket_path);
    }
}

/// Prepares the socket parent directory with mode `0700`, verifies ownership and non-symlink
/// status via `lstat` before any permission change, and removes any stale socket file at
/// `socket_path`.
#[cfg(unix)]
pub fn prepare_socket_path(socket_path: &Path) -> Result<(), IpcServerError> {
    use std::os::unix::fs::DirBuilderExt;

    validate_socket_path_len(socket_path)?;
    let parent = socket_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| IpcServerError::MissingParentDirectory(socket_path.to_path_buf()))?;

    let stat = match rustix::fs::lstat(parent) {
        Ok(stat) => stat,
        Err(rustix::io::Errno::NOENT) => {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
            rustix::fs::lstat(parent)
                .map_err(|_| IpcServerError::UntrustedParentDirectory(parent.to_path_buf()))?
        }
        Err(_) => {
            return Err(IpcServerError::UntrustedParentDirectory(
                parent.to_path_buf(),
            ));
        }
    };

    let euid = rustix::process::geteuid().as_raw();
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Directory
        || stat.st_uid != euid
    {
        return Err(IpcServerError::UntrustedParentDirectory(
            parent.to_path_buf(),
        ));
    }

    if (stat.st_mode & 0o777) != 0o700 {
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        let verify = rustix::fs::lstat(parent)
            .map_err(|_| IpcServerError::UntrustedParentDirectory(parent.to_path_buf()))?;
        if rustix::fs::FileType::from_raw_mode(verify.st_mode) != rustix::fs::FileType::Directory
            || verify.st_uid != euid
            || (verify.st_mode & 0o777) != 0o700
        {
            return Err(IpcServerError::UntrustedParentDirectory(
                parent.to_path_buf(),
            ));
        }
    }

    if let Ok(meta) = std::fs::symlink_metadata(socket_path) {
        if meta.is_dir() {
            return Err(IpcServerError::UntrustedParentDirectory(
                socket_path.to_path_buf(),
            ));
        }
        std::fs::remove_file(socket_path)?;
    }

    Ok(())
}

/// Starts the IPC v1 Unix socket server at `socket_path` backed by `backend`.
#[cfg(unix)]
pub async fn start_ipc_server(
    socket_path: PathBuf,
    backend: Arc<dyn IpcBackend>,
) -> Result<IpcServerHandle, IpcServerError> {
    prepare_socket_path(&socket_path)?;

    let listener = UnixListener::bind(&socket_path)?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let expected_euid = rustix::process::geteuid().as_raw();

    let join_handle = tokio::spawn(async move {
        let mut clients = JoinSet::new();
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    break;
                }
                accept_res = listener.accept() => {
                    match accept_res {
                        Ok((stream, _)) => {
                            if verify_unix_peer_with_euid(&stream, expected_euid).is_err() {
                                drop(stream);
                                continue;
                            }
                            let client_backend = Arc::clone(&backend);
                            let client_shutdown = shutdown_rx.clone();
                            clients.spawn(async move {
                                handle_client(stream, client_backend, client_shutdown).await;
                            });
                        }
                        Err(_) => {
                            if *shutdown_rx.borrow() {
                                break;
                            }
                        }
                    }
                }
            }
        }
        while clients.join_next().await.is_some() {}
    });

    Ok(IpcServerHandle {
        socket_path,
        shutdown_tx,
        join_handle: Some(join_handle),
    })
}

#[cfg(unix)]
async fn handle_client(
    stream: UnixStream,
    backend: Arc<dyn IpcBackend>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    if *shutdown_rx.borrow_and_update() {
        return;
    }

    let (mut reader, mut writer) = stream.into_split();
    let mut broadcast_rx = backend.subscribe_pushes();
    let mut subscribed_topics: HashSet<Topic> = HashSet::new();

    let mut read_codec = IpcCodec::new();
    let mut write_codec = IpcCodec::new();
    let mut read_buf = BytesMut::new();
    let mut write_buf = BytesMut::new();
    let mut raw_chunk = [0u8; 8192];

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => break,
            read_res = reader.read(&mut raw_chunk) => {
                match read_res {
                    Ok(0) => break,
                    Ok(n) => {
                        read_buf.extend_from_slice(&raw_chunk[..n]);
                        loop {
                            match read_codec.decode(&mut read_buf) {
                                Ok(Some(line)) => {
                                    let envelope_id = serde_json::from_str::<ariadusage_protocol::ipc::RawEnvelope>(&line)
                                        .ok()
                                        .and_then(|env| env.id);

                                    match parse_client_message(&line) {
                                        Ok(msg) => {
                                            let keep_open = dispatch_client_message(
                                                msg,
                                                envelope_id,
                                                &backend,
                                                &mut subscribed_topics,
                                                &mut writer,
                                                &mut write_codec,
                                                &mut write_buf,
                                            )
                                            .await;
                                            if !keep_open {
                                                return;
                                            }
                                        }
                                        Err((req_id, err)) => {
                                            let resp = ServerMessage::error(req_id, err);
                                            if !send_server_message(&mut writer, &mut write_codec, &mut write_buf, &resp).await {
                                                return;
                                            }
                                        }
                                    }
                                }
                                Ok(None) => break,
                                Err(IpcCodecError::MaxLineLengthExceeded) => {
                                    let resp = ServerMessage::error(
                                        None,
                                        IpcError::new(ErrorCode::PayloadTooLarge),
                                    );
                                    if !send_server_message(&mut writer, &mut write_codec, &mut write_buf, &resp).await {
                                        return;
                                    }
                                }
                                Err(_) => return,
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            pushed = broadcast_rx.recv() => {
                if let Ok(msg) = pushed {
                    let should_send = match &msg {
                        ServerMessage::Snapshot { .. } => subscribed_topics.contains(&Topic::Snapshot),
                        ServerMessage::SettingsChanged { .. } => subscribed_topics.contains(&Topic::Settings),
                        ServerMessage::Notice { .. } => subscribed_topics.contains(&Topic::Notices),
                        _ => false,
                    };
                    if should_send && !send_server_message(&mut writer, &mut write_codec, &mut write_buf, &msg).await {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(unix)]
async fn dispatch_client_message(
    msg: ClientMessage,
    envelope_id: Option<ariadusage_protocol::ids::RequestId>,
    backend: &Arc<dyn IpcBackend>,
    subscribed_topics: &mut HashSet<Topic>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    write_codec: &mut IpcCodec,
    write_buf: &mut BytesMut,
) -> bool {
    match msg {
        ClientMessage::Hello { protocols, id, .. } => match negotiate_protocol(&protocols) {
            Some(proto) => {
                let resp = ServerMessage::Welcome {
                    protocol: proto.to_string(),
                    engine_version: env!("CARGO_PKG_VERSION").to_string(),
                    capabilities: vec!["snapshot".into(), "settings".into(), "notices".into()],
                    id,
                };
                send_server_message(writer, write_codec, write_buf, &resp).await
            }
            None => {
                let resp = ServerMessage::error(id, IpcError::new(ErrorCode::InvalidRequest));
                let _ = send_server_message(writer, write_codec, write_buf, &resp).await;
                false
            }
        },
        ClientMessage::Subscribe { topics, id } => {
            let wants_snapshot = topics.contains(&Topic::Snapshot);
            for topic in topics {
                subscribed_topics.insert(topic);
            }
            let resp = ServerMessage::ok(id);
            if !send_server_message(writer, write_codec, write_buf, &resp).await {
                return false;
            }
            if wants_snapshot {
                let snapshot = backend.current_snapshot().await;
                let push = ServerMessage::Snapshot {
                    snapshot: Box::new(snapshot),
                };
                if !send_server_message(writer, write_codec, write_buf, &push).await {
                    return false;
                }
            }
            true
        }
        ClientMessage::GetSnapshot { id: _ } => {
            let snapshot = backend.current_snapshot().await;
            let push = ServerMessage::Snapshot {
                snapshot: Box::new(snapshot),
            };
            send_server_message(writer, write_codec, write_buf, &push).await
        }
        ClientMessage::Refresh { provider, id } => match backend.refresh(provider).await {
            Ok(snapshot) => {
                let resp = ServerMessage::ok(id);
                if !send_server_message(writer, write_codec, write_buf, &resp).await {
                    return false;
                }
                let push = ServerMessage::Snapshot {
                    snapshot: Box::new(snapshot),
                };
                send_server_message(writer, write_codec, write_buf, &push).await
            }
            Err(err) => {
                let resp = ServerMessage::error(id, err);
                send_server_message(writer, write_codec, write_buf, &resp).await
            }
        },
        ClientMessage::GetSettings { scope, id } => match backend.get_settings(scope).await {
            Ok(page) => match serde_json::to_value(page) {
                Ok(payload) => {
                    let resp = ServerMessage::ok_with_payload(id, payload);
                    send_server_message(writer, write_codec, write_buf, &resp).await
                }
                Err(_) => {
                    let resp = ServerMessage::error(id, IpcError::new(ErrorCode::InternalError));
                    send_server_message(writer, write_codec, write_buf, &resp).await
                }
            },
            Err(err) => {
                let resp = ServerMessage::error(id, err);
                send_server_message(writer, write_codec, write_buf, &resp).await
            }
        },
        ClientMessage::SetSetting { id, value } => match backend.set_setting(id, value).await {
            Ok(_scope) => {
                let resp = ServerMessage::ok(envelope_id);
                send_server_message(writer, write_codec, write_buf, &resp).await
            }
            Err(err) => {
                let resp = ServerMessage::error(envelope_id, err);
                send_server_message(writer, write_codec, write_buf, &resp).await
            }
        },
        ClientMessage::RunAction { id, confirm } => match backend.run_action(id, confirm).await {
            Ok(Some(payload)) => {
                let resp = ServerMessage::ok_with_payload(envelope_id, payload);
                send_server_message(writer, write_codec, write_buf, &resp).await
            }
            Ok(None) => {
                let resp = ServerMessage::ok(envelope_id);
                send_server_message(writer, write_codec, write_buf, &resp).await
            }
            Err(err) => {
                let resp = ServerMessage::error(envelope_id, err);
                send_server_message(writer, write_codec, write_buf, &resp).await
            }
        },
        ClientMessage::SetSecret { id: _, value } => {
            // Secrets are entered in-process via `ariadusage secret set` and never accepted over IPC.
            drop(value);
            let resp =
                ServerMessage::error(envelope_id, IpcError::new(ErrorCode::UnsupportedMessage));
            send_server_message(writer, write_codec, write_buf, &resp).await
        }
    }
}

#[cfg(unix)]
async fn send_server_message(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    codec: &mut IpcCodec,
    buf: &mut BytesMut,
    msg: &ServerMessage,
) -> bool {
    let Ok(json) = serde_json::to_string(msg) else {
        return true;
    };
    buf.clear();
    if codec.encode(&json, buf).is_err() {
        buf.clear();
        return true;
    }
    let ok = writer.write_all(buf).await.is_ok();
    buf.clear();
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_peer_uid_accepts_matching_euid_and_rejects_mismatch() {
        assert!(verify_peer_uid(501, 501).is_ok());
        let err = verify_peer_uid(502, 501).expect_err("mismatched UID must fail");
        assert!(matches!(
            err,
            IpcServerError::UntrustedPeerUid {
                peer_uid: 502,
                expected_euid: 501
            }
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn verify_unix_peer_extracts_os_credentials_and_enforces_uid() {
        let (a, _b) = UnixStream::pair().expect("unix stream pair");
        let euid = rustix::process::geteuid().as_raw();
        let peer = verify_unix_peer_with_euid(&a, euid).expect("same-process peer must pass");
        assert_eq!(peer.uid, euid);

        let err = verify_unix_peer_with_euid(&a, euid.wrapping_add(1))
            .expect_err("different expected UID must fail");
        assert!(matches!(err, IpcServerError::UntrustedPeerUid { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn prepare_socket_path_rejects_symlink_parent_without_modifying_target_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("target");
        std::fs::create_dir_all(&target).expect("create target");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
            .expect("set target 0755");

        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");

        let sock = link.join("engine.sock");
        let err = prepare_socket_path(&sock).expect_err("symlink parent must be rejected");
        assert!(matches!(err, IpcServerError::UntrustedParentDirectory(_)));

        let target_mode = std::fs::metadata(&target)
            .expect("target metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            target_mode, 0o755,
            "symlink target permissions must not be changed before lstat rejects the symlink"
        );
    }
}
