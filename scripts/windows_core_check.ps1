$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
# Native command failures are checked explicitly through $LASTEXITCODE below.
$PSNativeCommandUseErrorActionPreference = $false

function Invoke-NativeChecked {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,

        [Parameter()]
        [string[]]$Arguments = @()
    )

    $null = Get-Command -Name $FilePath -CommandType Application -ErrorAction Stop
    $previousErrorActionPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        & $FilePath @Arguments
        $nativeExitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    if ($null -eq $nativeExitCode) {
        throw "Native command '$FilePath' did not return an exit code."
    }
    if ($nativeExitCode -ne 0) {
        throw "Native command '$FilePath' exited with code $nativeExitCode."
    }
}

if (-not $IsWindows) {
    throw "windows_core_check.ps1 must run on Windows"
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if (-not $env:WEBCODEX_TEST_PWSH) {
    $env:WEBCODEX_TEST_PWSH = (Get-Command -Name "pwsh.exe" -CommandType Application -ErrorAction Stop).Source
}
Push-Location $repoRoot
try {
    Write-Host "== Chadex W2 Windows core gate =="
    Invoke-NativeChecked -FilePath "rustc" -Arguments @("--version")
    Invoke-NativeChecked -FilePath "cargo" -Arguments @("--version")

    Write-Host "== Compile the full runtime workspace, including test targets =="
    Invoke-NativeChecked -FilePath "cargo" -Arguments @(
        "check", "--locked", "--manifest-path", "runtime-engine/Cargo.toml", "--workspace", "--all-targets"
    )

    Write-Host "== Build debug binaries for the Windows runtime E2E harness =="
    Invoke-NativeChecked -FilePath "cargo" -Arguments @(
        "build", "--locked", "--manifest-path", "rust-helper/Cargo.toml", "--bin", "chadex-helper"
    )
    Invoke-NativeChecked -FilePath "cargo" -Arguments @(
        "build", "--locked", "--manifest-path", "chadex-runtime/Cargo.toml",
        "--bin", "chadex-runtime-cli",
        "--bin", "chadex-runtime-server",
        "--bin", "chadex-runtime-runner"
    )

    Write-Host "== Run Windows process/shell/computer/config unit tests =="
    Invoke-NativeChecked -FilePath "cargo" -Arguments @(
        "test", "--locked", "--manifest-path", "runtime-engine/Cargo.toml",
        "-p", "chadex-runtime-process",
        "-p", "chadex-runtime-persistent-shell",
        "-p", "chadex-runtime-computer",
        "-p", "chadex-runtime-runner-config"
    )

    Write-Host "== Run ignored Windows process lifecycle tests, including stress =="
    Invoke-NativeChecked -FilePath "cargo" -Arguments @(
        "test", "--locked", "--manifest-path", "runtime-engine/Cargo.toml",
        "-p", "chadex-runtime-process", "--", "--ignored", "--test-threads=1"
    )

    Write-Host "== Run ignored Windows persistent PowerShell lifecycle tests =="
    Invoke-NativeChecked -FilePath "cargo" -Arguments @(
        "test", "--locked", "--manifest-path", "runtime-engine/Cargo.toml",
        "-p", "chadex-runtime-persistent-shell", "--lib", "--", "--ignored", "--test-threads=1"
    )

    Write-Host "== Run Runner library tests and Windows-specific binary tests =="
    Invoke-NativeChecked -FilePath "cargo" -Arguments @(
        "test", "--locked", "--manifest-path", "runtime-engine/Cargo.toml",
        "-p", "chadex-runtime-runner", "--lib"
    )
    Invoke-NativeChecked -FilePath "cargo" -Arguments @(
        "test", "--locked", "--manifest-path", "runtime-engine/Cargo.toml",
        "-p", "chadex-runtime-runner", "--bin", "chadex-runtime-runner", "--", "windows"
    )

    Write-Host "== Compile and test the desktop helper on Windows =="
    Invoke-NativeChecked -FilePath "cargo" -Arguments @(
        "test", "--locked", "--manifest-path", "rust-helper/Cargo.toml"
    )

    Write-Host "== Verify official Windows tunnel asset download, reuse, replacement, and version =="
    Invoke-NativeChecked -FilePath "cargo" -Arguments @(
        "test", "--locked", "--manifest-path", "rust-helper/Cargo.toml",
        "chadex_core::tunnel::windows_tests::windows_official_asset_install_reuse_replacement_version",
        "--", "--ignored", "--exact"
    )

    Write-Host "== Compile the standalone runtime entrypoints =="
    Invoke-NativeChecked -FilePath "cargo" -Arguments @(
        "check", "--locked", "--manifest-path", "chadex-runtime/Cargo.toml", "--all-targets"
    )
}
finally {
    Pop-Location
}
