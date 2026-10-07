use std::collections::BTreeMap;
use std::path::Path;

use ariadusage_core::cookie as cookie_core;
use ariadusage_core::digest::sha256_hex;
use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;
use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags, params_from_iter};
use zeroize::Zeroizing;

use crate::brokers::call::BrokerCall;

use super::discover::DiscoveredProfile;
use super::error::BrowserError;
use super::sqlite_copy::{copy_database, create_copy_dir, read_optional_owned_file};
use super::{BrowserDiagnostics, BrowserPaths, CookieCandidate, CookieQuery};

struct RawCookie {
    host: String,
    name: String,
    path: String,
    expires: i64,
    secure: bool,
    http_only: bool,
    value: Zeroizing<Vec<u8>>,
    origin_attributes: String,
}

pub(super) fn read_profile(
    profile: &DiscoveredProfile,
    paths: &BrowserPaths,
    query: &CookieQuery<'_>,
    call: &BrokerCall,
) -> Result<Vec<CookieCandidate>, BrowserError> {
    let mut result = read_attempt(profile, paths, query, call);
    if matches!(result, Err(BrowserError::Malformed)) {
        result = read_attempt(profile, paths, query, call);
    }
    result
}

fn read_attempt(
    profile: &DiscoveredProfile,
    paths: &BrowserPaths,
    query: &CookieQuery<'_>,
    call: &BrokerCall,
) -> Result<Vec<CookieCandidate>, BrowserError> {
    if call.cancel.is_cancelled() {
        return Err(BrowserError::NoBrowserSession);
    }
    let temp = create_copy_dir(paths.runtime_dir.as_deref())?;
    let copied_path = copy_database(&profile.database_path, temp.path())?;
    let (version, rows) = read_rows(&copied_path, query)?;
    let containers = read_containers(&profile.profile_path);
    let now = Timestamp::now();
    let mut by_context = BTreeMap::<Option<u64>, Vec<cookie_core::CookieRecord>>::new();
    let mut diagnostics_by_context = BTreeMap::<Option<u64>, BrowserDiagnostics>::new();

    for row in rows {
        match firefox_context(&row.origin_attributes) {
            CookieContext::Partitioned(context) => {
                let diagnostics = diagnostics_by_context.entry(context).or_default();
                diagnostics.rows_read += 1;
                diagnostics.partitioned += 1;
                continue;
            }
            CookieContext::Unsupported => continue,
            CookieContext::Supported(context) => {
                let diagnostics = diagnostics_by_context.entry(context).or_default();
                diagnostics.rows_read += 1;
                let expires = firefox_expiration(row.expires, version);
                if expires.is_some_and(|expiry| expiry <= now) {
                    diagnostics.expired += 1;
                    continue;
                }
                let Ok(value) = std::str::from_utf8(&row.value) else {
                    continue;
                };
                by_context
                    .entry(context)
                    .or_default()
                    .push(cookie_core::CookieRecord {
                        name: row.name,
                        value: SecretString::new(value.to_owned()),
                        host_only: !row.host.starts_with('.'),
                        domain: row.host.trim_start_matches('.').to_ascii_lowercase(),
                        path: row.path,
                        secure: row.secure,
                        http_only: row.http_only,
                        expires,
                    });
            }
        }
    }

    let mut candidates = Vec::new();
    for (context, diagnostics) in diagnostics_by_context {
        let records = by_context.remove(&context).unwrap_or_default();
        let (profile_id, label) = match context {
            None => (profile.profile_id.clone(), profile.label.clone()),
            Some(id) => {
                let digest = sha256_hex("browser_container", &[&id.to_le_bytes()]);
                let name = containers
                    .get(&id)
                    .map(String::as_str)
                    .unwrap_or("Firefox container");
                (
                    format!("{}.container.{digest}", profile.profile_id),
                    super::local_state::safe_label(
                        &format!("{} · {}", profile.label, name),
                        "Firefox container",
                    ),
                )
            }
        };
        candidates.push(CookieCandidate {
            browser: profile.browser,
            profile_id,
            label,
            records,
            diagnostics,
        });
    }
    Ok(candidates)
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
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let mut statement = connection.prepare(&format!(
        "SELECT host, name, path, expiry, isSecure, isHttpOnly, value, originAttributes FROM moz_cookies WHERE {}{}",
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
        let value = secret_bytes(row.get(6)?);
        let origin_attributes: String = row.get(7)?;
        Ok(RawCookie {
            host,
            name,
            path,
            expires,
            secure,
            http_only,
            value,
            origin_attributes,
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

#[derive(Clone, Copy)]
enum CookieContext {
    Supported(Option<u64>),
    Partitioned(Option<u64>),
    Unsupported,
}

fn firefox_context(attributes: &str) -> CookieContext {
    if attributes.is_empty() {
        return CookieContext::Supported(None);
    }
    let mut user_context = None;
    let mut partitioned = false;
    for attribute in attributes.trim_start_matches('^').split('^') {
        let Some((key, value)) = attribute.split_once('=') else {
            return CookieContext::Unsupported;
        };
        if key == "partitionKey" {
            partitioned = true;
            continue;
        }
        if key != "userContextId" || user_context.is_some() {
            return CookieContext::Unsupported;
        }
        let Ok(id) = value.parse::<u64>() else {
            return CookieContext::Unsupported;
        };
        if id == 0 {
            return CookieContext::Unsupported;
        }
        user_context = Some(id);
    }
    if partitioned {
        return CookieContext::Partitioned(user_context);
    }
    match user_context {
        Some(id) => CookieContext::Supported(Some(id)),
        None => CookieContext::Unsupported,
    }
}

fn firefox_expiration(value: i64, schema_version: i64) -> Option<Timestamp> {
    if value <= 0 {
        return None;
    }
    let seconds = if schema_version >= 16 {
        value / 1_000
    } else {
        value
    };
    Timestamp::from_second(seconds).ok()
}

fn read_containers(profile: &Path) -> BTreeMap<u64, String> {
    let Some(bytes) = read_optional_owned_file(&profile.join("containers.json"), 2 * 1024 * 1024)
        .ok()
        .flatten()
    else {
        return BTreeMap::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return BTreeMap::new();
    };
    value
        .get("identities")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|identity| {
            let id = identity.get("userContextId")?.as_u64()?;
            let name = identity.get("name")?.as_str()?;
            Some((
                id,
                super::local_state::safe_label(name, "Firefox container"),
            ))
        })
        .collect()
}

fn domain_predicate(count: usize) -> String {
    let conditions = (0..count)
        .map(|_| "(host = ? OR host = ? OR host LIKE ? ESCAPE '\\')")
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
