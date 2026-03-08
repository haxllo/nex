#define MyAppName "SwiftFind"
#define MyAppId "{{E3A739E3-FAF7-4E18-BD8B-01744C9E7C27}"

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
AppId={#MyAppId}
AppName={#MyAppName}
AppVersion={#AppVersion}
AppVerName={#MyAppName}
UninstallDisplayName={#MyAppName}
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
; Always show install scope choice instead of silently reusing previous mode.
UsePreviousPrivileges=no
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

[Code]
const
  SwiftFindUninstallSubkey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#MyAppId}_is1';
  SwiftFindRuntimeRelativePath = 'bin\swiftfind-core.exe';

function StripWrappingQuotes(Value: string): string;
begin
  Result := Trim(Value);
  if (Length(Result) >= 2) and (Result[1] = '"') and (Result[Length(Result)] = '"') then
    Result := Copy(Result, 2, Length(Result) - 2);
end;

function StripDisplayIconSuffix(Value: string): string;
var
  SuffixPos: Integer;
begin
  Result := StripWrappingQuotes(Value);
  SuffixPos := Pos(',', Result);
  if SuffixPos > 0 then
    Result := Trim(Copy(Result, 1, SuffixPos - 1));
end;

function TryGetInstallLocation(RootKey: Integer; var InstallLocation: string): Boolean;
begin
  Result :=
    RegQueryStringValue(RootKey, SwiftFindUninstallSubkey, 'InstallLocation', InstallLocation) and
    (Trim(InstallLocation) <> '');
end;

function TryGetRegisteredRuntimeExe(RootKey: Integer; var RuntimeExe: string): Boolean;
var
  InstallLocation: string;
  DisplayIcon: string;
begin
  Result := false;

  if TryGetInstallLocation(RootKey, InstallLocation) then
  begin
    RuntimeExe := AddBackslash(StripWrappingQuotes(InstallLocation)) + SwiftFindRuntimeRelativePath;
    if FileExists(RuntimeExe) then
    begin
      Result := true;
      exit;
    end;
  end;

  if RegQueryStringValue(RootKey, SwiftFindUninstallSubkey, 'DisplayIcon', DisplayIcon) then
  begin
    RuntimeExe := StripDisplayIconSuffix(DisplayIcon);
    if FileExists(RuntimeExe) then
    begin
      Result := true;
      exit;
    end;
  end;

  RuntimeExe := '';
end;

function OppositeScopeInstallError(): string;
var
  CurrentScopeLabel: string;
  OtherScopeLabel: string;
  OtherScopeRoot: Integer;
  InstallLocation: string;
begin
  if IsAdminInstallMode then
  begin
    CurrentScopeLabel := 'all users';
    OtherScopeLabel := 'current user';
    OtherScopeRoot := HKCU;
  end
  else
  begin
    CurrentScopeLabel := 'current user';
    OtherScopeLabel := 'all users';
    OtherScopeRoot := HKLM;
  end;

  if not TryGetRegisteredRuntimeExe(OtherScopeRoot, InstallLocation) then
  begin
    Result := '';
    exit;
  end;

  Result :=
    ExpandConstant('{#MyAppName}') + ' is already installed for ' + OtherScopeLabel + '.' + #13#10 + #13#10 +
    'Existing install: ' + InstallLocation + #13#10 + #13#10 +
    'This installer is currently set to install for ' + CurrentScopeLabel + '.' + #13#10 +
    'Uninstall the existing ' + OtherScopeLabel + ' copy first, or rerun setup and choose the same scope.';
end;

procedure ForceStopRuntimeByPath(RuntimeExe: string);
var
  ResultCode: Integer;
  PowerShellExe: string;
  EscapedRuntimeExe: string;
  Command: string;
begin
  if not FileExists(RuntimeExe) then
    exit;

  PowerShellExe := ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe');
  if not FileExists(PowerShellExe) then
    exit;

  EscapedRuntimeExe := StringChangeEx(RuntimeExe, '''', '''''', True);
  Command :=
    '-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -Command ' +
    '"Get-CimInstance Win32_Process -Filter ""Name = ''swiftfind-core.exe''"" ' +
    '| Where-Object { $_.ExecutablePath -eq ''' + EscapedRuntimeExe + ''' } ' +
    '| ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }"';

  Exec(PowerShellExe, Command, '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

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

  ForceStopRuntimeByPath(RuntimeExe);
  Sleep(250);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    StopSwiftFindRuntime();
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := OppositeScopeInstallError();
  if Result <> '' then
    exit;

  StopSwiftFindRuntime();
end;
