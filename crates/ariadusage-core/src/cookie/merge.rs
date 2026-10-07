// Ported from CodexBar Sources/CodexBarCore/BrowserCookieProfiles.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;
use std::collections::{BTreeMap, HashMap};

/// The kind of cookie store within a browser profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CookieStoreKind {
    Network = 0,
    Primary = 1,
}

/// A single cookie record imported from a browser store.
#[derive(Clone, PartialEq, Eq)]
pub struct CookieRecord {
    pub name: String,
    pub value: SecretString,
    pub domain: String,
    pub host_only: bool,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires: Option<Timestamp>,
}

impl CookieRecord {
    pub fn host(&self) -> &str {
        &self.domain
    }
}

impl std::fmt::Debug for CookieRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CookieRecord")
            .field("name", &self.name)
            .field("value", &"[redacted]")
            .field("domain", &self.domain)
            .field("host_only", &self.host_only)
            .field("path", &self.path)
            .field("secure", &self.secure)
            .field("http_only", &self.http_only)
            .field("expires", &self.expires)
            .finish()
    }
}

/// Cookie records associated with one store of a profile.
#[derive(Clone, Debug)]
pub struct CookieStoreRecords {
    pub profile_id: String,
    pub store_label: String,
    pub store_kind: CookieStoreKind,
    pub records: Vec<CookieRecord>,
}

/// Merged cookie profile containing deduplicated records across stores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedProfile {
    pub profile_id: String,
    pub label: String,
    pub records: Vec<CookieRecord>,
}

/// Merges primary and network stores within each browser profile before session validation.
pub fn merge_profiles(sources: impl IntoIterator<Item = CookieStoreRecords>) -> Vec<MergedProfile> {
    let mut groups: HashMap<String, Vec<CookieStoreRecords>> = HashMap::new();
    for source in sources {
        groups
            .entry(source.profile_id.clone())
            .or_default()
            .push(source);
    }

    let mut profiles: Vec<MergedProfile> = groups
        .into_iter()
        .map(|(profile_id, store_sources)| {
            let label = merged_label(&store_sources);
            let records = merge_records(&store_sources);
            MergedProfile {
                profile_id,
                label,
                records,
            }
        })
        .collect();

    profiles.sort_by(|a, b| a.label.cmp(&b.label));
    profiles
}

fn merged_label(sources: &[CookieStoreRecords]) -> String {
    let min_label = sources
        .iter()
        .map(|s| s.store_label.as_str())
        .min()
        .unwrap_or("Unknown");

    const NETWORK_SUFFIX: &str = " (Network)";
    if let Some(stripped) = min_label.strip_suffix(NETWORK_SUFFIX) {
        stripped.to_string()
    } else {
        min_label.to_string()
    }
}

fn merge_records(sources: &[CookieStoreRecords]) -> Vec<CookieRecord> {
    let mut sorted_sources: Vec<&CookieStoreRecords> = sources.iter().collect();
    sorted_sources.sort_by_key(|s| s.store_kind);

    let mut merged_by_key: BTreeMap<String, CookieRecord> = BTreeMap::new();
    for source in sorted_sources {
        for record in &source.records {
            let key = format!(
                "{}|{}|{}|{}",
                record.name, record.domain, record.path, record.host_only
            );
            if let Some(existing) = merged_by_key.get(&key) {
                if should_replace(existing, record) {
                    merged_by_key.insert(key, record.clone());
                }
            } else {
                merged_by_key.insert(key, record.clone());
            }
        }
    }
    merged_by_key.into_values().collect()
}

fn should_replace(existing: &CookieRecord, candidate: &CookieRecord) -> bool {
    match (existing.expires, candidate.expires) {
        (Some(lhs), Some(rhs)) => rhs > lhs,
        (None, Some(_)) => true,
        (Some(_), None) => false,
        (None, None) => false,
    }
}
