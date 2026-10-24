// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityProtoReader.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::fmt;

/// Decodes only the independently recorded generation layout; no inferred opaque timestamps.
pub struct AntigravityProtoReader<'a> {
    bytes: &'a [u8],
    offset: usize,
    is_malformed: bool,
}

#[derive(Clone, Default)]
pub struct AuxiliaryIdentifier {
    value: Option<String>,
    is_valid: bool,
}

impl AuxiliaryIdentifier {
    pub fn new() -> Self {
        Self {
            value: None,
            is_valid: true,
        }
    }

    pub fn with_value(value: Option<String>) -> Self {
        Self {
            value,
            is_valid: true,
        }
    }

    pub fn value(&self) -> Option<&str> {
        self.value.as_deref()
    }

    fn read(&mut self, field: &ProtoField<'_>) {
        match field.string() {
            Ok(val) => {
                if self.is_valid {
                    self.value = val;
                }
            }
            Err(()) => {
                self.invalidate();
            }
        }
    }

    pub fn invalidate(&mut self) {
        self.value = None;
        self.is_valid = false;
    }
}

impl PartialEq for AuxiliaryIdentifier {
    fn eq(&self, other: &Self) -> bool {
        // Parser invalidity is not part of finalized event identity or copied-row deduplication.
        self.value == other.value
    }
}

impl Eq for AuxiliaryIdentifier {}

impl fmt::Debug for AuxiliaryIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuxiliaryIdentifier")
            .field("has_value", &self.value.is_some())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ParsedUsage {
    pub new_input: usize,
    pub cache_read: usize,
    pub output: usize,
    pub reasoning: usize,
    pub response_id: Option<String>,
    /// Field 1: the model enum ID (e.g. 1298 for `gemini-3.7-flash`). Validated and kept for
    /// copied-row identity; never counted as tokens. Absent for JSONL-sourced usage.
    pub model_id: Option<usize>,
    pub(crate) bot_identifier: AuxiliaryIdentifier,
}

impl Default for ParsedUsage {
    fn default() -> Self {
        Self {
            new_input: 0,
            cache_read: 0,
            output: 0,
            reasoning: 0,
            response_id: None,
            model_id: None,
            bot_identifier: AuxiliaryIdentifier::new(),
        }
    }
}

impl ParsedUsage {
    pub fn bot_id(&self) -> Option<&str> {
        self.bot_identifier.value()
    }

    pub fn set_bot_id(&mut self, bot_id: Option<String>) {
        self.bot_identifier = AuxiliaryIdentifier::with_value(bot_id);
    }
}

impl fmt::Debug for ParsedUsage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParsedUsage")
            .field("new_input", &self.new_input)
            .field("cache_read", &self.cache_read)
            .field("output", &self.output)
            .field("reasoning", &self.reasoning)
            .field("has_response_id", &self.response_id.is_some())
            .field("model_id", &self.model_id)
            .field("has_bot_id", &self.bot_id().is_some())
            .finish()
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct ParsedTurn {
    pub usage: Option<ParsedUsage>,
    pub timestamp_ms: Option<i64>,
    pub model: Option<String>,
    pub label: Option<String>,
    pub step_uuid: Option<String>,
}

impl fmt::Debug for ParsedTurn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParsedTurn")
            .field("usage", &self.usage)
            .field("timestamp_ms", &self.timestamp_ms)
            .field("model", &self.model)
            .field("has_label", &self.label.is_some())
            .field("has_step_uuid", &self.step_uuid.is_some())
            .finish()
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct StepMetadata {
    pub step_uuid: Option<String>,
    pub bot_id: Option<String>,
    pub timestamp_ms: Option<i64>,
}

impl fmt::Debug for StepMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StepMetadata")
            .field("has_step_uuid", &self.step_uuid.is_some())
            .field("has_bot_id", &self.bot_id.is_some())
            .field("timestamp_ms", &self.timestamp_ms)
            .finish()
    }
}

#[derive(Default)]
struct ProtoTimestamp {
    seconds: Option<u64>,
    nanos: u64,
}

impl ProtoTimestamp {
    fn milliseconds(&self) -> Result<Option<i64>, ()> {
        let Some(seconds) = self.seconds else {
            return Ok(None);
        };
        if seconds == 0 || seconds > 253_402_300_799 || self.nanos > 999_999_999 {
            return Err(());
        }
        let sec = i64::try_from(seconds).map_err(|_| ())?;
        let nanos = i64::try_from(self.nanos).map_err(|_| ())?;
        Ok(Some(sec * 1000 + nanos / 1_000_000))
    }
}

pub struct ProtoField<'a> {
    pub number: u32,
    pub wire: u8,
    pub data: Option<&'a [u8]>,
    pub value: Option<u64>,
}

impl<'a> ProtoField<'a> {
    fn message(&self) -> Result<&'a [u8], ()> {
        if self.wire == 2
            && let Some(data) = self.data
        {
            return Ok(data);
        }
        Err(())
    }

    fn integer(&self) -> Result<u64, ()> {
        if self.wire == 0
            && let Some(val) = self.value
        {
            return Ok(val);
        }
        Err(())
    }

    fn counter(&self) -> Result<usize, ()> {
        let raw = self.integer()?;
        let signed = i64::try_from(raw).map_err(|_| ())?;
        usize::try_from(signed).map_err(|_| ())
    }

    fn string(&self) -> Result<Option<String>, ()> {
        let bytes = self.message()?;
        let text = std::str::from_utf8(bytes).map_err(|_| ())?;
        // Preserve raw identities for conflict detection; whitespace-only optional fields are absent.
        if text.trim().is_empty() {
            Ok(None)
        } else {
            Ok(Some(text.to_owned()))
        }
    }
}

pub struct CheckedSum;

impl CheckedSum {
    #[must_use]
    pub fn integers(values: &[usize]) -> Option<usize> {
        let max_signed = usize::try_from(i64::MAX).unwrap_or(usize::MAX);
        let mut total = 0usize;
        for &val in values {
            if val > max_signed {
                return None;
            }
            total = total.checked_add(val)?;
            if total > max_signed {
                return None;
            }
        }
        Some(total)
    }
}

enum FieldError<E> {
    Invalid,
    Cancelled(E),
}

impl<'a> AntigravityProtoReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            is_malformed: false,
        }
    }

    pub fn is_malformed(&self) -> bool {
        self.is_malformed
    }

    pub fn read_varint(&mut self) -> Option<u64> {
        let mut result: u64 = 0;
        for index in 0..10 {
            if self.offset >= self.bytes.len() {
                break;
            }
            let byte = self.bytes[self.offset];
            self.offset += 1;
            if index == 9 && byte > 1 {
                break;
            }
            result |= u64::from(byte & 0x7F) << (index * 7);
            if byte & 0x80 == 0 {
                return Some(result);
            }
        }
        self.is_malformed = true;
        None
    }

    pub fn next_field(&mut self) -> Option<ProtoField<'a>> {
        if self.offset >= self.bytes.len() {
            return None;
        }
        let Some(tag) = self.read_varint() else {
            self.is_malformed = true;
            return None;
        };
        let field_num = tag >> 3;
        if field_num == 0 || field_num > 536_870_911 {
            self.is_malformed = true;
            return None;
        }
        let number = field_num as u32;
        let wire = (tag & 7) as u8;
        if wire == 0 {
            let value = self.read_varint()?;
            return Some(ProtoField {
                number,
                wire,
                data: None,
                value: Some(value),
            });
        }
        let count: Option<usize> = match wire {
            1 => Some(8),
            2 => self.read_varint().and_then(|v| usize::try_from(v).ok()),
            5 => Some(4),
            _ => None,
        };
        let Some(count) = count else {
            self.is_malformed = true;
            return None;
        };
        if count > self.bytes.len() - self.offset {
            self.is_malformed = true;
            return None;
        }
        let end = self.offset + count;
        let data = &self.bytes[self.offset..end];
        self.offset = end;
        Some(ProtoField {
            number,
            wire,
            data: Some(data),
            value: None,
        })
    }

    fn fields<'b, E, C, V>(
        bytes: &'b [u8],
        check_cancellation: &mut C,
        mut visit: V,
    ) -> Result<(), FieldError<E>>
    where
        C: FnMut() -> Result<(), E>,
        V: FnMut(ProtoField<'b>, &mut C) -> Result<(), FieldError<E>>,
    {
        let mut reader = AntigravityProtoReader::new(bytes);
        while reader.offset < reader.bytes.len() {
            check_cancellation().map_err(FieldError::Cancelled)?;
            let Some(field) = reader.next_field() else {
                break;
            };
            visit(field, check_cancellation)?;
        }
        if reader.is_malformed {
            return Err(FieldError::Invalid);
        }
        Ok(())
    }

    pub fn parse_turn<E, C>(
        root_bytes: &[u8],
        mut check_cancellation: C,
    ) -> Result<Option<ParsedTurn>, E>
    where
        C: FnMut() -> Result<(), E>,
    {
        let mut turn = ParsedTurn::default();
        let mut time = ProtoTimestamp::default();
        let mut found_chat = false;
        let res = Self::fields(root_bytes, &mut check_cancellation, |field, cancel| {
            match field.number {
                1 => {
                    found_chat = true;
                    let msg = field.message().map_err(|()| FieldError::Invalid)?;
                    Self::parse_chat(msg, &mut turn, &mut time, cancel)?;
                }
                4 => {
                    turn.step_uuid = field.string().map_err(|()| FieldError::Invalid)?;
                }
                _ => {}
            }
            Ok(())
        });
        match res {
            Ok(()) => {
                if !found_chat {
                    return Ok(None);
                }
                let Ok(ts) = time.milliseconds() else {
                    return Ok(None);
                };
                turn.timestamp_ms = ts;
                Ok(Some(turn))
            }
            Err(FieldError::Invalid) => Ok(None),
            Err(FieldError::Cancelled(e)) => Err(e),
        }
    }

    pub fn parse_step_metadata<E, C>(
        bytes: &[u8],
        mut check_cancellation: C,
    ) -> Result<Option<StepMetadata>, E>
    where
        C: FnMut() -> Result<(), E>,
    {
        let mut step_uuid: Option<String> = None;
        let mut bot_identifier = AuxiliaryIdentifier::new();
        let mut time = ProtoTimestamp::default();
        let mut timestamp_is_valid = true;

        let res = Self::fields(bytes, &mut check_cancellation, |field, cancel| {
            match field.number {
                1 => {
                    let res = field
                        .message()
                        .map_err(|()| FieldError::Invalid)
                        .and_then(|msg| Self::parse_timestamp_field(msg, &mut time, cancel));
                    match res {
                        Ok(()) => {}
                        Err(FieldError::Invalid) => {
                            timestamp_is_valid = false;
                        }
                        Err(FieldError::Cancelled(e)) => return Err(FieldError::Cancelled(e)),
                    }
                }
                9 => {
                    // Malformed auxiliary IDs disable exact matching, even if a later envelope is valid.
                    let res = field
                        .message()
                        .map_err(|()| FieldError::Invalid)
                        .and_then(|msg| {
                            Self::fields(msg, cancel, |subfield, _| {
                                if subfield.number == 7 {
                                    bot_identifier.read(&subfield);
                                }
                                Ok(())
                            })
                        });
                    match res {
                        Ok(()) => {}
                        Err(FieldError::Invalid) => {
                            bot_identifier.invalidate();
                        }
                        Err(FieldError::Cancelled(e)) => return Err(FieldError::Cancelled(e)),
                    }
                }
                12 => {
                    step_uuid = field.string().map_err(|()| FieldError::Invalid)?;
                }
                _ => {}
            }
            Ok(())
        });

        match res {
            Ok(()) => {
                let ms = if timestamp_is_valid {
                    let Ok(val) = time.milliseconds() else {
                        return Ok(None);
                    };
                    val
                } else {
                    None
                };
                Ok(Some(StepMetadata {
                    step_uuid,
                    bot_id: bot_identifier.value().map(str::to_owned),
                    timestamp_ms: ms,
                }))
            }
            Err(FieldError::Invalid) => Ok(None),
            Err(FieldError::Cancelled(e)) => Err(e),
        }
    }

    fn parse_chat<E, C>(
        bytes: &[u8],
        turn: &mut ParsedTurn,
        time: &mut ProtoTimestamp,
        check_cancellation: &mut C,
    ) -> Result<(), FieldError<E>>
    where
        C: FnMut() -> Result<(), E>,
    {
        Self::fields(bytes, check_cancellation, |field, cancel| {
            match field.number {
                4 => {
                    let mut usage = turn.usage.take().unwrap_or_default();
                    let msg = field.message().map_err(|()| FieldError::Invalid)?;
                    Self::parse_usage(msg, &mut usage, cancel)?;
                    turn.usage = Some(usage);
                }
                9 => {
                    let msg = field.message().map_err(|()| FieldError::Invalid)?;
                    Self::parse_generation(msg, time, cancel)?;
                }
                19 => {
                    turn.model = field.string().map_err(|()| FieldError::Invalid)?;
                }
                21 => {
                    turn.label = field.string().map_err(|()| FieldError::Invalid)?;
                }
                _ => {}
            }
            Ok(())
        })
    }

    fn parse_usage<E, C>(
        bytes: &[u8],
        usage: &mut ParsedUsage,
        check_cancellation: &mut C,
    ) -> Result<(), FieldError<E>>
    where
        C: FnMut() -> Result<(), E>,
    {
        Self::fields(bytes, check_cancellation, |field, _| {
            match field.number {
                1 => {
                    // Field 1 is the model enum ID (e.g. 1298 for gemini-3.7-flash), not a token count.
                    usage.model_id = Some(field.counter().map_err(|()| FieldError::Invalid)?);
                }
                2 => {
                    usage.new_input = field.counter().map_err(|()| FieldError::Invalid)?;
                }
                5 => {
                    usage.cache_read = field.counter().map_err(|()| FieldError::Invalid)?;
                }
                7 => {
                    usage.bot_identifier.read(&field);
                }
                9 => {
                    usage.reasoning = field.counter().map_err(|()| FieldError::Invalid)?;
                }
                10 => {
                    usage.output = field.counter().map_err(|()| FieldError::Invalid)?;
                }
                11 => {
                    usage.response_id = field.string().map_err(|()| FieldError::Invalid)?;
                }
                _ => {}
            }
            Ok(())
        })
    }

    fn parse_timestamp_field<E, C>(
        bytes: &[u8],
        time: &mut ProtoTimestamp,
        check_cancellation: &mut C,
    ) -> Result<(), FieldError<E>>
    where
        C: FnMut() -> Result<(), E>,
    {
        Self::fields(bytes, check_cancellation, |stamp, _| {
            match stamp.number {
                1 => {
                    let seconds = stamp.integer().map_err(|()| FieldError::Invalid)?;
                    if seconds == 0 || seconds > 253_402_300_799 {
                        return Err(FieldError::Invalid);
                    }
                    time.seconds = Some(seconds);
                }
                2 => {
                    let nanos = stamp.integer().map_err(|()| FieldError::Invalid)?;
                    if nanos > 999_999_999 {
                        return Err(FieldError::Invalid);
                    }
                    time.nanos = nanos;
                }
                _ => {}
            }
            Ok(())
        })
    }

    fn parse_generation<E, C>(
        bytes: &[u8],
        time: &mut ProtoTimestamp,
        check_cancellation: &mut C,
    ) -> Result<(), FieldError<E>>
    where
        C: FnMut() -> Result<(), E>,
    {
        Self::fields(bytes, check_cancellation, |field, cancel| {
            if field.number != 4 {
                return Ok(());
            }
            let msg = field.message().map_err(|()| FieldError::Invalid)?;
            Self::parse_timestamp_field(msg, time, cancel)
        })
    }
}
