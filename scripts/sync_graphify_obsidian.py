#!/usr/bin/env python3
"""Generate a curated Obsidian knowledge layer from Graphify output.

This intentionally does not mirror every Graphify symbol into Obsidian. It
keeps Graphify as the code-structure source of truth and emits a bounded set of
architecture/component/file notes that link into human-authored Phase and
Decision notes.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from collections import Counter, defaultdict
from dataclasses import dataclass
from pathlib import Path
import re
import subprocess
import sys
from typing import Iterable

SYNC_VERSION = 1
GENERATED_BY = "chadex-graphify-obsidian-sync"
GENERATED_ROOT = Path("Architecture/Generated/Graphify")
CANVAS_PATH = Path("Architecture/Graphify Architecture.canvas")

PRODUCTION_PREFIXES = (
    "Sources/ChadexApp/",
    "rust-helper/src/",
    "runtime-engine/src/",
    "runtime-engine/crates/",
)
EXCLUDED_PARTS = ("/tests/", "/test/", "vendor/", "benchmarks/", "graphify-out/")


@dataclass(frozen=True)
class ComponentSpec:
    title: str
    summary: str
    exact_files: tuple[str, ...] = ()
    prefixes: tuple[str, ...] = ()
    label_terms: tuple[str, ...] = ()
    phases: tuple[str, ...] = ()


COMPONENTS: tuple[ComponentSpec, ...] = (
    ComponentSpec(
        "Desktop App State",
        "SwiftUI/AppModel state projection and native helper coordination.",
        exact_files=("Sources/ChadexApp/AppModel.swift", "Sources/ChadexApp/HelperClient.swift"),
        phases=("P5A Perceived Latency UI", "P11 Concurrent Isolated Execution", "P14 Recovery and Runtime Closure"),
    ),
    ComponentSpec(
        "Rust Helper Bridge",
        "Native bridge between the desktop app, runtime lifecycle, and MCP tunnel.",
        exact_files=("rust-helper/src/runtime_bridge.rs", "rust-helper/src/main.rs"),
        phases=("P1 Execution Tracing", "P5C Cold-launch Attribution", "P6 Runtime Boundary and Independence", "P14 Recovery and Runtime Closure"),
    ),
    ComponentSpec(
        "Tunnel Lifecycle",
        "Tunnel process ownership, readiness, switching, and supervisor lifetime.",
        exact_files=("rust-helper/src/chadex_core/tunnel.rs", "rust-helper/src/tunnel_supervisor.rs"),
        phases=("P6 Runtime Boundary and Independence", "P14 Recovery and Runtime Closure", "P16C First-tool Variance Validation"),
    ),
    ComponentSpec(
        "Verification Boundary",
        "Epoch-fenced request/response evidence used to verify the current connection and project.",
        prefixes=("rust-helper/src/chadex_core/verification",),
        phases=("P14 Recovery and Runtime Closure", "P15 Execution Attribution"),
    ),
    ComponentSpec(
        "MCP Server",
        "Server ingress, protocol/auth handling, request tracing, and dispatch into ToolRuntime.",
        exact_files=("runtime-engine/src/mcp.rs", "runtime-engine/src/tool_request_trace.rs"),
        phases=("P1 Execution Tracing", "P15 Execution Attribution", "P16A First-tool Differential Diagnosis", "P16C First-tool Variance Validation"),
    ),
    ComponentSpec(
        "Adaptive Tool Surface",
        "Canonical tool definitions, startup direct surface, discovery, and gateway exposure.",
        exact_files=(
            "runtime-engine/src/tool_runtime/tool_definition.rs",
            "runtime-engine/src/tool_runtime/tool_catalog.rs",
            "runtime-engine/src/tool_runtime/surface.rs",
            "runtime-engine/src/tool_runtime/discovery_tools.rs",
        ),
        phases=("P16A First-tool Differential Diagnosis", "P16B First-tool Surface Optimization", "P16C First-tool Variance Validation"),
    ),
    ComponentSpec(
        "Project Authority",
        "Project resolution, Runner authorization, and exact project identity fences.",
        exact_files=(
            "runtime-engine/src/tool_runtime/project_resolution.rs",
            "runtime-engine/src/tool_runtime/runner_authorization.rs",
            "runtime-engine/src/tool_runtime/projects.rs",
        ),
        phases=("P2 Read Hot Path", "P3 Read and Mutation Hot Path", "P6 Runtime Boundary and Independence"),
    ),
    ComponentSpec(
        "Coding Task Orchestration",
        "work_on_project/coding-task bootstrap, context preparation, resume, and handoff.",
        exact_files=("runtime-engine/src/tool_runtime/coding_task.rs",),
        phases=("P15 Execution Attribution", "P16A First-tool Differential Diagnosis"),
    ),
    ComponentSpec(
        "Task Executor",
        "Deterministic execute_task execution, package planning, validation, recovery, and receipts.",
        prefixes=("runtime-engine/src/tool_runtime/chadex_task_executor",),
        phases=("P9 Deterministic Task Executor", "P10 Adaptive Task Packaging", "P11 Concurrent Isolated Execution", "P12 Execution Semantics Foundation", "P13 Long-running Execution Resilience", "P14 Recovery and Runtime Closure"),
    ),
    ComponentSpec(
        "Execution Workspace",
        "Managed worktree/isolation lifecycle, integration, reconciliation, and cleanup ownership.",
        exact_files=("runtime-engine/src/tool_runtime/execution_workspace.rs",),
        phases=("P11 Concurrent Isolated Execution", "P12 Execution Semantics Foundation", "P14 Recovery and Runtime Closure"),
    ),
    ComponentSpec(
        "Runner",
        "Runner-side process, filesystem, project, worktree, and command execution authority.",
        prefixes=("runtime-engine/crates/chadex-runtime-runner/",),
        phases=("P4 Short-command Runner Latency", "P4B Typed Runner-native Search", "P11 Concurrent Isolated Execution", "P13 Long-running Execution Resilience", "P14 Recovery and Runtime Closure"),
    ),
    ComponentSpec(
        "Graphify Planning",
        "Optional Graphify evidence used for navigation and dependency-aware package planning.",
        label_terms=("graphify",),
        phases=("P10 Adaptive Task Packaging", "P11 Concurrent Isolated Execution"),
    ),
    ComponentSpec(
        "Performance Attribution",
        "Content-free timing boundaries used to distinguish local runtime cost from pre-Server latency.",
        exact_files=("runtime-engine/src/tool_request_trace.rs",),
        label_terms=("performance", "trace"),
        phases=("P1 Execution Tracing", "P15 Execution Attribution", "P16A First-tool Differential Diagnosis", "P16C First-tool Variance Validation"),
    ),
)


def is_production_file(path: str) -> bool:
    if not path or not path.startswith(PRODUCTION_PREFIXES):
        return False
    lowered = "/" + path.lower()
    name = Path(path).name.lower()
    if any(part in lowered for part in EXCLUDED_PARTS):
        return False
    if Path(path).suffix.lower() not in {".rs", ".swift"}:
        return False
    if name.startswith("fake_") or name.startswith("test_"):
        return False
    if "_test." in name or "_tests." in name or name in {"test_support.rs", "validation_tree_helper.rs"}:
        return False
    return True


def node_matches_component(node: dict, spec: ComponentSpec) -> bool:
    source = str(node.get("source_file") or "")
    label = str(node.get("label") or "").lower()
    if source in spec.exact_files:
        return True
    if any(source.startswith(prefix) for prefix in spec.prefixes):
        return True
    return is_production_file(source) and any(term in label for term in spec.label_terms)


def safe_note_stem(text: str) -> str:
    text = re.sub(r'[\\/:*?"<>|#^\[\]]+', " - ", text)
    text = re.sub(r"\s+", " ", text).strip(" .-")
    return text or "Untitled"


def file_note_name(source_file: str) -> str:
    digest = hashlib.sha1(source_file.encode("utf-8")).hexdigest()[:7]
    base = safe_note_stem(Path(source_file).name)
    return f"Graphify File - {base} - {digest}"


def markdown_link(note: str, label: str | None = None) -> str:
    return f"[[{note}|{label}]]" if label else f"[[{note}]]"


def git_head(source_root: Path) -> str | None:
    try:
        completed = subprocess.run(
            ["git", "-C", str(source_root), "rev-parse", "HEAD"],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    return completed.stdout.strip() or None


def yaml_header(kind: str, graph_commit: str | None, source_head: str | None, extra: Iterable[tuple[str, str]] = ()) -> str:
    lines = [
        "---",
        f"type: {json.dumps(kind)}",
        "generated: true",
        f"generated_by: {json.dumps(GENERATED_BY)}",
        f"sync_version: {SYNC_VERSION}",
        f"graph_built_at_commit: {json.dumps(graph_commit or 'unknown')}",
        f"source_head: {json.dumps(source_head or 'unknown')}",
    ]
    for key, value in extra:
        lines.append(f"{key}: {json.dumps(value)}")
    lines += ["---", ""]
    return "\n".join(lines)


def load_graph(path: Path) -> dict:
    with path.open("r", encoding="utf-8") as handle:
        data = json.load(handle)
    if not isinstance(data.get("nodes"), list) or not isinstance(data.get("links"), list):
        raise ValueError("Graphify graph.json must contain nodes[] and links[].")
    return data


def build_outputs(graph: dict, source_head: str | None, max_files: int) -> tuple[dict[Path, str], dict]:
    nodes = graph["nodes"]
    links = graph["links"]
    graph_commit = graph.get("built_at_commit")

    by_id = {str(node.get("id")): node for node in nodes if node.get("id") is not None}
    node_degree: Counter[str] = Counter()
    file_degree: Counter[str] = Counter()

    for link in links:
        source_id = str(link.get("source") or "")
        target_id = str(link.get("target") or "")
        if source_id:
            node_degree[source_id] += 1
        if target_id:
            node_degree[target_id] += 1
        for node_id in (source_id, target_id):
            source_file = str((by_id.get(node_id) or {}).get("source_file") or "")
            if is_production_file(source_file):
                file_degree[source_file] += 1

    component_nodes: dict[str, set[str]] = {}
    node_components: defaultdict[str, set[str]] = defaultdict(set)
    for spec in COMPONENTS:
        matched = {
            str(node["id"])
            for node in nodes
            if node.get("id") is not None and node_matches_component(node, spec)
        }
        component_nodes[spec.title] = matched
        for node_id in matched:
            node_components[node_id].add(spec.title)

    # Keep the generated graph bounded. Exact component files are stable anchors;
    # broad prefix/label components contribute only their highest-degree files.
    selected_files: set[str] = set()
    for spec in COMPONENTS:
        selected_files.update(
            path for path in spec.exact_files
            if is_production_file(path) and path in file_degree
        )
        matched_files = {
            str(by_id[node_id].get("source_file") or "")
            for node_id in component_nodes[spec.title]
            if is_production_file(str(by_id[node_id].get("source_file") or ""))
        }
        selected_files.update(
            sorted(matched_files, key=lambda path: (-file_degree[path], path))[:2]
        )

    if len(selected_files) > max_files:
        selected_files = set(
            sorted(selected_files, key=lambda path: (-file_degree[path], path))[:max_files]
        )
    else:
        ranked_files = [
            path for path, _ in file_degree.most_common()
            if path not in selected_files and is_production_file(path)
        ]
        selected_files.update(ranked_files[: max_files - len(selected_files)])

    file_nodes: defaultdict[str, list[str]] = defaultdict(list)
    for node_id, node in by_id.items():
        source_file = str(node.get("source_file") or "")
        if source_file in selected_files:
            file_nodes[source_file].append(node_id)

    file_neighbors: defaultdict[str, Counter[str]] = defaultdict(Counter)
    component_relations: Counter[tuple[str, str]] = Counter()
    confidence_counts: Counter[str] = Counter()
    production_links = 0

    for link in links:
        source_id = str(link.get("source") or "")
        target_id = str(link.get("target") or "")
        source_node = by_id.get(source_id)
        target_node = by_id.get(target_id)
        if not source_node or not target_node:
            continue
        source_file = str(source_node.get("source_file") or "")
        target_file = str(target_node.get("source_file") or "")
        if is_production_file(source_file) and is_production_file(target_file):
            production_links += 1
            confidence_counts[str(link.get("confidence") or "UNKNOWN")] += 1
        if source_file in selected_files and target_file in selected_files and source_file != target_file:
            file_neighbors[source_file][target_file] += 1
            file_neighbors[target_file][source_file] += 1
        for left in node_components.get(source_id, ()):
            for right in node_components.get(target_id, ()):
                if left != right:
                    component_relations[(left, right)] += 1

    file_note_by_path = {path: file_note_name(path) for path in sorted(selected_files)}
    outputs: dict[Path, str] = {}

    for spec in COMPONENTS:
        matched_ids = component_nodes[spec.title]
        matched_files = sorted({
            str(by_id[node_id].get("source_file") or "")
            for node_id in matched_ids
            if str(by_id[node_id].get("source_file") or "") in selected_files
        })
        symbols = sorted(
            matched_ids,
            key=lambda node_id: (-node_degree[node_id], str(by_id[node_id].get("label") or "")),
        )[:18]
        related_components = Counter()
        for (left, right), count in component_relations.items():
            if left == spec.title:
                related_components[right] += count
            elif right == spec.title:
                related_components[left] += count

        body = [yaml_header("graphify_component", graph_commit, source_head, (("component", spec.title),))]
        body += [f"# {spec.title}", "", spec.summary, ""]
        body += ["## Related phases", ""]
        body += [f"- {markdown_link(phase)}" for phase in spec.phases] or ["- none"]
        body += ["", "## Source files", ""]
        body += [
            f"- {markdown_link(file_note_by_path[path], path)}"
            for path in matched_files
        ] or ["- No production source file matched the current Graphify graph."]
        body += ["", "## High-degree symbols", ""]
        for node_id in symbols:
            node = by_id[node_id]
            body.append(
                f"- `{node.get('label') or node_id}` — "
                f"`{node.get('source_file') or '?'}:{node.get('source_location') or '?'}` "
                f"(degree {node_degree[node_id]})"
            )
        if not symbols:
            body.append("- none")
        body += ["", "## Graph neighbors", ""]
        for other, count in related_components.most_common(10):
            body.append(f"- {markdown_link(other)} — {count} observed Graphify relations")
        if not related_components:
            body.append("- none")
        body += ["", "> Generated from Graphify. Edit the Phase/Decision notes, not this file; reruns replace this generated note.", ""]
        outputs[GENERATED_ROOT / "Components" / f"{safe_note_stem(spec.title)}.md"] = "\n".join(body)

    for source_file in sorted(selected_files):
        ids = sorted(
            file_nodes[source_file],
            key=lambda node_id: (-node_degree[node_id], str(by_id[node_id].get("label") or "")),
        )
        related_components = sorted({
            component
            for node_id in ids
            for component in node_components.get(node_id, ())
        })
        body = [yaml_header("graphify_file", graph_commit, source_head, (("source_file", source_file),))]
        body += [f"# {source_file}", "", "Graphify-selected production source file.", ""]
        body += ["## Components", ""]
        body += [f"- {markdown_link(component)}" for component in related_components] or ["- none"]
        body += ["", "## Important symbols", ""]
        for node_id in ids[:16]:
            node = by_id[node_id]
            body.append(
                f"- `{node.get('label') or node_id}` "
                f"(`{node.get('source_location') or '?'}`, degree {node_degree[node_id]})"
            )
        if not ids:
            body.append("- none")
        body += ["", "## Connected generated files", ""]
        for neighbor, count in file_neighbors[source_file].most_common(8):
            body.append(f"- {markdown_link(file_note_by_path[neighbor], neighbor)} — {count} relations")
        if not file_neighbors[source_file]:
            body.append("- none")
        body += ["", "> Generated from Graphify; reruns replace this file.", ""]
        outputs[GENERATED_ROOT / "Files" / f"{file_note_by_path[source_file]}.md"] = "\n".join(body)

    mismatch = bool(graph_commit and source_head and graph_commit != source_head)
    snapshot = [yaml_header("graphify_snapshot", graph_commit, source_head)]
    snapshot += [
        "# Graphify Snapshot",
        "",
        f"- Nodes: **{len(nodes):,}**",
        f"- Links: **{len(links):,}**",
        f"- Production-to-production links: **{production_links:,}**",
        f"- Curated components: **{len(COMPONENTS)}**",
        f"- Curated production files: **{len(selected_files)}**",
        f"- Graph metadata commit: `{graph_commit or 'unknown'}`",
        f"- Source HEAD at sync: `{source_head or 'unknown'}`",
        f"- Metadata/source mismatch: **{'yes' if mismatch else 'no'}**",
        "",
        "## Confidence distribution for production links",
        "",
    ]
    for confidence, count in sorted(confidence_counts.items()):
        snapshot.append(f"- {confidence}: {count:,}")
    snapshot += [
        "",
        "## Architecture components",
        "",
        *[f"- {markdown_link(spec.title)}" for spec in COMPONENTS],
        "",
        "## Development history",
        "",
        "- [[Phases Overview]]",
        "- [[Chadex Development Timeline]]",
        "",
        "> Graphify remains the code graph source of truth. This Obsidian layer is intentionally curated and bounded.",
        "",
    ]
    if mismatch:
        snapshot += [
            "> [!warning] Graph metadata commit differs from source HEAD",
            "> Graphify may leave graph metadata unchanged when an update produces no topology delta. Treat the graph as current only after the normal Graphify freshness/update checks have been run.",
            "",
        ]
    outputs[GENERATED_ROOT / "Graphify Snapshot.md"] = "\n".join(snapshot)

    index_body = [yaml_header("graphify_index", graph_commit, source_head)]
    index_body += [
        "# Graphify Knowledge Index",
        "",
        "Central generated bridge between Graphify's code graph and Chadex's human-authored development history.",
        "",
        "## Components",
        "",
        *[f"- {markdown_link(spec.title)}" for spec in COMPONENTS],
        "",
        "## Files",
        "",
        *[
            f"- {markdown_link(file_note_by_path[path], path)}"
            for path in sorted(selected_files)
        ],
        "",
        "## Human-authored knowledge",
        "",
        "- [[Chadex]]",
        "- [[Architecture Overview]]",
        "- [[Phases Overview]]",
        "- [[Chadex Development Timeline]]",
        "- [[Decisions]]",
        "",
    ]
    outputs[GENERATED_ROOT / "Graphify Knowledge Index.md"] = "\n".join(index_body)

    canvas_nodes = []
    canvas_edges = []
    central_id = "snapshot"
    canvas_nodes.append({
        "id": central_id,
        "type": "file",
        "file": str(GENERATED_ROOT / "Graphify Snapshot.md"),
        "x": -220,
        "y": -160,
        "width": 440,
        "height": 320,
    })
    ring_radius = 1500
    component_ids: dict[str, str] = {}
    for index, spec in enumerate(COMPONENTS):
        angle = (2 * math.pi * index / len(COMPONENTS)) - math.pi / 2
        node_id = "component-" + hashlib.sha1(spec.title.encode("utf-8")).hexdigest()[:10]
        component_ids[spec.title] = node_id
        canvas_nodes.append({
            "id": node_id,
            "type": "file",
            "file": str(GENERATED_ROOT / "Components" / f"{safe_note_stem(spec.title)}.md"),
            "x": int(math.cos(angle) * ring_radius) - 210,
            "y": int(math.sin(angle) * ring_radius) - 130,
            "width": 420,
            "height": 260,
        })
        canvas_edges.append({
            "id": f"snapshot-{index}",
            "fromNode": central_id,
            "toNode": node_id,
        })

    for edge_index, ((left, right), count) in enumerate(component_relations.most_common(24)):
        if left not in component_ids or right not in component_ids:
            continue
        canvas_edges.append({
            "id": f"relation-{edge_index}",
            "fromNode": component_ids[left],
            "toNode": component_ids[right],
            "label": str(count),
        })

    outputs[CANVAS_PATH] = json.dumps(
        {"nodes": canvas_nodes, "edges": canvas_edges},
        ensure_ascii=False,
        indent=2,
        sort_keys=False,
    ) + "\n"

    stats = {
        "nodes": len(nodes),
        "links": len(links),
        "components": len(COMPONENTS),
        "selected_files": len(selected_files),
        "graph_commit": graph_commit,
        "source_head": source_head,
        "metadata_source_mismatch": mismatch,
    }
    return outputs, stats


def managed_generated_files(obsidian_root: Path) -> set[Path]:
    root = obsidian_root / GENERATED_ROOT
    if not root.exists():
        return set()
    managed = set()
    for path in root.rglob("*.md"):
        try:
            head = path.read_text(encoding="utf-8")[:512]
        except OSError:
            continue
        if f"generated_by: \"{GENERATED_BY}\"" in head:
            managed.add(path.relative_to(obsidian_root))
    return managed


def sync_outputs(obsidian_root: Path, outputs: dict[Path, str], *, check: bool, dry_run: bool) -> tuple[list[Path], list[Path]]:
    changed: list[Path] = []
    expected_generated = {path for path in outputs if path.is_relative_to(GENERATED_ROOT)}
    stale = sorted(managed_generated_files(obsidian_root) - expected_generated)

    for relative, content in outputs.items():
        target = obsidian_root / relative
        existing = None
        try:
            existing = target.read_text(encoding="utf-8")
        except FileNotFoundError:
            pass
        if existing != content:
            changed.append(relative)
            if not check and not dry_run:
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(content, encoding="utf-8")

    if not check and not dry_run:
        for relative in stale:
            (obsidian_root / relative).unlink()

    return changed, stale


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--graph", default="graphify-out/graph.json", help="Graphify graph.json")
    parser.add_argument("--source-root", default=".", help="Chadex source repository root")
    parser.add_argument(
        "--obsidian-root",
        default=os.environ.get("CHADEX_OBSIDIAN_PROJECT"),
        help="Obsidian Chadex project root (or CHADEX_OBSIDIAN_PROJECT)",
    )
    parser.add_argument("--max-files", type=int, default=48, help="Maximum curated production file notes")
    parser.add_argument("--check", action="store_true", help="Exit nonzero if generated output is not current")
    parser.add_argument("--dry-run", action="store_true", help="Show what would change without writing")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if not args.obsidian_root:
        print("error: pass --obsidian-root or set CHADEX_OBSIDIAN_PROJECT", file=sys.stderr)
        return 2
    if args.max_files < 1 or args.max_files > 200:
        print("error: --max-files must be between 1 and 200", file=sys.stderr)
        return 2

    graph_path = Path(args.graph).expanduser().resolve()
    source_root = Path(args.source_root).expanduser().resolve()
    obsidian_root = Path(args.obsidian_root).expanduser().resolve()
    if not graph_path.is_file():
        print(f"error: Graphify graph not found: {graph_path}", file=sys.stderr)
        return 2
    if not obsidian_root.is_dir():
        print(f"error: Obsidian Chadex project not found: {obsidian_root}", file=sys.stderr)
        return 2

    graph = load_graph(graph_path)
    outputs, stats = build_outputs(graph, git_head(source_root), args.max_files)
    changed, stale = sync_outputs(obsidian_root, outputs, check=args.check, dry_run=args.dry_run)

    print(json.dumps({
        **stats,
        "outputs": len(outputs),
        "changed": len(changed),
        "stale_generated": len(stale),
        "mode": "check" if args.check else ("dry-run" if args.dry_run else "write"),
    }, ensure_ascii=False, indent=2))

    if changed:
        print("changed:")
        for path in changed:
            print(f"  {path}")
    if stale:
        print("stale generated:")
        for path in stale:
            print(f"  {path}")

    return 1 if args.check and (changed or stale) else 0


if __name__ == "__main__":
    raise SystemExit(main())
