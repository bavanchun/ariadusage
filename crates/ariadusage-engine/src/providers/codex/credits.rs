// Ported from CodexBar Sources/CodexBarCore/CreditsModels.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexExtraUsageCost.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::cmp::Ordering;
use std::fmt;

use ariadusage_core::model::cost::ProviderCostSnapshot;
use ariadusage_core::model::snapshot::UsageSnapshot;
use ariadusage_protocol::usage::{CreditLimit, Credits, ResetCredits};
use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

const RESET_CREDIT_ID_HASH_DOMAIN: &[u8] = b"ariadusage:codex-reset-credit-id:v1:";
const RESET_CREDIT_ID_PREFIX: &str = "codex-reset-credit-v1-";
const RESET_CREDIT_HASH_HEX_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditEvent {
    pub id: String,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    pub date: Timestamp,
    pub service: String,
    pub credits_used: f64,
}

impl CreditEvent {
    pub fn new(date: Timestamp, service: impl Into<String>, credits_used: f64) -> Self {
        let service = service.into();
        let id = format!("{}:{}:{credits_used}", date.as_millisecond(), service);
        Self {
            id,
            date,
            service,
            credits_used,
        }
    }

    pub fn with_id(
        id: impl Into<String>,
        date: Timestamp,
        service: impl Into<String>,
        credits_used: f64,
    ) -> Self {
        Self {
            id: id.into(),
            date,
            service: service.into(),
            credits_used,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexCreditLimitSnapshot {
    pub title: String,
    pub used: f64,
    pub limit: f64,
    pub remaining_percent: f64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub resets_at: Option<Timestamp>,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    pub updated_at: Timestamp,
}

impl CodexCreditLimitSnapshot {
    pub const DEFAULT_TITLE: &'static str = "Monthly credit limit";

    pub fn new(
        title: impl Into<String>,
        used: f64,
        limit: f64,
        remaining_percent: f64,
        resets_at: Option<Timestamp>,
        updated_at: Timestamp,
    ) -> Self {
        let raw_title = title.into();
        let trimmed = raw_title.trim();
        let normalized_title = if trimmed.is_empty() {
            Self::DEFAULT_TITLE.to_owned()
        } else {
            trimmed.to_owned()
        };
        Self {
            title: normalized_title,
            used: used.max(0.0),
            limit: limit.max(0.0),
            remaining_percent: remaining_percent.clamp(0.0, 100.0),
            resets_at,
            updated_at,
        }
    }

    pub fn new_monthly(
        used: f64,
        limit: f64,
        remaining_percent: f64,
        resets_at: Option<Timestamp>,
        updated_at: Timestamp,
    ) -> Self {
        Self::new(
            Self::DEFAULT_TITLE,
            used,
            limit,
            remaining_percent,
            resets_at,
            updated_at,
        )
    }

    pub fn remaining(&self) -> f64 {
        (self.limit - self.used).max(0.0)
    }

    pub fn used_percent(&self) -> f64 {
        (100.0 - self.remaining_percent.clamp(0.0, 100.0)).clamp(0.0, 100.0)
    }

    pub fn to_protocol(&self) -> CreditLimit {
        CreditLimit {
            title: Some(self.title.clone()),
            used: self.used,
            total: self.limit,
            remaining: Some(self.remaining()),
            unit: Some("credits".to_owned()),
            resets_at: self.resets_at,
            reset_description: None,
        }
    }
}

fn default_balance_read_succeeded() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditsSnapshot {
    pub remaining: f64,
    #[serde(default)]
    pub events: Vec<CreditEvent>,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    pub updated_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_credit_limit: Option<CodexCreditLimitSnapshot>,
    #[serde(default)]
    pub balance_is_workspace: bool,
    #[serde(default = "default_balance_read_succeeded")]
    pub balance_read_succeeded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits_available: Option<bool>,
}

impl CreditsSnapshot {
    pub fn new(remaining: f64, events: Vec<CreditEvent>, updated_at: Timestamp) -> Self {
        Self {
            remaining,
            events,
            updated_at,
            codex_credit_limit: None,
            balance_is_workspace: false,
            balance_read_succeeded: true,
            credits_available: None,
        }
    }

    pub fn with_details(
        remaining: f64,
        events: Vec<CreditEvent>,
        updated_at: Timestamp,
        codex_credit_limit: Option<CodexCreditLimitSnapshot>,
        balance_read_succeeded: bool,
        credits_available: Option<bool>,
        balance_is_workspace: bool,
    ) -> Self {
        Self {
            remaining,
            events,
            updated_at,
            codex_credit_limit,
            balance_is_workspace,
            balance_read_succeeded,
            credits_available,
        }
    }

    pub fn with_credit_limit(mut self, limit: Option<CodexCreditLimitSnapshot>) -> Self {
        self.codex_credit_limit = limit;
        self
    }

    pub fn with_balance_read_succeeded(mut self, succeeded: bool) -> Self {
        self.balance_read_succeeded = succeeded;
        self
    }

    pub fn with_credits_available(mut self, available: Option<bool>) -> Self {
        self.credits_available = available;
        self
    }

    pub fn with_balance_is_workspace(mut self, is_workspace: bool) -> Self {
        self.balance_is_workspace = is_workspace;
        self
    }

    pub fn display_remaining(&self) -> Option<f64> {
        if self.has_workspace_balance() {
            return Some(self.remaining);
        }
        self.codex_credit_limit
            .as_ref()
            .map(CodexCreditLimitSnapshot::remaining)
            .or_else(|| self.balance_read_succeeded.then_some(self.remaining))
    }

    pub fn has_workspace_balance(&self) -> bool {
        self.balance_read_succeeded && self.balance_is_workspace
    }

    pub fn with_workspace_remaining_balance(&self, remaining: f64, updated_at: Timestamp) -> Self {
        Self {
            remaining,
            events: self.events.clone(),
            updated_at,
            codex_credit_limit: self.codex_credit_limit.clone(),
            balance_is_workspace: true,
            balance_read_succeeded: true,
            credits_available: Some(true),
        }
    }

    pub fn to_protocol_credits(&self, reset_credits: Option<ResetCredits>) -> Credits {
        Credits {
            remaining: self.remaining,
            unit: Some("credits".to_owned()),
            balance_is_workspace: Some(self.balance_is_workspace),
            updated_at: Some(self.updated_at),
            balance_read_succeeded: Some(self.balance_read_succeeded),
            credits_available: self.credits_available,
            credit_limit: self
                .codex_credit_limit
                .as_ref()
                .map(CodexCreditLimitSnapshot::to_protocol),
            reset_credits,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexRateLimitResetCreditStatus {
    Available,
    Redeeming,
    Redeemed,
    Expired,
    Unknown(String),
}

impl CodexRateLimitResetCreditStatus {
    pub fn from_raw(raw_value: &str) -> Self {
        match raw_value {
            "available" => Self::Available,
            "redeeming" => Self::Redeeming,
            "redeemed" => Self::Redeemed,
            "expired" => Self::Expired,
            other => Self::Unknown(other.to_owned()),
        }
    }

    pub fn raw_value(&self) -> &str {
        match self {
            Self::Available => "available",
            Self::Redeeming => "redeeming",
            Self::Redeemed => "redeemed",
            Self::Expired => "expired",
            Self::Unknown(value) => value.as_str(),
        }
    }
}

impl Serialize for CodexRateLimitResetCreditStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.raw_value())
    }
}

impl<'de> Deserialize<'de> for CodexRateLimitResetCreditStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::from_raw(&raw))
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct CodexRateLimitResetCredit {
    pub id: String,
    #[serde(rename = "reset_type")]
    pub reset_type: String,
    pub status: CodexRateLimitResetCreditStatus,
    #[serde(rename = "granted_at", with = "ariadusage_protocol::time::rfc3339")]
    pub granted_at: Timestamp,
    #[serde(
        rename = "expires_at",
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub expires_at: Option<Timestamp>,
    #[serde(
        rename = "redeem_started_at",
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub redeem_started_at: Option<Timestamp>,
    #[serde(
        rename = "redeemed_at",
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub redeemed_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl fmt::Debug for CodexRateLimitResetCredit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexRateLimitResetCredit")
            .field("id", &self.id)
            .field("reset_type", &self.reset_type)
            .field("status", &self.status)
            .field("granted_at", &self.granted_at)
            .field("expires_at", &self.expires_at)
            .field("redeem_started_at", &self.redeem_started_at)
            .field("redeemed_at", &self.redeemed_at)
            .field("title", &self.title)
            .field("description", &self.description)
            .finish()
    }
}

#[derive(Deserialize)]
struct RawPersistedResetCredit {
    id: String,
    #[serde(alias = "resetType")]
    reset_type: String,
    status: CodexRateLimitResetCreditStatus,
    #[serde(alias = "grantedAt", with = "ariadusage_protocol::time::rfc3339")]
    granted_at: Timestamp,
    #[serde(
        alias = "expiresAt",
        default,
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    expires_at: Option<Timestamp>,
    #[serde(
        alias = "redeemStartedAt",
        default,
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    redeem_started_at: Option<Timestamp>,
    #[serde(
        alias = "redeemedAt",
        default,
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    redeemed_at: Option<Timestamp>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

impl<'de> Deserialize<'de> for CodexRateLimitResetCredit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawPersistedResetCredit::deserialize(deserializer)?;
        let id = if Self::is_stable_id(&raw.id) {
            raw.id
        } else {
            Self::stable_id_for_provider_id(&raw.id)
        };
        Ok(Self {
            id,
            reset_type: raw.reset_type,
            status: raw.status,
            granted_at: raw.granted_at,
            expires_at: raw.expires_at,
            redeem_started_at: raw.redeem_started_at,
            redeemed_at: raw.redeemed_at,
            title: raw.title,
            description: raw.description,
        })
    }
}

impl CodexRateLimitResetCredit {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl AsRef<str>,
        reset_type: impl Into<String>,
        status: CodexRateLimitResetCreditStatus,
        granted_at: Timestamp,
        expires_at: Option<Timestamp>,
        redeem_started_at: Option<Timestamp>,
        redeemed_at: Option<Timestamp>,
        title: Option<String>,
        description: Option<String>,
    ) -> Self {
        Self {
            id: Self::stable_id_for_provider_id(id.as_ref()),
            reset_type: reset_type.into(),
            status,
            granted_at,
            expires_at,
            redeem_started_at,
            redeemed_at,
            title,
            description,
        }
    }

    pub fn stable_id_for_provider_id(raw_id: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(RESET_CREDIT_ID_HASH_DOMAIN);
        hasher.update(raw_id.as_bytes());
        let digest = hasher.finalize();
        let mut out =
            String::with_capacity(RESET_CREDIT_ID_PREFIX.len() + RESET_CREDIT_HASH_HEX_LEN);
        out.push_str(RESET_CREDIT_ID_PREFIX);
        for byte in digest {
            use std::fmt::Write as _;
            let _ = write!(&mut out, "{byte:02x}");
        }
        out
    }

    fn is_stable_id(value: &str) -> bool {
        let Some(suffix) = value.strip_prefix(RESET_CREDIT_ID_PREFIX) else {
            return false;
        };
        suffix.len() == RESET_CREDIT_HASH_HEX_LEN
            && suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    pub fn is_available(&self, now: Timestamp) -> bool {
        if self.status != CodexRateLimitResetCreditStatus::Available {
            return false;
        }
        if let Some(expires_at) = self.expires_at {
            return expires_at > now;
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexRateLimitResetCreditInventory {
    pub credits: Vec<CodexRateLimitResetCredit>,
    pub next_expires_at: Option<Timestamp>,
}

impl CodexRateLimitResetCreditInventory {
    pub fn count(&self) -> usize {
        self.credits.len()
    }

    pub fn next_expiring_credit(&self) -> Option<&CodexRateLimitResetCredit> {
        self.credits
            .iter()
            .find(|credit| credit.expires_at.is_some())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexRateLimitResetCreditsSnapshot {
    pub credits: Vec<CodexRateLimitResetCredit>,
    pub available_count: usize,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    pub updated_at: Timestamp,
}

impl CodexRateLimitResetCreditsSnapshot {
    pub fn new(
        credits: Vec<CodexRateLimitResetCredit>,
        available_count: usize,
        updated_at: Timestamp,
    ) -> Self {
        Self {
            credits,
            available_count,
            updated_at,
        }
    }

    pub fn available_inventory(&self, now: Timestamp) -> CodexRateLimitResetCreditInventory {
        let mut available: Vec<CodexRateLimitResetCredit> = self
            .credits
            .iter()
            .filter(|c| c.is_available(now))
            .cloned()
            .collect();
        available.sort_by(|lhs, rhs| match (lhs.expires_at, rhs.expires_at) {
            (Some(left_exp), Some(right_exp)) if left_exp != right_exp => left_exp.cmp(&right_exp),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            _ => lhs.id.cmp(&rhs.id),
        });
        let next_expires_at = available.iter().find_map(|credit| credit.expires_at);
        CodexRateLimitResetCreditInventory {
            credits: available,
            next_expires_at,
        }
    }

    pub fn next_expiring_available_credit(&self) -> Option<CodexRateLimitResetCredit> {
        self.available_inventory(self.updated_at)
            .next_expiring_credit()
            .cloned()
    }

    pub fn to_protocol_reset_credits(&self, now: Timestamp) -> Option<ResetCredits> {
        let inventory = self.available_inventory(now);
        if inventory.count() == 0 {
            return None;
        }
        let available_count = u32::try_from(inventory.count()).unwrap_or(u32::MAX);
        Some(ResetCredits {
            available_count,
            next_expires_at: inventory.next_expires_at,
        })
    }
}

pub struct CodexExtraUsageCost;

impl CodexExtraUsageCost {
    pub const CURRENCY_CODE: &'static str = ProviderCostSnapshot::CREDITS_CURRENCY_CODE;
    pub const MONTHLY_CREDIT_LIMIT_PERIOD: &'static str = "Monthly credit limit";
    pub const PURCHASED_CREDITS_PERIOD: &'static str = "Extra usage";

    pub fn provider_cost(credits: Option<&CreditsSnapshot>) -> Option<ProviderCostSnapshot> {
        let credits = credits?;
        let extra_balance = Self::purchased_extra_credits_balance(credits);
        let balance_is_unavailable =
            !credits.balance_read_succeeded && credits.credits_available == Some(true);
        let balance_updated_at = (credits.balance_read_succeeded || balance_is_unavailable)
            .then_some(credits.updated_at);
        let limit = credits
            .codex_credit_limit
            .as_ref()
            .filter(|limit| limit.limit > 0.0);
        if limit.is_none() && balance_updated_at.is_none() {
            return None;
        }
        Some(ProviderCostSnapshot {
            used: limit.map_or(0.0, |l| l.used),
            limit: limit.map_or(0.0, |l| l.limit),
            currency_code: Self::CURRENCY_CODE.to_owned(),
            period: Some(
                limit
                    .map(|l| l.title.clone())
                    .unwrap_or_else(|| Self::PURCHASED_CREDITS_PERIOD.to_owned()),
            ),
            resets_at: limit.and_then(|l| l.resets_at),
            next_regen_amount: None,
            personal_used: None,
            balance: extra_balance,
            balance_updated_at,
            balance_is_workspace: credits.has_workspace_balance().then_some(true),
            updated_at: limit.map_or(credits.updated_at, |l| l.updated_at),
            balance_is_unavailable: balance_is_unavailable.then_some(true),
        })
    }

    pub fn attaching(to: UsageSnapshot, credits: Option<&CreditsSnapshot>) -> UsageSnapshot {
        let cost = Self::resolving(credits, to.provider_cost.as_ref());
        if cost == to.provider_cost {
            return to;
        }
        to.with_provider_cost(cost)
    }

    pub fn attaching_cost(cost: Option<ProviderCostSnapshot>, to: UsageSnapshot) -> UsageSnapshot {
        let Some(cost) = cost else {
            return to;
        };
        to.with_provider_cost(Some(cost))
    }

    pub fn resolving(
        live_credits: Option<&CreditsSnapshot>,
        attached: Option<&ProviderCostSnapshot>,
    ) -> Option<ProviderCostSnapshot> {
        Self::resolving_costs(Self::provider_cost(live_credits).as_ref(), attached)
    }

    pub fn resolving_costs(
        live_cost: Option<&ProviderCostSnapshot>,
        attached: Option<&ProviderCostSnapshot>,
    ) -> Option<ProviderCostSnapshot> {
        let Some(live) = live_cost else {
            return attached.cloned();
        };
        if live.currency_code != Self::CURRENCY_CODE {
            return Some(live.clone());
        }
        let Some(attached) = attached.filter(|c| c.currency_code == Self::CURRENCY_CODE) else {
            return Some(live.clone());
        };
        let live_balance_date = Self::balance_date(live);
        let attached_balance_date = Self::balance_date(attached);
        let balance_source = if let Some(live_bd) = live_balance_date
            && (attached_balance_date.is_none_or(|att_bd| live_bd > att_bd)
                || (Some(live_bd) == attached_balance_date
                    && live.balance_is_unavailable != Some(true)))
        {
            live
        } else {
            attached
        };
        let cap_source = if attached.limit > 0.0
            && (live.limit <= 0.0 || attached.updated_at > live.updated_at)
        {
            attached
        } else {
            live
        };
        let mut out = cap_source.clone();
        out.balance = balance_source.balance;
        out.balance_updated_at = Self::balance_date(balance_source);
        out.balance_is_workspace = balance_source.balance_is_workspace;
        out.balance_is_unavailable = balance_source.balance_is_unavailable;
        Some(out)
    }

    pub fn credits_for_display(
        credits: Option<&CreditsSnapshot>,
        attached: Option<&ProviderCostSnapshot>,
    ) -> Option<CreditsSnapshot> {
        let credits = credits?;
        let Some(cost) = Self::resolving(Some(credits), attached)
            .filter(|c| c.currency_code == Self::CURRENCY_CODE)
        else {
            return Some(credits.clone());
        };
        let balance_date = Self::balance_date(&cost);
        let replace_balance = balance_date
            .is_some_and(|bd| !credits.balance_read_succeeded || bd > credits.updated_at);
        let mut limit = credits.codex_credit_limit.clone();
        if cost.limit > 0.0
            && limit
                .as_ref()
                .is_none_or(|existing| cost.updated_at > existing.updated_at)
        {
            limit = Some(CodexCreditLimitSnapshot::new(
                cost.period
                    .as_deref()
                    .unwrap_or(Self::MONTHLY_CREDIT_LIMIT_PERIOD),
                cost.used,
                cost.limit,
                (100.0 - cost.used / cost.limit * 100.0).max(0.0),
                cost.resets_at,
                cost.updated_at,
            ));
        }
        if !replace_balance && limit == credits.codex_credit_limit {
            return Some(credits.clone());
        }
        Some(CreditsSnapshot {
            remaining: if replace_balance {
                cost.balance.unwrap_or(0.0)
            } else {
                credits.remaining
            },
            events: credits.events.clone(),
            updated_at: if replace_balance {
                balance_date.unwrap_or(credits.updated_at)
            } else {
                credits.updated_at
            },
            codex_credit_limit: limit,
            balance_is_workspace: if replace_balance {
                cost.balance_is_workspace == Some(true)
            } else {
                credits.balance_is_workspace
            },
            balance_read_succeeded: if replace_balance {
                cost.balance_is_unavailable != Some(true)
            } else {
                credits.balance_read_succeeded
            },
            credits_available: if replace_balance && cost.balance_is_unavailable == Some(true) {
                Some(true)
            } else {
                credits.credits_available
            },
        })
    }

    pub fn purchased_extra_credits_balance(credits: &CreditsSnapshot) -> Option<f64> {
        if !credits.balance_read_succeeded {
            return None;
        }
        if credits.has_workspace_balance() {
            return (credits.remaining > 0.0).then_some(credits.remaining);
        }
        if let Some(monthly) = &credits.codex_credit_limit {
            return ((credits.remaining - monthly.remaining()).abs() > 0.000_1
                && credits.remaining > 0.0)
                .then_some(credits.remaining);
        }
        (credits.remaining > 0.0).then_some(credits.remaining)
    }

    fn balance_date(cost: &ProviderCostSnapshot) -> Option<Timestamp> {
        cost.balance_updated_at
            .or_else(|| cost.balance.map(|_| cost.updated_at))
    }
}
