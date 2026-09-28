# Builds the release binaries and the installer: target\installer\PolyLoupe-Setup-<version>.exe
#
#   powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1
#
# Needs Inno Setup 6 (winget install JRSoftware.InnoSetup).

$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')

& cargo build --release --workspace --manifest-path (Join-Path $root 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

# build.rs draws the icon into its OUT_DIR; the setup wizard uses the same one.
$ico = Get-ChildItem (Join-Path $root 'target\release\build') -Recurse -Filter app.ico |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $ico) { throw 'app.ico not found: did build.rs run?' }
$out = Join-Path $root 'target\installer'
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item $ico.FullName (Join-Path $out 'app.ico') -Force

$iscc = @(
    (Get-Command iscc -ErrorAction SilentlyContinue).Source,
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup not found: winget install JRSoftware.InnoSetup' }

& $iscc /Q (Join-Path $root 'installer\polyloupe.iss')
if ($LASTEXITCODE -ne 0) { throw 'ISCC failed' }
Get-ChildItem $out -Filter '*.exe' | Select-Object Name, Length
