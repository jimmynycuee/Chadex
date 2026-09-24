# Phase 16A — Chadex vs WebCodex First-Tool Differential Profiling

## Scope

Phase 16A is diagnosis only. No cache, fast path, batching, planner, Graphify, execution, or routing optimization was implemented. The only behavioral changes are profiling/benchmark-driver compatibility changes needed for the current ChatGPT UI and early prompt-submission timestamp capture.

The narrow question is: **why does WebCodex reach its first actual tool sooner than Chadex?**

## Executive result

The differential is real and reproducible, and its **location is confirmed**:

- Round 3: WebCodex reached first tool 7.054–17.135 s before Chadex across Small/Medium/Large.
- Fresh paired Small: Chadex first Server ingress was **15.433 s** after prompt submission; WebCodex was **6.937 s**. Differential: **8.496 s**.
- Both fresh first tools were `runtime_status`; neither used the Runner.
- Chadex first Server request took **1.741 ms** receive→handler return; WebCodex took **54.076 ms**. WebCodex was already ~8.5 s ahead before Server execution.

The dominant span is therefore:

`prompt submitted -> first MCP request received by local Runtime Server`

That span remains **unattributed host/relay/model-side time**. Current interfaces do not expose prompt receipt, model request start, first token, tool decision, tool-call emit, or relay dispatch timestamps. Phase 16A does not rename residual time as model/reasoning time.

The strongest Chadex-controllable structural difference in that phase is the initial direct connector surface:

- Chadex: **46** direct ChatGPT callables, **163,429** characters of observed callable-definition text.
- WebCodex: **21** direct callables, **68,230** characters.
- Chadex exposes ~**2.19x** the direct tools and ~**2.40x** the definition text.

This makes direct tool/context surface the highest-priority Phase 16B candidate, but causality is not proven because host packaging, tokenization, cache state, and model tool-decision timing remain unavailable.

## Comparable request paths

### Chadex

`ChatGPT host -> OpenAI Secure MCP relay -> tunnel-client -> chadex-helper McpIngress proxy -> Chadex Runtime Server /mcp -> ToolRuntime -> Runner (when needed)`

### WebCodex

`ChatGPT host -> OpenAI Secure MCP relay -> tunnel-client -> WebCodex Runtime Server /mcp -> ToolRuntime -> Runner (when needed)`

WebCodex has a confirmed shorter local path because it lacks the Chadex helper ingress proxy. Existing Phase 15 measurements put helper/Server-local work in the millisecond to low-hundreds-of-milliseconds range, so that extra hop cannot by itself explain a multi-second gap.

Both products use the exact same OpenAI tunnel-client binary:

- version: `0.0.12+881c9a8fed7cccbe6607cd419863bbca506b8215`
- SHA-256: `b1757220cf4722cec9085ee4a908cf0ee4c1a499a33bd99979b9a9c7669e29b1`

A faster tunnel-client build/version in WebCodex is ruled out.

## Round 3 reproduction

| Task | Chadex prompt→first | WebCodex prompt→first | Chadex − WebCodex | Chadex first tool | WebCodex first tool |
|---|---:|---:|---:|---|---|
| Small | 27.513 s | 10.378 s | **17.135 s** | `runtime_status` 33 ms | `runtime_status` 26 ms |
| Medium | 25.603 s | 9.327 s | **16.277 s** | `runtime_status` 44 ms | `runtime_status` 30 ms |
| Large | 21.508 s | 14.453 s | **7.054 s** | `runtime_status` 40 ms | `tool_manifest` 41 ms |

Round 3 outer first-tool IDs can be exact-joined to the matching run-private raw Runtime Server trace directories, so Phase 16A also reconstructs prompt→Server-ingress and Server-handler timing without guessing by timestamp or tool name. What cannot be reconstructed retroactively is the Chadex helper-ingress envelope, because the historical Round 3 artifacts did not export the helper performance ring buffer.

Machine artifacts: `docs/performance/phase16a-round3-differential.json` (outer summary) and `docs/performance/phase16a-first-tool-profile.json` (exact `server_trace_id` Server decomposition).

## Fresh paired Small run

Both arms used the Round 3 Small task, Git HEAD `51336aa88fca2e0a601be90077158ada3ee9065c`, Git tree `376110e3ecb095b1babcf6bfa0a35da8cfdeb9eb`, Temporary Chat, Extra High / `極高`, and the Python 3.12 benchmark contract. Prompts were the same 36 lines with only required workspace/connector substitutions: 1,760 B Chadex and 1,769 B WebCodex probe.

The WebCodex diagnostic probe was intentionally stopped once the first-tool boundary was captured; full task completion is outside the metric.

### Exact-ID selection and join

`scripts/phase16a_first_tool_profile.py` never chooses requests by nearest timestamp or tool name:

1. Outer prompt submission defines the run time floor.
2. A target window is admitted only when a full-trace argument contains a string scalar **exactly equal** to the expected Project id/path; substrings in shell commands do not count.
3. That exact `client_window_key` selects the window.
4. The first request is selected inside that exact window.
5. Receive/parse/dispatch/response boundaries join by exact `server_trace_id`.
6. Optional helper traces join only by the same exact `server_trace_id`.
7. Ambiguous exact-marker windows fail closed.

### Fresh boundary decomposition

| Boundary | Chadex | WebCodex | Differential |
|---|---:|---:|---:|
| Prompt submit → Server receive | **15,432.869 ms** | **6,936.979 ms** | **+8,495.890 ms Chadex** |
| Server receive → parse | 0.200 ms | 0.307 ms | -0.107 ms |
| Parse → dispatch start | 0.148 ms | 0.173 ms | -0.025 ms |
| Dispatch | 1.128 ms | 53.437 ms | **WebCodex +52.309 ms** |
| Server receive → handler return | **1.741 ms** | **54.076 ms** | **WebCodex +52.335 ms** |
| First-tool response size | 3,787 B | 3,849 B | +62 B WebCodex |
| Runner requests in first tool | 0 | 0 | equal |

Exact first request identities:

- Chadex: `client_window_key=6caade9a8f22f677607da0150d40d4534f66011d6878fa18a0f4043c4a6f5ec2`, `server_trace_id=1290541e-f4be-47f0-801b-dce81ca6a2f9`.
- WebCodex: `client_window_key=95412562368419606b63754c5b4955f17b003e44f831c64eb04b45e3c123f641`, `server_trace_id=e8889057-b756-470f-9eec-cab5f7cc8ba5`.

Machine artifact: `docs/performance/phase16a-connected-small-pair.json`.

## Initial tool/context surface

Two surfaces must not be conflated.

### Current ChatGPT direct callable surface

| | Chadex | WebCodex |
|---|---:|---:|
| Direct callables | **46** | **21** |
| Callable-definition text | **163,429 chars** | **68,230 chars** |
| Chadex / WebCodex | **2.19x tools** | **2.40x text** |

Twenty direct tools are common. Chadex additionally exposes 26 direct callables across memory, skills, presentation/goal/agent continuation, shell/detached execution, artifacts, and diagnostics. WebCodex has one direct callable absent from the Chadex snapshot, `apply_patch`.

The character count is an observed host callable-definition text proxy, **not** hidden prompt bytes, model token count, cache state, or reasoning complexity.

### Canonical backend runtime catalog

| | Chadex | WebCodex |
|---|---:|---:|
| Canonical runtime tools | 129 | 135 |
| Full serialized `list_tools` result | 1,490,294 chars | 1,462,136 chars |

The full backend catalog differs by only ~1.9% in serialized size. Therefore "Chadex is slower because its total backend catalog is much larger" is ruled out. The important structural difference is **what is directly exposed to ChatGPT initially**.

Machine artifact: `docs/performance/phase16a-tool-surface.json`.

## Connector / relay / helper observability

### Server

Fully observed and exact-joined by `server_trace_id` for the fresh pair.

### Runner

Fully observable when a tool enqueues Runner work. Fresh `runtime_status` enqueued no Runner request in either product, so Runner latency cannot contribute to this first-tool differential.

### Chadex helper ingress

Phase 15 propagates Server response `x-chadex-trace-id` into `McpPerformanceTrace.server_trace_id`, so helper→Server can be exact-joined when the helper trace array is exported.

For the fresh run, the running app retained those traces only in its bounded helper ring buffer exposed by the GUI Diagnostics export. macOS accessibility denied automated access to the running Chadex app, so no safe automated fresh helper export was available. The fresh helper share is therefore **unavailable**, not inferred.

### OpenAI tunnel / relay

Both tunnel logs expose `request_id` / `rpc_request_id`, but current Server lifecycle events do not receive those tunnel identifiers. There is no exact-ID bridge tunnel log → Server trace. Phase 16A deliberately does not use nearest-time matching, so relay/connector-host time remains unattributed.

Machine artifact: `docs/performance/phase16a-relay-observability.json`.

## Cause classification

### Confirmed

1. **The first-tool differential occurs before the local Runtime Server receives the first request.** Fresh differential: +8.496 s Chadex; Round 3: +7.054 to +17.135 s.
2. **First-tool Server execution is not the cause.** Fresh Chadex Server handler was 1.741 ms; WebCodex was 54.076 ms.
3. **Runner execution is not the cause.** Neither fresh first tool used Runner.
4. **Coding startup/session preparation is not the cause of prompt→first-tool.** `work_on_project` occurs after first `runtime_status`; Phase 15 also measured coding startup at only ~0.1–0.15 s locally.
5. **WebCodex has a shorter local tunnel path** because it has no Chadex helper ingress proxy.
6. **The tunnel-client itself is identical** in version and binary hash.
7. **Chadex's directly exposed ChatGPT tool surface is much larger**, while the full backend catalog is roughly the same size.
8. **Fresh first-tool response payload size is essentially the same** (3,787 vs 3,849 B).

### Strongly supported

1. **Initial direct tool/context surface is the highest-priority Chadex-controllable contributor.** Chadex exposes 2.19x as many direct callables and ~2.40x as much observed callable-definition text before the first decision. The differential is located in the phase where this surface can matter: before first Server ingress.
2. **The extra helper proxy is a real but small contributor, not the primary multi-second cause.** Existing Phase 15 helper/local measurements are orders of magnitude below 8.5 s. A fresh exact helper envelope was unavailable, so its exact fresh-run contribution is not claimed.

### Unresolved

1. How the pre-Server residual divides among ChatGPT host context packaging, model inference/tool selection, model/cache state, host scheduling, connector host, and OpenAI relay.
2. Whether the larger direct tool surface causes most, some, or only a small fraction of the fresh 8.5 s differential. There is no host tool-decision timestamp or token/cache telemetry to prove causality.
3. The exact fresh Chadex helper-ingress envelope, because its exact-ID trace was not programmatically exportable from the running app.
4. Any relay/connector-host RTT share before local ingress, because tunnel IDs are not propagated into Server tracing.

### Ruled out / negligible for the observed multi-second gap

1. First `runtime_status` Server execution.
2. Runner command execution before first tool.
3. `work_on_project` coding startup/session ensure.
4. First-tool response payload size.
5. A different/faster tunnel-client binary or version in WebCodex.
6. Gross canonical backend tool-catalog size.
7. Prompt-size mismatch (fresh prompts differ by only 9 bytes and required connector/path text).

## Direct answers

### Why is WebCodex first-tool faster?

**Proven:** WebCodex gets the first request to its local Runtime Server sooner. In the fresh Small pair, it reached Server ingress 8.496 s before Chadex. The advantage exists before Server/Runner execution.

**Best-supported explanation:** Chadex presents a substantially larger initial direct callable/context surface to ChatGPT, while WebCodex keeps a much smaller initial direct surface. This is the largest measured Chadex-controllable difference in the relevant pre-Server phase. It is not possible to prove how much of the 8.496 s is model/tool-selection versus other host/relay work.

### Where does Chadex's extra latency appear?

Almost entirely in `prompt submitted -> first local Server request received`. It does **not** appear in Server dispatch, Runner execution, or coding startup for the first tool.

### Which original suspects are excluded?

Server execution, Runner execution, coding startup/session preparation, first-tool response size, tunnel-client version/binary, full backend catalog size, and prompt-size mismatch are excluded as explanations for a 7–17 s gap. The helper proxy remains a confirmed extra hop but is too small in known measurements to explain the whole gap.

### What is important about the initial surface?

The total backend catalogs are comparable, but Chadex's current directly exposed ChatGPT surface is dramatically larger: 46 vs 21 callables and ~163k vs ~68k definition-text characters. This is the most important Phase 16A finding for a Chadex-controlled Phase 16B.

## Phase 16B priorities — recommendation only, not implemented

1. **Reduce Chadex's initial direct callable surface to a WebCodex-like Adaptive Runtime surface.** Keep high-frequency coding primitives direct; move memory/skill/presentation/agent-lifecycle/diagnostic/specialized tools behind discovery or `call_runtime_tool` unless the host explicitly needs them loaded.
2. **Reduce host-facing definition/schema verbosity for remaining direct tools.** Measure direct callable bytes/tokens after surface reduction and remove redundant metadata/instructions while preserving contracts and safety semantics.
3. **Only after 1–2, instrument or shorten the helper proxy path if an exact helper export proves material.** Current evidence says it is lower order; removing it first is unlikely to recover an 8.5 s gap.

Phase 16A stops here. No Phase 16B optimization is included.

## Artifacts and validation

Checkpointed artifacts:

- `docs/performance/phase16a-round3-differential.json`
- `docs/performance/phase16a-fresh-pair-attempt.json`
- `docs/performance/phase16a-request-path.json`
- `docs/performance/phase16a-tool-surface.json`
- `docs/performance/phase16a-relay-observability.json`
- `scripts/phase16a_first_tool_profile.py`
- `scripts/test_phase16a_first_tool_profile.py`
- `scripts/phase16a_round3_profile.py`
- `scripts/test_phase16a_round3_profile.py`

Raw connected profiles, browser-driver captures, and copied exact-trace manifests remain local diagnostic artifacts because they contain machine-specific paths or connector/device identifiers. Their aggregate measurements and exact-ID methodology are preserved in the checkpointed report and machine-readable summaries above.

Benchmark-driver compatibility changes:

- `scripts/webcodex_chatgpt_driver.mjs`
- `../agent-harness-benchmark/round3/telemetry/chatgpt_ui_driver.mjs`
- `../agent-harness-benchmark/round3/telemetry/run_chatgpt_ui.py`

Validation completed during Phase 16A:

- Fresh connected-run profiler unit tests: 3/3 passed.
- Historical Round 3 exact-ID profiler unit tests: 2/2 passed.
- `phase16a_round3_profile.py` reproduces `phase16a-first-tool-profile.json` byte-for-byte.
- Fresh Chadex/WebCodex first-tool profiles reproduce from exact structured markers and exact IDs; WebCodex uses the recorded analysis cutoff to exclude requests appended after the first-tool recorder stopped.
- Chadex fresh Small fixture after agent execution: 7 visible tests passed.
- WebCodex connector-picker preflight: Temporary Chat + WebCodex + Extra High verified without prompt submission.
- Exact first-tool trace artifacts copied and SHA-256 manifested.
- Same tunnel-client version and SHA-256 verified on both live products.
- Fresh paired first-tool boundary reproduced the Round 3 direction with an 8.496 s Chadex disadvantage.
