// Ported from CodexBar Tests/CodexBarTests/AgentSessionParserTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AgentSessionExecutableTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeSessionMappingTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/DirectoryMetadataScanBudgetTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexSessionRolloutTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AdaptiveRolloutActivityTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/PiFamilySessionTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use ariadusage_core::refresh::{
    AdaptiveRefreshInput, AdaptiveRefreshPolicyCore, AdaptiveRefreshReason, ThermalPressure,
};
use ariadusage_engine::sessions::{
    AgentPSOutputParser, AgentProcessRecord, AgentSessionCorrelation, AgentSessionDialect,
    AgentSessionProvider, AgentSessionSource, AgentSessionState, ChatGPTCodexProcessTrust,
    ClaudeDesktopProjectsLocator, ClaudeSessionProjectMapper, ClaudeTranscript,
    CodexRolloutFirstLineParser, CodexRolloutMetadata, CodexThreadMetadata,
    DirectoryMetadataScanBudget, FutureModificationDateClamp, LSOFCWDOutputParser,
    LocalAgentSessionScanner, SessionScanConfig,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use rusqlite::Connection;

fn fixture_path(name: &str) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("crates dir")
        .parent()
        .expect("workspace root")
        .join("fixtures")
        .join("codexbar")
        .join(name)
}

fn fixture_string(name: &str) -> String {
    let path = fixture_path(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", path.display()))
}

fn set_modified(path: &Path, ts: Timestamp) {
    let millis = ts.as_millisecond();
    let st = if millis >= 0 {
        UNIX_EPOCH + Duration::from_millis(millis as u64)
    } else {
        UNIX_EPOCH
    };
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open file for set_modified");
    file.set_modified(st).expect("set_modified");
}

fn format_day_dir(now: Timestamp) -> PathBuf {
    let tz = TimeZone::try_system().unwrap_or(TimeZone::UTC);
    let date = now.to_zoned(tz).date();
    PathBuf::from(format!("{:04}", date.year()))
        .join(format!("{:02}", date.month()))
        .join(format!("{:02}", date.day()))
}

// ============================================================================
// 1. AgentSessionParserTests.swift (13/13)
// ============================================================================

// upstream: AgentSessionParserTests.swift:6 ps parser deduplicates desktop wrapper and excludes app server and helpers
#[test]
fn ps_parser_deduplicates_desktop_wrapper_and_excludes_app_server_and_helpers() {
    let output = fixture_string("agent-sessions-ps.txt");
    let records = AgentPSOutputParser::parse(&output);
    let agents = AgentPSOutputParser::agent_processes(&records);

    assert_eq!(records.len(), 18);
    assert_eq!(
        agents.iter().map(|r| r.pid).collect::<Vec<_>>(),
        vec![102, 201, 501, 502, 509]
    );
    assert_eq!(
        AgentPSOutputParser::provider(&agents[0]),
        Some(AgentSessionProvider::Claude)
    );
    assert_eq!(
        AgentPSOutputParser::source(&agents[0]),
        AgentSessionSource::DesktopApp
    );
    assert_eq!(
        AgentPSOutputParser::provider(&agents[1]),
        Some(AgentSessionProvider::Codex)
    );
    assert!(agents[1].command.ends_with("strange argv here"));
    assert!(AgentPSOutputParser::has_codex_app_server(&records));
    assert_eq!(
        AgentPSOutputParser::provider(&agents[2]),
        Some(AgentSessionProvider::Pi)
    );
    assert_eq!(
        AgentPSOutputParser::pi_dialect(&agents[2]),
        Some(AgentSessionDialect::Omp)
    );
    assert_eq!(
        AgentPSOutputParser::source(&agents[2]),
        AgentSessionSource::Cli
    );
    assert_eq!(
        AgentPSOutputParser::provider(&agents[3]),
        Some(AgentSessionProvider::Pi)
    );
    assert_eq!(
        AgentPSOutputParser::pi_dialect(&agents[3]),
        Some(AgentSessionDialect::Omp)
    );
    assert_eq!(
        AgentPSOutputParser::source(&agents[3]),
        AgentSessionSource::Cli
    );
    assert_eq!(
        AgentPSOutputParser::provider(&agents[4]),
        Some(AgentSessionProvider::Pi)
    );
    assert_eq!(
        AgentPSOutputParser::pi_dialect(&agents[4]),
        Some(AgentSessionDialect::Pi)
    );

    let unrelated_bun = records.iter().find(|r| r.pid == 507);
    let unrelated_npm = records.iter().find(|r| r.pid == 508);
    assert!(unrelated_bun.is_some());
    assert!(unrelated_npm.is_some());
    assert_eq!(AgentPSOutputParser::provider(unrelated_bun.unwrap()), None);
    assert_eq!(AgentPSOutputParser::provider(unrelated_npm.unwrap()), None);

    for helper_pid in [503, 504, 505, 506] {
        let helper = records.iter().find(|r| r.pid == helper_pid);
        assert!(helper.is_some());
        let helper = helper.unwrap();
        assert_eq!(AgentPSOutputParser::provider(helper), None);
        assert_eq!(AgentPSOutputParser::source(helper), AgentSessionSource::Cli);
    }
}

// upstream: AgentSessionParserTests.swift:38 chatgpt bundled codex app server is recognized without becoming a live agent
#[test]
fn chatgpt_bundled_codex_app_server_is_recognized_without_becoming_a_live_agent() {
    let records = AgentPSOutputParser::parse(
        "4234 1 Mon Jul 6 09:03:00 2026 /Applications/ChatGPT.app/Contents/Resources/codex -c features.code_mode_host=true app-server --analytics-default-enabled\n\
         20409 1 Mon Jul 6 09:04:00 2026 /Applications/ChatGPT.app/Contents/Resources/codex-code-mode-host",
    );

    assert_eq!(records.len(), 2);
    assert!(AgentPSOutputParser::agent_processes(&records).is_empty());
    assert!(AgentPSOutputParser::has_codex_app_server(&records));
}

// upstream: AgentSessionParserTests.swift:51 pi dialects use stable Codable raw values
#[test]
fn pi_dialects_use_stable_codable_raw_values() {
    let provider = serde_json::to_string(&AgentSessionProvider::Pi).unwrap();
    let dialects =
        serde_json::to_string(&vec![AgentSessionDialect::Pi, AgentSessionDialect::Omp]).unwrap();

    assert_eq!(provider, "\"pi\"");
    assert_eq!(dialects, "[\"pi\",\"omp\"]");
    assert_eq!(
        serde_json::from_str::<AgentSessionProvider>(&provider).unwrap(),
        AgentSessionProvider::Pi
    );
}

// upstream: AgentSessionParserTests.swift:61 lsof parser maps batched cwd records
#[test]
fn lsof_parser_maps_batched_cwd_records() {
    let output = fixture_string("agent-sessions-lsof.txt");
    let paths = LSOFCWDOutputParser::parse(&output);

    assert_eq!(
        paths.get(&102).map(String::as_str),
        Some("/Users/test/Projects/alpha")
    );
    assert_eq!(
        paths.get(&201).map(String::as_str),
        Some("/Users/test/Projects/project with spaces")
    );
}

// upstream: AgentSessionParserTests.swift:70 same cwd processes remain uncorrelated when ownership is ambiguous
#[test]
fn same_cwd_processes_remain_uncorrelated_when_ownership_is_ambiguous() {
    let older_start = Timestamp::from_second(100).unwrap();
    let newer_start = Timestamp::from_second(200).unwrap();
    let older = AgentProcessRecord::new(10, 1, Some(older_start), "claude");
    let newer = AgentProcessRecord::new(20, 1, Some(newer_start), "claude");
    let older_transcript =
        ClaudeTranscript::new("/tmp/older.jsonl", Timestamp::from_second(150).unwrap());
    let newer_transcript =
        ClaudeTranscript::new("/tmp/newer.jsonl", Timestamp::from_second(250).unwrap());

    let cwd_by_pid = HashMap::from([(10, "/project".to_owned()), (20, "/project".to_owned())]);
    let transcripts_by_cwd = HashMap::from([(
        "/project".to_owned(),
        vec![newer_transcript, older_transcript],
    )]);

    let assignments = AgentSessionCorrelation::assign_claude_transcripts(
        &[older, newer],
        &cwd_by_pid,
        &transcripts_by_cwd,
    );

    assert!(assignments.is_empty());
}

// upstream: AgentSessionParserTests.swift:91 newest process sorts first for rollout correlation
#[test]
fn newest_process_sorts_first_for_rollout_correlation() {
    let older = AgentProcessRecord::new(
        10,
        1,
        Some(Timestamp::from_second(100).unwrap()),
        "codex exec",
    );
    let newer = AgentProcessRecord::new(
        20,
        1,
        Some(Timestamp::from_second(200).unwrap()),
        "codex exec",
    );

    let sorted = AgentSessionCorrelation::newest_processes_first(&[older, newer]);
    assert_eq!(sorted.len(), 2);
    assert_eq!(
        sorted.iter().map(|r| r.pid).collect::<Vec<_>>(),
        vec![20, 10]
    );
}

const NESTED_CHATGPT_CODEX: &str =
    "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex";

// upstream: AgentSessionParserTests.swift:121 signed process validates outer ChatGPT bundle rather than nested CLI bundle
#[test]
fn signed_process_validates_outer_chatgpt_bundle_rather_than_nested_cli_bundle() {
    for path in [
        "/Applications/ChatGPT.app/Contents/Resources/codex",
        NESTED_CHATGPT_CODEX,
    ] {
        let trusted = ChatGPTCodexProcessTrust::is_trusted_with(
            123,
            |pid| {
                assert_eq!(pid, 123);
                Some(path.to_owned())
            },
            |p| p,
            |pid| pid == 123,
            |bundle| {
                assert_eq!(bundle, "/Applications/ChatGPT.app");
                true
            },
        );
        assert!(trusted);
    }
}

// upstream: AgentSessionParserTests.swift:141 claimed command cannot replace kernel executable identity
#[test]
fn claimed_command_cannot_replace_kernel_executable_identity() {
    let cases: [Option<&str>; 4] = [
        None,
        Some("/tmp/codex"),
        Some("/Users/test/Applications/ChatGPT.app/Contents/Resources/codex"),
        Some("/Applications/ChatGPT-copy.app/Contents/Resources/codex"),
    ];
    for actual_path in cases {
        let mut called_signature = false;
        assert!(!ChatGPTCodexProcessTrust::is_trusted_with(
            123,
            |_| actual_path.map(ToOwned::to_owned),
            |p| p,
            |_| {
                called_signature = true;
                true
            },
            |_| true,
        ));
        assert!(!called_signature);
    }
}

// upstream: AgentSessionParserTests.swift:159 running signature and outer bundle assessment must both succeed
#[test]
fn running_signature_and_outer_bundle_assessment_must_both_succeed() {
    for process_trusted in [false, true] {
        for bundle_trusted in [false, true] {
            let trusted = ChatGPTCodexProcessTrust::is_trusted_with(
                123,
                |_| Some(NESTED_CHATGPT_CODEX.to_owned()),
                |p| p,
                |_| process_trusted,
                |_| bundle_trusted,
            );
            assert_eq!(trusted, process_trusted && bundle_trusted);
        }
    }
}

// upstream: AgentSessionParserTests.swift:170 symlink redirects cannot authorize scanning even with trusted signatures
#[test]
fn symlink_redirects_cannot_authorize_scanning_even_with_trusted_signatures() {
    for resolved_path in [
        "/Users/test/Downloads/codex",
        "/tmp/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
        "/Applications/ChatGPT.app/Contents/Resources/other-codex",
    ] {
        assert!(!ChatGPTCodexProcessTrust::is_trusted_with(
            123,
            |_| Some(NESTED_CHATGPT_CODEX.to_owned()),
            |_| resolved_path.to_owned(),
            |_| true,
            |_| true,
        ));
    }
}

// upstream: AgentSessionParserTests.swift:184 untrusted first candidate cannot hide a later trusted app server
#[test]
fn untrusted_first_candidate_cannot_hide_a_later_trusted_app_server() {
    let records = AgentPSOutputParser::parse(&format!(
        "123 1 Mon Jul 6 09:03:00 2026 {NESTED_CHATGPT_CODEX} app-server\n\
         124 1 Mon Jul 6 09:03:00 2026 /Applications/ChatGPT.app/Contents/Resources/codex app-server"
    ));
    assert!(AgentPSOutputParser::has_trusted_chatgpt_codex_app_server(
        &records,
        |r| r.pid == 124
    ));
}

// upstream: AgentSessionParserTests.swift:193 nested executable requires app server argument and cannot bypass trust as a CLI
#[test]
fn nested_executable_requires_app_server_argument_and_cannot_bypass_trust_as_a_cli() {
    for argument in ["exec", "app-server-helper", "--help"] {
        let records = AgentPSOutputParser::parse(&format!(
            "123 1 Mon Jul 6 09:03:00 2026 {NESTED_CHATGPT_CODEX} {argument}"
        ));
        let mut called_validator = false;
        assert!(!AgentPSOutputParser::has_trusted_chatgpt_codex_app_server(
            &records,
            |_| {
                called_validator = true;
                true
            }
        ));
        assert!(!called_validator);
        assert!(AgentPSOutputParser::agent_processes(&records).is_empty());
    }
}

// upstream: AgentSessionParserTests.swift:203 forged app server command cannot borrow installed ChatGPT identity
#[tokio::test]
async fn forged_app_server_command_cannot_borrow_installed_chatgpt_identity() {
    let mut sleeper = std::process::Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .expect("spawn /bin/sleep");
    let pid = sleeper.id() as i32;
    let records = AgentPSOutputParser::parse(&format!(
        "{pid} 1 Mon Jul 6 09:03:00 2026 {NESTED_CHATGPT_CODEX} app-server"
    ));
    let scanner = LocalAgentSessionScanner::with_providers(
        move |_| format!("{pid} 1 Mon Jul 6 09:03:00 2026 {NESTED_CHATGPT_CODEX} app-server"),
        |_, _| HashMap::new(),
    );
    let sessions = scanner
        .scan_with_env(Timestamp::now(), &HashMap::new(), false)
        .await;
    let _ = sleeper.kill();
    let _ = sleeper.wait();
    assert!(records.len() == 1 && sessions.is_empty());
}

// ============================================================================
// 2. AgentSessionExecutableTests.swift (7/7)
// ============================================================================

fn make_record(arguments: &[&str], pid: i32, has_arguments: bool) -> AgentProcessRecord {
    let args_vec: Vec<String> = arguments.iter().map(|s| (*s).to_owned()).collect();
    AgentProcessRecord::new(pid, 1, None, args_vec.join(" "))
        .with_arguments(has_arguments.then_some(args_vec))
}

// upstream: AgentSessionExecutableTests.swift:6 preserved CLI arguments can mention application bundle paths
#[test]
fn preserved_cli_arguments_can_mention_application_bundle_paths() {
    for provider in ["codex", "claude"] {
        let executable = format!("/usr/local/bin/{provider}");
        let flag = if provider == "codex" { "exec" } else { "-p" };
        let record = make_record(
            &[
                &executable,
                flag,
                "Inspect /Applications/Example.app/Contents/Info.plist",
            ],
            101,
            true,
        );

        assert_eq!(
            AgentPSOutputParser::agent_processes(std::slice::from_ref(&record))
                .iter()
                .map(|r| r.pid)
                .collect::<Vec<_>>(),
            vec![101]
        );
        assert_eq!(
            AgentPSOutputParser::provider(&record).map(|p| p.as_str()),
            Some(provider)
        );
        assert_eq!(
            AgentPSOutputParser::source(&record),
            AgentSessionSource::Cli
        );
    }
}

// upstream: AgentSessionExecutableTests.swift:19 preserved CLI prompts cannot change provider or desktop source
#[test]
fn preserved_cli_prompts_cannot_change_provider_or_desktop_source() {
    for provider in ["codex", "claude"] {
        let executable = format!("/usr/local/bin/{provider}");
        let prompt = "Compare /Applications/Example.app/Contents with ~/Library/Application Support/Claude/claude-code/claude";
        let record = make_record(&[&executable, prompt], 101, true);

        assert_eq!(record.executable_basename(), provider);
        assert_eq!(
            AgentPSOutputParser::agent_processes(std::slice::from_ref(&record))
                .iter()
                .map(|r| r.pid)
                .collect::<Vec<_>>(),
            vec![101]
        );
        assert_eq!(
            AgentPSOutputParser::provider(&record).map(|p| p.as_str()),
            Some(provider)
        );
        assert_eq!(
            AgentPSOutputParser::source(&record),
            AgentSessionSource::Cli
        );
    }
}

// upstream: AgentSessionExecutableTests.swift:36 preserved wrapper arguments keep their existing source classification
#[test]
fn preserved_wrapper_arguments_keep_their_existing_source_classification() {
    for desktop in [false, true] {
        let executable = if desktop {
            "/synthetic/Library/Application Support/Claude/claude-code/claude"
        } else {
            "/usr/local/bin/claude"
        };
        let record = make_record(&["/usr/bin/disclaimer", executable], 101, true);

        assert_eq!(
            AgentPSOutputParser::agent_processes(std::slice::from_ref(&record))
                .iter()
                .map(|r| r.pid)
                .collect::<Vec<_>>(),
            vec![101]
        );
        assert_eq!(
            AgentPSOutputParser::source(&record),
            if desktop {
                AgentSessionSource::DesktopApp
            } else {
                AgentSessionSource::Cli
            }
        );
    }
}

// upstream: AgentSessionExecutableTests.swift:47 bundle executable policies remain distinct from prompt text
#[test]
fn bundle_executable_policies_remain_distinct_from_prompt_text() {
    let executables = [
        "/usr/local/bin/codex",
        "/usr/local/bin/claude",
        "/Applications/Codex.app/Contents/Resources/codex",
        "/Applications/ChatGPT.app/Contents/Resources/codex",
        "/Applications/Other.app/Contents/MacOS/codex",
        "/Applications/Claude.app/Contents/MacOS/Claude",
        "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
        "./Codex Computer Use.app/Contents/MacOS/helper",
        "./claude tools.app/Contents/MacOS/helper",
    ];
    for has_arguments in [false, true] {
        let records: Vec<AgentProcessRecord> = executables
            .iter()
            .enumerate()
            .map(|(idx, path)| {
                make_record(
                    &[path, "Inspect README.md"],
                    (idx + 1) as i32,
                    has_arguments,
                )
            })
            .collect();

        assert_eq!(
            AgentPSOutputParser::agent_processes(&records)
                .iter()
                .map(|r| r.pid)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
    }
}

// upstream: AgentSessionExecutableTests.swift:67 preserved app server arguments may refer to an application bundle
#[test]
fn preserved_app_server_arguments_may_refer_to_an_application_bundle() {
    let record = make_record(
        &[
            "/usr/local/bin/codex",
            "-c",
            "shell_environment_policy.set.REVIEW_PATH=\"/Applications/Example.app/Contents\"",
            "app-server",
        ],
        101,
        true,
    );

    assert!(AgentPSOutputParser::has_codex_app_server(
        std::slice::from_ref(&record)
    ));
    assert!(AgentPSOutputParser::agent_processes(std::slice::from_ref(&record)).is_empty());
}

// upstream: AgentSessionExecutableTests.swift:81 legacy Claude paths with spaces retain their existing exception
#[test]
fn legacy_claude_paths_with_spaces_retain_their_existing_exception() {
    let cases: [Option<Vec<String>>; 3] = [
        None,
        Some(Vec::new()),
        Some(vec![
            "/tmp/Example.app/Library/Application Support/Claude/claude-code/claude".to_owned(),
        ]),
    ];
    for arguments in cases {
        let record = AgentProcessRecord::new(
            101,
            1,
            None,
            "/tmp/Example.app/Library/Application Support/Claude/claude-code/claude",
        )
        .with_arguments(arguments);

        assert_eq!(
            AgentPSOutputParser::agent_processes(std::slice::from_ref(&record))
                .iter()
                .map(|r| r.pid)
                .collect::<Vec<_>>(),
            vec![101]
        );
        assert_eq!(
            AgentPSOutputParser::source(&record),
            AgentSessionSource::DesktopApp
        );
    }
}

// upstream: AgentSessionExecutableTests.swift:98 scanner keeps command-only CLI rows before a transcript exists
#[tokio::test]
async fn scanner_keeps_command_only_cli_rows_before_a_transcript_exists() {
    for provider in ["codex", "claude"] {
        let home = tempfile::tempdir().unwrap();
        let flag = if provider == "codex" { "exec" } else { "-p" };
        let command = format!("/usr/local/bin/{provider} {flag} Inspect README.md");
        let ps_line = format!("101 1 Mon Jul 6 09:03:00 2026 {command}");
        let scanner = LocalAgentSessionScanner::with_providers(
            move |_| ps_line.clone(),
            |_, _| HashMap::from([(101, "/synthetic/project".to_owned())]),
        );

        let env = HashMap::from([
            (
                "HOME".to_owned(),
                home.path().to_string_lossy().into_owned(),
            ),
            (
                "CODEX_HOME".to_owned(),
                home.path().join(".codex").to_string_lossy().into_owned(),
            ),
        ]);
        let sessions = scanner.scan_with_env(Timestamp::now(), &env, false).await;

        assert_eq!(sessions.len(), 1);
        assert!(!sessions.is_empty());
        let session = &sessions[0];
        assert_eq!(session.id, "pid:101");
        assert_eq!(session.pid, Some(101));
        assert_eq!(session.provider.as_str(), provider);
        assert_eq!(session.source, AgentSessionSource::Cli);
        assert_eq!(session.project_name.as_deref(), Some("project"));
    }
}

// ============================================================================
// 3. ClaudeSessionMappingTests.swift (4/4)
// ============================================================================

// upstream: ClaudeSessionMappingTests.swift:6 cwd escaping replaces every non alphanumeric ASCII byte
#[test]
fn cwd_escaping_replaces_every_non_alphanumeric_ascii_byte() {
    assert_eq!(
        ClaudeSessionProjectMapper::escaped_cwd("/Users/test/My Project_v2"),
        "-Users-test-My-Project-v2"
    );
}

// upstream: ClaudeSessionMappingTests.swift:11 newest transcript is selected from mapped project directory
#[test]
fn newest_transcript_is_selected_from_mapped_project_directory() {
    let home = tempfile::tempdir().expect("tempdir");
    let cwd = "/Users/test/Projects/alpha";
    let project_directory = home
        .path()
        .join(".claude/projects")
        .join(ClaudeSessionProjectMapper::escaped_cwd(cwd));
    fs::create_dir_all(&project_directory).expect("create project dir");
    let older = project_directory.join("older.jsonl");
    let newer = project_directory.join("newer.jsonl");
    fs::write(&older, "fixture\n").expect("write older");
    fs::write(&newer, "fixture\n").expect("write newer");
    set_modified(&older, Timestamp::from_second(100).unwrap());
    set_modified(&newer, Timestamp::from_second(200).unwrap());

    let match_transcript = ClaudeSessionProjectMapper::newest_transcript(cwd, home.path());
    assert!(match_transcript.is_some());
    let match_transcript = match_transcript.expect("newest transcript");
    assert_eq!(
        match_transcript.url.file_name().and_then(|s| s.to_str()),
        Some("newer.jsonl")
    );
    assert_eq!(
        match_transcript.modified_at,
        Timestamp::from_second(200).unwrap()
    );

    let bounded = ClaudeSessionProjectMapper::transcripts(
        cwd,
        home.path(),
        Some(1),
        Timestamp::from_second(150).unwrap(),
    );
    assert_eq!(
        bounded
            .iter()
            .filter_map(|t| t.url.file_name().and_then(|s| s.to_str()))
            .collect::<Vec<_>>(),
        vec!["newer.jsonl"]
    );
    assert_eq!(
        bounded.first().map(|t| t.modified_at),
        Some(Timestamp::from_second(150).unwrap())
    );
}

// upstream: ClaudeSessionMappingTests.swift:45 directory metadata scan bounds entry count depth and time
#[test]
fn directory_metadata_scan_bounds_entry_count_depth_and_time() {
    let root = tempfile::tempdir().expect("tempdir");
    for name in ["one.jsonl", "two.jsonl", "three.jsonl"] {
        fs::write(root.path().join(name), "fixture\n").expect("write file");
    }
    let nested = root.path().join("nested");
    fs::create_dir_all(&nested).expect("create nested");
    fs::write(nested.join("nested.jsonl"), "fixture\n").expect("write nested");

    let mut bounded = DirectoryMetadataScanBudget::new(2, 1, 60.0, Timestamp::now());
    let files = bounded.files(root.path());
    assert!(files.len() <= 2);
    assert!(!files.iter().any(|p| p.parent() == Some(nested.as_path())));

    let mut expired = DirectoryMetadataScanBudget::new(100, 2, 0.0, Timestamp::now());
    assert!(expired.files(root.path()).is_empty());
}

// upstream: ClaudeSessionMappingTests.swift:67 future modification dates use one path free clamp anchor
#[test]
fn future_modification_dates_use_one_path_free_clamp_anchor() {
    let url = Path::new("/tmp/future-session.jsonl");
    let first_now = Timestamp::from_second(100).unwrap();
    let clamp = FutureModificationDateClamp::new(Some(first_now));
    let future = Timestamp::from_second(3700).unwrap();

    assert_eq!(clamp.clamp(url, future, first_now), first_now);
    assert_eq!(
        clamp.clamp(url, future, Timestamp::from_second(130).unwrap()),
        first_now
    );
    assert_eq!(
        clamp.clamp(
            url,
            Timestamp::from_second(3701).unwrap(),
            Timestamp::from_second(130).unwrap(),
        ),
        first_now
    );
}

// ============================================================================
// 4. DirectoryMetadataScanBudgetTests.swift (5/5)
// ============================================================================

fn synthetic_urls() -> Vec<PathBuf> {
    ["first.jsonl", "skip.txt", "last.jsonl"]
        .into_iter()
        .map(|name| PathBuf::from(format!("/synthetic-sessions/{name}")))
        .collect()
}

// upstream: DirectoryMetadataScanBudgetTests.swift:10 expired enumeration budget does not start file enrichment
#[test]
fn expired_enumeration_budget_does_not_start_file_enrichment() {
    let urls = synthetic_urls();
    let budget = DirectoryMetadataScanBudget::new(512, 1, 1.0, Timestamp::MIN);
    let mut enriched = Vec::new();
    let results = budget.compact_map_while_time_remains(&urls, |url| {
        enriched.push(url.clone());
        Some(url.clone())
    });

    assert!(enriched.is_empty());
    assert!(results.is_empty());
}

// upstream: DirectoryMetadataScanBudgetTests.swift:27 enrichment stops between files when the shared deadline expires
#[test]
fn enrichment_stops_between_files_when_the_shared_deadline_expires() {
    let urls = synthetic_urls();
    let started_at = Timestamp::from_second(100).unwrap();
    let budget = DirectoryMetadataScanBudget::new(512, 1, 1.0, started_at);
    let now = std::cell::Cell::new(started_at);
    let mut enriched = Vec::new();
    let results = budget.compact_map_while_time_remains_with_clock(
        &urls,
        || now.get(),
        |url| {
            enriched.push(url.clone());
            now.set(Timestamp::from_second(102).unwrap());
            Some(url.clone())
        },
    );

    assert_eq!(enriched, vec![urls[0].clone()]);
    assert_eq!(results, vec![urls[0].clone()]);
}

// upstream: DirectoryMetadataScanBudgetTests.swift:47 live budget preserves enrichment filtering and input order
#[test]
fn live_budget_preserves_enrichment_filtering_and_input_order() {
    let urls = synthetic_urls();
    let budget = DirectoryMetadataScanBudget::new(512, 1, 1.0, Timestamp::MAX);
    let results = budget.compact_map_while_time_remains(&urls, |url| {
        (url.extension().and_then(|e| e.to_str()) == Some("jsonl"))
            .then(|| url.file_name().unwrap().to_str().unwrap().to_owned())
    });

    assert_eq!(
        results,
        vec!["first.jsonl".to_owned(), "last.jsonl".to_owned()]
    );
}

// upstream: DirectoryMetadataScanBudgetTests.swift:61 adaptive entry retention honors the injected deadline
#[test]
fn adaptive_entry_retention_honors_the_injected_deadline() {
    for elapsed in [0.149_f64, 0.15_f64, 0.151_f64] {
        let root = tempfile::tempdir().expect("tempdir");
        fs::write(root.path().join("entry.jsonl"), "fixture").expect("write entry");
        let started_at = Timestamp::from_second(100).unwrap();
        let mut budget = DirectoryMetadataScanBudget::new(
            1,
            1,
            SessionScanConfig::default().adaptive_directory_scan_budget,
            started_at,
        );
        let mut clock_reads = 0usize;

        let files = budget.files_with_clock(root.path(), || {
            clock_reads += 1;
            let delta_ms = if clock_reads < 3 {
                0
            } else {
                (elapsed * 1000.0).round() as i64
            };
            Timestamp::from_millisecond(started_at.as_millisecond() + delta_ms).unwrap()
        });

        assert_eq!(clock_reads, 3);
        assert_eq!(files.len(), usize::from(elapsed < 0.15));
    }
}

// upstream: DirectoryMetadataScanBudgetTests.swift:85 desktop root discovery honors the shared deadline
#[test]
fn desktop_root_discovery_honors_the_shared_deadline() {
    for expired in [false, true] {
        let root = tempfile::tempdir().expect("tempdir");
        let projects = root
            .path()
            .join("Library/Application Support/Claude/claude-code-sessions/.claude/projects");
        fs::create_dir_all(&projects).expect("create projects");
        let mut budget = DirectoryMetadataScanBudget::new(
            512,
            1,
            1.0,
            if expired {
                Timestamp::MIN
            } else {
                Timestamp::MAX
            },
        );

        let roots =
            ClaudeDesktopProjectsLocator::roots_with_budget(root.path(), &mut budget, false);

        let expected: Vec<PathBuf> = if expired { Vec::new() } else { vec![projects] };
        assert_eq!(roots, expected);
    }
}

// ============================================================================
// 5. CodexSessionRolloutTests.swift (19/19)
// ============================================================================

const CHATGPT_EXECUTABLES: [&str; 2] = [
    "/Applications/ChatGPT.app/Contents/Resources/codex",
    "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
];

// upstream: CodexSessionRolloutTests.swift:17 first rollout line maps to file only agent session
#[test]
fn first_rollout_line_maps_to_file_only_agent_session() {
    let url = fixture_path("agent-session-rollout.jsonl");
    let metadata = CodexRolloutFirstLineParser::read(&url).expect("read rollout metadata");
    let now = Timestamp::from_second(10_000).unwrap();
    let modified_at = Timestamp::from_second(10_000 - 60).unwrap();
    let session = CodexRolloutFirstLineParser::make_session(
        &metadata,
        &url,
        modified_at,
        None,
        None,
        "local-mac",
        &SessionScanConfig::default(),
        now,
    )
    .expect("make session");

    assert_eq!(session.id, "019f-session-fixture");
    assert_eq!(session.cwd.as_deref(), Some("/Users/test/Projects/alpha"));
    assert_eq!(session.project_name.as_deref(), Some("alpha"));
    assert_eq!(session.source, AgentSessionSource::Cli);
    assert_eq!(session.state, AgentSessionState::Active);
    assert_eq!(session.pid, None);
    assert!(url.exists());
    assert!(!metadata.session_id.is_empty());
}

// upstream: CodexSessionRolloutTests.swift:38 file only rollout outside window is excluded while live process remains
#[test]
fn file_only_rollout_outside_window_is_excluded_while_live_process_remains() {
    let url = fixture_path("agent-session-rollout.jsonl");
    let metadata = CodexRolloutFirstLineParser::read(&url).expect("read rollout metadata");
    let now = Timestamp::from_second(10_000).unwrap();
    let modified_at = Timestamp::from_second(10_000 - 1801).unwrap();
    let cfg = SessionScanConfig::default();

    assert!(
        CodexRolloutFirstLineParser::make_session(
            &metadata,
            &url,
            modified_at,
            None,
            None,
            "local-mac",
            &cfg,
            now,
        )
        .is_none()
    );
    assert_eq!(
        CodexRolloutFirstLineParser::make_session(
            &metadata,
            &url,
            modified_at,
            Some(42),
            None,
            "local-mac",
            &cfg,
            now,
        )
        .map(|s| s.state),
        Some(AgentSessionState::Idle)
    );
    assert!(!metadata.session_id.is_empty());
}

// upstream: CodexSessionRolloutTests.swift:60 app server presence classifies unknown file only rollout as desktop
#[test]
fn app_server_presence_classifies_unknown_file_only_rollout_as_desktop() {
    assert_eq!(
        AgentSessionCorrelation::file_only_codex_source(AgentSessionSource::Unknown, true),
        AgentSessionSource::DesktopApp
    );
    assert_eq!(
        AgentSessionCorrelation::file_only_codex_source(AgentSessionSource::Unknown, false),
        AgentSessionSource::Unknown
    );
}

// upstream: CodexSessionRolloutTests.swift:70 codex cwd matching rejects missing paths
#[test]
fn codex_cwd_matching_rejects_missing_paths() {
    assert!(AgentSessionCorrelation::codex_working_directories_match(
        Some("/repo/alpha"),
        Some("/repo/./alpha")
    ));
    assert!(!AgentSessionCorrelation::codex_working_directories_match(
        None, None
    ));
    assert!(!AgentSessionCorrelation::codex_working_directories_match(
        Some("/repo/alpha"),
        None
    ));
}

struct AdaptiveChatGPTFixture {
    _root: tempfile::TempDir,
    rollout: PathBuf,
    scanner: LocalAgentSessionScanner,
    environment: HashMap<String, String>,
}

type CustomTrustValidator = Arc<dyn Fn(&AgentProcessRecord) -> bool + Send + Sync>;

fn make_adaptive_chatgpt_fixture(
    now: Timestamp,
    rollout_age_secs: i64,
    app_server_executable: &str,
    app_server_is_trusted: bool,
    custom_validator: Option<CustomTrustValidator>,
) -> AdaptiveChatGPTFixture {
    let root = tempfile::tempdir().expect("tempdir");
    let codex_home = root.path().join("codex-home");
    let session_dir = codex_home.join("sessions").join(format_day_dir(now));
    fs::create_dir_all(&session_dir).expect("create session dir");

    let source = fixture_path("agent-session-rollout.jsonl");
    let rollout = session_dir.join("rollout-chatgpt-existing.jsonl");
    fs::copy(&source, &rollout).expect("copy rollout fixture");
    set_modified(
        &rollout,
        Timestamp::from_second(now.as_second() - rollout_age_secs).unwrap(),
    );

    let executable = app_server_executable.replace("<home>", &root.path().to_string_lossy());
    let ps_line = format!(
        "4234 1 Mon Jul 6 09:03:00 2026 {executable} -c features.code_mode_host=true app-server --analytics-default-enabled"
    );
    let validator: CustomTrustValidator =
        custom_validator.unwrap_or_else(|| Arc::new(move |_| app_server_is_trusted));
    let scanner =
        LocalAgentSessionScanner::with_providers(move |_| ps_line.clone(), |_, _| HashMap::new())
            .with_app_server_trust_validator(move |r| validator(r))
            .with_directory_scan_started_at(|| Timestamp::MAX);

    let environment = HashMap::from([
        (
            "CODEX_HOME".to_owned(),
            codex_home.to_string_lossy().into_owned(),
        ),
        (
            "HOME".to_owned(),
            root.path().to_string_lossy().into_owned(),
        ),
        (
            "PATH".to_owned(),
            "/usr/bin:/bin:/usr/sbin:/sbin".to_owned(),
        ),
    ]);
    AdaptiveChatGPTFixture {
        _root: root,
        rollout,
        scanner,
        environment,
    }
}

// upstream: CodexSessionRolloutTests.swift:77 trusted chatgpt app server projects recent codex rollout activity without an agent process
#[tokio::test]
async fn trusted_chatgpt_app_server_projects_recent_codex_rollout_activity_without_an_agent_process()
 {
    for executable in CHATGPT_EXECUTABLES {
        let now = Timestamp::now();
        let fixture = make_adaptive_chatgpt_fixture(now, 30, executable, true, None);
        let sessions = fixture
            .scanner
            .scan_with_env(now, &fixture.environment, false)
            .await;
        assert!(!sessions.is_empty());
        let session = sessions.first().expect("first session");

        assert_eq!(sessions.len(), 1);
        assert_eq!(session.provider, AgentSessionProvider::Codex);
        assert_eq!(session.source, AgentSessionSource::Cli);
        assert_eq!(session.state, AgentSessionState::Active);
        assert_eq!(session.pid, None);
        assert!(session.last_activity_at.is_some());
        let last_activity = session.last_activity_at.expect("last_activity_at");
        let expected = Timestamp::from_second(now.as_second() - 30).unwrap();
        assert!((last_activity.as_second() - expected.as_second()).abs() <= 1);
    }
}

// upstream: CodexSessionRolloutTests.swift:99 idle chatgpt app server with a stale rollout does not produce coding activity
#[tokio::test]
async fn idle_chatgpt_app_server_with_a_stale_rollout_does_not_produce_coding_activity() {
    for executable in CHATGPT_EXECUTABLES {
        let now = Timestamp::now();
        let fixture = make_adaptive_chatgpt_fixture(now, 31 * 60, executable, true, None);
        let sessions = fixture
            .scanner
            .scan_with_env(now, &fixture.environment, false)
            .await;

        assert!(sessions.is_empty());
    }
}

// upstream: CodexSessionRolloutTests.swift:118 continuing an existing chatgpt codex rollout advances the adaptive activity signal
#[tokio::test]
async fn continuing_an_existing_chatgpt_codex_rollout_advances_the_adaptive_activity_signal() {
    for executable in CHATGPT_EXECUTABLES {
        let now = Timestamp::now();
        let fixture = make_adaptive_chatgpt_fixture(now, 30, executable, true, None);
        let first_sessions = fixture
            .scanner
            .scan_with_env(now, &fixture.environment, false)
            .await;
        assert!(
            first_sessions
                .first()
                .and_then(|s| s.last_activity_at)
                .is_some()
        );
        let first_activity = first_sessions
            .first()
            .and_then(|s| s.last_activity_at)
            .expect("first activity");
        let next_activity = Timestamp::from_second(now.as_second() + 20).unwrap();
        set_modified(&fixture.rollout, next_activity);

        let continued_sessions = fixture
            .scanner
            .scan_with_env(
                Timestamp::from_second(next_activity.as_second() + 1).unwrap(),
                &fixture.environment,
                false,
            )
            .await;
        assert!(
            continued_sessions
                .first()
                .and_then(|s| s.last_activity_at)
                .is_some()
        );
        let continued_activity = continued_sessions
            .first()
            .and_then(|s| s.last_activity_at)
            .expect("continued activity");

        assert_eq!(
            continued_sessions.first().map(|s| &s.id),
            first_sessions.first().map(|s| &s.id)
        );
        assert!(continued_activity > first_activity);
        assert!((continued_activity.as_second() - next_activity.as_second()).abs() <= 1);
    }
}

// upstream: CodexSessionRolloutTests.swift:147 untrusted chatgpt app server cannot authorize adaptive rollout inspection
#[tokio::test]
async fn untrusted_chatgpt_app_server_cannot_authorize_adaptive_rollout_inspection() {
    for executable in CHATGPT_EXECUTABLES {
        let now = Timestamp::now();
        let fixture = make_adaptive_chatgpt_fixture(now, 30, executable, false, None);
        let sessions = fixture
            .scanner
            .scan_with_env(now, &fixture.environment, false)
            .await;

        assert!(sessions.is_empty());
    }
}

// upstream: CodexSessionRolloutTests.swift:162 unrecognized app server path cannot authorize adaptive rollout inspection
#[tokio::test]
async fn unrecognized_app_server_path_cannot_authorize_adaptive_rollout_inspection() {
    for executable in [
        "codex",
        "/tmp/codex",
        "/tmp/ChatGPT.app/Contents/Resources/codex",
        "<home>/Applications/ChatGPT.app/Contents/Resources/codex",
        "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex-fake",
        "/Applications/ChatGPT.app/Contents/Resources/../Resources/codex",
    ] {
        let now = Timestamp::now();
        let fixture = make_adaptive_chatgpt_fixture(now, 30, executable, true, None);
        let sessions = fixture
            .scanner
            .scan_with_env(now, &fixture.environment, false)
            .await;

        assert!(sessions.is_empty());
    }
}

// upstream: CodexSessionRolloutTests.swift:186 local scanner parses only its newest configured rollout candidates
#[tokio::test]
async fn local_scanner_parses_only_its_newest_configured_rollout_candidates() {
    let temp_root = tempfile::tempdir().expect("tempdir");
    let now = Timestamp::from_second(Timestamp::now().as_second()).unwrap();
    let codex_home = temp_root.path().join("codex-home");
    let session_dir = codex_home.join("sessions").join(format_day_dir(now));
    fs::create_dir_all(&session_dir).expect("create session dir");

    let fixture = fixture_string("agent-session-rollout.jsonl");
    for (idx, age_secs) in [30_i64, 20_i64, -3600_i64].into_iter().enumerate() {
        let id = format!("bounded-rollout-{idx}");
        let url = session_dir.join(format!("rollout-bounded-{idx}.jsonl"));
        fs::write(&url, fixture.replace("019f-session-fixture", &id)).expect("write rollout");
        set_modified(
            &url,
            Timestamp::from_second(now.as_second() - age_secs).unwrap(),
        );
    }

    let scanner = LocalAgentSessionScanner::with_config(SessionScanConfig {
        file_only_window: 60.0 * 60.0,
        max_process_count: 0,
        max_codex_rollout_count: 2,
        max_claude_transcript_count_per_project: 0,
        ..SessionScanConfig::default()
    });
    let env = HashMap::from([
        (
            "CODEX_HOME".to_owned(),
            codex_home.to_string_lossy().into_owned(),
        ),
        (
            "HOME".to_owned(),
            temp_root.path().to_string_lossy().into_owned(),
        ),
        (
            "PATH".to_owned(),
            "/usr/bin:/bin:/usr/sbin:/sbin".to_owned(),
        ),
    ]);
    let sessions = scanner.scan_with_env(now, &env, true).await;

    let ids: HashSet<String> = sessions.iter().map(|s| s.id.clone()).collect();
    let expected: HashSet<String> = [
        "bounded-rollout-1".to_owned(),
        "bounded-rollout-2".to_owned(),
    ]
    .into_iter()
    .collect();
    assert_eq!(ids, expected);
    assert_eq!(
        sessions
            .iter()
            .find(|s| s.id == "bounded-rollout-2")
            .and_then(|s| s.last_activity_at),
        Some(now)
    );

    let rescanned = scanner
        .scan_with_env(
            Timestamp::from_second(now.as_second() + 30).unwrap(),
            &env,
            true,
        )
        .await;
    assert_eq!(
        rescanned
            .iter()
            .find(|s| s.id == "bounded-rollout-2")
            .and_then(|s| s.last_activity_at),
        Some(now)
    );
}

// upstream: CodexSessionRolloutTests.swift:240 subagent and guardian rollout metadata produce descriptive names
#[test]
fn subagent_and_guardian_rollout_metadata_produce_descriptive_names() {
    let subagent_line = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"subagent\",\"cwd\":\"/repo\",\"originator\":\"codex_vscode\",\"source\":{\"subagent\":{\"thread_spawn\":{\"agent_path\":\"/root/neon_patch_review2\"}}}}}";
    let guardian_line = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"guardian\",\"cwd\":\"/repo\",\"originator\":\"codex_vscode\",\"source\":{\"subagent\":{\"other\":\"guardian\"}}}}";

    let subagent = CodexRolloutFirstLineParser::parse(subagent_line);
    let guardian = CodexRolloutFirstLineParser::parse(guardian_line);
    assert!(subagent.is_some());
    assert!(guardian.is_some());
    let subagent = subagent.expect("parse subagent");
    let guardian = guardian.expect("parse guardian");

    assert_eq!(
        subagent.agent_path.as_deref(),
        Some("/root/neon_patch_review2")
    );
    assert_eq!(
        subagent.descriptive_name(None).as_deref(),
        Some("Neon patch review 2")
    );
    assert!(guardian.is_guardian);
    assert_eq!(
        guardian.descriptive_name(None).as_deref(),
        Some("Approval review")
    );
}

// upstream: CodexSessionRolloutTests.swift:259 chat names stay primary for subagent and guardian sessions
#[test]
fn chat_names_stay_primary_for_subagent_and_guardian_sessions() {
    let subagent_line = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"subagent\",\"cwd\":\"/repo\",\"originator\":\"codex_vscode\",\"source\":{\"subagent\":{\"thread_spawn\":{\"agent_path\":\"/root/neon_patch_review2\"}}}}}";
    let guardian_line = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"guardian\",\"cwd\":\"/repo\",\"originator\":\"codex_vscode\",\"source\":{\"subagent\":{\"other\":\"guardian\"}}}}";

    let subagent = CodexRolloutFirstLineParser::parse(subagent_line);
    let guardian = CodexRolloutFirstLineParser::parse(guardian_line);
    assert!(subagent.is_some());
    assert!(guardian.is_some());
    let subagent = subagent.expect("parse subagent");
    let guardian = guardian.expect("parse guardian");

    assert_eq!(
        subagent
            .descriptive_name(Some(&CodexThreadMetadata::new(
                Some("Website refresh".to_owned()),
                None
            )))
            .as_deref(),
        Some("Website refresh · Neon patch review 2")
    );
    assert_eq!(
        guardian
            .descriptive_name(Some(&CodexThreadMetadata::new(
                Some("Dependency audit".to_owned()),
                None
            )))
            .as_deref(),
        Some("Dependency audit · Approval review")
    );
}

// upstream: CodexSessionRolloutTests.swift:279 chat and task names stay menu sized
#[test]
fn chat_and_task_names_stay_menu_sized() {
    let line = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"subagent\",\"cwd\":\"/repo\",\"originator\":\"codex_vscode\",\"source\":{\"subagent\":{\"thread_spawn\":{\"agent_path\":\"/root/review_the_complete_rollout_and_report_every_regression\"}}}}}";
    let metadata = CodexRolloutFirstLineParser::parse(line);
    assert!(metadata.is_some());
    let metadata = metadata.expect("parse metadata");

    let name = metadata
        .descriptive_name(Some(&CodexThreadMetadata::new(
            Some(
                "Continue work on the Concrete Authority website and compare every source"
                    .to_owned(),
            ),
            None,
        )))
        .expect("descriptive name");

    assert_eq!(
        name,
        "Continue work on the Concrete Authority… · Review the complete…"
    );
    assert!(name.chars().count() <= 64);
}

// upstream: CodexSessionRolloutTests.swift:295 current rollout agent path produces a descriptive subagent name without sqlite
#[test]
fn current_rollout_agent_path_produces_a_descriptive_subagent_name_without_sqlite() {
    let line = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"subagent\",\"cwd\":\"/repo\",\"originator\":\"codex_vscode\",\"source\":\"subagent\",\"agent_path\":\"/root/config_audit3\"}}";
    let metadata = CodexRolloutFirstLineParser::parse(line);
    assert!(metadata.is_some());
    let metadata = metadata.expect("parse metadata");

    assert_eq!(metadata.agent_path.as_deref(), Some("/root/config_audit3"));
    assert_eq!(
        metadata.descriptive_name(None).as_deref(),
        Some("Config audit 3")
    );
}

// upstream: CodexSessionRolloutTests.swift:308 thread titles skip command preambles and stay menu sized
#[test]
fn thread_titles_skip_command_preambles_and_stay_menu_sized() {
    let metadata = CodexRolloutMetadata::new(
        "main",
        Some("/repo".to_owned()),
        Some("codex_vscode".to_owned()),
        Some("vscode".to_owned()),
        None,
        false,
    );
    let title = "/brain-orient\n\nContinue work on the Concrete Authority website and compare every current source before changing anything.";

    let name = metadata
        .descriptive_name(Some(&CodexThreadMetadata::new(
            Some(title.to_owned()),
            None,
        )))
        .expect("descriptive name");
    assert_eq!(
        name,
        "Continue work on the Concrete Authority website and compare eve…"
    );
    assert_eq!(name.chars().count(), 64);
}

// upstream: CodexSessionRolloutTests.swift:328 live scanner suppresses descriptive names for ambiguous same project processes
#[tokio::test]
async fn live_scanner_suppresses_descriptive_names_for_ambiguous_same_project_processes() {
    let root = tempfile::tempdir().expect("tempdir");
    let now = Timestamp::from_second(Timestamp::now().as_second()).unwrap();
    let codex_home = root.path().join("codex-home");
    let session_dir = codex_home.join("sessions").join(format_day_dir(now));
    fs::create_dir_all(&session_dir).expect("create session dir");

    for (idx, name) in ["recent_activity", "older_activity"]
        .into_iter()
        .enumerate()
    {
        let line = format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"session-{idx}\",\"cwd\":\"/repo\",\"originator\":\"codex_cli\",\"source\":{{\"subagent\":{{\"thread_spawn\":{{\"agent_path\":\"/root/{name}\"}}}}}}}}}}"
        );
        let url = session_dir.join(format!("rollout-ambiguous-{idx}.jsonl"));
        fs::write(&url, line).expect("write rollout");
        set_modified(
            &url,
            Timestamp::from_second(now.as_second() - (idx as i64 * 30)).unwrap(),
        );
    }

    let scanner = LocalAgentSessionScanner::with_providers(
        |_| {
            "201 1 Mon Jul 6 09:03:00 2026 /usr/local/bin/codex exec\n\
             202 1 Tue Jul 7 09:03:00 2026 /usr/local/bin/codex exec"
                .to_owned()
        },
        |_, _| HashMap::from([(201, "/repo".to_owned()), (202, "/repo".to_owned())]),
    );
    let env = HashMap::from([
        (
            "CODEX_HOME".to_owned(),
            codex_home.to_string_lossy().into_owned(),
        ),
        (
            "HOME".to_owned(),
            root.path().to_string_lossy().into_owned(),
        ),
        (
            "PATH".to_owned(),
            "/usr/bin:/bin:/usr/sbin:/sbin".to_owned(),
        ),
    ]);
    let sessions = scanner.scan_with_env(now, &env, false).await;

    assert_eq!(sessions.len(), 2);
    assert!(
        sessions
            .iter()
            .all(|s| s.project_name.as_deref() == Some("repo"))
    );
    assert!(sessions.iter().all(|s| s.session_name.is_none()));
}

// upstream: CodexSessionRolloutTests.swift:379 chatgpt rollout freshness controls five versus thirty minute cadence
#[tokio::test]
async fn chatgpt_rollout_freshness_controls_five_versus_thirty_minute_cadence() {
    for executable in CHATGPT_EXECUTABLES {
        for age_secs in [30_i64, 6 * 60_i64] {
            let now = Timestamp::from_second(Timestamp::now().as_second()).unwrap();
            let fixture = make_adaptive_chatgpt_fixture(now, age_secs, executable, true, None);
            let sessions = fixture
                .scanner
                .scan_with_env(now, &fixture.environment, false)
                .await;
            let latest_activity = sessions
                .iter()
                .filter(|s| s.state == AgentSessionState::Active)
                .filter_map(|s| s.last_activity_at.or(s.started_at))
                .max();
            let decision = AdaptiveRefreshPolicyCore.next_delay(&AdaptiveRefreshInput::new(
                now,
                None,
                latest_activity,
                false,
                ThermalPressure::Nominal,
            ));

            assert_eq!(
                decision.reason,
                if age_secs < 300 {
                    AdaptiveRefreshReason::CodingActivity
                } else {
                    AdaptiveRefreshReason::LongIdle
                }
            );
            assert_eq!(
                decision.delay,
                Duration::from_secs(if age_secs < 300 { 300 } else { 1800 })
            );
        }
    }
}

// upstream: CodexSessionRolloutTests.swift:399 app server trust is revalidated after a successful scan
#[tokio::test]
async fn app_server_trust_is_revalidated_after_a_successful_scan() {
    for executable in CHATGPT_EXECUTABLES {
        let marker_dir = tempfile::tempdir().expect("tempdir");
        let marker = marker_dir.path().join("trusted-marker");
        fs::write(&marker, b"").expect("write marker");
        let marker_check = marker.clone();
        let now = Timestamp::from_second(Timestamp::now().as_second()).unwrap();
        let fixture = make_adaptive_chatgpt_fixture(
            now,
            30,
            executable,
            true,
            Some(Arc::new(move |_| marker_check.exists())),
        );

        let trusted = fixture
            .scanner
            .scan_with_env(now, &fixture.environment, false)
            .await;
        assert_eq!(trusted.len(), 1);
        fs::remove_file(&marker).expect("remove marker");
        let untrusted = fixture
            .scanner
            .scan_with_env(now, &fixture.environment, false)
            .await;
        assert!(untrusted.is_empty());
    }
}

fn create_thread_database(path: &Path, session_id: &str, title: &str) {
    let conn = Connection::open(path).expect("open sqlite db");
    conn.execute(
        "CREATE TABLE threads (id TEXT PRIMARY KEY, title TEXT, agent_path TEXT);",
        [],
    )
    .expect("create table");
    conn.execute(
        "INSERT INTO threads (id, title, agent_path) VALUES (?1, ?2, NULL);",
        [session_id, title],
    )
    .expect("insert thread");
}

// upstream: CodexSessionRolloutTests.swift:476 scanner resolves relative sqlite homes for multiple session projects
#[tokio::test]
async fn scanner_resolves_relative_sqlite_homes_for_multiple_session_projects() {
    let root = tempfile::tempdir().expect("tempdir");
    let now = Timestamp::from_second(Timestamp::now().as_second()).unwrap();
    let codex_home = root.path().join("codex-home");
    let session_dir = codex_home.join("sessions").join(format_day_dir(now));
    fs::create_dir_all(&session_dir).expect("create session dir");

    for index in 0..2 {
        let project = root.path().join(format!("project-{index}"));
        let sqlite_home = project.join("relative-state");
        fs::create_dir_all(&sqlite_home).expect("create sqlite home");
        let session_id = format!("relative-session-{index}");
        let line = format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"cwd\":\"{}\",\"originator\":\"codex_cli\",\"source\":\"cli\"}}}}",
            project.display()
        );
        fs::write(
            session_dir.join(format!("rollout-relative-{index}.jsonl")),
            line,
        )
        .expect("write rollout");
        create_thread_database(
            &sqlite_home.join("state_5.sqlite"),
            &session_id,
            &format!("Project {index} title"),
        );
    }

    let scanner = LocalAgentSessionScanner::with_config(SessionScanConfig {
        max_process_count: 0,
        max_claude_transcript_count_per_project: 0,
        ..SessionScanConfig::default()
    });
    let env = HashMap::from([
        (
            "CODEX_HOME".to_owned(),
            codex_home.to_string_lossy().into_owned(),
        ),
        ("CODEX_SQLITE_HOME".to_owned(), "relative-state".to_owned()),
        (
            "HOME".to_owned(),
            root.path().to_string_lossy().into_owned(),
        ),
        (
            "PATH".to_owned(),
            "/usr/bin:/bin:/usr/sbin:/sbin".to_owned(),
        ),
    ]);
    let sessions = scanner.scan_with_env(now, &env, true).await;

    let actual: HashMap<String, Option<String>> = sessions
        .into_iter()
        .map(|s| (s.id, s.session_name))
        .collect();
    let expected: HashMap<String, Option<String>> = HashMap::from([
        (
            "relative-session-0".to_owned(),
            Some("Project 0 title".to_owned()),
        ),
        (
            "relative-session-1".to_owned(),
            Some("Project 1 title".to_owned()),
        ),
    ]);
    assert_eq!(actual, expected);
}

// ============================================================================
// 6. AdaptiveRolloutActivityTests.swift (8/10 scanner tests)
// ============================================================================

const ADAPTIVE_NOW_SECS: i64 = 1_790_769_600;
const ADAPTIVE_DAEMON_PS: &str = "4234 1 Mon Sep 28 09:03:00 2026 /synthetic/codex-home/packages/app-server-daemon/releases/fixture/bin/codex app-server --listen unix:// --managed-daemon";

struct AdaptiveRolloutFixture {
    root: tempfile::TempDir,
    rollout: PathBuf,
}

impl AdaptiveRolloutFixture {
    fn new(age_secs: i64) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let now = Timestamp::from_second(ADAPTIVE_NOW_SECS).unwrap();
        let directory = root
            .path()
            .join("codex-home/sessions")
            .join(format_day_dir(now));
        fs::create_dir_all(&directory).expect("create session dir");
        let rollout = directory.join("rollout-fixture.jsonl");
        fs::copy(fixture_path("agent-session-rollout.jsonl"), &rollout).expect("copy fixture");
        let this = Self { root, rollout };
        this.set_age(age_secs);
        this
    }

    fn set_age(&self, age_secs: i64) {
        let ts = Timestamp::from_second(ADAPTIVE_NOW_SECS - age_secs).unwrap();
        set_modified(&self.rollout, ts);
    }

    fn environment(&self) -> HashMap<String, String> {
        HashMap::from([
            (
                "HOME".to_owned(),
                self.root.path().to_string_lossy().into_owned(),
            ),
            (
                "CODEX_HOME".to_owned(),
                self.root
                    .path()
                    .join("codex-home")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("PATH".to_owned(), String::new()),
        ])
    }

    async fn scan(
        &self,
        scanner: &LocalAgentSessionScanner,
        now: Timestamp,
    ) -> ariadusage_engine::sessions::LocalSessionScanResult {
        scanner
            .scan_with_activity_and_env(now, &self.environment(), false, true)
            .await
    }
}

fn default_adaptive_scanner(
    config: SessionScanConfig,
    started_at: Timestamp,
) -> LocalAgentSessionScanner {
    LocalAgentSessionScanner::with_config(config)
        .with_process_output_provider(|_| String::new())
        .with_cwd_provider(|_, _| HashMap::new())
        .with_directory_scan_started_at(move || started_at)
}

// upstream: AdaptiveRolloutActivityTests.swift:12 rollout freshness selects cadence without a recognized process
#[tokio::test]
async fn rollout_freshness_selects_cadence_without_a_recognized_process() {
    let now = Timestamp::from_second(ADAPTIVE_NOW_SECS).unwrap();
    for process in ["", ADAPTIVE_DAEMON_PS] {
        for age_secs in [30_i64, 300_i64, 6 * 60_i64, 31 * 60_i64] {
            let fixture = AdaptiveRolloutFixture::new(age_secs);
            let proc_str = process.to_owned();
            let scanner = LocalAgentSessionScanner::with_providers(
                move |_| proc_str.clone(),
                |_, _| HashMap::new(),
            )
            .with_app_server_trust_validator(|_| false)
            .with_directory_scan_started_at(|| Timestamp::MAX);
            let result = fixture.scan(&scanner, now).await;
            let activity = result.latest_rollout_activity_at;
            let decision = AdaptiveRefreshPolicyCore.next_delay(&AdaptiveRefreshInput::new(
                now,
                None,
                activity,
                false,
                ThermalPressure::Nominal,
            ));

            assert_eq!(
                decision.reason,
                if age_secs < 300 {
                    AdaptiveRefreshReason::CodingActivity
                } else {
                    AdaptiveRefreshReason::LongIdle
                }
            );
            assert_eq!(
                decision.delay,
                Duration::from_secs(if age_secs < 300 { 300 } else { 1800 })
            );
        }
    }
}

// upstream: AdaptiveRolloutActivityTests.swift:37 activity only never reads rollout contents or asserts a process identity
#[tokio::test]
async fn activity_only_never_reads_rollout_contents_or_asserts_a_process_identity() {
    let now = Timestamp::from_second(ADAPTIVE_NOW_SECS).unwrap();
    for process in [
        ADAPTIVE_DAEMON_PS,
        "4234 1 Mon Sep 28 09:03:00 2026 /usr/local/bin/codex exec",
        "4234 1 Mon Sep 28 09:03:00 2026 /Applications/ChatGPT.app/Contents/Resources/codex app-server",
    ] {
        let fixture = AdaptiveRolloutFixture::new(30);
        fs::write(&fixture.rollout, "not session metadata").expect("write invalid rollout");
        fixture.set_age(30);
        let proc_str = process.to_owned();
        let called_trust = Arc::new(AtomicUsize::new(0));
        let called_trust_clone = Arc::clone(&called_trust);
        let called_reader = Arc::new(AtomicUsize::new(0));
        let called_reader_clone = Arc::clone(&called_reader);
        let scanner = LocalAgentSessionScanner::with_providers(
            move |_| proc_str.clone(),
            |_, _| HashMap::new(),
        )
        .with_app_server_trust_validator(move |_| {
            called_trust_clone.fetch_add(1, Ordering::SeqCst);
            true
        })
        .with_rollout_metadata_reader(move |_| {
            called_reader_clone.fetch_add(1, Ordering::SeqCst);
            None
        })
        .with_directory_scan_started_at(|| Timestamp::MAX);
        let result = fixture.scan(&scanner, now).await;

        assert_eq!(called_trust.load(Ordering::SeqCst), 0);
        assert_eq!(called_reader.load(Ordering::SeqCst), 0);
        assert_eq!(
            result.latest_rollout_activity_at,
            Some(Timestamp::from_second(ADAPTIVE_NOW_SECS - 30).unwrap())
        );
        assert!(
            result
                .sessions
                .iter()
                .all(|s| s.last_activity_at.is_none() && s.transcript_path.is_none())
        );
    }
}

// upstream: AdaptiveRolloutActivityTests.swift:65 future rollouts keep the first clamp across scans and age out
#[tokio::test]
async fn future_rollouts_keep_the_first_clamp_across_scans_and_age_out() {
    let now = Timestamp::from_second(ADAPTIVE_NOW_SECS).unwrap();
    let fixture = AdaptiveRolloutFixture::new(-3600);
    let scanner = default_adaptive_scanner(SessionScanConfig::default(), Timestamp::MAX);
    let first = fixture.scan(&scanner, now).await;
    let later = Timestamp::from_second(ADAPTIVE_NOW_SECS + 360).unwrap();
    let repeated = fixture.scan(&scanner, later).await;

    assert_eq!(first.latest_rollout_activity_at, Some(now));
    assert_eq!(repeated.latest_rollout_activity_at, Some(now));
    let decision = AdaptiveRefreshPolicyCore.next_delay(&AdaptiveRefreshInput::new(
        later,
        None,
        repeated.latest_rollout_activity_at,
        false,
        ThermalPressure::Nominal,
    ));
    assert_eq!(decision.reason, AdaptiveRefreshReason::LongIdle);
    assert_eq!(decision.delay, Duration::from_secs(1800));
}

// upstream: AdaptiveRolloutActivityTests.swift:86 exhausted rollout budgets produce no activity
#[tokio::test]
async fn exhausted_rollout_budgets_produce_no_activity() {
    let now = Timestamp::from_second(ADAPTIVE_NOW_SECS).unwrap();
    for limit in ["entries", "depth", "candidates", "time"] {
        let fixture = AdaptiveRolloutFixture::new(30);
        let mut config = SessionScanConfig::default();
        match limit {
            "entries" => config.max_directory_entry_count = 0,
            "depth" => config.max_directory_depth = 0,
            "candidates" => config.max_codex_rollout_count = 0,
            _ => config.adaptive_directory_scan_budget = 0.0,
        }
        let started_at = if limit == "time" {
            Timestamp::MIN
        } else {
            Timestamp::MAX
        };
        let scanner = default_adaptive_scanner(config, started_at);
        let result = fixture.scan(&scanner, now).await;
        assert_eq!(result.latest_rollout_activity_at, None);
        assert!(result.sessions.is_empty());
    }
}

// upstream: AdaptiveRolloutActivityTests.swift:103 rollout activity respects the entry cap without a process
#[tokio::test]
async fn rollout_activity_respects_the_entry_cap_without_a_process() {
    let now = Timestamp::from_second(ADAPTIVE_NOW_SECS).unwrap();
    let fixture = AdaptiveRolloutFixture::new(30);
    let dir = fixture.rollout.parent().expect("parent");
    for idx in 0..12 {
        let url = dir.join(format!("rollout-{idx}.jsonl"));
        fs::copy(&fixture.rollout, &url).expect("copy rollout");
        set_modified(
            &url,
            Timestamp::from_second(ADAPTIVE_NOW_SECS - 30).unwrap(),
        );
    }
    let visits = Arc::new(AtomicUsize::new(0));
    let visits_clone = Arc::clone(&visits);
    let scanner = LocalAgentSessionScanner::with_config(SessionScanConfig {
        max_process_count: 0,
        max_directory_entry_count: 3,
        ..SessionScanConfig::default()
    })
    .with_process_output_provider(|_| String::new())
    .with_cwd_provider(|_, _| HashMap::new())
    .with_directory_scan_started_at(|| Timestamp::MAX)
    .with_did_visit_directory_entry(move || {
        visits_clone.fetch_add(1, Ordering::SeqCst);
    });
    let result = fixture.scan(&scanner, now).await;
    assert_eq!(
        result.latest_rollout_activity_at,
        Some(Timestamp::from_second(ADAPTIVE_NOW_SECS - 30).unwrap())
    );
    assert_eq!(visits.load(Ordering::SeqCst), 3);
}

// upstream: AdaptiveRolloutActivityTests.swift:125 yesterday rollouts count while other filenames and deeper directories do not
#[tokio::test]
async fn yesterday_rollouts_count_while_other_filenames_and_deeper_directories_do_not() {
    let now = Timestamp::from_second(ADAPTIVE_NOW_SECS).unwrap();
    let fixture = AdaptiveRolloutFixture::new(30);
    let today_dir = fixture.rollout.parent().expect("today dir").to_path_buf();
    let yesterday_ts = Timestamp::from_second(ADAPTIVE_NOW_SECS - 86_400).unwrap();
    let yesterday_dir = fixture
        .root
        .path()
        .join("codex-home/sessions")
        .join(format_day_dir(yesterday_ts));
    fs::create_dir_all(&yesterday_dir).expect("create yesterday dir");
    let moved_rollout = yesterday_dir.join("rollout-old.jsonl");
    fs::rename(&fixture.rollout, &moved_rollout).expect("move rollout");
    set_modified(
        &moved_rollout,
        Timestamp::from_second(ADAPTIVE_NOW_SECS - 30).unwrap(),
    );
    for name in [
        "other.jsonl",
        "rollout-ignore.txt",
        "nested/rollout-ignore.jsonl",
    ] {
        let url = today_dir.join(name);
        if let Some(parent) = url.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(&url, "unrelated").expect("write unrelated");
        set_modified(&url, now);
    }
    let scanner = default_adaptive_scanner(SessionScanConfig::default(), Timestamp::MAX);
    let result = fixture.scan(&scanner, now).await;
    assert!(moved_rollout.exists());
    assert_eq!(
        result.latest_rollout_activity_at,
        Some(Timestamp::from_second(ADAPTIVE_NOW_SECS - 30).unwrap())
    );
}

// upstream: AdaptiveRolloutActivityTests.swift:149 file only codex activity cannot starve a live claude transcript
#[tokio::test]
async fn file_only_codex_activity_cannot_starve_a_live_claude_transcript() {
    let now = Timestamp::from_second(ADAPTIVE_NOW_SECS).unwrap();
    for entry_limit in [1usize, 2usize] {
        let fixture = AdaptiveRolloutFixture::new(6 * 60);
        let cwd = "/synthetic/claude-project";
        let directory = fixture
            .root
            .path()
            .join(".claude/projects")
            .join(ClaudeSessionProjectMapper::escaped_cwd(cwd));
        fs::create_dir_all(&directory).expect("create claude dir");
        let transcript = directory.join("active.jsonl");
        fs::write(&transcript, "synthetic transcript").expect("write transcript");
        let activity = Timestamp::from_second(ADAPTIVE_NOW_SECS - 30).unwrap();
        set_modified(&transcript, activity);
        let visits = Arc::new(AtomicUsize::new(0));
        let visits_clone = Arc::clone(&visits);
        let scanner = LocalAgentSessionScanner::with_config(SessionScanConfig {
            max_directory_entry_count: entry_limit,
            ..SessionScanConfig::default()
        })
        .with_process_output_provider(|_| {
            "4234 1 Mon Sep 28 09:03:00 2026 /usr/local/bin/claude".to_owned()
        })
        .with_cwd_provider(move |_, _| HashMap::from([(4234, cwd.to_owned())]))
        .with_directory_scan_started_at(|| Timestamp::MAX)
        .with_did_visit_directory_entry(move || {
            visits_clone.fetch_add(1, Ordering::SeqCst);
        });
        let result = fixture.scan(&scanner, now).await;
        let latest_activity = result.latest_activity_at();
        let decision = AdaptiveRefreshPolicyCore.next_delay(&AdaptiveRefreshInput::new(
            now,
            None,
            latest_activity,
            false,
            ThermalPressure::Nominal,
        ));

        assert_eq!(
            result.sessions.first().and_then(|s| s.last_activity_at),
            Some(activity)
        );
        assert_eq!(decision.reason, AdaptiveRefreshReason::CodingActivity);
        assert_eq!(decision.delay, Duration::from_secs(300));
        assert_eq!(visits.load(Ordering::SeqCst), entry_limit);
    }
}

// upstream: AdaptiveRolloutActivityTests.swift:184 session enrichment and activity share one directory walk
#[tokio::test]
async fn session_enrichment_and_activity_share_one_directory_walk() {
    let now = Timestamp::from_second(ADAPTIVE_NOW_SECS).unwrap();
    let fixture = AdaptiveRolloutFixture::new(30);
    let visits = Arc::new(AtomicUsize::new(0));
    let visits_clone = Arc::clone(&visits);
    let reads = Arc::new(AtomicUsize::new(0));
    let reads_clone = Arc::clone(&reads);
    let scanner =
        LocalAgentSessionScanner::with_providers(|_| String::new(), |_, _| HashMap::new())
            .with_rollout_metadata_reader(move |url| {
                reads_clone.fetch_add(1, Ordering::SeqCst);
                CodexRolloutFirstLineParser::read(url)
            })
            .with_directory_scan_started_at(|| Timestamp::MAX)
            .with_did_visit_directory_entry(move || {
                visits_clone.fetch_add(1, Ordering::SeqCst);
            });
    let result = scanner
        .scan_with_activity_and_env(now, &fixture.environment(), true, true)
        .await;

    assert_eq!(visits.load(Ordering::SeqCst), 1);
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(result.sessions.len(), 1);
    assert_eq!(
        result.latest_rollout_activity_at,
        Some(Timestamp::from_second(ADAPTIVE_NOW_SECS - 30).unwrap())
    );
}

// ============================================================================
// 7. PiFamilySessionTests.swift (2 in-scope process/pid-only tests)
// ============================================================================

// upstream: PiFamilySessionTests.swift:74 process classification recognizes both pi dialects and excludes helpers
#[test]
fn process_classification_recognizes_both_pi_dialects_and_excludes_helpers() {
    let started = Some(Timestamp::from_second(1_899_999_900).unwrap());
    let records = vec![
        AgentProcessRecord::new(1, 1, started, "pi"),
        AgentProcessRecord::new(2, 1, started, "/usr/local/bin/pi --model test"),
        AgentProcessRecord::new(3, 1, started, "omp --profile work"),
        AgentProcessRecord::new(4, 1, started, "bun /tools/oh-my-pi/omp"),
        AgentProcessRecord::new(5, 1, started, "pi --help"),
        AgentProcessRecord::new(6, 1, started, "omp --version"),
        AgentProcessRecord::new(7, 1, started, "bun /tools/unrelated.js"),
    ];

    let agents = AgentPSOutputParser::agent_processes(&records);
    assert_eq!(
        agents.iter().map(|r| r.pid).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert_eq!(
        AgentPSOutputParser::pi_dialect(&records[0]),
        Some(AgentSessionDialect::Pi)
    );
    assert_eq!(
        AgentPSOutputParser::pi_dialect(&records[2]),
        Some(AgentSessionDialect::Omp)
    );
    assert_eq!(
        AgentPSOutputParser::pi_dialect(&records[3]),
        Some(AgentSessionDialect::Omp)
    );
    assert_eq!(
        AgentPSOutputParser::provider(&records[0]),
        Some(AgentSessionProvider::Pi)
    );
    assert_eq!(
        AgentPSOutputParser::provider(&records[2]),
        Some(AgentSessionProvider::Pi)
    );
}

// upstream: PiFamilySessionTests.swift:228 missing jsonl and unresolved custom roots retain pid only rows
#[tokio::test]
async fn missing_jsonl_and_unresolved_custom_roots_retain_pid_only_rows() {
    let root = tempfile::tempdir().expect("tempdir");
    let now = Timestamp::from_second(1_900_000_000).unwrap();
    let scanner = LocalAgentSessionScanner::with_providers(
        |_| {
            "41 1 Mon Aug 3 12:00:00 2026 /usr/local/bin/pi\n\
             42 1 Mon Aug 3 12:00:00 2026 /usr/local/bin/omp --profile missing"
                .to_owned()
        },
        |_, _| {
            HashMap::from([
                (41, "/tmp/no-jsonl-pi".to_owned()),
                (42, "/tmp/no-jsonl-omp".to_owned()),
            ])
        },
    );
    let env = HashMap::from([(
        "HOME".to_owned(),
        root.path().to_string_lossy().into_owned(),
    )]);
    let sessions = scanner.scan_with_env(now, &env, false).await;

    let ids: HashSet<String> = sessions.iter().map(|s| s.id.clone()).collect();
    assert_eq!(
        ids,
        ["pid:41".to_owned(), "pid:42".to_owned()]
            .into_iter()
            .collect()
    );
    assert!(
        sessions
            .iter()
            .all(|s| s.transcript_path.is_none() && s.state == AgentSessionState::Active)
    );
    let dialects: HashSet<AgentSessionDialect> =
        sessions.iter().filter_map(|s| s.dialect).collect();
    assert_eq!(
        dialects,
        [AgentSessionDialect::Pi, AgentSessionDialect::Omp]
            .into_iter()
            .collect()
    );
}
