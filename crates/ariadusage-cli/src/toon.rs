// Ported from CodexBar Sources/CodexBarCLI/ToonFormatter.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Dependencies-free TOON (Token-Oriented Object Notation) encoder over `serde::Serialize` values.
//!
//! Covers the subset needed for AriadUsage CLI payloads:
//! - Root arrays and root objects
//! - 2-space indentation (`key: value` for scalars, `key:` followed by indented children for nested objects)
//! - Inline primitive arrays (`tags[3]: a,b,c`)
//! - Tabular arrays of uniform scalar-valued objects (`rows[2]{a,b}:\n  1,2\n  3,4`)
//! - List arrays (`items[2]:\n  - ...`) for non-uniform or nested element shapes

use std::fmt;

use serde::Serialize;
use serde::ser::{
    self, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};

/// Errors returned when encoding a value as TOON (`ToonError` in `ToonFormatter.swift`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToonError {
    /// IEEE 754 `NaN` or `±Infinity` values are rejected.
    NonFiniteNumber(String),
    /// Custom serialization error message.
    Message(String),
}

impl fmt::Display for ToonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteNumber(value) => write!(
                f,
                "Unable to encode {value} in TOON: only finite numbers are representable."
            ),
            Self::Message(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for ToonError {}

impl ser::Error for ToonError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self::Message(msg.to_string())
    }
}

/// Intermediate ordered node tree produced by the TOON serializer (`ToonNode` in `ToonFormatter.swift`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToonNode {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<ToonNode>),
    Object(Vec<(String, ToonNode)>),
}

impl ToonNode {
    pub const fn is_scalar(&self) -> bool {
        matches!(
            self,
            Self::Null | Self::Bool(_) | Self::Number(_) | Self::String(_)
        )
    }
}

/// TOON encoder (`ToonFormatter` in `ToonFormatter.swift`).
pub struct ToonFormatter;

impl ToonFormatter {
    const INDENT_UNIT: &'static str = "  ";

    /// Encodes any `serde::Serialize` value into a TOON document string.
    pub fn encode<T: Serialize + ?Sized>(value: &T) -> Result<String, ToonError> {
        let node = Self::to_node(value)?;
        Ok(Self::render_root(&node))
    }

    /// Encodes a slice of UTF-8 JSON bytes into a TOON document string.
    pub fn encode_json_bytes(bytes: &[u8]) -> Result<String, ToonError> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|e| ToonError::Message(e.to_string()))?;
        Self::encode(&value)
    }

    /// Serializes `value` into an ordered [`ToonNode`] tree.
    pub fn to_node<T: Serialize + ?Sized>(value: &T) -> Result<ToonNode, ToonError> {
        value.serialize(ToonNodeSerializer)
    }

    /// Renders a [`ToonNode`] tree as a TOON document string (`ToonFormatter.renderRoot`).
    pub fn render_root(node: &ToonNode) -> String {
        match node {
            ToonNode::Object(fields) => {
                if fields.is_empty() {
                    return "{}".to_string();
                }
                Self::render_object_fields(fields, 0).join("\n")
            }
            ToonNode::Array(items) => Self::render_root_array(items, 0).join("\n"),
            _ => Self::format_scalar(node, ','),
        }
    }

    fn indent(depth: usize) -> String {
        Self::INDENT_UNIT.repeat(depth)
    }

    fn render_object_fields(fields: &[(String, ToonNode)], depth: usize) -> Vec<String> {
        let pad = Self::indent(depth);
        let mut lines = Vec::new();
        for (raw_key, value) in fields {
            let key = Self::format_key(raw_key);
            match value {
                ToonNode::Null | ToonNode::Bool(_) | ToonNode::Number(_) | ToonNode::String(_) => {
                    lines.push(format!("{pad}{key}: {}", Self::format_scalar(value, ',')));
                }
                ToonNode::Object(nested) => {
                    if nested.is_empty() {
                        lines.push(format!("{pad}{key}:"));
                    } else {
                        lines.push(format!("{pad}{key}:"));
                        lines.extend(Self::render_object_fields(nested, depth + 1));
                    }
                }
                ToonNode::Array(items) => {
                    lines.extend(Self::render_keyed_array(raw_key, items, depth));
                }
            }
        }
        lines
    }

    fn render_keyed_array(raw_key: &str, items: &[ToonNode], depth: usize) -> Vec<String> {
        let pad = Self::indent(depth);
        let row_pad = Self::indent(depth + 1);
        let key = Self::format_key(raw_key);
        let count = items.len();

        if count == 0 {
            return vec![format!("{pad}{key}: []")];
        }

        if items.iter().all(ToonNode::is_scalar) {
            let joined: Vec<String> = items
                .iter()
                .map(|item| Self::format_scalar(item, ','))
                .collect();
            return vec![format!("{pad}{key}[{count}]: {}", joined.join(","))];
        }

        if let Some(columns) = Self::tabular_columns(items) {
            let header_cols: Vec<String> = columns.iter().map(|c| Self::format_key(c)).collect();
            let mut lines = vec![format!("{pad}{key}[{count}]{{{}}}:", header_cols.join(","))];
            for item in items {
                if let ToonNode::Object(fields) = item {
                    let cells: Vec<String> = fields
                        .iter()
                        .map(|(_, val)| Self::format_scalar(val, ','))
                        .collect();
                    lines.push(format!("{row_pad}{}", cells.join(",")));
                }
            }
            return lines;
        }

        let mut lines = vec![format!("{pad}{key}[{count}]:")];
        lines.extend(Self::render_list_array_items(items, depth + 1));
        lines
    }

    fn render_root_array(items: &[ToonNode], depth: usize) -> Vec<String> {
        let pad = Self::indent(depth);
        let row_pad = Self::indent(depth + 1);
        let count = items.len();

        if count == 0 {
            return vec![format!("{pad}[]")];
        }

        if items.iter().all(ToonNode::is_scalar) {
            let joined: Vec<String> = items
                .iter()
                .map(|item| Self::format_scalar(item, ','))
                .collect();
            return vec![format!("{pad}[{count}]: {}", joined.join(","))];
        }

        if let Some(columns) = Self::tabular_columns(items) {
            let header_cols: Vec<String> = columns.iter().map(|c| Self::format_key(c)).collect();
            let mut lines = vec![format!("{pad}[{count}]{{{}}}:", header_cols.join(","))];
            for item in items {
                if let ToonNode::Object(fields) = item {
                    let cells: Vec<String> = fields
                        .iter()
                        .map(|(_, val)| Self::format_scalar(val, ','))
                        .collect();
                    lines.push(format!("{row_pad}{}", cells.join(",")));
                }
            }
            return lines;
        }

        let mut lines = vec![format!("{pad}[{count}]:")];
        lines.extend(Self::render_list_array_items(items, depth + 1));
        lines
    }

    fn render_list_array_items(items: &[ToonNode], depth: usize) -> Vec<String> {
        let pad = Self::indent(depth);
        let mut lines = Vec::new();
        for item in items {
            match item {
                ToonNode::Null | ToonNode::Bool(_) | ToonNode::Number(_) | ToonNode::String(_) => {
                    lines.push(format!("{pad}- {}", Self::format_scalar(item, ',')));
                }
                ToonNode::Array(nested) => {
                    let sub = Self::render_root_array(nested, depth);
                    if let Some(first) = sub.first() {
                        let drop_bytes = depth * Self::INDENT_UNIT.len();
                        let trimmed = first.get(drop_bytes..).unwrap_or(first.as_str());
                        lines.push(format!("{pad}- {trimmed}"));
                        lines.extend(sub.into_iter().skip(1));
                    }
                }
                ToonNode::Object(fields) => {
                    lines.extend(Self::render_list_item_object(fields, depth));
                }
            }
        }
        lines
    }

    fn render_list_item_object(fields: &[(String, ToonNode)], depth: usize) -> Vec<String> {
        let pad = Self::indent(depth);
        let child_pad = Self::indent(depth + 1);
        if fields.is_empty() {
            return vec![format!("{pad}-")];
        }

        let mut lines = Vec::new();
        let (first_raw_key, first_val) = &fields[0];
        let first_key = Self::format_key(first_raw_key);
        match first_val {
            ToonNode::Null | ToonNode::Bool(_) | ToonNode::Number(_) | ToonNode::String(_) => {
                lines.push(format!(
                    "{pad}- {first_key}: {}",
                    Self::format_scalar(first_val, ',')
                ));
            }
            ToonNode::Object(nested) => {
                lines.push(format!("{pad}- {first_key}:"));
                if !nested.is_empty() {
                    lines.extend(Self::render_object_fields(nested, depth + 2));
                }
            }
            ToonNode::Array(items) => {
                let array_lines = Self::render_keyed_array(first_raw_key, items, depth + 1);
                if let Some(head) = array_lines.first() {
                    let drop_bytes = (depth + 1) * Self::INDENT_UNIT.len();
                    let trimmed = head.get(drop_bytes..).unwrap_or(head.as_str());
                    lines.push(format!("{pad}- {trimmed}"));
                    lines.extend(array_lines.into_iter().skip(1));
                }
            }
        }

        for (raw_key, value) in &fields[1..] {
            let key = Self::format_key(raw_key);
            match value {
                ToonNode::Null | ToonNode::Bool(_) | ToonNode::Number(_) | ToonNode::String(_) => {
                    lines.push(format!(
                        "{child_pad}{key}: {}",
                        Self::format_scalar(value, ',')
                    ));
                }
                ToonNode::Object(nested) => {
                    lines.push(format!("{child_pad}{key}:"));
                    if !nested.is_empty() {
                        lines.extend(Self::render_object_fields(nested, depth + 2));
                    }
                }
                ToonNode::Array(items) => {
                    lines.extend(Self::render_keyed_array(raw_key, items, depth + 1));
                }
            }
        }

        lines
    }

    fn tabular_columns(items: &[ToonNode]) -> Option<Vec<String>> {
        let ToonNode::Object(first_fields) = items.first()? else {
            return None;
        };
        if first_fields.is_empty() {
            return None;
        }
        if !first_fields.iter().all(|(_, val)| val.is_scalar()) {
            return None;
        }
        let columns: Vec<String> = first_fields.iter().map(|(k, _)| k.clone()).collect();
        for item in &items[1..] {
            let ToonNode::Object(fields) = item else {
                return None;
            };
            if fields.len() != columns.len() {
                return None;
            }
            if !fields
                .iter()
                .map(|(k, _)| k.as_str())
                .eq(columns.iter().map(String::as_str))
            {
                return None;
            }
            if !fields.iter().all(|(_, val)| val.is_scalar()) {
                return None;
            }
        }
        Some(columns)
    }

    pub fn format_float(value: f64) -> Result<String, ToonError> {
        if !value.is_finite() {
            return Err(ToonError::NonFiniteNumber(value.to_string()));
        }
        if value == 0.0 {
            return Ok("0".to_string());
        }
        if value.round() == value && value.abs() < 1e15 {
            return Ok((value as i64).to_string());
        }
        if value.abs() >= 1e16 || (value.abs() > 0.0 && value.abs() < 1e-4) {
            let sci = format!("{value:e}");
            if let Some((mantissa, exp)) = sci.split_once('e')
                && let Ok(exp_num) = exp.parse::<i32>()
            {
                return Ok(if exp_num >= 0 {
                    format!("{mantissa}e+{exp_num}")
                } else {
                    format!("{mantissa}e-{:02}", exp_num.unsigned_abs())
                });
            }
            return Ok(sci);
        }
        let mut text = value.to_string();
        if let Some(stripped) = text.strip_suffix(".0") {
            text = stripped.to_string();
        }
        Ok(text)
    }

    fn format_scalar(node: &ToonNode, delimiter: char) -> String {
        match node {
            ToonNode::Null => "null".to_string(),
            ToonNode::Bool(value) => if *value { "true" } else { "false" }.to_string(),
            ToonNode::Number(raw) => raw.clone(),
            ToonNode::String(value) => Self::format_string(value, delimiter),
            ToonNode::Array(_) | ToonNode::Object(_) => "null".to_string(),
        }
    }

    fn format_string(value: &str, delimiter: char) -> String {
        if Self::needs_quoting(value, delimiter) {
            format!("\"{}\"", Self::escape(value))
        } else {
            value.to_string()
        }
    }

    fn needs_quoting(value: &str, delimiter: char) -> bool {
        if value.is_empty() {
            return true;
        }
        if value.starts_with(' ') || value.ends_with(' ') {
            return true;
        }
        if matches!(value, "true" | "false" | "null") {
            return true;
        }
        if value.starts_with('-') || value.starts_with('#') {
            return true;
        }
        if Self::looks_numeric(value) {
            return true;
        }
        for c in value.chars() {
            if c == delimiter {
                return true;
            }
            if matches!(
                c,
                ':' | '"' | '\\' | '[' | ']' | '{' | '}' | '\n' | '\r' | '\t'
            ) {
                return true;
            }
            let code = c as u32;
            if code < 0x20 || code == 0x7F {
                return true;
            }
        }
        false
    }

    fn looks_numeric(value: &str) -> bool {
        let stripped = value.strip_prefix('-').unwrap_or(value);
        if stripped.len() > 1
            && stripped.starts_with('0')
            && !stripped.starts_with("0.")
            && stripped.bytes().all(|b| b.is_ascii_digit())
        {
            return true;
        }
        value.parse::<f64>().is_ok_and(f64::is_finite)
    }

    fn format_key(key: &str) -> String {
        let mut chars = key.chars();
        let Some(first) = chars.next() else {
            return "\"\"".to_string();
        };
        let valid_start = first.is_ascii_alphabetic() || first == '_';
        let valid_rest = chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
        if valid_start && valid_rest {
            key.to_string()
        } else {
            format!("\"{}\"", Self::escape(key))
        }
    }

    fn escape(value: &str) -> String {
        let mut out = String::with_capacity(value.len());
        for c in value.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                _ => {
                    let code = c as u32;
                    if code < 0x20 || code == 0x7F {
                        out.push_str(&format!("\\u{code:04x}"));
                    } else {
                        out.push(c);
                    }
                }
            }
        }
        out
    }
}

struct ToonNodeSerializer;

impl ser::Serializer for ToonNodeSerializer {
    type Ok = ToonNode;
    type Error = ToonError;

    type SerializeSeq = ToonSeqSerializer;
    type SerializeTuple = ToonSeqSerializer;
    type SerializeTupleStruct = ToonSeqSerializer;
    type SerializeTupleVariant = ToonTupleVariantSerializer;
    type SerializeMap = ToonMapSerializer;
    type SerializeStruct = ToonMapSerializer;
    type SerializeStructVariant = ToonStructVariantSerializer;

    fn serialize_bool(self, v: bool) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Bool(v))
    }

    fn serialize_i8(self, v: i8) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Number(v.to_string()))
    }

    fn serialize_i16(self, v: i16) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Number(v.to_string()))
    }

    fn serialize_i32(self, v: i32) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Number(v.to_string()))
    }

    fn serialize_i64(self, v: i64) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Number(v.to_string()))
    }

    fn serialize_i128(self, v: i128) -> Result<Self::Ok, Self::Error> {
        if let Ok(exact) = i64::try_from(v) {
            Ok(ToonNode::Number(exact.to_string()))
        } else {
            Ok(ToonNode::Number(ToonFormatter::format_float(v as f64)?))
        }
    }

    fn serialize_u8(self, v: u8) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Number(v.to_string()))
    }

    fn serialize_u16(self, v: u16) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Number(v.to_string()))
    }

    fn serialize_u32(self, v: u32) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Number(v.to_string()))
    }

    fn serialize_u64(self, v: u64) -> Result<Self::Ok, Self::Error> {
        if let Ok(exact) = i64::try_from(v) {
            Ok(ToonNode::Number(exact.to_string()))
        } else {
            Ok(ToonNode::Number(ToonFormatter::format_float(v as f64)?))
        }
    }

    fn serialize_u128(self, v: u128) -> Result<Self::Ok, Self::Error> {
        if let Ok(exact) = i64::try_from(v) {
            Ok(ToonNode::Number(exact.to_string()))
        } else {
            Ok(ToonNode::Number(ToonFormatter::format_float(v as f64)?))
        }
    }

    fn serialize_f32(self, v: f32) -> Result<Self::Ok, Self::Error> {
        if !v.is_finite() {
            return Err(ToonError::NonFiniteNumber(v.to_string()));
        }
        Ok(ToonNode::Number(ToonFormatter::format_float(f64::from(v))?))
    }

    fn serialize_f64(self, v: f64) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Number(ToonFormatter::format_float(v)?))
    }

    fn serialize_char(self, v: char) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::String(v.to_string()))
    }

    fn serialize_str(self, v: &str) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::String(v.to_string()))
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<Self::Ok, Self::Error> {
        let items = v.iter().map(|b| ToonNode::Number(b.to_string())).collect();
        Ok(ToonNode::Array(items))
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Null)
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Null)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Null)
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::String(variant.to_string()))
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        let inner = value.serialize(Self)?;
        Ok(ToonNode::Object(vec![(variant.to_string(), inner)]))
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Ok(ToonSeqSerializer {
            items: Vec::with_capacity(len.unwrap_or(0)),
        })
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Ok(ToonSeqSerializer {
            items: Vec::with_capacity(len),
        })
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Ok(ToonSeqSerializer {
            items: Vec::with_capacity(len),
        })
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Ok(ToonTupleVariantSerializer {
            variant: variant.to_string(),
            items: Vec::with_capacity(len),
        })
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Ok(ToonMapSerializer {
            fields: Vec::with_capacity(len.unwrap_or(0)),
            pending_key: None,
        })
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(ToonMapSerializer {
            fields: Vec::with_capacity(len),
            pending_key: None,
        })
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Ok(ToonStructVariantSerializer {
            variant: variant.to_string(),
            fields: Vec::with_capacity(len),
        })
    }
}

struct ToonSeqSerializer {
    items: Vec<ToonNode>,
}

impl SerializeSeq for ToonSeqSerializer {
    type Ok = ToonNode;
    type Error = ToonError;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.items.push(value.serialize(ToonNodeSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Array(self.items))
    }
}

impl SerializeTuple for ToonSeqSerializer {
    type Ok = ToonNode;
    type Error = ToonError;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.items.push(value.serialize(ToonNodeSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Array(self.items))
    }
}

impl SerializeTupleStruct for ToonSeqSerializer {
    type Ok = ToonNode;
    type Error = ToonError;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.items.push(value.serialize(ToonNodeSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Array(self.items))
    }
}

struct ToonTupleVariantSerializer {
    variant: String,
    items: Vec<ToonNode>,
}

impl SerializeTupleVariant for ToonTupleVariantSerializer {
    type Ok = ToonNode;
    type Error = ToonError;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.items.push(value.serialize(ToonNodeSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Object(vec![(
            self.variant,
            ToonNode::Array(self.items),
        )]))
    }
}

struct ToonMapSerializer {
    fields: Vec<(String, ToonNode)>,
    pending_key: Option<String>,
}

impl ToonMapSerializer {
    fn upsert(&mut self, key: String, value: ToonNode) {
        if let Some(existing) = self.fields.iter_mut().find(|(k, _)| *k == key) {
            existing.1 = value;
        } else {
            self.fields.push((key, value));
        }
    }
}

impl SerializeMap for ToonMapSerializer {
    type Ok = ToonNode;
    type Error = ToonError;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Self::Error> {
        let key_node = key.serialize(ToonNodeSerializer)?;
        let key_str = match key_node {
            ToonNode::String(s) => s,
            ToonNode::Number(n) => n,
            ToonNode::Bool(b) => b.to_string(),
            ToonNode::Null => "null".to_string(),
            _ => {
                return Err(ToonError::Message(
                    "TOON map keys must be scalar values".to_string(),
                ));
            }
        };
        self.pending_key = Some(key_str);
        Ok(())
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        let key = self.pending_key.take().ok_or_else(|| {
            ToonError::Message("serialize_value called before serialize_key".into())
        })?;
        let val = value.serialize(ToonNodeSerializer)?;
        self.upsert(key, val);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Object(self.fields))
    }
}

impl SerializeStruct for ToonMapSerializer {
    type Ok = ToonNode;
    type Error = ToonError;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        let val = value.serialize(ToonNodeSerializer)?;
        self.upsert(key.to_string(), val);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Object(self.fields))
    }
}

struct ToonStructVariantSerializer {
    variant: String,
    fields: Vec<(String, ToonNode)>,
}

impl SerializeStructVariant for ToonStructVariantSerializer {
    type Ok = ToonNode;
    type Error = ToonError;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        let val = value.serialize(ToonNodeSerializer)?;
        self.fields.push((key.to_string(), val));
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(ToonNode::Object(vec![(
            self.variant,
            ToonNode::Object(self.fields),
        )]))
    }
}
