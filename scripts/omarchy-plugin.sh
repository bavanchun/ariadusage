#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLUGIN_ID="io.github.bavanchun.ariadusage"
QMLLINT="${QMLLINT:-/usr/lib/qt6/bin/qmllint}"
OMARCHY_PATH="${OMARCHY_PATH:-/usr/share/omarchy}"

cmd="${1:-}"
shift || true

usage() {
  cat <<EOF
Usage: $0 <command> [arguments...]

Commands:
  assemble <out_dir> [--dev-engine <path>] [--dev-socket <path>]
  check [dir]
  dev
  dev-enable
  dev-remove
EOF
  exit 1
}

cmd_assemble() {
  local out_dir="${1:-}"
  if [[ -z "$out_dir" ]]; then
    echo "ERROR: assemble requires an output directory" >&2
    exit 1
  fi
  shift || true

  local dev_engine=""
  local dev_socket=""

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --dev-engine)
        dev_engine="$2"
        shift 2
        ;;
      --dev-socket)
        dev_socket="$2"
        shift 2
        ;;
      *)
        echo "Unknown assemble option: $1" >&2
        exit 1
        ;;
    esac
  done

  mkdir -p "$out_dir"
  cp -a "$REPO_ROOT/integrations/omarchy/." "$out_dir/"

  if [[ -n "$dev_engine" || -n "$dev_socket" ]]; then
    local engine_val="${dev_engine:-/usr/bin/ariadusage}"
    local socket_val="${dev_socket:-\$XDG_RUNTIME_DIR/ariadusage/engine.sock}"

    cat > "$out_dir/components/EngineConfig.qml" <<QML
pragma ComponentBehavior: Bound
import QtQuick

Item {
  id: root

  readonly property string engineBinary: "${engine_val}"
  readonly property string socketPath: "${socket_val}"
}
QML
  fi
}

cmd_check() {
  local target_dir="${1:-}"
  if [[ -z "$target_dir" ]]; then
    target_dir="$REPO_ROOT/target/omarchy-check/$PLUGIN_ID"
    rm -rf "$REPO_ROOT/target/omarchy-check"
    mkdir -p "$REPO_ROOT/target/omarchy-check"
    cmd_assemble "$target_dir"
    if [[ -f "$REPO_ROOT/scripts/omarchy-qmllint.ini" ]]; then
      cp -a "$REPO_ROOT/scripts/omarchy-qmllint.ini" "$REPO_ROOT/target/omarchy-check/.qmllint.ini"
    fi
  fi

  if [[ ! -x "$QMLLINT" ]]; then
    echo "ERROR: qmllint executable not found at '$QMLLINT'" >&2
    exit 1
  fi

  if [[ ! -d "$OMARCHY_PATH" ]]; then
    echo "ERROR: OMARCHY_PATH directory not found at '$OMARCHY_PATH'" >&2
    exit 1
  fi

  local validator=""
  if [[ -x "$OMARCHY_PATH/bin/omarchy-plugin-validate" ]]; then
    validator="$OMARCHY_PATH/bin/omarchy-plugin-validate"
  elif command -v omarchy-plugin-validate >/dev/null 2>&1; then
    validator="$(command -v omarchy-plugin-validate)"
  else
    echo "ERROR: omarchy-plugin-validate tool not found" >&2
    exit 1
  fi

  echo "==> Running omarchy-plugin-validate on $target_dir"
  "$validator" "$target_dir"

  echo "==> Running strict qmllint (-W 0) on QML files in $target_dir"
  mapfile -t qml_files < <(find "$target_dir" -name "*.qml")
  if [[ ${#qml_files[@]} -eq 0 ]]; then
    echo "ERROR: No QML files found in '$target_dir'" >&2
    exit 1
  fi

  "$QMLLINT" -W 0 \
    -I "$OMARCHY_PATH/shell" \
    -i "$OMARCHY_PATH/shell/Commons/qmldir" \
    -i "$OMARCHY_PATH/shell/Ui/qmldir" \
    "${qml_files[@]}"

  echo "==> Running source-contract scan on $target_dir"
  if grep -rn "setSecret" "$target_dir"; then
    echo "ERROR: Forbidden pattern 'setSecret' detected in plugin files" >&2
    exit 1
  fi

  if grep -rn "/home/" "$target_dir"; then
    echo "ERROR: Forbidden hardcoded '/home/' path detected in plugin files" >&2
    exit 1
  fi

  if grep -rn "console\.log" "$target_dir"; then
    echo "ERROR: Forbidden 'console.log' call detected in plugin files" >&2
    exit 1
  fi

  if grep -rn "AutoText" "$target_dir"; then
    echo "ERROR: Forbidden 'AutoText' pattern detected in plugin files" >&2
    exit 1
  fi

  while IFS= read -r f; do
    local bname
    bname="$(basename "$f")"
    if [[ "$bname" =~ (install|installer|setup|uninstall) ]]; then
      # Exclude SettingsForm / SetupPanel if any, but enforce rule
      if [[ "$bname" =~ (SettingsForm\.qml|SetupPanel\.qml) ]]; then
        continue
      fi
      echo "ERROR: Forbidden marketplace installer filename pattern: $f" >&2
      exit 1
    fi
  done < <(find "$target_dir" -type f)

  echo "==> Plugin checks passed successfully!"
}

cmd_dev() {
  local dev_out="$REPO_ROOT/target/omarchy-dev/$PLUGIN_ID"
  local socket_dir="${XDG_RUNTIME_DIR:-/tmp}/ariadusage-dev"
  local socket_path="$socket_dir/engine.sock"
  local engine_path="$REPO_ROOT/target/debug/ariadusage"

  rm -rf "$REPO_ROOT/target/omarchy-dev"
  mkdir -p "$REPO_ROOT/target/omarchy-dev"
  cmd_assemble "$dev_out" --dev-engine "$engine_path" --dev-socket "$socket_path"

  local dest_plugins="${HOME}/.config/omarchy/plugins"
  mkdir -p "$dest_plugins/$PLUGIN_ID"
  rsync -a --delete "$dev_out/" "$dest_plugins/$PLUGIN_ID/"
  echo "==> Synced dev plugin into $dest_plugins/$PLUGIN_ID"
}

cmd_dev_enable() {
  local reports_dir="${REPORTS_DIR:-}"
  local shell_json="${HOME}/.config/omarchy/shell.json"

  if [[ -f "$shell_json" && -n "$reports_dir" ]]; then
    mkdir -p "$reports_dir"
    local snap="$reports_dir/shell.json.before-ariadusage-dev"
    if [[ ! -f "$snap" ]]; then
      cp -a "$shell_json" "$snap"
      echo "==> Snapshot saved to $snap"
    fi
  fi

  echo "==> Enabling plugin $PLUGIN_ID"
  omarchy plugin enable "$PLUGIN_ID"
}

cmd_dev_remove() {
  local reports_dir="${REPORTS_DIR:-}"
  echo "==> Removing plugin $PLUGIN_ID"
  omarchy plugin remove "$PLUGIN_ID" --yes || true

  rm -rf "${HOME}/.config/omarchy/plugins/.$PLUGIN_ID.bak."*

  if [[ -n "$reports_dir" ]]; then
    local snap="$reports_dir/shell.json.before-ariadusage-dev"
    if [[ -f "$snap" ]]; then
      cp -a "$snap" "${HOME}/.config/omarchy/shell.json"
      echo "==> Restored shell.json from $snap"
    fi
  fi

  if command -v omarchy-shell >/dev/null 2>&1; then
    omarchy-shell shell reloadConfig || true
  fi
}

case "$cmd" in
  assemble)
    cmd_assemble "$@"
    ;;
  check)
    cmd_check "$@"
    ;;
  dev)
    cmd_dev "$@"
    ;;
  dev-enable)
    cmd_dev_enable "$@"
    ;;
  dev-remove)
    cmd_dev_remove "$@"
    ;;
  *)
    usage
    ;;
esac
