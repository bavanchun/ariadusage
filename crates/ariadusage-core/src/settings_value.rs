// Ported from CodexBar Sources/CodexBarCore/Config/SettingsValue.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

pub enum SettingsValue {}

impl SettingsValue {
    /// Cleans and sanitizes a configuration or environment value.
    /// Trims leading and trailing whitespace, strips one matching pair of outer quotes
    /// (`"` or `'`), trims whitespace again, and returns `None` if empty.
    ///
    /// Single-character quote inputs (e.g. `"\""` or `"'"` ) safely produce `None`.
    pub fn cleaned(raw: Option<&str>) -> Option<String> {
        let raw = raw?;
        let mut s = raw.trim();
        if s.is_empty() {
            return None;
        }

        if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
            let mut chars = s.chars();
            chars.next();
            chars.next_back();
            s = chars.as_str();
        }

        s = s.trim();
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    }
}
