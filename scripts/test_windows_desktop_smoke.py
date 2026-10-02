#!/usr/bin/env python3
"""Deterministic tests for the Windows desktop smoke harness and resource prep."""
from __future__ import annotations

import json
from pathlib import Path, PureWindowsPath
import sys
import tempfile
import unittest
from unittest import mock

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))

import prepare_windows_desktop as prepare
import windows_desktop_smoke as smoke


class WindowsDesktopSmokeTests(unittest.TestCase):
    def test_canonical_windows_namespace_preserves_strict_isolation(self) -> None:
        for root in (r"C:\isolated\data", r"\\server\share\isolated\data"):
            canonical = smoke.windows_extended_path(root)
            self.assertEqual(smoke.windows_extended_path(canonical), canonical)
            self.assertTrue(canonical.startswith("\\\\?\\"))
            parent = PureWindowsPath(canonical)
            self.assertTrue(PureWindowsPath(canonical + r"\server.env").is_relative_to(parent))
            self.assertFalse(PureWindowsPath(canonical + r"-sibling\server.env").is_relative_to(parent))
            self.assertFalse(PureWindowsPath(r"\\?\D:\isolated\data\server.env").is_relative_to(parent))

    def test_identity_failures_remain_visible_without_exposing_runtime_data(self) -> None:
        report = smoke.new_report()
        with self.assertRaises(smoke.w2.E2EFailure):
            with report.stage("workflow.project_a_operation"):
                raise smoke.w2.E2EFailure("runtime_env_outside_isolation", token="PRIVATE_TOKEN")
        safe = smoke.sanitize_report(report)
        stage = next(item for item in safe["stages"]
                     if item["name"] == "workflow.project_a_operation")
        self.assertEqual(stage["error_code"], "runtime_env_outside_isolation")
        self.assertNotIn("PRIVATE_TOKEN", json.dumps(safe))

    def test_report_sanitizer_drops_secrets_paths_and_raw_outputs(self) -> None:
        report = smoke.new_report()
        report.value["status"] = "failed"
        report.value["stages"][0].update(
            status="failed",
            error_code="checkpoint_invalid",
            exception_type="Bearer SUPER_SECRET_TOKEN",
            evidence={"path": "/private/user/data", "token": "SUPER_SECRET_TOKEN"},
        )
        report.value["tool_calls"] = [
            {"tool": "read_files", "success": True, "output": "PRIVATE FILE CONTENT"},
            {"tool": "run_process", "success": False, "stderr": "PRIVATE STDERR"},
            {"tool": "unsafe_tool", "success": True, "arguments": "PRIVATE ARGUMENTS"},
        ]

        safe = smoke.sanitize_report(report)
        serialized = json.dumps(safe, ensure_ascii=False)
        for private_value in ("SUPER_SECRET_TOKEN", "/private/user/data", "PRIVATE FILE CONTENT",
                              "PRIVATE STDERR", "PRIVATE ARGUMENTS", "unsafe_tool"):
            self.assertNotIn(private_value, serialized)
        self.assertEqual(safe["tool_calls"], {
            "count": 1,
            "successful_tool_names": ["read_files"],
        })
        self.assertEqual(safe["manual_checks_pending"], [
            "native_picker", "explorer_open", "tray", "launch_at_login",
            "notifications", "credentialed_tunnel",
        ])

    def test_non_windows_execution_is_explicitly_unsupported(self) -> None:
        report = smoke.new_report()
        smoke.execute(Path("unused.exe"), report, platform_name="darwin")

        safe = smoke.sanitize_report(report)
        self.assertEqual(safe["status"], "unsupported")
        self.assertFalse(safe["passed"])
        self.assertEqual(safe["stages"][0]["error_code"], "windows_native_required")

    def test_failed_stage_cannot_be_reported_as_a_pass(self) -> None:
        report = smoke.new_report()
        with self.assertRaises(smoke.w2.E2EFailure):
            with report.stage("workflow.runtime_ready"):
                raise smoke.w2.E2EFailure("checkpoint_expectation_failed",
                                           token="PRIVATE_TOKEN")
        report.finish()

        safe = smoke.sanitize_report(report)
        stage = next(item for item in safe["stages"]
                     if item["name"] == "workflow.runtime_ready")
        self.assertEqual(stage["status"], "failed")
        self.assertEqual(stage["error_code"], "checkpoint_expectation_failed")
        self.assertFalse(safe["passed"])
        self.assertNotIn("PRIVATE_TOKEN", json.dumps(safe))

    def test_boolean_checkpoint_mismatch_is_visible_without_exposing_other_evidence(self) -> None:
        report = smoke.new_report()
        with self.assertRaises(smoke.w2.E2EFailure):
            with report.stage("workflow.runtime_ready"):
                smoke.validate_checkpoint({"runtime_ready": False},
                                          {"runtime_ready": True})
        stage_entry = next(item for item in report.value["stages"]
                           if item["name"] == "workflow.runtime_ready")
        stage_entry["evidence"].update({
            "path": "/private/user/data",
            "tokenhash": "PRIVATE_TOKEN_HASH",
            "arbitrary": {"payload": "PRIVATE_PAYLOAD"},
        })
        unauthorized = next(item for item in report.value["stages"]
                            if item["name"] == "force_exit.runtime_ready")
        unauthorized.update(
            status="failed",
            error_code="checkpoint_expectation_failed",
            evidence={"field": "private_field", "expected": True, "actual": False,
                      "path": "/private/other"},
        )

        safe = smoke.sanitize_report(report)
        safe_stage = next(item for item in safe["stages"]
                          if item["name"] == "workflow.runtime_ready")
        unsafe_stage = next(item for item in safe["stages"]
                            if item["name"] == "force_exit.runtime_ready")
        self.assertEqual(safe_stage["assertion"], {
            "field": "runtime_ready", "expected": True, "actual": False,
        })
        self.assertNotIn("assertion", unsafe_stage)
        serialized = json.dumps(safe)
        for private_value in ("/private/user/data", "PRIVATE_TOKEN_HASH",
                              "PRIVATE_PAYLOAD", "/private/other"):
            self.assertNotIn(private_value, serialized)

    def test_prior_failure_aborts_before_wait_and_fallback_without_masking_it(self) -> None:
        report = smoke.new_report()
        identity = {1234: {"pid": 1234}}
        with tempfile.TemporaryDirectory() as directory:
            control = Path(directory) / "control"
            control.mkdir()
            events: list[str] = []

            def wait_gone(*_args: object, **_kwargs: object) -> set[int]:
                self.assertTrue((control / "abort").is_file())
                events.append("wait")
                return {1234} if events.count("wait") == 1 else set()

            def force_cleanup(*_args: object, **_kwargs: object) -> int:
                self.assertTrue((control / "abort").is_file())
                events.append("fallback")
                return 0

            original = smoke.w2.E2EFailure("checkpoint_expectation_failed")
            with mock.patch.object(smoke.w2, "wait_gone", side_effect=wait_gone), \
                    mock.patch.object(smoke.w2, "force_cleanup", side_effect=force_cleanup):
                with self.assertRaises(smoke.w2.E2EFailure) as caught:
                    try:
                        with report.stage("workflow.runtime_ready"):
                            raise original
                    except smoke.w2.E2EFailure as prior_failure:
                        smoke.cleanup_owned(report, "workflow.cleanup", "powershell",
                                            control, identity, None, prior_failure)
                        raise

            self.assertIs(caught.exception, original)
            self.assertEqual(events, ["wait", "fallback", "wait"])
            report.finish()
            self.assertEqual(report.value["status"], "failed")
            safe_failure = next(item for item in smoke.sanitize_report(report)["stages"]
                                if item["name"] == "workflow.runtime_ready")
            self.assertEqual(safe_failure["error_code"], "checkpoint_expectation_failed")

            (control / "runtime_ready.json").write_text("{}", encoding="utf-8")
            (control / "runtime_ready.continue").write_bytes(b"continue")
            smoke.clean_control(control)
            self.assertFalse((control / "abort").exists())
            self.assertFalse((control / "runtime_ready.json").exists())
            self.assertFalse((control / "runtime_ready.continue").exists())

    def test_restore_only_requires_runtime_to_remain_offline(self) -> None:
        payload = {
            "helper_running": True,
            "runtime_ready": False,
            "verified": False,
            "rendered": True,
            "selected_expected": True,
            "preferences_restored": True,
            "helper_pid": 1234,
        }
        self.assertEqual(smoke.validate_checkpoint(payload, {
            "helper_running": True,
            "runtime_ready": False,
            "verified": False,
            "rendered": True,
            "selected_expected": True,
            "preferences_restored": True,
        }, require_helper_pid=True), 1234)
        payload["runtime_ready"] = True
        with self.assertRaises(smoke.w2.E2EFailure) as failure:
            smoke.validate_checkpoint(payload, {"runtime_ready": False})
        self.assertEqual(failure.exception.code, "checkpoint_expectation_failed")

    def test_helper_death_checkpoint_requires_credential_deletion_evidence(self) -> None:
        payload = {
            "helper_running": False,
            "runtime_ready": False,
            "verified": False,
            "rendered": True,
            "credential_deleted_after_helper_death": True,
        }
        smoke.validate_checkpoint(payload, {
            "helper_running": False,
            "runtime_ready": False,
            "verified": False,
            "rendered": True,
            "credential_deleted_after_helper_death": True,
        }, forbid_helper_pid=True)
        payload["credential_deleted_after_helper_death"] = False
        with self.assertRaises(smoke.w2.E2EFailure):
            smoke.validate_checkpoint(payload, {"credential_deleted_after_helper_death": True})

    def test_read_files_accepts_exact_content_or_text_field(self) -> None:
        self.assertEqual(smoke.project_file_content({"content": "中文 marker"}), "中文 marker")
        self.assertEqual(smoke.project_file_content({"text": "中文 marker"}), "中文 marker")
        self.assertEqual(smoke.project_file_content({
            "content": [{"type": "text", "text": "中文 "}, {"type": "text", "text": "marker"}],
        }), "中文 marker")

    def test_restore_only_checks_saved_identity_and_fixture_without_mcp(self) -> None:
        report = smoke.new_report()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project_b = root / "project B 中文"
            project_b.mkdir()
            (project_b / "smoke-result.txt").write_text(
                "W3 operation B passed", encoding="utf-8")
            with mock.patch.object(smoke.w2, "runtime_identity",
                                   return_value=("http://127.0.0.1:1/mcp", "secret", "project-B")) as identity, \
                    mock.patch.object(smoke, "mcp_for_data",
                                      side_effect=AssertionError("restore must stay offline")):
                smoke.verify_persisted_project_identity(
                    root / "data", project_b, "project-B", report)

            expected_data = root / "data"
            if sys.platform == "win32":
                expected_data = Path(smoke.windows_extended_path(str(expected_data.resolve())))
            identity.assert_called_once_with(expected_data)
        stage = next(item for item in report.value["stages"]
                     if item["name"] == "restore_only.persisted_project_identity")
        self.assertEqual(stage["status"], "passed")
        self.assertNotIn("secret", json.dumps(smoke.sanitize_report(report)))

    def test_prepare_preserves_native_suffix_for_local_builds(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory) / "repo"
            debug = repo / "existing target" / "debug"
            debug.mkdir(parents=True)
            binaries = {
                "chadex-helper": b"helper local build",
                "chadex-runtime-cli": b"runtime cli local build",
                "chadex-runtime-server": b"runtime server local build",
                "chadex-runtime-runner": b"runtime runner local build",
            }
            for name, content in binaries.items():
                (debug / name).write_bytes(content)
            desktop_output = repo / "desktop output"

            with mock.patch.object(prepare.sys, "platform", "darwin"):
                copied = prepare.prepare(repo, repo / "existing target", desktop_output)

            self.assertEqual(copied, 4)
            relative_paths = (
                Path("helper/chadex-helper"),
                Path("chadex-runtime/chadex-runtime-cli"),
                Path("chadex-runtime/chadex-runtime-server"),
                Path("chadex-runtime/chadex-runtime-runner"),
            )
            for relative in relative_paths:
                self.assertEqual((repo / "apps/windows/src-tauri/resources" / relative).read_bytes(),
                                 binaries[relative.name])
                self.assertEqual((desktop_output / relative).read_bytes(), binaries[relative.name])


if __name__ == "__main__":
    unittest.main(verbosity=2)
