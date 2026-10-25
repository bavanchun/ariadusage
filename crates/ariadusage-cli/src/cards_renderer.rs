// Ported from CodexBar Sources/CodexBarCLI/CLICardsRenderer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_protocol::{CliCreditsSnapshot, CliUsageSnapshot, ProviderStatusPayload};
use jiff::Timestamp;
use unicode_segmentation::UnicodeSegmentation;

use crate::claude_swap_cards::{
    CLIClaudeSwapCardsRenderOptions, CLIClaudeSwapText, ClaudeSwapAccountProjection,
    ProviderAccountUsageSnapshot,
};
use crate::format::{
    ResetTimeDisplayStyle, format_updated_string, strip_ansi_codes, usage_line, visible_width,
};
use crate::provider_meta::CliProvider;
use crate::renderer::{CLIRenderer, CliCardMetric, RenderContext};

#[derive(Debug, Clone, PartialEq)]
pub struct CLICardModel {
    pub provider: CliProvider,
    pub title: String,
    pub source_label: String,
    pub plan_badge: Option<String>,
    pub account_line: Option<String>,
    pub is_active: bool,
    pub uses_last_known_usage: bool,
    pub account_problem: Option<String>,
    pub info_lines: Vec<String>,
    pub metrics: Vec<CliCardMetric>,
    pub extra_lines: Vec<String>,
    pub history_summary: Option<String>,
    pub status_line: Option<String>,
}

impl CLICardModel {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        provider: CliProvider,
        title: impl Into<String>,
        source_label: impl Into<String>,
        plan_badge: Option<String>,
        account_line: Option<String>,
        is_active: bool,
        uses_last_known_usage: bool,
        account_problem: Option<String>,
        info_lines: Vec<String>,
        metrics: Vec<CliCardMetric>,
        extra_lines: Vec<String>,
        history_summary: Option<String>,
        status_line: Option<String>,
    ) -> Self {
        Self {
            provider,
            title: title.into(),
            source_label: source_label.into(),
            plan_badge,
            account_line,
            is_active,
            uses_last_known_usage,
            account_problem,
            info_lines,
            metrics,
            extra_lines,
            history_summary,
            status_line,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CLICardFailure {
    pub provider: CliProvider,
    pub account_label: Option<String>,
    pub message: String,
}

impl CLICardFailure {
    #[must_use]
    pub fn new(
        provider: CliProvider,
        account_label: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            account_label,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn display_name(&self) -> String {
        let provider_name = self.provider.display_name();
        if let Some(account) = self.account_label.as_deref().filter(|s| !s.is_empty()) {
            format!("{provider_name} ({account})")
        } else {
            provider_name.to_string()
        }
    }
}

pub struct CLICardBuildInput<'a> {
    pub provider: CliProvider,
    pub snapshot: &'a CliUsageSnapshot,
    pub credits: Option<&'a CliCreditsSnapshot>,
    pub source: &'a str,
    pub status: Option<&'a ProviderStatusPayload>,
    pub account_override: Option<String>,
    pub fallback_account_email: Option<String>,
    pub is_active: bool,
    pub uses_last_known_usage: bool,
    pub account_problem: Option<String>,
    pub notes: &'a [String],
    pub use_color: bool,
    pub reset_style: ResetTimeDisplayStyle,
    pub weekly_work_days: Option<u32>,
    pub now: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoxLineKind {
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentStyle {
    Normal,
    Subtle,
    Border,
}

pub struct CLICardsRenderer;

impl CLICardsRenderer {
    pub const MIN_CARD_WIDTH: usize = 38;
    pub const MAX_CARD_WIDTH: usize = 42;
    pub const CARD_GAP: usize = 2;

    #[must_use]
    pub fn column_count(terminal_width: usize, min_card_width: usize) -> usize {
        let usable = terminal_width.max(min_card_width);
        ((usable + Self::CARD_GAP) / (min_card_width + Self::CARD_GAP)).max(1)
    }

    #[must_use]
    pub fn card_width(terminal_width: usize, columns: usize) -> usize {
        let cols = columns.max(1);
        let total_gaps = (cols - 1) * Self::CARD_GAP;
        let available = (terminal_width.saturating_sub(total_gaps) / cols).max(1);
        available.min(Self::MAX_CARD_WIDTH)
    }

    #[must_use]
    pub fn make_card(input: CLICardBuildInput<'_>) -> CLICardModel {
        let display_name = input.provider.display_name();
        let context = RenderContext {
            header: display_name.to_string(),
            status: input.status.cloned(),
            account_override: input.account_override.clone(),
            fallback_account_email: input.fallback_account_email.clone(),
            use_color: input.use_color,
            reset_style: input.reset_style,
            notes: input.notes.to_vec(),
            weekly_work_days: input.weekly_work_days,
        };
        let account_line = if let Some(ov) = input
            .account_override
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(ov.to_string())
        } else {
            input
                .snapshot
                .account_email_for(input.provider.raw_value())
                .map(str::to_string)
        };
        let plan_badge = CLIRenderer::plan_badge_text(input.provider, input.snapshot);
        let info_lines = CLIRenderer::collect_card_info_lines(
            input.provider,
            input.snapshot,
            input.credits,
            &context,
            input.now,
        );
        let metrics = CLIRenderer::collect_card_metrics(
            input.provider,
            input.snapshot,
            input.reset_style,
            input.now,
            input.weekly_work_days,
        );
        let extra_lines =
            CLIRenderer::collect_card_extra_lines(input.provider, input.snapshot, &context);
        let history_summary =
            CLIRenderer::live_history_line(input.snapshot.cost_usage.as_ref(), false);
        let status_line = input
            .status
            .map(|st| CLIRenderer::status_line(st, input.use_color));

        CLICardModel::new(
            input.provider,
            display_name,
            Self::normalized_source_label(input.source),
            plan_badge,
            account_line,
            input.is_active,
            input.uses_last_known_usage,
            input.account_problem,
            info_lines,
            metrics,
            extra_lines,
            history_summary,
            status_line,
        )
    }

    #[must_use]
    pub fn make_claude_swap_card(
        account: &ProviderAccountUsageSnapshot,
        render_options: &CLIClaudeSwapCardsRenderOptions,
    ) -> CLICardModel {
        let source_label = if account.source_label.is_empty() {
            ClaudeSwapAccountProjection::SOURCE_LABEL.to_string()
        } else {
            account.source_label.clone()
        };
        let sanitized_label = CLIClaudeSwapText::sanitize_label(&account.display_label);
        let label = if sanitized_label.is_empty() {
            CLIClaudeSwapText::sanitize_label(&format!("Account {}", account.id.opaque_id))
        } else {
            sanitized_label
        };
        let problem = account
            .error
            .as_deref()
            .map(CLIClaudeSwapText::sanitize_diagnostic);
        if let Some(snapshot) = account.snapshot.as_ref() {
            let mut notes = Vec::new();
            if account.uses_last_known_usage
                && let Ok(ts) = snapshot.updated_at.parse::<Timestamp>()
            {
                let updated = format_updated_string(ts, render_options.now);
                notes.push(format!(
                    "Last known usage captured {}. {updated}",
                    snapshot.updated_at
                ));
            }
            let mut base = Self::make_card(CLICardBuildInput {
                provider: CliProvider::Claude,
                snapshot,
                credits: None,
                source: &source_label,
                status: render_options.status.as_ref(),
                account_override: Some(label.clone()),
                fallback_account_email: None,
                is_active: account.is_active,
                uses_last_known_usage: account.uses_last_known_usage,
                account_problem: problem.clone(),
                notes: &notes,
                use_color: render_options.use_color,
                reset_style: render_options.reset_style,
                weekly_work_days: render_options.weekly_work_days,
                now: render_options.now,
            });
            base.plan_badge = None;
            base.account_line = Some(label);
            base.is_active = account.is_active;
            base.uses_last_known_usage = account.uses_last_known_usage;
            base.account_problem = problem;
            return base;
        }

        let status_line = render_options
            .status
            .as_ref()
            .map(|st| CLIRenderer::status_line(st, render_options.use_color));
        CLICardModel::new(
            CliProvider::Claude,
            CliProvider::Claude.display_name(),
            source_label,
            None,
            Some(label),
            account.is_active,
            account.uses_last_known_usage,
            problem,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            status_line,
        )
    }

    #[must_use]
    pub fn render(
        cards: &[CLICardModel],
        failures: &[CLICardFailure],
        terminal_width: usize,
        use_color: bool,
        enhanced: bool,
    ) -> String {
        if cards.is_empty() {
            return Self::render_failures_only(failures, use_color);
        }

        let columns = Self::column_count(terminal_width, Self::MIN_CARD_WIDTH);
        let width = Self::card_width(terminal_width, columns);
        let mut chunks = Vec::new();
        let gap = " ".repeat(Self::CARD_GAP);

        let mut row_start = 0;
        while row_start < cards.len() {
            let row_end = (row_start + columns).min(cards.len());
            let row_cards = &cards[row_start..row_end];
            let rendered: Vec<Vec<String>> = row_cards
                .iter()
                .map(|c| Self::render_card(c, width, use_color, enhanced))
                .collect();
            let row_height = rendered.iter().map(Vec::len).max().unwrap_or(0);
            for line_index in 0..row_height {
                let parts: Vec<String> = rendered
                    .iter()
                    .map(|lines| {
                        if line_index < lines.len().saturating_sub(1) {
                            return lines[line_index].clone();
                        }
                        if line_index + 1 == row_height
                            && let Some(bottom) = lines.last()
                        {
                            return bottom.clone();
                        }
                        Self::empty_card_line(width, use_color, enhanced)
                    })
                    .collect();
                chunks.push(parts.join(&gap));
            }
            if row_start + columns < cards.len() {
                chunks.push(String::new());
            }
            row_start = row_end;
        }

        if !failures.is_empty() {
            if !chunks.is_empty() {
                chunks.push(String::new());
            }
            chunks.push(Self::render_failure_footer(failures, use_color));
        }

        chunks.join("\n")
    }

    #[must_use]
    pub fn render_card(
        card: &CLICardModel,
        width: usize,
        use_color: bool,
        enhanced: bool,
    ) -> Vec<String> {
        let inner_width = width.saturating_sub(4).max(12);
        let mut lines = Vec::new();
        lines.push(Self::box_line(
            BoxLineKind::Top,
            inner_width,
            use_color,
            enhanced,
        ));
        lines.push(Self::header_line(card, inner_width, use_color, enhanced));

        if let Some(account) = card
            .account_line
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let active = if card.is_active { " [active]" } else { "" };
            let label_width = inner_width.saturating_sub(2 + visible_width(active)).max(1);
            let account_text = format!("@ {}{active}", Self::truncate_plain(account, label_width));
            lines.push(Self::content_line(
                &account_text,
                inner_width,
                use_color,
                enhanced,
                ContentStyle::Subtle,
            ));
        }

        lines.push(Self::separator_line(inner_width, use_color, enhanced));

        if let Some(problem) = card.account_problem.as_deref().filter(|s| !s.is_empty()) {
            for problem_line in Self::wrap_plain_text(problem, inner_width) {
                lines.push(Self::content_line(
                    &problem_line,
                    inner_width,
                    use_color,
                    enhanced,
                    ContentStyle::Normal,
                ));
            }
        }

        for info_line in &card.info_lines {
            lines.push(Self::detail_line(
                info_line,
                inner_width,
                use_color,
                enhanced,
            ));
        }

        if !card.metrics.is_empty() && !card.info_lines.is_empty() {
            lines.push(Self::content_line(
                "",
                inner_width,
                use_color,
                enhanced,
                ContentStyle::Normal,
            ));
        }

        for (index, metric) in card.metrics.iter().enumerate() {
            if index > 0 {
                lines.push(Self::content_line(
                    "",
                    inner_width,
                    use_color,
                    enhanced,
                    ContentStyle::Normal,
                ));
            }
            lines.push(Self::metric_label_line(
                metric,
                inner_width,
                use_color,
                enhanced,
            ));
            if let Some(bar) = Self::metric_bar_line(metric, inner_width, use_color, enhanced) {
                lines.push(bar);
            }
            if let Some(reset_text) = metric.reset_text.as_deref() {
                lines.push(Self::content_line(
                    reset_text,
                    inner_width,
                    use_color,
                    enhanced,
                    ContentStyle::Subtle,
                ));
            }
            if let Some(detail_text) = metric.detail_text.as_deref() {
                lines.push(Self::content_line(
                    detail_text,
                    inner_width,
                    use_color,
                    enhanced,
                    ContentStyle::Subtle,
                ));
            }
        }

        for extra_line in &card.extra_lines {
            lines.push(Self::detail_line(
                extra_line,
                inner_width,
                use_color,
                enhanced,
            ));
        }

        if let Some(history) = card.history_summary.as_deref() {
            for history_line in Self::wrap_plain_text(history, inner_width) {
                lines.push(Self::content_line(
                    &history_line,
                    inner_width,
                    use_color,
                    enhanced,
                    ContentStyle::Normal,
                ));
            }
        }

        if let Some(status_line) = card.status_line.as_deref() {
            lines.push(Self::content_line(
                status_line,
                inner_width,
                use_color,
                enhanced,
                ContentStyle::Normal,
            ));
        }

        lines.push(Self::box_line(
            BoxLineKind::Bottom,
            inner_width,
            use_color,
            enhanced,
        ));
        lines
    }

    fn box_line(kind: BoxLineKind, inner_width: usize, use_color: bool, enhanced: bool) -> String {
        let (left, right) = match kind {
            BoxLineKind::Top => ("╭", "╮"),
            BoxLineKind::Bottom => ("╰", "╯"),
        };
        let line = format!("{left}{}{right}", "─".repeat(inner_width + 2));
        Self::style_border(&line, use_color, enhanced)
    }

    fn header_line(
        card: &CLICardModel,
        inner_width: usize,
        use_color: bool,
        enhanced: bool,
    ) -> String {
        let (title, badge) = if use_color && enhanced {
            (
                CLIRenderer::colorize_enhanced_accent_bold(&card.title),
                CLIRenderer::colorize_enhanced_badge(&card.source_label),
            )
        } else if use_color {
            (
                CLIRenderer::colorize_accent_bold(&card.title),
                CLIRenderer::colorize_card_badge(&card.source_label),
            )
        } else {
            (card.title.clone(), format!("[{}]", card.source_label))
        };
        let left = format!("{title} {badge}");
        let left_visible = Self::visible_length(&left);
        let raw_plan_text = card
            .plan_badge
            .as_deref()
            .map_or_else(String::new, |p| format!("PLAN {p}"));
        let max_plan_width = inner_width.saturating_sub(left_visible + 1);
        let plan_text = if max_plan_width >= 8 {
            Self::truncate_plain(&raw_plan_text, max_plan_width)
        } else {
            String::new()
        };
        let plan_visible = Self::visible_length(&plan_text);
        let gap = inner_width
            .saturating_sub(left_visible + plan_visible)
            .max(1);
        let plan = Self::plan_pill(&plan_text, use_color, enhanced);
        let content = format!("{left}{}{plan}", " ".repeat(gap));
        Self::side_border(
            &content,
            inner_width,
            use_color,
            enhanced,
            ContentStyle::Normal,
        )
    }

    fn plan_pill(text: &str, use_color: bool, enhanced: bool) -> String {
        if text.is_empty() {
            return String::new();
        }
        let Some((first, second)) = text.split_once(' ') else {
            return if use_color {
                CLIRenderer::colorize_card_plan_box(text)
            } else {
                text.to_string()
            };
        };
        if use_color && enhanced {
            return format!(
                "{} {}",
                CLIRenderer::colorize_enhanced_plan_label(first),
                CLIRenderer::colorize_enhanced_plan_value(second)
            );
        }
        if use_color {
            return format!(
                "{} {}",
                CLIRenderer::colorize_card_plan_box(first),
                CLIRenderer::colorize_warning(second)
            );
        }
        text.to_string()
    }

    fn separator_line(inner_width: usize, use_color: bool, enhanced: bool) -> String {
        Self::side_border(
            &"─".repeat(inner_width),
            inner_width,
            use_color,
            enhanced,
            ContentStyle::Border,
        )
    }

    fn metric_label_line(
        metric: &CliCardMetric,
        inner_width: usize,
        use_color: bool,
        enhanced: bool,
    ) -> String {
        let colored_percent = if let Some(remaining) = metric.remaining_percent {
            let percent_text = usage_line(remaining, 100.0 - remaining, false);
            if use_color && enhanced {
                CLIRenderer::colorize_enhanced_remaining_percent(&percent_text, remaining)
            } else {
                CLIRenderer::colorize_card_percent(&percent_text, remaining, use_color)
            }
        } else {
            "Unavailable".to_string()
        };
        let label = if use_color && enhanced {
            CLIRenderer::colorize_enhanced_readable(&metric.label)
        } else if use_color {
            CLIRenderer::colorize_readable(&metric.label)
        } else {
            metric.label.clone()
        };
        let gap = inner_width
            .saturating_sub(Self::visible_length(&label) + Self::visible_length(&colored_percent))
            .max(1);
        let content = format!("{label}{}{colored_percent}", " ".repeat(gap));
        Self::side_border(
            &content,
            inner_width,
            use_color,
            enhanced,
            ContentStyle::Normal,
        )
    }

    fn metric_bar_line(
        metric: &CliCardMetric,
        inner_width: usize,
        use_color: bool,
        enhanced: bool,
    ) -> Option<String> {
        let remaining = metric.remaining_percent?;
        let bar_width = inner_width.saturating_sub(4).max(4);
        let bar = if use_color && enhanced {
            CLIRenderer::gradient_remaining_track_bar(remaining, bar_width)
        } else {
            CLIRenderer::card_block_bar(remaining, bar_width, use_color)
        };
        Some(Self::side_border(
            &format!("[ {bar} ]"),
            inner_width,
            use_color,
            enhanced,
            ContentStyle::Normal,
        ))
    }

    fn detail_line(content: &str, inner_width: usize, use_color: bool, enhanced: bool) -> String {
        let normalized = Self::normalize_glyphs(content);
        let plain = strip_ansi_codes(&normalized);
        let Some((left_part, right_part)) = plain.split_once(':') else {
            return Self::content_line(
                &normalized,
                inner_width,
                use_color,
                enhanced,
                ContentStyle::Normal,
            );
        };

        let raw_label = format!("{}:", left_part.trim());
        let raw_value = right_part.trim();
        let label = if use_color && enhanced {
            CLIRenderer::colorize_enhanced_readable(&raw_label)
        } else if use_color {
            CLIRenderer::colorize_readable(&raw_label)
        } else {
            raw_label
        };
        let value = if use_color && enhanced {
            CLIRenderer::colorize_enhanced_good(raw_value)
        } else if use_color {
            CLIRenderer::colorize_accent(raw_value)
        } else {
            raw_value.to_string()
        };
        let gap = inner_width
            .saturating_sub(Self::visible_length(&label) + Self::visible_length(&value))
            .max(1);
        let line = format!("{label}{}{value}", " ".repeat(gap));
        Self::side_border(
            &line,
            inner_width,
            use_color,
            enhanced,
            ContentStyle::Normal,
        )
    }

    fn content_line(
        content: &str,
        inner_width: usize,
        use_color: bool,
        enhanced: bool,
        style: ContentStyle,
    ) -> String {
        let normalized = Self::normalize_glyphs(content);
        let stripped = strip_ansi_codes(&normalized);
        let clipped = if visible_width(&stripped) <= inner_width {
            normalized
        } else {
            Self::truncate_plain(&stripped, inner_width)
        };
        let display = if style == ContentStyle::Subtle && use_color && enhanced {
            CLIRenderer::colorize_enhanced_subtle(&strip_ansi_codes(&clipped))
        } else if style == ContentStyle::Subtle && use_color {
            CLIRenderer::colorize_subtle(&strip_ansi_codes(&clipped))
        } else {
            clipped
        };
        Self::side_border(&display, inner_width, use_color, enhanced, style)
    }

    fn side_border(
        content: &str,
        inner_width: usize,
        use_color: bool,
        enhanced: bool,
        content_style: ContentStyle,
    ) -> String {
        let fitted = Self::fit_content(content, inner_width);
        let padding = inner_width.saturating_sub(Self::visible_length(&fitted));
        let padded = format!("{fitted}{}", " ".repeat(padding));
        let visible = format!("│ {padded} │");
        if !use_color {
            return visible;
        }
        let left = Self::style_border("│ ", use_color, enhanced);
        let right = Self::style_border(" │", use_color, enhanced);
        let styled_content = if content_style == ContentStyle::Border {
            Self::style_border(&padded, use_color, enhanced)
        } else {
            padded
        };
        format!("{left}{styled_content}{right}")
    }

    fn style_border(text: &str, use_color: bool, enhanced: bool) -> String {
        if !use_color {
            return text.to_string();
        }
        if enhanced {
            return CLIRenderer::colorize_enhanced_border(text);
        }
        CLIRenderer::colorize_card_border(text)
    }

    fn empty_card_line(width: usize, use_color: bool, enhanced: bool) -> String {
        let inner_width = width.saturating_sub(4).max(12);
        Self::side_border("", inner_width, use_color, enhanced, ContentStyle::Normal)
    }

    fn visible_length(text: &str) -> usize {
        visible_width(&strip_ansi_codes(&Self::normalize_glyphs(text)))
    }

    #[must_use]
    pub fn truncate(text: &str, max_width: usize) -> String {
        Self::truncate_plain(text, max_width)
    }

    #[must_use]
    pub fn truncate_plain(text: &str, width: usize) -> String {
        if width == 0 {
            return String::new();
        }
        let graphemes: Vec<&str> = text.graphemes(true).collect();
        if graphemes.len() <= width {
            return text.to_string();
        }
        if width == 1 {
            return graphemes[..1].concat();
        }
        format!("{}…", graphemes[..width - 1].concat())
    }

    #[must_use]
    pub fn wrap_plain_text(text: &str, width: usize) -> Vec<String> {
        if width == 0 {
            return Vec::new();
        }
        let mut lines = Vec::new();
        let mut line = String::new();
        for word in text.split_whitespace() {
            let word_graphemes: Vec<&str> = word.graphemes(true).collect();
            if word_graphemes.len() > width {
                if !line.is_empty() {
                    lines.push(line);
                }
                let mut idx = 0;
                while word_graphemes.len() - idx > width {
                    lines.push(word_graphemes[idx..idx + width].concat());
                    idx += width;
                }
                line = word_graphemes[idx..].concat();
            } else if line.is_empty() {
                line = word.to_string();
            } else if visible_width(&line) + 1 + word_graphemes.len() <= width {
                line.push(' ');
                line.push_str(word);
            } else {
                lines.push(line);
                line = word.to_string();
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
        lines
    }

    fn fit_content(text: &str, width: usize) -> String {
        if Self::visible_length(text) <= width {
            return text.to_string();
        }
        Self::truncate_plain(&strip_ansi_codes(text), width)
    }

    #[must_use]
    pub fn normalize_glyphs(text: &str) -> String {
        text.replace('👤', "@")
            .replace("⏳ Resets in ", "Reset in ")
            .replace("⏳ Resets ", "Reset ")
            .replace("⏳ ", "Reset ")
    }

    #[must_use]
    pub fn render_failure_footer(failures: &[CLICardFailure], use_color: bool) -> String {
        let mut lines = vec!["Failed providers:".to_string()];
        for failure in failures {
            let name = failure.provider.display_name();
            if let Some(account) = failure.account_label.as_deref().filter(|s| !s.is_empty()) {
                lines.push(format!("  - {name} ({account}): {}", failure.message));
            } else {
                lines.push(format!("  - {name}: {}", failure.message));
            }
        }
        let text = lines.join("\n");
        if !use_color {
            return text;
        }
        CLIRenderer::colorize_error(&text)
    }

    #[must_use]
    pub fn render_failures_only(failures: &[CLICardFailure], use_color: bool) -> String {
        if failures.is_empty() {
            return String::new();
        }
        Self::render_failure_footer(failures, use_color)
    }

    #[must_use]
    pub fn normalized_source_label(source: &str) -> String {
        let trimmed = source.trim();
        if trimmed.is_empty() {
            return "auto".to_string();
        }
        if trimmed.contains("oauth") {
            return "oauth".to_string();
        }
        if trimmed.contains("web") || trimmed.contains("openai-web") {
            return "web".to_string();
        }
        if trimmed.contains("api") {
            return "api".to_string();
        }
        if trimmed.contains("cli") {
            return "cli".to_string();
        }
        trimmed.to_string()
    }
}
