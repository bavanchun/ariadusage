#!/usr/bin/env bash
# Runs the macOS app against the protocol fixture server on a private socket.
# The fixture server is stopped when the app exits.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macos-dev runs only on macOS" >&2
  exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# TMPDIR is a per-user private directory on macOS, unlike /tmp.
TMP_ROOT="${TMPDIR:-$(getconf DARWIN_USER_TEMP_DIR)}"
SOCKET_DIR="${TMP_ROOT%/}/ariadusage-dev"
SOCKET="$SOCKET_DIR/engine.sock"
STEP_SECONDS="${ARIADUSAGE_FIXTURE_STEP_SECONDS:-5}"

cd "$REPO_ROOT"
cargo build --quiet -p ariadusage-protocol --example fixture_server --features fixture
cargo build --quiet -p ariadusage-macos

mkdir -p "$SOCKET_DIR"
chmod 700 "$SOCKET_DIR"
rm -f "$SOCKET"

target/debug/examples/fixture_server --socket "$SOCKET" --step-seconds "$STEP_SECONDS" &
FIXTURE_PID=$!
trap 'kill "$FIXTURE_PID" 2>/dev/null || true; wait "$FIXTURE_PID" 2>/dev/null || true' EXIT

for _ in $(seq 1 50); do
  [[ -S "$SOCKET" ]] && break
  sleep 0.1
done
if [[ ! -S "$SOCKET" ]]; then
  echo "fixture server did not create $SOCKET" >&2
  exit 1
fi

target/debug/ariadusage-macos --socket "$SOCKET"
