#!/usr/bin/env python3
"""Extract and validate Tauri CLI's base64 updater signature output."""
from __future__ import annotations

import argparse
import base64
import binascii
from pathlib import Path
import re
import sys


VERSION_PATTERN = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")


class SignatureOutputError(ValueError):
    pass


def extract_signature(output: str, version: str) -> str:
    if not VERSION_PATTERN.fullmatch(version):
        raise SignatureOutputError("release_version_invalid")
    lines = output.splitlines()
    markers = [index for index, line in enumerate(lines) if line.strip() == "Public signature:"]
    if len(markers) != 1 or markers[0] + 1 >= len(lines):
        raise SignatureOutputError("public_signature_missing_or_ambiguous")
    encoded = lines[markers[0] + 1].strip()
    if not encoded or not re.fullmatch(r"[A-Za-z0-9+/]+={0,2}", encoded):
        raise SignatureOutputError("public_signature_invalid")
    try:
        raw_signature = base64.b64decode(encoded, validate=True).decode("utf-8")
    except (binascii.Error, UnicodeDecodeError):
        raise SignatureOutputError("public_signature_invalid") from None
    if not raw_signature.startswith("untrusted comment:"):
        raise SignatureOutputError("minisign_signature_invalid")
    trusted_comments = [
        line for line in raw_signature.splitlines() if line.startswith("trusted comment:")
    ]
    expected_version = re.compile(rf"(?:^|\s)version:{re.escape(version)}(?:$|\s)")
    if len(trusted_comments) != 1 or not expected_version.search(trusted_comments[0]):
        raise SignatureOutputError("signature_version_mismatch")
    return encoded


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        signature = extract_signature(sys.stdin.read(), args.version)
        args.output.write_text(signature, encoding="ascii")
    except (OSError, SignatureOutputError) as error:
        code = str(error) if isinstance(error, SignatureOutputError) else "signature_output_unavailable"
        print(code, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
