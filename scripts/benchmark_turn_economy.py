#!/usr/bin/env python3
"""Compare equivalent sequential/batched workflows on an isolated real runtime.

No live preferences, project, credentials or app bundle are modified. This is
local MCP timing, NOT ChatGPT end-to-end timing or a simulated network speedup.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import time
import tempfile
import sys
sys.dont_write_bytecode = True
from pathlib import Path
from benchmark_cold_runtime import IsolatedHelper, timing_summary
from benchmark_workflow import McpClient, helper_paths, parse_env_file, runtime_identity


class MeasuredClient(McpClient):
    def __init__(self, url, token):
        super().__init__(url, token)
        self.samples = []

    def invoke(self, name, arguments, expect_success=True):
        self.counter += 1
        body = json.dumps({"jsonrpc": "2.0", "id": self.counter, "method": "tools/call",
                           "params": {"name": name, "arguments": arguments}}, separators=(",", ":")).encode()
        started_at_ms = time.time_ns() // 1_000_000
        start = time.perf_counter()
        self.connection.request("POST", "/mcp", body=body, headers={
            "Authorization": f"Bearer {self.token}", "Content-Type": "application/json", "Accept": "application/json"})
        response = self.connection.getresponse()
        server_trace_id = response.getheader("x-chadex-trace-id")
        headers_ms = (time.perf_counter() - start) * 1000
        raw = response.read()
        elapsed = (time.perf_counter() - start) * 1000
        payload = json.loads(raw)
        result = payload.get("result", {})
        structured = result.get("structuredContent", {})
        output = structured.get("output", {})
        success = response.status == 200 and "error" not in payload and not result.get("isError", False) and structured.get("success") is True and output.get("failed_count", 0) == 0
        self.samples.append({"tool": name, "request_bytes": len(body), "response_bytes": len(raw),
                             "started_at_ms": started_at_ms,
                             "request_id_hash": hashlib.sha256(str(self.counter).encode()).hexdigest(),
                             "server_trace_id": server_trace_id,
                             "headers_ms": headers_ms, "stream_ms": elapsed - headers_ms, "total_ms": elapsed})
        if expect_success and not success:
            # Error metadata only; never print response bodies or credentials.
            rpc_error = payload.get("error", {})
            message = str(rpc_error.get("message") or "")[:300]
            raise RuntimeError(
                f"{name} failed: HTTP {response.status}, rpc={rpc_error.get('code')}, "
                f"kind={output.get('error_kind')}, message={message}"
            )
        return output, success


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helper", type=Path, help="Explicit candidate helper; packaged helper otherwise")
    parser.add_argument("--resources", type=Path, help="Explicit runtime resources; packaged resources otherwise")
    parser.add_argument("--iterations", type=int, default=30)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.iterations < 1: parser.error("iterations must be positive")
    repo = Path(__file__).resolve().parents[1]
    packaged_helper, packaged_resources = helper_paths(repo)
    binary = (args.helper or packaged_helper).resolve()
    resources = (args.resources or packaged_resources).resolve()
    rows = []
    guards = {}
    with tempfile.TemporaryDirectory(prefix="chadex-turn-economy-") as directory:
        root = Path(directory)
        project = root / "project"
        project.mkdir()
        for name in ["a.txt", "b.txt"]:
            (project / name).write_text("alpha\nbeta\ngamma\n")
        outside = root / "outside.txt"
        outside.write_text("OUTSIDE_TEST_MARKER\n")
        (project / "escape.txt").symlink_to(outside)
        helper = IsolatedHelper(binary, resources, root / "data")
        client = None
        try:
            helper.request("activateProject", {"path": str(project)})
            started = time.perf_counter()
            helper.request("configureLocalSetup")
            cold_setup_ms = (time.perf_counter() - started) * 1000
            url, env_path, project_id = runtime_identity(root / "data")
            client = MeasuredClient(url, parse_env_file(env_path)["WEBCODEX_TOKEN"])

            def read(paths, revision=False):
                return client.invoke("read_files", {"project": project_id, "items": [{"path": path} for path in paths],
                                                    "include_read_revision": revision, "max_result_bytes": 8192})[0]

            def workflow(scenario, batch):
                paths = ["a.txt", "b.txt"]
                groups = [paths] if batch else [[path] for path in paths]
                if scenario == "search_explain":
                    for group in groups:
                        client.invoke("search_project_texts", {"project": project_id, "queries": [
                            {"path": path, "pattern": "alpha", "pattern_mode": "literal", "limit": 5} for path in group], "max_result_bytes": 8192})
                    for group in groups: read(group)
                elif scenario == "multi_read":
                    for group in groups: read(group)
                else:
                    outputs = [read(group, True) for group in groups]
                    revisions = {item["path"]: item["output"]["read_revision"] for output in outputs for item in output["items"]}
                    for group in groups:
                        client.invoke("apply_text_edits", {"project": project_id, "changes": [
                            {"path": path, "old_text": "alpha", "new_text": "changed", "expected_read_revision": revisions[path]} for path in group]})
                    for group in groups:
                        result = read(group)
                        assert all("changed" in item["output"]["text"] for item in result["items"])

            for i in range(args.iterations + 1):
                for batch in ([False, True] if i % 2 == 0 else [True, False]):
                    for scenario in ["search_explain", "multi_read", "read_edit_verify"]:
                        for name in ["a.txt", "b.txt"]:
                            (project / name).write_text("alpha\nbeta\ngamma\n")
                        first = len(client.samples)
                        start = time.perf_counter()
                        workflow(scenario, batch)
                        elapsed = (time.perf_counter() - start) * 1000
                        if i:
                            samples = client.samples[first:]
                            rows.append({"iteration": i, "scenario": scenario, "batch": batch,
                                         "calls": len(samples), "total_ms": elapsed,
                                         "request_bytes": sum(x["request_bytes"] for x in samples),
                                         "response_bytes": sum(x["response_bytes"] for x in samples),
                                         "tool_samples": samples})
            old = read(["a.txt"], True)["items"][0]["output"]["read_revision"]
            (project / "a.txt").write_text("concurrent-change\n")
            _, accepted = client.invoke("apply_text_edits", {"project": project_id, "changes": [{
                "path": "a.txt", "old_text": "concurrent-change", "new_text": "bad", "expected_read_revision": old}]}, False)
            guards["stale_write_rejected"] = not accepted and (project / "a.txt").read_text() == "concurrent-change\n"
            for path in ["../outside.txt", "escape.txt"]:
                output, accepted = client.invoke("read_files", {"project": project_id, "items": [{"path": path}]}, False)
                guards["traversal_rejected" if path.startswith("..") else "symlink_escape_rejected"] = not accepted and "OUTSIDE_TEST_MARKER" not in json.dumps(output)
            assert all(guards.values()), guards
        finally:
            if client: client.close()
            helper.close()
    summaries = {}
    for scenario in ["search_explain", "multi_read", "read_edit_verify"]:
        summaries[scenario] = {str(batch).lower(): timing_summary([row["total_ms"] for row in rows if row["scenario"] == scenario and row["batch"] == batch]) for batch in [False, True]}
    report = {"schema": 1, "surface": "isolated local MCP; no tunnel or model", "comparison": "equivalent sequential vs batched tool workflows; alternating order; one warmup",
              "helper_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "runtime_sha256": {name: hashlib.sha256((resources / "chadex-runtime" / name).read_bytes()).hexdigest() for name in ["chadex-runtime-cli", "chadex-runtime-server", "chadex-runtime-runner"]},
              "iterations_per_variant": args.iterations,
              "cold_setup_ms_single_sample": cold_setup_ms, "guards": guards, "summary_ms": summaries, "rows": rows}
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"summary_ms": summaries, "guards": guards}, indent=2))


if __name__ == "__main__":
    main()
