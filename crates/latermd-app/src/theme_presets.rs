//! 出厂预设色板(ui-modernization.md §3 U0)。
//!
//! 应用色板覆盖整个工作台，正文样式与外壳一起导出。
//!
//! 抄 egui-thematic 0.1.1 的九套预设**色值**(该 crate MIT,2026-09-28 从
//! crates.io 下载源码核对;它锁 egui 0.33 故被否决引库,数值白拿 ——
//! ui-modernization.md §2.1)。正文沿用 MarkdownStyle，应用色板按各系列
//! 的公开配色搭配明暗表面与强调色，存为完整 SkinFile，零新增依赖。
//!
//! 载入语义:预设**以字面量编进二进制**,启动时铺进皮肤目录一次
//! (`themes/<name>.ron` 不存在才写)—— 皮肤系统由此拿到普通 `.ron`
//! 文件:可被扫描、可被用户改、改了不会在下次启动被出厂值顶掉。
//!
//! 色板是纯数据，投影与切换统一由 ThemeSettings 负责。

use std::sync::OnceLock;

use eframe::egui::Color32;
use egui_markdown_style::{CodeBlockStyle, MarkdownStyle, TableStyle};

/// 一份出厂预设:名字(= 皮肤文件名)与正文样式。
pub struct BuiltinSkin {
    /// 皮肤名,与落盘的 `themes/<name>.ron` 同名(含空格与 `é`,文件名合法)。
    pub name: &'static str,
    /// 完整正文样式。
    pub style: MarkdownStyle,
}

/// 出厂预设表(顺序即首次铺盘顺序,展示序由 SkinCatalog 排序决定)。
pub fn builtins() -> &'static [BuiltinSkin] {
    static BUILTINS: OnceLock<Vec<BuiltinSkin>> = OnceLock::new();
    BUILTINS.get_or_init(skins)
}

/// 把内置预设铺进皮肤目录:**文件不存在才写**,已存在(含用户改过的)
/// 一律不动。目录不存在则创建。返回实际写入的皮肤名(空 = 全部已在)。
///
/// 失败不 panic:预设缺失只是「少几套可选皮肤」,不该挡启动;调用方自行
/// 决定是否告警。
pub fn install_to(dir: &std::path::Path) -> Vec<String> {
    let themes = dir.join(crate::theme::THEMES_DIR);
    let mut written = Vec::new();
    for skin in builtins() {
        let path = themes.join(format!("{}.ron", skin.name));
        if path.exists() {
            continue;
        }
        let Ok(text) = ron::ser::to_string_pretty(
            &crate::theme::SkinFile {
                markdown: skin.style.clone(),
                shell: shell_palette(skin.name).unwrap_or_default(),
            },
            ron::ser::PrettyConfig::default(),
        ) else {
            continue;
        };
        if std::fs::create_dir_all(&themes).is_ok() && std::fs::write(&path, text).is_ok() {
            written.push(skin.name.to_owned());
        }
    }
    written
}

// —— 构造辅助 ——

/// 块级默认:代码块/表格圆角 4(与 `RADIUS_SM` 同档,shadcn rounded-sm);
/// 表格边框 1px + 表头/隔行底色(#30,与出厂默认同口径,线色/底色渲染时
/// 自动取 `widgets.noninteractive.bg_stroke` / `faint_bg_color`,明暗自适应)。
/// 标题排版(#23 F4/F5,preview-typography §2 待办 A):`heading.scales`
/// 用 vendored 新分级 [2.0,…,1.0](`MarkdownStyle::default()` 自带,不在
/// 此重复写死);`heading_space_above` 显式钉 [`crate::theme::
/// default_markdown_style`] 的出厂值(10.0,2026-10-30 设计审查自 vendored
/// 出厂 4.0 提档)—— 内置预设只在**颜色**上不同,标题节奏是全局一致性
/// 属性,不按皮肤微调;交叉断言见
/// `builtins_ship_heading_typography_matching_factory`。
fn base() -> MarkdownStyle {
    let mut style = MarkdownStyle::default();
    style.code_font_size = 13.0;
    style.heading_space_above = crate::theme::default_markdown_style().heading_space_above;
    style.code_block = CodeBlockStyle {
        corner_radius: 4.0,
        ..style.code_block
    };
    style.table = TableStyle {
        stroke_width: 1.0,
        corner_radius: 4.0,
        header_fill: true,
        zebra_fill: true,
        ..style.table
    };
    style
}

/// 行内代码颜色对(文字明/暗、底色明/暗)。`MarkdownStyle` 的字段本就成对
/// 设计(dark/light),一套色板同时给出两套值,明暗切换时皮肤自动跟随。
fn code(
    style: &mut MarkdownStyle,
    text_dark: [u8; 3],
    text_light: [u8; 3],
    bg_dark: [u8; 3],
    bg_light: [u8; 3],
) {
    let to_color = |rgb: [u8; 3]| Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    style.inline_code.color_dark = to_color(text_dark);
    style.inline_code.color_light = to_color(text_light);
    style.inline_code.background_dark = to_color(bg_dark);
    style.inline_code.background_light = to_color(bg_light);
}

/// 引用条几何统一(条宽 2、每层缩进 14)。`BlockquoteStyle` 没有颜色字段,
/// 色板的引用观感由正文层吸收,这里只统一形状。
fn quote(style: &mut MarkdownStyle) {
    style.blockquote.stroke_width = 2.0;
    style.blockquote.indent_per_depth = 14.0;
}

fn skin(
    name: &'static str,
    text_dark: [u8; 3],
    text_light: [u8; 3],
    bg_dark: [u8; 3],
    bg_light: [u8; 3],
) -> BuiltinSkin {
    let mut style = base();
    code(&mut style, text_dark, text_light, bg_dark, bg_light);
    quote(&mut style);
    BuiltinSkin { name, style }
}

/// 原九套色板加「青竹护眼」。原预设暗色侧数值抄自 egui-thematic 0.1.1 `config.rs` 的各 preset
/// (text_color → 文字色、code_bg/panel_fill → 底色);亮色侧是同色板的
/// 公开浅色变体配对(egui-thematic 对 Dracula 等 8 套只给了暗色一套值,
/// `MarkdownStyle` 的成对字段要求两边都给)。
/// One Light 与 Rosé Pine 在 egui-thematic 0.1.1 中**不存在**(它有的是
/// Monokai/Catppuccin Mocha),按各官方 palette 补齐 —— 任务点名的九套
/// 与该 crate 的九套并非同一集合。
fn skins() -> Vec<BuiltinSkin> {
    vec![
        skin(
            "青竹护眼",
            [0xC6, 0xDE, 0xCA],
            [0x29, 0x4B, 0x35],
            [0x27, 0x3C, 0x2E],
            [0xC2, 0xD9, 0xC7],
        ),
        skin(
            "Dracula",
            // 正文 F8F8F2 / panel 44475A / 极底 15161E
            [0xF8, 0xF8, 0xF2],
            [0x28, 0x2A, 0x36],
            [0x44, 0x47, 0x5A],
            [0xF2, 0xF2, 0xF5],
        ),
        skin(
            "Nord",
            // 正文 D8DEE9 / panel 3B4252 / 极底 1D202A
            [0xD8, 0xDE, 0xE9],
            [0x2E, 0x34, 0x40],
            [0x3B, 0x42, 0x52],
            [0xE5, 0xE9, 0xF0],
        ),
        skin(
            "Gruvbox Dark",
            // 正文 EBDBB2 / panel 3C3836 / 窗底 282828
            [0xEB, 0xDB, 0xB2],
            [0x50, 0x49, 0x45],
            [0x3C, 0x38, 0x36],
            [0xF2, 0xE5, 0xBC],
        ),
        skin(
            "Solarized Dark",
            // 正文 93A1A1 / panel 073642 / 窗底 002B36
            [0x93, 0xA1, 0xA1],
            [0x58, 0x6E, 0x75],
            [0x07, 0x36, 0x42],
            [0xEE, 0xE8, 0xD5],
        ),
        skin(
            "Solarized Light",
            // 正文 657B83 / panel EEE8D5 / 窗底 FDF6E3
            [0xB4, 0xC5, 0xC5],
            [0x58, 0x6E, 0x75],
            [0x07, 0x36, 0x42],
            [0xEE, 0xE8, 0xD5],
        ),
        skin(
            "Tokyo Night",
            // 正文 C0CAF5 / panel 24283B / 窗底 1A1B26
            [0xC0, 0xCA, 0xF5],
            [0x34, 0x3B, 0x58],
            [0x24, 0x28, 0x3B],
            [0xE1, 0xE6, 0xF2],
        ),
        skin(
            "One Dark",
            // 正文 ABB2BF / panel 21252B / 窗底 282C34
            [0xAB, 0xB2, 0xBF],
            [0x38, 0x3C, 0x44],
            [0x21, 0x25, 0x2B],
            [0xE8, 0xEA, 0xED],
        ),
        skin(
            "One Light",
            // Atom 官方 light:正文 383A42 / 底 FAFAFA。
            // 这套本身是亮色板,暗色侧(text_dark)沿用 One Dark 的正文色
            // 保持与暗色主题搭配时不至于浅字浅底
            [0xAB, 0xB2, 0xBF],
            [0x38, 0x3C, 0x44],
            [0x34, 0x34, 0x3B],
            [0xE8, 0xE9, 0xEB],
        ),
        skin(
            "Rosé Pine",
            // 官方 base 191724 / text E0DEE4 / surface 1F1D2E
            [0xE0, 0xDE, 0xE4],
            [0x5B, 0x58, 0x71],
            [0x2A, 0x28, 0x37],
            [0xEC, 0xEB, 0xF1],
        ),
    ]
}

/// 预设的应用色板。旧安装的纯正文 RON 按原名补齐，不覆盖用户修改。
/// 每行依次是内容、导航、文字、次文字、强调色；状态色统一按表面混合。
pub fn shell_palette(name: &str) -> Option<crate::theme::ShellPalette> {
    let (light, dark) = match name {
        // 低饱和绿纸面与森林绿暗色；三平台共享，不依赖系统强调色。
        "青竹护眼" => (
            [0xdcebdc, 0xcbdccb, 0x263e2d, 0x485f4e, 0x2a6440],
            [0x1c2b23, 0x16231c, 0xd8e8db, 0xa4bdaa, 0x8ec7a0],
        ),
        "Dracula" => (
            [0xf8f8fa, 0xeeeef4, 0x282a36, 0x656579, 0x7952b3],
            [0x282a36, 0x21222c, 0xf8f8f2, 0xb3b1c6, 0xbd93f9],
        ),
        "Nord" => (
            [0xeceff4, 0xe1e6ee, 0x2e3440, 0x596579, 0x456780],
            [0x2e3440, 0x272d38, 0xe5e9f0, 0xa5b1c2, 0x88c0d0],
        ),
        "Gruvbox Dark" => (
            [0xfbf1c7, 0xf2e5bc, 0x3c3836, 0x71624d, 0x95601a],
            [0x282828, 0x202020, 0xebdbb2, 0xbdae93, 0xd8a657],
        ),
        "Solarized Dark" => (
            [0xfdf6e3, 0xeee8d5, 0x475b62, 0x56686e, 0x1b668d],
            [0x002b36, 0x00232c, 0xc1d1d1, 0x93a1a1, 0x62b9cd],
        ),
        "Solarized Light" => (
            [0xfdf6e3, 0xeee8d5, 0x475b62, 0x56686e, 0x006e69],
            [0x073642, 0x002b36, 0xc1d1d1, 0x93a1a1, 0x70c6bc],
        ),
        "Tokyo Night" => (
            [0xf0f2f8, 0xe1e6f0, 0x343b58, 0x55617c, 0x34548a],
            [0x1a1b26, 0x16161f, 0xc0caf5, 0x99a4ca, 0x7aa2f7],
        ),
        "One Dark" => (
            [0xf5f6f8, 0xe9ebef, 0x383a42, 0x646976, 0x326f9b],
            [0x282c34, 0x21252b, 0xd2d7e0, 0xabb2bf, 0x61afef],
        ),
        "One Light" => (
            [0xfafafa, 0xeeeeef, 0x383a42, 0x646570, 0x8250a4],
            [0x2b2b30, 0x232328, 0xe5e5eb, 0xb0afbc, 0xc5a1df],
        ),
        "Rosé Pine" => (
            [0xfaf4ed, 0xf2e9e1, 0x575279, 0x65607d, 0x9d4862],
            [0x191724, 0x14121f, 0xe0def4, 0xaaa5c4, 0xebbcba],
        ),
        _ => return None,
    };
    Some(crate::theme::ShellPalette {
        light: palette(light),
        dark: palette(dark),
    })
}

fn palette(values: [u32; 5]) -> crate::theme::ShellTokens {
    let [content, sidebar, text, secondary, accent] =
        values.map(|hex| Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8));
    let mix = |a: Color32, b: Color32, t: f32| {
        let c = |x: u8, y: u8| (f32::from(x) * (1.0 - t) + f32::from(y) * t).round() as u8;
        Color32::from_rgb(c(a.r(), b.r()), c(a.g(), b.g()), c(a.b(), b.b()))
    };
    crate::theme::ShellTokens {
        chrome: mix(sidebar, content, 0.45),
        rail: mix(sidebar, text, 0.035),
        content,
        sidebar,
        text,
        secondary,
        accent,
        hover: mix(sidebar, text, 0.08),
        selected_bg: mix(content, accent, 0.20),
        border: mix(sidebar, text, 0.16),
        code_bg: mix(content, sidebar, 0.6),
        faint: mix(content, text, 0.045),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_palettes_keep_text_readable_on_surfaces_and_selection() {
        fn luminance(c: Color32) -> f32 {
            let linear = |v: u8| {
                let v = f32::from(v) / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            linear(c.r()) * 0.2126 + linear(c.g()) * 0.7152 + linear(c.b()) * 0.0722
        }
        for skin in builtins() {
            let palette = shell_palette(skin.name).unwrap();
            for colors in [palette.light, palette.dark] {
                for (text, background) in [
                    (colors.text, colors.content),
                    (colors.text, colors.sidebar),
                    (colors.text, colors.selected_bg),
                    (colors.secondary, colors.content),
                    (colors.secondary, colors.sidebar),
                    (colors.accent, colors.content),
                ] {
                    let a = luminance(text);
                    let b = luminance(background);
                    let contrast = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                    assert!(
                        contrast >= 4.5,
                        "{}: {:?} on {:?}: {contrast}",
                        skin.name,
                        text,
                        background
                    );
                }
            }
        }
    }

    /// #23 F4/F5:标题排版新字段随十套预设出厂 —— `heading_space_above`
    /// 显式钉在 app 出厂值([`crate::theme::default_markdown_style`] 的
    /// 10.0,2026-10-30 设计审查自 vendored 出厂 4.0 提档;vendored 默认
    /// 未动,单独断言防漂移),`heading.scales` 走 vendored 新分级
    /// [2.0,1.55,1.30,1.15,1.08,1.0]。两边都交叉对照出厂正文样式:
    /// 十套皮肤与「无皮肤默认」在标题节奏上必须完全一致,否则切皮肤会
    /// 意外改变标题观感(preview-typography §2.2「注意 skinsPresets 也要
    /// 同步」的防线)。出厂值将来再变时,此测试红 = 强制一次显式的
    /// 三处同步(base()/default_markdown_style/本测试字面量)。
    #[test]
    fn builtins_ship_heading_typography_matching_factory() {
        let factory = crate::theme::default_markdown_style();
        let vendored = MarkdownStyle::default();
        assert_eq!(vendored.heading_space_above, 4.0);
        assert_eq!(factory.heading_space_above, 10.0);
        assert_eq!(vendored.heading.scales, [2.0, 1.55, 1.30, 1.15, 1.08, 1.0]);
        for skin in builtins() {
            assert_eq!(
                skin.style.heading_space_above, 10.0,
                "{}: 标题上方呼吸间距与出厂默认一致",
                skin.name
            );
            assert_eq!(
                skin.style.heading.scales,
                [2.0, 1.55, 1.30, 1.15, 1.08, 1.0],
                "{}: 标题字号分级与出厂默认一致",
                skin.name
            );
            assert_eq!(
                skin.style.heading_space_above, factory.heading_space_above,
                "{}: 与未选皮肤时的出厂样式一致",
                skin.name
            );
        }
    }

    /// #23 F5:铺盘落档的十套 `.ron` 里新字段真实存在且可读回 ——
    /// 「十套预设与新字段一致」不仅断言内存里的 `base()`,还钉
    /// 序列化落盘(皮肤文件里写有 `heading_space_above`)与皮肤扫描
    /// 载入(`SkinCatalog` 读回的生效样式仍带着新字段,老安装里
    /// 没有该字段的旧 ron 走 serde default 4.0 兜底,不在此路径)。
    #[test]
    fn installed_skins_carry_heading_typography_fields() {
        let dir =
            std::env::temp_dir().join(format!("latermd-presets-heading-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let written = install_to(&dir);
        assert_eq!(written.len(), 10, "首次全量铺盘");
        for skin in builtins() {
            let path = dir.join("themes").join(format!("{}.ron", skin.name));
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(
                text.contains("heading_space_above: 10.0"),
                "{}: 铺盘 ron 应显式写有 heading_space_above(内容: {text})",
                skin.name
            );
            assert!(
                // RON 把定长数组写成元组、1.30 省尾零;字段名与数值档位即可证明分级已落盘
                text.contains("scales: (2.0, 1.55, 1.3, 1.15, 1.08, 1.0)"),
                "{}: 铺盘 ron 应带 vendored 新标题分级(内容: {text})",
                skin.name
            );
        }

        let catalog = crate::theme::SkinCatalog::load_from(&dir);
        assert_eq!(catalog.skins.len(), 10, "十套都能被皮肤扫描载入");
        for skin in catalog.skins.iter() {
            assert_eq!(
                skin.shell,
                shell_palette(&skin.name),
                "{}: 完整色板经 RON 原样恢复",
                skin.name
            );
            assert_eq!(
                skin.style.heading_space_above, 10.0,
                "{}: 载入后新字段保持",
                skin.name
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #30 回归防线:十套预设的表格边框与底色可见(stroke_width ≥ 0.5 且
    /// 两底色开关开;被 reset 回 vendored 默认(0.0 / false × 2)时此测试红)。
    #[test]
    fn builtins_draw_table_borders() {
        for skin in builtins() {
            assert!(
                skin.style.table.stroke_width >= 0.5,
                "{}: stroke_width = {}",
                skin.name,
                skin.style.table.stroke_width
            );
            assert!(
                skin.style.table.header_fill,
                "{}: 表头行底色开启",
                skin.name
            );
            assert!(skin.style.table.zebra_fill, "{}: 隔行底色开启", skin.name);
        }
    }

    /// 十套预设,名字互不重复,行内代码四色全部不透明。
    #[test]
    fn builtins_with_distinct_names() {
        let skins = builtins();
        assert_eq!(
            skins.len(),
            10,
            "十套预设:Dracula/Nord/Gruvbox/Solarized×2/Tokyo Night/One×2/Rosé Pine/青竹护眼"
        );
        let mut names: Vec<&str> = skins.iter().map(|skin| skin.name).collect();
        names.sort();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "预设名互不重复");
        for skin in skins {
            for color in [
                skin.style.inline_code.color_dark,
                skin.style.inline_code.color_light,
                skin.style.inline_code.background_dark,
                skin.style.inline_code.background_light,
            ] {
                assert_eq!(color.a(), 255, "{}: 颜色不透明", skin.name);
            }
        }
    }

    /// 铺盘:首次写入十个 .ron;再次调用零写入(不覆盖用户改动);
    /// 写出的文件能被皮肤扫描(`SkinCatalog`,即 #8 既有路径)载入。
    #[test]
    fn install_writes_once_and_files_load_as_skins() {
        let dir = std::env::temp_dir().join(format!("latermd-presets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let written = install_to(&dir);
        assert_eq!(written.len(), 10, "首次全量铺盘");
        assert!(dir.join("themes").join("Nord.ron").exists());

        // 用户改过的同名文件必须被尊重:第二次铺盘零写入
        let edited = ron::ser::to_string_pretty(
            &MarkdownStyle {
                block_spacing: 23.0,
                ..MarkdownStyle::default()
            },
            ron::ser::PrettyConfig::default(),
        )
        .unwrap();
        std::fs::write(dir.join("themes").join("Nord.ron"), edited).unwrap();
        assert!(install_to(&dir).is_empty(), "已存在的文件不覆盖");

        let catalog = crate::theme::SkinCatalog::load_from(&dir);
        assert_eq!(catalog.skins.len(), 10, "十套都能被皮肤扫描载入");
        let nord = catalog.find("Nord").expect("Nord 在目录里");
        assert_eq!(nord.style.block_spacing, 23.0, "用户改动未被顶掉");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
