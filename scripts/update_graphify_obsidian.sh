#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
OBSIDIAN_ROOT=${1:-${CHADEX_OBSIDIAN_PROJECT:-}}

if [ -z "$OBSIDIAN_ROOT" ]; then
  echo "usage: $0 /path/to/Obsidian/Chadex" >&2
  echo "or set CHADEX_OBSIDIAN_PROJECT" >&2
  exit 2
fi

if ! command -v graphify >/dev/null 2>&1; then
  echo "graphify is not available on PATH" >&2
  exit 2
fi

cd "$REPO_ROOT"

BEFORE_BACKUPS=$(mktemp)
AFTER_BACKUPS=$(mktemp)
trap 'rm -f "$BEFORE_BACKUPS" "$AFTER_BACKUPS"' EXIT HUP INT TERM

list_graphify_backups() {
  find "$REPO_ROOT/graphify-out" -mindepth 1 -maxdepth 1 -type d -name '20??-??-??' -print 2>/dev/null | sort
}

list_graphify_backups > "$BEFORE_BACKUPS"
graphify update .
list_graphify_backups > "$AFTER_BACKUPS"

python3 "$SCRIPT_DIR/sync_graphify_obsidian.py" \
  --source-root "$REPO_ROOT" \
  --graph "$REPO_ROOT/graphify-out/graph.json" \
  --obsidian-root "$OBSIDIAN_ROOT"

# Graphify update can create a dated safety copy of the previous graph. Chadex
# keeps only the canonical graphify-out graph, so remove only backup directories
# created by this successful refresh. Never touch pre-existing dated archives.
comm -13 "$BEFORE_BACKUPS" "$AFTER_BACKUPS" | while IFS= read -r backup; do
  [ -n "$backup" ] || continue
  if [ -f "$backup/graph.json" ] && [ -f "$backup/manifest.json" ]; then
    rm -rf -- "$backup"
  else
    echo "warning: leaving unexpected Graphify directory untouched: $backup" >&2
  fi
done
