#define MyAppName "Hardware Workbench"
#ifndef MyAppVersion
  #error MyAppVersion must be passed by build-installer.ps1 with /DMyAppVersion=<version>
#endif
#define MyAppExeName "hardware-workbench-app.exe"

[Setup]
AppId={{9C06F7D9-4E3B-45CF-8C3A-4373D6F83C79}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher=Hardware Workbench
DefaultDirName={localappdata}\Programs\HardwareWorkbench
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
OutputDir=..\dist
OutputBaseFilename=HardwareWorkbenchSetup
SetupIconFile=..\assets\app-icon.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=lowest
UninstallDisplayIcon={app}\assets\app-icon.ico
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked

[Dirs]
Name: "{app}\plugins"
Name: "{app}\logs"

[Files]
Source: "..\dist\hardware-workbench-app\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"; IconFilename: "{app}\assets\app-icon.ico"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"; IconFilename: "{app}\assets\app-icon.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent; WorkingDir: "{app}"

[UninstallDelete]
Type: files; Name: "{userappdata}\HardwareWorkbench\workspace.json"
Type: files; Name: "{userappdata}\HardwareWorkbench\workspace.json.backup"
Type: files; Name: "{userappdata}\HardwareWorkbench\*.tmp"
Type: filesandordirs; Name: "{userappdata}\HardwareWorkbench\plugin-config"
Type: filesandordirs; Name: "{userappdata}\HardwareWorkbench\update"
Type: filesandordirs; Name: "{userappdata}\HardwareWorkbench\updater"
Type: dirifempty; Name: "{userappdata}\HardwareWorkbench"
Type: files; Name: "{app}\workspace.json"
Type: files; Name: "{app}\workspace.json.backup"
Type: filesandordirs; Name: "{app}\plugin-config"
Type: filesandordirs; Name: "{app}\logs"
