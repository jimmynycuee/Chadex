#!/bin/sh

chadex_setup_rust() {
  ROOT=$1
  if [ -x "$ROOT/.toolchain/cargo/bin/cargo" ]; then
    export CARGO_HOME="$ROOT/.toolchain/cargo"
    export RUSTUP_HOME="$ROOT/.toolchain/rustup"
    CHADEX_CARGO="$ROOT/.toolchain/cargo/bin/cargo"
    CHADEX_RUSTUP="$ROOT/.toolchain/cargo/bin/rustup"
  elif command -v cargo >/dev/null 2>&1; then
    CHADEX_CARGO=$(command -v cargo)
    CHADEX_RUSTUP=$(command -v rustup || true)
  else
    echo "Rust toolchain not found. Run ./scripts/bootstrap_rust.sh first." >&2
    return 1
  fi
  export CHADEX_CARGO CHADEX_RUSTUP
}

chadex_pin_runtime_identity() {
  ROOT=${1:-$(pwd)}
  CHADEX_RUNTIME_GIT_COMMIT=$(git -C "$ROOT" rev-parse --short=12 HEAD 2>/dev/null || printf 'unknown')
  if git -C "$ROOT" diff --quiet HEAD -- 2>/dev/null && git -C "$ROOT" diff --cached --quiet HEAD -- 2>/dev/null; then
    CHADEX_RUNTIME_GIT_DIRTY=false
  else
    CHADEX_RUNTIME_GIT_DIRTY=true
  fi
  CHADEX_RUNTIME_BUILT_AT=$(git -C "$ROOT" show -s --format=%ct HEAD 2>/dev/null || date +%s)
  export CHADEX_RUNTIME_GIT_COMMIT CHADEX_RUNTIME_GIT_DIRTY CHADEX_RUNTIME_BUILT_AT
}
