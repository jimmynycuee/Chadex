#!/usr/bin/env python3
"""Measure repeated tools/list schema projection on isolated runtimes."""

import argparse
import hashlib
import http.client
import json
import tempfile
import time
from pathlib import Path
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from benchmark_cold_runtime import IsolatedHelper
from benchmark_workflow import parse_env_file, runtime_identity


def percentile(values, fraction):
    ordered = sorted(values)
    index = min(len(ordered) - 1, int((len(ordered) - 1) * fraction))
    return ordered[index]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--after", type=Path, required=True)
    parser.add_argument("--iterations", type=int, default=12)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    # This harness intentionally stays small and delegates runtime identity
    # parsing to the existing benchmark helper; each variant gets a clean temp
    # data directory and project.
    from benchmark_cold_runtime import timing_summary
    rows = []
    with tempfile.TemporaryDirectory(prefix="chadex-tools-list-") as root:
        root = Path(root)
        for label, app in (("before", args.before), ("after", args.after)):
            project = root / label / "project"
            project.mkdir(parents=True)
            (project / "README.md").write_text("# benchmark\n", encoding="utf-8")
            helper = IsolatedHelper(
                app.resolve() / "Contents/Helpers/chadex-helper",
                app.resolve() / "Contents/Resources",
                root / label / "data",
            )
            client = None
            try:
                helper.request("activateProject", {"path": str(project)})
                helper.request("configureLocalSetup")
                url, env_file, _ = runtime_identity(root / label / "data")
                parsed = __import__("urllib.parse", fromlist=["urlparse"]).urlparse(url)
                client = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=30)
                token = parse_env_file(env_file)["WEBCODEX_TOKEN"]
                samples = []
                for index in range(args.iterations + 1):
                    body = json.dumps({"jsonrpc":"2.0", "id":index + 1, "method":"tools/list", "params":{}}, separators=(",", ":")).encode()
                    started = time.perf_counter()
                    client.request("POST", "/mcp", body=body, headers={"Authorization":f"Bearer {token}", "Content-Type":"application/json", "Accept":"application/json"})
                    response = client.getresponse()
                    raw = response.read()
                    elapsed = (time.perf_counter() - started) * 1000
                    payload = json.loads(raw)
                    if response.status != 200 or "error" in payload:
                        raise RuntimeError(f"{label} tools/list failed: HTTP {response.status}")
                    tools = payload.get("result", {}).get("tools", [])
                    if index:
                        samples.append({"ms":elapsed, "response_bytes":len(raw), "tool_count":len(tools)})
                rows.append({"variant":label, "binary_sha256":hashlib.sha256((app.resolve() / "Contents/Helpers/chadex-helper").read_bytes()).hexdigest(), "samples":samples, "summary_ms":timing_summary([row["ms"] for row in samples])})
            finally:
                if client: client.close()
                helper.close()
    report = {"surface":"isolated loopback MCP; tools/list only; not connected workflow", "rows":rows}
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
