#!/usr/bin/env python3
"""Benchmark a representative Chadex local tool workflow end to end.

The harness starts the packaged Chadex helper, restores the selected local
runtime, then calls the local MCP endpoint directly with the bootstrap token.
Secrets, project identifiers, and tool outputs are never printed.
"""

from __future__ import annotations

import argparse
import http.client
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import time
from typing import Any
from urllib.parse import urlparse


DEFAULT_PREFS = Path.home() / "Library" / "Application Support" / "Chadex" / "preferences.json"
DEFAULT_DATA = Path.home() / "Library" / "Application Support" / "Chadex" / "runtime"


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    if not ordered:
        return 0.0
    index = max(0, min(len(ordered) - 1, int(round((len(ordered) - 1) * fraction))))
    return ordered[index]


def parse_env_file(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        values[key.strip()] = value.strip().strip('"').strip("'")
    return values


def selected_project_path(preferences_path: Path) -> Path:
    preferences = json.loads(preferences_path.read_text(encoding="utf-8"))
    selected_id = preferences.get("selectedProjectID")
    for project in preferences.get("projects", []):
        if project.get("id") == selected_id:
            return Path(project["path"]).expanduser().resolve()
    raise RuntimeError("No selected Chadex project is available for the workflow benchmark")


def first_readable_file(root: Path) -> str:
    preferred = ["README.md", "Package.swift", "Cargo.toml", ".gitignore"]
    for relative in preferred:
        candidate = root / relative
        if candidate.is_file():
            return relative
    for candidate in root.rglob("*"):
        if (
            candidate.is_file()
            and ".git" not in candidate.parts
            and candidate.stat().st_size <= 256 * 1024
        ):
            return str(candidate.relative_to(root))
    raise RuntimeError("The selected project has no small readable file")


def helper_paths(repo_root: Path) -> tuple[Path, Path]:
    app = repo_root / "dist" / "Chadex.app"
    helper = app / "Contents" / "Helpers" / "chadex-helper"
    resources = app / "Contents" / "Resources"
    if not helper.is_file():
        raise RuntimeError(
            "Packaged helper not found. Run scripts/build_app.sh before benchmarking."
        )
    return helper, resources


class Helper:
    def __init__(self, binary: Path, resources: Path) -> None:
        env = os.environ.copy()
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

    def request(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        assert self.process.stdin is not None
        assert self.process.stdout is not None
        self.counter += 1
        request_id = f"benchmark-{self.counter}"
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
            result = response.get("result")
            return result if isinstance(result, dict) else {"value": result}

    def close(self) -> None:
        if self.process.poll() is not None:
            return
        try:
            self.request("shutdown")
        finally:
            if self.process.stdin is not None:
                self.process.stdin.close()
            self.process.wait(timeout=30)


class McpClient:
    def __init__(self, server_url: str, token: str) -> None:
        parsed = urlparse(server_url)
        if parsed.scheme != "http" or parsed.hostname not in {"127.0.0.1", "localhost", "::1"}:
            raise RuntimeError("Benchmark refuses a non-loopback MCP server")
        if parsed.port is None:
            raise RuntimeError("MCP server URL has no explicit port")
        self.connection = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=30)
        self.token = token
        self.counter = 0

    def call(self, name: str, arguments: dict[str, Any]) -> float:
        self.counter += 1
        body = json.dumps(
            {
                "jsonrpc": "2.0",
                "id": self.counter,
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            },
            separators=(",", ":"),
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
        payload = json.loads(response.read())
        elapsed_ms = (time.perf_counter() - started) * 1000.0
        if response.status != 200:
            raise RuntimeError(f"{name} returned HTTP {response.status}")
        if "error" in payload:
            code = payload["error"].get("code", "unknown")
            raise RuntimeError(f"{name} returned JSON-RPC error {code}")
        result = payload.get("result", {})
        structured = result.get("structuredContent", {}) if isinstance(result, dict) else {}
        if isinstance(structured, dict) and structured.get("success") is False:
            output = structured.get("output", {})
            kind = output.get("error_kind", "tool_failure") if isinstance(output, dict) else "tool_failure"
            raise RuntimeError(f"{name} failed: {kind}")
        return elapsed_ms

    def close(self) -> None:
        self.connection.close()


def runtime_identity(data_dir: Path) -> tuple[str, Path, str]:
    state_path = data_dir / "desktop-state.json"
    state = json.loads(state_path.read_text(encoding="utf-8"))
    runtime = state.get("runtime") or {}
    server_url = runtime.get("server_url")
    env_file = runtime.get("server_env_file")
    project_id = runtime.get("runtime_project_id")
    if not server_url or not env_file or not project_id:
        raise RuntimeError("Local runtime identity is incomplete after resumeService")
    return str(server_url), Path(env_file), str(project_id)


def persistent_runtime_process_count(helper_pid: int) -> int:
    output = subprocess.check_output(["/bin/ps", "-axo", "pid=,ppid=,command="], text=True)
    rows: list[tuple[int, int, str]] = []
    for line in output.splitlines():
        parts = line.strip().split(None, 2)
        if len(parts) != 3:
            continue
        try:
            rows.append((int(parts[0]), int(parts[1]), parts[2]))
        except ValueError:
            continue

    descendants = {helper_pid}
    changed = True
    while changed:
        changed = False
        for pid, parent_pid, _ in rows:
            if parent_pid in descendants and pid not in descendants:
                descendants.add(pid)
                changed = True

    return sum(
        1
        for pid, _, command in rows
        if pid != helper_pid
        and pid in descendants
        and "webcodex" in command.lower()
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--preferences", type=Path, default=DEFAULT_PREFS)
    parser.add_argument("--data-dir", type=Path, default=DEFAULT_DATA)
    parser.add_argument("--iterations", type=int, default=20)
    parser.add_argument("--warmups", type=int, default=2)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    repo_root = args.repo_root.resolve()
    project_path = selected_project_path(args.preferences)
    read_path = first_readable_file(project_path)
    helper_binary, resources = helper_paths(repo_root)

    helper = Helper(helper_binary, resources)
    client: McpClient | None = None
    try:
        helper.request("activateProject", {"path": str(project_path)})
        helper.request("resumeService")
        server_url, env_file, runtime_project_id = runtime_identity(args.data_dir)
        env_values = parse_env_file(env_file)
        token = env_values.get("WEBCODEX_TOKEN", "").strip()
        if not token:
            raise RuntimeError("Local bootstrap token is unavailable")

        helper_status_samples: list[float] = []
        for _ in range(max(1, args.iterations)):
            started = time.perf_counter()
            helper.request("getStatus")
            helper_status_samples.append((time.perf_counter() - started) * 1000.0)

        client = McpClient(server_url, token)
        calls = [
            ("runtime_status", "runtime_status", {"summary_only": True}),
            (
                "read_files",
                "read_files",
                {
                    "project": runtime_project_id,
                    "items": [{"path": read_path, "start_line": 1, "limit": 40}],
                    "max_result_bytes": 32768,
                },
            ),
            (
                "search_project_texts",
                "search_project_texts",
                {
                    "project": runtime_project_id,
                    "queries": [
                        {
                            "pattern": "a",
                            "pattern_mode": "literal",
                            "path": read_path,
                            "limit": 20,
                        }
                    ],
                    "max_result_bytes": 32768,
                },
            ),
            (
                "git_status",
                "call_runtime_tool",
                {"tool": "git_status", "arguments": {"project": runtime_project_id}},
            ),
        ]

        for _ in range(max(0, args.warmups)):
            for _, wire_name, call_args in calls:
                client.call(wire_name, call_args)

        samples: dict[str, list[float]] = {name: [] for name, _, _ in calls}
        workflows: list[float] = []
        for _ in range(max(1, args.iterations)):
            workflow_started = time.perf_counter()
            for name, wire_name, call_args in calls:
                samples[name].append(client.call(wire_name, call_args))
            workflows.append((time.perf_counter() - workflow_started) * 1000.0)

        report = {
            "schema": 1,
            "iterations": len(workflows),
            "tool_calls_per_workflow": len(calls),
            "helper_owned_runtime_processes": persistent_runtime_process_count(
                helper.process.pid
            ),
            "helper_get_status_ms": {
                "median": round(statistics.median(helper_status_samples), 3),
                "p95": round(percentile(helper_status_samples, 0.95), 3),
            },
            "workflow_ms": {
                "median": round(statistics.median(workflows), 3),
                "p95": round(percentile(workflows, 0.95), 3),
                "min": round(min(workflows), 3),
                "max": round(max(workflows), 3),
            },
            "tools_ms": {
                name: {
                    "median": round(statistics.median(values), 3),
                    "p95": round(percentile(values, 0.95), 3),
                }
                for name, values in samples.items()
            },
        }
        encoded = json.dumps(report, indent=2, sort_keys=True)
        print(encoded)
        if args.output:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(encoded + "\n", encoding="utf-8")
        return 0
    finally:
        if client is not None:
            client.close()
        helper.close()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"benchmark failed: {error}", file=sys.stderr)
        raise SystemExit(1)
