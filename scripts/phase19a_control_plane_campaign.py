#!/usr/bin/env python3
"""Phase 19A deterministic control-plane pilot.

This harness calls Chadex's local loopback MCP endpoint directly. It does not
open, navigate, or automate ChatGPT. It is the primary Phase 19A baseline for
runtime/Runner/job/payload behavior.

Secrets are only read from Chadex's local runtime state and are never written
into artifacts or printed.
"""
from __future__ import annotations

import argparse
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid
from typing import Any
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

from benchmark_turn_economy import MeasuredClient
from benchmark_workflow import DEFAULT_DATA, parse_env_file, runtime_identity

HEARTBEAT = ROOT / "scripts/phase19a_heartbeat.py"
WORKLOAD = "scripts/phase19a_pilot_workload.py"
DEFAULT_SCENARIOS = ROOT / "benchmarks/phase19a-longhaul/scenarios.json"
DEFAULT_CONFIG = ROOT / "benchmarks/phase19a-longhaul/live-config.json"
DEFAULT_LOCK = Path.home() / "Library/Application Support/Chadex/runtime/phase19a-control-plane.lock"


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f"{path.name}.tmp-{os.getpid()}")
    tmp.write_text(
        json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    os.replace(tmp, path)


def load_json_object(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return value


def require_loopback_server_url(server_url: str) -> str:
    parsed = urlparse(server_url)
    if parsed.scheme not in {"http", "https"}:
        raise RuntimeError("Chadex runtime server URL must use http/https")
    if parsed.hostname not in {"127.0.0.1", "localhost", "::1"}:
        raise RuntimeError("control-plane baseline only permits a loopback Chadex server")
    return server_url


def load_scenarios(path: Path) -> dict[str, dict[str, Any]]:
    value = load_json_object(path)
    rows = value.get("scenarios")
    if not isinstance(rows, list):
        raise ValueError("scenario manifest must contain scenarios[]")
    result: dict[str, dict[str, Any]] = {}
    for row in rows:
        if not isinstance(row, dict) or not isinstance(row.get("id"), str):
            raise ValueError("each scenario requires an id")
        result[row["id"]] = row
    return result


def parse_simple_toml(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        value = value.strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in {"'", '"'}:
            value = value[1:-1]
        values[key] = value
    return values


def resolve_project_id(state_dir: Path, project_path: Path, client_id: str) -> str:
    target = project_path.expanduser().resolve()
    candidates = state_dir.glob("local-connections/*/*/*/project-registry/*.toml")
    matches: list[tuple[float, str]] = []
    for path in candidates:
        try:
            value = parse_simple_toml(path)
            registered_path = Path(str(value.get("path", ""))).expanduser().resolve()
            project_id = value.get("id")
            if registered_path == target and isinstance(project_id, str) and project_id:
                matches.append((path.stat().st_mtime, project_id))
        except (OSError, ValueError):
            continue
    if not matches:
        raise RuntimeError(f"project registry has no entry for {target}")
    _, project_id = max(matches)
    return f"agent:{client_id}:{project_id}"


def scaled_duration(value: Any, scale: float) -> float:
    seconds = float(value or 0)
    if seconds <= 0:
        return 0.0
    return max(0.05, seconds * scale)


def workload_argv(scenario: dict[str, Any], scale: float) -> tuple[list[str], int]:
    workload = scenario.get("workload")
    if workload == "real_repo_task":
        raise ValueError("real_repo_task is not a deterministic control-plane workload")
    if workload not in {"silent", "bursty", "large-log", "late-failure", "multi-stage"}:
        raise ValueError(f"unsupported workload: {workload!r}")

    args = [WORKLOAD, str(workload)]
    duration = scaled_duration(scenario.get("duration_seconds"), scale)
    if duration:
        args += ["--duration", f"{duration:.6f}"]
    output_bytes = scenario.get("output_bytes")
    if isinstance(output_bytes, int):
        args += ["--output-bytes", str(output_bytes)]
    fail_stage = scenario.get("fail_stage")
    if isinstance(fail_stage, str):
        args += ["--fail-stage", fail_stage]
    expected_exit = int(scenario.get("expected_exit_code", 0))
    return args, expected_exit


def acquire_lock(path: Path):
    path.parent.mkdir(parents=True, exist_ok=True)
    handle = path.open("a+", encoding="utf-8")
    try:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError as exc:
        handle.seek(0)
        owner = handle.read().strip() or "unknown"
        handle.close()
        raise RuntimeError(f"another control-plane campaign holds {path} ({owner})") from exc
    handle.seek(0)
    handle.truncate()
    handle.write(f"pid={os.getpid()} started_ms={time.time_ns() // 1_000_000}\n")
    handle.flush()
    return handle


def start_heartbeat(run_dir: Path, state_dir: Path, client_id: str):
    path = run_dir / "heartbeat.jsonl"
    stderr_path = run_dir / "heartbeat-stderr.log"
    stderr_handle = stderr_path.open("w", encoding="utf-8")
    process = subprocess.Popen(
        [
            sys.executable,
            str(HEARTBEAT),
            "--state-dir",
            str(state_dir),
            "--client-id",
            client_id,
            "--output",
            str(path),
            "--interval",
            "0.5",
        ],
        cwd=ROOT,
        stdout=subprocess.DEVNULL,
        stderr=stderr_handle,
        text=True,
    )
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if path.is_file() and path.stat().st_size > 0:
            return process, path, stderr_handle
        if process.poll() is not None:
            stderr_handle.close()
            raise RuntimeError(f"heartbeat exited early; see {stderr_path}")
        time.sleep(0.05)
    process.terminate()
    process.wait(timeout=5)
    stderr_handle.close()
    raise RuntimeError("heartbeat did not emit its first sample within 5 seconds")


def stop_heartbeat(process, handle) -> None:
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
    if handle is not None:
        handle.close()


def heartbeat_summary(path: Path) -> dict[str, Any]:
    rows: list[dict[str, Any]] = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            rows.append(value)
    transitions: dict[str, int] = {}
    final: dict[str, Any] = {}
    previous: dict[str, Any] = {}
    reconnects = 0
    disconnects = 0
    for row in rows:
        layers = row.get("connection_layers")
        if not isinstance(layers, dict):
            continue
        for name, item in layers.items():
            status = item.get("status") if isinstance(item, dict) else None
            if status is None:
                continue
            if name in previous and previous[name] != status:
                transitions[name] = transitions.get(name, 0) + 1
                if status in {"ready", "connected", "registered"}:
                    reconnects += 1
                else:
                    disconnects += 1
            previous[name] = status
            final[name] = status
    return {
        "samples": len(rows),
        "ok_samples": sum(1 for row in rows if row.get("ok") is True),
        "sample_errors": sum(1 for row in rows if row.get("ok") is not True),
        "layer_transitions": transitions,
        "disconnects": disconnects,
        "reconnects": reconnects,
        "final_layers": final,
    }


def trace_breakdown(trace_root: Path, trace_id: str | None) -> dict[str, Any] | None:
    if not trace_id:
        return None
    path = trace_root / trace_id / "events.jsonl"
    if not path.is_file():
        return None
    events: list[dict[str, Any]] = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            events.append(value)

    by_name: dict[str, dict[str, Any]] = {}
    for event in events:
        name = event.get("event")
        if isinstance(name, str) and name not in by_name:
            by_name[name] = event

    def wall(name: str) -> int | None:
        value = by_name.get(name, {}).get("wall_unix_ns")
        return value if isinstance(value, int) else None

    def delta(start: str, end: str) -> float | None:
        a, b = wall(start), wall(end)
        if a is None or b is None or b < a:
            return None
        return round((b - a) / 1_000_000, 3)

    runner_ids = sorted(
        {
            str(event["runner_request_id"])
            for event in events
            if isinstance(event.get("runner_request_id"), str)
        }
    )
    return {
        "server_trace_id": trace_id,
        "event_count": len(events),
        "request_to_handler_ms": delta("mcp_tool_request_received", "mcp_tool_handler_returned"),
        "auth_ms": delta("mcp_auth_validation_started", "mcp_auth_validation_finished"),
        "dispatch_ms": delta("mcp_tool_dispatch_started", "mcp_tool_dispatch_finished"),
        "serialize_to_return_ms": delta("mcp_tool_response_serialized", "mcp_tool_handler_returned"),
        "runner_request_ids": runner_ids,
        "decoded_request_bytes": by_name.get("mcp_request_decoded", {}).get("bytes"),
    }


def wait_for_terminal(
    client: MeasuredClient,
    initial: dict[str, Any],
    deadline: float,
    expected_exit: int,
) -> tuple[dict[str, Any], int]:
    if initial.get("terminal") is True:
        return initial, 0
    job_id = initial.get("job_id")
    if not isinstance(job_id, str) or not job_id:
        # Fast successful direct MCP calls are intentionally compacted to stdout
        # fields only. Treat that shape as terminal success only when success was
        # the expected outcome; expected failures must retain explicit exit data.
        if initial.get("command_completed") is True or initial.get("execution_state") in {"completed", "failed"}:
            return initial, 0
        if expected_exit == 0 and set(initial).issubset(
            {
                "stdout_tail",
                "stdout_lines",
                "stdout_truncated",
                "stderr_tail",
                "stderr_lines",
                "stderr_truncated",
            }
        ):
            return {**initial, "terminal": True, "execution_state": "completed", "exit_code": 0}, 0
        raise RuntimeError("run_process returned neither terminal result nor job_id")

    token = initial.get("observation_token")
    observe_calls = 0
    while time.monotonic() < deadline:
        item: dict[str, Any] = {"job_id": job_id}
        if isinstance(token, str) and token:
            item["after_observation_token"] = token
        remaining = max(1, int(deadline - time.monotonic()))
        output, _ = client.invoke(
            "observe_jobs",
            {"items": [item], "wait_secs": min(60, remaining)},
            expect_success=False,
        )
        observe_calls += 1
        rows = output.get("items")
        if not isinstance(rows, list) or not rows or not isinstance(rows[0], dict):
            raise RuntimeError("observe_jobs returned no item")
        current = rows[0]
        next_token = current.get("observation_token")
        if isinstance(next_token, str) and next_token:
            token = next_token
        if current.get("terminal") is True:
            return current, observe_calls
    raise TimeoutError(f"job {job_id} did not reach terminal state")


def write_summary(path: Path, rows: list[dict[str, Any]]) -> None:
    lines = [
        "# Phase 19A Control-Plane Pilot",
        "",
        "This is the deterministic primary baseline. It does not automate ChatGPT.",
        "",
        "| Scenario | Status | Total ms | MCP calls | Wire ms | Response bytes | Job handoff | Exit |",
        "|---|---|---:|---:|---:|---:|---|---:|",
    ]
    for row in rows:
        lines.append(
            "| {scenario_id} | {status} | {total_ms:.1f} | {mcp_calls} | {wire_ms:.1f} | {response_bytes} | {job_handoff} | {exit_code} |".format(
                **row
            )
        )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--scenarios", type=Path, default=DEFAULT_SCENARIOS)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--scenario-id", action="append")
    parser.add_argument("--scale", type=float, default=1.0)
    parser.add_argument("--state-dir", type=Path, default=DEFAULT_DATA)
    parser.add_argument("--lock", type=Path, default=DEFAULT_LOCK)
    args = parser.parse_args()
    if args.scale <= 0:
        parser.error("--scale must be > 0")

    config = load_json_object(args.config)
    client_id = config.get("client_id")
    trace_root_value = config.get("trace_root")
    if not isinstance(client_id, str) or not client_id:
        raise ValueError("config requires client_id")
    if not isinstance(trace_root_value, str) or not trace_root_value:
        raise ValueError("config requires trace_root")
    trace_root = Path(trace_root_value).expanduser().resolve()
    state_dir = args.state_dir.expanduser().resolve()

    scenarios = load_scenarios(args.scenarios)
    selected = args.scenario_id or [
        scenario_id
        for scenario_id, row in scenarios.items()
        if row.get("workload") != "real_repo_task"
    ]
    unknown = [scenario_id for scenario_id in selected if scenario_id not in scenarios]
    if unknown:
        raise ValueError(f"unknown scenarios: {unknown}")

    lock_handle = acquire_lock(args.lock.expanduser())
    client: MeasuredClient | None = None
    try:
        server_url, env_path, _active_project_id = runtime_identity(state_dir)
        server_url = require_loopback_server_url(server_url)
        project_path_value = config.get("project_path")
        if not isinstance(project_path_value, str) or not project_path_value:
            raise ValueError("config requires project_path")
        project_id = resolve_project_id(state_dir, Path(project_path_value), client_id)
        token = parse_env_file(env_path).get("WEBCODEX_TOKEN", "")
        if not token:
            raise RuntimeError("local Chadex bootstrap token is unavailable")
        client = MeasuredClient(server_url, token)

        # Fail closed if the active local runtime is not the Chadex source project.
        _, source_ok = client.invoke(
            "read_files",
            {
                "project": project_id,
                "items": [{"path": WORKLOAD, "start_line": 1, "limit": 1}],
                "max_result_bytes": 4096,
            },
            expect_success=False,
        )
        if not source_ok:
            raise RuntimeError("active Chadex runtime project does not contain Phase 19A workload")

        args.output_dir.mkdir(parents=True, exist_ok=True)
        rows: list[dict[str, Any]] = []

        for scenario_id in selected:
            scenario = scenarios[scenario_id]
            argv, expected_exit = workload_argv(scenario, args.scale)
            run_id = f"{scenario_id}-{uuid.uuid4().hex[:12]}"
            run_dir = args.output_dir / "runs" / run_id
            run_dir.mkdir(parents=True, exist_ok=True)
            heartbeat_process = None
            heartbeat_handle = None
            heartbeat_path = run_dir / "heartbeat.jsonl"
            first_sample = len(client.samples)
            started = time.perf_counter()
            try:
                heartbeat_process, heartbeat_path, heartbeat_handle = start_heartbeat(
                    run_dir, state_dir, client_id
                )
                duration = scaled_duration(scenario.get("duration_seconds"), args.scale)
                timeout_secs = max(30, int(duration) + 30)
                run_args: dict[str, Any] = {
                    "project": project_id,
                    "executable": "python3",
                    "args": argv,
                    "timeout_secs": timeout_secs,
                    "sync_wait_secs": 1,
                    "purpose": "test",
                }
                if expected_exit:
                    run_args["result_expectation"] = "failure"
                initial, _ = client.invoke("run_process", run_args, expect_success=False)
                terminal, observe_calls = wait_for_terminal(
                    client,
                    initial,
                    time.monotonic() + timeout_secs + 30,
                    expected_exit,
                )
            finally:
                stop_heartbeat(heartbeat_process, heartbeat_handle)

            elapsed_ms = (time.perf_counter() - started) * 1000.0
            samples = client.samples[first_sample:]
            exit_code = terminal.get("exit_code")
            status = "passed" if exit_code == expected_exit else "failed"
            job_handoff = isinstance(initial.get("job_id"), str)
            traces = [
                item
                for item in (
                    trace_breakdown(trace_root, sample.get("server_trace_id"))
                    for sample in samples
                )
                if item is not None
            ]
            timeline = {
                "run_id": run_id,
                "scenario_id": scenario_id,
                "mcp_samples": samples,
                "trace_breakdowns": traces,
            }
            metrics = {
                "run_id": run_id,
                "scenario_id": scenario_id,
                "status": status,
                "expected_exit_code": expected_exit,
                "exit_code": exit_code,
                "total_ms": round(elapsed_ms, 3),
                "mcp_calls": len(samples),
                "wire_ms": round(sum(float(sample["total_ms"]) for sample in samples), 3),
                "response_bytes": sum(int(sample["response_bytes"]) for sample in samples),
                "peak_response_bytes": max(
                    (int(sample["response_bytes"]) for sample in samples), default=0
                ),
                "job_handoff": job_handoff,
                "observe_calls": observe_calls,
                "heartbeat": heartbeat_summary(heartbeat_path),
                "trace_breakdowns_found": len(traces),
                "stdout_truncated": terminal.get("stdout_truncated"),
                "stderr_truncated": terminal.get("stderr_truncated"),
            }
            write_json(run_dir / "timeline.json", timeline)
            write_json(run_dir / "metrics.json", metrics)
            row = {
                "run_id": run_id,
                "scenario_id": scenario_id,
                "status": status,
                "total_ms": metrics["total_ms"],
                "mcp_calls": metrics["mcp_calls"],
                "wire_ms": metrics["wire_ms"],
                "response_bytes": metrics["response_bytes"],
                "job_handoff": job_handoff,
                "exit_code": exit_code,
                "metrics": str((run_dir / "metrics.json").relative_to(args.output_dir)),
                "timeline": str((run_dir / "timeline.json").relative_to(args.output_dir)),
            }
            rows.append(row)
            write_json(
                args.output_dir / "manifest.json",
                {
                    "surface": "deterministic local Chadex MCP; no ChatGPT browser automation",
                    "baseline_eligible": True,
                    "runs": rows,
                },
            )
            if status != "passed":
                raise RuntimeError(
                    f"{scenario_id} exit mismatch: expected {expected_exit}, got {exit_code}"
                )

        write_summary(args.output_dir / "summary.md", rows)
        print(
            json.dumps(
                {
                    "surface": "deterministic local Chadex MCP",
                    "baseline_eligible": True,
                    "runs": [
                        {
                            "scenario_id": row["scenario_id"],
                            "status": row["status"],
                            "total_ms": row["total_ms"],
                            "mcp_calls": row["mcp_calls"],
                            "response_bytes": row["response_bytes"],
                        }
                        for row in rows
                    ],
                },
                indent=2,
                sort_keys=True,
            )
        )
        return 0
    finally:
        if client is not None:
            client.close()
        fcntl.flock(lock_handle.fileno(), fcntl.LOCK_UN)
        lock_handle.close()


if __name__ == "__main__":
    raise SystemExit(main())
