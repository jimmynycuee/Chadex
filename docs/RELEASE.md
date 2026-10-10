# Release preparation

Chadex keeps development validation separate from release validation. A successful local build is not by itself a publishable release artifact.

## Release gate

Run from a clean checkout:

```sh
./scripts/release_check.sh
```

The gate verifies:

- root Apache-2.0 licensing and WebCodex attribution copies;
- Swift tests;
- Rust helper tests;
- compilation of every `runtime-engine` target plus release-critical process, tool-contract, registry, and trace regression suites;
- the production `chadex-runtime` wrapper binaries using the shared ignored target directory;
- `git diff --check` hygiene;
- a fresh temporary `.app` built with Cargo `release` profile;
- expected Chadex runtime executables and bundled license/provenance files;
- plist validity and deep/strict code-sign verification.

The gate fails on a dirty Git worktree by default. `CHADEX_RELEASE_ALLOW_DIRTY=1` exists only for pre-release development validation and must not be treated as release evidence.

The gate is single-instance per worktree. A second concurrent invocation fails before compilation instead of allowing two Cargo processes to mutate the same target directories and corrupt validation evidence. The lock records its owner PID; if an outer runner is forcibly terminated before shell traps execute, the next invocation detects the dead owner and safely reclaims the stale lock.

Rust is pinned by the repository `rust-toolchain.toml`; `scripts/bootstrap_rust.sh` installs that same toolchain for project-local builds instead of following a moving `stable` channel.

Release validation and packaging default Cargo concurrency to four jobs through `CARGO_BUILD_JOBS`. This avoids resource-sensitive native dependency failures observed under unrestricted parallel builds. Maintainers can override the bound with a positive integer in `CHADEX_CARGO_JOBS` when validating another build host.

To skip only the package-build portion while iterating on tests:

```sh
CHADEX_RELEASE_CHECK_BUILD_APP=0 ./scripts/release_check.sh
```

Normal Swift tests render visual-review screenshots into a temporary directory so a validation run does not rewrite tracked reference images. To intentionally refresh the committed UI review set, run:

```sh
CHADEX_UPDATE_UI_REVIEW=1 swift test --filter VisualReviewTests
```

## Compile-time Runtime Console assets

`runtime-engine/frontend/dist/` is a committed compile-time input, not a local cache. Rust embeds it with `include_str!`, so removing it breaks a fresh clone before runtime tests or packaging can start. Maintainers can regenerate it from the fixed upstream-derived frontend source with `./scripts/update_runtime_frontend.sh`; normal production builds consume the committed output and do not build from `vendor/webcodex`.

## Package metadata

`scripts/build_app.sh` accepts release metadata through environment variables:

```sh
CHADEX_APP_VERSION=0.6.3 \
CHADEX_APP_BUILD_NUMBER=1 \
CHADEX_CODESIGN_IDENTITY="Developer ID Application: ..." \
./scripts/build_app.sh
```

`CHADEX_APP_VERSION` and `CHADEX_APP_BUILD_NUMBER` must use decimal dot-separated values. Runtime packaging defaults to Cargo `release`; `CHADEX_RUNTIME_PROFILE=dogfood` is an explicit development-only override.

For a version bump, update the default in `scripts/build_app.sh`, the own-package versions in `chadex-runtime/Cargo.toml`, `runtime-engine/Cargo.toml` (`workspace.package.version`), and `rust-helper/Cargo.toml`, plus the synchronized Windows product metadata in `apps/windows/package.json`, `apps/windows/src-tauri/Cargo.toml`, and `apps/windows/src-tauri/tauri.conf.json`. Update only Chadex-owned path-package entries in the corresponding Cargo/package lockfiles; third-party dependency versions must remain unchanged. Advance the Windows synthetic upgrade baseline to the previous public version. Add `docs/releases/X.Y.Z.md` for the exact release tag and update `CHANGELOG.md` and the README release-note link. The generated app `Info.plist` takes its version from the build environment; it is not a source file to edit.

### Per-release validation notes

Each `docs/releases/X.Y.Z.md` records the validation evidence and remaining external boundaries for that tag (for example, `0.4.1.md` covers the Windows W5 automated closeout, historical-source upgrade and default NSIS self-copy coverage). Run the source gate from a clean release checkout. A dirty-tree run with `CHADEX_RELEASE_ALLOW_DIRTY=1` is development evidence only.

For the free artifact path, explicitly select ad-hoc signing, then package the resulting bundle:

```sh
CHADEX_APP_VERSION=0.6.3 CHADEX_APP_BUILD_NUMBER=1 \
CHADEX_CODESIGN_MODE=adhoc CHADEX_RUNTIME_PROFILE=release \
./scripts/build_app.sh
./scripts/package_free_macos_release.sh dist/Chadex.app
```

The expected artifacts are `dist/Chadex-v0.6.3-macos-arm64.dmg` and its `.sha256` sidecar. If the requested app is running, `build_app.sh` packages a `-next.app` sibling instead; pass the actual output path to the DMG packager. Package smoke does not replace visual mascot checks, installed-app launch, or updater validation.

## Public Git history

`main` on GitHub is the public, single-root history; every release tag is cut from it. The pre-publication private development history (which contained machine-local paths and connector/device identifiers) is kept outside the public repository for provenance only and must never be pushed or merged into the public remote.

Every push and every release tag re-runs the public-history gate below against `HEAD`, so new commits are held to the same identity, path and secret rules as the original public root.

## Distribution signing

`scripts/build_app.sh` supports four explicit signing modes through `CHADEX_CODESIGN_MODE`:

- `auto` (default): use an Apple Development identity when available, otherwise hardened-runtime ad-hoc signing;
- `development`: require Apple Development signing;
- `distribution`: require **Developer ID Application**, Hardened Runtime, and a secure timestamp;
- `adhoc`: hardened-runtime ad-hoc signing for the free public release path and CI package-shape validation.

Nested helper/runtime executables are signed explicitly before the app bundle. `--deep` is used only for final verification, not as a signing shortcut.

A Developer ID package must pass:

```sh
./scripts/distribution_check.sh /path/to/Chadex.app
```

This checks deep/strict signature validity, Developer ID authority, Hardened Runtime, secure timestamp, the expected `arm64` architecture, and bundled license/provenance files. Set `CHADEX_REQUIRE_NOTARIZED=1` to additionally require a stapled ticket and successful Gatekeeper assessment.

## Notarization

After building with `CHADEX_CODESIGN_MODE=distribution`, notarize with either a stored `notarytool` keychain profile or App Store Connect API-key credentials:

```sh
CHADEX_NOTARY_KEYCHAIN_PROFILE=chadex-notary \
./scripts/notarize_app.sh /path/to/Chadex.app
```

or:

```sh
CHADEX_NOTARY_API_KEY_PATH=/secure/path/AuthKey_ABC123.p8 \
CHADEX_NOTARY_KEY_ID=ABC123 \
CHADEX_NOTARY_ISSUER_ID=00000000-0000-0000-0000-000000000000 \
./scripts/notarize_app.sh /path/to/Chadex.app
```

The script submits a ZIP with `notarytool`, waits for acceptance, staples the ticket, validates it, performs a Gatekeeper assessment, then creates a fresh post-staple ZIP plus SHA-256 file. Never commit `.p12`, `.p8`, passwords, or notary credentials.

## GitHub CI and release workflow

`.github/workflows/ci.yml` uses the supported ARM64 `macos-15` runner for source/package validation, `windows-latest` for the Windows core, W3 desktop and W5 installer/historical-upgrade gates, and Ubuntu jobs for the dedicated Gitleaks history scan, change classification and docs checks. Gitleaks is pinned to 8.29.1 and its archive checksum is pinned in workflow source.

CI orchestration rules:

- **Change classification.** `scripts/ci_change_scope.py classify` diffs the push `before` SHA (or the PR base) against `HEAD`. Only when every changed path is in its explicit docs allowlist (`docs/**`, `graphify-out/**`, `README.md`, `CHANGELOG.md`, `PHASES.md`, `HANDOFF.md`, `.graphifyignore`) are the macOS source/package and Windows jobs skipped. A new branch, force push, manual dispatch, empty diff or unknown path always runs the full pipeline. `UPSTREAM.md`, `LICENSE` and `ui-review/` are package or test inputs and are never docs-only.
- **Always-on checks.** Public history / secret scan and `docs-check` (`git diff --check` plus relative Markdown link targets) run for every change set, including docs-only ones.
- **Caches.** Cargo registry and dependency build artifacts are cached with a SHA-pinned `Swatinem/rust-cache`, and Windows npm downloads with `setup-node`. Caches are keyed by toolchain/lockfiles, saved only from `main`, and only restored on other branches. The historical `v0.5.0` source build is not cached.
- **Release tags are unaffected.** `release.yml` has no change filter and no cache; every tag rebuilds and re-validates from scratch.

`.github/workflows/release.yml` is the **free public distribution path**, modeled after WebCodex Desktop's current macOS release approach. A tag-triggered run validates public history and source, builds the exact tagged source with Hardened Runtime and **ad-hoc signing**, packages an Apple Silicon DMG, smoke-tests the mounted DMG, publishes a SHA-256 checksum, and creates the GitHub Release. No Apple Developer Program membership or Apple release secret is required.

These free macOS artifacts are intentionally **not notarized**. Users may need to open **System Settings → Privacy & Security → Open Anyway** on first launch. Do not instruct users to disable Gatekeeper globally.

### In-app update contract

Packaged Chadex builds can update themselves from the latest stable GitHub Release. The app queries the repository's latest-release API and only accepts the exact free-release asset pair `Chadex-vX.Y.Z-macos-arm64.dmg` plus `Chadex-vX.Y.Z-macos-arm64.dmg.sha256`. Before shutdown it verifies the checksum, bundle identifier, version, Apple Silicon architecture, and macOS code signature, then stages the candidate beside the currently running app so write-permission failures are caught before quitting.

The final swap is performed by an external installer after Chadex has completed its normal helper/runtime shutdown. The previous app is kept as a temporary sibling backup until the new version launches and writes a version-bound health marker. If launch verification times out, the installer stops the candidate, restores the backup, and reopens the previous version. The updater never removes Gatekeeper quarantine or weakens the free release's existing ad-hoc-signing boundary.

`.github/workflows/release-notarized.yml` preserves the optional Developer ID + notarization path for a future paid distribution upgrade. It is manual-only so a normal free release tag does not trigger a failing Apple-signing job.

## Public-history gate

`./scripts/public_release_check.sh <ref>` rejects placeholder commit identity, scans history reachable from the ref for machine-local paths/device identifiers, and runs Gitleaks against only that ref. CI, the free release workflow and the optional notarized workflow all run it against `HEAD`. Optional local checks: `CHADEX_PUBLIC_REQUIRE_SINGLE_ROOT=1` asserts the ref still has exactly one root commit, and `CHADEX_PUBLIC_EXPECT_EMAIL` pins the author/committer email. CI sets `CHADEX_REQUIRE_GITLEAKS=1`, so absence of the dedicated scanner is a failure there. `.gitleaks.toml` extends the default rules with narrowly scoped allowlists for known synthetic test credentials and non-secret client-window correlation hashes.

## Distribution boundary

Do not publish `dist/` directly from an arbitrary development checkout. A public RC artifact must come from the exact public commit/tag that passed the source, public-history, secret-scan, package-shape, Hardened Runtime, DMG smoke, and checksum gates.

For the supported free path, ad-hoc signing is intentional and the DMG is not notarized. `scripts/package_free_macos_release.sh` is the canonical local DMG packager. Developer ID + notarization remains a stronger optional path, not a prerequisite for publishing Chadex on GitHub.
