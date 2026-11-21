#Requires -Version 5.1
# Installs a setup.exe silently for the current user and checks what the updater relies on: the
# launcher layout, a per-user folder the user can write and rename in, and an uninstall entry
# under HKCU only. Then reinstalls over an update's leftovers and uninstalls, checking both clean
# them and keep %LOCALAPPDATA%\Zenkai.
# It really installs into the current user's profile: meant for a CI runner or a test account.
param(
    [Parameter(Mandatory = $true)]
    [string]$Setup
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$appId = '{20C0FE1B-59E0-4985-8377-20D8CBD20083}_is1'
$userEntry = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$appId"
$machineEntry = "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$appId"
$expected = Join-Path $env:LOCALAPPDATA 'Programs\Zenkai'
$data = Join-Path $env:LOCALAPPDATA 'Zenkai'
$Setup = (Resolve-Path $Setup).Path

function Assert-True([bool]$condition, [string]$message) {
    if (-not $condition) { throw "Install check failed: $message" }
}

function Invoke-Silently([string]$program) {
    $arguments = '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-'
    $process = Start-Process -FilePath $program -ArgumentList $arguments -Wait -PassThru
    Assert-True ($process.ExitCode -eq 0) "$program exited with $($process.ExitCode)"
}

function Assert-Layout([string]$root) {
    foreach ($file in 'zenkai.exe', 'zenkai-mcp.exe', 'LICENSE', 'README.md', 'app\zenkai.exe', 'app\zenkai-pdf-host.exe', 'app\pdfium.dll', 'app\pdfium-LICENSE.txt') {
        Assert-True (Test-Path -LiteralPath (Join-Path $root $file) -PathType Leaf) "$file is missing"
    }
    foreach ($file in 'zenkai-pdf-host.exe', 'pdfium.dll') {
        Assert-True (-not (Test-Path -LiteralPath (Join-Path $root $file))) "$file is at the root instead of in app"
    }
}

function Add-Leftovers([string]$root) {
    New-Item -ItemType Directory -Force (Join-Path $root 'app.old'), (Join-Path $root '.update\new-9.9.9') | Out-Null
    Set-Content -LiteralPath (Join-Path $root 'app.old\zenkai.exe') -Value 'previous'
    Set-Content -LiteralPath (Join-Path $root '.update\new-9.9.9\zenkai.exe') -Value 'next'
    Set-Content -LiteralPath (Join-Path $root '.update\confirmed') -Value '9.9.9' -NoNewline
    Set-Content -LiteralPath (Join-Path $root '.update\lock') -Value '' -NoNewline
    Set-Content -LiteralPath (Join-Path $root 'app\stray.txt') -Value 'from an update'
}

Assert-True (-not (Test-Path $userEntry)) 'Zenkai is already installed for this user'

Invoke-Silently $Setup
$root = (Get-ItemProperty -LiteralPath $userEntry).InstallLocation.TrimEnd('\')
Assert-True ($root -eq $expected) "installed in $root instead of $expected"
Assert-True (-not (Test-Path $machineEntry)) 'the uninstall entry is under HKLM'
Assert-Layout $root

# What the updater does to the install, done as this user.
$probe = Join-Path $root '.update\write-check'
New-Item -ItemType Directory -Force (Split-Path $probe) | Out-Null
Set-Content -LiteralPath $probe -Value 'x'
Remove-Item -LiteralPath $probe
Rename-Item -LiteralPath (Join-Path $root 'app') -NewName 'app.old'
Rename-Item -LiteralPath (Join-Path $root 'app.old') -NewName 'app'
Assert-Layout $root

Add-Leftovers $root
Invoke-Silently $Setup
Assert-Layout $root
foreach ($left in 'app.old', '.update', 'app\stray.txt') {
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $root $left))) "reinstalling left $left"
}

New-Item -ItemType Directory -Force $data | Out-Null
$kept = Join-Path $data "install-check-$PID.txt"
Set-Content -LiteralPath $kept -Value 'kept'
Add-Leftovers $root
$uninstaller = (Get-ItemProperty -LiteralPath $userEntry).UninstallString.Trim('"')
Invoke-Silently $uninstaller
# The uninstaller hands over to a copy of itself in TEMP and returns before it finishes.
$deadline = (Get-Date).AddSeconds(120)
while (((Test-Path $userEntry) -or (Test-Path -LiteralPath (Join-Path $root 'app'))) -and (Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 500
}
Assert-True (-not (Test-Path $userEntry)) 'the uninstall entry is still there'
foreach ($left in 'app', 'app.old', '.update', 'zenkai.exe', 'zenkai-mcp.exe') {
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $root $left))) "uninstalling left $left"
}
Assert-True (Test-Path -LiteralPath $kept) 'uninstalling removed %LOCALAPPDATA%\Zenkai'
Remove-Item -LiteralPath $kept
Write-Host 'The installer, a reinstall over leftovers and the uninstaller behave as the updater needs.'
