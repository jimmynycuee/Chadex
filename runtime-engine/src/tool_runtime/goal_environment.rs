//! Runner-executed, bounded Goal environment probes. No project code runs here.
use serde_json::{json, Value};
use super::goal_supervisor::{ExecError, ProcessSpec, ProcessOutcome};

pub(super) const PROBE: &str = r#"
import json, os, pathlib, shutil, subprocess, sys, stat
root = pathlib.Path(sys.argv[1]).resolve(strict=True)
agent = sys.argv[2]
checks = json.loads(sys.argv[3])
def resolve(name):
    candidates = ([name] if os.path.isabs(name) else
        [shutil.which(name), str(pathlib.Path.home()/'.local/bin'/name),
         '/opt/homebrew/bin/'+name, '/usr/local/bin/'+name, '/usr/bin/'+name])
    for candidate in candidates:
        if candidate and os.path.isfile(candidate) and os.access(candidate, os.X_OK):
            return str(pathlib.Path(candidate).resolve())
    return None
sandbox = resolve('sandbox-exec') if sys.platform == 'darwin' else resolve('codex')
agent_path = resolve(agent) if agent else None
result = {'workspace': str(root), 'sandbox_executable': sandbox,
          'agent_executable': agent_path, 'python_executable': sys.executable,
          'platform': sys.platform, 'ready': bool(sandbox and (not agent or agent_path))}
if sandbox and sys.platform == 'darwin':
    p = subprocess.run([sandbox, '-p', '(version 1)(deny default)(allow process-exec (literal "/usr/bin/true"))(allow file-read*)(allow sysctl-read)', '/usr/bin/true'],
        cwd=str(root), capture_output=True, timeout=10, env={'PATH':'/usr/bin:/bin'})
    result['sandbox_probe_exit'] = p.returncode
    result['ready'] = result['ready'] and p.returncode == 0
if agent_path and agent == 'codex':
    p = subprocess.run([agent_path, '--version'], capture_output=True, timeout=10, env={'PATH':'/usr/bin:/bin'})
    result['agent_version'] = p.stdout.decode('utf-8','replace')[:160].strip()
    result['ready'] = result['ready'] and p.returncode == 0
records = []
if any(check.get('container_image') for check in checks):
    unsafe=False
    for directory,dirs,files in os.walk(root,followlinks=False):
        if pathlib.Path(directory)==root:dirs[:]=[name for name in dirs if name not in ('.goal-acceptance','.git')]
        for name in dirs+files:
            path=pathlib.Path(directory)/name;mode=path.lstat().st_mode
            if not (stat.S_ISDIR(mode) or stat.S_ISREG(mode) or stat.S_ISLNK(mode)):unsafe=True
            if stat.S_ISLNK(mode) and not path.resolve().is_relative_to(root):unsafe=True
    if unsafe:
        result['workspace_mount_cause']='WORKSPACE_SOCKET_DEVICE_OR_ESCAPING_SYMLINK'
        result['ready']=False
for check in checks:
    if check.get('container_image'):continue
    argv = list(check['argv'])
    target = argv[0]
    if pathlib.Path(target).name == 'env':
        rest = argv[1:]
        while rest and '=' in rest[0] and not rest[0].startswith('-'):
            rest.pop(0)
        if rest and rest[0].startswith('-'):
            target = None
        else:
            target = rest[0] if rest else None
    if target and ('/' in target and not os.path.isabs(target)):
        resolved = root / (check.get('cwd') or '.') / target
        path = str(resolved.resolve()) if resolved.is_file() and os.access(resolved, os.X_OK) else None
    else:
        path = resolve(target) if target else None
    cwd = (root / (check.get('cwd') or '.')).resolve()
    ready = bool(path and cwd.is_dir() and os.path.commonpath([root,cwd]) == str(root))
    records.append({'check':check['name'], 'executable':target, 'resolved_path':path, 'ready':ready})
result['checks'] = records
result['ready'] = result['ready'] and all(r['ready'] for r in records)
print(json.dumps(result, separators=(',',':')))
"#;

const SCRIPT_BOOTSTRAP:&str="import json,sys; payload=json.load(sys.stdin); sys.argv=['trusted-goal-environment',*payload['args']]; exec(compile(payload['code'],'<trusted-goal-environment>','exec'))";
fn script_spec(python:&str,code:&str,args:Vec<String>,timeout_secs:u32)->ProcessSpec {
    ProcessSpec{executable:python.into(),args:vec!["-I".into(),"-S".into(),"-c".into(),SCRIPT_BOOTSTRAP.into()],
        stdin:Some(json!({"code":code,"args":args}).to_string()),cwd:None,timeout_secs}
}
fn probe_python()->String {std::env::var("CHADEX_GOAL_PREFLIGHT_PYTHON").unwrap_or_else(|_|"/usr/bin/python3".into())}
pub(super) fn probe_spec(workspace:&str,agent:&str,checks:Value)->ProcessSpec {
    // Only capability metadata crosses this bounded channel, not whole commands
    // or environment assignment values. Actual acceptance argv remains unchanged.
    let checks:Vec<_>=checks.as_array().into_iter().flatten().map(|check|{
        let argv=check["argv"].as_array();
        let first=argv.and_then(|a|a.first()).and_then(Value::as_str).unwrap_or("");
        let target=if std::path::Path::new(first).file_name().and_then(|n|n.to_str())==Some("env") {
            argv.and_then(|a|a.iter().skip(1).filter_map(Value::as_str).find(|a|!a.contains('='))).unwrap_or("")
        }else{first};
        json!({"name":check["name"],"argv":[target],"cwd":check["cwd"],"container_image":check["container_image"]})
    }).collect();
    script_spec(&probe_python(),PROBE,vec![workspace.into(),agent.into(),json!(checks).to_string()],30)
}

pub(super) fn parse_probe(outcome: ProcessOutcome) -> Result<Value, ExecError> {
    if outcome.exit_code != Some(0) || outcome.timed_out {
        return Err(ExecError::Denied("TOOLCHAIN_PREFLIGHT_FAILED: install a supported local sandbox and Python toolchain; no Goal code was executed".into()));
    }
    let value: Value = serde_json::from_str(outcome.stdout_tail.trim()).map_err(|_| ExecError::Denied("TOOLCHAIN_PREFLIGHT_INVALID: Runner returned no bounded capability record".into()))?;
    if value.get("ready") != Some(&json!(true)) {
        if value.get("workspace_mount_cause").is_some() {return Err(ExecError::Denied("WORKSPACE_MOUNT_UNSAFE: remove host sockets/devices or escaping symlinks; no container payload was started".into()))}
        let missing = value.get("checks").and_then(Value::as_array).and_then(|checks| checks.iter().find(|check| check.get("ready") != Some(&json!(true))));
        return Err(ExecError::Denied(format!("EXECUTABLE_MISSING_OR_SANDBOX_UNAVAILABLE: blocking check {:?}; configure the Runner toolchain; no unsafe fallback is allowed", missing.map(|check| (&check["check"], &check["executable"])))));
    }
    Ok(value)
}

/// This fixed script runs only for an immutable, human-approved manifest.
/// The source root comes from the stored Runner/project binding, never the agent.
pub(super) const PREPARE_INPUTS: &str = r#"
import hashlib,json,os,pathlib,sys,secrets
source=pathlib.Path(sys.argv[1]).resolve(strict=True)
workspace=pathlib.Path(sys.argv[2]).resolve(strict=True)
manifest=json.loads(sys.argv[3])
requirements=manifest.get('workspace_requirements',[])
flags=os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW
source_fd=os.open(str(source),flags)
workspace_fd=os.open(str(workspace),flags)
def parent(root_fd,relative,create=False):
    p=pathlib.Path(relative)
    if p.is_absolute() or '..' in p.parts or not p.parts or '.git' in p.parts:
        raise ValueError('UNSAFE_WORKSPACE_PATH')
    fd=os.dup(root_fd)
    try:
        for part in p.parts[:-1]:
            if create:
                try: os.mkdir(part,0o700,dir_fd=fd)
                except FileExistsError: pass
            next_fd=os.open(part,flags,dir_fd=fd)
            os.close(fd)
            fd=next_fd
        return fd,p.parts[-1]
    except Exception:
        os.close(fd)
        raise

def digest_fd(fd):
    os.lseek(fd,0,os.SEEK_SET)
    h=hashlib.sha256()
    while True:
        chunk=os.read(fd,1024*1024)
        if not chunk:break
        h.update(chunk)
    os.lseek(fd,0,os.SEEK_SET)
    return h.hexdigest()
entries=[]
missing=[]
for requirement in requirements:
    if requirement['permission']!='read_only':raise ValueError('OWNER_ACTION_REQUIRED')
    try:
        fd,name=parent(source_fd,requirement['source_path'])
        try: src=os.open(name,os.O_RDONLY|os.O_NOFOLLOW,dir_fd=fd)
        finally:os.close(fd)
    except FileNotFoundError:
        missing.append({'path':requirement['source_path'],'cause':'MISSING_PRIVATE_FIXTURE'})
        continue
    st=os.fstat(src)
    if not __import__('stat').S_ISREG(st.st_mode) or st.st_size>64*1024*1024:raise ValueError('WORKSPACE_INPUT_SIZE_OR_TYPE')
    if digest_fd(src)!=requirement['source_sha256']:raise ValueError('WORKSPACE_INPUT_DIGEST_MISMATCH')
    entries.append((src,requirement['destination_path'],requirement['source_sha256']))
# Dependency provenance pins BOTH the lockfile and exact scoped tree. A lockfile
# by itself is not evidence that an existing node_modules actually matches it.
import stat,posixpath,shutil,tempfile
class TreeBudget:
    def __init__(self):self.files=0;self.bytes=0
    def charge(self,size=0):
        self.files+=1;self.bytes+=size
        if self.files>100000 or self.bytes>1024*1024*1024:raise ValueError('DEPENDENCY_TREE_LIMIT')
def tree_digest(directory,copy_to=None):
    digest=hashlib.sha256();budget=TreeBudget()
    def add(record):digest.update((json.dumps(record,separators=(',',':'),ensure_ascii=True)+'\n').encode())
    def walk(fd,prefix,out):
        for name in sorted(os.listdir(fd)):
            if name in ('.','..','.git') or '/' in name:raise ValueError('DEPENDENCY_ENTRY_UNSAFE')
            path=prefix+name;metadata=os.stat(name,dir_fd=fd,follow_symlinks=False);budget.charge()
            if stat.S_ISDIR(metadata.st_mode):
                add([path,'directory'])
                child=os.open(name,flags,dir_fd=fd);target=None
                try:
                    if out is not None:os.mkdir(name,0o700,dir_fd=out);target=os.open(name,flags,dir_fd=out)
                    walk(child,path+'/',target)
                    if target is not None:os.fchmod(target,0o555)
                finally:
                    os.close(child)
                    if target is not None:os.close(target)
            elif stat.S_ISLNK(metadata.st_mode):
                link=os.readlink(name,dir_fd=fd)
                target=posixpath.normpath(posixpath.join(posixpath.dirname(path),link))
                if posixpath.isabs(link) or target=='..' or target.startswith('../'):raise ValueError('DEPENDENCY_SYMLINK_ESCAPE')
                add([path,'symlink',link])
                if out is not None:os.symlink(link,name,dir_fd=out)
            elif stat.S_ISREG(metadata.st_mode):
                src=os.open(name,os.O_RDONLY|os.O_NOFOLLOW,dir_fd=fd);dst=None;h=hashlib.sha256();total=0
                try:
                    if out is not None:dst=os.open(name,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600,dir_fd=out)
                    while True:
                        block=os.read(src,1024*1024)
                        if not block:break
                        budget.charge(len(block));total+=len(block)
                        if total>256*1024*1024:raise ValueError('DEPENDENCY_FILE_LIMIT')
                        h.update(block)
                        if dst is not None:
                            view=memoryview(block)
                            while view:view=view[os.write(dst,view):]
                    mode=os.fstat(src).st_mode & 0o111
                    add([path,'file',h.hexdigest(),mode])
                    if dst is not None:os.fchmod(dst,0o444|mode)
                finally:
                    os.close(src)
                    if dst is not None:os.close(dst)
            else:raise ValueError('DEPENDENCY_PATH_UNSAFE')
    walk(directory,'',copy_to);return digest.hexdigest()
dependency_copies=[]
for dependency in manifest.get('dependencies',[]):
    proof=dependency['provenance'].split(':')
    if len(proof)!=4 or proof[0]!='sha256' or any(len(value)!=64 or any(c not in '0123456789abcdef' for c in value) for value in (proof[1],proof[3])):
        missing.append({'path':dependency['path'],'cause':'DEPENDENCY_PROVENANCE_UNVERIFIED'})
        try:
            fd,name=parent(workspace_fd,dependency['path']);os.stat(name,dir_fd=fd,follow_symlinks=False);os.close(fd)
        except FileNotFoundError:missing.append({'path':dependency['path'],'cause':'DEPENDENCY_MISSING'})
        continue
    fd,name=parent(workspace_fd,proof[2])
    try:
        lock=os.open(name,os.O_RDONLY|os.O_NOFOLLOW,dir_fd=fd)
        try:
            if digest_fd(lock)!=proof[1]:raise ValueError('DEPENDENCY_LOCKFILE_DIGEST_MISMATCH')
        finally:os.close(lock)
    finally:os.close(fd)
    try:
        fd,name=parent(workspace_fd,dependency['path'])
        try:directory=os.open(name,flags,dir_fd=fd)
        finally:os.close(fd)
        try:
            if tree_digest(directory)!=proof[3]:raise ValueError('DEPENDENCY_TREE_DIGEST_MISMATCH')
        finally:os.close(directory)
    except FileNotFoundError:
        try:
            fd,name=parent(source_fd,dependency['path'])
            try:directory=os.open(name,flags,dir_fd=fd)
            finally:os.close(fd)
        except FileNotFoundError:
            missing.append({'path':dependency['path'],'cause':'DEPENDENCY_MISSING'});continue
        if tree_digest(directory)!=proof[3]:os.close(directory);raise ValueError('DEPENDENCY_SOURCE_TREE_DIGEST_MISMATCH')
        dependency_copies.append((dependency['path'],directory,proof[3]))
if missing:
    print(json.dumps({'ready':False,'missing':[dict(item,path=item['path'][:160]) for item in missing[:8]],'missing_count':len(missing)},separators=(',',':')))
    sys.exit(0)
for relative in ['.goal-acceptance/home/.keep','.goal-acceptance/config/.keep','.goal-acceptance/cache/.keep','.goal-acceptance/data/.keep','.goal-acceptance/state/.keep','.goal-acceptance/tmp/.keep','.goal-acceptance/logs/.keep','.goal-acceptance/bin/.keep']:
    fd,name=parent(workspace_fd,relative,True)
    os.close(fd)
for relative,source_directory,expected in dependency_copies:
    temporary=pathlib.Path(tempfile.mkdtemp(prefix='.goal-dependency-prepare-',dir=str(workspace.parent)))
    try:
        temporary_fd=os.open(str(temporary),flags)
        try:
            if tree_digest(source_directory,temporary_fd)!=expected:raise ValueError('DEPENDENCY_COPY_DIGEST_MISMATCH')
        finally:os.close(temporary_fd);os.close(source_directory)
        target_fd,target_name=parent(workspace_fd,relative,True)
        try:
            # Atomic directory rename never replaces a populated target. Exact
            # destination absence is checked again; files/symlinks are rejected.
            try:os.stat(target_name,dir_fd=target_fd,follow_symlinks=False)
            except FileNotFoundError:pass
            else:raise ValueError('DEPENDENCY_DESTINATION_CHANGED')
            import ctypes
            libc=ctypes.CDLL(None,use_errno=True)
            origin=os.open(str(temporary.parent),flags)
            try:
                function=getattr(libc,'renameatx_np' if sys.platform=='darwin' else 'renameat2',None)
                if function is None:raise ValueError('DEPENDENCY_EXCLUSIVE_RENAME_UNAVAILABLE')
                function.argtypes=[ctypes.c_int,ctypes.c_char_p,ctypes.c_int,ctypes.c_char_p,ctypes.c_uint]
                function.restype=ctypes.c_int
                result=function(origin,os.fsencode(temporary.name),target_fd,os.fsencode(target_name),4 if sys.platform=='darwin' else 1)
                if result!=0:raise OSError(ctypes.get_errno(),'DEPENDENCY_DESTINATION_CHANGED')
                installed=os.open(target_name,flags,dir_fd=target_fd)
                try:os.fchmod(installed,0o555)
                finally:os.close(installed)
            finally:os.close(origin)
        finally:os.close(target_fd)
    finally:
        if temporary.exists():
            # Only our newly allocated staging directory; never a source path.
            for directory,dirs,files in os.walk(temporary,followlinks=False):
                os.chmod(directory,0o700)
            shutil.rmtree(temporary)
for src,relative,digest in entries:
    fd,name=parent(workspace_fd,relative,True)
    try:
        try: existing=os.open(name,os.O_RDONLY|os.O_NOFOLLOW,dir_fd=fd)
        except FileNotFoundError:existing=None
        if existing is not None:
            try:
                if os.fstat(existing).st_nlink!=1:raise ValueError('HARDLINK_WORKSPACE_INPUT')
                if digest_fd(existing)!=digest:raise ValueError('WORKSPACE_DESTINATION_CONFLICT')
                os.fchmod(existing,0o444)
            finally:os.close(existing)
            continue
        temporary='.chadex-input-'+secrets.token_hex(12)
        target=os.open(temporary,os.O_RDWR|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600,dir_fd=fd)
        try:
            os.lseek(src,0,os.SEEK_SET)
            while True:
                chunk=os.read(src,1024*1024)
                if not chunk:break
                remaining=memoryview(chunk)
                while remaining:
                    written=os.write(target,remaining)
                    remaining=remaining[written:]
            if digest_fd(target)!=digest:raise ValueError('WORKSPACE_INPUT_CHANGED_DURING_COPY')
            os.fchmod(target,0o444)
            # Never replace an unrelated destination that appeared concurrently.
            os.link(temporary,name,src_dir_fd=fd,dst_dir_fd=fd,follow_symlinks=False)
        finally:
            os.close(target)
            os.unlink(temporary,dir_fd=fd)
    finally:os.close(fd)
for src,relative,digest in entries:os.close(src)
os.close(source_fd)
os.close(workspace_fd)
print(json.dumps({'ready':True,'prepared_inputs':len(entries),'dependencies':len(manifest.get('dependencies',[]))},separators=(',',':')))
"#;

pub(super) fn prepare_inputs_spec(python:&str,source:&str,workspace:&str,manifest:Value)->ProcessSpec {
    // Full bounded dependency trees may need more than a short CLI probe.
    // environment_process still clips this to the original Goal deadline.
    let timeout=if manifest["dependencies"].as_array().is_some_and(|items|!items.is_empty()){300}else{60};
    script_spec(python,PREPARE_INPUTS,vec![source.into(),workspace.into(),manifest.to_string()],timeout)
}
pub(super) fn prepare_input_specs(python:&str,source:&str,workspace:&str,manifest:Value)->Vec<ProcessSpec> {
    let mut specs=Vec::new();let mut pending=json!({"workspace_requirements":[],"dependencies":[]});
    for key in ["workspace_requirements","dependencies"] {
        for entry in manifest[key].as_array().into_iter().flatten() {
            pending[key].as_array_mut().unwrap().push(entry.clone());
            let spec=prepare_inputs_spec(python,source,workspace,pending.clone());
            if spec.stdin.as_ref().is_some_and(|stdin|stdin.len()>64*1024) {
                pending[key].as_array_mut().unwrap().pop();
                specs.push(prepare_inputs_spec(python,source,workspace,pending));
                pending=json!({"workspace_requirements":[],"dependencies":[]});
                pending[key].as_array_mut().unwrap().push(entry.clone());
            }
        }
    }
    specs.push(prepare_inputs_spec(python,source,workspace,pending));specs
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_and_unready_preflights_fail_closed() {
        for output in ["", "{}", "{\"ready\":false}", "PASS"] {
            assert!(parse_probe(ProcessOutcome {exit_code:Some(0),timed_out:false,stdout_tail:output.into(),stderr_tail:String::new()}).is_err());
        }
    }
    #[test]
    fn probe_is_fixed_argv_and_has_no_project_interpolation() {
        let spec = probe_spec("/tmp/a; touch evil", "codex", json!([]));
        assert_eq!(spec.executable, "/usr/bin/python3");
        assert_eq!(&spec.args[..3], &["-I", "-S", "-c"]);
        assert!(!spec.args.iter().any(|arg|arg.contains("touch evil")));
        let payload:Value=serde_json::from_str(spec.stdin.as_deref().unwrap()).unwrap();
        assert_eq!(payload["args"][0], "/tmp/a; touch evil");
    }
}

// Enumerate only tracked/nonignored candidates, then exclude approved input
// prefixes as literal bytes. Passing ignored names as negative add pathspecs
// still returns exit 1 on supported Git versions. Never force-add private data.
const STAGE_CHECKPOINT:&str=r#"
import json,os,subprocess,sys
root,executable=sys.argv[1:3]
prefix=json.loads(sys.argv[3]);protected=[os.fsencode(path.rstrip('/')) for path in json.loads(sys.argv[4])]
git=[executable,'--literal-pathspecs',*prefix]
def call(args,**options):
    result=subprocess.run([*git,*args],cwd=root,capture_output=True,**options)
    if result.returncode:
        sys.stderr.buffer.write(result.stderr);sys.exit(result.returncode)
    return result.stdout
def private(path):return any(path==item or path.startswith(item+b'/') for item in protected)
def chunks(paths):
    for index in range(0,len(paths),32):yield paths[index:index+32]
cached=call(['ls-files','--cached','-z']).split(b'\0')
# Drop derived inputs from this worktree's index only, including accidental
# pre-staging. The worktree files, source project, HEAD and original index stay.
for batch in chunks([path for path in cached if path and private(path)]):
    call(['restore','--staged','--source=HEAD','--',*batch])
paths=call(['ls-files','--cached','--others','--exclude-standard','-z']).split(b'\0')
selected=sorted(set(path for path in paths if path and not private(path)))
for batch in chunks(selected):call(['add','-A','--',*batch])
"#;
pub(super) fn checkpoint_stage_spec(python:&str,workspace:&str,git:ProcessSpec,protected:Vec<String>)->ProcessSpec {
    script_spec(python,STAGE_CHECKPOINT,vec![workspace.into(),git.executable,json!(git.args).to_string(),json!(protected).to_string()],120)
}

// Only public attestation metadata/digests leave this fixed reader. Private
// artifact bytes, symlinks and unbounded files are never returned to the server.
const READ_EXTERNAL_ATTESTATION: &str = r#"
import hashlib,json,os,pathlib,stat,sys
root=os.open(sys.argv[1],os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
def open_scoped(path):
    parts=pathlib.PurePosixPath(path).parts
    if not parts or pathlib.PurePosixPath(path).is_absolute() or any(p in ('.','..','.git') for p in parts):raise ValueError('unsafe evidence path')
    fd=os.dup(root)
    try:
        for p in parts[:-1]:
            nxt=os.open(p,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=fd);os.close(fd);fd=nxt
        result=os.open(parts[-1],os.O_RDONLY|os.O_NOFOLLOW,dir_fd=fd)
        if not stat.S_ISREG(os.fstat(result).st_mode):os.close(result);raise ValueError('evidence must be a regular file')
        return result
    finally:os.close(fd)
try:
    fd=open_scoped(sys.argv[2])
    try:
        if os.fstat(fd).st_size>16384:raise ValueError('attestation too large')
        data=json.loads(os.read(fd,16385))
    finally:os.close(fd)
    if set(data)!=set(('statement','signature_hex','artifact_path')):raise ValueError('invalid attestation envelope')
    fd=open_scoped(data['artifact_path'])
    try:
        if os.fstat(fd).st_size>64*1024*1024:raise ValueError('artifact too large')
        h=hashlib.sha256()
        while True:
            block=os.read(fd,1024*1024)
            if not block:break
            h.update(block)
    finally:os.close(fd)
    print(json.dumps({'statement':data['statement'],'signature_hex':data['signature_hex'],'observed_artifact_sha256':h.hexdigest()},separators=(',',':')))
except (ValueError,OSError,KeyError,TypeError):
    print('{}')
finally:os.close(root)
"#;
pub(super) fn external_attestation_spec(workspace:&str,path:&str)->ProcessSpec {
    script_spec(&probe_python(),READ_EXTERNAL_ATTESTATION,vec![workspace.into(),path.into()],30)
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use webcodex_core::runner_protocol::{validate_process_argv,ShellProcessArgv,PROCESS_STDIN_MAX_BYTES};
    fn admitted(spec:ProcessSpec){
        validate_process_argv(&ShellProcessArgv{executable:spec.executable,args:spec.args}).unwrap();
        if let Some(stdin)=spec.stdin{assert!(stdin.len()<=PROCESS_STDIN_MAX_BYTES);assert!(!stdin.contains('\0'));}
    }
    #[test]
    fn fixed_environment_helpers_use_supported_structured_stdin_not_shell_mode(){
        admitted(probe_spec("/tmp/workspace","codex",json!([])));
        admitted(prepare_inputs_spec("/usr/bin/python3","/tmp/source","/tmp/workspace",json!({"workspace_requirements":[],"dependencies":[]})));
        admitted(external_attestation_spec("/tmp/workspace","evidence.json"));
        admitted(checkpoint_stage_spec("/usr/bin/python3","/tmp/workspace",ProcessSpec{executable:"/usr/bin/git".into(),args:vec!["--git-dir=/tmp/git".into()],stdin:None,cwd:None,timeout_secs:120},vec![".goal-acceptance".into()]));
    }
    #[test]
    fn maximum_approved_manifests_are_batched_within_existing_runner_limits(){
        let entries:Vec<_>=(0..128).map(|i|json!({"source_path":format!("{}-{i}","a".repeat(1020)),"destination_path":format!("{}-{i}","b".repeat(1020)),"source_sha256":"c".repeat(64),"permission":"read_only"})).collect();
        let specs=prepare_input_specs("/usr/bin/python3","/tmp/source","/tmp/workspace",json!({"workspace_requirements":entries,"dependencies":[]}));
        assert!(specs.len()>1);for spec in specs{admitted(spec)}
    }
}
