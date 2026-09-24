#!/usr/bin/env python3
"""Compare preserved Phase 11/12 Rust test executables, with no concurrent builds.

This is local integration-harness wall time, including disposable fixtures and
process startup. It is neither ChatGPT Web nor production connector latency.
"""
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


def measure(binary, case):
    start = time.perf_counter()
    run = subprocess.run([str(binary), case, "--exact", "--test-threads=1"],
                         check=True, capture_output=True, text=True)
    if "1 passed" not in run.stdout:
        raise RuntimeError(run.stdout + run.stderr)
    return (time.perf_counter() - start) * 1000


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=11)
    args = parser.parse_args()
    binaries = {"phase11": args.baseline.resolve(), "phase12": args.candidate.resolve()}
    result = {
        "kind": "local Rust integration harness",
        "limitations": ["includes fixture and test-process startup", "not ChatGPT Web or connector E2E",
                        "test builds isolate state; Phase 12 receipts are real fsync-backed files",
                        "legacy UI state/log persistence disabled equally in both test builds",
                        "Phase 12 test storage also persists recovery plans; Phase 11 test builds skipped this existing production I/O"],
        "method": "one warmup each, alternating AB/BA pairs, no concurrent builds",
        "samples_per_variant": args.samples,
        "binary_sha256": {name: hashlib.sha256(path.read_bytes()).hexdigest() for name, path in binaries.items()},
        "cases": {},
    }
    for name, case in CASES.items():
        samples = {key: [] for key in binaries}
        for binary in binaries.values():
            measure(binary, case)
        for index in range(args.samples):
            order = list(binaries) if index % 2 == 0 else list(reversed(binaries))
            for variant in order:
                samples[variant].append(measure(binaries[variant], case))
        medians = {key: statistics.median(values) for key, values in samples.items()}
        result["cases"][name] = {
            "filter": case, "samples_ms": samples, "median_ms": medians,
            "delta_ms": medians["phase12"] - medians["phase11"],
            "delta_pct": 100 * (medians["phase12"] / medians["phase11"] - 1),
        }
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({name: {k: v for k, v in case.items() if k != "samples_ms"}
                      for name, case in result["cases"].items()}, indent=2))


if __name__ == "__main__":
    main()
