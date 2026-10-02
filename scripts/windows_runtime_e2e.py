#!/usr/bin/env python3
"""W2 Windows-native helper -> server -> runner -> project runtime E2E.

Build debug binaries first (no build, Git mutation, tunnel or model in this script):
  cargo build --locked --manifest-path rust-helper/Cargo.toml
  cargo build --locked --manifest-path chadex-runtime/Cargo.toml --bins
  python scripts/windows_runtime_e2e.py --output <json-path>

Uses only the standard library and real binaries. All state and fixtures live in
an owned temporary directory. Explicit isolation overrides are reported; this
does NOT prove the desktop's default data/resource paths (test those separately).
Failures and non-Windows rejection still produce bounded, secret-free JSON.
No response bodies, commands, paths, credentials or raw exception text are logged.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import hashlib
import http.client
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from typing import Any, Iterator
from urllib.parse import urlsplit

sys.dont_write_bytecode = True
MAX_FRAME = 1024 * 1024
DURABLE_SECONDS = 55
WAIT_OUTCOMES = {"immediate", "updated", "terminal", "timeout"}
TERMINAL_STATUSES = {"completed", "failed", "stopped", "cancelled", "timed_out", "lost"}
ADAPTIVE_DIRECT_TOOLS = frozenset({
    "runtime_status", "work_on_project", "read_files", "apply_text_edits",
    "run_process", "run_shell", "observe_jobs",
})
SAFE_FAILURES = {
    "local_port_unavailable", "binary_missing", "binary_directory_missing",
    "binary_version_mismatch", "binary_version_unverifiable", "bundled_runtime_missing",
    "command_exit_nonzero", "timeout", "outcome_unknown", "capability_unavailable",
    "unsupported_resource", "unsupported_executable_type", "spawn_failed",
    "permission_denied", "invalid_arguments", "agent_offline", "session_guard_denied",
    "session_closed", "runtime_error", "tool_failure",
}
STAGES = (
    "preflight", "helper_runtime_startup", "registration_readiness", "file_roundtrip",
    "powershell", "native_exe", "cmd_bat", "persistent_shell", "durable_job",
    "terminal_edit_validation", "cancellation_child_cleanup", "helper_shutdown",
    "fixture_cleanup",
)


class E2EFailure(RuntimeError):
    """Only harness-owned codes and allowlisted scalar evidence reach the report."""

    def __init__(self, code: str, **evidence: Any) -> None:
        super().__init__(code)
        self.code = code
        self.evidence = evidence


def require(condition: bool, code: str, **evidence: Any) -> None:
    if not condition:
        raise E2EFailure(code, **evidence)


def digest(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def ps_quote(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def decode_object(raw: bytes, code: str) -> dict[str, Any]:
    require(len(raw) <= MAX_FRAME, "response_too_large", bytes=len(raw))
    try:
        value = json.loads(raw.decode("utf-8-sig"))
    except (UnicodeError, ValueError):
        raise E2EFailure(code) from None
    require(isinstance(value, dict), code)
    return value


def safe_failure(value: Any) -> str:
    return value if isinstance(value, str) and value in SAFE_FAILURES else "unclassified"


def decode_mcp(raw: bytes, status: int, request_id: int) -> tuple[dict[str, Any], bool]:
    require(status == 200, "mcp_http_failure", http_status=status)
    payload = decode_object(raw, "mcp_invalid_json")
    require(payload.get("id") == request_id, "mcp_response_id_mismatch")
    if "error" in payload:
        error = payload["error"]
        code = error.get("code") if isinstance(error, dict) else None
        raise E2EFailure("mcp_rpc_failure", rpc_code=code if type(code) is int else None)
    result = payload.get("result")
    require(isinstance(result, dict), "mcp_missing_result")
    structured = result.get("structuredContent")
    require(isinstance(structured, dict), "mcp_missing_structured_result")
    output = structured.get("output")
    require(isinstance(output, dict), "mcp_invalid_output")
    success = structured.get("success") is True and not result.get("isError", False)
    success = success and output.get("failed_count", 0) == 0
    return output, success


class Report:
    def __init__(self) -> None:
        self.value: dict[str, Any] = {
            "schema": 1, "track": "W2", "status": "failed", "passed": False,
            "platform": "windows" if sys.platform == "win32" else "non_windows",
            "scope": "isolated real helper/server/runner local MCP; no tunnel or model",
            "default_data_path_verified": False, "default_resource_path_verified": False,
            "isolation_overrides": ["CHADEX_DATA_DIR", "CHADEX_RESOURCE_DIR",
                                    "CHADEX_RUNTIME_BIN_DIR", "PYTHONDONTWRITEBYTECODE"],
            "stages": [{"name": name, "status": "not_run"} for name in STAGES],
            "tool_calls": [], "observations": [], "binaries": {},
        }

    @contextmanager
    def stage(self, name: str) -> Iterator[dict[str, Any]]:
        entry = next(item for item in self.value["stages"] if item["name"] == name)
        entry["status"] = "running"
        started = time.monotonic()
        try:
            yield entry
        except BaseException as error:
            entry["status"] = "failed"
            if isinstance(error, E2EFailure):
                entry["error_code"] = error.code
                entry["evidence"] = error.evidence
            else:
                # Exception messages can contain credentials, URLs or tool bodies.
                entry["error_code"] = "unexpected_exception"
                entry["exception_type"] = type(error).__name__
            raise
        else:
            entry["status"] = "passed"
        finally:
            entry["elapsed_ms"] = round((time.monotonic() - started) * 1000)

    def finish(self) -> None:
        passed = all(item["status"] == "passed" for item in self.value["stages"])
        self.value["passed"] = passed
        if self.value["status"] != "unsupported":
            self.value["status"] = "passed" if passed else "failed"


class Helper:
    def __init__(self, binary: Path, runtime_dir: Path, root: Path) -> None:
        resources = root / "resources"
        resources.mkdir()
        env = os.environ.copy()
        # Prevent caller app/resource settings from selecting installed binaries.
        env.pop("WEBCODEX_DESKTOP_BIN_DIR", None)
        env.update(CHADEX_DATA_DIR=str(root / "data"), CHADEX_RESOURCE_DIR=str(resources),
                   CHADEX_RUNTIME_BIN_DIR=str(runtime_dir), PYTHONDONTWRITEBYTECODE="1")
        self.process = subprocess.Popen(
            [str(binary)], cwd=root, env=env, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        self.responses: queue.Queue[Any] = queue.Queue(maxsize=64)
        self.counter = 0
        self.stderr_bytes = 0
        self.reader_error: str | None = None
        self.threads = [threading.Thread(target=self._read, daemon=True),
                        threading.Thread(target=self._drain_stderr, daemon=True)]
        for thread in self.threads:
            thread.start()

    def _read(self) -> None:
        assert self.process.stdout is not None
        try:
            while True:
                line = self.process.stdout.readline(MAX_FRAME + 1)
                if not line:
                    self.responses.put_nowait(None)
                    return
                response = decode_object(line, "helper_invalid_ndjson")
                if "request_id" in response:
                    self.responses.put_nowait(response)
        except E2EFailure as error:
            self.reader_error = error.code
        except Exception:
            self.reader_error = "helper_reader_failure"

    def _drain_stderr(self) -> None:
        assert self.process.stderr is not None
        while True:
            chunk = self.process.stderr.read(4096)
            if not chunk:
                return
            self.stderr_bytes += len(chunk)  # Count only; never retain raw logs.

    def request(self, method: str, params: dict[str, Any] | None = None,
                timeout: float = 60) -> Any:
        assert self.process.stdin is not None
        self.counter += 1
        request_id = f"w2-{self.counter}"
        frame = json.dumps({"protocol_version": 1, "request_id": request_id,
                            "method": method, "params": params or {}},
                           ensure_ascii=False).encode("utf-8") + b"\n"
        try:
            self.process.stdin.write(frame)
            self.process.stdin.flush()
        except (OSError, ValueError):
            raise E2EFailure("helper_write_failure") from None
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.reader_error:
                raise E2EFailure(self.reader_error)
            try:
                response = self.responses.get(timeout=min(0.2, max(0.001, deadline - time.monotonic())))
            except queue.Empty:
                continue
            require(response is not None, "helper_eof", exit_code=self.process.poll())
            if response.get("request_id") != request_id:
                continue
            if response.get("error"):
                error = response["error"]
                code = error.get("code") if isinstance(error, dict) else None
                raise E2EFailure("helper_request_failed", failure_kind=safe_failure(code))
            require("result" in response, "helper_missing_result")
            return response["result"]
        raise E2EFailure("helper_request_timeout", timeout_secs=timeout)

    def close_pipes(self) -> None:
        for thread in self.threads:
            thread.join(timeout=2)
        for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
            if stream is not None:
                stream.close()


def adaptive_mcp_call(name: str, arguments: dict[str, Any]) -> tuple[str, dict[str, Any]]:
    """Route W2 calls through the current Adaptive Runtime MCP surface."""
    if name in ADAPTIVE_DIRECT_TOOLS:
        return name, arguments
    return "call_runtime_tool", {"tool": name, "arguments": arguments}


class McpClient:
    def __init__(self, url: str, token: str, report: Report) -> None:
        parsed = urlsplit(url)
        require(parsed.scheme == "http" and parsed.hostname in {"127.0.0.1", "::1", "localhost"}
                and parsed.port is not None and not parsed.username and not parsed.password,
                "mcp_requires_loopback")
        self.host, self.port = parsed.hostname, parsed.port
        self.token, self.report, self.counter = token, report, 0

    def invoke(self, name: str, arguments: dict[str, Any], *,
               expect_success: bool = True, timeout: float = 30) -> tuple[dict[str, Any], bool]:
        # One request per connection; no implicit transport retries for mutations.
        self.counter += 1
        body = json.dumps({"jsonrpc": "2.0", "id": self.counter, "method": "tools/call",
                           "params": {"name": name, "arguments": arguments}},
                          ensure_ascii=False).encode("utf-8")
        connection = http.client.HTTPConnection(self.host, self.port, timeout=timeout)
        sample: dict[str, Any] = {"tool": name, "request_bytes": len(body), "success": False}
        self.report.value["tool_calls"].append(sample)
        started = time.monotonic()
        try:
            connection.request("POST", "/mcp", body=body, headers={
                "Authorization": f"Bearer {self.token}", "Content-Type": "application/json",
                "Accept": "application/json"})
            response = connection.getresponse()
            sample["http_status"] = response.status
            raw = response.read(MAX_FRAME + 1)
            sample["response_bytes"] = len(raw)
            output, success = decode_mcp(raw, response.status, self.counter)
            sample["success"] = success
            if expect_success and not success:
                raise E2EFailure("mcp_tool_failed", failure_kind=safe_failure(
                    output.get("failure_kind", output.get("error_kind"))))
            return output, success
        except (OSError, http.client.HTTPException):
            raise E2EFailure("mcp_transport_failure") from None
        finally:
            sample["elapsed_ms"] = round((time.monotonic() - started) * 1000)
            connection.close()


def binary_paths(repo: Path) -> tuple[Path, Path]:
    shared = os.environ.get("CARGO_TARGET_DIR")
    shared_debug = [Path(shared).expanduser().resolve() / "debug"] if shared else []
    helper_candidates = shared_debug + [repo / "rust-helper" / "target" / "debug"]
    runtime_candidates = shared_debug + [repo / "chadex-runtime" / "target" / "debug",
                                         repo / "runtime-engine" / "target" / "debug"]
    helper = next((directory / "chadex-helper.exe" for directory in helper_candidates
                   if (directory / "chadex-helper.exe").is_file()), helper_candidates[0] / "chadex-helper.exe")
    names = ("chadex-runtime-cli", "chadex-runtime-server", "chadex-runtime-runner")
    runtime = next((directory for directory in runtime_candidates
                    if all((directory / f"{name}.exe").is_file() for name in names)), runtime_candidates[0])
    require(helper.is_file(), "missing_debug_helper")
    for name in names:
        require((runtime / f"{name}.exe").is_file(), "missing_debug_runtime", binary=f"{name}.exe")
    return helper, runtime


def runtime_identity(data: Path) -> tuple[str, str, str]:
    state = decode_object((data / "desktop-state.json").read_bytes(), "runtime_state_invalid")
    runtime = state.get("runtime") or {}
    url, env_file, project = (runtime.get(key) for key in
                              ("server_url", "server_env_file", "runtime_project_id"))
    require(all(isinstance(item, str) and item for item in (url, env_file, project)),
            "runtime_identity_incomplete")
    env_path = Path(env_file).resolve()
    require(env_path.is_relative_to(data.resolve()), "runtime_env_outside_isolation")
    token = None
    for line in env_path.read_text(encoding="utf-8-sig").splitlines():
        key, separator, value = line.partition("=")
        if separator and key.strip() == "WEBCODEX_TOKEN":
            token = value.strip().strip('"').strip("'")
    require(bool(token), "runtime_token_missing")
    return url, token, project


def process_inventory(powershell: str) -> dict[int, dict[str, Any]]:
    # No command line/environment fields: inventory must never expose secrets.
    script = ("[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); "
              "@(Get-CimInstance Win32_Process -ErrorAction Stop | "
              "Select-Object ProcessId,ParentProcessId,Name,"
              "@{n='Created';e={$_.CreationDate.ToUniversalTime().ToString('o')}}) | "
              "ConvertTo-Json -Compress")
    completed = subprocess.run([powershell, "-NoLogo", "-NoProfile", "-NonInteractive",
                                "-Command", script], capture_output=True, timeout=15)
    require(completed.returncode == 0, "process_inventory_failed", exit_code=completed.returncode)
    require(len(completed.stdout) <= MAX_FRAME, "process_inventory_too_large")
    try:
        rows = json.loads(completed.stdout.decode("utf-8-sig"))
    except (ValueError, UnicodeError):
        raise E2EFailure("process_inventory_invalid") from None
    require(isinstance(rows, list) and all(isinstance(row, dict) for row in rows),
            "process_inventory_invalid")
    return {int(row["ProcessId"]): row for row in rows}


def descendants(rows: dict[int, dict[str, Any]], roots: set[int]) -> set[int]:
    result = set(roots)
    while True:
        added = {pid for pid, row in rows.items() if row["ParentProcessId"] in result} - result
        if not added:
            return result & rows.keys()
        result.update(added)


def same_process(row: dict[str, Any] | None, identity: dict[str, Any]) -> bool:
    return row is not None and bool(identity.get("Created")) and row.get("Created") == identity["Created"]


def remember_tree(powershell: str, helper_pid: int, owned: dict[int, dict[str, Any]]) -> dict[int, dict[str, Any]]:
    rows = process_inventory(powershell)
    roots = {pid for pid, identity in owned.items() if same_process(rows.get(pid), identity)}
    # Only seed the helper PID while its original Popen is still alive.
    if helper_pid in rows and (not owned or helper_pid in roots):
        roots.add(helper_pid)
    for pid in descendants(rows, roots):
        require(bool(rows[pid].get("Created")), "process_creation_identity_missing")
        owned[pid] = rows[pid]
    return rows


def wait_gone(powershell: str, owned: dict[int, dict[str, Any]], timeout: float = 15) -> list[int]:
    deadline = time.monotonic() + timeout
    while True:
        rows = process_inventory(powershell)
        remaining = [pid for pid, identity in owned.items() if same_process(rows.get(pid), identity)]
        if not remaining or time.monotonic() >= deadline:
            return remaining
        time.sleep(0.2)


def force_cleanup(powershell: str, owned: dict[int, dict[str, Any]]) -> int:
    forced = 0
    for pid, identity in list(owned.items()):
        # Creation time fences PID reuse; never kill an unrelated same-number PID.
        if same_process(process_inventory(powershell).get(pid), identity):
            subprocess.run(["taskkill.exe", "/PID", str(pid), "/T", "/F"],
                           capture_output=True, timeout=15)
            forced += 1
    return forced


def check_terminal(output: dict[str, Any], success: bool, *, exit_code: int = 0,
                   stdout: str | None = None, stderr: str | None = None) -> None:
    require(not output.get("job_id"), "short_command_became_job")
    if exit_code == 0:
        # The canonical compact synchronous success omits exit_code/completion flags.
        require(success and output.get("exit_code", 0) == 0 and
                output.get("command_completed", True) is True and
                output.get("command_ok", True) is True, "command_not_successful")
    else:
        require(not success and output.get("exit_code") == exit_code and
                output.get("command_completed") is True and
                not output.get("tool_failure", False), "nonzero_exit_not_preserved")
    if stdout is not None:
        require(stdout in output.get("stdout_tail", output.get("stdout", "")), "stdout_mismatch")
    if stderr is not None:
        require(stderr in output.get("stderr_tail", output.get("stderr", "")), "stderr_mismatch")


def decode_single_observation(batch: dict[str, Any], job_id: str) -> dict[str, Any]:
    items = batch.get("items", [])
    require(len(items) == 1 and isinstance(items[0], dict), "observation_batch_invalid")
    observed = items[0]
    if "success" in observed:
        require(observed.get("success") is True and isinstance(observed.get("output"), dict),
                "observation_item_failed")
        observed = observed["output"]
    else:
        require(isinstance(observed.get("observation_token"), str), "observation_item_invalid")
    wait_info = batch.get("wait")
    require(isinstance(wait_info, dict), "observation_wait_missing")
    output = dict(observed)
    output["wait_outcome"] = wait_info.get("outcome")
    output["waited_ms"] = wait_info.get("waited_ms", 0)
    require(output.get("job_id") == job_id, "observation_job_identity_changed")
    return output


def observation_evidence(output: dict[str, Any], job_id: str) -> dict[str, Any]:
    require(output.get("job_id") == job_id, "observation_job_identity_changed")
    require(output.get("wait_outcome") in WAIT_OUTCOMES, "observation_wait_outcome_invalid")
    require(isinstance(output.get("observation_token"), str) and bool(output["observation_token"]),
            "observation_token_missing")
    require(type(output.get("terminal")) is bool, "observation_terminal_missing")
    status = output.get("status")
    require(status in TERMINAL_STATUSES | {"running", "queued", "pending", "stop_requested"},
            "observation_status_invalid")
    return {"job_id_sha256": digest(job_id), "status": status,
            "wait_outcome": output["wait_outcome"], "terminal": output["terminal"],
            "token_sha256": digest(output["observation_token"]),
            "waited_ms": output.get("waited_ms") if type(output.get("waited_ms")) is int else None}


def write_fixture(project: Path) -> None:
    project.mkdir()
    (project / "子 dir's space").mkdir()
    tools = project / "tools with spaces"
    tools.mkdir()
    (project / "value.py").write_text("def clamp(value):\n    return value\n", encoding="utf-8")
    (project / "test_value.py").write_text(
        "import unittest\nfrom value import clamp\nclass ValueTest(unittest.TestCase):\n"
        "    def test_required_clamp(self):\n        self.assertEqual(clamp(-4), 0)\n"
        "        self.assertEqual(clamp(3), 3)\n", encoding="utf-8")
    (project / "durable.py").write_text(
        "import json, os, time\nfrom pathlib import Path\n"
        "with Path('launches.ndjson').open('a', encoding='utf-8') as log:\n"
        "    log.write(json.dumps({'pid': os.getpid(), 'start_ns': time.time_ns()}) + '\\n')\n"
        "    log.flush()\nprint('DURABLE_STARTED', flush=True)\n"
        f"time.sleep({DURABLE_SECONDS})\n"
        "Path('durable_done.txt').write_bytes('完成 UTF-8\\n'.encode('utf-8'))\n"
        "print('DURABLE_FINISHED', flush=True)\n", encoding="utf-8")
    (project / "cancel_tree.py").write_text(
        "import json, os, subprocess, sys, time\nfrom pathlib import Path\n"
        "child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(180)'])\n"
        "Path('cancel_pids.json').write_text(json.dumps({'parent': os.getpid(), 'child': child.pid}))\n"
        "print('CANCEL_TREE_STARTED', flush=True)\ntime.sleep(180)\n", encoding="utf-8")
    for extension in ("cmd", "bat"):
        (tools / f"fixture.{extension}").write_bytes(
            b"@echo off\r\necho BATCH_STDOUT:%~1\r\n"
            b"echo BATCH_STDERR 1>&2\r\nexit /b %~2\r\n")


def execute(repo: Path, report: Report) -> None:
    helper: Helper | None = None
    owned: dict[int, dict[str, Any]] = {}
    root: Path | None = None
    powershell = ""
    started = time.monotonic()
    try:
        with report.stage("preflight") as evidence:
            if sys.platform != "win32":
                report.value["status"] = "unsupported"
                raise E2EFailure("windows_native_required")
            binary, runtime = binary_paths(repo)
            evidence["cargo_target_dir_configured"] = bool(os.environ.get("CARGO_TARGET_DIR"))
            powershell = shutil.which("powershell.exe") or shutil.which("pwsh.exe") or ""
            require(bool(powershell), "powershell_missing")
            require(Path(sys.executable).suffix.lower() == ".exe", "native_python_exe_required")
            for path in [binary, *(runtime / f"{name}.exe" for name in
                                    ("chadex-runtime-cli", "chadex-runtime-server", "chadex-runtime-runner"))]:
                with path.open("rb") as stream:
                    sha = hashlib.sha256()
                    for block in iter(lambda: stream.read(1024 * 1024), b""):
                        sha.update(block)
                report.value["binaries"][path.name] = {"sha256": sha.hexdigest(), "bytes": path.stat().st_size}
            root = Path(tempfile.mkdtemp(prefix="chadex W2 中文 "))
            project = root / "專案 with spaces"
            write_fixture(project)
            evidence["fixture_has_spaces_and_unicode"] = True

        with report.stage("helper_runtime_startup"):
            helper = Helper(binary, runtime, root)
            remember_tree(powershell, helper.process.pid, owned)
            helper.request("activateProject", {"path": str(project)})
            helper.request("configureLocalSetup", timeout=120)
            url, token, project_id = runtime_identity(root / "data")
            client = McpClient(url, token, report)
            report.value["project_id_sha256"] = digest(project_id)

        def call(name: str, **arguments: Any) -> dict[str, Any]:
            wire_name, wire_arguments = adaptive_mcp_call(name, arguments)
            return client.invoke(wire_name, wire_arguments)[0]

        def process(args: list[str], **options: Any) -> tuple[dict[str, Any], bool]:
            return client.invoke("run_process", {"project": project_id, "executable": sys.executable,
                                 "args": args, "timeout_secs": 20, **options},
                                 expect_success=options.get("result_expectation") != "failure")

        def read(relative: str) -> dict[str, Any]:
            output = call("read_files", project=project_id, items=[{"path": relative}],
                          include_read_revision=True, max_result_bytes=8192)
            items = output.get("items", [])
            require(len(items) == 1 and items[0].get("success") is True, "file_read_failed")
            return items[0]["output"]

        with report.stage("registration_readiness") as evidence:
            status = call("runtime_status", summary_only=True)
            require(status.get("agents", {}).get("online_count") == 1, "runner_not_ready")
            projects = call("list_projects").get("projects", [])
            require(any(item.get("id") == project_id for item in projects), "project_not_registered")
            session = call(
                "work_on_project", project=project_id, instruction="W2 native runtime validation",
                include_project_instructions=False, include_workflow_guidance=False,
                include_extension_catalog=False,
            )
            session_id = session.get("session_id")
            require(isinstance(session_id, str) and bool(session_id), "session_missing")
            rows = remember_tree(powershell, helper.process.pid, owned)
            server_pids = [pid for pid in owned if rows.get(pid, {}).get("Name") == "chadex-runtime-server.exe"]
            runner_pids = [pid for pid in owned if rows.get(pid, {}).get("Name") == "chadex-runtime-runner.exe"]
            require(len(server_pids) == len(runner_pids) == 1, "runtime_process_tree_missing")
            runtime_processes = {pid: dict(owned[pid]) for pid in server_pids + runner_pids}
            evidence.update(online_runners=1, registered_project=True, server_count=1, runner_count=1)

        with report.stage("file_roundtrip"):
            content = "W2 UTF-8 中文\n"
            written = call("write_project_file", project=project_id, path="中文 file.txt", content=content)
            require(written.get("sha256") == digest(content) and
                    written.get("bytes_written") == len(content.encode("utf-8")),
                    "file_write_result_mismatch")
            # Complete read_files results intentionally keep the digest internal
            # when a read_revision handle is requested. Validate the projected
            # text/revision, then compare exact Runner-disk UTF-8 bytes directly.
            observed = read("中文 file.txt")
            require(observed.get("text") == content.rstrip("\n") and
                    type(observed.get("read_revision")) is int, "file_utf8_roundtrip_failed")
            require((project / "中文 file.txt").read_bytes() == content.encode("utf-8"),
                    "file_write_not_on_runner_disk")

        with report.stage("powershell"):
            command = ("if (-not $PSVersionTable) { exit 91 }; "
                       "$env:CHADEX_W2_FIXTURE='ENV_中文'; "
                       "[Console]::Out.WriteLine('PS_UTF8_中文'); "
                       "[Console]::Out.WriteLine($env:CHADEX_W2_FIXTURE); "
                       "[Console]::Out.WriteLine((Get-Location).Path); "
                       "[Console]::Error.WriteLine('PS_STDERR'); exit 0")
            output, success = client.invoke("run_shell", {"project": project_id, "command": command,
                                            "cwd": "子 dir's space", "timeout_secs": 20})
            check_terminal(output, success, stdout="PS_UTF8_中文", stderr="PS_STDERR")
            require("ENV_中文" in output.get("stdout_tail", "") and
                    str(project / "子 dir's space").casefold() in output.get("stdout_tail", "").casefold(),
                    "powershell_env_cwd_mismatch")
            output, success = client.invoke("run_shell", {"project": project_id,
                "command": "[Console]::Out.WriteLine('PS_NONZERO'); [Console]::Error.WriteLine('PS_ERROR'); exit 7",
                "timeout_secs": 20, "result_expectation": "failure"}, expect_success=False)
            check_terminal(output, success, exit_code=7, stdout="PS_NONZERO", stderr="PS_ERROR")

        with report.stage("native_exe"):
            literal = "中文 spaces 'single' \"double\" & | $ ;"
            probe = "import sys; print(sys.argv[1]); print('EXE_STDERR',file=sys.stderr)"
            output, success = process(["-X", "utf8", "-c", probe, literal], cwd="子 dir's space")
            check_terminal(output, success, stdout=literal, stderr="EXE_STDERR")
            output, success = process(["-c", "import sys; print('EXE_NONZERO'); sys.exit(9)"],
                                      result_expectation="failure")
            check_terminal(output, success, exit_code=9, stdout="EXE_NONZERO")

        with report.stage("cmd_bat"):
            for extension in ("cmd", "bat"):
                for exit_code in (0, 13):
                    output, success = client.invoke("run_process", {
                        "project": project_id, "executable": str(project / "tools with spaces" / f"fixture.{extension}"),
                        "args": ["literal argument with spaces", str(exit_code)], "timeout_secs": 20,
                        "cwd": "tools with spaces",
                        "result_expectation": "failure" if exit_code else "success",
                    }, expect_success=exit_code == 0)
                    check_terminal(output, success, exit_code=exit_code,
                                   stdout="BATCH_STDOUT:literal argument with spaces", stderr="BATCH_STDERR")

        shell_identity: dict[str, Any] = {}
        with report.stage("persistent_shell"):
            opened = call("open_session_shell", project=project_id, session_id=session_id)
            shell_identity = {"project": project_id, "session_id": session_id, "shell_id": opened.get("shell_id")}
            require(bool(shell_identity["shell_id"]) and opened.get("shell") == "powershell", "persistent_shell_not_powershell")
            persistent_cwd = ps_quote(str(project / "子 dir's space"))
            first = call("session_shell_exec", **shell_identity,
                         command=f"Set-Location -LiteralPath {persistent_cwd}; "
                                 "$global:W2State='PERSIST_中文'; $env:W2State='PERSIST_ENV'; Write-Output 'PERSIST_SET'",
                         timeout_secs=20)
            require(first.get("command_completed") is True and first.get("exit_code") == 0, "persistent_first_failed")
            second = call("session_shell_exec", **shell_identity,
                          command="Write-Output $global:W2State; Write-Output $env:W2State; Write-Output (Get-Location).Path",
                          timeout_secs=20)
            require(second.get("command_completed") is True and second.get("exit_code") == 0 and
                    "PERSIST_中文" in second.get("stdout", "") and "PERSIST_ENV" in second.get("stdout", "") and
                    str(project / "子 dir's space").casefold() in second.get("stdout", "").casefold(),
                    "persistent_state_cwd_not_retained")
            status = call("session_shell_status", **shell_identity)
            require(status.get("shell_id") == opened["shell_id"] and status.get("shell_state") == "running",
                    "persistent_shell_not_running")
            closed = call("close_session_shell", **shell_identity)
            require(closed.get("shell_state") == "closed", "persistent_shell_not_closed")

        def observe(job_id: str, previous: str | None = None, wait: int = 0) -> dict[str, Any]:
            item: dict[str, Any] = {"job_id": job_id}
            arguments: dict[str, Any] = {"items": [item], "tail_lines": 20}
            if previous is not None:
                item["after_observation_token"] = previous
                if wait:
                    arguments["wait_secs"] = wait
            batch = call("observe_jobs", **arguments)
            output = decode_single_observation(batch, job_id)
            report.value["observations"].append(observation_evidence(output, job_id))
            return output

        with report.stage("durable_job") as evidence:
            job_started = time.monotonic()
            admitted, _ = process(["-X", "utf8", "durable.py"], timeout_secs=90, sync_wait_secs=1)
            job_id = admitted.get("job_id")
            require(isinstance(job_id, str) and bool(job_id), "durable_handoff_missing")
            require(time.monotonic() - job_started < 20, "durable_admission_blocked")
            seen = observe(job_id)
            saw_timeout = False
            deadline = job_started + 100
            # Resume observation of the existing identity only. Never redispatch.
            while not seen["terminal"] and time.monotonic() < deadline:
                seen = observe(job_id, seen["observation_token"], 2)
                saw_timeout |= seen["wait_outcome"] == "timeout"
                require(len(report.value["observations"]) <= 80, "observation_budget_exceeded")
            require(seen["terminal"] and seen.get("status") == "completed" and seen.get("exit_code") == 0,
                    "durable_terminal_failed")
            require(saw_timeout, "durable_observation_timeout_not_exercised")
            final = observe(job_id)
            require("DURABLE_FINISHED" in final.get("stdout_tail", ""), "durable_terminal_stdout_missing")
            launches = [json.loads(line) for line in (project / "launches.ndjson").read_text(encoding="utf-8").splitlines()]
            require(len(launches) == 1, "durable_duplicate_launch", launch_count=len(launches))
            jobs = call("list_jobs", project=project_id, limit=100).get("jobs", [])
            require(len(jobs) == 1 and jobs[0].get("job_id") == job_id, "durable_duplicate_job", job_count=len(jobs))
            elapsed = time.monotonic() - job_started
            require(45 <= elapsed <= 75, "durable_duration_outside_gate", elapsed_ms=round(elapsed * 1000))
            rows = remember_tree(powershell, helper.process.pid, owned)
            require(all(same_process(rows.get(pid), identity) for pid, identity in runtime_processes.items()),
                    "runtime_restarted_during_observation")
            observed = read("durable_done.txt")
            require(observed.get("text") == "完成 UTF-8" and
                    observed.get("sha256") == digest("完成 UTF-8\n"), "durable_file_result_missing")
            evidence.update(duration_ms=round(elapsed * 1000), launch_count=1, job_count=1,
                            observation_timeout_exercised=True, runtime_identity_preserved=True)

        with report.stage("terminal_edit_validation"):
            current = read("value.py")
            call("apply_text_edits", project=project_id, changes=[{
                "path": "value.py", "old_text": "return value", "new_text": "return max(0, value)",
                "expected_read_revision": current.get("read_revision")}])
            require("return max(0, value)" in read("value.py").get("text", ""), "terminal_edit_missing")
            output, success = process(["-B", "-m", "unittest", "discover", "-p", "test_value.py", "-v"],
                                      purpose="validation")
            check_terminal(output, success, stderr="OK")

        with report.stage("cancellation_child_cleanup") as evidence:
            command = f"& {ps_quote(sys.executable)} -X utf8 cancel_tree.py; exit $LASTEXITCODE"
            admitted = call("run_job", project=project_id, command=command, timeout_secs=120)
            cancel_id = admitted.get("job_id")
            require(isinstance(cancel_id, str) and bool(cancel_id), "cancel_job_missing")
            deadline = time.monotonic() + 20
            while not (project / "cancel_pids.json").exists() and time.monotonic() < deadline:
                observe(cancel_id)
                time.sleep(0.2)
            require((project / "cancel_pids.json").exists(), "cancel_child_never_started")
            pids = json.loads((project / "cancel_pids.json").read_text(encoding="utf-8"))
            rows = remember_tree(powershell, helper.process.pid, owned)
            require(all(type(pid) is int and pid in owned and pid in rows for pid in pids.values()),
                    "cancel_child_not_in_owned_tree")
            cancel_tree = {pid: rows[pid] for pid in descendants(rows, {pids["parent"]})}
            require(pids["child"] in cancel_tree and len(cancel_tree) >= 2, "cancel_child_missing")
            stop = call("stop_job", project=project_id, job_id=cancel_id, confirm=True)
            require(stop.get("stop_request_accepted") is True and stop.get("command_started") is False,
                    "cancellation_not_accepted")
            seen = observe(cancel_id)
            deadline = time.monotonic() + 25
            while not seen["terminal"] and time.monotonic() < deadline:
                seen = observe(cancel_id, seen["observation_token"], 2)
            require(seen["terminal"] and seen.get("status") in {"stopped", "cancelled"}, "cancellation_not_terminal")
            remaining = wait_gone(powershell, cancel_tree)
            require(not remaining, "cancellation_child_leak", remaining_count=len(remaining))
            evidence.update(observed_payload_processes=len(cancel_tree), remaining_count=0)
    finally:
        if helper is not None:
            try:
                with report.stage("helper_shutdown") as evidence:
                    remember_tree(powershell, helper.process.pid, owned)
                    helper.request("shutdown", timeout=30)
                    helper.process.stdin.close()
                    helper.process.wait(timeout=30)
                    require(helper.process.returncode == 0, "helper_shutdown_exit_nonzero",
                            exit_code=helper.process.returncode)
                    remaining = wait_gone(powershell, owned)
                    require(not remaining, "helper_shutdown_process_leak", remaining_count=len(remaining))
                    evidence.update(owned_process_count=len(owned), remaining_count=0, forced_cleanup_count=0)
            except Exception:
                # Cleanup cannot turn a failed lifecycle test into a passing one.
                entry = next(item for item in report.value["stages"] if item["name"] == "helper_shutdown")
                try:
                    entry["forced_cleanup_count"] = force_cleanup(powershell, owned)
                    if helper.process.poll() is None:
                        helper.process.kill()
                    helper.process.wait(timeout=10)
                    entry["remaining_count"] = len(wait_gone(powershell, owned))
                except Exception:
                    entry["cleanup_error"] = "forced_cleanup_failed"
            finally:
                if helper.process.poll() is not None:
                    helper.close_pipes()
                report.value["helper_stderr_bytes"] = helper.stderr_bytes
        if root is not None:
            with report.stage("fixture_cleanup"):
                shutil.rmtree(root)
        report.value["elapsed_ms"] = round((time.monotonic() - started) * 1000)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    report = Report()
    try:
        execute(args.repo_root.resolve(), report)
    except (Exception, KeyboardInterrupt):
        # Stages already record allowlisted evidence; never print a traceback/body.
        pass
    report.finish()
    encoded = json.dumps(report.value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    if args.output:
        try:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(encoded, encoding="utf-8")
        except OSError:
            report.value["passed"] = False
            report.value["status"] = "failed"
            report.value["output_error"] = "report_write_failed"
            encoded = json.dumps(report.value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    print(encoded, end="")
    return 0 if report.value["passed"] else 2 if report.value["status"] == "unsupported" else 1


if __name__ == "__main__":
    raise SystemExit(main())
