# Build the release binary and deploy it to bin\, the canonical launch location.
#
# Project rule: the app is always run from D:\code\auto_media\bin\auto_media.exe
# (a stable path so autostart / single-instance reference one location, and
# RuntimePaths resolves the repo root as bin's parent). Rebuild + redeploy with
# this script after any change.
#
#   pwsh scripts\deploy.ps1           # build release + copy to bin
#   pwsh scripts\deploy.ps1 -Launch   # ...and start it from bin
param([switch]$Launch)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

# The running app locks bin\auto_media.exe — stop it before copying.
Get-Process auto_media -ErrorAction SilentlyContinue | Stop-Process -Force

# Force build.rs to re-read the git hash so the status bar shows the current commit.
(Get-Content "$root\build.rs" -Raw) | Set-Content "$root\build.rs"

Push-Location $root
try {
    cargo build --release
} finally {
    Pop-Location
}

New-Item -ItemType Directory -Force "$root\bin" | Out-Null
Copy-Item "$root\target\release\auto_media.exe" "$root\bin\auto_media.exe" -Force
Write-Host "Deployed -> $root\bin\auto_media.exe"

if ($Launch) {
    Start-Process "$root\bin\auto_media.exe"
    Write-Host "Launched from bin."
}
