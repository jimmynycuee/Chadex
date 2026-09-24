#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$ROOT/scripts/rust-env.sh"
chadex_setup_rust "$ROOT"
chadex_pin_runtime_identity "$ROOT"

cd "$ROOT"
swift test
"$CHADEX_CARGO" test --locked --manifest-path rust-helper/Cargo.toml
