#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TOOLCHAIN="$ROOT/.toolchain"
RUST_TOOLCHAIN=$(awk -F '"' '/^[[:space:]]*channel[[:space:]]*=/{print $2; exit}' "$ROOT/rust-toolchain.toml" 2>/dev/null || true)
RUST_TOOLCHAIN=${RUST_TOOLCHAIN:-stable}

if [ -x "$TOOLCHAIN/cargo/bin/cargo" ]; then
  echo "Project-local Rust toolchain already exists: $TOOLCHAIN"
  exit 0
fi

mkdir -p "$TOOLCHAIN"
TMP=${TMPDIR:-/tmp}/chadex-rustup-init.sh
/usr/bin/curl -fsSL https://sh.rustup.rs -o "$TMP"
CARGO_HOME="$TOOLCHAIN/cargo" \
RUSTUP_HOME="$TOOLCHAIN/rustup" \
/bin/sh "$TMP" -y --profile minimal --no-modify-path --default-toolchain "$RUST_TOOLCHAIN"

echo "Installed project-local Rust without modifying shell startup files."
