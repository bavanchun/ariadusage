// Ported from CodexBar Sources/CodexBarCore/CostUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

pub mod claude;
pub mod codex;
pub mod codex_priority;
pub mod ledger;
pub mod pricing;
pub mod store;
pub mod timestamp;

use std::path::PathBuf;

use ariadusage_protocol::usage::{CostHistory, CostProvenance};
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};

use crate::cost::claude::{ClaudeLogProviderFilter, ClaudeScanOptions};
use crate::cost::codex::CodexScanOptions;
use crate::cost::ledger::CostUsageDailyReport;
use crate::cost::pricing::ModelsDevCatalog;
use crate::cost::store::CostUsageStore;
use crate::cost::timestamp::DayRange;

/// Supported local cost providers in P6 part 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalCostProvider {
    Claude,
    VertexAi,
    Codex,
}

impl LocalCostProvider {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::VertexAi => "vertexai",
            Self::Codex => "codex",
        }
    }
}

/// Unified options for local cost scanning across Claude, Vertex AI, and Codex.
#[derive(Debug, Clone, Default)]
pub struct CostScannerOptions {
    pub claude_projects_roots: Vec<PathBuf>,
    pub claude_provider_filter: Option<ClaudeLogProviderFilter>,
    pub codex_home: Option<PathBuf>,
    pub codex_sessions_root: Option<PathBuf>,
    pub codex_priority_db_path: Option<PathBuf>,
    pub cache_root: Option<PathBuf>,
    pub temp_dir: Option<PathBuf>,
    pub max_codex_sqlite_bytes: Option<u64>,
    pub force_rescan: bool,
    pub timezone: Option<TimeZone>,
    pub catalog: Option<ModelsDevCatalog>,
}

/// Loads a `CostUsageDailyReport` for `provider` over `[since..=until]`, optionally persisting
/// results into `CostUsageStore` when `options.cache_root` is configured.
#[must_use]
pub fn load_daily_report(
    provider: LocalCostProvider,
    since: Timestamp,
    until: Timestamp,
    now: Timestamp,
    options: &CostScannerOptions,
) -> CostUsageDailyReport {
    let range = DayRange::new(since, until, options.timezone.clone());
    let now_ms = now.as_millisecond();

    match provider {
        LocalCostProvider::Claude | LocalCostProvider::VertexAi => {
            let filter = options.claude_provider_filter.unwrap_or(match provider {
                LocalCostProvider::Claude => ClaudeLogProviderFilter::All,
                LocalCostProvider::VertexAi => ClaudeLogProviderFilter::VertexAiOnly,
                LocalCostProvider::Codex => unreachable!(),
            });
            let claude_opts = ClaudeScanOptions {
                claude_projects_roots: options.claude_projects_roots.clone(),
                provider_filter: filter,
                timezone: options.timezone.clone(),
                catalog: options.catalog.clone(),
            };
            let rows = claude::scan_claude_projects(&range, &claude_opts);
            let report = ledger::build_claude_daily_report(&rows, &range);
            if let Some(ref cache_root) = options.cache_root {
                let mut store = CostUsageStore::new(Some(cache_root));
                let _ = store.save_provider_report(provider.as_str(), &report, now_ms);
            }
            report
        }
        LocalCostProvider::Codex => {
            let persisted_cursor = if options.force_rescan {
                None
            } else if let Some(ref cache_root) = options.cache_root {
                let mut store = CostUsageStore::new(Some(cache_root));
                store.load_metadata().and_then(|m| m.priority_turns_cursor)
            } else {
                None
            };
            let codex_opts = CodexScanOptions {
                codex_home: options.codex_home.clone(),
                codex_sessions_root: options.codex_sessions_root.clone(),
                priority_db_path: options.codex_priority_db_path.clone(),
                temp_dir: options.temp_dir.clone().or_else(|| {
                    options
                        .cache_root
                        .as_deref()
                        .map(|root| store::default_sqlite_copy_parent(Some(root)))
                }),
                max_sqlite_bytes: options.max_codex_sqlite_bytes,
                priority_deadline: None,
                persisted_priority_cursor: persisted_cursor,
                force_rescan: options.force_rescan,
                timezone: options.timezone.clone(),
                catalog: options.catalog.clone(),
            };
            let scan = codex::scan_codex_home(&range, &codex_opts);
            if let Some(ref cache_root) = options.cache_root {
                let mut store = CostUsageStore::new(Some(cache_root));
                store.save_codex_scan(&scan, &range, now_ms)
            } else {
                ledger::build_codex_daily_report(&scan, &range)
            }
        }
    }
}

/// Loads a Snapshot/IPC v1 `CostHistory` for `provider` over a rolling `history_days` window ending at `now`.
#[must_use]
pub fn load_cost_history(
    provider: LocalCostProvider,
    now: Timestamp,
    history_days: u32,
    options: &CostScannerOptions,
) -> CostHistory {
    let clamped_days = history_days.max(1);
    let since = now
        .checked_sub(Span::new().hours(i64::from(clamped_days.saturating_sub(1)) * 24))
        .unwrap_or(now);
    let mut effective_opts = options.clone();
    if provider == LocalCostProvider::Claude && effective_opts.claude_provider_filter.is_none() {
        effective_opts.claude_provider_filter = Some(ClaudeLogProviderFilter::ExcludeVertexAi);
    }
    let report = load_daily_report(provider, since, now, now, &effective_opts);
    ledger::build_cost_history(
        &report,
        now,
        clamped_days,
        options.timezone.clone(),
        true,
        CostProvenance::ListPriceEstimate,
    )
}
