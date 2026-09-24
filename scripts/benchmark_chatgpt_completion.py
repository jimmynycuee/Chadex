#!/usr/bin/env python3
"""Measure ChatGPT submit-to-final time for paired connected coding tasks.

Each arm uses an already-registered connector and a dedicated, clean benchmark
repository. This harness never switches a live runtime or resets unknown edits.
The browser driver must confirm the final UI response; correctness is graded
outside the model turn before the two owned files are reset.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import shutil
from pathlib import Path
import statistics
import subprocess
import sys
import time

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
DRIVER = ROOT / "scripts/webcodex_chatgpt_driver.mjs"
FIXTURE = ROOT / "scripts/benchmark_connected_fixture.py"
BASELINE = "49f793fd66b054a38923c3f6e3ccd9c1aca6579f"
EXPECTED_DIFF = "69d104a260d4803da81676ce24ac8067533e622ab71625131b49c8d86682fe63"
OWNED_PATHS = ["pricing/discount.py", "tests/test_checkout.py"]
PROJECT_NAMES = {"Benchmark-Chadex": "chadex", "Benchmark-WebCodex": "webcodex"}
ARM_NAMES = {"chadex-current", "chadex-candidate", "webcodex"}
DEFAULT_PROMPT = (
    "In the registered {project} project, ensure a fixed discount never makes "
    "the payable subtotal negative. Clamp an excessive fixed discount at zero "
    "and add a regression test, then run the full unittest suite and review "
    "the final changes."
)
DEFAULT_NODE = Path(
    os.environ.get("CHADEX_BENCHMARK_NODE")
    or shutil.which("node")
    or (Path.home() / ".cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node")
)


def git(fixture: Path, *args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=fixture, timeout=30).decode()


def fixture_state(fixture: Path) -> tuple[str, str]:
    if not fixture.is_dir() or fixture.name not in PROJECT_NAMES:
        raise RuntimeError(f"fixture must be a registered Benchmark-Chadex/WebCodex repo: {fixture}")
    if git(fixture, "rev-parse", "HEAD").strip() != BASELINE:
        raise RuntimeError(f"fixture HEAD differs from pinned baseline: {fixture}")
    status = git(fixture, "status", "--porcelain=v1", "--untracked-files=all")
    diff = hashlib.sha256(git(fixture, "diff", "--", *OWNED_PATHS).encode()).hexdigest()
    return status, diff


def assert_clean(fixture: Path) -> None:
    status, _ = fixture_state(fixture)
    if status:
        raise RuntimeError(f"fixture is not clean; refusing to start: {fixture}: {status[:300]}")


def verify_and_reset(fixture: Path) -> dict:
    status, diff = fixture_state(fixture)
    changed = sorted(git(fixture, "diff", "--name-only").splitlines())
    expected_status = sorted(" M " + path for path in OWNED_PATHS)
    if sorted(status.splitlines()) != expected_status or changed != sorted(OWNED_PATHS) or diff != EXPECTED_DIFF:
        raise RuntimeError(
            f"unexpected fixture state; left untouched for review: {fixture} "
            f"status={status[:300]!r} diff={diff}"
        )
    provider = PROJECT_NAMES[fixture.name]
    tests = subprocess.run(
        [sys.executable, "-B", "-m", "unittest", "discover", "-s", "tests", "-v"],
        cwd=fixture, capture_output=True, text=True, timeout=120, check=False,
    )
    diff_check = subprocess.run(
        ["git", "diff", "--check"], cwd=fixture,
        capture_output=True, text=True, timeout=30, check=False,
    )
    evaluation = subprocess.run(
        [sys.executable, "-B", str(FIXTURE), provider, "cycle", "--root", str(fixture.parent)],
        capture_output=True, text=True, timeout=90, check=True,
    )
    assert_clean(fixture)
    return {
        "business": json.loads(evaluation.stdout.strip().splitlines()[-1]),
        "visible_tests_exit_code": tests.returncode,
        "git_diff_check_exit_code": diff_check.returncode,
        "expected_diff": diff,
        "passed": tests.returncode == 0 and diff_check.returncode == 0,
    }


def validate_config(payload: dict) -> tuple[list[dict], str]:
    arms = payload.get("arms")
    if not isinstance(arms, list) or len(arms) not in (2, 3):
        raise ValueError("config requires two or three arms")
    template = payload.get("prompt_template", DEFAULT_PROMPT)
    if not isinstance(template, str) or template.count("{project}") != 1:
        raise ValueError("prompt_template must contain {project} exactly once")
    names, connectors, fixtures = set(), set(), set()
    for arm in arms:
        if not isinstance(arm, dict) or set(arm) != {"name", "connector", "fixture"}:
            raise ValueError("each arm requires only name, connector, fixture")
        if any(not isinstance(arm[key], str) or not arm[key].strip() for key in arm):
            raise ValueError("arm fields must be nonempty strings")
        if arm["name"] not in ARM_NAMES:
            raise ValueError("arm name must be chadex-current, chadex-candidate or webcodex")
        fixture = Path(arm["fixture"]).expanduser().resolve()
        if arm["name"] in names or arm["connector"] in connectors or fixture in fixtures:
            raise ValueError("arms require distinct names, connectors and fixtures")
        if fixture.name not in PROJECT_NAMES:
            raise ValueError("fixture basename must be Benchmark-Chadex or Benchmark-WebCodex")
        arm["fixture"] = str(fixture)
        names.add(arm["name"])
        connectors.add(arm["connector"])
        fixtures.add(fixture)
    return arms, template


def percentile95(values: list[float]) -> float:
    return sorted(values)[math.ceil(0.95 * len(values)) - 1]


def summarize(rows: list[dict], names: list[str]) -> dict:
    by_arm = {}
    for name in names:
        good = [row["submit_to_visible_final_ms"] for row in rows if row["arm"] == name and row["passed"]]
        by_arm[name] = {
            "valid_runs": len(good),
            "median_ms": statistics.median(good) if good else None,
            "p95_ms": percentile95(good) if good else None,
        }
    paired = {}
    first = names[0]
    for other in names[1:]:
        by_iteration = {(row["iteration"], row["arm"]): row for row in rows if row["passed"]}
        diffs = [
            by_iteration[(index, first)]["submit_to_visible_final_ms"]
            - by_iteration[(index, other)]["submit_to_visible_final_ms"]
            for index in sorted({row["iteration"] for row in rows})
            if (index, first) in by_iteration and (index, other) in by_iteration
        ]
        paired[f"{first}_minus_{other}"] = {
            "valid_pairs": len(diffs),
            "median_ms": statistics.median(diffs) if diffs else None,
        }
    return {"arms": by_arm, "paired": paired}


def acceptance(summary: dict) -> dict:
    arms = summary["arms"]
    required = {"chadex-current", "chadex-candidate", "webcodex"}
    if set(arms) != required:
        return {"evaluated": False, "reason": "three_connected_arms_required"}
    current, candidate, webcodex = (arms[name] for name in (
        "chadex-current", "chadex-candidate", "webcodex",
    ))
    if min(item["valid_runs"] for item in (current, candidate, webcodex)) < 20:
        return {"evaluated": False, "reason": "fewer_than_20_valid_runs_per_arm"}
    saved = current["median_ms"] - candidate["median_ms"]
    median_gain = saved >= 2000 and candidate["median_ms"] <= 0.8 * current["median_ms"]
    beats_webcodex = candidate["median_ms"] < webcodex["median_ms"]
    tail_ok = candidate["p95_ms"] <= 1.1 * current["p95_ms"]
    return {
        "evaluated": True,
        "median_saved_ms": saved,
        "median_gain_target_met": median_gain,
        "beats_webcodex_median": beats_webcodex,
        "candidate_p95_within_10_percent_of_current": tail_ok,
        "performance_target_met": median_gain and beats_webcodex and tail_ok,
        "release_ready": False,
        "release_note": "Verify candidate build identity and same-host/approval conditions separately before acceptance.",
    }


def write_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")
    path.chmod(0o600)


def call_driver(arm: dict, template: str, output_dir: Path, node: Path, timeout: int, preflight: bool) -> tuple[dict, int]:
    output_dir.mkdir(parents=True, exist_ok=True)
    prompt = template.replace("{project}", Path(arm["fixture"]).name)
    prompt_path = output_dir / "prompt.txt"
    prompt_path.write_text(prompt)
    prompt_path.chmod(0o600)
    result_path = output_dir / "driver.json"
    env = os.environ.copy()
    env.update({
        "BENCH_PROMPT_FILE": str(prompt_path),
        "BENCH_DRIVER_RESULT": str(result_path),
        "BENCH_WEBCODEX_CONNECTOR_NAME": arm["connector"],
        "BENCH_DRIVER_TIMEOUT_MS": str(timeout * 1000),
        "BENCH_PREFLIGHT_ONLY": "1" if preflight else "0",
    })
    completed = subprocess.run(
        [str(node), str(DRIVER)], env=env, cwd=ROOT,
        capture_output=True, text=True, timeout=timeout + 90, check=False,
    )
    result = json.loads(result_path.read_text()) if result_path.exists() else {"error": "driver_result_missing"}
    result["prompt_sha256"] = hashlib.sha256(prompt.encode()).hexdigest()
    result["driver_exit_code"] = completed.returncode
    return result, completed.returncode


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--iterations", type=int, default=20)
    parser.add_argument("--timeout-secs", type=int, default=600)
    parser.add_argument("--node", type=Path, default=DEFAULT_NODE)
    parser.add_argument("--preflight", action="store_true")
    args = parser.parse_args()
    if args.iterations < 1 or args.timeout_secs < 30:
        parser.error("iterations must be positive and timeout at least 30 seconds")
    arms, template = validate_config(json.loads(args.config.read_text()))
    if not args.node.is_file() or not DRIVER.is_file():
        parser.error("Node.js or browser driver is unavailable")
    for arm in arms:
        assert_clean(Path(arm["fixture"]))
    if args.output_dir.exists() and any(args.output_dir.iterdir()):
        parser.error("output directory must be new or empty")
    report = {
        "surface": "ChatGPT browser, submit to visible final answer",
        "model_effort": "extra-high; browser driver verifies control",
        "baseline": BASELINE,
        "prompt_template_sha256": hashlib.sha256(template.encode()).hexdigest(),
        "arms": arms,
        "iterations_requested": args.iterations,
        "runtime_builds_verified": False,
        "tool_calls": None,
        "token_context_usage": None,
        "rows": [],
        "valid_e2e": False,
    }
    output = args.output_dir / "result.json"
    write_json(output, report)
    for arm in arms:
        result, exit_code = call_driver(
            arm, template, args.output_dir / "preflight" / arm["name"],
            args.node, args.timeout_secs, True,
        )
        if exit_code or result.get("ready") is not True:
            report["blocked"] = {"arm": arm["name"], "reason": result.get("error", "preflight_failed")}
            write_json(output, report)
            return 2
    if args.preflight:
        report["preflight_ready"] = True
        write_json(output, report)
        return 0
    for iteration in range(args.iterations):
        order = arms if iteration % 2 == 0 else list(reversed(arms))
        for arm in order:
            fixture = Path(arm["fixture"])
            assert_clean(fixture)
            start = time.time_ns() // 1_000_000
            driver, exit_code = call_driver(
                arm, template, args.output_dir / f"run-{iteration:02d}-{arm['name']}",
                args.node, args.timeout_secs, False,
            )
            row = {
                "iteration": iteration, "arm": arm["name"],
                "started_at_ms": start,
                "driver": driver,
                "submit_to_visible_final_ms": driver.get("submit_to_visible_final_ms"),
                "passed": False,
            }
            try:
                row["evaluation"] = verify_and_reset(fixture)
                row["passed"] = (
                    exit_code == 0 and driver.get("completed") is True
                    and driver.get("temporary_chat") is True
                    and driver.get("connector") == arm["connector"]
                    and driver.get("submission_count") == 1
                    and isinstance(row["submit_to_visible_final_ms"], int)
                    and row["submit_to_visible_final_ms"] >= 0
                    and row["evaluation"]["passed"]
                )
            except Exception as exc:
                row["error"] = str(exc)
                report["rows"].append(row)
                write_json(output, report)
                return 2
            report["rows"].append(row)
            write_json(output, report)
            if not row["passed"]:
                report["blocked"] = {"arm": arm["name"], "reason": "incorrect_or_incomplete_run"}
                write_json(output, report)
                return 2
    report["summary"] = summarize(report["rows"], [arm["name"] for arm in arms])
    report["acceptance"] = acceptance(report["summary"])
    report["valid_e2e"] = True
    write_json(output, report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
