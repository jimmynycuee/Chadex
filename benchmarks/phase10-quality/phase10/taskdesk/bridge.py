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
        if command == "task_history":
            try:
                requested_limit = int(arguments.get("limit", 10))
            except (TypeError, ValueError):
                requested_limit = 10
            return self.service.task_history(max(1, min(10, requested_limit)))
        if command == "clear_task_history":
            self.service.clear_task_history()
            return {"cleared": True}
        raise ValueError(f"unknown command: {command}")
