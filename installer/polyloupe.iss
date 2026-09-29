; Inno Setup script for PolyLoupe (all users, admin).
;
;   cargo build --release --workspace
;   iscc installer\polyloupe.iss
;
; Output: target\installer\PolyLoupe-Setup-<version>.exe
;
; The thumbnail handler is registered machine-wide (regsvr32 /n /i:allusers). It renders on its
; own inside Windows' isolated thumbnail process, so no GPU or helper process is involved.

#define AppVersion "0.1.0"

[Setup]
AppId={{24342D06-700F-425F-BDEF-C297766C9278}
AppName=PolyLoupe
AppVersion={#AppVersion}
AppPublisher=Manuel Franze
DefaultDirName={autopf}\PolyLoupe
DefaultGroupName=PolyLoupe
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\target\installer
OutputBaseFilename=PolyLoupe-Setup-{#AppVersion}
SetupIconFile=..\target\installer\app.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\polyloupe.exe
ChangesAssociations=yes
; Explorer holds the thumbnail DLL; closing Explorer to replace it is worse than renaming it.
CloseApplications=no
; Upgrades from the pre-rename "3D Viewer" install keep this AppId but move to the new folder.
UsePreviousAppDir=no
; Code signing for releases: register a sign tool named "signtool", for example
;   iscc /Ssigntool="signtool.exe sign /fd sha256 /tr http://timestamp.acs.microsoft.com /td sha256 $f" installer\polyloupe.iss
; and uncomment the two lines below.
;SignTool=signtool
;SignedUninstaller=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "italian"; MessagesFile: "compiler:Languages\Italian.isl"

[CustomMessages]
english.FileTypeName=3D model
italian.FileTypeName=Modello 3D
english.RegisterThumbs=Registering Explorer thumbnails...
italian.RegisterThumbs=Registrazione delle miniature di Esplora file...
english.ChooseDefault=Choose PolyLoupe as the default app for 3D files
italian.ChooseDefault=Scegli PolyLoupe come app predefinita per i file 3D
english.Launch=Launch PolyLoupe
italian.Launch=Avvia PolyLoupe

[Files]
Source: "..\target\release\polyloupe.exe"; DestDir: "{app}"; Flags: ignoreversion uninsrestartdelete
Source: "..\target\release\polyloupe_thumbs.dll"; DestDir: "{app}"; Flags: ignoreversion uninsrestartdelete
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\target\THIRD-PARTY-NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion

[Registry]
; File type for "Open with" and Default apps. Windows no longer lets an installer take over a file
; type silently: the user picks it (the last page offers to open Default apps).
Root: HKLM; Subkey: "Software\Classes\PolyLoupe.Model"; ValueType: string; ValueData: "{cm:FileTypeName}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\PolyLoupe.Model\DefaultIcon"; ValueType: string; ValueData: "{app}\polyloupe.exe,0"
Root: HKLM; Subkey: "Software\Classes\PolyLoupe.Model\shell\open\command"; ValueType: string; ValueData: """{app}\polyloupe.exe"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\Applications\polyloupe.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "PolyLoupe"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\Applications\polyloupe.exe\shell\open\command"; ValueType: string; ValueData: """{app}\polyloupe.exe"" ""%1"""
Root: HKLM; Subkey: "Software\PolyLoupe"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\PolyLoupe\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "PolyLoupe"
Root: HKLM; Subkey: "Software\PolyLoupe\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "Fast 3D model viewer with a Blender-style viewport"
Root: HKLM; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "PolyLoupe"; ValueData: "Software\PolyLoupe\Capabilities"; Flags: uninsdeletevalue
Root: HKLM; Subkey: "Software\Classes\.glb\OpenWithProgids"; ValueType: string; ValueName: "PolyLoupe.Model"; ValueData: ""; Flags: uninsdeletevalue
Root: HKLM; Subkey: "Software\Classes\.gltf\OpenWithProgids"; ValueType: string; ValueName: "PolyLoupe.Model"; ValueData: ""; Flags: uninsdeletevalue
Root: HKLM; Subkey: "Software\Classes\.fbx\OpenWithProgids"; ValueType: string; ValueName: "PolyLoupe.Model"; ValueData: ""; Flags: uninsdeletevalue
Root: HKLM; Subkey: "Software\Classes\.obj\OpenWithProgids"; ValueType: string; ValueName: "PolyLoupe.Model"; ValueData: ""; Flags: uninsdeletevalue
Root: HKLM; Subkey: "Software\Classes\.stl\OpenWithProgids"; ValueType: string; ValueName: "PolyLoupe.Model"; ValueData: ""; Flags: uninsdeletevalue
Root: HKLM; Subkey: "Software\Classes\.ply\OpenWithProgids"; ValueType: string; ValueName: "PolyLoupe.Model"; ValueData: ""; Flags: uninsdeletevalue
Root: HKLM; Subkey: "Software\Classes\.3mf\OpenWithProgids"; ValueType: string; ValueName: "PolyLoupe.Model"; ValueData: ""; Flags: uninsdeletevalue
Root: HKLM; Subkey: "Software\Classes\.dae\OpenWithProgids"; ValueType: string; ValueName: "PolyLoupe.Model"; ValueData: ""; Flags: uninsdeletevalue
Root: HKLM; Subkey: "Software\Classes\Applications\polyloupe.exe\SupportedTypes"; ValueType: string; ValueName: ".glb"; ValueData: ""
Root: HKLM; Subkey: "Software\Classes\Applications\polyloupe.exe\SupportedTypes"; ValueType: string; ValueName: ".gltf"; ValueData: ""
Root: HKLM; Subkey: "Software\Classes\Applications\polyloupe.exe\SupportedTypes"; ValueType: string; ValueName: ".fbx"; ValueData: ""
Root: HKLM; Subkey: "Software\Classes\Applications\polyloupe.exe\SupportedTypes"; ValueType: string; ValueName: ".obj"; ValueData: ""
Root: HKLM; Subkey: "Software\Classes\Applications\polyloupe.exe\SupportedTypes"; ValueType: string; ValueName: ".stl"; ValueData: ""
Root: HKLM; Subkey: "Software\Classes\Applications\polyloupe.exe\SupportedTypes"; ValueType: string; ValueName: ".ply"; ValueData: ""
Root: HKLM; Subkey: "Software\Classes\Applications\polyloupe.exe\SupportedTypes"; ValueType: string; ValueName: ".3mf"; ValueData: ""
Root: HKLM; Subkey: "Software\Classes\Applications\polyloupe.exe\SupportedTypes"; ValueType: string; ValueName: ".dae"; ValueData: ""
Root: HKLM; Subkey: "Software\PolyLoupe\Capabilities\FileAssociations"; ValueType: string; ValueName: ".glb"; ValueData: "PolyLoupe.Model"
Root: HKLM; Subkey: "Software\PolyLoupe\Capabilities\FileAssociations"; ValueType: string; ValueName: ".gltf"; ValueData: "PolyLoupe.Model"
Root: HKLM; Subkey: "Software\PolyLoupe\Capabilities\FileAssociations"; ValueType: string; ValueName: ".fbx"; ValueData: "PolyLoupe.Model"
Root: HKLM; Subkey: "Software\PolyLoupe\Capabilities\FileAssociations"; ValueType: string; ValueName: ".obj"; ValueData: "PolyLoupe.Model"
Root: HKLM; Subkey: "Software\PolyLoupe\Capabilities\FileAssociations"; ValueType: string; ValueName: ".stl"; ValueData: "PolyLoupe.Model"
Root: HKLM; Subkey: "Software\PolyLoupe\Capabilities\FileAssociations"; ValueType: string; ValueName: ".ply"; ValueData: "PolyLoupe.Model"
Root: HKLM; Subkey: "Software\PolyLoupe\Capabilities\FileAssociations"; ValueType: string; ValueName: ".3mf"; ValueData: "PolyLoupe.Model"
Root: HKLM; Subkey: "Software\PolyLoupe\Capabilities\FileAssociations"; ValueType: string; ValueName: ".dae"; ValueData: "PolyLoupe.Model"

; Leftovers of the builds named "Poly Loupe" (with a space).
Root: HKLM; Subkey: "Software\Poly Loupe"; ValueType: none; Flags: deletekey
Root: HKLM; Subkey: "Software\RegisteredApplications"; ValueType: none; ValueName: "Poly Loupe"; Flags: deletevalue

; Leftovers of the pre-rename "3D Viewer" builds.
Root: HKLM; Subkey: "Software\Classes\3DViewer.Model"; ValueType: none; Flags: deletekey
Root: HKLM; Subkey: "Software\3D Viewer"; ValueType: none; Flags: deletekey
Root: HKLM; Subkey: "Software\Classes\Applications\viewer3d.exe"; ValueType: none; Flags: deletekey
Root: HKLM; Subkey: "Software\RegisteredApplications"; ValueType: none; ValueName: "3D Viewer"; Flags: deletevalue
Root: HKLM; Subkey: "Software\Classes\.glb\OpenWithProgids"; ValueType: none; ValueName: "3DViewer.Model"; Flags: deletevalue
Root: HKLM; Subkey: "Software\Classes\.gltf\OpenWithProgids"; ValueType: none; ValueName: "3DViewer.Model"; Flags: deletevalue
Root: HKLM; Subkey: "Software\Classes\.fbx\OpenWithProgids"; ValueType: none; ValueName: "3DViewer.Model"; Flags: deletevalue
Root: HKLM; Subkey: "Software\Classes\.obj\OpenWithProgids"; ValueType: none; ValueName: "3DViewer.Model"; Flags: deletevalue
Root: HKLM; Subkey: "Software\Classes\.stl\OpenWithProgids"; ValueType: none; ValueName: "3DViewer.Model"; Flags: deletevalue

[InstallDelete]
Type: filesandordirs; Name: "{autopf}\Poly Loupe"
Type: files; Name: "{autoprograms}\Poly Loupe.lnk"
Type: filesandordirs; Name: "{autopf}\3D Viewer"
Type: files; Name: "{autoprograms}\3D Viewer.lnk"

[Icons]
Name: "{autoprograms}\PolyLoupe"; Filename: "{app}\polyloupe.exe"

[Run]
Filename: "{sys}\regsvr32.exe"; Parameters: "/s /n /i:allusers ""{app}\polyloupe_thumbs.dll"""; \
    Flags: runhidden waituntilterminated; StatusMsg: "{cm:RegisterThumbs}"
Filename: "ms-settings:defaultapps?registeredAppMachine=PolyLoupe"; Description: "{cm:ChooseDefault}"; \
    Flags: shellexec postinstall skipifsilent unchecked
Filename: "{app}\polyloupe.exe"; Description: "{cm:Launch}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\regsvr32.exe"; Parameters: "/s /u /n /i:allusers ""{app}\polyloupe_thumbs.dll"""; \
    Flags: runhidden waituntilterminated; RunOnceId: "UnregisterThumbnails"

[Code]
// Windows' thumbnail process (or Explorer, with earlier builds) may hold the thumbnail DLL, and
// the app may be open: files in use cannot be overwritten but can be renamed. Move them aside so
// the new ones can be copied, and clear leftovers from earlier updates once nothing holds them.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Dir, Dll: String;
  Found: TFindRec;
begin
  Result := '';
  Dir := ExpandConstant('{app}');
  if FindFirst(Dir + '\*.old', Found) then
  begin
    try
      repeat
        DeleteFile(Dir + '\' + Found.Name);
      until not FindNext(Found);
    finally
      FindClose(Found);
    end;
  end;
  Dll := Dir + '\polyloupe_thumbs.dll';
  if FileExists(Dll) and not DeleteFile(Dll) then
    RenameFile(Dll, Dll + '.' + GetDateTimeString('yyyymmddhhnnss', #0, #0) + '.old');
  Dll := Dir + '\polyloupe.exe';
  if FileExists(Dll) and not DeleteFile(Dll) then
    RenameFile(Dll, Dll + '.' + GetDateTimeString('yyyymmddhhnnss', #0, #0) + '.old');
end;
