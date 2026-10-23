// Ported from CodexBar Sources/CodexBar/UsageMenuCardLayout.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuCardMetricRow.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageMenuCardHeaderAndUsageSectionView.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuTypes.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+OverviewSpend.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Manual layout of the usage card, the AppKit version of CodexBar's
//! SwiftUI stacks in `MenuCardView.swift`.
//!
//! Text sizes come from a [`TextMeasure`], so the AppKit bridge measures
//! with real fonts while tests use fixed metrics. The result lists every
//! label, bar and divider with its frame in a flipped coordinate space.

use crate::card::{CardModel, CreditsSection, MetricRow, SubtitleStyle};
use crate::geometry::{Rect, Size};
use crate::highlight_style::{self, Color, ColorRole};
use crate::menu_descriptor::OverviewSpendSummary;
use crate::progress_bar::{BAR_HEIGHT, BarSpec};

/// The card is never narrower than the menu's base width
/// (`StatusItemController.menuCardBaseWidth`).
pub const MIN_CARD_WIDTH: f64 = 310.0;
pub const HORIZONTAL_PADDING: f64 = 20.0;
pub const HEADER_ONLY_VERTICAL_PADDING: f64 = 6.0;
pub const HEADER_CONTENT_SPACING: f64 = 6.0;
pub const SECTION_TOP_PADDING: f64 = 6.0;
pub const USAGE_SECTION_TOP_PADDING: f64 = 10.0;
pub const SECTION_BOTTOM_PADDING: f64 = 6.0;
pub const HEADER_LINE_SPACING: f64 = 4.0;
pub const HEADER_COLUMN_SPACING: f64 = 12.0;
/// Spacing between sections and between metric rows (`VStack(spacing: 12)`).
pub const SECTION_SPACING: f64 = 12.0;
/// Spacing inside a metric row and the credits block (`VStack(spacing: 6)`).
pub const ROW_SPACING: f64 = 6.0;
/// `HStack(spacing: 8)` with `Spacer(minLength: 8)` between title and reset.
pub const RESET_MIN_GAP: f64 = 8.0 + 8.0 + 8.0;
/// Spacing of the stacked title and reset when they do not fit side by side.
pub const RESET_VERTICAL_SPACING: f64 = 2.0;
/// The error subtitle reserves 4 pt below its text.
pub const ERROR_SUBTITLE_BOTTOM_PADDING: f64 = 4.0;
pub const DIVIDER_THICKNESS: f64 = 1.0;
/// Default `HStack` spacing between the credits value and its scale.
pub const CREDITS_ROW_SPACING: f64 = 8.0;
/// Vertical padding of `OverviewSpendSummaryCardView` (`StatusItemController+OverviewSpend.swift:54`).
pub const SPEND_SUMMARY_VERTICAL_PADDING: f64 = 10.0;
/// Vertical spacing of `OverviewSpendSummaryCardView` (`VStack(alignment: .leading, spacing: 7)`).
pub const SPEND_SUMMARY_ROW_SPACING: f64 = 7.0;
/// Horizontal spacing of the `Usage & Spend · 30d` header row (`HStack(spacing: 6)`).
pub const SPEND_SUMMARY_HEADER_SPACING: f64 = 6.0;
/// Horizontal spacing of the `providerCoverage · tokenText` row (`HStack(spacing: 8)`).
pub const SPEND_SUMMARY_META_SPACING: f64 = 8.0;

/// The width the card renders at for a menu of `menu_width`.
pub fn rendered_width(menu_width: f64) -> f64 {
    menu_width.ceil().max(MIN_CARD_WIDTH)
}

/// macOS text styles used by the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextStyle {
    Title2,
    Headline,
    Subheadline,
    Body,
    Footnote,
    Caption,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Font {
    pub style: TextStyle,
    pub weight: Weight,
}

impl Font {
    pub const fn new(style: TextStyle, weight: Weight) -> Self {
        Self { style, weight }
    }
}

const NAME_FONT: Font = Font::new(TextStyle::Headline, Weight::Semibold);
const EMAIL_FONT: Font = Font::new(TextStyle::Subheadline, Weight::Regular);
const FOOTNOTE: Font = Font::new(TextStyle::Footnote, Weight::Regular);
const TITLE_FONT: Font = Font::new(TextStyle::Body, Weight::Medium);
const CAPTION: Font = Font::new(TextStyle::Caption, Weight::Regular);
const PLACEHOLDER_FONT: Font = Font::new(TextStyle::Subheadline, Weight::Regular);

/// Text color by meaning; resolved per highlight state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextColor {
    Primary,
    Secondary,
    Error,
}

impl TextColor {
    pub fn resolve(self, highlighted: bool) -> Color {
        match self {
            TextColor::Primary => highlight_style::primary(highlighted),
            TextColor::Secondary => highlight_style::secondary(highlighted),
            TextColor::Error => highlight_style::error(highlighted),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truncation {
    Tail,
    Middle,
}

/// A label.
#[derive(Debug, Clone, PartialEq)]
pub struct TextItem {
    pub text: String,
    pub font: Font,
    pub color: TextColor,
    /// Maximum lines, or 0 for no limit.
    pub max_lines: usize,
    pub align: Align,
    pub truncation: Truncation,
}

/// Something drawn on the card.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Text(TextItem),
    Bar {
        spec: BarSpec,
        accessibility_label: String,
    },
    Divider,
}

/// An item and its frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub item: Item,
    pub frame: Rect,
}

/// The laid-out card.
#[derive(Debug, Clone, PartialEq)]
pub struct CardLayout {
    pub size: Size,
    pub items: Vec<Placed>,
}

/// Measures text with real fonts.
pub trait TextMeasure {
    /// The size of `text` wrapped to at most `max_lines` lines (0 for no
    /// limit) within `max_width`. The width is the natural width when the
    /// text fits on one line.
    fn measure(&self, text: &str, font: Font, max_lines: usize, max_width: f64) -> Size;
    /// Distance from the top of a label to its first baseline.
    fn first_baseline(&self, font: Font) -> f64;
}

/// The divider color.
pub fn divider_color() -> Color {
    Color::opaque(ColorRole::Separator)
}

struct Builder<'a> {
    measure: &'a dyn TextMeasure,
    x: f64,
    width: f64,
    y: f64,
    items: Vec<Placed>,
}

impl Builder<'_> {
    fn size(&self, item: &TextItem, max_width: f64) -> Size {
        self.measure
            .measure(&item.text, item.font, item.max_lines, max_width.max(0.0))
    }

    fn push_text(&mut self, item: TextItem, frame: Rect) {
        self.items.push(Placed {
            item: Item::Text(item),
            frame,
        });
    }

    /// A full-width left-aligned label at the current position.
    fn text_line(&mut self, item: TextItem) {
        let size = self.size(&item, self.width);
        self.push_text(item, Rect::new(self.x, self.y, size.w, size.h));
        self.y += size.h;
    }

    /// A left and a right label on one row, aligned on their first
    /// baselines. The right label takes its width first when it has layout
    /// priority; otherwise the left one does.
    fn baseline_row(
        &mut self,
        left: TextItem,
        right: Option<TextItem>,
        right_first: bool,
        spacing: f64,
        top_aligned: bool,
        left_extra_height: f64,
    ) {
        let Some(right) = right else {
            let size = self.size(&left, self.width);
            self.push_text(left, Rect::new(self.x, self.y, size.w, size.h));
            self.y += size.h + left_extra_height;
            return;
        };
        let (left_size, right_size) = if right_first {
            let r = self.size(&right, self.width);
            let l = self.size(&left, self.width - r.w - spacing);
            (l, r)
        } else {
            let l = self.size(&left, self.width);
            let r = self.size(&right, self.width - l.w - spacing);
            (l, r)
        };
        let (left_y, right_y) = if top_aligned {
            (self.y, self.y)
        } else {
            let lb = self.measure.first_baseline(left.font);
            let rb = self.measure.first_baseline(right.font);
            let base = lb.max(rb);
            (self.y + base - lb, self.y + base - rb)
        };
        let bottom = (left_y + left_size.h + left_extra_height).max(right_y + right_size.h);
        let right_x = self.x + self.width - right_size.w;
        self.push_text(left, Rect::new(self.x, left_y, left_size.w, left_size.h));
        if right_size.w > 0.0 {
            self.push_text(
                right,
                Rect::new(right_x, right_y, right_size.w, right_size.h),
            );
        }
        self.y = bottom;
    }

    fn divider(&mut self) {
        self.items.push(Placed {
            item: Item::Divider,
            frame: Rect::new(self.x, self.y, self.width, DIVIDER_THICKNESS),
        });
        self.y += DIVIDER_THICKNESS;
    }

    fn bar(&mut self, spec: BarSpec, accessibility_label: &str) {
        self.items.push(Placed {
            item: Item::Bar {
                spec,
                accessibility_label: accessibility_label.to_string(),
            },
            frame: Rect::new(self.x, self.y, self.width, BAR_HEIGHT),
        });
        self.y += BAR_HEIGHT;
    }

    fn header(&mut self, model: &CardModel) {
        let name = label(&model.provider_name, NAME_FONT, TextColor::Primary, 1);
        let email = (!model.email.is_empty()).then(|| TextItem {
            truncation: Truncation::Middle,
            align: Align::Right,
            ..label(&model.email, EMAIL_FONT, TextColor::Secondary, 1)
        });
        self.baseline_row(name, email, false, HEADER_COLUMN_SPACING, false, 0.0);
        self.y += HEADER_LINE_SPACING;

        let is_error = model.subtitle.style == SubtitleStyle::Error;
        let subtitle_color = match model.subtitle.style {
            SubtitleStyle::Info | SubtitleStyle::Loading => TextColor::Secondary,
            SubtitleStyle::Error => TextColor::Error,
        };
        let subtitle = label(
            &model.subtitle.text,
            FOOTNOTE,
            subtitle_color,
            if is_error { 3 } else { 1 },
        );
        let plan = model.plan.as_ref().map(|plan| TextItem {
            align: Align::Right,
            ..label(plan, FOOTNOTE, TextColor::Secondary, 1)
        });
        self.baseline_row(
            subtitle,
            plan,
            true,
            HEADER_COLUMN_SPACING,
            is_error,
            if is_error {
                ERROR_SUBTITLE_BOTTOM_PADDING
            } else {
                0.0
            },
        );
    }

    fn metric_row(&mut self, row: &MetricRow) {
        if let Some(status) = &row.status_text {
            self.text_line(label(&row.title, TITLE_FONT, TextColor::Primary, 1));
            self.y += ROW_SPACING;
            self.text_line(label(status, FOOTNOTE, TextColor::Secondary, 1));
            return;
        }
        let title = label(&row.title_text(), TITLE_FONT, TextColor::Primary, 1);
        match &row.reset_text {
            Some(reset) => self.metric_header(title, reset),
            None => self.text_line(title),
        }
        self.y += ROW_SPACING;
        self.bar(row.bar(), row.percent_style.accessibility_label());
        if let Some(meta) = row.meta_text() {
            self.y += ROW_SPACING;
            self.text_line(label(&meta, FOOTNOTE, TextColor::Secondary, 2));
        }
        if let Some(detail) = &row.detail_text {
            self.y += ROW_SPACING;
            self.text_line(label(detail, FOOTNOTE, TextColor::Secondary, 1));
        }
    }

    /// `MetricRowHeader`: title and reset side by side when both fit at
    /// their natural widths, otherwise stacked with the reset trailing.
    fn metric_header(&mut self, title: TextItem, reset: &str) {
        let reset = TextItem {
            align: Align::Right,
            ..label(reset, FOOTNOTE, TextColor::Secondary, 2)
        };
        let title_ideal = self
            .measure
            .measure(&title.text, title.font, 1, f64::INFINITY);
        let reset_ideal = self
            .measure
            .measure(&reset.text, reset.font, 1, f64::INFINITY);
        if title_ideal.w + RESET_MIN_GAP + reset_ideal.w <= self.width {
            self.baseline_row(title, Some(reset), false, RESET_MIN_GAP, false, 0.0);
        } else {
            self.text_line(title);
            self.y += RESET_VERTICAL_SPACING;
            let size = self.size(&reset, self.width);
            let frame = Rect::new(self.x + self.width - size.w, self.y, size.w, size.h);
            self.push_text(reset, frame);
            self.y += size.h;
        }
    }

    fn credits(&mut self, credits: &CreditsSection) {
        self.text_line(label("Credits", TITLE_FONT, TextColor::Primary, 1));
        self.y += ROW_SPACING;
        match (credits.show_progress, credits.progress_percent) {
            (true, Some(percent)) => {
                self.bar(
                    BarSpec {
                        percent: percent.clamp(0.0, 100.0),
                        pace_on_top: true,
                        ..BarSpec::default()
                    },
                    "Credits remaining",
                );
                self.y += ROW_SPACING;
                let text = label(&credits.text, CAPTION, TextColor::Primary, 1);
                let scale = credits.scale_text.as_ref().map(|s| TextItem {
                    align: Align::Right,
                    ..label(s, CAPTION, TextColor::Secondary, 1)
                });
                self.baseline_row(text, scale, false, CREDITS_ROW_SPACING, false, 0.0);
            }
            _ => self.text_line(label(&credits.text, CAPTION, TextColor::Primary, 0)),
        }
    }
}

fn label(text: &str, font: Font, color: TextColor, max_lines: usize) -> TextItem {
    TextItem {
        text: text.to_string(),
        font,
        color,
        max_lines,
        align: Align::Left,
        truncation: Truncation::Tail,
    }
}

/// Lays out `model` in a card of `width` points.
pub fn layout(model: &CardModel, width: f64, measure: &dyn TextMeasure) -> CardLayout {
    let has_details = model.has_details();
    let stacked = model.uses_stacked_detail_layout();
    let has_usage = model.has_usage_content();
    let mut b = Builder {
        measure,
        x: HORIZONTAL_PADDING,
        width: (width - 2.0 * HORIZONTAL_PADDING).max(0.0),
        y: if has_details {
            SECTION_TOP_PADDING
        } else {
            HEADER_ONLY_VERTICAL_PADDING
        },
        items: Vec::new(),
    };

    b.header(model);

    if has_details {
        b.y += HEADER_CONTENT_SPACING;
        b.divider();
        b.y += if stacked && has_usage {
            SECTION_BOTTOM_PADDING + USAGE_SECTION_TOP_PADDING
        } else {
            SECTION_BOTTOM_PADDING
        };
    }

    if stacked {
        let mut first_section = true;
        let mut next_section = |b: &mut Builder| {
            if !first_section {
                b.y += SECTION_SPACING;
            }
            first_section = false;
        };
        if has_usage {
            next_section(&mut b);
            for (index, row) in model.metrics.iter().enumerate() {
                if index > 0 {
                    b.y += SECTION_SPACING;
                }
                b.metric_row(row);
            }
            if model.metrics.is_empty()
                && let Some(placeholder) = &model.placeholder
            {
                b.text_line(label(
                    placeholder,
                    PLACEHOLDER_FONT,
                    TextColor::Secondary,
                    0,
                ));
            }
        }
        if let Some(credits) = &model.credits {
            if has_usage {
                next_section(&mut b);
                b.divider();
            }
            next_section(&mut b);
            b.credits(credits);
        }
    } else if let Some(placeholder) = &model.placeholder {
        b.text_line(label(
            placeholder,
            PLACEHOLDER_FONT,
            TextColor::Secondary,
            0,
        ));
    }

    b.y += if has_details {
        SECTION_BOTTOM_PADDING
    } else {
        HEADER_ONLY_VERTICAL_PADDING
    };
    CardLayout {
        size: Size::new(width, b.y.ceil()),
        items: b.items,
    }
}

/// Lays out `model` as a compact overview card row (`OverviewMenuCardRowView`):
/// header and usage metrics without section dividers or credits.
pub fn overview_layout(model: &CardModel, width: f64, measure: &dyn TextMeasure) -> CardLayout {
    let mut b = Builder {
        measure,
        x: HORIZONTAL_PADDING,
        width: (width - 2.0 * HORIZONTAL_PADDING).max(0.0),
        y: HEADER_ONLY_VERTICAL_PADDING,
        items: Vec::new(),
    };

    b.header(model);
    b.y += HEADER_ONLY_VERTICAL_PADDING;

    if model.has_usage_content() {
        b.y += USAGE_SECTION_TOP_PADDING;
        for (index, row) in model.metrics.iter().enumerate() {
            if index > 0 {
                b.y += SECTION_SPACING;
            }
            b.metric_row(row);
        }
        if model.metrics.is_empty()
            && let Some(placeholder) = &model.placeholder
        {
            b.text_line(label(
                placeholder,
                PLACEHOLDER_FONT,
                TextColor::Secondary,
                0,
            ));
        }
        b.y += SECTION_BOTTOM_PADDING;
    }

    CardLayout {
        size: Size::new(width, b.y.ceil()),
        items: b.items,
    }
}

/// Lays out `summary` as the Overview spend summary card (`OverviewSpendSummaryCardView`,
/// `StatusItemController+OverviewSpend.swift:16-58`).
pub fn spend_summary_layout(
    summary: &OverviewSpendSummary,
    width: f64,
    measure: &dyn TextMeasure,
) -> CardLayout {
    let mut b = Builder {
        measure,
        x: HORIZONTAL_PADDING,
        width: (width - 2.0 * HORIZONTAL_PADDING).max(0.0),
        y: SPEND_SUMMARY_VERTICAL_PADDING,
        items: Vec::new(),
    };

    // Row 1: HStack(alignment: .firstTextBaseline, spacing: 6) { "Usage & Spend", "·", dayRangeText }
    let title = label("Usage & Spend", NAME_FONT, TextColor::Secondary, 1);
    let dot = label("·", EMAIL_FONT, TextColor::Secondary, 1);
    let range = label(&summary.day_range_text, EMAIL_FONT, TextColor::Secondary, 1);
    let title_sz = b.size(&title, b.width);
    let dot_sz = b.size(
        &dot,
        (b.width - title_sz.w - SPEND_SUMMARY_HEADER_SPACING).max(0.0),
    );
    let range_sz = b.size(
        &range,
        (b.width - title_sz.w - dot_sz.w - 2.0 * SPEND_SUMMARY_HEADER_SPACING).max(0.0),
    );
    let tb = b.measure.first_baseline(title.font);
    let db = b.measure.first_baseline(dot.font);
    let rb = b.measure.first_baseline(range.font);
    let base = tb.max(db).max(rb);
    let title_y = b.y + base - tb;
    let dot_y = b.y + base - db;
    let range_y = b.y + base - rb;
    let dot_x = b.x + title_sz.w + SPEND_SUMMARY_HEADER_SPACING;
    let range_x = dot_x + dot_sz.w + SPEND_SUMMARY_HEADER_SPACING;
    let row1_bottom = (title_y + title_sz.h)
        .max(dot_y + dot_sz.h)
        .max(range_y + range_sz.h);
    b.push_text(title, Rect::new(b.x, title_y, title_sz.w, title_sz.h));
    b.push_text(dot, Rect::new(dot_x, dot_y, dot_sz.w, dot_sz.h));
    b.push_text(range, Rect::new(range_x, range_y, range_sz.w, range_sz.h));
    b.y = row1_bottom + SPEND_SUMMARY_ROW_SPACING;

    // Row 2: primarySpendText (.title2, .bold, .primary, lineLimit: 2)
    b.text_line(label(
        &summary.primary_spend_text,
        Font::new(TextStyle::Title2, Weight::Bold),
        TextColor::Primary,
        2,
    ));
    b.y += SPEND_SUMMARY_ROW_SPACING;

    // Row 3: HStack(spacing: 8) { providerCoverageText, ["·", tokenText] } (.caption, .secondary, lineLimit: 1)
    let prov_cov = label(
        &summary.provider_coverage_text,
        CAPTION,
        TextColor::Secondary,
        1,
    );
    if let Some(tokens) = &summary.token_text {
        let cov_sz = b.size(&prov_cov, b.width);
        let sep = label("·", CAPTION, TextColor::Secondary, 1);
        let sep_sz = b.size(
            &sep,
            (b.width - cov_sz.w - SPEND_SUMMARY_META_SPACING).max(0.0),
        );
        let tok = label(tokens, CAPTION, TextColor::Secondary, 1);
        let tok_sz = b.size(
            &tok,
            (b.width - cov_sz.w - sep_sz.w - 2.0 * SPEND_SUMMARY_META_SPACING).max(0.0),
        );
        let sep_x = b.x + cov_sz.w + SPEND_SUMMARY_META_SPACING;
        let tok_x = sep_x + sep_sz.w + SPEND_SUMMARY_META_SPACING;
        let row3_h = cov_sz.h.max(sep_sz.h).max(tok_sz.h);
        b.push_text(prov_cov, Rect::new(b.x, b.y, cov_sz.w, cov_sz.h));
        b.push_text(sep, Rect::new(sep_x, b.y, sep_sz.w, sep_sz.h));
        b.push_text(tok, Rect::new(tok_x, b.y, tok_sz.w, tok_sz.h));
        b.y += row3_h;
    } else {
        b.text_line(prov_cov);
    }
    b.y += SPEND_SUMMARY_ROW_SPACING;

    // Row 4: "\(historyCoverageText) · \(provenanceText)" (.caption, .secondary, lineLimit: 2)
    let cov_prov = format!(
        "{} · {}",
        summary.history_coverage_text, summary.provenance_text
    );
    b.text_line(label(&cov_prov, CAPTION, TextColor::Secondary, 2));
    b.y += SPEND_SUMMARY_ROW_SPACING;

    // Row 5: pricingCoverageText (.caption, .secondary, lineLimit: 2)
    b.text_line(label(
        &summary.pricing_coverage_text,
        CAPTION,
        TextColor::Secondary,
        2,
    ));
    b.y += SPEND_SUMMARY_VERTICAL_PADDING;

    CardLayout {
        size: Size::new(width, b.y.ceil()),
        items: b.items,
    }
}
