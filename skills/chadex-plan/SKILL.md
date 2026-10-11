---
name: chadex-plan
description: Plan a change in a Chadex project before touching anything, then implement it yourself as soon as the user approves. Use when the user says "Chadex Plan Mode", "@plan", "plan first", "don't change code yet", or asks for an implementation plan for a Chadex-connected project. Enters backend-enforced read-only Plan Mode, saves a versioned plan with acceptance checks, waits for approval in the Chadex app, then implements it in the same reply. Not related to Goal Mode.
---

# Chadex Plan Mode

Plan Mode is enforced by the Chadex backend, not by these instructions. After `plan_start`, every write, shell, job, Git write, Computer Use, Plugin, local MCP or agent call from this connection is refused until the user approves the plan or exits Plan Mode in the Chadex app. Do not try to work around a refusal.

## Steps

1. Identify the project (ask only if it is ambiguous; `runtime_status` / `list_projects` help). Call `plan_start` with the project and a short title.
2. Investigate with read-only tools: `project_overview`, `search_project_texts`, `read_files`, `git_status`, `git_log`, `git_diff_hunks`, `memory_search`, `skill_list`. Ground every claim in what you read; say what you could not verify.
3. Write the plan in Markdown: goal, current state with evidence (paths, functions), approach, ordered steps, risks and rollback, what is out of scope.
4. Choose acceptance checks that prove the work is done. Each is literal argv (no shell), run in the project root (or `cwd`), exit 0 = pass. Prefer the repository's real test/build/lint commands, scoped to the change when the full suite is slow. Never use a check that always passes.
5. Save with `plan_save` (through `call_runtime_tool` if it is not listed directly) using `parent_revision` = current latest revision (0 for the first save). Revising creates a new immutable revision; an approval of an older revision no longer counts.
6. Tell the user: plan id, revision, the acceptance checks, and that they approve **that revision** in the Chadex app (Plan & Goal → **Approve, ChatGPT starts**). You cannot approve.
7. Do not end your reply. Call `plan_wait_approval` with the plan id and keep calling it while it returns `waiting`:
   - `approved`: Plan Mode has ended. Implement exactly the approved plan now, in this same reply, directly in the project with your normal tools. Then run each acceptance check with `run_process` (literal argv) and fix and re-run until they pass or you need the user. Report what changed and the check results. This is not a Goal: do not call `goal_run_start`.
   - `changes_requested`: revise with `plan_save` following the user's request, tell them what changed, and wait again.
   - `closed` or `approval_withdrawn`: do not implement; stop and ask what they want.
   - `goal_started`: the user started this revision as a Goal in the app; do not implement it yourself, follow it with `goal_run_status`.
   If your reply has to end before a decision (you hit a limit), tell the user to say "start" after approving; then call `plan_wait_approval` once and continue.

## Rules

- Never claim the plan is approved before `plan_wait_approval` says `approved`, and never claim checks passed without running them.
- If the user asks for changes, revise with `plan_save` and report the new revision and hash.
- If a tool is refused with `plan_mode_read_only`, explain that Plan Mode blocks it and continue planning.
