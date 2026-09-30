#!/usr/bin/env python3
"""Deterministic local workloads for Phase 19A long-haul pilot runs."""
from __future__ import annotations

import argparse
import json
import sys
import time


def emit(payload: dict) -> None:
    print(json.dumps(payload, sort_keys=True), flush=True)


def sleep_for(seconds: float) -> None:
    if seconds > 0:
        time.sleep(seconds)


def silent(duration: float) -> int:
    emit({"event": "started", "scenario": "silent"})
    sleep_for(duration)
    emit({"event": "terminal", "scenario": "silent", "status": "ok"})
    return 0


def bursty(duration: float, output_bytes: int) -> int:
    emit({"event": "started", "scenario": "bursty"})
    sleep_for(duration / 2)
    chunk = "B" * max(0, output_bytes)
    if chunk:
        sys.stdout.write(chunk + "\n")
        sys.stdout.flush()
    sleep_for(duration / 2)
    emit({"event": "terminal", "scenario": "bursty", "status": "ok"})
    return 0


def large_log(output_bytes: int) -> int:
    emit({"event": "started", "scenario": "large-log"})
    remaining = max(0, output_bytes)
    block = ("0123456789abcdef" * 256) + "\n"
    while remaining:
        piece = block[: min(len(block), remaining)]
        sys.stdout.write(piece)
        remaining -= len(piece)
    sys.stdout.write("\n")
    sys.stdout.flush()
    emit({"event": "terminal", "scenario": "large-log", "status": "ok"})
    return 0


def late_failure(duration: float) -> int:
    emit({"event": "started", "scenario": "late-failure"})
    sleep_for(duration)
    emit(
        {
            "event": "terminal",
            "scenario": "late-failure",
            "status": "expected_failure",
        }
    )
    return 23


def multi_stage(duration: float, fail_stage: str | None) -> int:
    stages = ("configure", "build", "test", "package")
    per_stage = duration / len(stages)
    for stage in stages:
        emit({"event": "stage_started", "stage": stage})
        sleep_for(per_stage)
        if stage == fail_stage:
            emit({"event": "stage_failed", "stage": stage})
            return 24
        emit({"event": "stage_completed", "stage": stage})
    emit({"event": "terminal", "scenario": "multi-stage", "status": "ok"})
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "scenario",
        choices=("silent", "bursty", "large-log", "late-failure", "multi-stage"),
    )
    parser.add_argument("--duration", type=float, default=5.0)
    parser.add_argument("--output-bytes", type=int, default=100_000)
    parser.add_argument("--fail-stage", choices=("configure", "build", "test", "package"))
    args = parser.parse_args()
    if args.duration < 0 or args.output_bytes < 0:
        parser.error("duration and output-bytes must be non-negative")
    if args.scenario == "silent":
        return silent(args.duration)
    if args.scenario == "bursty":
        return bursty(args.duration, args.output_bytes)
    if args.scenario == "large-log":
        return large_log(args.output_bytes)
    if args.scenario == "late-failure":
        return late_failure(args.duration)
    return multi_stage(args.duration, args.fail_stage)


if __name__ == "__main__":
    raise SystemExit(main())
