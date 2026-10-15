//! #50 编辑器排版验收(无头):M1 专用等宽族与混排基线 + M2 行距投影。
//!
//! M1 验收对象:`fonts::FAMILY_EDITOR_MONO`(`editor-mono` 族)——链头与
//! `FontFamily::Monospace` 同为内置 Hack(出厂行 metrics),链尾是 CJK
//! 等宽 face 的「反向 override 副本」(行 metrics 改写为链头同款 em 值,
//! #43 M2 手法反向:预览把链头对齐 CJK、行高随 CJK 变;编辑器把 CJK 对齐
//! 链头、纯 ASCII 行盒几何不变 —— 否决线要求链头 metrics 不可动)。
//! 字号仍走 #23 F3 的 `TextStyle::Monospace` 档投影(`theme::
//! apply_font_size`,按值键控 staleness),族由投影一并写入,源码
//! TextEdit / 行号槽 / Live 活动块经 `FontSelection::Style(Monospace)`
//! 自动跟随。
//!
//! M2 验收对象(#73 方案②转正,坤哥 2026-10-02「每行间距也太小」):
//! 行距滑杆经 `theme::apply_font_size` 投影成 `spacing.
//! extra_text_line_spacing`(TextEdit 行盒的绝对像素加值),行盒 = 自然
//! 行高 + extra = 字号 × 行距;调低(如 1.2)时被自然行高 clamp 到 0
//! (CJK 行盒物理下限,与预览侧 `min_line_height_em` 同语义)。
//!
//! M1 四组证据:
//! 1. 混排基线:同 galley 内 CJK 与拉丁字形的布局基线差 == 0;对照组
//!    (现状 `Monospace` 族)偏差 +1~+3px —— 量具对病灶敏感,不是恒真;
//! 2. 行盒高一致:纯拉丁 / 纯 CJK / 混排行的行盒高彼此相等,且与旧族
//!    完全相等(链头未动);
//! 3. 视觉尺寸:同一 CJK 探针字形的墨迹高在新旧族完全相等(scale 未动,
//!    大小观感的最终判据留真机目视);
//! 4. 否决线:纯 ASCII 文档 rows 数与行盒高对新族完全不变(①);预览侧
//!    专用族链与代码块共用的 `Monospace` 原生链零改动(②)。
//!
//! M2 证据见 `line_spacing_*` 与 `preview_side_is_untouched_by_line_
//! spacing_projection`:行距 1.2 不压缩行盒(CJK 不裁切)、行距 2.0 行盒
//! ≥ 字号×2.0−ε、滑杆拖动当帧生效、纯 ASCII 几何仅随行距**显式**变化
//! 且 M1 量具路径(直接 layout,TextFormat 无 line_height)逐像素不变、
//! vendored 预览链路(显式 LayoutJob)不吃 extra。
//!
//! 明暗两主题各跑一轮(字体链与主题无关,主题断言按任务书口径保留)。
//! 无 CJK 候选环境如实跳过(#43 口径);真机目视项:坤哥截图同款混排
//! 文档在源码页的基线/行距观感、行号槽/Live 活动块跟随、明暗两主题。

use std::sync::Arc;

use eframe::egui::{self, Color32, FontId, RawInput};

use crate::font_metrics_repro::is_cjk_ideograph;
use crate::fonts;
use crate::theme::{ThemeMode, ThemeSettings};

use latermd_editor::EditorBuffer;

/// 混排观测行:CJK 与拉丁字母、数字同行(源码页最常见形态)。
const MIXED_LINE: &str = "甲post中文123行Hn一";

/// 纯 ASCII 文档(否决线一):短行 + 超宽折行行 + 空行,覆盖行数与
/// 行盒高两个量。不得含任何非 ASCII 字符。
const ASCII_DOC: &str = concat!(
    "fn main() {\n",
    "    let greeting = \"hello, LaterMD editor typography!\";\n",
    "    println!(\"{greeting}\");\n",
    "}\n",
    "\n",
    "This long line is deliberately longer than the wrap width used by the test so that it ",
    "breaks into several visual rows, and the row count must match exactly between the two ",
    "families because every advance width comes from the identical chain head face.\n",
    "tail line",
);

/// 折行宽(否决线一的 wrap 输入;编辑器 TextEdit 同为 wrap 布局)。
const WRAP_W: f32 = 400.0;

/// #43 口径的无 CJK 环境检测:候选全失配时验收无混排对象,打印原因并
/// 跳过(不硬 panic 红门禁),与 preview_pixel/typography_acceptance 同款。
fn cjk_fonts_missing() -> bool {
    fonts::install(&egui::Context::default()).is_none()
}

/// 生产链路 context:装字体 + 出厂排版偏好投影(字号 15 走 `apply_font_size`,
/// 族随投影写入 `TextStyle::Monospace` 档)。`ctx.fonts` 须先过一帧
/// `run_ui` 才可用(egui 0.36 的 fonts 惰性初始化),与 fonts.rs 测试同款。
fn editor_ctx(dark: bool) -> egui::Context {
    let ctx = egui::Context::default();
    assert!(
        fonts::install(&ctx).is_some(),
        "editor_ctx 的调用方须先经 cjk_fonts_missing() 做无 CJK 跳过(#43 口径)"
    );
    ctx.run_ui(RawInput::default(), |_| {})
        .drop_without_applying_deltas();
    ThemeSettings::default().apply(
        &ctx,
        if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
    );
    ctx
}

/// 投影后的编辑器档 FontId(字号与族都从投影读 —— 生产 TextEdit 的取值
/// 路径:`TextStyle::Monospace` 档 + `FontSelection::Style` 解析)。
fn projected_editor_font(ctx: &egui::Context) -> FontId {
    ctx.style_of(ctx.theme())
        .text_styles
        .get(&egui::TextStyle::Monospace)
        .expect("出厂 Monospace 档恒存在")
        .clone()
}

/// 行内「CJK 基线 − 拉丁基线」(#43 M1 取证同式:基线 = placed.pos.y +
/// glyph.pos.y,已含 epaint 的整像素吸附,取各族字形的最大值作代表)。
fn baseline_cjk_minus_latin(galley: &egui::Galley) -> Option<f32> {
    let row = &galley.rows[0].row;
    let max_base = |pick: &dyn Fn(char) -> bool| -> Option<f32> {
        row.glyphs
            .iter()
            .filter(|g| pick(g.chr))
            .map(|g| g.pos.y)
            .fold(None::<f32>, |acc, v| Some(acc.map_or(v, |a| a.max(v))))
    };
    let cjk = max_base(&|c| is_cjk_ideograph(c))?;
    let latin = max_base(&|c| c.is_ascii_alphanumeric())?;
    Some(cjk - latin)
}

/// 指定字符的墨迹高(点;uv 尺寸即光栅化结果,同 face 同字号恒等)。
fn glyph_ink_height(galley: &egui::Galley, chr: char) -> Option<f32> {
    galley
        .rows
        .iter()
        .flat_map(|placed| placed.row.glyphs.iter())
        .find(|g| g.chr == chr)
        .map(|g| g.uv_rect.size.y)
}

fn layout_no_wrap(ctx: &egui::Context, text: &str, font: &FontId) -> Arc<egui::Galley> {
    ctx.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE))
}

fn layout_wrapped(ctx: &egui::Context, text: &str, font: &FontId) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = WRAP_W;
    job.append(
        text,
        0.0,
        egui::TextFormat::simple(font.clone(), Color32::WHITE),
    );
    ctx.fonts_mut(|f| f.layout_job(job))
}

/// 证据 1+2+3:混排基线偏差归零、行盒高一致且不偏离旧族、字形墨迹尺寸
/// 不被改动;全字号档(12-24,滑杆整数步进全域)与明暗两主题。
#[test]
fn mixed_line_baseline_row_height_and_ink_in_both_visuals() {
    if cjk_fonts_missing() {
        eprintln!("本机无 CJK 候选字体,编辑器混排验收无混排对象,跳过");
        return;
    }
    for dark in [true, false] {
        let theme_name = if dark { "暗色" } else { "亮色" };
        let ctx = editor_ctx(dark);
        let editor_font = projected_editor_font(&ctx);
        assert_eq!(
            editor_font.family,
            egui::FontFamily::Name(Arc::from(fonts::FAMILY_EDITOR_MONO)),
            "{theme_name}: 投影后编辑器档应为专用等宽族(#50 M1)"
        );
        assert_eq!(editor_font.size, crate::theme::EDITOR_FONT_SIZE_DEFAULT);

        // 对照组:换族前的 `Monospace`(链头同 face,病灶 witness)。
        let stock_font = FontId::new(editor_font.size, egui::FontFamily::Monospace);
        let new_galley = layout_no_wrap(&ctx, MIXED_LINE, &editor_font);
        let old_galley = layout_no_wrap(&ctx, MIXED_LINE, &stock_font);

        // ① 基线:新族精确归零;对照组必须呈现偏差(量具非恒真)。
        let old_dev = baseline_cjk_minus_latin(&old_galley).expect("混排行应含 CJK 与拉丁");
        let new_dev = baseline_cjk_minus_latin(&new_galley).expect("混排行应含 CJK 与拉丁");
        assert!(
            cfg!(target_os = "macos") || old_dev.abs() >= 1.0,
            "{theme_name}: 对照(现状 Monospace 族)应存在 ≥1px 基线偏差,实测 {old_dev}"
        );
        assert_eq!(
            new_dev, 0.0,
            "{theme_name}: 新族混排基线偏差应精确为 0(实测 {new_dev})"
        );

        // ② 行盒高:混排行与旧族完全相等(链头未动);纯拉丁/纯 CJK/
        //    混排三种行的行盒高彼此一致(行盒高一致,行内不跳变)。
        let row_h =
            |font: &FontId, text: &str| layout_no_wrap(&ctx, text, font).rows[0].rect().height();
        assert_eq!(
            new_galley.rows[0].rect().height(),
            old_galley.rows[0].rect().height(),
            "{theme_name}: 混排行行盒高不得偏离旧族"
        );
        let latin_h = row_h(&editor_font, "only latin 123");
        let cjk_h = row_h(&editor_font, "只有中文一行");
        assert_eq!(latin_h, cjk_h, "{theme_name}: 纯拉丁与纯 CJK 行盒高应一致");
        assert_eq!(
            new_galley.rows[0].rect().height(),
            latin_h,
            "{theme_name}: 混排行行盒高应与纯脚本行一致"
        );

        // ③ 视觉尺寸:同一字形墨迹高在新旧族完全相等 —— 字号观感由 face
        //    设计决定,本族不引入缩放(scale=1);CJK/拉丁的墨迹高比
        //    (Noto ideograph ≈0.88em vs Hack cap ≈0.72em)是 face 设计
        //    值,平衡观感留真机目视。
        let probe = |galley: &egui::Galley, chr: char| {
            glyph_ink_height(galley, chr)
                .unwrap_or_else(|| panic!("{theme_name}: {chr} 未被 shaping"))
        };
        assert_eq!(
            probe(&new_galley, '中'),
            probe(&old_galley, '中'),
            "{theme_name}: CJK 探针字形墨迹高不得被新族改动"
        );
        assert_eq!(
            probe(&new_galley, 'H'),
            probe(&old_galley, 'H'),
            "{theme_name}: 拉丁探针字形墨迹高不得被新族改动"
        );
        eprintln!(
            "[M1 {theme_name}] 基线偏差 旧{old_dev:+.3}px → 新 {new_dev:+.3}px;行盒 {:.2}px;墨迹高 中{:.2}px/H{:.2}px(比 {:.2})",
            new_galley.rows[0].rect().height(),
            probe(&new_galley, '中'),
            probe(&new_galley, 'H'),
            probe(&new_galley, '中') / probe(&new_galley, 'H'),
        );

        // ④ 全字号档:滑杆 12-24 整数步进全域,基线偏差恒为 0 ——
        //    副本表值是 em 改写,字号无关性由公式保证,这里逐档实证。
        for size in (12i16..=24).map(f32::from) {
            let font = FontId::new(size, editor_font.family.clone());
            let galley = layout_no_wrap(&ctx, MIXED_LINE, &font);
            assert_eq!(
                baseline_cjk_minus_latin(&galley),
                Some(0.0),
                "{theme_name}: {size}pt 档混排基线偏差应精确为 0"
            );
        }
    }
}

/// 否决线一:纯 ASCII 文档的 rows 数、逐行行盒高、总高对编辑器专用族
/// **完全不变**(链头 = 同一内置 Hack face,advance 与行 metrics 全等;
/// 折行位置因此逐行一致)。同时钉编辑器行高公式(`ui/editor.rs` 的
/// `row_height + extra_text_line_spacing`)在新族下不变。
#[test]
fn pure_ascii_layout_is_invariant_to_the_editor_family() {
    if cjk_fonts_missing() {
        eprintln!("本机无 CJK 候选字体,编辑器混排验收无混排对象,跳过");
        return;
    }
    assert!(
        ASCII_DOC.is_ascii(),
        "防御:否决线文档必须纯 ASCII(混入 CJK 会失去鉴别力)"
    );
    for dark in [true, false] {
        let theme_name = if dark { "暗色" } else { "亮色" };
        let ctx = editor_ctx(dark);
        let editor_family = projected_editor_font(&ctx).family;
        for size in [12.0, 15.0, 24.0] {
            let new_font = FontId::new(size, editor_family.clone());
            let old_font = FontId::new(size, egui::FontFamily::Monospace);
            let new_galley = layout_wrapped(&ctx, ASCII_DOC, &new_font);
            let old_galley = layout_wrapped(&ctx, ASCII_DOC, &old_font);

            assert_eq!(
                new_galley.rows.len(),
                old_galley.rows.len(),
                "{theme_name} {size}pt: 纯 ASCII 文档 rows 数改变"
            );
            for (idx, (new_row, old_row)) in new_galley
                .rows
                .iter()
                .zip(old_galley.rows.iter())
                .enumerate()
            {
                assert_eq!(
                    new_row.rect().height(),
                    old_row.rect().height(),
                    "{theme_name} {size}pt 第 {idx} 行: 行盒高改变"
                );
            }
            assert_eq!(
                new_galley.size().y,
                old_galley.size().y,
                "{theme_name} {size}pt: 纯 ASCII 文档总高改变"
            );

            // 编辑器行高公式(ui/editor.rs):desired_rows 的输入。
            let line_height = |font: &FontId| {
                ctx.fonts_mut(|f| f.row_height(font))
                    + ctx.style_of(ctx.theme()).spacing.extra_text_line_spacing
            };
            assert_eq!(
                line_height(&new_font),
                line_height(&old_font),
                "{theme_name} {size}pt: 编辑器行高公式输入改变"
            );
            eprintln!(
                "[M1 否决线一 {theme_name} {size}pt] 纯 ASCII:rows={} 总高={:.2}px 行高={:.2}px,新旧族全等",
                new_galley.rows.len(),
                new_galley.size().y,
                line_height(&new_font),
            );
        }
    }
}

/// 否决线二:预览侧零改动 —— 预览正文族仍是 #43 M2 的 `Inter-Preview`
/// 副本族且混排基线仍为 0;`bold` 别名族、代码块共用的 `Monospace` 原生
/// 链、以及预览链涉及的全部 font_data(无 tweak)分毫不动。编辑器族的
/// 引入不得外溢到预览渲染链路。
#[test]
fn preview_side_is_untouched_by_the_editor_family() {
    if cjk_fonts_missing() {
        eprintln!("本机无 CJK 候选字体,编辑器混排验收无混排对象,跳过");
        return;
    }
    let ctx = editor_ctx(true);
    assert_eq!(
        fonts::preview_body_family(&ctx),
        egui::FontFamily::Name(Arc::from(fonts::FAMILY_PREVIEW_BODY)),
        "预览正文族仍是 Inter-Preview 副本族"
    );
    ctx.fonts(|f| {
        let defs = f.definitions();
        // 预览正文族链:链头 override Inter 副本,链尾原生 CJK 比例 face
        // (键名为 fonts.rs 私有常量的字面值,改动时应同步本测试)。
        let preview =
            &defs.families[&egui::FontFamily::Name(Arc::from(fonts::FAMILY_PREVIEW_BODY))];
        assert_eq!(
            preview.first().map(String::as_str),
            Some("Inter-Regular-Preview")
        );
        assert_eq!(
            preview.last().map(String::as_str),
            Some("latermd-cjk-proportional")
        );
        // `bold` 别名族链头仍是 SemiBold 副本。
        let bold = &defs.families[&egui::FontFamily::Name(Arc::from(fonts::FAMILY_BOLD))];
        assert_eq!(
            bold.first().map(String::as_str),
            Some("Inter-SemiBold-Preview")
        );
        assert_eq!(
            bold.last().map(String::as_str),
            Some("latermd-cjk-proportional")
        );
        // 代码块(vendored `FontId::monospace` 硬编码)共用的原生等宽链:
        // 链头 Hack、链尾原生 CJK 等宽 —— 预览代码块渲染分毫不动。
        let mono = &defs.families[&egui::FontFamily::Monospace];
        assert_eq!(
            mono.first().map(String::as_str),
            Some(if cfg!(target_os = "macos") {
                "SF-Mono"
            } else {
                "Hack"
            })
        );
        assert_eq!(
            mono.last().map(String::as_str),
            Some("latermd-cjk-monospace")
        );
        assert!(
            !mono.iter().any(|n| n == "latermd-cjk-monospace-editor"),
            "原生等宽链不得混入编辑器副本"
        );
        // 预览链涉及的 font_data 一概无 tweak(编辑器侧改动不得外溢)。
        for key in [
            "Hack",
            "latermd-cjk-monospace",
            "latermd-cjk-proportional",
            "Inter-Regular-Preview",
            "Inter-SemiBold-Preview",
        ] {
            let mut tweak = defs.font_data[key].tweak.clone();
            // macOS uses the system variable font's real weight/optical-size
            // axes; no artificial glyph scaling or baseline shift is allowed.
            if cfg!(target_os = "macos") {
                tweak.coords = Default::default();
            }
            assert_eq!(
                tweak,
                egui::FontTweak::default(),
                "{key}: 预览链字体不得携带 tweak"
            );
        }
    });

    // 预览混排基线仍为 0(#43 M2 行为保留,编辑器族引入后不变)。
    let font = FontId::new(
        crate::theme::editor_font_size(&ctx),
        fonts::preview_body_family(&ctx),
    );
    let galley = layout_no_wrap(&ctx, MIXED_LINE, &font);
    assert_eq!(
        baseline_cjk_minus_latin(&galley),
        Some(0.0),
        "预览混排基线应保持 #43 M2 的归零状态"
    );
}

// ---------------------------------------------------------------------------
// #50 M2 行距投影(#73 方案②转正)
// ---------------------------------------------------------------------------

/// 滑杆拖动后渲染的文档:短行 + 折行行,覆盖行数与行盒高两个量。
const SPACING_DOC: &str = concat!(
    "fn main() {\n",
    "    let greeting = \"hello, LaterMD line spacing!\";\n",
    "    println!(\"{greeting}\");\n",
    "}\n",
    "\n",
    "This long line is deliberately longer than the wrap width so that it breaks into ",
    "several visual rows; wrapping depends on width only, so the row count must stay ",
    "constant across line-spacing changes.\n",
);

/// #50 M2 的生产链路 context(与 [`editor_ctx`] 的差别:行距投影已生效)。
/// 与生产 main 完全同构的两步 —— 装字体 →「启动装载」apply(首帧前,
/// fonts 未就绪,布防 fonts-ready 回调)→ 首帧 run_ui 闭包内 apply(=
/// 每帧 logic,fonts 已实例化、回调已置位,行距投影当帧补上)。
fn editor_ctx_with_line_spacing(ratio: f32, dark: bool) -> egui::Context {
    let settings = || ThemeSettings {
        line_height: ratio,
        ..ThemeSettings::default()
    };
    let mode = if dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    let ctx = egui::Context::default();
    assert!(
        fonts::install(&ctx).is_some(),
        "调用方须先经 cjk_fonts_missing() 做无 CJK 跳过(#43 口径)"
    );
    settings().apply(&ctx, mode);
    ctx.run_ui(RawInput::default(), |ui| {
        settings().apply(ui.ctx(), mode);
    })
    .drop_without_applying_deltas();
    ctx
}

/// 行距 1.2(合法域下限,任务书「投影可能为 0」档)下的 TextEdit 真实
/// 渲染:每行行盒 = 自然行高 + extra。CJK 不裁切的硬口径是行盒**不低于
/// 自然行高**(clamp ≥ 0 杜绝负 extra 的行间压缩),且行距投影不触碰
/// M1 的两个不变量:混排基线 == 0、CJK 墨迹高不改。
#[test]
fn line_spacing_1_2_keeps_cjk_rows_unclipped() {
    if cjk_fonts_missing() {
        eprintln!("本机无 CJK 候选字体,编辑器混排验收无混排对象,跳过");
        return;
    }
    for dark in [true, false] {
        let theme_name = if dark { "暗色" } else { "亮色" };
        let ctx = editor_ctx_with_line_spacing(1.2, dark);
        let font = projected_editor_font(&ctx);
        let size = font.size;
        let natural = ctx.fonts_mut(|f| f.row_height(&font));
        let extra = ctx.style_of(ctx.theme()).spacing.extra_text_line_spacing;
        assert_eq!(
            extra,
            (size * 1.2 - natural).max(0.0),
            "{theme_name}: 1.2 档投影值 = max(0, 字号×行距 − 自然行高)"
        );

        // TextEdit 真实渲染路径(与 ui/editor.rs 同款:Monospace 档 +
        // spacing.extra 经 egui builder.rs 折算进 galley):行盒不低于
        // 自然行高,CJK 字形的行高需求不被压缩。直接 layout 的 galley
        // 不吃 extra(见否决线测试),量行盒必须走 TextEdit。
        let galley = layout_no_wrap(&ctx, MIXED_LINE, &font);
        let (_, heights) = textedit_rows(&ctx, MIXED_LINE);
        let row_h = heights[0];
        assert!(
            row_h >= natural - 0.01,
            "{theme_name}: 1.2 档行盒 {row_h} 不得低于自然行高 {natural}(负 extra = 行间重叠)"
        );
        assert!(
            (row_h - (natural + extra)).abs() < 0.5,
            "{theme_name}: 行盒 {row_h} = 自然行高 {natural} + extra {extra}"
        );

        // M1 不变量在 M2 投影后原样成立:基线归零、墨迹高不改 —— 行距
        // 投影只动行盒高度,不碰字形布局与光栅化。墨迹对照取旧族(同
        // face,出厂 context 无行距投影)。
        assert_eq!(
            baseline_cjk_minus_latin(&galley),
            Some(0.0),
            "{theme_name}: M1 基线归零不受行距投影影响"
        );
        let stock_ctx = editor_ctx(true);
        let stock = FontId::new(size, egui::FontFamily::Monospace);
        assert_eq!(
            glyph_ink_height(&layout_no_wrap(&ctx, MIXED_LINE, &font), '中'),
            glyph_ink_height(&layout_no_wrap(&stock_ctx, MIXED_LINE, &stock), '中'),
            "{theme_name}: CJK 墨迹高与出厂 context(extra=0)完全一致"
        );
        eprintln!("[M2 1.2档 {theme_name}] natural={natural:.3} extra={extra:.3} 行盒={row_h:.3}");
    }
}

/// 行距 2.0(合法域上限):滑杆拖动**当帧生效** —— 1.2 档 context 上先
/// 渲染一帧取证,再不 run_ui 直接 apply(与拖动帧同构),下一次渲染即
/// 读到新行距;TextEdit 行盒 ≥ 字号 × 2.0 − ε(ε 只兜 epaint 的
/// 0.25pt 整像素吸附);行盒差精确等于 extra 差,rows 数不随行距变
/// (折行只由宽度决定)。
#[test]
fn line_spacing_2_0_row_boxes_meet_two_em_after_same_frame_drag() {
    if cjk_fonts_missing() {
        eprintln!("本机无 CJK 候选字体,编辑器混排验收无混排对象,跳过");
        return;
    }
    for dark in [true, false] {
        let theme_name = if dark { "暗色" } else { "亮色" };
        let mode = if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        };
        let ctx = editor_ctx_with_line_spacing(1.2, dark);
        let size = projected_editor_font(&ctx).size;

        // 1.2 档取证(投影已生效)
        let (rows_low, heights_low) = textedit_rows(&ctx, SPACING_DOC);
        let extra_low = ctx.style_of(ctx.theme()).spacing.extra_text_line_spacing;
        for (idx, height) in heights_low.iter().enumerate() {
            assert!(
                *height >= (size * 1.2).max(heights_low[0] - extra_low) - 0.5,
                "{theme_name} 第 {idx} 行: 1.2 档行盒 {height} 不低于自然行高(行间不压缩)"
            );
        }

        // 拖动到 2.0:不 run_ui,当帧重投影(#23 F3 键控手法)
        ThemeSettings {
            line_height: 2.0,
            ..ThemeSettings::default()
        }
        .apply(&ctx, mode);

        // 下一帧渲染即读到新行距 —— 「拖动当帧生效」的渲染侧口径
        let (rows_high, heights_high) = textedit_rows(&ctx, SPACING_DOC);
        let extra_high = ctx.style_of(ctx.theme()).spacing.extra_text_line_spacing;

        // 行盒 ≥ 字号×2.0−ε:行盒 = 自然行高 + extra = 字号×行距(恒等式)
        for (idx, height) in heights_high.iter().enumerate() {
            assert!(
                *height >= size * 2.0 - 0.5,
                "{theme_name} 第 {idx} 行: 行盒 {height} 不得低于 字号×2.0−ε(= {})",
                size * 2.0 - 0.5
            );
        }

        // 纯 ASCII 几何仅随行距显式变化:rows 不变(折行只随宽度),
        // 逐行行盒差 == extra 差(15pt 全档恰为 0.8×字号)
        assert_eq!(rows_high, rows_low, "{theme_name}: rows 数不随行距变");
        let delta_extra = extra_high - extra_low;
        assert!(
            delta_extra > 0.0,
            "{theme_name}: 2.0 与 1.2 档投影必有差(实测 {delta_extra})"
        );
        for (idx, (high, low)) in heights_high.iter().zip(heights_low.iter()).enumerate() {
            assert!(
                (high - low - delta_extra).abs() < 0.5,
                "{theme_name} 第 {idx} 行: 行盒差 {} 应等于 extra 差 {delta_extra}",
                high - low
            );
        }
        eprintln!(
            "[M2 2.0档 {theme_name}] rows={rows_low} extra 1.2={extra_low:.3} 2.0={extra_high:.3} 行盒 {:.3}",
            heights_high[0]
        );
    }
}

/// TextEdit 真实渲染一帧(与 ui/editor.rs 同款输入:Monospace 档、wrap),
/// 返回 (visual rows 数, 逐行行盒高)。`ctx` 的行距投影须已生效。
fn textedit_rows(ctx: &egui::Context, text: &str) -> (usize, Vec<f32>) {
    let mut buffer = text.to_owned();
    let mut rows = 0usize;
    let mut heights: Vec<f32> = Vec::new();
    ctx.run_ui(RawInput::default(), |ui| {
        let output = egui::TextEdit::multiline(&mut buffer)
            .font(egui::TextStyle::Monospace)
            .desired_width(400.0)
            .show(ui);
        rows = output.galley.rows.len();
        heights = output
            .galley
            .rows
            .iter()
            .map(|row| row.rect().height())
            .collect();
    })
    .drop_without_applying_deltas();
    (rows, heights)
}

/// 否决线(M2):行距投影不得外溢 ——
/// ① M1 量具路径(TextFormat 无 line_height 的直接 layout)在投影前后
///    逐像素不变:M1 的基线/字号/行盒断言不受 M2 影响;
/// ② vendored 预览链路(显式 `TextFormat::line_height` 的 LayoutJob,
///    即正文/代码块的 `fonts_mut(layout_job)` 路径)不读
///    `spacing.extra_text_line_spacing`:同一 job 在 extra=0 与大 extra
///    的 context 下 galley 全等;
/// ③ 行距覆盖链路(`line_height_ratio`)与 extra 分属两个 style 槽,
///    投影互不干扰;字体定义分毫不动。
#[test]
fn preview_side_is_untouched_by_line_spacing_projection() {
    if cjk_fonts_missing() {
        eprintln!("本机无 CJK 候选字体,编辑器混排验收无混排对象,跳过");
        return;
    }
    let projected = editor_ctx_with_line_spacing(2.0, true);
    let extra = projected
        .style_of(projected.theme())
        .spacing
        .extra_text_line_spacing;
    assert!(extra > 0.0, "防御:投影 context 应带正 extra(实测 {extra})");

    // 对照:M1 的 editor_ctx 只投影字号(apply 发生在首帧前,fonts 未
    // 就绪,行距投影按设计跳过)= 出厂 spacing.extra 0;行距覆盖(ratio)
    // 补到与投影 context 同值,使两个 context 唯一的差异就是 extra。
    let plain = editor_ctx(true);
    ThemeSettings {
        line_height: 2.0,
        ..ThemeSettings::default()
    }
    .apply(&plain, ThemeMode::Dark);
    let plain_extra = plain
        .style_of(plain.theme())
        .spacing
        .extra_text_line_spacing;
    assert_eq!(plain_extra, 0.0, "对照 context 不带行距投影");

    // ① M1 量具路径不变:extra=0 与大 extra 下同一 galley 逐行全等。
    let font_of = |ctx: &egui::Context| FontId::new(15.0, projected_editor_font(ctx).family);
    for text in [MIXED_LINE, ASCII_DOC] {
        let left = layout_no_wrap(&projected, text, &font_of(&projected));
        let right = layout_no_wrap(&plain, text, &font_of(&plain));
        assert_eq!(
            left.rows
                .iter()
                .map(|row| row.rect().height())
                .collect::<Vec<_>>(),
            right
                .rows
                .iter()
                .map(|row| row.rect().height())
                .collect::<Vec<_>>(),
            "直接 layout 路径(TextFormat 无 line_height)不吃 extra:{text:?} 行盒应全等"
        );
    }

    // ② vendored 同款路径:显式 line_height 的 LayoutJob(vendored
    //    `line_height_for` 产出的形态,size×ratio)不受 extra 影响。
    let vendored_job = |ctx: &egui::Context| {
        let ratio = egui_markdown_style::global_style(ctx).line_height_ratio;
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = WRAP_W;
        let mut format = egui::TextFormat::simple(font_of(ctx), Color32::WHITE);
        format.line_height = Some(15.0 * ratio);
        job.append(MIXED_LINE, 0.0, format);
        job
    };
    let left = ctx_layout(&projected, vendored_job(&projected));
    let right = ctx_layout(&plain, vendored_job(&plain));
    assert_eq!(
        left.size(),
        right.size(),
        "显式 line_height 的 LayoutJob(vendored 预览路径)不吃 extra"
    );

    // ③ 行距覆盖链路:预览正文 ratio 恒等于用户偏好,extra 投影不触碰
    //    markdown style 槽;字体链头不被行距投影改动。
    assert_eq!(
        egui_markdown_style::global_style(&projected).line_height_ratio,
        2.0,
        "预览侧行距覆盖仍是用户滑杆值"
    );
    assert_eq!(
        egui_markdown_style::global_style(&projected).min_line_height_em,
        egui_markdown_style::global_style(&plain).min_line_height_em,
        "CJK 行高下限注入不受 extra 投影影响"
    );
    projected.fonts(|f| {
        assert_eq!(
            f.definitions().families
                [&egui::FontFamily::Name(std::sync::Arc::from(fonts::FAMILY_EDITOR_MONO))]
                .first()
                .map(String::as_str),
            Some(if cfg!(target_os = "macos") {
                "SF-Mono"
            } else {
                "Hack"
            }),
            "字体链不被行距投影改动"
        );
    });
}

/// 在指定 context 上布局一个 job(测试局部助手)。
fn ctx_layout(ctx: &egui::Context, job: egui::text::LayoutJob) -> std::sync::Arc<egui::Galley> {
    ctx.fonts_mut(|f| f.layout_job(job))
}

/// 行距投影下的生产编辑器面板(`ui::editor::ui` 完整路径,含行号槽):
/// 行号数字中心 y 与 galley 各逻辑行首 visual row 中心对齐(不错位),
/// 编辑器行高公式(`row_height + extra`,desired_rows 的输入)与
/// TextEdit 实际渲染一致 —— 行距拉大后行号跟着走,不裁切、不错位。
#[test]
fn gutter_digits_stay_aligned_under_line_spacing_projection() {
    if cjk_fonts_missing() {
        eprintln!("本机无 CJK 候选字体,编辑器混排验收无混排对象,跳过");
        return;
    }
    let ctx = editor_ctx_with_line_spacing(2.0, true);
    let extra = ctx.style_of(ctx.theme()).spacing.extra_text_line_spacing;
    assert!(extra > 0.0);

    let text = ["混排行距甲"; 10].join("\n");
    let mut editor = EditorBuffer::new(&text);
    let mut preview = crate::state::PreviewState::new(&editor);
    let mut cursor = crate::state::OutlineCursor::default();
    let mut live = crate::live::LiveState::default();
    let mut selection = None;
    let mut pending = None;
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
    let output = ctx.run_ui(
        RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        },
        |ui| {
            crate::ui::editor::ui(
                ui,
                &mut editor,
                &mut preview,
                &crate::ai::AiState::default(),
                crate::ui::editor::CursorChannel {
                    cursor: &mut cursor,
                    selection: &mut selection,
                    pending: &mut pending,
                },
                &mut live,
                crate::live::RenderMode::Source,
                crate::ui::editor::tab_editor_id(1),
                // 排版验收不涉 minimap/打字机/专注:都关(现状路径)。
                false,
                false,
                false,
                None,
                false,
                &crate::keymap::Keymap::builtin(),
                &mut Vec::new(),
                &[],
            );
        },
    );
    let shapes = output.shapes.clone();
    output.drop_without_applying_deltas();

    // 行号数字 shape(纯 ASCII 数字 galley;测试文档不含 ASCII 数字,
    // 纯数字形状唯一来源就是行号槽)与编辑器正文 galley。
    let mut digits: Vec<(String, egui::Rect)> = Vec::new();
    let mut body: Option<&egui::epaint::TextShape> = None;
    for clipped in &shapes {
        if let egui::Shape::Text(t) = &clipped.shape {
            let s = t.galley.text();
            if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
                digits.push((
                    s.to_owned(),
                    egui::Rect::from_min_size(t.pos, t.galley.size()),
                ));
            } else if s.contains("混排行距甲") {
                body = Some(t);
            }
        }
    }
    let body = body.expect("编辑器正文 galley 已绘制");
    assert_eq!(body.galley.rows.len(), 10, "短行不折行,visual == 逻辑");
    digits.sort_by(|a, b| a.1.top().total_cmp(&b.1.top()));
    assert_eq!(digits.len(), 10, "10 行全部可见");
    for (idx, (label, rect)) in digits.iter().enumerate() {
        assert_eq!(label, &(idx + 1).to_string(), "行号连续");
        let row = body.galley.rows[idx].rect();
        let expected = body.pos.y + row.center().y;
        assert!(
            (rect.center().y - expected).abs() < 0.51,
            "第 {} 行号中心 {} 与 galley 行首中心 {expected} 对齐(行盒 {:.2} 含 extra {:.2})",
            idx + 1,
            rect.center().y,
            row.height(),
            extra
        );
    }
}
