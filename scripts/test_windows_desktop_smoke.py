#!/usr/bin/env python3
"""Deterministic tests for the Windows desktop smoke harness and resource prep."""
from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))

import prepare_windows_desktop as prepare
import windows_desktop_smoke as smoke


class WindowsDesktopSmokeTests(unittest.TestCase):
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

            identity.assert_called_once_with(root / "data")
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
