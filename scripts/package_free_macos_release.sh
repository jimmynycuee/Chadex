#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
APP=${1:-$ROOT/dist/Chadex.app}
OUTPUT=${2:-}
VOLUME_NAME=${CHADEX_DMG_VOLUME_NAME:-Chadex}
[ -d "$APP" ] || { echo "error: app bundle not found: $APP" >&2; exit 2; }
VERSION=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")
[ -n "$OUTPUT" ] || OUTPUT="$ROOT/dist/Chadex-v$VERSION-macos-arm64.dmg"
/usr/bin/plutil -lint "$APP/Contents/Info.plist"
/usr/bin/codesign --verify --deep --strict "$APP"
detail=$(/usr/bin/codesign -dvvv "$APP" 2>&1)
printf '%s\n' "$detail" | /usr/bin/grep -q '^Signature=adhoc$' || { echo "error: free release app must be ad-hoc signed" >&2; exit 1; }
printf '%s\n' "$detail" | /usr/bin/grep -q 'flags=.*runtime' || { echo "error: app is missing Hardened Runtime" >&2; exit 1; }
for executable in "$APP/Contents/MacOS/Chadex" "$APP/Contents/Helpers/chadex-helper" "$APP/Contents/Resources/chadex-runtime/chadex-runtime-cli" "$APP/Contents/Resources/chadex-runtime/chadex-runtime-server" "$APP/Contents/Resources/chadex-runtime/chadex-runtime-runner"; do
  test -x "$executable"
  test "$(/usr/bin/lipo -archs "$executable")" = "arm64"
  /usr/bin/codesign --verify --strict "$executable"
done
test -f "$APP/Contents/Resources/Chadex-LICENSE.txt"
test -f "$APP/Contents/Resources/WebCodex-LICENSE.txt"
test -f "$APP/Contents/Resources/UPSTREAM.md"
RESOURCE_BUNDLE="$APP/Contents/Resources/Chadex_ChadexApp.bundle"
test -d "$RESOURCE_BUNDLE"
if [ -d "$RESOURCE_BUNDLE/Contents/Resources" ]; then
  RESOURCE_PAYLOAD="$RESOURCE_BUNDLE/Contents/Resources"
else
  RESOURCE_PAYLOAD="$RESOURCE_BUNDLE"
fi
test -f "$RESOURCE_PAYLOAD/en.lproj/Localizable.strings"
if [ ! -f "$RESOURCE_PAYLOAD/zh-Hant.lproj/Localizable.strings" ] && [ ! -f "$RESOURCE_PAYLOAD/zh-hant.lproj/Localizable.strings" ]; then
  echo "error: packaged Traditional Chinese localization is missing" >&2
  exit 1
fi
"$APP/Contents/MacOS/Chadex" --resource-preflight
TMP=$(mktemp -d "${TMPDIR:-/tmp}/chadex-free-release.XXXXXX")
MOUNT=
cleanup() { if [ -n "$MOUNT" ] && /sbin/mount | /usr/bin/grep -Fq " on $MOUNT "; then /usr/bin/hdiutil detach "$MOUNT" -quiet || true; fi; rm -rf "$TMP"; }
trap cleanup EXIT HUP INT TERM
STAGE="$TMP/stage"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/Chadex.app"
ln -s /Applications "$STAGE/Applications"
mkdir -p "$(dirname "$OUTPUT")"
rm -f "$OUTPUT" "$OUTPUT.sha256"
/usr/bin/hdiutil create -volname "$VOLUME_NAME" -srcfolder "$STAGE" -ov -format UDZO "$OUTPUT" >/dev/null
MOUNT="$TMP/mount"
mkdir -p "$MOUNT"
/usr/bin/hdiutil attach -nobrowse -readonly -mountpoint "$MOUNT" "$OUTPUT" >/dev/null
test -d "$MOUNT/Chadex.app"
test -L "$MOUNT/Applications"
/usr/bin/codesign --verify --deep --strict "$MOUNT/Chadex.app"
test "$(/usr/bin/lipo -archs "$MOUNT/Chadex.app/Contents/MacOS/Chadex")" = "arm64"
"$MOUNT/Chadex.app/Contents/MacOS/Chadex" --resource-preflight
/usr/bin/hdiutil detach "$MOUNT" -quiet
MOUNT=
(cd "$(dirname "$OUTPUT")" && /usr/bin/shasum -a 256 "$(basename "$OUTPUT")" >"$(basename "$OUTPUT").sha256")
echo "Free macOS release artifact: $OUTPUT"
echo "Checksum: $OUTPUT.sha256"
echo "Signing: ad-hoc / non-notarized free release path."
