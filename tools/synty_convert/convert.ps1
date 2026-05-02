# Synty FBX -> GLB batch converter wrapper.
# Reads manifest.toml and calls Blender once per file so a crash in one FBX
# does not abort the entire batch.
#
# Usage (from repo root):
#   tools\synty_convert\convert.ps1 `
#       -SrcFbxDir  "C:\...\POLYGON_Fantasy_Kingdom_SourceFiles_v5\Source_Files\FBX" `
#       -DstAssetDir "crates\client\assets"
#
# Blender 5.1 must be installed at the default path, or set $env:BLENDER_EXE.

param(
    [Parameter(Mandatory)][string]$SrcFbxDir,
    [Parameter(Mandatory)][string]$DstAssetDir
)

$ErrorActionPreference = "Continue"

$blender = if ($env:BLENDER_EXE) { $env:BLENDER_EXE } else {
    "C:\Program Files\Blender Foundation\Blender 5.1\blender.exe"
}
$scriptPy  = Join-Path $PSScriptRoot "convert.py"
$manifest  = Join-Path $PSScriptRoot "manifest.toml"

if (-not (Test-Path $blender))   { Write-Error "Blender not found: $blender"; exit 1 }
if (-not (Test-Path $scriptPy))  { Write-Error "convert.py not found"; exit 1 }
if (-not (Test-Path $manifest))  { Write-Error "manifest.toml not found"; exit 1 }

# Parse manifest.toml without a TOML library (simple line scan).
$entries = @()
$currentSrc = $null
foreach ($line in Get-Content $manifest) {
    if ($line -match '^\s*src\s*=\s*"(.+)"') {
        $currentSrc = $matches[1]
    } elseif ($line -match '^\s*dst\s*=\s*"(.+)"') {
        if ($currentSrc) {
            $entries += [PSCustomObject]@{ Src = $currentSrc; Dst = $matches[1] }
            $currentSrc = $null
        }
    }
}

$total     = $entries.Count
$converted = 0
$skipped   = 0
$failed    = 0

Write-Host ""
Write-Host "=== Synty FBX -> GLB conversion ($total entries) ==="
Write-Host "src : $SrcFbxDir"
Write-Host "dst : $DstAssetDir"
Write-Host ""

foreach ($e in $entries) {
    $srcPath = Join-Path $SrcFbxDir $e.Src
    $dstPath = Join-Path $DstAssetDir $e.Dst

    if (-not (Test-Path $srcPath)) {
        Write-Host "  MISSING  $($e.Src)"
        $failed++
        continue
    }

    # Skip if GLB is newer than the FBX source.
    if ((Test-Path $dstPath) -and ((Get-Item $dstPath).LastWriteTime -ge (Get-Item $srcPath).LastWriteTime)) {
        Write-Host "  skip     $($e.Dst)  (up to date)"
        $skipped++
        continue
    }

    Write-Host "  convert  $($e.Src)  ->  $($e.Dst)"

    # Run Blender as a subprocess — one crash does not kill the loop.
    $proc = Start-Process -FilePath $blender `
        -ArgumentList "--background", "--python", $scriptPy, "--", $srcPath, $dstPath `
        -Wait -PassThru -NoNewWindow

    if ($proc.ExitCode -eq 0 -and (Test-Path $dstPath)) {
        $converted++
    } else {
        Write-Host "           FAILED (exit $($proc.ExitCode))"
        $failed++
    }
}

Write-Host ""
Write-Host "=== Done: $converted converted, $skipped skipped, $failed failed ==="
Write-Host ""

if ($failed -gt 0) { exit 1 }
