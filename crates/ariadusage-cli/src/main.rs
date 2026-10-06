use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "ariadusage",
    version = env!("CARGO_PKG_VERSION"),
    about = "Track usage and quotas for AI coding assistants"
)]
struct Cli {}

fn main() {
    let _ = Cli::parse();
}
