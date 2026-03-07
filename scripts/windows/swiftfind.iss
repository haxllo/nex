#define MyAppName "SwiftFind"

#ifndef AppVersion
  #define AppVersion "0.0.0-local"
#endif

#ifndef StageDir
  #error StageDir must be passed to ISCC via /DStageDir=...
#endif

#ifndef SetupIconPath
  #error SetupIconPath must be passed to ISCC via /DSetupIconPath=...
#endif

[Setup]
AppId={{E3A739E3-FAF7-4E18-BD8B-01744C9E7C27}
AppName={#MyAppName}
AppVersion={#AppVersion}
DefaultGroupName=SwiftFind
OutputDir=artifacts\windows
OutputBaseFilename=swiftfind-{#AppVersion}-windows-x64-setup
Compression=lzma
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
PrivilegesRequired=lowest
; Allow installer scope selection:
; - Current user (default, no elevation)
; - All users (elevates and uses common locations)
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={autopf}\SwiftFind
DisableDirPage=yes
DisableProgramGroupPage=yes
; Avoid installer hangs in "automatically close applications" stage.
; Runtime shutdown is handled explicitly in [UninstallRun] during upgrade/uninstall.
CloseApplications=no
RestartApplications=no
UninstallDisplayIcon={app}\bin\swiftfind-core.exe
SetupIconFile={#SetupIconPath}

[Files]
Source: "{#StageDir}\bin\swiftfind-core.exe"; DestDir: "{app}\bin"; Flags: ignoreversion
Source: "{#StageDir}\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#StageDir}\docs\*"; DestDir: "{app}\docs"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#StageDir}\scripts\*"; DestDir: "{app}\scripts"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\SwiftFind"; Filename: "{app}\bin\swiftfind-core.exe"; Parameters: "--background"
Name: "{autodesktop}\SwiftFind"; Filename: "{app}\bin\swiftfind-core.exe"; Parameters: "--background"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"
Name: "startuplaunch"; Description: "Launch at startup (can be changed later in config.toml)"; GroupDescription: "Startup:"

[Run]
Filename: "{app}\bin\swiftfind-core.exe"; Parameters: "--ensure-config"; Flags: runhidden
Filename: "{app}\bin\swiftfind-core.exe"; Parameters: "--set-launch-at-startup=true"; Flags: runhidden; Tasks: startuplaunch
Filename: "{app}\bin\swiftfind-core.exe"; Parameters: "--set-launch-at-startup=false"; Flags: runhidden; Tasks: not startuplaunch
Filename: "{app}\bin\swiftfind-core.exe"; Parameters: "--background"; Description: "Launch SwiftFind now"; Flags: runhidden nowait postinstall skipifsilent

[UninstallRun]
; Ask running instance to terminate cleanly first.
Filename: "{app}\bin\swiftfind-core.exe"; Parameters: "--quit"; Flags: runhidden nowait skipifdoesntexist; RunOnceId: "swiftfind-quit-runtime"
; Remove per-user startup registration even if config still had launch_at_startup=true.
Filename: "{cmd}"; Parameters: "/C reg delete HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v SwiftFind /f >NUL 2>&1 || exit /b 0"; Flags: runhidden; RunOnceId: "swiftfind-clear-startup"
; Remove machine-wide startup registration when present (all-users installs).
Filename: "{cmd}"; Parameters: "/C reg delete HKLM\Software\Microsoft\Windows\CurrentVersion\Run /v SwiftFind /f >NUL 2>&1 || exit /b 0"; Flags: runhidden; RunOnceId: "swiftfind-clear-startup-machine"
; Hard-stop any leftover process to avoid ghost hotkey/runtime after uninstall.
Filename: "{cmd}"; Parameters: "/C taskkill /IM swiftfind-core.exe /F /T >NUL 2>&1 || exit /b 0"; Flags: runhidden; RunOnceId: "swiftfind-kill-runtime"

[Code]
procedure StopSwiftFindRuntime();
var
  ResultCode: Integer;
  RuntimeExe: string;
begin
  RuntimeExe := ExpandConstant('{app}\bin\swiftfind-core.exe');
  if FileExists(RuntimeExe) then
  begin
    if Exec(RuntimeExe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then
      Sleep(250);
  end;

  Exec(
    ExpandConstant('{cmd}'),
    '/C taskkill /IM swiftfind-core.exe /F /T >NUL 2>&1',
    '',
    SW_HIDE,
    ewWaitUntilTerminated,
    ResultCode
  );
  Sleep(250);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopSwiftFindRuntime();
  Result := '';
end;
