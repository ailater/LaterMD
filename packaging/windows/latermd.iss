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
; PATH 注册(2026-10-24 反馈 #3)后广播 WM_SETTINGCHANGE("Environment"):
; 资源管理器与之后新开的终端立刻拿到新 PATH;装之前就开着的终端要
; 重开一次才认(Windows 不热更新已开进程的环境)。
ChangesEnvironment=yes

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
; 命令行入口(2026-10-24 反馈 #3):把安装目录追加进用户 PATH,终端里
; `latermd 文档.md` 直开。默认勾选(不写 Flags,与 mdassoc 同口径);
; 实际写入在 [Code] 段(HKCU\Environment 的 Path 只能读改写,[Registry]
; 段的整值覆盖会把用户已有 PATH 焊死)。
Name: "addpath"; Description: "Add LaterMD to PATH (添加到 PATH,可在命令行用 latermd 命令打开文档)"

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

; ---------------------------------------------------------------------------
; 命令行入口:把 {app} 追加进用户 PATH(2026-10-24 反馈 #3)
; ---------------------------------------------------------------------------
; 为什么在 [Code] 而不在 [Registry]:HKCU\Environment 的 Path 是「读出来、
; 追加、写回」的复合操作 —— [Registry] 段只会整值覆盖,等于把用户已有的
; PATH 条目全部焊死丢失。以下实现照抄 Inno 官方 FAQ 的 PATH 做法:
; - 读:`RegQueryStringValue` 读 REG_EXPAND_SZ 拿到的是**未展开**原文
;   (%USERPROFILE% 之类保持变量形态),因此写回必须用
;   `RegWriteExPandedValue` 保住 REG_EXPAND_SZ 类型 —— 换成
;   RegWriteStringValue 会把类型降成 REG_SZ,用户 Path 里的变量从此
;   不再展开;
; - 去重按「分号切开的整条目」精确比较(大小写不敏感),不做子串判断
;   —— D:\Tools 匹配不了 D:\Tools2;
; - 只写 HKCU:配合 PrivilegesRequired=lowest 不需要 UAC,也不碰机器
;   上其它用户的 PATH(与文件关联的 mdassoc 任务同口径);
; - 卸载时把本应用那条目从 Path 里移除(装时没勾 addpath 的用户卸载
;   也无损伤:切分重拼等于原值),PATH 被清空则删值不删键。

[Code]
const
  EnvironmentKey = 'Environment';

{ 把 Path 值按分号切进列表(Delphi/Pascal 没有内建 Split,手写循环)。 }
procedure SplitPathEntries(const Value: String; Entries: TStringList);
var
  Rest: String;
  Cut: Integer;
begin
  Rest := Value;
  while Rest <> '' do
  begin
    Cut := Pos(';', Rest);
    if Cut > 0 then
    begin
      Entries.Add(Copy(Rest, 1, Cut - 1));
      Delete(Rest, 1, Cut);
    end
    else
    begin
      Entries.Add(Rest);
      Rest := '';
    end;
  end;
end;

procedure AddToPath();
var
  PathValue: String;
  AppDir: String;
begin
  AppDir := ExpandConstant('{app}');
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path', PathValue) then
    PathValue := '';
  // 已含本目录(整条目精确比较)则不动:重装/升级幂等
  if Pos(';' + LowerCase(AppDir) + ';', ';' + LowerCase(PathValue) + ';') > 0 then
    exit;
  if PathValue <> '' then
    PathValue := PathValue + ';' + AppDir
  else
    PathValue := AppDir;
  RegWriteExPandedValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path', PathValue);
end;

procedure RemoveFromPath();
var
  PathValue: String;
  AppDir: String;
  Entries: TStringList;
  Rebuilt: String;
  Index: Integer;
begin
  AppDir := ExpandConstant('{app}');
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path', PathValue) then
    exit;
  Entries := TStringList.Create;
  try
    SplitPathEntries(PathValue, Entries);
    Rebuilt := '';
    for Index := 0 to Entries.Count - 1 do
      if CompareText(Entries.Strings[Index], AppDir) <> 0 then
        if Rebuilt = '' then
          Rebuilt := Entries.Strings[Index]
        else
          Rebuilt := Rebuilt + ';' + Entries.Strings[Index];
    // 条目没变(装时就没勾 addpath):原样写回也无害,但少一次写注册表
    if CompareText(Rebuilt, PathValue) = 0 then
      exit;
    if Rebuilt = '' then
      RegDeleteValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path')
    else
      RegWriteExPandedValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path', Rebuilt);
  finally
    Entries.Free;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addpath') then
    AddToPath();
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath();
end;
