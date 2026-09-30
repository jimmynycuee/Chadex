import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import phase19a_longhaul_profile as p


def write_trace(root, name, rows):
    directory = root / name
    directory.mkdir()
    (directory / "events.jsonl").write_text(
        "\n".join(json.dumps(row) for row in rows) + "\n", encoding="utf-8"
    )


class Phase19ALonghaulProfileTests(unittest.TestCase):
    def test_heartbeat_disconnect_reconnect(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "heartbeat.jsonl"
            rows = [
                {"connection_layers": {"tunnel": {"status": "connected"}}},
                {"connection_layers": {"tunnel": {"status": "disconnected"}}},
                {"ok": False, "error": "temporary server restart"},
                {"connection_layers": {"tunnel": {"status": "connected"}}},
            ]
            path.write_text("\n".join(json.dumps(x) for x in rows) + "\n")
            result = p.heartbeat_metrics(path)
            self.assertEqual(result["disconnects"], 1)
            self.assertEqual(result["reconnects"], 1)
            self.assertEqual(result["sample_errors"], 1)

    def test_profile_multi_tool_timeline(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            driver = root / "driver.json"
            driver.write_text(json.dumps({
                "submitted_at_ms": 1000,
                "confirmed_final_at_ms": 4000,
            }))
            traces = root / "traces"
            traces.mkdir()
            write_trace(traces, "a", [
                {"event":"mcp_tool_request_received","wall_unix_ns":1_100_000_000},
                {"event":"mcp_tool_request_parsed","server_trace_id":"a","client_window_key":"w","tool_name":"read_files","wall_unix_ns":1_101_000_000},
                {"event":"mcp_tool_dispatch_started","wall_unix_ns":1_102_000_000},
                {"event":"mcp_tool_dispatch_finished","wall_unix_ns":1_110_000_000},
                {"event":"tool_trace_payload_captured","payload_bytes":120,"compressed_bytes":40,"wall_unix_ns":1_111_000_000},
                {"event":"mcp_tool_handler_returned","estimated_json_bytes":100,"tool_success":True,"wall_unix_ns":1_112_000_000},
            ])
            write_trace(traces, "b", [
                {"event":"mcp_tool_request_received","wall_unix_ns":2_000_000_000},
                {"event":"mcp_tool_request_parsed","server_trace_id":"b","client_window_key":"w","tool_name":"run_shell","wall_unix_ns":2_001_000_000},
                {"event":"mcp_tool_dispatch_started","wall_unix_ns":2_002_000_000},
                {"event":"mcp_tool_dispatch_finished","wall_unix_ns":2_050_000_000},
                {"event":"tool_trace_payload_persisted","payload_bytes":250,"compressed_bytes":80,"wall_unix_ns":2_051_000_000},
                {"event":"mcp_tool_handler_returned","estimated_json_bytes":250,"tool_success":True,"wall_unix_ns":2_052_000_000},
            ])
            output = root / "out"
            result = p.profile(driver, traces, output, "test", client_window_key="w")
            metrics = result["metrics"]
            self.assertEqual(metrics["tool_calls"], 2)
            self.assertEqual(metrics["prompt_to_first_server_received_ms"], 100.0)
            self.assertEqual(metrics["model_facing_response_bytes"]["total"], 350)
            self.assertEqual(metrics["trace_payload_bytes"], 370)
            self.assertEqual(metrics["trace_payload_compressed_bytes"], 120)
            self.assertEqual(metrics["observation_to_next_server_action"]["p95_ms"], 888.0)
            self.assertEqual(metrics["total_completion_ms"], 3000)
            for name in ("timeline.json","metrics.json","events.jsonl","summary.md"):
                self.assertTrue((output / name).is_file())

    def test_marker_fallback_selects_unique_expected_first_tool(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            driver = root / "driver.json"
            driver.write_text(json.dumps({
                "submitted_at_ms": 1000,
                "confirmed_final_at_ms": 5000,
            }))
            traces = root / "traces"
            traces.mkdir()
            write_trace(traces, "bench-first", [{
                "event": "mcp_tool_request_parsed",
                "server_trace_id": "bench-first",
                "client_window_key": "bench",
                "tool_name": "work_on_project",
                "wall_unix_ns": 1_100_000_000,
            }])
            write_trace(traces, "bench-second", [{
                "event": "mcp_tool_request_parsed",
                "server_trace_id": "bench-second",
                "client_window_key": "bench",
                "tool_name": "run_process",
                "wall_unix_ns": 2_000_000_000,
            }])
            write_trace(traces, "other", [{
                "event": "mcp_tool_request_parsed",
                "server_trace_id": "other",
                "client_window_key": "other",
                "tool_name": "runtime_status",
                "wall_unix_ns": 1_200_000_000,
            }])
            with mock.patch.object(
                p.phase16a,
                "profile",
                side_effect=ValueError(
                    "expected exactly one exact-marker client window, got {}"
                ),
            ):
                key, mode = p.select_window(
                    driver,
                    traces,
                    1000,
                    5_000_000_000,
                    "marker",
                    None,
                    "work_on_project",
                    "instruction",
                )
            self.assertEqual(key, "bench")
            self.assertEqual(mode, "expected_first_tool_fallback")

    def test_marker_fallback_fails_closed_when_ambiguous(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            driver = root / "driver.json"
            driver.write_text(json.dumps({
                "submitted_at_ms": 1000,
                "confirmed_final_at_ms": 5000,
            }))
            traces = root / "traces"
            traces.mkdir()
            for index, window in enumerate(("a", "b"), 1):
                write_trace(traces, window, [{
                    "event": "mcp_tool_request_parsed",
                    "server_trace_id": window,
                    "client_window_key": window,
                    "tool_name": "work_on_project",
                    "wall_unix_ns": (1000 + index * 100) * 1_000_000,
                }])
            with mock.patch.object(
                p.phase16a,
                "profile",
                side_effect=ValueError(
                    "expected exactly one exact-marker client window, got {}"
                ),
            ):
                with self.assertRaisesRegex(
                    ValueError, "expected-first-tool fallback"
                ):
                    p.select_window(
                        driver,
                        traces,
                        1000,
                        5_000_000_000,
                        "marker",
                        None,
                        "work_on_project",
                        "instruction",
                    )

    def test_partial_profile_uses_driver_cutoff_and_excludes_later_traces(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            driver = root / "driver.json"
            driver.write_text(json.dumps({
                "submitted_at_ms": 1000,
                "driver_finished_at_ms": 3000,
                "completed": False,
                "driver_exit_code": 1,
                "error": "response timeout",
            }))
            traces = root / "traces"
            traces.mkdir()
            write_trace(traces, "inside", [
                {"event":"mcp_tool_request_received","wall_unix_ns":1_100_000_000},
                {"event":"mcp_tool_request_parsed","server_trace_id":"inside","client_window_key":"w","tool_name":"work_on_project","wall_unix_ns":1_101_000_000},
                {"event":"mcp_tool_handler_returned","estimated_json_bytes":100,"tool_success":True,"wall_unix_ns":1_120_000_000},
            ])
            write_trace(traces, "outside", [
                {"event":"mcp_tool_request_received","wall_unix_ns":4_000_000_000},
                {"event":"mcp_tool_request_parsed","server_trace_id":"outside","client_window_key":"w","tool_name":"runtime_status","wall_unix_ns":4_001_000_000},
                {"event":"mcp_tool_handler_returned","estimated_json_bytes":50,"tool_success":True,"wall_unix_ns":4_010_000_000},
            ])
            output = root / "out"
            result = p.profile(
                driver,
                traces,
                output,
                "partial",
                client_window_key="w",
            )
            metrics = result["metrics"]
            self.assertFalse(metrics["driver_completed"])
            self.assertEqual(metrics["completion_status"], "driver_failed")
            self.assertEqual(metrics["driver_error"], "response timeout")
            self.assertEqual(metrics["cutoff_source"], "driver_finished_at_ms")
            self.assertEqual(metrics["observed_window_ms"], 2000)
            self.assertIsNone(metrics["total_completion_ms"])
            self.assertEqual(metrics["tool_calls"], 1)
            events = json.loads((output / "timeline.json").read_text())["events"]
            self.assertEqual(events[-1]["event"], "driver_observation_ended")
            self.assertFalse(any(row["event"] == "final_response_completed" for row in events))

    def test_ambiguous_window_fails_closed(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            driver = root / "driver.json"
            driver.write_text(json.dumps({"submitted_at_ms":0,"confirmed_final_at_ms":5000}))
            traces = root / "traces"
            traces.mkdir()
            for index, window in enumerate(("a","b"),1):
                write_trace(traces, window, [{
                    "event":"mcp_tool_request_parsed",
                    "server_trace_id":window,
                    "client_window_key":window,
                    "tool_name":"runtime_status",
                    "wall_unix_ns":index*1_000_000_000,
                }])
            with self.assertRaisesRegex(ValueError, "expected one client window"):
                p.profile(driver, traces, root/"out", "ambiguous")


if __name__ == "__main__":
    unittest.main()
