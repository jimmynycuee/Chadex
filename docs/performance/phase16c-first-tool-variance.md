# Phase 16C — First-Tool Variance Attribution

## Objective

Phase 16C follows the Phase 16B surface reduction. Phase 16B showed that Chadex can reach WebCodex-level first-tool latency in its best run, but its prompt-to-Server boundary still varied from roughly 6.9 s to 11.5 s.

The Phase 16C goal is therefore not another speculative Server/Runner rewrite. It is to split the remaining pre-Server span precisely enough to determine whether variance is before Chadex local ingress, inside the helper proxy, or after helper dispatch but before Runtime Server receive. Optimization is permitted only after repeated matched evidence identifies a controllable component.

## Instrumentation

### 1. Exact helper-ingress timing projected into the existing Server trace

rust-helper/src/chadex_core/verification.rs now stamps each proxied Runtime Server request with two diagnostics-only numeric headers:

- helper ingress start, as Unix nanoseconds;
- helper ingress-to-backend-dispatch preparation time, as microseconds.

Incoming caller copies of those reserved headers are stripped before forwarding and replaced by helper-owned values. They are never used for authorization, routing, Session identity, permissions, or execution.

runtime-engine/src/mcp.rs accepts only bounded numeric helper timing values and attaches them to the existing ToolRequestLifecycle before mcp_tool_request_received is written.

runtime-engine/src/tool_request_trace.rs stores the two numeric values in the same metadata record that already owns the generated server_trace_id. No timestamp-nearest matching is introduced.

This removes Phase 16A/16B's GUI-export dependency for helper timing. A single exact Server trace can now expose helper ingress -> helper backend dispatch -> Runtime Server receive while retaining the Server-generated trace ID as the join authority.

### 2. Exact-ID first-tool profiler decomposition

scripts/phase16a_first_tool_profile.py remains the canonical exact-ID profiler and is backward compatible with older helper exports.

For new instrumented runs it additionally reports:

- prompt_to_helper_ingress_ms
- helper_ingress_to_server_received_ms
- helper_pre_backend_ms
- helper_backend_send_to_server_received_ms
- server_total_to_handler_return_ms

The first span remains labeled host/relay/model-side unattributed. It must never be renamed reasoning time.

### 3. Matched-run variance analyzer

scripts/phase16c_variance_profile.py consumes exact-ID profile JSONs through an explicit manifest and computes:

- n / mean / median / min / max / range;
- p95 / Q1 / Q3 / IQR / sample standard deviation;
- Chadex vs WebCodex paired deltas;
- order-position groups;
- cold/warm connector groups;
- cold/warm registry groups;
- fresh/reused chat groups;
- Chadex attribution-span distributions.

The analyzer never discovers request identity itself. Every profile must already contain an exact server_trace_id.

## Connected benchmark protocol

Use a balanced, interleaved matched design instead of always running Chadex first.

Minimum useful campaign:

- 10 matched pairs / 20 total runs;
- 5 pairs Chadex -> WebCodex;
- 5 pairs WebCodex -> Chadex;
- pair ordering randomized before execution;
- identical prompt, model effort, Temporary Chat state, benchmark repo revision/tree, and approval conditions;
- each run records explicit order_position, connector_state, registry_state, and chat_state;
- new Temporary Chat for every run;
- exact prompt submission timestamp from the existing ChatGPT browser driver;
- first local request selected only by the existing exact structured marker + exact client_window_key / server_trace_id rules.

Illustrative manifest entry shape:

    {
      "run_id": "pair01-chadex",
      "harness": "chadex",
      "pair_id": "pair01",
      "order_position": 1,
      "connector_state": "warm",
      "registry_state": "warm",
      "chat_state": "fresh",
      "profile": "profiles/pair01-chadex.json"
    }

Cold and warm labels must describe an actual controlled condition, not be inferred from latency. Unknown state is recorded as unknown.

## Interpretation rules

1. Compare medians, IQR/p95, and paired deltas; do not choose conclusions from the single best run.
2. Treat order effects as descriptive until both AB and BA have repeated samples.
3. If prompt -> helper ingress varies while helper -> Server stays stable, the remaining variance is upstream of the local proxy.
4. If helper ingress -> Server receive varies materially, split helper preparation from helper backend-send-to-Server before changing helper architecture.
5. Server handling after `mcp_tool_request_received` is outside the prompt-to-first-Server boundary. Do not use post-ingress handler time to explain first-tool latency.
6. Do not prune the Phase 16B direct tool surface again unless the connected distribution still supports surface/registry load as a controllable bottleneck.

## Final connected campaign

The canonical Phase 16C result is `docs/performance/phase16c-campaign-final-combined/`.

The final dataset contains **10 matched pairs / 20 valid runs** with **5 Chadex -> WebCodex** pairs and **5 WebCodex -> Chadex** pairs. Every included turn used a fresh Temporary Chat, Extra High was verified before submission, each run had one globally unique structured marker, and request identity was joined only by the exact `work_on_project.instruction` marker plus `client_window_key` / `server_trace_id`. No nearest-time request matching was used.

The fixed browser identity was the same for every included run:

- launcher PID: **38777**
- CDP endpoint: **http://127.0.0.1:50065**
- descriptor: `~/.codex-chatgpt-web/runtime/launcher-browser.json`

The browser driver no longer auto-starts Codex Web GPT. If the descriptor disappears, becomes unhealthy, or its PID/endpoint changes, the campaign fails closed.

The final dataset combines:

- the first eight complete matched pairs from `phase16c-campaign-final-pinned-v2`;
- both complete matched pairs from `phase16c-campaign-final-pinned-replacements`.

Only the canonical combined manifest, exact-ID profiles, and variance output are checkpointed. The two raw source-campaign directories remain local diagnostic artifacts so failed/pre-submit attempts, browser state, and machine-specific launcher metadata are not published.

The incomplete pinned-v2 pair09 was excluded because its WebCodex prompt was submitted but no exact tool trace materialized within 90 seconds. Pinned-v2 pair10 was not executed after that campaign stopped. No valid slow sample was manually removed.

Earlier v1-v7 campaigns are diagnostic only. They exposed browser-owner, duplicate-campaign, selector, blocking-modal, and resume/checkpoint problems and are excluded from the final statistics.

### Prompt -> Runtime Server

| Metric | Chadex | WebCodex |
| --- | ---: | ---: |
| n | 10 | 10 |
| median | **7.763 s** | **9.076 s** |
| mean | **7.747 s** | **9.469 s** |
| p95 | **9.665 s** | **11.764 s** |
| IQR | **1.121 s** | **1.126 s** |
| min | 6.360 s | 8.110 s |
| max | 10.351 s | 12.075 s |

The paired Chadex-minus-WebCodex delta has a **median of -1.540 s** and mean of **-1.722 s**. Chadex is faster in **8 of 10 matched pairs**. Of the two pairs where WebCodex is faster, one differs by only **6.3 ms**, effectively parity at this measurement scale.

Relative to the WebCodex median, the Chadex median is about **14.5% lower** in this sample. Phase 16B's surface-reduction improvement therefore persists under a larger balanced connected campaign.

### Chadex path decomposition

For the ten Chadex runs:

| Span | median | p95 | max |
| --- | ---: | ---: | ---: |
| prompt -> helper ingress | **7.761 s** | 9.665 s | 10.350 s |
| helper ingress -> Server receive | **0.870 ms** | 3.646 ms | 4.571 ms |
| helper pre-backend work | **0.092 ms** | 0.223 ms | 0.231 ms |
| helper backend-send -> Server receive | **0.766 ms** | 3.490 ms | 4.357 ms |

The helper proxy is not a meaningful source of the remaining multi-second first-tool variance. Comparing medians, essentially the entire prompt-to-Server span already exists before helper ingress. The remaining span must remain labeled **pre-Server host/relay/model-side unattributed**; Phase 16C does not have evidence to rename it model or reasoning time.

`work_on_project` Server handling occurs after first Runtime Server ingress and is therefore outside the prompt-to-first-Server boundary. It must not be used to explain first-tool latency.

### Order effect

The final dataset remains exactly balanced at **5 first / 5 second** positions for each harness.

Descriptively:

- Chadex position 1: median **7.285 s**, mean **7.917 s**.
- Chadex position 2: median **7.778 s**, mean **7.577 s**.
- WebCodex position 1: median **9.479 s**, mean **10.218 s**.
- WebCodex position 2: median **8.561 s**, mean **8.720 s**.

There is no single simple first-vs-second effect for Chadex: its median and mean move in opposite directions because of a small sample and outliers. WebCodex is descriptively slower when first. This is enough to justify keeping future comparisons balanced or randomized, but not enough to assign a causal cold/warm mechanism.

Connector and registry states remain recorded as `unknown`, so Phase 16C makes no causal cold-vs-warm claim.

## Benchmark-harness hardening

Phase 16C also hardened the benchmark infrastructure:

- `selectConnector()` now uses semantic ranking, multiple selector families, and post-selection verification.
- Known blocking ChatGPT rate-limit modals are detected, dismissed, and critical clicks retry safely.
- The driver no longer calls `browser.close()` on a launcher-owned browser attached through `connectOverCDP()`.
- **Per-run launcher auto-start has been removed.**
- Campaign startup pins one launcher PID + CDP endpoint in `launcher-identity.json`.
- Every later run must use that same launcher identity; identity changes fail closed.
- Launcher discovery still performs bounded health retry, but never starts or replaces the browser.
- Durable submit/commit checkpoints prevent submitted prompts from being replayed after process interruption.
- Output-directory and global campaign locks prevent concurrent benchmark owners.
- Pre-submit retries are permitted only while no prompt timestamp exists.

## Final validation

- Phase 16C Python/tooling regression suite: **22 passed, 0 failed**.
- JavaScript driver syntax validation: passed.
- helper reserved-header ownership test: passed.
- Server trace metadata timing test: passed.
- targeted Rust timing validations: **2 passed, 0 failed**.
- fixed-browser final dataset: **20 valid runs / 10 matched pairs**.
- balanced order: **5 Chadex-first / 5 WebCodex-first**.
- Chadex helper timing present on **10/10 Chadex profiles**.
- both final source campaigns record the same launcher PID **38777** and endpoint **127.0.0.1:50065**.
- repository-wide `cargo fmt --check` remains unsuitable as a Phase 16C gate because unrelated pre-existing dirty Rust files differ from rustfmt.

## Decision

Phase 16C does **not** justify another Server/Runner/helper first-tool optimization pass.

The local helper span is sub-millisecond at median and only **4.571 ms** at the observed maximum, while the remaining multi-second variance is already present before helper ingress. The same broad pre-Server variability is also visible in WebCodex.

Under the final fixed-browser, balanced campaign, Chadex has a lower median, mean, and p95 than WebCodex, with Chadex faster in 8/10 matched pairs. The immediate Phase16A first-tool disadvantage has therefore been removed without a helper or Runtime Server rewrite.

A future Phase 16D should only reopen first-tool latency work if new observability exposes a specific controllable component inside the currently unattributed host/relay/model-side span. Otherwise, first-tool optimization should stop here. A separate phase may target `work_on_project` tool-completion latency, because that is a different post-ingress metric.
