#!/usr/bin/env python3
"""Regression tests for Tauri updater signature output parsing."""
from __future__ import annotations

import base64
import io
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))

import tauri_updater_signature as signature


class TauriUpdaterSignatureTests(unittest.TestCase):
    def setUp(self) -> None:
        self.raw_signature = (
            "untrusted comment: signature from minisign secret key\n"
            "RUTestSignature==\n"
            "trusted comment: timestamp:1792080000\tfile:fixture-setup.exe\tversion:0.7.2\n"
            "TestGlobalSignature==\n"
        )
        self.encoded = base64.b64encode(self.raw_signature.encode()).decode("ascii")
        self.output = f"Your file was signed successfully\nPublic signature:\n{self.encoded}\n"

    def test_extracts_exact_base64_signature_and_version(self) -> None:
        self.assertEqual(signature.extract_signature(self.output, "0.7.2"), self.encoded)

    def test_rejects_wrong_version(self) -> None:
        with self.assertRaisesRegex(signature.SignatureOutputError, "signature_version_mismatch"):
            signature.extract_signature(self.output, "0.7.3")

    def test_rejects_missing_or_duplicate_public_signature(self) -> None:
        for output in (
            "Tauri signer output without a signature",
            self.output + "Public signature:\n" + self.encoded,
        ):
            with self.subTest(output=output[:40]), self.assertRaises(signature.SignatureOutputError):
                signature.extract_signature(output, "0.7.2")

    def test_rejects_invalid_base64_or_minisign_text(self) -> None:
        for encoded in ("%%%", base64.b64encode(b"not a minisign signature").decode("ascii")):
            output = f"Public signature:\n{encoded}\n"
            with self.subTest(encoded=encoded), self.assertRaises(signature.SignatureOutputError):
                signature.extract_signature(output, "0.7.2")

    def test_cli_writes_only_the_signature_value_without_newline(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "release.sig"
            with patch("sys.stdin", io.StringIO(self.output)):
                result = signature.main(["--version", "0.7.2", "--output", str(path)])
            self.assertEqual(result, 0)
            self.assertEqual(path.read_bytes(), self.encoded.encode("ascii"))


if __name__ == "__main__":
    unittest.main()
