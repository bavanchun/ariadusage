// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityLocalSQLiteSchema.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::collections::BTreeSet;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use rusqlite::types::ValueRef;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::local_reader::AntigravityLocalReader;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::local_scan::{Budget, ScanFailure};

/// Outcome of a recognized history table's schema inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SQLiteTableSupport {
    /// An ordinary table with the stored columns the payload scan needs.
    Supported,
    /// The schema walk completed and described tables, none matching the requested table.
    Foreign,
    /// Everything else: a failed inspection, an unreadable schema, or a requested table
    /// whose layout the payload scan does not support.
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SQLitePayload<'a> {
    pub byte_count: usize,
    bytes: Option<&'a [u8]>,
}

impl<'a> SQLitePayload<'a> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[must_use]
    pub fn from_row(row: &'a rusqlite::Row<'_>) -> Self {
        match row.get_ref(2) {
            Ok(ValueRef::Blob(bytes)) if !bytes.is_empty() => Self {
                byte_count: bytes.len(),
                bytes: Some(bytes),
            },
            _ => Self {
                byte_count: 0,
                bytes: None,
            },
        }
    }

    #[must_use]
    pub fn copy(&self, declared_count: usize, limit: usize) -> Option<Vec<u8>> {
        let bytes = self.bytes?;
        if self.byte_count == 0 || self.byte_count != declared_count || self.byte_count > limit {
            return None;
        }
        // Both slice and count belong to column 2. Never use a separate SQL expression as a buffer length.
        Some(bytes.to_vec())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn is_cannot_open(err: &rusqlite::Error) -> bool {
    err.sqlite_error_code() == Some(rusqlite::ErrorCode::CannotOpen)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl AntigravityLocalReader {
    pub fn inspect_sqlite_table_support(
        database: &rusqlite::Connection,
        table: &str,
        payload_column: &str,
        budget: &Budget,
    ) -> Result<SQLiteTableSupport, ScanFailure> {
        Ok(Self::inspect_sqlite_table_support_with_open_status(
            database,
            table,
            payload_column,
            budget,
        )?
        .0)
    }

    pub(crate) fn inspect_sqlite_table_support_with_open_status(
        database: &rusqlite::Connection,
        table: &str,
        payload_column: &str,
        budget: &Budget,
    ) -> Result<(SQLiteTableSupport, bool), ScanFailure> {
        // sqlite_master works on older SQLite versions too. Virtual tables and views have no root b-tree page.
        let query = "SELECT name, type, rootpage FROM main.sqlite_master LIMIT ?";
        let mut statement = match database.prepare(query) {
            Ok(stmt) => stmt,
            Err(err) => return Ok((SQLiteTableSupport::Unsupported, is_cannot_open(&err))),
        };
        let cap = budget.limits().schema_entries.min(128);
        let limit_bind = i64::try_from(cap.saturating_add(1)).unwrap_or(129);
        if let Err(err) = statement.raw_bind_parameter(1, limit_bind) {
            return Ok((SQLiteTableSupport::Unsupported, is_cannot_open(&err)));
        }
        let matched = {
            let mut rows = statement.raw_query();
            let mut entries = 0usize;
            loop {
                budget.check()?;
                let step = rows.next();
                // A completed walk over a described schema is evidence, not a failure. An empty database
                // describes nothing and stays unsupported, exactly as a failed inspection does. A walk that
                // the LIMIT truncated never proved the requested table absent, so it stays unsupported too.
                let row = match step {
                    Ok(None) => {
                        let support = if entries > 0 && entries <= cap {
                            SQLiteTableSupport::Foreign
                        } else {
                            SQLiteTableSupport::Unsupported
                        };
                        return Ok((support, false));
                    }
                    Err(err) => {
                        return Ok((SQLiteTableSupport::Unsupported, is_cannot_open(&err)));
                    }
                    Ok(Some(row)) => row,
                };
                entries += 1;
                budget.update_statistics(|s| s.schema_entries += 1);
                if entries > budget.limits().schema_entries {
                    return Err(ScanFailure::SchemaExhausted);
                }
                let Some(name) = Self::schema_text(row, 0, budget)? else {
                    return Ok((SQLiteTableSupport::Unsupported, false));
                };
                let Some(entry_type) = Self::schema_text(row, 1, budget)? else {
                    return Ok((SQLiteTableSupport::Unsupported, false));
                };
                if name.to_lowercase() != table {
                    continue;
                }
                let Ok(ValueRef::Integer(rootpage)) = row.get_ref(2) else {
                    return Ok((SQLiteTableSupport::Unsupported, false));
                };
                if entry_type != "table" || rootpage <= 0 {
                    return Ok((SQLiteTableSupport::Unsupported, false));
                }
                break true;
            }
        };
        drop(statement);
        if matched {
            let (has_columns, cannot_open) =
                Self::has_stored_sqlite_columns(database, table, payload_column, budget)?;
            Ok((
                if has_columns {
                    SQLiteTableSupport::Supported
                } else {
                    SQLiteTableSupport::Unsupported
                },
                cannot_open,
            ))
        } else {
            Ok((SQLiteTableSupport::Unsupported, false))
        }
    }

    fn has_stored_sqlite_columns(
        database: &rusqlite::Connection,
        table: &str,
        payload_column: &str,
        budget: &Budget,
    ) -> Result<(bool, bool), ScanFailure> {
        // table_info omits generated columns. An unknown table_xinfo pragma returns no columns: fail closed.
        let pragma = format!("PRAGMA main.table_xinfo('{table}')");
        let mut statement = match database.prepare(&pragma) {
            Ok(stmt) => stmt,
            Err(err) => return Ok((false, is_cannot_open(&err))),
        };
        if statement.column_count() < 7 {
            return Ok((false, false));
        }
        let mut rows = statement.raw_query();
        let mut columns = BTreeSet::new();
        let mut count = 0usize;
        loop {
            budget.check()?;
            let row = match rows.next() {
                Ok(None) => {
                    return Ok((
                        columns.contains("idx") && columns.contains(payload_column),
                        false,
                    ));
                }
                Err(err) => return Ok((false, is_cannot_open(&err))),
                Ok(Some(row)) => row,
            };
            count += 1;
            budget.update_statistics(|s| s.schema_columns += 1);
            if count > budget.limits().schema_columns.min(64) {
                return Err(ScanFailure::SchemaExhausted);
            }
            let Ok(ValueRef::Integer(hidden)) = row.get_ref(6) else {
                return Ok((false, false));
            };
            if hidden != 0 {
                return Ok((false, false));
            }
            let Some(name) = Self::schema_text(row, 1, budget)? else {
                return Ok((false, false));
            };
            let _ = Self::schema_text(row, 2, budget)?;
            let _ = Self::schema_text(row, 4, budget)?;
            columns.insert(name.to_lowercase());
        }
    }

    fn schema_text(
        row: &rusqlite::Row<'_>,
        column: usize,
        budget: &Budget,
    ) -> Result<Option<String>, ScanFailure> {
        let Ok(ValueRef::Text(bytes)) = row.get_ref(column) else {
            return Ok(None);
        };
        budget.charge_schema_bytes(bytes.len())?;
        Ok(std::str::from_utf8(bytes).ok().map(str::to_owned))
    }
}
