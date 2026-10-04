$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
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
    if ($null -eq $nativeExitCode -or $nativeExitCode -ne 0) {
        throw "Native command '$FilePath' exited with code $nativeExitCode."
    }
}

if (-not $IsWindows) {
    throw "windows_computer_live_check.ps1 requires an interactive Windows desktop"
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$tests = @(
    "computer_windows_control_fixture_live_smoke",
    "computer_windows_scroll_to_element_fixture_live_smoke",
    "computer_windows_key_input_fixture_live_smoke",
    "computer_windows_uia_stale_identity_rejects_indistinguishable_replacement_live"
)

Push-Location $repoRoot
try {
    Write-Host "== Chadex Computer Use private Windows fixture gate =="
    foreach ($test in $tests) {
        Write-Host "== $test =="
        Invoke-NativeChecked -FilePath "cargo" -Arguments @(
            "test", "--locked", "--manifest-path", "runtime-engine/Cargo.toml",
            "-p", "chadex-runtime-computer", $test,
            "--", "--ignored", "--test-threads=1"
        )
    }
    Write-Host "Windows Computer private fixture gate passed."
}
finally {
    Pop-Location
}
