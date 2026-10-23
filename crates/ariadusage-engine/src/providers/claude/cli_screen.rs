// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeCLIScreen.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeCLISpendCapParser.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::ProviderCostSnapshot;
use jiff::Timestamp;

/// Replays a bounded subset of VT/ANSI terminal controls captured from the Claude CLI PTY.
///
/// Newer Claude Code builds render `/usage` and `/status` with cursor movement (`CSI C/G/H/B`)
/// and line/screen erases (`CSI K/J`) instead of literal spaces and fresh lines. Stripping those
/// sequences collapses words (`59Gus` + `62Gd` -> `usd`) and leaves stale text behind.
pub struct ClaudeCLIScreen;

impl ClaudeCLIScreen {
    /// Matches the 160-column PTY geometry used by `ClaudeCLISession`.
    pub const COLUMNS: usize = 160;
    /// Matches the 200-row PTY geometry used by `ClaudeCLISession`.
    ///
    /// Claude Code 2.1.270+ `/usage` renders session stats, plugin skill footprint, quota bars and
    /// usage insights inline; when a tall insight list exceeds 50 rows, the top of the panel
    /// (`Current session`) scrolls out of a 50-row screen buffer before the probe reads it.
    pub const ROWS: usize = 200;
    const MAX_PARAMETER: usize = 200;

    pub fn render(text: &str) -> String {
        Self::render_with_options(text, false)
    }

    pub fn render_with_options(text: &str, preserve_plain_reports: bool) -> String {
        let mut buffer = ScreenBuffer::new(Self::COLUMNS, Self::ROWS);
        let mut replay = !preserve_plain_reports;
        let mut plain = String::new();

        let scalars: Vec<char> = text.chars().collect();
        let mut index = 0;

        while index < scalars.len() {
            if scalars[index] == '\x1b' {
                if index + 1 < scalars.len() && scalars[index + 1] == '[' {
                    let (csi_slice, next_idx) = Self::consume_csi_token(&scalars, index + 2);
                    if buffer.apply_csi(csi_slice) {
                        replay = true;
                    }
                    index = next_idx;
                    continue;
                }
                if index + 1 < scalars.len() && scalars[index + 1] == ']' {
                    index = Self::consume_osc_token(&scalars, index + 2);
                    continue;
                }
                index = (index + 2).min(scalars.len());
                continue;
            }

            let start = index;
            while index < scalars.len() && scalars[index] != '\x1b' {
                index += 1;
            }
            let chunk = &scalars[start..index];
            if !replay {
                for &ch in chunk {
                    plain.push(ch);
                }
                if chunk.contains(&'\x08') {
                    replay = true;
                }
            }
            buffer.apply_chars(chunk);
        }

        if !replay {
            return plain;
        }
        buffer.rendered_text()
    }

    fn consume_osc_token(scalars: &[char], mut index: usize) -> usize {
        while index < scalars.len() {
            if scalars[index] == '\x07' {
                return index + 1;
            }
            if scalars[index] == '\x1b' && index + 1 < scalars.len() && scalars[index + 1] == '\\' {
                return index + 2;
            }
            index += 1;
        }
        scalars.len()
    }

    fn consume_csi_token(scalars: &[char], start_index: usize) -> (&[char], usize) {
        let mut index = start_index;
        while index < scalars.len() {
            let value = scalars[index] as u32;
            if (0x30..=0x3F).contains(&value) {
                index += 1;
                continue;
            }
            break;
        }
        while index < scalars.len() {
            let value = scalars[index] as u32;
            if (0x20..=0x2F).contains(&value) {
                index += 1;
                continue;
            }
            break;
        }
        if index < scalars.len() {
            let final_value = scalars[index] as u32;
            if (0x40..=0x7E).contains(&final_value) {
                index += 1;
            }
        }
        (&scalars[start_index..index], index)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Cell {
    Empty,
    Glyph(String),
    Continuation,
}

struct ScreenBuffer {
    columns: usize,
    rows: usize,
    lines: Vec<Vec<Cell>>,
    row: usize,
    col: usize,
}

impl ScreenBuffer {
    fn new(columns: usize, rows: usize) -> Self {
        Self {
            columns,
            rows,
            lines: vec![vec![Cell::Empty; columns]; rows],
            row: 0,
            col: 0,
        }
    }

    fn apply_chars(&mut self, scalars: &[char]) {
        let mut index = 0;

        while index < scalars.len() {
            let scalar = scalars[index];
            match scalar {
                '\r' => {
                    self.col = 0;
                }
                '\n' => {
                    self.new_line();
                }
                '\x08' => {
                    self.col = self.col.min(self.columns - 1).saturating_sub(1);
                }
                '\t' => {
                    self.col = ((self.col / 8 + 1) * 8).min(self.columns - 1);
                }
                _ => {
                    let value = scalar as u32;
                    if value < 0x20 || value == 0x7F {
                        index += 1;
                        continue;
                    }
                    if is_combining(scalar) {
                        self.append_combining(scalar);
                        index += 1;
                        continue;
                    }
                    let mut glyph = String::from(scalar);
                    let mut width = display_width(scalar);
                    while index + 1 < scalars.len() && is_combining(scalars[index + 1]) {
                        index += 1;
                        let combining = scalars[index];
                        if let Some(composed) = compose_combining(&glyph, combining) {
                            glyph = composed;
                        } else {
                            glyph.push(combining);
                        }
                        if combining == '\u{FE0F}' {
                            width = 2;
                        }
                    }
                    self.put(&glyph, width);
                }
            }

            index += 1;
        }
    }

    fn rendered_text(&self) -> String {
        let mut rendered_lines: Vec<String> = self
            .lines
            .iter()
            .map(|line| {
                let mut rendered = String::with_capacity(self.columns);
                for cell in line {
                    match cell {
                        Cell::Empty => rendered.push(' '),
                        Cell::Glyph(text) => rendered.push_str(text),
                        Cell::Continuation => {}
                    }
                }
                while rendered.ends_with(' ') {
                    rendered.pop();
                }
                rendered
            })
            .collect();

        while rendered_lines.last().is_some_and(String::is_empty) {
            rendered_lines.pop();
        }
        rendered_lines.join("\n")
    }

    fn apply_csi(&mut self, sequence: &[char]) -> bool {
        let Some(&command) = sequence.last() else {
            return false;
        };
        if !matches!(command, 'A' | 'B' | 'C' | 'D' | 'G' | 'H' | 'f' | 'K' | 'J') {
            return false;
        }
        let parameters = &sequence[..sequence.len() - 1];
        if !parameters.iter().all(|&c| c.is_ascii_digit() || c == ';') {
            return false;
        }

        let param_str: String = parameters.iter().collect();
        let values: Vec<usize> = param_str
            .splitn(3, ';')
            .take(2)
            .map(|part| {
                if part.is_empty() {
                    0
                } else {
                    part.parse::<usize>()
                        .unwrap_or(ClaudeCLIScreen::MAX_PARAMETER)
                        .min(ClaudeCLIScreen::MAX_PARAMETER)
                }
            })
            .collect();

        let mode = values.first().copied().unwrap_or(0);
        let amount = mode.max(1);
        if matches!(command, 'K' | 'J') && mode > 2 {
            return false;
        }

        self.col = self.col.min(self.columns - 1);
        match command {
            'A' => {
                self.row = self.row.saturating_sub(amount);
            }
            'B' => {
                self.row = (self.row + amount).min(self.rows - 1);
            }
            'C' => {
                self.col = (self.col + amount).min(self.columns - 1);
            }
            'D' => {
                self.col = self.col.saturating_sub(amount);
            }
            'G' => {
                self.col = (amount - 1).min(self.columns - 1);
            }
            'H' | 'f' => {
                self.row = (amount - 1).min(self.rows - 1);
                let col_amount = values.get(1).copied().unwrap_or(0).max(1);
                self.col = (col_amount - 1).min(self.columns - 1);
            }
            'K' | 'J' => {
                let cursor = self.row * self.columns + self.col;
                let start = if command == 'K' {
                    self.row * self.columns
                } else {
                    0
                };
                let end = if command == 'K' {
                    (self.row + 1) * self.columns
                } else {
                    self.rows * self.columns
                };
                let range_start = if mode == 0 { cursor } else { start };
                let range_end = if mode == 1 { cursor + 1 } else { end };
                for cell in range_start..range_end {
                    self.clear_cell(cell / self.columns, cell % self.columns);
                }
            }
            _ => return false,
        }
        true
    }

    fn put(&mut self, glyph: &str, width: usize) {
        let cell_width = width.clamp(1, 2);
        if self.col + cell_width > self.columns {
            self.new_line();
        }
        for cell in self.col..(self.col + cell_width) {
            self.clear_cell(self.row, cell);
        }
        self.lines[self.row][self.col] = Cell::Glyph(glyph.to_owned());
        if cell_width == 2 && self.col + 1 < self.columns {
            self.lines[self.row][self.col + 1] = Cell::Continuation;
        }
        self.col += cell_width;
    }

    fn append_combining(&mut self, scalar: char) {
        if self.row >= self.rows {
            return;
        }
        let clamped_col = self.col.min(self.columns);
        if clamped_col == 0 {
            return;
        }
        let mut target_col = clamped_col - 1;
        if matches!(self.lines[self.row][target_col], Cell::Continuation) && target_col > 0 {
            target_col -= 1;
        }
        let Cell::Glyph(existing) = &mut self.lines[self.row][target_col] else {
            return;
        };
        if let Some(composed) = compose_combining(existing, scalar) {
            *existing = composed;
        } else {
            existing.push(scalar);
        }
        if scalar == '\u{FE0F}'
            && target_col + 1 < self.columns
            && matches!(self.lines[self.row][target_col + 1], Cell::Empty)
        {
            self.lines[self.row][target_col + 1] = Cell::Continuation;
            if self.col == target_col + 1 {
                self.col += 1;
            }
        }
    }

    fn clear_cell(&mut self, row: usize, col: usize) {
        if row >= self.rows || col >= self.columns {
            return;
        }
        if col > 0 && matches!(self.lines[row][col], Cell::Continuation) {
            self.lines[row][col - 1] = Cell::Empty;
        }
        if col + 1 < self.columns && matches!(self.lines[row][col + 1], Cell::Continuation) {
            self.lines[row][col + 1] = Cell::Empty;
        }
        self.lines[row][col] = Cell::Empty;
    }

    fn new_line(&mut self) {
        self.col = 0;
        self.row += 1;
        if self.row == self.rows {
            self.lines.remove(0);
            self.lines.push(vec![Cell::Empty; self.columns]);
            self.row -= 1;
        }
    }
}

fn compose_combining(base: &str, combining: char) -> Option<String> {
    let mut chars = base.chars();
    let first = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let composed = match (first, combining) {
        ('a', '\u{0300}') => 'à',
        ('a', '\u{0301}') => 'á',
        ('a', '\u{0302}') => 'â',
        ('a', '\u{0303}') => 'ã',
        ('a', '\u{0308}') => 'ä',
        ('e', '\u{0300}') => 'è',
        ('e', '\u{0301}') => 'é',
        ('e', '\u{0302}') => 'ê',
        ('e', '\u{0308}') => 'ë',
        ('i', '\u{0300}') => 'ì',
        ('i', '\u{0301}') => 'í',
        ('i', '\u{0302}') => 'î',
        ('i', '\u{0308}') => 'ï',
        ('o', '\u{0300}') => 'ò',
        ('o', '\u{0301}') => 'ó',
        ('o', '\u{0302}') => 'ô',
        ('o', '\u{0303}') => 'õ',
        ('o', '\u{0308}') => 'ö',
        ('u', '\u{0300}') => 'ù',
        ('u', '\u{0301}') => 'ú',
        ('u', '\u{0302}') => 'û',
        ('u', '\u{0308}') => 'ü',
        ('n', '\u{0303}') => 'ñ',
        ('c', '\u{0327}') => 'ç',
        _ => return None,
    };
    Some(composed.to_string())
}

fn is_combining(scalar: char) -> bool {
    let value = scalar as u32;
    matches!(
        value,
        0x0300..=0x036F
            | 0x0483..=0x0489
            | 0x0591..=0x05BD
            | 0x05BF
            | 0x05C1..=0x05C2
            | 0x05C4..=0x05C5
            | 0x05C7
            | 0x0600..=0x0605
            | 0x0610..=0x061A
            | 0x061C
            | 0x064B..=0x065F
            | 0x0670
            | 0x06D6..=0x06DD
            | 0x06DF..=0x06E4
            | 0x06E7..=0x06E8
            | 0x06EA..=0x06ED
            | 0x070F
            | 0x0711
            | 0x0730..=0x074A
            | 0x07A6..=0x07B0
            | 0x07EB..=0x07F3
            | 0x0816..=0x0819
            | 0x081B..=0x0823
            | 0x0825..=0x0827
            | 0x0829..=0x082D
            | 0x0859..=0x085B
            | 0x08D3..=0x0902
            | 0x093A
            | 0x093C
            | 0x0941..=0x0948
            | 0x094D
            | 0x0951..=0x0957
            | 0x0962..=0x0963
            | 0x0E31
            | 0x0E34..=0x0E3A
            | 0x0E47..=0x0E4E
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0x20D0..=0x20F0
            | 0x302A..=0x302D
            | 0x3099..=0x309A
            | 0xFE00..=0xFE0F
            | 0xFE20..=0xFE2F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x1D167..=0x1D169
            | 0x1D173..=0x1D182
            | 0x1D185..=0x1D18B
            | 0x1D1AA..=0x1D1AD
            | 0xE0001..=0xE007F
            | 0xE0100..=0xE01EF
    )
}

fn is_emoji_presentation(value: u32) -> bool {
    matches!(
        value,
        0x231A..=0x231B
            | 0x23E9..=0x23EC
            | 0x23F0
            | 0x23F3
            | 0x25FD..=0x25FE
            | 0x2614..=0x2615
            | 0x2648..=0x2653
            | 0x267F
            | 0x2693
            | 0x26A1
            | 0x26AA..=0x26AB
            | 0x26BD..=0x26BE
            | 0x26C4..=0x26C5
            | 0x26CE
            | 0x26D4
            | 0x26EA
            | 0x26F2..=0x26F3
            | 0x26F5
            | 0x26FA
            | 0x26FD
            | 0x2705
            | 0x270A..=0x270B
            | 0x2728
            | 0x274C
            | 0x274E
            | 0x2753..=0x2755
            | 0x2757
            | 0x2795..=0x2797
            | 0x27B0
            | 0x27BF
            | 0x2B1B..=0x2B1C
            | 0x2B50
            | 0x2B55
            | 0x1F004
            | 0x1F0CF
            | 0x1F18E
            | 0x1F191..=0x1F19A
            | 0x1F1E6..=0x1F1FF
            | 0x1F201..=0x1F202
            | 0x1F21A
            | 0x1F22F
            | 0x1F232..=0x1F23A
            | 0x1F250..=0x1F251
            | 0x1F300..=0x1FAFF
    )
}

fn display_width(scalar: char) -> usize {
    if is_combining(scalar) {
        return 0;
    }
    let value = scalar as u32;
    if is_emoji_presentation(value) {
        return 2;
    }
    match value {
        0x1100..=0x115F
        | 0x2329..=0x232A
        | 0x2E80..=0x303E
        | 0x3040..=0x3247
        | 0x3250..=0x4DBF
        | 0x4E00..=0xA4C6
        | 0xA960..=0xA97C
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6B
        | 0xFF01..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

/// Parses the Claude CLI `Usage credits` spend-cap section (`$X / $Y spent`) into a [`ProviderCostSnapshot`].
pub struct ClaudeCLISpendCapParser;

impl ClaudeCLISpendCapParser {
    pub fn parse(clean_text: &str, now: Timestamp) -> Option<ProviderCostSnapshot> {
        let lines: Vec<&str> = clean_text.lines().collect();
        let header_index = lines
            .iter()
            .rposition(|line| line.trim().eq_ignore_ascii_case("usage credits"))?;

        let end = (header_index + 5).min(lines.len());
        for line in &lines[(header_index + 1)..end] {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some((used_raw, limit_raw)) = Self::match_spend_cap_line(trimmed) {
                let Some(used) = Self::parse_amount(used_raw) else {
                    continue;
                };
                let Some(limit) = Self::parse_amount(limit_raw) else {
                    continue;
                };
                let mut snapshot = ProviderCostSnapshot::new(used, limit, "USD", now);
                snapshot.period = Some("Monthly cap".to_owned());
                return Some(snapshot);
            }
        }
        None
    }

    fn match_spend_cap_line(line: &str) -> Option<(&str, &str)> {
        let rest = line.strip_prefix('$')?;
        let (used_raw, rest) = Self::take_currency_token(rest)?;
        let rest = rest.trim_start().strip_prefix('/')?.trim_start();
        let rest = rest.strip_prefix('$')?;
        let (limit_raw, rest) = Self::take_currency_token(rest)?;
        let rest = rest.trim_start();
        if rest.len() < 5 || !rest.as_bytes()[..5].eq_ignore_ascii_case(b"spent") {
            return None;
        }
        let after_spent = &rest[5..];
        if let Some(next_ch) = after_spent.chars().next()
            && (next_ch.is_ascii_alphanumeric() || next_ch == '_')
        {
            return None;
        }
        Some((used_raw, limit_raw))
    }

    fn take_currency_token(input: &str) -> Option<(&str, &str)> {
        let bytes = input.as_bytes();
        if bytes.is_empty() || !bytes[0].is_ascii_digit() {
            return None;
        }
        let mut idx = 1;
        while idx < bytes.len() && (bytes[idx].is_ascii_digit() || bytes[idx] == b',') {
            idx += 1;
        }
        if idx < bytes.len() && bytes[idx] == b'.' {
            let frac_start = idx + 1;
            let mut frac_len = 0;
            while frac_start + frac_len < bytes.len()
                && bytes[frac_start + frac_len].is_ascii_digit()
            {
                frac_len += 1;
            }
            if !(1..=2).contains(&frac_len) {
                return None;
            }
            idx = frac_start + frac_len;
        }
        Some((&input[..idx], &input[idx..]))
    }

    fn parse_amount(raw: &str) -> Option<f64> {
        let normalized = raw.replace(',', "");
        let value = normalized.parse::<f64>().ok()?;
        (value.is_finite() && value >= 0.0).then_some(value)
    }
}
