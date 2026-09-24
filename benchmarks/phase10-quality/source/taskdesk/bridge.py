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
