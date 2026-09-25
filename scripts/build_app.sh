#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$ROOT/scripts/rust-env.sh"
chadex_setup_rust "$ROOT"
chadex_pin_runtime_identity "$ROOT"

cd "$ROOT"

APP=${CHADEX_APP_OUTPUT:-$ROOT/dist/Chadex.app}
APP_DISPLAY_NAME=${CHADEX_APP_DISPLAY_NAME:-Chadex}
APP_BUNDLE_ID=${CHADEX_APP_BUNDLE_ID:-app.chadex.Chadex}
APP_DATA_DIR=${CHADEX_APP_DATA_DIR:-}
APP_PREFERENCES_DIR=${CHADEX_APP_PREFERENCES_DIR:-}
APP_AUTOSTART_MODEL=${CHADEX_APP_AUTOSTART_MODEL:-}
APP_VERSION=${CHADEX_APP_VERSION:-0.1.1}
APP_BUILD_NUMBER=${CHADEX_APP_BUILD_NUMBER:-1}
RUNTIME_PROFILE=${CHADEX_RUNTIME_PROFILE:-release}
CODESIGN_MODE=${CHADEX_CODESIGN_MODE:-auto}
APP_ENTITLEMENTS=${CHADEX_APP_ENTITLEMENTS:-}
CARGO_JOBS=${CHADEX_CARGO_JOBS:-4}

case "$CARGO_JOBS" in
  ''|*[!0-9]*|0) echo "error: CHADEX_CARGO_JOBS must be a positive integer" >&2; exit 2 ;;
esac
export CARGO_BUILD_JOBS="$CARGO_JOBS"

case "$RUNTIME_PROFILE" in
  release|dogfood) ;;
  *) echo "error: CHADEX_RUNTIME_PROFILE must be release or dogfood" >&2; exit 2 ;;
esac

case "$CODESIGN_MODE" in
  auto|development|distribution|adhoc) ;;
  *) echo "error: CHADEX_CODESIGN_MODE must be auto, development, distribution, or adhoc" >&2; exit 2 ;;
esac

if [ -n "$APP_ENTITLEMENTS" ] && [ ! -f "$APP_ENTITLEMENTS" ]; then
  echo "error: CHADEX_APP_ENTITLEMENTS does not exist: $APP_ENTITLEMENTS" >&2
  exit 2
fi

# Fail before compilation when an explicitly requested signing class is unavailable.
if [ -z "${CHADEX_CODESIGN_IDENTITY:-}" ]; then
  case "$CODESIGN_MODE" in
    distribution)
      if ! /usr/bin/security find-identity -v -p codesigning 2>/dev/null | /usr/bin/grep -q 'Developer ID Application:'; then
        echo "error: distribution signing requires a Developer ID Application identity" >&2
        exit 2
      fi
      ;;
    development)
      if ! /usr/bin/security find-identity -v -p codesigning 2>/dev/null | /usr/bin/grep -q 'Apple Development:'; then
        echo "error: development signing requires an Apple Development identity" >&2
        exit 2
      fi
      ;;
  esac
fi

case "$APP_VERSION" in
  ''|*[!0-9.]*) echo "error: CHADEX_APP_VERSION must contain only decimal components" >&2; exit 2 ;;
esac
case "$APP_BUILD_NUMBER" in
  ''|*[!0-9.]*) echo "error: CHADEX_APP_BUILD_NUMBER must contain only decimal components" >&2; exit 2 ;;
esac

app_bundle_is_running() {
  bundle=$1
  for executable in \
    "$bundle/Contents/MacOS/Chadex" \
    "$bundle/Contents/Helpers/chadex-helper" \
    "$bundle/Contents/Resources/chadex-runtime/chadex-runtime-server" \
    "$bundle/Contents/Resources/chadex-runtime/chadex-runtime-runner"
  do
    if [ -e "$executable" ] && /usr/sbin/lsof -t "$executable" >/dev/null 2>&1; then
      return 0
    fi
  done
  return 1
}

REQUESTED_APP=$APP
if app_bundle_is_running "$REQUESTED_APP"; then
  case "$REQUESTED_APP" in
    *.app) APP="${REQUESTED_APP%.app}-next.app" ;;
    *) APP="$REQUESTED_APP-next" ;;
  esac
  if app_bundle_is_running "$APP"; then
    echo "error: both the requested Chadex bundle and its safe replacement are running" >&2
    echo "error: quit one of them before packaging another build" >&2
    exit 1
  fi
  echo "==> Running Chadex bundle detected; preserving $REQUESTED_APP"
  echo "==> Packaging this build at $APP"
fi

echo "==> Building Chadex runtime entrypoints"
"$CHADEX_CARGO" build --locked \
  --manifest-path chadex-runtime/Cargo.toml \
  --target-dir runtime-engine/target \
  --profile "$RUNTIME_PROFILE" \
  --bins

echo "==> Building Chadex helper"
"$CHADEX_CARGO" build --locked --release --manifest-path rust-helper/Cargo.toml

echo "==> Building Chadex Swift app executable"
# Use SwiftPM's default build system for packaged apps. Its generated
# Bundle.module accessor resolves package resources from Bundle.main.resourceURL,
# which maps to Contents/Resources inside a normal macOS .app bundle.
#
# Do not package an executable produced by `--build-system native` here: that
# accessor may embed build-directory/root-bundle candidates that are not valid
# after the executable is moved into dist/Chadex.app.
swift build -c release -debug-info-format none
SWIFT_BIN=$(swift build -c release -debug-info-format none --show-bin-path)
SWIFT_APP="$SWIFT_BIN/Chadex"
SWIFT_RESOURCES="$SWIFT_BIN/Chadex_ChadexApp.bundle"

if [ ! -x "$SWIFT_APP" ]; then
  echo "error: Swift executable missing: $SWIFT_APP" >&2
  exit 1
fi
if [ ! -d "$SWIFT_RESOURCES" ]; then
  echo "error: Swift resource bundle missing: $SWIFT_RESOURCES" >&2
  exit 1
fi
if [ -f "$SWIFT_RESOURCES/Contents/Info.plist" ]; then
  SWIFT_RESOURCE_INFO="$SWIFT_RESOURCES/Contents/Info.plist"
  SWIFT_RESOURCE_PAYLOAD="$SWIFT_RESOURCES/Contents/Resources"
elif [ -f "$SWIFT_RESOURCES/Info.plist" ]; then
  SWIFT_RESOURCE_INFO="$SWIFT_RESOURCES/Info.plist"
  SWIFT_RESOURCE_PAYLOAD="$SWIFT_RESOURCES"
else
  echo "error: Swift resource bundle is invalid (Info.plist missing): $SWIFT_RESOURCES" >&2
  exit 1
fi
if [ ! -d "$SWIFT_RESOURCE_PAYLOAD/en.lproj" ]; then
  echo "error: Swift resource bundle English localization is missing" >&2
  exit 1
fi
if [ ! -d "$SWIFT_RESOURCE_PAYLOAD/zh-Hant.lproj" ] && \
   [ ! -d "$SWIFT_RESOURCE_PAYLOAD/zh-hant.lproj" ]; then
  echo "error: Swift resource bundle Traditional Chinese localization is missing" >&2
  exit 1
fi
/usr/bin/plutil -lint "$SWIFT_RESOURCE_INFO"

CONTENTS="$APP/Contents"
mkdir -p "$(dirname "$APP")"
rm -rf "$APP"
mkdir -p \
  "$CONTENTS/MacOS" \
  "$CONTENTS/Helpers" \
  "$CONTENTS/Resources/chadex-runtime"

cp "$SWIFT_APP" "$CONTENTS/MacOS/Chadex"
cp "$ROOT/rust-helper/target/release/chadex-helper" "$CONTENTS/Helpers/chadex-helper"
cp "$ROOT/runtime-engine/target/$RUNTIME_PROFILE/chadex-runtime-cli" "$CONTENTS/Resources/chadex-runtime/chadex-runtime-cli"
cp "$ROOT/runtime-engine/target/$RUNTIME_PROFILE/chadex-runtime-server" "$CONTENTS/Resources/chadex-runtime/chadex-runtime-server"
cp "$ROOT/runtime-engine/target/$RUNTIME_PROFILE/chadex-runtime-runner" "$CONTENTS/Resources/chadex-runtime/chadex-runtime-runner"
cp -R "$SWIFT_RESOURCES" "$CONTENTS/Resources/Chadex_ChadexApp.bundle"
cp "$ROOT/attribution/WebCodex-LICENSE.txt" "$CONTENTS/Resources/WebCodex-LICENSE.txt"
cp "$ROOT/LICENSE" "$CONTENTS/Resources/Chadex-LICENSE.txt"
cp "$ROOT/UPSTREAM.md" "$CONTENTS/Resources/UPSTREAM.md"
cp "$ROOT/Sources/ChadexApp/Resources/ChadexIcon.icns" "$CONTENTS/Resources/ChadexIcon.icns"

cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleDisplayName</key><string>$APP_DISPLAY_NAME</string>
  <key>CFBundleExecutable</key><string>Chadex</string>
  <key>CFBundleIdentifier</key><string>$APP_BUNDLE_ID</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleLocalizations</key><array><string>en</string><string>zh-Hant</string></array>
  <key>CFBundleName</key><string>$APP_DISPLAY_NAME</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$APP_VERSION</string>
  <key>CFBundleVersion</key><string>$APP_BUILD_NUMBER</string>
  <key>CFBundleIconFile</key><string>ChadexIcon.icns</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>Chadex contains code derived from Apache-2.0 licensed WebCodex; see bundled UPSTREAM.md.</string>
</dict>
</plist>
PLIST

if [ -n "$APP_DATA_DIR" ] || [ -n "$APP_PREFERENCES_DIR" ] || [ -n "$APP_AUTOSTART_MODEL" ]; then
  /usr/libexec/PlistBuddy -c "Add :LSEnvironment dict" "$CONTENTS/Info.plist"
  if [ -n "$APP_DATA_DIR" ]; then
    /usr/libexec/PlistBuddy -c "Add :LSEnvironment:CHADEX_DATA_DIR string $APP_DATA_DIR" "$CONTENTS/Info.plist"
  fi
  if [ -n "$APP_PREFERENCES_DIR" ]; then
    /usr/libexec/PlistBuddy -c "Add :LSEnvironment:CHADEX_PREFERENCES_DIR string $APP_PREFERENCES_DIR" "$CONTENTS/Info.plist"
  fi
  if [ -n "$APP_AUTOSTART_MODEL" ]; then
    /usr/libexec/PlistBuddy -c "Add :LSEnvironment:CHADEX_AUTOSTART_MODEL string $APP_AUTOSTART_MODEL" "$CONTENTS/Info.plist"
  fi
fi

chmod 755 \
  "$CONTENTS/MacOS/Chadex" \
  "$CONTENTS/Helpers/chadex-helper" \
  "$CONTENTS/Resources/chadex-runtime/chadex-runtime-cli" \
  "$CONTENTS/Resources/chadex-runtime/chadex-runtime-server" \
  "$CONTENTS/Resources/chadex-runtime/chadex-runtime-runner"

/usr/bin/plutil -lint "$CONTENTS/Info.plist"
PACKAGED_RESOURCES="$CONTENTS/Resources/Chadex_ChadexApp.bundle"
if [ -f "$PACKAGED_RESOURCES/Contents/Info.plist" ]; then
  PACKAGED_RESOURCE_INFO="$PACKAGED_RESOURCES/Contents/Info.plist"
  PACKAGED_RESOURCE_PAYLOAD="$PACKAGED_RESOURCES/Contents/Resources"
else
  PACKAGED_RESOURCE_INFO="$PACKAGED_RESOURCES/Info.plist"
  PACKAGED_RESOURCE_PAYLOAD="$PACKAGED_RESOURCES"
fi
/usr/bin/plutil -lint "$PACKAGED_RESOURCE_INFO"

if [ ! -d "$PACKAGED_RESOURCE_PAYLOAD/en.lproj" ]; then
  echo "error: packaged English localization is missing" >&2
  exit 1
fi
if [ ! -d "$PACKAGED_RESOURCE_PAYLOAD/zh-Hant.lproj" ] && \
   [ ! -d "$PACKAGED_RESOURCE_PAYLOAD/zh-hant.lproj" ]; then
  echo "error: packaged Traditional Chinese localization is missing" >&2
  exit 1
fi

SIGN_IDENTITY="${CHADEX_CODESIGN_IDENTITY:-}"
case "$CODESIGN_MODE" in
  distribution)
    if [ -z "$SIGN_IDENTITY" ]; then
      SIGN_IDENTITY="$(/usr/bin/security find-identity -v -p codesigning 2>/dev/null \
        | /usr/bin/awk -F '"' '/Developer ID Application:/ { print $2; exit }')"
    fi
    if [ -z "$SIGN_IDENTITY" ]; then
      echo "error: distribution signing requires a Developer ID Application identity" >&2
      exit 2
    fi
    ;;
  development)
    if [ -z "$SIGN_IDENTITY" ]; then
      SIGN_IDENTITY="$(/usr/bin/security find-identity -v -p codesigning 2>/dev/null \
        | /usr/bin/awk -F '"' '/Apple Development:/ { print $2; exit }')"
    fi
    if [ -z "$SIGN_IDENTITY" ]; then
      echo "error: development signing requires an Apple Development identity" >&2
      exit 2
    fi
    ;;
  adhoc)
    SIGN_IDENTITY="-"
    ;;
  auto)
    if [ -z "$SIGN_IDENTITY" ]; then
      SIGN_IDENTITY="$(/usr/bin/security find-identity -v -p codesigning 2>/dev/null \
        | /usr/bin/awk -F '"' '/Apple Development:/ { print $2; exit }')"
    fi
    if [ -z "$SIGN_IDENTITY" ]; then
      SIGN_IDENTITY="-"
    fi
    ;;
esac

sign_code() {
  target=$1
  shift
  if [ "$CODESIGN_MODE" = "distribution" ]; then
    /usr/bin/codesign --force --options runtime --timestamp --sign "$SIGN_IDENTITY" "$@" "$target"
  else
    /usr/bin/codesign --force --options runtime --sign "$SIGN_IDENTITY" "$@" "$target"
  fi
}

if [ "$SIGN_IDENTITY" = "-" ]; then
  echo "warning: no Apple Development code-signing identity selected; using hardened-runtime ad-hoc signing" >&2
elif [ "$CODESIGN_MODE" = "distribution" ]; then
  echo "Signing Chadex for Developer ID distribution"
else
  echo "Signing Chadex with a stable development code-signing identity"
fi

# Sign nested code explicitly from the inside out. Avoid --deep for signing;
# --deep remains useful only as a final verification pass.
for nested in \
  "$CONTENTS/Helpers/chadex-helper" \
  "$CONTENTS/Resources/chadex-runtime/chadex-runtime-cli" \
  "$CONTENTS/Resources/chadex-runtime/chadex-runtime-server" \
  "$CONTENTS/Resources/chadex-runtime/chadex-runtime-runner"
do
  sign_code "$nested"
done

if [ -n "$APP_ENTITLEMENTS" ]; then
  sign_code "$APP" --entitlements "$APP_ENTITLEMENTS"
else
  sign_code "$APP"
fi

/usr/bin/codesign --verify --deep --strict "$APP"
if ! /usr/bin/codesign -dvvv "$APP" 2>&1 | /usr/bin/grep -q 'flags=.*runtime'; then
  echo "error: packaged app is not signed with Hardened Runtime" >&2
  exit 1
fi
if [ "$CODESIGN_MODE" = "distribution" ]; then
  if ! /usr/bin/codesign -dvvv "$APP" 2>&1 | /usr/bin/grep -q '^Authority=Developer ID Application:'; then
    echo "error: distribution package is not signed by Developer ID Application" >&2
    exit 1
  fi
  if ! /usr/bin/codesign -dvvv "$APP" 2>&1 | /usr/bin/grep -q '^Timestamp='; then
    echo "error: distribution signature is missing a secure timestamp" >&2
    exit 1
  fi
fi

echo "Built $APP"
