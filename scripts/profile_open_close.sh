#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCENARIO="$ROOT/scripts/perf_scenarios/open_close.json"
WINDOW_CMD="${TIRI_PROFILE_WINDOW_CMD:-foot --app-id perf-test}"
REPEAT="${TIRI_PROFILE_REPEAT:-9}"
OUTPUT_DIR="${TIRI_PROFILE_OUTPUT_DIR:-/tmp/tiri-open-close-$(date +%Y%m%d-%H%M%S)}"
SETTLE_TIMEOUT="${TIRI_PROFILE_SETTLE_TIMEOUT:-2.0}"
SETTLE_INTERVAL="${TIRI_PROFILE_SETTLE_INTERVAL:-0.02}"
IDLE_GRACE="${TIRI_PROFILE_IDLE_GRACE:-0.10}"

adopt_env() {
  if [[ -n "${TIRI_SOCKET:-}" && -n "${WAYLAND_DISPLAY:-}" ]]; then
    return 0
  fi

  local socket=""
  if [[ -n "${XDG_RUNTIME_DIR:-}" ]]; then
    socket="$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -type s -name 'niri.*.sock' | head -n1 || true)"
  fi
  if [[ -z "$socket" ]]; then
    socket="$(find /run/user/"$(id -u)" -maxdepth 1 -type s -name 'niri.*.sock' 2>/dev/null | head -n1 || true)"
  fi
  if [[ -z "$socket" ]]; then
    return 1
  fi

  export TIRI_SOCKET="$socket"
  if [[ -z "${XDG_RUNTIME_DIR:-}" ]]; then
    export XDG_RUNTIME_DIR="$(dirname "$socket")"
  fi

  local base
  base="$(basename "$socket")"
  base="${base#niri.}"
  export WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-${base%%.*}}"
  return 0
}

if ! adopt_env; then
  echo "Could not discover TIRI_SOCKET and WAYLAND_DISPLAY from a live tiri process." >&2
  echo "Run this inside the tiri session, or export both variables manually." >&2
  exit 1
fi

echo "Adopted session env from tiri socket $TIRI_SOCKET"
echo "Running open/close profiling scenario"
echo "  Scenario:   $SCENARIO"
echo "  Window cmd: $WINDOW_CMD"
echo "  Repeat:     $REPEAT"
echo "  Socket:     $TIRI_SOCKET"
echo "  Display:    $WAYLAND_DISPLAY"
echo "  Output:     $OUTPUT_DIR"
echo

python3 "$ROOT/scripts/profile_tiri_scenario.py" \
  --scenario "$SCENARIO" \
  --window-cmd "$WINDOW_CMD" \
  --output-dir "$OUTPUT_DIR" \
  --repeat "$REPEAT" \
  --settle-timeout "$SETTLE_TIMEOUT" \
  --settle-interval "$SETTLE_INTERVAL" \
  --idle-grace "$IDLE_GRACE"
