# W2 Windows validation evidence — run 36987012302

- Runtime-validated source: `cbe258cefedaa952cb720bf2594c8b5e8ec6e2d4`
- Branch: `windows/w2-runtime-e2e`
- GitHub Actions run: `36987012302`
- Windows job: `110774230581`
- Result: all CI jobs passed; Windows core and Windows runtime E2E passed.
- Host: Microsoft Windows Server 2025 Datacenter `10.0.26100`
- GitHub runner: `2.337.0`
- Runner image: `windows-2025-vs2026`, version `20260925.250.1`

## Native gate

- Runner library: 784 passed, 0 failed, 2 ignored.
- Helper: 150 passed, 0 failed, 6 ignored.
- Windows tunnel supervisor: 3 passed, 0 failed.
- Official pinned Windows tunnel asset install/reuse/replacement/version test: 1 passed.
- Source release gate, ARM64 package gate, and public-history/secret scan also passed.

## Runtime E2E

The canonical raw artifact remains attached to GitHub Actions run
`36987012302` as `windows-runtime-e2e-36987012302-1`. The repository keeps
a sanitized projection at
[`W2_windows_runtime_e2e_36987012302.sanitized.json`](W2_windows_runtime_e2e_36987012302.sanitized.json).
It records 13/13 stages as passed. The projection removes the 31
`token_sha256` observation fields because Gitleaks intentionally treats a
`token`-named high-entropy value as secret-like even when it is already a
one-way digest; no runtime result or lifecycle evidence is removed.

- Durable job: 55.093 s; `launch_count=1`; `job_count=1`.
- Observation timeout was exercised and runtime identity remained unchanged.
- Cancellation payload: 2 observed processes; 0 remained after cancellation.
- Helper shutdown: 7 owned processes observed; 0 remained; `forced_cleanup_count=0`.
- Helper stderr bytes: 0.
- Fixture cleanup passed.

## Scope boundary

This evidence is native Windows CI evidence for the real helper/server/runner local
runtime path. The E2E harness intentionally uses isolated
`CHADEX_DATA_DIR`, `CHADEX_RESOURCE_DIR`, and `CHADEX_RUNTIME_BIN_DIR`
overrides. It does not certify Windows Desktop UI, installer/signing/updater,
physical Windows 11 or ARM64 hardware, credentialed OpenAI relay/ChatGPT
workflow, or unbundled default resource discovery.
