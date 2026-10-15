#!/usr/bin/env python3
"""Actual private Docker sandbox fixture; no pull/install/production services."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import uuid

HERE=Path(__file__).resolve().parent
REPO=HERE.parents[2]

def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

class Invocation(list):
    def __init__(self, raw):
        fields = raw.split(b'\0')[:-1]
        assert fields[0] == b'chadex-process-v2'
        argc = int(fields[2]); assert len(fields) == argc + 5
        super().__init__(field.decode() for field in fields[3:4+argc])
        self.stdin = fields[-1].decode() if fields[1] == b'1' else None


def main():
    ap=argparse.ArgumentParser()
    ap.add_argument("--rustc",required=True)
    ap.add_argument("--docker",required=True)
    ap.add_argument("--endpoint",required=True)
    ap.add_argument("--image-id",required=True,help="Actual approved LOCAL sha256 ID; no tags/pull")
    ap.add_argument("--node-modules",required=True,help="Existing dependency root, read/copy-only")
    a=ap.parse_args()
    root=Path(tempfile.mkdtemp(prefix="chadex-goal-docker-",dir="/private/tmp")).resolve()
    w=root/"worktree";w.mkdir()
    (w/".goal-acceptance").mkdir()
    (w/"fixtures").mkdir()
    protected=w/"fixtures/approved.json";protected.write_text('{"marker":"SYNTHETIC_APPROVED_ONLY"}\n')
    protected.chmod(0o444)
    commit=subprocess.check_output(["git","-C",str(REPO),"rev-parse","HEAD"],text=True).strip()
    name="chadex-goal-"+str(uuid.uuid4())
    uid=os.getuid();gid=os.getgid()
    facts={"CLI":a.docker,"ENDPOINT":a.endpoint,"IMAGE":a.image_id,"GOAL":"wc_grun_fixture_private",
           "COMMIT":commit,"NAME":name,"UID":str(uid),"GID":str(gid)}
    env=os.environ.copy();env.update({"FIXTURE_DOCKER_"+k:v for k,v in facts.items()})
    driver=root/"driver"
    report={"kind":"local-private-docker-smoke-not-goal-receipt","module_sha256":sha(REPO/"runtime-engine/src/tool_runtime/goal_acceptance_sandbox.rs"),
            "source_commit":commit,"backend":facts,"root":str(root),"commands":[],"controls":[],
            "worker_d1":{"status":"NOT_VALIDATED"}}
    evidence=root/"report.json"
    def save():evidence.write_text(json.dumps(report,ensure_ascii=False,indent=2)+"\n")
    def record(argv,timeout=25):
        start=time.time_ns()
        try:
            p=subprocess.run(argv,cwd=w,capture_output=True,text=True,timeout=timeout,input=getattr(argv,"stdin",None))
            r={"argv":argv,"exit_code":p.returncode,"stdout":p.stdout[-48000:],"stderr":p.stderr[-24000:]}
        except subprocess.TimeoutExpired as e:
            r={"argv":argv,"exit_code":None,"timed_out":True,"stdout":str(e.stdout or "")[-48000:],"stderr":str(e.stderr or "")[-24000:]}
        r.update(started_unix_ns=start,finished_unix_ns=time.time_ns())
        report["commands"].append(r);save();return r
    def check(label,okay):
        report["controls"].append({"name":label,"passed":bool(okay)});save()
        print(label+(": PASS" if okay else ": FAIL"),flush=True)
        if not okay:raise RuntimeError(label+" failed; "+str(evidence))
    p=record([a.rustc,"--edition","2021","--cfg","test",str(HERE/"driver.rs"),"-o",str(driver)])
    check("standalone_driver_compile",p["exit_code"]==0)
    def command(mode,argv=(),readonly=()):
        p=subprocess.run([str(driver),mode,str(w),"-",*map(str,readonly),"--",*argv],env=env,capture_output=True)
        if p.returncode:raise RuntimeError(p.stderr.decode())
        return Invocation(p.stdout)
    version=record(command("docker-version"))
    if version["exit_code"]!=0:
        report["worker_d1"]={"status":"BLOCKED","reason":"Runner/local daemon unavailable; no Docker execution PASS"}
        save();print("BLOCKED daemon; evidence",evidence,flush=True);return 2
    server=json.loads(version["stdout"]);check("actual_linux_daemon",server["Os"]=="linux")
    inspected=record(command("docker-image"))
    if inspected["exit_code"]!=0:
        report["worker_d1"]={"status":"BLOCKED","reason":"approved pinned image absent locally; --pull never"}
        save();print("BLOCKED local image; evidence",evidence,flush=True);return 2
    image=json.loads(inspected["stdout"])
    check("local_image_pin_and_safe_config",image["Id"]==a.image_id and image["Os"]=="linux" and not image["Config"].get("Volumes"))
    report["image_platform"]={"os":image["Os"],"architecture":image["Architecture"]}
    listener=socket.socket();listener.bind(("127.0.0.1",0));listener.listen()
    port=listener.getsockname()[1];hits=[]
    def serve():
        while True:
            try:c,address=listener.accept()
            except OSError:return
            hits.append(address);c.close()
    threading.Thread(target=serve,daemon=True).start()
    code=r"""
const fs=require('node:fs'),net=require('node:net');
const checks={};
function denied(name,fn){try{fn();checks[name]={passed:false}}catch(e){checks[name]={passed:true,errno:e.code}}}
denied('rootfs_write',()=>fs.writeFileSync('/etc/chadex_probe','forbidden'));
denied('host_home_read',()=>fs.readFileSync('/Users/example/.codex/RTK.md'));
checks.docker_socket_absent={passed:!fs.existsSync('/var/run/docker.sock')};
checks.env_sanitized={passed:!process.env.CLOUDFLARE_API_TOKEN&&!process.env.DOCKER_HOST&&!process.env.NODE_OPTIONS};
denied('whole_worktree_write',()=>fs.writeFileSync('/workspace/allowed','FORBIDDEN'));
fs.writeFileSync('/workspace/.goal-acceptance/cache/allowed','CACHE_ONLY');
checks.cache_tmp_write={passed:fs.readFileSync('/workspace/.goal-acceptance/cache/allowed','utf8')==='CACHE_ONLY'};
const protected='/workspace/fixtures/approved.json';
denied('readonly_write',()=>fs.writeFileSync(protected,'corrupt'));
denied('readonly_chmod',()=>fs.chmodSync(protected,0o666));
denied('readonly_unlink',()=>fs.unlinkSync(protected));
denied('readonly_rename',()=>fs.renameSync(protected,'/workspace/alias'));
denied('readonly_hardlink',()=>fs.linkSync(protected,'/workspace/hardlink'));
function connect(host,port){return new Promise(resolve=>{
 const s=net.connect({host,port});s.on('connect',()=>{s.destroy();resolve('ALLOWED')});
 s.on('error',e=>resolve(e.code));s.setTimeout(1500,()=>{s.destroy();resolve('UNKNOWN_TIMEOUT')});
})}
(async()=>{
 const s=net.createServer(c=>{c.on('error',()=>{});c.end('PRIVATE_ONLY')});
 await new Promise((r,j)=>{s.on('error',j);s.listen(0,'127.0.0.1',r)});
 checks.private_loopback={passed:(await connect('127.0.0.1',s.address().port))==='ALLOWED'};
 const hostResult=await connect('127.0.0.1',Number(process.argv[1]));
 checks.host_loopback_unreachable={passed:hostResult==='ECONNREFUSED',errno:hostResult};
 const outbound=await connect('192.0.2.1',443);
 checks.outbound_unreachable={passed:['ENETUNREACH','EHOSTUNREACH'].includes(outbound),errno:outbound};
 s.close();
 console.log('DOCKER_SANDBOX_CONTROLS='+JSON.stringify(checks));
 await new Promise(r=>setTimeout(r,5000));
 if(!Object.values(checks).every(c=>c.passed))process.exitCode=1;
})().catch(e=>{console.error(e.stack);process.exitCode=1});
"""
    def inspect_facts(container):
        labels=container["Config"]["Labels"]
        return [container["Id"],container["Created"],container["Name"],container["Image"],
                labels.get("io.chadex.goal_run_id",""),labels.get("io.chadex.commit_sha","")]
    def create(payload,readonly=()):
        p=record(command("docker-create",payload,readonly))
        check("create_actual_success_no_payload_execution",p["exit_code"]==0)
        actual_id=p["stdout"].strip()
        check("create_stdout_full_actual_id",len(actual_id)==64 and all(c in "0123456789abcdef" for c in actual_id))
        inspected=record(command("docker-inspect",[actual_id]))
        check("initial_inspect_success",inspected["exit_code"]==0)
        c=json.loads(inspected["stdout"])
        check("created_not_started_before_journal",c["Id"]==actual_id and c["State"]["Status"]=="created")
        identity=[c["Id"],c["Created"]]
        report.setdefault("fixture_journal_before_start",[]).append({
            "actual_id":identity[0],"created":identity[1],"name":c["Name"],"image_id":c["Image"],
            "goal":inspect_facts(c)[4],"commit":inspect_facts(c)[5],"journal_unix_ns":time.time_ns(),
            "not_a_goal_db_receipt":True})
        save()
        return identity,c
    def cleanup(identity):
        p=record(command("docker-inspect",[identity[0]]))
        if p["exit_code"]!=0:
            raise RuntimeError("unknown inspect ownership; preserve container "+identity[0])
        c=json.loads(p["stdout"])
        # Rust helper rejects mismatched ID, Created, name, image or labels.
        p=record(command("docker-cleanup",[*identity,*inspect_facts(c)]))
        check("verified_created_id_label_cleanup",p["exit_code"]==0)
        p=record(command("docker-inspect",[identity[0]]))
        check("actual_owned_container_absent_after_cleanup",p["exit_code"]!=0)
    identity,container=create(["/usr/local/bin/node","-e",code,str(port)],[protected])
    hc=container["HostConfig"]
    check("actual_container_namespace_and_privileges",
          hc["NetworkMode"]=="none" and hc["ReadonlyRootfs"] and not hc["Privileged"]
          and hc.get("CapDrop")==["ALL"] and "no-new-privileges" in " ".join(hc.get("SecurityOpt") or [])
          and not hc.get("PortBindings") and hc.get("PidMode","")=="" and hc["IpcMode"]=="private")
    check("actual_container_identity_commit_user",
          container["Config"]["Labels"]["io.chadex.commit_sha"]==commit
          and container["Config"]["Labels"]["io.chadex.goal_run_id"]==facts["GOAL"]
          and container["Config"]["User"]==f"{uid}:{gid}")
    check("actual_mounts_whole_worktree_readonly_no_home_socket",
          all(m.get("Source","").startswith(str(w)) or m.get("Type")=="tmpfs" for m in container["Mounts"])
          and any(m.get("Destination")=="/workspace" and m.get("RW") is False for m in container["Mounts"]))
    report["actual_container_id"]=identity[0]
    argv=command("docker-start",identity);start=time.time_ns()
    child=subprocess.Popen(argv,cwd=w,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    try:
        stdout,stderr=child.communicate(timeout=20)
        p={"argv":argv,"exit_code":child.returncode,"stdout":stdout,"stderr":stderr,
           "started_unix_ns":start,"finished_unix_ns":time.time_ns()}
        report["commands"].append(p);save()
        waited=record(command("docker-wait",identity))
        check("actual_daemon_wait_exit_zero",waited["exit_code"]==0 and waited["stdout"].strip()=="0")
        line=next((s for s in stdout.splitlines() if s.startswith("DOCKER_SANDBOX_CONTROLS=")),None)
        check("actual_child_controls",line is not None)
        report["child_controls"]=json.loads(line.split("=",1)[1])
        check("all_child_controls_pass",all(c["passed"] for c in report["child_controls"].values()))
        check("host_listener_received_no_connections",not hits)
        check("host_workspace_unmodified_and_readonly_copy_intact",not (w/"allowed").exists()
              and json.loads(protected.read_text())["marker"]=="SYNTHETIC_APPROVED_ONLY")
    finally:
        listener.close()
        if child.poll() is None:child.terminate()
        cleanup(identity)
        if child.poll() is None:child.kill()
        child.wait()
    # Real external deadline: kill attach CLI, then fresh immutable-identity
    # inspect + exact-ID cleanup. No name-based rm, cidfile, or fake daemon exit.
    identity,c=create(["/bin/sh","-c","sleep 60"])
    argv=command("docker-start",identity)
    timed=subprocess.Popen(argv,cwd=w,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
    time.sleep(1)
    inspected=record(command("docker-inspect",[identity[0]]))
    check("deadline_fixture_actual_running",inspected["exit_code"]==0 and json.loads(inspected["stdout"])["State"]["Running"])
    timed.terminate()
    try:timed.communicate(timeout=3)
    except subprocess.TimeoutExpired:timed.kill();timed.communicate()
    cleanup(identity)
    report["external_deadline_cleanup"]={"actual_id":identity[0],"created":identity[1],"verified_cleanup":True}
    # Copy exact existing deps. Never mutate them or install missing native deps.
    deps=Path(a.node_modules).resolve(strict=True)
    for directory,dirs,files in os.walk(deps,followlinks=False):
        for entry in dirs+files:
            p=Path(directory)/entry
            if p.is_symlink() and not p.resolve().is_relative_to(deps):
                raise RuntimeError("dependency symlink escape: "+str(p))
    shutil.copytree(deps,w/"node_modules",symlinks=True)
    for name in ["worker.mjs","worker_d1.cjs"]:shutil.copy2(HERE/name,w/name)
    report["dependency_provenance"]={rel:sha(deps/rel) for rel in [
        "wrangler/package.json","miniflare/package.json","workerd/lib/main.js"]}
    # network=none permits ONLY private container localhost, including internal
    # ephemeral workerd/Miniflare ports; cannot reach host/backend service ports.
    identity,c=create(["/usr/local/bin/node","/workspace/worker_d1.cjs","55001"])
    try:
        p=record(command("docker-start",identity),timeout=20)
        waited=record(command("docker-wait",identity),timeout=10)
        okay=waited["exit_code"]==0 and waited["stdout"].strip()=="0" and "LOCAL_WORKER_D1_REAL_READBACK" in p["stdout"]
    finally:
        cleanup(identity)
    report["worker_d1"]={"status":"PASS" if okay else "BLOCKED",
        "command_index":report["commands"].index(p),
        "reason":None if okay else "actual copied deps/runtime failed; no Worker->D1 PASS",
        "worker_source_sha256":sha(w/"worker.mjs")}
    save();print("Worker/D1:",report["worker_d1"]["status"],flush=True)
    print("evidence:",evidence,flush=True)
    return 0 if okay else 2

if __name__=="__main__":
    try:raise SystemExit(main())
    except (RuntimeError,OSError,subprocess.SubprocessError) as e:
        print("FAILED/BLOCKED:",e,flush=True);raise SystemExit(2)
