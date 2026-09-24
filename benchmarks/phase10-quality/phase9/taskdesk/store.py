from __future__ import annotations

import json
import os
from pathlib import Path
import tempfile
from threading import RLock
from typing import Any

from .models import TaskResult


TERMINAL_STATUSES = {
    "completed",
    "failed",
    "failed_validation",
    "cancelled",
    "blocked",
}


class PreferencesStore:
    def __init__(self, data_dir: Path) -> None:
        self._path = data_dir / "preferences.json"
        self._lock = RLock()

    def load(self) -> dict[str, Any]:
        with self._lock:
            try:
                data = json.loads(self._path.read_text())
            except (FileNotFoundError, json.JSONDecodeError, OSError):
                return {}
            return data if isinstance(data, dict) else {}

    def save(self, values: dict[str, Any]) -> None:
        with self._lock:
            self._path.parent.mkdir(parents=True, exist_ok=True)
            self._path.write_text(json.dumps(values, sort_keys=True))


class TaskHistoryStore:
    MAX_ITEMS = 10
    MAX_CHANGED_FILES = 32
    SCHEMA_VERSION = 2

    def __init__(self, data_dir: Path) -> None:
        self._path = data_dir / "task-history.json"
        self._lock = RLock()

    def list(self, limit: int = MAX_ITEMS) -> list[dict[str, Any]]:
        bounded_limit = max(1, min(self.MAX_ITEMS, int(limit)))
        with self._lock:
            return [dict(item) for item in self._load_unlocked()[:bounded_limit]]

    def record(self, result: TaskResult) -> None:
        if result.status not in TERMINAL_STATUSES:
            return
        item = self._item_from_result(result)
        with self._lock:
            items = [
                existing
                for existing in self._load_unlocked()
                if existing.get("task_id") != result.task_id
            ]
            items.insert(0, item)
            self._write_unlocked(items[: self.MAX_ITEMS])

    def clear(self) -> None:
        with self._lock:
            self._write_unlocked([])

    def _load_unlocked(self) -> list[dict[str, Any]]:
        try:
            payload = json.loads(self._path.read_text())
        except (FileNotFoundError, json.JSONDecodeError, OSError, TypeError, ValueError):
            return []
        if not isinstance(payload, dict):
            return []

        if payload.get("schema_version") == 2:
            raw_items = payload.get("items", [])
        elif payload.get("schema_version") == 1:
            raw_items = payload.get("tasks", [])
        else:
            return []

        if not isinstance(raw_items, list):
            return []

        normalized: list[dict[str, Any]] = []
        seen_ids: set[str] = set()
        for raw in raw_items:
            item = self._normalize_item(raw)
            if item is None or item["task_id"] in seen_ids:
                continue
            normalized.append(item)
            seen_ids.add(item["task_id"])
            if len(normalized) >= self.MAX_ITEMS:
                break
        return normalized

    def _normalize_item(self, raw: Any) -> dict[str, Any] | None:
        if not isinstance(raw, dict):
            return None
        task_id = raw.get("task_id")
        status = raw.get("status")
        started_at = raw.get("started_at")
        finished_at = raw.get("finished_at")
        if not isinstance(task_id, str) or not task_id:
            return None
        if status not in TERMINAL_STATUSES:
            return None
        if not isinstance(started_at, (int, float)) or not isinstance(finished_at, (int, float)):
            return None

        changed_files = self._dedupe_files(raw.get("changed_files", []))
        validation_result = raw.get("validation_result")
        if validation_result is not None and not isinstance(validation_result, str):
            validation_result = str(validation_result)
        duration_ms = raw.get("duration_ms")
        if not isinstance(duration_ms, int) or duration_ms < 0:
            duration_ms = max(0, int(round((float(finished_at) - float(started_at)) * 1000)))

        return {
            "task_id": task_id,
            "status": status,
            "started_at": started_at,
            "finished_at": finished_at,
            "duration_ms": duration_ms,
            "changed_files": changed_files,
            "validation_result": validation_result,
        }

    def _item_from_result(self, result: TaskResult) -> dict[str, Any]:
        return {
            "task_id": result.task_id,
            "status": result.status,
            "started_at": result.started_at,
            "finished_at": result.finished_at,
            "duration_ms": result.duration_ms,
            "changed_files": self._dedupe_files(result.changed_files),
            "validation_result": result.validation_result,
        }

    def _dedupe_files(self, values: Any) -> list[str]:
        if not isinstance(values, (list, tuple)):
            return []
        result: list[str] = []
        seen: set[str] = set()
        for value in values:
            if not isinstance(value, str) or value in seen:
                continue
            seen.add(value)
            result.append(value)
            if len(result) >= self.MAX_CHANGED_FILES:
                break
        return result

    def _write_unlocked(self, items: list[dict[str, Any]]) -> None:
        self._path.parent.mkdir(parents=True, exist_ok=True)
        payload = {
            "schema_version": self.SCHEMA_VERSION,
            "items": items[: self.MAX_ITEMS],
        }
        temporary_path: Path | None = None
        try:
            with tempfile.NamedTemporaryFile(
                mode="w",
                encoding="utf-8",
                dir=self._path.parent,
                prefix=".task-history-",
                suffix=".tmp",
                delete=False,
            ) as handle:
                temporary_path = Path(handle.name)
                json.dump(payload, handle, sort_keys=True, separators=(",", ":"))
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temporary_path, self._path)
            temporary_path = None
        finally:
            if temporary_path is not None:
                try:
                    temporary_path.unlink()
                except FileNotFoundError:
                    pass
