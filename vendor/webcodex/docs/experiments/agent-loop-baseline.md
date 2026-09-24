# Agent Loop Baseline Protocol

This experiment provides a repeatable measurement baseline for comparing a real
coding-agent run that uses Direct Tools with a run of the same task through Code
Mode. It measures facts already recorded by WebCodex. It does not infer model
reasoning, intent, or private chain-of-thought.

The deterministic [`scripts/eval_coding_loop.sh`](../../scripts/eval_coding_loop.sh)
harness remains responsible for runtime mechanics. This protocol is deliberately
separate: it measures real model/tool interaction against fixed task cases.

## Evidence contract

The profiler is [`scripts/agent_loop_report.py`](../../scripts/agent_loop_report.py).
It accepts two payload-safe evidence sources:

1. **ActionAudit SQLite** is the preferred source for outer tool calls. Select one
   run with an exact Workflow Session id. Existing rows provide the hashed
   `ClientWindow`, canonical meaningful classification, request-observed and
   response-handoff timestamps, serial/overlap classification, status, tool
   identity, and `model_ergonomics` metadata including serialized `ToolResult`
   bytes. Code Mode outer rows also persist the payload-safe
   `code_mode_composition` summary: nested call counts/tool distribution,
   consequential outcome counters, slot wait/internal duration, and nested/raw
   versus returned byte counts.
2. **Per-trace `events.jsonl`** is an optional supplement for observed Runner
   enqueue events. The profiler reads only JSONL metadata; it never opens captured
   request/result payload files.

On the current runtime, `WEBCODEX_TOOL_REQUEST_TRACE=true`/metadata mode does not
persist a per-trace `events.jsonl` tree. That is not a blocker for the core
baseline because ActionAudit already persists the payload-safe outer/Window facts.
If Runner enqueue observations are required, a capture made with full request
trace can be passed with `--trace-root`; the profiler still ignores all full
payload blobs. A missing trace root makes Runner-request metrics unavailable
rather than zero.

Trace persistence is fail-open observability, so `runner.requests_observed` means
exactly that: enqueue records observed in the supplied trace tree. The report does
not claim that the trace tree is a complete Runner-request ledger.

## Timing semantics

For a non-streaming outer call `i`, WebCodex-owned service time is:

```text
service_i = response_handed_at_i - request_observed_at_i
```

For two continuity-eligible, meaningful calls in the same hashed Window and
principal, a canonical serial transition permits:

```text
outside_webcodex_gap_i = request_observed_at_(i+1) - response_handed_at_i
```

The latter may include model inference, host scheduling, network delay, UI delay,
or user delay. It must not be named `model_think_time` or `reasoning_time`.
Non-meaningful calls do not consume the meaningful predecessor. When a report is
scoped to one Workflow Session or trace set, the profiler privately replays
same-Window/same-principal meaningful ActionAudit rows across the selected span so
an interleaved call outside the selection cannot be skipped over. Such context
rows never contribute call/tool/failure counts; if the real canonical predecessor
is outside the selection, that selected gap remains unavailable. Overlap is counted
separately and never converted into a negative gap. Streaming handoff does not
prove response completion. Continuity breaks remain missing evidence.

`observed_span_ms` is only the span covered by observed WebCodex outer-call
timestamps. It is not end-to-end task wall time unless some independent harness
provides explicit task start/end timestamps.

Percentiles use deterministic nearest-rank semantics. For metrics with partially
missing samples, `observed_total` and sample counts remain visible, while `total`
is `null`; missing evidence is never substituted with zero.

## Benchmark cases

[`scripts/agent_loop_cases.json`](../../scripts/agent_loop_cases.json) is the
authoritative case manifest. Each run starts from a fresh clean target at an
exact Git base revision. Direct and Code Mode runs must use the same case id and
base revision before their reports are considered case-compatible.

The four initial cases are:

- `readonly_review`: status/read/search/diff inspection with no workspace changes.
- `focused_edit_validation`: one focused edit, diff review, and successful
  `cargo check` using the existing coding-loop fixture recipe.
- `failed_validation_recovery`: a deliberately failing `cargo test`, diagnostic
  inspection, a fix, and a successful rerun.
- `long_validation_handoff`: `cargo test --lib` with `sync_wait_secs=1`, independent
  read-only work while a real same-execution Job runs, then terminal observation.
  If the command completes synchronously, that run does not satisfy this case;
  the agent must not fabricate a handoff or redispatch validation.

The fixture-oriented cases reuse the disposable Rust project recipe already owned
by `eval_coding_loop.sh`; this protocol does not create a second runtime harness.
Correctness and validation expectations remain case-level facts and should be
checked alongside the profiler output when judging whether two runs did equivalent
work.

## Capture and summarize a run

Record the exact Workflow Session id and 40-hex Git base revision for each real
run. Session selection is authoritative only through ActionAudit; trace-only input
cannot apply `--workflow-session-id`.

Start each benchmark run with its fresh `work_on_project` bootstrap; that call
links its own ActionAudit row through the canonical `WorkOnProject` relation. Once
the bootstrap returns the exact run Session id, **every subsequent model-facing
outer call in the run must pass that id as `recording_session_id`**. A business
`session_id` does not substitute for ActionAudit recorder provenance. Code Mode
calls that require a business Session should pass both fields with the same exact
benchmark Session id. Otherwise the Workflow Session ledger can contain the
nested work while `--workflow-session-id` selects no corresponding outer
ActionAudit row, producing an invalid measurement sample rather than proof of zero
calls.

The core report needs only the server's ActionAudit SQLite database:

```bash
python3 scripts/agent_loop_report.py summarize \
  --audit-db <server-sqlite-db> \
  --workflow-session-id <wc_sess_...> \
  --case-id focused_edit_validation \
  --variant direct \
  --surface direct \
  --base-revision <40-hex-base> \
  --output direct.json
```

When an `events.jsonl` trace tree was captured and Runner enqueue observations are
useful, add:

```text
--trace-root <tool-request-trace-root>
```

For a Code Mode benchmark run, use the same case id/base, `--variant code_mode`,
and the exact experimental surface: `--surface e1`, `e2a`, or `e2b`. Direct
benchmark runs use `--surface direct` (and the profiler also infers `direct` when
that argument is omitted). A Code Mode benchmark intentionally requires an
explicit surface so E1/E2a/E2b samples cannot be mixed under one generic label.
`--variant` can still be used without a benchmark case when profiling an ad-hoc
run; case metadata is only attached when `--case-id` is supplied.

For the initial E2b-M capture pilot, use `readonly_review` with E1,
`focused_edit_validation` with E2b plus the validation call outside the mutating
cell, and `long_validation_handoff` with E2a. The pilot is for evidence-shape and
capture validation first; repeated paired runs come only after these three reports
are complete and correctly selected.

## Reported metrics

The schema-v1 JSON summary reports, when evidence is available:

- outer model-facing tool calls: total, meaningful, success/failure, and tool-name
  distribution;
- Direct Tools canonical-call count from per-outer `model_ergonomics` records;
- Code Mode composition distributions from outer ActionAudit
  `code_mode_composition`, including nested call/success/failure counts,
  `nested_tool_counts`, consequential known/Job/unknown outcomes, internal/slot
  timing, and nested raw versus returned bytes;
- WebCodex service time and ToolRuntime duration distributions;
- canonical serial `outside_webcodex_gap` distributions and overlap count;
- exact serialized `ToolResult` byte totals/distributions;
- observed Runner enqueue count and request-kind distribution from trace JSONL;
- structured error/failure/recovery-guidance distributions.

The report also carries an explicit `availability` object. Consumers must inspect
it rather than assuming absent metrics are zero.

### Counting semantics and explicitly unavailable facts

Code Mode nested canonical child calls are now provable from the payload-safe
outer ActionAudit composition summary and are reported under
`composition.nested_calls` and `composition.nested_tool_counts`.
`canonical_calls.total` deliberately remains `null` for a `code_mode` report: that
older field keeps its outer/direct counting contract instead of silently combining
one parent invocation with its child invocations.

Some desired comparison facts remain unprovable from the current metadata
contract:

- **Generic same-execution Job handoff count and terminal-observation count.** A
  Runner `job_id` can exist before a command returns synchronously; its presence
  does not prove that the model received a Job handoff. `jobs.handoffs` and
  `jobs.terminal` remain `null` rather than using that unsafe proxy. For
  consequential children inside E2a, the authoritative parent receipt-derived
  count is separately available as `composition.job_handoffs`.
- **Resolved recovery count.** `recovery_kind` is guidance attached to one failed
  result. It does not itself prove that a later call resolved that failure.
- **Complete Runner-request total from trace files.** Trace persistence is
  observability, not execution authority, and has no completeness fence.

These gaps are candidates for a later small, generic metadata improvement if they
become necessary. The profiler does not change production runtime merely to fill
this table.

## Compare two reports

```bash
python3 scripts/agent_loop_report.py compare \
  --baseline direct.json \
  --candidate code-mode.json \
  --output comparison.json
```

JSON is the authoritative comparison shape. Each numeric entry carries baseline,
candidate, candidate-minus-baseline delta, and a `comparable` flag. Composition
comparisons include nested calls, consequential outcomes, Code Mode internal/slot
time, and nested/raw versus returned bytes; nested tool-name distributions are
reported alongside the outer/canonical tool distributions. A metric that is
unavailable on either side is emitted with `comparable: false` and an explicit
reason. The comparison is descriptive and does not select a winner or score.

`case_compatibility` is true only when both reports name the same benchmark case
and exact base revision. Equivalent final correctness/validation is still checked
against the manifest expectations; the profiler does not infer quality from call
counts or elapsed time.

## Privacy and non-inferences

The report never emits raw `ClientWindow` values, principal ids, trace ids,
commands, patches, file contents, credentials, or tool arguments. It does not
parse command bodies or payloads to guess task type. Full payload tracing is not
required for the core ActionAudit-derived metrics.

These measurements cannot establish model reasoning time, reasoning quality,
intent, causal attribution for outside-WebCodex gaps, or a general claim that Code
Mode is faster. They provide bounded observations for controlled Direct-vs-Code-
Mode experiments only.
