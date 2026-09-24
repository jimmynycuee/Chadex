#!/usr/bin/env python3
from __future__ import annotations

import json
from pathlib import Path
import shutil
import subprocess
import textwrap

ROOT = Path("benchmarks/phase10-quality").resolve()
SOURCE = ROOT / "source"
PHASE9 = ROOT / "phase9"
PHASE10 = ROOT / "phase10"
EVALUATOR = ROOT / "evaluator"

PROMPT = """Implement persistent Task History for this project.

Requirements:
- Persist terminal task results in `task-history.json` under the service data directory.
- Persist only statuses: completed, failed, failed_validation, cancelled, blocked.
- Use schema v2: {"schema_version": 2, "items": [...]}. Each item must include task_id, status,
  started_at, finished_at, duration_ms, changed_files, and validation_result.
- Keep at most 10 items, newest first.
- Recording an existing task_id replaces the older item and moves it to the front.
- changed_files must be de-duplicated while preserving order and capped at 32 entries.
- validation_result may be null.
- Reads must never crash on a missing or corrupt history file; corrupt content should behave as
  empty history and the next successful mutation should repair the file.
- Support legacy schema v1 {"schema_version": 1, "tasks": [...]} and migrate it to v2 on the next
  successful mutation without losing valid items.
- Writes must be atomic using a temporary file in the same directory followed by os.replace.
- History record/list/clear operations must be thread-safe.
- TaskService should automatically record terminal TaskResult events while preserving the existing
  last_result behavior.
- Add bridge commands `task_history` (optional integer limit, clamped 1..10) and
  `clear_task_history`. Existing `task_status` behavior must remain backward compatible.
- TaskViewModel should expose `recent_task_rows(limit=10)` for presentation without reading files
  directly.
- Do not add third-party dependencies.
- Keep the implementation focused and consistent with the existing architecture.
- Update/add visible tests as needed and run the full visible test suite.
"""

FILES = {
    "README.md": """# TaskDesk\n\nA tiny layered task runtime used for coding benchmarks.\n""",
    "taskdesk/__init__.py": "",
    "taskdesk/models.py": """
from __future__ import annotations

from dataclasses import dataclass
from typing import Optional, Tuple


@dataclass(frozen=True)
class TaskResult:
    task_id: str
    status: str
    started_at: float
    finished_at: float
    changed_files: Tuple[str, ...] = ()
    validation_result: Optional[str] = None

    @property
    def duration_ms(self) -> int:
        return max(0, int(round((self.finished_at - self.started_at) * 1000)))
""",
    "taskdesk/runtime.py": """
from __future__ import annotations

from collections.abc import Callable
from threading import RLock

from .models import TaskResult


class TaskRuntime:
    def __init__(self) -> None:
        self._listeners: list[Callable[[TaskResult], None]] = []
        self._lock = RLock()

    def add_listener(self, listener: Callable[[TaskResult], None]) -> None:
        with self._lock:
            self._listeners.append(listener)

    def emit(self, result: TaskResult) -> None:
        with self._lock:
            listeners = list(self._listeners)
        for listener in listeners:
            listener(result)
""",
    "taskdesk/store.py": """
from __future__ import annotations

import json
from pathlib import Path
from threading import RLock
from typing import Any


class PreferencesStore:
    def __init__(self, data_dir: Path) -> None:
        self._path = data_dir / "preferences.json"
        self._lock = RLock()

    def load(self) -> dict[str, Any]:
        with self._lock:
            try:
                data = json.loads(self._path.read_text())
            except (FileNotFoundError, json.JSONDecodeError, OSError):
                return {}
            return data if isinstance(data, dict) else {}

    def save(self, values: dict[str, Any]) -> None:
        with self._lock:
            self._path.parent.mkdir(parents=True, exist_ok=True)
            self._path.write_text(json.dumps(values, sort_keys=True))
""",
    "taskdesk/service.py": """
from __future__ import annotations

from pathlib import Path
from threading import RLock

from .models import TaskResult
from .runtime import TaskRuntime


class TaskService:
    def __init__(self, runtime: TaskRuntime, data_dir: Path) -> None:
        self.runtime = runtime
        self.data_dir = data_dir
        self._lock = RLock()
        self.last_result: TaskResult | None = None
        runtime.add_listener(self._on_result)

    def _on_result(self, result: TaskResult) -> None:
        with self._lock:
            self.last_result = result
""",
    "taskdesk/bridge.py": """
from __future__ import annotations

from typing import Any

from .service import TaskService


class TaskBridge:
    def __init__(self, service: TaskService) -> None:
        self.service = service

    def call(self, command: str, arguments: dict[str, Any] | None = None) -> Any:
        arguments = arguments or {}
        if command == "task_status":
            result = self.service.last_result
            if result is None:
                return None
            return {
                "task_id": result.task_id,
                "status": result.status,
                "duration_ms": result.duration_ms,
            }
        raise ValueError(f"unknown command: {command}")
""",
    "taskdesk/viewmodel.py": """
from __future__ import annotations

from .service import TaskService


class TaskViewModel:
    def __init__(self, service: TaskService) -> None:
        self.service = service

    def last_task_label(self) -> str:
        result = self.service.last_result
        if result is None:
            return "No task"
        return f"{result.task_id}: {result.status}"
""",
    "tests/__init__.py": "",
    "tests/test_existing_behavior.py": """
from __future__ import annotations

from pathlib import Path
import tempfile
import unittest

from taskdesk.bridge import TaskBridge
from taskdesk.models import TaskResult
from taskdesk.runtime import TaskRuntime
from taskdesk.service import TaskService
from taskdesk.viewmodel import TaskViewModel


class ExistingBehaviorTests(unittest.TestCase):
    def test_status_and_label_remain_available(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            runtime = TaskRuntime()
            service = TaskService(runtime, Path(directory))
            bridge = TaskBridge(service)
            view_model = TaskViewModel(service)
            runtime.emit(TaskResult("t1", "completed", 1.0, 1.25, ("a.py",), "passed"))
            self.assertEqual(bridge.call("task_status"), {
                "task_id": "t1",
                "status": "completed",
                "duration_ms": 250,
            })
            self.assertEqual(view_model.last_task_label(), "t1: completed")


if __name__ == "__main__":
    unittest.main()
""",
    "tests/test_task_history_visible.py": """
from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest

from taskdesk.bridge import TaskBridge
from taskdesk.models import TaskResult
from taskdesk.runtime import TaskRuntime
from taskdesk.service import TaskService
from taskdesk.viewmodel import TaskViewModel


class TaskHistoryVisibleTests(unittest.TestCase):
    def make_stack(self, directory: str):
        runtime = TaskRuntime()
        service = TaskService(runtime, Path(directory))
        return runtime, service, TaskBridge(service), TaskViewModel(service)

    def test_terminal_result_persists_and_reloads(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            runtime, service, bridge, _ = self.make_stack(directory)
            runtime.emit(TaskResult("a", "completed", 10.0, 10.5, ("x.py", "x.py", "y.py"), "passed"))
            items = bridge.call("task_history")
            self.assertEqual(items[0]["task_id"], "a")
            self.assertEqual(items[0]["changed_files"], ["x.py", "y.py"])
            _, _, bridge2, _ = self.make_stack(directory)
            self.assertEqual(bridge2.call("task_history")[0]["task_id"], "a")

    def test_non_terminal_results_are_not_persisted(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            runtime, _, bridge, _ = self.make_stack(directory)
            runtime.emit(TaskResult("running", "running", 1.0, 2.0))
            self.assertEqual(bridge.call("task_history"), [])

    def test_history_is_capped_and_clearable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            runtime, _, bridge, _ = self.make_stack(directory)
            for index in range(12):
                runtime.emit(TaskResult(str(index), "completed", index, index + 0.1))
            items = bridge.call("task_history", {"limit": 99})
            self.assertEqual(len(items), 10)
            self.assertEqual(items[0]["task_id"], "11")
            self.assertEqual(items[-1]["task_id"], "2")
            self.assertEqual(bridge.call("clear_task_history"), {"cleared": True})
            self.assertEqual(bridge.call("task_history"), [])

    def test_view_model_exposes_recent_rows(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            runtime, _, _, view_model = self.make_stack(directory)
            runtime.emit(TaskResult("a", "failed", 1.0, 1.25, ("one.py",), "failed"))
            rows = view_model.recent_task_rows(limit=3)
            self.assertEqual(rows[0]["task_id"], "a")
            self.assertEqual(rows[0]["status"], "failed")


if __name__ == "__main__":
    unittest.main()
""",
}

HIDDEN = r'''
from __future__ import annotations

import json
from pathlib import Path
import tempfile
from concurrent.futures import ThreadPoolExecutor
import unittest
import sys

ROOT = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(ROOT))

from taskdesk.bridge import TaskBridge
from taskdesk.models import TaskResult
from taskdesk.runtime import TaskRuntime
from taskdesk.service import TaskService
from taskdesk.viewmodel import TaskViewModel


class HiddenTaskHistoryTests(unittest.TestCase):
    def stack(self, directory: str):
        runtime = TaskRuntime()
        service = TaskService(runtime, Path(directory))
        return runtime, service, TaskBridge(service), TaskViewModel(service)

    def test_all_terminal_statuses_persist(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime, _, bridge, _ = self.stack(directory)
            statuses = ["completed", "failed", "failed_validation", "cancelled", "blocked"]
            for index, status in enumerate(statuses):
                runtime.emit(TaskResult(str(index), status, index, index + 0.01))
            self.assertEqual([x["status"] for x in bridge.call("task_history")], list(reversed(statuses)))

    def test_duplicate_id_replaces_and_moves_to_front(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime, _, bridge, _ = self.stack(directory)
            runtime.emit(TaskResult("same", "failed", 1, 2, ("old.py",), "failed"))
            runtime.emit(TaskResult("other", "completed", 2, 3))
            runtime.emit(TaskResult("same", "completed", 3, 4, ("new.py",), "passed"))
            items = bridge.call("task_history")
            self.assertEqual([x["task_id"] for x in items], ["same", "other"])
            self.assertEqual(items[0]["status"], "completed")
            self.assertEqual(items[0]["changed_files"], ["new.py"])

    def test_changed_files_are_deduped_and_capped(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime, _, bridge, _ = self.stack(directory)
            values = tuple([f"f{i}.py" for i in range(40)] + ["f1.py", "f2.py"])
            runtime.emit(TaskResult("cap", "completed", 1, 2, values))
            files = bridge.call("task_history")[0]["changed_files"]
            self.assertEqual(len(files), 32)
            self.assertEqual(files[:3], ["f0.py", "f1.py", "f2.py"])
            self.assertEqual(len(files), len(set(files)))

    def test_corrupt_file_reads_empty_and_next_write_repairs(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "task-history.json"
            path.write_text("{not-json")
            runtime, _, bridge, _ = self.stack(directory)
            self.assertEqual(bridge.call("task_history"), [])
            runtime.emit(TaskResult("fixed", "completed", 1, 2))
            data = json.loads(path.read_text())
            self.assertEqual(data["schema_version"], 2)
            self.assertEqual(data["items"][0]["task_id"], "fixed")

    def test_v1_migrates_on_next_mutation(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "task-history.json"
            path.write_text(json.dumps({
                "schema_version": 1,
                "tasks": [{
                    "task_id": "legacy", "status": "completed", "started_at": 1,
                    "finished_at": 2, "changed_files": ["old.py"], "validation_result": "passed"
                }]
            }))
            runtime, _, bridge, _ = self.stack(directory)
            self.assertEqual(bridge.call("task_history")[0]["task_id"], "legacy")
            runtime.emit(TaskResult("new", "completed", 3, 4))
            data = json.loads(path.read_text())
            self.assertEqual(data["schema_version"], 2)
            self.assertEqual([x["task_id"] for x in data["items"]], ["new", "legacy"])

    def test_bridge_limit_is_clamped(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime, _, bridge, _ = self.stack(directory)
            for index in range(4):
                runtime.emit(TaskResult(str(index), "completed", index, index + 1))
            self.assertEqual(len(bridge.call("task_history", {"limit": 0})), 1)
            self.assertEqual(len(bridge.call("task_history", {"limit": 999})), 4)

    def test_concurrent_recording_keeps_valid_bounded_json(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime, _, bridge, _ = self.stack(directory)
            def emit(index: int):
                runtime.emit(TaskResult(str(index), "completed", index, index + .01, (f"{index}.py",)))
            with ThreadPoolExecutor(max_workers=8) as pool:
                list(pool.map(emit, range(50)))
            path = Path(directory) / "task-history.json"
            data = json.loads(path.read_text())
            self.assertEqual(data["schema_version"], 2)
            self.assertLessEqual(len(data["items"]), 10)
            self.assertEqual(len({x["task_id"] for x in data["items"]}), len(data["items"]))
            self.assertLessEqual(len(bridge.call("task_history")), 10)

    def test_atomic_replace_does_not_leave_temp_files(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime, _, _, _ = self.stack(directory)
            runtime.emit(TaskResult("a", "completed", 1, 2))
            leftovers = [p.name for p in Path(directory).iterdir() if p.name != "task-history.json"]
            self.assertEqual(leftovers, [])

    def test_duration_is_nonnegative_and_persisted(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime, _, bridge, _ = self.stack(directory)
            runtime.emit(TaskResult("backwards", "failed", 5, 4))
            self.assertEqual(bridge.call("task_history")[0]["duration_ms"], 0)

    def test_existing_status_api_stays_backward_compatible(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime, _, bridge, _ = self.stack(directory)
            runtime.emit(TaskResult("x", "cancelled", 1, 1.1))
            self.assertEqual(bridge.call("task_status"), {
                "task_id": "x", "status": "cancelled", "duration_ms": 100
            })


if __name__ == "__main__":
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(HiddenTaskHistoryTests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    print(f"HIDDEN_SCORE={result.testsRun - len(result.failures) - len(result.errors)}/{result.testsRun}")
    raise SystemExit(0 if result.wasSuccessful() else 1)
'''


def write_project(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=True)
    for relative, content in FILES.items():
        target = path / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(textwrap.dedent(content).lstrip())
    subprocess.run(["git", "init", "-q"], cwd=path, check=True)
    subprocess.run(["git", "config", "user.name", "Phase10 Quality Benchmark"], cwd=path, check=True)
    subprocess.run(["git", "config", "user.email", "phase10-quality@example.invalid"], cwd=path, check=True)
    subprocess.run(["git", "add", "."], cwd=path, check=True)
    subprocess.run(["git", "commit", "-qm", "benchmark baseline"], cwd=path, check=True)


def main() -> None:
    if ROOT.exists():
        shutil.rmtree(ROOT)
    write_project(SOURCE)
    subprocess.run(["python3", "-m", "graphify", "update", "."], cwd=SOURCE, check=True, timeout=120)
    for destination in [PHASE9, PHASE10]:
        shutil.copytree(SOURCE, destination)
    EVALUATOR.mkdir(parents=True)
    (EVALUATOR / "hidden_tests.py").write_text(HIDDEN.lstrip())
    (ROOT / "prompt.txt").write_text(PROMPT)
    print(json.dumps({
        "root": str(ROOT),
        "phase9": str(PHASE9),
        "phase10": str(PHASE10),
        "prompt": PROMPT,
    }, indent=2))


if __name__ == "__main__":
    main()
