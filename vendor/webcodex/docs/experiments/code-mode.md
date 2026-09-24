# Experimental Code Mode — E1 Read-Only, E2a Effectful Foundation, and E2b Guarded Mutation

> This is an experiment, not a stable compatibility surface.

## Purpose

E1 tests one hypothesis: WebCodex can move bounded read-only orchestration below the model round-trip boundary while keeping every real Project operation inside the existing canonical `ToolRuntime`.

One model-visible `code_mode_exec` call evaluates a bounded JavaScript program. The program may invoke several explicitly admitted read-only WebCodex tools, use ordinary JavaScript control flow, overlap independent observations with `Promise.all`, make later calls conditional on earlier results, and emit only the useful aggregate with `text(value)`.

E1 does **not** add a second filesystem, shell, permission system, Project resolver, Session recorder, Runner protocol, or workflow engine.

## Feature flag

The experiment is disabled by default.

```bash
cargo check -p webcodex-code-mode
cargo test -p webcodex-code-mode --features v8-runtime
cargo check --features experimental-code-mode --all-targets
```

The root `experimental-code-mode` feature enables:

- the optional `webcodex-code-mode` dependency;
- `webcodex-code-mode/v8-runtime`;
- `webcodex-tool-contracts/experimental-code-mode`;
- `webcodex-tool-runtime-contracts/experimental-code-mode`.

Without that feature, `code_mode_exec`, `code_mode_exec_effectful`, and `code_mode_exec_mutating` are absent from the canonical `ToolDefinition`, `ToolSpec`, `ToolCall`, discovery, Adaptive Runtime, OpenAPI, and MCP surfaces. The default `webcodex-code-mode` crate contains only lightweight transport-neutral contracts and does not compile or link V8.

## Architecture

Dependency direction is intentionally narrow:

```text
model
  |
  v
code_mode_exec
  |
  v
root ToolRuntime resolves/authorizes exact Project + Workflow Session
  |
  v
V8CodeModeHost (thin frontend adapter)
  |
  v
CanonicalOrchestrationHost
  |  admission / server-owned argument injection
  |  child correlation / composition accounting
  |  exact auth + Project + Session + transport context
  v
ToolRuntime::call_tool_with_context(...)
  |
  v
canonical parsing / OAuth / Project authority
permission evaluation / Session evidence
Runner routing / ToolResult projection

webcodex-code-mode (V8 thread)
  |
  | tools.<name>(args) Promise
  +---- CodeModeHost callback ----> V8CodeModeHost
```

`webcodex-code-mode` does not depend on the root WebCodex crate, `ToolRuntime`, `AuthContext`, `RunnerRegistry`, or Session storage. It owns only one-shot JavaScript execution, JSON/V8 conversion, bounded output, nested-call scheduling, termination, and the transport-neutral `CodeModeHost` callback contract.

The root-side canonical callback implementation is intentionally no longer V8-specific. `CanonicalOrchestrationHost` owns the reusable authority-preserving nested-tool boundary; `V8CodeModeHost` only adapts the Code Mode crate's request/response types. Canonical target, recorder, context/ACK, result-expectation, and private wrapper fields are denied by the host itself; a frontend policy may add restrictions but cannot opt those Server-owned fields back in. This is an E1.x architectural probe, not a new workflow engine or stable extension API.

The V8 integration follows the minimal runtime/thread, Promise callback, microtask-checkpoint, JSON conversion, and thread-safe isolate termination patterns used by OpenAI Codex's Apache-2.0-licensed code-mode implementation. WebCodex E1 does not copy Codex's persistent cells, remote sessions, stored values, media, module ecosystem, notification protocol, or full Code Mode subsystem.

### E1.x frontend/host separation

The experiment now distinguishes the orchestration **frontend** from the canonical **host**:

```text
frontend program/runtime
    |      current: bounded V8 JavaScript
    |      possible later: tested TS composition package or structured plan
    v
CanonicalOrchestrationHost
    |
    v
canonical ToolRuntime
```

Only the V8 frontend exists today. The separation is meant to answer a narrower architectural question: can different orchestration representations share one authority, evidence, canonical dispatch, and composition-accounting boundary instead of each reimplementing WebCodex semantics? The frontend/runtime still owns program evaluation, scheduling/concurrency limits, timeout/cancellation, and output shaping. This does not add a TypeScript Composition Plugin, a Rust plan executor, bidirectional Native Plugin RPC, or another durable workflow lifecycle.

Native Tool Plugins remain capability providers. A future reusable TypeScript composition layer, if dogfood justifies one, should consume this same canonical host boundary rather than teaching the existing stdin/stdout Native Plugin protocol to call back recursively into ToolRuntime.

## JavaScript API

The global API is deliberately small:

```javascript
const status = await tools.git_status({});

const [files, hits] = await Promise.all([
  tools.read_files({
    items: [{ path: "src/lib.rs", start_line: 1, limit: 120 }]
  }),
  tools.search_project_texts({
    queries: [{
      pattern: "ToolRuntime",
      pattern_mode: "literal",
      result_mode: "files_with_matches",
      limit: 20
    }]
  })
]);

if (hits.success && shouldReadMore(hits.output)) {
  const detail = await tools.read_files({
    items: [{ path: "src/tool_runtime/kernel.rs", start_line: 1, limit: 80 }]
  });
  text({ status: status.output, detail: detail.output });
} else {
  text({ status: status.output, files: files.output, hits: hits.output });
}
```

`tools.<name>(args)` returns a Promise. An ordinary nested `ToolResult` is resolved as a JavaScript object with `success`, `output`, and optional `error`. This lets the program inspect expected business failures without turning every failure into an exception.

Host/protocol failures such as an unsupported nested tool, a forbidden target override, or failure to form a canonical ToolResult reject the Promise.

`text(value)` appends model-facing output. Strings are emitted directly; JSON-safe objects and arrays are compact-JSON encoded. Nested raw results are not automatically copied into the outer result.

Use `Promise.all` only for observations that are independent. Result-dependent follow-ups remain sequential JavaScript control flow. For one simple observation, call the ordinary tool directly; Code Mode is useful only when local orchestration removes meaningful model/tool round trips.

Code Mode should transform, select, filter, summarize, and aggregate nested observations. It should not merely concatenate complete raw `ToolResult` values. The 64 KiB output bound is intentional and is not a substitute for result selection. In particular, prefer:

```text
search -> choose relevant files/ranges -> read -> emit selected aggregate
```

over:

```text
search + many reads -> dump every raw result
```

This keeps Code Mode useful as a composition boundary rather than turning it into a larger transport envelope.

## Read-only allowlist

E1 admits only this explicit set:

```text
read_files
search_project_texts
project_overview
list_project_tracked_files
git_status
git_log
git_diff_hunks
git_review_summary
show_changes
```

Admission is not inferred from future tools. A canonical metadata regression test requires every admitted tool to remain `Observe`/read-only, non-shell-like, non-write-like, and free of mutation permission requirements.

`code_mode_exec` itself is not in the nested allowlist, so recursive Code Mode is impossible.

## Authority and Session model

The outer call requires `project`, `session_id`, and `source`.

The normal root `ToolRuntime` resolves and authorizes the outer Project and exact Workflow Session before starting V8. The root adapter receives the **resolved runtime Project id**, not merely the caller's shorthand.

JavaScript never selects its Project or Workflow Session. Nested arguments are rejected if they try to set server-owned target/recorder fields such as:

```text
project
session_id
recording_session_id
ack_session_context_revision
ack_session_message_ids
context_request
session_message_resolution
expected_failure
expected_failure_kind
result_expectation
accepted_exit_codes
assertion_name
```

The adapter injects the exact outer Project and Session and re-enters `ToolRuntime::call_tool_with_context` with the original caller authentication and transport. Canonical nested OAuth/scope checks remain enabled, including Session recording of scope denials where applicable.

Therefore nested calls continue to produce ordinary `tool_call_started` / `tool_call_finished` evidence in the same Workflow Session. E1 reduces model round trips; it does not collapse or hide canonical tool evidence.

The outer durable request audit records only bounded metadata such as Project, source byte count, and timeout. It does not persist the JavaScript source body. The durable result audit likewise retains only failure kind and orchestration stats; it does not copy emitted `content` or detailed runtime error text into Session evidence. Model-facing failures keep their detailed runtime message in the ordinary bounded ToolResult output while the durable error summary uses a fixed generic failure string. Raw request tracing follows the existing WebCodex trace policy; E1 does not introduce a separate secret or tracing system.

## No ambient host authority

The V8 isolate is not a Server shell and exposes no WebCodex host filesystem, network, process, database, environment, Runner socket, Node, or Deno API.

E1 removes or does not provide at least:

```text
console
Atomics
SharedArrayBuffer
WebAssembly
fetch
process
require
Deno
```

Dynamic imports are rejected. All meaningful Project observations must pass through `tools.<name>(args)` and therefore through the canonical root host adapter.

## Hard bounds

The current server-owned E1 limits are intentionally simple and bounded:

| Resource | Bound |
| --- | ---: |
| JavaScript source | 64 KiB UTF-8 |
| default wall clock | 5 s |
| hard wall-clock maximum | 30 s |
| nested tool calls | 32 |
| Code Mode executions concurrently active per Server process | 2 default; env-configurable 1..64 |
| nested calls concurrently in flight per execution | 8 |
| total `text()` output | 64 KiB UTF-8 |
| `text()` emissions | 256 |
| model-facing runtime failure detail | 16 KiB UTF-8 |

The V8 runtime runs on its own OS thread. A Tokio timeout is not treated as proof that CPU-bound JavaScript stopped. At the deadline, the async driver calls `v8::IsolateHandle::terminate_execution()`, signals the runtime thread, joins it, and returns a bounded timeout failure. A regression test covers `while (true) {}`.

E1 V8 execution is **Server-side**, not Runner-side. The process admits two simultaneously active Code Mode executions by default; `WEBCODEX_CODE_MODE_MAX_CONCURRENT_EXECUTIONS` may raise or lower this process-local limit within 1..64 for host-specific dogfood capacity. Waiting for a slot consumes the same wall-clock deadline. Nested Project observations still execute on the owning Runner through canonical ToolRuntime dispatch. Therefore dogfood requires a Server binary built with `--features experimental-code-mode`; existing compatible Runners do not need the feature or a protocol upgrade. Rebuilding a Runner from the same source commit is optional when exact source-alignment telemetry is desired.

## Outer result

A successful call returns only the emitted content plus small orchestration evidence:

```json
{
  "content": ["{\"status\":\"clean\"}"],
  "stats": {
    "tool_calls": 3,
    "max_in_flight": 2,
    "duration_ms": 14,
    "returned_bytes": 18
  }
}
```

The stats are experimental diagnostic evidence, not performance telemetry and not proof of model-level speedup.

## E1.5 composition observability

Phase 2 dogfood adds a diagnostic-only parent/child composition view without changing execution authority. The outer canonical `code_mode_exec` invocation keeps its existing logical invocation identity. Nested calls receive only a short-lived child ordinal for tracing and still re-enter `ToolRuntime::call_tool_with_context` as independent canonical invocations. The parent relation is not a Project, Workflow Session, ClientWindow, Job, retry, idempotency, permission, OAuth, or Runner-routing identity.

The ordinary model-facing result remains the sparse `content` + four-field `stats` shape above. Separately, RuntimeMetrics and the outer ActionAudit row may retain this bounded composition summary:

```text
nested_calls
nested_successes
nested_failures
max_in_flight
duration_ms
slot_wait_ms
returned_bytes
nested_raw_result_bytes_total
nested_tool_counts
```

`nested_raw_result_bytes_total` is the sum of serialized canonical child `ToolResult` sizes before JavaScript selection/projection. Together with `returned_bytes`, it gives a direct projection/compression ratio without retaining any nested payload. `slot_wait_ms` measures only time waiting for the process-wide V8 execution permit, so it can be separated from the remaining Code Mode interval. Tracing RuntimeMetrics exposes the same observations as `code_mode_nested_raw_result_bytes_total` and `code_mode_slot_wait_seconds`; the durable composition summary keeps the millisecond field above. `nested_tool_counts` is limited to the explicit admitted tool set. Composition telemetry never stores JavaScript source, nested arguments, nested outputs, paths, queries, commands, credentials, raw Window identity, or arbitrary nested error text. RuntimeMetrics remains fail-open: metrics failure cannot change the `ToolResult`.

Nested canonical calls deliberately use no fabricated `ClientWindow`. One host/model-visible `code_mode_exec` request therefore remains one meaningful outer Window call, while the Runtime Console can project the bounded child summary from that outer ActionAudit row. This lets operators distinguish WebCodex-owned outer service time, Code Mode internal time, and the following outside-WebCodex inter-call gap without reclassifying nested calls as host round trips.

## Validation evidence

The E1 tests are intended to prove runtime capability, not real-model throughput:

- one `code_mode_exec` can contain several canonical nested calls;
- a second nested call can depend on the first result;
- independent Promise calls can overlap, proven with an in-flight counter/barrier rather than a wall-clock threshold;
- CPU-bound JavaScript is hard-terminated;
- source/output/call-count/concurrency bounds are enforced;
- a real root `ToolRuntime` fixture binds nested reads to the resolved Project and exact Workflow Session;
- nested target overrides and effectful tools fail closed;
- Session ledger evidence remains visible for nested calls;
- only `text()` emissions are returned to the model by default.

Actual model round-trip reduction and task wall-clock improvement require live dogfood after this branch is reviewed; unit/schema tests cannot establish those claims.

## Phase 2 Direct Tools vs Code Mode dogfood protocol

Use a real review task twice against the same repository state and comparable model context. The **Direct Tools trace is the control**; the Code Mode trace is the treatment. Record at least:

| Evidence | Direct Tools | Code Mode | Interpretation |
| --- | --- | --- | --- |
| outer model-facing tool calls | count | count | primary round-trip surface |
| canonical tool invocations | count | count | child work should not disappear |
| nested tool invocations | 0 | count | composition work moved below the model boundary |
| WebCodex-owned outer duration | per call / total | per call / total | service time owned by WebCodex |
| Window inter-call gaps | bounded samples | bounded samples | outside-WebCodex gap, not reasoning time |
| Code Mode internal duration | n/a | per outer call | total Code Mode interval, including any slot wait |
| Code Mode slot wait | n/a | `slot_wait_ms` | process-wide V8 capacity contention |
| nested raw result bytes | n/a | `nested_raw_result_bytes_total` | canonical child payload before projection |
| returned model-facing bytes | total | total | transport/result pressure; compare with nested raw bytes |
| Runner requests | where currently provable | where currently provable | canonical backend work actually performed |
| task end-to-end wall time | observed | observed | user-visible completion interval |
| analysis/review findings quality | findings + evidence | findings + evidence | correctness/usefulness guardrail |

Canonical child calls are expected to remain visible as canonical runtime and Session evidence; Code Mode is successful only if it reduces useful **outer model/tool round trips** without degrading review quality. Do not claim that an outside-WebCodex Window gap is model reasoning time: it can include inference, network latency, host scheduling, UI work, or user interaction. Likewise, a synthetic V8 microbenchmark can characterize runtime overhead but cannot establish model-level speedup.

Prefer real ChatGPT dogfood traces over a bespoke benchmark runner while the existing telemetry is sufficient. If repeated real branch reviews do not show a meaningful round-trip, wall-time, or workflow-quality benefit, do not advance to effectful Code Mode merely because the local JavaScript runtime is fast.

## E2a — Effectful orchestration foundation

E2a adds a separate experimental entry point, `code_mode_exec_effectful`. It does **not** upgrade or widen `code_mode_exec`; E1 remains the read-only control surface with the same `Observe / Read / PureRead / project:read` contract and the same explicit read allowlist.

The E2a outer tool is a conservative consequential envelope (`Execute / JobRun / Standard / NonIdempotent / job:run`) and requires an explicit business Workflow Session. That envelope is not child authority. Every nested call still re-enters canonical `ToolRuntime` with the caller's exact authentication, resolved Project, Workflow Session, scope checks, permissions, Runner capability checks, validation semantics, Job lifecycle, and Session evidence.

E2a admits exactly the E1 read tools plus:

```text
cargo_check
cargo_test
```

It intentionally does not admit `cargo_fmt`, generic process/shell tools, `observe_jobs`, edit/write/delete/rename tools, Git mutation, Session mutation, plugins/MCP, Computer control, release/deploy tools, or either Code Mode entry point. **E2a does not add source mutation.** Final Changes / `present_changes` semantics therefore remain unrelated to E2a; guarded source mutation is an E2b question.

### Canonical composition policy and scheduling

Nested scheduling is owned by canonical `ToolDefinition`, not by JavaScript. `ToolCompositionPolicy` has only three states:

```text
Denied      default, including unknown/future tools
Sequential  cargo_check, cargo_test, apply_text_edits
Parallel    the exact E1 read allowlist
```

Frontend admission remains a separate explicit policy. A tool becoming `Parallel` never makes it automatically reachable from Code Mode, and orchestration metadata never grants scope, permission, Project, Runner, or retry authority.

`CanonicalOrchestrationHost` enforces the policy with one composition-local shared/exclusive fence. `Parallel` child invocation intervals may overlap. A `Sequential` child has exclusive access through the canonical ToolRuntime invocation. The lock ends when that invocation returns, including when structured validation returns its existing same-execution Job handoff; it does not remain held for the durable Job's lifetime. Consequently two predetermined validators can enter canonical dispatch sequentially, hand off as two ordinary Jobs, and later run concurrently under existing Job ownership.

For E2a only, validators whose canonical execution continuation is `observe_jobs` get a frontend handoff preference cap of five seconds: omitted values become 5, values above 5 are clamped, 1..5 are preserved, and invalid values such as 0 remain invalid for the canonical parser. `timeout_secs` is never shortened by this policy. E2a does not add a second Job lifecycle or restart a validation.

### Frontend termination and effect truth

E2a treats `timeout_ms` as the JavaScript/frontend decision deadline, not as a promise that the model-facing response is handed off at that exact millisecond. On frontend timeout or termination the host first closes a monotonic nested-call admission gate, then V8 is terminated and runtime-queued requests are discarded. A child still waiting for a Sequential fence sees the closed gate when it wakes and never crosses canonical dispatch. Already-started host work gets a separate bounded five-second reconciliation window; work that finishes in that window reaches a canonical result or same-execution Job handoff, while any still-stuck host task is cancelled and an already-dispatched consequential child remains conservatively `outcome_unknown`. Completed child results are not sent back into an already terminated isolate. The process-wide V8 execution permit is released when the frontend decision phase ends, before this post-frontend reconciliation.

E1 deliberately keeps its original return-at-frontend-deadline behavior and does not acquire this consequential drain burden.

### Effect receipt

Composition performance telemetry is not effect truth. E2a therefore has a separate sparse `effect_receipt`, emitted only when a consequential child actually crossed canonical dispatch:

```json
{
  "consequential_calls": 2,
  "known_results": 0,
  "job_handoffs": 2,
  "outcome_unknown": 0,
  "children": [
    {
      "ordinal": 1,
      "tool": "cargo_check",
      "outcome": "job_handoff",
      "job_id": "...",
      "continuation": {"tool": "observe_jobs", "arguments": {}}
    }
  ]
}
```

Consequential classification comes from canonical `ToolDefinition.effect != Observe`. A completed failing test is a `known_result`, not uncertainty. Parse/scope/admission rejection before dispatch is not an effect. A normal active validation Job preserves only the canonical `job_id` and parser-ready continuation; the receipt does not copy command text, argv, paths, stdout/stderr, raw ToolResult, validation payload, credentials, or secrets. The durable outer Action/Session audit retains only the four counters (plus ordinary bounded failure metadata), not child Job identities or continuation tokens.

If JavaScript throws or times out after consequential dispatch, the parent failure keeps the receipt and explicitly warns against blindly rerunning the whole JavaScript program. E2a provides no `retry_same`, rollback fiction, or whole-program retry authority.

### Session and Job continuation

Each nested validator records its own ordinary canonical `tool_call_started` / `tool_call_finished`, validation, permission/scope, and Job evidence in the exact outer Workflow Session. The parent is not a fake validation event and does not compress children into one transaction. Server-owned nested fields, including Project/Session selection, context ACK, Session-message resolution, result expectations, and private `__webcodex_*` fields, remain forbidden inside JavaScript.

Unlike re-observable E1, consequential E2a participates in normal Session continuity. After already-started children have drained, the outer response is decorated from the latest monotonic Session state; it never fabricates an ACK and never derives authority from `ClientWindow`.

`observe_jobs` remains intentionally outside nested E2a. A validator uses a short sync grace, may return its existing Job handoff, and Code Mode returns. The model then observes that exact Job through ordinary `observe_jobs`, including `wake_on=all_terminal` for a predetermined set when later work genuinely depends on all of them.

### E2b — Guarded structured mutation

E2b adds a third experimental entry point, `code_mode_exec_mutating`, without replacing E1 or E2a. Its outer contract is conservatively `Mutate / ProjectWrite / Standard / NonIdempotent / project:write` and requires an explicit business Workflow Session. That outer envelope grants no child authority and does not itself become edit provenance.

E2b admits exactly the E1 read set plus one existing canonical mutation primitive:

```text
apply_text_edits
```

It intentionally does **not** admit `cargo_check`, `cargo_test`, `cargo_fmt`, generic process/shell tools, `observe_jobs`, `apply_patch`, `write_project_file`, delete/Git/Session/Goal/Agent mutation, gateways, Computer control, deploy/release tools, or any Code Mode entry point. Validation therefore remains outside the mutation-capable cell in this phase. A normal workflow is `code_mode_exec_mutating` followed by ordinary canonical validation after the cell returns; E2b does not attempt workspace-snapshot fencing for background validation Jobs.

`apply_text_edits` is canonically `Sequential`, but composition eligibility remains independent from frontend admission: E1 and E2a still cannot call it. One E2b cell may attempt a canonical mutation at most once. The budget is classified from canonical `ToolEffect::Mutate`, counts failed/pre-start attempts as attempts, and rejects a second mutation before canonical business dispatch. `apply_text_edits` already supports transactional multi-file batches, so E2b does not add an in-cell mutation retry engine or a second patch protocol.

Across independent Code Mode cells, orchestration-originated mutation is serialized by a small process-local registry keyed by the canonical resolved Project id. The Project fence is shared by cloned `ToolRuntime` state and is held only through the canonical mutation `ToolRuntime` result. Different Projects retain independent mutation lanes. Read-only orchestration and E2a validation do not acquire this fence, and ordinary direct `apply_text_edits` intentionally remains outside it. This is coarse Code Mode containment, not a global WebCodex write lock or generic resource-lock framework.

Mutation effect receipts preserve canonical state-change truth. A known mutation result is `known_result` only when the canonical child returns an authoritative boolean `state_changed`; otherwise the receipt fails closed to `outcome_unknown` and omits the field. Pre-start results that prove the mutation never began are not retained as effects. No-op and dry-run edits can therefore be known with `state_changed=false`, while a completed write carries `state_changed=true`. Parent JavaScript failure or frontend timeout does not erase a completed mutation; the same bounded five-second post-frontend reconciliation used by E2a either learns the canonical result or leaves the dispatched mutation `outcome_unknown`. E2b never retries automatically.

Final Changes remains owned by canonical Session evidence. The outer E2b call has no generic top-level `state_changed` and does not itself set `repository_edit_observed`. A successful nested canonical `apply_text_edits` event with `state_changed=true` is the first-class `Edit` provenance; no-op, dry-run, pre-start failure, and outcome uncertainty do not become successful edit evidence. Once eligible, `present_changes` still freezes the complete Session Git baseline → final workspace tree, including later or otherwise independently produced workspace changes; it is not a Code Mode provenance diff.

### E2b live dogfood protocol

Live E2b dogfood must run in an isolated managed worktree rather than the source checkout. First verify the three model-facing manifests still expose the intended stage controls: E1 as read-only `Observe / Read / PureRead / project:read`, E2a as `Execute / JobRun / NonIdempotent / job:run`, and E2b as `Mutate / ProjectWrite / NonIdempotent / project:write`.

The minimum mutation cases are: one real `read_files -> read_revision -> apply_text_edits -> read_files` cell; a canonical no-op/dry-run with `state_changed=false`; a stale revision rejection that leaves newer workspace state intact; JavaScript failure after a successful write with a preserved `known_result/state_changed=true` receipt; two mutation calls proving only one crosses canonical dispatch; `finish_coding_task` / `present_changes` proving real Session edit eligibility and full baseline-to-final workspace presentation; and denials for shell, validation, alternate mutation primitives, and recursive Code Mode.

Capture both call economy and effect truth: outer model-facing calls, nested calls, canonical edit calls, Runner file-write requests, Code Mode duration, `slot_wait_ms`, nested raw result bytes, returned bytes, consequential-call counters, known results, outcome uncertainty, and mutation `state_changed`. The value hypothesis is specifically whether one adaptive E2b call can replace the direct sequence `read_files -> model decision -> apply_text_edits -> model decision -> read/show_changes` without weakening canonical authority or evidence. Do not infer generic mutation safety or model-level speedup from local runtime tests alone.

### Current stage sequence

```text
E1   read-only orchestration
E2a  structured validation + Job/effect foundation
E2b  guarded structured mutation: E1 reads + one apply_text_edits attempt
E2c  decide whether validation and mutation should coexist in one cell;
     consider selective process/shell only with telemetry
E3   Async Event Delivery
E4   product/stability decision
```

E2b validates only this narrow read → one canonical structured edit → bounded post-edit inspection loop. It is not evidence that generic mutation orchestration, multi-effect transactions, nested validation freshness, or process/shell composition is solved.

## Known limitations / non-goals

E1 intentionally has no:

- shell, process, edit/write, validation, or Job tools;
- automatic Job events or Async Event Delivery;
- persistent cells or variables across calls;
- filesystem/network/module APIs;
- Computer Use, MCP, Plugin, or Agent mutation calls;
- multi-Project or cross-Session orchestration;
- Runner-side Code Mode;
- Windows/macOS Code Mode packaging guarantee;
- stable compatibility promise.

The implemented stage sequence is documented above. Current E2b remains deliberately narrower than nested validation, multiple mutation attempts, generic shell/process orchestration, global/direct-write serialization, finer-than-Project mutation locking, nested Job waiting, Async Event Delivery, or a stable product commitment.
