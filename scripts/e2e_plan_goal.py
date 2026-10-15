#!/usr/bin/env python3
"""End-to-end Plan Mode / Goal Mode check against a real isolated Chadex
backend (helper -> runtime server -> runner), never touching live state.

"ChatGPT" is emulated exactly like the tunnel: MCP Stateless-2026 tools/call
with the runtime bootstrap token. The "Chadex app" is the helper bridge
(planGoalOperator over operator REST). After goal_run_start no MCP call is
made at all: progress happens with ChatGPT absent.

usage: e2e_plan_goal.py <helper> <resources> <out.json> <mode: fake|codex> [--restart]

<helper> is a built chadex-helper and <resources> a directory containing
chadex-runtime/{chadex-runtime-cli,chadex-runtime-server,chadex-runtime-runner}
(e.g. dist/Chadex.app/Contents/Helpers/chadex-helper and .../Contents/Resources).
`fake` uses a deterministic local agent script; `codex` uses the real default
provider (`codex exec`, needs `codex login`) and consumes model usage.
`--restart` quits the backend while the agent runs and starts it again.
Everything runs in a fresh temporary directory; live Chadex state is untouched.
"""
import hashlib, json, os, shutil, subprocess, sys, tempfile, time, urllib.request
from pathlib import Path

HELPER, RESOURCES, OUT, MODE = sys.argv[1], sys.argv[2], Path(sys.argv[3]), sys.argv[4]
RESTART = "--restart" in sys.argv
report = {"mode": MODE, "steps": []}
AGENT_SLEEP = 20 if "--restart" in sys.argv else 2
SANDBOX_PREFIX = ["codex", "sandbox", "-c", 'sandbox_mode="workspace-write"', "-c", "sandbox_workspace_write.network_access=false", "--"]


def step(name, **data):
    report["steps"].append({"step": name, **data})
    print(f"[{time.strftime('%H:%M:%S')}] {name}: {json.dumps(data, ensure_ascii=False)[:600]}", flush=True)


class Helper:
    def __init__(self, data_dir, extra_env):
        env = os.environ.copy()
        env.update({"CHADEX_DATA_DIR": str(data_dir), "CHADEX_RESOURCE_DIR": RESOURCES, **extra_env})
        self.log = open(data_dir.parent / f"helper-{int(time.time())}.log", "w")
        self.p = subprocess.Popen([HELPER], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=self.log, text=True, bufsize=1, env=env)
        self.n = 0

    def request(self, method, params=None, allow_error=False):
        self.n += 1
        rid = f"e2e-{self.n}"
        self.p.stdin.write(json.dumps({"protocol_version": 1, "request_id": rid, "method": method, "params": params or {}}) + "\n")
        self.p.stdin.flush()
        while True:
            line = self.p.stdout.readline()
            if not line:
                raise RuntimeError(f"helper exited during {method}")
            msg = json.loads(line)
            if msg.get("request_id") != rid:
                continue
            if msg.get("error") and not allow_error:
                raise RuntimeError(f"{method} failed: {msg['error']}")
            return msg.get("result") if not msg.get("error") else {"bridge_error": msg["error"]}

    def close(self):
        if self.p.poll() is None:
            try:
                self.request("shutdown")
            except Exception:
                pass
            self.p.stdin.close()
            self.p.wait(timeout=60)


def tree_digest(root: Path):
    """Content digest of every file including .git (index, refs, objects)."""
    h = hashlib.sha256()
    for path in sorted(p for p in root.rglob("*") if p.is_file()):
        rel = path.relative_to(root).as_posix()
        if rel.startswith(".git/logs/"):
            pass
        h.update(rel.encode() + b"\0" + path.read_bytes() + b"\0")
    return h.hexdigest()


def git(root, *args):
    return subprocess.run(["git", "-C", str(root), *args], capture_output=True, text=True).stdout.strip()


def find_runtime(data_dir: Path):
    for env in data_dir.rglob("*.env"):
        values = {}
        for line in env.read_text().splitlines():
            if "=" in line and not line.startswith("#"):
                k, v = line.split("=", 1)
                values[k.strip()] = v.strip().strip('"')
        if values.get("WEBCODEX_TOKEN"):
            listen = values.get("WEBCODEX_LISTEN") or values.get("WEBCODEX_ADDR") or ""
            return values, env
    raise RuntimeError("runtime env file not found")


class ChatGPT:
    """MCP client exactly like the tunnel: bootstrap bearer, stateless 2026."""
    def __init__(self, url, token):
        self.url, self.token = url.rstrip("/") + "/mcp", token

    def call(self, tool, arguments):
        body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
            "name": "call_runtime_tool", "arguments": {"tool": tool, "arguments": arguments},
            "_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                      "io.modelcontextprotocol/clientCapabilities": {},
                      "io.modelcontextprotocol/clientInfo": {"name": "e2e-chatgpt", "version": "1"}}}}
        req = urllib.request.Request(self.url, data=json.dumps(body).encode(), method="POST", headers={
            "Authorization": f"Bearer {self.token}", "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream", "Mcp-Protocol-Version": "2026-07-28",
            "Mcp-Method": "tools/call", "Mcp-Name": "call_runtime_tool"})
        raw = urllib.request.urlopen(req, timeout=120).read().decode()
        if raw.startswith("event:") or raw.startswith("data:"):
            raw = [l[5:].strip() for l in raw.splitlines() if l.startswith("data:")][-1]
        result = json.loads(raw).get("result", {})
        payload = result.get("structuredContent")
        if payload is None:
            for item in result.get("content", []):
                if item.get("type") == "text":
                    try:
                        payload = json.loads(item["text"])
                    except Exception:
                        payload = {"text": item["text"]}
        if isinstance(payload, dict) and "output" in payload and "success" in payload:
            return payload["success"], payload["output"]
        ok = not result.get("isError", False)
        return ok, payload or {}


def operator(helper, project, tool, arguments):
    value = helper.request("planGoalOperator", {"path": str(project), "tool": tool, "arguments": arguments}, allow_error=True)
    if isinstance(value, dict) and "bridge_error" in value:
        return False, value
    return bool(value.get("success")), value.get("output", value)



def check_duplicate_goal_start(helper, project, plan_id, revision, content_sha256, run_id, mode):
    """Exercise the real app-operator route, not only model-side run replay."""
    ok_before, before = operator(helper, project, "plan_review", {"action": "inspect", "plan_id": plan_id})
    before_ids = {d["approval_id"] for d in before.get("decisions", [])}
    attempts = []
    for requested_mode in (mode, "codex" if mode == "chatgpt" else "chatgpt"):
        ok, output = operator(helper, project, "plan_review", {
            "action": "approve_and_start", "plan_id": plan_id, "revision": revision,
            "content_sha256": content_sha256, "mode": requested_mode})
        replayed = output.get("goal_run") or {}
        attempts.append({"mode": requested_mode, "ok": ok,
                         "run_id": replayed.get("goal_run_id"), "created": output.get("created"),
                         "error_kind": output.get("error_kind"),
                         "safe_reply": not ok or (replayed.get("goal_run_id") == run_id and output.get("created") is False)})
    ok_overview, overview = operator(helper, project, "goal_operator", {"action": "overview"})
    runs = [r for r in overview.get("goal_runs", []) if r.get("plan_id") == plan_id]
    ok_after, after = operator(helper, project, "plan_review", {"action": "inspect", "plan_id": plan_id})
    after_ids = {d["approval_id"] for d in after.get("decisions", [])}
    single_run = len(runs) == 1 and runs[0].get("goal_run_id") == run_id
    original_mode = single_run and runs[0].get("mode") == mode
    unchanged_approvals = ok_before and ok_after and before_ids == after_ids
    passed = ok_overview and single_run and original_mode and unchanged_approvals and all(a["safe_reply"] for a in attempts)
    step("goal_start_button_deduplication", passed=passed, attempts=attempts,
         run_count=len(runs), original_mode_preserved=original_mode,
         approval_ids_unchanged=unchanged_approvals)
    if not passed:
        OUT.write_text(json.dumps(report, indent=2, ensure_ascii=False))
        helper.close()
        raise AssertionError("Repeated app Goal start created or changed a run/approval")


def main():
    root = Path(tempfile.mkdtemp(prefix="chadex-e2e-pg-")).resolve()
    project, data = root / "calc", root / "data"
    project.mkdir()
    (project / "calc.py").write_text("def add(a, b):\n    return a - b\n")
    (project / "test_calc.py").write_text(
        "import unittest\nfrom calc import add\n\nclass T(unittest.TestCase):\n"
        "    def test_add(self):\n        self.assertEqual(add(2, 2), 4)\n        self.assertEqual(add(-1, 5), 4)\n\n"
        "if __name__ == '__main__':\n    unittest.main()\n")
    for args in (["init", "-q", "-b", "main"], ["add", "-A"],
                 ["-c", "user.name=e2e", "-c", "user.email=e2e@local", "commit", "-q", "-m", "init"]):
        subprocess.run(["git", "-C", str(project), *args], check=True)
    # A repository hook that must never run from Chadex's own git commands.
    hook = project / ".git" / "hooks" / "post-commit"
    hook.write_text(f"#!/bin/sh\ntouch {root}/HOOK_RAN\n")
    hook.chmod(0o755)
    report["project"] = str(project)
    extra_env = {}
    if MODE == "fake":
        counter = root / "fake-agent-count"
        agent = root / "fake_agent.sh"
        agent.write_text(f"""#!/bin/sh
cat >/dev/null
n=$(cat {counter} 2>/dev/null || echo 0); n=$((n+1)); echo $n > {counter}
# Stateless per worktree: first a wrong fix (claiming success), then the real one.
if grep -q 'return a - b' calc.py; then sed -i '' 's/return a - b/return a * b/' calc.py; echo 'All done, tests pass!';
else sed -i '' 's/return a \\* b/return a + b/' calc.py; echo 'Fixed add().'; fi
echo 'tokens used'; echo '1,000'
sleep {AGENT_SLEEP}
""")
        agent.chmod(0o755)
        extra_env["CHADEX_GOAL_AGENT_ARGV"] = json.dumps([str(agent)])

    helper = Helper(data, extra_env)
    helper.request("activateProject", {"path": str(project)})
    t0 = time.time()
    helper.request("configureLocalSetup")
    step("runtime_ready", seconds=round(time.time() - t0, 1))
    env, env_file = find_runtime(data)
    url = env.get("WEBCODEX_PUBLIC_URL") or "http://" + env.get("WEBCODEX_LISTEN", env.get("WEBCODEX_BIND", "127.0.0.1:0"))
    # Prefer the probe url the helper recorded.
    for cfg in data.rglob("runner.toml"):
        for line in cfg.read_text().splitlines():
            if line.startswith("server_url"):
                url = line.split("=", 1)[1].strip().strip('"')
    chatgpt = ChatGPT(url, env["WEBCODEX_TOKEN"])
    ok, projects = chatgpt.call("list_projects", {})
    proj_id = None
    for p in (projects.get("projects") or []):
        if p.get("path") == str(project) or str(p.get("root", "")) == str(project):
            proj_id = p.get("id") or p.get("project")
    if not proj_id:
        proj_id = (projects.get("projects") or [{}])[0].get("id")
    step("chatgpt_connected", ok=ok, project=proj_id)

    # ---------------- Plan Mode ----------------
    before = tree_digest(project)
    ok, started = chatgpt.call("plan_start", {"project": proj_id, "title": "Fix add()"})
    plan_id = started.get("plan", {}).get("plan_id")
    step("plan_start", ok=ok, plan_id=plan_id, active=started.get("plan", {}).get("plan_mode_active"))
    reads = {}
    for tool, args in [("read_files", {"project": proj_id, "items": [{"path": "calc.py"}]}),
                       ("git_status", {"project": proj_id}), ("git_log", {"project": proj_id}),
                       ("search_project_texts", {"project": proj_id, "queries": [{"pattern": "def add"}]})]:
        ok, out = chatgpt.call(tool, args)
        reads[tool] = {"ok": ok, "error_kind": out.get("error_kind") if isinstance(out, dict) else None}
    step("plan_mode_reads", **reads)
    attempts = {
        "write_project_file": {"project": proj_id, "path": "calc.py", "content": "x"},
        "apply_text_edits": {"project": proj_id, "items": []},
        "delete_project_files": {"project": proj_id, "paths": ["calc.py"]},
        "run_shell": {"project": proj_id, "command": "echo hacked > calc.py"},
        "run_process": {"project": proj_id, "executable": "touch", "args": ["pwned"]},
        "run_script": {"project": proj_id, "language": "sh", "script": "touch pwned"},
        "run_tests": {"project": proj_id},
        "git_commit": {"project": proj_id, "message": "x"},
        "coding_agent_start": {"project": proj_id, "provider_id": "codex", "prompt": "x"},
        "computer_control": {"action": "click"},
        "mcp_tool": {"server": "x", "tool": "y"},
        "plugin_tool": {"plugin": "x", "action": "y"},
        "ssh_resource": {"action": "list"},
        "work_on_project": {"project": proj_id, "instruction": "edit"},
        "create_project": {"path": str(root / "new")},
    }
    blocked = {}
    for tool, args in attempts.items():
        try:
            ok, out = chatgpt.call(tool, args)
            blocked[tool] = (out.get("error_kind") if isinstance(out, dict) else None) if not ok else "ALLOWED!"
        except Exception as error:
            blocked[tool] = f"http_error:{error}"
    step("plan_mode_write_attempts", **blocked)
    ok, s1 = chatgpt.call("plan_save", {"plan_id": plan_id, "parent_revision": 0,
                                        "content": "# Fix add\nadd() subtracts; make it add.",
                                        "acceptance": [{"name": "unit tests", "argv": ["python3", "-m", "unittest", "-q"]}],
                                        "budget": {"max_iterations": 4, "max_minutes": 30, "agent_timeout_secs": 900}})
    ok2, s2 = chatgpt.call("plan_save", {"plan_id": plan_id, "parent_revision": 1,
                                         "content": "# Fix add\nadd() subtracts; make it add.\nKeep the API unchanged."})
    step("plan_save", v1=s1.get("revision"), v2=s2.get("revision"), diff=s2.get("diff_from_previous"))
    after = tree_digest(project)
    step("plan_mode_project_unchanged", identical=before == after, git_status=git(project, "status", "--porcelain"))

    # ---------------- Approval ----------------
    ok, fake = chatgpt.call("plan_review", {"action": "approve", "plan_id": plan_id, "revision": 2,
                                           "content_sha256": s2["revision"]["content_sha256"]})
    step("chatgpt_self_approval", ok=ok, error_kind=fake.get("error_kind") if isinstance(fake, dict) else fake)
    ok, early = chatgpt.call("goal_run_start", {"plan_id": plan_id})
    step("goal_before_approval", ok=ok, error_kind=early.get("error_kind"))
    ok, stale = operator(helper, project, "plan_review", {"action": "approve", "plan_id": plan_id, "revision": 1,
                                                          "content_sha256": s1["revision"]["content_sha256"]})
    step("operator_approve_stale_v1", ok=ok, error_kind=stale.get("error_kind") if isinstance(stale, dict) else stale)
    ok, inspect = operator(helper, project, "plan_review", {"action": "inspect", "plan_id": plan_id})
    step("operator_inspect", ok=ok, revision=(inspect.get("revision") or {}).get("revision"), diff=inspect.get("diff"))
    ok_w, waiting = chatgpt.call("plan_wait_approval", {"plan_id": plan_id, "wait_secs": 0})
    step("plan_wait_before_approval", ok=ok_w, status=waiting.get("status"))
    ok, decision = operator(helper, project, "plan_review", {"action": "approve", "plan_id": plan_id, "revision": 2,
                                                             "content_sha256": s2["revision"]["content_sha256"]})
    step("operator_approve_v2", ok=ok, decision=decision.get("decision", decision))
    ok_w, handed = chatgpt.call("plan_wait_approval", {"plan_id": plan_id, "wait_secs": 5})
    ok_x, model_goal = chatgpt.call("goal_run_start", {"plan_id": plan_id})
    step("plan_handed_to_chatgpt", ok=ok_w, status=handed.get("status"),
         model_cannot_start_goal=(not ok_x and model_goal.get("error_kind") in ("goal_not_started", "not_a_goal")),
         model_start_error=model_goal.get("error_kind"))

    # ---------------- Goal Mode (Codex), started in the app ----------------
    ok, started = operator(helper, project, "plan_review", {"action": "approve_and_start", "plan_id": plan_id,
                                                            "revision": 2, "mode": "codex",
                                                            "content_sha256": s2["revision"]["content_sha256"]})
    run = started if ok else {}
    run_id = (run.get("goal_run") or {}).get("goal_run_id")
    ok_again, again = chatgpt.call("goal_run_start", {"plan_id": plan_id})
    step("goal_run_start", ok=ok, run_id=run_id, replay_same=again.get("goal_run", {}).get("goal_run_id") == run_id,
         replay_created=again.get("created"), error=None if ok else started)
    step("chatgpt_disconnected", note="no further MCP calls; only the Chadex app polls status")
    t0 = time.time()
    restarted = False
    last = None
    while time.time() - t0 < (3600 if MODE == "codex" else 300):
        ok, status = operator(helper, project, "goal_operator", {"action": "status", "goal_run_id": run_id})
        gr = status.get("goal_run", {}) if isinstance(status, dict) else {}
        state = gr.get("state")
        if (state, gr.get("iteration")) != last:
            step("goal_progress", state=state, iteration=gr.get("iteration"), reason=gr.get("state_reason"),
                 elapsed=round(time.time() - t0))
            last = (state, gr.get("iteration"))
        if RESTART and not restarted and state == "running" and any(
                o.get("kind") == "agent" and o.get("state") == "dispatched" for o in status.get("operations", [])):
            step("quit_chadex_app", note="helper shutdown stops server+runner while the agent job runs")
            helper.close()
            time.sleep(3)
            helper = Helper(data, extra_env)
            helper.request("activateProject", {"path": str(project)})
            helper.request("configureLocalSetup")
            restarted = True
            step("chadex_app_restarted")
            continue
        if RESTART and restarted and state == "needs_attention" and not report.get("operator_resumed"):
            ok_c, chat = chatgpt.call("goal_run_control", {"goal_run_id": run_id, "action": "resume"})
            step("chatgpt_resume_needs_attention", ok=ok_c, error_kind=chat.get("error_kind"))
            ok_r, res = operator(helper, project, "goal_operator", {"action": "resume", "goal_run_id": run_id})
            report["operator_resumed"] = True
            step("operator_reviewed_and_resumed", ok=ok_r, state=res.get("goal_run", {}).get("state"))
            continue
        if state in ("succeeded", "failed", "cancelled", "stopped", "needs_attention", "paused"):
            break
        time.sleep(3)
    ok, final = operator(helper, project, "goal_operator", {"action": "status", "goal_run_id": run_id})
    gr = final.get("goal_run", {})
    branch = gr.get("result_branch")
    report["final_status"] = final
    step("goal_final", state=gr.get("state"), reason=gr.get("state_reason"), iteration=gr.get("iteration"),
         tokens=gr.get("tokens_used"), branch=branch, seconds=round(time.time() - t0))
    iterations = [e["detail"] for e in final.get("events", []) if e.get("kind") == "iteration"]
    step("acceptance_evidence", iterations=iterations)
    agent_ops = [o for o in final.get("operations", []) if o.get("kind") == "agent"]
    step("agent_dispatches", count=len(agent_ops), keys=[(o["op_key"], o["state"]) for o in agent_ops])
    # Independent re-verification of the result branch, outside Chadex.
    if branch:
        check = root / "verify"
        subprocess.run(["git", "clone", "-q", "-b", branch, str(project), str(check)], check=True)
        r = subprocess.run(["python3", "-m", "unittest", "-q"], cwd=check, capture_output=True, text=True)
        step("independent_branch_check", exit_code=r.returncode, calc=(check / "calc.py").read_text(),
             commits=git(check, "log", "--oneline", "-5"))
    step("repository_hooks_not_run", hook_ran=(root / "HOOK_RAN").exists())
    step("main_worktree_untouched", calc=(project / "calc.py").read_text(), head_branch=git(project, "branch", "--show-current"),
         status=git(project, "status", "--porcelain"))
    # Can sandboxed acceptance code reach Chadex's loopback operator API?
    probe = ["python3", "-c", f"import urllib.request,sys\ntry:\n urllib.request.urlopen('{url}/healthz', timeout=5); print('reached')\nexcept Exception as e:\n print('blocked', type(e).__name__); sys.exit(3)"]
    outside = subprocess.run(probe, capture_output=True, text=True)
    inside = subprocess.run(SANDBOX_PREFIX + probe, capture_output=True, text=True, cwd=project)
    step("sandbox_loopback_probe", outside=outside.stdout.strip() or outside.stderr.strip()[-200:],
         inside=inside.stdout.strip() or inside.stderr.strip()[-200:], inside_exit=inside.returncode)
    # ---------------- Direct Goal (no plan) ----------------
    ok, req = chatgpt.call("goal_run_start", {
        "project": proj_id, "title": "Fix add() directly",
        "objective": "add() subtracts; make it add. Keep the API unchanged.",
        "acceptance": [{"name": "unit tests", "argv": ["python3", "-m", "unittest", "-q"]}],
        "budget": {"max_iterations": 4, "max_minutes": 30, "agent_timeout_secs": 900}})
    dplan = (req.get("plan") or {}).get("plan_id")
    drev = req.get("revision") or {}
    ok_w, write_ok = chatgpt.call("read_files", {"project": proj_id, "items": [{"path": "calc.py"}]})
    step("direct_goal_request", ok=ok, state=req.get("state"), origin=(req.get("plan") or {}).get("origin"),
         plan_mode_active=(req.get("plan") or {}).get("plan_mode_active"), connection_still_usable=ok_w)
    ok_s, self_start = chatgpt.call("goal_run_start", {"plan_id": dplan})
    ok_o, self_op = chatgpt.call("plan_review", {"action": "approve_and_start", "plan_id": dplan,
                                                "revision": 1, "content_sha256": drev.get("content_sha256")})
    step("direct_goal_model_cannot_start", start_ok=ok_s, start_error=self_start.get("error_kind"),
         operator_ok=ok_o, operator_error=self_op.get("error_kind") if isinstance(self_op, dict) else self_op)
    ok, started2 = operator(helper, project, "plan_review", {"action": "approve_and_start", "plan_id": dplan,
                                                              "revision": 1, "mode": "codex",
                                                              "content_sha256": drev.get("content_sha256")})
    run2 = ((started2 or {}).get("goal_run") or {}).get("goal_run_id")
    step("operator_start_direct_goal", ok=ok, run_id=run2, error=None if ok else started2)
    ok, replay2 = chatgpt.call("goal_run_start", {"plan_id": dplan})
    step("chatgpt_finds_started_run", ok=ok, same=(replay2.get("goal_run") or {}).get("goal_run_id") == run2)
    t1 = time.time()
    ok, waited = chatgpt.call("goal_run_status", {"goal_run_id": run2, "wait_secs": 45})
    while ok and waited.get("goal_run", {}).get("state") in ("queued", "running") and time.time() - t1 < 900:
        ok, waited = chatgpt.call("goal_run_status", {"goal_run_id": run2, "wait_secs": 45})
    gr2 = waited.get("goal_run", {})
    step("direct_goal_final", state=gr2.get("state"), reason=gr2.get("state_reason"), branch=gr2.get("result_branch"),
         iteration=gr2.get("iteration"), seconds=round(time.time() - t1), polls_used_wait=True)
    if gr2.get("result_branch"):
        check2 = root / "verify-direct"
        subprocess.run(["git", "clone", "-q", "-b", gr2["result_branch"], str(project), str(check2)], check=True)
        r2 = subprocess.run(["python3", "-m", "unittest", "-q"], cwd=check2, capture_output=True, text=True)
        step("direct_goal_branch_check", exit_code=r2.returncode, calc=(check2 / "calc.py").read_text())
    # ---------------- Goal Mode (ChatGPT): ChatGPT edits, Chadex checks ----------------
    ok, creq = chatgpt.call("goal_run_start", {
        "project": proj_id, "title": "Fix add() in ChatGPT mode",
        "objective": "add() subtracts; make it add.",
        "acceptance": [{"name": "unit tests", "argv": ["python3", "-m", "unittest", "-q"]}]})
    cplan = (creq.get("plan") or {}).get("plan_id")
    ok, cstart = operator(helper, project, "plan_review", {"action": "approve_and_start", "plan_id": cplan,
                                                           "revision": 1, "mode": "chatgpt",
                                                           "content_sha256": (creq.get("revision") or {}).get("content_sha256")})
    crun = ((cstart or {}).get("goal_run") or {}).get("goal_run_id")
    check_duplicate_goal_start(helper, project, cplan, 1,
                               (creq.get("revision") or {}).get("content_sha256"), crun, "chatgpt")
    workspace = None
    t2 = time.time()
    while time.time() - t2 < 120:
        ok_s, cstat = chatgpt.call("goal_run_status", {"goal_run_id": crun})
        if (cstat.get("goal_run") or {}).get("waiting_for_chatgpt"):
            workspace = cstat["goal_run"].get("workspace_project")
            break
        time.sleep(1)
    # Round 1 without a fix. The first call returns before the checks finish;
    # calling again only shows round 1's result and does not start round 2.
    ok_c0, check0 = chatgpt.call("goal_check", {"goal_run_id": crun, "wait_secs": 0})
    time.sleep(3)
    ok_c1, check1 = chatgpt.call("goal_check", {"goal_run_id": crun, "wait_secs": 45})
    check1b = check1
    # ChatGPT edits the Goal's worktree with its normal tools, then checks.
    ok_rd, read = chatgpt.call("read_files", {"project": workspace, "items": [{"path": "calc.py"}],
                                             "include_read_revision": True})
    rev = next((int(i["output"]["read_revision"]) for i in read.get("items", [])
                if (i.get("output") or {}).get("read_revision") is not None), None)
    ok_wr, wrote = chatgpt.call("write_project_file", {"project": workspace, "path": "calc.py",
                                                      "content": "def add(a, b):\n    return a + b\n",
                                                      "overwrite": True, "expected_read_revision": rev})
    ok_c2, check2 = chatgpt.call("goal_check", {"goal_run_id": crun, "wait_secs": 45})
    cgr = check2.get("goal_run") or {}
    step("chatgpt_mode_goal", start_ok=ok, run_id=crun, mode=cgr.get("mode"), workspace=workspace,
         round1_achieved=check1.get("achieved"), round1_has_failures=bool(check1.get("failures")),
         round1_iteration=(check1.get("goal_run") or {}).get("iteration"),
         chatgpt_wrote_worktree=ok_wr, write_error=None if ok_wr else wrote,
         round2_achieved=check2.get("achieved"), state=cgr.get("state"), iteration=cgr.get("iteration"),
         branch=cgr.get("result_branch"), main_untouched=(project / "calc.py").read_text().strip().endswith("a - b"))

    # ---------------- Git readiness: a project without any commit ----------------
    fresh = root / "fresh"
    fresh.mkdir()
    (fresh / "notes.py").write_text("print('hi')\n")
    subprocess.run(["git", "-C", str(fresh), "init", "-q", "-b", "main"], check=True)
    helper.request("activateProject", {"path": str(fresh)})
    helper.request("configureLocalSetup")
    fresh_id = None
    for _ in range(20):
        ok, projects = chatgpt.call("list_projects", {})
        for p in (projects.get("projects") or []):
            if p.get("path") == str(fresh) or str(p.get("root", "")) == str(fresh):
                fresh_id = p.get("id") or p.get("project")
        if fresh_id:
            break
        time.sleep(1)
    ok, freq = chatgpt.call("goal_run_start", {
        "project": fresh_id, "title": "Anything", "objective": "x",
        "acceptance": [{"name": "t", "argv": ["true"]}]})
    fplan = (freq.get("plan") or {}).get("plan_id")
    ok_i, finspect = operator(helper, fresh, "plan_review", {"action": "inspect", "plan_id": fplan})
    readiness = (finspect or {}).get("git_readiness") or {}
    ok_s, fstart = operator(helper, fresh, "plan_review", {
        "action": "approve_and_start", "plan_id": fplan, "revision": 1, "mode": "codex",
        "content_sha256": (freq.get("revision") or {}).get("content_sha256")})
    step("git_readiness_no_commit", project=fresh_id, request_ok=ok,
         request_readiness_blocks=((freq.get("git_readiness") or {}).get("blocks_goal_start")),
         inspect_blocks=readiness.get("blocks_goal_start"), has_commit=readiness.get("has_commit"),
         how_to_commit=readiness.get("how_to_commit"), start_ok=ok_s,
         start_error=(fstart or {}).get("error_kind") if isinstance(fstart, dict) else fstart)
    # Emergency stop round trip.
    # The helper's active project is now `fresh`; operator calls go through it.
    ok, es = operator(helper, fresh, "goal_operator", {"action": "emergency_stop", "reason": "e2e"})
    ok2, rel = operator(helper, fresh, "goal_operator", {"action": "release_emergency_stop"})
    step("emergency_stop_roundtrip", engaged=es.get("emergency_stop_engaged"), released=rel.get("emergency_stop_engaged") is False)
    helper.close()
    by_name={item["step"]:item for item in report["steps"]}
    checks={
        "background_goal_succeeded":by_name.get("goal_final",{}).get("state")=="succeeded",
        "direct_goal_succeeded":by_name.get("direct_goal_final",{}).get("state")=="succeeded",
        "chatgpt_two_rounds":by_name.get("chatgpt_mode_goal",{}).get("round1_has_failures") is True and by_name.get("chatgpt_mode_goal",{}).get("round2_achieved") is True,
        "independent_branch_check":by_name.get("independent_branch_check",{}).get("exit_code")==0,
        "direct_branch_check":by_name.get("direct_goal_branch_check",{}).get("exit_code")==0,
        "plan_readonly_preserved":by_name.get("plan_mode_project_unchanged",{}).get("identical") is True,
        "repository_hooks_not_run":by_name.get("repository_hooks_not_run",{}).get("hook_ran") is False,
        "start_is_singleflight":all(item.get("passed") for item in report["steps"] if item["step"]=="goal_start_button_deduplication"),
        "uncommitted_project_blocked":by_name.get("git_readiness_no_commit",{}).get("start_ok") is False,
        "emergency_stop_roundtrip":by_name.get("emergency_stop_roundtrip",{}).get("engaged") is True and by_name.get("emergency_stop_roundtrip",{}).get("released") is True,
    }
    report["assertions"]=checks;report["passed"]=all(checks.values())
    OUT.write_text(json.dumps(report, indent=2, ensure_ascii=False))
    print("ROOT", root)
    if not report["passed"]:raise AssertionError("Plan/Goal E2E failures: "+str([name for name,passed in checks.items() if not passed]))


if __name__ == "__main__":
    main()
