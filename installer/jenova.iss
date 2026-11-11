; Jenova installer (Inno Setup 6). Built by .github/workflows/build-windows.yml:
;   iscc /DAppVersion=1.2.5 installer\jenova.iss
; Per-user install, no admin needed, so the in-app updater can replace the exe.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId=Jenova
AppName=Jenova
AppVersion={#AppVersion}
AppVerName=Jenova {#AppVersion}
AppPublisher=AnimaTheAeon
AppPublisherURL=https://github.com/AnimaTheAeon/Jenova
AppSupportURL=https://github.com/AnimaTheAeon/Jenova/issues
AppUpdatesURL=https://github.com/AnimaTheAeon/Jenova/releases
VersionInfoVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\Jenova
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\target\installer
OutputBaseFilename=Jenova-Setup
SetupIconFile=..\crate\resources\jenova_icon.ico
UninstallDisplayIcon={app}\jenova.exe
UninstallDisplayName=Jenova
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
CloseApplications=force
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\target\release\jenova.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Jenova"; Filename: "{app}\jenova.exe"; AppUserModelID: "AnimaTheAeon.Jenova"
Name: "{autodesktop}\Jenova"; Filename: "{app}\jenova.exe"; Tasks: desktopicon; AppUserModelID: "AnimaTheAeon.Jenova"

[Run]
Filename: "{app}\jenova.exe"; Description: "{cm:LaunchProgram,Jenova}"; Flags: nowait postinstall skipifsilent

[InstallDelete]
; Leftovers from in-app updates.
Type: files; Name: "{app}\jenova.exe.old*"

[UninstallDelete]
Type: files; Name: "{app}\jenova.exe.old*"
Type: dirifempty; Name: "{app}"

[Registry]
; The startup entry and toast registration are written by Jenova itself;
; remove them on uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Jenova"; Flags: uninsdeletevalue dontcreatekey
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\AnimaTheAeon.Jenova"; Flags: uninsdeletekey dontcreatekey

[Code]
// Close every running Jenova (window and background watcher, from any folder)
// so the exe can be replaced or removed.
procedure EndJenova();
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /T /IM jenova.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Sleep(500);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  EndJenova();
  Result := '';
end;

function InitializeUninstall(): Boolean;
begin
  EndJenova();
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Data: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    Data := ExpandConstant('{userappdata}\Jenova');
    if DirExists(Data) then
      if (not UninstallSilent) and (MsgBox('Also delete Jenova''s settings, history and logs?' + #13#10 + Data,
          mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES) then
        DelTree(Data, True, True, True);
  end;
end;
