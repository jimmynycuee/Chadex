import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("phase15_attribution.py")
SPEC = importlib.util.spec_from_file_location("phase15_attribution", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class AttributionTests(unittest.TestCase):
    def test_outer_unobserved_time_stays_unattributed(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "outer.jsonl"
            events = [
                {"event": "prompt_submitted", "wall_ns": 0},
                {"event": "tool_start", "id": "a", "wall_ns": 20_000_000_000},
                {"event": "tool_end", "id": "a", "wall_ns": 22_000_000_000},
                {"event": "final_response_end", "wall_ns": 50_000_000_000},
            ]
            path.write_text("\n".join(json.dumps(row) for row in events))
            result = MODULE.outer_summary(path)
            self.assertEqual(result["prompt_to_first_tool_ms"], 20_000)
            self.assertEqual(result["outside_tool_unattributed_ms"], 48_000)
            self.assertIsNone(result["model_backend_ms"])
            self.assertIsNone(result["command_reported_sum_ms"])

    def test_server_envelopes_and_missing_runner_are_not_zero(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "events.jsonl"
            events = [
                {"event": "api_tool_request_received", "server_trace_id": "a", "request_elapsed_ns": 0},
                {"event": "api_tool_dispatch_started", "request_elapsed_ns": 2_000_000},
                {"event": "api_tool_dispatch_finished", "request_elapsed_ns": 5_000_000},
                {"event": "api_tool_response_serialized", "request_elapsed_ns": 6_000_000},
                {"event": "api_tool_handler_returned", "request_elapsed_ns": 7_000_000},
            ]
            path.write_text("\n".join(json.dumps(row) for row in events))
            result = MODULE.server_summary(path)
            self.assertEqual(result["request_preparation_envelope_ms"], 2)
            self.assertEqual(result["tool_dispatch_envelope_ms"], 3)
            self.assertEqual(result["tool_result_projection_ms"], 1)
            self.assertEqual(result["runner"], [])
            self.assertIsNone(result["auth_validation_ms"])
            self.assertIsNone(result["model_first_token_ms"])

    def test_helper_joins_only_by_exact_server_trace_id(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "helper.json"
            path.write_text(json.dumps([{"server_trace_id": "a", "total_us": 1234,
                                         "ingress_pre_backend_us": 12}]))
            indexed = MODULE.helper_index([path])
            self.assertEqual(indexed["a"]["native_ingress_total_ms"], 1.234)
            self.assertIsNone(indexed["a"]["native_backend_headers_ms"])
            self.assertNotIn("b", indexed)

    def test_runner_queue_uses_monotonic_process_clock(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "events.jsonl"
            events = [
                {"event": "mcp_tool_request_received", "server_trace_id": "a", "request_elapsed_ns": 0},
                {"event": "tool_runner_request_enqueued", "runner_request_id": "r", "process_elapsed_ns": 100,
                 "wall_unix_ns": 9_000_000_000},
                {"event": "tool_runner_request_dispatched", "runner_request_id": "r", "process_elapsed_ns": 2_000_100,
                 "wall_unix_ns": 1},
                {"event": "tool_runner_result_accepted", "runner_request_id": "r", "process_elapsed_ns": 5_000_100,
                 "wall_unix_ns": 2, "runner_reported_duration_ms": 1},
            ]
            path.write_text("\n".join(json.dumps(row) for row in events))
            runner = MODULE.server_summary(path)["runner"][0]
            self.assertEqual(runner["queue_to_dispatch_ms"], 2)
            self.assertEqual(runner["dispatch_to_accepted_ms"], 3)
            self.assertEqual(runner["runner_reported_duration_ms"], 1)

    def test_startup_residual_requires_observed_subphases(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "events.jsonl"
            events = [
                {"event": "work_on_project_prepared", "server_trace_id": "a", "duration_ns": 100_000_000},
                {"event": "coding_context_probes", "duration_ns": 20_000_000},
                {"event": "coding_session_create_ensure", "duration_ns": 5_000_000},
            ]
            path.write_text("\n".join(json.dumps(row) for row in events))
            self.assertEqual(MODULE.server_summary(path)["startup_other_unattributed_ms"], 75)
            path.write_text(json.dumps(events[0]))
            self.assertIsNone(MODULE.server_summary(path)["startup_other_unattributed_ms"])


if __name__ == "__main__":
    unittest.main()
