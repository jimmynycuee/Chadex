#!/usr/bin/env python3
"""Real local kernel sandbox smoke, NOT connected Runner readiness or a receipt.

No production G1 files/services are modified. Dependencies are read/copy-only.
A blocked Worker/D1 run exits 2 after recording successful controls separately.
"""
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

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
MODULE = REPO / "runtime-engine/src/tool_runtime/goal_acceptance_sandbox.rs"


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
    ap = argparse.ArgumentParser()
    ap.add_argument("--rustc", required=True)
    ap.add_argument("--node", required=True, help="Existing standalone Node, system dylibs only")
    ap.add_argument("--worker-node", help="Approved native deployment Node matching existing workerd arch")
    ap.add_argument("--node-modules", help="Existing complete Wrangler node_modules; readonly scoped copy")
    options = ap.parse_args()
    root = Path(tempfile.mkdtemp(prefix="chadex-goal-acceptance-", dir="/private/tmp")).resolve()
    workspace = root / "worktree"
    workspace.mkdir()
    report = {
        "kind": "local-kernel-smoke-not-goal-receipt",
        "module_sha256": sha(MODULE),
        "fixture_sha256": {p.name: sha(p) for p in HERE.iterdir() if p.is_file()},
        "source_commit": subprocess.check_output(["git", "-C", str(REPO), "rev-parse", "HEAD"], text=True).strip(),
        "root": str(root), "workspace": str(workspace), "commands": [], "controls": [],
        "worker_d1": {"status": "NOT_VALIDATED"},
    }
    report_path = root / "report.json"

    def persist():
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")

    def run(argv, *, timeout=20, env=None, cwd=workspace):
        started = time.time_ns()
        try:
            p = subprocess.run(argv, cwd=cwd, env=env, text=True, capture_output=True, timeout=timeout, input=getattr(argv,"stdin",None))
            record = {"argv": argv, "exit_code": p.returncode, "stdout": p.stdout[-24000:],
                      "stderr": p.stderr[-24000:], "started_unix_ns": started,
                      "finished_unix_ns": time.time_ns()}
        except subprocess.TimeoutExpired as exc:
            record = {"argv": argv, "exit_code": None, "timed_out": True,
                      "stdout": str(exc.stdout or "")[-24000:], "stderr": str(exc.stderr or "")[-24000:],
                      "started_unix_ns": started, "finished_unix_ns": time.time_ns()}
        report["commands"].append(record)
        persist()
        return record

    def control(name, passed, record):
        report["controls"].append({"name": name, "passed": bool(passed),
                                   "command_index": report["commands"].index(record)})
        persist()
        print(f"{name}: {'PASS' if passed else 'FAIL'} (exit {record.get('exit_code')})", flush=True)
        if not passed:
            raise RuntimeError(name + " failed; evidence " + str(report_path))

    compiler = str(Path(options.rustc).resolve(strict=True))
    unit = root / "unit"
    driver = root / "driver"
    p = run([compiler, "--edition", "2021", "--test", str(HERE / "driver.rs"), "-o", str(unit)])
    control("unit_compile", p["exit_code"] == 0, p)
    listing = run([str(unit), "--list"])
    expected = sum(line.endswith(": test") for line in listing["stdout"].splitlines())
    p = run([str(unit)])
    control("unit_tests", listing["exit_code"] == 0 and expected > 0 and p["exit_code"] == 0
            and f"test result: ok. {expected} passed; 0 failed; 0 ignored;" in p["stdout"]
            and "0 filtered out" in p["stdout"], p)
    p = run([compiler, "--edition", "2021", "--cfg", "test", str(HERE / "driver.rs"), "-o", str(driver)])
    control("fixture_driver_compile", p["exit_code"] == 0, p)
    bindir = workspace / ".goal-acceptance/bin"
    bindir.mkdir(parents=True)
    node_source = Path(options.node).resolve(strict=True)
    node = bindir / "node"
    shutil.copy2(node_source, node)
    report["node_provenance"] = {"source": str(node_source), "source_sha256": sha(node_source),
                                 "copy": str(node), "copy_sha256": sha(node)}

    def build(mode, argv, ports=(), readonly=()):
        p = subprocess.run([str(driver), mode, str(workspace),
                            ",".join(map(str, ports)) or "-", *map(str, readonly), "--", *argv],
                           capture_output=True)
        if p.returncode:
            raise RuntimeError(f"builder {mode}: {p.returncode} {p.stderr.decode()}")
        return Invocation(p.stdout)

    def sandbox(argv, mode="offline", ports=(), readonly=(), **kw):
        return run(build(mode, argv, ports, readonly), **kw)

    many = workspace / "readonly-many"
    many.mkdir()
    many_inputs = []
    for index in range(65):
        path = many / f"approved-evidence-{index:03}.json"
        path.write_text('SYNTHETIC_READONLY_ONLY')
        many_inputs.append(path)
    payload = build("offline", ["/bin/sh", "-c", 'printf corrupt > "$1"', "fixture", str(many_inputs[-1])], readonly=many_inputs)
    assert len(payload.stdin.encode()) > 8192
    p = run(payload)
    control("large_65_input_policy_is_enforced_without_oversized_argv",
            p["exit_code"] != 0 and "Operation not permitted" in p["stderr"]
            and all(path.read_text() == 'SYNTHETIC_READONLY_ONLY' for path in many_inputs), p)

    p = sandbox([], mode="preflight")
    if p["exit_code"] != 0:
        report["blocked"] = "sandbox profile execution unavailable; no kernel denial test is counted PASS"
        persist()
        print(f"BLOCKED: native preflight exit {p['exit_code']}; evidence {report_path}", flush=True)
        return 2
    control("native_true_preflight", True, p)
    p = sandbox(["/bin/sh", "-c", 'printf allowed > allowed.txt; printf "%s" "$HOME"'])
    control("worktree_write_and_home_redirect",
            p["exit_code"] == 0 and (workspace / "allowed.txt").read_text() == "allowed"
            and p["stdout"] == str(workspace / ".goal-acceptance/home"), p)
    outside = root / "outside-write"
    p = sandbox(["/bin/sh", "-c", 'printf malicious > "$1"', "probe", str(outside)])
    control("malicious_outside_write_denied", p["exit_code"] != 0 and not outside.exists()
            and "Operation not permitted" in p["stderr"], p)
    home = root / "mock-home/.ssh"
    home.mkdir(parents=True)
    secret = home / "synthetic-credential"
    secret.write_text("SYNTHETIC_CREDENTIAL_NEVER_REAL_SECRET")
    p = sandbox(["/bin/cat", str(secret)])
    control("home_credential_read_denied", p["exit_code"] != 0 and not p["stdout"]
            and "Operation not permitted" in p["stderr"], p)
    harmless_home = Path.home() / ".codex/RTK.md"
    if harmless_home.is_file():
        p = sandbox(["/bin/cat", str(harmless_home)])
        control("actual_home_config_read_denied", p["exit_code"] != 0 and not p["stdout"]
                and "Operation not permitted" in p["stderr"], p)
    else:
        report["actual_home_config_read"] = "NOT_VALIDATED: harmless known home file missing"
    link = workspace / "home-link"
    link.symlink_to(secret)
    p = sandbox(["/bin/cat", str(link)])
    control("worktree_symlink_cannot_read_home", p["exit_code"] != 0 and not p["stdout"]
            and "Operation not permitted" in p["stderr"], p)
    payload = "$(touch literal-argv-escape); unexpected"
    p = sandbox(["/bin/echo", payload])
    control("actual_argv_remains_literal", p["exit_code"] == 0 and p["stdout"] == payload + "\n"
            and not (workspace / "literal-argv-escape").exists(), p)
    redirected_home = workspace / ".goal-acceptance/home"
    redirected_home.rmdir()
    redirected_home.symlink_to(home.parent, target_is_directory=True)
    p = sandbox(["/usr/bin/true"])
    control("redirected_home_symlink_fails_closed", p["exit_code"] == 73, p)
    redirected_home.unlink()
    redirected_home.mkdir()
    sensitive_env = os.environ.copy()
    sensitive_env.update(CLOUDFLARE_API_TOKEN="SYNTHETIC_TOKEN", SSH_AUTH_SOCK="/mock/socket",
                         NODE_OPTIONS="--invalid-option", WEBCODEX_TOKEN="SYNTHETIC_TOKEN")
    p = sandbox(["/usr/bin/env"], env=sensitive_env)
    control("inherited_credentials_and_node_options_cleared",
            p["exit_code"] == 0 and "SYNTHETIC_TOKEN" not in p["stdout"]
            and "NODE_OPTIONS=" not in p["stdout"] and "SSH_AUTH_SOCK=" not in p["stdout"], p)

    p = sandbox(["/usr/sbin/sysctl", "-n", "kern.proc"])
    control("global_process_sysctl_denied", p["exit_code"] != 0 and not p["stdout"]
            and "Operation not permitted" in p["stderr"], p)
    for program, arguments in [("/usr/local/bin/python3", ["-I", "-S", "-c", "print(\"PYTHON_TOOLCHAIN_READY\")"]),
                               ("/Applications/Xcode.app/Contents/Developer/usr/bin/python3", ["-I", "-S", "-c", "print(\"XCODE_PYTHON_READY\")"]),
                               ("/usr/local/bin/node", ["--version"]),
                               ("/usr/local/bin/npm", ["--version"])]:
        if Path(program).is_file():
            p = sandbox([program, *arguments])
            control("legacy_readonly_toolchain_" + Path(program).name + ("_xcode" if "Xcode" in program else ""),
                    p["exit_code"] == 0, p)
        else:
            report.setdefault("missing_legacy_toolchains", []).append(program)
    protected = workspace / "fixtures/private/evidence.json"
    protected.parent.mkdir(parents=True)
    protected.write_text('{"marker":"REAL_HASH_APPROVED_COPY_SIMULATION"}\n')
    before = sha(protected)
    protected.chmod(0o444)
    attacks = [
        ("readonly_write", ["/bin/sh", "-c", 'printf corrupted > "$1"', "probe", str(protected)]),
        ("readonly_chmod", ["/bin/chmod", "666", str(protected)]),
        ("readonly_unlink", ["/bin/rm", str(protected)]),
        ("readonly_rename", ["/bin/mv", str(protected), str(workspace / "renamed.json")]),
        ("readonly_parent_rename", ["/bin/mv", str(protected.parent), str(workspace / "alias")]),
        ("readonly_hardlink", ["/bin/ln", str(protected), str(workspace / "hardlink.json")]),
    ]
    for name, argv in attacks:
        p = sandbox(argv, readonly=[protected])
        control(name + "_denied", p["exit_code"] != 0 and protected.exists()
                and sha(protected) == before and protected.stat().st_mode & 0o777 == 0o444, p)
    report["readonly_before_sha256"] = report["readonly_after_sha256"] = before

    listeners = []
    hits = {}
    lock = threading.Lock()

    def listen(host, port=0):
        s = socket.socket(socket.AF_INET6 if ":" in host else socket.AF_INET, socket.SOCK_STREAM)
        if ":" in host:
            s.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
        s.bind((host, port))
        s.listen()
        listeners.append(s)
        port = s.getsockname()[1]
        key = f"{host}:{port}"
        hits[key] = 0
        def serve():
            while True:
                try:
                    c, _ = s.accept()
                except OSError:
                    return
                with lock:
                    hits[key] += 1
                c.sendall(b"OWNED_FIXTURE_ENDPOINT")
                c.close()
        threading.Thread(target=serve, daemon=True).start()
        return port

    allowed = listen("127.0.0.1")
    other = listen("127.0.0.1")
    listen("::1", allowed)
    code = """const net=require('node:net');
const s=net.connect({host:process.argv[1],port:Number(process.argv[2])});
s.on('data',b=>{console.log(b.toString());s.destroy()});
s.on('error',e=>{console.error(e.code);process.exitCode=4});
s.setTimeout(3000,()=>{console.error('TIMEOUT');process.exit(5)});"""
    def connect(host, port, mode="offline", ports=()):
        return sandbox([str(node), "-e", code, host, str(port)], mode, ports)
    p = connect("127.0.0.1", allowed)
    control("offline_localhost_denied", p["exit_code"] == 4 and "EPERM" in p["stderr"], p)
    p = connect("192.0.2.1", 443)
    control("offline_outbound_denied", p["exit_code"] == 4 and "EPERM" in p["stderr"], p)
    p = connect("127.0.0.1", allowed, "loopback-owned-fixture", [allowed])
    control("candidate_exact_ipv4_port_allowed", p["exit_code"] == 0
            and "OWNED_FIXTURE_ENDPOINT" in p["stdout"], p)
    p = connect("127.0.0.1", other, "loopback-owned-fixture", [allowed])
    control("candidate_other_localhost_port_denied", p["exit_code"] == 4 and "EPERM" in p["stderr"], p)
    p = connect("::1", allowed, "loopback-owned-fixture", [allowed])
    control("candidate_same_port_ipv6_denied", p["exit_code"] == 4 and "EPERM" in p["stderr"], p)
    p = connect("192.0.2.1", 443, "loopback-owned-fixture", [allowed])
    control("candidate_outbound_denied", p["exit_code"] == 4 and "EPERM" in p["stderr"], p)
    p = sandbox([str(node), "-e",
        "const s=require('node:net').createServer();s.on('error',e=>{console.error(e.code);process.exit(4)});s.listen(0,'127.0.0.1',()=>{console.log('UNEXPECTED_ALLOW');s.close()});"],
        "loopback-owned-fixture", [allowed])
    control("candidate_ephemeral_bind_zero_denied", p["exit_code"] == 4 and "EPERM" in p["stderr"], p)
    p = run([str(driver), "loopback-production", str(workspace), str(allowed), "--", "/usr/bin/true"])
    control("production_loopback_ownership_unknown_fails_closed", p["exit_code"] == 78
            and "ownership_unverified" in p["stderr"], p)
    report["network"] = {"allowed_fixture_port": allowed, "denied_fixture_port": other, "listener_hits": hits.copy()}
    control("denied_listeners_received_no_connections", hits[f"127.0.0.1:{other}"] == 0
            and hits[f"::1:{allowed}"] == 0, p)
    for listener in listeners:
        listener.close()

    if options.node_modules:
        deps = Path(options.node_modules).resolve(strict=True)
        # Reject escapes rather than silently importing arbitrary linked content.
        for directory, dirs, files in os.walk(deps, followlinks=False):
            for name in dirs + files:
                path = Path(directory) / name
                if path.is_symlink() and not path.resolve().is_relative_to(deps):
                    raise RuntimeError("dependency symlink escapes scoped source: " + str(path))
        target = workspace / "node_modules"
        shutil.copytree(deps, target, symlinks=True)
        provenance = {}
        for rel in ["wrangler/package.json", "wrangler/wrangler-dist/cli.js",
                    "miniflare/package.json", "miniflare/dist/src/index.js",
                    "workerd/lib/main.js"]:
            source = deps / rel
            provenance[rel] = {"source_sha256": sha(source), "copy_sha256": sha(target / rel)}
        report["dependency_provenance"] = {"source_root": str(deps), "copies": provenance}
        shutil.copy2(HERE / "worker.mjs", workspace / "worker.mjs")
        shutil.copy2(HERE / "worker_d1.cjs", workspace / "worker_d1.cjs")
        # Exact explicit port; INTERNAL Miniflare bind(0) remains forbidden.
        # Host test owns no service at this port and production lane is blocked.
        worker_node = options.worker_node or str(node)
        p = sandbox([worker_node, str(workspace / "worker_d1.cjs"), str(allowed)],
                    "loopback-owned-fixture", [allowed], timeout=30)
        success = p["exit_code"] == 0 and "LOCAL_WORKER_D1_REAL_READBACK" in p["stdout"]
        report["worker_d1"] = {"status": "PASS" if success else "BLOCKED",
            "command_index": report["commands"].index(p),
            "reason": None if success else "actual dependency/runtime or exact-port failure; no Worker->D1 PASS",
            "worker_source_sha256": sha(workspace / "worker.mjs")}
        # Also run the actual copied Wrangler under the candidate sandbox.
        p = sandbox([worker_node, str(target / "wrangler/bin/wrangler.js"), "--version"],
                    "loopback-owned-fixture", [allowed], timeout=15)
        report["wrangler_version_command_index"] = report["commands"].index(p)
    else:
        report["worker_d1"]["reason"] = "existing Wrangler/Miniflare deps were not supplied; NOT_VALIDATED"
    persist()
    print("evidence:", report_path, flush=True)
    print("Worker/D1:", report["worker_d1"]["status"], flush=True)
    return 0 if report["worker_d1"]["status"] == "PASS" else 2


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, OSError) as exc:
        print("FAILED/BLOCKED:", exc, flush=True)
        raise SystemExit(2)
