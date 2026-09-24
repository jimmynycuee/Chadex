#!/usr/bin/env python3
"""Compare five direct calls with search/read + one deterministic task.

Isolated local MCP only. Includes workspace prepare/apply/cleanup, but no model,
relay or approval time. Never interprets the break-even estimate as Web evidence.
Uses disposable checkouts only; leaves the fixed five-step harness unchanged.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import sys
import tempfile
import time
import uuid

sys.dont_write_bytecode = True
os.environ["PYTHONDONTWRITEBYTECODE"] = "1"
from benchmark_cold_runtime import IsolatedHelper, timing_summary
from benchmark_phase9_e2e import (
    GOAL, TEST_ARGS, baseline_workflow, diff_sha256, edit_changes,
    hidden_evaluator, prepare_checkout, run, search_queries,
)
from benchmark_turn_economy import MeasuredClient
from benchmark_workflow import parse_env_file, runtime_identity

BASELINE = "49f793fd66b054a38923c3f6e3ccd9c1aca6579f"
FILES = ["pricing/discount.py", "tests/test_checkout.py"]
REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_BENCHMARK = REPO_ROOT.parent / "Benchmark-Chadex"


def fused_workflow(client, project):
    client.invoke("search_project_texts", {
        "project": project, "queries": search_queries(), "max_result_bytes": 8192,
    })
    observed, _ = client.invoke("read_files", {
        "project": project, "items": [{"path": p} for p in FILES],
        "include_read_revision": True, "with_line_numbers": False, "max_result_bytes": 32768,
    })
    # Source revisions are Project-scoped and cannot be sent as worktree
    # revisions. The existing executor guards its own snapshot reads. This
    # fixture has no concurrent writers; it does NOT measure source-read-to-task
    # stale-context equivalence. Exact anchors + evaluator verify the result.
    changes = edit_changes()
    for change, item in zip(changes, observed["items"], strict=True):
        if item["path"] != change["path"] or not isinstance(item["output"].get("text"), str):
            raise RuntimeError("missing_source_read")
    output, success = client.invoke("call_runtime_tool", {
        "tool": "execute_task", "arguments": {
            "project": project, "task_id": "chadex_task_" + uuid.uuid4().hex,
            "goal": GOAL,
            "steps": [
                {"kind": "edit", "changes": changes},
                {"kind": "validate", "checks": [{
                    "executable": "python3", "args": TEST_ARGS, "timeout_secs": 30,
                }]},
                {"kind": "review", "include_diff": False},
            ],
            "policy": {
                "max_steps": 3, "max_mutations": 2, "max_changed_files": 2,
                "timeout_secs": 120, "max_result_bytes": 8192,
                "allowed_operations": ["read", "edit", "validate", "review"],
            },
            "acceptance": {"require_validation_success": True, "require_review": True},
        },
    }, expect_success=False)
    workspace = output.get("workspace") or {}
    return {
        "success": success and output.get("execution_state") == "succeeded"
        and workspace.get("state") == "cleaned"
        and workspace.get("cleanup_pending") is not True,
        "task_status": output.get("status"),
        "execution_state": output.get("execution_state"),
        "test_ok": output.get("validation", {}).get("status") == "passed",
        "completed_steps": output.get("completed_steps"),
        "workspace_state": workspace.get("state"),
        "workspace_timings": workspace.get("timings"),
        "counters": output.get("counters"),
        "failure_kind": (output.get("failure") or {}).get("kind"),
        "error_kind": output.get("error_kind"),
        "reason_code": output.get("reason_code"),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, required=True)
    parser.add_argument("--reference-app", type=Path, help="Optional same-run reference bundle; alternate all four arms")
    parser.add_argument("--source", type=Path, default=DEFAULT_BENCHMARK)
    parser.add_argument("--iterations", type=int, default=20)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.iterations < 1:
        parser.error("iterations must be positive")
    candidates = [("reference/", args.reference_app), ("candidate/", args.app)] if args.reference_app else [("", args.app)]
    identities = {}
    for prefix, app in candidates:
        contents = app.resolve() / "Contents"
        binaries = [contents / "Helpers/chadex-helper"] + [contents / "Resources/chadex-runtime" / name for name in (
            "chadex-runtime-cli", "chadex-runtime-server", "chadex-runtime-runner",
        )]
        identities[prefix.rstrip("/") or "candidate"] = {
            str(p.relative_to(contents)): hashlib.sha256(p.read_bytes()).hexdigest() for p in binaries
        }
    rows, variants = [], {}
    report = {
        "surface": "isolated local MCP; no tunnel, approval or model",
        "baseline": BASELINE, "binary_sha256": identities,
        "one_warmup_per_variant": True, "rows": rows,
        "iterations_per_arm": args.iterations,
        "concurrent_writers": False,
        "guards": "direct uses source read_revision; fused guards its execution snapshot, not the prior source read",
        "timing_note": "workspace validation_ms is a subset of execution_ms; do not sum all workspace fields",
    }
    try:
        with tempfile.TemporaryDirectory(prefix="chadex-completion-") as directory:
            root = Path(directory)
            try:
                for label, app in [(prefix + mode, app) for prefix, app in candidates for mode in ("direct", "fused")]:
                    contents = app.resolve() / "Contents"
                    helper_binary = contents / "Helpers/chadex-helper"
                    resources = contents / "Resources"
                    project = root / label / "project"
                    project.parent.mkdir(parents=True)
                    prepare_checkout(args.source, BASELINE, project)
                    data = root / label / "data"
                    helper = IsolatedHelper(helper_binary, resources, data)
                    variants[label] = (helper, None, project, None)
                    helper.request("activateProject", {"path": str(project)})
                    helper.request("configureLocalSetup")
                    url, env, pid = runtime_identity(data)
                    client = MeasuredClient(url, parse_env_file(env)["WEBCODEX_TOKEN"])
                    variants[label] = (helper, client, project, pid)
                for iteration in range(args.iterations + 1):
                    for label in (list(variants) if iteration % 2 == 0 else list(reversed(variants))):
                        _, client, project, pid = variants[label]
                        # Reset only this harness's two files in its disposable clone.
                        run(["git", "restore", "--source=" + BASELINE, "--worktree", "--", *FILES], cwd=project)
                        offset = len(client.samples)
                        start = time.perf_counter()
                        row = {"variant": label, "iteration": iteration, "warmup": iteration == 0}
                        try:
                            result = baseline_workflow(client, pid) if label.endswith("direct") else fused_workflow(client, pid)
                            row["workflow_ms"] = (time.perf_counter() - start) * 1000
                            row["result"] = result
                            row["evaluator"] = hidden_evaluator(project, BASELINE)
                            row["diff_sha256"] = diff_sha256(project, BASELINE)
                            row["passed"] = row["evaluator"]["passed"] and result.get("test_ok") is True and result.get("success", True)
                        except Exception as error:
                            row["workflow_ms"] = (time.perf_counter() - start) * 1000
                            row["passed"] = False
                            row["error_type"] = type(error).__name__
                            if isinstance(error, RuntimeError):
                                row["error"] = str(error)[:300]
                        row["samples"] = client.samples[offset:]
                        row["outer_calls"] = len(row["samples"])
                        row["response_bytes"] = sum(s["response_bytes"] for s in row["samples"])
                        rows.append(row)
                        if not row["passed"]:
                            # Never replay an ambiguous mutation; abort this isolated run.
                            raise RuntimeError("benchmark_correctness_failed")
            finally:
                for helper, client, _, _ in variants.values():
                    if client:
                        client.close()
                    helper.close()
        measured = [r for r in rows if not r["warmup"]]
        if len({r["diff_sha256"] for r in rows}) != 1:
            raise RuntimeError("variant_diffs_differ")
        report["summary"] = {label: {
            "workflow_ms": timing_summary([r["workflow_ms"] for r in measured if r["variant"] == label]),
            "outer_calls": sorted({r["outer_calls"] for r in measured if r["variant"] == label}),
            "response_bytes_median": statistics.median(r["response_bytes"] for r in measured if r["variant"] == label),
        } for label in variants}
        by_pair = {(r["iteration"], r["variant"]): r for r in measured}
        # Hypothetical uniform external overhead per saved call; NOT predicted E2E latency.
        report["paired_break_even_external_ms_per_saved_call"] = {
            prefix.rstrip("/") or "candidate": statistics.median(
                max(0, (by_pair[i, prefix + "fused"]["workflow_ms"] - by_pair[i, prefix + "direct"]["workflow_ms"]) / 2)
                for i in range(1, args.iterations + 1)
            ) for prefix, _ in candidates
        }
        if args.reference_app:
            report["paired_candidate_minus_reference_ms"] = {
                mode: timing_summary([
                    by_pair[i, "candidate/" + mode]["workflow_ms"] - by_pair[i, "reference/" + mode]["workflow_ms"]
                    for i in range(1, args.iterations + 1)
                ]) for mode in ("direct", "fused")
            }
        report["passed"] = True
    except Exception as error:
        report["passed"] = False
        report["error_type"] = type(error).__name__
        if isinstance(error, RuntimeError):
            report["error"] = str(error)[:300]
    finally:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({k: v for k, v in report.items() if k not in ("rows", "binary_sha256")}, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
