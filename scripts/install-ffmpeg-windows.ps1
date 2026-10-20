<#
.SYNOPSIS
  Installs the FFmpeg that Unsub links on Windows, from a pinned vcpkg commit.

.DESCRIPTION
  vcpkg's ffmpeg port with only avcodec, avformat, swresample and the two external encoders the
  transcoder asks for (libmp3lame, libopus) is LGPL-2.1+: no GPL or nonfree libraries, which is
  also all the Windows installer has to ship (see scripts/THIRD_PARTY.md).

  Pin: vcpkg release 2026.07.29 carries ffmpeg 8.1.2, the version the macOS build uses.
  Outputs (when run by GitHub Actions): FFMPEG_DIR, LIBCLANG_PATH and PATH, so that
  ffmpeg-sys-next finds the headers and import libraries and test binaries find the DLLs.

.PARAMETER VcpkgCommit
  The vcpkg commit to check out.

.PARAMETER VcpkgRoot
  Where to keep vcpkg. Defaults to a folder in the runner's temp directory.

.PARAMETER StageDir
  Optional. Copies the FFmpeg DLLs here, and the licence texts of the installed ports into a
  "licenses" folder below it, for the installer's bundle.resources.
#>
param(
  [Parameter(Mandatory = $true)][string]$VcpkgCommit,
  [string]$VcpkgRoot = (Join-Path ($env:RUNNER_TEMP ?? $env:TEMP) 'vcpkg'),
  [string]$StageDir = ''
)
$ErrorActionPreference = 'Stop'

function Invoke-Native {
  param([string]$File, [string[]]$Arguments)
  & $File @Arguments
  if ($LASTEXITCODE -ne 0) {
    throw "$File $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
  }
}

# A one-commit fetch is enough: classic-mode vcpkg needs no history, and it is far faster to
# download than a full clone.
if (-not (Test-Path (Join-Path $VcpkgRoot '.git'))) {
  Invoke-Native git @('init', '--quiet', $VcpkgRoot)
  Invoke-Native git @('-C', $VcpkgRoot, 'remote', 'add', 'origin', 'https://github.com/microsoft/vcpkg.git')
}
Invoke-Native git @('-C', $VcpkgRoot, 'fetch', '--quiet', '--depth', '1', 'origin', $VcpkgCommit)
Invoke-Native git @('-C', $VcpkgRoot, 'checkout', '--quiet', '--detach', 'FETCH_HEAD')
Invoke-Native (Join-Path $VcpkgRoot 'bootstrap-vcpkg.bat') @('-disableMetrics')

if ($env:VCPKG_DEFAULT_BINARY_CACHE) {
  New-Item -ItemType Directory -Force -Path $env:VCPKG_DEFAULT_BINARY_CACHE | Out-Null
}
Invoke-Native (Join-Path $VcpkgRoot 'vcpkg.exe') @(
  'install', 'ffmpeg[core,avcodec,avformat,swresample,mp3lame,opus]:x64-windows', '--clean-after-build'
)

$installed = Join-Path $VcpkgRoot 'installed\x64-windows'
foreach ($library in 'avcodec', 'avformat', 'avutil', 'swresample') {
  if (-not (Test-Path (Join-Path $installed "lib\$library.lib"))) {
    throw "vcpkg did not produce $library.lib in $installed\lib"
  }
}

if ($env:GITHUB_ENV) {
  Add-Content -Path $env:GITHUB_ENV -Value "FFMPEG_DIR=$installed"
  # bindgen (ffmpeg-sys-next) needs libclang; the runner image ships LLVM here.
  Add-Content -Path $env:GITHUB_ENV -Value 'LIBCLANG_PATH=C:\Program Files\LLVM\bin'
  Add-Content -Path $env:GITHUB_PATH -Value (Join-Path $installed 'bin')
}

if ($StageDir) {
  New-Item -ItemType Directory -Force -Path $StageDir | Out-Null
  Copy-Item -Path (Join-Path $installed 'bin\*.dll') -Destination $StageDir
  Get-ChildItem $StageDir -Filter *.dll | ForEach-Object { Write-Host "staged $($_.Name)" }

  # vcpkg keeps each port's licence at share\<port>\copyright; they go into the installer too.
  $licenses = Join-Path $StageDir 'licenses'
  New-Item -ItemType Directory -Force -Path $licenses | Out-Null
  Get-ChildItem (Join-Path $installed 'share') -Directory | ForEach-Object {
    $copyright = Join-Path $_.FullName 'copyright'
    if (Test-Path $copyright) { Copy-Item $copyright (Join-Path $licenses "$($_.Name).txt") }
  }
}
