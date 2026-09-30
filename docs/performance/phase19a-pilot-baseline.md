# Phase 19A Pilot Baseline

Date: 2026-09-30
Source baseline: `55851ed960b424c18f5556a447a9757aa27025f0`

## Baseline status

Phase 19A's authoritative baseline is the deterministic local Chadex control-plane campaign. It does not automate ChatGPT.

Raw artifacts are stored outside the source repo under:

`~/Documents/ChatGPT/agent-harness-benchmark/phase19/`

The formal pilot below is `control-plane-pilot` with `scale=0.1`.

All P1–P5 scenarios passed.

## Deterministic control-plane pilot

| Scenario | Status | Total | MCP calls | MCP wire time | Response bytes | Peak response | Job handoff | Exit |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- | ---: |
| P1 silent runner | passed | 12.262 s | 3 | 12.090 s | 3,248 | 1,360 | yes | 0 |
| P2 bursty 500 KB | passed | 6.258 s | 3 | 6.089 s | 3,736 | 1,381 | yes | 0 |
| P3 1 MB log avalanche | passed | 0.327 s | 1 | 0.113 s | 12,263 | 12,263 | no | 0 |
| P4 late failure | passed | 18.177 s | 3 | 18.067 s | 3,286 | 1,366 | yes | 23 |
| P5 multi-stage late fail | passed | 13.708 s | 3 | 13.595 s | 3,503 | 1,384 | yes | 24 |

P1, P2, P4, and P5 exercised real async job handoff and terminal observation. P3 completed inside the synchronous window.

### Heartbeat / continuity

Every collected heartbeat sample was successful:

- P1: 22/22
- P2: 11/11
- P3: 1/1
- P4: 33/33
- P5: 24/24

Across all five scenarios:

- heartbeat sample errors: 0
- disconnect transitions: 0
- reconnect transitions: 0
- final app/helper/runtime-server/runtime-runner/tunnel layers: healthy
- P4 expected terminal exit: 23
- P5 expected terminal exit: 24

### Payload behavior

The payload scenarios already demonstrate bounded model-facing observations:

- P2 generated 500 KB bursty output, but its three MCP responses totaled only 3,736 bytes; stdout was reported as truncated.
- P3 generated a 1 MB log avalanche, while the returned MCP response was 12,263 bytes; stdout was reported as truncated.

This is not permission to discard execution evidence. Phase 19B should preserve complete local execution evidence while optimizing what is exposed to the model.

## Browser E2E development runs — NOT baseline eligible

Earlier Phase 19A development used an automated ChatGPT browser driver. Those runs were useful for identifying host-side gaps, but the automation itself changed the measurement environment and repeatedly coincided with ChatGPT extended-processing/review states.

Representative development evidence:

| Scenario | Total | Prompt → first helper | Runner RTT | Observation → next action |
| --- | ---: | ---: | ---: | --- |
| P1 silent | 29.278 s | 8.938 s | 69.859 ms | 8.765 s |
| P2 bursty 500 KB | 109.075 s | 10.337 s | 22.829 ms | P50 20.148 s, max 23.982 s |
| P3 1 MB log avalanche | 71.982 s | 15.952 s | 17.592 ms | P50 15.853 s, max 27.303 s |

Heartbeat remained healthy during these runs, so the visible host-side delay was not equivalent to a dead local Runner. Because the browser harness required repeated Temporary Chat navigation, connector selection, DOM monitoring, and structured automated prompts, these numbers are retained only as **harness-development / host-sensitive evidence**.

They must not be used as Chadex-vs-WebCodex performance baseline data unless a future neutrality check demonstrates that automated and normal user-driven host behavior are equivalent.

The E2E development traces still provide useful evidence-separation measurements:

| Scenario | Full trace payload | Compressed trace payload | Model-facing response |
| --- | ---: | ---: | ---: |
| P1 silent | 25,689 B | 14,222 B | 2,249 B |
| P2 bursty 500 KB | 49,365 B | 22,174 B | 24,759 B |
| P3 1 MB log avalanche | 108,053 B | 17,150 B | 22,391 B |

For P3, one `runner_job_update` carried 62,449 B of raw evidence while the persisted zstd payload was 421 B. Phase 19A therefore keeps execution evidence and model-facing observation as separate measurement surfaces.

## Harness issues found and corrected

Phase 19A exposed and corrected measurement-harness problems:

- current ChatGPT composer/user/assistant/Temporary Chat DOM compatibility;
- single-owner browser locking to prevent concurrent campaigns;
- browser automation now requires explicit `--allow-browser-automation`;
- credential-free heartbeat sampling;
- traced macOS launch via `launchctl setenv` + `open -a`;
- deterministic project resolution from Chadex's local project registry;
- Python 3.9-compatible registry parsing;
- compact synchronous `run_process` success normalization without masking expected failures;
- full trace correlation through Chadex Server trace IDs.

## Phase 19B priorities implied by this baseline

1. Preserve full execution evidence while minimizing model-facing observation cost.
2. Improve long-job supervision and terminal delivery without high-frequency polling.
3. Keep execution state separate from observation-delivery state.
4. Add durable execution identity, resume, and idempotency for reconnect/restart cases.
5. Reduce unnecessary agent/tool round trips and repeated validation.
6. Treat host-side next-action delay separately from Chadex local execution latency.

Phase 19A is sufficient to begin Phase 19B: the primary baseline is reproducible without browser automation, long-job outcomes are observable, payload behavior is measurable, and connection health is recorded independently.
