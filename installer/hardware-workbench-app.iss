#define MyAppName "Hardware Workbench"
#ifndef MyAppVersion
  #error MyAppVersion must be passed by build-installer.ps1 with /DMyAppVersion=<version>
#endif
#define MyAppExeName "hardware-workbench-app.exe"

[Setup]
AppId={{9C06F7D9-4E3B-45CF-8C3A-4373D6F83C79}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
VersionInfoVersion={#MyAppVersion}
AppPublisher=Tydwdh
AppPublisherURL=https://github.com/Tydwdh/serial_tool
AppSupportURL=https://github.com/Tydwdh/serial_tool/issues
AppUpdatesURL=https://github.com/Tydwdh/serial_tool/releases
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
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\assets\app-icon.ico
LicenseFile=..\LICENSE
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "chinesesimp"; MessagesFile: ".\ChineseSimplified.isl"

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
; 解析失败被隔离的配置副本，文件名形如 workspace.json.corrupt-1727….backup。
; 上面三条精确/通配模式都匹配不到它，留着会让 HardwareWorkbench 目录非空、dirifempty 失效。
Type: files; Name: "{userappdata}\HardwareWorkbench\*.corrupt-*.backup"
Type: filesandordirs; Name: "{userappdata}\HardwareWorkbench\plugin-config"
Type: filesandordirs; Name: "{userappdata}\HardwareWorkbench\update"
Type: filesandordirs; Name: "{userappdata}\HardwareWorkbench\updater"
; 实机卸载验证发现：早期版本把主题与另一份布局写在漫游目录下（现在的
; `user_themes_dir()` 在安装目录里），这些遗留不删就让目录非空、dirifempty 失效
; —— 加这三条之前，卸载后该目录仍剩 12 个文件。
Type: filesandordirs; Name: "{userappdata}\HardwareWorkbench\themes"
Type: files; Name: "{userappdata}\HardwareWorkbench\workspace-iced.json"
Type: files; Name: "{userappdata}\HardwareWorkbench\workspace-iced.json.backup"
Type: dirifempty; Name: "{userappdata}\HardwareWorkbench"
; eframe 的窗口位置与 egui memory 持久化：目录名取自 main.rs 的
; `ViewportBuilder::with_app_id("hardware-workbench")`（小写，且多一层 data\），
; 与上面的 HardwareWorkbench 是两个不同目录，因此不会被上面任何一条顺带删掉。
Type: filesandordirs; Name: "{userappdata}\hardware-workbench"
Type: files; Name: "{app}\workspace.json"
Type: files; Name: "{app}\workspace.json.backup"
Type: filesandordirs; Name: "{app}\plugin-config"
Type: filesandordirs; Name: "{app}\plugins"
Type: filesandordirs; Name: "{app}\themes"
Type: filesandordirs; Name: "{app}\logs"
; 更新器写在安装目录内的残留：.exe.bak 只在替换成功时才删，替换失败仅回滚不删；
; .hw_update_probe_* 是写权限探测文件，进程被强杀时留在原地。
Type: files; Name: "{app}\*.exe.bak"
Type: files; Name: "{app}\.hw_update_probe_*"
; Inno 只删它自己记录过的文件，并且只在安装目录已空时才移除该目录；而
; copy_updated_resources 会把更新包里的 assets/docs/licenses/examples 整体复制进来，
; 更新后新增的文件不在安装日志里 —— 不兜底就会整个目录残留。
Type: filesandordirs; Name: "{app}"
