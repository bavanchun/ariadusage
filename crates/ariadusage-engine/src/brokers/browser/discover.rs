use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ariadusage_core::digest::sha256_hex;
use rustix::fs::{FileType, stat};
use rustix::process::geteuid;

use crate::brokers::call::BrokerCall;
use crate::state_store::BrokerStateStore;

use super::catalog::{Browser, ProfileRoot};
use super::error::BrowserError;
use super::gate::{BrowserGate, RetryScope};
use super::{BrowserPaths, CookieCandidate, CookieQuery, SafeStorageKeyProvider};

const DISCOVERY_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Clone)]
pub(super) struct DiscoveredProfile {
    pub browser: Browser,
    pub profile_id: String,
    pub label: String,
    pub profile_path: PathBuf,
    pub database_path: PathBuf,
    pub _unverified: bool,
}

#[derive(Clone)]
struct CachedProfiles {
    loaded_at: Instant,
    profiles: Vec<DiscoveredProfile>,
}

pub(super) struct BrowserBrokerInner {
    paths: BrowserPaths,
    keys: Arc<dyn SafeStorageKeyProvider>,
    gate: BrowserGate,
    discoveries: Mutex<HashMap<Browser, CachedProfiles>>,
}

impl BrowserBrokerInner {
    pub fn new(
        paths: BrowserPaths,
        state: Arc<BrokerStateStore>,
        keys: Arc<dyn SafeStorageKeyProvider>,
    ) -> Self {
        Self {
            paths,
            gate: BrowserGate::new(Arc::clone(&state)),
            keys,
            discoveries: Mutex::new(HashMap::new()),
        }
    }

    pub async fn candidates(
        &self,
        call: &BrokerCall,
        query: CookieQuery<'_>,
    ) -> Result<Vec<CookieCandidate>, BrowserError> {
        if cfg!(feature = "test-hooks") && is_real_home(&self.paths) {
            return Err(BrowserError::Suppressed);
        }
        if call.cancel.is_cancelled() || query.domains.domains().is_empty() {
            return Err(BrowserError::NoBrowserSession);
        }

        let order = if query.browsers.is_empty() {
            &super::DEFAULT_IMPORT_ORDER[..]
        } else {
            query.browsers
        };
        let mut found = Vec::new();
        let mut saw_profile = false;
        let mut last_error = None;
        let mut retry_scope = RetryScope::new();
        let mut safe_storage_keys = HashMap::new();
        let mut chromium_access = super::chromium::ChromiumAccess::new(
            &self.gate,
            &mut retry_scope,
            &mut safe_storage_keys,
            self.keys.as_ref(),
        );
        for browser in order {
            let profiles = self.cached_profiles(*browser)?;
            saw_profile |= !profiles.is_empty();
            for profile in profiles {
                if call.cancel.is_cancelled() {
                    return if found.is_empty() {
                        Err(BrowserError::NoBrowserSession)
                    } else {
                        Ok(found)
                    };
                }
                let result = if profile.browser == Browser::Firefox {
                    super::firefox::read_profile(&profile, &self.paths, &query, call)
                        .map(|candidates| (candidates, None))
                } else {
                    super::chromium::read_profile(
                        &profile,
                        &self.paths,
                        &query,
                        call,
                        &mut chromium_access,
                    )
                    .await
                };
                match result {
                    Ok((mut candidates, error)) => {
                        if !candidates.is_empty() {
                            found.append(&mut candidates);
                        }
                        if let Some(error) = error {
                            last_error = prefer_error(last_error, error);
                        }
                    }
                    Err(error) => last_error = prefer_error(last_error, error),
                }
            }
        }
        if !found.is_empty() {
            return Ok(found);
        }
        match last_error {
            Some(error @ BrowserError::Dismissed) => Err(error),
            Some(error @ BrowserError::Locked) => Err(error),
            Some(error @ BrowserError::Unsupported) => Err(error),
            Some(error @ BrowserError::Io) => Err(error),
            Some(error @ BrowserError::Malformed) => Err(error),
            Some(error @ BrowserError::Suppressed) => Err(error),
            Some(error @ BrowserError::NoBrowserSession) => Err(error),
            None if saw_profile => Err(BrowserError::NoBrowserSession),
            None => Err(BrowserError::NoBrowserSession),
        }
    }

    fn cached_profiles(&self, browser: Browser) -> Result<Vec<DiscoveredProfile>, BrowserError> {
        let now = Instant::now();
        if let Some(cached) = self
            .discoveries
            .lock()
            .map_err(|_| BrowserError::Io)?
            .get(&browser)
            .filter(|cached| now.duration_since(cached.loaded_at) < DISCOVERY_TTL)
        {
            return Ok(cached.profiles.clone());
        }
        let profiles = discover_profiles(&self.paths, browser);
        self.discoveries
            .lock()
            .map_err(|_| BrowserError::Io)?
            .insert(
                browser,
                CachedProfiles {
                    loaded_at: now,
                    profiles: profiles.clone(),
                },
            );
        Ok(profiles)
    }
}

fn prefer_error(current: Option<BrowserError>, candidate: BrowserError) -> Option<BrowserError> {
    let priority = |error| match error {
        BrowserError::Dismissed => 6,
        BrowserError::Locked => 5,
        BrowserError::Unsupported => 4,
        BrowserError::Malformed => 3,
        BrowserError::Io => 2,
        BrowserError::Suppressed => 1,
        BrowserError::NoBrowserSession => 0,
    };
    current
        .filter(|current| priority(*current) > priority(candidate))
        .or(Some(candidate))
}

pub(super) fn discover_profiles(paths: &BrowserPaths, browser: Browser) -> Vec<DiscoveredProfile> {
    if browser == Browser::Firefox {
        return discover_firefox(paths);
    }
    let mut profiles = Vec::new();
    for root in browser.roots(paths) {
        let Some(root_path) = canonical_directory(&root.path) else {
            continue;
        };
        let mut paths = Vec::new();
        if has_cookie_database(&root_path, &root_path).is_some() {
            paths.push(root_path.clone());
        }
        if let Ok(entries) = std::fs::read_dir(&root_path) {
            let mut children = entries.flatten().collect::<Vec<_>>();
            children.sort_by_key(|entry| entry.file_name());
            for entry in children {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !is_profile_directory(&name) {
                    continue;
                }
                let Some(profile_path) = canonical_directory(&entry.path()) else {
                    continue;
                };
                if profile_path.starts_with(&root_path)
                    && has_cookie_database(&profile_path, &profile_path).is_some()
                {
                    paths.push(profile_path);
                }
            }
        }
        for profile_path in paths {
            let profile_name = profile_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let name = super::local_state::profile_name(&root_path, profile_name)
                .unwrap_or_else(|| super::local_state::safe_label("", "Browser profile"));
            let label = super::local_state::safe_label(
                &format!("{} {name}", browser.display_name()),
                "Browser profile",
            );
            if let Some(database_path) = has_cookie_database(&profile_path, &profile_path) {
                profiles.push(DiscoveredProfile {
                    browser,
                    profile_id: opaque_profile_id(&profile_path),
                    label,
                    profile_path,
                    database_path,
                    _unverified: root.unverified,
                });
            }
        }
    }
    deduplicate_profiles(&mut profiles);
    profiles
}

fn discover_firefox(paths: &BrowserPaths) -> Vec<DiscoveredProfile> {
    let mut profiles = Vec::new();
    for root in Browser::Firefox.roots(paths) {
        let Some(root_path) = canonical_directory(&root.path) else {
            continue;
        };
        let ini_path = root_path.join("profiles.ini");
        let ini = super::sqlite_copy::read_optional_owned_file(&ini_path, 1024 * 1024)
            .ok()
            .flatten();
        let installs = super::sqlite_copy::read_optional_owned_file(
            &root_path.join("installs.ini"),
            1024 * 1024,
        )
        .ok()
        .flatten();
        let entries = if let Some(ini) = ini {
            let ini_text = String::from_utf8_lossy(&ini);
            let installs_text = installs.map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
            super::profiles_ini::parse_profiles(&ini_text, installs_text.as_deref())
        } else {
            scan_firefox_profiles(&root_path)
        };
        for entry in entries {
            let Some(profile_path) =
                resolve_firefox_path(paths, &root, &root_path, &entry.path, entry.is_relative)
            else {
                continue;
            };
            let Some(database_path) = has_cookie_database(&profile_path, &profile_path) else {
                continue;
            };
            let name = entry.name.as_deref().unwrap_or("Firefox profile");
            let label =
                super::local_state::safe_label(&format!("Firefox {name}"), "Firefox profile");
            profiles.push(DiscoveredProfile {
                browser: Browser::Firefox,
                profile_id: opaque_profile_id(&profile_path),
                label,
                profile_path,
                database_path,
                _unverified: root.unverified,
            });
        }
    }
    deduplicate_profiles(&mut profiles);
    profiles
}

fn scan_firefox_profiles(root: &Path) -> Vec<super::profiles_ini::FirefoxProfileEntry> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut profiles = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let canonical = canonical_directory(&path)?;
            if !canonical.starts_with(root) || has_cookie_database(&canonical, &canonical).is_none()
            {
                return None;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            Some(super::profiles_ini::FirefoxProfileEntry {
                section: name.clone(),
                name: Some(name.clone()),
                path: name,
                is_relative: true,
                is_default: false,
            })
        })
        .collect::<Vec<_>>();
    profiles.sort_by_key(|entry| entry.path.clone());
    profiles
}

fn resolve_firefox_path(
    paths: &BrowserPaths,
    root: &ProfileRoot,
    root_path: &Path,
    raw: &str,
    is_relative: bool,
) -> Option<PathBuf> {
    let candidate = if is_relative {
        root_path.join(raw)
    } else {
        if root.sandboxed {
            return None;
        }
        PathBuf::from(raw)
    };
    let canonical = std::fs::canonicalize(candidate).ok()?;
    if !canonical.is_dir() {
        return None;
    }
    if is_relative {
        return canonical.starts_with(root_path).then_some(canonical);
    }
    let home = std::fs::canonicalize(&paths.home).ok()?;
    if !canonical.starts_with(home) {
        return None;
    }
    let stat = stat(&canonical).ok()?;
    (FileType::from_raw_mode(stat.st_mode) == FileType::Directory
        && stat.st_uid == geteuid().as_raw())
    .then_some(canonical)
}

fn has_cookie_database(profile: &Path, containment_root: &Path) -> Option<PathBuf> {
    for relative in ["Network/Cookies", "Cookies", "cookies.sqlite"] {
        let candidate = profile.join(relative);
        let Some(parent) = candidate.parent() else {
            continue;
        };
        let Ok(canonical_parent) = std::fs::canonicalize(parent) else {
            continue;
        };
        if !canonical_parent.starts_with(containment_root) {
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(&candidate) else {
            continue;
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        return Some(canonical_parent.join(candidate.file_name()?));
    }
    None
}

fn canonical_directory(path: &Path) -> Option<PathBuf> {
    let path = std::fs::canonicalize(path).ok()?;
    path.is_dir().then_some(path)
}

fn is_profile_directory(name: &str) -> bool {
    name == "Default" || name.starts_with("Profile ") || name.starts_with("user-")
}

fn deduplicate_profiles(profiles: &mut Vec<DiscoveredProfile>) {
    let mut seen = HashSet::new();
    profiles.retain(|profile| seen.insert(profile.profile_id.clone()));
}

fn opaque_profile_id(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    sha256_hex("browser_profile", &[path.as_os_str().as_bytes()])
}

fn is_real_home(paths: &BrowserPaths) -> bool {
    #[cfg(feature = "test-hooks")]
    {
        let Some(real_home) = paths.test_process_home.as_deref() else {
            return true;
        };
        match (
            std::fs::canonicalize(&paths.home),
            std::fs::canonicalize(real_home),
        ) {
            (Ok(home), Ok(real_home)) => home == real_home,
            _ => true,
        }
    }
    #[cfg(not(feature = "test-hooks"))]
    {
        let _ = paths;
        false
    }
}
