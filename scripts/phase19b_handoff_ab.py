#!/usr/bin/env python3
"""Compare long-job handoff projections across two isolated Chadex app bundles.

This benchmark never touches the live Chadex data directory. It launches each
bundle's helper with a temporary CHADEX_DATA_DIR, forces run_process to hand off
a short-lived job, then follows the exact observe_jobs continuation to terminal.
Only timing, response sizes, field names, and terminal identity are persisted.
"""

from __future__ import annotations

import argparse
import http.client
import json
import os
from pathlib import Path
import statistics
import tempfile
import time
from typing import Any
from urllib.parse import urlparse

os.environ["PYTHONDONTWRITEBYTECODE"] = "1"

from benchmark_cold_runtime import IsolatedHelper
from benchmark_workflow import parse_env_file, percentile, runtime_identity


def compact_json_bytes(value: Any) -> bytes:
    return json.dumps(value, separators=(",", ":"), sort_keys=True).encode("utf-8")


class RawMcpClient:
    def __init__(self, server_url: str, token: str) -> None:
        parsed = urlparse(server_url)
        if parsed.scheme != "http" or parsed.hostname not in {"127.0.0.1", "localhost", "::1"}:
            raise RuntimeError("benchmark refuses a non-loopback MCP server")
        if parsed.port is None:
            raise RuntimeError("MCP server URL has no explicit port")
        self.connection = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=30)
        self.token = token
        self.counter = 0

    def invoke(self, name: str, arguments: dict[str, Any]) -> tuple[dict[str, Any], int, float]:
        self.counter += 1
        body = compact_json_bytes(
            {
                "jsonrpc": "2.0",
                "id": self.counter,
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            }
        )
        started = time.perf_counter()
        self.connection.request(
            "POST",
            "/mcp",
            body=body,
            headers={
                "Authorization": f"Bearer {self.token}",
                "Content-Type": "application/json",
                "Accept": "application/json",
            },
        )
        response = self.connection.getresponse()
        raw = response.read()
        elapsed_ms = (time.perf_counter() - started) * 1000.0
        if response.status != 200:
            raise RuntimeError(f"{name} returned HTTP {response.status}")
        payload = json.loads(raw)
        if "error" in payload:
            raise RuntimeError(f"{name} returned JSON-RPC error")
        result = payload.get("result")
        if not isinstance(result, dict):
            raise RuntimeError(f"{name} returned an invalid result")
        structured = result.get("structuredContent")
        if not isinstance(structured, dict):
            raise RuntimeError(f"{name} omitted structuredContent")
        if structured.get("success") is False:
            raise RuntimeError(f"{name} returned tool failure")
        output = structured.get("output")
        if not isinstance(output, dict):
            raise RuntimeError(f"{name} omitted structured output")
        return output, len(raw), elapsed_ms

    def close(self) -> None:
        self.connection.close()


def timing_summary(values: list[float]) -> dict[str, float]:
    return {
        "median": round(statistics.median(values), 3),
        "p95": round(percentile(values, 0.95), 3),
        "min": round(min(values), 3),
        "max": round(max(values), 3),
    }


def app_paths(app: Path) -> tuple[Path, Path]:
    contents = app.resolve() / "Contents"
    helper = contents / "Helpers" / "chadex-helper"
    resources = contents / "Resources"
    if not helper.is_file():
        raise RuntimeError(f"helper missing from {app}")
    return helper, resources


def project_tool_id(helper: IsolatedHelper, project: Path, data_dir: Path) -> tuple[RawMcpClient, str]:
    helper.request("activateProject", {"path": str(project)})
    helper.request("configureLocalSetup")
    server_url, env_file, project_id = runtime_identity(data_dir)
    token = parse_env_file(env_file).get("WEBCODEX_TOKEN", "").strip()
    if not token:
        raise RuntimeError("isolated runtime did not produce a bootstrap token")
    return RawMcpClient(server_url, token), project_id


def force_handoff(client: RawMcpClient, project_id: str) -> dict[str, Any]:
    output, raw_bytes, elapsed_ms = client.invoke(
        "run_process",
        {
            "project": project_id,
            "executable": "/usr/bin/python3",
            "args": [
                "-c",
                "import time; time.sleep(2.5); print('phase19b-handoff-ok')",
            ],
            "timeout_secs": 30,
            "sync_wait_secs": 1,
            "purpose": "test",
        },
    )
    job_id = output.get("job_id")
    continuation = output.get("continuation")
    if not isinstance(job_id, str) or not job_id:
        raise RuntimeError("run_process did not hand off to a durable job")
    if not isinstance(continuation, dict) or continuation.get("tool") != "observe_jobs":
        raise RuntimeError("run_process did not return an exact observe_jobs continuation")
    arguments = continuation.get("arguments")
    if not isinstance(arguments, dict):
        raise RuntimeError("handoff continuation omitted arguments")

    terminal, terminal_raw_bytes, terminal_ms = client.invoke("observe_jobs", arguments)
    items = terminal.get("items")
    if not isinstance(items, list) or len(items) != 1 or not isinstance(items[0], dict):
        raise RuntimeError("observe_jobs returned an invalid terminal item")
    item = items[0]
    if item.get("job_id") != job_id:
        raise RuntimeError("observe_jobs returned a different job")
    if item.get("terminal") is not True or item.get("exit_code") != 0:
        raise RuntimeError("handoff job did not reach successful terminal state")

    return {
        "handoff_response_bytes": raw_bytes,
        "handoff_output_bytes": len(compact_json_bytes(output)),
        "handoff_ms": round(elapsed_ms, 3),
        "handoff_fields": sorted(output.keys()),
        "execution_state": output.get("execution_state"),
        "job_status": output.get("job_status"),
        "continuation_tool": continuation.get("tool"),
        "continuation_wait_secs": arguments.get("wait_secs"),
        "continuation_wake_on": arguments.get("wake_on"),
        "terminal_response_bytes": terminal_raw_bytes,
        "terminal_ms": round(terminal_ms, 3),
        "terminal_exit_code": item.get("exit_code"),
        "terminal": item.get("terminal"),
    }


def force_midflight_observation(client: RawMcpClient, project_id: str) -> dict[str, Any]:
    initial, _, _ = client.invoke(
        "run_process",
        {
            "project": project_id,
            "executable": "/usr/bin/python3",
            "args": ["-c", "import time; time.sleep(4.0); print('phase19b-midflight-ok')"],
            "timeout_secs": 30,
            "sync_wait_secs": 1,
            "purpose": "test",
        },
    )
    job_id = initial.get("job_id")
    token = initial.get("observation_token")
    if not isinstance(job_id, str) or not job_id:
        raise RuntimeError("midflight run_process did not hand off")
    if not isinstance(token, str) or not token:
        continuation = initial.get("continuation")
        try:
            token = continuation["arguments"]["items"][0]["after_observation_token"]
        except (KeyError, IndexError, TypeError):
            token = None
    if not isinstance(token, str) or not token:
        raise RuntimeError("midflight handoff omitted observation token")

    first_args = {
        "items": [{"job_id": job_id, "after_observation_token": token}],
        "wait_secs": 1,
        "wake_on": "terminal",
    }
    observed, observed_raw_bytes, observed_ms = client.invoke("observe_jobs", first_args)
    items = observed.get("items")
    if not isinstance(items, list) or len(items) != 1 or not isinstance(items[0], dict):
        raise RuntimeError("midflight observe_jobs returned invalid items")
    item = items[0]
    if item.get("terminal") is True:
        raise RuntimeError("midflight workload finished before continuation could be observed")

    continuation = observed.get("continuation")
    has_continuation = isinstance(continuation, dict) and continuation.get("tool") == "observe_jobs"
    if has_continuation:
        next_args = continuation.get("arguments")
        if not isinstance(next_args, dict):
            raise RuntimeError("midflight continuation omitted arguments")
    else:
        next_token = item.get("observation_token")
        if not isinstance(next_token, str) or not next_token:
            raise RuntimeError("midflight observation omitted continuation token")
        next_args = {
            "items": [{"job_id": job_id, "after_observation_token": next_token}],
            "wait_secs": 20,
            "wake_on": "terminal",
        }

    terminal, terminal_raw_bytes, terminal_ms = client.invoke("observe_jobs", next_args)
    terminal_items = terminal.get("items")
    if (
        not isinstance(terminal_items, list)
        or len(terminal_items) != 1
        or not isinstance(terminal_items[0], dict)
    ):
        raise RuntimeError("midflight terminal observation returned invalid items")
    terminal_item = terminal_items[0]
    if terminal_item.get("job_id") != job_id:
        raise RuntimeError("midflight terminal observation returned a different job")
    if terminal_item.get("terminal") is not True or terminal_item.get("exit_code") != 0:
        raise RuntimeError("midflight job did not complete successfully")

    continuation_args = continuation.get("arguments") if has_continuation else None
    return {
        "response_bytes": observed_raw_bytes,
        "output_bytes": len(compact_json_bytes(observed)),
        "observe_ms": round(observed_ms, 3),
        "fields": sorted(observed.keys()),
        "has_exact_continuation": has_continuation,
        "continuation_tool": continuation.get("tool") if has_continuation else None,
        "continuation_wait_secs": continuation_args.get("wait_secs") if isinstance(continuation_args, dict) else None,
        "continuation_wake_on": continuation_args.get("wake_on") if isinstance(continuation_args, dict) else None,
        "terminal_response_bytes": terminal_raw_bytes,
        "terminal_ms": round(terminal_ms, 3),
        "terminal_exit_code": terminal_item.get("exit_code"),
    }


def force_late_failure(client: RawMcpClient, project_id: str) -> dict[str, Any]:
    initial, _, _ = client.invoke(
        "run_process",
        {
            "project": project_id,
            "executable": "/usr/bin/python3",
            "args": ["-c", "import time; time.sleep(2.0); raise SystemExit(23)"],
            "timeout_secs": 30,
            "sync_wait_secs": 1,
            "purpose": "test",
        },
    )
    job_id = initial.get("job_id")
    continuation = initial.get("continuation")
    if not isinstance(job_id, str) or not job_id:
        raise RuntimeError("late-failure run_process did not hand off")
    if not isinstance(continuation, dict) or continuation.get("tool") != "observe_jobs":
        raise RuntimeError("late-failure handoff omitted exact continuation")
    arguments = continuation.get("arguments")
    if not isinstance(arguments, dict):
        raise RuntimeError("late-failure continuation omitted arguments")

    terminal, response_bytes, terminal_ms = client.invoke("observe_jobs", arguments)
    items = terminal.get("items")
    if not isinstance(items, list) or len(items) != 1 or not isinstance(items[0], dict):
        raise RuntimeError("late-failure observe_jobs returned invalid items")
    item = items[0]
    if item.get("job_id") != job_id:
        raise RuntimeError("late-failure observation returned a different job")
    if item.get("terminal") is not True or item.get("exit_code") != 23:
        raise RuntimeError(
            f"late-failure exit identity changed: terminal={item.get('terminal')!r} "
            f"exit_code={item.get('exit_code')!r}"
        )
    return {
        "terminal_exit_code": item.get("exit_code"),
        "terminal": item.get("terminal"),
        "response_bytes": response_bytes,
        "terminal_ms": round(terminal_ms, 3),
    }


def run_variant(label: str, app: Path, iterations: int, root: Path) -> dict[str, Any]:
    helper_binary, resources = app_paths(app)
    data_dir = root / label / "data"
    project = root / label / "project"
    project.mkdir(parents=True)
    (project / "README.md").write_text("# Phase 19B isolated handoff benchmark\n", encoding="utf-8")

    helper = IsolatedHelper(helper_binary, resources, data_dir)
    client: RawMcpClient | None = None
    rows: list[dict[str, Any]] = []
    try:
        client, project_id = project_tool_id(helper, project, data_dir)
        for _ in range(iterations):
            rows.append(force_handoff(client, project_id))
        midflight = force_midflight_observation(client, project_id)
        late_failure = force_late_failure(client, project_id)
    finally:
        if client is not None:
            client.close()
        helper.close()

    return {
        "label": label,
        "iterations": iterations,
        "handoff_response_bytes": timing_summary([float(r["handoff_response_bytes"]) for r in rows]),
        "handoff_output_bytes": timing_summary([float(r["handoff_output_bytes"]) for r in rows]),
        "handoff_ms": timing_summary([float(r["handoff_ms"]) for r in rows]),
        "terminal_response_bytes": timing_summary([float(r["terminal_response_bytes"]) for r in rows]),
        "terminal_ms": timing_summary([float(r["terminal_ms"]) for r in rows]),
        "handoff_fields": rows[0]["handoff_fields"],
        "execution_state": rows[0]["execution_state"],
        "job_status": rows[0]["job_status"],
        "continuation_tool": rows[0]["continuation_tool"],
        "continuation_wait_secs": rows[0]["continuation_wait_secs"],
        "continuation_wake_on": rows[0]["continuation_wake_on"],
        "terminal_exit_code": rows[0]["terminal_exit_code"],
        "terminal": rows[0]["terminal"],
        "midflight": midflight,
        "late_failure": late_failure,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--after", type=Path, required=True)
    parser.add_argument("--iterations", type=int, default=3)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.iterations < 1:
        parser.error("iterations must be positive")

    with tempfile.TemporaryDirectory(prefix="chadex-phase19b-handoff-") as directory:
        root = Path(directory)
        before = run_variant("before", args.before, args.iterations, root)
        after = run_variant("after", args.after, args.iterations, root)

    report = {
        "surface": "isolated loopback MCP forced handoff",
        "before": before,
        "after": after,
        "delta": {
            "handoff_response_bytes_median": round(
                after["handoff_response_bytes"]["median"] - before["handoff_response_bytes"]["median"], 3
            ),
            "handoff_output_bytes_median": round(
                after["handoff_output_bytes"]["median"] - before["handoff_output_bytes"]["median"], 3
            ),
            "midflight_exact_continuation_before": before["midflight"]["has_exact_continuation"],
            "midflight_exact_continuation_after": after["midflight"]["has_exact_continuation"],
            "late_failure_exit_before": before["late_failure"]["terminal_exit_code"],
            "late_failure_exit_after": after["late_failure"]["terminal_exit_code"],
        },
    }
    rendered = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(rendered, encoding="utf-8")
    print(rendered, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
