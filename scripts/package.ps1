# Builds Ocean Foundry and packs the portable ZIP into dist\.
# Usage (from the project folder):  powershell -ExecutionPolicy Bypass -File scripts\package.ps1
# Add -SkipTests to pack without running the test suite first.
param([switch]$SkipTests)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
}

# Link the C runtime into the programs, so the ZIP needs nothing installed.
$env:RUSTFLAGS = "-C target-feature=+crt-static"

if (-not $SkipTests) {
    cargo test --workspace --release
    if ($LASTEXITCODE -ne 0) { throw "Tests failed; nothing was packaged." }
}
cargo build --release -p of-app -p of-cli
if ($LASTEXITCODE -ne 0) { throw "Build failed." }

$line = (& "target\release\of-cli.exe" version | Select-Object -First 1)
if ($line -notmatch '(\d+\.\d+\.\d+)') { throw "Could not read the version from '$line'." }
$version = $Matches[1]
$name = "OceanFoundry-$version-win64"
$stage = Join-Path $root "dist\$name"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null

Copy-Item "target\release\of-app.exe" (Join-Path $stage "OceanFoundry.exe")
Copy-Item "target\release\of-cli.exe" (Join-Path $stage "OceanFoundry-cli.exe")
New-Item -ItemType Directory -Force (Join-Path $stage "licences") | Out-Null
Copy-Item "assets\fonts\NOTICE.txt" (Join-Path $stage "licences\Fonts-NOTICE.txt")
Copy-Item "assets\fonts\Inter-OFL.txt" (Join-Path $stage "licences\Inter-OFL.txt")
Copy-Item "scripts\README.txt" (Join-Path $stage "README.txt")
Copy-Item "LICENSE" (Join-Path $stage "LICENSE")
Copy-Item "AUTHORS" (Join-Path $stage "AUTHORS")

# Example files, made by the packaged program itself so they always match it.
$cli = Join-Path $stage "OceanFoundry-cli.exe"
& $cli sample (Join-Path $stage "samples\build-files") | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Could not write the sample build files." }
& $cli testruns (Join-Path $stage "samples\ocean-test-runs") | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Could not write the ocean test runs." }

$zip = Join-Path $root "dist\$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $stage -DestinationPath $zip
$size = [math]::Round((Get-Item $zip).Length / 1MB, 1)
Write-Host "Packaged $zip ($size MB)"
