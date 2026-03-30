#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${TIRI_PROFILE_OUTPUT_DIR:-/tmp/tiri-open-close-tracy-$(date +%Y%m%d-%H%M%S)}"
TRACE_FILE="$OUT_DIR/capture.tracy"
SCENARIO_OUT="$OUT_DIR/scenario"
TRACY_ADDR="${TIRI_TRACY_ADDRESS:-127.0.0.1}"
TRACY_PORT="${TIRI_TRACY_PORT:-8086}"

mkdir -p "$OUT_DIR"

echo "Starting Tracy capture"
echo "  Address: $TRACY_ADDR:$TRACY_PORT"
echo "  Trace:   $TRACE_FILE"
echo "  Output:  $OUT_DIR"
echo

tracy-capture -a "$TRACY_ADDR" -p "$TRACY_PORT" -o "$TRACE_FILE" &
CAPTURE_PID=$!
trap 'kill "$CAPTURE_PID" 2>/dev/null || true' EXIT
sleep 1

TIRI_PROFILE_OUTPUT_DIR="$SCENARIO_OUT" "$ROOT/scripts/profile_open_close.sh"

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
