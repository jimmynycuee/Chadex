---
name: chadex-goal
description: Give Chadex a goal that is finished only when objective acceptance checks pass. Use when the user says "Chadex Goal Mode", "Chadex goal", "@goal", "目標", "keep working until …", or asks for status of a Chadex Goal. You send the goal; the user picks the mode in the Chadex app — Codex (background, uses their Codex token quota, ChatGPT can be closed) or ChatGPT (you do the edits here and call goal_check until achieved). Independent of Plan Mode.
---

# Chadex Goal Mode

A Goal succeeds only when every acceptance check passes, judged by Chadex in an isolated Git worktree (one checkpoint commit per round). The user chooses who does each round when they press Start in the Chadex app:

- **Codex mode**: Chadex runs the Codex CLI in the background, round after round, without ChatGPT. It uses the user's Codex token quota. Closing ChatGPT does not stop it; quitting Chadex or sleeping the Mac pauses it.
- **ChatGPT mode**: you do the edits in this conversation; no Codex quota is used, but the conversation must stay open.

## Start a direct Goal (no plan needed)

A goal written only in the chat is not a Chadex Goal: send it with `goal_run_start`. If `goal_run_start` (or any tool below) is not in your tool list, the connector's tool list in ChatGPT is out of date: call it through `call_runtime_tool` with `{"tool": "goal_run_start", "arguments": {...}}`. Never tell the user Chadex has no goal tool before trying `call_runtime_tool`.

1. Leave `project` out unless the user named one or this conversation is clearly about a specific project: Chadex then uses the project open in the Chadex app. Tell the user which project the goal is for. Only if `goal_run_start` answers that the project is ambiguous, ask the user to pick from the candidates it lists. Look at the repository just enough to choose real acceptance checks (`project_overview`, `read_files`, `git_status`).
2. Call `goal_run_start` with a short `title`, the `objective` (what must be true when done, constraints, what must not change) and `acceptance`: each check is literal argv (no shell), run in the project root (or `cwd`), exit 0 = pass. Prefer the repository's real test/build/lint commands. Never use a check that always passes. Add `budget` only if the defaults (5 iterations, 120 minutes) do not fit.
3. Tell the user to open the Chadex app (Plan & Goal) and choose **Run with Codex in the background** (uses Codex tokens) or **Run in this ChatGPT conversation** (no Codex tokens). Chadex also shows a Mac notification. You cannot start it yourself.
4. Once they say it started, find the run with `goal_run_list` (its `plan_id` matches). Its `mode` tells you what to do next.

## ChatGPT mode: you do the work

1. Read `workspace_project` from `goal_run_status` (wait briefly with `wait_secs` while it is being prepared). Make every edit **in that workspace project**, not the original project.
2. After a round of edits, call `goal_check`. Chadex commits the round and runs the approved acceptance checks; you cannot change them.
3. If `achieved` is false, read `failures`, fix them in the workspace project, and call `goal_check` again **in the same reply**. Keep going until `achieved` is true or the run stops. If the checks are still running, just call `goal_check` again.
4. If your reply has to end first, tell the user to say "continue"; then call `goal_check` once to see where you are and keep going. After 30 minutes without `goal_check` the Goal pauses; the next `goal_check` resumes it.
5. Never claim the goal is done without `achieved: true`.

## Progress

- ChatGPT is not notified when a Goal finishes. Tell the user Chadex notifies them on the Mac and they can ask you for progress at any time.
- `goal_run_status` shows state, reason, iteration, per-iteration acceptance results, operations, events, result branch and token usage (or "unknown"). Pass `wait_secs` (up to 45) to wait for a run that is about to finish. `goal_run_list` finds runs started from the app.
- Report results only from that evidence. An agent's own "done" is not success; `succeeded` means the acceptance checks passed.
- `goal_run_control` can pause, resume a run the user paused, or cancel. Runs in `needs_attention` (unknown outcome, permission denial, no progress) are resumed only from the Chadex app after review.

## Finish

On `succeeded`, tell the user the result branch (`chadex/goal/<id>`) and suggest reviewing the diff before merging. Chadex never merges, pushes, deploys, publishes, pays, or sends messages on its own; do not offer to do those as part of the Goal.
