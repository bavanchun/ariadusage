// Ported from CodexBar Sources/CodexBarCLI/CLIClaudeSwapCards.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeSwap/ClaudeSwapAccountList.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeSwap/ClaudeSwapAccountProjection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeSwap/ClaudeSwapAccountReader.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeSwap/ClaudeSwapAccountSource.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeSwap/ClaudeSwapRetainedUsageStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIUsageCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::future::Future;
use std::path::Path;
use std::time::Duration;

use ariadusage_core::config::ProviderConfig;
use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_core::settings_value::SettingsValue;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::process::{
    AbsolutePath, Command, LaunchMode, ProcessEnv, ProcessError, ProcessRegistry, StreamPolicy,
};
use ariadusage_protocol::{
    CliNamedRateWindow, CliProviderCostSnapshot, CliProviderIdentitySnapshot, CliRateWindow,
    CliUsageSnapshot, ProviderPayload, ProviderResetCreditsPayload, ProviderStatusPayload,
};
use jiff::Timestamp;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::cards_brief_renderer::CLICardsBriefRenderer;
use crate::cards_renderer::{CLICardFailure, CLICardModel, CLICardsRenderer};
use crate::exit_code::ExitCode;
use crate::format::{ResetTimeDisplayStyle, format_reset_line};
use crate::options::{
    CliSourceMode, TokenAccountCliSelection, TokenAccountSet, UsageCommandOutput, format_header,
    make_error_payload, resolve_provider_token_accounts,
};
use crate::provider_meta::{CliProvider, make_provider_window_labels_payload};
use crate::renderer::{CLIRenderer, RenderContext};

pub struct CLIClaudeSwapText;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscapeState {
    Plain,
    Escape,
    ControlSequence,
    OperatingSystemCommand,
    OperatingSystemCommandEscape,
}

impl CLIClaudeSwapText {
    pub const LABEL_SCALAR_LIMIT: usize = 256;
    pub const DIAGNOSTIC_SCALAR_LIMIT: usize = 512;

    #[must_use]
    pub fn sanitize_label(text: &str) -> String {
        Self::sanitize(text, Self::LABEL_SCALAR_LIMIT)
    }

    #[must_use]
    pub fn sanitize_diagnostic(text: &str) -> String {
        Self::sanitize(text, Self::DIAGNOSTIC_SCALAR_LIMIT)
    }

    fn sanitize(text: &str, scalar_limit: usize) -> String {
        let mut state = EscapeState::Plain;
        let mut scalars: Vec<char> = Vec::with_capacity(text.len().min(scalar_limit));

        for scalar in text.chars() {
            let value = scalar as u32;
            match state {
                EscapeState::Escape => {
                    if value == 0x5B {
                        state = EscapeState::ControlSequence;
                    } else if value == 0x5D {
                        state = EscapeState::OperatingSystemCommand;
                    } else if (0x30..=0x7E).contains(&value) {
                        state = EscapeState::Plain;
                    }
                }
                EscapeState::ControlSequence => {
                    if (0x40..=0x7E).contains(&value) {
                        state = EscapeState::Plain;
                    }
                }
                EscapeState::OperatingSystemCommand => {
                    if value == 0x07 {
                        state = EscapeState::Plain;
                    } else if value == 0x1B {
                        state = EscapeState::OperatingSystemCommandEscape;
                    }
                }
                EscapeState::OperatingSystemCommandEscape => {
                    state = if value == 0x5C {
                        EscapeState::Plain
                    } else {
                        EscapeState::OperatingSystemCommand
                    };
                }
                EscapeState::Plain => match value {
                    0x0A | 0x0D | 0x2028 | 0x2029 => {
                        scalars.push(' ');
                    }
                    0x1B => {
                        state = EscapeState::Escape;
                    }
                    0x9B => {
                        state = EscapeState::ControlSequence;
                    }
                    0x9D => {
                        state = EscapeState::OperatingSystemCommand;
                    }
                    _ => {
                        if !scalar.is_control() && !Self::is_unicode_format_char(value) {
                            scalars.push(scalar);
                        }
                    }
                },
            }
        }

        let limited: String = scalars.into_iter().take(scalar_limit).collect();
        limited.trim().to_string()
    }

    const fn is_unicode_format_char(value: u32) -> bool {
        matches!(
            value,
            0x00AD
                | 0x0600..=0x0605
                | 0x061C
                | 0x06DD
                | 0x070F
                | 0x0890..=0x0891
                | 0x08E2
                | 0x180E
                | 0x200B..=0x200F
                | 0x202A..=0x202E
                | 0x2060..=0x2064
                | 0x2066..=0x206F
                | 0xFEFF
                | 0xFFF9..=0xFFFB
                | 0x110BD
                | 0x110CD
                | 0x13430..=0x1343F
                | 0x1BCA0..=0x1BCA3
                | 0x1D173..=0x1D17A
                | 0xE0001
                | 0xE0020..=0xE007F
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ClaudeSwapAccountSource {
    #[default]
    ClaudeSwap,
    Tcr,
}

impl ClaudeSwapAccountSource {
    pub const ALL_CASES: [Self; 2] = [Self::ClaudeSwap, Self::Tcr];

    #[must_use]
    pub const fn raw_value(self) -> &'static str {
        match self {
            Self::ClaudeSwap => "claude-swap",
            Self::Tcr => "tcr",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        self.raw_value()
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        self.raw_value()
    }

    #[must_use]
    pub const fn default_binary_name(self) -> &'static str {
        self.raw_value()
    }

    #[must_use]
    pub const fn list_arguments(self) -> &'static [&'static str] {
        match self {
            Self::ClaudeSwap => &["--list", "--json"],
            Self::Tcr => &["list", "--json"],
        }
    }

    #[must_use]
    pub fn from_identity_source(raw: &str) -> Option<Self> {
        match raw.trim() {
            "claude-swap" => Some(Self::ClaudeSwap),
            "tcr" => Some(Self::Tcr),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeSwapUsageStatus {
    Ok,
    TokenExpired,
    ReloginRequired,
    ApiKey,
    KeychainUnavailable,
    NoCredentials,
    ForeignCredential,
    Unavailable,
    Unknown(String),
}

impl ClaudeSwapUsageStatus {
    #[must_use]
    pub fn from_raw(raw_value: &str) -> Self {
        match raw_value {
            "ok" => Self::Ok,
            "token_expired" => Self::TokenExpired,
            "relogin_required" => Self::ReloginRequired,
            "api_key" => Self::ApiKey,
            "keychain_unavailable" => Self::KeychainUnavailable,
            "no_credentials" => Self::NoCredentials,
            "foreign_credential" => Self::ForeignCredential,
            "unavailable" => Self::Unavailable,
            _ => Self::Unknown(raw_value.to_string()),
        }
    }

    #[must_use]
    pub fn raw_value(&self) -> &str {
        match self {
            Self::Ok => "ok",
            Self::TokenExpired => "token_expired",
            Self::ReloginRequired => "relogin_required",
            Self::ApiKey => "api_key",
            Self::KeychainUnavailable => "keychain_unavailable",
            Self::NoCredentials => "no_credentials",
            Self::ForeignCredential => "foreign_credential",
            Self::Unavailable => "unavailable",
            Self::Unknown(value) => value.as_str(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeSwapUsageWindow {
    pub used_percent: f64,
    pub resets_at: Option<Timestamp>,
}

impl ClaudeSwapUsageWindow {
    #[must_use]
    pub fn new(used_percent: f64, resets_at: Option<Timestamp>) -> Self {
        Self {
            used_percent,
            resets_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeSwapScopedUsageWindow {
    pub name: String,
    pub used_percent: f64,
    pub resets_at: Option<Timestamp>,
}

impl ClaudeSwapScopedUsageWindow {
    #[must_use]
    pub fn new(name: impl Into<String>, used_percent: f64, resets_at: Option<Timestamp>) -> Self {
        Self {
            name: name.into(),
            used_percent,
            resets_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeSwapSpendWindow {
    pub used: f64,
    pub limit: f64,
    pub used_percent: f64,
    pub currency_code: String,
    pub resets_at: Option<Timestamp>,
}

impl ClaudeSwapSpendWindow {
    #[must_use]
    pub fn new(
        used: f64,
        limit: f64,
        used_percent: f64,
        currency_code: impl Into<String>,
        resets_at: Option<Timestamp>,
    ) -> Self {
        Self {
            used,
            limit,
            used_percent,
            currency_code: currency_code.into(),
            resets_at,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClaudeSwapUsageMeasurement {
    pub five_hour: Option<ClaudeSwapUsageWindow>,
    pub seven_day: Option<ClaudeSwapUsageWindow>,
    pub scoped: Vec<ClaudeSwapScopedUsageWindow>,
    pub spend: Option<ClaudeSwapSpendWindow>,
}

impl ClaudeSwapUsageMeasurement {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.five_hour.is_none()
            && self.seven_day.is_none()
            && self.scoped.is_empty()
            && self.spend.is_none()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeSwapLastGoodUsage {
    pub measurement: ClaudeSwapUsageMeasurement,
    pub fetched_at: Timestamp,
}

impl ClaudeSwapLastGoodUsage {
    #[must_use]
    pub fn new(measurement: ClaudeSwapUsageMeasurement, fetched_at: Timestamp) -> Self {
        Self {
            measurement,
            fetched_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeSwapAccountRow {
    pub number: i32,
    pub email: String,
    pub organization_uuid: String,
    pub organization_name: String,
    pub alias: Option<String>,
    pub is_active: bool,
    pub usage_status: ClaudeSwapUsageStatus,
    pub five_hour: Option<ClaudeSwapUsageWindow>,
    pub seven_day: Option<ClaudeSwapUsageWindow>,
    pub scoped: Vec<ClaudeSwapScopedUsageWindow>,
    pub usage_fetched_at: Option<Timestamp>,
    pub spend: Option<ClaudeSwapSpendWindow>,
    pub is_disabled: bool,
    pub last_good_usage: Option<ClaudeSwapLastGoodUsage>,
}

impl ClaudeSwapAccountRow {
    #[must_use]
    pub fn new(
        number: i32,
        email: impl Into<String>,
        is_active: bool,
        usage_status: ClaudeSwapUsageStatus,
        five_hour: Option<ClaudeSwapUsageWindow>,
        seven_day: Option<ClaudeSwapUsageWindow>,
    ) -> Self {
        Self {
            number,
            email: email.into(),
            organization_uuid: String::new(),
            organization_name: String::new(),
            alias: None,
            is_active,
            usage_status,
            five_hour,
            seven_day,
            scoped: Vec::new(),
            usage_fetched_at: None,
            spend: None,
            is_disabled: false,
            last_good_usage: None,
        }
    }

    #[must_use]
    pub fn measurement(&self) -> ClaudeSwapUsageMeasurement {
        ClaudeSwapUsageMeasurement {
            five_hour: self.five_hour.clone(),
            seven_day: self.seven_day.clone(),
            scoped: self.scoped.clone(),
            spend: self.spend.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeSwapAccountList {
    pub active_account_number: Option<i32>,
    pub accounts: Vec<ClaudeSwapAccountRow>,
    pub supports_account_switching: bool,
}

impl Default for ClaudeSwapAccountList {
    fn default() -> Self {
        Self {
            active_account_number: None,
            accounts: Vec::new(),
            supports_account_switching: true,
        }
    }
}

impl ClaudeSwapAccountList {
    #[must_use]
    pub fn new(active_account_number: Option<i32>, accounts: Vec<ClaudeSwapAccountRow>) -> Self {
        Self {
            active_account_number,
            accounts,
            supports_account_switching: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeSwapListParserError {
    NotJsonObject,
    MissingSchemaVersion,
    UnsupportedSchemaVersion(i32),
    ReportedError { kind: String, message: String },
    MalformedShape(String),
}

impl std::fmt::Display for ClaudeSwapListParserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotJsonObject => {
                f.write_str("claude-swap returned output that is not a JSON object.")
            }
            Self::MissingSchemaVersion => {
                f.write_str("claude-swap output has no schemaVersion field.")
            }
            Self::UnsupportedSchemaVersion(v) => write!(
                f,
                "claude-swap output uses unsupported schema version {v}; AriadUsage supports version 1."
            ),
            Self::ReportedError { kind, message } => {
                write!(f, "claude-swap reported {kind}: {message}")
            }
            Self::MalformedShape(details) => {
                write!(f, "claude-swap output is malformed: {details}")
            }
        }
    }
}

impl std::error::Error for ClaudeSwapListParserError {}

pub struct ClaudeSwapListParser;

impl ClaudeSwapListParser {
    pub const SUPPORTED_SCHEMA_VERSION: i32 = 1;

    pub fn parse(output: &str) -> Result<ClaudeSwapAccountList, ClaudeSwapListParserError> {
        let value: Value = serde_json::from_str(output.trim())
            .map_err(|_| ClaudeSwapListParserError::NotJsonObject)?;
        let Some(object) = value.as_object() else {
            return Err(ClaudeSwapListParserError::NotJsonObject);
        };

        let Some(schema_version) = object.get("schemaVersion").and_then(Value::as_i64) else {
            return Err(ClaudeSwapListParserError::MissingSchemaVersion);
        };
        let schema_version = i32::try_from(schema_version).unwrap_or(0);
        if schema_version != Self::SUPPORTED_SCHEMA_VERSION {
            return Err(ClaudeSwapListParserError::UnsupportedSchemaVersion(
                schema_version,
            ));
        }

        if let Some(error_obj) = object.get("error").and_then(Value::as_object) {
            let kind = error_obj
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("Error")
                .to_string();
            let message = error_obj
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string();
            return Err(ClaudeSwapListParserError::ReportedError { kind, message });
        }

        let Some(raw_accounts) = object.get("accounts").and_then(Value::as_array) else {
            return Err(ClaudeSwapListParserError::MalformedShape(
                "missing accounts array".to_string(),
            ));
        };

        let Some(raw_active) = object.get("activeAccountNumber") else {
            return Err(ClaudeSwapListParserError::MalformedShape(
                "missing activeAccountNumber".to_string(),
            ));
        };

        let supports_account_switching = match object.get("supportsAccountSwitching") {
            None => true,
            Some(Value::Bool(b)) => *b,
            Some(_) => {
                return Err(ClaudeSwapListParserError::MalformedShape(
                    "supportsAccountSwitching is not a boolean".to_string(),
                ));
            }
        };

        let active_account_number = match raw_active {
            Value::Null => None,
            Value::Number(n) => {
                let Some(num) = n.as_i64().and_then(|v| i32::try_from(v).ok()) else {
                    return Err(ClaudeSwapListParserError::MalformedShape(
                        "activeAccountNumber is not a numeric slot or null".to_string(),
                    ));
                };
                if num <= 0 {
                    return Err(ClaudeSwapListParserError::MalformedShape(
                        "activeAccountNumber is not a numeric slot or null".to_string(),
                    ));
                }
                Some(num)
            }
            _ => {
                return Err(ClaudeSwapListParserError::MalformedShape(
                    "activeAccountNumber is not a numeric slot or null".to_string(),
                ));
            }
        };

        let mut seen_slots = HashSet::new();
        let mut accounts = Vec::with_capacity(raw_accounts.len());
        for raw_row in raw_accounts {
            let Some(row) = raw_row.as_object() else {
                return Err(ClaudeSwapListParserError::MalformedShape(
                    "account row is not an object".to_string(),
                ));
            };
            let account = Self::parse_row(row)?;
            if !seen_slots.insert(account.number) {
                return Err(ClaudeSwapListParserError::MalformedShape(format!(
                    "duplicate account slot {}",
                    account.number
                )));
            }
            accounts.push(account);
        }

        let active_slots: Vec<i32> = accounts
            .iter()
            .filter(|a| a.is_active)
            .map(|a| a.number)
            .collect();
        let expected_active: Vec<i32> = active_account_number.into_iter().collect();
        if active_slots != expected_active {
            return Err(ClaudeSwapListParserError::MalformedShape(
                "active account fields disagree".to_string(),
            ));
        }

        Ok(ClaudeSwapAccountList {
            active_account_number,
            accounts,
            supports_account_switching,
        })
    }

    fn parse_row(
        row: &serde_json::Map<String, Value>,
    ) -> Result<ClaudeSwapAccountRow, ClaudeSwapListParserError> {
        let Some(number) = row
            .get("number")
            .and_then(Value::as_i64)
            .and_then(|v| i32::try_from(v).ok())
        else {
            return Err(ClaudeSwapListParserError::MalformedShape(
                "account row has no numeric slot".to_string(),
            ));
        };
        if number <= 0 {
            return Err(ClaudeSwapListParserError::MalformedShape(
                "account slot must be positive".to_string(),
            ));
        }
        let Some(is_active) = row.get("active").and_then(Value::as_bool) else {
            return Err(ClaudeSwapListParserError::MalformedShape(format!(
                "slot {number} has no active flag"
            )));
        };
        let Some(raw_status) = row.get("usageStatus").and_then(Value::as_str) else {
            return Err(ClaudeSwapListParserError::MalformedShape(format!(
                "slot {number} has no usageStatus"
            )));
        };

        let email = row
            .get("email")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let organization_uuid = row
            .get("organizationUuid")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let organization_name = row
            .get("organizationName")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let alias = row
            .get("alias")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);
        let usage = row.get("usage").and_then(Value::as_object);

        Ok(ClaudeSwapAccountRow {
            number,
            email,
            organization_uuid,
            organization_name,
            alias,
            is_active,
            usage_status: ClaudeSwapUsageStatus::from_raw(raw_status),
            five_hour: Self::parse_window(
                usage.and_then(|u| u.get("fiveHour")),
                number,
                "fiveHour",
            )?,
            seven_day: Self::parse_window(
                usage.and_then(|u| u.get("sevenDay")),
                number,
                "sevenDay",
            )?,
            scoped: Self::parse_scoped_windows(usage.and_then(|u| u.get("scoped"))),
            usage_fetched_at: Self::parse_date(row.get("usageFetchedAt")),
            spend: Self::parse_spend_window(usage.and_then(|u| u.get("spend"))),
            is_disabled: row
                .get("disabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            last_good_usage: Self::parse_last_good_usage(row, number),
        })
    }

    fn parse_last_good_usage(
        row: &serde_json::Map<String, Value>,
        slot: i32,
    ) -> Option<ClaudeSwapLastGoodUsage> {
        let raw = row.get("lastGoodUsage")?.as_object()?;
        let fetched_at = Self::parse_date(row.get("lastGoodFetchedAt"))?;
        let measurement = ClaudeSwapUsageMeasurement {
            five_hour: Self::parse_window(raw.get("fiveHour"), slot, "lastGoodUsage.fiveHour")
                .ok()
                .flatten(),
            seven_day: Self::parse_window(raw.get("sevenDay"), slot, "lastGoodUsage.sevenDay")
                .ok()
                .flatten(),
            scoped: Self::parse_scoped_windows(raw.get("scoped")),
            spend: Self::parse_spend_window(raw.get("spend")),
        };
        if measurement.is_empty() {
            return None;
        }
        Some(ClaudeSwapLastGoodUsage {
            measurement,
            fetched_at,
        })
    }

    fn parse_spend_window(raw: Option<&Value>) -> Option<ClaudeSwapSpendWindow> {
        let window = raw?.as_object()?;
        let used = Self::finite_double(window.get("used"))?;
        let limit = Self::finite_double(window.get("limit"))?;
        let percent = Self::finite_double(window.get("pct"))?;
        if used < 0.0 || limit <= 0.0 {
            return None;
        }
        let currency = window
            .get("currency")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("USD")
            .to_string();
        Some(ClaudeSwapSpendWindow {
            used,
            limit,
            used_percent: percent.clamp(0.0, 100.0),
            currency_code: currency,
            resets_at: Self::parse_date(window.get("resetsAt")),
        })
    }

    fn parse_window(
        raw: Option<&Value>,
        slot: i32,
        name: &str,
    ) -> Result<Option<ClaudeSwapUsageWindow>, ClaudeSwapListParserError> {
        let Some(raw) = raw.filter(|v| !v.is_null()) else {
            return Ok(None);
        };
        let Some(window) = raw.as_object() else {
            return Err(ClaudeSwapListParserError::MalformedShape(format!(
                "slot {slot} {name} window is not an object"
            )));
        };
        let Some(pct) = Self::finite_double(window.get("pct")) else {
            return Err(ClaudeSwapListParserError::MalformedShape(format!(
                "slot {slot} {name} percent is not a finite number"
            )));
        };
        Ok(Some(ClaudeSwapUsageWindow {
            used_percent: pct.clamp(0.0, 100.0),
            resets_at: Self::parse_date(window.get("resetsAt")),
        }))
    }

    fn parse_scoped_windows(raw: Option<&Value>) -> Vec<ClaudeSwapScopedUsageWindow> {
        let Some(arr) = raw.and_then(Value::as_array) else {
            return Vec::new();
        };
        arr.iter()
            .filter_map(|item| {
                let obj = item.as_object()?;
                let name = obj
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())?
                    .to_string();
                let pct = Self::finite_double(obj.get("pct"))?;
                Some(ClaudeSwapScopedUsageWindow {
                    name,
                    used_percent: pct.clamp(0.0, 100.0),
                    resets_at: Self::parse_date(obj.get("resetsAt")),
                })
            })
            .collect()
    }

    fn finite_double(raw: Option<&Value>) -> Option<f64> {
        let val = raw?;
        if val.is_boolean() {
            return None;
        }
        val.as_f64().filter(|f| f.is_finite())
    }

    fn parse_date(raw: Option<&Value>) -> Option<Timestamp> {
        let text = raw?.as_str()?.trim();
        if text.is_empty() {
            return None;
        }
        text.parse::<Timestamp>().ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderAccountIdentity {
    pub source: String,
    pub opaque_id: String,
}

impl ProviderAccountIdentity {
    #[must_use]
    pub fn new(source: impl Into<String>, opaque_id: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            opaque_id: opaque_id.into(),
        }
    }

    #[must_use]
    pub fn id(&self) -> String {
        format!("{}:{}", self.source, self.opaque_id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderAccountUsageSnapshot {
    pub id: ProviderAccountIdentity,
    pub provider: CliProvider,
    pub display_label: String,
    pub account_email: Option<String>,
    pub history_account_key: Option<String>,
    pub is_active: bool,
    pub can_activate: bool,
    pub uses_last_known_usage: bool,
    pub snapshot: Option<CliUsageSnapshot>,
    pub error: Option<String>,
    pub source_label: String,
}

impl ProviderAccountUsageSnapshot {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        id: ProviderAccountIdentity,
        provider: CliProvider,
        display_label: impl Into<String>,
        account_email: Option<String>,
        history_account_key: Option<String>,
        is_active: bool,
        can_activate: bool,
        uses_last_known_usage: bool,
        snapshot: Option<CliUsageSnapshot>,
        error: Option<String>,
        source_label: impl Into<String>,
    ) -> Self {
        Self {
            id,
            provider,
            display_label: display_label.into(),
            account_email,
            history_account_key,
            is_active,
            can_activate,
            uses_last_known_usage,
            snapshot,
            error,
            source_label: source_label.into(),
        }
    }
}

enum ProjectedUsage {
    None,
    Live(CliUsageSnapshot),
    LastKnown(CliUsageSnapshot),
}

impl ProjectedUsage {
    fn snapshot(&self) -> Option<&CliUsageSnapshot> {
        match self {
            Self::None => None,
            Self::Live(s) | Self::LastKnown(s) => Some(s),
        }
    }

    const fn uses_last_known_usage(&self) -> bool {
        matches!(self, Self::LastKnown(_))
    }
}

pub struct ClaudeSwapAccountProjection;

impl ClaudeSwapAccountProjection {
    pub const SOURCE_LABEL: &'static str = "claude-swap";
    const FIVE_HOUR_WINDOW_MINUTES: i64 = 5 * 60;
    const SEVEN_DAY_WINDOW_MINUTES: i64 = 7 * 24 * 60;
    const EXHAUSTED_USED_PERCENT: f64 = 100.0;
    const UNAVAILABLE_USAGE_NOTE: &'static str = "Usage unavailable.";

    #[must_use]
    pub const fn should_present_accounts(account_count: usize, show_single_account: bool) -> bool {
        account_count >= if show_single_account { 1 } else { 2 }
    }

    #[must_use]
    pub fn account_snapshots(
        list: &ClaudeSwapAccountList,
        source: ClaudeSwapAccountSource,
        previous_accounts: &[ProviderAccountUsageSnapshot],
        now: Timestamp,
    ) -> Vec<ProviderAccountUsageSnapshot> {
        let mut previous_by_id: HashMap<ProviderAccountIdentity, &ProviderAccountUsageSnapshot> =
            HashMap::new();
        for acc in previous_accounts {
            previous_by_id.entry(acc.id.clone()).or_insert(acc);
        }

        let mut ordered = list.accounts.clone();
        ordered.sort_by(|lhs, rhs| {
            if lhs.is_active != rhs.is_active {
                rhs.is_active.cmp(&lhs.is_active)
            } else {
                lhs.number.cmp(&rhs.number)
            }
        });

        let duplicate_emails = Self::duplicate_emails(&ordered);
        let labels = Self::display_labels(&ordered, &duplicate_emails);

        ordered
            .iter()
            .zip(labels)
            .map(|(row, label)| {
                let id = ProviderAccountIdentity::new(source.label(), row.number.to_string());
                let projected =
                    Self::usage_snapshot(row, source, previous_by_id.get(&id).copied(), now);
                let error = Self::error_text(row, source, &projected, now);
                let uses_last_known = projected.uses_last_known_usage();
                let snapshot = match projected {
                    ProjectedUsage::None => None,
                    ProjectedUsage::Live(s) | ProjectedUsage::LastKnown(s) => Some(s),
                };
                ProviderAccountUsageSnapshot {
                    id,
                    provider: CliProvider::Claude,
                    display_label: Self::decorated_label(&label, row),
                    account_email: (!row.email.is_empty()).then(|| row.email.clone()),
                    history_account_key: None,
                    is_active: row.is_active,
                    can_activate: list.supports_account_switching && Self::can_activate(row),
                    uses_last_known_usage: uses_last_known,
                    snapshot,
                    error,
                    source_label: source.label().to_string(),
                }
            })
            .collect()
    }

    fn decorated_label(label: &str, row: &ClaudeSwapAccountRow) -> String {
        if row.is_disabled {
            format!("{label} (disabled)")
        } else {
            label.to_string()
        }
    }

    fn display_label(row: &ClaudeSwapAccountRow, duplicate_emails: &HashSet<String>) -> String {
        if let Some(alias) = row
            .alias
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return alias.to_string();
        }
        if row.email.is_empty() {
            return format!("Account {}", row.number);
        }
        if !duplicate_emails.contains(&Self::normalized_email(&row.email)) {
            return row.email.clone();
        }
        if !row.organization_name.is_empty() {
            return format!("{} · {}", row.email, row.organization_name);
        }
        format!("{} · Account {}", row.email, row.number)
    }

    fn display_labels(
        rows: &[ClaudeSwapAccountRow],
        duplicate_emails: &HashSet<String>,
    ) -> Vec<String> {
        let candidates: Vec<String> = rows
            .iter()
            .map(|r| Self::display_label(r, duplicate_emails))
            .collect();
        let mut collision_counts: HashMap<String, usize> = HashMap::new();
        for (row, label) in rows.iter().zip(&candidates) {
            if duplicate_emails.contains(&Self::normalized_email(&row.email))
                && row
                    .alias
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .is_none()
            {
                *collision_counts
                    .entry(label.to_ascii_lowercase())
                    .or_insert(0) += 1;
            }
        }
        rows.iter()
            .zip(candidates)
            .map(|(row, label)| {
                if duplicate_emails.contains(&Self::normalized_email(&row.email))
                    && row
                        .alias
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .is_none()
                    && collision_counts
                        .get(&label.to_ascii_lowercase())
                        .copied()
                        .unwrap_or(0)
                        > 1
                {
                    format!("{label} · Account {}", row.number)
                } else {
                    label
                }
            })
            .collect()
    }

    fn duplicate_emails(rows: &[ClaudeSwapAccountRow]) -> HashSet<String> {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for row in rows {
            let norm = Self::normalized_email(&row.email);
            if !norm.is_empty() {
                *counts.entry(norm).or_insert(0) += 1;
            }
        }
        counts
            .into_iter()
            .filter_map(|(email, count)| (count > 1).then_some(email))
            .collect()
    }

    fn normalized_email(email: &str) -> String {
        email.trim().to_ascii_lowercase()
    }

    fn usage_snapshot(
        row: &ClaudeSwapAccountRow,
        source: ClaudeSwapAccountSource,
        previous: Option<&ProviderAccountUsageSnapshot>,
        now: Timestamp,
    ) -> ProjectedUsage {
        match row.usage_status {
            ClaudeSwapUsageStatus::Ok | ClaudeSwapUsageStatus::Unavailable => {
                if let Some(projected) = Self::snapshot_from_measurement(
                    &row.measurement(),
                    row,
                    source,
                    row.usage_fetched_at.unwrap_or(now),
                ) {
                    if row.usage_status == ClaudeSwapUsageStatus::Ok {
                        return ProjectedUsage::Live(projected);
                    }
                    let id = projected
                        .identity
                        .clone()
                        .unwrap_or_else(|| Self::identity_snapshot(row, source));
                    if let Some(pruned) = Self::pruned_at_limit_snapshot(&projected, id, now) {
                        return ProjectedUsage::Live(pruned);
                    }
                }
                if row.usage_status != ClaudeSwapUsageStatus::Unavailable {
                    return ProjectedUsage::None;
                }
                if let Some(reported) = Self::last_good_snapshot(row, source) {
                    return ProjectedUsage::LastKnown(reported);
                }
                if let Some(retained) = Self::retained_at_limit_snapshot(previous, row, source, now)
                {
                    return if previous.is_some_and(|p| p.uses_last_known_usage) {
                        ProjectedUsage::LastKnown(retained)
                    } else {
                        ProjectedUsage::Live(retained)
                    };
                }
                ProjectedUsage::None
            }
            ClaudeSwapUsageStatus::TokenExpired
            | ClaudeSwapUsageStatus::ReloginRequired
            | ClaudeSwapUsageStatus::ApiKey
            | ClaudeSwapUsageStatus::KeychainUnavailable
            | ClaudeSwapUsageStatus::NoCredentials
            | ClaudeSwapUsageStatus::ForeignCredential
            | ClaudeSwapUsageStatus::Unknown(_) => Self::last_good_snapshot(row, source)
                .map_or(ProjectedUsage::None, ProjectedUsage::LastKnown),
        }
    }

    fn last_good_snapshot(
        row: &ClaudeSwapAccountRow,
        source: ClaudeSwapAccountSource,
    ) -> Option<CliUsageSnapshot> {
        let last_good = row.last_good_usage.as_ref()?;
        Self::snapshot_from_measurement(&last_good.measurement, row, source, last_good.fetched_at)
    }

    fn retained_at_limit_snapshot(
        previous: Option<&ProviderAccountUsageSnapshot>,
        row: &ClaudeSwapAccountRow,
        source: ClaudeSwapAccountSource,
        now: Timestamp,
    ) -> Option<CliUsageSnapshot> {
        let previous = previous?;
        let snapshot = previous.snapshot.as_ref()?;
        let prev_fp = ClaudeSwapRetainedUsageStore::fingerprint_from_account(previous)?;
        let row_fp = ClaudeSwapRetainedUsageStore::fingerprint_for_email_slot(
            &row.email,
            &row.number.to_string(),
        )?;
        if prev_fp != row_fp {
            return None;
        }
        Self::pruned_at_limit_snapshot(snapshot, Self::identity_snapshot(row, source), now)
    }

    fn pruned_at_limit_snapshot(
        snapshot: &CliUsageSnapshot,
        identity: CliProviderIdentitySnapshot,
        now: Timestamp,
    ) -> Option<CliUsageSnapshot> {
        let primary = Self::unexpired_window(snapshot.primary.as_ref(), now);
        let secondary = Self::unexpired_window(snapshot.secondary.as_ref(), now);
        let extra: Vec<CliNamedRateWindow> = snapshot
            .extra_rate_windows
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter_map(|named| {
                let win = Self::unexpired_window(Some(&named.window), now)?;
                Some(CliNamedRateWindow {
                    id: named.id.clone(),
                    title: named.title.clone(),
                    window: win,
                    usage_known: named.usage_known,
                })
            })
            .collect();

        let has_exhausted = primary
            .as_ref()
            .is_some_and(|w| w.used_percent >= Self::EXHAUSTED_USED_PERCENT)
            || secondary
                .as_ref()
                .is_some_and(|w| w.used_percent >= Self::EXHAUSTED_USED_PERCENT)
            || extra
                .iter()
                .any(|n| n.window.used_percent >= Self::EXHAUSTED_USED_PERCENT);
        if !has_exhausted {
            return None;
        }

        let mut pruned = CliUsageSnapshot::new(
            primary,
            secondary,
            None,
            snapshot.updated_at.clone(),
            Some(identity),
        );
        pruned.extra_rate_windows = (!extra.is_empty()).then_some(extra);
        pruned.data_confidence = snapshot.data_confidence.clone();
        Some(pruned)
    }

    fn unexpired_window(window: Option<&CliRateWindow>, now: Timestamp) -> Option<CliRateWindow> {
        let window = window?;
        let resets_at = window.resets_at.as_deref()?.parse::<Timestamp>().ok()?;
        (resets_at > now).then(|| window.clone())
    }

    fn snapshot_from_measurement(
        measurement: &ClaudeSwapUsageMeasurement,
        row: &ClaudeSwapAccountRow,
        source: ClaudeSwapAccountSource,
        updated_at: Timestamp,
    ) -> Option<CliUsageSnapshot> {
        let primary = measurement.five_hour.as_ref().map(|w| {
            CliRateWindow::new(
                w.used_percent,
                Some(Self::FIVE_HOUR_WINDOW_MINUTES),
                w.resets_at.map(|ts| ts.to_string()),
                None,
            )
        });
        let secondary = measurement.seven_day.as_ref().map(|w| {
            CliRateWindow::new(
                w.used_percent,
                Some(Self::SEVEN_DAY_WINDOW_MINUTES),
                w.resets_at.map(|ts| ts.to_string()),
                None,
            )
        });
        let scoped: Vec<CliNamedRateWindow> = measurement
            .scoped
            .iter()
            .enumerate()
            .map(|(idx, w)| CliNamedRateWindow {
                id: format!("scoped-{idx}"),
                title: format!("{} only", w.name),
                window: CliRateWindow::new(
                    w.used_percent,
                    Some(Self::SEVEN_DAY_WINDOW_MINUTES),
                    w.resets_at.map(|ts| ts.to_string()),
                    None,
                ),
                usage_known: true,
            })
            .collect();
        let provider_cost = measurement.spend.as_ref().map(|s| {
            CliProviderCostSnapshot::new(
                s.used,
                s.limit,
                s.currency_code.clone(),
                None,
                s.resets_at.map(|ts| ts.to_string()),
                updated_at.to_string(),
            )
        });

        if primary.is_none() && secondary.is_none() && scoped.is_empty() && provider_cost.is_none()
        {
            return None;
        }

        let mut snap = CliUsageSnapshot::new(
            primary,
            secondary,
            None,
            updated_at.to_string(),
            Some(Self::identity_snapshot(row, source)),
        );
        snap.extra_rate_windows = (!scoped.is_empty()).then_some(scoped);
        snap.provider_cost = provider_cost;
        Some(snap)
    }

    fn identity_snapshot(
        row: &ClaudeSwapAccountRow,
        source: ClaudeSwapAccountSource,
    ) -> CliProviderIdentitySnapshot {
        let mut id = CliProviderIdentitySnapshot::new(
            Some(CliProvider::Claude.raw_value().to_string()),
            (!row.email.is_empty()).then(|| row.email.clone()),
            None,
            Some(source.label().to_string()),
        );
        id.account_id = Some(format!("{}:{}", source.label(), row.number));
        id
    }

    fn error_text(
        row: &ClaudeSwapAccountRow,
        source: ClaudeSwapAccountSource,
        usage: &ProjectedUsage,
        now: Timestamp,
    ) -> Option<String> {
        let tool = source.label();
        match &row.usage_status {
            ClaudeSwapUsageStatus::Ok => usage
                .snapshot()
                .is_none()
                .then(|| "No usage windows reported.".to_string()),
            ClaudeSwapUsageStatus::TokenExpired => Some(format!(
                "Token expired. Switch to this account in {tool} to refresh it."
            )),
            ClaudeSwapUsageStatus::ReloginRequired => Some(format!(
                "Re-login required. Re-authenticate this account in {tool}."
            )),
            ClaudeSwapUsageStatus::ApiKey => {
                Some("API-key account; subscription usage is unavailable.".to_string())
            }
            ClaudeSwapUsageStatus::KeychainUnavailable => Some(format!(
                "{tool} could not read the active account's Keychain entry."
            )),
            ClaudeSwapUsageStatus::NoCredentials => {
                Some("No stored credentials for this account slot.".to_string())
            }
            ClaudeSwapUsageStatus::ForeignCredential => Some(format!(
                "{tool} reports the live credential belongs to a different account. Re-authenticate this account in {tool} to restore its saved login."
            )),
            ClaudeSwapUsageStatus::Unavailable => {
                if usage.uses_last_known_usage() {
                    Some(Self::UNAVAILABLE_USAGE_NOTE.to_string())
                } else {
                    Self::at_limit_note(usage.snapshot(), now)
                        .or_else(|| Some(Self::UNAVAILABLE_USAGE_NOTE.to_string()))
                }
            }
            ClaudeSwapUsageStatus::Unknown(raw) => {
                Some(format!("Unrecognized {tool} status: {raw}"))
            }
        }
    }

    fn at_limit_note(snapshot: Option<&CliUsageSnapshot>, now: Timestamp) -> Option<String> {
        let snapshot = snapshot?;
        let mut parts = Vec::new();
        if let Some(primary) = snapshot.primary.as_ref() {
            Self::append_limit("Session", primary, now, &mut parts);
        }
        if let Some(secondary) = snapshot.secondary.as_ref() {
            Self::append_limit("Weekly", secondary, now, &mut parts);
        }
        for extra in snapshot.extra_rate_windows.as_deref().unwrap_or(&[]) {
            let name = extra.title.strip_suffix(" only").unwrap_or(&extra.title);
            Self::append_limit(name, &extra.window, now, &mut parts);
        }
        (!parts.is_empty()).then(|| parts.join(" "))
    }

    fn append_limit(name: &str, window: &CliRateWindow, now: Timestamp, parts: &mut Vec<String>) {
        if window.used_percent < Self::EXHAUSTED_USED_PERCENT {
            return;
        }
        let resets_at = window
            .resets_at
            .as_deref()
            .and_then(|s| s.parse::<Timestamp>().ok());
        if let Some(reset) = format_reset_line(
            resets_at,
            window.reset_description.as_deref(),
            ResetTimeDisplayStyle::Countdown,
            now,
        ) {
            parts.push(format!("{name} limit reached. {reset}."));
        } else {
            parts.push(format!("{name} limit reached."));
        }
    }

    const fn can_activate(row: &ClaudeSwapAccountRow) -> bool {
        if row.is_active {
            return matches!(row.usage_status, ClaudeSwapUsageStatus::ForeignCredential);
        }
        matches!(
            row.usage_status,
            ClaudeSwapUsageStatus::Ok
                | ClaudeSwapUsageStatus::ApiKey
                | ClaudeSwapUsageStatus::Unavailable
                | ClaudeSwapUsageStatus::ForeignCredential
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClaudeSwapRetainedUsageStore {
    retained_accounts_by_fingerprint: BTreeMap<String, Vec<ProviderAccountUsageSnapshot>>,
}

impl ClaudeSwapRetainedUsageStore {
    const FINGERPRINT_PREFIX: &'static str = "fp:";

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn previous_accounts(
        in_memory: &[ProviderAccountUsageSnapshot],
    ) -> Vec<ProviderAccountUsageSnapshot> {
        in_memory.to_vec()
    }

    #[must_use]
    pub fn fingerprint_for_email_slot(email: &str, slot: &str) -> Option<String> {
        let trimmed = email.trim().to_ascii_lowercase();
        if !trimmed.contains('@') {
            return None;
        }
        Some(format!("{slot}\0{trimmed}"))
    }

    #[must_use]
    pub fn fingerprint_from_account(account: &ProviderAccountUsageSnapshot) -> Option<String> {
        if let Some(stored) = account
            .snapshot
            .as_ref()
            .and_then(|s| s.identity.as_ref())
            .and_then(|id| id.account_id.as_deref())
            && let Some(stripped) = stored.strip_prefix(Self::FINGERPRINT_PREFIX)
        {
            return Some(stripped.to_string());
        }
        let email = account
            .snapshot
            .as_ref()
            .and_then(|s| s.identity.as_ref())
            .and_then(|id| id.account_email.as_deref())
            .or(account.account_email.as_deref())?;
        let trimmed = email.trim();
        if trimmed.is_empty() {
            return None;
        }
        Self::fingerprint_for_email_slot(trimmed, &account.id.opaque_id)
    }

    pub fn clear(&mut self) {
        self.retained_accounts_by_fingerprint.clear();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeSwapAccountReaderError {
    ExecutablePathNotConfigured,
    OutputTooLarge { byte_count: usize },
    Cancelled,
    SubprocessFailed(String),
    Parser(ClaudeSwapListParserError),
}

impl std::fmt::Display for ClaudeSwapAccountReaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExecutablePathNotConfigured => {
                f.write_str("No claude-swap executable path is configured.")
            }
            Self::OutputTooLarge { byte_count } => write!(
                f,
                "claude-swap produced {byte_count} bytes of output; refusing to parse more than {}.",
                ClaudeSwapAccountReader::MAX_OUTPUT_BYTES
            ),
            Self::Cancelled => f.write_str("claude-swap reader cancelled."),
            Self::SubprocessFailed(msg) => f.write_str(msg),
            Self::Parser(err) => std::fmt::Display::fmt(err, f),
        }
    }
}

impl std::error::Error for ClaudeSwapAccountReaderError {}

pub struct ClaudeSwapAccountReader;

impl ClaudeSwapAccountReader {
    pub const MAX_OUTPUT_BYTES: usize = 262_144;
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

    pub fn resolved_executable_path(
        configured_path: &str,
    ) -> Result<String, ClaudeSwapAccountReaderError> {
        let trimmed = configured_path.trim();
        if trimmed.is_empty() {
            return Err(ClaudeSwapAccountReaderError::ExecutablePathNotConfigured);
        }
        if trimmed == "~" {
            if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
                return Ok(home.to_string_lossy().into_owned());
            }
        } else if let Some(rest) = trimmed.strip_prefix("~/")
            && let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty())
        {
            return Ok(Path::new(&home).join(rest).to_string_lossy().into_owned());
        }
        Ok(trimmed.to_string())
    }

    pub async fn read_account_list(
        executable_path: &str,
        source: ClaudeSwapAccountSource,
    ) -> Result<ClaudeSwapAccountList, ClaudeSwapAccountReaderError> {
        Self::read_account_list_with_timeout(executable_path, source, Self::DEFAULT_TIMEOUT).await
    }

    pub async fn read_account_list_with_timeout(
        executable_path: &str,
        source: ClaudeSwapAccountSource,
        timeout: Duration,
    ) -> Result<ClaudeSwapAccountList, ClaudeSwapAccountReaderError> {
        let binary = Self::resolved_executable_path(executable_path)?;
        let program = AbsolutePath::new(Path::new(&binary))
            .map_err(|e| ClaudeSwapAccountReaderError::SubprocessFailed(e.to_string()))?;
        let args: Vec<OsString> = source.list_arguments().iter().map(OsString::from).collect();
        let base_os: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        let env = ProcessEnv::from_allowlist(base_os);
        let mut cmd = Command::new(program, args).with_env(env).timeout(timeout);
        cmd.set_stdout(StreamPolicy::Capture {
            cap: Self::MAX_OUTPUT_BYTES,
        });
        cmd.set_stderr(StreamPolicy::Capture {
            cap: Self::MAX_OUTPUT_BYTES,
        });
        cmd.set_launch_mode(LaunchMode::Group);
        cmd.set_retry_text_busy(true);

        let registry = ProcessRegistry::default();
        let gate = LaunchGate::default();
        let call = BrokerCall {
            interaction: FetchInteraction::UserInitiated,
            cancel: CancellationToken::new(),
            request_id: "cli-claude-swap".to_string(),
        };
        let out = ariadusage_engine::brokers::process::run(call, cmd, &registry, &gate)
            .await
            .map_err(|err| match err {
                ProcessError::OutputTooLarge { .. } => {
                    ClaudeSwapAccountReaderError::OutputTooLarge {
                        byte_count: Self::MAX_OUTPUT_BYTES + 1,
                    }
                }
                ProcessError::Cancelled => ClaudeSwapAccountReaderError::Cancelled,
                other => ClaudeSwapAccountReaderError::SubprocessFailed(other.to_string()),
            })?;

        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        if stdout.len() > Self::MAX_OUTPUT_BYTES {
            return Err(ClaudeSwapAccountReaderError::OutputTooLarge {
                byte_count: stdout.len(),
            });
        }
        ClaudeSwapListParser::parse(&stdout).map_err(ClaudeSwapAccountReaderError::Parser)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CLIClaudeSwapCardsRenderOptions {
    pub status: Option<ProviderStatusPayload>,
    pub use_color: bool,
    pub reset_style: ResetTimeDisplayStyle,
    pub weekly_work_days: Option<u32>,
    pub now: Timestamp,
}

impl Default for CLIClaudeSwapCardsRenderOptions {
    fn default() -> Self {
        Self {
            status: None,
            use_color: false,
            reset_style: ResetTimeDisplayStyle::Countdown,
            weekly_work_days: None,
            now: Timestamp::now(),
        }
    }
}

pub struct CLIClaudeSwapCards;

impl CLIClaudeSwapCards {
    #[must_use]
    pub fn executable_path(config: Option<&ProviderConfig>) -> String {
        let raw_path = config
            .and_then(|c| c.extra.get("claudeSwapExecutablePath"))
            .and_then(|raw| serde_json::from_str::<String>(raw.get()).ok());
        SettingsValue::cleaned(raw_path.as_deref()).unwrap_or_default()
    }

    #[must_use]
    pub fn show_single_account(config: Option<&ProviderConfig>) -> bool {
        config
            .and_then(|c| c.extra.get("claudeSwapShowSingleAccount"))
            .and_then(|raw| serde_json::from_str::<bool>(raw.get()).ok())
            .unwrap_or(false)
    }

    #[must_use]
    pub fn is_eligible(
        provider: CliProvider,
        integration_enabled: bool,
        has_explicit_account_selection: bool,
        source_mode_override: Option<&str>,
    ) -> bool {
        provider == CliProvider::Claude
            && integration_enabled
            && !has_explicit_account_selection
            && matches!(
                source_mode_override
                    .map(|s| s.trim().to_ascii_lowercase())
                    .as_deref(),
                None | Some("auto")
            )
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn fetch<A, AFut, R, RFut>(
        eligible: bool,
        executable_path: &str,
        source: ClaudeSwapAccountSource,
        show_single_account: bool,
        render_options: &CLIClaudeSwapCardsRenderOptions,
        ambient_fetch: A,
        account_list_reader: Option<R>,
        previous_accounts: &[ProviderAccountUsageSnapshot],
    ) -> UsageCommandOutput
    where
        A: FnOnce() -> AFut,
        AFut: Future<Output = UsageCommandOutput>,
        R: FnOnce(String) -> RFut,
        RFut: Future<Output = Result<ClaudeSwapAccountList, ClaudeSwapAccountReaderError>>,
    {
        if !eligible {
            return ambient_fetch().await;
        }

        let list_result = if let Some(reader) = account_list_reader {
            reader(executable_path.to_string()).await
        } else {
            ClaudeSwapAccountReader::read_account_list(executable_path, source).await
        };

        match list_result {
            Ok(list) => {
                let retained = ClaudeSwapRetainedUsageStore::previous_accounts(previous_accounts);
                let accounts = ClaudeSwapAccountProjection::account_snapshots(
                    &list,
                    source,
                    &retained,
                    render_options.now,
                );
                if !ClaudeSwapAccountProjection::should_present_accounts(
                    accounts.len(),
                    show_single_account,
                ) {
                    return ambient_fetch().await;
                }

                let cards = Self::make_cards(&accounts, render_options);
                let brief_rows = CLICardsBriefRenderer::make_rows(&cards);
                UsageCommandOutput {
                    sections: Vec::new(),
                    cards,
                    brief_rows,
                    card_failures: Vec::new(),
                    payload: Vec::new(),
                    exit_code: ExitCode::Success,
                }
            }
            Err(err) => {
                let mut output = ambient_fetch().await;
                let diagnostic = CLIClaudeSwapText::sanitize_diagnostic(&err.to_string());
                let message = if diagnostic.is_empty() {
                    format!("{} list failed.", source.label())
                } else {
                    diagnostic
                };
                output.card_failures.push(CLICardFailure::new(
                    CliProvider::Claude,
                    Some(source.label().to_string()),
                    message,
                ));
                output.exit_code = ExitCode::Failure;
                output
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn fetch_single_provider_usage_output<F, Fut>(
        provider: CliProvider,
        app_auto_verifier: bool,
        integration_enabled: bool,
        executable_path: &str,
        source: ClaudeSwapAccountSource,
        token_selection: &TokenAccountCliSelection,
        account_set: Option<&TokenAccountSet>,
        source_mode_override: Option<CliSourceMode>,
        configured_source_mode: CliSourceMode,
        json_output: bool,
        cards_layout: bool,
        render_options: &CLIClaudeSwapCardsRenderOptions,
        previous_accounts: &[ProviderAccountUsageSnapshot],
        fallback_fetch: F,
    ) -> UsageCommandOutput
    where
        F: FnOnce(String) -> Fut,
        Fut: Future<Output = UsageCommandOutput>,
    {
        let effective_mode = source_mode_override.unwrap_or(configured_source_mode);
        let mut source_label_suffix = String::new();
        if !app_auto_verifier
            && Self::is_eligible(
                provider,
                integration_enabled,
                token_selection.uses_override(),
                Some(effective_mode.raw_value()),
            )
        {
            if let Ok(list) =
                ClaudeSwapAccountReader::read_account_list(executable_path, source).await
                && let Some(active) = ClaudeSwapAccountProjection::account_snapshots(
                    &list,
                    source,
                    previous_accounts,
                    render_options.now,
                )
                .into_iter()
                .find(|acc| acc.is_active)
                && let Some(usage) = active.snapshot.clone()
            {
                let diagnostic = active
                    .error
                    .as_deref()
                    .map(CLIClaudeSwapText::sanitize_diagnostic);
                let account_label = CLIClaudeSwapText::sanitize_label(&active.display_label);
                let notes: Vec<String> = diagnostic.iter().cloned().collect();
                let mut result = UsageCommandOutput::default();
                if json_output {
                    result.payload.push(ProviderPayload {
                        provider: provider.raw_value().to_string(),
                        account: Some(account_label),
                        version: None,
                        source: source.label().to_string(),
                        status: render_options.status.clone(),
                        rate_window_labels: make_provider_window_labels_payload(
                            provider,
                            Some(&usage),
                            render_options.now,
                        ),
                        reset_credits: ProviderResetCreditsPayload::make(
                            &usage,
                            render_options.now,
                        ),
                        pace: CLIRenderer::provider_pace_payload(
                            provider,
                            &usage,
                            render_options.now,
                            render_options.weekly_work_days,
                        ),
                        usage: Some(usage),
                        credits: None,
                        antigravity_plan_info: None,
                        openai_dashboard: None,
                        diagnostic,
                        error: None,
                        age_seconds: None,
                    });
                } else {
                    let context = RenderContext {
                        header: format_header(provider, None, source.label()),
                        status: render_options.status.clone(),
                        account_override: None,
                        fallback_account_email: None,
                        use_color: render_options.use_color,
                        reset_style: render_options.reset_style,
                        notes,
                        weekly_work_days: render_options.weekly_work_days,
                    };
                    result.sections.push(CLIRenderer::render_text(
                        provider,
                        &usage,
                        None,
                        &context,
                        render_options.now,
                    ));
                }
                return result;
            }
            source_label_suffix = format!(" ({} fallback)", source.label());
        }

        if let Err(err_msg) = resolve_provider_token_accounts(
            provider,
            configured_source_mode,
            source_mode_override,
            account_set,
            token_selection,
        ) {
            let mut output = UsageCommandOutput {
                exit_code: ExitCode::Failure,
                ..UsageCommandOutput::default()
            };
            if json_output {
                output.payload.push(ProviderPayload {
                    provider: provider.raw_value().to_string(),
                    account: None,
                    version: None,
                    source: format!(
                        "{}{}",
                        source_mode_override
                            .unwrap_or(CliSourceMode::Auto)
                            .raw_value(),
                        source_label_suffix
                    ),
                    status: render_options.status.clone(),
                    usage: None,
                    rate_window_labels: None,
                    credits: None,
                    reset_credits: None,
                    antigravity_plan_info: None,
                    openai_dashboard: None,
                    diagnostic: None,
                    error: Some(make_error_payload(
                        ExitCode::Failure,
                        err_msg,
                        ariadusage_protocol::CliErrorKind::Provider,
                    )),
                    pace: None,
                    age_seconds: None,
                });
            } else if cards_layout {
                output
                    .card_failures
                    .push(CLICardFailure::new(provider, None, err_msg));
            }
            return output;
        }

        fallback_fetch(source_label_suffix).await
    }

    #[must_use]
    pub fn make_cards(
        accounts: &[ProviderAccountUsageSnapshot],
        options: &CLIClaudeSwapCardsRenderOptions,
    ) -> Vec<CLICardModel> {
        accounts
            .iter()
            .map(|acc| CLICardsRenderer::make_claude_swap_card(acc, options))
            .collect()
    }

    #[must_use]
    pub fn make_text_sections(
        accounts: &[ProviderAccountUsageSnapshot],
        options: &CLIClaudeSwapCardsRenderOptions,
    ) -> Vec<String> {
        accounts
            .iter()
            .map(|acc| Self::render_text_section(acc, options))
            .collect()
    }

    #[must_use]
    pub fn make_payloads(
        accounts: &[ProviderAccountUsageSnapshot],
        version: Option<&str>,
        options: &CLIClaudeSwapCardsRenderOptions,
    ) -> Vec<ProviderPayload> {
        accounts
            .iter()
            .map(|acc| Self::make_payload(acc, version, options))
            .collect()
    }

    fn render_text_section(
        account: &ProviderAccountUsageSnapshot,
        options: &CLIClaudeSwapCardsRenderOptions,
    ) -> String {
        let source_label = CLIClaudeSwapText::sanitize_label(&account.source_label);
        let source_label = if source_label.is_empty() {
            "claude-swap".to_string()
        } else {
            source_label
        };
        let header = format_header(CliProvider::Claude, None, &source_label);
        let account_label = CLIClaudeSwapText::sanitize_label(&account.display_label);
        let account_label = if account.is_active {
            format!("{account_label} [active]")
        } else {
            account_label
        };

        if let Some(snapshot) = account.snapshot.as_ref()
            && !account.uses_last_known_usage
        {
            let notes: Vec<String> = account
                .error
                .as_deref()
                .map(CLIClaudeSwapText::sanitize_diagnostic)
                .into_iter()
                .collect();
            let context = RenderContext {
                header,
                status: options.status.clone(),
                account_override: Some(account_label),
                fallback_account_email: None,
                use_color: options.use_color,
                reset_style: options.reset_style,
                notes,
                weekly_work_days: options.weekly_work_days,
            };
            return CLIRenderer::render_text(
                CliProvider::Claude,
                snapshot,
                None,
                &context,
                options.now,
            );
        }

        let mut lines = Vec::new();
        lines.push(if options.use_color {
            CLIRenderer::colorize_accent_bold(&header)
        } else {
            header
        });
        lines.push(format!("Account: {account_label}"));
        let problem = account
            .error
            .as_deref()
            .map(CLIClaudeSwapText::sanitize_diagnostic)
            .unwrap_or_else(|| "Usage unavailable.".to_string());
        lines.push(if options.use_color {
            CLIRenderer::colorize_warning(&format!("Note: {problem}"))
        } else {
            format!("Note: {problem}")
        });
        if let Some(status) = options.status.as_ref() {
            lines.push(CLIRenderer::status_line(status, options.use_color));
        }
        lines.join("\n")
    }

    fn make_payload(
        account: &ProviderAccountUsageSnapshot,
        version: Option<&str>,
        options: &CLIClaudeSwapCardsRenderOptions,
    ) -> ProviderPayload {
        let live_snapshot = if account.uses_last_known_usage {
            None
        } else {
            account.snapshot.clone()
        };
        let source_label = CLIClaudeSwapText::sanitize_label(&account.source_label);
        let source_label = if source_label.is_empty() {
            "claude-swap".to_string()
        } else {
            source_label
        };
        let account_label = CLIClaudeSwapText::sanitize_label(&account.display_label);
        let pace = live_snapshot.as_ref().and_then(|snap| {
            CLIRenderer::provider_pace_payload(
                CliProvider::Claude,
                snap,
                options.now,
                options.weekly_work_days,
            )
        });
        let rate_window_labels = make_provider_window_labels_payload(
            CliProvider::Claude,
            live_snapshot.as_ref(),
            options.now,
        );
        let reset_credits = live_snapshot
            .as_ref()
            .and_then(|snap| ProviderResetCreditsPayload::make(snap, options.now));

        ProviderPayload {
            provider: CliProvider::Claude.raw_value().to_string(),
            account: Some(account_label),
            version: version.map(ToString::to_string),
            source: source_label,
            status: options.status.clone(),
            usage: live_snapshot,
            rate_window_labels,
            credits: None,
            reset_credits,
            antigravity_plan_info: None,
            openai_dashboard: None,
            diagnostic: account
                .error
                .as_deref()
                .map(CLIClaudeSwapText::sanitize_diagnostic),
            error: None,
            pace,
            age_seconds: None,
        }
    }
}
