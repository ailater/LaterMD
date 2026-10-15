//! AI 指令卡与富渲染链接 handler(右栏预览与 Live 写作渲染**同一套**)。
//!
//! 2026-10-15 从 `ui::preview`（已于 P5 删除） 搬出并拆层(live-unify-plan P2/D1):
//! 原 preview::AiLinkHandler(右栏 handler,已随之退役)是纯右栏能力,右栏退役后 AI 卡
//! 必须有宿主。拆法:
//!
//! * 基础层 [`RichLinkHandler`]:wiki/hl/emoji/task/mermaid 的样式、点击、
//!   占位绘制,doclink 应用内打开,右键/图片菜单 —— 两栏同一套,字段与
//!   行为即原两栏 handler 的并集(分歧取预览口径:task 占位吃正文色)。
//! * AI 开关 [`RichLinkHandler::with_ai`]:```ai 指令卡渲染(block widget
//!   扩展点)与 `ai://` 链接的样式区分/点击拦截。预览恒开;Live 侧 P2 起
//!   显式开启 —— 这是 decisions-pending 对 #51 M3 岔路②「Live 不复用
//!   AiLinkHandler」的**显式反向决定**:右栏退役后 AI 卡必须有宿主,原
//!   裁决的实质(「不能借 mermaid 顺手改变」)由显式开关延续,不是无声
//!   漂移。
//!
//! 卡片「执行」产出 [`Message::AiLinkClicked`],与 `ai://` 链接点击同一
//! 条归约(流式续写、防重入都在归约侧)。

use crate::ai::AiState;
use crate::ai_link::{self, SCHEME};
use crate::state::Message;
use eframe::egui;
use egui_markdown::link::{LinkHandler, LinkStyle};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::ui::rich_widgets::{
    append_highlight_section, paint_task_checkbox, wiki_link_color, TaskProbeDomain,
};

/// 指令卡「已完成」状态色(绿),按明暗主题取两档,取色法同
/// [`crate::ui::rich_widgets::ai_link_color`]。
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
pub(crate) fn is_instruction_info(language: Option<&str>) -> bool {
    language.is_some_and(|info| info.split_whitespace().next() == Some("ai"))
}

/// 卡片内单条指令展示的字符上限:超长指令(整段文章粘进块里等)不再为它
/// 撑高预览,超出部分以 … 收尾。执行用的是完整原文,截断只影响展示。
pub(crate) const INSTRUCTION_DISPLAY_CHARS: usize = 240;

/// 指令文本的展示截断(字符计数,CJK 同算一个字符)。
pub(crate) fn truncate_instruction(instruction: &str) -> String {
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

/// 链接是否「可打开」(右键菜单挂点判定,context-menu-plan §3 M3):
/// 排除应用内协议(`ai://` `wiki://` `emoji://` `task://` `hl://`)—— 它们
/// 的 click 各有归属(AI 流/开文档/字形/checkbox/底色),菜单挂上去只会
/// 误导;其余(`http(s)://`、外部绝对地址、本地文档路径、`file://`、
/// `#锚点`)可开 —— 其中本地路径与纯锚点的「打开」在应用内完成
/// ([`crate::doclink`]),不走系统浏览器。
pub(crate) fn is_openable_href(href: &str) -> bool {
    crate::doclink::classify(href) != crate::doclink::HrefKind::AppScheme
}

/// 指令卡三态。判定与 `AiState::last_prompt` 绑定(见
/// [`RichLinkHandler::card_status`]),「进行中」样式从简:换色文字,不加
/// spinner。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AiCardStatus {
    /// 从未(或已换文档/失败复位)发起。
    Idle,
    /// 本卡发起的流在途。
    Running,
    /// 本卡发起的流已成功收尾。
    Done,
}

/// 两栏共用的富渲染链接 handler(P2/D1 拆层后的唯一实现,预览与 Live
/// 各自构造):基础层五类链接 + doclink + 菜单恒在;AI 卡与 `ai://` 由
/// [`Self::with_ai`] 显式开启。
pub(crate) struct RichLinkHandler {
    clicked: Rc<RefCell<Vec<Message>>>,
    /// `ai://` 链接与指令卡的紫罗兰色(`with_ai` 时填)。
    color: Option<egui::Color32>,
    /// 当前明暗(wikilink 取色用,与 ai:// 走两套色)。
    dark_mode: bool,
    /// 正文文本色(`link_style` 对 `emoji://`/`task://`/`hl://` 的回落色,
    /// 取构造时真实 visuals,不用 `Visuals::dark()/light()` 推 —— 皮肤可
    /// 自定义文本色)。
    text_color: egui::Color32,
    /// AI 是否在流(卡片「进行中」判据,取自 [`AiState::is_streaming`];
    /// `with_ai` 时填)。
    streaming: bool,
    /// 最近一次真实发起的 prompt(卡片状态匹配键,取自
    /// [`AiState::last_prompt`];`with_ai` 时填)。
    last_prompt: Option<String>,
    /// checkbox 探针坐标域(两面板各说各话,消费侧按域过滤)。
    task_domain: TaskProbeDomain,
    /// 本帧已渲染的指令卡数:卡片序号 = 文档序,是 widget id 的稳定成分
    /// (AGENTS.md §6.7:绝不含内容长度 —— 编辑指令文本不改序号,id 不变)。
    card_count: Cell<usize>,
    /// 本帧已渲染的 mermaid 块数(#51 M3):块序号同为文档序稳定 id 成分,
    /// 与 `card_count` 分开计数 —— 两种块可交错出现,各自序号互不牵连。
    /// Live 侧每块一个 handler,序号在块内从 0 起;widget id 另有块标签
    /// 的 id 命名空间兜住,两块各序 0 不相撞。
    mermaid_count: Cell<usize>,
    /// 本帧各卡片的 widget id(渲染序);测试借它断言 id 稳定性。
    card_ids: RefCell<Vec<egui::Id>>,
    /// 本帧画过的 `emoji://` inline widget 区块(屏幕坐标,含纹理缺失只留
    /// 占位的):手型抑制的命中判定 + 无头探针。`Rc` 共享:Live 一帧多块
    /// (每块一个 handler)收集进同一份,帧末统一消费(与预览单标签一份
    /// 同语义)。
    emoji_rects: Rc<RefCell<Vec<egui::Rect>>>,
}

impl RichLinkHandler {
    /// 基础层:两栏同一套(wiki/hl/emoji/task/mermaid + doclink 内开 +
    /// 右键/图片菜单)。AI 卡与 `ai://` 不在其中 —— 要就 [`Self::with_ai`]。
    pub(crate) fn new(
        task_domain: TaskProbeDomain,
        dark_mode: bool,
        text_color: egui::Color32,
        emoji_rects: Rc<RefCell<Vec<egui::Rect>>>,
    ) -> Self {
        Self {
            clicked: Rc::new(RefCell::new(Vec::new())),
            color: None,
            dark_mode,
            text_color,
            streaming: false,
            last_prompt: None,
            task_domain,
            card_count: Cell::new(0),
            mermaid_count: Cell::new(0),
            card_ids: RefCell::new(Vec::new()),
            emoji_rects,
        }
    }

    /// D1 开关:AI 卡渲染与 `ai://` 拦截。预览恒开;Live 侧 P2 起显式
    /// 开启(decisions-pending 对 #51 M3 岔路②的显式反向决定,不是无声
    /// 漂移)。`color` 是 `ai://` 链接与指令卡的紫罗兰;流状态取自 `ai`
    /// (全局面态:同时至多一个流,last_prompt 是状态匹配键 —— 与预览
    /// 同口径)。
    pub(crate) fn with_ai(mut self, color: egui::Color32, ai: &AiState) -> Self {
        self.color = Some(color);
        self.streaming = ai.is_streaming();
        self.last_prompt = ai.last_prompt.clone();
        self
    }

    /// 帧末收口:把点击消息并入 outbox。
    pub(crate) fn drain_into(&self, outbox: &mut Vec<Message>) {
        outbox.append(&mut self.clicked.borrow_mut());
    }

    /// 本帧是否已拦下链接点击(Live 帧内「点击归链接、不连带进编辑」的
    /// 判定用)。
    pub(crate) fn intercepted_is_empty(&self) -> bool {
        self.clicked.borrow().is_empty()
    }

    /// 指令卡状态:指令文本与最近一次发起的 prompt 相等才认领 —— 防重入
    /// 保证同时至多一个流,菜单入口(`AiStart`)的 prompt 是文档尾部拼装,
    /// 不会与任何指令文本相等,其它卡片不受牵连。同一文本的多张卡同状态,
    /// 是接受的简化(decisions-pending #13)。
    pub(crate) fn card_status(&self, instruction: &str) -> AiCardStatus {
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
    pub(crate) fn request_execute(&self, instruction: &str) {
        self.clicked.borrow_mut().push(Message::AiLinkClicked {
            prompt: Ok(instruction.to_owned()),
        });
    }
}

impl LinkHandler for RichLinkHandler {
    /// ai:// 链接换色(仅 `with_ai`);`underline: true` 只是声明意图 ——
    /// vendored 层当前未消费该字段(hover 下划线对全部链接无条件绘制),
    /// 见 decisions-pending #11。
    ///
    /// `emoji://`(#48 B2)与 `task://`(#63)是 inline widget:文字本就是
    /// 透明占位,这里返回正文色 + 无下划线只是把「不吃超链接样式」的意图
    /// 钉进协议 —— vendored 对 inline widget 的 hover 本就不画下划线
    /// (label.rs `handle_hover`),该返回同时兜住「inline_widget_size 未来
    /// 返回 None」的退化路径。
    fn link_style(&self, href: &str) -> Option<LinkStyle> {
        // `hl://`(#65 M2):高亮不是可点链接,是底色标记 —— 意图声明为
        // 正文色 + 无下划线。vendored 对悬停中的普通链接无条件画下划线
        // (#11 已知限),这里能定的是下划线颜色取正文色,悬停时低调呈现。
        if href.starts_with(latermd_md::HIGHLIGHT_SCHEME) {
            return Some(LinkStyle {
                color: Some(self.text_color),
                underline: false,
            });
        }
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
        // ai://(仅 with_ai):紫罗兰,与 wikilink 青绿区分。
        if let Some(color) = self.color {
            if href.starts_with(SCHEME) {
                return Some(LinkStyle {
                    color: Some(color),
                    underline: true,
                });
            }
        }
        None
    }

    fn click(&self, _text: &str, href: &str, _ui: &mut egui::Ui) -> bool {
        // `hl://`(#65 M2):高亮没有点击语义,吞掉不开浏览器,不发消息
        // (`hl://` 也不是合法 URL)。
        if href.starts_with(latermd_md::HIGHLIGHT_SCHEME) {
            return true;
        }
        // emoji:// 没有点击语义:吞掉(返回 true),绝不交系统浏览器 ——
        // `emoji://😀` 不是合法 URL,交给浏览器只会弹错误提示。
        if href.starts_with(latermd_md::EMOJI_SCHEME) {
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
        if self.color.is_some() {
            if let Some(prompt) = ai_link::parse(href) {
                self.clicked
                    .borrow_mut()
                    .push(Message::AiLinkClicked { prompt });
                return true;
            }
        }
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

    /// `hl://`(#65 M2)走非 widget 分支:追加「正文色文字 + 推导底色」的
    /// 排版段,不走 inline widget —— 文字要可见、行高要随正文,不是占位件。
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

    /// 在透明占位上画 inline widget:task 自绘 checkbox(#63)、emoji 画
    /// Twemoji 纹理(#48 B2,查面板同源纹理缓存,查不到什么都不画 ——
    /// 不画黑块不 panic)。方块边长取占位区宽高的较小者:单枚 emoji 的宽
    /// ≈ 字体自然推进,多字形跨度(旗帜等)会被行高封顶,不横向溢出邻字。
    fn paint_inline_widget(&self, ui: &mut egui::Ui, _text: &str, href: &str, rect: egui::Rect) {
        if href.starts_with(latermd_md::TASK_SCHEME) {
            paint_task_checkbox(ui, rect, href, self.task_domain);
            return;
        }
        let Some(glyph) = href.strip_prefix(latermd_md::EMOJI_SCHEME) else {
            return;
        };
        let side = rect.width().min(rect.height());
        if side <= 0.0 {
            return;
        }
        // 占位区先记探针(含纹理缺失/视口外的):手型抑制的判定面是
        // 「指针落在 widget 占位区」,不看纹理出没出。
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
        // 条目用 menu_item(自绘,墨迹重心补偿):CJK 标签在 egui Button 的
        // hover 灰影里偏上 ~2.5px(menu_item 模块注释有取证)。
        response.context_menu(|ui| {
            if local {
                if crate::ui::menu_item::button(ui, "打开文档").clicked() {
                    clicked.borrow_mut().push(Message::DocLinkClicked {
                        href: href.to_owned(),
                    });
                }
            } else if crate::ui::menu_item::button(ui, "打开链接").clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(href.to_owned()));
            }
            if crate::ui::menu_item::button(ui, "复制链接地址").clicked() {
                clicked
                    .borrow_mut()
                    .push(Message::CopyText(href.to_owned()));
            }
        });
    }

    /// 图片右键:「在浏览器打开 / 复制图片地址」。Live 的第 5 层改写
    /// (`image_rewrites`)已把相对地址改成 `file://` 绝对 URI,系统浏览器
    /// 直接可用。
    fn image_response(&self, _ui: &egui::Ui, _alt: &str, url: &str, response: &egui::Response) {
        let clicked = Rc::clone(&self.clicked);
        response.context_menu(|ui| {
            if crate::ui::menu_item::button(ui, "在浏览器打开").clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url.to_owned()));
            }
            if crate::ui::menu_item::button(ui, "复制图片地址").clicked() {
                clicked.borrow_mut().push(Message::CopyText(url.to_owned()));
            }
        });
    }

    /// mermaid(#51 M3)与 ai 指令卡共用 block_code_widget 扩展点;两个
    /// 判定互斥(info string 首词不同),非 mermaid/ai 围栏不受影响
    /// (否决线:普通代码块照走 vendored 原路径)。ai 卡仅在 `with_ai`
    /// 时认领 —— 关闭态 ```ai 围栏按普通代码块渲染。
    fn is_block_code_widget(&self, language: Option<&str>) -> bool {
        crate::ui::mermaid::is_mermaid_info(language)
            || (self.color.is_some() && is_instruction_info(language))
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
        if self.color.is_none() || !is_instruction_info(language) {
            return None;
        }
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
                            status_label(ui, status, self.color.unwrap_or_default());
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 右键菜单挂点判定(context-menu-plan §3 M3):协议链接一律不弹菜单
    /// (它们的 click 各有归属),普通 URL / 本地路径 / 锚点 / `file://` 弹。
    ///
    /// 随 live-unify P5 从已删的 `ui::preview` 迁来 —— 判定函数留在
    /// `ai_card`,它的覆盖不能随面板一起消失。
    #[test]
    fn openable_href_matrix() {
        assert!(is_openable_href("https://example.com/a"));
        assert!(is_openable_href("http://example.com"));
        assert!(is_openable_href("file:///tmp/x.png"));
        assert!(is_openable_href("relative/doc.md"));
        assert!(!is_openable_href(&format!("{}润色", SCHEME)));
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
}
