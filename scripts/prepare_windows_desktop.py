#!/usr/bin/env python3
"""Copy already-built Windows support binaries into the Tauri resource layout."""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import os
from pathlib import Path
import shutil
import sys
import tempfile
from typing import Iterator

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import windows_runtime_e2e as w2


RUNTIME_NAMES = (
    "chadex-runtime-cli",
    "chadex-runtime-server",
    "chadex-runtime-runner",
)


class PrepareFailure(RuntimeError):
    def __init__(self, code: str) -> None:
        super().__init__(code)
        self.code = code


@contextmanager
def cargo_target(value: Path | None) -> Iterator[None]:
    previous = os.environ.get("CARGO_TARGET_DIR")
    try:
        if value is not None:
            os.environ["CARGO_TARGET_DIR"] = str(value.expanduser().resolve())
        yield
    finally:
        if previous is None:
            os.environ.pop("CARGO_TARGET_DIR", None)
        else:
            os.environ["CARGO_TARGET_DIR"] = previous


def _debug_candidates(repo: Path) -> tuple[list[Path], list[Path]]:
    shared = os.environ.get("CARGO_TARGET_DIR")
    shared_debug = [Path(shared).expanduser().resolve() / "debug"] if shared else []
    helper = shared_debug + [repo / "rust-helper" / "target" / "debug"]
    runtime = shared_debug + [repo / "chadex-runtime" / "target" / "debug",
                              repo / "runtime-engine" / "target" / "debug"]
    return helper, runtime


def _local_binary_paths(repo: Path) -> tuple[Path, Path]:
    helper_dirs, runtime_dirs = _debug_candidates(repo)
    suffixes = (".exe", "") if sys.platform != "win32" else (".exe",)
    helper = next((directory / f"chadex-helper{suffix}"
                   for directory in helper_dirs for suffix in suffixes
                   if (directory / f"chadex-helper{suffix}").is_file()), None)
    runtime = next((directory for directory in runtime_dirs
                    if any(all((directory / f"{name}{suffix}").is_file() for name in RUNTIME_NAMES)
                           for suffix in suffixes)), None)
    if helper is None:
        raise PrepareFailure("debug_helper_missing")
    if runtime is None:
        raise PrepareFailure("debug_runtime_missing")
    return helper, runtime


def binary_paths(repo: Path, target_dir: Path | None) -> tuple[Path, Path]:
    """Use W2's exact candidate precedence, with native suffixes for local builds."""
    with cargo_target(target_dir):
        if sys.platform == "win32":
            try:
                return w2.binary_paths(repo)
            except w2.E2EFailure as error:
                raise PrepareFailure(error.code) from None
        return _local_binary_paths(repo)


def _copy_atomic(source: Path, destination: Path) -> None:
    if not source.is_file():
        raise PrepareFailure("debug_binary_missing")
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.is_symlink():
        raise PrepareFailure("destination_is_symlink")
    if source.resolve() == destination.resolve():
        return
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(prefix=".chadex-prepare-", dir=destination.parent,
                                         delete=False) as stream:
            temporary = Path(stream.name)
        shutil.copy2(source, temporary)
        os.replace(temporary, destination)
    except OSError:
        raise PrepareFailure("binary_copy_failed") from None
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def prepare(repo: Path, target_dir: Path | None, desktop_output: Path | None) -> int:
    repo = repo.expanduser().resolve()
    helper, runtime = binary_paths(repo, target_dir)
    suffix = helper.suffix
    sources = [(helper, Path("helper") / f"chadex-helper{suffix}")]
    sources.extend((runtime / f"{name}{helper.suffix}",
                    Path("chadex-runtime") / f"{name}{suffix}") for name in RUNTIME_NAMES)
    for source, relative in sources:
        if not source.is_file():
            raise PrepareFailure("debug_runtime_missing")
        _copy_atomic(source, repo / "apps" / "windows" / "src-tauri" / "resources" / relative)
        if desktop_output is not None:
            _copy_atomic(source, desktop_output.expanduser().resolve() / relative)
    return len(sources)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--target-dir", type=Path,
                        help="Cargo target root; defaults to CARGO_TARGET_DIR/W2 candidates")
    parser.add_argument("--desktop-output", type=Path,
                        help="also copy helper and runtime folders beside the debug desktop executable")
    args = parser.parse_args(argv)
    try:
        copied = prepare(args.repo_root, args.target_dir, args.desktop_output)
    except PrepareFailure as error:
        print(f'{{"status":"failed","error":"{error.code}"}}')
        return 1
    print(f'{{"status":"passed","copied_binaries":{copied}}}')
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
