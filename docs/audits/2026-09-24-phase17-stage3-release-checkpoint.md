# Phase 17 Stage 3 — Release Checkpoint & Public History Preparation

Date: 2026-09-24

## Goal

Convert the audited Stage 1/2 working tree into a coherent, clean release checkpoint and prepare a public-history candidate without rewriting or publishing the private development history.

## Change classification

The Stage 1/2 working tree was reviewed as one release-preparation set. Intended checkpoint content includes repository cleanup rules, release/license documentation, canonical benchmark evidence, Graphify → Obsidian tooling and documentation, current Graphify human-readable report, release/build tooling, portable benchmark harnesses, the pinned Rust toolchain, required Runtime Console compile-time assets, and the Stage 2 privacy/release audit.

Two source diffs that appeared alongside Stage 2 were separately verified rather than discarded:

- `runtime-engine/crates/chadex-runtime-cli/src/webcodex_cli/login.rs` updates a stale test expectation to include the already-existing `managed_worktree_root` policy introduced earlier in the runtime implementation. The targeted login regression passes.
- `runtime-engine/crates/chadex-runtime-core/tests/apply_patch_matching_benchmark.rs` uses the current `chadex_runtime_core` crate name after the Chadex runtime rename. The targeted benchmark test passes.

The `chadex-runtime-process` integration-test import fix is also retained because the Stage 2 all-target release compile demonstrated that the obsolete crate name otherwise prevents release validation.

## Public-history decision

Do not rewrite the existing private branch history and do not publish it unchanged. Preserve `phase6/runtime-core` as the private provenance history.

After the Phase 17 checkpoint commit is created, create a separate local public candidate branch from a new root commit whose tree is byte-for-byte identical to that checkpoint. This gives the first public repository a clean history without destroying internal provenance. The candidate branch must remain local until final identity, secret-history scan, CI, distribution signing, and publication checks are complete.

## Stage 3 validation contract

Stage 3 is complete only when:

1. the Phase 17 checkpoint is committed on the private development branch;
2. the development working tree is clean;
3. the clean public root branch exists locally and has the same tree as the checkpoint;
4. the public branch history does not contain the historical machine/device-bearing commits;
5. `scripts/release_check.sh` passes from a clean tree without `CHADEX_RELEASE_ALLOW_DIRTY`;
6. no remote, tag, GitHub Release, notarization, or private-history rewrite has occurred.

## Publication boundary

Stage 3 does not authorize a public push. Stage 4 remains responsible for final public commit identity, dedicated history secret scanning, CI/release workflow, Developer ID/Hardened Runtime/notarization/stapling/Gatekeeper validation, tag/RC naming, remote configuration, and actual GitHub publication.
