"""Portable regressions for the W5 installed-release wrapper."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import MagicMock, patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import windows_installer_smoke as smoke
import windows_runtime_e2e as w2


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def make_candidate(root: Path) -> tuple[Path, dict[str, object]]:
    candidate = root / "candidate"
    candidate.mkdir()
    installer_data = b"unsigned candidate installer"
    fixture_data = b"same production binaries, synthetic 0.3.1 metadata"
    installer_name = "Chadex-0.3.2-windows-x64-unsigned-setup.exe"
    (candidate / installer_name).write_bytes(installer_data)
    fixture_dir = candidate / "upgrade-fixture"
    fixture_dir.mkdir()
    (fixture_dir / "baseline-setup.exe").write_bytes(fixture_data)
    resources = []
    for index, relative in enumerate(sorted(smoke.EXPECTED_RESOURCES)):
        content = f"resource {index}".encode()
        resources.append({"relative_path": relative, "sha256": digest(content),
                          "size_bytes": len(content)})
    resource_manifest = {
        "schema": smoke.RESOURCE_SCHEMA, "version": "0.3.2", "arch": "AMD64",
        "profile": "release", "production_features": {
            "custom-protocol": True, "desktop-smoke": False,
        }, "unsigned": True, "resources": resources,
    }
    resource_bytes = (json.dumps(resource_manifest, sort_keys=True) + "\n").encode()
    (candidate / "release-resources.json").write_bytes(resource_bytes)
    metadata = {
        "schema": 1, "track": "W5", "version": "0.3.2", "source_sha": "a" * 40,
        "architecture": "x86_64", "profile": "release", "features": ["custom-protocol"],
        "desktop_smoke": False, "authenticode": "unsigned", "updater": "disabled",
        "installer": installer_name, "sha256": digest(installer_data),
        "desktop_sha256": "b" * 64, "upgrade_fixture_sha256": digest(fixture_data),
        "synthetic_upgrade_fixture": True,
    }
    (candidate / "candidate.json").write_text(json.dumps(metadata), encoding="utf-8")
    return candidate, metadata


class WindowsInstallerSmokeTests(unittest.TestCase):
    def test_candidate_accepts_manifest_only_resources_and_checks_both_installer_hashes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            candidate_dir, candidate_json = make_candidate(root)
            candidate = smoke.validate_candidate(candidate_dir)
            self.assertEqual(candidate["source_sha"], "a" * 40)
            self.assertEqual(len(candidate["resources"]["resources"]), 7)
            self.assertEqual({path.name for path in candidate_dir.iterdir()}, {
                candidate_json["installer"], "candidate.json", "release-resources.json",
                "upgrade-fixture",
            })

            candidate_json["upgrade_fixture_sha256"] = "0" * 64
            (candidate_dir / "candidate.json").write_text(json.dumps(candidate_json))
            with self.assertRaises(smoke.SmokeFailure) as failure:
                smoke.validate_candidate(candidate_dir)
            self.assertEqual(failure.exception.code, "candidate_hash_mismatch")

    def test_candidate_rejects_traversal_and_installer_hash_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            candidate_dir, metadata = make_candidate(Path(directory))
            manifest_path = candidate_dir / "release-resources.json"
            resource_manifest = json.loads(manifest_path.read_text())
            resource_manifest["resources"][0]["relative_path"] = "../helper/chadex-helper.exe"
            manifest_path.write_text(json.dumps(resource_manifest))
            with self.assertRaises(smoke.SmokeFailure) as failure:
                smoke.validate_candidate(candidate_dir)
            self.assertEqual(failure.exception.code, "resource_path_unsafe")

        with tempfile.TemporaryDirectory() as directory:
            candidate_dir, metadata = make_candidate(Path(directory))
            metadata["sha256"] = "0" * 64
            (candidate_dir / "candidate.json").write_text(json.dumps(metadata))
            with self.assertRaises(smoke.SmokeFailure) as failure:
                smoke.validate_candidate(candidate_dir)
            self.assertEqual(failure.exception.code, "candidate_hash_mismatch")

    def test_runner_gate_requires_github_hosted_and_preflight_refuses_existing_state(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            env = {
                "GITHUB_ACTIONS": "true", "RUNNER_ENVIRONMENT": "self-hosted",
                "RUNNER_TEMP": directory, "LOCALAPPDATA": directory, "APPDATA": directory,
            }
            with self.assertRaises(smoke.SmokeFailure) as failure:
                smoke.runner_gate(env, "win32")
            self.assertEqual(failure.exception.code, "github_hosted_runner_required")
            env["RUNNER_ENVIRONMENT"] = "github-hosted"
            self.assertEqual(smoke.runner_gate(env, "win32")[0], Path(directory).resolve())
            with self.assertRaises(smoke.SmokeFailure) as failure:
                smoke.runner_gate(env, "darwin")
            self.assertEqual(failure.exception.code, "windows_native_required")

            existing = Path(directory) / "app.chadex.windows"
            existing.mkdir()
            with self.assertRaises(smoke.SmokeFailure) as failure:
                smoke.check_absent_user_state([existing], False)
            self.assertEqual(failure.exception.code, "preexisting_appdata")
            self.assertTrue(existing.is_dir())
            with self.assertRaises(smoke.SmokeFailure) as failure:
                smoke.check_absent_user_state([], True)
            self.assertEqual(failure.exception.code, "preexisting_uninstall_entry")

    def test_install_resource_hashes_and_default_path_contract(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            install = root / "install"
            install.mkdir()
            resources = []
            for index, relative in enumerate(sorted(smoke.EXPECTED_RESOURCES)):
                content = f"installed resource {index}".encode()
                path = install / Path(relative)
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(content)
                resources.append({"relative_path": relative, "sha256": digest(content),
                                  "size_bytes": len(content)})
            desktop = b"installed Chadex.exe"
            (install / "Chadex.exe").write_bytes(desktop)
            manifest_bytes = b'{"installed":"release manifest"}\n'
            (install / "release-resources.json").write_bytes(manifest_bytes)
            manifest = {"resources": resources}
            smoke.verify_installed_resources(install, manifest, digest(manifest_bytes), digest(desktop))

            local_data = Path(r"C:\runner\AppData\Local\app.chadex.windows")
            smoke.validate_default_paths({
                "helper": r"C:\runner\install\helper\chadex-helper.exe",
                "runtime": r"C:\runner\install\chadex-runtime",
                "data": str(local_data),
            }, Path(r"C:\runner\install"), local_data)
            with self.assertRaises(smoke.SmokeFailure) as failure:
                smoke.validate_default_paths({
                    "helper": r"C:\wrong\helper\chadex-helper.exe",
                    "runtime": r"C:\runner\install\chadex-runtime",
                    "data": str(local_data),
                }, Path(r"C:\runner\install"), local_data)
            self.assertEqual(failure.exception.code, "installed_path_mismatch")

            first = install / "helper" / "chadex-helper.exe"
            first.write_bytes(b"tampered")
            with self.assertRaises(smoke.SmokeFailure) as failure:
                smoke.verify_installed_resources(install, manifest, digest(manifest_bytes), digest(desktop))
            self.assertEqual(failure.exception.code, "resource_hash_mismatch")

    def test_registry_install_location_accepts_only_matched_outer_quotes(self) -> None:
        install = Path(r"C:\runner\安裝 Chadex")
        values = {"DisplayName": "Chadex", "DisplayVersion": "0.3.2",
                  "InstallLocation": '"C:\\runner\\安裝 Chadex"'}
        flags = smoke.registry_owner_flags(values, "0.3.2", install)
        self.assertTrue(all(flags.values()))
        values["InstallLocation"] = r"C:\runner\Different"
        self.assertFalse(smoke.registry_owner_flags(values, "0.3.2", install)["registry_install_dir_match"])
        values["InstallLocation"] = '"C:\\runner\\安裝 Chadex'
        self.assertFalse(smoke.registry_owner_flags(values, "0.3.2", install)["registry_install_dir_match"])

    def test_in_place_uninstall_keeps_only_unchanged_stub_until_harness_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            install = root / "安裝 Chadex"
            install.mkdir()
            uninstaller = install / "uninstall.exe"
            original = b"fixture-owned in-place uninstaller"
            uninstaller.write_bytes(original)
            expected_hash = digest(original)
            manifest = {"resources": []}

            smoke._verify_uninstalled(install, manifest, expected_hash)
            uninstaller.write_bytes(b"changed")
            with self.assertRaises(smoke.SmokeFailure) as failure:
                smoke._verify_uninstalled(install, manifest, expected_hash)
            self.assertEqual(failure.exception.code, "uninstall_files_remain")
            uninstaller.write_bytes(original)

            (install / "Chadex.exe").write_bytes(b"unexpected remainder")
            with self.assertRaises(smoke.SmokeFailure) as failure:
                smoke._verify_uninstalled(install, manifest, expected_hash)
            self.assertEqual(failure.exception.code, "uninstall_files_remain")
            (install / "Chadex.exe").unlink()

            smoke._remove_uninstaller_stub(install, expected_hash)
            self.assertFalse(install.exists())
            self.assertTrue(root.is_dir())

    def test_raw_nsis_tail_and_shell_free_popen_are_preserved(self) -> None:
        exe = Path(r"C:\runner\setup files\Chadex setup.exe")
        install = Path(r"C:\runner\安裝 Chadex")
        command = smoke.installer_arguments(exe, install)
        self.assertTrue(command.startswith('"C:\\runner\\setup files\\Chadex setup.exe"'))
        self.assertTrue(command.endswith(r" /D=C:\runner\安裝 Chadex"))
        self.assertNotIn('"', command[command.rfind(" /D=") + 4:])
        uninstall = smoke.uninstaller_arguments(Path(r"C:\runner\uninstall.exe"), install)
        self.assertTrue(uninstall.endswith(r" _?=C:\runner\安裝 Chadex"))
        self.assertNotIn('"', uninstall[uninstall.rfind(" _?=") + 4:])

        process = MagicMock()
        process.poll.return_value = 0
        process.returncode = 0
        with patch.object(smoke.suspended, "launch_owned", return_value=process) as launch, \
                patch.object(smoke, "_wait_owned_gone", return_value=[]):
            report = smoke.SmokeReport("win32")
            smoke.run_owned_executable(
                exe, command, cwd=Path("."), powershell="powershell.exe", groups=[],
                timeout=1, timeout_code="installer_timeout", spawn_code="installer_spawn_failed",
                exit_code="installer_exit_nonzero", report=report,
            )
        self.assertEqual(launch.call_args.args[0], command)
        self.assertEqual(launch.call_args.kwargs["executable"], exe)
        self.assertEqual(launch.call_args.kwargs["owned"], {})

    def test_data_snapshot_and_w4_process_identity_remain_strict(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            data = Path(directory) / "data"
            data.mkdir()
            value = data / "preferences.json"
            value.write_bytes(b"stable")
            before = smoke.data_snapshot(data)
            smoke.require_unchanged_snapshot(data, before)
            value.write_bytes(b"changed")
            with self.assertRaises(smoke.SmokeFailure) as failure:
                smoke.require_unchanged_snapshot(data, before)
            self.assertEqual(failure.exception.code, "appdata_changed")

        self.assertFalse(w2.same_process(
            {"Created": "2026-01-01T00:00:01.0000000Z"},
            {"Created": "2026-01-01T00:00:00.0000000Z"},
        ))
        with self.assertRaises(w2.E2EFailure):
            w2.descendants({1: {"ParentProcessId": 0},
                            2: {"ParentProcessId": 1, "Created": ""}}, {1})

    def test_report_projection_drops_paths_commands_credentials_and_forced_runs_fail(self) -> None:
        report = smoke.SmokeReport("win32")
        for stage in report.value["stages"]:
            stage["status"] = "passed"
        report.value["cleanup"]["forced_count"] = 1
        report.value["stages"][0].update({
            "path": "C:\\private\\user", "commandline": "--token secret",
            "credential": "secret", "error": "raw exception text",
        })
        report.finish(True)
        payload = json.dumps(report.public_value())
        public = report.public_value()
        self.assertEqual(report.value["passed"], False)
        self.assertEqual(public["uninstall_mode"], "in_place_no_self_copy")
        self.assertEqual(public["uninstaller_cleanup"], "harness_after_exit")
        self.assertIn("default_uninstaller_self_copy", public["manual_checks_pending"])
        for secret in ("C:\\private", "--token secret", '"credential": "secret"', "raw exception"):
            self.assertNotIn(secret, payload)
        self.assertEqual(report.public_value()["upgrade_type"], "synthetic_metadata_upgrade")


if __name__ == "__main__":
    unittest.main()
