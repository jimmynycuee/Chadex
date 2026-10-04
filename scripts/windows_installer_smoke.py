#!/usr/bin/env python3
"""Exercise a W5 Windows installer only on an isolated GitHub Actions runner."""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import hashlib
import json
import ntpath
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import threading
import time
from typing import Any, Iterator, Mapping

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import windows_runtime_e2e as w2
import windows_suspended_launch as suspended


RESOURCE_SCHEMA = "chadex.windows.release-resources.v1"
EXPECTED_RESOURCES = frozenset({
    "helper/chadex-helper.exe",
    "chadex-runtime/chadex-runtime-cli.exe",
    "chadex-runtime/chadex-runtime-server.exe",
    "chadex-runtime/chadex-runtime-runner.exe",
    "licenses/Chadex-LICENSE.txt",
    "licenses/WebCodex-LICENSE.txt",
    "licenses/UPSTREAM.md",
})
STAGES = (
    "runner_preflight", "candidate_validation", "user_state_preflight",
    "runner_tools", "fixture_setup", "baseline_install",
    "baseline_resource_verification", "initial_installed_launch",
    "candidate_upgrade_install", "upgrade_resource_and_data_verification",
    "upgrade_restore_launch", "same_version_reinstall",
    "reinstall_resource_and_data_verification", "reinstall_restore_launch",
    "uninstall", "uninstall_preservation_verification",
    "default_uninstaller_self_copy", "fixture_cleanup",
)
EXTERNAL_CHECKS_PENDING = [
    "native_picker", "explorer_open", "tray", "launch_at_login",
    "notifications", "credentialed_tunnel", "physical_windows_11", "windows_arm64",
    "authenticode_signing", "missing_webview2", "interactive_installer",
    "credential_manager_uninstall_policy", "automatic_updater",
]
VERSION_PATTERN = r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?"
SAFE_CODES = frozenset({
    "windows_native_required", "github_actions_required", "github_hosted_runner_required",
    "runner_temp_missing",
    "runner_temp_unavailable", "known_data_paths_unavailable",
    "preexisting_appdata", "preexisting_uninstall_entry", "registry_unavailable",
    "preexisting_cdp_policy", "cdp_policy_failed", "cdp_policy_cleanup_failed",
    "registry_access_failed", "registry_read_failed", "candidate_dir_invalid",
    "candidate_manifest_missing", "candidate_manifest_invalid",
    "candidate_filename_unsafe", "candidate_hash_invalid", "candidate_hash_mismatch",
    "candidate_metadata_invalid", "candidate_fixture_missing",
    "resource_manifest_missing", "resource_manifest_invalid", "resource_path_unsafe",
    "resource_file_missing", "resource_hash_mismatch", "resource_set_invalid",
    "runner_tool_missing", "probe_script_missing", "fixture_setup_failed",
    "fixture_cleanup_failed", "fixture_reparse_point", "installer_spawn_failed",
    "installer_timeout", "installer_exit_nonzero", "installer_processes_remain",
    "uninstaller_missing", "uninstaller_exit_nonzero", "uninstall_files_remain",
    "uninstall_registry_remains", "installed_registry_mismatch",
    "installed_app_missing", "installed_probe_spawn_failed", "installed_probe_timeout",
    "installed_probe_failed", "installed_probe_invalid", "installed_path_mismatch",
    "installed_version_mismatch", "installed_helper_identity_missing",
    "installed_helper_identity_unowned", "installed_helper_identity_changed",
    "installed_runtime_not_ready", "installed_ui_not_rendered",
    "installed_preferences_not_restored", "installed_smoke_ipc_exposed",
    "installed_processes_remain", "installed_app_exit_nonzero", "process_inventory_failed", "process_cleanup_failed",
    "node_cleanup_failed", "owned_processes_remain", "forced_cleanup_required",
    "appdata_missing", "appdata_snapshot_invalid", "appdata_changed",
    "preferences_missing", "preferences_changed", "project_marker_changed",
    "unexpected_appdata_created", "data_cleanup_failed", "unexpected_exception",
    "unclassified",
})
PROBE_TIMEOUT_SECONDS = 300
INSTALLER_TIMEOUT_SECONDS = 300
MAX_JSON_BYTES = 2 * 1024 * 1024
MAX_PROBE_BYTES = 64 * 1024
SAFE_PROBE_ERRORS = frozenset({
    "installed_webview_missing", "cdp_not_loopback", "cdp_connect_timeout",
    "cdp_connect_failed", "cdp_request_failed", "cdp_closed", "cdp_evaluate_timeout",
    "probe_mode_invalid", "installed_rpc_failed", "installed_helper_unavailable",
    "installed_version_or_startup_error", "unexpected_installed_credential",
    "installed_ui_not_rendered", "installed_project_inspection_failed",
    "installed_preferences_not_restored", "installed_runtime_not_ready",
    "installed_selection_invalid", "production_smoke_ipc_exposed",
    "installed_credential_read_failed", "installed_ui_state_not_ready", "installed_ipc_not_ready",
}) | frozenset(
    f"installed_rpc_{step}:{reason}"
    for step in ("desktop_state", "inspectProject", "activateProject", "save_preferences", "configureLocalSetup")
    for reason in ("type_error", "command_not_found", "permission_denied", "project_invalid_path",
                   "project_unavailable", "runtime_start_failed", "runtime_not_found", "helper_unavailable", "other")
)
FILE_ATTRIBUTE_REPARSE_POINT = 0x400


class SmokeFailure(RuntimeError):
    """A finite, report-safe failure code."""

    def __init__(self, code: str) -> None:
        super().__init__(code if code in SAFE_CODES else "unclassified")
        self.code = code if code in SAFE_CODES else "unclassified"


def require(condition: bool, code: str) -> None:
    if not condition:
        raise SmokeFailure(code)


def _increment_forced(report: "SmokeReport") -> None:
    report.value["cleanup"]["forced_count"] += 1


def _is_reparse(info: os.stat_result) -> bool:
    return bool(getattr(info, "st_file_attributes", 0) & FILE_ATTRIBUTE_REPARSE_POINT)


def _lexists(path: Path) -> bool:
    try:
        path.lstat()
        return True
    except FileNotFoundError:
        return False
    except OSError:
        raise SmokeFailure("fixture_cleanup_failed") from None


def _regular_file(path: Path, missing_code: str) -> os.stat_result:
    try:
        info = path.lstat()
    except OSError:
        raise SmokeFailure(missing_code) from None
    require(stat.S_ISREG(info.st_mode) and not path.is_symlink() and not _is_reparse(info),
            missing_code)
    return info


def _directory(path: Path, code: str) -> None:
    try:
        info = path.lstat()
    except OSError:
        raise SmokeFailure(code) from None
    require(stat.S_ISDIR(info.st_mode) and not path.is_symlink() and not _is_reparse(info), code)


def _sha256(path: Path) -> str:
    _regular_file(path, "resource_file_missing")
    digest = hashlib.sha256()
    try:
        with path.open("rb") as stream:
            while chunk := stream.read(1024 * 1024):
                digest.update(chunk)
    except OSError:
        raise SmokeFailure("resource_file_missing") from None
    return digest.hexdigest()


def _read_json(path: Path, missing_code: str, invalid_code: str) -> dict[str, Any]:
    _regular_file(path, missing_code)
    try:
        raw = path.read_bytes()
        require(len(raw) <= MAX_JSON_BYTES, invalid_code)
        value = json.loads(raw.decode("utf-8-sig"))
    except SmokeFailure:
        raise
    except (OSError, UnicodeError, ValueError):
        raise SmokeFailure(invalid_code) from None
    require(isinstance(value, dict), invalid_code)
    return value


def _safe_executable_basename(value: Any) -> bool:
    if not isinstance(value, str) or not value or len(value) > 128:
        return False
    if value in {".", ".."} or value.endswith((".", " ")):
        return False
    if any(ord(character) < 32 or character in '<>:"/\\|?*' for character in value):
        return False
    if Path(value).name != value or PureWindowsPath(value).name != value:
        return False
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*\.exe", value, re.IGNORECASE):
        return False
    reserved = {"CON", "PRN", "AUX", "NUL", *(f"COM{i}" for i in range(1, 10)),
                *(f"LPT{i}" for i in range(1, 10))}
    return value.rsplit(".", 1)[0].upper() not in reserved


def _checked_relative(root: Path, relative: str, code: str) -> Path:
    require(isinstance(relative, str) and "\\" not in relative and "\x00" not in relative,
            code)
    pure = PurePosixPath(relative)
    require(not pure.is_absolute() and pure.parts and all(part not in {"", ".", ".."} for part in pure.parts),
            code)
    current = root
    for index, part in enumerate(pure.parts):
        current = current / part
        try:
            info = current.lstat()
        except OSError:
            raise SmokeFailure("resource_file_missing") from None
        require(not current.is_symlink() and not _is_reparse(info), code)
        if index < len(pure.parts) - 1:
            require(stat.S_ISDIR(info.st_mode), code)
    return current


def validate_release_resources(candidate_dir: Path, version: str) -> dict[str, Any]:
    manifest = _read_json(candidate_dir / "release-resources.json",
                          "resource_manifest_missing", "resource_manifest_invalid")
    require(manifest.get("schema") == RESOURCE_SCHEMA
            and manifest.get("version") == version
            and manifest.get("arch") == "AMD64"
            and manifest.get("profile") == "release"
            and manifest.get("production_features") == {
                "custom-protocol": True, "desktop-smoke": False,
            }
            and manifest.get("unsigned") is True,
            "resource_manifest_invalid")
    resources = manifest.get("resources")
    require(isinstance(resources, list) and len(resources) == len(EXPECTED_RESOURCES),
            "resource_set_invalid")
    paths = []
    for entry in resources:
        require(isinstance(entry, dict), "resource_manifest_invalid")
        relative = entry.get("relative_path")
        require(isinstance(relative, str) and "\\" not in relative and ":" not in relative
                and "\x00" not in relative, "resource_path_unsafe")
        pure = PurePosixPath(relative)
        require(relative == pure.as_posix() and not pure.is_absolute()
                and all(part not in {"", ".", ".."} for part in pure.parts),
                "resource_path_unsafe")
        paths.append(relative)
        digest = entry.get("sha256")
        size = entry.get("size_bytes")
        require(isinstance(digest, str) and re.fullmatch(r"[0-9a-f]{64}", digest)
                and type(size) is int and size >= 0, "resource_manifest_invalid")
    require(len(set(paths)) == len(paths) and set(paths) == EXPECTED_RESOURCES,
            "resource_set_invalid")
    return manifest


def validate_candidate(candidate_dir: Path, *, require_synthetic_fixture: bool = True) -> dict[str, Any]:
    try:
        info = candidate_dir.lstat()
    except OSError:
        raise SmokeFailure("candidate_dir_invalid") from None
    require(stat.S_ISDIR(info.st_mode) and not candidate_dir.is_symlink() and not _is_reparse(info),
            "candidate_dir_invalid")
    manifest = _read_json(candidate_dir / "candidate.json",
                          "candidate_manifest_missing", "candidate_manifest_invalid")
    installer_name = manifest.get("installer")
    require(_safe_executable_basename(installer_name), "candidate_filename_unsafe")
    version = manifest.get("version")
    require(isinstance(version, str) and re.fullmatch(VERSION_PATTERN, version),
            "candidate_metadata_invalid")
    baseline_version = manifest.get("synthetic_upgrade_baseline")
    require(isinstance(baseline_version, str)
            and re.fullmatch(VERSION_PATTERN, baseline_version)
            and baseline_version != version,
            "candidate_metadata_invalid")
    require(type(manifest.get("schema")) is int and manifest.get("schema") == 1
            and manifest.get("track") == "W5"
            and re.fullmatch(r"[0-9a-f]{40}", str(manifest.get("source_sha", "")))
            and manifest.get("architecture") == "x86_64"
            and manifest.get("profile") == "release"
            and manifest.get("features") == ["custom-protocol"]
            and manifest.get("desktop_smoke") is False
            and manifest.get("authenticode") == "unsigned"
            and manifest.get("synthetic_upgrade_fixture") is True,
            "candidate_metadata_invalid")
    installer_hash = manifest.get("sha256")
    require(isinstance(installer_hash, str) and re.fullmatch(r"[0-9a-f]{64}", installer_hash),
            "candidate_hash_invalid")
    desktop_hash = manifest.get("desktop_sha256")
    fixture_hash = manifest.get("upgrade_fixture_sha256")
    require(isinstance(desktop_hash, str) and re.fullmatch(r"[0-9a-f]{64}", desktop_hash)
            and isinstance(fixture_hash, str) and re.fullmatch(r"[0-9a-f]{64}", fixture_hash),
            "candidate_hash_invalid")
    installer = candidate_dir / installer_name
    _regular_file(installer, "candidate_manifest_invalid")
    require(_sha256(installer) == installer_hash, "candidate_hash_mismatch")
    fixture = candidate_dir / "upgrade-fixture" / "baseline-setup.exe"
    if require_synthetic_fixture:
        _regular_file(fixture, "candidate_fixture_missing")
        require(_sha256(fixture) == fixture_hash, "candidate_hash_mismatch")
        try:
            fixture_dir_info = fixture.parent.lstat()
        except OSError:
            raise SmokeFailure("candidate_fixture_missing") from None
        require(stat.S_ISDIR(fixture_dir_info.st_mode) and not fixture.parent.is_symlink()
                and not _is_reparse(fixture_dir_info), "candidate_fixture_missing")
    resources = validate_release_resources(candidate_dir, version)
    return {
        "manifest": manifest,
        "version": version,
        "baseline_version": baseline_version,
        "source_sha": manifest["source_sha"],
        "installer": installer,
        "installer_hash": installer_hash,
        "desktop_hash": desktop_hash,
        "fixture_hash": fixture_hash,
        "baseline_installer": fixture,
        "resources": resources,
        "resource_manifest_hash": _sha256(candidate_dir / "release-resources.json"),
    }


def validate_historical_baseline(baseline_dir: Path, candidate_version: str) -> dict[str, Any]:
    try:
        info = baseline_dir.lstat()
    except OSError:
        raise SmokeFailure("candidate_dir_invalid") from None
    require(stat.S_ISDIR(info.st_mode) and not baseline_dir.is_symlink() and not _is_reparse(info),
            "candidate_dir_invalid")
    manifest = _read_json(baseline_dir / "candidate.json",
                          "candidate_manifest_missing", "candidate_manifest_invalid")
    installer_name = manifest.get("installer")
    require(_safe_executable_basename(installer_name), "candidate_filename_unsafe")
    version = manifest.get("version")
    require(isinstance(version, str) and re.fullmatch(VERSION_PATTERN, version)
            and version != candidate_version, "candidate_metadata_invalid")
    require(type(manifest.get("schema")) is int and manifest.get("schema") == 1
            and manifest.get("track") == "W5"
            and re.fullmatch(r"[0-9a-f]{40}", str(manifest.get("source_sha", "")))
            and manifest.get("architecture") == "x86_64"
            and manifest.get("profile") == "release"
            and manifest.get("features") == ["custom-protocol"]
            and manifest.get("desktop_smoke") is False
            and manifest.get("authenticode") == "unsigned",
            "candidate_metadata_invalid")
    installer_hash = manifest.get("sha256")
    desktop_hash = manifest.get("desktop_sha256")
    require(isinstance(installer_hash, str) and re.fullmatch(r"[0-9a-f]{64}", installer_hash)
            and isinstance(desktop_hash, str) and re.fullmatch(r"[0-9a-f]{64}", desktop_hash),
            "candidate_hash_invalid")
    installer = baseline_dir / installer_name
    _regular_file(installer, "candidate_manifest_invalid")
    require(_sha256(installer) == installer_hash, "candidate_hash_mismatch")
    resources = validate_release_resources(baseline_dir, version)
    return {
        "manifest": manifest,
        "version": version,
        "source_sha": manifest["source_sha"],
        "installer": installer,
        "installer_hash": installer_hash,
        "desktop_hash": desktop_hash,
        "resources": resources,
        "resource_manifest_hash": _sha256(baseline_dir / "release-resources.json"),
    }


def runner_gate(env: Mapping[str, str], platform: str) -> tuple[Path, Path, Path]:
    require(platform == "win32", "windows_native_required")
    require(env.get("GITHUB_ACTIONS", "").casefold() == "true", "github_actions_required")
    require(env.get("RUNNER_ENVIRONMENT", "").casefold() == "github-hosted",
            "github_hosted_runner_required")
    runner_temp = env.get("RUNNER_TEMP", "")
    require(bool(runner_temp.strip()), "runner_temp_missing")
    local = env.get("LOCALAPPDATA", "")
    roaming = env.get("APPDATA", "")
    require(bool(local.strip()) and bool(roaming.strip()), "known_data_paths_unavailable")
    runner = Path(runner_temp)
    try:
        runner = runner.resolve(strict=True)
    except OSError:
        raise SmokeFailure("runner_temp_unavailable") from None
    _directory(runner, "runner_temp_unavailable")
    return runner, Path(local), Path(roaming)


def check_absent_user_state(paths: list[Path], registry_present: bool) -> None:
    for path in paths:
        require(not _lexists(path), "preexisting_appdata")
    require(registry_present is False, "preexisting_uninstall_entry")


def _winreg_module() -> Any:
    if sys.platform != "win32":
        raise SmokeFailure("windows_native_required")
    try:
        import winreg
    except ImportError:
        raise SmokeFailure("registry_unavailable") from None
    return winreg


def registry_values() -> dict[str, Any] | None:
    winreg = _winreg_module()
    key_path = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Chadex"
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, key_path, 0, winreg.KEY_READ) as key:
            result = {}
            for name in ("DisplayName", "DisplayVersion", "InstallLocation"):
                try:
                    result[name] = winreg.QueryValueEx(key, name)[0]
                except OSError:
                    result[name] = None
            return result
    except OSError as error:
        if getattr(error, "winerror", None) == 2:
            return None
        raise SmokeFailure("registry_access_failed") from None


def registry_entry_exists() -> bool:
    return registry_values() is not None


def _path_key(value: str | Path) -> str:
    text = str(value).replace("/", "\\")
    folded = text.casefold()
    if folded.startswith("\\\\?\\unc\\"):
        text = "\\\\" + text[8:]
    elif folded.startswith("\\\\?\\"):
        text = text[4:]
    return ntpath.normcase(ntpath.normpath(text))


def verify_registry_owner(expected_version: str, install_dir: Path) -> dict[str, bool]:
    value = registry_values()
    require(value is not None, "installed_registry_mismatch")
    flags = registry_owner_flags(value, expected_version, install_dir)
    require(all(flags.values()), "installed_registry_mismatch")
    return flags


def registry_owner_flags(value: Mapping[str, Any], expected_version: str,
                         install_dir: Path) -> dict[str, bool]:
    location = value.get("InstallLocation")
    if isinstance(location, str) and location.startswith('"') and location.endswith('"'):
        location = location[1:-1]
    flags = {
        "registry_owner_match": isinstance(value.get("DisplayName"), str)
        and value["DisplayName"].casefold() == "chadex",
        "registry_version_match": value.get("DisplayVersion") == expected_version,
        "registry_install_dir_match": isinstance(location, str)
        and _path_key(location) == _path_key(install_dir),
    }
    return flags


def verify_registry_removed() -> None:
    require(registry_values() is None, "uninstall_registry_remains")


def installer_arguments(installer: Path, install_dir: Path) -> str:
    return subprocess.list2cmdline([str(installer), "/S", "/NS"]) + f" /D={install_dir}"


def uninstaller_arguments(uninstaller: Path, install_dir: Path) -> str:
    return subprocess.list2cmdline([str(uninstaller), "/S"]) + f" _?={install_dir}"


def default_uninstaller_arguments(uninstaller: Path) -> str:
    return subprocess.list2cmdline([str(uninstaller), "/S"])


def child_environment(base: Mapping[str, str], port: int) -> dict[str, str]:
    env = dict(base)
    for key in tuple(env):
        upper = key.upper()
        if upper.startswith("CHADEX_DESKTOP_SMOKE"):
            env.pop(key, None)
        elif upper.startswith("CHADEX_") and any(part in upper for part in ("DATA", "RESOURCE")):
            env.pop(key, None)
        elif upper.startswith("WEBCODEX_") and (
                any(part in upper for part in ("DATA", "RESOURCE"))
                or upper.endswith("_BIN_DIR")):
            env.pop(key, None)
    env["CHADEX_DESKTOP_DEV_RESOURCES"] = ""
    env.pop("CHADEX_DESKTOP_DEV_RESOURCES", None)
    env["CHADEX_RUNTIME_BIN_DIR"] = ""
    env.pop("CHADEX_RUNTIME_BIN_DIR", None)
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = (
        f"--remote-debugging-port={port} --remote-debugging-address=127.0.0.1"
    )
    return env


@contextmanager
def installed_cdp_policy(port: int) -> Iterator[None]:
    """Own one app-specific policy only on the disposable native CI host.

    Elevated WebView2 150 ignores env/HKCU overrides. HKLM is honored without
    adding debug configuration or test IPC to the production executable.
    """
    require(sys.platform == "win32", "windows_native_required")
    require(os.environ.get("GITHUB_ACTIONS", "").casefold() == "true", "github_actions_required")
    require(os.environ.get("RUNNER_ENVIRONMENT", "").casefold() == "github-hosted",
            "github_hosted_runner_required")
    require(type(port) is int and 0 < port < 65536, "cdp_policy_failed")
    registry = _winreg_module()
    path = r"Software\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments"
    access = registry.KEY_READ | registry.KEY_SET_VALUE | registry.KEY_WOW64_64KEY
    value = f"--remote-debugging-port={port} --remote-debugging-address=127.0.0.1"
    written = False
    try:
        try:
            key = registry.OpenKey(registry.HKEY_LOCAL_MACHINE, path, 0, access)
        except FileNotFoundError:
            key = registry.CreateKeyEx(registry.HKEY_LOCAL_MACHINE, path, 0, access)
    except OSError:
        raise SmokeFailure("cdp_policy_failed") from None
    with key:
        try:
            try:
                for name in ("Chadex.exe", "app.chadex.windows", "*"):
                    try:
                        registry.QueryValueEx(key, name)
                    except FileNotFoundError:
                        continue
                    raise SmokeFailure("preexisting_cdp_policy")
                registry.SetValueEx(key, "Chadex.exe", 0, registry.REG_SZ, value)
                written = True
                require(registry.QueryValueEx(key, "Chadex.exe") == (value, registry.REG_SZ),
                        "cdp_policy_failed")
            except OSError:
                raise SmokeFailure("cdp_policy_failed") from None
            yield
        finally:
            # Setup failure after a write must clean it up as well.
            if written:
                try:
                    require(registry.QueryValueEx(key, "Chadex.exe") == (value, registry.REG_SZ),
                            "cdp_policy_cleanup_failed")
                    registry.DeleteValue(key, "Chadex.exe")
                    try:
                        registry.QueryValueEx(key, "Chadex.exe")
                    except FileNotFoundError:
                        pass
                    else:
                        raise SmokeFailure("cdp_policy_cleanup_failed")
                    # Never delete policy keys or other applications' values.
                    # An empty container has no override and is disposable CI state.
                except OSError:
                    raise SmokeFailure("cdp_policy_cleanup_failed") from None


def validate_default_paths(paths: Any, install_dir: Path, local_data: Path) -> None:
    require(isinstance(paths, dict), "installed_probe_invalid")
    helper = paths.get("helper")
    runtime = paths.get("runtime")
    data = paths.get("data")
    require(all(isinstance(item, str) and item for item in (helper, runtime, data)),
            "installed_probe_invalid")
    require(_path_key(helper) == _path_key(install_dir / "helper" / "chadex-helper.exe")
            and _path_key(runtime) == _path_key(install_dir / "chadex-runtime")
            and _path_key(data) == _path_key(local_data), "installed_path_mismatch")


def validate_probe_result(result: Any, *, install_dir: Path, local_data: Path,
                          version: str, mode: str,
                          owned: Mapping[int, dict[str, Any]],
                          current_rows: Mapping[int, dict[str, Any]]) -> dict[str, bool]:
    require(isinstance(result, dict), "installed_probe_invalid")
    require(result.get("version") == version, "installed_version_mismatch")
    validate_default_paths(result.get("paths"), install_dir, local_data)
    pid = result.get("helper_pid")
    require(type(pid) is int and pid > 0, "installed_helper_identity_missing")
    identity = owned.get(pid)
    require(identity is not None and identity.get("Name", "").casefold() == "chadex-helper.exe"
            and bool(identity.get("Created")), "installed_helper_identity_unowned")
    current = current_rows.get(pid)
    if current is not None:
        require(w2.same_process(current, identity), "installed_helper_identity_changed")
    require(result.get("runtime_ready") is True, "installed_runtime_not_ready")
    require(result.get("rendered") is True, "installed_ui_not_rendered")
    require(result.get("ui_state_ready") is True, "installed_ui_not_rendered")
    require(result.get("preferences_restored") is (mode == "restore"),
            "installed_preferences_not_restored")
    require(result.get("smoke_ipc_rejected") is True, "installed_smoke_ipc_exposed")
    return {
        "default_paths_verified": True,
        "rendered": True,
        "ui_state_ready": True,
        "runtime_ready": True,
        "preferences_restored": mode == "restore",
        "smoke_ipc_rejected": True,
    }


def verify_installed_resources(install_dir: Path, manifest: Mapping[str, Any],
                               resource_manifest_hash: str, desktop_hash: str) -> None:
    _directory(install_dir, "resource_file_missing")
    resources = manifest["resources"]
    for metadata in resources:
        relative = metadata["relative_path"]
        destination = _checked_relative(install_dir, relative, "resource_path_unsafe")
        info = _regular_file(destination, "resource_file_missing")
        require(info.st_size == metadata["size_bytes"]
                and _sha256(destination) == metadata["sha256"], "resource_hash_mismatch")
    installed_manifest = _checked_relative(install_dir, "release-resources.json", "resource_path_unsafe")
    require(_sha256(installed_manifest) == resource_manifest_hash, "resource_hash_mismatch")
    require(_sha256(install_dir / "Chadex.exe") == desktop_hash, "resource_hash_mismatch")


def data_snapshot(data_dir: Path) -> dict[str, tuple[str, int] | None]:
    _directory(data_dir, "appdata_missing")
    snapshot: dict[str, tuple[str, int] | None] = {}
    try:
        for current, directories, files in os.walk(data_dir, topdown=True, followlinks=False):
            current_path = Path(current)
            for name in tuple(directories):
                path = current_path / name
                try:
                    info = path.lstat()
                except OSError:
                    raise SmokeFailure("appdata_snapshot_invalid") from None
                require(stat.S_ISDIR(info.st_mode) and not path.is_symlink() and not _is_reparse(info),
                        "appdata_snapshot_invalid")
                snapshot[path.relative_to(data_dir).as_posix() + "/"] = None
            for name in files:
                path = current_path / name
                info = _regular_file(path, "appdata_snapshot_invalid")
                snapshot[path.relative_to(data_dir).as_posix()] = (_sha256(path), info.st_size)
    except OSError:
        raise SmokeFailure("appdata_snapshot_invalid") from None
    return snapshot


def require_unchanged_snapshot(data_dir: Path,
                              expected: Mapping[str, tuple[str, int] | None]) -> None:
    require(data_snapshot(data_dir) == dict(expected), "appdata_changed")


def preferences_hash(data_dir: Path) -> str:
    path = data_dir / "desktop-preferences.json"
    _regular_file(path, "preferences_missing")
    return _sha256(path)


def project_marker_hash(project: Path) -> str:
    return _sha256(project / "W5-preserve-marker.txt")


def _safe_tree_for_removal(root: Path) -> None:
    _directory(root, "fixture_cleanup_failed")
    for current, dirs, files in os.walk(root, topdown=True, followlinks=False):
        for name in [*dirs, *files]:
            path = Path(current) / name
            try:
                info = path.lstat()
            except OSError:
                raise SmokeFailure("fixture_cleanup_failed") from None
            require(not path.is_symlink() and not _is_reparse(info), "fixture_reparse_point")


def _remove_owned_fixture_dirs(paths: list[Path]) -> None:
    for path in paths:
        if not _lexists(path):
            continue
        _safe_tree_for_removal(path)
        try:
            shutil.rmtree(path)
        except OSError:
            raise SmokeFailure("data_cleanup_failed") from None
        require(not _lexists(path), "data_cleanup_failed")


def _port() -> int:
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
            listener.bind(("127.0.0.1", 0))
            return int(listener.getsockname()[1])
    except OSError:
        raise SmokeFailure("installed_probe_spawn_failed") from None


def _pipe_reader(stream: Any, cap: int, output: dict[str, Any], key: str) -> None:
    chunks: list[bytes] = []
    total = 0
    overflow = False
    try:
        while True:
            chunk = stream.read(4096)
            if not chunk:
                break
            if total < cap:
                keep = chunk[:cap - total]
                if keep:
                    chunks.append(keep)
            total += len(chunk)
            overflow = overflow or total > cap
    except OSError:
        overflow = True
    output[key] = (b"".join(chunks), overflow)


def _remember_tree(powershell: str, root_pid: int,
                   owned: dict[int, dict[str, Any]]) -> dict[int, dict[str, Any]]:
    try:
        return w2.remember_tree(powershell, root_pid, owned)
    except w2.E2EFailure as error:
        code = error.code if error.code in SAFE_CODES else "process_inventory_failed"
        raise SmokeFailure(code) from None
    except OSError:
        raise SmokeFailure("process_inventory_failed") from None


def _wait_owned_gone(powershell: str, owned: dict[int, dict[str, Any]],
                     timeout: float = 25) -> list[int]:
    try:
        return w2.wait_gone(powershell, owned, timeout=timeout)
    except w2.E2EFailure as error:
        code = error.code if error.code in SAFE_CODES else "process_inventory_failed"
        raise SmokeFailure(code) from None
    except OSError:
        raise SmokeFailure("process_inventory_failed") from None


def _new_owned_group(groups: list[dict[int, dict[str, Any]]]) -> dict[int, dict[str, Any]]:
    group: dict[int, dict[str, Any]] = {}
    groups.append(group)
    return group


def run_owned_executable(executable: Path, arguments: str, *, cwd: Path,
                         powershell: str, groups: list[dict[int, dict[str, Any]]],
                         timeout: float, timeout_code: str,
                         spawn_code: str, exit_code: str,
                         report: "SmokeReport", on_success: Any = None) -> None:
    group = _new_owned_group(groups)
    try:
        process = suspended.launch_owned(
            arguments, executable=executable, cwd=cwd, env=None,
            powershell=powershell, owned=group,
            on_forced=lambda: _increment_forced(report),
        )
    except suspended.LaunchFailure:
        raise SmokeFailure("process_inventory_failed") from None
    except OSError:
        raise SmokeFailure(spawn_code) from None
    deadline = time.monotonic() + timeout
    while process.poll() is None:
        _remember_tree(powershell, process.pid, group)
        if time.monotonic() >= deadline:
            try:
                process.kill()
                process.wait(timeout=10)
                report.value["cleanup"]["forced_count"] += 1
            except (OSError, subprocess.TimeoutExpired):
                raise SmokeFailure("process_cleanup_failed") from None
            raise SmokeFailure(timeout_code)
        time.sleep(0.2)
    code = process.returncode
    if code == 0 and on_success is not None:
        on_success()
    remaining = _wait_owned_gone(powershell, group)
    require(not remaining, "installer_processes_remain")
    require(code == 0, exit_code)


def run_installer(installer: Path, install_dir: Path, root: Path, *,
                  powershell: str, groups: list[dict[int, dict[str, Any]]],
                  report: "SmokeReport", on_success: Any = None) -> None:
    run_owned_executable(
        installer, installer_arguments(installer, install_dir), cwd=root,
        powershell=powershell, groups=groups, timeout=INSTALLER_TIMEOUT_SECONDS,
        timeout_code="installer_timeout", spawn_code="installer_spawn_failed",
        exit_code="installer_exit_nonzero", report=report, on_success=on_success,
    )


def run_uninstaller(install_dir: Path, root: Path, *, powershell: str,
                    groups: list[dict[int, dict[str, Any]]], report: "SmokeReport") -> str:
    uninstaller = install_dir / "uninstall.exe"
    _regular_file(uninstaller, "uninstaller_missing")
    original_hash = _sha256(uninstaller)
    run_owned_executable(
        uninstaller, uninstaller_arguments(uninstaller, install_dir), cwd=root,
        powershell=powershell, groups=groups, timeout=INSTALLER_TIMEOUT_SECONDS,
        timeout_code="installer_timeout", spawn_code="installer_spawn_failed",
        exit_code="uninstaller_exit_nonzero", report=report,
    )
    return original_hash


def run_default_uninstaller(install_dir: Path, root: Path, *, powershell: str,
                            groups: list[dict[int, dict[str, Any]]],
                            report: "SmokeReport") -> None:
    uninstaller = install_dir / "uninstall.exe"
    _regular_file(uninstaller, "uninstaller_missing")
    run_owned_executable(
        uninstaller, default_uninstaller_arguments(uninstaller), cwd=root,
        powershell=powershell, groups=groups, timeout=INSTALLER_TIMEOUT_SECONDS,
        timeout_code="installer_timeout", spawn_code="installer_spawn_failed",
        exit_code="uninstaller_exit_nonzero", report=report,
    )
    require(not _lexists(install_dir), "uninstall_files_remain")
    verify_registry_removed()


def _cleanup_default_install(install_dir: Path | None, *, install_attempted: bool,
                             install_succeeded: bool, uninstall_attempted: bool,
                             uninstall_succeeded: bool, root: Path | None,
                             powershell: str | None, groups: list[dict[int, dict[str, Any]]],
                             report: "SmokeReport") -> tuple[bool, bool]:
    """Fail-closed cleanup for the isolated default-NSIS uninstall fixture.

    A mutating uninstaller is attempted at most once.  If an installer/uninstaller
    already returned an error, cleanup only observes the desired absent state; it
    never blindly retries an uncertain mutation or recursively deletes its tree.
    """
    if not install_attempted:
        return uninstall_attempted, uninstall_succeeded
    require(install_dir is not None and root is not None and powershell is not None,
            "fixture_cleanup_failed")

    if install_succeeded and not uninstall_attempted:
        uninstall_attempted = True
        run_default_uninstaller(
            install_dir, root, powershell=powershell, groups=groups, report=report,
        )
        uninstall_succeeded = True

    # A failed/uncertain prior mutation is never replayed.  It is safe to remove the
    # parent temp root only when the application path and uninstall registration are
    # already gone by observation.
    require(not _lexists(install_dir), "fixture_cleanup_failed")
    verify_registry_removed()
    return uninstall_attempted, uninstall_succeeded


def _launch_and_probe(install_dir: Path, project: Path, root: Path, local_data: Path,
                     version: str, mode: str, *, powershell: str, node: str,
                     groups: list[dict[int, dict[str, Any]]],
                     report: "SmokeReport", port: int) -> dict[str, bool]:
    app = install_dir / "Chadex.exe"
    _regular_file(app, "installed_app_missing")
    probe = Path(__file__).resolve().with_name("windows_installed_probe.mjs")
    _regular_file(probe, "probe_script_missing")
    owned = _new_owned_group(groups)
    try:
        process = suspended.launch_owned(
            [str(app)], executable=app, cwd=root, env=child_environment(os.environ, port),
            powershell=powershell, owned=owned,
            on_forced=lambda: _increment_forced(report),
        )
    except suspended.LaunchFailure:
        raise SmokeFailure("process_inventory_failed") from None
    except OSError:
        raise SmokeFailure("installed_probe_spawn_failed") from None
    node_process: subprocess.Popen[bytes] | None = None
    readers: list[threading.Thread] = []
    captured: dict[str, Any] = {}
    node_killed = False
    try:
        _remember_tree(powershell, process.pid, owned)
        try:
            node_process = subprocess.Popen(
                [node, str(probe), str(port), str(project), mode, version],
                cwd=root, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
        except OSError:
            raise SmokeFailure("installed_probe_spawn_failed") from None
        assert node_process.stdout is not None and node_process.stderr is not None
        readers = [
            threading.Thread(target=_pipe_reader,
                             args=(node_process.stdout, MAX_PROBE_BYTES, captured, "stdout"), daemon=True),
            threading.Thread(target=_pipe_reader,
                             args=(node_process.stderr, 4096, captured, "stderr"), daemon=True),
        ]
        for reader in readers:
            reader.start()
        deadline = time.monotonic() + PROBE_TIMEOUT_SECONDS
        while node_process.poll() is None:
            _remember_tree(powershell, process.pid, owned)
            if time.monotonic() >= deadline:
                try:
                    node_process.kill()
                    node_process.wait(timeout=10)
                    node_killed = True
                    report.value["cleanup"]["forced_count"] += 1
                except (OSError, subprocess.TimeoutExpired):
                    raise SmokeFailure("node_cleanup_failed") from None
                raise SmokeFailure("installed_probe_timeout")
            time.sleep(0.2)
        return_code = node_process.wait(timeout=10)
        for reader in readers:
            reader.join(timeout=5)
        require(all(not reader.is_alive() for reader in readers), "node_cleanup_failed")
        output, overflow = captured.get("stdout", (b"", True))
        _stderr, stderr_overflow = captured.get("stderr", (b"", True))
        require(not overflow and not stderr_overflow, "installed_probe_invalid")
        if return_code != 0:
            code = _stderr.decode("utf-8", errors="replace").strip()
            current_stage = next((item for item in report.value["stages"] if item["status"] == "running"), None)
            if current_stage is not None and code in SAFE_PROBE_ERRORS:
                current_stage["probe_failure"] = code
        require(return_code == 0, "installed_probe_failed")
        require(len(output) > 0, "installed_probe_invalid")
        try:
            result = json.loads(output.decode("utf-8"))
        except (UnicodeError, ValueError):
            raise SmokeFailure("installed_probe_invalid") from None
        rows = _remember_tree(powershell, process.pid, owned)
        flags = validate_probe_result(
            result, install_dir=install_dir, local_data=local_data, version=version,
            mode=mode, owned=owned, current_rows=rows,
        )
        remaining = _wait_owned_gone(powershell, owned, timeout=25)
        require(not remaining, "installed_processes_remain")
        try:
            app_return_code = process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            raise SmokeFailure("installed_processes_remain") from None
        require(app_return_code == 0, "installed_app_exit_nonzero")
        return flags
    finally:
        if node_process is not None and node_process.poll() is None:
            try:
                node_process.kill()
                node_process.wait(timeout=10)
                if not node_killed:
                    report.value["cleanup"]["forced_count"] += 1
            except (OSError, subprocess.TimeoutExpired):
                raise SmokeFailure("node_cleanup_failed") from None
        if node_process is not None:
            for stream in (node_process.stdout, node_process.stderr):
                if stream is not None:
                    try:
                        stream.close()
                    except OSError:
                        pass
        for reader in readers:
            reader.join(timeout=1)


def launch_and_probe(install_dir: Path, project: Path, root: Path, local_data: Path,
                     version: str, mode: str, *, powershell: str, node: str,
                     groups: list[dict[int, dict[str, Any]]],
                     report: "SmokeReport") -> dict[str, bool]:
    port = _port()
    with installed_cdp_policy(port):
        first_group = len(groups)
        try:
            flags = _launch_and_probe(install_dir, project, root, local_data, version, mode,
                                      powershell=powershell, node=node, groups=groups,
                                      report=report, port=port)
        finally:
            _cleanup_process_groups(powershell, groups[first_group:], report)
    flags["cdp_policy_removed"] = True
    return flags


class SmokeReport:
    def __init__(self, platform: str, *, upgrade_type: str = "synthetic_metadata_upgrade",
                 external_checks_pending: list[str] | None = None) -> None:
        self.value: dict[str, Any] = {
            "schema": 1,
            "track": "W5",
            "status": "failed",
            "passed": False,
            "platform": "windows" if platform == "win32" else "non_windows",
            "upgrade_type": upgrade_type,
            "source_sha": None,
            "baseline_source_sha": None,
            "stages": [{"name": name, "status": "not_run"} for name in STAGES],
            "cleanup": {"owned_count": 0, "remaining_count": 0, "forced_count": 0},
            "external_checks_pending": list(EXTERNAL_CHECKS_PENDING if external_checks_pending is None
                                             else external_checks_pending),
        }

    @contextmanager
    def stage(self, name: str) -> Iterator[dict[str, Any]]:
        entry = next(item for item in self.value["stages"] if item["name"] == name)
        entry["status"] = "running"
        try:
            yield entry
        except SmokeFailure as error:
            entry["status"] = "failed"
            entry["error_code"] = error.code
            raise
        except Exception:
            entry["status"] = "failed"
            entry["error_code"] = "unexpected_exception"
            raise SmokeFailure("unexpected_exception") from None
        else:
            entry["status"] = "passed"

    def finish(self, success: bool) -> None:
        all_passed = all(item["status"] == "passed" for item in self.value["stages"])
        forced = self.value["cleanup"]["forced_count"]
        passed = success and all_passed and forced == 0
        self.value["passed"] = passed
        self.value["status"] = "passed" if passed else "failed"

    def public_value(self) -> dict[str, Any]:
        clean_stages = []
        for item in self.value["stages"]:
            status = item.get("status")
            row: dict[str, Any] = {
                "name": item["name"],
                "status": status if status in {"not_run", "running", "passed", "failed"}
                else "failed",
            }
            code = item.get("error_code")
            if isinstance(code, str):
                row["error_code"] = code if code in SAFE_CODES else "unclassified"
            probe_code = item.get("probe_failure")
            if isinstance(probe_code, str) and probe_code in SAFE_PROBE_ERRORS:
                row["probe_failure"] = probe_code
            for flag in (
                "default_paths_verified", "rendered", "ui_state_ready", "runtime_ready",
                "preferences_restored", "smoke_ipc_rejected", "resource_hashes_verified",
                "data_preserved", "registry_owner_match", "registry_version_match",
                "registry_install_dir_match", "registry_entry_removed", "project_marker_preserved",
                "fixture_appdata_cleaned", "uninstaller_stub_removed",
                "cdp_policy_removed", "default_uninstaller_self_copy_verified",
            ):
                if type(item.get(flag)) is bool:
                    row[flag] = item[flag]
            clean_stages.append(row)
        output: dict[str, Any] = {
            "schema": 1,
            "track": "W5",
            "status": self.value["status"],
            "passed": self.value["passed"],
            "platform": self.value["platform"],
            "upgrade_type": self.value["upgrade_type"],
            "uninstall_mode": "in_place_no_self_copy",
            "default_uninstaller_self_copy": "isolated_native_stage",
            "uninstaller_cleanup": "harness_after_exit",
            "stages": clean_stages,
            "cleanup": {
                key: max(0, value) if type(value) is int else 0
                for key, value in self.value["cleanup"].items()
                if key in {"owned_count", "remaining_count", "forced_count"}
            },
            "external_checks_pending": list(self.value["external_checks_pending"]),
        }
        source_sha = self.value.get("source_sha")
        if isinstance(source_sha, str) and re.fullmatch(r"[0-9a-f]{40}", source_sha):
            output["source_sha"] = source_sha
        baseline_source_sha = self.value.get("baseline_source_sha")
        if isinstance(baseline_source_sha, str) and re.fullmatch(r"[0-9a-f]{40}", baseline_source_sha):
            output["baseline_source_sha"] = baseline_source_sha
        return output


def _record_owned_count(report: SmokeReport,
                        groups: list[dict[int, dict[str, Any]]]) -> None:
    identities = {
        (pid, row.get("Created"))
        for group in groups for pid, row in group.items()
        if type(pid) is int and isinstance(row.get("Created"), str)
    }
    report.value["cleanup"]["owned_count"] = len(identities)


def _cleanup_process_groups(powershell: str | None,
                            groups: list[dict[int, dict[str, Any]]],
                            report: SmokeReport) -> None:
    if not groups:
        return
    if not powershell:
        report.value["cleanup"]["remaining_count"] = sum(len(group) for group in groups)
        raise SmokeFailure("process_cleanup_failed")
    remaining_count = 0
    cleanup_error: SmokeFailure | None = None
    for group in groups:
        if not group:
            continue
        try:
            remaining = _wait_owned_gone(powershell, group, timeout=3)
            if remaining:
                try:
                    forced = w2.force_cleanup(powershell, group)
                except (OSError, w2.E2EFailure):
                    cleanup_error = SmokeFailure("process_cleanup_failed")
                    forced = 0
                report.value["cleanup"]["forced_count"] += forced
                remaining = _wait_owned_gone(powershell, group, timeout=15)
            remaining_count += len(remaining)
        except SmokeFailure as error:
            cleanup_error = error
            remaining_count += len(group)
    report.value["cleanup"]["remaining_count"] = remaining_count
    if cleanup_error is not None:
        raise cleanup_error
    require(remaining_count == 0, "owned_processes_remain")


def _verify_uninstalled(install_dir: Path, manifest: Mapping[str, Any],
                        uninstaller_hash: str) -> None:
    uninstaller = install_dir / "uninstall.exe"
    paths = [install_dir / "Chadex.exe", install_dir / "release-resources.json"]
    paths.extend(install_dir.joinpath(*PurePosixPath(item["relative_path"]).parts)
                 for item in manifest["resources"])
    require(not any(_lexists(path) for path in paths), "uninstall_files_remain")
    _directory(install_dir, "uninstall_files_remain")
    _regular_file(uninstaller, "uninstall_files_remain")
    require(_sha256(uninstaller) == uninstaller_hash, "uninstall_files_remain")
    try:
        entries = list(install_dir.iterdir())
    except OSError:
        raise SmokeFailure("uninstall_files_remain") from None
    require(len(entries) == 1 and entries[0].name == "uninstall.exe",
            "uninstall_files_remain")


def _remove_uninstaller_stub(install_dir: Path, uninstaller_hash: str) -> None:
    _directory(install_dir, "fixture_cleanup_failed")
    uninstaller = install_dir / "uninstall.exe"
    _regular_file(uninstaller, "fixture_cleanup_failed")
    require(_sha256(uninstaller) == uninstaller_hash, "fixture_cleanup_failed")
    try:
        uninstaller.unlink()
        install_dir.rmdir()
    except OSError:
        raise SmokeFailure("fixture_cleanup_failed") from None
    require(not _lexists(uninstaller) and not _lexists(install_dir), "fixture_cleanup_failed")


def _remove_temp_root(root: Path, runner_temp: Path) -> None:
    try:
        resolved = root.resolve(strict=True)
        parent = runner_temp.resolve(strict=True)
        resolved.relative_to(parent)
    except (OSError, ValueError):
        raise SmokeFailure("fixture_cleanup_failed") from None
    _safe_tree_for_removal(resolved)
    try:
        shutil.rmtree(resolved)
    except OSError:
        raise SmokeFailure("fixture_cleanup_failed") from None
    require(not _lexists(resolved), "fixture_cleanup_failed")


def run_smoke(candidate_dir: Path, *, historical_baseline_dir: Path | None = None,
              env: Mapping[str, str] | None = None, platform: str | None = None) -> SmokeReport:
    environment = os.environ if env is None else env
    current_platform = sys.platform if platform is None else platform
    historical = historical_baseline_dir is not None
    pending = list(EXTERNAL_CHECKS_PENDING)
    if not historical:
        pending.append("historical_source_upgrade")
    report = SmokeReport(
        current_platform,
        upgrade_type="historical_source_upgrade" if historical else "synthetic_metadata_upgrade",
        external_checks_pending=pending,
    )
    runner_temp: Path | None = None
    local_app_data: Path | None = None
    roaming_app_data: Path | None = None
    powershell: str | None = None
    node: str | None = None
    candidate: dict[str, Any] | None = None
    baseline: dict[str, Any] | None = None
    root: Path | None = None
    install_dir: Path | None = None
    project: Path | None = None
    app_data: Path | None = None
    other_data_paths: list[Path] = []
    groups: list[dict[int, dict[str, Any]]] = []
    installer_succeeded = False
    installer_attempted = False
    uninstaller_attempted = False
    uninstaller_succeeded = False
    uninstaller_hash: str | None = None
    uninstall_verified = False
    default_install_dir: Path | None = None
    default_installer_attempted = False
    default_installer_succeeded = False
    default_uninstaller_attempted = False
    default_uninstaller_succeeded = False
    default_cleanup_verified = False
    main_success = False
    failure: SmokeFailure | None = None

    try:
        with report.stage("runner_preflight"):
            runner_temp, local_app_data, roaming_app_data = runner_gate(environment, current_platform)

        with report.stage("candidate_validation"):
            candidate = validate_candidate(
                candidate_dir, require_synthetic_fixture=historical_baseline_dir is None)
            report.value["source_sha"] = candidate["source_sha"]
            if historical_baseline_dir is not None:
                baseline = validate_historical_baseline(historical_baseline_dir, candidate["version"])
                baseline["probe_version"] = baseline["version"]
                report.value["baseline_source_sha"] = baseline["source_sha"]
            else:
                baseline = {
                    "version": candidate["baseline_version"],
                    "probe_version": candidate["version"],
                    "installer": candidate["baseline_installer"],
                    "desktop_hash": candidate["desktop_hash"],
                    "resources": candidate["resources"],
                    "resource_manifest_hash": candidate["resource_manifest_hash"],
                }

        with report.stage("user_state_preflight"):
            assert local_app_data is not None and roaming_app_data is not None
            app_data = local_app_data / "app.chadex.windows"
            other_data_paths = [roaming_app_data / "app.chadex.windows",
                                local_app_data / "Chadex"]
            check_absent_user_state(
                [app_data, *other_data_paths], registry_present=registry_entry_exists(),
            )

        with report.stage("runner_tools"):
            powershell = shutil.which("powershell.exe") or shutil.which("powershell")
            node = shutil.which("node.exe") or shutil.which("node")
            require(bool(powershell) and bool(node), "runner_tool_missing")
            probe = Path(__file__).resolve().with_name("windows_installed_probe.mjs")
            _regular_file(probe, "probe_script_missing")

        assert runner_temp is not None and app_data is not None and candidate is not None and baseline is not None
        with report.stage("fixture_setup"):
            try:
                root = Path(tempfile.mkdtemp(prefix="chadex-w5-", dir=runner_temp))
                install_dir = root / "安裝 Chadex"
                project = root / "專案 W5 中文 with spaces"
                project.mkdir()
                (project / "W5-preserve-marker.txt").write_text(
                    "W5 fixture marker must survive upgrade and uninstall\n", encoding="utf-8",
                )
            except OSError:
                raise SmokeFailure("fixture_setup_failed") from None

        assert root is not None and install_dir is not None and project is not None
        with report.stage("baseline_install"):
            installer_attempted = True

            def baseline_succeeded() -> None:
                nonlocal installer_succeeded
                installer_succeeded = True

            run_installer(baseline["installer"], install_dir, root,
                          powershell=powershell, groups=groups, report=report,
                          on_success=baseline_succeeded)
            require(installer_succeeded, "installer_exit_nonzero")
            flags = verify_registry_owner(baseline["version"], install_dir)
            next(item for item in report.value["stages"]
                 if item["name"] == "baseline_install").update(flags)

        with report.stage("baseline_resource_verification") as stage:
            verify_installed_resources(install_dir, baseline["resources"],
                                       baseline["resource_manifest_hash"], baseline["desktop_hash"])
            stage["resource_hashes_verified"] = True

        with report.stage("initial_installed_launch") as stage:
            flags = launch_and_probe(
                install_dir, project, root, app_data, baseline["probe_version"], "initial",
                powershell=powershell, node=node, groups=groups, report=report,
            )
            stage.update(flags)
            _directory(app_data, "appdata_missing")
            require(not any(_lexists(path) for path in other_data_paths),
                    "unexpected_appdata_created")
        initial_snapshot = data_snapshot(app_data)
        initial_preferences_hash = preferences_hash(app_data)
        marker_hash = project_marker_hash(project)

        with report.stage("candidate_upgrade_install"):
            installer_attempted = True
            run_installer(candidate["installer"], install_dir, root,
                          powershell=powershell, groups=groups, report=report)
            flags = verify_registry_owner(candidate["version"], install_dir)
            next(item for item in report.value["stages"]
                 if item["name"] == "candidate_upgrade_install").update(flags)

        with report.stage("upgrade_resource_and_data_verification") as stage:
            verify_installed_resources(install_dir, candidate["resources"],
                                       candidate["resource_manifest_hash"], candidate["desktop_hash"])
            require_unchanged_snapshot(app_data, initial_snapshot)
            require(preferences_hash(app_data) == initial_preferences_hash, "preferences_changed")
            require(project_marker_hash(project) == marker_hash, "project_marker_changed")
            stage["resource_hashes_verified"] = True
            stage["data_preserved"] = True
            stage["project_marker_preserved"] = True

        with report.stage("upgrade_restore_launch") as stage:
            stage.update(launch_and_probe(
                install_dir, project, root, app_data, candidate["version"], "restore",
                powershell=powershell, node=node, groups=groups, report=report,
            ))
            require(preferences_hash(app_data) == initial_preferences_hash, "preferences_changed")

        before_reinstall = data_snapshot(app_data)
        with report.stage("same_version_reinstall"):
            run_installer(candidate["installer"], install_dir, root,
                          powershell=powershell, groups=groups, report=report)
            verify_registry_owner(candidate["version"], install_dir)

        with report.stage("reinstall_resource_and_data_verification") as stage:
            verify_installed_resources(install_dir, candidate["resources"],
                                       candidate["resource_manifest_hash"], candidate["desktop_hash"])
            require_unchanged_snapshot(app_data, before_reinstall)
            require(preferences_hash(app_data) == initial_preferences_hash, "preferences_changed")
            require(project_marker_hash(project) == marker_hash, "project_marker_changed")
            stage["resource_hashes_verified"] = True
            stage["data_preserved"] = True
            stage["project_marker_preserved"] = True

        with report.stage("reinstall_restore_launch") as stage:
            stage.update(launch_and_probe(
                install_dir, project, root, app_data, candidate["version"], "restore",
                powershell=powershell, node=node, groups=groups, report=report,
            ))
            require(preferences_hash(app_data) == initial_preferences_hash, "preferences_changed")
        before_uninstall = data_snapshot(app_data)

        with report.stage("uninstall"):
            uninstaller_attempted = True
            uninstaller_hash = run_uninstaller(
                install_dir, root, powershell=powershell, groups=groups, report=report,
            )
            uninstaller_succeeded = True
            _verify_uninstalled(install_dir, candidate["resources"], uninstaller_hash)
            verify_registry_removed()
            uninstall_verified = True

        with report.stage("uninstall_preservation_verification") as stage:
            require_unchanged_snapshot(app_data, before_uninstall)
            require(preferences_hash(app_data) == initial_preferences_hash, "preferences_changed")
            require(project_marker_hash(project) == marker_hash, "project_marker_changed")
            stage["data_preserved"] = True
            stage["project_marker_preserved"] = True
            stage["registry_entry_removed"] = True

        with report.stage("default_uninstaller_self_copy") as stage:
            default_install_dir = root / "預設移除 Chadex"
            before_default_uninstall = data_snapshot(app_data)
            default_installer_attempted = True

            def default_install_succeeded() -> None:
                nonlocal default_installer_succeeded
                default_installer_succeeded = True

            run_installer(candidate["installer"], default_install_dir, root,
                          powershell=powershell, groups=groups, report=report,
                          on_success=default_install_succeeded)
            require(default_installer_succeeded, "installer_exit_nonzero")
            verify_registry_owner(candidate["version"], default_install_dir)
            default_uninstaller_attempted = True
            run_default_uninstaller(default_install_dir, root, powershell=powershell,
                                    groups=groups, report=report)
            default_uninstaller_succeeded = True
            default_cleanup_verified = True
            require_unchanged_snapshot(app_data, before_default_uninstall)
            require(preferences_hash(app_data) == initial_preferences_hash, "preferences_changed")
            require(project_marker_hash(project) == marker_hash, "project_marker_changed")
            stage["registry_entry_removed"] = True
            stage["data_preserved"] = True
            stage["project_marker_preserved"] = True
            stage["default_uninstaller_self_copy_verified"] = True

        main_success = True
    except SmokeFailure as error:
        failure = error
    except Exception:
        failure = SmokeFailure("unexpected_exception")

    try:
        with report.stage("fixture_cleanup") as cleanup_stage:
            _cleanup_process_groups(powershell, groups, report)

            if default_installer_attempted and not default_cleanup_verified:
                default_uninstaller_attempted, default_uninstaller_succeeded = _cleanup_default_install(
                    default_install_dir,
                    install_attempted=default_installer_attempted,
                    install_succeeded=default_installer_succeeded,
                    uninstall_attempted=default_uninstaller_attempted,
                    uninstall_succeeded=default_uninstaller_succeeded,
                    root=root, powershell=powershell, groups=groups, report=report,
                )
                default_cleanup_verified = True

            if installer_succeeded and not uninstaller_succeeded and not uninstaller_attempted:
                require(root is not None and install_dir is not None and candidate is not None,
                        "fixture_cleanup_failed")
                uninstaller_attempted = True
                uninstaller_hash = run_uninstaller(
                    install_dir, root, powershell=powershell, groups=groups, report=report,
                )
                uninstaller_succeeded = True
                _verify_uninstalled(install_dir, candidate["resources"], uninstaller_hash)
                verify_registry_removed()
                uninstall_verified = True

            if uninstaller_succeeded:
                require(app_data is not None, "data_cleanup_failed")
                _remove_owned_fixture_dirs([app_data, *other_data_paths])
                cleanup_stage["fixture_appdata_cleaned"] = True

            if uninstall_verified:
                require(root is not None and runner_temp is not None and install_dir is not None
                        and uninstaller_hash is not None, "fixture_cleanup_failed")
                if default_installer_attempted:
                    require(default_cleanup_verified and default_install_dir is not None
                            and not _lexists(default_install_dir), "fixture_cleanup_failed")
                _remove_uninstaller_stub(install_dir, uninstaller_hash)
                cleanup_stage["uninstaller_stub_removed"] = True
                _remove_temp_root(root, runner_temp)
            elif installer_succeeded:
                raise SmokeFailure("fixture_cleanup_failed")

            if root is not None and runner_temp is not None and install_dir is not None:
                if not installer_attempted:
                    _remove_temp_root(root, runner_temp)
                elif not installer_succeeded and not _lexists(install_dir):
                    _remove_temp_root(root, runner_temp)
                elif not uninstall_verified:
                    raise SmokeFailure("fixture_cleanup_failed")
    except SmokeFailure as error:
        if failure is None:
            failure = error

    _record_owned_count(report, groups)
    if failure is not None:
        current = next((item for item in report.value["stages"]
                        if item["status"] == "running"), None)
        if current is not None:
            current["status"] = "failed"
            current["error_code"] = failure.code
        elif not any(item.get("status") == "failed" for item in report.value["stages"]):
            report.value["stages"][0].update(status="failed", error_code=failure.code)
    if report.value["cleanup"]["forced_count"] > 0 and not any(
            item.get("status") == "failed" for item in report.value["stages"]):
        cleanup_stage = next(item for item in report.value["stages"]
                             if item["name"] == "fixture_cleanup")
        cleanup_stage.update(status="failed", error_code="forced_cleanup_required")
    report.finish(main_success and failure is None)
    return report


def write_report(path: Path, report: SmokeReport) -> None:
    output = report.public_value()
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_name(path.name + ".tmp")
        temporary.write_text(json.dumps(output, ensure_ascii=False, indent=2) + "\n",
                             encoding="utf-8")
        os.replace(temporary, path)
    except OSError:
        try:
            temporary.unlink(missing_ok=True)
        except (UnboundLocalError, OSError):
            pass
        raise SmokeFailure("fixture_cleanup_failed") from None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate-dir", type=Path, required=True)
    parser.add_argument("--historical-baseline-dir", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    report = run_smoke(args.candidate_dir, historical_baseline_dir=args.historical_baseline_dir)
    try:
        write_report(args.output, report)
    except SmokeFailure:
        return 2
    return 0 if report.value["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
