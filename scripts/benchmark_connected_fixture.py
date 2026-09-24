#!/usr/bin/env python3
"""Independent evaluator/reset for the connected discount workflow.

Only the named disposable benchmark repositories and two known edits are owned.
Never reset unknown edits or remove untracked/user files. Run outside tool timing.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

sys.dont_write_bytecode = True
BASE = '49f793fd66b054a38923c3f6e3ccd9c1aca6579f'
EXPECTED_DIFF = '69d104a260d4803da81676ce24ac8067533e622ab71625131b49c8d86682fe63'
PATHS = ['pricing/discount.py', 'tests/test_checkout.py']
REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_BENCHMARK_PARENT = REPO_ROOT.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('provider', choices=['chadex', 'webcodex'])
    parser.add_argument('action', choices=['reset', 'evaluate', 'cycle'])
    parser.add_argument('--root', type=Path, default=DEFAULT_BENCHMARK_PARENT)
    args = parser.parse_args()
    project = args.root / ('Benchmark-Chadex' if args.provider == 'chadex' else 'Benchmark-WebCodex')

    def git(*argv):
        return subprocess.check_output(['git', *argv], cwd=project, timeout=30)

    if git('rev-parse', 'HEAD').decode().strip() != BASE:
        raise RuntimeError('benchmark HEAD differs from baseline')
    if git('diff', '--cached', '--name-only'):
        raise RuntimeError('refusing staged changes')
    changed = git('diff', '--name-only').decode().splitlines()
    diff_hash = hashlib.sha256(git('diff', '--', *PATHS)).hexdigest()
    if changed and (sorted(changed) != sorted(PATHS) or diff_hash != EXPECTED_DIFF):
        raise RuntimeError('refusing to reset unknown edits')
    if args.action != 'reset':
        if not changed:
            raise RuntimeError('missing expected implementation')
        sys.path.insert(0, str(project))
        from pricing.discount import DiscountPolicy
        checks = 0
        for amount in [0, 0.01, 1, 25, 100, 9999.99]:
            for discount in [0, 0.01, 1, 25, 40, 100, 10000]:
                observed = DiscountPolicy().apply_fixed(amount, discount)
                if observed != round(max(0, amount - discount), 2):
                    raise RuntimeError('business-rule evaluator failed')
                checks += 1
        try:
            DiscountPolicy().apply_fixed(10, -1)
        except ValueError:
            checks += 1
        else:
            raise RuntimeError('negative discount accepted')
        print(json.dumps({'diff_sha256': diff_hash, 'business_checks': checks, 'changed_files': changed}))
    if args.action in ('reset', 'cycle'):
        git('restore', '--source=' + BASE, '--worktree', '--', *PATHS)


if __name__ == '__main__':
    main()
