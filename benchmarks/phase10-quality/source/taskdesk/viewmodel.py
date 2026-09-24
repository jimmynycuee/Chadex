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
