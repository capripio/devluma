<#
.SYNOPSIS
  Builds the portable edition: a zip with devluma.exe and its helper, no installer. Unzip
  anywhere writable (not Program Files) and run devluma.exe; everything Devluma downloads or
  creates stays in that folder (ADR 0014).

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\package-portable.ps1
#>
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Push-Location $root
try {
    node scripts/notices.mjs
    cargo tauri build --no-bundle
    if ($LASTEXITCODE -ne 0) { throw 'cargo tauri build failed' }
    $version = (Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json).version
    $release = Join-Path $root 'src-tauri\target\release'
    $stage = Join-Path $release "portable\Devluma"
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    New-Item -ItemType Directory -Force (Join-Path $stage 'sbom') | Out-Null

    foreach ($exe in 'devluma.exe', 'devluma-elevate.exe') {
        Copy-Item (Join-Path $release $exe) $stage
        & (Join-Path $root 'src-tauri\scripts\sign.ps1') (Join-Path $stage $exe)
    }
    Copy-Item THIRD_PARTY_NOTICES.md $stage
    Copy-Item sbom\devluma.cdx.json (Join-Path $stage 'sbom')
    @"
Devluma $version (portable)

Run devluma.exe. Everything Devluma downloads or creates - tools in bin\, databases in data\,
settings, logs, backups and temporary files - stays in this folder, so you can move or copy
it. Keep it somewhere you can write to (not C:\Program Files).

Requires the Microsoft Edge WebView2 Runtime, which Windows 11 and up-to-date Windows 10
already include.
"@ | Set-Content -Encoding UTF8 (Join-Path $stage 'README.txt')

    $zip = Join-Path $release "bundle\Devluma_${version}_x64-portable.zip"
    New-Item -ItemType Directory -Force (Split-Path $zip) | Out-Null
    if (Test-Path $zip) { Remove-Item $zip }
    Compress-Archive -Path $stage -DestinationPath $zip
    Write-Host "Portable edition: $zip"
} finally {
    Pop-Location
}
