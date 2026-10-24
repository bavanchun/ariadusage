pub mod hooks;

#[cfg(target_os = "macos")]
use ariadusage_cli::launch_agent;
use ariadusage_cli::{
    CacheClearOptions, CookieRefreshOptions, DiagnoseOptions, OutputFormat, execute_cache_clear,
    execute_cookie_refresh, execute_diagnose, execute_usage,
};

use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use std::io::Write;

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
#[cfg(target_os = "linux")]
use ariadusage_engine::brokers::secret_store::SecretStore;
#[cfg(target_os = "linux")]
use ariadusage_engine::brokers::secret_store::file::FileBackend;
#[cfg(target_os = "linux")]
use ariadusage_engine::brokers::secret_store::service::ServiceBackend;
use ariadusage_engine::brokers::secret_store::{SecretId, SecretStoreError};
use ariadusage_engine::build_default_secret_store;
use ariadusage_engine::config_store::ConfigStore;
use ariadusage_engine::hardening::harden_process;
use ariadusage_engine::paths;
use ariadusage_protocol::ids::SettingId;
use ariadusage_protocol::secret::SecretString;
use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

const MAX_SECRET_BYTES: usize = 64 * 1024;

#[derive(Parser, Debug)]
#[command(
    name = "ariadusage",
    version = env!("CARGO_PKG_VERSION"),
    about = "Track usage and quotas for AI coding assistants"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Manage sensitive secrets
    Secret {
        #[command(subcommand)]
        command: SecretCommands,
    },

    /// Run the foreground engine daemon or manage its per-user LaunchAgent on macOS
    Daemon {
        /// Optional socket path override when running the daemon
        #[arg(long)]
        socket: Option<PathBuf>,

        #[command(subcommand)]
        action: Option<DaemonCommands>,
    },

    /// Query the running engine daemon over IPC v1 for its current snapshot
    Status {
        /// Optional socket path override
        #[arg(long)]
        socket: Option<PathBuf>,

        /// Output the full Snapshot v1 JSON document
        #[arg(long)]
        json: bool,
    },

    /// Run external commands on quota/provider events
    #[command(long_about = hooks::HOOKS_LONG_ABOUT)]
    Hooks(hooks::HooksCommandArgs),

    /// Export redacted provider diagnostic JSON
    Diagnose {
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        format: Option<String>,
        #[arg(long)]
        redact: bool,
        #[arg(long)]
        output: Option<String>,
        #[arg(long)]
        pretty: bool,
        #[arg(short = 'v', long)]
        verbose: bool,
        #[arg(long)]
        json_output: bool,
        #[arg(long)]
        log_level: Option<String>,
        #[arg(long)]
        socket: Option<PathBuf>,
    },

    /// Manage cached browser cookies and local cost scan caches
    Cache {
        #[command(subcommand)]
        command: CacheCommands,
    },

    /// Manage browser session cookies
    Cookie {
        #[command(subcommand)]
        command: CookieCommands,
    },

    /// Fetch provider usage metrics directly for CLI inspection
    Usage {
        #[arg(long)]
        provider: Option<String>,
        #[arg(short = 'v', long)]
        verbose: bool,
        #[arg(long)]
        pretty: bool,
    },
}

#[derive(Subcommand, Debug)]
enum CacheCommands {
    /// Clear browser cookie caches, cost scan caches, or both
    Clear {
        #[arg(long)]
        cookies: bool,
        #[arg(long)]
        cost: bool,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        format: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        json_only: bool,
        #[arg(long)]
        pretty: bool,
        #[arg(short = 'v', long)]
        verbose: bool,
        #[arg(long)]
        json_output: bool,
        #[arg(long)]
        log_level: Option<String>,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum CookieCommands {
    /// Re-import browser cookies for a provider or all supported providers
    Refresh {
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        allow_keychain_prompt: bool,
        #[arg(long)]
        format: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        json_only: bool,
        #[arg(long)]
        pretty: bool,
        #[arg(short = 'v', long)]
        verbose: bool,
        #[arg(long)]
        json_output: bool,
    },
}

#[derive(Subcommand, Debug)]
enum DaemonCommands {
    /// Run the engine daemon in the foreground
    Run {
        /// Optional socket path override
        #[arg(long)]
        socket: Option<PathBuf>,
    },

    /// Write the per-user LaunchAgent plist to ~/Library/LaunchAgents/ on macOS
    Write {
        /// Override the absolute path to the ariadusage binary written into ProgramArguments
        #[arg(long)]
        binary: Option<PathBuf>,
    },

    /// Write (if missing) and load the per-user LaunchAgent via /bin/launchctl bootstrap
    #[command(visible_alias = "install")]
    Load {
        /// Override the absolute path to the ariadusage binary written into ProgramArguments
        #[arg(long)]
        binary: Option<PathBuf>,
    },

    /// Unload the per-user LaunchAgent via /bin/launchctl bootout
    Unload,

    /// Unload and remove the per-user LaunchAgent plist from ~/Library/LaunchAgents/
    #[command(visible_alias = "uninstall")]
    Remove,
}

#[derive(Subcommand, Debug)]
enum SecretCommands {
    /// Set a provider secret
    Set {
        /// Identifier of the setting to store
        #[arg(long)]
        id: String,

        /// Allow a private file when the keyring is unavailable
        #[arg(long)]
        allow_file_fallback: bool,
    },
}

#[derive(Clone, Copy)]
struct CliError {
    code: u8,
    message: &'static str,
}

impl CliError {
    const fn new(code: u8, message: &'static str) -> Self {
        Self { code, message }
    }
}

fn main() {
    if harden_process().is_err() {
        report(CliError::new(18, "process hardening failed"));
    }

    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Secret {
            command:
                SecretCommands::Set {
                    id,
                    allow_file_fallback,
                },
        }) => {
            if let Err(error) = run_secret_set(&id, allow_file_fallback) {
                report(error);
            }
        }
        Some(Commands::Daemon { socket, action }) => {
            if let Err(error) = run_daemon_command(socket, action) {
                report(error);
            }
        }
        Some(Commands::Status { socket, json }) => {
            if let Err(error) = run_status_command(socket, json) {
                report(error);
            }
        }
        Some(Commands::Hooks(args)) => {
            if let Err(error) = hooks::run_hooks_command(args) {
                let mut stdout = io::stdout();
                let mut stderr = io::stderr();
                hooks::write_hooks_cli_error(&error, &mut stdout, &mut stderr);
                std::process::exit(i32::from(error.code));
            }
        }
        Some(Commands::Diagnose {
            provider,
            format,
            redact,
            output,
            pretty,
            verbose,
            json_output,
            log_level,
            socket,
        }) => {
            let has_output_file = output.as_deref().is_some_and(|p| !p.is_empty());
            let opts = DiagnoseOptions {
                provider,
                format,
                redact,
                output,
                pretty,
                verbose,
                json_output,
                log_level,
                socket,
            };
            match execute_diagnose(&opts) {
                Ok(rendered) => {
                    if !has_output_file {
                        println!("{rendered}");
                    }
                }
                Err((code, msg)) => {
                    eprintln!("{msg}");
                    std::process::exit(i32::from(code));
                }
            }
        }
        Some(Commands::Cache {
            command:
                CacheCommands::Clear {
                    cookies,
                    cost,
                    all,
                    provider,
                    format,
                    json,
                    json_only,
                    pretty,
                    verbose,
                    json_output,
                    log_level,
                    socket,
                },
        }) => {
            let parsed_format = match format {
                Some(raw) => match OutputFormat::parse(&raw) {
                    Some(fmt) => Some(fmt),
                    None => {
                        eprintln!("Invalid format: {raw}");
                        std::process::exit(2);
                    }
                },
                None => None,
            };
            let opts = CacheClearOptions {
                cookies,
                cost,
                all,
                provider,
                format: parsed_format,
                json_shortcut: json,
                json_only,
                pretty,
                verbose,
                json_output,
                log_level,
            };
            match execute_cache_clear(&opts, socket.as_deref()) {
                Ok((_results, rendered)) => {
                    if !rendered.is_empty() {
                        println!("{rendered}");
                    }
                }
                Err((code, msg)) => {
                    eprintln!("{msg}");
                    std::process::exit(i32::from(code));
                }
            }
        }
        Some(Commands::Cookie {
            command:
                CookieCommands::Refresh {
                    provider,
                    all,
                    allow_keychain_prompt,
                    format,
                    json,
                    json_only,
                    pretty,
                    verbose,
                    json_output,
                },
        }) => {
            let parsed_format = match format {
                Some(raw) => match OutputFormat::parse(&raw) {
                    Some(fmt) => Some(fmt),
                    None => {
                        eprintln!("Invalid format: {raw}");
                        std::process::exit(2);
                    }
                },
                None => None,
            };
            let opts = CookieRefreshOptions {
                verbose,
                json_output,
                json_shortcut: json,
                json_only,
                pretty,
                format: parsed_format,
                all,
                provider,
                allow_keychain_prompt,
            };
            match execute_cookie_refresh(&opts) {
                Ok(rendered) => {
                    if !rendered.is_empty() {
                        println!("{rendered}");
                    }
                }
                Err((code, msg)) => {
                    if opts.effective_format() == OutputFormat::Json {
                        println!("{msg}");
                    } else {
                        eprintln!("{msg}");
                    }
                    std::process::exit(i32::from(code));
                }
            }
        }
        Some(Commands::Usage {
            provider,
            verbose,
            pretty,
        }) => match execute_usage(provider.as_deref(), verbose, pretty) {
            Ok(rendered) => println!("{rendered}"),
            Err((code, msg)) => {
                eprintln!("{msg}");
                std::process::exit(i32::from(code));
            }
        },
        None => {}
    }
}

fn report(error: CliError) -> ! {
    eprintln!("ariadusage: {}", error.message);
    std::process::exit(i32::from(error.code));
}

fn run_secret_set(id: &str, allow_file_fallback: bool) -> Result<(), CliError> {
    let setting_id = SettingId::new(id).map_err(|_| CliError::new(2, "invalid setting id"))?;
    let secret_id = SecretId::from_setting_id(&setting_id)
        .map_err(|_| CliError::new(2, "invalid setting id"))?;
    let secret = read_secret()?;
    if secret.expose_secret().is_empty() {
        return Err(CliError::new(5, "secret is empty"));
    }

    let config_path =
        paths::config_path().map_err(|_| CliError::new(16, "configuration unavailable"))?;
    let config_store = ConfigStore::new(config_path);
    let config = config_store
        .load_effective()
        .map_err(|_| CliError::new(16, "configuration unavailable"))?;
    let consented = config.secret_file_fallback == Some(true);
    let data_dir =
        paths::data_dir().map_err(|_| CliError::new(16, "data directory unavailable"))?;
    let disabled = std::env::var_os("ARIADUSAGE_DISABLE_KEYRING").as_deref()
        == Some(std::ffi::OsStr::new("1"));
    let store = build_default_secret_store(&config, disabled)
        .map_err(|_| CliError::new(16, "data directory unavailable"))?;
    let call = user_call();
    let runtime = current_thread_runtime()?;

    match runtime.block_on(store.user(call.clone()).set(&secret_id, &secret)) {
        Ok(()) => {
            println!("saved");
            Ok(())
        }
        Err(SecretStoreError::Unavailable) if !consented => {
            #[cfg(target_os = "linux")]
            {
                if !obtain_file_fallback_consent(allow_file_fallback)? {
                    return Err(CliError::new(15, "file fallback consent required"));
                }
                config_store
                    .update(|config| config.secret_file_fallback = Some(true))
                    .map_err(|_| CliError::new(16, "could not save fallback consent"))?;

                // The first backend call established that the service is unavailable. Keep
                // the fallback write in-process without making a second D-Bus request.
                let file_store = SecretStore::new(ServiceBackend::new(true)).with_file_fallback(
                    FileBackend::new(data_dir.join("ariadusage").join("secrets.json"), true),
                    true,
                );
                runtime
                    .block_on(file_store.user(call).set(&secret_id, &secret))
                    .map_err(map_store_error)?;
                println!("saved");
                Ok(())
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (allow_file_fallback, config_store, data_dir);
                Err(CliError::new(
                    17,
                    "file fallback is unsupported on this platform",
                ))
            }
        }
        Err(error) => Err(map_store_error(error)),
    }
}

fn user_call() -> BrokerCall {
    BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "secret-set".to_owned(),
    }
}

fn current_thread_runtime() -> Result<tokio::runtime::Runtime, CliError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| CliError::new(16, "could not start secret store"))
}

fn read_secret() -> Result<SecretString, CliError> {
    if io::stdin().is_terminal() {
        let secret = rpassword::prompt_password("Secret: ")
            .map_err(|_| CliError::new(3, "could not read secret"))?;
        let mut secret = Zeroizing::new(secret);
        if secret.len() > MAX_SECRET_BYTES {
            return Err(CliError::new(4, "secret exceeds 64 KiB"));
        }
        return Ok(SecretString::new(std::mem::take(&mut *secret)));
    }

    let mut bytes = Zeroizing::new(vec![0_u8; MAX_SECRET_BYTES + 1]);
    let mut filled = 0;
    let mut stdin = io::stdin().lock();
    while filled < bytes.len() {
        match stdin.read(&mut bytes[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(_) => return Err(CliError::new(3, "could not read secret")),
        }
    }
    if filled > MAX_SECRET_BYTES {
        return Err(CliError::new(4, "secret exceeds 64 KiB"));
    }
    bytes.truncate(filled);
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    let mut text = match String::from_utf8(std::mem::take(&mut *bytes)) {
        Ok(text) => Zeroizing::new(text),
        Err(error) => {
            drop(Zeroizing::new(error.into_bytes()));
            return Err(CliError::new(5, "secret input is not valid UTF-8"));
        }
    };
    Ok(SecretString::new(std::mem::take(&mut *text)))
}

#[cfg(target_os = "linux")]
fn obtain_file_fallback_consent(flag: bool) -> Result<bool, CliError> {
    if flag {
        return Ok(true);
    }
    if !io::stdin().is_terminal() {
        return Ok(false);
    }

    eprint!("The keyring is unavailable. Store this secret in a private file? [y/N] ");
    io::stderr()
        .flush()
        .map_err(|_| CliError::new(3, "could not read consent"))?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|_| CliError::new(3, "could not read consent"))?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn map_store_error(error: SecretStoreError) -> CliError {
    match error {
        SecretStoreError::Unavailable => CliError::new(10, "keyring unavailable"),
        SecretStoreError::Locked => CliError::new(11, "keyring is locked"),
        SecretStoreError::Busy => CliError::new(11, "keyring is busy with an active prompt"),
        SecretStoreError::Dismissed => CliError::new(12, "keyring unlock was dismissed"),
        SecretStoreError::Timeout => CliError::new(13, "keyring operation timed out"),
        SecretStoreError::Cancelled => CliError::new(14, "keyring operation was cancelled"),
        SecretStoreError::Invalid => CliError::new(5, "secret is invalid"),
        SecretStoreError::ConsentRequired => CliError::new(15, "file fallback consent required"),
        SecretStoreError::Unsupported => {
            CliError::new(17, "secret operation is unsupported on this platform")
        }
        SecretStoreError::Storage => CliError::new(16, "secret storage failed"),
    }
}

fn run_daemon_command(
    socket: Option<PathBuf>,
    action: Option<DaemonCommands>,
) -> Result<(), CliError> {
    match action {
        None => run_foreground_daemon(socket),
        Some(DaemonCommands::Run { socket: sub_socket }) => {
            run_foreground_daemon(sub_socket.or(socket))
        }
        Some(DaemonCommands::Write { binary }) => {
            #[cfg(target_os = "macos")]
            {
                let paths = current_launch_agent_paths()?;
                let bin = resolve_binary_path(binary)?;
                launch_agent::write_launch_agent_plist(&bin, &paths)
                    .map_err(|_| CliError::new(16, "could not write LaunchAgent plist"))?;
                println!("{}", paths.plist_path.display());
                Ok(())
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = binary;
                Err(CliError::new(
                    19,
                    "LaunchAgent management is supported only on macOS",
                ))
            }
        }
        Some(DaemonCommands::Load { binary }) => {
            #[cfg(target_os = "macos")]
            {
                let paths = current_launch_agent_paths()?;
                if binary.is_some() || !paths.plist_path.exists() {
                    let bin = resolve_binary_path(binary)?;
                    launch_agent::write_launch_agent_plist(&bin, &paths)
                        .map_err(|_| CliError::new(16, "could not write LaunchAgent plist"))?;
                }
                let uid = rustix::process::geteuid().as_raw();
                launch_agent::load_launch_agent(uid, &paths, &launch_agent::SystemLaunchctlRunner)
                    .map_err(|_| CliError::new(16, "launchctl bootstrap failed"))?;
                println!("loaded {}", launch_agent::LAUNCH_AGENT_LABEL);
                Ok(())
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = binary;
                Err(CliError::new(
                    19,
                    "LaunchAgent management is supported only on macOS",
                ))
            }
        }
        Some(DaemonCommands::Unload) => {
            #[cfg(target_os = "macos")]
            {
                let paths = current_launch_agent_paths()?;
                let uid = rustix::process::geteuid().as_raw();
                launch_agent::unload_launch_agent(
                    uid,
                    &paths,
                    &launch_agent::SystemLaunchctlRunner,
                )
                .map_err(|_| CliError::new(16, "launchctl bootout failed"))?;
                println!("unloaded {}", launch_agent::LAUNCH_AGENT_LABEL);
                Ok(())
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(CliError::new(
                    19,
                    "LaunchAgent management is supported only on macOS",
                ))
            }
        }
        Some(DaemonCommands::Remove) => {
            #[cfg(target_os = "macos")]
            {
                let paths = current_launch_agent_paths()?;
                let uid = rustix::process::geteuid().as_raw();
                launch_agent::remove_launch_agent(
                    uid,
                    &paths,
                    &launch_agent::SystemLaunchctlRunner,
                )
                .map_err(|_| CliError::new(16, "could not remove LaunchAgent plist"))?;
                println!("removed {}", launch_agent::LAUNCH_AGENT_LABEL);
                Ok(())
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(CliError::new(
                    19,
                    "LaunchAgent management is supported only on macOS",
                ))
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn current_launch_agent_paths() -> Result<launch_agent::LaunchAgentPaths, CliError> {
    launch_agent::resolve_launch_agent_paths(
        |var| std::env::var_os(var),
        etcetera::home_dir().as_deref().ok(),
    )
    .map_err(|_| CliError::new(16, "could not resolve LaunchAgent paths"))
}

#[cfg(target_os = "macos")]
fn resolve_binary_path(override_path: Option<PathBuf>) -> Result<PathBuf, CliError> {
    let path = match override_path {
        Some(p) => p,
        None => std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .map_err(|_| CliError::new(16, "could not resolve ariadusage binary path"))?,
    };
    if !path.is_absolute() {
        return Err(CliError::new(2, "binary path must be absolute"));
    }
    Ok(path)
}

fn run_foreground_daemon(socket: Option<PathBuf>) -> Result<(), CliError> {
    #[cfg(unix)]
    {
        let disable_keyring = std::env::var_os("ARIADUSAGE_DISABLE_KEYRING").as_deref()
            == Some(std::ffi::OsStr::new("1"));
        let runtime = current_thread_runtime()?;
        runtime
            .block_on(ariadusage_engine::daemon::run_daemon_with_options(
                socket,
                disable_keyring,
            ))
            .map_err(|_| CliError::new(16, "engine daemon failed"))
    }
    #[cfg(not(unix))]
    {
        let _ = socket;
        Err(CliError::new(
            19,
            "engine daemon is unsupported on this platform",
        ))
    }
}

fn run_status_command(socket: Option<PathBuf>, json: bool) -> Result<(), CliError> {
    #[cfg(unix)]
    {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;
        use std::time::Duration;

        use ariadusage_protocol::ipc::{
            ClientInfo, ClientMessage, PROTOCOL_V1, ServerMessage, parse_server_message,
        };

        let sock = match socket {
            Some(p) => {
                paths::validate_socket_path_len(&p)
                    .map_err(|_| CliError::new(2, "socket path exceeds sun_path limit"))?;
                p
            }
            None => paths::socket_path()
                .map_err(|_| CliError::new(16, "trusted runtime socket unavailable"))?,
        };

        let mut stream = UnixStream::connect(&sock)
            .map_err(|_| CliError::new(16, "could not connect to engine daemon socket"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|_| CliError::new(16, "could not configure socket timeout"))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(|_| CliError::new(16, "could not configure socket timeout"))?;

        let hello = ClientMessage::Hello {
            protocols: vec![PROTOCOL_V1.to_string()],
            client: ClientInfo::new("ariadusage-cli", env!("CARGO_PKG_VERSION")),
            id: None,
        };
        let get_snap = ClientMessage::GetSnapshot { id: None };
        for msg in [&hello, &get_snap] {
            let mut line =
                serde_json::to_vec(msg).map_err(|_| CliError::new(16, "ipc encode failed"))?;
            line.push(b'\n');
            stream
                .write_all(&line)
                .map_err(|_| CliError::new(16, "ipc write failed"))?;
        }
        stream
            .flush()
            .map_err(|_| CliError::new(16, "ipc flush failed"))?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        while reader
            .read_line(&mut line)
            .map_err(|_| CliError::new(16, "ipc read failed"))?
            > 0
        {
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if let Ok(ServerMessage::Snapshot { snapshot }) = parse_server_message(trimmed) {
                if json {
                    let text = serde_json::to_string_pretty(&snapshot)
                        .map_err(|_| CliError::new(16, "snapshot format failed"))?;
                    println!("{text}");
                } else {
                    println!("engine {}", snapshot.engine.version);
                    for p in &snapshot.providers {
                        println!(
                            "{}: enabled={} source={}",
                            p.id.as_str(),
                            p.enabled,
                            p.source_mode
                        );
                    }
                }
                return Ok(());
            }
            line.clear();
        }

        Err(CliError::new(16, "did not receive snapshot from daemon"))
    }
    #[cfg(not(unix))]
    {
        let _ = (socket, json);
        Err(CliError::new(
            19,
            "engine status is unsupported on this platform",
        ))
    }
}
