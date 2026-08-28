# Launch Petrel on a throwaway mailbox of synthetic mail — the Windows sibling
# of run-demo.sh, and the first thing to run on a fresh Windows machine.
#
# Demo mode needs no account and no password, so a smoke test on a clean VM
# exercises the list, the reader, search and threading without a real mailbox
# ever being involved. That is the whole point of using it here.
#
#   .\scripts\run-demo.ps1                 a fresh mailbox under $env:TEMP
#   .\scripts\run-demo.ps1 C:\demo-store   keep it, and reuse it next time
#
# PETREL_DATA_DIR is set on this process before Start-Process, which the child
# inherits — the Windows equivalent of `open --env` on macOS, and for the same
# reason: the app must never be pointed at a real store by accident.
[CmdletBinding()]
param([string]$DataDir)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

if (-not $DataDir) {
    $DataDir = Join-Path $env:TEMP ("petrel-demo-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
}
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null

# An installed build first: on Windows the notification toast only appears for
# an app with a registered AppUserModelID, which the installer's Start Menu
# shortcut is what creates. A build run straight out of target\release is fine
# for everything else and silently posts no notifications at all.
$candidates = @(
    (Join-Path $env:LOCALAPPDATA 'Programs\Petrel\Petrel.exe'),
    'C:\Program Files\Petrel\Petrel.exe',
    (Join-Path $root 'target\release\petrel-desktop.exe')
)
$exe = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $exe) {
    Write-Error ("no Petrel found. Install the MSI or NSIS package, or build one with:`n" +
                 "  cargo tauri build --bundles msi,nsis`n" +
                 "Looked in:`n  " + ($candidates -join "`n  "))
}
if ($exe -like '*target\release*') {
    Write-Warning 'Running an uninstalled build: desktop notifications will not appear. Everything else works.'
}

Write-Host "store : $DataDir"
Write-Host "app   : $exe"

$env:PETREL_DATA_DIR = $DataDir
Start-Process -FilePath $exe

# Seeding is 10,000 synthetic messages and the filing that follows. Waiting for
# the database file to actually grow is the difference between looking at the
# app and looking at an empty window.
$db = Join-Path $DataDir 'petrel.db'
Write-Host -NoNewline 'seeding'
foreach ($i in 1..60) {
    Start-Sleep -Seconds 1
    Write-Host -NoNewline '.'
    # No sqlite3 on a stock Windows box, so size is the signal available
    # without asking the tester to install anything.
    if ((Test-Path $db) -and ((Get-Item $db).Length -gt 1MB)) {
        Write-Host ' ready'
        Write-Host ''
        Write-Host 'Smoke pass: search, open a conversation, open an attachment,'
        Write-Host 'and send yourself a test notification from Settings.'
        exit 0
    }
}
Write-Host ' still seeding; give it a moment'
