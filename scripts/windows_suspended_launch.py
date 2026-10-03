"""Seed an exact Windows process identity before it can launch descendants."""
from __future__ import annotations

import ctypes
from pathlib import Path
import subprocess
import sys
from typing import Any, Callable

import windows_runtime_e2e as w2


class LaunchFailure(RuntimeError):
    pass


def _resume_primary_thread(process: subprocess.Popen[Any]) -> None:
    # CREATE_SUSPENDED leaves exactly the primary thread. Retain a thread handle
    # and verify its owning process before resuming that same handle.
    if sys.platform != "win32":
        raise LaunchFailure("suspended_launch_windows_required")
    api = ctypes.WinDLL("kernel32", use_last_error=True)
    handle = ctypes.c_void_p
    word = ctypes.c_uint32

    class ThreadEntry(ctypes.Structure):
        _fields_ = [("size", word), ("usage", word), ("tid", word),
                    ("pid", word), ("base_priority", ctypes.c_int32),
                    ("delta_priority", ctypes.c_int32), ("flags", word)]

    api.CreateToolhelp32Snapshot.argtypes = [word, word]
    api.CreateToolhelp32Snapshot.restype = handle
    for name in ("Thread32First", "Thread32Next"):
        function = getattr(api, name)
        function.argtypes = [handle, ctypes.POINTER(ThreadEntry)]
        function.restype = ctypes.c_int
    api.OpenThread.argtypes = [word, ctypes.c_int, word]
    api.OpenThread.restype = handle
    api.GetProcessIdOfThread.argtypes = [handle]
    api.GetProcessIdOfThread.restype = word
    api.ResumeThread.argtypes = [handle]
    api.ResumeThread.restype = word
    api.CloseHandle.argtypes = [handle]
    api.CloseHandle.restype = ctypes.c_int

    snapshot = api.CreateToolhelp32Snapshot(0x00000004, 0)
    if snapshot in (None, ctypes.c_void_p(-1).value):
        raise LaunchFailure("primary_thread_snapshot_failed")
    try:
        entry = ThreadEntry()
        entry.size = ctypes.sizeof(entry)
        present = api.Thread32First(snapshot, ctypes.byref(entry))
        tids = []
        while present:
            if entry.pid == process.pid:
                tids.append(entry.tid)
            entry.size = ctypes.sizeof(entry)
            present = api.Thread32Next(snapshot, ctypes.byref(entry))
        if len(tids) != 1 or process.poll() is not None:
            raise LaunchFailure("primary_thread_identity_invalid")
        thread = api.OpenThread(0x0002 | 0x0800, False, tids[0])
        if not thread:
            raise LaunchFailure("primary_thread_open_failed")
        try:
            if api.GetProcessIdOfThread(thread) != process.pid or process.poll() is not None:
                raise LaunchFailure("primary_thread_identity_invalid")
            if api.ResumeThread(thread) != 1:
                raise LaunchFailure("primary_thread_resume_failed")
        finally:
            api.CloseHandle(thread)
    finally:
        api.CloseHandle(snapshot)


def launch_owned(arguments: str | list[str], *, executable: Path, cwd: Path,
                 env: dict[str, str] | None, powershell: str,
                 owned: dict[int, dict[str, Any]], on_forced: Callable[[], None],
                 popen: Callable[..., Any] = subprocess.Popen,
                 resume: Callable[[Any], None] = _resume_primary_thread,
                 sample: Callable[..., Any] = w2.remember_tree) -> subprocess.Popen[Any]:
    if owned:
        raise LaunchFailure("initial_ownership_not_empty")
    process = popen(arguments, executable=str(executable), cwd=cwd, env=env,
                    shell=False, creationflags=0x00000004, stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        if process.poll() is not None:
            raise LaunchFailure("initial_process_exited")
        candidate: dict[int, dict[str, Any]] = {}
        sample(powershell, process.pid, candidate)
        root = candidate.get(process.pid)
        if process.poll() is not None or not root or not root.get("Created") or \
                root.get("Name", "").casefold() != executable.name.casefold():
            raise LaunchFailure("initial_process_identity_invalid")
        # Do not expose an untrusted temporary sample to fallback cleanup.
        owned.update(candidate)
        resume(process)
        return process
    except Exception:
        # Popen.kill uses the original Windows process handle, never a PID tree.
        if process.poll() is None:
            process.kill()
            on_forced()
            process.wait(timeout=10)
        raise
