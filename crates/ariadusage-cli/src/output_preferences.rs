// Ported from CodexBar Sources/CodexBarCLI/CLIOutputPreferences.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Output format and ANSI color preference resolution for CLI commands.

use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;

use crate::terminal::CliTerminalCapabilities;

/// Standard CLI output format (`OutputFormat` in `CLIOptions.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}

impl OutputFormat {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            _ => None,
        }
    }

    pub fn from_argument(raw: &str) -> Option<Self> {
        Self::parse(raw)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
        }
    }
}

/// Parsed command-line options/flags/positionals (`ParsedValues` in Commander).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedValues {
    pub positional: Vec<String>,
    pub options: HashMap<String, Vec<String>>,
    pub flags: HashSet<String>,
}

/// Resolved format and TOON flag (`ResolvedOutputFormat` in `CLIOutputPreferences.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedOutputFormat {
    pub format: OutputFormat,
    pub toon_requested: bool,
}

/// Output format and ANSI color preference helper (`CLIOutputPreferences` in `CLIOutputPreferences.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CLIOutputPreferences {
    pub format: OutputFormat,
    pub json_only: bool,
    pub pretty: bool,
    pub toon_requested: bool,
}

/// Alias for [`CLIOutputPreferences`].
pub type CliOutputPreferences = CLIOutputPreferences;

impl CLIOutputPreferences {
    #[must_use]
    pub const fn new(format: OutputFormat, json_only: bool, pretty: bool) -> Self {
        Self {
            format,
            json_only,
            pretty,
            toon_requested: false,
        }
    }

    #[must_use]
    pub const fn with_toon(
        format: OutputFormat,
        json_only: bool,
        pretty: bool,
        toon_requested: bool,
    ) -> Self {
        Self {
            format,
            json_only,
            pretty,
            toon_requested,
        }
    }

    #[must_use]
    pub fn from_parsed(options: &HashMap<String, Vec<String>>, flags: &HashSet<String>) -> Self {
        let values = ParsedValues {
            positional: Vec::new(),
            options: options.clone(),
            flags: flags.clone(),
        };
        Self::from_values(&values, true)
    }

    #[must_use]
    pub const fn uses_json_output(&self) -> bool {
        self.json_only || matches!(self.format, OutputFormat::Json)
    }

    #[must_use]
    pub fn from_values(values: &ParsedValues, allows_toon: bool) -> Self {
        let json_only = values.flags.contains("jsonOnly");
        let pretty = values.flags.contains("pretty");
        let resolved = Self::resolve_output_format_from_values(values, allows_toon);
        Self {
            format: resolved.format,
            json_only,
            pretty,
            toon_requested: resolved.toon_requested,
        }
    }

    #[must_use]
    pub fn from_argv(argv: &[String]) -> Self {
        let mut json_only = false;
        let mut pretty = false;
        let mut last_explicit_format: Option<String> = None;
        let mut json_shortcut = false;

        let mut index = 0;
        while index < argv.len() {
            let arg = &argv[index];
            match arg.as_str() {
                "--json-only" => {
                    json_only = true;
                    json_shortcut = true;
                }
                "--json" => {
                    json_shortcut = true;
                }
                "--pretty" => {
                    pretty = true;
                }
                "--format" => {
                    let next = index + 1;
                    if next < argv.len() {
                        last_explicit_format = Some(argv[next].clone());
                        index += 1;
                    }
                }
                _ => {
                    if let Some(rest) = arg.strip_prefix("--format=")
                        && !rest.is_empty()
                    {
                        last_explicit_format = Some(rest.to_string());
                    }
                }
            }
            index += 1;
        }

        let resolved = Self::resolve_output_format(
            last_explicit_format.as_deref(),
            json_shortcut || json_only,
            Self::command_supports_toon(argv),
        );
        Self {
            format: resolved.format,
            json_only,
            pretty,
            toon_requested: resolved.toon_requested,
        }
    }

    #[must_use]
    pub fn command_supports_toon(argv: &[String]) -> bool {
        let Some(first) = argv.first() else {
            return true;
        };
        if first.starts_with('-') {
            return true;
        }
        first == "usage"
    }

    #[must_use]
    pub fn resolve_output_format_from_values(
        values: &ParsedValues,
        allows_toon: bool,
    ) -> ResolvedOutputFormat {
        let json_shortcut = values.flags.contains("jsonShortcut")
            || values.flags.contains("json")
            || values.flags.contains("jsonOnly");
        let last_explicit = values
            .options
            .get("format")
            .and_then(|v| v.last())
            .map(String::as_str);
        Self::resolve_output_format(last_explicit, json_shortcut, allows_toon)
    }

    #[must_use]
    pub fn resolve_output_format(
        last_explicit_format: Option<&str>,
        json_shortcut: bool,
        allows_toon: bool,
    ) -> ResolvedOutputFormat {
        if let Some(raw) = last_explicit_format {
            if allows_toon && raw.trim().eq_ignore_ascii_case("toon") {
                return ResolvedOutputFormat {
                    format: OutputFormat::Json,
                    toon_requested: true,
                };
            }
            if let Some(parsed) = OutputFormat::parse(raw) {
                return ResolvedOutputFormat {
                    format: parsed,
                    toon_requested: false,
                };
            }
        }
        if json_shortcut {
            return ResolvedOutputFormat {
                format: OutputFormat::Json,
                toon_requested: false,
            };
        }
        ResolvedOutputFormat {
            format: OutputFormat::Text,
            toon_requested: false,
        }
    }

    pub const fn resolve_format(format: Option<OutputFormat>, json_shortcut: bool) -> OutputFormat {
        if let Some(format) = format {
            return format;
        }
        if json_shortcut {
            OutputFormat::Json
        } else {
            OutputFormat::Text
        }
    }

    pub const fn should_use_color(no_color: bool, format: OutputFormat, is_tty: bool) -> bool {
        if no_color || !matches!(format, OutputFormat::Text) {
            return false;
        }
        is_tty
    }

    pub fn should_use_color_stdout(no_color: bool, format: OutputFormat) -> bool {
        Self::should_use_color(no_color, format, std::io::stdout().is_terminal())
    }
}

#[must_use]
pub fn resolve_usage_output_preferences(values: &ParsedValues) -> CLIOutputPreferences {
    CLIOutputPreferences::from_values(values, true)
}

#[must_use]
pub const fn should_use_color(
    no_color: bool,
    format: OutputFormat,
    terminal: CliTerminalCapabilities,
) -> bool {
    if no_color || !matches!(format, OutputFormat::Text) {
        return false;
    }
    terminal.supports_color
}
