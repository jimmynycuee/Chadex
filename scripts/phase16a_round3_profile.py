#!/usr/bin/env python3
"""Rebuild the Phase 16A Round 3 first-tool decomposition with exact IDs."""
from __future__ import annotations

import argparse
import calendar
import json
from datetime import datetime
from pathlib import Path

TASKS = ("small", "medium", "large")
HARNESSES = ("chadex", "webcodex")


def read_jsonl(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def iso_ns(value: str | None) -> int | None:
    if not isinstance(value, str):
        return None
    parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    seconds = calendar.timegm(parsed.utctimetuple())
    return seconds * 1_000_000_000 + parsed.microsecond * 1_000


def ms(delta_ns: int | float | None) -> float | None:
    if delta_ns is None:
        return None
    value = round(delta_ns / 1_000_000, 3)
    return 0.0 if value == 0 else value


def singleton(rows: list[dict], event: str) -> dict | None:
    found = [row for row in rows if row.get("event") == event]
    return found[0] if len(found) == 1 else None


def outer_first(path: Path) -> dict:
    rows = read_jsonl(path)
    prompts = [r for r in rows if r.get("event") == "prompt_submitted" and isinstance(r.get("wall_ns"), int)]
    starts = [r for r in rows if r.get("event") == "tool_start" and isinstance(r.get("wall_ns"), int) and isinstance(r.get("id"), str)]
    if len(prompts) != 1 or not starts:
        raise ValueError(f"invalid outer trace: {path}")
    first = min(starts, key=lambda r: r["wall_ns"])
    ends = [r for r in rows if r.get("event") == "tool_end" and r.get("id") == first["id"]]
    if len(ends) != 1:
        raise ValueError(f"first tool exact id has {len(ends)} ends: {path}")
    duration_ns = ends[0].get("duration_ns")
    if not isinstance(duration_ns, int):
        duration_ns = ends[0]["wall_ns"] - first["wall_ns"]
    return {
        "prompt_ns": prompts[0]["wall_ns"],
        "first_ns": first["wall_ns"],
        "trace_id": first["id"],
        "tool": first.get("tool"),
        "duration_ms": ms(duration_ns),
    }


def exact_server_trace(runtime_root: Path, trace_id: str) -> Path:
    matches = [p for p in runtime_root.glob(f"*/{trace_id}/events.jsonl") if p.parent.name == trace_id]
    if len(matches) != 1:
        raise ValueError(f"expected exactly one server trace {trace_id}, found {len(matches)}")
    return matches[0]


def server_profile(events_path: Path, trace_id: str, outer_tool: str | None) -> dict:
    rows = read_jsonl(events_path)
    ids = {r.get("server_trace_id") for r in rows if r.get("server_trace_id") is not None}
    if ids != {trace_id}:
        raise ValueError(f"server_trace_id mismatch in {events_path}: {ids}")

    received = singleton(rows, "mcp_tool_request_received")
    parsed = singleton(rows, "mcp_tool_request_parsed")
    started = singleton(rows, "mcp_tool_dispatch_started")
    finished = singleton(rows, "mcp_tool_dispatch_finished")
    serialized = singleton(rows, "mcp_tool_response_serialized")
    returned = singleton(rows, "mcp_tool_handler_returned")
    if any(x is None for x in (received, parsed, started, finished, serialized, returned)):
        raise ValueError(f"incomplete server lifecycle: {events_path}")

    parsed_tool = parsed.get("tool_name")
    if outer_tool is not None and parsed_tool != outer_tool:
        raise ValueError(f"tool mismatch after exact-id join: outer={outer_tool!r} server={parsed_tool!r}")

    times = {name: iso_ns(row.get("timestamp")) for name, row in (
        ("received", received), ("parsed", parsed), ("started", started),
        ("finished", finished), ("serialized", serialized), ("returned", returned),
    )}
    if any(value is None for value in times.values()):
        raise ValueError(f"missing server timestamp: {events_path}")

    return {
        **times,
        "tool": parsed_tool,
        "client_window_key": parsed.get("client_window_key"),
        "response_bytes": serialized.get("estimated_json_bytes"),
    }


def same_window_pre_first(arm_root: Path, first_id: str, window: str | None, prompt_ns: int, received_ns: int) -> list[dict]:
    if not isinstance(window, str) or not window:
        return []
    found = []
    for path in arm_root.glob("*/events.jsonl"):
        if path.parent.name == first_id:
            continue
        rows = read_jsonl(path)
        parsed = singleton(rows, "mcp_tool_request_parsed")
        received = singleton(rows, "mcp_tool_request_received")
        if not parsed or not received or parsed.get("client_window_key") != window:
            continue
        stamp = iso_ns(received.get("timestamp"))
        if stamp is not None and prompt_ns <= stamp < received_ns:
            found.append({
                "server_trace_id": path.parent.name,
                "tool_name": parsed.get("tool_name"),
                "after_prompt_ms": ms(stamp - prompt_ns),
            })
    return sorted(found, key=lambda row: row["after_prompt_ms"])


def profile_arm(outer_path: Path, runtime_root: Path) -> dict:
    outer = outer_first(outer_path)
    events_path = exact_server_trace(runtime_root, outer["trace_id"])
    server = server_profile(events_path, outer["trace_id"], outer["tool"])
    arm_root = events_path.parent.parent
    pre = same_window_pre_first(
        arm_root, outer["trace_id"], server["client_window_key"],
        outer["prompt_ns"], server["received"],
    )
    return {
        "outer_trace": str(outer_path),
        "runtime_trace_root": str(arm_root),
        "exact_join": {
            "key": "server_trace_id",
            "value": outer["trace_id"],
            "joined_by_time_or_tool_name": False,
        },
        "first_tool": server["tool"],
        "prompt_to_first_tool_ms": ms(outer["first_ns"] - outer["prompt_ns"]),
        "prompt_to_server_ingress_ms": ms(server["received"] - outer["prompt_ns"]),
        "adapter_server_skew_ms": ms(server["received"] - outer["first_ns"]),
        "outer_first_tool_total_ms": outer["duration_ms"],
        "server_receive_to_parse_ms": ms(server["parsed"] - server["received"]),
        "server_parse_to_dispatch_ms": ms(server["started"] - server["parsed"]),
        "server_dispatch_ms": ms(server["finished"] - server["started"]),
        "server_dispatch_to_serialize_ms": ms(server["serialized"] - server["finished"]),
        "server_serialize_to_return_ms": ms(server["returned"] - server["serialized"]),
        "server_handler_ms": ms(server["returned"] - server["received"]),
        "first_response_estimated_json_bytes": server["response_bytes"],
        "server_visible_pre_first_request_count_same_window": len(pre),
        "server_visible_pre_first_requests_same_window": pre,
        "coverage": {
            "outer_prompt": "observed",
            "host_model_before_tool_emit": "unattributed",
            "host_connector_dispatch": "unattributed",
            "hosted_relay": "unattributed",
            "native_helper_ingress": "not_exported_in_round3",
            "server_ingress": "observed_exact_id",
            "server_dispatch": "observed_exact_id",
            "runner": "not_applicable_to_first_tool",
        },
    }


def build(round3_root: Path) -> dict:
    runtime_root = round3_root / "results" / "runtime-traces"
    runs = {}
    first_deltas, ingress_deltas = [], []
    for task in TASKS:
        pair = {}
        for harness in HARNESSES:
            outer = round3_root / "results" / f"{task}-{harness}-manual-trace.jsonl"
            pair[harness] = profile_arm(outer, runtime_root)
        delta = {
            "first_tool_ms_chadex_minus_webcodex": round(pair["chadex"]["prompt_to_first_tool_ms"] - pair["webcodex"]["prompt_to_first_tool_ms"], 3),
            "pre_server_ms_chadex_minus_webcodex": round(pair["chadex"]["prompt_to_server_ingress_ms"] - pair["webcodex"]["prompt_to_server_ingress_ms"], 3),
            "server_handler_ms_chadex_minus_webcodex": round(pair["chadex"]["server_handler_ms"] - pair["webcodex"]["server_handler_ms"], 3),
            "adapter_server_skew_ms_chadex_minus_webcodex": round(pair["chadex"]["adapter_server_skew_ms"] - pair["webcodex"]["adapter_server_skew_ms"], 3),
        }
        pair["differential"] = delta
        runs[task] = pair
        first_deltas.append(delta["first_tool_ms_chadex_minus_webcodex"])
        ingress_deltas.append(delta["pre_server_ms_chadex_minus_webcodex"])
    return {
        "schema_version": 1,
        "phase": "16A",
        "diagnostic_only": True,
        "round3_root": str(round3_root),
        "runs": runs,
        "summary": {
            "paired_tasks": list(TASKS),
            "join_policy": "exact server_trace_id only; no timing/tool-name join",
            "host_model_boundary_policy": "unattributed",
            "first_tool_gap_ms_range": [min(first_deltas), max(first_deltas)],
            "pre_server_gap_ms_range": [min(ingress_deltas), max(ingress_deltas)],
            "all_gap_before_server_ingress": all(abs(a - b) < 0.001 for a, b in zip(first_deltas, ingress_deltas)),
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--round3-root", type=Path,
        default=Path(__file__).resolve().parents[2] / "agent-harness-benchmark" / "round3",
    )
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    result = build(args.round3_root.resolve())
    encoded = json.dumps(result, indent=2, ensure_ascii=False, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded)
    else:
        print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
