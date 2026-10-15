#!/usr/bin/env python3
"""Exercise the fixed Runner input-preparation script against synthetic files.
Never reads production/private project data; no network or provider calls.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'runtime-engine/src/tool_runtime/goal_environment.rs'
CODE = re.search(r'const PREPARE_INPUTS: &str = r#"(.*?)"#;', SOURCE.read_text(), re.S).group(1)


class ScopedInputs(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='chadex-inputs-regression-')
        self.root = Path(self.temp.name).resolve()
        self.source = self.root / 'source'
        self.work = self.root / 'work'
        self.source.mkdir()
        self.work.mkdir()
        (self.source / 'private.json').write_bytes(b'synthetic private fixture only')
        self.sha = hashlib.sha256((self.source / 'private.json').read_bytes()).hexdigest()
        self.requirement = {'source_path': 'private.json', 'source_sha256': self.sha,
                            'destination_path': 'evidence/private/fixture.json', 'permission': 'read_only'}

    def tearDown(self):
        self.temp.cleanup()

    def run_setup(self, requirements=None, dependencies=None):
        return subprocess.run([sys.executable, '-I', '-S', '-c', CODE, str(self.source), str(self.work),
                               json.dumps({'workspace_requirements': requirements or [], 'dependencies': dependencies or []})],
                              capture_output=True, text=True, timeout=20)

    def test_copy_is_digest_bound_read_only_and_source_is_unchanged(self):
        before = (self.source / 'private.json').stat().st_mode
        proc = self.run_setup([self.requirement])
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(json.loads(proc.stdout)['ready'])
        copied = self.work / self.requirement['destination_path']
        self.assertEqual(hashlib.sha256(copied.read_bytes()).hexdigest(), self.sha)
        self.assertEqual(copied.stat().st_mode & 0o777, 0o444)
        self.assertEqual((self.source / 'private.json').stat().st_mode, before)
        self.assertEqual(hashlib.sha256((self.source / 'private.json').read_bytes()).hexdigest(), self.sha)

    def test_missing_fixture_reports_not_ready_without_copy(self):
        requirement = {**self.requirement, 'source_path': 'missing.json'}
        proc = self.run_setup([requirement])
        self.assertEqual(proc.returncode, 0)
        self.assertFalse(json.loads(proc.stdout)['ready'])
        self.assertFalse((self.work / requirement['destination_path']).exists())

    def test_bad_hash_refuses_copy(self):
        proc = self.run_setup([{**self.requirement, 'source_sha256': '0' * 64}])
        self.assertNotEqual(proc.returncode, 0)
        self.assertFalse((self.work / self.requirement['destination_path']).exists())

    def test_source_symlink_cannot_escape_original_project(self):
        (self.source / 'escape').symlink_to(self.root)
        proc = self.run_setup([{**self.requirement, 'source_path': 'escape/source/private.json'}])
        self.assertNotEqual(proc.returncode, 0)
        self.assertFalse((self.work / self.requirement['destination_path']).exists())

    def test_destination_symlink_cannot_write_outside_worktree(self):
        outside = self.root / 'outside'
        outside.mkdir()
        (self.work / 'evidence').symlink_to(outside)
        proc = self.run_setup([self.requirement])
        self.assertNotEqual(proc.returncode, 0)
        self.assertFalse((outside / 'private/fixture.json').exists())

    def test_existing_destination_hardlink_cannot_chmod_source(self):
        target = self.work / self.requirement['destination_path']
        target.parent.mkdir(parents=True)
        os.link(self.source / 'private.json', target)
        before = (self.source / 'private.json').stat().st_mode
        proc = self.run_setup([self.requirement])
        self.assertNotEqual(proc.returncode, 0)
        self.assertEqual((self.source / 'private.json').stat().st_mode, before)

    def test_dependency_missing_or_unverifiable_provenance_blocks(self):
        proc = self.run_setup(dependencies=[{'path': 'node_modules', 'provenance': 'copied somehow'}])
        self.assertEqual(proc.returncode, 0)
        self.assertFalse(json.loads(proc.stdout)['ready'])
        causes = {item['cause'] for item in json.loads(proc.stdout)['missing']}
        self.assertIn('DEPENDENCY_MISSING', causes)
        self.assertIn('DEPENDENCY_PROVENANCE_UNVERIFIED', causes)

    def test_dependency_lockfile_digest_is_checked(self):
        (self.work / 'node_modules').mkdir()
        lock = self.work / 'package-lock.json'
        lock.write_bytes(b'{"synthetic":true}')
        digest = hashlib.sha256(lock.read_bytes()).hexdigest()
        proc = self.run_setup(dependencies=[{'path': 'node_modules', 'provenance': f'sha256:{digest}:package-lock.json:{hashlib.sha256(b"").hexdigest()}'}])
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(json.loads(proc.stdout)['ready'])
        lock.write_bytes(b'changed')
        proc = self.run_setup(dependencies=[{'path': 'node_modules', 'provenance': f'sha256:{digest}:package-lock.json:{hashlib.sha256(b"").hexdigest()}'}])
        self.assertNotEqual(proc.returncode, 0)

    def dependency_fixture(self):
        tree = self.source / 'node_modules'
        tree.mkdir()
        (tree / 'dependency.js').write_bytes(b'synthetic dependency')
        record = ['dependency.js', 'file', hashlib.sha256(b'synthetic dependency').hexdigest(), 0]
        tree_hash = hashlib.sha256((json.dumps(record, separators=(',', ':')) + '\n').encode()).hexdigest()
        lock = self.work / 'package-lock.json'
        lock.write_bytes(b'{"synthetic":true}')
        lock_hash = hashlib.sha256(lock.read_bytes()).hexdigest()
        return tree, {'path':'node_modules', 'provenance':f'sha256:{lock_hash}:package-lock.json:{tree_hash}'}

    def test_approved_dependency_tree_is_copied_and_source_unchanged(self):
        tree, dependency = self.dependency_fixture()
        before = (tree / 'dependency.js').stat().st_mode
        proc = self.run_setup(dependencies=[dependency])
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(json.loads(proc.stdout)['ready'])
        self.assertEqual((self.work / 'node_modules/dependency.js').read_bytes(), b'synthetic dependency')
        self.assertEqual((tree / 'dependency.js').stat().st_mode, before)
        self.assertEqual((self.work / 'node_modules/dependency.js').stat().st_mode & 0o777, 0o444)
        replay = self.run_setup(dependencies=[dependency])
        self.assertEqual(replay.returncode, 0, replay.stderr)

    def test_dependency_source_change_cannot_be_silently_copied(self):
        tree, dependency = self.dependency_fixture()
        (tree / 'dependency.js').write_bytes(b'changed unapproved input')
        proc = self.run_setup(dependencies=[dependency])
        self.assertNotEqual(proc.returncode, 0)
        self.assertFalse((self.work / 'node_modules').exists())

    def test_existing_dependency_mutation_is_detected(self):
        tree, dependency = self.dependency_fixture()
        proc = self.run_setup(dependencies=[dependency])
        self.assertEqual(proc.returncode, 0, proc.stderr)
        copied = self.work / 'node_modules/dependency.js'
        copied.chmod(0o644); copied.write_bytes(b'mutated dependency')
        proc = self.run_setup(dependencies=[dependency])
        self.assertNotEqual(proc.returncode, 0)

    def test_dependency_external_symlink_is_rejected(self):
        tree, dependency = self.dependency_fixture()
        (tree / 'escape').symlink_to(self.root)
        proc = self.run_setup(dependencies=[dependency])
        self.assertNotEqual(proc.returncode, 0)
        self.assertFalse((self.work / 'node_modules').exists())

    def test_dependency_root_symlink_is_rejected(self):
        tree, dependency = self.dependency_fixture()
        (self.work / 'node_modules').symlink_to(tree)
        proc = self.run_setup(dependencies=[dependency])
        self.assertNotEqual(proc.returncode, 0)


if __name__ == '__main__':
    unittest.main(verbosity=2)
