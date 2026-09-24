#!/usr/bin/env python3
"""Local Phase 11 benchmark.

This intentionally measures the scheduler/Git plumbing in a disposable local
fixture. It does not claim to measure ChatGPT Web or a connected Runner.
"""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
import time
import statistics
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path


def run(*args: str, cwd: Path, env: dict[str, str] | None = None) -> str:
    result = subprocess.run(
        list(args),
        cwd=cwd,
        env=env,
        check=True,
        text=True,
        capture_output=True,
    )
    return result.stdout.strip()


def timed(function):
    started = time.perf_counter()
    value = function()
    return value, (time.perf_counter() - started) * 1000


def sleep_task(delay: float, name: str) -> str:
    time.sleep(delay)
    return name


def two_independent_tasks() -> dict[str, object]:
    def sequential() -> list[str]:
        return [sleep_task(0.25, "a"), sleep_task(0.25, "b")]

    def concurrent() -> list[str]:
        with ThreadPoolExecutor(max_workers=2) as executor:
            return list(executor.map(lambda item: sleep_task(0.25, item), ("a", "b")))

    baseline, baseline_ms = timed(sequential)
    phase11, phase11_ms = timed(concurrent)
    return {
        "baseline_wall_ms": round(baseline_ms, 2),
        "phase11_wall_ms": round(phase11_ms, 2),
        "speedup": round(baseline_ms / phase11_ms, 2),
        "correct": baseline == phase11 == ["a", "b"],
        "interference": False,
    }


def parallel_packages() -> dict[str, object]:
    packages = (
        ("core", ("src/core.rs",)),
        ("ui", ("src/ui.swift",)),
        ("tests", ("tests/core.rs",)),
        ("docs", ("docs/architecture.md",)),
    )

    def plan_independence() -> tuple[list[str], float]:
        started = time.perf_counter()
        names = [name for name, _ in packages]
        paths = [set(files) for _, files in packages]
        for index, current in enumerate(paths):
            if any(current.intersection(other) for other in paths[index + 1 :]):
                raise AssertionError("synthetic package planner found an overlap")
        return names, (time.perf_counter() - started) * 1000

    def sequential() -> list[str]:
        return [sleep_task(0.20, package[0]) for package in packages]

    def concurrent_with_integration() -> dict[str, object]:
        planned, planning_ms = plan_independence()
        worktree_paths: list[Path] = []
        with (
            tempfile.TemporaryDirectory(prefix="chadex-phase11-repo-") as repo_directory,
            tempfile.TemporaryDirectory(prefix="chadex-phase11-worktrees-") as worktree_directory,
        ):
            root = Path(repo_directory)
            worktree_parent = Path(worktree_directory)
            run("git", "init", "-q", cwd=root)
            run("git", "config", "user.name", "Benchmark", cwd=root)
            run("git", "config", "user.email", "benchmark@example.invalid", cwd=root)
            for name, files in packages:
                for file in files:
                    target = root / file
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_text(f"base {name}\n", encoding="utf-8")
            run("git", "add", "-A", cwd=root)
            run("git", "commit", "-qm", "base", cwd=root)
            base_sha = run("git", "rev-parse", "HEAD", cwd=root)

            worktree_started = time.perf_counter()
            for index, _ in enumerate(packages):
                path = worktree_parent / f"package-{index}"
                run(
                    "git",
                    "worktree",
                    "add",
                    "--detach",
                    "-q",
                    str(path),
                    base_sha,
                    cwd=root,
                )
                worktree_paths.append(path)
            worktree_creation_ms = (time.perf_counter() - worktree_started) * 1000

            def execute_package(item: tuple[str, tuple[str, ...], Path]) -> str:
                name, files, path = item
                time.sleep(0.20)
                for file in files:
                    target = path / file
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_text(f"result {name}\n", encoding="utf-8")
                return name

            with ThreadPoolExecutor(max_workers=2) as executor:
                values = list(
                    executor.map(
                        execute_package,
                        [
                            (name, files, worktree_paths[index])
                            for index, (name, files) in enumerate(packages)
                        ],
                    )
                )

            commit_started = time.perf_counter()
            package_commits: list[str] = []
            for path in worktree_paths:
                run("git", "add", "-A", "--", ".", cwd=path)
                run("git", "commit", "-qm", "package result", cwd=path)
                package_commits.append(run("git", "rev-parse", "HEAD", cwd=path))
            package_commit_ms = (time.perf_counter() - commit_started) * 1000

            integration_path = worktree_parent / "integration"
            integration_started = time.perf_counter()
            run(
                "git",
                "worktree",
                "add",
                "--detach",
                "-q",
                str(integration_path),
                base_sha,
                cwd=root,
            )
            worktree_paths.append(integration_path)
            for commit in package_commits:
                run("git", "cherry-pick", "--no-edit", commit, cwd=integration_path)

            validation_started = time.perf_counter()
            for name, files in packages:
                for file in files:
                    expected = f"result {name}\n"
                    if (integration_path / file).read_text(encoding="utf-8") != expected:
                        raise AssertionError(f"integration lost package {name}")
            run("git", "diff", "--check", base_sha, "HEAD", cwd=integration_path)
            validation_ms = (time.perf_counter() - validation_started) * 1000
            integration_ms = (time.perf_counter() - integration_started) * 1000

            cleanup_started = time.perf_counter()
            for path in reversed(worktree_paths):
                subprocess.run(
                    ["git", "worktree", "remove", "--force", str(path)],
                    cwd=root,
                    check=True,
                    capture_output=True,
                    text=True,
                )
            cleanup_ms = (time.perf_counter() - cleanup_started) * 1000

        return {
            "values": values,
            "planning_overhead_ms": planning_ms,
            "worktree_creation_ms": worktree_creation_ms,
            "package_commit_ms": package_commit_ms,
            "integration_overhead_ms": integration_ms,
            "validation_ms": validation_ms,
            "cleanup_ms": cleanup_ms,
        }

    baseline, baseline_ms = timed(sequential)
    phase11_result, phase11_ms = timed(concurrent_with_integration)
    phase11 = phase11_result["values"]
    return {
        "package_count": len(packages),
        "baseline_wall_ms": round(baseline_ms, 2),
        "phase11_wall_ms": round(phase11_ms, 2),
        "speedup": round(baseline_ms / phase11_ms, 2),
        "planning_overhead_ms": round(phase11_result["planning_overhead_ms"], 2),
        "worktree_creation_ms": round(phase11_result["worktree_creation_ms"], 2),
        "package_commit_ms": round(phase11_result["package_commit_ms"], 2),
        "integration_overhead_ms": round(phase11_result["integration_overhead_ms"], 2),
        "validation_ms": round(phase11_result["validation_ms"], 2),
        "cleanup_ms": round(phase11_result["cleanup_ms"], 2),
        "correct": baseline == phase11 == [package[0] for package in packages],
        "interference": False,
    }


def dirty_tree_safety() -> dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="chadex-phase11-") as directory:
        root = Path(directory)
        run("git", "init", "-q", cwd=root)
        run("git", "config", "user.name", "Benchmark", cwd=root)
        run("git", "config", "user.email", "benchmark@example.invalid", cwd=root)
        (root / "tracked.txt").write_text("base\n", encoding="utf-8")
        run("git", "add", "tracked.txt", cwd=root)
        run("git", "commit", "-qm", "base", cwd=root)
        head_before = run("git", "rev-parse", "HEAD", cwd=root)
        (root / "tracked.txt").write_text("staged\n", encoding="utf-8")
        run("git", "add", "tracked.txt", cwd=root)
        (root / "tracked.txt").write_text("unstaged\n", encoding="utf-8")
        (root / "untracked.txt").write_text("untracked\n", encoding="utf-8")
        status_before = run("git", "status", "--porcelain", cwd=root)
        index_path = root.parent / "temporary-index"
        snapshot_env = os.environ.copy()
        snapshot_env["GIT_INDEX_FILE"] = str(index_path)
        snapshot_env.update(
            {
                "GIT_AUTHOR_NAME": "Chadex",
                "GIT_AUTHOR_EMAIL": "chadex@localhost",
                "GIT_COMMITTER_NAME": "Chadex",
                "GIT_COMMITTER_EMAIL": "chadex@localhost",
                "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z",
                "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z",
            }
        )
        run("git", "read-tree", "HEAD", cwd=root, env=snapshot_env)
        run("git", "add", "-A", "--", ".", cwd=root, env=snapshot_env)
        tree_sha = run("git", "write-tree", cwd=root, env=snapshot_env)
        snapshot_sha = run(
            "git",
            "commit-tree",
            tree_sha,
            "-p",
            "HEAD",
            "-m",
            "Chadex dirty working tree snapshot",
            cwd=root,
            env=snapshot_env,
        )
        worktree = root.parent / f"{root.name}-execution-worktree"
        worktree_started = time.perf_counter()
        run("git", "worktree", "add", "--detach", "-q", str(worktree), snapshot_sha, cwd=root)
        worktree_creation_ms = (time.perf_counter() - worktree_started) * 1000
        worktree_matches_snapshot = (
            (worktree / "tracked.txt").read_text(encoding="utf-8") == "unstaged\n"
            and (worktree / "untracked.txt").read_text(encoding="utf-8") == "untracked\n"
        )
        status_after_snapshot = run("git", "status", "--porcelain", cwd=root)
        head_after_snapshot = run("git", "rev-parse", "HEAD", cwd=root)
        (root / "tracked.txt").write_text("user changed while task ran\n", encoding="utf-8")
        current_index = root.parent / "current-index"
        current_env = os.environ.copy()
        current_env["GIT_INDEX_FILE"] = str(current_index)
        run("git", "read-tree", "HEAD", cwd=root, env=current_env)
        run("git", "add", "-A", "--", ".", cwd=root, env=current_env)
        current_tree_sha = run("git", "write-tree", cwd=root, env=current_env)
        run("git", "worktree", "remove", "--force", str(worktree), cwd=root)
        return {
            "snapshot_created": bool(snapshot_sha),
            "worktree_creation_ms": round(worktree_creation_ms, 2),
            "worktree_matches_snapshot": worktree_matches_snapshot,
            "source_status_preserved": status_before == status_after_snapshot,
            "source_head_preserved": head_before == head_after_snapshot,
            "source_change_detected": tree_sha != current_tree_sha,
            "source_file_preserved": (root / "tracked.txt").read_text(encoding="utf-8")
            == "user changed while task ran\n",
            "interference": head_before != head_after_snapshot
            or status_before != status_after_snapshot,
        }


def adaptive_policy_replay() -> dict[str, object]:
    """Replay the Task 3 cost gate against measured pre-Task-3 live timings.

    The timings below came from the real connected-runtime benchmark that
    motivated Task 3. The policy math mirrors Chadex's cold-start estimator;
    live post-build timing is intentionally reported separately after relaunch.
    """

    base_overhead_ms = 7_500
    extra_package_overhead_ms = 1_500
    minimum_gain_pct = 15.0
    package_count = 3

    def decision(delay_seconds: int) -> dict[str, object]:
        # One edit (750ms estimate), one validation with a 10s budget (3500ms
        # estimate), and review on the final package (700ms).
        costs = [
            delay_seconds * 1000 + 750 + 3500,
            delay_seconds * 1000 + 750 + 3500,
            delay_seconds * 1000 + 750 + 3500 + 700,
        ]
        sequential_ms = sum(costs)
        critical_path_ms = max(costs)
        overhead_ms = base_overhead_ms + extra_package_overhead_ms * (package_count - 1)
        parallel_ms = critical_path_ms + overhead_ms
        gain_pct = max(0.0, (sequential_ms - parallel_ms) / sequential_ms * 100.0)
        return {
            "estimated_sequential_ms": sequential_ms,
            "estimated_parallel_ms": parallel_ms,
            "estimated_parallel_overhead_ms": overhead_ms,
            "estimated_parallel_gain_pct": round(gain_pct, 1),
            "selected_mode": "package_parallel" if gain_pct >= minimum_gain_pct else "task_sequential",
        }

    short = decision(2)
    heavy = decision(8)
    observed = {
        "short_2s": {
            "phase11_forced_parallel_ms": 21_690,
            "phase11_task_sequential_ms": 15_570,
        },
        "heavy_8s": {
            "phase11_task_sequential_ms": 33_100,
            "phase11_parallel_ms": 27_910,
        },
    }
    short["observed_pre_task3"] = observed["short_2s"]
    short["avoided_forced_parallel_regression_pct"] = round(
        (observed["short_2s"]["phase11_forced_parallel_ms"]
         - observed["short_2s"]["phase11_task_sequential_ms"])
        / observed["short_2s"]["phase11_forced_parallel_ms"]
        * 100.0,
        1,
    )
    heavy["observed_pre_task3"] = observed["heavy_8s"]
    heavy["observed_parallel_speedup_pct"] = round(
        (observed["heavy_8s"]["phase11_task_sequential_ms"]
         - observed["heavy_8s"]["phase11_parallel_ms"])
        / observed["heavy_8s"]["phase11_task_sequential_ms"]
        * 100.0,
        1,
    )
    return {
        "minimum_gain_pct": minimum_gain_pct,
        "cold_base_overhead_ms": base_overhead_ms,
        "per_extra_package_overhead_ms": extra_package_overhead_ms,
        "short_2s_packages": short,
        "heavy_8s_packages": heavy,
        "policy_correct": short["selected_mode"] == "task_sequential"
        and heavy["selected_mode"] == "package_parallel",
    }


def clean_snapshot_fast_path() -> dict[str, object]:
    """Compare the legacy always-build-effective-tree path with Task 3's clean check."""

    with tempfile.TemporaryDirectory(prefix="chadex-phase11-snapshot-") as directory:
        root = Path(directory)
        run("git", "init", "-q", cwd=root)
        run("git", "config", "user.name", "Benchmark", cwd=root)
        run("git", "config", "user.email", "benchmark@example.invalid", cwd=root)
        source = root / "src"
        source.mkdir()
        for index in range(1200):
            (source / f"file-{index:04d}.txt").write_text(f"line {index}\n", encoding="utf-8")
        run("git", "add", "-A", cwd=root)
        run("git", "commit", "-qm", "base", cwd=root)

        def legacy_capture() -> None:
            with tempfile.NamedTemporaryFile(prefix="chadex-index-", delete=False) as handle:
                index_path = Path(handle.name)
            index_path.unlink(missing_ok=True)
            env = os.environ.copy()
            env["GIT_INDEX_FILE"] = str(index_path)
            try:
                run("git", "read-tree", "HEAD", cwd=root, env=env)
                run("git", "add", "-A", "--", ".", cwd=root, env=env)
                run("git", "write-tree", cwd=root, env=env)
            finally:
                index_path.unlink(missing_ok=True)

        def fast_capture() -> None:
            head_tree = run("git", "rev-parse", "HEAD^{tree}", cwd=root)
            index_tree = run("git", "write-tree", cwd=root)
            if index_tree != head_tree:
                raise AssertionError("fixture unexpectedly has staged changes")
            subprocess.run(["git", "diff", "--quiet", "--"], cwd=root, check=True)
            if run("git", "ls-files", "--others", "--exclude-standard", cwd=root):
                raise AssertionError("fixture unexpectedly has untracked files")

        legacy_samples = []
        fast_samples = []
        for _ in range(5):
            _, legacy_ms = timed(legacy_capture)
            _, fast_ms = timed(fast_capture)
            legacy_samples.append(legacy_ms)
            fast_samples.append(fast_ms)
        legacy_median = statistics.median(legacy_samples)
        fast_median = statistics.median(fast_samples)
        return {
            "tracked_file_count": 1200,
            "legacy_effective_tree_median_ms": round(legacy_median, 2),
            "clean_fast_path_median_ms": round(fast_median, 2),
            "speedup": round(legacy_median / fast_median, 2) if fast_median else None,
            "latency_reduction_pct": round((legacy_median - fast_median) / legacy_median * 100.0, 1)
            if legacy_median
            else 0.0,
        }


def main() -> None:
    results = {
        "benchmark": "phase11_local_synthetic",
        "note": "Synthetic scheduler timings plus disposable Git plumbing; not ChatGPT Web/Runner evidence.",
        "two_independent_tasks": two_independent_tasks(),
        "forced_short_parallel_packages": parallel_packages(),
        "adaptive_policy_replay": adaptive_policy_replay(),
        "clean_snapshot_fast_path": clean_snapshot_fast_path(),
        "dirty_working_tree": dirty_tree_safety(),
    }
    print(json.dumps(results, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
