use std::io::{self, IsTerminal, Read};

#[cfg(target_os = "linux")]
use std::io::Write;

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::secret_store::file::FileBackend;
use ariadusage_engine::brokers::secret_store::service::ServiceBackend;
use ariadusage_engine::brokers::secret_store::{SecretId, SecretStore, SecretStoreError};
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
    let file_backend =
        FileBackend::new(data_dir.join("ariadusage").join("secrets.json"), consented);
    let disabled = std::env::var_os("ARIADUSAGE_DISABLE_KEYRING").as_deref()
        == Some(std::ffi::OsStr::new("1"));
    let store =
        SecretStore::new(ServiceBackend::new(disabled)).with_file_fallback(file_backend, consented);
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
