#!/usr/bin/env python3
"""Automated Phase 19A long-haul pilot through the ChatGPT browser driver.

The campaign sends the prompt itself, waits for the final assistant turn, and
then aggregates the selected ChatGPT window from full Chadex request traces.
No stopwatch timing or manual prompt copy/paste is part of the measurement.
"""
from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

import phase19a_longhaul_profile as longhaul

DRIVER = ROOT / "scripts/webcodex_chatgpt_driver.mjs"
HEARTBEAT = ROOT / "scripts/phase19a_heartbeat.py"
DEFAULT_RUNTIME_STATE = Path.home() / "Library/Application Support/Chadex/runtime"
DEFAULT_NODE = (
    Path.home()
    / ".cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node"
)
DEFAULT_BROWSER_DESCRIPTOR = (
    Path.home() / ".codex-chatgpt-web/runtime/launcher-browser.json"
)
DEFAULT_BROWSER_LOCK = (
    Path.home() / ".codex-chatgpt-web/runtime/phase19a-browser.lock"
)


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f"{path.name}.tmp-{os.getpid()}")
    temporary.write_text(
        json.dumps(value, indent=2, ensure_ascii=False, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    os.replace(temporary, path)


def load_config(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError("config must be a JSON object")
    required = ("project_path", "connector", "client_id", "trace_root")
    for key in required:
        if not isinstance(value.get(key), str) or not value[key]:
            raise ValueError(f"config requires non-empty {key}")
    effort = value.get("reasoning_effort", "high")
    if effort not in {"high", "extra-high"}:
        raise ValueError("reasoning_effort must be high or extra-high")
    return value


def load_scenarios(path: Path) -> dict[str, dict[str, Any]]:
    value = json.loads(path.read_text(encoding="utf-8"))
    rows = value.get("scenarios") if isinstance(value, dict) else None
    if not isinstance(rows, list):
        raise ValueError("scenario manifest must contain scenarios[]")
    result: dict[str, dict[str, Any]] = {}
    for row in rows:
        if not isinstance(row, dict) or not isinstance(row.get("id"), str):
            raise ValueError("every scenario requires string id")
        result[row["id"]] = row
    return result


def scaled_duration(seconds: int | float, scale: float) -> float:
    return max(0.0, round(float(seconds) * scale, 3))


def workload_argv(scenario: dict[str, Any], scale: float) -> list[str] | None:
    workload = scenario.get("workload")
    if workload == "real_repo_task":
        return None
    argv = ["python3", "scripts/phase19a_pilot_workload.py", str(workload)]
    if "duration_seconds" in scenario:
        argv += [
            "--duration",
            str(scaled_duration(scenario["duration_seconds"], scale)),
        ]
    if "output_bytes" in scenario:
        argv += ["--output-bytes", str(int(scenario["output_bytes"]))]
    if scenario.get("fail_stage"):
        argv += ["--fail-stage", str(scenario["fail_stage"])]
    return argv


def shell_display(argv: list[str]) -> str:
    # Workload arguments are manifest-controlled literals/numbers. This is only
    # model-facing display; execution is requested through structured run_process.
    return " ".join(argv)


def scenario_prompt(
    *,
    scenario: dict[str, Any],
    project_path: Path,
    client_id: str,
    connector: str,
    marker: str,
    scale: float,
) -> str:
    argv = workload_argv(scenario, scale)
    if argv is None:
        raise ValueError(
            "P6 repository marathon requires a dedicated engineering task fixture; "
            "run it only after the P1-P5 instrumentation pilot is valid"
        )
    expected = scenario.get("expected_exit_code", 0)
    return (
        f"PHASE19A benchmark run {marker}. "
        f"Using the selected {connector} connector, first bootstrap the local project "
        f"at {project_path} on client {client_id}. "
        f"For that first work_on_project call, pass instruction exactly {marker!r}, "
        "include_project_instructions=false, and include_workflow_guidance=false. "
        "Do not modify project files. After bootstrap, execute exactly one structured "
        f"run_process for: {shell_display(argv)}. "
        "Use the bootstrapped project and preserve the returned execution/job identity. "
        "If the call hands off a durable job, follow its exact continuation until terminal; "
        "do not restart the command, do not search unrelated jobs, and do not substitute "
        "polling with a second execution. "
        f"The expected process exit code is {expected}; a non-zero expected exit is an "
        "intentional benchmark outcome, not a reason to edit or repair the project. "
        f"After observing terminal state, reply only PHASE19A_DONE {scenario['id']} {marker}."
    )


def pin_launcher(descriptor_path: Path) -> dict[str, Any]:
    value = json.loads(descriptor_path.read_text(encoding="utf-8"))
    pid = value.get("pid")
    endpoint = value.get("endpoint")
    if not isinstance(pid, int) or pid <= 0:
        raise RuntimeError("launcher descriptor has invalid pid")
    if not isinstance(endpoint, str) or not endpoint.startswith("http://127.0.0.1:"):
        raise RuntimeError("launcher descriptor has invalid endpoint")
    return {"pid": pid, "endpoint": endpoint.rstrip("/"), "path": str(descriptor_path)}


def driver_call(
    *,
    connector: str,
    reasoning_effort: str,
    prompt: str,
    run_dir: Path,
    node: Path,
    timeout_secs: int,
    launcher: dict[str, Any],
) -> dict[str, Any]:
    run_dir.mkdir(parents=True, exist_ok=True)
    prompt_path = run_dir / "prompt.txt"
    result_path = run_dir / "driver.json"
    progress_path = run_dir / "driver-progress.json"
    prompt_path.write_text(prompt, encoding="utf-8")
    env = os.environ.copy()
    env.update(
        {
            "BENCH_PROMPT_FILE": str(prompt_path),
            "BENCH_DRIVER_RESULT": str(result_path),
            "BENCH_DRIVER_PROGRESS": str(progress_path),
            "BENCH_WEBCODEX_CONNECTOR_NAME": connector,
            "BENCH_DRIVER_TIMEOUT_MS": str(timeout_secs * 1000),
            "BENCH_PREFLIGHT_ONLY": "0",
            "BENCH_REQUIRE_CONNECTOR_AT_COMPLETION": "0",
            "BENCH_RETURN_AFTER_ACCEPTED": "0",
            "BENCH_POST_ACCEPT_LINGER_MS": "0",
            "BENCH_REASONING_EFFORT": reasoning_effort,
            "BENCH_BROWSER_DESCRIPTOR": launcher["path"],
            "BENCH_EXPECT_LAUNCHER_PID": str(launcher["pid"]),
            "BENCH_EXPECT_LAUNCHER_ENDPOINT": launcher["endpoint"],
        }
    )
    try:
        completed = subprocess.run(
            [str(node), str(DRIVER)],
            cwd=ROOT,
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout_secs + 60,
            check=False,
        )
    except subprocess.TimeoutExpired as exc:
        result = (
            json.loads(progress_path.read_text(encoding="utf-8"))
            if progress_path.is_file()
            else {}
        )
        result.update(
            {
                "error": "driver_subprocess_timeout",
                "completed": False,
                "driver_exit_code": None,
                "driver_timed_out": True,
                "driver_finished_at_ms": time.time_ns() // 1_000_000,
                "prompt_bytes": len(prompt.encode("utf-8")),
                "prompt_sha256": hashlib.sha256(prompt.encode("utf-8")).hexdigest(),
                "driver_progress_path": str(progress_path),
            }
        )
        if exc.stdout:
            result["driver_stdout_tail"] = str(exc.stdout)[-2000:]
        if exc.stderr:
            result["driver_stderr_tail"] = str(exc.stderr)[-2000:]
        write_json(result_path, result)
        return result

    if result_path.is_file():
        result = json.loads(result_path.read_text(encoding="utf-8"))
    elif progress_path.is_file():
        result = json.loads(progress_path.read_text(encoding="utf-8"))
        result["error"] = "driver_result_missing_after_progress"
        result["completed"] = False
    else:
        result = {"error": "driver_result_missing", "completed": False}
    result["driver_exit_code"] = completed.returncode
    result["driver_finished_at_ms"] = time.time_ns() // 1_000_000
    result["prompt_bytes"] = len(prompt.encode("utf-8"))
    result["prompt_sha256"] = hashlib.sha256(prompt.encode("utf-8")).hexdigest()
    result["driver_progress_path"] = str(progress_path)
    if completed.stdout:
        result["driver_stdout_tail"] = completed.stdout[-2000:]
    if completed.stderr:
        result["driver_stderr_tail"] = completed.stderr[-2000:]
    write_json(result_path, result)
    return result


def driver_status(result: dict[str, Any]) -> str:
    if not isinstance(result.get("submitted_at_ms"), int):
        return "pre_submit_failure"
    final_ms = result.get("confirmed_final_at_ms")
    if not isinstance(final_ms, int):
        final_ms = result.get("visible_final_at_ms")
    if (
        result.get("driver_exit_code") == 0
        and result.get("completed") is True
        and isinstance(final_ms, int)
    ):
        return "completed"
    return "driver_failed_after_submit"


def start_heartbeat(
    *, run_dir: Path, config: dict[str, Any]
) -> tuple[subprocess.Popen[str], Path, Any]:
    run_dir.mkdir(parents=True, exist_ok=True)
    heartbeat_path = run_dir / "heartbeat.jsonl"
    stderr_path = run_dir / "heartbeat-stderr.log"
    state_dir = Path(config.get("runtime_state_dir", DEFAULT_RUNTIME_STATE)).expanduser()
    stderr_handle = stderr_path.open("w", encoding="utf-8")
    process = subprocess.Popen(
        [
            sys.executable,
            str(HEARTBEAT),
            "--state-dir",
            str(state_dir),
            "--client-id",
            config["client_id"],
            "--output",
            str(heartbeat_path),
            "--interval",
            "1.0",
        ],
        cwd=ROOT,
        stdout=subprocess.DEVNULL,
        stderr=stderr_handle,
        text=True,
    )
    deadline = time.monotonic() + 5.0
    while time.monotonic() < deadline:
        if heartbeat_path.is_file() and heartbeat_path.stat().st_size > 0:
            return process, heartbeat_path, stderr_handle
        if process.poll() is not None:
            stderr_handle.flush()
            stderr_handle.close()
            raise RuntimeError(
                f"heartbeat sampler exited before first sample; see {stderr_path}"
            )
        time.sleep(0.1)
    process.terminate()
    process.wait(timeout=5)
    stderr_handle.close()
    raise RuntimeError("heartbeat sampler did not produce a sample within 5 seconds")


def stop_heartbeat(process: subprocess.Popen[str] | None, stderr_handle: Any) -> None:
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
    if stderr_handle is not None:
        stderr_handle.close()


def acquire_browser_lock(path: Path):
    path.parent.mkdir(parents=True, exist_ok=True)
    handle = path.open("a+", encoding="utf-8")
    try:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError as exc:
        handle.seek(0)
        owner = handle.read().strip() or "unknown"
        handle.close()
        raise RuntimeError(
            f"another Phase 19A browser campaign already holds {path} (owner={owner})"
        ) from exc
    handle.seek(0)
    handle.truncate()
    handle.write(f"pid={os.getpid()} started_ms={time.time_ns() // 1_000_000}\n")
    handle.flush()
    return handle


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument(
        "--scenarios",
        type=Path,
        default=ROOT / "benchmarks/phase19a-longhaul/scenarios.json",
    )
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--scenario-id", action="append")
    parser.add_argument("--scale", type=float, default=1.0)
    parser.add_argument("--node", type=Path, default=DEFAULT_NODE)
    parser.add_argument(
        "--browser-descriptor", type=Path, default=DEFAULT_BROWSER_DESCRIPTOR
    )
    parser.add_argument("--heartbeat-jsonl", type=Path)
    parser.add_argument("--browser-lock", type=Path, default=DEFAULT_BROWSER_LOCK)
    parser.add_argument(
        "--allow-browser-automation",
        action="store_true",
        help=(
            "Explicit opt-in for ChatGPT browser automation. Phase 19A primary "
            "baseline uses phase19a_control_plane_campaign.py instead."
        ),
    )
    args = parser.parse_args()
    if args.scale <= 0:
        parser.error("--scale must be > 0")
    if not args.allow_browser_automation:
        parser.error(
            "browser automation is opt-in only; use "
            "scripts/phase19a_control_plane_campaign.py for the primary baseline, "
            "or pass --allow-browser-automation for a low-frequency E2E sanity run"
        )

    config = load_config(args.config)
    scenarios = load_scenarios(args.scenarios)
    selected = args.scenario_id or [
        key for key in scenarios if scenarios[key].get("workload") != "real_repo_task"
    ]
    unknown = [key for key in selected if key not in scenarios]
    if unknown:
        raise ValueError(f"unknown scenarios: {unknown}")

    project_path = Path(config["project_path"]).expanduser().resolve()
    trace_root = Path(config["trace_root"]).expanduser().resolve()
    if not project_path.is_dir():
        raise ValueError(f"missing project_path: {project_path}")
    if not trace_root.is_dir():
        raise ValueError(f"missing trace_root: {trace_root}")
    if not args.node.is_file():
        raise RuntimeError(f"Node.js not found: {args.node}")
    if not args.browser_descriptor.is_file():
        raise RuntimeError(
            "launcher browser is not running; start the fixed authenticated benchmark "
            f"browser first ({args.browser_descriptor})"
        )

    browser_lock_handle = acquire_browser_lock(args.browser_lock.expanduser())
    launcher = pin_launcher(args.browser_descriptor)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    write_json(args.output_dir / "launcher-identity.json", launcher)
    results: list[dict[str, Any]] = []

    for scenario_id in selected:
        scenario = scenarios[scenario_id]
        marker = f"PHASE19A_{scenario_id}_{uuid.uuid4().hex}"
        run_id = f"{scenario_id}-{uuid.uuid4().hex[:12]}"
        run_dir = args.output_dir / "runs" / run_id
        prompt = scenario_prompt(
            scenario=scenario,
            project_path=project_path,
            client_id=config["client_id"],
            connector=config["connector"],
            marker=marker,
            scale=args.scale,
        )
        duration = scaled_duration(scenario.get("duration_seconds", 0), args.scale)
        timeout_secs = max(180, int(duration) + 180)
        heartbeat_process = None
        heartbeat_handle = None
        heartbeat_path = args.heartbeat_jsonl
        if heartbeat_path is None:
            heartbeat_process, heartbeat_path, heartbeat_handle = start_heartbeat(
                run_dir=run_dir, config=config
            )
        try:
            driver = driver_call(
                connector=config["connector"],
                reasoning_effort=str(config.get("reasoning_effort", "high")),
                prompt=prompt,
                run_dir=run_dir,
                node=args.node,
                timeout_secs=timeout_secs,
                launcher=launcher,
            )
        finally:
            stop_heartbeat(heartbeat_process, heartbeat_handle)

        status = driver_status(driver)
        profile_result: dict[str, Any] | None = None
        profile_error: str | None = None
        if isinstance(driver.get("submitted_at_ms"), int):
            try:
                profile_result = longhaul.profile(
                    run_dir / "driver.json",
                    trace_root,
                    run_dir / "profile",
                    run_id,
                    marker=marker,
                    expected_tool_name="work_on_project",
                    marker_argument_key="instruction",
                    heartbeat_path=heartbeat_path,
                )
            except Exception as exc:
                profile_error = f"{type(exc).__name__}: {exc}"

        result_row = {
            "run_id": run_id,
            "scenario_id": scenario_id,
            "marker": marker,
            "status": status,
            "driver": str((run_dir / "driver.json").relative_to(args.output_dir)),
            "profile": (
                str((run_dir / "profile").relative_to(args.output_dir))
                if profile_result is not None
                else None
            ),
            "heartbeat": str(heartbeat_path.relative_to(args.output_dir))
            if heartbeat_path is not None
            and heartbeat_path.is_relative_to(args.output_dir)
            else str(heartbeat_path) if heartbeat_path is not None else None,
            "driver_exit_code": driver.get("driver_exit_code"),
            "driver_error": driver.get("error"),
            "profile_error": profile_error,
            "metrics": profile_result["metrics"] if profile_result is not None else None,
        }
        results.append(result_row)
        write_json(args.output_dir / "manifest.json", {"runs": results})

    print(json.dumps({"runs": results}, indent=2, sort_keys=True))
    fcntl.flock(browser_lock_handle.fileno(), fcntl.LOCK_UN)
    browser_lock_handle.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
