// Ported from CodexBar Sources/CodexBar/AppNotifications.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! macOS user notification bridge (`UNUserNotificationCenter`) with a testable
//! [`UserNotificationBackend`] seam so unit and integration tests never trigger
//! real OS notifications.

use std::sync::atomic::{AtomicU64, Ordering};

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::{NSArray, NSBundle, NSString};

#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

static NEXT_FALLBACK_NOTIFICATION_SEQ: AtomicU64 = AtomicU64::new(1);

/// Sanitized desktop notification payload rendered by the macOS app process.
///
/// Security invariant: `title` and `body` never contain a token, an email, or
/// an account identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacOsNotice {
    pub identifier: String,
    pub title: String,
    pub body: String,
    pub sound_enabled: bool,
}

impl MacOsNotice {
    pub fn from_ipc(
        title: Option<String>,
        message: String,
        notification_id: Option<String>,
        sound: Option<bool>,
    ) -> Self {
        let identifier = notification_id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                let seq = NEXT_FALLBACK_NOTIFICATION_SEQ.fetch_add(1, Ordering::Relaxed);
                format!("ariadusage-notice-{seq}")
            });
        let title = title
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "AriadUsage".to_string());
        Self {
            identifier,
            title,
            body: message,
            sound_enabled: sound.unwrap_or(true),
        }
    }
}

/// Boundary for posting or removing macOS user notifications.
///
/// Production code uses [`UNUserNotificationCenterBackend`]; tests inject
/// [`RecordingUserNotificationBackend`] so CI never triggers a real OS banner.
pub trait UserNotificationBackend {
    fn post(&self, notice: &MacOsNotice);
    fn remove(&self, identifier: &str);
}

/// Returns `true` only when the current process runs inside an `.app` bundle
/// with a non-empty `CFBundleIdentifier` (`AppNotifications.isRunningUnderTests`
/// bundle check). Calling `UNUserNotificationCenter.currentNotificationCenter`
/// from an unbundled CLI/test binary throws an Objective-C exception on macOS.
pub fn has_app_bundle_identity() -> bool {
    let bundle = NSBundle::mainBundle();
    let Some(bundle_id) = bundle.bundleIdentifier() else {
        return false;
    };
    if bundle_id.to_string().trim().is_empty() {
        return false;
    }
    let bundle_path = bundle.bundlePath().to_string();
    std::path::Path::new(&bundle_path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
}

/// Production notification backend backed by macOS `UNUserNotificationCenter`.
#[derive(Debug, Default, Clone, Copy)]
pub struct UNUserNotificationCenterBackend;

impl UNUserNotificationCenterBackend {
    pub const fn new() -> Self {
        Self
    }
}

impl UserNotificationBackend for UNUserNotificationCenterBackend {
    fn post(&self, notice: &MacOsNotice) {
        if !has_app_bundle_identity() {
            return;
        }
        let Some(center_cls) = AnyClass::get(c"UNUserNotificationCenter") else {
            return;
        };
        let Some(content_cls) = AnyClass::get(c"UNMutableNotificationContent") else {
            return;
        };
        let Some(request_cls) = AnyClass::get(c"UNNotificationRequest") else {
            return;
        };

        // SAFETY: `has_app_bundle_identity()` verified that `NSBundle.mainBundle` is an `.app`
        // bundle with a valid bundle identifier, satisfying `UNUserNotificationCenter`'s
        // precondition for `currentNotificationCenter`.
        let center: Option<Retained<AnyObject>> =
            unsafe { msg_send![center_cls, currentNotificationCenter] };
        let Some(center) = center else {
            return;
        };

        // SAFETY: `UNMutableNotificationContent` is a standard Objective-C class whose
        // `+new` selector returns a freshly allocated and initialized instance.
        let content: Option<Retained<AnyObject>> = unsafe { msg_send![content_cls, new] };
        let Some(content) = content else {
            return;
        };

        let ns_title = NSString::from_str(&notice.title);
        let ns_body = NSString::from_str(&notice.body);
        // SAFETY: `setTitle:` and `setBody:` take an `NSString *` on `UNMutableNotificationContent`.
        unsafe {
            let _: () = msg_send![&content, setTitle: &*ns_title];
            let _: () = msg_send![&content, setBody: &*ns_body];
        }

        if notice.sound_enabled
            && let Some(sound_cls) = AnyClass::get(c"UNNotificationSound")
        {
            // SAFETY: `+[UNNotificationSound defaultSound]` returns a valid `UNNotificationSound *`
            // and `-[UNMutableNotificationContent setSound:]` accepts `UNNotificationSound *`.
            unsafe {
                let default_sound: Option<Retained<AnyObject>> = msg_send![sound_cls, defaultSound];
                if let Some(sound) = default_sound {
                    let _: () = msg_send![&content, setSound: &*sound];
                }
            }
        }

        let ns_identifier = NSString::from_str(&notice.identifier);
        let nil_trigger: *const AnyObject = std::ptr::null();
        let nil_completion: *const AnyObject = std::ptr::null();

        // SAFETY: `+[UNNotificationRequest requestWithIdentifier:content:trigger:]` accepts a
        // non-null `NSString *`, a non-null `UNNotificationContent *`, and a nullable trigger.
        // `-[UNUserNotificationCenter addNotificationRequest:withCompletionHandler:]` accepts a
        // non-null `UNNotificationRequest *` and a nullable completion block.
        unsafe {
            let request: Option<Retained<AnyObject>> = msg_send![
                request_cls,
                requestWithIdentifier: &*ns_identifier,
                content: &*content,
                trigger: nil_trigger,
            ];
            if let Some(request) = request {
                let _: () = msg_send![
                    &center,
                    addNotificationRequest: &*request,
                    withCompletionHandler: nil_completion,
                ];
            }
        }
    }

    fn remove(&self, identifier: &str) {
        if !has_app_bundle_identity() {
            return;
        }
        let Some(center_cls) = AnyClass::get(c"UNUserNotificationCenter") else {
            return;
        };
        // SAFETY: `has_app_bundle_identity()` verified that `NSBundle.mainBundle` is an `.app`
        // bundle with a valid bundle identifier.
        let center: Option<Retained<AnyObject>> =
            unsafe { msg_send![center_cls, currentNotificationCenter] };
        let Some(center) = center else {
            return;
        };

        let ns_id = NSString::from_str(identifier);
        let ids = NSArray::from_retained_slice(&[ns_id]);
        // SAFETY: `removePendingNotificationRequestsWithIdentifiers:` and
        // `removeDeliveredNotificationsWithIdentifiers:` take an `NSArray<NSString *> *`.
        unsafe {
            let _: () = msg_send![&center, removePendingNotificationRequestsWithIdentifiers: &*ids];
            let _: () = msg_send![&center, removeDeliveredNotificationsWithIdentifiers: &*ids];
        }
    }
}

/// In-memory [`UserNotificationBackend`] for tests that records posted and removed notices
/// without calling `UNUserNotificationCenter`.
#[derive(Debug, Default, Clone)]
pub struct RecordingUserNotificationBackend {
    posted: std::rc::Rc<std::cell::RefCell<Vec<MacOsNotice>>>,
    removed: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
}

impl RecordingUserNotificationBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn posted(&self) -> Vec<MacOsNotice> {
        self.posted.borrow().clone()
    }

    pub fn removed(&self) -> Vec<String> {
        self.removed.borrow().clone()
    }
}

impl UserNotificationBackend for RecordingUserNotificationBackend {
    fn post(&self, notice: &MacOsNotice) {
        self.posted.borrow_mut().push(notice.clone());
    }

    fn remove(&self, identifier: &str) {
        self.removed.borrow_mut().push(identifier.to_string());
    }
}
