# Phase 19B — Long-Task Control-Plane Optimization

Date: 2026-09-30

## Status

Phase 19B is complete.

Phase 19B optimizes the long-task control plane without weakening execution evidence, timeout semantics, validation rigor, durable execution identity, or terminal failure identity. Authoritative before/after A/B validation used isolated runtime bundles. After that validation passed, the identity-aligned CLI/Server/Runner dogfood bundle was deployed to `/Applications/Chadex.app` as a unit and verified live.

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
| Handoff median, 3 runs | 1024.486 ms | 1018.115 ms | -6.371 ms |
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

The candidate midflight response is intentionally slightly larger because it carries the exact next action: structured output increased from 1,203 B to 1,394 B (+191 B), and the raw MCP response increased from 1,389 B to 1,580 B (+191 B). This trades a small bounded payload for less model reconstruction/replanning and safer continuation.

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

### Apples-to-apples calibrated control rerun

To separate the benchmark-calibration effect from the runtime patch, the updated campaign was run against the restored pre-Phase-19B installed runtime and the isolated Phase 19B candidate using the same P1/P2/P4/P5 scenario definitions at `scale=0.1`.

| Scenario | Installed calls | Candidate calls | Installed response bytes | Candidate response bytes | Delta | Exit preserved |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| P1 silent runner | 2 | 2 | 2,693 | 2,465 | **-228 B** | 0 -> 0 |
| P2 bursty 500 KB | 2 | 2 | 2,680 | 2,452 | **-228 B** | 0 -> 0 |
| P4 late failure | 2 | 2 | 2,729 | 2,501 | **-228 B** | 23 -> 23 |
| P5 multi-stage late fail | 2 | 2 | 2,945 | 2,717 | **-228 B** | 24 -> 24 |

This cleanly separates the two effects:

- **3 -> 2 MCP calls** comes from the corrected harness following the already-provided exact first continuation;
- **-228 B per async scenario** comes from the Phase 19B runtime handoff projection and exactly matches the isolated B1 A/B.

Installed-runtime artifact root:

`~/Documents/ChatGPT/agent-harness-benchmark/phase19/phase19b-installed-control-plane-1790759000`

Candidate artifact root:

`~/Documents/ChatGPT/agent-harness-benchmark/phase19/phase19b-candidate-control-plane-1790758598`

The restored pre-Phase-19B installed-runtime control rerun had fully healthy live heartbeat samples: 0 sample errors, 0 disconnects, 0 reconnects, and final app/helper/runtime-server/runtime-runner/tunnel layers all ready or connected.

The candidate rerun intentionally launched the helper/runtime in an isolated data directory and therefore had no tunnel-session health URL. Its heartbeat samples reported `no tunnel health-url found`; those samples are not used as candidate continuity evidence. Candidate correctness is established by the isolated workflow, forced-handoff, midflight-continuation and terminal-result tests instead. Wall-clock timing between the live installed run and isolated candidate run is not treated as a performance comparison.

## Isolated runtime validation

All final candidate validation uses an isolated app bundle:

`/tmp/Chadex-phase19b-final.app`

Candidate runtime identity is internally aligned across CLI/Server/Runner:

`0.2.3 / 67356eedca69 / dirty=true`

The apples-to-apples **before** runtime used for comparison was the restored installed runtime:

`0.2.3 / d0edb8836cc0`

After the Phase 19B source/evidence review completed, the already-validated identity-aligned CLI/Server/Runner bundle was deployed to `/Applications/Chadex.app` **as a unit**. The installed dogfood runtime now reports:

`0.2.3 / 67356eedca69 / dirty=true`

This dogfood provenance reflects that the runtime bundle was built from the Phase 19B working tree before the source changes were committed as `357cf30`. It contains the same Phase 19B runtime code; a future release build should be rebuilt from a clean final commit for release provenance.

An isolated end-to-end local workflow completed successfully on both variants:

- before: 173.309 ms;
- candidate: 169.435 ms.

This is a one-sample smoke test and is not used as a performance claim.

## Reliability finding: runtime bundle identity

During early B1 dogfood work, replacing only the installed runtime-server caused Chadex to enter the UI recovery state because CLI/Server/Runner runtime identities no longer matched.

The installed app was restored from the pre-Phase-19B binary backup and verified with a real deterministic MCP workload.

Phase 19B then moved all candidate validation to isolated, identity-aligned runtime bundles. Only after that isolated bundle passed was the full CLI/Server/Runner trio deployed to the installed app together. Future dogfood/release validation must preserve the whole runtime bundle identity; never hot-swap only one runtime entrypoint in the installed app.

Post-deployment live validation:

- runtime identity: `0.2.3 / 67356eedca69 / dirty=true` across CLI/Server/Runner;
- B1 live probe returned the sparse Phase 19B handoff shape (redundant false/null/empty fields absent);
- B3 live midflight probe returned an exact `observe_jobs` continuation with the latest token, `wait_secs=20`, and `wake_on=terminal`, then reached terminal exit 0;
- final live P1 deterministic control: passed in 2 MCP calls, 2,465 response bytes, 12.232 s total, exit 0;
- final live P1 heartbeat: 22/22 samples healthy, 0 sample errors, 0 disconnects, 0 reconnects;
- app/helper/runtime-server/runtime-runner/tunnel layers: ready or connected;
- rollback bundle: `../Chadex-backups/pre-phase19-20260930-1313/installed/phase19b-runtime-bundle/`;
- final live P1 evidence: `~/Documents/ChatGPT/agent-harness-benchmark/phase19/phase19b-live-final-p1/`.

## Validation summary

Passed:

- B1 handoff projection focused Rust test.
- B1 output-schema lifecycle focused Rust test.
- B2 hidden terminal event/deadline tests: 2/2.
- Structured execution budget/state focused tests: 2/2.
- Phase 19A control-plane Python tests: 8/8.
- Calibrated P1/P2/P4/P5 installed-runtime control rerun: 4/4 passed with healthy live heartbeat.
- Calibrated P1/P2/P4/P5 isolated candidate rerun: 4/4 passed with expected exits preserved.
- Python compile checks for Phase 19A/B harnesses.
- Current-source aligned dogfood runtime build.
- Isolated local workflow smoke.
- Forced handoff payload A/B.
- Midflight exact-continuation A/B.
- Late-failure exit-23 A/B.
- Post-deployment live heartbeat and deterministic P1 smoke.
- `git diff --check` on Phase 19B code.

The repo-local Rust toolchain does not currently contain the `rustfmt` component, so `cargo fmt --check` was not available and no new toolchain component was installed during this phase.

## Development orchestration finding

During Phase 19B validation, cancelled/timed-out Cargo test jobs occasionally left `cargo`/`rustc` descendants running after the tracked WebCodex job had already become terminal. These orphan compiler processes consumed CPU and disk and can make the surrounding ChatGPT turn appear stalled even while the Chadex control plane itself is healthy.

The orphan processes were explicitly identified and terminated before final validation. This is treated as a development-tool/job-orchestration issue, not as a Chadex runtime regression.

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
