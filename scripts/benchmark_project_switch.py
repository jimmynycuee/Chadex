#!/usr/bin/env python3
"""Benchmark Chadex switching to an already-authorized selected project.

Only timings are emitted. Project paths, runtime identifiers, credentials, and
helper responses are intentionally excluded from the report.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import statistics
import time

from benchmark_workflow import (
    DEFAULT_PREFS,
    Helper,
    helper_paths,
    percentile,
    selected_project_path,
)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--preferences", type=Path, default=DEFAULT_PREFS)
    parser.add_argument("--iterations", type=int, default=50)
    parser.add_argument("--warmups", type=int, default=5)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    repo_root = args.repo_root.resolve()
    project_path = selected_project_path(args.preferences)
    helper_binary, resources = helper_paths(repo_root)
    helper = Helper(helper_binary, resources)
    samples: list[float] = []
    try:
        helper.request("activateProject", {"path": str(project_path)})
        helper.request("resumeService")
        for _ in range(max(0, args.warmups)):
            helper.request("switchLocalProject", {"path": str(project_path)})
        for _ in range(max(1, args.iterations)):
            started = time.perf_counter()
            helper.request("switchLocalProject", {"path": str(project_path)})
            samples.append((time.perf_counter() - started) * 1000.0)
    finally:
        helper.close()

    report = {
        "schema": 1,
        "iterations": len(samples),
        "same_project_switch_ms": {
            "median": round(statistics.median(samples), 3),
            "p95": round(percentile(samples, 0.95), 3),
            "min": round(min(samples), 3),
            "max": round(max(samples), 3),
        },
    }
    encoded = json.dumps(report, indent=2, sort_keys=True)
    print(encoded)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
