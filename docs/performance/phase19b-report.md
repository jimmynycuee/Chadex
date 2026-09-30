# Phase 19B — Long-Task Control-Plane Optimization

Date: 2026-09-30

## Status

Phase 19B is complete.

Phase 19B optimizes the long-task control plane without weakening execution evidence, timeout semantics, validation rigor, durable execution identity, or terminal failure identity. All final A/B validation uses isolated runtime bundles; the installed `/Applications/Chadex.app` remains on its restored pre-Phase-19B runtime.

Authoritative starting point:

- Phase 19A commit: `67356eedca691b3fa01ba2ba1c5e48090f988171`
- Primary benchmark: deterministic Phase 19A control-plane benchmark
- Browser-driven ChatGPT E2E remains secondary/host-sensitive evidence only.

## B1 — Model-facing observation projection

### Change

The canonical/internal job receipt remains complete. Only the model-facing nonterminal handoff projection is made sparser.

For a normal running handoff, Chadex now omits fields that carried no actionable information:

- `command_ok=false`
- `exit_code=null`
- `failure_kind=null`
- `tool_failure=false`
- empty stdout/stderr tails
- zero stdout/stderr line counters
- false truncation flags
- duplicate `recommended_poll_after_secs`

It keeps:

- `execution_state`
- `job_id` / `job_status`
- activity/progress
- timeout/sync-wait identity
- exact `observe_jobs` continuation
- bounded evidence whenever evidence is actually present.

### Isolated A/B result

Same forced-handoff workload, installed runtime vs Phase 19B candidate:

| Metric | Before | After | Delta |
| --- | ---: | ---: | ---: |
| Structured handoff output | 1186 B | 958 B | -228 B (-19.2%) |
| Raw MCP handoff response | 1372 B | 1144 B | -228 B (-16.6%) |
| Handoff median, 3 runs | 1029.01 ms | 1005.934 ms | -23.076 ms |
| Terminal response | 1240 B | 1240 B | unchanged |
| Terminal exit code | 0 | 0 | unchanged |

The latency difference is directional evidence only; the payload reduction is deterministic across repetitions and is the primary B1 result.

Focused validation:

- `job_handoff_model_projection_keeps_identity_and_exceptional_receipts`: passed.
- `key_tool_output_schemas_include_expected_fields`: passed.
- Canonical receipt preservation is explicitly covered by the handoff projection test.

## B2 — Event-driven initial long-process supervision

### Change

The initial hidden-job synchronous window previously queried RunnerRegistry every 25 ms until either terminal state or the sync deadline.

Phase 19B adds a narrow internal `wait_hidden_job_terminal_for_auth` API that reuses the existing per-job `Notify` primitive and the same double-check pattern already used by public job observation.

`await_hidden_structured_job` now waits on the job event rather than fixed-interval registry polling.

Unchanged:

- default sync window;
- effective command timeout;
- hidden-to-public job promotion rules;
- terminal state mapping;
- timeout taxonomy;
- validation behavior.

At the default 10-second sync window this removes up to roughly 400 internal 25 ms polling iterations for a silent long-running command.

### Validation

- `hidden_terminal_wait_wakes_on_terminal_update`: passed.
- `hidden_terminal_wait_returns_nonterminal_snapshot_at_deadline`: passed.
- Structured execution budget/state focused tests: 2/2 passed.

The existing timeout taxonomy (`timeout_with_progress` vs `timeout_stalled`) was audited and already implemented; Phase 19B does not duplicate it.

## B3 — Exact continuation + durable execution semantics

### Existing durable guarantees audited

Graphify + source inspection confirmed that Chadex already has the durable primitives Phase 19B needs:

- detached execution identity is deterministic from job/request/client identity;
- duplicate prepare reuses the same durable execution record;
- a mismatched execution/ownership identity fails closed;
- prepared delivery recovered after restart becomes `delivery_unknown` rather than blindly redispatching;
- exact message replay preserves the same message/deliveries and does not duplicate delivery.

Existing regression coverage includes:

- `duplicate_prepare_keeps_one_execution_identity`
- `mcp_app_restart_recovery_preserves_prepared_delivery_unknown_without_redispatch`
- `exact_message_replay_survives_endpoint_detach_without_duplicate_delivery`

These mechanisms were not rewritten in Phase 19B.

### New model-facing resume path

For a single nonterminal compact `observe_jobs` result, Chadex now emits an exact next continuation containing:

- the current job id;
- the latest observation token;
- the existing 20-second model-facing wait;
- `wake_on=terminal`.

Terminal and multi-job compact responses do not add a single-job continuation.

### Midflight A/B

A 4-second job is handed off, then intentionally observed with a 1-second wait so the first observation is still nonterminal.

| Metric | Before | After |
| --- | --- | --- |
| Exact continuation present | no | yes |
| Continuation tool | — | `observe_jobs` |
| Wait | — | 20 s |
| Wake policy | — | `terminal` |
| Final exit code | 0 | 0 |

The candidate midflight response is intentionally slightly larger because it carries the exact next action; this trades a small bounded payload for less model reconstruction/replanning and safer continuation.

Late-failure regression:

- forced handoff -> delayed exit 23;
- before exit code: 23;
- after exit code: 23.

Thus Phase 19B does not collapse an execution failure into a delivery failure or lose terminal failure identity.

## Benchmark calibration fix

The Phase 19A deterministic campaign previously ignored the handoff's exact continuation on the first observation and reconstructed a first observe call itself.

The campaign now consumes the exact continuation immediately.

This changes asynchronous benchmark scenarios from three MCP calls to two in the calibrated baseline.

This is a **benchmark correction**, not a Chadex runtime performance gain, and is excluded from B1/B2/B3 improvement claims.

## Isolated runtime validation

All final candidate validation uses an isolated app bundle:

`/tmp/Chadex-phase19b-final.app`

Candidate runtime identity is internally aligned across CLI/Server/Runner:

`0.2.3 / 67356eedca69 / dirty=true`

The installed Chadex app remains on its restored runtime identity:

`0.2.3 / d0edb8836cc0`

An isolated end-to-end local workflow completed successfully on both variants:

- before: 173.309 ms;
- candidate: 169.435 ms.

This is a one-sample smoke test and is not used as a performance claim.

## Reliability finding: runtime bundle identity

During early B1 dogfood work, replacing only the installed runtime-server caused Chadex to enter the UI recovery state because CLI/Server/Runner runtime identities no longer matched.

The installed app was restored from the pre-Phase-19B binary backup and verified with a real deterministic MCP workload.

Phase 19B then moved all candidate validation to isolated, identity-aligned runtime bundles. Future dogfood/release validation must preserve the whole runtime bundle identity; never hot-swap only one runtime entrypoint in the installed app.

## Validation summary

Passed:

- B1 handoff projection focused Rust test.
- B1 output-schema lifecycle focused Rust test.
- B2 hidden terminal event/deadline tests: 2/2.
- Structured execution budget/state focused tests: 2/2.
- Phase 19A control-plane Python tests: 8/8.
- Python compile checks for Phase 19A/B harnesses.
- Current-source aligned dogfood runtime build.
- Isolated local workflow smoke.
- Forced handoff payload A/B.
- Midflight exact-continuation A/B.
- Late-failure exit-23 A/B.
- `git diff --check` on Phase 19B code.

The repo-local Rust toolchain does not currently contain the `rustfmt` component, so `cargo fmt --check` was not available and no new toolchain component was installed during this phase.

## Phase 19B exit decision

Phase 19B passes.

The long-task control plane now has:

1. smaller nonterminal model-facing handoffs while preserving full canonical evidence;
2. event-driven initial hidden-job waiting instead of 25 ms polling;
3. exact model-facing resume instructions for a single nonterminal observed job;
4. preserved durable execution/idempotency semantics;
5. preserved terminal failure identity;
6. a calibrated deterministic benchmark that follows the runtime-provided continuation rather than inventing extra observation calls.

The next optimization phase can focus on harder disconnect/restart stress and real repository-marathon workloads without reopening these control-plane semantics.
