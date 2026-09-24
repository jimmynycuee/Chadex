#!/usr/bin/env python3
"""Benchmark isolated first-runtime creation without touching live Chadex state.

Every iteration uses a temporary CHADEX_DATA_DIR plus temporary projects. The
report contains timings only: no project paths, runtime ids, tokens, or helper
responses are persisted.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import tempfile
import time
from typing import Any

from benchmark_workflow import helper_paths, percentile


class IsolatedHelper:
    def __init__(self, binary: Path, resources: Path, data_dir: Path) -> None:
        env = os.environ.copy()
        env["CHADEX_DATA_DIR"] = str(data_dir)
        env["CHADEX_RESOURCE_DIR"] = str(resources)
        self.process = subprocess.Popen(
            [str(binary)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
            env=env,
        )
        self.counter = 0

    def request(self, method: str, params: dict[str, Any] | None = None) -> Any:
        assert self.process.stdin is not None
        assert self.process.stdout is not None
        self.counter += 1
        request_id = f"cold-{self.counter}"
        payload = {
            "protocol_version": 1,
            "request_id": request_id,
            "method": method,
            "params": params or {},
        }
        self.process.stdin.write(json.dumps(payload, separators=(",", ":")) + "\n")
        self.process.stdin.flush()
        while True:
            line = self.process.stdout.readline()
            if not line:
                raise RuntimeError(f"Helper exited while waiting for {method}")
            response = json.loads(line)
            if response.get("request_id") != request_id:
                continue
            if response.get("error"):
                code = response["error"].get("code", "unknown")
                raise RuntimeError(f"{method} failed: {code}")
            return response.get("result")

    def close(self) -> None:
        if self.process.poll() is not None:
            return
        try:
            self.request("shutdown")
        finally:
            if self.process.stdin is not None:
                self.process.stdin.close()
            self.process.wait(timeout=30)


def timing_summary(values: list[float]) -> dict[str, float]:
    return {
        "median": round(statistics.median(values), 3),
        "p95": round(percentile(values, 0.95), 3),
        "min": round(min(values), 3),
        "max": round(max(values), 3),
    }


def event_timestamp(
    activities: list[dict[str, Any]],
    event_kind: str,
    source: str,
) -> int:
    for entry in activities:
        if entry.get("event_kind") == event_kind and entry.get("source") == source:
            value = entry.get("timestamp_ms")
            if isinstance(value, int):
                return value
    raise RuntimeError(f"Missing activity event {event_kind}/{source}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--iterations", type=int, default=12)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    helper_binary, resources = helper_paths(args.repo_root.resolve())
    first_runtime: list[float] = []
    new_project_extension: list[float] = []
    exact_project_return: list[float] = []
    phases: dict[str, list[float]] = {
        "operation_to_setup": [],
        "setup_to_server_spawn": [],
        "server_to_runner_spawn": [],
        "runner_to_ready": [],
        "operation_to_ready": [],
    }

    for _ in range(max(1, args.iterations)):
        root = Path(tempfile.mkdtemp(prefix="chadex-cold-runtime-"))
        project_a = root / "project-a"
        project_b = root / "project-b"
        data_dir = root / "data"
        project_a.mkdir()
        project_b.mkdir()
        (project_a / "README.md").write_text("# A\n", encoding="utf-8")
        (project_b / "README.md").write_text("# B\n", encoding="utf-8")
        helper = IsolatedHelper(helper_binary, resources, data_dir)
        try:
            helper.request("activateProject", {"path": str(project_a)})

            started = time.perf_counter()
            helper.request("configureLocalSetup")
            first_runtime.append((time.perf_counter() - started) * 1000.0)

            activities = helper.request("queryActivities", {"limit": 200})
            if not isinstance(activities, list):
                raise RuntimeError("Activity query returned an unexpected shape")
            operation = event_timestamp(activities, "operation_started", "desktop")
            setup = event_timestamp(activities, "local_setup_preparing", "desktop")
            server = event_timestamp(activities, "process_started", "service")
            runner = event_timestamp(activities, "process_started", "runner")
            ready = event_timestamp(activities, "local_runtime_ready", "desktop")
            phases["operation_to_setup"].append(float(setup - operation))
            phases["setup_to_server_spawn"].append(float(server - setup))
            phases["server_to_runner_spawn"].append(float(runner - server))
            phases["runner_to_ready"].append(float(ready - runner))
            phases["operation_to_ready"].append(float(ready - operation))

            started = time.perf_counter()
            helper.request("switchLocalProject", {"path": str(project_b)})
            new_project_extension.append((time.perf_counter() - started) * 1000.0)

            started = time.perf_counter()
            helper.request("switchLocalProject", {"path": str(project_a)})
            exact_project_return.append((time.perf_counter() - started) * 1000.0)
        finally:
            try:
                helper.close()
            finally:
                if helper.process.poll() is None:
                    helper.process.kill()
                shutil.rmtree(root, ignore_errors=True)

    report = {
        "schema": 1,
        "iterations": len(first_runtime),
        "first_runtime_ms": timing_summary(first_runtime),
        "new_project_extension_ms": timing_summary(new_project_extension),
        "exact_project_return_ms": timing_summary(exact_project_return),
        "first_runtime_phases_ms": {
            name: timing_summary(values) for name, values in phases.items()
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
