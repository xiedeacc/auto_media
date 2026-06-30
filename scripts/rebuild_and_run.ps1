#requires -Version 5
<#
.SYNOPSIS
  Stop running Auto Media instances, incrementally rebuild, and relaunch.

.DESCRIPTION
  One-command dev restart: kills any running auto_media.exe, runs an incremental
  `cargo build` (debug by default, or release with -Release), then launches the
  freshly built binary from the project root so it picks up conf/ and data/.

.EXAMPLE
  pwsh -File scripts/rebuild_and_run.ps1
  pwsh -File scripts/rebuild_and_run.ps1 -Release
#>
param([switch]$Release)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

Write-Host "[1/3] Stopping running auto_media.exe ..."
Get-Process -Name auto_media -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 300

# WebView2 caches the embedded UI (styles.css / main.js) aggressively and keys on
# the asset URL, so a rebuilt frontend can otherwise render with stale CSS/JS.
# Drop the WebView2 HTTP/code cache so UI edits always take effect on relaunch.
$webview = "$env:LOCALAPPDATA\local.auto-media\EBWebView\Default"
if (Test-Path $webview) {
  Remove-Item -Recurse -Force "$webview\Cache" -ErrorAction SilentlyContinue
  Remove-Item -Recurse -Force "$webview\Code Cache" -ErrorAction SilentlyContinue
}

Write-Host "[2/3] Building (incremental) ..."
if ($Release) { cargo build --release } else { cargo build }
if ($LASTEXITCODE -ne 0) { Write-Error "cargo build failed (exit $LASTEXITCODE)"; exit 1 }

$exe = if ($Release) { ".\target\release\auto_media.exe" } else { ".\target\debug\auto_media.exe" }
Write-Host "[3/3] Launching $exe ..."
Start-Process -FilePath $exe -WorkingDirectory $root

Write-Host "Done."
