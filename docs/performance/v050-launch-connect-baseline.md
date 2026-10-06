# v0.5.0 Launch / Connect Baseline

Date: 2026-10-06
Source: `integration/skill-parity` @ `4d28600` (acceptance build `dist/Chadex.app`, reports version 0.4.1 before the version bump)
Machine: Apple M2, 16 GB, macOS 27.2 (26B5091g)
Settings: interface size 140, background runtime prewarm on (default)

## Method

One manual acceptance session: launch the app, connect ChatGPT, open the Skills page twice, then
**Settings → Export Diagnostics**. The numbers below are the app's own lifecycle traces
(`operation=… phase=… total=…`) from that export. This is a single-run, warm-machine sample, not a
statistical benchmark; treat it as the reference point for regressions, not as a guarantee.

## Launch (helper start → runtime ready)

| Operation | Phase | Total |
| --- | --- | ---: |
| bootstrap | helper_start | 14.36 ms |
| bootstrap | credential_push | 44.79 ms |
| bootstrap | project_activation | 113.32 ms |
| bootstrap | total (UI usable) | 185.96 ms |
| prewarm | runtime_resume | 3,571.54 ms |
| prewarm | total | 3,571.55 ms |
| launch | helper_start_to_runtime_ready | 3,794.09 ms |

The window is usable after ~186 ms; the local runtime finishes resuming in the background ~3.6 s
later. Prewarm runs off the launch critical path.

## Connect

| Operation | Phase | Total |
| --- | --- | ---: |
| connect | runtime_ensure | 0.07 ms |
| connect | tunnel_start | 677.95 ms |
| connect | total | 678.02 ms |

Connect started ~5 s after launch, after prewarm had completed, so `runtime_ensure` found the
runtime ready and no `prewarm_join_wait` phase was recorded. Connect time is dominated by the
tunnel start (~0.68 s). A connect clicked *during* prewarm would add a `prewarm_join_wait` phase of at
most the remaining prewarm time; that case was not captured in this session.

## Skill discovery

| Operation | Run | Total |
| --- | --- | ---: |
| skill_discovery | first (cold) | 30.69 ms |
| skill_discovery | second (warm) | 2.71 ms |

## Gaps

- Single sample; no cold-boot (first launch after reboot) measurement.
- `prewarm_join_wait` not exercised (connect after prewarm finished).
- Windows has no measurement yet (CI-only validation so far).
