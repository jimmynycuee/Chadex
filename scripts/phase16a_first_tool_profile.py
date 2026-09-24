#!/usr/bin/env python3
"""Phase 16A exact-ID first-tool profiler.

Select the target ChatGPT window only from structured raw/effective argument values
that exactly equal an expected Project id/path. Then join all request boundaries by
client_window_key and server_trace_id. Time is never used to guess request identity.
"""
from __future__ import annotations

import argparse
from collections import defaultdict
from datetime import datetime
import json
from pathlib import Path
import shutil
import subprocess
from typing import Any, Iterable


def iso_ns(value: str | None) -> int | None:
    if not value:
        return None
    try:
        dt = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return int(dt.timestamp() * 1_000_000_000)


def load_jsonl(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    if not path.is_file():
        return rows
    for line_no, line in enumerate(path.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError as exc:
            raise ValueError(f"invalid JSONL {path}:{line_no}: {exc}") from exc
        if isinstance(value, dict):
            rows.append(value)
    return rows


def scalar_strings(value: Any) -> Iterable[str]:
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for child in value.values():
            yield from scalar_strings(child)
    elif isinstance(value, list):
        for child in value:
            yield from scalar_strings(child)


def read_payload(trace_dir: Path, relative: str) -> Any | None:
    path = trace_dir / relative
    if not path.is_file():
        return None
    if path.suffix == ".zst":
        zstd = shutil.which("zstd")
        if not zstd:
            return None
        try:
            data = subprocess.check_output(
                [zstd, "-dc", str(path)], stderr=subprocess.DEVNULL, timeout=5
            )
        except (subprocess.SubprocessError, OSError):
            return None
        try:
            return json.loads(data)
        except json.JSONDecodeError:
            return None
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None


def exact_marker_hit(
    trace_dir: Path,
    events: list[dict[str, Any]],
    markers: set[str],
    *,
    expected_tool_name: str | None = None,
    marker_argument_key: str | None = None,
) -> bool:
    if expected_tool_name is not None:
        parsed_tools = {
            event.get("tool_name")
            for event in events
            if event.get("event") == "mcp_tool_request_parsed"
        }
        if expected_tool_name not in parsed_tools:
            return False
    for event in events:
        if event.get("event") != "tool_trace_payload_captured":
            continue
        if event.get("phase") not in {"raw_arguments", "effective_arguments"}:
            continue
        relative = event.get("payload_path")
        if not isinstance(relative, str):
            continue
        payload = read_payload(trace_dir, relative)
        if marker_argument_key is not None:
            if isinstance(payload, dict) and payload.get(marker_argument_key) in markers:
                return True
            continue
        if payload is not None and markers.intersection(scalar_strings(payload)):
            return True
    return False


def outer_prompt_ns(driver: dict[str, Any]) -> int:
    value = driver.get("submitted_at_ms")
    if not isinstance(value, int):
        raise ValueError("driver JSON lacks integer submitted_at_ms")
    return value * 1_000_000


def event_ns(event: dict[str, Any]) -> int | None:
    for key in ("wall_ns", "wall_unix_ns"):
        wall = event.get(key)
        if isinstance(wall, int):
            return wall
    return iso_ns(event.get("timestamp") if isinstance(event.get("timestamp"), str) else None)


def delta_ms(start_ns: int | None, end_ns: int | None) -> float | None:
    if start_ns is None or end_ns is None or end_ns < start_ns:
        return None
    return round((end_ns - start_ns) / 1_000_000, 3)


def first_named(events: list[dict[str, Any]], name: str) -> dict[str, Any] | None:
    rows = [row for row in events if row.get("event") == name and event_ns(row) is not None]
    return min(rows, key=lambda row: event_ns(row) or 0) if rows else None


def helper_timing(
    received: dict[str, Any] | None,
    helper: dict[str, Any] | None,
) -> tuple[dict[str, Any] | None, str]:
    if received is None:
        return None, "unavailable"
    received_ns = event_ns(received)
    if received_ns is None:
        return None, "unavailable"

    started_ns = received.get("helper_ingress_started_unix_ns")
    pre_backend_us = received.get("helper_ingress_pre_backend_us")
    source = "server_projected_exact_trace"
    if not isinstance(started_ns, int) or not isinstance(pre_backend_us, int):
        if helper is None:
            return None, "unavailable"
        started_ms = helper.get("started_at_ms")
        pre_backend_us = helper.get("ingress_pre_backend_us")
        if not isinstance(started_ms, int) or not isinstance(pre_backend_us, int):
            return None, "unavailable"
        started_ns = started_ms * 1_000_000
        source = "exact_helper_export"

    ingress_to_server_ms = delta_ms(started_ns, received_ns)
    if ingress_to_server_ms is None:
        return None, "unavailable"
    pre_backend_ms = round(pre_backend_us / 1_000.0, 3)
    backend_to_server_ms = (
        round(ingress_to_server_ms - pre_backend_ms, 3)
        if ingress_to_server_ms >= pre_backend_ms
        else None
    )
    return {
        "source": source,
        "ingress_started_unix_ns": started_ns,
        "ingress_pre_backend_ms": pre_backend_ms,
        "ingress_to_server_received_ms": ingress_to_server_ms,
        "backend_send_to_server_received_ms": backend_to_server_ms,
    }, source


def helper_index(path: Path | None) -> dict[str, dict[str, Any]]:
    if path is None:
        return {}
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, list):
        raise ValueError("helper trace export must be a JSON array")
    result: dict[str, dict[str, Any]] = {}
    for row in value:
        if not isinstance(row, dict):
            continue
        trace_id = row.get("server_trace_id")
        if isinstance(trace_id, str):
            if trace_id in result:
                raise ValueError(f"duplicate helper server_trace_id: {trace_id}")
            result[trace_id] = row
    return result


def profile(
    driver_path: Path,
    trace_root: Path,
    markers: list[str],
    helper_path: Path | None,
    cutoff_ms: int | None = None,
    *,
    expected_tool_name: str | None = None,
    marker_argument_key: str | None = None,
) -> dict[str, Any]:
    driver = json.loads(driver_path.read_text(encoding="utf-8"))
    prompt_ns = outer_prompt_ns(driver)
    end_ms = cutoff_ms if cutoff_ms is not None else driver.get("confirmed_final_at_ms", driver.get("visible_final_at_ms"))
    end_ns = end_ms * 1_000_000 if isinstance(end_ms, int) else None
    marker_set = {marker for marker in markers if marker}
    if not marker_set:
        raise ValueError("at least one non-empty exact marker is required")

    traces: dict[str, list[dict[str, Any]]] = {}
    marker_hit: dict[str, bool] = {}
    windows: dict[str, list[str]] = defaultdict(list)
    for trace_dir in trace_root.iterdir():
        if not trace_dir.is_dir():
            continue
        events = load_jsonl(trace_dir / "events.jsonl")
        if not events:
            continue
        parsed = [
            row for row in events
            if row.get("event") == "mcp_tool_request_parsed"
            and event_ns(row) is not None
            and event_ns(row) >= prompt_ns
            and (end_ns is None or event_ns(row) <= end_ns)
        ]
        if not parsed:
            continue
        ids = {row.get("server_trace_id") for row in parsed if isinstance(row.get("server_trace_id"), str)}
        if len(ids) != 1:
            raise ValueError(f"trace directory {trace_dir} has ambiguous server_trace_id")
        trace_id = next(iter(ids))
        traces[trace_id] = events
        hit = exact_marker_hit(
            trace_dir,
            events,
            marker_set,
            expected_tool_name=expected_tool_name,
            marker_argument_key=marker_argument_key,
        )
        marker_hit[trace_id] = hit
        for row in parsed:
            key = row.get("client_window_key")
            if isinstance(key, str) and key and trace_id not in windows[key]:
                windows[key].append(trace_id)

    candidates = []
    for key, trace_ids in windows.items():
        hits = [trace_id for trace_id in trace_ids if marker_hit.get(trace_id)]
        if hits:
            candidates.append((key, trace_ids, hits))
    if len(candidates) != 1:
        summary = {key: len(hits) for key, _, hits in candidates}
        raise ValueError(f"expected exactly one exact-marker client window, got {summary}")
    window_key, trace_ids, hit_ids = candidates[0]

    parsed_rows: list[tuple[int, str, dict[str, Any]]] = []
    for trace_id in trace_ids:
        for row in traces[trace_id]:
            if row.get("event") != "mcp_tool_request_parsed" or row.get("client_window_key") != window_key:
                continue
            stamp = event_ns(row)
            if stamp is not None:
                parsed_rows.append((stamp, trace_id, row))
    if not parsed_rows:
        raise ValueError("target window has no timestamped parsed request")
    parsed_ns, first_trace_id, parsed = min(parsed_rows, key=lambda item: item[0])
    first_events = traces[first_trace_id]

    received = first_named(first_events, "mcp_tool_request_received")
    dispatch_start = first_named(first_events, "mcp_tool_dispatch_started")
    dispatch_end = first_named(first_events, "mcp_tool_dispatch_finished")
    serialized = first_named(first_events, "mcp_tool_response_serialized")
    returned = first_named(first_events, "mcp_tool_handler_returned")
    received_ns = event_ns(received) if received else None
    dispatch_start_ns = event_ns(dispatch_start) if dispatch_start else None
    dispatch_end_ns = event_ns(dispatch_end) if dispatch_end else None
    returned_ns = event_ns(returned) if returned else None

    runner_ids = []
    for row in first_events:
        if row.get("event") == "tool_runner_request_enqueued" and isinstance(row.get("runner_request_id"), str):
            runner_ids.append(row["runner_request_id"])

    helpers = helper_index(helper_path)
    helper = helpers.get(first_trace_id)
    helper_timing_row, helper_status = helper_timing(received, helper)
    helper_started_ns = (
        helper_timing_row.get("ingress_started_unix_ns")
        if helper_timing_row is not None
        else None
    )

    first_tool = parsed.get("tool_name")
    return {
        "schema_version": 1,
        "classification_rule": (
            "target window selected only by exact structured argument marker"
            + (f" in {marker_argument_key!r}" if marker_argument_key is not None else "")
            + (f" for tool {expected_tool_name!r}" if expected_tool_name is not None else "")
            + "; request boundaries joined by exact client_window_key/server_trace_id; "
            "unobserved pre-Server time remains unattributed"
        ),
        "driver": {
            "submitted_at_ms": driver.get("submitted_at_ms"),
            "accepted_at_ms": driver.get("accepted_at_ms"),
            "confirmed_final_at_ms": driver.get("confirmed_final_at_ms"),
            "connector": driver.get("connector"),
            "effort": driver.get("effort"),
        },
        "selection": {
            "exact_markers": sorted(marker_set),
            "expected_tool_name": expected_tool_name,
            "marker_argument_key": marker_argument_key,
            "client_window_key": window_key,
            "window_trace_count": len(trace_ids),
            "exact_marker_trace_count": len(hit_ids),
            "exact_marker_trace_ids": sorted(hit_ids),
        },
        "first_tool": {
            "tool_name": first_tool,
            "server_trace_id": first_trace_id,
            "runner_request_ids": runner_ids,
            "prompt_to_server_received_ms": delta_ms(prompt_ns, received_ns),
            "prompt_to_server_parsed_ms": delta_ms(prompt_ns, parsed_ns),
            "server_receive_to_parse_ms": delta_ms(received_ns, parsed_ns),
            "server_parse_to_dispatch_ms": delta_ms(parsed_ns, dispatch_start_ns),
            "server_dispatch_ms": delta_ms(dispatch_start_ns, dispatch_end_ns),
            "server_total_to_handler_return_ms": delta_ms(received_ns, returned_ns),
            "response_serialized_at_ns": event_ns(serialized) if serialized else None,
            "helper_ingress": helper,
            "helper_timing": helper_timing_row,
            "helper_join_status": helper_status,
        },
        "unattributed": {
            "prompt_to_helper_ingress_ms": delta_ms(prompt_ns, helper_started_ns),
            "prompt_to_server_received_ms": delta_ms(prompt_ns, received_ns),
            "label": "prompt-to-helper remains host/relay/model-side unattributed when helper timing is present; never label this span as model reasoning time",
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--driver-json", type=Path, required=True)
    parser.add_argument("--runtime-trace-root", type=Path, required=True)
    parser.add_argument("--exact-marker", action="append", required=True)
    parser.add_argument("--helper-traces", type=Path)
    parser.add_argument("--cutoff-ms", type=int, help="Optional analysis cutoff for traces appended after the profiling window.")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    result = profile(args.driver_json, args.runtime_trace_root, args.exact_marker, args.helper_traces, args.cutoff_ms)
    encoded = json.dumps(result, indent=2, ensure_ascii=False, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded, encoding="utf-8")
    else:
        print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
