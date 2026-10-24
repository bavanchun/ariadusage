// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityJSONLObject.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Preserve number lexemes before JSON decoding can round them, while serde_json validates JSON structure and strings.
pub struct AntigravityJSONLObject;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AntigravityJSONLNumber {
    pub integer: Option<i64>,
}

impl fmt::Debug for AntigravityJSONLNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Number")
            .field("integer", &self.integer)
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub enum AntigravityJSONLValue {
    Null,
    Bool(bool),
    Number(AntigravityJSONLNumber),
    String(String),
    Array,
    Object,
}

impl fmt::Debug for AntigravityJSONLValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(f, "Null"),
            Self::Bool(b) => f.debug_tuple("Bool").field(b).finish(),
            Self::Number(n) => f.debug_tuple("Number").field(n).finish(),
            Self::String(_) => write!(f, "String([redacted])"),
            Self::Array => write!(f, "Array"),
            Self::Object => write!(f, "Object"),
        }
    }
}

impl AntigravityJSONLValue {
    pub fn as_number(&self) -> Option<AntigravityJSONLNumber> {
        match self {
            Self::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

enum LexError<E> {
    Syntax,
    Cancelled(E),
}

impl AntigravityJSONLObject {
    pub fn decode<E, C>(
        bytes: &[u8],
        mut check_cancellation: C,
    ) -> Result<Option<BTreeMap<String, AntigravityJSONLValue>>, E>
    where
        C: FnMut() -> Result<(), E>,
    {
        let mut lexer = Lexer {
            bytes,
            masked: bytes.to_vec(),
            check_cancellation: &mut check_cancellation,
            index: 0,
        };
        match lexer.object() {
            Ok(obj) => Ok(Some(obj)),
            Err(LexError::Syntax) => Ok(None),
            Err(LexError::Cancelled(e)) => Err(e),
        }
    }
}

struct Lexer<'a, C> {
    bytes: &'a [u8],
    masked: Vec<u8>,
    check_cancellation: &'a mut C,
    index: usize,
}

impl<'a, E, C> Lexer<'a, C>
where
    C: FnMut() -> Result<(), E>,
{
    fn object(&mut self) -> Result<BTreeMap<String, AntigravityJSONLValue>, LexError<E>> {
        let mut depth: i32 = 0;
        let mut key: Option<String> = None;
        let mut expecting_key = true;
        let mut keys = BTreeSet::new();
        let mut numbers: Vec<(String, AntigravityJSONLNumber)> = Vec::new();

        while let Some(byte) = self.current() {
            match byte {
                b'"' => {
                    let start = self.index;
                    self.string()?;
                    if depth == 1 && expecting_key {
                        let Ok(name) =
                            serde_json::from_slice::<String>(&self.bytes[start..self.index])
                        else {
                            return Err(LexError::Syntax);
                        };
                        if !keys.insert(name.clone()) {
                            return Err(LexError::Syntax);
                        }
                        key = Some(name);
                        expecting_key = false;
                    }
                }
                b'{' | b'[' => {
                    depth += 1;
                    if depth > 512 {
                        return Err(LexError::Syntax);
                    }
                    self.advance()?;
                }
                b'}' | b']' => {
                    depth -= 1;
                    if depth < 0 {
                        return Err(LexError::Syntax);
                    }
                    self.advance()?;
                }
                b',' => {
                    if depth == 1 {
                        key = None;
                        expecting_key = true;
                    }
                    self.advance()?;
                }
                b'-' | b'0'..=b'9' => {
                    let start = self.index;
                    let value = self.number()?;
                    if depth == 1
                        && let Some(ref k) = key
                    {
                        numbers.push((k.clone(), AntigravityJSONLNumber { integer: value }));
                    }
                    // Keep byte positions/JSON separators, replacing only a fully validated numeric token.
                    self.masked[start] = b'0';
                    for offset in (start + 1)..self.index {
                        if offset % 4096 == 0 {
                            (self.check_cancellation)().map_err(LexError::Cancelled)?;
                        }
                        self.masked[offset] = b' ';
                    }
                }
                _ => {
                    self.advance()?;
                }
            }
        }

        (self.check_cancellation)().map_err(LexError::Cancelled)?;
        let Ok(serde_json::Value::Object(raw_map)) =
            serde_json::from_slice::<serde_json::Value>(&self.masked)
        else {
            return Err(LexError::Syntax);
        };

        let mut object = BTreeMap::new();
        for (k, v) in raw_map {
            let mapped = match v {
                serde_json::Value::Null => AntigravityJSONLValue::Null,
                serde_json::Value::Bool(b) => AntigravityJSONLValue::Bool(b),
                serde_json::Value::Number(_) => {
                    AntigravityJSONLValue::Number(AntigravityJSONLNumber { integer: None })
                }
                serde_json::Value::String(s) => AntigravityJSONLValue::String(s),
                serde_json::Value::Array(_) => AntigravityJSONLValue::Array,
                serde_json::Value::Object(_) => AntigravityJSONLValue::Object,
            };
            object.insert(k, mapped);
        }

        for (idx, (k, num)) in numbers.into_iter().enumerate() {
            if idx % 256 == 0 {
                (self.check_cancellation)().map_err(LexError::Cancelled)?;
            }
            object.insert(k, AntigravityJSONLValue::Number(num));
        }

        Ok(object)
    }

    fn current(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    fn is_digit(&self) -> bool {
        self.current().is_some_and(|b| b.is_ascii_digit())
    }

    fn advance(&mut self) -> Result<(), LexError<E>> {
        if self.index.is_multiple_of(4096) {
            (self.check_cancellation)().map_err(LexError::Cancelled)?;
        }
        self.index += 1;
        Ok(())
    }

    fn string(&mut self) -> Result<(), LexError<E>> {
        self.advance()?;
        while let Some(byte) = self.current() {
            self.advance()?;
            if byte == b'"' {
                return Ok(());
            }
            if byte == b'\\' {
                if self.current().is_none() {
                    return Err(LexError::Syntax);
                }
                self.advance()?;
            }
        }
        Err(LexError::Syntax)
    }

    fn number(&mut self) -> Result<Option<i64>, LexError<E>> {
        let negative = self.current() == Some(b'-');
        if negative {
            self.advance()?;
        }
        if !self.is_digit() {
            return Err(LexError::Syntax);
        }
        let mut digits: Vec<u8> = Vec::with_capacity(19);
        let mut significant_count: isize = 0;
        let mut trailing_zeros: isize = 0;
        let mut record = |byte: u8| {
            if byte == b'0' && significant_count == 0 {
                return;
            }
            significant_count += 1;
            trailing_zeros = if byte == b'0' { trailing_zeros + 1 } else { 0 };
            if digits.len() < 19 {
                digits.push(byte - b'0');
            }
        };

        if self.current() == Some(b'0') {
            self.advance()?;
            if self.is_digit() {
                return Err(LexError::Syntax);
            }
        } else {
            while self.is_digit() {
                record(self.bytes[self.index]);
                self.advance()?;
            }
        }

        let mut fraction_digits: isize = 0;
        if self.current() == Some(b'.') {
            self.advance()?;
            if !self.is_digit() {
                return Err(LexError::Syntax);
            }
            while self.is_digit() {
                record(self.bytes[self.index]);
                fraction_digits += 1;
                self.advance()?;
            }
        }

        let mut exponent: isize = 0;
        if matches!(self.current(), Some(b'E' | b'e')) {
            self.advance()?;
            let negative_exponent = self.current() == Some(b'-');
            if self.current() == Some(b'+') || negative_exponent {
                self.advance()?;
            }
            if !self.is_digit() {
                return Err(LexError::Syntax);
            }
            let cap = (self.bytes.len() as isize).saturating_add(20);
            while self.is_digit() {
                let d = (self.bytes[self.index] - b'0') as isize;
                exponent = exponent.saturating_mul(10).saturating_add(d).min(cap);
                self.advance()?;
            }
            if negative_exponent {
                exponent = -exponent;
            }
        }

        if significant_count == 0 {
            return Ok(Some(0));
        }
        let count = significant_count - trailing_zeros;
        let power = exponent - fraction_digits + trailing_zeros;
        if negative || power < 0 || count + power > 19 {
            return Ok(None);
        }
        let total_digits = (count + power) as usize;
        let count_usize = count as usize;
        let mut value: i64 = 0;
        for idx in 0..total_digits {
            let digit = digits
                .get(idx)
                .filter(|_| idx < count_usize)
                .copied()
                .map_or(0, i64::from);
            let Some(product) = value.checked_mul(10) else {
                return Ok(None);
            };
            let Some(sum) = product.checked_add(digit) else {
                return Ok(None);
            };
            value = sum;
        }
        Ok(Some(value))
    }
}
