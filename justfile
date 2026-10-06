export CARGO_BUILD_WARNINGS := "deny"
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

deny:
    cargo deny check

secrets-selftest:
    bash scripts/gitleaks-selftest.sh

secrets: secrets-selftest
    gitleaks git --redact --no-banner .

portable:
    cargo clippy --workspace --all-targets --all-features --locked {{ if LINUX_ONLY_CRATES != "" { "--exclude " + LINUX_ONLY_CRATES } else { "" } }}

ci-linux: lint test deny

ci: ci-linux secrets

push: secrets
    git push -u origin HEAD

js:
    pnpm install --frozen-lockfile

