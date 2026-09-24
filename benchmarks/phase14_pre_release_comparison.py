#!/usr/bin/env python3
"""Compare preserved Phase 11 samples with Phase 13, Phase 14, and WebCodex binaries.

This is a local Rust integration-harness benchmark. Build time and ChatGPT/network
latency are intentionally excluded. Phase 11 is loaded from the preserved
Phase 12 comparison evidence because its original executable is no longer kept.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import statistics
import subprocess
import time
from pathlib import Path

CASES = {
    "small_read_cancel": "tool_runtime::tests::chadex_task_executor::execute_task_cancellation_is_cooperative_between_steps",
    "small_edit_validate_review": "tool_runtime::tests::chadex_task_executor::execute_task_runs_read_guarded_edit_validation_and_review",
}
ORDERS = [
    ["phase13", "phase14", "webcodex"],
    ["webcodex", "phase14", "phase13"],
    ["phase14", "phase13", "webcodex"],
    ["phase13", "webcodex", "phase14"],
    ["phase14", "webcodex", "phase13"],
    ["webcodex", "phase13", "phase14"],
]


def measure(binary: Path, case: str) -> float:
    started = time.perf_counter()
    run = subprocess.run(
        [str(binary), case, "--exact", "--test-threads=1"],
        check=False,
        capture_output=True,
        text=True,
    )
    elapsed_ms = (time.perf_counter() - started) * 1000
    if run.returncode != 0 or "1 passed" not in run.stdout:
        raise RuntimeError(run.stdout + run.stderr)
    return elapsed_ms


def summarize(values: list[float]) -> dict[str, float]:
    p25, _, p75 = statistics.quantiles(values, n=4, method="inclusive")
    return {
        "median_ms": statistics.median(values),
        "mean_ms": statistics.fmean(values),
        "p25_ms": p25,
        "p75_ms": p75,
        "min_ms": min(values),
        "max_ms": max(values),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--phase13", type=Path, required=True)
    parser.add_argument("--phase14", type=Path, required=True)
    parser.add_argument("--webcodex", type=Path, required=True)
    parser.add_argument(
        "--phase11-evidence",
        type=Path,
        default=Path("benchmarks/phase12-execution-semantics.json"),
    )
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=11)
    parser.add_argument("--confirmation-samples", type=int, default=7)
    args = parser.parse_args()

    historical = json.loads(args.phase11_evidence.read_text())
    binaries = {
        "phase13": args.phase13.resolve(),
        "phase14": args.phase14.resolve(),
        "webcodex": args.webcodex.resolve(),
    }
    result = {
        "benchmark": "phase11_phase13_phase14_webcodex_executor_comparison",
        "kind": "local Rust integration harness",
        "method": (
            "Phase 13/14/WebCodex-derived compatibility runtime: one warmup each, balanced rotating order, "
            "no concurrent builds; Phase 11: preserved historical samples using "
            "the same measure() contract"
        ),
        "samples_per_variant": args.samples,
        "limitations": [
            "Includes disposable fixture and test-process startup; not ChatGPT Web/network latency.",
            "Phase 11 is preserved historical evidence, not a fresh rerun.",
            "Build time is excluded.",
            "The webcodex binary is the retained WebCodex-derived Phase 12 compatibility runtime; pristine upstream lacks these Chadex execute_task cases.",
        ],
        "source_identity": {
            "phase11": {
                "measurement": "historical preserved samples",
                "binary_sha256": historical["binary_sha256"]["phase11"],
            },
            "phase13": {
                "commit": "b717b1c",
                "binary_sha256": hashlib.sha256(binaries["phase13"].read_bytes()).hexdigest(),
            },
            "phase14": {
                "commit": "c0b8297",
                "binary_sha256": hashlib.sha256(binaries["phase14"].read_bytes()).hexdigest(),
            },
            "webcodex": {
                "measurement_label": "WebCodex-derived Phase 12 compatibility runtime",
                "pristine_upstream_directly_comparable": False,
                "base_upstream_revision": "1bdc05e488ee56bca358bc6ba455cebd5831917d",
                "version": "0.4.1",
                "binary_sha256": hashlib.sha256(binaries["webcodex"].read_bytes()).hexdigest(),
            },
        },
        "cases": {},
    }

    for name, case in CASES.items():
        samples = {
            "phase11": list(historical["cases"][name]["samples_ms"]["phase11"])
        }
        for variant, binary in binaries.items():
            measure(binary, case)
            samples[variant] = []
        for index in range(args.samples):
            for variant in ORDERS[index % len(ORDERS)]:
                samples[variant].append(measure(binaries[variant], case))
        summary = {variant: summarize(values) for variant, values in samples.items()}
        phase11_median = summary["phase11"]["median_ms"]
        webcodex_median = summary["webcodex"]["median_ms"]
        comparison = {}
        for variant in ("phase13", "phase14"):
            median = summary[variant]["median_ms"]
            comparison[variant] = {
                "vs_phase11_pct": 100 * (median / phase11_median - 1),
                "vs_webcodex_pct": 100 * (median / webcodex_median - 1),
            }
        comparison["phase14_vs_phase13_pct"] = 100 * (
            summary["phase14"]["median_ms"] / summary["phase13"]["median_ms"] - 1
        )
        result["cases"][name] = {
            "filter": case,
            "samples_ms": samples,
            "summary": summary,
            "comparison": comparison,
        }

    if args.confirmation_samples > 0:
        name = "small_edit_validate_review"
        case = CASES[name]
        confirmation = {variant: [] for variant in binaries}
        for binary in binaries.values():
            measure(binary, case)
        for index in range(args.confirmation_samples):
            for variant in ORDERS[index % len(ORDERS)]:
                confirmation[variant].append(measure(binaries[variant], case))
        medians = {
            variant: statistics.median(values)
            for variant, values in confirmation.items()
        }
        result["cases"][name]["confirmation_7_samples"] = {
            "samples_ms": confirmation,
            "median_ms": medians,
            "phase14_vs_phase13_pct": 100
            * (medians["phase14"] / medians["phase13"] - 1),
            "phase14_vs_webcodex_pct": 100
            * (medians["phase14"] / medians["webcodex"] - 1),
        }

    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
