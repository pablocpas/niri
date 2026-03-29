# Performance Profiling

The profiling workflow for this fork is `tiri`-only. The goal is to compare local changes against a repeatable workload on your real hardware, then use Tracy to understand where the time went.

## Build for Tracy

Use on-demand Tracy so the compositor only collects profiling data while the Tracy UI is attached:

```sh
cargo build --release --features=profile-with-tracy-ondemand
```

Use `profile-with-tracy` instead only if you specifically need startup profiling.

## Recommended Setup

- Run the profiled build on a quiet TTY session.
- Keep the test output simple at first: one monitor, no heavy background workloads.
- Prefer running the same scenario several times before and after a change instead of trusting a single run.

## Scripted Scenario Runner

Use `scripts/profile_tiri_scenario.py` to drive a live session over `TIRI_SOCKET` and write numeric summaries.

Example:

```sh
python3 scripts/profile_tiri_scenario.py \
  --scenario scripts/perf_scenarios/open_close.json \
  --window-cmd "foot --app-id perf-test" \
  --output-dir /tmp/tiri-profile-open-close
```

The runner will:

- create an isolated temporary workspace for each run,
- open windows using `--window-cmd`,
- send scripted IPC actions to `tiri`,
- wait for the session to go quiet after each step,
- write `summary.json` and `summary.csv`,
- and restore the original focused workspace when it finishes.

The first run is kept as a warmup and marked as such in the summary.

## Scenario Format

Scenario files live under `scripts/perf_scenarios/` and use JSON:

```json
{
  "name": "open-close",
  "initial_windows": 2,
  "steps": [
    { "kind": "action", "name": "FocusColumnRight" },
    { "kind": "spawn_window", "label": "open_extra" },
    {
      "kind": "action",
      "name": "CloseWindow",
      "args": { "id": null }
    }
  ]
}
```

Rules:

- `initial_windows` opens that many windows before the scripted steps start.
- `spawn_window` uses the command from `--window-cmd`.
- `action` sends an IPC action by name, with optional `args` matching the JSON form documented in [IPC, tiri msg](IPC.md).

## Reading the Results

- Use `summary.csv` for quick comparisons in a spreadsheet or ad hoc scripts.
- Use `summary.json` when you want the full per-run breakdown.
- Focus on `p50`, `p95`, and `max` for each step rather than only the total scenario time.

If a numeric regression appears, repeat the same scenario with Tracy attached and inspect:

- `IPC::Action` spans for the step being driven,
- redraw/render spans in `src/tiri.rs`,
- layout update spans such as `Layout::refresh` and `Layout::update_render_elements`,
- backend render spans on TTY such as `Tty::render` and `Tty::on_vblank`.

## Suggested Baseline Scenarios

- `scripts/perf_scenarios/open_close.json`: window spawn and close latency.
- `scripts/perf_scenarios/layout_mutations.json`: layout mode transitions and floating/fullscreen toggles.
- `scripts/perf_scenarios/focus_reorder.json`: focus and column reordering on a populated workspace.

Keep scenarios small and deterministic. If a workload only happens in your daily session, encode that workflow into a new scenario instead of relying on memory or manual interaction.
