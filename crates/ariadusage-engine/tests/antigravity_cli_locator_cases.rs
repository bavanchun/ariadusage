// Ported from CodexBar Tests/CodexBarTests/AntigravityBinaryLocatorTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/AntigravityCLIStrategyLinuxTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/AntigravityPortDiscoveryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use ariadusage_core::pipeline::{
    FetchContext, FetchInteraction, FetchPhase, FetchRuntime, FetchStrategy,
};
#[cfg(target_os = "linux")]
use ariadusage_core::providers::SourceMode;
use ariadusage_engine::brokers::exec_resolver::{
    FileOwnerAndMode, ResolveError, ResolverFs, ResolverInputs, Tool, resolve,
};
#[cfg(target_os = "linux")]
use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
use ariadusage_engine::brokers::process::ProcessEnv;
#[cfg(target_os = "linux")]
use ariadusage_engine::providers::antigravity::cli_https::AntigravityCLIHTTPSFetchStrategy;
use ariadusage_engine::providers::antigravity::cli_https::{
    SnapshotWaitDependencies, wait_for_snapshot,
};
#[cfg(target_os = "linux")]
use ariadusage_engine::providers::antigravity::descriptor::AntigravityStrategyConfig;
use ariadusage_engine::providers::antigravity::status_probe::{
    AntigravityConnectionEndpoint, AntigravityModelQuota, AntigravitySnapshotSource,
    AntigravityStatusProbe, AntigravityStatusSnapshot, EndpointSource,
};
#[cfg(target_os = "linux")]
use ariadusage_protocol::ProviderId;
#[cfg(target_os = "linux")]
use ariadusage_protocol::secret::SecretString;
use tokio_util::sync::CancellationToken;

struct MockResolverFs {
    executables: HashSet<PathBuf>,
    current_uid: u32,
}

impl MockResolverFs {
    fn new(paths: &[&str]) -> Self {
        Self {
            executables: paths.iter().map(PathBuf::from).collect(),
            current_uid: 1000,
        }
    }
}

impl ResolverFs for MockResolverFs {
    fn is_executable(&self, path: &Path) -> bool {
        self.executables.contains(path)
    }

    fn owner_and_mode(&self, _path: &Path) -> Option<FileOwnerAndMode> {
        Some(FileOwnerAndMode {
            uid: self.current_uid,
            mode: 0o755,
        })
    }

    fn current_uid(&self) -> u32 {
        self.current_uid
    }
}

// upstream: AntigravityBinaryLocatorTests.swift:6 unusable env override fails without ambient fallback
#[test]
fn unusable_env_override_fails_without_ambient_fallback() {
    let ambient = "/opt/homebrew/bin/agy";
    let fs = MockResolverFs::new(&[ambient]);
    let home = Path::new("/fakehome");

    for override_value in ["/nonexistent-agy-guard", "", "   ", "agy"] {
        let env = ProcessEnv::from_iter([
            ("ANTIGRAVITY_CLI_PATH", override_value),
            ("PATH", "/opt/homebrew/bin"),
        ]);
        let login_path = [PathBuf::from("/opt/homebrew/bin")];
        let command_v = |_tool: &str| -> Option<PathBuf> {
            panic!("An unusable override must not run shell lookup");
        };
        let alias = |_tool: &str| -> Option<PathBuf> {
            panic!("An unusable override must not run alias lookup");
        };

        let inputs = ResolverInputs::new(&env, home, &fs)
            .with_login_path(&login_path)
            .with_command_v(&command_v)
            .with_alias(&alias);
        assert_eq!(
            resolve(Tool::Antigravity, &inputs),
            Err(ResolveError::OverrideInvalid)
        );
    }
}

// upstream: AntigravityBinaryLocatorTests.swift:33 usable env override is used without other lookups
#[test]
fn usable_env_override_is_used_without_other_lookups() {
    let override_path = "/custom/bin/agy";
    let fs = MockResolverFs::new(&[override_path]);
    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("ANTIGRAVITY_CLI_PATH", override_path)]);
    let command_v = |_tool: &str| -> Option<PathBuf> {
        panic!("A usable override must not run shell lookup");
    };
    let alias = |_tool: &str| -> Option<PathBuf> {
        panic!("A usable override must not run alias lookup");
    };

    let inputs = ResolverInputs::new(&env, home, &fs)
        .with_command_v(&command_v)
        .with_alias(&alias);
    assert_eq!(
        resolve(Tool::Antigravity, &inputs).unwrap().as_path(),
        Path::new(override_path)
    );
}

// upstream: AntigravityBinaryLocatorTests.swift:57 without override well known paths still resolve
#[test]
fn without_override_well_known_paths_still_resolve() {
    let homebrew_path = "/opt/homebrew/bin/agy";
    let fs = MockResolverFs::new(&[homebrew_path]);
    let home = Path::new("/fakehome");
    let env = ProcessEnv::from_iter([("SHELL", "/bin/zsh")]);
    let command_v = |_tool: &str| -> Option<PathBuf> { None };
    let alias = |_tool: &str| -> Option<PathBuf> { None };

    let inputs = ResolverInputs::new(&env, home, &fs)
        .with_command_v(&command_v)
        .with_alias(&alias);
    assert_eq!(
        resolve(Tool::Antigravity, &inputs).unwrap().as_path(),
        Path::new(homebrew_path)
    );
}

// upstream: AntigravityBinaryLocatorTests.swift:75 minimal fallback preserves a hit or no executable
#[test]
fn minimal_fallback_preserves_a_hit_or_no_executable() {
    let home = Path::new("/fakehome");
    let env = ProcessEnv::empty();
    let fs_hit = MockResolverFs::new(&["/bin/agy"]);
    let inputs_hit = ResolverInputs::new(&env, home, &fs_hit);
    assert_eq!(
        resolve(Tool::Antigravity, &inputs_hit).unwrap().as_path(),
        Path::new("/bin/agy")
    );

    let fs_miss = MockResolverFs::new(&[]);
    let inputs_miss = ResolverInputs::new(&env, home, &fs_miss);
    assert_eq!(
        resolve(Tool::Antigravity, &inputs_miss),
        Err(ResolveError::NotFound)
    );
}

#[cfg(target_os = "linux")]
fn fetch_context_with_binary(binary: &Path) -> FetchContext {
    FetchContext {
        provider: ProviderId::new("antigravity").unwrap(),
        runtime: FetchRuntime::Cli,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "antigravity-linux-locator-test".to_owned(),
        source_mode: SourceMode::Cli,
        env: [(
            "ANTIGRAVITY_CLI_PATH".to_owned(),
            SecretString::new(binary.to_string_lossy().into_owned()),
        )]
        .into_iter()
        .collect(),
        include_credits: false,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    }
}

#[cfg(target_os = "linux")]
fn strategy_for_tests() -> AntigravityCLIHTTPSFetchStrategy {
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    AntigravityCLIHTTPSFetchStrategy::new(AntigravityStrategyConfig::new(
        net,
        "/tmp/ariadusage-antigravity-linux-test",
        "/tmp/ariadusage-antigravity-linux-test-home",
    ))
}

#[cfg(target_os = "linux")]
fn synthetic_executable() -> tempfile::NamedTempFile {
    use std::os::unix::fs::PermissionsExt;
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    file
}

// upstream: AntigravityCLIStrategyLinuxTests.swift:7 cli local strategy is available with HTTP fallback
#[cfg(all(target_os = "linux", unix))]
#[tokio::test]
async fn cli_local_strategy_is_available_with_http_fallback() {
    let binary = synthetic_executable();
    let context = fetch_context_with_binary(binary.path());
    assert!(strategy_for_tests().is_available(&context).await);
}

// upstream: AntigravityCLIStrategyLinuxTests.swift:31 cli local endpoints include Linux HTTP fallback
#[test]
fn cli_local_endpoints_include_linux_http_fallback() {
    assert_eq!(
        AntigravityStatusProbe::cli_endpoints_for_os(&[55624], true),
        vec![
            AntigravityConnectionEndpoint::new("https", 55624, "", EndpointSource::CliHttps),
            AntigravityConnectionEndpoint::new("http", 55624, "", EndpointSource::CliHttps),
        ]
    );
}

// upstream: AntigravityCLIStrategyLinuxTests.swift:48 language server endpoints include Linux HTTP fallback
#[test]
fn language_server_endpoints_include_linux_http_fallback() {
    assert_eq!(
        AntigravityStatusProbe::connection_candidates_with_schemes(
            &[64440],
            Some("language-token"),
            None,
            None,
            &["https", "http"],
        ),
        vec![
            AntigravityConnectionEndpoint::new(
                "https",
                64440,
                "language-token",
                EndpointSource::LanguageServer,
            ),
            AntigravityConnectionEndpoint::new(
                "http",
                64440,
                "language-token",
                EndpointSource::LanguageServer,
            ),
        ]
    );
}

// upstream: AntigravityPortDiscoveryTests.swift:31 CLI readiness retries namespace warnings until the proc listener appears
#[tokio::test]
async fn cli_readiness_retries_until_the_listener_appears() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let seen_attempts = Arc::clone(&attempts);
    let fetched_ports = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_ports = Arc::clone(&fetched_ports);
    let dependencies = SnapshotWaitDependencies {
        poll_interval: Duration::ZERO,
        listening_ports: Arc::new(move |_, _| {
            let attempt = seen_attempts.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move {
                if attempt == 1 {
                    Err(ariadusage_engine::providers::antigravity::status_probe::AntigravityStatusProbeError::PortDetectionFailed(
                        "no listening ports found".to_owned(),
                    ))
                } else {
                    Ok(vec![8080])
                }
            })
        }),
        drain_output: Arc::new(|| Box::pin(async { Vec::new() })),
        fetch_snapshot: Arc::new(move |ports, _| {
            seen_ports.lock().unwrap().extend(ports);
            Box::pin(async {
                Ok(AntigravityStatusSnapshot {
                    model_quotas: vec![AntigravityModelQuota::new(
                        "Gemini Pro",
                        "gemini-pro",
                        Some(0.5),
                        None,
                        None,
                    )],
                    account_email: Some("fixture@example.com".to_owned()),
                    account_plan: Some("Pro".to_owned()),
                    source: AntigravitySnapshotSource::Local,
                    quota_summary: None,
                })
            })
        }),
        now: Arc::new(Instant::now),
    };
    let snapshot = wait_for_snapshot(
        42,
        Instant::now() + Duration::from_secs(2),
        None,
        &dependencies,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        snapshot.account_email.as_deref(),
        Some("fixture@example.com")
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(&*fetched_ports.lock().unwrap(), &[8080]);
}
