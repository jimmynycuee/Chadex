# Windows Track W3 — Desktop Product

W3 extends the verified W2 runtime with a Windows Tauri 2 + React + TypeScript application. Its branch is `windows/w3-desktop-product`, rooted at W2 final commit `c605691336d46f4f0e5d9efbee1d34f00ff22885`. W2 final CI `36990190294` was rechecked as successful before development. W3 acceptance remains pending until the final commit's Windows CI and the manual/private checks below have evidence.

## Architecture and authority

`apps/windows/src` → whitelisted Tauri commands in `apps/windows/src-tauri` → `apps/windows/bridge` → existing `chadex-helper` → existing server/runner/tunnel. The bridge preserves JSONL protocol v1, request IDs, structured responses, bounded frames, per-method deadlines and explicit shutdown. It does not cache a connected state, replay mutations or create a second runtime implementation.

The helper adds optional `runtime_status` and `tunnel_status` snapshot fields. They project the existing readiness and tunnel state; they add no state machine or lifecycle behavior. Older snapshots remain deserializable, and SwiftUI ignores the new fields. `runtime_ready`, tunnel readiness, and current-project ChatGPT verification remain separate facts. A failed/stale observation invalidates the frontend's usable/connected state.

The frontend has Home, Projects, Project Detail, Connection, Activity, Settings and Diagnostics. Runtime actions use existing helper methods, including `switchLocalProject`, `configureLocalSetup`, `connectChatGPT`, `disconnectAI` and `resumeService`. A tunnel available without current-project verification is shown as waiting for ChatGPT. Updates have an informational entry point only; no updater, installer, signing or release distribution is implemented in W3.

Code Ferret uses the canonical Swift state definitions and `Sources/ChadexApp/Resources/ferret-motion-*` assets through build-time projection. It is a display of observed state and never controls runtime execution. The Windows `.ico` is a format conversion of the canonical `ChadexIcon.png` using the pinned Tauri CLI, not a new character/icon source.

## Native integration and storage

- Project selection uses the native dialog plugin. Explorer opens only the helper's selected project through a canonical path argument, without shell interpolation.
- Tray has Show Chadex, current-status tooltip and Quit. Window close and tray Quit use the same asynchronous shutdown path. The bridge owns the helper process tree through the existing process crate/Windows Job Object; abrupt desktop termination closes kernel ownership handles.
- Login startup uses the native autostart plugin. Settings report the actual OS registration and persist only after successful changes. Notifications are limited to transitions into runtime/helper/connection failures.
- API keys use Windows Credential Manager (`app.chadex.windows.credentials` / `openai-tunnel-api-key`) and are passed to the existing in-memory helper credential method only when connecting. The frontend cannot read credentials or call `provideCredential` directly. Preferences contain Tunnel ID and ordinary settings only; unknown JSON fields are rejected.
- Preferences, recent projects and last selection live in the app-local data directory. Replacement uses a staged file and recoverable previous copy. Restart restores a selected project, never an old verified/connected indicator.
- Logs contain named desktop events and outcomes, not RPC arguments, responses, stderr, keys or raw OS errors. Diagnostic UI reads bounded runtime activity and safe state; there is no raw-config/credential export.

## Development and executable layout

Use Rust `1.98.1`, Node `>=22.12.0`, npm and Windows WebView2. Frontend dependencies are pinned in `package-lock.json`; native dependencies are locked in the two Cargo lockfiles. Exact resolved versions are recorded by CI/artifacts, not inferred from a requested range.

```powershell
# From repository root; keep the W2 runtime builds/gates intact.
cargo build --locked --manifest-path rust-helper/Cargo.toml
cargo build --locked --manifest-path chadex-runtime/Cargo.toml --bins
python scripts/prepare_windows_desktop.py
cd apps/windows
npm ci
npm run typecheck
npm test
npm run build
$env:CHADEX_DESKTOP_DEV_RESOURCES = (Resolve-Path src-tauri/resources).Path
npm run tauri dev
```

Production resource resolution is rooted in the executable's Tauri resource directory, with `helper/chadex-helper.exe` and `chadex-runtime/{chadex-runtime-cli,chadex-runtime-server,chadex-runtime-runner}.exe`. It does not search an installed macOS app or create another source/runtime fork. For a direct Cargo executable build, run the preparation script's desktop-output option to place those resources next to the executable. The development resource override is compiled out of release builds.

## Automated acceptance

CI retains the public-history secret scan, source/release sanity gates, macOS package gate and all W2 Windows gates, including core/helper tests, tunnel supervisor/official pinned asset tests and the 13-stage runtime E2E. W3 adds frontend typecheck/tests/build, bridge tests, production Tauri compile/build and a separate feature-gated desktop smoke executable. The smoke-only IPC is absent from production builds.

The real WebView smoke invokes the same commands used by the frontend, starts the real helper/runtime, performs an actual local MCP project operation, switches projects, tests credential-free connect rejection, restarts the helper, observes a forced helper crash, recovers and exits. A second application launch verifies persisted selection/preferences. Additional scenarios exercise startup failure and abrupt desktop termination. Process observations fence PID reuse with creation identity. Normal shutdown requires graceful bridge completion and zero remaining owned processes; any cleanup fallback makes the corresponding gate fail. Smoke Credential Manager entries are isolated from the user's production entry and deleted immediately after roundtrip validation.

`scripts/windows_desktop_smoke.py` writes a bounded projection: stage outcomes, scalar authority facts and cleanup counts. Credentials, token hashes, command output, raw logs, configuration and local paths are excluded. CI artifacts are the source for Windows environment/run details. Repo evidence must be sanitized, without relaxing Gitleaks.

## Manual/private acceptance still required

Record the exact commit, Windows version, WebView2 version, tester, time and outcome. Do not mark W3 complete based on UI unit tests or credential-free local MCP alone.

1. Launch the production Windows desktop executable and select a folder through the native picker; cancel and select a Unicode/space-containing path. Verify selected folder and recent-project entry.
2. Open the active folder in Explorer. Verify tray Show, actual status, window close and tray Quit. Observe no owned helper/server/runner/jobs remain.
3. Enable and disable launch-at-login; verify actual startup and restored settings. Test notifications on a background failure and verify normal activity causes none.
4. Use a restricted key through Connection settings and a valid Tunnel ID. Save/restart; verify Credential Manager reuse without plaintext preferences/logs. Connect ChatGPT to the existing secured tunnel and perform a real project read/edit/terminal operation. Verify Activity and current-project verification.
5. While connected, switch projects repeatedly; verify new project authority, readiness and continued connector operation. Disconnect/reconnect and test a tunnel/network failure; a failure must never retain connected UI.
6. Restart the application, verify selected project/preferences and no stale verified indicator. Verify Code Ferret enable/disable and observed working/waiting/success/error without interfering with the workflow.

Credentialed ChatGPT/OpenAI evidence must remain private or sanitized. Public CI never embeds a key or fakes verification. W3 stops here; W4 source-converged Graphify/Obsidian work and W5/release/distribution require a new instruction.

## Evidence status

[W3 validation evidence](evidence/W3_validation_status.md) records implementation SHA `3ecaf6e0b647d27db5e53aa6ae1385aff8bc5cd2`, full successful CI run `37026168474`, exact versions, Windows test/build results, W2 13/13 stages, W3 27/27 real WebView stages and lifecycle cleanup 56→0 with no forced cleanup. Sanitized projections are committed beside the report; canonical raw W2 observations remain in Actions.

The user has no Windows host and chose to retain the six native/private categories above as `not validated`. The automated implementation is verified; full W3 product acceptance remains pending. Credential Manager isolated roundtrip/deletion passed; credentialed ChatGPT/tunnel workflow remains unverified. The production-feature artifact is an unsigned direct-Cargo debug executable without smoke IPC, not a release/installer.

The evidence commit must also receive full CI on its own final HEAD; the final handoff identifies that exact SHA/run. No early implementation run substitutes for final-HEAD CI. W4 may start from the final pushed W3 commit only after a new user instruction; outstanding W3 manual/private acceptance remains explicit.
