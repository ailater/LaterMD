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
fn read_packaging(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读不到 {}:{error}", path.display()))
}

/// 与 `latermd_search::MARKDOWN_EXTENSIONS` 对齐的关联扩展名。
///
/// 这里**再写一份**是刻意的:它是「打包配置应该长什么样」的期望值,
/// 与运行时那张表交叉核对(见 `extensions_match_runtime_table`)——
/// 两份独立写出来才能互相验,抄同一处等于没验。
const EXPECTED_EXTENSIONS: [&str; 2] = ["md", "markdown"];

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
    assert!(
        iss.contains("Name: \"mdassoc\""),
        "应有 mdassoc 任务项(让用户能拒绝关联,不必手改注册表)"
    );
    let task_line = iss
        .lines()
        .find(|line| line.starts_with("Name: \"mdassoc\""))
        .expect("已断言存在");
    assert!(
        task_line.contains("checked"),
        "关联应默认勾选(用户可取消):{task_line}"
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
