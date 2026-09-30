# Phase 19A Long-Haul Benchmark Harness

Phase 19A establishes a reproducible long-task latency and reliability baseline before Phase 19B changes Chadex runtime behavior.

The suite is organized by failure mode rather than Small/Medium/Large:

- P1 `silent-runner`: long/silent process supervision.
- P2 `bursty-500k`: bursty 500 KB output.
- P3 `log-avalanche-1m`: 1 MB log avalanche.
- P4 `late-failure`: expected late non-zero exit.
- P5 `multi-stage-late-fail`: multi-stage workflow with a late failure.
- P6 `repository-marathon`: real engineering marathon; it requires a dedicated fixture and is intentionally not synthesized by the pilot workload.

## Measurement surfaces

### Primary baseline: deterministic Chadex control plane

`scripts/phase19a_control_plane_campaign.py` calls Chadex's local loopback MCP endpoint directly. It does **not** open, navigate, or automate ChatGPT.

Use this surface for Phase 19A/19B regression work. It isolates:

- local MCP request/response timing;
- Server dispatch and full request traces;
- Runner/job handoff;
- terminal observation through `observe_jobs`;
- model-facing response bytes and output truncation;
- expected late failures;
- credential-free heartbeat and connection-layer state.

Generated artifacts include per-run `metrics.json` and `timeline.json`, plus campaign `manifest.json` and `summary.md`.

### Secondary sanity: ChatGPT Web E2E

`scripts/phase19a_longhaul_campaign.py` is retained only for low-frequency host/browser sanity checks.

Browser automation is:

- explicit opt-in with `--allow-browser-automation`;
- protected by a single-owner browser lock;
- **not baseline-eligible by default**.

During Phase 19A development, repeated automated Temporary Chat / connector interactions correlated with ChatGPT host-side extended-processing/review states. Those runs remain useful for localization, but they are harness-development evidence rather than Chadex performance baseline data.

## Components

- `scenarios.json`: failure-mode scenario manifest.
- `live-config.example.json`: tracked template for machine-local configuration.
- `live-config.json`: machine-local config; intentionally ignored by Git.
- `scripts/phase19a_control_plane_campaign.py`: primary deterministic baseline runner.
- `scripts/phase19a_pilot_workload.py`: deterministic local workloads.
- `scripts/phase19a_heartbeat.py`: credential-free process/tunnel heartbeat sampler.
- `scripts/phase19a_longhaul_profile.py`: optional E2E trace profiler.
- `scripts/phase19a_longhaul_campaign.py`: optional browser E2E sanity runner.
- `scripts/phase19a_launch_traced_app.sh`: launch Chadex with full request tracing.

## Operational rules

1. Use the deterministic control-plane campaign for Phase 19 baseline/regression runs.
2. Keep full execution evidence; later optimization may reduce only model-facing observation.
3. Do not use a manual stopwatch. Harness timestamps and Chadex traces are authoritative.
4. Expected non-zero exits in P4/P5 are successful benchmark outcomes when they match the manifest.
5. Keep generated run artifacts outside the source repository. A typical local root is `~/Documents/ChatGPT/agent-harness-benchmark/phase19/`.
6. Heartbeat sampling must not add ChatGPT tool calls or expose credentials.
7. Browser E2E requires explicit opt-in and must run one campaign at a time.
8. Never interpret `observation_to_next_server_action` as pure model inference time; it is an end-to-end host/control-plane interval.

## Local configuration

```bash
cp benchmarks/phase19a-longhaul/live-config.example.json \
  benchmarks/phase19a-longhaul/live-config.json
```

Never commit the populated `live-config.json`.

## Primary smoke invocation

```bash
python3 scripts/phase19a_control_plane_campaign.py \
  --config benchmarks/phase19a-longhaul/live-config.json \
  --output-dir "$HOME/Documents/ChatGPT/agent-harness-benchmark/phase19/control-plane-smoke" \
  --scale 0.01
```

A longer pilot can use `--scale 0.1` or the manifest durations.

## Optional E2E sanity invocation

Only when intentionally testing the ChatGPT host/browser surface:

```bash
python3 scripts/phase19a_longhaul_campaign.py \
  --allow-browser-automation \
  --config benchmarks/phase19a-longhaul/live-config.json \
  --output-dir "$HOME/Documents/ChatGPT/agent-harness-benchmark/phase19/e2e-sanity" \
  --scenario-id P1-silent-runner \
  --scale 0.01
```

The formal Phase 19C benchmark should use failure-injection and engineering-marathon fixtures while keeping this deterministic control-plane suite as the regression control.
