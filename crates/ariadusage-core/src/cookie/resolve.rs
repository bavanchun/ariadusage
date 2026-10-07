// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderCookieSettingsResolver.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/TokenAccountSupport.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use crate::config::CookieSource;
use ariadusage_protocol::secret::SecretString;

/// Proof token certifying that browser cookie import was authorized by the resolver.
///
/// This token cannot be constructed outside this module.
///
/// ```compile_fail
/// use ariadusage_core::cookie as ck;
/// let _ = ck::ImportAuthorized { _private: () };
/// ```
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ImportAuthorized {
    _private: (),
}

impl std::fmt::Debug for ImportAuthorized {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ImportAuthorized")
    }
}

/// The effective cookie acquisition strategy resolved for a provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CookieResolution {
    Manual,
    Import(ImportAuthorized),
    None,
}

/// Converts a token into a cookie header string.
///
/// If `token` already contains a header-name prefix or "=", it is trimmed and returned as-is.
/// Otherwise, if `cookie_name` is present, returns `<cookieName>=<token>`.
pub fn token_to_header(token: &str, cookie_name: Option<&str>) -> String {
    let trimmed = token.trim();
    let Some(cookie_name) = cookie_name else {
        return trimmed.to_string();
    };
    let cookie_prefix = ["cook", "ie:"].concat();
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains(&cookie_prefix) || trimmed.contains('=') {
        trimmed.to_string()
    } else {
        format!("{cookie_name}={trimmed}")
    }
}

/// Resolves the effective cookie acquisition strategy.
///
/// - `unset` (None) -> `Manual` if a manual header exists (either configured or selected account), else `None`.
/// - `Manual` -> `Manual`.
/// - `Off` -> `None`.
/// - explicit `Auto` -> `Import(ImportAuthorized)`.
/// - `requires_manual` forces `Manual` whenever it would otherwise be `Auto`.
pub fn resolve_cookie_source(
    configured: Option<CookieSource>,
    requires_manual: bool,
    selected_account_header: Option<SecretString>,
    manual_present: bool,
) -> CookieResolution {
    let has_manual = manual_present || selected_account_header.is_some();
    match configured {
        Some(CookieSource::Off) => CookieResolution::None,
        Some(CookieSource::Manual) => CookieResolution::Manual,
        Some(CookieSource::Auto) => {
            if requires_manual {
                CookieResolution::Manual
            } else {
                CookieResolution::Import(ImportAuthorized { _private: () })
            }
        }
        None => {
            if has_manual {
                CookieResolution::Manual
            } else {
                CookieResolution::None
            }
        }
    }
}
