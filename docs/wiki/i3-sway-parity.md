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

## Evidence Pointers

- Action surface: `tiri-ipc/src/lib.rs`
- Input action handling: `src/input/mod.rs`
- Tree and layout logic: `src/layout/container.rs`, `src/layout/tiling.rs`, `src/layout/floating.rs`, `src/layout/workspace.rs`
- Parity-oriented tests: `src/layout/tests.rs`, `src/tests/containers.rs`, `src/tests/floating.rs`, `src/tests/transactions.rs`
