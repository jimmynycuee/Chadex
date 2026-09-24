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
