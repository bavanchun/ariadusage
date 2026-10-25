// Ported from CodexBar Sources/CodexBarCLI/CLITerminalCapabilities.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashMap;
use std::io::IsTerminal;

/// Terminal rendering backend (`CLITerminalBackend` in `CLITerminalCapabilities.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CLITerminalBackend {
    Standard,
    Truecolor,
    KittyGraphics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCapabilities {
    pub is_tty: bool,
    pub supports_color: bool,
    pub supports_truecolor: bool,
    pub supports_kitty_graphics: bool,
    pub enhanced_cards: bool,
    pub width: usize,
    pub columns: usize,
}

pub type CliTerminalCapabilities = TerminalCapabilities;

impl Default for TerminalCapabilities {
    fn default() -> Self {
        Self {
            is_tty: false,
            supports_color: false,
            supports_truecolor: false,
            supports_kitty_graphics: false,
            enhanced_cards: false,
            width: 80,
            columns: 80,
        }
    }
}

pub struct CLITerminalCapabilities;

impl CLITerminalCapabilities {
    #[must_use]
    pub fn detect(env: &HashMap<String, String>) -> CLITerminalBackend {
        if Self::supports_kitty_graphics(env) {
            return CLITerminalBackend::KittyGraphics;
        }
        if Self::supports_truecolor(env) {
            return CLITerminalBackend::Truecolor;
        }
        CLITerminalBackend::Standard
    }

    #[must_use]
    pub fn supports_enhanced_cards(use_color: bool, env: &HashMap<String, String>) -> bool {
        if !use_color {
            return false;
        }
        if env.get("ARIADUSAGE_CARDS_ENHANCED").map(String::as_str) == Some("1") {
            return true;
        }
        if env.get("ARIADUSAGE_CARDS_ENHANCED").map(String::as_str) == Some("0") {
            return false;
        }
        Self::supports_truecolor(env)
    }

    #[must_use]
    pub fn supports_truecolor(env: &HashMap<String, String>) -> bool {
        if Self::supports_kitty_graphics(env) {
            return true;
        }
        let color_term = env
            .get("COLORTERM")
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        if color_term.contains("truecolor") || color_term.contains("24bit") {
            return true;
        }
        let term = env
            .get("TERM")
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        term.contains("foot") || term.contains("alacritty")
    }

    #[must_use]
    pub fn supports_kitty_graphics(env: &HashMap<String, String>) -> bool {
        if env.contains_key("KITTY_WINDOW_ID") || env.contains_key("GHOSTTY_RESOURCES_DIR") {
            return true;
        }
        let term = env
            .get("TERM")
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        term.contains("kitty") || term.contains("ghostty") || term.contains("wezterm")
    }
}

impl TerminalCapabilities {
    #[must_use]
    pub fn detect_stdout() -> Self {
        let is_tty = std::io::stdout().is_terminal();
        let env: HashMap<String, String> = std::env::vars().collect();
        Self::from_env_map(is_tty, &env)
    }

    #[must_use]
    pub fn from_current_process() -> Self {
        Self::detect_stdout()
    }

    #[must_use]
    pub fn from_env_map(is_tty: bool, env: &HashMap<String, String>) -> Self {
        let term = env
            .get("TERM")
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_default();
        let supports_color = is_tty && term != "dumb";
        let supports_kitty_graphics = CLITerminalCapabilities::supports_kitty_graphics(env);
        let supports_truecolor =
            supports_kitty_graphics || CLITerminalCapabilities::supports_truecolor(env);
        let enhanced_cards = CLITerminalCapabilities::supports_enhanced_cards(supports_color, env);
        let width = env
            .get("COLUMNS")
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&w| w >= 20)
            .unwrap_or(80);
        Self {
            is_tty,
            supports_color,
            supports_truecolor,
            supports_kitty_graphics,
            enhanced_cards,
            width,
            columns: width,
        }
    }
}
