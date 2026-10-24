// Ported from CodexBar Sources/CodexBarCLI/CLIServeCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIDashboardCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIHelp.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
//
// CLI wiring for `ariadusage serve` and `ariadusage dashboard`.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use ariadusage_core::config::Config;
use ariadusage_core::pipeline::{
    FetchContext, FetchInteraction, FetchPhase, FetchRuntime, fetch_outcome,
};
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
use ariadusage_engine::config_store::ConfigStore;
use ariadusage_engine::paths;
use ariadusage_engine::registry::ProviderRegistry;
use ariadusage_engine::serve::accounts::FileManagedCodexAccountStore;
use ariadusage_engine::serve::auth::{CliServeDashboardAuth, CliServeSecurity};
use ariadusage_engine::serve::coordinator::CliServeOperationProgressSink;
use ariadusage_engine::serve::dashboard::{
    DashboardIdentityMode, DashboardManagedCodexAccounts, DashboardSnapshotBuilder,
    ProviderErrorPayload, ProviderPayload, ProviderStatusPayload, UsageCommandOutput,
    provider_selection_list, write_dashboard_snapshot_atomically,
};
use ariadusage_engine::serve::http_server::{
    CliLocalHttpServer, DEFAULT_MAXIMUM_CONNECTIONS, DEFAULT_READ_TIMEOUT_MS,
    DEFAULT_TOTAL_READ_TIMEOUT_MS,
};
use ariadusage_engine::serve::{
    CliServeArgumentError, CliServeDashboardTokenResolution, CliServeHeapTrimmer, ServeRuntime,
    clamped_serve_request_timeout, decode_dashboard_identity_mode, decode_serve_host,
    decode_serve_port, decode_serve_refresh_interval, decode_serve_request_timeout,
    handle_serve_request, resolve_dashboard_identity_mode, resolve_dashboard_token,
    serve_collect_usage_outputs, serve_provider_deadline, serve_usage_operation_fingerprint,
};
use ariadusage_engine::status::ProviderStatusFetcher;
use ariadusage_engine::{AppPreferences, EngineRuntime, TokioSleeper, build_default_secret_store};
use ariadusage_protocol::{ProviderId, SecretString};
use clap::Args;
use jiff::Timestamp;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub const SERVE_LONG_ABOUT: &str = "\
Start a foreground HTTP server that exposes CLI JSON payloads and a token-gated
dashboard snapshot, with a built-in web UI at /.

The server binds to 127.0.0.1 by default; `localhost` is normalized to 127.0.0.1.
GET /dashboard/v1/snapshot requires \"Authorization: Bearer YOUR_TOKEN\" and fails
closed (401) when no token is configured. Set the token with --dashboard-token or,
preferably, the ARIADUSAGE_DASHBOARD_TOKEN environment variable (argv leaks via ps).

A non-loopback --host requires both a dashboard token and --allow-plain-http.
On a non-loopback host the token also gates /accounts, /usage, and /cost;
/ and /health are always open.

Account IDs are stable opaque identifiers; clients must not parse their internal format.
Account discovery reads metadata only and never returns credentials or refreshes usage.";

pub const DASHBOARD_LONG_ABOUT: &str = "\
Print one dashboard-v1 snapshot as JSON, then exit. Honors enabled providers in
stable order and keeps provider failures as row-level errors without dropping
healthy rows.";

#[derive(Args, Debug, Clone, Default)]
pub struct ServeCommandArgs {
    /// IPv4 bind address or localhost (default: 127.0.0.1)
    #[arg(long, value_name = "host", allow_hyphen_values = true)]
    pub host: Option<String>,

    /// Local HTTP port (default: 8080)
    #[arg(long, value_name = "port", allow_hyphen_values = true)]
    pub port: Option<String>,

    /// Response cache TTL in seconds (default: 60)
    #[arg(
        long = "refresh-interval",
        value_name = "seconds",
        allow_hyphen_values = true
    )]
    pub refresh_interval: Option<String>,

    /// Total per-request deadline in seconds; 0 disables (default: 30)
    #[arg(
        long = "request-timeout",
        value_name = "seconds",
        allow_hyphen_values = true
    )]
    pub request_timeout: Option<String>,

    /// Bearer token for /dashboard/v1/snapshot (prefer ARIADUSAGE_DASHBOARD_TOKEN)
    #[arg(
        long = "dashboard-token",
        value_name = "token",
        allow_hyphen_values = true
    )]
    pub dashboard_token: Option<String>,

    /// Accept sending the dashboard token over cleartext HTTP on a non-loopback host
    #[arg(long = "allow-plain-http")]
    pub allow_plain_http: bool,

    /// Account identity detail: full or redacted
    #[arg(long, value_name = "redacted|full", allow_hyphen_values = true)]
    pub identity: Option<String>,

    /// Include all local accounts in dashboard snapshots
    #[arg(long = "all-accounts")]
    pub all_accounts: bool,

    /// Emit machine-readable logs
    #[arg(long = "json-output")]
    pub json_output: bool,

    /// Set log level (trace|verbose|debug|info|warning|error|critical)
    #[arg(
        long = "log-level",
        value_name = "trace|verbose|debug|info|warning|error|critical"
    )]
    pub log_level: Option<String>,

    /// Enable verbose logging
    #[arg(long)]
    pub verbose: bool,
}

#[derive(Args, Debug, Clone, Default)]
pub struct DashboardCommandArgs {
    /// Pretty-print JSON output
    #[arg(long)]
    pub pretty: bool,

    /// Total snapshot deadline in seconds; 0 disables (default: 30)
    #[arg(long, value_name = "seconds", allow_hyphen_values = true)]
    pub timeout: Option<String>,

    /// Write the snapshot atomically to a file instead of stdout
    #[arg(long, value_name = "path")]
    pub output: Option<PathBuf>,

    /// Account identity detail: full or redacted
    #[arg(long, value_name = "redacted|full", allow_hyphen_values = true)]
    pub identity: Option<String>,

    /// Emit machine-readable logs
    #[arg(long = "json-output")]
    pub json_output: bool,

    /// Set log level (trace|verbose|debug|info|warning|error|critical)
    #[arg(
        long = "log-level",
        value_name = "trace|verbose|debug|info|warning|error|critical"
    )]
    pub log_level: Option<String>,

    /// Enable verbose logging
    #[arg(long)]
    pub verbose: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeCliError {
    pub code: u8,
    pub kind: &'static str,
    pub message: String,
}

impl ServeCliError {
    #[must_use]
    pub fn args(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            kind: "args",
            message: message.into(),
        }
    }

    #[must_use]
    pub fn runtime(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            kind: "runtime",
            message: message.into(),
        }
    }
}

pub fn write_serve_cli_error(
    error: &ServeCliError,
    stdout: &mut dyn Write,
    _stderr: &mut dyn Write,
) {
    let payload = serde_json::json!({
        "error": {
            "code": i32::from(error.code),
            "message": error.message,
            "kind": error.kind,
        }
    });
    if let Ok(line) = serde_json::to_string(&payload) {
        let _ = writeln!(stdout, "{line}");
    }
}

pub fn validate_serve_args(
    args: &ServeCommandArgs,
    env: &BTreeMap<String, String>,
) -> Result<ValidatedServeOptions, ServeCliError> {
    let Some(port) = decode_serve_port(args.port.as_deref()) else {
        return Err(ServeCliError::args(
            CliServeArgumentError::InvalidPort.to_string(),
        ));
    };
    let Some(host) = decode_serve_host(args.host.as_deref()) else {
        return Err(ServeCliError::args(
            CliServeArgumentError::InvalidHost.to_string(),
        ));
    };
    let Some(refresh_interval) = decode_serve_refresh_interval(args.refresh_interval.as_deref())
    else {
        return Err(ServeCliError::args(
            CliServeArgumentError::InvalidRefreshInterval.to_string(),
        ));
    };
    let Some(request_timeout) = decode_serve_request_timeout(args.request_timeout.as_deref())
    else {
        return Err(ServeCliError::args(
            CliServeArgumentError::InvalidRequestTimeout.to_string(),
        ));
    };

    let dashboard_bearer = match resolve_dashboard_token(args.dashboard_token.as_deref(), env) {
        CliServeDashboardTokenResolution::Absent => None,
        CliServeDashboardTokenResolution::Token(t) => Some(t),
        CliServeDashboardTokenResolution::Empty { source } => {
            return Err(ServeCliError::args(
                CliServeArgumentError::EmptyDashboardToken {
                    source_name: source,
                }
                .to_string(),
            ));
        }
    };

    let Some(decoded_identity) = decode_dashboard_identity_mode(args.identity.as_deref()) else {
        return Err(ServeCliError::args(
            CliServeArgumentError::InvalidIdentity.to_string(),
        ));
    };
    let dashboard_identity_mode = args.identity.as_ref().map(|_| decoded_identity);
    let bind_host = CliServeSecurity::bind_host(Some(&host));
    let auth = CliServeDashboardAuth::new(dashboard_bearer.as_deref());
    if let Err(startup_err) =
        CliServeSecurity::validate_startup(&bind_host, &auth, args.allow_plain_http)
    {
        return Err(ServeCliError::args(startup_err.error_description()));
    }

    Ok(ValidatedServeOptions {
        bind_host,
        port,
        refresh_interval,
        request_timeout,
        auth,
        dashboard_identity_mode,
        all_accounts: args.all_accounts,
    })
}

#[derive(Debug, Clone)]
pub struct ValidatedServeOptions {
    pub bind_host: String,
    pub port: u16,
    pub refresh_interval: f64,
    pub request_timeout: f64,
    pub auth: CliServeDashboardAuth,
    pub dashboard_identity_mode: Option<DashboardIdentityMode>,
    pub all_accounts: bool,
}

pub fn run_serve_command(args: ServeCommandArgs) -> Result<(), ServeCliError> {
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let validated = validate_serve_args(&args, &env)?;
    let config_path = paths::config_path()
        .map_err(|err| ServeCliError::runtime(format!("Failed to resolve config path: {err}")))?;
    let store = ConfigStore::new(config_path);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| ServeCliError::runtime(format!("Failed to initialize runtime: {err}")))?;

    rt.block_on(async move {
        let mut runtime = ServeRuntime::new(
            store,
            validated.refresh_interval,
            validated.request_timeout,
            Some(env!("CARGO_PKG_VERSION").to_string()),
            validated.auth,
            validated.dashboard_identity_mode,
            validated.all_accounts,
            &validated.bind_host,
        );
        runtime.managed_accounts_store =
            FileManagedCodexAccountStore::new(FileManagedCodexAccountStore::default_path());
        install_live_engine_fetchers(&mut runtime);

        let trimmer =
            CliServeHeapTrimmer::start(CliServeHeapTrimmer::DEFAULT_INTERVAL, Arc::new(|| {}));

        let server = CliLocalHttpServer::bind(
            &validated.bind_host,
            validated.port,
            DEFAULT_READ_TIMEOUT_MS,
            DEFAULT_TOTAL_READ_TIMEOUT_MS,
            DEFAULT_MAXIMUM_CONNECTIONS,
        )
        .await
        .map_err(ServeCliError::runtime)?;

        let actual_port = server.port;
        eprintln!(
            "AriadUsage server listening on http://{}:{actual_port}",
            validated.bind_host
        );
        if !CliServeSecurity::is_loopback_host(&validated.bind_host) {
            eprintln!(
                "Warning: plain HTTP on a non-loopback host; the bearer token gating \
                 /usage, /cost, and /dashboard/v1/snapshot crosses the network \
                 in cleartext on every request."
            );
        }

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = shutdown_tx.send(true);
        });

        let rt_for_handler = runtime.clone();
        server
            .serve(
                shutdown_rx,
                Arc::new(move |req| {
                    let rt = rt_for_handler.clone();
                    Box::pin(async move { handle_serve_request(req, &rt).await })
                }),
            )
            .await;

        if let Some(t) = trimmer {
            t.cancel();
        }
        runtime.shutdown().await;
        Ok(())
    })
}

pub fn run_dashboard_command(args: DashboardCommandArgs) -> Result<(), ServeCliError> {
    let timeout = match args.timeout.as_deref() {
        None => 30.0,
        Some(raw) => {
            let Some(val) = decode_serve_request_timeout(Some(raw)) else {
                return Err(ServeCliError::args(
                    "--timeout must be a finite number of seconds from 0 through 86400."
                        .to_string(),
                ));
            };
            if val > 86_400.0 {
                return Err(ServeCliError::args(
                    "--timeout must be a finite number of seconds from 0 through 86400."
                        .to_string(),
                ));
            }
            val
        }
    };
    let Some(decoded_identity) = decode_dashboard_identity_mode(args.identity.as_deref()) else {
        return Err(ServeCliError::args(
            CliServeArgumentError::InvalidIdentity.to_string(),
        ));
    };
    let identity_mode = resolve_dashboard_identity_mode(
        args.identity.as_ref().map(|_| decoded_identity),
        false,
        false,
    );
    if let Some(output_path) = args.output.as_ref() {
        if output_path.as_os_str().is_empty() {
            return Err(ServeCliError::args(
                "--output requires a non-empty file path.".to_string(),
            ));
        }
        validate_dashboard_output_parent(output_path)?;
    }

    let config_path = paths::config_path()
        .map_err(|err| ServeCliError::runtime(format!("Failed to resolve config path: {err}")))?;
    let store = ConfigStore::new(config_path);
    let config = store
        .load_effective()
        .map_err(|err| ServeCliError::runtime(err.to_string()))?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| ServeCliError::runtime(format!("Failed to initialize runtime: {err}")))?;

    rt.block_on(async move {
        let mut runtime = ServeRuntime::new(
            store,
            0.0,
            clamped_serve_request_timeout(timeout),
            Some(env!("CARGO_PKG_VERSION").to_string()),
            CliServeDashboardAuth::new(None),
            Some(identity_mode),
            false,
            "127.0.0.1",
        );
        runtime.managed_accounts_store =
            FileManagedCodexAccountStore::new(FileManagedCodexAccountStore::default_path());
        install_live_engine_fetchers(&mut runtime);

        let started_at = Instant::now();
        let provider_deadline = serve_provider_deadline(started_at, timeout);
        let enabled = provider_selection_list(&config.enabled_providers());
        let usage_fp = serve_usage_operation_fingerprint("dashboard-cli", false, false);
        let u_fetcher = Arc::clone(&runtime.usage_fetcher);
        let cfg_u = config.clone();
        let usage_out = serve_collect_usage_outputs(
            &enabled,
            &usage_fp,
            provider_deadline,
            &runtime.provider_operations,
            move |prov, sink| u_fetcher(prov, cfg_u.clone(), sink),
        )
        .await;

        let mut account_collections = BTreeMap::new();
        if enabled.iter().any(|p| p == "codex")
            && let Some(coll) = DashboardManagedCodexAccounts::collect(
                &config,
                runtime.managed_accounts_store.path(),
            )
        {
            account_collections.insert("codex".to_string(), coll);
        }

        let snapshot = DashboardSnapshotBuilder::make_snapshot(
            &usage_out.payload,
            &[],
            &config,
            identity_mode,
            Timestamp::now(),
            0.0,
            Some(env!("CARGO_PKG_VERSION").to_string()),
            &account_collections,
            runtime.usage_bars_show_used(&config),
            false,
        );
        runtime.shutdown().await;

        let mut bytes = if args.pretty {
            serde_json::to_vec_pretty(&snapshot)
        } else {
            serde_json::to_vec(&snapshot)
        }
        .map_err(|err| ServeCliError::runtime(err.to_string()))?;

        if let Some(output_path) = args.output.as_ref() {
            bytes.push(b'\n');
            write_dashboard_snapshot_atomically(&bytes, output_path)
                .map_err(ServeCliError::runtime)?;
        } else {
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(&bytes);
            let _ = writeln!(stdout);
        }
        Ok(())
    })
}

pub fn validate_dashboard_output_parent(output_path: &Path) -> Result<(), ServeCliError> {
    let parent = output_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    match std::fs::metadata(parent) {
        Ok(meta) if meta.is_dir() => Ok(()),
        _ => Err(ServeCliError::args(format!(
            "--output directory does not exist: {} (parent directories are not created)",
            parent.display()
        ))),
    }
}

fn install_live_engine_fetchers(runtime: &mut ServeRuntime) {
    runtime.usage_fetcher = Arc::new(
        |provider: String,
         config: Config,
         _sink: CliServeOperationProgressSink<UsageCommandOutput>| {
            Box::pin(async move { fetch_live_provider_usage(&provider, &config).await })
        },
    );
}

async fn fetch_live_provider_usage(provider_raw: &str, config: &Config) -> UsageCommandOutput {
    let Ok(provider_id) = ProviderId::new(provider_raw) else {
        let mut item = ProviderPayload::new(provider_raw, "auto");
        item.error = Some(ProviderErrorPayload::provider(format!(
            "Unsupported provider '{provider_raw}'"
        )));
        return UsageCommandOutput {
            sections: Vec::new(),
            payload: vec![item],
            exit_code: 1,
        };
    };

    let Ok(net) = NetBroker::new(NetConfig::default()).map(Arc::new) else {
        let mut item = ProviderPayload::new(provider_raw, "auto");
        item.error = Some(ProviderErrorPayload::provider("network broker unavailable"));
        return UsageCommandOutput {
            sections: Vec::new(),
            payload: vec![item],
            exit_code: 1,
        };
    };

    let disable_keyring = std::env::var_os("ARIADUSAGE_DISABLE_KEYRING").as_deref()
        == Some(std::ffi::OsStr::new("1"));
    let secret_store = build_default_secret_store(config, disable_keyring).ok();
    let base_environment: BTreeMap<String, SecretString> = std::env::vars()
        .map(|(k, v)| (k, SecretString::new(v)))
        .collect();

    let registry = ProviderRegistry::default();
    let status_fetcher = ProviderStatusFetcher::default();
    let status_call = BrokerCall {
        interaction: FetchInteraction::Background,
        cancel: CancellationToken::new(),
        request_id: format!("cli-serve-status-{}", provider_id.as_str()),
    };

    let status_payload = if let Some(descriptor) = registry.descriptor(&provider_id)
        && descriptor.supports_status_polling()
    {
        let status_url = descriptor
            .effective_status_link_url()
            .unwrap_or("")
            .to_string();
        status_fetcher
            .fetch_for_descriptor(&net, &status_call, descriptor)
            .await
            .ok()
            .flatten()
            .map(|summary| ProviderStatusPayload {
                indicator: summary.status.indicator,
                description: summary.status.description,
                updated_at: summary.status.updated_at,
                url: status_url,
            })
    } else {
        None
    };

    let prefs = AppPreferences::from_config(config);
    let inputs = EngineRuntime::resolve_plan_inputs_with_store(
        secret_store.as_ref(),
        &provider_id,
        config,
        &prefs,
        base_environment,
        FetchInteraction::Background,
        CancellationToken::new(),
        format!("cli-serve-secret-{}", provider_id.as_str()),
    )
    .await;

    let Some(spec) = registry.resolve_spec(&provider_id, config, &inputs) else {
        let mut item = ProviderPayload::new(provider_raw, "auto");
        item.status = status_payload;
        item.error = Some(ProviderErrorPayload::provider(format!(
            "Provider '{provider_raw}' has no execution plan"
        )));
        return UsageCommandOutput {
            sections: Vec::new(),
            payload: vec![item],
            exit_code: 1,
        };
    };

    let cx = FetchContext {
        provider: provider_id.clone(),
        runtime: FetchRuntime::Cli,
        interaction: FetchInteraction::Background,
        phase: FetchPhase::Regular,
        request_id: format!("cli-serve-{}", provider_id.as_str()),
        source_mode: spec.source_mode,
        env: spec.environment,
        include_credits: true,
        selected_token_account: spec.selected_account.as_ref().map(|a| a.id.clone()),
        cancel: CancellationToken::new(),
    };

    let sleeper = TokioSleeper;
    let outcome = fetch_outcome(&provider_id, spec.plan.as_ref(), &cx, &sleeper).await;
    match outcome.result {
        Ok(fetch_res) => {
            let usage = fetch_res.usage.scoped_to(provider_id.clone());
            let mut item =
                ProviderPayload::new(provider_raw, fetch_res.source_label).with_usage(usage);
            item.status = status_payload;
            UsageCommandOutput {
                sections: Vec::new(),
                payload: vec![item],
                exit_code: 0,
            }
        }
        Err(err) => {
            let source_label = match spec.source_mode {
                ariadusage_core::providers::SourceMode::Auto => "auto",
                ariadusage_core::providers::SourceMode::Web => "web",
                ariadusage_core::providers::SourceMode::Cli => "cli",
                ariadusage_core::providers::SourceMode::Oauth => "oauth",
                ariadusage_core::providers::SourceMode::Api => "api",
            };
            let mut item = ProviderPayload::new(provider_raw, source_label);
            item.status = status_payload;
            item.error = Some(ProviderErrorPayload::provider(err.to_string()));
            UsageCommandOutput {
                sections: Vec::new(),
                payload: vec![item],
                exit_code: 1,
            }
        }
    }
}
