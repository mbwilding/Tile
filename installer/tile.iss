; Builds Tile-Setup-<version>.exe: `iscc /DAppVersion=1.2.3 /DSourceExe=..\target\release\tile.exe installer\tile.iss`.
; Setup runs as the signed-in user and only hands over to `tile install` / `tile uninstall`,
; which do the real work (the UAC step, the sign-in task, the user's registry values).

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\target\release\tile.exe"
#endif

[Setup]
AppId={{6B2F7C0E-4D0B-4E55-9B53-6C1A5E8C2A41}
AppName=Tile
AppVersion={#AppVersion}
AppPublisher=Matthew Wilding
AppPublisherURL=https://github.com/mbwilding/tile
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Tile Setup
DisableDirPage=yes
DisableProgramGroupPage=yes
OutputDir=..\target\installer
OutputBaseFilename=Tile-Setup-{#AppVersion}
UninstallDisplayName=Tile
UninstallDisplayIcon={app}\tile.exe
Compression=lzma2
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=no

[Files]
; Kept so the uninstaller can run `tile.exe uninstall`; the installed copy is in Program Files.
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion

[UninstallRun]
Filename: "{app}\tile.exe"; Parameters: "uninstall"; Flags: runhidden waituntilterminated; RunOnceId: "TileUninstall"

[Code]
procedure CurStepChanged(CurStep: TSetupStep);
var
  Code: Integer;
begin
  if CurStep = ssPostInstall then
    if not Exec(ExpandConstant('{app}\tile.exe'), 'install', '', SW_HIDE, ewWaitUntilTerminated, Code)
       or (Code <> 0) then
      MsgBox('Tile could not be installed (exit code ' + IntToStr(Code) + '). Run "tile install" in a terminal to see why.', mbError, MB_OK);
end;
