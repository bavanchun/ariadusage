// Ported from CodexBar Sources/CodexBarCLI/CLICardsBriefRenderer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use jiff::Timestamp;
use unicode_segmentation::UnicodeSegmentation;

use crate::cards_renderer::{CLICardFailure, CLICardModel, CLICardsRenderer};
use crate::format::strip_ansi_codes;
use crate::provider_meta::CliProvider;
use crate::renderer::CLIRenderer;

#[derive(Debug, Clone, PartialEq)]
pub struct CLICardsBriefRow {
    pub provider: CliProvider,
    pub provider_name: String,
    pub source_label: String,
    pub plan_badge: Option<String>,
    pub account_label: Option<String>,
    pub is_active: bool,
    pub account_problem: Option<String>,
    pub metric_label: Option<String>,
    pub used_percent: Option<f64>,
    pub reset_label: Option<String>,
    pub reset_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy)]
struct TableWidths {
    provider: usize,
    usage: usize,
    reset: usize,
}

pub struct CLICardsBriefRenderer;

impl CLICardsBriefRenderer {
    const PROVIDER_MIN: usize = 20;
    const PROVIDER_FLOOR: usize = 8;
    const PROVIDER_MAX: usize = 34;
    const USAGE_MIN: usize = 22;
    const USAGE_FLOOR: usize = 9;
    const USAGE_WIDTH: usize = 28;
    const USAGE_BAR_MAX_WIDTH: usize = 22;
    const RESET_MIN: usize = 8;
    const RESET_FLOOR: usize = 5;
    const RESET_MAX: usize = 10;
    const TABLE_BORDER_OVERHEAD: usize = 10;

    #[must_use]
    pub fn make_rows(cards: &[CLICardModel]) -> Vec<CLICardsBriefRow> {
        cards
            .iter()
            .map(|card| {
                let metric = if card.uses_last_known_usage {
                    None
                } else {
                    card.metrics.first()
                };
                let used_percent = metric
                    .and_then(|m| m.remaining_percent)
                    .map(|rem| (100.0 - rem).clamp(0.0, 100.0));
                let reset_label =
                    Self::brief_reset_label(metric.and_then(|m| m.reset_text.as_deref()));

                CLICardsBriefRow {
                    provider: card.provider,
                    provider_name: card.title.clone(),
                    source_label: card.source_label.clone(),
                    plan_badge: card.plan_badge.clone(),
                    account_label: card.account_line.clone(),
                    is_active: card.is_active,
                    account_problem: card.account_problem.clone(),
                    metric_label: metric.map(|m| m.label.clone()),
                    used_percent,
                    reset_label,
                    reset_at: metric.and_then(|m| m.reset_at),
                }
            })
            .collect()
    }

    #[must_use]
    pub fn render(
        rows: &[CLICardsBriefRow],
        failures: &[CLICardFailure],
        terminal_width: usize,
        use_color: bool,
        enhanced: bool,
        now: Timestamp,
    ) -> String {
        if rows.is_empty() {
            return CLICardsRenderer::render_failures_only(failures, use_color);
        }

        let mut lines = Vec::new();
        lines.push(Self::title_line(terminal_width, use_color, enhanced, now));
        lines.extend(Self::summary_lines(
            rows,
            terminal_width,
            use_color,
            enhanced,
            now,
        ));
        lines.push(String::new());

        let widths = Self::table_column_widths(rows, terminal_width);
        lines.push(Self::horizontal_border(
            "┌", "┬", "┐", widths, use_color, enhanced,
        ));
        lines.push(Self::header_row(widths, use_color, enhanced));
        lines.push(Self::horizontal_border(
            "├", "┼", "┤", widths, use_color, enhanced,
        ));

        for row in rows {
            lines.push(Self::data_row(row, widths, use_color, enhanced));
        }

        lines.push(Self::horizontal_border(
            "└", "┴", "┘", widths, use_color, enhanced,
        ));

        let warnings = Self::warning_lines(rows, terminal_width, use_color, enhanced);
        if !warnings.is_empty() {
            lines.push(String::new());
            lines.extend(warnings);
        }

        if !failures.is_empty() {
            lines.push(String::new());
            lines.push(CLICardsRenderer::render_failure_footer(failures, use_color));
        }

        lines.join("\n")
    }

    fn title_line(
        terminal_width: usize,
        use_color: bool,
        enhanced: bool,
        now: Timestamp,
    ) -> String {
        let left_plain = "ariadusage • AI Usage & Limits";
        let left = if use_color {
            if enhanced {
                CLIRenderer::colorize_enhanced_accent_bold(left_plain)
            } else {
                CLIRenderer::colorize_accent_bold(left_plain)
            }
        } else {
            left_plain.to_string()
        };

        let timestamp = Self::timestamp_text(now);
        let left_width = Self::visible_length(&left);
        if left_width + timestamp.len() + 1 > terminal_width {
            if left_width <= terminal_width {
                return left;
            }
            return Self::truncate(left_plain, terminal_width);
        }

        let gap = (terminal_width - left_width - timestamp.len()).max(1);
        let right = if use_color {
            if enhanced {
                CLIRenderer::colorize_enhanced_readable_muted(&timestamp)
            } else {
                CLIRenderer::colorize_subtle(&timestamp)
            }
        } else {
            timestamp
        };
        format!("{left}{}{right}", " ".repeat(gap))
    }

    fn summary_lines(
        rows: &[CLICardsBriefRow],
        terminal_width: usize,
        use_color: bool,
        enhanced: bool,
        now: Timestamp,
    ) -> Vec<String> {
        let Some(next_reset) = Self::next_reset_summary(rows, now) else {
            return Vec::new();
        };

        let raw = format!("Next reset: {next_reset}");
        let wrapped = Self::wrap_plain_text(&raw, "", "  ", terminal_width);
        wrapped
            .into_iter()
            .map(|line| {
                if use_color {
                    if enhanced {
                        CLIRenderer::colorize_enhanced_readable(&line)
                    } else {
                        CLIRenderer::colorize_readable(&line)
                    }
                } else {
                    line
                }
            })
            .collect()
    }

    fn next_reset_summary(rows: &[CLICardsBriefRow], now: Timestamp) -> Option<String> {
        let mut candidates: Vec<(&CLICardsBriefRow, String, Timestamp)> = rows
            .iter()
            .filter_map(|row| {
                let reset = row.reset_label.as_deref()?;
                if reset.is_empty() || reset == "—" {
                    return None;
                }
                let sort_date = row.reset_at.or_else(|| Self::reset_sort_date(reset, now))?;
                Some((row, reset.to_string(), sort_date))
            })
            .collect();

        candidates.sort_by_key(|a| a.2);

        let (row, label, _) = candidates.into_iter().next()?;
        let separator = if Self::reset_duration_minutes(&label).is_none() {
            " · "
        } else {
            " in "
        };
        Some(format!("{}{separator}{label}", row.provider_name))
    }

    fn reset_sort_date(text: &str, now: Timestamp) -> Option<Timestamp> {
        let minutes = Self::reset_duration_minutes(text)?;
        now.checked_add(jiff::SignedDuration::from_secs(i64::from(minutes) * 60))
            .ok()
    }

    fn reset_duration_minutes(label: &str) -> Option<i32> {
        let mut minutes = 0i32;
        let mut matched = false;
        if let Some(days) = Self::first_int_match(label, b'd') {
            matched = true;
            minutes += days * 24 * 60;
        }
        if let Some(hours) = Self::first_int_match(label, b'h') {
            matched = true;
            minutes += hours * 60;
        }
        if let Some(mins) = Self::first_int_match(label, b'm') {
            matched = true;
            minutes += mins;
        }
        matched.then_some(minutes)
    }

    fn first_int_match(label: &str, suffix: u8) -> Option<i32> {
        let bytes = label.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_digit() {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == suffix {
                    return label[start..i].parse::<i32>().ok();
                }
            } else {
                i += 1;
            }
        }
        None
    }

    fn horizontal_border(
        left: &str,
        mid: &str,
        right: &str,
        widths: TableWidths,
        use_color: bool,
        enhanced: bool,
    ) -> String {
        let provider_line = "─".repeat(widths.provider + 2);
        let usage_line = "─".repeat(widths.usage + 2);
        let reset_line = "─".repeat(widths.reset + 2);
        let border = format!("{left}{provider_line}{mid}{usage_line}{mid}{reset_line}{right}");
        if !use_color {
            return border;
        }
        if enhanced {
            CLIRenderer::colorize_enhanced_border(&border)
        } else {
            CLIRenderer::colorize_card_border(&border)
        }
    }

    fn header_row(widths: TableWidths, use_color: bool, enhanced: bool) -> String {
        let provider = Self::header_cell("Provider", widths.provider, false, use_color, enhanced);
        let usage = Self::header_cell("Usage", widths.usage, false, use_color, enhanced);
        let reset = Self::header_cell("Reset", widths.reset, true, use_color, enhanced);
        format!("│ {provider} │ {usage} │ {reset} │")
    }

    fn data_row(
        row: &CLICardsBriefRow,
        widths: TableWidths,
        use_color: bool,
        enhanced: bool,
    ) -> String {
        let provider = Self::provider_styled_cell(row, widths.provider, use_color, enhanced);
        let usage = if let Some(problem) = row.account_problem.as_deref().filter(|s| !s.is_empty())
        {
            let fitted = Self::fit_cell(problem, widths.usage, false);
            if use_color && enhanced {
                CLIRenderer::colorize_enhanced_readable(&fitted)
            } else if use_color {
                CLIRenderer::colorize_readable(&fitted)
            } else {
                fitted
            }
        } else if let Some(used_percent) = row.used_percent {
            let percent = format!("{}%", used_percent.round() as i64);
            let bar_width = widths
                .usage
                .saturating_sub(Self::visible_length(&percent) + 1)
                .clamp(4, Self::USAGE_BAR_MAX_WIDTH);
            let bar = if use_color && enhanced {
                CLIRenderer::gradient_used_bar(used_percent, bar_width)
            } else {
                CLIRenderer::card_used_bar(used_percent, bar_width, use_color)
            };
            let styled_percent = if use_color && enhanced {
                CLIRenderer::colorize_enhanced_used_percent(&percent, used_percent)
            } else {
                CLIRenderer::colorize_card_used_percent(&percent, used_percent, use_color)
            };
            Self::pad_right(&format!("{styled_percent} {bar}"), widths.usage)
        } else {
            Self::pad_right("—", widths.usage)
        };

        let reset = Self::styled_reset_cell(
            row.reset_label.as_deref().unwrap_or("—"),
            widths.reset,
            use_color,
            enhanced,
        );

        format!("│ {provider} │ {usage} │ {reset} │")
    }

    fn header_cell(
        title: &str,
        width: usize,
        align_right: bool,
        use_color: bool,
        enhanced: bool,
    ) -> String {
        let styled = if use_color && enhanced {
            CLIRenderer::colorize_enhanced_readable(title)
        } else if use_color {
            CLIRenderer::colorize_readable(title)
        } else {
            title.to_string()
        };
        if align_right {
            Self::pad_left(&styled, width)
        } else {
            Self::pad_right(&styled, width)
        }
    }

    fn styled_reset_cell(text: &str, width: usize, use_color: bool, enhanced: bool) -> String {
        let styled = if use_color && enhanced {
            CLIRenderer::colorize_enhanced_readable_muted(text)
        } else if use_color {
            CLIRenderer::colorize_subtle(text)
        } else {
            text.to_string()
        };
        Self::fit_cell(&styled, width, true)
    }

    fn provider_plain_label(row: &CLICardsBriefRow) -> String {
        if let Some(account) = row
            .account_label
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let active = if row.is_active { " [active]" } else { "" };
            return format!(
                "{}{active} · {account} · {}",
                row.provider_name, row.source_label
            );
        }
        if let Some(plan) = row.plan_badge.as_deref().filter(|s| !s.is_empty()) {
            return format!("{} · {} · {plan}", row.provider_name, row.source_label);
        }
        format!("{} · {}", row.provider_name, row.source_label)
    }

    fn provider_styled_cell(
        row: &CLICardsBriefRow,
        width: usize,
        use_color: bool,
        enhanced: bool,
    ) -> String {
        if row.is_active && width < row.provider_name.chars().count() + " [active]".len() {
            let fitted = Self::fit_cell("[active]", width, false);
            if use_color && enhanced {
                return CLIRenderer::colorize_enhanced_readable(&fitted);
            }
            return if use_color {
                CLIRenderer::colorize_readable(&fitted)
            } else {
                fitted
            };
        }
        if row
            .account_label
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| !s.is_empty())
        {
            let fitted = Self::fit_cell(&Self::provider_plain_label(row), width, false);
            if use_color && enhanced {
                return CLIRenderer::colorize_enhanced_readable(&fitted);
            }
            return if use_color {
                CLIRenderer::colorize_readable(&fitted)
            } else {
                fitted
            };
        }
        if !use_color {
            return Self::plain_provider_cell(row, width);
        }

        let source_visible_width = row.source_label.chars().count() + 2;
        let prefix_visible_width = row.provider_name.chars().count() + 1 + source_visible_width;
        let plan = row.plan_badge.as_deref().filter(|s| !s.is_empty());
        let plan_prefix = " · ";
        let plan_width = plan
            .map(|_| width.saturating_sub(prefix_visible_width + plan_prefix.chars().count()))
            .unwrap_or(0);

        let styled = if enhanced {
            format!(
                "{} {}{}",
                CLIRenderer::colorize_enhanced_accent_bold(&row.provider_name),
                CLIRenderer::colorize_enhanced_badge(&row.source_label),
                Self::styled_provider_plan(plan, plan_prefix, plan_width, use_color, enhanced),
            )
        } else {
            format!(
                "{} {}{}",
                CLIRenderer::colorize_readable(&row.provider_name),
                CLIRenderer::colorize_card_badge(&row.source_label),
                Self::styled_provider_plan(plan, plan_prefix, plan_width, use_color, enhanced),
            )
        };
        Self::fit_cell(&styled, width, false)
    }

    fn styled_provider_plan(
        plan: Option<&str>,
        prefix: &str,
        width: usize,
        use_color: bool,
        enhanced: bool,
    ) -> String {
        let Some(plan) = plan.filter(|_| width > 0) else {
            return String::new();
        };
        let text = format!("{prefix}{}", Self::truncate(plan, width));
        if use_color && enhanced {
            return CLIRenderer::colorize_enhanced_readable_muted(&text);
        }
        if use_color {
            return CLIRenderer::colorize_subtle(&text);
        }
        text
    }

    fn plain_provider_cell(row: &CLICardsBriefRow, width: usize) -> String {
        if row
            .account_label
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| !s.is_empty())
        {
            return Self::fit_cell(&Self::provider_plain_label(row), width, false);
        }
        let Some(plan) = row.plan_badge.as_deref().filter(|s| !s.is_empty()) else {
            return Self::fit_cell(&Self::provider_plain_label(row), width, false);
        };
        let prefix = format!("{} · {} · ", row.provider_name, row.source_label);
        let prefix_len = Self::visible_length(&prefix);
        let plan_width = width.saturating_sub(prefix_len);
        if plan_width > 0 {
            return Self::pad_right(
                &format!("{prefix}{}", Self::truncate(plan, plan_width)),
                width,
            );
        }
        Self::fit_cell(
            &format!("{} · {}", row.provider_name, row.source_label),
            width,
            false,
        )
    }

    fn warning_lines(
        rows: &[CLICardsBriefRow],
        terminal_width: usize,
        use_color: bool,
        _enhanced: bool,
    ) -> Vec<String> {
        let mut parts = Vec::new();
        for row in rows {
            let Some(used_percent) = row.used_percent else {
                continue;
            };
            if used_percent < 85.0 {
                continue;
            }
            let used = used_percent.round() as i64;
            let label = row.metric_label.as_deref().unwrap_or("Usage");
            parts.push(format!("{} {label}: {used}% used", row.provider_name));
        }

        if parts.is_empty() {
            return Vec::new();
        }

        let text = parts.join("; ");
        let wrapped = Self::wrap_plain_text(&text, "⚠ Warnings: ", "  ", terminal_width);
        wrapped
            .into_iter()
            .map(|line| {
                if use_color {
                    CLIRenderer::colorize_warning(&line)
                } else {
                    line
                }
            })
            .collect()
    }

    fn brief_reset_label(reset_text: Option<&str>) -> Option<String> {
        let mut value = reset_text?.trim();
        if value.is_empty() {
            return None;
        }
        if let Some(rest) = value.strip_prefix("⏳ ") {
            value = rest;
        }
        if let Some(rest) = value.strip_prefix("Resets in ") {
            value = rest;
        } else if let Some(rest) = value.strip_prefix("Resets ") {
            value = rest;
        }
        (!value.is_empty()).then(|| value.to_string())
    }

    fn timestamp_text(now: Timestamp) -> String {
        now.to_zoned(jiff::tz::TimeZone::system())
            .strftime("%Y-%m-%d %H:%M %Z")
            .to_string()
    }

    fn table_column_widths(rows: &[CLICardsBriefRow], terminal_width: usize) -> TableWidths {
        let max_provider_text = rows
            .iter()
            .map(|row| Self::visible_length(&Self::provider_plain_label(row)))
            .max()
            .unwrap_or(Self::PROVIDER_MIN);
        let max_reset_text = rows
            .iter()
            .filter_map(|row| row.reset_label.as_deref())
            .map(Self::visible_length)
            .max()
            .unwrap_or(6);

        let mut provider = max_provider_text.clamp(Self::PROVIDER_MIN, Self::PROVIDER_MAX);
        let mut usage = Self::USAGE_WIDTH;
        let mut reset = max_reset_text.clamp(Self::RESET_MIN, Self::RESET_MAX);

        while provider + usage + reset + Self::TABLE_BORDER_OVERHEAD > terminal_width {
            if usage > Self::USAGE_MIN {
                usage -= 1;
                continue;
            }
            if provider > Self::PROVIDER_MIN {
                provider -= 1;
                continue;
            }
            if reset > Self::RESET_MIN {
                reset -= 1;
                continue;
            }
            break;
        }

        while provider + usage + reset + Self::TABLE_BORDER_OVERHEAD > terminal_width {
            if provider > Self::PROVIDER_FLOOR {
                provider -= 1;
                continue;
            }
            if usage > Self::USAGE_FLOOR {
                usage -= 1;
                continue;
            }
            if reset > Self::RESET_FLOOR {
                reset -= 1;
                continue;
            }
            break;
        }

        TableWidths {
            provider,
            usage,
            reset,
        }
    }

    fn fit_cell(text: &str, width: usize, align_right: bool) -> String {
        let visible = Self::visible_length(text);
        if visible <= width {
            return if align_right {
                Self::pad_left(text, width)
            } else {
                Self::pad_right(text, width)
            };
        }
        let plain = strip_ansi_codes(text);
        let clipped = Self::truncate(&plain, width);
        if align_right {
            Self::pad_left(&clipped, width)
        } else {
            clipped
        }
    }

    fn truncate(text: &str, max_width: usize) -> String {
        if max_width == 0 {
            return String::new();
        }
        let glyphs: Vec<&str> = text.graphemes(true).collect();
        if glyphs.len() <= max_width {
            return text.to_string();
        }
        if max_width == 1 {
            return glyphs[..1].concat();
        }
        format!("{}…", glyphs[..max_width - 1].concat())
    }

    fn pad_right(text: &str, width: usize) -> String {
        let length = Self::visible_length(text);
        if length >= width {
            return text.to_string();
        }
        format!("{text}{}", " ".repeat(width - length))
    }

    fn pad_left(text: &str, width: usize) -> String {
        let length = Self::visible_length(text);
        if length >= width {
            return text.to_string();
        }
        format!("{}{text}", " ".repeat(width - length))
    }

    fn wrap_plain_text(
        text: &str,
        first_prefix: &str,
        continuation_prefix: &str,
        width: usize,
    ) -> Vec<String> {
        let line_width = width.max(16);
        let mut lines = Vec::new();
        let mut line = first_prefix.to_string();
        let mut has_content = false;

        for word in text.split(' ').filter(|w| !w.is_empty()) {
            let separator = if has_content { " " } else { "" };
            let candidate_len = Self::visible_length(&line)
                + Self::visible_length(separator)
                + Self::visible_length(word);
            if candidate_len <= line_width {
                line.push_str(separator);
                line.push_str(word);
                has_content = true;
                continue;
            }
            if has_content {
                lines.push(line);
            } else if !line.is_empty() {
                lines.push(Self::truncate(&line, line_width));
            }
            let available = line_width
                .saturating_sub(Self::visible_length(continuation_prefix))
                .max(1);
            line = format!("{continuation_prefix}{}", Self::truncate(word, available));
            has_content = true;
        }

        if has_content {
            lines.push(line);
        }
        lines
    }

    fn visible_length(text: &str) -> usize {
        strip_ansi_codes(text).graphemes(true).count()
    }
}
