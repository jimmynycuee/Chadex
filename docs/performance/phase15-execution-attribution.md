# Phase 15 — Execution Attribution

## Boundary and result

Chadex Server receives tool requests; ChatGPT owns prompt submission, model requests, first token, tool selection, and final answer generation. The Server cannot timestamp those model boundaries. Phase 15 adds content-free events to the existing `ToolRequestLifecycle` trace and retains all unobserved outer wall as `unattributed`. It does not change tool routing, authorization, workspace isolation, recovery, or execution behavior.

The Round 3 official Chadex traces predate these events. Reprocessing them gives the following **baseline**, not a post-change connected rerun:

| Task | Total wall | First tool | Tool interval union | Runner command sum | Outside tool, unattributed | Tool gaps over 10 s |
|---|---:|---:|---:|---:|---:|---:|
| Small | 228.837 s | 27.513 s | 3.600 s | 0.456 s | 225.237 s | 7 |
| Medium | 319.243 s | 25.603 s | 3.034 s | 0.935 s | 316.209 s | 12 |
| Large | 390.398 s | 21.508 s | 5.384 s | 3.469 s | 385.014 s | 8 |

These observations do **not** show that the model used all outside-tool time. They show that tool intervals occupy 1.57%, 0.95%, and 1.38% of task wall respectively. The Medium ~85 s gap versus WebCodex remains unlocalized by the old trace. No claim about backend, reasoning, or connector share is justified from it.

## Instrumentation architecture

- `ToolRequestLifecycle` now stores its existing request/parse/dispatch/response events in both metadata and full modes through the existing bounded, asynchronous trace writer. `off` still writes none. Every event has a generated Server trace ID, a wall timestamp for joining, `process_elapsed_ns` from a process monotonic origin, and request lifecycle events additionally have `request_elapsed_ns` from the request's `Instant`. Durations **within** a Server process use monotonic clocks; wall timestamps are never subtracted across hosts.
- MCP markers cover handler ingress, local auth/validation and decoded argument byte size. API markers start at the handler because its authentication middleware runs earlier. No API auth duration is fabricated.
- The existing active trace task scope links Server→Runner enqueue to the authoritative Runner request ID. Registry poll emits a dispatch marker **after** it commits the lease; result acceptance emits the typed Runner reported duration. Dispatch-to-accept includes transport and waiting and is not labeled command time. Rejections, retries and missing Runner durations remain N/A.
- Workflow Session results emit `SHA256(session_id)` so separate authorized tool calls can be associated without storing the raw identifier. Coding startup emits concurrent context-probe duration and bounded instruction file count/content byte count, explicit resume lookup and ensure duration, startup preparation, output projection, and handoff construction/JSON byte size. A requested resume is not labeled as a successful restore.
- The existing native helper MCP ingress trace already records `server_trace_id` from the Server response header. `scripts/phase15_attribution.py --helper-traces` can join an exported array of these traces by exact ID. It never joins by timing or tool name. Native pre-backend, backend-headers, and response-stream figures are ingress envelopes, not connector-host or model time.
- Metadata mode stores event names, opaque IDs/hashes, tool/Runner kinds, byte counts, status, durations, and build information. It does not store prompts, source, arguments, or outputs. Existing **full** mode still supports payload capture by explicit configuration; use metadata for performance runs.

Missing coverage: there is no ChatGPT host callback for prompt receipt, model request start, first token, tool decision, tool-call emit, next model request, or final response. No Server event can legitimately fill these. Cross-tool association exists for a Workflow Session or through a host that supplies the outer execution trace; a generic stable execution ID is not supplied by ChatGPT. Native helper and Server can be joined by Server ID, but the hosted relay and connector host have no corresponding exposed stamps. Token counts, model cache status, and host context packaging are unavailable. Project manifest/cache rebuild state is not yet exposed as a cheap canonical signal, so it is N/A rather than inferred. The outer Round 3 traces have no new Server IDs, hence cannot be retroactively joined with Phase 15 Server requests.

## Dogfood and latency attribution

Six runs of the existing in-process HTTP TestClient exercised `list_tools`, `runtime_status`, and `tool_manifest` with metadata tracing. A coding fixture also exercised fresh `work_on_project`, explicit resume, and read-only handoff. The 29 raw traces (18 HTTP, eleven direct coding subphase traces) and machine-readable analysis are in `phase15-dogfood-traces/` and `phase15-dogfood-attribution.json`. Each HTTP request returned 200 and `success=true`; the coding calls succeeded. None captured a payload event. Median of the six HTTP samples per tool:

| API tool | Handler wall | Receive→dispatch | Dispatch envelope | Result projection envelope | Response handoff |
|---|---:|---:|---:|---:|---:|
| `runtime_status` | 2.304 ms | 0.140 ms | 0.603 ms | 1.366 ms | 0.171 ms |
| `tool_manifest` | 18.476 ms | 0.076 ms | 16.200 ms | 1.865 ms | 0.283 ms |
| `list_tools` | 322.607 ms | 0.160 ms | 46.550 ms | 218.254 ms | 42.195 ms |

These are handler envelopes and may include measurement work, audit work, and nested processing. `list_tools` returned an estimated 1,491,072-byte JSON response; the large projection envelope is not proof of model time. In the latest direct coding fixture, fresh startup took 153.826 ms, including 14.463 ms of concurrent context probes (one instruction file, 37 content bytes) and 0.414 ms of session ensure; the other 138.949 ms remains unclassified within startup. Explicit resume took 113.021 ms, including 0.044 ms lookup, 11.019 ms context probes (one file, 37 bytes), and 0.097 ms ensure; the other 101.861 ms remains unclassified. Handoff construction took 0.773 ms and produced 5,668 JSON bytes. These subphases are nested and must not be added to their enclosing startup duration. Direct fixture traces have no HTTP handler lifecycle, correctly marked `has_handler_lifecycle=false`.

These probes do not exercise a connected ChatGPT execution. The coding fixture services Runner requests locally, but its test registry does not export the production Runner telemetry callback. They validate emission, parsing, resume/handoff observation and normal responses; no new connected Small/Medium/Large execution was available in this phase. Thus the new trace can locate **Server-side** portions of future first-tool/tool-gap spans if a host trace is available, but cannot answer where the old 21–28 s first-tool delay or 10–60 s gaps actually occurred.

The existing Round 3 command sums of 0.456/0.935/3.469 s are Runner-reported and are diagnostic only. They are nested within tool intervals and must not be added to wall time. The new Runner queue and dispatch markers can distinguish lease wait from dispatch-to-result on a future connected run; without Runner callback timestamps, actual command execution is only the typed reported duration.

## Overhead and validation

An alternating off/metadata TestClient smoke comparison on the same source (32 pairs `runtime_status`, 16 pairs `list_tools`) before removing an avoidable metadata-mode `serde_json::to_value` clone measured: status 1.601→1.878 ms median (+0.277 ms), list 296.787→347.131 ms median (+50.344 ms). The clone was then restricted to existing full-payload mode; metadata now sizes the serializable response directly. The final source measured status 1.598→1.817 ms (+0.219 ms), list 289.104→322.470 ms (+33.366 ms). These figures are local debug-build smoke measurements, not stable connected latency estimates. Metadata tracing remains opt-in because large response measurement can be material.

Validation commands:

```sh
CARGO_HOME="$PWD/.toolchain/cargo" RUSTUP_HOME="$PWD/.toolchain/rustup" CARGO_NET_OFFLINE=true \
  ./.toolchain/cargo/bin/cargo test --manifest-path runtime-engine/Cargo.toml \
  -p chadex-runtime-engine tool_request_trace --lib

CHADEX_PHASE15_DOGFOOD_DIR="$PWD/docs/performance/phase15-dogfood-traces" \
  CARGO_HOME="$PWD/.toolchain/cargo" RUSTUP_HOME="$PWD/.toolchain/rustup" CARGO_NET_OFFLINE=true \
  ./.toolchain/cargo/bin/cargo test --manifest-path runtime-engine/Cargo.toml \
  -p chadex-runtime-engine phase15_ --lib -- --nocapture

PYTHONDONTWRITEBYTECODE=1 python3 -m unittest scripts/test_phase15_attribution.py

CARGO_HOME="$PWD/.toolchain/cargo" RUSTUP_HOME="$PWD/.toolchain/rustup" CARGO_NET_OFFLINE=true \
  ./.toolchain/cargo/bin/cargo test --manifest-path runtime-engine/Cargo.toml \
  -p chadex-runtime-runner-registry registry_enqueues_polls_and_completes_shell_request --lib

CARGO_HOME="$PWD/.toolchain/cargo" RUSTUP_HOME="$PWD/.toolchain/rustup" CARGO_NET_OFFLINE=true \
  ./.toolchain/cargo/bin/cargo build --manifest-path runtime-engine/Cargo.toml \
  -p chadex-runtime-engine --bin chadex-runtime-server

PYTHONDONTWRITEBYTECODE=1 python3 scripts/phase15_attribution.py \
  --server-trace-root docs/performance/phase15-dogfood-traces \
  --output docs/performance/phase15-dogfood-attribution.json
```

The Round 3 baseline is reproducible with three `--outer-trace` arguments pointing at the official Small, Medium, and Large Chadex `trace.jsonl`/manual trace paths listed in `phase15-round3-baseline.json`. The script preserves absent fields as null and does not invent model time. Full workspace `cargo fmt --check` currently reports formatting differences in unrelated dirty files; Phase 15 did not reformat them.

Final validation: 39 tool-request trace tests, three Phase 15 dogfood/overhead tests, one registry dispatch test, five attribution-script tests, three existing coding/resume/handoff behavior tests, and the Server build passed. Targeted `rustfmt --check` and `git diff --check` passed. The full workspace formatter check still reports pre-existing differences outside the Phase 15 files.

## Next phase priorities

1. Add host-side model request/first-token/tool emit/final response timestamps, if the host API exposes them, and propagate one opaque execution ID. Until then the largest Round 3 spans stay unattributed.
2. Export host connector dispatch/receipt and relay timestamps with the Server trace ID. This can split the known large outside-ingress gap without touching executor behavior or authorization.
3. On a new connected Medium dogfood run, join host, native helper, Server, and Runner by exact IDs. Compare first tool and the longest inter-tool gaps with WebCodex under the same host, then optimize only the measured boundary.
4. If Server startup dominates a measured connected span, inspect context-probe and session-ensure subphases plus response byte size. Defer small local executor optimizations until they explain a material share of user wall time.
