#!/usr/bin/env python3
"""Phase 16C matched-run first-tool variance analyzer.

Consumes a manifest of exact-ID first-tool profile JSONs. It never infers request
identity by timestamp or tool name; every included profile must already contain a
server_trace_id produced by the exact-ID profiler.
"""
from __future__ import annotations

import argparse
from collections import defaultdict
import json
import math
from pathlib import Path
import statistics
from typing import Any


VALID_TEMPERATURE_STATES = {"cold", "warm", "unknown"}
VALID_CHAT_STATES = {"fresh", "reused", "unknown"}


def percentile(values: list[float], q: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    if len(ordered) == 1:
        return round(ordered[0], 3)
    position = (len(ordered) - 1) * q
    lower = math.floor(position)
    upper = math.ceil(position)
    if lower == upper:
        return round(ordered[lower], 3)
    weight = position - lower
    return round(ordered[lower] * (1.0 - weight) + ordered[upper] * weight, 3)


def stats(values: list[float]) -> dict[str, Any]:
    clean = [float(value) for value in values if isinstance(value, (int, float))]
    if not clean:
        return {"n": 0}
    q1 = percentile(clean, 0.25)
    q3 = percentile(clean, 0.75)
    return {
        "n": len(clean),
        "mean_ms": round(statistics.fmean(clean), 3),
        "median_ms": round(statistics.median(clean), 3),
        "min_ms": round(min(clean), 3),
        "max_ms": round(max(clean), 3),
        "range_ms": round(max(clean) - min(clean), 3),
        "p95_ms": percentile(clean, 0.95),
        "q1_ms": q1,
        "q3_ms": q3,
        "iqr_ms": round(q3 - q1, 3) if q1 is not None and q3 is not None else None,
        "sample_stdev_ms": round(statistics.stdev(clean), 3) if len(clean) > 1 else None,
    }


def numeric(value: Any) -> float | None:
    return float(value) if isinstance(value, (int, float)) else None


def read_profile(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"profile is not an object: {path}")
    first = value.get("first_tool")
    if not isinstance(first, dict) or not isinstance(first.get("server_trace_id"), str):
        raise ValueError(f"profile lacks exact server_trace_id: {path}")
    return value


def run_metrics(profile: dict[str, Any]) -> dict[str, float | None]:
    first = profile["first_tool"]
    helper = first.get("helper_timing")
    unattributed = profile.get("unattributed")
    return {
        "prompt_to_server_received_ms": numeric(first.get("prompt_to_server_received_ms")),
        "prompt_to_helper_ingress_ms": numeric(
            unattributed.get("prompt_to_helper_ingress_ms")
            if isinstance(unattributed, dict)
            else None
        ),
        "helper_ingress_to_server_received_ms": numeric(
            helper.get("ingress_to_server_received_ms") if isinstance(helper, dict) else None
        ),
        "helper_pre_backend_ms": numeric(
            helper.get("ingress_pre_backend_ms") if isinstance(helper, dict) else None
        ),
        "helper_backend_send_to_server_received_ms": numeric(
            helper.get("backend_send_to_server_received_ms")
            if isinstance(helper, dict)
            else None
        ),
        "server_handler_ms": numeric(first.get("server_total_to_handler_return_ms")),
    }


def normalized_run(manifest_dir: Path, raw: dict[str, Any]) -> dict[str, Any]:
    run_id = raw.get("run_id")
    harness = raw.get("harness")
    profile_ref = raw.get("profile")
    if not isinstance(run_id, str) or not run_id:
        raise ValueError("every run requires non-empty run_id")
    if harness not in {"chadex", "webcodex"}:
        raise ValueError(f"run {run_id} has unsupported harness {harness!r}")
    if not isinstance(profile_ref, str) or not profile_ref:
        raise ValueError(f"run {run_id} requires profile path")
    order_position = raw.get("order_position")
    if not isinstance(order_position, int) or order_position < 1:
        raise ValueError(f"run {run_id} requires positive integer order_position")

    states: dict[str, str] = {}
    for field in ("connector_state", "registry_state"):
        state = raw.get(field, "unknown")
        if state not in VALID_TEMPERATURE_STATES:
            raise ValueError(f"run {run_id} has invalid {field}={state!r}")
        states[field] = state
    chat_state = raw.get("chat_state", "unknown")
    if chat_state not in VALID_CHAT_STATES:
        raise ValueError(f"run {run_id} has invalid chat_state={chat_state!r}")
    states["chat_state"] = chat_state

    profile_path = (manifest_dir / profile_ref).resolve()
    profile = read_profile(profile_path)
    first = profile["first_tool"]
    return {
        "run_id": run_id,
        "harness": harness,
        "pair_id": raw.get("pair_id"),
        "order_position": order_position,
        **states,
        "profile": profile_ref,
        "server_trace_id": first["server_trace_id"],
        "tool_name": first.get("tool_name"),
        "helper_join_status": first.get("helper_join_status"),
        "metrics": run_metrics(profile),
    }


def metric_stats(runs: list[dict[str, Any]], metric: str) -> dict[str, Any]:
    values = [
        run["metrics"].get(metric)
        for run in runs
        if isinstance(run.get("metrics"), dict)
    ]
    return stats([value for value in values if isinstance(value, (int, float))])


def grouped_stats(
    runs: list[dict[str, Any]],
    fields: tuple[str, ...],
    metric: str = "prompt_to_server_received_ms",
) -> dict[str, Any]:
    groups: dict[tuple[Any, ...], list[dict[str, Any]]] = defaultdict(list)
    for run in runs:
        groups[tuple(run.get(field) for field in fields)].append(run)
    return {
        "|".join(str(part) for part in key): metric_stats(group, metric)
        for key, group in sorted(groups.items(), key=lambda item: tuple(str(x) for x in item[0]))
    }


def paired_deltas(runs: list[dict[str, Any]]) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    pairs: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for run in runs:
        pair_id = run.get("pair_id")
        if isinstance(pair_id, str) and pair_id:
            pairs[pair_id].append(run)

    rows: list[dict[str, Any]] = []
    deltas: list[float] = []
    for pair_id, members in sorted(pairs.items()):
        by_harness: dict[str, dict[str, Any]] = {}
        for member in members:
            harness = member["harness"]
            if harness in by_harness:
                raise ValueError(f"pair {pair_id} contains duplicate harness {harness}")
            by_harness[harness] = member
        if set(by_harness) != {"chadex", "webcodex"}:
            continue
        chadex = by_harness["chadex"]["metrics"]["prompt_to_server_received_ms"]
        webcodex = by_harness["webcodex"]["metrics"]["prompt_to_server_received_ms"]
        if chadex is None or webcodex is None:
            continue
        delta = round(chadex - webcodex, 3)
        deltas.append(delta)
        rows.append({
            "pair_id": pair_id,
            "chadex_run_id": by_harness["chadex"]["run_id"],
            "webcodex_run_id": by_harness["webcodex"]["run_id"],
            "chadex_order_position": by_harness["chadex"]["order_position"],
            "webcodex_order_position": by_harness["webcodex"]["order_position"],
            "chadex_minus_webcodex_ms": delta,
        })
    return rows, stats(deltas)


def analyze(manifest_path: Path) -> dict[str, Any]:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if not isinstance(manifest, dict) or not isinstance(manifest.get("runs"), list):
        raise ValueError("manifest must be an object with runs array")
    if manifest.get("schema_version", 1) != 1:
        raise ValueError("unsupported manifest schema_version")

    runs = [
        normalized_run(manifest_path.parent, raw)
        for raw in manifest["runs"]
        if isinstance(raw, dict)
    ]
    if len(runs) != len(manifest["runs"]):
        raise ValueError("every runs entry must be an object")
    run_ids = [run["run_id"] for run in runs]
    if len(set(run_ids)) != len(run_ids):
        raise ValueError("run_id values must be unique")

    pairs, pair_stats = paired_deltas(runs)
    chadex_runs = [run for run in runs if run["harness"] == "chadex"]
    attribution = {
        metric: metric_stats(chadex_runs, metric)
        for metric in (
            "prompt_to_helper_ingress_ms",
            "helper_ingress_to_server_received_ms",
            "helper_pre_backend_ms",
            "helper_backend_send_to_server_received_ms",
            "server_handler_ms",
        )
    }

    return {
        "schema_version": 1,
        "classification_rule": (
            "input profiles must already be exact server_trace_id joins; this analyzer "
            "never identifies requests by timestamp or tool name"
        ),
        "run_count": len(runs),
        "runs": runs,
        "prompt_to_server_by_harness": {
            harness: metric_stats(
                [run for run in runs if run["harness"] == harness],
                "prompt_to_server_received_ms",
            )
            for harness in ("chadex", "webcodex")
        },
        "grouped_effects": {
            "harness_by_order_position": grouped_stats(runs, ("harness", "order_position")),
            "harness_by_connector_state": grouped_stats(runs, ("harness", "connector_state")),
            "harness_by_registry_state": grouped_stats(runs, ("harness", "registry_state")),
            "harness_by_chat_state": grouped_stats(runs, ("harness", "chat_state")),
        },
        "paired_runs": pairs,
        "paired_chadex_minus_webcodex_ms": pair_stats,
        "chadex_attribution_spans": attribution,
        "interpretation_guardrail": (
            "group differences are descriptive until repeated/interleaved samples support "
            "them; prompt-to-helper is host/relay/model-side unattributed, not reasoning time"
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    result = analyze(args.manifest)
    encoded = json.dumps(result, indent=2, ensure_ascii=False, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded, encoding="utf-8")
    else:
        print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
