# Phase 12 execution semantics — validation and performance

Scope: the Phase 11 working tree present at task start, extended with Phase 12
execution semantics. This is local source/test evidence. No app deployment,
service restart, push, or ChatGPT Web/connector acceptance is claimed.

## Reproducible validation

Use the repository toolchain via `scripts/rust-env.sh` and the `dogfood` profile.
The task-start Phase 11 test executable was preserved before implementation; its
47 focused task executor tests passed. Candidate tests cover the same scenarios
plus state transitions, terminal immutability, real receipt durability, storage
failure, duplicate mutation/replay, cancellation, unknown results, retry lineage,
queued/running crash classification, ownership locks, and safe cleanup.

Commands:

```sh
. scripts/rust-env.sh
chadex_setup_rust "$PWD"
"$CHADEX_CARGO" test --offline --manifest-path vendor/webcodex/Cargo.toml --profile dogfood -p webcodex --lib chadex_task_executor -- --test-threads=4
"$CHADEX_CARGO" test --offline --manifest-path vendor/webcodex/Cargo.toml --profile dogfood -p webcodex-tool-contracts -p webcodex-tool-runtime-contracts --lib
CLANG_MODULE_CACHE_PATH=/tmp/chadex-phase12-clang SWIFTPM_MODULECACHE_OVERRIDE=/tmp/chadex-phase12-swift-module swift test --disable-sandbox --cache-path /tmp/chadex-phase12-swift-cache --filter ProtocolModelTests
```

The default Swift cache location was unavailable inside the sandbox. The same
protocol suite passed using temporary cache directories; no approval escalation
or change to production state was required. Existing dead-code warnings in the
Rust server are unrelated to this change.

## Final results

| Suite | Result |
|---|---:|
| Phase 11 baseline task executor | 47 passed |
| Phase 12 task executor + execution semantics | 65 passed |
| Execution workspace / bounded scheduler | 8 passed |
| Runtime schema / metadata regressions | 60 passed |
| `webcodex-tool-contracts` | 139 passed |
| `webcodex-tool-runtime-contracts` | 94 passed |
| Swift `ProtocolModelTests` | 13 passed |

Candidate total: **366 Rust tests and 13 Swift tests passed**, zero failures.
`cargo check --tests` completed successfully during implementation; final modified
Rust source passed compilation and the above regression suites. Targeted
`rustfmt --check`, `git diff --check`, localization `plutil -lint`, and the
benchmark script syntax check passed.

Two old expectations were deliberately updated to the new contract: ambiguous
mutation is now `unknown`, and an explicit unknown read result cannot be retried
merely because its failure category is transient. Known no-dispatch read errors
still use the original bounded one-retry policy.

## Benchmark method and limits

`benchmarks/phase12_execution_benchmark.py` accepts two preserved Rust libtest
executables. It runs one warmup each, then 11 alternating AB/BA sample pairs per
case, with no concurrent compilation. The JSON artifact records every sample,
medians, deltas, and binary SHA-256 values.

- `small_read_cancel`: reads one file and cooperatively cancels before a second
  read, with no worktree or Graphify dependency.
- `small_edit_validate_review`: one guarded edit with validation/review and the
  Phase 11 task-level worktree fast path; no package parallelism.

Wall times include test-process startup and disposable fixture creation. Both
builds disable legacy production UI/log persistence. Candidate tests additionally
write real fsync-backed execution receipts and recovery plans in their own
TempDir; the baseline test build skipped recovery-plan I/O that already existed
in Phase 11 production. Consequently, the observed increase is a conservative
local harness cost, not a production or ChatGPT Web latency estimate.

The new standalone small-read regression also checks exactly one Runner read,
mode `none`, one package, no replay dispatch, and no late-cancel state mutation.
Phase 11 concurrency and worktree behavior are verified by the retained tests;
this measurement does not repeat the earlier connected-runtime 8s/12s workload.

## Measured performance

| Local harness case | Phase 11 median | Phase 12 median | Difference |
|---|---:|---:|---:|
| Small read / cooperative cancel | 23.16 ms | 54.23 ms | +31.08 ms (+134.19%) |
| Single edit / validation / review | 1050.47 ms | 1079.14 ms | +28.67 ms (+2.73%) |

The structural small-task fast path remains intact, but there **is a measurable
fixed-cost regression of about 29–31 ms in this harness**. The percentage is large
for a very short read/cancel case; the single-edit case increases 2.73%. This is
not claimed as zero regression. Durable admission/result writes and the test
recovery-plan I/O account for additional synchronous storage work. The comparison
does not isolate each write's contribution, and production UI persistence makes
an exact production percentage impossible to infer from these numbers.

Raw samples: [`phase12-execution-semantics.json`](../../benchmarks/phase12-execution-semantics.json).
The benchmark completed after all compilation and other test jobs ended.
No latency threshold was specified; this measured durability cost is retained
rather than weakening the execution invariants. Further I/O consolidation can be
considered only with the same persistence/replay guarantees.

## Changed file inventory

- `vendor/webcodex/src/tool_runtime/chadex_task_executor.rs`: canonical transition/publication boundary, interruption guard, shared uncertainty classification, retry lineage and recovery integration.
- `vendor/webcodex/src/tool_runtime/chadex_task_executor/execution.rs`: state enum, exclusive durable claim/receipt, ownership lock and startup receipt classification.
- `vendor/webcodex/src/tool_runtime/execution_workspace.rs`: ownership checks before cleanup.
- `vendor/webcodex/src/tool_runtime/tests/chadex_task_executor.rs`: retained regressions and real task replay/fast-path/ownership checks.
- `vendor/webcodex/src/tool_runtime/tests/execution_semantics.rs`: focused semantics and persistence fault tests.
- `vendor/webcodex/crates/webcodex-tool-contracts/src/registry/input_schemas/chadex_tasks.rs`: explicit replay identity contract.
- `vendor/webcodex/crates/webcodex-tool-contracts/src/registry/output_schemas/chadex_tasks.rs`: canonical state, execution identity and lineage fields.
- `vendor/webcodex/crates/webcodex-tool-contracts/src/tool_definition/chadex_tasks.rs`: model-facing semantics and audit fields.
- `Sources/ChadexApp/TaskProgressView.swift`: unknown status symbol/color.
- `Sources/ChadexApp/Resources/en.lproj/Localizable.strings` and `zh-Hant.lproj/Localizable.strings`: unknown outcome label, with no retry suggestion.
- `docs/ARCHITECTURE.md`: final execution contract and Phase 13/14 deferrals.
- This report, `benchmarks/phase12_execution_benchmark.py`, and its JSON result: local validation/performance evidence.

## Boundaries and deferred work

Replay protection is keyed by an explicit stable `task_id`/`execution_id`.
Omitting that ID is a fresh submission; plan text is not used to infer identity.
Unknown executions remain blocked from retry, and uncertain workspaces are
preserved. Receipts/claims currently accumulate; archival/retention policy,
checkpoint/resume, conversation persistence, unknown-outcome reconciliation,
and new rollback/conflict recovery remain Phase 13/14 work. Reliable private
storage is required; storage failure never permits a successful response or
reuse of a claimed identity.
