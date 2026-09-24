#!/usr/bin/env python3
"""Benchmark Chadex runtime restore/status orchestration.

This intentionally measures only helper protocol timings. It never prints
project identifiers, paths, token contents, or tool output.
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


def timed_request(helper: Helper, method: str) -> float:
    started = time.perf_counter()
    helper.request(method)
    return (time.perf_counter() - started) * 1000.0


def summary(values: list[float]) -> dict[str, float]:
    return {
        "median": round(statistics.median(values), 3),
        "p95": round(percentile(values, 0.95), 3),
        "min": round(min(values), 3),
        "max": round(max(values), 3),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--preferences", type=Path, default=DEFAULT_PREFS)
    parser.add_argument("--cold-iterations", type=int, default=20)
    parser.add_argument("--refresh-iterations", type=int, default=30)
    parser.add_argument("--refresh-warmups", type=int, default=3)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    repo_root = args.repo_root.resolve()
    project_path = selected_project_path(args.preferences)
    helper_binary, resources = helper_paths(repo_root)

    cold_samples: list[float] = []
    for _ in range(max(1, args.cold_iterations)):
        helper = Helper(helper_binary, resources)
        try:
            helper.request("activateProject", {"path": str(project_path)})
            cold_samples.append(timed_request(helper, "resumeService"))
        finally:
            helper.close()

    helper = Helper(helper_binary, resources)
    try:
        helper.request("activateProject", {"path": str(project_path)})
        helper.request("resumeService")
        for _ in range(max(0, args.refresh_warmups)):
            helper.request("refreshRuntime")
        refresh_samples = [
            timed_request(helper, "refreshRuntime")
            for _ in range(max(1, args.refresh_iterations))
        ]
    finally:
        helper.close()

    report = {
        "schema": 1,
        "cold_resume_iterations": len(cold_samples),
        "refresh_iterations": len(refresh_samples),
        "cold_resume_ms": summary(cold_samples),
        "refresh_runtime_ms": summary(refresh_samples),
    }
    encoded = json.dumps(report, indent=2, sort_keys=True)
    print(encoded)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
