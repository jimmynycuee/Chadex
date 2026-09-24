#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("sync_graphify_obsidian.py")
SPEC = importlib.util.spec_from_file_location("sync_graphify_obsidian", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class GraphifyObsidianSyncTests(unittest.TestCase):
    def sample_graph(self):
        return {
            "built_at_commit": "abc123",
            "nodes": [
                {
                    "id": "appmodel",
                    "label": "AppModel",
                    "source_file": "Sources/ChadexApp/AppModel.swift",
                    "source_location": "L10",
                },
                {
                    "id": "helper",
                    "label": "HelperClient",
                    "source_file": "Sources/ChadexApp/HelperClient.swift",
                    "source_location": "L20",
                },
                {
                    "id": "mcp",
                    "label": "McpServer",
                    "source_file": "runtime-engine/src/mcp.rs",
                    "source_location": "L30",
                },
                {
                    "id": "executor",
                    "label": "execute_task",
                    "source_file": "runtime-engine/src/tool_runtime/chadex_task_executor.rs",
                    "source_location": "L40",
                },
                {
                    "id": "vendor",
                    "label": "VendorServer",
                    "source_file": "vendor/webcodex/src/server.rs",
                    "source_location": "L1",
                },
            ],
            "links": [
                {"source": "appmodel", "target": "helper", "relation": "calls", "confidence": "EXTRACTED"},
                {"source": "helper", "target": "mcp", "relation": "calls", "confidence": "EXTRACTED"},
                {"source": "mcp", "target": "executor", "relation": "calls", "confidence": "EXTRACTED"},
                {"source": "vendor", "target": "mcp", "relation": "calls", "confidence": "EXTRACTED"},
            ],
        }

    def test_outputs_are_curated_and_exclude_vendor(self):
        outputs, stats = MODULE.build_outputs(self.sample_graph(), "def456", 20)
        joined = "\n".join(outputs.values())
        self.assertIn("[[P15 Execution Attribution]]", joined)
        self.assertIn("runtime-engine/src/mcp.rs", joined)
        self.assertNotIn("vendor/webcodex/src/server.rs", joined)
        self.assertTrue(stats["metadata_source_mismatch"])
        self.assertGreaterEqual(stats["components"], 10)

    def test_sync_is_idempotent_and_check_detects_drift(self):
        outputs, _ = MODULE.build_outputs(self.sample_graph(), "abc123", 20)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            changed, stale = MODULE.sync_outputs(root, outputs, check=False, dry_run=False)
            self.assertTrue(changed)
            self.assertFalse(stale)

            changed, stale = MODULE.sync_outputs(root, outputs, check=True, dry_run=False)
            self.assertFalse(changed)
            self.assertFalse(stale)

            target = root / MODULE.GENERATED_ROOT / "Graphify Snapshot.md"
            target.write_text("drift", encoding="utf-8")
            changed, _ = MODULE.sync_outputs(root, outputs, check=True, dry_run=False)
            self.assertIn(MODULE.GENERATED_ROOT / "Graphify Snapshot.md", changed)


if __name__ == "__main__":
    unittest.main()
