#!/usr/bin/env python3
"""Run the Phase 16C matched first-tool latency campaign.

Each measured turn asks ChatGPT to make exactly one work_on_project call carrying
one globally unique marker. The existing exact-ID profiler then selects the
client window by that marker and joins request boundaries only by
client_window_key/server_trace_id. No timestamp-nearest request matching is used.
"""
from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import random
import subprocess
import sys
import time
import uuid
from typing import Any

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

import phase16a_first_tool_profile as exact_profile
import phase16c_variance_profile as variance

DRIVER = ROOT / "scripts/webcodex_chatgpt_driver.mjs"
DEFAULT_NODE = (
    Path.home()
    / ".cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node"
)
HARNESS_ORDER = ("chadex", "webcodex")
GLOBAL_CAMPAIGN_LOCK = ROOT / "docs/performance/.phase16c-connected-campaign.lock"
DEFAULT_BROWSER_DESCRIPTOR = Path.home() / ".codex-chatgpt-web/runtime/launcher-browser.json"


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f"{path.name}.tmp-{os.getpid()}")
    temporary.write_text(
        json.dumps(value, indent=2, ensure_ascii=False, sort_keys=True) + "\n"
    )
    temporary.chmod(0o600)
    os.replace(temporary, path)


def pin_launcher_identity(
    output_dir: Path,
    descriptor_path: Path | None = None,
) -> dict[str, Any]:
    descriptor_path = (
        descriptor_path
        or Path(os.environ.get("BENCH_BROWSER_DESCRIPTOR", DEFAULT_BROWSER_DESCRIPTOR))
    ).expanduser().resolve()
    if not descriptor_path.is_file():
        raise RuntimeError(
            "Codex Web GPT launcher browser is not running; "
            f"missing descriptor {descriptor_path}"
        )
    descriptor = json.loads(descriptor_path.read_text(encoding="utf-8"))
    pid = descriptor.get("pid")
    endpoint = descriptor.get("endpoint")
    if not isinstance(pid, int) or pid <= 0:
        raise RuntimeError("launcher descriptor has invalid pid")
    if not isinstance(endpoint, str) or not endpoint.startswith("http://127.0.0.1:"):
        raise RuntimeError("launcher descriptor has invalid local endpoint")
    endpoint = endpoint.rstrip("/")
    identity = {
        "schema_version": 1,
        "descriptor_path": str(descriptor_path),
        "pid": pid,
        "endpoint": endpoint,
    }
    identity_path = output_dir / "launcher-identity.json"
    if identity_path.is_file():
        expected = json.loads(identity_path.read_text(encoding="utf-8"))
        if expected != identity:
            raise RuntimeError(
                "launcher browser identity changed during Phase 16C; "
                "campaign must fail closed instead of restarting the browser"
            )
    else:
        write_json(identity_path, identity)
    os.environ["BENCH_BROWSER_DESCRIPTOR"] = str(descriptor_path)
    os.environ["BENCH_EXPECT_LAUNCHER_PID"] = str(pid)
    os.environ["BENCH_EXPECT_LAUNCHER_ENDPOINT"] = endpoint
    return identity


def balanced_orders(pairs: int, seed: int) -> list[tuple[str, str]]:
    if pairs < 2 or pairs % 2:
        raise ValueError("pairs must be an even integer >= 2")
    orders = [
        ("chadex", "webcodex")
        for _ in range(pairs // 2)
    ] + [
        ("webcodex", "chadex")
        for _ in range(pairs // 2)
    ]
    random.Random(seed).shuffle(orders)
    return orders


def new_run_limit_reached(completed: int, limit: int | None) -> bool:
    return limit is not None and completed >= limit


def stage_needs_launcher_lock(stage: str) -> bool:
    return stage in {"all", "submit"}


def validate_config(payload: dict[str, Any]) -> tuple[Path, dict[str, dict[str, Any]]]:
    project_path = payload.get("project_path")
    arms = payload.get("arms")
    if not isinstance(project_path, str) or not project_path:
        raise ValueError("config requires non-empty project_path")
    project = Path(project_path).expanduser().resolve()
    if not project.is_dir():
        raise ValueError(f"project_path is not a directory: {project}")
    if not isinstance(arms, list) or len(arms) != 2:
        raise ValueError("config requires exactly two arms")

    by_harness: dict[str, dict[str, Any]] = {}
    connectors: set[str] = set()
    for raw in arms:
        if not isinstance(raw, dict):
            raise ValueError("every arm must be an object")
        harness = raw.get("harness")
        connector = raw.get("connector")
        client_id = raw.get("client_id")
        trace_root = raw.get("trace_root")
        if harness not in HARNESS_ORDER or harness in by_harness:
            raise ValueError("arms must contain one chadex and one webcodex harness")
        for label, value in (
            ("connector", connector),
            ("client_id", client_id),
            ("trace_root", trace_root),
        ):
            if not isinstance(value, str) or not value:
                raise ValueError(f"{harness} requires non-empty {label}")
        if connector in connectors:
            raise ValueError("connector names must be distinct")
        trace_path = Path(trace_root).expanduser().resolve()
        if not trace_path.is_dir():
            raise ValueError(f"trace_root is not a directory: {trace_path}")

        connector_state = raw.get("connector_state", "unknown")
        registry_state = raw.get("registry_state", "unknown")
        chat_state = raw.get("chat_state", "fresh")
        if connector_state not in variance.VALID_TEMPERATURE_STATES:
            raise ValueError(f"invalid connector_state for {harness}")
        if registry_state not in variance.VALID_TEMPERATURE_STATES:
            raise ValueError(f"invalid registry_state for {harness}")
        if chat_state not in variance.VALID_CHAT_STATES:
            raise ValueError(f"invalid chat_state for {harness}")

        arm = dict(raw)
        arm["trace_root"] = str(trace_path)
        arm["connector_state"] = connector_state
        arm["registry_state"] = registry_state
        arm["chat_state"] = chat_state
        by_harness[harness] = arm
        connectors.add(connector)

    if set(by_harness) != set(HARNESS_ORDER):
        raise ValueError("config must include chadex and webcodex arms")
    return project, by_harness


def probe_prompt(arm: dict[str, Any], project_path: Path, marker: str) -> str:
    return (
        f"Using the selected {arm['connector']} connector, bootstrap the local project "
        f"at {project_path} on client {arm['client_id']}. "
        f"The work instruction for this diagnostic is exactly {marker}. "
        "I need the connector's work_on_project bootstrap result, so do not answer from "
        "general knowledge. Make that bootstrap the first and only tool call. Pass "
        "include_project_instructions false and include_workflow_guidance false. "
        "Do not inspect, edit, run, or otherwise change project files after bootstrap. "
        "Then reply only PHASE16C_OK."
    )


def driver_call(
    *,
    connector: str,
    prompt: str,
    run_dir: Path,
    node: Path,
    timeout_secs: int,
    preflight: bool,
) -> tuple[dict[str, Any], int]:
    run_dir.mkdir(parents=True, exist_ok=True)
    prompt_path = run_dir / "prompt.txt"
    result_path = run_dir / "driver.json"
    prompt_path.write_text(prompt, encoding="utf-8")
    prompt_path.chmod(0o600)
    env = os.environ.copy()
    env.update({
        "BENCH_PROMPT_FILE": str(prompt_path),
        "BENCH_DRIVER_RESULT": str(result_path),
        "BENCH_WEBCODEX_CONNECTOR_NAME": connector,
        "BENCH_DRIVER_TIMEOUT_MS": str(timeout_secs * 1000),
        "BENCH_PREFLIGHT_ONLY": "1" if preflight else "0",
        "BENCH_REQUIRE_CONNECTOR_AT_COMPLETION": "0",
        "BENCH_RETURN_AFTER_ACCEPTED": "0" if preflight else "1",
        "BENCH_POST_ACCEPT_LINGER_MS": (
            "0"
            if preflight
            else os.environ.get("PHASE16C_POST_ACCEPT_LINGER_MS", "0")
        ),
    })
    completed = subprocess.run(
        [str(node), str(DRIVER)],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout_secs + 60,
        check=False,
    )
    result = (
        json.loads(result_path.read_text(encoding="utf-8"))
        if result_path.is_file()
        else {"error": "driver_result_missing"}
    )
    result["driver_exit_code"] = completed.returncode
    result["prompt_bytes"] = len(prompt.encode("utf-8"))
    result["prompt_sha256"] = hashlib.sha256(prompt.encode("utf-8")).hexdigest()
    if completed.stdout:
        result["driver_stdout_tail"] = completed.stdout[-2000:]
    if completed.stderr:
        result["driver_stderr_tail"] = completed.stderr[-2000:]
    write_json(result_path, result)
    return result, completed.returncode


def exact_profile_with_retry(
    *,
    driver_path: Path,
    trace_root: Path,
    marker: str,
    wait_secs: float,
) -> dict[str, Any]:
    deadline = time.monotonic() + wait_secs
    last_error: Exception | None = None
    while True:
        try:
            result = exact_profile.profile(
                driver_path,
                trace_root,
                [marker],
                None,
                expected_tool_name="work_on_project",
                marker_argument_key="instruction",
            )
            first = result.get("first_tool")
            handler_ms = (
                first.get("server_total_to_handler_return_ms")
                if isinstance(first, dict)
                else None
            )
            if not isinstance(handler_ms, (int, float)):
                raise ValueError("exact first-tool trace has not reached handler return")
            return result
        except ValueError as exc:
            last_error = exc
            if time.monotonic() >= deadline:
                raise RuntimeError(
                    f"exact marker profile did not materialize within {wait_secs:.1f}s: {exc}"
                ) from exc
            time.sleep(0.25)
    raise AssertionError(last_error)


def durable_submission_checkpoint(driver: dict[str, Any]) -> bool:
    return (
        driver.get("submission_checkpoint_version") == 1
        and driver.get("submission_committed") is True
        and driver.get("driver_self_reported_stage") == "accepted"
        and driver.get("submitted_at_ms") is not None
        and driver.get("accepted_at_ms") is not None
    )


def validate_driver_turn(*, harness: str, connector: str, driver: dict[str, Any]) -> None:
    durable_submission = durable_submission_checkpoint(driver)
    if driver.get("driver_exit_code") != 0 and not durable_submission:
        raise RuntimeError(
            f"{harness} browser driver failed: {driver.get('error', 'unknown error')}"
        )
    accepted_only = (
        driver.get("accepted") is True
        and driver.get("completion_mode") == "submission-only"
    )
    if driver.get("completed") is not True and not accepted_only:
        raise RuntimeError(f"{harness} browser turn did not complete")
    if driver.get("temporary_chat") is not True:
        raise RuntimeError(f"{harness} turn was not Temporary Chat")
    if driver.get("connector") != connector:
        raise RuntimeError(f"{harness} connector changed during the turn")
    if driver.get("submission_count") != 1:
        raise RuntimeError(f"{harness} did not submit exactly one prompt")


def validate_measured_turn(
    *,
    harness: str,
    connector: str,
    driver: dict[str, Any],
    profile: dict[str, Any],
) -> None:
    validate_driver_turn(harness=harness, connector=connector, driver=driver)
    first = profile.get("first_tool")
    if not isinstance(first, dict) or first.get("tool_name") != "work_on_project":
        raise RuntimeError(f"{harness} first tool was not work_on_project")
    if harness == "chadex":
        timing = first.get("helper_timing")
        if (
            not isinstance(timing, dict)
            or timing.get("source") != "server_projected_exact_trace"
        ):
            raise RuntimeError(
                "Chadex profile lacks Phase 16C server-projected helper timing; "
                "the instrumented build is not active or full tracing is unavailable"
            )


def make_plan(
    *,
    pairs: int,
    seed: int,
    arms: dict[str, dict[str, Any]],
    project_path: Path,
) -> dict[str, Any]:
    campaign_id = uuid.uuid4().hex[:16]
    schedule = []
    for pair_index, order in enumerate(balanced_orders(pairs, seed), start=1):
        pair_id = f"pair{pair_index:02d}"
        runs = []
        for position, harness in enumerate(order, start=1):
            marker = "PHASE16C_" + uuid.uuid4().hex
            prompt = probe_prompt(arms[harness], project_path, marker)
            runs.append({
                "run_id": f"{pair_id}-{harness}",
                "pair_id": pair_id,
                "harness": harness,
                "order_position": position,
                "marker": marker,
                "prompt_bytes": len(prompt.encode("utf-8")),
            })
        schedule.append({"pair_id": pair_id, "order": list(order), "runs": runs})
    return {
        "schema_version": 1,
        "campaign_id": campaign_id,
        "seed": seed,
        "pairs": pairs,
        "balanced_ab_ba": True,
        "schedule": schedule,
    }


def preflight(
    *,
    arms: dict[str, dict[str, Any]],
    project_path: Path,
    output_dir: Path,
    node: Path,
    timeout_secs: int,
) -> None:
    for harness in HARNESS_ORDER:
        arm = arms[harness]
        marker = "PHASE16C_PREFLIGHT_" + uuid.uuid4().hex
        prompt = probe_prompt(arm, project_path, marker)
        result, code = driver_call(
            connector=arm["connector"],
            prompt=prompt,
            run_dir=output_dir / "preflight" / harness,
            node=node,
            timeout_secs=timeout_secs,
            preflight=True,
        )
        if code != 0 or result.get("ready") is not True:
            raise RuntimeError(
                f"{harness} connector preflight failed: "
                f"{result.get('error', 'unknown error')}"
            )


def available_attempt_dir(base: Path) -> Path:
    if not base.exists() or not any(base.iterdir()):
        return base
    index = 2
    while True:
        candidate = base / f"attempt-{index:02d}"
        if not candidate.exists():
            return candidate
        index += 1


def submitted_attempt(base: Path) -> tuple[Path, dict[str, Any]] | None:
    candidates = [base]
    if base.is_dir():
        candidates.extend(
            sorted(
                path
                for path in base.iterdir()
                if path.is_dir() and path.name.startswith("attempt-")
            )
        )
    submitted: list[tuple[Path, dict[str, Any]]] = []
    for run_dir in candidates:
        driver_path = run_dir / "driver.json"
        if not driver_path.is_file():
            continue
        try:
            driver = json.loads(driver_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise RuntimeError(f"invalid driver result at {driver_path}: {exc}") from exc
        legacy_clean_submission = (
            driver.get("driver_exit_code") == 0
            and driver.get("submitted_at_ms") is not None
        )
        if durable_submission_checkpoint(driver) or legacy_clean_submission:
            submitted.append((run_dir, driver))
    if len(submitted) > 1:
        raise RuntimeError(
            f"{base.name} has multiple submitted attempts; refusing ambiguous resume"
        )
    return submitted[0] if submitted else None


def _run_campaign_locked(args: argparse.Namespace) -> dict[str, Any]:
    config_payload = json.loads(args.config.read_text(encoding="utf-8"))
    project_path, arms = validate_config(config_payload)
    if not args.node.is_file() or not DRIVER.is_file():
        raise RuntimeError("Node.js or browser driver is unavailable")

    if stage_needs_launcher_lock(args.stage):
        pin_launcher_identity(args.output_dir)

    plan_path = args.output_dir / "plan.json"
    manifest_path = args.output_dir / "manifest.json"
    progress_path = args.output_dir / "progress.json"

    if args.resume:
        if not plan_path.is_file():
            raise RuntimeError("resume requires an existing plan.json")
        plan = json.loads(plan_path.read_text(encoding="utf-8"))
        if plan.get("pairs") != args.pairs:
            raise RuntimeError("resume pairs must match the existing plan")
        manifest = (
            json.loads(manifest_path.read_text(encoding="utf-8"))
            if manifest_path.is_file()
            else {"schema_version": 1, "runs": []}
        )
        progress = (
            json.loads(progress_path.read_text(encoding="utf-8"))
            if progress_path.is_file()
            else {"schema_version": 1, "plan": "plan.json", "completed_runs": 0, "runs": []}
        )
    else:
        bootstrap_metadata = {".campaign.lock", "launcher-identity.json"}
        if args.output_dir.exists() and any(
            path.name not in bootstrap_metadata for path in args.output_dir.iterdir()
        ):
            raise RuntimeError("output directory must be new or empty")
        args.output_dir.mkdir(parents=True, exist_ok=True)
        plan = make_plan(
            pairs=args.pairs,
            seed=args.seed,
            arms=arms,
            project_path=project_path,
        )
        write_json(plan_path, plan)
        if not args.skip_preflight:
            preflight(
                arms=arms,
                project_path=project_path,
                output_dir=args.output_dir,
                node=args.node,
                timeout_secs=args.timeout_secs,
            )
        manifest = {"schema_version": 1, "runs": []}
        progress = {
            "schema_version": 1,
            "plan": "plan.json",
            "completed_runs": 0,
            "runs": [],
        }
        write_json(progress_path, progress)

    completed_ids = {
        row.get("run_id")
        for row in manifest.get("runs", [])
        if isinstance(row, dict) and isinstance(row.get("run_id"), str)
    }
    if len(completed_ids) != len(manifest.get("runs", [])):
        raise RuntimeError("manifest contains duplicate or invalid run_id entries")

    new_runs_completed = 0
    stop_after_current = False
    for pair in plan["schedule"]:
        for planned in pair["runs"]:
            harness = planned["harness"]
            arm = arms[harness]
            run_id = planned["run_id"]
            if run_id in completed_ids:
                continue

            prompt = probe_prompt(arm, project_path, planned["marker"])
            base_run_dir = args.output_dir / "runs" / run_id
            recovered = submitted_attempt(base_run_dir) if args.resume else None
            if recovered is not None:
                run_dir, driver = recovered
                validate_driver_turn(
                    harness=harness,
                    connector=arm["connector"],
                    driver=driver,
                )
            elif args.stage == "commit":
                return {
                    "schema_version": 1,
                    "status": "awaiting_submission",
                    "completed_runs": len(completed_ids),
                    "next_run_id": run_id,
                    "next_harness": harness,
                }
            else:
                attempt = 0
                while True:
                    run_dir = available_attempt_dir(base_run_dir)
                    driver, _ = driver_call(
                        connector=arm["connector"],
                        prompt=prompt,
                        run_dir=run_dir,
                        node=args.node,
                        timeout_secs=args.timeout_secs,
                        preflight=False,
                    )
                    try:
                        validate_driver_turn(
                            harness=harness,
                            connector=arm["connector"],
                            driver=driver,
                        )
                        break
                    except RuntimeError:
                        safe_to_retry = driver.get("submitted_at_ms") is None
                        if not safe_to_retry or attempt >= args.pre_submit_retries:
                            raise
                        attempt += 1
                        time.sleep(2.0)

            if args.stage == "submit":
                return {
                    "schema_version": 1,
                    "status": "submitted",
                    "run_id": run_id,
                    "harness": harness,
                    "marker": planned["marker"],
                    "driver": str((run_dir / "driver.json").relative_to(args.output_dir)),
                    "durable_submission": durable_submission_checkpoint(driver),
                }

            profile = exact_profile_with_retry(
                driver_path=run_dir / "driver.json",
                trace_root=Path(arm["trace_root"]),
                marker=planned["marker"],
                wait_secs=args.trace_wait_secs,
            )
            validate_measured_turn(
                harness=harness,
                connector=arm["connector"],
                driver=driver,
                profile=profile,
            )
            profile_path = args.output_dir / "profiles" / f"{run_id}.json"
            write_json(profile_path, profile)
            manifest["runs"].append({
                "run_id": run_id,
                "harness": harness,
                "pair_id": planned["pair_id"],
                "order_position": planned["order_position"],
                "connector_state": arm["connector_state"],
                "registry_state": arm["registry_state"],
                "chat_state": arm["chat_state"],
                "profile": str(profile_path.relative_to(args.output_dir)),
            })
            completed_ids.add(run_id)
            progress["completed_runs"] = len(completed_ids)
            progress.setdefault("runs", []).append({
                "run_id": run_id,
                "server_trace_id": profile["first_tool"]["server_trace_id"],
                "prompt_to_server_received_ms": profile["first_tool"][
                    "prompt_to_server_received_ms"
                ],
                "prompt_to_helper_ingress_ms": profile["unattributed"].get(
                    "prompt_to_helper_ingress_ms"
                ),
                "helper_ingress_to_server_received_ms": (
                    profile["first_tool"].get("helper_timing") or {}
                ).get("ingress_to_server_received_ms"),
            })
            write_json(manifest_path, manifest)
            write_json(progress_path, progress)
            new_runs_completed += 1
            if new_run_limit_reached(new_runs_completed, args.max_new_runs):
                stop_after_current = True
                break
        if stop_after_current:
            break

    planned_runs = sum(len(pair["runs"]) for pair in plan["schedule"])
    if len(completed_ids) < planned_runs:
        return {
            "schema_version": 1,
            "status": "partial",
            "completed_runs": len(completed_ids),
            "planned_runs": planned_runs,
            "remaining_runs": planned_runs - len(completed_ids),
            "new_runs_completed": new_runs_completed,
        }
    result = variance.analyze(manifest_path)
    write_json(args.output_dir / "variance.json", result)
    return result


def run_campaign(args: argparse.Namespace) -> dict[str, Any]:
    args.output_dir.mkdir(parents=True, exist_ok=True)
    def run_with_output_lock() -> dict[str, Any]:
        lock_path = args.output_dir / ".campaign.lock"
        with lock_path.open("a+", encoding="utf-8") as lock_file:
            try:
                fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as exc:
                raise RuntimeError(
                    f"another Phase 16C campaign already owns {args.output_dir}"
                ) from exc
            try:
                return _run_campaign_locked(args)
            finally:
                fcntl.flock(lock_file.fileno(), fcntl.LOCK_UN)

    if not stage_needs_launcher_lock(args.stage):
        return run_with_output_lock()

    GLOBAL_CAMPAIGN_LOCK.parent.mkdir(parents=True, exist_ok=True)
    with GLOBAL_CAMPAIGN_LOCK.open("a+", encoding="utf-8") as global_lock:
        try:
            fcntl.flock(global_lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise RuntimeError(
                "another Phase 16C connected campaign already owns the ChatGPT launcher"
            ) from exc
        try:
            return run_with_output_lock()
        finally:
            fcntl.flock(global_lock.fileno(), fcntl.LOCK_UN)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--pairs", type=int, default=10)
    parser.add_argument("--seed", type=int, default=1603)
    parser.add_argument("--timeout-secs", type=int, default=120)
    parser.add_argument("--trace-wait-secs", type=float, default=10.0)
    parser.add_argument("--node", type=Path, default=DEFAULT_NODE)
    parser.add_argument("--skip-preflight", action="store_true")
    parser.add_argument("--resume", action="store_true")
    parser.add_argument("--stage", choices=("all", "submit", "commit"), default="all")
    parser.add_argument("--pre-submit-retries", type=int, default=2)
    parser.add_argument("--max-new-runs", type=int)
    args = parser.parse_args()
    if (
        args.timeout_secs < 30
        or args.trace_wait_secs <= 0
        or args.pre_submit_retries < 0
        or (args.max_new_runs is not None and args.max_new_runs <= 0)
    ):
        parser.error(
            "timeout-secs must be >= 30, trace-wait-secs > 0, retries >= 0, "
            "and max-new-runs > 0 when provided"
        )
    if args.stage == "commit" and not args.resume:
        parser.error("--stage commit requires --resume")
    try:
        run_campaign(args)
    except Exception as exc:
        print(f"phase16c campaign failed: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
