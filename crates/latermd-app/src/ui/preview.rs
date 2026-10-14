//! 预览面板:vendored `MarkdownLabel` 渲染快照 + AI 指令卡。
//!
//! 两类 AI 入口共用 [`AiLinkHandler`]:`ai://` 链接的样式区分与点击拦截,
//! 以及 AI 指令块(info string 为 ai 的围栏)的卡片渲染(vendored 代码块级 block widget 扩展点,
//! vendor/README.md 差异表 #7:命中 info string 的围栏成为独立 segment,
//! 由本模块画卡)。卡片「执行」产出 [`Message::AiLinkClicked`],与链接点击
//! 同一条归约(流式续写、防重入都在归约侧)。

use crate::ai::AiState;
use crate::ai_link::{self, SCHEME};
use crate::state::{Message, PreviewState};
use eframe::egui;
use egui_markdown::link::{LinkHandler, LinkStyle};
use egui_markdown::MarkdownLabel;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::Path;
use std::rc::Rc;

/// ai:// 链接的样式色(紫罗兰,与默认超链接色区分),按明暗主题取两档。
/// `pub(crate)`:选区 AI 浮标(#61)同源取色 —— AI 专属色只有一个真源。
pub(crate) fn ai_link_color(dark_mode: bool) -> egui::Color32 {
    if dark_mode {
        egui::Color32::from_rgb(0xC9, 0x9B, 0xF5)
    } else {
        egui::Color32::from_rgb(0x8B, 0x2F, 0xC9)
    }
}

/// `[[wikilink]]` 的链接色(青绿,与 ai:// 的紫罗兰区分),按明暗主题取两档。
pub(crate) fn wiki_link_color(dark_mode: bool) -> egui::Color32 {
    if dark_mode {
        egui::Color32::from_rgb(0x6C, 0xD4, 0xC0)
    } else {
        egui::Color32::from_rgb(0x0F, 0x7A, 0x66)
    }
}

/// `==高亮==` 底色的混色锚(#65 M2):荧光笔语义,色相固定黄,不随主题;
/// 明度端取当前主题的 `extreme_bg_color`(文本背景最远端)推导 —— 一条
/// 推导式出明暗两档,不硬编码双份十六进制(#122 同族:对比度下限由
/// `highlight_bg_meets_contrast_floor` 测试钉住 ≥ 4.5:1)。从 `visuals`
/// 推导还顺带吃进皮肤/主题覆盖:用户换了 extreme_bg,高亮跟着换。
const HIGHLIGHT_TINT: egui::Color32 = egui::Color32::from_rgb(255, 230, 0);
/// 黄的混入比例:出厂明主题(白底)出 ≈#fff8b3 淡黄(经典 mark 观感),
/// 出厂暗主题(灰阶 10 底)出 ≈#544c07 暗橄榄。HTML 导出的内嵌 CSS
/// (latermd-export)写的是同一推导式的产物,两边同源。
const HIGHLIGHT_TINT_RATIO: f32 = 0.30;
/// 高亮底色四周外扩(像素),与行内代码观感一致
/// (`InlineCodeStyle::default().expand_bg`)。
const HIGHLIGHT_BG_EXPAND: f32 = 3.0;
/// 行高下限的绝对补偿,复制 vendored `layout.rs` 的
/// `LINE_HEIGHT_FLOOR_SLACK_PX`(私有常量,epaint 整像素行高吸附的最坏
/// 下探 + 1/32 pt 量化)。
const LINE_HEIGHT_FLOOR_SLACK_PX: f32 = 0.75;

/// `==高亮==` 底色:30% 荧光黄 × 70% 主题背景端,逐通道按 u8 四舍五入。
/// `pub(crate)`:Live 富渲染块与测试同源取色 —— 高亮底色只有一个真源。
pub(crate) fn highlight_bg_color(visuals: &egui::Visuals) -> egui::Color32 {
    let mix = |tint: u8, base: u8| -> u8 {
        (tint as f32 * HIGHLIGHT_TINT_RATIO + base as f32 * (1.0 - HIGHLIGHT_TINT_RATIO)).round()
            as u8
    };
    egui::Color32::from_rgb(
        mix(HIGHLIGHT_TINT.r(), visuals.extreme_bg_color.r()),
        mix(HIGHLIGHT_TINT.g(), visuals.extreme_bg_color.g()),
        mix(HIGHLIGHT_TINT.b(), visuals.extreme_bg_color.b()),
    )
}

/// `hl://` 链接的排版段(#65 M2):正文色文字 + 推导底色,行高/底对齐与
/// vendored 给普通链接段用的 `base_format` 同式。行高公式复制 vendored
/// `line_height_for`(私有,不能调),由
/// `highlight_section_matches_plain_text_metrics` 钉住:同一 Ui 里正文
/// section 的行高与本段相等,vendored 公式变化当场红。
pub(crate) fn append_highlight_section(
    ui: &egui::Ui,
    text: &str,
    job: &mut egui::text::LayoutJob,
    font: &egui::FontId,
) {
    let style = egui_markdown::global_style(ui.ctx());
    let line_height = (font.size * style.line_height_ratio)
        .max(font.size * style.min_line_height_em + LINE_HEIGHT_FLOOR_SLACK_PX);
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color: ui.visuals().text_color(),
            background: highlight_bg_color(ui.visuals()),
            expand_bg: HIGHLIGHT_BG_EXPAND,
            valign: egui::Align::BOTTOM,
            line_height: Some(line_height),
            ..egui::TextFormat::default()
        },
    );
}

/// 指令卡「已完成」状态色(绿),按明暗主题取两档,取色法同 [`ai_link_color`]。
fn done_color(dark_mode: bool) -> egui::Color32 {
    if dark_mode {
        egui::Color32::from_rgb(0x7D, 0xCE, 0x8A)
    } else {
        egui::Color32::from_rgb(0x1E, 0x7E, 0x34)
    }
}

/// AI 指令块的 info string 判定:首词为 `ai` 即命中。vendor parser 把
/// Fenced 围栏的完整 info string 原样存进 `Token::CodeBlock.language`
/// (parser.rs `Tag::CodeBlock` 分支),空 info 与缩进代码块是 `None`;
/// `aifoo` 不命中。与 `ai://` scheme 一致大小写敏感:认不出就不当指令卡,
/// 留给普通代码块渲染。
fn is_instruction_info(language: Option<&str>) -> bool {
    language.is_some_and(|info| info.split_whitespace().next() == Some("ai"))
}

/// 卡片内单条指令展示的字符上限:超长指令(整段文章粘进块里等)不再为它
/// 撑高预览,超出部分以 … 收尾。执行用的是完整原文,截断只影响展示。
const INSTRUCTION_DISPLAY_CHARS: usize = 240;

/// 指令文本的展示截断(字符计数,CJK 同算一个字符)。
fn truncate_instruction(instruction: &str) -> String {
    if instruction.chars().count() <= INSTRUCTION_DISPLAY_CHARS {
        return instruction.to_owned();
    }
    let mut cut: String = instruction
        .chars()
        .take(INSTRUCTION_DISPLAY_CHARS)
        .collect();
    cut.push('…');
    cut
}

/// 把渲染文本里的**相对图片地址**改写成 `file://` 绝对 URI(docs/image-plan.md
/// §4.1,B 段唯一硬骨头)。
///
/// 文档里存相对路径是对的(`./foo.assets/x.png` 随目录走,可移植),但
/// vendored 层把 url 原样喂 `egui::Image::new`,egui 没有「文档目录」概念,
/// 相对地址一律加载失败。改写发生在**喂给预览之前的字符串层**,源码与
/// `PreviewState` 一字不动;vendored 一行不动(等上游 `Token::Image` 加
/// `base_dir`,vendor/README.md 已登记待上游化)。
///
/// `base_dir` 为 `None`(文档未落盘)或空串(裸相对文件名,`Path::parent`
/// 的产物)时原样借回:没有锚点可拼,交给加载器按失败处理,与改写前一致。
pub(crate) fn resolve_relative_images<'a>(
    text: &'a str,
    base_dir: Option<&'a Path>,
) -> Cow<'a, str> {
    let rewrites = image_rewrites(text, base_dir);
    if rewrites.is_empty() {
        return Cow::Borrowed(text);
    }
    // 拼串交给 `OffsetMap::apply`:改写清单只有一份,「渲染串长什么样」与
    // 「偏移怎么换算」(map_source_offset 的第二层)不漂移。
    Cow::Owned(latermd_md::OffsetMap::from_rewrites(rewrites).apply(text))
}

/// 相对图片地址的改写清单(改写逻辑见 [`resolve_relative_images`]):每处
/// 是一个 `latermd_md::Rewrite`(与 wikilink 展开层**同一结构**),字符串
/// 改写与偏移映射(`map_source_offset`)共用同一份,两处不漂移。
fn image_rewrites(text: &str, base_dir: Option<&Path>) -> Vec<latermd_md::Rewrite> {
    let Some(base_dir) = base_dir.filter(|dir| !dir.as_os_str().is_empty()) else {
        return Vec::new();
    };
    inline_image_dests(text)
        .into_iter()
        .filter(|(_, dest, _)| is_relative(dest))
        .map(|(span, dest, wrapped)| {
            let mut uri = file_uri(base_dir, &dest);
            // 原 `<…>` 形式保持包裹;拼出来的 URI 含空格时裸目标语法会被
            // 截断,也要包。目标本身就在 `(` 与 `)` 之间,替换串带尖括号
            // 仍是合法 Markdown。
            if wrapped || uri.chars().any(char::is_whitespace) {
                uri = format!("<{uri}>");
            }
            latermd_md::Rewrite {
                span,
                replacement: uri,
            }
        })
        .collect()
}

/// 源码字节偏移(大纲 `OutlineItem.span` 的口径)→ 喂给预览 label 的文本
/// 偏移。五层改写**各一张映射表,按改写顺序串行穿过**(可组合口径见
/// `latermd_md::OffsetMap`):wikilink 展开(源文本 → 展开后,表由
/// `PreviewState::offset_map` 持有)、高亮 `==` 链接改写(展开后 → 高亮层
/// 输出,`PreviewState::highlight_map`,#65)、emoji 链接改写(高亮层输出
/// → emoji 层输出,`PreviewState::emoji_map`,#48 B1)、任务 checkbox 链接
/// 改写(emoji 层输出 → `rendered`,`PreviewState::task_map`,#63)与相对
/// 图片 `file://` 改写(`rendered` → 最终渲染文本,点击是低频事件,消费点
/// 现算)。
fn map_source_offset(
    offset_map: &latermd_md::OffsetMap,
    highlight_map: &latermd_md::OffsetMap,
    emoji_map: &latermd_md::OffsetMap,
    task_map: &latermd_md::OffsetMap,
    rendered: &str,
    base_dir: Option<&Path>,
    offset: usize,
) -> usize {
    let after_wikilinks = offset_map.source_to_rendered(offset);
    let after_highlight = highlight_map.source_to_rendered(after_wikilinks);
    let after_emoji = emoji_map.source_to_rendered(after_highlight);
    let after_task = task_map.source_to_rendered(after_emoji);
    let image_map = latermd_md::OffsetMap::from_rewrites(image_rewrites(rendered, base_dir));
    image_map.source_to_rendered(after_task)
}

/// 扫出全部**内联图片** `![alt](dest)` / `![alt](<dest> "title")` 的目标:
/// `(字节区间, 目标文本, 是否 <…> 包裹)`。区间含 `<>` 包裹(替换时原样
/// 保持包裹形式),不含两侧圆括号。
///
/// 与 `latermd_md::wikilinks` 同款扫描纪律:围栏代码块内不认(代码里的
/// `![](x)` 是字面文本);引用式图片 `![alt][ref]` 目标不在行内,不认;
/// **链接** `[t](x)` 不是图片,不认(链接点击走浏览器,拼 file:// 反而坏)。
/// alt 内嵌套 `![![a](u)](v)` 会把内层也扫出来 —— 内层本来就是独立图片,
/// 正该改写。
fn inline_image_dests(text: &str) -> Vec<(Range<usize>, String, bool)> {
    let mut dests = Vec::new();
    let mut in_code = false;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let rest = &text[index..];
        let line_start = index == 0 || bytes[index - 1] == b'\n';
        if line_start {
            let trimmed = rest.trim_start_matches([' ', '\t']);
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_code = !in_code;
            }
        }
        if !in_code && rest.starts_with("![") {
            // alt 到下一个 `]`(CommonMark 裸 alt 不含 `]`);其后紧跟 `(` 才是
            // 内联图片,否则是引用式或残缺写法,跳过这两个字符继续。
            if let Some(alt_len) = rest[2..].find(']') {
                let paren = 2 + alt_len + 1;
                if rest[paren..].starts_with('(') {
                    if let Some((span, dest, wrapped)) = dest_span(rest, paren + 1) {
                        dests.push((index + span.start..index + span.end, dest, wrapped));
                        index += span.end;
                        continue;
                    }
                }
            }
            index += 2;
            continue;
        }
        let step = rest.chars().next().map_or(1, char::len_utf8);
        index += step;
    }
    dests
}

/// `(` 之后的目标段:跳过前导空白;`<…>` 形式取尖括号内(区间**含**尖括号),
/// 裸形式到首个空白或 `)`。空目标(`![]()`)返回 `None`。
fn dest_span(text: &str, after_paren: usize) -> Option<(Range<usize>, String, bool)> {
    let bytes = text.as_bytes();
    let mut i = after_paren;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    match bytes.get(i) {
        Some(b'<') => {
            let close = i + 1 + text[i + 1..].find('>')?;
            Some((i..close + 1, text[i + 1..close].to_owned(), true))
        }
        Some(_) => {
            let stop = text[i..]
                .find(|c: char| c.is_whitespace() || c == ')')
                .map_or(text.len(), |offset| i + offset);
            (stop > i).then(|| (i..stop, text[i..stop].to_owned(), false))
        }
        None => None,
    }
}

/// 目标是否**相对**:无 scheme(含 `wiki://`、`data:`、`ai://`)、非 Windows
/// 盘符、非 `/` 绝对路径、非纯锚点。首段含 `:` 即视为有 scheme ——
/// `https:`、`wiki:`、`C:` 一网打尽。
fn is_relative(dest: &str) -> bool {
    !dest.is_empty()
        && !dest.starts_with('#')
        && !dest.starts_with('/')
        && !dest.split('/').next().unwrap_or(dest).contains(':')
}

/// 相对目标 → `file://` URI:按文档目录拼绝对路径(词法拼接,不做
/// canonicalize —— 那是磁盘 IO 且会解析符号链接,渲染定位不需要)。
/// 路径分隔符统一 `/`,Windows 盘符路径补前导 `/`(`file:///C:/…`,
/// egui FileLoader 的解析约定)。
fn file_uri(base_dir: &Path, dest: &str) -> String {
    let cleaned = dest.trim_start_matches("./");
    let path = base_dir.join(cleaned);
    let unified = path.to_string_lossy().replace('\\', "/");
    if unified.starts_with('/') {
        format!("file://{unified}")
    } else {
        format!("file:///{unified}")
    }
}

/// 指令卡三态。判定与 `AiState::last_prompt` 绑定(见 [`AiLinkHandler::card_status`]),
/// 「进行中」样式从简:换色文字,不加 spinner。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AiCardStatus {
    /// 从未(或已换文档/失败复位)发起。
    Idle,
    /// 本卡发起的流在途。
    Running,
    /// 本卡发起的流已成功收尾。
    Done,
}

/// vendored [`LinkHandler`] 的 ai:// 扩展点:链接样式区分 + 点击拦截 +
/// AI 指令卡渲染。
///
/// `click` 只有 `&self`,产出的消息暂存 [`RefCell`],帧末由 [`ui`] 并入
/// outbox(下一帧归约执行)。返回 true 拦下 vendored 层的默认
/// `open_url`;非 ai:// 前缀返回 false,链接照常走系统浏览器。协议语义见
/// `ai_link` 模块文档。
/// 链接是否「可打开」(右键菜单挂点判定,context-menu-plan §3 M3):
/// 排除应用内协议(`ai://` `wiki://` `emoji://` `task://` `hl://`)—— 它们
/// 的 click 各有归属(AI 流/开文档/字形/checkbox/底色),菜单挂上去只会
/// 误导;其余(`http(s)://`、外部绝对地址、本地文档路径、`file://`、
/// `#锚点`)可开 —— 其中本地路径与纯锚点的「打开」在应用内完成
/// ([`crate::doclink`]),不走系统浏览器。
pub(crate) fn is_openable_href(href: &str) -> bool {
    crate::doclink::classify(href) != crate::doclink::HrefKind::AppScheme
}

struct AiLinkHandler {
    clicked: Rc<RefCell<Vec<Message>>>,
    color: egui::Color32,
    /// 当前明暗(wikilink 取色用,与 ai:// 走两套色)。
    dark_mode: bool,
    /// 正文文本色(`link_style` 对 `emoji://` 的回落色,取构造时真实
    /// visuals,不用 `Visuals::dark()/light()` 推 —— 皮肤可自定义文本色)。
    text_color: egui::Color32,
    /// AI 是否在流(卡片「进行中」判据,取自 [`AiState::is_streaming`])。
    streaming: bool,
    /// 最近一次真实发起的 prompt(卡片状态匹配键,取自 [`AiState::last_prompt`])。
    last_prompt: Option<String>,
    /// 本帧已渲染的指令卡数:卡片序号 = 文档序,是 widget id 的稳定成分
    /// (AGENTS.md §6.7:绝不含内容长度 —— 编辑指令文本不改序号,id 不变)。
    card_count: Cell<usize>,
    /// 本帧已渲染的 mermaid 块数(#51 M3):块序号同为文档序稳定 id 成分,
    /// 与 `card_count` 分开计数 —— 两种块可交错出现,各自序号互不牵连。
    mermaid_count: Cell<usize>,
    /// 本帧各卡片的 widget id(渲染序);测试借它断言 id 稳定性。
    card_ids: RefCell<Vec<egui::Id>>,
    /// 本帧画过的 `emoji://` inline widget 区块(屏幕坐标,含纹理缺失只留
    /// 占位的):手型抑制的命中判定 + 无头探针(见 [`emoji_probe_id`])。
    emoji_rects: RefCell<Vec<egui::Rect>>,
}

impl AiLinkHandler {
    fn new(color: egui::Color32, dark_mode: bool, text_color: egui::Color32, ai: &AiState) -> Self {
        Self {
            clicked: Rc::new(RefCell::new(Vec::new())),
            color,
            dark_mode,
            text_color,
            streaming: ai.is_streaming(),
            last_prompt: ai.last_prompt.clone(),
            card_count: Cell::new(0),
            mermaid_count: Cell::new(0),
            card_ids: RefCell::new(Vec::new()),
            emoji_rects: RefCell::new(Vec::new()),
        }
    }

    /// 帧末收口:把点击消息并入 outbox。
    fn drain_into(&self, outbox: &mut Vec<Message>) {
        outbox.append(&mut self.clicked.borrow_mut());
    }

    /// 指令卡状态:指令文本与最近一次发起的 prompt 相等才认领 —— 防重入
    /// 保证同时至多一个流,菜单入口(`AiStart`)的 prompt 是文档尾部拼装,
    /// 不会与任何指令文本相等,其它卡片不受牵连。同一文本的多张卡同状态,
    /// 是接受的简化(decisions-pending #13)。
    fn card_status(&self, instruction: &str) -> AiCardStatus {
        if self.last_prompt.as_deref() == Some(instruction) {
            if self.streaming {
                AiCardStatus::Running
            } else {
                AiCardStatus::Done
            }
        } else {
            AiCardStatus::Idle
        }
    }

    /// 卡片「执行」→ [`Message::AiLinkClicked`]:与 ai:// 链接同一消息、
    /// 同一归约(Ok = 发起流式续写;空指令的按钮是禁用的,到不了这里)。
    fn request_execute(&self, instruction: &str) {
        self.clicked.borrow_mut().push(Message::AiLinkClicked {
            prompt: Ok(instruction.to_owned()),
        });
    }
}

impl LinkHandler for AiLinkHandler {
    /// ai:// 链接换色;`underline: true` 只是声明意图 —— vendored 层当前
    /// 未消费该字段(hover 下划线对全部链接无条件绘制),见 decisions-pending #11。
    ///
    /// `emoji://`(#48 B2)与 `task://`(#63)是 inline widget:文字本就是
    /// 透明占位,这里返回正文色 + 无下划线只是把「不吃超链接样式」的意图
    /// 钉进协议 —— vendored 对 inline widget 的 hover 本就不画下划线
    /// (label.rs `handle_hover`),该返回同时兜住「inline_widget_size 未来
    /// 返回 None」的退化路径。
    fn link_style(&self, href: &str) -> Option<LinkStyle> {
        // emoji:// 不吃默认超链接色/下划线:它不是可点的链接,是彩字形;
        // task:// 同理:它不是链接,是 checkbox(自绘矢量接管视觉)。
        if href.starts_with(latermd_md::EMOJI_SCHEME) || href.starts_with(latermd_md::TASK_SCHEME) {
            return Some(LinkStyle {
                color: Some(self.text_color),
                underline: false,
            });
        }
        // [[wikilink]] 用青绿,与 ai:// 的紫罗兰区分:两种链接的点击后果不同
        // (一个开文档、一个发起 AI 流),颜色不该撞
        if href.starts_with(latermd_md::WIKI_SCHEME) {
            return Some(LinkStyle {
                color: Some(wiki_link_color(self.dark_mode)),
                underline: true,
            });
        }
        // `hl://`(#65 M2):高亮不是可点链接,是底色标记 —— 意图声明为
        // 正文色 + 无下划线(与 emoji:// 同款协议)。vendored 对悬停中的
        // 普通链接无条件画下划线(#11 已知限),这里能定的是下划线颜色取
        // 正文色,悬停时低调呈现。
        if href.starts_with(latermd_md::HIGHLIGHT_SCHEME) {
            return Some(LinkStyle {
                color: Some(self.text_color),
                underline: false,
            });
        }
        href.starts_with(SCHEME).then_some(LinkStyle {
            color: Some(self.color),
            underline: true,
        })
    }

    fn click(&self, _text: &str, href: &str, _ui: &mut egui::Ui) -> bool {
        // emoji:// 没有点击语义:吞掉(返回 true),绝不交系统浏览器 ——
        // `emoji://😀` 不是合法 URL,交给浏览器只会弹错误提示
        if href.starts_with(latermd_md::EMOJI_SCHEME) {
            return true;
        }
        // `hl://`(#65 M2)同理:高亮没有点击语义,吞掉不发消息、不开
        // 浏览器(`hl://` 也不是合法 URL)。
        if href.starts_with(latermd_md::HIGHLIGHT_SCHEME) {
            return true;
        }
        // task://(#63):吞掉(绝不交系统浏览器),但**不发消息** —— 列表项
        // 内 inline widget 的 vendored section/token 映射在 egui 0.36 下错位,
        // 这条 click 路径对任务项不可达(到达时也不双发);点击统一由
        // `ui` 帧末的探针命中判定发出。
        if href.starts_with(latermd_md::TASK_SCHEME) {
            return true;
        }
        if let Some(target) = href.strip_prefix(latermd_md::WIKI_SCHEME) {
            let target = target.trim();
            if !target.is_empty() {
                self.clicked.borrow_mut().push(Message::WikilinkClicked {
                    target: target.to_owned(),
                });
                return true;
            }
        }
        match ai_link::parse(href) {
            Some(prompt) => {
                self.clicked
                    .borrow_mut()
                    .push(Message::AiLinkClicked { prompt });
                true
            }
            None => {
                // 本地文档链接(相对/绝对路径、`file://`、纯锚点):应用内
                // 打开,不交系统浏览器 —— 相对路径交给浏览器只会失败,纯
                // 锚点(TOC 目录链接)的语义就在本文档内。解析与跳转在
                // 归约侧(`state::open_doc_link`,那里才有文档路径与库根)。
                if crate::doclink::classify(href) == crate::doclink::HrefKind::Local {
                    self.clicked.borrow_mut().push(Message::DocLinkClicked {
                        href: href.to_owned(),
                    });
                    return true;
                }
                false
            }
        }
    }

    /// `emoji://` 与 `task://` 的透明占位(#48 B2 / #63):用**链接文字
    /// 本体 + 周围同款字体**追加透明文本 —— 占位的推进宽度与「同一字符
    /// 以普通文本出现」逐像素一致(同一 shaping 同一回退链),改写前后的
    /// 文本流零漂移。行高不在这里设:vendored 会把 `inline_widget_size`
    /// 的高度强制盖到这些 section 上(append_link_to_job)。
    fn layout_link(
        &self,
        ui: &egui::Ui,
        text: &str,
        href: &str,
        job: &mut egui::text::LayoutJob,
        font: &egui::FontId,
        color: egui::Color32,
    ) -> bool {
        // `hl://`(#65 M2)走的是非 widget 分支(layout.rs `append_link_to_job`
        // 对普通链接以超链接色调用本方法,返回 true 即接管 section 样式,
        // #123 探明口径):追加「正文色文字 + 推导底色」的排版段,不走
        // inline widget —— 文字要可见、行高要随正文,不是占位件。
        if href.starts_with(latermd_md::HIGHLIGHT_SCHEME) {
            append_highlight_section(ui, text, job, font);
            return true;
        }
        if !href.starts_with(latermd_md::EMOJI_SCHEME) && !href.starts_with(latermd_md::TASK_SCHEME)
        {
            return false;
        }
        // 到这里的调用只来自 inline widget 分支,color 恒为 TRANSPARENT
        // (append_link_to_job);按参数透传,分支语义变化时随动。
        let format = egui::TextFormat {
            font_id: font.clone(),
            color,
            ..egui::TextFormat::default()
        };
        job.append(text, 0.0, format);
        true
    }

    /// `emoji://` / `task://` 判定(#48 B2 / #63):返回与正文字号匹配的
    /// 尺寸。宽度分量仅是声明(vendored 只消费 `.y` 作占位行高);高度取
    /// `font.size`,恒不超过正文自然行高(epaint 行高取行内 max,不缩行),
    /// 含 widget 的行与相邻行同高,文档布局不被改写扰动。
    fn inline_widget_size(&self, href: &str, font: &egui::FontId) -> Option<egui::Vec2> {
        (href.starts_with(latermd_md::EMOJI_SCHEME) || href.starts_with(latermd_md::TASK_SCHEME))
            .then(|| egui::vec2(font.size, font.size))
    }

    /// 在透明占位上画 Twemoji 纹理(#48 B2):查面板同源纹理缓存(#47 A2,
    /// [`crate::ui::emoji_panel::inline_texture`]),查到 → 白 tint 画正方形
    /// (原色,探针口径 emoji-color-feasibility §2.1);查不到 → 什么都不画,
    /// 透明占位原样保持(不画黑块不 panic)。方块边长取占位区宽高的较小者:
    /// 单枚 emoji 的宽 ≈ 字体自然推进(与字号成正比),多字形跨度(旗帜等)
    /// 会被行高封顶,不横向溢出到邻字。
    fn paint_inline_widget(&self, ui: &mut egui::Ui, _text: &str, href: &str, rect: egui::Rect) {
        // task://(#63):自绘 checkbox 替代字面 `☑`/`☐` 符号(vendored
        // 原生渲染是普通文本,不可点)。占位区即命中区,点击由帧末的
        // 探针判定发出(见 `task_click_hit`),这里只画 + 记录几何。
        if href.starts_with(latermd_md::TASK_SCHEME) {
            paint_task_checkbox(ui, rect, href, TaskProbeDomain::Preview);
            return;
        }
        let Some(glyph) = href.strip_prefix(latermd_md::EMOJI_SCHEME) else {
            return;
        };
        let side = rect.width().min(rect.height());
        if side <= 0.0 {
            return;
        }
        self.emoji_rects.borrow_mut().push(rect);
        // 视口剔除与面板单元同款(AGENTS.md §6.2:每帧工作量按可见区)
        let paint_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.center().y - side / 2.0),
            egui::vec2(side, side),
        );
        if !ui.is_rect_visible(paint_rect) {
            return;
        }
        if let Some(texture) = crate::ui::emoji_panel::inline_texture(ui, glyph) {
            ui.painter().image(
                texture.id(),
                paint_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    }

    /// 右键菜单(context-menu-plan §3 M3,vendor ①类暴露的 Response):
    /// 普通链接按 [`crate::doclink::classify`] 分流 —— **本地文档路径与
    /// 纯锚点**弹「打开文档」走应用内跳转(交系统浏览器只会失败/丢语义),
    /// 其余(`http(s)://` 等)弹「打开链接」交浏览器;**协议链接不弹** ——
    /// `ai://` 在 `click` 里有流语义、`emoji://`/`task://`/`hl://` 是字形/
    /// checkbox/底色标记不是链接,菜单挂在它们上只会制造「可点」的错觉
    /// (与 click 吞掉口径一致)。
    ///
    /// 挂点由 vendored galley 主路径驱动(secondary click → 指针下链接 →
    /// 弹层打开期间逐帧回调),这里的判定只管「要不要挂」:
    /// 协议一律返回;菜单打开期间闭包逐帧重入,消息只由点击产出。
    ///
    /// 注意 `Rc` 克隆:`clicked` 是 `Rc<RefCell<…>>`(不是裸 `RefCell`),
    /// 闭包捕获的是共享句柄 —— 裸 `RefCell` 的 `clone()` 会给出独立副本,
    /// 点击消息将被推进当帧即弃的副本(实测踩过)。
    fn link_response(&self, _ui: &egui::Ui, _text: &str, href: &str, response: &egui::Response) {
        if !is_openable_href(href) {
            return;
        }
        let clicked = Rc::clone(&self.clicked);
        let local = crate::doclink::classify(href) == crate::doclink::HrefKind::Local;
        response.context_menu(|ui| {
            if local {
                if ui.button("打开文档").clicked() {
                    clicked.borrow_mut().push(Message::DocLinkClicked {
                        href: href.to_owned(),
                    });
                }
            } else if ui.button("打开链接").clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(href.to_owned()));
            }
            if ui.button("复制链接地址").clicked() {
                clicked
                    .borrow_mut()
                    .push(Message::CopyText(href.to_owned()));
            }
        });
    }

    /// 图片右键:「在浏览器打开 / 复制图片地址」。`resolve_relative_images`
    /// 已把相对地址改写为 `file://` 绝对 URI,系统浏览器直接可用。
    fn image_response(&self, _ui: &egui::Ui, _alt: &str, url: &str, response: &egui::Response) {
        let clicked = Rc::clone(&self.clicked);
        response.context_menu(|ui| {
            if ui.button("在浏览器打开").clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url.to_owned()));
            }
            if ui.button("复制图片地址").clicked() {
                clicked.borrow_mut().push(Message::CopyText(url.to_owned()));
            }
        });
    }

    fn is_block_code_widget(&self, language: Option<&str>) -> bool {
        // mermaid(#51 M3)与 ai 指令卡共用 block_code_widget 扩展点;
        // 两个判定互斥(info string 首词不同),非 mermaid/ai 围栏不受
        // 影响(否决线:普通代码块照走 vendored 原路径)。
        is_instruction_info(language) || crate::ui::mermaid::is_mermaid_info(language)
    }

    fn block_code_widget(
        &self,
        ui: &mut egui::Ui,
        text: &str,
        language: Option<&str>,
    ) -> Option<egui::Response> {
        if crate::ui::mermaid::is_mermaid_info(language) {
            let index = self.mermaid_count.get();
            self.mermaid_count.set(index + 1);
            return Some(crate::ui::mermaid::block_widget(ui, index, text));
        }
        debug_assert!(
            is_instruction_info(language),
            "分段侧已按 info string 过滤,两侧条件不同步是 vendor 回归"
        );
        let index = self.card_count.get();
        self.card_count.set(index + 1);
        let instruction = text.trim();
        let status = self.card_status(instruction);
        let display = if instruction.is_empty() {
            "(空指令)".to_owned()
        } else {
            truncate_instruction(instruction)
        };
        let response = ui.push_id(("ai_instruction_block", index), |ui| {
            self.card_ids.borrow_mut().push(ui.id());
            egui::Frame::NONE
                .fill(ui.visuals().faint_bg_color)
                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                .corner_radius(ui.visuals().widgets.noninteractive.corner_radius)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.strong("AI 指令");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            status_label(ui, status, self.color);
                        });
                    });
                    ui.add_space(4.0);
                    ui.add(egui::Label::new(display).wrap());
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let response =
                            ui.add_enabled(!instruction.is_empty(), egui::Button::new("执行"));
                        if response.clicked() {
                            self.request_execute(instruction);
                        } else if instruction.is_empty() {
                            response.on_disabled_hover_text("指令块没有文本");
                        }
                    });
                })
                .response
        });
        Some(response.inner)
    }
}

/// 状态行文字与配色:未执行弱化、进行中用 AI 紫罗兰、完成用绿。
fn status_label(ui: &mut egui::Ui, status: AiCardStatus, ai_color: egui::Color32) {
    match status {
        AiCardStatus::Idle => {
            ui.weak("未执行");
        }
        AiCardStatus::Running => {
            ui.colored_label(ai_color, "进行中…");
        }
        AiCardStatus::Done => {
            ui.colored_label(done_color(ui.visuals().dark_mode), "已完成");
        }
    }
}

// —— 任务列表 checkbox(#63)——

/// checkbox 探针条目的坐标域(#63):载荷偏移在哪套坐标系里,消费侧据此
/// 换算回源码。两面板同帧写进同一份全局探针(几何共享),载荷坐标各说
/// 各话,必须带上出身才不会串台。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TaskProbeDomain {
    /// 右栏预览:emoji 层输出坐标,消费时逆穿 `offset_map` +
    /// `highlight_map` + `emoji_map`。
    Preview,
    /// 编辑器 Live 富渲染块:块内坐标,消费时加记录时的块首字节偏移。
    Live(usize),
}

/// 解析 `task://` 链接载荷:`Some((勾选态, emoji 层输出坐标偏移))`;
/// 形态不符(非本 scheme / 首字符非 u,c / 余非数字)返回 `None`。
/// 勾选态只在绘制时消费(画空框还是对勾);回写按偏移重新核验当前标记
/// 形态,不信任这个态(点击帧到归约帧之间文档可能已变)。
pub(crate) fn parse_task_href(href: &str) -> Option<(bool, usize)> {
    let payload = href.strip_prefix(latermd_md::TASK_SCHEME)?;
    let (checked, digits) = payload.split_at(1.min(payload.len()));
    let checked = match checked {
        "c" => true,
        "u" => false,
        _ => return None,
    };
    digits.parse::<usize>().ok().map(|offset| (checked, offset))
}

/// 在透明占位区上画 checkbox(#63):方框 + 已勾时叠 `Icon::Check` 对勾。
/// 边长取占位区高的 0.72(行高内舒适,不顶满),垂直居中、水平靠左
/// (与「行首标记」的视觉锚一致);颜色走正文色系(两主题各自成立),
/// 已勾的对勾用 tokens::OK 强调。悬停 = 指针落在占位区内:边框换 hover
/// 色 + 背景微亮(立即模式直接读指针位置,不注册 widget —— 点击命中
/// 读原始指针事件,见 [`task_click_hit`],13a 同层纪律:命中区即占位区)。
pub(crate) fn paint_task_checkbox(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    href: &str,
    domain: TaskProbeDomain,
) {
    let Some((checked, _)) = parse_task_href(href) else {
        return;
    };
    let side = rect.height() * 0.72;
    if side <= 0.0 {
        return;
    }
    record_task_checkbox(ui.ctx(), rect, href, domain);
    let box_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - side / 2.0),
        egui::vec2(side, side),
    );
    if !ui.is_rect_visible(box_rect) {
        return;
    }
    let hovered = ui
        .input(|input| input.pointer.latest_pos())
        .is_some_and(|pos| rect.contains(pos));
    let stroke = egui::Stroke::new(
        if hovered { 2.0 } else { 1.0 },
        if hovered {
            ui.visuals().widgets.hovered.fg_stroke.color
        } else {
            ui.visuals().text_color()
        },
    );
    if hovered {
        ui.painter().rect_filled(
            box_rect,
            crate::ui::tokens::RADIUS_SM,
            ui.visuals().widgets.hovered.bg_fill,
        );
    }
    ui.painter().rect_stroke(
        box_rect,
        crate::ui::tokens::RADIUS_SM,
        stroke,
        egui::StrokeKind::Inside,
    );
    if checked {
        crate::ui::icons::Icon::Check.draw(
            ui.painter(),
            box_rect.center(),
            side,
            crate::ui::tokens::OK,
        );
    }
}

/// checkbox 探针在 egui data 的键(#63,照 [`copy_button_rects_id`] 的
/// data 手法):本帧各 checkbox 的(占位区 rect, 链接载荷, 坐标域)。逐个
/// 追加、帧号核对。**点击不走 vendored 链接命中**——列表项的布局里
/// section_to_token 与 egui 0.36 galley 的空文本 section 会错位,vendored
/// `handle_hover`/`handle_click` 对列表项内的 inline widget 到不了
/// handler(实测两处:预览与 Live 富渲染块同样不可达,#63 live 测试
/// 断零消息证实);照 #38 复制按钮与 #61 浮标的同款纪律读原始指针:
/// [`task_click_hit`] 在渲染侧帧末判定,Live 列「点击进编辑」在
/// [`task_checkbox_rects`] 里排除 checkbox 命中。
fn task_checkbox_probe_id() -> egui::Id {
    egui::Id::new("latermd-task-checkbox-probe")
}

/// 追加一枚本帧 checkbox(帧号变了就重开清单,一帧一份)。预览与 Live
/// 两面板同帧都写进同一份全局清单(与 copy_button_rects 同语义);载荷
/// 坐标各说各话,出身记进坐标域,消费侧按域过滤(几何判定只看屏幕位置)。
fn record_task_checkbox(
    ctx: &egui::Context,
    rect: egui::Rect,
    href: &str,
    domain: TaskProbeDomain,
) {
    let probe_id = task_checkbox_probe_id();
    let frame = ctx.cumulative_pass_nr();
    let mut entries = ctx
        .data(|d| d.get_temp::<(u64, Vec<(egui::Rect, String, TaskProbeDomain)>)>(probe_id))
        .filter(|(seen, _)| *seen == frame)
        .map(|(_, entries)| entries)
        .unwrap_or_default();
    entries.push((rect, href.to_owned(), domain));
    ctx.data_mut(|d| d.insert_temp(probe_id, (frame, entries)));
}

/// 当前帧的 checkbox (rect, 载荷, 坐标域) 清单(帧号核对通过的才返回)。
/// 零 checkbox 帧返回空 —— 上一帧的旧 rect 不得再拦 Live 的「点击进编辑」。
fn task_checkbox_entries(ctx: &egui::Context) -> Vec<(egui::Rect, String, TaskProbeDomain)> {
    ctx.data(|d| {
        d.get_temp::<(u64, Vec<(egui::Rect, String, TaskProbeDomain)>)>(task_checkbox_probe_id())
            .filter(|(seen, _)| *seen == ctx.cumulative_pass_nr())
            .map(|(_, entries)| entries)
            .unwrap_or_default()
    })
}

/// 当前帧的 checkbox rect 清单(只留几何):Live 列「点击进编辑」的
/// 排除判定用(点击归 checkbox 切换,不连带进编辑)。几何排除不分面板。
pub(crate) fn task_checkbox_rects(ctx: &egui::Context) -> Vec<egui::Rect> {
    task_checkbox_entries(ctx)
        .into_iter()
        .map(|(rect, _, _)| rect)
        .collect()
}

/// 帧后读法(帧号已前进,核对必失配):测试按「最后写入者即本帧」拿
/// 最后写入一帧的 rect 清单。
#[cfg(test)]
pub(crate) fn task_checkbox_probe(ctx: &egui::Context) -> Vec<egui::Rect> {
    ctx.data(|d| {
        d.get_temp::<(u64, Vec<(egui::Rect, String, TaskProbeDomain)>)>(task_checkbox_probe_id())
            .map(|(_, entries)| entries.into_iter().map(|(rect, _, _)| rect).collect())
            .unwrap_or_default()
    })
}

/// checkbox 的原始指针点击命中(#63):本帧主键 click(egui 已按拖动阈值
/// 判定,块内拖选不算 click)且抬起点落在本帧某枚探针 rect 内 → 返回
/// 该枚的(链接载荷, 坐标域)。与 Live 列 `clicked_for_edit` 同款读法
/// (primary_clicked + interact_pos),不挂 egui widget、不依赖 vendored 的
/// section/token 映射。坐标域随条目带回:预览只认 [`TaskProbeDomain::
/// Preview`]、Live 只认 `Live(_)` 各自消费,同帧互不串台。
pub(crate) fn task_click_hit(ui: &egui::Ui) -> Option<(String, TaskProbeDomain)> {
    if !ui.input(|input| input.pointer.primary_clicked()) {
        return None;
    }
    let pos = ui.input(|input| input.pointer.interact_pos())?;
    task_checkbox_entries(ui.ctx())
        .into_iter()
        .find(|(rect, _, _)| rect.contains(pos))
        .map(|(_, href, domain)| (href, domain))
}

/// checkbox 点击的焦点暂存键(#63):egui 0.36 的让焦
/// (`SurrenderFocusOn::Presses`)发生在**按下帧**的 widget 绘制段,
/// 释放帧帧首已查无可焦者 —— 按下帧帧首的持焦者快照存这里,释放帧
/// 点击命中时归还(layout.rs `keep_find_focus` 同款:绘制段之后的
/// `request_focus` 不再被同一帧的让焦吃掉)。
fn task_focus_stash_id() -> egui::Id {
    egui::Id::new("latermd-task-focus-stash")
}

/// 按下帧调用(#63):帧首持焦者进暂存,供同次点击的释放帧归还。一帧
/// 只收首份快照(预览面板先画,持焦者尚未被让;Live 列后画时焦点可能
/// 已被按下帧的让焦清掉,不得覆盖),释放帧消费,拖选不放则自然过期。
pub(crate) fn stash_focus_on_press(ctx: &egui::Context) {
    if !ctx.input(|input| input.pointer.primary_pressed()) {
        return;
    }
    let frame = ctx.cumulative_pass_nr();
    let seen: Option<(u64, Option<egui::Id>)> = ctx.data(|d| d.get_temp(task_focus_stash_id()));
    if seen.is_some_and(|(seen_frame, _)| seen_frame == frame) {
        return;
    }
    let focused = ctx.memory(|mem| mem.focused());
    ctx.data_mut(|d| d.insert_temp(task_focus_stash_id(), (frame, focused)));
}

/// 点击命中帧调用(#63):把按下帧持焦者还回去(`request_focus` 的落地
/// 在下一帧;暂存为空或持焦者已消亡则不动 —— egui 对消亡 id 的
/// request_focus 是无害 no-op)。
pub(crate) fn restore_stashed_focus(ctx: &egui::Context) {
    let focused: Option<(u64, Option<egui::Id>)> = ctx.data(|d| d.get_temp(task_focus_stash_id()));
    if let Some((_, Some(id))) = focused {
        ctx.memory_mut(|mem| mem.request_focus(id));
    }
}

// —— 代码块复制头(docs/auto-plan.md #38)——

/// 「已复制」✓ 反馈的存活窗口:点击后停留 1.5s 自行消退,不需要用户再
/// 交互确认。
const COPIED_FEEDBACK: std::time::Duration = std::time::Duration::from_millis(1500);
/// 点击帧后再排一短帧:点击帧里按钮已按普通态画完,✓ 要等下一帧才上屏,
/// 不主动排程的话它得等到下一次用户输入。
const COPIED_FEEDBACK_DELAY: std::time::Duration = std::time::Duration::from_millis(16);

/// 反馈态在 egui data 的键:全局单份「最近复制块的指纹 + 到期时刻」。
/// 按块内容而非 widget id 认领 —— vendored 挂载点的子 Ui id 按帧内序号
/// 自动分配,文档编辑后块序平移会让 id 键控的反馈错位到别的块;内容指纹
/// 最多让同文本多块同显 ✓(与 ```ai 卡片按指令文本认领状态同款简化)。
fn copied_feedback_id() -> egui::Id {
    egui::Id::new("latermd-code-copy-feedback")
}

/// 探针在 egui data 的键:本帧各复制按钮的 rect(照 vendored
/// `section_anchors` 的 data 手法 —— 生产侧写入开销一次 Vec)。读法见
/// [`copy_button_probe`]/[`copy_button_rects`]。每帧首写按帧号重置。
fn copy_button_rects_id() -> egui::Id {
    egui::Id::new("latermd-code-copy-button-rects")
}

/// 原始探针:(最后写入帧号, 该帧按钮 rect 清单)。写入发生在渲染帧内,
/// 而测试在 `run_ui` 返回之后才读 —— 那时帧号已前进(`end_pass` 处 +1),
/// 帧号核对必然失配,故测试按「最后写入者即本帧」直接取清单。
pub(crate) fn copy_button_probe(ctx: &egui::Context) -> (u64, Vec<egui::Rect>) {
    ctx.data(|d| {
        d.get_temp::<(u64, Vec<egui::Rect>)>(copy_button_rects_id())
            .unwrap_or_default()
    })
}

/// 当前帧的按钮 rect 清单(帧号核对通过的才返回):供 Live 列「点击进
/// 编辑」在帧内排除按钮命中(live.rs `clicked_for_edit`)。零按钮帧返回
/// 空 —— 比如全部代码块都转入编辑态时,上一帧的旧 rect 不得再拦点击。
pub(crate) fn copy_button_rects(ctx: &egui::Context) -> Vec<egui::Rect> {
    let (frame, rects) = copy_button_probe(ctx);
    if frame == ctx.cumulative_pass_nr() {
        rects
    } else {
        Vec::new()
    }
}

fn code_fingerprint(code: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    code.hash(&mut hasher);
    hasher.finish()
}

/// 代码块头部一行(常驻):复制按钮 + 语言标签,挂在 vendored
/// `MarkdownLabel::code_block_buttons` 挂载点 —— 该挂载点上游自带(subtree
/// 引入即有),普通代码块右上角悬浮调用,回调直接收到 `(块源文本, 语言)`,
/// 无需从 `code_block_spans` 切偏移;```ai 指令卡走 `block_code_widget` 另
/// 一条路,不经过这里。
///
/// 点击 → `Context::copy_text` 整块源文本(vendored parser 已去块尾换行)
/// → 按钮进入 ✓ 反馈态。无语言/空块同样给按钮:复制空串是合法操作,
/// 交互一致比按内容藏按钮好猜。
pub(crate) fn code_copy_buttons(ui: &mut egui::Ui, code: &str, lang: &str) {
    let now = std::time::Instant::now();
    let until = ui
        .ctx()
        .data(|d| d.get_temp::<(u64, std::time::Instant)>(copied_feedback_id()))
        .filter(|(hash, until)| *hash == code_fingerprint(code) && *until > now)
        .map(|(_, until)| until);
    let copied = until.is_some();

    // 子 Ui 是 right_to_left(Center):先分配的贴右缘,按钮在语言标签右侧。
    let size = egui::vec2(
        crate::ui::tokens::ICON + 2.0 * crate::ui::tokens::SPACE_XS,
        20.0,
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(
                rect,
                crate::ui::tokens::RADIUS_SM,
                ui.visuals().widgets.hovered.bg_fill,
            );
        }
        let (icon, color) = if copied {
            (crate::ui::icons::Icon::Check, crate::ui::tokens::OK)
        } else {
            (crate::ui::icons::Icon::Copy, ui.visuals().weak_text_color())
        };
        icon.draw(painter, rect.center(), crate::ui::tokens::ICON, color);
    }

    // 探针:记录本帧按钮几何,帧号变了就重开清单(一帧一份)。
    let probe_id = copy_button_rects_id();
    let frame = ui.ctx().cumulative_pass_nr();
    let mut rects = ui
        .ctx()
        .data(|d| d.get_temp::<(u64, Vec<egui::Rect>)>(probe_id))
        .filter(|(seen, _)| *seen == frame)
        .map(|(_, rects)| rects)
        .unwrap_or_default();
    rects.push(rect);
    ui.ctx()
        .data_mut(|d| d.insert_temp(probe_id, (frame, rects)));

    // 语言标签:info string 首词(完整 info 可带 `title=` 等元数据,首词才是
    // 语言);无语言(裸围栏/缩进块,vendored 侧落到空串)不画标签。
    if let Some(word) = lang.split_whitespace().next() {
        ui.label(egui::RichText::new(word).small().weak())
            .on_hover_text("代码块语言");
    }

    if response.clicked() {
        ui.ctx().copy_text(code.to_owned());
        let deadline = now + COPIED_FEEDBACK;
        ui.ctx()
            .data_mut(|d| d.insert_temp(copied_feedback_id(), (code_fingerprint(code), deadline)));
        ui.ctx().request_repaint_after(COPIED_FEEDBACK_DELAY);
    }

    // hover 文案放最后(on_hover_text 消费 response)。
    if let Some(until) = until {
        // 到点排程一帧,让 ✓ 自行消退;静止窗口里 egui 不会再醒。
        ui.ctx()
            .request_repaint_after(until.saturating_duration_since(now));
        response.on_hover_text("已复制");
    } else {
        response.on_hover_text("复制代码");
    }
}

/// 预览 widget id 的 tab 维度成分(#39 M2)。vendored 层的解析/分段/高亮
/// 缓存全部挂在这个 id 命名空间下,每标签独立一份:切到别的标签期间本
/// 标签的缓存不被覆盖,切回即命中 —— 这是「切标签往返不重解析」的机制
/// (egui temp memory 无按帧回收,跨帧存活,实证见 tab_switch_perf 回归测试)。
/// id 只含稳定 tab id,绝不含内容 hash/长度(AGENTS.md §6.7)。
fn tab_preview_id(tab_id: u64) -> egui::Id {
    egui::Id::new("preview-md").with(tab_id)
}

/// 绘制预览面板。`tab_id` 是当前标签的稳定 id(缓存命名空间,见
/// [`tab_preview_id`]);`heal` 为 true 时渲染前对整篇文本补闭合 —— 仅在
/// AI 流式写入本标签时开:流式残缺帧(未闭合 fence/加粗)需要补闭合才
/// 语法合法(AGENTS.md §6.5),完整文档上 heal 是恒等变换(Cow::Borrowed),
/// 但扫描本身逐行全文(2 万行样本 ~2ms/帧),人工编辑的稳态帧不该付。
/// `base_dir` 是当前文档所在目录:相对图片地址以它为锚拼成 `file://`
/// 绝对 URI(见 [`resolve_relative_images`]);`None` = 文档未落盘,原样渲染。
pub fn ui(
    panel: &mut egui::Ui,
    preview: &mut PreviewState,
    ai: &AiState,
    tab_id: u64,
    heal: bool,
    base_dir: Option<&Path>,
    outbox: &mut Vec<Message>,
) {
    // 按下帧焦点快照(#63):checkbox 命中帧要把它还回去 —— MarkdownLabel
    // 的 `Sense::click_and_drag` 含可聚焦位,点击预览任何位置都会把编辑器
    // 的键盘焦点抢给 label;点 checkbox 的意图是切换勾选,不是转移焦点
    // (#61 浮标「先还焦再判」的同一纪律)。让焦发生在按下帧的绘制段,
    // 这里在帧首(未让)收快照,释放帧命中时归还。
    stash_focus_on_press(panel.ctx());
    let label_id = tab_preview_id(tab_id);
    let scrolled = egui::ScrollArea::vertical()
        .id_salt(label_id.with("scroll"))
        // 不收缩宽度,让 wrap 以面板宽为界
        .auto_shrink([false, false])
        .show(panel, |ui| {
            // widget id 必须稳定:只由 tab id 构成,绝不含内容长度/hash,
            // 否则每次编辑都清空 vendored 层临时缓存,增量高亮与分段缓存
            // 全部失效(AGENTS.md §6.7)。内容变化已在上游按修订号节流,
            // 这里每帧拿到的都是"仅在变化时重建"的同一字符串;相对图片的
            // 改写也是纯函数 —— 同样的输入永远产出同样的字符串,缓存照常
            // 命中。
            let handler = AiLinkHandler::new(
                ai_link_color(ui.visuals().dark_mode),
                ui.visuals().dark_mode,
                ui.visuals().text_color(),
                ai,
            );
            // 渲染的是**展开过 wikilink 的**文本:源码里的 [[X]] 在这里已是
            // [X](<wiki://X>) 链接,点击由下面的 handler 拦截;相对图片
            // 地址在这里再换成 file:// URI(两层都是"只改渲染,源码不动")
            let rendered = resolve_relative_images(&preview.rendered, base_dir);
            // 字体(#43 M2 + #23 F3):size 取用户字号偏好(投影槽的读侧,
            // 未投影的 context 回落出厂 15pt),族用预览专用族 —— 链头是行
            // metrics 对齐过 CJK 回退的 Inter 副本,中英数字混排基线齐;无
            // CJK 时回落 Proportional,行为与修复前一致。显式 FontId 经
            // vendored 布局自然传导:标题按 `heading.scales` 比例放大(base
            // font size × scales,layout.rs 标题分支),行高随各自字号重算,
            // 行距倍率来自 markdown style 的 `line_height_ratio`(用户滑杆,
            // ThemeSettings::apply 已在皮肤/overrides 之上覆盖)。font 参与
            // vendored 布局缓存 hash,族名与字号均随偏好稳定,不破坏 widget
            // id 稳定性(§6.7)。
            let font = egui::FontId::new(
                crate::theme::editor_font_size(ui.ctx()),
                crate::fonts::preview_body_family(ui.ctx()),
            );
            MarkdownLabel::new(label_id, rendered.as_ref())
                .font(font)
                .wrap()
                .scroll_code_blocks(true)
                .shrink_code_blocks(true)
                .code_block_min_width(Some(200.0))
                .overflow_wrap(egui_markdown::OverflowWrap::BreakAll)
                .heal(heal)
                // 视口外代码块高亮延迟(#59/#60 首切卡顿,vendor ①类配套):
                // 缓存 miss 帧里起始于视口下缘之外的 flush 段不做 syntect
                // 高亮(几何/文本零变化,仅颜色缺席),滚动到可见前的同帧
                // 补齐高亮——20k 行冷首切 876→173ms(探针口径)。等价关系
                // 由 vendored tests/deferred_highlight.rs 钉住。
                .defer_offscreen_highlight(true)
                .link_handler(&handler)
                // 代码块复制头(#38):挂载点上游自带,点击经回调出 app 侧
                // 执行复制(见 [`code_copy_buttons`])。源码/Live 两种模式下
                // 右栏都走本入口,label_id 只含 tab id,互切不清缓存、按钮仍在。
                .code_block_buttons(&code_copy_buttons)
                .show(ui);
            handler.drain_into(outbox);

            // emoji:// inline widget 的收尾(#48 B2):①把本帧的 widget 区块
            // 写进探针(每帧整帧覆盖,零 emoji 帧不留上一帧的旧区块 —— 与
            // copy_button_rects 同一语义);②手型抑制 —— vendored 对悬停中的
            // inline widget 无条件置 PointingHand(label.rs `handle_hover` /
            // layout.rs `render_link_in_ui`),`link_style` 管不到光标,只能在
            // label 渲染完之后把指针悬停 emoji 时的光标按回 Default(本面板
            // 内后写者胜;同帧后续面板仍按各自悬停自设,不受影响)。
            let emoji_rects = handler.emoji_rects.borrow().clone();
            let hovering_emoji = ui
                .input(|input| input.pointer.latest_pos())
                .is_some_and(|pos| emoji_rects.iter().any(|rect| rect.contains(pos)));
            if hovering_emoji {
                ui.output_mut(|out| out.cursor_icon = egui::CursorIcon::Default);
            }
            ui.ctx()
                .data_mut(|d| d.insert_temp(emoji_probe_id(tab_id), emoji_rects));

            // 任务 checkbox 的收尾(#63):①悬停手型 —— vendored 对列表项内
            // inline widget 的 hover 光标同样到不了(section 错位),这里按
            // 本帧探针几何自设 PointingHand(checkbox 是可点件,与 emoji 的
            // 手型抑制相反方向);②点击命中 —— 读原始指针事件,载荷偏移
            // 逆穿三层映射回源码坐标发切换消息(命中区即占位区,13a)。
            let task_entries = task_checkbox_entries(ui.ctx());
            if let Some(pos) = ui.input(|input| input.pointer.latest_pos()) {
                if task_entries.iter().any(|(rect, _, _)| rect.contains(pos)) {
                    ui.output_mut(|out| out.cursor_icon = egui::CursorIcon::PointingHand);
                }
            }
            if let Some((href, TaskProbeDomain::Preview)) = task_click_hit(ui) {
                // 只认预览域的条目:点击落在 Live 列的 checkbox 上时由
                // live.rs 的帧末判定消费,这里不得串台(#63 探针带坐标域)。
                if let Some((_, offset)) = parse_task_href(&href) {
                    let source = preview.offset_map.rendered_to_source(
                        preview
                            .highlight_map
                            .rendered_to_source(preview.emoji_map.rendered_to_source(offset)),
                    );
                    outbox.push(Message::TaskCheckboxToggled { byte: source });
                    // 焦点归还:按下帧被让走的键盘焦点交还原持有者。
                    restore_stashed_focus(ui.ctx());
                }
            }

            // 大纲跳转的预览侧(#42):源码偏移(大纲 span 口径)先穿过渲染
            // 改写链(map_source_offset,wikilink/高亮/emoji/任务/相对图片
            // 五层串行)映射到喂给 label 的文本
            // 偏移,再查 vendored 块表拿目标块 rect。两件事都必须发生在
            // ScrollArea 闭包内、label 渲染之后:scroll_to_rect 写的是本 pass
            // 的滚动目标,由 ScrollArea 收尾消费 —— 闭包外写会在下一帧开头被
            // 清空,永远不生效(用户反馈「就源码跳转了」的断点就在这);块表
            // 也只在渲染当帧有效,同帧先写后读。块缺失(空文档/未渲染)就
            // 不滚,下一帧有表了也不会再滚 —— 滚动目标消费即清空,一次语义。
            if let Some(target) = preview.scroll_target.take() {
                let offset = map_source_offset(
                    &preview.offset_map,
                    &preview.highlight_map,
                    &preview.emoji_map,
                    &preview.task_map,
                    &preview.rendered,
                    base_dir,
                    target,
                );
                if let Some(block) = egui_markdown::block_rect_at_offset(ui, label_id, offset) {
                    ui.scroll_to_rect_animation(
                        block.rect,
                        Some(egui::Align::Center),
                        egui::style::ScrollAnimation::none(),
                    );
                }
            }
        });

    // 滚动位置探针(照 copy_button_probe 的 data 手法):ScrollArea 的持久
    // state key 由其内部 id 链派生,外部无法稳定复现;测试用本探针断言
    // 「大纲点击真的滚动了预览、落点方位正确」。生产代码不读它。
    panel.ctx().data_mut(|d| {
        d.insert_temp(
            scroll_probe_id(tab_id),
            ScrollProbe {
                offset: scrolled.state.offset.y,
                viewport: scrolled.inner_rect,
                content_height: scrolled.content_size.y,
            },
        )
    });
}

/// 预览滚动位置探针的 data 键(每 tab 一份;写入见 [`ui`] 消费段)。
fn scroll_probe_id(tab_id: u64) -> egui::Id {
    egui::Id::new("latermd-preview-scroll-offset").with(tab_id)
}

/// 预览 emoji inline widget 区块探针的 data 键(#48 B2,每 tab 一份):
/// 本帧全部 `emoji://` 占位区块(含纹理缺失的),生产只写不读 —— 手型
/// 抑制走 handler 字段,无头测试读它断言「widget 在该段生效」。
fn emoji_probe_id(tab_id: u64) -> egui::Id {
    egui::Id::new("latermd-preview-emoji-rects").with(tab_id)
}

/// 探针载荷(仅测试读):帧末滚动偏移 + 视口矩形 + 内容高度 —— 无头测试
/// 用这三样把 `Align::Center` 的换算(块中心 − 视口中心,再钳到
/// `[0, 内容高 − 视口高]`)复算一遍,做「跳转落点方位」的精确断言。
/// 生产只写不读,字段读取都活在 `cfg(test)`,非测试构建豁免 dead_code。
#[derive(Clone, Copy)]
#[cfg_attr(not(test), allow(dead_code))]
struct ScrollProbe {
    offset: f32,
    viewport: egui::Rect,
    content_height: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::emoji_data;
    use eframe::egui::RawInput;
    use eframe::epaint::text::ByteRangeExt as _;

    /// 造一个指定流式/最近 prompt 的 AI 状态(卡片状态的三个输入)。
    fn ai_state(streaming: bool, last_prompt: Option<&str>) -> AiState {
        AiState {
            runtime: crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::new()),
            rx: None,
            streaming,
            last_prompt: last_prompt.map(str::to_owned),
            config: crate::ai_config::AiConfig::default(),
        }
    }

    /// handler 三态:ai:// 链接解析暂存且被拦截;非 ai:// 前缀不拦截(返回
    /// false,vendored 默认走系统浏览器);link_style 只对 ai:// 换色。
    #[test]
    fn handler_intercepts_ai_links_and_passes_through_others() {
        let ctx = egui::Context::default();
        let mut outbox = Vec::new();
        ctx.run_ui(RawInput::default(), |ui| {
            let handler = AiLinkHandler::new(
                ai_link_color(ui.visuals().dark_mode),
                ui.visuals().dark_mode,
                ui.visuals().text_color(),
                &AiState::default(),
            );
            assert!(handler.click("续写", "ai://write?prompt=%E7%BB%AD%E5%86%99", ui));
            assert!(
                !handler.click("LaterMD", "https://github.com/ailater/LaterMd", ui),
                "非 ai:// 前缀不拦截"
            );
            let style = handler.link_style("ai://write?prompt=x").unwrap();
            assert_eq!(style.color, Some(ai_link_color(true)));
            assert!(style.underline);
            assert!(
                handler.link_style("https://example.com").is_none(),
                "普通链接保持默认超链接样式"
            );
            handler.drain_into(&mut outbox);
        })
        .drop_without_applying_deltas();
        assert_eq!(
            outbox,
            vec![Message::AiLinkClicked {
                prompt: Ok("续写".into())
            }]
        );
    }

    /// info string 判定:首词 `ai` 命中(带参数也行),语言名/大小写/空 info
    /// 不命中,与 `ai://` scheme 的大小写敏感口径一致。
    #[test]
    fn instruction_info_matches_first_word_only() {
        for hit in [Some("ai"), Some("ai title=演示"), Some(" ai ")] {
            assert!(is_instruction_info(hit), "{hit:?} 应命中");
        }
        for miss in [
            None,
            Some(""),
            Some("rust"),
            Some("aifoo"),
            Some("AI"),
            Some("markdown ai"),
        ] {
            assert!(!is_instruction_info(miss), "{miss:?} 不应命中");
        }
    }

    /// 展示截断:不超上限原样返回;超上限取前 N 个字符加 …(按字符计数,
    /// CJK 不被劈开)。
    #[test]
    fn truncate_instruction_caps_by_chars() {
        let short = "续写".repeat(10);
        assert_eq!(truncate_instruction(&short), short);

        let long = "字".repeat(INSTRUCTION_DISPLAY_CHARS + 5);
        let cut = truncate_instruction(&long);
        assert_eq!(cut.chars().count(), INSTRUCTION_DISPLAY_CHARS + 1);
        assert!(cut.ends_with('…'));
        assert!(cut
            .chars()
            .take(INSTRUCTION_DISPLAY_CHARS)
            .all(|c| c == '字'));
    }

    /// 卡片状态派生:匹配最近 prompt 才认领;流式中「进行中」、收尾「已完成」、
    /// 失败/换文档复位后(prompt 清空)与无关指令都是「未执行」。
    #[test]
    fn card_status_follows_last_prompt_and_streaming() {
        let color = ai_link_color(true);
        let idle = AiLinkHandler::new(color, true, egui::Color32::WHITE, &ai_state(false, None));
        assert_eq!(idle.card_status("续写"), AiCardStatus::Idle);

        let running = AiLinkHandler::new(
            color,
            true,
            egui::Color32::WHITE,
            &ai_state(true, Some("续写")),
        );
        assert_eq!(running.card_status("续写"), AiCardStatus::Running);

        let done = AiLinkHandler::new(
            color,
            true,
            egui::Color32::WHITE,
            &ai_state(false, Some("续写")),
        );
        assert_eq!(done.card_status("续写"), AiCardStatus::Done);

        // 其它卡片不受牵连:菜单发起的 prompt 是拼装文本,不等任何指令
        let unrelated = AiLinkHandler::new(
            color,
            true,
            egui::Color32::WHITE,
            &ai_state(true, Some("请续写以下文档内容:\n……")),
        );
        assert_eq!(unrelated.card_status("续写"), AiCardStatus::Idle);
    }

    /// 汇集一帧里画出的全部文本(卡片与普通代码块都靠 Text shape 呈现)。
    fn painted_text(output: &eframe::egui::FullOutput) -> Vec<String> {
        fn collect(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
            match shape {
                egui::epaint::Shape::Text(t) => out.push(t.galley.text().to_owned()),
                egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut out);
        }
        out
    }

    /// 含 AI 指令块的文档整帧渲染不 panic,卡片三要素(标题/指令/按钮)都在,
    /// 普通代码块不受牵连;空指令块的按钮禁用路径同样只渲染不执行。
    #[test]
    fn ai_block_renders_card_without_panic() {
        let doc =
            "```rust\nfn main() {}\n```\n\n```ai\n续写一段 Markdown 介绍\n```\n\n```ai\n\n```\n";
        let ctx = egui::Context::default();
        let mut outbox = Vec::new();
        let output = ctx.run_ui(RawInput::default(), |panel| {
            let text = doc.to_owned();
            let (rendered, offset_map) = latermd_md::expand_wikilinks_with_map(&text);
            let mut preview = PreviewState {
                rendered,
                offset_map,
                highlight_map: latermd_md::OffsetMap::empty(),
                emoji_map: latermd_md::OffsetMap::empty(),
                task_map: latermd_md::OffsetMap::empty(),
                text,
                synced_rev: 0,
                outline: Vec::new(),
                scroll_target: None,
            };
            ui(
                panel,
                &mut preview,
                &AiState::default(),
                1,
                false,
                None,
                &mut outbox,
            );
        });
        let painted = painted_text(&output);
        output.drop_without_applying_deltas();

        for expected in ["AI 指令", "续写一段 Markdown 介绍", "执行", "(空指令)"] {
            assert!(
                painted.iter().any(|t| t.contains(expected)),
                "缺 {expected}:{painted:?}"
            );
        }
        // rust 围栏走原代码块路径(高亮 galley),不在卡片里
        assert!(
            painted.iter().any(|t| t.contains("fn main()")),
            "普通代码块丢失"
        );
        // 渲染帧没有点击,不产消息
        assert!(outbox.is_empty());
    }

    /// 卡片消息路由:「执行」产出 [`Message::AiLinkClicked`](Ok=完整指令原文,
    /// 不受展示截断影响),经 drain_into 进 outbox,与 ai:// 链接同一归约入口。
    #[test]
    fn execute_routes_ai_link_clicked_with_full_instruction() {
        let handler = AiLinkHandler::new(
            ai_link_color(true),
            true,
            egui::Color32::WHITE,
            &AiState::default(),
        );
        handler.request_execute("总结,本文要点!(含标点)");
        let mut outbox = Vec::new();
        handler.drain_into(&mut outbox);
        assert_eq!(
            outbox,
            vec![Message::AiLinkClicked {
                prompt: Ok("总结,本文要点!(含标点)".into())
            }]
        );
    }

    /// `[[wikilink]]` 的点击:`wiki://` 被拦成 [`Message::WikilinkClicked`],
    /// 不交系统浏览器;空目标不拦(交回默认行为)。
    #[test]
    fn wiki_link_click_is_intercepted_as_wikilink_message() {
        let ctx = egui::Context::default();
        let handler = AiLinkHandler::new(
            ai_link_color(true),
            true,
            egui::Color32::WHITE,
            &AiState::default(),
        );
        let intercepted = std::cell::Cell::new(true);
        let output = ctx.run_ui(RawInput::default(), |ui| {
            handler.click("架构决策", "wiki://架构决策", ui);
            // 普通 http 链接不拦:交回 vendored 默认的 open_url
            intercepted.set(handler.click("x", "https://example.com", ui));
        });
        output.drop_without_applying_deltas();
        assert!(!intercepted.get(), "http 链接不应被拦");

        let mut outbox = Vec::new();
        handler.drain_into(&mut outbox);
        assert_eq!(
            outbox,
            vec![Message::WikilinkClicked {
                target: "架构决策".into()
            }]
        );
    }

    /// 本地文档链接的点击(#doclink):相对路径/纯锚点被拦成
    /// [`Message::DocLinkClicked`](应用内跳转,归约侧解析),不交系统
    /// 浏览器;外部 scheme 照旧不拦(交回 vendored 默认 open_url)。
    #[test]
    fn local_doc_link_click_is_intercepted_as_doc_link_message() {
        let ctx = egui::Context::default();
        let handler = AiLinkHandler::new(
            ai_link_color(true),
            true,
            egui::Color32::WHITE,
            &AiState::default(),
        );
        let output = ctx.run_ui(RawInput::default(), |ui| {
            let local = handler.click("正文", "./正文.md#安装", ui);
            let anchor = handler.click("目录", "#安装指南", ui);
            let external = handler.click("x", "https://example.com", ui);
            assert!(local, "相对路径应被拦");
            assert!(anchor, "纯锚点应被拦");
            assert!(!external, "http 链接不应被拦");
        });
        output.drop_without_applying_deltas();

        let mut outbox = Vec::new();
        handler.drain_into(&mut outbox);
        assert_eq!(
            outbox,
            vec![
                Message::DocLinkClicked {
                    href: "./正文.md#安装".into()
                },
                Message::DocLinkClicked {
                    href: "#安装指南".into()
                },
            ]
        );
    }

    /// 预览消费滚动目标一次:不消费会导致每帧都把预览拽回目标位置,用户
    /// 再也滚不动(锚点缺失时也不 panic)。
    #[test]
    fn preview_consumes_scroll_target_once() {
        let ctx = egui::Context::default();
        let doc = "# 一\n\n正文\n\n## 二\n\n正文二\n";
        let mut preview = PreviewState {
            rendered: doc.to_owned(),
            offset_map: latermd_md::OffsetMap::empty(),
            highlight_map: latermd_md::OffsetMap::empty(),
            emoji_map: latermd_md::OffsetMap::empty(),
            task_map: latermd_md::OffsetMap::empty(),
            text: doc.to_owned(),
            synced_rev: 0,
            outline: Vec::new(),
            scroll_target: Some(0),
        };
        let mut outbox = Vec::new();
        let output = ctx.run_ui(RawInput::default(), |panel| {
            ui(
                panel,
                &mut preview,
                &AiState::default(),
                1,
                false,
                None,
                &mut outbox,
            );
        });
        output.drop_without_applying_deltas();
        assert_eq!(preview.scroll_target, None, "滚动目标只消费一次");
        assert!(outbox.is_empty(), "滚动不产消息");
    }

    /// 偏移映射多层口径:无改写恒等;wikilink 之后的源偏移按长度差平移;
    /// 高亮/emoji 链接与相对图片改写按同一顺序叠加;偏移落在改写区间内归
    /// 段首。(各层的「映射与输出逐字节一致」由 latermd-md 的
    /// `expand_wikilinks_with_map` / `expand_highlight_links` /
    /// `expand_emoji_links` 测试直接锁死 —— 本侧只验多层串行穿过。)
    #[test]
    fn map_source_offset_through_both_rewrites() {
        // 无改写:恒等
        let identity = latermd_md::OffsetMap::empty();
        assert_eq!(
            map_source_offset(&identity, &identity, &identity, &identity, "# h\n", None, 3),
            3
        );

        // wikilink 改写:[[架构决策]](12 字节)→ [架构决策](<wiki://架构决策>)
        // (2+12+2+4+8+12+2=…);标题在 wikilink 之后,映射后偏移必须落在
        // 展开文本里同一个标题的字节上。
        let source = "见 [[架构决策]] 再谈。\n\n## 后续标题\n\n正文。\n";
        let (rendered, offset_map) = latermd_md::expand_wikilinks_with_map(source);
        let heading_src = source.find("## 后续标题").expect("heading in source");
        let heading_out = rendered.find("## 后续标题").expect("heading in rendered");
        let mapped = map_source_offset(
            &offset_map,
            &identity,
            &identity,
            &identity,
            &rendered,
            None,
            heading_src,
        );
        assert_eq!(mapped, heading_out, "wikilink 之后的偏移按平移映射");

        // 偏移落在 wikilink 区间内(点击目标是标题,标题不会落在链接里,
        // 但边界语义仍要确定):归改写段首。
        let link_start = source.find("[[").expect("wikilink");
        let in_link = link_start + 3;
        let mapped_in = map_source_offset(
            &offset_map,
            &identity,
            &identity,
            &identity,
            &rendered,
            None,
            in_link,
        );
        assert_eq!(
            &rendered[mapped_in..].chars().take(3).collect::<String>(),
            "[架构",
            "区间内偏移归改写段首: {mapped_in}"
        );

        // 高亮层叠加(#65):展开文本里的 `==…==` 改写成链接后,标题偏移在
        // wikilink 平移之上再平移一次。
        let source = "==高亮== [[架构决策]]。\n\n## 后续标题\n\n正文。\n";
        let (after_wikilinks, offset_map) = latermd_md::expand_wikilinks_with_map(source);
        let (rendered, highlight_map) = latermd_md::expand_highlight_links(&after_wikilinks);
        assert!(rendered.contains("[高亮](<hl://>)"), "前置:高亮已改写");
        let heading_src = source.find("## 后续标题").expect("heading in source");
        let heading_out = rendered.find("## 后续标题").expect("heading in rendered");
        let mapped = map_source_offset(
            &offset_map,
            &highlight_map,
            &identity,
            &identity,
            &rendered,
            None,
            heading_src,
        );
        assert_eq!(mapped, heading_out, "高亮改写叠加平移");

        // emoji 层叠加(#48 B1):展开文本里的覆盖枚改写成链接后,标题偏移
        // 在前层平移之上再平移一次。
        let source = "见 [[架构决策]] 与 😀 再谈。\n\n## 后续标题\n\n正文。\n";
        let (after_wikilinks, offset_map) = latermd_md::expand_wikilinks_with_map(source);
        let (after_highlight, highlight_map) = latermd_md::expand_highlight_links(&after_wikilinks);
        let (rendered, emoji_map) =
            latermd_md::expand_emoji_links(&after_highlight, emoji_data::covered_glyphs());
        let heading_src = source.find("## 后续标题").expect("heading in source");
        let heading_out = rendered.find("## 后续标题").expect("heading in rendered");
        let mapped = map_source_offset(
            &offset_map,
            &highlight_map,
            &emoji_map,
            &identity,
            &rendered,
            None,
            heading_src,
        );
        assert_eq!(mapped, heading_out, "emoji 改写叠加平移");

        // 图片层叠加:渲染文本里相对图片地址换成 file:// URI 后,标题偏移
        // 再平移一次(与前层共存)。`rendered` 是 emoji 层的真实输出 ——
        // 图片层扫描的正是这份字符串(生产同款口径)。
        let doc = "![图](./x.png) 😀\n\n## 标题\n";
        let (rendered, emoji_map) =
            latermd_md::expand_emoji_links(doc, emoji_data::covered_glyphs());
        let base = Some(Path::new("/doc"));
        let with_uri = resolve_relative_images(&rendered, base);
        let heading = doc.find("## 标题").expect("heading");
        let heading_uri = with_uri.find("## 标题").expect("heading in uri text");
        let mapped_img = map_source_offset(
            &identity, &identity, &emoji_map, &identity, &rendered, base, heading,
        );
        assert_eq!(mapped_img, heading_uri, "图片 URI 改写叠加平移");
    }

    /// #48 B1 覆盖集的数据卫生:全表 272 枚改写后能被解析成链接,href 剥
    /// scheme 前缀还原原文(可逆口径),且载荷不含尖括号目标的非法字符
    /// (`>`、换行、空白 —— 含则改写产物不是合法 CommonMark)。
    #[test]
    fn covered_glyphs_round_trip_through_emoji_links() {
        let covered = emoji_data::covered_glyphs();
        assert_eq!(covered.len(), 272, "面板数据表全量");
        for glyph in covered {
            for c in glyph.chars() {
                assert!(
                    !matches!(c, '>' | '<' | '\n' | '\r' | ' ' | '\t'),
                    "{glyph:?} 含尖括号目标非法字符"
                );
            }
            let (rendered, _) = latermd_md::expand_emoji_links(glyph, covered);
            assert_eq!(
                rendered,
                format!("[{g}](<{}{g}>)", latermd_md::EMOJI_SCHEME, g = glyph),
                "{glyph:?} 单枚文档改写形状"
            );
            // 可逆:href 剥前缀还原原文,且真能被解析成链接 token
            let href = format!("{}{}", latermd_md::EMOJI_SCHEME, glyph);
            assert_eq!(href.strip_prefix(latermd_md::EMOJI_SCHEME), Some(*glyph));
            let doc = latermd_md::parse(&rendered);
            assert!(
                doc.tokens.iter().any(|token| match token {
                    egui_markdown::types::Token::Link {
                        href: found, text, ..
                    } => found.as_ref() == href && text.as_ref() == *glyph,
                    _ => false,
                }),
                "{glyph:?} 未解析成 emoji:// 链接"
            );
        }
    }

    /// #48 B1 接线回归:emoji 改写后的渲染串过生产入口 `ui()` 明暗两主题
    /// 各渲染一帧不 panic,emoji 密集文档的正文/标题/列表文字仍在;围栏
    /// 代码块的 emoji 保持原字符(未被改写,代码路径零牵连)。
    /// (链接的样式与点击行为归 #48 B2 的 handler 接线,本测只钉 B1 的
    /// 改写不破坏渲染。)
    #[test]
    fn preview_ui_renders_emoji_rewritten_doc_without_panic() {
        let doc = "# 标题 😀 一\n\n正文 😀🚀 密集 💡 段。\n\n- 项 🚀\n\n> 引 😀\n\n```rust\nlet e = \"😀\";\n```\n";
        let (after_wikilinks, _) = latermd_md::expand_wikilinks_with_map(doc);
        let (rendered, _) =
            latermd_md::expand_emoji_links(&after_wikilinks, emoji_data::covered_glyphs());
        assert!(
            rendered.contains("正文 [😀](<emoji://😀>)[🚀](<emoji://🚀>)"),
            "前置:改写确实发生(断言非恒真):{rendered}"
        );
        for dark in [true, false] {
            let ctx = egui::Context::default();
            ctx.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            // OffsetMap 不 Clone,两帧各建一份(纯函数,同输入同产出)
            let (after_wikilinks, offset_map) = latermd_md::expand_wikilinks_with_map(doc);
            let (frame_rendered, emoji_map) =
                latermd_md::expand_emoji_links(&after_wikilinks, emoji_data::covered_glyphs());
            assert_eq!(frame_rendered, rendered, "纯函数:两帧改写产出一致");
            let mut preview = PreviewState {
                rendered: frame_rendered,
                offset_map,
                highlight_map: latermd_md::OffsetMap::empty(),
                emoji_map,
                task_map: latermd_md::OffsetMap::empty(),
                text: doc.to_owned(),
                synced_rev: 0,
                outline: Vec::new(),
                scroll_target: None,
            };
            let mut outbox = Vec::new();
            let output = ctx.run_ui(RawInput::default(), |panel| {
                ui(
                    panel,
                    &mut preview,
                    &AiState::default(),
                    7,
                    false,
                    None,
                    &mut outbox,
                );
            });
            let painted = painted_text(&output);
            output.drop_without_applying_deltas();
            for expected in ["标题", "密集", "项", "let e = \"😀\";"] {
                assert!(
                    painted.iter().any(|t| t.contains(expected)),
                    "dark={dark} 缺 {expected}:{painted:?}"
                );
            }
        }
    }

    // —— #48 B2:emoji:// inline widget 接线(可行性调查 §3.2 点名的
    // 段落/标题/表格覆盖断言 + 副作用压住 + 回落面)——

    /// 一帧生产入口渲染的汇集(测试断言素材)。文档先走 B1 同款改写链
    /// (wikilink → 高亮 → emoji),与 `PreviewState::new` 生产链一致。
    struct EmojiFrame {
        /// 全部 Text shape 的文本(painted_text 同款汇集)。
        texts: Vec<String>,
        /// 画出的图片((纹理 id, mesh 包围盒) —— `Painter::image` 落成
        /// Mesh shape,纹理走 TextureManager 分配的 Managed id(egui 0.36
        /// 的 `load_texture` 即如此,不是 User)。这些测试文档里没有其它
        /// 图片来源,首帧可再与 `delta72` 交叉核对纹理身份。
        images: Vec<(egui::TextureId, egui::Rect)>,
        /// emoji widget 探针区块(含纹理缺失只留占位的)。
        probe: Vec<egui::Rect>,
        /// 本帧悬停光标(手型抑制断言)。
        cursor: egui::CursorIcon,
        /// 本帧上传的整幅 72×72 纹理的 id 集合(textures_delta,Twemoji 资产
        /// 尺寸;首帧解码上传,次帧走缓存不再出现)—— 图片 mesh 的纹理
        /// 身份交叉核对用。
        delta72: std::collections::HashSet<egui::TextureId>,
        /// 全部 Text shape 的(原点, galley)—— 按 glyph 位置精确定位文本用。
        galleys: Vec<(egui::Pos2, std::sync::Arc<egui::Galley>)>,
    }

    /// 生产入口渲染一帧并汇集断言素材(见 [`EmojiFrame`])。
    fn render_emoji_frame(
        ctx: &egui::Context,
        doc: &str,
        events: Vec<egui::Event>,
        tab_id: u64,
    ) -> EmojiFrame {
        let (rendered, offset_map) = latermd_md::expand_wikilinks_with_map(doc);
        let (rendered, emoji_map) =
            latermd_md::expand_emoji_links(&rendered, emoji_data::covered_glyphs());
        let mut preview = PreviewState {
            rendered,
            offset_map,
            highlight_map: latermd_md::OffsetMap::empty(),
            emoji_map,
            task_map: latermd_md::OffsetMap::empty(),
            text: doc.to_owned(),
            synced_rev: 0,
            outline: Vec::new(),
            scroll_target: None,
        };
        let mut outbox = Vec::new();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let output = ctx.run_ui(
            RawInput {
                events,
                screen_rect: Some(screen),
                ..Default::default()
            },
            |panel| {
                ui(
                    panel,
                    &mut preview,
                    &AiState::default(),
                    tab_id,
                    false,
                    None,
                    &mut outbox,
                );
            },
        );
        let mut images = Vec::new();
        let mut galleys = Vec::new();
        fn collect(
            shape: &egui::epaint::Shape,
            images: &mut Vec<(egui::TextureId, egui::Rect)>,
            galleys: &mut Vec<(egui::Pos2, std::sync::Arc<egui::Galley>)>,
        ) {
            match shape {
                egui::epaint::Shape::Mesh(mesh) => {
                    if let Some(first) = mesh.vertices.first() {
                        let mut min = first.pos;
                        let mut max = first.pos;
                        for vertex in &mesh.vertices {
                            min = min.min(vertex.pos);
                            max = max.max(vertex.pos);
                        }
                        images.push((mesh.texture_id, egui::Rect::from_min_max(min, max)));
                    }
                }
                egui::epaint::Shape::Text(t) => galleys.push((t.pos, t.galley.clone())),
                egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| collect(s, images, galleys)),
                _ => {}
            }
        }
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut images, &mut galleys);
        }
        // The preview can also emit mesh shapes for scroll fades when the
        // document grows past the viewport.  They are UI chrome, not emoji
        // textures, so keep the probe's image count focused on inline widgets.
        let probe = ctx
            .data(|d| d.get_temp::<Vec<egui::Rect>>(emoji_probe_id(tab_id)))
            .unwrap_or_default();
        images.retain(|(_, rect)| probe.iter().any(|widget| widget.contains(rect.center())));
        let delta72 = output
            .textures_delta
            .set
            .iter()
            .filter(|(_, deltas)| {
                deltas.iter().any(|delta| {
                    delta.pos.is_none() && delta.image.width() == 72 && delta.image.height() == 72
                })
            })
            .map(|(id, _)| *id)
            .collect();
        let frame = EmojiFrame {
            texts: painted_text(&output),
            images,
            probe,
            cursor: output.platform_output.cursor_icon,
            delta72,
            galleys,
        };
        output.drop_without_applying_deltas();
        frame
    }

    /// 在汇集的 galley 里找 `needle` 首字的字形行带(屏幕坐标):返回该行
    /// 的 y 区间 —— pos_from_cursor 是 0 宽 Rect,y 即所在行的 min_y..max_y。
    fn row_band_of(frame: &EmojiFrame, needle: &str) -> egui::Rect {
        for (origin, galley) in &frame.galleys {
            if let Some(byte) = galley.text().find(needle) {
                let chars = galley.text()[..byte].chars().count();
                let band = galley.pos_from_cursor(egui::text::CCursor::new(chars));
                return egui::Rect::from_min_max(
                    egui::pos2(origin.x + band.left(), origin.y + band.top()),
                    egui::pos2(origin.x + band.right(), origin.y + band.bottom()),
                );
            }
        }
        panic!("{needle:?} 不在任何文本 shape 内:{:?}", frame.texts)
    }

    /// 段落覆盖断言(§3.2 第一段):正文里的 emoji 经 inline widget 画成
    /// **带纹理的正方形图片**,尺寸与正文字号同源(边长落在字号 0.8-3 倍
    /// 区间),两枚按文档序左右排开,且纵向落在正文行带内;正文文字照常
    /// 在文本层。
    #[test]
    fn emoji_inline_widget_paints_textured_squares_in_paragraph() {
        let ctx = egui::Context::default();
        let doc = "正文 😀 与 🚀 密集段落。";
        let frame = render_emoji_frame(&ctx, doc, Vec::new(), 11);
        assert_eq!(frame.probe.len(), 2, "两枚 emoji 各留一个 widget 区块");
        assert_eq!(frame.images.len(), 2, "两枚 emoji 各画一张纹理图");
        let size = crate::theme::editor_font_size(&ctx);
        let band = row_band_of(&frame, "正文");
        assert_eq!(frame.delta72.len(), 2, "首帧解码上传两枚 Twemoji 纹理");
        for (texture, image) in &frame.images {
            assert!(
                frame.delta72.contains(texture),
                "图片 mesh 绑定的就是本帧上传的 72×72 Twemoji 纹理:{texture:?}"
            );
            assert!(
                (image.width() - image.height()).abs() <= 0.51,
                "画的是正方形:{image:?}"
            );
            let side = image.width();
            assert!(
                (0.8 * size..=3.0 * size).contains(&side),
                "边长 {side} 应与正文字号 {size} 同源(0.8-3 倍区间)"
            );
            assert!(
                band.top() - 1.0 <= image.center().y && image.center().y <= band.bottom() + 1.0,
                "图片纵向落在正文行带 {band:?} 内:{image:?}"
            );
        }
        // 文档序:😀 在 🚀 左边
        assert!(
            frame.images[0].1.left() < frame.images[1].1.left(),
            "图片按文档序排开:{:?}",
            frame.images
        );
        for expected in ["正文", "与", "密集段落"] {
            assert!(
                frame.texts.iter().any(|t| t.contains(expected)),
                "正文缺 {expected}:{:?}",
                frame.texts
            );
        }
    }

    /// 标题覆盖断言(§3.2 第二段 + #91 vendor ①类收口):heading 里的 emoji
    /// **确实吃到** inline widget —— 图片纵向落在「标题」文本的同一行带内;
    /// 边长随标题档放大:#91 的 vendor 补丁让 `Token::Link` 透传所在 heading
    /// level(parser),vendored `build_layout` 对 inline widget 分支按
    /// `heading.scales` 档位缩放入参字体(app 生产 style H1 = 2.0×,theme.rs),
    /// `inline_widget_size`/占位 shaping 都按入参派生,app 生产代码零改动。
    /// 标题里的**普通链接文本**(wiki:// 等)不吃缩放 —— vendored 改动严格限
    /// inline widget 字体路径,既有链接文本渲染逐像素不变(否决线在 vendored
    /// 测试 tests/inline_widget_heading_scale.rs 钉死)。
    #[test]
    fn emoji_inline_widget_scales_with_heading_level() {
        let ctx = egui::Context::default();
        let heading = render_emoji_frame(&ctx, "# 标题 😀 落位", Vec::new(), 12);
        assert_eq!(heading.images.len(), 1, "标题里恰一枚 emoji 图");
        let band = row_band_of(&heading, "标");
        let image = heading.images[0].1;
        assert!(
            band.top() - 1.0 <= image.center().y && image.center().y <= band.bottom() + 1.0,
            "图片纵向落在标题行带 {band:?} 内:{image:?}"
        );

        let para = render_emoji_frame(&ctx, "对照段落 😀", Vec::new(), 13);
        assert_eq!(para.images.len(), 1);
        let ratio = image.width() / para.images[0].1.width();
        assert!(
            (ratio - 2.0).abs() <= 0.1,
            "标题档边长 ≈ 段落档 × H1 scale(2.0):{ratio}({} vs {})",
            image.width(),
            para.images[0].1.width()
        );
        assert!(
            image.width() > para.images[0].1.width() * 1.5,
            "emoji 随标题字号放大,不再是段落档:{} vs {}",
            image.width(),
            para.images[0].1.width()
        );
    }

    /// 表格单元格覆盖断言(§3.2 第三段):表格 cell 走 vendored 的另一条
    /// `render_link_in_ui` 路径(每链接一个 widget),emoji 同样画成纹理图,
    /// 表头与正文文字照常渲染。
    #[test]
    fn emoji_inline_widget_paints_in_table_cell() {
        let ctx = egui::Context::default();
        let doc = "| 名称 | 图标 |\n| --- | --- |\n| 文字行 | 😀 |\n";
        let frame = render_emoji_frame(&ctx, doc, Vec::new(), 14);
        assert_eq!(frame.probe.len(), 1, "表内 emoji 留一个 widget 区块");
        assert_eq!(frame.images.len(), 1, "表内 emoji 画一张纹理图");
        let (_, image) = frame.images[0];
        assert!(
            (image.width() - image.height()).abs() <= 0.51,
            "表内同样画正方形:{image:?}"
        );
        for expected in ["名称", "图标", "文字行"] {
            assert!(
                frame.texts.iter().any(|t| t.contains(expected)),
                "表格缺 {expected}:{:?}",
                frame.texts
            );
        }
    }

    /// 副作用压住(handler 层):`emoji://` 的 link_style = 正文色 + 无下划线
    /// (不吃默认超链接样式);click 吞掉(返回 true)且**不产任何消息**,
    /// 不交系统浏览器;非 emoji:// 的行为分毫不动(默认样式/放行浏览器)。
    #[test]
    fn emoji_link_style_and_click_are_contained() {
        let body = egui::Color32::from_rgb(0x11, 0x22, 0x33);
        let ctx = egui::Context::default();
        let handler = AiLinkHandler::new(ai_link_color(true), true, body, &AiState::default());
        let output = ctx.run_ui(RawInput::default(), |ui| {
            let style = handler.link_style("emoji://😀").expect("emoji:// 有样式");
            assert_eq!(style.color, Some(body), "正文色,不吃超链接色");
            assert!(!style.underline, "无下划线");
            assert!(handler.click("😀", "emoji://😀", ui), "emoji:// 点击被吞掉");
            assert!(
                !handler.click("x", "https://example.com", ui),
                "普通链接照常放行"
            );
            // 尺寸判定:与传入字号匹配的正方形;非 emoji:// 不进 widget 路径
            assert_eq!(
                handler.inline_widget_size("emoji://😀", &egui::FontId::proportional(15.0)),
                Some(egui::vec2(15.0, 15.0))
            );
            assert_eq!(
                handler
                    .inline_widget_size("https://example.com", &egui::FontId::proportional(15.0)),
                None,
                "非 emoji:// 不认 inline widget"
            );
        });
        output.drop_without_applying_deltas();
        let mut outbox = Vec::new();
        handler.drain_into(&mut outbox);
        assert!(outbox.is_empty(), "吞掉的 emoji 点击不产消息:{outbox:?}");
    }

    /// 失败面:纹理查不到(资产表外的手写 `emoji://` 链接,模拟资产缺失/
    /// 解码失败面)→ 什么都不画,透明占位原样保持 —— 不画黑块、不 panic、
    /// 周边文本照常渲染。连续两帧钉住「未命中不落负缓存、次帧同样安全」。
    #[test]
    fn emoji_texture_miss_keeps_placeholder_without_panicking() {
        let uncovered = "🫠";
        assert!(
            !emoji_data::covered_glyphs().contains(uncovered),
            "前置:选的字符必须在资产表外"
        );
        let doc = format!(
            "前文 [{uncovered}](<{}{uncovered}>) 后文",
            latermd_md::EMOJI_SCHEME
        );
        let ctx = egui::Context::default();
        for frame in 0..2 {
            let frame_data = render_emoji_frame(&ctx, &doc, Vec::new(), 15);
            assert_eq!(
                frame_data.probe.len(),
                1,
                "第 {frame} 帧:widget 区块仍在(占位不动)"
            );
            assert!(
                frame_data.images.is_empty(),
                "第 {frame} 帧:查不到纹理就不画图:{:?}",
                frame_data.images
            );
            for expected in ["前文", "后文"] {
                assert!(
                    frame_data.texts.iter().any(|t| t.contains(expected)),
                    "第 {frame} 帧缺 {expected}:{:?}",
                    frame_data.texts
                );
            }
        }
    }

    /// 手型抑制(副作用压住的第三项):悬停 emoji → 光标保持 Default;同
    /// 文档悬停普通链接 → PointingHand(vendored 默认行为不受牵连)。vendored
    /// 对 inline widget 悬停无条件置手型(label.rs `handle_hover`),app 侧在
    /// label 渲染后按探针区块把光标压回 —— 两个方向都要钉住。
    #[test]
    fn emoji_hover_keeps_default_cursor_while_links_keep_pointing_hand() {
        let ctx = egui::Context::default();
        let doc = "开头 😀 结尾 [跳转链接](https://example.com/target) 完。";
        let frame = render_emoji_frame(&ctx, doc, Vec::new(), 16);
        assert_eq!(
            frame.images.len(),
            1,
            "文档只有一枚 emoji:{:?}",
            frame.images
        );
        let emoji_center = frame.probe[0].center();
        // 行带是 0 宽 caret rect(见 row_band_of),停在边界上会把命中让给
        // 前一个字符(空格,非链接段);往右挪半个字形,落进「跳」 glyph 内部
        let band = row_band_of(&frame, "跳转链接");
        let link_center = egui::pos2(band.left() + 8.0, band.center().y);

        let hover = |pos| vec![egui::Event::PointerMoved(pos)];
        let on_emoji = render_emoji_frame(&ctx, doc, hover(emoji_center), 16);
        assert_eq!(on_emoji.cursor, egui::CursorIcon::Default, "emoji 不吃手型");
        let on_link = render_emoji_frame(&ctx, doc, hover(link_center), 16);
        assert_eq!(
            on_link.cursor,
            egui::CursorIcon::PointingHand,
            "普通链接的手型不受牵连"
        );
    }

    /// 明暗两主题 × emoji 密集文档 × 连续两帧:渲染不 panic,每帧每枚覆盖
    /// emoji 都出图(首帧解码上传,次帧走会话缓存),围栏代码块与行内代码
    /// 里的 emoji 保持字面文本(B1 豁免在渲染层的复测:不进 widget 路径,
    /// 不多画一张图)。
    #[test]
    fn emoji_dense_document_renders_both_themes_without_panicking() {
        let mut doc = String::from("# 密集 😀 标题\n\n");
        // 6 行正文保持全文落进 800×600 视口(视口剔除是按设计工作的,底部
        // 出画的块本来就不该画 —— 断言的是「可见的全画」,不是「全可见」)
        for i in 0..6 {
            doc.push_str(&format!("第 {i} 行 😀🚀💡👍 正文收尾。\n\n"));
        }
        doc.push_str("- 项 ✅\n- 项 💡\n\n> 引用 💡\n\n");
        doc.push_str("| 键 | 值 |\n| --- | --- |\n| 图 | 🚀 |\n\n");
        doc.push_str("```rust\nlet e = \"😀\";\n```\n\n行内代码 `🚀` 结束。\n");
        for dark in [true, false] {
            let ctx = egui::Context::default();
            ctx.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            for frame in 0..2 {
                let data = render_emoji_frame(&ctx, &doc, Vec::new(), 17);
                // 正文 6 行 ×4 枚 + 标题 1 + 列表 2 + 引用 1 + 表格 1 = 29 枚
                // (代码块/行内代码里的 😀/🚀 被 B1 豁免,不进 widget)
                assert_eq!(
                    data.probe.len(),
                    29,
                    "dark={dark} 第 {frame} 帧:覆盖 emoji 全数进 widget"
                );
                assert_eq!(
                    data.images.len(),
                    29,
                    "dark={dark} 第 {frame} 帧:可见全数画图(代码内外不混)"
                );
                for expected in ["密集", "正文收尾", "let e = \"😀\";", "行内代码"] {
                    assert!(
                        data.texts.iter().any(|t| t.contains(expected)),
                        "dark={dark} 第 {frame} 帧缺 {expected}:{:?}",
                        data.texts
                    );
                }
            }
        }
    }

    /// 端到端:scroll_target 指向文档尾部标题,一帧 `ui()` 后预览 ScrollArea
    /// 真的滚动了(断点修复的回归锁 —— 修复前滚动指令写在 ScrollArea 闭包
    /// 外,永远不生效)。
    #[test]
    fn outline_jump_actually_scrolls_preview() {
        let ctx = egui::Context::default();
        let mut doc = String::from("# 顶部\n\n");
        for i in 0..30 {
            doc.push_str(&format!("第 {i} 段正文,占高度用。\n\n"));
        }
        doc.push_str("# 底部标题\n\n收尾。\n");
        let heading = doc.find("# 底部标题").expect("tail heading");
        let mut preview = PreviewState {
            rendered: doc.clone(),
            offset_map: latermd_md::OffsetMap::empty(),
            highlight_map: latermd_md::OffsetMap::empty(),
            emoji_map: latermd_md::OffsetMap::empty(),
            task_map: latermd_md::OffsetMap::empty(),
            text: doc.clone(),
            synced_rev: 0,
            outline: Vec::new(),
            scroll_target: Some(heading),
        };
        let mut outbox = Vec::new();
        let mut scroll_offset = 0.0f32;
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        // 3 帧 + 虚拟时间推进:滚动即使带动画也在第 2/3 帧到位,一次到位
        // (ScrollAnimation::none)则在第 1 帧就到。
        for frame in 0..3 {
            let output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    time: Some(frame as f64),
                    ..Default::default()
                },
                |panel| {
                    ui(
                        panel,
                        &mut preview,
                        &AiState::default(),
                        1,
                        false,
                        None,
                        &mut outbox,
                    );
                    scroll_offset = panel
                        .ctx()
                        .data(|d| d.get_temp::<ScrollProbe>(scroll_probe_id(1)))
                        .map_or(0.0, |probe| probe.offset);
                },
            );
            output.drop_without_applying_deltas();
        }
        assert_eq!(preview.scroll_target, None, "目标消费一次");
        assert!(
            scroll_offset > 50.0,
            "预览应滚动到尾部标题,实际 offset={scroll_offset}"
        );
    }

    /// 端到端**真点击路径**(#42 M2 验收主轴):`State` 归约
    /// `OutlineItemClicked`(源码 jump + 预览请求**双栏登记**)→ 预览消费帧
    /// 查块表滚动。落点断言不用「滚了一点」这类糊口径,而是复算 egui
    /// `Align::Center` 的换算(目标块中心 − 视口中心,再钳到
    /// `[0, 内容高 − 视口高]`,同帧 `end()` 内已钳完)误差 ≤1px;顶/中/尾
    /// 三个目标落点有序且可区分 —— 方位对应目标块,不是「随便滚了一下」。
    #[test]
    fn outline_click_via_state_scrolls_preview_centered_on_target_block() {
        let mut doc = String::from("# 顶部\n\n");
        for i in 0..30 {
            doc.push_str(&format!("第 {i} 段正文,占高度用。\n\n"));
        }
        doc.push_str("## 中部标题\n\n");
        for i in 0..15 {
            doc.push_str(&format!("续 {i} 段正文。\n\n"));
        }
        doc.push_str("# 尾部标题\n\n收尾。\n");

        // 走完整链:换文档(生产同步规则 = 修订号前进才 rebuild,此处直接
        // 调同一 API)→ 从大纲取真实条目 span → 归约 → 渲染两帧(第 2 帧
        // 确认落点稳定,不是动画中间态)。
        let jump = |heading: &str| -> (f32, f32, f32) {
            let mut state = crate::state::State::default();
            {
                let tab = state.tabs.current_mut();
                tab.editor.replace_all(&doc);
                tab.preview.rebuild(&tab.editor);
            }
            let span = state
                .tabs
                .current()
                .preview
                .outline
                .iter()
                .find(|item| item.text == heading)
                .unwrap_or_else(|| panic!("大纲缺 {heading}"))
                .span
                .clone();
            state.apply(Message::OutlineItemClicked(span.clone()));
            // 归约帧双栏都在场:预览请求登记 + 源码跳转请求登记(本测试
            // 不画编辑器,jump_to 应保持待消费)
            assert_eq!(
                state.tabs.current().preview.scroll_target,
                Some(span.start),
                "{heading}: 预览请求已登记"
            );
            assert!(
                state.tabs.current().cursor.jump_to.is_some(),
                "{heading}: 源码侧请求同帧登记(双栏)"
            );

            let ctx = egui::Context::default();
            let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
            let mut outbox = Vec::new();
            let mut block_rect = None;
            let mut probe = None;
            for frame in 0..2 {
                let output = ctx.run_ui(
                    RawInput {
                        screen_rect: Some(screen),
                        time: Some(frame as f64),
                        ..Default::default()
                    },
                    |panel| {
                        {
                            let tab = state.tabs.current_mut();
                            ui(
                                panel,
                                &mut tab.preview,
                                &AiState::default(),
                                1,
                                false,
                                None,
                                &mut outbox,
                            );
                        }
                        if frame == 0 {
                            // 与生产消费同帧读块表(帧号键控,跨帧即 None):
                            // 拿到的就是生产 `scroll_to_rect_animation` 滚向
                            // 的那个块。首帧滚动起点是 0,记录 rect 即内容坐标。
                            let tab = state.tabs.current();
                            let mapped = map_source_offset(
                                &tab.preview.offset_map,
                                &tab.preview.highlight_map,
                                &tab.preview.emoji_map,
                                &tab.preview.task_map,
                                &tab.preview.rendered,
                                None,
                                span.start,
                            );
                            block_rect = egui_markdown::block_rect_at_offset(
                                panel,
                                tab_preview_id(1),
                                mapped,
                            )
                            .map(|block| block.rect);
                        }
                        probe = panel
                            .ctx()
                            .data(|d| d.get_temp::<ScrollProbe>(scroll_probe_id(1)));
                    },
                );
                output.drop_without_applying_deltas();
            }
            let block = block_rect.unwrap_or_else(|| panic!("{heading}: 目标块不在表内"));
            let probe = probe.expect("探针已写入");
            assert_eq!(
                state.tabs.current().preview.scroll_target,
                None,
                "{heading}: 请求消费一次即清"
            );
            (
                probe.offset,
                block.center().y - probe.viewport.center().y,
                (probe.content_height - probe.viewport.height()).max(0.0),
            )
        };

        let (top, top_expected, top_max) = jump("顶部");
        let (mid, mid_expected, mid_max) = jump("中部标题");
        let (tail, tail_expected, tail_max) = jump("尾部标题");
        for (name, offset, expected, max) in [
            ("顶部", top, top_expected, top_max),
            ("中部标题", mid, mid_expected, mid_max),
            ("尾部标题", tail, tail_expected, tail_max),
        ] {
            let want = expected.clamp(0.0, max);
            assert!(
                (offset - want).abs() <= 1.0,
                "{name}: 偏移 {offset} 应等于 Center 换算+钳制 {want}(expected={expected}, max={max})"
            );
        }
        assert!(top < 1.0, "顶部标题钳在文档顶: {top}");
        assert!(mid > 50.0, "中部标题真的滚了: {mid}");
        assert!(
            top < mid && mid < tail,
            "落点按目标块方位有序: top={top} mid={mid} tail={tail}"
        );
    }

    /// LP2-4 端到端:文档前部有多条 `[[wikilink]]`(展开让渲染文本比源码
    /// 长,大纲 span 的源码偏移**直接**喂块表必然错位),大纲点击 → 归约
    /// → 预览消费帧经映射表换算 → 块表查询 → 滚动。断言三层:
    /// ①换算保真 —— 渲染文本从落点起逐字节重现「span.start → 标题行」的
    /// 源码片段(平移不改相对上下文,块表因此查到标题块,而不是被位移差
    /// 顶到前面的块);②复算 `Align::Center` 落点误差 ≤1px;③中部/尾部
    /// 两个目标落点有序可区分 —— 换算跟着目标走,不是「碰巧都滚到某处」。
    #[test]
    fn outline_click_through_wikilink_offsets_lands_on_target_block() {
        let mut doc = String::from("# 顶部\n\n");
        for i in 0..20 {
            doc.push_str(&format!(
                "第 {i} 段,见 [[架构决策{i}]] 与 [[Note {i}|笔记{i}]]。\n\n"
            ));
        }
        doc.push_str("## 中部标题\n\n");
        for i in 0..15 {
            doc.push_str(&format!("续 {i} 段正文。\n\n"));
        }
        doc.push_str("# 尾部标题\n\n收尾。\n");

        // 完整链:换文档(rebuild,生产同步规则)→ 换算/渲染两帧消费。
        // 返回 (换算落点, 渲染文本里目标标题的实际偏移, 滚动 offset,
        // Center 期望值, 钳制上限)。
        let jump = |heading: &str, rendered_needle: &str| -> (usize, usize, f32, f32, f32) {
            let mut state = crate::state::State::default();
            {
                let tab = state.tabs.current_mut();
                tab.editor.replace_all(&doc);
                tab.preview.rebuild(&tab.editor);
            }
            // 前置:wikilink 展开确实让文本漂移(否则换算对落点退化恒真)
            let tab = state.tabs.current();
            assert_ne!(tab.preview.rendered.len(), tab.preview.text.len());
            let rendered_at = tab
                .preview
                .rendered
                .find(rendered_needle)
                .unwrap_or_else(|| panic!("渲染文本缺 {rendered_needle}"));

            let span = tab
                .preview
                .outline
                .iter()
                .find(|item| item.text == heading)
                .unwrap_or_else(|| panic!("大纲缺 {heading}"))
                .span
                .clone();
            // span.start 吸收标题前的空行(outline 平铺口径),落在标题字符
            // 之前;换算保真的判据因此不是「== 标题字符位置」,而是「源码
            // 从 span.start 起的片段,在渲染文本的换算落点处逐字节重现」
            // (平移不改相对上下文;span.start 到标题行之间没有 wikilink,
            // 片段内部不可能再被改写)。
            let heading_char_src = tab
                .preview
                .text
                .find(rendered_needle)
                .unwrap_or_else(|| panic!("源文本缺 {rendered_needle}"));
            assert!(
                span.start <= heading_char_src,
                "{heading}: 前置 —— span.start 不先于标题字符"
            );
            let snippet =
                tab.preview.text[span.start..heading_char_src + rendered_needle.len()].to_owned();
            assert_ne!(
                span.start, rendered_at,
                "{heading}: 前置 —— 源码偏移与渲染偏移确实不同"
            );
            state.apply(Message::OutlineItemClicked(span.clone()));

            let ctx = egui::Context::default();
            let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
            let mut outbox = Vec::new();
            let mut mapped = 0;
            let mut block_rect = None;
            let mut probe = None;
            for frame in 0..2 {
                let output = ctx.run_ui(
                    RawInput {
                        screen_rect: Some(screen),
                        time: Some(frame as f64),
                        ..Default::default()
                    },
                    |panel| {
                        {
                            let tab = state.tabs.current_mut();
                            ui(
                                panel,
                                &mut tab.preview,
                                &AiState::default(),
                                1,
                                false,
                                None,
                                &mut outbox,
                            );
                        }
                        if frame == 0 {
                            // 与生产消费同帧读块表(帧号键控,跨帧即 None)
                            let tab = state.tabs.current();
                            mapped = map_source_offset(
                                &tab.preview.offset_map,
                                &tab.preview.highlight_map,
                                &tab.preview.emoji_map,
                                &tab.preview.task_map,
                                &tab.preview.rendered,
                                None,
                                span.start,
                            );
                            block_rect = egui_markdown::block_rect_at_offset(
                                panel,
                                tab_preview_id(1),
                                mapped,
                            )
                            .map(|block| block.rect);
                        }
                        probe = panel
                            .ctx()
                            .data(|d| d.get_temp::<ScrollProbe>(scroll_probe_id(1)));
                    },
                );
                output.drop_without_applying_deltas();
            }
            let block = block_rect.unwrap_or_else(|| panic!("{heading}: 换算后的偏移不在块表内"));
            let probe = probe.expect("探针已写入");
            // 换算保真:渲染文本从落点开始逐字节重现「span.start → 标题行」
            // 的源码片段(平移不改相对上下文)—— 落点正确性的直接证据
            let rendered = state.tabs.current().preview.rendered.clone();
            assert!(
                rendered[mapped..].starts_with(&snippet),
                "{heading}: 换算落点 {mapped} 处的上下文与源码不一致(应重现 {snippet:?})"
            );
            (
                mapped,
                rendered_at,
                probe.offset,
                block.center().y - probe.viewport.center().y,
                (probe.content_height - probe.viewport.height()).max(0.0),
            )
        };

        for (heading, needle) in [("中部标题", "## 中部标题"), ("尾部标题", "# 尾部标题")]
        {
            let (_mapped, _rendered_at, offset, expected, max) = jump(heading, needle);
            let want = expected.clamp(0.0, max);
            assert!(
                (offset - want).abs() <= 1.0,
                "{heading}: 偏移 {offset} 应等于 Center 换算+钳制 {want}(expected={expected}, max={max})"
            );
        }

        // 两个目标的换算落点与滚动落点都随目标前进(换算真实起效,不是
        // 所有点击都滚到同一处)
        let (mid_mapped, _, mid_offset, _, _) = jump("中部标题", "## 中部标题");
        let (tail_mapped, _, tail_offset, _, _) = jump("尾部标题", "# 尾部标题");
        assert!(
            mid_mapped < tail_mapped,
            "换算后的偏移随目标前进: mid={mid_mapped} tail={tail_mapped}"
        );
        assert!(
            mid_offset < tail_offset,
            "滚动落点随目标前进: mid={mid_offset} tail={tail_offset}"
        );
    }

    /// 相对图片地址解析(B 段验收的单测层):改写只发生在渲染字符串上,
    /// 源码与 `PreviewState` 一字不动 —— 相对路径落盘,文档目录整体搬走
    /// 仍有效(可移植验收)。
    #[test]
    fn relative_images_resolve_against_base_dir() {
        use std::borrow::Cow;
        let base = Path::new("/home/u/docs");
        // B 段自产形态:`./foo.assets/中文.png`
        let out = resolve_relative_images("![图](./foo.assets/中文.png)", Some(base));
        assert_eq!(out, "![图](file:///home/u/docs/foo.assets/中文.png)");
        // 上级目录与无 ./ 前缀
        let out = resolve_relative_images("![a](../img/x.png)", Some(base));
        assert_eq!(out, "![a](file:///home/u/docs/../img/x.png)");
        // `<…>` 包裹保持包裹
        let out = resolve_relative_images("![a](<./d/屏幕 截图.png>)", Some(base));
        assert_eq!(out, "![a](<file:///home/u/docs/d/屏幕 截图.png>)");
        // 多张图与周边文本逐字节保留
        let doc = "前文\n\n![一](a.png)中间![二](<b c.png>)\n\n后文";
        let out = resolve_relative_images(doc, Some(base));
        assert_eq!(
            out,
            "前文\n\n![一](file:///home/u/docs/a.png)中间![二](<file:///home/u/docs/b c.png>)\n\n后文"
        );
        // 没有相对图片时原样**借回**(零分配,vendored 缓存键不变)
        let clean = "![x](https://e.com/a.png) 与 [链](rel.md)";
        assert!(matches!(
            resolve_relative_images(clean, Some(base)),
            Cow::Borrowed(_)
        ));
        // 未落盘(无锚点)原样借回
        assert!(matches!(
            resolve_relative_images("![x](a.png)", None),
            Cow::Borrowed(_)
        ));
    }

    /// 不该改写的一律不动:绝对 URL、`wiki://`(wikilink 展开产物)、
    /// 绝对路径、锚点、空目标、围栏代码块内的 `![]()`、以及**链接**。
    #[test]
    fn non_relative_and_non_image_targets_stay_put() {
        let base = Path::new("/home/u/docs");
        let doc = "![a](https://e.com/x.png?w=1)\n![b](<wiki://架构决策>)\n\
                   ![c](/abs/path.png)\n![d](#anchor)\n![]()\n[e 链](./rel.md)\n\
                   ![f](data:image/png;base64,AAAA)\n![g](C:/win.png)\n\n\
                   ```rust\nlet x = ![](inner.png);\n```\n\n![h](ok.png)";
        let out = resolve_relative_images(doc, Some(base));
        let expected = doc.replace("![h](ok.png)", "![h](file:///home/u/docs/ok.png)");
        assert_eq!(out, expected);
    }

    /// B 段全链路(纯函数层):浏览复制产出的相对地址(含空格时包 `<>`)
    /// 能被本函数还原成绝对 URI —— 存盘文本与预览改写两侧的约定互相咬合。
    #[test]
    fn stored_url_round_trips_into_file_uri() {
        let base = Path::new("/home/u/docs");
        // assets::store 的产物形态(含空格包 <>):crate::assets 单测钉落盘,
        // 这里钉「这条地址进预览后能出图」
        let stored = "<./笔记.assets/屏幕 截图 (1).png>";
        let doc = format!("![alt]({stored})");
        let out = resolve_relative_images(&doc, Some(base));
        assert_eq!(
            out,
            "![alt](<file:///home/u/docs/笔记.assets/屏幕 截图 (1).png>)"
        );
    }

    /// 带标题的内联图片:`![a](./x.png "标题")` 目标到空白即止,标题保留。
    #[test]
    fn titled_image_keeps_its_title() {
        let base = Path::new("/d");
        let out = resolve_relative_images("![a](./x.png \"题\")", Some(base));
        assert_eq!(out, "![a](file:///d/x.png \"题\")");
    }

    /// B 段验收「预览出图」的无头全链路:真 PNG 落盘 → 生产渲染入口
    /// [`ui`](相对地址在这里被 [`resolve_relative_images`] 拼成 file://)→
    /// vendored `Token::Image` 分支(images feature)→ egui loader 装上后
    /// 真的画出**图片纹理**。
    ///
    /// 断言依据:加载完成的图片画成**带纹理的 RectShape**(纯色矩形/文字/
    /// spinner/⚠ 的纹理恒为默认值),出现即出图;加载在 loader 后台线程,
    /// 轮询有限帧直到它出现。
    #[test]
    fn local_file_image_paints_through_loaders() {
        let dir = std::env::temp_dir().join(format!("latermd-preview-img-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("foo.assets")).unwrap();
        let png = dir.join("foo.assets/dot.png");
        image::RgbImage::from_pixel(2, 2, image::Rgb([200, 40, 40]))
            .save(&png)
            .unwrap();

        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);

        let textured_rects = |output: &eframe::egui::FullOutput| -> usize {
            fn collect(shape: &egui::epaint::Shape, out: &mut usize) {
                match shape {
                    egui::epaint::Shape::Rect(rect) => {
                        // 0.36 起矩形纹理走 `brush`(纯色矩形/文字/spinner/⚠
                        // 均为 None),带 brush 的矩形即图片
                        if rect.brush.is_some() {
                            *out += 1;
                        }
                    }
                    egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
                    _ => {}
                }
            }
            let mut out = 0;
            for clipped in &output.shapes {
                collect(&clipped.shape, &mut out);
            }
            out
        };
        let render = |doc: &str| {
            let mut preview = PreviewState {
                rendered: doc.to_owned(),
                offset_map: latermd_md::OffsetMap::empty(),
                highlight_map: latermd_md::OffsetMap::empty(),
                emoji_map: latermd_md::OffsetMap::empty(),
                task_map: latermd_md::OffsetMap::empty(),
                text: doc.to_owned(),
                synced_rev: 0,
                outline: Vec::new(),
                scroll_target: None,
            };
            let mut outbox = Vec::new();
            ctx.run_ui(RawInput::default(), |panel| {
                ui(
                    panel,
                    &mut preview,
                    &AiState::default(),
                    1,
                    false,
                    Some(dir.as_path()),
                    &mut outbox,
                );
            })
        };

        // 基线:无图文档出不了带纹理的矩形(防御断言),浮窗/滚动区首帧
        // 只完成注册,与其它无头测试同节奏跑满三帧。
        for _ in 0..3 {
            let output = render("# 基线\n\n正文,没有图片。");
            assert_eq!(textured_rects(&output), 0, "无图文档不应有图片纹理");
            output.drop_without_applying_deltas();
        }

        let mut painted = false;
        for _ in 0..200 {
            let output = render("![红点](./foo.assets/dot.png)");
            let textured = textured_rects(&output);
            output.drop_without_applying_deltas();
            if textured > 0 {
                painted = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(painted, "相对路径图片应经 file:// 画出图片纹理(出图)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 卡片 widget id 的稳定性(AGENTS.md §6.7 的证据):id 由「块在文档中的
    /// 序号」构成 —— 编辑指令文本本身、或在卡片后增删内容,序号与 id 都不变;
    /// 多张卡按文档序得到互异 id。id 里绝不含内容长度,否则每次编辑都清空
    /// vendored 层缓存。
    #[test]
    fn card_id_is_index_based_and_stable_across_edits() {
        let ctx = egui::Context::default();

        // 与生产 preview::ui 同配置(MarkdownLabel + heal + handler)渲染一帧,
        // 取回 handler 记录的卡片 id。省掉 ScrollArea 外壳不影响结论:
        // push_id 是相对父 ui 的,稳定性断言看的是相对成分。
        let render = |doc: &str| {
            let handler = AiLinkHandler::new(
                ai_link_color(true),
                true,
                egui::Color32::WHITE,
                &AiState::default(),
            );
            ctx.run_ui(RawInput::default(), |ui| {
                MarkdownLabel::new(egui::Id::new("preview-md"), doc)
                    .wrap()
                    .heal(true)
                    .link_handler(&handler)
                    .show(ui);
            })
            .drop_without_applying_deltas();
            let ids = handler.card_ids.borrow().clone();
            ids
        };

        let doc_a = "# 标题\n\n```ai\nalpha\n```\n";
        let ids_a = render(doc_a);
        assert_eq!(ids_a.len(), 1, "一张指令卡");

        // 编辑指令文本(长度变了)+ 卡片后追加内容:序号仍是 0,id 不变
        let doc_b = "# 标题\n\n```ai\nalpha beta —— 一段长了很多的指令文本\n```\n\n后续段落。\n";
        let ids_b = render(doc_b);
        assert_eq!(ids_b, ids_a, "id 只由文档序构成,不含内容");

        // 两张卡:文档序互异 id
        let doc_c = "```ai\nfirst\n```\n\n```ai\nsecond\n```\n";
        let ids_c = render(doc_c);
        assert_eq!(ids_c.len(), 2);
        assert_ne!(ids_c[0], ids_c[1]);
    }

    /// #51 M3:mermaid 块经预览 handler(`AiLinkHandler` 扩展)分派到
    /// mermaid widget 出图;与 ```ai 卡混排时两套块序号各自独立计数
    /// (交错出现不牵连),回落块照常走源码路径。
    #[test]
    fn mermaid_blocks_dispatch_via_preview_handler_with_independent_indexing() {
        let ctx = egui::Context::default();
        let render = |doc: &str| {
            let handler = AiLinkHandler::new(
                ai_link_color(true),
                true,
                egui::Color32::WHITE,
                &AiState::default(),
            );
            ctx.run_ui(RawInput::default(), |ui| {
                MarkdownLabel::new(egui::Id::new("preview-md"), doc)
                    .wrap()
                    .link_handler(&handler)
                    .show(ui);
            })
            .drop_without_applying_deltas();
            let captured = (
                handler.card_ids.borrow().clone(),
                crate::ui::mermaid::read_probe(&ctx),
            );
            captured
        };

        // 合法图 + 卡混排:卡 2 张按文档序取 id;mermaid 探针 2 块出图,
        // widget id(块序号成分)互异且与卡片 id 无关。
        let mixed = concat!(
            "```ai\nfirst\n```\n\n",
            "```mermaid\nflowchart TD\nA --> B\n```\n\n",
            "```mermaid\nflowchart TD\nC --> D\n```\n\n",
            "```ai\nsecond\n```\n",
        );
        let (card_ids, probe) = render(mixed);
        assert_eq!(card_ids.len(), 2, "两张指令卡");
        assert_eq!(probe.len(), 2, "两个 mermaid 块探针");
        assert!(probe.iter().all(|b| b.rendered), "两个合法图都应出图");
        assert_ne!(probe[0].widget_id, probe[1].widget_id, "块序号互异 id");
        assert_eq!(probe[0].nodes.len(), 2);

        // 不支持类型(sequenceDiagram):同一 handler 下走回落,不出节点。
        let (card_ids, probe) =
            render("```mermaid\nsequenceDiagram\nA->>B: hi\n```\n\n```ai\nonly\n```\n");
        assert_eq!(card_ids.len(), 1, "指令卡照常渲染");
        assert_eq!(probe.len(), 1);
        assert!(!probe[0].rendered, "sequenceDiagram 应回落源码");
        assert!(probe[0].nodes.is_empty(), "回落态不出节点盒");
    }

    /// #39 M2 缓存正确性(编辑后缓存失效):同一 tab 的缓存槽位按文本 hash
    /// 键控,内容变了 hash 变 → miss 重建,预览第一帧就必须反映新文本 ——
    /// 缓存绝不允许吞掉编辑。撤销回旧文本(hash 回到旧值,命中旧产物)对
    /// 同一文本产物必然正确,同样立即显示。heal 开关切换(流式结束,调用方
    /// 由 heal(true) 转 heal(false))不改变完整文档的渲染结果。
    #[test]
    fn preview_reflects_edits_immediately_despite_per_tab_cache() {
        let ctx = egui::Context::default();
        // 生产入口 preview::ui(同一 tab id 反复渲染,模拟切换往返中的
        // 编辑/撤销);收 painted 文本做内容断言。
        let render = |rendered: &str, heal: bool| -> Vec<String> {
            let mut preview = PreviewState {
                rendered: rendered.to_owned(),
                offset_map: latermd_md::OffsetMap::empty(),
                highlight_map: latermd_md::OffsetMap::empty(),
                emoji_map: latermd_md::OffsetMap::empty(),
                task_map: latermd_md::OffsetMap::empty(),
                text: rendered.to_owned(),
                synced_rev: 0,
                outline: Vec::new(),
                scroll_target: None,
            };
            let mut outbox = Vec::new();
            let output = ctx.run_ui(RawInput::default(), |panel| {
                ui(
                    panel,
                    &mut preview,
                    &AiState::default(),
                    7,
                    heal,
                    None,
                    &mut outbox,
                );
            });
            let painted = painted_text(&output);
            output.drop_without_applying_deltas();
            painted
        };

        let v1 = "# 版本一\n\n原始段落 unique-v1-marker。\n";
        let v2 = "# 版本二\n\n编辑后的段落 unique-v2-marker。\n";

        let painted_v1 = render(v1, false);
        assert!(painted_v1.iter().any(|t| t.contains("unique-v1-marker")));

        // 编辑(v1 → v2):新文本第一帧就画出来,旧内容不得残留
        let painted_v2 = render(v2, false);
        assert!(
            painted_v2.iter().any(|t| t.contains("unique-v2-marker")),
            "编辑后预览必须立即反映新文本(缓存吞掉了编辑)"
        );
        assert!(
            !painted_v2.iter().any(|t| t.contains("unique-v1-marker")),
            "编辑后旧内容不得因缓存残留"
        );

        // 撤销(v2 → v1):文本回到旧值,立即显示 v1
        let painted_undo = render(v1, false);
        assert!(
            painted_undo.iter().any(|t| t.contains("unique-v1-marker")),
            "撤销后预览必须回到 v1"
        );

        // heal 开关切换(流式结束帧)不改变完整文档渲染:同一文本 heal
        // 是恒等变换,两次渲染的 painted 文本必须逐条一致。
        let painted_healed = render(v1, true);
        assert_eq!(
            painted_healed, painted_undo,
            "heal 开关不得改变完整文档渲染"
        );
    }

    // —— 代码块复制头(#38)——

    /// 覆盖 #38 验收面的文档:带语言块(CJK + emoji 行)、裸围栏、带空行的
    /// 空围栏(text 为空串但块存在)、```ai 指令卡、末尾短块(验「无尾换行」
    /// 边界)。另含一个无空行的空围栏 —— pulldown 对它不产 Text 事件,
    /// vendored parser 也就没有 token,整块不渲染(上游既有语义,只验不
    /// panic,不指望按钮)。
    fn code_copy_doc() -> String {
        let mut doc = String::from("前文段落。\n\n");
        doc.push_str("```zzprobe\nfn hello() {\n    println!(\"你好,世界 🌏\");\n}\n```\n\n");
        doc.push_str("```\n裸围栏,没有语言。\n```\n\n");
        doc.push_str("```\n\n```\n\n");
        doc.push_str("```\n```\n\n");
        doc.push_str("```ai\n指令不走这里\n```\n\n");
        doc.push_str("```rust\ntail_line()\n```\n");
        doc
    }

    /// 以生产入口渲染一帧,返回 (探针按钮 rect, 帧输出收集的 Text shape 文本,
    /// 本帧 CopyText 命令载荷)。
    fn render_copy_frame(
        ctx: &egui::Context,
        doc: &str,
        events: Vec<egui::Event>,
    ) -> (Vec<egui::Rect>, Vec<String>, Vec<String>) {
        let (rendered, offset_map) = latermd_md::expand_wikilinks_with_map(doc);
        let mut preview = PreviewState {
            rendered,
            offset_map,
            highlight_map: latermd_md::OffsetMap::empty(),
            emoji_map: latermd_md::OffsetMap::empty(),
            task_map: latermd_md::OffsetMap::empty(),
            text: doc.to_owned(),
            synced_rev: 0,
            outline: Vec::new(),
            scroll_target: None,
        };
        let mut outbox = Vec::new();
        let output = ctx.run_ui(
            eframe::egui::RawInput {
                events,
                ..Default::default()
            },
            |panel| {
                ui(
                    panel,
                    &mut preview,
                    &AiState::default(),
                    1,
                    false,
                    None,
                    &mut outbox,
                );
            },
        );
        let texts = painted_text(&output);
        let copied = copied_texts(&output);
        // 帧后读取:帧号已前进,按「最后写入者即本帧」取原始探针。
        let rects = copy_button_probe(ctx).1;
        output.drop_without_applying_deltas();
        (rects, texts, copied)
    }

    /// 按指针三帧(移入/按下/抬起)点击 `target`,汇集整个序列的 CopyText 载荷。
    fn click_and_collect(ctx: &egui::Context, doc: &str, target: egui::Pos2) -> Vec<String> {
        let mut copied = Vec::new();
        for events in click_events(target) {
            let (_, _, frame) = render_copy_frame(ctx, doc, events);
            copied.extend(frame);
        }
        copied
    }

    /// 指针序列:移入 → 按下 → 抬起(与 ai_key 的无头点击同款三帧)。
    fn click_events(pos: egui::Pos2) -> Vec<Vec<egui::Event>> {
        let click = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        vec![
            vec![egui::Event::PointerMoved(pos)],
            vec![click(pos, true)],
            vec![click(pos, false)],
        ]
    }

    /// 从帧输出里抽出全部 CopyText 命令的载荷。
    fn copied_texts(output: &eframe::egui::FullOutput) -> Vec<String> {
        output
            .platform_output
            .commands
            .iter()
            .filter_map(|cmd| match cmd {
                eframe::egui::OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// 按钮存在性与命中:每个**普通**代码块一枚按钮(裸围栏/空块也有,
    /// ```ai 指令卡走 block_code_widget 另一条路不经过挂载点);点击后
    /// `Context::copy_text` 出整块源文本 —— CJK/emoji 行逐字节保留、
    /// 末块无尾换行、空块复制空串。渲染不 panic、点击不产消息。
    #[test]
    fn code_block_copy_buttons_render_per_block_and_copy_on_click() {
        let ctx = egui::Context::default();
        let doc = code_copy_doc();

        let (rects, _, _) = render_copy_frame(&ctx, &doc, Vec::new());
        assert_eq!(
            rects.len(),
            4,
            "zzprobe/裸/空/末尾 rust 四个普通块各一枚按钮,ai 卡不算:{rects:?}"
        );

        // 点击 zzprobe 块:复制内容与块源文本逐字符相等(CJK + emoji +
        // 内部缩进换行原样),点击不产消息。
        assert_eq!(
            click_and_collect(&ctx, &doc, rects[0].center()),
            vec!["fn hello() {\n    println!(\"你好,世界 🌏\");\n}".to_owned()],
            "点击恰复制一次,内容与块源文本逐字符相等"
        );

        // 末块边界:无尾换行(vendored parser 的 trim_end_newlines)。
        let (rects, _, _) = render_copy_frame(&ctx, &doc, Vec::new());
        assert_eq!(
            click_and_collect(&ctx, &doc, rects[3].center()),
            vec!["tail_line()".to_owned()],
            "末块复制不带尾换行"
        );

        // 空块:按钮在,点击复制空串(不 panic)。
        let (rects, _, _) = render_copy_frame(&ctx, &doc, Vec::new());
        assert_eq!(
            click_and_collect(&ctx, &doc, rects[2].center()),
            vec![String::new()],
            "空块复制空串"
        );
    }

    /// 瞬时反馈:点击帧写入反馈态,下一帧按钮图标换成 ✓(Check 的两条
    /// 斜率异号线段落在按钮 rect 内;Copy 后框的边线是水平/垂直,不混判)。
    /// 再渲染一帧按钮仍在(连续帧稳定,源码/Live 互切共用本入口的底座)。
    #[test]
    fn copy_click_flashes_check_feedback_next_frame() {
        let ctx = egui::Context::default();
        let doc = code_copy_doc();

        let (rects, _, _) = render_copy_frame(&ctx, &doc, Vec::new());
        let target = rects[0].center();
        for events in click_events(target) {
            let _ = render_copy_frame(&ctx, &doc, events);
        }

        // 点击后的第一帧:按钮 rect 内出现一正一负两条非零斜率线段(Check)。
        let output = ctx.run_ui(egui::RawInput::default(), |panel| {
            let (rendered, offset_map) = latermd_md::expand_wikilinks_with_map(&doc);
            let mut preview = PreviewState {
                rendered,
                offset_map,
                highlight_map: latermd_md::OffsetMap::empty(),
                emoji_map: latermd_md::OffsetMap::empty(),
                task_map: latermd_md::OffsetMap::empty(),
                text: doc.clone(),
                synced_rev: 0,
                outline: Vec::new(),
                scroll_target: None,
            };
            let mut outbox = Vec::new();
            ui(
                panel,
                &mut preview,
                &AiState::default(),
                1,
                false,
                None,
                &mut outbox,
            );
        });
        let rects = copy_button_probe(&ctx).1;
        assert_eq!(rects.len(), 4, "反馈帧按钮仍在");
        let button = rects[0];
        let mut slopes = Vec::new();
        for clipped in &output.shapes {
            if let egui::epaint::Shape::LineSegment { points, .. } = &clipped.shape {
                let inside = points.iter().all(|p| button.contains(*p));
                if inside {
                    slopes.push((points[1].x - points[0].x) * (points[1].y - points[0].y));
                }
            }
        }
        assert!(
            slopes.iter().any(|s| *s > 0.0) && slopes.iter().any(|s| *s < 0.0),
            "✓ 反馈 = 两条斜率异号线段落在按钮内:{slopes:?}"
        );
        output.drop_without_applying_deltas();
    }

    /// 快照护栏(照 #32 图标护栏手法):复制按钮与语言标签不得回流正文
    /// 文本 —— 语言标签 `zzprobe` 只允许出现在独立的头部 Text shape,
    /// 含代码正文的 galley 不得混入语言标签;图标字形(`⧉`/`✓`)与
    /// hover 文案不得以文本形态出现(无 hover 帧本就不该有 tooltip)。
    #[test]
    fn code_block_header_stays_out_of_body_text() {
        let ctx = egui::Context::default();
        let doc = code_copy_doc();
        let (_, texts, _) = render_copy_frame(&ctx, &doc, Vec::new());

        let body_shapes: Vec<&String> = texts
            .iter()
            .filter(|t| t.contains("fn hello()") || t.contains("裸围栏,没有语言。"))
            .collect();
        assert!(!body_shapes.is_empty(), "代码正文应仍在文本层:{texts:?}");
        assert!(
            body_shapes.iter().all(|body| !body.contains("zzprobe")),
            "语言标签不得回流正文 galley:{body_shapes:?}"
        );
        assert!(
            body_shapes
                .iter()
                .any(|body| body.contains("裸围栏,没有语言。")),
            "裸围栏块的正文原样在 galley:{body_shapes:?}"
        );
        let label_shapes = texts.iter().filter(|t| t.as_str() == "zzprobe").count();
        assert_eq!(label_shapes, 1, "语言标签恰一枚独立 Text shape:{texts:?}");
        for text in &texts {
            assert!(
                !text.contains('⧉') && !text.contains('✓') && !text.contains("复制代码"),
                "图标字形/hover 文案不得以文本形态出现:{text:?}"
            );
        }
    }

    /// ```ai 指令卡回归:挂上复制头之后,指令块仍走卡片(标题/指令/执行
    /// 按钮三要素齐全),代码块复制按钮不叠上卡片。
    #[test]
    fn ai_instruction_card_unaffected_by_copy_header() {
        let ctx = egui::Context::default();
        let doc = code_copy_doc();
        let (rects, texts, _) = render_copy_frame(&ctx, &doc, Vec::new());

        assert_eq!(rects.len(), 4, "指令卡不产复制按钮");
        for expected in ["AI 指令", "指令不走这里", "执行"] {
            assert!(
                texts.iter().any(|t| t.contains(expected)),
                "卡片要素 {expected} 缺失:{texts:?}"
            );
        }
    }

    // —— 任务列表 checkbox(#63)——

    /// 覆盖 #63 验收面的文档:顶层未勾/已勾、嵌套缩进任务、空任务(纯
    /// `- [ ]` 无文字)、CJK 任务文本、代码块与行内代码里的 `[ ]`(豁免
    /// 面)、有序任务。
    fn task_doc() -> String {
        let mut doc = String::from("# 任务清单\n\n");
        doc.push_str("- [ ] 首项待办\n- [x] 已完成项\n\n");
        doc.push_str("  - [ ] 嵌套深层任务\n\n");
        doc.push_str("- [ ]\n\n");
        doc.push_str("```rust\nlet a = [ ];\nlet b = [x];\n```\n\n");
        doc.push_str("行内 `[ ]` 代码豁免\n\n");
        doc.push_str("1. [ ] 有序任务\n");
        doc
    }

    /// 以生产入口渲染一帧(快照走 `PreviewState::new` 的完整四层链),
    /// 返回 (本帧 checkbox 探针 rect, 产生的消息, 画出的文本)。
    fn render_task_frame(
        ctx: &egui::Context,
        doc: &str,
        events: Vec<egui::Event>,
    ) -> (Vec<egui::Rect>, Vec<Message>, Vec<String>) {
        let mut preview = PreviewState::new(&latermd_editor::EditorBuffer::new(doc));
        let mut outbox = Vec::new();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let output = ctx.run_ui(
            eframe::egui::RawInput {
                events,
                screen_rect: Some(screen),
                ..Default::default()
            },
            |panel| {
                ui(
                    panel,
                    &mut preview,
                    &AiState::default(),
                    1,
                    false,
                    None,
                    &mut outbox,
                );
            },
        );
        let texts = painted_text(&output);
        output.drop_without_applying_deltas();
        // 帧后读取:帧号已前进,按「最后写入者即本帧」取原始探针。
        (task_checkbox_probe(ctx), outbox, texts)
    }

    /// 按指针三帧点击 `target`,汇集整个序列的消息(点击在抬起帧产出)。
    fn click_task_and_collect(ctx: &egui::Context, doc: &str, target: egui::Pos2) -> Vec<Message> {
        let mut messages = Vec::new();
        for events in click_events(target) {
            let (_, mut frame, _) = render_task_frame(ctx, doc, events);
            messages.append(&mut frame);
        }
        messages
    }

    /// 存在与命中:每个任务一枚 checkbox(嵌套/空任务/有序都算),点击
    /// 命中出 [`Message::TaskCheckboxToggled`] 且载荷指向**源码**里该标记
    /// 的 `[`(三层映射逆穿后);代码块/行内代码的 `[ ]` 不产 checkbox。
    /// 悬停不切换(只有抬起帧的消息算数)。
    #[test]
    fn task_checkboxes_click_through_to_source_offsets() {
        let ctx = egui::Context::default();
        let doc = task_doc();
        let (rects, messages, _) = render_task_frame(&ctx, &doc, Vec::new());
        // 六个任务:首项/已完成/嵌套/空/有序 = 5 个(代码块 2 个 + 行内 1
        // 个豁免)。首帧无点击 → 零消息。
        assert_eq!(rects.len(), 5, "豁免面不产 checkbox:{rects:?}");
        assert!(messages.is_empty(), "渲染帧不产消息");

        // 逐个点击每个 checkbox:载荷必须是「源码里某个 `[ ]`/`[x]` 标记
        // 的 `[` 字节偏移」,且五个互不相同(嵌套列表打到正确的行)。
        let source_offsets: Vec<usize> =
            ["[ ] 首项", "[x] 已完成", "[ ] 嵌套", "[ ]\n", "[ ] 有序"]
                .iter()
                .map(|needle| doc.find(needle).expect("needle"))
                .collect();
        let mut seen = std::collections::HashSet::new();
        for rect in &rects {
            let clicked = click_task_and_collect(&ctx, &doc, rect.center());
            let toggles: Vec<_> = clicked
                .iter()
                .filter_map(|message| match message {
                    Message::TaskCheckboxToggled { byte } => Some(*byte),
                    _ => None,
                })
                .collect();
            assert_eq!(toggles.len(), 1, "一次点击恰一条切换消息:{toggles:?}");
            assert!(
                source_offsets.contains(&toggles[0]),
                "载荷 {}/{} 须指向源码任务标记的 [:{:?}",
                toggles[0],
                toggles[0] == 0,
                doc.get(toggles[0]..toggles[0] + 3)
            );
            assert!(
                matches!(&doc[toggles[0]..toggles[0] + 3], "[ ]" | "[x]" | "[X]"),
                "载荷指向的须是任务标记三字符"
            );
            seen.insert(toggles[0]);
        }
        assert_eq!(seen.len(), 5, "五个 checkbox 各指其行(嵌套不串行)");
    }

    /// 端到端切换 + 反向再点:点击 → 消息 → 归约 → 源码翻转为 `[x]`;
    /// 重建快照后 checkbox 数不变、再点同处载荷仍指向同一标记,归约翻回。
    /// 切换后预览即时反映(快照重建后 `rendered` 里勾选态翻转)。
    #[test]
    fn task_click_toggles_source_and_back_with_fresh_preview() {
        let ctx = egui::Context::default();
        let mut state = crate::state::State::default();
        {
            let tab = state.tabs.current_mut();
            tab.editor.replace_all("- [ ] 首项\n");
            tab.preview.rebuild(&tab.editor);
        }
        let doc_before = state.tabs.current().editor.text().to_owned();

        let (rects, _, _) = render_task_frame(&ctx, &doc_before, Vec::new());
        assert_eq!(rects.len(), 1);
        let messages = click_task_and_collect(&ctx, &doc_before, rects[0].center());
        let byte = match &messages[0] {
            Message::TaskCheckboxToggled { byte } => *byte,
            other => panic!("应为切换消息:{other:?}"),
        };
        state.apply(messages[0].clone());
        assert_eq!(state.tabs.current().editor.text(), "- [x] 首项\n");
        // 快照重建(修订号前进后生产路径自动做):勾选态即时反映。
        {
            let tab = state.tabs.current_mut();
            tab.preview.rebuild(&tab.editor);
        }
        let rendered = state.tabs.current().preview.rendered.clone();
        assert!(rendered.contains("task://c"), "重建后勾选态 c 载荷");

        // 反向再点:改写后 rect 位置可能微移,重新探针取新位置。
        let doc_after = state.tabs.current().editor.text().to_owned();
        let (rects2, messages2, _) = render_task_frame(&ctx, &doc_after, Vec::new());
        assert_eq!(rects2.len(), 1, "取消勾选后 checkbox 仍在");
        assert!(messages2.is_empty());
        let back = click_task_and_collect(&ctx, &doc_after, rects2[0].center());
        match &back[0] {
            Message::TaskCheckboxToggled { byte: b } => assert_eq!(*b, byte, "同一标记"),
            other => panic!("应为切换消息:{other:?}"),
        }
        state.apply(back[0].clone());
        assert_eq!(state.tabs.current().editor.text(), "- [ ] 首项\n");
    }

    /// 非任务文档零元素(否决线):纯文本/代码块/链接文档探针零 rect、
    /// 点击任意位置零消息。
    #[test]
    fn task_probe_empty_on_non_task_documents() {
        let ctx = egui::Context::default();
        for doc in [
            "# 标题\n\n正文段落,普通 [ ] 方括号\n",
            "```rust\nlet a = [ ];\n```\n",
            "链接 [文字](https://a.com) 与 [[wikilink]]\n",
        ] {
            let (rects, messages, _) = render_task_frame(&ctx, doc, Vec::new());
            assert!(rects.is_empty(), "非任务文档零 checkbox:{doc:?}");
            assert!(messages.is_empty());
        }
    }

    /// 两主题渲染不 panic + checkbox 形态在场(dark/light 各一帧,探针
    /// 数量一致;勾选态对勾与未勾空框都画得出)。
    #[test]
    fn task_checkboxes_render_in_both_themes_without_panic() {
        let ctx = egui::Context::default();
        let doc = task_doc();
        for dark in [true, false] {
            ctx.all_styles_mut(|style| {
                style.visuals = if dark {
                    egui::Visuals::dark()
                } else {
                    egui::Visuals::light()
                }
            });
            let (rects, messages, _) = render_task_frame(&ctx, &doc, Vec::new());
            assert_eq!(rects.len(), 5, "主题不改 checkbox 数量");
            assert!(messages.is_empty());
        }
    }

    /// 点击 checkbox 不抢编辑器焦点:同帧先渲染一个持焦 TextEdit,再渲染
    /// 预览并点击 checkbox —— 编辑器焦点原样保持(#61 焦点纪律的预览侧
    /// 对偶:预览面板本身无可聚焦件,点击不得惊动左栏)。
    #[test]
    fn task_click_keeps_editor_focus() {
        let ctx = egui::Context::default();
        let doc = "- [ ] 任务\n";
        let editor_id = egui::Id::new("focus-editor");
        let mut text = String::new();
        // 每帧从同一文档重建快照(生产里修订号不变即缓存,测试重建等价)。
        let render = |events: Vec<egui::Event>, text: &mut String| -> Option<egui::Rect> {
            let mut preview = PreviewState::new(&latermd_editor::EditorBuffer::new(doc));
            let mut outbox = Vec::new();
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
            let output = ctx.run_ui(
                eframe::egui::RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |panel| {
                    egui::TextEdit::multiline(text).id(editor_id).show(panel);
                    panel.ctx().memory_mut(|mem| mem.request_focus(editor_id));
                    ui(
                        panel,
                        &mut preview,
                        &AiState::default(),
                        1,
                        false,
                        None,
                        &mut outbox,
                    );
                },
            );
            output.drop_without_applying_deltas();
            task_checkbox_probe(&ctx).first().copied()
        };
        let rect = render(Vec::new(), &mut text).expect("checkbox 在场");
        assert!(
            ctx.memory(|mem| mem.has_focus(editor_id)),
            "前置:编辑器持焦"
        );
        let mut clicked_messages = Vec::new();
        for events in click_events(rect.center()) {
            // 三帧逐步喂:焦点断言在点击抬起帧之后(消息从 outbox 收集)。
            let mut preview = PreviewState::new(&latermd_editor::EditorBuffer::new(doc));
            let mut outbox = Vec::new();
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
            let output = ctx.run_ui(
                eframe::egui::RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |panel| {
                    egui::TextEdit::multiline(&mut text)
                        .id(editor_id)
                        .show(panel);
                    ui(
                        panel,
                        &mut preview,
                        &AiState::default(),
                        1,
                        false,
                        None,
                        &mut outbox,
                    );
                },
            );
            output.drop_without_applying_deltas();
            clicked_messages.extend(outbox);
        }
        assert_eq!(clicked_messages.len(), 1, "点击产出切换消息");
        // 焦点归还排在下一帧生效(request_focus 的语义):再渲染一帧,
        // 编辑器应重新持焦 —— 点击 checkbox 的意图是切换,不是转移焦点。
        {
            let mut preview = PreviewState::new(&latermd_editor::EditorBuffer::new(doc));
            let mut outbox = Vec::new();
            let output = ctx.run_ui(egui::RawInput::default(), |panel| {
                egui::TextEdit::multiline(&mut text)
                    .id(editor_id)
                    .show(panel);
                ui(
                    panel,
                    &mut preview,
                    &AiState::default(),
                    1,
                    false,
                    None,
                    &mut outbox,
                );
            });
            output.drop_without_applying_deltas();
        }
        assert!(
            ctx.memory(|mem| mem.has_focus(editor_id)),
            "点击 checkbox 不抢编辑器焦点"
        );
    }

    // —— ==高亮== 渲染(#65 M2)——

    /// WCAG 相对亮度对比度(#122 同族口径),测试本地实现,不引依赖。
    fn contrast_ratio(fg: egui::Color32, bg: egui::Color32) -> f32 {
        fn channel(c: u8) -> f32 {
            let v = c as f32 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        fn luminance(c: egui::Color32) -> f32 {
            0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
        }
        let (l1, l2) = (luminance(fg), luminance(bg));
        let (hi, lo) = (l1.max(l2), l1.min(l2));
        (hi + 0.05) / (lo + 0.05)
    }

    /// 底色 token:一条推导式出明暗两档(常量只有锚黄与比例),输入是
    /// **出厂皮肤投影**(`theme.rs apply_shell_to`:extreme_bg = code_bg
    /// token,正文 = shell text token)—— 出厂值与 HTML 导出内嵌 CSS
    /// 同源(#f8f1ad / #655e1b),两主题正文色压底色均 ≥ 4.5:1(#122
    /// 同族对比度下限)。
    #[test]
    fn highlight_bg_token_two_themes_and_contrast_floor() {
        for (dark, expected, label) in [
            (true, egui::Color32::from_rgb(101, 94, 27), "暗档 ≈#655e1b"),
            (
                false,
                egui::Color32::from_rgb(248, 241, 173),
                "明档 ≈#f8f1ad",
            ),
        ] {
            let tokens = crate::theme::shell_tokens(dark);
            let mut visuals = if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            };
            // 与 apply_shell_to 同投影:高亮底色的生产输入是 code_bg
            visuals.extreme_bg_color = tokens.code_bg;
            let bg = highlight_bg_color(&visuals);
            let expected = if cfg!(target_os = "macos") {
                if dark {
                    egui::Color32::from_rgb(105, 97, 31)
                } else {
                    egui::Color32::from_rgb(248, 241, 173)
                }
            } else {
                expected
            };
            assert_eq!(bg, expected, "{label}");
            let ratio = contrast_ratio(tokens.text, bg);
            assert!(
                ratio >= 4.5,
                "{}主题高亮对比度 {ratio:.2} < 4.5(WCAG AA 正文线)",
                if dark { "暗" } else { "明" }
            );
        }
    }

    /// handler 层收口:`hl://` 接管排版段(底色 = 共享推导、正文色、行高
    /// 与 global_style 推导一致、底对齐、外扩常数),非 `hl://` 不接管;
    /// link_style 正文色无下划线;click 吞掉且不产消息。
    #[test]
    fn highlight_layout_link_paints_background_section() {
        let ctx = egui::Context::default();
        let body = egui::Color32::from_rgb(0x11, 0x22, 0x33);
        let handler = AiLinkHandler::new(ai_link_color(true), true, body, &AiState::default());
        let output = ctx.run_ui(RawInput::default(), |ui| {
            let font = egui::FontId::proportional(15.0);
            let mut job = egui::text::LayoutJob::default();
            assert!(
                handler.layout_link(
                    ui,
                    "高亮内容",
                    "hl://",
                    &mut job,
                    &font,
                    egui::Color32::TRANSPARENT
                ),
                "hl:// 接管排版段"
            );
            assert_eq!(job.sections.len(), 1, "恰一段");
            let format = &job.sections[0].format;
            assert_eq!(
                format.background,
                highlight_bg_color(ui.visuals()),
                "底色 = 共享推导 token"
            );
            assert_eq!(
                format.color,
                ui.visuals().text_color(),
                "正文色,不吃超链接色"
            );
            assert_eq!(format.valign, egui::Align::BOTTOM, "与正文段同底对齐");
            assert_eq!(format.expand_bg, HIGHLIGHT_BG_EXPAND, "底色外扩常数");
            let style = egui_markdown::global_style(ui.ctx());
            assert_eq!(
                format.line_height,
                Some(
                    (15.0 * style.line_height_ratio)
                        .max(15.0 * style.min_line_height_em + LINE_HEIGHT_FLOOR_SLACK_PX)
                ),
                "行高 = vendored line_height_for 同式"
            );

            let mut other = egui::text::LayoutJob::default();
            assert!(
                !handler.layout_link(
                    ui,
                    "文本",
                    "https://example.com",
                    &mut other,
                    &font,
                    egui::Color32::TRANSPARENT
                ),
                "非 hl:// 不接管,走 vendored 默认"
            );
            let link_style = handler.link_style("hl://").expect("hl:// 有样式");
            assert_eq!(link_style.color, Some(body), "正文色意图");
            assert!(!link_style.underline, "无下划线意图");
            assert!(handler.click("x", "hl://", ui), "点击吞掉,不开浏览器");
        });
        output.drop_without_applying_deltas();
        let mut outbox = Vec::new();
        handler.drain_into(&mut outbox);
        assert!(outbox.is_empty(), "吞掉的高亮点击不产消息:{outbox:?}");
    }

    /// vendored 端到端:M1 改写产物进 `build_layout`,高亮段落成带底色的
    /// section,行高与相邻正文 section 相等(行高公式复制的钉子);否决线
    /// —— 无 `==` 的文档任何 section 都不吃推导底色(行内代码有自己的
    /// 底色,值不同,天然区分)。
    #[test]
    fn highlight_section_matches_plain_text_metrics_and_veto_line() {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(RawInput::default(), |ui| {
            let handler = AiLinkHandler::new(
                ai_link_color(ui.visuals().dark_mode),
                ui.visuals().dark_mode,
                ui.visuals().text_color(),
                &AiState::default(),
            );
            let style = egui_markdown::global_style(ui.ctx());
            let font = egui::FontId::proportional(15.0);
            let doc = egui_markdown::parse("前文 [高亮](<hl://>) 后文\n");
            let layout = egui_markdown::layout::build_layout(
                ui,
                &doc.tokens,
                font,
                ui.visuals().text_color(),
                None,
                400.0,
                false,
                Some(&handler),
                false,
                false,
                true,
                &style,
                None,
            );
            let hl = highlight_bg_color(ui.visuals());
            let hits: Vec<&egui::text::LayoutSection> = layout
                .job
                .sections
                .iter()
                .filter(|section| section.format.background == hl)
                .collect();
            assert_eq!(hits.len(), 1, "恰一段带高亮底色");
            assert_eq!(
                hits[0].byte_range.slice(&layout.job.text),
                "高亮",
                "底色段正是高亮内容"
            );
            let plain = layout
                .job
                .sections
                .iter()
                .find(|section| {
                    section.format.background == egui::Color32::TRANSPARENT
                        && !section.byte_range.slice(&layout.job.text).trim().is_empty()
                })
                .expect("正文段在");
            assert_eq!(
                plain.format.line_height, hits[0].format.line_height,
                "高亮段行高与相邻正文段相等(公式复制钉住)"
            );

            // 否决线:无 == 文档零底色段
            let doc2 = egui_markdown::parse("前文 [链接](https://example.com) `code` 后文\n");
            let layout2 = egui_markdown::layout::build_layout(
                ui,
                &doc2.tokens,
                egui::FontId::proportional(15.0),
                ui.visuals().text_color(),
                None,
                400.0,
                false,
                Some(&handler),
                false,
                false,
                true,
                &style,
                None,
            );
            assert!(
                layout2
                    .job
                    .sections
                    .iter()
                    .all(|section| section.format.background != hl),
                "无 == 文档不吃高亮底色"
            );
        });
        output.drop_without_applying_deltas();
    }

    /// 生产入口一帧的汇集(测试断言素材):galley 全集 + 携带高亮底色的
    /// mesh 顶点(像素级证据 —— 底色矩形真的进了绘制网格)。
    struct HighlightFrame {
        galleys: Vec<(egui::Pos2, std::sync::Arc<egui::Galley>)>,
        bg_vertices: Vec<egui::Pos2>,
    }

    fn render_highlight_frame(ctx: &egui::Context, doc: &str, tab_id: u64) -> HighlightFrame {
        // 与生产 `PreviewState::new` 同一条四层链(wikilink → 高亮 → emoji
        // → 任务),预览面板吃的就是这份渲染副本。
        let mut preview = PreviewState::new(&latermd_editor::EditorBuffer::new(doc));
        let mut outbox = Vec::new();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let output = ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |panel| {
                ui(
                    panel,
                    &mut preview,
                    &AiState::default(),
                    tab_id,
                    false,
                    None,
                    &mut outbox,
                );
            },
        );
        // 两主题 token 都收(行内代码底色灰阶 50/225,与两 token 均不同值,
        // 不会误报):哪一档出现都算高亮底色进了绘制网格。
        let tokens = [
            highlight_bg_color(&egui::Visuals::dark()),
            highlight_bg_color(&egui::Visuals::light()),
        ];
        let mut galleys = Vec::new();
        let mut bg_vertices = Vec::new();
        fn collect(
            shape: &egui::epaint::Shape,
            tokens: &[egui::Color32; 2],
            galleys: &mut Vec<(egui::Pos2, std::sync::Arc<egui::Galley>)>,
            bg_vertices: &mut Vec<egui::Pos2>,
        ) {
            match shape {
                egui::epaint::Shape::Mesh(mesh) => {
                    for vertex in &mesh.vertices {
                        if tokens.contains(&vertex.color) {
                            bg_vertices.push(vertex.pos);
                        }
                    }
                }
                egui::epaint::Shape::Text(t) => {
                    galleys.push((t.pos, t.galley.clone()));
                    // output.shapes 里的文本还没 tessellate(底色矩形要过
                    // 渲染器才成网格)—— 这里显式 tessellate,像素级断言
                    // 「底色真的进了绘制网格」。
                    let mut tessellator = egui::epaint::Tessellator::new(
                        1.0,
                        egui::epaint::TessellationOptions::default(),
                        [4096, 4096],
                        Vec::new(),
                    );
                    let mut mesh = egui::epaint::Mesh::default();
                    tessellator.tessellate_text(t, &mut mesh);
                    for vertex in &mesh.vertices {
                        if tokens.contains(&vertex.color) {
                            bg_vertices.push(vertex.pos);
                        }
                    }
                }
                egui::epaint::Shape::Vec(v) => v
                    .iter()
                    .for_each(|s| collect(s, tokens, galleys, bg_vertices)),
                _ => {}
            }
        }
        for clipped in &output.shapes {
            collect(&clipped.shape, &tokens, &mut galleys, &mut bg_vertices);
        }
        output.drop_without_applying_deltas();
        HighlightFrame {
            galleys,
            bg_vertices,
        }
    }

    /// 面板端到端(暗主题):正文与表格 cell 里的高亮段落成带底色的
    /// galley section,底色真的被画出来(mesh 顶点携带推导底色);普通
    /// 文本照旧无底色。
    #[test]
    fn highlight_flows_through_preview_panel_and_table_cell() {
        let ctx = egui::Context::default();
        let doc = "正文 ==高亮内容== 收尾\n\n| 列 |\n|---|\n| ==格== |\n";
        let frame = render_highlight_frame(&ctx, doc, 21);
        let hl = highlight_bg_color(&egui::Visuals::dark());
        let marked: Vec<String> = frame
            .galleys
            .iter()
            .flat_map(|(_, galley)| {
                galley
                    .job
                    .sections
                    .iter()
                    .filter(|section| section.format.background == hl)
                    .map(|section| section.byte_range.slice(&galley.job.text).to_owned())
            })
            .collect();
        assert!(
            marked.contains(&"高亮内容".to_owned()),
            "正文高亮段带底色:{marked:?}"
        );
        assert!(
            marked.contains(&"格".to_owned()),
            "表格 cell 高亮段带底色:{marked:?}"
        );
        assert_eq!(marked.len(), 2, "恰好两段:{marked:?}");
        assert!(
            !frame.bg_vertices.is_empty(),
            "像素级:高亮底色真的进了绘制网格"
        );
    }

    /// 面板端到端(明主题):同一推导式在明主题出明档底色,渲染路径同
    /// 样出底色段与网格像素。
    #[test]
    fn highlight_renders_in_light_theme_too() {
        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::light());
        let frame = render_highlight_frame(&ctx, "==明档高亮==\n", 22);
        let hl = highlight_bg_color(&egui::Visuals::light());
        let marked: Vec<&str> = frame
            .galleys
            .iter()
            .flat_map(|(_, galley)| {
                galley
                    .job
                    .sections
                    .iter()
                    .filter(|section| section.format.background == hl)
                    .map(|section| section.byte_range.slice(&galley.job.text))
            })
            .collect();
        assert_eq!(marked, vec!["明档高亮"], "明主题同通道出底色段");
        assert!(!frame.bg_vertices.is_empty(), "明主题底色同样有实际绘制");
    }

    /// 否决线(生产入口全帧口径):无 `==` 的文档 —— 链接/行内代码/emoji/
    /// wikilink 照常渲染 —— 不画任何高亮底色像素、无底色 galley 段。
    #[test]
    fn highlight_absent_document_stays_free_of_highlight_paint() {
        let ctx = egui::Context::default();
        let doc = "正文 [链接](https://example.com) `code` 😀 与 [目标](<wiki://X>)\n";
        let frame = render_highlight_frame(&ctx, doc, 23);
        let hl_dark = highlight_bg_color(&egui::Visuals::dark());
        assert!(frame.bg_vertices.is_empty(), "无 == 文档不画高亮底色像素");
        assert!(
            frame.galleys.iter().all(|(_, galley)| galley
                .job
                .sections
                .iter()
                .all(|section| section.format.background != hl_dark)),
            "无 == 文档零高亮底色段"
        );
    }

    /// 组合叠加:同一行里高亮与任务标记/粗体/行内代码/链接共存 —— 高亮
    /// 恰一段带底色,其余构造照常渲染、互不破(M1 豁免清单的同侧验证:
    /// 粗体/行内代码/链接内的 `==` 本就不会成对)。
    #[test]
    fn highlight_composes_with_other_inline_constructs() {
        let ctx = egui::Context::default();
        let doc = "- [ ] **粗体** ==真高亮== `code` [链接](https://example.com) 收尾\n";
        let frame = render_highlight_frame(&ctx, doc, 24);
        let hl = highlight_bg_color(&egui::Visuals::dark());
        let marked: Vec<String> = frame
            .galleys
            .iter()
            .flat_map(|(_, galley)| {
                galley
                    .job
                    .sections
                    .iter()
                    .filter(|section| section.format.background == hl)
                    .map(|section| section.byte_range.slice(&galley.job.text).to_owned())
            })
            .collect();
        assert_eq!(marked, vec!["真高亮"], "恰高亮段带底色:{marked:?}");
        let all: String = frame
            .galleys
            .iter()
            .map(|(_, galley)| galley.text().to_owned())
            .collect();
        for piece in ["粗体", "code", "链接", "收尾"] {
            assert!(
                all.contains(piece),
                "既有 inline 构造照常渲染,缺 {piece}:{all:?}"
            );
        }
    }

    /// 测试构造:默认参数的 handler(颜色值不影响消息产出)。
    fn handler_for_test() -> AiLinkHandler {
        AiLinkHandler::new(
            egui::Color32::WHITE,
            false,
            egui::Color32::BLACK,
            &AiState::default(),
        )
    }

    /// 测试渲染:与生产同构的 MarkdownLabel(展开 wikilink 后的单行渲染)。
    fn render_doc(ui: &mut egui::Ui, doc: &str, handler: &AiLinkHandler) {
        MarkdownLabel::new(egui::Id::new("preview-md"), doc)
            .link_handler(handler)
            .show(ui);
    }

    /// 右键菜单 M3(context-menu-plan §3 M3):`is_openable_href` 判定矩阵
    /// —— 协议链接(流/字形/checkbox/标记)不弹菜单,普通 URL 弹。
    #[test]
    fn openable_href_matrix() {
        assert!(is_openable_href("https://example.com/a"));
        assert!(is_openable_href("http://example.com"));
        assert!(is_openable_href("file:///tmp/x.png"));
        assert!(is_openable_href("relative/doc.md"));
        assert!(!is_openable_href(&format!(
            "{}润色",
            crate::ai_link::SCHEME
        )));
        assert!(!is_openable_href(&format!(
            "{}架构",
            latermd_md::WIKI_SCHEME
        )));
        assert!(!is_openable_href(&format!(
            "{}😀",
            latermd_md::EMOJI_SCHEME
        )));
        assert!(!is_openable_href(&format!("{}0", latermd_md::TASK_SCHEME)));
        assert!(!is_openable_href(&format!(
            "{}x",
            latermd_md::HIGHLIGHT_SCHEME
        )));
    }

    /// 预览链接右键菜单:右键链接 → 菜单在场 → 点「复制链接地址」产出
    /// `Message::CopyText`。走真实 MarkdownLabel 渲染(vendored galley 主
    /// 路径的 secondary-click 钩子),点击位置按字形量出。
    #[test]
    fn link_context_menu_copies_href() {
        let doc = "前文 [链接甲](https://example.com/target) 后文\n";
        let ctx = egui::Context::default();
        let handler = handler_for_test();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 600.0));

        // 帧一:量「链接甲」的中心
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(screen));
                render_doc(&mut child, doc, &handler);
            },
        );
        let target = output
            .shapes
            .iter()
            .find_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                let char_index = text.galley.job.text.chars().position(|c| c == '链')?;
                let rect = text
                    .galley
                    .pos_from_cursor(egui::text::CCursor::new(char_index));
                Some(
                    egui::pos2(0.0, 0.0)
                        + (text.galley.mesh_bounds.min.to_vec2()
                            + rect.min.to_vec2()
                            + egui::vec2(2.0, rect.height() / 2.0)
                            + text.pos.to_vec2()),
                )
            })
            .expect("「链接甲」已渲染");
        output.textures_delta.clear();

        // 帧二/三:右键(按下 + 释放)
        let press = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: Default::default(),
        };
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                events: vec![egui::Event::PointerMoved(target), press(target, true)],
                ..Default::default()
            },
            |ui| {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(screen));
                render_doc(&mut child, doc, &handler);
            },
        );
        output.textures_delta.clear();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                events: vec![press(target, false)],
                ..Default::default()
            },
            |ui| {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(screen));
                render_doc(&mut child, doc, &handler);
            },
        );
        output.textures_delta.clear();
        assert!(
            handler.clicked.borrow().is_empty(),
            "右键只弹菜单,不产出消息"
        );

        // 菜单打开期间(指针已离开链接,galley 主路径逐帧重挂):帧四渲出
        // 菜单条目,量「复制链接地址」中心;帧五/六左键点击。
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(screen));
                render_doc(&mut child, doc, &handler);
            },
        );
        let copy_pos = output
            .shapes
            .iter()
            .find_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                (text.galley.job.text == "复制链接地址").then(|| {
                    text.galley
                        .mesh_bounds
                        .translate(text.pos.to_vec2())
                        .center()
                })
            })
            .expect("菜单条目已渲染(弹层打开期间逐帧重挂)");
        output.textures_delta.clear();

        let primary = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        for events in [
            vec![egui::Event::PointerMoved(copy_pos)],
            vec![primary(copy_pos, true)],
            vec![primary(copy_pos, false)],
        ] {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(screen));
                    render_doc(&mut child, doc, &handler);
                },
            );
            output.textures_delta.clear();
        }
        assert_eq!(
            handler.clicked.borrow().clone(),
            vec![Message::CopyText("https://example.com/target".to_owned())]
        );
    }
}
