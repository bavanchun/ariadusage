// Ported from CodexBar Sources/CodexBar/MenuUsageBarStrip.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/InlineUsageDashboardContent.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/CostHistoryChartMenuView.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PlanUtilizationHistoryChartMenuView.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/QuotaBurndownChartMenuView.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/QuotaBurndownModel.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/ChartBarHoverSelection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/ChartAxisLabelLayout.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Pure-Rust models, layout geometry and hover hit-testing for:
//! - In-card 30-day mini usage bar strip (`MenuUsageBarStrip` / `InlineUsageDashboardContent`)
//! - Cost history submenu chart (`CostHistoryChartMenuView`)
//! - Quota burndown submenu chart (`QuotaBurndownChartMenuView` / `QuotaBurndownModel`)
//! - Plan utilization history submenu chart (`PlanUtilizationHistoryChartMenuView`)

use ariadusage_protocol::metric::{Metric, MetricState};
use ariadusage_protocol::snapshot::ProviderSnapshot;
use ariadusage_protocol::usage::{
    CostDailyEntry, PlanUtilizationSeries as ProtocolUtilizationSeries, RateWindow,
};
use jiff::Timestamp;

use crate::card::{Accent, Rgb, card_presentation};
use crate::format::{self, DateTexts};
use crate::geometry::{Point, Rect};

fn displayable_value<T>(metric: &Metric<T>) -> Option<&T> {
    match metric.state {
        MetricState::Value | MetricState::Stale => metric.value.as_ref(),
        _ => None,
    }
}

/// Resolves the accent color for `provider` from its presentation block or fallback palette.
pub fn provider_accent_color(provider: &ProviderSnapshot) -> Rgb {
    if let Accent::Brand(rgb) = card_presentation(provider).accent {
        return rgb;
    }
    match provider.id.as_str() {
        "claude" => Rgb {
            r: 217,
            g: 119,
            b: 87,
        },
        "codex" => Rgb {
            r: 16,
            g: 185,
            b: 129,
        },
        "antigravity" => Rgb {
            r: 99,
            g: 102,
            b: 241,
        },
        _ => Rgb {
            r: 59,
            g: 130,
            b: 246,
        },
    }
}

/// Height of the 30-day inline bar strip (`InlineUsageDashboardContent.swift:92`).
pub const MINI_BAR_STRIP_HEIGHT: f64 = 44.0;
/// Height of the top scale/hover label above the mini bar strip (`InlineUsageDashboardContent.swift:89`).
pub const MINI_BAR_LABEL_HEIGHT: f64 = 12.0;
/// Vertical spacing between the top scale label and the mini bar strip (`InlineUsageDashboardContent.swift:85`).
pub const MINI_BAR_BLOCK_SPACING: f64 = 2.0;
/// Total height of the mini bar block (`12 + 2 + 44 = 58 pt`, `InlineUsageDashboardContent.swift:96`).
pub const MINI_BAR_BLOCK_HEIGHT: f64 = 58.0;
/// Corner radius of each mini bar (`MenuUsageBarStrip.swift:52`).
pub const MINI_BAR_CORNER_RADIUS: f64 = 1.5;

/// Submenu chart item width (`StatusItemController+Menu.swift:722`).
pub const SUBMENU_CHART_WIDTH: f64 = 310.0;
/// Horizontal padding inside submenu chart views (`CostHistoryChartMenuView.swift:405`).
pub const SUBMENU_CHART_H_PADDING: f64 = 16.0;
/// Vertical padding inside submenu chart views (`CostHistoryChartMenuView.swift:462`).
pub const SUBMENU_CHART_V_PADDING: f64 = 10.0;
/// Height of the Core Graphics chart canvas in submenu chart views (`130 pt`).
pub const SUBMENU_CHART_CANVAS_HEIGHT: f64 = 130.0;
/// Ratio of bar width to day slot width in the cost history chart (`ChartBarHoverSelection.swift:4`).
pub const COST_BAR_WIDTH_RATIO: f64 = 0.7;
/// Fixed bar width in the plan utilization chart (`PlanUtilizationHistoryChartMenuView.swift:12`).
pub const PLAN_UTILIZATION_BAR_WIDTH: f64 = 6.0;
/// Half-width for hover hit-testing in the plan utilization chart (`PlanUtilizationHistoryChartMenuView.swift:13`).
pub const PLAN_UTILIZATION_BAR_HALF_WIDTH: f64 = 3.0;

// ---------------------------------------------------------------------------
// 1. In-card 30-day Mini Usage Bar Strip (`MenuUsageBarStrip`)
// ---------------------------------------------------------------------------

/// A single day point in the in-card 30-day mini usage bar strip.
#[derive(Debug, Clone, PartialEq)]
pub struct MiniBarPoint {
    pub day_key: String,
    pub value: Option<f64>,
    pub hover_summary: Option<String>,
}

/// Geometry of a single bar in the 30-day mini usage bar strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MiniBarRect {
    pub index: usize,
    pub rect: Rect,
    pub corner_radius: f64,
    pub alpha: f64,
}

/// Computed geometry for `MenuUsageBarStrip` in top-down coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct MiniBarStripGeometry {
    pub baseline: Rect,
    pub bars: Vec<MiniBarRect>,
}

impl MiniBarStripGeometry {
    /// Computes baseline and bar rects inside `strip_rect` (`MenuUsageBarStrip.swift:26-58`).
    pub fn compute(
        strip_rect: Rect,
        points: &[MiniBarPoint],
        hovered_index: Option<usize>,
    ) -> Self {
        let width = strip_rect.w.max(1.0);
        let height = strip_rect.h.max(2.0);
        let baseline_y = strip_rect.y + height - 1.0;
        let baseline = Rect::new(strip_rect.x, baseline_y, width, 1.0);

        let count = points.len();
        if count == 0 {
            return Self {
                baseline,
                bars: Vec::new(),
            };
        }

        let chart_height = (height - 2.0).max(1.0);
        let total_gap = ((count.saturating_sub(1)) as f64)
            .max(0.0)
            .min(width * 0.35);
        let gap = if count > 1 {
            total_gap / (count - 1) as f64
        } else {
            0.0
        };
        let bar_width = ((width - total_gap) / count as f64).max(1.0);
        let max_value = points
            .iter()
            .filter_map(|p| p.value)
            .fold(0.0_f64, f64::max);

        let mut bars = Vec::with_capacity(count);
        for (index, point) in points.iter().enumerate() {
            let val = point.value.unwrap_or(0.0);
            let raw_ratio = if max_value > 0.0 {
                val / max_value
            } else {
                0.0
            };
            let ratio = raw_ratio.clamp(0.0, 1.0);
            let min_bar_height = if val > 0.0 { 2.0 } else { 0.0 };
            let bar_height = (chart_height * ratio).max(min_bar_height);
            let x = strip_rect.x + index as f64 * (bar_width + gap);
            let y = baseline_y - bar_height;
            let alpha = if hovered_index == Some(index) {
                0.95
            } else {
                0.42 + 0.58 * ratio
            };
            bars.push(MiniBarRect {
                index,
                rect: Rect::new(x, y, bar_width, bar_height),
                corner_radius: MINI_BAR_CORNER_RADIUS,
                alpha,
            });
        }

        Self { baseline, bars }
    }

    /// Hit-tests a pointer position `(x, y)` relative to `strip_rect.origin`
    /// (`MenuUsageBarStrip.hoveredIndex`, `MenuUsageBarStrip.swift:12-23`).
    pub fn hovered_index(x: f64, y: f64, width: f64, height: f64, count: usize) -> Option<usize> {
        if count == 0 || width <= 0.0 || height <= 0.0 {
            return None;
        }
        if x < 0.0 || x > width || y < 0.0 || y > height {
            return None;
        }
        let total_gap = ((count.saturating_sub(1)) as f64)
            .max(0.0)
            .min(width * 0.35);
        let gap = if count > 1 {
            total_gap / (count - 1) as f64
        } else {
            0.0
        };
        let bar_width = ((width - total_gap) / count as f64).max(1.0);
        let stride = (bar_width + gap).max(1.0);
        let raw_index = (x / stride).floor() as isize;
        Some(raw_index.clamp(0, count as isize - 1) as usize)
    }
}

// ---------------------------------------------------------------------------
// 2. Shared Bar Hover Selection Helpers (`ChartBarHoverSelection.swift`)
// ---------------------------------------------------------------------------

/// Finds the index of the bar center closest to `target_x`.
pub fn nearest_center_index(centers: &[f64], target_x: f64) -> Option<usize> {
    centers
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            let da = (*a - target_x).abs();
            let db = (*b - target_x).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(idx, _)| idx)
}

/// Returns `true` when `distance_from_bar_center` falls within `bar_half_width`
/// (`PlanUtilizationHistoryChartMenuView.BarHoverGeometry.accepts`).
pub fn accepts_bar_distance(distance_from_bar_center: f64, bar_half_width: f64) -> bool {
    distance_from_bar_center.is_finite()
        && bar_half_width >= 0.0
        && distance_from_bar_center <= bar_half_width
}

// ---------------------------------------------------------------------------
// 3. Cost History Submenu Chart (`CostHistoryChartMenuView.swift`)
// ---------------------------------------------------------------------------

/// Display mode of the cost history chart (`CostHistoryChartMenuView.ChartMetric`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartMetric {
    Tokens,
    Cost,
}

impl ChartMetric {
    /// Segmented control title (`"Token"` / `"Cost"`).
    pub fn title(self) -> &'static str {
        match self {
            Self::Tokens => "Token",
            Self::Cost => "Cost",
        }
    }
}

/// A model breakdown row under the selected day in the Cost submenu.
#[derive(Debug, Clone, PartialEq)]
pub struct CostHistoryModelRow {
    pub title: String,
    pub subtitle: Option<String>,
    pub accent_opacity: f64,
}

/// A single day point in `CostHistoryChartModel`.
#[derive(Debug, Clone, PartialEq)]
pub struct CostHistoryPoint {
    pub date_key: String,
    pub date_label: String,
    pub cost_usd: Option<f64>,
    pub total_tokens: Option<u64>,
    pub request_count: Option<u64>,
    pub models: Vec<CostHistoryModelRow>,
}

impl CostHistoryPoint {
    /// Metric value for the active chart mode.
    pub fn metric_value(&self, metric: ChartMetric) -> f64 {
        match metric {
            ChartMetric::Tokens => self.total_tokens.unwrap_or(0) as f64,
            ChartMetric::Cost => self.cost_usd.unwrap_or(0.0).max(0.0),
        }
    }
}

/// Complete model for `CostHistoryChartMenuView`.
#[derive(Debug, Clone, PartialEq)]
pub struct CostHistoryChartModel {
    pub provider_id: String,
    pub bar_color: Rgb,
    pub currency_code: String,
    pub available_metrics: Vec<ChartMetric>,
    pub default_metric: ChartMetric,
    pub points: Vec<CostHistoryPoint>,
    pub default_selected_index: Option<usize>,
    pub detail_viewport_row_count: usize,
    pub total_line: Option<String>,
    pub disclaimer_line: Option<String>,
}

/// Computed bar and axis geometry for `CostHistoryChartMenuView`.
#[derive(Debug, Clone, PartialEq)]
pub struct CostBarGeometry {
    pub index: usize,
    pub slot_rect: Rect,
    pub base_rect: Rect,
    pub peak_cap_rect: Option<Rect>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostHistoryChartGeometry {
    pub plot_rect: Rect,
    pub bars: Vec<CostBarGeometry>,
    pub selection_band: Option<Rect>,
    /// `(y_coord, label)` in top-down coordinates.
    pub y_ticks: Vec<(f64, String)>,
    /// `(x_center, label)` for the X-axis start/end dates.
    pub x_labels: Vec<(f64, String)>,
}

impl CostHistoryChartModel {
    /// Builds a `CostHistoryChartModel` from a `ProviderSnapshot` when daily cost history exists.
    pub fn from_provider(provider: &ProviderSnapshot, currency_code: &str) -> Option<Self> {
        let history = provider.cost_history.as_ref().and_then(displayable_value)?;
        if history.daily.is_empty() {
            return None;
        }
        let pid = provider.id.as_str();
        let mut sorted: Vec<&CostDailyEntry> = history.daily.iter().collect();
        sorted.sort_by(|a, b| a.date.cmp(&b.date));

        let has_tokens = sorted.iter().any(|e| e.total_tokens.is_some_and(|t| t > 0));
        let has_cost = sorted.iter().any(|e| e.cost_usd.is_some_and(|c| c >= 0.0));
        let mut available_metrics = Vec::new();
        if has_tokens {
            available_metrics.push(ChartMetric::Tokens);
        }
        if has_cost {
            available_metrics.push(ChartMetric::Cost);
        }
        if available_metrics.is_empty() {
            available_metrics.push(ChartMetric::Cost);
        }

        let default_metric = if pid == "codex" && available_metrics.contains(&ChartMetric::Tokens) {
            ChartMetric::Tokens
        } else if available_metrics.contains(&ChartMetric::Cost) {
            ChartMetric::Cost
        } else {
            available_metrics[0]
        };

        let mut max_models_in_any_day = 0usize;
        let mut points = Vec::with_capacity(sorted.len());
        for entry in &sorted {
            let models = build_model_rows(entry, currency_code);
            max_models_in_any_day = max_models_in_any_day.max(models.len());
            points.push(CostHistoryPoint {
                date_key: entry.date.clone(),
                date_label: format::short_day_key_label(&entry.date),
                cost_usd: entry.cost_usd.filter(|c| *c >= 0.0),
                total_tokens: entry.total_tokens,
                request_count: entry.total_requests,
                models,
            });
        }

        let detail_viewport_row_count = max_models_in_any_day.min(4);
        let default_selected_index = points.len().checked_sub(1);

        let total_cost = history.last_30_days_cost_usd.or_else(|| {
            let sum: f64 = points.iter().filter_map(|p| p.cost_usd).sum();
            (sum > 0.0).then_some(sum)
        });
        let window_label = if points.len() == 1 {
            "Today".to_string()
        } else {
            format!("Last {} days", points.len())
        };
        let total_line = total_cost.map(|c| {
            format!(
                "Est. total ({window_label}): {}",
                format::currency_string(c, currency_code)
            )
        });
        let disclaimer_line = if pid == "codex" {
            Some("Estimated from token usage · not a subscription bill".to_string())
        } else if pid == "claude" {
            Some("Estimated from local usage logs.".to_string())
        } else {
            history.hint_lines.first().cloned()
        };

        Some(Self {
            provider_id: pid.to_string(),
            bar_color: provider_accent_color(provider),
            currency_code: currency_code.to_string(),
            available_metrics,
            default_metric,
            points,
            default_selected_index,
            detail_viewport_row_count,
            total_line,
            disclaimer_line,
        })
    }

    /// Primary detail line for `selected_index` (`CostHistoryChartMenuView.swift:946-969`).
    pub fn detail_primary(&self, selected_index: Option<usize>) -> String {
        let Some(point) = selected_index.and_then(|i| self.points.get(i)) else {
            return "Hover a bar for details".to_string();
        };
        let mut parts = Vec::new();
        if let Some(cost) = point.cost_usd {
            parts.push(format::currency_string(cost, &self.currency_code));
        }
        if let Some(tokens) = point.total_tokens {
            parts.push(format!(
                "{} tokens",
                format::token_count_string(tokens as i64)
            ));
        }
        if let Some(requests) = point.request_count {
            parts.push(format!(
                "{} requests",
                format::token_count_string(requests as i64)
            ));
        }
        if parts.is_empty() {
            parts.push("—".to_string());
        }
        format!("{}: {}", point.date_label, parts.join(" · "))
    }

    /// Model breakdown rows for `selected_index`.
    pub fn detail_rows(&self, selected_index: Option<usize>) -> &[CostHistoryModelRow] {
        selected_index
            .and_then(|i| self.points.get(i))
            .map_or(&[], |p| p.models.as_slice())
    }

    /// Total view height for the menu item (`CostHistoryChartMenuView.swift:257-407`).
    pub fn preferred_height(&self) -> f64 {
        let mut h = SUBMENU_CHART_V_PADDING * 2.0 + SUBMENU_CHART_CANVAS_HEIGHT;
        if self.available_metrics.len() > 1 {
            h += 10.0 + 22.0;
        }
        // Detail block: primary line (16 pt) + rows (36 pt each, 6 pt spacing).
        h += 10.0 + 16.0;
        if self.detail_viewport_row_count > 0 {
            h += 6.0
                + self.detail_viewport_row_count as f64 * 36.0
                + (self.detail_viewport_row_count.saturating_sub(1)) as f64 * 6.0;
        }
        if self.total_line.is_some() {
            h += 10.0 + 16.0;
            if self.disclaimer_line.is_some() {
                h += 2.0 + 14.0;
            }
        }
        h
    }

    /// Computes bar, peak cap, axis, and selection band geometry inside `canvas_rect` (`278 x 130`).
    pub fn geometry(
        &self,
        canvas_rect: Rect,
        metric: ChartMetric,
        selected_index: Option<usize>,
    ) -> CostHistoryChartGeometry {
        let label_left_w = 34.0;
        let label_bottom_h = 18.0;
        let top_pad = 6.0;
        let plot_rect = Rect::new(
            canvas_rect.x + label_left_w,
            canvas_rect.y + top_pad,
            (canvas_rect.w - label_left_w - 4.0).max(1.0),
            (canvas_rect.h - top_pad - label_bottom_h).max(1.0),
        );

        let count = self.points.len();
        let max_value = self
            .points
            .iter()
            .map(|p| p.metric_value(metric))
            .fold(0.0_f64, f64::max);
        let peak_index = if max_value > 0.0 {
            self.points
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| {
                    a.metric_value(metric)
                        .partial_cmp(&b.metric_value(metric))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i)
        } else {
            None
        };

        let mut bars = Vec::with_capacity(count);
        if count > 0 {
            let slot_w = plot_rect.w / count as f64;
            let bar_w = (slot_w * COST_BAR_WIDTH_RATIO).max(2.0);
            for (idx, point) in self.points.iter().enumerate() {
                let slot_x = plot_rect.x + idx as f64 * slot_w;
                let slot_rect = Rect::new(slot_x, plot_rect.y, slot_w, plot_rect.h);
                let val = point.metric_value(metric);
                let ratio = if max_value > 0.0 {
                    (val / max_value).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let total_h = plot_rect.h * ratio;
                let bar_x = slot_x + (slot_w - bar_w) * 0.5;
                let bar_top = plot_rect.max_y() - total_h;

                if peak_index == Some(idx) && total_h > 0.0 {
                    let cap_h = (plot_rect.h * 0.05).min(total_h).max(1.5);
                    let base_h = (total_h - cap_h).max(0.0);
                    bars.push(CostBarGeometry {
                        index: idx,
                        slot_rect,
                        base_rect: Rect::new(bar_x, bar_top + cap_h, bar_w, base_h),
                        peak_cap_rect: Some(Rect::new(bar_x, bar_top, bar_w, cap_h)),
                    });
                } else {
                    bars.push(CostBarGeometry {
                        index: idx,
                        slot_rect,
                        base_rect: Rect::new(bar_x, bar_top, bar_w, total_h),
                        peak_cap_rect: None,
                    });
                }
            }
        }

        let selection_band = selected_index
            .and_then(|idx| bars.get(idx))
            .map(|b| b.slot_rect);

        let y_ticks = if max_value > 0.0 {
            let raw_ticks = match metric {
                ChartMetric::Tokens if max_value < 2.0 => vec![0.0, max_value],
                ChartMetric::Cost if max_value < 1.0 => vec![0.0, max_value],
                _ => vec![0.0, max_value * 0.5, max_value],
            };
            raw_ticks
                .into_iter()
                .map(|v| {
                    let ratio = (v / max_value).clamp(0.0, 1.0);
                    let y = plot_rect.max_y() - plot_rect.h * ratio;
                    let text = match metric {
                        ChartMetric::Tokens => format::token_count_string(v.round() as i64),
                        ChartMetric::Cost => {
                            format::compact_currency_string(v, &self.currency_code)
                        }
                    };
                    (y, text)
                })
                .collect()
        } else {
            Vec::new()
        };

        let mut x_labels = Vec::new();
        if let (Some(first_bar), Some(first_pt)) = (bars.first(), self.points.first()) {
            x_labels.push((first_bar.slot_rect.mid_x(), first_pt.date_label.clone()));
            if bars.len() > 1
                && let (Some(last_bar), Some(last_pt)) = (bars.last(), self.points.last())
                && last_pt.date_key != first_pt.date_key
            {
                x_labels.push((last_bar.slot_rect.mid_x(), last_pt.date_label.clone()));
            }
        }

        CostHistoryChartGeometry {
            plot_rect,
            bars,
            selection_band,
            y_ticks,
            x_labels,
        }
    }
}

impl CostHistoryChartGeometry {
    /// Returns the bar index under `point` in canvas coordinates.
    pub fn hovered_index(&self, point: Point) -> Option<usize> {
        if !self.plot_rect.contains(point) || self.bars.is_empty() {
            return None;
        }
        let centers: Vec<f64> = self.bars.iter().map(|b| b.slot_rect.mid_x()).collect();
        nearest_center_index(&centers, point.x)
    }
}

fn build_model_rows(entry: &CostDailyEntry, currency_code: &str) -> Vec<CostHistoryModelRow> {
    let mut breakdowns = entry.model_breakdowns.clone();
    breakdowns.sort_by(|a, b| {
        let ac = a.cost_usd.unwrap_or(-1.0);
        let bc = b.cost_usd.unwrap_or(-1.0);
        bc.partial_cmp(&ac)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                let at = a.total_tokens.map_or(-1i64, |t| t as i64);
                let bt = b.total_tokens.map_or(-1i64, |t| t as i64);
                bt.cmp(&at)
            })
            .then_with(|| b.model_name.cmp(&a.model_name))
    });
    breakdowns
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let title = format::model_display_name(&item.model_name);
            let subtitle = format::model_cost_detail(
                &item.model_name,
                item.cost_usd,
                item.total_tokens,
                currency_code,
            );
            let accent_opacity = (0.75 - index as f64 * 0.12).max(0.3);
            CostHistoryModelRow {
                title,
                subtitle,
                accent_opacity,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 4. Quota Burndown Submenu Chart (`QuotaBurndownChartMenuView.swift`)
// ---------------------------------------------------------------------------

/// A single point on the observed quota burndown curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuotaBurndownPoint {
    /// Elapsed ratio across `[window_start, window_reset]` in `0.0..=1.0`.
    pub x_ratio: f64,
    /// Remaining quota percentage in `0.0..=100.0`.
    pub remaining_percent: f64,
}

/// One window series in `QuotaBurndownChartModel` (`Session`, `Weekly`, `Sonnet` / `Monthly`).
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaBurndownSeries {
    pub id: String,
    pub title: String,
    pub start_label: String,
    pub reset_label: String,
    pub points: Vec<QuotaBurndownPoint>,
    pub remaining_line: String,
    pub last_known_note: Option<String>,
}

/// Complete model for `QuotaBurndownChartMenuView`.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaBurndownChartModel {
    pub provider_id: String,
    pub accent_color: Rgb,
    pub series: Vec<QuotaBurndownSeries>,
    pub default_series_index: usize,
}

/// Computed geometry for `QuotaBurndownChartMenuView`.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaBurndownChartGeometry {
    pub plot_rect: Rect,
    pub ideal_start: Point,
    pub ideal_end: Point,
    pub observed_points: Vec<Point>,
    pub endpoint: Option<Point>,
    /// `(y_coord, label)` on the trailing Y-axis (`100`, `50`, `0`).
    pub y_ticks: [(f64, &'static str); 3],
}

impl QuotaBurndownChartModel {
    /// Builds a `QuotaBurndownChartModel` from a `ProviderSnapshot`.
    pub fn from_provider(
        provider: &ProviderSnapshot,
        now: Timestamp,
        dates: &dyn DateTexts,
    ) -> Option<Self> {
        let util_series_list = provider.utilization.as_ref().and_then(displayable_value)?;
        let pid = provider.id.as_str();
        let ref_now = provider.updated_at.unwrap_or(now);

        let find_series = |name: &str| util_series_list.iter().find(|s| s.name == name);
        let opus_or_monthly = find_series("opus").or_else(|| find_series("monthly"));
        let tertiary_default_title = if pid == "claude" { "Sonnet" } else { "Monthly" };

        let mut series = Vec::new();
        type BurndownCandidate<'a> = (
            &'a str,
            &'a str,
            Option<&'a ProtocolUtilizationSeries>,
            Option<&'a RateWindow>,
            u32,
        );
        let candidates: [BurndownCandidate<'_>; 3] = [
            (
                "session",
                "Session",
                find_series("session"),
                provider
                    .windows
                    .primary
                    .as_ref()
                    .and_then(displayable_value),
                300,
            ),
            (
                "weekly",
                "Weekly",
                find_series("weekly"),
                provider
                    .windows
                    .secondary
                    .as_ref()
                    .and_then(displayable_value),
                10_080,
            ),
            (
                "opus",
                tertiary_default_title,
                opus_or_monthly,
                provider
                    .windows
                    .tertiary
                    .as_ref()
                    .and_then(displayable_value),
                10_080,
            ),
        ];

        for (id, default_title, util_series, rate_window, fallback_mins) in candidates {
            let Some(u) = util_series else { continue };
            if u.entries.is_empty() {
                continue;
            }
            let title = u
                .title
                .as_deref()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or(default_title);
            if let Some(built) =
                build_burndown_series(id, title, u, rate_window, fallback_mins, ref_now, dates)
            {
                series.push(built);
            }
        }

        if series.is_empty() {
            return None;
        }

        Some(Self {
            provider_id: pid.to_string(),
            accent_color: provider_accent_color(provider),
            series,
            default_series_index: 0,
        })
    }

    /// Preferred height for the menu item (`QuotaBurndownChartMenuView.swift`).
    pub fn preferred_height(&self, series_index: usize) -> f64 {
        let mut h = SUBMENU_CHART_V_PADDING * 2.0 + SUBMENU_CHART_CANVAS_HEIGHT;
        if self.series.len() > 1 {
            h += 10.0 + 22.0;
        }
        // Start/reset edge labels row (14 pt) + remaining line (16 pt)
        h += 6.0 + 14.0 + 8.0 + 16.0;
        if self
            .series
            .get(series_index)
            .is_some_and(|s| s.last_known_note.is_some())
        {
            h += 2.0 + 14.0;
        }
        h
    }

    /// Computes ideal and observed lines inside `canvas_rect` (`278 x 130`).
    pub fn geometry(&self, canvas_rect: Rect, series_index: usize) -> QuotaBurndownChartGeometry {
        let trailing_axis_w = 28.0;
        let top_pad = 6.0;
        let bottom_pad = 6.0;
        let plot_rect = Rect::new(
            canvas_rect.x + 2.0,
            canvas_rect.y + top_pad,
            (canvas_rect.w - trailing_axis_w - 2.0).max(1.0),
            (canvas_rect.h - top_pad - bottom_pad).max(1.0),
        );

        let ideal_start = Point::new(plot_rect.x, plot_rect.y);
        let ideal_end = Point::new(plot_rect.max_x(), plot_rect.max_y());

        let mut observed_points = Vec::new();
        if let Some(series) = self.series.get(series_index) {
            for pt in &series.points {
                let x = plot_rect.x + plot_rect.w * pt.x_ratio.clamp(0.0, 1.0);
                let y = plot_rect.max_y()
                    - plot_rect.h * (pt.remaining_percent.clamp(0.0, 100.0) / 100.0);
                observed_points.push(Point::new(x, y));
            }
        }
        let endpoint = observed_points.last().copied();
        let y_ticks = [
            (plot_rect.y, "100"),
            (plot_rect.y + plot_rect.h * 0.5, "50"),
            (plot_rect.max_y(), "0"),
        ];

        QuotaBurndownChartGeometry {
            plot_rect,
            ideal_start,
            ideal_end,
            observed_points,
            endpoint,
            y_ticks,
        }
    }
}

fn build_burndown_series(
    id: &str,
    title: &str,
    u: &ProtocolUtilizationSeries,
    rate_window: Option<&RateWindow>,
    fallback_mins: u32,
    ref_now: Timestamp,
    dates: &dyn DateTexts,
) -> Option<QuotaBurndownSeries> {
    let mut entries = u.entries.clone();
    entries.sort_by_key(|e| e.captured_at);
    let latest = entries.last()?;

    let duration_mins = if u.window_minutes > 0 {
        Some(u.window_minutes)
    } else {
        rate_window.and_then(|w| w.window_minutes)
    }
    .unwrap_or(fallback_mins)
    .max(1);
    let duration_secs = i64::from(duration_mins) * 60;

    let reset_ts = rate_window
        .and_then(|w| w.resets_at)
        .or(latest.resets_at)
        .unwrap_or_else(|| {
            Timestamp::from_second(latest.captured_at.as_second() + duration_secs / 2)
                .unwrap_or(latest.captured_at)
        });
    let start_ts = Timestamp::from_second(reset_ts.as_second() - duration_secs).unwrap_or(reset_ts);
    let span_secs = (reset_ts.as_second() - start_ts.as_second()).max(1) as f64;

    let mut points = Vec::new();
    // Anchor at window start (100% remaining) when no entry precedes `start_ts`.
    points.push(QuotaBurndownPoint {
        x_ratio: 0.0,
        remaining_percent: 100.0,
    });
    for entry in &entries {
        if entry.captured_at < start_ts || entry.captured_at > reset_ts {
            continue;
        }
        let x_ratio = ((entry.captured_at.as_second() - start_ts.as_second()) as f64 / span_secs)
            .clamp(0.0, 1.0);
        let remaining = (100.0 - entry.used_percent).clamp(0.0, 100.0);
        points.push(QuotaBurndownPoint {
            x_ratio,
            remaining_percent: remaining,
        });
    }
    if points.len() == 1 {
        // Fallback if all entries were outside `[start_ts, reset_ts]`: map the latest entry into the window.
        let remaining = (100.0 - latest.used_percent).clamp(0.0, 100.0);
        points.push(QuotaBurndownPoint {
            x_ratio: 0.65,
            remaining_percent: remaining,
        });
    }

    let latest_remaining = points
        .last()
        .map_or(0, |p| p.remaining_percent.round() as i64);
    let remaining_line = format!("{latest_remaining}% Usage remaining");

    let start_label = format_range_edge(start_ts, ref_now, dates);
    let reset_label = format_range_edge(reset_ts, ref_now, dates);
    let elapsed_since_capture = (ref_now.as_second() - latest.captured_at.as_second()).abs();
    let last_known_note = (elapsed_since_capture >= 120).then(|| {
        format!(
            "Showing last-known usage captured {}.",
            format_range_edge(latest.captured_at, ref_now, dates)
        )
    });

    Some(QuotaBurndownSeries {
        id: id.to_string(),
        title: title.to_string(),
        start_label,
        reset_label,
        points,
        remaining_line,
        last_known_note,
    })
}

fn format_range_edge(date: Timestamp, reference: Timestamp, dates: &dyn DateTexts) -> String {
    let tz = dates.time_zone();
    if date.to_zoned(tz.clone()).date() == reference.to_zoned(tz).date() {
        dates.hour_minute(date)
    } else {
        dates.month_day_hour_minute(date)
    }
}

// ---------------------------------------------------------------------------
// 5. Plan Utilization History Submenu Chart (`PlanUtilizationHistoryChartMenuView.swift`)
// ---------------------------------------------------------------------------

/// A single bar point in `PlanUtilizationSeries`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanUtilizationBarPoint {
    pub date_label: String,
    pub detail_timestamp_label: String,
    pub used_percent: f64,
}

/// One window series in `PlanUtilizationChartModel`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanUtilizationSeries {
    pub id: String,
    pub title: String,
    pub bars: Vec<PlanUtilizationBarPoint>,
    /// `(bar_index, date_label)` for up to 4 proportional X-axis labels.
    pub x_axis_labels: Vec<(usize, String)>,
}

/// Complete model for `PlanUtilizationHistoryChartMenuView`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanUtilizationChartModel {
    pub provider_id: String,
    pub accent_color: Rgb,
    pub series: Vec<PlanUtilizationSeries>,
    pub default_series_index: usize,
}

/// Geometry of a single 6 pt capacity-track + fill bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanUtilizationBarGeometry {
    pub index: usize,
    pub center_x: f64,
    pub track_rect: Rect,
    pub fill_rect: Rect,
}

/// Computed geometry for `PlanUtilizationHistoryChartMenuView`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanUtilizationChartGeometry {
    pub plot_rect: Rect,
    pub bars: Vec<PlanUtilizationBarGeometry>,
    pub hover_rule_x: Option<f64>,
    pub y_ticks: [(f64, &'static str); 3],
    pub x_labels: Vec<(f64, String)>,
}

impl PlanUtilizationChartModel {
    /// Builds a `PlanUtilizationChartModel` from a `ProviderSnapshot`.
    pub fn from_provider(provider: &ProviderSnapshot, dates: &dyn DateTexts) -> Option<Self> {
        let util_series_list = provider.utilization.as_ref().and_then(displayable_value)?;
        let pid = provider.id.as_str();
        let find_series = |name: &str| util_series_list.iter().find(|s| s.name == name);
        let opus_or_monthly = find_series("opus").or_else(|| find_series("monthly"));
        let tertiary_default_title = if pid == "claude" { "Sonnet" } else { "Monthly" };

        let mut series = Vec::new();
        let candidates: [(&str, &str, Option<&ProtocolUtilizationSeries>); 3] = [
            ("session", "Session", find_series("session")),
            ("weekly", "Weekly", find_series("weekly")),
            ("opus", tertiary_default_title, opus_or_monthly),
        ];

        for (id, default_title, util_series) in candidates {
            let Some(u) = util_series else { continue };
            if u.entries.is_empty() {
                continue;
            }
            let title = u
                .title
                .as_deref()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or(default_title);
            let mut entries = u.entries.clone();
            entries.sort_by_key(|e| e.captured_at);
            if entries.len() > 30 {
                entries = entries.split_off(entries.len() - 30);
            }

            let tz = dates.time_zone();
            let mut seen_days = std::collections::HashSet::new();
            let mut has_same_day_duplicates = false;
            for entry in &entries {
                let d = entry.captured_at.to_zoned(tz.clone()).date();
                if !seen_days.insert(d) {
                    has_same_day_duplicates = true;
                    break;
                }
            }

            let bars: Vec<PlanUtilizationBarPoint> = entries
                .iter()
                .map(|e| {
                    let date_label = format::month_day_short(e.captured_at, tz.clone());
                    let detail_timestamp_label = if has_same_day_duplicates {
                        dates.month_day_hour_minute(e.captured_at)
                    } else {
                        date_label.clone()
                    };
                    PlanUtilizationBarPoint {
                        date_label,
                        detail_timestamp_label,
                        used_percent: e.used_percent.clamp(0.0, 100.0),
                    }
                })
                .collect();

            let x_axis_labels = proportional_axis_labels(&bars);
            series.push(PlanUtilizationSeries {
                id: id.to_string(),
                title: title.to_string(),
                bars,
                x_axis_labels,
            });
        }

        if series.is_empty() {
            return None;
        }

        Some(Self {
            provider_id: pid.to_string(),
            accent_color: provider_accent_color(provider),
            series,
            default_series_index: 0,
        })
    }

    /// Detail text below the chart (`"<Date>: <N>% used"`).
    pub fn detail_text(&self, series_index: usize, selected_bar_index: Option<usize>) -> String {
        let Some(series) = self.series.get(series_index) else {
            return "—".to_string();
        };
        let bar = selected_bar_index
            .and_then(|i| series.bars.get(i))
            .or_else(|| series.bars.last());
        let Some(bar) = bar else {
            return "—".to_string();
        };
        format!(
            "{}: {}% used",
            bar.detail_timestamp_label,
            bar.used_percent.round() as i64
        )
    }

    /// Preferred height for the menu item (`PlanUtilizationHistoryChartMenuView.swift`).
    pub fn preferred_height(&self) -> f64 {
        let mut h = SUBMENU_CHART_V_PADDING * 2.0 + SUBMENU_CHART_CANVAS_HEIGHT;
        if self.series.len() > 1 {
            h += 10.0 + 22.0;
        }
        // Detail line (16 pt)
        h += 10.0 + 16.0;
        h
    }

    /// Computes fixed 6 pt bars, axis ticks, and hover rule inside `canvas_rect` (`278 x 130`).
    pub fn geometry(
        &self,
        canvas_rect: Rect,
        series_index: usize,
        selected_bar_index: Option<usize>,
    ) -> PlanUtilizationChartGeometry {
        let trailing_axis_w = 28.0;
        let top_pad = 6.0;
        let bottom_label_h = 18.0;
        let plot_rect = Rect::new(
            canvas_rect.x + 2.0,
            canvas_rect.y + top_pad,
            (canvas_rect.w - trailing_axis_w - 2.0).max(1.0),
            (canvas_rect.h - top_pad - bottom_label_h).max(1.0),
        );

        let mut bars = Vec::new();
        let mut x_labels = Vec::new();
        if let Some(series) = self.series.get(series_index) {
            let count = series.bars.len();
            if count > 0 {
                let slot_w = plot_rect.w / count as f64;
                for (idx, pt) in series.bars.iter().enumerate() {
                    let center_x = plot_rect.x + (idx as f64 + 0.5) * slot_w;
                    let bar_x = center_x - PLAN_UTILIZATION_BAR_HALF_WIDTH;
                    let track_rect =
                        Rect::new(bar_x, plot_rect.y, PLAN_UTILIZATION_BAR_WIDTH, plot_rect.h);
                    let fill_h = plot_rect.h * (pt.used_percent.clamp(0.0, 100.0) / 100.0);
                    let fill_rect = Rect::new(
                        bar_x,
                        plot_rect.max_y() - fill_h,
                        PLAN_UTILIZATION_BAR_WIDTH,
                        fill_h,
                    );
                    bars.push(PlanUtilizationBarGeometry {
                        index: idx,
                        center_x,
                        track_rect,
                        fill_rect,
                    });
                }
                for (bar_idx, label) in &series.x_axis_labels {
                    if let Some(b) = bars.get(*bar_idx) {
                        x_labels.push((b.center_x, label.clone()));
                    }
                }
            }
        }

        let hover_rule_x = selected_bar_index
            .and_then(|i| bars.get(i))
            .map(|b| b.center_x);
        let y_ticks = [
            (plot_rect.y, "100"),
            (plot_rect.y + plot_rect.h * 0.5, "50"),
            (plot_rect.max_y(), "0"),
        ];

        PlanUtilizationChartGeometry {
            plot_rect,
            bars,
            hover_rule_x,
            y_ticks,
            x_labels,
        }
    }
}

impl PlanUtilizationChartGeometry {
    /// Hit-tests `point` against the 6 pt bars (`accepts_bar_distance(dist, 3.0)`).
    pub fn hovered_index(&self, point: Point) -> Option<usize> {
        if !self.plot_rect.contains(point) || self.bars.is_empty() {
            return None;
        }
        let centers: Vec<f64> = self.bars.iter().map(|b| b.center_x).collect();
        let idx = nearest_center_index(&centers, point.x)?;
        let dist = (centers[idx] - point.x).abs();
        accepts_bar_distance(dist, PLAN_UTILIZATION_BAR_HALF_WIDTH).then_some(idx)
    }
}

fn proportional_axis_labels(bars: &[PlanUtilizationBarPoint]) -> Vec<(usize, String)> {
    let count = bars.len();
    if count == 0 {
        return Vec::new();
    }
    if count <= 4 {
        return bars
            .iter()
            .enumerate()
            .map(|(i, b)| (i, b.date_label.clone()))
            .collect();
    }
    let last = count - 1;
    let mut indexes = vec![
        0,
        (last as f64 / 3.0).round() as usize,
        (last as f64 * 2.0 / 3.0).round() as usize,
        last,
    ];
    indexes.dedup();
    indexes
        .into_iter()
        .filter_map(|i| bars.get(i).map(|b| (i, b.date_label.clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ariadusage_protocol::fixture::generate_scenario_snapshot;
    use jiff::tz::TimeZone;

    struct FixedDates;
    impl DateTexts for FixedDates {
        fn time_zone(&self) -> TimeZone {
            TimeZone::UTC
        }
        fn hour_minute(&self, date: Timestamp) -> String {
            let z = date.to_zoned(TimeZone::UTC);
            format!("{:02}:{:02}", z.hour(), z.minute())
        }
        fn month_day_hour_minute(&self, date: Timestamp) -> String {
            let z = date.to_zoned(TimeZone::UTC);
            format!("Oct {} {:02}:{:02}", z.day(), z.hour(), z.minute())
        }
        fn relative_abbreviated(&self, _date: Timestamp, _now: Timestamp) -> String {
            "5m ago".to_string()
        }
    }

    #[test]
    fn mini_bar_strip_geometry_and_hover_hit_testing() {
        let points: Vec<MiniBarPoint> = (0..10)
            .map(|i| MiniBarPoint {
                day_key: format!("2026-10-{:02}", i + 1),
                value: Some((i + 1) as f64 * 10.0),
                hover_summary: Some(format!("Oct {}: ${}", i + 1, (i + 1) * 10)),
            })
            .collect();
        let rect = Rect::new(20.0, 100.0, 270.0, MINI_BAR_STRIP_HEIGHT);
        let geom = MiniBarStripGeometry::compute(rect, &points, Some(9));
        assert_eq!(geom.baseline, Rect::new(20.0, 143.0, 270.0, 1.0));
        assert_eq!(geom.bars.len(), 10);
        // Max bar has full 42 pt chart height and hovered alpha 0.95.
        let last = geom.bars[9];
        assert!((last.rect.h - 42.0).abs() < 1e-6);
        assert!((last.alpha - 0.95).abs() < 1e-6);
        // Hover hit-test at the start and end of the strip.
        assert_eq!(
            MiniBarStripGeometry::hovered_index(1.0, 10.0, 270.0, MINI_BAR_STRIP_HEIGHT, 10),
            Some(0)
        );
        assert_eq!(
            MiniBarStripGeometry::hovered_index(269.0, 10.0, 270.0, MINI_BAR_STRIP_HEIGHT, 10),
            Some(9)
        );
        assert_eq!(
            MiniBarStripGeometry::hovered_index(-1.0, 10.0, 270.0, MINI_BAR_STRIP_HEIGHT, 10),
            None
        );
    }

    #[test]
    fn cost_history_chart_model_defaults_and_peak_cap() {
        let snap = generate_scenario_snapshot(0);
        let codex = snap
            .providers
            .iter()
            .find(|p| p.id.as_str() == "codex")
            .unwrap();
        let claude = snap
            .providers
            .iter()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();

        let codex_model = CostHistoryChartModel::from_provider(codex, "USD").unwrap();
        assert_eq!(codex_model.default_metric, ChartMetric::Tokens);
        assert_eq!(codex_model.points.len(), 30);
        assert!(
            codex_model
                .detail_primary(codex_model.default_selected_index)
                .starts_with("Oct 6:")
        );
        let codex_geom = codex_model.geometry(
            Rect::new(0.0, 0.0, 278.0, SUBMENU_CHART_CANVAS_HEIGHT),
            ChartMetric::Tokens,
            codex_model.default_selected_index,
        );
        assert_eq!(
            codex_geom
                .bars
                .iter()
                .filter(|b| b.peak_cap_rect.is_some())
                .count(),
            1
        );

        let claude_model = CostHistoryChartModel::from_provider(claude, "USD").unwrap();
        assert_eq!(claude_model.default_metric, ChartMetric::Cost);
    }

    #[test]
    fn quota_burndown_and_plan_utilization_models_build_from_fixture() {
        let snap = generate_scenario_snapshot(0);
        let claude = snap
            .providers
            .iter()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();
        let now: Timestamp = "2026-10-06T12:53:00Z".parse().unwrap();

        let burndown = QuotaBurndownChartModel::from_provider(claude, now, &FixedDates).unwrap();
        assert_eq!(burndown.series.len(), 3);
        assert_eq!(burndown.series[0].title, "Session");
        assert_eq!(burndown.series[1].title, "Weekly");
        assert_eq!(burndown.series[2].title, "Sonnet");

        let util = PlanUtilizationChartModel::from_provider(claude, &FixedDates).unwrap();
        assert_eq!(util.series.len(), 3);
        let geom = util.geometry(
            Rect::new(0.0, 0.0, 278.0, SUBMENU_CHART_CANVAS_HEIGHT),
            0,
            Some(0),
        );
        assert!(!geom.bars.is_empty());
        let first_center = geom.bars[0].center_x;
        let mid_y = geom.plot_rect.y + geom.plot_rect.h * 0.5;
        assert_eq!(
            geom.hovered_index(Point::new(first_center + 2.0, mid_y)),
            Some(0)
        );
        assert_eq!(
            geom.hovered_index(Point::new(first_center + 4.0, mid_y)),
            None
        );
    }
}
