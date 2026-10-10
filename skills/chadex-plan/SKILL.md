---
name: chadex-plan
description: Plan a change in a Chadex project without modifying anything. Use when the user says "Chadex Plan Mode", "@plan", "plan first", "don't change code yet", or asks for an implementation plan for a Chadex-connected project. Enters backend-enforced read-only Plan Mode, investigates the real code and Git state, and saves a versioned plan with objective acceptance checks for the user to approve in the Chadex app.
---

# Chadex Plan Mode

Plan Mode is enforced by the Chadex backend, not by these instructions. After `plan_start`, every write, shell, job, Git write, Computer Use, Plugin, local MCP or agent call from this connection is refused until the user approves the plan or exits Plan Mode in the Chadex app. Do not try to work around a refusal.

## Steps

1. Identify the project (ask only if it is ambiguous; `runtime_status` / `list_projects` help). Call `plan_start` with the project and a short title.
2. Investigate with read-only tools: `project_overview`, `search_project_texts`, `read_files`, `git_status`, `git_log`, `git_diff_hunks`, `memory_search`, `skill_list`. Ground every claim in what you read; say what you could not verify.
3. Write the plan in Markdown: goal, current state with evidence (paths, functions), approach, ordered steps, risks and rollback, what is out of scope.
4. Choose acceptance checks the Goal will be judged by. Each is literal argv (no shell), run in the project root (or `cwd`), exit 0 = pass. Prefer the repository's real test/build/lint commands, scoped to the change when the full suite is slow. Never use a check that always passes.
5. Set a budget if the defaults (5 iterations, 120 minutes, 1800 s per agent turn) do not fit.
6. Save with `plan_save` (through `call_runtime_tool` if it is not listed directly) using `parent_revision` = current latest revision (0 for the first save). Revising creates a new immutable revision; an approval of an older revision no longer counts.
7. Tell the user: plan id, revision, the first 12 characters of `content_sha256`, the acceptance checks, and that they must review and approve **that revision** in the Chadex app (計畫與目標). You cannot approve, and nothing will be executed until they do.

## Rules

- Never claim the plan is approved, executed, or tested.
- If the user asks for changes, revise with `plan_save` and report the new revision and hash.
- If a tool is refused with `plan_mode_read_only`, explain that Plan Mode blocks it and continue planning.
