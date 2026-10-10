param(
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [switch]$BuildUpgradeFixture
)
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
$PSNativeCommandUseErrorActionPreference = $false

function Invoke-NativeChecked {
    param([string]$FilePath, [string[]]$Arguments)
    $null = Get-Command -Name $FilePath -CommandType Application -ErrorAction Stop
    $previous = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        & $FilePath @Arguments
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $previous }
    if ($null -eq $code -or $code -ne 0) { throw "Native command '$FilePath' failed: $code" }
}

function Assert-ReleaseSource {
    $current = (& git -C $repo rev-parse HEAD)
    if ($LASTEXITCODE -ne 0 -or $current -ne $source) { throw "Source HEAD changed during the build." }
    $status = @(& git -C $repo status --porcelain=v1 --untracked-files=all)
    if ($LASTEXITCODE -ne 0 -or $status.Count -ne 0) {
        foreach ($line in $status) { Write-Host "Release source status: $line" }
        throw "Release inputs must remain clean, including non-ignored untracked files."
    }
}

if (-not $IsWindows) { throw "Windows release build requires Windows." }
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$output = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $output) { throw "OutputDirectory must not already exist." }
if ($output.StartsWith($repo + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or $output -eq $repo) {
    throw "Candidate output must be outside the source checkout."
}
$source = (& git -C $repo rev-parse HEAD)
if ($LASTEXITCODE -ne 0) { throw "Git source identity unavailable." }
Assert-ReleaseSource
Invoke-NativeChecked "python" @((Join-Path $repo "scripts/prepare_windows_release.py"), "--repo-root", $repo, "--check-inputs")
$hostTriple = (& rustc -vV | Select-String '^host: ').ToString()
if ($LASTEXITCODE -ne 0 -or $hostTriple -ne 'host: x86_64-pc-windows-msvc') {
    throw "Only the native x86_64-pc-windows-msvc toolchain is supported."
}
$target = Join-Path $repo "runtime-engine/target/w5-release"
$env:CARGO_TARGET_DIR = $target
$env:CARGO_INCREMENTAL = "0"
$env:CHADEX_RUNTIME_GIT_COMMIT = $source.Substring(0, 12)
$env:CHADEX_RUNTIME_BUILT_AT = (& git -C $repo show -s --format=%ct HEAD)
if ($LASTEXITCODE -ne 0) { throw "Git build identity unavailable." }
$env:CHADEX_RUNTIME_GIT_DIRTY = "false"
$configPath = Join-Path $repo "apps/windows/src-tauri/tauri.conf.json"
$version = (Get-Content -Raw -LiteralPath $configPath | ConvertFrom-Json).version
$staging = Join-Path ([IO.Path]::GetTempPath()) ("chadex-release-" + [guid]::NewGuid().ToString('N'))
Push-Location $repo
try {
    Invoke-NativeChecked "cargo" @("build", "--release", "--locked", "--manifest-path", "rust-helper/Cargo.toml", "--bin", "chadex-helper")
    Assert-ReleaseSource
    Invoke-NativeChecked "cargo" @("build", "--release", "--locked", "--manifest-path", "chadex-runtime/Cargo.toml", "--bins")
    Assert-ReleaseSource
    Invoke-NativeChecked "python" @("scripts/prepare_windows_release.py", "--repo-root", $repo, "--target-dir", $target, "--output", $staging)
    # Debug staging is never a fallback: replace it only with this validated release set.
    $resources = Join-Path $repo "apps/windows/src-tauri/resources"
    if (Test-Path -LiteralPath $resources) {
        if ((Get-Item -LiteralPath $resources).Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "Refusing linked resource directory."
        }
        Remove-Item -LiteralPath $resources -Recurse -Force
    }
    Move-Item -LiteralPath $staging -Destination $resources
    Push-Location (Join-Path $repo "apps/windows")
    try {
        Invoke-NativeChecked "npm.cmd" @("ci")
        Assert-ReleaseSource
        # The explicit feature set excludes desktop-smoke. CLI supplies custom-protocol.
        Invoke-NativeChecked "npm.cmd" @("run", "tauri", "--", "build", "--ci", "--bundles", "nsis", "--config", "src-tauri/tauri.release.conf.json", "--", "--locked")
        Assert-ReleaseSource
        $upgradeBaselineVersion = $null
        if ($BuildUpgradeFixture) {
            # Synthetic older installer metadata around the SAME production binaries.
            # This tests installer migration, not historical application compatibility.
            if ($version -ne '0.6.4') { throw "Update the synthetic upgrade fixture for the candidate version." }
            $upgradeBaselineVersion = '0.6.3'
            $fixtureConfig = Get-Content -Raw -LiteralPath "src-tauri/tauri.release.conf.json" | ConvertFrom-Json
            $fixtureConfig | Add-Member -NotePropertyName version -NotePropertyValue $upgradeBaselineVersion
            $fixtureConfigPath = Join-Path $repo "apps/windows/src-tauri/w5-fixture.generated.json"
            try {
                $fixtureConfig | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $fixtureConfigPath -Encoding utf8NoBOM
                Invoke-NativeChecked "npm.cmd" @("run", "tauri", "--", "bundle", "--ci", "--bundles", "nsis", "--config", $fixtureConfigPath)
                New-Item -ItemType Directory -Path (Join-Path $output "upgrade-fixture") -Force | Out-Null
                $fixture = @(Get-ChildItem -LiteralPath (Join-Path $target "release/bundle/nsis") -Filter "*$upgradeBaselineVersion*setup.exe")
                if ($fixture.Count -ne 1) { throw "Expected one synthetic baseline installer." }
                Copy-Item -LiteralPath $fixture[0].FullName -Destination (Join-Path $output "upgrade-fixture/baseline-setup.exe")
            } finally { Remove-Item -LiteralPath $fixtureConfigPath -Force -ErrorAction SilentlyContinue }
        }
    } finally { Pop-Location }
    Assert-ReleaseSource
    $installers = @(Get-ChildItem -LiteralPath (Join-Path $target "release/bundle/nsis") -Filter "*_${version}_x64-setup.exe")
    if ($installers.Count -ne 1) { throw "Expected one x64 candidate installer." }
    $desktop = Join-Path $target "release/Chadex.exe"
    foreach ($binary in @($desktop, $installers[0].FullName, (Join-Path $resources "helper/chadex-helper.exe")) +
        @(Get-ChildItem -LiteralPath (Join-Path $resources "chadex-runtime") -Filter '*.exe' | ForEach-Object FullName)) {
        if ((Get-AuthenticodeSignature -LiteralPath $binary).Status -ne 'NotSigned') {
            throw "This unsigned candidate flow must not mislabel a signed or invalid signature."
        }
    }
    New-Item -ItemType Directory -Path $output -Force | Out-Null
    $name = "Chadex-$version-windows-x64-unsigned-setup.exe"
    $installerOutput = Join-Path $output $name
    Copy-Item -LiteralPath $installers[0].FullName -Destination $installerOutput
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $installerOutput).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText("$installerOutput.sha256", "$hash  $name`n", [Text.UTF8Encoding]::new($false))
    $manifest = [ordered]@{
        schema = 1; track = "W5"; version = $version; source_sha = $source;
        architecture = "x86_64"; profile = "release"; features = @("custom-protocol");
        desktop_smoke = $false; authenticode = "unsigned"; updater = "disabled";
        synthetic_upgrade_baseline = $upgradeBaselineVersion;
        installer = $name; sha256 = $hash; publication = "not authorized";
        desktop_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $desktop).Hash.ToLowerInvariant();
        full_product_acceptance = "pending"; synthetic_upgrade_fixture = [bool]$BuildUpgradeFixture
    }
    if ($BuildUpgradeFixture) {
        $manifest.upgrade_fixture_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $output "upgrade-fixture/baseline-setup.exe")).Hash.ToLowerInvariant()
    }
    $manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $output "candidate.json") -Encoding utf8NoBOM
    Copy-Item -LiteralPath (Join-Path $resources "release-resources.json") -Destination $output
} finally {
    Pop-Location
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
}
