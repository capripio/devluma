<#
.SYNOPSIS
  Tests Devluma's installer in a clean Windows Sandbox: fresh install, launch, optional
  upgrade, uninstall, and that user data survives uninstall.

.EXAMPLE
  # Fresh install + uninstall:
  powershell -ExecutionPolicy Bypass -File scripts\sandbox\run-sandbox-test.ps1 `
    -Installer src-tauri\target\release\bundle\nsis\Devluma_0.1.0_x64-setup.exe

  # Upgrade from an older installer to a newer one:
  ... -Installer new-setup.exe -UpgradeFrom old-setup.exe

.NOTES
  Requires the Windows Sandbox feature. The Sandbox window closes itself when the test is
  done; results are written to scripts\sandbox\results\<time>\results.json and summary.txt.
#>
param(
    [string]$Installer,
    [string]$UpgradeFrom,
    # The portable zip (scripts\package-portable.ps1); tested on its own if no installer is given.
    [string]$Portable,
    [int]$TimeoutMinutes = 20
)
$ErrorActionPreference = 'Stop'

if (-not $Installer -and -not $Portable) { throw 'Pass -Installer and/or -Portable.' }
$sandboxExe = Join-Path $env:SystemRoot 'System32\WindowsSandbox.exe'
if (-not (Test-Path $sandboxExe)) { throw 'Windows Sandbox is not enabled (Windows Features > Windows Sandbox).' }

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$inputs = Join-Path $here "inputs\$stamp"
$results = Join-Path $here "results\$stamp"
New-Item -ItemType Directory -Force $inputs, $results | Out-Null

if ($Installer) { Copy-Item (Resolve-Path $Installer) (Join-Path $inputs 'new-setup.exe') }
if ($Portable) { Copy-Item (Resolve-Path $Portable) (Join-Path $inputs 'portable.zip') }
if ($UpgradeFrom) { Copy-Item (Resolve-Path $UpgradeFrom) (Join-Path $inputs 'old-setup.exe') }
Copy-Item (Join-Path $here 'inside-sandbox.ps1') $inputs

$wsb = @"
<Configuration>
  <Networking>Enable</Networking>
  <MappedFolders>
    <MappedFolder><HostFolder>$inputs</HostFolder><SandboxFolder>C:\DevlumaTest\in</SandboxFolder><ReadOnly>true</ReadOnly></MappedFolder>
    <MappedFolder><HostFolder>$results</HostFolder><SandboxFolder>C:\DevlumaTest\out</SandboxFolder><ReadOnly>false</ReadOnly></MappedFolder>
  </MappedFolders>
  <LogonCommand>
    <Command>powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\DevlumaTest\in\inside-sandbox.ps1</Command>
  </LogonCommand>
</Configuration>
"@
$wsbPath = Join-Path $inputs 'devluma-test.wsb'
Set-Content -Path $wsbPath -Value $wsb -Encoding UTF8

Write-Host "Starting Windows Sandbox; results go to $results"
Start-Process $sandboxExe -ArgumentList "`"$wsbPath`""

$done = Join-Path $results 'done.txt'
$deadline = (Get-Date).AddMinutes($TimeoutMinutes)
while (-not (Test-Path $done)) {
    if ((Get-Date) -gt $deadline) { throw "No result after $TimeoutMinutes minutes; check the Sandbox window and $results." }
    Start-Sleep -Seconds 5
}
Get-Content (Join-Path $results 'summary.txt')
if ((Get-Content $done) -ne 'PASS') { exit 1 }
