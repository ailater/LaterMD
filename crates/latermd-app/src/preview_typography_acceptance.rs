//! #23 F5 预览排版验收(无头):标题分级 + 标题呼吸间距 + 正文观感不变,
//! 明暗两套主题下的**自动像素取样**。
//!
//! 验收对象(F4 vendored + F5 app 接线):`HeadingStyle::scales` 新分级
//! [2.0,1.55,1.30,1.15,1.08,1.0]、`MarkdownStyle::heading_space_above`
//! 出厂 4.0(vendored 默认与九套预设同步,`theme_presets::base()`)。与
//! vendored 侧 `tests/heading_spacing.rs`(vendored 默认样式、13pt、无
//! CJK 下限)不同,这里跑**生产生效链路**:`ThemeSettings::default().apply`
//! 装的全局样式(用户行距默认 1.5 覆盖 + 本机 CJK 行高下限注入)+ 预览
//! 专用字体 15pt(中英混排基线对齐过的 Inter-Preview 副本),即用户
//! 出厂看到的排版。
//!
//! 两组证据、两条渲染路径:
//! 1. **排版文档**(H1-H6 + 正文 + 列表,整篇 galley 路径)—— galley 层
//!    精确数值(标题阶梯/spacer 行高/呼吸差值/H1 行盒随字号重算)+ #43
//!    同款逐字形墨迹中心像素完整性;
//! 2. **全元素文档**(H1-H6 + 列表 + 引用 + 表格 + 代码块,分段渲染
//!    路径)—— 光栅化最终色图层的带结构分析:strong 色标题带阶梯、
//!    标题带上方空隙 > 正文带上方空隙且差值 ≈ spacer、引用竖条/表格
//!    横边框/代码块底色大矩形在像素层全部可检出。
//!
//! 「好不好看」(H4-H6 肉眼可辨、呼吸感、正文密度)的最终判据是眼睛,
//! 无头断言只覆盖几何与可见性;真机目视项清单见
//! `docs/preview-typography-acceptance.md`。

use eframe::egui::{self, Color32, RawInput, Rect, UiBuilder, Vec2};
use egui::epaint::text::PlacedRow;
use egui::epaint::Mesh;
use egui_markdown_style::MarkdownStyle;

use crate::font_metrics_repro::is_cjk_ideograph;
use crate::fonts;
use crate::theme::{ThemeMode, ThemeSettings};

use crate::preview_pixel_acceptance::{
    color_dist, final_covered_color, glyph_ink_rect, point_in_tri,
};

/// 验收文档(排版部分):正文段与 H1-H6 交错(每个标题都是文档中段
/// 标题 → 各得一条 spacer 行),列表与超长 wrap 段压尾。
const TYPOGRAPHY_DOC: &str = concat!(
    "开场正文段落,标题分级验收观测。\n\n",
    "# 标题分级一\n\n第一段正文,标题分级观测。\n\n",
    "## 标题分级二\n\n第二段正文,标题分级观测。\n\n",
    "### 标题分级三\n\n第三段正文,标题分级观测。\n\n",
    "#### 标题分级四\n\n第四段正文,标题分级观测。\n\n",
    "##### 标题分级五\n\n第五段正文,标题分级观测。\n\n",
    "###### 标题分级六\n\n第六段正文,标题分级观测。\n\n",
    "- 无序列表项甲\n- 无序列表项乙\n\n",
    "收尾正文段落中文中文中文中文中文中文中文中文中文中文中文中文中文中文",
    "中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文",
    "中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文",
);

/// 验收文档(全元素部分):顶部 H1-H6 与正文交错(交替段里的 spacer 行为
/// 走分段 flush),列表/引用/表格/代码块依次压尾 —— 引用/表格把文档切进
/// 分段渲染路径,引用竖条/表格边框/代码块底色在像素层各有签名。正文与
/// 列表不使用加粗(加粗与标题同为 strong 色,会污染标题带识别)。
const FULL_DOC: &str = concat!(
    "开场正文段落,标题分级验收观测。\n\n",
    "# 标题分级一\n\n第一段正文,标题分级观测。\n\n",
    "## 标题分级二\n\n第二段正文,标题分级观测。\n\n",
    "### 标题分级三\n\n第三段正文,标题分级观测。\n\n",
    "#### 标题分级四\n\n第四段正文,标题分级观测。\n\n",
    "##### 标题分级五\n\n第五段正文,标题分级观测。\n\n",
    "###### 标题分级六\n\n第六段正文,标题分级观测。\n\n",
    "- 无序列表项甲\n- 无序列表项乙\n\n",
    "> 引用块一行文字,交代引用来源。\n\n",
    "| 列一 | 列二 | 列三 |\n|---|---|---|\n",
    "| 数据甲 | 数值一 | 备注一 |\n",
    "| 数据乙 | 数值二 | 备注二 |\n\n",
    "```rust\nfn main() {\n    println!(\"你好,世界\");\n}\n```\n\n",
    "文末正文段落,标题分级收尾。",
);

/// 探针字形集(标题分级):见 [`row_probe_ink_max`],文案与字形集必须
/// 同步演进(锚词行都含这四个字)。
const PROBE_CHARS: [char; 4] = ['标', '题', '分', '级'];

/// 每级标题在 galley 行文本中的锚词(唯一、纯 CJK,便于 ink 高测量)。
const HEADING_ANCHORS: [&str; 6] = [
    "标题分级一",
    "标题分级二",
    "标题分级三",
    "标题分级四",
    "标题分级五",
    "标题分级六",
];

/// 段落间空隙的测量锚词:H1-H6 后的第一段~第六段(标题后空行)、列表
/// 首项(正文段后空行)、收尾首行(列表后空行)—— 覆盖三种前驱的段落
/// 空隙,「空行行高一致」由实测均匀性断言兜底。
const PARAGRAPH_ANCHORS: [&str; 8] = [
    "第一段正文",
    "第二段正文",
    "第三段正文",
    "第四段正文",
    "第五段正文",
    "第六段正文",
    "无序列表项甲",
    "收尾正文段落",
];

/// 出厂基准字号(pt,`ThemeSettings::default().editor_font_size`)。
const BODY_SIZE: f32 = 15.0;
/// 验收面板宽(与 #43 同款):wrap 与分段判定都依赖该宽度。
const PANEL_W: f32 = 700.0;

// ---------------------------------------------------------------------------
// 渲染基建:生产链路 + headless
// ---------------------------------------------------------------------------

/// 一帧渲染里的断言用色(全部从**当帧实际 visuals** 回读 —— app 的
/// shell token 投影会改 visuals,不能用 egui 出厂默认值顶替)。
#[derive(Clone, Copy, Debug)]
struct DocColors {
    text: Color32,
    strong: Color32,
    panel: Color32,
    border: Color32,
    code_bg: Color32,
}

/// 按生产链路渲染一帧:装出厂字体(预览专用族)+ `ThemeSettings::default()
/// .apply`(用户行距 1.5 覆盖 + CJK 行高下限注入)→ probe 回调里摆
/// `MarkdownLabel`(生产入口 `live::ui` 同款:`editor_id.with(("live-render",
/// index))` + 显式 FontId(用户字号 + 预览族)+ wrap,样式吃 context 全局槽)。
/// probe 拿到面板与验收区矩形,
/// 自行创建 child;返回 probe 的捕获值、实际 visuals 断言用色、生效样式、
/// 网格与 CJK 行高下限。
/// #43 口径的无 CJK 环境检测:像素/几何验收没有混排对象,失败不代表
/// 回归 —— 无头 CI 的 ubuntu runner 不预装 fonts-noto-cjk,测试入口检测
/// 到缺失时打印原因并 `return` 跳过(不硬 panic 红门禁),同
/// `preview_pixel_acceptance` 的既有口径。
fn cjk_fonts_missing() -> bool {
    fonts::install(&egui::Context::default()).is_none()
}

fn render_headless<T>(
    dark: bool,
    id: &'static str,
    panel_h: f32,
    probe: impl FnOnce(&mut egui::Ui, egui::FontId, Rect, egui::Id) -> T,
) -> (
    T,
    DocColors,
    std::sync::Arc<MarkdownStyle>,
    Vec<egui::epaint::ClippedPrimitive>,
    Option<f32>,
) {
    let ctx = egui::Context::default();
    assert!(
        fonts::install(&ctx).is_some(),
        "render_headless 的调用方须先经 cjk_fonts_missing() 做无 CJK 跳过(#43 口径)"
    );
    let mode = if dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    ThemeSettings::default().apply(&ctx, mode);
    let style = egui_markdown_style::global_style(&ctx);
    // 关抗锯齿羽化(#41 同款):边缘三角形的透明渐变会污染采样读色。
    ctx.options_mut(|o| o.tessellation_options.feathering = false);
    let font = egui::FontId::new(BODY_SIZE, fonts::preview_body_family(&ctx));
    let floor = fonts::line_height_floor_em(&ctx);

    let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(PANEL_W, panel_h));
    let mut colors = DocColors {
        text: Color32::BLACK,
        strong: Color32::BLACK,
        panel: Color32::WHITE,
        border: Color32::BLACK,
        code_bg: Color32::WHITE,
    };
    let mut captured = None;
    let mut probe_slot = Some(probe);
    // screen_rect 显式给全验收区:RawInput::default() 无屏幕矩形时根 clip 的
    // 回退不足以覆盖 1600px 高的全元素文档,引用/表格/代码块会整块被视口
    // 剔除(实测内容到 y≈736 即断,vendored 侧 label_height 探针同款显式传)。
    let raw = RawInput {
        screen_rect: Some(screen),
        ..RawInput::default()
    };
    let mut output = ctx.run_ui(raw, |panel| {
        colors = DocColors {
            text: panel.visuals().text_color(),
            strong: panel.visuals().strong_text_color(),
            panel: panel.visuals().panel_fill,
            border: panel.visuals().widgets.noninteractive.bg_stroke.color,
            code_bg: panel.visuals().code_bg_color,
        };
        // run_ui 收 FnMut,probe 是 FnOnce —— Option::take 转交所有权
        captured = probe_slot
            .take()
            .map(|probe| probe(panel, font.clone(), screen, egui::Id::new(id)));
    });
    let primitives = ctx.tessellate(std::mem::take(&mut output.shapes), 1.0);
    output.drop_without_applying_deltas();
    (
        captured.expect("probe 回调应产出捕获值"),
        colors,
        style,
        primitives,
        floor,
    )
}

/// 网格引用(断言用色的采样面)。
fn mesh_refs(primitives: &[egui::epaint::ClippedPrimitive]) -> Vec<&Mesh> {
    primitives
        .iter()
        .filter_map(|cp| match &cp.primitive {
            egui::epaint::Primitive::Mesh(mesh) => Some(mesh),
            _ => None,
        })
        .collect()
}

/// 共同探针字形集:标题行与正文行**共用同一组满高表意字形**,行间 ink 高
/// 比值才只反映字号差。实测教训(Noto Sans CJK):各字形的 em 覆盖率不同
/// (「级/标」≈0.99em、「文」≈0.99em、「题/甲」≈0.93em、「一」是横笔画
/// ≈0.1em),拿**不同字形集**的两行直接比,比值会被 em 覆盖率差污染到
/// ±0.15(H1 曾测得 1.87 而非 2.0);同字形集则只差光栅化取整(±1px)。
fn row_probe_ink_max(placed: &PlacedRow) -> f32 {
    let mut heights: Vec<f32> = placed
        .row
        .glyphs
        .iter()
        .filter(|g| PROBE_CHARS.contains(&g.chr))
        .filter_map(|g| glyph_ink_rect(placed.pos, g))
        .map(|(_, size)| size.y)
        .collect();
    heights.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!(
        !heights.is_empty(),
        "行 {:?} 应含探针字形 {:?}(标题/正文锚词文案改动需同步 PROBE_CHARS)",
        placed.row.text(),
        PROBE_CHARS
    );
    heights[heights.len() - 1]
}

// ---------------------------------------------------------------------------
// 光栅化:三角形扫描线 → 最终色图层(画家算法,feathering 关)
// ---------------------------------------------------------------------------

/// 把全部网格光栅化进一个最终色图层(绘制序,后画覆盖先画)。feathering
/// 关闭后三角形即硬边,同像素内颜色由顶点色唯一决定 —— 与
/// `final_covered_color` 同款语义,但一次性产出整幅图层,供带结构分析。
fn rasterize_final_colors(meshes: &[&Mesh], width: usize, height: usize) -> Vec<Color32> {
    let mut buf = vec![Color32::TRANSPARENT; width * height];
    for mesh in meshes {
        for tri in mesh.indices.as_chunks::<3>().0 {
            let v = |i: u32| mesh.vertices[i as usize].pos;
            let (a, b, c) = (v(tri[0]), v(tri[1]), v(tri[2]));
            let color = mesh.vertices[tri[0] as usize].color;
            // x/y 各自按自己的轴长钳 —— 复用同一个闭包会把 y 钳到 width-1
            // (本机实测 700px,全元素文档 736px 以下整片被压进同一行)。
            let clamp_x = |x: f32, hi: bool| {
                let v = if hi { x.ceil() } else { x.floor() };
                v.clamp(0.0, width as f32 - 1.0) as usize
            };
            let clamp_y = |y: f32, hi: bool| {
                let v = if hi { y.ceil() } else { y.floor() };
                v.clamp(0.0, height as f32 - 1.0) as usize
            };
            let x0 = clamp_x(a.x.min(b.x).min(c.x), false);
            let x1 = clamp_x(a.x.max(b.x).max(c.x), true);
            let y0 = clamp_y(a.y.min(b.y).min(c.y), false);
            let y1 = clamp_y(a.y.max(b.y).max(c.y), true);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    if point_in_tri(egui::pos2(x as f32 + 0.5, y as f32 + 0.5), a, b, c) {
                        buf[y * width + x] = color;
                    }
                }
            }
        }
    }
    buf
}

/// 带结构:y 轴上的连续命中带(`[top, bottom]` 像素闭区间,按 y 升序)。
fn bands_from_rows(hits: &[bool]) -> Vec<(usize, usize)> {
    let mut bands = Vec::new();
    let mut start: Option<usize> = None;
    for (y, &hit) in hits.iter().enumerate() {
        match (hit, start) {
            (true, None) => start = Some(y),
            (false, Some(s)) => {
                bands.push((s, y - 1));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        bands.push((s, hits.len() - 1));
    }
    bands
}

// ---------------------------------------------------------------------------
// 证据 1:排版文档(整篇 galley 路径)—— 精确几何 + 字形像素完整性
// ---------------------------------------------------------------------------

#[test]
fn typography_doc_geometry_matches_shipped_style_in_both_visuals() {
    if cjk_fonts_missing() {
        eprintln!("本机无 CJK 候选字体,排版验收无混排对象,跳过");
        return;
    }
    for dark in [true, false] {
        let theme_name = if dark { "暗色" } else { "亮色" };
        // 同帧两个同 id 同 wrap 宽的 child:前者取 galley(几何),后者
        // show()(网格)。共用 vendored 布局缓存,几何必然一致(#43 同款);
        // id 绝不含内容长度/hash(AGENTS §6.7)。
        let (galley, colors, style, primitives, floor) = render_headless(
            dark,
            "f5-typography-acceptance",
            1200.0,
            |panel, font, screen, label| {
                let mut layout_child = panel.new_child(UiBuilder::new().max_rect(screen));
                let (_, galley, _) = egui_markdown::MarkdownLabel::new(label, TYPOGRAPHY_DOC)
                    .font(font.clone())
                    .wrap()
                    .layout_in_ui(&mut layout_child);
                let mut paint_child = panel.new_child(UiBuilder::new().max_rect(screen));
                egui_markdown::MarkdownLabel::new(label, TYPOGRAPHY_DOC)
                    .font(font)
                    .wrap()
                    .show(&mut paint_child);
                galley
            },
        );
        let meshes = mesh_refs(&primitives);

        let scales = style.heading.scales;
        let spacer_h = style.block_spacing + style.heading_space_above;
        assert_eq!(
            spacer_h, 12.0,
            "出厂 spacer = block_spacing(8)+heading_space_above(4)"
        );
        assert_eq!(scales[0], 2.0, "出厂 H1 分级 2.0(F4 vendored 新默认)");

        let heading_rows: Vec<usize> = HEADING_ANCHORS
            .iter()
            .map(|anchor| {
                galley
                    .rows
                    .iter()
                    .enumerate()
                    .find(|(_, row)| row.row.text().contains(anchor))
                    .unwrap_or_else(|| panic!("{theme_name} 主题:找不到标题行 {anchor}"))
                    .0
            })
            .collect();

        // ① spacer 行:每个中段标题恰一条,行高精确等于
        //    `block_spacing + heading_space_above`(F4 的核心机制)。
        let spacer_rows: Vec<usize> = galley
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.row.text() == " ")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            spacer_rows.len(),
            HEADING_ANCHORS.len(),
            "{theme_name} 主题:6 个中段标题应各有一条 spacer 行"
        );
        for &i in &spacer_rows {
            let h = galley.rows[i].max_y() - galley.rows[i].min_y();
            assert!(
                (h - spacer_h).abs() < 0.01,
                "{theme_name} 主题:spacer 行高 {h:.2} 应精确等于 {spacer_h:.2}"
            );
        }

        // ② 标题阶梯:各级标题行 CJK 字形 ink 高 ≈ 正文行 × 对应 scale。
        //    另按 section 探针实测:布局层的字号精确为 15×scale(H1 30.00pt/
        //    行高 45.00),这里从**字形墨迹**侧再量一遍,两条证据互为印证。
        let body_rows: Vec<usize> = PARAGRAPH_ANCHORS
            .iter()
            .map(|anchor| {
                galley
                    .rows
                    .iter()
                    .enumerate()
                    .find(|(_, row)| row.row.text().contains(anchor))
                    .unwrap_or_else(|| panic!("{theme_name} 主题:找不到正文行 {anchor}"))
                    .0
            })
            .collect();
        // ②a 阶梯 —— **布局层精确断言**:各级标题所在 section 的字号
        //    == 基准 × 分级(±0.01,这是渲染的输入,无量化噪声);
        //    行高 == max(字号×行距倍率, 字号×CJK 下限+0.75)。
        let floor_em = floor.unwrap_or(1.0).max(1.0);
        let body_ref = row_probe_ink_max(&galley.rows[body_rows[0]]);
        eprintln!(
            "[F5 {theme_name}] 正文探针字形 ink 高 {:.2}px(「标题分级」),spacer={spacer_h}",
            body_ref
        );
        let section_text = |si: usize| -> String {
            eframe::epaint::text::ByteRangeExt::slice(
                &galley.job.sections[si].byte_range,
                &galley.job.text,
            )
            .to_owned()
        };
        for (level, anchor) in HEADING_ANCHORS.iter().enumerate() {
            let si = galley
                .job
                .sections
                .iter()
                .enumerate()
                .find(|(si, _)| section_text(*si).contains(anchor))
                .unwrap_or_else(|| {
                    panic!(
                        "{theme_name} 主题:找不到标题 section {anchor}(sections: {})",
                        galley.job.sections.len()
                    )
                })
                .0;
            let sec = &galley.job.sections[si];
            let expected_size = BODY_SIZE * scales[level];
            assert!(
                (sec.format.font_id.size - expected_size).abs() <= 0.01,
                "{theme_name} 主题:H{} section 字号 {:.2} 应精确等于 {:.2}(15×{})",
                level + 1,
                sec.format.font_id.size,
                expected_size,
                scales[level]
            );
            let expected_lh =
                (expected_size * style.line_height_ratio).max(expected_size * floor_em + 0.75);
            assert!(
                (sec.format.line_height.unwrap_or(0.0) - expected_lh).abs() <= 0.5,
                "{theme_name} 主题:H{} section 行高 {:?} 应 ≈ {expected_lh:.2}",
                level + 1,
                sec.format.line_height
            );
            eprintln!(
                "[F5 {theme_name}] H{} section 字号 {:.2}pt/行高 {:.2}({} × {:.2})",
                level + 1,
                sec.format.font_id.size,
                sec.format.line_height.unwrap_or(0.0),
                expected_size,
                style.line_height_ratio
            );
            // ②b 阶梯 —— **字形墨迹层互证**(放宽到 ±0.13):位图光栅化对
            // ink bbox 有 ±1px 的逐字形取整,同一探针字形在 15pt 与 30pt
            // 下并非严格线性(实测 16px@15pt vs 30px@30pt,比值 1.875);
            // ±0.13 内的新旧分级仍可判(旧默认 1.6 偏差 0.4)。
            let ink = row_probe_ink_max(&galley.rows[heading_rows[level]]);
            let ratio = ink / body_ref;
            assert!(
                (ratio - scales[level]).abs() <= 0.13,
                "{theme_name} 主题:H{} 探针字形 ink 比 {ratio:.3} 偏离出厂分级 {}(ink {:.2}/正文 {:.2})",
                level + 1,
                scales[level],
                ink,
                body_ref
            );
            eprintln!(
                "[F5 {theme_name}] H{} 探针字形 ink 高 {:.2}px(×{:.3})",
                level + 1,
                ink,
                ratio
            );
        }

        // ③ H1 行盒随字号重算(§1 之前的「写死 17px 裁切 H1」回归锚):
        //    行盒高 ≈ max(字号×行距倍率, 字号×CJK 下限+0.75px)。
        let h1 = heading_rows[0];
        let h1_row_h = galley.rows[h1].max_y() - galley.rows[h1].min_y();
        let h1_size = BODY_SIZE * scales[0];
        let floor_em = floor.unwrap_or(1.0).max(1.0);
        let expected = (h1_size * style.line_height_ratio).max(h1_size * floor_em + 0.75);
        assert!(
            (h1_row_h - expected).abs() <= 1.5,
            "{theme_name} 主题:H1 行盒高 {h1_row_h:.2} 应随字号重算 ≈ {expected:.2}(ratio {},floor {floor_em})",
            style.line_height_ratio
        );
        eprintln!(
            "[F5 {theme_name}] H1 行盒高 {:.2}px(期望 ≈{:.2},字号 {:.1},行距 {})",
            h1_row_h, expected, h1_size, style.line_height_ratio
        );

        // ④ 标题呼吸差值:标题行顶到上一内容行底的空隙(= 中间夹的
        //    「空行行 + spacer 行」行盒高之和)与段落间空隙(= 只夹空行行)
        //    的差 ≈ spacer 高 —— 行盒按堆叠相邻,目标行 min_y 减去上一
        //    内容行 max_y 即被跳过空行/spacer 的总高。
        let gap_above = |row_i: usize| -> f32 {
            // 向上找最近的**内容**行(非空行、非 spacer)
            for j in (0..row_i).rev() {
                let t = galley.rows[j].row.text();
                if t != " " && !t.is_empty() {
                    return galley.rows[row_i].min_y() - galley.rows[j].max_y();
                }
            }
            panic!("{theme_name} 主题:行 {row_i} 上方没有内容行");
        };
        let heading_gaps: Vec<f32> = heading_rows.iter().map(|&i| gap_above(i)).collect();
        let para_gaps: Vec<f32> = body_rows.iter().map(|&i| gap_above(i)).collect();
        // 空行行高一致性:三种前驱(标题后/正文段后/列表后)的段落空隙
        // 实测应均匀,否则「呼吸差值=spacer」的推导不成立。
        let (para_min, para_max) = (
            para_gaps.iter().cloned().fold(f32::INFINITY, f32::min),
            para_gaps.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
        );
        assert!(
            para_max - para_min <= 1.5,
            "{theme_name} 主题:段落空隙应均匀(实测 {para_gaps:?},极差 {:.2})",
            para_max - para_min
        );
        let (heading_min, heading_max) = (
            heading_gaps.iter().cloned().fold(f32::INFINITY, f32::min),
            heading_gaps
                .iter()
                .cloned()
                .fold(f32::NEG_INFINITY, f32::max),
        );
        assert!(
            heading_min - para_max > spacer_h * 0.4,
            "{theme_name} 主题:标题上方空隙最小 {heading_min:.2} 未明显大于段落空隙最大 {para_max:.2}(呼吸感失效)"
        );
        assert!(
            ((heading_max - para_min) - spacer_h).abs() <= 1.5,
            "{theme_name} 主题:呼吸差值 {:.2} 应 ≈ spacer 高 {:.2}(标题空隙 {:.2}/{:.2},段落空隙 {:.2}/{:.2})",
            heading_max - para_min,
            spacer_h,
            heading_min,
            heading_max,
            para_min,
            para_max
        );
        eprintln!(
            "[F5 {theme_name}] 标题上方空隙 {:.2}-{:.2}px vs 段落间 {:.2}-{:.2}px,差 ≈ {:.2}px(spacer {:.2}px)",
            heading_min,
            heading_max,
            para_min,
            para_max,
            heading_max - para_min,
            spacer_h
        );

        // ⑤ 字形像素完整性(#43 同款):全部内容行(含标题/列表)CJK 字形
        //    墨迹中心的最终覆盖者是不透明文本色 —— 画出来了,也没被任何
        //    背景块盖掉(分级放大后的标题行也不越盒互侵)。
        let mut quad_colors: Vec<Color32> = Vec::new();
        for mesh in &meshes {
            for vertex in &mesh.vertices {
                if vertex.color.a() > 200 && !quad_colors.contains(&vertex.color) {
                    quad_colors.push(vertex.color);
                }
            }
        }
        assert!(
            quad_colors
                .iter()
                .any(|c| color_dist(*c, colors.panel) > 100),
            "{theme_name} 主题:mesh 中应存在不透明文本色"
        );
        assert!(
            color_dist(colors.text, colors.panel) > 100,
            "{theme_name} 主题前景背景应可分"
        );
        assert!(
            color_dist(colors.strong, colors.panel) > 100,
            "{theme_name} 主题标题色应可与背景分"
        );
        let mut probed = 0usize;
        for (i, placed) in galley.rows.iter().enumerate() {
            if placed.row.glyphs.is_empty() {
                continue;
            }
            for glyph in &placed.row.glyphs {
                let Some((min, size)) = glyph_ink_rect(placed.pos, glyph) else {
                    continue;
                };
                if !is_cjk_ideograph(glyph.chr) {
                    continue;
                }
                let p = egui::pos2(min.x + size.x / 2.0, min.y + size.y * 0.75);
                match final_covered_color(&meshes, p) {
                    Some(got) => assert!(
                        got.a() > 200
                            && quad_colors.contains(&got)
                            && color_dist(got, colors.panel) > 100,
                        "{theme_name} 主题:行 {} 字形 {} 墨迹中心 {p:?} 最终色 {got:?} 不是可见文本色",
                        i,
                        glyph.chr
                    ),
                    None => panic!("{theme_name} 主题:字形墨迹中心 {p:?} 无任何覆盖(未绘制)"),
                }
                probed += 1;
            }
        }
        assert!(probed >= 100, "{theme_name} 主题:字形采样点过少({probed})");
        eprintln!("[F5 {theme_name}] CJK 字形墨迹中心采样 {probed} 个,全部为可见文本色",);
    }
}

// ---------------------------------------------------------------------------
// 证据 2:全元素文档(分段渲染路径)—— 最终色图层的带结构分析
// ---------------------------------------------------------------------------

#[test]
fn full_sample_doc_elements_visible_and_spaced_in_both_visuals() {
    if cjk_fonts_missing() {
        eprintln!("本机无 CJK 候选字体,排版验收无混排对象,跳过");
        return;
    }
    for dark in [true, false] {
        let theme_name = if dark { "暗色" } else { "亮色" };
        let (_, colors, style, primitives, _) = render_headless(
            dark,
            "f5-full-doc-acceptance",
            1600.0,
            |panel, font, screen, label| {
                let mut child = panel.new_child(UiBuilder::new().max_rect(screen));
                egui_markdown::MarkdownLabel::new(label, FULL_DOC)
                    .font(font)
                    .wrap()
                    .show(&mut child);
            },
        );
        let meshes = mesh_refs(&primitives);
        assert!(
            !meshes.is_empty(),
            "{theme_name} 主题:全元素文档应有绘制内容"
        );
        let (w, h) = (PANEL_W as usize, 1600usize);
        let layer = rasterize_final_colors(&meshes, w, h);
        let at = |x: usize, y: usize| layer[y * w + x];

        // 文本类墨(正文/标题/高亮 token 的最终覆盖色,不透明且非底色)
        let is_textish = |c: Color32| c.a() > 200 && color_dist(c, colors.panel) > 100;
        let is_strong = |c: Color32| c == colors.strong;
        let is_border = |c: Color32| c == colors.border;

        let strong_rows: Vec<bool> = (0..h)
            .map(|y| (0..w).any(|x| is_strong(at(x, y))))
            .collect();
        let strong_bands = bands_from_rows(&strong_rows);
        // 本文档里 strong 色应**恰好只属于 6 级标题**:正文与列表不用加粗
        // (文档构造时排除),表头单元格在 app 注册了 "bold" 字族后走
        // bold 字族 + 正文色(#30 口径,vendored table.rs 的 `link_font_and_color`
        // 仅在无 bold 字族时才回退 strong 色)。恰 6 条因此是「strong=标题」
        // 分类成立性的断言,不是凑数。
        assert_eq!(
            strong_bands.len(),
            HEADING_ANCHORS.len(),
            "{theme_name} 主题:strong 色带应恰为 6 级标题"
        );
        let heading_bands = &strong_bands[..6];

        // ① 标题阶梯(像素层):H1 带高 ≈ 2× H6 带高 ≈ 2× 正文带高。
        let textish_rows: Vec<bool> = (0..h)
            .map(|y| (8..w - 8).any(|x| is_textish(at(x, y))))
            .collect();
        let text_bands = bands_from_rows(&textish_rows);
        let band_h = |b: (usize, usize)| (b.1 - b.0 + 1) as f32;
        let h1_h = band_h(heading_bands[0]);
        let h6_h = band_h(heading_bands[5]);
        let scales = style.heading.scales;
        let first_body_band = *text_bands
            .iter()
            .find(|b| b.0 < heading_bands[0].0)
            .unwrap_or_else(|| panic!("{theme_name} 主题:H1 之上应有开场正文带"));
        let body_h = band_h(first_body_band);
        eprintln!(
            "[F5 全元素 {theme_name}] 像素带高:H1 {:.0}px/H6 {:.0}px/正文 {:.0}px,strong 带 {} 条",
            h1_h,
            h6_h,
            body_h,
            strong_bands.len()
        );
        for (level, &heading) in heading_bands.iter().enumerate().skip(1) {
            let ratio = h1_h / band_h(heading);
            // 容差 0.3:像素带高是**墨水外沿**的度量,CJK 粗体笔画的墨水
            // 比常规字重外扩 ~1px(2026-10-09 bold 族链接入 NotoSansCJK-Bold
            // 后 H6 带高 16→17px,H1/H6 比 1.875→1.765,阶梯本身未动)。
            // 精确分级比由排版文档测试的 galley 层探针承担(见下方正文比
            // 断言的注释,同款「±1-2px em 覆盖率噪声」口径)。
            assert!(
                (ratio - scales[0] / scales[level]).abs() <= 0.3,
                "{theme_name} 主题:H1/H{} 带高比 {:.3} 应 ≈ {:.3}(出厂分级之比)",
                level + 1,
                ratio,
                scales[0] / scales[level]
            );
        }
        // 像素带高对正文行的比:同一字号下不同字形集的带高有 ±1-2px 的
        // em 覆盖率噪声(实测正文带 17px vs H6 带 16px,同为 15pt),拿
        // 精确比值会被噪声淹没;改为**分离新旧分级**的下限断言 —— 旧默认
        // H1=1.6× 下带高比 ≈1.4,新 2.0× 下实测 ≈1.76-1.9。精确比值由
        // 排版文档测试的 galley 层探针字形测量承担。
        assert!(
            h1_h / body_h >= 1.7,
            "{theme_name} 主题:H1/正文带高比 {:.3} 应 ≥1.7(新分级 2.0;旧默认 1.6 下 ≈1.4)",
            h1_h / body_h
        );
        assert!(
            (h6_h / body_h - 1.0).abs() <= 0.2,
            "{theme_name} 主题:H6/正文带高比 {:.3} 应 ≈ 1.0(H6 与正文同字号)",
            h6_h / body_h
        );

        // ② 标题呼吸(像素层,分段渲染路径):标题带上方空隙 > 标题下方
        //    正文带的空隙(空行行),差值 ≈ spacer 高。配对只取可确信分类
        //    的带对(标题带上/下都取紧邻文本带),引用/表格/代码块之间的
        //    块间距不混进样本。
        let spacer_h = style.block_spacing + style.heading_space_above;
        let nearest_below = |b: (usize, usize)| -> Option<(usize, usize)> {
            text_bands.iter().copied().find(|t| t.0 > b.1)
        };
        let nearest_above = |b: (usize, usize)| -> Option<(usize, usize)> {
            text_bands.iter().copied().rfind(|t| t.1 < b.0)
        };
        let mut heading_gaps = Vec::new();
        let mut para_gaps = Vec::new();
        for &heading in heading_bands {
            if let Some(prev) = nearest_above(heading) {
                heading_gaps.push((heading.0 - prev.1 - 1) as f32);
            }
            if let Some(next) = nearest_below(heading) {
                para_gaps.push((next.0 - heading.1 - 1) as f32);
            }
        }
        assert!(
            heading_gaps.len() >= 5,
            "{theme_name} 主题:交替段标题空隙样本不足({heading_gaps:?})"
        );
        assert!(
            para_gaps.len() >= 5,
            "{theme_name} 主题:交替段段落空隙样本不足({para_gaps:?})"
        );
        // Compare the same glyphs with one extra spacer. Absolute gaps between
        // different fonts/headings depend on ascent and lineGap (Hiragino and
        // Noto differ); a paired measurement isolates the spacing behavior.
        let (_, _, _, spaced_primitives, _) = render_headless(
            dark,
            "f5-full-doc-extra-spacing",
            1600.0,
            |panel, font, screen, label| {
                let mut child = panel.new_child(UiBuilder::new().max_rect(screen));
                let mut spaced_style = (*style).clone();
                spaced_style.heading_space_above += spacer_h;
                egui_markdown::MarkdownLabel::new(label, FULL_DOC)
                    .font(font)
                    .style(&spaced_style)
                    .wrap()
                    .show(&mut child);
            },
        );
        let spaced_layer = rasterize_final_colors(&mesh_refs(&spaced_primitives), w, h);
        let spaced_headings = bands_from_rows(
            &(0..h)
                .map(|y| (0..w).any(|x| is_strong(spaced_layer[y * w + x])))
                .collect::<Vec<_>>(),
        );
        let spaced_text = bands_from_rows(
            &(0..h)
                .map(|y| (8..w - 8).any(|x| is_textish(spaced_layer[y * w + x])))
                .collect::<Vec<_>>(),
        );
        assert_eq!(spaced_headings.len(), heading_bands.len());
        let spaced_gaps: Vec<_> = spaced_headings
            .iter()
            .filter_map(|heading| {
                spaced_text
                    .iter()
                    .rfind(|text| text.1 < heading.0)
                    .map(|prev| (heading.0 - prev.1 - 1) as f32)
            })
            .collect();
        assert_eq!(spaced_gaps.len(), heading_gaps.len());
        for (before, after) in heading_gaps.iter().zip(spaced_gaps) {
            assert!(*before > 0.0, "标题与前一行不重叠");
            assert!(
                (after - before - spacer_h).abs() <= 1.0,
                "{theme_name}: 增加 {spacer_h}px spacer 应只增加同量空隙: {before} -> {after}"
            );
        }

        // ③ 引用竖条/表格竖线:竖直 border 色条(宽 ≤6px 的列上有 ≥16 个
        //    border 像素)在像素层可检出 —— 引用条与表格 cell 边框共用
        //    widgets.noninteractive.bg_stroke 色。
        let border_cols: Vec<usize> = (0..w)
            .filter(|&x| (0..h).filter(|&y| is_border(at(x, y))).count() >= 16)
            .collect();
        assert!(
            !border_cols.is_empty(),
            "{theme_name} 主题:应存在竖直 border 色条(引用条/表格竖线)"
        );

        // ④ 表格横边框:≥3 条 y 行上 border 色像素计数 ≥150(顶线/表头
        //    分隔线/底线;TableStyle 出厂 1px 全边框)。
        let wide_border_rows: Vec<bool> = (0..h)
            .map(|y| (0..w).filter(|&x| is_border(at(x, y))).count() >= 150)
            .collect();
        let table_lines = bands_from_rows(&wide_border_rows);
        assert!(
            table_lines.len() >= 3,
            "{theme_name} 主题:表格应有 ≥3 条横边框(实际 {} 条,竖 border 列 {} 根)",
            table_lines.len(),
            border_cols.len()
        );
        let y_table_top = table_lines[0].0;

        // ⑤ 代码块底色:大块 code_bg 色矩形(≥200px 宽 × ≥30px 高,位于
        //    表格之下)—— 代码块背景条在像素层的签名。
        let mut cb_bbox = (w, 0usize, h, 0usize); // (x_min, x_max, y_min, y_max)
        let mut cb_count = 0usize;
        for y in (y_table_top + 1)..h {
            for x in 0..w {
                if at(x, y) == colors.code_bg {
                    cb_count += 1;
                    cb_bbox.0 = cb_bbox.0.min(x);
                    cb_bbox.1 = cb_bbox.1.max(x);
                    cb_bbox.2 = cb_bbox.2.min(y);
                    cb_bbox.3 = cb_bbox.3.max(y);
                }
            }
        }
        let (cb_x0, cb_x1, cb_y0, cb_y1) = (cb_bbox.0, cb_bbox.1, cb_bbox.2, cb_bbox.3);
        assert!(
            cb_x1 - cb_x0 >= 200 && cb_y1 - cb_y0 >= 30,
            "{theme_name} 主题:代码块底色矩形应 ≥200×30px(实际 {}×{},像素数 {})",
            cb_x1 - cb_x0,
            cb_y1 - cb_y0,
            cb_count
        );
        eprintln!(
            "[F5 全元素 {theme_name}] 表格横边框 {} 条(首条 y={}),竖 border 列 {} 根,代码块底色 {}×{}px",
            table_lines.len(),
            y_table_top,
            border_cols.len(),
            cb_x1 - cb_x0,
            cb_y1 - cb_y0
        );

        // ⑥ 全元素行带量:全部元素(7 正文段 + 6 标题 + 2 列表 + 1 引用 +
        //    表头 + 表格数据行 + 代码 3 行 + 文末)折出的文本带总量。
        assert!(
            text_bands.len() >= 18,
            "{theme_name} 主题:全元素文档文本带应有 ≥18 条(实际 {})",
            text_bands.len()
        );
        eprintln!(
            "[F5 全元素 {theme_name}] 文本带 {} 条,全部元素渲染可见",
            text_bands.len()
        );
    }
}

// ---------------------------------------------------------------------------
// 证据 3:正文观感否决线(app 生效样式链路上的镜像)
// ---------------------------------------------------------------------------

/// 纯正文文档(无标题)的行数与总高度对 `heading_space_above` 完全不变
/// —— 用户把标题呼吸调到 0 或 40,一个字的正文排版都不动。vendored 侧
/// 已在 vendored 默认样式上钉过(`tests/heading_spacing.rs`
/// `body_only_document_height_is_invariant_to_heading_space`);这里在
/// **app 生效样式链路**(用户行距 1.5 覆盖 + CJK 行高下限 + 生产字体)
/// 上再钉一次。
#[test]
fn body_only_document_is_invariant_to_heading_space_on_app_style() {
    if cjk_fonts_missing() {
        eprintln!("本机无 CJK 候选字体,排版验收无混排对象,跳过");
        return;
    }
    let doc = "纯正文文档,中文与 English 数字 123 混排,不含任何标题。\n\n第二段正文,观测段落空隙与行推进。\n\n收尾正文段落。";
    let mut results = Vec::new();
    for heading_space in [0.0, 4.0, 40.0] {
        let ctx = egui::Context::default();
        // 入口已过无 CJK 跳过检测;循环内每个新 ctx 都要重装字体,
        // preview_body_family/行高下限读的是本 ctx 的注入状态。
        let _ = fonts::install(&ctx);
        ThemeSettings::default().apply(&ctx, ThemeMode::Dark);
        let mut style = (*egui_markdown_style::global_style(&ctx)).clone();
        style.heading_space_above = heading_space;
        egui_markdown_style::set_style(&ctx, style);
        let font = egui::FontId::new(BODY_SIZE, fonts::preview_body_family(&ctx));
        let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(PANEL_W, 600.0));
        let mut height = f32::NAN;
        let mut rows = 0usize;
        let output = ctx.run_ui(RawInput::default(), |panel| {
            let mut child = panel.new_child(UiBuilder::new().max_rect(screen));
            let (_, galley, _) = egui_markdown::MarkdownLabel::new(
                egui::Id::new(format!("f5-body-invariance-{heading_space}")),
                doc,
            )
            .font(font.clone())
            .wrap()
            .layout_in_ui(&mut child);
            height = galley.size().y;
            rows = galley.rows.len();
        });
        output.drop_without_applying_deltas();
        results.push((heading_space, rows, height));
    }
    let (base_space, base_rows, base_height) = results[0];
    for (space, rows, height) in &results {
        assert_eq!(*rows, base_rows, "heading_space={space}:纯正文文档行数改变");
        assert!(
            (*height - base_height).abs() < 0.01,
            "heading_space={space}:纯正文文档总高度 {height:.2} 改变(基准 {base_space} → {base_height:.2})"
        );
    }
    eprintln!(
        "[F5 否决线] 纯正文文档 rows={base_rows} 高度={base_height:.2}px,heading_space_above ∈ {{0,4,40}} 全不变"
    );
}
