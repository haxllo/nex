#define MyAppName "Nex"
#define MyAppId "{{E3A739E3-FAF7-4E18-BD8B-01744C9E7C27}"
#define MyAppUninstallKey "{E3A739E3-FAF7-4E18-BD8B-01744C9E7C27}_is1"

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
DefaultGroupName=Nex
OutputDir=artifacts\windows
OutputBaseFilename=nex-{#AppVersion}-windows-x64-setup
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
DefaultDirName={autopf}\Nex
DisableDirPage=yes
DisableProgramGroupPage=yes
; Avoid installer hangs in "automatically close applications" stage.
; Runtime shutdown is handled explicitly in [UninstallRun] during upgrade/uninstall.
CloseApplications=no
RestartApplications=no
UninstallDisplayIcon={app}\bin\nex.exe
SetupIconFile={#SetupIconPath}

[Files]
Source: "{#StageDir}\bin\nex.exe"; DestDir: "{app}\bin"; Flags: ignoreversion
Source: "{#StageDir}\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs

[InstallDelete]
Type: files; Name: "{app}\bin\nex-core.exe"
Type: files; Name: "{app}\bin\swiftfind-core.exe"

[Icons]
Name: "{autoprograms}\Nex"; Filename: "{app}\bin\nex.exe"; Parameters: "--background"
Name: "{autodesktop}\Nex"; Filename: "{app}\bin\nex.exe"; Parameters: "--background"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"
Name: "startuplaunch"; Description: "Launch at startup (can be changed later in config.toml)"; GroupDescription: "Startup:"

[Run]
Filename: "{app}\bin\nex.exe"; Parameters: "--ensure-config"; Flags: runhidden
Filename: "{app}\bin\nex.exe"; Parameters: "--set-launch-at-startup=true"; Flags: runhidden; Tasks: startuplaunch
Filename: "{app}\bin\nex.exe"; Parameters: "--set-launch-at-startup=false"; Flags: runhidden; Tasks: not startuplaunch
Filename: "{app}\bin\nex.exe"; Parameters: "--background"; Description: "Launch Nex now"; Flags: runhidden nowait postinstall skipifsilent

[UninstallRun]
; Ask running instance to terminate cleanly first.
Filename: "{app}\bin\nex.exe"; Parameters: "--quit"; Flags: runhidden nowait skipifdoesntexist; RunOnceId: "nex-quit-runtime"
Filename: "{app}\bin\nex-core.exe"; Parameters: "--quit"; Flags: runhidden nowait skipifdoesntexist; RunOnceId: "nex-quit-runtime-legacy"
Filename: "{app}\bin\swiftfind-core.exe"; Parameters: "--quit"; Flags: runhidden nowait skipifdoesntexist; RunOnceId: "nex-quit-runtime-swiftfind-legacy"
; Remove per-user startup registration even if config still had launch_at_startup=true.
Filename: "{cmd}"; Parameters: "/C reg delete HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v Nex /f >NUL 2>&1 || exit /b 0"; Flags: runhidden; RunOnceId: "nex-clear-startup"
Filename: "{cmd}"; Parameters: "/C reg delete HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v SwiftFind /f >NUL 2>&1 || exit /b 0"; Flags: runhidden; RunOnceId: "nex-clear-legacy-startup"
; Remove machine-wide startup registration when present (all-users installs).
Filename: "{cmd}"; Parameters: "/C reg delete HKLM\Software\Microsoft\Windows\CurrentVersion\Run /v Nex /f >NUL 2>&1 || exit /b 0"; Flags: runhidden; RunOnceId: "nex-clear-startup-machine"
Filename: "{cmd}"; Parameters: "/C reg delete HKLM\Software\Microsoft\Windows\CurrentVersion\Run /v SwiftFind /f >NUL 2>&1 || exit /b 0"; Flags: runhidden; RunOnceId: "nex-clear-legacy-startup-machine"

[Code]
const
  NexUninstallSubkey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#MyAppUninstallKey}';
  NexRuntimeRelativePath = 'bin\nex.exe';
  LegacyNexRuntimeRelativePath = 'bin\nex-core.exe';
  LegacySwiftFindRuntimeRelativePath = 'bin\swiftfind-core.exe';

procedure ForceStopRuntimeByPath(RuntimeExe: string); forward;

function StripWrappingQuotes(Value: string): string;
begin
  Result := Trim(Value);
  if (Length(Result) >= 2) and (Result[1] = '"') and (Result[Length(Result)] = '"') then
    Result := Copy(Result, 2, Length(Result) - 2);
end;

function ExtractCommandPath(Value: string): string;
var
  ClosingQuotePos: Integer;
  SpacePos: Integer;
begin
  Result := Trim(Value);
  if Result = '' then
    exit;

  if Result[1] = '"' then
  begin
    Delete(Result, 1, 1);
    ClosingQuotePos := Pos('"', Result);
    if ClosingQuotePos > 0 then
      Result := Copy(Result, 1, ClosingQuotePos - 1);
    exit;
  end;

  SpacePos := Pos(' ', Result);
  if SpacePos > 0 then
    Result := Copy(Result, 1, SpacePos - 1);
end;

function TryGetInstallLocation(RootKey: Integer; var InstallLocation: string): Boolean;
begin
  Result :=
    RegQueryStringValue(RootKey, NexUninstallSubkey, 'InstallLocation', InstallLocation) and
    (Trim(InstallLocation) <> '');
end;

function TryResolveExistingRuntimeRelativePath(InstallLocation: string; var RuntimeExe: string): Boolean;
begin
  RuntimeExe := AddBackslash(StripWrappingQuotes(InstallLocation)) + NexRuntimeRelativePath;
  if FileExists(RuntimeExe) then
  begin
    Result := true;
    exit;
  end;

  RuntimeExe := AddBackslash(StripWrappingQuotes(InstallLocation)) + LegacyNexRuntimeRelativePath;
  if FileExists(RuntimeExe) then
  begin
    Result := true;
    exit;
  end;

  RuntimeExe := AddBackslash(StripWrappingQuotes(InstallLocation)) + LegacySwiftFindRuntimeRelativePath;
  Result := FileExists(RuntimeExe);
  if not Result then
    RuntimeExe := '';
end;

function TryGetRegisteredRuntimeExe(RootKey: Integer; var RuntimeExe: string): Boolean;
var
  InstallLocation: string;
  DisplayIcon: string;
begin
  Result := false;

  if TryGetInstallLocation(RootKey, InstallLocation) then
  begin
    if TryResolveExistingRuntimeRelativePath(InstallLocation, RuntimeExe) then
    begin
      Result := true;
      exit;
    end;
  end;

  if RegQueryStringValue(RootKey, NexUninstallSubkey, 'DisplayIcon', DisplayIcon) then
  begin
    RuntimeExe := ExtractCommandPath(DisplayIcon);
    if FileExists(RuntimeExe) then
    begin
      Result := true;
      exit;
    end;
  end;

  RuntimeExe := '';
end;

function TryGetUninstallExe(RootKey: Integer; var UninstallExe: string): Boolean;
var
  UninstallString: string;
begin
  Result :=
    RegQueryStringValue(RootKey, NexUninstallSubkey, 'UninstallString', UninstallString) and
    (Trim(UninstallString) <> '');
  if not Result then
  begin
    UninstallExe := '';
    exit;
  end;

  UninstallExe := ExtractCommandPath(UninstallString);
  Result := FileExists(UninstallExe);
  if not Result then
    UninstallExe := '';
end;

function ScopeLabelForRootKey(RootKey: Integer): string;
begin
  if RootKey = HKLM then
    Result := 'all users'
  else
    Result := 'current user';
end;

procedure StopRuntimeByExecutable(RuntimeExe: string);
var
  ResultCode: Integer;
begin
  if FileExists(RuntimeExe) then
  begin
    if Exec(RuntimeExe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then
      Sleep(250);
  end;

  ForceStopRuntimeByPath(RuntimeExe);
  Sleep(250);
end;

function RemoveScopedInstall(RootKey: Integer; var ErrorMessage: string): Boolean;
var
  RuntimeExe: string;
  UninstallExe: string;
  ResultCode: Integer;
begin
  Result := false;
  ErrorMessage := '';

  if not TryGetRegisteredRuntimeExe(RootKey, RuntimeExe) then
  begin
    Result := true;
    exit;
  end;

  if not TryGetUninstallExe(RootKey, UninstallExe) then
  begin
    ErrorMessage :=
      ExpandConstant('{#MyAppName}') + ' is installed for ' + ScopeLabelForRootKey(RootKey) +
      ' at ' + RuntimeExe + ', but its uninstaller could not be located.';
    exit;
  end;

  StopRuntimeByExecutable(RuntimeExe);

  if not Exec(
    UninstallExe,
    '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART',
    '',
    SW_HIDE,
    ewWaitUntilTerminated,
    ResultCode
  ) then
  begin
    ErrorMessage :=
      'Failed to start the existing ' + ScopeLabelForRootKey(RootKey) +
      ' uninstaller: ' + UninstallExe;
    exit;
  end;

  if ResultCode <> 0 then
  begin
    ErrorMessage :=
      'The existing ' + ScopeLabelForRootKey(RootKey) +
      ' install could not be removed automatically (exit code ' + IntToStr(ResultCode) + ').';
    exit;
  end;

  Result := true;
end;

function PrepareOppositeScopeInstall(): string;
var
  OtherScopeRoot: Integer;
  RuntimeExe: string;
  ErrorMessage: string;
begin
  if IsAdminInstallMode then
    OtherScopeRoot := HKCU
  else
    OtherScopeRoot := HKLM;

  if not TryGetRegisteredRuntimeExe(OtherScopeRoot, RuntimeExe) then
  begin
    Result := '';
    exit;
  end;

  if (OtherScopeRoot = HKLM) and not IsAdminInstallMode then
  begin
    Result :=
      ExpandConstant('{#MyAppName}') + ' is already installed for all users.' + #13#10 + #13#10 +
      'Existing install: ' + RuntimeExe + #13#10 + #13#10 +
      'To replace it, rerun setup and choose All users, or uninstall the all-users copy first.';
    exit;
  end;

  if not RemoveScopedInstall(OtherScopeRoot, ErrorMessage) then
  begin
    Result := ErrorMessage;
    exit;
  end;

  Result :=
    '';
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

  EscapedRuntimeExe := RuntimeExe;
  StringChangeEx(EscapedRuntimeExe, '''', '''''', True);
  Command :=
    '-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -Command ' +
    '"Get-CimInstance Win32_Process ' +
    '| Where-Object { $_.ExecutablePath -eq ''' + EscapedRuntimeExe + ''' } ' +
    '| ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }"';

  Exec(PowerShellExe, Command, '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

procedure StopNexRuntime();
begin
  StopRuntimeByExecutable(ExpandConstant('{app}\bin\nex.exe'));
  StopRuntimeByExecutable(ExpandConstant('{app}\bin\nex-core.exe'));
  StopRuntimeByExecutable(ExpandConstant('{app}\bin\swiftfind-core.exe'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    StopNexRuntime();
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := PrepareOppositeScopeInstall();
  if Result <> '' then
    exit;

  StopNexRuntime();
end;
