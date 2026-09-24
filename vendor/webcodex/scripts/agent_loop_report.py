#!/usr/bin/env python3
"""Offline Agent Loop profiler for existing WebCodex observability evidence.

The profiler consumes payload-safe metadata only. ActionAudit is the preferred
source for canonical outer-call, Window, and ModelErgonomics facts. Per-trace
``events.jsonl`` is an optional supplement for observed Runner request metadata;
payload blobs are never opened.
"""

from __future__ import annotations

import argparse
import json
import math
import sqlite3
import sys
from collections import Counter
from pathlib import Path
from typing import Any, Iterable

SCHEMA_VERSION = 1
DEFAULT_CASE_MANIFEST = Path(__file__).with_name("agent_loop_cases.json")
CODE_MODE_TOOLS = frozenset(("code_mode_exec", "code_mode_exec_effectful", "code_mode_exec_mutating"))
CODE_MODE_SURFACES = frozenset(("e1", "e2a", "e2b"))
CODE_MODE_COMPOSITION_NUMERIC_FIELDS = (
    "nested_calls",
    "nested_successes",
    "nested_failures",
    "max_in_flight",
    "duration_ms",
    "slot_wait_ms",
    "returned_bytes",
    "nested_raw_result_bytes_total",
    "consequential_calls",
    "known_results",
    "job_handoffs",
    "outcome_unknown",
)


class ReportError(ValueError):
    """Deterministic user-facing report input error."""


def _stable_json(value: Any, *, pretty: bool = False) -> str:
    if pretty:
        return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def _write_json(value: Any, output: Path | None) -> None:
    text = _stable_json(value, pretty=True)
    if output is None:
        sys.stdout.write(text)
        return
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(text, encoding="utf-8")


def _require_nonempty_string(value: Any, field: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ReportError(f"{field} must be a non-empty string")
    return value


def validate_case_manifest(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ReportError("case manifest must be a JSON object")
    if value.get("schema_version") != 1:
        raise ReportError("unsupported case manifest schema_version")
    cases = value.get("cases")
    if not isinstance(cases, list) or not cases:
        raise ReportError("case manifest cases must be a non-empty array")
    seen: set[str] = set()
    for index, case in enumerate(cases):
        if not isinstance(case, dict):
            raise ReportError(f"cases[{index}] must be an object")
        case_id = _require_nonempty_string(case.get("id"), f"cases[{index}].id")
        if case_id in seen:
            raise ReportError(f"duplicate case id: {case_id}")
        seen.add(case_id)
        for field in ("title", "target", "prompt"):
            _require_nonempty_string(case.get(field), f"cases[{index}].{field}")
        for field in ("correctness", "validation"):
            if not isinstance(case.get(field), dict) or not case[field]:
                raise ReportError(f"cases[{index}].{field} must be a non-empty object")
    return value


def load_case_manifest(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise ReportError(f"could not read case manifest: {path}") from exc
    except json.JSONDecodeError as exc:
        raise ReportError(f"case manifest is not valid JSON: {path}:{exc.lineno}") from exc
    return validate_case_manifest(value)


def _case_by_id(manifest: dict[str, Any], case_id: str) -> dict[str, Any]:
    for case in manifest["cases"]:
        if case["id"] == case_id:
            return case
    raise ReportError(f"unknown case id: {case_id}")


def load_trace_events(trace_root: Path) -> tuple[set[str], list[dict[str, Any]], int]:
    if not trace_root.is_dir():
        raise ReportError(f"trace root is not a directory: {trace_root}")
    paths = sorted(trace_root.rglob("events.jsonl"))
    trace_ids: set[str] = set()
    events: list[dict[str, Any]] = []
    for path in paths:
        seen: set[str] = set()
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except (OSError, UnicodeDecodeError) as exc:
            raise ReportError(f"could not read trace metadata: {path}") from exc
        for line_number, raw in enumerate(lines, start=1):
            if not raw.strip():
                continue
            try:
                event = json.loads(raw)
            except json.JSONDecodeError as exc:
                raise ReportError(f"malformed JSONL in {path}:{line_number}: {exc.msg}") from exc
            if not isinstance(event, dict):
                raise ReportError(f"trace event must be an object: {path}:{line_number}")
            fingerprint = _stable_json(event)
            if fingerprint in seen:
                continue
            seen.add(fingerprint)
            trace_id = event.get("server_trace_id")
            if isinstance(trace_id, str) and trace_id:
                trace_ids.add(trace_id)
            events.append(event)
    return trace_ids, events, len(paths)


_AUDIT_COLUMNS = """
e.event_id, e.operation, e.action_name, e.project, e.status,
e.ids_json, e.summary_json, e.client_window_key, e.server_trace_id,
e.principal_correlation_kind, e.principal_correlation_id,
e.window_started_at_ms, e.request_observed_at_ms, e.response_handed_at_ms,
e.window_transition_kind, e.response_streaming,
e.window_continuity_eligible, e.window_meaningful, e.started_at
""".strip()

_CONTINUITY_COLUMNS = """
e.event_id, e.action_name, e.client_window_key,
e.principal_correlation_kind, e.principal_correlation_id,
e.window_started_at_ms, e.request_observed_at_ms, e.response_handed_at_ms,
e.window_transition_kind, e.window_continuity_eligible,
e.window_meaningful, e.started_at
""".strip()


def _open_sqlite_readonly(path: Path) -> sqlite3.Connection:
    if not path.is_file():
        raise ReportError(f"audit DB is not a file: {path}")
    try:
        connection = sqlite3.connect(f"{path.resolve().as_uri()}?mode=ro", uri=True)
    except sqlite3.Error as exc:
        raise ReportError(f"could not open audit DB read-only: {path}") from exc
    connection.row_factory = sqlite3.Row
    return connection


def _parse_json_object(raw: Any, field: str, event_id: str) -> dict[str, Any]:
    if not isinstance(raw, str):
        raise ReportError(f"{field} is not text for action event {event_id}")
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise ReportError(f"{field} is invalid JSON for action event {event_id}") from exc
    if not isinstance(value, dict):
        raise ReportError(f"{field} is not an object for action event {event_id}")
    return value


def _row_to_audit_event(row: sqlite3.Row) -> dict[str, Any]:
    event_id = str(row["event_id"])
    return {
        "event_id": event_id,
        "operation": row["operation"],
        "action_name": row["action_name"],
        "project": row["project"],
        "status": row["status"],
        "ids": _parse_json_object(row["ids_json"], "ids_json", event_id),
        "summary": _parse_json_object(row["summary_json"], "summary_json", event_id),
        "client_window_key": row["client_window_key"],
        "server_trace_id": row["server_trace_id"],
        "principal_correlation_kind": row["principal_correlation_kind"],
        "principal_correlation_id": row["principal_correlation_id"],
        "window_started_at_ms": row["window_started_at_ms"],
        "request_observed_at_ms": row["request_observed_at_ms"],
        "response_handed_at_ms": row["response_handed_at_ms"],
        "window_transition_kind": row["window_transition_kind"],
        "response_streaming": None if row["response_streaming"] is None else bool(row["response_streaming"]),
        "window_continuity_eligible": None if row["window_continuity_eligible"] is None else bool(row["window_continuity_eligible"]),
        "window_meaningful": bool(row["window_meaningful"]),
        "started_at": row["started_at"],
    }


def _row_to_continuity_event(row: sqlite3.Row) -> dict[str, Any]:
    return {
        "event_id": str(row["event_id"]),
        "action_name": row["action_name"],
        "client_window_key": row["client_window_key"],
        "principal_correlation_kind": row["principal_correlation_kind"],
        "principal_correlation_id": row["principal_correlation_id"],
        "window_started_at_ms": row["window_started_at_ms"],
        "request_observed_at_ms": row["request_observed_at_ms"],
        "response_handed_at_ms": row["response_handed_at_ms"],
        "window_transition_kind": row["window_transition_kind"],
        "window_continuity_eligible": None if row["window_continuity_eligible"] is None else bool(row["window_continuity_eligible"]),
        "window_meaningful": bool(row["window_meaningful"]),
        "started_at": row["started_at"],
    }


def load_audit_events(audit_db: Path, *, workflow_session_id: str | None, trace_ids: set[str]) -> list[dict[str, Any]]:
    connection = _open_sqlite_readonly(audit_db)
    try:
        tables = {row[0] for row in connection.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()}
        if "action_events" not in tables:
            raise ReportError("audit DB does not contain action_events")
        if workflow_session_id is not None:
            if "action_event_workflow_links" not in tables:
                raise ReportError("audit DB does not contain action_event_workflow_links")
            sql = f"""
                SELECT DISTINCT {_AUDIT_COLUMNS}
                FROM action_events e
                JOIN action_event_workflow_links l ON l.event_id = e.event_id
                WHERE l.workflow_session_id = ?
                ORDER BY COALESCE(e.request_observed_at_ms, e.window_started_at_ms, e.started_at * 1000), e.event_id
            """
            rows = connection.execute(sql, (workflow_session_id,)).fetchall()
        elif trace_ids:
            rows = []
            ordered_ids = sorted(trace_ids)
            for start in range(0, len(ordered_ids), 400):
                chunk = ordered_ids[start:start + 400]
                placeholders = ",".join("?" for _ in chunk)
                sql = f"""
                    SELECT {_AUDIT_COLUMNS}
                    FROM action_events e
                    WHERE e.server_trace_id IN ({placeholders})
                    ORDER BY COALESCE(e.request_observed_at_ms, e.window_started_at_ms, e.started_at * 1000), e.event_id
                """
                rows.extend(connection.execute(sql, chunk).fetchall())
            rows.sort(key=lambda row: (
                row["request_observed_at_ms"] if row["request_observed_at_ms"] is not None
                else row["window_started_at_ms"] if row["window_started_at_ms"] is not None
                else int(row["started_at"]) * 1000,
                row["event_id"],
            ))
        else:
            raise ReportError("audit DB selection requires --workflow-session-id or trace ids from --trace-root")
        return [_row_to_audit_event(row) for row in rows]
    except sqlite3.Error as exc:
        raise ReportError(f"could not query audit DB: {exc}") from exc
    finally:
        connection.close()


def load_audit_continuity_events(
    audit_db: Path, selected_events: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    selected_meaningful = [
        event
        for event in selected_events
        if event.get("action_name") == "toolsCall" and event.get("window_meaningful")
    ]
    keyed = [
        event
        for event in selected_meaningful
        if all(
            isinstance(event.get(field), str) and event.get(field)
            for field in (
                "client_window_key",
                "principal_correlation_kind",
                "principal_correlation_id",
            )
        )
        and isinstance(event.get("request_observed_at_ms"), int)
    ]
    if not keyed:
        return selected_meaningful

    keys = {
        (
            event["client_window_key"],
            event["principal_correlation_kind"],
            event["principal_correlation_id"],
        )
        for event in keyed
    }
    windows = sorted({key[0] for key in keys})
    first_started = min(event["request_observed_at_ms"] for event in keyed)
    last_started = max(event["request_observed_at_ms"] for event in keyed)
    connection = _open_sqlite_readonly(audit_db)
    rows: list[sqlite3.Row] = []
    try:
        for start in range(0, len(windows), 400):
            chunk = windows[start:start + 400]
            placeholders = ",".join("?" for _ in chunk)
            sql = f"""
                SELECT {_CONTINUITY_COLUMNS}
                FROM action_events e
                WHERE e.action_name = 'toolsCall'
                  AND e.window_meaningful = 1
                  AND e.request_observed_at_ms BETWEEN ? AND ?
                  AND e.client_window_key IN ({placeholders})
                ORDER BY COALESCE(e.request_observed_at_ms, e.window_started_at_ms, e.started_at * 1000), e.event_id
            """
            rows.extend(
                connection.execute(sql, [first_started, last_started, *chunk]).fetchall()
            )
    except sqlite3.Error as exc:
        raise ReportError(f"could not query audit continuity context: {exc}") from exc
    finally:
        connection.close()

    by_id = {event["event_id"]: event for event in selected_meaningful}
    for row in rows:
        event = _row_to_continuity_event(row)
        key = (
            event.get("client_window_key"),
            event.get("principal_correlation_kind"),
            event.get("principal_correlation_id"),
        )
        if key in keys:
            by_id[event["event_id"]] = event
    return sorted(by_id.values(), key=_audit_sort_key)


def _telemetry(event: dict[str, Any]) -> dict[str, Any] | None:
    value = event.get("summary", {}).get("model_ergonomics")
    return value if isinstance(value, dict) else None


def _code_mode_composition(event: dict[str, Any]) -> dict[str, Any] | None:
    value = event.get("summary", {}).get("code_mode_composition")
    if not isinstance(value, dict):
        return None
    normalized: dict[str, Any] = {}
    for field in CODE_MODE_COMPOSITION_NUMERIC_FIELDS:
        item = value.get(field)
        if isinstance(item, bool) or not isinstance(item, int) or item < 0:
            return None
        normalized[field] = item
    counts = value.get("nested_tool_counts")
    if not isinstance(counts, dict):
        return None
    normalized_counts: dict[str, int] = {}
    for tool, count in counts.items():
        if (
            not isinstance(tool, str)
            or not tool
            or isinstance(count, bool)
            or not isinstance(count, int)
            or count < 0
        ):
            return None
        normalized_counts[tool] = count
    if normalized["nested_successes"] + normalized["nested_failures"] != normalized["nested_calls"]:
        return None
    if sum(normalized_counts.values()) != normalized["nested_calls"]:
        return None
    if normalized["known_results"] + normalized["job_handoffs"] + normalized["outcome_unknown"] != normalized["consequential_calls"]:
        return None
    normalized["nested_tool_counts"] = normalized_counts
    return normalized


def _nearest_rank(values: Iterable[int], percentile: float) -> int | None:
    ordered = sorted(values)
    if not ordered:
        return None
    rank = max(1, math.ceil(percentile * len(ordered)))
    return ordered[rank - 1]


def _metric_distribution(values: list[int], *, missing: int = 0) -> dict[str, Any]:
    observed_total = sum(values)
    return {
        "total": observed_total if missing == 0 else None,
        "observed_total": observed_total,
        "p50": _nearest_rank(values, 0.50),
        "p95": _nearest_rank(values, 0.95),
        "samples": len(values),
        "missing": missing,
    }


def _summarize_code_mode_composition(
    outer: list[dict[str, Any]],
    variant: str | None,
    *,
    action_audit_available: bool = True,
) -> tuple[dict[str, Any], dict[str, Any]]:
    if not action_audit_available:
        metrics = {
            field: _metric_distribution([], missing=1)
            for field in CODE_MODE_COMPOSITION_NUMERIC_FIELDS
        }
        return (
            {
                "outer_code_mode_calls": None,
                "outer_calls_with_summary": None,
                "nested_tool_counts": {},
                **metrics,
            },
            {
                "available": False,
                "reason": "Code Mode composition is persisted in ActionAudit summary metadata, not in trace-only evidence",
            },
        )

    code_mode_outer = [event for event in outer if event.get("operation") in CODE_MODE_TOOLS]
    parsed = [
        composition
        for event in code_mode_outer
        if (composition := _code_mode_composition(event)) is not None
    ]
    missing = len(code_mode_outer) - len(parsed)
    reason = None
    if variant == "code_mode" and not code_mode_outer:
        missing = max(missing, 1)
        reason = (
            "no Code Mode outer ActionAudit row was selected; benchmark calls must link their outer "
            "ActionAudit rows with recording_session_id"
        )
    elif variant == "direct" and code_mode_outer:
        missing = max(missing, 1)
        reason = "a direct report selected one or more Code Mode outer calls"
    elif missing:
        reason = "one or more Code Mode outer calls lack a valid code_mode_composition summary"

    metrics = {
        field: _metric_distribution([int(value[field]) for value in parsed], missing=missing)
        for field in CODE_MODE_COMPOSITION_NUMERIC_FIELDS
    }
    nested_tool_counts: Counter[str] = Counter()
    for value in parsed:
        nested_tool_counts.update(value["nested_tool_counts"])
    available = missing == 0 and reason is None
    return (
        {
            "outer_code_mode_calls": len(code_mode_outer),
            "outer_calls_with_summary": len(parsed),
            "nested_tool_counts": dict(sorted(nested_tool_counts.items())),
            **metrics,
        },
        {"available": available, "reason": reason},
    )


def _audit_sort_key(event: dict[str, Any]) -> tuple[int, str]:
    for field in ("request_observed_at_ms", "window_started_at_ms"):
        value = event.get(field)
        if isinstance(value, int):
            return value, str(event["event_id"])
    started_at = event.get("started_at")
    return (int(started_at) * 1000 if isinstance(started_at, int) else 0, str(event["event_id"]))


def _window_timing(
    outer: list[dict[str, Any]],
    continuity_events: list[dict[str, Any]] | None = None,
) -> tuple[dict[str, Any], int, int]:
    selected_ids = {str(event["event_id"]) for event in outer}
    previous: dict[tuple[str, str, str], dict[str, Any]] = {}
    gaps: list[int] = []
    missing_serial = 0
    overlap_count = 0
    source = continuity_events if continuity_events is not None else outer
    for event in sorted(source, key=_audit_sort_key):
        if not event.get("window_meaningful"):
            continue
        selected = str(event["event_id"]) in selected_ids
        window_key = event.get("client_window_key")
        principal_kind = event.get("principal_correlation_kind")
        principal_id = event.get("principal_correlation_id")
        if not all(isinstance(value, str) and value for value in (window_key, principal_kind, principal_id)):
            if selected and event.get("window_transition_kind") == "serial":
                missing_serial += 1
            continue
        key = (window_key, principal_kind, principal_id)
        predecessor = previous.pop(key, None)
        transition = event.get("window_transition_kind")
        if selected and transition == "overlap":
            overlap_count += 1
        if selected and transition == "serial":
            current_started = event.get("request_observed_at_ms")
            previous_handed = predecessor.get("response_handed_at_ms") if predecessor else None
            predecessor_selected = (
                predecessor is not None
                and str(predecessor["event_id"]) in selected_ids
            )
            if (
                predecessor_selected
                and isinstance(current_started, int)
                and isinstance(previous_handed, int)
                and current_started >= previous_handed
            ):
                gaps.append(current_started - previous_handed)
            else:
                missing_serial += 1
        if event.get("window_continuity_eligible") is True:
            previous[key] = event
    return _metric_distribution(gaps, missing=missing_serial), overlap_count, missing_serial


def _observed_span_ms(outer: list[dict[str, Any]]) -> int | None:
    starts = [event["request_observed_at_ms"] for event in outer if isinstance(event.get("request_observed_at_ms"), int)]
    ends = [event["response_handed_at_ms"] for event in outer if event.get("response_streaming") is False and isinstance(event.get("response_handed_at_ms"), int)]
    if not starts or not ends:
        return None
    span = max(ends) - min(starts)
    return span if span >= 0 else None


def _summarize_audit(
    audit_events: list[dict[str, Any]],
    variant: str | None,
    continuity_events: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    outer = [event for event in audit_events if event.get("action_name") == "toolsCall"]
    statuses = Counter(str(event.get("status") or "unknown") for event in outer)
    outer_tools = Counter(event["operation"] for event in outer if isinstance(event.get("operation"), str))
    composition, composition_availability = _summarize_code_mode_composition(outer, variant)

    service_values: list[int] = []
    for event in outer:
        started = event.get("request_observed_at_ms")
        handed = event.get("response_handed_at_ms")
        if event.get("response_streaming") is False and isinstance(started, int) and isinstance(handed, int) and handed >= started:
            service_values.append(handed - started)
    service = _metric_distribution(service_values, missing=len(outer) - len(service_values))

    telemetries = [(event, _telemetry(event)) for event in outer]
    present_telemetries = [(event, value) for event, value in telemetries if value is not None]
    runtime_durations = [int(value["duration_ms"]) for _, value in present_telemetries if isinstance(value.get("duration_ms"), int) and value["duration_ms"] >= 0]
    runtime_duration = _metric_distribution(runtime_durations, missing=len(outer) - len(runtime_durations))
    result_bytes = [int(value["serialized_result_bytes"]) for _, value in present_telemetries if isinstance(value.get("serialized_result_bytes"), int) and value["serialized_result_bytes"] >= 0]
    result_byte_metric = _metric_distribution(result_bytes, missing=len(outer) - len(result_bytes))

    failure_kinds: Counter[str] = Counter()
    recovery_guidance: Counter[str] = Counter()
    error_kinds: Counter[str] = Counter()
    for _, value in present_telemetries:
        for field, counter in (("failure_kind", failure_kinds), ("recovery_kind", recovery_guidance), ("error_kind", error_kinds)):
            item = value.get(field)
            if isinstance(item, str) and item:
                counter[item] += 1

    gaps, overlap_count, missing_serial = _window_timing(outer, continuity_events)
    canonical_observed = len(present_telemetries)
    canonical_by_name = Counter(value["tool_name"] for _, value in present_telemetries if isinstance(value.get("tool_name"), str) and value["tool_name"])
    if variant == "direct":
        canonical_total = canonical_observed if canonical_observed == len(outer) else None
        canonical_reason = None if canonical_total is not None else "one or more outer calls lack ModelErgonomics evidence"
    elif variant == "code_mode":
        canonical_total = None
        canonical_reason = "canonical_calls.total keeps the outer/direct counting contract; use composition.nested_calls for persisted Code Mode child-call totals"
    else:
        canonical_total = None
        canonical_reason = "declare --variant direct or code_mode before interpreting canonical call count"

    return {
        "observed_span_ms": _observed_span_ms(outer),
        "outer_calls": {
            "total": len(outer),
            "meaningful": sum(bool(event.get("window_meaningful")) for event in outer),
            "successful": statuses.get("success", 0),
            "failed": statuses.get("failed", 0),
            "timeout_or_unknown": len(outer) - statuses.get("success", 0) - statuses.get("failed", 0),
            "by_status": dict(sorted(statuses.items())),
        },
        "tools": {"outer_by_name": dict(sorted(outer_tools.items()))},
        "canonical_calls": {
            "total": canonical_total,
            "observed_outer_runtime_records": canonical_observed,
            "by_name": dict(sorted(canonical_by_name.items())),
        },
        "composition": composition,
        "timing": {
            "webcodex_service_ms": service,
            "tool_runtime_ms": runtime_duration,
            "outside_webcodex_gap_ms": gaps,
            "overlap_count": overlap_count,
        },
        "results": {"serialized_tool_result_bytes": result_byte_metric},
        "failures": {
            "error_kind_by_name": dict(sorted(error_kinds.items())),
            "failure_kind_by_name": dict(sorted(failure_kinds.items())),
            "recovery_guidance_by_kind": dict(sorted(recovery_guidance.items())),
            "resolved_recoveries": None,
        },
        "availability": {
            "action_audit": {"available": True},
            "webcodex_service_timing": {"available": service["total"] is not None, "reason": None if service["total"] is not None else "one or more outer calls lack non-streaming request-observed/response-handoff timestamps"},
            "tool_runtime_timing": {"available": runtime_duration["total"] is not None, "reason": None if runtime_duration["total"] is not None else "one or more outer calls lack ModelErgonomics runtime duration evidence"},
            "window_timing": {"available": missing_serial == 0, "reason": None if missing_serial == 0 else "one or more canonical serial transitions lack the predecessor timestamps needed for a gap"},
            "canonical_calls": {"available": canonical_total is not None, "reason": canonical_reason},
            "code_mode_composition": composition_availability,
            "serialized_tool_result_bytes": {"available": result_byte_metric["total"] is not None, "reason": None if result_byte_metric["total"] is not None else "one or more outer calls lack a serialized ToolResult byte count; missing values are not treated as zero"},
            "resolved_recoveries": {"available": False, "reason": "recovery_kind is guidance metadata, not proof that a later call resolved the failure"},
        },
    }


def _trace_handler_events(trace_events: list[dict[str, Any]]) -> list[dict[str, Any]]:
    grouped: dict[str, list[dict[str, Any]]] = {}
    for event in trace_events:
        if event.get("event") != "tool_handler_returned":
            continue
        trace_id = event.get("server_trace_id")
        if isinstance(trace_id, str) and trace_id:
            grouped.setdefault(trace_id, []).append(event)
    handlers = []
    for trace_id in sorted(grouped):
        candidates = grouped[trace_id]
        candidates.sort(key=lambda event: (int(event.get("duration_ms") or -1), _stable_json(event)))
        handlers.append(candidates[-1])
    return handlers


def _summarize_trace_only(trace_events: list[dict[str, Any]], variant: str | None) -> dict[str, Any]:
    handlers = _trace_handler_events(trace_events)
    statuses: Counter[str] = Counter()
    tools: Counter[str] = Counter()
    for event in handlers:
        tool_success = event.get("tool_success")
        protocol_success = event.get("protocol_success")
        if tool_success is True or (tool_success is None and protocol_success is True):
            statuses["success"] += 1
        elif tool_success is False or protocol_success is False:
            statuses["failed"] += 1
        else:
            statuses["unknown"] += 1
        tool_name = event.get("tool_name")
        if isinstance(tool_name, str) and tool_name:
            tools[tool_name] += 1
    composition, composition_availability = _summarize_code_mode_composition(
        [], variant, action_audit_available=False
    )
    unavailable = {
        "webcodex_service_timing": {"available": False, "reason": "tool_handler_returned duration is not the canonical request-observed to response-handoff service interval"},
        "tool_runtime_timing": {"available": False, "reason": "events.jsonl does not persist canonical ModelErgonomics runtime duration evidence"},
        "window_timing": {"available": False, "reason": "events.jsonl does not persist canonical meaningful/continuity transition facts"},
        "canonical_calls": {"available": False, "reason": "outer trace lifecycle metadata is not a complete canonical nested-call ledger"},
        "serialized_tool_result_bytes": {"available": False, "reason": "estimated HTTP response bytes are not ToolResult serialized bytes"},
        "resolved_recoveries": {"available": False, "reason": "trace lifecycle metadata does not prove recovery completion"},
    }
    return {
        "observed_span_ms": None,
        "outer_calls": {"total": len(handlers), "meaningful": None, "successful": statuses.get("success", 0), "failed": statuses.get("failed", 0), "timeout_or_unknown": statuses.get("unknown", 0), "by_status": dict(sorted(statuses.items()))},
        "tools": {"outer_by_name": dict(sorted(tools.items()))},
        "canonical_calls": {"total": None, "observed_outer_runtime_records": None, "by_name": {}},
        "composition": composition,
        "timing": {
            "webcodex_service_ms": _metric_distribution([], missing=len(handlers)),
            "tool_runtime_ms": _metric_distribution([], missing=len(handlers)),
            "outside_webcodex_gap_ms": _metric_distribution([]),
            "overlap_count": None,
        },
        "results": {"serialized_tool_result_bytes": _metric_distribution([], missing=len(handlers))},
        "failures": {"error_kind_by_name": {}, "failure_kind_by_name": {}, "recovery_guidance_by_kind": {}, "resolved_recoveries": None},
        "availability": {
            "action_audit": {"available": False, "reason": "no ActionAudit DB evidence was provided"},
            "code_mode_composition": composition_availability,
            **unavailable,
        },
    }


def _runner_summary(trace_events: list[dict[str, Any]], trace_root_present: bool) -> tuple[dict[str, Any], dict[str, Any]]:
    if not trace_root_present:
        return ({"requests_observed": None, "by_kind": {}}, {"available": False, "complete": False, "reason": "no trace-root evidence was provided"})
    requests: dict[str, dict[str, Any]] = {}
    anonymous: set[str] = set()
    for event in trace_events:
        if event.get("event") != "tool_runner_request_enqueued":
            continue
        request_id = event.get("runner_request_id")
        if isinstance(request_id, str) and request_id:
            requests.setdefault(request_id, event)
        else:
            anonymous.add(_stable_json(event))
    kinds: Counter[str] = Counter()
    for event in [*requests.values(), *(json.loads(item) for item in anonymous)]:
        kind = event.get("runner_request_kind")
        if isinstance(kind, str) and kind:
            kinds[kind] += 1
    return (
        {"requests_observed": len(requests) + len(anonymous), "by_kind": dict(sorted(kinds.items()))},
        {"available": True, "complete": False, "reason": "events.jsonl proves observed Runner enqueue events but has no fail-open completeness marker"},
    )


def _job_summary(trace_events: list[dict[str, Any]], trace_root_present: bool) -> tuple[dict[str, Any], dict[str, Any]]:
    job_ids = {event.get("runner_job_id") for event in trace_events if event.get("event") == "tool_runner_request_enqueued" and isinstance(event.get("runner_job_id"), str) and event.get("runner_job_id")}
    return (
        {"runner_job_ids_observed": len(job_ids) if trace_root_present else None, "handoffs": None, "terminal": None},
        {"available": False, "reason": "runner_job_id may be allocated before synchronous completion and does not prove a same-execution model-visible handoff or terminal observation"},
    )


def _is_exact_git_revision(value: Any) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 40
        and all(char in "0123456789abcdefABCDEF" for char in value)
    )


def _benchmark_metadata(*, case_manifest: Path | None, case_id: str | None, variant: str | None, surface: str | None = None, base_revision: str | None = None) -> dict[str, Any] | None:
    if case_id is None:
        if case_manifest is not None or base_revision is not None or surface is not None:
            raise ReportError("--case-manifest/--base-revision/--surface require --case-id")
        return None
    if variant is None:
        raise ReportError("--case-id requires --variant")
    if variant == "direct":
        if surface is None:
            surface = "direct"
        elif surface != "direct":
            raise ReportError("--variant direct requires --surface direct")
    elif variant == "code_mode":
        if surface is None:
            raise ReportError("--variant code_mode benchmark runs require --surface e1, e2a, or e2b")
        if surface not in CODE_MODE_SURFACES:
            raise ReportError("--variant code_mode requires --surface e1, e2a, or e2b")
    if not _is_exact_git_revision(base_revision):
        raise ReportError("--case-id requires --base-revision as an exact 40-hex Git commit")
    manifest = load_case_manifest(case_manifest or DEFAULT_CASE_MANIFEST)
    case = _case_by_id(manifest, case_id)
    return {"manifest_schema_version": manifest["schema_version"], "case_id": case_id, "case_title": case["title"], "target": case["target"], "variant": variant, "surface": surface, "base_revision": base_revision}


def summarize(*, trace_root: Path | None, audit_db: Path | None, workflow_session_id: str | None, case_manifest: Path | None, case_id: str | None, variant: str | None, surface: str | None = None, base_revision: str | None = None) -> dict[str, Any]:
    if trace_root is None and audit_db is None:
        raise ReportError("summarize requires --trace-root and/or --audit-db")
    if workflow_session_id is not None and audit_db is None:
        raise ReportError("--workflow-session-id requires --audit-db for authoritative Session selection")
    if audit_db is not None and workflow_session_id is None and trace_root is None:
        raise ReportError("--audit-db without --trace-root requires --workflow-session-id")
    trace_ids: set[str] = set()
    trace_events: list[dict[str, Any]] = []
    trace_files = 0
    if trace_root is not None:
        trace_ids, trace_events, trace_files = load_trace_events(trace_root)
    audit_events: list[dict[str, Any]] = []
    continuity_events: list[dict[str, Any]] | None = None
    if audit_db is not None:
        audit_events = load_audit_events(audit_db, workflow_session_id=workflow_session_id, trace_ids=trace_ids)
        continuity_events = load_audit_continuity_events(audit_db, audit_events)
    if workflow_session_id is not None and trace_events:
        selected_trace_ids = {event["server_trace_id"] for event in audit_events if isinstance(event.get("server_trace_id"), str) and event["server_trace_id"]}
        trace_events = [event for event in trace_events if event.get("server_trace_id") in selected_trace_ids]
        trace_ids = {trace_id for trace_id in trace_ids if trace_id in selected_trace_ids}
    core = (
        _summarize_audit(audit_events, variant, continuity_events)
        if audit_db is not None
        else _summarize_trace_only(trace_events, variant)
    )
    trace_metadata_present = trace_files > 0
    runner, runner_availability = _runner_summary(trace_events, trace_metadata_present)
    jobs, jobs_availability = _job_summary(trace_events, trace_metadata_present)
    core["runner"] = runner
    core["jobs"] = jobs
    core["availability"]["runner_requests"] = runner_availability
    core["availability"]["job_handoffs"] = jobs_availability
    benchmark = _benchmark_metadata(case_manifest=case_manifest, case_id=case_id, variant=variant, surface=surface, base_revision=base_revision)
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": "agent_loop_summary",
        "benchmark": benchmark,
        "observed_span_ms": core.pop("observed_span_ms"),
        **core,
        "evidence": {"trace_files": trace_files, "trace_ids": len(trace_ids), "audit_events": len(audit_events), "workflow_session_selected": workflow_session_id is not None},
        "notes": [
            "observed_span_ms is the span between observed WebCodex outer-call timestamps, not task wall time",
            "outside_webcodex_gap_ms contains only canonical serial meaningful-Window gaps and is not model reasoning time",
            "Runner request counts are observed enqueue events, not an asserted complete total",
        ],
    }


def _get_path(value: dict[str, Any], path: str) -> Any:
    current: Any = value
    for part in path.split("."):
        if not isinstance(current, dict) or part not in current:
            return None
        current = current[part]
    return current


_COMPARISON_METRICS = [
    "observed_span_ms", "outer_calls.total", "outer_calls.meaningful",
    "outer_calls.successful", "outer_calls.failed", "canonical_calls.total",
    "runner.requests_observed", "composition.nested_calls.total",
    "composition.nested_successes.total", "composition.nested_failures.total",
    "composition.consequential_calls.total", "composition.known_results.total",
    "composition.job_handoffs.total",
    "composition.outcome_unknown.total", "composition.duration_ms.total",
    "composition.slot_wait_ms.total", "composition.returned_bytes.total",
    "composition.nested_raw_result_bytes_total.total", "timing.webcodex_service_ms.total",
    "timing.webcodex_service_ms.p50", "timing.webcodex_service_ms.p95",
    "timing.tool_runtime_ms.total", "timing.outside_webcodex_gap_ms.total",
    "timing.outside_webcodex_gap_ms.p50", "timing.outside_webcodex_gap_ms.p95",
    "timing.overlap_count", "results.serialized_tool_result_bytes.total",
    "jobs.handoffs", "jobs.terminal",
]


def _case_compatibility(baseline: dict[str, Any], candidate: dict[str, Any]) -> dict[str, Any]:
    left = baseline.get("benchmark")
    right = candidate.get("benchmark")
    if not isinstance(left, dict) or not isinstance(right, dict):
        return {"comparable": False, "reason": "both reports must carry benchmark case metadata"}
    if left.get("case_id") != right.get("case_id"):
        return {"comparable": False, "reason": "benchmark case ids differ"}
    left_base, right_base = left.get("base_revision"), right.get("base_revision")
    if not _is_exact_git_revision(left_base) or not _is_exact_git_revision(right_base):
        return {"comparable": False, "reason": "both reports must record an exact 40-hex Git base revision"}
    if left_base != right_base:
        return {"comparable": False, "reason": "benchmark base revisions differ"}
    return {"comparable": True, "reason": None}


def compare_reports(baseline: dict[str, Any], candidate: dict[str, Any]) -> dict[str, Any]:
    for label, report in (("baseline", baseline), ("candidate", candidate)):
        if report.get("schema_version") != SCHEMA_VERSION or report.get("kind") != "agent_loop_summary":
            raise ReportError(f"{label} is not an agent_loop_summary schema v{SCHEMA_VERSION}")
    metrics = []
    for path in _COMPARISON_METRICS:
        left, right = _get_path(baseline, path), _get_path(candidate, path)
        if left is None or right is None:
            metrics.append({"metric": path, "baseline": left, "candidate": right, "delta": None, "comparable": False, "reason": "metric is unavailable in one or both reports"})
        elif isinstance(left, bool) or isinstance(right, bool) or not isinstance(left, (int, float)) or not isinstance(right, (int, float)):
            metrics.append({"metric": path, "baseline": left, "candidate": right, "delta": None, "comparable": False, "reason": "metric is not numeric"})
        else:
            metrics.append({"metric": path, "baseline": left, "candidate": right, "delta": right - left, "comparable": True, "reason": None})
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": "agent_loop_comparison",
        "case_compatibility": _case_compatibility(baseline, candidate),
        "baseline": baseline.get("benchmark"),
        "candidate": candidate.get("benchmark"),
        "metrics": metrics,
        "tools": {
            "baseline_outer_by_name": _get_path(baseline, "tools.outer_by_name") or {},
            "candidate_outer_by_name": _get_path(candidate, "tools.outer_by_name") or {},
            "baseline_canonical_by_name": _get_path(baseline, "canonical_calls.by_name") or {},
            "candidate_canonical_by_name": _get_path(candidate, "canonical_calls.by_name") or {},
            "baseline_nested_by_name": _get_path(baseline, "composition.nested_tool_counts") or {},
            "candidate_nested_by_name": _get_path(candidate, "composition.nested_tool_counts") or {},
        },
        "notes": ["deltas are candidate minus baseline and are descriptive only", "no metric ordering, score, or winner is inferred"],
    }


def _read_report(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise ReportError(f"could not read report: {path}") from exc
    except json.JSONDecodeError as exc:
        raise ReportError(f"report is not valid JSON: {path}:{exc.lineno}") from exc
    if not isinstance(value, dict):
        raise ReportError(f"report must be a JSON object: {path}")
    return value


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    summarize_parser = subparsers.add_parser("summarize", help="summarize one observed agent run")
    summarize_parser.add_argument("--trace-root", type=Path)
    summarize_parser.add_argument("--audit-db", type=Path)
    summarize_parser.add_argument("--workflow-session-id")
    summarize_parser.add_argument("--case-manifest", type=Path)
    summarize_parser.add_argument("--case-id")
    summarize_parser.add_argument("--variant", choices=("direct", "code_mode"))
    summarize_parser.add_argument("--surface", choices=("direct", "e1", "e2a", "e2b"))
    summarize_parser.add_argument("--base-revision")
    summarize_parser.add_argument("--output", type=Path)
    compare_parser = subparsers.add_parser("compare", help="compare two summary JSON reports")
    compare_parser.add_argument("--baseline", type=Path, required=True)
    compare_parser.add_argument("--candidate", type=Path, required=True)
    compare_parser.add_argument("--output", type=Path)
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)
    try:
        if args.command == "summarize":
            report = summarize(trace_root=args.trace_root, audit_db=args.audit_db, workflow_session_id=args.workflow_session_id, case_manifest=args.case_manifest, case_id=args.case_id, variant=args.variant, surface=args.surface, base_revision=args.base_revision)
            _write_json(report, args.output)
            return 0
        comparison = compare_reports(_read_report(args.baseline), _read_report(args.candidate))
        _write_json(comparison, args.output)
        return 0
    except ReportError as exc:
        print(f"agent_loop_report: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
