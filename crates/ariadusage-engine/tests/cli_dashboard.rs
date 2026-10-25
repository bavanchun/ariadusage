// Ported from CodexBar Tests/CodexBarTests/DashboardAntigravityWindowTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/DashboardClaudeSwapSnapshotTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/DashboardManagedCodexCollectionTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/DashboardManagedCodexSnapshotTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/DashboardReportedCostTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/DashboardSnapshotBuilderTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/DashboardAccountInventoryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/DashboardAccountPrivacyTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/DashboardClaudeSwapCollectionTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/DashboardMultiAccountTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/DashboardServeScopeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use ariadusage_core::config::{
    Config, ProviderConfig, ProviderEntry, TokenAccountMeta, TokenAccountsMeta,
};
use ariadusage_core::model::{
    NamedRateWindow, ProviderIdentitySnapshot, RateWindow, UsageSnapshot,
};
use ariadusage_engine::providers::codex::CreditsSnapshot;
use ariadusage_engine::serve::accounts::{
    CodexActiveSource, FileManagedCodexAccountStore, ManagedCodexAccount, ManagedCodexAccountSet,
};
use ariadusage_engine::serve::coordinator::CliServeOperationCoordinator;
use ariadusage_engine::serve::dashboard::{
    CodexVisibleAccount, CostDailyEntryPayload, CostPayload, CostProjectPayload,
    CostUsageTokenSnapshot, DashboardAccountsInput, DashboardIdentityMode,
    DashboardManagedCodexAccounts, DashboardSnapshotBuilder, DashboardSnapshotPayload,
    DashboardSnapshotProducer, DashboardUsageAccount, ProviderAccountIdentity,
    ProviderAccountUsageSnapshot, ProviderErrorPayload, ProviderPayload, ProviderStatusPayload,
    UsageCommandOutput, collect_account_usage, dashboard_claude_swap_is_eligible,
    serve_includes_configured_accounts, write_dashboard_snapshot_atomically,
};
use ariadusage_engine::serve::http_server::CliLocalHttpResponse;
use ariadusage_engine::serve::{
    CliServeResponseCache, cached_serve_response, decode_dashboard_identity_mode,
    decode_dashboard_output_path, resolve_dashboard_identity_mode, serve_collect_usage_outputs,
    serve_dashboard_operation_key, serve_usage_operation_fingerprint,
};
use ariadusage_protocol::{Confidence, ProviderId, StatusIndicator};

fn pid(slug: &str) -> ProviderId {
    ProviderId::new(slug).unwrap()
}

fn rate_window(
    used_percent: f64,
    window_minutes: Option<i64>,
    resets_at: Option<Timestamp>,
    reset_description: Option<String>,
) -> RateWindow {
    RateWindow::new(
        used_percent,
        window_minutes,
        resets_at,
        reset_description,
        None,
        false,
    )
    .unwrap()
}

fn named_window(
    id: impl Into<String>,
    title: impl Into<String>,
    window: RateWindow,
    usage_known: bool,
) -> NamedRateWindow {
    NamedRateWindow::new(id, title, window).with_usage_known(usage_known)
}

#[allow(clippy::too_many_arguments)]
fn usage_snapshot(
    primary: Option<RateWindow>,
    secondary: Option<RateWindow>,
    tertiary: Option<RateWindow>,
    extra_rate_windows: Option<Vec<NamedRateWindow>>,
    provider_cost: Option<ariadusage_core::model::ProviderCostSnapshot>,
    subscription_expires_at: Option<Timestamp>,
    subscription_renews_at: Option<Timestamp>,
    updated_at: Timestamp,
    identity: Option<ProviderIdentitySnapshot>,
) -> UsageSnapshot {
    UsageSnapshot::new(
        primary,
        secondary,
        tertiary,
        extra_rate_windows,
        provider_cost,
        vec![],
        subscription_expires_at,
        subscription_renews_at,
        updated_at,
        identity,
        Confidence::Exact,
    )
    .unwrap()
}

fn empty_config() -> Config {
    Config::new(1, vec![])
}

fn make_token_account_meta(
    id: impl Into<String>,
    label: impl Into<String>,
    added_at: f64,
    last_used: Option<f64>,
) -> TokenAccountMeta {
    TokenAccountMeta {
        id: id.into(),
        label: label.into(),
        added_at,
        last_used,
        external_identifier: None,
        usage_scope: None,
        organization_id: None,
        workspace_id: None,
        claude_config_directory: None,
        seat_credit_entitlement: None,
        model_rate_limits: None,
        extra: BTreeMap::new(),
    }
}

fn make_token_accounts_meta(
    version: u32,
    active_index: usize,
    accounts: Vec<TokenAccountMeta>,
) -> TokenAccountsMeta {
    TokenAccountsMeta {
        version,
        active_index: active_index as i64,
        accounts,
    }
}
use jiff::Timestamp;
use tempfile::TempDir;
use tokio::sync::Notify;
use uuid::Uuid;

// ============================================================================
// Shared Test Helpers & ClaudeSwapAccountProjection
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeSwapUsageStatus {
    Ok,
    TokenExpired,
    NoCredentials,
    Unavailable,
}

#[derive(Debug, Clone)]
struct ClaudeSwapUsageWindow {
    used_percent: f64,
    resets_at: Option<Timestamp>,
}

#[derive(Debug, Clone)]
struct ClaudeSwapScopedUsageWindow {
    name: String,
    used_percent: f64,
    resets_at: Option<Timestamp>,
}

#[derive(Debug, Clone)]
struct ClaudeSwapAccountRow {
    number: i64,
    email: String,
    organization_name: String,
    alias: Option<String>,
    is_active: bool,
    usage_status: ClaudeSwapUsageStatus,
    five_hour: Option<ClaudeSwapUsageWindow>,
    seven_day: Option<ClaudeSwapUsageWindow>,
    scoped: Vec<ClaudeSwapScopedUsageWindow>,
}

#[derive(Debug, Clone)]
struct ClaudeSwapAccountList {
    active_account_number: Option<i64>,
    accounts: Vec<ClaudeSwapAccountRow>,
}

struct ClaudeSwapAccountProjection;

impl ClaudeSwapAccountProjection {
    fn account_snapshots(
        list: &ClaudeSwapAccountList,
        now: Timestamp,
    ) -> Vec<ProviderAccountUsageSnapshot> {
        let mut email_counts: HashMap<String, usize> = HashMap::new();
        for row in &list.accounts {
            let trimmed = row.email.trim().to_ascii_lowercase();
            if !trimmed.is_empty() {
                *email_counts.entry(trimmed).or_insert(0) += 1;
            }
        }
        let mut ordered = list.accounts.clone();
        ordered.sort_by(|a, b| {
            let a_active = a.is_active || Some(a.number) == list.active_account_number;
            let b_active = b.is_active || Some(b.number) == list.active_account_number;
            b_active
                .cmp(&a_active)
                .then_with(|| a.number.cmp(&b.number))
        });

        ordered
            .into_iter()
            .map(|row| {
                let is_active = row.is_active || Some(row.number) == list.active_account_number;
                let trimmed_email = row.email.trim();
                let has_duplicate_email = !trimmed_email.is_empty()
                    && email_counts
                        .get(&trimmed_email.to_ascii_lowercase())
                        .copied()
                        .unwrap_or(0)
                        > 1;
                let display_label = if let Some(alias) = row
                    .alias
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    alias.to_string()
                } else if !trimmed_email.is_empty() {
                    let org = row.organization_name.trim();
                    if has_duplicate_email && !org.is_empty() {
                        format!("{trimmed_email} · {org}")
                    } else {
                        trimmed_email.to_string()
                    }
                } else {
                    format!("Account {}", row.number)
                };
                let account_email = (!trimmed_email.is_empty()).then(|| trimmed_email.to_string());
                let (snapshot, error) = match row.usage_status {
                    ClaudeSwapUsageStatus::Ok => {
                        let has_windows = row.five_hour.is_some()
                            || row.seven_day.is_some()
                            || !row.scoped.is_empty();
                        let snap = has_windows.then(|| {
                            let primary = row
                                .five_hour
                                .as_ref()
                                .map(|w| rate_window(w.used_percent, Some(300), w.resets_at, None));
                            let secondary = row.seven_day.as_ref().map(|w| {
                                rate_window(w.used_percent, Some(7 * 24 * 60), w.resets_at, None)
                            });
                            let extra_rate_windows = (!row.scoped.is_empty()).then(|| {
                                row.scoped
                                    .iter()
                                    .map(|s| {
                                        named_window(
                                            format!(
                                                "claude-weekly-scoped-{}",
                                                s.name.to_ascii_lowercase()
                                            ),
                                            format!("{} only", s.name),
                                            rate_window(
                                                s.used_percent,
                                                Some(7 * 24 * 60),
                                                s.resets_at,
                                                None,
                                            ),
                                            true,
                                        )
                                    })
                                    .collect()
                            });
                            let identity = account_email.as_ref().map(|email| {
                                ProviderIdentitySnapshot::new(
                                    Some(pid("claude")),
                                    Some(email.clone()),
                                    (!row.organization_name.trim().is_empty())
                                        .then(|| row.organization_name.trim().to_string()),
                                    None,
                                    None,
                                )
                            });
                            usage_snapshot(
                                primary,
                                secondary,
                                None,
                                extra_rate_windows,
                                None,
                                None,
                                None,
                                now,
                                identity,
                            )
                        });
                        (snap, None)
                    }
                    ClaudeSwapUsageStatus::TokenExpired => (
                        None,
                        Some(
                            "Token expired. Switch to this account in claude-swap to refresh it."
                                .to_string(),
                        ),
                    ),
                    ClaudeSwapUsageStatus::NoCredentials => (
                        None,
                        Some("No credentials stored for this account in claude-swap.".to_string()),
                    ),
                    ClaudeSwapUsageStatus::Unavailable => (
                        None,
                        Some("Usage unavailable from claude-swap.".to_string()),
                    ),
                };
                ProviderAccountUsageSnapshot {
                    id: ProviderAccountIdentity::new("claude-swap", row.number.to_string()),
                    provider: "claude".to_string(),
                    display_label,
                    account_email,
                    is_active,
                    uses_last_known_usage: false,
                    snapshot,
                    error,
                    source_label: None,
                }
            })
            .collect()
    }
}

fn config_with_providers(providers: Vec<(&str, bool)>) -> Config {
    let entries = providers
        .into_iter()
        .map(|(id, enabled)| {
            if let Ok(pid) = ProviderId::new(id) {
                let mut pc = ProviderConfig::new(pid);
                pc.enabled = Some(enabled);
                ProviderEntry::Typed(pc)
            } else {
                let raw = serde_json::value::to_raw_value(&serde_json::json!({
                    "id": id,
                    "enabled": enabled,
                }))
                .unwrap();
                ProviderEntry::Opaque {
                    id: id.to_string(),
                    enabled,
                    raw,
                }
            }
        })
        .collect();
    Config::new(1, entries)
}

fn json_object<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap()
}

// ============================================================================
// Section 1: DashboardAntigravityWindowTests.swift (6 tests)
// ============================================================================

fn antigravity_windows_for_usage(usage: UsageSnapshot, now: Timestamp) -> Vec<serde_json::Value> {
    let payload = ProviderPayload::new("antigravity", "cli").with_usage(usage);
    let snapshot = DashboardSnapshotBuilder::make_snapshot(
        &[payload],
        &[],
        &config_with_providers(vec![("antigravity", true)]),
        DashboardIdentityMode::Redacted,
        now,
        60.0,
        None,
        &BTreeMap::new(),
        false,
        false,
    );
    let obj = json_object(&snapshot);
    obj["providers"][0]["windows"].as_array().unwrap().clone()
}

fn antigravity_windows(
    gemini_session_percent: f64,
    gemini_weekly_percent: f64,
    third_party_session_percent: f64,
    third_party_weekly_percent: f64,
    third_party_usage_known: bool,
) -> Vec<serde_json::Value> {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let gemini_session = rate_window(gemini_session_percent, Some(300), Some(now), None);
    let gemini_weekly = rate_window(gemini_weekly_percent, Some(7 * 24 * 60), Some(now), None);
    let third_party_session = rate_window(third_party_session_percent, Some(300), Some(now), None);
    let third_party_weekly = rate_window(
        third_party_weekly_percent,
        Some(7 * 24 * 60),
        Some(now),
        None,
    );
    let extras = vec![
        named_window(
            "antigravity-quota-summary-gemini-5h",
            "Gemini 5-hour",
            gemini_session.clone(),
            true,
        ),
        named_window(
            "antigravity-quota-summary-gemini-weekly",
            "Gemini weekly",
            gemini_weekly.clone(),
            true,
        ),
        named_window(
            "antigravity-quota-summary-3p-5h",
            "Claude/GPT 5-hour",
            third_party_session.clone(),
            third_party_usage_known,
        ),
        named_window(
            "antigravity-quota-summary-3p-weekly",
            "Claude/GPT weekly",
            third_party_weekly,
            third_party_usage_known,
        ),
    ];
    let primary = if gemini_weekly_percent >= gemini_session_percent {
        gemini_weekly
    } else {
        gemini_session
    };
    let usage = usage_snapshot(
        Some(primary),
        Some(third_party_session),
        None,
        Some(extras),
        None,
        None,
        None,
        now,
        None,
    );
    antigravity_windows_for_usage(usage, now)
}

// upstream: DashboardAntigravityWindowTests.swift:12 dashboard renders one Antigravity lane per quota bucket
#[test]
fn dashboard_renders_one_antigravity_lane_per_quota_bucket() {
    let windows = antigravity_windows(3.9, 8.1, 1.5, 2.5, true);
    let labels: Vec<&str> = windows
        .iter()
        .map(|w| w["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        labels,
        vec![
            "Gemini 5-hour",
            "Gemini weekly",
            "Claude/GPT 5-hour",
            "Claude/GPT weekly",
        ]
    );
    let used: Vec<f64> = windows
        .iter()
        .map(|w| w["usedPercent"].as_f64().unwrap())
        .collect();
    assert_eq!(used, vec![3.9, 8.1, 1.5, 2.5]);
    assert!(windows.iter().all(|w| w.get("idle").is_none()));
}

// upstream: DashboardAntigravityWindowTests.swift:31 dashboard omits the Antigravity representative labels
#[test]
fn dashboard_omits_the_antigravity_representative_labels() {
    let windows = antigravity_windows(3.9, 8.1, 0.0, 0.0, true);
    let labels: Vec<&str> = windows
        .iter()
        .map(|w| w["label"].as_str().unwrap())
        .collect();
    assert!(!labels.contains(&"Gemini Models"));
    assert!(!labels.contains(&"Claude and GPT"));
}

// upstream: DashboardAntigravityWindowTests.swift:40 dashboard marks an Antigravity family that reports no usage as idle
#[test]
fn dashboard_marks_an_antigravity_family_that_reports_no_usage_as_idle() {
    let windows = antigravity_windows(3.9, 8.1, 0.0, 0.0, true);
    let labels: Vec<&str> = windows
        .iter()
        .map(|w| w["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        labels,
        vec![
            "Gemini 5-hour",
            "Gemini weekly",
            "Claude/GPT 5-hour",
            "Claude/GPT weekly",
        ]
    );
    let idle: Vec<Option<bool>> = windows
        .iter()
        .map(|w| w.get("idle").and_then(serde_json::Value::as_bool))
        .collect();
    assert_eq!(idle, vec![None, None, Some(true), Some(true)]);
}

// upstream: DashboardAntigravityWindowTests.swift:54 dashboard marks no Antigravity family idle when none reports usage
#[test]
fn dashboard_marks_no_antigravity_family_idle_when_none_reports_usage() {
    let windows = antigravity_windows(0.0, 0.0, 0.0, 0.0, true);
    assert_eq!(windows.len(), 4);
    assert!(windows.iter().all(|w| w.get("idle").is_none()));
}

// upstream: DashboardAntigravityWindowTests.swift:62 dashboard leaves an Antigravity family with unknown usage unmarked
#[test]
fn dashboard_leaves_an_antigravity_family_with_unknown_usage_unmarked() {
    let windows = antigravity_windows(3.9, 8.1, 0.0, 0.0, false);
    assert_eq!(windows.len(), 4);
    assert!(windows.iter().all(|w| w.get("idle").is_none()));
}

// upstream: DashboardAntigravityWindowTests.swift:73 dashboard keeps standard lanes for an Antigravity snapshot without summary windows
#[test]
fn dashboard_keeps_standard_lanes_for_an_antigravity_snapshot_without_summary_windows() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let usage = usage_snapshot(
        Some(rate_window(12.0, Some(300), Some(now), None)),
        Some(rate_window(34.0, Some(7 * 24 * 60), Some(now), None)),
        None,
        None,
        None,
        None,
        None,
        now,
        None,
    );
    let windows = antigravity_windows_for_usage(usage, now);
    let kinds: Vec<&str> = windows
        .iter()
        .map(|w| w["kind"].as_str().unwrap())
        .collect();
    let labels: Vec<&str> = windows
        .iter()
        .map(|w| w["label"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["session", "weekly"]);
    assert_eq!(labels, vec!["Gemini Models", "Claude and GPT"]);
}

// ============================================================================
// Section 2: DashboardClaudeSwapSnapshotTests.swift (23 tests)
// ============================================================================

fn claude_swap_generated_at() -> Timestamp {
    Timestamp::from_second(1_800_000_000).unwrap()
}

fn claude_swap_account_row(
    number: i64,
    email: &str,
    organization_name: &str,
    alias: Option<&str>,
    active: bool,
    scoped: Vec<ClaudeSwapScopedUsageWindow>,
) -> ClaudeSwapAccountRow {
    let now = claude_swap_generated_at();
    ClaudeSwapAccountRow {
        number,
        email: email.to_string(),
        organization_name: organization_name.to_string(),
        alias: alias.map(ToString::to_string),
        is_active: active,
        usage_status: ClaudeSwapUsageStatus::Ok,
        five_hour: Some(ClaudeSwapUsageWindow {
            used_percent: 40.0,
            resets_at: Some(
                now.checked_add(jiff::SignedDuration::from_secs(60 * 60))
                    .unwrap(),
            ),
        }),
        seven_day: Some(ClaudeSwapUsageWindow {
            used_percent: 60.0,
            resets_at: Some(
                now.checked_add(jiff::SignedDuration::from_secs(2 * 24 * 60 * 60))
                    .unwrap(),
            ),
        }),
        scoped,
    }
}

fn claude_swap_three_account_list() -> ClaudeSwapAccountList {
    let now = claude_swap_generated_at();
    ClaudeSwapAccountList {
        active_account_number: Some(2),
        accounts: vec![
            claude_swap_account_row(1, "work@example.com", "", None, false, vec![]),
            claude_swap_account_row(3, "third@example.net", "", None, false, vec![]),
            claude_swap_account_row(
                2,
                "personal@personal.example",
                "",
                None,
                true,
                vec![ClaudeSwapScopedUsageWindow {
                    name: "Fable".to_string(),
                    used_percent: 33.0,
                    resets_at: Some(
                        now.checked_add(jiff::SignedDuration::from_secs(2 * 24 * 60 * 60))
                            .unwrap(),
                    ),
                }],
            ),
        ],
    }
}

fn claude_swap_ambient_payload() -> ProviderPayload {
    let now = claude_swap_generated_at();
    let usage = usage_snapshot(
        Some(rate_window(
            20.0,
            Some(300),
            Some(
                now.checked_add(jiff::SignedDuration::from_secs(60 * 60))
                    .unwrap(),
            ),
            None,
        )),
        None,
        None,
        None,
        None,
        None,
        None,
        now,
        Some(ProviderIdentitySnapshot::new(
            Some(pid("claude")),
            Some("ambient@ambient.example".to_string()),
            None,
            Some("pro".to_string()),
            None,
        )),
    );
    ProviderPayload::new("claude", "web").with_usage(usage)
}

fn claude_swap_codex_payload() -> ProviderPayload {
    ProviderPayload::new("codex", "oauth")
}

fn claude_swap_providers(
    identity_mode: DashboardIdentityMode,
    claude_swap: Option<DashboardAccountsInput>,
) -> Vec<serde_json::Value> {
    let mut collections = BTreeMap::new();
    if let Some(input) = claude_swap {
        collections.insert("claude".to_string(), input);
    }
    let snapshot = DashboardSnapshotBuilder::make_snapshot(
        &[claude_swap_ambient_payload(), claude_swap_codex_payload()],
        &[],
        &config_with_providers(vec![("claude", true), ("codex", true)]),
        identity_mode,
        claude_swap_generated_at(),
        60.0,
        None,
        &collections,
        false,
        false,
    );
    let obj = json_object(&snapshot);
    obj["providers"].as_array().unwrap().clone()
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:10 projects ordered claude swap accounts with windows pace and redacted identity
#[test]
fn projects_ordered_claude_swap_accounts_with_windows_pace_and_redacted_identity() {
    let accounts = ClaudeSwapAccountProjection::account_snapshots(
        &claude_swap_three_account_list(),
        claude_swap_generated_at(),
    );
    let providers = claude_swap_providers(
        DashboardIdentityMode::Redacted,
        Some(DashboardAccountsInput {
            accounts: Some(accounts),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let claude = providers.iter().find(|p| p["id"] == "claude").unwrap();
    let rows = claude["accounts"].as_array().unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["claude-swap:2", "claude-swap:1", "claude-swap:3"]);
    let labels: Vec<&str> = rows.iter().map(|r| r["label"].as_str().unwrap()).collect();
    assert_eq!(
        labels,
        vec![
            "redacted@personal.example",
            "redacted@example.com",
            "redacted@example.net"
        ]
    );
    let active_flags: Vec<bool> = rows
        .iter()
        .map(|r| r["active"].as_bool().unwrap())
        .collect();
    assert_eq!(active_flags, vec![true, false, false]);

    let active = &rows[0];
    assert_eq!(
        active["identity"]["accountEmail"],
        "redacted@personal.example"
    );
    assert!(active["identity"]["plan"].is_null());
    let windows = active["windows"].as_array().unwrap();
    let kinds: Vec<&str> = windows
        .iter()
        .map(|w| w["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec!["session", "weekly", "claude-weekly-scoped-fable"]
    );
    let win_labels: Vec<&str> = windows
        .iter()
        .map(|w| w["label"].as_str().unwrap())
        .collect();
    assert_eq!(win_labels, vec!["Session", "Weekly", "Fable only"]);
    assert!(active["pace"]["primary"].is_object());
    assert!(active["pace"]["secondary"].is_object());
    assert_eq!(active["updatedAt"], "2027-01-15T08:00:00Z");
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:44 full identity mode keeps real account emails
#[test]
fn full_identity_mode_keeps_real_account_emails() {
    let accounts = ClaudeSwapAccountProjection::account_snapshots(
        &claude_swap_three_account_list(),
        claude_swap_generated_at(),
    );
    let providers = claude_swap_providers(
        DashboardIdentityMode::Full,
        Some(DashboardAccountsInput {
            accounts: Some(accounts),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let claude = providers.iter().find(|p| p["id"] == "claude").unwrap();
    let rows = claude["accounts"].as_array().unwrap();
    let emails: Vec<&str> = rows
        .iter()
        .map(|r| r["identity"]["accountEmail"].as_str().unwrap())
        .collect();
    assert_eq!(
        emails,
        vec![
            "personal@personal.example",
            "work@example.com",
            "third@example.net"
        ]
    );
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:58 shared emails stay distinct and alias replaces the email label
#[test]
fn shared_emails_stay_distinct_and_alias_replaces_the_email_label() {
    let accounts = ClaudeSwapAccountProjection::account_snapshots(
        &ClaudeSwapAccountList {
            active_account_number: Some(1),
            accounts: vec![
                claude_swap_account_row(
                    1,
                    "shared@example.com",
                    "Sendbird",
                    Some("Work"),
                    true,
                    vec![],
                ),
                claude_swap_account_row(2, "shared@example.com", "Acme", None, false, vec![]),
            ],
        },
        claude_swap_generated_at(),
    );
    let providers = claude_swap_providers(
        DashboardIdentityMode::Full,
        Some(DashboardAccountsInput {
            accounts: Some(accounts.clone()),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let claude = providers.iter().find(|p| p["id"] == "claude").unwrap();
    let rows = claude["accounts"].as_array().unwrap();
    let labels: Vec<&str> = rows.iter().map(|r| r["label"].as_str().unwrap()).collect();
    assert_eq!(labels, vec!["Work", "shared@example.com · Acme"]);
    let emails: Vec<&str> = rows
        .iter()
        .map(|r| r["identity"]["accountEmail"].as_str().unwrap())
        .collect();
    assert_eq!(emails, vec!["shared@example.com", "shared@example.com"]);
    let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["claude-swap:1", "claude-swap:2"]);

    let redacted_providers = claude_swap_providers(
        DashboardIdentityMode::Redacted,
        Some(DashboardAccountsInput {
            accounts: Some(accounts),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let redacted_claude = redacted_providers
        .iter()
        .find(|p| p["id"] == "claude")
        .unwrap();
    let redacted_rows = redacted_claude["accounts"].as_array().unwrap();
    let redacted_labels: Vec<&str> = redacted_rows
        .iter()
        .map(|r| r["label"].as_str().unwrap())
        .collect();
    assert_eq!(redacted_labels, vec!["Work", "redacted@example.com · Acme"]);
    let redacted_emails: Vec<&str> = redacted_rows
        .iter()
        .map(|r| r["identity"]["accountEmail"].as_str().unwrap())
        .collect();
    assert_eq!(
        redacted_emails,
        vec!["redacted@example.com", "redacted@example.com"]
    );
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:96 redacted dashboard mode rewrites email shaped aliases
#[test]
fn redacted_dashboard_mode_rewrites_email_shaped_aliases() {
    let accounts = ClaudeSwapAccountProjection::account_snapshots(
        &ClaudeSwapAccountList {
            active_account_number: Some(1),
            accounts: vec![
                claude_swap_account_row(
                    1,
                    "shared@example.com",
                    "Sendbird",
                    Some("other@example.com"),
                    true,
                    vec![],
                ),
                claude_swap_account_row(2, "shared@example.com", "Acme", None, false, vec![]),
            ],
        },
        claude_swap_generated_at(),
    );
    let providers = claude_swap_providers(
        DashboardIdentityMode::Redacted,
        Some(DashboardAccountsInput {
            accounts: Some(accounts),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let claude = providers.iter().find(|p| p["id"] == "claude").unwrap();
    let rows = claude["accounts"].as_array().unwrap();
    let labels: Vec<&str> = rows.iter().map(|r| r["label"].as_str().unwrap()).collect();
    assert_eq!(
        labels,
        vec!["redacted@example.com", "redacted@example.com · Acme"]
    );
    let emails: Vec<&str> = rows
        .iter()
        .map(|r| r["identity"]["accountEmail"].as_str().unwrap())
        .collect();
    assert_eq!(emails, vec!["redacted@example.com", "redacted@example.com"]);
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:130 redacted dashboard mode keeps non email alias text around an address
#[test]
fn redacted_dashboard_mode_keeps_non_email_alias_text_around_an_address() {
    let accounts = ClaudeSwapAccountProjection::account_snapshots(
        &ClaudeSwapAccountList {
            active_account_number: Some(1),
            accounts: vec![
                claude_swap_account_row(
                    1,
                    "shared@example.com",
                    "Sendbird",
                    Some("Work (owner@example.com)"),
                    true,
                    vec![],
                ),
                claude_swap_account_row(2, "shared@example.com", "Acme", None, false, vec![]),
            ],
        },
        claude_swap_generated_at(),
    );
    let providers = claude_swap_providers(
        DashboardIdentityMode::Redacted,
        Some(DashboardAccountsInput {
            accounts: Some(accounts),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let claude = providers.iter().find(|p| p["id"] == "claude").unwrap();
    let rows = claude["accounts"].as_array().unwrap();
    let labels: Vec<&str> = rows.iter().map(|r| r["label"].as_str().unwrap()).collect();
    assert_eq!(
        labels,
        vec!["Work (redacted@example.com)", "redacted@example.com · Acme"]
    );
    let emails: Vec<&str> = rows
        .iter()
        .map(|r| r["identity"]["accountEmail"].as_str().unwrap())
        .collect();
    assert_eq!(emails, vec!["redacted@example.com", "redacted@example.com"]);
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:164 redacted dashboard mode keeps alias text around colon prefixed emails
#[test]
fn redacted_dashboard_mode_keeps_alias_text_around_colon_prefixed_emails() {
    let accounts = ClaudeSwapAccountProjection::account_snapshots(
        &ClaudeSwapAccountList {
            active_account_number: Some(1),
            accounts: vec![
                claude_swap_account_row(
                    1,
                    "shared@example.com",
                    "Sendbird",
                    Some("Work:owner@example.com"),
                    true,
                    vec![],
                ),
                claude_swap_account_row(2, "shared@example.com", "Acme", None, false, vec![]),
            ],
        },
        claude_swap_generated_at(),
    );
    let providers = claude_swap_providers(
        DashboardIdentityMode::Redacted,
        Some(DashboardAccountsInput {
            accounts: Some(accounts),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let claude = providers.iter().find(|p| p["id"] == "claude").unwrap();
    let rows = claude["accounts"].as_array().unwrap();
    let labels: Vec<&str> = rows.iter().map(|r| r["label"].as_str().unwrap()).collect();
    assert_eq!(
        labels,
        vec!["Work:redacted@example.com", "redacted@example.com · Acme"]
    );
    let emails: Vec<&str> = rows
        .iter()
        .map(|r| r["identity"]["accountEmail"].as_str().unwrap())
        .collect();
    assert_eq!(emails, vec!["redacted@example.com", "redacted@example.com"]);
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:198 redacted dashboard mode rewrites emails inside organization suffixes
#[test]
fn redacted_dashboard_mode_rewrites_emails_inside_organization_suffixes() {
    let accounts = ClaudeSwapAccountProjection::account_snapshots(
        &ClaudeSwapAccountList {
            active_account_number: Some(1),
            accounts: vec![
                claude_swap_account_row(
                    1,
                    "shared@example.com",
                    "admin@company.com",
                    None,
                    true,
                    vec![],
                ),
                claude_swap_account_row(2, "shared@example.com", "Acme", None, false, vec![]),
            ],
        },
        claude_swap_generated_at(),
    );
    let providers = claude_swap_providers(
        DashboardIdentityMode::Redacted,
        Some(DashboardAccountsInput {
            accounts: Some(accounts),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let claude = providers.iter().find(|p| p["id"] == "claude").unwrap();
    let rows = claude["accounts"].as_array().unwrap();
    let labels: Vec<&str> = rows.iter().map(|r| r["label"].as_str().unwrap()).collect();
    assert_eq!(
        labels,
        vec![
            "redacted@example.com · redacted@company.com",
            "redacted@example.com · Acme"
        ]
    );
    let emails: Vec<&str> = rows
        .iter()
        .map(|r| r["identity"]["accountEmail"].as_str().unwrap())
        .collect();
    assert_eq!(emails, vec!["redacted@example.com", "redacted@example.com"]);
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:227 redacted dashboard mode rewrites internal and domain literal addresses
#[test]
fn redacted_dashboard_mode_rewrites_internal_and_domain_literal_addresses() {
    let accounts = ClaudeSwapAccountProjection::account_snapshots(
        &ClaudeSwapAccountList {
            active_account_number: Some(1),
            accounts: vec![
                claude_swap_account_row(
                    1,
                    "shared@example.com",
                    "Sendbird",
                    Some("Work (owner@corp)"),
                    true,
                    vec![],
                ),
                claude_swap_account_row(
                    2,
                    "shared@example.com",
                    "ops@[192.0.2.1]",
                    None,
                    false,
                    vec![],
                ),
            ],
        },
        claude_swap_generated_at(),
    );
    let providers = claude_swap_providers(
        DashboardIdentityMode::Redacted,
        Some(DashboardAccountsInput {
            accounts: Some(accounts),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let claude = providers.iter().find(|p| p["id"] == "claude").unwrap();
    let rows = claude["accounts"].as_array().unwrap();
    let labels: Vec<&str> = rows.iter().map(|r| r["label"].as_str().unwrap()).collect();
    assert_eq!(
        labels,
        vec![
            "Work (redacted@corp)",
            "redacted@example.com · redacted@[192.0.2.1]"
        ]
    );
    let emails: Vec<&str> = rows
        .iter()
        .map(|r| r["identity"]["accountEmail"].as_str().unwrap())
        .collect();
    assert_eq!(emails, vec!["redacted@example.com", "redacted@example.com"]);
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:257 redacted dashboard mode rewrites apostrophe local parts
#[test]
fn redacted_dashboard_mode_rewrites_apostrophe_local_parts() {
    let accounts = ClaudeSwapAccountProjection::account_snapshots(
        &ClaudeSwapAccountList {
            active_account_number: Some(1),
            accounts: vec![
                claude_swap_account_row(
                    1,
                    "shared@example.com",
                    "Sendbird",
                    Some("Work (o'connor@example.com)"),
                    true,
                    vec![],
                ),
                claude_swap_account_row(2, "shared@example.com", "Acme", None, false, vec![]),
            ],
        },
        claude_swap_generated_at(),
    );
    let providers = claude_swap_providers(
        DashboardIdentityMode::Redacted,
        Some(DashboardAccountsInput {
            accounts: Some(accounts),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let claude = providers.iter().find(|p| p["id"] == "claude").unwrap();
    let rows = claude["accounts"].as_array().unwrap();
    let labels: Vec<&str> = rows.iter().map(|r| r["label"].as_str().unwrap()).collect();
    assert_eq!(
        labels,
        vec!["Work (redacted@example.com)", "redacted@example.com · Acme"]
    );
    let emails: Vec<&str> = rows
        .iter()
        .map(|r| r["identity"]["accountEmail"].as_str().unwrap())
        .collect();
    assert_eq!(emails, vec!["redacted@example.com", "redacted@example.com"]);
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:287 redacted dashboard mode rewrites unicode local parts
#[test]
fn redacted_dashboard_mode_rewrites_unicode_local_parts() {
    let accounts = ClaudeSwapAccountProjection::account_snapshots(
        &ClaudeSwapAccountList {
            active_account_number: Some(1),
            accounts: vec![claude_swap_account_row(
                1,
                "用户@例子.公司",
                "Sendbird",
                None,
                true,
                vec![],
            )],
        },
        claude_swap_generated_at(),
    );
    let providers = claude_swap_providers(
        DashboardIdentityMode::Redacted,
        Some(DashboardAccountsInput {
            accounts: Some(accounts),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let claude = providers.iter().find(|p| p["id"] == "claude").unwrap();
    let rows = claude["accounts"].as_array().unwrap();
    let labels: Vec<&str> = rows.iter().map(|r| r["label"].as_str().unwrap()).collect();
    assert_eq!(labels, vec!["redacted@例子.公司"]);
    let emails: Vec<&str> = rows
        .iter()
        .map(|r| r["identity"]["accountEmail"].as_str().unwrap())
        .collect();
    assert_eq!(emails, vec!["redacted@例子.公司"]);
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:308 redacted dashboard mode rewrites quoted local parts
#[test]
fn redacted_dashboard_mode_rewrites_quoted_local_parts() {
    let accounts = ClaudeSwapAccountProjection::account_snapshots(
        &ClaudeSwapAccountList {
            active_account_number: Some(1),
            accounts: vec![
                claude_swap_account_row(
                    1,
                    "shared@example.com",
                    "Sendbird",
                    Some("Contact \"owner\"@example.com"),
                    true,
                    vec![],
                ),
                claude_swap_account_row(2, "shared@example.com", "Acme", None, false, vec![]),
            ],
        },
        claude_swap_generated_at(),
    );
    let providers = claude_swap_providers(
        DashboardIdentityMode::Redacted,
        Some(DashboardAccountsInput {
            accounts: Some(accounts),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let claude = providers.iter().find(|p| p["id"] == "claude").unwrap();
    let rows = claude["accounts"].as_array().unwrap();
    let labels: Vec<&str> = rows.iter().map(|r| r["label"].as_str().unwrap()).collect();
    assert_eq!(
        labels,
        vec![
            "Contact redacted@example.com",
            "redacted@example.com · Acme"
        ]
    );
    let emails: Vec<&str> = rows
        .iter()
        .map(|r| r["identity"]["accountEmail"].as_str().unwrap())
        .collect();
    assert_eq!(emails, vec!["redacted@example.com", "redacted@example.com"]);
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:338 redacted dashboard mode rewrites quoted local parts that contain spaces
#[test]
fn redacted_dashboard_mode_rewrites_quoted_local_parts_that_contain_spaces() {
    let accounts = ClaudeSwapAccountProjection::account_snapshots(
        &ClaudeSwapAccountList {
            active_account_number: Some(1),
            accounts: vec![
                claude_swap_account_row(
                    1,
                    "shared@example.com",
                    "Sendbird",
                    Some("Contact \"owner smith\"@example.com"),
                    true,
                    vec![],
                ),
                claude_swap_account_row(2, "shared@example.com", "Acme", None, false, vec![]),
            ],
        },
        claude_swap_generated_at(),
    );
    let providers = claude_swap_providers(
        DashboardIdentityMode::Redacted,
        Some(DashboardAccountsInput {
            accounts: Some(accounts),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let claude = providers.iter().find(|p| p["id"] == "claude").unwrap();
    let rows = claude["accounts"].as_array().unwrap();
    let labels: Vec<&str> = rows.iter().map(|r| r["label"].as_str().unwrap()).collect();
    assert_eq!(
        labels,
        vec![
            "Contact redacted@example.com",
            "redacted@example.com · Acme"
        ]
    );
    let emails: Vec<&str> = rows
        .iter()
        .map(|r| r["identity"]["accountEmail"].as_str().unwrap())
        .collect();
    assert_eq!(emails, vec!["redacted@example.com", "redacted@example.com"]);
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:368 redacted dashboard mode rewrites slash separated addresses independently
#[test]
fn redacted_dashboard_mode_rewrites_slash_separated_addresses_independently() {
    let accounts = ClaudeSwapAccountProjection::account_snapshots(
        &ClaudeSwapAccountList {
            active_account_number: Some(1),
            accounts: vec![
                claude_swap_account_row(
                    1,
                    "shared@example.com",
                    "Sendbird",
                    Some("owner@example.com/backup@example.net"),
                    true,
                    vec![],
                ),
                claude_swap_account_row(2, "shared@example.com", "Acme", None, false, vec![]),
            ],
        },
        claude_swap_generated_at(),
    );
    let providers = claude_swap_providers(
        DashboardIdentityMode::Redacted,
        Some(DashboardAccountsInput {
            accounts: Some(accounts),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let claude = providers.iter().find(|p| p["id"] == "claude").unwrap();
    let rows = claude["accounts"].as_array().unwrap();
    let labels: Vec<&str> = rows.iter().map(|r| r["label"].as_str().unwrap()).collect();
    assert_eq!(
        labels,
        vec![
            "redacted@example.com/redacted@example.net",
            "redacted@example.com · Acme"
        ]
    );
    let emails: Vec<&str> = rows
        .iter()
        .map(|r| r["identity"]["accountEmail"].as_str().unwrap())
        .collect();
    assert_eq!(emails, vec!["redacted@example.com", "redacted@example.com"]);
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:398 dashboard identity flag decodes redacted full and rejects others
#[test]
fn dashboard_identity_flag_decodes_redacted_full_and_rejects_others() {
    assert_eq!(
        decode_dashboard_identity_mode(None),
        Some(DashboardIdentityMode::Full)
    );
    assert_eq!(
        decode_dashboard_identity_mode(Some("redacted")),
        Some(DashboardIdentityMode::Redacted)
    );
    assert_eq!(
        decode_dashboard_identity_mode(Some("full")),
        Some(DashboardIdentityMode::Full)
    );
    assert_eq!(
        decode_dashboard_identity_mode(Some("FULL")),
        Some(DashboardIdentityMode::Full)
    );
    assert_eq!(decode_dashboard_identity_mode(Some("none")), None);
    assert_eq!(decode_dashboard_identity_mode(Some("bogus")), None);
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:415 dashboard output flag decodes stdout default file paths and rejects empty
#[test]
fn dashboard_output_flag_decodes_stdout_default_file_paths_and_rejects_empty() {
    assert_eq!(decode_dashboard_output_path(None), Some(None));
    assert_eq!(
        decode_dashboard_output_path(Some("/tmp/snapshot.json")),
        Some(Some(PathBuf::from("/tmp/snapshot.json")))
    );
    assert_eq!(
        decode_dashboard_output_path(Some("relative.json")),
        Some(Some(PathBuf::from("relative.json")))
    );
    assert_eq!(decode_dashboard_output_path(Some("")), None);
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:432 per account failure keeps healthy siblings and ambient row intact
#[test]
fn per_account_failure_keeps_healthy_siblings_and_ambient_row_intact() {
    let list = ClaudeSwapAccountList {
        active_account_number: Some(1),
        accounts: vec![
            claude_swap_account_row(1, "healthy@example.com", "", None, true, vec![]),
            ClaudeSwapAccountRow {
                number: 2,
                email: "expired@example.com".to_string(),
                organization_name: String::new(),
                alias: None,
                is_active: false,
                usage_status: ClaudeSwapUsageStatus::TokenExpired,
                five_hour: None,
                seven_day: None,
                scoped: vec![],
            },
        ],
    };
    let accounts =
        ClaudeSwapAccountProjection::account_snapshots(&list, claude_swap_generated_at());
    let providers = claude_swap_providers(
        DashboardIdentityMode::Redacted,
        Some(DashboardAccountsInput {
            accounts: Some(accounts),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let claude = &providers[0];
    let rows = claude["accounts"].as_array().unwrap();
    let healthy = &rows[0];
    let failed = &rows[1];

    assert_eq!(healthy["windows"].as_array().unwrap().len(), 2);
    assert!(healthy["error"].is_null());
    assert_eq!(
        failed["error"],
        "Token expired. Switch to this account in claude-swap to refresh it."
    );
    assert!(failed["windows"].as_array().unwrap().is_empty());
    assert_eq!(failed["label"], "redacted@example.com");
    assert_eq!(failed["identity"]["accountEmail"], "redacted@example.com");
    assert!(failed["pace"].is_null());
    assert!(failed["updatedAt"].is_null());
    assert!(claude["error"].is_null());
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:467 failed disambiguated accounts keep the raw email identity
#[test]
fn failed_disambiguated_accounts_keep_the_raw_email_identity() {
    let list = ClaudeSwapAccountList {
        active_account_number: Some(1),
        accounts: vec![
            ClaudeSwapAccountRow {
                number: 1,
                email: "shared@example.com".to_string(),
                organization_name: "Sendbird".to_string(),
                alias: Some("Work".to_string()),
                is_active: true,
                usage_status: ClaudeSwapUsageStatus::TokenExpired,
                five_hour: None,
                seven_day: None,
                scoped: vec![],
            },
            ClaudeSwapAccountRow {
                number: 2,
                email: "shared@example.com".to_string(),
                organization_name: "Acme".to_string(),
                alias: None,
                is_active: false,
                usage_status: ClaudeSwapUsageStatus::NoCredentials,
                five_hour: None,
                seven_day: None,
                scoped: vec![],
            },
        ],
    };
    let accounts =
        ClaudeSwapAccountProjection::account_snapshots(&list, claude_swap_generated_at());
    let providers = claude_swap_providers(
        DashboardIdentityMode::Full,
        Some(DashboardAccountsInput {
            accounts: Some(accounts),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let rows = providers[0]["accounts"].as_array().unwrap();
    let labels: Vec<&str> = rows.iter().map(|r| r["label"].as_str().unwrap()).collect();
    assert_eq!(labels, vec!["Work", "shared@example.com · Acme"]);
    let emails: Vec<&str> = rows
        .iter()
        .map(|r| r["identity"]["accountEmail"].as_str().unwrap())
        .collect();
    assert_eq!(emails, vec!["shared@example.com", "shared@example.com"]);
    assert!(rows[1]["windows"].as_array().unwrap().is_empty());
    assert!(rows[1]["updatedAt"].is_null());
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:505 adapter failure adds only accounts error and preserves ambient fields
#[test]
fn adapter_failure_adds_only_accounts_error_and_preserves_ambient_fields() {
    let providers = claude_swap_providers(
        DashboardIdentityMode::Redacted,
        Some(DashboardAccountsInput {
            accounts: None,
            adapter_error: Some("claude-swap timed out.".to_string()),
            weekly_work_days: None,
        }),
    );
    let claude = &providers[0];
    assert!(claude.get("accounts").is_none());
    assert_eq!(claude["accountsError"], "claude-swap timed out.");
    assert_eq!(claude["windows"].as_array().unwrap().len(), 1);
    assert_eq!(
        claude["identity"]["accountEmail"],
        "redacted@ambient.example"
    );
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:522 absent swap omits additive keys from every provider row
#[test]
fn absent_swap_omits_additive_keys_from_every_provider_row() {
    let providers = claude_swap_providers(DashboardIdentityMode::Redacted, None);
    assert_eq!(providers.len(), 2);
    for provider in &providers {
        assert!(provider.get("accounts").is_none());
        assert!(provider.get("accountsError").is_none());
    }
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:533 account identity honors redaction modes and rejects placeholder labels
#[test]
fn account_identity_honors_redaction_modes_and_rejects_placeholder_labels() {
    let real = ClaudeSwapAccountProjection::account_snapshots(
        &ClaudeSwapAccountList {
            active_account_number: Some(1),
            accounts: vec![claude_swap_account_row(
                1,
                "person@example.com",
                "",
                None,
                true,
                vec![],
            )],
        },
        claude_swap_generated_at(),
    );
    let placeholder = ClaudeSwapAccountProjection::account_snapshots(
        &ClaudeSwapAccountList {
            active_account_number: Some(2),
            accounts: vec![claude_swap_account_row(2, "", "", None, true, vec![])],
        },
        claude_swap_generated_at(),
    );

    let none_provs = claude_swap_providers(
        DashboardIdentityMode::None,
        Some(DashboardAccountsInput {
            accounts: Some(real.clone()),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let full_provs = claude_swap_providers(
        DashboardIdentityMode::Full,
        Some(DashboardAccountsInput {
            accounts: Some(real),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let placeholder_provs = claude_swap_providers(
        DashboardIdentityMode::Full,
        Some(DashboardAccountsInput {
            accounts: Some(placeholder),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );

    assert!(none_provs[0]["accounts"][0]["identity"].is_null());
    assert_eq!(
        full_provs[0]["accounts"][0]["identity"]["accountEmail"],
        "person@example.com"
    );
    assert!(placeholder_provs[0]["accounts"][0]["identity"].is_null());
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:555 enabled swap with no accounts emits an empty array
#[test]
fn enabled_swap_with_no_accounts_emits_an_empty_array() {
    let providers = claude_swap_providers(
        DashboardIdentityMode::Redacted,
        Some(DashboardAccountsInput {
            accounts: Some(vec![]),
            adapter_error: None,
            weekly_work_days: None,
        }),
    );
    let claude = &providers[0];
    assert!(claude["accounts"].as_array().unwrap().is_empty());
    assert!(claude.get("accountsError").is_none());
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:566 producer collects swap accounts with config while its default omits them
#[tokio::test]
async fn producer_collects_swap_accounts_with_config_while_its_default_omits_them() {
    let received_enabled = Arc::new(AtomicBool::new(false));
    let account = ClaudeSwapAccountProjection::account_snapshots(
        &ClaudeSwapAccountList {
            active_account_number: Some(1),
            accounts: vec![claude_swap_account_row(
                1,
                "person@example.com",
                "",
                None,
                true,
                vec![],
            )],
        },
        claude_swap_generated_at(),
    );
    let mut claude_cfg = ProviderConfig::new(pid("claude"));
    claude_cfg.enabled = Some(true);
    claude_cfg.extra.insert(
        "claudeSwapEnabled".to_string(),
        serde_json::value::to_raw_value(&true).unwrap(),
    );
    let config = Config::new(1, vec![ProviderEntry::Typed(claude_cfg)]);
    let ambient = claude_swap_ambient_payload();
    let ambient_clone = ambient.clone();
    let mut producer = DashboardSnapshotProducer::new(
        Arc::new(move |_| {
            let a = ambient_clone.clone();
            Box::pin(async move {
                Ok(UsageCommandOutput {
                    sections: vec![],
                    payload: vec![a],
                    exit_code: 0,
                })
            })
        }),
        Arc::new(|_, _| Box::pin(async { vec![] })),
        Arc::new(claude_swap_generated_at),
    );
    let rec = Arc::clone(&received_enabled);
    let acc_clone = account.clone();
    producer.collect_accounts = Arc::new(move |cfg, prov| {
        let rec = Arc::clone(&rec);
        let acc = acc_clone.clone();
        Box::pin(async move {
            if prov != "claude" {
                return None;
            }
            rec.store(dashboard_claude_swap_is_eligible(&cfg), Ordering::SeqCst);
            Some(DashboardAccountsInput {
                accounts: Some(acc),
                adapter_error: None,
                weekly_work_days: None,
            })
        })
    });

    let result = producer
        .collect(&config, 0.0, None, None, None)
        .await
        .unwrap();
    let obj = json_object(&result.payload);
    assert_eq!(obj["providers"][0]["accounts"].as_array().unwrap().len(), 1);
    assert!(received_enabled.load(Ordering::SeqCst));

    let default_producer = DashboardSnapshotProducer::new(
        Arc::new(move |_| {
            let a = ambient.clone();
            Box::pin(async move {
                Ok(UsageCommandOutput {
                    sections: vec![],
                    payload: vec![a],
                    exit_code: 0,
                })
            })
        }),
        Arc::new(|_, _| Box::pin(async { vec![] })),
        Arc::new(claude_swap_generated_at),
    );
    let default_result = default_producer
        .collect(&config, 0.0, None, None, None)
        .await
        .unwrap();
    let default_obj = json_object(&default_result.payload);
    assert!(default_obj["providers"][0].get("accounts").is_none());
}

// upstream: DashboardClaudeSwapSnapshotTests.swift:611 dashboard swap eligibility requires enabled Claude and integration opt in
#[test]
fn dashboard_swap_eligibility_requires_enabled_claude_and_integration_opt_in() {
    let mut disabled = ProviderConfig::new(pid("claude"));
    disabled.enabled = Some(false);
    disabled.extra.insert(
        "claudeSwapEnabled".to_string(),
        serde_json::value::to_raw_value(&true).unwrap(),
    );

    let mut unset = ProviderConfig::new(pid("claude"));
    unset.enabled = Some(true);

    let mut explicit_false = ProviderConfig::new(pid("claude"));
    explicit_false.enabled = Some(true);
    explicit_false.extra.insert(
        "claudeSwapEnabled".to_string(),
        serde_json::value::to_raw_value(&false).unwrap(),
    );

    let mut enabled = ProviderConfig::new(pid("claude"));
    enabled.enabled = Some(true);
    enabled.extra.insert(
        "claudeSwapEnabled".to_string(),
        serde_json::value::to_raw_value(&true).unwrap(),
    );

    assert!(!dashboard_claude_swap_is_eligible(&Config::new(
        1,
        vec![ProviderEntry::Typed(disabled)]
    )));
    assert!(!dashboard_claude_swap_is_eligible(&Config::new(
        1,
        vec![ProviderEntry::Typed(unset)]
    )));
    assert!(!dashboard_claude_swap_is_eligible(&Config::new(
        1,
        vec![ProviderEntry::Typed(explicit_false)]
    )));
    assert!(dashboard_claude_swap_is_eligible(&Config::new(
        1,
        vec![ProviderEntry::Typed(enabled)]
    )));
}

// ============================================================================
// Section 3: DashboardManagedCodexCollectionTests.swift (8 tests)
// ============================================================================

struct ManagedDashboardFixture {
    _dir: TempDir,
    accounts_path: PathBuf,
    cache_path: PathBuf,
    accounts: Vec<ManagedCodexAccount>,
    config: Config,
    updated_at: Timestamp,
}

impl ManagedDashboardFixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let accounts_path = root.join("managed-codex-accounts.json");
        let cache_path = root.join("codex-account-snapshots.json");
        let updated_at = Timestamp::from_second(1_800_000_000).unwrap();
        let accounts: Vec<ManagedCodexAccount> = ["first@example.com", "second@example.net"]
            .into_iter()
            .enumerate()
            .map(|(index, email)| ManagedCodexAccount {
                id: Uuid::new_v4(),
                email: email.to_string(),
                provider_account_id: None,
                workspace_label: Some(if index == 0 { "Work" } else { "Personal" }.to_string()),
                workspace_account_id: Some(format!("workspace-{index}")),
                auth_fingerprint: None,
                managed_home_path: root.join(format!("home-{index}")).display().to_string(),
                created_at: 100,
                updated_at: 200,
                last_authenticated_at: 0,
            })
            .collect();

        let mut provider = ProviderConfig::new(pid("codex"));
        provider.enabled = Some(true);
        CodexActiveSource::ManagedAccount { id: accounts[0].id }
            .apply_to_provider_config(&mut provider);
        let config = Config::new(1, vec![ProviderEntry::Typed(provider)]);

        FileManagedCodexAccountStore::new(&accounts_path)
            .store_accounts(&ManagedCodexAccountSet {
                version: 3,
                accounts: accounts.clone(),
            })
            .unwrap();

        let usage = usage_snapshot(
            Some(rate_window(
                42.0,
                Some(300),
                Some(
                    updated_at
                        .checked_add(jiff::SignedDuration::from_secs(3600))
                        .unwrap(),
                ),
                None,
            )),
            None,
            None,
            None,
            None,
            None,
            None,
            updated_at,
            Some(ProviderIdentitySnapshot::new(
                Some(pid("codex")),
                Some("first@example.com".to_string()),
                None,
                Some("plus".to_string()),
                None,
            )),
        );

        let records: Vec<serde_json::Value> = accounts
            .iter()
            .enumerate()
            .map(|(index, account)| {
                serde_json::json!({
                    "accountIdentity": {
                        "normalizedEmail": account.email,
                        "workspaceAccountID": account.workspace_account_id,
                        "storedAccountID": account.id.hyphenated().to_string().to_ascii_uppercase(),
                    },
                    "snapshot": if index == 0 { serde_json::to_value(&usage).unwrap() } else { serde_json::Value::Null },
                    "error": if index == 0 { serde_json::Value::Null } else { serde_json::Value::String("Refresh failed for private@example.com at /synthetic/private/auth.json".to_string()) },
                    "sourceLabel": "oauth"
                })
            })
            .collect();
        let cache_json = serde_json::json!({
            "version": 1,
            "records": records,
        });
        std::fs::write(&cache_path, serde_json::to_vec_pretty(&cache_json).unwrap()).unwrap();

        Self {
            _dir: dir,
            accounts_path,
            cache_path,
            accounts,
            config,
            updated_at,
        }
    }

    fn collect(&self) -> Option<DashboardAccountsInput> {
        DashboardManagedCodexAccounts::collect(&self.config, &self.accounts_path)
    }
}

// upstream: DashboardManagedCodexCollectionTests.swift:8 dashboard CLI emits redacted managed accounts from an isolated home
#[test]
fn dashboard_cli_emits_redacted_managed_accounts_from_an_isolated_home() {
    let fixture = ManagedDashboardFixture::new();
    let collection = fixture.collect().unwrap();
    let mut collections = BTreeMap::new();
    collections.insert("codex".to_string(), collection);
    let snapshot = DashboardSnapshotBuilder::make_snapshot(
        &[ProviderPayload::new("codex", "oauth")],
        &[],
        &fixture.config,
        DashboardIdentityMode::Redacted,
        fixture.updated_at,
        60.0,
        None,
        &collections,
        false,
        false,
    );
    let mut bytes = serde_json::to_vec(&snapshot).unwrap();
    bytes.push(b'\n');
    assert_eq!(bytes.last(), Some(&b'\n'));
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["schemaVersion"], 1);
    let provider = &json["providers"][0];
    let accounts = provider["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 2);
    assert_eq!(
        accounts[0]["id"],
        format!(
            "codex-managed:{}",
            fixture.accounts[0]
                .id
                .hyphenated()
                .to_string()
                .to_ascii_lowercase()
        )
    );
    assert_eq!(accounts[0]["active"], true);
    assert_eq!(
        accounts[0]["identity"]["accountEmail"],
        "redacted@example.com"
    );
    assert_eq!(accounts[0]["windows"][0]["usedPercent"], 42.0);
    let text = serde_json::to_string(accounts).unwrap();
    assert!(!text.contains("first@"));
    assert!(!text.contains("second@"));
    assert!(!text.contains("/synthetic/private"));
}

// upstream: DashboardManagedCodexCollectionTests.swift:56 reads app written snapshots with stable IDs and independent errors
#[test]
fn reads_app_written_snapshots_with_stable_ids_and_independent_errors() {
    let fixture = ManagedDashboardFixture::new();
    let before = std::fs::read(&fixture.accounts_path).unwrap();
    let collection = fixture.collect().unwrap();
    let accounts = collection.accounts.unwrap();
    assert_eq!(collection.adapter_error, None);
    let opaque_ids: Vec<String> = accounts.iter().map(|a| a.id.opaque_id.clone()).collect();
    let expected_ids: Vec<String> = fixture
        .accounts
        .iter()
        .map(|a| a.id.hyphenated().to_string().to_ascii_lowercase())
        .collect();
    assert_eq!(opaque_ids, expected_ids);
    assert_eq!(
        accounts.iter().map(|a| a.is_active).collect::<Vec<_>>(),
        vec![true, false]
    );
    assert_eq!(
        accounts[0]
            .snapshot
            .as_ref()
            .and_then(|s| s.primary.as_ref())
            .map(|w| w.used_percent),
        Some(42.0)
    );
    assert_eq!(
        accounts[0].snapshot.as_ref().map(|s| s.updated_at),
        Some(fixture.updated_at)
    );
    assert_eq!(accounts[0].error, None);
    assert_eq!(accounts[1].snapshot, None);
    assert_eq!(
        accounts[1].error.as_deref(),
        Some("Saved usage refresh failed. Refresh this account in CodexBar.")
    );
    assert_eq!(accounts[1].display_label, "second@example.net — Personal");
    assert_eq!(std::fs::read(&fixture.accounts_path).unwrap(), before);
}

// upstream: DashboardManagedCodexCollectionTests.swift:75 rejects saved usage belonging to another account
#[test]
fn rejects_saved_usage_belonging_to_another_account() {
    for field in ["email", "workspace", "uuid"] {
        let fixture = ManagedDashboardFixture::new();
        let mut payload: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&fixture.cache_path).unwrap()).unwrap();
        let identity = payload["records"][0]["accountIdentity"]
            .as_object_mut()
            .unwrap();
        match field {
            "email" => {
                identity.insert(
                    "normalizedEmail".to_string(),
                    serde_json::json!("foreign@example.com"),
                );
            }
            "workspace" => {
                identity.insert(
                    "workspaceAccountID".to_string(),
                    serde_json::json!("other-workspace"),
                );
            }
            _ => {
                identity.insert(
                    "storedAccountID".to_string(),
                    serde_json::json!(Uuid::new_v4().hyphenated().to_string()),
                );
            }
        }
        std::fs::write(&fixture.cache_path, serde_json::to_vec(&payload).unwrap()).unwrap();
        let accounts = fixture.collect().unwrap().accounts.unwrap();
        assert_eq!(accounts[0].snapshot, None);
        assert!(accounts[0].error.is_some());
        assert!(accounts[1].error.is_some());
    }
}

// upstream: DashboardManagedCodexCollectionTests.swift:97 missing saved usage keeps identities and stable IDs without ambient fallback
#[test]
fn missing_saved_usage_keeps_identities_and_stable_ids_without_ambient_fallback() {
    let fixture = ManagedDashboardFixture::new();
    std::fs::remove_file(&fixture.cache_path).unwrap();
    let accounts = fixture.collect().unwrap().accounts.unwrap();
    assert_eq!(accounts.len(), 2);
    assert!(
        accounts
            .iter()
            .all(|a| a.snapshot.is_none() && a.error.is_some())
    );
    assert_eq!(
        accounts[0].account_email.as_deref(),
        Some("first@example.com")
    );
}

// upstream: DashboardManagedCodexCollectionTests.swift:108 whole adapter failures expose fixed diagnostics
#[test]
fn whole_adapter_failures_expose_fixed_diagnostics() {
    for failure in ["metadata", "cache", "version"] {
        let fixture = ManagedDashboardFixture::new();
        let path = if failure == "metadata" {
            &fixture.accounts_path
        } else {
            &fixture.cache_path
        };
        let bytes = if failure == "version" {
            b"{\"version\":999,\"records\":[]}".as_slice()
        } else {
            b"invalid private-path@example.com".as_slice()
        };
        std::fs::write(path, bytes).unwrap();
        let collection = fixture.collect().unwrap();
        assert_eq!(collection.accounts, None);
        assert!(collection.adapter_error.is_some());
        assert!(!collection.adapter_error.unwrap().contains("private-path"));
    }
}

// upstream: DashboardManagedCodexCollectionTests.swift:121 absent managed accounts and disabled Codex omit enrichment
#[test]
fn absent_managed_accounts_and_disabled_codex_omit_enrichment() {
    let fixture = ManagedDashboardFixture::new();
    assert_eq!(
        DashboardManagedCodexAccounts::collect(
            &config_with_providers(vec![("codex", false)]),
            &fixture.accounts_path
        ),
        None
    );
    std::fs::remove_file(&fixture.accounts_path).unwrap();
    assert_eq!(fixture.collect(), None);
    assert!(!fixture.accounts_path.exists());
}

// upstream: DashboardManagedCodexCollectionTests.swift:133 system and profile selections do not mark managed rows active
#[test]
fn system_and_profile_selections_do_not_mark_managed_rows_active() {
    let fixture = ManagedDashboardFixture::new();
    for source in [
        CodexActiveSource::LiveSystem,
        CodexActiveSource::ProfileHome {
            path: "/synthetic/profile".to_string(),
        },
    ] {
        let mut pc = ProviderConfig::new(pid("codex"));
        pc.enabled = Some(true);
        source.apply_to_provider_config(&mut pc);
        let cfg = Config::new(1, vec![ProviderEntry::Typed(pc)]);
        let collection =
            DashboardManagedCodexAccounts::collect(&cfg, &fixture.accounts_path).unwrap();
        assert!(collection.accounts.unwrap().iter().all(|a| !a.is_active));
    }
}

// upstream: DashboardManagedCodexCollectionTests.swift:145 legacy metadata projection never hydrates credentials or rewrites files
#[test]
fn legacy_metadata_projection_never_hydrates_credentials_or_rewrites_files() {
    let fixture = ManagedDashboardFixture::new();
    let legacy = ManagedCodexAccountSet {
        version: 1,
        accounts: fixture.accounts.clone(),
    };
    std::fs::write(&fixture.accounts_path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let home = PathBuf::from(&fixture.accounts[0].managed_home_path);
    std::fs::create_dir_all(&home).unwrap();
    let auth = home.join("auth.json");
    std::fs::write(&auth, b"synthetic credential sentinel").unwrap();
    let before = std::fs::read(&fixture.accounts_path).unwrap();

    let metadata = FileManagedCodexAccountStore::new(&fixture.accounts_path)
        .load_account_metadata()
        .unwrap();
    assert_eq!(metadata.version, 1);
    assert!(
        metadata
            .accounts
            .iter()
            .all(|a| a.auth_fingerprint.is_none())
    );
    assert_eq!(fixture.collect().unwrap().accounts.unwrap().len(), 2);
    assert_eq!(std::fs::read(&fixture.accounts_path).unwrap(), before);
    assert_eq!(
        std::fs::read_to_string(&auth).unwrap(),
        "synthetic credential sentinel"
    );
}

// ============================================================================
// Section 4: DashboardManagedCodexSnapshotTests.swift (3 tests)
// ============================================================================

// upstream: DashboardManagedCodexSnapshotTests.swift:9 Codex accounts attach once with provider specific windows and identity policy
#[test]
fn codex_accounts_attach_once_with_provider_specific_windows_and_identity_policy() {
    let generated_at = Timestamp::from_second(1_800_000_000).unwrap();
    for mode in [
        DashboardIdentityMode::Full,
        DashboardIdentityMode::Redacted,
        DashboardIdentityMode::None,
    ] {
        let account = ProviderAccountUsageSnapshot {
            id: ProviderAccountIdentity::new("codex-managed", "test-account"),
            provider: "codex".to_string(),
            display_label: "owner@example.com · Work admin@company.test".to_string(),
            account_email: Some("owner@example.com".to_string()),
            is_active: true,
            uses_last_known_usage: false,
            snapshot: Some(usage_snapshot(
                Some(rate_window(
                    40.0,
                    Some(300),
                    Some(
                        generated_at
                            .checked_add(jiff::SignedDuration::from_secs(3600))
                            .unwrap(),
                    ),
                    None,
                )),
                Some(rate_window(
                    60.0,
                    Some(10080),
                    Some(
                        generated_at
                            .checked_add(jiff::SignedDuration::from_secs(172_800))
                            .unwrap(),
                    ),
                    None,
                )),
                None,
                None,
                None,
                None,
                None,
                generated_at,
                Some(ProviderIdentitySnapshot::new(
                    Some(pid("codex")),
                    Some("owner@example.com".to_string()),
                    None,
                    Some("plus".to_string()),
                    None,
                )),
            )),
            error: None,
            source_label: None,
        };
        let mut collections = BTreeMap::new();
        collections.insert(
            "codex".to_string(),
            DashboardAccountsInput {
                accounts: Some(vec![account]),
                adapter_error: None,
                weekly_work_days: None,
            },
        );
        let snapshot = DashboardSnapshotBuilder::make_snapshot(
            &[
                ProviderPayload::new("claude", "fixture"),
                ProviderPayload::new("codex", "fixture"),
                ProviderPayload::new("codex", "fixture"),
            ],
            &[],
            &config_with_providers(vec![("codex", true)]),
            mode,
            generated_at,
            0.0,
            None,
            &collections,
            false,
            false,
        );
        assert_eq!(snapshot.schema_version, 1);
        let rows = &snapshot.providers;
        assert_eq!(rows[0].accounts, None);
        assert_eq!(rows[2].accounts, None);
        let projected = &rows[1].accounts.as_ref().unwrap()[0];
        assert_eq!(projected.id, "codex-managed:test-account");
        assert!(projected.active);
        let expected_label = match mode {
            DashboardIdentityMode::Full => "owner@example.com · Work admin@company.test",
            DashboardIdentityMode::Redacted => "redacted@example.com · Work redacted@company.test",
            DashboardIdentityMode::None => "Account test-account",
        };
        let expected_email = match mode {
            DashboardIdentityMode::Full => Some("owner@example.com"),
            DashboardIdentityMode::Redacted => Some("redacted@example.com"),
            DashboardIdentityMode::None => None,
        };
        assert_eq!(projected.label, expected_label);
        assert_eq!(
            projected
                .identity
                .as_ref()
                .and_then(|i| i.account_email.as_deref()),
            expected_email
        );
        assert_eq!(
            projected.identity.as_ref().and_then(|i| i.plan.as_deref()),
            if mode == DashboardIdentityMode::None {
                None
            } else {
                Some("Plus")
            }
        );
        assert_eq!(
            projected
                .windows
                .iter()
                .map(|w| w.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["session", "weekly"]
        );
        assert!(
            projected
                .pace
                .as_ref()
                .and_then(|p| p.primary.as_ref())
                .is_some()
        );
        assert_eq!(projected.updated_at, Some(generated_at));
        assert_eq!(rows[1].source, "fixture");
    }
}

// upstream: DashboardManagedCodexSnapshotTests.swift:75 Codex whole adapter failure preserves ambient provider data
#[test]
fn codex_whole_adapter_failure_preserves_ambient_provider_data() {
    let generated_at = Timestamp::from_second(1_800_000_000).unwrap();
    let mut collections = BTreeMap::new();
    collections.insert(
        "codex".to_string(),
        DashboardAccountsInput {
            accounts: None,
            adapter_error: Some("Managed account metadata unavailable.".to_string()),
            weekly_work_days: None,
        },
    );
    let snapshot = DashboardSnapshotBuilder::make_snapshot(
        &[ProviderPayload::new("codex", "fixture")],
        &[],
        &config_with_providers(vec![("codex", true)]),
        DashboardIdentityMode::Full,
        generated_at,
        60.0,
        None,
        &collections,
        false,
        false,
    );
    assert_eq!(snapshot.providers[0].accounts, None);
    assert_eq!(
        snapshot.providers[0].accounts_error.as_deref(),
        Some("Managed account metadata unavailable.")
    );
    assert_eq!(snapshot.providers[0].source, "fixture");
}

// upstream: DashboardManagedCodexSnapshotTests.swift:92 producer collects accounts only for requested providers
#[tokio::test]
async fn producer_collects_accounts_only_for_requested_providers() {
    let collected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let mut producer = DashboardSnapshotProducer::new(
        Arc::new(|_| {
            Box::pin(async {
                Ok(UsageCommandOutput {
                    sections: vec![],
                    payload: vec![ProviderPayload::new("codex", "fixture")],
                    exit_code: 0,
                })
            })
        }),
        Arc::new(|_, _| Box::pin(async { vec![] })),
        Arc::new(|| Timestamp::from_second(1_800_000_000).unwrap()),
    );
    let c = Arc::clone(&collected);
    producer.collect_accounts = Arc::new(move |_, prov| {
        let c = Arc::clone(&c);
        Box::pin(async move {
            c.lock().unwrap().push(prov);
            Some(DashboardAccountsInput {
                accounts: Some(vec![]),
                adapter_error: None,
                weekly_work_days: None,
            })
        })
    });
    let result = producer
        .collect(
            &config_with_providers(vec![("codex", true), ("claude", true)]),
            0.0,
            None,
            None,
            Some(&["codex".to_string()]),
        )
        .await
        .unwrap();
    assert_eq!(collected.lock().unwrap().as_slice(), &["codex"]);
    assert!(
        result.payload.providers[0]
            .accounts
            .as_ref()
            .unwrap()
            .is_empty()
    );
}

// ============================================================================
// Section 5: DashboardReportedCostTests.swift (5 tests)
// ============================================================================

fn reported_cost_now() -> Timestamp {
    Timestamp::from_second(1_789_779_600).unwrap() // 2026-09-19 01:00 UTC
}

fn reported_cost_history(
    cost: Option<f64>,
    daily_cost: Option<f64>,
    currency: &str,
    days: i32,
) -> CostUsageTokenSnapshot {
    let mut snap = CostUsageTokenSnapshot::new(
        cost,
        vec![CostDailyEntryPayload::new(
            "2026-09-18",
            daily_cost,
            Some(120_000),
            None,
        )],
        reported_cost_now(),
    );
    snap.last_30_days_tokens = Some(120_000);
    snap.currency_code = currency.to_string();
    snap.history_days = days;
    snap.history_label = Some("Last 30 days (UTC)".to_string());
    snap
}

fn reported_cost_dashboard(
    history: Option<CostUsageTokenSnapshot>,
    local: Option<CostPayload>,
    provider: &str,
) -> DashboardSnapshotPayload {
    let usage = usage_snapshot(
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        reported_cost_now(),
        None,
    );
    let payload = ProviderPayload::new(provider, "api")
        .with_usage(usage)
        .with_cost_usage(history);
    DashboardSnapshotBuilder::make_snapshot(
        &[payload],
        &local.into_iter().collect::<Vec<_>>(),
        &config_with_providers(vec![(provider, true)]),
        DashboardIdentityMode::Redacted,
        reported_cost_now(),
        60.0,
        None,
        &BTreeMap::new(),
        false,
        false,
    )
}

// upstream: DashboardReportedCostTests.swift:9 reported history keeps its aggregate without reinterpreting completed UTC days as Today
#[test]
fn reported_history_keeps_its_aggregate_without_reinterpreting_completed_utc_days_as_today() {
    for reported in [0.0, 12.5] {
        let history = reported_cost_history(Some(reported), Some(1.25), "USD", 30);
        let snapshot = reported_cost_dashboard(Some(history), None, "openrouter");
        let cost = snapshot.providers[0].cost.as_ref().unwrap();
        assert_eq!(cost.last_30_days_usd, Some(reported));
        assert_eq!(cost.today_usd, None);
        assert_eq!(cost.today_incomplete_request_count, None);
        let obj = json_object(&snapshot);
        assert!(obj["providers"][0]["cost"]["todayUSD"].is_null());
        assert_eq!(obj["providers"][0]["cost"]["last30DaysUSD"], reported);
    }
}

// upstream: DashboardReportedCostTests.swift:27 missing reported amounts remain unavailable even when daily history has a local matching date
#[test]
fn missing_reported_amounts_remain_unavailable_even_when_daily_history_has_a_local_matching_date() {
    assert_eq!(
        reported_cost_dashboard(
            Some(reported_cost_history(None, Some(1.25), "USD", 30)),
            None,
            "openrouter"
        )
        .providers[0]
            .cost,
        None
    );
    assert_eq!(
        reported_cost_dashboard(None, None, "openrouter").providers[0].cost,
        None
    );
}

// upstream: DashboardReportedCostTests.swift:33 reported costs never relabel a different currency or window as thirty day USD
#[test]
fn reported_costs_never_relabel_a_different_currency_or_window_as_thirty_day_usd() {
    for (currency, days) in [("EUR", 30), ("USD", 7), ("USD", 90)] {
        let history = reported_cost_history(Some(12.5), None, currency, days);
        assert_eq!(
            reported_cost_dashboard(Some(history), None, "openrouter").providers[0].cost,
            None
        );
    }
}

// upstream: DashboardReportedCostTests.swift:41 local cost payload retains precedence including an unavailable local amount
#[test]
fn local_cost_payload_retains_precedence_including_an_unavailable_local_amount() {
    for local_amount in [Some(5.0), None] {
        let mut local = CostPayload::new("claude", "local");
        local.updated_at = Some(reported_cost_now());
        local.history_days = Some(30);
        local.last_30_days_cost_usd = local_amount;
        let snapshot = reported_cost_dashboard(
            Some(reported_cost_history(Some(99.0), None, "USD", 30)),
            Some(local),
            "claude",
        );
        assert_eq!(
            snapshot.providers[0]
                .cost
                .as_ref()
                .and_then(|c| c.last_30_days_usd),
            local_amount
        );
        assert_eq!(
            snapshot.providers[0]
                .cost
                .as_ref()
                .and_then(|c| c.today_usd),
            None
        );
    }
}

// upstream: DashboardReportedCostTests.swift:60 reported incomplete history preserves unknown money and its exclusion count
#[test]
fn reported_incomplete_history_preserves_unknown_money_and_its_exclusion_count() {
    let entry = CostDailyEntryPayload::new("2026-09-18", None, None, Some(2));
    let history = CostUsageTokenSnapshot::new(None, vec![entry], reported_cost_now());
    let snapshot = reported_cost_dashboard(Some(history), None, "openrouter");
    let cost = snapshot.providers[0].cost.as_ref().unwrap();
    assert_eq!(cost.last_30_days_usd, None);
    assert_eq!(cost.last_30_days_incomplete_request_count, Some(2));
    assert_eq!(cost.today_usd, None);
    assert_eq!(cost.today_incomplete_request_count, None);
}

// ============================================================================
// Section 6: DashboardSnapshotBuilderTests.swift (19 tests; 3 non-v1 Cursor/Amp tests documented in porting.md)
// ============================================================================

fn builder_identity_payload(email: &str) -> ProviderPayload {
    ProviderPayload::new("claude", "web").with_usage(usage_snapshot(
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Timestamp::UNIX_EPOCH,
        Some(ProviderIdentitySnapshot::new(
            Some(pid("claude")),
            Some(email.to_string()),
            None,
            Some("pro".to_string()),
            None,
        )),
    ))
}

fn builder_claude_swap_snapshot(
    number: i64,
    email: &str,
    usage_status: ClaudeSwapUsageStatus,
    identity_mode: DashboardIdentityMode,
) -> DashboardSnapshotPayload {
    let account = ClaudeSwapAccountProjection::account_snapshots(
        &ClaudeSwapAccountList {
            active_account_number: Some(number),
            accounts: vec![ClaudeSwapAccountRow {
                number,
                email: email.to_string(),
                organization_name: String::new(),
                alias: None,
                is_active: true,
                usage_status,
                five_hour: None,
                seven_day: None,
                scoped: vec![],
            }],
        },
        Timestamp::UNIX_EPOCH,
    );
    let mut collections = BTreeMap::new();
    collections.insert(
        "claude".to_string(),
        DashboardAccountsInput {
            accounts: Some(account),
            adapter_error: None,
            weekly_work_days: None,
        },
    );
    DashboardSnapshotBuilder::make_snapshot(
        &[builder_identity_payload("ambient@example.com")],
        &[],
        &config_with_providers(vec![("claude", true)]),
        identity_mode,
        Timestamp::UNIX_EPOCH,
        60.0,
        None,
        &collections,
        false,
        false,
    )
}

fn first_claude_swap_account(snapshot: &DashboardSnapshotPayload) -> serde_json::Value {
    let obj = json_object(snapshot);
    obj["providers"][0]["accounts"][0].clone()
}

fn first_identity(snapshot: &DashboardSnapshotPayload) -> serde_json::Value {
    let obj = json_object(snapshot);
    obj["providers"][0]["identity"].clone()
}

// partial-upstream: DashboardSnapshotBuilderTests.swift:57 dashboard producer forwards its config to cost collection
#[tokio::test]
async fn dashboard_producer_forwards_its_config_to_cost_collection() {
    let recorded = Arc::new(Mutex::new(None::<(Vec<String>, Config)>));
    let mut claude = ProviderConfig::new(pid("claude"));
    claude.enabled = Some(true);
    let config = Config::new(1, vec![ProviderEntry::Typed(claude)]);

    let r = Arc::clone(&recorded);
    let producer = DashboardSnapshotProducer::new(
        Arc::new(|_| Box::pin(async { Ok(UsageCommandOutput::default()) })),
        Arc::new(move |provs, cfg| {
            let r = Arc::clone(&r);
            Box::pin(async move {
                *r.lock().unwrap() = Some((provs, cfg));
                vec![]
            })
        }),
        Arc::new(|| Timestamp::from_second(1_800_000_000).unwrap()),
    );

    let _ = producer
        .collect(&config, 0.0, Some("9.8.7".to_string()), None, None)
        .await
        .unwrap();

    let (provs, cfg) = recorded.lock().unwrap().clone().unwrap();
    assert_eq!(provs, vec!["claude".to_string()]);
    assert!(cfg.provider_config("claude").is_some());
}

// upstream: DashboardSnapshotBuilderTests.swift:86 producer defaults to full identity and keeps stable order and partial errors
#[tokio::test]
async fn producer_defaults_to_full_identity_and_keeps_stable_order_and_partial_errors() {
    let generated_at = Timestamp::from_second(1_800_000_000).unwrap();
    let healthy = builder_identity_payload("user@example.com");
    let mut failed = ProviderPayload::new("codex", "auto");
    failed.error = Some(ProviderErrorPayload::provider("temporary failure"));
    let mut rows = HashMap::new();
    rows.insert("claude".to_string(), healthy);
    rows.insert("codex".to_string(), failed);

    let producer = DashboardSnapshotProducer::new(
        Arc::new(move |providers| {
            let rows = rows.clone();
            Box::pin(async move {
                let payload = providers
                    .into_iter()
                    .filter_map(|p| rows.get(&p).cloned())
                    .collect();
                Ok(UsageCommandOutput {
                    sections: vec![],
                    payload,
                    exit_code: 1,
                })
            })
        }),
        Arc::new(|_, _| Box::pin(async { vec![] })),
        Arc::new(move || generated_at),
    );
    let config = config_with_providers(vec![("claude", true), ("codex", true)]);

    let result = producer
        .collect(&config, 0.0, Some("9.8.7".to_string()), None, None)
        .await
        .unwrap();
    let obj = json_object(&result.payload);
    let providers = obj["providers"].as_array().unwrap();
    let ids: Vec<&str> = providers
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();

    assert_eq!(ids, vec!["codex", "claude"]);
    assert_eq!(providers[1]["identity"]["accountEmail"], "user@example.com");
    assert_eq!(providers[0]["error"]["message"], "temporary failure");
    assert_eq!(providers[0]["display"]["sortKey"], 10);
    assert_eq!(providers[1]["display"]["sortKey"], 0);
    assert_eq!(obj["generatedAt"], "2027-01-15T08:00:00Z");
}

// upstream: DashboardSnapshotBuilderTests.swift:135 producer provider filter collects and returns exactly one row
#[tokio::test]
async fn producer_provider_filter_collects_and_returns_exactly_one_row() {
    for provider in ["codex", "antigravity"] {
        let usage_recorded: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let cost_recorded: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let u = Arc::clone(&usage_recorded);
        let c = Arc::clone(&cost_recorded);
        let producer = DashboardSnapshotProducer::new(
            Arc::new(move |providers| {
                let u = Arc::clone(&u);
                Box::pin(async move {
                    *u.lock().unwrap() = providers.clone();
                    let payload = providers
                        .into_iter()
                        .map(|p| ProviderPayload::new(p, "test"))
                        .collect();
                    Ok(UsageCommandOutput {
                        sections: vec![],
                        payload,
                        exit_code: 0,
                    })
                })
            }),
            Arc::new(move |providers, _| {
                let c = Arc::clone(&c);
                Box::pin(async move {
                    *c.lock().unwrap() = providers;
                    vec![]
                })
            }),
            Arc::new(|| Timestamp::UNIX_EPOCH),
        );
        let config = config_with_providers(vec![("claude", true), ("codex", true)]);

        let result = producer
            .collect(&config, 60.0, None, None, Some(&[provider.to_string()]))
            .await
            .unwrap();
        let obj = json_object(&result.payload);
        let providers = obj["providers"].as_array().unwrap();
        let ids: Vec<&str> = providers
            .iter()
            .map(|p| p["id"].as_str().unwrap())
            .collect();

        assert_eq!(ids, vec![provider]);
        assert_eq!(usage_recorded.lock().unwrap().as_slice(), &[provider]);
        assert_eq!(cost_recorded.lock().unwrap().as_slice(), &[provider]);
    }
}

// upstream: DashboardSnapshotBuilderTests.swift:180 shell builder emits config fields only without fetch collaborators
#[test]
fn shell_builder_emits_config_fields_only_without_fetch_collaborators() {
    let config = config_with_providers(vec![("claude", true), ("codex", true), ("gemini", false)]);
    let snapshot = DashboardSnapshotBuilder::make_shell_snapshot(
        &config,
        None,
        Timestamp::UNIX_EPOCH,
        300.0,
        Some("9.8.7".to_string()),
        false,
    );
    let obj = json_object(&snapshot);
    let providers = obj["providers"].as_array().unwrap();
    let ids: Vec<&str> = providers
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();

    assert_eq!(obj["schemaVersion"], 1);
    assert_eq!(ids, vec!["claude", "codex"]);
    let expected_keys: BTreeSet<String> = ["id", "name", "enabled", "display"]
        .into_iter()
        .map(ToString::to_string)
        .collect();
    for prov in providers {
        let keys: BTreeSet<String> = prov.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, expected_keys);
    }
    assert_eq!(providers[0]["display"]["sortKey"], 0);
    assert_eq!(providers[1]["display"]["sortKey"], 10);
}

// partial-upstream: DashboardSnapshotBuilderTests.swift:211 builds stable display-oriented dashboard snapshot
#[test]
fn builds_stable_display_oriented_dashboard_snapshot() {
    let generated_at = Timestamp::from_second(1_800_000_000).unwrap();
    let updated_at = Timestamp::from_second(1_800_000_010).unwrap();
    let cost_updated_at = Timestamp::from_second(1_800_000_020).unwrap();
    let reset_at = Timestamp::from_second(1_800_003_600).unwrap();
    let generated_day = DashboardSnapshotBuilder::gregorian_day_key(generated_at);

    let usage = usage_snapshot(
        Some(rate_window(28.0, Some(300), Some(reset_at), None)),
        Some(rate_window(59.0, Some(10080), None, None)),
        None,
        None,
        None,
        None,
        None,
        updated_at,
        Some(ProviderIdentitySnapshot::new(
            Some(pid("codex")),
            Some("user@example.com".to_string()),
            None,
            Some("pro".to_string()),
            None,
        )),
    );
    let mut payload = ProviderPayload::new("codex", "oauth").with_usage(usage);
    payload.credits = Some(CreditsSnapshot::new(112.4, vec![], updated_at));
    payload.status = Some(ProviderStatusPayload {
        indicator: StatusIndicator::None,
        description: Some("Operational".to_string()),
        updated_at: Some(updated_at),
        url: "https://status.example.com".to_string(),
    });

    let mut cost = CostPayload::new("codex", "local");
    cost.updated_at = Some(cost_updated_at);
    cost.session_tokens = Some(1000);
    cost.session_cost_usd = Some(1.04);
    cost.history_days = Some(30);
    cost.last_30_days_tokens = Some(30000);
    cost.last_30_days_cost_usd = Some(18.22);
    cost.daily = vec![CostDailyEntryPayload::new(
        &generated_day,
        Some(1.04),
        Some(1000),
        None,
    )];
    cost.projects = Some(vec![CostProjectPayload {
        name: "Private project".to_string(),
        path: "/private/work/project".to_string(),
        total_tokens: Some(1000),
        total_cost_usd: Some(1.04),
        daily: vec![],
        model_breakdowns: None,
    }]);

    let config = config_with_providers(vec![("codex", true), ("claude", false)]);
    let snapshot = DashboardSnapshotBuilder::make_snapshot(
        &[payload],
        &[cost],
        &config,
        DashboardIdentityMode::Redacted,
        generated_at,
        60.0,
        Some("9.8.7".to_string()),
        &BTreeMap::new(),
        false,
        false,
    );
    let obj = json_object(&snapshot);
    let provider = &obj["providers"][0];
    let host = &obj["host"];
    let identity = &provider["identity"];
    let status = &provider["status"];
    let windows = provider["windows"].as_array().unwrap();
    let credits = &provider["credits"];
    let cost_object = &provider["cost"];
    let encoded = serde_json::to_string(&snapshot).unwrap();
    assert!(!encoded.contains("Private project"));
    assert!(!encoded.contains("/private/work/project"));
    let display = &provider["display"];

    assert_eq!(obj["schemaVersion"], 1);
    assert_eq!(obj["staleAfterSeconds"], 180);
    assert_eq!(host["ariadusageVersion"], "9.8.7");
    assert_eq!(host["refreshIntervalSeconds"], 60);
    assert_eq!(host["usageBarsShowUsed"], false);

    assert_eq!(provider["id"], "codex");
    assert_eq!(provider["name"], "Codex");
    assert_eq!(provider["enabled"], true);
    assert_eq!(provider["source"], "oauth");
    assert!(provider["error"].is_null());
    assert_eq!(provider["updatedAt"], "2027-01-15T08:00:20Z");

    assert_eq!(status["level"], "ok");
    assert_eq!(status["label"], "Operational");
    assert_eq!(identity["accountEmail"], "redacted@example.com");
    assert_eq!(identity["plan"], "Pro 20x");

    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0]["kind"], "session");
    assert_eq!(windows[0]["label"], "Session");
    assert_eq!(windows[0]["usedPercent"], 28.0);
    assert_eq!(windows[0]["remainingPercent"], 72.0);
    assert_eq!(windows[0]["resetAt"], "2027-01-15T09:00:00Z");
    assert_eq!(windows[1]["kind"], "weekly");
    assert_eq!(windows[1]["label"], "Weekly");

    assert_eq!(credits["remaining"], 112.4);
    assert_eq!(credits["unit"], "credits");
    assert_eq!(cost_object["todayUSD"], 1.04);
    assert_eq!(cost_object["last30DaysUSD"], 18.22);
    assert_eq!(display["accentColor"], "#49A3B0");
    assert_eq!(display["sortKey"], 0);
    assert_eq!(display["priority"], "normal");
}

// upstream: DashboardSnapshotBuilderTests.swift:344 dashboard identity mode none emits null identity
#[test]
fn dashboard_identity_mode_none_emits_null_identity() {
    let payload = builder_identity_payload("user@example.com");
    let snapshot = DashboardSnapshotBuilder::make_snapshot(
        &[payload],
        &[],
        &config_with_providers(vec![("claude", true)]),
        DashboardIdentityMode::None,
        Timestamp::UNIX_EPOCH,
        60.0,
        None,
        &BTreeMap::new(),
        false,
        false,
    );
    let obj = json_object(&snapshot);
    let provider = &obj["providers"][0];

    assert!(provider["identity"].is_null());
    assert!(provider["status"].is_null());
    assert!(provider["credits"].is_null());
    assert!(provider["cost"].is_null());
}

// upstream: DashboardSnapshotBuilderTests.swift:429 dashboard identity mode redacted hides local part but keeps domain
#[test]
fn dashboard_identity_mode_redacted_hides_local_part_but_keeps_domain() {
    let snapshot = DashboardSnapshotBuilder::make_snapshot(
        &[builder_identity_payload("user@example.com")],
        &[],
        &config_with_providers(vec![("claude", true)]),
        DashboardIdentityMode::Redacted,
        Timestamp::UNIX_EPOCH,
        60.0,
        None,
        &BTreeMap::new(),
        false,
        false,
    );
    let domainless = DashboardSnapshotBuilder::make_snapshot(
        &[builder_identity_payload("not-an-email")],
        &[],
        &config_with_providers(vec![("claude", true)]),
        DashboardIdentityMode::Redacted,
        Timestamp::UNIX_EPOCH,
        60.0,
        None,
        &BTreeMap::new(),
        false,
        false,
    );

    assert_eq!(
        first_identity(&snapshot)["accountEmail"],
        "redacted@example.com"
    );
    assert_eq!(first_identity(&domainless)["accountEmail"], "redacted");
}

// upstream: DashboardSnapshotBuilderTests.swift:454 claude swap account keeps email when usage fetch fails
#[test]
fn claude_swap_account_keeps_email_when_usage_fetch_fails() {
    let snapshot = builder_claude_swap_snapshot(
        1,
        "stale@example.com",
        ClaudeSwapUsageStatus::TokenExpired,
        DashboardIdentityMode::Full,
    );
    let account = first_claude_swap_account(&snapshot);

    assert_eq!(account["identity"]["accountEmail"], "stale@example.com");
    assert_eq!(account["label"], "stale@example.com");
    assert!(account["windows"].as_array().unwrap().is_empty());
}

// upstream: DashboardSnapshotBuilderTests.swift:469 claude swap failed account redacts email in identity and label
#[test]
fn claude_swap_failed_account_redacts_email_in_identity_and_label() {
    let snapshot = builder_claude_swap_snapshot(
        4,
        "private-person@example.com",
        ClaudeSwapUsageStatus::NoCredentials,
        DashboardIdentityMode::Redacted,
    );
    let account = first_claude_swap_account(&snapshot);
    let encoded = serde_json::to_string(&snapshot).unwrap();

    assert_eq!(account["identity"]["accountEmail"], "redacted@example.com");
    assert_eq!(account["label"], "redacted@example.com");
    assert!(!encoded.contains("private-person"));
}

// upstream: DashboardSnapshotBuilderTests.swift:485 claude swap placeholder label stays a slot label without identity
#[test]
fn claude_swap_placeholder_label_stays_a_slot_label_without_identity() {
    let snapshot = builder_claude_swap_snapshot(
        7,
        "",
        ClaudeSwapUsageStatus::Unavailable,
        DashboardIdentityMode::Full,
    );
    let account = first_claude_swap_account(&snapshot);

    assert_eq!(account["label"], "Account 7");
    assert!(account["identity"].is_null());
}

// upstream: DashboardSnapshotBuilderTests.swift:498 dashboard redaction keeps only the final email domain
#[test]
fn dashboard_redaction_keeps_only_the_final_email_domain() {
    let snapshot = DashboardSnapshotBuilder::make_snapshot(
        &[builder_identity_payload("\"foo@bar\"@example.com")],
        &[],
        &config_with_providers(vec![("claude", true)]),
        DashboardIdentityMode::Redacted,
        Timestamp::UNIX_EPOCH,
        60.0,
        None,
        &BTreeMap::new(),
        false,
        false,
    );

    assert_eq!(
        first_identity(&snapshot)["accountEmail"],
        "redacted@example.com"
    );
}

// upstream: DashboardSnapshotBuilderTests.swift:513 dashboard provider errors are projected without raw usage internals
#[test]
fn dashboard_provider_errors_are_projected_without_raw_usage_internals() {
    let mut payload = ProviderPayload::new("codex", "auto");
    payload.error = Some(ProviderErrorPayload::provider("temporary failure"));

    let snapshot = DashboardSnapshotBuilder::make_snapshot(
        &[payload],
        &[],
        &config_with_providers(vec![("codex", true)]),
        DashboardIdentityMode::Redacted,
        Timestamp::UNIX_EPOCH,
        60.0,
        None,
        &BTreeMap::new(),
        false,
        false,
    );
    let obj = json_object(&snapshot);
    let provider = &obj["providers"][0];

    assert!(provider["windows"].as_array().unwrap().is_empty());
    assert_eq!(provider["error"]["message"], "temporary failure");
    assert!(provider.get("usage").is_none());
    assert!(provider.get("openaiDashboard").is_none());
}

// upstream: DashboardSnapshotBuilderTests.swift:545 dashboard surfaces cost failures when usage succeeds
#[test]
fn dashboard_surfaces_cost_failures_when_usage_succeeds() {
    let usage = builder_identity_payload("user@example.com");
    let mut cost = CostPayload::new("claude", "local");
    cost.updated_at = Some(Timestamp::from_second(10).unwrap());
    cost.error = Some(ProviderErrorPayload::provider("cost unavailable"));

    let snapshot = DashboardSnapshotBuilder::make_snapshot(
        &[usage],
        &[cost],
        &config_with_providers(vec![("claude", true)]),
        DashboardIdentityMode::Redacted,
        Timestamp::from_second(20).unwrap(),
        60.0,
        None,
        &BTreeMap::new(),
        false,
        false,
    );
    let obj = json_object(&snapshot);
    let provider = &obj["providers"][0];

    assert_eq!(provider["error"]["message"], "cost unavailable");
    assert_eq!(provider["updatedAt"], "1970-01-01T00:00:10Z");
}

// upstream: DashboardSnapshotBuilderTests.swift:577 dashboard provider freshness includes status updates
#[test]
fn dashboard_provider_freshness_includes_status_updates() {
    let mut payload = ProviderPayload::new("claude", "status");
    payload.status = Some(ProviderStatusPayload {
        indicator: StatusIndicator::None,
        description: Some("Operational".to_string()),
        updated_at: Some(Timestamp::from_second(30).unwrap()),
        url: "https://status.anthropic.com".to_string(),
    });

    let snapshot = DashboardSnapshotBuilder::make_snapshot(
        &[payload],
        &[],
        &config_with_providers(vec![("claude", true)]),
        DashboardIdentityMode::Redacted,
        Timestamp::from_second(40).unwrap(),
        60.0,
        None,
        &BTreeMap::new(),
        false,
        false,
    );
    let obj = json_object(&snapshot);
    let provider = &obj["providers"][0];

    assert_eq!(provider["updatedAt"], "1970-01-01T00:00:30Z");
}

// upstream: DashboardSnapshotBuilderTests.swift:609 dashboard safely clamps extreme refresh intervals
#[test]
fn dashboard_safely_clamps_extreme_refresh_intervals() {
    let snapshot = DashboardSnapshotBuilder::make_snapshot(
        &[],
        &[],
        &empty_config(),
        DashboardIdentityMode::Redacted,
        Timestamp::UNIX_EPOCH,
        f64::MAX,
        None,
        &BTreeMap::new(),
        false,
        false,
    );

    assert_eq!(snapshot.host.refresh_interval_seconds, i64::MAX / 3);
    assert_eq!(snapshot.stale_after_seconds, (i64::MAX / 3) * 3);
}

// upstream: DashboardSnapshotBuilderTests.swift:624 dashboard snapshot builder serializes usage bars show used host preference
#[test]
fn dashboard_snapshot_builder_serializes_usage_bars_show_used_host_preference() {
    let snapshot = DashboardSnapshotBuilder::make_snapshot(
        &[],
        &[],
        &empty_config(),
        DashboardIdentityMode::None,
        Timestamp::now(),
        60.0,
        Some("1.0.0".to_string()),
        &BTreeMap::new(),
        true,
        false,
    );
    let obj = json_object(&snapshot);
    assert_eq!(obj["host"]["usageBarsShowUsed"], true);

    let shell_snapshot = DashboardSnapshotBuilder::make_shell_snapshot(
        &empty_config(),
        None,
        Timestamp::now(),
        60.0,
        Some("1.0.0".to_string()),
        true,
    );
    let shell_obj = json_object(&shell_snapshot);
    assert_eq!(shell_obj["host"]["usageBarsShowUsed"], true);
}

// upstream: DashboardSnapshotBuilderTests.swift:650 dashboard daily cost uses generation day without update metadata
#[test]
fn dashboard_daily_cost_uses_generation_day_without_update_metadata() {
    let generated_at = Timestamp::from_second(1_800_000_000).unwrap();
    let usage = builder_identity_payload("user@example.com");
    let mut cost = CostPayload::new("claude", "local");
    cost.history_days = Some(1);
    cost.daily = vec![CostDailyEntryPayload::new(
        DashboardSnapshotBuilder::gregorian_day_key(generated_at),
        Some(2.5),
        None,
        None,
    )];

    let snapshot = DashboardSnapshotBuilder::make_snapshot(
        &[usage],
        &[cost],
        &config_with_providers(vec![("claude", true)]),
        DashboardIdentityMode::Redacted,
        generated_at,
        60.0,
        None,
        &BTreeMap::new(),
        false,
        false,
    );
    let obj = json_object(&snapshot);
    assert_eq!(obj["providers"][0]["cost"]["todayUSD"], 2.5);
}

// upstream: DashboardSnapshotBuilderTests.swift:691 dashboard keeps incomplete only costs and scopes Today to its generation date
#[test]
fn dashboard_keeps_incomplete_only_costs_and_scopes_today_to_its_generation_date() {
    let local_day = Timestamp::from_second(1_800_000_000).unwrap();
    let prior_day = local_day
        .checked_sub(jiff::SignedDuration::from_secs(24 * 60 * 60))
        .unwrap();
    let entries = vec![CostDailyEntryPayload::new(
        DashboardSnapshotBuilder::gregorian_day_key(prior_day),
        None,
        None,
        Some(2),
    )];
    let cost = ariadusage_engine::serve::dashboard::make_cost_payload(
        "claude",
        Some(CostUsageTokenSnapshot::new(None, entries, local_day)),
        None,
    );
    for (generated_at, expected_today_count) in [(prior_day, 2), (local_day, 0)] {
        let snapshot = DashboardSnapshotBuilder::make_snapshot(
            &[builder_identity_payload("fixture@example.test")],
            std::slice::from_ref(&cost),
            &config_with_providers(vec![("claude", true)]),
            DashboardIdentityMode::Redacted,
            generated_at,
            60.0,
            None,
            &BTreeMap::new(),
            false,
            false,
        );
        let obj = json_object(&snapshot);
        let payload = &obj["providers"][0]["cost"];
        assert!(payload["todayUSD"].is_null());
        assert!(payload["last30DaysUSD"].is_null());
        assert_eq!(
            payload["todayIncompleteRequestCount"].as_i64().unwrap_or(0),
            expected_today_count
        );
        assert_eq!(payload["last30DaysIncompleteRequestCount"], 2);
    }
}

// upstream: DashboardSnapshotBuilderTests.swift:726 dashboard narrows amounts and exclusions together for histories longer than thirty days
#[test]
fn dashboard_narrows_amounts_and_exclusions_together_for_histories_longer_than_thirty_days() {
    let now = Timestamp::from_second(1_800_000_000).unwrap();
    let old = now
        .checked_sub(jiff::SignedDuration::from_secs(45 * 24 * 60 * 60))
        .unwrap();
    let mut snap = CostUsageTokenSnapshot::new(
        Some(12.0),
        vec![
            CostDailyEntryPayload::new(
                DashboardSnapshotBuilder::gregorian_day_key(old),
                Some(7.0),
                Some(700),
                Some(1),
            ),
            CostDailyEntryPayload::new(
                DashboardSnapshotBuilder::gregorian_day_key(now),
                Some(5.0),
                Some(500),
                None,
            ),
        ],
        now,
    );
    snap.session_tokens = Some(500);
    snap.session_cost_usd = Some(5.0);
    snap.last_30_days_tokens = Some(1200);
    snap.history_days = 90;

    let cost = ariadusage_engine::serve::dashboard::make_cost_payload("claude", Some(snap), None);
    assert_eq!(cost.last_30_days_cost_usd, Some(5.0));
    assert_eq!(
        cost.totals.as_ref().and_then(|t| t.total_cost_usd),
        Some(12.0)
    );
    assert_eq!(cost.incomplete_request_count, Some(1));

    let snapshot = DashboardSnapshotBuilder::make_snapshot(
        &[builder_identity_payload("fixture@example.test")],
        &[cost],
        &config_with_providers(vec![("claude", true)]),
        DashboardIdentityMode::Redacted,
        now,
        60.0,
        None,
        &BTreeMap::new(),
        false,
        false,
    );
    let projected = snapshot.providers[0].cost.as_ref().unwrap();
    assert_eq!(projected.today_usd, Some(5.0));
    assert_eq!(projected.last_30_days_usd, Some(5.0));
    assert_eq!(projected.today_incomplete_request_count, None);
    assert_eq!(projected.last_30_days_incomplete_request_count, None);
}

// ============================================================================
// Section 7: DashboardAccountInventoryTests.swift (5 tests)
// ============================================================================

fn inventory_account_id(index: usize) -> String {
    format!("token:00000000-0000-0000-0000-00000000000{}", index + 1)
}

fn inventory_token_config() -> Config {
    let accounts: Vec<TokenAccountMeta> = (0..2)
        .map(|index| {
            make_token_account_meta(
                format!("00000000-0000-0000-0000-00000000000{}", index + 1),
                format!("Private account {}", index + 1),
                0.0,
                None,
            )
        })
        .collect();
    let mut provider = ProviderConfig::new(pid("claude"));
    provider.enabled = Some(true);
    provider.token_accounts = Some(make_token_accounts_meta(1, 0, accounts));
    Config::new(1, vec![ProviderEntry::Typed(provider)])
}

fn inventory_snapshot(
    payloads: &[ProviderPayload],
    mode: DashboardIdentityMode,
    config: &Config,
    expanded: bool,
    adapter: Option<DashboardAccountsInput>,
) -> DashboardSnapshotPayload {
    let mut collections = BTreeMap::new();
    if let Some(a) = adapter {
        collections.insert("claude".to_string(), a);
    }
    DashboardSnapshotBuilder::make_snapshot(
        payloads,
        &[],
        config,
        mode,
        Timestamp::UNIX_EPOCH,
        60.0,
        None,
        &collections,
        false,
        expanded,
    )
}

fn inventory_payload(provider: &str, id: &str, error: Option<&str>) -> ProviderPayload {
    let mut payload = ProviderPayload::new(provider, "fixture");
    if let Some(err) = error {
        payload.error = Some(ProviderErrorPayload::provider(err));
    } else {
        payload.usage = Some(usage_snapshot(
            Some(rate_window(25.0, Some(300), None, None)),
            None,
            None,
            None,
            None,
            None,
            None,
            Timestamp::UNIX_EPOCH,
            None,
        ));
    }
    payload.dashboard_account = Some(DashboardUsageAccount::new(id, "Private account", false));
    payload
}

// upstream: DashboardAccountInventoryTests.swift:7 unreadable inventory warns without losing healthy accounts or disclosing diagnostics
#[test]
fn unreadable_inventory_warns_without_losing_healthy_accounts_or_disclosing_diagnostics() {
    for mode in [
        DashboardIdentityMode::None,
        DashboardIdentityMode::Redacted,
        DashboardIdentityMode::Full,
    ] {
        let mut payload = inventory_payload("codex", "visible-profile", None);
        payload.dashboard_accounts_incomplete = true;
        let snap = inventory_snapshot(
            std::slice::from_ref(&payload),
            mode,
            &empty_config(),
            true,
            None,
        );
        let row = &snap.providers[0];
        assert_eq!(
            row.accounts_error.as_deref(),
            Some("Account list incomplete")
        );
        assert_eq!(row.accounts.as_ref().unwrap().len(), 1);
        assert_eq!(
            row.accounts.as_ref().unwrap()[0].windows[0].used_percent,
            25.0
        );
        assert!(row.error.is_none());
        let unexpanded = inventory_snapshot(
            std::slice::from_ref(&payload),
            mode,
            &empty_config(),
            false,
            None,
        );
        assert_eq!(unexpanded.providers[0].accounts_error, None);
        let json = serde_json::to_string(&payload).unwrap();
        assert!(!json.contains("dashboardAccountsIncomplete"));
        assert!(!json.contains("Account list incomplete"));
    }
}

// upstream: DashboardAccountInventoryTests.swift:24 missing configured account warns but an errored account still counts as collected
#[test]
fn missing_configured_account_warns_but_an_errored_account_still_counts_as_collected() {
    let cfg = inventory_token_config();
    for mode in [
        DashboardIdentityMode::None,
        DashboardIdentityMode::Redacted,
        DashboardIdentityMode::Full,
    ] {
        let first = inventory_payload("claude", &inventory_account_id(0), None);
        let failed = inventory_payload(
            "claude",
            &inventory_account_id(1),
            Some("Synthetic account failure"),
        );
        let missing_snap = inventory_snapshot(std::slice::from_ref(&first), mode, &cfg, true, None);
        let missing = &missing_snap.providers[0];
        assert_eq!(
            missing.accounts_error.as_deref(),
            Some("Account list incomplete")
        );
        assert_eq!(
            missing.accounts.as_ref().unwrap()[0].windows[0].used_percent,
            25.0
        );
        assert!(missing.error.is_none());

        let complete_snap = inventory_snapshot(&[first.clone(), failed], mode, &cfg, true, None);
        let complete = &complete_snap.providers[0];
        assert_eq!(complete.accounts_error, None);
        assert_eq!(complete.accounts.as_ref().unwrap().len(), 2);
        assert!(complete.accounts.as_ref().unwrap()[1].error.is_some());

        let unexpanded = inventory_snapshot(&[first], mode, &cfg, false, None);
        assert_eq!(unexpanded.providers[0].accounts_error, None);
    }
}

// upstream: DashboardAccountInventoryTests.swift:43 duplicate rows cannot hide a missing configured account
#[test]
fn duplicate_rows_cannot_hide_a_missing_configured_account() {
    let cfg = inventory_token_config();
    let first = inventory_payload("claude", &inventory_account_id(0), None);
    let snap = inventory_snapshot(
        &[first.clone(), first.clone()],
        DashboardIdentityMode::None,
        &cfg,
        true,
        None,
    );
    assert_eq!(
        snap.providers[0].accounts_error.as_deref(),
        Some("Account list incomplete")
    );
    let complete = inventory_snapshot(
        &[
            first,
            inventory_payload("claude", &inventory_account_id(1), None),
        ],
        DashboardIdentityMode::None,
        &cfg,
        true,
        None,
    );
    assert_eq!(complete.providers[0].accounts_error, None);
}

// upstream: DashboardAccountInventoryTests.swift:54 configured adapter remains authoritative over token inventory warnings
#[test]
fn configured_adapter_remains_authoritative_over_token_inventory_warnings() {
    let cfg = inventory_token_config();
    for failed in [false, true] {
        for mode in [
            DashboardIdentityMode::None,
            DashboardIdentityMode::Redacted,
            DashboardIdentityMode::Full,
        ] {
            let mut payload = inventory_payload("claude", &inventory_account_id(0), None);
            payload.dashboard_accounts_incomplete = true;
            let adapter = DashboardAccountsInput {
                accounts: if failed { None } else { Some(vec![]) },
                adapter_error: if failed {
                    Some("Synthetic adapter failure".to_string())
                } else {
                    None
                },
                weekly_work_days: None,
            };
            let snap = inventory_snapshot(&[payload], mode, &cfg, true, Some(adapter));
            let row = &snap.providers[0];
            let expected_err = if failed {
                if mode == DashboardIdentityMode::Full {
                    Some("Synthetic adapter failure")
                } else {
                    Some("Account list unavailable")
                }
            } else {
                None
            };
            assert_eq!(row.accounts_error.as_deref(), expected_err);
            if failed {
                assert!(row.accounts.is_none());
            } else {
                assert!(row.accounts.as_ref().unwrap().is_empty());
            }
        }
    }
}

// upstream: DashboardAccountInventoryTests.swift:76 collector preserves discovery warning in partial and completed results including ambient fallback
#[tokio::test]
async fn collector_preserves_discovery_warning_in_partial_and_completed_results_including_ambient_fallback()
 {
    for has_visible_account in [false, true] {
        let payload = inventory_payload("codex", "visible-profile", None);
        let account = if has_visible_account {
            payload.dashboard_account.clone()
        } else {
            None
        };
        let recorded: Arc<Mutex<Vec<UsageCommandOutput>>> = Arc::new(Mutex::new(Vec::new()));
        let rec = Arc::clone(&recorded);
        let sink = ariadusage_engine::serve::coordinator::CliServeOperationProgressSink::new(
            move |out| {
                let rec = Arc::clone(&rec);
                async move {
                    rec.lock().unwrap().push(out);
                }
            },
            tokio_util::sync::CancellationToken::new(),
        );
        let p_clone = payload.clone();
        let result =
            collect_account_usage("codex", &[account], true, None, Some(&sink), move |_| {
                let mut fetched = p_clone.clone();
                fetched.dashboard_account = None;
                async move {
                    UsageCommandOutput {
                        sections: vec![],
                        payload: vec![fetched],
                        exit_code: 0,
                    }
                }
            })
            .await;

        let partials = recorded.lock().unwrap().clone();
        assert_eq!(partials.len(), 1);
        for output in partials.into_iter().chain(std::iter::once(result.clone())) {
            let row = &output.payload[0];
            assert!(row.dashboard_accounts_incomplete);
            assert_eq!(row.dashboard_account.is_some(), has_visible_account);
            let snap = inventory_snapshot(
                &output.payload,
                DashboardIdentityMode::None,
                &empty_config(),
                true,
                None,
            );
            assert_eq!(
                snap.providers[0].accounts_error.as_deref(),
                Some("Account list incomplete")
            );
        }
        assert_eq!(
            result.payload[0]
                .usage
                .as_ref()
                .and_then(|u| u.primary.as_ref())
                .map(|w| w.used_percent),
            Some(25.0)
        );
    }
}

// ============================================================================
// Section 8: DashboardAccountPrivacyTests.swift (5 tests)
// ============================================================================

fn privacy_config() -> Config {
    config_with_providers(vec![("claude", true)])
}

fn privacy_producer(payloads: Vec<ProviderPayload>, expanded: bool) -> DashboardSnapshotProducer {
    let mut producer = DashboardSnapshotProducer::new(
        Arc::new(move |_| {
            let p = payloads.clone();
            Box::pin(async move {
                Ok(UsageCommandOutput {
                    sections: vec![],
                    payload: p,
                    exit_code: 0,
                })
            })
        }),
        Arc::new(|_, _| Box::pin(async { vec![] })),
        Arc::new(|| Timestamp::UNIX_EPOCH),
    );
    producer.all_accounts = expanded;
    producer
}

fn privacy_payload(error: Option<&str>) -> ProviderPayload {
    let mut payload = ProviderPayload::new("claude", "fixture");
    if let Some(err) = error {
        payload.error = Some(ProviderErrorPayload::provider(err));
    } else {
        payload.usage = Some(usage_snapshot(
            Some(rate_window(25.0, Some(300), None, None)),
            None,
            None,
            None,
            None,
            None,
            None,
            Timestamp::UNIX_EPOCH,
            Some(ProviderIdentitySnapshot::new(
                Some(pid("claude")),
                Some("person@example.test".to_string()),
                Some("Private Team".to_string()),
                Some("pro".to_string()),
                None,
            )),
        ));
    }
    payload.dashboard_account = Some(DashboardUsageAccount::new("opaque", "Private Team", true));
    payload
}

// upstream: DashboardAccountPrivacyTests.swift:7 expanded defaults never inherit the app identity preference
#[test]
fn expanded_defaults_never_inherit_the_app_identity_preference() {
    for hide_personal_info in [false, true] {
        assert_eq!(
            resolve_dashboard_identity_mode(None, hide_personal_info, true),
            DashboardIdentityMode::None
        );
        for explicit in [DashboardIdentityMode::Full, DashboardIdentityMode::Redacted] {
            assert_eq!(
                resolve_dashboard_identity_mode(Some(explicit), hide_personal_info, true),
                explicit
            );
        }
        assert_eq!(
            resolve_dashboard_identity_mode(None, hide_personal_info, false),
            if hide_personal_info {
                DashboardIdentityMode::Redacted
            } else {
                DashboardIdentityMode::Full
            }
        );
    }
}

// upstream: DashboardAccountPrivacyTests.swift:19 account labels and errors require full identity while usage remains available
#[tokio::test]
async fn account_labels_and_errors_require_full_identity_while_usage_remains_available() {
    for mode in [
        DashboardIdentityMode::None,
        DashboardIdentityMode::Redacted,
        DashboardIdentityMode::Full,
    ] {
        let mut healthy = privacy_payload(None);
        healthy.dashboard_account = Some(DashboardUsageAccount::new(
            "opaque-1",
            "Private Team",
            false,
        ));
        let mut failed = privacy_payload(Some(
            "Expired for Private Team: person@example.test at /home/private",
        ));
        failed.dashboard_account = Some(DashboardUsageAccount::new(
            "opaque-2",
            "Secret Client",
            true,
        ));
        let producer = privacy_producer(vec![healthy, failed], true);
        let result = producer
            .collect(
                &privacy_config(),
                60.0,
                None,
                if mode == DashboardIdentityMode::None {
                    None
                } else {
                    Some(mode)
                },
                None,
            )
            .await
            .unwrap();
        let row = &result.payload.providers[0];
        let accounts = row.accounts.as_ref().unwrap();
        let labels: Vec<&str> = accounts.iter().map(|a| a.label.as_str()).collect();
        let expected_labels = if mode == DashboardIdentityMode::Full {
            vec!["Private Team", "Secret Client"]
        } else {
            vec!["Account 1", "Account 2"]
        };
        assert_eq!(labels, expected_labels);
        assert_eq!(
            accounts.iter().map(|a| a.active).collect::<Vec<_>>(),
            vec![false, true]
        );
        assert_eq!(accounts[0].windows[0].used_percent, 25.0);
        assert!(accounts[1].windows.is_empty());
        assert!(row.windows.is_empty());
        assert_eq!(
            row.error.as_ref().map(|e| e.message.as_str()),
            accounts[1].error.as_deref()
        );
        let expected_email = match mode {
            DashboardIdentityMode::None => None,
            DashboardIdentityMode::Full => Some("person@example.test"),
            DashboardIdentityMode::Redacted => Some("redacted@example.test"),
        };
        assert_eq!(
            accounts[0]
                .identity
                .as_ref()
                .and_then(|i| i.account_email.as_deref()),
            expected_email
        );
        let json = serde_json::to_string(&result.payload).unwrap();
        if mode != DashboardIdentityMode::Full {
            for secret in [
                "Private Team",
                "Secret Client",
                "person@example.test",
                "/home/private",
            ] {
                assert!(!json.contains(secret));
            }
            assert_eq!(
                accounts[1].error.as_deref(),
                Some("Account usage unavailable")
            );
        } else {
            assert!(json.contains("Private Team"));
            assert!(json.contains("person@example.test"));
        }
    }
}

// upstream: DashboardAccountPrivacyTests.swift:59 configured adapter failure omits accounts even with token account fallback available
#[tokio::test]
async fn configured_adapter_failure_omits_accounts_even_with_token_account_fallback_available() {
    for mode in [
        DashboardIdentityMode::None,
        DashboardIdentityMode::Redacted,
        DashboardIdentityMode::Full,
    ] {
        for expanded in [false, true] {
            let diagnostic = "Private Team at /home/private: person@example.test";
            let mut producer = privacy_producer(vec![privacy_payload(None)], expanded);
            producer.collect_accounts = Arc::new(move |_, _| {
                Box::pin(async move {
                    Some(DashboardAccountsInput {
                        accounts: None,
                        adapter_error: Some(diagnostic.to_string()),
                        weekly_work_days: None,
                    })
                })
            });
            let result = producer
                .collect(&privacy_config(), 60.0, None, Some(mode), None)
                .await
                .unwrap();
            let row = &result.payload.providers[0];
            assert!(row.accounts.is_none());
            let expected_err = if expanded && mode != DashboardIdentityMode::Full {
                "Account list unavailable"
            } else {
                diagnostic
            };
            assert_eq!(row.accounts_error.as_deref(), Some(expected_err));
            assert_eq!(row.windows[0].used_percent, 25.0);
            let obj = json_object(&result.payload);
            assert!(obj["providers"][0].get("accounts").is_none());
            if expanded && mode != DashboardIdentityMode::Full {
                let json = serde_json::to_string(&result.payload).unwrap();
                assert!(!json.contains("Private Team"));
                assert!(!json.contains("person@example.test"));
                assert!(!json.contains("/home/private"));
            }
        }
    }
}

// upstream: DashboardAccountPrivacyTests.swift:86 adapter aliases and errors are private and successful empty results stay authoritative
#[tokio::test]
async fn adapter_aliases_and_errors_are_private_and_successful_empty_results_stay_authoritative() {
    for mode in [
        DashboardIdentityMode::None,
        DashboardIdentityMode::Redacted,
        DashboardIdentityMode::Full,
    ] {
        let account = ProviderAccountUsageSnapshot {
            id: ProviderAccountIdentity::new("claude-swap", "7"),
            provider: "claude".to_string(),
            display_label: "Private Team".to_string(),
            account_email: Some("person@example.test".to_string()),
            is_active: true,
            uses_last_known_usage: false,
            snapshot: None,
            error: Some("Private Team expired".to_string()),
            source_label: None,
        };
        for rows in [vec![account], vec![]] {
            let mut producer = privacy_producer(vec![privacy_payload(None)], true);
            let rows_clone = rows.clone();
            producer.collect_accounts = Arc::new(move |_, _| {
                let r = rows_clone.clone();
                Box::pin(async move {
                    Some(DashboardAccountsInput {
                        accounts: Some(r),
                        adapter_error: None,
                        weekly_work_days: None,
                    })
                })
            });
            let result = producer
                .collect(&privacy_config(), 60.0, None, Some(mode), None)
                .await
                .unwrap();
            let provider = &result.payload.providers[0];
            let accounts = provider.accounts.as_ref().unwrap();
            assert_eq!(accounts.len(), rows.len());
            assert_eq!(provider.accounts_error, None);
            if let Some(first) = accounts.first() {
                assert_eq!(first.id, "claude-swap:7");
                assert_eq!(
                    first.label,
                    if mode == DashboardIdentityMode::Full {
                        "Private Team"
                    } else {
                        "Account 1"
                    }
                );
                assert_eq!(
                    first.error.as_deref(),
                    if mode == DashboardIdentityMode::Full {
                        Some("Private Team expired")
                    } else {
                        Some("Account usage unavailable")
                    }
                );
                let expected_email = match mode {
                    DashboardIdentityMode::None => None,
                    DashboardIdentityMode::Full => Some("person@example.test"),
                    DashboardIdentityMode::Redacted => Some("redacted@example.test"),
                };
                assert_eq!(
                    first
                        .identity
                        .as_ref()
                        .and_then(|i| i.account_email.as_deref()),
                    expected_email
                );
            }
        }
    }
}

// upstream: DashboardAccountPrivacyTests.swift:120 warm response cache never replays full identities in private mode
#[tokio::test]
async fn warm_response_cache_never_replays_full_identities_in_private_mode() {
    let cache = CliServeResponseCache::new();
    for mode in [
        DashboardIdentityMode::Full,
        DashboardIdentityMode::None,
        DashboardIdentityMode::Redacted,
        DashboardIdentityMode::None,
    ] {
        let key = serve_dashboard_operation_key(mode, false, Some("claude"), true).unwrap();
        let expected = mode.as_str().as_bytes().to_vec();
        let expected_clone = expected.clone();
        let response =
            cached_serve_response(&key, &cache, 60.0, 30.0, "fixture", false, move || {
                let body = expected_clone.clone();
                async move { CliLocalHttpResponse::json(200, "OK", body) }
            })
            .await;
        assert_eq!(response.body, expected);
    }
    assert_eq!(cache.cached_entry_count(), 3);
}

// ============================================================================
// Section 9: DashboardClaudeSwapCollectionTests.swift (2 tests)
// ============================================================================

fn claude_swap_collection_accounts() -> Vec<TokenAccountMeta> {
    (0..2)
        .map(|index| {
            make_token_account_meta(
                format!("00000000-0000-0000-0000-00000000000{}", index + 1),
                format!("Fixture {}", index + 1),
                0.0,
                None,
            )
        })
        .collect()
}

fn claude_swap_collection_config(adapter_enabled: bool) -> Config {
    let accounts = claude_swap_collection_accounts();
    let data = make_token_accounts_meta(1, 1, accounts.clone());
    let mut claude = ProviderConfig::new(pid("claude"));
    claude.enabled = Some(true);
    claude.token_accounts = Some(data);
    claude.extra.insert(
        "claudeSwapEnabled".to_string(),
        serde_json::value::to_raw_value(&adapter_enabled).unwrap(),
    );
    let ibmbob_accounts: Vec<serde_json::Value> = accounts
        .iter()
        .map(|a| {
            serde_json::json!({
                "id": a.id,
                "label": a.label,
                "addedAt": a.added_at,
            })
        })
        .collect();
    let ibmbob_raw = serde_json::json!({
        "id": "ibmbob",
        "enabled": true,
        "tokenAccounts": {
            "version": 1,
            "activeIndex": 1,
            "accounts": ibmbob_accounts,
        },
    });
    Config::new(
        1,
        vec![
            ProviderEntry::Typed(claude),
            ProviderEntry::Opaque {
                id: "ibmbob".to_string(),
                enabled: true,
                raw: serde_json::value::to_raw_value(&ibmbob_raw).unwrap(),
            },
        ],
    )
}

// upstream: DashboardClaudeSwapCollectionTests.swift:7 serving queries only selected Claude credentials when the account adapter is authoritative
#[tokio::test]
async fn serving_queries_only_selected_claude_credentials_when_the_account_adapter_is_authoritative()
 {
    for adapter_enabled in [false, true] {
        for expanded in [false, true] {
            let accounts = claude_swap_collection_accounts();
            let config = claude_swap_collection_config(adapter_enabled);
            let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let operations = CliServeOperationCoordinator::new();
            let mut context = ariadusage_engine::serve::dashboard::ServeUsageContext::new(
                config.clone(),
                format!("fixture-{adapter_enabled}-{expanded}"),
                0.0,
                Some(10.0),
                Some(Instant::now() + std::time::Duration::from_secs(10)),
                operations.clone(),
            );
            context.include_all_codex_accounts = expanded;
            context.include_all_accounts = expanded;
            context.persist_cli_sessions = false;

            let c = Arc::clone(&calls);
            let cfg_clone = config.clone();
            let active_id = accounts[1].id.clone();
            let output = ariadusage_engine::serve::dashboard::serve_usage_output(
                &["claude".to_string()],
                &context,
                move |provider, includes_all_tokens, publish| {
                    let c = Arc::clone(&c);
                    let cfg = cfg_clone.clone();
                    let active_id = active_id.clone();
                    async move {
                        let resolved = ariadusage_engine::serve::dashboard::resolved_token_accounts(
                            &cfg,
                            &provider,
                            includes_all_tokens,
                        );
                        let dash_accounts: Vec<Option<DashboardUsageAccount>> = resolved
                            .iter()
                            .map(|acc| Some(DashboardUsageAccount::token(acc, acc.id == active_id)))
                            .collect();
                        let prov_for_closure = provider.clone();
                        collect_account_usage(
                            &provider,
                            &dash_accounts,
                            false,
                            None,
                            publish.as_ref(),
                            move |idx| {
                                let acc = resolved[idx].clone();
                                let c = Arc::clone(&c);
                                let prov = prov_for_closure.clone();
                                async move {
                                    c.lock().unwrap().push(acc.id.clone());
                                    let usage = usage_snapshot(
                                        Some(rate_window(25.0, Some(300), None, None)),
                                        None,
                                        None,
                                        None,
                                        None,
                                        None,
                                        None,
                                        Timestamp::UNIX_EPOCH,
                                        None,
                                    );
                                    UsageCommandOutput {
                                        sections: vec![],
                                        payload: vec![
                                            ProviderPayload::new(prov, "oauth").with_usage(usage),
                                        ],
                                        exit_code: 0,
                                    }
                                }
                            },
                        )
                        .await
                    }
                },
            )
            .await;

            let expected = if expanded && !adapter_enabled {
                accounts.clone()
            } else {
                vec![accounts[1].clone()]
            };
            let queried = calls.lock().unwrap().clone();
            assert_eq!(
                queried,
                expected.iter().map(|a| a.id.clone()).collect::<Vec<_>>()
            );
            assert_eq!(output.payload.len(), expected.len());
            assert_eq!(
                output
                    .payload
                    .iter()
                    .map(|p| p.dashboard_account.as_ref().map(|a| a.id.clone()))
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|a| Some(DashboardUsageAccount::token(a, a.id == accounts[1].id).id))
                    .collect::<Vec<_>>()
            );
            assert!(output.payload.iter().all(|p| {
                p.error.is_none()
                    && p.usage
                        .as_ref()
                        .and_then(|u| u.primary.as_ref())
                        .map(|w| w.used_percent)
                        == Some(25.0)
            }));
            assert_eq!(
                output
                    .payload
                    .last()
                    .and_then(|p| p.dashboard_account.as_ref())
                    .map(|a| a.active),
                Some(true)
            );
            assert_eq!(operations.snapshot().await.operation_count, 0);
        }
    }
}

// upstream: DashboardClaudeSwapCollectionTests.swift:80 adapter preference does not suppress other providers or disabled adapter configurations
#[test]
fn adapter_preference_does_not_suppress_other_providers_or_disabled_adapter_configurations() {
    let configured = claude_swap_collection_config(true);
    assert!(!serve_includes_configured_accounts(
        "claude",
        &configured,
        true
    ));
    assert!(serve_includes_configured_accounts(
        "ibmbob",
        &configured,
        true
    ));
    let mut disabled = configured.clone();
    if let ProviderEntry::Typed(pc) = &mut disabled.providers[0] {
        pc.enabled = Some(false);
    }
    assert!(serve_includes_configured_accounts(
        "claude", &disabled, true
    ));
}

// ============================================================================
// Section 10: DashboardMultiAccountTests.swift (11 tests)
// ============================================================================

fn multi_account_config() -> Config {
    config_with_providers(vec![("codex", true)])
}

fn multi_account_snapshot(
    payloads: &[ProviderPayload],
    mode: DashboardIdentityMode,
    all_accounts: bool,
) -> DashboardSnapshotPayload {
    DashboardSnapshotBuilder::make_snapshot(
        payloads,
        &[],
        &multi_account_config(),
        mode,
        Timestamp::UNIX_EPOCH,
        60.0,
        None,
        &BTreeMap::new(),
        false,
        all_accounts,
    )
}

fn multi_account_payload(
    provider: &str,
    id: &str,
    active: bool,
    used: f64,
    error: Option<&str>,
) -> ProviderPayload {
    let email = format!("{id}@example.test");
    let mut payload = ProviderPayload::new(provider, "fixture");
    payload.account = Some(email.clone());
    payload.cache_account_key = Some(format!("private:{email}"));
    if let Some(err) = error {
        payload.error = Some(ProviderErrorPayload::provider(err));
    } else {
        payload.usage = Some(usage_snapshot(
            Some(rate_window(used, Some(300), None, None)),
            None,
            None,
            None,
            None,
            None,
            None,
            Timestamp::UNIX_EPOCH,
            Some(ProviderIdentitySnapshot::new(
                ProviderId::new(provider).ok(),
                Some(email.clone()),
                None,
                Some("pro".to_string()),
                None,
            )),
        ));
    }
    payload.dashboard_account = Some(DashboardUsageAccount::new(id, email, active));
    payload
}

fn multi_account_profile(path: &str, email: &str, fingerprint: &str) -> CodexVisibleAccount {
    CodexVisibleAccount {
        id: format!("private-{email}"),
        email: email.to_string(),
        workspace_label: None,
        workspace_account_id: Some("same-workspace".to_string()),
        auth_fingerprint: Some(fingerprint.repeat(64)),
        stored_account_id: None,
        selection_source: CodexActiveSource::ProfileHome {
            path: path.to_string(),
        },
        is_active: false,
        is_live: false,
        can_reauthenticate: false,
        can_remove: false,
    }
}

fn multi_account_token(id: Uuid, label: &str, _token: &str) -> TokenAccountMeta {
    make_token_account_meta(id.hyphenated().to_string(), label, 0.0, None)
}

// upstream: DashboardMultiAccountTests.swift:7 two profiles group and non first active drives compatibility fields and cache key
#[tokio::test]
async fn two_profiles_group_and_non_first_active_drives_compatibility_fields_and_cache_key() {
    let first = multi_account_payload("codex", "one", false, 10.0, None);
    let second = multi_account_payload("codex", "two", true, 65.0, None);
    let mut producer = DashboardSnapshotProducer::new(
        Arc::new(move |_| {
            let f = first.clone();
            let s = second.clone();
            Box::pin(async move {
                Ok(UsageCommandOutput {
                    sections: vec![],
                    payload: vec![f, s],
                    exit_code: 0,
                })
            })
        }),
        Arc::new(|_, _| Box::pin(async { vec![] })),
        Arc::new(|| Timestamp::UNIX_EPOCH),
    );
    producer.usage_bars_show_used = Arc::new(|| true);
    producer.all_accounts = true;

    let result = producer
        .collect(
            &multi_account_config(),
            60.0,
            None,
            Some(DashboardIdentityMode::Full),
            None,
        )
        .await
        .unwrap();
    let row = &result.payload.providers[0];
    let accounts = row.accounts.as_ref().unwrap();
    assert_eq!(result.payload.providers.len(), 1);
    assert_eq!(
        accounts.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        vec!["one", "two"]
    );
    assert_eq!(
        accounts.iter().map(|a| a.active).collect::<Vec<_>>(),
        vec![false, true]
    );
    assert_eq!(row.windows[0].used_percent, 65.0);
    assert_eq!(
        row.identity
            .as_ref()
            .and_then(|i| i.account_email.as_deref()),
        Some("two@example.test")
    );
    assert_eq!(
        result.usage_cache_keys,
        vec![Some("private:two@example.test".to_string())]
    );
    assert!(result.payload.host.usage_bars_show_used);
}

// upstream: DashboardMultiAccountTests.swift:30 expired sibling stays local and redaction covers labels identities and errors
#[test]
fn expired_sibling_stays_local_and_redaction_covers_labels_identities_and_errors() {
    let failed =
        multi_account_payload("codex", "one", false, 0.0, Some("Expired one@example.test"));
    let healthy = multi_account_payload("codex", "two", true, 65.0, None);
    let snapshot =
        multi_account_snapshot(&[failed, healthy], DashboardIdentityMode::Redacted, true);
    let row = &snapshot.providers[0];
    let accounts = row.accounts.as_ref().unwrap();
    assert!(row.error.is_none());
    assert_eq!(row.windows[0].used_percent, 65.0);
    assert!(accounts[0].windows.is_empty());
    assert_eq!(
        accounts[0].error.as_deref(),
        Some("Account usage unavailable")
    );
    assert_eq!(accounts[1].windows[0].used_percent, 65.0);
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(!json.contains("one@example.test"));
    assert!(!json.contains("two@example.test"));
    assert!(!json.contains("private:"));
    assert!(!json.contains("cacheAccountKey"));
    assert!(!json.contains("dashboardAccount"));
}

// upstream: DashboardMultiAccountTests.swift:50 failed active account is not replaced by healthy sibling and its error is redacted
#[test]
fn failed_active_account_is_not_replaced_by_healthy_sibling_and_its_error_is_redacted() {
    let snapshot = multi_account_snapshot(
        &[
            multi_account_payload("codex", "one", false, 10.0, None),
            multi_account_payload("codex", "two", true, 0.0, Some("Expired two@example.test")),
        ],
        DashboardIdentityMode::Redacted,
        true,
    );
    let row = &snapshot.providers[0];
    assert!(row.windows.is_empty());
    assert_eq!(
        row.error.as_ref().map(|e| e.message.as_str()),
        Some("Account usage unavailable")
    );
    assert_eq!(
        row.accounts.as_ref().unwrap()[0].windows[0].used_percent,
        10.0
    );

    let none = multi_account_snapshot(
        &[multi_account_payload(
            "codex",
            "two",
            true,
            0.0,
            Some("two@example.test"),
        )],
        DashboardIdentityMode::None,
        true,
    );
    assert_eq!(
        none.providers[0].accounts.as_ref().unwrap()[0].label,
        "Account 1"
    );
    assert_eq!(
        none.providers[0].accounts.as_ref().unwrap()[0].identity,
        None
    );
    assert_eq!(
        none.providers[0].error.as_ref().map(|e| e.message.as_str()),
        Some("Account usage unavailable")
    );
}

// upstream: DashboardMultiAccountTests.swift:66 default dashboard shape and raw usage encoding do not expose new metadata
#[test]
fn default_dashboard_shape_and_raw_usage_encoding_do_not_expose_new_metadata() {
    let payload = multi_account_payload("codex", "one", true, 10.0, None);
    let snapshot = multi_account_snapshot(
        std::slice::from_ref(&payload),
        DashboardIdentityMode::Full,
        false,
    );
    assert_eq!(snapshot.providers.len(), 1);
    assert_eq!(snapshot.providers[0].accounts, None);
    let json = serde_json::to_string(&payload).unwrap();
    assert!(!json.contains("dashboardAccount"));
    assert!(!json.contains("cacheAccountKey"));
    assert!(!json.contains("private:"));

    let context = ariadusage_engine::serve::dashboard::ServeUsageContext::new(
        multi_account_config(),
        "fixture",
        60.0,
        None,
        None,
        CliServeOperationCoordinator::new(),
    );
    assert!(context.include_all_codex_accounts);
    assert!(!context.include_all_accounts);
}

// upstream: DashboardMultiAccountTests.swift:87 profile IDs survive email changes and token refreshes but separate profile homes
#[test]
fn profile_ids_survive_email_changes_and_token_refreshes_but_separate_profile_homes() {
    let one = multi_account_profile("/fixture/one", "one@example.test", "a");
    let refreshed = multi_account_profile("/fixture/one", "renamed@example.test", "b");
    let two = multi_account_profile("/fixture/two", "one@example.test", "a");
    let first_id = DashboardUsageAccount::codex_visible(&one).id;
    assert_eq!(
        first_id,
        DashboardUsageAccount::codex_visible(&refreshed).id
    );
    assert_ne!(first_id, DashboardUsageAccount::codex_visible(&two).id);
    assert!(!first_id.contains('@'));
    assert!(!first_id.contains("/fixture"));
    assert_eq!(first_id.len(), "codex:".len() + 64);

    let token_id = Uuid::new_v4();
    let initial = multi_account_token(token_id, "before@example.test", "old");
    let renewed = multi_account_token(token_id, "after@example.test", "new");
    assert_eq!(
        DashboardUsageAccount::token(&initial, false).id,
        DashboardUsageAccount::token(&renewed, true).id
    );
}

// upstream: DashboardMultiAccountTests.swift:105 configured accounts select non first active and missing accounts use ambient fallback
#[test]
fn configured_accounts_select_non_first_active_and_missing_accounts_use_ambient_fallback() {
    let accounts = vec![
        multi_account_token(Uuid::new_v4(), "Fixture", "synthetic"),
        multi_account_token(Uuid::new_v4(), "Fixture", "synthetic"),
    ];
    let mut claude = ProviderConfig::new(pid("claude"));
    claude.enabled = Some(true);
    claude.token_accounts = Some(make_token_accounts_meta(1, 1, accounts.clone()));
    let config = Config::new(1, vec![ProviderEntry::Typed(claude)]);

    let selected =
        ariadusage_engine::serve::dashboard::resolved_token_accounts(&config, "claude", false);
    let all = ariadusage_engine::serve::dashboard::resolved_token_accounts(&config, "claude", true);
    assert_eq!(
        selected.iter().map(|a| a.id.clone()).collect::<Vec<_>>(),
        vec![accounts[1].id.clone()]
    );
    assert_eq!(
        all.iter().map(|a| a.id.clone()).collect::<Vec<_>>(),
        accounts.iter().map(|a| a.id.clone()).collect::<Vec<_>>()
    );
    assert!(serve_includes_configured_accounts("claude", &config, true));
    assert!(!serve_includes_configured_accounts(
        "claude", &config, false
    ));
    assert!(!serve_includes_configured_accounts(
        "claude",
        &multi_account_config(),
        true
    ));
    assert!(!serve_includes_configured_accounts(
        "codex",
        &multi_account_config(),
        true
    ));
}

// upstream: DashboardMultiAccountTests.swift:130 default all and usage scopes cannot share provider operation fingerprints
#[test]
fn default_all_and_usage_scopes_cannot_share_provider_operation_fingerprints() {
    let selected = serve_usage_operation_fingerprint("fixture", false, false);
    let usage = serve_usage_operation_fingerprint("fixture", true, false);
    let all = serve_usage_operation_fingerprint("fixture", true, true);
    let set: BTreeSet<String> = [selected, usage, all].into_iter().collect();
    assert_eq!(set.len(), 3);
}

// upstream: DashboardMultiAccountTests.swift:141 warm response cache keeps account scopes separate
#[tokio::test]
async fn warm_response_cache_keeps_account_scopes_separate() {
    let cache = CliServeResponseCache::new();
    for all_accounts in [false, true, false, true] {
        let key = serve_dashboard_operation_key(
            DashboardIdentityMode::Redacted,
            false,
            Some("codex"),
            all_accounts,
        )
        .unwrap();
        let expected = if all_accounts { "all" } else { "selected" }
            .as_bytes()
            .to_vec();
        let expected_clone = expected.clone();
        let response =
            cached_serve_response(&key, &cache, 60.0, 30.0, "fixture", false, move || {
                let body = expected_clone.clone();
                async move { CliLocalHttpResponse::json(200, "OK", body) }
            })
            .await;
        assert_eq!(response.body, expected);
    }
    assert_eq!(cache.cached_entry_count(), 2);
}

// upstream: DashboardMultiAccountTests.swift:158 claude swap remains the authoritative account source
#[test]
fn claude_swap_remains_the_authoritative_account_source() {
    for all_accounts in [false, true] {
        let adapter = ClaudeSwapAccountProjection::account_snapshots(
            &ClaudeSwapAccountList {
                active_account_number: Some(7),
                accounts: vec![ClaudeSwapAccountRow {
                    number: 7,
                    email: "swap@example.test".to_string(),
                    organization_name: String::new(),
                    alias: None,
                    is_active: true,
                    usage_status: ClaudeSwapUsageStatus::Ok,
                    five_hour: None,
                    seven_day: None,
                    scoped: vec![],
                }],
            },
            Timestamp::UNIX_EPOCH,
        );
        let mut collections = BTreeMap::new();
        collections.insert(
            "claude".to_string(),
            DashboardAccountsInput {
                accounts: Some(adapter),
                adapter_error: None,
                weekly_work_days: None,
            },
        );
        let snapshot = DashboardSnapshotBuilder::make_snapshot(
            &[multi_account_payload("claude", "token", true, 0.0, None)],
            &[],
            &multi_account_config(),
            DashboardIdentityMode::Full,
            Timestamp::UNIX_EPOCH,
            60.0,
            None,
            &collections,
            false,
            all_accounts,
        );
        let account = &snapshot.providers[0].accounts.as_ref().unwrap()[0];
        assert_ne!(account.id, "token");
        assert_eq!(
            account
                .identity
                .as_ref()
                .and_then(|i| i.account_email.as_deref()),
            Some("swap@example.test")
        );
    }
}

// upstream: DashboardMultiAccountTests.swift:185 managed account public ID survives promotion to live system
#[test]
fn managed_account_public_id_survives_promotion_to_live_system() {
    let stored_id = Uuid::new_v4();
    let account = |source: CodexActiveSource, sid: Option<Uuid>| CodexVisibleAccount {
        id: "private".to_string(),
        email: "fixture@example.test".to_string(),
        workspace_label: None,
        workspace_account_id: Some("workspace".to_string()),
        auth_fingerprint: None,
        stored_account_id: sid,
        selection_source: source.clone(),
        is_active: true,
        is_live: source == CodexActiveSource::LiveSystem,
        can_reauthenticate: true,
        can_remove: true,
    };
    let managed = account(
        CodexActiveSource::ManagedAccount { id: stored_id },
        Some(stored_id),
    );
    let live = account(CodexActiveSource::LiveSystem, Some(stored_id));
    assert_eq!(
        DashboardUsageAccount::codex_visible(&managed).id,
        DashboardUsageAccount::codex_visible(&live).id
    );
    assert_eq!(
        DashboardUsageAccount::codex_visible(&managed).id,
        format!(
            "codex-managed:{}",
            stored_id.hyphenated().to_string().to_ascii_lowercase()
        )
    );
    let profile = account(
        CodexActiveSource::ProfileHome {
            path: "/fixture/separate".to_string(),
        },
        Some(stored_id),
    );
    assert_ne!(
        DashboardUsageAccount::codex_visible(&profile).id,
        DashboardUsageAccount::codex_visible(&live).id
    );
    let unmanaged = account(CodexActiveSource::LiveSystem, None);
    assert_ne!(
        DashboardUsageAccount::codex_visible(&unmanaged).id,
        DashboardUsageAccount::codex_visible(&live).id
    );
}

struct DashboardAccountTestGate {
    is_open: AtomicBool,
    notify: Notify,
}

impl DashboardAccountTestGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            is_open: AtomicBool::new(false),
            notify: Notify::new(),
        })
    }

    async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            if self.is_open.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
            if self.is_open.load(Ordering::SeqCst) {
                return;
            }
        }
    }

    fn open(&self) {
        self.is_open.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

// upstream: DashboardMultiAccountTests.swift:210 account collector preserves finished siblings at deadline for every joined waiter
#[tokio::test]
async fn account_collector_preserves_finished_siblings_at_deadline_for_every_joined_waiter() {
    for expanded in [false, true] {
        let timer = DashboardAccountTestGate::new();
        let slow_started = DashboardAccountTestGate::new();
        let slow_release = DashboardAccountTestGate::new();
        let now = Instant::now();
        let timer_clone = Arc::clone(&timer);
        let operations = CliServeOperationCoordinator::with_clock(
            Arc::new(move || now),
            Arc::new(move |_| {
                let t = Arc::clone(&timer_clone);
                Box::pin(async move {
                    t.wait().await;
                    Ok(())
                })
            }),
        );
        let first = multi_account_payload("codex", "one", false, 10.0, None);
        let second = multi_account_payload("codex", "two", true, 65.0, None);
        let accounts = vec![
            first.dashboard_account.clone(),
            second.dashboard_account.clone(),
        ];
        let deadline = now + std::time::Duration::from_secs(30);

        let ops_leader = operations.clone();
        let ss = Arc::clone(&slow_started);
        let sr = Arc::clone(&slow_release);
        let f_clone = first.clone();
        let s_clone = second.clone();
        let acc_clone = accounts.clone();
        let leader = tokio::spawn(async move {
            serve_collect_usage_outputs(
                &["codex".to_string()],
                "fixture",
                Some(deadline),
                &ops_leader,
                move |provider, publish| {
                    let ss = Arc::clone(&ss);
                    let sr = Arc::clone(&sr);
                    let f = f_clone.clone();
                    let s = s_clone.clone();
                    let accs = acc_clone.clone();
                    async move {
                        let sink = expanded.then_some(&publish);
                        collect_account_usage(&provider, &accs, false, None, sink, move |index| {
                            let ss = Arc::clone(&ss);
                            let sr = Arc::clone(&sr);
                            let f = f.clone();
                            let s = s.clone();
                            async move {
                                if index == 1 {
                                    ss.open();
                                    sr.wait().await;
                                }
                                UsageCommandOutput {
                                    sections: vec![],
                                    payload: vec![if index == 0 { f } else { s }],
                                    exit_code: 0,
                                }
                            }
                        })
                        .await
                    }
                },
            )
            .await
        });

        slow_started.wait().await;
        let ops_follower = operations.clone();
        let follower = tokio::spawn(async move {
            serve_collect_usage_outputs(
                &["codex".to_string()],
                "fixture",
                Some(deadline),
                &ops_follower,
                |_, _| async {
                    panic!("A joined request must not start another account fetch");
                },
            )
            .await
        });

        for _ in 0..10_000 {
            if operations.snapshot().await.waiter_count == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(operations.snapshot().await.waiter_count, 2);

        timer.open();
        let outputs = vec![leader.await.unwrap(), follower.await.unwrap()];
        assert_eq!(operations.snapshot().await.operation_count, 1);

        let changed_config = serve_collect_usage_outputs(
            &["codex".to_string()],
            "different-account-configuration",
            Some(deadline + std::time::Duration::from_secs(30)),
            &operations,
            |_, _| async {
                panic!("A new configuration must not overlap the retained source");
            },
        )
        .await;
        assert_eq!(changed_config.payload.len(), 1);
        assert_eq!(changed_config.payload[0].dashboard_account, None);
        assert_eq!(changed_config.payload[0].usage, None);

        slow_release.open();
        for _ in 0..10_000 {
            if operations.snapshot().await.operation_count == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(operations.snapshot().await.operation_count, 0);

        for output in outputs {
            if expanded {
                assert_eq!(output.payload.len(), 2);
                assert_eq!(
                    output.payload[0]
                        .usage
                        .as_ref()
                        .and_then(|u| u.primary.as_ref())
                        .map(|w| w.used_percent),
                    Some(10.0)
                );
                assert_eq!(
                    output.payload[0].cache_account_key.as_deref(),
                    Some("private:one@example.test")
                );
                assert_eq!(
                    output.payload[1]
                        .dashboard_account
                        .as_ref()
                        .map(|a| a.id.as_str()),
                    Some("two")
                );
                assert!(output.payload[1].error.is_some());
                assert_eq!(output.payload[1].cache_account_key, None);
                let snapshot =
                    multi_account_snapshot(&output.payload, DashboardIdentityMode::Redacted, true);
                let row = &snapshot.providers[0];
                assert_eq!(row.accounts.as_ref().unwrap().len(), 2);
                assert_eq!(
                    row.accounts.as_ref().unwrap()[0].windows[0].used_percent,
                    10.0
                );
                assert!(row.accounts.as_ref().unwrap()[1].active);
                assert!(row.error.is_some());
            } else {
                assert_eq!(output.payload.len(), 1);
                assert_eq!(output.payload[0].usage, None);
                assert_eq!(output.payload[0].dashboard_account, None);
                assert!(output.payload[0].error.is_some());
            }
        }
    }
}

// ============================================================================
// Section 11: DashboardServeScopeTests.swift (1 test)
// ============================================================================

// upstream: DashboardServeScopeTests.swift:6 live account expansion is an explicit serve only option
#[test]
fn live_account_expansion_is_an_explicit_serve_only_option() {
    let ordinary =
        ariadusage_engine::serve::resolve_serve_or_dashboard_all_accounts_flag(&["serve"]).unwrap();
    let expanded = ariadusage_engine::serve::resolve_serve_or_dashboard_all_accounts_flag(&[
        "serve",
        "--all-accounts",
    ])
    .unwrap();
    assert!(!ordinary);
    assert!(expanded);
    assert!(
        ariadusage_engine::serve::resolve_serve_or_dashboard_all_accounts_flag(&[
            "dashboard",
            "--all-accounts",
        ])
        .is_err()
    );
}

#[test]
fn write_dashboard_snapshot_atomically_replaces_existing_file_and_cleans_staging() {
    let dir = tempfile::tempdir().unwrap();
    let missing_parent = dir.path().join("nested/snapshot.json");
    let err = write_dashboard_snapshot_atomically(b"{}", &missing_parent).unwrap_err();
    assert!(err.contains("parent directories are not created"));

    let target = dir.path().join("snapshot.json");
    write_dashboard_snapshot_atomically(br#"{"schemaVersion":1,"first":true}"#, &target).unwrap();
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        r#"{"schemaVersion":1,"first":true}"#
    );

    write_dashboard_snapshot_atomically(br#"{"schemaVersion":1,"second":true}"#, &target).unwrap();
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        r#"{"schemaVersion":1,"second":true}"#
    );

    let entries: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec!["snapshot.json".to_string()]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);
    }
}
