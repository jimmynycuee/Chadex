# Phase 6I — Production runtime independence

Date: 2026-09-21

## Goal

Remove `vendor/webcodex` as a production runtime/build/library dependency without regressing the Phase 11 worktree model or Phase 12 execution semantics. Preserve the fixed upstream snapshot, Apache-2.0 license, and migration history as explicit provenance/attribution.

## Migration

- Promoted the maintained MCP/file/Runner implementation into `runtime-engine/`.
- Renamed production Cargo package identities to `chadex-runtime-*`.
- Repointed `chadex-runtime` and `chadex-helper` dependencies to `runtime-engine/`.
- Moved dogfood build output from `vendor/webcodex/target` to `runtime-engine/target`.
- Replaced upstream build identity pinning with Chadex-owned `CHADEX_RUNTIME_GIT_*` metadata.
- Kept `vendor/webcodex` as provenance/reference only.
- Kept Apache-2.0 attribution in `runtime-engine/LICENSE`, `attribution/WebCodex-LICENSE.txt`, `UPSTREAM.md`, and the packaged app resources.

## Dependency audit

Production `cargo tree` for both `chadex-runtime` and `chadex-helper` contains zero package identities beginning with `webcodex`. Active production manifests, lockfiles, Rust source paths, and build scripts contain no `vendor/webcodex` path dependency.

Some inherited internal identifiers and compatibility vocabulary still contain `webcodex` text inside the derivative runtime source. They are not external package/path dependencies and are retained only where changing protocol or compatibility vocabulary would add risk without changing ownership. Upstream origin remains documented rather than hidden.

## Regression validation

| Suite | Result |
|---|---:|
| Phase 12 execution semantics | 65 passed / 0 failed |
| Chadex runtime tool contracts | 139 passed / 0 failed |
| Chadex runtime tool-runtime contracts | 94 passed / 0 failed |
| Chadex runtime Runner | 855 passed / 0 failed / 4 ignored |
| `chadex-helper` | 123 passed / 0 failed / 3 ignored |
| Swift `ProtocolModelTests` | 13 passed / 0 failed |

The first promoted Runner run reported 76 failures. All were traced to test-only relative paths that still pointed at old crate/fixture locations (`webcodex-lsp`, `webcodex-process`, and the upstream `tests/fixtures` root). After copying the fixtures into `runtime-engine/tests/fixtures` and correcting those test-only paths, the complete 855-test Runner suite passed. No test was removed, weakened, or ignored to obtain the pass.

## Hard vendorless acceptance

For the decisive acceptance, the complete `vendor/webcodex` directory was temporarily moved outside the repository. While it was absent:

1. `chadex-runtime` locked/offline Cargo metadata and check succeeded.
2. `chadex-helper` locked/offline Cargo check succeeded.
3. `./scripts/build_app.sh` completed successfully through runtime build, helper release build, Swift release build, plist validation, app packaging, Apple Development signing, and strict codesign verification.
4. The resulting `Chadex.app` contained only:
   - `chadex-runtime-cli`
   - `chadex-runtime-server`
   - `chadex-runtime-runner`
5. No `webcodex`, `webcodex-server`, or `webcodex-runner` executable existed in the bundle.
6. Bundled `WebCodex-LICENSE.txt` exactly matched `attribution/WebCodex-LICENSE.txt` by SHA-1.

After acceptance, the original fixed upstream snapshot was restored to `vendor/webcodex` unchanged for provenance/reference.

## Build-time note

A cold dogfood optimized rebuild of the newly promoted large runtime crate exceeded the 300-second `run_shell` execution budget twice without a compiler or test failure. The same unchanged `build_app.sh` was then executed with the long-running process path and completed successfully. No optimization level, runtime behavior, or test requirement was reduced to work around the shell budget. This is a build orchestration/time-budget issue, not a Phase 6I runtime dependency failure.
