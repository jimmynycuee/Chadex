#!/usr/bin/env python3
"""Phase 19A long-haul trace aggregator.

Joins browser-driver timestamps, existing tool-request trace metadata, and
optional runtime_status heartbeat snapshots. Semantic trace payload bodies are
never read.
"""
from __future__ import annotations

import argparse
from datetime import datetime
import json
import math
from pathlib import Path
from typing import Any

import phase16a_first_tool_profile as phase16a


def load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return value


def load_jsonl(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    if not path.is_file():
        return rows
    for number, line in enumerate(
        path.read_text(encoding="utf-8", errors="replace").splitlines(), 1
    ):
        if not line.strip():
            continue
        value = json.loads(line)
        if not isinstance(value, dict):
            raise ValueError(f"{path}:{number} must contain a JSON object")
        rows.append(value)
    return rows


def event_ns(row: dict[str, Any]) -> int | None:
    for key in ("wall_unix_ns", "wall_ns"):
        value = row.get(key)
        if isinstance(value, int):
            return value
    value = row.get("timestamp")
    if isinstance(value, str):
        try:
            return int(
                datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp()
                * 1_000_000_000
            )
        except ValueError:
            pass
    return None


def delta_ms(start_ns: int | None, end_ns: int | None) -> float | None:
    if start_ns is None or end_ns is None or end_ns < start_ns:
        return None
    return round((end_ns - start_ns) / 1_000_000, 3)


def percentile(values: list[float], q: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    rank = max(0, min(len(ordered) - 1, math.ceil(q * len(ordered)) - 1))
    return round(ordered[rank], 3)


def first_named(rows: list[dict[str, Any]], suffix: str) -> dict[str, Any] | None:
    matches = [
        row
        for row in rows
        if str(row.get("event", "")).endswith(suffix) and event_ns(row) is not None
    ]
    return min(matches, key=lambda row: event_ns(row) or 0) if matches else None


def numeric(row: dict[str, Any], *keys: str) -> int | float | None:
    for key in keys:
        value = row.get(key)
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            return value
    return None


def trace_summary(
    trace_dir: Path, window_key: str, start_ns: int, end_ns: int | None
) -> dict[str, Any] | None:
    rows = load_jsonl(trace_dir / "events.jsonl")
    parsed = first_named(rows, "tool_request_parsed")
    if parsed is None or parsed.get("client_window_key") != window_key:
        return None
    parsed_ns = event_ns(parsed)
    if (
        parsed_ns is None
        or parsed_ns < start_ns
        or (end_ns is not None and parsed_ns > end_ns)
    ):
        return None

    received = first_named(rows, "tool_request_received")
    dispatch_started = first_named(rows, "tool_dispatch_started")
    dispatch_finished = first_named(rows, "tool_dispatch_finished")
    returned = first_named(rows, "tool_handler_returned")
    serialized = first_named(rows, "tool_response_serialized")
    runner_enqueued = first_named(rows, "tool_runner_request_enqueued")
    runner_dispatched = first_named(rows, "tool_runner_request_dispatched")
    runner_result = first_named(rows, "tool_runner_result_accepted")
    terminal = returned or serialized or dispatch_finished or parsed

    response_bytes = numeric(returned or {}, "estimated_json_bytes")
    if response_bytes is None:
        response_bytes = numeric(serialized or {}, "estimated_json_bytes", "bytes")

    payload_bytes = 0
    compressed_bytes = 0
    for row in rows:
        if row.get("event") in {
            "tool_trace_payload_captured",
            "tool_trace_payload_persisted",
        }:
            payload_bytes += int(numeric(row, "payload_bytes") or 0)
            compressed_bytes += int(numeric(row, "compressed_bytes") or 0)

    failed = any(
        row is not None
        and (
            row.get("tool_success") in (False, 0)
            or row.get("protocol_success") in (False, 0)
        )
        for row in (returned, dispatch_finished)
    )
    return {
        "server_trace_id": parsed.get("server_trace_id") or trace_dir.name,
        "tool_name": parsed.get("tool_name"),
        "request_received_ns": event_ns(received or parsed),
        "request_parsed_ns": parsed_ns,
        "dispatch_started_ns": event_ns(dispatch_started) if dispatch_started else None,
        "dispatch_finished_ns": event_ns(dispatch_finished) if dispatch_finished else None,
        "handler_returned_ns": event_ns(returned) if returned else event_ns(terminal),
        "runner_enqueued_ns": event_ns(runner_enqueued) if runner_enqueued else None,
        "runner_dispatched_ns": event_ns(runner_dispatched) if runner_dispatched else None,
        "runner_result_ns": event_ns(runner_result) if runner_result else None,
        "runner_job_id": (runner_enqueued or {}).get("runner_job_id"),
        "runner_request_kind": (runner_enqueued or {}).get("runner_request_kind"),
        "runner_transport": (runner_enqueued or {}).get("runner_transport"),
        "helper_ingress_started_ns": numeric(
            received or {}, "helper_ingress_started_unix_ns"
        ),
        "helper_ingress_pre_backend_ms": (
            round(
                float(numeric(received or {}, "helper_ingress_pre_backend_us")) / 1000,
                3,
            )
            if numeric(received or {}, "helper_ingress_pre_backend_us") is not None
            else None
        ),
        "response_bytes": int(response_bytes) if response_bytes is not None else None,
        "trace_payload_bytes": payload_bytes,
        "trace_payload_compressed_bytes": compressed_bytes,
        "failed": failed,
        "server_total_ms": (
            float(numeric(returned or {}, "duration_ms"))
            if numeric(returned or {}, "duration_ms") is not None
            else delta_ms(event_ns(received or parsed), event_ns(terminal))
        ),
        "dispatch_ms": delta_ms(
            event_ns(dispatch_started) if dispatch_started else None,
            event_ns(dispatch_finished) if dispatch_finished else None,
        ),
        "runner_round_trip_ms": delta_ms(
            event_ns(runner_dispatched)
            if runner_dispatched
            else event_ns(runner_enqueued)
            if runner_enqueued
            else None,
            event_ns(runner_result) if runner_result else None,
        ),
    }


def find_connection_layers(value: Any) -> dict[str, Any] | None:
    if isinstance(value, dict):
        layers = value.get("connection_layers")
        if isinstance(layers, dict):
            return layers
        for child in value.values():
            found = find_connection_layers(child)
            if found is not None:
                return found
    elif isinstance(value, list):
        for child in value:
            found = find_connection_layers(child)
            if found is not None:
                return found
    return None


def heartbeat_metrics(path: Path | None) -> dict[str, Any]:
    if path is None:
        return {
            "samples": 0,
            "sample_errors": 0,
            "layer_transitions": {},
            "disconnects": 0,
            "reconnects": 0,
            "last_status": {},
        }
    rows = load_jsonl(path)
    previous: dict[str, str] = {}
    transitions: dict[str, int] = {}
    disconnects = 0
    reconnects = 0
    sample_errors = 0
    down = {"disconnected", "offline", "lost", "unavailable", "stale"}
    up = {"connected", "ready", "registered", "online"}
    for row in rows:
        if row.get("ok") is False:
            sample_errors += 1
        layers = find_connection_layers(row) or row.get("layers")
        if not isinstance(layers, dict):
            continue
        for name, detail in layers.items():
            if not isinstance(detail, dict):
                continue
            status = detail.get("status")
            if not isinstance(status, str):
                continue
            old = previous.get(name)
            if old is not None and old != status:
                key = f"{name}:{old}->{status}"
                transitions[key] = transitions.get(key, 0) + 1
                if status in down:
                    disconnects += 1
                if old in down and status in up:
                    reconnects += 1
            previous[name] = status
    return {
        "samples": len(rows),
        "sample_errors": sample_errors,
        "layer_transitions": transitions,
        "disconnects": disconnects,
        "reconnects": reconnects,
        "last_status": previous,
    }


def request_windows(
    trace_root: Path,
    start_ns: int,
    end_ns: int | None,
) -> dict[str, list[tuple[int, str | None]]]:
    windows: dict[str, list[tuple[int, str | None]]] = {}
    for trace_dir in trace_root.iterdir():
        if not trace_dir.is_dir():
            continue
        for row in load_jsonl(trace_dir / "events.jsonl"):
            if row.get("event") != "mcp_tool_request_parsed":
                continue
            when = event_ns(row)
            key = row.get("client_window_key")
            if (
                not isinstance(key, str)
                or not key
                or key == "-"
                or when is None
                or when < start_ns
                or (end_ns is not None and when > end_ns)
            ):
                continue
            tool_name = row.get("tool_name")
            windows.setdefault(key, []).append(
                (when, tool_name if isinstance(tool_name, str) else None)
            )
    for rows in windows.values():
        rows.sort(key=lambda item: item[0])
    return windows


def select_window(
    driver_path: Path,
    trace_root: Path,
    submitted_ms: int,
    end_ns: int | None,
    marker: str | None,
    client_window_key: str | None,
    expected_tool_name: str | None,
    marker_argument_key: str | None,
) -> tuple[str, str]:
    if client_window_key:
        return client_window_key, "explicit"

    windows = request_windows(trace_root, submitted_ms * 1_000_000, end_ns)

    if marker:
        try:
            first = phase16a.profile(
                driver_path,
                trace_root,
                [marker],
                None,
                expected_tool_name=expected_tool_name,
                marker_argument_key=marker_argument_key,
            )
            return first["selection"]["client_window_key"], "exact_marker"
        except ValueError as exc:
            if not str(exc).startswith(
                "expected exactly one exact-marker client window"
            ):
                raise
            if expected_tool_name is None:
                raise
            candidates = sorted(
                key
                for key, rows in windows.items()
                if rows and rows[0][1] == expected_tool_name
            )
            if len(candidates) == 1:
                return candidates[0], "expected_first_tool_fallback"
            raise ValueError(
                f"exact marker correlation failed ({exc}); "
                f"expected-first-tool fallback found {candidates}"
            ) from exc

    candidates = sorted(windows)
    if len(candidates) != 1:
        raise ValueError(
            f"expected one client window, found {candidates}; "
            "pass --marker or --client-window-key"
        )
    return candidates[0], "time_window"


def profile(
    driver_path: Path,
    trace_root: Path,
    output_dir: Path,
    run_id: str,
    *,
    marker: str | None = None,
    client_window_key: str | None = None,
    expected_tool_name: str | None = None,
    marker_argument_key: str | None = None,
    heartbeat_path: Path | None = None,
) -> dict[str, Any]:
    driver = load_json(driver_path)
    submitted_ms = driver.get("submitted_at_ms")
    if not isinstance(submitted_ms, int):
        raise ValueError("driver JSON lacks integer submitted_at_ms")
    final_ms = driver.get("confirmed_final_at_ms")
    if not isinstance(final_ms, int):
        final_ms = driver.get("visible_final_at_ms")
    driver_finished_ms = driver.get("driver_finished_at_ms")
    if isinstance(final_ms, int):
        cutoff_ms = final_ms
        cutoff_source = "final_response"
    elif isinstance(driver_finished_ms, int):
        cutoff_ms = driver_finished_ms
        cutoff_source = "driver_finished_at_ms"
    else:
        cutoff_ms = driver_path.stat().st_mtime_ns // 1_000_000
        cutoff_source = "driver_file_mtime"
    end_ns = cutoff_ms * 1_000_000
    window_key, window_selection_mode = select_window(
        driver_path,
        trace_root,
        submitted_ms,
        end_ns,
        marker,
        client_window_key,
        expected_tool_name,
        marker_argument_key,
    )

    traces = [
        item
        for trace_dir in trace_root.iterdir()
        if trace_dir.is_dir()
        for item in [
            trace_summary(trace_dir, window_key, submitted_ms * 1_000_000, end_ns)
        ]
        if item is not None
    ]
    traces.sort(
        key=lambda item: item["request_received_ns"] or item["request_parsed_ns"] or 0
    )
    if not traces:
        raise ValueError("no tool traces matched the selected client window")

    gaps: list[float] = []
    for previous, current in zip(traces, traces[1:]):
        gap = delta_ms(
            previous.get("handler_returned_ns"), current.get("request_received_ns")
        )
        current["previous_observation_to_next_server_action_ms"] = gap
        if gap is not None:
            gaps.append(gap)

    response_sizes = [
        float(item["response_bytes"])
        for item in traces
        if isinstance(item.get("response_bytes"), int)
    ]
    heartbeat = heartbeat_metrics(heartbeat_path)
    driver_exit_code = driver.get("driver_exit_code")
    completed_flag = driver.get("completed")
    driver_completed = (
        isinstance(final_ms, int)
        and completed_flag is not False
        and driver_exit_code in (None, 0)
    )
    total_ms = (
        final_ms - submitted_ms
        if driver_completed and isinstance(final_ms, int) and final_ms >= submitted_ms
        else None
    )
    observed_window_ms = (
        cutoff_ms - submitted_ms if cutoff_ms >= submitted_ms else None
    )
    metrics = {
        "run_id": run_id,
        "client_window_key": window_key,
        "window_selection_mode": window_selection_mode,
        "driver_completed": driver_completed,
        "driver_exit_code": driver.get("driver_exit_code"),
        "driver_error": driver.get("error"),
        "completion_status": "completed" if driver_completed else "driver_failed",
        "cutoff_source": cutoff_source,
        "observed_window_ms": observed_window_ms,
        "tool_calls": len(traces),
        "failed_tool_calls": sum(1 for item in traces if item["failed"]),
        "unique_job_ids": len(
            {item["runner_job_id"] for item in traces if item.get("runner_job_id")}
        ),
        "prompt_to_first_server_received_ms": delta_ms(
            submitted_ms * 1_000_000, traces[0]["request_received_ns"]
        ),
        "prompt_to_first_helper_ingress_ms": delta_ms(
            submitted_ms * 1_000_000, traces[0].get("helper_ingress_started_ns")
        ),
        "first_helper_ingress_to_server_received_ms": delta_ms(
            traces[0].get("helper_ingress_started_ns"),
            traces[0]["request_received_ns"],
        ),
        "total_completion_ms": total_ms,
        "server_tool_time_ms": round(
            sum(item["server_total_ms"] or 0 for item in traces), 3
        ),
        "runner_round_trip_time_ms": round(
            sum(item["runner_round_trip_ms"] or 0 for item in traces), 3
        ),
        "observation_to_next_server_action": {
            "count": len(gaps),
            "p50_ms": percentile(gaps, 0.50),
            "p95_ms": percentile(gaps, 0.95),
            "max_ms": max(gaps) if gaps else None,
            "over_30s_count": sum(1 for value in gaps if value > 30_000),
        },
        "model_facing_response_bytes": {
            "known_count": len(response_sizes),
            "total": int(sum(response_sizes)),
            "peak": int(max(response_sizes)) if response_sizes else None,
        },
        "trace_payload_bytes": sum(item["trace_payload_bytes"] for item in traces),
        "trace_payload_compressed_bytes": sum(
            item["trace_payload_compressed_bytes"] for item in traces
        ),
        "heartbeat": heartbeat,
    }

    timeline: list[dict[str, Any]] = [
        {"event": "prompt_submitted", "wall_unix_ns": submitted_ms * 1_000_000}
    ]
    for index, trace in enumerate(traces, 1):
        timeline.extend(
            [
                {
                    "event": "tool_request_received",
                    "tool_index": index,
                    "tool_name": trace["tool_name"],
                    "server_trace_id": trace["server_trace_id"],
                    "wall_unix_ns": trace["request_received_ns"],
                },
                {
                    "event": "tool_handler_returned",
                    "tool_index": index,
                    "tool_name": trace["tool_name"],
                    "server_trace_id": trace["server_trace_id"],
                    "wall_unix_ns": trace["handler_returned_ns"],
                    "response_bytes": trace["response_bytes"],
                    "failed": trace["failed"],
                },
            ]
        )
    if driver_completed and isinstance(final_ms, int):
        timeline.append(
            {
                "event": "final_response_completed",
                "wall_unix_ns": final_ms * 1_000_000,
            }
        )
    else:
        timeline.append(
            {
                "event": "driver_observation_ended",
                "wall_unix_ns": cutoff_ms * 1_000_000,
                "driver_exit_code": driver.get("driver_exit_code"),
                "driver_error": driver.get("error"),
            }
        )
    timeline.sort(key=lambda row: row.get("wall_unix_ns") or 0)

    output_dir.mkdir(parents=True, exist_ok=True)
    (output_dir / "timeline.json").write_text(
        json.dumps({"run_id": run_id, "events": timeline}, indent=2) + "\n",
        encoding="utf-8",
    )
    (output_dir / "metrics.json").write_text(
        json.dumps(metrics, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    with (output_dir / "events.jsonl").open("w", encoding="utf-8") as handle:
        for row in timeline:
            handle.write(json.dumps({"run_id": run_id, **row}, sort_keys=True) + "\n")

    summary = [
        f"# Phase 19A run {run_id}",
        "",
        f"- Tool calls: {metrics['tool_calls']} (failed: {metrics['failed_tool_calls']})",
        f"- Total completion: {metrics['total_completion_ms']} ms",
        f"- Completion status: {metrics['completion_status']} "
        f"(driver exit: {metrics['driver_exit_code']})",
        f"- Observed window: {metrics['observed_window_ms']} ms "
        f"(cutoff: {metrics['cutoff_source']})",
        f"- Prompt -> first helper ingress: {metrics['prompt_to_first_helper_ingress_ms']} ms",
        f"- Prompt -> first server tool receipt: {metrics['prompt_to_first_server_received_ms']} ms",
        f"- First helper ingress -> server receipt: {metrics['first_helper_ingress_to_server_received_ms']} ms",
        f"- Server tool time: {metrics['server_tool_time_ms']} ms",
        f"- Observation -> next server action p95: {metrics['observation_to_next_server_action']['p95_ms']} ms",
        f"- >30 s inter-tool gaps: {metrics['observation_to_next_server_action']['over_30s_count']}",
        f"- Model-facing response bytes: {metrics['model_facing_response_bytes']['total']} "
        f"(peak: {metrics['model_facing_response_bytes']['peak']})",
        f"- Heartbeat samples: {heartbeat['samples']} "
        f"(errors: {heartbeat['sample_errors']}, disconnects: {heartbeat['disconnects']}, "
        f"reconnects: {heartbeat['reconnects']})",
        "",
        "Observation-to-next-server-action is an end-to-end control-plane interval, "
        "not pure model inference time.",
    ]
    (output_dir / "summary.md").write_text(
        "\n".join(summary) + "\n", encoding="utf-8"
    )
    return {"metrics": metrics, "traces": traces, "timeline": timeline}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--driver", type=Path, required=True)
    parser.add_argument("--trace-root", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--marker")
    parser.add_argument("--client-window-key")
    parser.add_argument("--expected-tool-name")
    parser.add_argument("--marker-argument-key")
    parser.add_argument("--heartbeat-jsonl", type=Path)
    args = parser.parse_args()
    result = profile(
        args.driver,
        args.trace_root,
        args.output_dir,
        args.run_id,
        marker=args.marker,
        client_window_key=args.client_window_key,
        expected_tool_name=args.expected_tool_name,
        marker_argument_key=args.marker_argument_key,
        heartbeat_path=args.heartbeat_jsonl,
    )
    print(json.dumps(result["metrics"], indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
