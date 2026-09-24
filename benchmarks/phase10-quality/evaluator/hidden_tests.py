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
