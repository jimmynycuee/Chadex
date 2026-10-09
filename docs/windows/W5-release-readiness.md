# Windows Track W5 — Installer and Release Readiness

W5 closes the **automatable Windows release-engineering track** on top of the W1 runtime, W2 runtime E2E, W3 desktop product and W4 source-converged knowledge work. The current (v0.5.0) candidate keeps Windows as an **unsigned x64 CI validation artifact**: passing W5 does not by itself make Windows a signed or publicly supported binary release.

The public macOS release workflow is unchanged. W5 does not authorize a GitHub Release, Windows asset publication, Authenticode signing policy, or updater rollout.

## Candidate contract

`apps/windows/src-tauri/tauri.release.conf.json` builds a per-user NSIS installer with Traditional Chinese and English resources, downgrade blocking and the WebView2 downloaded bootstrapper. `scripts/build_windows_release.ps1` requires native Windows x64/MSVC, pinned source identity and a clean checkout, builds the helper plus all three runtime entrypoints in release mode, stages only verified production resources, and emits SHA-256/size metadata. The production desktop uses `custom-protocol` and explicitly excludes the `desktop-smoke` feature.

For v0.5.0 the same-binary synthetic migration fixture is `0.4.1 → 0.5.0` (`scripts/build_windows_release.ps1 -BuildUpgradeFixture` fixes the baseline to `0.4.1` and requires the candidate version to be `0.5.0`). It remains useful for deterministic installer-metadata migration checks, but it is **not** treated as historical application compatibility evidence.

The synthetic baseline, the historical tag and the version literals in the W5 scripts and CI move forward together at each version bump. When the version moves to 0.6.0 they become `0.5.0 → 0.6.0` and the `v0.5.0` tag; see the 0.6.0 prep PR (#10, `chore/v0.6.0-prep`).

## Source-integrity gate

`Windows W5 source integrity` runs before the expensive candidate build. It resolves the Tauri production configuration and exercises Windows Cargo build scripts, then requires the Git source to remain unchanged. The existing path-specific LF rule for `apps/windows/src-tauri/Cargo.toml` prevents Tauri's Windows TOML serialization from creating a false dirty-tree result without weakening the source-clean gate.

The release builder repeats source/HEAD checks around native builds and reports changed paths rather than restoring or hiding mutations.

## Installed production lifecycle

The hosted Windows acceptance harness is restricted to an ephemeral GitHub-hosted runner with no pre-existing Chadex application data or uninstall registration. It installs to Unicode/space-containing paths outside the checkout and launches the **production** executable from a different working directory.

The normal W5 candidate lifecycle verifies:

- candidate/resource/installer hashes and unsigned x64 metadata;
- synthetic previous-version installation and uninstall-registry ownership;
- installed helper/runtime default resource discovery without developer overrides;
- rendered production WebView state, runtime readiness, selected project, empty secure credential observation and absence of smoke IPC;
- upgrade, relaunch and same-version reinstall with preferences and project data preserved;
- controlled in-place uninstall with exact post-exit file assertions and registry removal;
- a second isolated install followed by the **default NSIS self-copy uninstaller** (no `_?=` override), requiring the install directory and uninstall registration to disappear while user preferences/project data remain unchanged;
- exact process identity tracking, zero residual owned processes and zero forced cleanup.

The WebView probe uses an ephemeral, app-specific 64-bit HKLM WebView2 policy value only on the disposable GitHub-hosted runner. It refuses pre-existing Chadex/AUMID/wildcard values, cleans owned application processes before removing the policy value, preserves unrelated policy values, and fails acceptance when exact cleanup cannot be verified. Raw backend errors, local paths and credentials are excluded from the sanitized report.

## Historical-source upgrade

W5 also runs a separate `Windows W5 historical upgrade` job after the tested current candidate is uploaded. This job:

1. checks out full release history and resolves the exact public `v0.4.1` tag;
2. creates an isolated detached worktree at that tag;
3. builds a real v0.4.1 unsigned Windows candidate from **v0.4.1 source**;
4. installs and launches that older executable/resource set;
5. upgrades it to the already-tested v0.5.0 candidate;
6. re-runs resource, state-preservation, relaunch, reinstall, uninstall, process-ownership and cleanup acceptance.

The historical report records both current and baseline source SHAs. This is deliberately separate from the synthetic migration fixture so one cannot be mistaken for the other.

## Evidence and acceptance

A successful W5 automated closeout requires the ordinary source/history/macOS package gates, Windows W2/W3 regressions, W5 source-integrity gate, current-candidate installed lifecycle, default self-copy uninstall and historical-source upgrade to all pass for the same candidate HEAD. The exact final SHA, GitHub Actions run, job outcomes, sanitized report counts and artifact identities are recorded in `W5_HANDOFF.md` and `evidence/W5_readiness_status.json` (under `docs/windows/`). Those records describe the v0.4.1 closeout run; the v0.5.0 run is the CI run of the release commit.

A CI candidate may be described as **automated W5 release-engineering complete** only after those final records are written. It must not be described as full Windows product acceptance or a public Windows release.

## Known intermittent failures

- `default_uninstaller_self_copy` stage reporting `process_inventory_failed` in the W5 installer job: PR #3 raised the process-inventory time limit from 15 s to 60 s and makes the failure print its details. A repeat failure should be read from those details rather than simply re-run.
- `Windows core gate` persistent-shell cold-start timeout: tracked in PR #11. A single timeout on a cold runner is not by itself a regression in the code under test.

## External acceptance that remains outside W5 automation

The following still need a real user environment, private credentials/hardware, or an explicit product/distribution decision and therefore remain outside automated closeout:

- physical Windows 11 interaction and Windows ARM64 hardware/build coverage;
- native picker, Explorer open, tray, launch-at-login and notification user-visible behavior;
- credentialed ChatGPT/Secure Tunnel workflow;
- trusted Authenticode signing and SmartScreen/reputation behavior;
- a host that genuinely lacks WebView2 and interactive bootstrapper/installer choices;
- Credential Manager deletion policy during uninstall (default application-data preservation does not define credential deletion policy);
- production Windows updater artifacts, authentication/signing and delivery policy.

`.github/workflows/release.yml` continues to publish only the free macOS Apple Silicon DMG. A Windows CI artifact remains unsigned validation evidence until a later explicitly authorized distribution phase changes that boundary.
