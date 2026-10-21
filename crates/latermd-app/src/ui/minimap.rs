//! 源码页 Minimap 的行模型与修订号缓存(#55 M1,纯数据层)。
//!
//! 三件东西,全部零绘制:
//!
//! * **行模型**([`line_model`]):全文 → 每行一条 [`LineDesc`](前导空白折算
//!   的缩进 + 按显示宽归一的长度)。显示宽以「半宽单元」计:ASCII 半宽
//!   记 1,CJK/全角/emoji 记 2(简化 EAW 宽表见 [`is_fullwidth`]),tab 按
//!   编辑器现状口径折算(egui `FontTweak::tab_size` 默认 4,epaint 对
//!   `\t` 的 advance 即 tab_size × 空格宽;本仓未设 tweak,见
//!   [`TAB_UNITS`])。缩进+长度联合封顶在 [`LineParams::max_units`],
//!   行条永不溢出 minimap 右缘。
//! * **缓存**([`with_lines`]):行模型按「文档修订号 + 参数」键控,挂在
//!   `editor_id.with("minimap-lines")` 的 egui temp memory 上 —— 每标签
//!   一个槽位(照 #39 `tab_preview_id` per-tab 分槽先例),编辑只失效
//!   本标签的缓存,切标签往返命中(id 只含稳定 tab id,绝不含内容
//!   hash/长度,AGENTS §6.7)。
//! * **可见窗口**([`window`]):(minimap 高度, 总行数, 滚动位置, 比例尺)
//!   → 应绘制的行区间与每行 y。O(1) 纯函数,滚动帧只平移窗口、不触碰
//!   行模型缓存。
//!
//! M2 渲染层(本文件下半段)消费这三个入口:行条只画 [`window`] 的可见
//! 区间、颜色从当帧 visuals 推导(明暗两套);编辑器视口的高亮框、
//! 点击/拖动跳转([`jump_ratio`])与悬停滚轮转发([`wheel_editor_delta`])
//! 都按「内容比例」换算,与窗口平移同一套几何。比例尺/不满高铺排等自
//! 选项登记在 docs/decisions-pending.md #104,条宽/默认开关等 M2 口径
//! 见 #105(#105① 滚轮转发销账注记亦在该条)。

use latermd_editor::EditorBuffer;
use std::ops::Range;

use eframe::egui;

/// tab 折算的半宽单元数。编辑器现状无自定义 tab 口径(无 FontTweak、无
/// tab 设置项),egui 0.36 epaint 对 `\t` 的 advance = `tab_size × 空格宽`、
/// `tab_size` 默认 4.0 —— minimap 与编辑器显示宽同口径,取 4。
pub(crate) const TAB_UNITS: u16 = 4;

/// 比例尺:minimap 每行条的像素高。任务书建议 2-4px,取 3px:2px 在 HiDPI
/// 缩放下细至难辨,4px 让 10000 行占 40000px(行程过长、滑起来发飘),
/// 3px 下 10000 行 = 30000px、视口内约 200 行,与 VS Code 默认观感同量级。
pub(crate) const ROW_H: f32 = 3.0;

/// 长度归一封顶的出厂值(半宽单元),约对应 96px 条宽 × 1px/单元 —— M2
/// 定条宽后按 `条宽 / 期望单元像素` 重算传入,缓存随参数键控自动重建。
pub(crate) const MAX_UNITS: u16 = 96;

/// 行模型的构建参数(缓存键的一半,另一半是文档修订号)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LineParams {
    /// tab 折算的半宽单元数([`TAB_UNITS`] 口径)。
    pub tab_units: u16,
    /// 单行(缩进+内容)显示宽封顶:超出截断,行条右缘永不溢出。
    pub max_units: u16,
}

impl LineParams {
    /// 出厂参数:tab 4 单元、封顶 96 单元。
    pub(crate) const fn factory() -> Self {
        Self {
            tab_units: TAB_UNITS,
            max_units: MAX_UNITS,
        }
    }
}

/// 单行的 minimap 绘制描述:一条「缩进 × 长度」的抽象行条。
///
/// 两个数都以半宽单元计(一个 ASCII 字符位 = 1);渲染层换算像素 =
/// `值 × (minimap 条宽 / max_units)`。空行两值皆零。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LineDesc {
    /// 前导空白折算宽(空格 1、tab `tab_units`、全角空格 U+3000 记 2),
    /// 已封顶 `max_units`。
    pub indent: u16,
    /// 去掉前导空白后的内容显示宽(全宽字符记 2,超预算截断),
    /// 已封顶 `max_units - indent`。
    pub width: u16,
}

/// 单字符的半宽单元数。
fn char_units(c: char, tab_units: u16) -> u16 {
    match c {
        '\t' => tab_units,
        // 全角空格:既是前导空白白名单成员,又占全宽。
        '\u{3000}' => 2,
        // 零宽连接符:变体选择符与 ZWJ 不占宽(emoji 组合串)。
        '\u{200D}' | '\u{FE0E}' | '\u{FE0F}' => 0,
        c if is_fullwidth(c) => 2,
        _ => 1,
    }
}

/// 显示宽判定:East Asian Width 的 W(宽)/F(全角)大区段合并简化表。
///
/// 完整 EAW 表有上百个零散 W 单点(如 U+1F004、U+231A);本表只收连续
/// 大区段,零散点并入邻近 emoji 密集区 —— 误差方向只是把极少数窄字符记
/// 宽 ≤1 单元,对 minimap 抽象条的相对长度观感无影响。口径已登记
/// decisions-pending #104。
fn is_fullwidth(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x1100..=0x115F        // Hangul Jamo
        | 0x2300..=0x23FF      // 杂项技术(⌚⏰⏳ emoji 密集)
        | 0x2600..=0x27BF      // 杂项符号 + dingbats(❤☎ emoji 密集)
        | 0x2B00..=0x2BFF      // 杂项符号箭头(⭐)
        | 0x2E80..=0x303E      // CJK 部首/符号/注音
        | 0x3041..=0x33FF      // 假名/注音扩展/兼容
        | 0x3400..=0x4DBF      // CJK 扩展 A
        | 0x4E00..=0x9FFF      // CJK 统一表意
        | 0xA000..=0xA4CF      // 彝文
        | 0xA960..=0xA97F      // Hangul Jamo 扩展 A
        | 0xAC00..=0xD7A3      // Hangul 音节
        | 0xF900..=0xFAFF      // CJK 兼容表意
        | 0xFE10..=0xFE19      // 竖排形式
        | 0xFE30..=0xFE6F      // CJK 兼容形式
        | 0xFF00..=0xFF60      // 全角形式
        | 0xFFE0..=0xFFE6      // 全角符号
        | 0x16FE0..=0x16FE4    // 表意助记符
        | 0x17000..=0x18AFF    // 唐古特文/假名扩展
        | 0x1B000..=0x1B2FF    // 假名补充
        | 0x1F300..=0x1F64F    // 常用 emoji 与 CJK 符号密集区
        | 0x1F680..=0x1F6FF    // 交通/地图 emoji
        | 0x1F900..=0x1F9FF    // emoji 补充
        | 0x1FA70..=0x1FAFF    // emoji 扩展 A
        | 0x20000..=0x2FFFD    // CJK 扩展 B+(SIP)
        | 0x30000..=0x3FFFD    // CJK 扩展 G+(TIP)
    )
}

/// 前导空白白名单:只认空格、tab 与全角空格。`char::is_whitespace` 会把
/// NBSP/换行制表符等也计入,它们在 Markdown 缩进语义里不是缩进。
fn is_indent_char(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\u{3000}')
}

/// 行模型:全文 → 每行一条 [`LineDesc`]。
///
/// 行划分与 gutter/编辑器同口径(`split('\n')`,总行数 = 1 + `\n` 数,空
/// 文档 1 行)。ZWJ 序列(`👨‍👩‍👧`)整体只记一次全宽:前一个字符是
/// U+200D 时当前字符记 0 宽 —— 组合串显示为单个字形,逐段累计会把长度
/// 放大 N 倍。
pub(crate) fn line_model(text: &str, params: LineParams) -> Vec<LineDesc> {
    text.split('\n')
        .map(|line| {
            let mut indent: u16 = 0;
            let mut rest = line.chars().peekable();
            while let Some(&c) = rest.peek() {
                if !is_indent_char(c) {
                    break;
                }
                indent = indent.saturating_add(char_units(c, params.tab_units));
                rest.next();
            }
            let indent = indent.min(params.max_units);
            // 条永不溢出右缘:内容预算 = 封顶减缩进;逐字符累计,放不下
            // 整个全宽字符就停(宁可短一单元,不切字符)。
            let budget = params.max_units.saturating_sub(indent);
            let mut width: u16 = 0;
            let mut after_zwj = false;
            for c in rest {
                let units = if after_zwj {
                    0
                } else {
                    char_units(c, params.tab_units)
                };
                after_zwj = c == '\u{200D}';
                match width.checked_add(units) {
                    Some(sum) if sum <= budget => width = sum,
                    _ => break,
                }
            }
            LineDesc { indent, width }
        })
        .collect()
}

/// 行模型缓存条目(egui temp memory 里的值类型)。
///
/// 键控 = 文档修订号 + [`LineParams`]:编辑(rev 前进)、参数变化(条宽/
/// tab 口径变)才重建;同 rev 同参数命中时 `Vec` 原地不动(指针稳定,
/// 滚动/空闲帧零成本)。`synced_rev: Option` 让「从未构建」与「rev 0 的
/// 空文档」可区分 —— 后者也是合法缓存(1 条零长行)。
#[derive(Clone, Debug, Default)]
pub(crate) struct MinimapCache {
    synced_rev: Option<u64>,
    params: Option<LineParams>,
    lines: Vec<LineDesc>,
}

impl MinimapCache {
    /// 命中判定:修订号与参数都一致。
    fn matches(&self, rev: u64, params: LineParams) -> bool {
        self.synced_rev == Some(rev) && self.params == Some(params)
    }

    /// 取行模型(命中即返回既有切片,miss 才重建)。
    pub(crate) fn ensure(&mut self, editor: &EditorBuffer, params: LineParams) -> &[LineDesc] {
        let rev = editor.revision();
        if !self.matches(rev, params) {
            self.lines = line_model(editor.text(), params);
            self.synced_rev = Some(rev);
            self.params = Some(params);
        }
        &self.lines
    }
}

/// minimap 缓存的槽位 id:由标签稳定的 editor_id 派生(每标签一套,照
/// `tab_preview_id` / `ime-caret` 同款先例);绝不含内容 hash/长度。
pub(crate) fn cache_id(editor_id: egui::Id) -> egui::Id {
    editor_id.with("minimap-lines")
}

/// 取当前标签的行模型并在回调里消费。
///
/// 缓存条目活在 egui temp memory(id 见 [`cache_id`]):切走标签期间不被
/// 覆盖、切回即命中;回调借出存储条目的切片,命中帧零分配零拷贝。
pub(crate) fn with_lines<R>(
    ctx: &egui::Context,
    editor_id: egui::Id,
    editor: &EditorBuffer,
    params: LineParams,
    f: impl FnOnce(&[LineDesc]) -> R,
) -> R {
    ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<MinimapCache>(cache_id(editor_id));
        f(cache.ensure(editor, params))
    })
}

/// 可见窗口的输入(M2 从编辑器滚动状态换算后传入)。
#[derive(Clone, Copy, Debug)]
pub(crate) struct WindowInput {
    /// minimap 内容区高(px)。
    pub minimap_height: f32,
    /// 文档总行数(0 按 1 计,空文档也有一行)。
    pub total_lines: usize,
    /// 滚动位置:编辑器视口顶在可滚行程中的比例,0 = 文档顶、1 = 文档底
    /// (内容不满一屏时恒 0)。M2 侧按 `offset / (content_h - viewport_h)`
    /// 换算。
    pub scroll_ratio: f32,
    /// 比例尺:minimap 每行条高(px),建议 [`ROW_H`]。
    pub row_h: f32,
}

/// 可见窗口:应绘制的行区间与每行 y 坐标(等差,由
/// `y0 + (行号 − 区间首行) × row_h` 表达,不必逐行存表)。
#[derive(Clone, Debug)]
pub(crate) struct VisibleWindow {
    range: Range<usize>,
    /// 区间首行条的 y(相对 minimap 视口顶;半露行为负)。
    y0: f32,
    row_h: f32,
}

impl VisibleWindow {
    /// 应绘制的行区间(0-based,半开)。生产消费见 [`Self::iter`] 与
    /// [`shapes`] 的容量预估;也是 M1 单测的取证面。
    pub(crate) fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    /// 行号 → 该行条的 y(相对 minimap 视口顶);区间外返回 `None`。
    /// M1 单测的取证面,非测试构建豁免 dead_code(与 `live::LiveScrollProbe`
    /// 同款口径)。
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn y_of(&self, line: usize) -> Option<f32> {
        if !self.range.contains(&line) {
            return None;
        }
        Some(self.y0 + (line - self.range.start) as f32 * self.row_h)
    }

    /// (行号, y) 迭代,M2 绘制直接消费。
    pub(crate) fn iter(&self) -> impl Iterator<Item = (usize, f32)> + '_ {
        let start = self.range.start;
        self.range
            .clone()
            .map(move |line| (line, self.y0 + (line - start) as f32 * self.row_h))
    }
}

/// 可见窗口计算:O(1) 纯函数,滚动帧每帧调用也不触碰行模型缓存。
///
/// 长文档(minimap 内容总高 > 视口高)时 minimap 随滚动平移,平移量与
/// 编辑器滚动同比例(视口顶对视口顶);短文档不满高时顶部对齐、余量留
/// 在底部,滚动比例不影响铺排(编辑器此时也滚不动,#104)。
pub(crate) fn window(input: WindowInput) -> VisibleWindow {
    let total = input.total_lines.max(1);
    // 比例尺下限防退化:row_h → 0 会让可见行数发散,0.5px 已是肉眼极限。
    let row_h = input.row_h.max(0.5);
    let viewport = input.minimap_height.max(0.0);
    let full = total as f32 * row_h;
    let travel = (full - viewport).max(0.0);
    let offset = input.scroll_ratio.clamp(0.0, 1.0) * travel;

    let first = ((offset / row_h).floor() as usize).min(total - 1);
    // 视口底所在行 ceil 进一(半露也算可见),但至少一行、至多文末。
    let last = (((offset + viewport) / row_h).ceil() as usize).clamp(first + 1, total);
    VisibleWindow {
        range: first..last,
        y0: first as f32 * row_h - offset,
        row_h,
    }
}

// —— M2 渲染层(#55)——

/// 右缘窄条总宽 = 2px 左内边距、96px 行条区(= [`MAX_UNITS`],1px/单元)、
/// 10px 滚动条避让。egui ScrollArea 的滚动条贴视口右缘、画在后、居上,
/// 行条最长到 98px 处,最后 10px 留给它;行条有效宽 96 在任务书建议的
/// 80–100px 区间内。
/// 自动模式只在较宽编辑区、文档超过两屏时启用。检查最多两屏行数，
/// 不为每帧判断遍历整篇长文；宽度取添加缩略图之前的视口，避免反馈抖动。
pub(crate) fn automatic_visible(width: f32, height: f32, row_height: f32, text: &str) -> bool {
    let screen_lines = (height / row_height.max(1.0)).ceil() as usize;
    width >= 600.0 && text.lines().take(screen_lines * 2 + 1).count() > screen_lines * 2
}

pub(crate) const MINIMAP_W: f32 = 108.0;

/// 行条区左内边距:行条 x = 条左缘 + 此值 + 缩进。
const PAD_X: f32 = 2.0;

/// 行条颜色:正文 fg(noninteractive)按主题压透明度 —— 暗色白压到 ~35%、
/// 亮色黑压到 ~26%。两套 visuals 同一函数推导,不做每主题手工色表。
fn bar_color(visuals: &egui::Visuals) -> egui::Color32 {
    let [r, g, b, _] = visuals.widgets.noninteractive.fg_stroke.color.to_array();
    let alpha = if visuals.dark_mode { 0x59 } else { 0x42 };
    egui::Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

/// 视口高亮框的底色与描边:同从当帧 visuals 推导(暗色提白、亮色压黑,
/// 与行条方向相反,保证在行条之上仍可辨;描边复用 noninteractive 的
/// bg_stroke,明暗各自协调)。
fn viewport_fill(visuals: &egui::Visuals) -> egui::Color32 {
    if visuals.dark_mode {
        egui::Color32::from_white_alpha(0x14)
    } else {
        egui::Color32::from_black_alpha(0x0F)
    }
}

/// 点击/拖动跳转的输入:把 minimap 视口内的指针位置换算成编辑器的目标
/// 滚动比例(纯函数,单测锚点)。
pub(crate) struct JumpInput {
    /// minimap 内容区高(px,即窄条高)。
    pub minimap_height: f32,
    /// 文档总行数。
    pub total_lines: usize,
    /// 当前滚动比例(与 [`WindowInput::scroll_ratio`] 同口径)。
    pub scroll_ratio: f32,
    /// 比例尺(minimap 每行条高,px)。
    pub row_h: f32,
    /// 指针 y(相对 minimap 视口顶)。
    pub pointer_y: f32,
    /// 编辑器视口高占内容总高的比例(视口高 / 内容高)。
    pub viewport_frac: f32,
    /// #68 拖阴影的抓取偏移(px,相对窄条顶):按下帧指针落在高亮框内
    /// 时记录 `pointer_y − 高亮框 top`,由调用方判定与持有(本函数只管
    /// 几何)。`Some` = 目标阴影 top(`pointer_y − grab`)对准编辑器
    /// **视口顶**,抓取点相对高亮框不动;`None` = 居中跳转(#55 现状,
    /// 框外点击/非拖影路径)。
    pub grab: Option<f32>,
}

/// 位置 → 目标滚动比例。两条路径,同一套几何(`full`/`travel`/`span`
/// 与 None 口径共享):
///
/// * **`grab: None`(现状,#55)**:点击处的内容对准编辑器**视口中心**。
///   换算两步:①指针 y 加上 minimap 当前平移(`scroll_ratio × travel`),
///   除以全文档高得到内容比例 `p`;②让视口中心落在 `p` →
///   `target = (p − viewport_frac/2) / (1 − viewport_frac)`。
/// * **`grab: Some`(#68 拖阴影)**:目标阴影 top(`pointer_y − grab`)
///   对准编辑器**视口顶**,抓取点相对高亮框不动。滑轨([`viewport_highlight`]
///   里 `top` 的系数)统一写作 `span = (1−ve)·full − travel` —— 长文档时
///   `travel = full − map_h`,该式退化为 `map_h − ve·full`(与 #68 原式逐
///   位相等);短文档(`travel = 0`)退化成 `(1−ve)·full`,正是铺排 Outcomes
///   下阴影实际可走的行程。反解即 `target = (pointer_y − grab) / span`。
///   绝对目标语义:**不含** `scroll_ratio`(平移量已被反解吸收,与
///   居中路径同属「目标由指针唯一决定」)。滑轨退化(`span ≤ 0`:阴影高
///   ≥ 可走行程,生产恒不发生 —— minimap 3px/行远矮于编辑器行高)返回
///   `None` 原样不动。
///
/// **唯一必要的 `None` 闸门是内容不满一屏(`ve ≥ 1`)** —— 那时编辑器确实
/// 无行程,跳了也是空转。此前这版实现把「minimap 铺不满窄条」(`travel ≤ 0`)
/// 也当成该 `None`,那是错的:minimap 3px/行、编辑器 ≥18px/行,同一篇文档
/// 在两侧的行数阈值差一个量级(编辑区高 H 时编辑器 `H/18` 行就有行程,
/// minimap 要 `H/3` 行才铺满)—— `H=900` 下即「编辑器 50 行能滚 vs minimap
/// 300 行才有滑轨」。中间那段文档(全屏下约 50–300 行)**编辑器明明能滚、
/// minimap 却全哑**,正是坤哥 2026-10-08 / 10-09 两次真机报告的「小窗可拖、
/// 全屏不可拖」(同一篇文档,小视口下够长就绿)。2026-10-09 修:闸门只留
/// `ve`;`travel` 退化为 0 时两条路径照各自公式算 —— 短文档下 minimap 顶部
/// 铺排(`offset` 恒 0),加法分支天然退化为量纲自洽。
///
/// 输出钳进 0..=1(拖到顶到顶、拖到底到底)。
pub(crate) fn jump_ratio(input: JumpInput) -> Option<f32> {
    let total = input.total_lines.max(1) as f32;
    let row_h = input.row_h.max(0.5);
    let full = total * row_h;
    let viewport = input.minimap_height.max(0.0);
    let travel = (full - viewport).max(0.0);
    let ve = input.viewport_frac.clamp(0.0, 1.0);
    if ve >= 1.0 {
        return None; // 内容不满一屏:编辑器无行程
    }
    match input.grab {
        Some(grab) => {
            // 滑轨 = viewport_highlight 的 top 系数(长短文档同一式)。
            let span = (1.0 - ve) * full - travel;
            if span <= 0.0 {
                return None; // 阴影高 ≥ 可走行程:无滑轨可拖,原样不动
            }
            Some(((input.pointer_y - grab) / span).clamp(0.0, 1.0))
        }
        None => {
            let offset = input.scroll_ratio.clamp(0.0, 1.0) * travel;
            let p = ((input.pointer_y + offset) / full).clamp(0.0, 1.0);
            Some(((p - ve * 0.5) / (1.0 - ve)).clamp(0.0, 1.0))
        }
    }
}

/// 悬停滚轮转发(#105①)的输入:把窄条命中区截获的本帧滚轮换算成编辑器
/// 滚动增量(纯函数,单测锚点)。几何量与 [`JumpInput`] 同源——metrics
/// 两项是编辑器 ScrollArea 的上一帧真值,`total_lines`/`row_h` 与行模型
/// 同一套口径。
#[derive(Clone, Copy)]
pub(crate) struct WheelInput {
    /// 编辑器内容总高(`ScrollMetrics::content_height`)。
    pub content_height: f32,
    /// 编辑器视口高(`ScrollMetrics::viewport_height`)。
    pub viewport_height: f32,
    /// 文档总行数。
    pub total_lines: usize,
    /// 比例尺(minimap 每行条高,px)。
    pub row_h: f32,
    /// 本帧截获的滚轮 y 分量(px;egui 口径正值 = 内容下移 = 向上滚,
    /// 与 ScrollArea 内建消费 `offset -= delta` 同号)。
    pub delta_y: f32,
}

/// 悬停滚轮 → 编辑器滚动增量(px,与 `delta_y` 同号):delta × 「文档
/// 内容高 / 文档的 minimap 全高」——窄条上滚 1px,文档滚
/// `内容高/(总行数×比例尺)` px。无行程(编辑器滚不动)或无滚轮量返回
/// `None`,调用方原样不动;越端钳制交给 ScrollArea::end 的 offset 边界
/// (与内建滚轮同一条钳子)。
///
/// 放大系数取舍:VS Code 按「编辑器滚动高 / minimap 视口高」放大(更
/// 快),这里取与 [`jump_ratio`] 同一几何(full = 总行数 × 比例尺)——
/// 滚轮与点击/拖动共用一套换算源,滚轮步长与行条自身的像素位移 1:1,
/// 观感上「窄条被你滚动了」而不是「文档被随机加速」。
///
/// 落地走**相对** delta(`Ui::scroll_with_delta_animation`)而非绝对
/// `scroll_to_rect`:滚轮动量是逐帧尾量(egui 平滑滚轮事件帧交 90%、
/// 余量随后帧衰减交齐),绝对目标会拿上一帧基数互相覆写(实测振荡);
/// 相对增量在 end() 里加到**当帧已应用**的 offset 上,天然链接。
pub(crate) fn wheel_editor_delta(input: WheelInput) -> Option<f32> {
    let travel = (input.content_height - input.viewport_height).max(0.0);
    if travel <= 0.0 || input.delta_y == 0.0 {
        return None;
    }
    let total = input.total_lines.max(1) as f32;
    let map_full = total * input.row_h.max(0.5);
    Some(input.delta_y * (input.content_height / map_full))
}

/// 编辑器视口在 minimap 上的高亮框(相对窄条顶的 `top` 与 `height`,
/// 已钳进 `[0, minimap_height]`)。与 [`window`] 的平移同一套几何:视口
/// 占内容的比例区间 `[ratio×(1−ve), ratio×(1−ve)+ve]` 映射到 minimap
/// 内容坐标再减平移;非折行文档下与行条严格对齐,深度折行时按内容比例
/// (行条是均匀逻辑行模型,口径见 decisions-pending #105)。
pub(crate) fn viewport_highlight(input: WindowInput, viewport_frac: f32) -> (f32, f32) {
    let total = input.total_lines.max(1) as f32;
    let row_h = input.row_h.max(0.5);
    let full = total * row_h;
    let map_h = input.minimap_height.max(0.0);
    let travel = (full - map_h).max(0.0);
    let ve = viewport_frac.clamp(0.0, 1.0);
    let ratio = input.scroll_ratio.clamp(0.0, 1.0);
    let top = ratio * ((1.0 - ve) * full - travel);
    let height = (ve * full).clamp(0.0, map_h);
    let top = top.clamp(0.0, (map_h - height).max(0.0));
    (top, height)
}

/// 构造 minimap 的本帧形状:可见窗口内的行条 + 编辑器视口高亮框。返回
/// (shapes, 非零行条数, 高亮框矩形)。
///
/// **只构造、不上画布**:`with_lines` 把本函数持在 `ctx().data_mut` 的
/// 借用里,而 `painter` 的任何 add 都要再进同一把 Context 写锁 —— 重入
/// 即死锁(实测踩过)。调用方(editor.rs)在借用之外 `painter().extend(shapes)`
/// 并写测试探针。只在源码模式、开关开启时被构造(关闭 = 调用方根本不
/// 进,零形状)。
///
/// `scroll_ratio` / `viewport_frac` 由调用方从编辑器 ScrollArea 的当帧
/// 真值换算(`offset/(content−viewport)`、`viewport/content`)。
pub(crate) fn shapes(
    visuals: &egui::Visuals,
    rect: egui::Rect,
    lines: &[LineDesc],
    scroll_ratio: f32,
    viewport_frac: f32,
) -> (Vec<egui::Shape>, usize, egui::Rect) {
    let geometry = WindowInput {
        minimap_height: rect.height(),
        total_lines: lines.len(),
        scroll_ratio,
        row_h: ROW_H,
    };
    let win = window(geometry);
    let bar = bar_color(visuals);
    let left = rect.left() + PAD_X;
    let mut out = Vec::with_capacity(win.range().len() + 2);
    // 只构造可见窗口:O(视口行数),滚动帧不随文档长度增长(M1 窗口语义)。
    let mut bars = 0usize;
    for (line, y) in win.iter() {
        let desc = lines[line];
        if desc.width == 0 {
            continue; // 空行/纯缩进封顶行:零宽条不画
        }
        bars += 1;
        let tl = egui::pos2(left + f32::from(desc.indent), rect.top() + y);
        out.push(egui::Shape::Rect(egui::epaint::RectShape::filled(
            egui::Rect::from_min_size(tl, egui::vec2(f32::from(desc.width), ROW_H)),
            egui::CornerRadius::ZERO,
            bar,
        )));
    }
    // 视口高亮框:底色 + 1px 描边,横跨整条(缩进信息在行条上,框只表
    // 「编辑器现在看哪里」)。排在行条之后 → 盖在其上。
    let (top, height) = viewport_highlight(geometry, viewport_frac);
    let frame = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.top() + top),
        egui::vec2(rect.width(), height),
    );
    out.push(egui::Shape::Rect(egui::epaint::RectShape::filled(
        frame,
        egui::CornerRadius::ZERO,
        viewport_fill(visuals),
    )));
    out.push(egui::Shape::Rect(egui::epaint::RectShape::stroke(
        frame,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    )));
    (out, bars, frame)
}

/// 测试探针的键(与 [`cache_id`] 同源由 editor_id 派生)。
pub(crate) fn probe_id(editor_id: egui::Id) -> egui::Id {
    editor_id.with("minimap-probe")
}

/// 编辑器 ScrollArea 的滚动度量快照(每帧末由 editor.rs 写入,下一帧的
/// 点击/拖动跳转换算用它:闭包内拿不到本帧排版完成后的内容高,上一帧
/// 真值足够 —— 编辑帧的一帧误差在下一帧自愈)。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScrollMetrics {
    /// 视口顶的内容坐标偏移(`ScrollArea::state.offset.y`)。
    pub offset: f32,
    /// 内容总高(`ScrollArea::content_size.y`)。
    pub content_height: f32,
    /// 视口高(`ScrollArea::inner_rect.height()`)。
    pub viewport_height: f32,
}

/// metrics temp 的键(与 [`cache_id`] 同源由 editor_id 派生,每标签一份)。
pub(crate) fn metrics_id(editor_id: egui::Id) -> egui::Id {
    editor_id.with("minimap-metrics")
}

/// 跳转意图 temp 的键:值 = 指针 y(相对窄条顶)。闭外交互命中时写入,
/// 下一帧 ScrollArea 闭包开头消费即清 —— 一帧滞后的传接(窄条命中必须
/// 排在 ScrollArea 之后注册才不被背景拖拽抢层级,而 scroll_to_rect 只在
/// 闭包内才被同帧消费,二者不可兼得;拖动逐帧覆写,跟随无感)。
pub(crate) fn jump_id(editor_id: egui::Id) -> egui::Id {
    editor_id.with("minimap-jump")
}

/// #68 拖影手势的抓取状态 temp 键:值 = `Option<f32>`(按下帧指针相对
/// 高亮框顶的偏移,窄条局部坐标)。闭外 **press 帧**判定写入 —— 指针在
/// 上一帧高亮框内记 `Some(指针 y − 框 top)`,框外记 `None`(手势在而
/// 无抓取,拖动走 #55 居中现状);拖动序列期间常驻,闭内消费跳转意图时
/// 随读不删(整个序列用同一次判定的偏移),释放帧删除 —— 下一次按下
/// 重新判定。
pub(crate) fn grab_id(editor_id: egui::Id) -> egui::Id {
    editor_id.with("minimap-grab")
}

/// 测试探针载荷(仅测试读;生产每帧写、从不读,字段读取豁免
/// dead_code —— 与 `live::LiveScrollProbe` 同款)。
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct MinimapProbe {
    pub bars: usize,
    pub viewport: Option<egui::Rect>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::editor;

    /// ① 缩进折算:空格/tab/混合/全角空格/空行/深缩进封顶/tab 参数。
    #[test]
    fn indent_folds_spaces_tabs_and_mixed() {
        let p = LineParams::factory();
        // 空格
        assert_eq!(line_model("    a", p)[0].indent, 4);
        // tab:4 单元/个
        assert_eq!(line_model("\t\ta", p)[0].indent, 8);
        // 混合:2 + 4 + 1
        assert_eq!(line_model("  \t a", p)[0].indent, 7);
        // 全角空格:记 2
        assert_eq!(line_model("\u{3000}a", p)[0].indent, 2);
        // 空文档与空行:零缩进零长度
        assert_eq!(line_model("", p), vec![LineDesc::default()]);
        let m = line_model("\n\nx", p);
        assert_eq!(m[0], LineDesc::default(), "空行零长度");
        assert_eq!(m[1], LineDesc::default(), "全空白行也是零长度条");
        assert_eq!((m[2].indent, m[2].width), (0, 1));
        // 全空白行:缩进照折算,内容为零
        assert_eq!(
            line_model("   ", p)[0],
            LineDesc {
                indent: 3,
                width: 0
            }
        );
        // 深缩进封顶:30 个 tab = 120 单元,封到 96,内容预算 0
        assert_eq!(
            line_model(&("\t".repeat(30) + "x"), p)[0],
            LineDesc {
                indent: 96,
                width: 0
            }
        );
        // tab 口径是参数:8 单元/个时两 tab 折 16
        let p8 = LineParams {
            tab_units: 8,
            max_units: 96,
        };
        assert_eq!(line_model("\t\ta", p8)[0].indent, 16);
    }

    /// ① 长度归一:纯 ASCII / CJK 混排 / 各全宽文种 / emoji / ZWJ 序列 /
    /// 超长截断。
    #[test]
    fn width_normalizes_by_display_width() {
        let p = LineParams::factory();
        // 纯 ASCII
        assert_eq!(line_model("hello", p)[0].width, 5);
        // CJK 全宽
        assert_eq!(line_model("你好", p)[0].width, 4);
        // 中英混排:1+2+1
        assert_eq!(line_model("a你b", p)[0].width, 4);
        // 全角形式/假名/谚文同表覆盖
        assert_eq!(line_model("ＡＢ", p)[0].width, 4, "全角字母记 2");
        assert_eq!(line_model("あい", p)[0].width, 4, "假名记 2");
        assert_eq!(line_model("가나", p)[0].width, 4, "谚文记 2");
        // 单 emoji 记 2;ZWJ 家庭序列整体记 2(不是 6)
        assert_eq!(line_model("😀", p)[0].width, 2);
        assert_eq!(line_model("👨‍👩‍👧", p)[0].width, 2, "ZWJ 序列只记一次全宽");
        // 变体选择符零宽:❤(全宽区)+ FE0F 仍是 2
        assert_eq!(line_model("❤️", p)[0].width, 2);
        // 超长截断:200 个 ASCII 封 96;100 个 CJK(200 单元)也封 96
        assert_eq!(line_model(&"a".repeat(200), p)[0].width, 96);
        assert_eq!(line_model(&"你".repeat(100), p)[0].width, 96);
        // 截断预算扣缩进:缩进 4 + 内容 92
        let m = line_model(&format!("    {}", "a".repeat(200)), p);
        assert_eq!((m[0].indent, m[0].width), (4, 92));
        // 预算 1 放不下全宽字符:停在 0,不切半字符、不溢出
        let narrow = LineParams {
            tab_units: 4,
            max_units: 1,
        };
        assert_eq!(line_model("你", narrow)[0].width, 0);
        // 多行各自独立
        let m = line_model("aaa\n    bb\n你", p);
        let widths: Vec<u16> = m.iter().map(|d| d.width).collect();
        assert_eq!(widths, vec![3, 2, 2]);
        assert_eq!(m[1].indent, 4);
    }

    /// ② 行数口径:`split('\n')`,空文档 1 行,与 gutter 的 1 + `\n` 数
    /// 同构(editor.rs 的 total_lines 口径)。
    #[test]
    fn line_count_matches_gutter_convention() {
        let p = LineParams::factory();
        assert_eq!(line_model("", p).len(), 1);
        assert_eq!(line_model("a", p).len(), 1);
        assert_eq!(line_model("a\n", p).len(), 2, "末尾换行产出尾空行");
        assert_eq!(line_model("a\n\nb", p).len(), 3);
    }

    /// ③ 缓存命中与失效(纯层):同 rev 同参数返回同一块内存(指针稳定
    /// = 未重算);编辑推进 rev 才重建;参数变化重建。
    #[test]
    fn cache_rebuilds_only_on_rev_or_params_change() {
        let mut editor = EditorBuffer::new("aaa\n    bb\n你");
        let p = LineParams::factory();
        let mut cache = MinimapCache::default();

        let first = cache.ensure(&editor, p);
        let first_ptr = first.as_ptr();
        assert_eq!(first.len(), 3);
        // 同 rev 命中:指针不动
        assert_eq!(
            cache.ensure(&editor, p).as_ptr(),
            first_ptr,
            "同 rev 同参数命中不重算"
        );

        // 编辑推进 rev → 重建,指针变化、内容更新
        editor.insert_chars(0, "x");
        let rebuilt = cache.ensure(&editor, p);
        let rebuilt_ptr = rebuilt.as_ptr();
        assert_ne!(rebuilt_ptr, first_ptr, "rev 前进后重建");
        assert_eq!(rebuilt.len(), 3);
        assert_eq!(rebuilt[0].width, 4, "新文本首行 xaaa 宽 4");

        // 空闲(无编辑)依旧命中新表
        assert_eq!(
            cache.ensure(&editor, p).as_ptr(),
            rebuilt_ptr,
            "无编辑帧命中未重算"
        );

        // 参数变化(tab 口径变)→ 重建
        let p2 = LineParams {
            tab_units: 8,
            max_units: 96,
        };
        let via_p2_ptr = cache.ensure(&editor, p2).as_ptr();
        assert_ne!(via_p2_ptr, rebuilt_ptr, "参数变化重建");
        // 换回原参数:params 槽已被 p2 覆盖,须重建回 tab=4 口径
        assert_eq!(
            cache.ensure(&editor, p)[1].indent,
            4,
            "tab=4 口径的缩进保持"
        );
    }

    /// ③ 缓存按修订号键控而非内容:undo 回到相同文本但 rev 不同,照键控
    /// 语义重建(与预览快照 `synced_rev` 同语义,rev 单调不回退)。
    #[test]
    fn cache_keys_on_revision_not_content() {
        let mut editor = EditorBuffer::new("甲");
        let p = LineParams::factory();
        let mut cache = MinimapCache::default();
        let before = cache.ensure(&editor, p).as_ptr();

        editor.insert_chars(1, "乙"); // "甲乙"
        let mid = cache.ensure(&editor, p).as_ptr();
        assert_ne!(mid, before);

        editor.remove_chars(1..2); // 回到 "甲",rev 再 +1
        let after = cache.ensure(&editor, p);
        assert_ne!(after.as_ptr(), mid, "内容相同但 rev 不同,照键控语义重建");
        assert_eq!(after[0].width, 2, "内容正确(回到「甲」)");
    }

    /// ③ per-tab 隔离(egui temp memory 层):两个标签各一槽互不覆盖;
    /// 编辑 A 只失效 A;切走再切回命中 B 的既有行模型(存储指针不变)。
    #[test]
    fn per_tab_slots_isolate_and_survive_tab_switch() {
        let ctx = egui::Context::default();
        let mut doc_a = EditorBuffer::new("a文档\n    缩进");
        let doc_b = EditorBuffer::new("b文档");
        let id_a = editor::tab_editor_id(1);
        let id_b = editor::tab_editor_id(2);
        let p = LineParams::factory();

        with_lines(&ctx, id_a, &doc_a, p, |lines| assert_eq!(lines.len(), 2));
        // B 首帧:记下存储条目里 Vec 的指针(with_lines 借出的就是存储本体)
        let b_ptr = with_lines(&ctx, id_b, &doc_b, p, |lines| lines.as_ptr());

        // 编辑 A → 只失效 A 的槽;B 的 synced_rev 纹丝不动
        doc_a.insert_chars(0, "!");
        with_lines(&ctx, id_a, &doc_a, p, |_| {});
        let a_slot = ctx
            .data(|d| d.get_temp::<MinimapCache>(cache_id(id_a)))
            .unwrap();
        assert_eq!(
            a_slot.synced_rev,
            Some(doc_a.revision()),
            "A 已重建到新 rev"
        );
        let b_slot = ctx
            .data(|d| d.get_temp::<MinimapCache>(cache_id(id_b)))
            .unwrap();
        assert_eq!(
            b_slot.synced_rev,
            Some(doc_b.revision()),
            "B 的缓存不受 A 的编辑影响"
        );

        // 切回 B(往返):命中既有行模型 —— 存储指针与切走前一致
        with_lines(&ctx, id_b, &doc_b, p, |lines| {
            assert_eq!(lines.as_ptr(), b_ptr, "切回 B 命中既有行模型,零重建");
            assert_eq!(lines.len(), 1);
        });
    }

    /// ④ 可见窗口:首/中/尾、滚动比例钳制(10000 行档,3px 比例尺,
    /// 600px 视口:总高 30000、行程 29400)。
    #[test]
    fn window_computes_first_middle_tail() {
        let base = WindowInput {
            minimap_height: 600.0,
            total_lines: 10000,
            scroll_ratio: 0.0,
            row_h: ROW_H,
        };
        // 首:0..200,y 从 0 起
        let top = window(base);
        assert_eq!(top.range(), 0..200);
        assert_eq!(top.y_of(0), Some(0.0));
        assert_eq!(top.y_of(199), Some(597.0));
        assert_eq!(top.y_of(200), None, "区间外无 y");

        // 中:比例 0.5 → 平移 14700,首行 4900,窗口顶行贴视口顶
        let mid = window(WindowInput {
            scroll_ratio: 0.5,
            ..base
        });
        assert_eq!(mid.range(), 4900..5100);
        assert_eq!(mid.y_of(4900), Some(0.0));
        // y 随行号单调递增,步长 = 比例尺
        let ys: Vec<f32> = mid.iter().map(|(_, y)| y).collect();
        assert!(ys.windows(2).all(|w| w[1] - w[0] == ROW_H));

        // 尾:比例 1 → 平移 29400,末行 9999 在区间内、不越总行数
        let tail = window(WindowInput {
            scroll_ratio: 1.0,
            ..base
        });
        assert_eq!(tail.range(), 9800..10000);
        assert_eq!(tail.y_of(9999), Some(597.0));
        assert!(tail.range().end <= 10000, "不越总行数");

        // 滚动比例超界钳制
        let over = window(WindowInput {
            scroll_ratio: 3.0,
            ..base
        });
        assert_eq!(over.range(), tail.range(), "比例 >1 钳到 1");
        let under = window(WindowInput {
            scroll_ratio: -1.0,
            ..base
        });
        assert_eq!(under.range(), top.range(), "比例 <0 钳到 0");
    }

    /// ④ 文档短于视口:全部行可见、顶部对齐,滚动比例不影响铺排。
    #[test]
    fn short_document_fills_from_top_without_scroll() {
        let short = WindowInput {
            minimap_height: 600.0,
            total_lines: 10,
            scroll_ratio: 0.0,
            row_h: ROW_H,
        };
        let w = window(short);
        assert_eq!(w.range(), 0..10, "短文档全可见");
        assert_eq!(w.y_of(0), Some(0.0));
        assert_eq!(w.y_of(9), Some(27.0), "不满高:10 行只占 30px,余量留底");

        // 编辑器滚不动(内容不满一屏)时 M2 应给比例 0;即便误传非零,
        // 短文档 travel=0,铺排纹丝不动。
        let nudged = window(WindowInput {
            scroll_ratio: 0.7,
            ..short
        });
        assert_eq!(nudged.range(), w.range());
        assert_eq!(nudged.y_of(9), Some(27.0));
    }

    /// ④ 边界几何:半露行(首行 y 为负、视口底 ceil 进一)、零高视口、
    /// 比例尺退化下限。
    #[test]
    fn window_handles_half_visible_and_degenerate_inputs() {
        let base = WindowInput {
            minimap_height: 600.0,
            total_lines: 10000,
            scroll_ratio: 0.0,
            row_h: ROW_H,
        };
        // 平移 1.5px:首行条半露(y=-1.5),区间仍从 0 起
        let half = window(WindowInput {
            scroll_ratio: 1.5 / 29400.0,
            ..base
        });
        assert_eq!(half.range().start, 0);
        assert_eq!(half.y_of(0), Some(-1.5), "半露行 y 为负");
        // 视口底落在 200.5 行处:ceil 进一,第 200 行半露在底
        assert_eq!(half.range().end, 201);

        // 零高视口:至少一行(避免空区间),不 panic
        let zero = window(WindowInput {
            minimap_height: 0.0,
            ..base
        });
        assert_eq!(zero.range().len(), 1);

        // 比例尺退化为 0:钳到 0.5px 下限,行数不发散
        let tiny = window(WindowInput {
            minimap_height: 600.0,
            total_lines: 1000,
            row_h: 0.0,
            scroll_ratio: 0.0,
        });
        assert_eq!(
            tiny.range(),
            0..1000,
            "row_h 钳 0.5 后 1000 行=500px 全可见"
        );
        assert!(tiny.iter().all(|(_, y)| y >= 0.0));
    }

    /// ⑤ 窗口只随滚动平移、行模型不重算:滚动比例扫全程,行模型的存储
    /// 指针与内容纹丝不动(命中路径);窗口首行随比例单调前移。
    #[test]
    fn scrolling_shifts_window_without_touching_model() {
        let ctx = egui::Context::default();
        // "行\n" ×10000 → 10001 行(含尾空行,与 split('\n') 口径一致)
        let editor = EditorBuffer::new(&"行\n".repeat(10000));
        let id = editor::tab_editor_id(7);
        let p = LineParams::factory();

        let model_ptr = with_lines(&ctx, id, &editor, p, |lines| lines.as_ptr());
        let snapshot: Vec<LineDesc> = with_lines(&ctx, id, &editor, p, |lines| lines.to_vec());

        let mut firsts = Vec::new();
        for i in 0..=10 {
            let ratio = f64::from(i) / 10.0;
            let w = window(WindowInput {
                minimap_height: 600.0,
                total_lines: 10001,
                scroll_ratio: ratio as f32,
                row_h: ROW_H,
            });
            assert!(!w.range().is_empty());
            firsts.push(w.range().start);
            // 每个滚动档都取一次模型:指针与内容都必须与首帧一致
            let (ptr_now, now) = with_lines(&ctx, id, &editor, p, |lines| {
                (lines.as_ptr(), lines.to_vec())
            });
            assert_eq!(ptr_now, model_ptr, "滚动帧不重建行模型(第 {i} 档)");
            assert_eq!(now, snapshot, "内容与首帧逐元素相等");
        }
        // 窗口确实随比例平移:首行单调不减,顶档从 0、尾档贴文末
        assert_eq!(firsts.first(), Some(&0), "顶档从 0 起");
        assert_eq!(
            firsts.last(),
            Some(&9801),
            "尾档停在 9801(10001 行:平移 29403px、首行 floor(29403/3))"
        );
        assert!(
            firsts.windows(2).all(|w| w[1] >= w[0]),
            "比例单调 → 首行单调({firsts:?})"
        );
    }

    /// ⑤ 大文档档位(#55 否决线的 10000 行):建表一次成器,行数与全表
    /// 封顶不变量成立;耗时口径归 perf-recheck 基准,此处钉正确性。
    #[test]
    fn ten_thousand_line_document_builds_correctly() {
        let p = LineParams::factory();
        let text = (0..10000)
            .map(|i| {
                if i % 10 == 0 {
                    format!("# 标题{i}")
                } else if i % 5 == 0 {
                    format!("    缩进行{i}")
                } else {
                    format!("普通行 {i}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let model = line_model(&text, p);
        assert_eq!(model.len(), 10000);
        // "# 标题0" = 1 + 1 + 2 + 2 + 1
        assert_eq!(model[0].width, 7);
        assert_eq!(model[5].indent, 4, "第 6 行是缩进行");
        assert!(model.iter().all(|d| d.indent <= p.max_units));
        assert!(
            model
                .iter()
                .all(|d| d.width.saturating_add(d.indent) <= p.max_units),
            "缩进+长度联合封顶"
        );
    }

    // —— M2 渲染与跳转(#55)——

    use crate::live::{LiveState, RenderMode};
    use crate::state::{OutlineCursor, PreviewState};
    use egui::epaint::ClippedShape;

    /// 跑一帧源码模式编辑面板(带 minimap 开关;视口 800×600 与 editor.rs
    /// 滚动测试同口径),返回 (shapes, 探针)。探针 None = 本帧没进 minimap
    /// 渲染路径(关闭态的零元素断言就用它)。
    fn frame(
        ctx: &egui::Context,
        editor: &mut EditorBuffer,
        now: f64,
        show_minimap: bool,
        events: Vec<egui::Event>,
    ) -> (Vec<ClippedShape>, Option<MinimapProbe>) {
        frame_sized(
            ctx,
            editor,
            now,
            show_minimap,
            events,
            egui::vec2(800.0, 600.0),
        )
    }

    /// 同 [`frame`],视口尺寸参数化(大视口回归用:真机报告小窗可拖/全屏
    /// 不可拖,先在 editor 层排除尺寸相关)。
    fn frame_sized(
        ctx: &egui::Context,
        editor: &mut EditorBuffer,
        now: f64,
        show_minimap: bool,
        events: Vec<egui::Event>,
        size: egui::Vec2,
    ) -> (Vec<ClippedShape>, Option<MinimapProbe>) {
        let mut preview = PreviewState::new(editor);
        let mut cursor = OutlineCursor::default();
        let mut selection = None;
        let mut pending = None;
        let mut live = LiveState::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), size);
        let editor_id = editor::tab_editor_id(1);
        let output = ctx.run_ui(
            egui::RawInput {
                events,
                time: Some(now),
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                editor::ui(
                    ui,
                    editor,
                    &mut preview,
                    &crate::ai::AiState::default(),
                    editor::CursorChannel {
                        cursor: &mut cursor,
                        selection: &mut selection,
                        pending: &mut pending,
                    },
                    &mut live,
                    RenderMode::Source,
                    editor_id,
                    show_minimap,
                    // 打字机/专注关:本模块测试验 minimap 自身路径
                    false,
                    false,
                    None,
                    false,
                    &crate::keymap::Keymap::builtin(),
                    &mut Vec::new(),
                    &[],
                    &mut None,
                );
            },
        );
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        let probe = ctx.data(|d| d.get_temp::<MinimapProbe>(probe_id(editor_id)));
        (shapes, probe)
    }

    /// shapes 里的 minimap 行条:高恰为 [`ROW_H`]、宽 ≤ 条区 96px 的填充
    /// 矩形(行条是全帧唯一的 3px 高矩形形状)。返回 (矩形, 颜色)。
    fn bar_rects(shapes: &[ClippedShape]) -> Vec<(egui::Rect, egui::Color32)> {
        shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(r) if r.fill != egui::Color32::TRANSPARENT => {
                    let rect = r.rect;
                    (rect.height() == ROW_H && rect.width() <= 96.0).then_some((rect, r.fill))
                }
                _ => None,
            })
            .collect()
    }

    /// shapes 里的视口高亮框:横跨整条、宽 = [`MINIMAP_W`] 的填充矩形。
    fn viewport_rect(shapes: &[ClippedShape]) -> Option<egui::Rect> {
        shapes.iter().find_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(r) => (r.fill != egui::Color32::TRANSPARENT
                && (r.rect.width() - MINIMAP_W).abs() < 0.5)
                .then_some(r.rect),
            _ => None,
        })
    }

    /// ⑤-1 关闭态零渲染(否决线):开关关 → 无探针、无行模型缓存、无行条
    /// 形状,TextEdit 右缘保持让位前的现状(不因开关存在而变窄)。
    #[test]
    fn disabled_minimap_renders_zero_elements() {
        let ctx = egui::Context::default();
        let text = (0..500)
            .map(|_| "普通的一行")
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = EditorBuffer::new(&text);
        let id = editor::tab_editor_id(1);

        frame(&ctx, &mut editor, 0.0, false, Vec::new());
        let (shapes, probe) = frame(&ctx, &mut editor, 0.1, false, Vec::new());
        assert!(probe.is_none(), "关闭帧不写探针");
        assert!(
            ctx.data(|d| d.get_temp::<MinimapCache>(cache_id(id)))
                .is_none(),
            "关闭帧不进行模型缓存路径"
        );
        assert!(bar_rects(&shapes).is_empty(), "关闭帧零行条形状");
        assert!(viewport_rect(&shapes).is_none(), "关闭帧零高亮框形状");
        // 关闭 = TextEdit 吃满剩余宽(与 #55 之前一致):右缘贴近视口右缘
        let text_rect = ctx.read_response(id).expect("TextEdit 响应已记录").rect;
        assert!(
            (800.0 - text_rect.right()).abs() < 12.0,
            "关闭时 TextEdit 右缘贴面板(实测 {}),不被窄条挤窄",
            text_rect.right()
        );
    }

    /// ⑤-2 开启态可见行条数 = 窗口计算结果:1000 行全非空文档、600px
    /// minimap、顶部对齐 → 行条数恰为 window() 区间长;行条都落在窄条
    /// 96px 行条区内,TextEdit 同步让位。
    #[test]
    fn enabled_minimap_paints_exactly_the_visible_window() {
        let ctx = egui::Context::default();
        let text = (0..1000)
            .map(|_| "普通的一行")
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = EditorBuffer::new(&text);
        let id = editor::tab_editor_id(1);

        frame(&ctx, &mut editor, 0.0, true, Vec::new());
        let (shapes, probe) = frame(&ctx, &mut editor, 0.1, true, Vec::new());
        let expect = window(WindowInput {
            minimap_height: 600.0,
            total_lines: 1000,
            scroll_ratio: 0.0,
            row_h: ROW_H,
        })
        .range()
        .len();
        // 1000 行全非空:非零行条数 == 窗口区间长
        let probe = probe.expect("开启帧写探针");
        assert_eq!(probe.bars, expect);
        let bars = bar_rects(&shapes);
        assert_eq!(bars.len(), expect, "行条形状数与探针一致");
        // 高亮框探针与 shapes 实画一致(取证两面互证)
        assert_eq!(
            probe.viewport,
            Some(viewport_rect(&shapes).expect("高亮框已绘制")),
            "探针高亮框与实画形状一致"
        );
        // 行条几何:都在右缘窄条的行条区(x ∈ [map_left+pad, +96]),y 从 0 递增
        let map_left = viewport_rect(&shapes).expect("高亮框已绘制").left();
        for (rect, _) in &bars {
            assert!(
                rect.left() >= map_left && rect.right() <= map_left + MINIMAP_W,
                "行条落在窄条内({rect:?}, map_left={map_left})"
            );
        }
        // TextEdit 让位:右缘离面板右缘 ≈ MINIMAP_W(±16 容差吸收 gutter
        // 右缘 token 间距与 TextEdit 自身边距)
        let text_rect = ctx.read_response(id).expect("TextEdit 响应已记录").rect;
        assert!(
            (800.0 - text_rect.right() - MINIMAP_W).abs() < 16.0,
            "开启时 TextEdit 右缘让出窄条(实测右缘 {})",
            text_rect.right()
        );
    }

    /// ⑤-3 跳转映射(纯函数):点击处对准视口中心;点顶/点底钳到端点,
    /// 短文档与不满屏内容返回 None,位置 → 目标单调。
    #[test]
    fn jump_ratio_maps_pointer_to_centered_target() {
        // 10000 行、600px minimap(200 行窗口)、ve = 600/30000
        let base = JumpInput {
            minimap_height: 600.0,
            total_lines: 10000,
            scroll_ratio: 0.0,
            row_h: ROW_H,
            pointer_y: 0.0,
            viewport_frac: 600.0 / 30000.0,
            grab: None,
        };
        // 点顶:目标钳 0
        assert_eq!(
            jump_ratio(JumpInput {
                pointer_y: 0.0,
                ..base
            }),
            Some(0.0)
        );
        // 点底(y=600):p=0.02 > ve/2 → 正向小步
        let bottom = jump_ratio(JumpInput {
            pointer_y: 600.0,
            ..base
        })
        .expect("长文档有行程");
        assert!(bottom > 0.0 && bottom < 0.05, "点底小步正向(实测 {bottom})");
        // 点中(y=300,此时 p=0.01≈ve/2 略小)…… 用比例推:点在 minimap
        // 中部、当前也在中部时目标 ≈ 中部。直接构造 p=0.5:pointer_y +
        // offset = 15000 → offset = 14700 → ratio = 0.5
        let mid = jump_ratio(JumpInput {
            scroll_ratio: 0.5,
            pointer_y: 300.0,
            ..base
        })
        .expect("长文档有行程");
        assert!((mid - 0.5).abs() < 0.01, "文档中部对准视口中心(实测 {mid})");
        // 单调:指针下移 → 目标比例不减
        let mut previous = f32::NEG_INFINITY;
        for step in 0..=10 {
            let target = jump_ratio(JumpInput {
                pointer_y: f32::from(step as u16) * 60.0,
                ..base
            })
            .expect("长文档有行程");
            assert!(
                target >= previous,
                "指针下移目标单调({previous} → {target})"
            );
            previous = target;
        }
        // 钳制:指针越界(负)仍给端点;滚到底时点视口底 → 文档底(视口
        // 顶对文末,minimap 平移量此时最大)
        assert_eq!(
            jump_ratio(JumpInput {
                pointer_y: -50.0,
                ..base
            }),
            Some(0.0)
        );
        assert_eq!(
            jump_ratio(JumpInput {
                scroll_ratio: 1.0,
                pointer_y: 600.0,
                ..base
            }),
            Some(1.0),
            "minimap 平移到底 + 点视口底 = 文档底"
        );
        // 短文档(minimap 不满一屏)→ 编辑器同样滚不动 → None
        assert_eq!(
            jump_ratio(JumpInput {
                total_lines: 10,
                viewport_frac: 1.0,
                ..base
            }),
            None
        );
        // 内容不满一屏(ve=1)→ None
        assert_eq!(
            jump_ratio(JumpInput {
                viewport_frac: 1.0,
                ..base
            }),
            None
        );
    }

    /// #68 M1 抓取拖动(纯函数):按下帧指针在高亮框内 → 拖动帧「指针
    /// y − 抓取偏移」对准**视口顶**,落地后高亮框 top 恰到该处 —— 抓取
    /// 点相对高亮框不动(与 [`viewport_highlight`] 几何对偶互证);目标
    /// 与当帧 scroll_ratio 无关(绝对目标,平移量被滑轨反解吸收)。
    #[test]
    fn grab_drag_keeps_grab_point_fixed_on_highlight() {
        // 10000 行、600px 窄条、ve=0.01:full=30000、travel=29400、
        // 阴影高 ve·full≈300、滑轨 = 窄条高 − 阴影高 ≈ 300。
        let ve = 0.01_f32;
        let geometry = WindowInput {
            minimap_height: 600.0,
            total_lines: 10000,
            scroll_ratio: 0.0,
            row_h: ROW_H,
        };
        let (top0, height0) = viewport_highlight(geometry, ve);
        assert!(top0.abs() < 1e-3, "顶档阴影贴顶");
        assert!(
            (height0 - 300.0).abs() < 1e-3,
            "阴影高 = ve·full(实测 {height0})"
        );

        // 从中部档起拖:r0=0.4 → 阴影 top≈120;按在框内中部(偏移 90)。
        let (top_mid, _) = viewport_highlight(
            WindowInput {
                scroll_ratio: 0.4,
                ..geometry
            },
            ve,
        );
        assert!((top_mid - 120.0).abs() < 1e-3, "0.4 档阴影 top≈120");
        let grab = 90.0;
        assert!(grab < height0, "抓取点在高亮框内");
        let make = |scroll_ratio: f32, pointer_y: f32| JumpInput {
            minimap_height: 600.0,
            total_lines: 10000,
            scroll_ratio,
            row_h: ROW_H,
            pointer_y,
            viewport_frac: ve,
            grab: Some(grab),
        };

        // 拖动 +100px:目标阴影 top = 指针 − grab = top_mid + 100。
        let pointer = top_mid + grab + 100.0;
        let target = jump_ratio(make(0.4, pointer)).expect("长文档有滑轨");
        // 公式直算(同路径重算,位相等):(指针 − grab)/(窄条高 − ve·full)
        let span = 600.0 - ve * (10000.0_f32 * ROW_H);
        assert_eq!(target, ((pointer - grab) / span).clamp(0.0, 1.0));

        // 落地对偶:target 作滚动比例,高亮框 top 恰到「指针 − grab」——
        // 抓取点 = 指针 − top = grab 不变(两套浮点路径,容差互证)。
        let (top1, _) = viewport_highlight(
            WindowInput {
                scroll_ratio: target,
                ..geometry
            },
            ve,
        );
        assert!(
            (top1 - (pointer - grab)).abs() < 1e-3,
            "阴影 top 跟随指针平移(期望 ≈{},实测 {top1})",
            pointer - grab
        );
        assert!(
            (pointer - top1 - grab).abs() < 1e-3,
            "抓取点相对高亮框不动(实测偏移 {})",
            pointer - top1
        );

        // 反向拖回 −200px:同一把尺子量回去,不动性照旧。
        let pointer_up = pointer - 200.0;
        let target_up = jump_ratio(make(0.4, pointer_up)).expect("长文档有滑轨");
        let (top_up, _) = viewport_highlight(
            WindowInput {
                scroll_ratio: target_up,
                ..geometry
            },
            ve,
        );
        assert!(
            (pointer_up - top_up - grab).abs() < 1e-3,
            "反向拖动抓取点同样不动(实测偏移 {})",
            pointer_up - top_up
        );

        // 绝对目标:同指针下目标与当帧 scroll_ratio 无关(逐位相等)。
        assert_eq!(jump_ratio(make(0.0, pointer)), Some(target));
        assert_eq!(jump_ratio(make(1.0, pointer)), Some(target));
    }

    /// #68 M1 grab=None 等价旧行为(**逐字节**):网格扫 scroll_ratio ×
    /// pointer_y,输出与居中公式同路径独立重算位相等;居中语义抽查(点
    /// 中部 → 视口中心,框外点击的现状回归);同位置走 grab 语义输出
    /// 与之分叉(两条路径互不渗透)。
    #[test]
    fn grab_none_equals_legacy_centered_target_byte_for_byte() {
        let ve = 0.01_f32;
        let full = 10000.0_f32 * ROW_H;
        let viewport = 600.0_f32;
        let travel = (full - viewport).max(0.0);
        let make = |scroll_ratio: f32, pointer_y: f32| JumpInput {
            minimap_height: viewport,
            total_lines: 10000,
            scroll_ratio,
            row_h: ROW_H,
            pointer_y,
            viewport_frac: ve,
            grab: None,
        };
        for scroll_ratio in [0.0_f32, 0.3, 0.7, 1.0] {
            for pointer_y in [-50.0_f32, 0.0, 150.0, 300.0, 450.0, 600.0, 650.0] {
                // 旧公式同路径重算(offset → p → target,钳制同款)
                let offset = scroll_ratio.clamp(0.0, 1.0) * travel;
                let p = ((pointer_y + offset) / full).clamp(0.0, 1.0);
                let expect = Some(((p - ve * 0.5) / (1.0 - ve)).clamp(0.0, 1.0));
                assert_eq!(
                    jump_ratio(make(scroll_ratio, pointer_y)),
                    expect,
                    "grab=None 逐字节等价旧行为(scroll {scroll_ratio},y {pointer_y})"
                );
            }
        }
        // 居中语义抽查:停在中部、点中部 → 目标 ≈ 中部(现状回归)。
        let mid = jump_ratio(make(0.5, 300.0)).expect("长文档有行程");
        assert!(
            (mid - 0.5).abs() < 1e-3,
            "无 grab 点击仍居中对准(实测 {mid})"
        );
        // 分叉:同位置走 grab 语义,输出落在滑轨公式上,与居中值不同。
        let grabbed = jump_ratio(JumpInput {
            grab: Some(90.0),
            ..make(0.5, 300.0)
        })
        .expect("长文档有滑轨");
        assert_ne!(grabbed, mid, "grab 语义与居中语义是两条路径");
        assert!(
            (grabbed - 210.0 / 300.0).abs() < 1e-3,
            "grab 目标 = (指针−grab)/滑轨(实测 {grabbed})"
        );
    }

    /// #68 M1 端点与钳制:抓在阴影**下缘**拖到窄条底 → 文档底(阴影贴
    /// 底);抓在阴影顶拖到窄条顶 → 文档顶;指针越出窄条钳 0/1;grab
    /// 固定时目标随指针单调不减。
    #[test]
    fn grab_edges_reach_document_endpoints_and_clamp() {
        let ve = 0.01_f32;
        let geometry = WindowInput {
            minimap_height: 600.0,
            total_lines: 10000,
            scroll_ratio: 0.0,
            row_h: ROW_H,
        };
        let (_, height) = viewport_highlight(geometry, ve);
        let make = |pointer_y: f32, grab: f32| JumpInput {
            minimap_height: 600.0,
            total_lines: 10000,
            scroll_ratio: 0.0,
            row_h: ROW_H,
            pointer_y,
            viewport_frac: ve,
            grab: Some(grab),
        };
        // 抓阴影下缘拖到窄条底:目标 = 1(文档底),高亮框贴底
        let bottom = jump_ratio(make(600.0, height)).expect("长文档有滑轨");
        assert_eq!(bottom, 1.0, "下缘抓到底 = 文档底");
        let (top_at_bottom, _) = viewport_highlight(
            WindowInput {
                scroll_ratio: bottom,
                ..geometry
            },
            ve,
        );
        assert!(
            (top_at_bottom - (600.0 - height)).abs() < 1e-3,
            "文档底时阴影贴窄条底(实测 {top_at_bottom},期望 ≈{})",
            600.0 - height
        );
        // 抓阴影顶拖到窄条顶:目标 = 0(文档顶),阴影贴顶
        assert_eq!(
            jump_ratio(make(0.0, 0.0)).expect("长文档有滑轨"),
            0.0,
            "顶缘抓到顶 = 文档顶"
        );
        // 越端钳制:拖出窄条上下沿,目标仍钳在 0..=1
        assert_eq!(jump_ratio(make(-50.0, 0.0)), Some(0.0), "拖出窄条顶钳 0");
        assert_eq!(jump_ratio(make(700.0, height)), Some(1.0), "拖出窄条底钳 1");
        // 单调:grab 固定,指针下移目标不减
        let mut previous = f32::NEG_INFINITY;
        for step in 0..=10 {
            let target =
                jump_ratio(make(f32::from(step as u16) * 60.0, 90.0)).expect("长文档有滑轨");
            assert!(
                target >= previous,
                "指针下移目标单调({previous} → {target})"
            );
            previous = target;
        }
    }

    /// #68 M1 None 口径(**2026-10-09 修订**):只剩下的真闸门是「内容不满一屏」
    /// (`ve ≥ 1`,编辑器确实无行程)与「滑轨退化」(`span ≤ 0`,阴影高 ≥ 可走
    /// 行程)。**「minimap 铺不满窄条」(`travel ≤ 0`)不再是 None** —— 那是
    /// 2026-10-08/10-09 两次真机「小窗可拖、全屏不可拖」的根因:同一种 actionable
    /// 文档在宽高都变大后 minimap 反而全哑(详见 [`jump_ratio`] 的量级失配注记)。
    /// 修订后`None` 语义收敛成一句话:**编辑器滚不动才不跳**。
    #[test]
    fn grab_keeps_none_precedence_on_short_or_degenerate_inputs() {
        let make = |total_lines: usize, viewport_frac: f32| JumpInput {
            minimap_height: 600.0,
            total_lines,
            scroll_ratio: 0.0,
            row_h: ROW_H,
            pointer_y: 300.0,
            viewport_frac,
            grab: Some(90.0),
        };
        // 内容不满一屏(ve=1):编辑器无行程 —— 修订前后同为 None。
        assert_eq!(
            jump_ratio(make(10000, 1.0)),
            None,
            "内容不满一屏,grab 也不跳"
        );
        // 滑轨退化:阴影高 ve·full ≥ 可走行程(10000 行 ×3px=30000、ve=0.05
        // → 阴影 1500px,而窄条仅 600px 且意味着编辑器视口只看得见 50 行)。
        assert_eq!(
            jump_ratio(make(10000, 0.05)),
            None,
            "阴影高 ≥ 可走行程:无滑轨,原样不动"
        );

        // —— 修订点:minimap 铺不满窄条但编辑器滚得动时必须能拖 ——
        // 100 行 × 3px = 300px < 600px 窄条(travel = 0),而编辑器视口只占
        // 内容 45% → 有行程。这是全屏下真实的几何(行条 3px vs 编辑器 ≥18px
        // 行高,同一篇文档在两侧的行数阈值差一个量级)。
        let full = 100.0 * ROW_H;
        let expect_span = (1.0 - 0.45_f32) * full; // travel=0 时的滑轨
        assert!(expect_span > 0.0);
        assert_eq!(
            jump_ratio(make(100, 0.45)),
            Some(((300.0 - 90.0) / expect_span).clamp(0.0, 1.0)),
            "短文档(minimap 未满屏)仍走滑轨公式,不再哑"
        );
        // 与 [`viewport_highlight`] 对偶:落地后阴影 top 恰 = 指针 − 抓取偏移。
        // travel=0 时 top = ratio×span,「位移与反解同一把尺子」仍成立。
        let target = jump_ratio(make(100, 0.45)).expect("短文档也有滑轨");
        let (top, _) = viewport_highlight(
            WindowInput {
                minimap_height: 600.0,
                total_lines: 100,
                scroll_ratio: target,
                row_h: ROW_H,
            },
            0.45,
        );
        let expected_top: f32 = 210.0_f32.min(expect_span);
        assert!(
            (top - expected_top).abs() < 1e-3,
            "短文档拖影落地仍保抓取点不动(期望 ≈{expected_top},实测 {top})"
        );
    }

    /// #68 量级失配的回归锁(坤哥 2026-10-08 / 10-09 两次真机「小窗可拖、
    /// 全屏不可拖」):同一份**200 行**文档,小视口下 minimap 满屏可拖,大视口
    /// (1920×1008)下 minimap 行条总高 600px 铺不满 ~1000px 窄条 → 修订前的
    /// `jump_ratio` 在第一道闸门就返回 None,编辑器明明有 2000px 行程却拖不动。
    ///
    /// 为何必须端到端:这是 `editor` 层才拿得到的真实条件(ScrollArea 的
    /// `viewport_height`/`content_height` 决定 `viewport_frac`),纯函数层的
    /// 用例凭构造就能绕过。**撤销性强 hip — revert 掉 `jump_ratio` 的闸门修订
    /// 本测试当场红**(拖 6 步 offset 恒 0)。
    #[test]
    fn dragging_short_document_works_at_fullscreen_viewport() {
        let ctx = egui::Context::default();
        // 200 行:editor 层 1920×1008 下 content ≈3000px vs viewport ≈1000px
        // (编辑器滚得动),而 minimap 全高 600px < 窄条(铺不满)。
        let text = (0..200)
            .map(|_| "普通的一行")
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = EditorBuffer::new(&text);
        let id = editor::tab_editor_id(1);
        let big = egui::vec2(1920.0, 1008.0);

        frame_sized(&ctx, &mut editor, 0.0, true, Vec::new(), big);
        let (shapes, _) = frame_sized(&ctx, &mut editor, 0.1, true, Vec::new(), big);
        let metrics0 = ctx
            .data(|d| d.get_temp::<ScrollMetrics>(metrics_id(id)))
            .unwrap_or_default();
        assert!(
            metrics0.content_height - metrics0.viewport_height > 100.0,
            "前提:编辑器自身有行程(内容 {} / 视口 {}),否则本用例无意义",
            metrics0.content_height,
            metrics0.viewport_height
        );
        assert!(
            200.0 * ROW_H < metrics0.viewport_height,
            "前提:minimap 全高 {} 铺不满窄条 {}(正在复现的那个几何)",
            200.0 * ROW_H,
            metrics0.viewport_height
        );

        let map = viewport_rect(&shapes).expect("高亮框定位窄条");
        let press = egui::pos2(map.center().x, map.top() + 20.0);
        frame_sized(
            &ctx,
            &mut editor,
            0.2,
            true,
            vec![
                egui::Event::PointerMoved(press),
                egui::Event::PointerButton {
                    pos: press,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            big,
        );

        let mut previous = metrics0.offset;
        let mut moved = 0;
        for step in 0..6 {
            let pos = egui::pos2(press.x, press.y + f32::from(step as u16) * 40.0);
            frame_sized(
                &ctx,
                &mut editor,
                0.3 + f64::from(step) * 0.1,
                true,
                vec![egui::Event::PointerMoved(pos)],
                big,
            );
            let offset = ctx
                .data(|d| d.get_temp::<ScrollMetrics>(metrics_id(id)))
                .map_or(0.0, |m| m.offset);
            assert!(
                offset >= previous - 0.5,
                "短文档大视口拖动单调不减(第 {step} 步 {previous} → {offset})"
            );
            if offset > previous + 1.0 {
                moved += 1;
            }
            previous = offset;
        }
        assert!(
            moved >= 3,
            "minimap 铺不满窄条时仍然可拖(6 步中 {moved} 步在滚;窄条 {map:?})"
        );
    }

    /// ⑤-3 端到端:点击 minimap 中部 → 编辑器滚动前进,光标纹丝不动
    /// (跳转只动滚动 offset,不触碰 TextEdit 持久光标)。
    #[test]
    fn clicking_minimap_scrolls_without_moving_cursor() {
        let ctx = egui::Context::default();
        let text = (0..500)
            .map(|_| "普通的一行")
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = EditorBuffer::new(&text);
        let id = editor::tab_editor_id(1);

        frame(&ctx, &mut editor, 0.0, true, Vec::new());
        let (shapes, _) = frame(&ctx, &mut editor, 0.1, true, Vec::new());
        // 高亮框只标编辑器视口对应的短段;点击目标取窄条自身的中下部
        // (窄条高 = 编辑区高 600,y=400 离当前视口中心够远,滚动量明确)。
        let map = viewport_rect(&shapes).expect("高亮框定位窄条");
        let click = egui::pos2(map.center().x, 400.0);
        let top_before = ctx.read_response(id).expect("TextEdit 已记录").rect.top();
        let cursor_before = egui::widgets::text_edit::TextEditState::load(&ctx, id)
            .and_then(|s| s.cursor.char_range())
            .map(|r| (r.primary.index.0, r.secondary.index.0));

        // 点击:press 与 release 分两帧(clicked = 双帧语义)
        frame(
            &ctx,
            &mut editor,
            0.2,
            true,
            vec![
                egui::Event::PointerMoved(click),
                egui::Event::PointerButton {
                    pos: click,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        frame(
            &ctx,
            &mut editor,
            0.3,
            true,
            vec![egui::Event::PointerButton {
                pos: click,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        // 落账链条比滚轮多一环(意图帧→闭包消费帧→offset 生效帧→布局
        // 反映帧→read_response 可读帧),补四帧再取证。
        for step in 0..4 {
            frame(
                &ctx,
                &mut editor,
                0.4 + f64::from(step) * 0.1,
                true,
                Vec::new(),
            );
        }

        let top_after = ctx.read_response(id).expect("TextEdit 已记录").rect.top();
        assert!(
            top_after < top_before - 50.0,
            "点击 minimap 把视口推离文档顶(实测 {top_before} → {top_after})"
        );
        let cursor_after = egui::widgets::text_edit::TextEditState::load(&ctx, id)
            .and_then(|s| s.cursor.char_range())
            .map(|r| (r.primary.index.0, r.secondary.index.0));
        assert_eq!(
            cursor_before, cursor_after,
            "跳转不动光标(前后持久光标一致)"
        );
    }

    /// #68 落地语义的回归锁:从**非零 offset** 起手的纯点击落在自己的
    /// 绝对目标上;同位置再点一次不在此之上累加。
    ///
    /// 为何必须单开这条:`land` 由「rect 顶 = clip.top + target×travel」改成
    /// 「clip.top + (target×travel − metrics.offset)」,差的正是当帧 offset
    /// 这一项 —— offset=0 时两式恒等,而既有点击测试(`clicking_minimap_…`)
    /// 与三条端到端拖影测试全部从文档顶起步,对这次修正完全不敏感。本条
    /// 把起点挪到非零 offset,补上这条路径的回归锁。取证读 `metrics.offset`
    /// (ScrollArea 帧末真值),不受 `read_response` 再滞后一帧的影响。
    ///
    /// 三次点击的**意图数值各不相同**(第二、三次的 `scroll_ratio` 已非零,
    /// 居中公式的输入随之变化),所以钉的断言不是「三次 offset 相等」,而是
    /// 「每一次都落在自己当帧意图算出的绝对目标上」——期望经与生产同源的
    /// `jump_ratio` 纯函数独立算出(输入全部取自被测 ScrollArea 同一帧
    /// metrics 真值),不由实现自证;容差 12px。累加式实现(旧式)从第二次
    /// 起每调用就多走一份 target,三条断言立刻红(已做反向验证:把 land
    /// 改回旧式,本条在第二次断言处红 —— 期望 ≈1983.9、实测 3184.9)。
    #[test]
    fn repeated_click_from_nonzero_offset_does_not_accumulate() {
        let ctx = egui::Context::default();
        let text = (0..500)
            .map(|_| "普通的一行")
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = EditorBuffer::new(&text);
        let id = editor::tab_editor_id(1);

        let mut now = 0.0_f64;
        let tick = |now: &mut f64| {
            *now += 0.1;
            *now
        };
        let read = |ctx: &egui::Context| {
            ctx.data(|d| d.get_temp::<ScrollMetrics>(metrics_id(id)))
                .unwrap_or_default()
        };

        frame(&ctx, &mut editor, tick(&mut now), true, Vec::new());
        let (shapes, _) = frame(&ctx, &mut editor, tick(&mut now), true, Vec::new());
        // 文档顶时高亮框贴窄条顶 → 框顶即窄条顶,横向中点也即窄条中点。
        let strip = viewport_rect(&shapes).expect("高亮框定位窄条");
        let (click_x, map_top) = (strip.center().x, strip.top());
        // 取样点:窄条内一处固定屏幕 y(这里 map_top = 0,屏幕 y 即窄条局部
        // y,`pointer_local` 就是生产传给 `jump_ratio` 的那个同名量)。
        let click = egui::pos2(click_x, 300.0);
        let pointer_local = click.y - map_top;

        // 此刻若发生一次纯点击会产生的**绝对目标 offset**:与生产同一条
        // `jump_ratio` 纯函数(grab=None 居中语义),输入取自同一帧 metrics。
        let intent_offset = |m: &ScrollMetrics, from: f32| {
            let travel = (m.content_height - m.viewport_height).max(0.0);
            let target = jump_ratio(JumpInput {
                minimap_height: m.viewport_height,
                total_lines: 500,
                scroll_ratio: (from / travel).clamp(0.0, 1.0),
                row_h: ROW_H,
                pointer_y: pointer_local,
                viewport_frac: (m.viewport_height / m.content_height).clamp(0.0, 1.0),
                grab: None,
            })
            .expect("长文档有行程");
            target * travel
        };
        // 一次完整纯点击:press 帧 → release 帧(clicked 是双帧语义)→ 空帧
        // 补足「意图→消费→offset 生效→帧末 metrics」这条链路。
        let click_once = |now: &mut f64, ctx: &egui::Context, editor: &mut EditorBuffer| {
            frame(
                ctx,
                editor,
                tick(now),
                true,
                vec![
                    egui::Event::PointerMoved(click),
                    egui::Event::PointerButton {
                        pos: click,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            frame(
                ctx,
                editor,
                tick(now),
                true,
                vec![egui::Event::PointerButton {
                    pos: click,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            for _ in 0..8 {
                frame(ctx, editor, tick(now), true, Vec::new());
            }
            read(ctx).offset
        };

        let base = read(&ctx);
        assert_eq!(base.offset, 0.0, "起手势前在文档顶");
        assert!(
            base.content_height > base.viewport_height + 100.0,
            "文档有足够行程(内容高 {},视口高 {})",
            base.content_height,
            base.viewport_height
        );

        // 第一次:与既有点击测试同一起点(文档顶),把这一次的目标也钉住
        // —— 它是后两次断言的基准,顺手确认 click_once 这一支真在跳转。
        let expect_first = intent_offset(&base, base.offset);
        assert!(
            expect_first > 100.0,
            "第一次点击的目标是深入文档的一处(实测目标 {expect_first})"
        );
        let after_first = click_once(&mut now, &ctx, &mut editor);
        assert!(
            (after_first - expect_first).abs() < 12.0,
            "第一次落在自己的绝对目标上(期望 ≈{expect_first},实测 {after_first})"
        );

        // 第二次:**非零 offset 起点** —— 新旧两式的分水岭。
        let before_second = read(&ctx);
        let expect_second = intent_offset(&before_second, after_first);
        assert!(
            (expect_second - after_first).abs() > 20.0,
            "第二次的目标与当前落点拉开距离(当前 {after_first},目标 {expect_second}),才谈得上『有没有累加』"
        );
        let after_second = click_once(&mut now, &ctx, &mut editor);
        assert!(
            (after_second - expect_second).abs() < 12.0,
            "第二次从非零 offset 起,落在自己的绝对目标上(期望 ≈{expect_second},实测 {after_second})"
        );

        // 第三次:同一支路再来一次 —— 「每次调用都再累加一遍」最露骨的一帧
        // (旧式在这一帧会再叠一份 ≈1200px 的位移)。
        let before_third = read(&ctx);
        let expect_third = intent_offset(&before_third, after_second);
        let after_third = click_once(&mut now, &ctx, &mut editor);
        assert!(
            (after_third - expect_third).abs() < 12.0,
            "第三次同样落在自己的绝对目标上(期望 ≈{expect_third},实测 {after_third})"
        );
        // 兜底量级锁(不依赖上面的公式):三次意图都走居中、**不看反馈**,
        // 落点应同量级;累加式实现一次比一次远,2× 余量足够把它卡住。
        assert!(
            after_second < after_first * 2.0 && after_third < after_second * 2.0,
            "三次落点同量级、不逐次翻倍(第一次 {after_first},第二次 {after_second},第三次 {after_third})"
        );
    }

    /// #68 尺寸回归(坤哥 2026-10-08 真机报告:小窗可拖、全屏不可拖):大视口
    /// (1920×1008)下抓住阴影拖动,offset 逐帧跟随——与既有拖动测试同流程,
    /// 唯一变量=视口尺寸。editor 层不复现则 bug 在 layout 层面板分配。
    ///
    /// **本测试用的 500 行是「够长」的一档**:`jump_ratio` 曾有的「minimap 铺
    /// 不满窄条即哑」闸门要 >~300 行才放行。缺的那一半(更短的文档)与 [#68]
    /// 的量级失配修复一起补齐了 —— 见
    /// `dragging_short_document_works_at_fullscreen_viewport`(200 行、同一视口)。
    #[test]
    fn dragging_minimap_works_at_fullscreen_viewport() {
        let ctx = egui::Context::default();
        let text = (0..500)
            .map(|_| "普通的一行")
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = EditorBuffer::new(&text);
        let id = editor::tab_editor_id(1);
        let big = egui::vec2(1920.0, 1008.0);

        frame_sized(&ctx, &mut editor, 0.0, true, Vec::new(), big);
        let (shapes, _) = frame_sized(&ctx, &mut editor, 0.1, true, Vec::new(), big);
        let map = viewport_rect(&shapes).expect("大视口下高亮框仍在");
        let press = egui::pos2(map.center().x, map.top() + 40.0);
        frame_sized(
            &ctx,
            &mut editor,
            0.2,
            true,
            vec![
                egui::Event::PointerMoved(press),
                egui::Event::PointerButton {
                    pos: press,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            big,
        );

        let mut previous = 0.0_f32;
        let mut moved = 0;
        for step in 0..6 {
            let pos = egui::pos2(press.x, press.y + f32::from(step as u16) * 40.0);
            frame_sized(
                &ctx,
                &mut editor,
                0.3 + f64::from(step) * 0.1,
                true,
                vec![egui::Event::PointerMoved(pos)],
                big,
            );
            let offset: f32 = ctx
                .data(|d| d.get_temp::<ScrollMetrics>(metrics_id(id)))
                .map_or(0.0, |m| m.offset);
            assert!(
                offset >= previous - 0.5,
                "大视口拖动单调不减(第 {step} 步 {previous} → {offset})"
            );
            if offset > previous + 1.0 {
                moved += 1;
            }
            previous = offset;
        }
        assert!(
            moved >= 3,
            "大视口拖动确实连续跟随(6 步中 {moved} 步在滚,窄条 {map:?})"
        );
    }

    /// ⑤-3 端到端:按住拖动逐帧下移 → 滚动逐帧跟随(offset 单调前进)。
    #[test]
    fn dragging_minimap_follows_continuously() {
        let ctx = egui::Context::default();
        let text = (0..500)
            .map(|_| "普通的一行")
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = EditorBuffer::new(&text);
        let id = editor::tab_editor_id(1);

        frame(&ctx, &mut editor, 0.0, true, Vec::new());
        let (shapes, _) = frame(&ctx, &mut editor, 0.1, true, Vec::new());
        let map = viewport_rect(&shapes).expect("高亮框定位窄条");
        let press = egui::pos2(map.center().x, map.top() + 40.0);
        frame(
            &ctx,
            &mut editor,
            0.2,
            true,
            vec![
                egui::Event::PointerMoved(press),
                egui::Event::PointerButton {
                    pos: press,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );

        // 逐帧跟随用 metrics.offset 断言(ScrollArea 帧末真值,无布局与
        // read_response 各一帧的滞后);屏幕坐标的最终落点在 settle 帧后验。
        let mut previous = 0.0_f32;
        let mut moved = 0;
        for step in 0..6 {
            let pos = egui::pos2(press.x, press.y + f32::from(step as u16) * 40.0);
            frame(
                &ctx,
                &mut editor,
                0.3 + f64::from(step) * 0.1,
                true,
                vec![egui::Event::PointerMoved(pos)],
            );
            let offset: f32 = ctx
                .data(|d| d.get_temp::<ScrollMetrics>(metrics_id(id)))
                .map_or(0.0, |m| m.offset);
            assert!(
                offset >= previous - 0.5,
                "拖动下移滚动单调不减(第 {step} 步 {previous} → {offset})"
            );
            if offset > previous + 1.0 {
                moved += 1;
            }
            previous = offset;
        }
        assert!(moved >= 3, "拖动确实连续跟随(6 步中 {moved} 步在滚)");
        // settle(意图→消费→offset→布局→可读各差一帧)后,屏幕坐标到位
        for step in 0..4 {
            frame(
                &ctx,
                &mut editor,
                0.9 + f64::from(step) * 0.1,
                true,
                Vec::new(),
            );
        }
        let top = ctx.read_response(id).expect("TextEdit 已记录").rect.top();
        assert!(
            top < -50.0,
            "拖到窄条下部后视口深入文档(实测 top {top},offset {previous})"
        );
    }

    /// 读取某标签当前 grab 通道(#68 白盒锚点):`None` = 通道无值(无手势
    /// 或已释放清零),`Some(None)` = 手势在而按在阴影外,`Some(Some(x))` =
    /// 抓着阴影本体、偏移 x。
    fn grab_slot(ctx: &egui::Context, id: egui::Id) -> Option<Option<f32>> {
        ctx.data(|d| d.get_temp::<Option<f32>>(grab_id(id)))
    }

    /// #68 M2 端到端:按住高亮框本体拖动 → 抓取点相对高亮框不动(阴影
    /// 顶对齐「指针 − 抓取偏移」的滑轨换算,而非视口中心追指针);拖动中
    /// offset 单调跟随;释放帧抓取通道清零。与居中语义的差异断言:抓取
    /// 点选在 0.3×阴影高(远离半高),若实现退回居中,落地后指针相对阴影
    /// 的锚会被读成 ≈ 半高,断言红。
    #[test]
    fn dragging_highlight_body_pins_pointer_to_highlight() {
        let ctx = egui::Context::default();
        let text = (0..500)
            .map(|_| "普通的一行")
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = EditorBuffer::new(&text);
        let id = editor::tab_editor_id(1);

        frame(&ctx, &mut editor, 0.0, true, Vec::new());
        let (shapes, probe) = frame(&ctx, &mut editor, 0.1, true, Vec::new());
        // 初始在文档顶(阴影贴窄条顶),高亮框 top 即窄条 top。
        let map_top = viewport_rect(&shapes).expect("高亮框定位窄条").top();
        let hl0 = probe.expect("探针已写").viewport.expect("高亮框");
        let height = hl0.height();
        assert!(
            height > 60.0 && height < 500.0,
            "阴影高在可拖影区间(实测 {height}):太矮无拖影意义,太高贴满窄条"
        );
        let grab = height * 0.3;
        let press = egui::pos2(hl0.center().x, map_top + grab);
        frame(
            &ctx,
            &mut editor,
            0.2,
            true,
            vec![
                egui::Event::PointerMoved(press),
                egui::Event::PointerButton {
                    pos: press,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        // press 帧判定:按在阴影内,抓取偏移即入通道(判定用上一帧高亮
        // 框,与用户按下时所见一致)。
        assert_eq!(
            grab_slot(&ctx, id),
            Some(Some(grab)),
            "按在阴影内:press 帧记录抓取偏移(指针 − 框顶)"
        );

        // 拖 6 帧每帧 +40px:offset 单调不减(与既有拖动测试同款容差)。
        let mut previous = 0.0_f32;
        for step in 0..6 {
            let pos = egui::pos2(press.x, press.y + f32::from(step as u16) * 40.0);
            frame(
                &ctx,
                &mut editor,
                0.3 + f64::from(step) * 0.1,
                true,
                vec![egui::Event::PointerMoved(pos)],
            );
            let offset: f32 = ctx
                .data(|d| d.get_temp::<ScrollMetrics>(metrics_id(id)))
                .map_or(0.0, |m| m.offset);
            assert!(
                offset >= previous - 0.5,
                "拖影下移滚动单调不减(第 {step} 步 {previous} → {offset})"
            );
            previous = offset;
        }
        // settle(意图→消费→落地→布局→metrics→绘制各差一帧)后验锚定。
        for step in 0..4 {
            frame(
                &ctx,
                &mut editor,
                0.9 + f64::from(step) * 0.1,
                true,
                Vec::new(),
            );
        }
        let pointer_local = press.y + 200.0 - map_top;
        let hl1 = ctx
            .data(|d| d.get_temp::<MinimapProbe>(probe_id(id)))
            .expect("探针已写")
            .viewport
            .expect("高亮框");
        let pinned = pointer_local - (hl1.top() - map_top);
        assert!(
            (pinned - grab).abs() < 3.0,
            "抓取点相对高亮框不动(期望 ≈{grab},实测 {pinned})"
        );
        // 差异断言:退回居中语义时指针会被按在阴影半高处 —— 抓取点选
        // 0.3×高远离半高,两条语义的锚可分;两个方向都断(后者同时是
        // 测试自身有效性的前提,防高度退化到两锚重合)。
        assert!(
            (grab - height * 0.5).abs() > 6.0,
            "测试有效性:抓取点远离半高锚(实测差 {})",
            (grab - height * 0.5).abs()
        );
        assert!(
            (pinned - height * 0.5).abs() > 6.0,
            "落地是滑轨顶对齐(指针 − grab),不是居中(指针 − 半高)(实测锚 {pinned})"
        );

        // 释放:通道清零,下一次按下重新判定。
        frame(
            &ctx,
            &mut editor,
            1.4,
            true,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(press.x, press.y + 200.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(
            grab_slot(&ctx, id).is_none(),
            "释放帧清零抓取通道(下一次按下重新判定)"
        );
    }

    /// #68 M2 端到端:按在高亮框**外**(窄条内)拖动 → 仍走 #55 居中现状
    /// (grab 通道记录「手势在而无抓取」)。拖动的落地是逐帧反馈迭代,
    /// 几何稳态 = 指针贴阴影中心(视口中心对准指针);若框外被误判成
    /// 拖影,稳态会变成指针贴「框顶 + 抓取偏移」,与中心锚差 0.2×高,
    /// 断言必红。
    #[test]
    fn dragging_outside_highlight_keeps_centered_target() {
        let ctx = egui::Context::default();
        let text = (0..500)
            .map(|_| "普通的一行")
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = EditorBuffer::new(&text);
        let id = editor::tab_editor_id(1);

        frame(&ctx, &mut editor, 0.0, true, Vec::new());
        let (shapes, probe) = frame(&ctx, &mut editor, 0.1, true, Vec::new());
        let map_top = viewport_rect(&shapes).expect("高亮框定位窄条").top();
        let hl0 = probe.expect("探针已写").viewport.expect("高亮框");
        let metrics0: ScrollMetrics = ctx
            .data(|d| d.get_temp::<ScrollMetrics>(metrics_id(id)))
            .unwrap_or_default();
        assert_eq!(metrics0.offset, 0.0, "起拖前在文档顶(公式 s=0 前提)");

        // 框外按下:阴影下方 60px,仍在窄条内、x 取窄条中心避让滚动条。
        let press_local = (hl0.top() - map_top) + hl0.height() + 60.0;
        assert!(press_local < 580.0, "取样点留在窄条内(实测 {press_local})");
        let press = egui::pos2(hl0.center().x, map_top + press_local);
        frame(
            &ctx,
            &mut editor,
            0.2,
            true,
            vec![
                egui::Event::PointerMoved(press),
                egui::Event::PointerButton {
                    pos: press,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(
            grab_slot(&ctx, id),
            Some(None),
            "按在阴影外:手势在而无抓取,拖动走居中现状"
        );

        // 拖 3 帧 +40px。落地链与稳态:拖动中意图逐帧带当帧 scroll_ratio,
        // 居中公式是反馈迭代(每帧把视口中心往指针处收),收敛不动点
        // s*=(指针−阴影半高)/滑轨 —— 几何稳态恰是「指针贴阴影中心」
        // (#55 现状拖动的固有语义,与 M1 单次点击的单步公式不同)。
        let mut previous = 0.0_f32;
        for step in 0..3 {
            let pos = egui::pos2(press.x, press.y + f32::from(step as u16) * 40.0);
            frame(
                &ctx,
                &mut editor,
                0.3 + f64::from(step) * 0.1,
                true,
                vec![egui::Event::PointerMoved(pos)],
            );
            let offset: f32 = ctx
                .data(|d| d.get_temp::<ScrollMetrics>(metrics_id(id)))
                .map_or(0.0, |m| m.offset);
            assert!(
                offset >= previous - 0.5,
                "框外拖动滚动单调不减(第 {step} 步 {previous} → {offset})"
            );
            previous = offset;
        }
        // settle 到不动点:egui 的落地带逐帧平滑,反馈链每帧只推进一
        // 部分(实测 40 帧从 204px 锚收敛到 60.0,理论不动点 = 半高
        // 59.97;余量给足帧数,容差 2px ≈ 40 倍实测误差)。
        for step in 0..40 {
            frame(
                &ctx,
                &mut editor,
                0.7 + f64::from(step) * 0.1,
                true,
                Vec::new(),
            );
        }
        let pointer_final_local = press_local + 80.0;
        let hl1 = ctx
            .data(|d| d.get_temp::<MinimapProbe>(probe_id(id)))
            .expect("探针已写")
            .viewport
            .expect("高亮框");
        let center_anchor = pointer_final_local - (hl1.top() - map_top);
        assert!(
            (center_anchor - hl1.height() * 0.5).abs() < 2.0,
            "居中稳态:指针贴阴影中心(期望 ≈{},实测 {center_anchor})",
            hl1.height() * 0.5
        );
        // 与拖影语义分叉:抓着阴影本体时指针贴「框顶 + 0.3×高」(见
        // dragging_highlight_body 测试),两条锚差 0.2×高,远超容差。
        assert!(
            hl1.height() * 0.2 > 6.0,
            "测试有效性:两语义的锚距 0.2×高可分(实测 {})",
            hl1.height() * 0.2
        );
        // 释放清零(与拖影路径同一条清零通路)。
        frame(
            &ctx,
            &mut editor,
            5.0,
            true,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(press.x, press.y + 80.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(grab_slot(&ctx, id).is_none(), "释放帧清零抓取通道");
    }

    /// #68 M2 端到端:释放后再按下**重新判定** —— 拖影落地后阴影已移位,
    /// 新的按下按当帧所见阴影重新算抓取:按新阴影外记「无抓取」,按新
    /// 阴影内记新偏移;纯点击(按下即放)同样先判定、后随点击清零,点击
    /// 意图仍是居中(否决线:点击跳转行为不变)。
    #[test]
    fn release_then_press_re_judges_grab_state() {
        let ctx = egui::Context::default();
        let text = (0..500)
            .map(|_| "普通的一行")
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = EditorBuffer::new(&text);
        let id = editor::tab_editor_id(1);
        let probe_hl = |ctx: &egui::Context| {
            ctx.data(|d| d.get_temp::<MinimapProbe>(probe_id(id)))
                .expect("探针已写")
                .viewport
                .expect("高亮框")
        };
        let press_frame =
            |ctx: &egui::Context, editor: &mut EditorBuffer, now: f64, pos: egui::Pos2| {
                frame(
                    ctx,
                    editor,
                    now,
                    true,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            };
        let release_frame =
            |ctx: &egui::Context, editor: &mut EditorBuffer, now: f64, pos: egui::Pos2| {
                frame(
                    ctx,
                    editor,
                    now,
                    true,
                    vec![egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
            };

        frame(&ctx, &mut editor, 0.0, true, Vec::new());
        let (shapes, _) = frame(&ctx, &mut editor, 0.1, true, Vec::new());
        let map_top = viewport_rect(&shapes).expect("高亮框定位窄条").top();
        let hl0 = probe_hl(&ctx);

        // 第一次:按阴影内 0.25×高,小幅拖影把阴影带下去,释放。
        let grab1 = hl0.height() * 0.25;
        let p1 = egui::pos2(hl0.center().x, hl0.top() + grab1);
        press_frame(&ctx, &mut editor, 0.2, p1);
        assert_eq!(
            grab_slot(&ctx, id),
            Some(Some(grab1)),
            "第一次按下:阴影内,记录偏移"
        );
        for step in 0..2 {
            frame(
                &ctx,
                &mut editor,
                0.3 + f64::from(step) * 0.1,
                true,
                vec![egui::Event::PointerMoved(egui::pos2(
                    p1.x,
                    p1.y + f32::from(step as u16) * 60.0,
                ))],
            );
        }
        for step in 0..3 {
            frame(
                &ctx,
                &mut editor,
                0.5 + f64::from(step) * 0.1,
                true,
                Vec::new(),
            );
        }
        release_frame(&ctx, &mut editor, 0.9, egui::pos2(p1.x, p1.y + 60.0));
        assert!(grab_slot(&ctx, id).is_none(), "释放清零");

        // 第二次:按当前(已移位)阴影外 → 无抓取;纯点击释放后意图是
        // 居中,settle 让它落地,阴影再次移位。
        let hl1 = probe_hl(&ctx);
        assert!(
            hl1.top() > hl0.top(),
            "第一次拖影确实移动了阴影({} → {})",
            hl0.top(),
            hl1.top()
        );
        let p2 = egui::pos2(hl1.center().x, hl1.bottom() + 30.0);
        assert!(
            p2.y - map_top < 580.0,
            "第二次取样留在窄条内(实测 {})",
            p2.y - map_top
        );
        press_frame(&ctx, &mut editor, 1.0, p2);
        assert_eq!(
            grab_slot(&ctx, id),
            Some(None),
            "第二次按下:新阴影外,无抓取(居中路径)"
        );
        release_frame(&ctx, &mut editor, 1.1, p2);
        assert!(grab_slot(&ctx, id).is_none(), "纯点击释放后清零");
        for step in 0..4 {
            frame(
                &ctx,
                &mut editor,
                1.2 + f64::from(step) * 0.1,
                true,
                Vec::new(),
            );
        }

        // 第三次:按(又移位了的)阴影内部 → 新偏移(取当帧 probe 为尺,
        // 与判定读的上一帧 metrics 同帧稳态,自洽)。
        let hl2 = probe_hl(&ctx);
        let inside = hl2.top() + hl2.height() * 0.5;
        let p3 = egui::pos2(hl2.center().x, inside);
        press_frame(&ctx, &mut editor, 1.7, p3);
        let expected = inside - hl2.top();
        match grab_slot(&ctx, id) {
            Some(Some(g)) => assert!(
                (g - expected).abs() < 0.01,
                "第三次按下:新阴影内,偏移按新框重算(期望 {expected},实测 {g})"
            ),
            other => panic!("第三次按下应记录抓取偏移,实测通道 {other:?}"),
        }
        release_frame(&ctx, &mut editor, 1.8, p3);
        assert!(grab_slot(&ctx, id).is_none(), "最终释放清零");
    }

    /// ⑤-3b 评审修复回归:窄条最右的滚动条避让区**命中也让**。修复前
    /// minimap 的 interact 区盖满整条 108px 且注册在 ScrollArea 之后,同层
    /// 命中 tie 恒胜,滚动条 handle 既 hover 不到也拖不动。修复后避让区内
    /// 的按下/拖动归滚动条:offset 前进,minimap 跳转意图一次都不写。
    #[test]
    fn scrollbar_reserve_stays_interactive_over_minimap() {
        let ctx = egui::Context::default();
        let text = (0..500)
            .map(|_| "普通的一行")
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = EditorBuffer::new(&text);
        let id = editor::tab_editor_id(1);

        frame(&ctx, &mut editor, 0.0, true, Vec::new());
        let (shapes, _) = frame(&ctx, &mut editor, 0.1, true, Vec::new());
        let map = viewport_rect(&shapes).expect("高亮框定位窄条");

        // 避让宽与生产同源(当帧样式的 bar_width + bar_outer_margin;测试
        // 未改样式,取 Style::default 同值)。取样点在避让区中点:既在
        // 滚动条 interact 区内,又远离 minimap 命中边界 5px,两侧都不贴边。
        let scroll = egui::Style::default().spacing.scroll;
        let reserve = scroll.bar_width + scroll.bar_outer_margin;
        assert!(
            (reserve - 10.0).abs() < 0.5,
            "默认 floating 样式的避让宽 = 10px(实测 {reserve})"
        );
        let grab = egui::pos2(map.right() - reserve * 0.5, 300.0);

        // 按下并拖到下方:滚动条 handle 抓住指针逐帧跟(offset 直接重
        // 映射,无动画),视口应被推下去
        frame(
            &ctx,
            &mut editor,
            0.2,
            true,
            vec![
                egui::Event::PointerMoved(grab),
                egui::Event::PointerButton {
                    pos: grab,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        frame(
            &ctx,
            &mut editor,
            0.3,
            true,
            vec![egui::Event::PointerMoved(egui::pos2(grab.x, 520.0))],
        );
        let offset: f32 = ctx
            .data(|d| d.get_temp::<ScrollMetrics>(metrics_id(id)))
            .map_or(0.0, |m| m.offset);
        assert!(
            offset > 100.0,
            "避让区内拖动由滚动条承接,视口前进(实测 offset {offset})"
        );
        assert!(
            ctx.data(|d| d.get_temp::<f32>(jump_id(id))).is_none(),
            "避让区内的按下/拖动不写 minimap 跳转意图(命中真让出去了)"
        );
    }

    /// #105① 悬停滚轮转发(纯函数):滚轮量按「文档内容高 / minimap 全
    /// 文档高」放大 —— 窄条上滚 1px = 文档滚 内容高/(总行数×比例尺)
    /// px;符号与 delta 同向;无行程/无滚轮量返回 None。端点钳制不在此
    /// 函数(end() 的 offset 边界统一钳)。
    #[test]
    fn wheel_delta_scales_by_document_over_minimap_height() {
        // 1000 行、内容高 10000、视口 600:minimap 全高 3000,放大系数
        // 10000/3000 = 10/3。
        let base = WheelInput {
            content_height: 10000.0,
            viewport_height: 600.0,
            total_lines: 1000,
            row_h: ROW_H,
            delta_y: 0.0,
        };
        // 向下滚一档(egui 口径 delta 为负):文档滚动量 = -120×10/3 = -400px
        let down = wheel_editor_delta(WheelInput {
            delta_y: -120.0,
            ..base
        })
        .expect("长文档有行程");
        assert!(
            (down - (-400.0)).abs() < 1e-4,
            "窄条滚 120px = 文档滚 400px(实测 {down})"
        );
        // 比例恒等式(任务书「滚轮量与滚动量比例断言」):文档滚动量 /
        // 滚轮量 == 内容高 / (总行数×ROW_H)。
        assert!(
            (down / -120.0 - 10000.0 / 3000.0).abs() < 1e-7,
            "步长比 == 内容高/minimap 全文档高"
        );
        // 向上滚一档:符号随 delta 翻转(对称)
        let up = wheel_editor_delta(WheelInput {
            delta_y: 120.0,
            ..base
        })
        .expect("长文档有行程");
        assert!((up - 400.0).abs() < 1e-4);
        // 无行程(内容不满一屏)→ None:编辑器滚不动,不转发
        assert_eq!(
            wheel_editor_delta(WheelInput {
                viewport_height: 10000.0,
                delta_y: -120.0,
                ..base
            }),
            None
        );
        // 无滚轮量 → None
        assert_eq!(wheel_editor_delta(base), None);
        // 总行数 0 按 1 行计(与 window/jump_ratio 的 .max(1) 同款),
        // 不 panic、不除零。
        assert!(wheel_editor_delta(WheelInput {
            total_lines: 0,
            delta_y: -120.0,
            ..base
        })
        .is_some());
    }

    /// #105① 端到端:悬停窄条滚轮 → 编辑器 offset 前进一个**放大**步长
    /// (同 delta 在正文区只滚 1:1);连续两档 = 2×步长(截获清零生效,
    /// ScrollArea 内建 1:1 消费不叠加);向上滚退回;高亮框随文档滚动
    /// 自然下移;滚轮全程不写跳转意图(点击/拖动通道不受影响)。
    #[test]
    fn hover_wheel_over_minimap_scrolls_editor_scaled() {
        let ctx = egui::Context::default();
        let text = (0..500)
            .map(|_| "普通的一行")
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = EditorBuffer::new(&text);
        let id = editor::tab_editor_id(1);

        frame(&ctx, &mut editor, 0.0, true, Vec::new());
        let (shapes, _) = frame(&ctx, &mut editor, 0.1, true, Vec::new());
        let map = viewport_rect(&shapes).expect("高亮框定位窄条");
        let strip = egui::pos2(map.center().x, 300.0);
        let read = |ctx: &egui::Context| {
            ctx.data(|d| d.get_temp::<ScrollMetrics>(metrics_id(id)))
                .unwrap_or_default()
        };
        let wheel = |pos: egui::Pos2, delta: f32| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, delta),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        // 落地链:截获帧(end 记 target)→ 下一帧 begin 应用 → 帧末
        // metrics 可读。egui 的滚轮是平滑量(smooth_scroll_delta):事件
        // 帧只交 90%,尾量随后几帧按 10 倍衰减交齐(内建 1:1 同样如此,
        // 实测 108→118.8→120)——转发消费同一平滑源,落定节奏一致,每档
        // 后补 4 帧空转再取证。
        let settle =
            |ctx: &egui::Context, editor: &mut EditorBuffer, pos: egui::Pos2, now: &mut f64| {
                for _ in 0..4 {
                    *now += 0.1;
                    frame(
                        ctx,
                        editor,
                        *now,
                        true,
                        vec![egui::Event::PointerMoved(pos)],
                    );
                }
            };

        let before = read(&ctx);
        assert_eq!(before.offset, 0.0, "起滚前在文档顶");
        let probe_before = ctx
            .data(|d| d.get_temp::<MinimapProbe>(probe_id(id)))
            .expect("探针已写");
        let step = 120.0 * before.content_height / (500.0 * ROW_H);
        assert!(
            step > 240.0,
            "放大系数显著大于 1(实测一档 {step}px,内容高 {})",
            before.content_height
        );

        // 向下滚一档:offset 前进一个放大步长
        frame(&ctx, &mut editor, 0.2, true, wheel(strip, -120.0));
        let mut now = 0.2_f64;
        settle(&ctx, &mut editor, strip, &mut now);
        let after1 = read(&ctx).offset;
        assert!(
            (after1 - step).abs() < 8.0,
            "悬停滚轮滚出一个放大步长(期望 ≈{step},实测 {after1})"
        );
        // 高亮框随文档滚动自然下移(转发不改高亮框逻辑,它只读滚动真值)
        let probe_after = ctx
            .data(|d| d.get_temp::<MinimapProbe>(probe_id(id)))
            .expect("探针已写");
        let (top_before, top_after) = (
            probe_before.viewport.expect("高亮框").top(),
            probe_after.viewport.expect("高亮框").top(),
        );
        assert!(
            top_after > top_before,
            "高亮框随滚动下移({top_before} → {top_after})"
        );

        // 第二档:两档合计 ≈ 2×步长 —— 截获清零的否决线:若内建 1:1 未
        // 被抑制,每档会叠加成 step+120,8px 容差必挂。
        frame(&ctx, &mut editor, now + 0.1, true, wheel(strip, -120.0));
        now += 0.1;
        settle(&ctx, &mut editor, strip, &mut now);
        let after2 = read(&ctx).offset;
        assert!(
            (after2 - 2.0 * step).abs() < 8.0,
            "连续两档 = 2×步长,内建 1:1 不叠加(期望 ≈{},实测 {after2})",
            2.0 * step
        );

        // 向上滚一档:退回一个步长(方向与 egui 口径一致)
        frame(&ctx, &mut editor, now + 0.1, true, wheel(strip, 120.0));
        now += 0.1;
        settle(&ctx, &mut editor, strip, &mut now);
        let after3 = read(&ctx).offset;
        assert!(
            (after3 - step).abs() < 8.0,
            "向上滚退回一个步长(期望 ≈{step},实测 {after3})"
        );

        // 滚轮转发不写跳转意图:点击/拖动通道原样
        assert!(
            ctx.data(|d| d.get_temp::<f32>(jump_id(id))).is_none(),
            "滚轮帧不写 minimap 跳转意图"
        );

        // 正文区滚轮保持 1:1 原速:窄条之外的编辑器滚动零变化(否决线)
        let text_pos = egui::pos2(200.0, 300.0);
        frame(&ctx, &mut editor, now + 0.1, true, wheel(text_pos, -120.0));
        now += 0.1;
        settle(&ctx, &mut editor, text_pos, &mut now);
        let after4 = read(&ctx).offset;
        assert!(
            (after4 - (after3 + 120.0)).abs() < 8.0,
            "正文区滚轮仍按 1:1 原速(期望 ≈{},实测 {after4})",
            after3 + 120.0
        );
    }

    /// ⑤-4 10000 行档连续滚动若干帧无 panic,行模型缓存命中(存储指针
    /// 纹丝不动 = 无逐帧全量重建)。
    #[test]
    fn ten_k_line_scrolling_keeps_cache_hot_without_panic() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new(&"行\n".repeat(10000)); // 10001 行
        let id = editor::tab_editor_id(1);
        let p = LineParams::factory();

        frame(&ctx, &mut editor, 0.0, true, Vec::new());
        let model_ptr = with_lines(&ctx, id, &editor, p, |lines| lines.as_ptr());

        for i in 0..10 {
            let (shapes, probe) = frame(
                &ctx,
                &mut editor,
                0.1 + f64::from(i) * 0.1,
                true,
                vec![
                    egui::Event::PointerMoved(egui::pos2(200.0, 300.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -120.0),
                        phase: egui::TouchPhase::Move,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            let probe = probe.expect("滚动帧 minimap 照常绘制");
            assert!(probe.bars > 0, "第 {i} 帧行条在画");
            assert!(!bar_rects(&shapes).is_empty(), "第 {i} 帧行条形状在");
            let (ptr, len) =
                with_lines(&ctx, id, &editor, p, |lines| (lines.as_ptr(), lines.len()));
            assert_eq!(ptr, model_ptr, "滚动帧命中缓存,零重建(第 {i} 帧)");
            assert_eq!(len, 10001);
        }
    }

    /// ⑤-5 两主题渲染:明暗各三帧不 panic,行条色随主题翻转(暗色偏白、
    /// 亮色偏黑,且都非透明)。
    #[test]
    fn both_themes_paint_distinct_bar_colors() {
        let mut seen = Vec::new();
        for dark in [true, false] {
            let ctx = egui::Context::default();
            ctx.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            let mut editor = EditorBuffer::new("明暗各三帧\n不 panic\n");
            for step in 0..3 {
                let (shapes, probe) =
                    frame(&ctx, &mut editor, f64::from(step) * 0.1, true, Vec::new());
                let bars = bar_rects(&shapes);
                assert!(
                    bars.iter().all(|(_, c)| c.a() > 0),
                    "{} 第 {step} 帧行条非透明",
                    if dark { "暗色" } else { "亮色" }
                );
                if step == 2 {
                    assert!(probe.expect("开启帧写探针").bars >= 2);
                    seen.push((dark, bars[0].1));
                }
            }
        }
        let (dark_color, light_color) = (seen[0].1, seen[1].1);
        assert_ne!(dark_color, light_color, "两主题行条色不同");
        assert!(
            dark_color.r() > light_color.r(),
            "暗色主题行条偏白、亮色偏黑(实测 {dark_color:?} vs {light_color:?})"
        );
    }

    /// 范围:仅源码模式 —— Live 模式整帧不进 minimap 渲染路径(无探针、
    /// 无行条形状)。
    #[test]
    fn live_mode_renders_no_minimap() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("# 标题\n\n正文一段。\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut selection = None;
        let mut pending = None;
        let mut live = LiveState::default();
        let editor_id = editor::tab_editor_id(1);
        let output = ctx.run_ui(
            egui::RawInput {
                time: Some(0.0),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                // 开关开着也只该在源码模式生效
                editor::ui(
                    ui,
                    &mut editor,
                    &mut preview,
                    &crate::ai::AiState::default(),
                    editor::CursorChannel {
                        cursor: &mut cursor,
                        selection: &mut selection,
                        pending: &mut pending,
                    },
                    &mut live,
                    RenderMode::Live,
                    editor_id,
                    true,
                    false,
                    false,
                    None,
                    false,
                    &crate::keymap::Keymap::builtin(),
                    &mut Vec::new(),
                    &[],
                    &mut None,
                );
            },
        );
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        assert!(
            ctx.data(|d| d.get_temp::<MinimapProbe>(probe_id(editor_id)))
                .is_none(),
            "Live 帧不写 minimap 探针"
        );
        assert!(bar_rects(&shapes).is_empty(), "Live 帧零行条形状");
        assert!(viewport_rect(&shapes).is_none(), "Live 帧零高亮框");
    }

    /// 范围:切 tab 后开关照常渲染(全局开关)+ 各 tab 行模型分槽
    /// (第二个 tab 首帧用自己的槽重建,不影响第一个 tab 的缓存)。
    #[test]
    fn tab_switch_keeps_toggle_and_slot_isolation() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new(&"行\n".repeat(300));
        frame(&ctx, &mut editor, 0.0, true, Vec::new()); // tab1 建立
        let id1 = editor::tab_editor_id(1);
        let ptr1 = with_lines(&ctx, id1, &editor, LineParams::factory(), |l| l.as_ptr());

        // 第二个标签同开关渲染(生产里 tab2 用 tab_editor_id(2);本测试
        // 直接验证分槽:tab2 的槽独立建立)
        let id2 = editor::tab_editor_id(2);
        with_lines(&ctx, id2, &editor, LineParams::factory(), |lines| {
            assert_eq!(lines.len(), 301, "tab2 槽独立成表");
        });
        // tab1 缓存不受影响
        assert_eq!(
            with_lines(&ctx, id1, &editor, LineParams::factory(), |l| l.as_ptr()),
            ptr1,
            "tab1 缓存纹丝不动"
        );
        // 开关再渲一帧(tab 往返语义):探针仍在
        let (_, probe) = frame(&ctx, &mut editor, 0.1, true, Vec::new());
        assert!(probe.is_some_and(|p| p.bars > 0), "切回照常渲染");
    }
}
