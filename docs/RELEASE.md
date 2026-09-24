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
CHADEX_APP_VERSION=0.1.0 \
CHADEX_APP_BUILD_NUMBER=1 \
CHADEX_CODESIGN_IDENTITY="Developer ID Application: ..." \
./scripts/build_app.sh
```

`CHADEX_APP_VERSION` and `CHADEX_APP_BUILD_NUMBER` must use decimal dot-separated values. Runtime packaging defaults to Cargo `release`; `CHADEX_RUNTIME_PROFILE=dogfood` is an explicit development-only override.

## Public Git history strategy

The private development history contains historical machine-local paths and connector/device identifiers. Do not publish that history unchanged.

For the first public release, Chadex uses a separate clean-history candidate branch whose root commit contains exactly the audited release-checkpoint tree. The private `phase6/runtime-core` history remains untouched for provenance. The clean public branch is created locally only; adding a remote or pushing it is a later distribution step.

Before the first public push, verify the public candidate branch independently: inspect its single-root history, scan that history for secrets and machine-specific identifiers, confirm its tree matches the release checkpoint, and run the release gate from a clean checkout of that branch.

## Distribution signing

`scripts/build_app.sh` supports four explicit signing modes through `CHADEX_CODESIGN_MODE`:

- `auto` (default): use an Apple Development identity when available, otherwise hardened-runtime ad-hoc signing;
- `development`: require Apple Development signing;
- `distribution`: require **Developer ID Application**, Hardened Runtime, and a secure timestamp;
- `adhoc`: hardened-runtime ad-hoc signing for CI package-shape validation only.

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

`.github/workflows/ci.yml` uses the supported ARM64 `macos-15` runner for source/package validation and an Ubuntu job for the dedicated Gitleaks history scan. Gitleaks is pinned to 8.29.1 and its archive checksum is pinned in workflow source.

`.github/workflows/release.yml` is the **free public distribution path**, modeled after WebCodex Desktop's current macOS release approach. A tag-triggered run validates public history and source, builds the exact tagged source with Hardened Runtime and **ad-hoc signing**, packages an Apple Silicon DMG, smoke-tests the mounted DMG, publishes a SHA-256 checksum, and creates the GitHub Release. No Apple Developer Program membership or Apple release secret is required.

These free macOS artifacts are intentionally **not notarized**. Users may need to open **System Settings → Privacy & Security → Open Anyway** on first launch. Do not instruct users to disable Gatekeeper globally.

`.github/workflows/release-notarized.yml` preserves the optional Developer ID + notarization path for a future paid distribution upgrade. It is manual-only so a normal free release tag does not trigger a failing Apple-signing job.

## Public-history gate

Run `./scripts/public_release_check.sh` on the public branch before publication. It rejects placeholder commit identity, scans reachable history for machine-local paths/device identifiers, and runs Gitleaks against only the selected public ref. For the first public root, set `CHADEX_PUBLIC_REQUIRE_SINGLE_ROOT=1`; `CHADEX_PUBLIC_EXPECT_EMAIL` can additionally pin the author/committer email. CI sets `CHADEX_REQUIRE_GITLEAKS=1`, so absence of the dedicated scanner is a failure there. `.gitleaks.toml` extends the default rules with narrowly scoped allowlists for known synthetic test credentials and non-secret client-window correlation hashes.

## Distribution boundary

Do not publish `dist/` directly from an arbitrary development checkout. A public RC artifact must come from the exact public commit/tag that passed the source, public-history, secret-scan, package-shape, Hardened Runtime, DMG smoke, and checksum gates.

For the supported free path, ad-hoc signing is intentional and the DMG is not notarized. `scripts/package_free_macos_release.sh` is the canonical local DMG packager. Developer ID + notarization remains a stronger optional path, not a prerequisite for publishing Chadex on GitHub.
