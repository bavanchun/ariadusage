//! Opt-in browser cookie import. Platform-specific profile and database access lives on Linux.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use ariadusage_core::cookie as cookie_core;
use ariadusage_protocol::ProviderId;
use zeroize::Zeroizing;

use crate::brokers::call::BrokerCall;
use crate::brokers::secret_store::safe_storage::{SafeStorageError, SafeStorageKeySource};
use crate::state_store::BrokerStateStore;

#[cfg(target_os = "linux")]
mod catalog;
#[cfg(target_os = "linux")]
mod chromium;
#[cfg(target_os = "linux")]
mod chromium_crypto;
#[cfg(target_os = "linux")]
mod discover;
#[cfg(target_os = "linux")]
mod error;
#[cfg(target_os = "linux")]
mod firefox;
#[cfg(target_os = "linux")]
mod gate;
#[cfg(target_os = "linux")]
mod local_state;
#[cfg(target_os = "linux")]
mod profiles_ini;
#[cfg(target_os = "linux")]
mod sqlite_copy;

#[cfg(all(target_os = "linux", feature = "test-hooks"))]
pub mod test_support {
    pub use super::chromium_crypto::{DecryptFailure, derive_test_key, encrypt_test};

    pub fn decrypt_v10_test(
        encrypted: &[u8],
        host: &str,
        version: i64,
    ) -> Result<zeroize::Zeroizing<Vec<u8>>, DecryptFailure> {
        super::chromium_crypto::decrypt_v10(encrypted, host, version)
    }

    pub fn decrypt_v11_test(
        encrypted: &[u8],
        password: &[u8],
        host: &str,
        version: i64,
    ) -> Result<zeroize::Zeroizing<Vec<u8>>, DecryptFailure> {
        super::chromium_crypto::decrypt_v11(encrypted, password, host, version)
    }
}

#[cfg(target_os = "linux")]
pub use catalog::{Browser, DEFAULT_IMPORT_ORDER};
#[cfg(not(target_os = "linux"))]
pub use catalog_stub::{Browser, DEFAULT_IMPORT_ORDER};
#[cfg(target_os = "linux")]
pub use error::BrowserError;

#[cfg(target_os = "linux")]
use discover::BrowserBrokerInner;

#[cfg(not(target_os = "linux"))]
mod catalog_stub {
    use std::fmt;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub enum Browser {
        Chrome,
        Edge,
        Brave,
        Chromium,
        Vivaldi,
        Firefox,
        Opera,
    }

    pub const DEFAULT_IMPORT_ORDER: [Browser; 7] = [
        Browser::Chrome,
        Browser::Edge,
        Browser::Brave,
        Browser::Chromium,
        Browser::Vivaldi,
        Browser::Firefox,
        Browser::Opera,
    ];

    impl fmt::Display for Browser {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(match self {
                Self::Chrome => "Chrome",
                Self::Edge => "Edge",
                Self::Brave => "Brave",
                Self::Chromium => "Chromium",
                Self::Vivaldi => "Vivaldi",
                Self::Firefox => "Firefox",
                Self::Opera => "Opera",
            })
        }
    }
}

/// Injectable Secret Service seam used for Chromium-family key retrieval.
pub type SafeStorageFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<Zeroizing<Vec<u8>>>, SafeStorageError>> + Send + 'a>>;

pub trait SafeStorageKeyProvider: Send + Sync {
    fn chromium_safe_storage<'a>(
        &'a self,
        app_attribute: &'a str,
        call: &'a BrokerCall,
    ) -> SafeStorageFuture<'a>;
}

impl SafeStorageKeyProvider for SafeStorageKeySource {
    fn chromium_safe_storage<'a>(
        &'a self,
        app_attribute: &'a str,
        call: &'a BrokerCall,
    ) -> SafeStorageFuture<'a> {
        Box::pin(async move {
            SafeStorageKeySource::chromium_safe_storage(self, app_attribute, call)
                .await
                .map(Some)
        })
    }
}

#[cfg(not(target_os = "linux"))]
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

#[cfg(not(target_os = "linux"))]
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

#[cfg(not(target_os = "linux"))]
impl std::error::Error for BrowserError {}

/// Paths are injected so tests never probe the process home or browser profiles.
#[derive(Clone)]
pub struct BrowserPaths {
    pub home: PathBuf,
    pub config_home: PathBuf,
    pub runtime_dir: Option<PathBuf>,
    #[cfg(all(feature = "test-hooks", target_os = "linux"))]
    test_process_home: Option<PathBuf>,
}

impl BrowserPaths {
    pub fn from_env(
        mut env: impl FnMut(&str) -> Option<std::ffi::OsString>,
        home: PathBuf,
    ) -> Result<Self, BrowserError> {
        if !home.is_absolute() {
            return Err(BrowserError::Unsupported);
        }
        #[cfg(all(feature = "test-hooks", target_os = "linux"))]
        let test_process_home = env("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        let config_home = env("XDG_CONFIG_HOME")
            .and_then(|value| value.into_string().ok())
            .map(|value| PathBuf::from(value.trim()))
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        let runtime_value = env("XDG_RUNTIME_DIR");
        let runtime_dir = crate::paths::resolve_runtime_dir(
            |key| {
                (key == "XDG_RUNTIME_DIR")
                    .then(|| runtime_value.clone())
                    .flatten()
            },
            Some(&home),
        );
        Ok(Self {
            home,
            config_home,
            runtime_dir,
            #[cfg(all(feature = "test-hooks", target_os = "linux"))]
            test_process_home,
        })
    }
}

impl std::fmt::Debug for BrowserPaths {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserPaths")
            .field("home_path_len", &self.home.as_os_str().len())
            .field("config_path_len", &self.config_home.as_os_str().len())
            .field(
                "runtime_path_len",
                &self.runtime_dir.as_ref().map(|path| path.as_os_str().len()),
            )
            .finish()
    }
}

/// Optional allow-list of cookie names and a provider's declared domain set.
#[derive(Clone)]
pub struct CookieQuery<'a> {
    pub provider: ProviderId,
    pub domains: &'a cookie_core::DeclaredDomains,
    pub names: Option<&'a [&'a str]>,
    pub browsers: &'a [Browser],
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BrowserDiagnostics {
    pub rows_read: usize,
    pub rejected_by_hash: usize,
    pub unsupported_by_tag: std::collections::BTreeMap<String, usize>,
    pub expired: usize,
    pub partitioned: usize,
}

pub struct CookieCandidate {
    pub browser: Browser,
    pub profile_id: String,
    pub label: String,
    pub records: Vec<cookie_core::CookieRecord>,
    pub diagnostics: BrowserDiagnostics,
}

impl std::fmt::Debug for CookieCandidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CookieCandidate")
            .field("browser", &self.browser)
            .field("profile_id", &"[opaque]")
            .field("label", &self.label)
            .field("record_count", &self.records.len())
            .field("diagnostics", &self.diagnostics)
            .finish()
    }
}

pub struct BrowserBroker {
    #[cfg(target_os = "linux")]
    inner: BrowserBrokerInner,
}

impl BrowserBroker {
    /// Builds a broker from paths and state resolved by the application layer.
    pub fn new(
        paths: BrowserPaths,
        state: Arc<BrokerStateStore>,
        keys: Arc<dyn SafeStorageKeyProvider>,
    ) -> Self {
        #[cfg(target_os = "linux")]
        sqlite_copy::sweep_stale(paths.runtime_dir.as_deref());
        Self::with_paths(paths, state, keys)
    }

    /// Builds a broker with injected paths and state for isolated tests or host integration.
    pub fn with_paths(
        paths: BrowserPaths,
        state: Arc<BrokerStateStore>,
        keys: Arc<dyn SafeStorageKeyProvider>,
    ) -> Self {
        #[cfg(not(target_os = "linux"))]
        let _ = (paths, state, keys);
        Self {
            #[cfg(target_os = "linux")]
            inner: BrowserBrokerInner::new(paths, state, keys),
        }
    }

    /// Reads candidates only when the core resolver supplied an import capability.
    ///
    /// ```compile_fail
    /// use ariadusage_engine::brokers::browser::{BrowserBroker, CookieQuery};
    /// use ariadusage_engine::brokers::call::BrokerCall;
    ///
    /// async fn missing_authorization(
    ///     broker: &BrowserBroker,
    ///     call: &BrokerCall,
    ///     query: CookieQuery<'_>,
    /// ) {
    ///     let _ = broker.candidates(call, query).await;
    /// }
    /// ```
    pub async fn candidates(
        &self,
        call: &BrokerCall,
        _authorization: cookie_core::ImportAuthorized,
        query: CookieQuery<'_>,
    ) -> Result<Vec<CookieCandidate>, BrowserError> {
        #[cfg(target_os = "linux")]
        {
            self.inner.candidates(call, query).await
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (call, query);
            Err(BrowserError::Unsupported)
        }
    }
}
