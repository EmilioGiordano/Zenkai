#Requires -Version 5.1
# Places pdfium.dll next to zenkai.exe. Zenkai loads it from its own folder only, and shows
# "PDF viewer unavailable" when it is missing.
#   fetch-pdfium.ps1 -Destination target\debug     (to read PDFs from a development build)
#   build.ps1 runs it for the release exe and puts the DLL in the zip and the installer.
param(
    [Parameter(Mandatory = $true)]
    [string]$Destination
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# bblanchon/pdfium-binaries, the build without V8: PDF JavaScript cannot run at all.
# The release must match the pdfium_NNNN feature of the pdfium-render dependency.
$release = 'chromium/7881'
$asset = 'pdfium-win-x64.tgz'
$archiveSha256 = '73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08'
$dllSha256 = '79d4676b656cfb1abcea88f9ade3b4b0826c5200382db5f4ec72a636c598c118'

function Get-Sha256([string]$path) {
    (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()
}

New-Item -ItemType Directory -Force $Destination | Out-Null
$target = Join-Path $Destination 'pdfium.dll'
if ((Test-Path $target) -and ((Get-Sha256 $target) -eq $dllSha256)) {
    Write-Host "pdfium.dll is already in $Destination."
    return
}

$work = Join-Path ([IO.Path]::GetTempPath()) ("zenkai-pdfium-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $work | Out-Null
try {
    $archive = Join-Path $work $asset
    $url = "https://github.com/bblanchon/pdfium-binaries/releases/download/$release/$asset"
    Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing
    if ((Get-Sha256 $archive) -ne $archiveSha256) {
        throw "The checksum of $asset does not match the pinned one; nothing was installed."
    }
    $unpacked = Join-Path $work 'unpacked'
    New-Item -ItemType Directory $unpacked | Out-Null
    & (Join-Path $env:SystemRoot 'System32\tar.exe') -xzf $archive -C $unpacked
    if ($LASTEXITCODE -ne 0) { throw 'tar could not unpack the archive' }
    $dll = Join-Path $unpacked 'bin\pdfium.dll'
    if ((Get-Sha256 $dll) -ne $dllSha256) {
        throw 'The pdfium.dll inside the archive does not match the pinned checksum; nothing was installed.'
    }
    Copy-Item -LiteralPath $dll -Destination $target -Force
    Copy-Item -LiteralPath (Join-Path $unpacked 'LICENSE') -Destination (Join-Path $Destination 'pdfium-LICENSE.txt') -Force
    Write-Host "pdfium.dll ($release) is in $Destination."
} finally {
    Remove-Item -Recurse -Force $work
}
