// Ported from CodexBar Tests/CodexBarTests/CodexUsageFetcherFallbackTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#![allow(dead_code)] // Each test binary uses a different subset of the upstream RPC fixture builders.

use ariadusage_engine::brokers::process::AbsolutePath;
use ariadusage_engine::providers::codex::*;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, time::Duration};
use tempfile::TempDir;
pub fn script(home: &std::path::Path, raw: &str) -> std::path::PathBuf {
    let path = home.join("fixture-codex");
    fs::write(&path, raw).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}
pub fn shell_json(v: &Value) -> String {
    serde_json::to_string(v).unwrap()[1..].replace('\'', "'\"'\"'")
}

pub const RPC_SCRIPT: &str = r####"#!/bin/sh
if [ "$#" != 5 ] || [ "$1 $2 $3 $4 $5" != '-s read-only -a never app-server' ]; then
  printf '%s\n' 'unexpected Codex arguments' >&2
  exit 64
fi
while IFS= read -r line; do
  identifier=${line#*'"id"'}
  identifier=${identifier#*:}
  identifier=${identifier#"${identifier%%[![:space:]]*}"}
  identifier=${identifier%%[!0-9]*}
  case "$line" in
    *'"initialized"'*) ;;
    *'"initialize"'*) printf '{"id":%s,"result":{}}\n' "$identifier" ;;
    *'"account/rateLimits/read"'*|*'"account\/rateLimits\/read"'*)
      printf '{"id":%s,%s\n' "$identifier" '__LIMITS__' ;;
    *'"account/read"'*|*'"account\/read"'*)
      printf '{"id":%s,%s\n' "$identifier" '__ACCOUNT__' ;;
    *) printf '%s\n' 'unexpected RPC method' >&2; exit 65 ;;
  esac
done
"####;
pub const HUNG_SCRIPT: &str = r####"#!/bin/sh
case " $* " in
  *" app-server "*) ;;
  *) printf '%s\n' "unexpected non app-server Codex invocation" >&2; exit 92 ;;
esac

while IFS= read -r line; do
  case "$line" in
    *'"method":"initialized"'*|*'"method": "initialized"'*)
      ;;
    *'"method":"initialize"'*|*'"method": "initialize"'*)
      printf '%s\n' '{"id":1,"result":{}}'
      ;;
    *'"account/rateLimits/read"'*|*'"account\/rateLimits\/read"'*)
      printf '%s\n' 'account/rateLimits/read' >> "$CODEXBAR_TEST_RPC_REQUEST_PATH"
      # Keep the request unanswered until the RPC client closes stdin.
      while IFS= read -r ignored; do :; done
      exit 0
      ;;
    *)
      printf '%s\n' '{"id":1,"result":{}}'
      ;;
  esac
done
"####;
pub const DECODE_MISMATCH_BODY_MESSAGE: &str = r####"failed to fetch codex rate limits: Decode error for https://chatgpt.com/backend-api/wham/usage:
unknown variant `prolite`, expected one of `guest`, `free`, `go`, `plus`, `pro`;
content-type=application/json; body={
  "user_id": "user-TEST",
  "account_id": "account-TEST",
  "email": "prolite-test@example.com",
  "plan_type": "prolite",
  "rate_limit": {
    "allowed": true,
    "limit_reached": false,
    "primary_window": {
      "used_percent": 4,
      "limit_window_seconds": 18000,
      "reset_after_seconds": 8657,
      "reset_at": 1776216359
    },
    "secondary_window": {
      "used_percent": 19,
      "limit_window_seconds": 604800,
      "reset_after_seconds": 187681,
      "reset_at": 1776395384
    }
  },
  "credits": {
    "has_credits": false,
    "unlimited": false,
    "overage_limit_reached": false,
    "balance": "0E-10"
  }
}"####;
pub const PARTIAL_DECODE_BODY_MESSAGE: &str = r####"failed to fetch codex rate limits: Decode error for https://chatgpt.com/backend-api/wham/usage:
unknown variant `prolite`, expected one of `guest`, `free`, `go`, `plus`, `pro`;
content-type=application/json; body={
  "email": "prolite-test@example.com",
  "plan_type": "prolite",
  "rate_limit": {
    "allowed": true,
    "limit_reached": false,
    "primary_window": {
      "used_percent": "oops",
      "limit_window_seconds": 18000,
      "reset_at": 1776216359
    },
    "secondary_window": {
      "used_percent": 19,
      "limit_window_seconds": 604800,
      "reset_after_seconds": 187681,
      "reset_at": 1776395384
    }
  }
}"####;
pub const CREDITS_ONLY_DECODE_MISMATCH_BODY_MESSAGE: &str = r####"failed to fetch codex rate limits: Decode error for https://chatgpt.com/backend-api/wham/usage:
unknown variant `prolite`, expected one of `guest`, `free`, `go`, `plus`, `pro`;
content-type=application/json; body={
  "email": "prolite-test@example.com",
  "plan_type": "prolite",
  "rate_limit": {
    "allowed": true,
    "limit_reached": false,
    "primary_window": {
      "used_percent": "oops",
      "limit_window_seconds": 18000,
      "reset_at": 1776216359
    }
  },
  "credits": {
    "has_credits": true,
    "unlimited": false,
    "overage_limit_reached": false,
    "balance": "14.5"
  }
}"####;
pub fn fixture(response: Value, plan: &str) -> (TempDir, CodexCLIUsageFetcher) {
    let h = TempDir::new().unwrap();
    let account = json!({"result":{"account":{"type":"chatgpt","email":"stub@example.com","planType":plan},"requiresOpenaiAuth":false}});
    let raw = RPC_SCRIPT
        .replace("__LIMITS__", &shell_json(&response))
        .replace("__ACCOUNT__", &shell_json(&account));
    let path = script(h.path(), &raw);
    let fetcher = CodexCLIUsageFetcher::new(
        BTreeMap::from([
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("CODEX_HOME".into(), h.path().to_string_lossy().into_owned()),
        ]),
        Some(h.path().into()),
        h.path().into(),
    )
    .with_executable(AbsolutePath::new(path).unwrap())
    .with_timeouts(Duration::from_secs(20), Duration::from_secs(3));
    (h, fetcher)
}
pub fn mismatch(message: &str) -> (TempDir, CodexCLIUsageFetcher) {
    fixture(json!({"error":{"message":message}}), "prolite")
}
pub fn plan_only(
    plan: Option<&str>,
    account_plan: &str,
    windows: bool,
    credits: bool,
) -> (TempDir, CodexCLIUsageFetcher) {
    let mut limits = json!({"planType":plan});
    if windows {
        limits["primary"] = json!({"usedPercent":12,"windowDurationMins":300});
    }
    if credits {
        limits["credits"] = json!({"hasCredits":true,"unlimited":false,"balance":"21"});
    }
    fixture(json!({"result":{"rateLimits":limits}}), account_plan)
}
pub fn hung() -> (TempDir, CodexCLIUsageFetcher) {
    let (h, mut f) = fixture(json!({}), "pro");
    let request = h.path().join("requests");
    let raw = HUNG_SCRIPT.replace("$CODEXBAR_TEST_RPC_REQUEST_PATH", request.to_str().unwrap());
    script(h.path(), &raw);
    f.request_timeout = Duration::from_millis(10);
    (h, f)
}
