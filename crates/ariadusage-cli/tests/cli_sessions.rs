// Ported from CodexBar Tests/CodexBarTests/AgentSessionJSONTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{HashMap, HashSet};

use ariadusage_cli::{
    SessionsOptions, execute_sessions_focus_with_scanner, render_sessions_table, sessions_for_json,
    sessions_help, sessions_json_protocol_version,
};
use ariadusage_engine::sessions::{
    AgentSession, AgentSessionDialect, AgentSessionProvider, AgentSessionSource, AgentSessionState,
    LocalAgentSessionScanner,
};
use jiff::Timestamp;
use serde::Deserialize;

fn make_session(
    id: &str,
    provider: AgentSessionProvider,
    dialect: Option<AgentSessionDialect>,
) -> AgentSession {
    AgentSession {
        id: id.to_owned(),
        provider,
        dialect,
        source: AgentSessionSource::Cli,
        state: AgentSessionState::Active,
        pid: Some(42),
        cwd: Some("/tmp/project".to_owned()),
        project_name: Some("project".to_owned()),
        session_name: None,
        started_at: Some(Timestamp::from_second(100).expect("ts")),
        last_activity_at: Some(Timestamp::from_second(200).expect("ts")),
        transcript_path: None,
        host: "local-mac".to_owned(),
    }
}

fn make_protocol_fixture() -> Vec<AgentSession> {
    vec![
        make_session("codex-session", AgentSessionProvider::Codex, None),
        make_session("claude-session", AgentSessionProvider::Claude, None),
        make_session(
            "omp-session",
            AgentSessionProvider::Pi,
            Some(AgentSessionDialect::Omp),
        ),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LegacyProvider {
    Codex,
    Claude,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct LegacyAgentSession {
    provider: LegacyProvider,
}

// upstream: AgentSessionJSONTests.swift:8 sessions json round trip preserves stable schema
#[test]
fn sessions_json_round_trip_preserves_stable_schema() {
    let session = AgentSession {
        id: "fixture-session".to_owned(),
        provider: AgentSessionProvider::Codex,
        dialect: None,
        source: AgentSessionSource::Ide,
        state: AgentSessionState::Active,
        pid: Some(42),
        cwd: Some("/tmp/project".to_owned()),
        project_name: Some("project".to_owned()),
        session_name: Some("Fix session labels".to_owned()),
        started_at: Some(Timestamp::from_second(100).expect("ts")),
        last_activity_at: Some(Timestamp::from_second(200).expect("ts")),
        transcript_path: Some("/tmp/rollout.jsonl".to_owned()),
        host: "local-mac".to_owned(),
    };

    let data = serde_json::to_vec(&vec![session.clone()]).expect("encode sessions");
    let object: Vec<serde_json::Map<String, serde_json::Value>> =
        serde_json::from_slice(&data).expect("decode json map");
    assert_eq!(object.len(), 1);
    assert!(!object.is_empty());
    let keys: HashSet<&str> = object[0].keys().map(String::as_str).collect();
    let expected: HashSet<&str> = [
        "id",
        "provider",
        "source",
        "state",
        "pid",
        "cwd",
        "projectName",
        "sessionName",
        "startedAt",
        "lastActivityAt",
        "transcriptPath",
        "host",
    ]
    .into_iter()
    .collect();
    assert_eq!(keys, expected);

    let decoded: Vec<AgentSession> = serde_json::to_vec(&vec![session.clone()])
        .and_then(|bytes| serde_json::from_slice(&bytes))
        .expect("decode AgentSession");
    assert_eq!(decoded, vec![session.clone()]);

    assert!(!object.is_empty());
    let mut legacy_object = object[0].clone();
    legacy_object.remove("sessionName");
    let legacy_data = serde_json::to_vec(&vec![legacy_object]).expect("encode legacy");
    let legacy_decoded: Vec<AgentSession> =
        serde_json::from_slice(&legacy_data).expect("decode legacy AgentSession");
    assert_eq!(legacy_decoded.len(), 1);
    assert_eq!(legacy_decoded[0].session_name, None);
    assert_eq!(legacy_decoded[0].id, session.id);
}

// upstream: AgentSessionJSONTests.swift:44 legacy v1 JSON excludes Pi-family sessions and remains decodable by closed provider clients
#[test]
fn legacy_v1_json_excludes_pi_family_sessions_and_remains_decodable_by_closed_provider_clients() {
    let sessions = make_protocol_fixture();
    let legacy_sessions = sessions_for_json(&sessions, false);
    assert_eq!(
        legacy_sessions
            .iter()
            .map(|s| s.provider)
            .collect::<Vec<_>>(),
        vec![AgentSessionProvider::Codex, AgentSessionProvider::Claude]
    );

    let legacy_data = serde_json::to_vec(&legacy_sessions).expect("encode legacy");
    let decoded: Vec<LegacyAgentSession> =
        serde_json::from_slice(&legacy_data).expect("decode LegacyAgentSession");
    assert_eq!(
        decoded.into_iter().map(|s| s.provider).collect::<Vec<_>>(),
        vec![LegacyProvider::Codex, LegacyProvider::Claude]
    );
}

// upstream: AgentSessionJSONTests.swift:55 versioned JSON flags preserve legacy compatibility and expose Pi-family sessions only in v2
#[test]
fn versioned_json_flags_preserve_legacy_compatibility_and_expose_pi_family_sessions_only_in_v2() {
    let sessions = make_protocol_fixture();
    let expectations = [("--json", 1_u8, false), ("--json-v2", 2_u8, true)];

    for (flag, expected_version, includes_pi_family) in expectations {
        let parsed = SessionsOptions::parse_from_slice(&[flag]).expect("parse flag");
        let protocol_version = sessions_json_protocol_version(&parsed);
        assert_eq!(protocol_version, Some(expected_version));

        let current_sessions = sessions_for_json(&sessions, protocol_version == Some(2));
        let current_data = serde_json::to_vec(&current_sessions).expect("encode sessions");
        let decoded: Vec<AgentSession> =
            serde_json::from_slice(&current_data).expect("decode sessions");

        let expected: Vec<AgentSession> = if includes_pi_family {
            sessions.clone()
        } else {
            sessions
                .iter()
                .filter(|s| s.provider != AgentSessionProvider::Pi)
                .cloned()
                .collect()
        };
        assert_eq!(decoded, expected);
        assert_eq!(
            decoded
                .iter()
                .any(|s| s.provider == AgentSessionProvider::Pi),
            includes_pi_family
        );
    }
}

// upstream: AgentSessionJSONTests.swift:82 mixed-version clients decode the repaired protocol in both upgrade directions
#[test]
fn mixed_version_clients_decode_the_repaired_protocol_in_both_upgrade_directions() {
    let sessions = make_protocol_fixture();
    let new_host_legacy_flag = SessionsOptions::parse_from_slice(&["--json"]).expect("parse");
    assert_eq!(
        sessions_json_protocol_version(&new_host_legacy_flag),
        Some(1)
    );

    let new_host_legacy_sessions = sessions_for_json(&sessions, false);
    let new_host_legacy_data =
        serde_json::to_vec(&new_host_legacy_sessions).expect("encode legacy");
    let old_client_sessions: Vec<LegacyAgentSession> =
        serde_json::from_slice(&new_host_legacy_data).expect("decode old client");
    assert_eq!(
        old_client_sessions
            .into_iter()
            .map(|s| s.provider)
            .collect::<Vec<_>>(),
        vec![LegacyProvider::Codex, LegacyProvider::Claude]
    );

    let old_host_legacy_data = serde_json::to_vec(
        &sessions
            .iter()
            .filter(|s| s.provider != AgentSessionProvider::Pi)
            .cloned()
            .collect::<Vec<_>>(),
    )
    .expect("encode old host");
    let new_client_sessions: Vec<AgentSession> =
        serde_json::from_slice(&old_host_legacy_data).expect("decode new client");
    assert_eq!(new_client_sessions, new_host_legacy_sessions);
}

// upstream: AgentSessionJSONTests.swift:103 human-readable sessions table includes Pi-family dialect tags
#[test]
fn human_readable_sessions_table_includes_pi_family_dialect_tags() {
    let table = render_sessions_table(
        &make_protocol_fixture(),
        Timestamp::from_second(250).expect("ts"),
    );
    assert!(table.contains("DIALECT"));
    assert!(table.contains("omp"));
}

// Rust-added: verifies `sessions focus` exit codes (1 for missing/unknown id, 2 for unsupported focus)
#[test]
fn sessions_focus_rejects_missing_or_unknown_session_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let scanner = LocalAgentSessionScanner::with_providers(
        |_env| "4242 1 Mon Jul 6 09:03:00 2026 /usr/local/bin/claude".to_owned(),
        |_pids, _env| HashMap::from([(4242, "/tmp/demo-project".to_owned())]),
    );
    let env = HashMap::from([
        (
            "HOME".to_owned(),
            temp.path().to_string_lossy().into_owned(),
        ),
        ("HOSTNAME".to_owned(), "test-host".to_owned()),
    ]);

    assert_eq!(
        execute_sessions_focus_with_scanner(None, &scanner, &env),
        Err((1, "Missing session id.".to_owned()))
    );
    assert_eq!(
        execute_sessions_focus_with_scanner(Some("nonexistent-session-id-99999"), &scanner, &env),
        Err((
            1,
            "Unknown session: nonexistent-session-id-99999".to_owned()
        ))
    );
    assert_eq!(
        execute_sessions_focus_with_scanner(Some("pid:4242"), &scanner, &env),
        Err((
            2,
            "Session focus is unsupported on this frontend.".to_owned()
        ))
    );
}

// Rust-added: verifies AriadUsage branding in `sessions_help`
#[test]
fn sessions_help_uses_ariadusage_branding() {
    let help = sessions_help("0.1.0");
    assert!(help.starts_with("AriadUsage 0.1.0\n"));
    assert!(help.contains("ariadusage sessions [--json|--json-v2] [--pretty]"));
    assert!(help.contains("ariadusage sessions focus <id>"));
    assert!(!help.contains("CodexBar"));
    assert!(!help.contains("codexbar"));
}
