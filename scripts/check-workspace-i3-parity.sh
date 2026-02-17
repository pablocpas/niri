#!/usr/bin/env bash
set -euo pipefail

BIN="${TIRI_BIN:-}"
if [[ -z "${BIN}" ]]; then
    if [[ -x "./target/release/tiri" ]]; then
        BIN="./target/release/tiri"
    else
        BIN="tiri"
    fi
fi

if ! command -v jq >/dev/null 2>&1; then
    echo "FAIL: jq is required"
    exit 1
fi

pass() { echo "PASS: $*"; }
fail() { echo "FAIL: $*"; exit 1; }
warn() { echo "WARN: $*"; }

ws_json() {
    "${BIN}" msg --json workspaces
}

ws_id_by_name() {
    local name="$1"
    ws_json | jq -r --arg n "${name}" 'map(select(.name == $n))[0].id // empty'
}

ws_exists_by_name() {
    local name="$1"
    ws_json | jq -e --arg n "${name}" '.[] | select(.name == $n)' >/dev/null
}

focused_name() {
    ws_json | jq -r 'map(select(.is_focused))[0].name // empty'
}

ws_idx_by_name() {
    local name="$1"
    ws_json | jq -r --arg n "${name}" 'map(select(.name == $n))[0].idx // empty'
}

wait_for() {
    local check_cmd="$1"
    local retries="${2:-40}"
    local sleep_s="${3:-0.05}"
    local i=0
    while (( i < retries )); do
        if eval "${check_cmd}" >/dev/null 2>&1; then
            return 0
        fi
        sleep "${sleep_s}"
        i=$((i + 1))
    done
    return 1
}

pick_unused_numeric_name() {
    local n="$1"
    while (( n < 1000 )); do
        if ! ws_json | jq -e --arg n "${n}" '.[] | select(.name == $n)' >/dev/null; then
            echo "${n}"
            return 0
        fi
        n=$((n + 1))
    done
    return 1
}

echo "Using binary: ${BIN}"

ws_json | jq -e 'type == "array"' >/dev/null || fail "workspaces JSON is not an array"
pass "IPC format is array (compatible with your current client output)"

A="$(pick_unused_numeric_name 91)" || fail "could not find unused numeric workspace name"
B="$(pick_unused_numeric_name $((A + 1)))" || fail "could not find a second unused numeric workspace name"

echo "Test workspace names: A=${A}, B=${B}"

"${BIN}" msg action focus-workspace "${A}" >/dev/null
wait_for "ws_id_by_name ${A} | grep -Eq '^[0-9]+'" || fail "focus-workspace ${A} did not create workspace '${A}'"
wait_for "focused_name | grep -Fxq '${A}'" || fail "workspace '${A}' did not become focused"
id_a="$(ws_id_by_name "${A}")"
pass "focus-workspace ${A} creates/focuses workspace '${A}' (id=${id_a})"

"${BIN}" msg action focus-workspace 2 >/dev/null
wait_for "focused_name | grep -Fxq '2'" || fail "focus-workspace 2 did not focus workspace '2'"
id2_first="$(ws_id_by_name "2")"
[[ -n "${id2_first}" ]] || fail "workspace '2' was not found"
idx2_original="$(ws_idx_by_name "2")"

"${BIN}" msg action focus-workspace "${A}" >/dev/null
"${BIN}" msg action focus-workspace 2 >/dev/null
wait_for "focused_name | grep -Fxq '2'" || fail "workspace '2' did not refocus after switching"
id2_second="$(ws_id_by_name "2")"
[[ "${id2_first}" == "${id2_second}" ]] || fail "workspace '2' changed identity (${id2_first} -> ${id2_second})"
pass "numeric workspace keeps stable identity (workspace '2' id=${id2_first})"

# Reorder around and ensure numeric reference still resolves by name, not by position.
# Do this on temporary workspace A so regular workspaces keep their ordering.
"${BIN}" msg action focus-workspace "${A}" >/dev/null
"${BIN}" msg action move-workspace-down >/dev/null || true
"${BIN}" msg action focus-workspace 2 >/dev/null
wait_for "focused_name | grep -Fxq '2'" || fail "workspace '2' did not focus after workspace reorder"
id2_third="$(ws_id_by_name "2")"
[[ "${id2_first}" == "${id2_third}" ]] || fail "workspace '2' resolved by position after reorder (${id2_first} -> ${id2_third})"
pass "numeric reference remains name-based after workspace reorder"

# Restore workspace "2" to its original visible position to avoid side effects.
idx2_current="$(ws_idx_by_name "2")"
if [[ -n "${idx2_original}" && -n "${idx2_current}" && "${idx2_original}" != "${idx2_current}" ]]; then
    "${BIN}" msg action focus-workspace 2 >/dev/null
    "${BIN}" msg action move-workspace-to-index "${idx2_original}" >/dev/null || true
    pass "workspace '2' position restored to idx=${idx2_original}"
fi

wait_for "! ws_exists_by_name ${A}" 80 0.05 \
    || fail "auto-created numeric workspace '${A}' did not disappear after leaving it empty"
pass "auto-created numeric workspace '${A}' disappears when empty and unfocused"

"${BIN}" msg action focus-workspace "${B}" >/dev/null
wait_for "ws_id_by_name ${B} | grep -Eq '^[0-9]+'" || fail "focus-workspace ${B} did not create workspace '${B}'"
pass "lazy creation also works for a second numeric workspace '${B}'"

ws_json | jq -e 'all(.[]; (.idx | type == "number") and (.idx >= 1))' >/dev/null \
    || fail "some workspace has invalid idx"
pass "all published idx values are >= 1"

ws_json | jq -e '
  [group_by(.output)[] | map(select(.is_active)) | length]
  | all(. == 1)
' >/dev/null || fail "expected exactly one active workspace per output in IPC"
pass "exactly one active workspace per output"

dups="$(
    ws_json | jq -r '
      map(select(.name != null) | .name)
      | group_by(.)[]
      | select(length > 1)
      | .[0]
    '
)"
if [[ -n "${dups}" ]]; then
    fail "duplicate workspace names detected: ${dups//$'\n'/, }"
fi
pass "workspace names are unique"

hidden_empty_count="$(
    ws_json | jq '
      [.[] | select((.name == null) and (.active_window_id == null) and (.is_focused | not))]
      | length
    '
)"
if [[ "${hidden_empty_count}" != "0" ]]; then
    warn "found ${hidden_empty_count} anonymous empty non-focused workspaces in IPC"
else
    pass "no anonymous empty non-focused workspaces exposed in IPC"
fi

echo
echo "Done. i3/sway parity checks passed for:"
echo "- numeric workspace reference semantics"
echo "- lazy numeric creation"
echo "- stable identity for named numeric workspaces"
echo "- IPC consistency expected by waybar-niri"
