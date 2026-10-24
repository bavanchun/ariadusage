// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeAdminAPISettingsReader.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeAdminAPIUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeAdminAPIUsageSnapshot.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/CostUsageModels.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::sync::LazyLock;
use std::time::Duration;

use ariadusage_core::hosts::ProviderHosts;
use ariadusage_core::model::{ProviderCostSnapshot, ProviderIdentitySnapshot, UsageSnapshot};
use ariadusage_core::pipeline::{
    AccountTokenProjection, ClassifiedError, EnvPrecedence, EnvProjection, FetchError,
    ProviderEnvDescriptor, TransportClass,
};
use ariadusage_core::settings_value::SettingsValue;
use ariadusage_protocol::ids::SettingId;
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::{
    Chart, ChartKind, ChartPoint, DetailRow, DetailSection, ProviderErrorKind,
};
use ariadusage_protocol::{Confidence, ProviderId};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, ToSpan};
use reqwest::header::{ACCEPT, HeaderName, HeaderValue, USER_AGENT};
use reqwest::{Method, Url};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::brokers::call::BrokerCall;
use crate::brokers::net::{
    DeclaredOrigins, NetBroker, NetError, NetRequest, OriginError, OverridePolicy,
    is_loopback_host, validate_override,
};
use crate::brokers::secret_store::{SecretId, SecretLookup, SecretStore};

use super::credential_routing::{ClaudeCredentialRoute, ClaudeCredentialRouting};
use super::credentials::{CLAUDE_OAUTH_SCOPES_ENV_KEY, CLAUDE_OAUTH_TOKEN_ENV_KEY};

pub const ADMIN_API_HOST: &str = "api.anthropic.com";
pub const DEFAULT_ADMIN_API_BASE_URL: &str = "https://api.anthropic.com";
pub const COST_REPORT_PATH: &str = "/v1/organizations/cost_report";
pub const MESSAGES_REPORT_PATH: &str = "/v1/organizations/usage_report/messages";
pub const COST_REPORT_ENDPOINT: &str = "cost_report";
pub const MESSAGES_ENDPOINT: &str = "messages";
pub const ANTHROPIC_VERSION_HEADER: &str = "anthropic-version";
pub const ANTHROPIC_VERSION_VALUE: &str = "2023-06-01";
pub const API_KEY_HEADER: &str = "x-api-key";
pub const ADMIN_API_USER_AGENT: &str = "AriadUsage/1.0";
pub const DEFAULT_ADMIN_API_TIMEOUT: Duration = Duration::from_secs(20);

pub const SESSION_KEY_ENV_KEY: &str = "CLAUDE_SESSION_KEY";
pub const ALTERNATE_SESSION_KEY_ENV_KEY: &str = "ANTHROPIC_SESSION_KEY";

static CLAUDE_ADMIN_API_DECLARED_ORIGINS: LazyLock<DeclaredOrigins> = LazyLock::new(|| {
    let hosts = ProviderHosts::new([ADMIN_API_HOST]).expect("valid Claude Admin API host");
    DeclaredOrigins::from_provider_hosts(&hosts)
});

/// Returns the static `DeclaredOrigins` pinned strictly to `https://api.anthropic.com`.
pub fn claude_admin_api_declared_origins() -> &'static DeclaredOrigins {
    &CLAUDE_ADMIN_API_DECLARED_ORIGINS
}

// ---------------------------------------------------------------------------
// Settings Reader (`ClaudeAdminAPISettingsReader.swift`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeAdminAPISettingsError {
    MissingToken,
}

impl fmt::Display for ClaudeAdminAPISettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingToken => f.write_str("Claude API usage needs an Anthropic Admin API key."),
        }
    }
}

impl std::error::Error for ClaudeAdminAPISettingsError {}

impl ClaudeAdminAPISettingsError {
    pub fn into_classified_error(self) -> ClassifiedError {
        ClassifiedError::new(ProviderErrorKind::MissingCredential, self.to_string())
    }

    pub fn into_fetch_error(self) -> FetchError {
        FetchError::Classified(self.into_classified_error())
    }
}

pub struct ClaudeAdminAPISettingsReader;

impl ClaudeAdminAPISettingsReader {
    pub const ADMIN_API_KEY_ENV_KEY: &'static str = "ANTHROPIC_ADMIN_KEY";
    pub const ALTERNATE_ADMIN_API_KEY_ENV_KEY: &'static str = "ANTHROPIC_ADMIN_API_KEY";
    pub const WORKSPACE_SPEND_ENV_KEY: &'static str = "ANTHROPIC_ADMIN_WORKSPACE_SPEND";

    pub const API_KEY_ENVIRONMENT_KEYS: [&'static str; 2] = [
        Self::ADMIN_API_KEY_ENV_KEY,
        Self::ALTERNATE_ADMIN_API_KEY_ENV_KEY,
    ];

    /// Resolves the Admin API key from a `SecretString` environment map, preferring
    /// `ANTHROPIC_ADMIN_KEY` over `ANTHROPIC_ADMIN_API_KEY`.
    pub fn api_key(environment: &BTreeMap<String, SecretString>) -> Option<SecretString> {
        for key in Self::API_KEY_ENVIRONMENT_KEYS {
            if let Some(raw) = environment.get(key)
                && let Some(cleaned) = Self::clean_token(Some(raw.expose_secret()))
            {
                return Some(cleaned);
            }
        }
        None
    }

    /// Resolves the Admin API key from a plain `String` environment map.
    pub fn api_key_from_strings(environment: &BTreeMap<String, String>) -> Option<SecretString> {
        for key in Self::API_KEY_ENVIRONMENT_KEYS {
            if let Some(raw) = environment.get(key)
                && let Some(cleaned) = Self::clean_token(Some(raw.as_str()))
            {
                return Some(cleaned);
            }
        }
        None
    }

    /// Trims surrounding whitespace and one matching quote layer (`SettingsValue.cleaned`).
    pub fn clean_token(raw: Option<&str>) -> Option<SecretString> {
        SettingsValue::cleaned(raw).map(SecretString::new)
    }

    /// Reads `ANTHROPIC_ADMIN_WORKSPACE_SPEND` from a `SecretString` environment map,
    /// matching `ClaudeProviderDescriptor.swift:577` (`env[key] == "true"`).
    pub fn workspace_spend_enabled(environment: &BTreeMap<String, SecretString>) -> bool {
        environment
            .get(Self::WORKSPACE_SPEND_ENV_KEY)
            .is_some_and(|v| v.expose_secret() == "true")
    }

    /// Reads `ANTHROPIC_ADMIN_WORKSPACE_SPEND` from a plain `String` environment map,
    /// matching `ClaudeProviderDescriptor.swift:577` (`env[key] == "true"`).
    pub fn workspace_spend_enabled_from_strings(environment: &BTreeMap<String, String>) -> bool {
        environment
            .get(Self::WORKSPACE_SPEND_ENV_KEY)
            .is_some_and(|v| v == "true")
    }

    /// Looks up `providers.claude.apiKey` in [`SecretStore`] and cleans the token.
    pub async fn api_key_from_secret_store(
        secret_store: &SecretStore,
        call: BrokerCall,
    ) -> Option<SecretString> {
        let setting = SettingId::new("providers.claude.apiKey").ok()?;
        let id = SecretId::from_setting_id(&setting).ok()?;
        match secret_store.user(call).lookup(&id).await {
            SecretLookup::Found(secret) => Self::clean_token(Some(secret.expose_secret())),
            _ => None,
        }
    }

    /// Looks up `providers.claude.accounts.<account_id>.token` in [`SecretStore`].
    pub async fn token_account_from_secret_store(
        secret_store: &SecretStore,
        account_id: &str,
        call: BrokerCall,
    ) -> Option<SecretString> {
        let setting =
            SettingId::new(format!("providers.claude.accounts.{account_id}.token")).ok()?;
        let id = SecretId::from_setting_id(&setting).ok()?;
        match secret_store.user(call).lookup(&id).await {
            SecretLookup::Found(secret) => Some(secret),
            _ => None,
        }
    }
}

/// Environment descriptor matching `ClaudeProviderDescriptor.swift:39-44, 69-74`.
pub fn claude_env_descriptor<C>(
    sanitized_api_key: fn(&C) -> Option<String>,
    workspace_spend_enabled: fn(&C) -> Option<String>,
) -> ProviderEnvDescriptor<C> {
    ProviderEnvDescriptor {
        projections: vec![
            EnvProjection {
                key: ClaudeAdminAPISettingsReader::ADMIN_API_KEY_ENV_KEY.to_owned(),
                precedence: EnvPrecedence::Config,
                value_from_config: sanitized_api_key,
            },
            EnvProjection {
                key: ClaudeAdminAPISettingsReader::WORKSPACE_SPEND_ENV_KEY.to_owned(),
                precedence: EnvPrecedence::Config,
                value_from_config: workspace_spend_enabled,
            },
        ],
        account_projection: Some(AccountTokenProjection {
            scrub_keys: vec![
                CLAUDE_OAUTH_TOKEN_ENV_KEY.to_owned(),
                CLAUDE_OAUTH_SCOPES_ENV_KEY.to_owned(),
                SESSION_KEY_ENV_KEY.to_owned(),
                ALTERNATE_SESSION_KEY_ENV_KEY.to_owned(),
                ClaudeAdminAPISettingsReader::ADMIN_API_KEY_ENV_KEY.to_owned(),
                ClaudeAdminAPISettingsReader::ALTERNATE_ADMIN_API_KEY_ENV_KEY.to_owned(),
            ],
            target_key: None,
        }),
    }
}

/// Returns [`claude_env_descriptor`] bound to [`ariadusage_core::config::ProviderConfig`],
/// projecting `claude_workspace_spend_enabled` into `ANTHROPIC_ADMIN_WORKSPACE_SPEND`.
pub fn claude_provider_config_env_descriptor(
    sanitized_api_key: fn(&ariadusage_core::config::ProviderConfig) -> Option<String>,
) -> ProviderEnvDescriptor<ariadusage_core::config::ProviderConfig> {
    claude_env_descriptor(sanitized_api_key, |cfg| {
        cfg.claude_workspace_spend_enabled_opt()
            .map(|v| v.to_string())
    })
}

/// Token account environment override matching `ClaudeProviderDescriptor.swift:59-68`.
pub fn claude_token_account_env_override(token: &str) -> Option<BTreeMap<String, SecretString>> {
    match ClaudeCredentialRouting::resolve(Some(token), None) {
        ClaudeCredentialRoute::Oauth { access_token } => {
            let mut env = BTreeMap::new();
            env.insert(CLAUDE_OAUTH_TOKEN_ENV_KEY.to_owned(), access_token);
            Some(env)
        }
        ClaudeCredentialRoute::AdminApiKey(api_key) => {
            let mut env = BTreeMap::new();
            env.insert(
                ClaudeAdminAPISettingsReader::ADMIN_API_KEY_ENV_KEY.to_owned(),
                api_key,
            );
            Some(env)
        }
        ClaudeCredentialRoute::None | ClaudeCredentialRoute::WebCookie { .. } => None,
    }
}

// ---------------------------------------------------------------------------
// Bucket Interval Helper (`CostUsageModels.swift:318-323`)
// ---------------------------------------------------------------------------

pub struct CostUsageBucketInterval;

impl CostUsageBucketInterval {
    /// Returns `true` when `now` falls inside `[start_time, end_time)` (defaulting `end_time` to `start_time + 24h`).
    pub fn contains(now: Timestamp, start_time: Timestamp, end_time: Option<Timestamp>) -> bool {
        let resolved_end = end_time.unwrap_or_else(|| {
            start_time
                .checked_add(SignedDuration::from_secs(86_400))
                .unwrap_or(Timestamp::MAX)
        });
        now >= start_time && now < resolved_end
    }
}

// ---------------------------------------------------------------------------
// Snapshot Model (`ClaudeAdminAPIUsageSnapshot.swift`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyBucket {
    pub day: String,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    pub start_time: Timestamp,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    pub end_time: Timestamp,
    #[serde(rename = "costUSD")]
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub output_tokens: i64,
    pub cost_items: Vec<CostBreakdown>,
    pub models: Vec<ModelBreakdown>,
}

impl DailyBucket {
    pub fn new(day: impl Into<String>, start_time: Timestamp) -> Self {
        let end_time = start_time
            .checked_add(SignedDuration::from_secs(86_400))
            .unwrap_or(start_time);
        Self {
            day: day.into(),
            start_time,
            end_time,
            cost_usd: 0.0,
            input_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            output_tokens: 0,
            cost_items: Vec::new(),
            models: Vec::new(),
        }
    }

    pub fn with_end_time(mut self, end_time: Timestamp) -> Self {
        self.end_time = end_time;
        self
    }

    pub fn total_tokens(&self) -> i64 {
        self.input_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
            + self.output_tokens
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostBreakdown {
    pub name: String,
    #[serde(rename = "costUSD")]
    pub cost_usd: f64,
}

impl CostBreakdown {
    pub fn new(name: impl Into<String>, cost_usd: f64) -> Self {
        Self {
            name: name.into(),
            cost_usd,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelBreakdown {
    pub name: String,
    pub input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub output_tokens: i64,
}

impl ModelBreakdown {
    pub fn total_tokens(&self) -> i64 {
        self.input_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
            + self.output_tokens
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub output_tokens: i64,
}

impl Summary {
    pub const ZERO: Self = Self {
        cost_usd: 0.0,
        input_tokens: 0,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        output_tokens: 0,
    };

    pub fn total_tokens(&self) -> i64 {
        self.input_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
            + self.output_tokens
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAdminAPIUsageSnapshot {
    pub daily: Vec<DailyBucket>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_costs: Option<Vec<CostBreakdown>>,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    pub updated_at: Timestamp,
}

impl ClaudeAdminAPIUsageSnapshot {
    pub fn new(
        mut daily: Vec<DailyBucket>,
        workspace_costs: Option<Vec<CostBreakdown>>,
        updated_at: Timestamp,
    ) -> Self {
        daily.sort_by_key(|lhs| lhs.start_time);
        Self {
            daily,
            workspace_costs,
            updated_at,
        }
    }

    fn suffix(&self, max_len: usize) -> &[DailyBucket] {
        let len = max_len.max(1);
        let start = self.daily.len().saturating_sub(len);
        &self.daily[start..]
    }

    pub fn summary(&self, days: usize) -> Summary {
        Self::summarize(self.suffix(days))
    }

    pub fn last_30_days(&self) -> Summary {
        self.summary(30)
    }

    pub fn last_7_days(&self) -> Summary {
        self.summary(7)
    }

    pub fn current_day(&self) -> Summary {
        self.summary_for_local_day_containing(self.updated_at)
    }

    pub fn current_day_in(&self, _tz: &TimeZone) -> Summary {
        self.summary_for_local_day_containing(self.updated_at)
    }

    pub fn summary_for_local_day_containing(&self, date: Timestamp) -> Summary {
        self.daily
            .iter()
            .filter(|bucket| {
                CostUsageBucketInterval::contains(date, bucket.start_time, Some(bucket.end_time))
            })
            .fold(Summary::ZERO, |acc, bucket| Summary {
                cost_usd: acc.cost_usd + bucket.cost_usd,
                input_tokens: acc.input_tokens + bucket.input_tokens,
                cache_creation_input_tokens: acc.cache_creation_input_tokens
                    + bucket.cache_creation_input_tokens,
                cache_read_input_tokens: acc.cache_read_input_tokens
                    + bucket.cache_read_input_tokens,
                output_tokens: acc.output_tokens + bucket.output_tokens,
            })
    }

    pub fn latest_day(&self) -> Summary {
        self.summary(1)
    }

    pub fn top_models(&self) -> Vec<ModelBreakdown> {
        let mut totals: HashMap<String, ModelBreakdown> = HashMap::new();
        for bucket in &self.daily {
            for model in &bucket.models {
                let entry = totals
                    .entry(model.name.clone())
                    .or_insert_with(|| ModelBreakdown {
                        name: model.name.clone(),
                        input_tokens: 0,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                        output_tokens: 0,
                    });
                entry.input_tokens += model.input_tokens;
                entry.cache_creation_input_tokens += model.cache_creation_input_tokens;
                entry.cache_read_input_tokens += model.cache_read_input_tokens;
                entry.output_tokens += model.output_tokens;
            }
        }
        let mut models: Vec<ModelBreakdown> = totals.into_values().collect();
        models.sort_by(|lhs, rhs| {
            let l_total = lhs.total_tokens();
            let r_total = rhs.total_tokens();
            if l_total != r_total {
                r_total.cmp(&l_total)
            } else {
                lhs.name.cmp(&rhs.name)
            }
        });
        models
    }

    pub fn top_cost_items(&self) -> Vec<CostBreakdown> {
        let mut totals: HashMap<String, f64> = HashMap::new();
        for bucket in &self.daily {
            for item in &bucket.cost_items {
                *totals.entry(item.name.clone()).or_insert(0.0) += item.cost_usd;
            }
        }
        let mut items: Vec<CostBreakdown> = totals
            .into_iter()
            .map(|(name, cost_usd)| CostBreakdown { name, cost_usd })
            .collect();
        items.sort_by(|lhs, rhs| {
            if lhs.cost_usd != rhs.cost_usd {
                rhs.cost_usd.total_cmp(&lhs.cost_usd)
            } else {
                lhs.name.cmp(&rhs.name)
            }
        });
        items
    }

    pub fn to_usage_snapshot(&self) -> UsageSnapshot {
        self.to_usage_snapshot_in(&TimeZone::system())
    }

    pub fn to_usage_snapshot_in(&self, _tz: &TimeZone) -> UsageSnapshot {
        let today = self.current_day();
        let last_7 = self.last_7_days();
        let total = self.last_30_days();

        let mut summary_rows = vec![
            DetailRow {
                id: Some("Today spend".to_owned()),
                label: "Today spend".to_owned(),
                value: Self::format_usd(today.cost_usd),
                secondary_value: None,
                progress: None,
                severity: None,
                usage_value: None,
            },
            DetailRow {
                id: Some("7d spend".to_owned()),
                label: "7d spend".to_owned(),
                value: Self::format_usd(last_7.cost_usd),
                secondary_value: None,
                progress: None,
                severity: None,
                usage_value: None,
            },
            DetailRow {
                id: Some("30d spend".to_owned()),
                label: "30d spend".to_owned(),
                value: Self::format_usd(total.cost_usd),
                secondary_value: None,
                progress: None,
                severity: None,
                usage_value: None,
            },
            DetailRow {
                id: Some("Today tokens".to_owned()),
                label: "Today tokens".to_owned(),
                value: Self::format_count(today.total_tokens()),
                secondary_value: None,
                progress: None,
                severity: None,
                usage_value: None,
            },
            DetailRow {
                id: Some("30d tokens".to_owned()),
                label: "30d tokens".to_owned(),
                value: Self::format_count(total.total_tokens()),
                secondary_value: None,
                progress: None,
                severity: None,
                usage_value: None,
            },
            DetailRow {
                id: Some("Cache read".to_owned()),
                label: "Cache read".to_owned(),
                value: Self::format_count(total.cache_read_input_tokens),
                secondary_value: None,
                progress: None,
                severity: None,
                usage_value: None,
            },
        ];

        if let Some(top) = self.top_models().into_iter().next() {
            summary_rows.push(DetailRow {
                id: Some("Top model".to_owned()),
                label: "Top model".to_owned(),
                value: top.name,
                secondary_value: None,
                progress: None,
                severity: None,
                usage_value: None,
            });
        }

        let chart = if self.daily.is_empty() {
            None
        } else {
            Some(Chart {
                kind: ChartKind::Bars,
                title: Some("Daily spend".to_owned()),
                unit: Some("USD".to_owned()),
                points: self
                    .daily
                    .iter()
                    .map(|b| ChartPoint {
                        label: b.day.clone(),
                        value: b.cost_usd,
                    })
                    .collect(),
            })
        };

        let mut sections = vec![DetailSection {
            title: Some("Usage summary".to_owned()),
            rows: summary_rows,
            chart,
        }];

        let cost_rows: Vec<DetailRow> = self
            .top_cost_items()
            .into_iter()
            .take(20)
            .map(|item| DetailRow {
                id: Some(item.name.clone()),
                label: item.name,
                value: Self::format_usd(item.cost_usd),
                secondary_value: None,
                progress: None,
                severity: None,
                usage_value: None,
            })
            .collect();
        if !cost_rows.is_empty() {
            sections.push(DetailSection {
                title: Some("Cost items".to_owned()),
                rows: cost_rows,
                chart: None,
            });
        }

        if let Some(ref workspaces) = self.workspace_costs
            && workspaces.len() > 1
        {
            let workspace_rows: Vec<DetailRow> = workspaces
                .iter()
                .take(20)
                .map(|item| DetailRow {
                    id: Some(item.name.clone()),
                    label: item.name.clone(),
                    value: Self::format_usd(item.cost_usd),
                    secondary_value: None,
                    progress: None,
                    severity: None,
                    usage_value: None,
                })
                .collect();
            sections.push(DetailSection {
                title: Some("Workspace spend · 30d".to_owned()),
                rows: workspace_rows,
                chart: None,
            });
        }

        let identity = ProviderIdentitySnapshot {
            provider_id: ProviderId::new("claude").ok(),
            account_email: None,
            account_organization: None,
            account_id: None,
            login_method: Some("Admin API".to_owned()),
        };

        let mut cost = ProviderCostSnapshot::new(total.cost_usd, 0.0, "USD", self.updated_at);
        cost.period = Some("Last 30 days".to_owned());

        UsageSnapshot::new(
            None,
            None,
            None,
            None,
            Some(cost),
            sections,
            None,
            None,
            self.updated_at,
            Some(identity),
            Confidence::Exact,
        )
        .expect("valid Claude Admin API UsageSnapshot")
    }

    fn summarize(buckets: &[DailyBucket]) -> Summary {
        buckets.iter().fold(Summary::ZERO, |acc, bucket| Summary {
            cost_usd: acc.cost_usd + bucket.cost_usd,
            input_tokens: acc.input_tokens + bucket.input_tokens,
            cache_creation_input_tokens: acc.cache_creation_input_tokens
                + bucket.cache_creation_input_tokens,
            cache_read_input_tokens: acc.cache_read_input_tokens + bucket.cache_read_input_tokens,
            output_tokens: acc.output_tokens + bucket.output_tokens,
        })
    }

    pub fn format_usd(value: f64) -> String {
        if !value.is_finite() {
            return "$0.00".to_owned();
        }
        let negative = value < 0.0;
        let cents = (value.abs() * 100.0).round() as i64;
        let whole = cents / 100;
        let frac = cents % 100;
        let grouped = Self::format_count(whole);
        if negative && cents > 0 {
            format!("-${grouped}.{frac:02}")
        } else {
            format!("${grouped}.{frac:02}")
        }
    }

    pub fn format_count(value: i64) -> String {
        let negative = value < 0;
        let digits = value.unsigned_abs().to_string();
        let len = digits.len();
        let mut out = String::with_capacity(len + len / 3 + 1);
        if negative {
            out.push('-');
        }
        for (idx, ch) in digits.chars().enumerate() {
            if idx > 0 && (len - idx).is_multiple_of(3) {
                out.push(',');
            }
            out.push(ch);
        }
        out
    }
}

/// Optional detail visibility rules for Claude (`ClaudeProviderDescriptor.swift:83-89`,
/// `MenuCardView+ProviderDetailVisibility.swift:18-32`, `MenuCardView.swift:912-941`).
pub struct ClaudeOptionalDetailPolicy;

impl ClaudeOptionalDetailPolicy {
    pub const COST_SUMMARY_TITLES: [&'static str; 3] =
        ["Usage summary", "Cost items", "Workspace spend · 30d"];
    pub const HIDDEN_TITLES_WITHOUT_OPTIONAL_USAGE: [&'static str; 1] = ["Prepaid credits"];

    /// Returns whether the standalone `providerCost` card section is shown for a Claude snapshot.
    ///
    /// Admin API snapshots emit a 30-day `providerCost` with `limit == 0` and no prepaid `balance`,
    /// which is suppressed in favor of the `"Usage summary"` detail section (`MenuCardView+Costs.swift:562-575`).
    pub fn shows_provider_cost_section(
        snapshot: &UsageSnapshot,
        cost_summary_inline_enabled: bool,
        show_optional_credits_and_extra_usage: bool,
    ) -> bool {
        if !show_optional_credits_and_extra_usage {
            return false;
        }
        let Some(cost) = snapshot.provider_cost.as_ref() else {
            return false;
        };
        let is_claude_admin_api = snapshot
            .identity
            .as_ref()
            .and_then(|id| id.login_method.as_deref())
            == Some("Admin API");
        let follows_summary_style = is_claude_admin_api && cost.limit <= 0.0;
        if follows_summary_style && !cost_summary_inline_enabled {
            return false;
        }
        if cost.limit <= 0.0 {
            return cost.balance.is_some();
        }
        true
    }

    /// Filters `snapshot.details` according to `ClaudeProviderDescriptor`'s `optionalDetails` policy.
    pub fn visible_details(
        details: &[DetailSection],
        cost_summary_inline_enabled: bool,
        show_optional_credits_and_extra_usage: bool,
    ) -> Vec<DetailSection> {
        details
            .iter()
            .filter(|section| {
                if !cost_summary_inline_enabled
                    && section
                        .title
                        .as_deref()
                        .is_some_and(|t| Self::COST_SUMMARY_TITLES.contains(&t))
                {
                    return false;
                }
                if !show_optional_credits_and_extra_usage
                    && section
                        .title
                        .as_deref()
                        .is_some_and(|t| Self::HIDDEN_TITLES_WITHOUT_OPTIONAL_USAGE.contains(&t))
                {
                    return false;
                }
                true
            })
            .cloned()
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Usage Fetcher (`ClaudeAdminAPIUsageFetcher.swift`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeAdminAPIEndpointConfig {
    base_url: Url,
    declared_origins: DeclaredOrigins,
    timeout: Duration,
}

impl Default for ClaudeAdminAPIEndpointConfig {
    fn default() -> Self {
        let base_url = Url::parse(DEFAULT_ADMIN_API_BASE_URL)
            .expect("valid default Claude Admin API base URL");
        let declared_origins = claude_admin_api_declared_origins().clone();
        Self {
            base_url,
            declared_origins,
            timeout: DEFAULT_ADMIN_API_TIMEOUT,
        }
    }
}

impl ClaudeAdminAPIEndpointConfig {
    /// Creates an endpoint config with a validated base URL.
    ///
    /// Only `https://api.anthropic.com` (the declared Anthropic origin) or an HTTPS loopback
    /// test origin (`127.0.0.1`, `localhost`, `[::1]`) is permitted. Any other host or scheme is
    /// rejected with [`OriginError::Invalid`] before any request or secret can be sent.
    pub fn with_base_url(base_url: Url) -> Result<Self, OriginError> {
        validate_admin_api_url(&base_url)?;
        let declared_origins = if base_url.host_str().is_some_and(is_loopback_host) {
            DeclaredOrigins::from_urls([base_url.clone()])?
        } else {
            claude_admin_api_declared_origins().clone()
        };
        Ok(Self {
            base_url,
            declared_origins,
            timeout: DEFAULT_ADMIN_API_TIMEOUT,
        })
    }

    pub fn with_base_url_str(raw: &str) -> Result<Self, OriginError> {
        let parsed = Url::parse(raw).map_err(|_| OriginError::Invalid)?;
        Self::with_base_url(parsed)
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub fn declared_origins(&self) -> &DeclaredOrigins {
        &self.declared_origins
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// Validates that `url` targets either `https://api.anthropic.com` or an HTTPS loopback test server.
pub fn validate_admin_api_url(url: &Url) -> Result<(), OriginError> {
    if !url.scheme().eq_ignore_ascii_case("https") {
        return Err(OriginError::Invalid);
    }
    let normalized = validate_override(url.as_str(), &OverridePolicy::HttpsOnly)
        .map_err(|_| OriginError::Invalid)?;
    let host = normalized.host_str().ok_or(OriginError::Invalid)?;
    if is_loopback_host(host) {
        DeclaredOrigins::from_urls([normalized])?;
        return Ok(());
    }
    let policy = OverridePolicy::ProviderOwnedOnly {
        allowed_hosts: vec![ADMIN_API_HOST.to_owned()],
        allowed_domain_suffixes: Vec::new(),
    };
    validate_override(normalized.as_str(), &policy).map_err(|_| OriginError::Invalid)?;
    if !claude_admin_api_declared_origins().contains_url(&normalized) {
        return Err(OriginError::Invalid);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeAdminAPIUsageError {
    MissingCredentials,
    NetworkError(String),
    ApiError {
        endpoint: &'static str,
        status_code: u16,
    },
    ParseFailed {
        endpoint: &'static str,
    },
    OriginRejected(OriginError),
    Cancelled,
}

impl fmt::Display for ClaudeAdminAPIUsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCredentials => f.write_str("Missing Anthropic Admin API key."),
            Self::NetworkError(msg) => write!(f, "Claude API usage network error: {msg}"),
            Self::ApiError {
                endpoint,
                status_code,
            } => write!(f, "Claude API usage {endpoint} error: HTTP {status_code}"),
            Self::ParseFailed { endpoint } => {
                write!(f, "Failed to parse Claude API usage {endpoint}")
            }
            Self::OriginRejected(err) => {
                write!(f, "Claude API usage network error: {err}")
            }
            Self::Cancelled => f.write_str("Claude API usage network error: cancelled"),
        }
    }
}

impl std::error::Error for ClaudeAdminAPIUsageError {}

impl ClaudeAdminAPIUsageError {
    pub fn into_classified_error(self) -> ClassifiedError {
        let message = self.to_string();
        match self {
            Self::MissingCredentials => {
                ClassifiedError::new(ProviderErrorKind::MissingCredential, message)
            }
            Self::NetworkError(_) => {
                ClassifiedError::new(ProviderErrorKind::NetworkFailure, message)
            }
            Self::ApiError {
                status_code: 401 | 403,
                ..
            } => ClassifiedError::new(ProviderErrorKind::AuthenticationExpired, message),
            Self::ApiError {
                status_code: 429, ..
            } => ClassifiedError::new(ProviderErrorKind::RateLimited, message),
            Self::ApiError {
                status_code: 500..=599,
                ..
            } => ClassifiedError::new(ProviderErrorKind::ProviderUnavailable, message),
            Self::ApiError { .. } | Self::OriginRejected(_) => {
                ClassifiedError::new(ProviderErrorKind::ApiFailure, message)
            }
            Self::ParseFailed { .. } => {
                ClassifiedError::new(ProviderErrorKind::ParseFailure, message)
            }
            Self::Cancelled => ClassifiedError::new(ProviderErrorKind::Unknown, message)
                .with_transport(TransportClass::Cancelled),
        }
    }

    pub fn into_fetch_error(self) -> FetchError {
        match self {
            Self::Cancelled => FetchError::Cancelled,
            other => FetchError::Classified(other.into_classified_error()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct CostReportResponse {
    data: Option<Vec<CostReportBucket>>,
}

#[derive(Debug, Deserialize)]
struct CostReportBucket {
    starting_at: Option<String>,
    ending_at: Option<String>,
    results: Option<Vec<CostReportResult>>,
}

#[derive(Debug, Deserialize)]
struct CostReportResult {
    #[serde(default, deserialize_with = "deserialize_string_or_number")]
    amount: Option<String>,
    description: Option<String>,
    cost_type: Option<String>,
    #[allow(dead_code)]
    model: Option<String>,
    workspace_id: Option<String>,
}

fn deserialize_string_or_number<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let val = Option::<Value>::deserialize(deserializer)?;
    Ok(match val {
        Some(Value::String(s)) => Some(s),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    })
}

#[derive(Debug, Deserialize)]
struct MessagesUsageReportResponse {
    data: Option<Vec<MessagesUsageBucket>>,
}

#[derive(Debug, Deserialize)]
struct MessagesUsageBucket {
    starting_at: Option<String>,
    ending_at: Option<String>,
    results: Option<Vec<MessagesUsageResult>>,
}

#[derive(Debug, Deserialize)]
struct MessagesUsageResult {
    model: Option<String>,
    uncached_input_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_creation: Option<CacheCreation>,
}

#[derive(Debug, Deserialize)]
struct CacheCreation {
    ephemeral_1h_input_tokens: Option<i64>,
    ephemeral_5m_input_tokens: Option<i64>,
}

pub struct ClaudeAdminAPIUsageFetcher;

impl ClaudeAdminAPIUsageFetcher {
    pub async fn fetch_usage(
        net: &NetBroker,
        endpoint: &ClaudeAdminAPIEndpointConfig,
        api_key: &str,
        workspace_spend_enabled: bool,
        now: Timestamp,
        call: &BrokerCall,
    ) -> Result<ClaudeAdminAPIUsageSnapshot, ClaudeAdminAPIUsageError> {
        let trimmed = api_key.trim();
        if trimmed.is_empty() {
            return Err(ClaudeAdminAPIUsageError::MissingCredentials);
        }
        let cleaned_key = SecretString::new(trimmed);

        validate_admin_api_url(endpoint.base_url())
            .map_err(ClaudeAdminAPIUsageError::OriginRejected)?;

        let (start_date, end_date) = Self::window_bounds_utc(now)?;
        let cost_query = Self::cost_query_items(start_date, end_date, workspace_spend_enabled);
        let usage_query = Self::messages_query_items(start_date, end_date);

        let cost_url = Self::build_url(endpoint.base_url(), COST_REPORT_PATH, &cost_query)?;
        let usage_url = Self::build_url(endpoint.base_url(), MESSAGES_REPORT_PATH, &usage_query)?;

        let cost_bytes = Self::fetch_bytes(
            net,
            endpoint,
            cost_url,
            &cleaned_key,
            COST_REPORT_ENDPOINT,
            call,
        )
        .await?;
        let cost_response = Self::decode_costs(&cost_bytes)?;

        let usage_bytes = Self::fetch_bytes(
            net,
            endpoint,
            usage_url,
            &cleaned_key,
            MESSAGES_ENDPOINT,
            call,
        )
        .await?;
        let usage_response = Self::decode_messages(&usage_bytes)?;

        Ok(Self::build_snapshot(
            cost_response,
            usage_response,
            workspace_spend_enabled,
            now,
        ))
    }

    pub fn parse_snapshot(
        cost_data: &[u8],
        usage_data: &[u8],
        workspace_spend_enabled: bool,
        now: Timestamp,
    ) -> Result<ClaudeAdminAPIUsageSnapshot, ClaudeAdminAPIUsageError> {
        let cost_response = Self::decode_costs(cost_data)?;
        let usage_response = Self::decode_messages(usage_data)?;

        Ok(Self::build_snapshot(
            cost_response,
            usage_response,
            workspace_spend_enabled,
            now,
        ))
    }

    fn decode_costs(data: &[u8]) -> Result<CostReportResponse, ClaudeAdminAPIUsageError> {
        serde_json::from_slice(data).map_err(|_| ClaudeAdminAPIUsageError::ParseFailed {
            endpoint: COST_REPORT_ENDPOINT,
        })
    }

    fn decode_messages(
        data: &[u8],
    ) -> Result<MessagesUsageReportResponse, ClaudeAdminAPIUsageError> {
        serde_json::from_slice(data).map_err(|_| ClaudeAdminAPIUsageError::ParseFailed {
            endpoint: MESSAGES_ENDPOINT,
        })
    }

    fn build_snapshot(
        cost_response: CostReportResponse,
        usage_response: MessagesUsageReportResponse,
        workspace_spend_enabled: bool,
        now: Timestamp,
    ) -> ClaudeAdminAPIUsageSnapshot {
        struct DailyAccumulator {
            starting_at: String,
            ending_at: String,
            cost_usd: f64,
            input_tokens: i64,
            cache_creation_input_tokens: i64,
            cache_read_input_tokens: i64,
            output_tokens: i64,
            cost_items: HashMap<String, f64>,
            models: HashMap<String, ModelBreakdown>,
        }

        let mut accumulators: HashMap<String, DailyAccumulator> = HashMap::new();
        let cost_buckets = cost_response.data.unwrap_or_default();

        for bucket in &cost_buckets {
            let Some(starting_at) = bucket.starting_at.as_deref() else {
                continue;
            };
            let Some(ending_at) = bucket.ending_at.as_deref() else {
                continue;
            };
            let current = accumulators
                .entry(starting_at.to_owned())
                .or_insert_with(|| DailyAccumulator {
                    starting_at: starting_at.to_owned(),
                    ending_at: ending_at.to_owned(),
                    cost_usd: 0.0,
                    input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                    output_tokens: 0,
                    cost_items: HashMap::new(),
                    models: HashMap::new(),
                });
            for result in bucket.results.as_deref().unwrap_or(&[]) {
                let amount_cents = Self::parse_double(result.amount.as_deref()).unwrap_or(0.0);
                let amount_usd = amount_cents / 100.0;
                current.cost_usd += amount_usd;
                let item = Self::display_name(
                    result
                        .description
                        .as_deref()
                        .or(result.cost_type.as_deref()),
                    "Claude API",
                );
                *current.cost_items.entry(item).or_insert(0.0) += amount_usd;
            }
        }

        for bucket in usage_response.data.unwrap_or_default() {
            let Some(starting_at) = bucket.starting_at.as_deref() else {
                continue;
            };
            let Some(ending_at) = bucket.ending_at.as_deref() else {
                continue;
            };
            let current = accumulators
                .entry(starting_at.to_owned())
                .or_insert_with(|| DailyAccumulator {
                    starting_at: starting_at.to_owned(),
                    ending_at: ending_at.to_owned(),
                    cost_usd: 0.0,
                    input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                    output_tokens: 0,
                    cost_items: HashMap::new(),
                    models: HashMap::new(),
                });
            for result in bucket.results.unwrap_or_default() {
                let input = result.uncached_input_tokens.unwrap_or(0);
                let cache_creation = result
                    .cache_creation
                    .as_ref()
                    .and_then(|c| c.ephemeral_1h_input_tokens)
                    .unwrap_or(0)
                    + result
                        .cache_creation
                        .as_ref()
                        .and_then(|c| c.ephemeral_5m_input_tokens)
                        .unwrap_or(0);
                let cache_read = result.cache_read_input_tokens.unwrap_or(0);
                let output = result.output_tokens.unwrap_or(0);
                current.input_tokens += input;
                current.cache_creation_input_tokens += cache_creation;
                current.cache_read_input_tokens += cache_read;
                current.output_tokens += output;

                let model_name = Self::display_name(result.model.as_deref(), "Claude API");
                let entry =
                    current
                        .models
                        .entry(model_name.clone())
                        .or_insert_with(|| ModelBreakdown {
                            name: model_name,
                            input_tokens: 0,
                            cache_creation_input_tokens: 0,
                            cache_read_input_tokens: 0,
                            output_tokens: 0,
                        });
                entry.input_tokens += input;
                entry.cache_creation_input_tokens += cache_creation;
                entry.cache_read_input_tokens += cache_read;
                entry.output_tokens += output;
            }
        }

        let mut daily: Vec<DailyBucket> = accumulators
            .into_values()
            .filter_map(|acc| {
                let start_time = Self::parse_date(&acc.starting_at)?;
                let end_time = Self::parse_date(&acc.ending_at)?;
                if start_time > now {
                    return None;
                }
                let mut cost_items: Vec<CostBreakdown> = acc
                    .cost_items
                    .into_iter()
                    .map(|(name, cost_usd)| CostBreakdown { name, cost_usd })
                    .collect();
                cost_items.sort_by(|lhs, rhs| {
                    if lhs.cost_usd != rhs.cost_usd {
                        rhs.cost_usd.total_cmp(&lhs.cost_usd)
                    } else {
                        lhs.name.cmp(&rhs.name)
                    }
                });

                let mut models: Vec<ModelBreakdown> = acc.models.into_values().collect();
                models.sort_by(|lhs, rhs| {
                    let l_total = lhs.total_tokens();
                    let r_total = rhs.total_tokens();
                    if l_total != r_total {
                        r_total.cmp(&l_total)
                    } else {
                        lhs.name.cmp(&rhs.name)
                    }
                });

                Some(DailyBucket {
                    day: Self::day_string(start_time),
                    start_time,
                    end_time,
                    cost_usd: acc.cost_usd,
                    input_tokens: acc.input_tokens,
                    cache_creation_input_tokens: acc.cache_creation_input_tokens,
                    cache_read_input_tokens: acc.cache_read_input_tokens,
                    output_tokens: acc.output_tokens,
                    cost_items,
                    models,
                })
            })
            .collect();
        daily.sort_by_key(|lhs| lhs.start_time);

        let workspace_costs = if workspace_spend_enabled {
            let suffix_start = daily.len().saturating_sub(30);
            let included_starts: HashSet<Timestamp> =
                daily[suffix_start..].iter().map(|b| b.start_time).collect();
            let mut workspace_totals: HashMap<String, f64> = HashMap::new();

            for bucket in &cost_buckets {
                let Some(starting_at) = bucket.starting_at.as_deref() else {
                    continue;
                };
                let Some(start_date) = Self::parse_date(starting_at) else {
                    continue;
                };
                if !included_starts.contains(&start_date) {
                    continue;
                }
                for result in bucket.results.as_deref().unwrap_or(&[]) {
                    let amount_cents = Self::parse_double(result.amount.as_deref()).unwrap_or(0.0);
                    let amount_usd = amount_cents / 100.0;
                    let workspace_name =
                        Self::display_name(result.workspace_id.as_deref(), "Default");
                    *workspace_totals.entry(workspace_name).or_insert(0.0) += amount_usd;
                }
            }

            let mut costs: Vec<CostBreakdown> = workspace_totals
                .into_iter()
                .map(|(name, cost_usd)| CostBreakdown { name, cost_usd })
                .collect();
            costs.sort_by(|lhs, rhs| {
                if lhs.cost_usd != rhs.cost_usd {
                    rhs.cost_usd.total_cmp(&lhs.cost_usd)
                } else {
                    lhs.name.cmp(&rhs.name)
                }
            });
            Some(costs)
        } else {
            None
        };

        ClaudeAdminAPIUsageSnapshot::new(daily, workspace_costs, now)
    }

    fn display_name(raw: Option<&str>, fallback: &str) -> String {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            Some(trimmed) => trimmed.to_owned(),
            None => fallback.to_owned(),
        }
    }

    fn window_bounds_utc(
        now: Timestamp,
    ) -> Result<(Timestamp, Timestamp), ClaudeAdminAPIUsageError> {
        let utc_date = now.to_zoned(TimeZone::UTC).date();
        let today = utc_date
            .at(0, 0, 0, 0)
            .to_zoned(TimeZone::UTC)
            .map_err(|e| ClaudeAdminAPIUsageError::NetworkError(e.to_string()))?;
        let start = today
            .checked_sub(30.days())
            .map_err(|e| ClaudeAdminAPIUsageError::NetworkError(e.to_string()))?
            .timestamp();
        let end = today
            .checked_add(1.days())
            .map_err(|e| ClaudeAdminAPIUsageError::NetworkError(e.to_string()))?
            .timestamp();
        Ok((start, end))
    }

    fn cost_query_items(
        start_date: Timestamp,
        end_date: Timestamp,
        workspace_spend_enabled: bool,
    ) -> Vec<(&'static str, String)> {
        let mut items = vec![
            ("starting_at", Self::iso8601_string(start_date)),
            ("ending_at", Self::iso8601_string(end_date)),
            ("bucket_width", "1d".to_owned()),
            ("limit", "31".to_owned()),
            ("group_by[]", "description".to_owned()),
        ];
        if workspace_spend_enabled {
            items.push(("group_by[]", "workspace_id".to_owned()));
        }
        items
    }

    fn messages_query_items(
        start_date: Timestamp,
        end_date: Timestamp,
    ) -> Vec<(&'static str, String)> {
        vec![
            ("starting_at", Self::iso8601_string(start_date)),
            ("ending_at", Self::iso8601_string(end_date)),
            ("bucket_width", "1d".to_owned()),
            ("limit", "31".to_owned()),
            ("group_by[]", "model".to_owned()),
        ]
    }

    fn build_url(
        base_url: &Url,
        path: &str,
        query_items: &[(&str, String)],
    ) -> Result<Url, ClaudeAdminAPIUsageError> {
        let base = base_url.as_str().trim_end_matches('/');
        let rel = path.trim_start_matches('/');
        let mut url = Url::parse(&format!("{base}/{rel}"))
            .map_err(|e| ClaudeAdminAPIUsageError::NetworkError(e.to_string()))?;
        {
            let mut pairs = url.query_pairs_mut();
            for (k, v) in query_items {
                pairs.append_pair(k, v);
            }
        }
        validate_admin_api_url(&url).map_err(ClaudeAdminAPIUsageError::OriginRejected)?;
        Ok(url)
    }

    async fn fetch_bytes(
        net: &NetBroker,
        endpoint: &ClaudeAdminAPIEndpointConfig,
        url: Url,
        api_key: &SecretString,
        endpoint_name: &'static str,
        call: &BrokerCall,
    ) -> Result<Vec<u8>, ClaudeAdminAPIUsageError> {
        validate_admin_api_url(&url).map_err(ClaudeAdminAPIUsageError::OriginRejected)?;

        let mut req = NetRequest::new(Method::GET, url.clone());
        req.headers
            .insert(ACCEPT, HeaderValue::from_static("application/json"));
        req.headers
            .insert(USER_AGENT, HeaderValue::from_static(ADMIN_API_USER_AGENT));
        req.headers.insert(
            HeaderName::from_static(ANTHROPIC_VERSION_HEADER),
            HeaderValue::from_static(ANTHROPIC_VERSION_VALUE),
        );
        req.credentials
            .push((HeaderName::from_static(API_KEY_HEADER), api_key.clone()));
        req.timeout = Some(endpoint.timeout());

        let response = net
            .send_response(call, endpoint.declared_origins(), req)
            .await;

        let response = match response {
            Ok(resp) => resp,
            Err(NetError::Cancelled) => return Err(ClaudeAdminAPIUsageError::Cancelled),
            Err(NetError::OriginNotDeclared | NetError::OverrideRejected(_)) => {
                return Err(ClaudeAdminAPIUsageError::OriginRejected(
                    OriginError::Invalid,
                ));
            }
            Err(NetError::Http { status }) => {
                return Err(ClaudeAdminAPIUsageError::ApiError {
                    endpoint: endpoint_name,
                    status_code: status,
                });
            }
            Err(err) => return Err(ClaudeAdminAPIUsageError::NetworkError(err.to_string())),
        };

        let status = response.status.as_u16();
        if status == 200 {
            Ok(response.body.to_vec())
        } else {
            Err(ClaudeAdminAPIUsageError::ApiError {
                endpoint: endpoint_name,
                status_code: status,
            })
        }
    }

    pub fn parse_date(raw: &str) -> Option<Timestamp> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        trimmed.parse::<Timestamp>().ok()
    }

    pub fn day_string(date: Timestamp) -> String {
        let z = date.to_zoned(TimeZone::UTC);
        let d = z.date();
        format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
    }

    pub fn iso8601_string(date: Timestamp) -> String {
        let z = date.to_zoned(TimeZone::UTC);
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            z.year(),
            z.month(),
            z.day(),
            z.hour(),
            z.minute(),
            z.second()
        )
    }

    fn parse_double(raw: Option<&str>) -> Option<f64> {
        let trimmed = raw?.trim();
        if trimmed.is_empty() {
            return None;
        }
        trimmed.parse::<f64>().ok().filter(|v| v.is_finite())
    }
}
