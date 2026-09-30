# Phase 19A — Long-Haul Observability Baseline

## Purpose

Phase 19A establishes measurement before Phase 19B changes runtime behavior. The goal is to explain where long-task time and failures occur instead of reducing everything to one total-wall-time number.

The implementation reuses Chadex's existing request tracing rather than adding a competing telemetry stack. Existing traces provide wall-clock timestamps, process-relative durations, Server trace IDs, Runner request/job correlation, serialized/model-facing response sizes, and full-trace payload metadata.

## Pre-Phase-19 rollback point

A verified rollback point was created before Phase 19 work:

- Source HEAD: `55851ed960b424c18f5556a447a9757aa27025f0`
- Repository copy: `../Chadex-backups/pre-phase19-20260930-1313/repo`
- Backup branch: `backup/pre-phase19-20260930-1313`
- Git bundle: `../Chadex-backups/pre-phase19-20260930-1313/pre-phase19.bundle`
- The pre-existing dirty `graphify-out/GRAPH_REPORT.md` was preserved separately and is not part of Phase 19A changes.

## Measurement architecture

Phase 19A uses two deliberately separated measurement surfaces.

### 1. Deterministic control-plane baseline

`scripts/phase19a_control_plane_campaign.py` talks directly to Chadex's loopback MCP endpoint. It resolves the Chadex project from the local project registry by absolute path, so it does not change the GUI-selected project.

This surface measures:

- MCP wire latency and response bytes;
- Server request trace breakdown;
- Runner/job handoff;
- long-process terminal delivery via `observe_jobs`;
- output truncation/bounding;
- expected failure exits;
- independent connection-layer heartbeat.

It never automates ChatGPT and is the authoritative Phase 19A baseline.

### 2. Optional ChatGPT E2E sanity

The browser E2E profiler can additionally observe:

1. prompt submission;
2. helper ingress;
3. Server tool-request receipt;
4. Server dispatch start/finish;
5. Runner enqueue/dispatch/result;
6. Server handler return;
7. next tool request;
8. final visible/confirmed response.

`observation_to_next_server_action` is intentionally an end-to-end interval. It must not be described as pure model inference time.

Browser automation is explicitly opt-in because Phase 19A development showed that repeated automated Temporary Chat/connector interactions can change host behavior and trigger extended-processing/review states. Those development runs are not baseline-eligible.

## Heartbeat

`scripts/phase19a_heartbeat.py` samples out of band and does not call ChatGPT tools.

It is credential-free and records:

- Chadex app process;
- helper process;
- runtime server;
- runtime runner;
- tunnel-client process;
- tunnel-client loopback health;
- main-channel server transport probe;
- control-plane tunnel state.

The campaign aggregates samples, errors, transitions, disconnects, reconnects, and final layer state.

## Artifacts

Primary control-plane runs generate:

- `manifest.json`
- `summary.md`
- per-run `metrics.json`
- per-run `timeline.json`
- per-run `heartbeat.jsonl`

Trace metadata is correlated through Chadex's `x-chadex-trace-id` and full request trace directory. Secrets and raw credential values are never written into benchmark artifacts.

Optional E2E profiling additionally generates its own timeline/events/metrics/summary artifacts.

## Pilot workload

`scripts/phase19a_pilot_workload.py` provides deterministic workloads:

- silent runner;
- bursty 500 KB output;
- 1 MB log avalanche;
- late expected failure;
- multi-stage late failure.

P6 repository marathon is intentionally reserved for a dedicated engineering fixture.

## Phase 19A exit criteria

Phase 19A is complete when:

- deterministic runs can reconstruct Server/Runner/job timing without ChatGPT UI automation;
- long jobs can be distinguished from host/browser lifetime failures;
- payload size and bounded model-facing output can be measured;
- connection health can be sampled independently of task execution;
- expected non-zero exits remain distinguishable from transport/tool failures;
- artifacts can be regenerated without manual stopwatch timing;
- optional browser automation cannot run accidentally or concurrently.

These criteria are satisfied by the 2026-09-30 control-plane pilot documented in `phase19a-pilot-baseline.md`.

No Phase 19B optimization should be accepted without rerunning the relevant Phase 19A control scenario.
