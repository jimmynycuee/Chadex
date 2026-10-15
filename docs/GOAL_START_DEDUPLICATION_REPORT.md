# Goal start duplicate-click regression

## Scope

The user reported that selecting a Goal execution option twice created two effective runs. Work is isolated on branch `codex/goal-start-singleflight` from published v0.7.1 (`3256161b5f7a7e077523922107cd2956bb9f3244`). Existing live runs, the primary dirty checkout and other worktrees are preserved.

## Cause and fix

The UI disabled start options only while an action was in flight or an approval remained active. Starting consumes the approval, so the active-approval test became false and re-enabled the options. Each operator `approve_and_start` request issued another approval, bypassing the store's per-approval run replay protection.

- Reserve the exact project/plan/revision/hash synchronously on button tap, before confirmation. Both execution options are blocked together. Cancelling an unsubmitted confirmation releases the reservation.
- After success, replace execution options with the existing run's state, mode and View run action. Preserve this state from the successful response even if refresh fails; reconcile newer durable state on later reads. A consumed approval restores the started state after reopening the panel.
- An unknown transport outcome stays blocked while status is checked. A complete overview plus exact-revision inspection can resolve it; do not automatically replay a write. Pending submissions remain unknown across project switches and are scoped to their original project.
- In the Store's immediate transaction, check for an existing run by owner/plan/latest revision/hash before consumption or creation. Replay returns its original identity/mode/dispatch with `created: false`; changed project/Runner/root bindings fail closed. Consumed approvals with missing or inconsistent run bindings fail closed.
- Reapproval of an already-started revision reuses its consumed decision. Concurrent direct Goal requests share an unexpired active approval. Ordinary Plan reapproval retains its superseding semantics; an expired Goal approval can be replaced. New revisions can create distinct runs.

## Baseline and corrected cross-layer result

`scripts/e2e_plan_goal.py` exercises the actual helper -> operator -> runtime -> runner route against fresh temporary data. The agent provider is a deterministic local script and ChatGPT-side calls are emulated; no model credits, real tunnel or production data are used.

| Check | Installed v0.7.1 build35 baseline | Final patched runtime |
| --- | --- | --- |
| Same-mode retry | Distinct run; `created: true` | Same run; `created: false` |
| Cross-mode retry | Distinct run; `created: true` | Same run; `created: false` |
| Runs for the exact plan | 3 | 1 |
| Original mode and approval IDs preserved | No | Yes |
| Regression exit code | 1 (expected failure) | 0 |

Final E2E uses the unchanged installed helper with three newly built debug runtime entrypoints in `dist/goal-start-test-resources/chadex-runtime`. It also completes the existing read-only Plan, local fake-agent Goal, emulated ChatGPT two-round acceptance, Git-readiness refusal and emergency-stop checks. The fixture's main worktree remains unchanged.

Local ignored evidence: `dist/goal-start-evidence/baseline-v071.json`, `fixed-e2e.json`, `fixed-e2e.log`, `runtime-build-final.log`.

## Validation — 2026-10-14

| Validation | Result |
| --- | --- |
| Swift complete suite | 279 executed, 1 skipped, 0 failures; All tests passed |
| Focused Swift start-state tests | 12 passed |
| Rust Store complete suite | 194 passed, 0 failed; exit 0 |
| Runtime Plan operator suite | 31 passed, 0 failed; exit 0 |
| Runtime Goal suite | 18 passed, 0 failed; exit 0 |
| Three runtime debug entrypoints | Build passed; exit 0 |
| Isolated helper/operator E2E | Passed; exit 0 |
| Python source parsing and diff whitespace | Passed |

An intermediate attempt reused active approvals too broadly and failed two original Store tests. The final implementation narrows active reuse to direct Goal requests and excludes expired approvals; both original tests and a new expiry regression pass unchanged. Swift test results above are taken from the completed test log: the surrounding shell harness subsequently failed on zsh's readonly `status` variable, which did not change the test result. The Rust/E2E final harnesses use task-specific variable names and return their real exit codes.

## Subagent execution

Both calls were accepted, completed and closed. Actual settings are confirmed from execution-record `turn_context` metadata and saved locally in `dist/goal-start-evidence/agent-execution-settings.json`.

| Task / agent | Assigned model / effort | Confirmed execution model / effort | Result |
| --- | --- | --- | --- |
| Swift UI / 01a13a53-c519-7dc0-86d4-ac6f9b04139e | gpt-6-luna / xhigh | gpt-6-luna / xhigh | UI ledger, presentation and regression tests integrated |
| Store/operator / 01a13a53-c680-7891-a967-bfa275ae4c5b | gpt-6-luna / max | gpt-6-luna / max | Deduplication and regressions integrated; main agent corrected active-approval scope and ran final gates |

## Remaining delivery boundaries

Native button-click smoke and connected human ChatGPT/tunnel acceptance are **not validated** for this new patch. No release candidate is packaged or published, and `/Applications/Chadex.app` remains the original v0.7.1 build35. Existing duplicate live runs are not cancelled, deleted or merged. PG2 remains in progress pending review, candidate acceptance and merge.

The separate macOS 28 warning has been traced to external Intel Node/esbuild/workerd task dependencies. That toolchain migration has not been applied by this Goal fix.
