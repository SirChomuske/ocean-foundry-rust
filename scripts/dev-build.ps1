# Builds Ocean Foundry and refreshes the development folder dist\OceanFoundry-dev.
# Usage (from the project folder):  powershell -ExecutionPolicy Bypass -File scripts\dev-build.ps1
# Add -Test to run the test suite first, -Run to start the program afterwards.
#
# This is not the release package (that is scripts\package.ps1). It makes a
# plain folder that is refreshed in place: only the program files are
# replaced, and the "OceanFoundry-data" folder inside it - your settings and
# ocean-tuning.json - is never touched.
param([switch]$Test, [switch]$Run)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
}

# Link the C runtime into the programs, so the folder needs nothing installed.
$env:RUSTFLAGS = "-C target-feature=+crt-static"

if ($Test) {
    cargo test --workspace --release --features of-app/dev,of-cli/dev
    if ($LASTEXITCODE -ne 0) { throw "Tests failed; the development folder was left as it was." }
}
# The development build keeps lava and the coast and cliffs lands (feature "dev");
# the package (package.ps1) is built without them.
cargo build --release -p of-app -p of-cli --features of-app/dev,of-cli/dev
if ($LASTEXITCODE -ne 0) { throw "Build failed; the development folder was left as it was." }

$stage = Join-Path $root "dist\OceanFoundry-dev"
New-Item -ItemType Directory -Force $stage | Out-Null
New-Item -ItemType Directory -Force (Join-Path $stage "licences") | Out-Null

$files = @(
    @("target\release\of-app.exe", "OceanFoundry.exe"),
    @("target\release\of-cli.exe", "OceanFoundry-cli.exe"),
    @("scripts\README-DEV.txt", "README-DEV.txt"),
    @("assets\TUNING.txt", "TUNING.txt"),
    @("assets\fonts\NOTICE.txt", "licences\Fonts-NOTICE.txt"),
    @("assets\fonts\Inter-OFL.txt", "licences\Inter-OFL.txt")
)
foreach ($pair in $files) {
    try {
        Copy-Item $pair[0] (Join-Path $stage $pair[1]) -Force
    } catch {
        throw "Could not replace $($pair[1]). Close Ocean Foundry and run this again. ($($_.Exception.Message))"
    }
}

$line = (& (Join-Path $stage "OceanFoundry-cli.exe") version | Select-Object -First 1)
$stamp = Get-Date -Format "yyyy-MM-dd HH:mm"
Set-Content -Path (Join-Path $stage "BUILD.txt") -Value "$line`r`nDevelopment build made $stamp" -Encoding utf8

$data = Join-Path $stage "OceanFoundry-data"
$kept = if (Test-Path $data) { "settings and tuning in OceanFoundry-data were kept" } else { "no settings yet; they appear on first run" }
Write-Host "Development build refreshed: $stage ($kept)"

if ($Run) {
    Start-Process (Join-Path $stage "OceanFoundry.exe") -WorkingDirectory $stage
}
