$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if (-not $IsWindows) {
    throw "windows_core_check.ps1 must run on Windows"
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Push-Location $repoRoot
try {
    Write-Host "== Chadex W1 Windows core gate =="
    rustc --version
    cargo --version

    Write-Host "== Compile the full runtime workspace, including test targets =="
    cargo check --locked --manifest-path runtime-engine/Cargo.toml --workspace --all-targets

    Write-Host "== Run Windows process/shell/computer/config unit tests =="
    cargo test --locked --manifest-path runtime-engine/Cargo.toml `
        -p chadex-runtime-process `
        -p chadex-runtime-persistent-shell `
        -p chadex-runtime-computer `
        -p chadex-runtime-runner-config

    Write-Host "== Run Runner library tests and Windows-specific binary tests =="
    cargo test --locked --manifest-path runtime-engine/Cargo.toml -p chadex-runtime-runner --lib
    cargo test --locked --manifest-path runtime-engine/Cargo.toml -p chadex-runtime-runner --bin chadex-runtime-runner -- windows

    Write-Host "== Compile and test the desktop helper on Windows =="
    cargo test --locked --manifest-path rust-helper/Cargo.toml

    Write-Host "== Compile the standalone runtime entrypoints =="
    cargo check --locked --manifest-path chadex-runtime/Cargo.toml --all-targets
}
finally {
    Pop-Location
}
