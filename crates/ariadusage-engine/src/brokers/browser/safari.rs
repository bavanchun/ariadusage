use ariadusage_core::cookie as cookie_core;
use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;
use zeroize::Zeroizing;

use crate::brokers::call::BrokerCall;

use super::discover::DiscoveredProfile;
use super::error::BrowserError;
use super::sqlite_copy::read_optional_owned_file_allowing_permission_denied;
use super::{BrowserDiagnostics, CookieCandidate, CookieQuery};

const MAX_BINARY_COOKIES_SIZE: usize = 16 * 1024 * 1024;
const MAC_EPOCH_UNIX_SECONDS: f64 = 978_307_200.0;
const BINARY_COOKIES_MAGIC: &[u8; 4] = b"cook";
const PAGE_HEADER_MAGIC: [u8; 4] = [0x00, 0x00, 0x01, 0x00];
const MIN_COOKIE_RECORD_LEN: usize = 48;

struct RawSafariCookie {
    host: String,
    name: String,
    path: String,
    value: Zeroizing<String>,
    expires: Option<Timestamp>,
    secure: bool,
    http_only: bool,
}

pub(super) fn read_profile(
    profile: &DiscoveredProfile,
    query: &CookieQuery<'_>,
    call: &BrokerCall,
) -> Result<Vec<CookieCandidate>, BrowserError> {
    if call.cancel.is_cancelled() {
        return Err(BrowserError::NoBrowserSession);
    }

    // Safari's `Cookies.binarycookies` may be protected by macOS TCC (Full Disk Access).
    // When the file is missing or unreadable due to `PermissionDenied`, skip Safari gracefully
    // without surfacing a fatal error or recording a denial cooldown.
    let Some(raw) = read_optional_owned_file_allowing_permission_denied(
        &profile.database_path,
        MAX_BINARY_COOKIES_SIZE,
    )?
    else {
        return Ok(Vec::new());
    };
    let bytes = Zeroizing::new(raw);
    let parsed = parse_binary_cookies(&bytes)?;

    let now = Timestamp::now();
    let mut diagnostics = BrowserDiagnostics::default();
    let mut records = Vec::new();

    for mut cookie in parsed {
        if !query.domains.matches(&cookie.host)
            || !query
                .names
                .is_none_or(|names| names.contains(&cookie.name.as_str()))
        {
            continue;
        }
        diagnostics.rows_read += 1;
        if cookie.expires.is_some_and(|expiry| expiry <= now) {
            diagnostics.expired += 1;
            continue;
        }
        let value = std::mem::take(&mut *cookie.value);
        records.push(cookie_core::CookieRecord {
            name: cookie.name,
            value: SecretString::new(value),
            host_only: !cookie.host.starts_with('.'),
            domain: cookie.host.trim_start_matches('.').to_ascii_lowercase(),
            path: cookie.path,
            secure: cookie.secure,
            http_only: cookie.http_only,
            expires: cookie.expires,
        });
    }

    if records.is_empty() && diagnostics.rows_read == 0 {
        return Ok(Vec::new());
    }

    Ok(vec![CookieCandidate {
        browser: profile.browser,
        profile_id: profile.profile_id.clone(),
        label: profile.label.clone(),
        records,
        diagnostics,
    }])
}

fn parse_binary_cookies(bytes: &[u8]) -> Result<Vec<RawSafariCookie>, BrowserError> {
    if bytes.len() < 8 || &bytes[0..4] != BINARY_COOKIES_MAGIC {
        return Err(BrowserError::Malformed);
    }
    let num_pages = read_be_u32(bytes, 4)? as usize;
    let page_sizes_end = 8usize
        .checked_add(num_pages.checked_mul(4).ok_or(BrowserError::Malformed)?)
        .ok_or(BrowserError::Malformed)?;
    if page_sizes_end > bytes.len() {
        return Err(BrowserError::Malformed);
    }

    let mut page_sizes = Vec::with_capacity(num_pages);
    for idx in 0..num_pages {
        let size = read_be_u32(bytes, 8 + idx * 4)? as usize;
        page_sizes.push(size);
    }

    let mut cursor = page_sizes_end;
    let mut cookies = Vec::new();
    for page_size in page_sizes {
        let page_end = cursor
            .checked_add(page_size)
            .ok_or(BrowserError::Malformed)?;
        if page_end > bytes.len() {
            return Err(BrowserError::Malformed);
        }
        parse_page(&bytes[cursor..page_end], &mut cookies)?;
        cursor = page_end;
    }

    Ok(cookies)
}

fn parse_page(page: &[u8], out: &mut Vec<RawSafariCookie>) -> Result<(), BrowserError> {
    if page.len() < 8 || page[0..4] != PAGE_HEADER_MAGIC {
        return Err(BrowserError::Malformed);
    }
    let num_cookies = read_le_u32(page, 4)? as usize;
    let offsets_end = 8usize
        .checked_add(num_cookies.checked_mul(4).ok_or(BrowserError::Malformed)?)
        .ok_or(BrowserError::Malformed)?;
    if offsets_end > page.len() {
        return Err(BrowserError::Malformed);
    }

    for idx in 0..num_cookies {
        let cookie_offset = read_le_u32(page, 8 + idx * 4)? as usize;
        if cookie_offset < offsets_end {
            return Err(BrowserError::Malformed);
        }
        let cookie_size = read_le_u32(page, cookie_offset)? as usize;
        if cookie_size < MIN_COOKIE_RECORD_LEN {
            return Err(BrowserError::Malformed);
        }
        let cookie_end = cookie_offset
            .checked_add(cookie_size)
            .ok_or(BrowserError::Malformed)?;
        if cookie_end > page.len() {
            return Err(BrowserError::Malformed);
        }
        out.push(parse_cookie_record(&page[cookie_offset..cookie_end])?);
    }

    Ok(())
}

fn parse_cookie_record(record: &[u8]) -> Result<RawSafariCookie, BrowserError> {
    if record.len() < MIN_COOKIE_RECORD_LEN {
        return Err(BrowserError::Malformed);
    }
    let flags = read_le_u32(record, 8)?;
    let url_offset = read_le_u32(record, 16)? as usize;
    let name_offset = read_le_u32(record, 20)? as usize;
    let path_offset = read_le_u32(record, 24)? as usize;
    let value_offset = read_le_u32(record, 28)? as usize;
    let expiration_secs = read_le_f64(record, 40)?;

    let host = read_cstring(record, url_offset)?.to_owned();
    let name = read_cstring(record, name_offset)?.to_owned();
    let path = read_cstring(record, path_offset)?.to_owned();
    let value = Zeroizing::new(read_cstring(record, value_offset)?.to_owned());

    let expires = safari_expiration(expiration_secs);
    let secure = (flags & 0x1) != 0;
    let http_only = (flags & 0x4) != 0;

    Ok(RawSafariCookie {
        host,
        name,
        path,
        value,
        expires,
        secure,
        http_only,
    })
}

fn safari_expiration(mac_epoch_seconds: f64) -> Option<Timestamp> {
    if !mac_epoch_seconds.is_finite() || mac_epoch_seconds <= 0.0 {
        return None;
    }
    let unix_seconds = (mac_epoch_seconds + MAC_EPOCH_UNIX_SECONDS).trunc();
    if unix_seconds < (i64::MIN as f64) || unix_seconds > (i64::MAX as f64) {
        return None;
    }
    Timestamp::from_second(unix_seconds as i64).ok()
}

fn read_cstring(record: &[u8], offset: usize) -> Result<&str, BrowserError> {
    if offset < MIN_COOKIE_RECORD_LEN || offset >= record.len() {
        return Err(BrowserError::Malformed);
    }
    let tail = &record[offset..];
    let nul_pos = tail
        .iter()
        .position(|&byte| byte == 0)
        .ok_or(BrowserError::Malformed)?;
    std::str::from_utf8(&tail[..nul_pos]).map_err(|_| BrowserError::Malformed)
}

fn read_be_u32(bytes: &[u8], offset: usize) -> Result<u32, BrowserError> {
    let end = offset.checked_add(4).ok_or(BrowserError::Malformed)?;
    let slice = bytes.get(offset..end).ok_or(BrowserError::Malformed)?;
    Ok(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_le_u32(bytes: &[u8], offset: usize) -> Result<u32, BrowserError> {
    let end = offset.checked_add(4).ok_or(BrowserError::Malformed)?;
    let slice = bytes.get(offset..end).ok_or(BrowserError::Malformed)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_le_f64(bytes: &[u8], offset: usize) -> Result<f64, BrowserError> {
    let end = offset.checked_add(8).ok_or(BrowserError::Malformed)?;
    let slice = bytes.get(offset..end).ok_or(BrowserError::Malformed)?;
    Ok(f64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}
