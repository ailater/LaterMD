//! macOS 的 accesskit 关停守门:`eframe` 的 feature 分档不许退化。
//!
//! 为什么需要守门(与 `packaging_assoc.rs` 同款理由):
//! `crates/latermd-app/Cargo.toml` 把 `eframe` 拆成了两个
//! `[target.*.dependencies]` 段 —— macOS 关 accesskit,其余平台保留。
//! 这么写**没有任何编译器会报警**:谁把 macOS 段的 `accesskit` 加回来、
//! 或顺手删掉 `default-features = false`,Linux 上照样编译通过、测试全绿,
//! 而崩溃只发生在 macOS 真机(且是间歇性的,跑几分钟才炸一次)。
//! 本机是 Linux,永远测不到那个症状。
//!
//! 判据取自**真实 manifest**,不写死一份期望副本(抄一份等于测自己)。
//!
//! 解析必须走结构化提取,不能 `section.contains("accesskit")`:
//! 本文件与 Cargo.toml 的注释里都写着 accesskit 这个词,子串判断恒红
//! (或反过来:删掉 features 数组后靠注释也能"绿")。一律只认
//! `features = [ ... ]` 数组里的 token,并断言数组非空。

use std::path::{Path, PathBuf};

/// 仓库根(`crates/` 的父)。
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("定位仓库根")
}

fn manifest_text() -> String {
    let path = repo_root().join("crates/latermd-app/Cargo.toml");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读不到 {}:{error}", path.display()))
}

/// 取某个段头之下的正文,直到下一个段头(或 EOF)。
///
/// 段头指行首 `[` 开头的那一行 —— TOML 的表头一定在行首,代码块里的
/// `[` 不会(本仓 Cargo.toml 无内联表数组嵌套)。
fn section<'a>(manifest: &'a str, header: &str) -> &'a str {
    let start = manifest
        .find(header)
        .unwrap_or_else(|| panic!("manifest 里找不到段头 {header}"));
    let body_start = manifest[start..]
        .find('\n')
        .map(|offset| start + offset + 1)
        .unwrap_or(manifest.len());
    let rest = &manifest[body_start..];
    match rest.find("\n[") {
        Some(offset) => &rest[..offset + 1],
        None => rest,
    }
}

/// 取段内 `eframe = { ... }` 声明块(含大括号),按括号配对取到闭合的 `}`。
fn eframe_declaration(section: &str) -> &str {
    let start = section
        .find("eframe = {")
        .unwrap_or_else(|| panic!("该段里没有 `eframe = {{ ... }}` 声明:\n{section}"));
    let open = start + section[start..].find('{').expect("有 `{`");
    let mut depth = 0usize;
    for (offset, byte) in section[open..].bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &section[open..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("`eframe` 声明的大括号没有闭合:\n{section}")
}

/// 取声明块里 `features = [ ... ]` 的 token(去引号、去空白)。
fn feature_list(declaration: &str) -> Vec<String> {
    let start = declaration
        .find("features = [")
        .unwrap_or_else(|| panic!("`eframe` 声明里没有 `features = [ ... ]`:\n{declaration}"));
    let open = start + declaration[start..].find('[').expect("有 `[`");
    let mut depth = 0usize;
    for (offset, byte) in declaration[open..].bytes().enumerate() {
        match byte {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    let inner = &declaration[open + 1..open + offset];
                    return inner
                        .split(',')
                        .map(|token| token.trim().trim_matches('"').trim().to_owned())
                        .filter(|token| !token.is_empty())
                        .collect();
                }
            }
            _ => {}
        }
    }
    panic!("`features` 数组没有闭合:\n{declaration}")
}

/// 声明里是否写了 `default-features = false`。
fn default_features_off(declaration: &str) -> bool {
    declaration
        .split(',')
        .chain(declaration.lines())
        .any(|token| {
            let token = token.trim();
            token.starts_with("default-features") && token.contains("false")
        })
}

/// macOS 段:关掉 accesskit,其余默认 feature 一项不能少。
#[test]
fn macos_build_drops_accesskit() {
    let manifest = manifest_text();
    let macos = section(
        &manifest,
        "[target.'cfg(target_os = \"macos\")'.dependencies]",
    );
    let declaration = eframe_declaration(macos);
    assert!(
        default_features_off(declaration),
        "macOS 段必须显式 `default-features = false`:eframe 的 accesskit 在 default 里,\n\
         不写这句等于把崩溃又开回来:\n{declaration}"
    );
    let features = feature_list(declaration);
    assert!(!features.is_empty(), "features 数组不能为空(解析退化?)");
    assert!(
        !features.iter().any(|feature| feature == "accesskit"),
        "macOS 段不许开 accesskit —— accesskit_macos 0.26.3 的 SubclassingAdapter\n\
         会让 AppKit 的 Touch Bar KVO 在下一次 removeObserver 时抛 NSRangeException\n\
         (上游 AccessKit/accesskit#823,真机崩溃 2026-10-23 22:49):\n{features:?}"
    );
    // eframe 0.36.2 的 default 去掉 accesskit 后的六项(逐项抄自
    // eframe-0.36.2/Cargo.toml;`winit/default` 因 Cargo 禁止依赖声明里
    // 出现 "dep/feat" 而无法照抄,见 Cargo.toml 里的注释)。
    for expected in [
        "default_fonts",
        "links",
        "wayland",
        "web_screen_reader",
        "wgpu",
        "x11",
    ] {
        assert!(
            features.iter().any(|feature| feature == expected),
            "macOS 段的 eframe 少了默认 feature `{expected}`(关 accesskit 不该连带丢别的):{features:?}"
        );
    }
}

/// 其余平台:accesskit **必须**保留 —— 屏幕阅读器支持不该为 macOS 的坑陪葬。
#[test]
fn other_platforms_keep_accesskit() {
    let manifest = manifest_text();
    let others = section(
        &manifest,
        "[target.'cfg(not(target_os = \"macos\"))'.dependencies]",
    );
    let declaration = eframe_declaration(others);
    assert!(
        !default_features_off(declaration),
        "非 macOS 段不该写 `default-features = false`:那会把 Windows/Linux 的无障碍一起关掉\n\
         (accesskit 在这两个平台走 UIA / AT-SPI,没有 macOS 那个缺陷):\n{declaration}"
    );
}

/// 取声明块里的版本号。
///
/// 不按 `,` 切分再找 `version = `:macOS 段的声明是**跨行**的,数组里的
/// 逗号会把 `version = "0.36.2"` 所在的那片切碎。直接找 `version` 之后
/// 第一对引号之间的内容。
fn eframe_version(declaration: &str) -> String {
    let key = declaration
        .find("version")
        .unwrap_or_else(|| panic!("`eframe` 声明里没有 version:\n{declaration}"));
    let rest = &declaration[key..];
    let open = rest.find('"').expect("版本号有开引号");
    let close = rest[open + 1..].find('"').expect("版本号有闭引号");
    rest[open + 1..open + 1 + close].to_owned()
}

/// 两段的 eframe 版本必须同步:版本漂移会产生「只有某一平台编不过」的
/// 半成品提交,而本机(Linux)只验得到其中一段。
#[test]
fn both_sections_pin_the_same_eframe_version() {
    let manifest = manifest_text();
    let version_of = |header: &str| -> String {
        let declaration = eframe_declaration(section(&manifest, header));
        eframe_version(declaration)
    };
    let macos = version_of("[target.'cfg(target_os = \"macos\")'.dependencies]");
    let others = version_of("[target.'cfg(not(target_os = \"macos\"))'.dependencies]");
    assert_eq!(macos, others, "两段的 eframe 版本必须一致");
    assert!(!macos.is_empty(), "版本号不能解析成空串(解析退化?)");
}
