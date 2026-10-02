# Windows Track W2 — Runtime E2E

W2 starts at W1 commit `0e1b3172a08b81cc7b2b3deb7ffc25f47862db01`,
which descends from macOS `v0.3.2` (`7eb430d`). The development branch is
`windows/w2-runtime-e2e`. W3 desktop, W4 distribution and W5 stable release
remain separate milestones; W2 does not certify a Windows desktop application.

## Native gate

Run from a Windows x64 developer environment with the repository's pinned
Rust toolchain, Python 3, Git, Windows PowerShell and PowerShell 7 available:

```powershell
.\scripts\windows_core_check.ps1
python scripts/windows_runtime_e2e.py --output windows-runtime-e2e.json
if ($LASTEXITCODE -ne 0) { throw "Runtime E2E failed" }
```

The `windows-core` GitHub Actions job runs the same gate on `windows-latest`.
Each native command must propagate its exit code: PowerShell's
`$ErrorActionPreference = "Stop"` alone is insufficient for Cargo failures.
The workflow preserves the E2E JSON artifact on failure as well as success.

The gate retains the W1 full workspace/all-targets compile, default process,
persistent-shell, computer and runner-config tests, runner library and
Windows-specific binary tests, helper tests and standalone entrypoint check.
W2 additionally builds real helper/runtime binaries and explicitly runs the
existing opt-in native process/shell lifecycle tests. An explicitly selected
network test exercises the pinned official tunnel-client asset; it does not
require an API key or a live OpenAI tunnel credential.

## Evidence boundaries

The runtime harness must use the real NDJSON helper and local runtime server,
runner and registered project. A mocked tool response is not runtime E2E.
Required observations include file read/write, PowerShell command execution,
persistent state, an observed 45–75 second durable job, cancellation and
owned-process cleanup, followed by edit/validation and graceful shutdown.

Tunnel-native tests cover protected current-user/SYSTEM DACLs, junction and
reparse rejection, regular-file identity, actual PE version invocation,
archive/binary SHA-256, extraction, managed-cache reuse and corrupt-cache
replacement. Validating the executable's installer and `--version` does not
prove a credentialed OpenAI relay connection or ChatGPT workflow.

The helper's default Windows runtime state is `%LOCALAPPDATA%\Chadex\runtime`.
`CHADEX_DATA_DIR` remains an explicit override. The macOS default remains
`~/Library/Application Support/Chadex/runtime`. Missing Windows application
data fails closed instead of falling back to the current project directory.

## Validation record

Status: **complete for W2; native Windows core and runtime E2E exit criteria passed**.

- Baseline native CI: [run 36962494433](https://github.com/jimmynycuee/Chadex/actions/runs/36962494433)
  at W1 `0e1b317`, dispatched from the W2 development branch. All four job
  conclusions are green, but the Windows log contains runner `E0599` compile
  errors and a failed helper macOS-asset assertion. The old PowerShell gate
  masked these failures, so this is **not a valid Windows core pass**.
  The runner's tree-marker helpers now use the same Windows/feature cfg as
  their callers; the macOS asset test selects its platform explicitly.
  The observed host is Windows Server 2025 `10.0.26100`, x64, image
  `windows-2025-vs2026` version `20260925.250.1`, not a Windows 11 smoke test.
- Baseline remote macOS source/package gates: passed.
- First checked checkpoint: `4b0f484`,
  [run 36963928762](https://github.com/jimmynycuee/Chadex/actions/runs/36963928762).
  Full Windows workspace/all-targets compile and debug entrypoint builds passed.
  The 15 opt-in process lifecycle tests and 8 opt-in PowerShell lifecycle tests
  passed. Runner library tests reported 781 passed, 2 failed, 2 existing opt-in
  tests. The checked gate correctly failed with Cargo exit 101 and did not
  proceed to helper/tunnel/runtime E2E. macOS source/package and secret scan passed.
  The two failures concern the timeout diagnostic after large stderr output and
  a LF fixture inheriting Windows Git `core.autocrlf` conversion. The latter is
  addressed by explicit fixture configuration plus a separate CRLF preservation
  test; both LF/CRLF source-preservation tests passed locally.
  The timeout case also reproduced locally: its complete terminal stderr uses
  the newer adaptive diagnostic instead of the legacy `command timed out after`
  prefix. Repair preserves that prefix and retains the adaptive metadata; it
  does not change timeout budgets or execution behavior.
- Local macOS Swift regression: 70 passed, 0 failed.
- Second checked checkpoint: `28529a5`,
  [run 36966169327](https://github.com/jimmynycuee/Chadex/actions/runs/36966169327).
  Windows runner library: 784 passed / 0 failed / 2 existing opt-in;
  helper: 150 passed / 0 failed / 6 existing opt-in; tunnel supervisor:
  3 passed. The two extra helper EOF/blocked-stdin cleanup tests passed.
  LF and CRLF managed-worktree preservation tests both passed on Windows.
  DACL, reparse, file-identity and compiled-PE cache tests passed. The official
  asset installer then failed its version check, so runtime E2E did not run.
  The pinned upstream [formatter](https://github.com/openai/tunnel-client/blob/v0.0.12/pkg/version/version.go)
  and [CLI](https://github.com/openai/tunnel-client/blob/v0.0.12/cmd/client/root_command.go)
  append `+GitSHA` to the base version; the strict parser incorrectly rejected
  valid build metadata. The repair keeps an exact pinned base version and validates
  SemVer metadata identifiers, continuing to reject prereleases and prefix collisions.
  macOS source/package and secret-scan gates passed.
  The durable-job fixture now writes explicit UTF-8 bytes to keep its expected
  LF hash identical on Windows, where Python text writes otherwise use CRLF.
- Local macOS helper regression after integration: 159 unit and 3 integration
  tests passed, 0 failed; 3 pre-existing opt-in unit tests remain unchanged.
- The complete local helper suite also passed after the metadata-parser repair.
  The pinned official `darwin-arm64` archive and binary hashes were checked,
  then the executable's actual `--version` exited 0 with
  `0.0.12+881c9a8fed7cccbe6607cd419863bbca506b8215 (git sha: 881c9a8fed7cccbe6607cd419863bbca506b8215)`.
  This was an official binary/version smoke, not a credentialed tunnel. Its
  temporary archive and executable were removed automatically. Initial Python
  HTTPS hit a missing local CA; system curl retained TLS verification and completed.
- Harness deterministic tests: 16 passed, 0 failed. The actual runtime harness rejects
  non-Windows hosts; unit tests do not certify Windows execution.
- Local macOS process default tests: 3 passed, 15 existing opt-in tests.
- Local macOS process lifecycle opt-in tests: 15 passed, 0 failed, including
  process-tree drop, explicit termination, parent EOF and 20 stress cycles.
- Local macOS runner-config tests: 24 passed, 0 failed.
- Local macOS runner library after timeout repair: 863 passed, 0 failed,
  5 existing opt-in tests (`--test-threads=4`). An initial default-concurrency
  run reported 862 passed and one CPU-progress timing failure; that test passed
  both the narrow adaptive-timeout run and the full four-thread rerun. No tests
  were skipped or weakened to resolve it.
- Local macOS persistent-shell default tests: 7 passed, 4 failed, 6 existing
  opt-in tests. The failures compare logical `/tmp`/`/var` paths with physical
  `/private/tmp`/`/private/var` paths. With `TMPDIR=/private/tmp`, 8 pass and the
  three explicit `/tmp` assertions still fail. This crate is unchanged from
  W1; the handoff records the same pre-existing canonical-path limitation.
- Final runtime-validated checkpoint:
  `cbe258cefedaa952cb720bf2594c8b5e8ec6e2d4`,
  [run 36987012302](https://github.com/jimmynycuee/Chadex/actions/runs/36987012302),
  Windows job `110774230581`. Source release, ARM64 package, public-history /
  secret scan, Windows core and Windows runtime E2E all passed.
- The final Windows host was Microsoft Windows Server 2025 Datacenter
  `10.0.26100`, GitHub runner `2.337.0`, image
  `windows-2025-vs2026` version `20260925.250.1`.
- Final Windows core results include runner library 784 passed / 0 failed /
  2 ignored, helper 150 passed / 0 failed / 6 ignored, Windows tunnel
  supervisor 3 passed / 0 failed, and the selected official pinned Windows
  tunnel asset install/reuse/replacement/version test passed.
- Runtime E2E repository evidence:
  [evidence/W2_windows_runtime_e2e_36987012302.sanitized.json](evidence/W2_windows_runtime_e2e_36987012302.sanitized.json).
  The canonical raw JSON remains the GitHub Actions artifact for run
  `36987012302`; the repository projection omits only the 31
  `token_sha256` observation digests so the public-history secret scan remains
  fail-closed without allowlisting secret-like token fields.
  All 13 stages passed. The durable job ran 55.093 seconds with one launch and
  one Job, exercised observation timeout, and preserved runtime identity.
  Cancellation observed a two-process payload tree and left zero remaining
  processes. Helper shutdown observed seven owned processes, required no forced
  cleanup, and left zero remaining processes.

W2 is complete. This is not Windows Desktop certification: physical Windows 11
and Windows ARM64 hardware, Desktop UI, installer/signing/updater, credentialed
OpenAI relay / ChatGPT workflow, and unbundled default resource discovery remain
outside W2. The E2E harness uses isolated data/resource/bin overrides. Graphify
and Obsidian source-converged synchronization remains deferred to W4.

Do not automatically advance to W3; start W3 only after an explicit user request.
