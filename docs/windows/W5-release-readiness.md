# Windows Track W5 — Installer and Release Readiness

W5 starts on `codex/w5-release-readiness` from W4 final source `c3eedfc537bf7aae55cb2e264f3d15b9c4cbf09b`. [W4 CI 37045486251](https://github.com/jimmynycuee/Chadex/actions/runs/37045486251), attempt 1, was rechecked as successful on 2026-10-03 (Asia/Taipei): all four jobs passed. W4 reports verified W2 13/13 and W3 27/27 stages, 55 observed desktop process identities, zero residuals and zero forced cleanup.

This stage prepares an **unsigned x64 installer candidate** and its automated lifecycle evidence. Full product acceptance and public distribution remain pending. The existing macOS release workflow is unchanged. W5 does not authorize merging `main`, creating release tags or publishing a release.

## Build contract

`apps/windows/src-tauri/tauri.release.conf.json` overlays the existing development configuration. It enables a per-user NSIS installer with Traditional Chinese and English, blocks downgrades and uses the WebView2 downloaded bootstrapper. Installation may need internet when WebView2 is absent. These settings follow the [Tauri Windows installer documentation](https://v2.tauri.app/distribute/windows-installer/); the fixed CLI 2.8.4 template supplies the install/uninstall behavior.

`scripts/build_windows_release.ps1` requires a native Windows x64 MSVC toolchain and clean source, including non-ignored untracked files. It pins the source SHA before building and rechecks source/HEAD before emitting the candidate. Output must be outside the checkout. It builds the helper and all three standalone runtime entrypoints with `--release --locked`. A separate Cargo target directory keeps debug/smoke builds out of this package. The release resource stager validates PE architecture and version agreement, produces a SHA-256/size manifest and ships Chadex's license, upstream WebCodex license and `UPSTREAM.md`. There is no debug binary fallback.

Tauri CLI builds the production application with embedded frontend assets and `custom-protocol`, without `desktop-smoke`. The candidate records the exact source SHA, desktop/installer hashes, architecture and features. Actual Authenticode checks must observe unsigned executables before labeling this artifact unsigned. `createUpdaterArtifacts` is false: there is no updater feed, signing key or automatic update implementation.

JavaScript `@tauri-apps/api` and the locked Rust `tauri` crate use 2.12.1; CLI stays fixed at 2.8.4. Input preflight checks npm lock agreement and the API/crate minor versions before compiling, following [Tauri's dependency synchronization requirement](https://v2.tauri.app/develop/updating-dependencies/). The first W5 CI exposed the previous 2.8.0/2.12.1 mismatch; the check remains enabled.

On Windows, after installing the pinned Rust/Node toolchain:

```powershell
.\scripts\build_windows_release.ps1 -OutputDirectory "$env:TEMP\chadex-w5-candidate" -BuildUpgradeFixture
```

The output directory must be new. Candidate version `0.3.2` denotes the current development source, not a new published stable Windows release. The installer name explicitly includes `windows-x64-unsigned`. The synthetic baseline wraps the **same production executables** with older installer metadata `0.3.1`; it is an installer migration fixture, not a historical Chadex release or source compatibility test.

## Automated installed acceptance

The CI `Windows W5 installer candidate` job builds and tests on the Windows GitHub runner. All existing public-history, source, ARM64 macOS package and Windows W2/W3 gates remain required. Installer output is uploaded only after its installed lifecycle test succeeds. The synthetic baseline is excluded from uploaded candidate files.

The mandatory `Windows W5 source integrity` job first runs Tauri configuration resolution with an intentional no-build runner, then `cargo check` to exercise Windows build scripts. Resource copies are excluded only in this early diagnostic; the full release build still stages and verifies all production resources. Both checks require unchanged HEAD and clean source. The release builder also checks cleanliness after each native build and prints changed repository paths on failure. This does not restore or hide mutations.

CI 37102421007 isolated a modified `apps/windows/src-tauri/Cargo.toml` immediately after Tauri configuration resolution, with no content diff and an LF/CRLF warning. A path-specific `.gitattributes` rule keeps that manifest in LF on Windows, matching Tauri's TOML serialization. It changes no dependency or feature and leaves all source-clean checks enabled.

The installation harness requires `RUNNER_ENVIRONMENT=github-hosted`, refuses ordinary/self-hosted user hosts and pre-existing Chadex installation/data on the ephemeral runner. It installs to a new Unicode/space-containing path outside the source tree, launches the production executable from another working directory and tests:

- Installer/resource hashes and the installed helper/runtime's default discovery, without developer or smoke resource overrides.
- Rendered production WebView showing the selected project and service readiness without a state error, real helper/runtime readiness, successful empty credential-store observation and absence of smoke IPC.
- Synthetic older-metadata installation → candidate upgrade, selected project and preferences retained on relaunch, and same-version reinstall preservation.
- Graceful shutdown with exact process identities, zero residuals and zero forced fallback.
- In-place uninstall removes application resources and registry entries while retaining preferences and user project data. The harness removes the original uninstaller only after its process exits; fixture cleanup is evaluated separately.

For the installed WebView probe, the ephemeral runner temporarily owns only the `Chadex.exe` value under 64-bit `HKLM\Software\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments`, with a loopback CDP port. WebView2 150 [ignores environment/HKCU overrides in elevated hosts](https://github.com/MicrosoftEdge/WebView2Feedback/issues/5645#issuecomment-4934355430); [Microsoft documents the app-specific HKLM policy](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/webview2-idl?view=webview2-1.0.3912.50). The fixture refuses pre-existing app/AUMID/wildcard values, verifies the exact owned value immediately before removing it after owned app-process cleanup on success or failure, and fails acceptance if cleanup cannot be verified. It never deletes policy keys or other applications' values; an empty container may remain on the disposable host. The candidate does not embed CDP arguments or test IPC. The Node driver calls existing production commands; paths and PIDs returned to the wrapper stay local. The public report contains bounded stages, counts and flags, including verified policy-value removal. This does not prove physical Windows native interactions or a credentialed external workflow.

The harness invokes NSIS with an explicitly selected executable and a shell-free command line. Its special `/D=` and `_?=` directory parameters are last and have no quotes around the path, as required by the [NSIS command-line documentation](https://nsis.sourceforge.io/Docs/Chapter3.html#3.2), including paths with spaces.

`_?=` disables the uninstaller's normal self-copy to a temporary directory. This controlled in-place test can wait on the original process, but does not certify the default self-copy workflow. The running original executable cannot be treated as a product-deleted file; its post-exit removal is explicitly harness cleanup.

`windows_suspended_launch.py` creates each installer, uninstaller and desktop with [CREATE_SUSPENDED](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags). A temporary inventory must observe the original `Popen` alive before and after capture, with the expected executable name and creation identity, before any sample enters cleanup ownership. The harness then obtains the sole primary thread through [Toolhelp thread enumeration](https://learn.microsoft.com/en-us/windows/win32/toolhelp/thread-walking), checks its owning process and resumes that same handle. Existing W4 descendant and termination identity checks remain unchanged. A failed initial sample or resume fails acceptance; killing the original `Popen` handle is counted as forced cleanup.

## Publication gate and remaining acceptance

These W3 manual/private categories remain **not validated** until actual tester evidence exists: `native_picker`, `explorer_open`, `tray`, `launch_at_login`, `notifications`, `credentialed_tunnel`. Record exact source SHA, Windows/WebView2 version, tester, time, actions and outcome in private or sanitized evidence. Do not put API keys or tunnel credentials in public CI artifacts.

Further release prerequisites remain explicit:

- Physical Windows 11/x64 acceptance; Windows ARM64 builds/hardware are outside this candidate's certified scope.
- Trusted Authenticode signing and its actual verification; unsigned CI artifacts do not establish SmartScreen reputation.
- Missing-WebView2/bootstrapper behavior, the default uninstaller self-copy workflow and interactive installer/uninstaller choices on a real machine.
- Credential Manager cleanup policy and manual data deletion choices; default uninstall preservation does not prove credential deletion.
- Historical source-version upgrade compatibility and recovery/rollback. The synthetic installer fixture checks installer metadata migration only.
- Automatic updater implementation, authenticated/signed update artifacts and delivery policy, if that distribution method is adopted later.

CI success establishes automated installer engineering evidence. Public release requires the outstanding product acceptance, a chosen signing/distribution policy and explicit publication authorization. No tag-triggered Windows publishing path is introduced by W5.
