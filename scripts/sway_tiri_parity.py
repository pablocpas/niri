#!/usr/bin/env python3
"""
Differential parity harness for sway vs tiri.

Capabilities:
1) generate reproducible random scenarios,
2) run a scenario against sway or tiri,
3) compare two traces step-by-step,
4) run a full campaign of many seeds,
5) optionally auto-start/stop headless sway and/or tiri for unattended runs.

This focuses on tree/layout behavior parity. Floating parity currently compares
observable summary fields (counts + focused mode), since tiri IPC does not
expose a floating container tree equivalent to sway's full tree.
"""

from __future__ import annotations

import argparse
import copy
import difflib
import json
import os
import random
import re
import signal
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any


KNOWN_LAYOUTS = {"splith", "splitv", "tabbed", "stacked"}
READY_TIMEOUT_S = 20.0

ACTION_MAP: dict[str, dict[str, Any]] = {
    # Use direct directional focus actions; workspace/column wrap fallbacks are not
    # equivalent to sway's container-tree traversal semantics.
    "focus_left": {"sway": "focus left", "tiri": ["focus-column-left"]},
    "focus_right": {"sway": "focus right", "tiri": ["focus-column-right"]},
    "focus_up": {"sway": "focus up", "tiri": ["focus-window-up"]},
    "focus_down": {"sway": "focus down", "tiri": ["focus-window-down"]},
    "split_h": {"sway": "split h", "tiri": ["split-horizontal"]},
    "split_v": {"sway": "split v", "tiri": ["split-vertical"]},
    "layout_splith": {"sway": "layout splith", "tiri": ["set-layout-split-h"]},
    "layout_splitv": {"sway": "layout splitv", "tiri": ["set-layout-split-v"]},
    "layout_toggle_split": {
        "sway": "layout toggle split",
        "tiri": ["toggle-split-layout"],
    },
    "layout_tabbed": {"sway": "layout tabbed", "tiri": ["set-layout-tabbed"]},
    "layout_stacked": {"sway": "layout stacking", "tiri": ["set-layout-stacked"]},
    "focus_parent": {"sway": "focus parent", "tiri": ["focus-parent"]},
    "focus_child": {"sway": "focus child", "tiri": ["focus-child"]},
    "toggle_floating": {"sway": "floating toggle", "tiri": ["toggle-window-floating"]},
    "toggle_focus_mode": {
        "sway": "focus mode_toggle",
        "tiri": ["switch-focus-between-floating-and-tiling"],
    },
    "toggle_fullscreen": {"sway": "fullscreen toggle", "tiri": ["fullscreen-window"]},
    "close_focused": {"sway": "kill", "tiri": ["close-window"]},
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="sway/tiri differential parity harness")
    sub = parser.add_subparsers(dest="cmd", required=True)

    gen = sub.add_parser("generate", help="Generate a random scenario JSON")
    gen.add_argument("--seed", type=int, required=True)
    gen.add_argument("--steps", type=int, default=200)
    gen.add_argument("--initial-windows", type=int, default=4)
    gen.add_argument("--output", type=Path, required=True)

    run = sub.add_parser("run", help="Run a scenario against sway or tiri")
    run.add_argument("--target", choices=["sway", "tiri"], required=True)
    run.add_argument("--scenario", type=Path, required=True)
    run.add_argument("--output", type=Path, required=True)
    run.add_argument("--window-cmd", required=True, help="Shell command that opens one window")
    run.add_argument("--workspace", default=None, help="Workspace name override")
    run.add_argument("--sway-socket", default=None, help="sway IPC socket path")
    run.add_argument("--tiri-socket", default=None, help="tiri IPC socket path")
    run.add_argument("--swaymsg-cmd", default="swaymsg", help="swaymsg command path")
    run.add_argument("--sway-bin", default="sway", help="sway binary path")
    run.add_argument("--tiri-cmd", default="tiri", help="tiri command/binary path")
    run.add_argument("--auto-headless-sway", action="store_true")
    run.add_argument("--auto-headless-tiri", action="store_true")
    run.add_argument("--headless-outputs", type=int, default=1)
    run.add_argument("--headless-output-width", type=int, default=1920)
    run.add_argument("--headless-output-height", type=int, default=1080)
    run.add_argument(
        "--runtime-dir",
        type=Path,
        default=None,
        help="Directory for auto-headless sockets/config/logs",
    )
    run.add_argument("--settle-timeout", type=float, default=2.5)
    run.add_argument("--settle-interval", type=float, default=0.05)
    run.add_argument(
        "--open-window-timeout",
        type=float,
        default=8.0,
        help="Max seconds to wait for a newly spawned window to appear",
    )
    run.add_argument(
        "--open-window-retries",
        type=int,
        default=1,
        help="Extra open-window retries if window count did not increase",
    )

    compare = sub.add_parser("compare", help="Compare two trace JSONs")
    compare.add_argument("--left", type=Path, required=True)
    compare.add_argument("--right", type=Path, required=True)
    compare.add_argument(
        "--mode",
        choices=["strict", "tiling-only"],
        default="strict",
        help="strict: tree + floating summary, tiling-only: tree only",
    )

    campaign = sub.add_parser(
        "campaign",
        help="Run many seeds end-to-end (generate + run sway + run tiri + compare)",
    )
    campaign.add_argument("--start-seed", type=int, default=1)
    campaign.add_argument("--count", type=int, default=50)
    campaign.add_argument("--steps", type=int, default=250)
    campaign.add_argument("--initial-windows", type=int, default=4)
    campaign.add_argument("--window-cmd", required=True, help="Shell command that opens one window")
    campaign.add_argument("--output-dir", type=Path, required=True)
    campaign.add_argument(
        "--mode",
        choices=["strict", "tiling-only"],
        default="strict",
        help="strict: tree + floating summary, tiling-only: tree only",
    )
    campaign.add_argument("--workspace-prefix", default="PARITY")
    campaign.add_argument("--sway-socket", default=None, help="sway IPC socket path")
    campaign.add_argument("--tiri-socket", default=None, help="tiri IPC socket path")
    campaign.add_argument("--swaymsg-cmd", default="swaymsg", help="swaymsg command path")
    campaign.add_argument("--sway-bin", default="sway", help="sway binary path")
    campaign.add_argument("--tiri-cmd", default="tiri", help="tiri command/binary path")
    campaign.add_argument("--auto-headless-sway", action="store_true")
    campaign.add_argument("--auto-headless-tiri", action="store_true")
    campaign.add_argument("--headless-outputs", type=int, default=1)
    campaign.add_argument("--headless-output-width", type=int, default=1920)
    campaign.add_argument("--headless-output-height", type=int, default=1080)
    campaign.add_argument(
        "--runtime-dir",
        type=Path,
        default=None,
        help="Directory for auto-headless sockets/config/logs",
    )
    campaign.add_argument("--settle-timeout", type=float, default=2.5)
    campaign.add_argument("--settle-interval", type=float, default=0.05)
    campaign.add_argument(
        "--open-window-timeout",
        type=float,
        default=8.0,
        help="Max seconds to wait for a newly spawned window to appear",
    )
    campaign.add_argument(
        "--open-window-retries",
        type=int,
        default=1,
        help="Extra open-window retries if window count did not increase",
    )
    campaign.add_argument("--stop-on-first-fail", action="store_true")

    return parser.parse_args()


def run_cmd(
    args: list[str], expect_json: bool = False, env_updates: dict[str, str] | None = None
) -> Any:
    env = os.environ.copy()
    if env_updates:
        env.update(env_updates)
    proc = subprocess.run(args, text=True, capture_output=True, env=env)
    if proc.returncode != 0:
        msg = proc.stderr.strip() or proc.stdout.strip() or "(no output)"
        raise RuntimeError(f"command failed ({proc.returncode}): {' '.join(args)}\n{msg}")
    out = proc.stdout.strip()
    if expect_json:
        if not out:
            raise RuntimeError(f"expected JSON output from {' '.join(args)}, got empty output")
        return json.loads(out)
    return out


def is_sway_context_noop_error(error_text: str) -> bool:
    """
    Return True when sway rejected a command due to runtime context (not syntax),
    which should be treated as a no-op for parity fuzzing.
    """
    lowered = error_text.lower()

    # swaymsg usually returns parse_error=false for context failures.
    # Some runtime-context failures are still reported with parse_error=true.
    if '"success": false' in lowered and '"parse_error": false' in lowered:
        return True

    # Fallback substrings for common sway runtime-context errors.
    known = [
        "unable to change layout of floating windows",
        "failed to find a floating container in workspace",
        "cannot change focus mode",
        "can't float an empty workspace",
    ]
    if any(s in lowered for s in known):
        return True

    # Broad fallback for empty-workspace command rejections.
    if "empty workspace" in lowered and '"success": false' in lowered:
        return True

    return False


def tail_text(path: Path, lines: int = 80) -> str:
    if not path.exists():
        return ""
    try:
        data = path.read_text(encoding="utf-8", errors="replace")
    except Exception:  # noqa: BLE001
        return ""
    split = data.splitlines()
    return "\n".join(split[-lines:])


@dataclass
class ManagedCompositor:
    name: str
    proc: subprocess.Popen[Any]
    socket: str
    log_path: Path
    log_handle: Any
    config_path: Path | None = None

    def stop(self) -> None:
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=5)
        try:
            self.log_handle.close()
        except Exception:  # noqa: BLE001
            pass


def start_headless_sway(
    runtime_dir: Path,
    sway_bin: str,
    swaymsg_cmd: str,
    outputs: int,
) -> ManagedCompositor:
    runtime_dir.mkdir(parents=True, exist_ok=True)
    xdg_runtime_dir = runtime_dir / "xdg-runtime-sway"
    xdg_runtime_dir.mkdir(parents=True, exist_ok=True)
    xdg_runtime_dir.chmod(0o700)
    socket_path = runtime_dir / "sway-headless.sock"
    log_path = runtime_dir / "sway-headless.log"
    config_path = runtime_dir / "sway-headless.config"

    config_path.write_text(
        "\n".join(
            [
                "# autogenerated for parity harness",
                "set $mod Mod4",
                "bindsym $mod+Shift+e exit",
            ]
        )
        + "\n",
        encoding="utf-8",
    )

    env = os.environ.copy()
    env["SWAYSOCK"] = str(socket_path)
    env["XDG_RUNTIME_DIR"] = str(xdg_runtime_dir)
    env["WLR_BACKENDS"] = "headless"
    env["WLR_LIBINPUT_NO_DEVICES"] = "1"
    env["WLR_HEADLESS_OUTPUTS"] = str(max(outputs, 1))

    log_handle = open(log_path, "w", encoding="utf-8")
    proc = subprocess.Popen(
        [sway_bin, "-c", str(config_path), "-d"],
        stdout=log_handle,
        stderr=subprocess.STDOUT,
        env=env,
    )

    deadline = time.monotonic() + READY_TIMEOUT_S
    last_error = ""
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(
                f"sway headless exited early with code {proc.returncode}.\n{tail_text(log_path)}"
            )

        if socket_path.exists():
            try:
                out = run_cmd(
                    [swaymsg_cmd, "-t", "get_outputs"],
                    expect_json=True,
                    env_updates={"SWAYSOCK": str(socket_path)},
                )
                active = any(
                    bool(x.get("active", False)) for x in out if isinstance(x, dict)
                )
                if active:
                    return ManagedCompositor(
                        name="sway",
                        proc=proc,
                        socket=str(socket_path),
                        log_path=log_path,
                        log_handle=log_handle,
                        config_path=config_path,
                    )
                last_error = "sway started but no active headless output yet"
            except Exception as e:  # noqa: BLE001
                last_error = str(e)
        time.sleep(0.1)

    raise RuntimeError(
        "timed out waiting for headless sway readiness.\n"
        f"last_error: {last_error}\n"
        f"log tail:\n{tail_text(log_path)}"
    )


def parse_tiri_socket_from_log(log_path: Path) -> str | None:
    if not log_path.exists():
        return None
    try:
        text = log_path.read_text(encoding="utf-8", errors="replace")
    except Exception:  # noqa: BLE001
        return None
    m = re.findall(r"IPC listening on:\s*(\S+)", text)
    if not m:
        return None
    return m[-1]


def start_headless_tiri(
    runtime_dir: Path,
    tiri_cmd: str,
    outputs: int,
    width: int,
    height: int,
) -> ManagedCompositor:
    runtime_dir.mkdir(parents=True, exist_ok=True)
    xdg_runtime_dir = runtime_dir / "xdg-runtime-tiri"
    xdg_runtime_dir.mkdir(parents=True, exist_ok=True)
    xdg_runtime_dir.chmod(0o700)
    log_path = runtime_dir / "tiri-headless.log"
    config_path = runtime_dir / "tiri-headless.config.kdl"

    config_path.write_text(
        "\n".join(
            [
                "// autogenerated for parity harness",
                "layout {",
                "    gaps 0",
                "}",
                "binds {}",
            ]
        )
        + "\n",
        encoding="utf-8",
    )

    log_handle = open(log_path, "w", encoding="utf-8")
    proc = subprocess.Popen(
        [
            tiri_cmd,
            "--config",
            str(config_path),
            "--headless",
            "--headless-outputs",
            str(max(outputs, 1)),
            "--headless-output-width",
            str(max(width, 1)),
            "--headless-output-height",
            str(max(height, 1)),
        ],
        stdout=log_handle,
        stderr=subprocess.STDOUT,
        env={**os.environ.copy(), "XDG_RUNTIME_DIR": str(xdg_runtime_dir)},
    )

    deadline = time.monotonic() + READY_TIMEOUT_S
    last_error = ""
    socket_path: str | None = None
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(
                f"tiri headless exited early with code {proc.returncode}.\n{tail_text(log_path)}"
            )

        socket_path = parse_tiri_socket_from_log(log_path)
        if socket_path:
            try:
                run_cmd(
                    [tiri_cmd, "msg", "--json", "workspaces"],
                    expect_json=True,
                    env_updates={"TIRI_SOCKET": socket_path},
                )
                return ManagedCompositor(
                    name="tiri",
                    proc=proc,
                    socket=socket_path,
                    log_path=log_path,
                    log_handle=log_handle,
                    config_path=config_path,
                )
            except Exception as e:  # noqa: BLE001
                last_error = str(e)

        time.sleep(0.1)

    raise RuntimeError(
        "timed out waiting for headless tiri readiness.\n"
        f"last_error: {last_error}\n"
        f"log tail:\n{tail_text(log_path)}"
    )


def map_layout_name(layout: Any) -> str:
    if not isinstance(layout, str):
        return "splith"
    lowered = layout.strip().lower()
    if lowered in KNOWN_LAYOUTS:
        return lowered
    if lowered in {"none", "default"}:
        return "splith"
    return "splith"


def map_tiri_layout(layout: Any) -> str:
    if layout == "SplitH":
        return "splith"
    if layout == "SplitV":
        return "splitv"
    if layout == "Tabbed":
        return "tabbed"
    if layout == "Stacked":
        return "stacked"
    return "splith"


def is_sway_window_leaf(node: dict[str, Any]) -> bool:
    if node.get("nodes"):
        return False
    if node.get("floating_nodes"):
        return False
    if node.get("window") is not None:
        return True
    if node.get("app_id") is not None or node.get("pid") is not None:
        return True
    node_type = node.get("type")
    return node_type in {"con", "floating_con"}


def normalize_sway_node(node: dict[str, Any]) -> dict[str, Any]:
    children = [normalize_sway_node(child) for child in node.get("nodes", [])]
    focused = bool(node.get("focused", False))
    if children:
        return {
            "kind": "container",
            "layout": map_layout_name(node.get("layout")),
            "focused": focused,
            "children": children,
        }
    if is_sway_window_leaf(node):
        return {"kind": "leaf", "focused": focused}
    return {
        "kind": "container",
        "layout": map_layout_name(node.get("layout")),
        "focused": focused,
        "children": [],
    }


def normalize_tiri_node(node: dict[str, Any]) -> dict[str, Any]:
    focused = bool(node.get("focused", False))
    layout = node.get("layout")
    if layout is None:
        return {"kind": "leaf", "focused": focused}
    children = [normalize_tiri_node(child) for child in node.get("children", [])]
    return {
        "kind": "container",
        "layout": map_tiri_layout(layout),
        "focused": focused,
        "children": children,
    }


def count_tiri_leaf_nodes(node: Any) -> int:
    if not isinstance(node, dict):
        return 0

    layout = node.get("layout")
    if layout is None:
        return 1

    count = 0
    for child in node.get("children", []):
        count += count_tiri_leaf_nodes(child)
    return count


def collect_sway_workspaces(root: dict[str, Any]) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []

    def walk(node: dict[str, Any]) -> None:
        if node.get("type") == "workspace":
            out.append(node)
            return
        for child in node.get("nodes", []):
            walk(child)
        for child in node.get("floating_nodes", []):
            walk(child)

    walk(root)
    return out


def find_focused_sway_workspace(
    root: dict[str, Any], workspace_hint: str | None = None
) -> dict[str, Any] | None:
    workspaces = collect_sway_workspaces(root)
    if not workspaces:
        return None

    focused = next((ws for ws in workspaces if bool(ws.get("focused", False))), None)
    if focused is not None:
        return focused

    if workspace_hint is not None:
        hinted = next((ws for ws in workspaces if ws.get("name") == workspace_hint), None)
        if hinted is not None:
            return hinted

    # Deterministic fallback for headless cases without explicit focus.
    named = [ws for ws in workspaces if ws.get("name") is not None]
    if named:
        named.sort(key=lambda ws: (ws.get("num", 10_000), str(ws.get("name"))))
        return named[0]

    return workspaces[0]


def count_sway_windows(node: dict[str, Any]) -> int:
    count = 1 if is_sway_window_leaf(node) else 0
    for child in node.get("nodes", []):
        count += count_sway_windows(child)
    for child in node.get("floating_nodes", []):
        count += count_sway_windows(child)
    return count


def subtree_has_focus(node: dict[str, Any]) -> bool:
    if bool(node.get("focused", False)):
        return True
    for child in node.get("nodes", []):
        if subtree_has_focus(child):
            return True
    for child in node.get("floating_nodes", []):
        if subtree_has_focus(child):
            return True
    return False


def sway_focused_child_target(node: dict[str, Any]) -> tuple[str, int] | None:
    """Return focused direct child target as ("tiling"|"floating", index)."""
    nodes = node.get("nodes", [])
    floating_nodes = node.get("floating_nodes", [])
    if not isinstance(nodes, list) or not isinstance(floating_nodes, list):
        return None

    node_ids: dict[Any, int] = {}
    for idx, child in enumerate(nodes):
        if isinstance(child, dict):
            node_ids[child.get("id")] = idx

    floating_ids: dict[Any, int] = {}
    for idx, child in enumerate(floating_nodes):
        if isinstance(child, dict):
            floating_ids[child.get("id")] = idx

    focus_order = node.get("focus", [])
    if isinstance(focus_order, list):
        for target_id in focus_order:
            if target_id in node_ids:
                return ("tiling", node_ids[target_id])
            if target_id in floating_ids:
                return ("floating", floating_ids[target_id])

    # Fallback for environments where focus order is missing/incomplete.
    for idx, child in enumerate(nodes):
        if isinstance(child, dict) and subtree_has_focus(child):
            return ("tiling", idx)
    for idx, child in enumerate(floating_nodes):
        if isinstance(child, dict) and subtree_has_focus(child):
            return ("floating", idx)

    return None


def sway_tiling_focus_path(
    workspace: dict[str, Any],
) -> tuple[list[int], bool, str]:
    """
    Returns:
      - tiling focus path from workspace nodes
      - whether tiling focus path is known
      - focus mode: "tiling", "floating", or "unknown"
    """
    path: list[int] = []
    current = workspace
    max_depth = 512

    for _ in range(max_depth):
        target = sway_focused_child_target(current)
        if target is None:
            return path, False, "unknown"

        kind, idx = target
        if kind == "floating":
            return path, False, "floating"

        children = current.get("nodes", [])
        if not isinstance(children, list) or idx < 0 or idx >= len(children):
            return path, False, "unknown"

        path.append(idx)
        next_node = children[idx]
        if not isinstance(next_node, dict):
            return path, False, "unknown"

        current = next_node
        if not current.get("nodes") and not current.get("floating_nodes"):
            return path, True, "tiling"

    return path, False, "unknown"


def mark_normalized_focus_path(root: dict[str, Any] | None, path: list[int]) -> bool:
    if not isinstance(root, dict):
        return False

    current = root
    current["focused"] = True

    for idx in path:
        children = current.get("children")
        if not isinstance(children, list) or idx < 0 or idx >= len(children):
            return False
        child = children[idx]
        if not isinstance(child, dict):
            return False
        child["focused"] = True
        current = child

    return True


def normalize_sway_tiling_root(workspace: dict[str, Any]) -> dict[str, Any] | None:
    raw_children = workspace.get("nodes", [])
    children = [normalize_sway_node(child) for child in raw_children]
    if not children:
        return None
    if len(children) == 1:
        return children[0]
    return {
        "kind": "container",
        "layout": map_layout_name(workspace.get("layout")),
        "focused": bool(workspace.get("focused", False)),
        "children": children,
    }


def normalize_sway_snapshot(
    sway_socket: str | None, swaymsg_cmd: str, workspace_hint: str | None = None
) -> dict[str, Any]:
    env_updates = {"SWAYSOCK": sway_socket} if sway_socket else None
    tree = run_cmd([swaymsg_cmd, "-t", "get_tree"], expect_json=True, env_updates=env_updates)
    workspace = find_focused_sway_workspace(tree, workspace_hint=workspace_hint)
    if workspace is None:
        raise RuntimeError("sway: could not find focused workspace in get_tree")

    tiling_root = normalize_sway_tiling_root(workspace)
    raw_focus_path, raw_focus_known, raw_focus_mode = sway_tiling_focus_path(workspace)
    normalized_focus_path = list(raw_focus_path)
    if (
        len(workspace.get("nodes", [])) == 1
        and normalized_focus_path
        and normalized_focus_path[0] == 0
    ):
        # normalize_sway_tiling_root() unwraps single workspace child.
        normalized_focus_path = normalized_focus_path[1:]

    tiling_focus_known = False
    if raw_focus_known:
        tiling_focus_known = mark_normalized_focus_path(tiling_root, normalized_focus_path)
    if not tiling_focus_known:
        tiling_focus_known = tree_has_focus_marker(tiling_root)

    tiling_count = sum(count_sway_windows(node) for node in workspace.get("nodes", []))
    floating_count = sum(count_sway_windows(node) for node in workspace.get("floating_nodes", []))
    focused_is_floating_known = raw_focus_mode != "unknown"
    focused_is_floating = (
        raw_focus_mode == "floating"
        if focused_is_floating_known
        else any(subtree_has_focus(node) for node in workspace.get("floating_nodes", []))
    )
    tiling_leaf_focus_known = tree_has_leaf_focus_marker(tiling_root)

    return {
        "workspace_name": workspace.get("name"),
        "workspace_num": workspace.get("num"),
        "tiling_tree": tiling_root,
        "tiling_focus_known": tiling_focus_known,
        "tiling_leaf_focus_known": tiling_leaf_focus_known,
        "sway_raw_tiling_focus_path": raw_focus_path,
        "sway_raw_tiling_focus_known": raw_focus_known,
        "sway_raw_focus_mode": raw_focus_mode,
        "tiling_count": tiling_count,
        "floating_count": floating_count,
        "window_count": tiling_count + floating_count,
        "focused_is_floating": focused_is_floating,
        "focused_is_floating_known": focused_is_floating_known,
    }


def sway_has_connected_outputs(sway_socket: str | None, swaymsg_cmd: str) -> bool:
    env_updates = {"SWAYSOCK": sway_socket} if sway_socket else None
    outputs = run_cmd(
        [swaymsg_cmd, "-t", "get_outputs"],
        expect_json=True,
        env_updates=env_updates,
    )
    if not isinstance(outputs, list):
        return False
    return any(bool(output.get("active", False)) for output in outputs if isinstance(output, dict))


def normalize_tiri_snapshot(tiri_socket: str | None, tiri_cmd: str) -> dict[str, Any]:
    env_updates = {"TIRI_SOCKET": tiri_socket} if tiri_socket else None
    tree = run_cmd([tiri_cmd, "msg", "--json", "tree"], expect_json=True, env_updates=env_updates)
    workspaces = run_cmd(
        [tiri_cmd, "msg", "--json", "workspaces"],
        expect_json=True,
        env_updates=env_updates,
    )
    windows = run_cmd(
        [tiri_cmd, "msg", "--json", "windows"],
        expect_json=True,
        env_updates=env_updates,
    )

    focused_workspace = next((w for w in workspaces if w.get("is_focused")), None)
    if focused_workspace is None:
        raise RuntimeError("tiri: could not find focused workspace in workspaces IPC")
    ws_id = focused_workspace.get("id")

    ws_windows = [w for w in windows if w.get("workspace_id") == ws_id]
    floating_count = sum(1 for w in ws_windows if w.get("is_floating"))
    focused_window = next((w for w in ws_windows if w.get("is_focused")), None)
    focused_is_floating_known = focused_window is not None
    focused_is_floating = bool(focused_window and focused_window.get("is_floating"))

    root = tree.get("root")
    tiling_root = normalize_tiri_node(root) if root is not None else None
    tiling_focus_known = tree_has_focus_marker(tiling_root)
    tiling_leaf_focus_known = tree_has_leaf_focus_marker(tiling_root)
    tiling_count = count_tiri_leaf_nodes(root)

    return {
        "workspace_name": focused_workspace.get("name"),
        "workspace_num": None,
        "tiling_tree": tiling_root,
        "tiling_focus_known": tiling_focus_known,
        "tiling_leaf_focus_known": tiling_leaf_focus_known,
        "tiling_count": tiling_count,
        "floating_count": floating_count,
        "window_count": tiling_count + floating_count,
        "focused_is_floating": focused_is_floating,
        "focused_is_floating_known": focused_is_floating_known,
    }


def focused_path(root: dict[str, Any] | None) -> list[int]:
    if root is None:
        return []

    def deepest_focused(node: dict[str, Any], cur: list[int]) -> list[int] | None:
        best: list[int] | None = cur if node.get("focused", False) else None
        for i, child in enumerate(node.get("children", [])):
            candidate = deepest_focused(child, cur + [i])
            if candidate is not None:
                best = candidate
        return best

    def node_at_path(node: dict[str, Any], path: list[int]) -> dict[str, Any] | None:
        cur = node
        for idx in path:
            children = cur.get("children", [])
            if not isinstance(children, list) or idx < 0 or idx >= len(children):
                return None
            child = children[idx]
            if not isinstance(child, dict):
                return None
            cur = child
        return cur

    path = deepest_focused(root, []) or []
    cur = node_at_path(root, path)
    if cur is None:
        return path

    # Canonicalize focus to a leaf path when a focused child is explicitly marked.
    # If only container focus is marked, keep that container path instead of
    # guessing child index 0.
    while isinstance(cur, dict):
        children = cur.get("children")
        if not isinstance(children, list) or not children:
            break
        focused_idx = next(
            (i for i, child in enumerate(children) if isinstance(child, dict) and child.get("focused", False)),
            None,
        )
        if focused_idx is None:
            break
        path.append(focused_idx)
        nxt = children[focused_idx]
        if not isinstance(nxt, dict):
            break
        cur = nxt

    return path


def tree_has_focus_marker(node: Any) -> bool:
    if not isinstance(node, dict):
        return False
    if bool(node.get("focused", False)):
        return True
    children = node.get("children")
    if not isinstance(children, list):
        return False
    return any(tree_has_focus_marker(child) for child in children)


def tree_has_leaf_focus_marker(node: Any) -> bool:
    if not isinstance(node, dict):
        return False

    children = node.get("children")
    is_leaf = not isinstance(children, list) or len(children) == 0
    if is_leaf:
        return bool(node.get("focused", False))

    return any(tree_has_leaf_focus_marker(child) for child in children)


def canonical_snapshot(snapshot: dict[str, Any]) -> dict[str, Any]:
    canonical = copy.deepcopy(snapshot)
    canonical["focused_path"] = focused_path(canonical.get("tiling_tree"))
    return canonical


@dataclass
class Runner:
    target: str
    window_cmd: str
    settle_timeout: float
    settle_interval: float
    sway_socket: str | None = None
    tiri_socket: str | None = None
    swaymsg_cmd: str = "swaymsg"
    tiri_cmd: str = "tiri"
    workspace_hint: str | None = None
    open_window_timeout: float = 8.0
    open_window_retries: int = 1

    def _sway_env(self) -> dict[str, str] | None:
        return {"SWAYSOCK": self.sway_socket} if self.sway_socket else None

    def _tiri_env(self) -> dict[str, str] | None:
        return {"TIRI_SOCKET": self.tiri_socket} if self.tiri_socket else None

    def _tiri_workspaces(self) -> list[dict[str, Any]]:
        workspaces = run_cmd(
            [self.tiri_cmd, "msg", "--json", "workspaces"],
            expect_json=True,
            env_updates=self._tiri_env(),
        )
        if not isinstance(workspaces, list):
            raise RuntimeError("tiri: workspaces IPC did not return a list")
        return workspaces

    def _tiri_focused_workspace(self) -> dict[str, Any] | None:
        return next((ws for ws in self._tiri_workspaces() if ws.get("is_focused")), None)

    def _focus_workspace_tiri(self, name: str) -> None:
        # Try direct focus first. This works if the named workspace already exists.
        run_cmd(
            [self.tiri_cmd, "msg", "action", "focus-workspace", name],
            env_updates=self._tiri_env(),
        )
        focused = self._tiri_focused_workspace()
        if focused is not None and focused.get("name") == name:
            return

        # Unlike sway, focusing a non-existing named workspace is a no-op in tiri.
        # Create one by focusing a free transient numeric workspace, naming it, and
        # then focusing by name.
        names = {
            str(ws.get("name"))
            for ws in self._tiri_workspaces()
            if ws.get("name") is not None
        }
        slot = next((str(i) for i in range(255, 0, -1) if str(i) not in names), None)
        if slot is None:
            raise RuntimeError("tiri: could not allocate a transient workspace slot")

        run_cmd(
            [self.tiri_cmd, "msg", "action", "focus-workspace", slot],
            env_updates=self._tiri_env(),
        )
        run_cmd(
            [self.tiri_cmd, "msg", "action", "set-workspace-name", name],
            env_updates=self._tiri_env(),
        )
        run_cmd(
            [self.tiri_cmd, "msg", "action", "focus-workspace", name],
            env_updates=self._tiri_env(),
        )

        focused = self._tiri_focused_workspace()
        if focused is None or focused.get("name") != name:
            raise RuntimeError(f"tiri: failed to focus workspace '{name}'")

    def preflight(self) -> None:
        if self.target == "sway":
            try:
                run_cmd(
                    [self.swaymsg_cmd, "-t", "get_tree"],
                    expect_json=True,
                    env_updates=self._sway_env(),
                )
                if not sway_has_connected_outputs(self.sway_socket, self.swaymsg_cmd):
                    raise RuntimeError(
                        "sway IPC is reachable but has no connected outputs. "
                        "If running on VTs, this usually means another compositor owns DRM."
                    )
            except RuntimeError as e:
                raise RuntimeError(
                    "sway preflight failed. Ensure SWAYSOCK is correct and sway has active outputs.\n"
                    f"{e}"
                ) from e
            return

        if self.target == "tiri":
            try:
                run_cmd(
                    [self.tiri_cmd, "msg", "--json", "workspaces"],
                    expect_json=True,
                    env_updates=self._tiri_env(),
                )
            except RuntimeError as e:
                raise RuntimeError(
                    "tiri preflight failed. Ensure TIRI_SOCKET is correct.\n"
                    f"{e}"
                ) from e
            return

        raise RuntimeError(f"unknown target for preflight: {self.target}")

    def snapshot(self) -> dict[str, Any]:
        if self.target == "sway":
            return canonical_snapshot(
                normalize_sway_snapshot(
                    self.sway_socket,
                    self.swaymsg_cmd,
                    workspace_hint=self.workspace_hint,
                )
            )
        if self.target == "tiri":
            return canonical_snapshot(normalize_tiri_snapshot(self.tiri_socket, self.tiri_cmd))
        raise RuntimeError(f"unknown target: {self.target}")

    def run_target_action(self, op: str) -> None:
        if op == "open_window":
            if self.target == "sway":
                run_cmd(
                    [self.swaymsg_cmd, "exec", self.window_cmd],
                    env_updates=self._sway_env(),
                )
            else:
                run_cmd(
                    [self.tiri_cmd, "msg", "action", "spawn-sh", "--", self.window_cmd],
                    env_updates=self._tiri_env(),
                )
            return

        mapping = ACTION_MAP.get(op)
        if mapping is None:
            raise RuntimeError(f"unsupported operation: {op}")

        try:
            if self.target == "sway":
                run_cmd([self.swaymsg_cmd, mapping["sway"]], env_updates=self._sway_env())
                return

            run_cmd(
                [self.tiri_cmd, "msg", "action", *mapping["tiri"]],
                env_updates=self._tiri_env(),
            )
        except RuntimeError as e:
            # Sway may reject some actions depending on current focus/container type.
            # Treat these as no-op so campaigns can continue and compare resulting trees.
            if self.target == "sway" and is_sway_context_noop_error(str(e)):
                return
            raise

    def focus_workspace(self, name: str) -> None:
        self.workspace_hint = name
        if self.target == "sway":
            run_cmd([self.swaymsg_cmd, f"workspace {name}"], env_updates=self._sway_env())
            return
        self._focus_workspace_tiri(name)

    def wait_for_window_count_increase(self, before_count: int) -> dict[str, Any]:
        deadline = time.monotonic() + self.open_window_timeout
        last = self.snapshot()
        while time.monotonic() < deadline:
            snap = self.settle_and_capture()
            last = snap
            if snap["window_count"] > before_count:
                return snap
            time.sleep(self.settle_interval)
        return last

    def open_window_and_capture(self, before_count: int) -> dict[str, Any]:
        attempts = max(self.open_window_retries, 0) + 1
        last = self.snapshot()
        for _ in range(attempts):
            self.run_target_action("open_window")
            snap = self.wait_for_window_count_increase(before_count)
            last = snap
            if snap["window_count"] > before_count:
                return snap
        return last

    def settle_and_capture(self) -> dict[str, Any]:
        deadline = time.monotonic() + self.settle_timeout
        last = None
        stable = 0
        while time.monotonic() < deadline:
            snap = self.snapshot()
            if snap == last:
                stable += 1
            else:
                stable = 0
                last = snap
            if stable >= 1:
                return snap
            time.sleep(self.settle_interval)
        return self.snapshot()


def generate_scenario(seed: int, steps: int, initial_windows: int) -> dict[str, Any]:
    rng = random.Random(seed)

    weighted_ops: list[tuple[str, int]] = [
        ("focus_left", 6),
        ("focus_right", 6),
        ("focus_up", 6),
        ("focus_down", 6),
        ("split_h", 5),
        ("split_v", 5),
        ("layout_splith", 3),
        ("layout_splitv", 3),
        ("layout_toggle_split", 4),
        ("layout_tabbed", 3),
        ("layout_stacked", 3),
        ("focus_parent", 6),
        ("focus_child", 6),
        ("toggle_floating", 4),
        ("toggle_focus_mode", 4),
        ("toggle_fullscreen", 2),
        ("close_focused", 3),
        ("open_window", 8),
    ]

    bag: list[str] = []
    for op, weight in weighted_ops:
        bag.extend([op] * weight)

    est_windows = max(initial_windows, 1)
    operations: list[dict[str, Any]] = []
    for _ in range(steps):
        if est_windows <= 1:
            op = "open_window"
        else:
            op = bag[rng.randrange(len(bag))]

        if op == "close_focused" and est_windows <= 1:
            op = "open_window"

        if op == "open_window":
            est_windows += 1
        elif op == "close_focused":
            est_windows = max(est_windows - 1, 0)

        operations.append({"op": op})

    return {
        "version": 1,
        "seed": seed,
        "initial_windows": initial_windows,
        "operations": operations,
    }


def run_scenario(
    target: str,
    scenario: dict[str, Any],
    output: Path,
    window_cmd: str,
    workspace_override: str | None,
    settle_timeout: float,
    settle_interval: float,
    open_window_timeout: float = 8.0,
    open_window_retries: int = 1,
    sway_socket: str | None = None,
    tiri_socket: str | None = None,
    swaymsg_cmd: str = "swaymsg",
    tiri_cmd: str = "tiri",
) -> dict[str, Any]:
    seed = scenario.get("seed", 0)
    workspace = workspace_override or f"PARITY_{seed}_{target}"
    initial_windows = int(scenario.get("initial_windows", 0))
    operations = scenario.get("operations", [])

    if not isinstance(operations, list):
        raise RuntimeError("scenario.operations must be a list")

    runner = Runner(
        target=target,
        window_cmd=window_cmd,
        settle_timeout=settle_timeout,
        settle_interval=settle_interval,
        sway_socket=sway_socket,
        tiri_socket=tiri_socket,
        swaymsg_cmd=swaymsg_cmd,
        tiri_cmd=tiri_cmd,
        open_window_timeout=open_window_timeout,
        open_window_retries=open_window_retries,
    )
    runner.preflight()

    runner.focus_workspace(workspace)
    initial_snapshot = runner.settle_and_capture()
    for i in range(initial_windows):
        before_count = initial_snapshot["window_count"]
        initial_snapshot = runner.open_window_and_capture(before_count)
        if initial_snapshot["window_count"] <= before_count:
            raise RuntimeError(
                f"initial open_window #{i} did not increase window count "
                f"({before_count} -> {initial_snapshot['window_count']})"
            )

    states: list[dict[str, Any]] = [{"step": -1, "op": "initial", "snapshot": initial_snapshot}]

    for i, action in enumerate(operations):
        if not isinstance(action, dict) or "op" not in action:
            raise RuntimeError(f"invalid scenario operation at index {i}: {action!r}")
        op = action["op"]
        before_count = states[-1]["snapshot"]["window_count"]
        if op == "open_window":
            snap = runner.open_window_and_capture(before_count)
        else:
            runner.run_target_action(op)
            snap = runner.settle_and_capture()

        if op == "open_window" and snap["window_count"] <= before_count:
            raise RuntimeError(
                f"open_window at step {i} did not increase window count "
                f"({before_count} -> {snap['window_count']})"
            )

        states.append({"step": i, "op": op, "snapshot": snap})

    trace = {
        "version": 1,
        "target": target,
        "workspace": workspace,
        "scenario": {
            "seed": scenario.get("seed"),
            "initial_windows": initial_windows,
            "operations": operations,
        },
        "states": states,
    }
    output.write_text(json.dumps(trace, indent=2) + "\n", encoding="utf-8")
    return trace


def trace_key_snapshot(snapshot: dict[str, Any], mode: str) -> dict[str, Any]:
    def clear_tree_focus(node: Any) -> Any:
        if not isinstance(node, dict):
            return node
        out = dict(node)
        if "focused" in out:
            out["focused"] = False
        children = out.get("children")
        if isinstance(children, list):
            out["children"] = [clear_tree_focus(child) for child in children]
        return out

    # Tiling tree structure is the parity target; per-node "focused" flags can
    # differ across compositors and IPC normalization, so compare focus via
    # canonical focused_path/focused_is_floating instead.
    focused_is_floating = bool(snapshot.get("focused_is_floating"))
    normalized_focused_path = [] if focused_is_floating else snapshot.get("focused_path", [])
    normalized_tiling_tree = clear_tree_focus(snapshot.get("tiling_tree"))

    if mode == "tiling-only":
        return {
            "tiling_tree": normalized_tiling_tree,
            "focused_path": normalized_focused_path,
            "tiling_count": snapshot.get("tiling_count"),
        }
    return {
        "tiling_tree": normalized_tiling_tree,
        "focused_path": normalized_focused_path,
        "tiling_count": snapshot.get("tiling_count"),
        "floating_count": snapshot.get("floating_count"),
        "window_count": snapshot.get("window_count"),
        "focused_is_floating": focused_is_floating,
    }


def json_diff(left: Any, right: Any, left_name: str, right_name: str) -> str:
    left_s = json.dumps(left, indent=2, sort_keys=True).splitlines(keepends=True)
    right_s = json.dumps(right, indent=2, sort_keys=True).splitlines(keepends=True)
    return "".join(difflib.unified_diff(left_s, right_s, fromfile=left_name, tofile=right_name))


def collect_trace_mismatches(
    left: dict[str, Any], right: dict[str, Any], mode: str
) -> tuple[list[dict[str, Any]], str | None]:
    left_states = left.get("states", [])
    right_states = right.get("states", [])

    if not isinstance(left_states, list) or not isinstance(right_states, list):
        return [], "invalid trace format: 'states' must be a list"

    if not left_states or not right_states:
        return [], "empty traces: did you pass scenario JSON instead of trace JSON?"

    if len(left_states) != len(right_states):
        return (
            [],
            f"state count mismatch: left={len(left_states)} right={len(right_states)}",
        )

    left_target = left.get("target")
    right_target = right.get("target")

    def floating_focus_known(target: Any, snapshot: dict[str, Any]) -> bool:
        explicit_known = snapshot.get("focused_is_floating_known")
        if explicit_known is not None:
            return bool(explicit_known)

        if target == "sway":
            raw_mode = snapshot.get("sway_raw_focus_mode")
            if isinstance(raw_mode, str):
                return raw_mode in {"tiling", "floating"}

            raw_tiling_known = snapshot.get("sway_raw_tiling_focus_known")
            if raw_tiling_known is not None:
                # Old trace format: sway could not resolve focus mode reliably
                # when tiling focus chain was unknown.
                return bool(raw_tiling_known)

        # Keep previous behavior for traces without explicit metadata.
        return True

    mismatches: list[dict[str, Any]] = []
    for i, (ls, rs) in enumerate(zip(left_states, right_states)):
        lop = ls.get("op")
        rop = rs.get("op")
        if lop != rop:
            mismatches.append(
                {
                    "step": i,
                    "op": None,
                    "reason": "operation mismatch",
                    "details": {"left_op": lop, "right_op": rop},
                }
            )
            continue

        if "snapshot" not in ls or "snapshot" not in rs:
            mismatches.append(
                {
                    "step": i,
                    "op": lop,
                    "reason": "missing snapshot",
                    "details": None,
                }
            )
            continue

        lk = trace_key_snapshot(ls["snapshot"], mode)
        rk = trace_key_snapshot(rs["snapshot"], mode)

        # Some sway states expose no focused marker in the tiling tree even when
        # focus-affecting commands are valid. Treat focused path as unknown for
        # that step (on both sides) so comparison stays anchored to observable
        # structure and counts.
        left_leaf_focus_known = ls["snapshot"].get("tiling_leaf_focus_known")
        if left_leaf_focus_known is None:
            left_leaf_focus_known = tree_has_leaf_focus_marker(ls["snapshot"].get("tiling_tree"))
        right_leaf_focus_known = rs["snapshot"].get("tiling_leaf_focus_known")
        if right_leaf_focus_known is None:
            right_leaf_focus_known = tree_has_leaf_focus_marker(rs["snapshot"].get("tiling_tree"))
        left_leaf_focus_known = bool(left_leaf_focus_known)
        right_leaf_focus_known = bool(right_leaf_focus_known)
        leaf_focus_ambiguous = not left_leaf_focus_known or not right_leaf_focus_known
        if leaf_focus_ambiguous:
            lk["focused_path"] = []
            rk["focused_path"] = []
            if "focused_is_floating" in lk and "focused_is_floating" in rk:
                # When leaf focus is unknown on either side, focus mode can be ambiguous in sway
                # (e.g. focused container without a focused leaf marker). Ignore this bit here.
                lk["focused_is_floating"] = False
                rk["focused_is_floating"] = False

        left_floating_focus_known = floating_focus_known(left_target, ls["snapshot"])
        right_floating_focus_known = floating_focus_known(right_target, rs["snapshot"])
        if not left_floating_focus_known or not right_floating_focus_known:
            if "focused_is_floating" in lk and "focused_is_floating" in rk:
                lk["focused_is_floating"] = False
                rk["focused_is_floating"] = False

        if lk != rk:
            mismatches.append(
                {
                    "step": i,
                    "op": lop,
                    "reason": f"snapshot mismatch (mode={mode})",
                    "details": {
                        "left": lk,
                        "right": rk,
                        "diff": json_diff(lk, rk, "left", "right"),
                    },
                }
            )

    return mismatches, None


def compare_traces(left: dict[str, Any], right: dict[str, Any], mode: str) -> int:
    mismatches, fatal_error = collect_trace_mismatches(left, right, mode)
    if fatal_error is not None:
        print(f"FAIL: {fatal_error}", file=sys.stderr)
        return 1

    if mismatches:
        for mismatch in mismatches:
            step = mismatch["step"]
            op = mismatch["op"]
            reason = mismatch["reason"]
            if op is None:
                print(f"FAIL step {step}: {reason}", file=sys.stderr)
            else:
                print(f"FAIL step {step} op={op}: {reason}", file=sys.stderr)

            details = mismatch.get("details")
            if isinstance(details, dict) and "diff" in details:
                print(details["diff"], file=sys.stderr)
            elif details is not None:
                print(json.dumps(details, indent=2, sort_keys=True), file=sys.stderr)

        print(f"Found {len(mismatches)} mismatching step(s).", file=sys.stderr)
        return 1

    print(f"PASS: traces match ({len(left.get('states', []))} states, mode={mode})")
    return 0


def run_campaign(
    start_seed: int,
    count: int,
    steps: int,
    initial_windows: int,
    window_cmd: str,
    output_dir: Path,
    mode: str,
    workspace_prefix: str,
    settle_timeout: float,
    settle_interval: float,
    open_window_timeout: float,
    open_window_retries: int,
    stop_on_first_fail: bool,
    sway_socket: str | None,
    tiri_socket: str | None,
    swaymsg_cmd: str,
    tiri_cmd: str,
) -> int:
    output_dir.mkdir(parents=True, exist_ok=True)
    seeds = list(range(start_seed, start_seed + count))

    summary: dict[str, Any] = {
        "version": 1,
        "mode": mode,
        "params": {
            "start_seed": start_seed,
            "count": count,
            "steps": steps,
            "initial_windows": initial_windows,
            "workspace_prefix": workspace_prefix,
            "sway_socket": sway_socket,
            "tiri_socket": tiri_socket,
            "swaymsg_cmd": swaymsg_cmd,
            "tiri_cmd": tiri_cmd,
            "settle_timeout": settle_timeout,
            "settle_interval": settle_interval,
            "open_window_timeout": open_window_timeout,
            "open_window_retries": open_window_retries,
        },
        "results": [],
    }

    try:
        Runner(
            target="sway",
            window_cmd=window_cmd,
            settle_timeout=settle_timeout,
            settle_interval=settle_interval,
            open_window_timeout=open_window_timeout,
            open_window_retries=open_window_retries,
            sway_socket=sway_socket,
            tiri_socket=tiri_socket,
            swaymsg_cmd=swaymsg_cmd,
            tiri_cmd=tiri_cmd,
        ).preflight()
        Runner(
            target="tiri",
            window_cmd=window_cmd,
            settle_timeout=settle_timeout,
            settle_interval=settle_interval,
            open_window_timeout=open_window_timeout,
            open_window_retries=open_window_retries,
            sway_socket=sway_socket,
            tiri_socket=tiri_socket,
            swaymsg_cmd=swaymsg_cmd,
            tiri_cmd=tiri_cmd,
        ).preflight()
    except Exception as e:  # noqa: BLE001
        summary["totals"] = {
            "seeds_requested": len(seeds),
            "seeds_executed": 0,
            "passes": 0,
            "failures": 1,
        }
        summary["preflight_error"] = str(e)
        summary_path = output_dir / "summary.json"
        summary_path.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
        print(f"ERROR: {e}")
        print(f"Campaign aborted. Summary written to: {summary_path}")
        return 1

    passes = 0
    failures = 0

    for idx, seed in enumerate(seeds, start=1):
        print(f"[{idx}/{len(seeds)}] seed={seed} ...")

        if not sway_has_connected_outputs(sway_socket, swaymsg_cmd):
            msg = (
                "sway lost all active outputs during campaign. "
                "Aborting to avoid repeated per-seed failures."
            )
            summary["results"].append({"seed": seed, "status": "error", "error": msg})
            failures += 1
            print(f"  ERROR: {msg}")
            break

        scenario = generate_scenario(seed=seed, steps=steps, initial_windows=initial_windows)
        scenario_path = output_dir / f"seed-{seed}.scenario.json"
        sway_trace_path = output_dir / f"seed-{seed}.sway.trace.json"
        tiri_trace_path = output_dir / f"seed-{seed}.tiri.trace.json"
        scenario_path.write_text(json.dumps(scenario, indent=2) + "\n", encoding="utf-8")

        entry: dict[str, Any] = {
            "seed": seed,
            "scenario": str(scenario_path),
            "sway_trace": str(sway_trace_path),
            "tiri_trace": str(tiri_trace_path),
        }

        try:
            sway_trace = run_scenario(
                target="sway",
                scenario=scenario,
                output=sway_trace_path,
                window_cmd=window_cmd,
                workspace_override=f"{workspace_prefix}_{seed}_sway",
                settle_timeout=settle_timeout,
                settle_interval=settle_interval,
                open_window_timeout=open_window_timeout,
                open_window_retries=open_window_retries,
                sway_socket=sway_socket,
                tiri_socket=tiri_socket,
                swaymsg_cmd=swaymsg_cmd,
                tiri_cmd=tiri_cmd,
            )
            tiri_trace = run_scenario(
                target="tiri",
                scenario=scenario,
                output=tiri_trace_path,
                window_cmd=window_cmd,
                workspace_override=f"{workspace_prefix}_{seed}_tiri",
                settle_timeout=settle_timeout,
                settle_interval=settle_interval,
                open_window_timeout=open_window_timeout,
                open_window_retries=open_window_retries,
                sway_socket=sway_socket,
                tiri_socket=tiri_socket,
                swaymsg_cmd=swaymsg_cmd,
                tiri_cmd=tiri_cmd,
            )
        except Exception as e:  # noqa: BLE001
            failures += 1
            entry["status"] = "error"
            entry["error"] = str(e)
            summary["results"].append(entry)
            print(f"  ERROR: {e}")
            if stop_on_first_fail:
                break
            continue

        mismatches, fatal_error = collect_trace_mismatches(sway_trace, tiri_trace, mode)
        if fatal_error is not None:
            failures += 1
            entry["status"] = "error"
            entry["error"] = fatal_error
            summary["results"].append(entry)
            print(f"  ERROR: {fatal_error}")
            if stop_on_first_fail:
                break
            continue

        if mismatches:
            failures += 1
            first = mismatches[0]
            entry["status"] = "mismatch"
            entry["mismatch_count"] = len(mismatches)
            entry["first_mismatch"] = {
                "step": first["step"],
                "op": first["op"],
                "reason": first["reason"],
            }
            summary["results"].append(entry)
            print(
                f"  FAIL: {len(mismatches)} mismatch(es); first at step "
                f"{first['step']} op={first['op']}"
            )
            if stop_on_first_fail:
                break
            continue

        passes += 1
        entry["status"] = "pass"
        entry["states"] = len(sway_trace.get("states", []))
        summary["results"].append(entry)
        print("  PASS")

    summary["totals"] = {
        "seeds_requested": len(seeds),
        "seeds_executed": len(summary["results"]),
        "passes": passes,
        "failures": failures,
    }
    summary_path = output_dir / "summary.json"
    summary_path.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")

    print()
    print(f"Campaign summary written to: {summary_path}")
    print(
        f"Results: passes={passes}, failures={failures}, "
        f"executed={len(summary['results'])}/{len(seeds)}"
    )
    return 0 if failures == 0 else 1


def main() -> int:
    args = parse_args()

    if args.cmd == "generate":
        scenario = generate_scenario(args.seed, args.steps, args.initial_windows)
        args.output.write_text(json.dumps(scenario, indent=2) + "\n", encoding="utf-8")
        print(
            f"Wrote scenario to {args.output} "
            f"(seed={args.seed}, steps={args.steps}, initial_windows={args.initial_windows})"
        )
        return 0

    if args.cmd == "compare":
        left = json.loads(args.left.read_text(encoding="utf-8"))
        right = json.loads(args.right.read_text(encoding="utf-8"))
        return compare_traces(left, right, args.mode)

    managed: list[ManagedCompositor] = []
    try:
        if args.cmd in {"run", "campaign"}:
            runtime_dir = args.runtime_dir or (args.output_dir / "runtime" if args.cmd == "campaign" else args.output.parent / "runtime")
            runtime_dir.mkdir(parents=True, exist_ok=True)

            if args.auto_headless_sway:
                headless_sway = start_headless_sway(
                    runtime_dir=runtime_dir,
                    sway_bin=args.sway_bin,
                    swaymsg_cmd=args.swaymsg_cmd,
                    outputs=args.headless_outputs,
                )
                managed.append(headless_sway)
                args.sway_socket = headless_sway.socket
                print(f"Auto-started headless sway: socket={args.sway_socket}")

            if args.auto_headless_tiri:
                headless_tiri = start_headless_tiri(
                    runtime_dir=runtime_dir,
                    tiri_cmd=args.tiri_cmd,
                    outputs=args.headless_outputs,
                    width=args.headless_output_width,
                    height=args.headless_output_height,
                )
                managed.append(headless_tiri)
                args.tiri_socket = headless_tiri.socket
                print(f"Auto-started headless tiri: socket={args.tiri_socket}")

        if args.cmd == "run":
            scenario = json.loads(args.scenario.read_text(encoding="utf-8"))
            run_scenario(
                target=args.target,
                scenario=scenario,
                output=args.output,
                window_cmd=args.window_cmd,
                workspace_override=args.workspace,
                settle_timeout=args.settle_timeout,
                settle_interval=args.settle_interval,
                open_window_timeout=args.open_window_timeout,
                open_window_retries=args.open_window_retries,
                sway_socket=args.sway_socket,
                tiri_socket=args.tiri_socket,
                swaymsg_cmd=args.swaymsg_cmd,
                tiri_cmd=args.tiri_cmd,
            )
            print(f"Wrote trace to {args.output} for target={args.target}")
            return 0

        if args.cmd == "campaign":
            return run_campaign(
                start_seed=args.start_seed,
                count=args.count,
                steps=args.steps,
                initial_windows=args.initial_windows,
                window_cmd=args.window_cmd,
                output_dir=args.output_dir,
                mode=args.mode,
                workspace_prefix=args.workspace_prefix,
                settle_timeout=args.settle_timeout,
                settle_interval=args.settle_interval,
                open_window_timeout=args.open_window_timeout,
                open_window_retries=args.open_window_retries,
                stop_on_first_fail=args.stop_on_first_fail,
                sway_socket=args.sway_socket,
                tiri_socket=args.tiri_socket,
                swaymsg_cmd=args.swaymsg_cmd,
                tiri_cmd=args.tiri_cmd,
            )

        raise RuntimeError(f"unknown command: {args.cmd}")
    finally:
        for proc in reversed(managed):
            proc.stop()


if __name__ == "__main__":
    # Exit codes are consumed by shell loops in campaign automation.
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        # Keep Ctrl+C behavior explicit and stable.
        try:
            os.kill(os.getpid(), signal.SIGINT)
        except Exception:  # noqa: BLE001
            pass
        sys.exit(130)
