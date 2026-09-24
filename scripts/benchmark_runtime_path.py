#!/usr/bin/env python3
"""AB/BA five-step workflow against two isolated real Server/Runner bundles.

Local MCP only: excludes tunnel, connector approval and model reasoning. Both
variants use the same guarded reads and mutation, baseline and hidden evaluator.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import time

sys.dont_write_bytecode = True
os.environ['PYTHONDONTWRITEBYTECODE'] = '1'
from benchmark_cold_runtime import IsolatedHelper, timing_summary
from benchmark_workflow import runtime_identity, parse_env_file
from benchmark_turn_economy import MeasuredClient
from benchmark_phase9_e2e import baseline_workflow, prepare_checkout, hidden_evaluator, diff_sha256, run

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_BENCHMARK = REPO_ROOT.parent / "Benchmark-Chadex"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', type=Path, required=True)
    parser.add_argument('--after', type=Path, required=True)
    parser.add_argument('--source', type=Path, default=DEFAULT_BENCHMARK)
    parser.add_argument('--iterations', type=int, default=20)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.iterations < 1:
        parser.error('iterations must be positive')
    baseline = '49f793fd66b054a38923c3f6e3ccd9c1aca6579f'
    rows, identities, variants = [], {}, {}
    with tempfile.TemporaryDirectory(prefix='chadex-path-audit-') as directory:
        root = Path(directory)
        try:
            for label, app in [('before', args.before), ('after', args.after)]:
                project = root / label / 'project'
                project.parent.mkdir()
                prepare_checkout(args.source, baseline, project)
                contents = app.resolve() / 'Contents'
                resources = contents / 'Resources'
                helper_binary = contents / 'Helpers/chadex-helper'
                identities[label] = {str(p.relative_to(contents)): hashlib.sha256(p.read_bytes()).hexdigest() for p in [helper_binary, *[resources / 'chadex-runtime' / name for name in ['chadex-runtime-cli', 'chadex-runtime-server', 'chadex-runtime-runner']]]}
                helper = IsolatedHelper(helper_binary, resources, root / label / 'data')
                variants[label] = (helper, None, project, None)
                helper.request('activateProject', {'path':str(project)})
                helper.request('configureLocalSetup')
                url, env, pid = runtime_identity(root / label / 'data')
                client = MeasuredClient(url, parse_env_file(env)['WEBCODEX_TOKEN'])
                variants[label] = (helper, client, project, pid)
            for iteration in range(args.iterations + 1):
                for label in (['before','after'] if iteration % 2 == 0 else ['after','before']):
                    helper, client, project, pid = variants[label]
                    run(['git','restore','--source='+baseline,'--worktree','--','pricing/discount.py','tests/test_checkout.py'],cwd=project)
                    offset = len(client.samples)
                    start = time.perf_counter()
                    result = baseline_workflow(client, pid)
                    ms = (time.perf_counter()-start)*1000
                    evaluator = hidden_evaluator(project, baseline)
                    evaluator['diff_sha256'] = diff_sha256(project, baseline)
                    if not evaluator['passed']:
                        raise RuntimeError(f'{label} evaluator failed: {evaluator}')
                    client.invoke('runtime_status', {'summary_only':True})
                    if iteration:
                        rows.append({'variant':label,'iteration':iteration,'workflow_ms':ms,'steps':client.samples[offset:offset+5], 'runtime_status_ms':client.samples[-1]['total_ms'],'workflow_result':result,'evaluator':evaluator})
        finally:
            for helper, client, _, _ in variants.values():
                if client: client.close()
                helper.close()
    if len({r['evaluator']['diff_sha256'] for r in rows}) != 1:
        raise RuntimeError('variant diffs differ')
    summary = {label:{'workflow_ms':timing_summary([r['workflow_ms'] for r in rows if r['variant']==label]),'runtime_status_ms':timing_summary([r['runtime_status_ms'] for r in rows if r['variant']==label]), 'tools_ms':{tool:timing_summary([s['total_ms'] for r in rows if r['variant']==label for s in r['steps'] if s['tool']==tool]) for tool in ['search_project_texts','read_files','apply_text_edits','run_process','show_changes']}} for label in variants}
    report = {'surface':'isolated loopback MCP, not connected workflow','baseline':baseline,'one_warmup':True,'binary_sha256':identities,'summary':summary,'rows':rows}
    args.output.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(summary,indent=2))


if __name__ == '__main__':
    main()
