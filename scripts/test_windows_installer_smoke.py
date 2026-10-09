"""Portable regressions for the W5 installed-release wrapper."""
from __future__ import annotations

import hashlib
from contextlib import nullcontext
import io
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


def policy_registry(values: dict[str, tuple[str, int]]) -> MagicMock:
    registry = MagicMock()
    registry.KEY_READ, registry.KEY_SET_VALUE, registry.KEY_WOW64_64KEY = 1, 2, 256
    registry.REG_SZ = 1
    key = MagicMock()
    registry.OpenKey.return_value = key
    registry.CreateKeyEx.return_value = key
    def query(_key: object, name: str) -> tuple[str, int]:
        if name not in values:
            raise FileNotFoundError(name)
        return values[name]
    registry.QueryValueEx.side_effect = query
    registry.SetValueEx.side_effect = lambda _key, name, _reserved, kind, value: values.__setitem__(name, (value, kind))
    registry.DeleteValue.side_effect = lambda _key, name: values.pop(name)
    return registry


class InstalledCdpPolicyTests(unittest.TestCase):
    def fixture(self, registry: MagicMock):
        return (patch.object(smoke.sys, 'platform', 'win32'),
                patch.dict(smoke.os.environ, {'GITHUB_ACTIONS': 'true', 'RUNNER_ENVIRONMENT': 'github-hosted'}),
                patch.object(smoke, '_winreg_module', return_value=registry))

    def test_owned_value_removed_on_success_and_app_failure_preserving_other_policy(self) -> None:
        for fails in (False, True):
            values = {'Other.exe': ('other policy', 1)}
            registry = policy_registry(values)
            if fails:
                registry.OpenKey.side_effect = FileNotFoundError()
            a, b, c = self.fixture(registry)
            with a, b, c:
                def run():
                    with smoke.installed_cdp_policy(9222):
                        self.assertIn('--remote-debugging-address=127.0.0.1', values['Chadex.exe'][0])
                        if fails:
                            raise smoke.SmokeFailure('installed_probe_failed')
                if fails:
                    with self.assertRaises(smoke.SmokeFailure) as failure:
                        run()
                    self.assertEqual(failure.exception.code, 'installed_probe_failed')
                else:
                    run()
            self.assertEqual(values, {'Other.exe': ('other policy', 1)})
            registry.SetValueEx.assert_called_once()
            registry.DeleteValue.assert_called_once()
            registry.DeleteKey.assert_not_called()

    def test_existing_app_identifier_or_wildcard_is_not_overwritten(self) -> None:
        for name in ('Chadex.exe', 'app.chadex.windows', '*'):
            values = {name: ('preexisting', 1)}
            registry = policy_registry(values)
            a, b, c = self.fixture(registry)
            with a, b, c, self.assertRaises(smoke.SmokeFailure) as failure:
                with smoke.installed_cdp_policy(9222):
                    self.fail('existing policy entered application lifecycle')
            self.assertEqual(failure.exception.code, 'preexisting_cdp_policy')
            self.assertEqual(values, {name: ('preexisting', 1)})
            registry.SetValueEx.assert_not_called()
            registry.DeleteValue.assert_not_called()

    def test_non_hosted_runner_cannot_touch_machine_policy(self) -> None:
        registry = policy_registry({})
        a, b, c = self.fixture(registry)
        with a, b, c, patch.dict(smoke.os.environ, {'RUNNER_ENVIRONMENT': 'self-hosted'}):
            with self.assertRaises(smoke.SmokeFailure) as failure:
                with smoke.installed_cdp_policy(9222):
                    self.fail('non-hosted runner entered lifecycle')
        self.assertEqual(failure.exception.code, 'github_hosted_runner_required')
        registry.OpenKey.assert_not_called()

    def test_changed_value_is_preserved_and_cleanup_cannot_pass(self) -> None:
        values = {}
        registry = policy_registry(values)
        a, b, c = self.fixture(registry)
        with a, b, c, self.assertRaises(smoke.SmokeFailure) as failure:
            with smoke.installed_cdp_policy(9222):
                values['Chadex.exe'] = ('concurrent replacement', 1)
        self.assertEqual(failure.exception.code, 'cdp_policy_cleanup_failed')
        self.assertEqual(values['Chadex.exe'], ('concurrent replacement', 1))
        registry.DeleteValue.assert_not_called()

    def test_failed_readback_never_launches_and_owned_value_is_removed(self) -> None:
        values = {}
        registry = policy_registry(values)
        original = registry.QueryValueEx.side_effect
        failed_readback = False
        def query(key, name):
            nonlocal failed_readback
            if name == 'Chadex.exe' and name in values and not failed_readback:
                failed_readback = True
                return ('invalid readback', 1)
            return original(key, name)
        registry.QueryValueEx.side_effect = query
        a, b, c = self.fixture(registry)
        with a, b, c, self.assertRaises(smoke.SmokeFailure) as failure:
            with smoke.installed_cdp_policy(9222):
                self.fail('failed policy readback entered lifecycle')
        self.assertEqual(failure.exception.code, 'cdp_policy_failed')
        self.assertEqual(values, {})

    def test_app_groups_are_cleaned_before_policy_removal_on_success_and_failure(self) -> None:
        for fails in (False, True):
            values = {}
            registry = policy_registry(values)
            groups = [{10: {'Created': 'old installation identity'}}]
            owned = {20: {'Created': 'new application identity'}}
            def run_app(*args, **kwargs):
                groups.append(owned)
                if fails:
                    raise smoke.SmokeFailure('installed_probe_failed')
                return {'rendered': True}
            def cleanup(_powershell, cleanup_groups, _report):
                self.assertIn('Chadex.exe', values)
                self.assertEqual(cleanup_groups, [owned])
            a, b, c = self.fixture(registry)
            with a, b, c, patch.object(smoke, '_port', return_value=9222), \
                    patch.object(smoke, '_launch_and_probe', side_effect=run_app), \
                    patch.object(smoke, '_cleanup_process_groups', side_effect=cleanup) as clean:
                args = (Path('install'), Path('project'), Path('.'), Path('data'), '0.3.2', 'initial')
                kwargs = dict(powershell='powershell.exe', node='node', groups=groups,
                              report=smoke.SmokeReport('win32'))
                if fails:
                    with self.assertRaises(smoke.SmokeFailure) as failure:
                        smoke.launch_and_probe(*args, **kwargs)
                    self.assertEqual(failure.exception.code, 'installed_probe_failed')
                else:
                    self.assertTrue(smoke.launch_and_probe(*args, **kwargs)['cdp_policy_removed'])
            clean.assert_called_once()
            self.assertEqual(values, {})


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def make_candidate(root: Path) -> tuple[Path, dict[str, object]]:
    candidate = root / "candidate"
    candidate.mkdir()
    installer_data = b"unsigned candidate installer"
    fixture_data = b"same production binaries, synthetic 0.6.0 metadata"
    installer_name = "Chadex-0.6.1-windows-x64-unsigned-setup.exe"
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
        "schema": smoke.RESOURCE_SCHEMA, "version": "0.6.1", "arch": "AMD64",
        "profile": "release", "production_features": {
            "custom-protocol": True, "desktop-smoke": False,
        }, "unsigned": True, "resources": resources,
    }
    resource_bytes = (json.dumps(resource_manifest, sort_keys=True) + "\n").encode()
    (candidate / "release-resources.json").write_bytes(resource_bytes)
    metadata = {
        "schema": 1, "track": "W5", "version": "0.6.1", "source_sha": "a" * 40,
        "architecture": "x86_64", "profile": "release", "features": ["custom-protocol"],
        "desktop_smoke": False, "authenticode": "unsigned", "updater": "disabled",
        "synthetic_upgrade_baseline": "0.6.0",
        "installer": installer_name, "sha256": digest(installer_data),
        "desktop_sha256": "b" * 64, "upgrade_fixture_sha256": digest(fixture_data),
        "synthetic_upgrade_fixture": True,
    }
    (candidate / "candidate.json").write_text(json.dumps(metadata), encoding="utf-8")
    return candidate, metadata


def make_historical_baseline(root: Path) -> tuple[Path, dict[str, object]]:
    baseline = root / "historical"
    baseline.mkdir()
    installer_data = b"historical 0.6.0 installer"
    installer_name = "Chadex-0.6.0-windows-x64-unsigned-setup.exe"
    (baseline / installer_name).write_bytes(installer_data)
    resources = []
    for index, relative in enumerate(sorted(smoke.EXPECTED_RESOURCES)):
        content = f"historical resource {index}".encode()
        resources.append({"relative_path": relative, "sha256": digest(content),
                          "size_bytes": len(content)})
    resource_manifest = {
        "schema": smoke.RESOURCE_SCHEMA, "version": "0.6.0", "arch": "AMD64",
        "profile": "release", "production_features": {
            "custom-protocol": True, "desktop-smoke": False,
        }, "unsigned": True, "resources": resources,
    }
    (baseline / "release-resources.json").write_text(
        json.dumps(resource_manifest, sort_keys=True) + "\n", encoding="utf-8")
    metadata = {
        "schema": 1, "track": "W5", "version": "0.6.0", "source_sha": "c" * 40,
        "architecture": "x86_64", "profile": "release", "features": ["custom-protocol"],
        "desktop_smoke": False, "authenticode": "unsigned", "updater": "disabled",
        "installer": installer_name, "sha256": digest(installer_data),
        "desktop_sha256": "d" * 64, "synthetic_upgrade_fixture": False,
    }
    (baseline / "candidate.json").write_text(json.dumps(metadata), encoding="utf-8")
    return baseline, metadata


class WindowsInstallerSmokeTests(unittest.TestCase):
    def test_installed_probe_rejects_crashed_app_after_successful_probe(self) -> None:
        for exit_code in (0, 0xC0000005):
            with self.subTest(exit_code=exit_code):
                app = MagicMock()
                app.poll.return_value = exit_code
                app.wait.return_value = exit_code
                node = MagicMock()
                node.poll.return_value = 0
                node.wait.return_value = 0
                node.stdout = io.BytesIO(b"{}")
                node.stderr = io.BytesIO(b"")
                with patch.object(smoke, "_regular_file"), \
                        patch.object(smoke, "_port", return_value=9222), \
                        patch.object(smoke, "installed_cdp_policy", side_effect=lambda _port: nullcontext()), \
                        patch.object(smoke.suspended, "launch_owned", return_value=app), \
                        patch.object(smoke.subprocess, "Popen", return_value=node), \
                        patch.object(smoke, "_remember_tree", return_value={}), \
                        patch.object(smoke, "_wait_owned_gone", return_value=[]), \
                        patch.object(smoke, "validate_probe_result", return_value={"ui_state_ready": True}):
                    kwargs = dict(powershell="powershell.exe", node="node", groups=[],
                                  report=smoke.SmokeReport("win32"))
                    args = (Path("install"), Path("project"), Path("."), Path("data"), "0.3.2", "initial")
                    if exit_code == 0:
                        self.assertEqual(smoke.launch_and_probe(*args, **kwargs), {"ui_state_ready": True, "cdp_policy_removed": True})
                    else:
                        with self.assertRaises(smoke.SmokeFailure) as failure:
                            smoke.launch_and_probe(*args, **kwargs)
                        self.assertEqual(failure.exception.code, "installed_app_exit_nonzero")
                app.wait.assert_called_once_with(timeout=10)

    def test_candidate_accepts_manifest_only_resources_and_checks_both_installer_hashes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            candidate_dir, candidate_json = make_candidate(root)
            candidate = smoke.validate_candidate(candidate_dir)
            self.assertEqual(candidate["source_sha"], "a" * 40)
            self.assertEqual(candidate["baseline_version"], "0.6.0")
            self.assertEqual(len(candidate["resources"]["resources"]), 7)
            self.assertEqual({path.name for path in candidate_dir.iterdir()}, {
                candidate_json["installer"], "candidate.json", "release-resources.json",
                "upgrade-fixture",
            })

            candidate_json["synthetic_upgrade_baseline"] = candidate_json["version"]
            (candidate_dir / "candidate.json").write_text(json.dumps(candidate_json))
            with self.assertRaises(smoke.SmokeFailure) as failure:
                smoke.validate_candidate(candidate_dir)
            self.assertEqual(failure.exception.code, "candidate_metadata_invalid")
            candidate_json["synthetic_upgrade_baseline"] = "0.6.0"

            candidate_json["upgrade_fixture_sha256"] = "0" * 64
            (candidate_dir / "candidate.json").write_text(json.dumps(candidate_json))
            with self.assertRaises(smoke.SmokeFailure) as failure:
                smoke.validate_candidate(candidate_dir)
            self.assertEqual(failure.exception.code, "candidate_hash_mismatch")

            candidate_json["upgrade_fixture_sha256"] = digest((candidate_dir / "upgrade-fixture" / "baseline-setup.exe").read_bytes())
            (candidate_dir / "candidate.json").write_text(json.dumps(candidate_json))
            (candidate_dir / "upgrade-fixture" / "baseline-setup.exe").unlink()
            candidate = smoke.validate_candidate(candidate_dir, require_synthetic_fixture=False)
            self.assertEqual(candidate["version"], "0.6.1")
            with self.assertRaises(smoke.SmokeFailure) as failure:
                smoke.validate_candidate(candidate_dir)
            self.assertEqual(failure.exception.code, "candidate_fixture_missing")

    def test_historical_baseline_uses_its_own_source_and_resource_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            baseline_dir, metadata = make_historical_baseline(root)
            baseline = smoke.validate_historical_baseline(baseline_dir, "0.6.1")
            self.assertEqual(baseline["version"], "0.6.0")
            self.assertEqual(baseline["source_sha"], "c" * 40)
            self.assertEqual(baseline["resources"]["version"], "0.6.0")
            self.assertEqual(len(baseline["resources"]["resources"]), 7)

            metadata["version"] = "0.6.1"
            (baseline_dir / "candidate.json").write_text(json.dumps(metadata), encoding="utf-8")
            with self.assertRaises(smoke.SmokeFailure) as failure:
                smoke.validate_historical_baseline(baseline_dir, "0.6.1")
            self.assertEqual(failure.exception.code, "candidate_metadata_invalid")

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
            smoke.validate_default_paths({
                "helper": r"\\?\C:\RUNNER\install\helper\chadex-helper.exe",
                "runtime": r"\\?\C:\RUNNER\install\chadex-runtime",
                "data": r"\\?\C:\RUNNER\AppData\Local\app.chadex.windows",
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
        default_uninstall = smoke.default_uninstaller_arguments(Path(r"C:\runner\uninstall.exe"))
        self.assertEqual(default_uninstall, r"C:\runner\uninstall.exe /S")
        self.assertNotIn("_?=", default_uninstall)

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

    def test_default_cleanup_never_retries_an_uncertain_uninstall(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            install = root / "default install"
            install.mkdir()
            report = smoke.SmokeReport("win32")
            with patch.object(smoke, "run_default_uninstaller") as run, \
                    patch.object(smoke, "verify_registry_removed") as registry:
                with self.assertRaises(smoke.SmokeFailure) as failure:
                    smoke._cleanup_default_install(
                        install, install_attempted=True, install_succeeded=True,
                        uninstall_attempted=True, uninstall_succeeded=False, root=root,
                        powershell="powershell.exe", groups=[], report=report,
                    )
            self.assertEqual(failure.exception.code, "fixture_cleanup_failed")
            run.assert_not_called()
            registry.assert_not_called()

            install.rmdir()
            with patch.object(smoke, "run_default_uninstaller") as run, \
                    patch.object(smoke, "verify_registry_removed") as registry:
                attempted, succeeded = smoke._cleanup_default_install(
                    install, install_attempted=True, install_succeeded=True,
                    uninstall_attempted=True, uninstall_succeeded=False, root=root,
                    powershell="powershell.exe", groups=[], report=report,
                )
            self.assertTrue(attempted)
            self.assertFalse(succeeded)
            run.assert_not_called()
            registry.assert_called_once_with()

    def test_default_cleanup_attempts_uninstaller_once_only_after_known_install_success(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            install = root / "default install"
            install.mkdir()
            report = smoke.SmokeReport("win32")

            def remove_install(*_args: object, **_kwargs: object) -> None:
                install.rmdir()

            with patch.object(smoke, "run_default_uninstaller", side_effect=remove_install) as run, \
                    patch.object(smoke, "verify_registry_removed") as registry:
                attempted, succeeded = smoke._cleanup_default_install(
                    install, install_attempted=True, install_succeeded=True,
                    uninstall_attempted=False, uninstall_succeeded=False, root=root,
                    powershell="powershell.exe", groups=[], report=report,
                )
            self.assertTrue(attempted)
            self.assertTrue(succeeded)
            run.assert_called_once()
            registry.assert_called_once_with()

            partial = root / "partial install"
            partial.mkdir()
            with patch.object(smoke, "run_default_uninstaller") as run, \
                    patch.object(smoke, "verify_registry_removed"):
                with self.assertRaises(smoke.SmokeFailure) as failure:
                    smoke._cleanup_default_install(
                        partial, install_attempted=True, install_succeeded=False,
                        uninstall_attempted=False, uninstall_succeeded=False, root=root,
                        powershell="powershell.exe", groups=[], report=report,
                    )
            self.assertEqual(failure.exception.code, "fixture_cleanup_failed")
            run.assert_not_called()

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

    def test_rpc_projection_accepts_only_finite_step_and_reason(self) -> None:
        report = smoke.SmokeReport("win32")
        stage = report.value["stages"][0]
        stage["probe_failure"] = "installed_rpc_inspectProject:project_unavailable"
        self.assertEqual(report.public_value()["stages"][0]["probe_failure"], stage["probe_failure"])
        for raw in ("installed_rpc_inspectProject:C:/private/token", "installed_rpc_secret:other"):
            stage["probe_failure"] = raw
            self.assertNotIn("probe_failure", report.public_value()["stages"][0])

    def test_inventory_timeout_during_owned_launch_keeps_its_code(self) -> None:
        report = smoke.SmokeReport("win32")
        with tempfile.TemporaryDirectory() as tmp, \
                patch.object(smoke.suspended, "launch_owned",
                             side_effect=smoke.w2.E2EFailure("process_inventory_timeout")):
            with self.assertRaises(smoke.SmokeFailure) as raised:
                smoke.run_owned_executable(
                    Path(tmp) / "setup.exe", "/S", cwd=Path(tmp), powershell="powershell.exe",
                    groups=[], timeout=1, timeout_code="installer_timeout",
                    spawn_code="installer_spawn_failed", exit_code="installer_exit_nonzero",
                    report=report,
                )
        self.assertEqual(raised.exception.code, "process_inventory_timeout")
        self.assertIn("process_inventory_timeout", smoke.SAFE_CODES)

    def test_unexpected_stage_exception_logs_traceback_and_reports_only_its_type(self) -> None:
        report = smoke.SmokeReport("win32")
        stderr = io.StringIO()
        with patch.object(sys, "stderr", stderr):
            with self.assertRaises(smoke.SmokeFailure) as raised:
                with report.stage("baseline_install"):
                    raise PermissionError("C:\\private\\user\\Chadex.exe is locked")
        self.assertEqual(raised.exception.code, "unexpected_exception")
        log = stderr.getvalue()
        self.assertIn("W5 stage baseline_install raised an unexpected exception", log)
        self.assertIn("PermissionError", log)
        self.assertIn("is locked", log)
        row = next(item for item in report.public_value()["stages"]
                   if item["name"] == "baseline_install")
        self.assertEqual(row["error_code"], "unexpected_exception")
        self.assertEqual(row["exception_type"], "PermissionError")
        self.assertNotIn("private", json.dumps(report.public_value()))

        stage = next(item for item in report.value["stages"] if item["name"] == "baseline_install")
        stage["exception_type"] = "not a class name; C:\\private"
        self.assertNotIn("exception_type", next(
            item for item in report.public_value()["stages"] if item["name"] == "baseline_install"))

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
        self.assertEqual(public["default_uninstaller_self_copy"], "isolated_native_stage")
        self.assertEqual(public["uninstaller_cleanup"], "harness_after_exit")
        self.assertNotIn("default_uninstaller_self_copy", public["external_checks_pending"])
        self.assertEqual(public["upgrade_type"], "synthetic_metadata_upgrade")
        historical = smoke.SmokeReport(
            "win32", upgrade_type="historical_source_upgrade",
            external_checks_pending=["default_uninstaller_self_copy"],
        )
        historical.value["baseline_source_sha"] = "c" * 40
        for stage in historical.value["stages"]:
            stage["status"] = "passed"
        historical.finish(True)
        historical_public = historical.public_value()
        self.assertEqual(historical_public["upgrade_type"], "historical_source_upgrade")
        self.assertEqual(historical_public["baseline_source_sha"], "c" * 40)
        self.assertNotIn("historical_source_upgrade", historical_public["external_checks_pending"])
        for secret in ("C:\\private", "--token secret", '"credential": "secret"', "raw exception"):
            self.assertNotIn(secret, payload)
        self.assertEqual(report.public_value()["upgrade_type"], "synthetic_metadata_upgrade")


if __name__ == "__main__":
    unittest.main()
