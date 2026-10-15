#!/usr/bin/env python3
"""Read-only manifest generator for an exact local dependency tree.

No install, package code execution or network. Review the returned project-relative
path and both digests in Chadex before approving the Plan/acceptance revision.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat


def manifest(project, dependency, lockfile):
    root = Path(project).resolve(strict=True)
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    def open_relative(relative, directory=False):
        path = Path(relative)
        if path.is_absolute() or not path.parts or any(p in ('.', '..', '.git') for p in path.parts):
            raise ValueError('Expected a project-relative path without traversal or .git')
        fd = os.open(root, flags)
        try:
            for part in path.parts[:-1]:
                child = os.open(part, flags, dir_fd=fd)
                os.close(fd); fd = child
            return os.open(path.parts[-1], flags if directory else os.O_RDONLY | os.O_NOFOLLOW, dir_fd=fd)
        finally:
            os.close(fd)
    fd = open_relative(lockfile)
    try:
        metadata = os.fstat(fd)
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > 16 * 1024 * 1024:
            raise ValueError('Expected a bounded regular lockfile')
        digest = hashlib.sha256()
        while True:
            block = os.read(fd, 1024 * 1024)
            if not block: break
            digest.update(block)
    finally:
        os.close(fd)
    # Use the same fixed, versioned algorithm as the Runner, without executing
    # its preparation entrypoint or any code from the dependency directory.
    source = (Path(__file__).resolve().parents[1] / 'runtime-engine/src/tool_runtime/goal_environment.rs').read_text()
    code = re.search(r'const PREPARE_INPUTS: &str = r#"(.*?)"#;', source, re.S).group(1)
    library = code[code.index('import stat,posixpath'):code.index('dependency_copies=[]')]
    scope = {'os': os, 'hashlib': hashlib, 'json': json, 'flags': flags}
    exec(compile(library, '<trusted-goal-dependency-digest>', 'exec'), scope)
    fd = open_relative(dependency, directory=True)
    try:
        tree_hash = scope['tree_digest'](fd)
    finally:
        os.close(fd)
    return {'path': str(Path(dependency)), 'provenance': f'sha256:{digest.hexdigest()}:{Path(lockfile)}:{tree_hash}'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project', required=True)
    parser.add_argument('--dependency', required=True)
    parser.add_argument('--lockfile', required=True)
    args = parser.parse_args()
    print(json.dumps(manifest(args.project, args.dependency, args.lockfile), indent=2))
