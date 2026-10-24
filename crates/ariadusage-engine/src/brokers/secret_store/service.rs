// Ported from CodexBar Sources/CodexBarCore/KeychainAccessGate.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/KeychainNoUIQuery.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/KeychainStringStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#[cfg(target_os = "macos")]
use std::cell::Cell;
#[cfg(target_os = "macos")]
use std::collections::BTreeSet;
use std::fmt;
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
#[cfg(target_os = "macos")]
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(target_os = "linux")]
use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::future::Future;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::time::Duration;

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_protocol::secret::SecretString;
#[cfg(target_os = "macos")]
use zeroize::Zeroizing;

use super::{BackendFuture, SecretBackend, SecretId, SecretLookup, SecretStoreError};
use crate::brokers::call::BrokerCall;

#[cfg(target_os = "linux")]
const DBUS_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(target_os = "macos")]
pub(crate) const KEYCHAIN_BACKGROUND_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) const USER_UNLOCK_TIMEOUT: Duration = Duration::from_secs(120);

/// macOS `Security.framework` `OSStatus` codes used to classify Keychain outcomes.
pub mod os_status {
    pub const ERR_SEC_SUCCESS: i32 = 0;
    pub const ERR_SEC_UNIMPLEMENTED: i32 = -4;
    pub const ERR_SEC_PARAM: i32 = -50;
    pub const ERR_SEC_USER_CANCELED: i32 = -128;
    pub const ERR_SEC_NO_ACCESS_FOR_ITEM: i32 = -25243;
    pub const ERR_SEC_NOT_AVAILABLE: i32 = -25291;
    pub const ERR_SEC_AUTH_FAILED: i32 = -25293;
    pub const ERR_SEC_NO_SUCH_KEYCHAIN: i32 = -25294;
    pub const ERR_SEC_INVALID_KEYCHAIN: i32 = -25295;
    pub const ERR_SEC_DUPLICATE_ITEM: i32 = -25299;
    pub const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
    pub const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25308;
    pub const ERR_SEC_READ_ONLY_ATTR: i32 = -25309;
    /// Internal synthetic status returned by [`super::with_macos_keychain_interaction`] when a
    /// non-interactive background call is skipped because a user-initiated prompt holds
    /// `USER_PROMPT_LOCK` on another thread. Chosen outside Apple's `OSStatus` range so real
    /// Security framework status codes (such as `errSecReadOnlyAttr = -25309`) are never
    /// misclassified as `Busy`.
    pub const ERR_SEC_PROMPT_IN_FLIGHT_BUSY: i32 = i32::MIN;
}

/// Low-level Keychain operations seam on macOS so tests can inject deterministic status codes
/// or an isolated temporary `.keychain-db` file without touching the user's login Keychain.
#[cfg(target_os = "macos")]
pub trait MacosKeychainOperations: Send + Sync {
    fn read_generic_password(
        &self,
        service: &str,
        account: &str,
        allow_interaction: bool,
    ) -> Result<Zeroizing<Vec<u8>>, i32>;

    fn write_generic_password(
        &self,
        service: &str,
        account: &str,
        label: &str,
        password: &[u8],
    ) -> Result<(), i32>;
}

#[cfg(target_os = "macos")]
impl<T: MacosKeychainOperations + ?Sized> MacosKeychainOperations for Arc<T> {
    fn read_generic_password(
        &self,
        service: &str,
        account: &str,
        allow_interaction: bool,
    ) -> Result<Zeroizing<Vec<u8>>, i32> {
        (**self).read_generic_password(service, account, allow_interaction)
    }

    fn write_generic_password(
        &self,
        service: &str,
        account: &str,
        label: &str,
        password: &[u8],
    ) -> Result<(), i32> {
        (**self).write_generic_password(service, account, label, password)
    }
}

/// Secret Service (Linux) and Keychain Services (macOS) backend.
/// The kill switch (`ARIADUSAGE_DISABLE_KEYRING=1`) is injected by the caller.
pub struct ServiceBackend {
    disabled: bool,
    #[cfg(target_os = "macos")]
    macos_ops: Option<Arc<dyn MacosKeychainOperations>>,
}

impl ServiceBackend {
    pub const fn new(disabled: bool) -> Self {
        Self {
            disabled,
            #[cfg(target_os = "macos")]
            macos_ops: None,
        }
    }

    /// Creates a `ServiceBackend` backed by an isolated `SecKeychain` file (for tests).
    #[cfg(target_os = "macos")]
    pub fn with_keychain(keychain: security_framework::os::macos::keychain::SecKeychain) -> Self {
        Self::with_keychain_disabled(keychain, false)
    }

    /// Creates a `ServiceBackend` backed by an isolated `SecKeychain` file with an explicit
    /// kill-switch flag.
    #[cfg(target_os = "macos")]
    pub fn with_keychain_disabled(
        keychain: security_framework::os::macos::keychain::SecKeychain,
        disabled: bool,
    ) -> Self {
        Self {
            disabled,
            macos_ops: Some(Arc::new(SecKeychainOperations::new(Some(keychain)))),
        }
    }

    /// Creates a `ServiceBackend` backed by an isolated `SecKeychain` file where
    /// `kSecUseAuthenticationUISkip` (`skip_authenticated_items`) simulates skipping an
    /// authentication-protected item, exercising the unfiltered attribute probe and cache.
    #[cfg(target_os = "macos")]
    pub fn with_keychain_simulating_authenticated_skip(
        keychain: security_framework::os::macos::keychain::SecKeychain,
    ) -> Self {
        Self {
            disabled: false,
            macos_ops: Some(Arc::new(
                SecKeychainOperations::new_simulating_authenticated_skip(Some(keychain)),
            )),
        }
    }

    /// Creates a `ServiceBackend` with injected Keychain operations (for deterministic tests).
    #[cfg(target_os = "macos")]
    pub fn with_operations(ops: impl MacosKeychainOperations + 'static) -> Self {
        Self::with_operations_disabled(ops, false)
    }

    /// Creates a `ServiceBackend` with injected Keychain operations and an explicit kill-switch flag.
    #[cfg(target_os = "macos")]
    pub fn with_operations_disabled(
        ops: impl MacosKeychainOperations + 'static,
        disabled: bool,
    ) -> Self {
        Self {
            disabled,
            macos_ops: Some(Arc::new(ops)),
        }
    }
}

impl fmt::Debug for ServiceBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceBackend")
            .field("disabled", &self.disabled)
            .finish()
    }
}

impl SecretBackend for ServiceBackend {
    fn lookup_background<'a>(
        &'a self,
        id: &'a SecretId,
        call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<SecretLookup, SecretStoreError>> {
        Box::pin(async move {
            if call.cancel.is_cancelled() {
                return Err(SecretStoreError::Cancelled);
            }
            if self.disabled {
                return Ok(SecretLookup::Unavailable);
            }
            #[cfg(target_os = "linux")]
            {
                lookup_secret(id, call, false).await
            }
            #[cfg(target_os = "macos")]
            {
                let ops = self.macos_ops.clone();
                let id = id.clone();
                let call_for_task = call.clone();
                run_macos_blocking(call, KEYCHAIN_BACKGROUND_TIMEOUT, move || {
                    lookup_secret_macos(ops.as_deref(), &id, &call_for_task, false)
                })
                .await
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                let _ = (id, call);
                Ok(SecretLookup::Unavailable)
            }
        })
    }

    fn lookup_user<'a>(
        &'a self,
        id: &'a SecretId,
        call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<SecretLookup, SecretStoreError>> {
        Box::pin(async move {
            if call.cancel.is_cancelled() {
                return Err(SecretStoreError::Cancelled);
            }
            if self.disabled {
                return Ok(SecretLookup::Unavailable);
            }
            #[cfg(target_os = "linux")]
            {
                lookup_secret(
                    id,
                    call,
                    call.interaction == FetchInteraction::UserInitiated,
                )
                .await
            }
            #[cfg(target_os = "macos")]
            {
                let user_initiated = call.interaction == FetchInteraction::UserInitiated;
                let timeout = if user_initiated {
                    USER_UNLOCK_TIMEOUT
                } else {
                    KEYCHAIN_BACKGROUND_TIMEOUT
                };
                let ops = self.macos_ops.clone();
                let id = id.clone();
                let call_for_task = call.clone();
                run_macos_blocking(call, timeout, move || {
                    lookup_secret_macos(ops.as_deref(), &id, &call_for_task, user_initiated)
                })
                .await
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                let _ = (id, call);
                Ok(SecretLookup::Unavailable)
            }
        })
    }

    fn set_user<'a>(
        &'a self,
        id: &'a SecretId,
        secret: &'a SecretString,
        call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<(), SecretStoreError>> {
        Box::pin(async move {
            if call.cancel.is_cancelled() {
                return Err(SecretStoreError::Cancelled);
            }
            if call.interaction != FetchInteraction::UserInitiated {
                return Err(SecretStoreError::Locked);
            }
            if self.disabled {
                return Err(SecretStoreError::Unavailable);
            }
            if secret.expose_secret().is_empty() {
                return Err(SecretStoreError::Invalid);
            }
            #[cfg(target_os = "linux")]
            {
                set_secret(id, secret, call).await
            }
            #[cfg(target_os = "macos")]
            {
                let ops = self.macos_ops.clone();
                let id = id.clone();
                let secret = secret.clone();
                let call_for_task = call.clone();
                run_macos_blocking(call, USER_UNLOCK_TIMEOUT, move || {
                    set_secret_macos(ops.as_deref(), &id, &secret, &call_for_task)
                })
                .await
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                let _ = (id, call);
                Err(SecretStoreError::Unavailable)
            }
        })
    }

    fn chromium_key_background<'a>(
        &'a self,
        app_attribute: &'a str,
        call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<Option<zeroize::Zeroizing<Vec<u8>>>, SecretStoreError>> {
        Box::pin(async move {
            if call.cancel.is_cancelled() {
                return Err(SecretStoreError::Cancelled);
            }
            if self.disabled {
                return Err(SecretStoreError::Unavailable);
            }
            #[cfg(target_os = "linux")]
            {
                chromium_key(app_attribute, call, false).await
            }
            #[cfg(target_os = "macos")]
            {
                let ops = self.macos_ops.clone();
                let app_attribute = app_attribute.to_owned();
                let call_for_task = call.clone();
                run_macos_blocking(call, KEYCHAIN_BACKGROUND_TIMEOUT, move || {
                    chromium_key_macos(ops.as_deref(), &app_attribute, &call_for_task, false)
                })
                .await
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                let _ = (app_attribute, call);
                Err(SecretStoreError::Unavailable)
            }
        })
    }

    fn chromium_key_user<'a>(
        &'a self,
        app_attribute: &'a str,
        call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<Option<zeroize::Zeroizing<Vec<u8>>>, SecretStoreError>> {
        Box::pin(async move {
            if call.cancel.is_cancelled() {
                return Err(SecretStoreError::Cancelled);
            }
            if self.disabled {
                return Err(SecretStoreError::Unavailable);
            }
            #[cfg(target_os = "linux")]
            {
                chromium_key(
                    app_attribute,
                    call,
                    call.interaction == FetchInteraction::UserInitiated,
                )
                .await
            }
            #[cfg(target_os = "macos")]
            {
                let user_initiated = call.interaction == FetchInteraction::UserInitiated;
                let timeout = if user_initiated {
                    USER_UNLOCK_TIMEOUT
                } else {
                    KEYCHAIN_BACKGROUND_TIMEOUT
                };
                let ops = self.macos_ops.clone();
                let app_attribute = app_attribute.to_owned();
                let call_for_task = call.clone();
                run_macos_blocking(call, timeout, move || {
                    chromium_key_macos(
                        ops.as_deref(),
                        &app_attribute,
                        &call_for_task,
                        user_initiated,
                    )
                })
                .await
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                let _ = (app_attribute, call);
                Err(SecretStoreError::Unavailable)
            }
        })
    }
}

/// Runs a blocking macOS Keychain closure on `tokio::task::spawn_blocking`, bounded by
/// `timeout_duration` (`USER_UNLOCK_TIMEOUT` for interactive user operations,
/// `KEYCHAIN_BACKGROUND_TIMEOUT` for non-interactive background queries), while observing
/// `call.cancel` before, during, and after the blocking execution.
///
/// Note: if `timeout_duration` elapses before `f` returns, `run_macos_blocking` returns
/// [`SecretStoreError::Timeout`] while the underlying blocking worker thread and any open OS
/// Keychain dialog stay alive holding the prompt lock until the OS call finishes.
#[cfg(target_os = "macos")]
async fn run_macos_blocking<F, T>(
    call: &BrokerCall,
    timeout_duration: Duration,
    f: F,
) -> Result<T, SecretStoreError>
where
    F: FnOnce() -> Result<T, SecretStoreError> + Send + 'static,
    T: Send + 'static,
{
    if call.cancel.is_cancelled() {
        return Err(SecretStoreError::Cancelled);
    }

    let handle = tokio::task::spawn_blocking(f);
    let outcome = tokio::select! {
        _ = call.cancel.cancelled() => return Err(SecretStoreError::Cancelled),
        timed = tokio::time::timeout(timeout_duration, handle) => {
            match timed {
                Err(_) => return Err(SecretStoreError::Timeout),
                Ok(Err(_join_error)) => return Err(SecretStoreError::Storage),
                Ok(Ok(res)) => res,
            }
        }
    };

    if call.cancel.is_cancelled() {
        return Err(SecretStoreError::Cancelled);
    }
    outcome
}

#[cfg(target_os = "macos")]
static USER_PROMPT_LOCK: Mutex<()> = Mutex::new(());
#[cfg(target_os = "macos")]
static BACKGROUND_NO_UI_LOCK: Mutex<()> = Mutex::new(());
#[cfg(target_os = "macos")]
static USER_PROMPT_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "macos")]
static BACKGROUND_NO_UI_SKIPPED_DURING_PROMPT_COUNT: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_os = "macos")]
static LABEL_UPDATE_WARNING_COUNT: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_os = "macos")]
thread_local! {
    static CURRENT_THREAD_INTERACTION: Cell<Option<bool>> = const { Cell::new(None) };
}

#[cfg(target_os = "macos")]
struct UserPromptActiveGuard;

#[cfg(target_os = "macos")]
impl UserPromptActiveGuard {
    fn enter() -> Self {
        USER_PROMPT_ACTIVE.store(true, Ordering::SeqCst);
        Self
    }
}

#[cfg(target_os = "macos")]
impl Drop for UserPromptActiveGuard {
    fn drop(&mut self) {
        USER_PROMPT_ACTIVE.store(false, Ordering::SeqCst);
    }
}

#[cfg(target_os = "macos")]
struct ThreadInteractionGuard {
    previous: Option<bool>,
}

#[cfg(target_os = "macos")]
impl ThreadInteractionGuard {
    fn enter(allow_interaction: bool) -> Self {
        let previous =
            CURRENT_THREAD_INTERACTION.with(|cell| cell.replace(Some(allow_interaction)));
        Self { previous }
    }
}

#[cfg(target_os = "macos")]
impl Drop for ThreadInteractionGuard {
    fn drop(&mut self) {
        let previous = self.previous;
        CURRENT_THREAD_INTERACTION.with(|cell| cell.set(previous));
    }
}

/// Returns `true` if `USER_PROMPT_LOCK` is currently held by either a user-initiated prompt or a
/// background no-UI section.
#[cfg(target_os = "macos")]
pub fn is_macos_prompt_lock_held() -> bool {
    USER_PROMPT_LOCK.try_lock().is_err()
}

/// Returns `true` if a user-initiated macOS Keychain operation (`allow_interaction == true`) is
/// currently active.
///
/// Unlike `USER_PROMPT_LOCK.try_lock().is_err()`, this flag is set only by the user-initiated
/// prompt path and remains `false` when a background thread acquires `USER_PROMPT_LOCK` to hold
/// `SecKeychainSetUserInteractionAllowed(false)`.
#[cfg(target_os = "macos")]
pub fn is_macos_user_prompt_active() -> bool {
    USER_PROMPT_ACTIVE.load(Ordering::SeqCst)
}

/// Returns the cumulative count of background no-UI Keychain calls that returned
/// `ERR_SEC_PROMPT_IN_FLIGHT_BUSY` early because a user-initiated prompt held `USER_PROMPT_LOCK`.
#[cfg(target_os = "macos")]
pub fn macos_background_no_ui_skipped_during_prompt_count() -> usize {
    BACKGROUND_NO_UI_SKIPPED_DURING_PROMPT_COUNT.load(Ordering::SeqCst)
}

/// Returns the cumulative count of non-fatal Keychain item label update failures on macOS.
#[cfg(target_os = "macos")]
pub fn macos_label_update_warning_count() -> usize {
    LABEL_UPDATE_WARNING_COUNT.load(Ordering::SeqCst)
}

/// Process-wide Keychain UI control seam on macOS (`SecKeychainGetUserInteractionAllowed` /
/// `SecKeychainSetUserInteractionAllowed`) so unit tests can verify UI disable and restore
/// ordering inside [`with_macos_keychain_interaction_using`] without depending on an outer
/// test-suite guard.
#[cfg(target_os = "macos")]
pub trait MacosKeychainUiControl {
    type Guard;

    fn user_interaction_allowed(&self) -> Result<bool, i32>;
    fn disable_user_interaction(&self) -> Result<Self::Guard, i32>;
}

#[cfg(target_os = "macos")]
struct DefaultMacosKeychainUiControl;

#[cfg(target_os = "macos")]
impl MacosKeychainUiControl for DefaultMacosKeychainUiControl {
    type Guard = security_framework::os::macos::keychain::KeychainUserInteractionLock;

    fn user_interaction_allowed(&self) -> Result<bool, i32> {
        use security_framework::os::macos::keychain::SecKeychain;
        SecKeychain::user_interaction_allowed().map_err(|e| e.code())
    }

    fn disable_user_interaction(&self) -> Result<Self::Guard, i32> {
        use security_framework::os::macos::keychain::SecKeychain;
        SecKeychain::disable_user_interaction().map_err(|e| e.code())
    }
}

/// Runs `f` while enforcing per-call `Security.framework` user-interaction state across threads.
///
/// - The `allow_interaction` flag is decided per call from that call's own interaction, never
///   from shared state.
/// - User-initiated operations (`allow_interaction == true`) acquire `USER_PROMPT_LOCK` and mark
///   `USER_PROMPT_ACTIVE = true` for the duration of `f`.
/// - Background operations (`allow_interaction == false`) acquire `BACKGROUND_NO_UI_LOCK` and then
///   attempt `USER_PROMPT_LOCK.try_lock()`:
///   - When a user-initiated prompt holds `USER_PROMPT_LOCK` (or `USER_PROMPT_ACTIVE` is `true`),
///     process-wide Keychain UI (`SecKeychainSetUserInteractionAllowed`) cannot be disabled for
///     this background call without interfering with the open user dialog, and running `f` with
///     process-wide UI enabled could trigger a second password dialog. Therefore, this helper
///     immediately returns `Err(os_status::ERR_SEC_PROMPT_IN_FLIGHT_BUSY)` **before** executing
///     any data-read closure `f`.
///   - Otherwise, this helper holds both `BACKGROUND_NO_UI_LOCK` and `USER_PROMPT_LOCK` with
///     `SecKeychainSetUserInteractionAllowed(false)` for the entire duration of `f` (preserving an
///     outer test guard if UI was already disabled).
#[cfg(target_os = "macos")]
pub fn with_macos_keychain_interaction<T>(
    allow_interaction: bool,
    f: impl FnOnce() -> Result<T, i32>,
) -> Result<T, i32> {
    with_macos_keychain_interaction_using(&DefaultMacosKeychainUiControl, allow_interaction, f)
}

/// Testable core of [`with_macos_keychain_interaction`] accepting an explicit
/// [`MacosKeychainUiControl`] seam.
#[cfg(target_os = "macos")]
pub fn with_macos_keychain_interaction_using<T, C: MacosKeychainUiControl>(
    ui_control: &C,
    allow_interaction: bool,
    f: impl FnOnce() -> Result<T, i32>,
) -> Result<T, i32> {
    if let Some(current_mode) = CURRENT_THREAD_INTERACTION.with(Cell::get) {
        if allow_interaction && !current_mode {
            return Err(os_status::ERR_SEC_INTERACTION_NOT_ALLOWED);
        }
        return f();
    }

    if allow_interaction {
        let _user_guard = USER_PROMPT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _active_guard = UserPromptActiveGuard::enter();
        let _thread_guard = ThreadInteractionGuard::enter(true);
        f()
    } else {
        let _bg_guard = BACKGROUND_NO_UI_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if is_macos_user_prompt_active() {
            BACKGROUND_NO_UI_SKIPPED_DURING_PROMPT_COUNT.fetch_add(1, Ordering::SeqCst);
            return Err(os_status::ERR_SEC_PROMPT_IN_FLIGHT_BUSY);
        }
        let Ok(_user_guard) = USER_PROMPT_LOCK.try_lock() else {
            BACKGROUND_NO_UI_SKIPPED_DURING_PROMPT_COUNT.fetch_add(1, Ordering::SeqCst);
            return Err(os_status::ERR_SEC_PROMPT_IN_FLIGHT_BUSY);
        };
        if is_macos_user_prompt_active() {
            BACKGROUND_NO_UI_SKIPPED_DURING_PROMPT_COUNT.fetch_add(1, Ordering::SeqCst);
            return Err(os_status::ERR_SEC_PROMPT_IN_FLIGHT_BUSY);
        }
        let _no_ui_guard = if ui_control.user_interaction_allowed().unwrap_or(true) {
            Some(ui_control.disable_user_interaction()?)
        } else {
            None
        };
        let _thread_guard = ThreadInteractionGuard::enter(false);
        f()
    }
}

#[cfg(target_os = "macos")]
struct SecKeychainOperations {
    keychain: Option<security_framework::os::macos::keychain::SecKeychain>,
    interaction_required_cache: Mutex<BTreeSet<(String, String)>>,
    simulate_authenticated_skip: bool,
}

#[cfg(target_os = "macos")]
impl SecKeychainOperations {
    fn new(keychain: Option<security_framework::os::macos::keychain::SecKeychain>) -> Self {
        Self {
            keychain,
            interaction_required_cache: Mutex::new(BTreeSet::new()),
            simulate_authenticated_skip: false,
        }
    }

    fn new_simulating_authenticated_skip(
        keychain: Option<security_framework::os::macos::keychain::SecKeychain>,
    ) -> Self {
        Self {
            keychain,
            interaction_required_cache: Mutex::new(BTreeSet::new()),
            simulate_authenticated_skip: true,
        }
    }
}

#[cfg(target_os = "macos")]
fn default_sec_keychain_operations() -> &'static SecKeychainOperations {
    static DEFAULT_OPS: OnceLock<SecKeychainOperations> = OnceLock::new();
    DEFAULT_OPS.get_or_init(|| SecKeychainOperations::new(None))
}

#[cfg(target_os = "macos")]
impl MacosKeychainOperations for SecKeychainOperations {
    fn read_generic_password(
        &self,
        service: &str,
        account: &str,
        allow_interaction: bool,
    ) -> Result<Zeroizing<Vec<u8>>, i32> {
        use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
        use security_framework::os::macos::passwords::find_generic_password;

        let cache_key = (service.to_owned(), account.to_owned());
        if allow_interaction {
            self.interaction_required_cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&cache_key);
        }

        with_macos_keychain_interaction(allow_interaction, || {
            let keychains = self.keychain.as_ref().map(std::slice::from_ref);
            if !allow_interaction {
                // First check with `kSecUseAuthenticationUISkip` (`skip_authenticated_items`).
                // If an item exists in the keychain without authentication requirements,
                // `find_generic_password` under `SecKeychainSetUserInteractionAllowed(0)`
                // reads its bytes; if the keychain is locked or the item requires UI, it
                // immediately fails with `errSecInteractionNotAllowed` (-25308) without prompting.
                let skip_outcome = if self.simulate_authenticated_skip {
                    Err(os_status::ERR_SEC_ITEM_NOT_FOUND)
                } else {
                    let mut skip_search = ItemSearchOptions::new();
                    skip_search
                        .class(ItemClass::generic_password())
                        .service(service)
                        .account(account)
                        .limit(Limit::Max(1))
                        .load_attributes(true)
                        .skip_authenticated_items(true);
                    if let Some(kc) = keychains {
                        skip_search.keychains(kc);
                    }
                    skip_search.search().map_err(|e| e.code())
                };
                match skip_outcome {
                    Ok(_) => {
                        self.interaction_required_cache
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .remove(&cache_key);
                    }
                    Err(code) if code == os_status::ERR_SEC_ITEM_NOT_FOUND => {
                        if self
                            .interaction_required_cache
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .contains(&cache_key)
                        {
                            return Err(os_status::ERR_SEC_INTERACTION_NOT_ALLOWED);
                        }
                        if is_macos_user_prompt_active() {
                            return Err(os_status::ERR_SEC_INTERACTION_NOT_ALLOWED);
                        }
                        // Distinguish a truly missing item from an item skipped because it requires
                        // authentication by checking attributes without `skip_authenticated_items`
                        // while `SecKeychainSetUserInteractionAllowed(0)` is held, caching positive
                        // `InteractionRequired` hits so subsequent background refreshes do not
                        // repeat the unfiltered query.
                        let mut attr_probe = ItemSearchOptions::new();
                        attr_probe
                            .class(ItemClass::generic_password())
                            .service(service)
                            .account(account)
                            .limit(Limit::Max(1))
                            .load_attributes(true);
                        if let Some(kc) = keychains {
                            attr_probe.keychains(kc);
                        }
                        return match attr_probe.search() {
                            Ok(_) => {
                                self.interaction_required_cache
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .insert(cache_key);
                                Err(os_status::ERR_SEC_INTERACTION_NOT_ALLOWED)
                            }
                            Err(probe_err) => Err(probe_err.code()),
                        };
                    }
                    Err(code) => return Err(code),
                }
            }

            let (password, _item) =
                find_generic_password(keychains, service, account).map_err(|e| e.code())?;
            Ok(Zeroizing::new(password.as_ref().to_vec()))
        })
    }

    fn write_generic_password(
        &self,
        service: &str,
        account: &str,
        label: &str,
        password: &[u8],
    ) -> Result<(), i32> {
        use security_framework::item::{
            ItemClass, ItemSearchOptions, ItemUpdateOptions, update_item,
        };
        use security_framework::os::macos::keychain::SecKeychain;

        let cache_key = (service.to_owned(), account.to_owned());
        self.interaction_required_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&cache_key);

        with_macos_keychain_interaction(true, || {
            let target_keychain = match &self.keychain {
                Some(kc) => kc.clone(),
                None => SecKeychain::default().map_err(|e| e.code())?,
            };
            target_keychain
                .set_generic_password(service, account, password)
                .map_err(|e| e.code())?;

            let mut search = ItemSearchOptions::new();
            search
                .class(ItemClass::generic_password())
                .keychains(std::slice::from_ref(&target_keychain))
                .service(service)
                .account(account);
            let mut update = ItemUpdateOptions::new();
            update.set_label(label);
            if update_item(&search, &update).is_err() {
                LABEL_UPDATE_WARNING_COUNT.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        })
    }
}

/// Reads non-secret attribute dictionary entries (`acct`, `svce`, `labl`, etc.) for an item in
/// an isolated `SecKeychain` without requesting secret data or allowing UI interaction.
#[cfg(target_os = "macos")]
pub fn read_keychain_item_attributes(
    keychain: &security_framework::os::macos::keychain::SecKeychain,
    service: &str,
    account: &str,
) -> Result<std::collections::HashMap<String, String>, i32> {
    use security_framework::item::{ItemClass, ItemSearchOptions, Limit, SearchResult};

    with_macos_keychain_interaction(false, || {
        let mut search = ItemSearchOptions::new();
        search
            .class(ItemClass::generic_password())
            .keychains(std::slice::from_ref(keychain))
            .service(service)
            .account(account)
            .limit(Limit::Max(1))
            .load_attributes(true)
            .skip_authenticated_items(true);
        let results = search.search().map_err(|e| e.code())?;
        for result in results {
            if let SearchResult::Dict(_) = &result
                && let Some(dict) = result.simplify_dict()
            {
                return Ok(dict);
            }
        }
        Err(os_status::ERR_SEC_ITEM_NOT_FOUND)
    })
}

#[cfg(target_os = "macos")]
fn lookup_secret_macos(
    ops: Option<&dyn MacosKeychainOperations>,
    id: &SecretId,
    call: &BrokerCall,
    user_initiated: bool,
) -> Result<SecretLookup, SecretStoreError> {
    if call.cancel.is_cancelled() {
        return Err(SecretStoreError::Cancelled);
    }

    let ops = ops.unwrap_or_else(|| default_sec_keychain_operations());
    let service = id.keychain_service();
    let account = id.keychain_account();

    let result = with_macos_keychain_interaction(user_initiated, || {
        ops.read_generic_password(service, &account, user_initiated)
    });

    match result {
        Ok(mut secret) => {
            if secret.is_empty() {
                return Ok(SecretLookup::Invalid);
            }
            let bytes = std::mem::take(&mut *secret);
            let text = match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(error) => {
                    drop(Zeroizing::new(error.into_bytes()));
                    return Ok(SecretLookup::Invalid);
                }
            };
            Ok(SecretLookup::Found(SecretString::new(text)))
        }
        Err(os_status::ERR_SEC_ITEM_NOT_FOUND) => Ok(SecretLookup::Missing),
        Err(os_status::ERR_SEC_PROMPT_IN_FLIGHT_BUSY) if !user_initiated => Ok(SecretLookup::Busy),
        Err(os_status::ERR_SEC_PROMPT_IN_FLIGHT_BUSY) => Err(SecretStoreError::Busy),
        Err(
            os_status::ERR_SEC_INTERACTION_NOT_ALLOWED
            | os_status::ERR_SEC_AUTH_FAILED
            | os_status::ERR_SEC_NO_ACCESS_FOR_ITEM
            | os_status::ERR_SEC_USER_CANCELED,
        ) if !user_initiated => Ok(SecretLookup::Locked),
        Err(os_status::ERR_SEC_USER_CANCELED) => Err(SecretStoreError::Dismissed),
        Err(
            os_status::ERR_SEC_INTERACTION_NOT_ALLOWED
            | os_status::ERR_SEC_AUTH_FAILED
            | os_status::ERR_SEC_NO_ACCESS_FOR_ITEM,
        ) => Err(SecretStoreError::Locked),
        Err(
            os_status::ERR_SEC_NOT_AVAILABLE
            | os_status::ERR_SEC_NO_SUCH_KEYCHAIN
            | os_status::ERR_SEC_INVALID_KEYCHAIN
            | os_status::ERR_SEC_UNIMPLEMENTED,
        ) => {
            if user_initiated {
                Err(SecretStoreError::Unavailable)
            } else {
                Ok(SecretLookup::Unavailable)
            }
        }
        Err(_) => Err(SecretStoreError::Storage),
    }
}

#[cfg(target_os = "macos")]
fn set_secret_macos(
    ops: Option<&dyn MacosKeychainOperations>,
    id: &SecretId,
    secret: &SecretString,
    call: &BrokerCall,
) -> Result<(), SecretStoreError> {
    if call.cancel.is_cancelled() {
        return Err(SecretStoreError::Cancelled);
    }

    let ops = ops.unwrap_or_else(|| default_sec_keychain_operations());
    let service = id.keychain_service();
    let account = id.keychain_account();
    let label = id.label();

    let result = with_macos_keychain_interaction(true, || {
        ops.write_generic_password(service, &account, &label, secret.expose_secret().as_bytes())
    });

    match result {
        Ok(()) => Ok(()),
        Err(os_status::ERR_SEC_USER_CANCELED) => Err(SecretStoreError::Dismissed),
        Err(
            os_status::ERR_SEC_INTERACTION_NOT_ALLOWED
            | os_status::ERR_SEC_AUTH_FAILED
            | os_status::ERR_SEC_NO_ACCESS_FOR_ITEM,
        ) => Err(SecretStoreError::Locked),
        Err(
            os_status::ERR_SEC_NOT_AVAILABLE
            | os_status::ERR_SEC_NO_SUCH_KEYCHAIN
            | os_status::ERR_SEC_INVALID_KEYCHAIN
            | os_status::ERR_SEC_UNIMPLEMENTED,
        ) => Err(SecretStoreError::Unavailable),
        Err(_) => Err(SecretStoreError::Storage),
    }
}

#[cfg(target_os = "macos")]
fn chromium_safe_storage_service_account(
    app_attribute: &str,
) -> Option<(&'static str, &'static str)> {
    match app_attribute {
        "chrome" | "Chrome" | "Google Chrome" => Some(("Chrome Safe Storage", "Chrome")),
        "chromium" | "Chromium" => Some(("Chromium Safe Storage", "Chromium")),
        "brave" | "Brave" | "Brave Browser" => Some(("Brave Safe Storage", "Brave")),
        "vivaldi" | "Vivaldi" => Some(("Vivaldi Safe Storage", "Vivaldi")),
        "edge" | "Edge" | "microsoft-edge" | "Microsoft Edge" => {
            Some(("Microsoft Edge Safe Storage", "Microsoft Edge"))
        }
        "opera" | "Opera" => Some(("Opera Safe Storage", "Opera")),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn chromium_key_macos(
    ops: Option<&dyn MacosKeychainOperations>,
    app_attribute: &str,
    call: &BrokerCall,
    user_initiated: bool,
) -> Result<Option<Zeroizing<Vec<u8>>>, SecretStoreError> {
    if call.cancel.is_cancelled() {
        return Err(SecretStoreError::Cancelled);
    }

    let Some((service, account)) = chromium_safe_storage_service_account(app_attribute) else {
        return Err(SecretStoreError::Unsupported);
    };

    let ops = ops.unwrap_or_else(|| default_sec_keychain_operations());

    let result = with_macos_keychain_interaction(user_initiated, || {
        ops.read_generic_password(service, account, user_initiated)
    });

    match result {
        Ok(key) => {
            if key.is_empty() {
                return Err(SecretStoreError::Invalid);
            }
            Ok(Some(key))
        }
        Err(os_status::ERR_SEC_ITEM_NOT_FOUND) => Err(SecretStoreError::Unsupported),
        Err(os_status::ERR_SEC_PROMPT_IN_FLIGHT_BUSY) => Err(SecretStoreError::Busy),
        Err(os_status::ERR_SEC_USER_CANCELED) => Err(SecretStoreError::Dismissed),
        Err(os_status::ERR_SEC_AUTH_FAILED | os_status::ERR_SEC_NO_ACCESS_FOR_ITEM) => {
            if user_initiated {
                Err(SecretStoreError::Dismissed)
            } else {
                Err(SecretStoreError::Locked)
            }
        }
        Err(os_status::ERR_SEC_INTERACTION_NOT_ALLOWED) => Err(SecretStoreError::Locked),
        Err(
            os_status::ERR_SEC_NOT_AVAILABLE
            | os_status::ERR_SEC_NO_SUCH_KEYCHAIN
            | os_status::ERR_SEC_INVALID_KEYCHAIN
            | os_status::ERR_SEC_UNIMPLEMENTED,
        ) => Err(SecretStoreError::Unavailable),
        Err(_) => Err(SecretStoreError::Storage),
    }
}

#[cfg(target_os = "linux")]
async fn lookup_secret(
    id: &SecretId,
    call: &BrokerCall,
    user_initiated: bool,
) -> Result<SecretLookup, SecretStoreError> {
    use zeroize::Zeroizing;

    let service = connect(call).await?;
    let collection = timed(call, DBUS_TIMEOUT, service.get_default_collection()).await?;
    let attrs = id.attributes();
    let search = attrs
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<HashMap<_, _>>();
    let items = timed(call, DBUS_TIMEOUT, collection.search_items(search)).await?;
    if items.is_empty() {
        return Ok(SecretLookup::Missing);
    }

    let mut found_locked = false;
    for item in items {
        let locked = timed(call, DBUS_TIMEOUT, item.is_locked()).await?;
        if locked {
            found_locked = true;
            if !user_initiated {
                continue;
            }
            timed(call, USER_UNLOCK_TIMEOUT, item.unlock()).await?;
        }
        let mut secret = Zeroizing::new(timed(call, DBUS_TIMEOUT, item.get_secret()).await?);
        if secret.is_empty() {
            return Ok(SecretLookup::Invalid);
        }
        let bytes = std::mem::take(&mut *secret);
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => {
                drop(Zeroizing::new(error.into_bytes()));
                return Ok(SecretLookup::Invalid);
            }
        };
        return Ok(SecretLookup::Found(SecretString::new(text)));
    }

    if found_locked {
        Ok(SecretLookup::Locked)
    } else {
        Ok(SecretLookup::Missing)
    }
}

#[cfg(target_os = "linux")]
async fn set_secret(
    id: &SecretId,
    secret: &SecretString,
    call: &BrokerCall,
) -> Result<(), SecretStoreError> {
    let service = connect(call).await?;
    let collection = timed(call, DBUS_TIMEOUT, service.get_default_collection()).await?;
    if timed(call, DBUS_TIMEOUT, collection.is_locked()).await? {
        timed(call, USER_UNLOCK_TIMEOUT, collection.unlock()).await?;
        if timed(call, DBUS_TIMEOUT, collection.is_locked()).await? {
            return Err(SecretStoreError::Locked);
        }
    }

    let attrs = id.attributes();
    let properties = attrs
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<HashMap<_, _>>();
    timed(
        call,
        DBUS_TIMEOUT,
        collection.create_item(
            &id.label(),
            properties,
            secret.expose_secret().as_bytes(),
            true,
            "text/plain",
        ),
    )
    .await?;
    Ok(())
}

#[cfg(target_os = "linux")]
async fn chromium_key(
    app_attribute: &str,
    call: &BrokerCall,
    user_initiated: bool,
) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, SecretStoreError> {
    use std::collections::HashMap;

    use zeroize::Zeroizing;

    let service = connect(call).await?;
    let results = timed(
        call,
        DBUS_TIMEOUT,
        service.search_items(HashMap::from([("application", app_attribute)])),
    )
    .await?;

    let (item, locked) = if let Some(item) = results.unlocked.into_iter().next() {
        (item, false)
    } else if let Some(item) = results.locked.into_iter().next() {
        (item, true)
    } else {
        return Err(SecretStoreError::Unsupported);
    };

    if locked {
        if !user_initiated {
            return Err(SecretStoreError::Locked);
        }
        timed(call, USER_UNLOCK_TIMEOUT, item.unlock()).await?;
    }
    if timed(call, DBUS_TIMEOUT, item.is_locked()).await? {
        return Err(SecretStoreError::Locked);
    }
    let key = Zeroizing::new(timed(call, DBUS_TIMEOUT, item.get_secret()).await?);
    if key.is_empty() {
        return Err(SecretStoreError::Invalid);
    }
    Ok(Some(key))
}

#[cfg(target_os = "linux")]
async fn connect(
    call: &BrokerCall,
) -> Result<secret_service::SecretService<'static>, SecretStoreError> {
    use secret_service::{EncryptionType, SecretService};

    tokio::select! {
        _ = call.cancel.cancelled() => Err(SecretStoreError::Cancelled),
        result = tokio::time::timeout(DBUS_TIMEOUT, SecretService::connect(EncryptionType::Dh)) => {
            match result {
                Err(_) => Err(SecretStoreError::Timeout),
                Ok(Err(_)) => Err(SecretStoreError::Unavailable),
                Ok(Ok(service)) => Ok(service),
            }
        }
    }
}

#[cfg(target_os = "linux")]
async fn timed<F, T>(
    call: &BrokerCall,
    duration: Duration,
    future: F,
) -> Result<T, SecretStoreError>
where
    F: Future<Output = Result<T, secret_service::Error>>,
{
    tokio::select! {
        _ = call.cancel.cancelled() => Err(SecretStoreError::Cancelled),
        result = tokio::time::timeout(duration, future) => {
            match result {
                Err(_) => Err(SecretStoreError::Timeout),
                Ok(result) => result.map_err(map_error),
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn map_error(error: secret_service::Error) -> SecretStoreError {
    use secret_service::Error;

    match error {
        Error::Locked => SecretStoreError::Locked,
        Error::Prompt => SecretStoreError::Dismissed,
        Error::Unavailable | Error::NoResult => SecretStoreError::Unavailable,
        _ => SecretStoreError::Storage,
    }
}
