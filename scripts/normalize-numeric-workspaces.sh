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
    echo "jq is required" >&2
    exit 1
fi

json="$(${BIN} msg --json workspaces)"

# Build lines: output<TAB>name<TAB>num sorted by output then numeric value.
mapfile -t rows < <(
    jq -r '
      [ .[]
        | select(.output != null)
        | select(.name != null)
        | . as $ws
        | ($ws.name | tonumber?) as $n
        | select($n != null)
        | [$ws.output, $ws.name, ($n|tostring)]
      ]
      | sort_by(.[0], (.[2] | tonumber))
      | .[]
      | @tsv
    ' <<<"${json}"
)

if [[ "${#rows[@]}" -eq 0 ]]; then
    echo "No numeric workspaces found." >&2
    exit 0
fi

current_output=""
desired_idx=0

for row in "${rows[@]}"; do
    IFS=$'\t' read -r output name _num <<<"${row}"

    if [[ "${output}" != "${current_output}" ]]; then
        current_output="${output}"
        desired_idx=1
    fi

    "${BIN}" msg action focus-workspace "${name}" >/dev/null || true
    "${BIN}" msg action move-workspace-to-index "${desired_idx}" >/dev/null || true
    desired_idx=$((desired_idx + 1))
done

echo "Numeric workspaces normalized by output."
