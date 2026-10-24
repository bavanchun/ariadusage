//! Entry point of the macOS menu-bar app.

#[cfg(target_os = "macos")]
fn main() {
    let mut args = std::env::args().skip(1);
    let mut socket = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => socket = args.next().map(std::path::PathBuf::from),
            "--help" | "-h" => {
                println!("Usage: ariadusage-macos [--socket <path>]");
                return;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    let socket = match socket {
        Some(path) => path,
        None => match ariadusage_macos::engine_client::default_socket_path() {
            Ok(path) => path,
            Err(err) => {
                eprintln!("{err}");
                std::process::exit(2);
            }
        },
    };
    if let Err(err) = ariadusage_macos::appkit::run(socket) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("ariadusage-macos runs only on macOS");
    std::process::exit(1);
}
