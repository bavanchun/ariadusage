//! IPC protocol v1 messages, envelope, errors, and negotiation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::{ActionId, ProviderId, RequestId, SettingId};
use crate::secret::SecretString;
use crate::settings::SettingsScope;
use crate::snapshot::EngineSnapshot;

/// The canonical protocol identifier for AriadUsage IPC v1.
pub const PROTOCOL_V1: &str = "ariadusage-ipc/1";

/// All protocol versions supported by this engine build, in descending order of preference.
pub const SUPPORTED_PROTOCOLS: &[&str] = &[PROTOCOL_V1];

/// Subscription topics supported by the IPC engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Topic {
    Snapshot,
    Settings,
    Notices,
    #[serde(other)]
    Unknown,
}

/// Severity level for asynchronous notice messages pushed by the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
    #[serde(other)]
    Unknown,
}

/// Standard machine-readable error codes for IPC failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    UnsupportedMessage,
    InvalidRequest,
    PayloadTooLarge,
    InternalError,
    NotFound,
    #[serde(other)]
    Unknown,
}

impl ErrorCode {
    /// Return a safe, static English description for this error code.
    ///
    /// Never returns dynamic input or serde error details.
    #[must_use]
    pub const fn static_message(&self) -> &'static str {
        match self {
            Self::UnsupportedMessage => "unsupported message type",
            Self::InvalidRequest => "malformed or invalid request payload",
            Self::PayloadTooLarge => "message frame exceeds maximum allowed size",
            Self::InternalError => "internal engine error",
            Self::NotFound => "requested resource not found",
            Self::Unknown => "unknown error",
        }
    }
}

/// Structured IPC error carrying a machine-readable code and static safe message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: ErrorCode,
    pub message: String,
}

impl IpcError {
    /// Construct an error with the default static message for `code`.
    pub fn new(code: ErrorCode) -> Self {
        Self {
            message: code.static_message().to_string(),
            code,
        }
    }

    /// Construct an error with a custom static message.
    pub fn with_static_message(code: ErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

/// Information identifying the client in the `hello` handshake message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

impl ClientInfo {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

/// Raw message envelope inspected in the first pass of two-pass decoding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RawEnvelope {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
}

/// Messages sent from client to server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClientMessage {
    /// Initial client handshake advertising supported protocols and client metadata.
    #[serde(rename_all = "camelCase")]
    Hello {
        protocols: Vec<String>,
        client: ClientInfo,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<RequestId>,
    },
    /// Subscribe to pushed update topics.
    #[serde(rename_all = "camelCase")]
    Subscribe {
        topics: Vec<Topic>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<RequestId>,
    },
    /// Request an immediate usage snapshot push.
    #[serde(rename_all = "camelCase")]
    GetSnapshot {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<RequestId>,
    },
    /// Trigger an immediate refresh for all providers or a specific provider.
    #[serde(rename_all = "camelCase")]
    Refresh {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<ProviderId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<RequestId>,
    },
    /// Fetch settings descriptors for the given scope.
    #[serde(rename_all = "camelCase")]
    GetSettings {
        scope: SettingsScope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<RequestId>,
    },
    /// Modify a non-secret setting value.
    #[serde(rename_all = "camelCase")]
    SetSetting {
        id: SettingId,
        value: serde_json::Value,
    },
    /// Run a provider action.
    #[serde(rename_all = "camelCase")]
    RunAction {
        id: ActionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confirm: Option<bool>,
    },
    /// Store a sensitive secret (e.g. API token). Never echoed or logged.
    #[serde(rename_all = "camelCase")]
    SetSecret { id: SettingId, value: SecretString },
}

impl ClientMessage {
    /// Returns the message type name matching the wire `type` tag.
    #[must_use]
    pub const fn msg_type(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::Subscribe { .. } => "subscribe",
            Self::GetSnapshot { .. } => "getSnapshot",
            Self::Refresh { .. } => "refresh",
            Self::GetSettings { .. } => "getSettings",
            Self::SetSetting { .. } => "setSetting",
            Self::RunAction { .. } => "runAction",
            Self::SetSecret { .. } => "setSecret",
        }
    }
}

/// Messages sent from server to client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ServerMessage {
    /// Handshake response confirming negotiated protocol and engine capabilities.
    #[serde(rename_all = "camelCase")]
    Welcome {
        protocol: String,
        engine_version: String,
        capabilities: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<RequestId>,
    },
    /// Explicit response to a client request.
    #[serde(rename_all = "camelCase")]
    Response {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<RequestId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ok: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<IpcError>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<serde_json::Value>,
    },
    /// Pushed engine usage snapshot.
    #[serde(rename_all = "camelCase")]
    Snapshot { snapshot: Box<EngineSnapshot> },
    /// Pushed notification that settings in `scope` have changed.
    #[serde(rename_all = "camelCase")]
    SettingsChanged { scope: SettingsScope },
    /// Pushed system notice or transient status warning.
    #[serde(rename_all = "camelCase")]
    Notice { level: NoticeLevel, message: String },
}

impl ServerMessage {
    /// Construct a successful response to a request.
    pub fn ok(id: Option<RequestId>) -> Self {
        Self::Response {
            id,
            ok: Some(true),
            error: None,
            payload: None,
        }
    }

    /// Construct a successful response carrying a payload.
    pub fn ok_with_payload(id: Option<RequestId>, payload: serde_json::Value) -> Self {
        Self::Response {
            id,
            ok: Some(true),
            error: None,
            payload: Some(payload),
        }
    }

    /// Construct an error response to a request.
    pub fn error(id: Option<RequestId>, error: IpcError) -> Self {
        Self::Response {
            id,
            ok: Some(false),
            error: Some(error),
            payload: None,
        }
    }

    /// Returns the message type name matching the wire `type` tag.
    #[must_use]
    pub const fn msg_type(&self) -> &'static str {
        match self {
            Self::Welcome { .. } => "welcome",
            Self::Response { .. } => "response",
            Self::Snapshot { .. } => "snapshot",
            Self::SettingsChanged { .. } => "settingsChanged",
            Self::Notice { .. } => "notice",
        }
    }
}

/// Top-level message for IPC v1 framing, covering both client requests and server pushes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum IpcMessage {
    Client(ClientMessage),
    Server(ServerMessage),
}

/// Perform two-pass decode of an inbound client message string.
///
/// 1. Decodes [`RawEnvelope`] to extract the message type and request ID.
/// 2. If the `type` is unrecognized, returns `(envelope.id, IpcError::new(ErrorCode::UnsupportedMessage))`.
/// 3. Otherwise, decodes the full [`ClientMessage`]. If decoding fails, returns
///    `(envelope.id, IpcError::new(ErrorCode::InvalidRequest))` with a static message,
///    ensuring no raw input or serde error text is ever echoed.
pub fn parse_client_message(line: &str) -> Result<ClientMessage, (Option<RequestId>, IpcError)> {
    let envelope: RawEnvelope = match serde_json::from_str(line) {
        Ok(env) => env,
        Err(_) => {
            return Err((None, IpcError::new(ErrorCode::InvalidRequest)));
        }
    };

    match envelope.msg_type.as_str() {
        "hello" | "subscribe" | "getSnapshot" | "refresh" | "getSettings" | "setSetting"
        | "runAction" | "setSecret" => match serde_json::from_str::<ClientMessage>(line) {
            Ok(msg) => Ok(msg),
            Err(_) => Err((envelope.id, IpcError::new(ErrorCode::InvalidRequest))),
        },
        _ => Err((envelope.id, IpcError::new(ErrorCode::UnsupportedMessage))),
    }
}

/// Perform two-pass decode of an inbound server message string.
pub fn parse_server_message(line: &str) -> Result<ServerMessage, (Option<RequestId>, IpcError)> {
    let envelope: RawEnvelope = match serde_json::from_str(line) {
        Ok(env) => env,
        Err(_) => {
            return Err((None, IpcError::new(ErrorCode::InvalidRequest)));
        }
    };

    match envelope.msg_type.as_str() {
        "welcome" | "response" | "snapshot" | "settingsChanged" | "notice" => {
            match serde_json::from_str::<ServerMessage>(line) {
                Ok(msg) => Ok(msg),
                Err(_) => Err((envelope.id, IpcError::new(ErrorCode::InvalidRequest))),
            }
        }
        _ => Err((envelope.id, IpcError::new(ErrorCode::UnsupportedMessage))),
    }
}

/// Negotiate protocol version between client-offered protocols and supported server versions.
///
/// Returns the highest protocol version present in both lists, or `None` if there is no match.
#[must_use]
pub fn negotiate_protocol<S: AsRef<str>>(client_protocols: &[S]) -> Option<&'static str> {
    SUPPORTED_PROTOCOLS
        .iter()
        .copied()
        .find(|&supported| client_protocols.iter().any(|c| c.as_ref() == supported))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_negotiation_picks_highest_common() {
        let client = vec!["ariadusage-ipc/0", "ariadusage-ipc/1"];
        assert_eq!(negotiate_protocol(&client), Some(PROTOCOL_V1));

        let unsupported = vec!["other-proto/1", "ariadusage-ipc/99"];
        assert_eq!(negotiate_protocol(&unsupported), None);
    }

    #[test]
    fn two_pass_decode_unsupported_returns_original_id() {
        let line = r#"{"type":"futureCommand","id":"req-42","foo":"bar"}"#;
        let err = parse_client_message(line).unwrap_err();
        assert_eq!(err.0.as_deref(), Some("req-42"));
        assert_eq!(err.1.code, ErrorCode::UnsupportedMessage);
        assert_eq!(err.1.message, "unsupported message type");
    }

    #[test]
    fn two_pass_decode_malformed_returns_original_id_and_static_error() {
        let line = r#"{"type":"refresh","id":"req-99","provider":12345}"#;
        let err = parse_client_message(line).unwrap_err();
        assert_eq!(err.0.as_deref(), Some("req-99"));
        assert_eq!(err.1.code, ErrorCode::InvalidRequest);
        assert_eq!(err.1.message, "malformed or invalid request payload");
        assert!(!err.1.message.contains("12345"));

        let invalid_id_line = r#"{"type":"setSetting","id":"bad id with spaces","value":123}"#;
        let err2 = parse_client_message(invalid_id_line).unwrap_err();
        assert_eq!(err2.0, None);
        assert_eq!(err2.1.code, ErrorCode::InvalidRequest);
        assert!(!err2.1.message.contains("spaces"));
    }

    #[test]
    fn unknown_topic_decodes_to_unknown() {
        let json = r#"{"type":"subscribe","topics":["snapshot","quantumState"]}"#;
        let msg = serde_json::from_str::<ClientMessage>(json).unwrap();
        match msg {
            ClientMessage::Subscribe { topics, .. } => {
                assert_eq!(topics, vec![Topic::Snapshot, Topic::Unknown]);
            }
            _ => panic!("unexpected message"),
        }
    }

    #[test]
    fn unknown_notice_level_decodes_to_unknown() {
        let json = r#"{"type":"notice","level":"urgent","message":"test"}"#;
        let msg = serde_json::from_str::<ServerMessage>(json).unwrap();
        match msg {
            ServerMessage::Notice { level, message } => {
                assert_eq!(level, NoticeLevel::Unknown);
                assert_eq!(message, "test");
            }
            _ => panic!("unexpected message"),
        }
    }

    #[test]
    fn set_secret_debug_redacts_planted_value() {
        let planted = "sk-ant-api03-planted-secret-123456789";
        let msg = ClientMessage::SetSecret {
            id: SettingId::new("providers.claude.apiKey").unwrap(),
            value: SecretString::new(planted),
        };
        let debug = format!("{msg:?}");
        assert!(!debug.contains(planted));
        assert!(debug.contains("[redacted]"));
    }
}
