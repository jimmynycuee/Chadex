#!/usr/bin/env python3
"""One bounded actual Codex-product Goal; never an engineering subagent.

Uses the configured default provider, one iteration and a 90-second agent limit.
Only a synthetic isolated Git fixture is writable. A quota/auth/provider failure
is recorded honestly; this script never retries it or substitutes a fake agent.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ('helper','resources','output'):parser.add_argument('--'+name,required=True)
    args=parser.parse_args()
    if os.environ.get('CHADEX_GOAL_AGENT_ARGV') or os.environ.get('CHADEX_GOAL_CHECK_SANDBOX_ARGV'):
        raise RuntimeError('Actual provider smoke refuses agent/sandbox overrides')
    saved=sys.argv;sys.argv=[__file__,args.helper,args.resources,args.output,'codex']
    spec=importlib.util.spec_from_file_location('goal_harness',Path(__file__).with_name('e2e_plan_goal.py'))
    harness=importlib.util.module_from_spec(spec);spec.loader.exec_module(harness);sys.argv=saved
    root=Path(tempfile.mkdtemp(prefix='chadex-actual-codex-goal-')).resolve()
    project=root/'fixture';project.mkdir();data=root/'data'
    (project/'README.md').write_text('Synthetic fixture. Only codex-goal-smoke.txt may be created.\n')
    for command in (['init','-q','-b','main'],['add','README.md'],['-c','user.name=fixture','-c','user.email=fixture@local','commit','-qm','synthetic fixture']):
        subprocess.run(['git','-C',str(project),*command],check=True)
    original=harness.git(project,'rev-parse','HEAD')
    report={'kind':'actual-provider-product-test-not-delegation-or-human-approval','root':str(root),'passed':False}
    out=Path(args.output);helper=harness.Helper(data,{})
    run=None
    try:
        helper.request('activateProject',{'path':str(project)})
        helper.request('configureLocalSetup')
        env,_=harness.find_runtime(data);url=env.get('WEBCODEX_PUBLIC_URL')
        for cfg in data.rglob('runner.toml'):
            for line in cfg.read_text().splitlines():
                if line.startswith('server_url'):url=line.split('=',1)[1].strip().strip('"')
        client=harness.ChatGPT(url,env['WEBCODEX_TOKEN'])
        ok,projects=client.call('list_projects',{})
        if not ok:raise RuntimeError('Synthetic Runner unavailable')
        project_id=next(p.get('id') or p.get('project') for p in projects['projects'] if (p.get('path') or p.get('root'))==str(project))
        ok,request=client.call('goal_run_start',{'project':project_id,'title':'Actual provider single-iteration smoke','objective':'In this synthetic fixture only, create codex-goal-smoke.txt containing exactly repaired followed by a newline. Do not edit README.md, tests, configuration or other files. Do not commit; Chadex checkpoints the result.','acceptance':[{'name':'independent exact marker','argv':['/usr/bin/grep','-qx','repaired','codex-goal-smoke.txt']}],'budget':{'max_iterations':1,'max_minutes':3,'agent_timeout_secs':90}})
        if not ok:raise RuntimeError('Synthetic Goal request rejected')
        ok,approved=harness.operator(helper,project,'plan_review',{'action':'approve_and_start','plan_id':request['plan']['plan_id'],'revision':1,'content_sha256':request['revision']['content_sha256'],'mode':'codex'})
        if not ok:raise RuntimeError('Synthetic local operator route rejected request')
        run=approved['goal_run']['goal_run_id'];report['goal_run_id']=run
        until=time.monotonic()+210
        while time.monotonic()<until:
            ok,current=client.call('goal_run_status',{'goal_run_id':run,'wait_secs':5})
            if not ok:raise RuntimeError('Synthetic Goal status unavailable')
            report['result']=current
            if current['goal_run']['state'] not in ('queued','running'):break
        final=report['result'];workspace=Path(final['goal_run'].get('workspace_path') or root/'absent')
        marker=workspace/'codex-goal-smoke.txt'
        report['original_source_untouched']=harness.git(project,'rev-parse','HEAD')==original and not harness.git(project,'status','--porcelain')
        report['changed_files']=harness.git(workspace,'diff','--name-only',original,'HEAD').splitlines() if workspace.is_dir() else []
        report['actual_agent_dispatch_count']=sum(op.get('kind')=='agent' for op in final.get('operations',[]))
        report['passed']=final['goal_run']['state']=='succeeded' and marker.is_file() and marker.read_text()=='repaired\n' and report['original_source_untouched'] and report['actual_agent_dispatch_count']==1 and report['changed_files']==['codex-goal-smoke.txt']
        print('Actual Codex product Goal:', 'PASS' if report['passed'] else 'NOT PASSED',final['goal_run']['state'],final['goal_run'].get('state_reason'),flush=True)
    finally:
        if run:
            try:
                ok,current=client.call('goal_run_status',{'goal_run_id':run,'wait_secs':0})
                if not ok:raise RuntimeError('Cannot confirm fixture state')
                if current['goal_run']['state'] in ('queued','running'):
                    harness.operator(helper,project,'goal_operator',{'action':'cancel','goal_run_id':run})
                    for _ in range(30):
                        ok,current=client.call('goal_run_status',{'goal_run_id':run,'wait_secs':1})
                        if ok and current['goal_run']['state'] not in ('queued','running'):break
                    else:report['cleanup_blocked']='Synthetic Goal cancellation not confirmed'
            except Exception as error:report['cleanup_blocked']=type(error).__name__
        helper.close();out.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
    return 0 if report['passed'] else 2


if __name__=='__main__':raise SystemExit(main())
