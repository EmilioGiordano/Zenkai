#Requires -Version 5.1
# Lays out the Windows install once and packs that same folder as the portable zip and, given
# iscc, as the installer, so both always match:
#   <name>\zenkai.exe        the launcher (zenkai-launcher.exe from the build)
#   <name>\zenkai-mcp.exe    the relay agents register
#   <name>\LICENSE, README.md
#   <name>\app\              the app, its helpers and pdfium; the only folder the updater replaces
# Run by build.ps1 and by the release workflow.
param(
    [Parameter(Mandatory = $true)]
    [string]$ExeDir,
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [Parameter(Mandatory = $true)]
    [string]$Name,
    [Parameter(Mandatory = $true)]
    [string]$OutputDir,
    [string]$Iscc
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$ExeDir = (Resolve-Path $ExeDir).Path
New-Item -ItemType Directory -Force $OutputDir | Out-Null
$OutputDir = (Resolve-Path $OutputDir).Path

$layout = Join-Path $OutputDir $Name
if (Test-Path $layout) { Remove-Item -Recurse -Force $layout }
$app = Join-Path $layout 'app'
New-Item -ItemType Directory -Force $app | Out-Null

Copy-Item -LiteralPath (Join-Path $ExeDir 'zenkai-launcher.exe') -Destination (Join-Path $layout 'zenkai.exe')
foreach ($file in (Join-Path $ExeDir 'zenkai-mcp.exe'), (Join-Path $root 'LICENSE'), (Join-Path $root 'README.md')) {
    Copy-Item -LiteralPath $file -Destination $layout
}
foreach ($file in 'zenkai.exe', 'zenkai-pdf-host.exe', 'pdfium.dll', 'pdfium-LICENSE.txt') {
    Copy-Item -LiteralPath (Join-Path $ExeDir $file) -Destination $app
}

try {
    # tar writes zip entries with forward slashes on every Windows PowerShell, which the
    # updater's extractor expects; Compress-Archive in Windows PowerShell 5.1 does not.
    $zip = Join-Path $OutputDir "$Name.zip"
    if (Test-Path $zip) { Remove-Item -Force $zip }
    & (Join-Path $env:SystemRoot 'System32\tar.exe') -a -c -f $zip -C $OutputDir $Name
    if ($LASTEXITCODE -ne 0) { throw 'tar could not write the zip' }
    Write-Host "Portable: $zip"

    if ($Iscc) {
        & $Iscc "/DAppVersion=$Version" "/DLayoutDir=$layout" "/DOutputDir=$OutputDir" "/DOutputBaseName=$Name-setup" (Join-Path $PSScriptRoot 'zenkai.iss')
        if ($LASTEXITCODE -ne 0) { throw 'iscc failed' }
        Write-Host "Installer: $(Join-Path $OutputDir "$Name-setup.exe")"
    }
} finally {
    Remove-Item -Recurse -Force $layout
}
