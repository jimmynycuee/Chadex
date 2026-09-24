from __future__ import annotations

import json
from pathlib import Path
from threading import RLock
from typing import Any


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
