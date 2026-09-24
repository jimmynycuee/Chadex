#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$ROOT/scripts/rust-env.sh"
chadex_setup_rust "$ROOT"
chadex_pin_runtime_identity "$ROOT"

CARGO_JOBS=${CHADEX_CARGO_JOBS:-4}
case "$CARGO_JOBS" in
  ''|*[!0-9]*|0) echo "error: CHADEX_CARGO_JOBS must be a positive integer" >&2; exit 2 ;;
esac
export CARGO_BUILD_JOBS="$CARGO_JOBS"
cd "$ROOT"

LOCK_KEY=$(printf '%s' "$ROOT" | cksum | awk '{print $1}')
LOCK_DIR="${TMPDIR:-/tmp}/chadex-release-check.$LOCK_KEY.lock"
LOCK_OWNER="$LOCK_DIR/pid"
STAGING=

acquire_release_lock() {
  if mkdir "$LOCK_DIR" 2>/dev/null; then
    printf '%s\n' "$$" > "$LOCK_OWNER"
    return 0
  fi

  owner_pid=$(cat "$LOCK_OWNER" 2>/dev/null || true)
  case "$owner_pid" in
    ''|*[!0-9]*) owner_pid= ;;
  esac
  if [ -n "$owner_pid" ] && kill -0 "$owner_pid" 2>/dev/null; then
    echo "error: another release check is already running for this worktree (pid $owner_pid)" >&2
    echo "error: lock: $LOCK_DIR" >&2
    exit 2
  fi

  echo "warning: removing stale release-check lock: $LOCK_DIR" >&2
  rm -rf "$LOCK_DIR"
  if ! mkdir "$LOCK_DIR" 2>/dev/null; then
    echo "error: another release check acquired the worktree lock concurrently" >&2
    exit 2
  fi
  printf '%s\n' "$$" > "$LOCK_OWNER"
}

cleanup_release_check() {
  if [ -n "$STAGING" ]; then
    rm -rf "$STAGING"
  fi
  if [ "$(cat "$LOCK_OWNER" 2>/dev/null || true)" = "$$" ]; then
    rm -rf "$LOCK_DIR"
  fi
}

acquire_release_lock
trap cleanup_release_check EXIT HUP INT TERM

if [ "${CHADEX_RELEASE_ALLOW_DIRTY:-0}" != "1" ] && [ -n "$(git status --porcelain=v1 --untracked-files=all)" ]; then
  echo "error: release check requires a clean Git worktree" >&2
  echo "error: set CHADEX_RELEASE_ALLOW_DIRTY=1 only for pre-release development validation" >&2
  exit 2
fi

echo "==> License / attribution consistency"
test -f LICENSE
test -f UPSTREAM.md
test -f attribution/WebCodex-LICENSE.txt
python3 - <<'PY'
from pathlib import Path
pairs = [
    (Path("runtime-engine/LICENSE"), Path("LICENSE")),
    (Path("vendor/webcodex/LICENSE"), Path("attribution/WebCodex-LICENSE.txt")),
]
for left, right in pairs:
    if left.read_bytes().rstrip() != right.read_bytes().rstrip():
        raise SystemExit(f"license mismatch: {left} != {right}")
PY

echo "==> Swift tests"
swift test

echo "==> Rust helper tests"
"$CHADEX_CARGO" test --locked --manifest-path rust-helper/Cargo.toml

echo "==> Runtime engine: compile every target"
"$CHADEX_CARGO" check --locked --manifest-path runtime-engine/Cargo.toml --workspace --all-targets

echo "==> Runtime release-critical tests"
"$CHADEX_CARGO" test --locked --manifest-path runtime-engine/Cargo.toml -p chadex-runtime-process
"$CHADEX_CARGO" test --locked --manifest-path runtime-engine/Cargo.toml -p chadex-runtime-tool-contracts
"$CHADEX_CARGO" test --locked --manifest-path runtime-engine/Cargo.toml -p chadex-runtime-tool-runtime-contracts
"$CHADEX_CARGO" test --locked --manifest-path runtime-engine/Cargo.toml -p chadex-runtime-runner-registry
"$CHADEX_CARGO" test --locked --manifest-path runtime-engine/Cargo.toml -p chadex-runtime-engine tool_request_trace --lib

echo "==> Production runtime wrapper check"
"$CHADEX_CARGO" check --locked --manifest-path chadex-runtime/Cargo.toml --target-dir runtime-engine/target --bins

echo "==> Release tooling tests"
PYTHONDONTWRITEBYTECODE=1 python3 -B scripts/test_sync_graphify_obsidian.py
PYTHONDONTWRITEBYTECODE=1 python3 -B scripts/test_benchmark_chatgpt_completion.py
PYTHONDONTWRITEBYTECODE=1 python3 -B scripts/test_benchmark_terminal_ab.py

echo "==> Repository diff hygiene"
git diff --check

if [ "${CHADEX_RELEASE_CHECK_BUILD_APP:-1}" = "1" ]; then
  STAGING=$(mktemp -d "${TMPDIR:-/tmp}/chadex-release-check.XXXXXX")
  APP="$STAGING/Chadex.app"

  echo "==> Fresh release-profile app package"
  CHADEX_APP_OUTPUT="$APP" \
  CHADEX_RUNTIME_PROFILE=release \
  "$ROOT/scripts/build_app.sh"

  test -x "$APP/Contents/MacOS/Chadex"
  test -x "$APP/Contents/Helpers/chadex-helper"
  test -x "$APP/Contents/Resources/chadex-runtime/chadex-runtime-cli"
  test -x "$APP/Contents/Resources/chadex-runtime/chadex-runtime-server"
  test -x "$APP/Contents/Resources/chadex-runtime/chadex-runtime-runner"
  test -f "$APP/Contents/Resources/Chadex-LICENSE.txt"
  test -f "$APP/Contents/Resources/WebCodex-LICENSE.txt"
  test -f "$APP/Contents/Resources/UPSTREAM.md"

  for legacy in webcodex webcodex-server webcodex-runner; do
    test ! -e "$APP/Contents/Resources/webcodex-runtime/$legacy"
  done

  /usr/bin/plutil -lint "$APP/Contents/Info.plist"
  /usr/bin/codesign --verify --deep --strict "$APP"
  if ! /usr/bin/codesign -dvvv "$APP" 2>&1 | /usr/bin/grep -q 'flags=.*runtime'; then
    echo "error: release-check app is missing Hardened Runtime" >&2
    exit 1
  fi
  for executable in \
    "$APP/Contents/MacOS/Chadex" \
    "$APP/Contents/Helpers/chadex-helper" \
    "$APP/Contents/Resources/chadex-runtime/chadex-runtime-cli" \
    "$APP/Contents/Resources/chadex-runtime/chadex-runtime-server" \
    "$APP/Contents/Resources/chadex-runtime/chadex-runtime-runner"
  do
    test "$(/usr/bin/lipo -archs "$executable")" = "arm64"
    /usr/bin/codesign --verify --strict "$executable"
    if ! /usr/bin/codesign -dvvv "$executable" 2>&1 | /usr/bin/grep -q 'flags=.*runtime'; then
      echo "error: nested executable is missing Hardened Runtime: $executable" >&2
      exit 1
    fi
  done
  rm -rf "$STAGING"
  STAGING=
fi

echo "Source release check passed. Distribution signing/notarization is a separate gate."
