#!/usr/bin/env python3
"""Stage release-only Windows resources and write a path-free manifest."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import struct
import tempfile
import tomllib
from typing import Any


AMD64_MACHINE = 0x8664
PE32_PLUS_MAGIC = 0x20B
SCHEMA = "chadex.windows.release-resources.v1"
PRODUCTION_FEATURES = {
    "custom-protocol": True,
    "desktop-smoke": False,
}
EXECUTABLES = (
    ("chadex-helper.exe", Path("helper") / "chadex-helper.exe"),
    ("chadex-runtime-cli.exe", Path("chadex-runtime") / "chadex-runtime-cli.exe"),
    ("chadex-runtime-server.exe", Path("chadex-runtime") / "chadex-runtime-server.exe"),
    ("chadex-runtime-runner.exe", Path("chadex-runtime") / "chadex-runtime-runner.exe"),
)
LICENSES = (
    (Path("LICENSE"), Path("licenses") / "Chadex-LICENSE.txt"),
    (Path("attribution") / "WebCodex-LICENSE.txt",
     Path("licenses") / "WebCodex-LICENSE.txt"),
    (Path("UPSTREAM.md"), Path("licenses") / "UPSTREAM.md"),
)


class ReleasePreparationError(RuntimeError):
    """A safe, stable error code for callers and the command line."""

    def __init__(self, code: str) -> None:
        super().__init__(code)
        self.code = code


def _regular_file(path: Path, code: str) -> None:
    try:
        mode = path.lstat().st_mode
    except OSError:
        raise ReleasePreparationError(code) from None
    if stat.S_ISLNK(mode) or not stat.S_ISREG(mode):
        raise ReleasePreparationError(code)


def _read_versions(repo: Path) -> str:
    config_path = repo / "apps" / "windows" / "src-tauri" / "tauri.conf.json"
    package_path = repo / "apps" / "windows" / "package.json"
    cargo_path = repo / "apps" / "windows" / "src-tauri" / "Cargo.toml"
    try:
        config = json.loads(config_path.read_text(encoding="utf-8"))
        package = json.loads(package_path.read_text(encoding="utf-8"))
        cargo_text = cargo_path.read_text(encoding="utf-8")
    except (OSError, UnicodeError, json.JSONDecodeError):
        raise ReleasePreparationError("version_metadata_invalid") from None
    if not isinstance(config, dict) or not isinstance(package, dict):
        raise ReleasePreparationError("version_metadata_invalid")

    tauri_version = config.get("version")
    if tauri_version is None and isinstance(config.get("package"), dict):
        tauri_version = config["package"].get("version")
    package_version = package.get("version")

    in_package = False
    cargo_version: str | None = None
    for line in cargo_text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            in_package = stripped == "[package]"
            continue
        if in_package:
            match = re.fullmatch(r'version\s*=\s*"([^"\r\n]+)"\s*(?:#.*)?', stripped)
            if match:
                cargo_version = match.group(1)
                break

    versions = (tauri_version, package_version, cargo_version)
    if not all(isinstance(value, str) and value for value in versions):
        raise ReleasePreparationError("version_metadata_invalid")
    if len(set(versions)) != 1:
        raise ReleasePreparationError("version_mismatch")
    return tauri_version


def _release_directory(target_dir: Path) -> Path:
    release = target_dir / "release"
    try:
        mode = release.lstat().st_mode
    except OSError:
        raise ReleasePreparationError("release_directory_missing") from None
    if stat.S_ISLNK(mode) or not stat.S_ISDIR(mode):
        raise ReleasePreparationError("release_directory_invalid")
    return release


def check_build_inputs(repo: Path) -> str:
    version = _read_versions(repo)
    windows = repo / "apps/windows"
    try:
        package = json.loads((windows / "package.json").read_text(encoding="utf-8"))
        npm_lock = json.loads((windows / "package-lock.json").read_text(encoding="utf-8"))
        cargo_lock = tomllib.loads((windows / "src-tauri/Cargo.lock").read_text(encoding="utf-8"))
        api = package["dependencies"]["@tauri-apps/api"]
        root_api = npm_lock["packages"][""]["dependencies"]["@tauri-apps/api"]
        locked_api = npm_lock["packages"]["node_modules/@tauri-apps/api"]["version"]
        rust = [item["version"] for item in cargo_lock["package"] if item["name"] == "tauri"]
    except (OSError, UnicodeError, ValueError, KeyError, TypeError):
        raise ReleasePreparationError("tauri_dependency_metadata_invalid") from None
    if not isinstance(api, str) or not re.fullmatch(r"\d+\.\d+\.\d+", api) or \
            api != root_api or api != locked_api or len(rust) != 1 or \
            not isinstance(rust[0], str) or not re.fullmatch(r"\d+\.\d+\.\d+", rust[0]):
        raise ReleasePreparationError("tauri_dependency_metadata_invalid")
    if api.split(".")[:2] != rust[0].split(".")[:2]:
        raise ReleasePreparationError("tauri_api_minor_mismatch")
    return version


def _inspect_pe(path: Path) -> bool:
    """Validate an AMD64 PE32+ image and report a present certificate table."""
    _regular_file(path, "release_resource_invalid")
    size = path.stat().st_size
    try:
        with path.open("rb") as stream:
            if size < 64:
                raise ReleasePreparationError("invalid_pe")
            dos = stream.read(64)
            if dos[:2] != b"MZ":
                raise ReleasePreparationError("invalid_pe")
            pe_offset = struct.unpack_from("<I", dos, 0x3C)[0]
            if pe_offset < 64 or pe_offset + 24 > size:
                raise ReleasePreparationError("invalid_pe")
            stream.seek(pe_offset)
            signature = stream.read(4)
            coff = stream.read(20)
            if signature != b"PE\0\0" or len(coff) != 20:
                raise ReleasePreparationError("invalid_pe")
            machine, section_count = struct.unpack_from("<HH", coff, 0)
            optional_size = struct.unpack_from("<H", coff, 16)[0]
            if machine != AMD64_MACHINE:
                raise ReleasePreparationError("wrong_architecture")
            optional_offset = pe_offset + 24
            if (section_count == 0 or optional_size < 0xF0
                    or optional_offset + optional_size + section_count * 40 > size):
                raise ReleasePreparationError("invalid_pe")
            stream.seek(optional_offset)
            optional = stream.read(optional_size)
            if len(optional) != optional_size:
                raise ReleasePreparationError("invalid_pe")
            if struct.unpack_from("<H", optional, 0)[0] != PE32_PLUS_MAGIC:
                raise ReleasePreparationError("invalid_pe")
            directory_count = struct.unpack_from("<I", optional, 108)[0]
            if directory_count > (optional_size - 112) // 8:
                raise ReleasePreparationError("invalid_pe")
            if directory_count <= 4:
                return False
            certificate_offset, certificate_size = struct.unpack_from("<II", optional, 112 + 4 * 8)
            if bool(certificate_offset) != bool(certificate_size):
                raise ReleasePreparationError("invalid_pe")
            if certificate_size and (
                    certificate_size < 8 or certificate_offset % 8 != 0
                    or certificate_offset + certificate_size > size):
                raise ReleasePreparationError("invalid_pe")
            return certificate_size > 0
    except OSError:
        raise ReleasePreparationError("release_resource_unreadable") from None


def _output_preflight(output: Path, release: Path) -> None:
    if output.is_symlink():
        raise ReleasePreparationError("output_is_symlink")
    resolved_output = output.resolve(strict=False)
    resolved_release = release.resolve(strict=True)
    if resolved_output == resolved_release:
        raise ReleasePreparationError("output_is_release_directory")
    try:
        resolved_output.relative_to(resolved_release)
    except ValueError:
        pass
    else:
        raise ReleasePreparationError("output_inside_release_directory")

    if not output.exists():
        return
    if not output.is_dir():
        raise ReleasePreparationError("output_not_directory")
    try:
        next(output.iterdir())
    except StopIteration:
        return
    except OSError:
        raise ReleasePreparationError("output_unreadable") from None
    raise ReleasePreparationError("output_not_empty")


def _copy_and_hash(source: Path, destination: Path) -> dict[str, Any]:
    _regular_file(source, "release_resource_invalid")
    digest = hashlib.sha256()
    byte_size = 0
    try:
        with source.open("rb") as src, destination.open("xb") as dst:
            while True:
                block = src.read(1024 * 1024)
                if not block:
                    break
                dst.write(block)
                digest.update(block)
                byte_size += len(block)
            dst.flush()
            os.fsync(dst.fileno())
    except OSError:
        raise ReleasePreparationError("resource_copy_failed") from None
    return {"sha256": digest.hexdigest(), "size_bytes": byte_size}


def prepare(repo: Path, target_dir: Path, output: Path) -> dict[str, Any]:
    """Stage exact release resources; raise ReleasePreparationError on failure."""
    repo = Path(repo).expanduser().resolve()
    target_dir = Path(target_dir).expanduser().resolve()
    output = Path(output).expanduser().absolute()
    release = _release_directory(target_dir)
    _output_preflight(output, release)
    version = _read_versions(repo)

    sources: list[tuple[Path, Path, bool]] = []
    signatures: list[bool] = []
    for name, relative in EXECUTABLES:
        source = release / name
        if not source.exists():
            raise ReleasePreparationError("release_binary_missing")
        signed = _inspect_pe(source)
        signatures.append(signed)
        sources.append((source, relative, True))
    for source_relative, destination_relative in LICENSES:
        source = repo / source_relative
        _regular_file(source, "license_resource_missing")
        sources.append((source, destination_relative, False))

    try:
        output.parent.mkdir(parents=True, exist_ok=True)
    except OSError:
        raise ReleasePreparationError("output_parent_unavailable") from None

    try:
        stage = Path(tempfile.mkdtemp(prefix=f".{output.name}.staging-", dir=output.parent))
    except OSError:
        raise ReleasePreparationError("staging_directory_unavailable") from None
    try:
        resource_metadata: list[dict[str, Any]] = []
        for source, relative, is_executable in sources:
            destination = stage / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            metadata = _copy_and_hash(source, destination)
            resource_metadata.append({
                "relative_path": relative.as_posix(),
                **metadata,
            })
            if is_executable:
                _inspect_pe(destination)

        manifest: dict[str, Any] = {
            "schema": SCHEMA,
            "version": version,
            "arch": "AMD64",
            "profile": "release",
            "production_features": dict(PRODUCTION_FEATURES),
            "unsigned": not any(signatures),
            "resources": resource_metadata,
        }
        manifest_path = stage / "release-resources.json"
        manifest_path.write_text(
            json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )

        # Recheck immediately before publication so concurrent output changes fail closed.
        _output_preflight(output, release)
        removed_empty_output = False
        if output.exists():
            try:
                output.rmdir()
                removed_empty_output = True
            except OSError:
                raise ReleasePreparationError("output_changed_during_staging") from None
        try:
            os.replace(stage, output)
        except OSError:
            if removed_empty_output and not output.exists():
                try:
                    output.mkdir()
                except OSError:
                    pass
            raise ReleasePreparationError("output_publish_failed") from None
        return manifest
    except ReleasePreparationError:
        raise
    except OSError:
        raise ReleasePreparationError("staging_failed") from None
    finally:
        if stage.exists():
            shutil.rmtree(stage, ignore_errors=True)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, required=True)
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--check-inputs", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.check_inputs:
            version = check_build_inputs(args.repo_root)
            print(json.dumps({"status": "passed", "version": version, "build_inputs": "valid"}, sort_keys=True))
            return 0
        if args.target_dir is None or args.output is None:
            parser.error("staging requires --target-dir and --output")
        check_build_inputs(args.repo_root)
        manifest = prepare(args.repo_root, args.target_dir, args.output)
    except ReleasePreparationError as error:
        print(json.dumps({"status": "failed", "error": error.code}, sort_keys=True))
        return 1
    print(json.dumps({
        "status": "passed",
        "version": manifest["version"],
        "resource_count": len(manifest["resources"]),
        "unsigned": manifest["unsigned"],
    }, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
