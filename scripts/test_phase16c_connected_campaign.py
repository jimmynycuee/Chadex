import tempfile
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import phase16c_connected_campaign as c


class Phase16CConnectedCampaignTests(unittest.TestCase):
    def test_balanced_orders_are_reproducible_and_balanced(self):
        first = c.balanced_orders(10, 1603)
        second = c.balanced_orders(10, 1603)
        self.assertEqual(first, second)
        self.assertEqual(first.count(("chadex", "webcodex")), 5)
        self.assertEqual(first.count(("webcodex", "chadex")), 5)

    def test_balanced_orders_reject_odd_pair_count(self):
        with self.assertRaisesRegex(ValueError, "even"):
            c.balanced_orders(9, 1)

    def test_new_run_limit_is_optional_and_bounded(self):
        self.assertFalse(c.new_run_limit_reached(10, None))
        self.assertFalse(c.new_run_limit_reached(0, 1))
        self.assertTrue(c.new_run_limit_reached(1, 1))
        self.assertTrue(c.new_run_limit_reached(2, 1))

    def test_only_submit_and_all_stages_need_launcher_lock(self):
        self.assertTrue(c.stage_needs_launcher_lock("all"))
        self.assertTrue(c.stage_needs_launcher_lock("submit"))
        self.assertFalse(c.stage_needs_launcher_lock("commit"))

    def test_launcher_identity_is_pinned_and_change_fails_closed(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            descriptor = root / "launcher-browser.json"
            output = root / "campaign"
            output.mkdir()
            descriptor.write_text(
                '{"pid": 123, "endpoint": "http://127.0.0.1:9222"}'
            )
            identity = c.pin_launcher_identity(output, descriptor)
            self.assertEqual(identity["pid"], 123)
            self.assertEqual(identity["endpoint"], "http://127.0.0.1:9222")
            descriptor.write_text(
                '{"pid": 456, "endpoint": "http://127.0.0.1:9333"}'
            )
            with self.assertRaisesRegex(RuntimeError, "identity changed"):
                c.pin_launcher_identity(output, descriptor)

    def test_probe_prompt_is_single_tool_and_carries_exact_marker(self):
        arm = {"connector": "Chadex", "client_id": "device-abc"}
        prompt = c.probe_prompt(
            arm,
            Path("/tmp/example"),
            "PHASE16C_0123456789abcdef",
        )
        self.assertIn("first and only tool call", prompt)
        self.assertIn("work_on_project", prompt)
        self.assertIn("device-abc", prompt)
        self.assertIn("/tmp/example", prompt)
        self.assertIn("PHASE16C_0123456789abcdef", prompt)
        self.assertIn("include_project_instructions false", prompt)
        self.assertIn("include_workflow_guidance false", prompt)
        self.assertIn("Do not inspect, edit, run", prompt)

    def test_available_attempt_dir_preserves_prior_attempt(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td) / "run"
            self.assertEqual(c.available_attempt_dir(base), base)
            base.mkdir()
            (base / "driver.json").write_text("{}")
            self.assertEqual(c.available_attempt_dir(base), base / "attempt-02")
            (base / "attempt-02").mkdir()
            self.assertEqual(c.available_attempt_dir(base), base / "attempt-03")

    def test_submitted_attempt_recovers_exactly_one_submitted_turn(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td) / "run"
            base.mkdir()
            (base / "driver.json").write_text('{"submitted_at_ms": null}')
            attempt = base / "attempt-02"
            attempt.mkdir()
            (attempt / "driver.json").write_text(
                '{"driver_exit_code": 0, "submitted_at_ms": 1234}'
            )
            recovered = c.submitted_attempt(base)
            self.assertIsNotNone(recovered)
            run_dir, driver = recovered
            self.assertEqual(run_dir, attempt)
            self.assertEqual(driver["submitted_at_ms"], 1234)

    def test_submitted_attempt_rejects_multiple_submitted_turns(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td) / "run"
            base.mkdir()
            (base / "driver.json").write_text(
                '{"driver_exit_code": 0, "submitted_at_ms": 1}'
            )
            attempt = base / "attempt-02"
            attempt.mkdir()
            (attempt / "driver.json").write_text(
                '{"driver_exit_code": 0, "submitted_at_ms": 2}'
            )
            with self.assertRaisesRegex(RuntimeError, "multiple submitted attempts"):
                c.submitted_attempt(base)

    def test_submitted_attempt_recovers_durable_checkpoint_without_parent_exit_code(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td) / "run"
            base.mkdir()
            (base / "driver.json").write_text(
                '{"submission_checkpoint_version": 1, "submission_committed": true, '
                '"driver_self_reported_stage": "accepted", "submitted_at_ms": 1234, '
                '"accepted_at_ms": 1250}'
            )
            recovered = c.submitted_attempt(base)
            self.assertIsNotNone(recovered)
            _, driver = recovered
            self.assertTrue(c.durable_submission_checkpoint(driver))

    def test_submitted_attempt_ignores_interrupted_legacy_submission(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td) / "run"
            base.mkdir()
            (base / "driver.json").write_text('{"submitted_at_ms": 1234}')
            self.assertIsNone(c.submitted_attempt(base))

    def test_validate_driver_turn_accepts_submission_only_probe(self):
        c.validate_driver_turn(
            harness="webcodex",
            connector="WebCodex",
            driver={
                "driver_exit_code": 0,
                "completed": False,
                "accepted": True,
                "completion_mode": "submission-only",
                "temporary_chat": True,
                "connector": "WebCodex",
                "submission_count": 1,
            },
        )

    def test_validate_driver_turn_accepts_durable_checkpoint_without_parent_exit_code(self):
        c.validate_driver_turn(
            harness="chadex",
            connector="Chadex",
            driver={
                "completed": False,
                "accepted": True,
                "completion_mode": "submission-only",
                "submission_checkpoint_version": 1,
                "submission_committed": True,
                "driver_self_reported_stage": "accepted",
                "submitted_at_ms": 1234,
                "accepted_at_ms": 1250,
                "temporary_chat": True,
                "connector": "Chadex",
                "submission_count": 1,
            },
        )

    def test_validate_driver_turn_rejects_interrupted_submission_without_checkpoint(self):
        with self.assertRaisesRegex(RuntimeError, "browser driver failed"):
            c.validate_driver_turn(
                harness="chadex",
                connector="Chadex",
                driver={
                    "completed": False,
                    "accepted": True,
                    "completion_mode": "submission-only",
                    "submitted_at_ms": 1234,
                    "accepted_at_ms": 1250,
                    "temporary_chat": True,
                    "connector": "Chadex",
                    "submission_count": 1,
                },
            )

    def test_validate_driver_turn_rejects_unfinished_non_probe(self):
        with self.assertRaisesRegex(RuntimeError, "did not complete"):
            c.validate_driver_turn(
                harness="webcodex",
                connector="WebCodex",
                driver={
                    "driver_exit_code": 0,
                    "completed": False,
                    "temporary_chat": True,
                    "connector": "WebCodex",
                    "submission_count": 1,
                },
            )


if __name__ == "__main__":
    unittest.main()
