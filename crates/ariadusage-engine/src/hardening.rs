use std::fmt;

/// Errors returned while applying process hardening.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HardeningError {
    Dumpable,
    CoreLimit,
}

impl fmt::Display for HardeningError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Dumpable => "could not disable process dumpability",
            Self::CoreLimit => "could not disable core dumps",
        })
    }
}

impl fmt::Debug for HardeningError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Dumpable => "Dumpable",
            Self::CoreLimit => "CoreLimit",
        })
    }
}

impl std::error::Error for HardeningError {}

/// Prevents core dumps and ptrace access for this process on Linux.
pub fn harden_process() -> Result<(), HardeningError> {
    #[cfg(target_os = "linux")]
    {
        rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)
            .map_err(|_| HardeningError::Dumpable)?;
        rustix::process::setrlimit(
            rustix::process::Resource::Core,
            rustix::process::Rlimit {
                current: Some(0),
                maximum: Some(0),
            },
        )
        .map_err(|_| HardeningError::CoreLimit)?;
    }

    Ok(())
}
