#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).with_name("benchmark_chatgpt_completion.py")
SPEC = importlib.util.spec_from_file_location("benchmark_chatgpt_completion", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
bench = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = bench
SPEC.loader.exec_module(bench)


class ChatGPTCompletionBenchmarkTests(unittest.TestCase):
    def test_summary_uses_matched_pairs_only(self) -> None:
        rows = [
            {"iteration": 0, "arm": "candidate", "passed": True, "submit_to_visible_final_ms": 7000},
            {"iteration": 0, "arm": "control", "passed": True, "submit_to_visible_final_ms": 9000},
            {"iteration": 1, "arm": "candidate", "passed": True, "submit_to_visible_final_ms": 8000},
            {"iteration": 1, "arm": "control", "passed": False, "submit_to_visible_final_ms": 10000},
        ]
        summary = bench.summarize(rows, ["candidate", "control"])
        self.assertEqual(summary["arms"]["candidate"]["valid_runs"], 2)
        self.assertEqual(summary["arms"]["control"]["valid_runs"], 1)
        self.assertEqual(summary["paired"]["candidate_minus_control"], {
            "valid_pairs": 1, "median_ms": -2000,
        })

    def test_config_rejects_shared_fixture(self) -> None:
        with self.assertRaisesRegex(ValueError, "distinct"):
            bench.validate_config({"arms": [
                {"name": "chadex-current", "connector": "A", "fixture": "/tmp/x/Benchmark-Chadex"},
                {"name": "chadex-candidate", "connector": "B", "fixture": "/tmp/x/Benchmark-Chadex"},
            ]})

    def test_acceptance_requires_three_arms_and_material_gain(self) -> None:
        arm = lambda median, p95: {"valid_runs": 20, "median_ms": median, "p95_ms": p95}
        result = bench.acceptance({"arms": {
            "chadex-current": arm(13000, 17000),
            "chadex-candidate": arm(10000, 16000),
            "webcodex": arm(11000, 15000),
        }})
        self.assertTrue(result["performance_target_met"])
        self.assertFalse(result["release_ready"])
        self.assertFalse(bench.acceptance({"arms": {"chadex-current": arm(13000, 17000)}})["evaluated"])

    def test_unknown_edit_is_preserved(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            fixture = Path(raw) / "Benchmark-Chadex"
            fixture.mkdir()
            subprocess.run(["git", "init", "-q"], cwd=fixture, check=True)
            subprocess.run(["git", "config", "user.name", "Benchmark"], cwd=fixture, check=True)
            subprocess.run(["git", "config", "user.email", "benchmark@example.invalid"], cwd=fixture, check=True)
            (fixture / "unrelated.txt").write_text("original\n")
            subprocess.run(["git", "add", "."], cwd=fixture, check=True)
            subprocess.run(["git", "commit", "-qm", "baseline"], cwd=fixture, check=True)
            head = bench.git(fixture, "rev-parse", "HEAD").strip()
            with patch.object(bench, "BASELINE", head):
                bench.assert_clean(fixture)
                (fixture / "unrelated.txt").write_text("user edit\n")
                with self.assertRaisesRegex(RuntimeError, "left untouched"):
                    bench.verify_and_reset(fixture)
                self.assertEqual((fixture / "unrelated.txt").read_text(), "user edit\n")


if __name__ == "__main__":
    unittest.main()
