param(
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [string]$Version,
    [string]$ReleaseTag,
    [switch]$EnableUpdater,
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

function Get-NsisPayloadDesktopHash {
    param([Parameter(Mandatory = $true)][string]$BinaryPath)
    $hashOutput = & python -B (Join-Path $PSScriptRoot "windows_tauri_bundle_hash.py") --path $BinaryPath
    $exitCode = $LASTEXITCODE
    $hash = ($hashOutput | Out-String).Trim()
    if ($null -eq $exitCode -or $exitCode -ne 0 -or $hash -notmatch '^[0-9a-f]{64}$') {
        throw "Could not derive the Tauri NSIS desktop payload hash."
    }
    return $hash
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
if ($EnableUpdater -and $BuildUpgradeFixture) {
    throw "The synthetic upgrade fixture must keep updater artifacts disabled."
}
$publicKey = [string]$env:CHADEX_UPDATER_PUBLIC_KEY
$signingPrivateKey = $null
$signingPassword = $null
if ($EnableUpdater) {
    $trimmedPublicKey = $publicKey.Trim()
    if ([string]::IsNullOrWhiteSpace($trimmedPublicKey) -or
        $trimmedPublicKey.Equals("UNCONFIGURED", [StringComparison]::OrdinalIgnoreCase) -or
        $trimmedPublicKey.Equals("UNCONFIGURED_UPDATER_PUBLIC_KEY", [StringComparison]::OrdinalIgnoreCase)) {
        throw "Set the GitHub repository variable CHADEX_UPDATER_PUBLIC_KEY before building Windows release assets."
    }
    $signingPrivateKey = [string]$env:TAURI_SIGNING_PRIVATE_KEY
    $signingPassword = [string]$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD
    if ([string]::IsNullOrWhiteSpace($signingPrivateKey)) {
        throw "Set the GitHub Actions secret TAURI_SIGNING_PRIVATE_KEY before building Windows release assets."
    }
    $publicKey = $trimmedPublicKey
} else {
    # W5 CI builds unsigned installers only; never compile a placeholder as a
    # production signature-verification key.
    $publicKey = "UNCONFIGURED_UPDATER_PUBLIC_KEY"
}
# Do not let Git, npm lifecycle scripts, or Cargo build scripts inherit the
# private signing material. It is restored only for Tauri's final bundle step.
$env:TAURI_SIGNING_PRIVATE_KEY = $null
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $null
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$output = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $output) { throw "OutputDirectory must not already exist." }
if ($output.StartsWith($repo + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or $output -eq $repo) {
    throw "Candidate output must be outside the source checkout."
}
$source = (& git -C $repo rev-parse HEAD)
if ($LASTEXITCODE -ne 0) { throw "Git source identity unavailable." }
$releaseConfig = Get-Content -Raw -LiteralPath (Join-Path $repo "apps/windows/src-tauri/tauri.conf.json") | ConvertFrom-Json
$version = if ([string]::IsNullOrWhiteSpace($Version)) { [string]$releaseConfig.version } else { $Version }
if ($version -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
    throw "Release version must be a stable MAJOR.MINOR.PATCH value."
}
$releaseTag = if ([string]::IsNullOrWhiteSpace($ReleaseTag)) { "v$version" } else { $ReleaseTag }
$versionPattern = [regex]::Escape($version)
if ($releaseTag -notmatch "^v$versionPattern(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?$") {
    throw "Release tag must match v$version with an optional safe prerelease suffix."
}
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
$staging = Join-Path ([IO.Path]::GetTempPath()) ("chadex-release-" + [guid]::NewGuid().ToString('N'))
$updaterStatus = if ($EnableUpdater) { "tauri-signed" } else { "disabled" }
$previousUpdaterPublicKey = $env:CHADEX_UPDATER_PUBLIC_KEY
$previousRequireUpdaterKey = $env:CHADEX_REQUIRE_UPDATER_KEY
$previousTauriConfig = $env:TAURI_CONFIG
Push-Location $repo
try {
    $env:CHADEX_UPDATER_PUBLIC_KEY = $publicKey
    $env:CHADEX_REQUIRE_UPDATER_KEY = if ($EnableUpdater) { "1" } else { "0" }
    $env:TAURI_CONFIG = @{
        version = $version
        bundle = @{ createUpdaterArtifacts = $false }
    } | ConvertTo-Json -Compress
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
        # Keep the smoke-only feature disabled and explicitly enable custom-protocol,
        # which registers the updater plugin required by updater:default capability.
        Invoke-NativeChecked "npm.cmd" @("run", "tauri", "--", "build", "--ci", "--features", "custom-protocol", "--bundles", "nsis", "--config", "src-tauri/tauri.release.conf.json", "--", "--locked")
        Assert-ReleaseSource
        if ($EnableUpdater) {
            $env:TAURI_CONFIG = @{
                version = $version
                bundle = @{ createUpdaterArtifacts = $true }
            } | ConvertTo-Json -Compress
            $updaterBundleConfig = @{
                bundle = @{ createUpdaterArtifacts = $true }
            } | ConvertTo-Json -Compress
            $env:TAURI_SIGNING_PRIVATE_KEY = $signingPrivateKey
            if (-not [string]::IsNullOrWhiteSpace($signingPassword)) {
                $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $signingPassword
            }
            try {
                # The release config disables updater artifacts for unsigned W5 builds.
                # Pass an explicit final CLI config so it overrides that value in this
                # signed bundle; TAURI_CONFIG alone does not override --config files.
                # Tauri v2 reuses the standard NSIS setup.exe as the updater payload.
                Invoke-NativeChecked "npm.cmd" @(
                    "run", "tauri", "--", "bundle", "--ci", "--bundles", "nsis",
                    "--config", "src-tauri/tauri.release.conf.json",
                    "--config", $updaterBundleConfig
                )
                $bundleInstallers = @(Get-ChildItem -LiteralPath (Join-Path $target "release/bundle/nsis") -Filter "*_${version}_x64-setup.exe" -File)
                if ($bundleInstallers.Count -ne 1) { throw "Expected exactly one signed x64 NSIS updater installer for version $version." }
                $sourceUpdaterInstaller = $bundleInstallers[0].FullName
                $signatureOutput = & npm.cmd run --silent tauri -- signer sign $sourceUpdaterInstaller --app-version $version
                $signatureExitCode = $LASTEXITCODE
                if ($null -eq $signatureExitCode -or $signatureExitCode -ne 0) {
                    throw "Tauri failed to bind the updater signature to version $version."
                }
                $signatureOutput | & python (Join-Path $PSScriptRoot "tauri_updater_signature.py") `
                    --version $version --output "${sourceUpdaterInstaller}.sig"
                $signatureParseExitCode = $LASTEXITCODE
                if ($null -eq $signatureParseExitCode -or $signatureParseExitCode -ne 0) {
                    throw "Tauri updater signature output is invalid or does not attest release version $version."
                }
                Invoke-NativeChecked "cargo" @(
                    "run", "--locked", "--quiet",
                    "--manifest-path", (Join-Path $repo "scripts/windows-updater-signature-check/Cargo.toml"), "--",
                    "--signature-file", "${sourceUpdaterInstaller}.sig",
                    "--installer", $sourceUpdaterInstaller
                )
            } finally {
                $env:TAURI_SIGNING_PRIVATE_KEY = $null
                $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $null
                $env:TAURI_CONFIG = @{
                    version = $version
                    bundle = @{ createUpdaterArtifacts = $false }
                } | ConvertTo-Json -Compress
            }
            Assert-ReleaseSource
        }
        # Tauri patches a copy for each bundle and restores the raw target binary.
        # Derive the NSIS payload hash by applying that same marker replacement in memory.
        $desktop = Join-Path $target "release/Chadex.exe"
        $desktopSourceHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $desktop).Hash.ToLowerInvariant()
        $desktopHash = Get-NsisPayloadDesktopHash $desktop
        $upgradeBaselineVersion = $null
        $baselineDesktopSourceHash = $null
        $baselineDesktopHash = $null
        if ($BuildUpgradeFixture) {
            # Synthetic older installer metadata around the SAME production binaries.
            # This tests installer migration, not historical application compatibility.
            if ($version -ne '0.7.2') { throw "Update the synthetic upgrade fixture for the candidate version." }
            $upgradeBaselineVersion = '0.7.1'
            $fixtureConfig = Get-Content -Raw -LiteralPath "src-tauri/tauri.release.conf.json" | ConvertFrom-Json
            $fixtureConfig | Add-Member -NotePropertyName version -NotePropertyValue $upgradeBaselineVersion
            $fixtureConfig.bundle.createUpdaterArtifacts = $false
            $fixtureConfigPath = Join-Path $repo "apps/windows/src-tauri/w5-fixture.generated.json"
            $bundleDirectory = Join-Path $target "release/bundle/nsis"
            $updaterBeforeFixture = @{}
            if (Test-Path -LiteralPath $bundleDirectory) {
                foreach ($artifact in @(Get-ChildItem -LiteralPath $bundleDirectory -Filter "*.exe.sig" -File)) {
                    $updaterBeforeFixture[$artifact.FullName] = "{0}:{1}" -f $artifact.Length, $artifact.LastWriteTimeUtc.Ticks
                }
            }
            try {
                $fixtureConfig | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $fixtureConfigPath -Encoding utf8NoBOM
                $env:TAURI_CONFIG = @{
                    version = $upgradeBaselineVersion
                    bundle = @{ createUpdaterArtifacts = $false }
                } | ConvertTo-Json -Compress
                Invoke-NativeChecked "npm.cmd" @("run", "tauri", "--", "bundle", "--ci", "--bundles", "nsis", "--config", $fixtureConfigPath)
                $baselineDesktopSourceHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $desktop).Hash.ToLowerInvariant()
                $baselineDesktopHash = Get-NsisPayloadDesktopHash $desktop
                if ($baselineDesktopSourceHash -ne $desktopSourceHash -or $baselineDesktopHash -ne $desktopHash) {
                    throw "Synthetic baseline bundle did not preserve the candidate desktop binary."
                }
                New-Item -ItemType Directory -Path (Join-Path $output "upgrade-fixture") -Force | Out-Null
                $fixture = @(Get-ChildItem -LiteralPath (Join-Path $target "release/bundle/nsis") -Filter "*$upgradeBaselineVersion*setup.exe")
                if ($fixture.Count -ne 1) { throw "Expected one synthetic baseline installer." }
                Copy-Item -LiteralPath $fixture[0].FullName -Destination (Join-Path $output "upgrade-fixture/baseline-setup.exe")
                $updaterAfterFixture = @()
                if (Test-Path -LiteralPath $bundleDirectory) {
                    $updaterAfterFixture = @(Get-ChildItem -LiteralPath $bundleDirectory -Filter "*.exe.sig" -File)
                }
                foreach ($artifact in $updaterAfterFixture) {
                    $currentState = "{0}:{1}" -f $artifact.Length, $artifact.LastWriteTimeUtc.Ticks
                    if (-not $updaterBeforeFixture.ContainsKey($artifact.FullName) -or $updaterBeforeFixture[$artifact.FullName] -ne $currentState) {
                        throw "Synthetic upgrade fixture unexpectedly created or changed an updater artifact."
                    }
                }
            } finally { Remove-Item -LiteralPath $fixtureConfigPath -Force -ErrorAction SilentlyContinue }
        }
    } finally { Pop-Location }
    Assert-ReleaseSource
    $installers = @(Get-ChildItem -LiteralPath (Join-Path $target "release/bundle/nsis") -Filter "*_${version}_x64-setup.exe")
    if ($installers.Count -ne 1) { throw "Expected one x64 candidate installer." }
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
    [IO.File]::WriteAllText("${installerOutput}.sha256", "$hash  $name`n", [Text.UTF8Encoding]::new($false))
    if ($EnableUpdater) {
        $updaterInstallers = @(Get-ChildItem -LiteralPath (Join-Path $target "release/bundle/nsis") -Filter "*_${version}_x64-setup.exe" -File)
        if ($updaterInstallers.Count -ne 1) { throw "Expected exactly one signed x64 NSIS updater installer for version $version." }
        $sourceUpdaterInstaller = $updaterInstallers[0].FullName
        $sourceUpdaterSignature = "${sourceUpdaterInstaller}.sig"
        if (-not (Test-Path -LiteralPath $sourceUpdaterSignature -PathType Leaf)) {
            throw "Tauri did not produce the updater signature. Check the signing key and updater build configuration."
        }
        $signatureOutput = "${installerOutput}.sig"
        Copy-Item -LiteralPath $sourceUpdaterSignature -Destination $signatureOutput
        Invoke-NativeChecked "python" @(
            (Join-Path $repo "scripts/windows_updater_manifest.py"),
            "--version", $version,
            "--tag", $releaseTag,
            "--repository", $env:GITHUB_REPOSITORY,
            "--installer", $installerOutput,
            "--signature-file", $signatureOutput,
            "--output", (Join-Path $output "latest.json")
        )
    }
    $manifest = [ordered]@{
        schema = 2; track = "W5"; version = $version; source_sha = $source;
        architecture = "x86_64"; profile = "release"; features = @("custom-protocol");
        desktop_smoke = $false; authenticode = "unsigned"; updater = $updaterStatus;
        synthetic_upgrade_baseline = $upgradeBaselineVersion;
        installer = $name; sha256 = $hash; publication = "not authorized";
        desktop_source_sha256 = $desktopSourceHash;
        desktop_payload_sha256 = $desktopHash;
        full_product_acceptance = "pending"; synthetic_upgrade_fixture = [bool]$BuildUpgradeFixture
    }
    if ($BuildUpgradeFixture) {
        $manifest.baseline_desktop_source_sha256 = $baselineDesktopSourceHash
        $manifest.baseline_desktop_payload_sha256 = $baselineDesktopHash
        $manifest.upgrade_fixture_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $output "upgrade-fixture/baseline-setup.exe")).Hash.ToLowerInvariant()
    }
    $manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $output "candidate.json") -Encoding utf8NoBOM
    Copy-Item -LiteralPath (Join-Path $resources "release-resources.json") -Destination $output
} finally {
    Pop-Location
    $env:TAURI_SIGNING_PRIVATE_KEY = $null
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $null
    $env:TAURI_CONFIG = $previousTauriConfig
    $env:CHADEX_REQUIRE_UPDATER_KEY = $previousRequireUpdaterKey
    $env:CHADEX_UPDATER_PUBLIC_KEY = $previousUpdaterPublicKey
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
}
