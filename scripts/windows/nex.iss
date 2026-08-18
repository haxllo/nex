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
PrivilegesRequiredOverridesAllowed=commandline
; Always show install scope choice instead of silently reusing previous mode.
UsePreviousPrivileges=no
DefaultDirName={autopf}\Nex
DisableDirPage=yes
DisableProgramGroupPage=yes
; Avoid installer hangs in "automatically close applications" stage.
; Runtime shutdown is handled explicitly in [UninstallRun] during upgrade/uninstall.
CloseApplications=no
RestartApplications=no
UninstallDisplayIcon={app}\bin\Nex.exe
SetupIconFile={#SetupIconPath}

[Files]
Source: "{#StageDir}\bin\Nex.exe"; DestDir: "{app}\bin"; Flags: ignoreversion
Source: "{#StageDir}\bin\NexHelper.exe"; DestDir: "{app}\bin"; Flags: ignoreversion
Source: "{#StageDir}\bin\Everything64.dll"; DestDir: "{app}\bin"; Flags: ignoreversion
Source: "{#StageDir}\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#StageDir}\scripts\update-nex.ps1"; DestDir: "{app}\scripts"; Flags: ignoreversion

[InstallDelete]
Type: files; Name: "{app}\bin\nex-core.exe"
Type: files; Name: "{app}\bin\swiftfind-core.exe"

[Icons]
Name: "{autoprograms}\Nex"; Filename: "{app}\bin\Nex.exe"; Parameters: "--background"
Name: "{autodesktop}\Nex"; Filename: "{app}\bin\Nex.exe"; Parameters: "--background"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"
Name: "startuplaunch"; Description: "Launch at startup (can be changed later in config.toml)"; GroupDescription: "Startup:"

[Registry]
; Startup registration — no process spawn needed.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Nex"; ValueData: "{app}\bin\Nex.exe --background"; Flags: uninsdeletevalue; Tasks: startuplaunch
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Nex"; Flags: deletevalue; Tasks: not startuplaunch

[Run]
; Launch Nex after install (no config/setup processes needed — nex handles those on first run).
Filename: "{app}\bin\Nex.exe"; Parameters: "--background"; Description: "Launch Nex now"; Flags: runhidden nowait postinstall skipifsilent

[UninstallRun]
; Ask running instance to terminate cleanly first.
Filename: "{app}\bin\Nex.exe"; Parameters: "--quit"; Flags: runhidden nowait skipifdoesntexist; RunOnceId: "nex-quit-runtime"
Filename: "{app}\bin\nex-core.exe"; Parameters: "--quit"; Flags: runhidden nowait skipifdoesntexist; RunOnceId: "nex-quit-runtime-legacy"
Filename: "{app}\bin\swiftfind-core.exe"; Parameters: "--quit"; Flags: runhidden nowait skipifdoesntexist; RunOnceId: "nex-quit-runtime-swiftfind-legacy"
; Remove per-user startup registration even if config still had launch_at_startup=true.
Filename: "{cmd}"; Parameters: "/C reg delete HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v Nex /f >NUL 2>&1 || exit /b 0"; Flags: runhidden; RunOnceId: "nex-clear-startup"
Filename: "{cmd}"; Parameters: "/C reg delete HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v SwiftFind /f >NUL 2>&1 || exit /b 0"; Flags: runhidden; RunOnceId: "nex-clear-legacy-startup"
; Remove the elevated helper scheduled task.
Filename: "{cmd}"; Parameters: "/C schtasks /delete /tn NexHelperV2 /f >NUL 2>&1 || exit /b 0"; Flags: runhidden; RunOnceId: "nex-remove-helper-task"
; Remove machine-wide startup registration when present (all-users installs).
Filename: "{cmd}"; Parameters: "/C reg delete HKLM\Software\Microsoft\Windows\CurrentVersion\Run /v Nex /f >NUL 2>&1 || exit /b 0"; Flags: runhidden; RunOnceId: "nex-clear-startup-machine"
Filename: "{cmd}"; Parameters: "/C reg delete HKLM\Software\Microsoft\Windows\CurrentVersion\Run /v SwiftFind /f >NUL 2>&1 || exit /b 0"; Flags: runhidden; RunOnceId: "nex-clear-legacy-startup-machine"

[Code]
const
  NexUninstallSubkey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#MyAppUninstallKey}';
  NexRuntimeRelativePath = 'bin\Nex.exe';
  LegacyNexRuntimeRelativePath = 'bin\nex-core.exe';
  LegacySwiftFindRuntimeRelativePath = 'bin\swiftfind-core.exe';

var
  DeleteDataCheckbox: TNewCheckBox;

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
  TaskKill: string;
  ImageName: string;
begin
  if not FileExists(RuntimeExe) then
    exit;

  TaskKill := ExpandConstant('{sys}\taskkill.exe');
  if not FileExists(TaskKill) then
    exit;

  ImageName := ExtractFileName(RuntimeExe);
  Exec(TaskKill, '/IM "' + ImageName + '" /F /T', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

function IsNexInstalled(): Boolean;
var
  RuntimeExe: string;
begin
  Result := TryGetRegisteredRuntimeExe(HKCU, RuntimeExe) or
            TryGetRegisteredRuntimeExe(HKLM, RuntimeExe);
end;

procedure StopNexRuntime();
begin
  StopRuntimeByExecutable(ExpandConstant('{app}\bin\Nex.exe'));
  StopRuntimeByExecutable(ExpandConstant('{app}\bin\NexHelper.exe'));
  StopRuntimeByExecutable(ExpandConstant('{app}\bin\nex-core.exe'));
  StopRuntimeByExecutable(ExpandConstant('{app}\bin\swiftfind-core.exe'));
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  // All-users installs run elevated, so pre-create the elevated helper
  // task here — nex then never needs a UAC prompt on first run. Mirrors
  // the task nex builds itself (past one-shot trigger + on-demand start,
  // HighestAvailable run level). Current-user installs are not elevated,
  // so they keep nex's first-run creation (prompt at the first hotkey).
  if (CurStep = ssPostInstall) and IsAdminInstallMode then
    Exec(
      ExpandConstant('{sys}\schtasks.exe'),
      '/create /tn NexHelperV2 /tr ""{app}\bin\NexHelper.exe"" --config ""{userappdata}\Nex\helper-config.json"" /sc once /st 23:59 /rl HIGHEST /f',
      '',
      SW_HIDE,
      ewWaitUntilTerminated,
      ResultCode
    );
end;

procedure InitializeUninstallWizard();
begin
  // Opt-in checkbox on the uninstall welcome page — user data is
  // preserved by default.
  DeleteDataCheckbox := TNewCheckBox.Create(WizardForm);
  DeleteDataCheckbox.Parent := WizardForm.WelcomePage;
  DeleteDataCheckbox.Left := ScaleX(48);
  DeleteDataCheckbox.Top := ScaleY(196);
  DeleteDataCheckbox.Width := WizardForm.WelcomePage.ClientWidth - ScaleX(96);
  DeleteDataCheckbox.Height := ScaleY(17);
  DeleteDataCheckbox.Caption := 'Also delete my settings and search index';
  DeleteDataCheckbox.Checked := false;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    StopNexRuntime()
  else if (CurUninstallStep = usPostUninstall) and DeleteDataCheckbox.Checked then
  begin
    // Remove per-user data only when the user opted in. The runtime was
    // stopped during usUninstall and [UninstallRun] already ran, so
    // nothing holds these files open. {userappdata} resolves to the
    // account running the uninstaller.
    DelTree(ExpandConstant('{userappdata}\Nex'), True, True, True);
    DelTree(ExpandConstant('{userappdata}\SwiftFind'), True, True, True);
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := PrepareOppositeScopeInstall();
  if Result <> '' then
    exit;

  // Only kill running nex if there's an existing installation to replace.
  // On fresh install there's nothing to stop — the --quit and PowerShell
  // fallback just waste time and flash windows.
  if IsNexInstalled() then
    StopNexRuntime();
end;
