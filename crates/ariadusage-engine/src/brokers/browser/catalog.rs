#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::path::PathBuf;

#[cfg(any(target_os = "linux", target_os = "macos"))]
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
    Safari,
}

#[cfg(target_os = "macos")]
pub const DEFAULT_IMPORT_ORDER: [Browser; 8] = [
    Browser::Chrome,
    Browser::Edge,
    Browser::Brave,
    Browser::Chromium,
    Browser::Vivaldi,
    Browser::Firefox,
    Browser::Opera,
    Browser::Safari,
];

#[cfg(not(target_os = "macos"))]
pub const DEFAULT_IMPORT_ORDER: [Browser; 7] = [
    Browser::Chrome,
    Browser::Edge,
    Browser::Brave,
    Browser::Chromium,
    Browser::Vivaldi,
    Browser::Firefox,
    Browser::Opera,
];

#[cfg(any(target_os = "linux", target_os = "macos"))]
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
            Self::Safari => "Safari",
        }
    }

    pub fn raw_id(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Edge => "edge",
            Self::Brave => "brave",
            Self::Chromium => "chromium",
            Self::Vivaldi => "vivaldi",
            Self::Firefox => "firefox",
            Self::Opera => "opera",
            Self::Safari => "safari",
        }
    }

    pub fn from_raw_id(id: &str) -> Option<Self> {
        match id {
            "chrome" => Some(Self::Chrome),
            "edge" => Some(Self::Edge),
            "brave" => Some(Self::Brave),
            "chromium" => Some(Self::Chromium),
            "vivaldi" => Some(Self::Vivaldi),
            "firefox" => Some(Self::Firefox),
            "opera" => Some(Self::Opera),
            "safari" => Some(Self::Safari),
            _ => None,
        }
    }

    pub fn is_chromium(self) -> bool {
        !matches!(self, Self::Firefox | Self::Safari)
    }

    pub fn safe_storage_attribute(self) -> Option<&'static str> {
        #[cfg(target_os = "macos")]
        {
            match self {
                Self::Chrome => Some("chrome"),
                Self::Edge => Some("edge"),
                Self::Brave => Some("brave"),
                Self::Chromium => Some("chromium"),
                Self::Vivaldi => Some("vivaldi"),
                Self::Firefox | Self::Safari => None,
                Self::Opera => Some("opera"),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            match self {
                Self::Chrome | Self::Vivaldi => Some("chrome"),
                Self::Edge | Self::Chromium | Self::Opera => Some("chromium"),
                Self::Brave => Some("brave"),
                Self::Firefox | Self::Safari => None,
            }
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(super) fn roots(self, paths: &BrowserPaths) -> Vec<ProfileRoot> {
        #[cfg(target_os = "macos")]
        {
            let app_support = paths.home.join("Library/Application Support");
            let rel = match self {
                Self::Chrome => "Google/Chrome",
                Self::Edge => "Microsoft Edge",
                Self::Brave => "BraveSoftware/Brave-Browser",
                Self::Chromium => "Chromium",
                Self::Vivaldi => "Vivaldi",
                Self::Firefox => return firefox_roots(paths),
                Self::Opera => "com.operasoftware.Opera",
                Self::Safari => return safari_roots(paths),
            };
            vec![ProfileRoot {
                path: app_support.join(rel),
                sandboxed: false,
                unverified: matches!(self, Self::Edge | Self::Vivaldi | Self::Opera),
            }]
        }
        #[cfg(not(target_os = "macos"))]
        {
            let mut roots = Vec::new();
            let config_root = match self {
                Self::Chrome => "google-chrome",
                Self::Edge => "microsoft-edge",
                Self::Brave => "BraveSoftware/Brave-Browser",
                Self::Chromium => "chromium",
                Self::Vivaldi => "vivaldi",
                Self::Firefox => return firefox_roots(paths),
                Self::Opera => "opera",
                Self::Safari => return Vec::new(),
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
                Self::Firefox | Self::Safari => None,
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
}

impl std::fmt::Display for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn firefox_roots(paths: &BrowserPaths) -> Vec<ProfileRoot> {
    #[cfg(target_os = "macos")]
    {
        vec![ProfileRoot {
            path: paths.home.join("Library/Application Support/Firefox"),
            sandboxed: false,
            unverified: false,
        }]
    }
    #[cfg(not(target_os = "macos"))]
    {
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
}

#[cfg(target_os = "macos")]
fn safari_roots(paths: &BrowserPaths) -> Vec<ProfileRoot> {
    vec![
        ProfileRoot {
            path: paths
                .home
                .join("Library/Containers/com.apple.Safari/Data/Library/Cookies"),
            sandboxed: true,
            unverified: false,
        },
        ProfileRoot {
            path: paths.home.join("Library/Cookies"),
            sandboxed: false,
            unverified: false,
        },
    ]
}
