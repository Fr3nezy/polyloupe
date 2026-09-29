# Installs the Explorer thumbnail handler.
#
#   powershell -ExecutionPolicy Bypass -File scripts\install-thumbnails.ps1                       # current user
#   powershell -ExecutionPolicy Bypass -File scripts\install-thumbnails.ps1 -AllUsers             # all users (admin)
#   powershell -ExecutionPolicy Bypass -File scripts\install-thumbnails.ps1 [-AllUsers] -Uninstall
#
# Copies polyloupe_thumbs.dll out of the build folder (so rebuilding the project never fights with
# Windows holding the DLL), then registers the handler for .glb .gltf .fbx .obj .stl .ply .3mf .dae.
# The DLL renders on its own (no GPU, no polyloupe.exe) inside Windows' thumbnail process, which
# only hands it the file's bytes: .obj renders without its .mtl, .gltf with external files fails.
#
# Current user: %LOCALAPPDATA%\Programs\Poly Loupe, HKCU, no admin rights.
# All users: %ProgramFiles%\Poly Loupe, HKLM, what the installer does. Prefer it: the thumbnail
# process caches per-user COM classes and may not notice a per-user registration right away.

param([switch]$AllUsers, [switch]$Uninstall)

$ErrorActionPreference = 'Stop'

if ($AllUsers) {
    $admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
    if (-not $admin) {
        $argList = @('-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"", '-AllUsers')
        if ($Uninstall) { $argList += '-Uninstall' }
        $p = Start-Process powershell.exe -Verb RunAs -ArgumentList $argList -Wait -PassThru
        exit $p.ExitCode
    }
    $dest = Join-Path $env:ProgramFiles 'Poly Loupe'
    $scope = @('/n', '/i:allusers')
} else {
    $dest = Join-Path $env:LOCALAPPDATA 'Programs\Poly Loupe'
    $scope = @()
}
$dll = Join-Path $dest 'polyloupe_thumbs.dll'

# regsvr32 is a GUI program, so PowerShell does not wait for it or set $LASTEXITCODE on its own.
function Invoke-Regsvr32([string[]]$flags, [string]$path) {
    $p = Start-Process regsvr32.exe -ArgumentList ($flags + "`"$path`"") -Wait -PassThru
    if ($p.ExitCode -ne 0) { throw "regsvr32 $flags failed with code $($p.ExitCode)" }
}

if ($Uninstall) {
    if (Test-Path $dll) {
        Invoke-Regsvr32 (@('/s', '/u') + $scope) $dll
        Write-Host "Thumbnail handler unregistered. Files are left in $dest."
    } else {
        Write-Host 'Nothing to uninstall.'
    }
    return
}

$build = Join-Path $PSScriptRoot '..\target\release'
if (-not (Test-Path (Join-Path $build 'polyloupe_thumbs.dll'))) {
    throw 'Missing polyloupe_thumbs.dll. Build first: cargo build --release --workspace'
}

New-Item -ItemType Directory -Force $dest | Out-Null
# Windows' thumbnail process keeps the DLL loaded, so it cannot be overwritten. A loaded DLL can
# still be renamed: move it aside and delete the leftovers once nothing holds them.
Get-ChildItem $dest -Filter '*.old' | Remove-Item -Force -ErrorAction SilentlyContinue
if (Test-Path $dll) {
    try { Remove-Item $dll -Force } catch { Rename-Item $dll "polyloupe_thumbs.dll.$([guid]::NewGuid().ToString('N')).old" }
}
Copy-Item (Join-Path $build 'polyloupe_thumbs.dll') $dest
# The all-users registration also removes per-user ones, which would shadow it.
Invoke-Regsvr32 (@('/s') + $scope) $dll

Write-Host "Installed to $dest and registered for .glb .gltf .fbx .obj .stl .ply .3mf .dae."
Write-Host 'Existing thumbnails refresh once Windows drops its cache (new files show immediately).'
