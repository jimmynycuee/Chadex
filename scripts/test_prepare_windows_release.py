#!/usr/bin/env python3
"""Regression tests for Windows release resource staging."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))

import prepare_windows_release as release


EXECUTABLE_NAMES = (
    "chadex-helper.exe",
    "chadex-runtime-cli.exe",
    "chadex-runtime-server.exe",
    "chadex-runtime-runner.exe",
)


def synthetic_pe(machine: int = 0x8664, *, valid: bool = True, signed: bool = False) -> bytes:
    pe_offset = 0x80
    optional_size = 0xF0
    section_offset = pe_offset + 24 + optional_size
    image = bytearray(max(section_offset + 40, 0x201))
    image[:2] = b"MZ"
    struct.pack_into("<I", image, 0x3C, pe_offset)
    image[pe_offset:pe_offset + 4] = b"PE\0\0" if valid else b"NOPE"
    struct.pack_into("<HHIIIHH", image, pe_offset + 4,
                     machine, 1, 0, 0, 0, optional_size, 0x0022)
    optional = pe_offset + 24
    struct.pack_into("<H", image, optional, 0x20B)
    struct.pack_into("<I", image, optional + 108, 16)
    if signed:
        certificate_offset = (len(image) + 7) & ~7
        struct.pack_into("<II", image, optional + 112 + 4 * 8, certificate_offset, 8)
        image.extend(b"\0" * (certificate_offset - len(image)))
        image.extend(struct.pack("<IHH", 8, 0x0200, 0x0002))
    image[section_offset:section_offset + 8] = b".text\0\0\0"
    struct.pack_into("<IIIIIIHHI", image, section_offset + 8,
                     1, 0x1000, 1, 0x200, 0, 0, 0, 0, 0x60000020)
    image[0x200] = ord("X")
    return bytes(image)


class PrepareWindowsReleaseTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.target = self.root / "target"
        self.release_dir = self.target / "release"
        self.output = self.root / "staged"
        (self.repo / "apps/windows/src-tauri").mkdir(parents=True)
        (self.repo / "apps/windows").mkdir(exist_ok=True)
        (self.repo / "apps/windows/src-tauri/tauri.conf.json").write_text(
            json.dumps({"version": "0.4.0"}), encoding="utf-8")
        (self.repo / "apps/windows/package.json").write_text(
            json.dumps({"version": "0.4.0", "dependencies": {"@tauri-apps/api": "2.12.1"}}), encoding="utf-8")
        (self.repo / "apps/windows/package-lock.json").write_text(json.dumps({"packages": {
            "": {"dependencies": {"@tauri-apps/api": "2.12.1"}},
            "node_modules/@tauri-apps/api": {"version": "2.12.1"},
        }}), encoding="utf-8")
        (self.repo / "apps/windows/src-tauri/Cargo.lock").write_text(
            'version = 4\n[[package]]\nname = "tauri"\nversion = "2.12.1"\n', encoding="utf-8")
        (self.repo / "apps/windows/src-tauri/Cargo.toml").write_text(
            '[package]\nname = "chadex-windows"\nversion = "0.4.0"\n', encoding="utf-8")
        (self.repo / "attribution").mkdir()
        (self.repo / "LICENSE").write_bytes(b"Chadex license fixture\n")
        (self.repo / "attribution/WebCodex-LICENSE.txt").write_bytes(
            b"WebCodex license fixture\n")
        (self.repo / "UPSTREAM.md").write_bytes(b"Upstream attribution fixture\n")
        self.release_dir.mkdir(parents=True)
        for name in EXECUTABLE_NAMES:
            (self.release_dir / name).write_bytes(synthetic_pe())

    def tearDown(self) -> None:
        self.temp.cleanup()

    def prepare(self) -> dict[str, object]:
        return release.prepare(self.repo, self.target, self.output)

    def test_build_inputs_reject_api_rust_minor_mismatch(self) -> None:
        package_path = self.repo / "apps/windows/package.json"
        package = json.loads(package_path.read_text())
        package["dependencies"]["@tauri-apps/api"] = "2.8.0"
        package_path.write_text(json.dumps(package))
        lock_path = self.repo / "apps/windows/package-lock.json"
        lock = json.loads(lock_path.read_text())
        lock["packages"][""]["dependencies"]["@tauri-apps/api"] = "2.8.0"
        lock["packages"]["node_modules/@tauri-apps/api"]["version"] = "2.8.0"
        lock_path.write_text(json.dumps(lock))
        with self.assertRaises(release.ReleasePreparationError) as failure:
            release.check_build_inputs(self.repo)
        self.assertEqual(failure.exception.code, "tauri_api_minor_mismatch")
        self.assertFalse(self.output.exists())

    def test_build_inputs_reject_npm_lock_disagreement(self) -> None:
        lock_path = self.repo / "apps/windows/package-lock.json"
        lock = json.loads(lock_path.read_text())
        lock["packages"]["node_modules/@tauri-apps/api"]["version"] = "2.8.0"
        lock_path.write_text(json.dumps(lock))
        with self.assertRaises(release.ReleasePreparationError) as failure:
            release.check_build_inputs(self.repo)
        self.assertEqual(failure.exception.code, "tauri_dependency_metadata_invalid")

    def test_input_preflight_needs_no_compiled_resources(self) -> None:
        import shutil
        shutil.rmtree(self.release_dir)
        self.assertEqual(release.main(["--repo-root", str(self.repo), "--check-inputs"]), 0)
        self.assertFalse(self.output.exists())

    def test_uses_only_release_executables_and_records_hashes_and_licenses(self) -> None:
        debug_dir = self.target / "debug"
        debug_dir.mkdir()
        for name in EXECUTABLE_NAMES:
            (debug_dir / name).write_bytes(b"debug fallback must not be selected")

        manifest = self.prepare()

        self.assertEqual(manifest["schema"], release.SCHEMA)
        self.assertEqual(manifest["arch"], "AMD64")
        self.assertEqual(manifest["profile"], "release")
        self.assertEqual(manifest["production_features"], {
            "custom-protocol": True,
            "desktop-smoke": False,
        })
        self.assertIs(manifest["unsigned"], True)
        self.assertEqual(manifest["version"], "0.4.0")
        self.assertIsInstance(manifest["resources"], list)
        resources = {item["relative_path"]: item for item in manifest["resources"]}
        expected_sources = {
            "helper/chadex-helper.exe": self.release_dir / "chadex-helper.exe",
            **{
                f"chadex-runtime/{name}": self.release_dir / name
                for name in EXECUTABLE_NAMES[1:]
            },
            "licenses/Chadex-LICENSE.txt": self.repo / "LICENSE",
            "licenses/WebCodex-LICENSE.txt": self.repo / "attribution/WebCodex-LICENSE.txt",
            "licenses/UPSTREAM.md": self.repo / "UPSTREAM.md",
        }
        self.assertEqual(set(resources), set(expected_sources))
        for relative, source in expected_sources.items():
            staged = self.output / relative
            source_bytes = source.read_bytes()
            self.assertEqual(staged.read_bytes(), source_bytes)
            self.assertEqual(resources[relative]["sha256"], hashlib.sha256(source_bytes).hexdigest())
            self.assertEqual(resources[relative]["size_bytes"], len(source_bytes))
            self.assertIsInstance(resources[relative]["relative_path"], str)
            self.assertIsInstance(resources[relative]["size_bytes"], int)
            self.assertRegex(resources[relative]["sha256"], r"^[0-9a-f]{64}$")
        saved_manifest = json.loads((self.output / "release-resources.json").read_text())
        self.assertEqual(saved_manifest, manifest)
        serialized = json.dumps(manifest)
        self.assertNotIn(str(self.repo), serialized)
        self.assertNotIn(str(self.target), serialized)

    def test_rejects_non_amd64_release_pe_without_publishing_output(self) -> None:
        (self.release_dir / EXECUTABLE_NAMES[0]).write_bytes(synthetic_pe(0x14C))

        with self.assertRaises(release.ReleasePreparationError) as failure:
            self.prepare()

        self.assertEqual(failure.exception.code, "wrong_architecture")
        self.assertFalse(self.output.exists())

    def test_rejects_malformed_pe(self) -> None:
        (self.release_dir / EXECUTABLE_NAMES[0]).write_bytes(synthetic_pe(valid=False))

        with self.assertRaises(release.ReleasePreparationError) as failure:
            self.prepare()

        self.assertEqual(failure.exception.code, "invalid_pe")
        self.assertFalse(self.output.exists())

    def test_missing_release_binary_does_not_fall_back_to_debug(self) -> None:
        debug_dir = self.target / "debug"
        debug_dir.mkdir()
        (debug_dir / EXECUTABLE_NAMES[0]).write_bytes(synthetic_pe())
        (self.release_dir / EXECUTABLE_NAMES[0]).unlink()

        with self.assertRaises(release.ReleasePreparationError) as failure:
            self.prepare()

        self.assertEqual(failure.exception.code, "release_binary_missing")
        self.assertFalse(self.output.exists())

    def test_rejects_output_symlink(self) -> None:
        real_output = self.root / "real-output"
        real_output.mkdir()
        try:
            self.output.symlink_to(real_output, target_is_directory=True)
        except (OSError, NotImplementedError):
            self.skipTest("symlink creation is unavailable")

        with self.assertRaises(release.ReleasePreparationError) as failure:
            self.prepare()

        self.assertEqual(failure.exception.code, "output_is_symlink")
        self.assertEqual(list(real_output.iterdir()), [])

    def test_rejects_symlinked_release_executable(self) -> None:
        linked = self.release_dir / EXECUTABLE_NAMES[0]
        linked.unlink()
        real_binary = self.root / "real-helper.exe"
        real_binary.write_bytes(synthetic_pe())
        try:
            linked.symlink_to(real_binary)
        except (OSError, NotImplementedError):
            self.skipTest("symlink creation is unavailable")

        with self.assertRaises(release.ReleasePreparationError) as failure:
            self.prepare()

        self.assertEqual(failure.exception.code, "release_resource_invalid")
        self.assertFalse(self.output.exists())

    def test_rejects_output_equal_to_release_directory(self) -> None:
        with self.assertRaises(release.ReleasePreparationError) as failure:
            release.prepare(self.repo, self.target, self.release_dir)

        self.assertEqual(failure.exception.code, "output_is_release_directory")
        self.assertEqual(
            set(path.name for path in self.release_dir.iterdir()), set(EXECUTABLE_NAMES))

    def test_manifest_reports_a_present_authenticode_certificate_table(self) -> None:
        (self.release_dir / EXECUTABLE_NAMES[0]).write_bytes(synthetic_pe(signed=True))

        manifest = self.prepare()

        self.assertFalse(manifest["unsigned"])

    def test_rejects_nonempty_output_and_preserves_existing_data(self) -> None:
        self.output.mkdir()
        sentinel = self.output / "keep.txt"
        sentinel.write_text("existing output", encoding="utf-8")

        with self.assertRaises(release.ReleasePreparationError) as failure:
            self.prepare()

        self.assertEqual(failure.exception.code, "output_not_empty")
        self.assertEqual(sentinel.read_text(encoding="utf-8"), "existing output")

    def test_rejects_mismatched_versions(self) -> None:
        package_path = self.repo / "apps/windows/package.json"
        package_path.write_text(json.dumps({"version": "0.3.2"}), encoding="utf-8")

        with self.assertRaises(release.ReleasePreparationError) as failure:
            self.prepare()

        self.assertEqual(failure.exception.code, "version_mismatch")
        self.assertFalse(self.output.exists())

    def test_cli_accepts_the_three_explicit_paths(self) -> None:
        status = release.main([
            "--repo-root", str(self.repo),
            "--target-dir", str(self.target),
            "--output", str(self.output),
        ])

        self.assertEqual(status, 0)
        self.assertTrue((self.output / "release-resources.json").is_file())


if __name__ == "__main__":
    unittest.main()
