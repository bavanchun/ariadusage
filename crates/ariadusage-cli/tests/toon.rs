// Ported from CodexBar Tests/CodexBarTests/ToonFormatterTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ToonScalarEncodingTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIToonSnapshotTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeSet, HashMap, HashSet};

use ariadusage_cli::exit_code::ExitCode;
use ariadusage_cli::options::{
    encode_json, make_cli_error_provider_payload, render_provider_payloads,
};
use ariadusage_cli::output_preferences::{CliOutputPreferences, OutputFormat};
use ariadusage_cli::provider_meta::{CliProvider, make_provider_window_labels_payload};
use ariadusage_cli::toon::ToonFormatter;
use ariadusage_protocol::{
    CliErrorKind, CliProviderIdentitySnapshot, CliRateWindow, CliUsageSnapshot, DetailRow,
    DetailSection, ProviderPayload,
};
use jiff::Timestamp;
use serde::Serialize;

fn encode_toon<T: Serialize>(value: &T) -> String {
    ToonFormatter::encode(value).unwrap_or_default()
}

#[derive(Serialize)]
struct Simple {
    name: &'static str,
    count: i32,
    ratio: f64,
    active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<&'static str>,
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:16 renders a flat object as key colon value lines
#[test]
fn renders_a_flat_object_as_key_colon_value_lines() {
    let value = Simple {
        name: "claude",
        count: 3,
        ratio: 93.0,
        active: true,
        note: None,
    };
    let toon = encode_toon(&value);

    assert!(toon.contains("name: claude"));
    assert!(toon.contains("count: 3"));
    assert!(toon.contains("ratio: 93"));
    assert!(toon.contains("active: true"));
    assert!(!toon.contains("note"));
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:28 collapses a uniform scalar array into tabular form
#[test]
fn collapses_a_uniform_scalar_array_into_tabular_form() {
    #[derive(Serialize)]
    struct Row {
        id: i32,
        label: &'static str,
    }
    #[derive(Serialize)]
    struct Wrapper {
        rows: Vec<Row>,
    }
    let value = Wrapper {
        rows: vec![
            Row {
                id: 1,
                label: "Ada",
            },
            Row {
                id: 2,
                label: "Bob",
            },
        ],
    };

    let toon = encode_toon(&value);

    assert_eq!(toon, "rows[2]{id,label}:\n  1,Ada\n  2,Bob");
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:43 falls back to list form instead of fabricating null for an omitted optional field
#[test]
fn falls_back_to_list_form_instead_of_fabricating_null_for_an_omitted_optional_field() {
    #[derive(Serialize)]
    struct Row {
        id: i32,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<&'static str>,
    }
    #[derive(Serialize)]
    struct Wrapper {
        rows: Vec<Row>,
    }
    let value = Wrapper {
        rows: vec![
            Row {
                id: 1,
                note: Some("hi"),
            },
            Row { id: 2, note: None },
        ],
    };

    let toon = encode_toon(&value);

    assert!(
        !toon.contains('{'),
        "differing field sets must not collapse into a tabular header"
    );
    assert!(
        !toon.contains("null"),
        "an omitted optional field must not be fabricated as null"
    );
    assert!(toon.contains("note: hi"));
    assert_eq!(toon, "rows[2]:\n  - id: 1\n    note: hi\n  - id: 2");
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:75 falls back to list form when array items are not uniform scalars
#[test]
fn falls_back_to_list_form_when_array_items_are_not_uniform_scalars() {
    #[derive(Serialize)]
    struct Inner {
        x: i32,
    }
    #[derive(Serialize)]
    struct Row {
        id: i32,
        inner: Inner,
    }
    #[derive(Serialize)]
    struct Wrapper {
        rows: Vec<Row>,
    }
    let value = Wrapper {
        rows: vec![
            Row {
                id: 1,
                inner: Inner { x: 9 },
            },
            Row {
                id: 2,
                inner: Inner { x: 8 },
            },
        ],
    };

    let toon = encode_toon(&value);

    assert_eq!(
        toon,
        "rows[2]:\n  - id: 1\n    inner:\n      x: 9\n  - id: 2\n    inner:\n      x: 8"
    );
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:95 renders a primitive array inline
#[test]
fn renders_a_primitive_array_inline() {
    #[derive(Serialize)]
    struct Wrapper {
        tags: Vec<&'static str>,
    }
    let toon = encode_toon(&Wrapper {
        tags: vec!["admin", "ops", "dev"],
    });

    assert_eq!(toon, "tags[3]: admin,ops,dev");
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:103 renders an empty array explicitly
#[test]
fn renders_an_empty_array_explicitly() {
    #[derive(Serialize)]
    struct Wrapper {
        tags: Vec<&'static str>,
    }
    let toon = encode_toon(&Wrapper { tags: vec![] });

    assert_eq!(toon, "tags: []");
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:111 quotes strings that collide with delimiters keywords or numeric syntax
#[test]
fn quotes_strings_that_collide_with_delimiters_keywords_or_numeric_syntax() {
    #[derive(Serialize)]
    struct Wrapper {
        a: &'static str,
        b: &'static str,
        c: &'static str,
        d: &'static str,
    }
    let value = Wrapper {
        a: "hello, world",
        b: "true",
        c: "42",
        d: "-negative",
    };
    let toon = encode_toon(&value);

    assert!(toon.contains(r#"a: "hello, world""#));
    assert!(toon.contains(r#"b: "true""#));
    assert!(toon.contains(r#"c: "42""#));
    assert!(toon.contains(r#"d: "-negative""#));
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:123 fails closed instead of fabricating zero for a NaN or infinite double
#[test]
fn fails_closed_instead_of_fabricating_zero_for_a_nan_or_infinite_double() {
    #[derive(Serialize)]
    struct Wrapper {
        #[serde(rename = "usedPercent")]
        used_percent: f64,
    }

    assert!(
        encode_toon(&Wrapper {
            used_percent: f64::NAN
        })
        .is_empty()
    );
    assert!(
        encode_toon(&Wrapper {
            used_percent: f64::INFINITY
        })
        .is_empty()
    );
    assert!(
        encode_toon(&Wrapper {
            used_percent: f64::NEG_INFINITY
        })
        .is_empty()
    );
    assert!(!encode_toon(&Wrapper { used_percent: 42.0 }).is_empty());
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:139 NaN and infinity are rejected identically by the JSON path this formatter mirrors
#[test]
fn nan_and_infinity_are_rejected_identically_by_the_json_path_this_formatter_mirrors() {
    #[derive(Serialize)]
    struct Wrapper {
        #[serde(rename = "usedPercent")]
        used_percent: f64,
    }

    assert!(
        encode_json(
            &Wrapper {
                used_percent: f64::NAN
            },
            false
        )
        .is_none()
    );
    assert!(
        encode_json(
            &Wrapper {
                used_percent: f64::INFINITY
            },
            false
        )
        .is_none()
    );
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:150 encodes dates as ISO8601 strings matching the JSON formatter
#[test]
fn encodes_dates_as_iso8601_strings_matching_the_json_formatter() {
    #[derive(Serialize)]
    struct Wrapper {
        #[serde(rename = "updatedAt")]
        updated_at: Timestamp,
    }
    let date = Timestamp::UNIX_EPOCH;
    let toon = encode_toon(&Wrapper { updated_at: date });

    assert_eq!(toon, r#"updatedAt: "1970-01-01T00:00:00Z""#);
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:161 renders provider usage payloads with tabular detail rows
#[test]
fn renders_provider_usage_payloads_with_tabular_detail_rows() {
    let section = DetailSection::new(
        Some("Usage summary"),
        vec![
            DetailRow::new(None::<&str>, "Requests", "120", None::<&str>, None, None).unwrap(),
            DetailRow::new(None::<&str>, "Tokens", "4.2k", None::<&str>, None, None).unwrap(),
        ],
        None,
    )
    .unwrap();
    let mut snapshot = CliUsageSnapshot::new(
        Some(CliRateWindow::new(7.0, Some(300), None, None)),
        None,
        None,
        "1970-01-01T00:00:00Z",
        None,
    );
    snapshot.details = vec![section];
    let payload = vec![ProviderPayload {
        provider: "claude".to_string(),
        account: None,
        version: None,
        source: "fixture".to_string(),
        status: None,
        usage: Some(snapshot),
        rate_window_labels: None,
        credits: None,
        reset_credits: None,
        antigravity_plan_info: None,
        openai_dashboard: None,
        diagnostic: None,
        error: None,
        pace: None,
        age_seconds: None,
    }];

    let toon = encode_toon(&payload);

    assert!(toon.contains("[1]:"));
    assert!(toon.contains("- provider: claude"));
    assert!(toon.contains("rows[2]{label,value}:"));
    assert!(toon.contains(r#"Requests,"120""#));
    assert!(toon.contains("Tokens,4.2k"));
    assert!(toon.contains("usedPercent: 7"));
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:199 usage --format toon resolves output preferences with toon requested
#[test]
fn usage_format_toon_resolves_output_preferences_with_toon_requested() {
    let mut options = HashMap::new();
    options.insert("format".to_string(), vec!["toon".to_string()]);
    let flags = HashSet::new();
    let output = CliOutputPreferences::from_parsed(&options, &flags);

    assert_eq!(output.format, OutputFormat::Json);
    assert!(output.toon_requested);
    assert!(output.uses_json_output());
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:211 usage --format json does not request toon
#[test]
fn usage_format_json_does_not_request_toon() {
    let mut options = HashMap::new();
    options.insert("format".to_string(), vec!["json".to_string()]);
    let flags = HashSet::new();
    let output = CliOutputPreferences::from_parsed(&options, &flags);

    assert_eq!(output.format, OutputFormat::Json);
    assert!(!output.toon_requested);
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:222 explicit toon format wins over a json shortcut in parsed values
#[test]
fn explicit_toon_format_wins_over_a_json_shortcut_in_parsed_values() {
    let mut options = HashMap::new();
    options.insert("format".to_string(), vec!["toon".to_string()]);
    let mut flags = HashSet::new();
    flags.insert("jsonShortcut".to_string());
    let output = CliOutputPreferences::from_parsed(&options, &flags);

    assert_eq!(output.format, OutputFormat::Json);
    assert!(output.toon_requested);
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:233 a later explicit format overrides an earlier toon request in parsed values
#[test]
fn a_later_explicit_format_overrides_an_earlier_toon_request_in_parsed_values() {
    let mut options_json = HashMap::new();
    options_json.insert(
        "format".to_string(),
        vec!["toon".to_string(), "json".to_string()],
    );
    let flags = HashSet::new();
    let json_wins = CliOutputPreferences::from_parsed(&options_json, &flags);

    assert_eq!(json_wins.format, OutputFormat::Json);
    assert!(!json_wins.toon_requested);

    let mut options_toon = HashMap::new();
    options_toon.insert(
        "format".to_string(),
        vec!["json".to_string(), "toon".to_string()],
    );
    let toon_wins = CliOutputPreferences::from_parsed(&options_toon, &flags);

    assert_eq!(toon_wins.format, OutputFormat::Json);
    assert!(toon_wins.toon_requested);
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:252 early CLI failures render as TOON instead of falling back to JSON
#[test]
fn early_cli_failures_render_as_toon_instead_of_falling_back_to_json() {
    let toon_output = CliOutputPreferences::with_toon(OutputFormat::Json, false, false, true);
    let error_payload = make_cli_error_provider_payload(
        "Error: --source must be auto|web|cli|oauth|api.",
        ExitCode::Failure,
        CliErrorKind::Args,
    );

    let rendered = render_provider_payloads(&[error_payload], toon_output).unwrap();

    assert!(rendered.contains("error:"));
    assert!(rendered.contains(r#"message: "Error: --source must be auto|web|cli|oauth|api.""#));
    assert!(
        !rendered.contains('{'),
        "TOON error output should not fall back to a JSON object literal"
    );
}

// upstream: Tests/CodexBarTests/ToonFormatterTests.swift:267 early CLI failures still render as JSON when toon was not requested
#[test]
fn early_cli_failures_still_render_as_json_when_toon_was_not_requested() {
    let json_output = CliOutputPreferences::new(OutputFormat::Json, false, false);
    let error_payload =
        make_cli_error_provider_payload("Nope", ExitCode::Failure, CliErrorKind::Args);

    let rendered = render_provider_payloads(&[error_payload], json_output).unwrap();

    assert!(rendered.starts_with('['));
    assert!(rendered.contains(r#""message":"Nope""#));
}

#[derive(Serialize)]
struct Scalars {
    signed: isize,
    signed8: i8,
    signed16: i16,
    signed32: i32,
    signed64: i64,
    unsigned: usize,
    unsigned8: u8,
    unsigned16: u16,
    unsigned32: u32,
    unsigned64: u64,
    float: f32,
    double: f64,
    boolean: bool,
    string: &'static str,
}

impl Default for Scalars {
    fn default() -> Self {
        Self {
            signed: isize::MIN,
            signed8: i8::MIN,
            signed16: i16::MIN,
            signed32: i32::MIN,
            signed64: i64::MIN,
            unsigned: usize::MAX,
            unsigned8: u8::MAX,
            unsigned16: u16::MAX,
            unsigned32: u32::MAX,
            unsigned64: u64::MAX,
            float: 1.25_f32,
            double: 1.25_f64,
            boolean: true,
            string: "literal, quoted",
        }
    }
}

struct ScalarArray;

impl Serialize for ScalarArray {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq;
        let value = Scalars::default();
        let mut seq = serializer.serialize_seq(Some(15))?;
        seq.serialize_element(&value.signed)?;
        seq.serialize_element(&value.signed8)?;
        seq.serialize_element(&value.signed16)?;
        seq.serialize_element(&value.signed32)?;
        seq.serialize_element(&value.signed64)?;
        seq.serialize_element(&value.unsigned)?;
        seq.serialize_element(&value.unsigned8)?;
        seq.serialize_element(&value.unsigned16)?;
        seq.serialize_element(&value.unsigned32)?;
        seq.serialize_element(&value.unsigned64)?;
        seq.serialize_element(&value.float)?;
        seq.serialize_element(&value.double)?;
        seq.serialize_element(&value.boolean)?;
        seq.serialize_element(&value.string)?;
        seq.serialize_element(&Option::<String>::None)?;
        seq.end()
    }
}

const EXPECTED_SCALARS: [&str; 14] = [
    "-9223372036854775808",
    "-128",
    "-32768",
    "-2147483648",
    "-9223372036854775808",
    "1.8446744073709552e+19",
    "255",
    "65535",
    "4294967295",
    "1.8446744073709552e+19",
    "1.25",
    "1.25",
    "true",
    r#""literal, quoted""#,
];

// upstream: Tests/CodexBarTests/ToonScalarEncodingTests.swift:51 keyed scalar overloads preserve numeric bounds and field order
#[test]
fn keyed_scalar_overloads_preserve_numeric_bounds_and_field_order() {
    let keys = [
        "signed",
        "signed8",
        "signed16",
        "signed32",
        "signed64",
        "unsigned",
        "unsigned8",
        "unsigned16",
        "unsigned32",
        "unsigned64",
        "float",
        "double",
        "boolean",
        "string",
    ];
    let expected: String = keys
        .iter()
        .zip(EXPECTED_SCALARS.iter())
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(encode_toon(&Scalars::default()), expected);
}

// upstream: Tests/CodexBarTests/ToonScalarEncodingTests.swift:62 unkeyed scalar overloads preserve numeric bounds and explicit null
#[test]
fn unkeyed_scalar_overloads_preserve_numeric_bounds_and_explicit_null() {
    let mut all: Vec<&str> = EXPECTED_SCALARS.to_vec();
    all.push("null");
    let expected = format!("[15]: {}", all.join(","));
    assert_eq!(encode_toon(&ScalarArray), expected);
}

// upstream: Tests/CodexBarTests/ToonScalarEncodingTests.swift:68 nonfinite floats fail closed in every container
#[test]
fn nonfinite_floats_fail_closed_in_every_container() {
    #[derive(Serialize)]
    struct Keyed {
        value: f32,
    }
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(encode_toon(&value).is_empty());
        assert!(encode_toon(&vec![value]).is_empty());
        assert!(encode_toon(&Keyed { value }).is_empty());
    }
}

// Rust-added: verify ToonError::NonFiniteNumber matches upstream error wording (ToonFormatter.swift:38)
#[test]
fn nonfinite_float_error_message_matches_upstream_wording() {
    let err = ToonFormatter::encode(&f64::NAN).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Unable to encode NaN in TOON: only finite numbers are representable."
    );
}

fn make_fixture_payload() -> Vec<ProviderPayload> {
    let uniform_rows = DetailSection::new(
        Some("Usage summary"),
        vec![
            DetailRow::new(None::<&str>, "Requests", "120", None::<&str>, None, None).unwrap(),
            DetailRow::new(None::<&str>, "Tokens", "4.2k", None::<&str>, None, None).unwrap(),
        ],
        None,
    )
    .unwrap();
    let mixed_rows = DetailSection::new(
        Some("Extra usage"),
        vec![
            DetailRow::new(
                None::<&str>,
                "Spend",
                "$5.00",
                Some("of $20.00"),
                None,
                None,
            )
            .unwrap(),
            DetailRow::new(None::<&str>, "Balance", "$100.00", None::<&str>, None, None).unwrap(),
        ],
        None,
    )
    .unwrap();
    let updated_at = Timestamp::from_second(1_770_000_000).unwrap();
    let mut snapshot = CliUsageSnapshot::new(
        Some(CliRateWindow::new(7.0, Some(300), None, None)),
        Some(CliRateWindow::new(42.5, Some(10080), None, None)),
        None,
        updated_at.to_string(),
        Some(CliProviderIdentitySnapshot::new(
            Some("claude".to_string()),
            Some("dev@example.com".to_string()),
            None,
            Some("OAuth".to_string()),
        )),
    );
    snapshot.details = vec![uniform_rows, mixed_rows];
    let rate_window_labels =
        make_provider_window_labels_payload(CliProvider::Claude, Some(&snapshot), updated_at);
    vec![ProviderPayload {
        provider: "claude".to_string(),
        account: Some("work".to_string()),
        version: Some("1.2.3".to_string()),
        source: "oauth".to_string(),
        status: None,
        usage: Some(snapshot),
        rate_window_labels,
        credits: None,
        reset_credits: None,
        antigravity_plan_info: None,
        openai_dashboard: None,
        diagnostic: None,
        error: None,
        pace: None,
        age_seconds: None,
    }]
}

// upstream: Tests/CodexBarTests/CLIToonSnapshotTests.swift:10 renders a fixture usage payload as an exact TOON document
#[test]
fn renders_a_fixture_usage_payload_as_an_exact_toon_document() {
    let toon = ToonFormatter::encode(&make_fixture_payload()).unwrap();

    let expected = [
        "[1]:",
        "  - provider: claude",
        "    account: work",
        "    version: 1.2.3",
        "    source: oauth",
        "    usage:",
        "      primary:",
        "        usedPercent: 7",
        "        windowMinutes: 300",
        "      secondary:",
        "        usedPercent: 42.5",
        "        windowMinutes: 10080",
        "      tertiary: null",
        "      details[2]:",
        "        - title: Usage summary",
        "          rows[2]{label,value}:",
        r#"            Requests,"120""#,
        "            Tokens,4.2k",
        "        - title: Extra usage",
        "          rows[2]:",
        "            - label: Spend",
        "              value: $5.00",
        "              secondaryValue: of $20.00",
        "            - label: Balance",
        "              value: $100.00",
        r#"      updatedAt: "2026-02-02T02:40:00Z""#,
        "      identity:",
        "        providerID: claude",
        "        accountEmail: dev@example.com",
        "        loginMethod: OAuth",
        "      accountEmail: dev@example.com",
        "      loginMethod: OAuth",
        "    rateWindowLabels:",
        "      primary: Session",
        "      secondary: Weekly",
    ]
    .join("\n");

    assert_eq!(toon, expected);
}

fn json_field_names(value: &serde_json::Value) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                names.insert(k.clone());
                names.extend(json_field_names(v));
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                names.extend(json_field_names(item));
            }
        }
        _ => {}
    }
    names
}

fn toon_field_names(toon: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for raw_line in toon.split('\n') {
        let mut line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("- ") {
            line = rest;
        }
        let Some((head, _)) = line.split_once(':') else {
            continue;
        };
        let (key_part, fields_part) = if let Some((before_brace, rest)) = head.split_once('{') {
            let fields = rest.strip_suffix('}');
            (before_brace, fields)
        } else {
            (head, None)
        };
        let key = if let Some((before_bracket, bracket_rest)) = key_part.split_once('[') {
            if bracket_rest.ends_with(']')
                && bracket_rest[..bracket_rest.len() - 1]
                    .chars()
                    .all(|c| c.is_ascii_digit())
            {
                before_bracket
            } else {
                continue;
            }
        } else {
            key_part
        };
        let mut chars = key.chars();
        let Some(first) = chars.next() else {
            continue;
        };
        if !(first.is_ascii_alphabetic() || first == '_')
            || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            continue;
        }
        names.insert(key.to_string());
        if let Some(fields) = fields_part {
            for field in fields.split(',') {
                let trimmed = field.trim();
                if !trimmed.is_empty() {
                    names.insert(trimmed.to_string());
                }
            }
        }
    }
    names
}

// upstream: Tests/CodexBarTests/CLIToonSnapshotTests.swift:57 TOON emits the same field names as the JSON encoding of the same payload
#[test]
fn toon_emits_the_same_field_names_as_the_json_encoding_of_the_same_payload() {
    let payload = make_fixture_payload();
    let json = encode_json(&payload, false).unwrap();
    let toon = ToonFormatter::encode(&payload).unwrap();

    let json_val: serde_json::Value = serde_json::from_str(&json).unwrap();
    let json_keys = json_field_names(&json_val);
    let toon_keys = toon_field_names(&toon);

    assert!(!json_keys.is_empty());
    assert_eq!(toon_keys, json_keys);
}
