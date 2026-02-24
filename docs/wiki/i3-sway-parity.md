# i3/sway Parity Matrix

This page tracks parity between tiri and i3/sway behavior.

Status legend:

- `OK`: implemented and used in day-to-day workflows.
- `PARTIAL`: works, but behavior or syntax is not fully i3/sway-equivalent.
- `MISSING`: not implemented yet.

Scope notes:

- This matrix focuses on UX and behavior parity, not byte-for-byte config compatibility.
- tiri intentionally keeps some niri model decisions (for example dynamic workspaces).

## Core Tiling and Tree

| Area | i3/sway behavior | Status | Notes |
| --- | --- | --- | --- |
| Container tree | Arbitrary nested containers and leaves | `OK` | Implemented in tree/container model. |
| Split commands | `split h` / `split v` | `OK` | Available as split actions and keybinds. |
| Layout modes | `splith`, `splitv`, `tabbed`, `stacking` | `OK` | Supported at container level. |
| Layout cycle | `layout toggle all` | `OK` | Implemented as full layout cycle. |
| Parent/child focus | `focus parent` / `focus child` | `OK` | Container selection and re-entry behavior implemented. |
| Visual selected-container focus | Active border/ring on selected parent | `OK` | Implemented for border/focus ring path. |
| Tree cleanup | Flatten/squash redundant levels | `OK` | Backed by targeted cleanup regressions plus randomized container-tree operation tests. |

## Floating Parity

| Area | i3/sway behavior | Status | Notes |
| --- | --- | --- | --- |
| Toggle floating | `floating toggle` | `OK` | Implemented, including active-window path and by-id path. |
| Floating child containers | Split/tabbed/stacked inside floating | `OK` | Implemented and tested in layout tests. |
| Focus switch | Toggle focus between tiling and floating | `OK` | Implemented action and bindings. |
| Floating initial sizing | Stable first floating size behavior | `OK` | Deterministic 50% x 75% preset semantics with focused regression coverage and floating snapshots. |
| Floating tab hit-testing | Tabs should not leak focus/resize through top edge | `OK` | Recent fixes in floating hit logic. |

## Window and Workspace Operations

| Area | i3/sway behavior | Status | Notes |
| --- | --- | --- | --- |
| Directional focus and move | focus/move in directions | `OK` | Broad coverage in actions and tests. |
| Workspace focus/move | Focus/move windows and containers across workspaces | `OK` | Implemented with workspace reference support. |
| Fullscreen | Fullscreen toggle on focused window | `OK` | Implemented. |
| Scratchpad | `move scratchpad`, `scratchpad show` | `PARTIAL` | Implemented (round-robin queue), but semantics are not guaranteed 1:1 with sway in all edge cases. |
| Marks | `mark`, `unmark`, toggle/add/replace | `PARTIAL` | Implemented; criteria integration is not full sway syntax. |
| Sticky windows | Always-on-visible floating | `N/A+` | Extra feature not in i3 core parity target. |

## Config and Command Compatibility

| Area | i3/sway behavior | Status | Notes |
| --- | --- | --- | --- |
| Config syntax | i3/sway config syntax | `MISSING` | tiri uses KDL config, not i3/sway syntax. |
| Command grammar | Full `swaymsg`/`i3-msg` command grammar | `MISSING` | tiri uses `tiri msg action ...` with action enums. |
| Criteria commands | Full criteria selectors in command language | `PARTIAL` | Window rules and marks exist, but not full i3/sway criteria command model. |
| IPC protocol compatibility | sway/i3 IPC wire and command compatibility | `MISSING` | tiri has its own IPC model (`tiri-ipc`). |

## Intentional Differences

| Topic | Difference |
| --- | --- |
| Workspace model | Dynamic workspace stack inherited from niri, not strict i3 fixed workspace behavior. |
| Configuration | KDL-first configuration model. |
| Compositor scope | tiri includes compositor-centric features beyond i3/sway parity (screencast, overview, etc.). |

## Priority Backlog to Reach "High Parity"

1. Document exact intentional divergences (workspace model, config syntax, IPC) in one place and keep it current.
2. Add regression tests for border/focus visuals in all layout modes.
3. Decide whether to implement an i3/sway-compatible command adapter layer.
4. If adapter is desired, implement a minimal criteria subset (`con_mark`, `app_id/class`, workspace).
5. Add a compatibility table for keybind translation (sway/i3 to tiri actions).
6. Add CI gate for parity-critical tests so behavior changes are explicit.
7. Keep extending long-run randomized invariants (tree cleanup and mixed-flow transactions) as behavior evolves.
8. Revisit this matrix whenever behavior around floating/tree transactions changes.

## Differential Harness (sway vs tiri)

Use `scripts/sway_tiri_parity.py` to run the same scenario on both compositors and compare normalized state.

What it compares:

- Tiling tree structure and focused-path parity (strict).
- Tiling/floating window counts and `focused_is_floating` summary (strict mode).

What it does *not* compare yet:

- Full floating container tree shape (tiri IPC currently exposes tiling tree only).

Example workflow:

```bash
# 1) Generate reproducible random scenario.
python3 scripts/sway_tiri_parity.py generate \
  --seed 424242 --steps 300 --initial-windows 4 \
  --output /tmp/parity-scenario.json

# 2) Run against sway (requires running sway + swaymsg).
python3 scripts/sway_tiri_parity.py run \
  --target sway \
  --scenario /tmp/parity-scenario.json \
  --window-cmd "foot --app-id parity-test" \
  --output /tmp/parity-sway.json

# 3) Run against tiri (requires running tiri + tiri msg).
python3 scripts/sway_tiri_parity.py run \
  --target tiri \
  --scenario /tmp/parity-scenario.json \
  --window-cmd "foot --app-id parity-test" \
  --output /tmp/parity-tiri.json

# 4) Compare traces.
python3 scripts/sway_tiri_parity.py compare \
  --left /tmp/parity-sway.json \
  --right /tmp/parity-tiri.json \
  --mode strict
```

Batch campaign (many seeds):

```bash
python3 scripts/sway_tiri_parity.py campaign \
  --start-seed 1 \
  --count 100 \
  --steps 300 \
  --initial-windows 4 \
  --window-cmd "foot --app-id parity-test" \
  --output-dir /tmp/parity-campaign \
  --mode strict

# Inspect summary:
cat /tmp/parity-campaign/summary.json
```

Fully automatic headless campaign (recommended):

```bash
python3 scripts/sway_tiri_parity.py campaign \
  --start-seed 1 \
  --count 100 \
  --steps 300 \
  --initial-windows 4 \
  --window-cmd "foot --app-id parity-test" \
  --output-dir /tmp/parity-campaign \
  --auto-headless-sway \
  --auto-headless-tiri \
  --headless-outputs 1 \
  --headless-output-width 1920 \
  --headless-output-height 1080 \
  --mode strict
```

The harness will:

- spawn headless sway automatically,
- spawn headless tiri automatically (using `tiri --headless ...`),
- isolate both in dedicated runtime directories (no socket collisions),
- wire both IPC sockets,
- run the campaign,
- and stop both compositors at the end.

During random campaigns, some commands may be context-invalid in sway (for example layout changes while focused floating leaf). The harness treats those runtime context errors as no-op and continues, then compares resulting state.

If you run outside one compositor session and the socket env vars are missing:

- sway: set `SWAYSOCK` or pass `--sway-socket /path/to/sway.sock`
- tiri: set `TIRI_SOCKET` or pass `--tiri-socket /path/to/tiri.sock`

The harness now runs a preflight check and aborts early with a clear message if either IPC socket is unavailable.

Important: on real hardware, two DRM compositors on different VTs are not reliably testable in parallel.
If tiri is the active VT, sway can report "no outputs connected" (and vice versa).
Using `--auto-headless-sway --auto-headless-tiri` avoids this DRM/VT contention.
If you do not use headless mode, run in two phases:

1. Run all sway traces while sway owns the active output.
2. Run all tiri traces while tiri owns the active output.
3. Compare traces afterward.

## Evidence Pointers

- Action surface: `tiri-ipc/src/lib.rs`
- Input action handling: `src/input/mod.rs`
- Tree and layout logic: `src/layout/container.rs`, `src/layout/tiling.rs`, `src/layout/floating.rs`, `src/layout/workspace.rs`
- Parity-oriented tests: `src/layout/tests.rs`, `src/tests/containers.rs`, `src/tests/floating.rs`, `src/tests/transactions.rs`
