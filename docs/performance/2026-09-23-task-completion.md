# Task completion optimization — 2026-09-23

## Scope

The fixed five-step connected benchmark remains the regression control. This
work adds a separate comparison of five direct calls versus two inspection
calls followed by one deterministic `execute_task(edit -> validate -> review)`.
The task still uses ordinary permission/path checks, an owned detached
worktree, read guard in that worktree, durable task state, validation, review,
apply-back and cleanup. No source read revision is passed into the worktree:
revisions are scoped to one Project. Use direct guarded edits when source-read
freshness itself is required. The fixture has no concurrent writers and is not
evidence that these two stale-context contracts are interchangeable.

## Changes

- An adaptive 1 ms observation cadence for the first 25 ms of each bounded
  Runner Git subprocess reduces the scheduling floor paid by multiple short
  worktree probes. After 25 ms, the previous 10 ms cadence, deadline and tree
  termination logic remain in force.
- Worktree `snapshot_ms` now stops at snapshot completion. Previously it also
  included worktree creation; `package_workspace_component_sum_ms` counted that
  phase twice. The scheduler's recorded parallel overhead uses separate wall
  timers and is unchanged by this accounting fix.
- `execute_task` discovery describes when one ordered call can save external
  trips and when to retain direct guarded calls. The task result already
  includes validation, review, failure and workspace state, so routine status
  calls after a completed task are unnecessary.
- The MCP handler generates a response `x-chadex-trace-id` that matches its
  existing Server-to-Runner trace. Helper ingress diagnostics store only a
  validated UUID, bounded hashes of JSON-RPC IDs, and millisecond start/end
  timestamps. They do not record request bodies, raw string IDs or secrets.

## Measurements and decision boundary

`scripts/benchmark_task_completion.py` runs disposable repositories and
isolated local Server/Runner pairs, alternating arms after one warmup each.
It verifies the same exact diff and independent business-rule evaluator per
run. With `--reference-app`, both bundles are measured in the same run and
binary SHA-256 values are recorded. Fixture cloning, resets and evaluation are
outside the task timer; workspace preparation, execution, apply-back and
cleanup are inside. The five-call arm uses source `read_revision`; the fused
arm uses the executor's worktree revision. A failed or uncertain task aborts
the run and remains in the report rather than becoming a speed sample.

Initial five-pair local control on the earlier candidate bundle:

| Arm | Median | Outer calls | Correctness |
|---|---:|---:|---|
| Five direct calls | 185.649 ms | 5 | 5/5 correct |
| Two inspect + one task | 1045.723 ms | 3 | 5/5 correct |

The paired local break-even was about **430 ms of external overhead per saved
call**. This is a threshold for an experiment, not a predicted connected
speedup. Workspace timings include overlapping subphases (`validation_ms`
is inside `execution_ms`) and should not be added blindly.

## Same-run candidate A/B

The previous `Chadex-Performance-OutputSchemaCache.app` and new
`Chadex-TaskPerformance.app` were measured in one isolated run: 20 iterations
per arm, one warmup each, with reversed ordering every iteration. All **84/84**
workflow runs across four arms, including warmups, passed the evaluator and produced
the same diff SHA-256. Exact samples and binary hashes are in
`benchmarks/task-completion-ab.json`.

| Arm | Reference median / p95 | Candidate median / p95 |
|---|---:|---:|
| Five direct calls | 197.290 / 230.935 ms | 193.700 / 216.867 ms |
| Two inspect + one task | 1089.897 / 1240.387 ms | 1025.515 / 1127.749 ms |

For the fused task, the paired candidate-minus-reference median was **-62.431
ms** (about 5.7% of reference median). The measured local break-even dropped
from **448.265** to **416.922 ms per saved call**. The direct arm changed by
-4.469 ms paired median. The test is local, so none of these numbers include
host dispatch, tunnel, relay, approval or model time.

The new candidate returned a valid Server trace header on all **160/160**
measured direct-to-Server MCP calls; the reference bundle returned none. This
verifies the correlation field at the Server boundary, not through the
connected tunnel or host.

The unchanged fixed five-step harness also ran 20 alternating before/after
pairs, 40/40 evaluator successes with one identical diff SHA-256. Workflow
median was **194.453 -> 191.798 ms**, p95 **200.939 -> 197.457 ms**; exact
samples are in `benchmarks/task-performance-five-step-ab.json`. This shows no
local regression on that control, but says nothing about the earlier connected
`run_process` regression.

Focused checks passed: helper ingress/verification **15/15**, task executor
**72/72**, tool contracts **139/139**, Swift protocol models **16/16**, and
Runner real-process Git lifecycle **5/5**. A separate Server test confirmed
the trace header is generated per request and ignores caller-provided values.
The isolated App built successfully; plist lint, arm64 inspection and
unsandboxed `codesign --verify --deep --strict` passed. The restricted sandbox
could not read the Keychain trust chain and reported `CSSMERR_TP_NOT_TRUSTED`
for both old and new bundles, so signature verification was repeated with the
necessary system access.

The candidate must clear two separate gates: no meaningful regression in the
unchanged five-step workflow, and faster correct task completion under the
same connected host/model/approval conditions as WebCodex. Local loopback
evidence, a signed app and a ready tunnel do not prove ChatGPT Web latency.
The live connected runtime observed during this work still reported build
`72e7fb0decd4` / `git_dirty=false`; it did not contain this candidate.
