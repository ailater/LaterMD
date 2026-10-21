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
; 文件关联做成**默认勾选**的可选任务:勾上则写 HKCU 注册表(不需要管理员,
; 与 PrivilegesRequired=lowest 一致),LaterMD 出现在「打开方式」列表里;
; 不勾也能正常用,只是要用户自己去「设置 → 应用 → 默认应用」里认领。
; 用户可随时在安装器里取消勾选重装回去,不必手动清注册表。
;
; 「默认勾选」在 Inno 里的正确表达是**不写 Flags**(任务默认即勾选)。
; [Tasks] 段的 Flags 只认 checkablealone / checkedonce / dontinheritcheck /
; exclusive / restart / unchecked —— **没有 checked**,写上去 iscc 直接报
; `Parameter "Flags" includes an unknown flag` 并中止编译,整个安装器就没了
; (2026-10-18 v0.0.7 实测:release.yml 的 windows-installer job 红,Release
; 里只剩 zip + 裸 exe)。要表达「默认不勾」才写 Flags: unchecked。
Name: "mdassoc"; Description: "Associate .md and .markdown files with LaterMD (关联 .md / .markdown 文件,可在系统「默认应用」中改为其它程序)"

[Icons]
Name: "{group}\LaterMD"; Filename: "{app}\latermd.exe"
Name: "{userdesktop}\LaterMD"; Filename: "{app}\latermd.exe"; Tasks: desktop

; ---------------------------------------------------------------------------
; 文件关联(2026-10-16)
; ---------------------------------------------------------------------------
; 关键口径(与 macOS Info.plist / Linux .desktop 三处保持一致):
; - 扩展名清单只 md / markdown,与 latermd_search::MARKDOWN_EXTENSIONS 同源;
; -**不设默认打开程序**。写 HKCU\...\FileExts\.md\UserChoice 是受保护
;  键(需 TrustedInstaller 权限接管),程序无权可靠写;能写的
;  OpenWithProgids 只让应用「出现在列表里」,设默认仍由用户在系统 UI
;  里点一次 —— 这也是 Windows 对所有第三方应用的一致要求。
; -全部写 HKCU(当前用户),不碰 HKLM:配合 PrivilegesRequired=lowest,
;  按用户装不需要 UAC,也不会与机器上其它用户的 LaterMD 打架。
; -ProgID 用 `Latermd.Markdown` 而非 `.md`:扩展名键下直接放命令是
;  老式做法,ProgID 才能让「其它应用 → 打开方式」也找得到本应用。

[Registry]
; ① 应用自身的 ProgID:声明「本应用能打开 .md / .markdown」
Root: HKCU; Subkey: "Software\Classes\Latermd.Markdown\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\latermd.exe,0"; Flags: uninsdeletekey; Tasks: mdassoc
Root: HKCU; Subkey: "Software\Classes\Latermd.Markdown\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\latermd.exe"" ""%1"""; Flags: uninsdeletekey; Tasks: mdassoc
; 说明: 直接双击走「.md → ProgID」的默认路径;下面的「打开方式」
; 让用户能在任意 .md 上右键手动选本应用(没有这一条时,应用在列表里
; 是隐形的,只有先设了默认才能被发现)。

; ② 扩展名 → ProgID 的候选登记(不夺默认,只进「打开方式」列表)
Root: HKCU; Subkey: "Software\Classes\.md\OpenWithProgids"; ValueType: string; ValueName: "Latermd.Markdown"; ValueData: ""; Flags: uninsdeletevalue; Tasks: mdassoc
Root: HKCU; Subkey: "Software\Classes\.markdown\OpenWithProgids"; ValueType: string; ValueName: "Latermd.Markdown"; ValueData: ""; Flags: uninsdeletevalue; Tasks: mdassoc

; ③ 「用其它应用打开」列表里的显示名与图标(Explorer 读这两个键渲染条目)
Root: HKCU; Subkey: "Software\Classes\Applications\latermd.exe\SupportedTypes"; ValueType: string; ValueName: ".md"; ValueData: ""; Flags: uninsdeletekey; Tasks: mdassoc
Root: HKCU; Subkey: "Software\Classes\Applications\latermd.exe\SupportedTypes"; ValueType: string; ValueName: ".markdown"; ValueData: ""; Flags: uninsdeletekey; Tasks: mdassoc

[Run]
Filename: "{app}\latermd.exe"; Description: "Launch LaterMD now"; Flags: nowait postinstall skipifsilent
