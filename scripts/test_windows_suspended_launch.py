#!/usr/bin/env python3
import unittest
from pathlib import Path
from unittest.mock import Mock

import windows_suspended_launch as launch


class SuspendedLaunchTests(unittest.TestCase):
    def invoke(self, process, sample, owned):
        resume = Mock()
        forced = Mock()
        factory = Mock(return_value=process)
        result = launch.launch_owned(
            'installer.exe /S /D=C:\\space path', executable=Path('installer.exe'),
            cwd=Path('fixture'), env=None, powershell='powershell', owned=owned,
            on_forced=forced, popen=factory, resume=resume, sample=sample)
        return result, factory, resume, forced

    def test_root_is_observed_before_execution_and_committed_before_resume(self):
        process = Mock(pid=100)
        process.poll.return_value = None
        owned = {}
        def sample(_shell, pid, temporary):
            self.assertEqual(owned, {})
            temporary[pid] = {'Name':'installer.exe', 'Created':'original'}
        result, factory, resume, forced = self.invoke(process, sample, owned)
        self.assertIs(result, process)
        self.assertEqual(factory.call_args.kwargs['creationflags'], 4)
        self.assertFalse(factory.call_args.kwargs['shell'])
        self.assertEqual(owned[100]['Created'], 'original')
        resume.assert_called_once_with(process)
        forced.assert_not_called()

    def test_fast_exit_cannot_be_accepted_as_an_empty_clean_tree(self):
        process = Mock(pid=100)
        process.poll.return_value = 0
        owned = {}
        sample = Mock()
        with self.assertRaisesRegex(launch.LaunchFailure, 'initial_process_exited'):
            self.invoke(process, sample, owned)
        sample.assert_not_called()
        self.assertEqual(owned, {})
        process.kill.assert_not_called()

    def test_a_root_exit_during_sampling_discards_the_bare_pid_sample(self):
        process = Mock(pid=100)
        process.poll.side_effect = [None, 0, 0]
        owned = {}
        def sample(_shell, pid, temporary):
            temporary[pid] = {'Name':'unrelated.exe', 'Created':'reused'}
        with self.assertRaisesRegex(launch.LaunchFailure, 'initial_process_identity_invalid'):
            self.invoke(process, sample, owned)
        self.assertEqual(owned, {})
        process.kill.assert_not_called()

    def test_missing_creation_identity_never_enters_tree_cleanup(self):
        process = Mock(pid=100)
        process.poll.return_value = None
        owned = {}
        def sample(_shell, pid, temporary):
            temporary[pid] = {'Name':'installer.exe', 'Created':None}
        with self.assertRaisesRegex(launch.LaunchFailure, 'initial_process_identity_invalid'):
            self.invoke(process, sample, owned)
        self.assertEqual(owned, {})
        process.kill.assert_called_once()
        process.wait.assert_called_once_with(timeout=10)


if __name__ == '__main__':
    unittest.main()
