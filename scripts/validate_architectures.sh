#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$ROOT/scripts/rust-env.sh"
chadex_setup_rust "$ROOT"
chadex_pin_runtime_identity "$ROOT"

cd "$ROOT"

if [ -z "${CHADEX_RUSTUP:-}" ]; then
  echo "rustup is required for cross-architecture validation." >&2
  exit 1
fi

"$CHADEX_RUSTUP" target add x86_64-apple-darwin aarch64-apple-darwin

echo "==> Swift arm64 compile"
swift build -c release --triple arm64-apple-macosx14.0
echo "==> Swift x86_64 compile"
swift build -c release --triple x86_64-apple-macosx14.0

echo "==> Rust helper arm64 compile"
"$CHADEX_CARGO" check --locked --manifest-path rust-helper/Cargo.toml --target aarch64-apple-darwin
echo "==> Rust helper x86_64 compile"
"$CHADEX_CARGO" check --locked --manifest-path rust-helper/Cargo.toml --target x86_64-apple-darwin

echo "Architecture compile validation complete. This is not an Intel runtime test."
