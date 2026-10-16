# Goal acceptance sandbox integration contract

The pure native/Docker argv module is integrated by `goal_container.rs` and
`goal_supervisor.rs`; Store types are deliberately not imported by the pure
builder. See `docs/GOAL_RELIABILITY_FIX_REPORT.md` for the complete policy,
operator-managed image allowlist, immutable approvals and trusted receipts.

```rust
pub(super) enum SandboxLane { Offline, Loopback }
pub(super) type ProcessArgv = (String, Vec<String>, Option<String>);
pub(super) fn build(workspace: &Path, argv: &[String], cwd: Option<&Path>,
    lane: SandboxLane, ports: &[u16]) -> Result<ProcessArgv, SandboxError>;
pub(super) fn build_with_read_only(workspace: &Path, argv: &[String], cwd: Option<&Path>,
    lane: SandboxLane, ports: &[u16], read_only_paths: &[PathBuf])
    -> Result<ProcessArgv, SandboxError>;
pub(super) fn preflight(workspace: &Path, cwd: Option<&Path>,
    lane: SandboxLane, ports: &[u16]) -> Result<ProcessArgv, SandboxError>;
pub(super) fn preflight_with_read_only(workspace: &Path, cwd: Option<&Path>,
    lane: SandboxLane, ports: &[u16], read_only_paths: &[PathBuf])
    -> Result<ProcessArgv, SandboxError>;
```

- Map OfflineSandbox -> Offline; LoopbackIntegration -> Loopback.
- ExternalAttestation executes its approved checker under the offline lane, then
  requires the separately verified external-authority artifact. Exit 0 alone is
  not authority and never completes the gate.
- read_only_paths are absolute lexical paths under the managed worktree, outside
  .goal-acceptance. These are trusted hash-approved fixture destinations, not source roots.
- Builder is pure. It does not stat/read/resolve any server-local path.
- Native profile preflight only executes /usr/bin/true, with no workspace writes.
  Actual bound Runner exit is required. This does not establish cwd/tool readiness.
- Main must check canonical managed-worktree, cwd and every argv[0] on the Runner,
  BEFORE modifications, reject provider/environment overrides, and journal the
  actual preflight / acceptance commands, SHA, actual result and signed receipt.
- Native preflight and acceptance use a fixed `/usr/bin/python3 -I -S -c` launcher
  and bounded structured stdin. An owned 0600 temporary SBPL file is passed to
  `/usr/bin/sandbox-exec -f`. Preflight only executes true; ordinary acceptance's
  trusted static /bin/sh setup receives paths/argv ONLY as positional arguments.
  State/home/config/cache/log/temp are inside .goal-acceptance. No user runtime/config,
  credentials, inherited tokens, global npm install or environment mutation.
- The read allowlist does not grant whole /usr/local, /opt/homebrew or personal
  homes. Explicit installed runtime roots (Cellar/opt/frameworks) and selected
  executable literals are read-only. Project dependencies require exact approved
  scoped copies; missing or changed dependencies remain blockers.
- Sandbox file denies complement exact source SHA / fresh independent copy;
  chmod 444 alone is not immutability. Main must reject source symlinks, avoid
  pre-existing destination hardlinks, and exclude concurrent unsandboxed writers.
- ports must be 49152..65535, <=8, unique. Offline forbids any ports.
- Production Loopback returns LoopbackOwnershipUnverified. Even exact tcp4 port
  policy cannot prove service identity / exclude Chadex operator/backend services
  / prevent free-port-preflight -> bind replacement. Unknown inventory, platform,
  capabilities, endpoint ownership or sensitive-service classification is blocked.
- Native host loopback still requires a Runner-owned socket broker. Production
  instead supports an explicitly approved pinned Linux image in a local private
  Docker namespace (`--network none`), with no host ports/network/socket/home.
  It is NOT an automatic fallback for a host-port declaration. Supervisor
  journals create -> actual ID/Created -> start -> daemon wait -> exact owned
  cleanup, and independently recovers pending cleanup after lease expiry.
  Killing only the attach CLI never counts as cleanup or successful acceptance.
- Original G1 e2e_local_mcp.py uses bind(127.0.0.1,0) and inspector-port=0. It blocks under native exact approved ports, even with Wrangler's main port fixed; Miniflare
  has additional internal ephemeral sockets. Keep checkpoint 6d6e811 and original
  evidence; a safe private Linux image/lane and complete manifests still need human
  hash approval. The original Goal is now cancelled, so preserve the commit and
  use a newly approved Goal; do not revive a cancelled ID. Never modify formal
  G1 or relabel real incomplete gates as PASS.
- Candidate tcp4 SBPL is available ONLY in cfg(test), for dedicated owned-listener
  evidence. Its execution is not production readiness or a usable Goal receipt.

Sources:
- https://developers.cloudflare.com/workers/wrangler/system-environment-variables/
- https://developers.cloudflare.com/d1/best-practices/local-development/
- Installed Apple /System/Library/Sandbox/Profiles/dyld-support.sb documents the
  literal "/" dyld openat bootstrap requirement. No system.sb import is used.
