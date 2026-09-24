import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import phase16c_variance_profile as p


def write_profile(path: Path, *, total: float, helper_start: float | None = None,
                  helper_to_server: float | None = None, handler: float = 2.0) -> None:
    helper = None
    unattributed = {"prompt_to_helper_ingress_ms": helper_start}
    if helper_to_server is not None:
        helper = {
            "ingress_to_server_received_ms": helper_to_server,
            "ingress_pre_backend_ms": 1.0,
            "backend_send_to_server_received_ms": helper_to_server - 1.0,
        }
    path.write_text(json.dumps({
        "first_tool": {
            "server_trace_id": path.stem,
            "tool_name": "runtime_status",
            "prompt_to_server_received_ms": total,
            "server_total_to_handler_return_ms": handler,
            "helper_join_status": "server_projected_exact_trace" if helper else "unavailable",
            "helper_timing": helper,
        },
        "unattributed": unattributed,
    }))


class Phase16CVarianceTests(unittest.TestCase):
    def test_analyze_interleaved_pairs_and_helper_spans(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            write_profile(root / "c1.json", total=10.0, helper_start=8.0, helper_to_server=2.0)
            write_profile(root / "w1.json", total=7.0)
            write_profile(root / "w2.json", total=9.0)
            write_profile(root / "c2.json", total=12.0, helper_start=10.0, helper_to_server=2.0)
            manifest = root / "manifest.json"
            manifest.write_text(json.dumps({
                "schema_version": 1,
                "runs": [
                    {"run_id": "c1", "harness": "chadex", "pair_id": "p1",
                     "order_position": 1, "connector_state": "cold",
                     "registry_state": "cold", "chat_state": "fresh", "profile": "c1.json"},
                    {"run_id": "w1", "harness": "webcodex", "pair_id": "p1",
                     "order_position": 2, "connector_state": "warm",
                     "registry_state": "warm", "chat_state": "fresh", "profile": "w1.json"},
                    {"run_id": "w2", "harness": "webcodex", "pair_id": "p2",
                     "order_position": 1, "connector_state": "cold",
                     "registry_state": "cold", "chat_state": "fresh", "profile": "w2.json"},
                    {"run_id": "c2", "harness": "chadex", "pair_id": "p2",
                     "order_position": 2, "connector_state": "warm",
                     "registry_state": "warm", "chat_state": "fresh", "profile": "c2.json"},
                ],
            }))
            result = p.analyze(manifest)
            self.assertEqual(result["run_count"], 4)
            self.assertEqual(result["prompt_to_server_by_harness"]["chadex"]["median_ms"], 11.0)
            self.assertEqual(result["paired_chadex_minus_webcodex_ms"]["median_ms"], 3.0)
            self.assertEqual(
                result["chadex_attribution_spans"]["prompt_to_helper_ingress_ms"]["median_ms"],
                9.0,
            )
            self.assertEqual(
                result["chadex_attribution_spans"]["helper_ingress_to_server_received_ms"]["range_ms"],
                0.0,
            )
            self.assertEqual(
                result["grouped_effects"]["harness_by_order_position"]["chadex|1"]["median_ms"],
                10.0,
            )

    def test_duplicate_harness_in_pair_fails_closed(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            write_profile(root / "a.json", total=1.0)
            write_profile(root / "b.json", total=2.0)
            manifest = root / "manifest.json"
            manifest.write_text(json.dumps({
                "runs": [
                    {"run_id": "a", "harness": "chadex", "pair_id": "p",
                     "order_position": 1, "profile": "a.json"},
                    {"run_id": "b", "harness": "chadex", "pair_id": "p",
                     "order_position": 2, "profile": "b.json"},
                ],
            }))
            with self.assertRaisesRegex(ValueError, "duplicate harness"):
                p.analyze(manifest)


if __name__ == "__main__":
    unittest.main()
