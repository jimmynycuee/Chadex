---
name: chadex-goal
description: Give Chadex a goal to accomplish autonomously on the user's Mac, or run an approved Chadex plan. Use when the user says "Chadex Goal Mode", "@goal", "keep working until …", "execute the approved plan", or asks for status of a Chadex Goal. No plan is needed for a direct goal; the user presses Start in the Chadex app, then progress is judged only by objective acceptance checks.
---

# Chadex Goal Mode

A Goal runs inside Chadex on the user's Mac, not in this conversation. Once started it keeps iterating (agent work in an isolated Git worktree → checkpoint commit → the acceptance checks) without further ChatGPT messages, and it succeeds only when every acceptance check passes. Closing ChatGPT does not stop it; quitting Chadex or sleeping the Mac pauses it until Chadex runs again.

## Start a direct Goal (no plan needed)

1. Identify the project (`list_projects` if unclear). Look at the repository just enough to choose real acceptance checks (`project_overview`, `read_files`, `git_status`).
2. Call `goal_run_start` with `project`, a short `title`, the `objective` (what must be true when done, constraints, what must not change) and `acceptance`: each check is literal argv (no shell), run in the project root (or `cwd`), exit 0 = pass. Prefer the repository's real test/build/lint commands. Never use a check that always passes. Add `budget` only if the defaults (5 iterations, 120 minutes) do not fit.
3. Tell the user to press **Start Goal** in the Chadex app (計畫與目標 / Plan & Goal). Chadex also shows a Mac notification. You cannot start it yourself, and it does not run until they do.
4. Once they say it started, find the run with `goal_run_list` (its `plan_id` matches) and follow it with `goal_run_status`.

## Start an approved plan

If the user planned first with Chadex Plan Mode, the app's **Approve and start Goal** button already starts it; otherwise call `goal_run_start` with the `plan_id` after they approved the latest revision. If it fails with `plan_not_approved`, `plan_approval_stale` or `plan_approval_environment_changed`, ask the user to review/approve again in the Chadex app. Never try to approve yourself.

## Progress

- ChatGPT is not notified when a Goal finishes. Tell the user Chadex notifies them on the Mac and they can ask you for progress at any time.
- `goal_run_status` shows state, reason, iteration, per-iteration acceptance results, operations, events, result branch and token usage (or "unknown"). Pass `wait_secs` (up to 45) to wait for a run that is about to finish. `goal_run_list` finds runs started from the app.
- Report results only from that evidence. An agent's own "done" is not success; `succeeded` means the acceptance checks passed.
- `goal_run_control` can pause, resume a run the user paused, or cancel. Runs in `needs_attention` (unknown outcome, permission denial, no progress) are resumed only from the Chadex app after review.

## Finish

On `succeeded`, tell the user the result branch (`chadex/goal/<id>`) and suggest reviewing the diff before merging. Chadex never merges, pushes, deploys, publishes, pays, or sends messages on its own; do not offer to do those as part of the Goal.
