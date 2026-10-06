#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
CONFIG_PATH="${REPO_ROOT}/.gitleaks.toml"

if [[ ! -f "$CONFIG_PATH" ]]; then
    if [[ -f "${PWD}/.gitleaks.toml" ]]; then
        CONFIG_PATH="${PWD}/.gitleaks.toml"
    else
        echo "Error: .gitleaks.toml not found at ${CONFIG_PATH}" >&2
        exit 1
    fi
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

TARGET_DIR="${TMP_DIR}/target"
mkdir -p "${TARGET_DIR}"

# Build synthetic values at runtime by concatenating fragments.
# No fragment must match any rule on its own.

# Helper fragments for entropy without tripping generic-api-key
e1="K7x9m"
e2="Q2vL5"
e3="wP8nR"
e4="1tY4u"
e5="I0oE3"
e6="aZ6sD"
ent="${e1}${e2}${e3}${e4}${e5}${e6}"

# 1. Claude OAuth token (sk-ant-(oat|ort|sid)...)
c_prefix="sk-ant-"
c_kind="oat01-"
claude_token="${c_prefix}${c_kind}${ent}"

# 2. JWT token in JSON (refresh_token / id_token / access_token)
j_k="refresh"
j_t="token"
j_key="{\"${j_k}_${j_t}\": \""
j_hdr='eyJhbGciOiJIUzI1NiJ9.'
j_payload='eyJzdWIiOiIxMjM0NTY3ODkwIn0.'
j_sig="${ent}\"}"
jwt_token="${j_key}${j_hdr}${j_payload}${j_sig}"

# 3. Google OAuth token (ya29. or 1//0)
g_prefix="ya"
g_dot="29."
google_token="${g_prefix}${g_dot}a0AfH6SMCx${ent}${ent}"

# 4. Cookie header with value
ck_h="Coo"
ck_sep="kie: "
ck_k="session"
ck_t="id="
cookie_token="${ck_h}${ck_sep}${ck_k}_${ck_t}${ent}"

# 5. Default gitleaks rule (anthropic-api-key)
d_prefix="sk-ant-"
d_kind="api03-"
d_body="$(printf 'a%.0s' {1..93})"
d_suffix="AA"
default_token="${d_prefix}${d_kind}${d_body}${d_suffix}"

cat << SECRETS > "${TARGET_DIR}/synthetic_secrets.txt"
${claude_token}
${jwt_token}
${google_token}
${cookie_token}
${default_token}
SECRETS

REPORT_FILE="${TMP_DIR}/report.json"

gitleaks dir --no-banner --redact --config "${CONFIG_PATH}" --report-format json --report-path "${REPORT_FILE}" "${TARGET_DIR}" > /dev/null 2>&1 || true

if [[ ! -s "${REPORT_FILE}" ]]; then
    echo "Error: gitleaks report was not generated or is empty" >&2
    exit 1
fi

FIRED_RULES="$(jq -r '.[].RuleID' "${REPORT_FILE}" | sort -u)"

REQUIRED_CUSTOM_RULES=(
    "claude-oauth-token"
    "jwt-token-in-json"
    "google-oauth-token"
    "cookie-session-token"
)

missing=0
for rule in "${REQUIRED_CUSTOM_RULES[@]}"; do
    if ! echo "${FIRED_RULES}" | grep -Fxq "${rule}"; then
        echo "Error: required custom rule '${rule}' did not fire" >&2
        missing=1
    fi
done

has_default=0
for rule in ${FIRED_RULES}; do
    is_custom=0
    for custom in "${REQUIRED_CUSTOM_RULES[@]}"; do
        if [[ "${rule}" == "${custom}" ]]; then
            is_custom=1
            break
        fi
    done
    if [[ ${is_custom} -eq 0 ]]; then
        has_default=1
        break
    fi
done

if [[ ${has_default} -eq 0 ]]; then
    echo "Error: no default gitleaks rule fired" >&2
    missing=1
fi

if [[ ${missing} -ne 0 ]]; then
    echo "Self-test failed. Fired rules were:" >&2
    echo "${FIRED_RULES}" >&2
    exit 1
fi

echo "gitleaks selftest passed: all custom rules and at least one default rule fired successfully"
