; Inno Setup 6. Built by scripts\package.ps1 with /DAppVersion and /DStageDir.
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef StageDir
  #define StageDir "..\dist\tidemark"
#endif

[Setup]
AppId={{6F0B5C0E-6C8B-4B7E-9D55-2D7A4D1E5A10}
AppName=Tidemark
AppVersion={#AppVersion}
AppPublisher=Tidemark
DefaultDirName={autopf}\Tidemark
DefaultGroupName=Tidemark
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
OutputBaseFilename=tidemark-{#AppVersion}-setup
SetupIconFile=..\assets\brand\tidemark.ico
UninstallDisplayIcon={app}\win-island.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayName=Tidemark
ChangesEnvironment=yes
CloseApplications=no

[Languages]
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "autostart"; Description: "Запускать вместе с Windows"; GroupDescription: "Дополнительно:"
Name: "desktopicon"; Description: "Ярлык на рабочем столе"; GroupDescription: "Дополнительно:"; Flags: unchecked

[Files]
Source: "{#StageDir}\win-island.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\island-hook.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\docs\*"; DestDir: "{app}\docs"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#StageDir}\wallpapers\*.png"; DestDir: "{app}\wallpapers"; Flags: ignoreversion
Source: "{#StageDir}\integrations\*"; DestDir: "{app}\integrations"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Tidemark"; Filename: "{app}\win-island.exe"
Name: "{group}\Настройки Tidemark"; Filename: "{app}\win-island.exe"; Parameters: "--settings"
Name: "{autodesktop}\Tidemark"; Filename: "{app}\win-island.exe"; Tasks: desktopicon

[Registry]
; The island registers this itself too (Settings window); the task just sets the first state.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "WinIsland"; ValueData: """{app}\win-island.exe"""; Flags: uninsdeletevalue; Tasks: autostart
; Lets the opencode plugin find island-hook.exe wherever the app was installed.
Root: HKCU; Subkey: "Environment"; ValueType: string; ValueName: "WIN_ISLAND_HOOK"; ValueData: "{app}\island-hook.exe"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\win-island.exe"; Description: "Запустить Tidemark"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM win-island.exe"; Flags: runhidden; RunOnceId: "KillIsland"

[Code]
procedure StopRunning();
var
  code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM win-island.exe', '', SW_HIDE, ewWaitUntilTerminated, code);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM island-hook.exe', '', SW_HIDE, ewWaitUntilTerminated, code);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopRunning();
  Result := '';
end;
