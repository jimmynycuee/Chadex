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
