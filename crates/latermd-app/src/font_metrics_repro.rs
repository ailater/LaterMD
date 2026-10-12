//! #43 M1 复现取证 + M2 修复回归护栏(无头)。
//!
//! 用户三症状(2026-09-30 反馈,docs/auto-plan.md #43 条目):
//! 「中文英文数字高低不一,行高也有问题,有显示不全的问题」。
//! 本模块把症状转成可断言的无头测试(正文/粗体/斜体/标题四态 × 中英数字混排)。
//!
//! ## 根因(M1 取证结论;行号引用 egui 0.36.2 / vendored HEAD)
//!
//! 1. **高低不一 = 混排基线偏差,四态全中**。egui 行 metrics 取**链头** face
//!    (`Font::styled_metrics`,epaint text/font.rs:691-703);fallback 字形
//!    (CJK 走链尾 Noto)的基线由
//!    `face_ascent + valign*(行高−line_height) + 0.5*(链头行高−face行高)`
//!    决定(epaint text/text_layout.rs:977-981,`0.5*` 是把差值**取中点**的
//!    近似对齐,不是基线对齐)。Inter(ascent 0.9688em / 行高 1.2100em)与
//!    Noto Sans CJK SC(1.1600em / 1.4480em,hhea)的 ascent 占比不同,残差
//!    `(1.16−0.9688) − (1.448−1.21)/2 = 0.0722em`,再经 `round_to_pixel`
//!    (整像素吸附,text_layout.rs:986)量化放大为 1-2px 的可见错位。
//! 2. **行高问题 = 行距公式是「字号 × 单一比例」**,不是行内实际字形 metrics。
//!    vendored `line_height_for` = `size × line_height_ratio`(默认 1.30)覆盖
//!    epaint 原生行高;egui 行盒取 `max(glyph.line_height)`
//!    (text_layout.rs:964-971)→ CJK face 需要 1.448em,行盒只有 1.30em,
//!    **按 em 盒论越界 0.148em**(13pt ≈1.9px,H1 ≈3.1px)。
//! 3. **显示不全** = 行盒不足时 egui 不裁字形,只按行盒高推进下一行
//!    (text_layout.rs:989-992):越界墨迹与相邻行的墨迹/后续块的不透明背景
//!    (代码块底色、表格底色等)相互侵入遮挡。实墨是否真越界由
//!    `mixed_script_strict_regression_target` 用 `uv_rect` 实测裁决。
//!
//! ## M2 修复(本模块断言的现行口径)
//!
//! - **基线**(症状 1):残差只由链头与 fallback 的**表值差**决定,与
//!   vendored 的 line_height 覆盖值无关 → 修复在 `fonts.rs`:预览链头的
//!   Inter Regular/SemiBold 注册为「行 metrics override 副本」(hhea/OS2 typo
//!   的 ascent/descent/lineGap 改写为本机 CJK face 同款 em 值,等价 CSS
//!   `@font-face { ascent-override }`;字形 outline 不动)。链头与 fallback
//!   行 metrics 全等 → `0.5*(链头行高−face行高)` 与 ascent 差同时归零,
//!   基线偏差实测 = 0。
//! - **行高**(症状 2/3):vendored ①类 `MarkdownStyle::min_line_height_em`
//!   行高下限,`line_height_for = max(size*ratio, size*floor + 0.75px)`;
//!   app 在 `theme::effective_markdown_style` 注入本机 CJK face 实际行高
//!   (Noto ≈1.448em)。floor 只升不降,纯拉丁(min_em=1.0)严格不变。
//! - 预览正文族 = `fonts::preview_body_family`(链头 override Inter);
//!   预览标题/加粗经 vendored `apply_bold` 切 `bold` 别名族(链头 =
//!   override SemiBold)。UI 原生族(Proportional/SemiBold/Medium)不动。
//!
//! ## 字形 metrics 的两种度量(测试里都有,勿混用)
//! - **em 盒**:face 的 `[−ascent, +|descent|+lineGap]`
//!   (`Glyph::font_face_ascent/font_face_height`)——epaint 基线公式消费的
//!   就是它。注意 Latin 字形实墨远小于 em 盒(cap≈0.73em),CJK 表意字符
//!   实墨 ≈ `[−0.08, +0.84]em`,而 CJK em 盒高达 `[−0.288, +1.16]em`
//!   (Noto hhea 对全脚本堆叠超配)。
//! - **实墨**:光栅化字形 quad(`Glyph::uv_rect.offset/size`,tessellate_glyphs
//!   按 `glyph.pos + uv_rect.offset` 贴,epaint text_layout.rs:1159)——
//!   「显示不全」的最终裁决度量。
//!
//! ## 断言归宿
//! - `font_table_metrics_forensics`:字体表值断言(**绿**),锚定 Inter/Noto
//!   的 ascent/descent/line_gap 实际值与 skrifa 的取表策略。
//! - `mixed_script_galley_metrics_forensics`:修复后口径的实测-推算对账
//!   (**绿**):行高/基线/越界每一项都由「表值 + epaint 公式」独立推算,
//!   与实测逐项对上,公式即机制的实证不因修复而放松。
//! - `mixed_script_strict_regression_target`:回归护栏(**绿**,M1 期间曾以
//!   `#[ignore]` 归档 20 项失败 = 复现成功;M2 修复后转正)。四态基线偏差
//!   ≤0.5px、行盒 ≥ 行内 face 行高、实墨不越行盒、相邻行实墨净空 ≥0。

use std::sync::Arc;

use eframe::egui::{self, Color32, FontFamily, FontId, RawInput, Rect, UiBuilder, Vec2};

use crate::fonts;
use crate::fonts::{override_vertical_metrics, parse_vertical_tables, VerticalTables};

/// 预览默认正文字号:egui 0.36.2 出厂 Body(egui style.rs:1419)。
/// 预览侧不显式设字号(`MarkdownLabel` 无 `.font`),走 Default → Body。
const BODY_SIZE: f32 = 13.0;
/// vendored `MarkdownStyle::line_height_ratio` 默认值(egui_markdown_style style.rs:59-75)。
const LINE_HEIGHT_RATIO: f32 = 1.30;
/// vendored `line_height_for` floor 生效时的吸附安全余量(egui_markdown
/// layout.rs `LINE_HEIGHT_FLOOR_SLACK_PX`,推算与 vendored 同源)。
const LINE_HEIGHT_FLOOR_SLACK_PX: f32 = 0.75;
/// vendored 标题字号缩放(egui_markdown_style style.rs,H1-H6;镜像复制,默认变更需同步)。
const HEADING_SCALES: [f32; 6] = [2.0, 1.55, 1.30, 1.15, 1.08, 1.0];
/// epaint 的 UI 量化网格(emath gui_rounding.rs:17;styled_metrics 按 1/32 吸附)。
const GUI_ROUNDING: f32 = 1.0 / 32.0;
/// 严格断言的基线偏差阈值:0.5px。半像素以内人眼不可辨;egui 对基线做
/// 整像素吸附(`round_to_pixel`),亚像素残差一旦超过 0.5px 就必跳 1px。
/// 任务书参考值 1px 已可辨(正文 13pt 现状即 1px),故严格线取 0.5px。
const STRICT_BASELINE_TOLERANCE: f32 = 0.5;
/// 实墨越界的判定容差:亚像素(光栅化 quad 与行盒都是整像素/1/32 网格值)。
const STRICT_INK_EPSILON: f32 = 0.01;
/// em 盒越界的容忍线:基线整像素吸附的理论量化界(round 最多向下 0.5px,
/// 加行盒吸附交互,实测最大 0.375px)。精确 0 在离散吸附下不可达,
/// 豁免理由见 `mixed_script_strict_regression_target` 函数级文档。
const STRICT_EM_BOX_SNAP_TOLERANCE: f32 = 1.0;
/// 测量对账容差:推算与实测都落在同一量化网格上,只容忍 ulp 级噪声。
const FORENSIC_EPSILON: f32 = 1.0 / 64.0;

fn round_ui(v: f32) -> f32 {
    (v / GUI_ROUNDING).round() * GUI_ROUNDING
}

fn assert_approx(a: f32, b: f32, eps: f32, msg: &str) {
    assert!((a - b).abs() <= eps, "{msg}: |{a:.4} − {b:.4}| > {eps:.4}");
}

pub(crate) fn is_cjk_ideograph(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}')
}

pub(crate) fn is_latin_alnum(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

// ---------------------------------------------------------------------------
// 取证 1:字体文件表值(Inter 与 CJK 回退的 ascent/descent/line_gap 实际值)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 取证 2:galley 侧(实测与推算逐项对账,把三症状量化值钉进现状断言)
// ---------------------------------------------------------------------------

/// 一个 face 在具体字号下的行 metrics(round_ui 网格上;与 epaint
/// `StyledMetrics` 的 ascent/row_height 同一量化)。
#[derive(Clone, Copy, Debug, PartialEq)]
struct FaceMetrics {
    ascent: f32,
    row_height: f32,
}

/// 从 `styled_metrics` 的同式推算(skrifa px_scale_factor = size/upem,
/// epaint text/font.rs:562-565 逐项 round_ui)。
fn styled_metrics(size: f32, tables: &VerticalTables) -> FaceMetrics {
    let scale = size / tables.units_per_em as f32;
    let ascent = round_ui(tables.selected().0 as f32 * scale);
    let descent = round_ui(tables.selected().1 as f32 * scale);
    let line_gap = round_ui(tables.selected().2 as f32 * scale);
    FaceMetrics {
        ascent,
        row_height: ascent - descent + line_gap,
    }
}

/// epaint galley_from_rows 的行高:吸到整像素(headless ppp=1)。
fn snapped_row_height(needed: f32) -> f32 {
    needed.round()
}

/// epaint 基线公式(text_layout.rs:977-981;vendored `text_format` 是
/// valign=BOTTOM → factor 1.0,line_height = size×ratio 的原始值,行高是
/// 吸附后的整像素值)。
fn predicted_baseline(
    face: FaceMetrics,
    head: FaceMetrics,
    row_height: f32,
    line_height: f32,
) -> f32 {
    (face.ascent + (row_height - line_height) + 0.5 * (head.row_height - face.row_height)).round()
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ScriptFace {
    /// 链头拉丁 face(Inter / Inter-SemiBold):拉丁字符与数字由它 shaping。
    LatinChainHead,
    /// CJK 回退 face(Noto Sans CJK SC):汉字由它 shaping。
    CjkFallback,
    /// 其他(标点/空格等未参与断言的字符)。
    Other,
}

/// 单个 glyph 的取证快照(行相对基线换算为 galley 坐标)。
#[derive(Clone, Copy, Debug)]
struct GlyphForensics {
    ch: char,
    face: ScriptFace,
    /// 基线(galley 坐标,行顶 + 行相对 glyph.pos.y)。
    baseline: f32,
    /// em 盒(基线起算):`[baseline − face_ascent, baseline + face行高 − face_ascent]`。
    em_box: (f32, f32),
    /// 实墨(uv_rect 起算,基线相对 offset/size 换算到 galley 坐标)。
    /// `(top, bottom)`;字形未进 atlas(如空白)时为 `None`。
    ink: Option<(f32, f32)>,
    /// vendored 的行高覆盖值(size × 1.30,未吸像素)。
    line_height: f32,
    face_ascent: f32,
    face_row_height: f32,
    head_row_height: f32,
}

#[derive(Clone, Debug)]
struct RowForensics {
    text: String,
    /// 行盒(galley 坐标,`[top, bottom]`;bottom = top + row.size.y)。
    top: f32,
    bottom: f32,
    glyphs: Vec<GlyphForensics>,
    /// 行内字形的最大 face 行高(em 盒口径的「实际字形 metrics」需求)。
    max_face_row_height: f32,
    /// 行内实墨的极值范围(仅计已进 atlas 的字形)。
    ink_extent: Option<(f32, f32)>,
    /// `Some(CJK基线最大值 − 拉丁基线最大值)`,仅当该行同时含两种字形。
    baseline_cjk_minus_latin: Option<f32>,
    /// 行内 glyph 相对行盒的最大越界(正值 = 越界像素;em 盒口径)。
    overflow_top_em: f32,
    overflow_bottom_em: f32,
}

impl RowForensics {
    fn height(&self) -> f32 {
        self.bottom - self.top
    }
}

/// 把 galley 的每行/每字形换算成取证快照。face 归属按**字符判据**:
/// CJK 表意字符只可能由链尾 CJK face 塑形(链头 Inter 无这些码位),ASCII
/// 字母数字只可能由链头塑形(CJK face 的 ASCII 码位排在链头之后)。
/// M1 曾用探针 metrics 数值对账分类 —— M2 修复后链头(override Inter)与
/// CJK 回退的行 metrics **全等**,数值判据天然失效,字符判据才是稳定归宿
/// (metrics 全等本身由 `mixed_script_galley_metrics_forensics` 的探针断言
/// 单独对账)。
fn collect_row_forensics(galley: &Arc<egui::epaint::text::Galley>) -> Vec<RowForensics> {
    galley
        .rows
        .iter()
        .map(|placed| {
            let top = placed.pos.y;
            let bottom = top + placed.row.size.y;
            let mut out = RowForensics {
                text: placed.row.text(),
                top,
                bottom,
                glyphs: Vec::new(),
                max_face_row_height: 0.0,
                ink_extent: None,
                baseline_cjk_minus_latin: None,
                overflow_top_em: f32::NEG_INFINITY,
                overflow_bottom_em: f32::NEG_INFINITY,
            };
            for glyph in &placed.row.glyphs {
                let face_ascent = glyph.font_face_ascent;
                let face_row_height = glyph.font_face_height;
                let face = if is_cjk_ideograph(glyph.chr) {
                    ScriptFace::CjkFallback
                } else if is_latin_alnum(glyph.chr) {
                    ScriptFace::LatinChainHead
                } else {
                    ScriptFace::Other
                };
                let baseline = placed.pos.y + glyph.pos.y;
                let em_box = (
                    baseline - face_ascent,
                    baseline + (face_row_height - face_ascent),
                );
                let ink = if glyph.uv_rect.offset == Vec2::ZERO && glyph.uv_rect.size == Vec2::ZERO
                {
                    None
                } else {
                    let t = baseline + glyph.uv_rect.offset.y;
                    Some((t, t + glyph.uv_rect.size.y))
                };
                out.max_face_row_height = out.max_face_row_height.max(face_row_height);
                if let Some((t, b)) = ink {
                    let extent = out.ink_extent.unwrap_or((t, b));
                    out.ink_extent = Some((extent.0.min(t), extent.1.max(b)));
                }
                out.overflow_top_em = out.overflow_top_em.max(top - em_box.0);
                out.overflow_bottom_em = out.overflow_bottom_em.max(em_box.1 - bottom);
                out.glyphs.push(GlyphForensics {
                    ch: glyph.chr,
                    face,
                    baseline,
                    em_box,
                    ink,
                    line_height: glyph.line_height,
                    face_ascent,
                    face_row_height,
                    head_row_height: glyph.font_height,
                });
            }
            let latin_base = out
                .glyphs
                .iter()
                .filter(|g| g.face == ScriptFace::LatinChainHead && is_latin_alnum(g.ch))
                .map(|g| g.baseline)
                .fold(None::<f32>, |acc, v| Some(acc.map_or(v, |a| a.max(v))));
            let cjk_base = out
                .glyphs
                .iter()
                .filter(|g| g.face == ScriptFace::CjkFallback && is_cjk_ideograph(g.ch))
                .map(|g| g.baseline)
                .fold(None::<f32>, |acc, v| Some(acc.map_or(v, |a| a.max(v))));
            if let (Some(l), Some(c)) = (latin_base, cjk_base) {
                out.baseline_cjk_minus_latin = Some(c - l);
            }
            out
        })
        .collect()
}

/// 一行取证的紧凑描述(断言失败信息里逐行可读)。
fn describe_rows(rows: &[RowForensics]) -> String {
    rows.iter()
        .map(|r| {
            let format_g = |g: &GlyphForensics| {
                let ink = g
                    .ink
                    .map(|(t, b)| format!("[{t:.2},{b:.2}]"))
                    .unwrap_or_else(|| "[]".into());
                format!(
                    "{}:{:?}@{}(em[{},{}],ink{},lh{})",
                    escape(g.ch),
                    g.face,
                    g.baseline,
                    g.em_box.0,
                    g.em_box.1,
                    ink,
                    g.line_height
                )
            };
            format!(
                "  row「{}」[{},{}]h={:.2} maxFaceRow={:.2} Δbase={:?} 越界em(top{:+.2},b{:+.2})\n{}",
                r.text.chars().take(12).collect::<String>(),
                r.top,
                r.bottom,
                r.height(),
                r.max_face_row_height,
                r.baseline_cjk_minus_latin,
                r.overflow_top_em,
                r.overflow_bottom_em,
                r.glyphs
                    .iter()
                    .take(6)
                    .map(format_g)
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        })
        .collect()
}

fn escape(c: char) -> String {
    if c.is_control() || c == ' ' {
        format!("{:?}", c)
    } else {
        c.to_string()
    }
}

/// 四态用例(正文/粗体/斜体/标题 H1/H2),混排同一句式:中英数字同现。
struct CaseSpec {
    name: &'static str,
    md: &'static str,
    /// 正文侧期望字号(标题按 HEADING_SCALES 缩放)。
    size: f32,
}

const CASES: &[CaseSpec] = &[
    CaseSpec {
        name: "正文",
        md: "正文 Body 中文 123 数字 abc 混排 Test 高低不一",
        size: BODY_SIZE,
    },
    CaseSpec {
        name: "粗体",
        md: "**粗体 Bold 加粗 456 数字 ABC 混排**",
        size: BODY_SIZE,
    },
    CaseSpec {
        name: "斜体",
        md: "*斜体 Italic 斜体 789 数字 def 混排*",
        size: BODY_SIZE,
    },
    CaseSpec {
        name: "H1",
        md: "# 标题 Heading 一 中文 H1 123",
        size: BODY_SIZE * HEADING_SCALES[0],
    },
    CaseSpec {
        name: "H2",
        md: "## 二级标题 H2 中文 abc 456",
        size: BODY_SIZE * HEADING_SCALES[1],
    },
];

/// 足够长以在 700px 内折行(相邻行墨迹侵入观测)。
const WRAP_DOC: &str = "中文连续行观测相邻行墨迹净空中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文中文";

/// 无头渲染一个 markdown 片段,返回其 galley(`layout_in_ui` 与预览的
/// `show()` 走同一 `build_layout` + `layout_job` 管线;此处只取形,不绘制)。
/// #43 M2 起**与生产 preview::ui 同配置**:字体族取 [`fonts::preview_body_family`]
/// (链头 = 行 metrics 对齐 CJK 回退的 Inter 副本),样式取
/// [`theme::effective_markdown_style`](行高下限已按本机 CJK face 注入)。
fn render_case(ctx: &egui::Context, id_salt: &str, md: &str) -> Arc<egui::epaint::text::Galley> {
    let mut captured: Option<Arc<egui::epaint::text::Galley>> = None;
    let mut output = ctx.run_ui(RawInput::default(), |panel| {
        let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(700.0, 400.0));
        let mut child = panel.new_child(UiBuilder::new().max_rect(screen));
        let style =
            crate::theme::effective_markdown_style(panel, crate::theme::default_markdown_style());
        let font = FontId::new(BODY_SIZE, crate::fonts::preview_body_family(panel));
        let (_, galley, _) = egui_markdown::MarkdownLabel::new(egui::Id::new(id_salt), md)
            .font(font)
            .style(&style)
            .wrap()
            .layout_in_ui(&mut child);
        captured = Some(galley);
    });
    output.textures_delta.clear();
    captured.expect("layout_in_ui 应产出 galley")
}

/// 逐行摘要(跨帧稳定性断言的指纹;不含浮点全量,只取量化网格上的关键量)。
fn geometry_fingerprint(rows: &[RowForensics]) -> Vec<(String, f32, Option<f32>, f32, f32, f32)> {
    rows.iter()
        .map(|r| {
            (
                r.text.clone(),
                r.height(),
                r.baseline_cjk_minus_latin,
                r.max_face_row_height,
                r.overflow_top_em,
                r.overflow_bottom_em,
            )
        })
        .collect()
}

/// 四态 × 混排的现状量化与「推算 == 实测」对账(**现状断言,绿**;
/// 每个 TODO(M2) 处标注修复后的收紧点)。
#[test]
fn mixed_script_galley_metrics_forensics() {
    let ctx = egui::Context::default();
    let has_cjk = fonts::install(&ctx).is_some();
    ctx.run_ui(RawInput::default(), |_| {})
        .drop_without_applying_deltas();
    if !has_cjk {
        eprintln!("本机无 CJK 候选字体,galley 侧 CJK 断言跳过");
        return;
    }

    // 表值侧(链头与 fallback 两侧的 ground truth)。M2 修复后预览链头 =
    // override Inter 副本(fonts.rs 用本机 CJK face 的 em 值改写 hhea/OS2 typo),
    // 推算链头侧必须用 **override 后**的表值(与 fonts::build_definitions 同源,
    // 此处以同一 patch 函数重建,不用实测值自证,避免套套逻辑)。
    let (cjk_path, cjk_prop_idx, _) = fonts::cjk_source_for_test().expect("已确认本机有 CJK");
    let cjk_bytes = std::fs::read(cjk_path).expect("CJK 候选读取失败");
    let noto = parse_vertical_tables(&cjk_bytes, cjk_prop_idx).expect("Noto 表解析失败");
    let target = noto.vertical_metrics_em();
    let native_bytes = if cfg!(target_os = "macos") {
        std::fs::read("/System/Library/Fonts/SFNS.ttf").expect("macOS system font")
    } else {
        include_bytes!("../../../assets/fonts/Inter-Regular.ttf").to_vec()
    };
    let bold_bytes = if cfg!(target_os = "macos") {
        native_bytes.clone()
    } else {
        include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf").to_vec()
    };
    let inter = parse_vertical_tables(
        &override_vertical_metrics(&native_bytes, 0, target)
            .expect("Inter-Regular override patch 失败"),
        0,
    )
    .expect("override Inter-Regular 表解析失败");
    let semibold = parse_vertical_tables(
        &override_vertical_metrics(&bold_bytes, 0, target)
            .expect("Inter-SemiBold override patch 失败"),
        0,
    )
    .expect("override Inter-SemiBold 表解析失败");
    let native_inter =
        parse_vertical_tables(&native_bytes, 0).expect("原生 Inter-Regular 表解析失败");

    // 行高下限(生产注入值):fonts::install 存的本机 CJK face 实际行高。
    let floor_em = fonts::line_height_floor_em(&ctx).expect("有 CJK 时行高下限应存在");

    // galley 探针:链头字符与汉字各 shaping 一次,取 face 实际值。
    // 与 render_case 同族(预览正文族 = override Inter 链头 + CJK 链尾)。
    let body_font = FontId::new(BODY_SIZE, fonts::preview_body_family(&ctx));
    let probe = |font: &FontId, ch: char| {
        let galley =
            ctx.fonts_mut(|f| f.layout_no_wrap(ch.to_string(), font.clone(), Color32::WHITE));
        let g = &galley.rows[0].row.glyphs[0];
        FaceMetrics {
            ascent: g.font_face_ascent,
            row_height: g.font_face_height,
        }
    };
    let inter_probe = probe(&body_font, 'H');
    let noto_probe = probe(&body_font, '中');

    // 探针与表值推算对账(skrifa 策略 + round_ui 网格)。修复后的核心
    // 机制实锚:预览链头(override Inter)与 CJK 回退的 ascent/行高探针
    // **逐项相等** —— epaint 基线公式的 `0.5*(链头行高−face行高)` 项与
    // ascent 差同时归零,基线偏差失去来源。
    assert_approx(
        inter_probe.ascent,
        styled_metrics(BODY_SIZE, &inter).ascent,
        FORENSIC_EPSILON,
        "override Inter 探针 ascent 与表值推算不符",
    );
    assert_approx(
        inter_probe.row_height,
        styled_metrics(BODY_SIZE, &inter).row_height,
        FORENSIC_EPSILON,
        "override Inter 探针行高与表值推算不符",
    );
    // 「全等」= 量化网格内等价:Inter 副本的 upem(2048)与 CJK face 的
    // upem(如 1000)不同,em 值各自 round 到整数 font units 后存在
    // ≤0.5/upem em 的量化残差,在个别字号会差一个 1/32 点网格刻度
    // (0.031px)—— 远小于基线的整像素吸附粒度,基线偏差实测仍为 0
    // (由 strict 矩阵断言)。
    assert!(
        (inter_probe.ascent - noto_probe.ascent).abs() <= GUI_ROUNDING + 0.005
            && (inter_probe.row_height - noto_probe.row_height).abs() <= GUI_ROUNDING + 0.005,
        "预览链头 {:?} 与 CJK 回退 {:?} 的行 metrics 应在量化网格内等价",
        inter_probe,
        noto_probe
    );
    // UI 原生族不受修复影响:Proportional 链头仍是原生 Inter 出厂表值。
    let ui_latin = probe(&FontId::new(BODY_SIZE, FontFamily::Proportional), 'H');
    assert_eq!(
        ui_latin,
        styled_metrics(BODY_SIZE, &native_inter),
        "Proportional(UI)链头 metrics 应保持原生 Inter 不变"
    );
    // UI 原生族与 CJK 回退之间**保持** M1 时的表值差(修复只作用于预览族):
    // 该差值的存在同时是「修复没有外溢到 UI」的反向证据。
    assert!(
        (ui_latin.row_height - noto_probe.row_height).abs() > 2.0,
        "Proportional(UI)链头行高 {:.3} 与 CJK {:.3} 的表值差应保持(未被外溢修复)",
        ui_latin.row_height,
        noto_probe.row_height
    );
    assert_approx(
        noto_probe.ascent,
        styled_metrics(BODY_SIZE, &noto).ascent,
        FORENSIC_EPSILON,
        "Noto 探针 ascent 与表值推算不符",
    );
    assert_approx(
        noto_probe.row_height,
        styled_metrics(BODY_SIZE, &noto).row_height,
        FORENSIC_EPSILON,
        "Noto 探针行高与表值推算不符",
    );

    // 全族回退链核查(PR #52 前科:每个 FontFamily::Name 是独立回退链,
    // bold/权重族漏挂 CJK 会整行变方块;此处按 shaping 实测,比链表断言强)。
    // bold 别名族(M2 起链头 = override SemiBold 副本,预览标题/加粗的
    // 基线对齐由它承接);Inter-SemiBold 原生族(UI 消费)保持出厂表值。
    let bold_font = FontId::new(BODY_SIZE, FontFamily::Name(Arc::from(fonts::FAMILY_BOLD)));
    let bold_head = probe(&bold_font, 'H');
    assert_approx(
        bold_head.ascent,
        styled_metrics(BODY_SIZE, &semibold).ascent,
        FORENSIC_EPSILON,
        "bold 族链头应为 override Inter-SemiBold",
    );
    let bold_cjk_head = probe(&bold_font, '中');
    assert!(
        (bold_head.ascent - bold_cjk_head.ascent).abs() <= GUI_ROUNDING + 0.005
            && (bold_head.row_height - bold_cjk_head.row_height).abs() <= GUI_ROUNDING + 0.005,
        "bold 族链头 {:?} 与 CJK 回退 {:?} 的行 metrics 应在量化网格内等价",
        bold_head,
        bold_cjk_head
    );
    let semibold_font = FontId::new(
        BODY_SIZE,
        FontFamily::Name(Arc::from(fonts::FAMILY_SEMIBOLD)),
    );
    for (family_name, font, head_tables) in [
        ("bold 别名族(预览标题/加粗)", &bold_font, &semibold),
        ("Inter-SemiBold 族(UI)", &semibold_font, &native_inter),
    ] {
        let head = probe(font, 'H');
        assert_approx(
            head.ascent,
            styled_metrics(BODY_SIZE, head_tables).ascent,
            FORENSIC_EPSILON,
            &format!("{family_name} 链头表值与预期不符"),
        );
        assert!(
            ctx.fonts_mut(|f| f.has_glyphs(font, "中文标题")),
            "{family_name} 的 CJK 回退链不完整(方块风险,PR #52 同类)"
        );
    }
    // CJK 回退的行 metrics(#43 M2 的 UI 推广,2026-10-12,两族方向相反):
    // 预览侧 bold 族链尾仍是原生表值 Noto(其链头 = CJK 表值副本);UI
    // SemiBold 族链尾换成「链头对齐 CJK 副本」,行 metrics 与链头全等 ——
    // 旧断言在这里钉的是修复前的错位现状,随 UI 推广反转为等值断言。
    let bold_cjk = probe(&bold_font, '中');
    assert_approx(
        bold_cjk.row_height,
        noto_probe.row_height,
        FORENSIC_EPSILON,
        "bold 别名族(预览)汉字应回落 Noto(链尾原生 CJK 生效)",
    );
    let ui_cjk = probe(&semibold_font, '中');
    let ui_head = probe(&semibold_font, 'H');
    assert_approx(
        ui_cjk.row_height,
        ui_head.row_height,
        FORENSIC_EPSILON,
        "Inter-SemiBold 族(UI) 汉字行 metrics 应与链头全等(链头对齐副本)",
    );
    let mono_font = FontId::new(BODY_SIZE, FontFamily::Monospace);
    let mono_cjk = probe(&mono_font, '中');
    assert_approx(
        mono_cjk.row_height,
        styled_metrics(BODY_SIZE, &noto).row_height,
        FORENSIC_EPSILON,
        "Monospace 族的 CJK 回退应为 Noto Mono CJK(同表值)",
    );

    // 四态逐案对账:行高/基线偏差/越界,推算 == 实测(修复后口径)。
    for case in CASES {
        let galley = render_case(&ctx, &format!("m1-forensics-{}", case.name), case.md);
        let probe_at_size = probe(
            &FontId::new(case.size, fonts::preview_body_family(&ctx)),
            'H',
        );
        let noto_at_size_probe = probe(
            &FontId::new(case.size, fonts::preview_body_family(&ctx)),
            '中',
        );
        // 修复机制实锚:任何字号下,预览链头与 CJK 回退的行 metrics
        // 在量化网格内等价(upem 残差 ≤ 一个 1/32 刻度,见上文对账口径)。
        assert!(
            (probe_at_size.ascent - noto_at_size_probe.ascent).abs() <= GUI_ROUNDING + 0.005
                && (probe_at_size.row_height - noto_at_size_probe.row_height).abs()
                    <= GUI_ROUNDING + 0.005,
            "{}: 预览链头 {:?} 与 CJK 回退 {:?} 的行 metrics 应在量化网格内等价",
            case.name,
            probe_at_size,
            noto_at_size_probe
        );
        let rows = collect_row_forensics(&galley);
        let content_rows: Vec<&RowForensics> =
            rows.iter().filter(|r| !r.glyphs.is_empty()).collect();
        assert!(!content_rows.is_empty(), "{}: 无内容行", case.name);
        println!("== {} ==\n{}", case.name, describe_rows(&rows));

        let head = styled_metrics(case.size, &inter);
        let noto_at_size = styled_metrics(case.size, &noto);
        // vendored 行高 = max(字号×ratio, 字号×floor + 吸附安全余量)(M2 floor 模型)。
        let line_height =
            (case.size * LINE_HEIGHT_RATIO).max(case.size * floor_em + LINE_HEIGHT_FLOOR_SLACK_PX);
        let row_height = snapped_row_height(line_height);

        for row in &content_rows {
            // 行高 = 吸附后的 floor 模型行高(行盒取 max(glyph.line_height))。
            assert_approx(
                row.height(),
                row_height,
                FORENSIC_EPSILON,
                &format!("{} 行「{}」行高与推算不符", case.name, row.text),
            );
            // M2 收紧点兑现:行盒 ≥ 行内 max face 行高(CJK 1.448em 需求被覆盖)。
            assert!(
                row.height() + FORENSIC_EPSILON >= row.max_face_row_height,
                "{} 行「{}」行盒 {:.3}px 低于行内 face 行高需求 {:.3}px",
                case.name,
                row.text,
                row.height(),
                row.max_face_row_height
            );
            let latin = row
                .glyphs
                .iter()
                .find(|g| g.face == ScriptFace::LatinChainHead && is_latin_alnum(g.ch))
                .unwrap_or_else(|| panic!("{} 行「{}」缺拉丁字形", case.name, row.text));
            let cjk = row
                .glyphs
                .iter()
                .find(|g| g.face == ScriptFace::CjkFallback && is_cjk_ideograph(g.ch))
                .unwrap_or_else(|| panic!("{} 行「{}」缺 CJK 字形", case.name, row.text));
            // 机制实锚:行内每个字形(含 CJK fallback 字形)携带的链头行高
            // 都是 override Inter 的(= CJK face 同款)—— 修复前是原生 Inter
            // 的 1.21em,与 CJK 1.448em 的差正是基线偏差与行盒缺口的来源。
            for g in [latin, cjk] {
                assert_approx(
                    g.head_row_height,
                    head.row_height,
                    FORENSIC_EPSILON,
                    &format!(
                        "{}「{}」字形链头行高应恒为 override Inter 行高",
                        case.name, row.text
                    ),
                );
                assert_approx(
                    g.line_height,
                    line_height,
                    FORENSIC_EPSILON,
                    &format!(
                        "{}「{}」字形 line_height 应为 vendored 覆盖值",
                        case.name, row.text
                    ),
                );
            }
            // 基线 = epaint 公式逐项复算(现状断言:公式即真凶的实证)。
            assert_approx(
                latin.baseline - row.top,
                predicted_baseline(
                    FaceMetrics {
                        ascent: latin.face_ascent,
                        row_height: latin.face_row_height,
                    },
                    head,
                    row_height,
                    latin.line_height,
                ),
                FORENSIC_EPSILON,
                &format!("{} 拉丁基线与公式推算不符", case.name),
            );
            assert_approx(
                cjk.baseline - row.top,
                predicted_baseline(
                    FaceMetrics {
                        ascent: cjk.face_ascent,
                        row_height: cjk.face_row_height,
                    },
                    head,
                    row_height,
                    cjk.line_height,
                ),
                FORENSIC_EPSILON,
                &format!("{} CJK 基线与公式推算不符", case.name),
            );
            // 修复后口径:链头与 fallback 的 ascent/行高全等 → 公式残差
            // 精确为零,偏差只剩基线整像素吸附的取整差(实测 0)。
            // M1 期间实测为 +1px(13pt)/+2px(H1),修复前的对照见 commit ea31d02。
            let deviation = row.baseline_cjk_minus_latin.unwrap();
            assert!(
                deviation.abs() <= STRICT_BASELINE_TOLERANCE,
                "{}「{}」基线偏差 {:+.3}px 超出 ±{:.1}px",
                case.name,
                row.text.chars().take(8).collect::<String>(),
                deviation,
                STRICT_BASELINE_TOLERANCE
            );
            println!(
                "{}「{}」基线偏差 CJK−拉丁 = {:+.3}px (字号 {:.2}pt, {:.4}em)",
                case.name,
                row.text.chars().take(8).collect::<String>(),
                deviation,
                case.size,
                deviation / case.size
            );
            let predicted_deviation = predicted_baseline(
                FaceMetrics {
                    ascent: noto_at_size.ascent,
                    row_height: noto_at_size.row_height,
                },
                head,
                row_height,
                line_height,
            ) - predicted_baseline(head, head, row_height, line_height);
            assert_approx(
                deviation,
                predicted_deviation,
                FORENSIC_EPSILON,
                &format!("{} 基线偏差与公式推算不符", case.name),
            );
            // em 盒越界量对账(推算只用表值+公式,不用实测 glyph 字段,
            // 避免套套逻辑)。实测口径是「行内所有字形的 em 盒极值」,推算
            // 须对链头(override Inter)与 CJK 回退两个 face 各推 em 盒顶/底
            // 再取 max —— 修复后两者行 metrics 近全等,谁多出 1/32 刻度的
            // ascent 谁就是越界极值的提供者(与 M1 只需推 CJK 一侧不同)。
            let em_extent_rowrel = |face: FaceMetrics| -> (f32, f32) {
                let base = predicted_baseline(face, head, row_height, line_height);
                (base - face.ascent, base + (face.row_height - face.ascent))
            };
            let (latin_top, latin_bottom) = em_extent_rowrel(head);
            let (cjk_top, cjk_bottom) = em_extent_rowrel(noto_at_size);
            // 带符号对账(不钳非负):修复前恒正(越界),修复后为负(em 盒
            // 落在行盒内),保留符号才能把「行盒余量」也对进账。
            let expected_overflow_top = 0.0 - latin_top.min(cjk_top);
            let expected_overflow_bottom = latin_bottom.max(cjk_bottom) - row_height;
            assert_approx(
                row.overflow_top_em,
                expected_overflow_top,
                FORENSIC_EPSILON,
                &format!("{} em 盒越顶量与表值推算不符", case.name),
            );
            assert_approx(
                row.overflow_bottom_em,
                expected_overflow_bottom,
                FORENSIC_EPSILON,
                &format!("{} em 盒越底量与表值推算不符", case.name),
            );
            println!(
                "{} em 盒越界(top {:+.3}px / bottom {:+.3}px),行盒 {:.2}px vs CJK face 需 {:.2}px",
                case.name,
                row.overflow_top_em,
                row.overflow_bottom_em,
                row.height(),
                row.max_face_row_height
            );
        }
    }

    // 相邻行墨迹净空(wrap 文档;M2 收紧点兑现:净空 ≥ 0,不粘连不互侵)。
    let wrap_galley = render_case(&ctx, "m1-forensics-wrap", WRAP_DOC);
    let wrap_rows = collect_row_forensics(&wrap_galley);
    let content: Vec<&RowForensics> = wrap_rows.iter().filter(|r| !r.glyphs.is_empty()).collect();
    assert!(
        content.len() >= 2,
        "wrap 文档应折出至少两行(实际 {})",
        content.len()
    );
    for pair in content.windows(2) {
        let (prev, next) = (pair[0], pair[1]);
        if let (Some((_, prev_bottom)), Some((next_top, _))) = (prev.ink_extent, next.ink_extent) {
            // 实墨口径的行间净空(负值 = 上一行实墨压进下一行区域)。
            let clearance = next_top - prev_bottom;
            println!(
                "相邻行实墨净空 = {clearance:+.3}px(行盒推进 {}px)",
                next.top - prev.top
            );
            assert!(
                clearance >= -STRICT_INK_EPSILON,
                "wrap 相邻行实墨净空为负({clearance:+.3}px),墨迹互侵"
            );
        }
    }

    // 跨帧稳定性:同一 ctx 渲染 3 帧(show() 真实缓存路径 ×1 + 测量 ×2),
    // 几何指纹逐项相等(防 M2 修复引入跨帧漂移的回归盲区)。show() 路径
    // 与 render_case 同配置(预览族 + effective style)。
    let fingerprints: Vec<_> = CASES
        .iter()
        .map(|case| {
            let g = render_case(&ctx, &format!("m1-forensics-{}", case.name), case.md);
            geometry_fingerprint(&collect_row_forensics(&g))
        })
        .collect();
    for frame in 2..=3 {
        let _ = ctx.run_ui(RawInput::default(), |ui| {
            // 真实渲染路径(show() 会填充并命中 vendored 布局缓存)。
            let style = crate::theme::effective_markdown_style(
                ui.ctx(),
                crate::theme::default_markdown_style(),
            );
            let font = FontId::new(BODY_SIZE, fonts::preview_body_family(ui.ctx()));
            for case in CASES {
                egui_markdown::MarkdownLabel::new(
                    egui::Id::new(format!("m1-forensics-show-{}", case.name)),
                    case.md,
                )
                .font(font.clone())
                .style(&style)
                .wrap()
                .show(ui);
            }
        });
        for (i, case) in CASES.iter().enumerate() {
            let g = render_case(&ctx, &format!("m1-forensics-{}", case.name), case.md);
            let f = geometry_fingerprint(&collect_row_forensics(&g));
            assert_eq!(
                f, fingerprints[i],
                "{} 第 {} 帧几何指纹漂移(跨帧布局不稳定)",
                case.name, frame
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 取证 3:回归护栏(M1 期间 #[ignore] 归档 20 项失败 = 复现成功;M2 转正)
// ---------------------------------------------------------------------------

/// 「标题/粗体/斜体/正文四态 × 中英混排」回归矩阵(M1 复现断言的转正形态)。
/// 断言集:
/// - S2:同一视觉行内拉丁与 CJK 基线偏差 ≤ 0.5px(修复后实测 0;
///   M1 现状 13pt +1px、H1 +2px,commit ea31d02 归档)。
/// - S1:行盒高度 ≥ 行内实际字形 metrics(em 盒口径:face 的行高需求;
///   M1 现状缺口 1.8-3.1px)。
/// - S3a:em 盒越出行盒 ≤ 1px —— **有意豁免的口径**(M1 曾按 0.01px 断言,
///   现状必红是复现目标;M2 修复后实测仍有 0.125-0.375px 越顶,登记豁免
///   理由:epaint 基线公式末尾的整像素吸附 `round_to_pixel`(text_layout.rs:986)
///   对非整数 ascent 的 face 有 ±0.5px 离散量化,任何 line_height 取值都无法
///   让 `round(ascent + φ) ≥ ascent` 对全部字号成立(φ ∈ [−0.5,+0.5] 由行盒
///   吸附决定);em 盒本身又是 Noto 对全脚本堆叠的超配度量(CJK 实墨只占
///   ≈0.85em)。「显示不全」的可裁决口径是 S3b(实墨)与 wrap 净空,
///   修复后实墨在行盒内有 3px 以上余量)。
/// - S3b:实墨(atlas 中非透明像素，不含 uv_rect 透明留白)不越出行盒 —— 「显示不全」的最终裁决(严格 0.01px)。
/// - wrap:相邻行实墨净空 ≥ 0(不粘连/不互相遮挡)。
#[test]
fn mixed_script_strict_regression_target() {
    let ctx = egui::Context::default();
    let has_cjk = fonts::install(&ctx).is_some();
    ctx.run_ui(RawInput::default(), |_| {})
        .drop_without_applying_deltas();
    if !has_cjk {
        eprintln!("本机无 CJK 候选字体,严格断言无混排对象,跳过");
        return;
    }

    let mut failures: Vec<String> = Vec::new();
    let mut checked_rows = 0usize;

    for case in CASES {
        let galley = render_case(&ctx, &format!("m1-strict-{}", case.name), case.md);
        let rows = collect_row_forensics(&galley);
        let atlas = ctx.fonts(|fonts| fonts.image());
        for (row, placed) in rows
            .iter()
            .zip(&galley.rows)
            .filter(|(r, _)| !r.glyphs.is_empty())
        {
            checked_rows += 1;
            // S2:同一视觉行内拉丁与 CJK 基线偏差 ≤ 0.5px。
            if let Some(dev) = row.baseline_cjk_minus_latin {
                if dev.abs() > STRICT_BASELINE_TOLERANCE {
                    failures.push(format!(
                        "[{}「{}」] 基线偏差 CJK−拉丁 = {:+.3}px > ±{}px",
                        case.name,
                        row.text.chars().take(8).collect::<String>(),
                        dev,
                        STRICT_BASELINE_TOLERANCE
                    ));
                }
            }
            // S1:行盒高度 ≥ 行内实际字形 metrics(em 盒口径:face 的行高需求)。
            if row.height() + STRICT_INK_EPSILON < row.max_face_row_height {
                failures.push(format!(
                    "[{}「{}」] 行盒 {:.3}px < 行内字形 face 行高 {:.3}px",
                    case.name,
                    row.text.chars().take(8).collect::<String>(),
                    row.height(),
                    row.max_face_row_height
                ));
            }
            // S3a:em 盒越界 ≤ 基线整像素吸附的量化界(1px)。豁免理由见
            // 函数级文档(em 盒是超配度量 + round 离散量化,精确 0 不可达;
            // 可见裁切由 S3b 实墨口径严格断言)。
            if row.overflow_top_em > STRICT_EM_BOX_SNAP_TOLERANCE {
                failures.push(format!(
                    "[{}「{}」] em 盒越出行盒顶 {:+.3}px > {:.1}px",
                    case.name,
                    row.text.chars().take(8).collect::<String>(),
                    row.overflow_top_em,
                    STRICT_EM_BOX_SNAP_TOLERANCE
                ));
            }
            if row.overflow_bottom_em > STRICT_EM_BOX_SNAP_TOLERANCE {
                failures.push(format!(
                    "[{}「{}」] em 盒越出行盒底 {:+.3}px > {:.1}px",
                    case.name,
                    row.text.chars().take(8).collect::<String>(),
                    row.overflow_bottom_em,
                    STRICT_EM_BOX_SNAP_TOLERANCE
                ));
            }
            // S3b:实墨(atlas 中非透明像素)不越出行盒——「显示不全」的最终裁决。
            for (g, glyph) in row.glyphs.iter().zip(&placed.row.glyphs) {
                // The UV quad may include transparent rasterizer padding. Measure
                // actual coverage, rather than treating that padding as clipped ink.
                let uv = &glyph.uv_rect;
                let covered: Vec<_> = (usize::from(uv.min[1])..usize::from(uv.max[1]))
                    .filter(|&y| {
                        (usize::from(uv.min[0])..usize::from(uv.max[0]))
                            .any(|x| atlas[(x, y)].a() != 0)
                    })
                    .collect();
                if let (Some(&first), Some(&last)) = (covered.first(), covered.last()) {
                    let scale = uv.size.y / f32::from(uv.max[1] - uv.min[1]);
                    let top = g.baseline + uv.offset.y;
                    let ink_top = top + (first - usize::from(uv.min[1])) as f32 * scale;
                    let ink_bottom = top + (last + 1 - usize::from(uv.min[1])) as f32 * scale;
                    if row.top - ink_top > STRICT_INK_EPSILON {
                        failures.push(format!(
                            "[{}「{}」字符 {}] 实墨越出行盒顶 {:+.3}px",
                            case.name,
                            row.text.chars().take(8).collect::<String>(),
                            escape(g.ch),
                            row.top - ink_top
                        ));
                    }
                    if ink_bottom - row.bottom > STRICT_INK_EPSILON {
                        failures.push(format!(
                            "[{}「{}」字符 {}] 实墨越出行盒底 {:+.3}px",
                            case.name,
                            row.text.chars().take(8).collect::<String>(),
                            escape(g.ch),
                            ink_bottom - row.bottom
                        ));
                    }
                }
            }
        }
    }

    // wrap 文档的相邻行实墨净空 ≥ 0(不粘连/不互相遮挡)。
    let wrap_galley = render_case(&ctx, "m1-strict-wrap", WRAP_DOC);
    let wrap_rows = collect_row_forensics(&wrap_galley);
    let content: Vec<&RowForensics> = wrap_rows.iter().filter(|r| !r.glyphs.is_empty()).collect();
    for pair in content.windows(2) {
        let (prev, next) = (pair[0], pair[1]);
        if let (Some((_, prev_bottom)), Some((next_top, _))) = (prev.ink_extent, next.ink_extent) {
            let clearance = next_top - prev_bottom;
            if clearance < -STRICT_INK_EPSILON {
                failures.push(format!(
                    "[wrap 相邻行「{}」→「{}」] 上一行实墨压进下一行 {:+.3}px",
                    prev.text.chars().take(6).collect::<String>(),
                    next.text.chars().take(6).collect::<String>(),
                    -clearance
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "#43 严格断言失败(共 {} 项;已检 {} 行 + wrap {} 行):\n{}",
        failures.len(),
        checked_rows,
        content.len(),
        failures.join("\n")
    );
}
