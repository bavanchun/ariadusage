// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexPlanFormatting.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

pub struct CodexPlanFormatting;

impl CodexPlanFormatting {
    const UPPERCASE_WORDS: &'static [&'static str] = &["cbp", "k12"];

    pub fn display_name(raw_plan: &str) -> Option<String> {
        let raw = raw_plan.trim();
        if raw.is_empty() {
            return None;
        }

        if let Some(exact) = Self::exact_display_name(raw) {
            return Some(exact.to_owned());
        }

        let cleaned = Self::clean_plan_name(raw);
        let candidate = cleaned.trim();
        if candidate.is_empty() {
            return Some(raw.to_owned());
        }

        if let Some(exact) = Self::exact_display_name(candidate) {
            return Some(exact.to_owned());
        }

        let components: Vec<&str> = candidate
            .split(|c: char| c == '_' || c == '-' || c.is_whitespace())
            .filter(|part| !part.is_empty())
            .collect();
        if components.is_empty() {
            return Some(candidate.to_owned());
        }

        let formatted = components
            .into_iter()
            .map(Self::word_display_name)
            .collect::<Vec<_>>()
            .join(" ");
        if formatted.is_empty() {
            Some(candidate.to_owned())
        } else {
            Some(formatted)
        }
    }

    fn exact_display_name(value: &str) -> Option<&'static str> {
        match value.to_lowercase().as_str() {
            "pro" => Some("Pro 20x"),
            "prolite" | "pro_lite" | "pro-lite" | "pro lite" => Some("Pro 5x"),
            _ => None,
        }
    }

    fn word_display_name(raw: &str) -> String {
        let lower = raw.to_lowercase();
        if Self::UPPERCASE_WORDS.contains(&lower.as_str()) {
            return lower.to_uppercase();
        }
        if raw == raw.to_uppercase() && raw.chars().any(char::is_alphabetic) {
            return raw.to_owned();
        }
        let mut chars = raw.chars();
        if let Some(first) = chars.next()
            && first.is_lowercase()
        {
            let mut out = first.to_uppercase().to_string();
            out.push_str(chars.as_str());
            return out;
        }
        raw.to_owned()
    }

    fn clean_plan_name(text: &str) -> String {
        let stripped = Self::strip_ansi_codes(text);
        let without_codes = Self::strip_leading_bracket_codes(&stripped);
        let without_boilerplate = Self::strip_boilerplate_words(without_codes);
        let mut cleaned = without_boilerplate
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if cleaned.is_empty() {
            cleaned = stripped.trim().to_owned();
        }
        if cleaned.eq_ignore_ascii_case("oauth") {
            return "Ollama".to_owned();
        }
        let mut chars = cleaned.chars();
        if let Some(first) = chars.next()
            && first.is_lowercase()
        {
            let mut out = first.to_uppercase().to_string();
            out.push_str(chars.as_str());
            return out;
        }
        cleaned
    }

    fn strip_ansi_codes(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\x1b' {
                match chars.peek().copied() {
                    Some('[') => {
                        let _ = chars.next();
                        for c in chars.by_ref() {
                            if ('@'..='~').contains(&c) {
                                break;
                            }
                        }
                    }
                    Some(']') => {
                        let _ = chars.next();
                        while let Some(c) = chars.next() {
                            if c == '\x07' {
                                break;
                            }
                            if c == '\x1b' && chars.peek() == Some(&'\\') {
                                let _ = chars.next();
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            } else {
                out.push(ch);
            }
        }
        out
    }

    fn strip_leading_bracket_codes(mut text: &str) -> &str {
        loop {
            let trimmed = text.trim_start();
            let Some(rest) = trimmed.strip_prefix('[') else {
                return text;
            };
            let digits_len = rest.bytes().take_while(u8::is_ascii_digit).count();
            if !(1..=3).contains(&digits_len) {
                return text;
            }
            let after_digits = &rest[digits_len..];
            let Some(after_m) = after_digits.strip_prefix('m') else {
                return text;
            };
            text = after_m.trim_start();
        }
    }

    fn strip_boilerplate_words(text: &str) -> String {
        fn is_word_char(ch: char) -> bool {
            ch.is_alphanumeric() || ch == '_'
        }

        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let mut out = String::with_capacity(text.len());
        let mut idx = 0;
        while idx < chars.len() {
            let (start_byte, ch) = chars[idx];
            if is_word_char(ch) {
                let word_start = idx;
                while idx < chars.len() && is_word_char(chars[idx].1) {
                    idx += 1;
                }
                let end_byte = if idx < chars.len() {
                    chars[idx].0
                } else {
                    text.len()
                };
                let token = &text[start_byte..end_byte];
                let prev_is_word = word_start > 0 && is_word_char(chars[word_start - 1].1);
                let next_is_word = idx < chars.len() && is_word_char(chars[idx].1);
                if !prev_is_word
                    && !next_is_word
                    && matches!(
                        token.to_ascii_lowercase().as_str(),
                        "claude" | "codex" | "account" | "plan"
                    )
                {
                    continue;
                }
                out.push_str(token);
            } else {
                out.push(ch);
                idx += 1;
            }
        }
        out
    }
}
