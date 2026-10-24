// Ported from CodexBar Tests/CodexBarTests/AntigravityStatusProbeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
use ariadusage_engine::providers::antigravity::*;
fn first(output: &str, scope: ProcessScope) -> ProcessInfoResult {
    AntigravityStatusProbe::process_infos_from_process_list_output(output, scope)
        .unwrap()
        .remove(0)
}

// upstream: AntigravityStatusProbeTests.swift:24 process detection accepts antigravity 2 unsuffixed language server
#[test]
fn process_detection_accepts_antigravity_2_unsuffixed_language_server() {
    assert!(
        AntigravityStatusProbe::is_antigravity_language_server_command_line(
            "/Applications/Antigravity.app/Contents/Resources/bin/language_server --standalone --override_ide_name antigravity --override_ide_version 2.0.0 --csrf_token token --app_data_dir antigravity\n"
        )
    );
}

// upstream: AntigravityStatusProbeTests.swift:35 process detection accepts antigravity language server paths with spaces
#[test]
fn process_detection_accepts_antigravity_language_server_paths_with_spaces() {
    assert!(
        AntigravityStatusProbe::is_antigravity_language_server_command_line(
            "/Applications/Google Antigravity.app/Contents/Resources/bin/language_server --standalone --override_ide_name antigravity --csrf_token token --app_data_dir antigravity\n"
        )
    );
}

// upstream: AntigravityStatusProbeTests.swift:60 process detection keeps ignoring non language server antigravity helpers
#[test]
fn process_detection_keeps_ignoring_non_language_server_antigravity_helpers() {
    assert!(
        !AntigravityStatusProbe::is_antigravity_language_server_command_line(
            "/Applications/Antigravity.app/Contents/Frameworks/Antigravity Helper.app/Contents/MacOS/Antigravity Helper --type=renderer --user-data-dir=/Users/test/Library/Application Support/Antigravity\n"
        )
    );
}

// upstream: AntigravityStatusProbeTests.swift:70 process detection still accepts legacy antigravity language server
#[test]
fn process_detection_still_accepts_legacy_antigravity_language_server() {
    assert!(
        AntigravityStatusProbe::is_antigravity_language_server_command_line(
            "/Applications/Antigravity.app/Contents/Resources/bin/language_server_macos --csrf_token token --app_data_dir antigravity\n"
        )
    );
}

// upstream: AntigravityStatusProbeTests.swift:45 process detection accepts hyphenated language server from app bundle
#[test]
fn process_detection_accepts_hyphenated_language_server_from_app_bundle() {
    let command = "/Applications/Google Antigravity.app/Contents/Resources/bin/language-server --standalone --csrf_token token --extension_server_port 64123\n";
    assert!(AntigravityStatusProbe::is_antigravity_language_server_command_line(command));
    let result = first(&format!("  321 {command}"), ProcessScope::IdeAndCli);
    assert_eq!(result.pid, 321);
    assert_eq!(result.csrf_token.as_deref(), Some("token"));
    assert_eq!(result.extension_port, Some(64123));
}

// upstream: AntigravityStatusProbeTests.swift:80 process detection accepts platform suffixed antigravity language server
#[test]
fn process_detection_accepts_platform_suffixed_antigravity_language_server() {
    let result = first(
        "101 /Applications/Antigravity.app/Contents/Resources/bin/language_server_macos_arm --csrf_token ide-token --app_data_dir antigravity --extension_server_port 54977\n",
        ProcessScope::AppOnly,
    );
    assert_eq!(result.pid, 101);
    assert_eq!(result.csrf_token.as_deref(), Some("ide-token"));
    assert_eq!(result.extension_port, Some(54977));
}

// upstream: AntigravityStatusProbeTests.swift:94 process detection accepts antigravity cli without csrf token
#[test]
fn process_detection_accepts_antigravity_cli_without_csrf_token() {
    assert!(
        AntigravityStatusProbe::is_antigravity_language_server_command_line(
            "node /Users/test/.gemini/antigravity-cli/build/mcp-server.cjs --app_data_dir /Users/test/.gemini/antigravity\n"
        )
    );
    assert!(
        AntigravityStatusProbe::is_antigravity_language_server_command_line(
            "/Users/test/.local/bin/agy -p hello"
        )
    );
    assert!(
        AntigravityStatusProbe::is_antigravity_language_server_command_line(
            "/usr/local/bin/agy --app_data_dir /Users/test/.gemini/antigravity_cli"
        )
    );
}

// upstream: AntigravityStatusProbeTests.swift:110 process detection ignores unrelated binaries containing agy substring
#[test]
fn process_detection_ignores_unrelated_binaries_containing_agy_substring() {
    assert!(
        !AntigravityStatusProbe::is_antigravity_language_server_command_line(
            "/usr/bin/legacy --run"
        )
    );
    assert!(
        !AntigravityStatusProbe::is_antigravity_language_server_command_line(
            "/opt/imagymagic/bin/tool"
        )
    );
}

// upstream: AntigravityStatusProbeTests.swift:117 process detection ignores cli names outside explicit cli path segments
#[test]
fn process_detection_ignores_cli_names_outside_explicit_cli_path_segments() {
    assert!(
        !AntigravityStatusProbe::is_antigravity_language_server_command_line(
            "/usr/bin/node /tmp/not-antigravity-cli/build/server.js"
        )
    );
    assert!(
        !AntigravityStatusProbe::is_antigravity_language_server_command_line(
            "/usr/bin/helper --workspace antigravity-cli"
        )
    );
}

// upstream: AntigravityStatusProbeTests.swift:127 process kind distinguishes app ide language server and cli
#[test]
fn process_kind_distinguishes_app_ide_language_server_and_cli() {
    assert_eq!(
        AntigravityStatusProbe::antigravity_process_kind(
            "/Applications/Antigravity.app/Contents/Resources/bin/language_server --csrf_token token --app_data_dir antigravity\n"
        ),
        Some(AntigravityProcessKind::App)
    );
    assert_eq!(
        AntigravityStatusProbe::antigravity_process_kind(
            "/Applications/Antigravity IDE.app/Contents/Resources/app/extensions/antigravity/bin/language_server_macos_arm --csrf_token token --app_data_dir antigravity-ide\n"
        ),
        Some(AntigravityProcessKind::Ide)
    );
    assert_eq!(
        AntigravityStatusProbe::antigravity_process_kind("/Users/test/.local/bin/agy -p hi"),
        Some(AntigravityProcessKind::Cli)
    );
    assert_eq!(
        AntigravityStatusProbe::antigravity_process_kind(
            "node /x/.gemini/antigravity-cli/build/mcp-server.cjs --app_data_dir /x/.gemini/antigravity"
        ),
        Some(AntigravityProcessKind::Cli)
    );
    assert_eq!(
        AntigravityStatusProbe::antigravity_process_kind("/usr/bin/legacy --run"),
        None
    );
}

// upstream: AntigravityStatusProbeTests.swift:146 csrf token stays required for ide but optional for cli
#[test]
fn csrf_token_stays_required_for_ide_but_optional_for_cli() {
    assert_eq!(AntigravityStatusProbe::resolved_csrf_token_for_kind(AntigravityProcessKind::App,
"/Applications/Antigravity.app/Contents/Resources/bin/language_server --csrf_token ide-token --app_data_dir antigravity\n").as_deref(),
Some("ide-token"));
    assert_eq!(
        AntigravityStatusProbe::resolved_csrf_token_for_kind(
            AntigravityProcessKind::App,
            "/Applications/Antigravity.app/Contents/Resources/bin/language_server --app_data_dir antigravity\n"
        ),
        None
    );
    assert_eq!(
        AntigravityStatusProbe::resolved_csrf_token_for_kind(
            AntigravityProcessKind::Cli,
            "/Users/test/.local/bin/agy -p hi"
        )
        .as_deref(),
        Some("")
    );
    assert_eq!(
        AntigravityStatusProbe::resolved_csrf_token_for_kind(
            AntigravityProcessKind::Cli,
            "/Users/test/.local/bin/agy --csrf_token cli-token"
        )
        .as_deref(),
        Some("cli-token")
    );
}

// upstream: AntigravityStatusProbeTests.swift:174 process scan skips tokenless ide before later valid ide
#[test]
fn process_scan_skips_tokenless_ide_before_later_valid_ide() {
    let output=["  100 /Applications/Antigravity.app/Contents/Resources/bin/language_server --app_data_dir antigravity",
"  101 /Applications/Antigravity.app/Contents/Resources/bin/language_server --csrf_token ide-token --app_data_dir antigravity --extension_server_port 64432 --extension_server_csrf_token extension-token"].join("\n");
    let result = first(&output, ProcessScope::IdeAndCli);
    assert_eq!(result.pid, 101);
    assert_eq!(result.csrf_token.as_deref(), Some("ide-token"));
    assert_eq!(result.extension_port, Some(64432));
    assert_eq!(
        result.extension_csrf_token.as_deref(),
        Some("extension-token")
    );
}

// upstream: AntigravityStatusProbeTests.swift:191 process scan returns all valid app candidates
#[test]
fn process_scan_returns_all_valid_app_candidates() {
    let output=["  101 /Applications/Antigravity.app/Contents/Resources/bin/language_server --csrf_token first-token --app_data_dir antigravity",
"  102 /Applications/Antigravity.app/Contents/Resources/bin/language_server --csrf_token second-token --app_data_dir antigravity --extension_server_port 64432 --extension_server_csrf_token extension-token"].join("\n");
    let results = AntigravityStatusProbe::process_infos_from_process_list_output(
        &output,
        ProcessScope::AppOnly,
    )
    .unwrap();
    assert_eq!(
        results.iter().map(|r| r.pid).collect::<Vec<_>>(),
        [101, 102]
    );
    assert_eq!(
        results
            .iter()
            .map(|r| r.csrf_token.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["first-token", "second-token"]
    );
    assert_eq!(results.last().unwrap().extension_port, Some(64432));
    assert_eq!(
        results.last().unwrap().extension_csrf_token.as_deref(),
        Some("extension-token")
    );
}

// upstream: AntigravityStatusProbeTests.swift:234 process scan reports missing csrf when only tokenless ide matches
#[test]
fn process_scan_reports_missing_csrf_when_only_tokenless_ide_matches() {
    assert!(matches!(
        AntigravityStatusProbe::process_infos_from_process_list_output(
            "100 /Applications/Antigravity.app/Contents/Resources/bin/language_server --app_data_dir antigravity\n",
            ProcessScope::IdeAndCli
        ),
        Err(AntigravityStatusProbeError::MissingCSRFToken)
    ));
}

// upstream: AntigravityStatusProbeTests.swift:245 process scan allows empty csrf only for explicit cli match
#[test]
fn process_scan_allows_empty_csrf_only_for_explicit_cli_match() {
    let result = first(
        "200 /Users/test/.local/bin/agy -p hello\n",
        ProcessScope::IdeAndCli,
    );
    assert_eq!(result.pid, 200);
    assert_eq!(result.csrf_token.as_deref(), Some(""));
    assert_eq!(result.command, "/Users/test/.local/bin/agy -p hello");
}

// upstream: AntigravityStatusProbeTests.swift:258 ideOnly scope skips app and cli processes and reports not running
#[test]
fn ideonly_scope_skips_app_and_cli_processes_and_reports_not_running() {
    assert!(matches!(
        AntigravityStatusProbe::process_infos_from_process_list_output(
            "  200 /Users/test/.local/bin/agy -p hello",
            ProcessScope::IdeOnly
        ),
        Err(AntigravityStatusProbeError::NotRunning)
    ));
    assert!(matches!(
        AntigravityStatusProbe::process_infos_from_process_list_output(
            "  101 /Applications/Antigravity.app/Contents/Resources/bin/language_server --csrf_token app-token --app_data_dir antigravity",
            ProcessScope::IdeOnly
        ),
        Err(AntigravityStatusProbeError::NotRunning)
    ));
}

// upstream: AntigravityStatusProbeTests.swift:273 ideOnly scope still matches ide server listed after cli and app processes
#[test]
fn ideonly_scope_still_matches_ide_server_listed_after_cli_and_app_processes() {
    let output=["  200 /Users/test/.local/bin/agy -p hello",
"  101 /Applications/Antigravity.app/Contents/Resources/bin/language_server --csrf_token app-token --app_data_dir antigravity",
"  102 /Applications/Antigravity IDE.app/Contents/Resources/app/extensions/antigravity/bin/language_server_macos_arm --csrf_token ide-token --app_data_dir antigravity"].join("\n");
    let result = first(&output, ProcessScope::IdeOnly);
    assert_eq!(result.pid, 102);
    assert_eq!(result.csrf_token.as_deref(), Some("ide-token"));
}

// upstream: AntigravityStatusProbeTests.swift:289 appOnly scope skips ide and cli processes
#[test]
fn apponly_scope_skips_ide_and_cli_processes() {
    let output=["  200 /Users/test/.local/bin/agy -p hello",
"  102 /Applications/Antigravity IDE.app/Contents/Resources/app/extensions/antigravity/bin/language_server_macos_arm --csrf_token ide-token --app_data_dir antigravity-ide",
"  101 /Applications/Antigravity.app/Contents/Resources/bin/language_server --csrf_token app-token --app_data_dir antigravity"].join("\n");
    let result = first(&output, ProcessScope::AppOnly);
    assert_eq!(result.pid, 101);
    assert_eq!(result.csrf_token.as_deref(), Some("app-token"));
}

// upstream: AntigravityStatusProbeTests.swift:306 localhost trust policy only accepts local server trust challenges
#[test]
fn localhost_trust_policy_only_accepts_local_server_trust_challenges() {
    assert!(LocalhostTrustPolicy::should_accept_server_trust_challenge(
        LocalhostTrustPolicy::SERVER_TRUST_AUTH_METHOD,
        "127.0.0.1",
        true
    ));
    assert!(LocalhostTrustPolicy::should_accept_server_trust_challenge(
        LocalhostTrustPolicy::SERVER_TRUST_AUTH_METHOD,
        "LOCALHOST",
        true
    ));
    assert!(!LocalhostTrustPolicy::should_accept_server_trust_challenge(
        LocalhostTrustPolicy::SERVER_TRUST_AUTH_METHOD,
        "cursor.com",
        true
    ));
    assert!(!LocalhostTrustPolicy::should_accept_server_trust_challenge(
        "NSURLAuthenticationMethodHTTPBasic",
        "127.0.0.1",
        true
    ));
    assert!(!LocalhostTrustPolicy::should_accept_server_trust_challenge(
        LocalhostTrustPolicy::SERVER_TRUST_AUTH_METHOD,
        "127.0.0.1",
        false
    ));
}

// upstream: AntigravityStatusProbeTests.swift:336 localhost trust policy rejects non loopback hostnames that contain localhost
#[test]
fn localhost_trust_policy_rejects_non_loopback_hostnames_that_contain_localhost() {
    assert!(!LocalhostTrustPolicy::should_accept_server_trust_challenge(
        LocalhostTrustPolicy::SERVER_TRUST_AUTH_METHOD,
        "localhost.example.com",
        true
    ));
}
