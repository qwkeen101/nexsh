; Windows 安装程序脚本（Inno Setup 6）。由 .github/workflows/release.yml 调用，
; 版本号和标签通过环境变量 NEXSH_VERSION、NEXSH_TAG 传入。

#define AppVersion GetEnv("NEXSH_VERSION")
#define AppTag GetEnv("NEXSH_TAG")

[Setup]
; AppId 用来识别同一个程序的不同版本，升级安装依赖它，不能改。
AppId={{6B1F3C52-8D0E-4E0B-9C61-5A7B0D2F4E19}
AppName=nexsh
AppVersion={#AppVersion}
AppPublisher=nexsh
AppPublisherURL=https://github.com/qwkeen101/nexsh
DefaultDirName={autopf}\nexsh
DisableProgramGroupPage=yes
; 默认装到当前用户目录，不需要管理员权限；安装时可以选择为所有用户安装。
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..
OutputBaseFilename=nexsh-{#AppTag}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\nexsh.exe

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\target\release\nexsh.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\nexsh-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\nexsh"; Filename: "{app}\nexsh.exe"
Name: "{autodesktop}\nexsh"; Filename: "{app}\nexsh.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\nexsh.exe"; Description: "{cm:LaunchProgram,nexsh}"; Flags: nowait postinstall skipifsilent
