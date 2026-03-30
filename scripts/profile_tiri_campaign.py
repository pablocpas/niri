#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import json
import subprocess
from pathlib import Path


SCENARIOS = {
    "focus_reorder": "scripts/perf_scenarios/focus_reorder.json",
    "open_close": "scripts/perf_scenarios/open_close.json",
    "layout_mutations": "scripts/perf_scenarios/layout_mutations.json",
}


def run_campaign(args: argparse.Namespace) -> int:
    root = Path(__file__).resolve().parents[1]
    out_dir = Path(args.output_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    campaign = {"scenarios": []}

    for name, rel in SCENARIOS.items():
        scenario = root / rel
        scenario_out = out_dir / name
        cmd = [
            "python3",
            str(root / "scripts/profile_tiri_scenario.py"),
            "--scenario",
            str(scenario),
            "--window-cmd",
            args.window_cmd,
            "--output-dir",
            str(scenario_out),
            "--repeat",
            str(args.repeat),
        ]
        subprocess.run(cmd, check=True)
        campaign["scenarios"].append(
            {
                "name": name,
                "summary_json": str(scenario_out / "summary.json"),
                "summary_csv": str(scenario_out / "summary.csv"),
            }
        )

    with (out_dir / "campaign.json").open("w", encoding="utf-8") as fh:
        json.dump(campaign, fh, indent=2)

    print(out_dir / "campaign.json")
    return 0


def load_aggregate(path: Path) -> dict[str, dict[str, float]]:
    data = json.loads(path.read_text(encoding="utf-8"))
    out = {}
    for step in data["aggregate"]:
        out[step["label"]] = {
            "p50_ms": float(step["p50_ms"]),
            "p95_ms": float(step["p95_ms"]),
            "max_ms": float(step["max_ms"]),
        }
    return out


def compare_campaign(args: argparse.Namespace) -> int:
    baseline = Path(args.baseline)
    candidate = Path(args.candidate)
    regressions = []

    for name in SCENARIOS:
        base = load_aggregate(baseline / name / "summary.json")
        cand = load_aggregate(candidate / name / "summary.json")
        for label, b in base.items():
            c = cand.get(label)
            if c is None:
                continue
            abs_delta = c["p95_ms"] - b["p95_ms"]
            pct_delta = (abs_delta / b["p95_ms"] * 100.0) if b["p95_ms"] else 0.0
            if args.show_all or (
                abs_delta >= args.regression_abs_ms and pct_delta >= args.regression_pct
            ):
                regressions.append(
                    {
                        "scenario": name,
                        "label": label,
                        "baseline_p95_ms": b["p95_ms"],
                        "candidate_p95_ms": c["p95_ms"],
                        "delta_ms": abs_delta,
                        "delta_pct": pct_delta,
                    }
                )

    writer = csv.DictWriter(
        __import__("sys").stdout,
        fieldnames=[
            "scenario",
            "label",
            "baseline_p95_ms",
            "candidate_p95_ms",
            "delta_ms",
            "delta_pct",
        ],
    )
    writer.writeheader()
    writer.writerows(regressions)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)

    run = sub.add_parser("run")
    run.add_argument("--window-cmd", default="foot --app-id perf-test")
    run.add_argument("--output-dir", required=True)
    run.add_argument("--repeat", type=int, default=7)
    run.set_defaults(func=run_campaign)

    compare = sub.add_parser("compare")
    compare.add_argument("--baseline", required=True)
    compare.add_argument("--candidate", required=True)
    compare.add_argument("--regression-abs-ms", type=float, default=1.0)
    compare.add_argument("--regression-pct", type=float, default=10.0)
    compare.add_argument("--show-all", action="store_true")
    compare.set_defaults(func=compare_campaign)

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
