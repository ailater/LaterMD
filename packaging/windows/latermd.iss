; LaterMD Inno Setup script(对齐 GLMeter 的发行形态)
; Build (from repo root, after copying latermd.exe + latermd.ico here):
;   iscc /DVERSION=x.y.z latermd.iss

#ifndef VERSION
  #define VERSION "0.0.0"
#endif

[Setup]
AppId={{7E4F2A18-9C3B-4D5E-8A61-2F0B9C4D5E7A}
AppName=LaterMD
AppVersion={#VERSION}
AppPublisher=crazykun
AppPublisherURL=https://github.com/ailater/LaterMd
AppSupportURL=https://github.com/ailater/LaterMd/issues
; 按用户装(localappdata),无 UAC 弹窗;需要机器级安装时向导可选提权
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={localappdata}\Programs\LaterMD
DefaultGroupName=LaterMD
OutputBaseFilename=latermd-{#VERSION}-setup
OutputDir=.
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=latermd.ico
UninstallDisplayIcon={app}\latermd.exe

[Files]
Source: "latermd.exe"; DestDir: "{app}"; Flags: ignoreversion

[Tasks]
Name: "desktop"; Description: "Create a desktop shortcut (创建桌面快捷方式)"; Flags: unchecked

[Icons]
Name: "{group}\LaterMD"; Filename: "{app}\latermd.exe"
Name: "{userdesktop}\LaterMD"; Filename: "{app}\latermd.exe"; Tasks: desktop

[Run]
Filename: "{app}\latermd.exe"; Description: "Launch LaterMD now"; Flags: nowait postinstall skipifsilent
