//! Command-line example running the in-process fixture server for UI and integration testing.

use std::path::PathBuf;

use ariadusage_protocol::fixture::{FixtureConfig, MisbehaveMode, start_fixture_server};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut config = FixtureConfig::default();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => {
                if let Some(path) = args.next() {
                    config.socket_path = PathBuf::from(path);
                }
            }
            "--step-seconds" => {
                if let Some(sec) = args.next() {
                    config.step_seconds = sec.parse().unwrap_or(5);
                }
            }
            "--misbehave" => {
                if args.next().as_deref() == Some("oversize-frame") {
                    config.misbehave = Some(MisbehaveMode::OversizeFrame);
                }
            }
            "--help" | "-h" => {
                println!(
                    "Usage: fixture_server --socket <path> [--step-seconds <n>] [--misbehave oversize-frame]"
                );
                return Ok(());
            }
            _ => {}
        }
    }

    let mut server = start_fixture_server(config).await?;
    println!(
        "Fixture server listening on {}",
        server.socket_path.display()
    );

    tokio::signal::ctrl_c().await?;
    println!("Shutting down fixture server...");
    server.stop().await;
    Ok(())
}
