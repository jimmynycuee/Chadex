#!/usr/bin/env python3
"""Derive the main executable hash Tauri embeds in a Windows NSIS bundle."""
from __future__ import annotations

import argparse
import hashlib
from pathlib import Path
import stat
import sys


BUNDLE_TYPE_UNKNOWN = b"__TAURI_BUNDLE_TYPE_VAR_UNK"
BUNDLE_TYPE_NSIS = b"__TAURI_BUNDLE_TYPE_VAR_NSS"


def patch_nsis_bundle_type(binary: bytes) -> bytes:
    """Apply Tauri's first-occurrence, same-length NSIS marker replacement."""
    if len(BUNDLE_TYPE_UNKNOWN) != len(BUNDLE_TYPE_NSIS):
        raise ValueError("bundle_type_marker_invalid")
    index = binary.find(BUNDLE_TYPE_UNKNOWN)
    if index < 0:
        raise ValueError("bundle_type_marker_missing")
    return (binary[:index] + BUNDLE_TYPE_NSIS
            + binary[index + len(BUNDLE_TYPE_UNKNOWN):])


def nsis_payload_sha256(binary: bytes) -> str:
    return hashlib.sha256(patch_nsis_bundle_type(binary)).hexdigest()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--path", required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        info = args.path.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
            raise ValueError("desktop_binary_invalid")
        print(nsis_payload_sha256(args.path.read_bytes()))
    except (OSError, ValueError) as exc:
        print(str(exc), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
