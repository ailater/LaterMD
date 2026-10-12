//! 字体接入:M0 CJK 系统回退 + U1 Inter 三字重(docs/ui-modernization.md §3 U1)
//! + #43 M2 混排基线/行高修复(Inter 行 metrics override 副本 + 行高下限)。
//!
//! egui 内置字体只有拉丁字符,中文默认渲染为方块(tofu)。此处叠加两层:
//!
//! - **Inter(嵌入资源)**:Regular 挂 Proportional **首位**,拉丁字形优先
//!   命中 Inter;Medium/SemiBold 注册为独立 `FontFamily::Name` 供工具条/
//!   标题按字重取用(egui 无字重轴,权重即族名)。include_bytes 随包分发,
//!   SIL OFL 1.1,许可文本在 assets/fonts/LICENSE-Inter.txt。
//! - **CJK 系统回退**:按候选路径表读系统字体,追加到 Proportional /
//!   Monospace、两个 Inter 权重族与 `bold` 别名族(预览标题/加粗)的链尾。
//!   候选全失配时如实返回 `None`,
//!   界面显示警告,不静默吞掉;Inter 照装(嵌入资源不依赖系统)。
//!   不引 fontdb / font-kit 的定案见 docs/m0-report.md。
//!
//! egui 出厂链(Ubuntu-Light → NotoEmoji → emoji-icon-font)的**相对顺序
//! 不动**:Inter 没有 emoji 字形,NotoEmoji 仍是 emoji 的第一个命中;把
//! CJK 或 Inter 插到它前面会让 emoji 落到 CJK 字体或变豆腐块。
//!
//! # #43 M2:混排基线/行高修复(为什么有「override 副本」)
//!
//! epaint 的行 metrics 取**链头** face,行内 fallback 字形(汉字走链尾
//! CJK face)的基线由近似公式
//! `face_ascent + valign·(行高−line_height) + 0.5·(链头行高−face行高)`
//! 决定(`epaint text_layout.rs` 的字形垂直定位)。Inter(行高 1.21em)与
//! Noto Sans CJK(1.448em)的 ascent 占比与行高都不同,公式残差
//! ≈ 0.072em,再经整像素吸附放大为正文 1px、H1 2px 的可见基线错位 ——
//! 这就是「中文英文数字高低不一」的根因,且**与 line_height 覆盖值无关**
//! (残差只由两个 face 的表值决定)。
//!
//! 修复 = 把预览链头的 Inter Regular/SemiBold 注册为「行 metrics override
//! 副本」:运行时读本机命中的 CJK face 的 (ascent, descent, lineGap)
//! 表值,以 em 为单位改写到 Inter 副本的 hhea 与 OS/2 typo 表(等价 CSS
//! `@font-face { ascent-override / descent-override }` 的标准手法 ——
//! 布局 metrics 对齐,字形 outline 与光栅化不动)。链头与 fallback 的
//! 行 metrics 因此全等,基线公式残差精确归零;字形 atlas 不重复
//! (`GlyphCacheKey` 不含 ascent/descent,副本与原生 face 共享缓存)。
//!
//! 副本最初只挂预览专用族(`Inter-Preview` 正文族 + `bold` 别名族链头)。
//! **2026-10-12 UI 推广**(坤哥「标题栏/菜单里英文整体高于中文」):UI
//! 四链(Proportional/Medium/SemiBold/`bold`)同样吃到基线修复,但方向
//! 与 #50 同款 —— 改 **CJK fallback 副本**去就链头 Inter(链头表值
//! 分毫不动,纯 ASCII 行盒/换行逐像素不变),而非把 UI 链头换成大行盒
//! 副本(界面密度会全面变松)。代价是混排行的行盒从 CJK 的 1.448em 收
//! 到链头的 1.21em(汉字墨高 ~1.0em 仍充足),混排与纯 ASCII 行高从此
//! 一致。UI 链因此挂专属副本键(CJK 数据两套:UI 链 = CJK 就 Inter,
//! 预览链 = 链头就 CJK,原生表值条目只服务预览链)。macOS 路径
//! (`macos::install`)暂未跟进,菜单走原生 NSMenu、标题混排仍有残差。
//! 无 CJK 候选时不注册副本,各回落族行为与修复前一致。
//!
//! 行高(「显示不全」:行盒不足以容纳 CJK face 的行高需求)由 vendored
//! ①类修复承接:`MarkdownStyle::min_line_height_em` 行高下限,app 侧在
//! `theme` 应用样式时通过 [`line_height_floor_em`] 注入本机 CJK face 的
//! 实际行高。
//!
//! # #50 M1:编辑器专用等宽族(为什么 override 方向与预览相反)
//!
//! 源码编辑器的等宽渲染面(源码 TextEdit、行号槽、Live 活动块)走
//! `TextStyle::Monospace` 档,行 metrics 取 `FontFamily::Monospace` 链头
//! (egui 内置 Hack)出厂值;行内 fallback 的 CJK 字形基线由中点近似公式
//! 决定(见上节),Hack(行高 1.164em)与 Noto CJK(1.448em)的表值差
//! 在源码页放大为 1-3px 的可见基线错位(坤哥 2026-10-02「高低不一致」)。
//!
//! 修法与 #43 M2 同一套 override 机制、**方向相反**:预览把链头 Inter 的
//! 表值改写为 CJK 同款(行高随之变大,那是阅读排版想要的);编辑器则把
//! CJK 等宽 face **副本**的表值改写为链头 Hack 同款 em 值 —— 链头 metrics
//! 分毫不动,纯 ASCII 的行盒高与换行位置逐像素不变(否决线),而链头与
//! fallback 的行 metrics 全等后,基线公式残差同样精确归零。副本只挂
//! [`FAMILY_EDITOR_MONO`] 专用族且替换掉链尾原生 CJK 等宽条目(链内混入
//! 原生 face 会先命中、override 白做,与预览族的教训同款);字号仍走
//! `theme::apply_font_size` 的 `TextStyle::Monospace` 档投影,族由投影写
//! 入投影档(见 [`editor_mono_family`])。

use std::path::Path;
use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

#[cfg(target_os = "macos")]
mod macos;

static INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
static INTER_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Inter-Medium.ttf");
static INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");

/// Inter Regular 的 font_data 键,同时是 Proportional 链首名。
const NAME_REGULAR: &str = "Inter-Regular";
/// Inter SemiBold 的族名:工具条 / 标题等强调位用(正文 Regular)。
pub const FAMILY_SEMIBOLD: &str = "Inter-SemiBold";
/// vendored `egui_markdown` 探测的字面量族名:注册后表头/加粗走 SemiBold
/// 字重而非 `strong_text_color` 回落(后者 = active 控件前景,投影后是 accent
/// 蓝,会把 `**加粗**` 染成链接色;decisions-pending #48)。
pub const FAMILY_BOLD: &str = "bold";
/// Inter Medium 的族名:介于正文与强调之间,U1 注册备用、暂无消费者。
pub const FAMILY_MEDIUM: &str = "Inter-Medium";
/// 预览正文的专用族名:#43 M2 起链头是行 metrics override 过的 Inter
/// Regular 副本(与 CJK 回退 face 行 metrics 全等,混排基线对齐)。
pub const FAMILY_PREVIEW_BODY: &str = "Inter-Preview";
/// Inter Regular override 副本的 font_data 键。
const PREVIEW_REGULAR: &str = "Inter-Regular-Preview";
/// Inter SemiBold override 副本的 font_data 键(`bold` 别名族的链头)。
const PREVIEW_SEMIBOLD: &str = "Inter-SemiBold-Preview";
/// 编辑器专用等宽族(#50 M1):链头与 `FontFamily::Monospace` 同为内置
/// Hack(出厂行 metrics 不动,纯 ASCII 排版逐像素不变),链尾是 CJK 等宽
/// face 的「反向 override 副本」(行 metrics 改写为链头同款 em 值)——
/// 混排行的行内 fallback 基线因此与拉丁字形精确同线(见模块级注释)。
pub const FAMILY_EDITOR_MONO: &str = "editor-mono";
/// 编辑器族 CJK 回退的 font_data 键(`FAMILY_EDITOR_MONO` 专属副本;
/// 原生 `CJK_MONOSPACE` 不得入链,否则 CJK 先命中未修补的原生 face)。
const CJK_MONOSPACE_EDITOR: &str = "latermd-cjk-monospace-editor";
/// UI 四链(Proportional/Medium/SemiBold/`bold`)CJK 回退副本的
/// font_data 键:行 metrics 改写为链头 Inter Regular 同款 em 值(#43 M2
/// 的 UI 推广,2026-10-12;方向同 #50 —— 改 fallback 就链头,链头表值
/// 分毫不动)。原生 `CJK_PROPORTIONAL` 不得入 UI 链,否则 CJK 先命中
/// 未修补 face、override 白做。
const CJK_PROPORTIONAL_UI: &str = "latermd-cjk-proportional-ui";

/// sfnt 垂直排印相关的表值集合(font units,大端)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VerticalTables {
    pub(crate) units_per_em: u16,
    /// hhea (ascender, descender, lineGap)。
    pub(crate) hhea: (i16, i16, i16),
    /// OS/2 typo (sTypoAscender, sTypoDescender, sTypoLineGap)。
    pub(crate) typo: (i16, i16, i16),
    /// OS/2 win (usWinAscent, usWinDescent)。
    pub(crate) win: (u16, u16),
    /// OS/2 fsSelection bit7(USE_TYPO_METRICS)。
    pub(crate) use_typo_metrics: bool,
}

impl VerticalTables {
    /// skrifa(epaint 的字体解析后端)最终采纳的行 metrics:
    /// fsSelection bit7 置位 → OS/2 typo;否则 hhea;hhea 双零才回落 typo/win。
    pub(crate) fn selected(&self) -> (i16, i16, i16) {
        if self.use_typo_metrics {
            self.typo
        } else {
            self.hhea
        }
    }

    /// 采纳表值换算为 em 单位(#43 M2 的 override 目标/下限口径)。
    pub(crate) fn vertical_metrics_em(&self) -> VerticalMetricsEm {
        let scale = 1.0 / f32::from(self.units_per_em);
        let (asc, desc, gap) = self.selected();
        VerticalMetricsEm {
            ascent: f32::from(asc) * scale,
            descent: f32::from(desc) * scale,
            line_gap: f32::from(gap) * scale,
        }
    }
}

/// 垂直排印 metrics(em 单位)——跨 upem 可比的目标值口径。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VerticalMetricsEm {
    pub(crate) ascent: f32,
    /// 负值(位于基线下方的深度)。
    pub(crate) descent: f32,
    pub(crate) line_gap: f32,
}

impl VerticalMetricsEm {
    /// `ascent − descent + line_gap`(epaint StyledMetrics.row_height 同式)。
    pub(crate) fn row_height(self) -> f32 {
        self.ascent - self.descent + self.line_gap
    }
}

fn be_u16(data: &[u8], offset: usize) -> Option<u16> {
    let r = data.get(offset..offset + 2)?;
    Some(u16::from_be_bytes([r[0], r[1]]))
}

fn be_i16(data: &[u8], offset: usize) -> Option<i16> {
    be_u16(data, offset).map(|v| v as i16)
}

fn be_u32(data: &[u8], offset: usize) -> Option<u32> {
    let r = data.get(offset..offset + 4)?;
    Some(u32::from_be_bytes([r[0], r[1], r[2], r[3]]))
}

/// 解析一个 sfnt face(支持 .ttc 的 face index)的垂直排印表值。
/// 只读表头与固定字段,不解构 outline;失败返回 `None`(文件残缺/非 sfnt)。
pub(crate) fn parse_vertical_tables(data: &[u8], face_index: u32) -> Option<VerticalTables> {
    // ttcf: tag(u32) + version(u32) + numFonts(u32) + offsets[numFonts](u32);
    // 普通 sfnt: face 偏移 0。
    let face_offset = if data.get(0..4)? == b"ttcf" {
        let num_fonts = be_u32(data, 8)? as usize;
        let idx = face_index as usize;
        if idx >= num_fonts {
            return None;
        }
        be_u32(data, 12 + idx * 4)? as usize
    } else {
        0
    };
    let num_tables = be_u16(data, face_offset + 4)? as usize;
    let mut head = None;
    let mut hhea = None;
    let mut os2 = None;
    for i in 0..num_tables {
        let rec = face_offset + 12 + i * 16;
        let tag = data.get(rec..rec + 4)?;
        let offset = be_u32(data, rec + 8)? as usize;
        match tag {
            b"head" => head = Some(offset),
            b"hhea" => hhea = Some(offset),
            b"OS/2" => os2 = Some(offset),
            _ => {}
        }
    }
    let head = head?;
    let units_per_em = be_u16(data, head + 18)?;
    // hhea: version(u32) + ascender(i16)+4 + descender(i16)+6 + lineGap(i16)+8。
    let hhea_off = hhea?;
    let hhea_metrics = (
        be_i16(data, hhea_off + 4)?,
        be_i16(data, hhea_off + 6)?,
        be_i16(data, hhea_off + 8)?,
    );
    // OS/2: fsSelection(u16)+62、typo 三元组(i16)+68/70/72、win 二元组(u16)+74/76。
    let (typo, win, use_typo_metrics) = match os2 {
        Some(os2) => (
            (
                be_i16(data, os2 + 68)?,
                be_i16(data, os2 + 70)?,
                be_i16(data, os2 + 72)?,
            ),
            (be_u16(data, os2 + 74)?, be_u16(data, os2 + 76)?),
            be_u16(data, os2 + 62)? & (1 << 7) != 0,
        ),
        // 无 OS/2: skrifa 直接用 hhea。
        None => (hhea_metrics, (0, 0), false),
    };
    Some(VerticalTables {
        units_per_em,
        hhea: hhea_metrics,
        typo,
        win,
        use_typo_metrics,
    })
}

/// 把 face 的垂直排印布局 metrics(ascent/descent/lineGap)改写为目标值,
/// 返回字节副本。hhea 与 OS/2 typo 两表同步改写(Inter 置位 USE_TYPO_METRICS,
/// skrifa 采纳 typo;hhea 同步是为了两表口径一致,防采纳策略差异);
/// OS/2 win 表是实际渲染的剪裁参考而非布局 metrics,保持不动
/// (与 CSS `@font-face { ascent-override / descent-override }` 的语义一致:
/// 只对齐布局,字形 outline 与光栅化完全不变)。
///
/// 目标值以 em 为单位、按目标文件的 upem 量化到 i16 font units ——
/// 量化残差(≤0.5/upem em)远小于 epaint 布局的 1/32 点量化网格,
/// 在任何字号下都吸附到与目标 face 相同的取值。
///
/// 支持普通 sfnt(face_index 0,Inter/Hack 单 face)与 .ttc 集合内指定
/// face(#50 M1 的 Noto CJK 等宽 face):ttc 各 face 的表目录在其自有
/// 偏移处,目录内的表偏移按 sfnt 规则从**文件起点**计,与
/// [`parse_vertical_tables`] 同一口径。表目录里的 checksum 字段不重算:
/// 光栅化器(skrifa)不校验表 checksum,单测以实际 shaping 裁决 patch
/// 产物可正常解析。
pub(crate) fn override_vertical_metrics(
    data: &[u8],
    face_index: u32,
    target: VerticalMetricsEm,
) -> Option<Vec<u8>> {
    let face = parse_vertical_tables(data, face_index)?;
    let upem = f32::from(face.units_per_em);
    let to_units = |em: f32| {
        let v = (em * upem)
            .round()
            .clamp(f32::from(i16::MIN), f32::from(i16::MAX));
        v as i16
    };
    let asc = to_units(target.ascent);
    let desc = to_units(target.descent);
    let gap = to_units(target.line_gap);

    // ttc:目录在 face 自有偏移处;表偏移从文件起点计(补目录偏移会
    // 越界写坏共享表)。普通 sfnt 的 face_offset 为 0,与原行为一致。
    let face_offset = if data.get(0..4)? == b"ttcf" {
        let num_fonts = be_u32(data, 8)? as usize;
        let idx = face_index as usize;
        if idx >= num_fonts {
            return None;
        }
        be_u32(data, 12 + idx * 4)? as usize
    } else if face_index == 0 {
        0
    } else {
        return None;
    };
    let num_tables = be_u16(data, face_offset + 4)? as usize;
    let mut head = None;
    let mut hhea = None;
    let mut os2 = None;
    for i in 0..num_tables {
        let rec = face_offset + 12 + i * 16;
        let tag = data.get(rec..rec + 4)?;
        let offset = be_u32(data, rec + 8)? as usize;
        match tag {
            b"head" => head = Some(offset),
            b"hhea" => hhea = Some(offset),
            b"OS/2" => os2 = Some(offset),
            _ => {}
        }
    }
    // head 表存在性 = 合法 sfnt 的前置条件(units_per_em 的来源表);
    // 其数值字段(upem)在 parse 阶段已消费,patch 阶段无需再用。
    let _ = head?;
    let hhea = hhea?;
    let os2 = os2?;
    // 表长不校验(Inter 的 hhea/OS/2 均为定长标准表;越界写字节会 panic,
    // 而非静默产出残缺字体 —— 单 face ttf 实测字段位置恒在表内)。
    let mut out = data.to_vec();
    let put_i16 = |out: &mut Vec<u8>, off: usize, v: i16| {
        out[off..off + 2].copy_from_slice(&v.to_be_bytes());
    };
    // hhea: ascender +4 / descender +6 / lineGap +8。
    put_i16(&mut out, hhea + 4, asc);
    put_i16(&mut out, hhea + 6, desc);
    put_i16(&mut out, hhea + 8, gap);
    // OS/2 typo: sTypoAscender +68 / sTypoDescender +70 / sTypoLineGap +72。
    put_i16(&mut out, os2 + 68, asc);
    put_i16(&mut out, os2 + 70, desc);
    put_i16(&mut out, os2 + 72, gap);
    Some(out)
}

/// CJK 回退在 font_data 里的两个键(比例 / 等宽 face)。
const CJK_PROPORTIONAL: &str = "latermd-cjk-proportional";
const CJK_MONOSPACE: &str = "latermd-cjk-monospace";
/// CJK **粗体** face 的键:同族 Bold 变体(NotoSansCJK-Bold 等),只进
/// `bold` 族链 —— 中文加粗有真粗字重可命中,而不是落回 Regular face
/// 视觉零变化(2026-10-09 坤哥:「Live 模式加粗 ** 不渲染」;拉丁侧由
/// 链头 Inter SemiBold 覆盖,不受影响)。
const CJK_BOLD: &str = "latermd-cjk-bold";

/// 系统 CJK 候选表(路径 + `.ttc` 的比例/等宽 face 序号,按序取第一个可读
/// 文件):单一来源在 [`latermd_export::CJK_SYSTEM_CANDIDATES`],PDF 导出
/// 与本界面侧共用同一张表 —— 预览与导出命中同一个 face,中文观感一致。
/// face 序号的来历(Linux 两条经 `fc-query` 枚举,Windows/macOS 为资料
/// 建议值待真机核验)与逐条注释见 latermd-export 的 `pdf::cjk` 模块。
const CANDIDATES: &[latermd_export::CjkFontCandidate] = latermd_export::CJK_SYSTEM_CANDIDATES;

/// 注入字体,返回 CJK 回退的加载来源(用户可见);`None` = 候选全失配,
/// 中文将显示为方块 —— 但 Inter 三字重照常生效(嵌入资源,与系统无关)。
///
/// 命中 CJK 时同时解析其比例 face 的垂直排印表值(可能失败:文件结构
/// 非预期/表残缺 —— 此时 CJK 照挂、预览副本与行高下限均不生效,回退到
/// 修复前行为,基线偏差与行盒缺口保留,但不影响中文可显示)。
pub fn install(ctx: &egui::Context) -> Option<String> {
    #[cfg(target_os = "macos")]
    if let Some(source) = macos::install(ctx) {
        return Some(source);
    }
    for candidate in CANDIDATES {
        let (path, prop_idx, mono_idx) = (
            candidate.path,
            candidate.proportional_index,
            candidate.monospace_index,
        );
        if !Path::new(path).is_file() {
            continue;
        }
        // 读取失败(权限/竞态删除)跳到下一候选、不 panic;FontData::from_owned 只持有字节不做解析,.ttc 真正的解码在 set_fonts 之后的首帧、此处无从捕获
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let cjk_metrics =
            parse_vertical_tables(&bytes, prop_idx).map(|tables| tables.vertical_metrics_em());
        let cjk = cjk_metrics.map(|metrics| (&bytes[..], prop_idx, mono_idx, metrics));
        // 粗体变体:同族平行集合,face 序号复用比例 face;文件缺失/不可读
        // 静默降级(该平台无粗体 = 修复前行为)。
        let cjk_bold = candidate
            .bold_path
            .and_then(|bold| std::fs::read(bold).ok())
            .map(|bold| (bold, prop_idx));
        let defs = build_definitions(cjk, cjk_bold.as_ref().map(|(b, i)| (b[..].as_ref(), *i)));
        let editor_mono = defs
            .families
            .contains_key(&FontFamily::Name(Arc::from(FAMILY_EDITOR_MONO)));
        ctx.set_fonts(defs);
        mark_installed(ctx, cjk_metrics, editor_mono);
        return Some(format!(
            "{path} (比例 face {prop_idx} / 等宽 face {mono_idx}){}",
            if cjk_metrics.is_some() {
                String::new()
            } else {
                ";垂直表值解析失败,预览混排基线/行高修复未生效".to_owned()
            }
        ));
    }
    ctx.set_fonts(build_definitions(None, None));
    mark_installed(ctx, None, false);
    None
}

/// 构建字体定义:Inter 三字重 + 可选 CJK 回退。纯函数、不触 context ——
/// 链顺序单测由此注入假字节(字体解析发生在 set_fonts 之后的首帧,此处
/// 只排链)。`cjk_metrics` 存在且可解析时,额外注册两个「行 metrics override
/// 副本」并挂预览专用族(#43 M2,见模块级注释)。
fn build_definitions(
    cjk: Option<(&[u8], u32, u32, VerticalMetricsEm)>,
    cjk_bold: Option<(&[u8], u32)>,
) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    for (name, bytes) in [
        (NAME_REGULAR, INTER_REGULAR),
        (FAMILY_MEDIUM, INTER_MEDIUM),
        (FAMILY_SEMIBOLD, INTER_SEMIBOLD),
    ] {
        defs.font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    }
    // 出厂 Proportional 链(Ubuntu-Light → NotoEmoji → emoji-icon-font)整体
    // 后移一位:Inter 插首位,emoji 相对顺序不变。先取尾巴再插入。
    let fallback_tail: Vec<String> = defs.families[&FontFamily::Proportional].clone();
    defs.families
        .get_mut(&FontFamily::Proportional)
        .expect("出厂 Proportional 族恒存在")
        .insert(0, NAME_REGULAR.to_owned());
    // #43 M2 的 UI 推广(2026-10-12,坤哥「标题栏/菜单里英文整体高于
    // 中文」):UI 四链(Proportional/Medium/SemiBold/bold)的中英混排
    // 走同一套基线修复,方向与 #50 同款 —— CJK 副本的行 metrics 改写为
    // 链头 Inter 同款 em 值(链头表值分毫不动,纯 ASCII 行盒/换行逐像素
    // 不变),链头与 fallback 的 metrics 全等后基线公式残差精确归零。
    // 预览链方向相反(链头副本就 CJK,行盒更大),故 CJK 数据分两套:
    // UI 链挂这里的副本,预览/bold 预览链仍挂原生表值条目。改写失败
    // (系统字体表结构异常)回落原生条目,行为与修复前一致。
    let ui_cjk_name = if let Some((bytes, prop_idx, _, _)) = cjk {
        let regular_target = parse_vertical_tables(INTER_REGULAR, 0)
            .expect("嵌入 Inter-Regular 的垂直排印表结构恒可解析")
            .vertical_metrics_em();
        override_vertical_metrics(bytes, prop_idx, regular_target)
            .map(|patched| {
                defs.font_data.insert(
                    CJK_PROPORTIONAL_UI.to_owned(),
                    Arc::new(FontData {
                        index: prop_idx,
                        ..FontData::from_owned(patched)
                    }),
                );
                CJK_PROPORTIONAL_UI
            })
            .unwrap_or(CJK_PROPORTIONAL)
    } else {
        CJK_PROPORTIONAL
    };
    // `bold` 族不在此处理:UI 链随后会被预览段整体覆写(该族的链头本就
    // 是 CJK 表值的 SemiBold 副本,混排基线天然对齐,且无 UI 消费者)。
    // Medium/SemiBold 独立族:链头是对应字重,其余与 Proportional 同构 ——
    // emoji 链照旧,CJK 回退也挂尾(工具条/标题是中文,链尾无 CJK 会变方块)
    // `bold` 是 SemiBold 的别名族(vendored 渲染按字面量探测,decisions #48)
    for name in [FAMILY_MEDIUM, FAMILY_SEMIBOLD] {
        let mut chain = vec![name.to_owned()];
        chain.extend(fallback_tail.iter().cloned());
        defs.families
            .insert(FontFamily::Name(Arc::from(name)), chain);
    }
    // `bold` 族链(无 CJK 段):SemiBold 头 + 出厂尾巴;命中 CJK 时下面
    // 在头后插 CJK 粗体 face。CJK_BOLD 数据只在 CJK 分支注册,链条目必须
    // 与之同进退(cjk=None 而 cjk_bold=Some 会留下指向未注册数据的悬空链)。
    let mut bold_chain = vec![FAMILY_SEMIBOLD.to_owned()];
    if cjk.is_some() && cjk_bold.is_some() {
        bold_chain.push(CJK_BOLD.to_owned());
    }
    bold_chain.extend(fallback_tail.iter().cloned());
    defs.families
        .insert(FontFamily::Name(Arc::from(FAMILY_BOLD)), bold_chain);
    if let Some((bytes, prop_idx, mono_idx, metrics)) = cjk {
        let face = |index: u32| FontData {
            index,
            ..FontData::from_owned(bytes.to_vec())
        };
        defs.font_data
            .insert(CJK_PROPORTIONAL.to_owned(), Arc::new(face(prop_idx)));
        defs.font_data
            .insert(CJK_MONOSPACE.to_owned(), Arc::new(face(mono_idx)));
        // #43 M2:Inter Regular/SemiBold 的行 metrics override 副本(ascent/
        // descent/lineGap 改写为 CJK face 同款 em 值;字形 outline 不动)。
        // 副本只挂下面的预览专用族,原生族(UI 消费)不受影响。
        let preview_regular = override_vertical_metrics(INTER_REGULAR, 0, metrics)
            .expect("嵌入 Inter-Regular 的垂直排印表结构恒可解析");
        let preview_semibold = override_vertical_metrics(INTER_SEMIBOLD, 0, metrics)
            .expect("嵌入 Inter-SemiBold 的垂直排印表结构恒可解析");
        defs.font_data.insert(
            PREVIEW_REGULAR.to_owned(),
            Arc::new(FontData::from_owned(preview_regular)),
        );
        defs.font_data.insert(
            PREVIEW_SEMIBOLD.to_owned(),
            Arc::new(FontData::from_owned(preview_semibold)),
        );
        // 预览正文族:与 Proportional 完全同构,唯一差别是链头 —— 副本
        // Inter(行 metrics = CJK face 同款)替下原生 Inter。链内没有原生
        // Inter,拉丁字形才真正落到副本上(否则先命中原生、override 白做)。
        let mut preview_chain = vec![PREVIEW_REGULAR.to_owned()];
        preview_chain.extend(fallback_tail.iter().cloned());
        defs.families.insert(
            FontFamily::Name(Arc::from(FAMILY_PREVIEW_BODY)),
            preview_chain,
        );
        // `bold` 别名族链头换成 SemiBold 副本:预览标题/加粗的混排基线
        // 同样对齐;该族无 UI 消费者(格式条/标题栏走 semibold_family)。
        // CJK 粗体 face 紧随链头(先于出厂尾巴与链尾 Regular CJK):
        // 中文加粗命中真粗字重,拉丁仍由副本覆盖,CJK 缺字落回尾巴。
        let mut bold_chain = vec![PREVIEW_SEMIBOLD.to_owned()];
        if let Some((bold_bytes, bold_idx)) = cjk_bold {
            defs.font_data.insert(
                CJK_BOLD.to_owned(),
                Arc::new(FontData {
                    index: bold_idx,
                    ..FontData::from_owned(bold_bytes.to_vec())
                }),
            );
            bold_chain.push(CJK_BOLD.to_owned());
        }
        bold_chain.extend(fallback_tail.iter().cloned());
        defs.families
            .insert(FontFamily::Name(Arc::from(FAMILY_BOLD)), bold_chain);
        // UI 三链挂「链头对齐 CJK 副本」(见函数前段);预览正文族与
        // `bold` 族保持原生表值 CJK —— 它们的链头本来就是 CJK 表值副本
        // (方向相反),`bold` 族无 UI 消费者、链由本段预览部分构建。
        for (family, name) in [
            (FontFamily::Proportional, ui_cjk_name),
            (FontFamily::Monospace, CJK_MONOSPACE),
            (FontFamily::Name(Arc::from(FAMILY_MEDIUM)), ui_cjk_name),
            (FontFamily::Name(Arc::from(FAMILY_SEMIBOLD)), ui_cjk_name),
            // `bold` 别名族:预览标题(heading)与加粗文本经 vendored
            // apply_bold 切到它,链尾无 CJK 会整行变方块
            (FontFamily::Name(Arc::from(FAMILY_BOLD)), CJK_PROPORTIONAL),
            // 预览正文族的链尾同样挂 CJK 回退
            (
                FontFamily::Name(Arc::from(FAMILY_PREVIEW_BODY)),
                CJK_PROPORTIONAL,
            ),
        ] {
            // push 到末尾 = 回退:拉丁字符命中 Inter 后不再往下走
            defs.families
                .entry(family)
                .or_default()
                .push(name.to_owned());
        }
        // #50 M1:编辑器专用等宽族。#43 M2 的 override 手法反向应用 ——
        // 目标不是「链头对齐 CJK」而是「CJK 副本对齐链头」:链头(Hack)
        // 的行 metrics 就是纯 ASCII 行盒/换行的现状,否决线要求分毫不动;
        // 把 CJK 等宽 face 副本的表值改写为链头同款 em 值后,链头与
        // fallback 的行 metrics 全等,基线公式残差精确归零。目标表值来自
        // 内置 Hack(嵌入资源,恒可解析),源头是系统字体文件 —— 表值
        // 不合预期(如 ttc 结构异常)时整体跳过本族,回落 `Monospace`,
        // 行为与修复前一致,不 panic。
        let head_target = defs
            .font_data
            .get("Hack")
            .and_then(|head| parse_vertical_tables(head.font.as_ref(), head.index))
            .map(|tables| tables.vertical_metrics_em());
        if let Some(target) = head_target {
            if let Some(patched) = override_vertical_metrics(bytes, mono_idx, target) {
                defs.font_data.insert(
                    CJK_MONOSPACE_EDITOR.to_owned(),
                    Arc::new(FontData {
                        index: mono_idx,
                        ..FontData::from_owned(patched)
                    }),
                );
                // 与预览正文族同一纪律:链内不得残留原生 CJK 等宽条目,
                // 否则 CJK 先命中未修补的原生 face、override 白做;出厂链
                // 序(Hack → Ubuntu-Light → NotoEmoji → emoji-icon)原样
                // 保留,副本占原生 CJK 的链尾位置。
                let mut chain: Vec<String> = defs.families[&FontFamily::Monospace]
                    .iter()
                    .filter(|name| name.as_str() != CJK_MONOSPACE)
                    .cloned()
                    .collect();
                chain.push(CJK_MONOSPACE_EDITOR.to_owned());
                defs.families
                    .insert(FontFamily::Name(Arc::from(FAMILY_EDITOR_MONO)), chain);
            }
        }
    }
    defs
}

/// SemiBold 族(工具条 / 标题取字重用)。未注册的 `FontFamily::Name` 在
/// epaint 里直接 panic(`Font::font` 无 fallback),而大量无头 UI 测试不走
/// main 的 install —— 探测不到安装标志就回落 Proportional,不炸测试。
pub fn semibold_family(ctx: &egui::Context) -> FontFamily {
    let installed = ctx.data(|data| data.get_temp::<bool>(installed_id()).unwrap_or(false));
    if installed {
        FontFamily::Name(Arc::from(FAMILY_SEMIBOLD))
    } else {
        FontFamily::Proportional
    }
}

/// 预览正文的字体族:行 metrics override 副本已注册(`Inter-Preview` 族,
/// 链头与 CJK 回退的行 metrics 全等,混排基线对齐);否则回落
/// Proportional(本机无 CJK 候选/表值解析失败 —— 预览行为与修复前一致,
/// 代价是基线偏差与行盒缺口保留,如实可见而非静默换字体)。
pub fn preview_body_family(ctx: &egui::Context) -> FontFamily {
    let installed = ctx.data(|data| data.get_temp::<bool>(installed_id()).unwrap_or(false));
    let has_metrics = ctx.data(|data| data.get_temp::<Option<f32>>(floor_id()).flatten().is_some());
    if installed && has_metrics {
        FontFamily::Name(Arc::from(FAMILY_PREVIEW_BODY))
    } else {
        FontFamily::Proportional
    }
}

/// 编辑器等宽渲染面(源码 TextEdit、行号槽、Live 活动块)的字体族:
/// 专用族已注册时返回 [`FAMILY_EDITOR_MONO`](链头 = 内置 Hack 出厂
/// metrics,纯 ASCII 排版与 `FontFamily::Monospace` 逐像素一致;链尾 CJK
/// 副本行 metrics 对齐链头,混排基线对齐),否则回落 `FontFamily::
/// Monospace`(无 CJK 候选 / 表值解析失败 / 副本构建失败 —— 行为与修复
/// 前一致)。注册标志经 `install` 一次性写入 data 槽(每帧渲染读槽续期,
/// 与 `semibold_family`/`preview_body_family` 同一模式,避免走
/// `ctx.fonts` —— 首帧前 `theme.apply` 调不到它)。
pub fn editor_mono_family(ctx: &egui::Context) -> FontFamily {
    let registered = ctx.data(|data| data.get_temp::<bool>(editor_mono_id()).unwrap_or(false));
    if registered {
        FontFamily::Name(Arc::from(FAMILY_EDITOR_MONO))
    } else {
        FontFamily::Monospace
    }
}

/// 行高下限(em 倍数):本机命中的 CJK 回退 face 的实际行高
/// (`ascent − descent + lineGap`,如 Noto Sans CJK ≈ 1.448em)。
/// 预览渲染把它写进 vendored `MarkdownStyle::min_line_height_em`,
/// 行盒不足导致的越界墨迹遮挡(「显示不全」)由此兜底。
/// `None` = 无 CJK 候选或表值解析失败,调用方应保持 vendored 默认(无下限)。
pub fn line_height_floor_em(ctx: &egui::Context) -> Option<f32> {
    ctx.data(|data| {
        data.get_temp::<Option<f32>>(floor_id())
            .flatten()
            .filter(|em| em.is_finite() && *em > 0.0)
    })
}

fn installed_id() -> egui::Id {
    egui::Id::new("latermd-fonts-installed")
}

fn floor_id() -> egui::Id {
    egui::Id::new("latermd-cjk-line-height-floor-em")
}

/// [`FAMILY_EDITOR_MONO`] 注册标志的 data 槽键。
fn editor_mono_id() -> egui::Id {
    egui::Id::new("latermd-editor-mono-registered")
}

fn mark_installed(ctx: &egui::Context, cjk_metrics: Option<VerticalMetricsEm>, editor_mono: bool) {
    ctx.data_mut(|data| {
        data.insert_temp(installed_id(), true);
        data.insert_temp(
            floor_id(),
            cjk_metrics.map(|m| m.row_height()).filter(|em| *em > 0.0),
        );
        data.insert_temp(editor_mono_id(), editor_mono);
    });
}

/// 测试取证用:#43 M1 混排复现需要直接解析命中的 CJK 回退字体文件
/// (路径 + 两个 face index),与 `install` 同一条候选探测路径。
/// 未命中任何候选时返回 `None`(本机无 CJK 字体,如实跳过)。
#[cfg(test)]
pub(crate) fn cjk_source_for_test() -> Option<(std::path::PathBuf, u32, u32)> {
    #[cfg(target_os = "macos")]
    if let Some((path, regular, _)) = macos::cjk_source() {
        return Some((path, regular, regular));
    }
    CANDIDATES
        .iter()
        .find(|candidate| Path::new(candidate.path).is_file())
        .map(|candidate| {
            (
                std::path::PathBuf::from(candidate.path),
                candidate.proportional_index,
                candidate.monospace_index,
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::FontId;
    use std::collections::HashSet;

    /// `Path::is_absolute` 按宿主平台判定(Linux 上 `C:\...` 是相对路径),
    /// 故按目标平台语义判前缀:Unix 以 `/` 开头,Windows 为盘符路径。
    fn is_absolute_on_target_platform(path: &str) -> bool {
        path.starts_with('/') || path.as_bytes().get(1) == Some(&b':')
    }

    #[test]
    fn all_three_platforms_have_candidates() {
        let windows = CANDIDATES
            .iter()
            .filter(|c| c.path.starts_with("C:"))
            .count();
        let macos = CANDIDATES
            .iter()
            .filter(|c| c.path.starts_with("/System/"))
            .count();
        let linux = CANDIDATES
            .iter()
            .filter(|c| c.path.starts_with('/') && !c.path.starts_with("/System/"))
            .count();
        assert!(windows > 0, "Windows 候选为空");
        assert!(macos > 0, "macOS 候选为空");
        assert!(linux > 0, "Linux 候选为空");
    }

    #[test]
    fn candidate_paths_are_absolute_and_unique() {
        for candidate in CANDIDATES {
            assert!(
                is_absolute_on_target_platform(candidate.path),
                "非绝对路径: {}",
                candidate.path
            );
        }
        let uniq: HashSet<&str> = CANDIDATES.iter().map(|c| c.path).collect();
        assert_eq!(uniq.len(), CANDIDATES.len(), "候选路径存在重复");
    }

    #[test]
    fn windows_candidates_use_regular_face_zero() {
        // msyh / simhei / msyhbd 的 face 0 都是常规字型(1 为 UI/粗体变体);
        // 与 noto 不同,换 Windows 候选文件时沿用 0 前先重查
        for candidate in CANDIDATES {
            if candidate.path.starts_with("C:") {
                assert_eq!(
                    (candidate.proportional_index, candidate.monospace_index),
                    (0, 0),
                    "{} 偏离 face 0 约定",
                    candidate.path
                );
            }
        }
    }

    /// U1 验收:字体链顺序。Inter 恒在 Proportional 首位;出厂链
    /// (Ubuntu-Light → NotoEmoji → emoji-icon-font)相对顺序不变 ——
    /// NotoEmoji 仍是 emoji 的第一命中,不得被 Inter 或 CJK 挤后;CJK 回退
    /// 挂在各族的链尾。假字节不进 set_fonts,不触发解析。
    /// #43 M2 补充:预览专用族(`Inter-Preview` / `bold`)链头是 override
    /// 副本,且链内不得混入原生 Inter(否则拉丁先命中原生、override 白做)。
    #[test]
    fn inter_leads_and_emoji_order_survives() {
        // metrics 数值任意(排链阶段不消费字体字节,只取键名)。
        let fake = [0u8; 16];
        let metrics = VerticalMetricsEm {
            ascent: 1.16,
            descent: -0.288,
            line_gap: 0.0,
        };
        let defs = build_definitions(Some((&fake, 2, 7, metrics)), Some((&fake, 2)));
        let chain = |family: &FontFamily| -> &[String] {
            defs.families
                .get(family)
                .unwrap_or_else(|| panic!("{family:?} 未注册"))
        };
        let pos = |names: &[String], name: &str| {
            names
                .iter()
                .position(|n| n == name)
                .unwrap_or_else(|| panic!("{name} 不在链里: {names:?}"))
        };

        let prop = chain(&FontFamily::Proportional);
        assert_eq!(prop.first().map(String::as_str), Some(NAME_REGULAR));
        assert!(pos(prop, NAME_REGULAR) < pos(prop, "Ubuntu-Light"));
        assert!(pos(prop, "Ubuntu-Light") < pos(prop, "NotoEmoji-Regular"));
        assert!(pos(prop, "NotoEmoji-Regular") < pos(prop, "emoji-icon-font"));
        assert_eq!(prop.last().map(String::as_str), Some(CJK_PROPORTIONAL));
        // UI 共用的 Proportional 族不得被 override 副本污染(接口密度不变)。
        assert!(!prop.contains(&PREVIEW_REGULAR.to_owned()));

        // Inter 非等宽,不进 Monospace;CJK 照挂尾
        let mono = chain(&FontFamily::Monospace);
        assert!(!mono.iter().any(|n| n == NAME_REGULAR));
        assert_eq!(mono.last().map(String::as_str), Some(CJK_MONOSPACE));

        // 三字重数据齐全;权重族链头是对应字重,emoji 链与 CJK 回退同构
        for name in [
            NAME_REGULAR,
            FAMILY_MEDIUM,
            FAMILY_SEMIBOLD,
            PREVIEW_REGULAR,
            PREVIEW_SEMIBOLD,
        ] {
            assert!(defs.font_data.contains_key(name), "{name} 缺字体数据");
        }
        for (family, head) in [
            (FontFamily::Name(Arc::from(FAMILY_MEDIUM)), FAMILY_MEDIUM),
            (
                FontFamily::Name(Arc::from(FAMILY_SEMIBOLD)),
                FAMILY_SEMIBOLD,
            ),
        ] {
            let weight = chain(&family);
            assert_eq!(weight.first().map(String::as_str), Some(head));
            assert!(pos(weight, head) < pos(weight, "NotoEmoji-Regular"));
            assert!(pos(weight, "NotoEmoji-Regular") < pos(weight, "emoji-icon-font"));
            assert_eq!(weight.last().map(String::as_str), Some(CJK_PROPORTIONAL));
        }

        // 预览正文族:链头 = override Regular 副本,链内无原生 Inter,
        // emoji 相对顺序与出厂一致,CJK 挂尾(与 Proportional 完全同构)。
        let preview = chain(&FontFamily::Name(Arc::from(FAMILY_PREVIEW_BODY)));
        assert_eq!(preview.first().map(String::as_str), Some(PREVIEW_REGULAR));
        assert!(!preview.iter().any(|n| n == NAME_REGULAR));
        assert!(pos(preview, "NotoEmoji-Regular") < pos(preview, "emoji-icon-font"));
        assert_eq!(preview.last().map(String::as_str), Some(CJK_PROPORTIONAL));

        // `bold` 别名族(#43 M2 起链头 = override SemiBold 副本):预览标题与
        // 加粗文本经 vendored apply_bold 切到它,链内同样不得有原生 SemiBold。
        // CJK 粗体 face 紧随链头、先于链尾 Regular CJK(2026-10-09 修复:
        // 中文加粗此前落 Regular face 视觉零变化):
        //   头(SemiBold 副本,拉丁) → CJK_BOLD(中文真粗) → 出厂尾巴 → CJK Regular
        let bold = chain(&FontFamily::Name(Arc::from(FAMILY_BOLD)));
        assert_eq!(bold.first().map(String::as_str), Some(PREVIEW_SEMIBOLD));
        assert!(!bold.iter().any(|n| n == FAMILY_SEMIBOLD));
        assert_eq!(bold.last().map(String::as_str), Some(CJK_PROPORTIONAL));
        assert_eq!(bold.get(1).map(String::as_str), Some(CJK_BOLD));
        assert!(pos(bold, CJK_BOLD) < pos(bold, CJK_PROPORTIONAL));
        assert!(
            defs.font_data.get(CJK_BOLD).is_some_and(|d| d.index == 2),
            "CJK 粗体 face 序号复用比例 face(平行集合)"
        );
        assert!(
            defs.font_data
                .get(PREVIEW_SEMIBOLD)
                .is_some_and(|d| d.index == 0),
            "Inter 副本是单 face ttf,index 应为 0"
        );

        // #50 M1 降级:假 CJK 字节(非 sfnt)让编辑器族副本构建失败,
        // 整族跳过而非 panic(其余族照常注册 —— 源头是系统文件,结构
        // 异常时回落 `Monospace` 现状)。
        assert!(
            !defs
                .families
                .contains_key(&FontFamily::Name(Arc::from(FAMILY_EDITOR_MONO))),
            "副本构建失败时编辑器族不应注册"
        );
    }

    /// 候选全失配(`None`):Inter 仍注册(嵌入资源),族链上无 CJK 条目;
    /// #43 M2:预览 override 副本与预览正文族**都不注册**(无 metrics 来源),
    /// `bold` 族链头保持原生 SemiBold —— 行为与修复前的降级完全一致。
    #[test]
    fn inter_registers_even_without_cjk() {
        let defs = build_definitions(None, None);
        assert_eq!(
            defs.families[&FontFamily::Proportional]
                .first()
                .map(String::as_str),
            Some(NAME_REGULAR)
        );
        for family in [
            FontFamily::Proportional,
            FontFamily::Monospace,
            FontFamily::Name(Arc::from(FAMILY_MEDIUM)),
            FontFamily::Name(Arc::from(FAMILY_SEMIBOLD)),
            FontFamily::Name(Arc::from(FAMILY_BOLD)),
        ] {
            assert!(
                !defs.families[&family].iter().any(|n| n == CJK_PROPORTIONAL),
                "{family:?} 不应残留 CJK 条目"
            );
        }
        assert!(
            !defs
                .families
                .contains_key(&FontFamily::Name(Arc::from(FAMILY_PREVIEW_BODY))),
            "无 CJK 时不应注册预览正文族"
        );
        for key in [PREVIEW_REGULAR, PREVIEW_SEMIBOLD] {
            assert!(!defs.font_data.contains_key(key), "{key} 不应注册");
        }
        assert_eq!(
            defs.families[&FontFamily::Name(Arc::from(FAMILY_BOLD))]
                .first()
                .map(String::as_str),
            Some(FAMILY_SEMIBOLD),
            "无 CJK 时 bold 族链头保持原生 SemiBold"
        );
        assert!(
            !defs
                .families
                .contains_key(&FontFamily::Name(Arc::from(FAMILY_EDITOR_MONO))),
            "无 CJK 时不应注册编辑器专用等宽族"
        );
    }

    /// CJK 粗体链位(2026-10-09 修复回归):有粗体变体时,`bold` 族链
    /// [头=CJK_BOLD=…=链尾 Regular] 次序成立;无粗体变体时链上无
    /// CJK_BOLD(行为 = 修复前,中文加粗回落 Regular —— 平台无粗体
    /// 文件时的诚实降级)。两个方向都钉:链序错(粗体在 Regular 后)
    /// 与「有文件却没挂」都当场红。
    #[test]
    fn cjk_bold_sits_after_head_before_regular_fallback() {
        let fake = [0u8; 16];
        let metrics = VerticalMetricsEm {
            ascent: 1.16,
            descent: -0.288,
            line_gap: 0.0,
        };
        // 有粗体:链 [头=CJK_BOLD=…=链尾 Regular] 次序成立
        let defs = build_definitions(Some((&fake, 2, 7, metrics)), Some((&fake, 2)));
        let chain = &defs.families[&FontFamily::Name(Arc::from(FAMILY_BOLD))];
        let pos = |name: &str| {
            chain
                .iter()
                .position(|n| n == name)
                .unwrap_or_else(|| panic!("{name} 不在 bold 链里:{chain:?}"))
        };
        assert!(pos(CJK_BOLD) < pos(CJK_PROPORTIONAL));
        assert!(pos(PREVIEW_SEMIBOLD) < pos(CJK_BOLD));
        // 无粗体(如文泉驿/macOS):链上零 CJK_BOLD,其余结构与修复前一致
        let defs = build_definitions(Some((&fake, 0, 0, metrics)), None);
        let chain = &defs.families[&FontFamily::Name(Arc::from(FAMILY_BOLD))];
        assert!(!chain.iter().any(|n| n == CJK_BOLD));
        assert_eq!(chain.last().map(String::as_str), Some(CJK_PROPORTIONAL));
        assert!(!defs.font_data.contains_key(CJK_BOLD), "无粗体时不注册数据");
    }

    /// install 端到端:无论本机有无 CJK 候选,context 里 Proportional 首位
    /// 恒为 Inter;装完 `semibold_family` 不再回落。
    #[test]
    fn install_puts_inter_first_in_context() {
        let ctx = egui::Context::default();
        let cjk = install(&ctx);
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        ctx.fonts(|f| {
            let prop = &f.definitions().families[&FontFamily::Proportional];
            assert_eq!(prop.first().map(String::as_str), Some(NAME_REGULAR));
            match cjk {
                // 本机 CJK 命中时链尾是「链头对齐 CJK 副本」(#43 M2 的 UI
                // 推广);原生表值条目只留在预览链。
                Some(_) => {
                    assert_eq!(prop.last().map(String::as_str), Some(CJK_PROPORTIONAL_UI));
                    assert!(!prop.iter().any(|n| n == CJK_PROPORTIONAL));
                }
                None => assert!(prop.iter().all(|n| n != CJK_PROPORTIONAL_UI)),
            }
        });
        assert_eq!(
            semibold_family(&ctx),
            FontFamily::Name(Arc::from(FAMILY_SEMIBOLD))
        );
    }

    /// #43 M2:override 副本的表值改写正确性(纯字节级,不依赖系统字体)。
    /// 目标 = Noto Sans CJK 的 (ascent 1.16 / descent −0.288 / gap 0) em,
    /// 在 Inter 的 2048 upem 下量化;patch 后 hhea 与 OS/2 typo 同步等于量化值,
    /// win 表与 fsSelection(bit7)保持不动 —— 光栅化不受影响,采纳表仍是 typo。
    #[test]
    fn override_vertical_metrics_rewrites_layout_tables_only() {
        let target = VerticalMetricsEm {
            ascent: 1.16,
            descent: -0.288,
            line_gap: 0.0,
        };
        let to_units = |em: f32| (em * 2048.0).round() as i16;
        for (name, bytes) in [
            ("Inter-Regular", INTER_REGULAR),
            ("Inter-SemiBold", INTER_SEMIBOLD),
        ] {
            let original = parse_vertical_tables(bytes, 0).expect("{name} 原始表解析失败");
            let patched_bytes = override_vertical_metrics(bytes, 0, target)
                .unwrap_or_else(|| panic!("{name} patch 失败"));
            let patched =
                parse_vertical_tables(&patched_bytes, 0).expect("{name} patch 后表解析失败");
            assert_eq!(
                patched.units_per_em, original.units_per_em,
                "{name} upem 不应被改"
            );
            assert_eq!(
                patched.use_typo_metrics, original.use_typo_metrics,
                "{name} fsSelection 不应被改"
            );
            assert_eq!(
                patched.win, original.win,
                "{name} win 表(渲染剪裁参考)不应被改"
            );
            let want = (
                to_units(target.ascent),
                to_units(target.descent),
                to_units(target.line_gap),
            );
            assert_eq!(patched.hhea, want, "{name} hhea 未改写为目标值");
            assert_eq!(patched.typo, want, "{name} OS/2 typo 未改写为目标值");
            assert_eq!(patched.selected(), want, "{name} skrifa 采纳表未对齐");
            let diff_positions: Vec<usize> = bytes
                .iter()
                .zip(patched_bytes.iter())
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .map(|(i, _)| i)
                .collect();
            // diff 只允许出现在两张表的 6 个 i16 字段(hhea +4/+6/+8 与
            // OS/2 typo +68/+70/+72)—— 按位置对照,而非仅数量:除此之外
            // 全文件逐字节相同 = 字形 outline/字宽/cmap 完全未动的硬证据。
            let table_offset = |tag: &[u8; 4]| -> usize {
                let num = be_u16(bytes, 4).expect("numTables") as usize;
                (0..num)
                    .map(|i| 12 + i * 16)
                    .find(|rec| bytes.get(*rec..rec + 4) == Some(tag.as_slice()))
                    .map(|rec| be_u32(bytes, rec + 8).expect("表偏移") as usize)
                    .unwrap_or_else(|| panic!("{name} 缺 {tag:?} 表"))
            };
            let hhea = table_offset(b"hhea");
            let os2 = table_offset(b"OS/2");
            let mut expected: Vec<usize> =
                [hhea + 4, hhea + 6, hhea + 8, os2 + 68, os2 + 70, os2 + 72]
                    .into_iter()
                    .flat_map(|off| [off, off + 1])
                    .collect();
            expected.sort_unstable();
            // 子集断言(非相等):字段值与原值碰巧共享高位/低位字节时,
            // 该字节的 diff 为空(如 lineGap 0→0 全等)——只要改动不出
            // 6 个字段之外,字形 outline/字宽/cmap 就是完全未动的。
            assert!(
                diff_positions.iter().all(|p| expected.contains(p)),
                "{name} 改动越出了 hhea/typo 的 6 个 i16 字段:diff={diff_positions:?} 字段位={expected:?}"
            );
            assert!(
                !diff_positions.is_empty(),
                "{name} 目标值与原值全等,patch 未生效"
            );
        }
    }

    /// #43 M2 install 端到端:本机有 CJK 候选时,预览正文族与 `bold` 族的
    /// 链头 face 在 context 里的实测行 metrics(ascent/行高)== CJK 回退
    /// face 同款 —— 这既是「patch 产物可正常解析、shaping 正常」的裁决,
    /// 也是「链头与 fallback 行 metrics 全等 → 基线公式残差归零」的直接前提。
    /// 同时核对行高下限值(CJK face 实际行高的 em 值)。无 CJK 候选的机器上
    /// 如实跳过(CI runner 可能没有;三平台真机核验在 #43 人工清单)。
    #[test]
    fn install_aligns_preview_family_metrics_with_cjk_fallback() {
        let ctx = egui::Context::default();
        let cjk = install(&ctx);
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();

        let Some((path, prop_idx, _)) = cjk_source_for_test() else {
            eprintln!("本机无 CJK 候选字体,预览族 metrics 对齐断言跳过");
            assert!(cjk.is_none(), "候选字体存在但 install 返回 None?");
            assert_eq!(preview_body_family(&ctx), FontFamily::Proportional);
            assert_eq!(
                line_height_floor_em(&ctx),
                None,
                "无 CJK 时行高下限应为 None"
            );
            return;
        };
        let cjk_bytes = std::fs::read(path).expect("候选字体文件读取失败");
        let cjk_metrics = parse_vertical_tables(&cjk_bytes, prop_idx)
            .expect("已确认本机有 CJK")
            .vertical_metrics_em();

        let size = 13.0;
        let probe = |family: FontFamily| {
            let font = FontId::new(size, family);
            let galley =
                ctx.fonts_mut(|f| f.layout_no_wrap("中H".to_owned(), font, egui::Color32::WHITE));
            let glyphs = &galley.rows[0].row.glyphs;
            let face = |ch: char| {
                glyphs
                    .iter()
                    .find(|g| g.chr == ch)
                    .unwrap_or_else(|| panic!("{ch} 未被 shaping"))
            };
            (
                face('H').font_face_ascent,
                face('H').font_face_height,
                face('中').font_face_ascent,
                face('中').font_face_height,
            )
        };

        // 预览正文族:链头(override Inter)与 CJK 回退的 ascent/行高逐项相等。
        let (head_asc, head_h, cjk_asc, cjk_h) =
            probe(FontFamily::Name(Arc::from(FAMILY_PREVIEW_BODY)));
        assert_eq!(
            head_asc, cjk_asc,
            "预览族链头 ascent 应与 CJK face 全等(基线对齐前提)"
        );
        assert_eq!(
            head_h, cjk_h,
            "预览族链头行高应与 CJK face 全等(基线对齐前提)"
        );
        // 与表值推算对账:styled_metrics 对 ascent/descent/lineGap 各自做
        // 1/32 网格量化后再合成行高(不二次量化),推算须同式,否则会拿
        // 「量化合成值」错比「合成再量化值」(18.84375 vs 18.8125 的差源)。
        let round_ui = |v: f32| (v * 32.0).round() / 32.0;
        let expect_ascent = round_ui(cjk_metrics.ascent * size);
        let expect_h = round_ui(cjk_metrics.ascent * size) - round_ui(cjk_metrics.descent * size)
            + round_ui(cjk_metrics.line_gap * size);
        assert!(
            (head_asc - expect_ascent).abs() < 0.02,
            "链头 ascent {head_asc} 与表值 {expect_ascent} 不符"
        );
        assert!(
            (head_h - expect_h).abs() < 0.02,
            "链头行高 {head_h} 与表值 {expect_h} 不符"
        );

        // `bold` 别名族(预览标题/加粗)同样对齐。
        let (b_head_asc, b_head_h, b_cjk_asc, b_cjk_h) =
            probe(FontFamily::Name(Arc::from(FAMILY_BOLD)));
        assert_eq!(
            b_head_asc, b_cjk_asc,
            "bold 族链头 ascent 应与 CJK face 全等"
        );
        assert_eq!(b_head_h, b_cjk_h, "bold 族链头行高应与 CJK face 全等");

        // UI 原生族不受影响:Proportional 链头仍是原生 Inter 的行 metrics
        // (Inter 出厂表值 0.96875 / 1.20996em)。
        let (p_head_asc, p_head_h, _, _) = probe(FontFamily::Proportional);
        let native = if cfg!(target_os = "macos") {
            std::fs::read("/System/Library/Fonts/SFNS.ttf").expect("macOS system font")
        } else {
            INTER_REGULAR.to_vec()
        };
        let native = parse_vertical_tables(&native, 0)
            .unwrap()
            .vertical_metrics_em();
        assert!(
            (p_head_asc - round_ui(native.ascent * size)).abs() < 0.03,
            "Proportional 链头 ascent 不应被修复改动"
        );
        assert!(
            (p_head_h
                - (round_ui(native.ascent * size) - round_ui(native.descent * size)
                    + round_ui(native.line_gap * size)))
            .abs()
                < 0.03,
            "Proportional 链头行高不应被修复改动"
        );

        // 行高下限 = CJK face 实际行高;预览正文族可用。
        let floor = line_height_floor_em(&ctx).expect("有 CJK 时行高下限应存在");
        assert!(
            (floor - cjk_metrics.row_height()).abs() < 1e-4,
            "下限 {floor} 与 CJK face 行高 {} 不符",
            cjk_metrics.row_height()
        );
        assert_eq!(
            preview_body_family(&ctx),
            FontFamily::Name(Arc::from(FAMILY_PREVIEW_BODY))
        );
    }

    /// U1 验收:Inter 与 CJK 混排的基线/行高对齐(无头 galley 布局对比)。
    /// egui 的行 metrics 取**链头**字体(此处 Inter),fallback 字形(Noto
    /// CJK)共用同一基线与行框 —— 断言纯拉丁 / 纯 CJK / 混排三条 galley
    /// 单行且 rect 高度一致,即混排不分行、行高不跳变。CJK 侧断言依赖本机
    /// 候选命中(CI runner 可能无 CJK 字体,如实跳过);三平台真机目视
    /// 验收待人工(docs/ui-modernization.md §2.2)。
    #[test]
    fn mixed_script_shares_row_height_and_baseline() {
        let ctx = egui::Context::default();
        let has_cjk = install(&ctx).is_some();
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        let font = egui::FontId::new(14.0, FontFamily::Proportional);
        let layout = |text: &str| {
            ctx.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE))
        };

        let latin = layout("LaterMD Inter");
        let mixed = layout("LaterMD 中文混排 Inter");
        for (name, galley) in [("latin", &latin), ("mixed", &mixed)] {
            assert_eq!(galley.rows.len(), 1, "{name}: 不换行(宽度无限)");
            assert!(
                galley.rect.height().is_finite() && galley.rect.height() > 0.0,
                "{name}: 行高无效 {}",
                galley.rect.height()
            );
        }
        assert_eq!(
            latin.rect.height(),
            mixed.rect.height(),
            "混排不得改变行高(fallback 字形共用链头 metrics)"
        );

        if !has_cjk {
            eprintln!("本机无 CJK 候选字体,CJK 侧断言跳过");
            return;
        }
        let cjk = layout("中文字体测试");
        assert_eq!(
            cjk.rect.height(),
            latin.rect.height(),
            "CJK 也吃链头 Inter 的行 metrics"
        );
        assert!(
            ctx.fonts_mut(|f| f.has_glyphs(&font, "中文混排")),
            "CJK 回退链生效,不出豆腐块"
        );
        // 权重族也要能显示中文:工具条/标题是中文,链尾无 CJK 会变方块
        let semibold = egui::FontId::new(14.0, semibold_family(&ctx));
        assert!(
            ctx.fonts_mut(|f| f.has_glyphs(&semibold, "标题")),
            "SemiBold 族的 CJK 回退链生效"
        );
        // bold 别名族:预览 heading/加粗经 vendored apply_bold 切到它,
        // 漏挂 CJK 曾让标题中文整行变方块
        let bold = egui::FontId::new(14.0, FontFamily::Name(Arc::from(FAMILY_BOLD)));
        assert!(
            ctx.fonts_mut(|f| f.has_glyphs(&bold, "标题加粗")),
            "bold 别名族的 CJK 回退链生效"
        );
    }

    /// UI 混排基线对齐的端到端验收:install 后,Proportional 链行内
    /// 「中文+拉丁」混排的逐字形基线原点必须全等(#43 M2 公式残差归零的
    /// 直接可观测面 —— 坤哥 2026-10-12「LaterMD 与未命名不在一条线、
    /// 英文比中文高」)。无 CJK 候选时跳过(与链顺序测试同一豁免口径)。
    #[test]
    fn ui_mixed_run_glyph_baselines_align() {
        use eframe::egui::Color32;
        let ctx = egui::Context::default();
        let cjk = install(&ctx);
        if cjk.is_none() {
            eprintln!("本机无 CJK 候选字体,基线对齐断言跳过");
            return;
        }
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        let mut ys = Vec::new();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let base = egui::TextStyle::Button.resolve(ui.style());
            let mut job = egui::text::LayoutJob::default();
            job.append(
                "文件",
                0.0,
                egui::text::TextFormat::simple(base.clone(), Color32::WHITE),
            );
            job.append(
                "(F)",
                0.0,
                egui::text::TextFormat::simple(base, Color32::WHITE),
            );
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            for glyph in &galley.rows[0].glyphs {
                ys.push((glyph.chr, glyph.pos.y));
            }
        })
        .drop_without_applying_deltas();
        assert!(ys.len() >= 4, "混排行应有字形: {ys:?}");
        let cjk_y = ys.iter().find(|(c, _)| *c == '文').expect("中文字形").1;
        for (chr, y) in &ys {
            assert_eq!(y, &cjk_y, "字形 '{chr}' 的基线与中文不一致");
        }
    }

    /// #43 M2 的 UI 推广:UI 四链(Proportional/Medium/SemiBold/`bold`)
    /// 链尾是「链头对齐 CJK 副本」,副本采纳表值 == 链头 Inter 采纳表值
    /// (em),链内不残留原生表值 CJK 条目(否则先命中未修补 face、
    /// override 白做);预览正文族方向相反,仍挂原生表值条目。纯 build
    /// 层测试,用嵌入 Inter 字节充当 CJK 源字节(合法 sfnt,patch 必然
    /// 成功,不依赖本机字体)。
    #[test]
    fn ui_chains_fallback_to_head_aligned_cjk_copy() {
        let fake = VerticalMetricsEm {
            ascent: 1.16,
            descent: -0.288,
            line_gap: 0.0,
        };
        let defs = build_definitions(Some((INTER_REGULAR, 0, 0, fake)), Some((INTER_REGULAR, 0)));
        for family in [
            FontFamily::Proportional,
            FontFamily::Name(Arc::from(FAMILY_MEDIUM)),
            FontFamily::Name(Arc::from(FAMILY_SEMIBOLD)),
        ] {
            let chain = &defs.families[&family];
            assert_eq!(
                chain.last().map(String::as_str),
                Some(CJK_PROPORTIONAL_UI),
                "{family:?} 链尾 = UI 对齐副本"
            );
            assert!(
                !chain.iter().any(|n| n == CJK_PROPORTIONAL),
                "{family:?} 链内不得残留原生表值 CJK 条目"
            );
        }
        // UI 副本的采纳表值 == 链头 Inter 采纳表值(em),基线残差因此归零。
        let head = parse_vertical_tables(INTER_REGULAR, 0)
            .expect("Inter-Regular 表可解析")
            .vertical_metrics_em();
        let patched = &defs.font_data[CJK_PROPORTIONAL_UI];
        let got = parse_vertical_tables(patched.font.as_ref(), patched.index)
            .expect("patch 产物可解析")
            .vertical_metrics_em();
        assert_eq!(got, head, "UI 副本行 metrics == 链头 Inter");
        // 预览正文族方向相反:链尾仍是原生表值条目。
        let preview = &defs.families[&FontFamily::Name(Arc::from(FAMILY_PREVIEW_BODY))];
        assert_eq!(preview.last().map(String::as_str), Some(CJK_PROPORTIONAL));
    }

    /// #50 M1:编辑器专用等宽族的链结构与字节级正确性(纯 build 层,用
    /// 嵌入 Inter 字节充当 CJK 源字节 —— 它们是合法 sfnt,patch 必然成
    /// 功,不依赖本机字体)。链头 = 内置 Hack(与 `FontFamily::Monospace`
    /// 同 face 同 metrics,纯 ASCII 排版逐像素一致的前提);链内不得残留
    /// 原生 CJK 等宽条目(否则先命中未修补 face,override 白做);emoji
    /// 出厂链序保留;副本的表值 = Hack 采纳表值按副本 upem 量化。
    #[test]
    fn editor_mono_family_chain_and_patched_tables() {
        let fake = VerticalMetricsEm {
            ascent: 1.16,
            descent: -0.288,
            line_gap: 0.0,
        };
        let defs = build_definitions(Some((INTER_REGULAR, 0, 0, fake)), None);
        let family = FontFamily::Name(Arc::from(FAMILY_EDITOR_MONO));
        let chain = defs
            .families
            .get(&family)
            .unwrap_or_else(|| panic!("{FAMILY_EDITOR_MONO} 未注册"));

        let factory_mono = &FontDefinitions::default().families[&FontFamily::Monospace];
        let expected: Vec<String> = factory_mono
            .iter()
            .filter(|n| n.as_str() != CJK_MONOSPACE)
            .cloned()
            .chain([CJK_MONOSPACE_EDITOR.to_owned()])
            .collect();
        assert_eq!(chain, &expected, "链 = 出厂等宽链 - 原生CJK + 副本");
        assert_eq!(
            chain.first().map(String::as_str),
            Some("Hack"),
            "链头保持内置 Hack"
        );
        assert_eq!(chain.last().map(String::as_str), Some(CJK_MONOSPACE_EDITOR));
        assert!(
            !chain.iter().any(|n| n == CJK_MONOSPACE),
            "原生 CJK 等宽条目不得入链"
        );
        assert!(
            pos_in(chain, "NotoEmoji-Regular") < pos_in(chain, "emoji-icon-font"),
            "emoji 相对顺序保留"
        );

        // 副本字节:采纳表值 == Hack 采纳表值按副本 upem(Inter 2048)量化。
        let hack = defs.font_data.get("Hack").expect("内置 Hack 存在");
        let target = parse_vertical_tables(hack.font.as_ref(), 0)
            .expect("Hack 表恒可解析")
            .vertical_metrics_em();
        let copy = defs
            .font_data
            .get(CJK_MONOSPACE_EDITOR)
            .expect("副本已注册");
        let patched = parse_vertical_tables(copy.font.as_ref(), copy.index).expect("副本可解析");
        let to_units = |em: f32| (em * f32::from(patched.units_per_em)).round() as i16;
        let want = (
            to_units(target.ascent),
            to_units(target.descent),
            to_units(target.line_gap),
        );
        assert_eq!(
            patched.selected(),
            want,
            "副本采纳表值应等于链头 em 值的量化(基线归零的前提)"
        );
        let original = parse_vertical_tables(INTER_REGULAR, 0).expect("stand-in 原始表解析失败");
        assert_eq!(
            patched.use_typo_metrics, original.use_typo_metrics,
            "采纳策略(fsSelection bit7)不应被翻转"
        );
    }

    fn pos_in(chain: &[String], name: &str) -> usize {
        chain
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| panic!("{name} 不在链里: {chain:?}"))
    }

    /// #50 M1 install 端到端:注册标志落槽、`editor_mono_family` 不再回落、
    /// 新族在投影字号下 CJK 可见(不豆腐)、**混排基线偏差精确为 0** 且
    /// 纯 ASCII 行盒高与 `FontFamily::Monospace` 完全一致。本机无 CJK 候选
    /// 时如实跳过(降级语义由 [`inter_registers_even_without_cjk`] 钉住)。
    #[test]
    fn install_registers_editor_mono_family_with_aligned_baseline() {
        let ctx = egui::Context::default();
        let cjk = install(&ctx);
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();

        if cjk.is_none() {
            eprintln!("本机无 CJK 候选字体,编辑器族端到端断言跳过");
            assert_eq!(editor_mono_family(&ctx), FontFamily::Monospace);
            return;
        }
        assert_eq!(
            editor_mono_family(&ctx),
            FontFamily::Name(Arc::from(FAMILY_EDITOR_MONO))
        );

        let size = 15.0;
        let editor = egui::FontId::new(size, editor_mono_family(&ctx));
        ctx.fonts_mut(|f| assert!(f.has_glyphs(&editor, "中文混排"), "CJK 回退在链上,不出豆腐"));

        // 混排基线:同一行内 CJK 与拉丁字形的布局基线(placed.pos.y +
        // glyph.pos.y,#43 M1 同一度量)逐项相等 —— 链头与 fallback 行
        // metrics 全等的直接读数。
        let baseline_gap = |font: &egui::FontId| {
            ctx.fonts_mut(|f| {
                let galley = f.layout_no_wrap(
                    "甲post中文123".to_owned(),
                    font.clone(),
                    egui::Color32::WHITE,
                );
                baseline_cjk_minus_latin(&galley)
            })
        };
        assert_eq!(
            baseline_gap(&editor),
            Some(0.0),
            "新族混排基线偏差应精确为 0"
        );
        let stock = egui::FontId::new(size, FontFamily::Monospace);
        let old_gap = baseline_gap(&stock).expect("现状族混排行应同时含 CJK 与拉丁");
        assert!(
            cfg!(target_os = "macos") || old_gap.abs() >= 1.0,
            "对照(现状 Monospace 族)应存在基线偏差,实测 {old_gap}"
        );

        // 行盒高:新族与现状族完全一致(链头未动的直接读数)。
        let row_height = |font: &egui::FontId| {
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(
                    "甲post中文123".to_owned(),
                    font.clone(),
                    egui::Color32::WHITE,
                )
                .rows[0]
                    .rect()
                    .height()
            })
        };
        assert_eq!(
            row_height(&editor),
            row_height(&stock),
            "编辑器族行盒高不得偏离现状族"
        );
    }

    /// 从 galley 首行读「CJK 基线 − 拉丁基线」(无头布局度量,与
    /// #43 M1 取证同式);行内缺 CJK 或缺拉丁时返回 `None`。
    fn baseline_cjk_minus_latin(galley: &egui::Galley) -> Option<f32> {
        let is_ideograph = |c: char| ('\u{4E00}'..='\u{9FFF}').contains(&c);
        let is_latin = |c: char| c.is_ascii_alphanumeric();
        let row = &galley.rows[0].row;
        let latin = row
            .glyphs
            .iter()
            .filter(|g| is_latin(g.chr))
            .map(|g| g.pos.y)
            .fold(None::<f32>, |acc, v| Some(acc.map_or(v, |a| a.max(v))));
        let cjk = row
            .glyphs
            .iter()
            .filter(|g| is_ideograph(g.chr))
            .map(|g| g.pos.y)
            .fold(None::<f32>, |acc, v| Some(acc.map_or(v, |a| a.max(v))));
        Some(cjk? - latin?)
    }

    /// #23 F3 CJK 防回归:F3 起预览正文用**显式 FontId**(size = 用户字号
    /// 偏好读侧,族 = [`preview_body_family`]),编辑器字号经 theme 投影到
    /// Monospace 档。本测试把整条 F3 链路(install 字体链 → `ThemeSettings
    /// ::apply` 投影字号/行距 → 读侧取族与字号)在同一个 context 上串起来
    /// 验证:预览所用的族在用户字号下 CJK 回退仍在链尾、字形可见 —— 字号/
    /// 行距偏好不得以任何方式(断链、漏挂 CJK)破坏中文可见性(#50 M1 起
    /// 编辑器档的族投影到专用等宽族,其链纪律同样在此钉住)。本机无 CJK
    /// 候选时如实跳过(降级语义由 [`inter_registers_even_without_cjk`]
    /// 钉住)。
    #[test]
    fn font_size_projection_keeps_cjk_fallback_on_preview_family() {
        let ctx = egui::Context::default();
        let cjk = install(&ctx);
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        // 用户字号 22(接近上界,排版偏好的显式自定义值)
        crate::theme::ThemeSettings {
            editor_font_size: 22.0,
            ..Default::default()
        }
        .apply(&ctx, crate::theme::ThemeMode::Dark);

        // 读侧拿到的就是投影值:预览 FontId 与编辑器档位的共同 size 来源
        let size = crate::theme::editor_font_size(&ctx);
        assert_eq!(size, 22.0, "读侧应与投影同源");

        // 编辑器 Monospace 档:size 已投影,#50 M1 起族同步投影为编辑器
        // 专用等宽族(链头与 Monospace 同为内置 Hack,字号语义不变;
        // CJK 回退挂专用族链尾 —— fonts.rs 的字体链纪律在投影后保持)
        let mono = ctx
            .style_of(egui::Theme::Dark)
            .text_styles
            .get(&egui::TextStyle::Monospace)
            .expect("出厂 Monospace 档恒存在")
            .clone();
        assert_eq!(mono.size, 22.0);
        assert_eq!(mono.family, editor_mono_family(&ctx));

        if cjk.is_none() {
            eprintln!("本机无 CJK 候选字体,CJK 链尾断言跳过");
            return;
        }
        // 预览正文族(Inter-Preview)与回落族(Proportional)的链尾都还是
        // CJK 回退:F3 只动 FontId 的 size,族选择沿用 #43 M2 的预览族,
        // 其 CJK 回退链没被字号偏好殃及。
        let preview_family = preview_body_family(&ctx);
        assert_eq!(
            preview_family,
            FontFamily::Name(Arc::from(FAMILY_PREVIEW_BODY)),
            "有 CJK 时预览正文族应为 override 副本族(不是 Proportional)"
        );
        ctx.fonts(|f| {
            let chain = &f.definitions().families[&preview_family];
            assert_eq!(
                chain.last().map(String::as_str),
                Some(CJK_PROPORTIONAL),
                "预览正文族: CJK 回退仍在链尾(原生表值)"
            );
            let prop = &f.definitions().families[&FontFamily::Proportional];
            assert_eq!(
                prop.last().map(String::as_str),
                Some(CJK_PROPORTIONAL_UI),
                "Proportional: CJK 回退仍在链尾(UI 对齐副本)"
            );
        });
        ctx.fonts_mut(|f| {
            let preview = FontId::new(size, preview_family.clone());
            assert!(
                f.has_glyphs(&preview, "中文字体测试"),
                "预览正文族在用户字号下 CJK 可见"
            );
            let fallback = FontId::new(size, FontFamily::Proportional);
            assert!(
                f.has_glyphs(&fallback, "中文字体测试"),
                "回落族 Proportional 的 CJK 回退可见"
            );
        });
    }
}
