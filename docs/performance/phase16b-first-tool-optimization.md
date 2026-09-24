# Phase 16B — First-Tool Surface Optimization

## Objective

Phase 16B implements the highest-priority controllable action from Phase 16A: reduce Chadex's initial model-facing tool surface and compact repeated MCP definition text without changing canonical runtime authority, validation, permissions, Runner behavior, Session semantics, or Host-required integrations.

This phase deliberately does **not** optimize the helper proxy. Phase 16A showed that the dominant first-tool gap exists before local Runtime Server ingress; Server dispatch and Runner startup are already millisecond-scale.

## Implementation

### 1. Bounded startup-direct policy

`runtime-engine/crates/chadex-runtime-tool-contracts/src/tool_policy.rs` now treats historical adaptive-direct ranks as ordering metadata, while effective startup admission is controlled by a bounded allowlist.

The retained direct set covers:

- ordinary coding loop: `work_on_project`, `runtime_status`, `tool_manifest`, search/read/edit/process/shell/job observation/change review/finish;
- Host/protocol primitives that cannot be faithfully reconstructed through the generic gateway: attachment import, `project_artifact`, stable Plugin gateway, Session handoff;
- descriptor-time MCP App bindings: Goal Plan, Work Result, Changes, Agent Continuation, and Agent Wait presentation entries;
- feature-gated Code Mode tools when compiled.

Other model-visible tools remain discoverable through `tool_manifest` and executable through `call_runtime_tool`. Their canonical ToolDefinition, scopes, permission gates, parsing, effects, and Runner requirements are unchanged.

### 2. Compact host projection

`runtime-engine/src/mcp/tools.rs` now compacts only the Host-facing projection:

- direct tools prefer their shorter `gpt_action_description` where available;
- repeated stateless Session/context wrapper descriptions are shortened at the final projection boundary;
- generic gateway wording is shorter;
- app-only state/continuation descriptors use concise Host-facing descriptions.

The canonical schemas and validation rules are not weakened.

### 3. Feature-preserving boundary

An early aggressive variant reached 17 non-UI tools / 67,621 bytes but removed descriptor-time Host behavior. It was rejected.

The final current-source 2026/stateless MCP projection is:

| Surface | Tools | Serialized tools bytes |
| --- | ---: | ---: |
| non-UI | 27 | 95,145 |
| UI-capable | 38 | 109,249 |

These counts include protocol/stateless extensions and the generic gateway, so they are not identical to Phase 16A's ChatGPT host-registry proxy measurement of 46 direct callables. They are directionally consistent, not an apples-to-apples registry count.

A legacy isolated `tools/list params={}` package probe still returns 22 tools / 53,665 response bytes; that probe exercises the legacy projection and must not be compared directly with the 2026/stateless counts above.

## Connected first-tool results

The same Phase 16A semantic boundary was reused: outer prompt submission to the first exact-`server_trace_id` local `mcp_tool_request_received`. The unobserved span remains **pre-Server host/relay/model-side unattributed**; it is not labeled model reasoning time.

| Run | Prompt → Server receive | First local handler |
| --- | ---: | ---: |
| Phase 16B Small r1 | 6,923.828 ms | 2.090 ms |
| Phase 16B Small r2 | 11,462.240 ms | 3.139 ms |
| Phase 16B Small r3 | 8,885.380 ms | 2.013 ms |

Phase 16B distribution:

- median: **8,885.380 ms**
- mean: **9,090.483 ms**
- best: **6,923.828 ms**
- worst: **11,462.240 ms**

Reference measurements from Phase 16A:

- matched Chadex: **15,432.869 ms**
- matched WebCodex: **6,936.979 ms**

Compared with the Phase 16A Chadex run, the Phase 16B median is **6,547.489 ms faster (42.4%)** and the mean is **6,342.386 ms faster (41.1%)**.

The best Phase 16B run is **13.151 ms faster** than the historical matched WebCodex reference, effectively parity at this measurement resolution. The Phase 16B median remains **1,948.401 ms slower** than that WebCodex reference, with substantial run-to-run variance.

## Attribution

### Confirmed

- Most of the previous fresh 8.496-second Chadex-vs-WebCodex first-tool gap can be removed without changing Server or Runner execution.
- The improvement is in the same pre-Server span identified by Phase 16A.
- First local Runtime Server handling remains approximately 2–3 ms in the Phase 16B runs.
- The final policy preserves Host-required App, attachment, Plugin, Session recovery, and native artifact behavior.
- The final packaged runtime CLI/server/runner binaries are byte-for-byte identical to the current dogfood build used by `scripts/build_app.sh`.

### Strongly supported

The Phase 16A hypothesis that initial Host-facing surface size materially contributes to Chadex first-tool latency is now supported by an intervention: reducing and compacting that surface coincided with roughly a 42% median reduction in prompt-to-first-Server time, while local execution stayed unchanged.

This is still not a controlled causal decomposition of the opaque Host/model/relay span; cache state, scheduling, model-side tool-selection work, and relay/connector-host variance remain unobservable.

### Not supported as the next bottleneck

The helper proxy does not appear to impose a necessary multi-second latency floor: one Phase 16B run reached essentially the historical WebCodex baseline without removing the helper hop. Exact helper contribution is still unresolved because an exact exported helper trace is unavailable.

Server dispatch, Runner startup, first response size, and gross backend catalog size remain poor optimization targets for first-tool latency.

## Validation

Current-source focused validation:

- `chadex-runtime-tool-contracts` full suite: **139 passed, 0 failed**
- current closeout set: **10 passed, 0 failed**
  - adaptive direct policy
  - seven model-surface tests
  - Goal Plan App descriptor-time binding
  - `project_artifact` native image framing
- Session handoff representative test: pass
- Plugin direct/gateway representative tests: pass
- runtime status and attachment `openai/fileParams` representative tests: pass
- final app build: pass, stable Apple Development signature
- final bundle: `dist/Chadex-Phase16B-Final.app`
- standard launch path updated safely after confirming the old bundle was not running: `dist/Chadex.app`

A broad MCP suite attempted during an earlier, intentionally over-aggressive intermediate surface produced routing-assumption failures and later timed out in a long artifact-export test. Those results predate the final feature-preserving direct set and are not evidence against the current source. The current focused regression set was rerun after the final source changes and is green.

## Packaging

The production wrapper manifest `chadex-runtime/Cargo.toml` path-depends on `../runtime-engine`. `scripts/build_app.sh` builds that wrapper into `runtime-engine/target/dogfood`.

For the final package, these bundle binaries exactly match the current dogfood outputs:

- `chadex-runtime-cli`: `a770a28a2ef3590b00eefdf0cfec51d5028623b2515af8f0eb3658eca2f0fbf3`
- `chadex-runtime-server`: `6855e66dd29e36e131bf8c6f067a009c3ae9f0985f99a2c13ffe312e6fef6b2f`
- `chadex-runtime-runner`: `03eaa81084cc36c0283920c97562b14034b0fd4e71f033d16feebcd1facd8f0d`

## Conclusion

Phase 16B succeeds at its intended target: Chadex's first-tool latency is no longer consistently separated from WebCodex by the 7–17 seconds seen in Round 3. The median fresh result improved by about 42%, and the best run reached WebCodex-level first-tool latency.

The remaining issue is **variance in the opaque pre-Server span**, not local Runtime execution.

A next phase should therefore prioritize measuring and stabilizing that pre-Server variance—Host surface/cache state, connector-host/relay timing, and any remaining definition-load variability—before attempting Server/Runner/helper micro-optimizations. Further surface pruning should be feature-preserving and evidence-driven.

Phase 16B stops here; no next-phase optimization is implemented in this phase.
