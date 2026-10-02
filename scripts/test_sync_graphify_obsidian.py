#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import os
import subprocess
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

    def test_windows_sources_are_curated_and_smoke_is_excluded(self):
        graph = self.sample_graph()
        sources = [
            "apps/windows/src/App.tsx",
            "apps/windows/src/state.ts",
            "apps/windows/src-tauri/src/main.rs",
            "apps/windows/bridge/src/transport.rs",
            "apps/windows/src-tauri/src/credentials.rs",
            "apps/windows/src-tauri/src/preferences.rs",
        ]
        graph["nodes"].extend({"id": path, "label": Path(path).stem, "source_file": path} for path in sources)
        outputs, stats = MODULE.build_outputs(graph, "abc123", 48)
        joined = "\n".join(outputs.values())
        for path in sources:
            self.assertIn(path, joined, path)
        self.assertIn("[[W3 Desktop Product]]", joined)
        self.assertEqual(stats["components"], 17)
        for path in [
            "apps/windows/src-tauri/src/smoke.rs",
            "apps/windows/bridge/src/bin/smoke.rs",
            "apps/windows/src/state.test.ts",
            "apps/windows/src/App.spec.tsx",
            "apps/windows/src/env.d.ts",
            "apps/windows/node_modules/library/index.ts",
            "apps/windows/src-tauri/target/generated.rs",
            "vendor/webcodex/src/main.rs",
        ]:
            self.assertFalse(MODULE.is_production_file(path), path)

    def test_output_does_not_depend_on_graph_input_order(self):
        graph = self.sample_graph()
        expected, _ = MODULE.build_outputs(graph, "abc123", 48)
        graph["nodes"].reverse()
        graph["links"].reverse()
        actual, _ = MODULE.build_outputs(graph, "abc123", 48)
        self.assertEqual(expected, actual)

    def test_canvas_and_generated_links_use_vault_paths(self):
        prefix = Path("01 Projects/Chadex")
        outputs, _ = MODULE.build_outputs(self.sample_graph(), "abc123", 48, prefix)
        canvas = json.loads(outputs[MODULE.CANVAS_PATH])
        for node in canvas["nodes"]:
            self.assertTrue(node["file"].startswith(prefix.as_posix() + "/"))
            self.assertIn(Path(node["file"]).relative_to(prefix), outputs)
        snapshot = outputs[MODULE.GENERATED_ROOT / "Graphify Snapshot.md"]
        self.assertIn("[[01 Projects/Chadex/Architecture/Generated/Graphify/Components/Desktop App State|Desktop App State]]", snapshot)
        with tempfile.TemporaryDirectory() as temporary:
            vault = Path(temporary)
            (vault / ".obsidian").mkdir()
            self.assertEqual(MODULE.vault_project_prefix(vault / prefix), prefix)

    def test_output_does_not_depend_on_python_hash_seed(self):
        code = (
            "import json; from test_sync_graphify_obsidian import GraphifyObsidianSyncTests, MODULE; "
            "outputs, _ = MODULE.build_outputs(GraphifyObsidianSyncTests().sample_graph(), 'abc123', 48); "
            "print(json.dumps({str(k): v for k, v in outputs.items()}, sort_keys=True))"
        )
        outputs = [subprocess.check_output(
            [sys.executable, "-c", code], cwd=SCRIPT.parent,
            env={**os.environ, "PYTHONHASHSEED": seed}, text=True,
        ) for seed in ("0", "1", "97")]
        self.assertEqual(json.loads(outputs[0]), json.loads(outputs[1]))
        self.assertEqual(json.loads(outputs[0]), json.loads(outputs[2]))


if __name__ == "__main__":
    unittest.main()
