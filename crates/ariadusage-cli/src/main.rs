use std::io::IsTerminal;

use clap::{Parser, Subcommand};

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

        /// Path to the engine Unix socket
        #[arg(long)]
        socket: Option<std::path::PathBuf>,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Secret {
            command: SecretCommands::Set { id, socket },
        }) => {
            if let Err(err) = run_secret_set(&id, socket.as_deref()) {
                eprintln!("{err}");
                std::process::exit(1);
            }
        }
        None => {}
    }
}

#[cfg(unix)]
fn run_secret_set(id: &str, socket_path: Option<&std::path::Path>) -> Result<(), String> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    use ariadusage_protocol::ids::SettingId;
    use ariadusage_protocol::ipc::{ClientInfo, ClientMessage, PROTOCOL_V1, ServerMessage};
    use ariadusage_protocol::secret::SecretString;

    let setting_id = SettingId::new(id).map_err(|e| e.to_string())?;

    let default_sock = std::env::var("XDG_RUNTIME_DIR")
        .map(|dir| std::path::PathBuf::from(dir).join("ariadusage/engine.sock"))
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/ariadusage/engine.sock"));
    let sock = socket_path.unwrap_or(&default_sock);

    let secret = if std::io::stdin().is_terminal() {
        rpassword::prompt_password(format!("Enter secret for {id}: "))
            .map_err(|e| format!("failed to read secret: {e}"))?
    } else {
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| format!("failed to read from stdin: {e}"))?;
        line.trim_end_matches(&['\r', '\n'][..]).to_string()
    };

    let mut stream = UnixStream::connect(sock)
        .map_err(|e| format!("failed to connect to engine at {}: {e}", sock.display()))?;

    let hello = ClientMessage::Hello {
        protocols: vec![PROTOCOL_V1.to_string()],
        client: ClientInfo {
            name: "ariadusage-cli".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        id: None,
    };
    let hello_json = serde_json::to_string(&hello).map_err(|e| e.to_string())?;
    writeln!(stream, "{hello_json}").map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut welcome_line = String::new();
    reader
        .read_line(&mut welcome_line)
        .map_err(|e| e.to_string())?;

    let set_secret = ClientMessage::SetSecret {
        id: setting_id,
        value: SecretString::new(secret),
    };
    let set_secret_json = serde_json::to_string(&set_secret).map_err(|e| e.to_string())?;
    writeln!(stream, "{set_secret_json}").map_err(|e| e.to_string())?;

    let mut resp_line = String::new();
    reader
        .read_line(&mut resp_line)
        .map_err(|e| e.to_string())?;

    let resp: ServerMessage = serde_json::from_str(&resp_line)
        .map_err(|_| "malformed response from engine".to_string())?;

    match resp {
        ServerMessage::Response { ok: Some(true), .. } => {
            println!("saved");
            Ok(())
        }
        ServerMessage::Response {
            error: Some(err), ..
        } => Err(err.message),
        _ => Err("unexpected response from engine".to_string()),
    }
}

#[cfg(not(unix))]
fn run_secret_set(_id: &str, _socket_path: Option<&std::path::Path>) -> Result<(), String> {
    Err("unsupported on this platform".to_string())
}
