#!/usr/bin/env python3
"""Focused safety tests for the Windows Tauri updater manifest generator."""
from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import tomllib
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))

import windows_updater_manifest as updater


class WindowsUpdaterManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.archive = self.root / "Chadex-0.7.2-windows-x64-setup.nsis.zip"
        self.signature_file = self.root / "Chadex-0.7.2-windows-x64-setup.nsis.zip.sig"
        self.archive.write_bytes(b"signed archive fixture")
        self.signature_contents = b"untrusted comment: fixture\r\nRUTestSignature==\r\n"
        self.signature_file.write_bytes(self.signature_contents)
        self.output = self.root / "latest.json"

    def tearDown(self) -> None:
        self.temp.cleanup()

    def generate(self, **overrides: object) -> dict[str, object]:
        values: dict[str, object] = {
            "version": "0.7.2",
            "tag": "v0.7.2",
            "repository": "jimmynycuee/Chadex",
            "archive": self.archive,
            "signature_file": self.signature_file,
            "output": self.output,
            "pub_date": "2026-10-15T00:00:00Z",
        }
        values.update(overrides)
        return updater.generate_manifest(**values)  # type: ignore[arg-type]

    def test_manifest_uses_exact_release_asset_url_and_signature_contents(self) -> None:
        manifest = self.generate()
        self.assertEqual(manifest["version"], "0.7.2")
        self.assertEqual(manifest["pub_date"], "2026-10-15T00:00:00Z")
        platform = manifest["platforms"]["windows-x86_64"]  # type: ignore[index]
        self.assertEqual(
            platform["url"],
            "https://github.com/jimmynycuee/Chadex/releases/download/"
            "v0.7.2/Chadex-0.7.2-windows-x64-setup.nsis.zip",
        )
        self.assertEqual(platform["signature"], self.signature_contents.decode("ascii"))
        self.assertEqual(json.loads(self.output.read_text(encoding="utf-8")), manifest)

    def test_standalone_cli_generates_manifest(self) -> None:
        result = updater.main([
            "--version", "0.7.2",
            "--tag", "v0.7.2",
            "--repository", "jimmynycuee/Chadex",
            "--archive", str(self.archive),
            "--signature-file", str(self.signature_file),
            "--output", str(self.output),
        ])
        self.assertEqual(result, 0)
        manifest = json.loads(self.output.read_text(encoding="utf-8"))
        self.assertEqual(manifest["version"], "0.7.2")
        self.assertEqual(manifest["platforms"]["windows-x86_64"]["signature"], self.signature_contents.decode("ascii"))

    def test_safe_prerelease_tag_can_reference_its_release_asset(self) -> None:
        manifest = self.generate(tag="v0.7.2-rc.1")
        platform = manifest["platforms"]["windows-x86_64"]  # type: ignore[index]
        self.assertIn("/download/v0.7.2-rc.1/", platform["url"])

    def test_rejects_unsafe_or_mismatched_versions_and_tags(self) -> None:
        for version, tag in (
            ("0.07.2", "v0.07.2"),
            ("0.7.2/evil", "v0.7.2/evil"),
            ("0.7.2", "v0.7.3"),
            ("0.7.2", "v0.7.2/../../other"),
            ("0.7.2", "v0.7.2-"),
        ):
            with self.subTest(version=version, tag=tag), self.assertRaises(updater.ManifestError):
                self.generate(version=version, tag=tag)
        self.assertFalse(self.output.exists())

    def test_rejects_repository_url_injection(self) -> None:
        for repository in (
            "owner/repo?download=1",
            "owner/../repo",
            "https://github.com/owner/repo",
            "owner/repo#fragment",
        ):
            with self.subTest(repository=repository), self.assertRaises(updater.ManifestError):
                self.generate(repository=repository)
        self.assertFalse(self.output.exists())

    def test_rejects_missing_or_non_text_artifacts(self) -> None:
        with self.assertRaisesRegex(updater.ManifestError, "archive is missing"):
            self.generate(archive=self.root / "missing" / self.archive.name)
        self.signature_file.write_bytes(b"\xff")
        with self.assertRaisesRegex(updater.ManifestError, "readable ASCII"):
            self.generate()
        self.assertFalse(self.output.exists())

    def test_rejects_archive_filename_that_cannot_match_generated_release_url(self) -> None:
        wrong_name = self.root / "candidate.nsis.zip"
        wrong_name.write_bytes(b"archive")
        with self.assertRaisesRegex(updater.ManifestError, "filename does not match"):
            self.generate(archive=wrong_name)
        self.assertFalse(self.output.exists())

    def test_rejects_timestamp_without_timezone(self) -> None:
        with self.assertRaisesRegex(updater.ManifestError, "include a timezone"):
            self.generate(pub_date="2026-10-15T00:00:00")
        self.assertFalse(self.output.exists())

    def test_rejects_empty_oversized_or_unsafe_signature(self) -> None:
        for signature in ("", " \n", "signature\x00injected", "x" * (16 * 1024 + 1)):
            with self.subTest(signature=signature[:20]), self.assertRaises(updater.ManifestError):
                updater.build_manifest(
                    version="0.7.2", tag="v0.7.2", repository="owner/repo",
                    signature=signature, pub_date="2026-10-15T00:00:00Z",
                )

    def test_release_build_binds_updater_signature_to_the_announced_version(self) -> None:
        root = Path(__file__).resolve().parent.parent
        tauri_config = json.loads((root / "apps/windows/src-tauri/tauri.conf.json").read_text(encoding="utf-8"))
        self.assertIs(tauri_config["plugins"]["updater"]["requireSignedVersion"], True)

        package = json.loads((root / "apps/windows/package.json").read_text(encoding="utf-8"))
        cli_version = package["devDependencies"]["@tauri-apps/cli"]
        self.assertGreaterEqual(tuple(map(int, cli_version.split("."))), (2, 11, 5))
        package_lock = json.loads((root / "apps/windows/package-lock.json").read_text(encoding="utf-8"))
        self.assertEqual(package_lock["packages"][""]["devDependencies"]["@tauri-apps/cli"], cli_version)

        cargo = tomllib.loads((root / "apps/windows/src-tauri/Cargo.toml").read_text(encoding="utf-8"))
        updater_version = cargo["dependencies"]["tauri-plugin-updater"]["version"]
        self.assertGreaterEqual(tuple(map(int, updater_version.split("."))), (2, 12, 0))
        cargo_lock = tomllib.loads((root / "apps/windows/src-tauri/Cargo.lock").read_text(encoding="utf-8"))
        locked_updater = [entry["version"] for entry in cargo_lock["package"] if entry["name"] == "tauri-plugin-updater"]
        self.assertEqual(locked_updater, [updater_version])

        release_config = json.loads((root / "apps/windows/src-tauri/tauri.release.conf.json").read_text(encoding="utf-8"))
        self.assertIs(release_config["bundle"]["createUpdaterArtifacts"], False)
        release_script = (root / "scripts/build_windows_release.ps1").read_text(encoding="utf-8")
        self.assertIn('"--features", "custom-protocol"', release_script)
        self.assertIn("signer sign $sourceUpdaterArchive --app-version $version", release_script)
        self.assertIn("Updater signature does not attest release version", release_script)
        desktop_hash_capture = release_script.index(
            '$desktopHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $desktop).Hash.ToLowerInvariant()'
        )
        fixture_start = release_script.index(
            "if ($BuildUpgradeFixture) {",
            release_script.index("$upgradeBaselineVersion = $null"),
        )
        self.assertLess(desktop_hash_capture, fixture_start)
        self.assertIn("desktop_sha256 = $desktopHash", release_script)
        self.assertNotIn("desktop_sha256 = (Get-FileHash", release_script)

        source_check = (root / "scripts/windows_release_source_check.ps1").read_text(encoding="utf-8")
        self.assertIn("build --ci --features custom-protocol", source_check)


if __name__ == "__main__":
    unittest.main()
