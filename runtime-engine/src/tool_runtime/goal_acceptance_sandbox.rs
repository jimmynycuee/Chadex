//! Server-generated literal argv; never inspect server-local Runner paths.
//! Only the bound Runner's actual preflight result is readiness evidence.
//! Human approval/revision hashes and signed execution receipts belong to main.
//! Production loopback fails closed: port numbers cannot prove service identity
//! or prevent listener replacement between preflight and child bind.
//! The cfg(test) exact-tcp4 candidate is only for owned-listener fixtures.

use std::fmt;
use std::path::{Component, Path, PathBuf};

pub(super) type ProcessArgv = (String, Vec<String>, Option<String>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SandboxLane {
    Offline,
    Loopback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SandboxError {
    InvalidWorkspace,
    InvalidCwd,
    InvalidArgv,
    InvalidPorts,
    InvalidReadOnlyPath,
    PolicyTooLarge,
    InvalidDockerBackend,
    DockerOwnershipUnverified,
    LoopbackOwnershipUnverified,
}

impl fmt::Display for SandboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidWorkspace => "acceptance_sandbox_invalid_workspace",
            Self::InvalidCwd => "acceptance_sandbox_invalid_cwd",
            Self::InvalidArgv => "acceptance_sandbox_invalid_argv",
            Self::InvalidPorts => "acceptance_sandbox_invalid_ports",
            Self::InvalidReadOnlyPath => "acceptance_sandbox_invalid_read_only_path",
            Self::PolicyTooLarge => "acceptance_sandbox_policy_too_large: use a scoped tree-pinned input group",
            Self::InvalidDockerBackend => "acceptance_docker_backend_unverified_or_invalid",
            Self::DockerOwnershipUnverified => "acceptance_docker_container_ownership_unverified",
            Self::LoopbackOwnershipUnverified =>
                "acceptance_loopback_ownership_unverified: exact ports cannot exclude sensitive services or listener replacement; requires Runner-owned sockets/broker",
        })
    }
}
impl std::error::Error for SandboxError {}

const SYSTEM_ROOTS: &[&str] = &[
    "/bin", "/sbin", "/usr/bin", "/usr/sbin", "/usr/lib", "/usr/libexec",
    "/usr/share", "/System/Library", "/System/Cryptexes",
    "/System/Volumes/Preboot/Cryptexes", "/Library/Apple/System",
    "/usr/local/Cellar", "/usr/local/opt", "/usr/local/Frameworks",
    "/usr/local/lib/node_modules/npm",
    "/Library/Frameworks/Python.framework", "/opt/homebrew/Cellar",
    "/opt/homebrew/opt", "/Applications/Xcode.app/Contents/Developer",
];
// No system.sb import: it adds network/IPC/credential reads. Literal "/" is
// only the directory needed by dyld openat bootstrap, never its descendants.
const BOOTSTRAP: &str = r#"(version 1)
(deny default)
(deny file-link)
(allow mach-bootstrap)
(allow syscall*)
(allow process-fork)
(allow file-read* file-map-executable (literal "/Library/Apple/usr/libexec/oah/libRosettaRuntime"))
(allow sysctl-read (sysctl-name-prefix "hw.") (sysctl-name "kern.ostype" "kern.osrelease" "kern.osrevision" "kern.osversion" "kern.version" "kern.osproductversion" "kern.hostname" "kern.domainname" "sysctl.proc_translated" "sysctl.proc_native"))
(allow file-read* (literal "/") (literal "/dev/null") (literal "/dev/zero") (literal "/dev/random") (literal "/dev/urandom") (literal "/private/var/select/sh"))
(allow file-read* (literal "/usr/local/bin/python3") (literal "/usr/local/bin/node") (literal "/usr/local/bin/npm") (literal "/usr/local/bin/npx") (literal "/opt/homebrew/bin/python3") (literal "/opt/homebrew/bin/node") (literal "/opt/homebrew/bin/npm") (literal "/opt/homebrew/bin/npx"))
(allow file-read-metadata (literal "/usr/local") (literal "/usr/local/bin") (literal "/opt/homebrew") (literal "/opt/homebrew/bin"))
(allow file-write-data (literal "/dev/null") (literal "/dev/zero"))
"#;

// Fixed trusted shell code, not acceptance input. Every path and argv byte is
// a positional argument. No interpolation, eval, source, or profile files.
// Setup runs AFTER sandbox application and canonical-path validation.
const EXEC: &str = r#"set -eu
umask 077
workspace=$1
cwd=$2
shift 2
cd "$workspace"
[ "$(/bin/pwd -P)" = "$workspace" ] || exit 72
cd "$cwd"
[ "$(/bin/pwd -P)" = "$cwd" ] || exit 72
for dir in "$workspace/.goal-acceptance" "$HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$TMPDIR" "$workspace/.goal-acceptance/logs"; do
    [ ! -L "$dir" ] || exit 73
    if [ -e "$dir" ]; then
        [ -d "$dir" ] || exit 73
    else
        /bin/mkdir "$dir"
    fi
done
exec "$@"
"#;

/// Pure builder; execute preflight on the SAME bound Runner before dispatch.
/// Supply only a trusted managed-worktree path and approved revision argv.
/// Third-party runtimes/deps require an approved scoped copy into the worktree.
/// Reject environment/provider overrides; never use an empty-prefix fallback.
pub(super) fn build(
    workspace: &Path,
    argv: &[String],
    cwd: Option<&Path>,
    lane: SandboxLane,
    ports: &[u16],
) -> Result<ProcessArgv, SandboxError> {
    build_with_read_only(workspace, argv, cwd, lane, ports, &[])
}

/// Approved fixture destinations receive explicit deny rules (not chmod-only).
/// The caller owns source/provenance/hash verification and must ensure no
/// pre-existing hardlinks and no concurrent unsandboxed writer can mutate them.
pub(super) fn build_with_read_only(
    workspace: &Path,
    argv: &[String],
    cwd: Option<&Path>,
    lane: SandboxLane,
    ports: &[u16],
    read_only_paths: &[PathBuf],
) -> Result<ProcessArgv, SandboxError> {
    validate_lane(lane, ports)?;
    build_inner(workspace, argv, cwd, &[], read_only_paths)
}

/// Native profile-application probe: only /usr/bin/true can execute, no shell,
/// acceptance script, workspace mutation, or agent-written program. Success
/// does NOT prove cwd/toolchain/service readiness or acceptance completion.
pub(super) fn preflight(
    workspace: &Path,
    cwd: Option<&Path>,
    lane: SandboxLane,
    ports: &[u16],
) -> Result<ProcessArgv, SandboxError> {
    preflight_with_read_only(workspace, cwd, lane, ports, &[])
}

pub(super) fn preflight_with_read_only(
    workspace: &Path,
    cwd: Option<&Path>,
    lane: SandboxLane,
    ports: &[u16],
    read_only_paths: &[PathBuf],
) -> Result<ProcessArgv, SandboxError> {
    validate_lane(lane, ports)?;
    let (root, _) = paths(workspace, cwd)?;
    validate_read_only(workspace, read_only_paths)?;
    let entries=environment(&root);
    let env=entries.iter().skip(1).map(|value|json_string(value)).collect::<Vec<_>>().join(",");
    let payload=format!("{{\"profile\":{},\"environment\":[{}],\"root\":{},\"preflight\":true}}",json_string(&profile(&root,&[],&[],true)),env,json_string(&root));
    if payload.len()>64*1024{return Err(SandboxError::PolicyTooLarge)}
    Ok(("/usr/bin/python3".into(),vec!["-I".into(),"-S".into(),"-c".into(),NATIVE_LAUNCHER.into()],Some(payload)))
}

fn validate_ports(ports: &[u16]) -> Result<(), SandboxError> {
    if ports.len() > 8 || ports.iter().any(|p| *p < 49152) {
        return Err(SandboxError::InvalidPorts);
    }
    let mut sorted = ports.to_vec();
    sorted.sort_unstable();
    if sorted.windows(2).any(|w| w[0] == w[1]) {
        return Err(SandboxError::InvalidPorts);
    }
    Ok(())
}

fn validate_lane(lane: SandboxLane, ports: &[u16]) -> Result<(), SandboxError> {
    validate_ports(ports)?;
    match lane {
        SandboxLane::Offline if !ports.is_empty() => Err(SandboxError::InvalidPorts),
        SandboxLane::Offline => Ok(()),
        SandboxLane::Loopback if ports.is_empty() => Err(SandboxError::InvalidPorts),
        SandboxLane::Loopback => Err(SandboxError::LoopbackOwnershipUnverified),
    }
}

fn text_path(path: &Path) -> Option<&str> {
    path.to_str().filter(|s| !s.is_empty() && !s.chars().any(char::is_control))
}

fn clean(path: &Path) -> bool {
    text_path(path).is_some_and(|s| !s.contains("//") && !s.ends_with('/')
        && !s.split('/').any(|c| c == "." || c == ".."))
}

fn paths(workspace: &Path, cwd: Option<&Path>) -> Result<(String, String), SandboxError> {
    if !workspace.is_absolute() || !clean(workspace)
        || workspace.components().count() < 4
        || ["/System", "/usr", "/bin", "/sbin", "/Library", "/dev", "/private/etc"]
            .iter().any(|p| workspace.starts_with(p))
    {
        return Err(SandboxError::InvalidWorkspace);
    }
    let cwd = match cwd {
        None => workspace.to_path_buf(),
        Some(path) => {
            text_path(path).ok_or(SandboxError::InvalidCwd)?;
            if path.components().any(|c| matches!(c, Component::ParentDir)) {
                return Err(SandboxError::InvalidCwd);
            }
            let path: PathBuf = path.components()
                .filter(|c| !matches!(c, Component::CurDir)).collect();
            if path.as_os_str().is_empty() { workspace.to_path_buf() }
            else if path.is_absolute() { path } else { workspace.join(path) }
        }
    };
    if !cwd.starts_with(workspace) || !clean(&cwd) {
        return Err(SandboxError::InvalidCwd);
    }
    Ok((workspace.to_str().unwrap().into(), cwd.to_str().unwrap().into()))
}

fn validate_read_only(workspace: &Path, paths: &[PathBuf]) -> Result<(), SandboxError> {
    if paths.len() > 128 || paths.iter().any(|p|
        !p.is_absolute() || !clean(p) || p == workspace || !p.starts_with(workspace)
        // Setup directories must stay writable. Fixture files belong elsewhere.
        || p.starts_with(workspace.join(".goal-acceptance"))
    ) {
        return Err(SandboxError::InvalidReadOnlyPath);
    }
    Ok(())
}

/// Scheme string encoding is independent from shell and argv quoting.
fn sb_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn profile(workspace: &str, ports: &[u16], readonly: &[PathBuf], probe: bool) -> String {
    let mut p = BOOTSTRAP.to_owned();
    // Traverse the approved worktree without reading ancestor contents.
    for parent in Path::new(workspace).ancestors().skip(1) {
        p.push_str(&format!("(allow file-read-metadata (literal {}))\n", sb_string(parent.to_str().unwrap())));
    }
    let exec = if probe { "(literal \"/usr/bin/true\")" } else { "" };
    p.push_str(&format!("(allow process-exec {exec})\n"));
    if !probe {
        p.push_str("(allow signal (target same-sandbox))\n");
    }
    for root in SYSTEM_ROOTS.iter().copied().chain(std::iter::once(workspace)) {
        p.push_str(&format!("(allow file-read* file-map-executable (subpath {}))\n", sb_string(root)));
        for parent in Path::new(root).ancestors().skip(1) {
            p.push_str(&format!("(allow file-read-metadata (literal {}))\n", sb_string(parent.to_str().unwrap())));
        }
    }
    if !probe {
        p.push_str(&format!("(allow file-write* (subpath {}))\n", sb_string(workspace)));
    }
    let mut parents=std::collections::BTreeSet::new();
    for path in readonly {
        let quoted=sb_string(path.to_str().unwrap());
        p.push_str(&format!("(deny file-write* (subpath {quoted}))\n"));
        for parent in path.ancestors().skip(1).take_while(|p|p.starts_with(workspace)){parents.insert(parent);}
    }
    for parent in parents {
        p.push_str(&format!("(deny file-write-unlink (literal {}))\n",sb_string(parent.to_str().unwrap())));
    }
    // Test-only candidate. tcp (without 4) would also permit same-port ::1.
    // No port 0, wildcard port, UDP, Unix sockets, DNS, or external hosts.
    for port in ports {
        p.push_str(&format!(
            "(allow network-bind network-inbound (local tcp4 \"localhost:{port}\"))\n\
             (allow network-outbound (remote tcp4 \"localhost:{port}\"))\n"
        ));
    }
    p
}

fn environment(workspace: &str) -> Vec<String> {
    let state = format!("{workspace}/.goal-acceptance");
    let mut args = vec!["-i".into()];
    for (key, value) in [
        ("PATH", format!("{workspace}/.goal-acceptance/bin:{workspace}/node_modules/.bin:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin")),
        ("HOME", format!("{state}/home")),
        ("CFFIXED_USER_HOME", format!("{state}/home")),
        ("XDG_CONFIG_HOME", format!("{state}/config")),
        ("XDG_CACHE_HOME", format!("{state}/cache")),
        ("XDG_DATA_HOME", format!("{state}/data")),
        ("XDG_STATE_HOME", format!("{state}/state")),
        ("TMPDIR", format!("{state}/tmp")),
        ("TMP", format!("{state}/tmp")),
        ("TEMP", format!("{state}/tmp")),
        ("WRANGLER_LOG_PATH", format!("{state}/logs")),
        ("WRANGLER_SEND_METRICS", "false".into()),
        ("WRANGLER_SEND_ERROR_REPORTS", "false".into()),
        ("CLOUDFLARE_CF_FETCH_ENABLED", "false".into()),
        ("CLOUDFLARE_LOAD_DEV_VARS_FROM_DOT_ENV", "false".into()),
        ("npm_config_cache", format!("{state}/cache/npm")),
        ("npm_config_userconfig", format!("{state}/config/npmrc")),
        ("npm_config_globalconfig", format!("{state}/config/npm-globalrc")),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("GIT_CONFIG_GLOBAL", "/dev/null".into()),
        ("OPENSSL_CONF", "/dev/null".into()),
        // Bytecode is derived acceptance state, not a source/checkpoint edit.
        ("PYTHONDONTWRITEBYTECODE", "1".into()),
        ("LANG", "C".into()), ("LC_ALL", "C".into()), ("CI", "true".into()),
        ("NO_COLOR", "1".into()),
    ] { args.push(format!("{key}={value}")); }
    args
}

fn json_string(value:&str)->String {
    let mut output=String::from("\"");
    for c in value.chars(){match c {
        '\\'=>output.push_str("\\\\"),'"'=>output.push_str("\\\""),
        '\n'=>output.push_str("\\n"),'\r'=>output.push_str("\\r"),'\t'=>output.push_str("\\t"),
        c if (c as u32)<32=>output.push_str(&format!("\\u{:04x}",c as u32)),c=>output.push(c),
    }}
    output.push('"');output
}
const NATIVE_LAUNCHER:&str=r#"
import json,os,signal,subprocess,sys,tempfile
try:
    data=json.load(sys.stdin)
except (ValueError,TypeError):
    print('ACCEPTANCE_WRAPPER_INPUT_INVALID',file=sys.stderr);sys.exit(78)
fd,path=tempfile.mkstemp(prefix='chadex-acceptance-',suffix='.sb',dir='/private/tmp')
try:
    with os.fdopen(fd,'w',encoding='utf-8') as output:output.write(data['profile'])
    environment=dict(item.split('=',1) for item in data['environment'])
    try:
        command=['/usr/bin/sandbox-exec','-f',path,'/usr/bin/true'] if data.get('preflight') is True else ['/usr/bin/sandbox-exec','-f',path,'/bin/sh','-c',data['setup'],'goal-acceptance',data['root'],data['cwd'],*data['argv']]
        child=subprocess.Popen(command,env=environment,cwd=data['root'],stdin=subprocess.DEVNULL)
    except OSError:
        print('NATIVE_SANDBOX_UNAVAILABLE',file=sys.stderr);sys.exit(78)
    def forward(sig,frame):
        if child.poll() is None:child.send_signal(sig)
    for sig in (signal.SIGTERM,signal.SIGINT,signal.SIGHUP):signal.signal(sig,forward)
    code=child.wait()
    sys.exit(code if code>=0 else 128-code)
finally:
    try:os.unlink(path)
    except FileNotFoundError:pass
"#;

fn build_inner(
    workspace: &Path, argv: &[String], cwd: Option<&Path>,
    ports: &[u16], readonly: &[PathBuf],
) -> Result<ProcessArgv, SandboxError> {
    let (workspace_text, cwd) = paths(workspace, cwd)?;
    validate_read_only(workspace, readonly)?;
    if argv.is_empty() || argv[0].trim().is_empty() || argv[0].starts_with('-')
        || argv.iter().any(|arg| arg.contains('\0'))
    { return Err(SandboxError::InvalidArgv); }
    let entries=environment(&workspace_text);
    let env=entries.iter().skip(1).map(|value|json_string(value)).collect::<Vec<_>>().join(",");
    let args=argv.iter().map(|value|json_string(value)).collect::<Vec<_>>().join(",");
    let payload=format!("{{\"profile\":{},\"environment\":[{}],\"root\":{},\"cwd\":{},\"setup\":{},\"argv\":[{}]}}",
        json_string(&profile(&workspace_text,ports,readonly,false)),env,json_string(&workspace_text),json_string(&cwd),json_string(EXEC),args);
    if payload.len()>64*1024{return Err(SandboxError::PolicyTooLarge)}
    Ok(("/usr/bin/python3".into(),vec!["-I".into(),"-S".into(),"-c".into(),NATIVE_LAUNCHER.into()],Some(payload)))
}

/// Stand-alone smoke driver uses --cfg test. NEVER available in production.
#[cfg(test)]
pub(super) fn fixture_build(
    workspace: &Path, argv: &[String], ports: &[u16], readonly: &[PathBuf],
) -> Result<ProcessArgv, SandboxError> {
    validate_ports(ports)?;
    if ports.is_empty() { return Err(SandboxError::InvalidPorts); }
    build_inner(workspace, argv, None, ports, readonly)
}


/// All fields originate from trusted, bound Runner probes and an approved
/// backend revision, never from agent JSON. No server-local existence checks.
#[derive(Debug, Clone)]
pub(super) struct DockerBackend {
    pub executable: PathBuf,
    pub endpoint: String,
    pub image_id: String,
    pub goal_run_id: String,
    pub commit_sha: String,
    pub container_name: String,
    pub uid: u32,
    pub gid: u32,
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn docker_backend(backend: &DockerBackend) -> Result<(), SandboxError> {
    let executable = backend.executable.to_str().unwrap_or_default();
    let socket = backend.endpoint.strip_prefix("unix://").map(Path::new);
    let uuid = backend.container_name.strip_prefix("chadex-goal-");
    let valid_uuid = uuid.is_some_and(|s| {
        let groups: Vec<_> = s.split('-').collect();
        groups.len() == 5 && groups.iter().zip([8,4,4,4,12]).all(|(s,n)| hex(s,n))
    });
    if !["/usr/local/bin/docker", "/opt/homebrew/bin/docker", "/usr/bin/docker",
        "/bin/docker", "/Applications/Docker.app/Contents/Resources/bin/docker"]
        .contains(&executable)
        || !socket.is_some_and(|p| p.is_absolute() && clean(p))
        || !backend.image_id.strip_prefix("sha256:").is_some_and(|s| hex(s,64))
        || !(hex(&backend.commit_sha,40) || hex(&backend.commit_sha,64))
        || backend.goal_run_id.is_empty() || backend.goal_run_id.len() > 128
        || !backend.goal_run_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || !valid_uuid || backend.uid == 0 || backend.gid == 0
    { return Err(SandboxError::InvalidDockerBackend); }
    Ok(())
}

fn docker_command(backend: &DockerBackend, args: Vec<String>) -> Result<ProcessArgv, SandboxError> {
    docker_backend(backend)?;
    let mut outer: Vec<String> = [
        "-i", "PATH=/usr/bin:/bin", "HOME=/var/empty",
        "DOCKER_CONFIG=/var/empty", "DOCKER_CLI_HINTS=false",
    ].map(String::from).into();
    outer.extend([backend.executable.to_str().unwrap().into(), "--host".into(), backend.endpoint.clone()]);
    outer.extend(args);
    Ok(("/usr/bin/env".into(), outer, None))
}

/// Actual Runner daemon/version result is required; missing daemon is blocked.
pub(super) fn docker_preflight(backend: &DockerBackend) -> Result<ProcessArgv, SandboxError> {
    docker_command(backend, vec!["version".into(), "--format".into(), "{{json .Server}}".into()])
}

/// Main verifies exact ID, Linux platform, approved image config/provenance,
/// no image-declared Volumes, and its required in-image tools/deps. No pull.
pub(super) fn docker_image_preflight(backend: &DockerBackend) -> Result<ProcessArgv, SandboxError> {
    docker_command(backend, vec![
        "image".into(), "inspect".into(), "--format".into(), "{\"Id\":{{json .Id}},\"Os\":{{json .Os}},\"Architecture\":{{json .Architecture}},\"Config\":{\"Volumes\":{{json (index .Config \"Volumes\")}}}}".into(), backend.image_id.clone(),
    ])
}

/// Private-loopback backend. Inside network=none, ephemeral localhost ports
/// cannot reach host/operator/backend ports; this is a DIFFERENT approved
/// backend policy, never an automatic substitute for native exact-port lane.
/// Only the approved worktree is mounted; no Docker socket, home, host ports,
/// environment file, privilege, device, shared PID/IPC, registry access or pull.
/// argv describes container paths. Main must approve a new revision if argv
/// used host-only absolute paths. cwd is scoped and translated to /workspace.
fn docker_execution(
    workspace: &Path, argv: &[String], cwd: Option<&Path>,
    backend: &DockerBackend, read_only_paths: &[PathBuf], create: bool,
) -> Result<ProcessArgv, SandboxError> {
    docker_backend(backend)?;
    let (root, cwd) = paths(workspace, cwd)?;
    validate_read_only(workspace, read_only_paths)?;
    if root.contains([',','"']) || Path::new(&backend.endpoint[7..]).starts_with(workspace)
        || read_only_paths.iter().any(|p| p.to_str().unwrap().contains([',','"']))
    { return Err(SandboxError::InvalidDockerBackend); }
    if argv.is_empty() || argv[0].trim().is_empty() || argv[0].starts_with('-')
        || argv.iter().any(|s| s.contains('\0'))
    { return Err(SandboxError::InvalidArgv); }
    let relative = Path::new(&cwd).strip_prefix(workspace).map_err(|_| SandboxError::InvalidCwd)?;
    let container_cwd = if relative.as_os_str().is_empty() { "/workspace".to_owned() }
        else { Path::new("/workspace").join(relative).to_str().unwrap().to_owned() };
    let mut args = [
        "--pull", "never", "--network", "none", "--read-only",
        "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
        "--ipc", "private", "--pids-limit", "256", "--memory", "1g",
        "--cpus", "2", "--no-healthcheck", "--log-driver", "none",
        "--entrypoint", "/usr/bin/env",
    ].map(String::from).to_vec();
    args.insert(0, if create { "create" } else { "run" }.into());
    if !create { args.insert(1, "--rm".into()); }
    args.extend([
        "--name".into(), backend.container_name.clone(),
        "--label".into(), format!("io.chadex.goal_run_id={}", backend.goal_run_id),
        "--label".into(), format!("io.chadex.commit_sha={}", backend.commit_sha),
        "--user".into(), format!("{}:{}", backend.uid, backend.gid),
        "--mount".into(), format!("type=bind,src={root},dst=/workspace,readonly"),
        "--tmpfs".into(), format!("/workspace/.goal-acceptance:rw,nosuid,nodev,mode=0700,uid={},gid={},size=256m",backend.uid,backend.gid),
        "--tmpfs".into(), format!("/tmp:rw,nosuid,nodev,noexec,mode=0700,uid={},gid={},size=64m",backend.uid,backend.gid),
        "--workdir".into(), container_cwd.clone(),
    ]);
    for path in read_only_paths {
        let relative = path.strip_prefix(workspace).map_err(|_| SandboxError::InvalidReadOnlyPath)?;
        let target = Path::new("/workspace").join(relative);
        args.extend(["--mount".into(), format!("type=bind,src={},dst={},readonly",
            path.to_str().unwrap(), target.to_str().unwrap())]);
    }
    args.push(backend.image_id.clone());
    args.extend(environment("/workspace"));
    args.extend([
        "/bin/sh".into(), "-c".into(), EXEC.into(), "goal-acceptance".into(),
        "/workspace".into(), container_cwd,
    ]);
    args.extend(argv.iter().cloned());
    docker_command(backend, args)
}

/// Test-only convenience. Production must create -> journal ID/Created ->
/// start/attach -> actual wait/inspect exit -> externally bounded owned cleanup.
#[cfg(test)]
pub(super) fn build_docker(
    workspace: &Path, argv: &[String], cwd: Option<&Path>,
    backend: &DockerBackend, read_only_paths: &[PathBuf],
) -> Result<ProcessArgv, SandboxError> {
    docker_execution(workspace, argv, cwd, backend, read_only_paths, false)
}

pub(super) fn docker_container_probe(backend: &DockerBackend) -> Result<ProcessArgv, SandboxError> {
    docker_command(backend, vec!["container".into(), "inspect".into(), "--format".into(), "{{json .}}".into(), backend.container_name.clone()])
}


/// Identity is captured from trusted create stdout + FIRST inspect, persisted
/// before start. Never read an agent-writable worktree cidfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DockerContainerIdentity {
    pub id: String,
    pub created: String,
}

/// Populate ONLY from the actual bound Runner Docker inspect response.
/// Main separately verifies HostConfig isolation/RO mounts and image config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DockerContainerFacts {
    pub id: String,
    pub created: String,
    pub name: String,
    pub image_id: String,
    pub goal_run_id: String,
    pub commit_sha: String,
}

fn container_identity(identity: &DockerContainerIdentity) -> Result<(), SandboxError> {
    let c = &identity.created;
    if !hex(&identity.id,64) || c.len() < 20 || c.len() > 40 || !c.ends_with('Z')
        || !c.contains('T')
        || !c.bytes().all(|b| b.is_ascii_digit() || b"-:.TZ".contains(&b))
    { return Err(SandboxError::DockerOwnershipUnverified); }
    Ok(())
}

/// Call only after actual Runner exit=0, not timed out/unknown. Exactly one
/// full container ID is accepted; CLI progress/error output is not an identity.
pub(super) fn docker_created_id(stdout: &str) -> Result<String, SandboxError> {
    let id = stdout.trim();
    if !hex(id,64) { return Err(SandboxError::DockerOwnershipUnverified); }
    Ok(id.to_owned())
}

/// docker wait CLI can exit 0 when the CONTAINER failed. Its trusted stdout
/// is the acceptance exit status; never treat start/attach CLI exit as PASS.
pub(super) fn docker_wait_exit_code(stdout: &str) -> Result<i32, SandboxError> {
    let text = stdout.trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(SandboxError::DockerOwnershipUnverified);
    }
    text.parse::<i32>().ok().filter(|n| (0..=255).contains(n))
        .ok_or(SandboxError::DockerOwnershipUnverified)
}

/// Creation has NO agent-code execution and NO autoremove. Main journals the
/// ExactlyOnce create intent, actual ID, first-inspect Created and ownership
/// before start. Unknown creation/start outcomes are reconciled, not replayed.
pub(super) fn docker_create(
    workspace: &Path, argv: &[String], cwd: Option<&Path>,
    backend: &DockerBackend, read_only_paths: &[PathBuf],
) -> Result<ProcessArgv, SandboxError> {
    docker_execution(workspace, argv, cwd, backend, read_only_paths, true)
}

pub(super) fn docker_start(
    backend: &DockerBackend, identity: &DockerContainerIdentity,
) -> Result<ProcessArgv, SandboxError> {
    container_identity(identity)?;
    docker_command(backend, vec![
        "container".into(), "start".into(), "--attach".into(), identity.id.clone(),
    ])
}

pub(super) fn docker_inspect(
    backend: &DockerBackend, actual_container_id: &str,
) -> Result<ProcessArgv, SandboxError> {
    if !hex(actual_container_id,64) { return Err(SandboxError::DockerOwnershipUnverified); }
    docker_command(backend, vec![
        "container".into(), "inspect".into(), "--format".into(),
        concat!("{\"Id\":{{json .Id}},\"Created\":{{json .Created}},\"Name\":{{json .Name}},",
            "\"Image\":{{json .Image}},\"Config\":{\"User\":{{json (index .Config \"User\")}},\"Labels\":{{json (index .Config \"Labels\")}}},",
            "\"State\":{{json .State}},\"Mounts\":{{json .Mounts}},\"HostConfig\":{",
            "\"NetworkMode\":{{json (index .HostConfig \"NetworkMode\")}},\"ReadonlyRootfs\":{{json (index .HostConfig \"ReadonlyRootfs\")}},",
            "\"Privileged\":{{json (index .HostConfig \"Privileged\")}},\"CapDrop\":{{json (index .HostConfig \"CapDrop\")}},",
            "\"SecurityOpt\":{{json (index .HostConfig \"SecurityOpt\")}},\"PortBindings\":{{json (index .HostConfig \"PortBindings\")}},",
            "\"PidMode\":{{json (index .HostConfig \"PidMode\")}},\"IpcMode\":{{json (index .HostConfig \"IpcMode\")}}}}").into(), actual_container_id.into(),
    ])
}

pub(super) fn docker_wait(
    backend: &DockerBackend, identity: &DockerContainerIdentity,
) -> Result<ProcessArgv, SandboxError> {
    container_identity(identity)?;
    docker_command(backend, vec!["container".into(), "wait".into(), identity.id.clone()])
}

pub(super) fn verify_docker_ownership(
    backend: &DockerBackend, identity: &DockerContainerIdentity,
    fresh: &DockerContainerFacts,
) -> Result<(), SandboxError> {
    docker_backend(backend)?;
    container_identity(identity)?;
    if fresh.id != identity.id || fresh.created != identity.created
        || fresh.name.strip_prefix('/').unwrap_or(&fresh.name) != backend.container_name
        || fresh.image_id != backend.image_id || fresh.goal_run_id != backend.goal_run_id
        || fresh.commit_sha != backend.commit_sha
    { return Err(SandboxError::DockerOwnershipUnverified); }
    Ok(())
}

/// External deadline/cancel/recovery must stop the Runner attach job AND call
/// verified cleanup by actual ID. Killing Docker CLI alone is not completion.
/// Main persists the trusted fresh-inspect facts before removal; unknown
/// ownership/Created/ID is preserved. Never rm by name, prune, or replay.
pub(super) fn docker_cleanup(
    backend: &DockerBackend, identity: &DockerContainerIdentity,
    fresh: &DockerContainerFacts,
) -> Result<ProcessArgv, SandboxError> {
    verify_docker_ownership(backend, identity, fresh)?;
    docker_command(backend, vec![
        "container".into(), "rm".into(), "--force".into(), identity.id.clone(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    const ROOT: &str = "/private/tmp/goal-acceptance-fixture";
    fn argv() -> Vec<String> { vec!["/bin/echo".into(), "ok".into()] }

    #[test]
    fn literal_argv_without_server_filesystem_dependency() {
        let c = build(Path::new(ROOT), &argv(), None, SandboxLane::Offline, &[]).unwrap();
        assert_eq!(c.0, "/usr/bin/python3");
        assert_eq!(&c.1[..3], ["-I","-S","-c"]);
        assert_eq!(c.1[3],NATIVE_LAUNCHER);
        let p=profile(ROOT,&[],&[],false);
        assert!(c.2.as_ref().unwrap().contains(&format!("\"profile\":{}",json_string(&p))));
        assert!(c.2.as_ref().unwrap().contains("\"argv\":[\"/bin/echo\",\"ok\"]"));
        assert!(p.contains("(deny default)"));
        for broad in ["(allow network", "(subpath \"/usr\")", "(subpath \"/Users\")", "system.sb"] {
            assert!(!p.contains(broad), "{broad}");
        }
    }
    #[test]
    fn caller_bytes_never_become_shell_source() {
        let root = Path::new("/private/tmp/w \\ \" ) (allow default) ; $(touch x)");
        let a = vec!["/bin/echo".into(), "$(touch escaped)".into(), "; exit 0".into(), "".into(), "line\nnext".into()];
        let c = build(root, &a, None, SandboxLane::Offline, &[]).unwrap();
        assert_eq!(c.1[3],NATIVE_LAUNCHER);
        assert!(!c.1.iter().any(|value|a.iter().any(|arg|!arg.is_empty() && value==arg)));
        let literal=a.iter().map(|arg|json_string(arg)).collect::<Vec<_>>().join(",");
        assert!(c.2.as_ref().unwrap().contains(&format!("\"argv\":[{literal}]")));
        assert!(c.2.as_ref().unwrap().contains(&format!("\"setup\":{}",json_string(EXEC))));
        assert!(sb_string(root.to_str().unwrap()).contains("\\\""));
    }
    #[test]
    fn cwd_stays_scoped() {
        for cwd in ["../escape", "child/../../escape", "/private/tmp/elsewhere"] {
            assert_eq!(build(Path::new(ROOT), &argv(), Some(Path::new(cwd)), SandboxLane::Offline, &[]), Err(SandboxError::InvalidCwd));
        }
        assert!(build(Path::new(ROOT), &argv(), Some(Path::new("./tests")), SandboxLane::Offline, &[]).is_ok());
        assert!(build(Path::new(ROOT), &argv(), Some(Path::new(".")), SandboxLane::Offline, &[]).is_ok());
    }
    #[test]
    fn broad_or_malformed_workspace_is_rejected() {
        for root in ["/", "/private/tmp", "/Users/example", "/usr/local/project", "/System/foo/bar", "/private/tmp/../home", "/private/tmp/a\nb", "/private//tmp/root", "/private/tmp/root/"] {
            assert_eq!(build(Path::new(root), &argv(), None, SandboxLane::Offline, &[]), Err(SandboxError::InvalidWorkspace), "{root}");
        }
    }
    #[test]
    fn malformed_argv_is_rejected() {
        for a in [vec![], vec!["".into()], vec!["-option".into()], vec!["/bin/echo".into(), "a\0b".into()]] {
            assert_eq!(build(Path::new(ROOT), &a, None, SandboxLane::Offline, &[]), Err(SandboxError::InvalidArgv));
        }
    }
    #[test]
    fn loopback_never_claims_service_ownership() {
        assert_eq!(build(Path::new(ROOT), &argv(), None, SandboxLane::Loopback, &[55991]), Err(SandboxError::LoopbackOwnershipUnverified));
        assert_eq!(preflight(Path::new(ROOT), None, SandboxLane::Loopback, &[55991]), Err(SandboxError::LoopbackOwnershipUnverified));
    }
    #[test]
    fn invalid_ports_never_widen_access() {
        for (l,p) in [(SandboxLane::Offline, vec![55991]), (SandboxLane::Loopback, vec![]), (SandboxLane::Loopback, vec![0]), (SandboxLane::Loopback, vec![49151]), (SandboxLane::Loopback, vec![55991,55991]), (SandboxLane::Loopback, (55000..55009).collect())] {
            assert_eq!(build(Path::new(ROOT), &argv(), None, l, &p), Err(SandboxError::InvalidPorts));
        }
    }
    #[test]
    fn native_preflight_only_executes_true() {
        let c = preflight(Path::new(ROOT), None, SandboxLane::Offline, &[]).unwrap();
        let p=c.2.as_ref().unwrap();
        assert!(p.contains(json_string("(allow process-exec (literal \"/usr/bin/true\"))").trim_matches('"')));
        assert!(!p.contains("(allow file-write*"));assert!(!p.contains("(allow network"));
        assert!(p.contains("\"preflight\":true"));assert!(!p.contains("\"argv\""));
        assert_eq!(c.0,"/usr/bin/python3");assert!(c.1.iter().all(|arg|arg.len()<8192));
    }
    #[test]
    fn environment_is_closed_allowlist() {
        let args = environment(ROOT);
        assert!(args.contains(&"PYTHONDONTWRITEBYTECODE=1".to_string()));
        assert_eq!(args[0], "-i");
        for key in ["HOME=", "XDG_CONFIG_HOME=", "XDG_CACHE_HOME=", "TMPDIR=", "WRANGLER_LOG_PATH="] {
            assert!(args.iter().any(|a| a.starts_with(key) && a.contains(ROOT)));
        }
        for key in ["CLOUDFLARE_API_TOKEN=", "AUTHORIZATION=", "NODE_OPTIONS=", "DYLD_", "SSH_AUTH_SOCK=", "WEBCODEX_TOKEN=", "WRANGLER_HOME="] {
            assert!(!args.iter().any(|a| a.starts_with(key)));
        }
    }
    #[test]
    fn candidate_exact_ports_are_ipv4_tcp_only() {
        let c = fixture_build(Path::new(ROOT), &argv(), &[55991,55992], &[]).unwrap();
        let p=profile(ROOT,&[55991,55992],&[],false);
        assert!(c.2.as_ref().unwrap().contains(&json_string(&p)));
        for s in ["(remote tcp4 \"localhost:55991\")", "(local tcp4 \"localhost:55992\")"] { assert!(p.contains(s)); }
        for s in ["localhost:*", "localhost:0", "(remote tcp "] { assert!(!p.contains(s)); }
    }
    #[test]
    fn immutable_copy_has_explicit_denies_including_parent_unlink() {
        let dest = Path::new(ROOT).join("fixtures/private/evidence.json");
        let c = build_with_read_only(Path::new(ROOT), &argv(), None, SandboxLane::Offline, &[], &[dest.clone()]).unwrap();
        let p=profile(ROOT,&[],&[dest.clone()],false);
        assert!(c.2.as_ref().unwrap().contains(&json_string(&p)));
        assert!(p.contains(&format!("(deny file-write* (subpath {})", sb_string(dest.to_str().unwrap()))));
        assert!(p.contains(&format!("(deny file-write-unlink (literal \"{ROOT}/fixtures/private\"))")));
    }
    #[test]
    fn readonly_path_cannot_escape_or_block_setup() {
        for path in [ROOT.into(), "/private/tmp/escape".into(), format!("{ROOT}/a/../b"), format!("{ROOT}/.goal-acceptance/home")] {
            assert_eq!(build_with_read_only(Path::new(ROOT), &argv(), None, SandboxLane::Offline, &[], &[PathBuf::from(path)]), Err(SandboxError::InvalidReadOnlyPath));
        }
    }
    #[test]
    fn large_private_manifest_is_not_truncated_or_put_in_an_oversized_argv() {
        let readonly:Vec<_>=(0..65).map(|i|Path::new(ROOT).join(format!("fixtures/private/approved-evidence-{i:03}.json"))).collect();
        let command=build_with_read_only(Path::new(ROOT),&argv(),None,SandboxLane::Offline,&[],&readonly).unwrap();
        let policy=profile(ROOT,&[],&readonly,false);
        assert!(policy.len()>8192);
        assert!(command.1.iter().all(|arg|arg.len()<=8192));
        let input=command.2.unwrap();assert!(input.len()<=64*1024);
        assert!(input.contains(&json_string(&policy)));
    }
    #[test]
    fn excessive_policy_fails_closed_without_a_different_wrapper() {
        let readonly:Vec<_>=(0..128).map(|i|Path::new(ROOT).join(format!("{}-{i}","a".repeat(900)))).collect();
        assert_eq!(build_with_read_only(Path::new(ROOT),&argv(),None,SandboxLane::Offline,&[],&readonly),Err(SandboxError::PolicyTooLarge));
    }

    fn docker() -> DockerBackend {
        DockerBackend {
            executable: "/usr/local/bin/docker".into(),
            endpoint: "unix:///Users/runner/.docker/run/docker.sock".into(),
            image_id: format!("sha256:{}", "a".repeat(64)),
            goal_run_id: "wc_grun_fixture".into(), commit_sha: "b".repeat(40),
            container_name: "chadex-goal-12345678-1234-1234-1234-123456789abc".into(),
            uid: 501, gid: 20,
        }
    }
    #[test]
    fn docker_is_pinned_private_nonprivileged_and_env_clear() {
        let b = docker();
        let c = build_docker(Path::new(ROOT), &["node".into(), "-e".into(), "$(touch x)".into()], None, &b, &[]).unwrap();
        for pair in [("--pull","never"),("--network","none"),("--cap-drop","ALL"),("--security-opt","no-new-privileges"),("--entrypoint","/usr/bin/env")] {
            assert!(c.1.windows(2).any(|w| w == [pair.0,pair.1]));
        }
        assert!(c.1.contains(&"--read-only".into()));
        assert!(!c.1.contains(&"/workspace/".into()));
        assert!(c.1.contains(&b.image_id));
        assert!(c.1.contains(&"$(touch x)".into()));
        for flag in ["--privileged","--publish","-p","--use-api-socket","--env-file","--volumes-from","--device"] {
            assert!(!c.1.contains(&flag.into()));
        }
        assert!(!c.1.iter().any(|s| s.contains("src=/Users/runner/.docker")));
    }
    #[test]
    fn docker_untrusted_names_images_and_endpoints_fail_closed() {
        let mut b = docker(); b.image_id = "node:latest".into();
        assert_eq!(docker_preflight(&b), Err(SandboxError::InvalidDockerBackend));
        b = docker(); b.endpoint = "tcp://127.0.0.1:2375".into();
        assert_eq!(docker_preflight(&b), Err(SandboxError::InvalidDockerBackend));
        b = docker(); b.container_name = "existing-sensitive-container".into();
        assert_eq!(docker_preflight(&b), Err(SandboxError::InvalidDockerBackend));
        b = docker(); b.uid = 0;
        assert_eq!(docker_preflight(&b), Err(SandboxError::InvalidDockerBackend));
    }
    #[test]
    fn docker_readonly_fixtures_are_readonly_mounts_and_cwd_is_scoped() {
        let path = Path::new(ROOT).join("fixtures/approved.json");
        let c = build_docker(Path::new(ROOT), &argv(), Some(Path::new("tests")), &docker(), &[path.clone()]).unwrap();
        assert!(c.1.contains(&format!("type=bind,src={},dst=/workspace/fixtures/approved.json,readonly",path.display())));
        assert!(c.1.windows(2).any(|w| w == ["--workdir","/workspace/tests"]));
        assert!(build_docker(Path::new(ROOT), &argv(), Some(Path::new("../escape")), &docker(), &[]).is_err());
    }
    #[test]
    fn docker_mount_csv_injection_and_workspace_socket_are_rejected() {
        assert_eq!(build_docker(Path::new("/private/tmp/foo,dst=/host"), &argv(), None, &docker(), &[]), Err(SandboxError::InvalidDockerBackend));
        let mut b = docker(); b.endpoint = format!("unix://{ROOT}/docker.sock");
        assert_eq!(build_docker(Path::new(ROOT), &argv(), None, &b, &[]), Err(SandboxError::InvalidDockerBackend));
    }

    fn identity() -> DockerContainerIdentity {
        DockerContainerIdentity { id: "c".repeat(64), created: "2026-10-14T14:00:00.000000000Z".into() }
    }
    fn facts() -> DockerContainerFacts {
        let b = docker(); let i = identity();
        DockerContainerFacts { id:i.id, created:i.created, name:format!("/{}",b.container_name), image_id:b.image_id, goal_run_id:b.goal_run_id, commit_sha:b.commit_sha }
    }
    #[test]
    fn docker_cleanup_is_exact_verified_identity_only() {
        let i=identity(); let f=facts();
        let c=docker_cleanup(&docker(),&i,&f).unwrap();
        assert_eq!(c.1.last(),Some(&i.id));
        assert!(c.1.windows(3).any(|w| w == ["container","rm","--force"]));
        let mut f=f;f.id="--all".into();
        assert!(docker_cleanup(&docker(),&i,&f).is_err());
    }
    #[test]
    fn docker_create_never_executes_or_autoremoves_and_workspace_is_ro() {
        let c=docker_create(Path::new(ROOT),&argv(),None,&docker(),&[]).unwrap();
        assert!(c.1.contains(&"create".into()));
        assert!(!c.1.contains(&"--rm".into()));
        assert!(c.1.contains(&format!("type=bind,src={ROOT},dst=/workspace,readonly")));
        assert_eq!(docker_created_id(&format!("{}\n","c".repeat(64))).unwrap(),"c".repeat(64));
        for text in ["short","--all","0\n1"] { assert!(docker_created_id(text).is_err()); }
    }
    #[test]
    fn docker_created_time_labels_image_and_id_are_all_bound() {
        for field in ["created","id","name","image","goal","commit"] {
            let mut f=facts();
            match field {
                "created"=>f.created="2026-10-15T14:00:00Z".into(),
                "id"=>f.id="d".repeat(64),"name"=>f.name="other".into(),
                "image"=>f.image_id="sha256:other".into(),"goal"=>f.goal_run_id="other".into(),
                _=>f.commit_sha="d".repeat(40),
            }
            assert!(docker_cleanup(&docker(),&identity(),&f).is_err(),"{field}");
        }
        let mut i=identity();i.created="unknown".into();assert!(docker_start(&docker(),&i).is_err());
    }
    #[test]
    fn docker_wait_cli_success_is_not_container_success() {
        assert_eq!(docker_wait_exit_code("137\n"),Ok(137));
        assert_eq!(docker_wait_exit_code("0\n"),Ok(0));
        for text in ["", "0\n1", "ExitCode:0", "256", "-1"] { assert!(docker_wait_exit_code(text).is_err()); }
        let c=docker_start(&docker(),&identity()).unwrap();
        assert!(c.1.windows(3).any(|w|w == ["container","start","--attach"]));
        assert_eq!(c.1.last(),Some(&identity().id));
    }

}

#[cfg(test)]
mod preflight_transport_regression {
    use super::*;
    #[test]
    fn deep_workspace_profile_uses_bounded_stdin_before_any_agent_work(){
        let root=format!("/private/tmp/goal-fixture/{}","nested/".repeat(20).trim_end_matches('/'));
        let command=preflight(Path::new(&root),None,SandboxLane::Offline,&[]).unwrap();
        assert!(command.2.as_ref().unwrap().len()>8192);
        assert!(command.1.iter().all(|arg|arg.len()<8192));
        assert!(command.2.as_ref().unwrap().len()<64*1024);
    }
}
