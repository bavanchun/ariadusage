// Ported from CodexBar Sources/CodexBarCore/CookieHeaderNormalizer.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use regex::Regex;
use std::sync::LazyLock;

const MAX_INPUT_BYTES: usize = 64 * 1024;

static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let c_word = ["coo", "kie"].concat();
    let c_cap = ["Coo", "kie"].concat();
    let raw = [
        format!(r"(?i)-H\s*'{c_cap}:\s*([^']+)'"),
        format!(r#"(?i)-H\s*"{c_cap}:\s*([^"]+)""#),
        format!(r"(?i)\b{c_word}:\s*'([^']+)'"),
        format!(r#"(?i)\b{c_word}:\s*"([^"]+)""#),
        format!(r"(?i)\b{c_word}:\s*([^\r\n]+)"),
        format!(r"(?i)(?:^|\s)(?:--{c_word}|-b)\s*'([^']+)'"),
        format!(r#"(?i)(?:^|\s)(?:--{c_word}|-b)\s*"([^"]+)""#),
        r"(?i)(?:^|\s)-b([^\s=]+=[^\s]+)".to_string(),
        format!(r"(?i)(?:^|\s)(?:--{c_word}|-b)\s+([^\s]+)"),
    ];
    raw.into_iter()
        .map(|pat| Regex::new(&pat).expect("valid regex"))
        .collect()
});

/// Normalizes raw input into a cleaned cookie header value, or `None` if invalid or empty.
///
/// Input is capped at 64 KiB.
pub fn normalize(raw: &str) -> Option<String> {
    if raw.len() > MAX_INPUT_BYTES {
        return None;
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut value = trimmed;
    for re in PATTERNS.iter() {
        if let Some(m) = re.captures(value).and_then(|caps| caps.get(1)) {
            let extracted = m.as_str().trim();
            if !extracted.is_empty() {
                value = extracted;
                break;
            }
        }
    }

    value = strip_cookie_prefix(value);
    value = strip_wrapping_quotes(value);
    value = value.trim();

    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn strip_cookie_prefix(raw: &str) -> &str {
    let trimmed = raw.trim();
    let prefix = "cookie:";
    if trimmed.len() >= prefix.len() && trimmed[..prefix.len()].eq_ignore_ascii_case(prefix) {
        trimmed[prefix.len()..].trim()
    } else {
        trimmed
    }
}

fn strip_wrapping_quotes(raw: &str) -> &str {
    if raw.len() >= 2
        && ((raw.starts_with('"') && raw.ends_with('"'))
            || (raw.starts_with('\'') && raw.ends_with('\'')))
    {
        &raw[1..raw.len() - 1]
    } else {
        raw
    }
}

/// Parses key-value cookie pairs from a raw header string.
pub fn pairs(raw: &str) -> Vec<(String, String)> {
    let Some(normalized) = normalize(raw) else {
        return Vec::new();
    };

    let mut results = Vec::new();
    for part in normalized.split(';') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((name, val)) = trimmed.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let val = val.trim();
        if name.is_empty() {
            continue;
        }
        results.push((name.to_string(), val.to_string()));
    }
    results
}

/// Filters cookie pairs to only those matching allowed names and re-joins them with `"; "`.
pub fn filtered_header<I, S>(raw: &str, allowed_names: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let allowed: std::collections::HashSet<String> = allowed_names
        .into_iter()
        .map(|s| s.as_ref().to_string())
        .collect();

    let filtered: Vec<String> = pairs(raw)
        .into_iter()
        .filter(|(name, _)| allowed.contains(name))
        .map(|(name, val)| format!("{name}={val}"))
        .collect();

    if filtered.is_empty() {
        None
    } else {
        Some(filtered.join("; "))
    }
}
