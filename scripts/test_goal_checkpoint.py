#!/usr/bin/env python3
"""Actual Git staging regression with synthetic inputs; never production repos."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

SOURCE=Path(__file__).resolve().parents[1]/'runtime-engine/src/tool_runtime/goal_environment.rs'
CODE=re.search(r'const STAGE_CHECKPOINT:&str=r#"(.*?)"#;',SOURCE.read_text(),re.S).group(1)
class CheckpointStaging(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='chadex-stage-regression-')
        self.root=Path(self.temp.name).resolve()
        self.git('init','-q','-b','main')
        (self.root/'tracked.txt').write_text('before')
        (self.root/'deleted.txt').write_text('remove')
        self.git('add','-A');self.git('-c','user.name=fixture','-c','user.email=fixture@local','commit','-qm','init')
        self.head=self.git('rev-parse','HEAD')
    def tearDown(self):self.temp.cleanup()
    def git(self,*args):return subprocess.check_output(['git','-C',str(self.root),*args],text=True).strip()
    def stage(self,protected):
        prefix=[f'--git-dir={self.root}/.git','--work-tree=.','-c','core.hooksPath=/dev/null','-c','core.fsmonitor=false']
        return subprocess.run([sys.executable,'-I','-S','-c',CODE,str(self.root),'git',json.dumps(prefix),json.dumps(protected)],capture_output=True,text=True)
    def test_ignored_inputs_do_not_fail_or_enter_index(self):
        (self.root/'.gitignore').write_text('.goal-acceptance/\nnode_modules/\nevidence/private/\n')
        for name in ['.goal-acceptance/cache','node_modules/dependency.js','evidence/private/input.json']:
            path=self.root/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text('SYNTHETIC_PRIVATE')
        (self.root/'tracked.txt').write_text('after');(self.root/'deleted.txt').unlink()
        result=self.stage(['.goal-acceptance','node_modules','evidence/private'])
        self.assertEqual(result.returncode,0,result.stderr)
        names=self.git('diff','--cached','--name-only').splitlines()
        self.assertIn('tracked.txt',names);self.assertIn('deleted.txt',names)
        self.assertFalse(any(name.startswith(('.goal-acceptance','node_modules','evidence/private')) for name in names))
        self.assertEqual(self.git('rev-parse','HEAD'),self.head)
    def test_unignored_or_prestaged_private_files_are_preserved_but_not_committed(self):
        path=self.root/'private/input.json';path.parent.mkdir();path.write_text('SYNTHETIC_PRIVATE')
        self.git('add','private/input.json')
        result=self.stage(['private'])
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(self.git('ls-files','private/input.json'),'')
        self.assertEqual(path.read_text(),'SYNTHETIC_PRIVATE')
        self.assertEqual(self.git('rev-parse','HEAD'),self.head)
    def test_literal_metacharacters_are_data_and_neighbors_still_stage(self):
        path=self.root/'private[1].txt';path.write_text('SYNTHETIC_PRIVATE')
        (self.root/'private1.txt').write_text('public neighbor')
        (self.root/':(exclude)tricky.txt').write_text('literal public name')
        result=self.stage(['private[1].txt'])
        self.assertEqual(result.returncode,0,result.stderr)
        names=self.git('diff','--cached','--name-only').splitlines()
        self.assertNotIn('private[1].txt',names)
        self.assertIn('private1.txt',names)
        self.assertIn(':(exclude)tricky.txt',names)

if __name__=='__main__':unittest.main(verbosity=2)
