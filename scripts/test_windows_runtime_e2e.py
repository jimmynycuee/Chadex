#!/usr/bin/env python3
"""Deterministic W2 harness contract tests; never mock or simulate the runtime E2E."""
from __future__ import annotations

from contextlib import contextmanager
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
import windows_runtime_e2e as harness


@contextmanager
def cargo_target(value):
    previous = os.environ.get("CARGO_TARGET_DIR")
    try:
        if value is None:
            os.environ.pop("CARGO_TARGET_DIR", None)
        else:
            os.environ["CARGO_TARGET_DIR"] = str(value)
        yield
    finally:
        if previous is None:
            os.environ.pop("CARGO_TARGET_DIR", None)
        else:
            os.environ["CARGO_TARGET_DIR"] = previous


class HarnessContracts(unittest.TestCase):
    def frame(self, output=None, success=True, **result_fields):
        return json.dumps({"jsonrpc": "2.0", "id": 1, "result": {
            "structuredContent": {"success": success, "output": output or {}},
            **result_fields}}).encode("utf-8")

    def test_mcp_preserves_real_nonzero_result(self):
        output = {"exit_code": 7, "command_completed": True, "tool_failure": False,
                  "stdout_tail": "OUT", "stderr_tail": "ERR"}
        decoded, success = harness.decode_mcp(self.frame(output, False, isError=True), 200, 1)
        harness.check_terminal(decoded, success, exit_code=7, stdout="OUT", stderr="ERR")
        with self.assertRaises(harness.E2EFailure):
            harness.check_terminal(decoded, success)
        with self.assertRaises(harness.E2EFailure):
            harness.check_terminal({**decoded, "tool_failure": True}, success, exit_code=7)

    def test_mcp_rejects_malformed_mismatched_and_oversized_frames(self):
        for raw, status, request_id in [(b"bad secret", 200, 1), (self.frame(), 401, 1),
                                        (self.frame(), 200, 2), (b"x" * (harness.MAX_FRAME + 1), 200, 1),
                                        (b'{"id":1,"result":{}}', 200, 1)]:
            with self.subTest(status=status, request_id=request_id, length=len(raw)):
                with self.assertRaises(harness.E2EFailure):
                    harness.decode_mcp(raw, status, request_id)

    def test_partial_batch_failure_and_is_error_never_pass(self):
        for raw in (self.frame({"failed_count": 1}), self.frame(isError=True), self.frame(success=False)):
            self.assertFalse(harness.decode_mcp(raw, 200, 1)[1])

    def test_adaptive_runtime_routes_direct_and_long_tail_tools(self):
        arguments = {"summary_only": True}
        self.assertEqual(harness.adaptive_mcp_call("runtime_status", arguments),
                         ("runtime_status", arguments))
        self.assertEqual(harness.adaptive_mcp_call("work_on_project", {"instruction": "W2"}),
                         ("work_on_project", {"instruction": "W2"}))
        self.assertEqual(harness.adaptive_mcp_call("list_projects", {}),
                         ("call_runtime_tool", {"tool": "list_projects", "arguments": {}}))
        self.assertEqual(harness.adaptive_mcp_call("open_session_shell", {"project": "p"}),
                         ("call_runtime_tool", {"tool": "open_session_shell",
                                                "arguments": {"project": "p"}}))

    def test_job_status_vocabulary_matches_runner_lifecycle_and_recovery_overlay(self):
        self.assertEqual(
            harness.ACTIVE_STATUSES,
            {"queued", "agent_queued", "started", "running", "stop_requested", "recovering"},
        )
        self.assertEqual(
            harness.TERMINAL_STATUSES,
            {"completed", "failed", "stopped", "cancelled", "timeout", "timed_out", "lost"},
        )
        for status in harness.ACTIVE_STATUSES | harness.TERMINAL_STATUSES:
            output = {
                "job_id": "job-1", "status": status,
                "terminal": status in harness.TERMINAL_STATUSES,
                "observation_token": "token-1", "wait_outcome": "immediate", "waited_ms": 0,
            }
            evidence = harness.observation_evidence(output, "job-1")
            self.assertEqual(evidence["status"], status)

    def test_observe_jobs_sparse_and_full_items_normalize_without_identity_loss(self):
        base = {"job_id": "job-1", "status": "running", "terminal": False,
                "observation_token": "token-1", "stdout_tail": "hello"}
        sparse = harness.decode_single_observation(
            {"items": [base], "wait": {"outcome": "timeout", "waited_ms": 2000}}, "job-1")
        self.assertEqual(sparse["wait_outcome"], "timeout")
        self.assertEqual(sparse["waited_ms"], 2000)
        full = harness.decode_single_observation(
            {"items": [{"success": True, "output": {**base, "terminal": True, "status": "completed"}}],
             "wait": {"outcome": "terminal"}}, "job-1")
        self.assertTrue(full["terminal"])
        self.assertEqual(full["waited_ms"], 0)
        with self.assertRaises(harness.E2EFailure):
            harness.decode_single_observation(
                {"items": [{"success": False, "output": None}], "wait": {"outcome": "item_error"}},
                "job-1")

    def test_compact_terminal_success_and_handoff_are_distinct(self):
        harness.check_terminal({"stdout_tail": "OK"}, True, stdout="OK")
        for output in ({"job_id": "job"}, {"exit_code": 8}, {"command_completed": False}):
            with self.assertRaises(harness.E2EFailure):
                harness.check_terminal(output, True)

    def test_observation_requires_same_job_and_keeps_opaque_token_private(self):
        output = {"job_id": "PRIVATE_JOB", "observation_token": "PRIVATE_TOKEN",
                  "wait_outcome": "timeout", "terminal": False, "status": "running", "waited_ms": 2000,
                  "stdout_tail": "PRIVATE_LOG", "credentials": "PRIVATE_SECRET"}
        evidence = harness.observation_evidence(output, "PRIVATE_JOB")
        self.assertEqual(evidence["wait_outcome"], "timeout")
        self.assertEqual(evidence["waited_ms"], 2000)
        self.assertNotIn("PRIVATE", json.dumps(evidence))
        for changes in ({"job_id": "other"}, {"observation_token": ""},
                        {"wait_outcome": "secret"}, {"status": "unknown"}, {"terminal": None}):
            with self.assertRaises(harness.E2EFailure):
                harness.observation_evidence({**output, **changes}, "PRIVATE_JOB")

    def test_failure_report_never_includes_raw_exception_or_rpc_error(self):
        report = harness.Report()
        with self.assertRaises(ValueError):
            with report.stage("preflight"):
                raise ValueError("Bearer PRIVATE_SECRET C:\\private\\path")
        report.finish()
        self.assertFalse(report.value["passed"])
        self.assertNotIn("PRIVATE_SECRET", json.dumps(report.value))
        raw = b'{"id":1,"error":{"code":-32000,"message":"PRIVATE_SECRET","data":"PRIVATE_SECRET"}}'
        with self.assertRaises(harness.E2EFailure) as caught:
            harness.decode_mcp(raw, 200, 1)
        self.assertEqual(caught.exception.evidence, {"rpc_code": -32000})
        self.assertEqual(harness.safe_failure("PRIVATE_SECRET"), "unclassified")

    def test_process_tree_and_creation_time_fence(self):
        rows = {10: {"ParentProcessId": 1, "Created": "first"},
                11: {"ParentProcessId": 10, "Created": "child"},
                12: {"ParentProcessId": 11, "Created": "grandchild"},
                20: {"ParentProcessId": 1, "Created": "unrelated"}}
        self.assertEqual(harness.descendants(rows, {10}), {10, 11, 12})
        self.assertTrue(harness.same_process(rows[10], {"Created": "first"}))
        self.assertFalse(harness.same_process(rows[10], {"Created": "reused"}))
        self.assertFalse(harness.same_process(None, rows[10]))
        self.assertFalse(harness.same_process({}, {}))

    def test_powershell_literal_quote(self):
        self.assertEqual(harness.ps_quote("C:\\中文 path\\dir's"), "'C:\\中文 path\\dir''s'")

    def test_cancellation_process_uses_absolute_literal_argv(self):
        with tempfile.TemporaryDirectory(prefix="w2 cancel 中文 ") as directory:
            project = Path(directory).resolve() / "project with spaces"
            executable, args, marker = harness.cancellation_process_spec(project)
            self.assertEqual(executable, sys.executable)
            self.assertEqual(marker, project / "cancel_pids.json")
            self.assertEqual(args, ["-X", "utf8", str(project / "cancel_tree.py"), str(marker)])

    def binaries(self, directory, helper=True, runtime=True):
        directory.mkdir(parents=True, exist_ok=True)
        names = (["chadex-helper"] if helper else []) + (
            ["chadex-runtime-cli", "chadex-runtime-server", "chadex-runtime-runner"] if runtime else [])
        # Files test discovery only; never launch a fake helper/runtime.
        for name in names:
            (directory / f"{name}.exe").write_bytes(b"discovery fixture")

    def test_shared_cargo_target_precedence_and_manifest_fallback(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory).resolve()
            helper_default = repo / "rust-helper" / "target" / "debug"
            runtime_default = repo / "chadex-runtime" / "target" / "debug"
            shared = repo / "shared target"
            self.binaries(helper_default, runtime=False)
            self.binaries(runtime_default, helper=False)
            with cargo_target(None):
                self.assertEqual(harness.binary_paths(repo), (helper_default / "chadex-helper.exe", runtime_default))
            with cargo_target(shared):
                self.assertEqual(harness.binary_paths(repo), (helper_default / "chadex-helper.exe", runtime_default))
                self.binaries(shared / "debug")
                self.assertEqual(harness.binary_paths(repo), (shared / "debug" / "chadex-helper.exe", shared / "debug"))

    def test_missing_debug_binaries_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory, cargo_target(None):
            repo = Path(directory)
            with self.assertRaises(harness.E2EFailure) as caught:
                harness.binary_paths(repo)
            self.assertEqual(caught.exception.code, "missing_debug_helper")
            self.binaries(repo / "rust-helper" / "target" / "debug", runtime=False)
            with self.assertRaises(harness.E2EFailure) as caught:
                harness.binary_paths(repo)
            self.assertEqual(caught.exception.code, "missing_debug_runtime")

    def test_fixture_contains_real_long_job_and_required_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory) / "專案 with spaces"
            harness.write_fixture(project)
            self.assertIn("time.sleep(55)", (project / "durable.py").read_text(encoding="utf-8"))
            for script in project.glob("*.py"):
                compile(script.read_text(encoding="utf-8"), str(script), "exec")
            result = subprocess.run([sys.executable, "-B", "-m", "unittest", "discover", "-p", "test_value.py"],
                                    cwd=project, capture_output=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"AssertionError", result.stderr)

    def test_cli_produces_failure_json_and_never_claims_native_pass(self):
        with tempfile.TemporaryDirectory() as directory, cargo_target(None):
            output = Path(directory) / "result.json"
            result = subprocess.run([sys.executable, "-B", str(Path(harness.__file__).resolve()),
                                     "--repo-root", directory, "--output", str(output)],
                                    capture_output=True, timeout=10)
            report = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(json.loads(result.stdout), report)
            self.assertFalse(report["passed"])
            self.assertFalse(report["default_data_path_verified"])
            if sys.platform == "win32":
                self.assertEqual(result.returncode, 1)
                self.assertEqual(report["stages"][0]["error_code"], "missing_debug_helper")
            else:
                self.assertEqual(result.returncode, 2)
                self.assertEqual(report["status"], "unsupported")
                self.assertEqual(report["stages"][0]["error_code"], "windows_native_required")
            self.assertEqual(result.stderr, b"")


if __name__ == "__main__":
    unittest.main()
