// Ported from CodexBar Sources/CodexBarCLI/CLIRenderer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};

use ariadusage_core::UsagePace;
use ariadusage_protocol::{
    CliCostUsageTokenSnapshot, CliCreditsSnapshot, CliNamedRateWindow, CliRateWindow,
    CliUsageSnapshot, CostProvenance, PacePayload, PaceStage, ProviderPacePayload,
    ProviderStatusPayload, StatusIndicator,
};
use jiff::Timestamp;

use crate::format::{
    ResetTimeDisplayStyle, format_cost_amount, format_number, format_reset_countdown,
    format_reset_countdown_no_prefix, format_reset_line, format_token_count, usage_line,
};
use crate::provider_meta::{
    CliProvider, ProviderCostPresentation, ProviderPaceKind, ProviderPaceResolution,
    ProviderPaceSlot, resolved_account_text,
};

#[derive(Debug, Clone)]
pub struct RenderContext {
    pub header: String,
    pub status: Option<ProviderStatusPayload>,
    pub account_override: Option<String>,
    pub fallback_account_email: Option<String>,
    pub use_color: bool,
    pub reset_style: ResetTimeDisplayStyle,
    pub notes: Vec<String>,
    pub weekly_work_days: Option<u32>,
}

impl RenderContext {
    #[must_use]
    pub fn new(
        header: impl Into<String>,
        status: Option<ProviderStatusPayload>,
        use_color: bool,
        reset_style: ResetTimeDisplayStyle,
    ) -> Self {
        Self {
            header: header.into(),
            status,
            account_override: None,
            fallback_account_email: None,
            use_color,
            reset_style,
            notes: Vec::new(),
            weekly_work_days: None,
        }
    }

    #[must_use]
    pub fn with_notes(mut self, notes: Vec<String>) -> Self {
        self.notes = notes;
        self
    }

    #[must_use]
    pub fn with_account_override(mut self, account_override: Option<String>) -> Self {
        self.account_override = account_override;
        self
    }

    #[must_use]
    pub fn with_fallback_account_email(mut self, fallback_account_email: Option<String>) -> Self {
        self.fallback_account_email = fallback_account_email;
        self
    }

    #[must_use]
    pub fn with_weekly_work_days(mut self, weekly_work_days: Option<u32>) -> Self {
        self.weekly_work_days = weekly_work_days;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CliCardMetric {
    pub label: String,
    pub remaining_percent: Option<f64>,
    pub reset_text: Option<String>,
    pub reset_at: Option<Timestamp>,
    pub detail_text: Option<String>,
}

impl CliCardMetric {
    #[must_use]
    pub fn new(
        label: impl Into<String>,
        remaining_percent: Option<f64>,
        reset_text: Option<String>,
        reset_at: Option<Timestamp>,
        detail_text: Option<String>,
    ) -> Self {
        Self {
            label: label.into(),
            remaining_percent,
            reset_text,
            reset_at,
            detail_text,
        }
    }
}

pub struct CLIRenderer;

impl CLIRenderer {
    const ACCENT_COLOR: &'static str = "95";
    const ACCENT_BOLD_COLOR: &'static str = "1;95";
    const SUBTLE_COLOR: &'static str = "90";
    const PACE_MINIMUM_EXPECTED_PERCENT: f64 = 3.0;
    const CODEX_HEADROOM_DELTA_THRESHOLD: f64 = -15.0;
    const CODEX_HEADROOM_MULTIPLIER_THRESHOLD: f64 = 1.5;

    #[must_use]
    pub fn render_text(
        provider: CliProvider,
        snapshot: &CliUsageSnapshot,
        credits: Option<&CliCreditsSnapshot>,
        context: &RenderContext,
        now: Timestamp,
    ) -> String {
        let mut lines = Vec::new();
        lines.push(Self::header_line(&context.header, context.use_color));
        lines.extend(Self::render_card_body_lines_with_identity(
            provider, snapshot, credits, context, now, true,
        ));
        if let Some(status) = context.status.as_ref() {
            lines.push(Self::status_line(status, context.use_color));
        }
        lines.join("\n")
    }

    #[must_use]
    pub fn render_card_body_lines(
        provider: CliProvider,
        snapshot: &CliUsageSnapshot,
        credits: Option<&CliCreditsSnapshot>,
        context: &RenderContext,
        now: Timestamp,
    ) -> Vec<String> {
        Self::render_card_body_lines_with_identity(provider, snapshot, credits, context, now, true)
    }

    #[must_use]
    pub fn render_card_body_lines_with_identity(
        provider: CliProvider,
        snapshot: &CliUsageSnapshot,
        credits: Option<&CliCreditsSnapshot>,
        context: &RenderContext,
        now: Timestamp,
        include_identity: bool,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        let cost_presentation = provider.cost_presentation(snapshot);

        Self::append_rate_window_lines(
            &mut lines,
            provider,
            snapshot,
            &cost_presentation,
            context,
            now,
        );
        Self::append_provider_details(&mut lines, snapshot, &cost_presentation, context.use_color);
        Self::append_presentation_cost_lines(&mut lines, &cost_presentation, context.use_color);
        if let Some(history_line) =
            Self::live_history_line(snapshot.cost_usage.as_ref(), context.use_color)
        {
            lines.push(history_line);
        }
        Self::append_limits_unavailable_line(&mut lines, provider, snapshot, context.use_color);
        Self::append_credits_line(&mut lines, provider, credits, context.use_color);
        Self::append_codex_reset_credits_line(
            &mut lines,
            provider,
            snapshot,
            context.reset_style,
            now,
            context.use_color,
        );

        if include_identity {
            let identity = provider.identity_presentation(snapshot);
            if let Some(email) = Self::resolved_account_email(
                provider,
                snapshot,
                context.account_override.as_deref(),
                context.fallback_account_email.as_deref(),
            ) {
                lines.push(Self::label_value_line("Account", &email, context.use_color));
            }
            if let Some(plan_val) = identity
                .plan_text
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                lines.push(Self::label_value_line("Plan", plan_val, context.use_color));
            }
            if let Some(activity) = identity
                .activity_text
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                lines.push(Self::label_value_line(
                    "Activity",
                    activity,
                    context.use_color,
                ));
            }
            for note in &context.notes {
                let trimmed = note.trim();
                if !trimmed.is_empty() {
                    lines.push(Self::label_value_line("Note", trimmed, context.use_color));
                }
            }
        } else {
            for note in &context.notes {
                let trimmed = note.trim();
                if !trimmed.is_empty() {
                    lines.push(Self::label_value_line("Note", trimmed, context.use_color));
                }
            }
        }

        lines
    }

    #[must_use]
    pub fn collect_card_metrics(
        provider: CliProvider,
        snapshot: &CliUsageSnapshot,
        reset_style: ResetTimeDisplayStyle,
        now: Timestamp,
        _weekly_work_days: Option<u32>,
    ) -> Vec<CliCardMetric> {
        let mut metrics = Vec::new();
        let detail_backed = provider.uses_detail_backed_window();
        let summary_lanes = Self::antigravity_quota_summary_lanes(provider, snapshot);

        if !summary_lanes.is_empty() {
            for lane in summary_lanes {
                metrics.push(Self::make_card_metric(
                    provider,
                    &lane,
                    reset_style,
                    now,
                    detail_backed,
                ));
            }
            return metrics;
        }

        let labels = provider.rate_window_labels(snapshot, now);

        if let Some(primary) = snapshot.primary.as_ref().and_then(CliRateWindow::measured) {
            let named = CliNamedRateWindow::new(&labels.primary, &labels.primary, primary.clone());
            metrics.push(Self::make_card_metric(
                provider,
                &named,
                reset_style,
                now,
                detail_backed,
            ));
        }

        if let Some(secondary) = snapshot
            .secondary
            .as_ref()
            .and_then(CliRateWindow::measured)
        {
            let named =
                CliNamedRateWindow::new(&labels.secondary, &labels.secondary, secondary.clone());
            metrics.push(Self::make_card_metric(
                provider,
                &named,
                reset_style,
                now,
                detail_backed,
            ));
        }

        if labels.shows_tertiary
            && let Some(tertiary) = snapshot.tertiary.as_ref().and_then(CliRateWindow::measured)
        {
            let named =
                CliNamedRateWindow::new(&labels.tertiary, &labels.tertiary, tertiary.clone());
            metrics.push(Self::make_card_metric(
                provider,
                &named,
                reset_style,
                now,
                detail_backed,
            ));
        }

        for extra in provider.extra_rate_windows(snapshot) {
            metrics.push(Self::make_card_metric(
                provider,
                &extra,
                reset_style,
                now,
                detail_backed,
            ));
        }

        metrics
    }

    fn make_card_metric(
        provider: CliProvider,
        window: &CliNamedRateWindow,
        reset_style: ResetTimeDisplayStyle,
        now: Timestamp,
        detail_backed: bool,
    ) -> CliCardMetric {
        let rate_window = &window.window;
        let reset = Self::reset_text(rate_window, reset_style, now, detail_backed)
            .map(|r| format!("⏳ {r}"));
        let reset_at = rate_window
            .resets_at
            .as_deref()
            .and_then(|s| s.parse::<Timestamp>().ok());
        let detail = Self::window_detail_text(provider, rate_window);
        CliCardMetric::new(
            window.title.clone(),
            window
                .usage_known
                .then(|| rate_window.remaining_percent().clamp(0.0, 100.0)),
            reset,
            reset_at,
            detail,
        )
    }

    #[must_use]
    pub fn collect_card_info_lines(
        provider: CliProvider,
        snapshot: &CliUsageSnapshot,
        credits: Option<&CliCreditsSnapshot>,
        context: &RenderContext,
        now: Timestamp,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        if provider == CliProvider::Codex
            && let Some(reset_credits) = snapshot.codex_reset_credits.as_ref()
        {
            let available = reset_credits.available_credits(now);
            let value = format!("{} available", available.len());
            lines.push(Self::label_value_line(
                "Limit Reset Credits",
                &value,
                context.use_color,
            ));
        }
        Self::append_credits_line(&mut lines, provider, credits, context.use_color);
        for note in &context.notes {
            let trimmed = note.trim();
            if !trimmed.is_empty() {
                lines.push(Self::label_value_line("Note", trimmed, context.use_color));
            }
        }
        lines
    }

    #[must_use]
    pub fn collect_card_extra_lines(
        provider: CliProvider,
        snapshot: &CliUsageSnapshot,
        context: &RenderContext,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        let cost_presentation = provider.cost_presentation(snapshot);
        if snapshot.primary.is_none()
            && cost_presentation.shows_generic_fallback
            && let Some(cost) = snapshot.provider_cost.as_ref()
        {
            let cost_label = if cost.currency_code == "Quota" {
                "Quota"
            } else {
                "Cost"
            };
            lines.push(Self::label_value_line(
                cost_label,
                &format!("{:.1} / {:.1}", cost.used, cost.limit),
                context.use_color,
            ));
        }
        Self::append_provider_details(&mut lines, snapshot, &cost_presentation, context.use_color);
        Self::append_presentation_cost_lines(&mut lines, &cost_presentation, context.use_color);
        Self::append_limits_unavailable_line(&mut lines, provider, snapshot, context.use_color);
        let identity = provider.identity_presentation(snapshot);
        if let Some(activity) = identity
            .activity_text
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            lines.push(Self::label_value_line(
                "Activity",
                activity,
                context.use_color,
            ));
        }
        lines
    }

    #[must_use]
    pub fn live_history_line(
        cost_usage: Option<&CliCostUsageTokenSnapshot>,
        use_color: bool,
    ) -> Option<String> {
        let history = cost_usage?;
        let amount = history.last_30_days_cost_usd.filter(|a| a.is_finite());
        let token_count = history.last_30_days_tokens.filter(|&t| t >= 0);
        if amount.is_none() && token_count.is_none() {
            return None;
        }
        let mut parts = Vec::new();
        if let Some(amount) = amount {
            let cost = format_cost_amount(amount, &history.currency_code);
            let provenance = match history.cost_provenance {
                CostProvenance::VendorMetered => Some("(reported)"),
                CostProvenance::ListPriceEstimate => Some("(estimated)"),
                CostProvenance::Mixed => Some("(includes estimates)"),
                CostProvenance::Unknown => None,
            };
            if let Some(provenance) = provenance {
                parts.push(format!("{cost} {provenance}"));
            } else {
                parts.push(cost);
            }
        }
        if let Some(token_count) = token_count {
            let noun = if token_count == 1 { "token" } else { "tokens" };
            parts.push(format!("{} {noun}", format_token_count(token_count)));
        }
        let label = match history
            .history_label
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(custom) => custom.to_string(),
            None if history.history_days == 1 => "Last 1 day".to_string(),
            None => format!("Last {} days", history.history_days),
        };
        Some(Self::label_value_line(
            &label,
            &parts.join(" · "),
            use_color,
        ))
    }

    #[must_use]
    pub fn plan_badge_text(provider: CliProvider, snapshot: &CliUsageSnapshot) -> Option<String> {
        provider.identity_presentation(snapshot).badge_text
    }

    #[must_use]
    pub fn resolved_account_email(
        provider: CliProvider,
        snapshot: &CliUsageSnapshot,
        account_override: Option<&str>,
        fallback_account_email: Option<&str>,
    ) -> Option<String> {
        if let Some(ov) = account_override.map(str::trim).filter(|s| !s.is_empty()) {
            return Some(ov.to_string());
        }
        resolved_account_text(provider, snapshot, fallback_account_email)
    }

    fn append_rate_window_lines(
        lines: &mut Vec<String>,
        provider: CliProvider,
        snapshot: &CliUsageSnapshot,
        cost_presentation: &ProviderCostPresentation,
        context: &RenderContext,
        now: Timestamp,
    ) {
        let summary_lanes = Self::antigravity_quota_summary_lanes(provider, snapshot);
        if !summary_lanes.is_empty() {
            for lane in summary_lanes {
                lines.push(Self::rate_line(
                    &lane.title,
                    &lane.window,
                    lane.usage_known,
                    context.use_color,
                ));
                if let Some(reset) = Self::reset_text(&lane.window, context.reset_style, now, false)
                {
                    lines.push(Self::subtle_line(&reset, context.use_color));
                }
            }
            return;
        }

        let labels = provider.rate_window_labels(snapshot, now);
        let detail_backed = provider.uses_detail_backed_window();

        if let Some(primary) = snapshot.primary.as_ref().and_then(CliRateWindow::measured) {
            lines.push(Self::rate_line(
                &labels.primary,
                primary,
                true,
                context.use_color,
            ));
            if let Some(pace) = Self::pace_line(
                provider,
                snapshot,
                ProviderPaceSlot::Primary,
                primary,
                now,
                context.weekly_work_days,
                context.use_color,
            ) {
                lines.push(pace);
            }
            if let Some(reset) = Self::reset_text(primary, context.reset_style, now, detail_backed)
            {
                lines.push(Self::subtle_line(&reset, context.use_color));
            }
            if let Some(detail) = Self::window_detail_text(provider, primary) {
                lines.push(Self::subtle_line(&detail, context.use_color));
            }
        } else if cost_presentation.shows_generic_fallback
            && let Some(cost) = snapshot.provider_cost.as_ref()
        {
            let cost_label = if cost.currency_code == "Quota" {
                "Quota"
            } else {
                "Cost"
            };
            lines.push(Self::label_value_line(
                cost_label,
                &format!("{:.1} / {:.1}", cost.used, cost.limit),
                context.use_color,
            ));
        }

        if let Some(secondary) = snapshot.secondary.as_ref() {
            lines.push(Self::rate_line(
                &labels.secondary,
                secondary,
                true,
                context.use_color,
            ));
            if let Some(pace) = Self::pace_line(
                provider,
                snapshot,
                ProviderPaceSlot::Secondary,
                secondary,
                now,
                context.weekly_work_days,
                context.use_color,
            ) {
                lines.push(pace);
            }
            if let Some(reset) =
                Self::reset_text(secondary, context.reset_style, now, detail_backed)
            {
                lines.push(Self::subtle_line(&reset, context.use_color));
            }
            if let Some(detail) = Self::window_detail_text(provider, secondary) {
                lines.push(Self::subtle_line(&detail, context.use_color));
            }
        }

        if labels.shows_tertiary
            && let Some(tertiary) = snapshot.tertiary.as_ref()
        {
            lines.push(Self::rate_line(
                &labels.tertiary,
                tertiary,
                true,
                context.use_color,
            ));
            if let Some(pace) = Self::pace_line(
                provider,
                snapshot,
                ProviderPaceSlot::Tertiary,
                tertiary,
                now,
                context.weekly_work_days,
                context.use_color,
            ) {
                lines.push(pace);
            }
            if let Some(reset) = Self::reset_text(tertiary, context.reset_style, now, detail_backed)
            {
                lines.push(Self::subtle_line(&reset, context.use_color));
            }
            if let Some(detail) = Self::window_detail_text(provider, tertiary) {
                lines.push(Self::subtle_line(&detail, context.use_color));
            }
        }

        for extra in provider.extra_rate_windows(snapshot) {
            lines.push(Self::rate_line(
                &extra.title,
                &extra.window,
                extra.usage_known,
                context.use_color,
            ));
            if let Some(reset) =
                Self::reset_text(&extra.window, context.reset_style, now, detail_backed)
            {
                lines.push(Self::subtle_line(&reset, context.use_color));
            }
            if detail_backed && let Some(detail) = Self::window_detail_text(provider, &extra.window)
            {
                lines.push(Self::subtle_line(&detail, context.use_color));
            }
        }
    }

    fn append_provider_details(
        lines: &mut Vec<String>,
        snapshot: &CliUsageSnapshot,
        _cost_presentation: &ProviderCostPresentation,
        use_color: bool,
    ) {
        for section in &snapshot.details {
            for row in &section.rows {
                let value = if let Some(secondary) =
                    row.secondary_value.as_deref().filter(|s| !s.is_empty())
                {
                    format!("{} · {secondary}", row.value)
                } else {
                    row.value.clone()
                };
                lines.push(Self::label_value_line(&row.label, &value, use_color));
            }
            if let Some(chart) = section.chart.as_ref() {
                let unit = chart
                    .unit
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(|u| format!(" {u}"))
                    .unwrap_or_default();
                for point in &chart.points {
                    let label = chart
                        .title
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .map_or_else(|| point.label.clone(), |t| format!("{t} · {}", point.label));
                    let value = format!("{}{unit}", format_number(point.value));
                    lines.push(Self::label_value_line(&label, &value, use_color));
                }
            }
        }
    }

    fn append_presentation_cost_lines(
        lines: &mut Vec<String>,
        cost_presentation: &ProviderCostPresentation,
        use_color: bool,
    ) {
        for line in &cost_presentation.cli_lines {
            lines.push(Self::label_value_line(&line.label, &line.value, use_color));
        }
    }

    fn append_limits_unavailable_line(
        lines: &mut Vec<String>,
        provider: CliProvider,
        snapshot: &CliUsageSnapshot,
        use_color: bool,
    ) {
        if snapshot.rate_limits_unavailable(provider.raw_value()) {
            lines.push(Self::label_value_line("Limits", "not available", use_color));
        }
    }

    fn append_credits_line(
        lines: &mut Vec<String>,
        provider: CliProvider,
        credits: Option<&CliCreditsSnapshot>,
        use_color: bool,
    ) {
        if provider.supports_credits()
            && let Some(credits) = credits
            && let Some(remaining) = credits.display_remaining()
        {
            let value = format!("{} left", format_number(remaining));
            lines.push(Self::label_value_line("Credits", &value, use_color));
        }
    }

    fn append_codex_reset_credits_line(
        lines: &mut Vec<String>,
        provider: CliProvider,
        snapshot: &CliUsageSnapshot,
        _reset_style: ResetTimeDisplayStyle,
        now: Timestamp,
        use_color: bool,
    ) {
        if provider != CliProvider::Codex {
            return;
        }
        let Some(reset_credits) = snapshot.codex_reset_credits.as_ref() else {
            return;
        };
        let available = reset_credits.available_credits(now);
        let value = format!("{} available", available.len());
        lines.push(Self::label_value_line(
            "Limit Reset Credits",
            &value,
            use_color,
        ));
        if let Some(next_expiry) = reset_credits
            .next_expiring_credit(now)
            .and_then(|c| c.expires_at.as_deref())
            .and_then(|s| s.parse::<Timestamp>().ok())
        {
            let expiry = format_reset_countdown(next_expiry, now);
            lines.push(Self::subtle_line(
                &format!("Next reset credit expires {expiry}"),
                use_color,
            ));
        }
    }

    #[must_use]
    pub fn antigravity_quota_summary_lanes(
        provider: CliProvider,
        snapshot: &CliUsageSnapshot,
    ) -> Vec<CliNamedRateWindow> {
        if provider != CliProvider::Antigravity {
            return Vec::new();
        }
        let Some(extras) = snapshot.extra_rate_windows.as_ref() else {
            return Vec::new();
        };
        let summary_lanes: Vec<&CliNamedRateWindow> = extras
            .iter()
            .filter(|w| w.id.starts_with("antigravity-quota-summary-"))
            .collect();
        if summary_lanes.is_empty() {
            return Vec::new();
        }
        let idle_ids = antigravity_idle_window_ids(extras);
        summary_lanes
            .into_iter()
            .filter(|w| !idle_ids.contains(&w.id))
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn rate_line(
        title: &str,
        window: &CliRateWindow,
        usage_known: bool,
        use_color: bool,
    ) -> String {
        if !usage_known {
            return Self::label_value_line(title, "Unavailable", use_color);
        }
        let remaining = window.remaining_percent().clamp(0.0, 100.0);
        let text = usage_line(remaining, window.used_percent, false);
        let colored = Self::colorize(&text, remaining, use_color);
        let bar = Self::usage_bar(remaining, use_color);
        format!("{title}: {colored} {bar}")
    }

    fn usage_bar(percent_left: f64, use_color: bool) -> String {
        let clamped = percent_left.clamp(0.0, 100.0);
        let width = 12usize;
        let filled = (((clamped / 100.0) * (width as f64)) as usize).min(width);
        let empty = width.saturating_sub(filled);
        let bar = format!("[{}{}]", "=".repeat(filled), "-".repeat(empty));
        if !use_color {
            return bar;
        }
        Self::ansi(Self::ACCENT_COLOR, &bar)
    }

    fn window_detail_text(provider: CliProvider, window: &CliRateWindow) -> Option<String> {
        if !provider.uses_detail_backed_window() {
            return None;
        }
        let trimmed = window.reset_description.as_deref()?.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    fn reset_text(
        window: &CliRateWindow,
        style: ResetTimeDisplayStyle,
        now: Timestamp,
        uses_detail_backed_window: bool,
    ) -> Option<String> {
        let reset_desc = if uses_detail_backed_window {
            None
        } else {
            window.reset_description.as_deref()
        };
        let reset_at = window
            .resets_at
            .as_deref()
            .and_then(|s| s.parse::<Timestamp>().ok());
        format_reset_line(reset_at, reset_desc, style, now)
    }

    #[must_use]
    pub fn status_line(status: &ProviderStatusPayload, use_color: bool) -> String {
        let line = format!(
            "Status: {}{}",
            status.indicator_label(),
            status.description_suffix()
        );
        if !use_color {
            return line;
        }
        let code = match status.indicator {
            StatusIndicator::None => "32",
            StatusIndicator::Minor => "33",
            StatusIndicator::Major | StatusIndicator::Critical => "31",
            StatusIndicator::Maintenance => "34",
            StatusIndicator::Unknown => "90",
        };
        Self::ansi(code, &line)
    }

    fn header_line(header: &str, use_color: bool) -> String {
        let decorated = format!("== {header} ==");
        if use_color {
            Self::ansi(Self::ACCENT_BOLD_COLOR, &decorated)
        } else {
            decorated
        }
    }

    #[must_use]
    pub fn label_value_line(label: &str, value: &str, use_color: bool) -> String {
        let label_text = Self::label(label, use_color);
        format!("{label_text}: {value}")
    }

    #[must_use]
    pub fn label(text: &str, use_color: bool) -> String {
        if use_color {
            Self::ansi(Self::ACCENT_COLOR, text)
        } else {
            text.to_string()
        }
    }

    #[must_use]
    pub fn subtle_line(text: &str, use_color: bool) -> String {
        if use_color {
            Self::ansi(Self::SUBTLE_COLOR, text)
        } else {
            text.to_string()
        }
    }

    fn colorize(text: &str, percent_left: f64, use_color: bool) -> String {
        if !use_color {
            return text.to_string();
        }
        let code = if percent_left < 10.0 {
            "31"
        } else if percent_left < 25.0 {
            "33"
        } else {
            "32"
        };
        Self::ansi(code, text)
    }

    #[must_use]
    pub fn ansi(code: &str, text: &str) -> String {
        format!("\x1b[{code}m{text}\x1b[0m")
    }

    #[must_use]
    pub fn colorize_accent_bold(text: &str) -> String {
        Self::ansi(Self::ACCENT_BOLD_COLOR, text)
    }

    #[must_use]
    pub fn colorize_accent(text: &str) -> String {
        Self::ansi(Self::ACCENT_COLOR, text)
    }

    #[must_use]
    pub fn colorize_subtle(text: &str) -> String {
        Self::ansi(Self::SUBTLE_COLOR, text)
    }

    #[must_use]
    pub fn colorize_card_border(text: &str) -> String {
        Self::ansi("90", text)
    }

    #[must_use]
    pub fn colorize_card_badge(source: &str) -> String {
        Self::ansi("97;44", &format!(" {source} "))
    }

    #[must_use]
    pub fn colorize_card_plan_box(text: &str) -> String {
        Self::ansi("37", text)
    }

    #[must_use]
    pub fn colorize_card_percent(text: &str, remaining_percent: f64, use_color: bool) -> String {
        if !use_color {
            return text.to_string();
        }
        let code = if remaining_percent < 10.0 {
            "31"
        } else if remaining_percent < 50.0 {
            "33"
        } else {
            "36"
        };
        Self::ansi(code, text)
    }

    #[must_use]
    pub fn colorize_card_used_percent(text: &str, used_percent: f64, use_color: bool) -> String {
        if !use_color {
            return text.to_string();
        }
        let code = if used_percent >= 90.0 {
            "31"
        } else if used_percent >= 60.0 {
            "33"
        } else {
            "36"
        };
        Self::ansi(code, text)
    }

    #[must_use]
    pub fn card_used_bar(used_percent: f64, width: usize, use_color: bool) -> String {
        let clamped = used_percent.clamp(0.0, 100.0);
        let bar_width = width.max(4);
        let raw_filled = ((clamped / 100.0) * (bar_width as f64)) as usize;
        let filled = raw_filled.min(bar_width);
        let empty = bar_width.saturating_sub(filled);
        let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));
        if !use_color {
            return bar;
        }
        Self::colorize_card_used_percent(&bar, clamped, true)
    }

    #[must_use]
    pub fn colorize_warning(text: &str) -> String {
        Self::ansi("33", text)
    }

    #[must_use]
    pub fn colorize_error(text: &str) -> String {
        Self::ansi("31", text)
    }

    #[must_use]
    pub fn ansi_true_color(red: i32, green: i32, blue: i32, text: &str) -> String {
        let r = red.clamp(0, 255);
        let g = green.clamp(0, 255);
        let b = blue.clamp(0, 255);
        format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m")
    }

    #[must_use]
    pub fn ansi_true_color_background(red: i32, green: i32, blue: i32, text: &str) -> String {
        let r = red.clamp(0, 255);
        let g = green.clamp(0, 255);
        let b = blue.clamp(0, 255);
        format!("\x1b[48;2;{r};{g};{b}m{text}\x1b[0m")
    }

    fn remaining_gradient_rgb(remaining_percent: f64) -> ((i32, i32, i32), (i32, i32, i32)) {
        if remaining_percent < 10.0 {
            ((180, 55, 55), (255, 95, 95))
        } else if remaining_percent < 50.0 {
            ((200, 120, 40), (255, 190, 90))
        } else {
            ((40, 150, 140), (90, 220, 200))
        }
    }

    #[must_use]
    pub fn gradient_remaining_track_bar(remaining_percent: f64, width: usize) -> String {
        let clamped = remaining_percent.clamp(0.0, 100.0);
        let bar_width = width.max(4);
        let raw_filled = ((clamped / 100.0) * (bar_width as f64)) as usize;
        let filled = raw_filled.min(bar_width);
        let empty = bar_width.saturating_sub(filled);
        let (dark, light) = Self::remaining_gradient_rgb(clamped);
        let mut bar = String::new();
        if filled > 0 {
            for index in 0..filled {
                let t = if filled == 1 {
                    1.0
                } else {
                    (index as f64) / ((filled - 1) as f64)
                };
                let red = ((dark.0 as f64) * (1.0 - t) + (light.0 as f64) * t) as i32;
                let green = ((dark.1 as f64) * (1.0 - t) + (light.1 as f64) * t) as i32;
                let blue = ((dark.2 as f64) * (1.0 - t) + (light.2 as f64) * t) as i32;
                bar.push_str(&Self::ansi_true_color_background(red, green, blue, " "));
            }
        }
        if empty > 0 {
            let empty_cell = Self::ansi_true_color_background(17, 30, 50, " ");
            bar.push_str(&empty_cell.repeat(empty));
        }
        bar
    }

    #[must_use]
    pub fn gradient_used_bar(used_percent: f64, width: usize) -> String {
        let clamped = used_percent.clamp(0.0, 100.0);
        let bar_width = width.max(4);
        let raw_filled = ((clamped / 100.0) * (bar_width as f64)) as usize;
        let filled = raw_filled.min(bar_width);
        let empty = bar_width.saturating_sub(filled);
        let (dark, light) = Self::remaining_gradient_rgb(100.0 - clamped);
        let mut bar = String::new();
        if filled > 0 {
            for index in 0..filled {
                let t = if filled == 1 {
                    1.0
                } else {
                    (index as f64) / ((filled - 1) as f64)
                };
                let red = ((dark.0 as f64) * (1.0 - t) + (light.0 as f64) * t) as i32;
                let green = ((dark.1 as f64) * (1.0 - t) + (light.1 as f64) * t) as i32;
                let blue = ((dark.2 as f64) * (1.0 - t) + (light.2 as f64) * t) as i32;
                bar.push_str(&Self::ansi_true_color(red, green, blue, "█"));
            }
        }
        if empty > 0 {
            let empty_cell = Self::ansi_true_color(48, 50, 62, "░");
            bar.push_str(&empty_cell.repeat(empty));
        }
        bar
    }

    #[must_use]
    pub fn colorize_enhanced_accent_bold(text: &str) -> String {
        Self::ansi_true_color(198, 146, 255, text)
    }

    #[must_use]
    pub fn colorize_enhanced_subtle(text: &str) -> String {
        Self::ansi_true_color(130, 135, 150, text)
    }

    #[must_use]
    pub fn colorize_enhanced_border(text: &str) -> String {
        Self::ansi_true_color(90, 95, 110, text)
    }

    #[must_use]
    pub fn colorize_enhanced_badge(source: &str) -> String {
        format!("\x1b[38;2;245;248;255;48;2;66;133;244m {source} \x1b[0m")
    }

    #[must_use]
    pub fn colorize_enhanced_plan_label(text: &str) -> String {
        Self::ansi_true_color(104, 111, 135, text)
    }

    #[must_use]
    pub fn colorize_enhanced_plan_value(text: &str) -> String {
        Self::ansi_true_color(238, 184, 92, text)
    }

    #[must_use]
    pub fn colorize_enhanced_remaining_percent(text: &str, remaining_percent: f64) -> String {
        let (_, light) = Self::remaining_gradient_rgb(remaining_percent);
        Self::ansi_true_color(light.0, light.1, light.2, text)
    }

    #[must_use]
    pub fn colorize_enhanced_used_percent(text: &str, used_percent: f64) -> String {
        Self::colorize_enhanced_remaining_percent(text, 100.0 - used_percent)
    }

    #[must_use]
    pub fn colorize_enhanced_readable(text: &str) -> String {
        Self::ansi_true_color(235, 238, 245, text)
    }

    #[must_use]
    pub fn colorize_enhanced_readable_muted(text: &str) -> String {
        Self::ansi_true_color(170, 178, 195, text)
    }

    #[must_use]
    pub fn colorize_enhanced_good(text: &str) -> String {
        Self::ansi_true_color(116, 220, 195, text)
    }

    #[must_use]
    pub fn colorize_readable(text: &str) -> String {
        Self::ansi("97", text)
    }

    #[must_use]
    pub fn colorize_readable_muted(text: &str) -> String {
        Self::ansi("37", text)
    }

    #[must_use]
    pub fn card_block_bar(remaining_percent: f64, width: usize, use_color: bool) -> String {
        let clamped = remaining_percent.clamp(0.0, 100.0);
        let bar_width = width.max(8);
        let raw_filled = ((clamped / 100.0) * (bar_width as f64)) as usize;
        let filled = raw_filled.min(bar_width);
        let empty = bar_width.saturating_sub(filled);
        let filled_bar = "━".repeat(filled);
        let empty_bar = " ".repeat(empty);
        if !use_color {
            return format!("{filled_bar}{empty_bar}");
        }
        format!(
            "{}{}",
            Self::colorize_card_percent(&filled_bar, remaining_percent, true),
            Self::colorize_subtle(&"─".repeat(empty))
        )
    }

    #[must_use]
    pub fn provider_pace_payload(
        provider: CliProvider,
        snapshot: &CliUsageSnapshot,
        now: Timestamp,
        weekly_work_days: Option<u32>,
    ) -> Option<ProviderPacePayload> {
        let labels = provider.rate_window_labels(snapshot, now);
        let primary = snapshot.primary.as_ref().and_then(|w| {
            Self::pace_payload(
                provider,
                snapshot,
                ProviderPaceSlot::Primary,
                w,
                now,
                weekly_work_days,
            )
        });
        let secondary = snapshot.secondary.as_ref().and_then(|w| {
            Self::pace_payload(
                provider,
                snapshot,
                ProviderPaceSlot::Secondary,
                w,
                now,
                weekly_work_days,
            )
        });
        let tertiary = if labels.shows_tertiary {
            snapshot.tertiary.as_ref().and_then(|w| {
                Self::pace_payload(
                    provider,
                    snapshot,
                    ProviderPaceSlot::Tertiary,
                    w,
                    now,
                    weekly_work_days,
                )
            })
        } else {
            None
        };
        if primary.is_none() && secondary.is_none() && tertiary.is_none() {
            return None;
        }
        Some(ProviderPacePayload {
            primary,
            secondary,
            tertiary,
        })
    }

    fn pace_line(
        provider: CliProvider,
        snapshot: &CliUsageSnapshot,
        slot: ProviderPaceSlot,
        window: &CliRateWindow,
        now: Timestamp,
        weekly_work_days: Option<u32>,
        use_color: bool,
    ) -> Option<String> {
        let resolved = Self::resolve_pace(provider, snapshot, slot, window, now, weekly_work_days)?;
        let summary = Self::pace_summary(provider, resolved.kind, &resolved.pace, now);
        Some(Self::label_value_line("Pace", &summary, use_color))
    }

    fn pace_payload(
        provider: CliProvider,
        snapshot: &CliUsageSnapshot,
        slot: ProviderPaceSlot,
        window: &CliRateWindow,
        now: Timestamp,
        weekly_work_days: Option<u32>,
    ) -> Option<PacePayload> {
        let resolved = Self::resolve_pace(provider, snapshot, slot, window, now, weekly_work_days)?;
        let pace = &resolved.pace;
        let summary = Self::pace_summary(provider, resolved.kind, pace, now);
        Some(PacePayload {
            stage: pace.stage,
            delta_percent: pace.delta_percent.round(),
            expected_used_percent: pace.expected_used_percent.round(),
            will_last_to_reset: pace.will_last_to_reset,
            eta_seconds: pace.eta_seconds.map(f64::round),
            run_out_probability: pace.run_out_probability,
            summary,
        })
    }

    fn resolve_pace(
        provider: CliProvider,
        snapshot: &CliUsageSnapshot,
        slot: ProviderPaceSlot,
        window: &CliRateWindow,
        now: Timestamp,
        weekly_work_days: Option<u32>,
    ) -> Option<ProviderPaceResolution> {
        let resolved = provider.resolve_pace(snapshot, slot, window, now, weekly_work_days)?;
        if resolved.pace.expected_used_percent < Self::PACE_MINIMUM_EXPECTED_PERCENT {
            return None;
        }
        Some(resolved)
    }

    fn pace_summary(
        provider: CliProvider,
        kind: ProviderPaceKind,
        pace: &UsagePace,
        now: Timestamp,
    ) -> String {
        let expected = pace.expected_used_percent.round() as i64;
        let mut parts = vec![
            Self::pace_left_label(pace),
            format!("Expected {expected}% used"),
        ];
        if let Some(right) = Self::pace_right_label(provider, kind, pace, now) {
            parts.push(right);
        }
        parts.join(" | ")
    }

    fn pace_left_label(pace: &UsagePace) -> String {
        let delta_value = pace.delta_percent.abs().round() as i64;
        match pace.stage {
            PaceStage::OnTrack | PaceStage::Unknown => "On pace".to_string(),
            PaceStage::SlightlyAhead | PaceStage::Ahead | PaceStage::FarAhead => {
                format!("{delta_value}% in deficit")
            }
            PaceStage::SlightlyBehind | PaceStage::Behind | PaceStage::FarBehind => {
                format!("{delta_value}% in reserve")
            }
        }
    }

    fn pace_right_label(
        provider: CliProvider,
        kind: ProviderPaceKind,
        pace: &UsagePace,
        now: Timestamp,
    ) -> Option<String> {
        if pace.will_last_to_reset {
            if let Some(headroom) = Self::pace_headroom_hint(provider, pace) {
                return Some(format!("Lasts until reset | {headroom}"));
            }
            return Some("Lasts until reset".to_string());
        }
        let eta_seconds = pace.eta_seconds?;
        let eta_text = format_reset_countdown_no_prefix(eta_seconds, now);
        match kind {
            ProviderPaceKind::Session => {
                if eta_text == "now" {
                    Some("Projected empty now".to_string())
                } else {
                    Some(format!("Projected empty in {eta_text}"))
                }
            }
            ProviderPaceKind::Weekly => {
                if eta_text == "now" {
                    Some("Runs out now".to_string())
                } else {
                    Some(format!("Runs out in {eta_text}"))
                }
            }
        }
    }

    fn pace_headroom_hint(provider: CliProvider, pace: &UsagePace) -> Option<&'static str> {
        if !provider.shows_headroom_hint() {
            return None;
        }
        if pace.delta_percent < Self::CODEX_HEADROOM_DELTA_THRESHOLD
            && let Some(multiplier) = pace.speed_multiplier_to_reset
            && multiplier >= Self::CODEX_HEADROOM_MULTIPLIER_THRESHOLD
        {
            return Some("1.5× headroom");
        }
        None
    }
}

fn antigravity_idle_window_ids(extras: &[CliNamedRateWindow]) -> BTreeSet<String> {
    let summary_windows: Vec<&CliNamedRateWindow> = extras
        .iter()
        .filter(|w| w.id.starts_with("antigravity-quota-summary-"))
        .collect();
    if summary_windows.is_empty() {
        return BTreeSet::new();
    }

    let mut families: BTreeMap<String, Vec<&CliNamedRateWindow>> = BTreeMap::new();
    for window in summary_windows {
        families
            .entry(antigravity_family_key(window))
            .or_default()
            .push(window);
    }

    let idle_families: Vec<Vec<&CliNamedRateWindow>> = families
        .values()
        .filter_map(|lanes| {
            if lanes
                .iter()
                .all(|w| w.usage_known && w.window.used_percent <= 0.0)
            {
                Some(lanes.clone())
            } else {
                None
            }
        })
        .collect();

    if idle_families.len() >= families.len() {
        return BTreeSet::new();
    }

    idle_families
        .into_iter()
        .flatten()
        .map(|w| w.id.clone())
        .collect()
}

fn antigravity_family_key(window: &CliNamedRateWindow) -> String {
    let id = window.id.to_ascii_lowercase();
    if id.contains("gemini") {
        return "gemini".to_string();
    }
    if id.contains("3p") || id.contains("third-party") {
        return "claude-gpt".to_string();
    }
    let title = window.title.to_ascii_lowercase();
    if title.contains("gemini") {
        return "gemini".to_string();
    }
    if title.contains("claude") || title.contains("gpt") {
        return "claude-gpt".to_string();
    }
    for suffix in [" 5-hour", " weekly"] {
        if let Some(prefix) = title.strip_suffix(suffix) {
            let stripped = prefix.trim();
            if !stripped.is_empty() {
                return stripped.to_string();
            }
        }
    }
    title
}
