#!/bin/sh
set -eu

APP=${1:-dist/Chadex.app}
RUNNER="$APP/Contents/Resources/chadex-runtime/chadex-runtime-runner"
EXPECTED_IDENTIFIER=app.chadex.runtime.runner

[ -x "$RUNNER" ] || {
  echo "error: Computer runner is missing or not executable: $RUNNER" >&2
  exit 2
}

/usr/bin/codesign --verify --strict "$RUNNER"
detail=$(/usr/bin/codesign -dvvv "$RUNNER" 2>&1)
identifier=$(printf '%s\n' "$detail" | /usr/bin/sed -n 's/^Identifier=//p' | /usr/bin/head -1)
[ "$identifier" = "$EXPECTED_IDENTIFIER" ] || {
  echo "error: unexpected Computer runner code identifier: $identifier" >&2
  exit 1
}

requirement=$(/usr/bin/codesign -d -r- "$RUNNER" 2>&1)
if printf '%s\n' "$detail" | /usr/bin/grep -q '^Signature=adhoc$'; then
  printf '%s\n' "$requirement" | /usr/bin/grep -q 'cdhash' || {
    echo "error: ad-hoc Computer runner requirement was not conservatively cdhash-bound" >&2
    exit 1
  }
  echo "computer_runner_identifier=$identifier"
  echo "computer_tcc_identity=rebuild_sensitive"
  echo "warning: ad-hoc Computer Use permissions are tied to this runner build and may require re-approval after an update." >&2
else
  printf '%s\n' "$requirement" | /usr/bin/grep -Fq "identifier \"$EXPECTED_IDENTIFIER\"" || {
    echo "error: signed Computer runner designated requirement does not retain its stable identifier" >&2
    exit 1
  }
  if printf '%s\n' "$requirement" | /usr/bin/grep -q 'cdhash'; then
    echo "error: signed Computer runner designated requirement unexpectedly depends on cdhash" >&2
    exit 1
  fi
  echo "computer_runner_identifier=$identifier"
  echo "computer_tcc_identity=stable_signed"
fi
