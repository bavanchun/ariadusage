use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FirefoxProfileEntry {
    pub section: String,
    pub name: Option<String>,
    pub path: String,
    pub is_relative: bool,
    pub is_default: bool,
}

pub(super) fn parse_profiles(
    profiles_ini: &str,
    installs_ini: Option<&str>,
) -> Vec<FirefoxProfileEntry> {
    let profiles = parse_sections(profiles_ini);
    let install_defaults = installs_ini
        .map(parse_sections)
        .unwrap_or_default()
        .into_values()
        .filter_map(|section| section.get("Default").cloned())
        .collect::<Vec<_>>();

    let mut entries = profiles
        .into_iter()
        .filter(|(section, _)| section.starts_with("Profile"))
        .filter_map(|(section, values)| {
            let path = values.get("Path")?.trim();
            if path.is_empty() {
                return None;
            }
            let is_relative = values
                .get("IsRelative")
                .is_none_or(|value| value.trim() != "0");
            let is_default = values
                .get("Default")
                .is_some_and(|value| value.trim() == "1")
                || install_defaults
                    .iter()
                    .any(|default| default.trim() == path);
            Some(FirefoxProfileEntry {
                section,
                name: values.get("Name").map(|value| value.trim().to_owned()),
                path: path.to_owned(),
                is_relative,
                is_default,
            })
        })
        .collect::<Vec<_>>();

    entries.sort_by_key(|entry| {
        let profile_name = entry.name.as_deref().unwrap_or(&entry.path);
        let lower = profile_name.to_ascii_lowercase();
        let rank = if lower.contains("default-release") {
            0
        } else if lower == "default" || lower.contains("default-") {
            1
        } else if entry.is_default {
            2
        } else {
            3
        };
        (rank, lower, entry.section.clone())
    });
    entries
}

fn parse_sections(input: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut sections = BTreeMap::<String, BTreeMap<String, String>>::new();
    let mut current = None::<String>;
    for raw_line in input.lines() {
        let line = raw_line.trim().trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            let section = section.trim();
            if section.is_empty() {
                current = None;
            } else {
                sections.entry(section.to_owned()).or_default();
                current = Some(section.to_owned());
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Some(section) = current.as_ref() else {
            continue;
        };
        let key = key.trim();
        if !key.is_empty() {
            sections
                .entry(section.clone())
                .or_default()
                .insert(key.to_owned(), value.trim().to_owned());
        }
    }
    sections
}
