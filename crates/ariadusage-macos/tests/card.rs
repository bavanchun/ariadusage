mod common;

use ariadusage_macos::card::{
    Accent, CardPreferences, CardPresentation, CardTitles, PercentStyle, Rgb, SubtitleStyle,
    card_model, card_presentation, provider_card,
};
use ariadusage_macos::progress_bar::WorkdayTickAppearance;
use ariadusage_protocol::ids::ProviderId;
use ariadusage_protocol::metric::{Confidence, Metric, MetricError};
use ariadusage_protocol::snapshot::{ProviderPresentation, ProviderSnapshot};
use ariadusage_protocol::usage::{
    Credits, Identity, NamedWindow, Pace, PaceStage, ProviderError, ProviderErrorCategory,
    ProviderErrorKind, RateWindow,
};
use common::{FixedDates, ts};
use jiff::Timestamp;

const NOW: &str = "2026-10-23T03:00:00Z";

fn now() -> Timestamp {
    ts(NOW)
}

fn value<T>(v: T) -> Metric<T> {
    Metric::value(v, None, None, Confidence::Exact)
}

fn window(used: f64, minutes: u32, resets_in_seconds: i64) -> RateWindow {
    RateWindow {
        window_minutes: Some(minutes),
        resets_at: Some(now() + jiff::SignedDuration::from_secs(resets_in_seconds)),
        ..RateWindow::new(used)
    }
}

fn provider(id: &str, name: &str) -> ProviderSnapshot {
    let mut p = ProviderSnapshot::new(ProviderId::new(id).unwrap(), name, true, "cli");
    p.updated_at = Some(now() - jiff::SignedDuration::from_secs(5 * 60));
    p
}

fn claude_with_session() -> ProviderSnapshot {
    let mut p = provider("claude", "Claude");
    p.windows.primary = Some(value(window(42.0, 300, 2 * 3600 + 13 * 60)));
    p
}

fn build(p: &ProviderSnapshot, prefs: &CardPreferences) -> ariadusage_macos::card::CardModel {
    provider_card(p, prefs, now(), &FixedDates::utc())
}

fn invented_email() -> String {
    format!("{}@{}", "someone", "example.invalid")
}

#[test]
fn session_row_shows_title_remaining_percent_and_countdown() {
    let card = build(&claude_with_session(), &CardPreferences::default());
    assert_eq!(card.provider_name, "Claude");
    assert_eq!(card.metrics.len(), 1);
    let row = &card.metrics[0];
    assert_eq!(row.id, "primary");
    assert_eq!(row.title_text(), "Session 58% left");
    assert_eq!(row.percent, 58.0);
    assert_eq!(row.reset_text.as_deref(), Some("Resets in 2h 13m"));
    assert_eq!(row.percent_style, PercentStyle::Left);
    assert!(row.pace_percent.is_none());
    assert!(row.warning_marker_percents.is_empty());
    assert_eq!(card.subtitle.text, "Updated 5 min ago");
    assert_eq!(card.subtitle.style, SubtitleStyle::Info);
    assert_eq!(card.accent, Accent::System);
    assert!(card.placeholder.is_none());
}

#[test]
fn used_mode_and_absolute_resets_follow_preferences() {
    let prefs = CardPreferences {
        show_used: true,
        reset_times_show_absolute: true,
        ..CardPreferences::default()
    };
    let card = build(&claude_with_session(), &prefs);
    let row = &card.metrics[0];
    assert_eq!(row.title_text(), "Session 42% used");
    assert_eq!(row.percent, 42.0);
    assert_eq!(row.reset_text.as_deref(), Some("Resets 5:13 AM"));
}

#[test]
fn header_shows_email_and_cleaned_plan_from_identity() {
    let mut p = claude_with_session();
    p.identity = Some(value(Identity {
        provider_id: ProviderId::new("claude").unwrap(),
        account_email: Some(invented_email()),
        account_organization: None,
        login_method: Some("Claude Max".to_string()),
        account_id: None,
        plan: None,
    }));
    let card = build(&p, &CardPreferences::default());
    assert_eq!(card.email, invented_email());
    assert_eq!(card.plan.as_deref(), Some("Max"));
}

#[test]
fn header_has_no_email_or_plan_without_identity() {
    let card = build(&claude_with_session(), &CardPreferences::default());
    assert!(card.email.is_empty());
    assert!(card.plan.is_none());
}

#[test]
fn weekly_row_carries_engine_pace_and_markers() {
    let mut p = provider("codex", "Codex");
    let mut weekly = window(50.0, 10080, 4 * 24 * 3600);
    weekly.pace = Some(Pace {
        stage: PaceStage::Ahead,
        delta_percent: 7.0,
        expected_used_percent: 43.0,
        will_last_to_reset: false,
        eta_seconds: Some(3 * 24 * 3600),
        speed_multiplier_to_reset: None,
        run_out_probability: None,
        summary: "ignored".to_string(),
    });
    p.windows.secondary = Some(value(weekly));
    let prefs = CardPreferences {
        work_days_per_week: Some(5),
        weekly_warning_thresholds: vec![20],
        ..CardPreferences::default()
    };
    let card = build(&p, &prefs);
    let row = &card.metrics[0];
    assert_eq!(row.id, "secondary");
    assert_eq!(row.title_text(), "Weekly 50% left");
    assert_eq!(row.reset_text.as_deref(), Some("Resets in 4d"));
    assert_eq!(
        row.meta_text().as_deref(),
        Some("7% in deficit · Runs out in 3d")
    );
    assert_eq!(row.pace_percent, Some(57.0));
    assert!(!row.pace_on_top);
    assert_eq!(row.warning_marker_percents, [20.0]);
    assert_eq!(row.workday_marker_percents, [20.0, 40.0, 60.0, 80.0]);
    assert_eq!(row.workday_tick_appearance, WorkdayTickAppearance::Subtle);
}

#[test]
fn on_track_pace_has_text_but_no_tip_and_hidden_pace_clears_both() {
    let mut p = provider("claude", "Claude");
    let mut weekly = window(40.0, 10080, 4 * 24 * 3600);
    weekly.pace = Some(Pace {
        stage: PaceStage::OnTrack,
        delta_percent: -1.0,
        expected_used_percent: 41.0,
        will_last_to_reset: true,
        eta_seconds: None,
        speed_multiplier_to_reset: None,
        run_out_probability: None,
        summary: String::new(),
    });
    p.windows.secondary = Some(value(weekly));
    let card = build(&p, &CardPreferences::default());
    let row = &card.metrics[0];
    assert_eq!(
        row.meta_text().as_deref(),
        Some("On pace · Lasts until reset")
    );
    assert_eq!(row.pace_percent, None);
    assert!(row.pace_on_top);

    let hidden = build(
        &p,
        &CardPreferences {
            pace_visible: false,
            ..CardPreferences::default()
        },
    );
    assert_eq!(hidden.metrics[0].meta_text(), None);
}

#[test]
fn exhausted_weekly_window_has_no_pace() {
    let mut p = provider("claude", "Claude");
    let mut weekly = window(100.0, 10080, 3600);
    weekly.pace = Some(Pace {
        stage: PaceStage::FarAhead,
        delta_percent: 40.0,
        expected_used_percent: 60.0,
        will_last_to_reset: false,
        eta_seconds: Some(0),
        speed_multiplier_to_reset: None,
        run_out_probability: None,
        summary: String::new(),
    });
    p.windows.secondary = Some(value(weekly));
    let card = build(&p, &CardPreferences::default());
    assert_eq!(card.metrics[0].meta_text(), None);
    assert_eq!(card.metrics[0].title_text(), "Weekly 0% left");
}

#[test]
fn tertiary_row_needs_a_title_from_the_presentation() {
    let mut p = claude_with_session();
    p.windows.tertiary = Some(value(window(10.0, 10080, 3 * 24 * 3600)));
    let fallback = build(&p, &CardPreferences::default());
    assert_eq!(fallback.metrics.len(), 1);

    let presentation = CardPresentation {
        titles: CardTitles {
            session: "Session".to_string(),
            weekly: "Weekly".to_string(),
            tertiary: Some("Sonnet".to_string()),
        },
        accent: Accent::Brand(Rgb::from_hex("#CC7C5E").unwrap()),
    };
    let card = card_model(
        &p,
        &presentation,
        &CardPreferences::default(),
        now(),
        &FixedDates::utc(),
    );
    assert_eq!(card.metrics.len(), 2);
    assert_eq!(card.metrics[1].title_text(), "Sonnet 90% left");
    assert_eq!(
        card.accent,
        Accent::Brand(Rgb {
            r: 204,
            g: 124,
            b: 94
        })
    );
}

#[test]
fn presentation_falls_back_to_session_weekly_and_system_accent() {
    let presentation = card_presentation(&claude_with_session());
    assert_eq!(presentation.titles, CardTitles::default());
    assert_eq!(presentation.titles.session, "Session");
    assert_eq!(presentation.titles.weekly, "Weekly");
    assert_eq!(presentation.titles.tertiary, None);
    assert_eq!(presentation.accent, Accent::System);
}

fn presentation_block(tertiary: Option<&str>, accent: Option<&str>) -> ProviderPresentation {
    ProviderPresentation::new(
        "Anti",
        "Gemini Models",
        "Claude and GPT",
        tertiary.map(str::to_string),
        accent.map(str::to_string),
    )
    .unwrap()
}

#[test]
fn presentation_block_supplies_titles_and_accent() {
    let mut p = claude_with_session();
    p.presentation = Some(presentation_block(Some("Sonnet"), Some("#CC7C5E")));
    let presentation = card_presentation(&p);
    assert_eq!(presentation.titles.session, "Gemini Models");
    assert_eq!(presentation.titles.weekly, "Claude and GPT");
    assert_eq!(presentation.titles.tertiary.as_deref(), Some("Sonnet"));
    assert_eq!(
        presentation.accent,
        Accent::Brand(Rgb {
            r: 204,
            g: 124,
            b: 94
        })
    );

    let without_accent = Some(presentation_block(None, None));
    p.presentation = without_accent;
    let presentation = card_presentation(&p);
    assert_eq!(presentation.titles.tertiary, None);
    assert_eq!(presentation.accent, Accent::System);
}

#[test]
fn claude_card_titles_rows_from_its_presentation_block() {
    let mut p = claude_with_session();
    p.windows.secondary = Some(value(window(30.0, 10080, 3 * 24 * 3600)));
    p.windows.tertiary = Some(value(window(10.0, 10080, 3 * 24 * 3600)));
    p.presentation = Some(
        ProviderPresentation::new(
            "Claude",
            "Session",
            "Weekly",
            Some("Sonnet".to_string()),
            Some("#CC7C5E".to_string()),
        )
        .unwrap(),
    );
    let card = build(&p, &CardPreferences::default());
    let titles: Vec<String> = card.metrics.iter().map(|m| m.title_text()).collect();
    assert_eq!(
        titles,
        ["Session 58% left", "Weekly 70% left", "Sonnet 90% left"]
    );
    assert_eq!(
        card.accent,
        Accent::Brand(Rgb {
            r: 204,
            g: 124,
            b: 94
        })
    );
}

#[test]
fn rgb_parses_only_hash_rrggbb() {
    assert_eq!(
        Rgb::from_hex("#49A3B0"),
        Some(Rgb {
            r: 73,
            g: 163,
            b: 176
        })
    );
    assert_eq!(Rgb::from_hex("49A3B0"), None);
    assert_eq!(Rgb::from_hex("#49A3B"), None);
    assert_eq!(Rgb::from_hex("#49A3BZ"), None);
}

#[test]
fn extra_windows_keep_engine_titles_and_mark_unknown_usage_unavailable() {
    let mut p = claude_with_session();
    let mut unknown_with_reset = Metric::unknown();
    unknown_with_reset.value = Some(window(0.0, 60, 2 * 3600));
    p.windows.extra = vec![
        NamedWindow {
            id: "burst".to_string(),
            title: "1-Hour Burst".to_string(),
            window: value(window(15.0, 60, 45 * 60)),
        },
        NamedWindow {
            id: "unknown".to_string(),
            title: "Daily routines".to_string(),
            window: Metric::unknown(),
        },
        NamedWindow {
            id: "unknown-with-reset".to_string(),
            title: "Quota summary".to_string(),
            window: unknown_with_reset,
        },
        NamedWindow {
            id: "loading".to_string(),
            title: "Loading".to_string(),
            window: Metric::loading(None),
        },
    ];
    let card = build(&p, &CardPreferences::default());
    let ids: Vec<&str> = card.metrics.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["primary", "burst", "unknown", "unknown-with-reset"]);
    assert_eq!(card.metrics[1].title_text(), "1-Hour Burst 85% left");
    assert_eq!(card.metrics[1].reset_text.as_deref(), Some("Resets in 45m"));
    assert_eq!(card.metrics[2].status_text.as_deref(), Some("Unavailable"));
    assert_eq!(
        card.metrics[3].status_text.as_deref(),
        Some("Unavailable - Resets in 2h")
    );
    assert_eq!(card.metrics[3].reset_text, None);
}

#[test]
fn claude_scoped_weekly_extra_windows_format_as_model_weekly() {
    let mut p = claude_with_session();
    p.windows.extra = vec![
        NamedWindow {
            id: "claude-weekly-scoped-fable".to_string(),
            title: "Fable only".to_string(),
            window: value(window(20.0, 10080, 3 * 24 * 3600)),
        },
        NamedWindow {
            id: "claude-weekly-scoped-opus".to_string(),
            title: "Opus 4   ONLY  ".to_string(),
            window: value(window(40.0, 10080, 3 * 24 * 3600)),
        },
        NamedWindow {
            id: "claude-weekly-scoped-sonnet".to_string(),
            title: "Sonnet".to_string(),
            window: value(window(10.0, 10080, 3 * 24 * 3600)),
        },
    ];
    let card = build(&p, &CardPreferences::default());
    let titles: Vec<&str> = card.metrics.iter().map(|m| m.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Session", "Fable weekly", "Opus 4 weekly", "Sonnet weekly"]
    );

    let mut non_claude = provider("codex", "Codex");
    non_claude.windows.extra = vec![NamedWindow {
        id: "claude-weekly-scoped-fable".to_string(),
        title: "Fable only".to_string(),
        window: value(window(20.0, 10080, 3 * 24 * 3600)),
    }];
    let codex_card = build(&non_claude, &CardPreferences::default());
    assert_eq!(codex_card.metrics[0].title, "Fable only");
}

#[test]
fn claude_daily_routines_and_optional_extras_respect_visibility_preferences() {
    let mut p = claude_with_session();
    p.windows.extra = vec![
        NamedWindow {
            id: "claude-weekly-scoped-fable".to_string(),
            title: "Fable only".to_string(),
            window: value(window(20.0, 10080, 3 * 24 * 3600)),
        },
        NamedWindow {
            id: "claude-routines".to_string(),
            title: "Daily Routines".to_string(),
            window: value(window(0.0, 10080, 3 * 24 * 3600)),
        },
    ];

    let default_card = build(&p, &CardPreferences::default());
    let default_ids: Vec<&str> = default_card.metrics.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        default_ids,
        ["primary", "claude-weekly-scoped-fable", "claude-routines"]
    );

    let routines_hidden = CardPreferences {
        claude_daily_routines_usage_visible: false,
        ..CardPreferences::default()
    };
    let card = build(&p, &routines_hidden);
    let ids: Vec<&str> = card.metrics.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["primary", "claude-weekly-scoped-fable"]);

    let extras_hidden = CardPreferences {
        show_optional_credits_and_extra_usage: false,
        ..CardPreferences::default()
    };
    let card = build(&p, &extras_hidden);
    let ids: Vec<&str> = card.metrics.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["primary", "claude-weekly-scoped-fable"]);
}

#[test]
fn loading_provider_shows_refreshing_and_no_rows() {
    let mut p = provider("claude", "Claude");
    p.updated_at = None;
    p.windows.primary = Some(Metric::loading(None));
    let card = build(&p, &CardPreferences::default());
    assert_eq!(card.subtitle.text, "Refreshing…");
    assert_eq!(card.subtitle.style, SubtitleStyle::Loading);
    assert!(card.metrics.is_empty());
    assert!(card.placeholder.is_none());
    assert!(!card.has_details());
}

#[test]
fn error_wins_over_refreshing_and_hides_failed_windows() {
    let mut p = provider("codex", "Codex");
    p.windows.primary = Some(Metric::error(
        MetricError::new("api", "Provider API failure"),
        None,
    ));
    p.windows.secondary = Some(Metric::loading(None));
    p.last_error = Some(ProviderError {
        kind: ProviderErrorKind::RateLimited,
        category: ProviderErrorCategory::Api,
        message: "  Rate limit encountered on provider \n".to_string(),
        retry_after_seconds: Some(60),
    });
    let card = build(&p, &CardPreferences::default());
    assert_eq!(card.subtitle.text, "Rate limit encountered on provider");
    assert_eq!(card.subtitle.style, SubtitleStyle::Error);
    assert!(card.metrics.is_empty());
}

#[test]
fn never_fetched_provider_says_so_and_shows_the_placeholder() {
    let mut p = provider("antigravity", "Antigravity");
    p.updated_at = None;
    let card = build(&p, &CardPreferences::default());
    assert_eq!(card.subtitle.text, "Not fetched yet");
    assert_eq!(card.placeholder.as_deref(), Some("No usage yet"));
    assert!(card.has_details());
    assert!(!card.uses_stacked_detail_layout());
}

#[test]
fn stale_windows_still_render_their_values() {
    let mut p = provider("claude", "Claude");
    p.windows.primary = Some(Metric::stale(
        window(42.0, 300, 3600),
        None,
        None,
        Confidence::Exact,
        None,
    ));
    let card = build(&p, &CardPreferences::default());
    assert_eq!(card.metrics[0].title_text(), "Session 58% left");
}

#[test]
fn credits_show_balance_bar_and_scale() {
    let mut p = provider("codex", "Codex");
    p.credits = Some(value(Credits::new(42.5)));
    let card = build(&p, &CardPreferences::default());
    assert!(card.uses_stacked_detail_layout());
    let credits = card.credits.expect("credits");
    assert_eq!(credits.text, "42.5 left");
    assert!(credits.show_progress);
    assert_eq!(credits.progress_percent, Some(42.5));
    assert_eq!(credits.scale_text.as_deref(), Some("100 tokens"));
}

#[test]
fn large_and_zero_credit_balances_scale_to_the_next_power_of_ten() {
    let mut p = provider("codex", "Codex");
    for (remaining, percent, scale) in [
        (1234.0, 12.34, "10K tokens"),
        (0.0, 0.0, "1 tokens"),
        (1000.0, 10.0, "10K tokens"),
    ] {
        p.credits = Some(value(Credits {
            balance_is_workspace: Some(false),
            ..Credits::new(remaining)
        }));
        let credits = build(&p, &CardPreferences::default()).credits.unwrap();
        assert!((credits.progress_percent.unwrap() - percent).abs() < 1e-9);
        assert_eq!(credits.scale_text.as_deref(), Some(scale));
    }
}

#[test]
fn workspace_credit_balances_have_no_bar() {
    let mut p = provider("codex", "Codex");
    p.credits = Some(value(Credits {
        balance_is_workspace: Some(true),
        ..Credits::new(1234.56)
    }));
    let credits = build(&p, &CardPreferences::default()).credits.unwrap();
    assert_eq!(credits.text, "1234.56 left");
    assert!(!credits.show_progress);
    assert_eq!(credits.progress_percent, None);
    assert_eq!(credits.scale_text, None);
}

#[test]
fn credits_error_shows_its_message() {
    let mut p = provider("codex", "Codex");
    p.credits = Some(Metric::error(
        MetricError::new("unavailable", "Credits unavailable"),
        None,
    ));
    let credits = build(&p, &CardPreferences::default()).credits.unwrap();
    assert_eq!(credits.text, "Credits unavailable");
    assert_eq!(credits.progress_percent, None);
}

#[test]
fn session_warning_markers_flip_in_used_mode() {
    let prefs = CardPreferences {
        show_used: true,
        session_warning_thresholds: vec![50, 20],
        ..CardPreferences::default()
    };
    let card = build(&claude_with_session(), &prefs);
    assert_eq!(card.metrics[0].warning_marker_percents, [50.0, 80.0]);
}

#[cfg(unix)]
#[test]
fn fixture_cards_populate_session_equivalent_renewal_reset_credits_dashboard_and_costs() {
    let snap = ariadusage_protocol::fixture::generate_scenario_snapshot(0);
    let claude = snap
        .providers
        .iter()
        .find(|p| p.id.as_str() == "claude")
        .unwrap();
    let codex = snap
        .providers
        .iter()
        .find(|p| p.id.as_str() == "codex")
        .unwrap();
    let ref_now: Timestamp = "2026-10-06T12:52:00Z".parse().unwrap();

    let claude_card = provider_card(
        claude,
        &CardPreferences::default(),
        ref_now,
        &FixedDates::utc(),
    );
    assert!(claude_card.shows_submenu_indicator);
    assert_eq!(claude_card.usage_notes, ["Plan renews: Nov 1, 2026"]);
    assert_eq!(
        claude_card.metrics[1].meta_text().as_deref(),
        Some(
            "9% in deficit · Runs out in 2d (75% risk) · Est. 2.4 session quotas left · 16 windows until reset"
        )
    );
    assert!(claude_card.inline_usage_dashboard.is_some());
    let claude_dash = claude_card.inline_usage_dashboard.as_ref().unwrap();
    assert_eq!(claude_dash.kpis.len(), 6);
    assert_eq!(claude_dash.points.len(), 30);
    assert!(!claude_dash.recent_windows.is_empty());
    assert!(claude_card.cloud_credits.is_some());
    assert!(claude_card.provider_cost.is_some());

    let codex_card = provider_card(
        codex,
        &CardPreferences::default(),
        ref_now,
        &FixedDates::utc(),
    );
    assert!(codex_card.shows_submenu_indicator);
    assert_eq!(codex_card.usage_notes, ["Plan renews: Nov 15, 2026"]);
    let reset_credits = codex_card.limit_reset_credits.as_ref().unwrap();
    assert_eq!(reset_credits.count_text, "2 available");
    assert_eq!(reset_credits.expiry_text.as_deref(), Some("13d 11h"));
    assert!(codex_card.inline_usage_dashboard.is_some());
    assert!(codex_card.credits.is_some());
}
