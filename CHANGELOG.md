# Changelog

All notable public Chadex releases are summarized here. Detailed notes remain under `docs/releases/`.

## 0.6.0 (planned, not yet released)

Expected to include the following open PRs (jimmynycuee/Chadex); none are merged yet.

- Design v2 for Windows (#1): Fluent type scale, connection circuit, activity timeline, Mica backdrop and dark as the default appearance. Verified in CI only; there has been no physical-machine testing.
- Design v2 for macOS (#2): Liquid Glass sidebar, connection circuit, activity timeline, dark as the default appearance, and a fix for building on the macOS 15 SDK. Pending validation: reviewed by AI reviewers only, with no external design validation yet.
- Computer Use page permission card for Accessibility and Screen Recording (#4).
- Computer Use tool descriptions now call `activate_window` only before key or pointer actions (#5), and every Computer Use action records its elapsed time (#6).
- W5 smoke prints unexpected exceptions, and the `process_inventory` time limit goes from 15 to 60 s (#3).
- Runtime tests no longer race on environment variables (#7).
- Candidates, depending on what merges: macOS cursor overlay (#8) and Chromium web-page AX (#9). Both were originally planned for 0.6.1 and are not included yet.
- Public distribution remains the macOS Apple Silicon DMG; Windows stays an unsigned CI candidate. See `docs/releases/0.6.0.md`.

## 0.5.0

- Added **External Skill sources**: connect Agents, Claude Code or Codex Skill folders at the Runner level; external scripts are not executable by default and are enabled one source at a time.
- Added a standalone macOS **Skills** page with a compact list, search and filters, per-Skill enable switch and Remove (with confirmation); Windows reaches Skills UI parity.
- Skill ZIPs can be imported from any location (staged under `.chadex/skill-imports/` and deleted afterwards), single-top-level-folder ZIPs are flattened automatically, and failures give concrete messages.
- Added a local admin token (`chadex-desktop-admin`) for Skill and Project Memory management; the tunnel ingress refuses management tools.
- Prepare the local runtime in the background at launch (on by default) on macOS and Windows, align the prewarmed runtime to the selected project, and speed up connect feedback; baseline in `docs/performance/v050-launch-connect-baseline.md`.
- Fixed the Project Memory catalog limit, activation replay idempotency, errors swallowed by refresh, and Skills loading around launch.
- Known limitations: a ChatGPT session with the bootstrap token still has full permissions and ingress filtering is not a permission boundary (tunnel token scoping is a separate task); hidden `$` shares cannot be Skill sources and a few local UNC aliases are not yet blocked; Windows is CI-verified only; removing a Skill affects every project on the local runtime; `credential_push` may log an error at startup (under investigation).
- Public distribution remains the macOS Apple Silicon DMG; Windows stays an unsigned CI candidate. See `docs/releases/0.5.0.md`.

## 0.4.1

- Added **Chadex Global Instructions** as app-managed behavior preferences shared across projects, editable even without an active project; repository `AGENTS.md` files remain repository-native and are no longer duplicated as a Chadex-managed project-instructions UI.
- Stopped ambient ancestor `AGENTS.md` files outside the registered project root from influencing Chadex projects, while preserving project-root and target-scoped nested repository instruction hierarchy.
- Defined behavior precedence as current user request → nested/root repository AGENTS → Global Instructions → built-in baseline, inside the existing non-overridable safety/authority envelope.
- Redesigned macOS Computer Use settings with clearer progressive disclosure, a stopped-state-only Resume action, persistent **Always allow**, temporary **Allow this session**, and Stop that remains in force across tunnel reconnects until explicit resume.
- Removed the redundant Repository Instructions block from project detail while preserving repo-root/nested `AGENTS.md` runtime behavior, and suppressed stale/transient runtime errors during an in-progress cold-start connection.
- Removed the redundant Code Ferret status strip from the sidebar and moved companion visibility/animation preferences into General Settings.
- Fixed first-time connection setup so saving Tunnel ID/API key performs the full connection lifecycle immediately instead of leaving a credential-only error state that required Retry.
- Fixed Secure Tunnel false-ready detection: Chadex now waits for authenticated control-plane metadata after `/readyz`, fails fast on rejected credentials, and directs credential errors to Connection Settings instead of waiting through 10-second poll backoff cycles.
- Fixed a first-connect state race where background status polling could leave the UI on a false Retry screen even though the Tunnel had already connected successfully.
- Fixed Connection Settings so an explicit Save with complete credentials and a selected project always initiates the connection lifecycle instead of preserving a stale disconnected state merely because credentials were already stored.
- Closed the automatable Windows W5 release-readiness gaps while keeping Windows distribution private to CI artifacts.
- Added real historical-source `v0.4.0 → v0.4.1` installed-upgrade acceptance instead of relying only on the same-binary synthetic metadata fixture.
- Added a separate native NSIS default self-copy uninstall stage while retaining strict process-identity, zero-residual and zero-forced-cleanup requirements.
- Synchronized Chadex-owned product/runtime metadata to 0.4.1 and advanced the synthetic Windows migration baseline to 0.4.0.
- Kept physical/private Windows interaction, signing, missing-WebView2, credential-uninstall policy and Windows updater/distribution decisions explicitly outside automated acceptance.
- See `docs/releases/0.4.1.md` and the W5 handoff for exact validation evidence and boundaries.

## 0.4.0

- Added a streamlined **Agent Settings** workspace that combines project instructions (`AGENTS.md`) and Skills while keeping their runtime contracts separate.
- Added safety-gated **Computer Use** with read-only observation, semantic control actions, approval modes, emergency Stop, sensitive-field protection, stale-identity fencing, and outcome-unknown recovery.
- Productized Project Memory as a background capability: authorized bootstrap summaries are loaded automatically for coding tasks, durable architecture/decision/workflow knowledge can be maintained across sessions, and manual inspection stays behind an advanced project action.
- Made root `AGENTS.md` creation and the Read-only / Ask-before-control Computer policy configurable before ChatGPT connects; session-only Computer permissions remain gated by an active tunnel.
- Hardened Computer acceptance and macOS TCC identity handling, and added Windows parity/live-check coverage. The public v0.4.0 workflow still publishes the macOS ARM64 DMG; Windows installer candidates remain CI validation artifacts.
- See `docs/releases/0.4.0.md` for validation and distribution boundaries.

## 0.3.3

- Enabled ChatGPT host-file imports for Chadex's desktop-owned loopback Server when accessed through the OpenAI Secure Tunnel with a normal user API token; non-loopback and untrusted credential paths remain rejected.
- Made automatic update discovery run once after each app launch and again when the app becomes active after the six-hour interval, so a newly available release exposes the blue **Update** button next to Settings without requiring a manual check first.
- Kept automatic discovery quiet while preserving the existing manual **Check for Updates…** flow and install safeguards.
- Carried the Windows W1-W5 source work into the release tree, including runtime/desktop integration, installer/source-integrity gates, and 0.3.2 → 0.3.3 upgrade-fixture metadata. The public v0.3.3 release workflow still publishes the macOS ARM64 DMG; Windows installer candidates remain CI validation artifacts.
- See `docs/releases/0.3.3.md` for validation and distribution boundaries.

## 0.3.2

- Distinguished idle connection verification from actual queued, blocked, recovering, or cancellation waits.
- Retained fast completed tool activity for two seconds with an explicit recent-activity label; new work supersedes older terminal reactions.
- Made fresh listening visible for one second and prevented restored work or long waits from replaying that reaction.
- Added actual runtime symbol-navigation, file-edit, and Go-test mappings. Scoped live tools take precedence over generic process Job activity without inheriting unrelated wait duration.
- Preserved failures when tasks and Jobs finish together; kept diagnostic polling from waking the companion.
- Organized local fixtures/worktrees and refreshed Graphify/Obsidian knowledge. Runtime polling, animation rate, character assets, and durable-job execution contracts are unchanged.
- See `docs/releases/0.3.2.md` for validation and distribution boundaries.

## 0.3.1

- Added the animated Code Ferret mascot, driven by observed tool activity, task progress, and completion/failure evidence.
- Added mascot visibility and motion controls, macOS Reduce Motion support, and English / Traditional Chinese labels.
- Kept unknown progress indeterminate and reset mascot reactions when switching projects.
- Added optional, bounded current-project durable Job observation so completed HTTP responses do not make active work appear idle. Diagnostics do not wake the mascot.
- Validated the native UI, source gate, packaged long-job success/failure, DMG resources and signatures; see `docs/releases/0.3.1.md`.

## 0.3.0

- Increased the model-facing long-job observation window from the previous short slicing behavior to a 60-second bounded wait while preserving event-driven terminal/failure wakeups.
- Preserved durable-job identity across observation timeout and truncated output so long work is resumed/observed instead of restarted.
- Added bounded terminal handoff/follow-up correlation for diagnosing `local terminal -> next host/model action` stalls without replaying completed local work.
- Kept the existing in-app updater and free ad-hoc-signed macOS release path; no migration is required from 0.2.x.
- Validated the release direction with paired short/medium benchmarks and a 10–20 minute endurance repository workflow including one ~5-minute bursty validation and one ~6-minute silent validation, with no duplicate long-job execution or terminal stall in the formal run.

## 0.2.3

- Fixed duplicate main windows when reopening Chadex from the Dock after the last visible window was closed.

## 0.2.2

- Hardened packaged resource lookup and updater presentation behavior.

## 0.2.1

- Fixed updater shutdown/reopen reliability.

## 0.2.0

- Added the public in-app updater and long-job observation improvements.

See `docs/releases/` for per-release validation and upgrade notes.
