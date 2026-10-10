---
name: chadex-goal
description: Execute an approved Chadex plan autonomously. Use when the user says "Chadex Goal Mode", "@goal", "execute the approved plan", or asks for status of a running Chadex Goal. Starts the Goal only from a plan revision the user approved in the Chadex app, then reports progress from objective acceptance evidence.
---

# Chadex Goal Mode

A Goal runs inside Chadex on the user's Mac, not in this conversation. Once started it keeps iterating (agent work in an isolated Git worktree → checkpoint commit → the approved acceptance checks) without further ChatGPT messages, and it succeeds only when every approved acceptance check passes. Closing ChatGPT does not stop it; quitting Chadex or sleeping the Mac pauses it until Chadex runs again.

## Start

1. Find the plan (`plan_list` / `plan_get`). Confirm with the user which plan and revision they mean if unclear.
2. Call `goal_run_start` with the `plan_id`. It succeeds only if the user approved the latest revision in the Chadex app and the project's Runner/root are unchanged. If it fails with `plan_not_approved`, `plan_approval_stale` or `plan_approval_environment_changed`, ask the user to review/approve again in the Chadex app. Never try to approve yourself.
3. Tell the user the `goal_run_id`, the budget, and that they can close this chat.

## Progress

- `goal_run_status` shows state, reason, iteration, per-iteration acceptance results, operations, events, result branch and token usage (or "unknown").
- Report results only from that evidence. An agent's own "done" is not success; `succeeded` means the acceptance checks passed.
- `goal_run_control` can pause, resume a run the user paused, or cancel. Runs in `needs_attention` (unknown outcome, permission denial, no progress) are resumed only from the Chadex app after review.

## Finish

On `succeeded`, tell the user the result branch (`chadex/goal/<id>`) and suggest reviewing the diff before merging. Chadex never merges, pushes, deploys, publishes, pays, or sends messages on its own; do not offer to do those as part of the Goal.
