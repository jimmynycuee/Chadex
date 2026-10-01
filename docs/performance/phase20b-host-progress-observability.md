# Phase 20B — Host-progress observability after local terminal

Date: 2026-10-01

## Scope

Phase 20B targets the Phase 19C C5-R1 failure boundary: local durable work had already reached a correct terminal state, the second release validation had passed, and local Chadex reported no active relevant Jobs or processes, but ChatGPT did not dispatch the next expected action (`verify.py` / runner finish) for roughly 17m48s before the run was stopped.

This phase does **not** attempt to make Chadex autonomously resume a ChatGPT turn. The ordinary direct-tool flow has no safe host continuation binding. Existing Agent Continuation wake support is valid only for the MCP App continuation protocol after explicit host binding and acquire/prepare/finish fencing, so reusing it for ordinary tool calls would expand the state machine without proving that the host can accept the wake.

## Result

The runtime now exposes a correlated evidence chain for the boundary:

1. A meaningful MCP tool response that reports terminal execution is handed to the HTTP framework.
2. Chadex records `mcp_terminal_handoff_pending` with the Server-generated `server_trace_id` and `response_handed_at_ms`.
3. The completed meaningful call remains a bounded Window+principal continuity anchor. If the terminal result exposes Job identity, Chadex keeps only bounded `job_id` and optional `exit_code` evidence; this is diagnostic evidence, never retry authority.
4. Nonmeaningful diagnostics do not consume the pending terminal anchor, but the pending state is intentionally **not** projected as a global/model-facing `runtime_status` alarm: a normal conversation may legitimately end after a terminal response, so such a projection would create false-positive stall signals.
5. The first later meaningful request in the same Window+principal consumes that anchor and records `mcp_terminal_followup_observed` with the predecessor trace id and the exact `response_handed_at_ms -> next request observed` gap.
6. The existing helper already preserves the Server response header `x-chadex-trace-id` in `McpPerformanceTrace`, so Server events and helper ingress/response timing can be joined without adding another helper protocol field.

## Diagnostic semantics

`mcp_terminal_handoff_pending` means only that Chadex handed a terminal tool response to the local HTTP framework. It is **not proof that the remote host received the response**. `mcp_terminal_followup_observed` means a later meaningful request reached the same authenticated Window/principal and reports the measured local handoff-to-next-request gap. A normal conversation may simply end after the terminal result, so absence of a follow-up is not independently classified as a host failure. Neither event authorizes replay: recovery decisions still require durable Job/effect evidence and existing recovery semantics.

## Correlation and privacy

The new trace/log events contain only bounded correlation metadata:

- current Server trace id;
- predecessor Server trace id;
- response handoff timestamp;
- follow-up gap in milliseconds;
- bounded terminal Job id / exit-code evidence in the internal continuity anchor.

Raw OpenAI/MCP Window identifiers remain hashed and are not written into the trace. Request bodies, tool output bodies, credentials, stdout/stderr, and raw host identifiers are not added to these lifecycle events.

## Edge cases covered

- Nonmeaningful diagnostics such as `runtime_status` do not consume the pending terminal anchor, but do not expose it as an alarm.
- Overlapping meaningful requests invalidate ambiguous serial continuity instead of manufacturing a host gap.
- Principal/window isolation prevents another caller or chat Window from consuming or observing the anchor.
- Terminal detection handles both sparse model-facing `item.terminal` results and canonical mixed-batch `item.output.terminal` results.
- Terminal Job evidence is bounded, deduplicated, and optional; the correlation still works when no Job id is available.
- Existing `unknown_job -> list_jobs` recovery semantics remain canonical; the host-facing result-app route uses the Adaptive Runtime gateway when `list_jobs` is not directly exposed.

## Validation

Focused validation completed during Phase 20B:

- `tool_runtime::window_activity::tests::` — 15/15 passed.
- `mcp::tests::terminal_result_detector_covers_direct_and_observed_job_shapes_without_false_positives` — 1/1 passed after adding canonical mixed-batch coverage.
- `tool_request_trace::tests::metadata_lifecycle_persists_only_safe_boundaries` — 1/1 passed.
- `mcp::tests::result_app::mcp_job_presentation_tracks_real_running_to_terminal_transition` — 1/1 passed on the final routing contract.
- `mcp::tests::http_transport::terminal_response_handoff_correlates_to_next_meaningful_http_request` — 1/1 passed. This is the deterministic HTTP/MCP synthetic: a terminal `observe_jobs` response is handed off, a later meaningful request arrives in the same stateless OpenAI Window, and the follow-up trace records the predecessor trace id/gap without persisting the raw Window id.
- `git diff --check` on the Phase 20A/20B runtime paths — passed.

`cargo fmt --check` could not run because the repo-local Rust toolchain does not currently include the `rustfmt` component. No toolchain component was installed as part of this phase; compile/test validation and `git diff --check` were used instead.

## Phase boundary

Phase 20B makes C5-R1 diagnosable and safer to recover from, but it does not claim to eliminate an upstream ChatGPT host/model stall. Phase 20C must still perform the paired A/B campaign (synthetic long waits plus targeted C1/C2/C6 and paired C5 marathon) to determine whether Phase 20A's reduced re-entry count plus Phase 20B's evidence materially improves end-to-end latency/reliability.
