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

