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

Status: **in progress; W2 exit criteria are not yet certified**.

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
- Local macOS Swift regression: 70 passed, 0 failed.
- Local macOS helper regression after integration: 159 unit and 3 integration
  tests passed, 0 failed; 3 pre-existing opt-in unit tests remain unchanged.
- Harness unit tests: 12 passed, 0 failed. The actual runtime harness rejects
  non-Windows hosts; unit tests do not certify Windows execution.
- Local macOS process default tests: 3 passed, 15 existing opt-in tests.
- Local macOS process lifecycle opt-in tests: 15 passed, 0 failed, including
  process-tree drop, explicit termination, parent EOF and 20 stress cycles.
- Local macOS runner-config tests: 24 passed, 0 failed.
- Local macOS persistent-shell default tests: 7 passed, 4 failed, 6 existing
  opt-in tests. The failures compare logical `/tmp`/`/var` paths with physical
  `/private/tmp`/`/private/var` paths. With `TMPDIR=/private/tmp`, 8 pass and the
  three explicit `/tmp` assertions still fail. This crate is unchanged from
  W1; the handoff records the same pre-existing canonical-path limitation.
- Windows runtime/long-job/PowerShell/tunnel results: **not validated yet**.

Do not advance to W3 until the checked native gate and real runtime scenarios
have passed, with the final source commit and actual runner environment recorded.
