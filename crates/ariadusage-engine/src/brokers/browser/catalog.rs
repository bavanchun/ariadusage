use std::path::PathBuf;

use super::BrowserPaths;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Browser {
    Chrome,
    Edge,
    Brave,
    Chromium,
    Vivaldi,
    Firefox,
    Opera,
}

pub const DEFAULT_IMPORT_ORDER: [Browser; 7] = [
    Browser::Chrome,
    Browser::Edge,
    Browser::Brave,
    Browser::Chromium,
    Browser::Vivaldi,
    Browser::Firefox,
    Browser::Opera,
];

#[derive(Clone, Debug)]
pub(super) struct ProfileRoot {
    pub path: PathBuf,
    pub sandboxed: bool,
    pub unverified: bool,
}

impl Browser {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Edge => "Edge",
            Self::Brave => "Brave",
            Self::Chromium => "Chromium",
            Self::Vivaldi => "Vivaldi",
            Self::Firefox => "Firefox",
            Self::Opera => "Opera",
        }
    }

    pub fn is_chromium(self) -> bool {
        !matches!(self, Self::Firefox)
    }

    pub fn safe_storage_attribute(self) -> Option<&'static str> {
        match self {
            Self::Chrome | Self::Vivaldi => Some("chrome"),
            Self::Edge | Self::Chromium | Self::Opera => Some("chromium"),
            Self::Brave => Some("brave"),
            Self::Firefox => None,
        }
    }

    pub(super) fn roots(self, paths: &BrowserPaths) -> Vec<ProfileRoot> {
        let mut roots = Vec::new();
        let config_root = match self {
            Self::Chrome => "google-chrome",
            Self::Edge => "microsoft-edge",
            Self::Brave => "BraveSoftware/Brave-Browser",
            Self::Chromium => "chromium",
            Self::Vivaldi => "vivaldi",
            Self::Firefox => return firefox_roots(paths),
            Self::Opera => "opera",
        };
        roots.push(ProfileRoot {
            path: paths.config_home.join(config_root),
            sandboxed: false,
            unverified: matches!(self, Self::Edge | Self::Vivaldi | Self::Opera),
        });

        let flatpak = match self {
            Self::Chrome => Some(("com.google.Chrome", "google-chrome")),
            Self::Edge => Some(("com.microsoft.Edge", "microsoft-edge")),
            Self::Brave => Some(("com.brave.Browser", "BraveSoftware/Brave-Browser")),
            Self::Chromium => Some(("org.chromium.Chromium", "chromium")),
            Self::Vivaldi => Some(("com.vivaldi.Vivaldi", "vivaldi")),
            Self::Opera => Some(("com.operasoftware.Opera", "opera")),
            Self::Firefox => None,
        };
        if let Some((app_id, root)) = flatpak {
            roots.push(ProfileRoot {
                path: paths
                    .home
                    .join(".var/app")
                    .join(app_id)
                    .join("config")
                    .join(root),
                sandboxed: true,
                unverified: true,
            });
        }

        if self == Self::Chromium {
            roots.push(ProfileRoot {
                path: paths.home.join("snap/chromium/common/chromium"),
                sandboxed: true,
                unverified: true,
            });
        }
        roots
    }
}

impl std::fmt::Display for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

fn firefox_roots(paths: &BrowserPaths) -> Vec<ProfileRoot> {
    vec![
        ProfileRoot {
            path: paths.config_home.join("mozilla/firefox"),
            sandboxed: false,
            unverified: false,
        },
        ProfileRoot {
            path: paths.home.join(".mozilla/firefox"),
            sandboxed: false,
            unverified: false,
        },
        ProfileRoot {
            path: paths
                .home
                .join(".var/app/org.mozilla.firefox/config/mozilla/firefox"),
            sandboxed: true,
            unverified: true,
        },
        ProfileRoot {
            path: paths
                .home
                .join(".var/app/org.mozilla.firefox/.mozilla/firefox"),
            sandboxed: true,
            unverified: true,
        },
        ProfileRoot {
            path: paths.home.join("snap/firefox/common/.mozilla/firefox"),
            sandboxed: true,
            unverified: true,
        },
    ]
}
