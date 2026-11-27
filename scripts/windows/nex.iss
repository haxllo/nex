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

; The wizard background: a light and a dark image behind the page content, drawn
; at 497:360 and scaled by Setup. Regenerate them with
; scripts/windows/make-installer-backgrounds.ps1.
#ifndef WizardBackLightPath
  #define WizardBackLightPath "..\..\apps\assets\installer\back-light.png"
#endif

#ifndef WizardBackDarkPath
  #define WizardBackDarkPath "..\..\apps\assets\installer\back-dark.png"
#endif

[Setup]
AppId={#MyAppId}
AppName={#MyAppName}
AppVersion={#AppVersion}
AppVerName={#MyAppName} {#AppVersion}
AppPublisher=haxllo
AppPublisherURL=https://github.com/haxllo/nex
AppSupportURL=https://github.com/haxllo/nex/issues
AppUpdatesURL=https://github.com/haxllo/nex/releases
AppCopyright=Copyright (C) 2026 haxllo
VersionInfoCompany=haxllo
VersionInfoDescription=Nex Setup
UninstallDisplayName={#MyAppName}
DefaultGroupName=Nex
OutputDir=artifacts\windows
OutputBaseFilename=nex-{#AppVersion}-windows-x64-setup
Compression=lzma
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern dynamic windows11 includetitlebar hidebevels
; Back images are the only artwork: the stock large and small wizard images are
; switched off, so the page content sits on the tinted background instead of
; beside a generic bitmap. dynamic keeps the pair following Windows' own theme;
; a forced dark wizard on a light desktop is a worse first impression than none.
WizardBackImageFile={#WizardBackLightPath}
WizardBackImageFileDynamicDark={#WizardBackDarkPath}
WizardImageFile=
WizardSmallImageFile=
; Two pages, both of them choices: what gets set up, then the install itself.
; The licence page is gone (MIT ships in the zip and in the app), and the welcome
; and ready pages only repeat what the task list and the progress bar already say.
DisableWelcomePage=yes
DisableReadyPage=yes
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
UninstallDisplayIcon={app}\bin\NexUIProcess.exe
SetupIconFile={#SetupIconPath}

[Messages]
; The words the wizard says. Everything here replaces Inno's stock phrasing, so
; the pages read as Nex rather than as a generic setup program.
SetupAppTitle=Nex Setup
SetupWindowTitle=Nex Setup
SetupLdrStartupMessage=Starting Nex Setup...
WizardSelectTasks=Choose what Nex sets up
SelectTasksDesc=Nex searches your files, folders and apps from one panel. Press Win to open it.
SelectTasksLabel2=Both are optional and can be changed later.
FinishedHeadingLabel=Nex is installed
FinishedLabel=Nex runs in the background. Press Win to search everything on this PC.
ClickFinish=Finish

[Files]
Source: "{#StageDir}\bin\NexUIProcess.exe"; DestDir: "{app}\bin"; Flags: ignoreversion
Source: "{#StageDir}\bin\NexIndexerProcess.exe"; DestDir: "{app}\bin"; Flags: ignoreversion
Source: "{#StageDir}\bin\NexHelper.exe"; DestDir: "{app}\bin"; Flags: ignoreversion
Source: "{#StageDir}\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs

[InstallDelete]
; Legacy binaries from older releases: never shipped now, always removed.
; bin\Nex.exe is the 2.38.0-2.39.0 updater shim left by earlier updates.
Type: files; Name: "{app}\bin\Nex.exe"
Type: files; Name: "{app}\bin\nex-core.exe"
Type: files; Name: "{app}\bin\swiftfind-core.exe"
Type: files; Name: "{app}\bin\Everything64.dll"
; Legacy-orphan removal lives in [Code] RemoveLegacyOrphan (called from
; CurStepChanged) so the hybrid-key check provably runs before deletion.

[Icons]
Name: "{autoprograms}\Nex"; Filename: "{app}\bin\NexUIProcess.exe"; Parameters: "--background"
Name: "{autodesktop}\Nex"; Filename: "{app}\bin\NexUIProcess.exe"; Parameters: "--background"; Tasks: desktopicon

[Tasks]
; Flat list, no group headers: two choices do not need headings.
Name: "desktopicon"; Description: "Desktop shortcut"
Name: "startuplaunch"; Description: "Start Nex when Windows starts"

[Registry]
; Startup registration — no process spawn needed.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Nex"; ValueData: "{app}\bin\NexUIProcess.exe --background"; Flags: uninsdeletevalue; Tasks: startuplaunch
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Nex"; Flags: deletevalue; Tasks: not startuplaunch

[UninstallRun]
; Ask running instance to terminate cleanly first.
Filename: "{app}\bin\NexUIProcess.exe"; Parameters: "--quit"; Flags: runhidden nowait skipifdoesntexist; RunOnceId: "nex-quit-runtime"
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
function ShowWindow(hWnd: HWND; nCmdShow: Integer): Boolean;
  external 'ShowWindow@user32.dll stdcall';
function SetForegroundWindow(hWnd: HWND): Boolean;
  external 'SetForegroundWindow@user32.dll stdcall';

const
  NexUninstallSubkey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#MyAppUninstallKey}';
  NexRuntimeRelativePath = 'bin\NexUIProcess.exe';
  LegacyNexRuntimeRelativePath = 'bin\nex-core.exe';
  LegacySwiftFindRuntimeRelativePath = 'bin\swiftfind-core.exe';

var
  DeleteDataCheckbox: TNewCheckBox;
  DeleteUserDataAfterUninstall: Boolean;

procedure ActivateInstallerWindow();
begin
  ShowWindow(WizardForm.Handle, 9);
  SetForegroundWindow(WizardForm.Handle);
end;

function ShouldLaunchNexAfterInstall(): Boolean;
begin
  Result := Pos('/NEXUPDATER', Uppercase(GetCmdTail())) = 0;
end;

procedure RemoveLegacyOrphan(); forward;

var
  WizardTextColor, WizardMutedColor: TColor;
  ScopeNote: TNewStaticText;
  FinishedBody: TNewStaticText;
  LaunchCheck: TNewCheckBox;
  LaunchAfterInstall: Boolean;

function SystemUsesLightTheme(): Boolean;
var
  value: Cardinal;
begin
  Result := True;
  if RegQueryDWordValue(HKCU, 'Software' + Chr(92) + 'Microsoft' + Chr(92) + 'Windows' + Chr(92) + 'CurrentVersion' + Chr(92) + 'Themes' + Chr(92) + 'Personalize', 'AppsUseLightTheme', value) then
    Result := value <> 0;
end;

// One left edge for every piece of text on a page, a heavier title, muted body
// copy, and colours taken from the same light/dark choice Setup already made.
// Without this the stock wizard indents the title less than the description
// under it and sets both in the dialog font at one size.
procedure ApplyModernLook();
var
  light: Boolean;
begin
  light := SystemUsesLightTheme();
  if light then
  begin
    WizardTextColor := $1E1E20;
    WizardMutedColor := $64646A;
  end
  else
  begin
    WizardTextColor := $F2F2F4;
    WizardMutedColor := $A6A6AC;
  end;

  WizardForm.PageNameLabel.Left := ScaleX(40);
  WizardForm.PageNameLabel.Top := ScaleY(8);
  WizardForm.PageNameLabel.Width := ScaleX(484);
  WizardForm.PageNameLabel.Height := ScaleY(22);
  WizardForm.PageNameLabel.Font.Name := 'Segoe UI Semibold';
  WizardForm.PageNameLabel.Font.Size := 12;
  WizardForm.PageNameLabel.Font.Color := WizardTextColor;

  WizardForm.PageDescriptionLabel.Left := ScaleX(40);
  WizardForm.PageDescriptionLabel.Top := ScaleY(31);
  WizardForm.PageDescriptionLabel.Width := ScaleX(484);
  WizardForm.PageDescriptionLabel.Font.Name := 'Segoe UI';
  WizardForm.PageDescriptionLabel.Font.Size := 9;
  WizardForm.PageDescriptionLabel.Font.Color := WizardMutedColor;

  // All wizard artwork is the back image, so the empty small/left bitmap frames
  // are hidden instead of standing in the corner as blank boxes.
  WizardForm.WizardSmallBitmapImage.Visible := False;

  // The stock label on the choices page repeated the header, so it goes and the
  // list takes the surface at a size that matches the body copy.
  WizardForm.SelectTasksLabel.Visible := False;
  WizardForm.TasksList.Left := 0;
  WizardForm.TasksList.Top := 0;
  WizardForm.TasksList.Height := ScaleY(300);
  WizardForm.TasksList.Font.Name := 'Segoe UI';
  WizardForm.TasksList.Font.Size := 10;

  ScopeNote := TNewStaticText.Create(WizardForm);
  ScopeNote.Parent := WizardForm.SelectTasksPage;
  ScopeNote.Left := 0;
  ScopeNote.Top := ScaleY(64);
  ScopeNote.Width := ScaleX(556);
  ScopeNote.Caption := 'Installs for this Windows account, with no admin prompt.';
  ScopeNote.Font.Name := 'Segoe UI';
  ScopeNote.Font.Size := 8;
  ScopeNote.Font.Color := WizardMutedColor;

  WizardForm.FilenameLabel.Visible := False;
  WizardForm.StatusLabel.Font.Name := 'Segoe UI';
  WizardForm.StatusLabel.Font.Size := 10;
  WizardForm.StatusLabel.Font.Color := WizardTextColor;
  WizardForm.ProgressGauge.Top := ScaleY(32);

  WizardForm.WizardBitmapImage2.Visible := False;
  WizardForm.FinishedHeadingLabel.Left := ScaleX(40);
  WizardForm.FinishedHeadingLabel.Top := ScaleY(16);
  WizardForm.FinishedHeadingLabel.Width := ScaleX(528);
  WizardForm.FinishedHeadingLabel.AutoSize := False;
  WizardForm.FinishedHeadingLabel.WordWrap := False;
  WizardForm.FinishedHeadingLabel.Font.Name := 'Segoe UI Semibold';
  WizardForm.FinishedHeadingLabel.Font.Size := 15;
  WizardForm.FinishedHeadingLabel.Font.Color := WizardTextColor;
  FinishedBody := TNewStaticText.Create(WizardForm);
  FinishedBody.Parent := WizardForm.FinishedPage;
  FinishedBody.Left := ScaleX(40);
  FinishedBody.Top := ScaleY(58);
  FinishedBody.Width := ScaleX(520);
  FinishedBody.Height := ScaleY(120);
  FinishedBody.AutoSize := False;
  FinishedBody.WordWrap := True;
  FinishedBody.Caption := 'Nex runs in the background. Press Win to search everything on this PC.';
  FinishedBody.Font.Name := 'Segoe UI';
  FinishedBody.Font.Size := 10;
  FinishedBody.Font.Color := WizardMutedColor;
  FinishedBody.Visible := False;

  LaunchAfterInstall := True;
  LaunchCheck := TNewCheckBox.Create(WizardForm);
  LaunchCheck.Parent := WizardForm.FinishedPage;
  LaunchCheck.Left := ScaleX(40);
  LaunchCheck.Top := ScaleY(96);
  LaunchCheck.Width := ScaleX(300);
  LaunchCheck.Caption := 'Launch Nex now';
  LaunchCheck.Checked := True;
  LaunchCheck.Font.Name := 'Segoe UI';
  LaunchCheck.Font.Size := 10;
  LaunchCheck.Font.Color := WizardTextColor;
  LaunchCheck.Visible := False;

  WizardForm.FinishedLabel.Left := ScaleX(40);
  WizardForm.FinishedLabel.Top := ScaleY(54);
  WizardForm.FinishedLabel.Width := ScaleX(528);
  WizardForm.FinishedLabel.AutoSize := False;
  WizardForm.FinishedLabel.WordWrap := True;
  WizardForm.FinishedLabel.Height := ScaleY(150);
  WizardForm.FinishedLabel.Font.Name := 'Segoe UI';
  WizardForm.FinishedLabel.Font.Size := 10;
  WizardForm.FinishedLabel.Font.Color := WizardMutedColor;
end;

procedure InitializeWizard();
begin
  ActivateInstallerWindow();
  ApplyModernLook();
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  // The choice is read here because this is the last moment the user can make it.
  if CurPageID = wpFinished then
    LaunchAfterInstall := LaunchCheck.Checked;
end;

procedure DeinitializeSetup();
var
  ResultCode: Integer;
begin
  // Read the box at the last moment as well, so closing the wizard with the X
  // honours an unticked box instead of falling back to the default.
  if LaunchCheck <> nil then
    LaunchAfterInstall := LaunchCheck.Checked;
  if WizardSilent or (not LaunchAfterInstall) then
    exit;
  Exec(ExpandConstant('{app}\bin\NexUIProcess.exe'), '--background', '', SW_HIDE, ewNoWait, ResultCode);
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  ActivateInstallerWindow();
  // The wizard's own wording for the last button is "Next" on the only page that
  // asks anything and "Finish" at the end. Both say what happens instead.
  if CurPageID <> wpFinished then
  begin
    FinishedBody.Visible := False;
    LaunchCheck.Visible := False;
  end;

  LaunchAfterInstall := True;
  LaunchCheck := TNewCheckBox.Create(WizardForm);
  LaunchCheck.Parent := WizardForm.FinishedPage;
  LaunchCheck.Left := ScaleX(40);
  LaunchCheck.Top := ScaleY(96);
  LaunchCheck.Width := ScaleX(300);
  LaunchCheck.Caption := 'Launch Nex now';
  LaunchCheck.Checked := True;
  LaunchCheck.Font.Name := 'Segoe UI';
  LaunchCheck.Font.Size := 10;
  LaunchCheck.Font.Color := WizardTextColor;
  LaunchCheck.Visible := False;

  if CurPageID = wpSelectTasks then
  begin
    WizardForm.NextButton.Caption := 'Install';
    WizardForm.ActiveControl := WizardForm.NextButton;
  end
  else if CurPageID = wpFinished then
  begin
    WizardForm.NextButton.Caption := 'Done';
    // The finished page recomputes its own layout when it is shown, which puts the
    // body label back to one clipped line, so the geometry is applied here.
    WizardForm.WizardBitmapImage2.Visible := False;
    WizardForm.FinishedHeadingLabel.Left := ScaleX(40);
    WizardForm.FinishedHeadingLabel.Top := ScaleY(16);
    WizardForm.FinishedHeadingLabel.Width := ScaleX(528);
    WizardForm.FinishedHeadingLabel.AutoSize := False;
    WizardForm.FinishedHeadingLabel.WordWrap := False;
    FinishedBody := TNewStaticText.Create(WizardForm);
  FinishedBody.Parent := WizardForm.FinishedPage;
  FinishedBody.Left := ScaleX(40);
  FinishedBody.Top := ScaleY(58);
  FinishedBody.Width := ScaleX(520);
  FinishedBody.Height := ScaleY(120);
  FinishedBody.AutoSize := False;
  FinishedBody.WordWrap := True;
  FinishedBody.Caption := 'Nex runs in the background. Press Win to search everything on this PC.';
  FinishedBody.Font.Name := 'Segoe UI';
  FinishedBody.Font.Size := 10;
  FinishedBody.Font.Color := WizardMutedColor;
  FinishedBody.Visible := False;

  LaunchAfterInstall := True;
  LaunchCheck := TNewCheckBox.Create(WizardForm);
  LaunchCheck.Parent := WizardForm.FinishedPage;
  LaunchCheck.Left := ScaleX(40);
  LaunchCheck.Top := ScaleY(96);
  LaunchCheck.Width := ScaleX(300);
  LaunchCheck.Caption := 'Launch Nex now';
  LaunchCheck.Checked := True;
  LaunchCheck.Font.Name := 'Segoe UI';
  LaunchCheck.Font.Size := 10;
  LaunchCheck.Font.Color := WizardTextColor;
  LaunchCheck.Visible := False;

  WizardForm.FinishedLabel.Left := ScaleX(40);
    WizardForm.FinishedLabel.Top := ScaleY(54);
    WizardForm.FinishedLabel.Width := ScaleX(528);
    WizardForm.FinishedLabel.Height := ScaleY(150);
    WizardForm.FinishedLabel.AutoSize := False;
    WizardForm.FinishedLabel.WordWrap := True;
    // Not hidden: the finished page drops the "launch now" checkbox when this
    // label is invisible, so it is emptied instead and the body above carries
    // the sentence.
    WizardForm.FinishedLabel.Caption := '';
    FinishedBody.Visible := True;
    FinishedBody.BringToFront;
    LaunchCheck.Top := ScaleY(96);
    LaunchCheck.Checked := True;
    LaunchCheck.Visible := True;
    LaunchCheck.BringToFront;
  end;
end;

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

function TrimTrailingBackslash(Value: string): string;
begin
  Result := Trim(Value);
  while (Length(Result) > 0) and ((Result[Length(Result)] = '\') or (Result[Length(Result)] = '/')) do
    Result := Copy(Result, 1, Length(Result) - 1);
end;

function SameInstallDir(A, B: string): Boolean;
begin
  Result := CompareText(TrimTrailingBackslash(A), TrimTrailingBackslash(B)) = 0;
end;

function IsUnderDir(PathValue, DirValue: string): Boolean;
var
  P, D: string;
begin
  P := TrimTrailingBackslash(PathValue);
  D := TrimTrailingBackslash(DirValue);
  Result :=
    (D <> '') and (Length(P) >= Length(D)) and
    (CompareText(Copy(P, 1, Length(D)), D) = 0) and
    ((Length(P) = Length(D)) or (P[Length(D) + 1] = '\') or (P[Length(D) + 1] = '/'));
end;

function IsUnderProgramFiles(Value: string): Boolean;
begin
  Result :=
    IsUnderDir(Value, ExpandConstant('{commonpf}')) or
    IsUnderDir(Value, ExpandConstant('{commonpf32}')) or
    IsUnderDir(Value, GetEnv('ProgramW6432'));
end;

function TryCleanStaleHybridMachineEntry(): Boolean;
var
  Location: string;
begin
  Result := False;
  if not TryGetInstallLocation(HKLM, Location) then
    exit;
  Location := StripWrappingQuotes(Trim(Location));
  { Stale hybrid = machine-wide entry pointing at this per-user install,
    or anywhere outside Program Files (legit all-users installs always
    live under Program Files). Genuine all-users installs return False
    so the caller keeps the hard error. }
  if (not SameInstallDir(Location, ExpandConstant('{app}'))) and IsUnderProgramFiles(Location) then
    exit;
  Log('Removing stale hybrid machine-wide entry pointing at: ' + Location);
  if RegDeleteKeyIncludingSubkeys(HKLM, NexUninstallSubkey) then
  begin
    Log('Removed stale hybrid machine-wide uninstall entry (pre-install)');
    Result := True;
  end
  else
    Log('WARNING: could not remove stale hybrid machine-wide entry (needs elevation?)');
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

  { Location-based stale-hybrid cleanup BEFORE file gate. Updater moves tree
    to backup before setup, so FileExists false on updater runs. }
  if (OtherScopeRoot = HKLM) and (not IsAdminInstallMode) then
  begin
    if TryCleanStaleHybridMachineEntry() then
    begin
      Result := '';
      exit;
    end;
  end;

  if not TryGetRegisteredRuntimeExe(OtherScopeRoot, RuntimeExe) then
  begin
    Result := '';
    exit;
  end;

  if (OtherScopeRoot = HKLM) and not IsAdminInstallMode then
  begin
    { Stale hybrid already cleaned above by registry path; reaching here
      means genuine all-users install under Program Files. }
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
  StopRuntimeByExecutable(ExpandConstant('{app}\bin\NexUIProcess.exe'));
  // Process-isolation indexer: no `--quit` of its own, and the UI is gone by
  // now, so terminate it by image name before files are replaced.
  ForceStopRuntimeByPath(ExpandConstant('{app}\bin\NexIndexerProcess.exe'));
  StopRuntimeByExecutable(ExpandConstant('{app}\bin\NexHelper.exe'));
  StopRuntimeByExecutable(ExpandConstant('{app}\bin\Nex.exe'));
  StopRuntimeByExecutable(ExpandConstant('{app}\bin\nex-core.exe'));
  StopRuntimeByExecutable(ExpandConstant('{app}\bin\swiftfind-core.exe'));
end;

procedure RemoveLegacyOrphan();
var
  Location: string;
  HasHklmEntry: Boolean;
  LegacyDir, LegacyShortcut: string;
begin
  // Runs for per-user installs only (see CurStepChanged). Removes, in order:
  // 1. the orphaned pre-AppId tree + its stale all-users shortcut, but ONLY
  //    when no legitimate all-users install is registered — a registered
  //    entry pointing anywhere other than this install dir means hands off;
  // 2. a stale hybrid machine-wide entry left by an older updater bug:
  //    an HKLM entry pointing at this per-user install dir, or anywhere
  //    outside Program Files (legit all-users always under Program Files).
  // Kept in [Code] (not [InstallDelete]) so the key removal provably happens
  // before the directory removal is evaluated.
  HasHklmEntry := RegQueryStringValue(
    HKLM,
    'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#MyAppUninstallKey}',
    'InstallLocation',
    Location
  );
  if HasHklmEntry then
    Location := StripWrappingQuotes(Trim(Location));
  LegacyDir := ExpandConstant('{commonpf}\Nex');
  if (not HasHklmEntry) or SameInstallDir(Location, ExpandConstant('{app}')) or (not IsUnderProgramFiles(Location)) then
  begin
    if DirExists(LegacyDir) then
    begin
      if DelTree(LegacyDir, True, True, True) then
        Log('Removed legacy orphan dir: ' + LegacyDir)
      else
        Log('WARNING: could not remove legacy orphan dir (need elevation?): ' + LegacyDir);
    end;
    LegacyShortcut := ExpandConstant('{commonprograms}\Nex.lnk');
    if FileExists(LegacyShortcut) and DeleteFile(LegacyShortcut) then
      Log('Removed stale all-users shortcut: ' + LegacyShortcut);
    if HasHklmEntry then
    begin
      if RegDeleteKeyIncludingSubkeys(
        HKLM,
        'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#MyAppUninstallKey}'
      ) then
        Log('Removed stale hybrid machine-wide uninstall entry')
      else
        Log('WARNING: could not remove stale hybrid machine-wide uninstall entry');
    end;
  end
  else
    Log('Keeping machine-wide install at: ' + Trim(Location));
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
  // Clean up a legacy orphan / hybrid registration left by older
  // installers (per-user installs only — never touch admin installs).
  if (CurStep = ssPostInstall) and (not IsAdminInstallMode) then
    RemoveLegacyOrphan();
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

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  // The uninstaller asks once, on its welcome page. Reading the box here keeps the
  // answer before the wizard form and its controls are torn down.
  if Assigned(DeleteDataCheckbox) then
    DeleteUserDataAfterUninstall := DeleteDataCheckbox.Checked;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    if Assigned(DeleteDataCheckbox) then
      DeleteUserDataAfterUninstall := DeleteDataCheckbox.Checked;
    StopNexRuntime();
  end
  // The control is gone by the last step, which is why the answer was captured
  // above: touching it here raised "Could not call proc" and dropped the deletion.
  else if (CurUninstallStep = usPostUninstall) and DeleteUserDataAfterUninstall then
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
