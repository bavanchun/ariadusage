// Ported from CodexBar Tests/CodexBarTests/AgentSessionParserTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AgentSessionExecutableTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashMap;
use std::path::PathBuf;

use ariadusage_engine::sessions::{
    AgentPSOutputParser, AgentProcessRecord, AgentSessionCorrelation, AgentSessionDialect,
    AgentSessionProvider, AgentSessionSource, ChatGPTCodexProcessTrust, ClaudeTranscript,
    LSOFCWDOutputParser, LocalAgentSessionScanner,
};
use jiff::Timestamp;

fn fixture_string(name: &str) -> String {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir
        .parent()
        .expect("crates dir")
        .parent()
        .expect("workspace root")
        .join("fixtures")
        .join("codexbar")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", path.display()))
}

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

    let unrelated_bun = records.iter().find(|r| r.pid == 507).expect("pid 507");
    let unrelated_npm = records.iter().find(|r| r.pid == 508).expect("pid 508");
    assert_eq!(AgentPSOutputParser::provider(unrelated_bun), None);
    assert_eq!(AgentPSOutputParser::provider(unrelated_npm), None);

    for helper_pid in [503, 504, 505, 506] {
        let helper = records
            .iter()
            .find(|r| r.pid == helper_pid)
            .expect("helper pid");
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

    assert_eq!(
        AgentSessionCorrelation::newest_processes_first(&[older, newer])
            .iter()
            .map(|r| r.pid)
            .collect::<Vec<_>>(),
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
        assert!(!ChatGPTCodexProcessTrust::is_trusted_with(
            123,
            |_| actual_path.map(ToOwned::to_owned),
            |p| p,
            |_| {
                panic!("Unrecognized paths must be rejected before signature inspection");
            },
            |_| true,
        ));
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
        assert!(!AgentPSOutputParser::has_trusted_chatgpt_codex_app_server(
            &records,
            |_| {
                panic!("Non-server processes must not reach the app-server validator");
            }
        ));
        assert!(AgentPSOutputParser::agent_processes(&records).is_empty());
    }
}

// upstream: AgentSessionParserTests.swift:203 forged app server command cannot borrow installed ChatGPT identity
#[test]
fn forged_app_server_command_cannot_borrow_installed_chatgpt_identity() {
    let mut sleeper = std::process::Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .expect("spawn /bin/sleep");
    let pid = sleeper.id() as i32;
    let records = AgentPSOutputParser::parse(&format!(
        "{pid} 1 Mon Jul 6 09:03:00 2026 {NESTED_CHATGPT_CODEX} app-server"
    ));
    let trusted = AgentPSOutputParser::has_trusted_chatgpt_codex_app_server(&records, |r| {
        ChatGPTCodexProcessTrust::is_trusted(r.pid)
    });
    let _ = sleeper.kill();
    let _ = sleeper.wait();
    assert!(!trusted);
}

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
        let scanner = LocalAgentSessionScanner::new(
            move |_| Some(ps_line.clone()),
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
        let sessions = scanner.scan(&env, false).await;

        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session.id, "pid:101");
        assert_eq!(session.pid, Some(101));
        assert_eq!(session.provider.as_str(), provider);
        assert_eq!(session.source, AgentSessionSource::Cli);
        assert_eq!(session.project_name.as_deref(), Some("project"));
    }
}
