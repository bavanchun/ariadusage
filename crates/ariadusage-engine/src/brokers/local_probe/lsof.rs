// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityStatusProbe+PortDetection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
//! Linux lsof execution stays inside the Process and executable trust boundaries.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_protocol::secret::SecretString;

use super::error::{ListenerCommandError, ProbeError};
use crate::brokers::call::BrokerCall;
use crate::brokers::exec_resolver::{RealResolverFs, resolve_absolute_candidates};
use crate::brokers::process::{
    self, Command, DEFAULT_OUTPUT_CAP, ProcessEnv, ProcessError, ProcessRegistry, StreamPolicy,
};

const LABEL: &str = "antigravity-lsof";

#[derive(Clone)]
pub(super) struct ListenerCommand {
    candidates: Vec<PathBuf>,
    registry: Arc<ProcessRegistry>,
    gate: Arc<LaunchGate>,
}

impl std::fmt::Debug for ListenerCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListenerCommand")
            .field("candidate_count", &self.candidates.len())
            .finish_non_exhaustive()
    }
}

impl Default for ListenerCommand {
    fn default() -> Self {
        Self::with_candidates(vec!["/usr/sbin/lsof".into(), "/usr/bin/lsof".into()])
    }
}

impl ListenerCommand {
    pub(super) fn with_candidates(candidates: Vec<PathBuf>) -> Self {
        Self {
            candidates,
            registry: Arc::new(ProcessRegistry::new()),
            gate: Arc::new(LaunchGate::default()),
        }
    }

    /// An empty successful result means procfs should be consulted, without a diagnostic.
    pub(super) async fn ports(
        &self,
        call: &BrokerCall,
        pid: i32,
        timeout: Duration,
    ) -> Result<Vec<u16>, ProbeError> {
        if self.candidates.is_empty() {
            return Ok(Vec::new());
        }
        let program =
            resolve_absolute_candidates(&self.candidates, &RealResolverFs).map_err(|_| {
                ProbeError::ListenerDiscoveryPending(ListenerCommandError::ExecutableUnavailable)
            })?;
        let environment = ProcessEnv::from_allowlist(
            ["PATH", "LANG", "LC_ALL", "LC_CTYPE"]
                .into_iter()
                .filter_map(|key| std::env::var_os(key).map(|value| (key, value))),
        );
        let pid = pid.to_string();
        let mut command = Command::new(
            program,
            ["-nP", "-iTCP", "-sTCP:LISTEN", "-a", "-p", pid.as_str()]
                .into_iter()
                .map(Into::into)
                .collect(),
        )
        .env(environment)
        .timeout(timeout);
        command.set_stdout(StreamPolicy::Capture {
            cap: DEFAULT_OUTPUT_CAP,
        });
        command.set_stderr(StreamPolicy::Capture {
            cap: DEFAULT_OUTPUT_CAP,
        });
        let output = process::run(call.clone(), command, &self.registry, &self.gate).await;
        if call.cancel.is_cancelled() {
            return Err(ProbeError::Cancelled);
        }
        let output = output.map_err(|source| match source {
            ProcessError::Cancelled => ProbeError::Cancelled,
            ProcessError::LaunchFailed | ProcessError::LaunchSuppressed { .. } => {
                ProbeError::ListenerDiscoveryPending(ListenerCommandError::Process {
                    label: LABEL,
                    source,
                })
            }
            _ => ProbeError::ListenerCommand(ListenerCommandError::Process {
                label: LABEL,
                source,
            }),
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let code = output.status.code().unwrap_or(-1);
            if code == 1 && stderr.trim().is_empty() {
                return Ok(Vec::new());
            }
            return Err(ProbeError::ListenerDiscoveryPending(
                ListenerCommandError::NonZeroExit {
                    code,
                    stderr: SecretString::new(stderr.into_owned()),
                },
            ));
        }
        Ok(parse_listening_ports(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }
}

fn parse_listening_ports(output: &str) -> Vec<u16> {
    // Equivalent to upstream's :(\d+)\s+\(LISTEN\), without a new regex dependency.
    output
        .match_indices(':')
        .filter_map(|(index, _)| {
            let tail = &output[index + 1..];
            let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
            let after = &tail[digits..];
            if digits == 0
                || !after.starts_with(char::is_whitespace)
                || !after.trim_start().starts_with("(LISTEN)")
            {
                return None;
            }
            tail[..digits].parse::<u16>().ok()
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
