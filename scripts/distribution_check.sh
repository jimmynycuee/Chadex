#!/bin/sh
set -eu

APP=${1:-dist/Chadex.app}
EXPECT_ARCHS=${CHADEX_EXPECT_ARCHS:-arm64}
REQUIRE_NOTARIZED=${CHADEX_REQUIRE_NOTARIZED:-0}

if [ ! -d "$APP" ]; then
  echo "error: app bundle not found: $APP" >&2
  exit 2
fi

CONTENTS="$APP/Contents"
executables="
$CONTENTS/MacOS/Chadex
$CONTENTS/Helpers/chadex-helper
$CONTENTS/Resources/chadex-runtime/chadex-runtime-cli
$CONTENTS/Resources/chadex-runtime/chadex-runtime-server
$CONTENTS/Resources/chadex-runtime/chadex-runtime-runner
"

/usr/bin/plutil -lint "$CONTENTS/Info.plist"
/usr/bin/codesign --verify --deep --strict "$APP"

echo "==> Distribution signature"
signature=$(/usr/bin/codesign -dvvv "$APP" 2>&1)
printf '%s\n' "$signature" | /usr/bin/grep -q '^Authority=Developer ID Application:' || {
  echo "error: app is not signed with Developer ID Application" >&2
  exit 1
}
printf '%s\n' "$signature" | /usr/bin/grep -q 'flags=.*runtime' || {
  echo "error: app is missing Hardened Runtime" >&2
  exit 1
}
printf '%s\n' "$signature" | /usr/bin/grep -q '^Timestamp=' || {
  echo "error: app is missing a secure signing timestamp" >&2
  exit 1
}

printf '%s\n' "$executables" | while IFS= read -r executable; do
  [ -n "$executable" ] || continue
  [ -x "$executable" ] || {
    echo "error: executable missing: $executable" >&2
    exit 1
  }
  archs=$(/usr/bin/lipo -archs "$executable")
  if [ "$archs" != "$EXPECT_ARCHS" ]; then
    echo "error: unexpected architectures for $executable: $archs (expected $EXPECT_ARCHS)" >&2
    exit 1
  fi
  nested_signature=$(/usr/bin/codesign -dvvv "$executable" 2>&1)
  printf '%s\n' "$nested_signature" | /usr/bin/grep -q 'flags=.*runtime' || {
    echo "error: nested executable is missing Hardened Runtime: $executable" >&2
    exit 1
  }
done

test -f "$CONTENTS/Resources/Chadex-LICENSE.txt"
test -f "$CONTENTS/Resources/WebCodex-LICENSE.txt"
test -f "$CONTENTS/Resources/UPSTREAM.md"

if [ "$REQUIRE_NOTARIZED" = "1" ]; then
  echo "==> Notarization ticket and Gatekeeper"
  /usr/bin/xcrun stapler validate "$APP"
  /usr/sbin/spctl -a -vv --type exec "$APP"
fi

echo "Distribution package check passed."
