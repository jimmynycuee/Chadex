#!/usr/bin/env python3
"""Classify a CI change set as docs-only or heavy.

Docs-only changes skip the macOS/Windows build and package jobs in
`.github/workflows/ci.yml`; the public-history / secret scan always runs.
The classification fails closed: an unknown base, a git error, or any path
outside the explicit allowlist selects the full (heavy) pipeline.

Release tags do not use this workflow; `.github/workflows/release.yml`
always runs its complete gate.
"""
from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path, PurePosixPath

# Paths that no build, test or package step reads. Keep this list explicit:
# UPSTREAM.md and LICENSE are bundled into the app, and ui-review/ holds
# visual-test references, so they are deliberately absent.
DOC_ONLY_PREFIXES = ("docs/", "graphify-out/")
DOC_ONLY_FILES = frozenset({
    "README.md",
    "CHANGELOG.md",
    "PHASES.md",
    "HANDOFF.md",
    ".graphifyignore",
})

ZERO_SHA = re.compile(r"^0+$")
MARKDOWN_LINK = re.compile(r"\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
EXTERNAL_LINK = re.compile(r"^(?:[a-zA-Z][a-zA-Z0-9+.-]*:|#|//)")
LINE_SUFFIX = re.compile(r":\d+(?:[-:]\d+)*$")


def is_doc_only_path(path: str) -> bool:
    return path in DOC_ONLY_FILES or path.startswith(DOC_ONLY_PREFIXES)


def is_docs_only(paths: list[str]) -> bool:
    # An empty change set (e.g. a merge with no tree diff) gets the full gate.
    return bool(paths) and all(is_doc_only_path(path) for path in paths)


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], check=True, capture_output=True, text=True
    ).stdout


def resolve_base(event: str, push_before: str, pr_base: str) -> str | None:
    if event == "pull_request":
        candidate = pr_base
    elif event == "push":
        candidate = push_before
    else:
        return None
    if not candidate or ZERO_SHA.match(candidate):
        return None
    try:
        git("cat-file", "-e", f"{candidate}^{{commit}}")
    except subprocess.CalledProcessError:
        # Force-pushed or otherwise unreachable base.
        return None
    return candidate


def broken_relative_links(root: Path, markdown_paths: list[str]) -> list[str]:
    problems = []
    for relative in markdown_paths:
        source = root / relative
        if not source.is_file():
            continue
        in_fence = False
        for number, line in enumerate(source.read_text(encoding="utf-8").splitlines(), 1):
            if line.lstrip().startswith("```"):
                in_fence = not in_fence
            if in_fence:
                continue
            for target in MARKDOWN_LINK.findall(line):
                if EXTERNAL_LINK.match(target):
                    continue
                target_path = target.split("#", 1)[0]
                if not target_path:
                    continue
                resolved = (source.parent / target_path).resolve()
                if not resolved.exists():
                    # Allow editor-style `path:line` source references.
                    resolved = (source.parent / LINE_SUFFIX.sub("", target_path)).resolve()
                if not resolved.exists():
                    problems.append(f"{relative}:{number}: missing link target {target}")
    return problems


def write_output(name: str, value: str) -> None:
    output = os.environ.get("GITHUB_OUTPUT")
    if output:
        with open(output, "a", encoding="utf-8") as handle:
            handle.write(f"{name}={value}\n")
    print(f"{name}={value}")


def classify(base: str | None, head: str) -> int:
    if base is None:
        print("No reliable diff base; running the full pipeline.")
        write_output("heavy", "true")
        return 0
    paths = changed_paths(base, head)
    docs_only = is_docs_only(paths)
    print(f"{len(paths)} changed path(s) since {base[:12]}; docs_only={str(docs_only).lower()}")
    for path in paths:
        print(f"  {'doc ' if is_doc_only_path(path) else 'code'} {path}")
    write_output("heavy", "false" if docs_only else "true")
    return 0


def check_docs(base: str | None, head: str) -> int:
    root = Path(git("rev-parse", "--show-toplevel").strip())
    if base is None:
        # Without a diff base, check every doc-only Markdown file instead.
        markdown = [
            path for path in git("ls-files", "*.md").splitlines()
            if is_doc_only_path(path)
        ]
        whitespace_ok = True
    else:
        markdown = [
            path for path in changed_paths(base, head)
            if PurePosixPath(path).suffix == ".md"
        ]
        whitespace_ok = subprocess.run(["git", "diff", "--check", base, head]).returncode == 0
    problems = broken_relative_links(root, markdown)
    for problem in problems:
        print(problem, file=sys.stderr)
    print(f"Checked {len(markdown)} Markdown file(s); whitespace_ok={str(whitespace_ok).lower()}")
    return 0 if whitespace_ok and not problems else 1


def changed_paths(base: str, head: str) -> list[str]:
    return [line for line in git("diff", "--name-only", base, head).splitlines() if line]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("classify", "docs-check"))
    parser.add_argument("--event", default=os.environ.get("GITHUB_EVENT_NAME", ""))
    parser.add_argument("--push-before", default="")
    parser.add_argument("--pr-base", default="")
    parser.add_argument("--head", default="HEAD")
    args = parser.parse_args()

    base = resolve_base(args.event, args.push_before, args.pr_base)
    if args.mode == "classify":
        return classify(base, args.head)
    return check_docs(base, args.head)


if __name__ == "__main__":
    raise SystemExit(main())
