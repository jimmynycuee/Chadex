#!/usr/bin/env python3
"""Generate the Tauri updater manifest for one signed Windows release asset."""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import sys
import tempfile


VERSION_RE = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
REPOSITORY_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
SIGNATURE_MAX_BYTES = 16 * 1024


class ManifestError(ValueError):
    """Raised when release metadata cannot safely form an updater manifest."""


def validate_version_and_tag(version: str, tag: str) -> None:
    if not VERSION_RE.fullmatch(version):
        raise ManifestError("version must be a stable MAJOR.MINOR.PATCH value")
    tag_pattern = re.compile(
        rf"^v{re.escape(version)}(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?$"
    )
    if not tag_pattern.fullmatch(tag):
        raise ManifestError("tag must be v<version> with an optional safe prerelease suffix")


def validate_repository(repository: str) -> None:
    if not REPOSITORY_RE.fullmatch(repository):
        raise ManifestError("repository must be a safe GitHub owner/repository pair")
    owner, name = repository.split("/", 1)
    if owner in {".", ".."} or name in {".", ".."}:
        raise ManifestError("repository components cannot be dot paths")


def validate_signature(signature: str) -> str:
    try:
        encoded = signature.encode("utf-8")
    except UnicodeEncodeError as error:
        raise ManifestError("signature must be printable ASCII text") from error
    if not encoded or len(encoded) > SIGNATURE_MAX_BYTES or not signature.strip():
        raise ManifestError("signature must be non-empty and at most 16 KiB")
    if any(ord(char) > 0x7E or (ord(char) < 0x20 and char not in "\r\n") for char in signature):
        raise ManifestError("signature must contain printable ASCII and line breaks only")
    return signature


def default_pub_date() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def validate_pub_date(pub_date: str) -> str:
    try:
        parsed = datetime.fromisoformat(pub_date.replace("Z", "+00:00"))
    except ValueError as error:
        raise ManifestError("pub_date must be an ISO-8601 timestamp") from error
    if parsed.tzinfo is None or parsed.utcoffset() is None:
        raise ManifestError("pub_date must include a timezone")
    return pub_date


def build_manifest(
    *,
    version: str,
    tag: str,
    repository: str,
    signature: str,
    pub_date: str | None = None,
) -> dict[str, object]:
    validate_version_and_tag(version, tag)
    validate_repository(repository)
    signature = validate_signature(signature)
    pub_date = validate_pub_date(pub_date or default_pub_date())
    archive_name = f"Chadex-{version}-windows-x64-setup.nsis.zip"
    return {
        "version": version,
        "notes": f"Chadex {version} Windows x64 update",
        "pub_date": pub_date,
        "platforms": {
            "windows-x86_64": {
                "signature": signature,
                "url": (
                    f"https://github.com/{repository}/releases/download/"
                    f"{tag}/{archive_name}"
                ),
            }
        },
    }


def generate_manifest(
    *,
    version: str,
    tag: str,
    repository: str,
    archive: Path,
    signature_file: Path,
    output: Path,
    pub_date: str | None = None,
) -> dict[str, object]:
    validate_version_and_tag(version, tag)
    expected_archive_name = f"Chadex-{version}-windows-x64-setup.nsis.zip"
    if archive.name != expected_archive_name:
        raise ManifestError("archive filename does not match the versioned release asset name")
    if signature_file.name != f"{expected_archive_name}.sig":
        raise ManifestError("signature filename must be the updater archive filename plus .sig")
    if not archive.is_file():
        raise ManifestError("signed updater archive is missing")
    if not signature_file.is_file():
        raise ManifestError("Tauri updater signature file is missing")
    if signature_file.stat().st_size > SIGNATURE_MAX_BYTES:
        raise ManifestError("signature must be non-empty and at most 16 KiB")
    try:
        signature = signature_file.read_bytes().decode("ascii")
    except (OSError, UnicodeError) as error:
        raise ManifestError("Tauri updater signature must be readable ASCII text") from error
    manifest = build_manifest(
        version=version,
        tag=tag,
        repository=repository,
        signature=signature,
        pub_date=pub_date,
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    serialized = json.dumps(manifest, ensure_ascii=True, indent=2) + "\n"
    temporary_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w", encoding="utf-8", newline="\n", dir=output.parent,
            prefix=f".{output.name}.", suffix=".tmp", delete=False,
        ) as temporary:
            temporary.write(serialized)
            temporary_path = Path(temporary.name)
        temporary_path.replace(output)
    finally:
        if temporary_path is not None and temporary_path.exists():
            temporary_path.unlink()
    return manifest


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--signature-file", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        generate_manifest(
            version=args.version,
            tag=args.tag,
            repository=args.repository,
            archive=args.archive,
            signature_file=args.signature_file,
            output=args.output,
        )
    except ManifestError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
