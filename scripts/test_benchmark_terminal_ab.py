#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("benchmark_terminal_ab.py")
SPEC = importlib.util.spec_from_file_location("benchmark_terminal_ab", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
bench = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = bench
SPEC.loader.exec_module(bench)


class TerminalABBenchmarkTests(unittest.TestCase):
    def test_run_metadata_records_fairness_inputs(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            run_dir = Path(raw_tmp) / "20260922-120000-short"
            args = type(
                "Args",
                (),
                {
                    "model_label": "same-as-B",
                    "effort_label": "extra-high",
                    "webcodex_tunnel": "cloudflare",
                    "webcodex_auth": "query-token",
                    "cgw_model": "chatgpt-web/extra-high",
                },
            )()
            spec = bench.TaskSpec(
                name="short",
                source=Path("/unused"),
                baseline="abc123",
                prompt="same public prompt",
            )

            metadata = bench.run_metadata(run_dir, spec, args, "BA")

            self.assertEqual(metadata["run_id"], run_dir.name)
            self.assertEqual(metadata["baseline_sha"], "abc123")
            self.assertEqual(
                metadata["prompt_sha256"],
                bench.hashlib.sha256(spec.prompt.encode()).hexdigest(),
            )
            self.assertEqual(metadata["provider_order"], "BA")
            self.assertIn("independent fresh Git clones", metadata["fixture_policy"])
            self.assertIn("only after", metadata["hidden_grading_policy"])
            self.assertEqual(
                metadata["providers"]["B_codex_chatgpt_web_full"]["effort_inferred_from_model_route"],
                "extra-high",
            )
            self.assertEqual(metadata["providers"]["A_webcodex"]["tunnel"], "cloudflare")
            self.assertEqual(metadata["providers"]["A_webcodex"]["auth"], "query-token")

    def test_parse_codex_jsonl_prefers_completed_items_without_double_counting(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            tmp = Path(raw_tmp)
            fixture = tmp / "fixture"
            fixture.mkdir()
            (fixture / "foo.py").write_text("print('ok')\n")
            log = tmp / "codex.jsonl"
            events = [
                {
                    "type": "exec_command_begin",
                    "command": "sed -n '1,80p' foo.py",
                },
                {
                    "type": "item.completed",
                    "item": {
                        "type": "command_execution",
                        "command": "sed -n '1,80p' foo.py",
                    },
                },
                {
                    "type": "item.completed",
                    "item": {
                        "type": "command_execution",
                        "command": "python3 -m unittest discover -s tests -v",
                    },
                },
                {
                    "type": "item.completed",
                    "item": {
                        "type": "mcp_tool_call",
                        "tool": "read_files",
                        "arguments": {"paths": ["foo.py"]},
                    },
                },
                {
                    "type": "turn.completed",
                    "usage": {"input_tokens": 120, "output_tokens": 30},
                },
            ]
            log.write_text("\n".join(json.dumps(event) for event in events) + "\n")

            parsed = bench.parse_codex_jsonl(log, fixture)

            self.assertEqual(parsed["tool_call_schema"], "item.completed")
            self.assertEqual(parsed["tool_calls"], 3)
            self.assertEqual(parsed["shell_calls"], 2)
            self.assertEqual(parsed["build_test_calls"], 1)
            self.assertEqual(parsed["files_read"]["value"], 1)
            self.assertEqual(parsed["files_read"]["paths"], ["foo.py"])
            self.assertEqual(
                parsed["token_context_usage"],
                {"input_tokens": 120, "output_tokens": 30},
            )
            self.assertTrue(parsed["turn_completed"])

    def test_decode_first_json_accepts_pretty_json_with_trailing_text(self) -> None:
        text = '\n  {\n    "ok": true,\n    "url": "http://127.0.0.1:1234"\n  }\ntrailing log\n'
        self.assertEqual(
            bench.decode_first_json(text),
            {"ok": True, "url": "http://127.0.0.1:1234"},
        )

    def test_long_materialization_creates_independent_clean_git_repo(self) -> None:
        with tempfile.TemporaryDirectory() as raw_tmp:
            run_dir = Path(raw_tmp) / "run"
            run_dir.mkdir()
            original = bench.task_spec("long")

            materialized = bench.materialize_spec(original, run_dir)
            canonical = run_dir / "canonical-source"

            self.assertEqual(materialized.source, canonical)
            self.assertTrue((canonical / ".git").is_dir())
            self.assertEqual(
                bench.git(canonical, "status", "--porcelain=v1").stdout.strip(),
                "",
            )
            self.assertEqual(
                bench.git(canonical, "rev-parse", "HEAD").stdout.strip(),
                materialized.baseline,
            )
            self.assertFalse((canonical / "benchmarks" / "phase10-quality").exists())

            fixture = run_dir / "fixture"
            bench.prepare_fixture(materialized, fixture)
            self.assertEqual(
                bench.git(fixture, "rev-parse", "HEAD").stdout.strip(),
                materialized.baseline,
            )
            self.assertEqual(
                bench.git(fixture, "status", "--porcelain=v1").stdout.strip(),
                "",
            )


if __name__ == "__main__":
    unittest.main()
