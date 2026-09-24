#!/usr/bin/env python3
"""Phase 10D E2E benchmark: Phase 9 fixed-one packaging vs Phase 10 adaptive packaging.

The benchmark uses an isolated candidate helper/server/runner and synthetic Git fixture.
Graphify is generated once outside the measured region and copied into each checkout.
Only the execute_task workflow is timed; setup, cloning, graph generation and hidden
evaluation are excluded.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from typing import Any

from benchmark_cold_runtime import IsolatedHelper, timing_summary
from benchmark_phase9_e2e import candidate_paths
from benchmark_turn_economy import MeasuredClient
from benchmark_workflow import parse_env_file, runtime_identity

os.environ["PYTHONDONTWRITEBYTECODE"] = "1"

SCENARIOS: dict[str, dict[str, Any]] = {
    "small": {
        "requested_packages": 1,
        "files": {
            "small/value.py": "def clamp(value):\n    return value\n",
            "tests/test_small.py": (
                "import unittest\n"
                "from small.value import clamp\n\n"
                "class SmallTests(unittest.TestCase):\n"
                "    def test_clamp(self):\n"
                "        self.assertEqual(clamp(-4), 0)\n"
                "        self.assertEqual(clamp(3), 3)\n\n"
                "if __name__ == '__main__':\n"
                "    unittest.main()\n"
            ),
        },
        "units": [
            {
                "files": ["small/value.py"],
                "changes": [
                    ("small/value.py", "return value", "return max(0, value)"),
                ],
                "validation": ["-m", "unittest", "tests.test_small", "-v"],
            }
        ],
    },
    "medium": {
        "requested_packages": 2,
        "files": {
            "alpha/service.py": "def alpha():\n    return 'OLD_ALPHA'\n",
            "beta/service.py": "def beta():\n    return 'OLD_BETA'\n",
            "tests/test_alpha.py": (
                "import unittest\n"
                "from alpha.service import alpha\n\n"
                "class AlphaTests(unittest.TestCase):\n"
                "    def test_alpha(self): self.assertEqual(alpha(), 'NEW_ALPHA')\n"
            ),
            "tests/test_beta.py": (
                "import unittest\n"
                "from beta.service import beta\n\n"
                "class BetaTests(unittest.TestCase):\n"
                "    def test_beta(self): self.assertEqual(beta(), 'NEW_BETA')\n"
            ),
        },
        "units": [
            {
                "files": ["alpha/service.py"],
                "changes": [("alpha/service.py", "OLD_ALPHA", "NEW_ALPHA")],
                "validation": ["-m", "unittest", "tests.test_alpha", "-v"],
            },
            {
                "files": ["beta/service.py"],
                "changes": [("beta/service.py", "OLD_BETA", "NEW_BETA")],
                "validation": ["-m", "unittest", "tests.test_beta", "-v"],
            },
        ],
    },
    "large": {
        "requested_packages": 4,
        "files": {
            "app/model.py": "from app.helper import helper\ndef model(): return 'OLD_MODEL:' + helper()\n",
            "app/helper.py": "from bridge.client import client\ndef helper(): return 'OLD_HELPER:' + client()\n",
            "bridge/client.py": "def client(): return 'OLD_CLIENT'\n",
            "bridge/protocol.py": "def protocol(): return 'OLD_PROTOCOL'\n",
            "runtime/bridge.py": "from backend.backend import backend\ndef bridge(): return 'OLD_BRIDGE:' + backend()\n",
            "runtime/common.py": "def common(): return 'OLD_COMMON'\n",
            "backend/backend.py": "def backend(): return 'OLD_BACKEND'\n",
            "backend/marker.rs": "pub const MARKER: &str = \"OLD_MARKER\";\n",
        },
        "units": [
            {
                "files": ["app/model.py", "app/helper.py"],
                "changes": [
                    ("app/model.py", "OLD_MODEL", "NEW_MODEL"),
                    ("app/helper.py", "OLD_HELPER", "NEW_HELPER"),
                ],
                "validation": ["-c", "from app.model import model; assert model().startswith('NEW_MODEL:NEW_HELPER:')"],
            },
            {
                "files": ["bridge/client.py", "bridge/protocol.py"],
                "changes": [
                    ("bridge/client.py", "OLD_CLIENT", "NEW_CLIENT"),
                    ("bridge/protocol.py", "OLD_PROTOCOL", "NEW_PROTOCOL"),
                ],
                "validation": ["-c", "from bridge.client import client; from bridge.protocol import protocol; assert client() == 'NEW_CLIENT'; assert protocol() == 'NEW_PROTOCOL'"],
            },
            {
                "files": ["runtime/bridge.py", "runtime/common.py"],
                "changes": [
                    ("runtime/bridge.py", "OLD_BRIDGE", "NEW_BRIDGE"),
                    ("runtime/common.py", "OLD_COMMON", "NEW_COMMON"),
                ],
                "validation": ["-c", "from runtime.bridge import bridge; from runtime.common import common; assert bridge().startswith('NEW_BRIDGE:'); assert common() == 'NEW_COMMON'"],
            },
            {
                "files": ["backend/backend.py", "backend/marker.rs"],
                "changes": [
                    ("backend/backend.py", "OLD_BACKEND", "NEW_BACKEND"),
                    ("backend/marker.rs", "OLD_MARKER", "NEW_MARKER"),
                ],
                "validation": ["-c", "from pathlib import Path; from backend.backend import backend; assert backend() == 'NEW_BACKEND'; assert 'NEW_MARKER' in Path('backend/marker.rs').read_text()"],
            },
        ],
    },
}


def run(argv: list[str], *, cwd: Path | None = None, timeout: int = 90) -> subprocess.CompletedProcess[str]:
    return subprocess.run(argv, cwd=cwd, check=True, capture_output=True, text=True, timeout=timeout)


def write_fixture(root: Path) -> tuple[str, Path]:
    root.mkdir(parents=True)
    for scenario in SCENARIOS.values():
        for relative, content in scenario["files"].items():
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
    for package in ["small", "alpha", "beta", "tests", "app", "bridge", "runtime", "backend"]:
        path = root / package / "__init__.py"
        if not path.exists():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("")
    run(["git", "init", "-q"], cwd=root)
    run(["git", "config", "user.name", "Phase10 Benchmark"], cwd=root)
    run(["git", "config", "user.email", "phase10@example.invalid"], cwd=root)
    run(["git", "add", "."], cwd=root)
    run(["git", "commit", "-qm", "phase10 benchmark baseline"], cwd=root)
    baseline = run(["git", "rev-parse", "HEAD"], cwd=root).stdout.strip()
    run([sys.executable, "-m", "graphify", "update", "."], cwd=root, timeout=120)
    graph = root / "graphify-out"
    if not (graph / "graph.json").is_file():
        raise RuntimeError("Graphify did not produce graphify-out/graph.json")
    return baseline, graph


def prepare_checkout(source: Path, baseline: str, graph_source: Path, destination: Path) -> None:
    run(["git", "clone", "--quiet", "--no-hardlinks", "--no-checkout", str(source), str(destination)])
    run(["git", "-C", str(destination), "checkout", "--quiet", "--detach", baseline])
    shutil.copytree(graph_source, destination / "graphify-out", copy_function=shutil.copy)
    now = time.time()
    os.utime(destination / "graphify-out" / "graph.json", (now, now))


def scenario_steps(scenario: dict[str, Any]) -> list[dict[str, Any]]:
    steps: list[dict[str, Any]] = []
    for unit in scenario["units"]:
        steps.append({
            "kind": "read",
            "items": [{"path": path} for path in unit["files"]],
            "with_line_numbers": True,
            "max_result_bytes": 16384,
        })
        steps.append({
            "kind": "edit",
            "changes": [
                {
                    "kind": "edit",
                    "path": path,
                    "edits": [{"kind": "replace_exact", "old_text": old, "new_text": new}],
                }
                for path, old, new in unit["changes"]
            ],
            "dry_run": False,
        })
        steps.append({
            "kind": "validate",
            "checks": [{
                "executable": "python3",
                "args": unit["validation"],
                "timeout_secs": 20,
            }],
        })
    steps.append({
        "kind": "review",
        "include_diff": False,
        "max_hunks": 20,
        "max_hunk_lines": 120,
    })
    return steps


def expected_changed_files(scenario: dict[str, Any]) -> list[str]:
    return sorted({path for unit in scenario["units"] for path, _, _ in unit["changes"]})


def hidden_evaluator(project: Path, scenario: dict[str, Any], baseline: str) -> dict[str, Any]:
    files = expected_changed_files(scenario)
    status = run(["git", "status", "--porcelain=v1"], cwd=project).stdout.splitlines()
    observed = sorted(line[3:] for line in status if len(line) >= 4 and not line[3:].startswith("graphify-out/"))
    diff_check = subprocess.run(["git", "diff", "--check", baseline, "--"], cwd=project, capture_output=True, text=True)
    markers_ok = True
    for unit in scenario["units"]:
        for path, old, new in unit["changes"]:
            text = (project / path).read_text()
            markers_ok = markers_ok and old not in text and new in text
    python_compile = subprocess.run(
        ["python3", "-m", "compileall", "-q", "."],
        cwd=project,
        capture_output=True,
        text=True,
        timeout=20,
        env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
    )
    return {
        "passed": observed == files and diff_check.returncode == 0 and markers_ok and python_compile.returncode == 0,
        "changed_files": observed,
        "changed_files_exact": observed == files,
        "git_diff_check": diff_check.returncode == 0,
        "markers_ok": markers_ok,
        "python_compile": python_compile.returncode == 0,
    }


def diff_sha256(project: Path, baseline: str) -> str:
    completed = subprocess.run(
        ["git", "diff", "--binary", baseline, "--", ".", ":(exclude)graphify-out"],
        cwd=project,
        check=True,
        capture_output=True,
        timeout=10,
    )
    return hashlib.sha256(completed.stdout).hexdigest()


def execute_workflow(
    client: MeasuredClient,
    project_id: str,
    scenario_name: str,
    scenario: dict[str, Any],
    variant: str,
) -> dict[str, Any]:
    requested = 1 if variant == "phase9_fixed1" else int(scenario["requested_packages"])
    task_arguments = {
        "project": project_id,
        "goal": f"Phase 10D {scenario_name} deterministic benchmark.",
        "package_count": requested,
        "steps": scenario_steps(scenario),
        "policy": {
            "max_steps": 20,
            "max_mutations": 8,
            "max_changed_files": 16,
            "timeout_secs": 90,
            "max_result_bytes": 16384,
            "allowed_operations": ["read", "edit", "run_process", "validate", "review"],
        },
        "acceptance": {"require_validation_success": True, "require_review": True},
    }
    output, success = client.invoke(
        "call_runtime_tool",
        {"tool": "execute_task", "arguments": task_arguments},
    )
    packaging = output.get("packaging") or {}
    complexity = output.get("complexity") or {}
    validation = output.get("validation") or {}
    counters = output.get("counters") or {}
    return {
        "invoke_success": success,
        "task_status": output.get("status"),
        "validation_ok": validation.get("status") == "passed",
        "retries": int(counters.get("retries", 0)),
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
        "outer_execute_task_count": packaging.get("outer_execute_task_count"),
    }


def run_variant(
    *,
    source: Path,
    graph_source: Path,
    baseline: str,
    helper_binary: Path,
    resources: Path,
    scenario_name: str,
    variant: str,
) -> dict[str, Any]:
    root = Path(tempfile.mkdtemp(prefix=f"chadex-phase10d-{scenario_name}-{variant}-"))
    project = root / "project"
    data_dir = root / "data"
    helper: IsolatedHelper | None = None
    client: MeasuredClient | None = None
    try:
        prepare_checkout(source, baseline, graph_source, project)
        helper = IsolatedHelper(helper_binary, resources, data_dir)
        helper.request("activateProject", {"path": str(project)})
        helper.request("configureLocalSetup")
        server_url, env_file, project_id = runtime_identity(data_dir)
        token = parse_env_file(env_file).get("WEBCODEX_TOKEN", "").strip()
        if not token:
            raise RuntimeError("isolated bootstrap token is unavailable")
        client = MeasuredClient(server_url, token)
        client.invoke("runtime_status", {"summary_only": True})
        client.samples.clear()

        started = time.perf_counter()
        workflow = execute_workflow(client, project_id, scenario_name, SCENARIOS[scenario_name], variant)
        wall_ms = (time.perf_counter() - started) * 1000.0
        samples = list(client.samples)
        evaluator = hidden_evaluator(project, SCENARIOS[scenario_name], baseline)
        correct = (
            workflow["invoke_success"]
            and workflow["task_status"] == "completed"
            and workflow["validation_ok"]
            and evaluator["passed"]
        )
        return {
            "scenario": scenario_name,
            "variant": variant,
            "correct": correct,
            "wall_ms": round(wall_ms, 3),
            "time_to_correct_ms": round(wall_ms, 3) if correct else None,
            "tool_calls": len(samples),
            "request_bytes": sum(int(sample["request_bytes"]) for sample in samples),
            "result_bytes": sum(int(sample["response_bytes"]) for sample in samples),
            "diff_hash": diff_sha256(project, baseline) if correct else None,
            "evaluator": evaluator,
            **workflow,
        }
    except Exception as error:
        samples = list(client.samples) if client is not None else []
        return {
            "scenario": scenario_name,
            "variant": variant,
            "correct": False,
            "tool_calls": len(samples),
            "request_bytes": sum(int(sample["request_bytes"]) for sample in samples),
            "result_bytes": sum(int(sample["response_bytes"]) for sample in samples),
            "error_kind": type(error).__name__,
            "error_message": str(error)[:500],
        }
    finally:
        if client is not None:
            client.close()
        if helper is not None:
            try:
                helper.close()
            except Exception:
                if helper.process.poll() is None:
                    helper.process.kill()
        shutil.rmtree(root, ignore_errors=True)


def median(rows: list[dict[str, Any]], key: str) -> float | None:
    values = [float(row[key]) for row in rows if row.get("correct") and isinstance(row.get(key), (int, float))]
    return round(statistics.median(values), 3) if values else None


def summarize(rows: list[dict[str, Any]]) -> dict[str, Any]:
    summary: dict[str, Any] = {}
    for scenario_name in SCENARIOS:
        summary[scenario_name] = {}
        for variant in ["phase9_fixed1", "phase10_adaptive"]:
            subset = [row for row in rows if row["scenario"] == scenario_name and row["variant"] == variant]
            correct = [row for row in subset if row.get("correct")]
            summary[scenario_name][variant] = {
                "runs": len(subset),
                "correct_runs": len(correct),
                "wall_ms": timing_summary([float(row["wall_ms"]) for row in correct]) if correct else None,
                "median_time_to_correct_ms": median(correct, "time_to_correct_ms"),
                "median_tool_calls": median(correct, "tool_calls"),
                "median_result_bytes": median(correct, "result_bytes"),
                "median_effective_package_count": median(correct, "effective_package_count"),
                "median_graphify_planning_ms": median(correct, "graphify_planning_ms"),
                "graphify_adjusted_runs": sum(bool(row.get("graphify_adjusted")) for row in correct),
                "diff_hashes": sorted({row.get("diff_hash") for row in correct}),
            }
        fixed = summary[scenario_name]["phase9_fixed1"]
        adaptive = summary[scenario_name]["phase10_adaptive"]
        if fixed["median_time_to_correct_ms"] and adaptive["median_time_to_correct_ms"]:
            summary[scenario_name]["adaptive_overhead_percent"] = round(
                100.0 * (adaptive["median_time_to_correct_ms"] / fixed["median_time_to_correct_ms"] - 1.0),
                2,
            )
    return summary


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate-app", type=Path, required=True)
    parser.add_argument("--iterations", type=int, default=5)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.iterations < 5:
        parser.error("Phase 10D benchmark requires at least 5 iterations per variant")

    app = args.candidate_app.expanduser().resolve()
    helper_binary, resources = candidate_paths(app)
    rows: list[dict[str, Any]] = []

    with tempfile.TemporaryDirectory(prefix="chadex-phase10d-source-") as directory:
        source = Path(directory) / "source"
        baseline, graph_source = write_fixture(source)
        for iteration in range(1, args.iterations + 1):
            for scenario_name in SCENARIOS:
                variants = ["phase9_fixed1", "phase10_adaptive"]
                order = variants if iteration % 2 else list(reversed(variants))
                for variant in order:
                    row = run_variant(
                        source=source,
                        graph_source=graph_source,
                        baseline=baseline,
                        helper_binary=helper_binary,
                        resources=resources,
                        scenario_name=scenario_name,
                        variant=variant,
                    )
                    row["iteration"] = iteration
                    rows.append(row)
                    print(
                        f"{scenario_name:6} {variant:16} {iteration}/{args.iterations}: "
                        f"{'correct' if row.get('correct') else 'failed'}, "
                        f"wall_ms={row.get('wall_ms')}, packages={row.get('effective_package_count')}, "
                        f"graphify={row.get('graphify_status')}"
                    )

        summary = summarize(rows)
        equivalent = True
        for scenario_name in SCENARIOS:
            fixed = summary[scenario_name]["phase9_fixed1"]
            adaptive = summary[scenario_name]["phase10_adaptive"]
            equivalent = equivalent and fixed["correct_runs"] == args.iterations
            equivalent = equivalent and adaptive["correct_runs"] == args.iterations
            equivalent = equivalent and fixed["diff_hashes"] == adaptive["diff_hashes"]
            equivalent = equivalent and len(fixed["diff_hashes"]) == 1

        report = {
            "schema": 1,
            "surface": "isolated local MCP through Phase 10 candidate helper/server/runner",
            "comparison": "same execute_task plans; Phase 9 fixed package_count=1 vs Phase 10 adaptive package_count with fresh Graphify evidence",
            "candidate_app": app.name,
            "candidate_helper_sha256": hashlib.sha256(helper_binary.read_bytes()).hexdigest(),
            "iterations_per_variant_per_scenario": args.iterations,
            "measured_scope": "execute_task workflow only; setup, clone, Graphify rebuild and hidden evaluator excluded",
            "rows": rows,
            "summary": summary,
            "equivalent_correct_output": equivalent,
            "production_app_replaced": False,
        }
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
        print(json.dumps({"summary": summary, "equivalent_correct_output": equivalent}, indent=2))
        return 0 if equivalent else 1


if __name__ == "__main__":
    raise SystemExit(main())
