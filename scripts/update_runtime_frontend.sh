#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SOURCE="$ROOT/vendor/webcodex/frontend"
OUTPUT="$ROOT/runtime-engine/frontend/dist"
NODE=${CHADEX_NODE:-$(command -v node || true)}
NPM=${CHADEX_NPM:-$(command -v npm || true)}

if [ -z "$NODE" ] || [ ! -x "$NODE" ]; then
  echo "error: Node.js is required to regenerate Runtime Console assets" >&2
  exit 2
fi
if [ -z "$NPM" ] || [ ! -x "$NPM" ]; then
  echo "error: npm is required to regenerate Runtime Console assets" >&2
  exit 2
fi

cd "$SOURCE"
"$NPM" ci --ignore-scripts
rm -rf "$OUTPUT"
mkdir -p "$OUTPUT"
"$NODE" scripts/build.mjs --out-dir "$OUTPUT"

echo "Updated $OUTPUT"
