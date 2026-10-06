// Ported from CodexBar TestsLinux/SettingsValueTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::settings_value::SettingsValue;

#[test]
fn settings_trim_whitespace_and_unwrap_one_matching_quote_layer() {
    let cases: &[(Option<&str>, Option<&str>)] = &[
        (None, None),
        (Some(""), None),
        (Some(" \n\t "), None),
        (Some("\""), None),
        (Some("'"), None),
        (Some("\"\""), None),
        (Some("''"), None),
        (Some("  \" \t \"  "), None),
        (Some("  fixture-token  "), Some("fixture-token")),
        (Some(" \" fixture-token \" "), Some("fixture-token")),
        (Some(" ' fixture-token ' "), Some("fixture-token")),
        (Some("\"nested 'quotes'\""), Some("nested 'quotes'")),
        (Some("\"\"fixture\"\""), Some("\"fixture\"")),
        (Some("\"mismatched'"), Some("\"mismatched'")),
        (Some("line one\nline two"), Some("line one\nline two")),
        (Some(" '\u{1F99E}' "), Some("\u{1F99E}")),
    ];

    for (raw, expected) in cases {
        assert_eq!(
            SettingsValue::cleaned(*raw).as_deref(),
            *expected,
            "Failed for input: {:?}",
            raw
        );
    }
}
