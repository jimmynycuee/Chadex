#!/usr/bin/env python3
"""Terminal-only WebCodex vs codex-chatgpt-web Full-mode benchmark harness.

The measured agents never see the hidden evaluator. Each provider receives its own
fresh clone at the same pinned commit, and grading happens only after the provider
process has terminated.

WebCodex intentionally requires an external ChatGPT Web driver command. The
WebCodex CLI can expose a project as an MCP runtime, but it cannot itself submit a
prompt to ChatGPT Web. This harness will not substitute direct connector replay,
because that would no longer be an end-to-end agent benchmark.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import sqlite3
import subprocess
import sys
import time
from typing import Any, Iterable


REPO_ROOT = Path(__file__).resolve().parents[1]
BENCH_ROOT = REPO_ROOT / "benchmarks" / "terminal-ab"
RESULTS_ROOT = BENCH_ROOT / "results"

WEBCODEX_BIN = Path(
    "/Applications/WebCodex Desktop.app/Contents/Resources/"
    "webcodex-runtime/webcodex"
)
CODEX_BIN = Path(
    os.environ.get("CHADEX_CODEX_BIN")
    or shutil.which("codex")
    or (Path.home() / ".local/bin/codex")
)
CGW_VERSION = os.environ.get("CHADEX_CGW_VERSION", "5.0.8-darwin-arm64")
CGW_ROOT = Path(
    os.environ.get("CHADEX_CGW_ROOT")
    or (Path.home() / ".codex-chatgpt-web" / "versions" / CGW_VERSION)
)
CGW_BUN = CGW_ROOT / "runtime" / "bun"
CGW_CLI = CGW_ROOT / "app" / "cli.js"
CGW_DIAGNOSTICS = Path.home() / ".codex-chatgpt-web" / "diagnostics" / "browser-turns"

SHORT_SOURCE = Path(
    os.environ.get("CHADEX_BENCHMARK_SOURCE")
    or (REPO_ROOT.parent / "Benchmark-Chadex")
)
SHORT_BASELINE = "49f793fd66b054a38923c3f6e3ccd9c1aca6579f"
SHORT_EXPECTED_DIFF = "69d104a260d4803da81676ce24ac8067533e622ab71625131b49c8d86682fe63"
SHORT_PATHS = ["pricing/discount.py", "tests/test_checkout.py"]
SHORT_PROMPT = (
    "Ensure a fixed discount never makes the payable subtotal negative. "
    "Clamp an excessive fixed discount at zero and add a regression test, "
    "then run the full unittest suite and review the final changes."
)

LONG_SOURCE = REPO_ROOT / "benchmarks" / "phase10-quality" / "source"
LONG_PROMPT = REPO_ROOT / "benchmarks" / "phase10-quality" / "prompt.txt"
LONG_HIDDEN = REPO_ROOT / "benchmarks" / "phase10-quality" / "evaluator" / "hidden_tests.py"

BUILD_TEST_RE = re.compile(
    r"(?:\bpytest\b|\bunittest\b|\bcargo\s+(?:test|build)\b|"
    r"\bswift\s+(?:test|build)\b|\bnpm\s+(?:test|run\s+build)\b|"
    r"\bpnpm\s+(?:test|build)\b|\byarn\s+(?:test|build)\b|"
    r"\bflutter\s+(?:test|build)\b|\bxcodebuild\b)",
    re.IGNORECASE,
)


@dataclass(frozen=True)
class TaskSpec:
    name: str
    source: Path
    baseline: str
    prompt: str


def task_spec(name: str, short_source: Path = SHORT_SOURCE) -> TaskSpec:
    if name == "short":
        return TaskSpec(name, short_source, SHORT_BASELINE, SHORT_PROMPT)
    if name == "long":
        return TaskSpec(name, LONG_SOURCE, "generated-per-run", LONG_PROMPT.read_text())
    raise ValueError(name)


def run(
    argv: list[str],
    *,
    cwd: Path | None = None,
    timeout: int = 60,
    check: bool = True,
    input_text: str | None = None,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        argv,
        cwd=cwd,
        input=input_text,
        capture_output=True,
        text=True,
        timeout=timeout,
        check=check,
        env=env,
    )


def git(cwd: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return run(["git", *args], cwd=cwd, timeout=60, check=check)


def materialize_spec(spec: TaskSpec, run_dir: Path) -> TaskSpec:
    if not spec.source.is_dir():
        raise RuntimeError(f"benchmark source is missing: {spec.source}")
    if spec.name == "short":
        observed = git(spec.source, "rev-parse", "HEAD").stdout.strip()
        if observed != spec.baseline:
            raise RuntimeError(
                f"short source baseline mismatch: expected {spec.baseline}, observed {observed}"
            )
        if git(spec.source, "status", "--porcelain=v1").stdout.strip():
            raise RuntimeError("short benchmark source has local changes; refusing to use it")
        return spec

    source_repo = run_dir / "canonical-source"
    shutil.copytree(
        spec.source,
        source_repo,
        ignore=shutil.ignore_patterns("__pycache__", "*.pyc", ".DS_Store"),
    )
    git(source_repo, "init", "-q")
    git(source_repo, "config", "user.name", "Terminal A/B Benchmark")
    git(source_repo, "config", "user.email", "terminal-ab@example.invalid")
    git(source_repo, "add", ".")
    commit_env = os.environ.copy()
    commit_env.update(
        {
            "GIT_AUTHOR_DATE": "2000-01-01T00:00:00+0000",
            "GIT_COMMITTER_DATE": "2000-01-01T00:00:00+0000",
        }
    )
    run(
        ["git", "commit", "-qm", "terminal A/B canonical task fixture"],
        cwd=source_repo,
        env=commit_env,
    )
    baseline = git(source_repo, "rev-parse", "HEAD").stdout.strip()
    return TaskSpec(spec.name, source_repo, baseline, spec.prompt)


def prepare_fixture(spec: TaskSpec, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    run(
        [
            "git",
            "clone",
            "--quiet",
            "--no-hardlinks",
            "--no-checkout",
            str(spec.source),
            str(destination),
        ],
        timeout=90,
    )
    git(destination, "checkout", "--quiet", "--detach", spec.baseline)
    observed = git(destination, "rev-parse", "HEAD").stdout.strip()
    if observed != spec.baseline:
        raise RuntimeError(
            f"fixture baseline mismatch: expected {spec.baseline}, observed {observed}"
        )


def diff_sha256(fixture: Path, baseline: str, paths: list[str] | None = None) -> str:
    argv = ["git", "diff", "--binary", baseline, "--"]
    if paths:
        argv.extend(paths)
    completed = run(argv, cwd=fixture, timeout=30)
    return hashlib.sha256(completed.stdout.encode()).hexdigest()


def changed_files(fixture: Path, baseline: str) -> list[str]:
    completed = git(fixture, "diff", "--name-only", baseline, "--")
    return sorted(x for x in completed.stdout.splitlines() if x)


def visible_tests(fixture: Path) -> dict[str, Any]:
    completed = run(
        [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-v"],
        cwd=fixture,
        timeout=120,
        check=False,
    )
    return {
        "passed": completed.returncode == 0,
        "exit_code": completed.returncode,
        "stdout_tail": completed.stdout[-4000:],
        "stderr_tail": completed.stderr[-4000:],
    }


def evaluate_short(fixture: Path, spec: TaskSpec) -> dict[str, Any]:
    code = r'''
import json
from pricing.discount import DiscountPolicy

passed = 0
failures = []
for amount in [0, 0.01, 1, 25, 100, 9999.99]:
    for discount in [0, 0.01, 1, 25, 40, 100, 10000]:
        expected = round(max(0, amount - discount), 2)
        try:
            observed = DiscountPolicy().apply_fixed(amount, discount)
            if observed == expected:
                passed += 1
            else:
                failures.append([amount, discount, observed, expected])
        except Exception as exc:
            failures.append([amount, discount, type(exc).__name__])
try:
    DiscountPolicy().apply_fixed(10, -1)
except ValueError:
    passed += 1
except Exception as exc:
    failures.append(["negative", type(exc).__name__])
else:
    failures.append(["negative", "accepted"])
print(json.dumps({"passed": passed, "total": 43, "failures": failures}))
raise SystemExit(0 if passed == 43 else 1)
'''
    hidden = run([sys.executable, "-c", code], cwd=fixture, timeout=30, check=False)
    try:
        hidden_payload = json.loads(hidden.stdout.strip().splitlines()[-1])
    except (IndexError, json.JSONDecodeError):
        hidden_payload = {"passed": 0, "total": 43, "failures": [hidden.stderr[-1000:]]}
    files = changed_files(fixture, spec.baseline)
    scoped_hash = diff_sha256(fixture, spec.baseline, SHORT_PATHS)
    diff_check = git(fixture, "diff", "--check", spec.baseline, "--", check=False)
    visible = visible_tests(fixture)
    exact_files = files == sorted(SHORT_PATHS)
    exact_diff = scoped_hash == SHORT_EXPECTED_DIFF
    correctness = exact_files and exact_diff and diff_check.returncode == 0
    return {
        "hidden_tests": {
            **hidden_payload,
            "rate": hidden_payload.get("passed", 0) / max(hidden_payload.get("total", 1), 1),
        },
        "visible_tests": visible,
        "changed_files": files,
        "diff_sha256": scoped_hash,
        "diff_correctness": {
            "passed": correctness,
            "mode": "exact",
            "changed_files_exact": exact_files,
            "canonical_diff_exact": exact_diff,
            "git_diff_check": diff_check.returncode == 0,
        },
        "passed": hidden.returncode == 0 and visible["passed"] and correctness,
    }


def evaluate_long(fixture: Path, spec: TaskSpec) -> dict[str, Any]:
    hidden = run(
        [sys.executable, str(LONG_HIDDEN), str(fixture)],
        cwd=REPO_ROOT,
        timeout=120,
        check=False,
    )
    match = re.search(r"HIDDEN_SCORE=(\d+)/(\d+)", hidden.stdout + hidden.stderr)
    passed_count = int(match.group(1)) if match else 0
    total = int(match.group(2)) if match else 10
    visible = visible_tests(fixture)
    diff_check = git(fixture, "diff", "--check", spec.baseline, "--", check=False)
    files = changed_files(fixture, spec.baseline)
    behavioral = hidden.returncode == 0 and diff_check.returncode == 0
    return {
        "hidden_tests": {
            "passed": passed_count,
            "total": total,
            "rate": passed_count / max(total, 1),
            "stdout_tail": hidden.stdout[-4000:],
            "stderr_tail": hidden.stderr[-4000:],
        },
        "visible_tests": visible,
        "changed_files": files,
        "diff_sha256": diff_sha256(fixture, spec.baseline),
        "diff_correctness": {
            "passed": behavioral,
            "mode": "behavioral",
            "git_diff_check": diff_check.returncode == 0,
            "note": "Long task accepts multiple valid diffs; hidden behavior + diff hygiene define correctness.",
        },
        "passed": behavioral and visible["passed"],
    }


def evaluate(fixture: Path, spec: TaskSpec) -> dict[str, Any]:
    return evaluate_short(fixture, spec) if spec.name == "short" else evaluate_long(fixture, spec)


def walk_strings(value: Any) -> Iterable[str]:
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for child in value.values():
            yield from walk_strings(child)
    elif isinstance(value, list):
        for child in value:
            yield from walk_strings(child)


def load_jsonl(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    if not path.is_file():
        return rows
    for line in path.read_text(errors="replace").splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            rows.append(value)
    return rows


def all_fixture_files(fixture: Path) -> set[str]:
    result: set[str] = set()
    for path in fixture.rglob("*"):
        if not path.is_file() or ".git" in path.parts:
            continue
        result.add(path.relative_to(fixture).as_posix())
    return result


def explicit_paths_from_values(values: Iterable[str], fixture: Path) -> set[str]:
    known = all_fixture_files(fixture)
    found: set[str] = set()
    for text in values:
        normalized = text.replace("\\", "/")
        for relative in known:
            if relative in normalized or str(fixture / relative) in normalized:
                found.add(relative)
    return found


def command_strings(event: dict[str, Any]) -> list[str]:
    result: list[str] = []
    for key in ("command", "cmd", "argv"):
        value = event.get(key)
        if isinstance(value, str):
            result.append(value)
        elif isinstance(value, list):
            result.append(" ".join(str(x) for x in value))
    item = event.get("item")
    if isinstance(item, dict):
        result.extend(command_strings(item))
    return result


def is_read_evidence(item: dict[str, Any]) -> bool:
    item_type = str(item.get("type", "")).lower()
    if item_type == "command_execution":
        command = " ".join(command_strings(item)).lower()
        return bool(
            re.search(
                r"(?:^|[;&|]\s*|\s)(?:cat|sed|rg|grep|head|tail|find|ls)\b",
                command,
            )
        )
    if item_type in {"mcp_tool_call", "dynamic_tool_call", "tool_call"}:
        text = " ".join(walk_strings(item)).lower()
        return any(
            marker in text
            for marker in (
                "read_file",
                "read_files",
                "search_project_texts",
                "list_project_files",
            )
        )
    return False


def parse_codex_jsonl(path: Path, fixture: Path) -> dict[str, Any]:
    events = load_jsonl(path)
    item_tools: list[dict[str, Any]] = []
    direct_tool_begins: list[dict[str, Any]] = []
    item_shells: list[dict[str, Any]] = []
    direct_shells: list[dict[str, Any]] = []
    retry_events: list[str] = []
    compact_events: list[str] = []
    turn_completed = False
    usage_total: dict[str, int] = {}

    tool_item_types = {
        "command_execution",
        "mcp_tool_call",
        "file_change",
        "dynamic_tool_call",
        "tool_call",
        "web_search",
    }
    for event in events:
        event_type = str(event.get("type", ""))
        lowered_type = event_type.lower()
        item = event.get("item") if isinstance(event.get("item"), dict) else None
        item_type = str(item.get("type", "")) if item else ""
        if event_type == "item.completed" and item and item_type in tool_item_types:
            item_tools.append(item)
            if item_type == "command_execution":
                item_shells.append(item)
        if lowered_type.endswith("_begin") and any(
            marker in lowered_type for marker in ("exec_command", "mcp_tool_call", "tool_call")
        ):
            direct_tool_begins.append(event)
            if "exec_command" in lowered_type:
                direct_shells.append(event)
        if "retry" in lowered_type:
            retry_events.append(event_type)
        if "compact" in lowered_type:
            compact_events.append(event_type)
        if event_type == "turn.completed":
            turn_completed = True
            usage = event.get("usage")
            if isinstance(usage, dict):
                for key, value in usage.items():
                    if isinstance(value, int):
                        usage_total[key] = usage_total.get(key, 0) + value

    selected_tools = item_tools if item_tools else direct_tool_begins
    selected_shells = item_shells if item_tools else direct_shells
    read_strings: list[str] = []
    commands: list[str] = []
    for event in selected_tools:
        if is_read_evidence(event):
            read_strings.extend(walk_strings(event))
        commands.extend(command_strings(event))
    explicit_files = explicit_paths_from_values(read_strings, fixture)
    build_test_calls = sum(1 for command in commands if BUILD_TEST_RE.search(command))
    return {
        "events": len(events),
        "tool_calls": len(selected_tools),
        "tool_call_schema": "item.completed" if item_tools else "*_begin",
        "shell_calls": len(selected_shells),
        "build_test_calls": build_test_calls,
        "files_read": {
            "value": len(explicit_files),
            "paths": sorted(explicit_files),
            "exact": False,
            "note": "Lower bound from explicit file paths present in Codex JSONL tool/command events.",
        },
        "runtime_retries": len(retry_events),
        "context_compactions": len(compact_events),
        "turn_completed": turn_completed,
        "token_context_usage": usage_total or None,
    }


def snapshot_diagnostics(root: Path) -> dict[str, tuple[int, int]]:
    snapshot: dict[str, tuple[int, int]] = {}
    try:
        for path in root.glob("*.json"):
            stat = path.stat()
            snapshot[str(path)] = (stat.st_mtime_ns, stat.st_size)
    except OSError:
        return {}
    return snapshot


def collect_diagnostics_delta(
    root: Path,
    before: dict[str, tuple[int, int]],
    destination: Path,
) -> tuple[list[Path], str | None]:
    destination.mkdir(parents=True, exist_ok=True)
    copied: list[Path] = []
    try:
        for path in root.glob("*.json"):
            stat = path.stat()
            key = str(path)
            if before.get(key) == (stat.st_mtime_ns, stat.st_size):
                continue
            target = destination / path.name
            shutil.copy2(path, target)
            copied.append(target)
    except OSError as exc:
        return copied, str(exc)
    return copied, None


def parse_browser_diagnostics(paths: list[Path]) -> dict[str, Any]:
    stages: list[str] = []
    retries = 0
    failures: list[str] = []
    compaction_handoffs = 0
    for path in paths:
        try:
            payload = json.loads(path.read_text())
        except (OSError, json.JSONDecodeError):
            continue
        for text in walk_strings(payload):
            lowered = text.lower()
            if any(
                marker in lowered
                for marker in (
                    "browser-page-acquired",
                    "effort-selected",
                    "prompt-attachment-complete",
                    "send-accepted",
                    "response-visible",
                    "response-stalled",
                    "turn-completed",
                    "turn-failed",
                    "compaction-handoff",
                )
            ):
                stages.append(text)
            if "retry" in lowered:
                retries += 1
            if any(marker in lowered for marker in ("turn-failed", "response-stalled", "handoff-failed")):
                failures.append(text)
            if "compaction-handoff-accepted" in lowered:
                compaction_handoffs += 1
    return {
        "files": len(paths),
        "stages": stages,
        "retries": retries,
        "compaction_handoffs": compaction_handoffs,
        "continuation_failure": bool(failures),
        "failure_markers": failures,
    }


def cgw_doctor(bun: Path, cli: Path) -> tuple[dict[str, Any] | None, str]:
    completed = run([str(bun), str(cli), "doctor", "--json"], timeout=30, check=False)
    try:
        payload = json.loads(completed.stdout)
    except json.JSONDecodeError:
        payload = None
    return payload, completed.stderr


def run_cgw(
    fixture: Path,
    spec: TaskSpec,
    artifact_dir: Path,
    *,
    codex_bin: Path,
    cgw_bun: Path,
    cgw_cli: Path,
    model: str,
    timeout: int,
    allow_unready: bool,
) -> dict[str, Any]:
    artifact_dir.mkdir(parents=True, exist_ok=True)
    total_start = time.perf_counter()
    doctor, doctor_stderr = cgw_doctor(cgw_bun, cgw_cli)
    (artifact_dir / "doctor.json").write_text(
        json.dumps(doctor, indent=2, ensure_ascii=False) if doctor is not None else "null\n"
    )
    if doctor_stderr:
        (artifact_dir / "doctor.stderr.log").write_text(doctor_stderr)
    ready = bool(doctor and doctor.get("ok") and doctor.get("mode") == "full")
    if not ready and not allow_unready:
        return {
            "provider": "codex-chatgpt-web-full",
            "task": spec.name,
            "status": "blocked_preflight",
            "valid_e2e": False,
            "reason": "codex-chatgpt-web doctor is not ready in Full mode",
            "wall_time_s": time.perf_counter() - total_start,
            "doctor": doctor,
        }

    before = snapshot_diagnostics(CGW_DIAGNOSTICS)
    stdout_path = artifact_dir / "codex.jsonl"
    stderr_path = artifact_dir / "codex.stderr.log"
    last_message_path = artifact_dir / "last-message.txt"
    argv = [
        str(codex_bin),
        "exec",
        "--json",
        "--ephemeral",
        "-m",
        model,
        "-s",
        "workspace-write",
        "-C",
        str(fixture),
        "-o",
        str(last_message_path),
        "-",
    ]
    agent_start = time.perf_counter()
    timed_out = False
    with stdout_path.open("w") as stdout_file, stderr_path.open("w") as stderr_file:
        process = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE,
            stdout=stdout_file,
            stderr=stderr_file,
            text=True,
        )
        try:
            process.communicate(spec.prompt, timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            process.kill()
            process.wait(timeout=15)
    agent_wall = time.perf_counter() - agent_start
    wall = time.perf_counter() - total_start
    after_paths, diag_error = collect_diagnostics_delta(
        CGW_DIAGNOSTICS, before, artifact_dir / "browser-turns"
    )
    parsed = parse_codex_jsonl(stdout_path, fixture)
    browser = parse_browser_diagnostics(after_paths)
    evaluation = evaluate(fixture, spec)
    retries = parsed["runtime_retries"] + browser["retries"]
    continuation_failure = bool(browser["continuation_failure"])
    if timed_out:
        continuation_failure = True
    process_ok = process.returncode == 0 and not timed_out
    one_shot = process_ok and evaluation["passed"] and retries == 0 and not continuation_failure
    return {
        "provider": "codex-chatgpt-web-full",
        "task": spec.name,
        "status": "completed" if process_ok else "failed",
        "valid_e2e": process_ok,
        "model": model,
        "effort": model.rsplit("/", 1)[-1] if "/" in model else None,
        "wall_time_s": wall,
        "agent_wall_time_s": agent_wall,
        "tool_calls": parsed["tool_calls"],
        "files_read": parsed["files_read"],
        "shell_calls": parsed["shell_calls"],
        "build_test_calls": parsed["build_test_calls"],
        "retries": retries,
        "token_context_usage": parsed["token_context_usage"],
        "hidden_tests": evaluation["hidden_tests"],
        "diff_correctness": evaluation["diff_correctness"],
        "one_shot_complete": one_shot,
        "context_continuation_failure": continuation_failure,
        "context_compactions": parsed["context_compactions"],
        "compaction_handoffs": browser["compaction_handoffs"],
        "process_exit_code": process.returncode,
        "timed_out": timed_out,
        "diagnostics_error": diag_error,
        "browser_diagnostics": browser,
        "evaluation": evaluation,
    }


def decode_first_json(text: str) -> dict[str, Any] | None:
    stripped = text.lstrip()
    if not stripped:
        return None
    decoder = json.JSONDecoder()
    try:
        value, _ = decoder.raw_decode(stripped)
    except json.JSONDecodeError:
        return None
    return value if isinstance(value, dict) else None


def wait_for_webcodex_share(
    process: subprocess.Popen[str],
    log_path: Path,
    timeout: int,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        text = log_path.read_text(errors="replace") if log_path.exists() else ""
        payload = decode_first_json(text)
        if payload is not None:
            if payload.get("ok") is False:
                raise RuntimeError(json.dumps(payload.get("error"), ensure_ascii=False))
            if payload.get("ok") is True:
                return payload
        if process.poll() is not None:
            raise RuntimeError(
                f"WebCodex share exited before ready (exit={process.returncode}): {text[-2000:]}"
            )
        time.sleep(0.1)
    raise TimeoutError("WebCodex share did not become ready before timeout")


def find_url(value: Any) -> str | None:
    for text in walk_strings(value):
        if text.startswith("https://") or text.startswith("http://"):
            return text
    return None


def find_webcodex_db(state_dir: Path) -> Path | None:
    for path in state_dir.rglob("*.db"):
        try:
            with sqlite3.connect(path) as connection:
                row = connection.execute(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name='action_events'"
                ).fetchone()
            if row:
                return path
        except sqlite3.Error:
            continue
    return None


def parse_json_text(text: str) -> Any:
    try:
        return json.loads(text)
    except (TypeError, json.JSONDecodeError):
        return None


def parse_webcodex_runtime(state_dir: Path, fixture: Path) -> dict[str, Any]:
    db = find_webcodex_db(state_dir)
    if db is None:
        return {
            "tool_calls": None,
            "shell_calls": None,
            "build_test_calls": None,
            "build_test_calls_reason": "isolated WebCodex action_events DB not found",
            "files_read": {"value": None, "reason": "isolated WebCodex action_events DB not found"},
            "runtime_retries": None,
            "db": None,
        }
    rows: list[sqlite3.Row]
    with sqlite3.connect(db) as connection:
        connection.row_factory = sqlite3.Row
        rows = connection.execute(
            "SELECT operation, action_name, endpoint, status, summary_json, changed_files_json "
            "FROM action_events ORDER BY started_at"
        ).fetchall()
        retry_kinds: list[str] = []
        try:
            retry_kinds = [
                str(row[0])
                for row in connection.execute(
                    "SELECT kind FROM wc_task_events WHERE lower(kind) LIKE '%retry%'"
                ).fetchall()
            ]
        except sqlite3.Error:
            pass
    tool_rows = [row for row in rows if row["endpoint"] == "/mcp" or row["action_name"] == "toolsCall"]
    shell_rows = [
        row
        for row in tool_rows
        if str(row["operation"] or "") in {"run_process", "run_shell", "session_shell_exec"}
    ]
    read_texts: list[str] = []
    for row in tool_rows:
        summary = parse_json_text(row["summary_json"])
        if summary is not None:
            values = list(walk_strings(summary))
            if str(row["operation"] or "") in {
                "read_files",
                "read_file",
                "search_project_texts",
                "list_project_files",
            }:
                read_texts.extend(values)
    explicit_files = explicit_paths_from_values(read_texts, fixture)
    read_ops = sum(
        1
        for row in tool_rows
        if str(row["operation"] or "") in {"read_files", "read_file", "search_project_texts", "list_project_files"}
    )
    files_metric: dict[str, Any]
    if explicit_files:
        files_metric = {
            "value": len(explicit_files),
            "paths": sorted(explicit_files),
            "exact": False,
            "note": "Lower bound from explicit paths in isolated WebCodex action summaries.",
        }
    elif read_ops:
        files_metric = {
            "value": None,
            "reason": "WebCodex logged read/search calls but did not expose exact file paths in action summaries.",
        }
    else:
        files_metric = {"value": 0, "paths": [], "exact": True}
    return {
        "tool_calls": len(tool_rows),
        "shell_calls": len(shell_rows),
        "build_test_calls": None if shell_rows else 0,
        "build_test_calls_reason": (
            "WebCodex action_events records run_process/run_shell operations but not executable/args."
            if shell_rows
            else None
        ),
        "files_read": files_metric,
        "runtime_retries": len(retry_kinds),
        "failed_tool_calls": sum(1 for row in tool_rows if row["status"] != "success"),
        "db": str(db),
    }


def load_driver_result(path: Path) -> dict[str, Any] | None:
    if not path.is_file():
        return None
    try:
        value = json.loads(path.read_text())
    except json.JSONDecodeError:
        return None
    return value if isinstance(value, dict) else None


def stop_child(process: subprocess.Popen[str]) -> None:
    if process.stdin is not None:
        try:
            process.stdin.close()
        except OSError:
            pass
    try:
        process.wait(timeout=15)
    except subprocess.TimeoutExpired:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def run_webcodex(
    fixture: Path,
    spec: TaskSpec,
    artifact_dir: Path,
    *,
    webcodex_bin: Path,
    driver_command: str,
    tunnel: str,
    auth: str,
    timeout: int,
    share_timeout: int,
    model_label: str,
    effort_label: str,
) -> dict[str, Any]:
    artifact_dir.mkdir(parents=True, exist_ok=True)
    state_dir = artifact_dir / "webcodex-state"
    state_dir.mkdir(parents=True, exist_ok=True)
    share_log = artifact_dir / "webcodex-share.log"
    prompt_file = artifact_dir / "prompt.txt"
    prompt_file.write_text(spec.prompt)
    driver_result_path = artifact_dir / "driver-result.json"
    driver_stdout = artifact_dir / "driver.stdout.log"
    driver_stderr = artifact_dir / "driver.stderr.log"
    run_identity = artifact_dir.parents[1].name
    profile = f"terminal-ab-{run_identity}-{spec.name}".lower()[:60]
    argv = [
        str(webcodex_bin),
        "share",
        "--root",
        str(fixture),
        "--profile",
        profile,
        "--state-dir",
        str(state_dir),
        "--json",
        "--tunnel",
        tunnel,
        "--auth",
        auth,
        "--no-copy-url",
        "--stop-on-stdin-eof",
    ]
    total_start = time.perf_counter()
    share_file = share_log.open("w")
    share = subprocess.Popen(
        argv,
        stdin=subprocess.PIPE,
        stdout=share_file,
        stderr=subprocess.STDOUT,
        text=True,
    )
    try:
        ready = wait_for_webcodex_share(share, share_log, share_timeout)
    except Exception as exc:
        stop_child(share)
        share_file.close()
        return {
            "provider": "webcodex",
            "task": spec.name,
            "status": "blocked_runtime",
            "valid_e2e": False,
            "reason": str(exc),
            "wall_time_s": time.perf_counter() - total_start,
        }

    connection = ready.get("connection") if isinstance(ready.get("connection"), dict) else {}
    share_url = connection.get("mcp_url") if isinstance(connection.get("mcp_url"), str) else None
    ready_path = artifact_dir / "webcodex-share-ready.json"
    ready_path.write_text(json.dumps(ready, indent=2, ensure_ascii=False))
    env = os.environ.copy()
    env.update(
        {
            "BENCH_TASK": spec.name,
            "BENCH_FIXTURE_ROOT": str(fixture),
            "BENCH_PROMPT_FILE": str(prompt_file),
            "BENCH_WEBCODEX_STATE_DIR": str(state_dir),
            "BENCH_WEBCODEX_SHARE_JSON": str(ready_path),
            "BENCH_WEBCODEX_MCP_URL": share_url or "",
            "BENCH_WEBCODEX_AUTH": auth,
            "BENCH_WEBCODEX_SHARE_PID": str(share.pid),
            "BENCH_DRIVER_RESULT": str(driver_result_path),
            "BENCH_MODEL": model_label,
            "BENCH_EFFORT": effort_label,
        }
    )
    driver_start = time.perf_counter()
    timed_out = False
    with driver_stdout.open("w") as out, driver_stderr.open("w") as err:
        driver = subprocess.Popen(
            shlex.split(driver_command),
            cwd=fixture,
            env=env,
            stdout=out,
            stderr=err,
            text=True,
        )
        try:
            driver.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            driver.kill()
            driver.wait(timeout=10)
    agent_wall = time.perf_counter() - driver_start
    stop_child(share)
    share_file.close()
    total_wall = time.perf_counter() - total_start
    runtime = parse_webcodex_runtime(state_dir, fixture)
    driver_result = load_driver_result(driver_result_path)
    evaluation = evaluate(fixture, spec)
    process_ok = driver.returncode == 0 and not timed_out
    observed_runtime = isinstance(runtime.get("tool_calls"), int) and runtime["tool_calls"] > 0
    valid_e2e = process_ok and observed_runtime
    model_retries = driver_result.get("retries") if driver_result else None
    context_failure = (
        driver_result.get("context_continuation_failure") if driver_result else None
    )
    token_context = driver_result.get("token_context_usage") if driver_result else None
    one_shot = valid_e2e and evaluation["passed"]
    if isinstance(model_retries, int) and model_retries > 0:
        one_shot = False
    if context_failure is True:
        one_shot = False
    return {
        "provider": "webcodex",
        "task": spec.name,
        "status": "completed" if process_ok else "failed",
        "valid_e2e": valid_e2e,
        "validity_note": (
            "Driver completed and isolated WebCodex MCP activity was observed."
            if valid_e2e
            else "A valid run requires both driver completion and isolated WebCodex MCP activity."
        ),
        "model": model_label,
        "effort": effort_label,
        "wall_time_s": total_wall,
        "agent_wall_time_s": agent_wall,
        "tool_calls": runtime["tool_calls"],
        "files_read": runtime["files_read"],
        "shell_calls": runtime["shell_calls"],
        "build_test_calls": runtime["build_test_calls"],
        "retries": model_retries,
        "runtime_retries": runtime["runtime_retries"],
        "token_context_usage": token_context,
        "hidden_tests": evaluation["hidden_tests"],
        "diff_correctness": evaluation["diff_correctness"],
        "one_shot_complete": one_shot,
        "context_continuation_failure": context_failure,
        "process_exit_code": driver.returncode,
        "timed_out": timed_out,
        "runtime": runtime,
        "driver_result": driver_result,
        "evaluation": evaluation,
    }


def metric_value(result: dict[str, Any], key: str) -> Any:
    value = result.get(key)
    if key == "files_read" and isinstance(value, dict):
        return value.get("value")
    if key == "hidden_tests" and isinstance(value, dict):
        return f"{value.get('passed')}/{value.get('total')}"
    if key == "diff_correctness" and isinstance(value, dict):
        return value.get("passed")
    if key == "token_context_usage" and isinstance(value, dict):
        return value
    return value


def comparison(a: dict[str, Any], b: dict[str, Any]) -> dict[str, Any]:
    keys = [
        "wall_time_s",
        "tool_calls",
        "files_read",
        "shell_calls",
        "build_test_calls",
        "retries",
        "token_context_usage",
        "hidden_tests",
        "diff_correctness",
        "one_shot_complete",
        "context_continuation_failure",
    ]
    return {
        "task": a.get("task") or b.get("task"),
        "A_webcodex": {key: metric_value(a, key) for key in keys},
        "B_codex_chatgpt_web_full": {key: metric_value(b, key) for key in keys},
        "valid": bool(a.get("valid_e2e") and b.get("valid_e2e")),
        "notes": [
            "wall_time_s is measured externally by the parent Terminal harness.",
            "files_read is an explicit-log-path lower bound unless exact=true.",
            "WebCodex token/context and model-level retry/continuation metrics are null unless the external ChatGPT Web driver reports them.",
        ],
    }


def run_metadata(
    run_dir: Path,
    spec: TaskSpec,
    args: argparse.Namespace,
    provider_order: str,
) -> dict[str, Any]:
    prompt_sha256 = hashlib.sha256(spec.prompt.encode()).hexdigest()
    b_effort = args.cgw_model.rsplit("/", 1)[-1] if "/" in args.cgw_model else None
    return {
        "run_id": run_dir.name,
        "task": spec.name,
        "baseline_sha": spec.baseline,
        "prompt_sha256": prompt_sha256,
        "provider_order": provider_order,
        "fixture_policy": (
            "Two independent fresh Git clones from the same exact baseline SHA; "
            "no source code or execution state is shared between A and B."
        ),
        "hidden_grading_policy": (
            "Hidden evaluator is outside both provider fixtures and runs only after "
            "the measured provider process exits."
        ),
        "providers": {
            "A_webcodex": {
                "model_label": args.model_label,
                "effort_label": args.effort_label,
                "tunnel": args.webcodex_tunnel,
                "auth": args.webcodex_auth,
            },
            "B_codex_chatgpt_web_full": {
                "model": args.cgw_model,
                "effort_inferred_from_model_route": b_effort,
            },
        },
    }


def write_result(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n")


def make_run_dir(task: str, output_root: Path) -> Path:
    stamp = time.strftime("%Y%m%d-%H%M%S")
    base = output_root / f"{stamp}-{task}"
    candidate = base
    index = 1
    while candidate.exists():
        index += 1
        candidate = output_root / f"{base.name}-{index}"
    candidate.mkdir(parents=True)
    return candidate


def run_pair(args: argparse.Namespace, task_name: str) -> dict[str, Any]:
    run_dir = make_run_dir(task_name, args.output_root)
    spec = materialize_spec(task_spec(task_name, args.short_source), run_dir)
    metadata = run_metadata(run_dir, spec, args, args.order)
    write_result(run_dir / "run-metadata.json", metadata)
    a_dir = run_dir / "A-webcodex"
    b_dir = run_dir / "B-codex-chatgpt-web-full"
    a_fixture = a_dir / "fixture"
    b_fixture = b_dir / "fixture"
    prepare_fixture(spec, a_fixture)
    prepare_fixture(spec, b_fixture)
    if args.order == "AB":
        a = run_webcodex(
            a_fixture,
            spec,
            a_dir / "artifacts",
            webcodex_bin=args.webcodex_bin,
            driver_command=args.webcodex_driver_command,
            tunnel=args.webcodex_tunnel,
            auth=args.webcodex_auth,
            timeout=args.timeout,
            share_timeout=args.share_timeout,
            model_label=args.model_label,
            effort_label=args.effort_label,
        )
        b = run_cgw(
            b_fixture,
            spec,
            b_dir / "artifacts",
            codex_bin=args.codex_bin,
            cgw_bun=args.cgw_bun,
            cgw_cli=args.cgw_cli,
            model=args.cgw_model,
            timeout=args.timeout,
            allow_unready=args.allow_unready_cgw,
        )
    else:
        b = run_cgw(
            b_fixture,
            spec,
            b_dir / "artifacts",
            codex_bin=args.codex_bin,
            cgw_bun=args.cgw_bun,
            cgw_cli=args.cgw_cli,
            model=args.cgw_model,
            timeout=args.timeout,
            allow_unready=args.allow_unready_cgw,
        )
        a = run_webcodex(
            a_fixture,
            spec,
            a_dir / "artifacts",
            webcodex_bin=args.webcodex_bin,
            driver_command=args.webcodex_driver_command,
            tunnel=args.webcodex_tunnel,
            auth=args.webcodex_auth,
            timeout=args.timeout,
            share_timeout=args.share_timeout,
            model_label=args.model_label,
            effort_label=args.effort_label,
        )
    write_result(a_dir / "result.json", a)
    write_result(b_dir / "result.json", b)
    compared = comparison(a, b)
    compared["metadata"] = metadata
    write_result(run_dir / "comparison.json", compared)
    return {
        "run_dir": str(run_dir),
        "metadata": metadata,
        "A": a,
        "B": b,
        "comparison": compared,
    }


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("command", choices=["pair", "suite", "preflight"])
    result.add_argument("--task", choices=["short", "long"], default="short")
    result.add_argument("--order", choices=["AB", "BA"], default="AB")
    result.add_argument("--output-root", type=Path, default=RESULTS_ROOT)
    result.add_argument("--short-source", type=Path, default=SHORT_SOURCE)
    result.add_argument("--timeout", type=int, default=1200)
    result.add_argument("--share-timeout", type=int, default=60)
    result.add_argument("--webcodex-bin", type=Path, default=WEBCODEX_BIN)
    result.add_argument("--webcodex-tunnel", choices=["openai", "cloudflare", "none"], default="openai")
    result.add_argument(
        "--webcodex-auth",
        choices=["bearer", "query-token", "oauth"],
        default="bearer",
        help="Authentication mode passed to `webcodex share`; browser-driven custom connectors normally use query-token.",
    )
    result.add_argument(
        "--webcodex-driver-command",
        default=os.environ.get("WEBCODEX_BENCH_DRIVER", ""),
        help="External Terminal command that submits BENCH_PROMPT_FILE to ChatGPT Web with the live WebCodex connector and exits only after the final response.",
    )
    result.add_argument("--model-label", default="same-as-B")
    result.add_argument("--effort-label", default="extra-high")
    result.add_argument("--codex-bin", type=Path, default=CODEX_BIN)
    result.add_argument("--cgw-bun", type=Path, default=CGW_BUN)
    result.add_argument("--cgw-cli", type=Path, default=CGW_CLI)
    result.add_argument("--cgw-model", default="chatgpt-web/extra-high")
    result.add_argument("--allow-unready-cgw", action="store_true")
    return result


def preflight(args: argparse.Namespace) -> dict[str, Any]:
    doctor, doctor_stderr = cgw_doctor(args.cgw_bun, args.cgw_cli)
    return {
        "webcodex_bin": {"path": str(args.webcodex_bin), "exists": args.webcodex_bin.is_file()},
        "webcodex_driver": {
            "configured": bool(args.webcodex_driver_command),
            "command": args.webcodex_driver_command or None,
        },
        "webcodex_share": {
            "tunnel": args.webcodex_tunnel,
            "auth": args.webcodex_auth,
        },
        "codex_bin": {"path": str(args.codex_bin), "exists": args.codex_bin.is_file()},
        "cgw_doctor": doctor,
        "cgw_doctor_stderr": doctor_stderr or None,
        "short_source": {"path": str(args.short_source), "exists": args.short_source.is_dir()},
        "long_source": {"path": str(LONG_SOURCE), "exists": LONG_SOURCE.is_dir()},
    }


def main() -> int:
    args = parser().parse_args()
    if args.command == "preflight":
        print(json.dumps(preflight(args), indent=2, ensure_ascii=False))
        return 0
    if not args.webcodex_driver_command:
        raise SystemExit(
            "WebCodex E2E requires --webcodex-driver-command (or WEBCODEX_BENCH_DRIVER). "
            "The harness intentionally refuses direct connector replay."
        )
    if args.command == "pair":
        payload = run_pair(args, args.task)
    else:
        first = run_pair(args, "short")
        args.order = "BA" if args.order == "AB" else "AB"
        second = run_pair(args, "long")
        payload = {"short": first, "long": second}
    print(json.dumps(payload, indent=2, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
