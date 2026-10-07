export CARGO_BUILD_WARNINGS := "deny"
export INSTA_UPDATE := "no"
export PATH := env_var_or_default("CARGO_HOME", env_var_or_default("HOME", "") + "/.cargo") + "/bin:" + env_var("PATH")

LINUX_ONLY_CRATES := ""

default: ci

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

lint: fmt-check clippy typos

clippy:
    cargo clippy --workspace --all-targets --all-features --locked

typos:
    typos

test:
    cargo nextest run --workspace --all-features --locked
    cargo test --doc --workspace --all-features --locked
    ! pgrep -u "$(id -u)" -f '[a]riadusage-test-child'

keyring-test:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v dbus-run-session >/dev/null
    command -v gnome-keyring-daemon >/dev/null
    unset DBUS_SESSION_BUS_ADDRESS GNOME_KEYRING_CONTROL GNOME_KEYRING_PID
    dbus-run-session -- bash -euo pipefail -c '
      test_root="$(mktemp -d)"
      daemon_pid=""
      cleanup() {
        if [[ -n "$daemon_pid" ]]; then
          kill -TERM "$daemon_pid" 2>/dev/null || true
          wait "$daemon_pid" 2>/dev/null || true
        fi
        rm -rf -- "$test_root"
      }
      trap cleanup EXIT
      trap "exit 130" INT
      trap "exit 143" TERM

      export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
      export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
      export HOME="$test_root/home"
      export XDG_CONFIG_HOME="$HOME/.config"
      export XDG_DATA_HOME="$HOME/.local/share"
      export XDG_STATE_HOME="$HOME/.local/state"
      export XDG_RUNTIME_DIR="$HOME/.runtime"
      export ARIADUSAGE_KEYRING_TEST=isolated-keyring-v1
      unset ARIADUSAGE_DISABLE_KEYRING
      mkdir -m 700 -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_RUNTIME_DIR"
      printf "%s\n" isolated-keyring-v1 > "$HOME/.ariadusage-keyring-test-sentinel"

      keyring_password() {
        local password
        password="$(od -An -N32 -tx1 /dev/urandom | tr -d " \n")"
        printf "%s\n" "$password"
        unset password
      }
      gnome-keyring-daemon --foreground --unlock --components=secrets \
        < <(keyring_password) >/dev/null 2>&1 &
      daemon_pid=$!

      cargo nextest run -p ariadusage-engine --locked -E "binary(secret_keyring)" --run-ignored only
    '

release-check:
    cargo build -p ariadusage-cli --release --locked
    ! test -e target/release/ariadusage-test-child
    ! cargo tree -p ariadusage-cli -e features --locked | grep -q 'test-hooks'

schemas:
    ARIADUSAGE_BLESS_SCHEMAS=1 cargo test -p ariadusage-protocol --test schema_drift --locked

deny:
    cargo deny check

secrets-selftest:
    bash scripts/gitleaks-selftest.sh

secrets: secrets-selftest
    gitleaks git --redact --no-banner .

portable:
    cargo clippy --workspace --all-targets --all-features --locked {{ if LINUX_ONLY_CRATES != "" { "--exclude " + LINUX_ONLY_CRATES } else { "" } }}

ci-linux: lint test deny release-check

ci: ci-linux brand-ci omarchy-check secrets

push: secrets
    git push -u origin HEAD

js:
    pnpm install --frozen-lockfile

brand:
    pnpm --dir brand build

brand-check:
    node brand/scripts/check-contrast.mjs
    node brand/scripts/simulate-cvd.mjs

brand-ci: js
    pnpm audit --audit-level high
    just brand
    just brand-check
    git diff --exit-code brand/svg

omarchy-check:
    bash scripts/omarchy-plugin.sh check

omarchy-dev:
    bash scripts/omarchy-plugin.sh dev

omarchy-dev-enable:
    bash scripts/omarchy-plugin.sh dev-enable

omarchy-dev-remove:
    bash scripts/omarchy-plugin.sh dev-remove
