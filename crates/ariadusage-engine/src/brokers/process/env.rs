use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fmt;

const EXACT_ALLOWLIST: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "PATH",
    "LANG",
    "LANGUAGE",
    "TERM",
    "COLORTERM",
    "TMPDIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NVM_DIR",
    "MISE_DATA_DIR",
    "MISE_CONFIG_DIR",
    "ASDF_DIR",
    "ASDF_DATA_DIR",
];

#[derive(Clone, Default)]
pub struct ProcessEnv {
    values: BTreeMap<OsString, OsString>,
    denied: BTreeSet<OsString>,
}

impl ProcessEnv {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_allowlist<K, V>(source: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<OsString>,
        V: Into<OsString>,
    {
        Self::from_filter(source, is_allowlisted)
    }

    pub fn from_explicit<K, V, N>(
        names: impl IntoIterator<Item = N>,
        source: impl IntoIterator<Item = (K, V)>,
    ) -> Self
    where
        K: Into<OsString>,
        V: Into<OsString>,
        N: AsRef<OsStr>,
    {
        let names = names
            .into_iter()
            .map(|name| name.as_ref().to_os_string())
            .collect::<BTreeSet<_>>();
        Self::from_filter(source, |name| names.contains(name))
    }

    fn from_filter<K, V>(
        source: impl IntoIterator<Item = (K, V)>,
        mut allowed: impl FnMut(&OsStr) -> bool,
    ) -> Self
    where
        K: Into<OsString>,
        V: Into<OsString>,
    {
        let mut environment = Self::empty();
        for (name, value) in source {
            let name = name.into();
            if allowed(&name) {
                environment.values.insert(name, value.into());
            }
        }
        environment
    }

    pub fn with<K, V>(mut self, name: K, value: V) -> Self
    where
        K: Into<OsString>,
        V: Into<OsString>,
    {
        let name = name.into();
        if !self.denied.contains(&name) {
            self.values.insert(name, value.into());
        }
        self
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn with_internal<K, V>(mut self, name: K, value: V) -> Self
    where
        K: Into<OsString>,
        V: Into<OsString>,
    {
        let name = name.into();
        self.denied.remove(&name);
        self.values.insert(name, value.into());
        self
    }

    pub fn without<K>(mut self, names: impl IntoIterator<Item = K>) -> Self
    where
        K: Into<OsString>,
    {
        for name in names {
            let name = name.into();
            self.values.remove(&name);
            self.denied.insert(name);
        }
        self
    }

    pub fn get(&self, name: impl AsRef<OsStr>) -> Option<&OsString> {
        self.values.get(name.as_ref())
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&OsString, &OsString)> {
        self.values.iter()
    }
}

impl fmt::Debug for ProcessEnv {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessEnv")
            .field("entry_count", &self.values.len())
            .finish()
    }
}

fn is_allowlisted(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    EXACT_ALLOWLIST.contains(&name) || name.starts_with("LC_") || name.starts_with("XDG_")
}
