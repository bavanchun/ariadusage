// Ported from CodexBar Sources/CodexBarCore/ProviderHTTPClient.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use reqwest::Url;
use reqwest::redirect::{Attempt, Policy};

/// Checks a redirect against the initial HTTPS origin, rather than the prior hop.
pub fn same_origin_https(original: Option<&Url>, target: &Url) -> bool {
    let Some(original) = original else {
        return false;
    };

    original.scheme().eq_ignore_ascii_case("https")
        && target.scheme().eq_ignore_ascii_case("https")
        && target.username().is_empty()
        && target.password().is_none()
        && original
            .host_str()
            .zip(target.host_str())
            .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
        && original.port_or_known_default() == target.port_or_known_default()
}

pub(crate) fn policy() -> Policy {
    Policy::custom(|attempt: Attempt<'_>| {
        if attempt.previous().len() > 10 {
            return attempt.stop();
        }

        match same_origin_https(attempt.previous().first(), attempt.url()) {
            true => attempt.follow(),
            false => attempt.stop(),
        }
    })
}
