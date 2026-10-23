// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudePlan.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use serde::{Deserialize, Serialize};

/// Canonical Claude subscription or account plan classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaudePlan {
    Max,
    Pro,
    Team,
    Enterprise,
    Ultra,
}

impl ClaudePlan {
    /// Branded login method label (e.g. `"Claude Max"`, `"Claude Pro"`).
    pub fn branded_login_method(self) -> &'static str {
        match self {
            Self::Max => "Claude Max",
            Self::Pro => "Claude Pro",
            Self::Team => "Claude Team",
            Self::Enterprise => "Claude Enterprise",
            Self::Ultra => "Claude Ultra",
        }
    }

    /// Compact login method label (e.g. `"Max"`, `"Pro"`).
    pub fn compact_login_method(self) -> &'static str {
        match self {
            Self::Max => "Max",
            Self::Pro => "Pro",
            Self::Team => "Team",
            Self::Enterprise => "Enterprise",
            Self::Ultra => "Ultra",
        }
    }

    /// Whether this plan counts as a subscription (`false` for `Enterprise`).
    pub fn counts_as_subscription(self) -> bool {
        match self {
            Self::Max | Self::Pro | Self::Team | Self::Ultra => true,
            Self::Enterprise => false,
        }
    }

    /// Resolves a `ClaudePlan` from an OAuth `rateLimitTier` token.
    pub fn from_oauth_rate_limit_tier(raw: Option<&str>) -> Option<Self> {
        Self::parse_token(raw, false)
    }

    /// Resolves a `ClaudePlan` from a web `rateLimitTier` and optional `billingType`.
    pub fn from_web_rate_limit_tier(raw: Option<&str>, billing_type: Option<&str>) -> Option<Self> {
        let allow_stripe_claude_pro_fallback =
            Self::normalized_text(billing_type).is_some_and(|bt| bt.contains("stripe"));
        Self::parse_token(raw, allow_stripe_claude_pro_fallback)
    }

    /// Resolves a `ClaudePlan` from CLI `/status` or account config login method text.
    pub fn from_cli_login_method(raw: Option<&str>) -> Option<Self> {
        Self::parse_words(raw, true)
    }

    /// Resolves a `ClaudePlan` from compatibility login method or OAuth `subscriptionType`.
    pub fn from_compatibility_login_method(raw: Option<&str>) -> Option<Self> {
        Self::parse_words(raw, true)
    }

    /// Resolves a `ClaudePlan` from OAuth `subscriptionType` (preferred) or `rateLimitTier` (fallback).
    pub fn from_oauth_credentials(
        subscription_type: Option<&str>,
        rate_limit_tier: Option<&str>,
    ) -> Option<Self> {
        Self::from_compatibility_login_method(subscription_type)
            .or_else(|| Self::from_oauth_rate_limit_tier(rate_limit_tier))
    }

    /// Formats the branded OAuth login method, including any `Max Nx` multiplier from `rate_limit_tier`.
    pub fn oauth_login_method(
        subscription_type: Option<&str>,
        rate_limit_tier: Option<&str>,
    ) -> Option<String> {
        let plan = Self::from_oauth_credentials(subscription_type, rate_limit_tier)?;
        if plan == Self::Max
            && let Some(multiplier) = Self::max_multiplier(rate_limit_tier)
        {
            return Some(format!("{} {multiplier}x", plan.branded_login_method()));
        }
        Some(plan.branded_login_method().to_owned())
    }

    /// Formats the branded web login method, incorporating `seat_tier` for Team plans and `Max Nx` multipliers.
    pub fn web_login_method(
        rate_limit_tier: Option<&str>,
        billing_type: Option<&str>,
        seat_tier: Option<&str>,
    ) -> Option<String> {
        let plan = Self::from_web_rate_limit_tier(rate_limit_tier, billing_type);
        if let Some(seat_tier_plan) = Self::team_seat_tier_login_method(seat_tier)
            && (plan.is_none() || plan == Some(Self::Team))
        {
            return Some(seat_tier_plan.to_owned());
        }
        let plan = plan?;
        if plan == Self::Max
            && let Some(multiplier) = Self::max_multiplier(rate_limit_tier)
        {
            return Some(format!("{} {multiplier}x", plan.branded_login_method()));
        }
        Some(plan.branded_login_method().to_owned())
    }

    /// Formats the compact CLI compatibility login method, falling back to the trimmed raw string when unknown.
    pub fn cli_compatibility_login_method(raw: Option<&str>) -> Option<String> {
        let trimmed = raw?.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(plan) = Self::from_compatibility_login_method(Some(trimmed)) {
            return Some(plan.compact_login_method().to_owned());
        }
        Some(trimmed.to_owned())
    }

    /// Returns true if `raw` describes a non-Enterprise Claude subscription login method.
    pub fn is_subscription_login_method(raw: Option<&str>) -> bool {
        Self::from_compatibility_login_method(raw)
            .map(Self::counts_as_subscription)
            .unwrap_or(false)
    }

    /// Extracts the integer multiplier `N` from a Max rate limit tier such as `default_claude_max_5x`.
    pub fn max_multiplier(rate_limit_tier: Option<&str>) -> Option<u32> {
        let words = Self::normalized_words(rate_limit_tier);
        let max_index = words.iter().position(|w| w == "max")?;
        let candidate = words.get(max_index + 1)?;
        let digits = candidate.strip_suffix('x')?;
        if digits.is_empty() {
            return None;
        }
        digits.parse::<u32>().ok()
    }

    fn team_seat_tier_login_method(raw: Option<&str>) -> Option<&'static str> {
        match Self::normalized_text(raw)?.as_str() {
            "team_standard" => Some("Claude Team Standard"),
            "team_tier_1" => Some("Claude Team Premium"),
            _ => None,
        }
    }

    fn parse_token(raw: Option<&str>, allow_stripe_claude_pro_fallback: bool) -> Option<Self> {
        let text = Self::normalized_text(raw)?;
        let words = Self::words_from_normalized(&text);

        if words.contains(&"max") || text.contains("max") {
            return Some(Self::Max);
        }
        if words.contains(&"pro") || text.contains("pro") {
            return Some(Self::Pro);
        }
        if text.contains("team") {
            return Some(Self::Team);
        }
        if text.contains("enterprise") {
            return Some(Self::Enterprise);
        }
        if allow_stripe_claude_pro_fallback && text.contains("claude") {
            return Some(Self::Pro);
        }
        None
    }

    fn parse_words(raw: Option<&str>, include_ultra: bool) -> Option<Self> {
        let words = Self::normalized_words(raw);
        if words.is_empty() {
            return None;
        }
        if words.iter().any(|w| w == "max") {
            return Some(Self::Max);
        }
        if words.iter().any(|w| w == "pro") {
            return Some(Self::Pro);
        }
        if words.iter().any(|w| w == "team") {
            return Some(Self::Team);
        }
        if words.iter().any(|w| w == "enterprise") {
            return Some(Self::Enterprise);
        }
        if include_ultra && words.iter().any(|w| w == "ultra") {
            return Some(Self::Ultra);
        }
        None
    }

    fn normalized_text(raw: Option<&str>) -> Option<String> {
        let trimmed = raw?.trim().to_lowercase();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }

    fn normalized_words(raw: Option<&str>) -> Vec<String> {
        let Some(text) = Self::normalized_text(raw) else {
            return Vec::new();
        };
        Self::words_from_normalized(&text)
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    fn words_from_normalized(text: &str) -> Vec<&str> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .collect()
    }
}
