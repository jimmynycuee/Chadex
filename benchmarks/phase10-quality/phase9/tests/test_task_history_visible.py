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
