from __future__ import annotations

from dataclasses import dataclass
from typing import Optional, Tuple


@dataclass(frozen=True)
class TaskResult:
    task_id: str
    status: str
    started_at: float
    finished_at: float
    changed_files: Tuple[str, ...] = ()
    validation_result: Optional[str] = None

    @property
    def duration_ms(self) -> int:
        return max(0, int(round((self.finished_at - self.started_at) * 1000)))
