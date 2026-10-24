//! 文件关联守门:三平台打包配置里的「.md 关联声明」不许丢。
//!
//! 为什么需要守门(而不是只在提交时记得写):
//! 三份关联声明分布在**三个平台各自的打包文件**里(`packaging/linux/
//! latermd.desktop`、`packaging/macos/Info.plist`、`packaging/windows/
//! latermd.iss`),改这些文件没有任何编译器与测试会报警 —— 一个键名
//! 拼错、某段被重构掉、CI 里文件换了写法,用户侧症状都是「双击 md 用
//! 不了 LaterMD」,而且**本机测不出来**(本机是 Linux X11,只能验到
//! Linux 那一份;mac/Windows 的真实效果要真机)。
//!
//! 因此本守门做的是**文本层**的事实核对:文件在、关键键在、扩展名齐全、
//! 三处一致。它不替代真机验证,但能挡住最常见的退化(整段被删、键名写错、
//! 三平台扩展名清单漂移)。
//!
//! 判据不写成「文件里有 md 这个字符串」—— 那会被注释里的示例文字满足
//! (恒真)。每条断言都指向**具体键名**,且取自真实文件而非本模块写死的
//! 一份副本:后者会与文件漂移,变成测自己。

use std::path::{Path, PathBuf};

/// 仓库根(`packaging/` 的父)。`CARGO_MANIFEST_DIR` 是
/// `crates/latermd-app`,要上溯两级。
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("定位仓库根")
}

/// 读一份打包配置文件;读不到直接失败 —— 「文件没了」是最彻底的关联
/// 退化,不该退化成"跳过断言"。
///
/// 顺带剥掉 UTF-8 BOM:`latermd.iss` 存的是带 BOM 的 UTF-8(见
/// `inno_script_is_utf8_with_bom`),而 `read_to_string` 会把 BOM 当成正文
/// 的第一个字符,让「首行以 X 开头」这类判据莫名其妙地红。
fn read_packaging(relative: &str) -> String {
    let path = repo_root().join(relative);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读不到 {}:{error}", path.display()));
    raw.trim_start_matches('\u{feff}').to_owned()
}

/// 与 `latermd_search::MARKDOWN_EXTENSIONS` 对齐的关联扩展名。
///
/// 这里**再写一份**是刻意的:它是「打包配置应该长什么样」的期望值,
/// 与运行时那张表交叉核对(见 `extensions_match_runtime_table`)——
/// 两份独立写出来才能互相验,抄同一处等于没验。
const EXPECTED_EXTENSIONS: [&str; 2] = ["md", "markdown"];

/// 取出一条 `[Tasks]` 记录里 `Flags:` 参数的**逐个 token**(空列表 = 没写)。
///
/// 按 `;` 切参数再找 `Flags:` 那段,而不是 `line.contains("Flags: checked")`
/// 那种子串判断 —— 子串会把 `unchecked` / `checkedonce` 一起算进去。
fn inno_flags_of_line(line: &str) -> Vec<String> {
    line.split(';')
        .filter_map(|parameter| parameter.trim().strip_prefix("Flags:"))
        .flat_map(|flags| flags.split_whitespace())
        .map(|flag| flag.to_owned())
        .collect()
}

/// Inno Setup 6 文档里 `[Tasks]` 段 `Flags` 的**全部**合法取值。
/// 少一个都会被 iscc 判成 `Parameter "Flags" includes an unknown flag`
/// 并中止编译;多一个(`checked`)是本项目 v0.0.7 真实踩过的坑。
///
/// 出处:jrsoftware.org/ishelp `topic_taskssection.htm` 的 Flags 参数表。
const INNO_TASK_FLAGS: [&str; 6] = [
    "checkablealone",
    "checkedonce",
    "dontinheritcheck",
    "exclusive",
    "restart",
    "unchecked",
];

/// 运行时那张表的真源(`MARKDOWN_EXTENSIONS` 是唯一事实源,这里读它)。
fn runtime_extensions() -> Vec<String> {
    latermd_search::MARKDOWN_EXTENSIONS
        .iter()
        .map(|ext| (*ext).to_owned())
        .collect()
}

#[test]
fn linux_desktop_declares_markdown_mimetype_and_file_argument() {
    let desktop = read_packaging("packaging/linux/latermd.desktop");

    let mime_line = desktop
        .lines()
        .find(|line| line.starts_with("MimeType="))
        .unwrap_or_else(|| panic!(".desktop 没有 MimeType= 行(关联已失效)\n{desktop}"));
    assert!(
        mime_line.contains("text/markdown"),
        "MimeType 应含 text/markdown:{mime_line}"
    );

    let exec_line = desktop
        .lines()
        .find(|line| line.starts_with("Exec="))
        .unwrap_or_else(|| panic!(".desktop 没有 Exec= 行"));
    assert!(
        exec_line.contains("%F"),
        "Exec 必须带 %F,**去掉 %F 启动器照样能启动、但文件参数丢了**——\
         双击表现为「开了个空窗口」:{exec_line}"
    );
}

#[test]
fn macos_info_plist_declares_document_types_for_both_extensions() {
    let plist = read_packaging("packaging/macos/Info.plist");

    // 缺整段时LaterMD 在 Finder 的「打开方式」里根本不出现,症状与
    // 「没装上」无法区分 —— 所以这段的缺失必须被守住。
    assert!(
        plist.contains("<key>CFBundleDocumentTypes</key>"),
        "Info.plist 缺 CFBundleDocumentTypes(macOS 完全认不出 .md 关联)"
    );
    assert!(
        plist.contains("<key>CFBundleTypeExtensions</key>"),
        "缺 CFBundleTypeExtensions,只有 UTI 组不足以让 LaunchServices 找到本应用"
    );
    // UTI 与扩展名两组互补,少了 UTI 组则按类型匹配失效(14.0+ 主口径)
    assert!(
        plist.contains("<key>LSItemContentTypes</key>"),
        "缺 LSItemContentTypes"
    );
    assert!(
        plist.contains("net.daringfireball.markdown"),
        "应声明系统 Markdown UTI"
    );

    for extension in EXPECTED_EXTENSIONS {
        assert!(
            plist.contains(&format!("<string>{extension}</string>")),
            "CFBundleTypeExtensions 缺 {extension}"
        );
    }

    // 不抢默认:Owner/Default 会被 LaunchServices 当首选,顶掉用户原有的
    // 默认打开程序(与 AGENTS.md §5 的分发路线一致,故守住 Alternate)。
    assert!(
        plist.contains("<string>Alternate</string>"),
        "LSHandlerRank 应为 Alternate(声明候选而非抢占默认)"
    );
    for rank in ["<string>Owner</string>", "<string>Default</string>"] {
        assert!(
            !plist.contains(rank),
            "不该声明 {rank}:那是抢用户的默认打开程序"
        );
    }
}

#[test]
fn windows_inno_setup_registers_both_extensions_under_hkcu() {
    let iss = read_packaging("packaging/windows/latermd.iss");

    // 扩展名 → ProgID 的候选登记,两处都要
    for extension in EXPECTED_EXTENSIONS {
        let key = format!("Software\\Classes\\.{extension}\\OpenWithProgids");
        assert!(
            iss.contains(&key),
            ".iss 缺 {key}(LaterMD 不会出现在「打开方式」列表)"
        );
    }
    // ProgID 的 open 命令:这是「双击真能打开」的落点,键名错一个字符
    // 就变成"有入口没命令"。
    assert!(
        iss.contains("Software\\Classes\\Latermd.Markdown\\shell\\open\\command"),
        ".iss 缺 ProgID 的 shell\\open\\command"
    );
    // 命令行必须是 `"{app}\latermd.exe" "%1"` 的形态:路径含空格时,不加引号
    // 会被 Explorer 拆成两个参数,表现为「打开了一个奇怪的文件名 + 报找不到
    // 另一个」。
    //
    // 判据比对**整条 ValueData** 而不是 `contains("%1")`:后者会被任何含 %1
    // 的写法满足(含空格不引号的那种)。
    //
    // 期望值分两段拼(外层引号 / Inno 内层值),而不是写成一大串手抄的转义
    // 引号 —— 后者会被 Rust 自己的字符串字面量规则吃掉引号,判据与意图悄悄
    // 分叉(实测踩到两次:`"""%1"""` 被解析成三个参数、`r#""""%1""""#` 又多
    // 了一个引号)。内层 `""` 是 Inno 的转义写法(在引号内写一个字面量引号),
    // 落到注册表的值是 `"{app}\latermd.exe" "%1"`。
    let inner_value = "\"\"{app}\\latermd.exe\"\" \"\"%1\"\"";
    let expected_data = format!("\"{inner_value}\""); // .iss 里整串仍被引号包着
    let command_line = iss
        .lines()
        .find(|line| line.contains("shell\\open\\command"))
        .unwrap_or_else(|| panic!(".iss 缺 ProgID 的 shell\\open\\command 行"));
    assert!(
        command_line.contains(&format!("ValueData: {expected_data}")),
        "open command 应为 ValueData: {expected_data}(%1 必须带引号,\
     否则含空格的路径被拆成两个参数),实际:{command_line}"
    );

    // 关联做成安装向导里的可选项,且默认勾上。
    //
    // 「默认勾选」的**正确表达是不写 Flags** —— Inno 的任务默认就是勾选,
    // 而 `[Tasks]` 段的 Flags 白名单里**没有 `checked`** 这个值
    // (见 `inno_task_flags_only_use_documented_flags`)。所以这里判据是
    // 「没有 unchecked」,而不是「有 checked」:后者会把 `unchecked` 也算成
    // 满足(子串),恒真;而它期待的写法恰恰是让 iscc 编译失败的那个。
    assert!(
        iss.contains("Name: \"mdassoc\""),
        "应有 mdassoc 任务项(让用户能拒绝关联,不必手改注册表)"
    );
    let task_line = iss
        .lines()
        .find(|line| line.starts_with("Name: \"mdassoc\""))
        .expect("已断言存在");
    assert!(
        !inno_flags_of_line(task_line)
            .iter()
            .any(|flag| flag == "unchecked"),
        "关联应默认勾选(用户可取消勾选再重装):{task_line}"
    );

    // 全部写 HKCU:配合 PrivilegesRequired=lowest 不需要 UAC,也不与
    // 机器上其它用户的 LaterMD 打架。
    let registry_section = iss
        .split_once("[Registry]")
        .map(|(_, rest)| {
            rest.split_once("[Run]")
                .map(|(head, _)| head)
                .unwrap_or(rest)
        })
        .unwrap_or_else(|| panic!(".iss 缺 [Registry] 段"));
    assert!(
        registry_section.contains("Root: HKCU;"),
        "[Registry] 段应写 HKCU"
    );
    assert!(
        !registry_section.contains("Root: HKLM;") && !registry_section.contains("Root: HKA;"),
        "[Registry] 段不该出现 HKLM/HKA:会要管理员权限,与按用户装冲突"
    );
}

/// 守 Windows 安装器的 PATH 注册(2026-10-24 反馈 #3):终端里
/// `latermd 文档.md` 直开,靠的是安装目录进了用户 PATH。整段被删/写法
/// 漂移(RegWriteExPandedValue 误换成 RegWriteStringValue、ChangesEnvironment
/// 丢失)在本机(Linux,无 iscc)完全测不出来 —— 与文件关联守门同理由。
#[test]
fn windows_inno_setup_adds_to_path_via_code_section() {
    let iss = read_packaging("packaging/windows/latermd.iss");

    // 广播指令:没有它,装完要注销/重启资源管理器新终端才认新 PATH
    assert!(
        iss.contains("ChangesEnvironment=yes"),
        ".iss 缺 ChangesEnvironment=yes:PATH 写进去了也不广播,\
         新开的终端照样找不到 latermd 命令"
    );

    // 任务项存在且默认勾选:不写 Flags 即默认勾选(与 mdassoc 同判据,
    // 「有 checked」那种子串判断恒真,见上方踩坑注释)
    assert!(
        iss.contains("Name: \"addpath\""),
        ".iss 缺 addpath 任务项(用户应能在向导里拒绝 PATH 注册)"
    );
    let task_line = iss
        .lines()
        .find(|line| line.starts_with("Name: \"addpath\""))
        .expect("已断言存在");
    assert!(
        !inno_flags_of_line(task_line)
            .iter()
            .any(|flag| flag == "unchecked"),
        "PATH 注册应默认勾选:{task_line}"
    );

    // PATH 只能「读-改-写」:[Registry] 段整值覆盖会把用户已有 PATH 条目
    // 焊死丢失,所以写入必须在 [Code] 段完成
    assert!(
        iss.contains("RegQueryStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path'"),
        "PATH 应在 [Code] 段读-改-写,不能用 [Registry] 整值覆盖"
    );
    // 写回必须保 REG_EXPAND_SZ 类型:RegWriteStringValue 会把类型降成
    // REG_SZ,用户 Path 里的 %USERPROFILE% 之类变量从此不再展开
    assert!(
        iss.contains("RegWriteExPandedValue"),
        "PATH 写回应使用 RegWriteExPandedValue(保 REG_EXPAND_SZ 类型)"
    );
    // 装时按任务勾选决定写入,卸载时清理本应用条目
    assert!(
        iss.contains("WizardIsTaskSelected('addpath')"),
        "装时应按 addpath 任务勾选决定是否写入"
    );
    assert!(
        iss.contains("CurUninstallStepChanged"),
        "卸载时应把本应用条目从 PATH 移除"
    );
    // 只碰 HKCU:配合按用户安装不需要 UAC,也不动其它用户的 PATH
    assert!(
        !iss.contains("HKEY_LOCAL_MACHINE"),
        "PATH 注册不该碰 HKLM:会要管理员权限,与按用户装冲突"
    );
}

/// 守 `.iss` 里 `[Tasks]` 段的 Flags 只用文档白名单里的值。
///
/// 为什么单独立一条:v0.0.7(2026-10-18)把 Windows 安装器整包发没了 ——
/// `mdassoc` 任务写了 `Flags: checked`,iscc 报 `Parameter "Flags" includes
/// an unknown flag` 后中止,`release.yml` 的 `windows-installer` job 变红,
/// Release 里只剩 zip 与裸 exe。而**本机是 Linux,没有 iscc**,这条错在本机
/// 完全测不出来;当时 `packaging_assoc.rs` 里那条守门还在**主动要求**这个
/// 非法写法(`contains("checked")`),于是「测试全绿」与「发布物缺失」并存。
///
/// 这条判据与被断言的实现无关:白名单抄自 Inno Setup 官方文档,不是从当前
/// .iss 里反推的,所以「.iss 改出一个新写法」它会红,「.iss 保持现状」它绿。
#[test]
fn inno_task_flags_only_use_documented_flags() {
    let iss = read_packaging("packaging/windows/latermd.iss");

    // 只扫 [Tasks] 段:[Files]/[Run]/[Icons] 段的 Flags 是各自独立的
    // 参数集(ignoreversion / nowait / skipifsilent 等),混进来会误报。
    let tasks_section = iss
        .split_once("[Tasks]")
        .map(|(_, rest)| {
            // 段落在下一个 `[` 开头的行处结束
            rest.lines()
                .take_while(|line| !line.trim_start().starts_with('['))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_else(|| panic!(".iss 缺 [Tasks] 段"));
    assert!(
        tasks_section.contains("Name:"),
        "[Tasks] 段里一条任务都没有 —— 扫描范围退化成空集,下面会恒绿"
    );

    for line in tasks_section.lines().filter(|line| !line.trim().is_empty()) {
        // 注释与空行不在 Flags 白名单管辖内(注释里会写「unchecked」等字样)
        if line.trim_start().starts_with(';') {
            continue;
        }
        for flag in inno_flags_of_line(line) {
            assert!(
                INNO_TASK_FLAGS.contains(&flag.as_str()),
                "[Tasks] 用了文档里不存在的 flag `{flag}`:iscc 会报 \
                 `Parameter \"Flags\" includes an unknown flag` 并中止编译,\
                 整个安装器发不出来(合法值只有 {}),行:{line}",
                INNO_TASK_FLAGS.join(" / ")
            );
        }
    }
}

/// `.iss` 必须是带 BOM 的 UTF-8,且 `read_packaging` 确实把 BOM 剥掉了。
///
/// Inno Setup 6.3 之前不带 BOM 一律按 ANSI 读脚本,本文件 `[Tasks]` 段的
/// Description 里有中文,会被读成乱码显示在安装向导的「选择附加任务」页上
/// (6.3 起才支持无 BOM;而 GitHub runner 上预装的 iscc 版本会随镜像漂移,
/// 不能假定 ≥6.3)。
///
/// 顺带守 `read_packaging` 的 BOM 剥离:文件带 BOM 而读取端不剥,任何
/// 「首行以 X 开头」的判据都会莫名其妙地红(首字符是 U+FEFF)。这条判据
/// 读的是**字节**判存在性、**文本**判已剥离,两头都要。
#[test]
fn inno_script_is_utf8_with_bom() {
    let path = repo_root().join("packaging/windows/latermd.iss");
    let bytes =
        std::fs::read(&path).unwrap_or_else(|error| panic!("读不到 {}:{error}", path.display()));
    assert!(
        bytes.starts_with(&[0xEF, 0xBB, 0xBF]),
        "latermd.iss 缺 UTF-8 BOM:Inno Setup 6.3 之前按 ANSI 读脚本,\
         [Tasks] 段 Description 里的中文会在安装向导上显示成乱码"
    );
    let body = &bytes[3..];
    assert!(std::str::from_utf8(body).is_ok(), "BOM 之后不是合法 UTF-8");

    // 读取端必须剥掉,否则首行派生的判据全被 U+FEFF 顶掉一个字符
    let text = read_packaging("packaging/windows/latermd.iss");
    assert!(
        !text.starts_with('\u{feff}'),
        "read_packaging 没剥 BOM:首行以 U+FEFF 开头,\
         `starts_with(\"...\")` 类判据会全红"
    );
    assert_eq!(
        text,
        String::from_utf8(body.to_vec()).expect("上面已验过 UTF-8"),
        "剥 BOM 后应与文件内容逐字节一致(剥多了别的字符)"
    );
}

#[test]
fn extensions_match_runtime_table() {
    let runtime = runtime_extensions();
    assert_eq!(
        runtime,
        EXPECTED_EXTENSIONS.to_vec(),
        "latermd_search::MARKDOWN_EXTENSIONS 变了 —— 三平台关联清单要同步,\
         否则「程序打不开」与「系统不让选」两处症状会分叉"
    );
}

/// 三平台都必须认这两个扩展名 —— 拆成一条是为了报错时直接点出是哪个
/// 平台缺哪个扩展名。
#[test]
fn all_three_platforms_cover_both_extensions() {
    let desktop = read_packaging("packaging/linux/latermd.desktop");
    let plist = read_packaging("packaging/macos/Info.plist");
    let iss = read_packaging("packaging/windows/latermd.iss");

    for extension in EXPECTED_EXTENSIONS {
        // Linux:两条 glob 归到同一个 text/markdown(实测 shared-mime-info
        // 的 globs2 里 *.md 与 *.markdown 同属 text/markdown),故按 MIME
        // 类型断言而非按扩展名逐个列。
        assert!(
            desktop.contains("text/markdown"),
            "Linux: text/markdown 未声明,{extension} 无从关联"
        );
        assert!(
            plist.contains(&format!("<string>{extension}</string>")),
            "macOS: CFBundleTypeExtensions 缺 {extension}"
        );
        assert!(
            iss.contains(&format!("Software\\Classes\\.{extension}\\OpenWithProgids")),
            "Windows: 缺 .{extension} 的 OpenWithProgids"
        );
    }
}
