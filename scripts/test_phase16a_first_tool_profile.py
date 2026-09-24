import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import phase16a_first_tool_profile as p


class Phase16AProfilerTests(unittest.TestCase):
    def test_scalar_marker_is_exact_not_substring(self):
        value = {"project": "target", "command": "mentions /target/path only"}
        strings = set(p.scalar_strings(value))
        self.assertIn("target", strings)
        self.assertNotIn("/target/path", {"target"}.intersection(strings))

    def test_exact_marker_hit_can_require_tool_and_argument_field(self):
        with tempfile.TemporaryDirectory() as td:
            trace_dir = Path(td)
            (trace_dir / "args.json").write_text(
                json.dumps({"instruction": "PHASE16C_TEST"})
            )
            events = [
                {
                    "event": "mcp_tool_request_parsed",
                    "tool_name": "work_on_project",
                },
                {
                    "event": "tool_trace_payload_captured",
                    "phase": "raw_arguments",
                    "payload_path": "args.json",
                },
            ]
            self.assertTrue(
                p.exact_marker_hit(
                    trace_dir,
                    events,
                    {"PHASE16C_TEST"},
                    expected_tool_name="work_on_project",
                    marker_argument_key="instruction",
                )
            )
            self.assertFalse(
                p.exact_marker_hit(
                    trace_dir,
                    events,
                    {"PHASE16C_TEST"},
                    expected_tool_name="run_process",
                    marker_argument_key="instruction",
                )
            )

    def test_exact_marker_hit_rejects_marker_in_unrelated_argument_field(self):
        with tempfile.TemporaryDirectory() as td:
            trace_dir = Path(td)
            (trace_dir / "args.json").write_text(
                json.dumps({"args": ["PHASE16C_TEST"]})
            )
            events = [
                {
                    "event": "mcp_tool_request_parsed",
                    "tool_name": "run_process",
                },
                {
                    "event": "tool_trace_payload_captured",
                    "phase": "raw_arguments",
                    "payload_path": "args.json",
                },
            ]
            self.assertFalse(
                p.exact_marker_hit(
                    trace_dir,
                    events,
                    {"PHASE16C_TEST"},
                    expected_tool_name="work_on_project",
                    marker_argument_key="instruction",
                )
            )

    def test_profile_joins_window_and_trace_ids(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            driver = root / "driver.json"
            driver.write_text(json.dumps({"submitted_at_ms": 1000, "connector": "Chadex"}))
            trace_root = root / "traces"
            trace_root.mkdir()
            first = trace_root / "trace-a"
            first.mkdir()
            (first / "args.json").write_text(json.dumps({"project": "project-exact"}))
            events = [
                {"timestamp":"1970-01-01T00:00:01.100000+00:00","wall_unix_ns":1_100_000_000,"event":"mcp_tool_request_received","server_trace_id":"trace-a","helper_ingress_started_unix_ns":1_080_000_000,"helper_ingress_pre_backend_us":2_000},
                {"timestamp":"1970-01-01T00:00:01.101000+00:00","event":"mcp_tool_request_parsed","server_trace_id":"trace-a","client_window_key":"window-a","tool_name":"runtime_status"},
                {"timestamp":"1970-01-01T00:00:01.102000+00:00","event":"tool_trace_payload_captured","server_trace_id":"trace-a","phase":"raw_arguments","payload_path":"args.json"},
                {"timestamp":"1970-01-01T00:00:01.103000+00:00","event":"mcp_tool_dispatch_started","server_trace_id":"trace-a"},
                {"timestamp":"1970-01-01T00:00:01.105000+00:00","event":"mcp_tool_dispatch_finished","server_trace_id":"trace-a"},
                {"timestamp":"1970-01-01T00:00:01.106000+00:00","event":"mcp_tool_handler_returned","server_trace_id":"trace-a"},
            ]
            (first / "events.jsonl").write_text("\n".join(json.dumps(x) for x in events)+"\n")
            result = p.profile(driver, trace_root, ["project-exact"], None)
            self.assertEqual(result["selection"]["client_window_key"], "window-a")
            self.assertEqual(result["first_tool"]["server_trace_id"], "trace-a")
            self.assertEqual(result["first_tool"]["prompt_to_server_received_ms"], 100.0)
            self.assertEqual(result["first_tool"]["server_total_to_handler_return_ms"], 6.0)
            self.assertEqual(result["first_tool"]["helper_join_status"], "server_projected_exact_trace")
            self.assertEqual(result["first_tool"]["helper_timing"]["ingress_to_server_received_ms"], 20.0)
            self.assertEqual(result["first_tool"]["helper_timing"]["ingress_pre_backend_ms"], 2.0)
            self.assertEqual(result["first_tool"]["helper_timing"]["backend_send_to_server_received_ms"], 18.0)
            self.assertEqual(result["unattributed"]["prompt_to_helper_ingress_ms"], 80.0)

    def test_multiple_marker_windows_fail_closed(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            driver = root / "driver.json"
            driver.write_text(json.dumps({"submitted_at_ms": 0}))
            traces = root / "traces"; traces.mkdir()
            for idx in (1, 2):
                d = traces / f"t{idx}"; d.mkdir()
                (d / "args.json").write_text(json.dumps({"project": "same-project"}))
                rows = [
                    {"timestamp":f"1970-01-01T00:00:0{idx}.000000+00:00","event":"mcp_tool_request_parsed","server_trace_id":f"t{idx}","client_window_key":f"w{idx}","tool_name":"x"},
                    {"timestamp":f"1970-01-01T00:00:0{idx}.001000+00:00","event":"tool_trace_payload_captured","server_trace_id":f"t{idx}","phase":"raw_arguments","payload_path":"args.json"},
                ]
                (d / "events.jsonl").write_text("\n".join(json.dumps(x) for x in rows)+"\n")
            with self.assertRaisesRegex(ValueError, "exact-marker client window"):
                p.profile(driver, traces, ["same-project"], None)


if __name__ == "__main__":
    unittest.main()
