// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityLocalJSONL.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use super::jsonl_object::{AntigravityJSONLObject, AntigravityJSONLValue as JsonlValue};
use super::local_reader::{AntigravityLocalReader, Event, SourceResult};
use super::local_scan::{Budget, ScanFailure};
use super::proto_reader::{ParsedTurn, ParsedUsage};

#[derive(Debug, Default)]
struct JSONLSession {
    id: Option<String>,
    model: Option<String>,
    line: i64,
}

impl AntigravityLocalReader {
    pub fn read_jsonl(paths: &[PathBuf], budget: &Budget) -> Result<SourceResult, ScanFailure> {
        let mut result = SourceResult::default();
        for url in paths {
            budget.check()?;
            let files = budget.update_statistics(|s| {
                s.files += 1;
                s.files
            });
            if files > budget.limits().databases {
                return Err(ScanFailure::Exhausted);
            }
            let source = Self::read_jsonl_file(url, budget)?;
            result.events.extend(source.events);
            result.is_complete = result.is_complete && source.is_complete;
            result.contains_history_source =
                result.contains_history_source || source.contains_history_source;
        }
        Ok(result)
    }

    fn read_jsonl_file(url: &Path, budget: &Budget) -> Result<SourceResult, ScanFailure> {
        let Ok(meta) = std::fs::metadata(url) else {
            return Ok(SourceResult::incomplete());
        };
        if !meta.is_file() {
            return Ok(SourceResult::incomplete());
        }
        let size = usize::try_from(meta.len()).unwrap_or(usize::MAX);
        // Check the known size before allocation; streaming also handles a file that grows after this check.
        if size > budget.limits().database_bytes
            || size
                > budget
                    .limits()
                    .bytes
                    .saturating_sub(budget.statistics().attempted_bytes)
        {
            budget.charge_bytes(size)?;
            return Err(ScanFailure::Exhausted);
        }
        let Ok(mut handle) = std::fs::File::open(url) else {
            return Ok(SourceResult::incomplete());
        };
        let mut result = SourceResult::default();
        let mut session = JSONLSession::default();
        let mut line: Vec<u8> = Vec::new();
        let mut file_bytes = 0usize;
        let mut chunk_buf = vec![0u8; 65_536];
        loop {
            budget.check()?;
            let remaining = budget
                .limits()
                .database_bytes
                .saturating_sub(file_bytes)
                .min(
                    budget
                        .limits()
                        .bytes
                        .saturating_sub(budget.statistics().attempted_bytes),
                );
            let max_count = 65_536usize.min(remaining.saturating_add(1));
            if chunk_buf.len() < max_count {
                chunk_buf.resize(max_count, 0);
            }
            let read_count = match Self::read_up_to(&mut handle, &mut chunk_buf[..max_count]) {
                Ok(n) => n,
                Err(()) => {
                    result.is_complete = false;
                    break;
                }
            };
            if read_count == 0 {
                break;
            }
            let chunk = &chunk_buf[..read_count];
            budget.charge_bytes(chunk.len())?;
            file_bytes += chunk.len();
            if file_bytes > budget.limits().database_bytes {
                return Err(ScanFailure::Exhausted);
            }
            for (index, &byte) in chunk.iter().enumerate() {
                if index % 4096 == 0 {
                    budget.check()?;
                }
                if byte == b'\n' {
                    Self::consume_jsonl_line(&line, &mut session, &mut result, budget)?;
                    line.clear();
                } else {
                    if line.len() >= budget.limits().blob_bytes {
                        return Err(ScanFailure::Exhausted);
                    }
                    line.push(byte);
                }
            }
        }
        if !line.is_empty() {
            Self::consume_jsonl_line(&line, &mut session, &mut result, budget)?;
        }
        result.contains_history_source = session.id.is_some();
        if session.id.is_none() {
            result.is_complete = false;
        }
        Ok(result)
    }

    fn read_up_to(handle: &mut std::fs::File, buf: &mut [u8]) -> Result<usize, ()> {
        let mut total = 0usize;
        while total < buf.len() {
            match handle.read(&mut buf[total..]) {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(()),
            }
        }
        Ok(total)
    }

    fn consume_jsonl_line(
        line: &[u8],
        session: &mut JSONLSession,
        result: &mut SourceResult,
        budget: &Budget,
    ) -> Result<(), ScanFailure> {
        budget.charge_row()?;
        session.line += 1;
        if usize::try_from(session.line).unwrap_or(usize::MAX) > budget.limits().rows_per_database {
            return Err(ScanFailure::Exhausted);
        }
        if line.iter().all(|&b| b == b'\r' || b == b' ' || b == b'\t') {
            return Ok(());
        }
        let Some(json) = AntigravityJSONLObject::decode(line, || budget.check())? else {
            result.is_complete = false;
            return Ok(());
        };
        let outcome: Result<(), ScanFailure> = (|| {
            let Some(JsonlValue::String(record_type)) = json.get("type") else {
                return Err(ScanFailure::Invalid);
            };
            if record_type == "session_meta" {
                let id = Self::optional_string(&json, &["sessionId"])?;
                let Some(id) = id else {
                    return Err(ScanFailure::Invalid);
                };
                if session.id.as_ref().is_some_and(|existing| existing != &id) {
                    return Err(ScanFailure::Invalid);
                }
                session.id = Some(id);
                session.model = Self::optional_string(&json, &["modelId", "model_id"])?;
                return Ok(());
            }
            if record_type != "usage" {
                return Err(ScanFailure::Invalid);
            }
            let id = Self::optional_string(&json, &["sessionId"])?.or_else(|| session.id.clone());
            let Some(id) = id else {
                return Err(ScanFailure::Invalid);
            };
            if session.id.as_ref().is_some_and(|existing| existing != &id) {
                return Err(ScanFailure::Invalid);
            }
            let Some(timestamp) = Self::optional_timestamp(json.get("timestamp"))? else {
                return Err(ScanFailure::Invalid);
            };
            let mut usage = ParsedUsage {
                new_input: Self::json_counter(&json, &["input"], true)?,
                output: Self::json_counter(&json, &["output"], true)?,
                cache_read: Self::json_counter(&json, &["cacheRead", "cache_read"], false)?,
                reasoning: Self::json_counter(&json, &["reasoning"], false)?,
                ..ParsedUsage::default()
            };
            // The producer names both counters but does not establish their overlap. Do not guess an exact total.
            if usage.reasoning != 0 {
                return Err(ScanFailure::Invalid);
            }
            usage.response_id = Self::optional_string(&json, &["responseId", "response_id"])?;
            let write = Self::json_counter(&json, &["cacheWrite", "cache_write"], false)?;
            let model = Self::optional_string(&json, &["modelId", "model_id"])?
                .or_else(|| session.model.clone());
            let turn = ParsedTurn {
                step_uuid: None,
                usage: Some(usage),
                timestamp_ms: Some(timestamp),
                model,
                label: None,
            };
            let Some(event) = Event::new(id.clone(), session.line, turn, write) else {
                return Err(ScanFailure::Invalid);
            };
            session.id = Some(id);
            result.events.push(event);
            Ok(())
        })();
        if outcome.is_err() {
            result.is_complete = false;
        }
        Ok(())
    }

    fn exact_integer(value: &JsonlValue) -> Result<i64, ScanFailure> {
        let JsonlValue::Number(number) = value else {
            return Err(ScanFailure::Invalid);
        };
        number.integer.ok_or(ScanFailure::Invalid)
    }

    fn json_counter(
        json: &BTreeMap<String, JsonlValue>,
        keys: &[&str],
        required: bool,
    ) -> Result<usize, ScanFailure> {
        let mut counter: Option<usize> = None;
        for &key in keys {
            let Some(value) = json.get(key) else {
                continue;
            };
            let integer = Self::exact_integer(value)?;
            let Ok(non_negative) = usize::try_from(integer) else {
                return Err(ScanFailure::Invalid);
            };
            if counter.is_some_and(|existing| existing != non_negative) {
                return Err(ScanFailure::Invalid);
            }
            counter = Some(non_negative);
        }
        if required && counter.is_none() {
            return Err(ScanFailure::Invalid);
        }
        Ok(counter.unwrap_or(0))
    }

    fn optional_string(
        json: &BTreeMap<String, JsonlValue>,
        keys: &[&str],
    ) -> Result<Option<String>, ScanFailure> {
        let mut text: Option<String> = None;
        for &key in keys {
            let Some(value) = json.get(key) else {
                continue;
            };
            if matches!(value, JsonlValue::Null) {
                continue;
            }
            let JsonlValue::String(s) = value else {
                return Err(ScanFailure::Invalid);
            };
            if s.trim().is_empty() {
                continue;
            }
            if text.as_ref().is_some_and(|existing| existing != s) {
                return Err(ScanFailure::Invalid);
            }
            text = Some(s.clone());
        }
        Ok(text)
    }

    fn optional_timestamp(value: Option<&JsonlValue>) -> Result<Option<i64>, ScanFailure> {
        let Some(value) = value else {
            return Ok(None);
        };
        if matches!(value, JsonlValue::Null) {
            return Ok(None);
        }
        let timestamp = Self::exact_integer(value)?;
        if timestamp <= 0 || timestamp > 253_402_300_799_999 {
            return Err(ScanFailure::Invalid);
        }
        Ok(Some(timestamp))
    }
}
