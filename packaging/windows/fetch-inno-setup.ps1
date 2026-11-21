#Requires -Version 5.1
# Installs the pinned Inno Setup into -Destination and prints the path of its compiler, ISCC.exe.
# For the release workflow, whose runner has no Inno Setup of its own; locally, install Inno Setup
# and build.ps1 finds iscc on PATH.
param(
    [Parameter(Mandatory = $true)]
    [string]$Destination
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# jrsoftware/issrc immutable release; the hash matches the file-hash of its published .issig.
$version = '6.7.4'
$url = 'https://github.com/jrsoftware/issrc/releases/download/is-6_7_4/innosetup-6.7.4.exe'
$sha256 = 'a549929319f9120716677c685535ca4671e4b937b5fdc87e2585f32d8eb30e8c'

$work = Join-Path ([IO.Path]::GetTempPath()) ("zenkai-inno-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $work | Out-Null
try {
    $installer = Join-Path $work "innosetup-$version.exe"
    Invoke-WebRequest -Uri $url -OutFile $installer -UseBasicParsing
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $installer).Hash.ToLowerInvariant() -ne $sha256) {
        throw "The checksum of Inno Setup $version does not match the pinned one; nothing was installed."
    }
    $arguments = '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', "/DIR=`"$Destination`""
    $process = Start-Process -FilePath $installer -ArgumentList $arguments -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "The Inno Setup installer exited with $($process.ExitCode)." }
} finally {
    Remove-Item -Recurse -Force $work
}

$compiler = Join-Path $Destination 'ISCC.exe'
if (-not (Test-Path -LiteralPath $compiler -PathType Leaf)) { throw "ISCC.exe is not in $Destination." }
Write-Host "Inno Setup $version is in $Destination."
Write-Output $compiler
