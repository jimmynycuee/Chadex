#!/usr/bin/env python3
"""Out-of-band Chadex runtime heartbeat sampler for Phase 19A.

The sampler observes local process liveness plus tunnel-client's loopback health
API. It never reads Chadex bearer credentials and never calls ChatGPT tools, so
heartbeat sampling does not contaminate benchmark tool-call counts.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import time
from typing import Any
from urllib import request as urlrequest


DEFAULT_STATE_DIR = Path.home() / "Library/Application Support/Chadex/runtime"


def latest_health_url(state_dir: Path) -> str:
    candidates: list[tuple[float, Path]] = []
    sessions = state_dir / "tunnel-sessions"
    if sessions.is_dir():
        for path in sessions.glob("*/health-url"):
            try:
                candidates.append((path.stat().st_mtime, path))
            except OSError:
                continue
    if not candidates:
        raise FileNotFoundError(f"no tunnel health-url found under {sessions}")
    _, path = max(candidates)
    value = path.read_text(encoding="utf-8").strip()
    if not value.startswith("http://127.0.0.1:"):
        raise ValueError("tunnel health URL is not loopback")
    return value.rstrip("/")


def process_layers() -> dict[str, dict[str, Any]]:
    ps = subprocess.check_output(
        ["ps", "ax", "-o", "command="], text=True, stderr=subprocess.DEVNULL
    )
    checks = {
        "app_process": "/Applications/Chadex.app/Contents/MacOS/Chadex",
        "helper_process": "/Applications/Chadex.app/Contents/Helpers/chadex-helper",
        "runtime_server": "/Applications/Chadex.app/Contents/Resources/chadex-runtime/chadex-runtime-server",
        "runtime_runner": "/Applications/Chadex.app/Contents/Resources/chadex-runtime/chadex-runtime-runner",
        "tunnel_client": "Application Support/Chadex/runtime/tools/tunnel-client",
    }
    return {
        name: {
            "status": "ready" if needle in ps else "offline",
            "source": "process_table",
        }
        for name, needle in checks.items()
    }


def tunnel_status(state_dir: Path, timeout: float) -> dict[str, Any]:
    base_url = latest_health_url(state_dir)
    req = urlrequest.Request(f"{base_url}/api/status", method="GET")
    with urlrequest.urlopen(req, timeout=timeout) as response:
        value = json.loads(response.read().decode("utf-8"))
    if not isinstance(value, dict):
        raise ValueError("tunnel /api/status did not return a JSON object")
    return value


def connection_layers_from_tunnel(value: dict[str, Any]) -> dict[str, dict[str, Any]]:
    channels = value.get("channels")
    main = None
    if isinstance(channels, list):
        main = next(
            (
                row
                for row in channels
                if isinstance(row, dict) and row.get("name") == "main"
            ),
            None,
        )
    probe = main.get("probe_status") if isinstance(main, dict) else None
    tunnel_id = value.get("control_plane_tunnel_id")
    return {
        "tunnel_health": {"status": "ready", "source": "tunnel_client_health_api"},
        "server_transport": {
            "status": "connected" if probe == "ok" else "unavailable",
            "source": "tunnel_client_main_channel_probe",
            "probe_status": probe,
        },
        "control_plane": {
            "status": "connected"
            if isinstance(tunnel_id, str) and tunnel_id
            else "unavailable",
            "source": "tunnel_client_status",
        },
    }


def fetch_status(state_dir: Path, client_id: str, timeout: float) -> dict[str, Any]:
    layers = process_layers()
    status = tunnel_status(state_dir, timeout)
    layers.update(connection_layers_from_tunnel(status))
    return {
        "client_id": client_id,
        "connection_layers": layers,
        "tunnel": {
            "version": status.get("version"),
            "client_instance_id": status.get("client_instance_id"),
            "started_at": status.get("started_at"),
            "uptime_seconds": status.get("uptime_seconds"),
            "control_plane_tunnel_id": status.get("control_plane_tunnel_id"),
        },
    }


def write_row(handle, row: dict[str, Any]) -> None:
    handle.write(json.dumps(row, ensure_ascii=False, sort_keys=True) + "\n")
    handle.flush()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--state-dir", type=Path, default=DEFAULT_STATE_DIR)
    parser.add_argument("--client-id", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--interval", type=float, default=1.0)
    parser.add_argument("--timeout", type=float, default=2.0)
    parser.add_argument("--duration", type=float)
    args = parser.parse_args()
    if args.interval <= 0 or args.timeout <= 0:
        parser.error("--interval and --timeout must be > 0")
    if args.duration is not None and args.duration <= 0:
        parser.error("--duration must be > 0")

    state_dir = args.state_dir.expanduser().resolve()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()

    with args.output.open("a", encoding="utf-8") as handle:
        while True:
            sampled_at_ms = time.time_ns() // 1_000_000
            try:
                status = fetch_status(state_dir, args.client_id, args.timeout)
                write_row(handle, {"sampled_at_ms": sampled_at_ms, "ok": True, **status})
            except Exception as exc:
                write_row(
                    handle,
                    {
                        "sampled_at_ms": sampled_at_ms,
                        "ok": False,
                        "error_type": type(exc).__name__,
                        "error": str(exc)[:512],
                    },
                )
            if args.duration is not None and time.monotonic() - started >= args.duration:
                break
            time.sleep(args.interval)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
