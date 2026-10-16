#!/usr/bin/env python3
"""Real isolated helper -> server -> Runner Goal checks, no AI calls/live state.

Operator actions below authorize synthetic fixture Plans through the real local
operator route. They do not constitute human/native UI acceptance or authority
for any production Goal. No signing key/owner evidence is manufactured.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from goal_dependency_manifest import manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--helper', required=True)
    parser.add_argument('--resources', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--docker', required=True)
    parser.add_argument('--endpoint', required=True)
    parser.add_argument('--image-id', required=True)
    parser.add_argument('--deps', required=True)
    args = parser.parse_args()
    saved_argv = sys.argv
    sys.argv = [__file__, args.helper, args.resources, args.output, 'fake']
    spec = importlib.util.spec_from_file_location('plan_goal_harness', Path(__file__).with_name('e2e_plan_goal.py'))
    harness = importlib.util.module_from_spec(spec); spec.loader.exec_module(harness)
    sys.argv = saved_argv
    root = Path(tempfile.mkdtemp(prefix='chadex-goal-reliability-')).resolve()
    project, data = root / 'fixture', root / 'data'
    project.mkdir()
    fixture = Path(__file__).with_name('tests') / 'goal_acceptance_sandbox'
    for name in ('worker.mjs', 'worker_d1.cjs'):
        shutil.copyfile(fixture / name, project / name)
    (project / '.gitignore').write_text('node_modules/\n.goal-acceptance/\n')
    shutil.copyfile(Path(args.deps).parent / 'package-lock.json', project / 'package-lock.json')
    shutil.copytree(args.deps, project / 'node_modules', symlinks=True)
    dependency = manifest(project, 'node_modules', 'package-lock.json')
    for command in (['init','-q','-b','main'],['add','-A'],['-c','user.name=fixture','-c','user.email=fixture@local','commit','-q','-m','synthetic Worker/D1 fixture']):
        subprocess.run(['git','-C',str(project),*command],check=True)
    original_head = harness.git(project,'rev-parse','HEAD')
    policy = {'executable':args.docker,'endpoint':args.endpoint,'allowed_images':[args.image_id],'uid':os.getuid(),'gid':os.getgid()}
    helper = harness.Helper(data, {'CHADEX_GOAL_CONTAINER_POLICY':json.dumps(policy)})
    report = {'kind':'isolated-real-helper-runner-not-human-or-live-goal-proof','root':str(root),'image_id':args.image_id,'checks':[]}
    out = Path(args.output)
    started_runs = []
    def record(name, passed, **details):
        report['checks'].append({'name':name,'passed':bool(passed),**details})
        out.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
        print(name+(': PASS' if passed else ': FAIL'),flush=True)
        if not passed:raise AssertionError(name+'; '+str(out))
    try:
        helper.request('activateProject',{'path':str(project)})
        helper.request('configureLocalSetup')
        env,_ = harness.find_runtime(data)
        url = env.get('WEBCODEX_PUBLIC_URL')
        for cfg in data.rglob('runner.toml'):
            for line in cfg.read_text().splitlines():
                if line.startswith('server_url'):url=line.split('=',1)[1].strip().strip('"')
        client=harness.ChatGPT(url,env['WEBCODEX_TOKEN'])
        ok,projects=client.call('list_projects',{})
        project_id=next((p.get('id') or p.get('project') for p in projects.get('projects',[]) if (p.get('path') or p.get('root'))==str(project)),None)
        record('real_bound_runner_ready',ok and project_id is not None)
        def operator(action,**kwargs):return harness.operator(helper,project,'goal_operator',{'action':action,**kwargs})
        def status(run):
            ok,body=operator('status',goal_run_id=run)
            if not ok:raise RuntimeError('status failed '+json.dumps(body))
            return body
        def wait(run,predicate,timeout=420):
            until=time.monotonic()+timeout
            while time.monotonic()<until:
                body=status(run)
                if predicate(body):return body
                time.sleep(.5)
            raise TimeoutError(json.dumps(body))
        def start(title,checks,dependencies=(),expect_ready=True):
            ok,request=client.call('goal_run_start',{'project':project_id,'title':title,'objective':title,'acceptance':checks,'dependencies':list(dependencies),'budget':{'max_iterations':3,'max_minutes':10}})
            record(title+' request',ok and request.get('revision') is not None)
            plan_id=request['plan']['plan_id']
            ok,result=harness.operator(helper,project,'plan_review',{'action':'approve_and_start','plan_id':plan_id,'revision':1,'content_sha256':request['revision']['content_sha256'],'mode':'chatgpt'})
            record(title+' synthetic operator approval',ok and result.get('goal_run') is not None)
            run=result['goal_run']['goal_run_id']
            started_runs.append(run)
            wait(run,lambda body:body['goal_run'].get('waiting_for_chatgpt') or body['goal_run']['state']=='needs_attention')
            body=status(run)
            record(title+' preflight',body['goal_run']['state']==('running' if expect_ready else 'needs_attention'),state=body['goal_run'],events=body.get('events'))
            return run
        check={'name':'real Worker D1 readback','lane':'loopback_integration','container_image':args.image_id,'argv':['/usr/local/bin/node','/workspace/worker_d1.cjs','55001'],'timeout_secs':60}
        run=start('private Worker D1',[check],[dependency])
        client.call('goal_check',{'goal_run_id':run,'wait_secs':0})
        result=wait(run,lambda body:body['goal_run']['state'] not in ('queued','running'))
        record('worker_d1_goal_achieved',result['goal_run']['state']=='succeeded',goal=result)
        # A real Store row plus success enforcement proves this was a trusted
        # Runner receipt, not a fixture-created JSON marker or exit assumption.
        receipts=[]
        for database in (p for p in data.rglob('*') if p.suffix in ('.db','.sqlite','.sqlite3')):
            with sqlite3.connect(f'file:{database}?mode=ro',uri=True) as conn:
                if not conn.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='wc_goal_acceptance_receipts'").fetchone():continue
                conn.row_factory=sqlite3.Row
                receipts=[dict(row) for row in conn.execute('SELECT goal_run_id,acceptance_revision,checkpoint_sha,check_name,argv_sha256,outcome,exit_code,output_sha256,observed_at_unix_ms,proof_hmac_sha256 FROM wc_goal_acceptance_receipts WHERE goal_run_id=?',(run,))]
        record('runner_signed_receipt_bound_to_checkpoint',len(receipts)==1 and receipts[0]['outcome']=='passed' and receipts[0]['exit_code']==0 and receipts[0]['checkpoint_sha']==result['goal_run']['checkpoint_sha'] and len(receipts[0]['proof_hmac_sha256'])==64,receipts=receipts)
        preparation_check={**check,'name':'repair preflight before any agent work','argv':['/workspace/repair-ready.sh']}
        preparing=start('repair before-work preflight',[preparation_check],expect_ready=False)
        parked=status(preparing); workspace=parked['goal_run']['workspace_project']
        record('before_work_failure_has_no_commit',parked['goal_run']['committed_rounds']==0 and not any(op.get('kind') in ('agent','commit') for op in parked.get('operations',[])))
        ok,denied=client.call('goal_run_control',{'goal_run_id':preparing,'action':'resume'})
        record('model_cannot_resume_needs_attention',not ok,error=denied)
        ok,written=client.call('write_project_file',{'project':workspace,'path':'repair-ready.sh','content':'#!/bin/sh\nexit 0\n'})
        record('repair_synthetic_declared_executable',ok,output=written)
        ok,changed=client.call('run_process',{'project':workspace,'executable':'/bin/chmod','args':['755','repair-ready.sh'],'sync_wait_secs':5})
        record('repair_executable_mode',ok,output=changed)
        ok,resumed=operator('resume',goal_run_id=preparing)
        record('local_operator_retries_preparation_same_goal',ok and resumed['goal_run']['goal_run_id']==preparing)
        wait(preparing,lambda body:body['goal_run'].get('waiting_for_chatgpt') or body['goal_run']['state']=='needs_attention')
        record('preparation_retry_gets_fresh_probe_not_deleted_container_cache',status(preparing)['goal_run']['state']=='running')
        client.call('goal_check',{'goal_run_id':preparing,'wait_secs':0})
        repaired=wait(preparing,lambda body:body['goal_run']['state'] not in ('queued','running'))
        record('repaired_preparation_goal_achieved',repaired['goal_run']['state']=='succeeded',goal=repaired)
        external={'name':'real owner remains pending','kind':'external_gate','lane':'external_attestation','argv':['/bin/sh','-c','exit 2'],'external_gate':{'owner':'fixture has no real authority','action':'must remain blocked','evidence':'missing-authority.json','next_step':'real owner needed'}}
        gate=start('external gate stays blocked',[external])
        client.call('goal_check',{'goal_run_id':gate,'wait_secs':0})
        blocked=wait(gate,lambda body:body['goal_run']['state']=='needs_attention')
        head=blocked['goal_run']['checkpoint_sha']
        record('external_exit_two_keeps_goal_blocked','blocked_external' in blocked['goal_run']['state_reason'],goal=blocked['goal_run'])
        revised=[external,{'name':'independent later phase','argv':['/usr/bin/true']}]
        ok,proposal=operator('propose_acceptance',goal_run_id=gate,expected_acceptance_revision=1,acceptance=revised,workspace_requirements=[],dependencies=[])
        record('immutable_acceptance_proposal',ok and proposal.get('source_acceptance_revision')==1,proposal=proposal)
        denied,body=client.call('goal_operator',{'action':'approve_acceptance','goal_run_id':gate,'proposal_id':proposal['proposal_id'],'expected_acceptance_revision':1,'acceptance_sha256':proposal['proposed_acceptance_sha256']})
        record('model_cannot_grant_local_operator_approval',not denied,error=body)
        ok,bad=operator('approve_acceptance',goal_run_id=gate,proposal_id=proposal['proposal_id'],expected_acceptance_revision=1,acceptance_sha256='0'*64)
        record('stale_or_forged_revision_hash_rejected',not ok,error=bad)
        ok,approved=operator('approve_acceptance',goal_run_id=gate,proposal_id=proposal['proposal_id'],expected_acceptance_revision=1,acceptance_sha256=proposal['proposed_acceptance_sha256'])
        record('exact_revision_hash_approved',ok,approval=approved)
        ok,retry=operator('retry_acceptance',goal_run_id=gate)
        record('human_route_retries_existing_checkpoint',ok,retry=retry)
        retried=wait(gate,lambda body:body['goal_run']['state']=='needs_attention')
        commits=[op for op in retried.get('operations',retried.get('ops',[])) if op.get('op_key','').startswith('commit:')]
        record('same_goal_checkpoint_not_recommitted',retried['goal_run']['checkpoint_sha']==head and retried['goal_run']['acceptance_revision']==2 and len(commits)==1,goal=retried,commit_count=len(commits))
        record('formal_gate_still_blocks_after_independent_check','blocked_external' in retried['goal_run']['state_reason'])
        long_check={**check,'name':'cancel bounded private container','argv':['/usr/local/bin/node','-e','setInterval(()=>{},1000)'],'timeout_secs':60}
        cancel=start('cancel isolated container',[long_check])
        client.call('goal_check',{'goal_run_id':cancel,'wait_secs':0})
        running=wait(cancel,lambda body:any(op.get('kind')=='container_start' and op.get('state')=='dispatched' and op.get('op_key','').endswith(':verify:start') for op in body.get('operations',body.get('ops',[]))))
        owned=next(e['detail'] for e in reversed(running.get('events',[])) if e.get('kind')=='container_owned' and e.get('detail',{}).get('key','').endswith(':verify'))
        ok,_=operator('cancel',goal_run_id=cancel)
        final=wait(cancel,lambda body:body['goal_run']['state']=='cancelled')
        cleaned=wait(cancel,lambda body:any(e.get('kind')=='container_cleaned' and e.get('detail',{}).get('id')==owned['id'] for e in body.get('events',[])))
        record('cancel_cleans_exact_owned_container',ok and cleaned['goal_run']['state']=='cancelled',id=owned['id'],created=owned['created'])
        record('original_fixture_project_untouched',harness.git(project,'rev-parse','HEAD')==original_head and harness.git(project,'status','--porcelain','--untracked-files=no')=='')
    finally:
        # Cleanup of our synthetic runs is not a substitute for a passed check.
        # Keep failure evidence and only cancel an actually active fixture run.
        if 'operator' in locals():
            for owned_run in started_runs:
                try:
                    current=status(owned_run)['goal_run']['state']
                    if current in ('queued','running'):
                        operator('cancel',goal_run_id=owned_run)
                        wait(owned_run,lambda body:body['goal_run']['state'] not in ('queued','running'),timeout=25)
                except Exception as error:
                    report.setdefault('cleanup_blockers',[]).append({'run':owned_run,'error':type(error).__name__})
        helper.close()
        out.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
    print('evidence:',out,flush=True)


if __name__=='__main__':main()
