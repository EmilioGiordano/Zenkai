#Requires -Version 5.1
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$dist = Join-Path $root 'dist'

Push-Location $root
try {
    $metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $version = ($metadata.packages | Where-Object { $_.name -eq 'zenkai' }).version

    cargo build --profile dist -p zenkai -p zenkai-mcp -j 6
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    $exeDir = Join-Path $root 'target\dist'

    $name = "zenkai-$version-windows-x64"
    $staging = Join-Path $dist $name
    if (Test-Path $staging) { Remove-Item -Recurse -Force $staging }
    New-Item -ItemType Directory -Force $staging | Out-Null
    Copy-Item (Join-Path $exeDir 'zenkai.exe'), (Join-Path $exeDir 'zenkai-mcp.exe'), (Join-Path $root 'LICENSE'), (Join-Path $root 'README.md') $staging
    $zip = Join-Path $dist "$name.zip"
    Compress-Archive -Path $staging -DestinationPath $zip -Force
    Remove-Item -Recurse -Force $staging
    Write-Host "Portable: $zip"

    $iscc = Get-Command iscc -ErrorAction SilentlyContinue
    if ($null -eq $iscc) {
        Write-Host 'iscc is not on PATH; skipping the installer (install Inno Setup 6.3 or later to build it).'
    } else {
        & $iscc.Source "/DAppVersion=$version" "/DExeDir=$exeDir" "/DOutputDir=$dist" (Join-Path $PSScriptRoot 'zenkai.iss')
        if ($LASTEXITCODE -ne 0) { throw 'iscc failed' }
        Write-Host "Installer: $(Join-Path $dist "$name-setup.exe")"
    }
} finally {
    Pop-Location
}
