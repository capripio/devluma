# Code-signing hook for `cargo tauri build` (bundle.windows.signCommand). Called once per
# file to sign. Signing is optional until Devluma has a certificate:
#   DEVLUMA_SIGN_THUMBPRINT  SHA-1 thumbprint of a code-signing certificate in the current
#                            user's certificate store -> signed with signtool (SHA-256,
#                            RFC 3161 timestamp from DEVLUMA_SIGN_TIMESTAMP_URL or DigiCert).
# Without it the file is left unsigned and a warning is printed; Windows SmartScreen will
# warn users about unsigned installers.
param([Parameter(Mandatory = $true)][string]$File)
$ErrorActionPreference = 'Stop'

if (-not $env:DEVLUMA_SIGN_THUMBPRINT) {
    Write-Warning "Not signing $File (DEVLUMA_SIGN_THUMBPRINT is not set)."
    exit 0
}
$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
    Sort-Object FullName -Descending | Select-Object -First 1
if (-not $signtool) { throw "signtool.exe not found; install the Windows SDK." }
$ts = if ($env:DEVLUMA_SIGN_TIMESTAMP_URL) { $env:DEVLUMA_SIGN_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }
& $signtool.FullName sign /sha1 $env:DEVLUMA_SIGN_THUMBPRINT /fd sha256 /tr $ts /td sha256 /d Devluma $File
if ($LASTEXITCODE -ne 0) { throw "signtool failed for $File" }
