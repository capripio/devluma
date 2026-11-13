# Compiles src-tauri/windows/hooks.nsh into a small harness and checks the reinstall reset
# (move aside, delete bin, move data, roll back on a locked file). Needs Tauri's NSIS
# (downloaded by the first `cargo tauri build`).
param([string]$Hooks = (Join-Path $PSScriptRoot '..\..\src-tauri\windows\hooks.nsh'))
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$work = Join-Path $env:TEMP "devluma-hooks-test"
New-Item -ItemType Directory -Force $work | Out-Null
$makensis = Join-Path $env:LOCALAPPDATA 'tauri\NSIS\Bin\makensis.exe'
& $makensis /V2 "/DHOOKS=$Hooks" (Join-Path $here 'hooks-harness.nsi')
if ($LASTEXITCODE -ne 0) { throw "makensis failed" }
$exe = Join-Path $here 'harness.exe'

function New-Root($name) {
  $r = Join-Path $work $name
  if (Test-Path $r) { Remove-Item -Recurse -Force $r }
  foreach ($d in 'bin\php\8.4', 'services\nginx', 'www\shop', 'data\mariadb', 'certificates', 'logs') {
    New-Item -ItemType Directory -Force (Join-Path $r $d) | Out-Null
  }
  Set-Content (Join-Path $r 'app.db') 'db'
  Set-Content (Join-Path $r 'bin\php\8.4\php.exe') 'x'
  Set-Content (Join-Path $r 'www\shop\index.php') 'mine'
  Set-Content (Join-Path $r 'data\mariadb\ibdata1') 'data'
  return $r
}
function Run($root, [string[]]$extra) {
  $p = Start-Process -FilePath $exe -ArgumentList (@('/S') + $extra + @("/D=$root")) -Wait -PassThru
  return $p.ExitCode
}
function Check($cond, $msg) { if (-not $cond) { throw "FAILED: $msg" } else { "ok: $msg" } }

# 1. Reset, keep a copy of bin, data stays.
$r = New-Root 'root1'
$code = Run $r @()
$prev = Get-Content (Join-Path $r 'harness-done.txt')
Check ($code -eq 0) "reset exits 0 ($code)"
Check (-not (Test-Path "$r\app.db")) 'app.db moved'
Check (Test-Path "$prev\app.db") 'app.db in previous'
Check (Test-Path "$prev\bin\php\8.4\php.exe") 'bin kept in previous'
Check (Test-Path "$prev\services\nginx") 'services moved'
Check ((Get-Content "$r\www\shop\index.php") -eq 'mine') 'www untouched'
Check (Test-Path "$r\data\mariadb\ibdata1") 'data stays in place'

# 2. Reset, delete bin, move data.
$r = New-Root 'root2'
$code = Run $r @('/BIN', '/DATA')
$prev = Get-Content (Join-Path $r 'harness-done.txt')
Check ($code -eq 0) "reset with options exits 0 ($code)"
Check (-not (Test-Path "$prev\bin")) 'bin deleted'
Check (-not (Test-Path "$r\bin")) 'no bin left in place'
Check (Test-Path "$prev\data\mariadb\ibdata1") 'data moved aside'
Check ((Get-Content "$r\www\shop\index.php") -eq 'mine') 'www untouched'

# 3. A file another program holds: nothing is reset.
$r = New-Root 'root3'
$lock = [System.IO.File]::Open("$r\logs\locked.log", 'Create', 'ReadWrite', 'None')
try {
  $code = Run $r @()
} finally { $lock.Close() }
Check ($code -ne 0) "locked file stops the install ($code)"
Check (Test-Path "$r\app.db") 'app.db put back'
Check (Test-Path "$r\services\nginx") 'services put back'
Check (Test-Path "$r\bin\php\8.4\php.exe") 'bin put back'
Check (-not (Test-Path "$r\previous")) 'no previous folder left'
Check (-not (Test-Path "$r\harness-done.txt")) 'install stopped'
'all passed'
