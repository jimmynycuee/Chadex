# Early Windows source-integrity check. This does not build or accept a package.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
$PSNativeCommandUseErrorActionPreference = $false
if (-not $IsWindows -or $env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw "This check requires an ephemeral Windows GitHub-hosted runner."
}
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$source = (& git -C $repo rev-parse HEAD)
if ($LASTEXITCODE -ne 0) { throw "Source identity unavailable." }

function Assert-SourceClean {
    $head = (& git -C $repo rev-parse HEAD)
    if ($LASTEXITCODE -ne 0 -or $head -ne $source) { throw "Source HEAD changed." }
    $status = @(& git -C $repo status --porcelain=v1 --untracked-files=all)
    if ($LASTEXITCODE -ne 0 -or $status.Count -ne 0) {
        foreach ($line in $status) { Write-Host "Release source status: $line" }
        & git -C $repo diff -- apps/windows/src-tauri/Cargo.toml apps/windows/src-tauri/Cargo.lock
        throw "Source integrity check failed."
    }
}

function Invoke-Checked {
    param([string]$FilePath, [string[]]$Arguments)
    $previous = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        & $FilePath @Arguments
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $previous }
    Assert-SourceClean
    if ($null -eq $code -or $code -ne 0) { throw "Source preflight command failed: $FilePath ($code)" }
}

Assert-SourceClean
$owned = Join-Path $env:RUNNER_TEMP ("w5-source-check-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $owned -ErrorAction Stop | Out-Null
$previousTarget = $env:CARGO_TARGET_DIR
$previousConfig = $env:TAURI_CONFIG
Push-Location (Join-Path $repo "apps/windows")
try {
    Invoke-Checked "python" @((Join-Path $repo "scripts/prepare_windows_release.py"), "--repo-root", $repo, "--check-inputs")
    Invoke-Checked "npm.cmd" @("ci")
    $env:CARGO_TARGET_DIR = Join-Path $owned 'target'
    $runner = Join-Path $owned 'no-build.cmd'
    $log = Join-Path $owned 'tauri-resolution.log'
    [IO.File]::WriteAllText($runner, "@echo W5_SOURCE_RUNNER_EXECUTED`r`n@exit /b 73`r`n", [Text.Encoding]::ASCII)
    $previous = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        & npm.cmd run tauri -- build --ci --bundles nsis --config src-tauri/tauri.release.conf.json --runner $runner -- --locked *> $log
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $previous }
    Assert-SourceClean
    $text = Get-Content -Raw -LiteralPath $log
    if ($code -ne 1 -or -not $text.Contains('W5_SOURCE_RUNNER_EXECUTED')) {
        Write-Host $text
        throw "Tauri did not reach the intentional no-build runner."
    }
    Write-Host 'Tauri configuration resolution left source clean; intentional runner prevented compilation.'
    # Exercise Windows build scripts without compiling release runtimes first.
    # Resource copies are excluded ONLY in this diagnostic; the real package
    # build still stages and verifies every production resource and checks clean.
    $config = Get-Content -Raw -LiteralPath 'src-tauri/tauri.release.conf.json' | ConvertFrom-Json
    $config.bundle.resources = @()
    $env:TAURI_CONFIG = $config | ConvertTo-Json -Depth 10 -Compress
    Invoke-Checked "cargo" @("check", "--locked", "--manifest-path", "src-tauri/Cargo.toml", "--features", "custom-protocol")
    Write-Host 'Windows build scripts left source clean.'
} finally {
    Pop-Location
    $env:CARGO_TARGET_DIR = $previousTarget
    $env:TAURI_CONFIG = $previousConfig
    Remove-Item -LiteralPath $owned -Recurse -Force
}
