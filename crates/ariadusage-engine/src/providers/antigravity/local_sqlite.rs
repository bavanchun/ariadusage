// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityLocalSQLite.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod imp {
    use std::collections::{BTreeMap, BTreeSet};
    use std::io::Read as _;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    use rusqlite::functions::FunctionFlags;
    use rusqlite::limits::Limit;
    use rusqlite::types::ValueRef;
    use rusqlite::{Connection, OpenFlags};

    use super::super::local_reader::{AntigravityLocalReader, Event, SourceResult};
    use super::super::local_scan::{Budget, ScanFailure};
    use super::super::local_sqlite_schema::{SQLitePayload, SQLiteTableSupport, is_cannot_open};
    use super::super::proto_reader::{AntigravityProtoReader, ParsedTurn, ParsedUsage};

    #[derive(Debug, Default)]
    struct SQLProgressState {
        failure: Option<ScanFailure>,
        database_bytes: usize,
        database_rows: usize,
    }

    #[derive(Clone)]
    struct SQLProgress {
        budget: Budget,
        state: Arc<Mutex<SQLProgressState>>,
    }

    impl SQLProgress {
        fn new(budget: Budget) -> Self {
            Self {
                budget,
                state: Arc::new(Mutex::new(SQLProgressState::default())),
            }
        }

        fn payload_limit(&self) -> i64 {
            let stats = self.budget.statistics();
            let (database_bytes, database_rows) = self.counts();
            let limits = self.budget.limits();
            if stats.rows >= limits.rows || database_rows >= limits.rows_per_database {
                return 0;
            }
            let limit = limits
                .blob_bytes
                .min(limits.database_bytes.saturating_sub(database_bytes))
                .min(limits.bytes.saturating_sub(stats.attempted_bytes));
            i64::try_from(limit).unwrap_or(i64::MAX)
        }

        fn step_payload_limit(&self) -> i64 {
            let stats = self.budget.statistics();
            let (database_bytes, _) = self.counts();
            let limits = self.budget.limits();
            if stats.rows >= limits.rows
                || database_bytes >= limits.database_bytes
                || stats.attempted_bytes >= limits.bytes
            {
                return 0;
            }
            let limit = limits
                .blob_bytes
                .min(limits.database_bytes.saturating_sub(database_bytes))
                .min(limits.bytes.saturating_sub(stats.attempted_bytes));
            i64::try_from(limit).unwrap_or(i64::MAX)
        }

        fn advance(&self) -> bool {
            match self.budget.check() {
                Ok(()) => false,
                Err(err) => {
                    let mut guard = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    guard.failure = Some(err);
                    true
                }
            }
        }

        fn take_failure(&self) -> Option<ScanFailure> {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .failure
                .take()
        }

        fn counts(&self) -> (usize, usize) {
            let guard = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (guard.database_bytes, guard.database_rows)
        }

        fn increment_database_rows(&self) -> usize {
            let mut guard = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.database_rows = guard.database_rows.saturating_add(1);
            guard.database_rows
        }

        fn database_bytes(&self) -> usize {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .database_bytes
        }

        fn add_database_bytes(&self, count: usize) {
            let mut guard = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.database_bytes = guard.database_bytes.saturating_add(count);
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum OpenMode {
        /// Ordinary read-only access with WAL read-mark coordination through the sidecars.
        ReadOnly,
        /// Lock-free access to the main database file alone. Never creates or touches sidecars.
        Immutable,
    }

    struct DatabaseAttempt {
        source: SourceResult,
        /// SQLite declined the database itself (`SQLITE_CANTOPEN`) before any row was read.
        cannot_open: bool,
    }

    impl DatabaseAttempt {
        fn new(source: SourceResult, cannot_open: bool) -> Self {
            Self {
                source,
                cannot_open,
            }
        }
    }

    /// Identity of an idle WAL database: no `-wal` sidecar, plus the main file's size, modification time,
    /// file system number, and 100-byte header. `None` when a `-wal` sidecar exists or the file cannot be examined.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct IdleDatabaseState {
        size: u64,
        modified: SystemTime,
        file_number: u64,
        header: Vec<u8>,
    }

    struct DatabaseHandleGuard {
        conn: Option<Connection>,
        budget: Budget,
        rollback_on_drop: bool,
    }

    impl DatabaseHandleGuard {
        fn new(conn: Connection, budget: Budget) -> Self {
            Self {
                conn: Some(conn),
                budget,
                rollback_on_drop: false,
            }
        }

        fn conn(&self) -> &Connection {
            self.conn
                .as_ref()
                .expect("connection present while guard is live")
        }
    }

    impl Drop for DatabaseHandleGuard {
        fn drop(&mut self) {
            if let Some(conn) = self.conn.take() {
                let _ = conn.progress_handler(0, None::<fn() -> bool>);
                if self.rollback_on_drop {
                    let _ = conn.execute_batch("ROLLBACK");
                }
                if conn.close().is_ok() {
                    self.budget
                        .update_statistics(|s| s.sqlite_handles_closed += 1);
                }
            }
        }
    }

    #[derive(Clone)]
    struct PendingTimestampRow {
        row: i64,
        step_uuid: String,
        turn: ParsedTurn,
    }

    impl std::fmt::Debug for PendingTimestampRow {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("PendingTimestampRow")
                .field("row", &self.row)
                .field("has_step_uuid", &!self.step_uuid.is_empty())
                .field("turn", &self.turn)
                .finish()
        }
    }

    #[derive(Default)]
    struct ParsedRows {
        source: SourceResult,
        pending_timestamp_rows: Vec<PendingTimestampRow>,
        step_occurrences: BTreeMap<String, Vec<StepOccurrence>>,
        bot_id_uses: BTreeMap<String, usize>,
    }

    #[derive(Clone)]
    struct StepOccurrence {
        row: i64,
        timestamp_ms: Option<i64>,
        bot_id: Option<String>,
    }

    impl std::fmt::Debug for StepOccurrence {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("StepOccurrence")
                .field("row", &self.row)
                .field("timestamp_ms", &self.timestamp_ms)
                .field("has_bot_id", &self.bot_id.is_some())
                .finish()
        }
    }

    #[derive(Clone)]
    struct ExactStepTimestamp {
        step_uuid: String,
        timestamp_ms: i64,
    }

    impl std::fmt::Debug for ExactStepTimestamp {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("ExactStepTimestamp")
                .field("has_step_uuid", &!self.step_uuid.is_empty())
                .field("timestamp_ms", &self.timestamp_ms)
                .finish()
        }
    }

    #[derive(Default)]
    struct StepTimestampScan {
        timestamps: BTreeMap<String, Vec<i64>>,
        by_bot_id: BTreeMap<String, ExactStepTimestamp>,
        ambiguous_bot_ids: BTreeSet<String>,
        is_complete: bool,
    }

    impl AntigravityLocalReader {
        pub fn read_databases(
            paths: &[PathBuf],
            budget: &Budget,
        ) -> Result<SourceResult, ScanFailure> {
            let mut result = SourceResult::default();
            for url in paths {
                let step: Result<SourceResult, ScanFailure> = (|| {
                    budget.check()?;
                    let files = budget.update_statistics(|s| {
                        s.files += 1;
                        s.files
                    });
                    if files > budget.limits().databases {
                        return Err(ScanFailure::Exhausted);
                    }
                    budget.begin_database();
                    Self::read_database(url, budget)
                })();
                match step {
                    Ok(source) => {
                        result.events.extend(source.events);
                        result.is_complete = result.is_complete && source.is_complete;
                        result.contains_history_source =
                            result.contains_history_source || source.contains_history_source;
                        result.evidence_is_unstable =
                            result.evidence_is_unstable || source.evidence_is_unstable;
                    }
                    Err(ScanFailure::SchemaExhausted) => {
                        // Schema limits apply to each database, so an oversized schema costs only its own database.
                        // Rows from the other databases stay valid partial history. Hard row, byte, and duration
                        // limits are not caught here and continue to withhold newly truncated reports as documented.
                        result.is_complete = false;
                    }
                    Err(err) => return Err(err),
                }
            }
            Ok(result)
        }

        fn read_database(url: &Path, budget: &Budget) -> Result<SourceResult, ScanFailure> {
            let before_source = Self::idle_database_state(url);
            let Some(temp_parent) = Self::resolve_copy_parent(budget) else {
                return Ok(SourceResult::incomplete());
            };
            let Ok(temp_dir) = crate::private_tempdir::create_in(&temp_parent) else {
                return Ok(SourceResult::incomplete());
            };
            let Ok(copied_url) =
                crate::brokers::browser::sqlite_copy::copy_database(url, temp_dir.path())
            else {
                return Ok(SourceResult::incomplete());
            };
            let attempt = Self::read_database_with_mode(&copied_url, budget, OpenMode::ReadOnly)?;
            // Some SQLite builds (Apple's system library among them) decline read-only access to a WAL database
            // whose -wal and -shm sidecars are absent, because a read-only connection may not create them.
            // A cleanly closed conversation is exactly that file. An absent -wal also means no WAL connection
            // holds the database, so the main file alone carries the checkpointed state.
            let Some(before_copy) = (if attempt.cannot_open {
                Self::idle_database_state(&copied_url)
            } else {
                None
            }) else {
                return Ok(attempt.source);
            };
            let Some(before_source) = before_source else {
                return Ok(attempt.source);
            };
            budget.update_statistics(|s| s.immutable_fallbacks += 1);
            let mut source =
                Self::read_database_with_mode(&copied_url, budget, OpenMode::Immutable)?.source;
            // An immutable connection neither locks nor detects changes. A writer that appeared during the read
            // could have checkpointed pages into the main file, so the result is trusted only when the file and
            // its sidecar state are unchanged afterwards. Anything else stays incomplete, as before.
            if Self::idle_database_state(&copied_url).as_ref() != Some(&before_copy)
                || Self::idle_database_state(url).as_ref() != Some(&before_source)
            {
                source.is_complete = false;
                source.evidence_is_unstable = true;
            }
            Ok(source)
        }

        fn resolve_copy_parent(budget: &Budget) -> Option<PathBuf> {
            let configured = budget.copy_parent()?;
            crate::private_tempdir::ensure_private_0700_dir(configured)
        }

        fn idle_database_state(url: &Path) -> Option<IdleDatabaseState> {
            let mut wal_os = url.as_os_str().to_os_string();
            wal_os.push("-wal");
            if std::fs::symlink_metadata(Path::new(&wal_os)).is_ok() {
                return None;
            }
            let metadata = std::fs::symlink_metadata(url).ok()?;
            if !metadata.file_type().is_file() {
                return None;
            }
            let size = metadata.len();
            let modified = metadata.modified().ok()?;
            #[cfg(unix)]
            let file_number = std::os::unix::fs::MetadataExt::ino(&metadata);
            #[cfg(not(unix))]
            let file_number = 0u64;
            let fd = rustix::fs::open(
                url,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::NONBLOCK
                    | rustix::fs::OFlags::NOCTTY
                    | rustix::fs::OFlags::CLOEXEC
                    | rustix::fs::OFlags::NOFOLLOW,
                rustix::fs::Mode::empty(),
            )
            .ok()?;
            let mut handle = std::fs::File::from(fd);
            let mut buf = [0u8; 100];
            let mut read_len = 0usize;
            while read_len < 100 {
                match handle.read(&mut buf[read_len..]) {
                    Ok(0) => break,
                    Ok(n) => read_len += n,
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => return None,
                }
            }
            Some(IdleDatabaseState {
                size,
                modified,
                file_number,
                header: buf[..read_len].to_vec(),
            })
        }

        /// The `immutable=1` query parameter is only reachable through a URI filename.
        fn immutable_uri(url: &Path) -> Option<String> {
            let raw_path = url.to_str()?;
            let mut encoded = String::with_capacity(raw_path.len() + 16);
            for &byte in raw_path.as_bytes() {
                let is_allowed = byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'-' | b'.'
                            | b'_'
                            | b'~'
                            | b'/'
                            | b':'
                            | b'@'
                            | b'!'
                            | b'$'
                            | b'&'
                            | b'\''
                            | b'('
                            | b')'
                            | b'*'
                            | b'+'
                            | b','
                            | b';'
                            | b'='
                    );
                if is_allowed {
                    encoded.push(char::from(byte));
                } else {
                    use std::fmt::Write as _;
                    let _ = write!(&mut encoded, "%{byte:02X}");
                }
            }
            Some(format!("file:{encoded}?immutable=1"))
        }

        /// Opens the database for the given mode. A failed open leaves no handle behind.
        fn open_database(url: &Path, mode: OpenMode, budget: &Budget) -> Result<Connection, bool> {
            let result = match mode {
                OpenMode::ReadOnly => Connection::open_with_flags(
                    url,
                    OpenFlags::SQLITE_OPEN_READ_ONLY
                        | OpenFlags::SQLITE_OPEN_NO_MUTEX
                        | OpenFlags::SQLITE_OPEN_NOFOLLOW,
                ),
                OpenMode::Immutable => {
                    let Some(uri) = Self::immutable_uri(url) else {
                        return Err(true);
                    };
                    Connection::open_with_flags(
                        uri,
                        OpenFlags::SQLITE_OPEN_READ_ONLY
                            | OpenFlags::SQLITE_OPEN_URI
                            | OpenFlags::SQLITE_OPEN_NO_MUTEX
                            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
                    )
                }
            };
            match result {
                Ok(conn) => {
                    if conn
                        .set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
                        .is_err()
                        || conn.pragma_update(None, "trusted_schema", false).is_err()
                    {
                        let _ = conn.close();
                        return Err(false);
                    }
                    budget.update_statistics(|s| s.sqlite_handles_opened += 1);
                    Ok(conn)
                }
                Err(err) => Err(is_cannot_open(&err)),
            }
        }

        fn read_database_with_mode(
            url: &Path,
            budget: &Budget,
            mode: OpenMode,
        ) -> Result<DatabaseAttempt, ScanFailure> {
            let conn = match Self::open_database(url, mode, budget) {
                Ok(conn) => conn,
                Err(cannot_open) => {
                    return Ok(DatabaseAttempt::new(
                        SourceResult::incomplete(),
                        cannot_open,
                    ));
                }
            };
            let mut guard = DatabaseHandleGuard::new(conn, budget.clone());
            let progress = SQLProgress::new(budget.clone());
            // Also bound SQLite's own intermediate values, before step can materialize a hostile record/view.
            let maximum_value_bytes = budget.limits().blob_bytes.min(16 * 1024 * 1024) + 1024;
            let initial_limit =
                i32::try_from(maximum_value_bytes.min(64 * 1024)).unwrap_or(64 * 1024);
            let _ = guard
                .conn()
                .set_limit(Limit::SQLITE_LIMIT_LENGTH, initial_limit);

            let limit_progress = progress.clone();
            if guard
                .conn()
                .create_scalar_function(
                    "antigravity_payload_limit",
                    0,
                    FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DIRECTONLY,
                    move |_ctx| Ok(limit_progress.payload_limit()),
                )
                .is_err()
            {
                return Ok(DatabaseAttempt::new(SourceResult::incomplete(), false));
            }

            let handler_progress = progress.clone();
            if guard
                .conn()
                .progress_handler(1000, Some(move || handler_progress.advance()))
                .is_err()
            {
                return Ok(DatabaseAttempt::new(SourceResult::incomplete(), false));
            }
            guard.rollback_on_drop = true;

            // Ordinary read-only SQLite permits WAL read-mark coordination; it does not promise unchanged SHM bytes.
            if let Err(err) = guard.conn().execute_batch("BEGIN DEFERRED") {
                if let Some(failure) = progress.take_failure() {
                    return Err(failure);
                }
                return Ok(DatabaseAttempt::new(
                    SourceResult::incomplete(),
                    is_cannot_open(&err),
                ));
            }

            let (support, cannot_open) = Self::inspect_sqlite_table_support_with_open_status(
                guard.conn(),
                "gen_metadata",
                "data",
                budget,
            )?;
            if let Some(failure) = progress.take_failure() {
                return Err(failure);
            }
            match support {
                SQLiteTableSupport::Supported => {}
                SQLiteTableSupport::Foreign => {
                    // A database in a declared root that describes its own tables and no gen_metadata table is not
                    // Antigravity history. Skipping it costs no coverage. A gen_metadata table with unknown columns
                    // is schema drift, not a foreign file, and stays incomplete on purpose.
                    budget.update_statistics(|s| s.foreign_databases += 1);
                    return Ok(DatabaseAttempt::new(SourceResult::default(), false));
                }
                SQLiteTableSupport::Unsupported => {
                    // The deferred transaction first touches the file at the schema read, so a declined WAL open
                    // surfaces here as a failed prepare and leaves SQLITE_CANTOPEN as the connection's last error.
                    return Ok(DatabaseAttempt::new(
                        SourceResult::incomplete(),
                        cannot_open,
                    ));
                }
            }

            let payload_limit = i32::try_from(maximum_value_bytes).unwrap_or(i32::MAX);
            let _ = guard
                .conn()
                .set_limit(Limit::SQLITE_LIMIT_LENGTH, payload_limit);

            // length(BLOB) reads its size without loading the payload. The non-deterministic limit is
            // evaluated for each row against the remaining job budget, before selecting any payload.
            // No ORDER BY: a sorter could otherwise materialize multiple payloads ahead of accounting.
            let query = "SELECT idx, CASE WHEN typeof(data) = 'blob' THEN length(data) END, \
            CASE WHEN typeof(data) = 'blob' AND length(data) <= antigravity_payload_limit() THEN data END \
            FROM main.gen_metadata NOT INDEXED LIMIT ?";
            let mut statement = match guard.conn().prepare(query) {
                Ok(stmt) => stmt,
                Err(err) => {
                    if let Some(failure) = progress.take_failure() {
                        return Err(failure);
                    }
                    return Ok(DatabaseAttempt::new(
                        SourceResult::incomplete(),
                        is_cannot_open(&err),
                    ));
                }
            };
            let row_limit_bind = i64::try_from(
                budget
                    .limits()
                    .rows_per_database
                    .min(10_000)
                    .saturating_add(1),
            )
            .unwrap_or(10_001);
            if let Err(err) = statement.raw_bind_parameter(1, row_limit_bind) {
                return Ok(DatabaseAttempt::new(
                    SourceResult::incomplete(),
                    is_cannot_open(&err),
                ));
            }

            let session = url
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_owned();
            let mut rows = Self::read_rows(&mut statement, &session, &progress)?;
            // Release the gen_metadata cursor before the optional steps pass reuses the same snapshot.
            drop(statement);

            if !rows.pending_timestamp_rows.is_empty() {
                // Positional recovery is safe only when every generation row participated in the occurrence list.
                // A malformed row can still carry a reused step UUID, so partial primary scans must not realign later rows.
                if !rows.source.is_complete {
                    return Ok(DatabaseAttempt::new(rows.source, false));
                }
                let has_steps =
                    Self::inspect_sqlite_table_support(guard.conn(), "steps", "metadata", budget)?
                        == SQLiteTableSupport::Supported;
                if let Some(failure) = progress.take_failure() {
                    return Err(failure);
                }
                if has_steps {
                    let mut needed_step_occurrences: BTreeMap<String, Vec<StepOccurrence>> =
                        BTreeMap::new();
                    for pending in &rows.pending_timestamp_rows {
                        let occurrences = rows
                            .step_occurrences
                            .get(&pending.step_uuid)
                            .cloned()
                            .unwrap_or_default();
                        needed_step_occurrences.insert(pending.step_uuid.clone(), occurrences);
                    }
                    let step_scan = Self::read_step_timestamps(
                        guard.conn(),
                        &needed_step_occurrences,
                        &progress,
                    )?;
                    if !step_scan.is_complete
                        || !Self::embedded_timestamps_agree(
                            &needed_step_occurrences,
                            &step_scan,
                            &rows.bot_id_uses,
                        )
                    {
                        rows.source.is_complete = false;
                        return Ok(DatabaseAttempt::new(rows.source, false));
                    }
                    let recovered_count =
                        Self::append_recovered_events(&mut rows, &session, &step_scan);
                    if recovered_count < rows.pending_timestamp_rows.len() {
                        rows.source.is_complete = false;
                    }
                } else {
                    rows.source.is_complete = false;
                }
            }
            Ok(DatabaseAttempt::new(rows.source, false))
        }

        fn read_step_timestamps(
            database: &Connection,
            needed_step_occurrences: &BTreeMap<String, Vec<StepOccurrence>>,
            progress: &SQLProgress,
        ) -> Result<StepTimestampScan, ScanFailure> {
            if needed_step_occurrences.is_empty() {
                return Ok(StepTimestampScan {
                    is_complete: true,
                    ..StepTimestampScan::default()
                });
            }
            let step_progress = progress.clone();
            if database
                .create_scalar_function(
                    "antigravity_step_payload_limit",
                    0,
                    FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DIRECTONLY,
                    move |_ctx| Ok(step_progress.step_payload_limit()),
                )
                .is_err()
            {
                return Ok(StepTimestampScan::default());
            }

            struct StepFnGuard<'a>(&'a Connection);
            impl Drop for StepFnGuard<'_> {
                fn drop(&mut self) {
                    let _ = self.0.remove_function("antigravity_step_payload_limit", 0);
                }
            }
            let _fn_guard = StepFnGuard(database);

            let query = "SELECT idx, CASE WHEN typeof(metadata) = 'blob' THEN length(metadata) END, \
            CASE WHEN typeof(metadata) = 'blob' \
                AND length(metadata) <= antigravity_step_payload_limit() THEN metadata END \
            FROM main.steps NOT INDEXED";
            let mut statement = match database.prepare(query) {
                Ok(stmt) => stmt,
                Err(_) => {
                    if let Some(failure) = progress.take_failure() {
                        return Err(failure);
                    }
                    return Ok(StepTimestampScan::default());
                }
            };

            let mut step_timestamps: BTreeMap<String, Vec<StepOccurrence>> = BTreeMap::new();
            let mut exact_by_bot_id: BTreeMap<String, ExactStepTimestamp> = BTreeMap::new();
            let mut ambiguous_bot_ids: BTreeSet<String> = BTreeSet::new();
            let mut is_complete = false;
            let mut rows_are_valid = true;
            let mut saw_unidentified_rows = false;

            {
                let mut rows = statement.raw_query();
                loop {
                    progress.budget.check()?;
                    let step = rows.next();
                    if let Some(failure) = progress.take_failure() {
                        return Err(failure);
                    }
                    let row = match step {
                        Ok(None) => {
                            is_complete = true;
                            break;
                        }
                        Err(_) => break,
                        Ok(Some(row)) => row,
                    };
                    let payload = SQLitePayload::from_row(row);
                    progress
                        .budget
                        .update_statistics(|s| s.materialized_payload_bytes += payload.byte_count);
                    // Every secondary row belongs to the job-wide budget; the separate byte ceiling only
                    // prevents one database's step metadata from consuming the whole job allocation.
                    progress.budget.charge_row()?;
                    let count = match row.get_ref(1) {
                        Ok(ValueRef::Integer(n)) => usize::try_from(n.max(0)).unwrap_or(usize::MAX),
                        _ => 0,
                    };
                    let attempted_bytes = count.max(payload.byte_count);
                    progress.budget.charge_bytes(attempted_bytes)?;
                    if attempted_bytes
                        > progress
                            .budget
                            .limits()
                            .database_bytes
                            .saturating_sub(progress.database_bytes())
                    {
                        return Err(ScanFailure::Exhausted);
                    }
                    progress.add_database_bytes(attempted_bytes);
                    let Ok(ValueRef::Integer(row_idx)) = row.get_ref(0) else {
                        rows_are_valid = false;
                        continue;
                    };
                    if count == 0 || count > progress.budget.limits().blob_bytes || row_idx < 0 {
                        rows_are_valid = false;
                        continue;
                    }
                    let Some(bytes) = payload.copy(count, progress.budget.limits().blob_bytes)
                    else {
                        rows_are_valid = false;
                        continue;
                    };
                    let Some(parsed) = AntigravityProtoReader::parse_step_metadata(&bytes, || {
                        progress.budget.check()
                    })?
                    else {
                        rows_are_valid = false;
                        continue;
                    };
                    if let Some(ref bot_id) = parsed.bot_id {
                        Self::record_exact_bot_id(
                            bot_id,
                            parsed.step_uuid.as_deref(),
                            parsed.timestamp_ms,
                            &mut exact_by_bot_id,
                            &mut ambiguous_bot_ids,
                        );
                    }
                    // Unidentified rows cannot supply UUID positions, but their bot IDs still count as evidence.
                    let Some(step_uuid) = parsed.step_uuid else {
                        saw_unidentified_rows = true;
                        continue;
                    };
                    if needed_step_occurrences.contains_key(&step_uuid) {
                        step_timestamps
                            .entry(step_uuid)
                            .or_default()
                            .push(StepOccurrence {
                                row: row_idx,
                                timestamp_ms: parsed.timestamp_ms,
                                bot_id: parsed.bot_id,
                            });
                    }
                }
            }
            drop(statement);

            let resolved = Self::resolve_step_timestamps(
                &step_timestamps,
                needed_step_occurrences,
                &ambiguous_bot_ids,
                saw_unidentified_rows,
            );
            Ok(StepTimestampScan {
                timestamps: resolved,
                by_bot_id: exact_by_bot_id,
                ambiguous_bot_ids,
                is_complete: is_complete && rows_are_valid,
            })
        }

        fn record_exact_bot_id(
            bot_id: &str,
            step_uuid: Option<&str>,
            timestamp_ms: Option<i64>,
            exact: &mut BTreeMap<String, ExactStepTimestamp>,
            ambiguous: &mut BTreeSet<String>,
        ) {
            if ambiguous.contains(bot_id) {
                return;
            }
            let (Some(step_uuid), Some(timestamp_ms)) = (step_uuid, timestamp_ms) else {
                exact.remove(bot_id);
                ambiguous.insert(bot_id.to_owned());
                return;
            };
            if let Some(existing) = exact.get(bot_id) {
                if existing.timestamp_ms != timestamp_ms || existing.step_uuid != step_uuid {
                    exact.remove(bot_id);
                    ambiguous.insert(bot_id.to_owned());
                }
            } else {
                exact.insert(
                    bot_id.to_owned(),
                    ExactStepTimestamp {
                        step_uuid: step_uuid.to_owned(),
                        timestamp_ms,
                    },
                );
            }
        }

        fn embedded_timestamps_agree(
            occurrences: &BTreeMap<String, Vec<StepOccurrence>>,
            step_scan: &StepTimestampScan,
            bot_id_uses: &BTreeMap<String, usize>,
        ) -> bool {
            for (step_uuid, rows) in occurrences {
                for row in rows {
                    let (Some(timestamp_ms), Some(bot_id)) =
                        (row.timestamp_ms, row.bot_id.as_deref())
                    else {
                        continue;
                    };
                    if step_scan.ambiguous_bot_ids.contains(bot_id) {
                        return false;
                    }
                    if let Some(exact) = step_scan.by_bot_id.get(bot_id)
                        && (&exact.step_uuid != step_uuid
                            || (bot_id_uses.get(bot_id).copied() == Some(1)
                                && exact.timestamp_ms != timestamp_ms))
                    {
                        return false;
                    }
                }
            }
            true
        }

        fn resolve_step_timestamps(
            step_timestamps: &BTreeMap<String, Vec<StepOccurrence>>,
            needed_step_occurrences: &BTreeMap<String, Vec<StepOccurrence>>,
            ambiguous_bot_ids: &BTreeSet<String>,
            unidentified_rows_present: bool,
        ) -> BTreeMap<String, Vec<i64>> {
            let mut resolved = BTreeMap::new();
            for (step_uuid, timestamps) in step_timestamps {
                let Some(occurrences) = needed_step_occurrences.get(step_uuid) else {
                    continue;
                };
                let mut sorted_occurrences = occurrences.clone();
                sorted_occurrences.sort_by_key(|occ| occ.row);
                let unique_rows: BTreeSet<i64> =
                    sorted_occurrences.iter().map(|occ| occ.row).collect();
                if unique_rows.len() != sorted_occurrences.len() {
                    continue;
                }
                let needed_count = sorted_occurrences.len();
                let mut sorted = timestamps.clone();
                sorted.sort_by_key(|occ| occ.row);
                // The defensive schema does not require idx to be a key. Conflicting duplicate indices
                // cannot be ordered safely, so withhold that UUID instead of publishing a false date.
                if sorted.windows(2).any(|pair| pair[0].row == pair[1].row) {
                    continue;
                }
                // Keep ambiguous slots so later timestamps cannot slide into their positions.
                let ordered_timestamps: Vec<Option<i64>> = sorted
                    .iter()
                    .map(|step| {
                        if step
                            .bot_id
                            .as_deref()
                            .is_some_and(|bot_id| ambiguous_bot_ids.contains(bot_id))
                        {
                            None
                        } else {
                            step.timestamp_ms
                        }
                    })
                    .collect();
                let selected: Vec<i64> = if ordered_timestamps.len() == 1
                    && (needed_count == 1 || !unidentified_rows_present)
                    && let Some(shared_timestamp) = ordered_timestamps[0]
                {
                    // Sharing one timestamp across reused UUIDs requires a complete identity census.
                    vec![shared_timestamp; needed_count]
                } else {
                    if ordered_timestamps.len() < needed_count {
                        continue;
                    }
                    let candidates = &ordered_timestamps[..needed_count];
                    if candidates.iter().any(Option::is_none) {
                        continue;
                    }
                    candidates.iter().filter_map(|&ts| ts).collect()
                };
                if !sorted_occurrences
                    .iter()
                    .zip(selected.iter())
                    .all(|(occ, &ts)| occ.timestamp_ms.is_none() || occ.timestamp_ms == Some(ts))
                {
                    continue;
                }
                resolved.insert(step_uuid.clone(), selected);
            }
            resolved
        }

        fn read_rows(
            statement: &mut rusqlite::Statement<'_>,
            session: &str,
            progress: &SQLProgress,
        ) -> Result<ParsedRows, ScanFailure> {
            let budget = &progress.budget;
            let mut result = SourceResult {
                contains_history_source: true,
                ..SourceResult::default()
            };
            let mut pending_timestamp_rows = Vec::new();
            let mut step_occurrences: BTreeMap<String, Vec<StepOccurrence>> = BTreeMap::new();
            let mut bot_id_uses: BTreeMap<String, usize> = BTreeMap::new();

            let mut rows = statement.raw_query();
            loop {
                budget.check()?;
                let step = rows.next();
                if let Some(failure) = progress.take_failure() {
                    return Err(failure);
                }
                let row = match step {
                    Ok(None) => break,
                    Err(_) => {
                        result.is_complete = false;
                        break;
                    }
                    Ok(Some(row)) => row,
                };
                let payload = SQLitePayload::from_row(row);
                budget.update_statistics(|s| s.materialized_payload_bytes += payload.byte_count);
                let database_rows = progress.increment_database_rows();
                budget.charge_row()?;
                // Count every row, even NULL/empty records, and charge rejected bytes before another attempt.
                let count = match row.get_ref(1) {
                    Ok(ValueRef::Integer(n)) => usize::try_from(n.max(0)).unwrap_or(usize::MAX),
                    _ => 0,
                };
                let attempted_bytes = count.max(payload.byte_count);
                budget.charge_bytes(attempted_bytes)?;
                if database_rows > budget.limits().rows_per_database
                    || attempted_bytes
                        > budget
                            .limits()
                            .database_bytes
                            .saturating_sub(progress.database_bytes())
                {
                    return Err(ScanFailure::Exhausted);
                }
                progress.add_database_bytes(attempted_bytes);
                let Ok(ValueRef::Integer(row_idx)) = row.get_ref(0) else {
                    result.is_complete = false;
                    continue;
                };
                if count == 0 || count > budget.limits().blob_bytes || row_idx < 0 {
                    result.is_complete = false;
                    continue;
                }
                let Some(bytes) = payload.copy(count, budget.limits().blob_bytes) else {
                    result.is_complete = false;
                    continue;
                };
                // Validate exactly once while the single SQL snapshot is held; buffer only typed events.
                let Some(turn) = AntigravityProtoReader::parse_turn(&bytes, || budget.check())?
                else {
                    result.is_complete = false;
                    continue;
                };
                if let Some(bot_id) = turn.usage.as_ref().and_then(ParsedUsage::bot_id) {
                    *bot_id_uses.entry(bot_id.to_owned()).or_insert(0) += 1;
                }
                if let Some(ref step_uuid) = turn.step_uuid {
                    step_occurrences
                        .entry(step_uuid.clone())
                        .or_default()
                        .push(StepOccurrence {
                            row: row_idx,
                            timestamp_ms: turn.timestamp_ms,
                            bot_id: turn
                                .usage
                                .as_ref()
                                .and_then(ParsedUsage::bot_id)
                                .map(str::to_owned),
                        });
                }
                if turn.timestamp_ms.is_none()
                    && let Some(ref step_uuid) = turn.step_uuid
                {
                    pending_timestamp_rows.push(PendingTimestampRow {
                        row: row_idx,
                        step_uuid: step_uuid.clone(),
                        turn,
                    });
                    continue;
                }
                let Some(event) = Event::new(session.to_owned(), row_idx, turn, 0) else {
                    result.is_complete = false;
                    continue;
                };
                result.events.push(event);
            }

            Ok(ParsedRows {
                source: result,
                pending_timestamp_rows,
                step_occurrences,
                bot_id_uses,
            })
        }

        fn append_recovered_events(
            rows: &mut ParsedRows,
            session: &str,
            step_scan: &StepTimestampScan,
        ) -> usize {
            let mut occurrence_offsets: BTreeMap<String, BTreeMap<i64, usize>> = BTreeMap::new();
            for (step_uuid, occurrences) in &rows.step_occurrences {
                let mut sorted: Vec<i64> = occurrences.iter().map(|occ| occ.row).collect();
                sorted.sort_unstable();
                let unique: BTreeSet<i64> = sorted.iter().copied().collect();
                if unique.len() != sorted.len() {
                    continue;
                }
                let offsets = sorted
                    .into_iter()
                    .enumerate()
                    .map(|(offset, row_idx)| (row_idx, offset))
                    .collect();
                occurrence_offsets.insert(step_uuid.clone(), offsets);
            }

            let mut sorted_pending = rows.pending_timestamp_rows.clone();
            sorted_pending.sort_by_key(|p| p.row);
            let mut recovered_count = 0usize;
            for pending in sorted_pending {
                if let Some(bot_id) = pending.turn.usage.as_ref().and_then(ParsedUsage::bot_id) {
                    if step_scan.ambiguous_bot_ids.contains(bot_id) {
                        // Conflicting step timestamps for one bot ID: withhold rather than guess.
                        continue;
                    }
                    if let Some(exact) = step_scan.by_bot_id.get(bot_id)
                        && exact.step_uuid != pending.step_uuid
                    {
                        continue;
                    }
                    // All generations participate, including rows with an embedded timestamp.
                    if rows.bot_id_uses.get(bot_id).copied() == Some(1)
                        && let Some(exact) = step_scan.by_bot_id.get(bot_id)
                    {
                        let mut turn = pending.turn.clone();
                        turn.timestamp_ms = Some(exact.timestamp_ms);
                        if let Some(event) = Event::new(session.to_owned(), pending.row, turn, 0) {
                            rows.source.events.push(event);
                            recovered_count += 1;
                        }
                        continue;
                    }
                }
                let (Some(timestamps), Some(offset)) = (
                    step_scan.timestamps.get(&pending.step_uuid),
                    occurrence_offsets
                        .get(&pending.step_uuid)
                        .and_then(|map| map.get(&pending.row))
                        .copied(),
                ) else {
                    continue;
                };
                let Some(&timestamp_ms) = timestamps.get(offset) else {
                    continue;
                };
                let mut turn = pending.turn;
                turn.timestamp_ms = Some(timestamp_ms);
                if let Some(event) = Event::new(session.to_owned(), pending.row, turn, 0) {
                    rows.source.events.push(event);
                    recovered_count += 1;
                }
            }
            rows.source.events.sort_by_key(|e| e.row);
            recovered_count
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod imp {
    use std::path::PathBuf;

    use super::super::local_reader::{AntigravityLocalReader, SourceResult};
    use super::super::local_scan::{Budget, ScanFailure};

    impl AntigravityLocalReader {
        pub fn read_databases(
            paths: &[PathBuf],
            budget: &Budget,
        ) -> Result<SourceResult, ScanFailure> {
            budget.check()?;
            Ok(SourceResult {
                events: Vec::new(),
                is_complete: paths.is_empty(),
                contains_history_source: false,
                evidence_is_unstable: false,
            })
        }
    }
}
