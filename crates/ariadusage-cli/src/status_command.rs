//! AriadUsage-specific `ariadusage status` IPC v1 snapshot inspection command.

use std::path::PathBuf;

use ariadusage_protocol::{EngineSnapshot, ProviderStatusPayload};

use crate::ipc_client::fetch_daemon_snapshot;
use crate::provider_meta::{CliProvider, ProviderSelection};

pub fn execute_status_from_snapshot(
    snapshot: &EngineSnapshot,
    provider_arg: Option<&str>,
    json: bool,
    pretty: bool,
) -> Result<String, (u8, &'static str)> {
    let mut filtered = snapshot.clone();
    if let Some(raw_provider) = provider_arg {
        let enabled: Vec<CliProvider> = snapshot
            .providers
            .iter()
            .filter(|p| p.enabled)
            .filter_map(|p| CliProvider::from_core_provider_id(&p.id))
            .collect();
        let Some(selection) = ProviderSelection::from_raw(Some(raw_provider), &enabled) else {
            return Err((1, "invalid --provider value"));
        };
        let selected: Vec<&str> = selection
            .as_list(CliProvider::V1_PROVIDERS)
            .into_iter()
            .map(CliProvider::raw_value)
            .collect();
        filtered
            .providers
            .retain(|p| selected.contains(&p.id.as_str()));
    }

    if json {
        let text = if pretty {
            serde_json::to_string_pretty(&filtered)
        } else {
            serde_json::to_string(&filtered)
        }
        .map_err(|_| (16_u8, "snapshot format failed"))?;
        return Ok(text);
    }

    let mut lines = vec![format!("engine {}", filtered.engine.version)];
    for p in &filtered.providers {
        let cli_prov = CliProvider::from_core_provider_id(&p.id);
        let status_payload = p
            .status
            .as_ref()
            .and_then(|m| m.value.as_ref())
            .map(ProviderStatusPayload::from);
        let fallback_url =
            cli_prov.and_then(|cp| cp.status_page_url().or_else(|| cp.status_link_url()));
        if let Some(st) = status_payload {
            let url = if st.url.is_empty() {
                fallback_url.unwrap_or("")
            } else {
                st.url.as_str()
            };
            if url.is_empty() {
                lines.push(format!(
                    "{}: enabled={} source={} status={}{}",
                    p.id.as_str(),
                    p.enabled,
                    p.source_mode,
                    st.indicator_label(),
                    st.description_suffix()
                ));
            } else {
                lines.push(format!(
                    "{}: enabled={} source={} status={}{} ({url})",
                    p.id.as_str(),
                    p.enabled,
                    p.source_mode,
                    st.indicator_label(),
                    st.description_suffix()
                ));
            }
        } else if let Some(url) = fallback_url {
            lines.push(format!(
                "{}: enabled={} source={} url={url}",
                p.id.as_str(),
                p.enabled,
                p.source_mode
            ));
        } else {
            lines.push(format!(
                "{}: enabled={} source={}",
                p.id.as_str(),
                p.enabled,
                p.source_mode
            ));
        }
    }
    Ok(lines.join("\n"))
}

pub fn run_status_live(
    socket: Option<PathBuf>,
    provider: Option<&str>,
    format: Option<&str>,
    json_flag: bool,
    json_only_flag: bool,
    pretty_flag: bool,
) -> Result<(), (u8, &'static str)> {
    let format_json = match format.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("json") => true,
        Some("text") | None => false,
        Some(_) => return Err((1, "invalid --format value (expected text|json)")),
    };
    let is_json = json_flag || json_only_flag || format_json;
    let is_pretty = pretty_flag || (json_flag && format.is_none());
    let snapshot = fetch_daemon_snapshot(socket)?;
    let out = execute_status_from_snapshot(&snapshot, provider, is_json, is_pretty)?;
    println!("{out}");
    Ok(())
}
