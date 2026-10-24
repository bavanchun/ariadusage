// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityScopedPrintFetch.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Account-scoped `agy print` support.
//!
//! The selected OAuth account is copied into a private, short-lived HOME.  The
//! child receives an explicit environment allowlist and the account is checked
//! twice after the report: once against the staged ID-token claim and once by
//! asking Google's userinfo endpoint about the effective access token.  The
//! process and HTTP calls are injected by the CLI strategy so this module does
//! not have a path to a real executable or network in tests.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::private_file::{WriteHooks, write_private};

use super::oauth_store::{
    AntigravityOAuthConfig, AntigravityOAuthCredentials, AntigravityOAuthCredentialsStore, EnvValue,
};
use super::status_probe::{
    AntigravitySnapshotSource, AntigravityStatusProbeError, AntigravityStatusSnapshot,
};

/// The file format consumed by agy's file-token fallback.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AntigravityAgyFileTokenPayload {
    pub token: AntigravityAgyFileToken,
    #[serde(rename = "auth_method")]
    pub auth_method: String,
    #[serde(rename = "id_token", skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
}

impl std::fmt::Debug for AntigravityAgyFileTokenPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AntigravityAgyFileTokenPayload")
            .field("has_token", &true)
            .field("auth_method", &self.auth_method)
            .field("has_id_token", &self.id_token.is_some())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AntigravityAgyFileToken {
    #[serde(rename = "access_token")]
    pub access_token: String,
    #[serde(rename = "token_type")]
    pub token_type: String,
    #[serde(rename = "refresh_token")]
    pub refresh_token: String,
    pub expiry: String,
}

impl std::fmt::Debug for AntigravityAgyFileToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AntigravityAgyFileToken")
            .field("has_access_token", &!self.access_token.is_empty())
            .field("token_type", &self.token_type)
            .field("has_refresh_token", &!self.refresh_token.is_empty())
            .field("has_expiry", &!self.expiry.is_empty())
            .finish()
    }
}

impl AntigravityAgyFileTokenPayload {
    /// Converts a saved account to the exact payload expected by agy.
    pub fn from_credentials(credentials: &AntigravityOAuthCredentials) -> Option<Self> {
        let access_token = non_empty(&credentials.access_token)?;
        let refresh_token = non_empty(credentials.refresh_token.as_deref()?)?;
        let expiry = credentials.expiry_date?;
        Some(Self {
            token: AntigravityAgyFileToken {
                access_token,
                token_type: "Bearer".to_owned(),
                refresh_token,
                expiry: expiry.to_string(),
            },
            auth_method: "consumer".to_owned(),
            id_token: credentials.id_token.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AntigravityScopedStagingError {
    #[error("Antigravity account credentials lack required token or expiry fields.")]
    CredentialsMissingRequiredFields,
    #[error("Antigravity scoped credentials could not be verified against the selected account.")]
    IdentityUnverifiable,
    #[error("could not create private Antigravity staging directory")]
    Io,
}

/// A staged account. Keeping the TempDir in this value guarantees cleanup on
/// every return path, including cancellation and failed identity checks.
pub struct AntigravityScopedAgyHome {
    pub staging_root: tempfile::TempDir,
    pub home: PathBuf,
}

impl std::fmt::Debug for AntigravityScopedAgyHome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AntigravityScopedAgyHome")
            .field("staging_root", &"<private>")
            .field("home", &"<private>")
            .finish()
    }
}

impl AntigravityScopedAgyHome {
    pub fn staging_root_path(&self) -> &Path {
        self.staging_root.path()
    }
}

/// Provider-specific because agy's file-token storage path is an external CLI contract.
pub struct AntigravityScopedAgyStaging;

impl AntigravityScopedAgyStaging {
    pub const TOKEN_RELATIVE_PATH: &'static str = ".gemini/antigravity-cli/antigravity-oauth-token";

    /// Build the child environment from a narrow allowlist. In particular,
    /// injected credentials and unrelated provider secrets are never inherited.
    pub fn child_environment<V: EnvValue>(
        environment: &BTreeMap<String, V>,
        home: &Path,
    ) -> BTreeMap<String, String> {
        const ALLOWLIST: &[&str] = &[
            "PATH",
            "TMPDIR",
            "LANG",
            "LC_ALL",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
        ];
        let mut child = BTreeMap::new();
        for key in ALLOWLIST {
            if let Some(value) = environment
                .get(*key)
                .and_then(|v| non_empty(v.as_env_str()))
            {
                child.insert((*key).to_owned(), value);
            }
        }
        if !child.contains_key("PATH") {
            child.insert("PATH".to_owned(), "/usr/bin:/bin".to_owned());
        }
        let home = home.to_string_lossy().into_owned();
        child.insert("HOME".to_owned(), home.clone());
        child.insert("PWD".to_owned(), home);
        // agy chooses the OS keyring unless SSH_TTY is set. This marker forces
        // its file-token path and has no user-controlled value.
        child.insert("SSH_TTY".to_owned(), "ariadusage-scoped".to_owned());
        child
    }

    pub fn stage(
        credentials: &AntigravityOAuthCredentials,
        expected_account_email: &str,
    ) -> Result<AntigravityScopedAgyHome, AntigravityScopedStagingError> {
        let payload = AntigravityAgyFileTokenPayload::from_credentials(credentials)
            .ok_or(AntigravityScopedStagingError::CredentialsMissingRequiredFields)?;
        Self::validate_claim(credentials.id_token.as_deref(), expected_account_email)?;

        let root = tempfile::Builder::new()
            .prefix("ariadusage-agy-scoped-")
            .tempdir()
            .map_err(|_| AntigravityScopedStagingError::Io)?;
        set_private_dir(root.path()).map_err(|_| AntigravityScopedStagingError::Io)?;
        let token_path = root.path().join(Self::TOKEN_RELATIVE_PATH);
        let bytes = serde_json::to_vec(&payload).map_err(|_| AntigravityScopedStagingError::Io)?;
        write_private(&token_path, &bytes, &WriteHooks::default())
            .map_err(|_| AntigravityScopedStagingError::Io)?;
        Ok(AntigravityScopedAgyHome {
            home: root.path().to_path_buf(),
            staging_root: root,
        })
    }

    pub fn validate_claim(
        id_token: Option<&str>,
        expected_account_email: &str,
    ) -> Result<(), AntigravityScopedStagingError> {
        let claimed = AntigravityOAuthConfig::extract_email_from_id_token(id_token);
        if let Some(claimed) = normalize_email(claimed.as_deref())
            && Some(claimed.as_str()) != normalize_email(Some(expected_account_email)).as_deref()
        {
            return Err(AntigravityScopedStagingError::IdentityUnverifiable);
        }
        Ok(())
    }

    pub fn normalized_email(email: Option<&str>) -> Option<String> {
        normalize_email(email)
    }

    pub fn staged_token_payload(home: &Path) -> Option<AntigravityAgyFileTokenPayload> {
        let call = crate::brokers::call::BrokerCall {
            interaction: ariadusage_core::pipeline::FetchInteraction::Background,
            cancel: CancellationToken::new(),
            request_id: "antigravity-scoped-token".into(),
        };
        let declared = crate::brokers::credential_file::CredentialDecl::new(
            home.join(Self::TOKEN_RELATIVE_PATH),
        );
        let data = crate::brokers::credential_file::read(&declared, &call).ok()?;
        serde_json::from_slice(&data.bytes).ok()
    }

    /// Return updated saved-account credentials only when agy rewrote its file.
    pub fn refreshed_credentials(
        payload: &AntigravityAgyFileTokenPayload,
        original: &AntigravityOAuthCredentials,
    ) -> Option<AntigravityOAuthCredentials> {
        let original_payload = AntigravityAgyFileTokenPayload::from_credentials(original)?;
        if payload == &original_payload {
            return None;
        }
        let mut updated = original.clone();
        if let Some(value) = non_empty(&payload.token.access_token) {
            updated.access_token = value;
        }
        if let Some(value) = non_empty(&payload.token.refresh_token) {
            updated.refresh_token = Some(value);
        }
        if let Ok(expiry) = payload.token.expiry.parse::<Timestamp>() {
            updated.expiry_date = Some(expiry);
        }
        if let Some(id_token) = payload.id_token.as_deref().and_then(non_empty) {
            updated.id_token = Some(id_token);
        }
        Some(updated)
    }
}

/// Google OAuth userinfo endpoint used to verify the effective staged grant.
pub const ANTIGRAVITY_USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v3/userinfo";

/// Input supplied to the parent CLI process seam.
#[derive(Clone)]
pub struct ScopedPrintInvocation {
    pub environment: BTreeMap<String, String>,
    pub home: PathBuf,
    pub timeout: Duration,
}

impl std::fmt::Debug for ScopedPrintInvocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedPrintInvocation")
            .field("environment_count", &self.environment.len())
            .field("has_home", &!self.home.as_os_str().is_empty())
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Input supplied to the parent network seam. The token is wrapped so a debug
/// implementation cannot accidentally print it.
#[derive(Debug, Clone)]
pub struct ScopedUserInfoRequest {
    pub access_token: SecretString,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScopedUserInfoError {
    #[error("userinfo request cancelled")]
    Cancelled,
    #[error("userinfo transport was cancelled")]
    TransportCancelled,
    #[error("userinfo request failed")]
    Failed,
}

/// Fetch a report for one selected account. The parent strategy supplies the
/// process runner and turns the returned snapshot into its FetchResult.
pub async fn fetch_scoped_print_snapshot<V, Run, RunFut, Info, InfoFut, Persist, PersistFut>(
    environment: &BTreeMap<String, V>,
    timeout: Duration,
    cancel: &CancellationToken,
    run_print: Run,
    userinfo: Info,
    persist: Option<Persist>,
) -> Result<AntigravityStatusSnapshot, AntigravityScopedPrintError>
where
    V: EnvValue,
    Run: FnOnce(ScopedPrintInvocation) -> RunFut,
    RunFut: std::future::Future<Output = Result<AntigravityStatusSnapshot, AntigravityStatusProbeError>>,
    Info: Fn(ScopedUserInfoRequest) -> InfoFut,
    InfoFut: std::future::Future<Output = Result<Option<String>, ScopedUserInfoError>>,
    Persist: Fn(AntigravityOAuthCredentials) -> PersistFut,
    PersistFut: std::future::Future<Output = Result<(), ScopedUserInfoError>>,
{
    check_cancel(cancel)?;
    let value = environment
        .get(AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY)
        .and_then(|v| {
            AntigravityOAuthCredentialsStore::credentials_from_token_account_value(v.as_env_str())
        })
        .ok_or_else(|| {
            AntigravityScopedPrintError::Staging(
                AntigravityScopedStagingError::CredentialsMissingRequiredFields,
            )
        })?;
    let expected = value
        .resolved_account_email()
        .ok_or(AntigravityScopedPrintError::Staging(
            AntigravityScopedStagingError::CredentialsMissingRequiredFields,
        ))?;
    let staged = AntigravityScopedAgyStaging::stage(&value, &expected)?;
    let child_environment =
        AntigravityScopedAgyStaging::child_environment(environment, &staged.home);
    let parsed = run_print(ScopedPrintInvocation {
        environment: child_environment,
        home: staged.home.clone(),
        timeout,
    })
    .await
    .map_err(AntigravityScopedPrintError::Print)?;
    check_cancel(cancel)?;

    if let Some(found) = parsed
        .account_email
        .as_deref()
        .and_then(|v| normalize_email(Some(v)))
        && Some(found.as_str()) != normalize_email(Some(&expected)).as_deref()
    {
        return Err(AntigravityScopedPrintError::AccountMismatch {
            expected: expected.clone(),
            found: Some(found),
        });
    }
    let payload = AntigravityScopedAgyStaging::staged_token_payload(&staged.home)
        .ok_or(AntigravityScopedStagingError::IdentityUnverifiable)?;
    AntigravityScopedAgyStaging::validate_claim(payload.id_token.as_deref(), &expected)?;
    let access_token = non_empty(&payload.token.access_token)
        .ok_or(AntigravityScopedStagingError::IdentityUnverifiable)?;
    let effective = userinfo(ScopedUserInfoRequest {
        access_token: SecretString::new(access_token),
        timeout: timeout.min(Duration::from_secs(15)),
    })
    .await
    .map_err(|error| match error {
        ScopedUserInfoError::Cancelled | ScopedUserInfoError::TransportCancelled => {
            AntigravityScopedPrintError::Cancelled
        }
        other => AntigravityScopedPrintError::UserInfo(other),
    })?;
    check_cancel(cancel)?;
    let Some(effective) = normalize_email(effective.as_deref()) else {
        return Err(AntigravityScopedPrintError::Staging(
            AntigravityScopedStagingError::IdentityUnverifiable,
        ));
    };
    if Some(effective.as_str()) != normalize_email(Some(&expected)).as_deref() {
        return Err(AntigravityScopedPrintError::AccountMismatch {
            expected,
            found: Some(effective),
        });
    }
    if let (Some(persist), Some(refreshed)) = (
        persist,
        AntigravityScopedAgyStaging::refreshed_credentials(&payload, &value),
    ) {
        match persist(refreshed).await {
            Ok(()) => {}
            Err(ScopedUserInfoError::Cancelled | ScopedUserInfoError::TransportCancelled) => {
                return Err(AntigravityScopedPrintError::Cancelled);
            }
            Err(ScopedUserInfoError::Failed) => {}
        }
    }
    check_cancel(cancel)?;
    Ok(AntigravityStatusSnapshot {
        model_quotas: parsed.model_quotas,
        account_email: Some(expected),
        account_plan: parsed.account_plan,
        source: AntigravitySnapshotSource::Cli,
        quota_summary: parsed.quota_summary,
    })
}

#[derive(Clone, PartialEq, Eq, thiserror::Error)]
pub enum AntigravityScopedPrintError {
    #[error(transparent)]
    Staging(#[from] AntigravityScopedStagingError),
    #[error("scoped print identity mismatch")]
    AccountMismatch {
        expected: String,
        found: Option<String>,
    },
    #[error(transparent)]
    Print(#[from] AntigravityStatusProbeError),
    #[error(transparent)]
    UserInfo(#[from] ScopedUserInfoError),
    #[error("scoped print cancelled")]
    Cancelled,
}

impl std::fmt::Debug for AntigravityScopedPrintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AccountMismatch { .. } => f.write_str("AccountMismatch(<redacted>)"),
            Self::Print(error) => f.debug_tuple("Print").field(error).finish(),
            _ => std::fmt::Display::fmt(self, f),
        }
    }
}

fn check_cancel(cancel: &CancellationToken) -> Result<(), AntigravityScopedPrintError> {
    if cancel.is_cancelled() {
        Err(AntigravityScopedPrintError::Cancelled)
    } else {
        Ok(())
    }
}

fn normalize_email(email: Option<&str>) -> Option<String> {
    email
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_lowercase)
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn set_private_dir(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
