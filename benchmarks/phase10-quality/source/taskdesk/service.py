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
