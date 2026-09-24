# User-visible completion latency — 2026-09-23

## Decision

Milliseconds of local runtime improvement are not the acceptance target. For a
common coding task, measure from ChatGPT submission to the first DOM-observed
completed answer, with a separate stricter confirmation timestamp. Keep
correctness, failures, tool-call count, model/host conditions, and p95 beside
median. A candidate needs at least 20 correct
alternating runs per arm, a median improvement of both 2 seconds and 20% over
the current Chadex runtime, a lower median than same-host WebCodex, and p95 no
more than 10% above current Chadex. Build identity and approval equivalence
must be checked separately before acceptance; the harness never labels an
unverified App build as released.

## Connected pilot: tool-await only

`benchmarks/task-completion-connected-pilot.json` contains all five pilot
pairs. Every arm used the same pinned discount fixture, produced the same exact
diff, passed 43 independent business checks, and was reset by the guarded
fixture script. The live Chadex runtime was **`72e7fb0decd4` clean**, not the
new `Chadex-TaskPerformance.app` candidate. This is the Codex connector host,
not a ChatGPT Web model turn; model thinking and final-response time are
excluded. The first two pairs ran fused then direct; the final three alternated
order.

| Pair | Five direct calls | Search/read + execute_task | Direct minus fused |
|---:|---:|---:|---:|
| 1 | 15.671 s | 21.983 s | -6.312 s |
| 2 | 14.072 s | 11.556 s | +2.516 s |
| 3 | 17.201 s | 12.496 s | +4.705 s |
| 4 | 13.043 s | 12.211 s | +0.832 s |
| 5 | 15.460 s | 12.577 s | +2.883 s |

Arm medians were **15.460 s direct** and **12.496 s fused**; paired median
saving was **2.516 s**. One fused call regressed by 6.312 s, and five pairs
are insufficient for a stable p95 or release decision. In pairs 3–5,
`execute_task` itself reported roughly 5.2 s of local task duration. Worktree
creation was around 2.3 s and cleanup around 2.1 s. The live old runtime's
`snapshot_ms` still overlaps worktree creation, so these component values
must not be added. The 9–20 s outer `execute_task` waits also contain
unlocated host/relay/approval time. Do not infer a single cause from them.
The pilot excluded model reasoning and the one-time `tool_manifest` discovery
call; an ordinary model turn may pay both costs.

A separate five-iteration isolated control placed disposable clones on the
same Documents volume as the benchmark projects. Candidate fused-task median
was **0.992 s**, versus **1.129 s** for the prior packaged reference; both
retained exact-diff correctness (`benchmarks/task-completion-same-volume.json`).
This does not reproduce the live runtime's ~5.2 s task duration. The isolated
App, project inventory and connected transport differ, so the discrepancy
requires phase-level tracing in the actual connected candidate before changing
worktree or Runner code.

## ChatGPT completion harness

`scripts/benchmark_chatgpt_completion.py` drives the existing independent
ChatGPT browser driver with dedicated, already-registered benchmark fixtures.
It alternates two or three arms, records submission, accepted prompt, visible
final answer and stricter confirmed completion times, then grades the exact
diff, business behavior, visible tests and `git diff --check` outside the
timed model turn. It only restores the two fixture files when their status and
diff match the pinned expected result; unknown changes remain untouched.
Results retain missing tool/token and runtime-build measurements as null or
unverified. The UI driver is designed to reuse a marked benchmark-owned
Temporary Chat tab because the embedded browser cannot create a new CDP tab;
that repeated-turn behavior still needs a successful connector preflight.

Example configuration; replace connector names with **actually installed**
benchmark connectors and register the candidate fixture separately before
including it:

```json
{
  "arms": [
    {
      "name": "chadex-current",
      "connector": "Chadex Benchmark",
      "fixture": "/path/to/Benchmark-Chadex"
    },
    {
      "name": "webcodex",
      "connector": "WebCodex Benchmark",
      "fixture": "/path/to/Benchmark-WebCodex"
    }
  ]
}
```

Run `--preflight` first with a new or empty output directory. It checks clean
fixtures and UI connector selection without submitting a task. For the full
gate, add a `chadex-candidate` arm with a **different dedicated clean fixture
path and connector**, then run 20 iterations. All three arms must use the same
host/model effort and comparable approval conditions. The result includes
per-arm median/p95, paired differences and a performance gate; `release_ready`
remains false until runtime identity and conditions are independently verified.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -B scripts/benchmark_chatgpt_completion.py \
  --config /path/to/benchmark-config.json \
  --output-dir /path/to/new-preflight-dir --preflight

PYTHONDONTWRITEBYTECODE=1 python3 -B scripts/benchmark_chatgpt_completion.py \
  --config /path/to/three-arm-config.json \
  --output-dir /path/to/new-results-dir --iterations 20
```

Current preflight **did not pass**: the embedded ChatGPT UI could not select
`Chadex Benchmark` (the connector was unavailable there), and the separate
Codex WebCodex connector reported that its tunnel client had not been seen for
300 seconds. The preflight result is saved at
`benchmarks/chatgpt-completion-preflight.json`. The candidate App is not
connected. No valid ChatGPT Web
completion sample was produced. The next engineering choice is therefore to
restore the benchmark host/connector prerequisites and measure the candidate
on the same path; only then decide whether worktree lifecycle or the external
request path deserves a code change. Do not force task fusion for jobs whose
edits depend on intermediate model decisions or whose source-read revision
must fence the write.
