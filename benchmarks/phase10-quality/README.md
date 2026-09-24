# Phase 10 Quality Benchmark

This directory preserves the blind Phase 9 vs Phase 10 coding benchmark used to evaluate the pre-plan complexity estimator and Graphify-aware package planning.

## Retained evidence

- `source/`: untouched TaskDesk benchmark fixture.
- `phase9/`: implementation produced with the Phase 9 execution strategy.
- `phase10/`: implementation produced with the Phase 10 execution strategy.
- `evaluator/hidden_tests.py`: hidden evaluator used after both implementations completed.
- `prompt.txt`: identical coding task supplied to both runs.
- `quality-report.json`: execution metadata, diffs, visible tests, hidden-test results, and planning signals.
- `graphify-out/` inside each retained snapshot: graph evidence used to reproduce or audit Graphify-aware planning.

Exact replay copies were intentionally removed because they were byte-for-byte identical to their corresponding `phase9/` and `phase10/` snapshots and added no independent evidence.

## Recorded result

Both implementations passed the benchmark tests. The report records that Phase 10 used Graphify-informed planning, selected two effective packages, and completed with 10/10 hidden tests while preserving the blind-until-both-complete evaluation setup.

Treat `quality-report.json` as the canonical machine-readable benchmark record.
