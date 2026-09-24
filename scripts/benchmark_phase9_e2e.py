#!/usr/bin/env python3
"""Phase 9 E2E benchmark: direct tool workflow vs execute_task.

Each measured run uses:
- the same committed Benchmark-Chadex Git baseline,
- a fresh local clone and isolated Chadex runtime,
- the same deterministic code/test change,
- an evaluator that runs only after timing ends.

The report never stores runtime tokens, project ids, or tool response bodies.
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
import tempfile
import time
from typing import Any

from benchmark_cold_runtime import IsolatedHelper, timing_summary
from benchmark_turn_economy import MeasuredClient
from benchmark_workflow import parse_env_file, runtime_identity

# Prevent validation/evaluator Python processes from polluting the measured Git worktree.
os.environ["PYTHONDONTWRITEBYTECODE"] = "1"


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_BENCHMARK = REPO_ROOT.parent / "Benchmark-Chadex"
EXPECTED_CHANGED_FILES = ["pricing/discount.py", "tests/test_checkout.py"]
TEST_ARGS = ["-m", "unittest", "discover", "-s", "tests", "-v"]
GOAL = (
    "Ensure a fixed discount never makes the payable subtotal negative. "
    "Clamp an excessive fixed discount at zero and add a regression test, "
    "then run the full unittest suite and review the final changes."
)
NEW_TEST = """    def test_fixed_discount_larger_than_subtotal_clamps_at_zero(self):
        cart = Cart([LineItem("Adapter", 25.0)])
        summary = CheckoutService().calculate(cart, discount_amount=40.0, tax_rate=0.1)

        self.assertEqual(summary.subtotal, 25.0)
        self.assertEqual(summary.discounted_subtotal, 0.0)
        self.assertEqual(summary.tax, 0.0)
        self.assertEqual(summary.total, 0.0)

"""


def run(
    argv: list[str],
    *,
    cwd: Path | None = None,
    timeout: int = 30,
    text: bool = True,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        argv,
        cwd=cwd,
        check=True,
        capture_output=True,
        text=text,
        timeout=timeout,
    )


def candidate_paths(app: Path) -> tuple[Path, Path]:
    helper = app / "Contents" / "Helpers" / "chadex-helper"
    resources = app / "Contents" / "Resources"
    if not helper.is_file():
        raise RuntimeError("candidate helper is missing")
    for name in ["chadex-runtime-cli", "chadex-runtime-server", "chadex-runtime-runner"]:
        if not (resources / "chadex-runtime" / name).is_file():
            raise RuntimeError(f"candidate runtime binary is missing: {name}")
    return helper, resources


def baseline_commit(source: Path) -> str:
    return run(["git", "-C", str(source), "rev-parse", "HEAD"]).stdout.strip()


def prepare_checkout(source: Path, baseline: str, destination: Path) -> None:
    run(
        [
            "git",
            "clone",
            "--quiet",
            "--no-hardlinks",
            "--no-checkout",
            str(source),
            str(destination),
        ],
        timeout=60,
    )
    run(
        ["git", "-C", str(destination), "checkout", "--quiet", "--detach", baseline],
        timeout=30,
    )
    observed = run(
        ["git", "-C", str(destination), "rev-parse", "HEAD"]
    ).stdout.strip()
    if observed != baseline:
        raise RuntimeError("temporary checkout did not resolve the requested baseline")


def search_queries() -> list[dict[str, Any]]:
    return [
        {
            "pattern": "return round(amount - discount_amount, 2)",
            "pattern_mode": "literal",
            "path": "pricing/discount.py",
            "limit": 5,
        },
        {
            "pattern": "def test_zero_discount",
            "pattern_mode": "literal",
            "path": "tests/test_checkout.py",
            "limit": 5,
        },
    ]


def edit_changes(revisions: dict[str, int] | None = None) -> list[dict[str, Any]]:
    revisions = revisions or {}
    discount: dict[str, Any] = {
        "kind": "edit",
        "path": "pricing/discount.py",
        "edits": [
            {
                "kind": "replace_exact",
                "old_text": "return round(amount - discount_amount, 2)",
                "new_text": "return round(max(0.0, amount - discount_amount), 2)",
            }
        ],
    }
    tests: dict[str, Any] = {
        "kind": "edit",
        "path": "tests/test_checkout.py",
        "edits": [
            {
                "kind": "insert_before",
                "anchor_text": "    def test_zero_discount(self):",
                "new_text": NEW_TEST,
            }
        ],
    }
    for change in [discount, tests]:
        revision = revisions.get(change["path"])
        if revision is not None:
            change["expected_read_revision"] = revision
    return [discount, tests]


def baseline_workflow(client: MeasuredClient, project_id: str) -> dict[str, Any]:
    client.invoke(
        "search_project_texts",
        {
            "project": project_id,
            "queries": search_queries(),
            "max_result_bytes": 8192,
        },
    )
    read_output, _ = client.invoke(
        "read_files",
        {
            "project": project_id,
            "items": [
                {"path": "pricing/discount.py"},
                {"path": "tests/test_checkout.py"},
            ],
            "include_read_revision": True,
            "max_result_bytes": 32768,
        },
    )
    revisions = {
        item["path"]: int(item["output"]["read_revision"])
        for item in read_output["items"]
    }
    client.invoke(
        "apply_text_edits",
        {
            "project": project_id,
            "changes": edit_changes(revisions),
        },
    )
    test_output, test_success = client.invoke(
        "run_process",
        {
            "project": project_id,
            "executable": "python3",
            "args": TEST_ARGS,
            "timeout_secs": 30,
            "sync_wait_secs": 30,
            "purpose": "validation",
        },
    )
    review_output, _ = client.invoke(
        "show_changes",
        {
            "project": project_id,
            "include_diff": False,
            "session_event_limit": 0,
        },
    )
    return {
        "internal_steps": 5,
        "retries": 0,
        "test_ok": test_success,
        "review_changed_files": int(
            review_output.get("files_total")
            or len(review_output.get("files") or [])
        ),
        "task_status": "completed",
    }


def phase9_workflow(client: MeasuredClient, project_id: str) -> dict[str, Any]:
    task_arguments = {
            "project": project_id,
            "goal": GOAL,
            "steps": [
                {
                    "kind": "search",
                    "queries": search_queries(),
                    "max_result_bytes": 8192,
                },
                {
                    "kind": "read",
                    "items": [
                        {"path": "pricing/discount.py"},
                        {"path": "tests/test_checkout.py"},
                    ],
                    "with_line_numbers": True,
                    "max_result_bytes": 32768,
                },
                {
                    "kind": "edit",
                    "changes": edit_changes(),
                    "dry_run": False,
                },
                {
                    "kind": "validate",
                    "checks": [
                        {
                            "executable": "python3",
                            "args": TEST_ARGS,
                            "timeout_secs": 30,
                        }
                    ],
                },
                {
                    "kind": "review",
                    "include_diff": False,
                    "max_hunks": 20,
                    "max_hunk_lines": 120,
                },
            ],
            "policy": {
                "max_steps": 8,
                "max_mutations": 4,
                "max_changed_files": 2,
                "timeout_secs": 60,
                "max_result_bytes": 8192,
                "allowed_operations": [
                    "search",
                    "read",
                    "edit",
                    "run_process",
                    "validate",
                    "review",
                ],
            },
            "acceptance": {
                "require_validation_success": True,
                "require_review": True,
            },
        }
    output, _ = client.invoke(
        "call_runtime_tool",
        {"tool": "execute_task", "arguments": task_arguments},
    )
    counters = output.get("counters") or {}
    validation = output.get("validation") or {}
    review = output.get("review") or {}
    return {
        "internal_steps": int(output.get("total_steps", 0)),
        "completed_steps": int(output.get("completed_steps", 0)),
        "retries": int(counters.get("retries", 0)),
        "internal_result_bytes_before": int(counters.get("result_bytes_before", 0)),
        "internal_result_bytes_after": int(counters.get("result_bytes_after", 0)),
        "test_ok": validation.get("status") == "passed",
        "review_changed_files": int(review.get("changed_file_count", 0)),
        "task_status": output.get("status"),
        "result_truncated": bool(output.get("result_truncated", False)),
    }


def changed_files(project: Path) -> list[str]:
    output = run(
        ["git", "-C", str(project), "status", "--porcelain=v1"],
        timeout=10,
    ).stdout
    files: list[str] = []
    for line in output.splitlines():
        if len(line) < 4:
            continue
        path = line[3:]
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        files.append(path)
    return sorted(files)


def diff_sha256(project: Path, baseline: str) -> str:
    completed = subprocess.run(
        ["git", "-C", str(project), "diff", "--binary", baseline, "--"],
        check=True,
        capture_output=True,
        timeout=10,
    )
    return hashlib.sha256(completed.stdout).hexdigest()


def hidden_evaluator(project: Path, baseline: str) -> dict[str, Any]:
    evaluator = """from pricing.cart import Cart
from pricing.checkout import CheckoutService
from pricing.models import LineItem

service = CheckoutService()
edge = service.calculate(Cart([LineItem("Adapter", 25.0)]), discount_amount=40.0, tax_rate=0.1)
assert edge.subtotal == 25.0
assert edge.discounted_subtotal == 0.0
assert edge.tax == 0.0
assert edge.total == 0.0
normal = service.calculate(Cart([LineItem("Keyboard", 100.0)]), discount_amount=20.0, tax_rate=0.05)
assert normal.total == 84.0
try:
    service.calculate(Cart([LineItem("Cable", 10.0)]), discount_amount=-1.0, tax_rate=0.05)
except ValueError:
    pass
else:
    raise AssertionError("negative discount must still fail")
"""
    business = subprocess.run(
        ["python3", "-c", evaluator],
        cwd=project,
        capture_output=True,
        text=True,
        timeout=10,
    )
    tests = subprocess.run(
        ["python3", *TEST_ARGS],
        cwd=project,
        capture_output=True,
        text=True,
        timeout=30,
    )
    diff_check = subprocess.run(
        ["git", "-C", str(project), "diff", "--check", baseline, "--"],
        capture_output=True,
        text=True,
        timeout=10,
    )
    files = changed_files(project)
    expected_files = files == EXPECTED_CHANGED_FILES
    combined_test_text = f"{tests.stdout}\n{tests.stderr}"
    test_count = None
    marker = "Ran "
    if marker in combined_test_text:
        suffix = combined_test_text.split(marker, 1)[1]
        token = suffix.split(" ", 1)[0]
        if token.isdigit():
            test_count = int(token)
    return {
        "passed": (
            business.returncode == 0
            and tests.returncode == 0
            and diff_check.returncode == 0
            and expected_files
        ),
        "business_rule": business.returncode == 0,
        "tests": tests.returncode == 0,
        "tests_run": test_count,
        "git_diff_check": diff_check.returncode == 0,
        "changed_files_exact": expected_files,
        "changed_files": files,
    }


def run_variant(
    *,
    variant: str,
    source: Path,
    baseline: str,
    helper_binary: Path,
    resources: Path,
) -> dict[str, Any]:
    root = Path(tempfile.mkdtemp(prefix=f"chadex-phase9-{variant}-"))
    project = root / "project"
    data_dir = root / "data"
    helper: IsolatedHelper | None = None
    client: MeasuredClient | None = None
    try:
        prepare_checkout(source, baseline, project)
        helper = IsolatedHelper(helper_binary, resources, data_dir)
        helper.request("activateProject", {"path": str(project)})
        helper.request("configureLocalSetup")
        server_url, env_file, project_id = runtime_identity(data_dir)
        token = parse_env_file(env_file).get("WEBCODEX_TOKEN", "").strip()
        if not token:
            raise RuntimeError("isolated bootstrap token is unavailable")
        client = MeasuredClient(server_url, token)

        # Warm protocol/tool registry only. This is outside the measured task.
        client.invoke("runtime_status", {"summary_only": True})
        client.samples.clear()

        started = time.perf_counter()
        if variant == "baseline":
            workflow = baseline_workflow(client, project_id)
        elif variant == "phase9":
            workflow = phase9_workflow(client, project_id)
        else:
            raise RuntimeError(f"unknown benchmark variant: {variant}")
        wall_ms = (time.perf_counter() - started) * 1000.0
        samples = list(client.samples)

        evaluator = hidden_evaluator(project, baseline)
        result = {
            "variant": variant,
            "wall_ms": round(wall_ms, 3),
            "tool_calls": len(samples),
            "request_bytes": sum(int(sample["request_bytes"]) for sample in samples),
            "result_bytes": sum(int(sample["response_bytes"]) for sample in samples),
            "internal_steps": workflow["internal_steps"],
            "retries": workflow["retries"],
            "errors": 0,
            "tests": {
                "workflow_ok": bool(workflow["test_ok"]),
                "hidden_ok": bool(evaluator["tests"]),
                "hidden_count": evaluator["tests_run"],
            },
            "hidden_evaluator": evaluator,
            "changed_files": evaluator["changed_files"],
            "diff_hash": diff_sha256(project, baseline),
            "task_status": workflow["task_status"],
            "review_changed_files": workflow["review_changed_files"],
            "workflow_diagnostics": {
                key: workflow[key]
                for key in [
                    "test_exit_code", "test_execution_state", "test_command_ok", "test_job_status",
                    "test_output_keys", "test_nested_output_keys", "test_nested_exit_code",
                ]
                if key in workflow
            },
        }
        if "completed_steps" in workflow:
            result["completed_steps"] = workflow["completed_steps"]
        if "result_truncated" in workflow:
            result["result_truncated"] = workflow["result_truncated"]
        for key in ["internal_result_bytes_before", "internal_result_bytes_after"]:
            if key in workflow:
                result[key] = workflow[key]
        result["correct"] = (
            workflow["test_ok"]
            and workflow["task_status"] == "completed"
            and evaluator["passed"]
        )
        return result
    except Exception as error:
        samples = list(client.samples) if client is not None else []
        return {
            "variant": variant,
            "correct": False,
            "tool_calls": len(samples),
            "request_bytes": sum(int(sample["request_bytes"]) for sample in samples),
            "result_bytes": sum(int(sample["response_bytes"]) for sample in samples),
            "errors": 1,
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


def median_metric(rows: list[dict[str, Any]], key: str) -> float | None:
    values = [
        float(row[key])
        for row in rows
        if row.get("correct") and isinstance(row.get(key), (int, float))
    ]
    if not values:
        return None
    return round(statistics.median(values), 3)


def summarize(rows: list[dict[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for variant in ["baseline", "phase9"]:
        subset = [row for row in rows if row["variant"] == variant]
        correct = [row for row in subset if row.get("correct")]
        result[variant] = {
            "runs": len(subset),
            "correct_runs": len(correct),
            "wall_ms": timing_summary([float(row["wall_ms"]) for row in correct])
            if correct
            else None,
            "median_tool_calls": median_metric(correct, "tool_calls"),
            "median_request_bytes": median_metric(correct, "request_bytes"),
            "median_result_bytes": median_metric(correct, "result_bytes"),
            "median_internal_steps": median_metric(correct, "internal_steps"),
            "median_internal_result_bytes_before": median_metric(correct, "internal_result_bytes_before"),
            "median_internal_result_bytes_after": median_metric(correct, "internal_result_bytes_after"),
            "total_retries": sum(int(row.get("retries", 0)) for row in subset),
            "total_errors": sum(int(row.get("errors", 0)) for row in subset),
            "diff_hashes": sorted({row.get("diff_hash") for row in correct}),
        }
    baseline = result["baseline"]
    phase9 = result["phase9"]
    if (
        baseline["median_result_bytes"] is not None
        and phase9["median_result_bytes"] is not None
        and baseline["median_result_bytes"] > 0
    ):
        result["result_bytes_reduction_percent"] = round(
            100.0
            * (
                1.0
                - phase9["median_result_bytes"]
                / baseline["median_result_bytes"]
            ),
            2,
        )
    if (
        phase9.get("median_internal_result_bytes_before")
        and phase9.get("median_internal_result_bytes_after") is not None
    ):
        result["internal_result_bytes_reduction_percent"] = round(
            100.0
            * (1.0 - phase9["median_internal_result_bytes_after"]
               / phase9["median_internal_result_bytes_before"]),
            2,
        )
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate-app", type=Path, required=True)
    parser.add_argument("--benchmark-root", type=Path, default=DEFAULT_BENCHMARK)
    parser.add_argument("--iterations", type=int, default=5)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.iterations < 5:
        parser.error("Phase 9 benchmark requires at least 5 iterations per variant")

    app = args.candidate_app.expanduser().resolve()
    source = args.benchmark_root.expanduser().resolve()
    helper_binary, resources = candidate_paths(app)
    baseline = baseline_commit(source)

    rows: list[dict[str, Any]] = []
    variants = ["baseline", "phase9"]
    for iteration in range(1, args.iterations + 1):
        order = variants if iteration % 2 else list(reversed(variants))
        for variant in order:
            row = run_variant(
                variant=variant,
                source=source,
                baseline=baseline,
                helper_binary=helper_binary,
                resources=resources,
            )
            row["iteration"] = iteration
            rows.append(row)
            status = "correct" if row.get("correct") else "failed"
            print(
                f"{variant} {iteration}/{args.iterations}: "
                f"{status}, calls={row.get('tool_calls')}, wall_ms={row.get('wall_ms')}"
            )

    summary = summarize(rows)
    equivalent = (
        summary["baseline"]["correct_runs"] == args.iterations
        and summary["phase9"]["correct_runs"] == args.iterations
        and len(summary["baseline"]["diff_hashes"]) == 1
        and summary["baseline"]["diff_hashes"] == summary["phase9"]["diff_hashes"]
    )
    report = {
        "schema": 1,
        "surface": "isolated local MCP through candidate helper/server/runner",
        "candidate_app": app.name,
        "candidate_helper_sha256": hashlib.sha256(helper_binary.read_bytes()).hexdigest(),
        "baseline_commit": baseline,
        "iterations_per_variant": args.iterations,
        "task": GOAL,
        "measured_scope": "workflow only; runtime setup, baseline clone, and hidden evaluator excluded",
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
