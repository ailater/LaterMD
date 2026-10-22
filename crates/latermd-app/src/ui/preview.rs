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

/// ai:// 链接的样式色(紫罗兰,与默认超链接色区分),按明暗主题取两档。
fn ai_link_color(dark_mode: bool) -> egui::Color32 {
    if dark_mode {
        egui::Color32::from_rgb(0xC9, 0x9B, 0xF5)
    } else {
        egui::Color32::from_rgb(0x8B, 0x2F, 0xC9)
    }
}

/// `[[wikilink]]` 的链接色(青绿,与 ai:// 的紫罗兰区分),按明暗主题取两档。
fn wiki_link_color(dark_mode: bool) -> egui::Color32 {
    if dark_mode {
        egui::Color32::from_rgb(0x6C, 0xD4, 0xC0)
    } else {
        egui::Color32::from_rgb(0x0F, 0x7A, 0x66)
    }
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
    let Some(base_dir) = base_dir.filter(|dir| !dir.as_os_str().is_empty()) else {
        return Cow::Borrowed(text);
    };
    let rewrites: Vec<(Range<usize>, String)> = inline_image_dests(text)
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
            (span, uri)
        })
        .collect();
    if rewrites.is_empty() {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 64);
    let mut cursor = 0;
    for (span, uri) in rewrites {
        out.push_str(&text[cursor..span.start]);
        out.push_str(&uri);
        cursor = span.end;
    }
    out.push_str(&text[cursor..]);
    Cow::Owned(out)
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
struct AiLinkHandler {
    clicked: RefCell<Vec<Message>>,
    color: egui::Color32,
    /// 当前明暗(wikilink 取色用,与 ai:// 走两套色)。
    dark_mode: bool,
    /// AI 是否在流(卡片「进行中」判据,取自 [`AiState::is_streaming`])。
    streaming: bool,
    /// 最近一次真实发起的 prompt(卡片状态匹配键,取自 [`AiState::last_prompt`])。
    last_prompt: Option<String>,
    /// 本帧已渲染的指令卡数:卡片序号 = 文档序,是 widget id 的稳定成分
    /// (AGENTS.md §6.7:绝不含内容长度 —— 编辑指令文本不改序号,id 不变)。
    card_count: Cell<usize>,
    /// 本帧各卡片的 widget id(渲染序);测试借它断言 id 稳定性。
    card_ids: RefCell<Vec<egui::Id>>,
}

impl AiLinkHandler {
    fn new(color: egui::Color32, dark_mode: bool, ai: &AiState) -> Self {
        Self {
            clicked: RefCell::new(Vec::new()),
            color,
            dark_mode,
            streaming: ai.is_streaming(),
            last_prompt: ai.last_prompt.clone(),
            card_count: Cell::new(0),
            card_ids: RefCell::new(Vec::new()),
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
    fn link_style(&self, href: &str) -> Option<LinkStyle> {
        // [[wikilink]] 用青绿,与 ai:// 的紫罗兰区分:两种链接的点击后果不同
        // (一个开文档、一个发起 AI 流),颜色不该撞
        if href.starts_with(latermd_md::WIKI_SCHEME) {
            return Some(LinkStyle {
                color: Some(wiki_link_color(self.dark_mode)),
                underline: true,
            });
        }
        href.starts_with(SCHEME).then_some(LinkStyle {
            color: Some(self.color),
            underline: true,
        })
    }

    fn click(&self, _text: &str, href: &str, _ui: &mut egui::Ui) -> bool {
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
            None => false,
        }
    }

    fn is_block_code_widget(&self, language: Option<&str>) -> bool {
        is_instruction_info(language)
    }

    fn block_code_widget(
        &self,
        ui: &mut egui::Ui,
        text: &str,
        language: Option<&str>,
    ) -> Option<egui::Response> {
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

/// 绘制预览面板。`base_dir` 是当前文档所在目录:相对图片地址以它为锚
/// 拼成 `file://` 绝对 URI(见 [`resolve_relative_images`]);`None` =
/// 文档未落盘,原样渲染。
pub fn ui(
    panel: &mut egui::Ui,
    preview: &mut PreviewState,
    ai: &AiState,
    base_dir: Option<&Path>,
    outbox: &mut Vec<Message>,
) {
    egui::ScrollArea::vertical()
        .id_salt("preview-scroll")
        // 不收缩宽度,让 wrap 以面板宽为界
        .auto_shrink([false, false])
        .show(panel, |ui| {
            // widget id 必须是常量:绝不含内容长度/hash,否则每次编辑都
            // 清空 vendored 层临时缓存,增量高亮与分段缓存全部失效
            // (AGENTS.md §6.7)。内容变化已在上游按修订号节流,这里每帧
            // 拿到的都是"仅在变化时重建"的同一字符串;相对图片的改写也
            // 是纯函数 —— 同样的输入永远产出同样的字符串,缓存照常命中。
            let handler = AiLinkHandler::new(
                ai_link_color(ui.visuals().dark_mode),
                ui.visuals().dark_mode,
                ai,
            );
            // 渲染的是**展开过 wikilink 的**文本:源码里的 [[X]] 在这里已是
            // [X](<wiki://X>) 链接,点击由下面的 handler 拦截;相对图片
            // 地址在这里再换成 file:// URI(两层都是"只改渲染,源码不动")
            let rendered = resolve_relative_images(&preview.rendered, base_dir);
            MarkdownLabel::new(egui::Id::new("preview-md"), rendered.as_ref())
                .wrap()
                // heal:true = 每帧渲染前对整篇文本补闭合(vendored parser::heal),
                // AI 流式输出的残缺帧(未闭合 fence/加粗)语法合法,完整文档
                // 上是恒等变换(Cow::Borrowed 原样返回)。P1 流式的必需品
                // (AGENTS.md §6.5),岔路登记见 docs/decisions-pending.md #10。
                .heal(true)
                .link_handler(&handler)
                .show(ui);
            handler.drain_into(outbox);
        });

    // 大纲跳转的预览侧:把字节偏移换算成 y 再滚过去。锚点是上一行渲染时
    // vendored 层记录下的(section → y),这里只做查表 + 请求滚动。
    if let Some(target) = preview.scroll_target.take() {
        if let Some(anchors) = egui_markdown::section_anchors(panel, egui::Id::new("preview-md")) {
            // 取「起点不超过目标」的最后一个锚点:标题所在节的顶部
            let anchor = anchors
                .iter()
                .rev()
                .find(|anchor| anchor.byte_start <= target)
                .or_else(|| anchors.first());
            if let Some(anchor) = anchor {
                panel.scroll_to_rect(
                    egui::Rect::from_min_size(
                        egui::pos2(panel.min_rect().left(), anchor.y),
                        egui::vec2(panel.available_width().max(1.0), 1.0),
                    ),
                    Some(egui::Align::TOP),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::RawInput;

    /// 造一个指定流式/最近 prompt 的 AI 状态(卡片状态的三个输入)。
    fn ai_state(streaming: bool, last_prompt: Option<&str>) -> AiState {
        let mut ai = AiState::default();
        ai.streaming = streaming;
        ai.last_prompt = last_prompt.map(str::to_owned);
        ai
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
        let idle = AiLinkHandler::new(color, true, &ai_state(false, None));
        assert_eq!(idle.card_status("续写"), AiCardStatus::Idle);

        let running = AiLinkHandler::new(color, true, &ai_state(true, Some("续写")));
        assert_eq!(running.card_status("续写"), AiCardStatus::Running);

        let done = AiLinkHandler::new(color, true, &ai_state(false, Some("续写")));
        assert_eq!(done.card_status("续写"), AiCardStatus::Done);

        // 其它卡片不受牵连:菜单发起的 prompt 是拼装文本,不等任何指令
        let unrelated = AiLinkHandler::new(
            color,
            true,
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
            let mut preview = PreviewState {
                rendered: latermd_md::expand_wikilinks(&text),
                text,
                synced_rev: 0,
                outline: Vec::new(),
                scroll_target: None,
            };
            ui(panel, &mut preview, &AiState::default(), None, &mut outbox);
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
        let handler = AiLinkHandler::new(ai_link_color(true), true, &AiState::default());
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
        let handler = AiLinkHandler::new(ai_link_color(true), true, &AiState::default());
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

    /// 预览消费滚动目标一次:不消费会导致每帧都把预览拽回目标位置,用户
    /// 再也滚不动(锚点缺失时也不 panic)。
    #[test]
    fn preview_consumes_scroll_target_once() {
        let ctx = egui::Context::default();
        let doc = "# 一\n\n正文\n\n## 二\n\n正文二\n";
        let mut preview = PreviewState {
            rendered: doc.to_owned(),
            text: doc.to_owned(),
            synced_rev: 0,
            outline: Vec::new(),
            scroll_target: Some(0),
        };
        let mut outbox = Vec::new();
        let output = ctx.run_ui(RawInput::default(), |panel| {
            ui(panel, &mut preview, &AiState::default(), None, &mut outbox);
        });
        output.drop_without_applying_deltas();
        assert_eq!(preview.scroll_target, None, "滚动目标只消费一次");
        assert!(outbox.is_empty(), "滚动不产消息");
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
            let handler = AiLinkHandler::new(ai_link_color(true), true, &AiState::default());
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
}
