#!/usr/bin/env python3
"""Exercise the real Windows Tauri WebView and its owned helper/runtime tree."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
from typing import Any

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import windows_runtime_e2e as w2


CHECKPOINT_FIELDS = frozenset({
    "helper_running", "runtime_ready", "verified", "rendered", "selected_expected",
    "preferences_restored", "helper_pid", "secure_storage_roundtrip",
    "credential_deleted_after_helper_death",
})
FIXTURE_STAGES = (
    "webview_ready", "runtime_ready", "project_switched", "helper_restarted",
    "helper_crash_observed", "recovered", "app_restored", "startup_failure", "failed",
)
STAGES = (
    "preflight",
    "workflow.webview_ready", "workflow.runtime_ready", "workflow.project_switched",
    "workflow.project_a_operation", "workflow.project_b_operation",
    "workflow.helper_restarted", "workflow.helper_kill",
    "workflow.helper_crash_observed", "workflow.old_helper_tree_gone",
    "workflow.recovered", "workflow.recovered_project_operation", "workflow.shutdown",
    "restore_only.app_restored", "restore_only.persisted_project_identity",
    "restore_only.shutdown",
    "startup_failure.startup_failure", "startup_failure.shutdown",
    "force_exit.webview_ready", "force_exit.runtime_ready", "force_exit.project_operation",
    "force_exit.parent_terminated",
    "workflow.cleanup", "restore_only.cleanup", "startup_failure.cleanup", "force_exit.cleanup",
    "fixture_cleanup",
)
SAFE_ERROR_CODES = frozenset(w2.SAFE_FAILURES) | frozenset({
    "windows_native_required", "app_missing", "app_must_be_exe", "powershell_missing",
    "native_python_required", "fixture_setup_failed", "app_spawn_failed",
    "runtime_state_invalid", "runtime_identity_incomplete",
    "runtime_env_outside_isolation", "runtime_token_missing",
    "app_identity_missing", "helper_identity_missing", "helper_name_mismatch",
    "checkpoint_timeout", "app_exited_before_checkpoint", "checkpoint_invalid",
    "checkpoint_too_large", "checkpoint_schema_invalid", "checkpoint_value_invalid",
    "checkpoint_expectation_failed", "checkpoint_stage_failed", "checkpoint_ack_exists",
    "smoke_js_failed", "unexpected_readiness_checkpoint", "helper_restart_not_observed",
    "helper_pid_not_changed", "old_helper_tree_alive", "mcp_project_not_switched",
    "project_fixture_mismatch", "project_operation_failed", "project_result_mismatch",
    "desktop_shutdown_missing", "desktop_shutdown_invalid", "desktop_shutdown_not_graceful",
    "desktop_shutdown_not_clean", "unexpected_graceful_shutdown", "parent_termination_failed",
    "owned_processes_remain", "forced_cleanup_required", "fixture_cleanup_failed",
    "helper_identity_not_owned", "process_creation_identity_missing",
    "process_creation_identity_invalid", "process_ownership_ambiguous",
    "process_handle_open_failed", "process_handle_query_failed",
    "process_handle_termination_failed",
    "unexpected_exception", "unclassified",
})
SAFE_TOOL_NAMES = frozenset({"read_files", "run_process"})
CHECKPOINT_TIMEOUT = 105
MCP_TIMEOUT = 25
MAX_CHECKPOINT_BYTES = 16 * 1024


def require(condition: bool, code: str, **evidence: Any) -> None:
    if not condition:
        raise w2.E2EFailure(code, **evidence)


def new_report() -> w2.Report:
    report = w2.Report()
    report.value.update({
        "schema": 1,
        "track": "W3",
        "status": "failed",
        "passed": False,
        "platform": "windows" if sys.platform == "win32" else "non_windows",
        "scope": "real Tauri WebView, desktop restart, isolated local runtime",
        "stages": [{"name": name, "status": "not_run"} for name in STAGES],
        "tool_calls": [],
        "_owned_process_count": 0,
        "_owned_processes_remaining": 0,
        "_forced_count": 0,
    })
    return report


def sanitize_report(report: w2.Report) -> dict[str, Any]:
    """Project only allowlisted evidence; drop paths, IDs, outputs, hashes and tokens."""
    stages = []
    for item in report.value.get("stages", []):
        if not isinstance(item, dict) or item.get("name") not in STAGES:
            continue
        stage: dict[str, Any] = {
            "name": item["name"],
            "status": item.get("status") if item.get("status") in
            {"not_run", "running", "passed", "failed"} else "failed",
        }
        elapsed = item.get("elapsed_ms")
        if type(elapsed) is int and elapsed >= 0:
            stage["elapsed_ms"] = elapsed
        error = item.get("error_code")
        if isinstance(error, str):
            stage["error_code"] = error if error in SAFE_ERROR_CODES else "unclassified"
        evidence = item.get("evidence")
        if item.get("error_code") == "checkpoint_expectation_failed" and isinstance(evidence, dict):
            field = evidence.get("field")
            expected = evidence.get("expected")
            actual = evidence.get("actual")
            if (isinstance(field, str) and field in CHECKPOINT_FIELDS
                    and field != "helper_pid" and type(expected) is bool
                    and type(actual) is bool):
                stage["assertion"] = {
                    "field": field,
                    "expected": expected,
                    "actual": actual,
                }
        stages.append(stage)

    successful_names = sorted({
        call.get("tool") for call in report.value.get("tool_calls", [])
        if isinstance(call, dict) and call.get("success") is True
        and call.get("tool") in SAFE_TOOL_NAMES
    })
    successful_count = sum(
        1 for call in report.value.get("tool_calls", [])
        if isinstance(call, dict) and call.get("success") is True
        and call.get("tool") in SAFE_TOOL_NAMES
    )
    status = report.value.get("status")
    if status not in {"passed", "failed", "unsupported"}:
        status = "failed"
    forced = report.value.get("_forced_count", 0)
    owned = report.value.get("_owned_process_count", 0)
    return {
        "schema": 1,
        "track": "W3",
        "status": status,
        "passed": status == "passed" and report.value.get("passed") is True and forced == 0,
        "platform": "windows" if report.value.get("platform") == "windows" else "non_windows",
        "scope": "real Tauri WebView, desktop restart, isolated local runtime",
        "stages": stages,
        "tool_calls": {"count": successful_count, "successful_tool_names": successful_names},
        "owned_processes_observed": owned if type(owned) is int and owned >= 0 else 0,
        "owned_processes_remaining": report.value.get("_owned_processes_remaining", 0)
        if type(report.value.get("_owned_processes_remaining", 0)) is int
        and report.value.get("_owned_processes_remaining", 0) >= 0 else 0,
        "forced_count": forced if type(forced) is int and forced >= 0 else 0,
        "manual_checks_pending": [
            "native_picker", "explorer_open", "tray", "launch_at_login",
            "notifications", "credentialed_tunnel",
        ],
    }


def validate_checkpoint(payload: dict[str, Any], expected: dict[str, bool], *,
                        require_helper_pid: bool = False,
                        forbid_helper_pid: bool = False) -> int | None:
    require(set(payload) <= CHECKPOINT_FIELDS, "checkpoint_schema_invalid")
    for name, value in payload.items():
        if name == "helper_pid":
            require(type(value) is int and value > 0, "checkpoint_value_invalid")
        else:
            require(type(value) is bool, "checkpoint_value_invalid")
    for name, value in expected.items():
        actual = payload.get(name)
        require(type(actual) is bool and actual is value,
                "checkpoint_expectation_failed", field=name,
                expected=value, actual=actual)
    helper_pid = payload.get("helper_pid")
    if require_helper_pid:
        require(type(helper_pid) is int and helper_pid > 0, "helper_identity_missing")
    if forbid_helper_pid:
        require(helper_pid is None, "checkpoint_value_invalid")
    return helper_pid if type(helper_pid) is int else None


def checkpoint_payload(control: Path, stage: str, process: subprocess.Popen[Any],
                       timeout: float = CHECKPOINT_TIMEOUT) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    path = control / f"{stage}.json"
    failed = control / "failed.json"
    while time.monotonic() < deadline:
        require(not failed.exists(), "smoke_js_failed")
        if stage == "startup_failure":
            require(not (control / "runtime_ready.json").exists(),
                    "unexpected_readiness_checkpoint")
        if path.is_file():
            try:
                raw = path.read_bytes()
            except OSError:
                raise w2.E2EFailure("checkpoint_invalid") from None
            require(len(raw) <= MAX_CHECKPOINT_BYTES, "checkpoint_too_large")
            return w2.decode_object(raw, "checkpoint_invalid")
        require(process.poll() is None, "app_exited_before_checkpoint")
        time.sleep(0.1)
    raise w2.E2EFailure("checkpoint_timeout")


def acknowledge(control: Path, stage: str) -> None:
    path = control / f"{stage}.continue"
    require(not path.exists(), "checkpoint_ack_exists")
    try:
        path.write_bytes(b"continue")
    except OSError:
        raise w2.E2EFailure("checkpoint_invalid") from None


def clean_control(control: Path) -> None:
    for stage in FIXTURE_STAGES:
        for suffix in (".json", ".continue"):
            path = control / f"{stage}{suffix}"
            if path.exists():
                require(path.is_file() and not path.is_symlink(), "fixture_cleanup_failed")
                path.unlink()
    shutdown = control / "shutdown.json"
    if shutdown.exists():
        require(shutdown.is_file() and not shutdown.is_symlink(), "fixture_cleanup_failed")
        shutdown.unlink()
    abort = control / "abort"
    if abort.exists():
        require(abort.is_file() and not abort.is_symlink(), "fixture_cleanup_failed")
        abort.unlink()


def make_fixture(root: Path) -> dict[str, Path]:
    data = root / "data"
    control = root / "control"
    projects = root / "projects"
    project_a = projects / "專案 A with spaces"
    project_b = projects / "專案 B 中文 with spaces"
    for directory in (data, control, project_a, project_b):
        directory.mkdir(parents=True, exist_ok=True)
    (project_a / "smoke.txt").write_text("W3 project A initial marker", encoding="utf-8")
    (project_b / "smoke.txt").write_text("W3 project B 初始標記", encoding="utf-8")
    return {"root": root, "data": data, "control": control,
            "project_a": project_a, "project_b": project_b}


def app_environment(paths: dict[str, Path], scenario: str) -> dict[str, str]:
    environment = os.environ.copy()
    environment.pop("CHADEX_DESKTOP_DEV_RESOURCES", None)
    environment.update({
        "CHADEX_DESKTOP_SMOKE_DATA": str(paths["data"]),
        "CHADEX_DESKTOP_SMOKE_DIR": str(paths["control"]),
        "CHADEX_DESKTOP_SMOKE_PROJECT_A": str(paths["project_a"]),
        "CHADEX_DESKTOP_SMOKE_PROJECT_B": str(paths["project_b"]),
        "CHADEX_DESKTOP_SMOKE_SCENARIO": scenario,
    })
    if scenario == "startup_failure":
        environment["CHADEX_DESKTOP_DEV_RESOURCES"] = str(paths["root"] / "missing resources")
    return environment


def process_identity(powershell: str, pid: int, owned: dict[int, dict[str, Any]],
                     *, expected_name: str | None = None) -> dict[str, Any]:
    rows = w2.process_inventory(powershell)
    row = rows.get(pid)
    require(row is not None and bool(row.get("Created")), "helper_identity_missing")
    if expected_name is not None:
        require(str(row.get("Name", "")).casefold() == expected_name.casefold(),
                "helper_name_mismatch")
    # A checkpoint reports a PID, not ownership. The preceding desktop-tree
    # observation must already have established this exact incarnation.
    require(w2.same_process(row, owned.get(pid, {})), "helper_identity_not_owned")
    w2.remember_tree(powershell, pid, owned)
    return row


def track_desktop(powershell: str, process: subprocess.Popen[Any],
                  owned: dict[int, dict[str, Any]]) -> dict[str, Any]:
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise w2.E2EFailure("app_exited_before_checkpoint")
        rows = w2.process_inventory(powershell)
        identity = rows.get(process.pid)
        if identity is not None and identity.get("Created"):
            owned[process.pid] = identity
            w2.remember_tree(powershell, process.pid, owned)
            return identity
        time.sleep(0.1)
    raise w2.E2EFailure("app_identity_missing")


def launch_app(app: Path, paths: dict[str, Path], scenario: str,
               powershell: str, owned: dict[int, dict[str, Any]]) -> subprocess.Popen[Any]:
    clean_control(paths["control"])
    try:
        process = subprocess.Popen(
            [str(app)], cwd=app.parent, env=app_environment(paths, scenario),
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
    except OSError:
        raise w2.E2EFailure("app_spawn_failed") from None
    return process


def observed_checkpoint(report: w2.Report, stage_name: str, checkpoint_name: str,
                        process: subprocess.Popen[Any], paths: dict[str, Path],
                        powershell: str, owned: dict[int, dict[str, Any]],
                        expected: dict[str, bool], *, require_helper_pid: bool = False,
                        forbid_helper_pid: bool = False, ack: bool = True) -> tuple[dict[str, Any], dict[str, Any] | None]:
    with report.stage(stage_name):
        payload = checkpoint_payload(paths["control"], checkpoint_name, process)
        helper_pid = validate_checkpoint(payload, expected,
                                         require_helper_pid=require_helper_pid,
                                         forbid_helper_pid=forbid_helper_pid)
        w2.remember_tree(powershell, process.pid, owned)
        helper_identity = None
        if helper_pid is not None:
            helper_identity = process_identity(powershell, helper_pid, owned,
                                               expected_name="chadex-helper.exe")
        if ack:
            acknowledge(paths["control"], checkpoint_name)
        return payload, helper_identity


def windows_extended_path(value: str) -> str:
    """Match Rust canonicalize's namespace without changing filesystem isolation."""
    if value.startswith("\\\\?\\"):
        return value
    if value.startswith("\\\\"):
        return "\\\\?\\UNC\\" + value[2:]
    return "\\\\?\\" + value


def desktop_runtime_identity(data: Path) -> tuple[str, str, str]:
    # The native bridge passes a Rust-canonicalized data root to the helper.
    # pathlib retains an existing extended prefix but removes one it added;
    # both sides of W2's strict containment check must use the same namespace.
    if sys.platform == "win32":
        data = Path(windows_extended_path(str(data.resolve())))
    return w2.runtime_identity(data)


def mcp_for_data(data: Path, report: w2.Report) -> tuple[w2.McpClient, str]:
    url, token, project_id = desktop_runtime_identity(data)
    return w2.McpClient(url, token, report), project_id


def read_project_text(client: w2.McpClient, project_id: str, path: str) -> str:
    tool, arguments = w2.adaptive_mcp_call("read_files", {
        "project": project_id,
        "items": [{"path": path}],
        "include_read_revision": True,
        "max_result_bytes": 8192,
    })
    output, _success = client.invoke(tool, arguments, timeout=MCP_TIMEOUT)
    items = output.get("items")
    require(isinstance(items, list) and len(items) == 1 and isinstance(items[0], dict)
            and items[0].get("success") is True and isinstance(items[0].get("output"), dict),
            "project_operation_failed")
    text = project_file_content(items[0]["output"])
    return text


def project_file_content(output: dict[str, Any]) -> str:
    value = output.get("content")
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        blocks = [item.get("text") for item in value
                  if isinstance(item, dict) and item.get("type") == "text"
                  and isinstance(item.get("text"), str)]
        if len(blocks) == len(value) and blocks:
            return "".join(blocks)
    value = output.get("text")
    if isinstance(value, str):
        return value
    raise w2.E2EFailure("project_operation_failed")


def verify_project_operation(client: w2.McpClient, project_id: str,
                             marker: str, result_text: str, *, write_result: bool) -> None:
    require(read_project_text(client, project_id, "smoke.txt") == marker,
            "project_fixture_mismatch")
    if write_result:
        code = ("from pathlib import Path; import sys; "
                "Path('smoke-result.txt').write_text(sys.argv[1], encoding='utf-8'); "
                "print('W3_OPERATION_OK')")
        tool, arguments = w2.adaptive_mcp_call("run_process", {
            "project": project_id,
            "executable": sys.executable,
            "args": ["-X", "utf8", "-c", code, result_text],
            "timeout_secs": 20,
        })
        output, success = client.invoke(tool, arguments, timeout=MCP_TIMEOUT)
        w2.check_terminal(output, success, stdout="W3_OPERATION_OK")
    require(read_project_text(client, project_id, "smoke-result.txt") == result_text,
            "project_result_mismatch")


def terminate_exact(powershell: str, pid: int, identity: dict[str, Any], code: str) -> None:
    try:
        terminated = w2.terminate_identity(pid, identity)
    except (OSError, w2.E2EFailure):
        raise w2.E2EFailure(code) from None
    require(terminated, code)


def helper_tree(powershell: str, helper_pid: int,
                owned: dict[int, dict[str, Any]]) -> dict[int, dict[str, Any]]:
    rows = w2.process_inventory(powershell)
    pids = w2.descendants(rows, {helper_pid})
    tree = {pid: owned[pid] for pid in pids if pid in owned}
    require(helper_pid in tree, "helper_identity_missing")
    return tree


def wait_clean_shutdown(powershell: str, paths: dict[str, Path],
                        owned: dict[int, dict[str, Any]], report: w2.Report) -> None:
    remaining = w2.wait_gone(powershell, owned, timeout=25)
    report.value["_owned_processes_remaining"] = max(
        report.value["_owned_processes_remaining"], len(remaining))
    require(not remaining, "desktop_shutdown_not_clean", remaining_count=len(remaining))
    marker = paths["control"] / "shutdown.json"
    require(marker.is_file(), "desktop_shutdown_missing")
    try:
        raw = marker.read_bytes()
    except OSError:
        raise w2.E2EFailure("desktop_shutdown_invalid") from None
    shutdown = w2.decode_object(raw, "desktop_shutdown_invalid")
    require(set(shutdown) == {"graceful"} and type(shutdown.get("graceful")) is bool,
            "desktop_shutdown_invalid")
    require(shutdown["graceful"] is True, "desktop_shutdown_not_graceful")


def verify_persisted_project_identity(data: Path, project_b: Path,
                                      expected_project_id: str, report: w2.Report) -> None:
    """Verify the saved selection and project fixture without contacting the stopped runtime."""
    with report.stage("restore_only.persisted_project_identity"):
        _url, _token, restored_project_id = desktop_runtime_identity(data)
        require(restored_project_id == expected_project_id, "mcp_project_not_switched")
        try:
            persisted_result = (project_b / "smoke-result.txt").read_text(encoding="utf-8")
        except OSError:
            raise w2.E2EFailure("project_result_mismatch") from None
        require(persisted_result == "W3 operation B passed", "project_result_mismatch")


def run_workflow(app: Path, paths: dict[str, Path], powershell: str,
                 report: w2.Report) -> str:
    owned: dict[int, dict[str, Any]] = {}
    process: subprocess.Popen[Any] | None = None
    initial_helper: dict[str, Any] | None = None
    restarted_helper: dict[str, Any] | None = None
    project_a_id: str | None = None
    project_b_id: str | None = None
    failure: BaseException | None = None
    try:
        process = launch_app(app, paths, "workflow", powershell, owned)
        track_desktop(powershell, process, owned)
        observed_checkpoint(report, "workflow.webview_ready", "webview_ready", process,
                           paths, powershell, owned,
                           {"helper_running": True, "rendered": True,
                            "secure_storage_roundtrip": True}, require_helper_pid=True)
        _runtime_payload, initial_helper = observed_checkpoint(
            report, "workflow.runtime_ready", "runtime_ready", process, paths,
            powershell, owned,
            {"helper_running": True, "runtime_ready": True, "verified": False,
             "rendered": True, "selected_expected": True}, require_helper_pid=True,
            ack=False,
        )
        with report.stage("workflow.project_a_operation"):
            client_a, project_a_id = mcp_for_data(paths["data"], report)
            verify_project_operation(client_a, project_a_id,
                                     "W3 project A initial marker", "W3 operation A passed",
                                     write_result=True)
        acknowledge(paths["control"], "runtime_ready")

        observed_checkpoint(report, "workflow.project_switched", "project_switched", process,
                           paths, powershell, owned,
                           {"helper_running": True, "runtime_ready": True, "verified": False,
                            "rendered": True, "selected_expected": True},
                           require_helper_pid=True, ack=False)
        with report.stage("workflow.project_b_operation"):
            client_b, project_b_id = mcp_for_data(paths["data"], report)
            require(project_b_id != project_a_id, "mcp_project_not_switched")
            verify_project_operation(client_b, project_b_id,
                                     "W3 project B 初始標記", "W3 operation B passed",
                                     write_result=True)
        acknowledge(paths["control"], "project_switched")

        restarted_payload, restarted_helper = observed_checkpoint(
            report, "workflow.helper_restarted", "helper_restarted", process, paths,
            powershell, owned,
            {"helper_running": True, "runtime_ready": True, "verified": False, "rendered": True,
             "selected_expected": True, "preferences_restored": True},
            require_helper_pid=True, ack=False,
        )
        require(initial_helper is not None and restarted_helper is not None,
                "helper_identity_missing")
        require(not w2.same_process(restarted_helper, initial_helper),
                "helper_restart_not_observed")
        killed_helper_pid = int(restarted_payload["helper_pid"])
        with report.stage("workflow.helper_kill"):
            killed_helper_tree = helper_tree(powershell, killed_helper_pid, owned)
            terminate_exact(powershell, killed_helper_pid, restarted_helper,
                            "helper_restart_not_observed")
            require(not w2.wait_gone(powershell, {killed_helper_pid: restarted_helper}, timeout=15),
                    "helper_restart_not_observed")
        acknowledge(paths["control"], "helper_restarted")

        observed_checkpoint(report, "workflow.helper_crash_observed", "helper_crash_observed",
                           process, paths, powershell, owned,
                           {"helper_running": False, "runtime_ready": False,
                            "verified": False, "rendered": True,
                            "credential_deleted_after_helper_death": True},
                           forbid_helper_pid=True,
                           ack=False)
        with report.stage("workflow.old_helper_tree_gone"):
            require(not w2.wait_gone(powershell, killed_helper_tree, timeout=15),
                    "old_helper_tree_alive")
        acknowledge(paths["control"], "helper_crash_observed")

        recovered, recovered_helper = observed_checkpoint(
            report, "workflow.recovered", "recovered", process, paths, powershell,
            owned,
            {"helper_running": True, "runtime_ready": True, "verified": False,
             "rendered": True, "selected_expected": True}, require_helper_pid=True,
            ack=False,
        )
        require(recovered_helper is not None, "helper_identity_missing")
        require(not w2.same_process(recovered_helper, restarted_helper),
                "helper_restart_not_observed")
        require(project_b_id is not None, "mcp_project_not_switched")
        with report.stage("workflow.recovered_project_operation"):
            recovered_client, recovered_id = mcp_for_data(paths["data"], report)
            require(recovered_id == project_b_id, "mcp_project_not_switched")
            require(read_project_text(recovered_client, recovered_id, "smoke-result.txt")
                    == "W3 operation B passed", "project_result_mismatch")
        acknowledge(paths["control"], "recovered")

        with report.stage("workflow.shutdown"):
            wait_clean_shutdown(powershell, paths, owned, report)
        return project_b_id
    except BaseException as error:
        failure = error
        raise
    finally:
        cleanup_owned(report, "workflow.cleanup", powershell, paths["control"],
                      owned, process, failure)


def run_restore_only(app: Path, paths: dict[str, Path], powershell: str,
                     report: w2.Report, expected_project_id: str) -> None:
    owned: dict[int, dict[str, Any]] = {}
    process: subprocess.Popen[Any] | None = None
    failure: BaseException | None = None
    try:
        process = launch_app(app, paths, "restore_only", powershell, owned)
        track_desktop(powershell, process, owned)
        observed_checkpoint(report, "restore_only.app_restored", "app_restored", process,
                           paths, powershell, owned,
                            {"helper_running": True, "runtime_ready": False, "verified": False,
                            "rendered": True, "selected_expected": True,
                            "preferences_restored": True}, require_helper_pid=True, ack=False)
        verify_persisted_project_identity(paths["data"], paths["project_b"],
                                          expected_project_id, report)
        acknowledge(paths["control"], "app_restored")
        with report.stage("restore_only.shutdown"):
            wait_clean_shutdown(powershell, paths, owned, report)
    except BaseException as error:
        failure = error
        raise
    finally:
        cleanup_owned(report, "restore_only.cleanup", powershell, paths["control"],
                      owned, process, failure)


def run_startup_failure(app: Path, paths: dict[str, Path], powershell: str,
                        report: w2.Report) -> None:
    owned: dict[int, dict[str, Any]] = {}
    process: subprocess.Popen[Any] | None = None
    failure: BaseException | None = None
    try:
        process = launch_app(app, paths, "startup_failure", powershell, owned)
        track_desktop(powershell, process, owned)
        observed_checkpoint(report, "startup_failure.startup_failure", "startup_failure",
                           process, paths, powershell, owned,
                           {"helper_running": False, "runtime_ready": False,
                            "verified": False, "rendered": True})
        with report.stage("startup_failure.shutdown"):
            wait_clean_shutdown(powershell, paths, owned, report)
    except BaseException as error:
        failure = error
        raise
    finally:
        cleanup_owned(report, "startup_failure.cleanup", powershell, paths["control"],
                      owned, process, failure)


def run_force_exit(app: Path, paths: dict[str, Path], powershell: str,
                   report: w2.Report) -> None:
    owned: dict[int, dict[str, Any]] = {}
    process: subprocess.Popen[Any] | None = None
    failure: BaseException | None = None
    try:
        process = launch_app(app, paths, "force_exit", powershell, owned)
        track_desktop(powershell, process, owned)
        observed_checkpoint(report, "force_exit.webview_ready", "webview_ready", process,
                           paths, powershell, owned,
                           {"helper_running": True, "rendered": True,
                            "secure_storage_roundtrip": True}, require_helper_pid=True)
        _payload, helper_identity = observed_checkpoint(
            report, "force_exit.runtime_ready", "runtime_ready", process, paths,
            powershell, owned,
            {"helper_running": True, "runtime_ready": True, "verified": False,
             "rendered": True, "selected_expected": True},
            require_helper_pid=True, ack=False,
        )
        require(helper_identity is not None, "helper_identity_missing")
        with report.stage("force_exit.project_operation"):
            client, project_id = mcp_for_data(paths["data"], report)
            verify_project_operation(client, project_id,
                                     "W3 project A initial marker", "W3 force exit operation",
                                     write_result=True)
        with report.stage("force_exit.parent_terminated"):
            desktop_identity = owned.get(process.pid)
            require(desktop_identity is not None, "app_identity_missing")
            terminate_exact(powershell, process.pid, desktop_identity,
                            "parent_termination_failed")
            remaining = w2.wait_gone(powershell, owned, timeout=25)
            report.value["_owned_processes_remaining"] = max(
                report.value["_owned_processes_remaining"], len(remaining))
            require(not remaining,
                    "owned_processes_remain")
            require(not (paths["control"] / "shutdown.json").exists(),
                    "unexpected_graceful_shutdown")
    except BaseException as error:
        failure = error
        raise
    finally:
        cleanup_owned(report, "force_exit.cleanup", powershell, paths["control"],
                      owned, process, failure)


def cleanup_owned(report: w2.Report, stage_name: str, powershell: str,
                  control: Path, owned: dict[int, dict[str, Any]],
                  process: subprocess.Popen[Any] | None,
                  prior_failure: BaseException | None) -> None:
    with report.stage(stage_name):
        report.value["_owned_process_count"] += len(owned)
        if prior_failure is not None:
            try:
                (control / "abort").write_bytes(b"abort")
            except OSError:
                # Cleanup must not replace the failure that caused it.
                pass
        if owned:
            remaining = w2.wait_gone(powershell, owned, timeout=20)
            if remaining:
                forced = w2.force_cleanup(powershell, owned)
                report.value["_forced_count"] += forced
                remaining = w2.wait_gone(powershell, owned, timeout=10)
                require(not remaining, "owned_processes_remain", remaining_count=len(remaining))
                require(forced == 0, "forced_cleanup_required", forced_count=forced)
            report.value["_owned_processes_remaining"] = max(
                report.value["_owned_processes_remaining"], len(remaining))
        if process is not None and process.poll() is None:
            # The Popen handle is the exact process created by this harness.
            process.kill()
            process.wait(timeout=5)
            report.value["_forced_count"] += 1
            raise w2.E2EFailure("forced_cleanup_required", forced_count=1)
        if prior_failure is None:
            require(report.value["_forced_count"] == 0, "forced_cleanup_required")


def execute(app: Path, report: w2.Report, *, platform_name: str | None = None) -> None:
    platform_name = sys.platform if platform_name is None else platform_name
    if platform_name != "win32":
        report.value["status"] = "unsupported"
        try:
            with report.stage("preflight"):
                require(False, "windows_native_required")
        except w2.E2EFailure:
            pass
        report.finish()
        return

    powershell = shutil.which("powershell.exe") or shutil.which("pwsh.exe") or ""
    with report.stage("preflight"):
        require(bool(powershell), "powershell_missing")
        require(Path(sys.executable).suffix.casefold() == ".exe", "native_python_required")
        require(app.is_file(), "app_missing")
        require(app.suffix.casefold() == ".exe", "app_must_be_exe")

    root: Path | None = None
    try:
        root = Path(tempfile.mkdtemp(prefix="Chadex W3 中文 "))
        workflow_paths = make_fixture(root / "workflow")
        project_b_id = run_workflow(app, workflow_paths, powershell, report)

        # Keep the exact same data, projects and control root for a true app restart.
        run_restore_only(app, workflow_paths, powershell, report, project_b_id)

        startup_paths = make_fixture(root / "startup failure")
        run_startup_failure(app, startup_paths, powershell, report)

        force_paths = make_fixture(root / "force exit")
        run_force_exit(app, force_paths, powershell, report)
    except BaseException as error:
        if isinstance(error, w2.E2EFailure):
            report.value["_fatal_error_code"] = error.code
        else:
            report.value["_fatal_error_code"] = "unexpected_exception"
    finally:
        try:
            with report.stage("fixture_cleanup"):
                if root is not None:
                    shutil.rmtree(root)
        except OSError:
            report.value["_fatal_error_code"] = "fixture_cleanup_failed"
        except BaseException as error:
            report.value["_fatal_error_code"] = (
                error.code if isinstance(error, w2.E2EFailure) else "unexpected_exception")

    report.finish()
    if report.value.get("_fatal_error_code"):
        report.value["status"] = "failed"
        report.value["passed"] = False
        # Attach the safe code to the first stage that did not itself record a failure.
        if not any(item.get("status") == "failed" for item in report.value["stages"]):
            report.value["stages"][0]["status"] = "failed"
            report.value["stages"][0]["error_code"] = report.value["_fatal_error_code"]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", required=True, type=Path, help="debug Tauri desktop executable")
    parser.add_argument("--output", required=True, type=Path, help="safe JSON report destination")
    args = parser.parse_args(argv)
    report = new_report()
    try:
        execute(args.app.expanduser().resolve(), report)
    except BaseException as error:
        code = error.code if isinstance(error, w2.E2EFailure) else "unexpected_exception"
        report.value["status"] = "failed"
        report.value["passed"] = False
        if not any(item.get("status") == "failed" for item in report.value["stages"]):
            report.value["stages"][0].update(status="failed", error_code=code)
        report.finish()
        report.value["status"] = "failed"
        report.value["passed"] = False
    safe = sanitize_report(report)
    try:
        args.output.expanduser().resolve().parent.mkdir(parents=True, exist_ok=True)
        args.output.expanduser().resolve().write_text(
            json.dumps(safe, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    except OSError:
        platform = "windows" if sys.platform == "win32" else "non_windows"
        print(json.dumps({"schema": 1, "track": "W3", "status": "failed",
                          "passed": False, "platform": platform,
                          "error": "report_write_failed"}))
        return 1
    print(json.dumps(safe, ensure_ascii=False))
    if safe["status"] == "unsupported":
        return 2
    return 0 if safe["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
