//! Live Preview(P3 深水区 #9):光标所在块显示源码,其余富渲染。
//!
//! **铁律(roadmap 阶段 5)**:源码模式与 Live Preview 必须是**一个编辑器 +
//! 一个 `render_mode` 标志** —— 共用同一个 rope buffer 和同一套撤销语义。
//! 本模块因此不持有任何文本副本:活动块的编辑经 [`BlockBuffer`] 直接落在同
//! 一个 [`EditorBuffer`] 上(块内偏移 + 块首偏移 = 全文偏移),切模式不丢
//! 光标也不丢 undo 栈(undo 走 TextEdit 内建 undoer,快照就是这份文本)。
//!
//! 块表来自 `latermd_md::blocks`(字节区间**连续覆盖全文**):这是安全底线
//! —— 若有字节落在任何块之外,在那儿敲一个字符就会静默丢失。
//!
//! v1 的范围(roadmap「内联标记半隐藏 v1 可简化」):块是**整块**切换。LP2-1
//! (v2)在活动块内做**内联标记半隐藏**:正文正常显示,`**` 等标记字符以半
//! 透明遮罩弱化,光标/选区贴上的段显形 —— 区间提取是纯函数
//! `latermd_md::inline_marks`,绘制是纯叠加,不碰 TextEdit 的状态与命中测试。
//! LP2-2 加**选区扩展与配对显形**:选区/光标触碰某标记时其配对另一侧同
//! 时显形;从标记内侧发起的双击/拖选把选区一次性扩到完整标记对(内容+
//! 两侧标记),让编辑标记本身有可及入口 —— 决策是纯函数
//! `latermd_md::mark_interaction`,选区仍在块内闭合(v1 既定,块是独立
//! TextEdit,不引入跨块选区行为变更)。
//!
//! LP2-3 补**活动块跟随与跳转时序**(#29 源码栏修复在 Live 侧的同款缺
//! 口):Live 自有 ScrollArea,在键盘导航帧与光标写回帧把光标行滚入视口
//! —— 照 #29 的「帧内一次性标志」手法(标志当帧即焚,滚轮/空闲帧不抢
//! 滚动);跨块路由与大纲跳转(`cursor.jump_to`)的光标落地与视口跟随
//! **同帧**发生,不出现「光标写了但视图没跟」的帧。滚动偏移与跟随标志
//! 都在 UI 侧,不进 State 归约。

use std::{
    hash::{Hash, Hasher},
    ops::Range,
    sync::Arc,
};

use eframe::egui;
use egui_markdown::{LinkHandler, LinkStyle, MarkdownLabel};
use egui_markdown_style::MarkdownStyle;
use latermd_editor::EditorBuffer;

use crate::state::{Message, OutlineCursor, PreviewState};

type RichCacheEntry = (
    u64,
    Arc<str>,
    Arc<latermd_md::OffsetMap>,
    Arc<latermd_md::OffsetMap>,
    Arc<latermd_md::OffsetMap>,
    Arc<latermd_md::OffsetMap>,
);
type RichBlock = (
    Arc<str>,
    Arc<latermd_md::OffsetMap>,
    Arc<latermd_md::OffsetMap>,
    Arc<latermd_md::OffsetMap>,
    Arc<latermd_md::OffsetMap>,
);

fn source_hash(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// 编辑器的渲染模式(P3 的那**一个标志**)。
///
/// 源码模式与 Live Preview 共用同一个 rope buffer、同一套撤销语义,区别仅
/// 在于是否应用「光标所在块显示源码」这条规则 —— 所以切换不该有任何恢复
/// 逻辑(roadmap 阶段 5 的原则)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderMode {
    /// 整篇源码。
    #[default]
    Source,
    /// 光标块源码、其余富渲染。
    Live,
}

impl RenderMode {
    /// 互换(命令入口)。
    pub fn opposite(self) -> Self {
        match self {
            Self::Source => Self::Live,
            Self::Live => Self::Source,
        }
    }
}

/// Live Preview 的每标签状态:块表 + 当前活动块。
#[derive(Debug, Clone, Default)]
pub struct LiveState {
    /// 块的字节区间,连续覆盖全文(`latermd_md::blocks`)。
    pub blocks: Vec<Range<usize>>,
    /// 与 `blocks` 一一对应的源码指纹。编辑后只重建指纹发生变化的块，
    /// 让后方未修改块继续复用富渲染缓存。
    block_hashes: Vec<u64>,
    /// 正在以源码编辑的块序号;`None` = 没有块获得焦点。
    pub active: Option<usize>,
    /// 打开文档后的浏览态。浏览态只绘制富文本，不自动抢占编辑焦点；
    /// 左键双击块才进入写作态。
    pub browse_only: bool,
    /// 是否启用打开文档时的浏览态交互。默认关闭以保持无头测试夹具和
    /// 直接构造 LiveState 的既有语义；真实切入 Live 时由 State 打开。
    pub(crate) browse_enabled: bool,
    /// 下一帧要落到(块, 块内字符偏移)的光标:跨块 caret 路由的交接点。
    pub pending_caret: Option<(usize, usize)>,
    /// pending_caret 落地时的块内选区(主, 副)。`None` = 塌缩成
    /// pending_caret 的单点(跳转/路由/点击进编辑的既有口径);`Some` =
    /// 格式动作(live 分支)产出的「内容仍选中」落点 —— 与源码模式
    /// 「新选区落在内容处」同语义(连点第二次才 toggle 得掉,见
    /// `compose::View::wrap` 的严判定)。与 pending_caret 同生共死:每个
    /// 非格式入口置 pending_caret 时都一并清掉它,落地帧 take。
    pub pending_range: Option<(usize, usize)>,
    /// 块表对应的修订号;只在修订号前进时重算块。
    synced_rev: Option<u64>,
    /// 活动块的内联标记缓存(LP2-1 半隐藏 + LP2-2 选区扩展):(修订号,
    /// 块序号) → 标记段与成对表(同一次解析的产出,不会互相错位)。与块
    /// 表同一条纪律:修订号不动就不重解析(打一个字的代价 = 重解析活动
    /// 块,与 v1 每次编辑重算整篇块表同一量级)。
    marks: Option<(u64, usize, latermd_md::InlineMarks)>,
    /// 拖选锚点(LP2-2):活动块 TextEdit 上**按下帧**的塌缩光标(块内
    /// 字符偏移),拖选松手帧用于「从标记发起的拖选扩展到完整标记对」。
    /// 生命周期 = 一次按压:松手帧消费,按在别处的按下帧清除。
    drag_anchor: Option<usize>,
    /// pending_caret 落地帧是否同时把光标行滚入视口(LP2-3)。跨块路由
    /// 与大纲跳转置真 —— 光标去了别的块,视图必须同帧跟上;点击进编辑
    /// 置假 —— 指针刚把视口定位到点击处,再跟随等于把视图拽离用户点的
    /// 地方(#29「滚轮/空闲帧不抢滚动」的同族红线)。与 pending_caret
    /// 同生共死:每个置 pending 的入口都一并写,落地帧消费。格式动作
    /// (live 归约)也置假 —— 选区是用户刚框出来的,无需拽视口。
    pub caret_follow: bool,
    /// 富渲染块链接改写的块级缓存(#63;#65 M2 起高亮层并入同一条缓存):
    /// 块序号 → (源码指纹, 改写后块文本, 高亮层映射, 任务层映射)。
    /// 修订号前进时保留未变化块的条目，只让源码指纹变化的块重新改写，
    /// 避免在大文档中输入一个字符就清空整篇缓存。两张映射随渲染文本
    /// 同源缓存:点击进编辑的落点换算要把渲染文本偏移**逆穿**回块源码坐标。
    rich_cache: std::collections::HashMap<usize, RichCacheEntry>,
}

impl LiveState {
    /// 同步块表:修订号变了才重解析。
    ///
    /// 写作态重算后**按光标字节重新定位活动块** —— 敲回车会分裂块、删空行
    /// 会合并块,按序号记忆必然错位(在第 2 块开头敲回车后,原来的第 3 块变成
    /// 第 4 块)。浏览态保留空活动块,不因源码光标自动进入写作。
    pub fn sync(&mut self, editor: &EditorBuffer, cursor_byte: Option<usize>) {
        let rev = editor.revision();
        if self.synced_rev == Some(rev) {
            // 写作态块表没变但还没有活动块(首次进入 Live 模式):按光标补定位,
            // 不必等下一次编辑 —— 否则要「先敲一个字」才出现可编辑的块。
            // 浏览态没有编辑意图，不能因为源码模式遗留的光标而自动抢
            // 出 TextEdit；进入写作只由双击或明确的程序跳转触发。
            if self.active.is_none() && !self.browse_only {
                self.active = cursor_byte.and_then(|byte| self.block_containing(byte));
            }
            return;
        }
        self.blocks = latermd_md::blocks(editor.text());
        self.block_hashes = self
            .blocks
            .iter()
            .map(|range| source_hash(&editor.text()[range.clone()]))
            .collect();
        self.synced_rev = Some(rev);
        self.active = if self.browse_only {
            // 保留归约侧预先指定的活动块(例如格式动作与大纲跳转);
            // 普通浏览态的活动块本来就是 None，因此不会被光标复活。
            self.active.filter(|index| *index < self.blocks.len())
        } else {
            cursor_byte
                .and_then(|byte| self.block_containing(byte))
                .or_else(|| self.active.filter(|index| *index < self.blocks.len()))
        };
    }

    /// 富渲染块的链接改写(#63 checkbox + #65 高亮):缓存命中直取,未命中
    /// 重算并入表。返回渲染文本与**逆穿方向**的两张映射(渲染坐标 → 上一
    /// 层坐标):层序与预览四层链一致(高亮在任务之前,生产顺序 wikilink →
    /// 高亮 → emoji → 任务的 Live 子集);两层各有「无目标字符不启动解析」
    /// 的快路径,两层快路径直接恒等返回。
    fn rich_block(&mut self, index: usize, _rev: u64, block_text: &str) -> RichBlock {
        let hash = self.block_hashes.get(index).copied();
        if let Some((seen_hash, rendered, offset_map, highlight_map, emoji_map, task_map)) =
            self.rich_cache.get(&index)
        {
            if Some(*seen_hash) == hash {
                return (
                    Arc::clone(rendered),
                    Arc::clone(offset_map),
                    Arc::clone(highlight_map),
                    Arc::clone(emoji_map),
                    Arc::clone(task_map),
                );
            }
        }
        let (after_wikilinks, offset_map) = latermd_md::expand_wikilinks_with_map(block_text);
        let (after_highlight, highlight_map) = latermd_md::expand_highlight_links(&after_wikilinks);
        let (after_emoji, emoji_map) = latermd_md::expand_emoji_links(
            &after_highlight,
            crate::ui::emoji_data::covered_glyphs(),
        );
        let (rendered, task_map) = latermd_md::expand_task_links(&after_emoji);
        let rendered = Arc::<str>::from(rendered);
        let offset_map = Arc::new(offset_map);
        let highlight_map = Arc::new(highlight_map);
        let emoji_map = Arc::new(emoji_map);
        let task_map = Arc::new(task_map);
        self.rich_cache.insert(
            index,
            (
                hash.unwrap_or_else(|| source_hash(block_text)),
                Arc::clone(&rendered),
                Arc::clone(&offset_map),
                Arc::clone(&highlight_map),
                Arc::clone(&emoji_map),
                Arc::clone(&task_map),
            ),
        );
        (rendered, offset_map, highlight_map, emoji_map, task_map)
    }

    /// 把点击点的光标落点换算成块内字符偏移(#170):点击帧富渲染块已
    /// 画出,vendored 的帧内命中表给出「点击点 → 渲染文本字符索引」;
    /// 渲染坐标经任务层、高亮层两张映射**逆序**穿回块源码字节,钳进块
    /// 文本后换算块内字符偏移。文本 galley 之外(代码块/表格 widget、块
    /// 间空隙)回退到「y 最近渲染子块的起点」(点代码块落 ``` 行,点段
    /// 间空隙落相邻段落首);连渲染子块表都没有(被剔除)时回退 None,
    /// 调用方落块尾 —— 与旧行为一致,不劣化。
    #[allow(clippy::too_many_arguments)]
    fn click_caret_local(
        ui: &egui::Ui,
        label_id: egui::Id,
        pos: egui::Pos2,
        rendered: &str,
        offset_map: &latermd_md::OffsetMap,
        highlight_map: &latermd_md::OffsetMap,
        emoji_map: &latermd_md::OffsetMap,
        task_map: &latermd_md::OffsetMap,
        block_source_len: usize,
    ) -> Option<usize> {
        let rendered_byte = match egui_markdown::char_index_at_pos(ui, label_id, pos) {
            Some(char_index) => char_indices_to_byte(rendered, char_index),
            None => {
                let blocks = egui_markdown::block_span_rects(ui, label_id)?;
                let last = blocks
                    .iter()
                    .max_by_key(|block| block.rect.min.y.to_bits())?;
                if pos.y > last.rect.max.y {
                    rendered.len()
                } else {
                    let nearest = blocks.iter().min_by_key(|block| {
                        // 点击点 y 到子块 rect 的垂直距离(区间内为 0)
                        let dy = if pos.y < block.rect.min.y {
                            block.rect.min.y - pos.y
                        } else if pos.y > block.rect.max.y {
                            pos.y - block.rect.max.y
                        } else {
                            0.0
                        };
                        dy.to_bits()
                    })?;
                    nearest.span.start.min(rendered.len())
                }
            }
        };
        // 逆穿改写层(生产顺序 高亮 → 任务,逆序 任务 → 高亮);改写段
        // 内部命中映回段首,是映射表的既定语义。
        let after_task = task_map.rendered_to_source(rendered_byte);
        let after_emoji = emoji_map.rendered_to_source(after_task);
        let after_highlight = highlight_map.rendered_to_source(after_emoji);
        let source_byte = offset_map.rendered_to_source(after_highlight);
        let clamped = source_byte.min(block_source_len);
        Some(clamped)
    }

    /// 包含该字节的块序号;落在块之间的边界上取后一块(边界属于后一块的起点)。
    pub fn block_containing(&self, byte: usize) -> Option<usize> {
        self.blocks
            .iter()
            .position(|block| byte >= block.start && byte < block.end)
            // 文末:落在最后一块的闭合端也算最后一块(末尾可编辑)
            .or_else(|| (!self.blocks.is_empty()).then(|| self.blocks.len() - 1))
    }

    /// 块的字符数(光标路由要按字符偏移算,块区间是字节)。
    pub fn block_char_len(&self, editor: &EditorBuffer, index: usize) -> usize {
        self.blocks.get(index).map_or(0, |block| {
            editor.byte_to_char(block.end) - editor.byte_to_char(block.start)
        })
    }

    /// 重置(换文档 / 关标签):不留下上一份文档的块表。
    pub fn reset(&mut self) {
        self.blocks.clear();
        self.active = None;
        self.browse_only = true;
        self.browse_enabled = true;
        self.pending_caret = None;
        self.pending_range = None;
        self.synced_rev = None;
        self.marks = None;
        self.drag_anchor = None;
        self.caret_follow = false;
        self.rich_cache.clear();
        self.block_hashes.clear();
    }
}

/// Live 富渲染块的链接 handler(#51 M3 + #63):mermaid 围栏出图原样
/// 转发给 [`crate::ui::mermaid::LiveMermaidHandler`](行为零变化,含块序
/// 计数);任务 checkbox 的点击在 `task://` 前缀处**吞掉**(不交系统
/// 浏览器、也不在此发切换消息)—— 列表项内 inline widget 的 vendored
/// section/token 映射在 egui 0.36 下错位,这条 click 路径实测不可达
/// (#63 live 测试断零消息);切换统一由 `ui` 帧末的探针命中判定发出,
/// 与右栏预览同一判定层。链接的其余行为(ai:// 卡、emoji、wikilink)
/// 维持 Live 接入 handler 前的 vendored 默认(#51 M3 否决线,不因
/// checkbox 顺手改变)。
struct LiveRichHandler<'a> {
    mermaid: &'a crate::ui::mermaid::LiveMermaidHandler,
    /// 本块在全文中的字节起点(checkbox 探针坐标域携带,帧末命中换算用)。
    task_base: usize,
    /// 正文文本色(#65 M2 高亮取色用;构造时取真实 visuals,与预览
    /// handler 同一纪律 —— 不用 `Visuals::dark()/light()` 推)。
    text_color: egui::Color32,
    dark_mode: bool,
    clicked: std::cell::RefCell<Vec<Message>>,
}

impl<'a> LiveRichHandler<'a> {
    fn new(
        mermaid: &'a crate::ui::mermaid::LiveMermaidHandler,
        task_base: usize,
        text_color: egui::Color32,
        dark_mode: bool,
    ) -> Self {
        Self {
            mermaid,
            task_base,
            text_color,
            dark_mode,
            clicked: std::cell::RefCell::new(Vec::new()),
        }
    }

    fn drain_into(&self, outbox: &mut Vec<Message>) {
        outbox.append(&mut self.clicked.borrow_mut());
    }
}

impl LinkHandler for LiveRichHandler<'_> {
    fn is_block_code_widget(&self, language: Option<&str>) -> bool {
        self.mermaid.is_block_code_widget(language)
    }

    fn block_code_widget(
        &self,
        ui: &mut egui::Ui,
        text: &str,
        language: Option<&str>,
    ) -> Option<egui::Response> {
        self.mermaid.block_code_widget(ui, text, language)
    }

    /// 与预览 handler 同款意图声明:checkbox 不吃超链接样式(占位透明,
    /// 视觉由自绘接管;同时兜住退化路径);`hl://`(#65 M2)同样不是
    /// 可点链接,正文色 + 无下划线,下划线颜色的处理与预览一致。
    fn link_style(&self, href: &str) -> Option<LinkStyle> {
        if href.starts_with(latermd_md::HIGHLIGHT_SCHEME) {
            return Some(LinkStyle {
                color: Some(self.text_color),
                underline: false,
            });
        }
        if href.starts_with(latermd_md::EMOJI_SCHEME) {
            return Some(LinkStyle {
                color: Some(self.text_color),
                underline: false,
            });
        }
        if href.starts_with(latermd_md::WIKI_SCHEME) {
            return Some(LinkStyle {
                color: Some(crate::ui::preview::wiki_link_color(self.dark_mode)),
                underline: true,
            });
        }
        href.starts_with(latermd_md::TASK_SCHEME)
            .then_some(LinkStyle {
                color: None,
                underline: false,
            })
    }

    fn click(&self, _text: &str, href: &str, _ui: &mut egui::Ui) -> bool {
        // `hl://`(#65 M2):高亮没有点击语义,吞掉不开浏览器,与预览
        // handler 同一口径。
        if href.starts_with(latermd_md::HIGHLIGHT_SCHEME) {
            return true;
        }
        if href.starts_with(latermd_md::EMOJI_SCHEME) {
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
        // 只吞不发:切换消息由帧末探针判定单点发出,这里若转发,将来
        // vendored 错位修好时会双发(两次切换 = 一步空转 + 两份 undo)。
        crate::ui::preview::parse_task_href(href).is_some()
    }

    /// 透明占位:与预览 handler 同一实现(链接文字本体 + 同款字体)。
    /// `hl://`(#65 M2)走非 widget 分支:追加「正文色文字 + 推导底色」
    /// 的排版段,与预览同一共享实现(一个真源)。
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
            crate::ui::preview::append_highlight_section(ui, text, job, font);
            return true;
        }
        if href.starts_with(latermd_md::EMOJI_SCHEME) {
            let format = egui::TextFormat {
                font_id: font.clone(),
                color: egui::Color32::TRANSPARENT,
                ..egui::TextFormat::default()
            };
            job.append(text, 0.0, format);
            return true;
        }
        if !href.starts_with(latermd_md::TASK_SCHEME) {
            return false;
        }
        let format = egui::TextFormat {
            font_id: font.clone(),
            color,
            ..egui::TextFormat::default()
        };
        job.append(text, 0.0, format);
        true
    }

    fn inline_widget_size(&self, href: &str, font: &egui::FontId) -> Option<egui::Vec2> {
        (href.starts_with(latermd_md::TASK_SCHEME) || href.starts_with(latermd_md::EMOJI_SCHEME))
            .then(|| egui::vec2(font.size, font.size))
    }

    fn paint_inline_widget(&self, ui: &mut egui::Ui, _text: &str, href: &str, rect: egui::Rect) {
        if href.starts_with(latermd_md::TASK_SCHEME) {
            crate::ui::preview::paint_task_checkbox(
                ui,
                rect,
                href,
                crate::ui::preview::TaskProbeDomain::Live(self.task_base),
            );
        } else if let Some(glyph) = href.strip_prefix(latermd_md::EMOJI_SCHEME) {
            let side = rect.width().min(rect.height());
            if side > 0.0 && ui.is_rect_visible(rect) {
                if let Some(texture) = crate::ui::emoji_panel::inline_texture(ui, glyph) {
                    let paint_rect =
                        egui::Rect::from_center_size(rect.center(), egui::vec2(side, side));
                    ui.painter().image(
                        texture.id(),
                        paint_rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
            }
        }
    }
}

/// 块的编辑代理:把 [`EditorBuffer`] 的**一块**暴露成 egui `TextBuffer`。
///
/// 与源码模式的 `EditorText` 同理(孤儿规则需本地 newtype),但 `as_str` 借的是
/// 镜像的切片、编辑按块首换算回全文 —— 因此文本只有一份真源。
struct BlockBuffer<'a> {
    buffer: &'a mut EditorBuffer,
    /// 块在全文中的字节区间。
    range: Range<usize>,
}

impl BlockBuffer<'_> {
    /// 块首的字符偏移(TextBuffer 的偏移是字符,块区间是字节)。
    fn base(&self) -> usize {
        self.buffer.byte_to_char(self.range.start)
    }

    /// 块文本切片;越界与落在字符中间都钳到字符边界(块边界来自 parser 的
    /// span,理论上是边界,但手改过的文本值得防御)。
    fn slice<'a>(text: &'a str, range: &Range<usize>) -> &'a str {
        let start = text.floor_char_boundary(range.start.min(text.len()));
        let end = text.floor_char_boundary(range.end.min(text.len()));
        &text[start..end.max(start)]
    }
}

impl egui::TextBuffer for BlockBuffer<'_> {
    fn is_mutable(&self) -> bool {
        true
    }

    fn as_str(&self) -> &str {
        Self::slice(self.buffer.text(), &self.range)
    }

    fn insert_text(&mut self, text: &str, char_index: egui::text::CharIndex) -> usize {
        self.buffer.insert_chars(self.base() + char_index.0, text);
        text.chars().count()
    }

    fn delete_char_range(&mut self, char_range: Range<egui::text::CharIndex>) {
        let base = self.base();
        self.buffer
            .remove_chars(base + char_range.start.0..base + char_range.end.0);
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<BlockBuffer<'static>>()
    }
}

/// 块的粗分类(块间前距的判定用,不做解析)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    /// ATX 标题(前距额外叠加 `heading_space_above`)。
    Heading,
    /// 块级元件:围栏代码 / 表格 / 分割线 / 引用 / 图片行 —— vendored 单
    /// 标签流对它们走 `before_block`/`after_block`(`block_spacing` 包围)。
    Widget,
    /// 普通文本流:段落与列表(列表在 vendored 里是文本行 + 标记,不是
    /// 块元件)。
    Text,
}

/// 按块源码首行做前缀粗分类。Setext 标题与「裸文本首行的 GFM 表格」不
/// 识别 —— 误分类只差一档间距(8px),不值得为它上解析器。
fn block_kind(block_text: &str) -> BlockKind {
    let first = block_text.lines().next().unwrap_or("");
    let t = first.trim_start();
    let hashes = t.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hashes) && t.as_bytes().get(hashes) == Some(&b' ') {
        return BlockKind::Heading;
    }
    if t.starts_with("```") || t.starts_with("~~~") || t.starts_with('>') || t.starts_with('|') {
        return BlockKind::Widget; // 围栏代码 / 引用 / 表格(常用形态)
    }
    // 分割线:整行只由 -*-_ 与空白构成(非空)
    if !t.is_empty() && t.bytes().all(|b| matches!(b, b'-' | b'*' | b'_' | b' ')) {
        return BlockKind::Widget;
    }
    if t.starts_with("![") {
        return BlockKind::Widget; // 图片行(vendored Image 是块元件)
    }
    BlockKind::Text
}

/// 正文行盒高:vendored `layout.rs::line_height_for` 同式(那边是私有
/// fn,字段全为公开样式值;上游若改式,最坏是间距观感偏移,不伤正确性)。
fn body_line_height(style: &MarkdownStyle, font_size: f32) -> f32 {
    (font_size * style.line_height_ratio).max(font_size * style.min_line_height_em + 0.75)
}

/// 块间前距:镜像 vendored **单标签流**的块间节奏(右栏预览即此口径)。
///
/// 预览把整篇塞进一个 `MarkdownLabel`,块间空气由三种机制天然产生:文
/// 本之间的空行是真实行盒、块级元件前后 `add_space(block_spacing)`
/// (`before_block`/`after_block`)、标题前再叠一行
/// `block_spacing + heading_space_above` 的透明 spacer。Live 是**逐块独
/// 立标签**,标签会裁掉尾随空行、spacer 在段首被刻意抑制,块间只剩
/// egui 出厂 3px —— 全部挤在一起(2026-10-10 用户反馈「Live 太紧凑」)。
/// 本函数把三种机制按边界类型补回:
///
/// * 前一块是元件:`block_spacing`(对应 `after_block`,元件后无空行);
/// * 前一块是文本:一个正文行盒高(对应真实空行);
/// * 本块是标题:再叠 `block_spacing + heading_space_above`(spacer 行);
/// * 本块是元件:再叠 `block_spacing`(`before_block`)。
fn block_gap(style: &MarkdownStyle, font_size: f32, prev: BlockKind, this: BlockKind) -> f32 {
    let base = match prev {
        BlockKind::Widget => style.block_spacing,
        BlockKind::Heading | BlockKind::Text => body_line_height(style, font_size),
    };
    let extra = match this {
        BlockKind::Heading => style.block_spacing + style.heading_space_above,
        BlockKind::Widget => style.block_spacing,
        BlockKind::Text => 0.0,
    };
    base + extra
}

/// 绘制 Live Preview:非活动块富渲染(点击即进入编辑),活动块源码编辑。
///
/// `selection` 是格式工具条的选区镜像出参(与源码模式 `ui::editor` 同一
/// 契约,见 `TabState::selection`):Live 的选区只存在于活动块 TextEdit 的
/// 持久 state,每帧抄出(文档坐标字符区间);无活动块 = 无选区,置 `None`。
///
/// 返回活动块 TextEdit 的响应(没有活动块时返回一块占位区域,便于测试定位)。
#[allow(clippy::too_many_arguments)]
pub fn ui(
    panel: &mut egui::Ui,
    editor: &mut EditorBuffer,
    preview: &mut PreviewState,
    cursor: &mut OutlineCursor,
    selection: &mut Option<(usize, usize)>,
    live: &mut LiveState,
    editor_id: egui::Id,
    show_typewriter: bool,
    show_focus: bool,
    keymap: &crate::keymap::Keymap,
    outbox: &mut Vec<Message>,
) -> egui::Response {
    live.sync(editor, cursor.byte);

    // 归约侧的大纲跳转、格式动作和测试/恢复路径可以预先指定活动块；
    // 这类明确的程序动作应直接进入编辑态。普通打开文档时活动块为空，
    // 仍保持浏览态，等待左键双击。
    if live.browse_only && live.active.is_some() {
        live.browse_only = false;
    }

    // 格式工具条的选区镜像(2026-10-10,live 格式动作的前半段):先按
    // 「无选区」起底,活动块分支内回填。起底防的是陈旧值 —— `tab.selection`
    // 是两种模式的共用槽,live 下不回填就会留着源码模式的旧值,归约侧
    // 拿它盲写会把 `**` 插到任意位置(曾是在 live 下点加粗的现行 bug)。
    *selection = None;

    // 按下帧焦点快照(#63,与右栏预览共用同一暂存):checkbox 命中帧要把
    // 它还给活动块 —— 点击 checkbox 的意图是切换勾选,不是转移焦点。预览
    // 面板同帧先画时它已收过首份快照(帧号核对,后画者不覆盖)。
    crate::ui::preview::stash_focus_on_press(panel.ctx());

    // 打字机模式(#64 M1):每标签一份记忆(与源码模式分槽),帧首读、
    // 帧末写回。关闭态不读写 temp、活动块不进任何打字机分支 —— LP2-3 的
    // 跟随路径与从前逐字节相同(否决线)。
    let mut tw = if show_typewriter {
        panel
            .ctx()
            .data(|d| {
                d.get_temp::<crate::ui::typewriter::Memory>(crate::ui::typewriter::live_memory_id(
                    editor_id,
                ))
            })
            .unwrap_or_default()
    } else {
        crate::ui::typewriter::Memory::default()
    };
    let tw_just_enabled = show_typewriter && !tw.enabled;
    // 闭包回写:编辑/路由落地帧(恢复事件)与打字机自身落地。
    let mut tw_resumed = false;
    let mut tw_landed = false;

    // 大纲/搜索跳转(LP2-3):与源码模式同一入口(`cursor.jump_to`)、同一
    // 时序口径 —— 当帧把目标块切成活动块并交 pending_caret,本帧闭包内
    // 即落地光标 + 请求滚动,不晚一帧。字符偏移是全文口径,先落块再换算
    // 块内偏移(块区间是字节;`char_to_byte`/`block_containing` 均对过期
    // 目标钳制)。空文档没有可落点,jump_to 已 take 即算消费,不悬置到
    // 将来的文档上。
    if let Some(char_idx) = cursor.jump_to.take() {
        let byte = editor.char_to_byte(char_idx);
        if let Some(block) = live.block_containing(byte) {
            let local = editor.byte_to_char(byte) - editor.byte_to_char(live.blocks[block].start);
            live.active = Some(block);
            live.browse_only = false;
            live.pending_caret = Some((block, local));
            live.pending_range = None;
            live.caret_follow = true;
            live.drag_anchor = None;
        }
    }

    let mut active_response: Option<egui::Response> = None;
    // macOS 会在切换输入源时短暂重置 IME 上下文。若这发生在 Live
    // TextEdit 的活动块上，egui 可能保留光标状态却丢掉 widget 焦点，
    // 下一帧就不再绘制插入光标。记住上一帧的焦点，在 IME 事件帧末尾
    // 把焦点交还给同一个稳定的块 id；源码编辑器也有同类的恢复通道。
    let mut ime_restore_focus: Option<egui::Id> = None;
    let mut activate: Option<(usize, usize)> = None;
    let mut route: Option<(usize, usize)> = None;
    // 选区 AI 浮标(#61 M1)的锚点素材:活动块持久选区(**文档坐标**字符
    // 区对,块内捕获时换算)与尾端光标条屏幕矩形,活动块分支内捕获、
    // ScrollArea 之后消费(命中区同层末尾注册,与源码模式同一纪律)。
    let mut sel_ai_selection: Option<(usize, usize)> = None;
    let mut sel_ai_anchor: Option<egui::Rect> = None;
    let mut sel_ai_response_id = egui::Id::NULL;
    let block_count = live.blocks.len();
    // mermaid 块出图(#51 M3):富渲染块经只拦 mermaid 的 handler 接入
    // block_code_widget 扩展点。专用 handler(而非复用 AiLinkHandler):
    // Live 列此前不接任何 handler,不能借 mermaid 顺手改变 ai 卡/emoji/
    // wikilink 的渲染行为(否决线);其余块照走 vendored 默认。
    let mermaid_handler = crate::ui::mermaid::LiveMermaidHandler::new();

    let scrolled = egui::ScrollArea::vertical()
        .id_salt("live-preview")
        .auto_shrink([false, false])
        .show(panel, |ui| {
            // 块间节奏完全由 [`block_gap`] 前距决定(与右栏预览同口径,
            // 2026-10-10):egui 出厂 item_spacing.y = 3px 会叠加在演算值
            // 上,归零让前距成为唯一间距源。
            ui.spacing_mut().item_spacing.y = 0.0;
            if block_count == 0 {
                if live.browse_only {
                    // 空文档的浏览态没有可渲染块，只显示非编辑提示。双击
                    // 提示后下一帧才创建 TextEdit，避免打开新文档就抢焦点。
                    let response = ui.add(
                        egui::Label::new(
                            egui::RichText::new("双击此处开始写作")
                                .color(ui.visuals().weak_text_color()),
                        )
                        .sense(egui::Sense::click()),
                    );
                    if response.double_clicked() {
                        live.browse_only = false;
                    }
                } else {
                    // 写作态使用共享缓冲，不另建镜像。
                    let mut buffer = BlockBuffer {
                        buffer: editor,
                        range: 0..0,
                    };
                    let response_id = editor_id.with("live-empty");
                    let output = egui::TextEdit::multiline(&mut buffer)
                        .id(response_id)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .desired_rows(1)
                        .hint_text("从这里开始写作")
                        .lock_focus(true)
                        .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 2)))
                        .show(ui);
                    cursor.byte = output
                        .state
                        .cursor
                        .char_range()
                        .map(|range| editor.char_to_byte(range.primary.index.0));
                    // 右键菜单:空文档块与活动块同一挂点(条目表共用)。
                    // 排在 active_response 之前 —— 那条会移走 response。
                    crate::ui::editor_menu::attach(ui, &output, keymap, true, outbox);
                    active_response = Some(output.response.response);
                }
            }
            // 前一块的分类(首块无前距:vendored `had_content` 同语义,
            // 文档顶贴顶不悬空)。
            let mut prev_kind: Option<BlockKind> = None;
            for index in 0..block_count {
                let range = live.blocks[index].clone();
                // 块文本直接借用共享缓冲；渲染缓存持有改写结果，稳态帧不再复制整块。
                let block_text = BlockBuffer::slice(editor.text(), &range);
                let kind = block_kind(block_text);
                if let Some(prev) = prev_kind {
                    let style = egui_markdown_style::global_style(ui.ctx());
                    ui.add_space(block_gap(
                        &style,
                        crate::theme::editor_font_size(ui.ctx()),
                        prev,
                        kind,
                    ));
                }
                prev_kind = Some(kind);
                if !live.browse_only && live.active == Some(index) {
                    let char_len = live.block_char_len(editor, index);
                    let lines = block_text.lines().count().max(1);
                    let mut buffer = BlockBuffer {
                        buffer: editor,
                        range: range.clone(),
                    };
                    // 稳定 id:按块序号(不带长度/hash),AGENTS §6.7 的红线
                    let response_id = editor_id.with(("live-block", index));
                    let focus_memory_id = editor_id.with(("live-ime-focus", index));
                    let focused_last_frame = ui
                        .ctx()
                        .data(|data| data.get_temp::<bool>(focus_memory_id).unwrap_or(false));
                    let edit = egui::TextEdit::multiline(&mut buffer)
                        .id(response_id)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .desired_rows(lines.clamp(1, 40))
                        // 与源码编辑器同样锁定焦点。活动块仍可由本模块的
                        // 双击/单击路由显式切换，普通工具条、滚动和预览
                        // 控件不会让写作插入点被 egui 无意收回。
                        .lock_focus(true);
                    // 活动块无输入框式外框(2026-10-10 mac 精修全平台化,
                    // #169):只留文字内距,光标/选区/IME 通路不变
                    let edit =
                        edit.frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 2)));
                    let output = edit.show(ui);
                    // 右键菜单(context-menu-plan §3 M1):活动块与源码
                    // 编辑器同一挂点,条目表共用;块内坐标落光标见
                    // editor_menu::attach。
                    crate::ui::editor_menu::attach(ui, &output, keymap, true, outbox);
                    let focused_now = ui.ctx().memory(|mem| mem.has_focus(response_id));
                    let ime_event = ui.ctx().input(|input| {
                        input
                            .events
                            .iter()
                            .any(|event| matches!(event, egui::Event::Ime(_)))
                    });
                    // 不在输入事件帧直接抢焦点：等这个活动块的所有绘制和
                    // 命中探针完成后再恢复，避免后续控件把恢复请求覆盖。
                    // 指针按下表示用户正在把焦点交给别的控件，此时让出。
                    if focused_last_frame
                        && ime_event
                        && !ui.ctx().input(|input| input.pointer.any_pressed())
                    {
                        ime_restore_focus = Some(response_id);
                    }
                    // egui-winit 的自动 IME spot 以整个 TextEdit 矩形为锚点，
                    // 输入法切换时会把候选框临时移到块首。活动块有稳定光标时
                    // 同帧补发光标矩形，保证中文候选框和插入点仍贴在原位置。
                    if ime_event && (focused_now || ime_restore_focus == Some(response_id)) {
                        if let Some(caret) = output
                            .state
                            .cursor
                            .char_range()
                            .map(|range| range.primary.index.0)
                        {
                            let caret = output
                                .galley
                                .pos_from_cursor(egui::text::CCursor::new(caret));
                            let caret_rect = egui::Rect::from_min_max(
                                output.galley_pos
                                    + caret.min.to_vec2()
                                    + egui::vec2(0.0, caret.height()),
                                output.galley_pos
                                    + caret.max.to_vec2()
                                    + egui::vec2(0.0, caret.height()),
                            );
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::IMERect(caret_rect));
                        }
                    }
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(
                            focus_memory_id,
                            focused_now || ime_restore_focus == Some(response_id),
                        )
                    });

                    // 光标(块内字符偏移):跨块路由判定、键盘跟随与回填共用
                    let caret = output
                        .state
                        .cursor
                        .char_range()
                        .map(|range| range.primary.index.0);
                    // 跨块 caret 路由:块首按 ↑ 去上一块末尾;块尾按 ↓ 去下一块开头
                    if output.response.response.has_focus() {
                        let at_start = caret == Some(0);
                        let at_end = caret == Some(char_len);
                        if ui.input(|input| input.key_pressed(egui::Key::ArrowUp))
                            && at_start
                            && index > 0
                        {
                            let target = index - 1;
                            route = Some((target, live.block_char_len(editor, target)));
                        } else if ui.input(|input| input.key_pressed(egui::Key::ArrowDown))
                            && at_end
                            && index + 1 < block_count
                        {
                            route = Some((index + 1, 0));
                        }
                    }

                    // 光标回填(大纲/状态栏用):块内字符偏移 + 块首
                    cursor.byte = output.state.cursor.char_range().map(|range| {
                        editor.char_to_byte(
                            editor.byte_to_char(live.blocks[index].start) + range.primary.index.0,
                        )
                    });
                    // 选区 AI 浮标(#61 M1)素材:块内持久选区(#38 同源读法)
                    // + 尾端光标条屏幕矩形(galley pos_from_cursor,IME 同先例),
                    // ScrollArea 之后消费。选区当帧换算成**文档坐标**字符区间
                    // (块基为字节偏移,块内光标是字符偏移,与光标回填同款换算)
                    // —— M2 起随动作消息带走做插入点捕获,归约不读
                    // `TabState::selection`(该字段只有源码模式回填)。活动块
                    // 是唯一可编辑块,块基不受本帧编辑影响,换算恒成立。
                    if let Some(range) = output.state.cursor.char_range() {
                        let block_base = editor.byte_to_char(live.blocks[index].start);
                        sel_ai_selection = Some((
                            block_base + range.primary.index.0,
                            block_base + range.secondary.index.0,
                        ));
                        // 同值镜像给格式工具条(帧首已起底 None):活动块是
                        // 唯一可编辑块,块基不受本帧编辑影响,换算恒成立
                        // (与上一行选区 AI 的换算同一条)。
                        *selection = sel_ai_selection;
                        let tail = range.primary.index.0.max(range.secondary.index.0);
                        let rect = output
                            .galley
                            .pos_from_cursor(egui::text::CCursor::new(tail));
                        sel_ai_anchor = Some(egui::Rect::from_min_max(
                            output.galley_pos + rect.min.to_vec2(),
                            output.galley_pos + rect.max.to_vec2(),
                        ));
                    }
                    sel_ai_response_id = response_id;
                    // LP2-1 内联标记半隐藏 + LP2-2 选区扩展。显形集合与扩
                    // 展选区都是纯函数(`latermd_md::mark_interaction`),绘
                    // 制仍是纯叠加 —— 不挂 widget(不参与命中测试)。交互
                    // 边界:双击帧把 egui 选出的词扩到最内层标记对;拖选松
                    // 手帧只有「锚点压着标记发起」的拖选才扩(从内容中部
                    // 发起的普通拖选保持原样);键盘选区/三击选行不扩展。
                    // 光标取 `output.state`(帧内真值)而非 `output.cursor_
                    // range`:后者在焦点门控的键盘事件段产出,指针交互(选
                    // 词/拖选/按压塌缩)发生在其后、只改 state —— 用陈旧值
                    // 会把按压点读成旧光标。
                    if let Some(cursor_range) = output.state.cursor.char_range() {
                        let rev = editor.revision();
                        if !matches!(&live.marks, Some((r, i, _)) if *r == rev && *i == index) {
                            // 与 galley 同一份文本:本帧的编辑发生在 show()
                            // 内部,这里取的也是编辑后的同一切片,缓存键即
                            // 编辑后的修订号 —— 区间与字形永远对得上
                            live.marks = Some((
                                rev,
                                index,
                                latermd_md::inline_marks_with_pairs(BlockBuffer::slice(
                                    editor.text(),
                                    &range,
                                )),
                            ));
                        }
                        let bundle: &latermd_md::InlineMarks = match &live.marks {
                            Some((_, _, bundle)) => bundle,
                            None => unreachable!("缓存已在上一行建立"),
                        };
                        let caret = cursor_range.primary.index.0;
                        let other = cursor_range.secondary.index.0;

                        // 手势判定。egui 在 show() 内部已处理双击选词(词选
                        // 区就在本帧的 cursor_range 里)与按下塌缩光标;这里
                        // 只读信号:双击 → 扩词;松手 + 非空选区 + 锚点 → 扩
                        // 拖选;按下帧记/清锚点(按在别处清,防跨按压串帧)。
                        let double_clicked = output.response.response.double_clicked();
                        let primary_pressed = ui.input(|input| input.pointer.primary_pressed());
                        let primary_released = ui.input(|input| input.pointer.primary_released());
                        if primary_pressed {
                            live.drag_anchor = if output.response.response.contains_pointer() {
                                Some(caret)
                            } else {
                                None
                            };
                        }
                        let gesture = if double_clicked {
                            live.drag_anchor = None;
                            Some(latermd_md::MarkGesture::DoubleClick)
                        } else if primary_released {
                            match (live.drag_anchor.take(), caret != other) {
                                (Some(anchor), true) => {
                                    Some(latermd_md::MarkGesture::DragRelease { anchor })
                                }
                                _ => None,
                            }
                        } else {
                            None
                        };

                        let interaction = latermd_md::mark_interaction(
                            BlockBuffer::slice(editor.text(), &range),
                            bundle,
                            caret,
                            other,
                            gesture,
                        );
                        paint_mark_fades(
                            ui,
                            &output.galley,
                            output.galley_pos,
                            BlockBuffer::slice(editor.text(), &range),
                            &bundle.segments,
                            &interaction.revealed,
                        );
                        // 扩展写回:与 pending_caret 同一条通道(改 TextEdit
                        // 光标状态,下一帧生效);primary 方向保留手势原方向。
                        if let Some(expanded) = interaction.expanded {
                            let (low, high) = (expanded.start, expanded.end);
                            if low != caret.min(other) || high != caret.max(other) {
                                let mut state = output.state.clone();
                                let (primary, secondary) = if caret <= other {
                                    (low, high)
                                } else {
                                    (high, low)
                                };
                                state
                                    .cursor
                                    .set_char_range(Some(egui::text::CCursorRange::two(
                                        egui::text::CCursor::new(primary),
                                        egui::text::CCursor::new(secondary),
                                    )));
                                state.store(ui.ctx(), response_id);
                            }
                        }
                    }
                    let response = output.response.response;
                    // 活动块跟随(LP2-3,#29「帧内一次性标志」同款):只认
                    // 两类帧 —— 键盘行/页导航键且活动块持焦、pending_caret
                    // 落地帧(跨块路由/大纲跳转)。标志是帧内局部变量,当帧
                    // 即焚;滚轮、拖滚动条、空闲帧一概不跟随,徒手滚动不被
                    // 拽回光标处。egui 内建跟随只覆盖「事件处理改了选区」的
                    // 帧(它按帧内前后选区比较),程序化写回改 state 再
                    // store,内建永远比不出变化 —— 路由/跳转的落地帧必须在
                    // 这里补,这是本层存在的硬理由;键盘导航帧内建本会跟
                    // 随,这里再补一层是显式化并与源码模式(#29)对齐口
                    // 径,不依赖内建的比较时机。跨块路由帧本身不跟随:光标
                    // 这一帧还在旧块,下一帧 pending 落地时随写回同帧跟随,
                    // 先滚旧位置只会与目标抢动画。
                    let mut follow: Option<usize> = None;
                    if route.is_none() {
                        let nav_key = ui.input(|input| {
                            input.events.iter().any(|event| {
                                matches!(
                                    event,
                                    egui::Event::Key {
                                        key: egui::Key::ArrowUp
                                            | egui::Key::ArrowDown
                                            | egui::Key::Home
                                            | egui::Key::End
                                            | egui::Key::PageUp
                                            | egui::Key::PageDown,
                                        pressed: true,
                                        ..
                                    }
                                )
                            })
                        });
                        if nav_key && ui.ctx().memory(|mem| mem.has_focus(response_id)) {
                            follow = caret;
                        }
                        // 打字机模式(#64 M1)的触发面扩展:活动块持焦的
                        // **修订号前进帧**(打字/IME/undo/redo —— 修订号是
                        // 不依赖事件枚举的编辑信号)、键盘导航帧(接管后的
                        // 恢复事件,与源码模式同口径)与开关开启帧也跟随。
                        // 关闭态整块不进,LP2-3 路径原样。
                        if show_typewriter {
                            let edited = tw.last_rev.is_none_or(|rev| rev != editor.revision());
                            if (edited || nav_key || tw_just_enabled)
                                && ui.ctx().memory(|mem| mem.has_focus(response_id))
                            {
                                follow = follow.or(caret);
                                tw_resumed = true;
                                tw.phase = crate::ui::typewriter::step(
                                    tw.phase,
                                    crate::ui::typewriter::Turn::Edit,
                                );
                            }
                        }
                    }
                    // 上一帧交过来的光标:落到本块的指定字符偏移并要焦点
                    // (放在回填之后 —— `output.state` 在这里被移走)。写回
                    // 与视口跟随同帧落地:跟随目标取写回偏移(本帧 state 是
                    // 写回前的旧快照,但 galley 文本未变,行位置即目标行)。
                    if let Some((block, char_idx)) = live.pending_caret {
                        if block == index {
                            let id = response.id;
                            let mut state = output.state;
                            // 落地光标:格式入口(live 归约)附带 pending_range
                            // → 落成「内容仍选中」;其余入口塌缩单点(既有口径)。
                            match live.pending_range.take() {
                                Some((primary, secondary)) => {
                                    state.cursor.set_char_range(Some(
                                        egui::text::CCursorRange::two(
                                            egui::text::CCursor::new(primary),
                                            egui::text::CCursor::new(secondary),
                                        ),
                                    ));
                                }
                                None => {
                                    state.cursor.set_char_range(Some(
                                        egui::text::CCursorRange::one(egui::text::CCursor::new(
                                            char_idx,
                                        )),
                                    ));
                                }
                            }
                            state.store(ui.ctx(), id);
                            ui.ctx().memory_mut(|mem| mem.request_focus(id));
                            live.pending_caret = None;
                            if live.caret_follow {
                                follow = Some(char_idx);
                                // 跨块路由/大纲跳转 = 光标移动 = 打字机恢复
                                // 事件(#121 口径)
                                if show_typewriter {
                                    tw_resumed = true;
                                    tw.phase = crate::ui::typewriter::step(
                                        tw.phase,
                                        crate::ui::typewriter::Turn::Edit,
                                    );
                                }
                            }
                            live.caret_follow = false;
                        }
                    }
                    if let Some(index) = follow {
                        let row_rect = output
                            .galley
                            .pos_from_cursor(egui::text::CCursor::new(index));
                        if show_typewriter {
                            // 打字机落地:与源码模式同一决策面(1/3 目标线 +
                            // 死区),单像素矩形 + TOP 对齐一次到位,文首/
                            // 文末钳位由 egui end() 兜底。
                            let clip = ui.clip_rect();
                            let cursor_screen_y = output.galley_pos.y + row_rect.min.y;
                            let ask = crate::ui::typewriter::ScrollAsk {
                                view_y: cursor_screen_y - clip.top(),
                                row_h: row_rect.height().max(1.0),
                                viewport: clip.height(),
                            };
                            if let Some(delta) = crate::ui::typewriter::scroll_delta(&ask) {
                                let land_rect = egui::Rect::from_min_size(
                                    egui::pos2(
                                        clip.left() + 1.0,
                                        clip.top() + delta + ui.spacing().item_spacing.y,
                                    ),
                                    egui::vec2(1.0, 1.0),
                                );
                                ui.scroll_to_rect_animation(
                                    land_rect,
                                    Some(egui::Align::TOP),
                                    egui::style::ScrollAnimation::none(),
                                );
                                tw_landed = true;
                            }
                        } else {
                            ui.scroll_to_rect(
                                egui::Rect::from_min_max(
                                    output.galley_pos + row_rect.min.to_vec2(),
                                    output.galley_pos + row_rect.max.to_vec2(),
                                )
                                .expand(1.5),
                                None,
                            );
                        }
                    }
                    active_response = Some(response);
                } else {
                    // 富渲染。区域用渲染前后的 cursor 差值框出来;「点击进
                    // 编辑」的命中不走 egui widget,理由见 [`clicked_for_edit`]。
                    let top = ui.cursor().top();
                    // 链接改写(#63 checkbox + #65 高亮):块文本先过改写层
                    // (缓存键是源码指纹 + 块序号,稳态帧零解析),`task://`
                    // 链接由本块专属 handler 画成自绘 checkbox、`hl://` 画
                    // 成底色段 —— 探针坐标域带块首,点击切换由 `ui` 帧末的
                    // 探针命中判定统一发出。无目标字符的块改写恒等,渲染
                    // 零变化。
                    let (block_rendered, offset_map, highlight_map, emoji_map, task_map) =
                        live.rich_block(index, editor.revision(), block_text);
                    // 字体与右栏预览同源(#23 F3):size = 用户字号偏好,
                    // 族 = 预览专用族(#43 M2 的行 metrics 对齐副本,无 CJK
                    // 回落 Proportional)。源码/Live 两种模式下排版偏好同观感。
                    let rich_handler = LiveRichHandler::new(
                        &mermaid_handler,
                        range.start,
                        ui.visuals().text_color(),
                        matches!(ui.ctx().theme(), egui::Theme::Dark),
                    );
                    MarkdownLabel::new(
                        editor_id.with(("live-render", index)),
                        block_rendered.as_ref(),
                    )
                    .font(egui::FontId::new(
                        crate::theme::editor_font_size(ui.ctx()),
                        crate::fonts::preview_body_family(ui.ctx()),
                    ))
                    .wrap()
                    .overflow_wrap(egui_markdown::OverflowWrap::BreakAll)
                    // 代码块复制头(#38)与右栏预览同一份:Live 模式的
                    // 富渲染块也是「code 预览的地方」。
                    .scroll_code_blocks(true)
                    .shrink_code_blocks(true)
                    .code_block_min_width(Some(200.0))
                    .code_block_buttons(&crate::ui::preview::code_copy_buttons)
                    // mermaid 块出图(#51 M3)+ 任务 checkbox(#63),
                    // 与右栏预览同一渲染入口。
                    .link_handler(&rich_handler)
                    .show(ui);
                    rich_handler.drain_into(outbox);
                    let bottom = ui.cursor().top();
                    let rect = egui::Rect::from_min_max(
                        egui::pos2(ui.min_rect().left(), top),
                        egui::pos2(ui.min_rect().right(), bottom.max(top + 1.0)),
                    );
                    // 专注模式(#64 M2):非活动块整块蒙一层背景色遮罩(#81
                    // 内联标记半隐藏同款纯绘制 —— 后画覆盖,不注册交互、
                    // 不参与命中测试),淡化块的点击仍走 `clicked_for_edit`
                    // 进入编辑,进入即变活动块、下帧起不再淡化。关闭态整块
                    // 不进(逐像素现状,否决线)。
                    if show_focus && crate::ui::focus::dimmed(live.active, index) {
                        paint_block_fade(ui, rect);
                    }
                    // 点击进编辑(#170):光标落**点击处**而非块尾。点击帧
                    // 富渲染块刚画出,帧内命中表把点击点映到渲染文本字符,
                    // 经两张改写映射逆穿回块源码字节;文本 galley 之外回退
                    // 最近子块起点,全落空(块被剔除等)保持旧行为落块尾。
                    // 落点不滚动跟随:指针刚把视口定位到点击处。
                    // 浏览态只响应滚动和左键双击，不因单击抢入插入模式；
                    // 已经进入写作态后，单击其他块才切换活动编辑块。
                    let edit_pos = if live.browse_only {
                        live.browse_enabled
                            .then(|| clicked_for_edit(ui, rect, editor_id, true))
                            .flatten()
                    } else {
                        clicked_for_edit(ui, rect, editor_id, false)
                    };
                    if let Some(pos) = edit_pos {
                        let label_id = editor_id.with(("live-render", index));
                        let block_source = BlockBuffer::slice(editor.text(), &range);
                        let local = LiveState::click_caret_local(
                            ui,
                            label_id,
                            pos,
                            block_rendered.as_ref(),
                            offset_map.as_ref(),
                            highlight_map.as_ref(),
                            emoji_map.as_ref(),
                            task_map.as_ref(),
                            block_source.len(),
                        )
                        .map(|byte| {
                            editor.byte_to_char(range.start + byte)
                                - editor.byte_to_char(range.start)
                        })
                        .unwrap_or_else(|| live.block_char_len(editor, index));
                        activate = Some((index, local));
                    }
                }
            }
        });

    let focus_target_changed = activate.is_some() || route.is_some();
    if let Some((index, local)) = activate.take() {
        live.browse_only = false;
        live.active = Some(index);
        live.pending_caret = Some((index, local));
        live.pending_range = None;
        // 点击进编辑不跟随:指针刚把视口定位到点击处(decisions-pending #83)
        live.caret_follow = false;
        live.drag_anchor = None;
    }

    // 打字机接管判定(#64 M1):本帧偏移动了、且不是打字机自己落的地、
    // 也不是编辑/路由落地帧 —— 即用户主动滚动,记一次 Override 暂停。
    // 已知边界(如实):活动块切换引起内容高度变化的 offset 钳位也会被
    // 记一次(无法与用户滚动区分),后果 = 一次无害的暂停,下次编辑即
    // 恢复(取舍登记 decisions-pending #121)。帧末写回记忆。
    if show_typewriter {
        let in_grace = tw.land_grace > 0;
        let moved =
            (scrolled.state.offset.y - tw.last_offset).abs() > crate::ui::typewriter::MOVED_EPSILON;
        if moved && !tw_landed && !tw_resumed && !in_grace {
            tw.phase =
                crate::ui::typewriter::step(tw.phase, crate::ui::typewriter::Turn::UserScroll);
        }
        tw.land_grace = if tw_landed {
            crate::ui::typewriter::LAND_GRACE_FRAMES
        } else {
            tw.land_grace.saturating_sub(1)
        };
        tw.last_offset = scrolled.state.offset.y;
        tw.last_rev = Some(editor.revision());
        tw.enabled = true;
        panel
            .ctx()
            .data_mut(|d| d.insert_temp(crate::ui::typewriter::live_memory_id(editor_id), tw));
    }
    if let Some(route) = route.take() {
        live.browse_only = false;
        live.active = Some(route.0);
        live.pending_caret = Some(route);
        live.pending_range = None;
        // 跨块路由:光标去了别的块,落地帧视口必须同帧跟上
        live.caret_follow = true;
        live.drag_anchor = None;
    }

    // IME 切换造成的临时失焦在整帧命中处理完成后恢复。若用户同帧点击
    // 了另一个块，`activate`/`route` 已经改变活动目标，此请求不会覆盖
    // 新目标的焦点交接。
    if !focus_target_changed {
        if let Some(id) = ime_restore_focus {
            panel.ctx().memory_mut(|mem| mem.request_focus(id));
        }
    }

    // 任务 checkbox 的帧末命中(#63,与右栏预览同一探针纪律):全部块
    // 画完后读本帧探针,点击落在 Live 域某枚占位区内 → 切换消息(块内
    // 载荷 + 记录时块首 = 源码偏移;过期偏移由归约侧 pulldown 重扫闸住),
    // 并归还按下帧持焦者 —— vendored 的 handler.click 对列表项 widget
    // 不可达(实测断零消息),这里是 Live 侧唯一切换入口。
    if let Some((href, crate::ui::preview::TaskProbeDomain::Live(base))) =
        crate::ui::preview::task_click_hit(panel)
    {
        if let Some((_, offset)) = crate::ui::preview::parse_task_href(&href) {
            outbox.push(Message::TaskCheckboxToggled {
                byte: base + offset,
            });
            crate::ui::preview::restore_stashed_focus(panel.ctx());
        }
    }

    // 选区 AI 浮标(#61 M1):活动块持久选区 + 持焦 + 无拖拽进行中才弹,
    // 判定与命中区注册与源码模式同一套(`selection_ai`);命中区在 ScrollArea
    // 之后同层注册(13a 纪律),尾端随滚动出视口即隐。按在浮标/菜单上的
    // 帧,egui 内建清焦(widget 创建段)已把活动块焦点吃掉 —— 以上一帧
    // 浮标矩形为准先还焦再判可见性(与源码模式同一手法)。
    {
        let prev_floater = crate::ui::selection_ai::hit_rects(panel.ctx(), editor_id);
        let pointer = panel.ctx().input(|input| input.pointer.interact_pos());
        if sel_ai_response_id != egui::Id::NULL
            && pointer.is_some_and(|pos| prev_floater.iter().any(|rect| rect.contains(pos)))
        {
            panel
                .ctx()
                .memory_mut(|mem| mem.request_focus(sel_ai_response_id));
        }
    }
    let sel_focused = panel.ctx().memory(|mem| mem.has_focus(sel_ai_response_id));
    let sel_anchor = if crate::ui::selection_ai::badge_visible(
        sel_ai_selection,
        sel_focused,
        // 拖选进行中不弹;按在浮标自己上(dragged_id = 浮标)不算拖选。
        panel
            .ctx()
            .dragged_id()
            .is_some_and(|id| !crate::ui::selection_ai::is_floater_id(editor_id, id)),
    ) {
        sel_ai_anchor
    } else {
        None
    };
    crate::ui::selection_ai::show(
        panel,
        editor_id,
        sel_ai_selection,
        sel_anchor,
        scrolled.inner_rect,
        outbox,
    );

    // 快照同步(与源码模式同一条规则:仅修订号前进时重建)
    if preview.synced_rev != editor.revision() {
        preview.rebuild(editor);
    }
    active_response
        .unwrap_or_else(|| panel.allocate_response(egui::vec2(0.0, 0.0), egui::Sense::click()))
}

/// 富渲染块「点击进入编辑」的命中检测。**不挂 egui widget**:块级矩形一旦
/// 参与 hit-test,注册序必与块内其它可点件二选一 —— egui 0.36 同层点击
/// 平局取后注册者(hit_test.rs「In tie, pick last = topmost」),后注册的
/// 块矩形抢走代码块复制头(#38)的点击(复制失效、块误进编辑);先注册
/// 又抢不过富渲染体(`MarkdownLabel` 的 `Sense::click_and_drag` 整块响应
/// 在 `show()` 内注册得更晚,实测点击后进编辑同样失效)。所以直接读原始
/// 指针事件:本帧有主键 click(egui 已按拖动阈值判定,块内拖选文本不算
/// click,不误触发)且抬起点落在本块;再排除两类本就另有归属的点击 ——
/// 落在代码块复制按钮上的(本帧探针几何,`crate::ui::preview::
/// copy_button_rects`)和打开了链接的(链接点击归属富渲染层,不连带进
/// 编辑;OpenUrl 在 `show()` 内已进本帧输出,这里读得到)。
///
/// 抬起点核对而按不下起点核对:`press_origin` 在抬起帧已被 egui 清空
/// (input_state 释放即置 None);好在 `primary_clicked` 本身就含「未超出
/// 点击距离」判定,残余歧义最多块边界 max_click_dist 一线。
///
/// 命中时返回抬起点(点击点):进编辑的光标落点换算(#170)以它为输入。
fn clicked_for_edit(
    ui: &egui::Ui,
    rect: egui::Rect,
    editor_id: egui::Id,
    double_click: bool,
) -> Option<egui::Pos2> {
    let clicked = ui.input(|input| {
        if double_click {
            input
                .pointer
                .button_double_clicked(egui::PointerButton::Primary)
        } else {
            input.pointer.primary_clicked()
        }
    });
    if !clicked {
        return None;
    }
    let pos = ui.input(|input| input.pointer.interact_pos())?;
    if !rect.contains(pos) {
        return None;
    }
    if crate::ui::preview::copy_button_rects(ui.ctx())
        .iter()
        .any(|button| button.contains(pos))
    {
        return None;
    }
    // 任务 checkbox(#63):点击归 checkbox 切换(帧内探针几何,由 paint
    // 先于本判定写入),不连带进编辑。
    if crate::ui::preview::task_checkbox_rects(ui.ctx())
        .iter()
        .any(|checkbox| checkbox.contains(pos))
    {
        return None;
    }
    // 选区 AI 浮标/菜单(#61)压在本块上时,点击归浮标(探针是上一帧的
    // 几何 —— 浮标本帧在场,下一帧才有点击可落,一帧滞后无影响)。
    if crate::ui::selection_ai::hit_rects(ui.ctx(), editor_id)
        .iter()
        .any(|hit| hit.contains(pos))
    {
        return None;
    }
    (!ui.ctx().output(|output| {
        output
            .commands
            .iter()
            .any(|cmd| matches!(cmd, egui::OutputCommand::OpenUrl(_)))
    }))
    .then_some(pos)
}

/// 渲染文本的第 `index` 个字符的字节偏移(越界 = 文本末尾):帧内命中
/// 给的是字符索引,改写映射吃字节偏移。
fn char_indices_to_byte(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(byte, _)| byte)
}

/// 标记半隐藏的遮罩不透明度(遮罩色 = 编辑框背景色)。「弱化但可辨识」,
/// 不做用户可调的透明度配置面(规格口径)。
const MARK_FADE_ALPHA: u8 = 166;

/// 专注模式(#64 M2)的块级遮罩:编辑框背景色 × α 蒙在整块上(含块内
/// checkbox/复制头等自绘件 —— 它们的命中探针独立于绘制,点击行为不受
/// 影响)。遮罩不透明度与可读性折算见 [`crate::ui::focus::DIM_ALPHA`]。
fn paint_block_fade(ui: &egui::Ui, rect: egui::Rect) {
    let bg = ui.visuals().text_edit_bg_color();
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius::ZERO,
        egui::Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), crate::ui::focus::DIM_ALPHA),
    );
}

/// 把活动块里的内联标记以半透明遮罩弱化(LP2-1)。
///
/// 遮罩 = 编辑框背景色 × α 叠在标记字形上,视觉即「标记变淡」,不改
/// TextEdit 的任何状态。显形与否由调用方经 [`latermd_md::mark_interaction`]
/// 算好(`revealed` 与 `marks` 平行):光标/选区贴上的段显形,被触碰标
/// 记的**配对另一侧**所在段也显形(LP2-2)。
fn paint_mark_fades(
    ui: &egui::Ui,
    galley: &egui::Galley,
    galley_pos: egui::Pos2,
    block_text: &str,
    marks: &[Range<usize>],
    revealed: &[bool],
) {
    if marks.is_empty() {
        return;
    }
    let bg = ui.visuals().text_edit_bg_color();
    let fade = egui::Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), MARK_FADE_ALPHA);
    let painter = ui.painter();
    // 标记是字节区间,TextEdit 的光标是字符偏移(相对同一份块文本);一次
    // 遍历换算全部区间(标记已升序,逐段各扫一遍前文是 O(len×段数))
    let mut chars = 0usize;
    let mut walk = block_text.char_indices().peekable();
    for (index, mark) in marks.iter().enumerate() {
        while walk.next_if(|(byte, _)| *byte < mark.start).is_some() {
            chars += 1;
        }
        let start = chars;
        while walk.next_if(|(byte, _)| *byte < mark.end).is_some() {
            chars += 1;
        }
        if revealed.get(index).copied().unwrap_or(false) {
            continue;
        }
        for rect in galley_mark_rects(galley, start..chars) {
            painter.rect_filled(
                rect.translate(galley_pos.to_vec2()),
                egui::CornerRadius::ZERO,
                fade,
            );
        }
    }
}

/// 标记字符区间(字符偏移)在 galley 里占的矩形(相对 galley 原点),按行
/// 切分 —— 折行/换行会把一段标记拆成多行多矩形。
fn galley_mark_rects(galley: &egui::Galley, char_range: Range<usize>) -> Vec<egui::Rect> {
    let mut rects = Vec::new();
    let mut row_start = 0;
    for row in &galley.rows {
        let row_chars = row.char_count_excluding_newline().0;
        let start = char_range.start.max(row_start);
        let end = char_range.end.min(row_start + row_chars);
        if end > start {
            let x0 = row.pos.x + row.x_offset(egui::text::CharIndex(start - row_start));
            let x1 = row.pos.x + row.x_offset(egui::text::CharIndex(end - row_start));
            rects.push(egui::Rect::from_min_max(
                egui::pos2(x0.min(x1), row.min_y()),
                egui::pos2(x0.max(x1), row.max_y()),
            ));
        }
        row_start += row.char_count_including_newline().0;
        if row_start >= char_range.end {
            break;
        }
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(text: &str) -> (EditorBuffer, LiveState, OutlineCursor) {
        let editor = EditorBuffer::new(text);
        let mut live = LiveState::default();
        live.sync(&editor, None);
        (editor, live, OutlineCursor::default())
    }

    /// 块表连续覆盖全文(安全底线:落在块外的字节会在编辑时丢失)。
    #[test]
    fn blocks_cover_the_whole_document() {
        let (editor, live, _) = state_with("# 标题\n\n正文\n\n```rust\nfn a(){}\n```\n");
        let mut cursor = 0;
        for block in &live.blocks {
            assert_eq!(block.start, cursor, "{live:?}");
            cursor = block.end;
        }
        assert_eq!(cursor, editor.text().len());
    }

    /// 修订号不变就不重算块(每次编辑都重解析会拖垮打字)。
    #[test]
    fn sync_only_recomputes_when_revision_advances() {
        let (mut editor, mut live, _) = state_with("a\n\nb\n");
        let before = live.blocks.clone();
        live.sync(&editor, None);
        assert_eq!(live.blocks, before, "未编辑不重算");
        editor.insert_chars(0, "x");
        live.sync(&editor, None);
        assert_ne!(live.blocks, before, "编辑后重算");
    }

    /// 浏览态同步块表时不能把源码光标复活成活动块，否则打开文档会
    /// 立即显示 TextEdit；活动块只能由双击或明确的程序跳转设置。
    #[test]
    fn browse_sync_does_not_activate_cursor_block() {
        let editor = EditorBuffer::new("第一段\n\n第二段\n");
        let mut live = LiveState {
            browse_only: true,
            browse_enabled: true,
            ..LiveState::default()
        };

        live.sync(&editor, Some(0));

        assert!(live.active.is_none(), "浏览态不应因源码光标显示插入框");
        assert!(!live.blocks.is_empty(), "浏览态仍需建立富渲染块表");
    }

    /// 写作态连续输入空行仍属于正文编辑，不能把活动块清掉并让插入点
    /// 消失；用户可通过双击其他块或切换模式主动结束当前块。
    #[test]
    fn writing_keeps_caret_after_blank_line() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("alpha\n\nnext\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState {
            active: Some(0),
            browse_enabled: true,
            ..LiveState::default()
        };
        live.pending_caret = Some((0, 0));
        let _ = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        let _ = live_frame(
            &ctx,
            vec![enter.clone()],
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let _ = live_frame(
            &ctx,
            vec![enter],
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert_eq!(live.active, Some(0), "连续 Enter 不应退出当前写作块");
        assert!(!live.browse_only, "连续 Enter 后仍处于写作态");
        assert!(cursor.byte.is_some(), "插入点仍应回填到共享缓冲");
    }

    /// macOS 切换中文输入法会短暂重置 IME 上下文。Live 活动块应在该帧
    /// 之后恢复焦点，并继续使用原来的持久光标状态，而不是回到浏览态。
    #[test]
    fn ime_switch_restores_live_focus_and_caret() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("中文输入\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState {
            active: Some(0),
            browse_enabled: true,
            ..LiveState::default()
        };
        live.pending_caret = Some((0, 2));

        let _ = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let id = egui::Id::new("live-copy-test").with(("live-block", 0));
        ctx.memory_mut(|memory| memory.request_focus(id));
        // 让上一帧的焦点记忆落地，再模拟 macOS 输入源切换时的短暂失焦。
        let _ = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        ctx.memory_mut(|memory| memory.surrender_focus(id));

        let _ = live_frame(
            &ctx,
            vec![egui::Event::Ime(egui::ImeEvent::Preedit {
                text: String::new(),
                active_range_chars: None,
            })],
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );

        assert!(
            ctx.memory(|memory| memory.has_focus(id)),
            "IME 切换后焦点应回到活动块"
        );
        let state = egui::widgets::text_edit::TextEditState::load(&ctx, id)
            .expect("活动块 TextEdit 状态应保留");
        assert_eq!(
            state.cursor.char_range().map(|range| range.primary.index.0),
            Some(2),
            "IME 切换不应丢失插入点"
        );
    }

    /// 编辑前块序号发生漂移时，源码未变化的块仍复用富渲染结果。
    #[test]
    fn sync_keeps_rich_cache_for_unchanged_blocks() {
        let (mut editor, mut live, _) = state_with("第一段 **a**\n\n第二段 **b**\n");
        let first_rev = editor.revision();
        for (index, range) in live.blocks.clone().into_iter().enumerate() {
            let text = &editor.text()[range];
            live.rich_block(index, first_rev, text);
        }
        let cached = live
            .rich_cache
            .get(&1)
            .map(|entry| Arc::clone(&entry.1))
            .expect("第二块已建立缓存");

        editor.insert_chars(0, "前缀 ");
        live.sync(&editor, None);

        let second = live.blocks[1].clone();
        let rendered = live.rich_block(1, editor.revision(), &editor.text()[second]);
        assert!(Arc::ptr_eq(&cached, &rendered.0), "未变化块不应重新改写");
    }

    /// 活动块按光标字节定位:在第 2 块开头敲回车后,活动块跟着光标而不是
    /// 停在旧序号上(按序号记忆会错位)。
    #[test]
    fn active_block_follows_cursor_after_split() {
        let text = "一\n\n二\n";
        let (mut editor, mut live, _) = state_with(text);
        let second = live.blocks[1].start;
        live.sync(&editor, Some(second));
        assert_eq!(live.active, Some(1));

        // 在块首敲回车:块表分裂,光标仍在原处 → 活动块重新定位
        editor.insert_chars(editor.byte_to_char(second), "\n");
        live.sync(&editor, Some(second));
        assert_eq!(live.active, Some(live.block_containing(second).unwrap()));

        // 合并块(删掉中间的空行):块数变少,活动块仍跟着光标
        let blocks_before = live.blocks.len();
        editor.remove_chars(editor.byte_to_char(second - 1)..editor.byte_to_char(second + 1));
        live.sync(&editor, Some(second - 1));
        assert!(live.blocks.len() < blocks_before, "块被合并了");
        assert_eq!(
            live.active,
            Some(live.block_containing(second - 1).unwrap())
        );
    }

    /// 块内编辑落在同一份缓冲上(不产生第二份文本):块首插入 = 全文对应位置插入。
    #[test]
    fn block_edits_land_in_the_shared_buffer() {
        let mut editor = EditorBuffer::new("hello\n\nworld\n");
        let mut live = LiveState::default();
        live.sync(&editor, None);
        let block = live.blocks[0].clone();
        {
            let mut buffer = BlockBuffer {
                buffer: &mut editor,
                range: block.clone(),
            };
            assert_eq!(egui::TextBuffer::as_str(&buffer), "hello\n\n");
            egui::TextBuffer::insert_text(&mut buffer, ">> ", egui::text::CharIndex(0));
        }
        assert_eq!(editor.text(), ">> hello\n\nworld\n");
        assert!(editor.is_dirty());
    }

    /// 块内删除:块内字符区间换算回全文区间(删掉的是块首两个字)。
    #[test]
    fn block_delete_maps_back_to_full_text() {
        let mut editor = EditorBuffer::new("hello\n\nworld\n");
        let mut live = LiveState::default();
        live.sync(&editor, None);
        let block = live.blocks[1].clone();
        {
            let mut buffer = BlockBuffer {
                buffer: &mut editor,
                range: block.clone(),
            };
            assert_eq!(egui::TextBuffer::as_str(&buffer), "world\n");
            egui::TextBuffer::delete_char_range(
                &mut buffer,
                egui::text::CharIndex(0)..egui::text::CharIndex(2),
            );
        }
        assert_eq!(editor.text(), "hello\n\nrld\n");
    }

    /// 块字符长度(光标路由按字符算)与中英混排一致。
    #[test]
    fn block_char_len_counts_chars_not_bytes() {
        let (editor, live, _) = state_with("你好\n\nworld\n");
        assert_eq!(live.block_char_len(&editor, 0), 4, "你好 + 空行");
        assert_eq!(live.block_char_len(&editor, 1), 6, "world + \\n");
        assert_eq!(live.block_char_len(&editor, 99), 0, "越界给 0");
    }

    /// Live 面板渲一帧不 panic(活动块走 TextEdit、其余走富渲染),且不改动
    /// 缓冲 —— 绘制不该有副作用。
    #[test]
    fn live_panel_renders_a_frame_without_touching_text() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("# 标题\n\n正文\n\n```rust\nfn a(){}\n```\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState {
            active: Some(0),
            ..LiveState::default()
        };
        let before = editor.text().to_owned();
        let mut outbox = Vec::new();
        let mut selection_mirror = None;
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            super::ui(
                ui,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut selection_mirror,
                &mut live,
                egui::Id::new("live-test"),
                false,
                false,
                &crate::keymap::Keymap::builtin(),
                &mut outbox,
            );
        });
        output.drop_without_applying_deltas();
        assert_eq!(editor.text(), before, "渲染不改动缓冲");
        assert!(!editor.is_dirty(), "渲染不算用户修改");
        assert!(live.blocks.len() >= 3, "{live:?}");
    }

    /// 替换(#17)与 Live 块表的联动(#60 M2 核验②):替换走的**整篇 rope**
    /// (`EditorBuffer::replace_all` / `replace_range`,归约侧
    /// `replace_all_in_doc` / `replace_current` 的同一对写路径),不是活动
    /// 块缓冲 —— rev 推进后下一次 `sync` 按新文本重切块,块表重新覆盖全
    /// 文;替换本身不产 `pending_caret`(那是跳转/路由的通道,替换定位走
    /// 源码侧 `pending_selection`,Live 下查找卡本就不可达,#17 口径)。
    /// 换句话说:Live 模式被(程序化或切模式前的)替换改写后,渲染口径与
    /// 文本永远对得上,不存在「替换词写进活动块、其余块还是旧文本」的
    /// 第二份缓冲。
    #[test]
    fn replace_writes_whole_rope_and_next_sync_rechunks() {
        // —— 全部替换路径(replace_all):三块文档里的 foo 全变 bar ——
        let (mut editor, mut live, _) = state_with("foo aa\n\nfoo bb\n\nfoo cc\n");
        live.sync(&editor, Some(editor.byte_to_char(live.blocks[1].start)));
        assert_eq!(live.active, Some(1), "前置:活动块在第 2 块");
        let rev_before = editor.revision();

        // #17 replace_all_in_doc 的同一写:一次整篇写入,undo 栈单快照
        editor.replace_all("bar aa\n\nbar bb\n\nbar cc\n");
        assert!(editor.revision() > rev_before, "整篇替换推进修订号");
        assert_ne!(
            live.synced_rev,
            Some(editor.revision()),
            "块表此刻还是旧文本的,等下一帧 sync"
        );

        // 渲染帧入口的第一次 sync:重切块 + 活动块按光标字节重定位
        live.sync(&editor, Some(editor.byte_to_char(live.blocks[1].start)));
        assert_eq!(live.synced_rev, Some(editor.revision()), "块表已追上");
        let mut covered = 0;
        for block in &live.blocks {
            assert_eq!(block.start, covered, "块表连续覆盖新全文");
            covered = block.end;
        }
        assert_eq!(covered, editor.text().len());
        assert_eq!(
            BlockBuffer::slice(editor.text(), &live.blocks[1]),
            "bar bb\n\n",
            "块切片渲染的就是替换后的文本"
        );
        assert_eq!(
            live.active,
            Some(live.block_containing(live.blocks[1].start).unwrap()),
            "活动块按光标落回新块表"
        );

        // —— 单个替换路径(replace_range):当前命中定点改写同样整篇生效 ——
        let (mut editor, mut live, _) = state_with("foo one\n\nfoo two\n");
        live.sync(&editor, None);
        let first_foo_end = editor.byte_to_char(3);
        editor.replace_range(0..first_foo_end, "bar");
        live.sync(&editor, None);
        assert_eq!(editor.text(), "bar one\n\nfoo two\n");
        assert_eq!(
            BlockBuffer::slice(editor.text(), &live.blocks[0]),
            "bar one\n\n",
            "块 0 已是替换后文本,块 1 未受牵连"
        );
        assert_eq!(
            BlockBuffer::slice(editor.text(), &live.blocks[1]),
            "foo two\n"
        );
        assert!(live.pending_caret.is_none(), "替换不产块路由待落地");
    }

    /// 块包含判定:块内 / 文末 / 边界都给出确定的块。
    #[test]
    fn block_containing_resolves_edges() {
        let (editor, live, _) = state_with("一\n\n二\n");
        assert_eq!(live.block_containing(0), Some(0));
        assert_eq!(live.block_containing(live.blocks[1].start), Some(1));
        assert_eq!(
            live.block_containing(editor.text().len()),
            Some(live.blocks.len() - 1),
            "文末归最后一块"
        );
    }

    // —— 代码块复制头(#38)与「整块点击进编辑」的点击优先级 ——

    /// 从帧输出抽出全部 CopyText 命令的载荷(照 ui/preview.rs tests 同款)。
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

    /// 指针序列:移入 → 按下 → 抬起(照 ui/preview.rs tests 同款三帧;
    /// egui 的 click 判定发生在抬起帧)。
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

    /// 一帧 Live 列渲染的取证:复制按钮 rect 探针、CopyText 载荷、OpenUrl
    /// 目标、画出的文本 rect(正文/链接点击定位用)、浮标菜单点选的消息
    /// (#61)、每条文本 galley 的字形屏幕坐标(#170 点击落点定位)。
    struct LiveFrame {
        button_rects: Vec<egui::Rect>,
        copied: Vec<String>,
        opened: Vec<String>,
        texts: Vec<(String, egui::Rect)>,
        /// galley 文本 + 全部字形的屏幕坐标(行序拼接,与渲染文本字符序
        /// 对齐):定位「点第二段第二行首字」这类目标,不必模拟字体度量。
        glyphs: Vec<(String, Vec<egui::Pos2>)>,
        messages: Vec<Message>,
        /// 本帧镜像出的格式工具条选区(活动块持久选区的文档坐标)。
        selection_mirror: Option<(usize, usize)>,
    }

    /// 跑一帧 Live 面板(生产入口 `super::ui`)并收集取证。
    fn live_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        editor: &mut EditorBuffer,
        preview: &mut PreviewState,
        cursor: &mut OutlineCursor,
        live: &mut LiveState,
    ) -> LiveFrame {
        let mut outbox = Vec::new();
        let mut selection_mirror = None;
        let output = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                super::ui(
                    ui,
                    editor,
                    preview,
                    cursor,
                    &mut selection_mirror,
                    live,
                    egui::Id::new("live-copy-test"),
                    false,
                    false,
                    &crate::keymap::Keymap::builtin(),
                    &mut outbox,
                );
            },
        );
        let messages = outbox;
        let mut texts = Vec::new();
        let mut glyphs = Vec::new();
        for clipped in &output.shapes {
            if let egui::epaint::Shape::Text(t) = &clipped.shape {
                texts.push((
                    t.galley.text().to_owned(),
                    egui::Rect::from_min_size(t.pos, t.galley.size()),
                ));
                glyphs.push((
                    t.galley.text().to_owned(),
                    t.galley
                        .rows
                        .iter()
                        .flat_map(|row| row.glyphs.iter())
                        .map(|glyph| t.pos + glyph.pos.to_vec2())
                        .collect(),
                ));
            }
        }
        let copied = copied_texts(&output);
        let opened = output
            .platform_output
            .commands
            .iter()
            .filter_map(|cmd| match cmd {
                egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        LiveFrame {
            // 帧后读取:帧号已前进,按「最后写入者即本帧」取原始探针。
            button_rects: crate::ui::preview::copy_button_probe(ctx).1,
            copied,
            opened,
            texts,
            glyphs,
            messages,
            selection_mirror,
        }
    }

    /// Live 模式点非活动代码块的复制按钮必须复制、且不得切入编辑态(#38
    /// 评审修复):「整块点击进编辑」一旦参与 hit-test,无论注册先后都按
    /// egui 0.36 同层平局规则取后注册者(hit_test.rs `find_closest_within`)
    /// 与块内可点件二选一 —— 后注册抢按钮(复制失效、块误进编辑),先注册
    /// 被富渲染体抢(进编辑失效,实测点击后 active 仍 None)。修法 = 块级
    /// 命中不挂 widget,读原始指针事件([`clicked_for_edit`])。
    #[test]
    fn live_copy_button_click_copies_without_entering_edit() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("# 标题\n\n正文段落。\n\n```rust\nfn a(){}\n```\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();

        // 静帧:非活动代码块恰一枚按钮,块全部富渲染。
        let frame = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert!(frame.copied.is_empty(), "静帧不应复制");
        assert_eq!(
            frame.button_rects.len(),
            1,
            "非活动代码块一枚复制按钮:{:?}",
            frame.button_rects
        );
        assert_eq!(live.active, None);

        // 三帧点击按钮中心:复制恰一次、内容为块源文本;块不得切入编辑。
        let target = frame.button_rects[0].center();
        let mut copied = Vec::new();
        for events in click_events(target) {
            let frame = live_frame(
                &ctx,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
            copied.extend(frame.copied);
        }
        assert_eq!(
            copied,
            vec!["fn a(){}".to_owned()],
            "点按钮必须复制块源文本(修复前被整块 rect 抢走,这里为空)"
        );
        assert_eq!(live.active, None, "点按钮不得切入编辑态:{live:?}");
    }

    /// 点块内正文(非按钮)仍切入编辑(修复不得伤及「整块点击进编辑」本身)。
    #[test]
    fn live_click_outside_copy_button_still_enters_edit() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("# 标题\n\n正文段落。\n\n```rust\nfn a(){}\n```\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();

        let frame = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert_eq!(frame.button_rects.len(), 1);

        // 点击代码正文(galley rect 中心):离右上角复制按钮足够远。
        let (_, code_rect) = frame
            .texts
            .iter()
            .find(|(text, _)| text.contains("fn a()"))
            .expect("代码正文已渲染");
        let target = code_rect.center();
        assert!(
            !frame.button_rects[0].contains(target),
            "点击点必须避开按钮:{target:?}"
        );
        let mut copied = Vec::new();
        for events in click_events(target) {
            let frame = live_frame(
                &ctx,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
            copied.extend(frame.copied);
        }
        assert!(copied.is_empty(), "点正文不触发复制:{copied:?}");
        assert_eq!(live.active, Some(2), "点块内正文应切入编辑态:{live:?}");
    }

    /// #170:点击富渲染块进入编辑,光标落**点击处**而非块尾(旧行为一律
    /// 块尾 —— 点块内任何位置,光标都跳到块末尾的空行上)。第二段是软换
    /// 行的两行源码,富渲染成一行(CommonMark 语义,换行渲染成空格):点
    /// 渲染文本第 7 个字形(= 源码第二行行首「第」),块内字符偏移应落在
    /// 7,而不是块尾 14。
    #[test]
    fn live_click_lands_caret_on_the_clicked_glyph() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("第一段\n\n第二段第一行\n第二段第二行\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();

        let frame = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let (_, glyphs) = frame
            .glyphs
            .iter()
            .find(|(text, _)| text.contains("第二段第二行"))
            .expect("第二段已富渲染");
        assert_eq!(glyphs.len(), 13, "软换行合行后 13 字形:{glyphs:?}");
        // 渲染文本 "第二段第一行 第二段第二行" 第 7 字形,字形内左缘 +
        // 1px(中点判定命中该字形)。
        let target = glyphs[7] + egui::vec2(1.0, 0.0);

        for events in click_events(target) {
            live_frame(
                &ctx,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
        }
        assert_eq!(live.active, Some(1), "点击切入编辑态:{live:?}");
        assert_eq!(
            live.pending_caret,
            Some((1, 7)),
            "光标落点击字形而非块尾:{:?}",
            live.pending_caret
        );
    }

    /// #170 的落点精度:点第一段首字形,块内偏移落 0(字形级精确),
    /// 不是块尾 5,也不是「最近子块」回退能给出的任何其它值。
    #[test]
    fn live_click_lands_caret_at_first_paragraph_start() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("第一段\n\n第二段第一行\n第二段第二行\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();

        let frame = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let (_, glyphs) = frame
            .glyphs
            .iter()
            .find(|(text, _)| text.trim() == "第一段")
            .expect("第一段已富渲染");
        let target = glyphs[0] + egui::vec2(1.0, 0.0);

        for events in click_events(target) {
            live_frame(
                &ctx,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
        }
        assert_eq!(live.active, Some(0), "点击切入编辑态:{live:?}");
        assert_eq!(
            live.pending_caret,
            Some((0, 0)),
            "光标落第一段行首而非块尾 5:{:?}",
            live.pending_caret
        );
    }

    /// 活动块选区每帧镜像进 selection 出参(live 格式动作的前半段,
    /// 2026-10-10):活动块的持久 TextEditState 选区换算成文档坐标抄给
    /// `TabState::selection`;**无活动块帧必须镜像出 None** —— 不然归约
    /// 侧会拿源码模式的陈旧值盲写(曾是在 live 下点加粗的现行 bug)。
    #[test]
    fn active_block_selection_is_mirrored_for_the_format_bar() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("第一段\n\n第二段\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();

        // 无活动块:镜像 None
        let frame = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert_eq!(frame.selection_mirror, None, "无活动块 = 无选区");

        // 活动块 0 内框选 0..3:经持久 TextEditState 注入,再跑一帧
        live.active = Some(0);
        let block_id = egui::Id::new("live-copy-test").with(("live-block", 0));
        let mut st = egui::widgets::text_edit::TextEditState::default();
        st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(3),
        )));
        st.store(&ctx, block_id);
        ctx.memory_mut(|mem| mem.request_focus(block_id));
        let frame = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert_eq!(
            frame.selection_mirror.map(|(a, b)| (a.min(b), a.max(b))),
            Some((0, 3)),
            "活动块选区镜像成文档坐标(块基 0,同值;TextEdit 会归一化主副序)"
        );
    }

    /// 格式动作的落地帧(链路末端,2026-10-10):归约挂上的
    /// `pending_caret` + `pending_range` 落成块的持久双侧选区
    /// (「内容仍选中」),并即焚;焦点还给活动块。
    #[test]
    fn format_landing_restores_the_content_selection() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("**甲乙丙**\n\n后文\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();
        live.sync(&editor, Some(editor.char_to_byte(2)));
        assert_eq!(live.active, Some(0));
        live.pending_caret = Some((0, 2));
        live.pending_range = Some((2, 5));

        live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );

        let id = egui::Id::new("live-copy-test").with(("live-block", 0));
        let st =
            egui::widgets::text_edit::TextEditState::load(&ctx, id).expect("活动块 state 已建立");
        let range = st.cursor.char_range().expect("落地选区");
        assert_eq!(
            (
                range.primary.index.0.min(range.secondary.index.0),
                range.primary.index.0.max(range.secondary.index.0)
            ),
            (2, 5),
            "双侧选区落地(块内坐标;TextEdit 会归一化主副序)"
        );
        assert!(
            live.pending_caret.is_none() && live.pending_range.is_none(),
            "落地即焚,不悬置到下一帧"
        );
    }

    /// Live 富渲染块的任务 checkbox(#63):任务块渲染出自绘 checkbox
    /// (探针在场、豁免面的代码块不产),点击走块内载荷 + 块首换算出
    /// [`Message::TaskCheckboxToggled`](载荷指向**源码**里标记的 `[`),
    /// 且不切入编辑态(`clicked_for_edit` 的 checkbox 排除,#13a 同层)。
    #[test]
    fn live_task_checkbox_click_toggles_without_entering_edit() {
        let ctx = egui::Context::default();
        let doc = "- [ ] 首项待办\n\n第二块正文\n";
        let mut editor = EditorBuffer::new(doc);
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();

        // 静帧:任务块富渲染,恰一枚 checkbox;零消息、零切入。
        let frame = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert!(frame.messages.is_empty(), "静帧零消息");
        let rects = crate::ui::preview::task_checkbox_probe(&ctx);
        assert_eq!(rects.len(), 1, "任务块一枚 checkbox:{rects:?}");
        assert_eq!(live.active, None, "静帧不切入编辑");

        // 三帧点击 checkbox 中心:恰一条切换消息,载荷指向源码标记的 `[`;
        // 点击不得切入编辑态(点 checkbox 的意图是切换,不是进编辑)。
        let target = rects[0].center();
        let mut toggles = Vec::new();
        for events in click_events(target) {
            let frame = live_frame(
                &ctx,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
            toggles.extend(frame.messages.iter().filter_map(|message| match message {
                Message::TaskCheckboxToggled { byte } => Some(*byte),
                _ => None,
            }));
        }
        assert_eq!(toggles.len(), 1, "一次点击恰一条切换消息:{toggles:?}");
        assert_eq!(
            toggles[0],
            doc.find("[ ]").expect("task marker in source"),
            "块内载荷 + 块首 = 源码标记的 ["
        );
        assert_eq!(live.active, None, "点 checkbox 不得切入编辑态");

        // 切换落地(归约的同一写法)+ 重同步:勾选态即时反映 —— 修订号
        // 前进 → sync 清块级改写缓存 → 块改写重算出 c 载荷。
        let byte = toggles[0];
        let char_at = editor.byte_to_char(byte);
        editor.replace_range(char_at + 1..char_at + 2, "x");
        live.sync(&editor, None);
        let (block_rendered, _, _, _, _) = live.rich_block(
            0,
            editor.revision(),
            BlockBuffer::slice(editor.text(), &live.blocks[0]),
        );
        assert!(
            block_rendered.contains("task://c"),
            "切换后块改写即时反映勾选态:{block_rendered}"
        );
    }

    /// #51 M3:Live 富渲染块里的 ```mermaid 围栏出图(生产入口 `ui` 经
    /// `LiveMermaidHandler` 接入);解析失败的围栏回落源码(探针不出节点),
    /// 其余块(代码块复制按钮等)不受牵连。
    #[test]
    fn live_rich_block_renders_mermaid_and_falls_back() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new(concat!(
            "# 标题\n\n",
            "```mermaid\nflowchart TD\nA --> B\n```\n\n",
            "正文段落。\n\n",
            "```mermaid\nsequenceDiagram\nA->>B\n```\n",
        ));
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();
        let frame = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let probe = crate::ui::mermaid::read_probe(&ctx);
        assert_eq!(
            probe.len(),
            2,
            "两个 mermaid 块都进 widget:{:?}",
            probe.len()
        );
        assert!(probe[0].rendered, "合法图应出图");
        assert_eq!(probe[0].nodes.len(), 2, "A/B 两节点");
        assert!(!probe[1].rendered, "sequenceDiagram 应回落源码");
        assert!(probe[1].nodes.is_empty());
        // 回落块的复制按钮照常挂出(嵌套 label 的代码块增强仍可用)。
        assert!(
            !frame.button_rects.is_empty(),
            "回落块应保留复制按钮:{:?}",
            frame.button_rects
        );
    }

    /// 链接点击归属富渲染层:打开 URL,不连带把块切进编辑(修复后富渲染
    /// 块的链接恢复点击 —— 修复前被整块矩形整体抢走)。
    #[test]
    fn live_link_click_opens_url_without_entering_edit() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("[点我](https://example.com)\n\n后续。\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState {
            active: Some(1),
            ..LiveState::default()
        };

        let frame = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let (_, link_rect) = frame
            .texts
            .iter()
            .find(|(text, _)| text.contains("点我"))
            .expect("链接文本已渲染");

        let mut opened = Vec::new();
        for events in click_events(link_rect.center()) {
            let frame = live_frame(
                &ctx,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
            opened.extend(frame.opened);
        }
        assert_eq!(
            opened,
            vec!["https://example.com".to_owned()],
            "链接点击打开 URL"
        );
        assert_eq!(live.active, Some(1), "链接点击不切进编辑:{live:?}");
    }

    // —— LP2-1 活动块内联标记半隐藏 ——

    /// 半隐藏态的测试文档:六段标记(开/闭 `**`、`[`、`](…)`、开/闭 `` ` ``),
    /// 光标目标「粗体」两字之间 = 字符 3,不贴任何标记。
    const FADE_DOC: &str = "**粗体** 与 [链接](https://e.com) 和 `代码`\n";
    const FADE_BLOCK: usize = 0;
    const FADE_CARET_CONTENT: usize = 3;

    fn fade_state(text: &str) -> (EditorBuffer, PreviewState, OutlineCursor, LiveState) {
        let editor = EditorBuffer::new(text);
        let preview = PreviewState::new(&editor);
        (
            editor,
            preview,
            OutlineCursor::default(),
            LiveState {
                active: Some(FADE_BLOCK),
                ..LiveState::default()
            },
        )
    }

    /// 遮罩取证帧驱动共用的编辑器 id(各测试独立 `Context`,不互扰)。
    const FADE_EDITOR_ID: &str = "live-fade";

    /// 跑一帧 Live 面板(生产入口 `super::ui`),取证半透明遮罩矩形
    /// (以遮罩色从帧 shapes 里挑 Rect,与 #38/#41 的 shapes 取证同款)。
    fn live_fade_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        editor: &mut EditorBuffer,
        preview: &mut PreviewState,
        cursor: &mut OutlineCursor,
        live: &mut LiveState,
    ) -> Vec<egui::Rect> {
        live_timed_frame(ctx, 0.0, events, editor, preview, cursor, live)
    }

    /// 带 `RawInput.time` 的帧驱动(LP2-2 手势测试专用):双击判定依赖
    /// 相邻两次 click 的时间差(input_state 的 max_double_click_delay),
    /// 必须显式给每帧一个递增时钟。
    fn live_timed_frame(
        ctx: &egui::Context,
        time: f64,
        events: Vec<egui::Event>,
        editor: &mut EditorBuffer,
        preview: &mut PreviewState,
        cursor: &mut OutlineCursor,
        live: &mut LiveState,
    ) -> Vec<egui::Rect> {
        let mut fade = egui::Color32::TRANSPARENT;
        let mut outbox = Vec::new();
        let mut selection_mirror = None;
        let output = ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                super::ui(
                    ui,
                    editor,
                    preview,
                    cursor,
                    &mut selection_mirror,
                    live,
                    egui::Id::new(FADE_EDITOR_ID),
                    false,
                    false,
                    &crate::keymap::Keymap::builtin(),
                    &mut outbox,
                );
                let bg = ui.visuals().text_edit_bg_color();
                fade = egui::Color32::from_rgba_unmultiplied(
                    bg.r(),
                    bg.g(),
                    bg.b(),
                    super::MARK_FADE_ALPHA,
                );
            },
        );
        let rects = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Rect(shape) if shape.fill == fade => Some(shape.rect),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        rects
    }

    /// 光标在正文里:全部标记半隐藏,渲染一帧不 panic、不改动缓冲、不置脏。
    #[test]
    fn live_fades_marks_when_caret_in_content() {
        let ctx = egui::Context::default();
        let (mut editor, mut preview, mut cursor, mut live) = fade_state(FADE_DOC);

        // 经 v1 的 pending_caret 把光标交到正文中间(第一帧只落光标)
        live.pending_caret = Some((FADE_BLOCK, FADE_CARET_CONTENT));
        let _ = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );

        let before = editor.text().to_owned();
        let rects = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert_eq!(editor.text(), before, "渲染不改动缓冲");
        assert!(!editor.is_dirty(), "渲染不置脏");
        assert_eq!(
            live.marks
                .as_ref()
                .map(|(_, _, bundle)| bundle.segments.len()),
            Some(6),
            "六段标记:{:?}",
            live.marks
        );
        assert!(
            rects.len() >= 6,
            "每段标记至少一枚遮罩(折行只会更多):{:?}",
            rects
        );
        // 标记缓存命中:再跑一帧仍是六段(修订号未动不重解析)
        live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert_eq!(
            live.marks
                .as_ref()
                .map(|(_, _, bundle)| bundle.segments.len()),
            Some(6)
        );

        // 暗色主题:遮罩色随 visuals 取,同样逐段落位(存在性,非目视裁决)
        ctx.set_visuals(egui::Visuals::dark());
        let dark = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert!(dark.len() >= 6, "暗色下遮罩仍在:{dark:?}");
    }

    /// 光标贴上标记 → 该段显形;选区压住标记 → 相交段显形。
    #[test]
    fn live_reveals_mark_under_caret_or_selection() {
        let ctx = egui::Context::default();
        let (mut editor, mut preview, mut cursor, mut live) = fade_state(FADE_DOC);
        live.pending_caret = Some((FADE_BLOCK, FADE_CARET_CONTENT));
        let _ = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let base = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        )
        .len();
        assert!(base >= 6);

        // 光标挪进开头的 `**`(两星之间 = 字符 1)→ 该段显形
        live.pending_caret = Some((FADE_BLOCK, 1));
        let _ = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let on_mark = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        )
        .len();
        assert!(
            on_mark < base,
            "光标贴上后应有标记段显形:{on_mark} vs 基准 {base}"
        );

        // 选区盖住链接前后(字符 9..14:`[`、链接文本、`)` → 链接两段显形
        let id = egui::Id::new("live-fade").with(("live-block", FADE_BLOCK));
        let mut state =
            egui::widgets::text_edit::TextEditState::load(&ctx, id).expect("状态已就位");
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(9),
                egui::text::CCursor::new(14),
            )));
        state.store(&ctx, id);
        let selected = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        )
        .len();
        assert!(
            selected + 2 <= base,
            "链接两段(`[` 与 `](…)`)应显形:{selected} vs 基准 {base}"
        );
    }

    /// 点击半隐藏的标记:遮罩不参与命中测试 —— TextEdit 照常获得焦点,光标
    /// 落到标记上,下一帧该段显形。
    #[test]
    fn live_click_on_faded_mark_focuses_editor_and_reveals() {
        let ctx = egui::Context::default();
        let (mut editor, mut preview, mut cursor, mut live) = fade_state(FADE_DOC);
        live.pending_caret = Some((FADE_BLOCK, FADE_CARET_CONTENT));
        let _ = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let base_rects = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert!(base_rects.len() >= 6);

        // 点最靠左的遮罩矩形(即开头的 `**`)中心
        let target = base_rects
            .iter()
            .min_by(|a, b| a.min.x.total_cmp(&b.min.x))
            .expect("至少一枚遮罩")
            .center();
        for events in click_events(target) {
            let _ = live_fade_frame(
                &ctx,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
        }
        let id = egui::Id::new("live-fade").with(("live-block", FADE_BLOCK));
        assert!(
            ctx.memory(|mem| mem.has_focus(id)),
            "点击应命中活动块 TextEdit 本体(遮罩不参与 hit-test)"
        );
        let clicked = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        )
        .len();
        assert!(
            clicked < base_rects.len(),
            "点击落点在标记上,该段应显形:{clicked} vs 基准 {}",
            base_rects.len()
        );
    }

    /// 编辑语义不回退:活动块里打一个字仍直落同一份全文缓冲(undo 栈共用),
    /// 标记缓存随修订号前进失效重算。
    #[test]
    fn live_typing_in_active_block_updates_marks_cache() {
        let ctx = egui::Context::default();
        let (mut editor, mut preview, mut cursor, mut live) = fade_state(FADE_DOC);
        live.pending_caret = Some((FADE_BLOCK, FADE_CARET_CONTENT));
        let _ = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let _ = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let (rev_before, _, _) = live.marks.clone().expect("缓存已建立");

        // 在「粗体」内容里插一个字(走 BlockBuffer 同一条路,= TextEdit 打字)
        editor.insert_chars(FADE_CARET_CONTENT, "字");
        assert!(editor.text().contains("粗字体**"), "落在同一份缓冲");
        assert!(editor.is_dirty());

        live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let (rev_after, _, marks) = live.marks.clone().expect("缓存已重算");
        assert!(rev_after > rev_before, "修订号前进,缓存键换新");
        assert_eq!(marks.segments.len(), 6, "标记段数不变:{:?}", marks.segments);
        // 插入点在开 `**` 之后:闭 `**` 及其后所有区间右移一个 CJK 字(3 字节)
        let text_now = editor.text();
        assert_eq!(
            &text_now[marks.segments[1].clone()],
            "**",
            "闭 ** 仍标对位置"
        );
        assert_eq!(
            marks.segments[1],
            11..13,
            "原 8..10 平移一个 CJK 字(3 字节):{marks:?}"
        );
    }

    // —— LP2-2 选区扩展与配对显形 ——

    /// 手势测试文档:一对 `**` 包着四个等宽字母(内容中部/四分位的点击
    /// 坐标可从两枚遮罩矩形折算),后跟普通文本。
    const GESTURE_DOC: &str = "**abcd** 常规文本\n";
    const GESTURE_BLOCK: usize = 0;
    /// `**abcd**` 的完整字符区间(选区扩展的目标)。
    const GESTURE_PAIR: Range<usize> = 0..8;

    fn gesture_state(text: &str) -> (EditorBuffer, PreviewState, OutlineCursor, LiveState) {
        let editor = EditorBuffer::new(text);
        let preview = PreviewState::new(&editor);
        (
            editor,
            preview,
            OutlineCursor::default(),
            LiveState {
                active: Some(GESTURE_BLOCK),
                ..LiveState::default()
            },
        )
    }

    /// 跑一帧带时钟的 Live 面板,返回遮罩矩形(双击判定依赖帧时间)。
    fn gesture_frame(
        ctx: &egui::Context,
        time: f64,
        events: Vec<egui::Event>,
        editor: &mut EditorBuffer,
        preview: &mut PreviewState,
        cursor: &mut OutlineCursor,
        live: &mut LiveState,
    ) -> Vec<egui::Rect> {
        live_timed_frame(ctx, time, events, editor, preview, cursor, live)
    }

    /// 活动块 TextEdit 的当前选区 (min, max)(字符偏移)。
    fn block_selection(ctx: &egui::Context) -> Option<(usize, usize)> {
        let id = egui::Id::new(FADE_EDITOR_ID).with(("live-block", GESTURE_BLOCK));
        egui::widgets::text_edit::TextEditState::load(ctx, id).and_then(|state| {
            state.cursor.char_range().map(|range| {
                let (a, b) = (range.primary.index.0, range.secondary.index.0);
                (a.min(b), a.max(b))
            })
        })
    }

    /// 主键按下/抬起事件。
    fn button_event(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// 两枚 `**` 遮罩矩形按 x 排序成 (开侧, 闭侧)。
    fn pair_rects(rects: &[egui::Rect]) -> (egui::Rect, egui::Rect) {
        let mut sorted = rects.to_vec();
        sorted.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
        assert_eq!(sorted.len(), 2, "恰两枚 `**` 遮罩:{rects:?}");
        (sorted[0], sorted[1])
    }

    /// 渲染不 panic 且无副作用:标记块 + 非空选区状态连跑三帧(含选
    /// 区触碰标记的形态),缓冲不动、不置脏。
    #[test]
    fn live_renders_marks_and_selection_frames_without_panic() {
        let ctx = egui::Context::default();
        let (mut editor, mut preview, mut cursor, mut live) = fade_state(FADE_DOC);
        live.pending_caret = Some((FADE_BLOCK, FADE_CARET_CONTENT));
        let _ = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );

        // 直接把一个非空选区写进 TextEdit 状态(盖住链接开 `[` 与部分内容,
        // 触碰标记的形态)
        let id = egui::Id::new("live-fade").with(("live-block", FADE_BLOCK));
        let mut state =
            egui::widgets::text_edit::TextEditState::load(&ctx, id).expect("状态已就位");
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(9),
                egui::text::CCursor::new(14),
            )));
        state.store(&ctx, id);

        let before = editor.text().to_owned();
        for _ in 0..3 {
            let _ = live_fade_frame(
                &ctx,
                Vec::new(),
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
        }
        assert_eq!(editor.text(), before, "渲染不改动缓冲");
        assert!(!editor.is_dirty(), "渲染不置脏");
    }

    /// 双击标记对内的词:选区一次性扩到完整标记对(内容+两侧标记),
    /// 扩展后两段标记随选区显形;再跑一帧不二次改写(扩展不持续吸附)。
    #[test]
    fn live_double_click_inside_pair_expands_selection_once() {
        let ctx = egui::Context::default();
        let (mut editor, mut preview, mut cursor, mut live) = gesture_state(GESTURE_DOC);
        live.pending_caret = Some((GESTURE_BLOCK, 12)); // 光标放普通文本里
        let _ = gesture_frame(
            &ctx,
            0.0,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let rects = gesture_frame(
            &ctx,
            0.05,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let (opening, closing) = pair_rects(&rects);
        // 内容中部 = 四个等宽字母的正中(b/c 边界)
        let content_mid = egui::pos2((opening.right() + closing.left()) / 2.0, opening.center().y);

        // 双击 = 两次完整点击,第二次抬起帧 egui 判定 double_click 并选词
        for (time, pressed) in [(0.10, true), (0.15, false), (0.25, true), (0.30, false)] {
            let _ = gesture_frame(
                &ctx,
                time,
                vec![button_event(content_mid, pressed)],
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
        }
        assert_eq!(
            block_selection(&ctx),
            Some((GESTURE_PAIR.start, GESTURE_PAIR.end)),
            "双击词 abcd → 扩到 **abcd**"
        );

        // 稳定帧:选区保持原样,盖住整对 → 两段标记显形(遮罩清零)
        let rects = gesture_frame(
            &ctx,
            0.40,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert!(rects.is_empty(), "扩展选区盖住整对,两段都显形:{rects:?}");
        assert_eq!(
            block_selection(&ctx),
            Some((GESTURE_PAIR.start, GESTURE_PAIR.end)),
            "扩展是一次性的,静止帧不再改写选区"
        );
        assert_eq!(editor.text(), GESTURE_DOC, "手势不改文本");
    }

    /// 拖选松手:从标记上发起(锚点压着开 `**`)→ 扩到整对;从内容中部
    /// 发起的普通拖选 → 保持原选区不吸附(最小惊讶)。
    #[test]
    fn live_drag_release_expands_only_from_mark_anchor() {
        let ctx = egui::Context::default();
        let (mut editor, mut preview, mut cursor, mut live) = gesture_state(GESTURE_DOC);
        live.pending_caret = Some((GESTURE_BLOCK, 12));
        let _ = gesture_frame(
            &ctx,
            0.0,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let rects = gesture_frame(
            &ctx,
            0.05,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let (opening, closing) = pair_rects(&rects);
        let on_mark = egui::pos2(opening.left() + opening.width() * 0.25, opening.center().y);
        let content_mid = egui::pos2((opening.right() + closing.left()) / 2.0, opening.center().y);
        let content_quarter = egui::pos2(
            opening.right() + (closing.left() - opening.right()) * 0.25,
            opening.center().y,
        );

        // ① 按在开 `**` 上、拖到内容中部松手 → 扩到整对
        for (time, events) in [
            (0.10, vec![egui::Event::PointerMoved(on_mark)]),
            (0.15, vec![button_event(on_mark, true)]),
            (0.30, vec![egui::Event::PointerMoved(content_mid)]),
            (0.40, vec![button_event(content_mid, false)]),
        ] {
            let _ = gesture_frame(
                &ctx,
                time,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
        }
        assert_eq!(
            block_selection(&ctx),
            Some((GESTURE_PAIR.start, GESTURE_PAIR.end)),
            "从标记发起的拖选松手 → 扩到 **abcd**"
        );

        // ② 从内容中部(四分位)发起、拖到闭 `**` 边界松手 → 保持原选区
        for (time, events) in [
            (0.60, vec![egui::Event::PointerMoved(content_quarter)]),
            (0.65, vec![button_event(content_quarter, true)]),
            (
                0.80,
                vec![egui::Event::PointerMoved(egui::pos2(
                    closing.left(),
                    opening.center().y,
                ))],
            ),
            (
                0.90,
                vec![button_event(
                    egui::pos2(closing.left(), opening.center().y),
                    false,
                )],
            ),
        ] {
            let _ = gesture_frame(
                &ctx,
                time,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
        }
        // 锚点在 b/c 之间的内容里:普通拖选,选区保持 bcd(3..6)不吸附
        assert_eq!(
            block_selection(&ctx),
            Some((3, 6)),
            "内容中部发起的拖选不扩展:{:?}",
            block_selection(&ctx)
        );
        assert_eq!(editor.text(), GESTURE_DOC, "手势不改文本");
    }

    /// 配对显形:选区只盖住一侧标记,配对**另一侧**所在段同时显形 ——
    /// 这是 LP2-1「只显形被压住的段」之上的增量(该实现会留 3 枚遮罩)。
    #[test]
    fn live_selection_on_mark_reveals_partner_segment() {
        let ctx = egui::Context::default();
        let (mut editor, mut preview, mut cursor, mut live) = fade_state("**粗** x *i*\n");
        // 光标放到两组标记之间的 `x` 上(字符 6):不贴任何标记
        live.pending_caret = Some((0, 6));
        let _ = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let base = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert_eq!(base.len(), 4, "两对标记四枚遮罩:{base:?}");

        // 非空选区只盖住开 `**`(字符 0..2):开闭两段显形,`*i*` 仍半隐藏
        let id = egui::Id::new("live-fade").with(("live-block", 0));
        let mut state =
            egui::widgets::text_edit::TextEditState::load(&ctx, id).expect("状态已就位");
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(2),
            )));
        state.store(&ctx, id);
        let rects = live_fade_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert_eq!(
            rects.len(),
            2,
            "配对闭侧显形 + 无关的 *i* 两段保持半隐藏:{rects:?}"
        );
    }

    // —— LP2-3 活动块跟随与跨块 caret/大纲跳转时序 ——

    /// 滚动路径帧驱动的编辑器 id(与生产同一派生规则推导块 TextEdit id)。
    const SCROLL_EDITOR_ID: &str = "live-scroll";

    /// 滚动测试文档:`count` 行软换行段落(CommonMark 里软换行不成段,整
    /// 段一块;100 行 ≈ 2000px,远超 600px 视口)+ 一行尾块。
    fn scroll_doc_lines(count: usize) -> String {
        let mut text = String::new();
        for i in 0..count {
            text.push_str(&format!("行 {i:03}\n"));
        }
        text.push_str("\n尾块\n");
        text
    }

    /// 活动块 TextEdit 的持久 id(生产同一派生:`editor_id.with(...)`)。
    fn scroll_block_id(index: usize) -> egui::Id {
        egui::Id::new(SCROLL_EDITOR_ID).with(("live-block", index))
    }

    /// 活动块 TextEdit 当前主光标(块内字符偏移)。
    fn scroll_block_caret(ctx: &egui::Context, index: usize) -> Option<usize> {
        egui::widgets::text_edit::TextEditState::load(ctx, scroll_block_id(index))
            .and_then(|state| state.cursor.char_range().map(|range| range.primary.index.0))
    }

    /// 跑一帧 Live 面板(生产入口 `super::ui`),视口 800×600(#29 测试基建
    /// 口径:默认测试视口永远装得下整篇,滚动路径测不到),返回本帧活动块
    /// TextEdit 的屏幕矩形 —— 滚动直接平移屏幕坐标,量 rect 比量内部滚动
    /// state 更黑盒(editor.rs #29 同款)。
    #[allow(clippy::too_many_arguments)]
    fn live_scroll_frame(
        ctx: &egui::Context,
        time: f64,
        events: Vec<egui::Event>,
        editor: &mut EditorBuffer,
        preview: &mut PreviewState,
        cursor: &mut OutlineCursor,
        live: &mut LiveState,
        show_typewriter: bool,
        show_focus: bool,
    ) -> Option<egui::Rect> {
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let mut id = egui::Id::NULL;
        let mut outbox = Vec::new();
        let mut selection_mirror = None;
        let output = ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                events,
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                id = super::ui(
                    ui,
                    editor,
                    preview,
                    cursor,
                    &mut selection_mirror,
                    live,
                    egui::Id::new(SCROLL_EDITOR_ID),
                    show_typewriter,
                    show_focus,
                    &crate::keymap::Keymap::builtin(),
                    &mut outbox,
                )
                .id;
            },
        );
        output.drop_without_applying_deltas();
        ctx.read_response(id).map(|response| response.rect)
    }

    /// 光标行的屏幕 y:块内字符偏移 → 行号 × 行高。行高用「矩形高 / 行
    /// 数」从同一响应矩形反推(TextEdit 随内容长高;行数 = 换行数 + 1,
    /// 与无折行 galley 的行划分同构)。内边距引入的常量误差由断言容差吸收。
    fn caret_screen_y(rect: egui::Rect, block_text: &str, caret: usize) -> f32 {
        let rows = block_text.chars().filter(|c| *c == '\n').count() as f32 + 1.0;
        let line = block_text
            .chars()
            .take(caret)
            .filter(|c| *c == '\n')
            .count();
        rect.top() + line as f32 * (rect.height() / rows)
    }

    /// 主键导航事件(ArrowUp / ArrowDown)。
    fn arrow(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    /// 键盘导航触发跟随:活动块(百行段落)持焦、ArrowDown 逐行下移,光标
    /// 行越出视口底后,导航帧的跟随把光标行抬回视口内 —— 落光标的写回帧
    /// (跟随关闭口径)出发时视口纹丝不动,跟随只由导航帧触发。
    #[test]
    fn live_keyboard_navigation_scrolls_caret_into_view() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new(&scroll_doc_lines(100));
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState {
            active: Some(0),
            ..LiveState::default()
        };
        // 落光标到块首但不开跟随(点击进编辑同款口径):视口从文档顶部出发
        live.pending_caret = Some((0, 0));
        live.caret_follow = false;
        let top0 = live_scroll_frame(
            &ctx,
            0.0,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        )
        .expect("活动块响应可读");
        assert_eq!(
            live.blocks.len(),
            2,
            "百行软换行段落 = 一块 + 尾块:{:?}",
            live.blocks
        );
        assert!(
            top0.top() >= 0.0,
            "出发时视口在文档顶部(实测 {})",
            top0.top()
        );

        let down = arrow(egui::Key::ArrowDown);
        // 空转一帧再进导航键:egui 0.36 的焦点锁过滤(set_focus_lock_
        // filter)要在「持焦的下一帧 show()」才生效,焦点授予当帧的下一帧
        // 立即按裸方向键会被 egui 的记忆层焦点导航抢走焦点(Live 面板里
        // 富渲染块 Sense::click_and_drag 可聚焦,候选存在)。真机人类输入
        // 点击→按键间隔 ≥100ms,早跨过这一帧;测试按真实输入节奏驱动。
        let _ = live_scroll_frame(
            &ctx,
            0.05,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        );
        for i in 0..45 {
            let _ = live_scroll_frame(
                &ctx,
                0.1 + f64::from(i) * 0.1,
                vec![down.clone()],
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                false,
                false,
            );
        }
        // 光标写回/变化 → 请求滚动 → 动画完成(默认 ≤0.3s)→ 布局生效,各差一帧
        for i in 0..3 {
            let _ = live_scroll_frame(
                &ctx,
                6.0 + f64::from(i) * 0.1,
                Vec::new(),
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                false,
                false,
            );
        }
        let rect = live_scroll_frame(
            &ctx,
            6.4,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        )
        .expect("活动块响应可读");
        assert!(
            rect.top() < 0.0,
            "键盘导航把视口推离文档顶部(实测 top {}):跟随发生了",
            rect.top()
        );
        let caret = scroll_block_caret(&ctx, 0).expect("光标已落位");
        let y = caret_screen_y(
            rect,
            BlockBuffer::slice(editor.text(), &live.blocks[0]),
            caret,
        );
        assert!(
            (-25.0..=625.0).contains(&y),
            "光标行留在视口内(实测 y {y},视口 [0,600])"
        );
    }

    /// 滚轮/空闲帧不触发跟随:光标留在文档头,滚轮把视口推进文档中部、
    /// 光标行被甩出视口后,空闲帧一步也不许把视口拽回光标处(#29 红线在
    /// Live 侧的同款)。
    #[test]
    fn live_wheel_and_idle_frames_do_not_steal_scroll() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new(&scroll_doc_lines(100));
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState {
            active: Some(0),
            ..LiveState::default()
        };
        live.pending_caret = Some((0, 0));
        live.caret_follow = false;
        let _ = live_scroll_frame(
            &ctx,
            0.0,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        );

        let pointer = egui::pos2(400.0, 300.0);
        let wheel = |delta: f32| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, delta),
            phase: egui::TouchPhase::Move,
            modifiers: Default::default(),
        };
        // ① 滚轮下滚 10 格:内容上移,文档头(光标所在行)被甩出视口上方
        for i in 0..10 {
            let _ = live_scroll_frame(
                &ctx,
                0.1 + f64::from(i) * 0.1,
                vec![egui::Event::PointerMoved(pointer), wheel(-120.0)],
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                false,
                false,
            );
        }
        // 滚动带动画:空转几帧让偏移落到最终值
        for i in 0..3 {
            let _ = live_scroll_frame(
                &ctx,
                1.5 + f64::from(i) * 0.1,
                vec![egui::Event::PointerMoved(pointer)],
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                false,
                false,
            );
        }
        let settled = live_scroll_frame(
            &ctx,
            1.9,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        )
        .expect("活动块响应可读")
        .top();
        assert!(settled < 0.0, "滚轮确实把视口推进文档(实测 top {settled})");

        // ② 空闲帧:光标仍被甩在视口外的文档头,视口一步也不许动
        for i in 0..6 {
            let _ = live_scroll_frame(
                &ctx,
                3.0 + f64::from(i) * 0.1,
                Vec::new(),
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                false,
                false,
            );
        }
        let after = live_scroll_frame(
            &ctx,
            3.7,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        )
        .expect("活动块响应可读")
        .top();
        assert!(
            (after - settled).abs() < 1.0,
            "空闲帧不得把视口拽回光标(稳定于 {settled},现在 {after})"
        );
    }

    /// 跨块路由后目标块光标与滚动一致:尾块块首按 ↑ 路由到段落块末尾,
    /// 落地帧把光标写进目标块持久 state 并**同帧**请求滚动,落定后光标行
    /// 在视口内 —— 不出现「光标写了但视图没跟」的终态。
    #[test]
    fn live_cross_block_route_lands_caret_and_scroll_together() {
        let ctx = egui::Context::default();
        ctx.style_mut_of(egui::Theme::Dark, |style| {
            style.scroll_animation = egui::style::ScrollAnimation::none();
        });
        let mut editor = EditorBuffer::new(&scroll_doc_lines(100));
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState {
            active: Some(1),
            ..LiveState::default()
        };
        // 尾块落光标到块首(不跟随):视口停在文档顶部,尾块在视口外
        live.pending_caret = Some((1, 0));
        live.caret_follow = false;
        let _ = live_scroll_frame(
            &ctx,
            0.0,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        );

        // ↑ 在块首 → 路由到段落块末尾
        let _ = live_scroll_frame(
            &ctx,
            0.1,
            vec![arrow(egui::Key::ArrowUp)],
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        );
        assert_eq!(live.active, Some(0), "路由切换活动块:{live:?}");
        assert_eq!(
            live.pending_caret,
            Some((0, live.block_char_len(&editor, 0))),
            "交接点 = 上一块末尾"
        );

        // 落地帧:光标写进块 0 持久 state + 同帧请求滚动 + 焦点转移
        let _ = live_scroll_frame(
            &ctx,
            0.2,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        );
        let caret = scroll_block_caret(&ctx, 0).expect("目标块光标已写入");
        assert_eq!(caret, live.block_char_len(&editor, 0), "光标落在上一块末尾");
        assert!(
            ctx.memory(|mem| mem.has_focus(scroll_block_id(0))),
            "焦点随路由转到目标块"
        );
        assert!(live.pending_caret.is_none(), "交接点已消费");

        // 同帧性:写回帧(0.2)即请求滚动 —— 本测试已把滚动动画设为「一次
        // 到位」(ScrollAnimation::none,#42 预览跳转同款),无头帧驱动下
        // 请求经 落账→应用→布局 三帧可见:第 3 帧(0.23)必已离开文档顶
        // 部;若写回与跟随差一帧(请求挪到 0.21),则要到第 4 帧才动 ——
        // 0.23 这一眼把「同帧」钉到一帧分辨率。
        for time in [0.21, 0.22] {
            let _ = live_scroll_frame(
                &ctx,
                time,
                Vec::new(),
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                false,
                false,
            );
        }
        let top_next = live_scroll_frame(
            &ctx,
            0.23,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        )
        .expect("活动块响应可读")
        .top();
        assert!(
            top_next < 0.0,
            "光标写回帧同帧请求了滚动(第 3 帧 top 已 {top_next})"
        );

        // 布局稳定后再量光标行落点
        for time in [1.0, 1.5, 2.0] {
            let _ = live_scroll_frame(
                &ctx,
                time,
                Vec::new(),
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                false,
                false,
            );
        }
        let rect = live_scroll_frame(
            &ctx,
            2.1,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        )
        .expect("活动块响应可读");
        assert!(rect.top() < 0.0, "视口离开文档顶部(实测 {})", rect.top());
        let y = caret_screen_y(
            rect,
            BlockBuffer::slice(editor.text(), &live.blocks[0]),
            caret,
        );
        assert!(
            (-25.0..=625.0).contains(&y),
            "目标块光标行滚入视口(实测 y {y},视口 [0,600])"
        );
    }

    /// 大纲跳转消费:Live 模式下 jump_to(与源码模式同一入口、同一时序
    /// 口径)当帧把光标路由到目标块、写进持久 state 并请求滚动;请求即
    /// 消费,光标回填随后续帧跟上。
    #[test]
    fn live_jump_to_routes_caret_and_scrolls_into_view() {
        let mut text = String::from("# 顶\n\n");
        for i in 0..100 {
            text.push_str(&format!("行 {i:03}\n"));
        }
        text.push_str("\n# 底\n");
        let ctx = egui::Context::default();
        ctx.style_mut_of(egui::Theme::Dark, |style| {
            style.scroll_animation = egui::style::ScrollAnimation::none();
        });
        let mut editor = EditorBuffer::new(&text);
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();
        let _ = live_scroll_frame(
            &ctx,
            0.0,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        );
        assert_eq!(live.blocks.len(), 3, "标题/段落/标题:{:?}", live.blocks);

        // 归约产出的跳转目标:底部标题行首(远在首屏之外)。块表修复后
        // (2026-10-10)`# ` 标记归属**标题块自身**,跳转进入标题块、光标
        // 落在行首 —— 与源码模式同一文档位置,同一入口同一落点。
        let heading_byte = editor.text().find("# 底").expect("文档里有底部标题");
        let target = live
            .block_containing(heading_byte)
            .expect("跳转字节必落在某块");
        let target_local =
            editor.byte_to_char(heading_byte) - editor.byte_to_char(live.blocks[target].start);
        assert_eq!(target, 2, "底部标题自身是跳转目标块");
        assert_eq!(target_local, 0, "标题标记位于块内行首");
        cursor.jump_to = Some(editor.byte_to_char(heading_byte));
        let _ = live_scroll_frame(
            &ctx,
            0.1,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        );
        assert_eq!(live.active, Some(target), "跳转目标块成为活动块:{live:?}");
        assert_eq!(
            scroll_block_caret(&ctx, target),
            Some(target_local),
            "光标落在跳转字节处(块内偏移)"
        );
        assert!(
            ctx.memory(|mem| mem.has_focus(scroll_block_id(target))),
            "焦点交给目标块"
        );
        assert_eq!(cursor.jump_to, None, "跳转请求即消费,不悬置");

        // 同帧性:跳转帧(0.1)即请求滚动(与跨块路由同一条口径:一次到位
        // 动画 + 三帧可见,第 3 帧 0.13 必已离开文档顶部,差一帧的实现此
        // 刻还不动)
        for time in [0.11, 0.12] {
            let _ = live_scroll_frame(
                &ctx,
                time,
                Vec::new(),
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                false,
                false,
            );
        }
        let top_next = live_scroll_frame(
            &ctx,
            0.13,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        )
        .expect("活动块响应可读")
        .top();
        // 文末目标:视口被钳在文档底,滚动生效的判据从「离开文档顶」改为
        // 「活动块顶已入视口」—— 若同帧没请求滚动,目标块还在视口外
        // 1700px 处,top 会远超视口高。
        assert!(
            (-1.0..600.0).contains(&top_next),
            "跳转帧同帧请求了滚动(文末目标钳在文档底;第 3 帧 top 已 {top_next})"
        );

        // 布局稳定后再量光标行落点
        for time in [1.0, 1.5, 2.0] {
            let _ = live_scroll_frame(
                &ctx,
                time,
                Vec::new(),
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                false,
                false,
            );
        }
        let rect = live_scroll_frame(
            &ctx,
            2.1,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        )
        .expect("活动块响应可读");
        let y = caret_screen_y(
            rect,
            BlockBuffer::slice(editor.text(), &live.blocks[target]),
            target_local,
        );
        assert!(
            (-25.0..=625.0).contains(&y),
            "跳转落点(光标行)滚入视口(实测 y {y},视口 [0,600])"
        );
        assert_eq!(
            cursor.byte,
            Some(heading_byte),
            "光标回填随跳转落位(标题行首字节)"
        );
    }

    // —— 打字机模式(#64 M1,Live 活动块)——

    /// 读 Live 打字机记忆(接管状态机探针;live_memory_id 分槽)。
    fn live_tw_memory(ctx: &egui::Context) -> crate::ui::typewriter::Memory {
        ctx.data(|d| {
            d.get_temp::<crate::ui::typewriter::Memory>(crate::ui::typewriter::live_memory_id(
                egui::Id::new(SCROLL_EDITOR_ID),
            ))
        })
        .unwrap_or_default()
    }

    /// Live 打字机跟随(任务书「两模式各自验证」的 Live 档):活动块持焦、
    /// ArrowDown 逐行下移越出首屏后,光标行停在目标带(600 视口的 1/3 线
    /// ±死区 + 一个行步进)—— 与源码模式同一决策面,不是 LP2-3 的最小
    /// 滚入(关闭态仍走最小滚入,见 live_keyboard_navigation_scrolls_
    /// caret_into_view)。
    #[test]
    fn live_typewriter_keeps_caret_row_near_anchor() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new(&scroll_doc_lines(100));
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState {
            active: Some(0),
            ..LiveState::default()
        };
        live.pending_caret = Some((0, 0));
        live.caret_follow = false;
        let _ = live_scroll_frame(
            &ctx,
            0.0,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
            false,
        );
        // 空转一帧再进导航键(set_focus_lock_filter,既有口径)
        let _ = live_scroll_frame(
            &ctx,
            0.05,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
            false,
        );
        let down = arrow(egui::Key::ArrowDown);
        for i in 0..45 {
            let _ = live_scroll_frame(
                &ctx,
                0.1 + f64::from(i) * 0.1,
                vec![down.clone()],
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                true,
                false,
            );
        }
        for t in [6.2, 6.3] {
            let _ = live_scroll_frame(
                &ctx,
                t,
                Vec::new(),
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                true,
                false,
            );
        }
        let rect = live_scroll_frame(
            &ctx,
            6.4,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
            false,
        )
        .expect("活动块响应可读");
        let caret = scroll_block_caret(&ctx, 0).expect("光标已落位");
        let y = caret_screen_y(
            rect,
            BlockBuffer::slice(editor.text(), &live.blocks[0]),
            caret,
        );
        let row_h = rect.height()
            / BlockBuffer::slice(editor.text(), &live.blocks[0])
                .lines()
                .count()
                .max(1) as f32;
        let anchor = 600.0 * crate::ui::typewriter::ANCHOR_RATIO;
        assert!(
            ((anchor - crate::ui::typewriter::DEAD_ZONE_ROWS * row_h - row_h)
                ..=(anchor + crate::ui::typewriter::DEAD_ZONE_ROWS * row_h + row_h))
                .contains(&y),
            "Live 导航后光标行应停在目标带,实测 {y:.1}(行高 {row_h:.1})"
        );
    }

    /// Live 打字机打字帧跟随:活动块内逐帧敲换行推进,光标行保持在目标
    /// 带内(修订号前进触发;关闭态打字帧不滚,见既有 wheel/idle 测试)。
    #[test]
    fn live_typewriter_typing_frames_follow() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new(&scroll_doc_lines(100));
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState {
            active: Some(0),
            ..LiveState::default()
        };
        live.pending_caret = Some((0, 0));
        live.caret_follow = false;
        let _ = live_scroll_frame(
            &ctx,
            0.0,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
            false,
        );
        // 焦点锁过滤帧
        let _ = live_scroll_frame(
            &ctx,
            0.05,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
            false,
        );

        // 换行走 Event::Key Enter(egui 过滤 Event::Text 中的 "\n");每帧
        // 之间插空转,对齐真实打字节奏(见 editor 侧同款测试)
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        let mut t = 0.1;
        for _ in 0..30 {
            let _ = live_scroll_frame(
                &ctx,
                t,
                vec![enter.clone()],
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                true,
                false,
            );
            t += 0.1;
            for _ in 0..2 {
                let _ = live_scroll_frame(
                    &ctx,
                    t,
                    Vec::new(),
                    &mut editor,
                    &mut preview,
                    &mut cursor,
                    &mut live,
                    true,
                    false,
                );
                t += 0.05;
            }
        }
        // 打字机 land:offset 次帧 begin 应用、布局再次帧反映
        for _ in 0..3 {
            let _ = live_scroll_frame(
                &ctx,
                t,
                Vec::new(),
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                true,
                false,
            );
            t += 0.05;
        }
        let rect = live_scroll_frame(
            &ctx,
            t,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
            false,
        )
        .expect("活动块响应可读");
        let caret = scroll_block_caret(&ctx, 0).expect("光标已落位");
        let y = caret_screen_y(
            rect,
            BlockBuffer::slice(editor.text(), &live.blocks[0]),
            caret,
        );
        let row_h = rect.height()
            / BlockBuffer::slice(editor.text(), &live.blocks[0])
                .lines()
                .count()
                .max(1) as f32;
        let anchor = 600.0 * crate::ui::typewriter::ANCHOR_RATIO;
        assert!(
            ((anchor - crate::ui::typewriter::DEAD_ZONE_ROWS * row_h - row_h)
                ..=(anchor + crate::ui::typewriter::DEAD_ZONE_ROWS * row_h + row_h))
                .contains(&y),
            "Live 打字推进后光标行应停在目标带,实测 {y:.1}"
        );
    }

    /// Live 用户滚动接管:滚轮滚离光标 → Override(空闲帧不拽回);下一
    /// 次键盘导航帧恢复 Follow 并把光标行带回目标带(与源码模式同口径)。
    #[test]
    fn live_typewriter_user_scroll_takes_over_until_next_edit() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new(&scroll_doc_lines(100));
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState {
            active: Some(0),
            ..LiveState::default()
        };
        live.pending_caret = Some((0, 0));
        live.caret_follow = false;
        let _ = live_scroll_frame(
            &ctx,
            0.0,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
            false,
        );
        let _ = live_scroll_frame(
            &ctx,
            0.05,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
            false,
        );
        let down = arrow(egui::Key::ArrowDown);
        for i in 0..45 {
            let _ = live_scroll_frame(
                &ctx,
                0.1 + f64::from(i) * 0.1,
                vec![down.clone()],
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                true,
                false,
            );
        }
        for t in [6.2, 6.3, 6.4] {
            let _ = live_scroll_frame(
                &ctx,
                t,
                Vec::new(),
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                true,
                false,
            );
        }
        assert_eq!(
            live_tw_memory(&ctx).phase,
            crate::ui::typewriter::Phase::Follow,
            "导航帧保持跟随态"
        );

        // 用户滚轮向下滚离(光标被甩在视口上方;不撞边界,滚轮量不残留)
        let pointer = egui::pos2(400.0, 300.0);
        let wheel = |delta: f32| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, delta),
            phase: egui::TouchPhase::Move,
            modifiers: Default::default(),
        };
        for i in 0..8 {
            let _ = live_scroll_frame(
                &ctx,
                6.5 + f64::from(i) * 0.1,
                vec![egui::Event::PointerMoved(pointer), wheel(-120.0)],
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                true,
                false,
            );
        }
        let _ = live_scroll_frame(
            &ctx,
            7.4,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
            false,
        );
        let settled = live_scroll_frame(
            &ctx,
            7.5,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
            false,
        )
        .expect("活动块响应可读")
        .top();
        assert_eq!(
            live_tw_memory(&ctx).phase,
            crate::ui::typewriter::Phase::Override,
            "滚轮后接管态置入"
        );
        for i in 0..5 {
            let after = live_scroll_frame(
                &ctx,
                8.0 + f64::from(i) * 0.1,
                Vec::new(),
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                true,
                false,
            )
            .expect("活动块响应可读")
            .top();
            assert!(
                (after - settled).abs() < 1.0,
                "接管期空闲帧不得拽回(稳定于 {settled:.1},现在 {after:.1})"
            );
        }

        // 下一次导航帧:恢复 Follow,光标行回目标带
        let _ = live_scroll_frame(
            &ctx,
            9.0,
            vec![down],
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
            false,
        );
        for t in [9.1, 9.2, 9.3, 9.4] {
            let _ = live_scroll_frame(
                &ctx,
                t,
                Vec::new(),
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                true,
                false,
            );
        }
        let rect = live_scroll_frame(
            &ctx,
            9.5,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
            false,
        )
        .expect("活动块响应可读");
        assert_eq!(
            live_tw_memory(&ctx).phase,
            crate::ui::typewriter::Phase::Follow,
            "编辑/导航帧恢复跟随"
        );
        let caret = scroll_block_caret(&ctx, 0).expect("光标已落位");
        let y = caret_screen_y(
            rect,
            BlockBuffer::slice(editor.text(), &live.blocks[0]),
            caret,
        );
        let row_h = rect.height()
            / BlockBuffer::slice(editor.text(), &live.blocks[0])
                .lines()
                .count()
                .max(1) as f32;
        let anchor = 600.0 * crate::ui::typewriter::ANCHOR_RATIO;
        assert!(
            ((anchor - crate::ui::typewriter::DEAD_ZONE_ROWS * row_h - row_h)
                ..=(anchor + crate::ui::typewriter::DEAD_ZONE_ROWS * row_h + row_h))
                .contains(&y),
            "恢复后光标行回目标带,实测 {y:.1}"
        );
    }

    // —— 选区 AI 浮标(#61 M1,Live 活动块)——

    use crate::state::SelectionAiAction;

    /// 指针主键按下/抬起事件(pos 处)。
    fn pointer_click(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// 读浮标探针(#61):本帧浮标/菜单矩形,未画为 `None`。
    fn sel_ai_probe(ctx: &egui::Context, editor_id: egui::Id) -> crate::ui::selection_ai::Probe {
        ctx.data(|d| {
            d.get_temp(crate::ui::selection_ai::probe_id(editor_id))
                .unwrap_or_default()
        })
    }

    /// 激活块 1 并把选区(块内字符 2..6)写进其持久 state:
    /// pending_caret 落光标 + 要焦点(生产 pending_caret 通道),选区经
    /// TextEditState 直写(与源码侧 pending 写回通道同一落点)。返回
    /// (块 widget id, 浮标所在帧之后的探针读取前提)。
    fn live_selection_fixture(
        ctx: &egui::Context,
    ) -> (
        egui::Id,
        EditorBuffer,
        PreviewState,
        OutlineCursor,
        LiveState,
    ) {
        let mut editor = EditorBuffer::new("# 标题块\n\n选中的正文内容块\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();
        let editor_id = egui::Id::new("live-copy-test");
        let block_id = editor_id.with(("live-block", 1));

        // 帧①:块 1 直接激活,pending_caret 把光标交进去并要焦点
        // (pending_caret 只在活动块内落地,激活与交棒是两步,与生产
        // 「点击富块 → activate + pending_caret」同终点)
        live.active = Some(1);
        live.pending_caret = Some((1, 0));
        live_frame(
            ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert_eq!(live.active, Some(1), "前置:块 1 已激活");

        // 块内持久 state 直写选区(2..6 =「的正文内」,字符偏移)
        let mut state = egui::widgets::text_edit::TextEditState::load(ctx, block_id)
            .expect("块 state 已持久化");
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(2),
                egui::text::CCursor::new(6),
            )));
        state.store(ctx, block_id);
        (block_id, editor, preview, cursor, live)
    }

    /// Live 活动块选区:浮标在场;打字照常进编辑器(不吞输入、焦点不丢),
    /// 输入替换选区后浮标即隐(选区塌缩)。
    #[test]
    fn live_selection_badge_appears_and_typing_reaches_editor() {
        let ctx = egui::Context::default();
        let editor_id = egui::Id::new("live-copy-test");
        let (block_id, mut editor, mut preview, mut cursor, mut live) =
            live_selection_fixture(&ctx);

        // 帧②:选区 + 焦点 → 浮标在场
        live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let probe = sel_ai_probe(&ctx, editor_id);
        assert!(probe.badge.is_some(), "Live 活动块选区浮标在场");
        assert!(probe.menu.is_none(), "菜单默认合拢");

        // 帧③:打字不吞 —— 输入替换选区落进同一份缓冲,焦点仍在活动块
        live_frame(
            &ctx,
            vec![egui::Event::Text("添".into())],
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert_eq!(
            editor.text(),
            "# 标题块\n\n选中添容块\n",
            "浮标在场期间打字照常写入(选区被输入替换)"
        );
        assert!(ctx.memory(|mem| mem.has_focus(block_id)), "焦点仍在活动块");
        assert_eq!(
            sel_ai_probe(&ctx, editor_id),
            crate::ui::selection_ai::Probe::default(),
            "选区被输入塌缩 → 浮标即隐"
        );
    }

    /// Live 浮标菜单:点浮标开菜单、点「AI 润色」行动作发消息;菜单压在
    /// 富渲染块上时,点击归菜单、不误进块编辑(clicked_for_edit 的浮标
    /// 排除);动作后菜单合拢。
    #[test]
    fn live_badge_menu_click_emits_action_without_activating_block() {
        let ctx = egui::Context::default();
        let editor_id = egui::Id::new("live-copy-test");
        let (_block_id, mut editor, mut preview, mut cursor, mut live) =
            live_selection_fixture(&ctx);

        // 帧②:浮标在场
        live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let badge = sel_ai_probe(&ctx, editor_id).badge.expect("浮标在场");

        // 帧③④:点击浮标 → 菜单打开,活动块不因点击漂移
        live_frame(
            &ctx,
            vec![
                egui::Event::PointerMoved(badge.center()),
                pointer_click(badge.center(), true),
            ],
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let frame = live_frame(
            &ctx,
            vec![pointer_click(badge.center(), false)],
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert!(frame.messages.is_empty(), "开菜单不产消息");
        let menu = sel_ai_probe(&ctx, editor_id)
            .menu
            .expect("点击浮标后菜单在场");
        assert_eq!(live.active, Some(1), "活动块不漂移");

        // 帧⑤⑥:点菜单第二行(润色)→ 发动作消息;菜单与富渲染块重叠,
        // 点击不得误进块编辑
        let row = egui::pos2(menu.center().x, menu.bottom() - 14.0);
        live_frame(
            &ctx,
            vec![egui::Event::PointerMoved(row), pointer_click(row, true)],
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let frame = live_frame(
            &ctx,
            vec![pointer_click(row, false)],
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert_eq!(
            frame.messages,
            vec![Message::SelectionAiActionRequested {
                action: SelectionAiAction::Polish,
                // 活动块内选区换算文档坐标:块 1 基 = "# 标题块\n\n" 的 7 字符,
                // 块内 (2,6) → (9,13);primary/secondary 无序(与
                // `TabState::selection` 同约定),归约侧 min/max 归一
                selection: Some((13, 9)),
            }],
            "菜单第二行(润色)点击发动作消息,选区已换算文档坐标"
        );
        assert_eq!(live.active, Some(1), "菜单压在富渲染块上,点击不误进块编辑");
        assert_eq!(
            sel_ai_probe(&ctx, editor_id).menu,
            None,
            "动作触发后菜单合拢"
        );
    }

    /// 否决线探针(Live):块激活 + 持焦但无选区(塌缩光标),零浮标元素。
    #[test]
    fn live_no_selection_renders_zero_badge_elements() {
        let ctx = egui::Context::default();
        let editor_id = egui::Id::new("live-copy-test");
        let mut editor = EditorBuffer::new("# 标题块\n\n正文块\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState {
            active: Some(1),
            pending_caret: Some((1, 0)),
            ..LiveState::default()
        };

        live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert_eq!(live.active, Some(1), "前置:块已激活持焦");
        assert_eq!(
            sel_ai_probe(&ctx, editor_id),
            crate::ui::selection_ai::Probe::default(),
            "无选区零浮标(Live)"
        );
    }

    // —— M2 专注模式(Live 非活动块淡化)——

    /// 专注模式测试文档:五块等距(块序 0..=4)。active=Some(2) 时淡化集
    /// = {0, 4},一段文档同时摆出「活动块/邻块正常、远块淡化」三种位形;
    /// 用 ASCII 免掉 CJK 字体缺席的测试环境分叉。
    const FOCUS_DOC: &str = "block zero\n\nblock one\n\nblock two\n\nblock three\n\nblock four\n";
    const FOCUS_EDITOR_ID: &str = "live-focus";

    /// 专注模式的帧驱动:跑一帧 Live(生产入口 `super::ui`),返回(块级
    /// 遮罩矩形、帧内文本 rect、全部 shape —— 供逐 shape 对照)。遮罩取证
    /// 以「编辑框背景色 × DIM_ALPHA」从 shapes 里挑 Rect(#81 同款手法,
    /// 明暗主题各自当帧取色)。
    #[allow(clippy::too_many_arguments)]
    fn live_focus_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        editor: &mut EditorBuffer,
        preview: &mut PreviewState,
        cursor: &mut OutlineCursor,
        live: &mut LiveState,
        show_focus: bool,
    ) -> (
        Vec<egui::Rect>,
        Vec<(String, egui::Rect)>,
        Vec<egui::epaint::ClippedShape>,
    ) {
        let mut outbox = Vec::new();
        let mut fade = egui::Color32::TRANSPARENT;
        let mut selection_mirror = None;
        let output = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                super::ui(
                    ui,
                    editor,
                    preview,
                    cursor,
                    &mut selection_mirror,
                    live,
                    egui::Id::new(FOCUS_EDITOR_ID),
                    false,
                    show_focus,
                    &crate::keymap::Keymap::builtin(),
                    &mut outbox,
                );
                let bg = ui.visuals().text_edit_bg_color();
                fade = egui::Color32::from_rgba_unmultiplied(
                    bg.r(),
                    bg.g(),
                    bg.b(),
                    crate::ui::focus::DIM_ALPHA,
                );
            },
        );
        let mut fades = Vec::new();
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            match &clipped.shape {
                egui::epaint::Shape::Rect(shape) if shape.fill == fade => fades.push(shape.rect),
                egui::epaint::Shape::Text(t) => texts.push((
                    t.galley.text().to_owned(),
                    egui::Rect::from_min_size(t.pos, t.galley.size()),
                )),
                _ => {}
            }
        }
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        (fades, texts, shapes)
    }

    /// 块文本的渲染 rect(遮罩「该盖谁/不该盖谁」的取证锚点)。
    fn text_rect(texts: &[(String, egui::Rect)], needle: &str) -> egui::Rect {
        texts
            .iter()
            .find(|(text, _)| text.contains(needle))
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("文本 {needle} 未渲染:{texts:?}"))
    }

    fn focus_state() -> (EditorBuffer, PreviewState, OutlineCursor, LiveState) {
        let editor = EditorBuffer::new(FOCUS_DOC);
        let preview = PreviewState::new(&editor);
        (
            editor,
            preview,
            OutlineCursor::default(),
            LiveState {
                active: Some(2),
                ..LiveState::default()
            },
        )
    }

    /// 开启 = 非活动块绘制淡化(像素取证):恰两枚遮罩,分别盖住距活动块
    /// ≥2 的块 0 与块 4;活动块(2)与光标邻块(1、3)不被任何遮罩覆盖。
    #[test]
    fn focus_mode_fades_distant_blocks_keeps_active_and_neighbors() {
        let ctx = egui::Context::default();
        let (mut editor, mut preview, mut cursor, mut live) = focus_state();

        let (fades, texts, _) = live_focus_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
        );
        assert_eq!(fades.len(), 2, "淡化块 = {{0, 4}}:{fades:?}");
        // 遮罩与块矩形同形状全宽(点击命中区同一 rect),相邻遮罩与邻块
        // 文本边界相接不算「盖住」 —— 判定用文本 rect 中心是否落入遮罩。
        for needle in ["block zero", "block four"] {
            let center = text_rect(&texts, needle).center();
            assert!(
                fades.iter().any(|fade| fade.contains(center)),
                "{needle} 应被遮罩盖住:{fades:?} vs 中心 {center:?}"
            );
        }
        for needle in ["block one", "block two", "block three"] {
            let center = text_rect(&texts, needle).center();
            assert!(
                fades.iter().all(|fade| !fade.contains(center)),
                "{needle}(活动块/邻块)不该被遮罩盖住:{fades:?} vs 中心 {center:?}"
            );
        }
    }

    /// 点击淡化块进入编辑且恢复正常:遮罩不参与命中,点击仍触发
    /// `clicked_for_edit`;块切换后焦点与光标路由到新活动块,下一帧该块
    /// 无遮罩,原活动块(与新活动块隔一块)转为淡化。
    #[test]
    fn focus_click_on_faded_block_enters_edit_and_recovers() {
        let ctx = egui::Context::default();
        let (mut editor, mut preview, mut cursor, mut live) = focus_state();
        let (_, texts, _) = live_focus_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
        );
        let target = text_rect(&texts, "block four").center();
        for events in click_events(target) {
            let _ = live_focus_frame(
                &ctx,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                true,
            );
        }
        assert_eq!(live.active, Some(4), "点击淡化块进入编辑");

        // pending_caret 落地帧:焦点与光标落进块 4
        let _ = live_focus_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
        );
        let id = egui::Id::new(FOCUS_EDITOR_ID).with(("live-block", 4));
        assert!(
            ctx.memory(|mem| mem.has_focus(id)),
            "点击后块 4 TextEdit 持焦"
        );

        // 恢复正常:块 4 不再淡化;淡化集随活动块移到 {0, 1, 2}
        let (fades, texts, _) = live_focus_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
        );
        assert_eq!(fades.len(), 3, "淡化块 = {{0, 1, 2}}:{fades:?}");
        let center = text_rect(&texts, "block four").center();
        assert!(
            fades.iter().all(|fade| !fade.contains(center)),
            "点过的块已恢复正常(无遮罩):{fades:?} vs 中心 {center:?}"
        );
    }

    /// 否决线:关闭 = 逐像素现状。关闭帧零遮罩;重复两帧 shape 序列逐项
    /// 相等(确定性);开启帧剔除遮罩 shape 后与关闭帧完全一致(遮罩对
    /// 其余绘制零扰动);开启/关闭两帧整体不同(开关真的改变输出)。
    #[test]
    fn focus_off_matches_baseline_shapes_pixel_for_pixel() {
        let ctx = egui::Context::default();
        let (mut editor, mut preview, mut cursor, mut live) = focus_state();

        let (off_fades, _, off_shapes) = live_focus_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
        );
        assert!(off_fades.is_empty(), "关闭帧零遮罩形状:{off_fades:?}");

        let (off_fades2, _, off_shapes2) = live_focus_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
        );
        assert!(off_fades2.is_empty());
        assert_eq!(off_shapes, off_shapes2, "关闭帧逐 shape 确定重现");

        // 开启帧:剔除遮罩后应与关闭帧逐 shape 一致(纯叠加,零扰动)
        let (on_fades, _, on_shapes) = live_focus_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
        );
        assert_eq!(on_fades.len(), 2, "前置:开启帧有两枚遮罩");
        let bg = ctx.global_style().visuals.text_edit_bg_color();
        let fade = egui::Color32::from_rgba_unmultiplied(
            bg.r(),
            bg.g(),
            bg.b(),
            crate::ui::focus::DIM_ALPHA,
        );
        let on_without_fades: Vec<_> = on_shapes
            .into_iter()
            .filter(|clipped| {
                !matches!(
                    &clipped.shape,
                    egui::epaint::Shape::Rect(shape) if shape.fill == fade
                )
            })
            .collect();
        assert_eq!(
            on_without_fades, off_shapes,
            "开启帧剔除遮罩后与关闭帧逐 shape 一致(遮罩零扰动)"
        );
    }

    /// 开关叠加组合矩阵:打字机 × 专注 4 组合各画一帧不 panic;遮罩数量
    /// 只由专注开关决定(与打字机正交,两开关独立)。
    #[test]
    fn focus_and_typewriter_matrix_independent() {
        for (typewriter, focus) in [(false, false), (true, false), (false, true), (true, true)] {
            let ctx = egui::Context::default();
            let (mut editor, mut preview, mut cursor, mut live) = focus_state();
            let mut outbox = Vec::new();
            let mut fade = egui::Color32::TRANSPARENT;
            let mut selection_mirror = None;
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                super::ui(
                    ui,
                    &mut editor,
                    &mut preview,
                    &mut cursor,
                    &mut selection_mirror,
                    &mut live,
                    egui::Id::new(FOCUS_EDITOR_ID),
                    typewriter,
                    focus,
                    &crate::keymap::Keymap::builtin(),
                    &mut outbox,
                );
                let bg = ui.visuals().text_edit_bg_color();
                fade = egui::Color32::from_rgba_unmultiplied(
                    bg.r(),
                    bg.g(),
                    bg.b(),
                    crate::ui::focus::DIM_ALPHA,
                );
            });
            let fades = output
                .shapes
                .iter()
                .filter(|clipped| {
                    matches!(
                        &clipped.shape,
                        egui::epaint::Shape::Rect(shape) if shape.fill == fade
                    )
                })
                .count();
            let want = if focus { 2 } else { 0 };
            assert_eq!(
                fades, want,
                "打字机={typewriter} 专注={focus}:遮罩数只随专注开关"
            );
            output.drop_without_applying_deltas();
        }
    }

    /// 明暗两主题:遮罩都画(存在性断言,非目视裁决 —— 对比度折算见
    /// focus::DIM_ALPHA 文档,真机观感留人工)。
    #[test]
    fn focus_fades_under_both_themes() {
        for (name, visuals) in [
            ("亮", egui::Visuals::light()),
            ("暗", egui::Visuals::dark()),
        ] {
            let ctx = egui::Context::default();
            ctx.set_visuals(visuals);
            let (mut editor, mut preview, mut cursor, mut live) = focus_state();
            let (fades, _, _) = live_focus_frame(
                &ctx,
                Vec::new(),
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                true,
            );
            assert_eq!(fades.len(), 2, "{name}色主题下遮罩仍在:{fades:?}");
        }
    }

    /// 无活动块(光标未落):全部正常,零遮罩 —— 专注的对象是「正在编辑
    /// 的块」,没有编辑焦点就没有淡化(decisions-pending #122)。
    #[test]
    fn focus_no_active_block_fades_nothing() {
        let ctx = egui::Context::default();
        let (mut editor, mut preview, mut cursor, mut live) = focus_state();
        live.active = None;

        let (fades, _, _) = live_focus_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            true,
        );
        assert!(fades.is_empty(), "无活动块不淡化任何块:{fades:?}");
    }

    // —— ==高亮== Live 接入(#65 M2)——

    /// 富渲染块改写:高亮层先跑、任务层后跑(与预览四层链的生产顺序一
    /// 致),缓存命中直取;无目标字符的块恒等(零变化护栏)。
    #[test]
    fn live_rich_block_stacks_highlight_and_task_layers() {
        let mut live = LiveState::default();
        let doc = "- [ ] ==完== 成\n";
        let (rendered, _, _, _, _) = live.rich_block(0, 1, doc);
        assert!(rendered.contains("[完](<hl://>)"), "高亮层先跑:{rendered}");
        assert!(rendered.contains("task://"), "任务层后跑:{rendered}");
        assert!(!rendered.contains("=="), "标记被改写消费,不回流:{rendered}");

        // 缓存:同修订号直取同一份
        assert_eq!(live.rich_block(0, 1, doc).0, rendered, "同修订号缓存命中");

        // 零变化护栏:无 == 无任务标记的块恒等
        assert_eq!(
            live.rich_block(1, 1, "普通段落。\n").0.as_ref(),
            "普通段落。\n",
            "无目标字符的块改写恒等"
        );
    }

    /// Live handler 收口:`hl://` 接管排版段(底色 = 与预览同一共享推导
    /// 真源)、link_style 正文色无下划线、click 吞掉(不开浏览器);任务
    /// checkbox 路径分毫不动。
    #[test]
    fn live_highlight_handler_contained() {
        let ctx = egui::Context::default();
        let mermaid = crate::ui::mermaid::LiveMermaidHandler::new();
        let body = egui::Color32::from_rgb(0x11, 0x22, 0x33);
        let handler = LiveRichHandler::new(&mermaid, 0, body, false);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let font = egui::FontId::proportional(15.0);
            let mut job = egui::text::LayoutJob::default();
            assert!(
                handler.layout_link(
                    ui,
                    "完",
                    "hl://",
                    &mut job,
                    &font,
                    egui::Color32::TRANSPARENT
                ),
                "Live 里 hl:// 同样接管排版段"
            );
            assert_eq!(job.sections.len(), 1, "恰一段(非 inline widget 占位)");
            assert_eq!(
                job.sections[0].format.background,
                crate::ui::preview::highlight_bg_color(ui.visuals()),
                "底色 = 与预览同一推导真源"
            );
            let style = handler.link_style("hl://").expect("hl:// 有样式");
            assert_eq!(style.color, Some(body), "正文色");
            assert!(!style.underline, "无下划线");
            assert!(handler.click("完", "hl://", ui), "点击吞掉,不开浏览器");
            // 任务路径不受牵连:仍走 inline widget 占位
            let mut task_job = egui::text::LayoutJob::default();
            assert!(
                handler.layout_link(
                    ui,
                    " ",
                    "task://c",
                    &mut task_job,
                    &font,
                    egui::Color32::TRANSPARENT
                ),
                "task:// 仍走占位路径"
            );
            assert_eq!(
                task_job.sections[0].format.color,
                egui::Color32::TRANSPARENT,
                "task:// 占位仍是透明文字"
            );
        });
        output.drop_without_applying_deltas();
    }

    /// 面板端到端:Live 富渲染块(非活动)里的 `==高亮==` 与右栏预览同一
    /// 观感 —— 底色段落进 galley,且不开浏览器(`click` 吞掉后 vendored
    /// 不会把 `hl://` 交给 `OpenUrl`)。
    #[test]
    fn live_rich_block_renders_highlight_section() {
        use eframe::epaint::text::ByteRangeExt as _;
        let ctx = egui::Context::default();
        let mut editor =
            EditorBuffer::new("正文 ==高亮内容== 收尾\n\n[链接](https://example.com)\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();
        let mut outbox = Vec::new();
        let mut selection_mirror = None;
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            super::ui(
                ui,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut selection_mirror,
                &mut live,
                egui::Id::new("live-highlight-test"),
                false,
                false,
                &crate::keymap::Keymap::builtin(),
                &mut outbox,
            );
        });
        let hl = crate::ui::preview::highlight_bg_color(&egui::Visuals::dark());
        let mut marked = Vec::new();
        for clipped in &output.shapes {
            if let egui::epaint::Shape::Text(t) = &clipped.shape {
                for section in &t.galley.job.sections {
                    if section.format.background == hl {
                        marked.push(section.byte_range.slice(&t.galley.job.text).to_owned());
                    }
                }
            }
        }
        assert_eq!(
            marked,
            vec!["高亮内容".to_owned()],
            "Live 富渲染块恰一段高亮底色:{marked:?}"
        );
        assert!(
            !output
                .platform_output
                .commands
                .iter()
                .any(|cmd| matches!(cmd, egui::OutputCommand::OpenUrl(_))),
            "无浏览器弹出(hl:// 点击即使触发也吞掉)"
        );
        output.drop_without_applying_deltas();
    }

    /// 首行前缀粗分类:标题要空格(CommonMark)、列表是文本流、
    /// `***加粗开头**` 不是分割线。
    #[test]
    fn block_kind_classifies_by_first_line() {
        use super::BlockKind;
        assert_eq!(super::block_kind("# 标题"), BlockKind::Heading);
        assert_eq!(super::block_kind("###### 六级"), BlockKind::Heading);
        assert_eq!(super::block_kind("##无空格不是标题"), BlockKind::Text);
        assert_eq!(super::block_kind("####### 七个"), BlockKind::Text);
        assert_eq!(
            super::block_kind("```rust\nfn a(){}\n```\n"),
            BlockKind::Widget
        );
        assert_eq!(super::block_kind("> 引用"), BlockKind::Widget);
        assert_eq!(super::block_kind("| a | b |\n| - | - |"), BlockKind::Widget);
        assert_eq!(super::block_kind("---"), BlockKind::Widget);
        assert_eq!(super::block_kind("* * *"), BlockKind::Widget);
        assert_eq!(super::block_kind("![图](x.png)"), BlockKind::Widget);
        assert_eq!(super::block_kind("- 列表项"), BlockKind::Text);
        assert_eq!(super::block_kind("- [ ] 任务"), BlockKind::Text);
        assert_eq!(super::block_kind("1. 有序"), BlockKind::Text);
        assert_eq!(super::block_kind("***加粗开头**"), BlockKind::Text);
        assert_eq!(super::block_kind("段落"), BlockKind::Text);
    }

    /// 前距矩阵逐项对齐 vendored 单标签流(见 [`super::block_gap`] 的
    /// 三机制对照表)。
    #[test]
    fn block_gap_mirrors_the_single_label_rhythm() {
        use super::BlockKind;
        let style = MarkdownStyle::default();
        let line = super::body_line_height(&style, 15.0);
        assert!((line - 19.5).abs() < 1e-4, "出厂 15pt×1.30 → 行盒 19.5");
        let s = style.block_spacing;
        let h = style.heading_space_above;
        // 文本 → 文本:一个空行
        assert_eq!(
            super::block_gap(&style, 15.0, BlockKind::Text, BlockKind::Text),
            line
        );
        // 文本 → 标题:空行 + spacer 行
        assert_eq!(
            super::block_gap(&style, 15.0, BlockKind::Text, BlockKind::Heading),
            line + s + h
        );
        // 文本 → 元件:空行 + before_block
        assert_eq!(
            super::block_gap(&style, 15.0, BlockKind::Text, BlockKind::Widget),
            line + s
        );
        // 元件 → 文本:仅 after_block(元件后无空行)
        assert_eq!(
            super::block_gap(&style, 15.0, BlockKind::Widget, BlockKind::Text),
            s
        );
        // 元件 → 标题:after_block + spacer 行
        assert_eq!(
            super::block_gap(&style, 15.0, BlockKind::Widget, BlockKind::Heading),
            s + s + h
        );
        // 标题 → 文本:空行(标题不是元件)
        assert_eq!(
            super::block_gap(&style, 15.0, BlockKind::Heading, BlockKind::Text),
            line
        );
        // CJK 行高下限兜底:floor 抬行盒
        let floored = MarkdownStyle {
            min_line_height_em: 2.0,
            ..MarkdownStyle::default()
        };
        assert!(super::body_line_height(&floored, 15.0) > line);
    }

    /// 块间前距逐像素守门(2026-10-10「Live 太紧凑」):标签裁尾随空行、
    /// item_spacing 归零后,相邻文本块行盒之间的实测空隙必须等于
    /// `block_gap` 的演算值(出厂样式 15pt×1.30 → 行盒 19.5)。改版前
    /// 实测全部边界都是 3px(egui 出厂 item_spacing),本测试当场抓它。
    /// 元件边界(代码块自绘框有内距、语言头与内容行纵向重叠)只断
    /// 「已拉开」下界。
    #[test]
    fn inter_block_gaps_match_the_preview_rhythm() {
        let ctx = egui::Context::default();
        let doc = "# 标题\n\n段落甲正文。\n\n## 二级标题\n\n段落乙正文。\n\n```rust\nfn a(){}\n```\n\n段落丙。\n";
        let (mut editor, mut live, mut cursor) = state_with(doc);
        let mut preview = PreviewState::new(&editor);
        let mut outbox = Vec::new();
        let mut sink = None;
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            super::ui(
                ui,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut sink,
                &mut live,
                egui::Id::new("gap-test"),
                false,
                false,
                &crate::keymap::Keymap::builtin(),
                &mut outbox,
            );
        });
        let mut rows: Vec<(f32, f32, String)> = Vec::new();
        for clipped in &output.shapes {
            if let egui::epaint::Shape::Text(t) = &clipped.shape {
                let text = t.galley.text().to_owned();
                if text.trim().is_empty() {
                    continue;
                }
                let rect = egui::Rect::from_min_size(t.pos, t.galley.size());
                rows.push((rect.top(), rect.bottom(), text));
            }
        }
        output.drop_without_applying_deltas();
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));

        let gap_between = |from: &str, to: &str| -> f32 {
            let a = rows.iter().rev().find(|r| r.2.trim() == from).expect(from);
            let b = rows
                .iter()
                .find(|r| r.2.trim() == to && r.0 >= a.1)
                .expect(to);
            b.0 - a.1
        };
        let style = MarkdownStyle::default();
        let line = super::body_line_height(&style, 15.0);
        let s = style.block_spacing;
        let h = style.heading_space_above;
        // 文本边界:恰等于演算值
        assert!(
            (gap_between("段落甲正文。", "二级标题") - (line + s + h)).abs() < 0.6,
            "段落 → 二级标题 = 空行 + spacer"
        );
        assert!(
            (gap_between("二级标题", "段落乙正文。") - line).abs() < 0.6,
            "标题 → 段落 = 一个空行"
        );
        assert!(
            (gap_between("标题", "段落甲正文。") - line).abs() < 0.6,
            "首块之后的第一个边界同为空行"
        );
        // 元件边界:自绘框有内距,断「已拉开」下界(改版前实测 3px)
        assert!(
            gap_between("段落乙正文。", "fn a(){}") >= s,
            "段落 → 代码块前距:{:?}",
            gap_between("段落乙正文。", "fn a(){}")
        );
        assert!(
            gap_between("fn a(){}", "段落丙。") >= s,
            "代码块 → 段落前距:{:?}",
            gap_between("fn a(){}", "段落丙。")
        );

        // 行距同源(2026-10-10 用户问「Live 和预览能一样吗」):两栏的
        // 标签用同一 `FontId`(字号档 + 预览族)与同一全局 MarkdownStyle,
        // 行盒高必须逐像素相等 —— 谁改了其中一栏的字体取值,这里变红。
        let out2 = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui_markdown::MarkdownLabel::new(egui::Id::new("gap-test-preview"), doc)
                .font(egui::FontId::new(
                    crate::theme::editor_font_size(ui.ctx()),
                    crate::fonts::preview_body_family(ui.ctx()),
                ))
                .wrap()
                .show(ui);
        });
        // 预览整篇是单个 galley shape:按行遍历取目标段的行盒
        let preview_row = out2
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(t) => {
                    let mut start = 0usize;
                    t.galley.rows.iter().find_map(move |row| {
                        let count = row.char_count_including_newline().0;
                        let text: String =
                            t.galley.job.text.chars().skip(start).take(count).collect();
                        start += count;
                        (text.trim() == "段落甲正文。").then_some(row.size.y)
                    })
                }
                _ => None,
            })
            .next()
            .expect("预览单标签里有该段行");
        let live_row = rows
            .iter()
            .find(|r| r.2.trim() == "段落甲正文。")
            .map(|r| r.1 - r.0)
            .expect("Live 里有该段行");
        assert!(
            (preview_row - live_row).abs() < 0.6,
            "行距须同源:预览行盒 {preview_row}px vs Live 行盒 {live_row}px"
        );
        out2.drop_without_applying_deltas();
    }
}
