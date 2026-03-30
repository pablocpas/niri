#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCENARIO="$ROOT/scripts/perf_scenarios/diverse_interactions.json"
WINDOW_CMD="${TIRI_PROFILE_WINDOW_CMD:-weston-simple-shm}"
REPEAT="${TIRI_PROFILE_REPEAT:-7}"
OUT_DIR="${TIRI_PROFILE_OUTPUT_DIR:-/tmp/tiri-diverse-tracy-$(date +%Y%m%d-%H%M%S)}"
TRACE_FILE="$OUT_DIR/capture.tracy"
SCENARIO_OUT="$OUT_DIR/scenario"
TRACY_ADDR="${TIRI_TRACY_ADDRESS:-127.0.0.1}"
TRACY_PORT="${TIRI_TRACY_PORT:-8086}"
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

mkdir -p "$OUT_DIR" "$SCENARIO_OUT"

if ! adopt_env; then
  echo "Could not discover TIRI_SOCKET and WAYLAND_DISPLAY from a live tiri process." >&2
  exit 1
fi

echo "Adopted session env from tiri socket $TIRI_SOCKET"
echo "Starting Tracy capture"
echo "  Address: $TRACY_ADDR:$TRACY_PORT"
echo "  Trace:   $TRACE_FILE"
echo "  Scenario: $SCENARIO"
echo "  Window cmd: $WINDOW_CMD"
echo "  Output:  $OUT_DIR"
echo

tracy-capture -a "$TRACY_ADDR" -p "$TRACY_PORT" -o "$TRACE_FILE" &
CAPTURE_PID=$!
trap 'kill "$CAPTURE_PID" 2>/dev/null || true' EXIT
sleep 1

python3 "$ROOT/scripts/profile_tiri_scenario.py" \
  --scenario "$SCENARIO" \
  --window-cmd "$WINDOW_CMD" \
  --output-dir "$SCENARIO_OUT" \
  --repeat "$REPEAT" \
  --settle-timeout "$SETTLE_TIMEOUT" \
  --settle-interval "$SETTLE_INTERVAL" \
  --idle-grace "$IDLE_GRACE" \
  --workspace-prefix PERF-DIVERSE

kill "$CAPTURE_PID" 2>/dev/null || true
wait "$CAPTURE_PID" || true
trap - EXIT

if command -v tracy-export >/dev/null 2>&1; then
  tracy-export -f csv -u "$TRACE_FILE" > "$OUT_DIR/cpu-zones-total.csv"
  tracy-export -f csv -u -s "$TRACE_FILE" > "$OUT_DIR/cpu-zones-self.csv"
  tracy-export -f messages "$TRACE_FILE" > "$OUT_DIR/messages.csv"
fi

echo
echo "Artifacts written to:"
echo "  $TRACE_FILE"
[[ -f "$OUT_DIR/cpu-zones-self.csv" ]] && echo "  $OUT_DIR/cpu-zones-self.csv"
[[ -f "$OUT_DIR/cpu-zones-total.csv" ]] && echo "  $OUT_DIR/cpu-zones-total.csv"
[[ -f "$OUT_DIR/messages.csv" ]] && echo "  $OUT_DIR/messages.csv"
echo "  $SCENARIO_OUT/summary.json"
echo "  $SCENARIO_OUT/summary.csv"
