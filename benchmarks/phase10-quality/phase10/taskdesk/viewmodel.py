from __future__ import annotations

from typing import Any

from .service import TaskService


class TaskViewModel:
    def __init__(self, service: TaskService) -> None:
        self.service = service

    def last_task_label(self) -> str:
        result = self.service.last_result
        if result is None:
            return "No task"
        return f"{result.task_id}: {result.status}"

    def recent_task_rows(self, limit: int = 10) -> list[dict[str, Any]]:
        return self.service.task_history(limit)
