import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

MODULE_PATH = Path(__file__).with_name("phase16a_round3_profile.py")
SPEC = importlib.util.spec_from_file_location("phase16a_round3_profile", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def write_jsonl(path: Path, rows: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(json.dumps(row) for row in rows) + "\n")


class Round3ProfileTests(unittest.TestCase):
    def test_exact_trace_lookup_fails_on_duplicate(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            write_jsonl(root / "a" / "same" / "events.jsonl", [])
            write_jsonl(root / "b" / "same" / "events.jsonl", [])
            with self.assertRaises(ValueError):
                MODULE.exact_server_trace(root, "same")

    def test_tool_name_is_only_integrity_check_after_id_join(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "events.jsonl"
            rows = [
                {"timestamp":"1970-01-01T00:00:01+00:00","event":"mcp_tool_request_received","server_trace_id":"x"},
                {"timestamp":"1970-01-01T00:00:01.001000+00:00","event":"mcp_tool_request_parsed","server_trace_id":"x","tool_name":"tool_manifest"},
                {"timestamp":"1970-01-01T00:00:01.002000+00:00","event":"mcp_tool_dispatch_started","server_trace_id":"x"},
                {"timestamp":"1970-01-01T00:00:01.003000+00:00","event":"mcp_tool_dispatch_finished","server_trace_id":"x"},
                {"timestamp":"1970-01-01T00:00:01.004000+00:00","event":"mcp_tool_response_serialized","server_trace_id":"x"},
                {"timestamp":"1970-01-01T00:00:01.005000+00:00","event":"mcp_tool_handler_returned","server_trace_id":"x"},
            ]
            write_jsonl(path, rows)
            with self.assertRaisesRegex(ValueError, "tool mismatch"):
                MODULE.server_profile(path, "x", "runtime_status")


if __name__ == "__main__":
    unittest.main()
