use std::collections::HashMap;
use std::path::Path;

use ariadusage_core::cookie as cookie_core;
use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;
use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags, params_from_iter};
use zeroize::Zeroizing;

use crate::brokers::call::BrokerCall;

use super::catalog::Browser;
use super::discover::DiscoveredProfile;
use super::error::BrowserError;
use super::gate::{BrowserGate, RetryScope};
use super::sqlite_copy::{copy_database, create_copy_dir};
use super::{
    BrowserDiagnostics, BrowserPaths, CookieCandidate, CookieQuery, SafeStorageKeyProvider,
};

const UNIX_EPOCH_FROM_CHROMIUM_MICROS: i64 = 11_644_473_600_000_000;
pub(super) type SafeStorageKeyResult = Result<
    Option<Zeroizing<Vec<u8>>>,
    crate::brokers::secret_store::safe_storage::SafeStorageError,
>;

pub(super) struct ChromiumAccess<'a> {
    gate: &'a BrowserGate,
    retry_scope: &'a mut RetryScope,
    key_cache: &'a mut HashMap<Browser, SafeStorageKeyResult>,
    keys: &'a dyn SafeStorageKeyProvider,
}

impl<'a> ChromiumAccess<'a> {
    pub(super) fn new(
        gate: &'a BrowserGate,
        retry_scope: &'a mut RetryScope,
        key_cache: &'a mut HashMap<Browser, SafeStorageKeyResult>,
        keys: &'a dyn SafeStorageKeyProvider,
    ) -> Self {
        Self {
            gate,
            retry_scope,
            key_cache,
            keys,
        }
    }
}

struct RawCookie {
    host: String,
    name: String,
    path: String,
    expires: i64,
    secure: bool,
    http_only: bool,
    plain: Zeroizing<Vec<u8>>,
    encrypted: Zeroizing<Vec<u8>>,
    partitioned: bool,
}

pub(super) async fn read_profile(
    profile: &DiscoveredProfile,
    paths: &BrowserPaths,
    query: &CookieQuery<'_>,
    call: &BrokerCall,
    access: &mut ChromiumAccess<'_>,
) -> Result<(Vec<CookieCandidate>, Option<BrowserError>), BrowserError> {
    let mut result = read_attempt(profile, paths, query, call, access).await;
    if matches!(result, Err(BrowserError::Malformed)) {
        result = read_attempt(profile, paths, query, call, access).await;
    }
    result
}

async fn read_attempt(
    profile: &DiscoveredProfile,
    paths: &BrowserPaths,
    query: &CookieQuery<'_>,
    call: &BrokerCall,
    access: &mut ChromiumAccess<'_>,
) -> Result<(Vec<CookieCandidate>, Option<BrowserError>), BrowserError> {
    let temp = create_copy_dir(paths.runtime_dir.as_deref())?;
    let copied_path = copy_database(&profile.database_path, temp.path())?;
    let (version, rows) = read_rows(&copied_path, query)?;
    let mut diagnostics = BrowserDiagnostics {
        rows_read: rows.len(),
        ..BrowserDiagnostics::default()
    };
    let now = Timestamp::now();
    let mut records = Vec::new();
    let mut encrypted_v11 = Vec::new();

    for mut row in rows {
        if row.partitioned {
            diagnostics.partitioned += 1;
            continue;
        }
        if chromium_expiration(row.expires).is_some_and(|expires| expires <= now) {
            diagnostics.expired += 1;
            continue;
        }
        if !row.plain.is_empty() {
            let plain = Zeroizing::new(std::mem::take(&mut *row.plain));
            records.push(to_record(row, plain)?);
            continue;
        }
        if row.encrypted.starts_with(b"v10") {
            match super::chromium_crypto::decrypt_v10(&row.encrypted[3..], &row.host, version) {
                Ok(plain) => records.push(to_record(row, plain)?),
                Err(super::chromium_crypto::DecryptFailure::HostHashMismatch) => {
                    diagnostics.rejected_by_hash += 1;
                }
                Err(_) => {}
            }
        } else if row.encrypted.starts_with(b"v11") {
            encrypted_v11.push(row);
        } else {
            let tag = encryption_tag(&row.encrypted);
            *diagnostics.unsupported_by_tag.entry(tag).or_default() += 1;
        }
    }

    let mut key_error = None;
    let mut safe_storage_key = None;
    if !encrypted_v11.is_empty()
        && access.gate.should_attempt_key(
            profile.browser,
            call,
            access.retry_scope,
            unix_now_seconds(),
        )?
    {
        let attribute = profile
            .browser
            .safe_storage_attribute()
            .ok_or(BrowserError::Unsupported)?;
        let key_result = if let Some(cached) = access.key_cache.get(&profile.browser) {
            cached.clone()
        } else {
            let result = access.keys.chromium_safe_storage(attribute, call).await;
            access.key_cache.insert(profile.browser, result.clone());
            result
        };
        match key_result {
            Ok(Some(key)) => {
                access.gate.record_success(profile.browser)?;
                safe_storage_key = Some(key);
            }
            Ok(None) => key_error = Some(BrowserError::Unsupported),
            Err(error) => {
                key_error = Some(match error {
                    crate::brokers::secret_store::safe_storage::SafeStorageError::Locked => {
                        BrowserError::Locked
                    }
                    crate::brokers::secret_store::safe_storage::SafeStorageError::Dismissed => {
                        access.gate.record_dismissal(
                            profile.browser,
                            access.retry_scope,
                            unix_now_seconds(),
                        )?;
                        BrowserError::Dismissed
                    }
                    crate::brokers::secret_store::safe_storage::SafeStorageError::NoSecretService
                    | crate::brokers::secret_store::safe_storage::SafeStorageError::KWalletUnsupported
                    | crate::brokers::secret_store::safe_storage::SafeStorageError::Unsupported => {
                        BrowserError::Unsupported
                    }
                    _ => BrowserError::Io,
                });
            }
        }
    } else if !encrypted_v11.is_empty() {
        key_error = Some(BrowserError::Dismissed);
    }
    for row in encrypted_v11 {
        let ciphertext = &row.encrypted[3..];
        let primary = safe_storage_key.as_ref().and_then(|key| {
            super::chromium_crypto::decrypt_v11(ciphertext, key, &row.host, version).ok()
        });
        let decrypted = primary.or_else(|| {
            super::chromium_crypto::decrypt_empty_password(ciphertext, &row.host, version).ok()
        });
        match decrypted {
            Some(plain) => records.push(to_record(row, plain)?),
            None => {
                if safe_storage_key.as_ref().is_some_and(|key| {
                    matches!(
                        super::chromium_crypto::decrypt_v11(ciphertext, key, &row.host, version),
                        Err(super::chromium_crypto::DecryptFailure::HostHashMismatch)
                    )
                }) || matches!(
                    super::chromium_crypto::decrypt_empty_password(ciphertext, &row.host, version),
                    Err(super::chromium_crypto::DecryptFailure::HostHashMismatch)
                ) {
                    diagnostics.rejected_by_hash += 1;
                }
            }
        }
    }

    let candidates = if records.is_empty()
        && (diagnostics.unsupported_by_tag.is_empty() || key_error.is_some())
    {
        Vec::new()
    } else {
        vec![CookieCandidate {
            browser: profile.browser,
            profile_id: profile.profile_id.clone(),
            label: profile.label.clone(),
            records,
            diagnostics,
        }]
    };
    Ok((candidates, key_error))
}

fn read_rows(
    database: &Path,
    query: &CookieQuery<'_>,
) -> Result<(i64, Vec<RawCookie>), BrowserError> {
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    connection.pragma_update(None, "trusted_schema", false)?;
    let version = connection.query_row("SELECT version FROM meta LIMIT 1", [], |row| row.get(0))?;
    let mut statement = connection.prepare(&format!(
        "SELECT host_key, name, path, expires_utc, is_secure, is_httponly, value, encrypted_value, top_frame_site_key FROM cookies WHERE {}{}",
        domain_predicate(query.domains.domains().len()),
        name_predicate(query.names)
    ))?;
    let mut parameters = domain_parameters(query.domains.domains());
    if let Some(names) = query.names {
        parameters.extend(names.iter().map(|name| Value::Text((*name).to_owned())));
    }
    let rows = statement.query_map(params_from_iter(parameters.iter()), |row| {
        let host: String = row.get(0)?;
        let name: String = row.get(1)?;
        let path: String = row.get(2)?;
        let expires: i64 = row.get(3)?;
        let secure: bool = row.get(4)?;
        let http_only: bool = row.get(5)?;
        let plain = secret_bytes(row.get(6)?);
        let encrypted = secret_bytes(row.get(7)?);
        let partition_key: String = row.get(8)?;
        Ok(RawCookie {
            host,
            name,
            path,
            expires,
            secure,
            http_only,
            plain,
            encrypted,
            partitioned: !partition_key.is_empty(),
        })
    })?;
    let mut matched = Vec::new();
    for row in rows {
        let row = row?;
        if query.domains.matches(&row.host)
            && query
                .names
                .is_none_or(|names| names.contains(&row.name.as_str()))
        {
            matched.push(row);
        }
    }
    Ok((version, matched))
}

fn domain_predicate(count: usize) -> String {
    let conditions = (0..count)
        .map(|_| "(host_key = ? OR host_key = ? OR host_key LIKE ? ESCAPE '\\')")
        .collect::<Vec<_>>()
        .join(" OR ");
    format!("({conditions})")
}

fn name_predicate(names: Option<&[&str]>) -> String {
    names
        .filter(|names| !names.is_empty())
        .map(|names| format!(" AND name IN ({})", vec!["?"; names.len()].join(", ")))
        .unwrap_or_default()
}

fn domain_parameters(domains: &[String]) -> Vec<Value> {
    let mut parameters = Vec::with_capacity(domains.len() * 3);
    for domain in domains {
        let escaped = domain
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        parameters.push(Value::Text(domain.clone()));
        parameters.push(Value::Text(format!(".{domain}")));
        parameters.push(Value::Text(format!("%.{escaped}")));
    }
    parameters
}

fn secret_bytes(value: Value) -> Zeroizing<Vec<u8>> {
    Zeroizing::new(match value {
        Value::Text(value) => value.into_bytes(),
        Value::Blob(value) => value,
        _ => Vec::new(),
    })
}

fn to_record(
    row: RawCookie,
    plain: Zeroizing<Vec<u8>>,
) -> Result<cookie_core::CookieRecord, BrowserError> {
    let value = std::str::from_utf8(&plain).map_err(|_| BrowserError::Malformed)?;
    let secret = SecretString::new(value.to_owned());
    Ok(cookie_core::CookieRecord {
        name: row.name,
        value: secret,
        host_only: !row.host.starts_with('.'),
        domain: row.host.trim_start_matches('.').to_ascii_lowercase(),
        path: row.path,
        secure: row.secure,
        http_only: row.http_only,
        expires: chromium_expiration(row.expires),
    })
}

fn chromium_expiration(value: i64) -> Option<Timestamp> {
    if value == 0 {
        return None;
    }
    let unix_micros = value.checked_sub(UNIX_EPOCH_FROM_CHROMIUM_MICROS)?;
    Timestamp::from_nanosecond(i128::from(unix_micros) * 1_000).ok()
}

fn unix_now_seconds() -> i64 {
    Timestamp::now().as_second()
}

fn encryption_tag(value: &[u8]) -> String {
    let tag = value.get(..3).unwrap_or(value);
    if tag.len() == 3 && tag.iter().all(u8::is_ascii_alphanumeric) {
        String::from_utf8_lossy(tag).into_owned()
    } else {
        "unknown".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params_from_iter};

    use super::{domain_parameters, domain_predicate};

    #[test]
    fn domain_query_binds_quotes_and_escapes_like_wildcards() {
        let connection = Connection::open_in_memory().expect("in-memory database");
        connection
            .execute("CREATE TABLE cookies(host_key TEXT)", [])
            .expect("cookie table");
        for host in [
            "quoted'_%",
            ".quoted'_%",
            "sub.quoted'_%",
            "sub.quoted'Xvalue",
            "subXquoted'_%",
        ] {
            connection
                .execute("INSERT INTO cookies VALUES (?1)", [host])
                .expect("insert synthetic host");
        }

        let domains = ["quoted'_%".to_owned()];
        let parameters = domain_parameters(&domains);
        let query = format!(
            "SELECT host_key FROM cookies WHERE {} ORDER BY host_key",
            domain_predicate(domains.len())
        );
        let mut statement = connection.prepare(&query).expect("parameterized query");
        let matched = statement
            .query_map(params_from_iter(parameters.iter()), |row| {
                row.get::<_, String>(0)
            })
            .expect("query rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("host rows");

        assert_eq!(matched, [".quoted'_%", "quoted'_%", "sub.quoted'_%"]);
    }
}
