# Code Ferret

Code Ferret lives in the native SwiftUI sidebar. It observes Chadex's selected project and local runtime; it does not start work, retry commands, or infer private model reasoning.

## Architecture

- `CodeFerretState.swift`: deterministic reducer with an injected clock, bounded completion reactions, and project/history fencing.
- `CodeFerretView.swift`: a 30 fps `TimelineView`, continuous body poses, independently anchored tails, localized eyelid overlays, breathing, glow, and native progress UI. Reactions use elapsed state time and settle once.
- `AppModel.swift`: bounded status and MCP trace polling, one second in the foreground and five seconds in the background when connected. Hidden mascots do not request Job observation or mascot traces.
- The Rust helper registers an in-flight MCP trace before upstream headers arrive and updates the same sequence at completion. Cancellation closes that record. Response evidence retains only a matching boolean tool outcome, never a response body.
- Optional `mascot_jobs` snapshots contain bounded selected-project Job evidence. Observation failure is unknown, not success. No whole-Runner activity counts or invented progress percentages are used.

## Runtime mapping

| Observed fact | Presentation |
| --- | --- |
| Fresh meaningful work request, working task, or scoped Job | One-second listening reaction, then its observed activity; restored history and actual waits do not replay listening |
| Task search/read step; file/search tool | Search Repo |
| Task edit/integration; edit/write tool | Coding |
| Task validation; explicit validation tool | Running Tests |
| Generic shell/native process Job | Thinking; Long Task after 60 seconds |
| Queued, recovering, cancellation pending, blocked or preserved unknown task | Waiting |
| Active work older than 60 seconds | Long Task, retaining its activity and known task-step progress |
| Fresh completed task or successful terminal Job | Brief Success reaction |
| Explicit task/validation/tool failure or failed/lost/timed-out Job | Brief Error reaction |
| Ordinary successful work-tool response | Its actual activity for two seconds, labelled as recent activity, not whole-task success |
| Status-only polling (`task_status`, `job_status`, `poll_job`, `observe_jobs`, `observe_task`, `list_jobs`) | Diagnostic evidence; does not wake the companion or consume reactions |
| Connection ready but awaiting selected-project verification, without observed work | Idle, then Sleep; verification remains visible in the connection UI |
| Idle for three minutes, or inactive app with no observed work | Sleep |

Non-meaningful diagnostics, including `runtime_status`, do not wake the mascot or consume reactions. Terminal host handoff remains diagnostic evidence: lack of a follow-up request never becomes an invented host failure.

Fast completed work is observed even when its entire request fits between polls. Only the most recent completion is retained; there is no replay queue. The two-second hold expires without being extended by repeated snapshots. Live tools, known task steps and real waiting conditions take precedence; generic running Jobs allow this explicitly recent activity to remain visible in the foreground. Concurrent finishes are ordered by completion time; an explicit failure is not erased by another successful finish in the same snapshot. Foreground/background polling intervals and the animation clock are unchanged.

New observed work supersedes an older success or error reaction rather than replaying it after the new activity finishes. Errors retain five seconds and terminal success retains three seconds when no new work arrives; a simultaneous task/Job success cannot erase a failure. Waiting calls and cancellation remain Waiting after 60 seconds. A live scoped search/edit/validation tool takes precedence over generic process Job activity, and the Long Task threshold uses the selected activity's own start time rather than an unrelated older Job or wait. Known task-step evidence retains precedence. Runtime symbol navigation and project overview tools map to Search Repo, `write_project_file` and `apply_unified_diff` to Coding, and `go_test` to Running Tests; generic shell commands and `cargo_fmt` remain Thinking because traces contain no command/check-mode parameters.

## Appearance and motion

The approved character uses dark gray-brown fur, cream face/chest/underside, large dark expressive eyes, a black collar with cyan LED charm, cyan ear modules, and a cyan `>_` tail motif. Warning UI may use amber. The updated user-provided canonical, state, and motion sheets guide appearance and behavior.

Thirty transparent PNG assets include eleven continuous body poses, seventeen tightly localized eyelid overlays, and two tails. `ferret-motion-poses.json` records registration geometry. No head/body cut is animated during blinking. Native overlays supply thought dots, sniff marks, checklist/progress, clock, sparkles, and sleep marks.

Visibility and motion are independently adjustable from the mascot's status button. macOS Reduce Motion and inactive-window rendering pause continuous motion. The isolated `--ferret-review` mode exposes a visual state picker; a normal launch uses runtime evidence only.

## Validation boundaries

`CodeFerretTests` covers live request start/finish, out-of-order responses, durable work after HTTP completion, observation loss, real completion/failure, recovery, diagnostic exclusion, project/history changes, long-task/background behavior, localized blink pixels, paused motion, one-shot reactions, asset registration, and all eleven states in both appearances. Packaged resource preflight checks every required pose/eyelid/tail file.

A local runtime smoke is distinct from a ChatGPT Web conversation. The mascot cannot observe silent host reasoning between requests; it shows the latest bounded local evidence instead.
