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

    cargo build --profile dist -p zenkai -p zenkai-mcp -p zenkai-pdf-host -p zenkai-values-host -p zenkai-launcher -j 6
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    $exeDir = Join-Path $root 'target\dist'
    & (Join-Path $PSScriptRoot 'fetch-pdfium.ps1') -Destination $exeDir
    if (-not $?) { throw 'fetching pdfium.dll failed' }

    $iscc = Get-Command iscc -ErrorAction SilentlyContinue
    if ($null -eq $iscc) {
        Write-Host 'iscc is not on PATH; skipping the installer (install Inno Setup 6.3 or later to build it).'
        $isccPath = ''
    } else {
        $isccPath = $iscc.Source
    }
    & (Join-Path $PSScriptRoot 'package.ps1') -ExeDir $exeDir -Version $version -Name "zenkai-$version-windows-x64" -OutputDir $dist -Iscc $isccPath
    if (-not $?) { throw 'packaging failed' }
} finally {
    Pop-Location
}
