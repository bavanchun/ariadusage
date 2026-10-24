// Ported from CodexBar Sources/CodexBarCore/OpenAIWeb/OpenAIDashboardBrowserCookieImporter.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/OpenAIWeb/OpenAIDashboardBrowserCookieImporter+Deadline.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+OpenAIWeb.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use ariadusage_core::config::CookieSource;
use ariadusage_core::cookie::{
    CookieRecord, CookieResolution, DeclaredDomains, ImportAuthorized, normalize, pairs,
    resolve_cookie_source,
};
use ariadusage_core::pipeline::FetchPhase;
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;
use serde_json::Value;

use crate::brokers::browser::{
    Browser, BrowserBroker, BrowserError, CookieQuery, DEFAULT_IMPORT_ORDER,
};
use crate::brokers::call::BrokerCall;
use crate::brokers::cookie_cache::{CacheScope, CookieCache};
use crate::brokers::net::NetBroker;

use super::openai_dashboard_fetcher::{
    OpenAIDashboardEndpointConfig, OpenAIDashboardFetchError, OpenAIDashboardFetcher,
};
use super::openai_dashboard_models::OpenAIDashboardSnapshot;

pub const OPENAI_COOKIE_DOMAINS: [&str; 2] = ["chatgpt.com", "openai.com"];

static OPENAI_DECLARED_DOMAINS: LazyLock<DeclaredDomains> = LazyLock::new(|| {
    DeclaredDomains::new(OPENAI_COOKIE_DOMAINS).expect("valid OpenAI cookie domains")
});

pub fn openai_declared_domains() -> &'static DeclaredDomains {
    &OPENAI_DECLARED_DOMAINS
}

fn codex_provider_id() -> ProviderId {
    ProviderId::new("codex").expect("valid codex provider id")
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FoundAccount {
    pub source_label: String,
    pub email: String,
}

impl FoundAccount {
    pub fn new(source_label: impl Into<String>, email: impl Into<String>) -> Self {
        Self {
            source_label: source_label.into(),
            email: email.into(),
        }
    }
}

impl fmt::Debug for FoundAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FoundAccount")
            .field("source_label", &self.source_label)
            .field("email", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum OpenAIDashboardCookieImportError {
    NoCookiesFound,
    BrowserAccessDenied { details: String },
    BrowserCookieLoadTimedOut { details: String },
    DashboardStillRequiresLogin,
    NoMatchingAccount { found: Vec<FoundAccount> },
    ManualCookieHeaderInvalid,
}

impl fmt::Debug for OpenAIDashboardCookieImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCookiesFound => f.write_str("NoCookiesFound"),
            Self::BrowserAccessDenied { details } => f
                .debug_struct("BrowserAccessDenied")
                .field("details", details)
                .finish(),
            Self::BrowserCookieLoadTimedOut { details } => f
                .debug_struct("BrowserCookieLoadTimedOut")
                .field("details", details)
                .finish(),
            Self::DashboardStillRequiresLogin => f.write_str("DashboardStillRequiresLogin"),
            Self::NoMatchingAccount { found } => f
                .debug_struct("NoMatchingAccount")
                .field("found_count", &found.len())
                .finish(),
            Self::ManualCookieHeaderInvalid => f.write_str("ManualCookieHeaderInvalid"),
        }
    }
}

impl fmt::Display for OpenAIDashboardCookieImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCookiesFound => f.write_str("No browser cookies found."),
            Self::BrowserAccessDenied { details } => {
                write!(f, "Browser cookie access denied. {details}")
            }
            Self::BrowserCookieLoadTimedOut { details } => {
                write!(f, "Browser cookie loading timed out. {details}")
            }
            Self::DashboardStillRequiresLogin => {
                f.write_str("Browser cookies imported, but dashboard still requires login.")
            }
            Self::NoMatchingAccount { found } => {
                if found.is_empty() {
                    return f.write_str("No matching OpenAI web session found in browsers.");
                }
                let mut sorted = found.clone();
                sorted.sort();
                let display = sorted
                    .iter()
                    .map(|item| format!("{}={}", item.source_label, item.email))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "OpenAI web session does not match Codex account. Found: {display}."
                )
            }
            Self::ManualCookieHeaderInvalid => {
                f.write_str("Manual cookie header is missing a valid OpenAI session cookie.")
            }
        }
    }
}

impl std::error::Error for OpenAIDashboardCookieImportError {}

#[derive(Clone, PartialEq, Eq)]
pub struct OpenAIDashboardCookieImportResult {
    pub source_label: String,
    pub cookie_count: usize,
    pub signed_in_email: Option<String>,
    pub matches_codex_email: bool,
    pub cookie_header: SecretString,
}

impl fmt::Debug for OpenAIDashboardCookieImportResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAIDashboardCookieImportResult")
            .field("source_label", &self.source_label)
            .field("cookie_count", &self.cookie_count)
            .field(
                "signed_in_email",
                &self.signed_in_email.as_ref().map(|_| "[redacted]"),
            )
            .field("matches_codex_email", &self.matches_codex_email)
            .field("cookie_header", &"[redacted]")
            .finish()
    }
}

#[derive(Clone)]
pub struct OpenAIDashboardCookieCandidate {
    pub label: String,
    pub cookie_header: SecretString,
    pub cookie_count: usize,
    pub has_session_cookie: bool,
}

impl OpenAIDashboardCookieCandidate {
    pub fn from_cookie_header(
        label: impl Into<String>,
        cookie_header: impl Into<SecretString>,
    ) -> Option<Self> {
        let secret = cookie_header.into();
        let normalized = normalize(secret.expose_secret())?;
        let parsed_pairs = pairs(&normalized);
        if parsed_pairs.is_empty() {
            return None;
        }
        let has_session_cookie = parsed_pairs
            .iter()
            .any(|(name, _)| is_openai_session_cookie_name(name));
        let cookie_count = parsed_pairs.len() * OPENAI_COOKIE_DOMAINS.len();
        Some(Self {
            label: label.into(),
            cookie_header: SecretString::from(normalized),
            cookie_count,
            has_session_cookie,
        })
    }

    pub fn from_records(label: impl Into<String>, records: &[CookieRecord]) -> Option<Self> {
        if records.is_empty() {
            return None;
        }
        let chatgpt_or_openai: Vec<&CookieRecord> = records
            .iter()
            .filter(|rec| {
                let d = rec.domain.to_ascii_lowercase();
                d.contains("chatgpt.com") || d.contains("openai.com")
            })
            .collect();
        if chatgpt_or_openai.is_empty() {
            return None;
        }
        let has_session_cookie = chatgpt_or_openai
            .iter()
            .any(|rec| is_openai_session_cookie_name(&rec.name));
        let raw_header = chatgpt_or_openai
            .iter()
            .map(|rec| format!("{}={}", rec.name, rec.value.expose_secret()))
            .collect::<Vec<_>>()
            .join("; ");
        let normalized = normalize(&raw_header)?;
        Some(Self {
            label: label.into(),
            cookie_header: SecretString::from(normalized),
            cookie_count: chatgpt_or_openai.len(),
            has_session_cookie,
        })
    }
}

impl fmt::Debug for OpenAIDashboardCookieCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAIDashboardCookieCandidate")
            .field("label", &self.label)
            .field("cookie_count", &self.cookie_count)
            .field("has_session_cookie", &self.has_session_cookie)
            .finish()
    }
}

pub fn is_openai_session_cookie_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    lower.contains("session-token")
        || lower.contains("authjs")
        || lower.contains("next-auth")
        || lower == "_account"
}

pub fn has_session_cookies_in_header(cookie_header: &SecretString) -> bool {
    pairs(cookie_header.expose_secret())
        .iter()
        .any(|(name, _)| is_openai_session_cookie_name(name))
}

pub fn cookie_summary_from_header(cookie_header: &SecretString) -> String {
    let parsed = pairs(cookie_header.expose_secret());
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (name, _) in &parsed {
        *counts.entry(name.as_str()).or_insert(0) += 1;
    }
    let important = [
        "__Secure-next-auth.session-token",
        "__Secure-next-auth.session-token.0",
        "__Secure-next-auth.session-token.1",
        "_account",
        "oai-did",
        "cf_clearance",
    ];
    let parts: Vec<String> = important
        .iter()
        .filter_map(|&name| {
            let count = counts.get(name).copied().unwrap_or(0);
            (count > 0).then(|| format!("{name}={count}"))
        })
        .collect();
    if parts.is_empty() {
        "no key cookies detected".to_owned()
    } else {
        parts.join(", ")
    }
}

pub fn find_first_email_in_json(data: &[u8]) -> Option<String> {
    let root: Value = serde_json::from_slice(data).ok()?;
    let mut queue: VecDeque<&Value> = VecDeque::from([&root]);
    let mut seen = 0usize;
    while let Some(cur) = queue.pop_front() {
        if seen >= 2000 {
            break;
        }
        seen += 1;
        match cur {
            Value::String(s) if s.contains('@') => {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_owned());
                }
            }
            Value::Object(map) => {
                for (k, v) in map {
                    if k.eq_ignore_ascii_case("email")
                        && let Some(s) = v.as_str()
                        && s.contains('@')
                    {
                        let trimmed = s.trim();
                        if !trimmed.is_empty() {
                            return Some(trimmed.to_owned());
                        }
                    }
                    queue.push_back(v);
                }
            }
            Value::Array(arr) => {
                for item in arr {
                    queue.push_back(item);
                }
            }
            _ => {}
        }
    }
    None
}

#[derive(Clone, Default)]
pub enum OpenAIDashboardCandidateSource {
    #[default]
    None,
    BrowserBroker(Arc<BrowserBroker>),
    StaticCandidates(Vec<OpenAIDashboardCookieCandidate>),
}

#[derive(Clone)]
pub struct OpenAIDashboardBrowserCookieImporter {
    net: Arc<NetBroker>,
    endpoint_config: OpenAIDashboardEndpointConfig,
    cookie_cache: Arc<CookieCache>,
    candidate_source: OpenAIDashboardCandidateSource,
}

impl OpenAIDashboardBrowserCookieImporter {
    pub fn new(
        net: Arc<NetBroker>,
        endpoint_config: OpenAIDashboardEndpointConfig,
        cookie_cache: Arc<CookieCache>,
        candidate_source: OpenAIDashboardCandidateSource,
    ) -> Self {
        Self {
            net,
            endpoint_config,
            cookie_cache,
            candidate_source,
        }
    }

    pub fn remaining_timeout(
        deadline: Option<Instant>,
        capped_at: Option<Duration>,
        now: Instant,
    ) -> Result<Duration, OpenAIDashboardCookieImportError> {
        let Some(deadline) = deadline else {
            return Ok(capped_at
                .map(OpenAIDashboardFetcher::sanitized_timeout)
                .unwrap_or(Duration::from_secs(60)));
        };
        let Some(remaining) = deadline.checked_duration_since(now) else {
            return Err(
                OpenAIDashboardCookieImportError::BrowserCookieLoadTimedOut {
                    details: "OpenAI web deadline expired before cookie import completed."
                        .to_owned(),
                },
            );
        };
        if remaining.is_zero() {
            return Err(
                OpenAIDashboardCookieImportError::BrowserCookieLoadTimedOut {
                    details: "OpenAI web deadline expired before cookie import completed."
                        .to_owned(),
                },
            );
        }
        Ok(match capped_at {
            Some(cap) => remaining.min(OpenAIDashboardFetcher::sanitized_timeout(cap)),
            None => remaining,
        })
    }

    pub fn browser_profile_access_hint(
        browser: Browser,
        access_denied: bool,
        process_name: &str,
        executable_path: Option<&str>,
    ) -> String {
        let failure = if access_denied {
            "macOS denied"
        } else {
            "macOS could not read"
        };
        let target = Self::full_disk_access_target(process_name, executable_path);
        format!(
            "{failure} {browser} profile access for {target}. Grant that exact component Full Disk Access, then quit and reopen it."
        )
    }

    fn full_disk_access_target(process_name: &str, executable_path: Option<&str>) -> String {
        let Some(executable) = executable_path.map(str::trim).filter(|s| !s.is_empty()) else {
            return process_name.to_owned();
        };
        if process_name != "ariadusage"
            && process_name != "CodexBarCLI"
            && let Some(idx) = executable.find(".app/")
        {
            let app_path = &executable[..idx + 4];
            let app_name = std::path::Path::new(app_path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("AriadUsage.app");
            return format!("{app_name} ({app_path})");
        }
        format!("{process_name} ({executable})")
    }

    pub fn browser_cookie_load_timeout_error(
        browser: Browser,
        process_name: &str,
        executable_path: Option<&str>,
    ) -> OpenAIDashboardCookieImportError {
        let target = Self::full_disk_access_target(process_name, executable_path);
        let uses_keychain = !matches!(browser, Browser::Safari | Browser::Firefox);
        let keychain_hint = if uses_keychain {
            " If a macOS Keychain prompt is waiting, approve it."
        } else {
            ""
        };
        let details = format!(
            "{browser} did not finish before the web timeout for {target}.{keychain_hint} If the timeout repeats, grant that exact component Full Disk Access, quit and reopen it, then retry."
        );
        OpenAIDashboardCookieImportError::BrowserCookieLoadTimedOut { details }
    }

    /// Resolves cookies honoring the explicit opt-in rule:
    /// - `None` (without manual header) or `Some(CookieSource::Off)` never touches `BrowserBroker`.
    /// - `Some(CookieSource::Manual)` (or `None` with manual header) validates the manual cookie header.
    /// - `Some(CookieSource::Auto)` imports from `CookieCache` / `BrowserBroker`.
    #[allow(clippy::too_many_arguments)]
    pub async fn import_for_cookie_source(
        &self,
        call: &BrokerCall,
        cookie_source: Option<CookieSource>,
        manual_cookie_header: Option<&SecretString>,
        target_email: Option<&str>,
        allow_any_account: bool,
        prefer_cached_cookie_header: bool,
        cache_scope: Option<&CacheScope>,
        deadline: Option<Instant>,
    ) -> Result<OpenAIDashboardCookieImportResult, OpenAIDashboardCookieImportError> {
        let manual_present = manual_cookie_header
            .and_then(|h| normalize(h.expose_secret()))
            .is_some();
        match resolve_cookie_source(cookie_source, false, None, manual_present) {
            CookieResolution::None => Err(OpenAIDashboardCookieImportError::NoCookiesFound),
            CookieResolution::Manual => {
                let Some(header) = manual_cookie_header else {
                    return Err(OpenAIDashboardCookieImportError::ManualCookieHeaderInvalid);
                };
                self.import_manual_cookies(
                    call,
                    header,
                    target_email,
                    allow_any_account,
                    cache_scope,
                    deadline,
                )
                .await
            }
            CookieResolution::Import(auth) => {
                self.import_best_cookies_authorized(
                    call,
                    auth,
                    target_email,
                    allow_any_account,
                    prefer_cached_cookie_header,
                    cache_scope,
                    deadline,
                )
                .await
            }
        }
    }

    pub async fn import_manual_cookies(
        &self,
        call: &BrokerCall,
        cookie_header: &SecretString,
        target_email: Option<&str>,
        allow_any_account: bool,
        cache_scope: Option<&CacheScope>,
        deadline: Option<Instant>,
    ) -> Result<OpenAIDashboardCookieImportResult, OpenAIDashboardCookieImportError> {
        let normalized_target = target_email.map(str::trim).filter(|s| !s.is_empty());
        let allow_any = allow_any_account || normalized_target.is_none();

        let Some(candidate) =
            OpenAIDashboardCookieCandidate::from_cookie_header("Manual", cookie_header.clone())
        else {
            return Err(OpenAIDashboardCookieImportError::ManualCookieHeaderInvalid);
        };

        match self
            .evaluate_candidate(call, &candidate, normalized_target, allow_any, deadline)
            .await?
        {
            CandidateEvaluation::Match { signed_in_email }
            | CandidateEvaluation::LoggedIn { signed_in_email } => {
                let matches = normalized_target
                    .is_some_and(|target| signed_in_email.eq_ignore_ascii_case(target));
                self.store_in_cache(&candidate, cache_scope);
                Ok(OpenAIDashboardCookieImportResult {
                    source_label: candidate.label,
                    cookie_count: candidate.cookie_count,
                    signed_in_email: Some(signed_in_email),
                    matches_codex_email: matches,
                    cookie_header: candidate.cookie_header,
                })
            }
            CandidateEvaluation::Mismatch { signed_in_email } => {
                Err(OpenAIDashboardCookieImportError::NoMatchingAccount {
                    found: vec![FoundAccount::new("Manual", signed_in_email)],
                })
            }
            CandidateEvaluation::Unknown => {
                if allow_any {
                    self.store_in_cache(&candidate, cache_scope);
                    Ok(OpenAIDashboardCookieImportResult {
                        source_label: candidate.label,
                        cookie_count: candidate.cookie_count,
                        signed_in_email: None,
                        matches_codex_email: false,
                        cookie_header: candidate.cookie_header,
                    })
                } else {
                    Err(OpenAIDashboardCookieImportError::NoMatchingAccount { found: Vec::new() })
                }
            }
            CandidateEvaluation::LoginRequired => {
                Err(OpenAIDashboardCookieImportError::ManualCookieHeaderInvalid)
            }
        }
    }

    pub async fn import_best_cookies(
        &self,
        call: &BrokerCall,
        target_email: Option<&str>,
        allow_any_account: bool,
        prefer_cached_cookie_header: bool,
        cache_scope: Option<&CacheScope>,
        deadline: Option<Instant>,
    ) -> Result<OpenAIDashboardCookieImportResult, OpenAIDashboardCookieImportError> {
        let CookieResolution::Import(auth) =
            resolve_cookie_source(Some(CookieSource::Auto), false, None, false)
        else {
            return Err(OpenAIDashboardCookieImportError::NoCookiesFound);
        };
        self.import_best_cookies_authorized(
            call,
            auth,
            target_email,
            allow_any_account,
            prefer_cached_cookie_header,
            cache_scope,
            deadline,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn import_best_cookies_authorized(
        &self,
        call: &BrokerCall,
        authorization: ImportAuthorized,
        target_email: Option<&str>,
        allow_any_account: bool,
        prefer_cached_cookie_header: bool,
        cache_scope: Option<&CacheScope>,
        deadline: Option<Instant>,
    ) -> Result<OpenAIDashboardCookieImportResult, OpenAIDashboardCookieImportError> {
        let normalized_target = target_email.map(str::trim).filter(|s| !s.is_empty());
        if normalized_target.is_none() && !allow_any_account {
            return Err(OpenAIDashboardCookieImportError::NoCookiesFound);
        }

        if prefer_cached_cookie_header
            && let Some(cached) = self.cookie_cache.load(&codex_provider_id(), cache_scope)
            && !cached.header.expose_secret().trim().is_empty()
        {
            match self
                .import_manual_cookies(
                    call,
                    &cached.header,
                    normalized_target,
                    allow_any_account,
                    cache_scope,
                    deadline,
                )
                .await
            {
                Ok(mut res) => {
                    res.source_label = cached.source_label;
                    return Ok(res);
                }
                Err(
                    OpenAIDashboardCookieImportError::ManualCookieHeaderInvalid
                    | OpenAIDashboardCookieImportError::NoMatchingAccount { .. }
                    | OpenAIDashboardCookieImportError::DashboardStillRequiresLogin,
                ) => {
                    self.cookie_cache.clear(&codex_provider_id(), cache_scope);
                }
                Err(other) => return Err(other),
            }
        }

        let mut mismatches: BTreeSet<FoundAccount> = BTreeSet::new();
        let mut found_any_cookies = false;
        let mut found_unknown_email = false;
        let mut access_denied_hints: BTreeSet<String> = BTreeSet::new();

        match &self.candidate_source {
            OpenAIDashboardCandidateSource::None => {}
            OpenAIDashboardCandidateSource::StaticCandidates(candidates) => {
                for candidate in candidates {
                    Self::remaining_timeout(deadline, None, Instant::now())?;
                    found_any_cookies = true;
                    if let Some(matched) = self
                        .apply_candidate(
                            call,
                            candidate,
                            normalized_target,
                            allow_any_account,
                            cache_scope,
                            deadline,
                            &mut mismatches,
                            &mut found_unknown_email,
                        )
                        .await?
                    {
                        return Ok(matched);
                    }
                }
            }
            OpenAIDashboardCandidateSource::BrowserBroker(broker) => {
                let provider_id = codex_provider_id();
                for browser in DEFAULT_IMPORT_ORDER {
                    Self::remaining_timeout(deadline, None, Instant::now())?;
                    if call.cancel.is_cancelled() {
                        return Err(OpenAIDashboardCookieImportError::NoCookiesFound);
                    }
                    let browsers = [browser];
                    let query = CookieQuery {
                        provider: provider_id.clone(),
                        domains: openai_declared_domains(),
                        names: None,
                        browsers: &browsers,
                        acknowledged_retry: false,
                    };
                    match broker.candidates(call, authorization, query).await {
                        Ok(candidates) => {
                            for browser_candidate in candidates {
                                Self::remaining_timeout(deadline, None, Instant::now())?;
                                if browser_candidate.diagnostics.access_denied > 0 {
                                    access_denied_hints.insert(Self::browser_profile_access_hint(
                                        browser,
                                        true,
                                        "ariadusage",
                                        None,
                                    ));
                                }
                                let Some(candidate) = OpenAIDashboardCookieCandidate::from_records(
                                    browser_candidate.label,
                                    &browser_candidate.records,
                                ) else {
                                    continue;
                                };
                                found_any_cookies = true;
                                if let Some(matched) = self
                                    .apply_candidate(
                                        call,
                                        &candidate,
                                        normalized_target,
                                        allow_any_account,
                                        cache_scope,
                                        deadline,
                                        &mut mismatches,
                                        &mut found_unknown_email,
                                    )
                                    .await?
                                {
                                    return Ok(matched);
                                }
                            }
                        }
                        Err(BrowserError::Locked | BrowserError::Dismissed) => {
                            access_denied_hints.insert(Self::browser_profile_access_hint(
                                browser,
                                true,
                                "ariadusage",
                                None,
                            ));
                        }
                        Err(_) => continue,
                    }
                }
            }
        }

        Self::remaining_timeout(deadline, None, Instant::now())?;

        if !mismatches.is_empty() {
            return Err(OpenAIDashboardCookieImportError::NoMatchingAccount {
                found: mismatches.into_iter().collect(),
            });
        }

        if found_unknown_email || found_any_cookies {
            return Err(OpenAIDashboardCookieImportError::NoMatchingAccount { found: Vec::new() });
        }

        if !access_denied_hints.is_empty() {
            let details = access_denied_hints
                .into_iter()
                .collect::<Vec<_>>()
                .join(" ");
            return Err(OpenAIDashboardCookieImportError::BrowserAccessDenied { details });
        }

        Err(OpenAIDashboardCookieImportError::NoCookiesFound)
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_candidate(
        &self,
        call: &BrokerCall,
        candidate: &OpenAIDashboardCookieCandidate,
        target_email: Option<&str>,
        allow_any_account: bool,
        cache_scope: Option<&CacheScope>,
        deadline: Option<Instant>,
        mismatches: &mut BTreeSet<FoundAccount>,
        found_unknown_email: &mut bool,
    ) -> Result<Option<OpenAIDashboardCookieImportResult>, OpenAIDashboardCookieImportError> {
        match self
            .evaluate_candidate(call, candidate, target_email, allow_any_account, deadline)
            .await?
        {
            CandidateEvaluation::Match { signed_in_email } => {
                let Some(target) = target_email else {
                    return Ok(None);
                };
                self.store_in_cache(candidate, cache_scope);
                Ok(Some(OpenAIDashboardCookieImportResult {
                    source_label: candidate.label.clone(),
                    cookie_count: candidate.cookie_count,
                    matches_codex_email: signed_in_email.eq_ignore_ascii_case(target),
                    signed_in_email: Some(signed_in_email),
                    cookie_header: candidate.cookie_header.clone(),
                }))
            }
            CandidateEvaluation::Mismatch { signed_in_email } => {
                mismatches.insert(FoundAccount::new(
                    candidate.label.clone(),
                    signed_in_email.clone(),
                ));
                // Keep mismatched signed-in session inside an account-scoped cache entry without
                // overwriting the active target's cache scope.
                if cache_scope.is_none() {
                    let mismatch_scope =
                        CacheScope::provider_variant(&signed_in_email.to_ascii_lowercase());
                    self.store_in_cache(candidate, Some(&mismatch_scope));
                }
                Self::remaining_timeout(deadline, None, Instant::now())?;
                Ok(None)
            }
            CandidateEvaluation::LoggedIn { signed_in_email } => {
                self.store_in_cache(candidate, cache_scope);
                Ok(Some(OpenAIDashboardCookieImportResult {
                    source_label: candidate.label.clone(),
                    cookie_count: candidate.cookie_count,
                    signed_in_email: Some(signed_in_email),
                    matches_codex_email: false,
                    cookie_header: candidate.cookie_header.clone(),
                }))
            }
            CandidateEvaluation::Unknown => {
                if allow_any_account {
                    self.store_in_cache(candidate, cache_scope);
                    return Ok(Some(OpenAIDashboardCookieImportResult {
                        source_label: candidate.label.clone(),
                        cookie_count: candidate.cookie_count,
                        signed_in_email: None,
                        matches_codex_email: false,
                        cookie_header: candidate.cookie_header.clone(),
                    }));
                }
                *found_unknown_email = true;
                Ok(None)
            }
            CandidateEvaluation::LoginRequired => Ok(None),
        }
    }

    async fn evaluate_candidate(
        &self,
        call: &BrokerCall,
        candidate: &OpenAIDashboardCookieCandidate,
        target_email: Option<&str>,
        allow_any_account: bool,
        deadline: Option<Instant>,
    ) -> Result<CandidateEvaluation, OpenAIDashboardCookieImportError> {
        let api_email = OpenAIDashboardFetcher::fetch_signed_in_email_from_api(
            &self.net,
            &self.endpoint_config,
            candidate.cookie_header.expose_secret(),
            deadline,
            &Instant::now,
            Some(call),
        )
        .await
        .map_err(|err| match err {
            OpenAIDashboardFetchError::Timeout => {
                OpenAIDashboardCookieImportError::BrowserCookieLoadTimedOut {
                    details: "OpenAI web identity probe timed out.".to_owned(),
                }
            }
            _ => OpenAIDashboardCookieImportError::NoCookiesFound,
        })?;

        if let Some(email) = api_email
            .map(|e| e.trim().to_owned())
            .filter(|e| !e.is_empty())
        {
            if let Some(target) = target_email {
                if email.eq_ignore_ascii_case(target) {
                    return Ok(CandidateEvaluation::Match {
                        signed_in_email: email,
                    });
                }
                return Ok(CandidateEvaluation::Mismatch {
                    signed_in_email: email,
                });
            }
            if allow_any_account {
                return Ok(CandidateEvaluation::LoggedIn {
                    signed_in_email: email,
                });
            }
        }

        if !candidate.has_session_cookie {
            return Ok(CandidateEvaluation::LoginRequired);
        }

        Ok(CandidateEvaluation::Unknown)
    }

    fn store_in_cache(
        &self,
        candidate: &OpenAIDashboardCookieCandidate,
        scope: Option<&CacheScope>,
    ) {
        self.cookie_cache.store(
            &codex_provider_id(),
            scope,
            candidate.cookie_header.expose_secret(),
            &candidate.label,
            Timestamp::now(),
        );
    }
}

enum CandidateEvaluation {
    Match { signed_in_email: String },
    Mismatch { signed_in_email: String },
    LoggedIn { signed_in_email: String },
    Unknown,
    LoginRequired,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenAIWebRefreshGateContext {
    pub force: bool,
    pub account_did_change: bool,
    pub last_error: Option<String>,
    pub last_snapshot_at: Option<Timestamp>,
    pub last_attempt_at: Option<Timestamp>,
    pub now: Timestamp,
    pub refresh_interval_seconds: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenAIWebRefreshPolicyContext {
    pub access_enabled: bool,
    pub battery_saver_enabled: bool,
    pub force: bool,
    pub refresh_phase: FetchPhase,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenAIDashboardPageScrapeGate {
    pub force: bool,
    pub user_initiated: bool,
    pub has_history: bool,
    pub last_page_scrape_at: Option<Timestamp>,
    pub history_updated_at: Option<Timestamp>,
    pub now: Timestamp,
    pub refresh_interval_seconds: f64,
}

pub fn refresh_phase(has_completed_initial_refresh: bool) -> FetchPhase {
    if has_completed_initial_refresh {
        FetchPhase::Regular
    } else {
        FetchPhase::Startup
    }
}

pub fn openai_web_refresh_phase(
    provider_refresh_phase: FetchPhase,
    startup_connectivity_retry_attempt: u32,
) -> FetchPhase {
    if startup_connectivity_retry_attempt > 0 {
        FetchPhase::Startup
    } else {
        provider_refresh_phase
    }
}

pub fn should_run_openai_web_refresh(context: OpenAIWebRefreshPolicyContext) -> bool {
    if !context.access_enabled {
        return false;
    }
    if !context.force && context.refresh_phase == FetchPhase::Startup {
        return false;
    }
    context.force || !context.battery_saver_enabled
}

pub fn force_openai_web_refresh_for_stale_request(battery_saver_enabled: bool) -> bool {
    !battery_saver_enabled
}

pub fn should_skip_openai_web_refresh(context: &OpenAIWebRefreshGateContext) -> bool {
    if context.force || context.account_did_change {
        return false;
    }
    if let Some(last_attempt_at) = context.last_attempt_at {
        let elapsed = context.now.duration_since(last_attempt_at).as_secs_f64();
        if elapsed < context.refresh_interval_seconds {
            return true;
        }
    }
    if context.last_error.is_none()
        && let Some(last_snapshot_at) = context.last_snapshot_at
    {
        let elapsed = context.now.duration_since(last_snapshot_at).as_secs_f64();
        if elapsed < context.refresh_interval_seconds {
            return true;
        }
    }
    false
}

pub fn should_allow_openai_dashboard_page_scrape(gate: &OpenAIDashboardPageScrapeGate) -> bool {
    if !gate.has_history {
        return true;
    }
    if gate.force && gate.user_initiated {
        return true;
    }
    let Some(last_scrape) = gate.last_page_scrape_at.or(gate.history_updated_at) else {
        return false;
    };
    gate.now.duration_since(last_scrape).as_secs_f64() >= gate.refresh_interval_seconds
}

pub fn dashboard_has_page_history(snapshot: Option<&OpenAIDashboardSnapshot>) -> bool {
    let Some(snapshot) = snapshot else {
        return false;
    };
    !snapshot.daily_breakdown.is_empty() || !snapshot.usage_breakdown.is_empty()
}

pub fn should_skip_openai_web_empty_history_retry(context: &OpenAIWebRefreshGateContext) -> bool {
    if context.force || context.account_did_change {
        return false;
    }
    let Some(last_attempt_at) = context.last_attempt_at else {
        return false;
    };
    let elapsed = context.now.duration_since(last_attempt_at).as_secs_f64();
    if elapsed >= context.refresh_interval_seconds {
        return false;
    }
    let Some(last_snapshot_at) = context.last_snapshot_at else {
        return true;
    };
    last_attempt_at >= last_snapshot_at
}

pub fn openai_web_target_isolation_key(email: &str, scope: Option<&CacheScope>) -> String {
    let scope_id = match scope {
        None => "live".to_owned(),
        Some(scope) => scope.isolation_identifier(),
    };
    format!("{}|{scope_id}", email.trim().to_ascii_lowercase())
}

pub fn should_attempt_cookie_import(
    force: bool,
    allow_any_account: bool,
    requires_login: bool,
    normalized_target: Option<&str>,
    last_email: Option<&str>,
    last_attempt_at: Option<Timestamp>,
    now: Timestamp,
) -> bool {
    if force {
        return true;
    }
    let cooldown_elapsed = last_attempt_at
        .map(|last| now.duration_since(last).as_secs_f64() > 300.0)
        .unwrap_or(true);
    if allow_any_account {
        cooldown_elapsed
    } else {
        let email_changed = match (last_email, normalized_target) {
            (Some(a), Some(b)) => !a.trim().eq_ignore_ascii_case(b.trim()),
            (None, None) => false,
            _ => true,
        };
        requires_login && (email_changed || cooldown_elapsed)
    }
}

pub fn concise_openai_cookie_mismatch_status(
    found: &[String],
    target_email: Option<&str>,
) -> String {
    let normalized_found: Vec<String> = found
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let found_label = match normalized_found.len() {
        0 => String::new(),
        1 => normalized_found[0].clone(),
        2 => format!("{} or {}", normalized_found[0], normalized_found[1]),
        n => format!("{} or {} other accounts", normalized_found[0], n - 1),
    };

    let target_label = target_email.map(str::trim).filter(|s| !s.is_empty());
    if normalized_found.is_empty() {
        let Some(target_label) = target_label else {
            return "No matching OpenAI web session found.".to_owned();
        };
        return format!("No matching OpenAI web session found for {target_label}.");
    }
    let Some(target_label) = target_label else {
        return format!("OpenAI cookies are for {found_label}.");
    };
    format!("OpenAI cookies are for {found_label}, not {target_label}.")
}

pub fn actionable_openai_dashboard_import_failure(
    target_email: Option<&str>,
    cookie_import_status: Option<&str>,
) -> Option<String> {
    let status = cookie_import_status
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let lower = status.to_ascii_lowercase();
    if lower.contains("openai cookies are for") {
        return Some(format!(
            "{status} Switch chatgpt.com account, then refresh OpenAI cookies."
        ));
    }
    if lower.contains("no signed-in openai web session found")
        || lower.contains("no matching openai web session found")
    {
        let account_label = target_email
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("your OpenAI account");
        return Some(format!(
            "{status} Sign in to chatgpt.com as {account_label}, then refresh OpenAI cookies."
        ));
    }
    if lower.contains("openai cookie import failed")
        || lower.contains("browser cookie import failed")
    {
        return Some(status.to_owned());
    }
    None
}

pub fn openai_dashboard_friendly_error(
    body: &str,
    target_email: Option<&str>,
    cookie_import_status: Option<&str>,
) -> Option<String> {
    let trimmed = body.trim();
    let status = cookie_import_status
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if trimmed.is_empty() {
        return Some(
            "OpenAI web dashboard returned an empty page. Sign in to chatgpt.com and update OpenAI cookies in Providers → Codex."
                .to_owned(),
        );
    }

    let lower = trimmed.to_ascii_lowercase();
    let looks_like_public_landing = lower.contains("skip to content")
        && (lower.contains("about") || lower.contains("openai") || lower.contains("chatgpt"));
    let looks_logged_out = lower.contains("sign in")
        || lower.contains("log in")
        || lower.contains("create account")
        || lower.contains("continue with google")
        || lower.contains("continue with apple")
        || lower.contains("continue with microsoft");

    if !looks_like_public_landing && !looks_logged_out {
        return None;
    }

    let target_label = target_email
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("your OpenAI account");
    if let Some(status) = status {
        let lower_status = status.to_ascii_lowercase();
        if status.contains("cookies do not match Codex account")
            || lower_status.contains("openai cookies are for")
            || lower_status.contains("cookie import failed")
        {
            return Some(format!(
                "{status} Switch chatgpt.com account, then refresh OpenAI cookies."
            ));
        }
    }

    Some(format!(
        "OpenAI web dashboard returned a public page (not signed in). Sign in to chatgpt.com as {target_label}, then update OpenAI cookies in Providers → Codex."
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistentCookieSessionError {
    Import(OpenAIDashboardCookieImportError),
    Fetch(OpenAIDashboardFetchError),
    TimedOut,
    PersistentValidationTimedOut,
    Cancelled,
}

impl fmt::Display for PersistentCookieSessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Import(e) => Display::fmt(e, f),
            Self::Fetch(e) => Display::fmt(e, f),
            Self::TimedOut => f.write_str("Timed out."),
            Self::PersistentValidationTimedOut => f.write_str("Persistent validation timed out."),
            Self::Cancelled => f.write_str("Cancelled."),
        }
    }
}

use fmt::Display;

impl std::error::Error for PersistentCookieSessionError {}

static PERSISTENT_COOKIE_LOCKS: LazyLock<
    std::sync::Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
> = LazyLock::new(|| std::sync::Mutex::new(BTreeMap::new()));

static COOKIE_CACHE_SERIAL_LOCK: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));

static SERIALIZED_CALLBACK_LOCKS: LazyLock<
    std::sync::Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
> = LazyLock::new(|| std::sync::Mutex::new(BTreeMap::new()));

impl OpenAIDashboardBrowserCookieImporter {
    pub fn should_trust_verified_session(error: &PersistentCookieSessionError) -> bool {
        matches!(
            error,
            PersistentCookieSessionError::PersistentValidationTimedOut
        )
    }

    pub fn persistent_validation_failure(
        error: PersistentCookieSessionError,
    ) -> PersistentCookieSessionError {
        match error {
            PersistentCookieSessionError::TimedOut
            | PersistentCookieSessionError::Fetch(OpenAIDashboardFetchError::Timeout) => {
                PersistentCookieSessionError::PersistentValidationTimedOut
            }
            other => other,
        }
    }

    pub async fn with_persistent_cookie_transaction<T, F, Fut>(
        key: &str,
        deadline: Option<Instant>,
        cancel: Option<&tokio_util::sync::CancellationToken>,
        operation: F,
    ) -> Result<T, PersistentCookieSessionError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, PersistentCookieSessionError>>,
    {
        if cancel.is_some_and(|c| c.is_cancelled()) {
            return Err(PersistentCookieSessionError::Cancelled);
        }
        let lock = {
            let mut map = PERSISTENT_COOKIE_LOCKS.lock().expect("lock not poisoned");
            Arc::clone(
                map.entry(key.to_owned())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };

        let _guard = match (deadline, cancel) {
            (Some(dl), Some(ct)) => {
                let rem = Self::remaining_timeout(Some(dl), None, Instant::now())
                    .map_err(|_| PersistentCookieSessionError::TimedOut)?;
                tokio::select! {
                    () = ct.cancelled() => return Err(PersistentCookieSessionError::Cancelled),
                    res = tokio::time::timeout(rem, lock.lock_owned()) => {
                        res.map_err(|_| PersistentCookieSessionError::TimedOut)?
                    }
                }
            }
            (Some(dl), None) => {
                let rem = Self::remaining_timeout(Some(dl), None, Instant::now())
                    .map_err(|_| PersistentCookieSessionError::TimedOut)?;
                tokio::time::timeout(rem, lock.lock_owned())
                    .await
                    .map_err(|_| PersistentCookieSessionError::TimedOut)?
            }
            (None, Some(ct)) => {
                tokio::select! {
                    () = ct.cancelled() => return Err(PersistentCookieSessionError::Cancelled),
                    guard = lock.lock_owned() => guard,
                }
            }
            (None, None) => lock.lock_owned().await,
        };

        if cancel.is_some_and(|c| c.is_cancelled()) {
            return Err(PersistentCookieSessionError::Cancelled);
        }
        if let Some(dl) = deadline {
            Self::remaining_timeout(Some(dl), None, Instant::now())
                .map_err(|_| PersistentCookieSessionError::TimedOut)?;
        }

        if let Some(ct) = cancel {
            tokio::select! {
                () = ct.cancelled() => Err(PersistentCookieSessionError::Cancelled),
                res = operation() => res,
            }
        } else {
            operation().await
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn commit_persistent_cookie_session<ClearFn, WriteFn, ProbeFn, ProbeFut, EvictFn>(
        candidate_label: &str,
        candidate_cookie_count: usize,
        target_email: Option<&str>,
        store_key: &str,
        deadline: Option<Instant>,
        mut clear_existing: ClearFn,
        mut write_candidate: WriteFn,
        probe_signed_in_email: ProbeFn,
        mut evict_web_view: EvictFn,
    ) -> Result<OpenAIDashboardCookieImportResult, PersistentCookieSessionError>
    where
        ClearFn: FnMut(),
        WriteFn: FnMut(),
        ProbeFn: FnOnce() -> ProbeFut,
        ProbeFut:
            std::future::Future<Output = Result<Option<String>, PersistentCookieSessionError>>,
        EvictFn: FnMut(),
    {
        Self::with_persistent_cookie_transaction(store_key, deadline, None, || async move {
            clear_existing();
            write_candidate();
            match probe_signed_in_email().await {
                Ok(signed) => {
                    let signed = signed.map(|s| s.trim().to_owned());
                    let matches = target_email.is_some_and(|target| {
                        signed
                            .as_deref()
                            .is_some_and(|s| s.eq_ignore_ascii_case(target))
                    });
                    if target_email.is_some()
                        && let Some(actual) = signed.as_deref()
                        && !matches
                    {
                        evict_web_view();
                        clear_existing();
                        let found = if actual.is_empty() {
                            Vec::new()
                        } else {
                            vec![FoundAccount::new(candidate_label, actual)]
                        };
                        return Err(PersistentCookieSessionError::Import(
                            OpenAIDashboardCookieImportError::NoMatchingAccount { found },
                        ));
                    }
                    Ok(OpenAIDashboardCookieImportResult {
                        source_label: candidate_label.to_owned(),
                        cookie_count: candidate_cookie_count,
                        signed_in_email: signed,
                        matches_codex_email: matches,
                        cookie_header: SecretString::from(""),
                    })
                }
                Err(PersistentCookieSessionError::Fetch(
                    OpenAIDashboardFetchError::LoginRequired,
                )) => {
                    evict_web_view();
                    clear_existing();
                    Err(PersistentCookieSessionError::Import(
                        OpenAIDashboardCookieImportError::DashboardStillRequiresLogin,
                    ))
                }
                Err(err) => {
                    evict_web_view();
                    if matches!(
                        err,
                        PersistentCookieSessionError::Import(
                            OpenAIDashboardCookieImportError::NoMatchingAccount { .. }
                        )
                    ) {
                        clear_existing();
                    }
                    if target_email.is_none() {
                        Err(err)
                    } else {
                        Err(Self::persistent_validation_failure(err))
                    }
                }
            }
        })
        .await
    }

    pub async fn run_bounded_cookie_load<T, F>(
        deadline: Option<Instant>,
        timeout_observer: Option<Arc<dyn Fn() + Send + Sync>>,
        operation: F,
    ) -> Result<T, PersistentCookieSessionError>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, PersistentCookieSessionError> + Send + 'static,
    {
        let Some(dl) = deadline else {
            return tokio::task::spawn_blocking(operation)
                .await
                .map_err(|_| PersistentCookieSessionError::Cancelled)?;
        };
        let rem = Self::remaining_timeout(Some(dl), None, Instant::now())
            .map_err(|_| PersistentCookieSessionError::TimedOut)?;
        let handle = tokio::task::spawn_blocking(operation);
        match tokio::time::timeout(rem, handle).await {
            Ok(Ok(res)) => res,
            Ok(Err(_)) => Err(PersistentCookieSessionError::Cancelled),
            Err(_) => {
                if let Some(obs) = timeout_observer {
                    obs();
                }
                Err(PersistentCookieSessionError::TimedOut)
            }
        }
    }

    pub async fn run_bounded_cookie_cache_operation<T, F>(
        deadline: Option<Instant>,
        operation: F,
    ) -> Result<T, PersistentCookieSessionError>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, PersistentCookieSessionError> + Send + 'static,
    {
        let lock = Arc::clone(&COOKIE_CACHE_SERIAL_LOCK);
        let Some(dl) = deadline else {
            let _guard = lock.lock_owned().await;
            return tokio::task::spawn_blocking(operation)
                .await
                .map_err(|_| PersistentCookieSessionError::Cancelled)?;
        };
        let rem = Self::remaining_timeout(Some(dl), None, Instant::now())
            .map_err(|_| PersistentCookieSessionError::TimedOut)?;
        let task = tokio::spawn(async move {
            let _guard = lock.lock_owned().await;
            tokio::task::spawn_blocking(operation)
                .await
                .map_err(|_| PersistentCookieSessionError::Cancelled)?
        });
        match tokio::time::timeout(rem, task).await {
            Ok(Ok(res)) => res,
            Ok(Err(_)) => Err(PersistentCookieSessionError::Cancelled),
            Err(_) => Err(PersistentCookieSessionError::TimedOut),
        }
    }

    pub async fn run_bounded_callback<F>(
        deadline: Option<Instant>,
        timeout_observer: Option<Arc<dyn Fn() + Send + Sync>>,
        start: F,
    ) -> Result<(), PersistentCookieSessionError>
    where
        F: FnOnce(Box<dyn FnOnce() + Send>),
    {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        start(Box::new(move || {
            let _ = tx.send(());
        }));
        let Some(dl) = deadline else {
            let _ = rx.await;
            return Ok(());
        };
        let rem = Self::remaining_timeout(Some(dl), None, Instant::now())
            .map_err(|_| PersistentCookieSessionError::TimedOut)?;
        match tokio::time::timeout(rem, rx).await {
            Ok(_) => Ok(()),
            Err(_) => {
                if let Some(obs) = timeout_observer {
                    obs();
                }
                Err(PersistentCookieSessionError::TimedOut)
            }
        }
    }

    pub async fn run_bounded_value_callback<T, F>(
        deadline: Option<Instant>,
        timeout_observer: Option<Arc<dyn Fn() + Send + Sync>>,
        start: F,
    ) -> Result<T, PersistentCookieSessionError>
    where
        T: Send + 'static,
        F: FnOnce(Box<dyn FnOnce(T) + Send>),
    {
        let (tx, rx) = tokio::sync::oneshot::channel::<T>();
        start(Box::new(move |val| {
            let _ = tx.send(val);
        }));
        let Some(dl) = deadline else {
            return rx
                .await
                .map_err(|_| PersistentCookieSessionError::Cancelled);
        };
        let rem = Self::remaining_timeout(Some(dl), None, Instant::now())
            .map_err(|_| PersistentCookieSessionError::TimedOut)?;
        match tokio::time::timeout(rem, rx).await {
            Ok(Ok(val)) => Ok(val),
            Ok(Err(_)) => Err(PersistentCookieSessionError::Cancelled),
            Err(_) => {
                if let Some(obs) = timeout_observer {
                    obs();
                }
                Err(PersistentCookieSessionError::TimedOut)
            }
        }
    }

    pub async fn run_serialized_callback<F>(
        key: &str,
        deadline: Option<Instant>,
        start: F,
    ) -> Result<(), PersistentCookieSessionError>
    where
        F: FnOnce(Box<dyn FnOnce() + Send>) + Send + 'static,
    {
        let lock = {
            let mut map = SERIALIZED_CALLBACK_LOCKS.lock().expect("lock not poisoned");
            Arc::clone(
                map.entry(key.to_owned())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        let guard = if let Some(dl) = deadline {
            let rem = Self::remaining_timeout(Some(dl), None, Instant::now())
                .map_err(|_| PersistentCookieSessionError::TimedOut)?;
            tokio::time::timeout(rem, lock.lock_owned())
                .await
                .map_err(|_| PersistentCookieSessionError::TimedOut)?
        } else {
            lock.lock_owned().await
        };

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        start(Box::new(move || {
            let _ = tx.send(());
        }));
        let wait_task = tokio::spawn(async move {
            let _held = guard;
            let _ = rx.await;
        });
        if let Some(dl) = deadline {
            let rem = Self::remaining_timeout(Some(dl), None, Instant::now())
                .map_err(|_| PersistentCookieSessionError::TimedOut)?;
            tokio::time::timeout(rem, wait_task)
                .await
                .map_err(|_| PersistentCookieSessionError::TimedOut)?
                .map_err(|_| PersistentCookieSessionError::Cancelled)?;
        } else {
            let _ = wait_task.await;
        }
        Ok(())
    }
}
