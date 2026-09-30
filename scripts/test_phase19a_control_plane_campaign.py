import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import phase19a_control_plane_campaign as c


class Phase19AControlPlaneCampaignTests(unittest.TestCase):
    def test_server_url_must_be_loopback(self):
        self.assertEqual(
            c.require_loopback_server_url("http://127.0.0.1:1234"),
            "http://127.0.0.1:1234",
        )
        with self.assertRaisesRegex(RuntimeError, "loopback"):
            c.require_loopback_server_url("https://example.com")

    def test_workload_argv_preserves_payload_size_while_scaling_duration(self):
        args, expected = c.workload_argv(
            {
                "workload": "bursty",
                "duration_seconds": 60,
                "output_bytes": 500000,
            },
            0.01,
        )
        self.assertEqual(expected, 0)
        self.assertIn("0.600000", args)
        self.assertIn("500000", args)

    def test_expected_failure_is_retained(self):
        args, expected = c.workload_argv(
            {
                "workload": "late-failure",
                "duration_seconds": 180,
                "expected_exit_code": 23,
            },
            0.01,
        )
        self.assertEqual(expected, 23)
        self.assertIn("1.800000", args)

    def test_resolve_project_id_matches_registered_path(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            registry = root / "local-connections/a/http_local/user/project-registry/chadex.toml"
            registry.parent.mkdir(parents=True)
            project = root / "repo"
            project.mkdir()
            registry.write_text(
                f'id = "chadex-abc"\npath = "{project}"\n', encoding="utf-8"
            )
            self.assertEqual(
                c.resolve_project_id(root, project, "device-x"),
                "agent:device-x:chadex-abc",
            )

    def test_fast_compact_success_is_normalized(self):
        class NoCallClient:
            def invoke(self, *_args, **_kwargs):
                raise AssertionError("compact success must not poll")

        terminal, polls = c.wait_for_terminal(
            NoCallClient(),
            {"stdout_tail": "ok\n", "stdout_lines": 1},
            999999999.0,
            0,
        )
        self.assertEqual(polls, 0)
        self.assertEqual(terminal["exit_code"], 0)
        self.assertTrue(terminal["terminal"])

    def test_heartbeat_summary_counts_transitions(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "heartbeat.jsonl"
            rows = [
                {"ok": True, "connection_layers": {"server": {"status": "connected"}}},
                {"ok": True, "connection_layers": {"server": {"status": "offline"}}},
                {"ok": True, "connection_layers": {"server": {"status": "connected"}}},
            ]
            path.write_text("\n".join(json.dumps(row) for row in rows) + "\n")
            result = c.heartbeat_summary(path)
            self.assertEqual(result["samples"], 3)
            self.assertEqual(result["disconnects"], 1)
            self.assertEqual(result["reconnects"], 1)
            self.assertEqual(result["layer_transitions"]["server"], 2)

    def test_trace_breakdown_uses_trace_directory(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            trace = root / "trace-a"
            trace.mkdir()
            events = [
                {"event": "mcp_tool_request_received", "wall_unix_ns": 1_000_000_000},
                {"event": "mcp_tool_dispatch_started", "wall_unix_ns": 1_010_000_000},
                {"event": "mcp_tool_dispatch_finished", "wall_unix_ns": 1_040_000_000},
                {"event": "mcp_tool_response_serialized", "wall_unix_ns": 1_045_000_000},
                {"event": "mcp_tool_handler_returned", "wall_unix_ns": 1_050_000_000},
            ]
            (trace / "events.jsonl").write_text(
                "\n".join(json.dumps(row) for row in events) + "\n"
            )
            result = c.trace_breakdown(root, "trace-a")
            self.assertEqual(result["request_to_handler_ms"], 50.0)
            self.assertEqual(result["dispatch_ms"], 30.0)
            self.assertEqual(result["serialize_to_return_ms"], 5.0)


if __name__ == "__main__":
    unittest.main()
