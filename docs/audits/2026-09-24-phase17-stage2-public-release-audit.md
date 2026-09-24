# Phase 17 Stage 2 — Public Release Audit

Date: 2026-09-24

## Scope

This stage audited the repository surface that would become public on GitHub. It did not publish, push, tag, create a GitHub Release, notarize a binary, or start the distribution stage.

## Remediated in Stage 2

- Added a root Apache-2.0 `LICENSE` and made Chadex package metadata explicit about Apache-2.0 licensing.
- Kept WebCodex provenance and the exact upstream license separately through `UPSTREAM.md` and `attribution/WebCodex-LICENSE.txt`.
- Removed developer-specific absolute paths and real device/project identifiers from the current public source/documentation/benchmark surface.
- Replaced machine-specific benchmark defaults with repository-relative, environment-driven, or explicit inputs.
- Updated the Phase 9 benchmark to validate the current `chadex-runtime-*` bundle layout instead of obsolete `webcodex-runtime` executable names.
- Made `scripts/build_app.sh` use Cargo `release` for normal packaging; `dogfood` now requires an explicit `CHADEX_RUNTIME_PROFILE=dogfood` override.
- Added configurable app version/build metadata and validation for those values.
- Added the Chadex license to the packaged app and retained the separate WebCodex attribution copy.
- Avoided printing the full local signing identity in build logs.
- Added `scripts/release_check.sh` as the source-release gate.
- Kept ordinary tests fast while the release gate compiles every runtime target and runs release-critical runtime/tooling suites.
- Made visual-review tests write to a temporary location unless `CHADEX_UPDATE_UI_REVIEW=1` is explicitly requested.
- Restored `runtime-engine/frontend/dist/` as a committed compile-time input. Rust embeds these assets with `include_str!`; they are not a disposable build cache.
- Added `scripts/update_runtime_frontend.sh` and documentation for intentional frontend regeneration.
- Pinned the project Rust toolchain to `1.98.1` and aligned the project-local bootstrap path with that pin so a later clone does not silently move to a newer stable compiler.
- Documented that RC1 validation currently targets Apple silicon (`arm64`) only; Intel/Universal runtime support is not claimed.

## Validation completed

The bounded source release gate passed with `CHADEX_RELEASE_ALLOW_DIRTY=1` while this stage remains uncommitted. It covered:

- root license / upstream attribution consistency;
- Swift tests;
- Rust helper tests;
- `runtime-engine` `--workspace --all-targets` compilation;
- process, tool-contract, tool-runtime-contract, runner-registry, and tool-request-trace regression suites;
- production `chadex-runtime` wrapper compilation;
- Graphify → Obsidian and benchmark tooling unit tests;
- `git diff --check`.

A separate temporary package validation built a fresh `Chadex.app` with Cargo `release` profile and verified:

- Swift app executable;
- Rust helper;
- `chadex-runtime-cli`, `chadex-runtime-server`, and `chadex-runtime-runner`;
- bundled Chadex and WebCodex license files;
- `UPSTREAM.md` packaging;
- version/build plist metadata;
- plist validity;
- `codesign --verify --deep --strict`.

The temporary release-profile bundle was approximately 105 MiB and was deleted after validation. The existing `dist/Chadex.app` was not replaced by this stage.

## Findings intentionally not hidden

An unrestricted `cargo test --workspace` is not used as the RC gate. During audit it compiled successfully but entered environment/lifecycle tests that remained blocked for several minutes. The release gate instead compiles every target and runs bounded release-critical suites. Long-running/manual integration tests remain separate evidence and must not be silently treated as passed.

## Remaining release risks

### 1. Git history still contains machine-specific historical data

The current tree is sanitized, but existing history includes commits that previously contained developer-local absolute paths and real device/project identifiers. Publishing the full existing history would therefore re-expose information removed from the current tree.

Before the first public push, choose one explicit strategy:

- publish a clean/squashed public root history from the audited tree; or
- rewrite and re-audit the private history before publishing it.

Do not push the current full private history unchanged and assume the Stage 2 current-tree scrub is sufficient.

### 2. Distribution signing is not complete

The current local bundle is signed with a development identity and does not show Hardened Runtime flags. `scripts/build_app.sh` verifies signing structure, but Stage 2 does not implement:

- Developer ID Application distribution signing;
- Hardened Runtime configuration;
- notarization and stapling;
- Gatekeeper validation on a clean external machine;
- update signing / auto-update trust;
- GitHub Release publication.

These remain distribution-stage gates.

### 3. Release branch / history promotion is still unresolved

Development remains on `phase6/runtime-core`; `main` is still an older checkpoint. Stage 2 intentionally does not merge, rebase, rewrite history, create a release branch, or tag RC1.

### 4. CI publication gate is not yet authoritative

The repository now has a deterministic local source-release gate, but public CI / release automation has not been established as the authoritative publishing path. This should be resolved before a public binary release.

### 5. Dedicated secret-history scanner still required

Current-tree path/device scans and credential-pattern review found no credible live secret in the audited public surface, but dedicated tools such as gitleaks/trufflehog were not installed in this environment. Run a dedicated secret scan against the final public-history candidate before the first push.

## Stage 2 exit assessment

The current source surface is suitable to proceed to the next release-preparation stage. The remaining blockers are distribution/history/publishing decisions rather than unresolved repository garbage or known current-tree privacy leaks.
