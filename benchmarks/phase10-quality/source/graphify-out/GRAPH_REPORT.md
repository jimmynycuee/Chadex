# Graph Report - source  (2026-09-20)

## Corpus Check
- 11 files · ~603 words
- Verdict: corpus is large enough that graph structure adds value.

## Summary
- 43 nodes · 101 edges · 10 communities (5 shown, 5 thin omitted)
- Extraction: 85% EXTRACTED · 15% INFERRED · 0% AMBIGUOUS · INFERRED: 15 edges (avg confidence: 0.5)
- Token cost: 0 input · 0 output

## Graph Freshness
- Built from commit: `fe3b5fa0`
- Run `git rev-parse HEAD` and compare to check if the graph is stale.
- Run `graphify update .` after code changes (no API cost).

## Community Hubs (Navigation)
- TaskResult
- PreferencesStore
- TaskService
- TaskRuntime
- TaskViewModel
- README.md
- .call

## God Nodes (most connected - your core abstractions)
1. `TaskResult` - 18 edges
2. `TaskService` - 17 edges
3. `TaskRuntime` - 14 edges
4. `TaskHistoryVisibleTests` - 11 edges
5. `TaskBridge` - 10 edges
6. `TaskViewModel` - 10 edges
7. `ExistingBehaviorTests` - 7 edges
8. `PreferencesStore` - 4 edges
9. `TaskDesk` - 1 edges

## Surprising Connections (you probably didn't know these)
- `TaskHistoryVisibleTests` --uses--> `TaskBridge`  [INFERRED]
  tests/test_task_history_visible.py → taskdesk/bridge.py
- `ExistingBehaviorTests` --uses--> `TaskResult`  [INFERRED]
  tests/test_existing_behavior.py → taskdesk/models.py
- `ExistingBehaviorTests` --uses--> `TaskRuntime`  [INFERRED]
  tests/test_existing_behavior.py → taskdesk/runtime.py
- `TaskHistoryVisibleTests` --uses--> `TaskRuntime`  [INFERRED]
  tests/test_task_history_visible.py → taskdesk/runtime.py
- `TaskHistoryVisibleTests` --uses--> `TaskService`  [INFERRED]
  tests/test_task_history_visible.py → taskdesk/service.py

## Import Cycles
- None detected.

## Communities (10 total, 5 thin omitted)

### Community 2 - "PreferencesStore"
Cohesion: 0.33
Nodes (3): PreferencesStore, Any, Path

### Community 3 - "TaskService"
Cohesion: 0.60
Nodes (3): TaskBridge, TaskService, ExistingBehaviorTests

## Knowledge Gaps
- **1 isolated node(s):** `TaskDesk`
  These have ≤1 connection - possible missing edges or undocumented components.
- **5 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `TaskResult` connect `TaskResult` to `test_existing_behavior.py`, `TaskService`, `TaskRuntime`?**
  _High betweenness centrality (0.135) - this node is a cross-community bridge._
- **Why does `TaskService` connect `TaskService` to `TaskResult`, `test_existing_behavior.py`, `TaskRuntime`, `TaskViewModel`?**
  _High betweenness centrality (0.123) - this node is a cross-community bridge._
- **Why does `TaskRuntime` connect `TaskRuntime` to `TaskResult`, `test_existing_behavior.py`, `TaskService`?**
  _High betweenness centrality (0.089) - this node is a cross-community bridge._
- **Are the 4 inferred relationships involving `TaskResult` (e.g. with `TaskRuntime` and `TaskService`) actually correct?**
  _`TaskResult` has 4 INFERRED edges - model-reasoned connections that need verification._
- **Are the 6 inferred relationships involving `TaskService` (e.g. with `TaskBridge` and `TaskResult`) actually correct?**
  _`TaskService` has 6 INFERRED edges - model-reasoned connections that need verification._
- **Are the 4 inferred relationships involving `TaskRuntime` (e.g. with `TaskResult` and `TaskService`) actually correct?**
  _`TaskRuntime` has 4 INFERRED edges - model-reasoned connections that need verification._
- **Are the 5 inferred relationships involving `TaskHistoryVisibleTests` (e.g. with `TaskBridge` and `TaskResult`) actually correct?**
  _`TaskHistoryVisibleTests` has 5 INFERRED edges - model-reasoned connections that need verification._