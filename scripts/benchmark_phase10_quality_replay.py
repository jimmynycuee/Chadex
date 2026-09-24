#!/usr/bin/env python3
from __future__ import annotations

import json
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from benchmark_cold_runtime import IsolatedHelper
from benchmark_phase9_e2e import candidate_paths
from benchmark_turn_economy import MeasuredClient
from benchmark_workflow import parse_env_file, runtime_identity

ROOT = Path("benchmarks/phase10-quality").resolve()
SOURCE = ROOT / "source"
ARM9 = ROOT / "phase9"
ARM10 = ROOT / "phase10"
REPLAY9 = ROOT / "replay-phase9"
REPLAY10 = ROOT / "replay-phase10"
HIDDEN = ROOT / "evaluator" / "hidden_tests.py"
CANDIDATE = Path("dist/Phase 10 Candidate.app").resolve()
REPORT = ROOT / "quality-report.json"


def run(argv: list[str], *, cwd: Path | None = None, check: bool = True, timeout: int = 120):
    return subprocess.run(argv, cwd=cwd, check=check, capture_output=True, text=True, timeout=timeout)


def changed_files(arm: Path) -> list[str]:
    output = run(["git", "diff", "--name-only", "HEAD", "--"], cwd=arm).stdout.splitlines()
    return sorted(path for path in output if not path.startswith("graphify-out/") and "__pycache__" not in path)


def copy_clean_source(destination: Path) -> None:
    if destination.exists():
        shutil.rmtree(destination)
    shutil.copytree(SOURCE, destination)


def replacement_change(source: Path, arm: Path, relative: str) -> dict:
    before = (source / relative).read_text()
    after = (arm / relative).read_text()
    return {
        "kind": "edit",
        "path": relative,
        "edits": [{"kind": "replace_exact", "old_text": before, "new_text": after}],
    }


def validate_step(kind: str) -> dict:
    if kind == "store":
        args = [
            "-c",
            "from pathlib import Path; import tempfile,json; "
            "from taskdesk.models import TaskResult; from taskdesk.store import TaskHistoryStore; "
            "d=tempfile.TemporaryDirectory(); s=TaskHistoryStore(Path(d.name)); "
            "s.record(TaskResult('s','completed',1,2,('a.py','a.py'),'passed')); "
            "assert s.list()[0]['changed_files']==['a.py']; "
            "assert json.loads((Path(d.name)/'task-history.json').read_text())['schema_version']==2",
        ]
        checks = [{"executable": "python3", "args": args, "timeout_secs": 30}]
    elif kind == "service":
        args = [
            "-c",
            "from pathlib import Path; import tempfile; from taskdesk.models import TaskResult; "
            "from taskdesk.runtime import TaskRuntime; from taskdesk.service import TaskService; "
            "d=tempfile.TemporaryDirectory(); r=TaskRuntime(); s=TaskService(r,Path(d.name)); "
            "r.emit(TaskResult('svc','failed',1,2)); assert s.last_result.task_id=='svc'; "
            "assert s.task_history()[0]['status']=='failed'",
        ]
        checks = [{"executable": "python3", "args": args, "timeout_secs": 30}]
    elif kind == "bridge":
        args = [
            "-c",
            "from pathlib import Path; import tempfile; from taskdesk.bridge import TaskBridge; "
            "from taskdesk.models import TaskResult; from taskdesk.runtime import TaskRuntime; "
            "from taskdesk.service import TaskService; d=tempfile.TemporaryDirectory(); "
            "r=TaskRuntime(); b=TaskBridge(TaskService(r,Path(d.name))); "
            "r.emit(TaskResult('b','completed',1,2)); "
            "assert b.call('task_history',{'limit':0})[0]['task_id']=='b'; "
            "assert b.call('clear_task_history')=={'cleared': True}",
        ]
        checks = [{"executable": "python3", "args": args, "timeout_secs": 30}]
    else:
        checks = [
            {"executable": "python3", "args": ["-m", "unittest", "discover", "-s", "tests", "-v"], "timeout_secs": 60},
            {"executable": "python3", "args": ["-m", "compileall", "-q", "taskdesk", "tests"], "timeout_secs": 30},
        ]
    return {"kind": "validate", "checks": checks}


def task_args(variant: str, source: Path, arm: Path) -> dict:
    files = changed_files(arm)
    if variant == "phase9_fixed1":
        steps = [
            {"kind": "edit", "changes": [replacement_change(source, arm, path) for path in files]},
            validate_step("full"),
            {"kind": "review", "include_diff": False, "max_hunks": 20, "max_hunk_lines": 120},
        ]
        package_count = 1
    else:
        grouped = [
            ("taskdesk/store.py", "store"),
            ("taskdesk/service.py", "service"),
            ("taskdesk/bridge.py", "bridge"),
        ]
        consumed = {path for path, _ in grouped}
        remaining = [path for path in files if path not in consumed]
        steps = []
        for path, validation in grouped:
            if path in files:
                steps.append({"kind": "edit", "changes": [replacement_change(source, arm, path)]})
                steps.append(validate_step(validation))
        if remaining:
            steps.append({"kind": "edit", "changes": [replacement_change(source, arm, path) for path in remaining]})
            steps.append(validate_step("full"))
        steps.append({"kind": "review", "include_diff": False, "max_hunks": 20, "max_hunk_lines": 120})
        package_count = None

    arguments = {
        "goal": "Implement persistent Task History across persistence, service, bridge, and view model while preserving existing behavior.",
        "preplan": {
            "estimated_file_count": 5,
            "estimated_subsystem_count": 4,
            "estimated_language_count": 1,
            "validation_domain_count": 3,
            "cross_runtime_boundary": False,
            "stateful_or_schema_change": True,
            "concurrency_or_security_sensitive": True,
            "graphify_informed": True,
        },
        "steps": steps,
        "policy": {
            "max_steps": 16,
            "max_mutations": 8,
            "max_changed_files": 10,
            "timeout_secs": 150,
            "max_result_bytes": 40000,
            "allowed_operations": ["edit", "run_process", "validate", "review"],
        },
        "acceptance": {"require_validation_success": True, "require_review": True},
    }
    if package_count is not None:
        arguments["package_count"] = package_count
    return arguments


def execute_arm(variant: str, arm: Path, replay: Path) -> dict:
    copy_clean_source(replay)
    helper_binary, resources = candidate_paths(CANDIDATE)
    data_dir = replay.parent / f".data-{variant}"
    shutil.rmtree(data_dir, ignore_errors=True)
    helper = IsolatedHelper(helper_binary, resources, data_dir)
    client = None
    try:
        helper.request("activateProject", {"path": str(replay)})
        helper.request("configureLocalSetup")
        server_url, env_file, project_id = runtime_identity(data_dir)
        token = parse_env_file(env_file).get("WEBCODEX_TOKEN", "").strip()
        if not token:
            raise RuntimeError("missing isolated token")
        client = MeasuredClient(server_url, token)
        client.invoke("runtime_status", {"summary_only": True})
        client.samples.clear()
        arguments = task_args(variant, SOURCE, arm)
        arguments["project"] = project_id
        started = time.perf_counter()
        output, success = client.invoke("call_runtime_tool", {"tool": "execute_task", "arguments": arguments})
        wall_ms = (time.perf_counter() - started) * 1000.0
        packaging = output.get("packaging") or {}
        complexity = output.get("complexity") or {}
        return {
            "success": success,
            "status": output.get("status"),
            "wall_ms": round(wall_ms, 3),
            "duration_ms": output.get("duration_ms"),
            "tool_calls": len(client.samples),
            "requested_package_count": packaging.get("requested_package_count"),
            "recommended_package_count": packaging.get("recommended_package_count"),
            "effective_package_count": packaging.get("effective_package_count"),
            "graphify_status": packaging.get("graphify_status"),
            "graphify_fresh": packaging.get("graphify_fresh"),
            "graphify_adjusted": packaging.get("graphify_adjusted"),
            "dependency_merge_count": packaging.get("dependency_merge_count"),
            "graphify_planning_ms": packaging.get("graphify_planning_ms"),
            "complexity_score": complexity.get("score"),
            "complexity_band": complexity.get("band"),
            "complexity_signals": complexity.get("signals"),
            "validation": output.get("validation"),
            "review": output.get("review"),
            "steps": output.get("steps"),
        }
    finally:
        if client is not None:
            client.close()
        try:
            helper.close()
        except Exception:
            if helper.process.poll() is None:
                helper.process.kill()


def test_result(project: Path, hidden: bool) -> dict:
    if hidden:
        completed = run(["python3", str(HIDDEN), str(project)], cwd=project, check=False)
        score_line = next((line for line in completed.stdout.splitlines() if line.startswith("HIDDEN_SCORE=")), "HIDDEN_SCORE=0/10")
        score = score_line.split("=", 1)[1]
        passed, total = [int(value) for value in score.split("/")]
        return {
            "passed": completed.returncode == 0,
            "passed_tests": passed,
            "total_tests": total,
            "stdout": completed.stdout[-2000:],
            "stderr": completed.stderr[-3000:],
        }
    completed = run(["python3", "-m", "unittest", "discover", "-s", "tests", "-v"], cwd=project, check=False)
    return {
        "passed": completed.returncode == 0,
        "stdout": completed.stdout[-1000:],
        "stderr": completed.stderr[-2000:],
    }


def diff_metrics(project: Path) -> dict:
    numstat = run(["git", "diff", "--numstat", "HEAD", "--"], cwd=project).stdout.splitlines()
    files = []
    additions = deletions = 0
    for line in numstat:
        parts = line.split("\t")
        if len(parts) != 3 or parts[2].startswith("graphify-out/") or "__pycache__" in parts[2]:
            continue
        try:
            additions += int(parts[0])
            deletions += int(parts[1])
        except ValueError:
            pass
        files.append(parts[2])
    return {
        "changed_files": sorted(files),
        "changed_file_count": len(files),
        "additions": additions,
        "deletions": deletions,
        "total_changed_lines": additions + deletions,
    }


def main() -> int:
    phase9_execution = execute_arm("phase9_fixed1", ARM9, REPLAY9)
    phase10_execution = execute_arm("phase10_adaptive", ARM10, REPLAY10)

    report = {
        "candidate_app": CANDIDATE.name,
        "blind_until_both_implementations_complete": True,
        "phase9": {
            "execution": phase9_execution,
            "visible": test_result(REPLAY9, hidden=False),
            "hidden": test_result(REPLAY9, hidden=True),
            "diff": diff_metrics(REPLAY9),
        },
        "phase10": {
            "execution": phase10_execution,
            "visible": test_result(REPLAY10, hidden=False),
            "hidden": test_result(REPLAY10, hidden=True),
            "diff": diff_metrics(REPLAY10),
        },
    }
    REPORT.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
