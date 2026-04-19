<#
.SYNOPSIS
    Build a distributable Freetasia bundle with ffmpeg included.

.DESCRIPTION
    1. Builds Freetasia in release mode.
    2. Downloads a pre-built ffmpeg (LGPL, shared) from BtbN/FFmpeg-Builds.
    3. Assembles everything into dist/Freetasia/ ready for zipping.

.NOTES
    Run from the repository root:  .\scripts\bundle.ps1
#>

param(
    [string]$OutputDir = "dist\Freetasia"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Push-Location $RepoRoot

# ── 1. Build release ──────────────────────────────────────────────────────────
Write-Host "==> Building Freetasia (release)..." -ForegroundColor Cyan
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "Cargo build failed" }

# ── 2. Prepare output directory ──────────────────────────────────────────────
if (Test-Path $OutputDir) { Remove-Item $OutputDir -Recurse -Force }
New-Item -ItemType Directory -Path $OutputDir | Out-Null

# ── 3. Copy Freetasia binary ─────────────────────────────────────────────────
Copy-Item "target\release\freetasia.exe" "$OutputDir\freetasia.exe"
Write-Host "    Copied freetasia.exe" -ForegroundColor Green

# ── 4. Download ffmpeg if not cached ─────────────────────────────────────────
$FfmpegCache = "target\tmp\ffmpeg"
$FfmpegZip   = "target\tmp\ffmpeg.zip"
$FfmpegUrl   = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl-shared.zip"

if (-not (Test-Path "$FfmpegCache\ffmpeg.exe")) {
    Write-Host "==> Downloading ffmpeg (GPL shared build)..." -ForegroundColor Cyan
    New-Item -ItemType Directory -Path "target\tmp" -Force | Out-Null

    # Download
    Invoke-WebRequest -Uri $FfmpegUrl -OutFile $FfmpegZip -UseBasicParsing
    Write-Host "    Downloaded $FfmpegZip" -ForegroundColor Green

    # Extract — the zip contains a top-level folder; we want bin/*
    $ExtractDir = "target\tmp\ffmpeg_extract"
    if (Test-Path $ExtractDir) { Remove-Item $ExtractDir -Recurse -Force }
    Expand-Archive -Path $FfmpegZip -DestinationPath $ExtractDir

    # Find the bin directory inside the extracted folder.
    $BinDir = Get-ChildItem -Path $ExtractDir -Recurse -Directory -Filter "bin" |
              Select-Object -First 1

    if (-not $BinDir) { throw "Could not find bin/ in ffmpeg archive" }

    New-Item -ItemType Directory -Path $FfmpegCache -Force | Out-Null
    Copy-Item "$($BinDir.FullName)\*" -Destination $FfmpegCache -Recurse
    Write-Host "    Extracted ffmpeg binaries to cache" -ForegroundColor Green

    # Clean up zip and extract dir.
    Remove-Item $FfmpegZip -Force
    Remove-Item $ExtractDir -Recurse -Force
}

# ── 5. Copy ffmpeg binaries into bundle ──────────────────────────────────────
# Place ffmpeg.exe (and DLLs) directly next to freetasia.exe so find_ffmpeg()
# picks it up automatically.
Copy-Item "$FfmpegCache\*" -Destination $OutputDir -Recurse
Write-Host "    Bundled ffmpeg binaries" -ForegroundColor Green

# ── 6. Copy docs / licenses ─────────────────────────────────────────────────
Copy-Item "README.md"                "$OutputDir\README.md"
Copy-Item "THIRD_PARTY_LICENSES.md"  "$OutputDir\THIRD_PARTY_LICENSES.md"
if (Test-Path "LICENSE") {
    Copy-Item "LICENSE" "$OutputDir\LICENSE"
}

Write-Host ""
Write-Host "==> Bundle ready at: $OutputDir" -ForegroundColor Green
Write-Host "    Zip this folder and distribute!" -ForegroundColor Green

Pop-Location
