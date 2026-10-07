use std::collections::BTreeSet;
use std::fmt;

/// A validated, lowercase DNS hostname.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostName(String);

impl HostName {
    /// Parses a DNS hostname and normalizes ASCII letters to lowercase.
    pub fn new(value: impl AsRef<str>) -> Result<Self, HostsError> {
        let value = value.as_ref();
        let normalized = value.to_ascii_lowercase();

        if normalized.is_empty()
            || normalized.len() > 253
            || !normalized.is_ascii()
            || normalized.split('.').any(|label| !valid_label(label))
        {
            return Err(HostsError::InvalidHost);
        }

        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for HostName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for HostName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && label
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && label
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// A non-empty, normalized set of hostnames declared by one provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderHosts {
    hosts: Vec<HostName>,
}

impl ProviderHosts {
    pub fn new<I, S>(hosts: I) -> Result<Self, HostsError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let hosts = hosts
            .into_iter()
            .map(HostName::new)
            .collect::<Result<BTreeSet<_>, _>>()?;

        if hosts.is_empty() {
            return Err(HostsError::Empty);
        }

        Ok(Self {
            hosts: hosts.into_iter().collect(),
        })
    }

    pub fn hosts(&self) -> &[HostName] {
        &self.hosts
    }

    /// Returns canonical HTTPS origins on port 443 for the declared hosts.
    pub fn origins(&self) -> impl Iterator<Item = String> + '_ {
        self.hosts
            .iter()
            .map(|host| format!("https://{}:443", host.as_str()))
    }
}

/// Errors returned when a provider host declaration is invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HostsError {
    #[error("provider host declarations must not be empty")]
    Empty,
    #[error("invalid provider hostname")]
    InvalidHost,
}
