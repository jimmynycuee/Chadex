# W3 validation evidence

The automated implementation is verified. **Full W3 product acceptance remains pending**: the user has no Windows host and explicitly elected to retain manual/private checks as not validated.

## Provenance

- Branch: `windows/w3-desktop-product`.
- W2 baseline: `c605691336d46f4f0e5d9efbee1d34f00ff22885`; W2 final CI `36990190294` was verified before work began.
- Runtime-validated W3 implementation: `3ecaf6e0b647d27db5e53aa6ae1385aff8bc5cd2`.
- [Full CI run 37026168474](https://github.com/jimmynycuee/Chadex/actions/runs/37026168474): **success**, testing that exact implementation SHA.
- Windows job: `110901233862`. Public-history/Gitleaks, source release, ARM64 package and Windows core/W2/W3 gates all passed.
- This evidence commit changes documentation/projections only. Its own final HEAD must receive another full successful CI run; the handoff records that final SHA/run separately. An earlier implementation result is not substituted for final-HEAD CI.

W4 baseline recheck (2026-10-03, Asia/Taipei): final W3 evidence commit `dd7d5ebbc4bf06c91ebe3a86a8e52c2d38fc52c3` passed [CI 37029362354](https://github.com/jimmynycuee/Chadex/actions/runs/37029362354), attempt 2. Public-history/secret scan, source release, ARM64 package and Windows core/W2/W3 gates all concluded success. This closes the final-HEAD CI requirement above; it does not close the manual/private acceptance items below.

## Environment and versions

Windows Server 2025 Datacenter `10.0.26100`; GitHub runner `2.337.0`; image `windows-2025-vs2026`, version `20260925.250.1`. WebView2 launch was exercised; its installed version was not separately recorded. Physical Windows 11/ARM64 hardware is not validated.

Rust `1.98.1`, Node `22.12.0`, Tauri `2.12.1`, `tauri-build` `2.7.1`, React `19.1.1`, TypeScript `5.9.2`, Vite `7.3.6`, Vitest `4.1.11`, Tauri CLI `2.8.4`, `keyring` `3.6.3`. Lockfiles pin resolved dependencies.

## Executed Windows checks

| Check | Result |
| --- | --- |
| Runner library | 784 passed, 0 failed, 2 ignored |
| Helper | 150 passed, 0 failed, 6 ignored |
| Windows tunnel supervisor | 3 passed |
| Official pinned Windows tunnel asset | passed |
| Frontend | typecheck/build passed; 34 tests passed |
| Bridge | 16 tests passed, including abrupt-parent Job Object ownership |
| Native desktop | 6 tests passed |
| Smoke harness/resource prep | 12 tests passed |
| Production desktop executable | built with `custom-protocol`, without smoke IPC |
| Smoke desktop executable | built with `custom-protocol,desktop-smoke`; actual WebView launched |
| W2 runtime E2E | 13/13 stages passed |
| W3 desktop/runtime smoke | 27/27 stages passed; 10 successful real MCP tool calls |

[W2 regression projection](W3_W2_regression_37026168474.sanitized.json): durable job 55.125 s, `launch_count=1`, `job_count=1`; observation timeout exercised and runtime identity preserved. Cancellation payload 2→0; helper shutdown 7→0; forced cleanup 0; helper stderr 0 bytes.

[W3 desktop projection](W3_desktop_smoke_37026168474.sanitized.json): actual WebView → helper/runtime readiness → project A read/process/write/read → project B operation → disconnect and credential-free connect rejection → helper restart → exact helper kill → failed/offline observation and isolated credential deletion → recovery and project B read → graceful exit. A second launch restored preferences/selection while remaining offline. Startup failure and abrupt desktop-parent termination also passed. Across scenarios, 56 owned process identities were observed, 0 remained, and `forced_count=0`; fixture cleanup passed. The count is aggregate observations across scenarios, not simultaneous processes.

The Windows extended-path correction is confined to W3. Its harness matches Rust-canonicalized `CHADEX_DATA_DIR` before calling the unchanged W2 strict environment-file containment/token checks. Finite failure codes and regression checks retain diagnostic value without raw paths, credentials or hashes. No W2 test, secret scan or isolation rule was weakened.

## macOS and local regression

The same implementation SHA passed the macOS source release gate and ARM64 package build/shape gate. Local development checks also passed: Swift 70 tests (`swift test --disable-sandbox`); helper 159 passed/3 ignored; Unix tunnel 3 passed; native unit tests 6 passed; frontend 34 tests/typecheck/build; isolated real-helper bridge 6/6 with helper stopped and fixture removed. These local checks do not certify a new macOS visible-UI/credentialed ChatGPT session or Windows native controls.

## Manual/private checks: not validated

The user has no Windows host. Preserve these items until an actual Windows tester can record the exact commit, Windows/WebView2 versions, time and outcome:

| Check | Status |
| --- | --- |
| `native_picker` — cancel/select, Unicode/space path and recents | not validated |
| `explorer_open` — active project folder | not validated |
| `tray` — Show/status/Quit and window close | not validated |
| `launch_at_login` — OS registration and actual login launch | not validated |
| `notifications` — background failure transitions | not validated |
| `credentialed_tunnel` — saved restricted key, real secured tunnel/ChatGPT operation, Activity, switch/reconnect/network failure | not validated |

Credential Manager isolated write/read/delete and deletion after helper death passed in public CI; reuse of a real production credential and credentialed ChatGPT verification remain private/manual. The detailed product checklist, including Code Ferret control/observed states, is in [W3 desktop product](../W3-desktop-product.md#manualprivate-acceptance-still-required).

CI uses isolated data/resources and direct Cargo debug executables. The production-feature artifact excludes smoke IPC but remains an unsigned debug build, not a release/installer. Default installed resource discovery, manual native controls, signing, installer/updater distribution and physical hardware acceptance are not certified. No merge to `main`, W4 Graphify/Obsidian work or release was performed.

## Reproducible artifacts

Artifacts retained for seven days on the successful implementation run:

- [W2 raw runtime report](https://github.com/jimmynycuee/Chadex/actions/runs/37026168474/artifacts/11235863109): `windows-runtime-e2e-37026168474-1`.
- [Sanitized W3 report](https://github.com/jimmynycuee/Chadex/actions/runs/37026168474/artifacts/11235894174): `windows-desktop-w3-report-37026168474-1`.
- [Smoke-feature executable/resources](https://github.com/jimmynycuee/Chadex/actions/runs/37026168474/artifacts/11235724330): `windows-desktop-w3-smoke-debug-37026168474-1`.
- [Desktop executable without smoke IPC](https://github.com/jimmynycuee/Chadex/actions/runs/37026168474/artifacts/11235654410): `windows-desktop-w3-production-debug-37026168474-1`.

For later final-HEAD runs, use the same artifact prefixes and actual run ID/attempt. Raw W2 observations remain in Actions; the repository projection excludes paths, IDs, tokens, token hashes, command output and configuration. No Gitleaks allowlist was added. Reproduce with `.github/workflows/ci.yml`; local Windows commands and native/private acceptance are documented in the desktop product file.

## Subagent dispatch record

All calls were accepted, tasks completed, results integrated and agents closed. The tool returned agent identities but no trustworthy actual model/reasoning metadata: every execution setting below remains **unconfirmed**. Specified settings are spawn parameters only.

| Task / agent ID | Specified model / reasoning | Confirmed execution | Result |
| --- | --- | --- | --- |
| Bridge — `01a0fc0c-d19e-71c1-a085-eeb9c0cb5c47` | `gpt-6.1-sol / xhigh` | unconfirmed | integrated; CI bridge tests passed |
| Frontend — `01a0fc0c-d2a5-7503-a91c-a6398f241b9d` | `gpt-6.1-sol / high` | unconfirmed | integrated; 34 tests passed |
| Helper contract — `01a0fc0d-fc25-7ed1-adfe-882bccd713e5` | `gpt-6-luna / max` | unconfirmed | additive readiness projection integrated |
| CI/harness — `01a0fc14-a458-7950-badf-eed78f0238c5` | `gpt-6-luna / max` | unconfirmed | integrated and refined; actual CI workflow passed |
| Security/lifecycle review — `01a0fc25-9a7d-7ba3-b5a5-15da3eebecb5` | `gpt-6.1-sol / xhigh` | unconfirmed | findings corrected; lifecycle/credential tests passed |
| Failure cleanup tests — `01a0fc4c-ec31-74b0-96a4-758fb620a2a8` | `gpt-6-luna / xhigh` | unconfirmed | integrated; abort and finite assertion tests passed |
| Evidence draft — `01a0fd33-630b-7112-b3f7-1eab78274b75` | `gpt-6-luna / xhigh` | unconfirmed | reviewed/refined with final implementation evidence |
