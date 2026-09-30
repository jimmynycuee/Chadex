import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import phase19a_heartbeat as h


class Phase19AHeartbeatTests(unittest.TestCase):
    def test_latest_health_url_uses_newest_loopback_session(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            old = root / "tunnel-sessions/old/health-url"
            new = root / "tunnel-sessions/new/health-url"
            old.parent.mkdir(parents=True)
            new.parent.mkdir(parents=True)
            old.write_text("http://127.0.0.1:1111\n", encoding="utf-8")
            new.write_text("http://127.0.0.1:2222\n", encoding="utf-8")
            old.touch()
            new.touch()
            self.assertEqual(h.latest_health_url(root), "http://127.0.0.1:2222")

    def test_latest_health_url_rejects_non_loopback(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            path = root / "tunnel-sessions/x/health-url"
            path.parent.mkdir(parents=True)
            path.write_text("https://example.com/status\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "not loopback"):
                h.latest_health_url(root)

    def test_connection_layers_from_tunnel(self):
        result = h.connection_layers_from_tunnel(
            {
                "control_plane_tunnel_id": "tun-123",
                "channels": [{"name": "main", "probe_status": "ok"}],
            }
        )
        self.assertEqual(result["tunnel_health"]["status"], "ready")
        self.assertEqual(result["server_transport"]["status"], "connected")
        self.assertEqual(result["server_transport"]["probe_status"], "ok")
        self.assertEqual(result["control_plane"]["status"], "connected")

    def test_fetch_status_combines_process_and_tunnel_without_credentials(self):
        process = {
            "app_process": {"status": "ready", "source": "process_table"},
            "runtime_runner": {"status": "ready", "source": "process_table"},
        }
        tunnel = {
            "version": "test",
            "client_instance_id": "inst-1",
            "started_at": 123,
            "uptime_seconds": 10,
            "control_plane_tunnel_id": "tun-123",
            "channels": [{"name": "main", "probe_status": "ok"}],
        }
        with mock.patch.object(h, "process_layers", return_value=process), mock.patch.object(
            h, "tunnel_status", return_value=tunnel
        ):
            result = h.fetch_status(Path("/tmp/unused"), "client-x", 0.1)
        self.assertEqual(result["client_id"], "client-x")
        self.assertEqual(result["connection_layers"]["app_process"]["status"], "ready")
        self.assertEqual(result["connection_layers"]["server_transport"]["status"], "connected")
        serialized = json.dumps(result).lower()
        self.assertNotIn("token", serialized)
        self.assertNotIn("authorization", serialized)


if __name__ == "__main__":
    unittest.main()
