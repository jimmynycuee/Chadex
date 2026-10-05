#!/usr/bin/env python3
import tempfile
import unittest
from pathlib import Path

import ci_change_scope as scope


class ChangeScopeTests(unittest.TestCase):
    def test_docs_and_graph_only_changes_are_docs_only(self):
        self.assertTrue(scope.is_docs_only([
            "docs/ARCHITECTURE.md",
            "docs/windows/evidence/W5_readiness_status.json",
            "graphify-out/GRAPH_REPORT.md",
            "README.md",
            "PHASES.md",
            ".graphifyignore",
        ]))

    def test_any_code_or_packaged_path_requires_full_pipeline(self):
        for path in [
            "Sources/ChadexApp/AppModel.swift",
            "rust-helper/src/runtime_bridge.rs",
            ".github/workflows/ci.yml",
            "scripts/ci_change_scope.py",
            "UPSTREAM.md",
            "LICENSE",
            "ui-review/main-light.png",
            "runtime-engine/README.md",
            "docsx/notes.md",
        ]:
            with self.subTest(path=path):
                self.assertFalse(scope.is_docs_only(["docs/ARCHITECTURE.md", path]))

    def test_empty_change_set_is_not_docs_only(self):
        self.assertFalse(scope.is_docs_only([]))

    def test_missing_or_zero_base_fails_closed(self):
        self.assertIsNone(scope.resolve_base("push", "", ""))
        self.assertIsNone(scope.resolve_base("push", "0" * 40, ""))
        self.assertIsNone(scope.resolve_base("workflow_dispatch", "abc", "def"))
        self.assertIsNone(scope.resolve_base("push", "f" * 40, ""))

    def test_relative_link_check_ignores_external_anchor_and_fenced_links(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "docs").mkdir()
            (root / "docs/present.md").write_text("ok\n", encoding="utf-8")
            (root / "docs/page.md").write_text(
                "[a](present.md) [b](present.md#part) [c](https://example.com)\n"
                "[d](#local) [e](mailto:x@example.com) [h](present.md:12)\n"
                "```\n[f](missing-in-fence.md)\n```\n"
                "[g](missing.md)\n",
                encoding="utf-8",
            )
            problems = scope.broken_relative_links(root, ["docs/page.md", "docs/deleted.md"])
        self.assertEqual(problems, ["docs/page.md:6: missing link target missing.md"])


if __name__ == "__main__":
    unittest.main()
