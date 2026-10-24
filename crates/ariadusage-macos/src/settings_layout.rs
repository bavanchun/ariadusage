// Ported from CodexBar Sources/CodexBar/PreferencesView.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesSidebar.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesComponents.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuBarLayoutEditor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesProviderDetailView.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesSpendDashboardPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Pure layout geometry and hit-testing for the Settings window.
//!
//! Coordinates are in points with a top-left origin (`y` grows downwards),
//! shared by AppKit drawing (`SettingsView`) and unit tests.

use ariadusage_protocol::ids::SettingId;

use crate::geometry::{Point, Rect, Size};
use crate::settings::{
    ChoiceItem, DETAIL_MAX_WIDTH, FormButtonAction, FormControl, FormRowModel, FormSectionModel,
    PaneModel, ProviderStatusDotColor, Rgb, SHORTCUT_RECORDER_WIDTH, SettingsPane,
    SettingsSelection, SidebarChipStyle, SidebarFilterState, SidebarModel, clamp_sidebar_width,
};

/// Height of the transparent titlebar area above sidebar/detail headers.
pub const TITLEBAR_HEIGHT: f64 = 38.0;
/// Height of the detail header bar containing Back/Forward buttons and pane title.
pub const DETAIL_HEADER_HEIGHT: f64 = 52.0;
/// Horizontal padding inside the detail scroll column.
pub const DETAIL_HORIZONTAL_PADDING: f64 = 28.0;
/// Height of a sidebar row (`SettingsSidebarMetrics.rowHeight`).
pub const SIDEBAR_ROW_HEIGHT: f64 = 28.0;
/// Vertical stride between sidebar rows.
pub const SIDEBAR_ROW_STRIDE: f64 = 30.0;
/// Corner radius of a grouped form card container.
pub const SECTION_CARD_CORNER_RADIUS: f64 = 10.0;

/// Traffic light circle in the top-left window chrome (used when rendering offscreen).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrafficLightCircle {
    pub rect: Rect,
}

/// Layout of the sidebar search and filter bar.
#[derive(Debug, Clone, PartialEq)]
pub struct SidebarSearchBarLayout {
    pub search_field_rect: Rect,
    pub clear_button_rect: Option<Rect>,
    pub sort_button_rect: Rect,
    pub filter_button_rect: Rect,
    pub search_text: String,
    pub sorted_alphabetically: bool,
    pub enabled_only: bool,
}

/// Layout of a standard app pane row in the sidebar.
#[derive(Debug, Clone, PartialEq)]
pub struct SidebarAppPaneRowLayout {
    pub pane: SettingsPane,
    pub title: String,
    pub selected: bool,
    pub row_rect: Rect,
    pub chip_rect: Rect,
    pub chip_style: Option<SidebarChipStyle>,
    pub label_rect: Rect,
}

/// Layout of a provider row in the sidebar.
#[derive(Debug, Clone, PartialEq)]
pub struct SidebarProviderRowLayout {
    pub provider_id: String,
    pub display_name: String,
    pub enabled: bool,
    pub selected: bool,
    pub row_rect: Rect,
    pub icon_rect: Rect,
    pub label_rect: Rect,
    pub status_dot_rect: Option<Rect>,
    pub status_dot_color: Option<ProviderStatusDotColor>,
}

/// Layout of the left sidebar column.
#[derive(Debug, Clone, PartialEq)]
pub struct SidebarLayout {
    pub frame: Rect,
    pub traffic_lights: [TrafficLightCircle; 3],
    pub search_bar: SidebarSearchBarLayout,
    pub app_rows: Vec<SidebarAppPaneRowLayout>,
    pub providers_header_label_rect: Rect,
    pub providers_header_count_rect: Rect,
    pub providers_header_count_text: String,
    pub provider_rows: Vec<SidebarProviderRowLayout>,
    pub empty_filter_rect: Option<Rect>,
    pub clear_filters_button_rect: Option<Rect>,
}

/// Layout of the detail header bar (`< | >` navigation + pane title).
#[derive(Debug, Clone, PartialEq)]
pub struct DetailHeaderLayout {
    pub frame: Rect,
    pub nav_group_rect: Rect,
    pub back_button_rect: Rect,
    pub nav_divider_rect: Rect,
    pub forward_button_rect: Rect,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub title_rect: Option<Rect>,
    pub title: String,
}

/// Layout of a button inside a form row.
#[derive(Debug, Clone, PartialEq)]
pub struct FormButtonLayout {
    pub rect: Rect,
    pub title: String,
    pub enabled: bool,
    pub prominent: bool,
    pub action: FormButtonAction,
}

/// Layout of a token chip inside the Menu Bar layout preview.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenChipLayout {
    pub rect: Rect,
    pub label: String,
    pub active_strip: bool,
}

/// Layout of a palette group in the Menu Bar layout editor preview.
#[derive(Debug, Clone, PartialEq)]
pub struct PaletteGroupLayout {
    pub title_rect: Rect,
    pub title: String,
    pub chips: Vec<TokenChipLayout>,
}

/// Layout of the Menu Bar layout editor preview card.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuBarLayoutPreviewGeometry {
    pub scope_label_rect: Rect,
    pub scope_picker_rect: Rect,
    pub scope_label: String,
    pub preset_label_rect: Rect,
    pub preset_picker_rect: Rect,
    pub preset_label: String,
    pub preview_title_rect: Rect,
    pub preview_box_rect: Rect,
    pub preview_pill_rect: Rect,
    pub preview_percent_text: String,
    pub reset_button_rect: Rect,
    pub strip_title_rect: Rect,
    pub strip_box_rect: Rect,
    pub line_label_rect: Rect,
    pub strip_chips: Vec<TokenChipLayout>,
    pub remove_button_rect: Rect,
    pub palette_groups: Vec<PaletteGroupLayout>,
    pub conditionals_title_rect: Rect,
    pub new_conditional_button_rect: Rect,
    pub size_label_rect: Rect,
    pub size_popup_rect: Rect,
    pub gap_label_rect: Rect,
    pub gap_popup_rect: Rect,
    pub vertical_label_rect: Rect,
    pub vertical_value_rect: Rect,
    pub vertical_stepper_rect: Rect,
    pub monospaced_checkbox_rect: Rect,
    pub window_labels_checkbox_rect: Rect,
    pub enabled: bool,
}

/// Layout of the control on a form row.
#[derive(Debug, Clone, PartialEq)]
pub enum FormControlLayout {
    Toggle {
        rect: Rect,
        value: bool,
        enabled: bool,
        setting_id: Option<SettingId>,
    },
    Choice {
        rect: Rect,
        selected_label: String,
        options: Vec<ChoiceItem>,
        enabled: bool,
        setting_id: Option<SettingId>,
    },
    Buttons {
        buttons: Vec<FormButtonLayout>,
    },
    ShortcutRecorder {
        rect: Rect,
        placeholder: String,
        enabled: bool,
    },
    TextField {
        rect: Rect,
        value: String,
        placeholder: String,
        enabled: bool,
    },
    Slider {
        value_label_rect: Rect,
        slider_rect: Rect,
        reset_button_rect: Rect,
        value: f64,
        min: f64,
        max: f64,
        value_label: String,
        reset_title: String,
        enabled: bool,
    },
    StepperPopup {
        field_rect: Rect,
        stepper_rect: Rect,
        popup_rect: Rect,
        value_text: String,
        popup_label: String,
        enabled: bool,
    },
    MenuBarLayoutPreview(Box<MenuBarLayoutPreviewGeometry>),
    ExternalLink {
        leading_icon_rect: Rect,
        trailing_icon_rect: Rect,
        symbol: &'static str,
        url: String,
    },
    StatusNote {
        text_rect: Rect,
        lines: Vec<String>,
        command_rect: Option<Rect>,
        copy_button_rect: Option<Rect>,
        command: Option<String>,
    },
    Checkbox {
        rect: Rect,
        value: bool,
        enabled: bool,
        setting_id: Option<SettingId>,
    },
    BorderedChoice {
        rect: Rect,
        selected_label: String,
        options: Vec<ChoiceItem>,
        enabled: bool,
        setting_id: Option<SettingId>,
    },
    Segmented {
        segmented_rect: Rect,
        trailing_note_rect: Option<Rect>,
        labels: Vec<String>,
        selected_index: usize,
        trailing_note: Option<String>,
        enabled: bool,
    },
    ReadOnlyValue {
        value_rect: Rect,
        value: String,
        monospaced: bool,
    },
    SecretStatus {
        status_rect: Rect,
        button: FormButtonLayout,
        is_set: bool,
        status_text: String,
    },
    ColorHex {
        field_rect: Rect,
        swatch_rect: Rect,
        hex_value: String,
        placeholder: String,
        swatch_rgb: Rgb,
        enabled: bool,
    },
    ProviderHeader {
        icon_rect: Rect,
        title_rect: Rect,
        subtitle_rect: Rect,
        refresh_button: FormButtonLayout,
        toggle_rect: Rect,
        provider_id: String,
        display_name: String,
        subtitle: String,
        enabled: bool,
        toggle_setting_id: Option<SettingId>,
    },
    UsageWindowBar {
        title_rect: Rect,
        reset_rect: Rect,
        bar_rect: Rect,
        pace_rect: Option<Rect>,
        title: String,
        percent_used: f64,
        percent_label: String,
        reset_label: String,
        pace_label: Option<String>,
        accent_rgb: Rgb,
    },
    EmptyStateCard {
        icon_rect: Rect,
        title_rect: Rect,
        subtitle_rect: Rect,
        symbol: &'static str,
        title: String,
        subtitle: String,
    },
    SpendSummaryGrid {
        currency_rect: Rect,
        coverage_rect: Rect,
        metric_boxes: Vec<(Rect, String, String)>,
        token_items: Vec<(Rect, String, String)>,
        footer_rect: Rect,
        currency_label: String,
        coverage_label: String,
        footer_note: String,
    },
}

/// Layout of one row inside a grouped form section card.
#[derive(Debug, Clone, PartialEq)]
pub struct FormRowLayout {
    pub rect: Rect,
    pub title: String,
    pub title_rect: Option<Rect>,
    pub subtitle: Option<String>,
    pub subtitle_rect: Option<Rect>,
    pub row_enabled: bool,
    pub control: FormControlLayout,
    pub show_bottom_divider: bool,
}

/// Layout of a grouped form section (`Section` in `.formStyle(.grouped)`).
#[derive(Debug, Clone, PartialEq)]
pub struct FormSectionLayout {
    pub header_title: Option<String>,
    pub header_rect: Option<Rect>,
    pub card_rect: Rect,
    pub rows: Vec<FormRowLayout>,
    pub footer_lines: Vec<(Rect, String)>,
}

/// Layout of the About hero card (`AboutPane.hero`).
#[derive(Debug, Clone, PartialEq)]
pub struct AboutHeroLayout {
    pub card_rect: Rect,
    pub icon_rect: Rect,
    pub name_rect: Rect,
    pub version_rect: Rect,
    pub tagline_rect: Rect,
    pub app_name: String,
    pub version_text: String,
    pub tagline: String,
    pub homepage_url: String,
}

/// Layout of the bottom footer in the General pane (`Version` + `Quit AriadUsage`).
#[derive(Debug, Clone, PartialEq)]
pub struct GeneralFooterLayout {
    pub version_rect: Rect,
    pub version_text: String,
    pub quit_button: FormButtonLayout,
}

/// Full layout of the Settings window for a given size, sidebar width, and scroll offset.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsWindowLayout {
    pub window_size: Size,
    pub sidebar_width: f64,
    pub sidebar: SidebarLayout,
    pub divider_rect: Rect,
    pub resize_handle_rect: Rect,
    pub detail_header: DetailHeaderLayout,
    pub detail_viewport_rect: Rect,
    pub detail_content_height: f64,
    pub max_scroll_y: f64,
    pub scroll_y: f64,
    pub about_hero: Option<AboutHeroLayout>,
    pub sections: Vec<FormSectionLayout>,
    pub general_footer: Option<GeneralFooterLayout>,
    pub centered_footer: Option<(Rect, String)>,
}

/// Interactive target hit by a mouse click in the Settings window.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsHitTarget {
    SidebarPane(SettingsPane),
    SidebarSortToggle,
    SidebarFilterToggle,
    SidebarClearSearch,
    SidebarClearFilters,
    SidebarResizeHandle,
    NavBack,
    NavForward,
    ToggleSetting {
        setting_id: SettingId,
        new_value: bool,
    },
    ChoicePopup {
        rect: Rect,
        setting_id: SettingId,
        options: Vec<ChoiceItem>,
        selected_label: String,
    },
    ButtonAction(FormButtonAction),
    ExternalUrl(String),
}

/// Computes the complete [`SettingsWindowLayout`].
pub fn layout_settings_window(
    window_size: Size,
    sidebar_width: f64,
    scroll_y: f64,
    selection: &SettingsSelection,
    filter: &SidebarFilterState,
    sidebar_model: &SidebarModel,
    pane_model: &PaneModel,
) -> SettingsWindowLayout {
    let sidebar_w = clamp_sidebar_width(sidebar_width);
    let sidebar = layout_sidebar(
        window_size.h,
        sidebar_w,
        selection.pane(),
        filter,
        sidebar_model,
    );

    let divider_rect = Rect::new(sidebar_w, 0.0, 1.0, window_size.h);
    let resize_handle_rect = Rect::new(sidebar_w - 3.0, 0.0, 7.0, window_size.h);

    let detail_x = sidebar_w + 1.0;
    let detail_w = (window_size.w - detail_x).max(240.0);
    let max_inner_w = DETAIL_MAX_WIDTH - 2.0 * DETAIL_HORIZONTAL_PADDING;
    let content_w = (detail_w - 2.0 * DETAIL_HORIZONTAL_PADDING).clamp(200.0, max_inner_w);
    let content_x = detail_x + ((detail_w - content_w) / 2.0).max(DETAIL_HORIZONTAL_PADDING);

    let detail_header = layout_detail_header(
        detail_x, detail_w, content_x, content_w, selection, pane_model,
    );

    let detail_viewport_rect = Rect::new(
        detail_x,
        DETAIL_HEADER_HEIGHT,
        detail_w,
        (window_size.h - DETAIL_HEADER_HEIGHT).max(0.0),
    );

    // First pass at scroll_y = 0 to measure total content height and clamp scroll_y.
    let unclipped_height = measure_detail_content_height(content_w, pane_model);
    let max_scroll_y = (unclipped_height - detail_viewport_rect.h).max(0.0);
    let clamped_scroll_y = scroll_y.clamp(0.0, max_scroll_y);

    let mut cur_y = DETAIL_HEADER_HEIGHT + 8.0 - clamped_scroll_y;

    let about_hero = pane_model.about_hero.as_ref().map(|hero| {
        let card_h = 198.0;
        let card_rect = Rect::new(content_x, cur_y, content_w, card_h);
        let icon_size = 88.0;
        let icon_rect = Rect::new(
            content_x + (content_w - icon_size) / 2.0,
            cur_y + 16.0,
            icon_size,
            icon_size,
        );
        let name_rect = Rect::new(content_x + 20.0, cur_y + 112.0, content_w - 40.0, 24.0);
        let version_rect = Rect::new(content_x + 20.0, cur_y + 138.0, content_w - 40.0, 16.0);
        let tagline_rect = Rect::new(content_x + 20.0, cur_y + 158.0, content_w - 40.0, 24.0);
        cur_y += card_h + 16.0;
        AboutHeroLayout {
            card_rect,
            icon_rect,
            name_rect,
            version_rect,
            tagline_rect,
            app_name: hero.app_name.clone(),
            version_text: hero.version_text.clone(),
            tagline: hero.tagline.clone(),
            homepage_url: hero.homepage_url.clone(),
        }
    });

    let mut sections = Vec::with_capacity(pane_model.sections.len());
    for section in &pane_model.sections {
        let sec_layout = layout_section(content_x, content_w, &mut cur_y, section);
        sections.push(sec_layout);
    }

    let general_footer = pane_model.general_footer.as_ref().map(|footer| {
        let footer_y = cur_y + 4.0;
        let version_rect = Rect::new(content_x + 4.0, footer_y + 4.0, 220.0, 18.0);
        let quit_w = 128.0;
        let quit_rect = Rect::new(content_x + content_w - quit_w, footer_y, quit_w, 26.0);
        cur_y = footer_y + 34.0;
        GeneralFooterLayout {
            version_rect,
            version_text: footer.version_text.clone(),
            quit_button: FormButtonLayout {
                rect: quit_rect,
                title: footer.quit_title.clone(),
                enabled: true,
                prominent: false,
                action: FormButtonAction::QuitApp,
            },
        }
    });

    let centered_footer = pane_model.centered_footer.as_ref().map(|text| {
        let rect = Rect::new(content_x, cur_y + 4.0, content_w, 18.0);
        cur_y += 28.0;
        (rect, text.clone())
    });

    SettingsWindowLayout {
        window_size,
        sidebar_width: sidebar_w,
        sidebar,
        divider_rect,
        resize_handle_rect,
        detail_header,
        detail_viewport_rect,
        detail_content_height: unclipped_height,
        max_scroll_y,
        scroll_y: clamped_scroll_y,
        about_hero,
        sections,
        general_footer,
        centered_footer,
    }
}

fn layout_sidebar(
    window_h: f64,
    sidebar_w: f64,
    selected_pane: &SettingsPane,
    filter: &SidebarFilterState,
    sidebar_model: &SidebarModel,
) -> SidebarLayout {
    let frame = Rect::new(0.0, 0.0, sidebar_w, window_h);
    let traffic_lights = [
        TrafficLightCircle {
            rect: Rect::new(14.0, 14.0, 12.0, 12.0),
        },
        TrafficLightCircle {
            rect: Rect::new(34.0, 14.0, 12.0, 12.0),
        },
        TrafficLightCircle {
            rect: Rect::new(54.0, 14.0, 12.0, 12.0),
        },
    ];

    let filter_btn_rect = Rect::new(sidebar_w - 12.0 - 24.0, 46.0, 24.0, 24.0);
    let sort_btn_rect = Rect::new(filter_btn_rect.x - 4.0 - 24.0, 46.0, 24.0, 24.0);
    let search_w = (sort_btn_rect.x - 6.0 - 12.0).max(80.0);
    let search_field_rect = Rect::new(12.0, 44.0, search_w, 28.0);
    let clear_button_rect = (!filter.search_text.is_empty()).then(|| {
        Rect::new(
            search_field_rect.max_x() - 20.0,
            search_field_rect.y + 6.0,
            16.0,
            16.0,
        )
    });

    let search_bar = SidebarSearchBarLayout {
        search_field_rect,
        clear_button_rect,
        sort_button_rect: sort_btn_rect,
        filter_button_rect: filter_btn_rect,
        search_text: filter.search_text.clone(),
        sorted_alphabetically: filter.sorted_alphabetically,
        enabled_only: filter.enabled_only,
    };

    let mut cur_y = 84.0;
    let mut app_rows = Vec::with_capacity(sidebar_model.app_panes.len());
    for pane in &sidebar_model.app_panes {
        let row_rect = Rect::new(10.0, cur_y, sidebar_w - 20.0, SIDEBAR_ROW_HEIGHT);
        let chip_rect = Rect::new(18.0, cur_y + 4.0, 20.0, 20.0);
        let label_rect = Rect::new(46.0, cur_y + 5.0, sidebar_w - 58.0, 18.0);
        app_rows.push(SidebarAppPaneRowLayout {
            pane: pane.clone(),
            title: pane.title(),
            selected: pane == selected_pane,
            row_rect,
            chip_rect,
            chip_style: pane.chip_style(),
            label_rect,
        });
        cur_y += SIDEBAR_ROW_STRIDE;
    }

    cur_y += 10.0;
    let providers_header_label_rect = Rect::new(18.0, cur_y, 100.0, 16.0);
    let providers_header_count_rect = Rect::new(sidebar_w - 78.0, cur_y, 60.0, 16.0);
    cur_y += 22.0;

    let mut provider_rows = Vec::with_capacity(sidebar_model.providers.len());
    for provider in &sidebar_model.providers {
        let row_rect = Rect::new(10.0, cur_y, sidebar_w - 20.0, SIDEBAR_ROW_HEIGHT);
        let icon_rect = Rect::new(18.0, cur_y + 4.0, 20.0, 20.0);
        let label_rect = Rect::new(46.0, cur_y + 5.0, sidebar_w - 76.0, 18.0);
        let status_dot_rect = provider
            .status_dot
            .map(|_| Rect::new(sidebar_w - 26.0, cur_y + 11.0, 6.0, 6.0));
        let is_selected = matches!(selected_pane, SettingsPane::Provider(id) if id == &provider.id);
        provider_rows.push(SidebarProviderRowLayout {
            provider_id: provider.id.clone(),
            display_name: provider.display_name.clone(),
            enabled: provider.enabled,
            selected: is_selected,
            row_rect,
            icon_rect,
            label_rect,
            status_dot_rect,
            status_dot_color: provider.status_dot,
        });
        cur_y += SIDEBAR_ROW_STRIDE;
    }

    let (empty_filter_rect, clear_filters_button_rect) = if sidebar_model.shows_empty_filter_state {
        let empty_rect = Rect::new(18.0, cur_y + 4.0, sidebar_w - 36.0, 18.0);
        let clear_rect = Rect::new(18.0, cur_y + 26.0, 96.0, 22.0);
        (Some(empty_rect), Some(clear_rect))
    } else {
        (None, None)
    };

    SidebarLayout {
        frame,
        traffic_lights,
        search_bar,
        app_rows,
        providers_header_label_rect,
        providers_header_count_rect,
        providers_header_count_text: sidebar_model.providers_header_count_text.clone(),
        provider_rows,
        empty_filter_rect,
        clear_filters_button_rect,
    }
}

fn layout_detail_header(
    detail_x: f64,
    detail_w: f64,
    content_x: f64,
    content_w: f64,
    selection: &SettingsSelection,
    pane_model: &PaneModel,
) -> DetailHeaderLayout {
    let frame = Rect::new(detail_x, 0.0, detail_w, DETAIL_HEADER_HEIGHT);
    let nav_y = 15.0;
    let nav_group_rect = Rect::new(content_x, nav_y, 53.0, 22.0);
    let back_button_rect = Rect::new(content_x, nav_y, 26.0, 22.0);
    let nav_divider_rect = Rect::new(content_x + 26.0, nav_y + 3.0, 1.0, 16.0);
    let forward_button_rect = Rect::new(content_x + 27.0, nav_y, 26.0, 22.0);
    let title_rect = pane_model
        .shows_header_title
        .then(|| Rect::new(content_x + 64.0, 16.0, (content_w - 64.0).max(80.0), 20.0));

    DetailHeaderLayout {
        frame,
        nav_group_rect,
        back_button_rect,
        nav_divider_rect,
        forward_button_rect,
        can_go_back: selection.can_go_back(),
        can_go_forward: selection.can_go_forward(),
        title_rect,
        title: pane_model.title.clone(),
    }
}

fn measure_detail_content_height(content_w: f64, pane_model: &PaneModel) -> f64 {
    let mut h = 8.0;
    if pane_model.about_hero.is_some() {
        h += 198.0 + 16.0;
    }
    for section in &pane_model.sections {
        if section.title.is_some() {
            h += 22.0;
        }
        let rows_h: f64 = section
            .rows
            .iter()
            .map(|r| measure_row_height(content_w, r))
            .sum();
        h += rows_h;
        if !section.footer_lines.is_empty() {
            h += 6.0;
            for line in &section.footer_lines {
                h += measure_footer_line_height(content_w, line);
            }
        }
        h += 16.0;
    }
    if pane_model.general_footer.is_some() {
        h += 38.0;
    }
    if pane_model.centered_footer.is_some() {
        h += 28.0;
    }
    h + 16.0
}

fn measure_row_height(content_w: f64, row: &FormRowModel) -> f64 {
    match &row.control {
        FormControl::MenuBarLayoutPreview { .. } => 418.0,
        FormControl::Slider { .. } => 70.0,
        FormControl::ProviderHeader { .. } => 60.0,
        FormControl::UsageWindowBar { pace_text, .. } => {
            if pace_text.is_some() {
                62.0
            } else {
                46.0
            }
        }
        FormControl::EmptyStateCard { .. } => 148.0,
        FormControl::SpendSummaryGrid { .. } => 176.0,
        FormControl::Checkbox { .. } if row.subtitle.is_none() => 32.0,
        FormControl::Segmented { .. } => 36.0,
        FormControl::ColorHex { .. } => 42.0,
        FormControl::StatusNote { command, .. } => {
            if command.is_some() {
                78.0
            } else {
                40.0
            }
        }
        _ => {
            let Some(sub) = &row.subtitle else {
                return 36.0;
            };
            let avail_w = (content_w - 200.0).max(180.0);
            let chars_per_line = (avail_w / 6.3).floor().max(28.0) as usize;
            let explicit_lines: usize = sub
                .split('\n')
                .map(|part| {
                    let len = part.chars().count().max(1);
                    len.div_ceil(chars_per_line)
                })
                .sum();
            match explicit_lines {
                0 | 1 => 50.0,
                2 => 64.0,
                _ => 76.0,
            }
        }
    }
}

fn measure_footer_line_height(content_w: f64, line: &str) -> f64 {
    let chars_per_line = ((content_w - 8.0) / 6.2).floor().max(40.0) as usize;
    let lines = line.chars().count().max(1).div_ceil(chars_per_line);
    if lines <= 1 {
        16.0
    } else {
        (lines as f64) * 15.0 + 2.0
    }
}

fn layout_section(
    content_x: f64,
    content_w: f64,
    cur_y: &mut f64,
    section: &FormSectionModel,
) -> FormSectionLayout {
    let header_rect = section.title.as_ref().map(|_| {
        let r = Rect::new(content_x + 4.0, *cur_y, content_w - 8.0, 18.0);
        *cur_y += 22.0;
        r
    });

    let card_start_y = *cur_y;
    let row_count = section.rows.len();
    let mut rows = Vec::with_capacity(row_count);
    for (idx, row) in section.rows.iter().enumerate() {
        let row_h = measure_row_height(content_w, row);
        let row_rect = Rect::new(content_x, *cur_y, content_w, row_h);
        let row_layout = layout_row(row_rect, row, idx + 1 < row_count);
        rows.push(row_layout);
        *cur_y += row_h;
    }
    let card_rect = Rect::new(content_x, card_start_y, content_w, *cur_y - card_start_y);

    let mut footer_lines = Vec::with_capacity(section.footer_lines.len());
    if !section.footer_lines.is_empty() {
        *cur_y += 6.0;
        for line in &section.footer_lines {
            let lh = measure_footer_line_height(content_w, line);
            let lr = Rect::new(content_x + 4.0, *cur_y, content_w - 8.0, lh);
            footer_lines.push((lr, line.clone()));
            *cur_y += lh;
        }
    }

    *cur_y += 16.0;

    FormSectionLayout {
        header_title: section.title.clone(),
        header_rect,
        card_rect,
        rows,
        footer_lines,
    }
}

fn layout_row(row_rect: Rect, row: &FormRowModel, show_bottom_divider: bool) -> FormRowLayout {
    let inner_x = row_rect.x + 12.0;
    let inner_right = row_rect.max_x() - 12.0;
    let inner_w = row_rect.w - 24.0;

    let (control, right_reserved_w) = match &row.control {
        FormControl::Toggle {
            value,
            enabled,
            setting_id,
        } => {
            let sw = 38.0;
            let sh = 22.0;
            let rect = Rect::new(
                inner_right - sw,
                row_rect.y + (row_rect.h - sh) / 2.0,
                sw,
                sh,
            );
            (
                FormControlLayout::Toggle {
                    rect,
                    value: *value,
                    enabled: *enabled,
                    setting_id: setting_id.clone(),
                },
                sw + 12.0,
            )
        }
        FormControl::Choice {
            selected_label,
            options,
            enabled,
            setting_id,
        } => {
            let cw = if selected_label.chars().count() > 16 {
                182.0
            } else {
                148.0
            };
            let ch = 24.0;
            let rect = Rect::new(
                inner_right - cw,
                row_rect.y + (row_rect.h - ch) / 2.0,
                cw,
                ch,
            );
            (
                FormControlLayout::Choice {
                    rect,
                    selected_label: selected_label.clone(),
                    options: options.clone(),
                    enabled: *enabled,
                    setting_id: setting_id.clone(),
                },
                cw + 12.0,
            )
        }
        FormControl::ShortcutRecorder {
            placeholder,
            enabled,
        } => {
            let rw = SHORTCUT_RECORDER_WIDTH;
            let rh = 24.0;
            let rect = Rect::new(
                inner_right - rw,
                row_rect.y + (row_rect.h - rh) / 2.0,
                rw,
                rh,
            );
            (
                FormControlLayout::ShortcutRecorder {
                    rect,
                    placeholder: placeholder.clone(),
                    enabled: *enabled,
                },
                rw + 12.0,
            )
        }
        FormControl::TextField {
            value,
            placeholder,
            enabled,
        } => {
            let tw = 210.0;
            let th = 24.0;
            let rect = Rect::new(
                inner_right - tw,
                row_rect.y + (row_rect.h - th) / 2.0,
                tw,
                th,
            );
            (
                FormControlLayout::TextField {
                    rect,
                    value: value.clone(),
                    placeholder: placeholder.clone(),
                    enabled: *enabled,
                },
                tw + 12.0,
            )
        }
        FormControl::Slider {
            value,
            min,
            max,
            value_label,
            reset_title,
            enabled,
        } => {
            let value_label_rect = Rect::new(inner_right - 56.0, row_rect.y + 8.0, 56.0, 17.0);
            let reset_w = 56.0;
            let reset_button_rect =
                Rect::new(inner_right - reset_w, row_rect.y + 26.0, reset_w, 22.0);
            let slider_rect = Rect::new(
                inner_x,
                row_rect.y + 28.0,
                (inner_w - reset_w - 8.0).max(120.0),
                20.0,
            );
            (
                FormControlLayout::Slider {
                    value_label_rect,
                    slider_rect,
                    reset_button_rect,
                    value: *value,
                    min: *min,
                    max: *max,
                    value_label: value_label.clone(),
                    reset_title: reset_title.clone(),
                    enabled: *enabled,
                },
                64.0,
            )
        }
        FormControl::StepperPopup {
            value_text,
            popup_label,
            enabled,
        } => {
            let popup_w = 180.0;
            let popup_rect = Rect::new(
                inner_right - popup_w,
                row_rect.y + (row_rect.h - 24.0) / 2.0,
                popup_w,
                24.0,
            );
            let stepper_rect = Rect::new(
                popup_rect.x - 19.0,
                row_rect.y + (row_rect.h - 22.0) / 2.0,
                15.0,
                22.0,
            );
            let field_rect = Rect::new(
                stepper_rect.x - 46.0,
                row_rect.y + (row_rect.h - 22.0) / 2.0,
                42.0,
                22.0,
            );
            let reserved = (inner_right - field_rect.x) + 12.0;
            (
                FormControlLayout::StepperPopup {
                    field_rect,
                    stepper_rect,
                    popup_rect,
                    value_text: value_text.clone(),
                    popup_label: popup_label.clone(),
                    enabled: *enabled,
                },
                reserved,
            )
        }
        FormControl::Buttons { buttons } => {
            let bh = 24.0;
            let by = row_rect.y + (row_rect.h - bh) / 2.0;
            let mut btn_layouts = Vec::with_capacity(buttons.len());
            if row.title.is_empty() {
                let mut bx = inner_x;
                for btn in buttons {
                    let bw = (btn.title.chars().count() as f64 * 6.8 + 24.0).max(96.0);
                    btn_layouts.push(FormButtonLayout {
                        rect: Rect::new(bx, by, bw, bh),
                        title: btn.title.clone(),
                        enabled: btn.enabled,
                        prominent: btn.prominent,
                        action: btn.action.clone(),
                    });
                    bx += bw + 8.0;
                }
                (
                    FormControlLayout::Buttons {
                        buttons: btn_layouts,
                    },
                    0.0,
                )
            } else {
                let mut bx = inner_right;
                for btn in buttons.iter().rev() {
                    let bw = (btn.title.chars().count() as f64 * 6.8 + 24.0).max(96.0);
                    bx -= bw;
                    btn_layouts.push(FormButtonLayout {
                        rect: Rect::new(bx, by, bw, bh),
                        title: btn.title.clone(),
                        enabled: btn.enabled,
                        prominent: btn.prominent,
                        action: btn.action.clone(),
                    });
                    bx -= 8.0;
                }
                btn_layouts.reverse();
                let reserved = (inner_right - bx).max(0.0) + 8.0;
                (
                    FormControlLayout::Buttons {
                        buttons: btn_layouts,
                    },
                    reserved,
                )
            }
        }
        FormControl::ExternalLink { symbol, url } => {
            let leading_icon_rect = Rect::new(inner_x, row_rect.y + 10.0, 16.0, 16.0);
            let trailing_icon_rect = Rect::new(inner_right - 14.0, row_rect.y + 11.0, 14.0, 14.0);
            (
                FormControlLayout::ExternalLink {
                    leading_icon_rect,
                    trailing_icon_rect,
                    symbol,
                    url: url.clone(),
                },
                24.0,
            )
        }
        FormControl::StatusNote { lines, command } => {
            let text_h = if command.is_some() { 32.0 } else { 18.0 };
            let text_rect = Rect::new(inner_x, row_rect.y + 8.0, inner_w, text_h);
            let cmd_y = row_rect.y + 8.0 + text_h + 4.0;
            let command_rect = command
                .as_ref()
                .map(|_| Rect::new(inner_x, cmd_y, (inner_w - 64.0).max(120.0), 24.0));
            let copy_button_rect = command
                .as_ref()
                .map(|_| Rect::new(inner_right - 56.0, cmd_y, 56.0, 24.0));
            (
                FormControlLayout::StatusNote {
                    text_rect,
                    lines: lines.clone(),
                    command_rect,
                    copy_button_rect,
                    command: command.clone(),
                },
                0.0,
            )
        }
        FormControl::MenuBarLayoutPreview {
            scope_label,
            preset_label,
            preview_percent_text,
            strip_chips,
            palette_groups,
            enabled,
        } => {
            let geom = layout_menu_bar_preview(
                row_rect,
                scope_label,
                preset_label,
                preview_percent_text,
                strip_chips,
                palette_groups,
                *enabled,
            );
            (FormControlLayout::MenuBarLayoutPreview(Box::new(geom)), 0.0)
        }
        FormControl::Checkbox {
            checked,
            indent,
            enabled,
            setting_id,
        } => {
            let cy = if row.subtitle.is_some() {
                row_rect.y + 6.0
            } else {
                row_rect.y + (row_rect.h - 20.0) / 2.0
            };
            let rect = Rect::new(
                inner_x + *indent,
                cy,
                (inner_w - 8.0 - *indent).max(160.0),
                20.0,
            );
            (
                FormControlLayout::Checkbox {
                    rect,
                    value: *checked,
                    enabled: *enabled,
                    setting_id: setting_id.clone(),
                },
                0.0,
            )
        }
        FormControl::BorderedChoice {
            selected_label,
            options,
            enabled,
            setting_id,
        } => {
            let cw = if selected_label.chars().count() > 14 {
                160.0
            } else {
                118.0
            };
            let ch = 24.0;
            let rect = Rect::new(
                inner_right - cw,
                row_rect.y + (row_rect.h - ch) / 2.0,
                cw,
                ch,
            );
            (
                FormControlLayout::BorderedChoice {
                    rect,
                    selected_label: selected_label.clone(),
                    options: options.clone(),
                    enabled: *enabled,
                    setting_id: setting_id.clone(),
                },
                cw + 12.0,
            )
        }
        FormControl::Segmented {
            segments,
            selected_index,
            trailing_note,
            enabled,
        } => {
            let title_col_w = if trailing_note.is_some() { 64.0 } else { 96.0 };
            let natural_w: f64 = segments
                .iter()
                .map(|s| (s.chars().count() as f64 * 6.8 + 20.0).max(46.0))
                .sum();
            let seg_w = natural_w.clamp(120.0, (inner_w - title_col_w - 8.0).max(160.0));
            let seg_h = 22.0;
            let seg_x = (inner_x + title_col_w).min(inner_right - seg_w);
            let seg_y = row_rect.y + (row_rect.h - seg_h) / 2.0;
            let segmented_rect = Rect::new(seg_x, seg_y, seg_w, seg_h);
            let trailing_note_rect = trailing_note.as_ref().map(|_| {
                let nx = segmented_rect.max_x() + 12.0;
                let nw = (inner_right - nx).max(80.0);
                Rect::new(nx, row_rect.y + (row_rect.h - 16.0) / 2.0, nw, 16.0)
            });
            (
                FormControlLayout::Segmented {
                    segmented_rect,
                    trailing_note_rect,
                    labels: segments.clone(),
                    selected_index: *selected_index,
                    trailing_note: trailing_note.clone(),
                    enabled: *enabled,
                },
                (inner_right - seg_x) + 4.0,
            )
        }
        FormControl::ReadOnlyValue { value } => {
            if row.title.is_empty() {
                let value_rect = Rect::new(inner_x, row_rect.y + 9.0, inner_w, row_rect.h - 16.0);
                (
                    FormControlLayout::ReadOnlyValue {
                        value_rect,
                        value: value.clone(),
                        monospaced: false,
                    },
                    0.0,
                )
            } else {
                let vw = (value.chars().count() as f64 * 6.8 + 12.0).clamp(80.0, 260.0);
                let value_rect = Rect::new(
                    inner_right - vw,
                    row_rect.y + (row_rect.h - 17.0) / 2.0,
                    vw,
                    17.0,
                );
                let reserved = if value.is_empty() { 0.0 } else { vw + 12.0 };
                (
                    FormControlLayout::ReadOnlyValue {
                        value_rect,
                        value: value.clone(),
                        monospaced: false,
                    },
                    reserved,
                )
            }
        }
        FormControl::SecretStatus {
            is_set,
            source,
            setting_id,
        } => {
            let status_text = if *is_set {
                match source.as_deref() {
                    Some(src) if !src.is_empty() => format!("Set ({src})"),
                    _ => "Set".to_string(),
                }
            } else {
                "Not set".to_string()
            };
            let button_title = "Set in Terminal".to_string();
            let bw = (button_title.chars().count() as f64 * 6.8 + 24.0).max(116.0);
            let bh = 24.0;
            let button_rect = Rect::new(
                inner_right - bw,
                row_rect.y + (row_rect.h - bh) / 2.0,
                bw,
                bh,
            );
            let sw = (status_text.chars().count() as f64 * 6.6 + 12.0).clamp(64.0, 140.0);
            let status_rect = Rect::new(
                button_rect.x - 10.0 - sw,
                row_rect.y + (row_rect.h - 17.0) / 2.0,
                sw,
                17.0,
            );
            let reserved = (inner_right - status_rect.x) + 12.0;
            (
                FormControlLayout::SecretStatus {
                    status_rect,
                    button: FormButtonLayout {
                        rect: button_rect,
                        title: button_title,
                        enabled: row.row_enabled,
                        prominent: false,
                        action: FormButtonAction::LaunchSecretSetInTerminal {
                            setting_id: setting_id.clone(),
                        },
                    },
                    is_set: *is_set,
                    status_text,
                },
                reserved,
            )
        }
        FormControl::ColorHex {
            hex_value,
            swatch_rgb,
            enabled,
            setting_id: _,
        } => {
            let cy = row_rect.y + (row_rect.h - 24.0) / 2.0;
            let (field_rect, swatch_rect, reserved) = if row.title.is_empty() {
                (
                    Rect::new(inner_x, cy, 96.0, 24.0),
                    Rect::new(inner_x + 106.0, cy, 48.0, 24.0),
                    0.0,
                )
            } else {
                let sw_rect = Rect::new(inner_right - 48.0, cy, 48.0, 24.0);
                let fld_rect = Rect::new(sw_rect.x - 10.0 - 96.0, cy, 96.0, 24.0);
                (fld_rect, sw_rect, 166.0)
            };
            (
                FormControlLayout::ColorHex {
                    field_rect,
                    swatch_rect,
                    hex_value: hex_value.clone(),
                    placeholder: hex_value.clone(),
                    swatch_rgb: *swatch_rgb,
                    enabled: *enabled,
                },
                reserved,
            )
        }
        FormControl::ProviderHeader {
            provider_id,
            display_name,
            subtitle,
            enabled,
            enable_toggle_supported,
            enable_setting_id,
        } => {
            let icon_rect = Rect::new(inner_x, row_rect.y + 16.0, 28.0, 28.0);
            let toggle_rect = Rect::new(inner_right - 38.0, row_rect.y + 19.0, 38.0, 22.0);
            let refresh_rect = Rect::new(toggle_rect.x - 32.0, row_rect.y + 18.0, 24.0, 24.0);
            let text_left = icon_rect.max_x() + 12.0;
            let text_avail = (refresh_rect.x - 10.0 - text_left).max(100.0);
            let title_rect = Rect::new(text_left, row_rect.y + 11.0, text_avail, 20.0);
            let subtitle_rect = Rect::new(text_left, row_rect.y + 32.0, text_avail, 16.0);
            (
                FormControlLayout::ProviderHeader {
                    icon_rect,
                    title_rect,
                    subtitle_rect,
                    refresh_button: FormButtonLayout {
                        rect: refresh_rect,
                        title: "Refresh".to_string(),
                        enabled: *enable_toggle_supported && *enabled,
                        prominent: false,
                        action: FormButtonAction::RefreshProvider(Some(provider_id.clone())),
                    },
                    toggle_rect,
                    provider_id: provider_id.clone(),
                    display_name: display_name.clone(),
                    subtitle: subtitle.clone(),
                    enabled: *enabled,
                    toggle_setting_id: enable_setting_id.clone(),
                },
                0.0,
            )
        }
        FormControl::UsageWindowBar {
            title,
            percent_text,
            fill_fraction,
            accent_rgb,
            reset_text,
            pace_text,
        } => {
            let title_rect =
                Rect::new(inner_x, row_rect.y + 8.0, (inner_w - 140.0).max(80.0), 16.0);
            let reset_rect = Rect::new(inner_right - 136.0, row_rect.y + 8.0, 136.0, 16.0);
            let bar_rect = Rect::new(inner_x, row_rect.y + 28.0, inner_w, 7.0);
            let pace_rect = pace_text
                .as_ref()
                .map(|_| Rect::new(inner_x, row_rect.y + 39.0, inner_w, 16.0));
            (
                FormControlLayout::UsageWindowBar {
                    title_rect,
                    reset_rect,
                    bar_rect,
                    pace_rect,
                    title: title.clone(),
                    percent_used: (*fill_fraction * 100.0).clamp(0.0, 100.0),
                    percent_label: percent_text.clone(),
                    reset_label: reset_text.clone(),
                    pace_label: pace_text.clone(),
                    accent_rgb: *accent_rgb,
                },
                0.0,
            )
        }
        FormControl::EmptyStateCard {
            symbol,
            title,
            subtitle,
        } => {
            let icon_rect = Rect::new(
                inner_x + (inner_w - 36.0) / 2.0,
                row_rect.y + 22.0,
                36.0,
                36.0,
            );
            let title_rect = Rect::new(inner_x + 20.0, row_rect.y + 68.0, inner_w - 40.0, 22.0);
            let subtitle_rect = Rect::new(inner_x + 24.0, row_rect.y + 94.0, inner_w - 48.0, 36.0);
            (
                FormControlLayout::EmptyStateCard {
                    icon_rect,
                    title_rect,
                    subtitle_rect,
                    symbol,
                    title: title.clone(),
                    subtitle: subtitle.clone(),
                },
                0.0,
            )
        }
        FormControl::SpendSummaryGrid {
            currency_code,
            coverage_text,
            estimated_spend,
            tracked_tokens,
            subscriptions,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            reasoning_tokens,
            provenance_text,
        } => {
            let currency_rect = Rect::new(inner_x, row_rect.y + 10.0, 64.0, 18.0);
            let coverage_rect = Rect::new(inner_x + 70.0, row_rect.y + 10.0, inner_w - 70.0, 18.0);
            let box_w = ((inner_w - 20.0) / 3.0).max(90.0);
            let box_y = row_rect.y + 34.0;
            let metric_boxes = vec![
                (
                    Rect::new(inner_x, box_y, box_w, 52.0),
                    "Estimated spend".to_string(),
                    estimated_spend.clone(),
                ),
                (
                    Rect::new(inner_x + box_w + 10.0, box_y, box_w, 52.0),
                    "Tracked tokens".to_string(),
                    tracked_tokens.clone(),
                ),
                (
                    Rect::new(inner_x + 2.0 * (box_w + 10.0), box_y, box_w, 52.0),
                    "Subscriptions".to_string(),
                    subscriptions.clone(),
                ),
            ];
            let tok_w = ((inner_w - 32.0) / 5.0).max(60.0);
            let tok_y = row_rect.y + 96.0;
            let token_items = vec![
                (
                    Rect::new(inner_x, tok_y, tok_w, 38.0),
                    "Input".to_string(),
                    input_tokens.clone(),
                ),
                (
                    Rect::new(inner_x + (tok_w + 8.0), tok_y, tok_w, 38.0),
                    "Output".to_string(),
                    output_tokens.clone(),
                ),
                (
                    Rect::new(inner_x + 2.0 * (tok_w + 8.0), tok_y, tok_w, 38.0),
                    "Cache read".to_string(),
                    cache_read_tokens.clone(),
                ),
                (
                    Rect::new(inner_x + 3.0 * (tok_w + 8.0), tok_y, tok_w, 38.0),
                    "Cache write".to_string(),
                    cache_write_tokens.clone(),
                ),
                (
                    Rect::new(inner_x + 4.0 * (tok_w + 8.0), tok_y, tok_w, 38.0),
                    "Reasoning".to_string(),
                    reasoning_tokens.clone(),
                ),
            ];
            let footer_rect = Rect::new(inner_x, row_rect.y + 144.0, inner_w, 22.0);
            (
                FormControlLayout::SpendSummaryGrid {
                    currency_rect,
                    coverage_rect,
                    metric_boxes,
                    token_items,
                    footer_rect,
                    currency_label: currency_code.clone(),
                    coverage_label: coverage_text.clone(),
                    footer_note: provenance_text.clone(),
                },
                0.0,
            )
        }
    };

    let is_external_link = matches!(row.control, FormControl::ExternalLink { .. });
    let is_slider = matches!(row.control, FormControl::Slider { .. });
    let owns_own_title = matches!(
        row.control,
        FormControl::Checkbox { .. }
            | FormControl::ProviderHeader { .. }
            | FormControl::UsageWindowBar { .. }
            | FormControl::EmptyStateCard { .. }
            | FormControl::SpendSummaryGrid { .. }
    );
    let text_x = if is_external_link {
        inner_x + 24.0
    } else {
        inner_x
    };
    let text_w = (inner_w - right_reserved_w - if is_external_link { 24.0 } else { 0.0 }).max(60.0);

    let (title_rect, subtitle_rect) = if let FormControl::Checkbox { indent, .. } = &row.control {
        let sr = row.subtitle.as_ref().map(|_| {
            Rect::new(
                inner_x + *indent + 20.0,
                row_rect.y + 26.0,
                (inner_w - *indent - 20.0).max(60.0),
                (row_rect.h - 32.0).max(16.0),
            )
        });
        (None, sr)
    } else if row.title.is_empty() || owns_own_title {
        (None, None)
    } else if is_slider {
        let tr = Rect::new(text_x, row_rect.y + 8.0, text_w, 17.0);
        let sr = row
            .subtitle
            .as_ref()
            .map(|_| Rect::new(inner_x, row_rect.y + 50.0, inner_w, 16.0));
        (Some(tr), sr)
    } else if row.subtitle.is_some() {
        let tr = Rect::new(text_x, row_rect.y + 8.0, text_w, 17.0);
        let sr = Rect::new(
            text_x,
            row_rect.y + 26.0,
            text_w,
            (row_rect.h - 32.0).max(16.0),
        );
        (Some(tr), Some(sr))
    } else {
        let tr = Rect::new(text_x, row_rect.y + (row_rect.h - 17.0) / 2.0, text_w, 17.0);
        (Some(tr), None)
    };

    FormRowLayout {
        rect: row_rect,
        title: row.title.clone(),
        title_rect,
        subtitle: row.subtitle.clone(),
        subtitle_rect,
        row_enabled: row.row_enabled,
        control,
        show_bottom_divider,
    }
}

fn layout_menu_bar_preview(
    row_rect: Rect,
    scope_label: &str,
    preset_label: &str,
    preview_percent_text: &str,
    strip_chips: &[String],
    palette_groups: &[crate::settings::LayoutPaletteGroup],
    enabled: bool,
) -> MenuBarLayoutPreviewGeometry {
    let inner_x = row_rect.x + 12.0;
    let inner_w = row_rect.w - 24.0;
    let top_y = row_rect.y + 8.0;

    let scope_label_rect = Rect::new(inner_x, top_y + 3.0, 96.0, 16.0);
    let scope_picker_rect = Rect::new(inner_x + 98.0, top_y, 136.0, 22.0);
    let preset_picker_rect = Rect::new(inner_x + inner_w - 140.0, top_y, 140.0, 22.0);
    let preset_label_rect = Rect::new(preset_picker_rect.x - 52.0, top_y + 3.0, 48.0, 16.0);

    let preview_title_rect = Rect::new(inner_x, top_y + 26.0, 180.0, 14.0);
    let preview_box_rect = Rect::new(inner_x, top_y + 42.0, inner_w, 32.0);
    let preview_pill_rect = Rect::new(
        preview_box_rect.x + 10.0,
        preview_box_rect.y + 4.0,
        74.0,
        24.0,
    );
    let reset_button_rect = Rect::new(
        preview_box_rect.max_x() - 64.0,
        preview_box_rect.y + 5.0,
        56.0,
        22.0,
    );

    let strip_title_rect = Rect::new(inner_x, preview_box_rect.max_y() + 6.0, 120.0, 14.0);
    let strip_box_rect = Rect::new(inner_x, strip_title_rect.max_y() + 2.0, inner_w, 32.0);
    let line_label_rect = Rect::new(strip_box_rect.x + 8.0, strip_box_rect.y + 8.0, 42.0, 16.0);
    let mut strip_chip_layouts = Vec::with_capacity(strip_chips.len());
    let mut cx = line_label_rect.max_x() + 6.0;
    for chip in strip_chips {
        let cw = (chip.chars().count() as f64 * 6.8 + 22.0).max(52.0);
        strip_chip_layouts.push(TokenChipLayout {
            rect: Rect::new(cx, strip_box_rect.y + 5.0, cw, 22.0),
            label: chip.clone(),
            active_strip: true,
        });
        cx += cw + 8.0;
    }
    let remove_button_rect = Rect::new(
        strip_box_rect.max_x() - 148.0,
        strip_box_rect.y + 5.0,
        140.0,
        22.0,
    );

    let mut group_layouts = Vec::with_capacity(palette_groups.len());
    let mut gy = strip_box_rect.max_y() + 8.0;
    for group in palette_groups {
        let title_rect = Rect::new(inner_x, gy, 100.0, 14.0);
        gy += 15.0;
        let mut g_chips = Vec::with_capacity(group.chips.len());
        let mut gx = inner_x;
        for chip in &group.chips {
            let cw = (chip.chars().count() as f64 * 6.4 + 18.0).max(32.0);
            if gx + cw > inner_x + inner_w && gx > inner_x {
                gx = inner_x;
                gy += 21.0;
            }
            g_chips.push(TokenChipLayout {
                rect: Rect::new(gx, gy, cw, 19.0),
                label: chip.clone(),
                active_strip: false,
            });
            gx += cw + 5.0;
        }
        gy += 22.0;
        group_layouts.push(PaletteGroupLayout {
            title_rect,
            title: group.title.clone(),
            chips: g_chips,
        });
    }

    let conditionals_title_rect = Rect::new(inner_x, gy, 90.0, 14.0);
    let new_conditional_button_rect = Rect::new(inner_x + 92.0, gy - 2.0, 128.0, 20.0);
    gy += 22.0;

    let size_label_rect = Rect::new(inner_x, gy + 3.0, 32.0, 16.0);
    let size_popup_rect = Rect::new(inner_x + 34.0, gy, 92.0, 22.0);
    let gap_label_rect = Rect::new(size_popup_rect.max_x() + 10.0, gy + 3.0, 30.0, 16.0);
    let gap_popup_rect = Rect::new(gap_label_rect.max_x() + 2.0, gy, 88.0, 22.0);
    let vertical_label_rect = Rect::new(gap_popup_rect.max_x() + 10.0, gy + 3.0, 50.0, 16.0);
    let vertical_value_rect = Rect::new(vertical_label_rect.max_x() + 2.0, gy + 3.0, 32.0, 16.0);
    let vertical_stepper_rect = Rect::new(vertical_value_rect.max_x() + 2.0, gy, 15.0, 22.0);
    let monospaced_checkbox_rect =
        Rect::new(vertical_stepper_rect.max_x() + 10.0, gy + 2.0, 134.0, 18.0);
    let window_labels_checkbox_rect = Rect::new(
        (inner_x + inner_w - 140.0).max(monospaced_checkbox_rect.max_x() + 4.0),
        gy + 2.0,
        140.0,
        18.0,
    );

    MenuBarLayoutPreviewGeometry {
        scope_label_rect,
        scope_picker_rect,
        scope_label: scope_label.to_string(),
        preset_label_rect,
        preset_picker_rect,
        preset_label: preset_label.to_string(),
        preview_title_rect,
        preview_box_rect,
        preview_pill_rect,
        preview_percent_text: preview_percent_text.to_string(),
        reset_button_rect,
        strip_title_rect,
        strip_box_rect,
        line_label_rect,
        strip_chips: strip_chip_layouts,
        remove_button_rect,
        palette_groups: group_layouts,
        conditionals_title_rect,
        new_conditional_button_rect,
        size_label_rect,
        size_popup_rect,
        gap_label_rect,
        gap_popup_rect,
        vertical_label_rect,
        vertical_value_rect,
        vertical_stepper_rect,
        monospaced_checkbox_rect,
        window_labels_checkbox_rect,
        enabled,
    }
}

/// Hit-tests a point `(px, py)` in window coordinates against [`SettingsWindowLayout`].
pub fn hit_test(layout: &SettingsWindowLayout, point: (f64, f64)) -> Option<SettingsHitTarget> {
    let pt = Point::new(point.0, point.1);

    if layout.resize_handle_rect.contains(pt) {
        return Some(SettingsHitTarget::SidebarResizeHandle);
    }

    // Sidebar region
    if layout.sidebar.frame.contains(pt) {
        if let Some(clear_rect) = layout.sidebar.search_bar.clear_button_rect
            && clear_rect.contains(pt)
        {
            return Some(SettingsHitTarget::SidebarClearSearch);
        }
        if layout.sidebar.search_bar.sort_button_rect.contains(pt) {
            return Some(SettingsHitTarget::SidebarSortToggle);
        }
        if layout.sidebar.search_bar.filter_button_rect.contains(pt) {
            return Some(SettingsHitTarget::SidebarFilterToggle);
        }
        if let Some(clear_filters) = layout.sidebar.clear_filters_button_rect
            && clear_filters.contains(pt)
        {
            return Some(SettingsHitTarget::SidebarClearFilters);
        }
        for row in &layout.sidebar.app_rows {
            if row.row_rect.contains(pt) {
                return Some(SettingsHitTarget::SidebarPane(row.pane.clone()));
            }
        }
        for prow in &layout.sidebar.provider_rows {
            if prow.row_rect.contains(pt) {
                return Some(SettingsHitTarget::SidebarPane(SettingsPane::Provider(
                    prow.provider_id.clone(),
                )));
            }
        }
        return None;
    }

    // Detail header bar
    if layout.detail_header.frame.contains(pt) {
        if layout.detail_header.can_go_back && layout.detail_header.back_button_rect.contains(pt) {
            return Some(SettingsHitTarget::NavBack);
        }
        if layout.detail_header.can_go_forward
            && layout.detail_header.forward_button_rect.contains(pt)
        {
            return Some(SettingsHitTarget::NavForward);
        }
        return None;
    }

    // Detail scroll viewport
    if !layout.detail_viewport_rect.contains(pt) {
        return None;
    }

    for section in &layout.sections {
        for row in &section.rows {
            if !row.rect.contains(pt) {
                continue;
            }
            match &row.control {
                FormControlLayout::Toggle {
                    rect,
                    value,
                    enabled: true,
                    setting_id: Some(id),
                }
                | FormControlLayout::Checkbox {
                    rect,
                    value,
                    enabled: true,
                    setting_id: Some(id),
                } if rect.contains(pt) => {
                    return Some(SettingsHitTarget::ToggleSetting {
                        setting_id: id.clone(),
                        new_value: !*value,
                    });
                }
                FormControlLayout::Choice {
                    rect,
                    selected_label,
                    options,
                    enabled: true,
                    setting_id: Some(id),
                }
                | FormControlLayout::BorderedChoice {
                    rect,
                    selected_label,
                    options,
                    enabled: true,
                    setting_id: Some(id),
                } if rect.contains(pt) => {
                    return Some(SettingsHitTarget::ChoicePopup {
                        rect: *rect,
                        setting_id: id.clone(),
                        options: options.clone(),
                        selected_label: selected_label.clone(),
                    });
                }
                FormControlLayout::Buttons { buttons } => {
                    for btn in buttons {
                        if btn.enabled && btn.rect.contains(pt) {
                            return Some(SettingsHitTarget::ButtonAction(btn.action.clone()));
                        }
                    }
                }
                FormControlLayout::SecretStatus { button, .. } => {
                    if button.enabled && button.rect.contains(pt) {
                        return Some(SettingsHitTarget::ButtonAction(button.action.clone()));
                    }
                }
                FormControlLayout::ProviderHeader {
                    refresh_button,
                    toggle_rect,
                    enabled,
                    toggle_setting_id,
                    ..
                } => {
                    if refresh_button.enabled && refresh_button.rect.contains(pt) {
                        return Some(SettingsHitTarget::ButtonAction(
                            refresh_button.action.clone(),
                        ));
                    }
                    if toggle_rect.contains(pt)
                        && let Some(id) = toggle_setting_id
                    {
                        return Some(SettingsHitTarget::ToggleSetting {
                            setting_id: id.clone(),
                            new_value: !*enabled,
                        });
                    }
                }
                FormControlLayout::ExternalLink { url, .. } => {
                    return Some(SettingsHitTarget::ExternalUrl(url.clone()));
                }
                _ => {}
            }
        }
    }

    if let Some(footer) = &layout.general_footer
        && footer.quit_button.enabled
        && footer.quit_button.rect.contains(pt)
    {
        return Some(SettingsHitTarget::ButtonAction(
            footer.quit_button.action.clone(),
        ));
    }

    None
}
