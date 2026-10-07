mod buffers;
mod command;
mod env;
mod error;
mod procscan;
mod run;
mod signal;

pub use buffers::{BoundedLineBuffer, BoundedOutputBuffer, LineDrain};
pub use command::{AbsolutePath, Command, LaunchMode, Output, StdinSpec, StreamPolicy};
pub use env::ProcessEnv;
pub use error::{OutputStream, ProcessError};
pub use procscan::{
    ProcessIdentity, fd_targets, parse_marker_environment, process_group, process_identity,
    process_uid, read_marker_environment, same_uid_processes,
};
#[cfg(feature = "test-hooks")]
pub use run::retry_spawn_for_test;
pub use run::run;
pub use signal::{ProcessSignal, signal, signal_group};

pub const DEFAULT_OUTPUT_CAP: usize = 1024 * 1024;
