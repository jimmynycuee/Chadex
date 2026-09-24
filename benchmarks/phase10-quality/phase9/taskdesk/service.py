from __future__ import annotations

from pathlib import Path
from threading import RLock
from typing import Any

from .models import TaskResult
from .runtime import TaskRuntime
from .store import TaskHistoryStore


class TaskService:
    def __init__(self, runtime: TaskRuntime, data_dir: Path) -> None:
        self.runtime = runtime
        self.data_dir = data_dir
        self._lock = RLock()
        self._history = TaskHistoryStore(data_dir)
        self.last_result: TaskResult | None = None
        runtime.add_listener(self._on_result)

    def _on_result(self, result: TaskResult) -> None:
        with self._lock:
            self.last_result = result
        self._history.record(result)

    def task_history(self, limit: int = 10) -> list[dict[str, Any]]:
        return self._history.list(limit)

    def clear_task_history(self) -> None:
        self._history.clear()
