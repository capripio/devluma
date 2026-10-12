# Runs inside Windows Sandbox (started by run-sandbox-test.ps1). Writes results.json,
# summary.txt and done.txt (PASS/FAIL) to C:\DevlumaTest\out.
$ErrorActionPreference = 'Continue'
$in = 'C:\DevlumaTest\in'
$out = 'C:\DevlumaTest\out'
$appDir = Join-Path $env:LOCALAPPDATA 'Devluma'
$checks = New-Object System.Collections.Generic.List[object]

function Check([string]$name, [bool]$ok, [string]$detail = '') {
    $checks.Add([pscustomobject]@{ check = $name; ok = $ok; detail = $detail })
}

function Install([string]$setup) {
    # NSIS silent install; per-user, so no UAC prompt is expected.
    $p = Start-Process $setup -ArgumentList '/S' -Wait -PassThru
    return $p.ExitCode
}

function Launch([int]$seconds) {
    $exe = Join-Path $appDir 'devluma.exe'
    $p = Start-Process $exe -PassThru
    Start-Sleep -Seconds $seconds
    $alive = -not $p.HasExited
    if ($alive) { Stop-Process -Id $p.Id -Force }
    return $alive
}

function PortableTest {
    # Unzip somewhere new, run it, and check nothing is written outside that folder.
    $dir = 'C:\PortableTest'
    Expand-Archive "$in\portable.zip" $dir -Force
    $exe = Get-ChildItem $dir -Recurse -Filter devluma.exe | Select-Object -First 1
    $folder = $exe.DirectoryName
    $p = Start-Process $exe.FullName -PassThru
    Start-Sleep -Seconds 20
    Check 'portable: app starts and keeps running' (-not $p.HasExited)
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
    Check 'portable: metadata in its own folder' (Test-Path (Join-Path $folder 'app.db'))
    Check 'portable: tools folder is bin' (Test-Path (Join-Path $folder 'bin'))
    Check 'portable: temp files in its own folder' (Test-Path (Join-Path $folder 'tmp'))
    Check 'portable: WebView2 data in its own folder' (Test-Path (Join-Path $folder 'webview'))
    Check 'portable: nothing in %LOCALAPPDATA%\Devluma' (-not (Test-Path (Join-Path $env:LOCALAPPDATA 'Devluma')))
    Check 'portable: no WebView2 data in the profile' (-not (Test-Path (Join-Path $env:LOCALAPPDATA 'pk.capripio.devluma')))
    # Moving the folder keeps it working.
    Move-Item $dir 'C:\Moved' -Force
    $moved = Get-ChildItem 'C:\Moved' -Recurse -Filter devluma.exe | Select-Object -First 1
    $p = Start-Process $moved.FullName -PassThru
    Start-Sleep -Seconds 15
    Check 'portable: still starts after moving the folder' (-not $p.HasExited)
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
}

try {
    if (Test-Path (Join-Path $in 'portable.zip')) { PortableTest }
    if (-not (Test-Path (Join-Path $in 'new-setup.exe'))) { throw 'done' }
    $first = if (Test-Path "$in\old-setup.exe") { "$in\old-setup.exe" } else { "$in\new-setup.exe" }
    $code = Install $first
    Check 'install exits 0' ($code -eq 0) "exit code $code"
    Check 'devluma.exe installed per-user' (Test-Path "$appDir\devluma.exe") $appDir
    Check 'elevation helper installed next to it' (Test-Path "$appDir\devluma-elevate.exe")
    Check 'third-party notices installed' (Test-Path "$appDir\THIRD_PARTY_NOTICES.md")
    Check 'no machine-wide install' (-not (Test-Path "$env:ProgramFiles\Devluma"))

    $alive = Launch 20
    Check 'app starts and keeps running (WebView2 present)' $alive
    Check 'app created its metadata database' (Test-Path "$appDir\app.db")

    # User data that must survive upgrade and uninstall.
    $marker = Join-Path $appDir 'data\mariadb\db\keep-me.txt'
    New-Item -ItemType Directory -Force (Split-Path $marker) | Out-Null
    Set-Content $marker 'user data'
    New-Item -ItemType Directory -Force "$appDir\backups\databases" | Out-Null
    Set-Content "$appDir\backups\databases\backup.sql" 'backup'

    if (Test-Path "$in\old-setup.exe") {
        $before = (Get-Item "$appDir\devluma.exe").VersionInfo.ProductVersion
        $code = Install "$in\new-setup.exe"
        $after = (Get-Item "$appDir\devluma.exe").VersionInfo.ProductVersion
        Check 'upgrade exits 0' ($code -eq 0) "exit code $code"
        Check 'upgrade replaced the app' ($before -ne $after) "$before -> $after"
        Check 'data kept across upgrade' (Test-Path $marker)
        Check 'app starts after upgrade' (Launch 20)
    }

    $uninstaller = Join-Path $appDir 'uninstall.exe'
    $p = Start-Process $uninstaller -ArgumentList '/S' -Wait -PassThru
    Start-Sleep -Seconds 5 # the NSIS uninstaller copies itself to %TEMP% and returns early
    Check 'uninstall exits 0' ($p.ExitCode -eq 0) "exit code $($p.ExitCode)"
    Check 'app removed' (-not (Test-Path "$appDir\devluma.exe"))
    Check 'database data kept after uninstall' (Test-Path $marker)
    Check 'backups kept after uninstall' (Test-Path "$appDir\backups\databases\backup.sql")
    Check 'metadata kept after uninstall' (Test-Path "$appDir\app.db")
} catch {
    if ($_.Exception.Message -ne 'done') { Check 'script error' $false $_.Exception.Message }
}

$checks | ConvertTo-Json | Set-Content "$out\results.json"
$lines = $checks | ForEach-Object { '{0}  {1}{2}' -f ($(if ($_.ok) { 'PASS' } else { 'FAIL' })), $_.check, $(if ($_.detail) { " ($($_.detail))" } else { '' }) }
$lines | Set-Content "$out\summary.txt"
$status = if (($checks | Where-Object { -not $_.ok }).Count -eq 0) { 'PASS' } else { 'FAIL' }
Set-Content "$out\done.txt" $status
Start-Sleep -Seconds 3
shutdown.exe /s /t 0
