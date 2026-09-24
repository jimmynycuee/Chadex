# Phase 17 Stage 4 — Public RC1 Readiness

Date: 2026-09-24

## Starting state

Stage 4 starts from the clean Stage 3 private checkpoint and single-root public candidate with identical trees. No Git remote exists locally.

## Verified external release capabilities

- GitHub account access is available through the connected GitHub integration, but the local `gh` CLI is not installed and the repository has no configured Git remote.
- The public Git identity can therefore be set to the authenticated account's ID-based GitHub noreply address without exposing the account's private email.
- Authenticated GitHub identity: `jimmynycuee` (user id `170504951`); the public root uses `170504951+jimmynycuee@users.noreply.github.com` rather than the account contact email.
- Xcode provides `notarytool` and `stapler`.
- The local keychain currently contains an **Apple Development** signing identity only. There is no `Developer ID Application` identity available.
- The existing `dist/Chadex.app` is Apple Development-signed, has no stapled notarization ticket, and Gatekeeper rejects it as a distribution artifact. It remains a valid development build, not a publishable RC binary.

## Stage 4 implementation

Stage 4 adds:

- explicit `auto`, `development`, `distribution`, and `adhoc` signing modes;
- inside-out nested code signing without using `--deep` as a signing shortcut;
- Hardened Runtime for packaged executable code;
- Developer ID and secure-timestamp enforcement in distribution mode;
- a strict distribution package verifier;
- an Apple `notarytool` + stapling + Gatekeeper release path;
- a public-history privacy and optional required Gitleaks gate;
- GitHub Actions CI for history/secret scanning, source release validation, and ARM64 packaging;
- a tag-driven release workflow that imports a Developer ID certificate from repository secrets, uses an App Store Connect API key for notarization, staples the ticket, publishes checksums, and creates the GitHub Release only after those gates succeed;
- RC1 release notes explicitly scoped to Apple Silicon.

## Secret-scanner hardening

Stage 4 does not rely on Gitleaks 8.30.1 because a public regression report indicates that release can silently miss canonical secrets. CI/release tooling is pinned instead to Gitleaks 8.29.1 with verified archive hashes. The scanner is scoped to the selected public ref so the intentionally retained private development history cannot contaminate the public release audit. A checked-in Gitleaks configuration only suppresses known non-secret client-window hashes and explicit synthetic redaction-test fixtures.

## Build-concurrency stabilization

The first clean public full gate encountered an environment-sensitive native compile failure in `aws-lc-sys`, followed by a missing `objc2` intermediate artifact. The identical all-target build completed successfully when Cargo concurrency was bounded to four jobs. Stage 4 therefore makes four jobs the release/package default via `CARGO_BUILD_JOBS`, while retaining `CHADEX_CARGO_JOBS` as an explicit positive-integer override.

## Release-gate concurrency hardening

During final public-root validation, the same release gate was accidentally started twice against one worktree. The duplicate Cargo processes contended for the same target directory and one rustc invocation failed while creating a fingerprint output file. This was an execution collision, not a source/test failure. `scripts/release_check.sh` now acquires a per-worktree single-instance lock before validation, so duplicate local invocations fail closed before compilation and cannot corrupt shared build evidence.

A later outer execution timeout also demonstrated that a hard kill can bypass normal shell traps. The lock therefore stores its owner PID and reclaims itself only when that owner is no longer alive; cleanup removes a lock only when the current process still owns it. This makes retries safe after runner-level termination without weakening the concurrent-run exclusion.

## Free-publication decision

Chadex now supports a free GitHub distribution path modeled after WebCodex Desktop: an exact-tag Apple Silicon DMG is built with ad-hoc signing plus Hardened Runtime, smoke-tested, checksummed, and published without Apple notarization. The user-facing instructions explicitly use **System Settings → Privacy & Security → Open Anyway** when Gatekeeper blocks first launch, and never recommend disabling Gatekeeper globally.

Developer ID + notarization is retained as an optional future upgrade rather than a release blocker. The remaining publication blocker is the GitHub destination: no Chadex repository/remote has been created or selected yet.
