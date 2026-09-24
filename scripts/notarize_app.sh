#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
APP=${1:-$ROOT/dist/Chadex.app}
PROFILE=${CHADEX_NOTARY_KEYCHAIN_PROFILE:-}
API_KEY_PATH=${CHADEX_NOTARY_API_KEY_PATH:-}
API_KEY_ID=${CHADEX_NOTARY_KEY_ID:-}
API_ISSUER=${CHADEX_NOTARY_ISSUER_ID:-}
OUTPUT=${CHADEX_NOTARIZED_ZIP_OUTPUT:-}

if [ ! -d "$APP" ]; then
  echo "error: app bundle not found: $APP" >&2
  exit 2
fi

"$ROOT/scripts/distribution_check.sh" "$APP"

STAGING=$(mktemp -d "${TMPDIR:-/tmp}/chadex-notary.XXXXXX")
trap 'rm -rf "$STAGING"' EXIT HUP INT TERM
PRE_ZIP="$STAGING/Chadex-notary-upload.zip"
/usr/bin/ditto -c -k --keepParent "$APP" "$PRE_ZIP"

echo "==> Submit to Apple notary service"
if [ -n "$PROFILE" ]; then
  /usr/bin/xcrun notarytool submit "$PRE_ZIP" --keychain-profile "$PROFILE" --wait
elif [ -n "$API_KEY_PATH" ] && [ -n "$API_KEY_ID" ] && [ -n "$API_ISSUER" ]; then
  test -f "$API_KEY_PATH" || {
    echo "error: notary API key file not found: $API_KEY_PATH" >&2
    exit 2
  }
  /usr/bin/xcrun notarytool submit "$PRE_ZIP" \
    --key "$API_KEY_PATH" \
    --key-id "$API_KEY_ID" \
    --issuer "$API_ISSUER" \
    --wait
else
  echo "error: configure CHADEX_NOTARY_KEYCHAIN_PROFILE or API key path/key-id/issuer" >&2
  exit 2
fi

echo "==> Staple notarization ticket"
/usr/bin/xcrun stapler staple "$APP"
CHADEX_REQUIRE_NOTARIZED=1 "$ROOT/scripts/distribution_check.sh" "$APP"

if [ -z "$OUTPUT" ]; then
  VERSION=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")
  OUTPUT="$ROOT/dist/Chadex-$VERSION-macos-arm64.zip"
fi
mkdir -p "$(dirname "$OUTPUT")"
rm -f "$OUTPUT" "$OUTPUT.sha256"
/usr/bin/ditto -c -k --keepParent "$APP" "$OUTPUT"
(
  cd "$(dirname "$OUTPUT")"
  /usr/bin/shasum -a 256 "$(basename "$OUTPUT")" >"$(basename "$OUTPUT").sha256"
)

echo "Notarized artifact: $OUTPUT"
echo "Checksum: $OUTPUT.sha256"
