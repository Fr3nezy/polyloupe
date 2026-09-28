# Installs the Explorer thumbnail handler.
#
#   powershell -ExecutionPolicy Bypass -File scripts\install-thumbnails.ps1                       # current user
#   powershell -ExecutionPolicy Bypass -File scripts\install-thumbnails.ps1 -AllUsers             # all users (admin)
#   powershell -ExecutionPolicy Bypass -File scripts\install-thumbnails.ps1 [-AllUsers] -Uninstall
#
# Copies polyloupe.exe and polyloupe_thumbs.dll out of the build folder (so rebuilding the project
# never fights with Explorer holding the DLL), then registers the handler for
# .glb .gltf .fbx .obj .stl.
#
# Current user: %LOCALAPPDATA%\Programs\Poly Loupe, HKCU, no admin rights. Explorer only hands the
# handler the file's bytes, so .obj renders without its .mtl and .gltf with external files fails.
# All users: %ProgramFiles%\Poly Loupe, HKLM, what the installer does. .gltf and .obj are loaded in
# process with their real path, so their materials, buffers and textures are found.

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
foreach ($file in 'polyloupe.exe', 'polyloupe_thumbs.dll') {
    if (-not (Test-Path (Join-Path $build $file))) {
        throw "Missing $file. Build first: cargo build --release --workspace"
    }
}

New-Item -ItemType Directory -Force $dest | Out-Null
# Explorer keeps the DLL loaded (in process for .gltf/.obj), so it cannot be overwritten. A loaded
# DLL can still be renamed: move it aside and delete the leftovers once nothing holds them.
Get-ChildItem $dest -Filter '*.old' | Remove-Item -Force -ErrorAction SilentlyContinue
foreach ($file in 'polyloupe.exe', 'polyloupe_thumbs.dll') {
    $target = Join-Path $dest $file
    if (Test-Path $target) {
        try { Remove-Item $target -Force } catch { Rename-Item $target "$file.$([guid]::NewGuid().ToString('N')).old" }
    }
    Copy-Item (Join-Path $build $file) $dest
}
Invoke-Regsvr32 (@('/s') + $scope) $dll

if ($AllUsers) {
    # A per-user registration of the same handler would shadow the machine-wide one.
    $userDll = Join-Path $env:LOCALAPPDATA 'Programs\Poly Loupe\polyloupe_thumbs.dll'
    if (Test-Path $userDll) { Invoke-Regsvr32 @('/s', '/u') $userDll }
}

Write-Host "Installed to $dest and registered for .glb .gltf .fbx .obj .stl."
Write-Host 'Existing thumbnails refresh once Windows drops its cache (new files show immediately).'
