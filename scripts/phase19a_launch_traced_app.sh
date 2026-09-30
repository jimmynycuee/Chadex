#!/bin/sh
set -eu

APP="/Applications/Chadex.app"
APP_BIN="$APP/Contents/MacOS/Chadex"
TRACE_DIR="${PHASE19_TRACE_DIR:-$HOME/Documents/ChatGPT/agent-harness-benchmark/phase19/runtime-traces/chadex}"
LOG_DIR="$HOME/Library/Logs/Chadex"
LOG_FILE="$LOG_DIR/phase19a-traced-launch.log"

if [ ! -x "$APP_BIN" ]; then
  echo "Chadex executable not found: $APP_BIN" >&2
  exit 2
fi

mkdir -p "$TRACE_DIR" "$LOG_DIR"
chmod 700 "$TRACE_DIR"

pids=$(pgrep -f '^/Applications/Chadex.app/Contents/MacOS/Chadex$' || true)
if [ -n "$pids" ]; then
  kill $pids || true
  i=0
  while pgrep -f '^/Applications/Chadex.app/Contents/MacOS/Chadex$' >/dev/null 2>&1; do
    i=$((i + 1))
    if [ "$i" -ge 20 ]; then
      echo "Chadex did not exit cleanly before traced launch." >&2
      exit 3
    fi
    sleep 0.25
  done
fi

cleanup_env() {
  launchctl unsetenv WEBCODEX_TOOL_REQUEST_TRACE >/dev/null 2>&1 || true
  launchctl unsetenv WEBCODEX_TOOL_REQUEST_TRACE_DIR >/dev/null 2>&1 || true
}
trap cleanup_env EXIT INT TERM

launchctl setenv WEBCODEX_TOOL_REQUEST_TRACE full
launchctl setenv WEBCODEX_TOOL_REQUEST_TRACE_DIR "$TRACE_DIR"
open -a "$APP"

sleep 4

if ! pgrep -f '^/Applications/Chadex.app/Contents/MacOS/Chadex$' >/dev/null 2>&1; then
  echo "Chadex failed to stay running after traced launch." >&2
  exit 4
fi

echo "Started Chadex with Phase 19A full request tracing."
echo "Trace directory: $TRACE_DIR"
echo "Launch log: $LOG_FILE"
