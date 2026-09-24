#!/usr/bin/env python3
"""Summarize existing Chadex lifecycle and external execution traces without payloads.

All outer time outside observed tool intervals remains unattributed. Server
request segments are nested envelopes; runner-reported command duration is not
added to Server↔Runner elapsed time. No model/backend time is inferred.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
from datetime import datetime


def ms(ns: int | float | None) -> float | None:
    return round(ns / 1_000_000, 3) if ns is not None else None


def timestamp_ns(event: dict) -> int | None:
    direct = event.get("wall_unix_ns", event.get("wall_ns"))
    if isinstance(direct, int):
        return direct
    raw = event.get("timestamp")
    if isinstance(raw, str):
        try:
            return int(datetime.fromisoformat(raw.replace("Z", "+00:00")).timestamp() * 1e9)
        except ValueError:
            pass
    return None


def read_jsonl(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def interval_union(intervals: list[tuple[int, int]]) -> int:
    merged: list[list[int]] = []
    for start, end in sorted(intervals):
        if not merged or start > merged[-1][1]:
            merged.append([start, end])
        else:
            merged[-1][1] = max(end, merged[-1][1])
    return sum(end - start for start, end in merged)


def outer_summary(path: Path) -> dict:
    events = read_jsonl(path)
    prompt = [timestamp_ns(x) for x in events if x.get("event") == "prompt_submitted"]
    final = [timestamp_ns(x) for x in events if x.get("event") == "final_response_end"]
    starts = {x.get("id"): timestamp_ns(x) for x in events if x.get("event") == "tool_start"}
    ends = {x.get("id"): timestamp_ns(x) for x in events if x.get("event") == "tool_end"}
    spans = sorted((start, ends[key]) for key, start in starts.items()
                   if key in ends and start is not None and ends[key] is not None and ends[key] >= start)
    total = final[0] - prompt[0] if len(prompt) == len(final) == 1 and prompt[0] is not None and final[0] is not None else None
    tool_union = interval_union(spans) if spans else None
    gaps = [(spans[i][1], spans[i+1][0]) for i in range(len(spans)-1)
            if spans[i+1][0] > spans[i][1]]
    commands = [x["duration_ns"] for x in events if x.get("event") == "command_measurement"
                and isinstance(x.get("duration_ns"), int)]
    return {
        "source": str(path),
        "task": next((x.get("task") for x in events if x.get("task")), None),
        "harness": next((x.get("harness") for x in events if x.get("harness")), None),
        "total_wall_ms": ms(total),
        "prompt_to_first_tool_ms": ms(spans[0][0]-prompt[0]) if spans and len(prompt)==1 and prompt[0] is not None else None,
        "tool_count": len(spans),
        "tool_wall_union_ms": ms(tool_union),
        "command_reported_sum_ms": ms(sum(commands)) if commands else None,
        "outside_tool_unattributed_ms": ms(total-tool_union) if total is not None and tool_union is not None else None,
        "gaps_over_10s_ms": [ms(end-start) for start, end in gaps if end-start > 10_000_000_000],
        "model_backend_ms": None,
    }


def server_summary(path: Path) -> dict:
    events = read_jsonl(path)
    points = {x.get("event"): x for x in events}
    request = next((x for x in events if x.get("event", "").endswith("_tool_request_received")), None)
    prefix = request["event"].split("_tool_request_received")[0] if request else None
    def elapsed(suffix: str) -> int | None:
        event = points.get(f"{prefix}_{suffix}") if prefix else None
        value = event.get("request_elapsed_ns") if event else None
        return value if isinstance(value, int) else None
    def delta(start: str, end: str) -> float | None:
        a, b = elapsed(start), elapsed(end)
        return ms(b-a) if a is not None and b is not None and b >= a else None
    runner: dict[str, dict] = {}
    for event in events:
        request_id = event.get("runner_request_id")
        if not isinstance(request_id, str):
            continue
        entry = runner.setdefault(request_id, {"kind": event.get("runner_request_kind")})
        when = event.get("process_elapsed_ns")
        when = when if isinstance(when, int) else None
        name = event.get("event")
        if name == "tool_runner_request_enqueued": entry["enqueued_ns"] = when
        elif name == "tool_runner_request_dispatched": entry["dispatched_ns"] = when
        elif name == "tool_runner_result_accepted":
            entry["accepted_ns"] = when
            entry["runner_reported_duration_ms"] = event.get("runner_reported_duration_ms")
    runner_rows = []
    for request_id, entry in runner.items():
        a,b,c = (entry.get(key) for key in ("enqueued_ns","dispatched_ns","accepted_ns"))
        runner_rows.append({
            "request_id": request_id, "kind": entry.get("kind"),
            "queue_to_dispatch_ms": ms(b-a) if a is not None and b is not None and b >= a else None,
            "dispatch_to_accepted_ms": ms(c-b) if b is not None and c is not None and c >= b else None,
            "runner_reported_duration_ms": entry.get("runner_reported_duration_ms"),
        })
    subphases = [{"event": x["event"], "duration_ms": ms(x.get("duration_ns")),
                  "bytes": x.get("bytes"), "item_count": x.get("item_count")}
                 for x in events if x.get("event") in {
                     "session_handoff_built", "work_on_project_prepared",
                     "work_on_project_resume_requested_prepared", "work_on_project_projected",
                     "coding_session_resume_lookup", "coding_context_probes",
                     "coding_session_resume_ensure", "coding_session_create_ensure"}]
    startup = next((x["duration_ms"] for x in subphases if x["event"] in
                    ("work_on_project_prepared", "work_on_project_resume_requested_prepared")), None)
    known_startup = [x["duration_ms"] for x in subphases if x["event"] in {
        "coding_session_resume_lookup", "coding_context_probes",
        "coding_session_resume_ensure", "coding_session_create_ensure"}]
    phase_names = {x["event"] for x in subphases}
    startup_complete = "coding_context_probes" in phase_names and bool(
        {"coding_session_resume_ensure", "coding_session_create_ensure"} & phase_names)
    if "work_on_project_resume_requested_prepared" in phase_names:
        startup_complete = startup_complete and "coding_session_resume_lookup" in phase_names
    startup_other = round(startup - sum(known_startup), 3) if startup is not None and startup_complete and all(
        value is not None for value in known_startup) else None
    return {
        "source": str(path), "server_trace_id": next((x.get("server_trace_id") for x in events if x.get("server_trace_id")), None),
        "has_handler_lifecycle": request is not None,
        "tool": next((x.get("tool_name") for x in reversed(events) if x.get("tool_name")), None),
        "workflow_session_sha256": next((x.get("workflow_session_sha256") for x in events
                                         if x.get("event") == "tool_workflow_session_linked"), None),
        "client_window_key": next((x.get("client_window_key") for x in reversed(events)
                                   if x.get("client_window_key")), None),
        "request_total_ms": ms(elapsed("tool_handler_returned")),
        "request_preparation_envelope_ms": delta("tool_request_received", "tool_dispatch_started"),
        "tool_dispatch_envelope_ms": delta("tool_dispatch_started", "tool_dispatch_finished"),
        "tool_result_projection_ms": delta("tool_dispatch_finished", "tool_response_serialized"),
        "response_handoff_ms": delta("tool_response_serialized", "tool_handler_returned"),
        "auth_validation_ms": ms(points["mcp_auth_validation_finished"]["request_elapsed_ns"]-points["mcp_auth_validation_started"]["request_elapsed_ns"])
            if "mcp_auth_validation_finished" in points and "mcp_auth_validation_started" in points
            and isinstance(points["mcp_auth_validation_finished"].get("request_elapsed_ns"), int)
            and isinstance(points["mcp_auth_validation_started"].get("request_elapsed_ns"), int) else None,
        "input_bytes": next((x.get("bytes") for x in events if x.get("event") in ("mcp_request_decoded", "api_request_decoded")), None),
        "runner": runner_rows, "subphases": subphases,
        "startup_other_unattributed_ms": startup_other,
        "model_backend_ms": None, "model_first_token_ms": None,
    }


def helper_index(paths: list[Path]) -> dict[str, dict]:
    """Join the existing native MCP ingress export by its Server response ID."""
    indexed = {}
    for path in paths:
        payload = json.loads(path.read_text())
        traces = payload if isinstance(payload, list) else payload.get("mcp_traces", [])
        for trace in traces:
            trace_id = trace.get("server_trace_id")
            if not isinstance(trace_id, str) or not trace_id:
                continue
            indexed[trace_id] = {
                "source": str(path),
                "native_ingress_total_ms": ms(trace["total_us"] * 1000) if isinstance(trace.get("total_us"), int) else None,
                "native_pre_backend_ms": ms(trace["ingress_pre_backend_us"] * 1000) if isinstance(trace.get("ingress_pre_backend_us"), int) else None,
                "native_backend_headers_ms": ms(trace["backend_headers_us"] * 1000) if isinstance(trace.get("backend_headers_us"), int) else None,
                "native_response_stream_ms": ms(trace["response_stream_us"] * 1000) if isinstance(trace.get("response_stream_us"), int) else None,
                "request_bytes": trace.get("request_bytes"),
                "response_bytes": trace.get("response_bytes"),
            }
    return indexed


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--outer-trace", type=Path, action="append", default=[])
    parser.add_argument("--server-trace-root", type=Path)
    parser.add_argument("--helper-traces", type=Path, action="append", default=[],
                        help="JSON array or object with mcp_traces from existing native ingress export")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    server_files = sorted(args.server_trace_root.glob("*/events.jsonl")) if args.server_trace_root else []
    servers = [server_summary(path) for path in server_files]
    helpers = helper_index(args.helper_traces)
    for server in servers:
        server["native_ingress"] = helpers.get(server["server_trace_id"])
    result = {"schema_version": 1,
              "classification_rule": "outer time outside observed tool intervals is unattributed; server envelopes may contain nested runner and handoff durations",
              "outer": [outer_summary(path) for path in args.outer_trace],
              "server_requests": servers}
    encoded = json.dumps(result, indent=2, ensure_ascii=False) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded)
    else:
        print(encoded, end="")


if __name__ == "__main__":
    main()
