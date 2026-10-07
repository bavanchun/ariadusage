// Ported from CodexBar Sources/CodexBarCore/ProviderEndpointOverrideValidator.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::net::{Ipv4Addr, Ipv6Addr};

use reqwest::Url;

/// Policy applied to a provider-supplied endpoint override.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum OverridePolicy {
    #[default]
    HttpsOnly,
    ProviderOwnedOnly {
        allowed_hosts: Vec<String>,
        allowed_domain_suffixes: Vec<String>,
    },
    AllowLoopbackHttp,
    AllowPrivateNetworkHttp,
}

/// Validates an endpoint override and returns its normalized URL.
pub fn validate_override(input: &str, policy: &OverridePolicy) -> Result<Url, OverrideError> {
    let candidate = if has_explicit_url_scheme(input) {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    let url = Url::parse(&candidate).map_err(|_| OverrideError::Malformed)?;

    if !url.username().is_empty() || url.password().is_some() {
        return Err(OverrideError::UserInfo);
    }

    let host = url.host_str().ok_or(OverrideError::MissingHost)?;
    let raw_host = raw_authority_host(&candidate).ok_or(OverrideError::Malformed)?;
    if contains_encoded_delimiter(raw_host) {
        return Err(OverrideError::EncodedDelimiter);
    }

    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    validate_host(host)?;

    let host_policy = match policy {
        OverridePolicy::ProviderOwnedOnly {
            allowed_hosts,
            allowed_domain_suffixes,
        } => {
            let host = host.to_ascii_lowercase();
            let allowed_host = allowed_hosts
                .iter()
                .any(|allowed| host == allowed.to_ascii_lowercase());
            let allowed_suffix = allowed_domain_suffixes.iter().any(|suffix| {
                let suffix = suffix.to_ascii_lowercase();
                host == suffix || host.ends_with(&format!(".{suffix}"))
            });
            if !allowed_host && !allowed_suffix {
                return Err(OverrideError::HostNotAllowed);
            }
            true
        }
        _ => false,
    };

    let scheme = url.scheme().to_ascii_lowercase();
    match scheme.as_str() {
        "https" => {}
        "http" if !host_policy && permits_http(host, policy) => {}
        "http" => return Err(OverrideError::HttpNotAllowed),
        _ => return Err(OverrideError::UnsupportedScheme),
    }

    Ok(url)
}

pub fn is_loopback_host(host: &str) -> bool {
    let host = unbracket(host);
    if host.eq_ignore_ascii_case("localhost") || host == "::1" {
        return true;
    }

    host.parse::<Ipv4Addr>()
        .is_ok_and(|address| address.octets()[0] == 127)
}

pub fn is_private_network_host(host: &str) -> bool {
    if is_loopback_host(host) {
        return true;
    }

    let hostname = host.strip_suffix('.').unwrap_or(host);
    if hostname.ends_with(".local") && hostname.len() > ".local".len() {
        return true;
    }

    if let Ok(address) = hostname.parse::<Ipv4Addr>() {
        let [first, second, _, _] = address.octets();
        return first == 10
            || (first == 172 && (16..=31).contains(&second))
            || (first == 192 && second == 168)
            || (first == 169 && second == 254);
    }

    hostname.parse::<Ipv6Addr>().is_ok_and(|address| {
        let first = address.segments()[0];
        first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
    })
}

fn permits_http(host: &str, policy: &OverridePolicy) -> bool {
    match policy {
        OverridePolicy::AllowLoopbackHttp => is_loopback_host(host),
        OverridePolicy::AllowPrivateNetworkHttp => is_private_network_host(host),
        OverridePolicy::HttpsOnly | OverridePolicy::ProviderOwnedOnly { .. } => false,
    }
}

fn validate_host(host: &str) -> Result<(), OverrideError> {
    if host.is_empty()
        || host.contains('%')
        || host
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(OverrideError::InvalidHost);
    }

    if host.contains(':') {
        return host
            .parse::<Ipv6Addr>()
            .map(|_| ())
            .map_err(|_| OverrideError::InvalidHost);
    }

    let hostname = host.strip_suffix('.').unwrap_or(host);
    if hostname.is_empty() || hostname.len() > 253 {
        return Err(OverrideError::InvalidHost);
    }

    for label in hostname.split('.') {
        let bytes = label.as_bytes();
        if bytes.is_empty()
            || bytes.len() > 63
            || !bytes[0].is_ascii_alphanumeric()
            || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
            || !bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        {
            return Err(OverrideError::InvalidHost);
        }
    }

    Ok(())
}

fn raw_authority_host(url: &str) -> Option<&str> {
    let scheme_end = url.find("://")? + 3;
    let authority = url[scheme_end..]
        .split(['/', '?', '#'])
        .next()?
        .rsplit('@')
        .next()?;

    if authority.starts_with('[') {
        let close = authority.find(']')?;
        Some(&authority[1..close])
    } else {
        Some(authority.split(':').next()?)
    }
}

fn contains_encoded_delimiter(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    ["%2f", "%5c", "%3f", "%23", "%40", "%3a"]
        .iter()
        .any(|delimiter| host.contains(delimiter))
}

fn has_explicit_url_scheme(raw: &str) -> bool {
    let Some(colon) = raw.find(':') else {
        return false;
    };
    if raw[colon..].starts_with("://") {
        return true;
    }

    if raw
        .find(['/', '?', '#'])
        .is_some_and(|authority_end| colon > authority_end)
    {
        return false;
    }

    let after_colon = colon + 1;
    let suffix_end = raw[after_colon..]
        .find(['/', '?', '#'])
        .map_or(raw.len(), |index| after_colon + index);
    let suffix = &raw[after_colon..suffix_end];
    if !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }

    let scheme = &raw[..colon];
    let mut bytes = scheme.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

fn unbracket(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OverrideError {
    #[error("endpoint override is malformed")]
    Malformed,
    #[error("endpoint override has no host")]
    MissingHost,
    #[error("endpoint override contains user information")]
    UserInfo,
    #[error("endpoint override host is invalid")]
    InvalidHost,
    #[error("endpoint override contains an encoded host delimiter")]
    EncodedDelimiter,
    #[error("endpoint override host is not provider-owned")]
    HostNotAllowed,
    #[error("endpoint override uses HTTP where HTTPS is required")]
    HttpNotAllowed,
    #[error("endpoint override uses an unsupported scheme")]
    UnsupportedScheme,
}
