# Phase 10D — Adaptive Packaging Benchmark

## Scope

Compare the same deterministic `execute_task` plans on the same Phase 10 candidate runtime:

- **Phase 9 fixed-1**: `package_count = 1`
- **Phase 10 adaptive**: complexity-guided package count plus Graphify-aware dependency merging

The benchmark uses an isolated helper/server/runner and a fresh Git checkout per measured run. Runtime setup, checkout creation, Graphify graph generation, and hidden evaluation are excluded from the timed region.

Final dataset: **10 iterations × 2 variants × 3 scenarios = 60 measured workflows**.

## Correctness

- 60/60 measured workflows completed correctly.
- Phase 9 and Phase 10 produced identical diff hashes for every scenario.
- Hidden changed-file checks, marker checks, Python compilation, validation, and review all passed.
- Outer `execute_task` call count remained **1** for both variants.

## Final timing

| Scenario | Phase 9 fixed-1 median | Phase 10 adaptive median | Relative change | Adaptive packages | Graphify planning |
|---|---:|---:|---:|---:|---:|
| Small | 350.942 ms | 347.171 ms | -1.07% | 1 | 0 ms |
| Medium | 458.580 ms | 448.973 ms | -2.09% | 2 | 0 ms |
| Large | 635.338 ms | 711.211 ms | +11.94% | 2 after 4→2 merge | 94.5 ms |

Paired median deltas:

- Small: **-0.441 ms**
- Medium: **-8.162 ms**
- Large: **+91.728 ms**

Small/medium differences are within normal local-run variance. The large-task overhead is real and is dominated by fresh Graphify dependency analysis.

## Phase 10D tuning

Initial Phase 10C behavior queried Graphify for any task with 2+ packages. The first benchmark showed medium tasks paying about 70 ms of Graphify planning despite Graphify never changing their 2-package plan.

Phase 10D therefore changed the hot-path rule:

- 1 package → skip Graphify
- 2 packages → skip Graphify and keep the Phase 10B plan
- 3–4 packages → allow Graphify dependency analysis

After tuning, medium Graphify planning dropped from **70 ms median to 0 ms** while the 2-package plan and correctness were preserved.

## Graphify behavior

For large tasks, Graphify is allowed to reduce package count only when a fresh graph contains strong **EXTRACTED** dependency evidence.

It may merge adjacent packages when either:

- there is a direct extracted dependency edge, or
- the two sides share at least two project dependency neighbors.

Graphify may only reduce package count. It never increases package count.

Missing, stale, scoped, unavailable, or failed Graphify analysis falls back to the Phase 10B safe-boundary plan.

## Decision

Keep the Phase 10D threshold tuning.

The resulting policy preserves Phase 9-like latency for small work, keeps medium work effectively cost-neutral, and spends about 90–100 ms of local runtime on Graphify only for large tasks where dependency-aware merging actually changes the plan.

This benchmark establishes packaging correctness and latency cost. It does **not** prove better code quality by itself: all deterministic benchmark variants were already correct. The demonstrated Phase 10 benefit here is improved task granularity and dependency-aware failure/recovery boundaries without adding outer ChatGPT↔Chadex calls.
