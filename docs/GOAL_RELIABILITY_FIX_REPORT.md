# Goal execution environment and checkpoint reliability

Status: local implementation and automated integration validation completed, 2026-10-15.
Native human/connected acceptance and production authority remain separate gates.
Branch: `codex/goal-start-singleflight`; based on v0.7.1 with the separately
committed Goal-start singleflight fix. This document does not announce a release.
No production Goal, financial policy, scheduled task, remote database, website,
installed application, push, merge or deployment is changed by these tests.

## Confirmed causes

- The v0.7.1 `GoalProvider::check_sandbox_prefix` built acceptance around the
  bare executable `codex`. A ChatGPT Goal therefore inherited a PATH dependency
  unrelated to its coding agent. Tool availability was not verified before the
  checkpoint. `run_op` correctly failed closed on a pre-dispatch spawn failure,
  but the run lacked an acceptance-only recovery operation.
- One network-disabled policy was used for unit tests and localhost Worker/D1.
  It could not provide private service ownership or Wrangler's scoped state.
  Opening the host's loopback ports would also expose unrelated local services.
- The stored iteration meant a completed acceptance round, not a completed Git
  commit. Legacy staging and commit operations both used `kind=commit`; migration
  must identify exact `commit:<round>@g<generation>` keys, never count `stage`.
- Ignored inputs/dependencies were not part of approved workspace preparation.
  Merely finding a lockfile or copying a directory does not establish provenance.
- External authority and executable success were conflated. Even `exit 0` from
  agent-editable checker code cannot establish real owner authorization.
- ChatGPT idle activity was primarily tied to `goal_check`. The new code counts
  successful, scoped changes and verified running jobs; read-only polling is not
  activity. This is a measured regression fixture, not a claim that every
  historical long-running job was falsely paused.

Relevant current implementations:
`runtime-engine/src/tool_runtime/{goal_environment,goal_acceptance_sandbox,goal_container,goal_workspace_gate,goal_supervisor,plan_mode}.rs`,
`runtime-engine/crates/chadex-runtime-store/src/{plan,goal_run,goal_attestation}.rs`,
and `Sources/ChadexApp/PlanGoal{Models,PanelModel,View}.swift`.

Current source anchors (line numbers at this repair):

| Concern | Source / function |
| --- | --- |
| Preflight before work and checkpoint | `goal_supervisor.rs:566` (`drive_inner`), `:1760` (`RuntimeGoalExecutor::preflight`); `goal_environment.rs:75` (`probe_spec`) |
| Bounded input/staging transport | `goal_environment.rs:317` (`prepare_inputs_spec`), `:385` (`checkpoint_stage_spec`) |
| Long-path native preflight | `goal_acceptance_sandbox.rs:139` (`preflight_with_read_only`) |
| Queued ChatGPT Resume scheduling | Store `goal_run.rs:1096` (`goal_run_claim_excluding`) |
| Immutable revisions and operator recovery | Store `goal_run.rs:2182` (`goal_acceptance_revision_propose`), `:2317` (`goal_acceptance_revision_approve`), `:2585` (`goal_run_control`) |
| Trusted observations and external owner evidence | Store `goal_run.rs:2008` (`goal_acceptance_receipt_record`); `goal_attestation.rs:56` (`verify`) |
| Activity / waiting state / UI retry | `plan_mode.rs:1156`, `:2013` (`goal_run_projection`); `PlanGoalModels.swift:380` (`canRetryPreparation`) |

## Execution and security decisions

### Preflight and lanes

All filesystem/toolchain probes and preparation run on the bound Runner, not on
server-local assumptions about a remote filesystem. A fixed Python script runs
with `-I -S`; no project code is imported. It resolves the actual executable,
uses bounded structured stdin with a small fixed bootstrap (no top-level shell mode),
checks the native sandbox, validates declared input digests and executable/cwd
availability, and records bounded capability metadata before agent work.
Large approved dependency preparation has a bounded 300-second per-batch limit;
active preparation is additionally clipped to the original Goal deadline.
Cancellation/pause/deadline checks also run while a trusted environment job is
being observed. An unconfirmed stop remains unknown and cannot become a safe retry.
Parked operator metadata inspection remains possible after the original deadline.
The waiting-for-ChatGPT projection is withheld while preparation holds a lease
or the run is queued. Explicitly resumed queued ChatGPT workspaces are reclaimable
without inventing a completed ChatGPT turn; an idle running workspace is not
reclaimed repeatedly.
ChatGPT coding does not require a Codex model invocation. The background Codex
provider resolves its own executable and retains its original hardening flags.
Every `CHADEX_GOAL_CHECK_SANDBOX_ARGV` override, including an empty or malformed
one, is rejected. There is no unsandboxed acceptance fallback.

`offline_sandbox` uses the reviewed macOS deny-by-default native profile, an
environment allowlist and scoped HOME/config/cache/tmp/log directories in
`.goal-acceptance`. Personal homes, inherited credentials, outbound network,
localhost and writes outside the workspace are denied. Necessary system and
installed toolchain roots are read-only; this is not an allowance for personal
configuration roots. Hash-approved private input/dependency paths are read-only,
including rename, unlink and hardlink attempts in the native controls.
`PYTHONDONTWRITEBYTECODE=1` prevents Python acceptance imports from adding derived
bytecode to a later checkpoint or modifying already tracked bytecode. The final
tracked-cleanliness test remains strict. Both native preflight and acceptance
transport the full policy in bounded stdin, including long managed-worktree paths.
The fixed isolated Python launcher writes an owned temporary 0600 policy file,
executes the actual argv inside sandbox-exec, relays stop signals and removes its
policy file. Policy/argv transport stays within existing Runner limits.

`loopback_integration` with an explicitly approved `container_image` uses an
already provisioned local Linux Docker daemon over an explicit Unix socket.
The trusted server's `CHADEX_GOAL_CONTAINER_POLICY` must authorize the exact
local image ID; no tags, automatic pulls, host network, host ports, Docker socket,
home mounts, privileged containers, capabilities or inherited credentials are
allowed. The worktree is mounted read-only. Only private in-container loopback
is available under `--network none`; ephemeral internal Miniflare sockets cannot
reach host/backend services. Rootfs is read-only, user is non-root, capabilities
are dropped, and process/CPU/memory limits apply. Temporary state lives under
`/workspace/.goal-acceptance` and a bounded `/tmp` tmpfs.

Before mounting, the Runner rejects host sockets, devices, FIFOs and escaping
symlinks in the workspace. Within the runtime, a canonical Runner/root mutation
barrier drains admitted writes, blocks project aliases from concurrent mutation,
and waits for already registered jobs. Other projects remain independent.
The supervisor verifies the checkpoint again before success. Arbitrary external
editors/other applications are outside this in-process barrier; tracked changes
are detected, not silently committed during an acceptance-only retry. This is
not an atomic snapshot of the entire host filesystem.

A native host-port-only loopback configuration remains fail-closed because port
numbers alone cannot establish endpoint ownership or exclude listener replacement.
The test-only exact-port candidate is not a production backend or a Goal receipt.

Container creation is journaled before dispatch. The actual ID, Created timestamp,
image and Goal/commit labels are verified and persisted before start. `docker wait`
provides the actual container exit; successful attach CLI exit is insufficient.
Cancel/timeout/lost-fence cleanup re-inspects and removes only that exact owned
identity. A durable recovery query is independent of event pagination and includes
parked/terminal runs after lease expiry. Uncertain creation/ownership is preserved
for operator reconciliation, never blindly replayed or removed by name. Retry is
blocked while known container cleanup is pending. Runtime shutdown/crash recovery
is distinct from a successful test result.

### Checkpoints, retries and approved revisions

The Store persists `checkpoint_sha` (40/64-character Git OID), committed rounds,
completed acceptance rounds, attempt and acceptance revision separately. Migration
preserves legacy Plan hashes through omitted/default serde fields and leaves an
unverified legacy checkpoint SHA empty. A local operator may explicitly adopt it
only after trusted Runner checks of HEAD, original base ancestry, expected Goal
commit subject, successful commit journal and tracked cleanliness.

`retry_acceptance` is operator-only and checks the parked state, successful commit,
known outcome, exact checkpoint, deadline and emergency stop. It queues the same
Goal/checkpoint with a new attempt; it does not rerun agent work or commit again.
The UI uses a confirmation step and disables generic Resume for `needs_attention`.
Unknown outcomes and unresolved cleanup do not receive an automatic retry.

Acceptance changes use immutable proposals and a second explicit local approval
of proposal ID, source revision, checkpoint and complete spec SHA-256. The hash
includes checks, private-input requirements and dependency provenance. Historical
revisions and original Plan approval remain intact. Required checks/inputs cannot
be removed; external checks cannot be downgraded, and external command/cwd and
owner/action/evidence identity remain fixed. New receipts cannot be reused across
revision, checkpoint or attempt. Scoped-input stdout/stderr is redacted in the
operator/model projection, including container attach results.

### Scoped inputs and dependencies

A workspace requirement is a project-relative source, exact SHA-256, destination
and `read_only` permission in the human-approved specification. Preparation uses
held directory/file descriptors and `O_NOFOLLOW`, rejects traversal, special files,
source/destination symlinks and shared destination inodes, validates before copying,
and never changes the source. Private destinations and dependency paths are
explicitly excluded from Chadex's checkpoint staging, even if not gitignored.
Staging enumerates tracked/nonignored paths with literal path handling instead
of passing ignored negative pathspecs to `git add`. It clears accidentally staged
private paths from this worktree's index without changing their files or the
source project's index. Real Git regressions cover ignored inputs, pre-staging,
deletions and metacharacter filenames.

A dependency declaration pins both its lockfile and complete tree:

```
sha256:<lockfile-sha256>:<relative-lockfile-path>:<tree-sha256>
```

Generate a reviewable declaration without installing or executing package code:

```sh
python3 scripts/goal_dependency_manifest.py --project /path/to/project \
  --dependency g1/mcp/node_modules --lockfile g1/mcp/package-lock.json
```

The tree digest has ordered JSON records for directories, internal symlink targets,
regular-file SHA-256 and executable bits. The fixed reader rejects escaping links,
sockets/devices and oversized trees. A missing destination is copied only from
the same bound source project with the approved digests, staged outside the managed
worktree, then installed with an exclusive rename. No global install, network or
unapproved replacement occurs. An existing destination must match the full tree.
A legacy lockfile-only provenance string can still be read, but is not READY.
Inputs are revalidated after agent work and before independent acceptance.

### Trusted receipts and external authority

The Store signs actual Runner observations using its persistent private HMAC-SHA256
key. Each immutable receipt binds Goal, acceptance revision, checkpoint, attempt,
check, approved argv digest, actual exit/timeout, captured-output digest and time.
The secret is never returned. Read-back and Goal success verify receipt integrity;
Agent JSON cannot create a passed receipt. Local HMAC receipts are not public-key
certificates or proof of real external owner authority.

An `external_gate` requires the `external_attestation` lane and a complete
owner/action/evidence/next-step manifest. Exit 2 stays `blocked_external`.
Exit 0 without trusted detached authorization is also `blocked_external`; its
actual exit code remains 0 in the receipt. Independent checks continue according
to the approved dependency DAG, but the entire Goal cannot succeed.

Real external authority is configured separately with public keys only:
`CHADEX_GOAL_EXTERNAL_ATTESTATION_KEYS` is an operator-managed array containing
`owner`, `project_root`, `runner_client_id`, `public_key_hex`. No signing key is
created or stored by Chadex. A fixed Runner reader opens the manifest's evidence
path beneath the workspace without following links. The file contains
`statement`, `signature_hex`, and project-relative `artifact_path`; the reader
independently hashes the actual bounded artifact, without returning its contents.

Ed25519 signs the byte prefix `chadex.external-gate.v1\0`, followed by compact
UTF-8 JSON with the following typed field order:

1. `goal_run_id`, `acceptance_revision`, `checkpoint_sha`, `check_name`
2. `argv_sha256`, `output_sha256`, `artifact_sha256`, `owner`, `action`
3. `exit_code`, `not_before_unix_ms`, `expires_at_unix_ms`

The signature, exact current scope/output/artifact, configured owner/Runner/project,
actual successful exit, time window and at most 24-hour validity must all match.
Missing, synthetic, forged, expired or cross-scope evidence remains blocked. The
cryptographic tests use synthetic keys only; they are not G1 owner approvals,
financial parity, remote restore or Cloudflare Access evidence.

## Validation ledger (A–J)

Evidence files below are local ignored files under `dist/goal-reliability-evidence`.
A command is considered passed only after its actual successful completion.

| DoD | Executed evidence | Current result |
| --- | --- | --- |
| A | PATH-excluding-local-Codex probe; missing executable fixture; native preflight; fresh actual-Codex Goal | Passed. Preflight failures precede agent/commit; actual provider then succeeded in one agent dispatch |
| B | Native `smoke.py`: 33/33 real controls; 23/23 standalone Rust unit tests | Controls passed. Combined script exits 2 for native Worker/D1 NOT_VALIDATED; the supported private Linux lane is verified separately under C |
| C | Docker standalone: 30/30 controls, 14/14 child controls, actual Worker→D1; fresh helper/Runner Goal | Passed, exit 0. Real D1 Goal succeeded; immutable Store receipt verified actual exit 0, current checkpoint and HMAC proof |
| D | Unknown native loopback ownership, unsafe overrides, missing tools and unavailable capabilities | Fail-closed controls passed. No empty-prefix or unsandboxed fallback |
| E | Controller pre-dispatch spawn fault; actual helper preparation repair/resume; actual checkpoint-only retry | Passed. Same Goal and budget retained, no replayed agent/checkpoint commit. Human authorization is represented only by the synthetic local operator test route |
| F | Store immutable history/CAS/hash tests; UI serialization; actual helper proposal/approval/retry | Passed. Model approval and forged hash rejected; exact operator approval produced revision 2 with retained checkpoint/history |
| G | `test_goal_environment.py` (13); real dependency-tree copy/hash in isolated helper; Git staging regressions (3) | Passed; source fixture remained untouched. No real G1 private evidence was copied or approved |
| H | Actual strict exit-2 gate remains blocked after independent check; unsigned exit-0 rejection; Ed25519 scope/artifact/expiry tests | Passed as honest blocking. Synthetic cryptographic keys do not constitute owner, parity or remote acceptance |
| I | Verified scoped activity/jobs, lease/idle/deadline tests; environment control tests; queued-resume regression; UI counters; mutation barrier | Passed with deterministic time/fault injection. No claim of an actual 30-minute wall-clock/human idle test |
| J | Swift 307 tests (1 skipped, 0 failures); Store 213; Supervisor 23; Plan 32; contracts 142 + 96; environment 5; mutation gate 1 | Passed at these scopes. Fresh standard helper E2E passed all 10 assertions; reliability E2E passed 32/32 checks; actual Codex Goal passed once, with only the intended synthetic file changed |

The single Swift skip is the existing live ScreenCaptureKit/display test
`ComputerOverlayLiveCaptureTests.testOverlayWindowIsExcludedFromScreenCaptureKitAndReportsNoSharing`,
which requires `CHADEX_LIVE_CAPTURE_TESTS=1` and Screen Recording permission.
It is unrelated to Goal execution and was not disabled or weakened by this repair.

Final integration artifacts:

- `helper-reliability-e2e.json`: 32 passed checks, no cleanup blockers; Worker/D1,
  before-work repair/resume, revision/hash/authority fences, same-checkpoint retry,
  actual long-running verify-container cancellation and exact owned cleanup.
- `helper-plan-goal-e2e.json`: all 10 final assertions passed, including independent
  branch tests, two ChatGPT-mode rounds, background/direct Goals, Plan read-only,
  duplicate-start handling, hooks, Git readiness and emergency stop.
- `actual-codex-goal-smoke.json`: `passed=true`, one actual agent dispatch, Goal
  succeeded, changed-files list exactly `codex-goal-smoke.txt`, original source clean.
  This was a bounded product test, not engineering delegation or a quota fallback.
- Native report: `an ignored temporary path`.
  Private Docker standalone report: `an ignored temporary path`.
  These ignored local reports remain distinct from real G1 or public CI evidence.

The MCP ChatGPT-mode test caller is a synthetic client using the actual local
helper/server/Runner routes. It is not a connected ChatGPT website or a human
native button click. The actual Codex test uses the configured default provider,
without an agent/sandbox override; quota/auth failures would not be retried.
No AI agent was started by its earlier preflight-only transport failure.

Reproducible entrypoints: `scripts/e2e_goal_reliability.py`,
`scripts/e2e_plan_goal.py`, `scripts/e2e_goal_codex_smoke.py`,
`scripts/test_goal_environment.py`, `scripts/test_goal_checkpoint.py`, and
`scripts/tests/goal_acceptance_sandbox/{smoke,docker_smoke}.py`.
All use isolated synthetic projects; Docker inputs require an already authorized
local image/daemon and pinned dependencies, never an automatic image pull.

The test-only Linux workerd package was explicitly approved for public download
and checked against the existing lockfile's SHA-512 integrity. Other packages were
prepared from offline cache. No private lockfile or source was uploaded to a registry.
The initially rejected online `npm ci` was not retried or bypassed.

Known tooling limitation: the local Rust toolchain's `rust-objcopy` cannot find its
`libLLVM.dylib`. Builds/tests emit a nonfatal stripping warning. `rustfmt` is not
installed in that toolchain; no global toolchain installation was performed.

## Original Goal recovery and remaining gates

A fresh read-only status check found that the original G1 Goal is already
`cancelled` (`cancel requested`), not the earlier `needs_attention`. This repair
did not perform that cancellation. Its retained checkpoint is still present and
tracked files are clean. A cancelled Goal is terminal and must not be resurrected
by `resume`, adoption or an acceptance revision.

For this original case, preserve the existing checkpoint and create a **new**
explicitly approved Goal whose base contains that checkpoint. Use a separately
prepared source copy/branch selected for the new Goal; do not reset or switch the
formal original project on the user's behalf. Private inputs/toolchains still
need their exact approved manifests. The new budget is a new human approval, not
a silent extension of the cancelled run. No new G1 Goal was created by this repair.

For other nonterminal legacy runs still parked in `needs_attention`, the safe route
is explicit local adoption (HEAD/base ancestry/subject/clean checks), full immutable
acceptance proposal, exact-hash approval, then `retry_acceptance` at the same Goal
and checkpoint. Real external owner/remote evidence continues to block SUCCESS.
If the original approved deadline has expired, preserve the checkpoint and seek a
new Goal/budget approval instead. An installed v0.7.1 binary does not acquire this
working-tree implementation automatically.

Not validated: real G1 recovery on an upgraded installed App; real human/native
button authorization; final release package; Windows/Linux native acceptance;
public CI or release. These require separate evidence and are not replaced by
synthetic local tests. In particular, native acceptance currently targets macOS;
unsupported platform capability must fail closed, never use a weaker provider.

## Delegation and integration

Trusted execution records confirm the actual settings, not just spawn arguments:

| Slice | Assigned | Confirmed execution | Outcome |
| --- | --- | --- | --- |
| Store migration, revisions and receipts | `gpt-6-luna / max` | `gpt-6-luna / max` | Completed; integrated and extended by main |
| Goal UI, human recovery/revision flow | `gpt-6-luna / max` | `gpt-6-luna / max` | Completed; integrated and extended by main |
| Native/container isolation | `gpt-6.1-sol / xhigh` | `gpt-6.1-sol / xhigh` | Interrupted by workspace credits; main completed subsequent integration/validation |

No replacement agents or external CLI delegation were used to bypass that quota.
Product tests of the Runner/Goal executor are separate from engineering delegation.

Agent execution-record identities: Store `01a13aa1-4e45-7a91-90db-aacc6422cd13`;
UI `01a13aa3-e50a-7032-b02c-ece11b274256`;
sandbox `01a13aa1-4cba-78f1-bed1-102f544e2670`.
Assigned and actual model/effort values above were checked against the local
execution records' `turn_context`, not inferred from spawn acceptance or answers.
