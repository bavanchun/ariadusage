// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityLocalScan.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityProviderConfig.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::local_reader::AntigravityLocalReader;
use super::offline_store::{AntigravityOfflineStore, standardize_lexical_path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub home: PathBuf,
    pub additional_profile_homes: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub copy_parent: Option<PathBuf>,
}

impl Context {
    #[must_use]
    pub fn new(
        environment: BTreeMap<String, String>,
        additional_profile_homes: Vec<String>,
    ) -> Self {
        let home = environment
            .get("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let copy_parent = default_sqlite_copy_parent(&environment, &home);
        Self {
            home,
            additional_profile_homes,
            environment,
            copy_parent,
        }
    }

    #[must_use]
    pub fn with_copy_parent(mut self, copy_parent: impl Into<PathBuf>) -> Self {
        self.copy_parent = Some(copy_parent.into());
        self
    }

    #[must_use]
    pub fn with_provider_config(
        mut self,
        provider_config: &ariadusage_core::config::ProviderConfig,
    ) -> Self {
        self.additional_profile_homes = provider_config
            .antigravity_additional_profile_homes()
            .unwrap_or_default();
        self
    }

    #[must_use]
    pub fn database_roots(&self) -> Vec<PathBuf> {
        AntigravityOfflineStore::gemini_home_directories(
            &self.home,
            &self.environment,
            &self.additional_profile_homes,
        )
        .into_iter()
        .flat_map(|home| {
            [
                "antigravity-cli/conversations",
                "antigravity",
                "antigravity/conversations",
            ]
            .into_iter()
            .map(move |subpath| home.join(subpath))
        })
        .collect()
    }

    #[must_use]
    pub fn cache_root(&self) -> PathBuf {
        let config = self
            .environment
            .get("TOKSCALE_CONFIG_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.home.join(".config/tokscale"));
        config.join("antigravity-cache/sessions")
    }
}

fn default_sqlite_copy_parent(
    environment: &BTreeMap<String, String>,
    home: &Path,
) -> Option<PathBuf> {
    let valid_home = (home.is_absolute() && home != Path::new("/")).then_some(home);
    let cache_root = crate::cost::store::resolve_cache_root(environment, valid_home);
    (!cache_root.as_os_str().is_empty())
        .then(|| crate::cost::store::default_sqlite_copy_parent(Some(&cache_root)))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub databases: usize,
    pub directory_entries: usize,
    pub rows_per_database: usize,
    pub rows: usize,
    pub blob_bytes: usize,
    pub database_bytes: usize,
    pub bytes: usize,
    pub schema_entries: usize,
    pub schema_columns: usize,
    pub schema_bytes: usize,
    pub duration: f64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            databases: 500,
            directory_entries: 10_000,
            rows_per_database: 10_000,
            rows: 100_000,
            blob_bytes: 16 * 1024 * 1024,
            database_bytes: 64 * 1024 * 1024,
            bytes: 512 * 1024 * 1024,
            schema_entries: 128,
            schema_columns: 64,
            schema_bytes: 64 * 1024,
            duration: 30.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Statistics {
    pub directory_entries: usize,
    pub files: usize,
    pub rows: usize,
    pub attempted_bytes: usize,
    pub materialized_payload_bytes: usize,
    pub schema_entries: usize,
    pub schema_columns: usize,
    pub schema_bytes: usize,
    pub sqlite_handles_opened: usize,
    pub sqlite_handles_closed: usize,
    /// Sidecar-less WAL databases that the ordinary read-only open declined and an immutable open read.
    pub immutable_fallbacks: usize,
    /// Databases in a declared root whose schema has no `gen_metadata` table; skipped without affecting coverage.
    pub foreign_databases: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanFailure {
    Exhausted,
    /// Schema-budget exhaustion (bytes, entries, or columns). Unlike hard row/byte/duration exhaustion,
    /// schema exhaustion preserves already-decoded rows as partial history instead of withholding the report.
    SchemaExhausted,
    Invalid,
    Cancelled(String),
}

impl std::fmt::Display for ScanFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exhausted => write!(f, "Antigravity local scan budget exhausted"),
            Self::SchemaExhausted => write!(f, "Antigravity local schema budget exhausted"),
            Self::Invalid => write!(f, "Antigravity local record is invalid"),
            Self::Cancelled(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for ScanFailure {}

#[derive(Debug, Default)]
struct BudgetState {
    statistics: Statistics,
    database_schema_bytes: usize,
}

type ClockFn = dyn Fn() -> f64 + Send + Sync;
type CancellationFn = dyn Fn() -> Result<(), ScanFailure> + Send + Sync;

/// One budget belongs to one executor job, including discovery and fallback.
#[derive(Clone)]
pub struct Budget {
    limits: Limits,
    cancellation: Arc<CancellationFn>,
    clock: Arc<ClockFn>,
    started: f64,
    state: Arc<Mutex<BudgetState>>,
    copy_parent: Option<PathBuf>,
}

impl std::fmt::Debug for Budget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Budget")
            .field("limits", &self.limits)
            .field("started", &self.started)
            .field("statistics", &self.statistics())
            .field("database_schema_bytes", &self.database_schema_bytes())
            .field("copy_parent", &self.copy_parent)
            .finish()
    }
}

impl Budget {
    #[must_use]
    pub fn new<C, K>(limits: Limits, clock: C, cancellation: K) -> Self
    where
        C: Fn() -> f64 + Send + Sync + 'static,
        K: Fn() -> Result<(), ScanFailure> + Send + Sync + 'static,
    {
        let started = clock();
        Self {
            limits,
            cancellation: Arc::new(cancellation),
            clock: Arc::new(clock),
            started,
            state: Arc::new(Mutex::new(BudgetState::default())),
            copy_parent: None,
        }
    }

    #[must_use]
    pub fn with_cancellation<K>(limits: Limits, cancellation: K) -> Self
    where
        K: Fn() -> Result<(), ScanFailure> + Send + Sync + 'static,
    {
        let epoch = Instant::now();
        Self::new(limits, move || epoch.elapsed().as_secs_f64(), cancellation)
    }

    #[must_use]
    pub fn with_copy_parent(mut self, copy_parent: Option<PathBuf>) -> Self {
        self.copy_parent = copy_parent;
        self
    }

    #[must_use]
    pub fn copy_parent(&self) -> Option<&Path> {
        self.copy_parent.as_deref()
    }

    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    #[must_use]
    pub fn statistics(&self) -> Statistics {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .statistics
    }

    pub fn update_statistics<R>(&self, f: impl FnOnce(&mut Statistics) -> R) -> R {
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut guard.statistics)
    }

    #[must_use]
    pub fn database_schema_bytes(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .database_schema_bytes
    }

    pub fn check(&self) -> Result<(), ScanFailure> {
        (self.cancellation)()?;
        if (self.clock)() - self.started < self.limits.duration {
            Ok(())
        } else {
            Err(ScanFailure::Exhausted)
        }
    }

    pub fn charge_bytes(&self, count: usize) -> Result<(), ScanFailure> {
        self.check()?;
        let (attempted, overflow) = {
            let mut guard = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (attempted, overflow) = guard.statistics.attempted_bytes.overflowing_add(count);
            guard.statistics.attempted_bytes = if overflow { usize::MAX } else { attempted };
            (attempted, overflow)
        };
        if !overflow && attempted <= self.limits.bytes {
            Ok(())
        } else {
            Err(ScanFailure::Exhausted)
        }
    }

    pub fn charge_row(&self) -> Result<(), ScanFailure> {
        self.check()?;
        let rows = {
            let mut guard = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.statistics.rows = guard.statistics.rows.saturating_add(1);
            guard.statistics.rows
        };
        if rows <= self.limits.rows {
            Ok(())
        } else {
            Err(ScanFailure::Exhausted)
        }
    }

    pub fn begin_database(&self) {
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.database_schema_bytes = 0;
    }

    pub fn charge_schema_bytes(&self, count: usize) -> Result<(), ScanFailure> {
        self.check()?;
        let (attempted, overflow) = {
            let mut guard = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (total, total_overflow) = guard.statistics.schema_bytes.overflowing_add(count);
            guard.statistics.schema_bytes = if total_overflow { usize::MAX } else { total };
            let (attempted, overflow) = guard.database_schema_bytes.overflowing_add(count);
            guard.database_schema_bytes = if overflow { usize::MAX } else { attempted };
            (attempted, overflow)
        };
        if !overflow && attempted <= self.limits.schema_bytes {
            Ok(())
        } else {
            Err(ScanFailure::SchemaExhausted)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub paths: Vec<PathBuf>,
    pub is_complete: bool,
}

impl Default for Discovery {
    fn default() -> Self {
        Self {
            paths: Vec::new(),
            is_complete: true,
        }
    }
}

impl AntigravityLocalReader {
    pub fn discover(
        roots: &[PathBuf],
        suffix: &str,
        budget: &Budget,
    ) -> Result<Discovery, ScanFailure> {
        let mut result = Discovery::default();
        for root in roots {
            budget.check()?;
            let resolved_root = match std::fs::metadata(root) {
                Ok(meta) => {
                    if !meta.is_dir() {
                        result.is_complete = false;
                        continue;
                    }
                    let Ok(canon) = std::fs::canonicalize(root) else {
                        result.is_complete = false;
                        continue;
                    };
                    canon
                }
                Err(err) => {
                    // ENOENT means absent. Permission errors and invalid roots must block fallback.
                    if err.kind() == std::io::ErrorKind::NotFound
                        && std::fs::symlink_metadata(root).is_err()
                    {
                        continue;
                    }
                    result.is_complete = false;
                    continue;
                }
            };
            let Ok(mut entries_iter) = std::fs::read_dir(&resolved_root) else {
                result.is_complete = false;
                continue;
            };
            let mut enumeration_failed = false;
            loop {
                budget.check()?;
                let next_entry = match entries_iter.next() {
                    Some(Ok(entry)) => entry,
                    Some(Err(_)) => {
                        enumeration_failed = true;
                        break;
                    }
                    None => break,
                };
                let dir_entries = budget.update_statistics(|s| {
                    s.directory_entries += 1;
                    s.directory_entries
                });
                if dir_entries > budget.limits().directory_entries {
                    return Err(ScanFailure::Exhausted);
                }
                let url = next_entry.path();
                let Some(file_name) = url.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if file_name.starts_with('.') {
                    continue;
                }
                let ext = url
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(str::to_ascii_lowercase);
                if ext.as_deref() != Some(suffix) {
                    continue;
                }
                if result.paths.len() >= budget.limits().databases {
                    return Err(ScanFailure::Exhausted);
                }
                match std::fs::symlink_metadata(&url) {
                    Ok(meta) if meta.file_type().is_file() => match std::fs::canonicalize(&url) {
                        Ok(canonical_entry)
                            if canonical_entry.starts_with(&resolved_root)
                                && canonical_entry.parent() == Some(resolved_root.as_path()) =>
                        {
                            result.paths.push(url);
                        }
                        _ => {
                            result.is_complete = false;
                        }
                    },
                    _ => {
                        result.is_complete = false;
                    }
                }
            }
            if enumeration_failed {
                result.is_complete = false;
            }
        }
        result.paths.sort();
        Ok(result)
    }

    #[must_use]
    pub fn antigravity_history_scope(
        environment: &BTreeMap<String, String>,
        additional_profile_homes: &[String],
    ) -> String {
        let context = Context::new(environment.clone(), additional_profile_homes.to_vec());
        let mut roots: BTreeSet<String> = BTreeSet::new();
        for root in context
            .database_roots()
            .into_iter()
            .chain(std::iter::once(context.cache_root()))
        {
            let resolved = resolve_path_preserving_existing_prefix(&root);
            roots.insert(resolved.to_string_lossy().into_owned());
        }
        roots.into_iter().collect::<Vec<_>>().join("\0")
    }
}

fn resolve_path_preserving_existing_prefix(path: &Path) -> PathBuf {
    let normalized = standardize_lexical_path(path);
    if let Ok(canon) = std::fs::canonicalize(&normalized) {
        return canon;
    }
    let mut current = normalized.as_path();
    let mut suffix_components = Vec::new();
    while let Some(name) = current.file_name() {
        suffix_components.push(name.to_os_string());
        let Some(parent) = current.parent() else {
            break;
        };
        if let Ok(canon_parent) = std::fs::canonicalize(parent) {
            let mut rebuilt = canon_parent;
            for comp in suffix_components.into_iter().rev() {
                rebuilt.push(comp);
            }
            return rebuilt;
        }
        current = parent;
    }
    normalized
}
