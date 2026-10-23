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
//!
//! LP2-4 补**内联构造自动成形**(2026-10-20「输入后自动变成对应格式」):
//! 活动块里**已配平**的内联构造(`**粗体**`、`` `代码` ``、`~~删除线~~`、
//! `==高亮==`、`[链接]`、`*斜体*`)的内容直接套渲染格式 —— 区间判定是纯
//! 函数 `latermd_md::inline_style_spans`(与渲染同一套 pulldown options,
//! 未配平不出区间 = 打完闭标记的那一刻成形),排版落在
//! [`active_block_job`] 的分段作业里(配方逐条镜像 vendored 富渲染),
//! 定界符仍由 LP2-1 遮罩淡化 —— 成形与淡化两层职责互不侵吞,光标/选区/
//! IME 通路零改动(还是那一个 TextEdit,只是排版变了)。
//!
//! LP2-5 补**列表点击保持位置**(2026-10-20「点击列表项不再缩进跳变」):
//! 活动块是列表项时,前缀(缩进空白、marker、`[ ]` 任务标记)幽灵化
//! (ATX 前缀同款),内容列按 vendored `layout.rs` ListMarker 分支的同一
//! 套公式对位(marker 右对齐 `"99. "` 槽位、内容落 槽位+gap+勾选框列)
//! —— 点击进编辑与富渲染的内容 x 逐像素一致,不再「往前缩」。

use std::{
    hash::{Hash, Hasher},
    ops::Range,
    sync::Arc,
};

use eframe::egui;
use egui_markdown::MarkdownLabel;
use egui_markdown_style::MarkdownStyle;
use latermd_editor::EditorBuffer;

use crate::ai::AiState;
use crate::state::{Message, OutlineCursor, PreviewState};
use crate::ui::ai_card::RichLinkHandler;
use crate::ui::rich_widgets::TaskProbeDomain;

type RichCacheEntry = (
    u64,
    Arc<str>,
    Arc<latermd_md::OffsetMap>,
    Arc<latermd_md::OffsetMap>,
    Arc<latermd_md::OffsetMap>,
    Arc<latermd_md::OffsetMap>,
    Arc<latermd_md::OffsetMap>,
    Option<std::path::PathBuf>,
);
type RichBlock = (
    Arc<str>,
    Arc<latermd_md::OffsetMap>,
    Arc<latermd_md::OffsetMap>,
    Arc<latermd_md::OffsetMap>,
    Arc<latermd_md::OffsetMap>,
    Arc<latermd_md::OffsetMap>,
);

/// 徽标槽宽度(LP2-7):写作栏内容左移量,左侧悬一个**圆点手柄图标**
/// (2×3 点阵,Notion 手柄同款),点击弹快捷菜单切换标题级别/行内格式。
/// 用户反馈 44px 文字芯片「左右偏移的更狠了」,收窄到 16px 只容图标。
pub const BADGE_GUTTER: f32 = 16.0;

/// 圆点手柄的点阵(相对芯片左上):2 列 × 3 行,Notion 拖拽手柄同款。
const HANDLE_DOTS: [egui::Vec2; 6] = [
    egui::vec2(3.0, 2.0),
    egui::vec2(8.0, 2.0),
    egui::vec2(3.0, 6.5),
    egui::vec2(8.0, 6.5),
    egui::vec2(3.0, 11.0),
    egui::vec2(8.0, 11.0),
];

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
///
/// **三态(2026-10-15)**:`Split` 是「源码 + 只读对照列」—— 中央仍是源码
/// 模式那一个 TextEdit,右侧多一列由 `live::ui` 渲染的**只读**富渲染。它与
/// `Live` 互斥(不是正交开关),故不需要「离开某模式时该不该恢复右栏」那类
/// 快照补工;旧存档的 `"source"` / `"live"` 照旧解析,零迁移。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderMode {
    /// 整篇源码。
    #[default]
    Source,
    /// 光标块源码、其余富渲染。
    Live,
    /// 源码 + 右侧只读对照列(渲染同一份文档的 Live 富渲染)。
    Split,
}

impl RenderMode {
    /// 循环切档(命令 `Ctrl/Cmd+/` 与标题栏三段开关的同一入口):
    /// `Source → Live → Split → Source`。
    pub fn cycle(self) -> Self {
        match self {
            Self::Source => Self::Live,
            Self::Live => Self::Split,
            Self::Split => Self::Source,
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
    /// 左键单击块即进入写作态（2026-10-19 拍板，取代早期的双击门槛：
    /// 双击进编辑与 MarkText 的「点哪编哪」手感相悖，且单击进块的
    /// 视口锚定依赖同一入口）。
    pub browse_only: bool,
    /// 是否启用打开文档时的浏览态交互。默认关闭以保持无头测试夹具和
    /// 直接构造 LiveState 的既有语义；真实切入 Live 时由 State 打开。
    pub(crate) browse_enabled: bool,
    /// **锁定浏览**(P3/D2):禅定帧内的浏览是只读阅读,点击/双击/跳转一律
    /// 不进编辑、不抢焦点。与 `browse_only` 的区别是后者是「还没开始写」
    /// 的软状态(jump_to 可翻成写作),本字段是阅读场景的硬锁。每帧由
    /// `draw_zen` 置位,不进 `State` 持久化。
    pub(crate) browse_locked: bool,
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
    /// 只读对照列专用:上一次已跟随过的源码光标字节。
    ///
    /// 「跟随一次」的记忆 —— 源码侧光标**变化**时才下一次滚动目标,此后
    /// 右栏保留自己的滚动位置供自由翻阅(用户翻对照列不该被主列拽回)。
    /// `None` = 还没跟随过(或文档刚换入)。
    pub(crate) followed_byte: Option<usize>,
    /// 只读对照列专用:源码列滚轮滚动帧写下的「鼠标行锚点」——(指针所
    /// 在行的源码字节, 指针在源码视口里的相对高度)。`ui::editor` 在滚轮
    /// 帧写入,`ui::layout` 的对照列帧头**消费一次即清**:连续滚轮逐帧
    /// 重下锚,滚轮一停右栏保留自己的滚动位置供自由翻阅。锚点不写
    /// `followed_byte`(那里只记忆光标侧的最后一次),两条跟随
    /// 路径互不拽扯。
    pub(crate) mouse_anchor: Option<(usize, f32)>,
    /// 一次 pending_caret 落地的**对齐意图**:Some(视口相对高度) = 目标块
    /// 顶落到该高度(滚轮锚点的「两侧同一行等高呈现」);None = 现状最小
    /// 滚动(`scroll_to_rect(rect, None)`,光标跟随/大纲跳转同款)。与
    /// pending_caret 同帧置位、同帧消费;光标跟随路径入口处显式清,
    /// 防残留的旧意图劫持一次普通跳转。
    pub(crate) anchor_align_y: Option<f32>,
    /// 点击进编辑的视口锚定（块切换防跳变）：(目标块, 被点击富渲染行的
    /// 屏幕顶 y)。块从富渲染切到源码形态高度必变，视口内内容整体平移
    /// —— 点击帧已按预测把 ScrollArea 偏移先行修正（见循环内
    /// `pending_shift` 的产出与帧末落账），本字段承载**落地帧**的残差
    /// 校正：实测源码光标行顶与锚点的差超过 0.5px 即做一次同帧补偿。
    /// 与 pending_caret 同生共死：jump_to/路由覆盖 pending 时一并清空，
    /// 落地帧消费即清；`None` = 无待校正锚点。
    pub(crate) toggle_anchor: Option<(usize, f32)>,
    /// 富渲染块链接改写的块级缓存(#63;#65 M2 起高亮层并入同一条缓存):
    /// 块序号 → (源码指纹, 改写后块文本, 高亮层映射, 任务层映射)。
    /// 修订号前进时保留未变化块的条目，只让源码指纹变化的块重新改写，
    /// 避免在大文档中输入一个字符就清空整篇缓存。两张映射随渲染文本
    /// 同源缓存:点击进编辑的落点换算要把渲染文本偏移**逆穿**回块源码坐标。
    rich_cache: std::collections::HashMap<usize, RichCacheEntry>,
    /// 块级改写的待确认动作(2026-10-23):关闭代码块 / 删除表格。挂起中
    /// 每帧渲染确认弹窗,确认才改写。挂在 LiveState(每列一份)而非全局:
    /// 对照列锁浏览(`browse_locked`)根本不会产生动作,互不干扰。
    pub(crate) pending_block_action: Option<PendingBlockAction>,
}

/// 待确认的块级改写。序号在弹窗打开期间是稳定的(模态,文档不动),
/// 确认帧再取 `blocks[index]`,不在点击帧缓存区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingBlockAction {
    /// 关闭代码块:去围栏,内容保留为普通文本。
    CodeClose(usize),
    /// 删除整个表格块。
    TableDelete(usize),
}

impl PendingBlockAction {
    /// 确认弹窗文案:(标题, 导语, 警示, 确认按钮)。
    fn dialog_text(self) -> (&'static str, String, &'static str, &'static str) {
        match self {
            Self::CodeClose(_) => (
                "关闭代码块",
                "把此代码块转为普通文本。".to_owned(),
                "围栏标记将被移除,代码内容保留为正文;Ctrl+Z 可撤销。",
                "转为普通文本",
            ),
            Self::TableDelete(_) => (
                "删除表格",
                "把此表格从文档中移除。".to_owned(),
                "整个表格(含所有行列)将被删除;Ctrl+Z 可撤销。",
                "删除表格",
            ),
        }
    }
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
            // 出 TextEdit；进入写作只由单击或明确的程序跳转触发。
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
    /// 重算并入表。返回渲染文本与**逆穿方向**的映射组(渲染坐标 → 上一层
    /// 坐标,最后一元是图片层):层序与预览链一致(wikilink → 高亮 → emoji
    /// → 任务 → 相对图片 `file://` 改写),各层有「无目标字符不启动解析」
    /// 的快路径,快路径直接恒等返回。
    ///
    /// `base_dir`(相对图片锚定的文档目录)是缓存维度之一(live-unify-plan
    /// R4):同一块源码在不同目录下改写结果不同,换目录/首次落盘必须失效;
    /// 测试传 `None` 走未落盘分支。
    fn rich_block_with_base(
        &mut self,
        index: usize,
        block_text: &str,
        base_dir: Option<&std::path::Path>,
    ) -> RichBlock {
        let hash = self.block_hashes.get(index).copied();
        if let Some((
            seen_hash,
            rendered,
            offset_map,
            highlight_map,
            emoji_map,
            task_map,
            image_map,
            seen_base,
        )) = self.rich_cache.get(&index)
        {
            if Some(*seen_hash) == hash && seen_base.as_deref() == base_dir {
                return (
                    Arc::clone(rendered),
                    Arc::clone(offset_map),
                    Arc::clone(highlight_map),
                    Arc::clone(emoji_map),
                    Arc::clone(task_map),
                    Arc::clone(image_map),
                );
            }
        }
        let (after_wikilinks, offset_map) = latermd_md::expand_wikilinks_with_map(block_text);
        let (after_highlight, highlight_map) = latermd_md::expand_highlight_links(&after_wikilinks);
        let (after_emoji, emoji_map) = latermd_md::expand_emoji_links(
            &after_highlight,
            crate::ui::emoji_data::covered_glyphs(),
        );
        let (after_task, task_map) = latermd_md::expand_task_links(&after_emoji);
        // 第 5 层(live-unify-plan P1):相对图片地址按文档目录改写成
        // `file://` URI —— 写作模式下 `./x.assets/a.png` 同样要出图。与预览
        // 同一份扫描实现(`ui::rich_widgets`),源码不动、只改喂给渲染的串。
        let rewrites = crate::ui::rich_widgets::image_rewrites(&after_task, base_dir);
        let (rendered, image_map) = if rewrites.is_empty() {
            (after_task, latermd_md::OffsetMap::empty())
        } else {
            let map = latermd_md::OffsetMap::from_rewrites(rewrites);
            let out = map.apply(&after_task);
            (out, map)
        };
        let rendered = Arc::<str>::from(rendered);
        let offset_map = Arc::new(offset_map);
        let highlight_map = Arc::new(highlight_map);
        let emoji_map = Arc::new(emoji_map);
        let task_map = Arc::new(task_map);
        let image_map = Arc::new(image_map);
        self.rich_cache.insert(
            index,
            (
                hash.unwrap_or_else(|| source_hash(block_text)),
                Arc::clone(&rendered),
                Arc::clone(&offset_map),
                Arc::clone(&highlight_map),
                Arc::clone(&emoji_map),
                Arc::clone(&task_map),
                Arc::clone(&image_map),
                base_dir.map(std::path::Path::to_path_buf),
            ),
        );
        (
            rendered,
            offset_map,
            highlight_map,
            emoji_map,
            task_map,
            image_map,
        )
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
        image_map: &latermd_md::OffsetMap,
        offset_map: &latermd_md::OffsetMap,
        highlight_map: &latermd_md::OffsetMap,
        emoji_map: &latermd_md::OffsetMap,
        task_map: &latermd_md::OffsetMap,
        block_source_len: usize,
    ) -> Option<usize> {
        let rendered_byte = match egui_markdown::char_index_at_pos(ui, label_id, pos) {
            Some(char_index) => {
                // 命中表给的是排版 job 文本字符序号;单段块经映射表折回
                // rendered 字符序号(标题剥 `# `、列表换 `• `、任务换 `☐`
                // 都在这里归位),多段块保持旧口径(恒等)。
                let rendered_char =
                    JobRenderMap::build(rendered).map_or(char_index, |map| map.lookup(char_index));
                char_indices_to_byte(rendered, rendered_char)
            }
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
        // 逆穿改写层(生产顺序 wikilink → 高亮 → emoji → 任务 → 图片,
        // 逆序 图片 → 任务 → emoji → 高亮 → wikilink);改写段内部命中映
        // 回段首,是映射表的既定语义。
        let after_task = image_map.rendered_to_source(rendered_byte);
        let after_emoji = task_map.rendered_to_source(after_task);
        let after_highlight = emoji_map.rendered_to_source(after_emoji);
        let after_wikilink = highlight_map.rendered_to_source(after_highlight);
        let source_byte = offset_map.rendered_to_source(after_wikilink);
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
        self.browse_locked = false;
        self.pending_caret = None;
        self.pending_range = None;
        self.synced_rev = None;
        self.marks = None;
        self.drag_anchor = None;
        self.caret_follow = false;
        self.followed_byte = None;
        self.mouse_anchor = None;
        self.anchor_align_y = None;
        self.toggle_anchor = None;
        self.rich_cache.clear();
        self.block_hashes.clear();
        self.pending_block_action = None;
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
    /// 编辑视图的字节长(从块首起):活动块裁掉块尾的幻影空行(块间空行
    /// 归前一块,富渲染本就忽略它们),块切源码不再凭空多出 1-2 个空行
    /// —— 那是「点击后块展开」的主要来源。裁剪的是**前缀截断**,块内
    /// 字符偏移在截断前后同义,编辑换算(`base + char`)零改动;全块
    /// 字节仍被块表覆盖,不存在编辑丢字。默认取全块。
    visible_bytes: usize,
}

impl BlockBuffer<'_> {
    /// 全块视图(块首起整个区间)。
    fn full(buffer: &mut EditorBuffer, range: Range<usize>) -> BlockBuffer<'_> {
        let visible_bytes = range.end.saturating_sub(range.start);
        BlockBuffer {
            buffer,
            range,
            visible_bytes,
        }
    }
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
        let end = self.range.start + self.visible_bytes.min(self.range.len());
        Self::slice(self.buffer.text(), &(self.range.start..end))
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

/// Live 帧 emoji 占位区探针的 data 键(P1,与预览 `emoji_probe_id` 同款,
/// 按编辑器 id 分标签):生产只写不读,无头测试读它断言「widget 在该段
/// 生效」。
fn emoji_probe_id(editor_id: egui::Id) -> egui::Id {
    egui::Id::new("latermd-live-emoji-rects").with(editor_id)
}

/// 锁定浏览的跳转落地(屏外占位臂与真实渲染臂共用):无对齐意图时最小
/// 滚动(大纲跳转/光标跟随的现状);带 `anchor_align_y` 意图时块顶对齐
/// 到指针在源码视口里的相对高度(滚轮锚点的「两侧同一行等高呈现」),
/// 块比视口高时钳回顶对齐 —— 块内的行级定位要走富渲染逆穿,不在本路径
/// 的能力面。egui 相对增量的符号:queue 后被取负应用(`offset -= queued`),
/// 正 = 向上滚;目标差为负表示块在目标位置下方,需向下滚把它抬上来。
fn locked_jump_landing(ui: &egui::Ui, live: &mut LiveState, rect: egui::Rect) {
    match live.anchor_align_y.take() {
        Some(rel) => {
            let clip = ui.clip_rect();
            let rel = rel.clamp(0.0, (clip.height() - rect.height()).max(0.0));
            let delta = clip.top() + rel - rect.top();
            ui.scroll_with_delta_animation(
                egui::vec2(0.0, delta),
                egui::style::ScrollAnimation::none(),
            );
        }
        None => ui.scroll_to_rect(rect, None),
    }
}

/// LP2-4 内联构造自动成形的着色参数(逐条镜像 vendored 富渲染的配方):
/// 粗体走 bold 族(无族回退 strong 色,vendored `apply_bold` 同式)、斜体
/// 假斜(vendored `TokenStyle.italic` 同式)、删除线 1px 基色描边
/// (vendored `Stroke::new(1.0, color)` 同式)、行内代码 = 正文族 +
/// `InlineCodeStyle` 前景/底色/外扩(vendored `apply_inline_code_bg`
/// 同式,**字体不换** —— 富渲染的行内代码就是正文字体)、高亮 =
/// `highlight_bg_color` 推导底色、链接文字 = `hyperlink_color`。
struct InlineStylePaint {
    has_bold: bool,
    bold_family: egui::FontFamily,
    strong_color: egui::Color32,
    hyperlink_color: egui::Color32,
    dark_mode: bool,
    inline_code: egui_markdown_style::InlineCodeStyle,
    highlight_bg: egui::Color32,
}

/// 把 [`latermd_md::inline_style_spans`] 的样式区间按字节边界切进 `region`,
/// 每段从 `base` 克隆后叠加命中的样式集(LP2-4「输入后自动成形」的排版
/// 落盘)。行内代码整段自成样式、不吃外层强调/链接色 —— vendored 渲染
/// 「Code 事件重置 TokenStyle」同一语义。区间被 `region` 截断(构造跨
/// 标题行边界等)时按截断后命中;样式表为空或 region 为空退化为单
/// section,与 LP2-4 之前逐字节等价。
fn push_inline_styled_sections(
    job: &mut egui::text::LayoutJob,
    region: Range<usize>,
    base: &egui::TextFormat,
    styles: &[latermd_md::InlineStyleSpan],
    paint: &InlineStylePaint,
) {
    let hit = |kind: latermd_md::InlineStyleKind, seg: &Range<usize>| {
        styles.iter().any(|span| {
            span.kind == kind && span.range.start <= seg.start && seg.end <= span.range.end
        })
    };
    let mut points = vec![region.start, region.end];
    for span in styles {
        if span.range.start < region.end && region.start < span.range.end {
            points.push(span.range.start.max(region.start));
            points.push(span.range.end.min(region.end));
        }
    }
    points.sort_unstable();
    points.dedup();
    for window in points.windows(2) {
        let seg = window[0]..window[1];
        let mut format = base.clone();
        if hit(latermd_md::InlineStyleKind::Code, &seg) {
            format.color = paint.inline_code.color(paint.dark_mode);
            format.background = paint.inline_code.background(paint.dark_mode);
            format.expand_bg = paint.inline_code.expand_bg;
        } else {
            if hit(latermd_md::InlineStyleKind::Strong, &seg) {
                if paint.has_bold {
                    format.font_id.family = paint.bold_family.clone();
                } else {
                    format.color = paint.strong_color;
                }
            }
            if hit(latermd_md::InlineStyleKind::Emphasis, &seg) {
                format.italics = true;
            }
            if hit(latermd_md::InlineStyleKind::Strikethrough, &seg) {
                format.strikethrough = egui::Stroke::new(1.0, base.color);
            }
            if hit(latermd_md::InlineStyleKind::LinkText, &seg) {
                format.color = paint.hyperlink_color;
            }
            if hit(latermd_md::InlineStyleKind::Highlight, &seg) {
                format.background = paint.highlight_bg;
                format.expand_bg = crate::ui::rich_widgets::HIGHLIGHT_BG_EXPAND;
            }
        }
        job.sections.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: seg.start.into()..seg.end.into(),
            format,
        });
    }
}

/// 活动块编辑视图的排版作业(#158 渲染形态原地编辑):**文本与缓冲不变**,
/// 只换排版 —— Text 块用预览正文字体逐行排版(与富渲染同字体同字号同行
/// 盒),Heading 块的标题行按 vendored 同式放大加粗(字号 ×
/// `heading.scales[level]`、`bold` 族、strong 色文案、行高按放大后字号
/// 重推导),ATX 前缀 `# ` 淡化为弱色正文小字;Widget 块(代码/表格/引用/
/// 分隔线/图片)保持等宽(对齐表格与代码列,与既有源码编辑一致)。编辑态
/// 与渲染态的字体、字号、行高全部同源取值 —— 点击进标题块不再「缩成
/// 小字」,配合幻影行裁剪连标题块的高度都几乎不变。
/// 行内已配平的构造由 [`push_inline_styled_sections`] 直接套渲染格式
/// (LP2-4「输入后自动成形」:`**粗**` 出粗体、`` `代码` `` 出代码药丸
/// ……),定界符仍由 LP2-1 遮罩层淡化;列表项首行走 LP2-5 的幽灵前缀 +
/// 富渲染同式内容列对位(见 [`list_prefix`]);预测函数与编辑视图共用
/// 本函数,视口锚定的预测精度随排版同源自动保持。
fn active_block_job(ui: &egui::Ui, text: &str, wrap_width: f32) -> egui::text::LayoutJob {
    let style = egui_markdown_style::global_style(ui.ctx());
    let font_size = crate::theme::editor_font_size(ui.ctx());
    let body_family = crate::fonts::preview_body_family(ui.ctx());
    let mono_font = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Monospace)
        .cloned()
        .unwrap_or_else(|| egui::FontId::monospace(14.0));
    let body_lh = body_line_height(&style, font_size);
    let text_color = ui.visuals().text_color();
    let bold_family = egui::FontFamily::Name(std::sync::Arc::from("bold"));
    let has_bold = ui.ctx().fonts(|f| f.families().contains(&bold_family));

    let mut job = egui::text::LayoutJob {
        text: text.to_owned(),
        wrap: egui::text::TextWrapping {
            max_width: wrap_width,
            ..egui::text::TextWrapping::default()
        },
        // 默认保留源码换行;普通段落分支会按 Markdown 软换行语义关闭
        // `break_on_newline`,引用/标题/列表/Widget 保持既有逐行口径。
        break_on_newline: true,
        halign: egui::Align::LEFT,
        keep_trailing_whitespace: true,
        ..egui::text::LayoutJob::default()
    };
    let section =
        |byte_range: std::ops::Range<usize>, format: egui::TextFormat| egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: byte_range.start.into()..byte_range.end.into(),
            format,
        };

    // 首行分类提前取出:引用块需要独立的多行排版,不能落进代码/表格等
    // Widget 的等宽整块回退。
    let first_line_end = text.find('\n').unwrap_or(text.len());
    let first_line = &text[..first_line_end];
    let trimmed = first_line.trim_start();
    let is_quote = trimmed.starts_with('>');

    // Widget(代码/表格/分隔线/图片):等宽回退 —— 对齐代码列与表格列。
    // 引用单独走下方的正文族多行分支,保留每个物理换行和 `> ` 前缀。
    if block_kind(text) == BlockKind::Widget && !is_quote {
        job.break_on_newline = true;
        let mut mono = egui::TextFormat::simple(mono_font, text_color);
        mono.line_height = Some(body_lh);
        // 围栏代码的活动态「原样」(#310 需求):代码框感 —— 首尾围栏行
        // 淡成弱色(结构标记退后,内容仍是编辑主体);外框由 TextEdit 的
        // frame 侧补(见活动臂 code_frame)。缩进代码/表格/分隔线维持整块
        // 单色等宽。
        if let Some((marker, open_len)) = fence_open(first_line) {
            let content = text.trim_end_matches('\n');
            if let Some(last_start) = content.rfind('\n').map(|at| at + 1) {
                if is_fence_close(&text[last_start..], marker, open_len) {
                    let weak = egui::Color32::from_rgba_unmultiplied(
                        text_color.r(),
                        text_color.g(),
                        text_color.b(),
                        110,
                    );
                    let mut fence_fmt = mono.clone();
                    fence_fmt.color = weak;
                    let open_end = first_line_end
                        + usize::from(text.as_bytes().get(first_line_end) == Some(&b'\n'));
                    // section 必须按字节序推送(epaint 有序性校验硬门槛)
                    job.sections.push(section(0..open_end, fence_fmt.clone()));
                    if last_start > open_end {
                        job.sections.push(section(open_end..last_start, mono));
                    }
                    job.sections
                        .push(section(last_start..text.len(), fence_fmt));
                    return job;
                }
            }
        }
        job.sections.push(section(0..text.len(), mono));
        return job;
    }

    // ATX 标题首行(与 [`block_kind`] 同判):`#{1,6}` + 空格,缩进行保守
    // 不当标题排版。
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    let is_heading = (1..=6).contains(&hashes)
        && trimmed.as_bytes().get(hashes) == Some(&b' ')
        && first_line.starts_with(trimmed);
    // LP2-4:已配平的内联构造内容直接套渲染格式(纯函数 + 快路径,纯正文
    // 零解析、单 section 行为不变);定界符留给 LP2-1 遮罩。
    let styles = latermd_md::inline_style_spans(text);
    let paint = InlineStylePaint {
        has_bold,
        bold_family: bold_family.clone(),
        strong_color: ui.visuals().strong_text_color(),
        hyperlink_color: ui.visuals().hyperlink_color,
        dark_mode: ui.visuals().dark_mode,
        inline_code: style.inline_code.clone(),
        highlight_bg: crate::ui::rich_widgets::highlight_bg_color(ui.visuals()),
    };
    if is_quote {
        // 引用活动态(LP2-8):富渲染的 Blockquote 是正文流,源码中的
        // 普通换行(包括 lazy continuation,即后续行没有 `> ` 前缀)是软
        // 换行,不能一律强制拆行——截图中的「普通文本都换行」正是这里
        // 的回归。保留源码字符和索引,只关闭 epaint 的强制换行,让它按
        // 可用宽自然折行;显式 `> ` 前缀仍在下方幽灵化。
        job.break_on_newline = false;
        // `> ` 前缀幽灵化,正文继续 LP2-4 内联成形。
        let mut line_start = 0usize;
        for line in text.split_inclusive('\n') {
            let line_end = line_start + line.len();
            let content_end = line.strip_suffix('\n').map_or(line.len(), str::len);
            let line_body = &line[..content_end];
            let prefix_len = line_body
                .find('>')
                .map(|at| at + 1 + usize::from(line_body.as_bytes().get(at + 1) == Some(&b' ')))
                .unwrap_or(0);
            if prefix_len > 0 {
                let mut ghost = egui::TextFormat::simple(
                    egui::FontId::new(font_size, body_family.clone()),
                    egui::Color32::from_rgba_unmultiplied(
                        text_color.r(),
                        text_color.g(),
                        text_color.b(),
                        110,
                    ),
                );
                ghost.line_height = Some(body_lh);
                job.sections
                    .push(section(line_start..line_start + prefix_len, ghost));
            }
            let body_start = line_start + prefix_len;
            let body_end = line_start + content_end;
            if body_start < body_end {
                let mut body = egui::TextFormat::simple(
                    egui::FontId::new(font_size, body_family.clone()),
                    text_color,
                );
                body.line_height = Some(body_lh);
                push_inline_styled_sections(&mut job, body_start..body_end, &body, &styles, &paint);
            }
            if content_end < line.len() {
                let mut newline = egui::TextFormat::simple(
                    egui::FontId::new(font_size, body_family.clone()),
                    text_color,
                );
                newline.line_height = Some(body_lh);
                job.sections
                    .push(section(line_start + content_end..line_end, newline));
            }
            line_start = line_end;
        }
        return job;
    }
    if is_heading {
        let marker_len = hashes + 1;
        let scale = style.heading.scales[(hashes.min(6) - 1).min(5)];
        let scaled = font_size * scale;
        let heading_lh = body_line_height(&style, scaled);
        // ATX 前缀:幽灵语法(Obsidian/MarkText 观感)—— 与标题**同字号**,
        // 文本色约 43% 透明度。weak 色正文小字太暗太小,用户看不见 #、
        // 以为前缀不可编辑(#159);升到标题字号后既是明显的可编辑语法,
        // 点击目标也够大。行高用标题行盒,行内不劈行。
        let ghost = egui::Color32::from_rgba_unmultiplied(
            text_color.r(),
            text_color.g(),
            text_color.b(),
            110,
        );
        let mut marker_format =
            egui::TextFormat::simple(egui::FontId::new(scaled, body_family.clone()), ghost);
        marker_format.line_height = Some(heading_lh);
        job.sections.push(section(0..marker_len, marker_format));
        // 标题文字:vendored 同式 —— bold 族(已注册时)+ 字号缩放 +
        // strong 色,行高按放大后字号重推导
        let mut heading_format = egui::TextFormat::simple(
            egui::FontId::new(scaled, body_family.clone()),
            ui.visuals().strong_text_color(),
        );
        if has_bold {
            heading_format.font_id.family = bold_family;
        }
        heading_format.line_height = Some(heading_lh);
        push_inline_styled_sections(
            &mut job,
            marker_len..first_line_end,
            &heading_format,
            &styles,
            &paint,
        );
        if first_line_end < text.len() {
            let mut rest =
                egui::TextFormat::simple(egui::FontId::new(font_size, body_family), text_color);
            rest.line_height = Some(body_lh);
            push_inline_styled_sections(
                &mut job,
                first_line_end..text.len(),
                &rest,
                &styles,
                &paint,
            );
        }
    } else if let Some(prefix) = list_prefix(first_line) {
        // 列表项(LP2-5「列表点击保持位置」,2026-10-20):marker 与内容按
        // 富渲染同式对位 —— marker 右对齐进 "99. " 槽位、内容落在
        // 槽位 + gap(+ 任务勾选框宽)列,公式逐项照抄 vendored layout.rs
        // 的 ListMarker 分支;前缀(缩进空白、marker、marker 后空白、
        // `[ ]`/`[x]` 任务标记)整体幽灵化(ATX 前缀同款 43% 弱化)。
        // 点击进列表项不再横向跳变:活动态内容列 == 富渲染内容列。折行
        // 行回到 x=0,与富渲染「wrapped rows return to column 0」同一
        // 口径;首行外的续行(懒延续)按正文无缩进,与现状一致。
        let ghost = egui::Color32::from_rgba_unmultiplied(
            text_color.r(),
            text_color.g(),
            text_color.b(),
            110,
        );
        let body_font = egui::FontId::new(font_size, body_family.clone());
        let mut ghost_format = egui::TextFormat::simple(body_font.clone(), ghost);
        ghost_format.line_height = Some(body_lh);
        let width_of = |text: &str| {
            ui.ctx().fonts_mut(|f| {
                text.chars()
                    .map(|c| f.glyph_width(&body_font, c))
                    .sum::<f32>()
            })
        };
        let space_w = width_of(" ");
        let slot_w = width_of("99. ");
        // vendored 的 marker 字串含尾随空格("• "/"N. ")且右对齐进槽位 ——
        // 活动块的 marker 段因此吃进 marker 后的第一个空格,宽度口径一致,
        // 内容才会落在同列;其余(marker 后多余空白、`[ ]` 任务标记)归
        // rest 段自然排布。
        let marker_span = if text[prefix.rest.clone()].starts_with(' ') {
            prefix.marker.start..prefix.rest.start + 1
        } else {
            prefix.marker.clone()
        };
        let marker_w = width_of(&text[marker_span.clone()]);
        let rest_str = &text[prefix.rest.clone()];
        let rest_span = marker_span.end..prefix.rest.end;
        let rest_w = width_of(&text[rest_span.clone()]);
        // 任务项:富渲染在内容列头部画 "☐ "(Token::TaskListMarker 基础
        // 格式),内容再多让出这一列宽
        let is_task = rest_str.trim_start_matches(' ').starts_with('[');
        let task_w = if is_task { width_of("☐ ") } else { 0.0 };
        // 嵌套缩进:源码前导空白按 2 空格一级折算(vendored indent_width
        // 同单位);奇数空格向下取整,与 pulldown 按层级记 indent 的口径
        // 在常见的 2 空格缩进下逐像素一致
        let nesting = (prefix.leading.len() / 2) as f32 * space_w * 2.0;
        let nudge = if prefix.is_ordered {
            style.list.number_nudge
        } else {
            style.list.bullet_nudge
        };
        // vendored 同式:marker(含尾随空格)右对齐进槽位,leading 下限 0 =
        // 宽 marker 不越出条目左缘
        let marker_x = nesting + (slot_w - nudge - marker_w).max(0.0);
        let mut cursor_x = prefix.leading.len() as f32 * space_w;
        let ghost_section =
            |byte_range: Range<usize>, leading_space: f32| egui::text::LayoutSection {
                leading_space,
                byte_range: byte_range.start.into()..byte_range.end.into(),
                format: ghost_format.clone(),
            };
        if !prefix.leading.is_empty() {
            job.sections
                .push(ghost_section(prefix.leading.clone(), 0.0));
        }
        let marker_lead = (marker_x - cursor_x).max(0.0);
        job.sections
            .push(ghost_section(marker_span.clone(), marker_lead));
        cursor_x = (cursor_x + marker_lead).max(marker_x) + marker_w;
        if !rest_span.is_empty() {
            job.sections.push(ghost_section(rest_span, 0.0));
            cursor_x += rest_w;
        }
        // 内容列校准:nesting + 槽位 + gap(+ 勾选框);已越过(宽 marker)
        // 则保持自然位置 —— vendored 的 trailing 空段同一语义
        let content_target = nesting + slot_w + style.list.gap + task_w;
        let calibrate = (content_target - cursor_x).max(0.0);
        if calibrate > 0.0 {
            job.sections.push(egui::text::LayoutSection {
                leading_space: calibrate,
                byte_range: prefix.rest.end.into()..prefix.rest.end.into(),
                format: ghost_format.clone(),
            });
        }
        let mut body_fmt = egui::TextFormat::simple(
            egui::FontId::new(font_size, body_family.clone()),
            text_color,
        );
        body_fmt.line_height = Some(body_lh);
        push_inline_styled_sections(
            &mut job,
            prefix.rest.end..first_line_end,
            &body_fmt,
            &styles,
            &paint,
        );
        if first_line_end < text.len() {
            let mut rest =
                egui::TextFormat::simple(egui::FontId::new(font_size, body_family), text_color);
            rest.line_height = Some(body_lh);
            push_inline_styled_sections(
                &mut job,
                first_line_end..text.len(),
                &rest,
                &styles,
                &paint,
            );
        }
    } else {
        // 普通正文的 Markdown 软换行在富渲染中是空格;活动态布局文本也
        // 用等长空格替换源码换行,这样按可用宽自动折行,但字符数量与源码
        // 完全一致,点击/光标/选区索引和 TextBuffer 回写不发生偏移。
        job.text = text.replace('\n', " ");
        job.break_on_newline = false;
        let mut body =
            egui::TextFormat::simple(egui::FontId::new(font_size, body_family), text_color);
        body.line_height = Some(body_lh);
        push_inline_styled_sections(&mut job, 0..text.len(), &body, &styles, &paint);
    }
    job
}

/// 活动块的类型徽标(#158):H1–H6 / 代码 / 表格 / 引用 / 分隔线 / 图片,
/// 编辑时悬在块顶的块间空隙里(Notion 同款);普通段落不标。纯判定,
/// 与 [`block_kind`] 同一套前缀规则。
fn block_badge_label(block_text: &str) -> Option<&'static str> {
    let first = block_text.lines().next().unwrap_or("");
    let t = first.trim_start();
    let hashes = t.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hashes) && t.as_bytes().get(hashes) == Some(&b' ') {
        return Some(match hashes {
            1 => "H1",
            2 => "H2",
            3 => "H3",
            4 => "H4",
            5 => "H5",
            _ => "H6",
        });
    }
    if t.starts_with("```") || t.starts_with("~~~") {
        return Some("代码");
    }
    if t.starts_with('|') {
        return Some("表格");
    }
    if t.starts_with('>') {
        return Some("引用");
    }
    if t.starts_with("![") {
        return Some("图片");
    }
    if !t.is_empty() && t.bytes().all(|b| matches!(b, b'-' | b'*' | b'_' | b' ')) {
        return Some("分隔线");
    }
    None
}

/// vendored 命中表(`char_index_at_pos`)返回的是**排版 job 文本**的字符
/// 序号,不是块源码的 —— 富渲染会把 `# ` 前缀剥进样式、把 `- ` 换成 `• `、
/// 把 `[ ] ` 换成 `☐ `,job 文本与源码逐字符错位(2026-10-21 用户实测:H1
/// 点到 M 后面、光标落在 e,正好差 `# ` 两字符)。本表把 job 字符序号映
/// 射回 rendered 字符序号,再走既有改写层逆穿回源码。
///
/// 只对**单段块**成立(代码块/表格/引用/图片/分隔线是独立 galley 分段,
/// 命中序号按段内计数,全块累计表对不上)——多段块 `build` 返回 `None`,
/// 调用方保持旧口径(恒等),与修复前一致,不劣化。
struct JobRenderMap {
    /// 按 job_start 升序的伸展表。ren = ren_start + min(rel, ren_cap_len):
    /// Text 伸展 cap == job_len(逐字符 1:1);marker/任务伸展 cap 取源码
    /// 段长(点在渲染 marker 上 → 落到源码 marker 区间内);链接文字伸展
    /// cap = 文字长(点在目标/括号上 → 吸到文字尾)。
    stretches: Vec<JobStretch>,
}

struct JobStretch {
    job_start: usize,
    ren_start: usize,
    ren_cap_len: usize,
}

impl JobRenderMap {
    /// 从 rendered 的 vendored 解析建表。返回 `None` = 多段块,调用方保持
    /// 旧口径。
    fn build(rendered: &str) -> Option<Self> {
        use egui_markdown::{Markdown, Token};
        let md: Markdown<'_> = egui_markdown::parser::parse(rendered);
        // 多段块:命中序号按段内计数,恒等回退
        if md.tokens.iter().any(|token| {
            matches!(
                token,
                Token::CodeBlock { .. }
                    | Token::Table(_)
                    | Token::Image { .. }
                    | Token::HorizontalRule
                    | Token::BlockquoteStart
                    | Token::BlockquoteEnd
            )
        }) {
            return None;
        }
        let char_at = |byte: usize| rendered[..byte.min(rendered.len())].chars().count();
        let mut stretches = Vec::new();
        let mut job = 0usize;
        for (token, span) in md.tokens.iter().zip(&md.spans) {
            let span_start = char_at(span.start);
            let span_end = span.end.min(rendered.len());
            let span_chars = rendered[span.start..span_end].chars().count();
            // 追加一个伸展:job 前进 job_len,rendered 侧覆盖
            // [ren_start, ren_start + ren_cap_len)
            let mut push =
                |job: &mut usize, ren_start: usize, job_len: usize, ren_cap_len: usize| {
                    stretches.push(JobStretch {
                        job_start: *job,
                        ren_start,
                        ren_cap_len,
                    });
                    *job += job_len;
                };
            match token {
                // 正文/行内代码/软换行空格:append 的就是 span 尾部这段,
                // 逐字符 1:1(标题 `# `、列表缩进等被吸收进 span 头部)
                Token::Text { text, .. } => {
                    let len = text.chars().count();
                    push(&mut job, span_start + span_chars - len, len, len);
                }
                Token::Newline => push(&mut job, span_start + span_chars - 1, 1, 1),
                // `- ` → "• "、`12. ` → `12. `:源码 marker 恰在 span 尾部
                // 且字符数与渲染 marker 相同
                Token::ListMarker { marker, .. } => {
                    let len = marker.chars().count();
                    push(&mut job, span_start + span_chars - len, len, len);
                }
                // `[ ] ` → "☐ ":渲染 2 字符、源码段 4 字符,点渲染勾选框
                // 落进源码 `[ ]` 区间
                Token::TaskListMarker { .. } => push(&mut job, span_start, 2, span_chars),
                // `[文字](目标)`:文字在首个 `[` 之后,1:1;点在括号/目标上
                // 吸到文字尾。autolink `<url>` 同构(`<` 即开括号)
                Token::Link { text, .. } => {
                    let len = text.chars().count();
                    let slice = &rendered[span.start..span_end];
                    let open = slice
                        .find('[')
                        .map(|i| span_start + slice[..i].chars().count());
                    let (ren_start, ren_cap_len) = match open {
                        // 文字须真落在 span 内(防御:开放式残构退化为整段吸)
                        Some(open) if open + 1 + len <= span_start + span_chars => (open + 1, len),
                        _ => (span_start, span_chars),
                    };
                    push(&mut job, ren_start, len, ren_cap_len);
                }
                Token::FootnoteRef { label } | Token::FootnoteDef { label } => {
                    let len = label.chars().count();
                    push(&mut job, span_start + 1, len, len);
                }
                // 不产 job 字符的 token(分段类已在门里回退,不会走到)
                _ => {}
            }
        }
        (!stretches.is_empty()).then_some(Self { stretches })
    }

    /// job 字符序号 → rendered 字符序号。越界钳到表端。
    fn lookup(&self, index: usize) -> usize {
        let last = self
            .stretches
            .partition_point(|s| s.job_start <= index)
            .saturating_sub(1);
        let s = &self.stretches[last];
        let rel = (index - s.job_start).min(s.ren_cap_len);
        s.ren_start + rel
    }
}

/// 徽标菜单的标题级别切换(LP2-7):`target = Some(n)` 把块变成 `#{n}` 标
/// 题,`None` 变回正文 —— 从单击循环(LP2-6,用户反馈要菜单不要循环)泛
/// 化成「指哪打哪」。只处理**顶格**前缀:标题改写 `#{N} `,普通段落插入
/// `# `;列表/引用/代码等带结构前缀的块不动。改写走 `replace_range`
/// (与格式动作/AI 润色同一通道,TextEdit 内建 undo 单步可退);光标按
/// 前缀字符差平移并经 `pending_caret` 落回,不拽视口。目标与现状相同时
/// 幂等不动。
fn set_block_heading_level(
    editor: &mut EditorBuffer,
    live: &mut LiveState,
    index: usize,
    target: Option<u8>,
    caret_local: usize,
) {
    let Some(range) = live.blocks.get(index).cloned() else {
        return;
    };
    let block = BlockBuffer::slice(editor.text(), &range).to_owned();
    let first_line_end = block.find('\n').unwrap_or(block.len());
    let first_line = &block[..first_line_end];
    if !first_line.starts_with(first_line.trim_start()) {
        return; // 缩进行保守不处理
    }
    let hashes = first_line.bytes().take_while(|&b| b == b'#').count();
    let is_heading = (1..=6).contains(&hashes) && first_line.as_bytes().get(hashes) == Some(&b' ');
    let (new_prefix, old_prefix_len) = match (is_heading, target) {
        (true, Some(n)) if n as usize != hashes => {
            (format!("{} ", "#".repeat(n as usize)), hashes + 1)
        }
        (true, None) => (String::new(), hashes + 1),
        (false, Some(n)) => {
            // 普通段落才可升级:列表项(有自己的徽标语义)、引用/代码等
            // 结构块一律不动
            if block_kind(&block) != BlockKind::Text || list_prefix(first_line).is_some() {
                return;
            }
            (format!("{} ", "#".repeat(n as usize)), 0)
        }
        // 已是正文 / 目标级别与现状相同:幂等不动
        (false, None) | (true, Some(_)) => return,
    };
    let delta_chars = new_prefix.chars().count() as isize - old_prefix_len as isize;
    // 前缀是纯 ASCII(`#` 与空格),字符数 == 字节数
    editor.replace_range(range.start..range.start + old_prefix_len, &new_prefix);
    // 光标平移:前缀差按字符计,钳进新块文本(旧块长 + 差值)
    let new_len = (block.chars().count() as isize + delta_chars).max(0) as usize;
    let new_caret = ((caret_local as isize + delta_chars).max(0) as usize).min(new_len);
    live.pending_caret = Some((index, new_caret));
    live.pending_range = None;
    live.caret_follow = false;
}

// ---------------------------------------------------------------- 块级改写(2026-10-23)
//
// 关闭代码块 / 表格加行列 / 删表:与 [`set_block_heading_level`] 同一通道
// (直接 `replace_range`,TextEdit 内建 undo 单步可退;光标经 pending_caret
// 落回)。入口两条:头部/工具条点击 → 简单动作就地执行,破坏性动作(关
// 代码块、删表)先挂 `pending_block_action`,帧末弹确认,确认帧才改写。

/// CommonMark 围栏允许的前导缩进上限。
const FENCE_MAX_INDENT: usize = 3;

/// 剥掉围栏允许的前导缩进(≤[`FENCE_MAX_INDENT`] 个空格);更多缩进时
/// 原样返回 —— 后续的围栏判定自然失败(CommonMark:4 空格起是缩进代码)。
fn strip_fence_indent(line: &str) -> &str {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent <= FENCE_MAX_INDENT {
        &line[indent..]
    } else {
        line
    }
}

/// 围栏开栏行的标记:行首(≤3 空格缩进)连续 ≥3 个 `` ` `` 或 `~`,返回
/// (围栏字符, 长度)。info string 跟在后面不算。
fn fence_open(line: &str) -> Option<(char, usize)> {
    let trimmed = strip_fence_indent(line);
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let run = trimmed.chars().take_while(|&c| c == marker).count();
    (run >= 3).then_some((marker, run))
}

/// 闭合围栏行:与开栏同字符、长度不短于开栏,且**标记之外整行空白**
/// (CommonMark 对闭合围栏的硬性要求 —— 内容行里嵌的 ```` ``` ```` 不算)。
fn is_fence_close(line: &str, marker: char, open_len: usize) -> bool {
    let trimmed = strip_fence_indent(line);
    let run = trimmed.chars().take_while(|&c| c == marker).count();
    run >= open_len && trimmed[run..].trim().is_empty()
}

/// 可关闭的围栏块:有开栏行且**存在对应的闭合行**。独立空围栏不会被
/// 解析器当成 CodeBlock token(实测并入邻块),这个判定挡的是「开栏 +
/// 正文粘连」的合并块 —— 头部不挂关闭按钮,确认弹窗不会空转。
fn closeable_fence(block: &str) -> bool {
    let Some((marker, open_len)) = fence_open(block.lines().next().unwrap_or("")) else {
        return false;
    };
    block
        .lines()
        .skip(1)
        .any(|l| is_fence_close(l, marker, open_len))
}

/// 关闭代码块(用户需求 2026-10-23):去掉首尾围栏行,内容保留为普通
/// 文本段落。内容为空(只有 info 行的空围栏)时整块删除 —— 留下一段
/// 空白块没有意义。块尾的分隔空行(块区间吞进的 `\n\n`,见
/// `latermd_md::blocks` 规定 ②)原样保留,改写后与下一块的间距不变;
/// 整块删除时区间整体替换为空串,前一块自带的分隔正好凑成单空行。
/// 光标落块首,不拽视口。
fn close_code_block(editor: &mut EditorBuffer, live: &mut LiveState, index: usize) {
    let Some(range) = live.blocks.get(index).cloned() else {
        return;
    };
    let block = BlockBuffer::slice(editor.text(), &range).to_owned();
    let Some((marker, open_len)) = fence_open(block.lines().next().unwrap_or("")) else {
        return; // 缩进代码块/非围栏:没有围栏可去
    };
    let mut lines = block.split('\n').collect::<Vec<_>>();
    // 块尾吞进的分隔空行退回:内容行只数到最后一根非空行
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    // 末行是闭合围栏才剥;流式未闭合(AI 写入中)整段都是内容,不动
    if lines
        .last()
        .is_some_and(|l| is_fence_close(l, marker, open_len))
    {
        lines.pop();
    } else {
        return;
    }
    lines.remove(0); // 开栏行(含 info)
    let trailing = &block[block.trim_end_matches('\n').len()..];
    let replacement = if lines.is_empty() || lines.iter().all(|l| l.trim().is_empty()) {
        // 空块:整块删除。区间含块尾分隔,替换为空串后前一块自带的
        // 分隔正好是唯一的空行
        String::new()
    } else {
        format!("{}{}", lines.join("\n"), trailing)
    };
    // 块表是字节区间,`replace_range` 吃字符区间 —— 换算后替换
    let chars = editor.byte_to_char(range.start)..editor.byte_to_char(range.end);
    editor.replace_range(chars, &replacement);
    live.pending_caret = Some((index, 0));
    live.pending_range = None;
    live.caret_follow = false;
}

/// 表格的行列规格:首行表头(管道分隔),次行分隔行(至少一根 `---`),
/// 其后为数据行。列数取表头;行格式要求首尾都有 `|`(本编辑器与常见
/// 教程的产出形态),不合规格返回 None(工具条点了也不动)。
fn table_shape(block: &str) -> Option<(Vec<&str>, usize)> {
    let content = block.trim_end_matches('\n');
    let mut lines = content.lines().filter(|l| !l.trim().is_empty());
    let header = lines.next()?;
    if !header.trim_start().starts_with('|') {
        return None;
    }
    let cols = header.split('|').count().saturating_sub(2);
    if cols == 0 {
        return None;
    }
    Some((content.lines().collect(), cols))
}

/// 表格末尾加一行(空单元格,列数对齐表头)。落点在最后一根内容行之后
/// (块尾分隔空行之前),光标落块首;规格不齐(无表头/列数为 0)不动。
fn table_add_row(editor: &mut EditorBuffer, live: &mut LiveState, index: usize) {
    let Some(range) = live.blocks.get(index).cloned() else {
        return;
    };
    let block = BlockBuffer::slice(editor.text(), &range).to_owned();
    let Some((_, cols)) = table_shape(&block) else {
        return;
    };
    let content_end = block.trim_end_matches('\n').len();
    let cells = vec![""; cols].join(" | ");
    let row = format!("| {cells} |");
    let mut replacement = String::with_capacity(block.len() + row.len() + 1);
    replacement.push_str(&block[..content_end]);
    replacement.push('\n');
    replacement.push_str(&row);
    replacement.push_str(&block[content_end..]);
    // 块表是字节区间,`replace_range` 吃字符区间 —— 换算后替换
    let chars = editor.byte_to_char(range.start)..editor.byte_to_char(range.end);
    editor.replace_range(chars, &replacement);
    live.pending_caret = Some((index, 0));
    live.pending_range = None;
    live.caret_follow = false;
}

/// 表格右侧加一列:表头/数据行各在行尾补一个空单元格,分隔行补
/// ` --- `。逐行替换而非整块重排 —— 行内已有的对齐标记(`:-:` 等)
/// 原样保留。列数以表头为准;短于表头的行(残缺表格)补齐到表头列数,
/// 不动超长行。光标落块首。
fn table_add_col(editor: &mut EditorBuffer, live: &mut LiveState, index: usize) {
    let Some(range) = live.blocks.get(index).cloned() else {
        return;
    };
    let block = BlockBuffer::slice(editor.text(), &range).to_owned();
    let Some((lines, cols)) = table_shape(&block) else {
        return;
    };
    let content_end = block.trim_end_matches('\n').len();
    let trailing = &block[content_end..];
    let mut out = String::with_capacity(block.len() + lines.len() * 5);
    for (i, line) in lines.iter().enumerate() {
        let body = line.trim_end_matches('\r');
        out.push_str(body);
        // 行尾带 `|` 的形态追加「新单元格 + 收尾管」;不带尾管的形态
        // (CommonMark 允许)只补单元格本身,不凭空引入管。
        let closed = body.trim_end().ends_with('|');
        let is_separator = i == 1 && body.trim_start().trim_matches('|').contains('-');
        if is_separator {
            if closed {
                out.push_str(" --- |");
            } else {
                out.push_str(" ---");
            }
        } else {
            if i > 1 {
                // 数据行:短行先补齐到表头列数(残缺行缺的尾巴一并补上)
                let cells = body.split('|').count().saturating_sub(2);
                for _ in cells..cols {
                    if closed {
                        out.push_str(" |");
                    } else {
                        out.push(' ');
                    }
                }
            }
            if closed {
                out.push_str("  |");
            } else {
                out.push_str("  ");
            }
        }
        out.push('\n');
    }
    // 上面逐行补了 '\n',块尾分隔不该再翻倍:剥掉最后一个 '\n' 再接 trailing
    out.pop();
    out.push_str(trailing);
    // 块表是字节区间,`replace_range` 吃字符区间 —— 换算后替换
    let chars = editor.byte_to_char(range.start)..editor.byte_to_char(range.end);
    editor.replace_range(chars, &out);
    live.pending_caret = Some((index, 0));
    live.pending_range = None;
    live.caret_follow = false;
}

/// 删除整个块(删表用):区间整体替换为空串。块区间含块尾分隔空行,
/// 删除后前一块自带的分隔正好是唯一的空行,间距不变。
fn delete_block(editor: &mut EditorBuffer, live: &mut LiveState, index: usize) {
    let Some(range) = live.blocks.get(index).cloned() else {
        return;
    };
    // 块表是字节区间,`replace_range` 吃字符区间 —— 换算后替换
    let chars = editor.byte_to_char(range.start)..editor.byte_to_char(range.end);
    editor.replace_range(chars, "");
    live.pending_caret = Some((index.max(1) - 1, 0));
    live.pending_range = None;
    live.caret_follow = false;
}

/// 列表项首行的前缀结构(LP2-5「列表点击保持位置」):前导空白、
/// marker(`-`/`*`/`+`/`N.`/`N)`,不含尾随空白)、marker 后的空白与可选
/// 任务标记(`[ ]`/`[x]` + 空白)。三段区间首尾相接 = `0..rest.end`,即
/// 列表项「内容」的起点。与 [`block_kind`]/[`block_badge_label`] 同一套
/// 保守前缀嗅探口径:marker 后必须有空白(或行尾),`*强调*` 这类行内
/// 构造不会被误判成列表。
struct ListPrefix {
    leading: Range<usize>,
    marker: Range<usize>,
    rest: Range<usize>,
    is_ordered: bool,
}

/// 解析列表项首行前缀;不是列表项返回 `None`。有序 marker 最多 9 位数字
/// (CommonMark 上限),前导空白只认空格(tab 视作非空白,保守不当列表)。
fn list_prefix(first_line: &str) -> Option<ListPrefix> {
    let bytes = first_line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    let leading = 0..i;
    let marker_start = i;
    if matches!(bytes.get(i), Some(b'-' | b'*' | b'+')) {
        i += 1;
    } else {
        let digits = bytes[i..]
            .iter()
            .take_while(|&&b| b.is_ascii_digit())
            .count();
        if (1..=9).contains(&digits) && matches!(bytes.get(i + digits), Some(b'.' | b')')) {
            i += digits + 1;
        } else {
            return None;
        }
    }
    let marker = marker_start..i;
    let is_ordered = first_line[marker.clone()].starts_with(|c: char| c.is_ascii_digit());
    // marker 后必须是空白(或行尾:空列表项 `-` 单独成行也是列表)
    if bytes.get(i).is_some_and(|&b| b != b' ') {
        return None;
    }
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    // 任务列表标记:`[ ]`/`[x]`/`[X]` + 空白,贴在内容起点才算
    if bytes.get(i) == Some(&b'[')
        && matches!(bytes.get(i + 1), Some(b' ' | b'x' | b'X'))
        && bytes.get(i + 2) == Some(&b']')
        && bytes.get(i + 3).is_some_and(|&b| b == b' ')
    {
        i += 3;
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
    }
    let rest = marker.end..i;
    Some(ListPrefix {
        leading,
        marker,
        rest,
        is_ordered,
    })
}

/// 预测活动块源码 TextEdit 的布局几何：(外框高, 光标行顶相对块顶的 y)。
///
/// 与 egui TextEdit multiline 的 default_layouter 完全同参 shaping ——
/// Monospace 字号、行盒高与活动块同源（[`body_line_height`]，见活动块
/// 分支的 `extra_text_line_spacing` 拨动）、可用宽（水平内距已归零,
/// LP2-5「活动块与富渲染同 x」）的折行宽、`keep_trailing_whitespace`；
/// 外框高 = galley 高（内距已全零,LP2-6），光标行顶 = galley 行顶。
/// **text 须与编辑视图同文**（裁掉幻影空行的前缀，见
/// `BlockBuffer::visible_bytes`）。用于点击帧预判「块切源码后被点击行
/// 落到哪里」，让 ScrollArea 偏移在内容重排**之前**修正（重排发生在下
/// 一帧，偏移先行到位则重排帧不跳）；预测残差由落地帧按实测光标行做
/// 一次校正（`LiveState::toggle_anchor`）。
fn predict_source_layout(ui: &egui::Ui, text: &str, caret_char: usize) -> (f32, f32) {
    let wrap_width = ui.available_width().max(24.0);
    // 与编辑视图同一个排版作业(#158):预测即真相 —— 标题缩放/正文字体/
    // 行盒高全部同源,视口锚定的预测精度不随排版改变而回退。
    // (作业构造必须在 fonts_mut 闭包外:它内部取 context 写锁,与
    // fonts_mut 的写锁嵌套即自死锁 —— 49ef922 记过的同类陷阱。)
    let job = active_block_job(ui, text, wrap_width);
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let caret_row = galley.pos_from_cursor(egui::text::CCursor::new(caret_char));
    let floor = body_line_height(
        &egui_markdown_style::global_style(ui.ctx()),
        crate::theme::editor_font_size(ui.ctx()),
    );
    // 内距已全零(LP2-6):外框高 = galley 高,光标行顶 = galley 行顶。
    // 例外:围栏代码的活动态带代码框(#310「原样编辑」,与活动臂的 frame
    // 同常数)—— 高度加框,光标行顶加竖直内距。
    let fenced = fence_open(text.lines().next().unwrap_or("")).is_some();
    let (pad_v,) = (6.0_f32,);
    let extra_h = if fenced { 2.0 * pad_v } else { 0.0 };
    (
        galley.size().y.max(floor) + extra_h,
        caret_row.min.y + if fenced { pad_v } else { 0.0 },
    )
}

/// 块内「可见内容末尾」的块内字符偏移：块源码尾部的换行（含块间空行
/// —— 块表规定空行归前一块）是幻影行，方向键路由把它们视作块尾边界，
/// 光标到可见内容末尾即按一次方向键就路由到相邻块，不必先穿过 1–2 个
/// 看不见的空行（写作模式键盘「发粘」的主因）。
fn visible_char_len(block_text: &str) -> usize {
    block_text.trim_end_matches('\n').chars().count()
}

/// 滚动探针的 data 键(P3,按编辑器 id 分标签 —— 两个渲染实例各一份)。
/// `pub(crate)`:布局层测试读右栏对照列的探针断言滚轮锚点落地。
pub(crate) fn scroll_probe_id(editor_id: egui::Id) -> egui::Id {
    egui::Id::new("latermd-live-scroll").with(editor_id)
}

/// 滚动探针载荷(仅测试读):帧末滚动偏移 + 视口矩形 + 内容高度。
/// 生产只写不读,字段读取都活在 `cfg(test)`,非测试构建豁免 dead_code。
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct LiveScrollProbe {
    pub(crate) offset: f32,
    pub(crate) viewport: egui::Rect,
    pub(crate) content_height: f32,
}

/// 绘制 Live Preview:非活动块富渲染(点击即进入编辑),活动块源码编辑。
///
/// `selection` 是格式工具条的选区镜像出参(与源码模式 `ui::editor` 同一
/// 契约,见 `TabState::selection`):Live 的选区只存在于活动块 TextEdit 的
/// 持久 state,每帧抄出(文档坐标字符区间);无活动块 = 无选区,置 `None`。
///
/// 返回活动块 TextEdit 的响应(没有活动块时返回一块占位区域,便于测试定位)。
///
/// **同一文档可以被两个实例同时渲染**(写作列 + 只读对照列),前提是二者
/// 各自持有 `LiveState` 与 `editor_id` —— 前者避免共享可写状态,后者让
/// vendored 的 per-label 缓存与逐帧命中表落在互不干扰的 id 命名空间里。
/// 本函数内部已按 `editor_id` 派生全部派生 id(滚动、`MarkdownLabel`、
/// 活动块 TextEdit),调用方无需再手工加盐。
#[allow(clippy::too_many_arguments)]
pub fn ui(
    panel: &mut egui::Ui,
    editor: &mut EditorBuffer,
    preview: &mut PreviewState,
    ai: &AiState,
    cursor: &mut OutlineCursor,
    selection: &mut Option<(usize, usize)>,
    live: &mut LiveState,
    editor_id: egui::Id,
    show_typewriter: bool,
    show_focus: bool,
    // 选区 AI 浮标开关(#61 配置面):false 时浮标不弹(anchor 恒 None,
    // show 走隐藏路径清探针),右键菜单动作不受影响。
    show_selection_ai: bool,
    base_dir: Option<&std::path::Path>,
    heal: bool,
    badge_gutter: bool,
    keymap: &crate::keymap::Keymap,
    outbox: &mut Vec<Message>,
) -> egui::Response {
    live.sync(editor, cursor.byte);

    // 归约侧的大纲跳转、格式动作和测试/恢复路径可以预先指定活动块；
    // 这类明确的程序动作应直接进入编辑态。普通打开文档时活动块为空，
    // 仍保持浏览态，等待左键单击进编辑。
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
    crate::ui::rich_widgets::stash_focus_on_press(panel.ctx());

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
    //
    // D2(live-unify-plan P3):**锁定浏览**(禅定强制浏览)的跳转**只滚动**
    // —— 不切活动块、不写光标、不抢焦点;目标块画完后由富渲染臂的落地
    // 分支把块的渲染矩形滚进视口。写作浏览态(未锁定时)维持既有语义:
    // 跳转即落点与进编辑(#60 goto / #42 大纲的老契约不动)。
    if let Some(char_idx) = cursor.jump_to.take() {
        let byte = editor.char_to_byte(char_idx);
        if let Some(block) = live.block_containing(byte) {
            // 跳转覆盖 pending_caret,点击锚定一并作废(它只对「点击块 →
            // 落地」这一对帧有意义)。
            live.toggle_anchor = None;
            if live.browse_locked {
                live.pending_caret = Some((block, 0));
                live.pending_range = None;
                live.caret_follow = true;
            } else {
                let local =
                    editor.byte_to_char(byte) - editor.byte_to_char(live.blocks[block].start);
                live.active = Some(block);
                // 非锁定场景保留原语义:跳转即进编辑(#60 goto / #42 大纲)。
                live.browse_only = false;
                live.pending_caret = Some((block, local));
                live.pending_range = None;
                live.caret_follow = true;
                live.drag_anchor = None;
            }
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
    // 块切换视口锚定(点击帧产出,帧末 `activate` 消费点落账):shift =
    // 要写进 ScrollArea 偏移的补偿量;anchor = 交给落地帧做残差校正的
    // (目标块, 点击行屏幕顶)。定义见富渲染臂 `pending_shift` 产出点与
    // `LiveState::toggle_anchor`。
    let mut pending_shift: Option<f32> = None;
    let mut pending_anchor: Option<(usize, f32)> = None;
    // 选区 AI 浮标(#61 M1)的锚点素材:活动块持久选区(**文档坐标**字符
    // 区对,块内捕获时换算)与尾端光标条屏幕矩形,活动块分支内捕获、
    // ScrollArea 之后消费(命中区同层末尾注册,与源码模式同一纪律)。
    let mut sel_ai_selection: Option<(usize, usize)> = None;
    let mut sel_ai_anchor: Option<egui::Rect> = None;
    let mut sel_ai_response_id = egui::Id::NULL;
    let block_count = live.blocks.len();
    // emoji 探针(P1,与预览同一收尾纪律):全块共享一份收集器,块循环内
    // 逐 handler 记占位区,ScrollArea 之后统一消费。
    let emoji_rects: std::rc::Rc<std::cell::RefCell<Vec<egui::Rect>>> = Default::default();

    // ScrollArea 状态键:ScrollArea 在 begin() 里以 `ui.make_persistent_id(
    // IdSalt::new(id_salt))` 派生 —— **必须经 IdSalt 包一层**,裸 `Id` 直接进
    // `make_persistent_id` 哈希的是 Id 本体,与 IdSalt 的哈希不同源,得到的
    // 是另一个键(2026-10-19 实测踩过:State::load 恒 MISS,#111 B 的
    // estimate_drift 落账因此一直静默无效)。且 make_persistent_id 随 **Ui
    // 链**变:LP2-6 把 ScrollArea 挪进了徽标槽子 Ui,状态键必须同样从子
    // Ui 派生 —— 从 panel 派生会写到旧键上,锚定补偿/残差校正全部静默
    // 失效(2026-10-21 实测:点击锚定测试 offset 纹丝不动)。旧键下的持久
    // 滚动位置随本次换键作废一次,属可接受迁移成本。
    // 徽标槽(LP2-6):写作栏内容整体右移 [`BADGE_GUTTER`],左缘空槽给类型
    // 徽标(H1–H6/代码/引用…与块首行对齐,点击可切换标题级别)。禅定/对照
    // 列传 false,gutter=0 时子 Ui 矩形与面板重合;块 id 全部显式派生,不
    // 受父子链变化影响。
    let gutter = if badge_gutter { BADGE_GUTTER } else { 0.0 };
    let area_rect = panel.available_rect_before_wrap();
    let mut content = panel.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_max(
                egui::pos2(area_rect.left() + gutter, area_rect.top()),
                area_rect.max,
            ))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let scroll_state_id =
        content.make_persistent_id(egui::IdSalt::new(editor_id.with("live-scroll")));
    let scrolled =
        egui::ScrollArea::vertical()
            // id 必须带 `editor_id`:同一文档可能被两个实例渲染(主写作列 +
            // 只读对照列),写死常量会让两列的滚动状态缠在一起。
            .id_salt(editor_id.with("live-scroll"))
            .auto_shrink([false, false])
            .show(&mut content, |ui| {
                // 块间节奏完全由 [`block_gap`] 前距决定(与右栏预览同口径,
                // 2026-10-10):egui 出厂 item_spacing.y = 3px 会叠加在演算值
                // 上,归零让前距成为唯一间距源。
                ui.spacing_mut().item_spacing.y = 0.0;
                // #111 B 的滚动锚定落点:视口上方块的占位高变化会平移下方全
                // 部内容,`estimate_drift` 累计本帧全部此类差值,在 ScrollArea
                // 收尾后把 state.offset 同向修正回去(egui 0.36 的 State 是
                // persisted+可写,`store` 先于本帧 clamp,修改即生效)。视口
                // 下方的占位差只改内容总高(滚动条长度),不影响可见几何,
                // 不补偿。
                if block_count == 0 {
                    if live.browse_only {
                        // 空文档的浏览态没有可渲染块，只显示非编辑提示。单击
                        // 提示后下一帧才创建 TextEdit，避免打开新文档就抢焦点。
                        // 单击同样过 browse_enabled 闸(P3):禅定强制浏览时
                        // 单击提示不得进写作 —— 与富块的单击入口同一判定。
                        let response = ui.add(
                            egui::Label::new(
                                egui::RichText::new("单击此处开始写作")
                                    .color(ui.visuals().weak_text_color()),
                            )
                            .sense(egui::Sense::click()),
                        );
                        if response.clicked() && live.browse_enabled {
                            live.browse_only = false;
                        }
                    } else {
                        // 写作态使用共享缓冲，不另建镜像。
                        let mut buffer = BlockBuffer::full(editor, 0..0);
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
                        // 新建直落写作(默认模式跟随):消费一次的焦点请求,
                        // 首帧把焦点交给空文档块;点击进写作不走这里。
                        if ui.ctx().data(|d| {
                            d.get_temp::<bool>(editor_id.with("live-focus-new"))
                                .unwrap_or(false)
                        }) {
                            ui.memory_mut(|mem| mem.request_focus(response_id));
                            ui.ctx()
                                .data_mut(|d| d.remove::<bool>(editor_id.with("live-focus-new")));
                        }
                        // 右键菜单:空文档块与活动块同一挂点(条目表共用)。
                        // 排在 active_response 之前 —— 那条会移走 response。
                        crate::ui::editor_menu::attach(ui, &output, keymap, true, outbox);
                        active_response = Some(output.response.response);
                    }
                }
                // 前一块的分类(首块无前距:vendored `had_content` 同语义,
                // 文档顶贴顶不悬空)。
                let mut prev_kind: Option<BlockKind> = None;
                // 视口外块估高剔除(#111 B):本帧被剔除块的「估计高 − 实测高」
                // 累计,ScrollArea 收尾后按顶部/底部位置补偿滚动偏移。见
                // [`estimate_drift`] 的逐条说明。
                let mut estimate_drift: f32 = 0.0;
                // 块间距的两个输入**一帧内是常量**:全局 MarkdownStyle 与编辑
                // 器字号。原先每块各读一次(`global_style` 还要 clone 槽位里的
                // Arc)—— 634KB 文档 6000+ 块就是上万次 `ctx.data` 读 + Id
                // hash,而屏外块走的是估高臂,那点距离根本没人看。提到循环
                // 外一次读齐后,`typing_finding_report` 的 Live 档(634KB)
                // 稳态帧 3.85ms → 3.59ms、击键帧 10.72ms → 10.50ms。
                let gap_style = egui_markdown_style::global_style(ui.ctx());
                let gap_font_size = crate::theme::editor_font_size(ui.ctx());
                for index in 0..block_count {
                    let range = live.blocks[index].clone();
                    // 块文本直接借用共享缓冲；渲染缓存持有改写结果，稳态帧不再复制整块。
                    let block_text = BlockBuffer::slice(editor.text(), &range);
                    let kind = block_kind(block_text);
                    if let Some(prev) = prev_kind {
                        ui.add_space(block_gap(&gap_style, gap_font_size, prev, kind));
                    }
                    prev_kind = Some(kind);
                    // 视口外块估高剔除(#111 B,decisions-pending 备选 B 的落地):
                    // 屏外块不 shape,用 vendored 估高器(`estimate.rs`,结构化
                    // 遍历 + 逐字宽度求和,缓存于 id+文本 hash)占位。命中条件
                    // 与 vendored flush 段剔除同向(起于裁剪底缘之外才剔),
                    // 故滚动中被裁到的块**当帧即真实渲染**,视口内几何永远精
                    // 确,滚动无跳变;被替换的估高与实测的差值按块在视口上/
                    // 下方分别补偿(`estimate_drift`),滚动锚定不因占位失准
                    // 而漂移。首帧估高误差造成的滚动条长度失真由后续帧的
                    // 剔除-渲染交替逐步消化(每帧最多一个方向失真,方向稳定)。
                    // 富渲染配置逐块镜像(与下方真实渲染臂完全一致,估高器按
                    // 这些配置计算,含 mermaid 块高与滚动代码框头)。
                    let clip = ui.clip_rect();
                    let cursor_y = ui.cursor().top();
                    let is_active = !live.browse_only && live.active == Some(index);
                    let below_fold = cursor_y > clip.max.y;
                    // 上方剔除:块底 = cursor_y + 高(本帧估高或上一帧实测,任一
                    // 近似)。ui.cursor().bottom() 在无限高内容 Ui 下恒 inf,不可
                    // 用 —— egui 0.36 的 content Ui 是 max_rect 无界的。
                    let known_h = ui
                        .ctx()
                        .data(|d| d.get_temp::<f32>(editor_id.with(("live-render-h", index))))
                        .unwrap_or(0.0);
                    // 上缘判定带 1px 容差:ScrollArea 的内容 cursor 与 clip
                    // 缘之间存在亚像素舍入差(实测 0.1px),首块恰贴视口顶时
                    // `cursor_y + known_h < clip.min.y` 会被这点误差判真 ——
                    // 首块从此永远走占位臂不渲染(known_h 恒 0,误判自我
                    // 维持),写作/对照列顶部整块空白(2026-10-16 坤哥截图,
                    // 写作与分栏两档同根因)。方向取保守:差这 1px 本来也
                    // 看不见字形,宁可多画不可空块。
                    const ABOVE_FOLD_SLOP: f32 = 1.0;
                    let above_fold = cursor_y + known_h < clip.min.y - ABOVE_FOLD_SLOP;
                    if !is_active && (below_fold || above_fold) {
                        let dark_mode = matches!(ui.ctx().theme(), egui::Theme::Dark);
                        let rich_handler = RichLinkHandler::new(
                            TaskProbeDomain::Live(range.start),
                            dark_mode,
                            ui.visuals().text_color(),
                            std::rc::Rc::clone(&emoji_rects),
                        )
                        .with_ai(crate::ui::rich_widgets::ai_link_color(dark_mode), ai);
                        let (block_rendered, ..) =
                            live.rich_block_with_base(index, block_text, base_dir);
                        // heal 与真实臂同开关:流式帧的残缺 fence 在估高里也
                        // 补闭合,高度与真实渲染同源。
                        let estimate = egui_markdown::MarkdownLabel::new(
                            editor_id.with(("live-render", index)),
                            block_rendered.as_ref(),
                        )
                        .font(egui::FontId::new(
                            crate::theme::editor_font_size(ui.ctx()),
                            crate::fonts::preview_body_family(ui.ctx()),
                        ))
                        .wrap()
                        .overflow_wrap(egui_markdown::OverflowWrap::BreakAll)
                        .scroll_code_blocks(true)
                        .shrink_code_blocks(true)
                        .code_block_min_width(Some(200.0))
                        .code_block_buttons(&crate::ui::rich_widgets::code_copy_buttons)
                        .heal(heal)
                        .defer_offscreen_highlight(true)
                        .link_handler(&rich_handler)
                        .estimated_height(ui);
                        // 表格 titlebar 的 24px 不在 vendored 估高器认知里,
                        // 手工补上 —— 占位与实测同源,滚动锚定不漂
                        // (闸门与真实渲染臂的 [`table_titlebar_h`] 一致)。
                        let estimate = estimate.max(1.0)
                            + table_titlebar_h(kind, block_text, live, index);
                        // TextEdit 内距已全零(LP2-6):真实渲染块的框高 ==
                        // galley 高,占位即估高,与实测的差值只剩估高误差本身。
                        let placeholder = estimate;
                        let rect = egui::Rect::from_min_size(
                            ui.cursor().min,
                            egui::vec2(ui.available_width(), placeholder),
                        );
                        ui.allocate_rect(rect, egui::Sense::hover());
                        // 差值记账:块在视口上方,占位高度变化直接平移下方所有
                        // 内容 → 等量补偿滚动偏移;在下方,只影响滚动条长度,
                        // 不补偿。估高与实测的差只在块被真实渲染的帧才可知,
                        // 所以这里记的是「估高 vs 上一帧实测」——块一直在屏外
                        // 时无实测可比,记 0(占位高度恒等于估高,滚动几何以
                        // 估高为准,自洽)。
                        let measured = ui
                            .ctx()
                            .data(|d| d.get_temp::<f32>(editor_id.with(("live-render-h", index))))
                            .unwrap_or(placeholder);
                        if above_fold {
                            estimate_drift += placeholder - measured;
                        }
                        // 锁定浏览的跳转滚动(与真实渲染臂同一落地):目标块在
                        // 屏外同样要滚到位 —— 占位矩形就是它此刻的几何,滚动
                        // 下一帧把它带进视口后即转真实渲染,落点随实测校正。
                        if live.browse_locked {
                            if let Some((block, _)) = live.pending_caret {
                                if block == index {
                                    live.pending_caret = None;
                                    live.caret_follow = false;
                                    locked_jump_landing(ui, live, rect);
                                }
                            }
                        }
                        continue;
                    }
                    if !live.browse_only && live.active == Some(index) {
                        // 可见内容末尾(幻影行之前的最后一个字符位置),跨块
                        // 路由的块尾判定边界;定义见 [`visible_char_len`]。
                        let visible_end = visible_char_len(block_text);
                        // 编辑视图 = 块内容去掉尾部的幻影空行(块间空行归前
                        // 一块,富渲染本就忽略它们;全显示会让块切源码凭空
                        // 多出 1-2 个空行 —— 「点击后块展开」的主要来源)。
                        // 两条延伸规则保住编辑语义:
                        // ① 光标所在行可见 —— 光标进了幻影区(块尾 Enter 后
                        //    的新行)时视图跟到光标行,否则光标落进不可见区;
                        // ② 块尾 Enter 的当帧预先多给一个换行 —— 插入的换行
                        //    必须当场可见,不然 egui 把新光标钳回内容末,新行
                        //    落进不可见区,接着敲的字把新段落吞回块尾。
                        let response_id = editor_id.with(("live-block", index));
                        let content_bytes = block_text.trim_end_matches('\n').len();
                        let full_chars = block_text.chars().count();
                        let caret_local_chars = cursor
                            .byte
                            .filter(|byte| range.contains(byte))
                            .map(|byte| editor.byte_to_char(byte) - editor.byte_to_char(range.start))
                            .unwrap_or(0)
                            .min(full_chars);
                        let content_chars = block_text[..content_bytes].chars().count();
                        let enter_at_tail = ui.input(|input| input.key_pressed(egui::Key::Enter))
                            && caret_local_chars >= content_chars;
                        let visible_chars = (content_chars.max(caret_local_chars)
                            + usize::from(enter_at_tail))
                        .min(full_chars);
                        let visible_bytes = block_text
                            .char_indices()
                            .nth(visible_chars)
                            .map_or(block_text.len(), |(byte, _)| byte)
                            .max(content_bytes);
                        let lines = block_text[..visible_bytes].lines().count().max(1);
                        // 类型徽标文案提前判定(&'static str,不留 block_text 借用)
                        let badge = block_badge_label(block_text);
                        // 徽标交互判定提前算(&'static str/枚举/拷贝,不留块
                        // 文本借用 —— buffer 要独占 editor,菜单闭包也要可变
                        // 借用 editor 做级别转换)
                        let kind = block_kind(block_text);
                        // 围栏代码活动态的代码框感判定提前(此处 block_text
                        // 借用尚未让位给 BlockBuffer 的 &mut editor)
                        let code_frame = closeable_fence(block_text);
                        let first_line = block_text.lines().next().unwrap_or("");
                        // 表格活动态:单元格原样编辑(#310 需求)—— 不塌成
                        // 整块源码 TextEdit,单元格各自成单行编辑器,改动经
                        // CellBuffer 直接落回共享缓冲。表头/数据行照表格形态,
                        // 分隔行自动维护不可编辑。徽标/内联标记淡显/打字机是
                        // 文本块特性,表格不走;此处 block_text 借用尚在,
                        // 行宽/单元格几何都在函数内现测。
                        if kind == BlockKind::Widget && table_rows(block_text).is_some() {
                            // 表格文本用 owned 副本:block_text 借着 editor,
                            // 单元格编辑要 &mut —— 借用让位在拷贝之后完成
                            let owned = block_text.to_owned();
                            let block_start = live.blocks[index].start;
                            if let Some(focused) = draw_active_table(
                                ui,
                                editor,
                                live,
                                index,
                                &owned,
                                block_start,
                                editor_id,
                                keymap,
                                outbox,
                            ) {
                                active_response = Some(focused.output.response.response.clone());
                                crate::ui::editor_menu::attach(ui, &focused.output, keymap, true, outbox);
                                // 光标回填(大纲/状态栏):聚焦单元格内光标的文档字节
                                cursor.byte = focused
                                    .output
                                    .state
                                    .cursor
                                    .char_range()
                                    .map(|r| editor.char_to_byte(focused.cell_start_char + r.primary.index.0));
                            }
                            continue;
                        }
                        let plain_cycle = badge.is_none()
                            && kind == BlockKind::Text
                            && list_prefix(first_line).is_none();
                        let text_family = kind == BlockKind::Heading || plain_cycle;
                        let current_level = if kind == BlockKind::Heading {
                            Some(
                                first_line
                                    .bytes()
                                    .take_while(|&b| b == b'#')
                                    .count()
                                    .min(6) as u8,
                            )
                        } else {
                            None
                        };
                        // 菜单首行的当前类型名:widget 用 #158 徽标词,列表
                        // 项「列表」,其余「正文」
                        let type_label = badge.unwrap_or_else(|| {
                            if list_prefix(first_line).is_some() {
                                "列表"
                            } else {
                                "正文"
                            }
                        });
                        let mut buffer = BlockBuffer {
                            buffer: editor,
                            range: range.clone(),
                            visible_bytes,
                        };
                        // 稳定 id:按块序号(不带长度/hash),AGENTS §6.7 的红线
                        // (response_id 已在视图计算处派生)
                        let focus_memory_id = editor_id.with(("live-ime-focus", index));
                        let focused_last_frame = ui
                            .ctx()
                            .data(|data| data.get_temp::<bool>(focus_memory_id).unwrap_or(false));
                        // 渲染形态原地编辑(#158)的排版闭包:先绑定再挂 ——
                        // `.layouter(&mut 闭包字面量)` 的临时值撑不过语句
                        let mut layouter = |ui: &egui::Ui,
                                            text: &dyn egui::TextBuffer,
                                            wrap_width: f32| {
                            // 作业构造必须在 fonts_mut 闭包外:内部取
                            // context 写锁,嵌套即自死锁(49ef922 同类)
                            let job = active_block_job(ui, text.as_str(), wrap_width);
                            ui.fonts_mut(|f| f.layout_job(job))
                        };
                        let edit = egui::TextEdit::multiline(&mut buffer)
                            .id(response_id)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .desired_rows(lines.clamp(1, 40))
                            // 与源码编辑器同样锁定焦点。活动块仍可由本模块的
                            // 单击路由显式切换，普通工具条、滚动和预览
                            // 控件不会让写作插入点被 egui 无意收回。
                            .lock_focus(true)
                            // 渲染形态原地编辑(#158):自定义排版作业 ——
                            // 文本与缓冲不变,光标/选区/IME 通路零改动;
                            // 标题行放大加粗、正文行 = 预览正文字体,
                            // Widget 块等宽回退。见 [`active_block_job`]。
                            .layouter(&mut layouter);
                        // 活动块无输入框式外框(2026-10-10 mac 精修全平台化,
                        // #169):内距全部归零 —— 水平归零是 LP2-5 的「活动块
                        // 与富渲染绝对同 x」;垂直归零是 LP2-6 的「点击进标题
                        // 不再下坠」:文档顶部滚动被钳位、锚定补偿修不掉,
                        // 原上下各 2px 让块切源码整体下坠 4px(用户实测)。
                        // 光标/选区/IME 通路不变。
                        // 例外是围栏代码(#310「原样编辑」):活动态保留代码框
                        // 感 —— 代码底 + 描边 + 圆角 + 内距,与富渲染的代码框
                        // 同源(围栏行已淡显,见 [`active_block_job`]);锚定的
                        // 高差由 frame 内距同步进 [`predict_source_layout`]。
                        let code_pad_h = 10.0_f32;
                        let code_pad_v = 6.0_f32;
                        let edit = if code_frame {
                            edit.frame(
                                egui::Frame::NONE
                                    .fill(ui.visuals().code_bg_color)
                                    .stroke(egui::Stroke::new(
                                        1.0,
                                        ui.visuals().widgets.noninteractive.bg_stroke.color,
                                    ))
                                    .corner_radius(egui::CornerRadius::same(
                                        crate::ui::tokens::RADIUS_SM as u8,
                                    ))
                                    .inner_margin(egui::Margin {
                                        left: code_pad_h as i8,
                                        right: code_pad_h as i8,
                                        top: code_pad_v as i8,
                                        bottom: code_pad_v as i8,
                                    }),
                            )
                        } else {
                            edit.frame(egui::Frame::NONE.inner_margin(egui::Margin::ZERO))
                        };
                        // 行距同源(#157):TextEdit 行盒高 = 等宽行高 + 本间距,
                        // 拨到与预览正文行盒(`body_line_height`)等高 —— 配合
                        // 幻影行裁剪,普通段落切形态**逐行等高**,高度零变化。
                        // 差值可为负(等宽行高偏大时收紧行距);空文档分支与
                        // 本分支互斥,且 Ui 每帧重建,不留残值。
                        let mono_font = ui
                            .style()
                            .text_styles
                            .get(&egui::TextStyle::Monospace)
                            .cloned()
                            .unwrap_or_else(|| egui::FontId::monospace(14.0));
                        let mono_row_height = ui.fonts_mut(|f| f.row_height(&mono_font));
                        ui.spacing_mut().extra_text_line_spacing =
                            body_line_height(&gap_style, gap_font_size) - mono_row_height;
                        let output = edit.show(ui);
                        ui.spacing_mut().extra_text_line_spacing = 0.0;
                        // 活动块源码形态的实测高(块切换视口锚定的塌缩侧输入,
                        // 与富渲染臂的 `live-render-h` 同款记账):下一帧本块
                        // 塌缩回富渲染时,点击帧用「富渲染估高 − 源码实测」
                        // 预判它对下方内容造成的位移。
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(
                                editor_id.with(("live-src-h", index)),
                                output.response.response.rect.height(),
                            )
                        });
                        // 徽标手柄(LP2-7,取代 LP2-6 的文字芯片):**左侧槽**
                        // 内悬一个 2×3 圆点手柄(Notion 拖拽手柄同款)—— 用户
                        // 反馈 44px 文字芯片「左右偏移的更狠了」,收窄到 16px
                        // 只容图标。点击弹快捷菜单:标题级别转换(H1–H6/正文,
                        // 文本族块可用)+ 行内格式(加粗/斜体/删除线,作用于
                        // 当前选区)。所有块型都有手柄(菜单首行显示当前类型)。
                        {
                            let clip = ui.clip_rect();
                            let block_rect = output.response.response.rect;
                            // 槽在内容左缘之外;绘制裁剪扩到槽区(galley 画在
                            // 内容 Ui 里,默认裁剪不含槽)
                            let gutter_clip = egui::Rect::from_min_max(
                                egui::pos2(block_rect.left() - BADGE_GUTTER, clip.top()),
                                egui::pos2(block_rect.left(), clip.bottom()),
                            );
                            let painter = ui.painter().with_clip_rect(gutter_clip);
                            // 纵向锚到**第一行 galley 行盒**,不能用 TextEdit
                            // 外框顶:标题行字号被放大时,外框顶与行顶的差会变
                            // 大,手柄就会被抬到上一行(用户实测「徽标槽不再同
                            // 一行,往上了好多」)。第一行行盒与标题/正文排版作业
                            // 同源,因此 H1–H6、正文和代码块都同一基线。
                            let first_row = output.galley.rows.first();
                            let first_row_top = first_row
                                .map(|row| output.galley_pos.y + row.min_y())
                                .unwrap_or(block_rect.top());
                            let first_row_bottom = first_row
                                .map(|row| output.galley_pos.y + row.max_y())
                                .unwrap_or(block_rect.top() + 14.0);
                            let chip_h = 14.0;
                            let chip_y = first_row_top
                                + ((first_row_bottom - first_row_top - chip_h) * 0.5).max(0.0);
                            let chip = egui::Rect::from_min_size(
                                egui::pos2(
                                    block_rect.left() - BADGE_GUTTER + 3.0,
                                    chip_y,
                                ),
                                egui::vec2(12.0, chip_h),
                            );
                            painter.rect_filled(chip, 3.0, egui::Color32::TRANSPARENT);
                            let dot = ui.visuals().weak_text_color();
                            for offset in HANDLE_DOTS {
                                painter.circle_filled(chip.min + offset, 1.3, dot);
                            }
                            let handle = ui.interact(
                                chip,
                                editor_id.with(("live-badge", index)),
                                egui::Sense::click(),
                            );
                            let caret_local = output
                                .state
                                .cursor
                                .char_range()
                                .map(|range| range.primary.index.0)
                                .unwrap_or(0);
                            egui::Popup::menu(&handle).show(|ui| {
                                ui.set_min_width(120.0);
                                ui.label(egui::RichText::new(type_label).weak().monospace());
                                ui.separator();
                                for level in 1u8..=6 {
                                    let row = ui.add_enabled_ui(text_family, |ui| {
                                        ui.selectable_label(current_level == Some(level), format!("H{level}"))
                                    })
                                    .inner;
                                    if row.clicked() {
                                        set_block_heading_level(
                                            editor,
                                            live,
                                            index,
                                            Some(level),
                                            caret_local,
                                        );
                                        ui.close();
                                    }
                                }
                                let row = ui.add_enabled_ui(text_family, |ui| {
                                    ui.selectable_label(
                                        text_family && current_level.is_none(),
                                        "正文",
                                    )
                                })
                                .inner;
                                if row.clicked() {
                                    set_block_heading_level(editor, live, index, None, caret_local);
                                    ui.close();
                                }
                                ui.separator();
                                // 行内格式:复用格式动作通道,作用于当前选区
                                // (无选区时 apply_format_live 静默不动)
                                for (label, action) in [
                                    ("加粗", crate::compose::FormatAction::Bold),
                                    ("斜体", crate::compose::FormatAction::Italic),
                                    ("删除线", crate::compose::FormatAction::Strike),
                                ] {
                                    if ui.selectable_label(false, label).clicked() {
                                        outbox.push(Message::FormatRequested(action));
                                        ui.close();
                                    }
                                }
                            });
                        }
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
                        // 跨块 caret 路由:块首按 ↑/← 去上一块可见末尾;块尾
                        // (可见内容末尾,含幻影行)按 ↓/→ 去下一块开头。四个
                        // 方向同一语义,Shift 按住时一律不路由 —— 选区扩展
                        // 不能跨块(选区活在单个 TextEdit 的 state 里),宁可
                        // 原地不动也别把用户正在拉的选择打断并清空。
                        if output.response.response.has_focus() {
                            let modifiers = ui.input(|input| input.modifiers);
                            let routing_allowed = !modifiers.shift;
                            let at_start = caret == Some(0);
                            let at_end = caret.is_some_and(|caret| caret >= visible_end);
                            let prev_end = (index > 0).then(|| {
                                let prev = &editor.text()[live.blocks[index - 1].clone()];
                                visible_char_len(prev)
                            });
                            if routing_allowed
                                && index > 0
                                && at_start
                                && (ui.input(|input| input.key_pressed(egui::Key::ArrowUp))
                                    || ui.input(|input| input.key_pressed(egui::Key::ArrowLeft)))
                            {
                                route = Some((index - 1, prev_end.unwrap_or(0)));
                            } else if routing_allowed
                                && index + 1 < block_count
                                && at_end
                                && (ui.input(|input| input.key_pressed(egui::Key::ArrowDown))
                                    || ui.input(|input| input.key_pressed(egui::Key::ArrowRight)))
                            {
                                route = Some((index + 1, 0));
                            }
                        }

                        // 光标回填(大纲/状态栏用):块内字符偏移 + 块首
                        cursor.byte = output.state.cursor.char_range().map(|range| {
                            editor.char_to_byte(
                                editor.byte_to_char(live.blocks[index].start)
                                    + range.primary.index.0,
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
                            let primary_released =
                                ui.input(|input| input.pointer.primary_released());
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
                                    state.cursor.set_char_range(Some(
                                        egui::text::CCursorRange::two(
                                            egui::text::CCursor::new(primary),
                                            egui::text::CCursor::new(secondary),
                                        ),
                                    ));
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
                                            egui::text::CCursorRange::one(
                                                egui::text::CCursor::new(char_idx),
                                            ),
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
                                // 点击锚定的残差校正(块切换防跳变的收尾):点击
                                // 帧的预测(源码同参 shaping + 上一活动块富渲染
                                // 估高)与实测源码光标行顶的差通常 <1px,超出
                                // 即把偏移再修一次 —— 本帧内容已画完,修正对
                                // 下一帧生效,量级亚像素/一两个像素,肉眼无感。
                                if let Some((anchor_block, anchor_top)) = live.toggle_anchor {
                                    if anchor_block == index {
                                        let landed_row = output
                                            .galley
                                            .pos_from_cursor(egui::text::CCursor::new(char_idx));
                                        let residual =
                                            output.galley_pos.y + landed_row.min.y - anchor_top;
                                        if residual.abs() > 0.5 {
                                            if let Some(mut scroll) =
                                                egui::scroll_area::State::load(
                                                    ui.ctx(),
                                                    scroll_state_id,
                                                )
                                            {
                                                scroll.offset.y += residual;
                                                scroll.store(ui.ctx(), scroll_state_id);
                                            }
                                        }
                                        live.toggle_anchor = None;
                                    }
                                }
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
                        // 表格 titlebar(2026-10-23,常驻,代码块头部同款 24px):
                        // 直接贴在表格上缘,左「表格」标签、右四枚操作图标。
                        // 悬浮条形态被否(坤哥:飘在整列最右缘太远,要 titlebar
                        // 的感觉)。改写动作不在此刻执行:`block_text` 还借着
                        // editor(下方点击落点还要用),动作在块循环里借用结束
                        // 后落地。
                        let table_action = if kind == BlockKind::Widget
                            && block_text.trim_start().starts_with('|')
                            && !live.browse_locked
                            && live.active != Some(index)
                        {
                            draw_table_titlebar(ui, block_text)
                        } else {
                            TableAction::None
                        };
                        // 链接改写(#63 checkbox + #65 高亮):块文本先过改写层
                        // (缓存键是源码指纹 + 块序号,稳态帧零解析),`task://`
                        // 链接由本块专属 handler 画成自绘 checkbox、`hl://` 画
                        // 成底色段 —— 探针坐标域带块首,点击切换由 `ui` 帧末的
                        // 探针命中判定统一发出。无目标字符的块改写恒等,渲染
                        // 零变化。
                        let (
                            block_rendered,
                            offset_map,
                            highlight_map,
                            emoji_map,
                            task_map,
                            image_map,
                        ) = live.rich_block_with_base(index, block_text, base_dir);
                        // 字体与右栏预览同源(#23 F3):size = 用户字号偏好,
                        // 族 = 预览专用族(#43 M2 的行 metrics 对齐副本,无 CJK
                        // 回落 Proportional)。源码/Live 两种模式下排版偏好同观感。
                        // 两栏同一套富渲染链接 handler(P2/D1):wiki/hl/
                        // emoji/task/mermaid + AI 卡与 ai:// 拦截(with_ai
                        // 显式开启 —— #51 M3 岔路②的显式反向决定,
                        // decisions-pending 在册)。task 坐标域带块首字节;
                        // emoji 收集器全块共享。
                        let dark_mode = matches!(ui.ctx().theme(), egui::Theme::Dark);
                        let rich_handler = RichLinkHandler::new(
                            TaskProbeDomain::Live(range.start),
                            dark_mode,
                            ui.visuals().text_color(),
                            std::rc::Rc::clone(&emoji_rects),
                        )
                        .with_ai(crate::ui::rich_widgets::ai_link_color(dark_mode), ai);
                        // 代码块头部按钮(复制 + 关闭):可写列的**围栏代码块**
                        // 才挂关闭 —— 缩进代码块没有围栏可去(关了还是缩进块),
                        // 对照列只读。关闭点击只把块序号推进 sink,确认弹窗
                        // 在帧末统一渲染(见 `pending_block_action`)。
                        // sink 只在围栏代码块上分配:普通块每帧零分配。
                        let fenced_header =
                            kind == BlockKind::Widget && closeable_fence(block_text);
                        let close_sink = if fenced_header && !live.browse_locked {
                            Some(std::rc::Rc::new(std::cell::RefCell::new(Vec::new())))
                        } else {
                            None
                        };
                        let with_close = close_sink.as_ref().map(|sink| {
                            let sink = std::rc::Rc::clone(sink);
                            move |ui: &mut egui::Ui, code: &str, lang: &str| {
                                crate::ui::rich_widgets::code_header_buttons(
                                    ui,
                                    code,
                                    lang,
                                    Some(crate::ui::rich_widgets::CodeClose {
                                        index,
                                        sink: &sink,
                                    }),
                                );
                            }
                        });
                        let label = MarkdownLabel::new(
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
                        .code_block_min_width(Some(200.0));
                        // 代码块头部按钮:可写围栏代码块挂「复制+关闭」,其余
                        // 挂只读的「复制」。
                        let label = if let Some(buttons) = with_close.as_ref() {
                            label.code_block_buttons(buttons)
                        } else {
                            label.code_block_buttons(&crate::ui::rich_widgets::code_copy_buttons)
                        };
                        label
                        // heal(live-unify-plan P1/R1):AI 流式写入本标签的帧
                        // 开启,残缺 fence / 未闭合 ** 在块片段上补闭合 —— 块级
                        // 契约由 latermd_md 的 block_level_heal_closes_streaming_
                        // fragments 钉住。完整块上 heal 恒等(Cow::Borrowed),
                        // 开关切换不改变完整块渲染(与预览同一口径)。
                        .heal(heal)
                        // 视口外代码块高亮延迟(#59/#60,vendor ①类配套):缓存
                        // miss 帧里起始于视口下缘之外的 flush 段不做 syntect
                        // 高亮,滚到可见前的同帧补齐 —— 大文档写作模式首切
                        // 同享 20k 行 876→173ms 的收益(预览同参数)。
                        .defer_offscreen_highlight(true)
                        // mermaid 块出图(#51 M3)+ 任务 checkbox(#63),
                        // 与右栏预览同一渲染入口。
                        .link_handler(&rich_handler)
                        .show(ui);
                        // drain 前先取「本块拦截了链接点击」标志(wikilink/本地
                        // 文档链接都走消息、无 OpenUrl,`clicked_for_edit` 的
                        // OpenUrl 探测看不见它们,须显式排除 —— 点击归链接,
                        // 不连带切入编辑)。
                        let link_intercepted = !rich_handler.intercepted_is_empty();
                        rich_handler.drain_into(outbox);
                        // 代码块关闭信号(drain 在 sink 之后读,同一帧成立):
                        // 只置待确认动作,弹窗与改写在帧末统一处理。
                        if close_sink
                            .as_ref()
                            .is_some_and(|sink| !sink.borrow().is_empty())
                        {
                            live.pending_block_action = Some(PendingBlockAction::CodeClose(index));
                        }
                        let bottom = ui.cursor().top();
                        let rect = egui::Rect::from_min_max(
                            egui::pos2(ui.min_rect().left(), top),
                            egui::pos2(ui.min_rect().right(), bottom.max(top + 1.0)),
                        );
                        // 真实渲染块的实测高(#111 B 估高剔除的记账半边):下一帧
                        // 本块若滚出视口被估高占位替换,占位与实测的差值按它
                        // 补偿滚动偏移(见循环头剔除臂的 `estimate_drift`)。
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(editor_id.with(("live-render-h", index)), rect.height())
                        });
                        // D2 落地(P3):浏览态的跳转在本块画完后只做「滚到
                        // 目标块」—— 块是富渲染,没有可落的光标,不切活动、
                        // 不抢焦点,pending 即刻消费。块循环每帧遍历全部块
                        // (视口剔除只作用于绘制),目标块在屏外的帧同样拿得
                        // 到 rect,滚动当帧生效。
                        if live.browse_locked {
                            if let Some((block, _)) = live.pending_caret {
                                if block == index {
                                    live.pending_caret = None;
                                    live.caret_follow = false;
                                    locked_jump_landing(ui, live, rect);
                                }
                            }
                        }
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
                        // 落点不滚动跟随:指针刚把视口定位到点击处(视口锚定
                        // 见下方 `pending_shift` —— 高度差的补偿恰恰是为了让
                        // 「点击处」在块切源码后钉在原地)。
                        // 浏览态同样单击进编辑(2026-10-19 拍板,取代早期
                        // 「双击才进写作」的门槛):点哪编哪是 MarkText 手感
                        // 的底线,禅定锁定时整条入口被 browse_enabled 关死。
                        let edit_pos = if live.browse_only {
                            live.browse_enabled
                                .then(|| clicked_for_edit(ui, rect, editor_id, link_intercepted))
                                .flatten()
                        } else {
                            clicked_for_edit(ui, rect, editor_id, link_intercepted)
                        };
                        if let Some(pos) = edit_pos {
                            let label_id = editor_id.with(("live-render", index));
                            let block_source = BlockBuffer::slice(editor.text(), &range);
                            let local = LiveState::click_caret_local(
                                ui,
                                label_id,
                                pos,
                                block_rendered.as_ref(),
                                image_map.as_ref(),
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
                            // 块切换视口锚定(点击帧预测,帧末 `activate` 消费
                            // 点落账):块从富渲染切源码高度必变,若不动偏移,
                            // 下一帧视口内内容整体平移 —— 「点哪跳哪」。这里
                            // 同帧凑齐四项差值,把「光标行切源码后的屏幕顶」
                            // 预算出来,帧末写进 ScrollArea 偏移让重排帧一次
                            // 到位:
                            // ① 被点击渲染行的屏幕顶(`row_rect_at_pos`,锚点
                            //    本体;点击落在文本 galley 之外时拿不到行几何,
                            //    放弃锚定保持旧行为);
                            // ② 本块源码形态的光标行顶与外框高(同参 shaping
                            //    预测,见 [`predict_source_layout`]);
                            // ③ 上一活动块(若在本块上方)塌缩富渲染的位移:
                            //    富渲染高用估高器预测、源码高读上一帧实测
                            //    (`live-src-h`;无实测 = 上帧不可见/刚换文档,
                            //    放弃该项,残差交给落地帧校正)。
                            let clicked_row_top =
                                egui_markdown::row_rect_at_pos(ui, label_id, pos).map(|r| r.min.y);
                            if let Some(row_top) = clicked_row_top {
                                let view_bytes = block_text.trim_end_matches('\n').len();
                                let (_src_h, src_caret_top) =
                                    predict_source_layout(ui, &block_text[..view_bytes], local);
                                let collapse_delta = match live.active {
                                    Some(old) if old < index => {
                                        let old_src_h = ui.ctx().data(|d| {
                                            d.get_temp::<f32>(editor_id.with(("live-src-h", old)))
                                        });
                                        match old_src_h {
                                            Some(old_src_h) if old_src_h > 0.0 => {
                                                let old_range = live.blocks[old].clone();
                                                let old_text =
                                                    BlockBuffer::slice(editor.text(), &old_range);
                                                let (old_rendered, ..) =
                                                    live.rich_block_with_base(old, old_text, base_dir);
                                                let dark_mode =
                                                    matches!(ui.ctx().theme(), egui::Theme::Dark);
                                                let old_handler = RichLinkHandler::new(
                                                    TaskProbeDomain::Live(
                                                        live.blocks[old].start,
                                                    ),
                                                    dark_mode,
                                                    ui.visuals().text_color(),
                                                    std::rc::Rc::clone(&emoji_rects),
                                                )
                                                .with_ai(
                                                    crate::ui::rich_widgets::ai_link_color(dark_mode),
                                                    ai,
                                                );
                                                // 与剔除臂占位同参的估高(+4px 内距
                                                // 同款),与 `live-render-h` 记账口径
                                                // 一致(见 #111 B)。
                                                let estimate = egui_markdown::MarkdownLabel::new(
                                                    editor_id.with(("live-render", old)),
                                                    old_rendered.as_ref(),
                                                )
                                                .font(egui::FontId::new(
                                                    crate::theme::editor_font_size(ui.ctx()),
                                                    crate::fonts::preview_body_family(ui.ctx()),
                                                ))
                                                .wrap()
                                                .overflow_wrap(egui_markdown::OverflowWrap::BreakAll)
                                                .scroll_code_blocks(true)
                                                .shrink_code_blocks(true)
                                                .code_block_min_width(Some(200.0))
                                                .heal(heal)
                                                .defer_offscreen_highlight(true)
                                                .link_handler(&old_handler)
                                                .estimated_height(ui);
                                                // 内距已全零(LP2-6):估高即源码框高,
                                                // 不再加 TextEdit 上下内距。
                                                estimate - old_src_h
                                            }
                                            _ => 0.0,
                                        }
                                    }
                                    _ => 0.0,
                                };
                                pending_shift =
                                    Some(rect.min.y + collapse_delta + src_caret_top - row_top);
                                pending_anchor = Some((index, row_top));
                            }
                        }
                        // 表格工具条动作落地:加行/加列就地改写(单步 undo
                        // 可退),删除挂待确认。放在点击落点之后 —— `block_text`
                        // 对 editor 的借用到这里才结束,更早会撞借用。
                        match table_action {
                            TableAction::AddRow => table_add_row(editor, live, index),
                            TableAction::AddCol => table_add_col(editor, live, index),
                            TableAction::Delete => {
                                live.pending_block_action =
                                    Some(PendingBlockAction::TableDelete(index));
                            }
                            TableAction::None => {}
                        }
                    }
                }
                // 滚动锚定收尾(#111 B):视口上方的估高占位若比上一帧实测
                // (或上一帧占位)更高/更矮,下方内容会整体位移 —— 同向修正
                // 偏移让视口内容钉在原地。下方块的差值只影响内容总高(滚动
                // 条长度),不修。必须发生在 ScrollArea end() 之前才对下一帧
                // 生效;egui 0.36 的 State::load/store 都是 persisted data,
                // 读写一致。
                if estimate_drift.abs() > f32::EPSILON {
                    if let Some(mut state) =
                        egui::scroll_area::State::load(ui.ctx(), scroll_state_id)
                    {
                        state.offset.y += estimate_drift;
                        state.store(ui.ctx(), scroll_state_id);
                    }
                }
            });

    // emoji inline widget 的收尾(P1,与右栏预览同一收尾:①手型抑制 ——
    // vendored 对悬停中的 inline widget 无条件置 PointingHand(label.rs
    // `handle_hover`),`link_style` 管不到光标,只能在帧末把指针悬停 emoji
    // 时的光标按回 Default;中央面板后画,后写者胜,预览侧同帧的同名写被
    // 覆盖是预期);②把本帧占位区写进探针(每帧整帧覆盖,零 emoji 帧不留
    // 上一帧的旧区块),无头测试读它断言 widget 生效段。
    let emoji_rects = emoji_rects.borrow().clone();
    let hovering_emoji = panel
        .input(|input| input.pointer.latest_pos())
        .is_some_and(|pos| emoji_rects.iter().any(|rect| rect.contains(pos)));
    if hovering_emoji {
        panel.output_mut(|out| out.cursor_icon = egui::CursorIcon::Default);
    }
    panel
        .ctx()
        .data_mut(|d| d.insert_temp(emoji_probe_id(editor_id), emoji_rects));

    // 滚动探针(P3,`LiveScrollProbe` 的 data 落点):帧末滚动偏移 +
    // 视口矩形 + 内容高度,生产只写不读 —— 测试断言「浏览态跳转真的滚动
    // 了视口」用。
    panel.ctx().data_mut(|d| {
        d.insert_temp(
            scroll_probe_id(editor_id),
            LiveScrollProbe {
                offset: scrolled.state.offset.y,
                viewport: scrolled.inner_rect,
                content_height: scrolled.content_size.y,
            },
        )
    });

    let focus_target_changed = activate.is_some() || route.is_some();
    if let Some((index, local)) = activate.take() {
        live.browse_only = false;
        live.active = Some(index);
        live.pending_caret = Some((index, local));
        live.pending_range = None;
        // 点击进编辑不跟随:指针刚把视口定位到点击处(decisions-pending #83;
        // 块切源码造成的高度差由下方 pending_shift 补偿,而非光标跟随)。
        live.caret_follow = false;
        live.drag_anchor = None;
    }
    // 锚定补偿落账:点击帧预算出的位移在 ScrollArea 收尾之后写进 state
    // —— 本帧内容已按旧偏移画完,下一帧 begin() 读到的就是修正后的偏移,
    // 块切源码的重排帧一次到位、不闪跳。落地帧按实测光标行做残差校正
    // (toggle_anchor 的消费点)。打字机的 last_offset 在其写回处同步加同
    // 等量(anchor_shift_applied):这次「用户感知不到的滚动」不该被记成
    // 一次 Override 暂停(#121 已知边界类的显式豁免,而非新增边界)。
    let mut anchor_shift_applied = 0.0;
    if let Some(shift) = pending_shift.take() {
        if let Some(mut state) = egui::scroll_area::State::load(panel.ctx(), scroll_state_id) {
            state.offset.y += shift;
            state.store(panel.ctx(), scroll_state_id);
        }
        // last_offset 的位移补偿推迟到打字机块的写回处:本帧的 moved 判定
        // 比的是两个「闭包内偏移」(锚定 store 在 ScrollArea 收尾之后,不进
        // 本帧闭包),提前 bump 反而会让本帧凭空记一次移动。
        anchor_shift_applied = shift;
    }
    live.toggle_anchor = pending_anchor.take();

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
        // 锚定位移写进记忆基线:下一帧闭包读到的偏移已含这笔修正,基线
        // 不同步就会把「感知不到的视口钉扎」误记成一次用户滚动。
        tw.last_offset = scrolled.state.offset.y + anchor_shift_applied;
        tw.last_rev = Some(editor.revision());
        tw.enabled = true;
        panel
            .ctx()
            .data_mut(|d| d.insert_temp(crate::ui::typewriter::live_memory_id(editor_id), tw));
    }
    if let Some(route) = route.take() {
        live.browse_only = false;
        // 键盘路由覆盖 pending_caret,点击锚定一并作废(路由有自己的
        // caret_follow 滚动语义,两者不能抢同一次视口)。
        live.toggle_anchor = None;
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
    if let Some((href, crate::ui::rich_widgets::TaskProbeDomain::Live(base))) =
        crate::ui::rich_widgets::task_click_hit(panel)
    {
        if let Some((_, offset)) = crate::ui::rich_widgets::parse_task_href(&href) {
            outbox.push(Message::TaskCheckboxToggled {
                byte: base + offset,
            });
            crate::ui::rich_widgets::restore_stashed_focus(panel.ctx());
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
    let sel_anchor = if show_selection_ai
        && crate::ui::selection_ai::badge_visible(
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

    // 快照同步(与源码模式同一条规则:仅修订号前进时重建;大纲那一半另有
    // 「侧栏是否正在显示」的闸门,见 `PreviewState::sync_outline`)
    preview.sync_outline(editor);

    // 块级改写确认弹窗(2026-10-23):关闭代码块 / 删除表格。只在可写列
    // 渲染(对照列锁浏览,产生的动作都不该有);确认帧才改写,取消/× 即弃。
    if !live.browse_locked {
        if let Some(action) = live.pending_block_action {
            let (title, lead, warning, confirm_text) = action.dialog_text();
            let (confirm, cancel, close) = crate::ui::layout::destructive_confirm_dialog(
                panel,
                title,
                &lead,
                warning,
                confirm_text,
                None,
                None,
            );
            if confirm.clicked() {
                apply_pending_block_action(editor, live, action);
                live.pending_block_action = None;
            } else if cancel.clicked() || close {
                live.pending_block_action = None;
            }
        }
    }

    active_response
        .unwrap_or_else(|| panel.allocate_response(egui::vec2(0.0, 0.0), egui::Sense::click()))
}

/// 确认弹窗「确认」帧的改写分派。抽成独立函数:测试直接驱动它验证
/// 动作路由,不必在无头帧里点弹窗按钮。
fn apply_pending_block_action(
    editor: &mut EditorBuffer,
    live: &mut LiveState,
    action: PendingBlockAction,
) {
    match action {
        PendingBlockAction::CodeClose(index) => close_code_block(editor, live, index),
        PendingBlockAction::TableDelete(index) => delete_block(editor, live, index),
    }
}

// ---------------------------------------------------------------- 表格活动态:单元格原样编辑
//
// (#310 需求,2026-10-23)活动表格不再塌成整块源码 TextEdit,而是渲染成
// 仍在表格形态的单元格编辑器:每个单元格一个单行 TextEdit,改动经
// [`CellBuffer`] 直接落回共享缓冲(与 [`BlockBuffer`] 同一通道,TextEdit
// 内建 undo 单步可退)。分隔行自动维护、不可编辑;行列增删走 titlebar。

/// 活动表格的一行。`cells` 的 `src` 是**块内字节区间**(管道之间的原始
/// 单元格文本,含空白);分隔行无单元格。
struct ActiveTableRow {
    header: bool,
    separator: bool,
    /// The source line range within the table block, excluding its newline.
    source: Range<usize>,
    cells: Vec<(String, Range<usize>)>,
}

/// 把表格块解析成行/单元格(带块内字节区间)。要求管道表格的规整形态
/// (每行 ≥2 根管道);不合规格返回 None,活动态回落整块源码 TextEdit。
fn table_rows(block: &str) -> Option<Vec<ActiveTableRow>> {
    let content = block.trim_end_matches(['\r', '\n']);
    if content.is_empty() {
        return None;
    }
    let mut rows = Vec::new();
    let mut line_start = 0;
    for (i, raw_line) in content.split('\n').enumerate() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let line_source_start = line_start;
        line_start += raw_line.len() + usize::from(line_start + raw_line.len() < content.len());
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            return None;
        }
        let separator = i == 1 && trimmed.trim_matches('|').contains('-');
        if separator {
            rows.push(ActiveTableRow {
                header: false,
                separator: true,
                source: line_source_start..line_source_start + line.len(),
                cells: Vec::new(),
            });
            continue;
        }
        let pipes: Vec<usize> = line
            .char_indices()
            .filter(|(_, c)| *c == '|')
            .map(|(b, _)| b)
            .collect();
        if pipes.len() < 2 {
            return None;
        }
        let cells = pipes
            .windows(2)
            .map(|w| {
                let raw = &line[w[0] + 1..w[1]];
                (
                    raw.trim().to_owned(),
                    line_source_start + w[0] + 1..line_source_start + w[1],
                )
            })
            .collect();
        rows.push(ActiveTableRow {
            header: i == 0,
            separator: false,
            source: line_source_start..line_source_start + line.len(),
            cells,
        });
    }
    (rows.len() >= 2).then_some(rows)
}

/// 单元格的编辑代理:把共享缓冲的**一个单元格**暴露成 egui `TextBuffer`。
/// 与 [`BlockBuffer`] 同一落盘通道;偏移全程用字符(单元格起点换算成文档
/// 字符位后,单元格内偏移直接相加)。`len_chars` 随编辑自维护 —— 同一帧
/// 内只有聚焦单元格会产生编辑,区间自洽。
struct CellBuffer<'a> {
    buffer: &'a mut EditorBuffer,
    /// 单元格起点的文档字符偏移。
    start_char: usize,
    len_chars: usize,
}

impl egui::TextBuffer for CellBuffer<'_> {
    fn is_mutable(&self) -> bool {
        true
    }

    fn as_str(&self) -> &str {
        let text = self.buffer.text();
        let start_char = self.start_char.min(self.buffer.len_chars());
        let end_char = (self.start_char + self.len_chars).min(self.buffer.len_chars());
        let start = self.buffer.char_to_byte(start_char);
        let end = self.buffer.char_to_byte(end_char.max(start_char));
        &text[start..end]
    }

    fn insert_text(&mut self, text: &str, char_index: egui::text::CharIndex) -> usize {
        let local = char_index.0.min(self.len_chars);
        let at = (self.start_char + local).min(self.buffer.len_chars());
        self.buffer.insert_chars(at, text);
        let inserted = text.chars().count();
        self.len_chars += inserted;
        inserted
    }

    fn delete_char_range(&mut self, char_range: Range<egui::text::CharIndex>) {
        let start_local = char_range.start.0.min(self.len_chars);
        let end_local = char_range.end.0.min(self.len_chars).max(start_local);
        let start = (self.start_char + start_local).min(self.buffer.len_chars());
        let end = (self.start_char + end_local)
            .min(self.buffer.len_chars())
            .max(start);
        self.buffer.remove_chars(start..end);
        self.len_chars -= end_local - start_local;
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<CellBuffer<'static>>()
    }
}

/// 活动表格的渲染产出:聚焦单元格的 TextEdit 输出(右键菜单挂点 +
/// 光标回填),无聚焦单元格时为 None。
struct ActiveTableFocus {
    output: egui::widgets::text_edit::TextEditOutput,
    /// 聚焦单元格起点的文档字符偏移(块内光标 → 文档光标的换算基)。
    cell_start_char: usize,
}

/// 活动表格:单元格原样编辑。逐行渲染 —— 表头行文字加色、数据行普通、
/// 分隔行画一条横线(不可编辑);每个单元格一个单行 TextEdit,列宽取该列
/// 最宽单元格的 galley 宽(与 vendored 表格同口径:按文本测量)。
///
/// `pending_caret` 落地:块内字符偏移映射到所在单元格,焦点与光标一并
/// 写入(点击富渲染表格进编辑的落点通道不变)。返回聚焦单元格的产出供
/// 右键菜单与光标回填;没有单元格持焦时返回 None。
#[allow(clippy::too_many_arguments)]
fn draw_active_table(
    ui: &mut egui::Ui,
    editor: &mut EditorBuffer,
    live: &mut LiveState,
    index: usize,
    block_text: &str,
    block_start_byte: usize,
    editor_id: egui::Id,
    keymap: &crate::keymap::Keymap,
    outbox: &mut Vec<Message>,
) -> Option<ActiveTableFocus> {
    let _ = keymap;
    let _ = outbox;
    let rows = table_rows(block_text)?;
    let font_size = crate::theme::editor_font_size(ui.ctx());
    let body_family = crate::fonts::preview_body_family(ui.ctx());
    let font = egui::FontId::new(font_size, body_family.clone());
    let style = egui_markdown_style::global_style(ui.ctx());
    let pad = &style.table.cell_padding;
    let cell_pad_x = pad[0] + pad[2];
    let grid_stroke = egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);

    // 列宽:该列最宽单元格 galley + 单元格内距,钳最小可点宽
    let col_count = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
    let mut col_w = vec![48.0_f32; col_count];
    for row in &rows {
        for (c, (text, _)) in row.cells.iter().enumerate() {
            let galley =
                ui.painter()
                    .layout_no_wrap(text.clone(), font.clone(), egui::Color32::WHITE);
            col_w[c] = col_w[c].max(galley.size().x + cell_pad_x + 4.0);
        }
    }
    // titlebar 与表格网格共用最小宽度:四个按钮必须完整落在表格范围内,
    // 不能出现「titlebar 比下面表格宽」或按钮溢出表格的断层。
    let table_width = col_w.iter().sum::<f32>();
    let titlebar_min_width = table_titlebar_min_width();
    if table_width < titlebar_min_width && col_count > 0 {
        let extra = (titlebar_min_width - table_width) / col_count as f32;
        for width in &mut col_w {
            *width += extra;
        }
    }

    // pending_caret 落点 → (行, 列, 单元格内字符)。所有比较都基于
    // table_rows 产出的块内源区间，不能把区间误当成当前行的局部 offset。
    let mut land: Option<(usize, usize, usize)> = None;
    if let Some((block, char_idx)) = live.pending_caret {
        if block == index {
            for (r, row) in rows.iter().enumerate() {
                let row_end = block_text[..row.source.end].chars().count();
                if char_idx <= row_end {
                    let mut target = None;
                    for (c, (_, src)) in row.cells.iter().enumerate() {
                        let cell_start = block_text[..src.start].chars().count();
                        let cell_len = block_text[src.clone()].chars().count();
                        if char_idx <= cell_start + cell_len {
                            target = Some((
                                c,
                                char_idx
                                    .saturating_sub(cell_start)
                                    .saturating_add(2)
                                    .min(cell_len),
                            ));
                            break;
                        }
                    }
                    let (c, local) = target.unwrap_or_else(|| {
                        let (c, (_, src)) = row
                            .cells
                            .iter()
                            .enumerate()
                            .next_back()
                            .expect("table row has cells");
                        (c, block_text[src.clone()].chars().count())
                    });
                    land = Some((r, c, local));
                    break;
                }
            }
            live.pending_caret = None;
            live.caret_follow = false;
        }
    }

    let mut focus: Option<ActiveTableFocus> = None;
    // Keep source offsets valid after an earlier cell edit in this frame.
    let mut source_shift: isize = 0;
    for (r, row) in rows.iter().enumerate() {
        if row.separator {
            // 分隔行:横线占位(不可编辑,列数变化时自动重排)
            let total: f32 = col_w.iter().sum();
            let (rect, _) = ui.allocate_exact_size(egui::vec2(total, 6.0), egui::Sense::hover());
            ui.painter().line_segment(
                [
                    egui::pos2(rect.left(), rect.center().y),
                    egui::pos2(rect.right(), rect.center().y),
                ],
                egui::Stroke::new(1.5, grid_stroke.color),
            );
            continue;
        }
        for (c, (_, src)) in row.cells.iter().enumerate() {
            let cell_id = editor_id.with(("live-tcell", index, r, c));
            let adjusted_start = (src.start as isize + source_shift).max(0) as usize;
            let adjusted_end =
                (src.end as isize + source_shift).max(adjusted_start as isize) as usize;
            let text = editor.text();
            let start_byte = (block_start_byte + adjusted_start).min(text.len());
            let end_byte = (block_start_byte + adjusted_end)
                .min(text.len())
                .max(start_byte);
            let cell_start_char = editor.byte_to_char(start_byte);
            let len_chars = text[start_byte..end_byte].chars().count();
            let before_len = editor.text().len();
            let mut buffer = CellBuffer {
                buffer: editor,
                start_char: cell_start_char,
                len_chars,
            };
            let mut cell = egui::TextEdit::singleline(&mut buffer)
                .id(cell_id)
                .font(egui::FontId::new(font_size, body_family.clone()))
                .desired_width(col_w[c])
                // 与活动块同款:编辑中不被普通控件收回焦点
                .lock_focus(true);
            if row.header {
                cell = cell.text_color(ui.visuals().strong_text_color());
            }
            let output = cell.show(ui);
            let after_len = editor.text().len();
            source_shift += after_len as isize - before_len as isize;
            // 单元格右缘竖线,拼出网格感
            let rect = output.response.response.rect;
            ui.painter().line_segment(
                [
                    egui::pos2(rect.right(), rect.top()),
                    egui::pos2(rect.right(), rect.bottom()),
                ],
                grid_stroke,
            );
            // pending_caret 落地:写光标 + 抢焦点(与活动块 TextEdit 同口径)
            if let Some((lr, lc, lchar)) = land {
                if (lr, lc) == (r, c) {
                    let mut state = output.state.clone();
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(
                            egui::text::CCursor::new(lchar),
                        )));
                    state.store(ui.ctx(), cell_id);
                    ui.ctx().memory_mut(|mem| mem.request_focus(cell_id));
                }
            }
            let focused = ui.ctx().memory(|mem| mem.has_focus(cell_id));
            if focused {
                focus = Some(ActiveTableFocus {
                    output,
                    cell_start_char,
                });
            }
        }
    }
    focus
}

/// 表格 titlebar 的高度:与代码块头部同档(vendored
/// `CODE_BLOCK_HEADER_HEIGHT` = 24)。「常驻 titlebar」形态的高度要进估高
/// 臂的占位(`table_titlebar_h`),两处同源,别只改一处。
const TABLE_TITLEBAR_H: f32 = 24.0;

/// 表格 titlebar 与表格网格共同使用的最小宽度。
fn table_titlebar_min_width() -> f32 {
    let icon_w = crate::ui::tokens::ICON + 2.0 * crate::ui::tokens::SPACE_XS;
    4.0 * icon_w + 3.0 * 2.0 + 60.0 + 2.0 * 4.0
}

/// 表格 titlebar 的占位高度:表格块且可写列(与真实渲染臂同一闸门)才有。
/// 估高臂给视口外表格块补这 24px,否则占位永远比实测矮一截。
fn table_titlebar_h(kind: BlockKind, block_text: &str, live: &LiveState, index: usize) -> f32 {
    if kind == BlockKind::Widget
        && block_text.trim_start().starts_with('|')
        && !live.browse_locked
        && live.active != Some(index)
    {
        TABLE_TITLEBAR_H
    } else {
        0.0
    }
}

/// 表格工具条本帧的用户意图。绘制时收集,绘制返回后(块文本的借用
/// 结束处)才执行改写。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableAction {
    None,
    AddRow,
    AddCol,
    Delete,
}

/// 表格 titlebar(2026-10-23,常驻,代码块头部同款):在表格上缘分配
/// [`TABLE_TITLEBAR_H`] 高的横条,左端「表格」标签、右端四枚操作图标
/// (加行 / 加列 / 复制 / 删除)。复制就地走剪贴板(块文本已在手上);
/// 加行/加列/删除作为动作返回,由调用方在借用结束后执行。图标 rect 记进
/// 工具条探针,「点击进编辑」据此排除;titlebar 空白处的点击仍归
/// 「整块点击进编辑」。
///
/// 悬浮条形态被否(坤哥:飘在整列最右缘太远,要 titlebar 的感觉);
/// 常驻条直接参与布局,估高臂按 [`table_titlebar_h`] 补占位,滚动无跳变。
fn draw_table_titlebar(ui: &mut egui::Ui, block_text: &str) -> TableAction {
    let size = egui::vec2(
        crate::ui::tokens::ICON + 2.0 * crate::ui::tokens::SPACE_XS,
        20.0,
    );
    let count = 4.0_f32;
    let gap = 2.0;
    let pad = 4.0_f32;
    // 宽度跟表格实际宽:取块内最宽行的 galley 宽(vendored 表格列宽同样是
    // 按单元格文本测量),加左右内距 —— 不铺满整行,窄表格的 titlebar 贴
    // 着表格右缘。下限 = 图标带 + 标签的最小可读宽,上限 = 可用宽。
    let font = egui::FontId::new(
        crate::theme::editor_font_size(ui.ctx()),
        crate::fonts::preview_body_family(ui.ctx()),
    );
    let mut measured = 0.0_f32;
    for line in block_text.trim_end_matches('\n').lines() {
        let galley =
            ui.painter()
                .layout_no_wrap(line.to_owned(), font.clone(), egui::Color32::WHITE);
        measured = measured.max(galley.size().x);
    }
    let min_w = table_titlebar_min_width();
    let bar_w = measured.clamp(min_w, (ui.available_width() - pad).max(min_w)) + 2.0 * pad;
    let bar_rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(bar_w, TABLE_TITLEBAR_H));
    // 占位推进游标:hover 感知即可 —— 空白处的点击让给「整块点击进编辑」
    let _ = ui.allocate_rect(bar_rect, egui::Sense::hover());

    // 底:淡填充 + 描边,顶部圆角、底部直角 —— 视觉上与下方表格一体
    let painter = ui.painter();
    let radius = egui::CornerRadius {
        nw: crate::ui::tokens::RADIUS_SM as u8,
        ne: crate::ui::tokens::RADIUS_SM as u8,
        sw: 0,
        se: 0,
    };
    painter.rect_filled(bar_rect, radius, ui.visuals().extreme_bg_color);
    painter.rect_stroke(
        bar_rect,
        radius,
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    // 左端「表格」标签(弱色小字,标题位)
    painter.text(
        egui::pos2(bar_rect.left() + pad + 2.0, bar_rect.center().y),
        egui::Align2::LEFT_CENTER,
        "表格",
        egui::FontId::proportional(12.0),
        ui.visuals().weak_text_color(),
    );
    let actions = [
        (
            crate::ui::icons::Icon::RowAdd,
            "在表格末尾添加一行",
            TableAction::AddRow,
        ),
        (
            crate::ui::icons::Icon::ColAdd,
            "在表格右侧添加一列",
            TableAction::AddCol,
        ),
        (
            crate::ui::icons::Icon::Copy,
            "复制表格 Markdown",
            TableAction::None,
        ),
        (
            crate::ui::icons::Icon::Trash,
            "删除表格",
            TableAction::Delete,
        ),
    ];
    let mut action = TableAction::None;
    let mut hit_rects = Vec::new();
    // 图标贴右缘排布:从右往左逐枚推进
    let icons_w = count * size.x + (count - 1.0) * gap;
    let mut cursor_x = bar_rect.right() - pad - icons_w;
    for (icon, tip, effect) in actions {
        let rect = egui::Rect::from_min_size(
            egui::pos2(cursor_x, bar_rect.center().y - size.y * 0.5),
            size,
        );
        cursor_x += size.x + gap;
        let response = ui.allocate_rect(rect, egui::Sense::click());
        let (fill, color) = if response.hovered() {
            (
                Some(ui.visuals().widgets.hovered.bg_fill),
                ui.visuals().text_color(),
            )
        } else {
            (None, ui.visuals().weak_text_color())
        };
        if let Some(fill) = fill {
            ui.painter()
                .rect_filled(rect, crate::ui::tokens::RADIUS_SM, fill);
        }
        icon.draw(ui.painter(), rect.center(), crate::ui::tokens::ICON, color);
        let response = response.on_hover_text(tip);
        hit_rects.push(rect);
        if !response.clicked() {
            continue;
        }
        match (icon, effect) {
            // 复制就地执行:块文本就在参数里,无需回头借 editor。
            (crate::ui::icons::Icon::Copy, _) => ui
                .ctx()
                .copy_text(block_text.trim_end_matches(['\n', '\r']).to_owned()),
            (_, TableAction::None) => {}
            (_, effect) => action = effect,
        }
    }
    crate::ui::rich_widgets::push_table_toolbar_rects(ui.ctx(), hit_rects);
    action
}

/// 富渲染块「点击进入编辑」的命中检测。**不挂 egui widget**:块级矩形一旦
/// 参与 hit-test,注册序必与块内其它可点件二选一 —— egui 0.36 同层点击
/// 平局取后注册者(hit_test.rs「In tie, pick last = topmost」),后注册的
/// 块矩形抢走代码块复制头(#38)的点击(复制失效、块误进编辑);先注册
/// 又抢不过富渲染体(`MarkdownLabel` 的 `Sense::click_and_drag` 整块响应
/// 在 `show()` 内注册得更晚,实测点击后进编辑同样失效)。所以直接读原始
/// 指针事件:本帧有主键 click(egui 已按拖动阈值判定,块内拖选文本不算
/// click,不误触发)且抬起点落在本块;再排除两类本就另有归属的点击 ——
/// 落在代码块复制按钮上的(本帧探针几何,`crate::ui::rich_widgets::
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
    link_intercepted: bool,
) -> Option<egui::Pos2> {
    // 链接点击归链接(wikilink → 开文档、本地链接 → 应用内跳转):不连带
    // 进编辑。普通外链走 vendored 默认 open_url,由帧输出探测兜住;应用内
    // 跳转只发消息、无 OpenUrl,靠 drain 前的 [`link_intercepted`] 标志。
    if link_intercepted {
        return None;
    }
    let clicked = ui.input(|input| input.pointer.primary_clicked());
    if !clicked {
        return None;
    }
    let pos = ui.input(|input| input.pointer.interact_pos())?;
    if !rect.contains(pos) {
        return None;
    }
    if crate::ui::rich_widgets::copy_button_rects(ui.ctx())
        .iter()
        .any(|button| button.contains(pos))
    {
        return None;
    }
    // 表格工具条(2026-10-23):悬浮条通常在块外,块贴视口顶被钳进块内
    // 时靠这份探针兜住 —— 点击归工具条,不连带进编辑。
    if crate::ui::rich_widgets::table_toolbar_rects(ui.ctx())
        .iter()
        .any(|rect| rect.contains(pos))
    {
        return None;
    }
    // 任务 checkbox(#63):点击归 checkbox 切换(帧内探针几何,由 paint
    // 先于本判定写入),不连带进编辑。
    if crate::ui::rich_widgets::task_checkbox_rects(ui.ctx())
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
#[path = "live/live_offscreen_tests.rs"]
mod live_offscreen_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use egui_markdown::link::LinkHandler as _;

    fn state_with(text: &str) -> (EditorBuffer, LiveState, OutlineCursor) {
        let editor = EditorBuffer::new(text);
        let mut live = LiveState::default();
        live.sync(&editor, None);
        (editor, live, OutlineCursor::default())
    }

    // —— LP2-4 活动块内联构造自动成形的排版守门 ——

    /// 期望色/字号快照:与 job 同一帧取自同一个 Ui,断言不跨帧猜样式。
    struct StyleExpectations {
        strong: egui::Color32,
        hyperlink: egui::Color32,
        text: egui::Color32,
        code_fg: egui::Color32,
        code_bg: egui::Color32,
        highlight_bg: egui::Color32,
        h1_size: f32,
    }

    /// 无头构造 [`active_block_job`]:`run_ui` 取 Ui,顺手带回期望快照。
    fn styled_job(text: &str) -> (egui::text::LayoutJob, StyleExpectations) {
        let ctx = egui::Context::default();
        let mut slot = None;
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let job = active_block_job(ui, text, 480.0);
            let style = egui_markdown_style::global_style(ui.ctx());
            let inline_code = style.inline_code.clone();
            let dark = ui.visuals().dark_mode;
            let expected = StyleExpectations {
                strong: ui.visuals().strong_text_color(),
                hyperlink: ui.visuals().hyperlink_color,
                text: ui.visuals().text_color(),
                code_fg: inline_code.color(dark),
                code_bg: inline_code.background(dark),
                highlight_bg: crate::ui::rich_widgets::highlight_bg_color(ui.visuals()),
                h1_size: crate::theme::editor_font_size(ui.ctx()) * style.heading.scales[0],
            };
            slot = Some((job, expected));
        })
        .drop_without_applying_deltas();
        slot.expect("run_ui 闭包必已执行")
    }

    /// 覆盖 `byte`(字节偏移)的 section;没有则 panic(排版漏段当场红)。
    fn section_at(job: &egui::text::LayoutJob, byte: usize) -> &egui::text::LayoutSection {
        job.sections
            .iter()
            .find(|section| section.byte_range.start.0 <= byte && byte < section.byte_range.end.0)
            .unwrap_or_else(|| panic!("字节 {byte} 不在任何 section"))
    }

    /// 六种构造的内容段套上对应格式;定界符段保持正文观感(LP2-1 的地盘,
    /// 成形层不侵吞)。
    #[test]
    fn active_block_styles_completed_inline_constructs() {
        let text = "**b** *i* ~~s~~ `c` [t](u) ==h==";
        let (job, expected) = styled_job(text);
        // 无头环境没有 bold 族:粗体回退 strong 色(vendored apply_bold 同式)
        assert_eq!(section_at(&job, 2).format.color, expected.strong);
        assert!(section_at(&job, 7).format.italics, "斜体假斜");
        let strike = section_at(&job, 12).format.strikethrough;
        assert_eq!(strike.color, expected.text, "删除线 1px 基色描边");
        assert_eq!(strike.width, 1.0);
        let code = &section_at(&job, 17).format;
        assert_eq!(code.color, expected.code_fg, "行内代码前景");
        assert_eq!(code.background, expected.code_bg, "行内代码药丸底色");
        assert_eq!(
            section_at(&job, 21).format.color,
            expected.hyperlink,
            "链接色"
        );
        let highlight = &section_at(&job, 29).format;
        assert_eq!(highlight.background, expected.highlight_bg, "高亮荧光底");
        assert_eq!(
            highlight.expand_bg,
            crate::ui::rich_widgets::HIGHLIGHT_BG_EXPAND
        );
        // 定界符(`**` 的首字节)不吃任何成形格式:正文色、无底、不假斜
        let delimiter = &section_at(&job, 0).format;
        assert_eq!(delimiter.color, expected.text);
        assert_eq!(delimiter.background, egui::Color32::TRANSPARENT);
        assert!(!delimiter.italics);
    }

    /// 多行引用活动态保留物理换行,`> ` 前缀弱化、正文使用预览正文字体。
    /// 这是 LP2-8 的核心回归:引用不能再落进 Widget 单行等宽路径。
    #[test]
    fn active_block_multiline_quote_keeps_rows_and_prefix() {
        let text = "> 第一行引用内容很长,用于在编辑器宽度内产生稳定折行并验证引用行\n> 第二行 `代码` 内容很长,用于验证内联成形与软换行\n> 第三行引用内容很长,用于验证点击后高度不会压成单行";
        let (job, expected) = styled_job(text);
        let galley = egui::Context::default();
        let ctx = egui::Context::default();
        let mut rows = 0usize;
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(job.clone()));
            rows = galley.rows.len();
        })
        .drop_without_applying_deltas();
        assert!(rows >= 3, "多行引用不能压成单行(实际 {rows} 行)");
        assert_eq!(job.text, text);
        assert_eq!(job.sections.first().unwrap().format.color.a(), 110);
        assert_eq!(
            job.sections
                .iter()
                .find(|section| section.byte_range.start.0 >= 2)
                .unwrap()
                .format
                .color,
            expected.text,
            "引用正文保持正文色"
        );
        let _ = galley;
    }

    /// 纯正文与未配平构造:单 section、零成形 —— LP2-4 之前的行为逐字节
    /// 不变(「打完闭标记才成形」的语义边界)。
    #[test]
    fn active_block_plain_and_unclosed_stay_single_section() {
        for text in ["普通正文,没有标记。", "**b `c [t](u ==h"] {
            let (job, _) = styled_job(text);
            assert_eq!(job.sections.len(), 1, "{text:?}");
            assert_eq!(job.sections[0].byte_range.start.0, 0);
            assert_eq!(job.sections[0].byte_range.end.0, text.len());
            assert_eq!(
                job.sections[0].format.background,
                egui::Color32::TRANSPARENT
            );
        }
    }

    /// 标题块:幽灵 `# ` 前缀观感不回归(#158),标题行内已配平构造在
    /// **标题字号**上成形(代码药丸叠在放大字号上,不是缩回正文)。
    #[test]
    fn active_block_heading_keeps_marker_and_styles_inline() {
        let (job, expected) = styled_job("# 标题 `代码`");
        let ranges: Vec<(usize, usize)> = job
            .sections
            .iter()
            .map(|section| (section.byte_range.start.0, section.byte_range.end.0))
            .collect();
        // 字节:# (0..2) | 标题+空格+开反引号 (2..10) | 代码内容 (10..16) | 闭反引号 (16..17)
        // —— 反引号是定界符,不吃药丸,留在标题正文段里(成形只管内容)
        assert_eq!(ranges, vec![(0, 2), (2, 10), (10, 16), (16, 17)]);
        // 幽灵前缀:正文色 × 43% 透明度(110/255),字号 = 标题字号
        assert_eq!(
            job.sections[0].format.color,
            egui::Color32::from_rgba_unmultiplied(
                expected.text.r(),
                expected.text.g(),
                expected.text.b(),
                110
            )
        );
        assert_eq!(job.sections[0].format.font_id.size, expected.h1_size);
        // 标题文字段:标题字号 + strong 色;代码内容段:标题字号上叠药丸
        assert_eq!(job.sections[1].format.font_id.size, expected.h1_size);
        assert_eq!(job.sections[1].format.color, expected.strong);
        assert_eq!(job.sections[2].format.background, expected.code_bg);
        assert_eq!(job.sections[2].format.font_id.size, expected.h1_size);
        // 两个反引号定界符都不吃药丸(成形只管内容,定界符归 LP2-1)
        assert_eq!(
            job.sections[1].format.background,
            egui::Color32::TRANSPARENT
        );
        assert_eq!(
            job.sections[3].format.background,
            egui::Color32::TRANSPARENT
        );
    }

    // —— LP2-5 列表点击保持位置 ——

    /// 前缀嗅探期望:(文本, 前导空白, marker, rest, 有序)。
    type PrefixCase<'a> = (&'a str, Range<usize>, Range<usize>, Range<usize>, bool);

    /// 前缀嗅探:常见形态全识别,行内强调/缺空白/纯数字不误判。
    #[test]
    fn list_prefix_detects_common_forms() {
        let cases: &[PrefixCase<'_>] = &[
            ("- 项目", 0..0, 0..1, 1..2, false),
            ("* 项目", 0..0, 0..1, 1..2, false),
            ("+ 项目", 0..0, 0..1, 1..2, false),
            ("  * 嵌套", 0..2, 2..3, 3..4, false),
            ("1. 有序", 0..0, 0..2, 2..3, true),
            ("12) 有序", 0..0, 0..3, 3..4, true),
            // 任务项:rest 吃进 marker 后空白 + `[x]` + 尾空白
            ("- [x] 完成", 0..0, 0..1, 1..6, false),
            // 空列表项:marker 后即行尾
            ("-", 0..0, 0..1, 1..1, false),
        ];
        for (text, leading, marker, rest, ordered) in cases {
            let prefix = list_prefix(text).unwrap_or_else(|| panic!("{text:?} 应识别为列表项"));
            assert_eq!(prefix.leading, leading.clone(), "{text:?}");
            assert_eq!(prefix.marker, marker.clone(), "{text:?}");
            assert_eq!(prefix.rest, rest.clone(), "{text:?}");
            assert_eq!(prefix.is_ordered, *ordered, "{text:?}");
        }
        for text in ["*强调*", "10 数字", "正文段落", "> 引用", "1.x 无空"] {
            assert!(list_prefix(text).is_none(), "{text:?} 不该识别成列表项");
        }
    }

    /// 无头排版列表项活动块,测「内容列 x」并对回 vendored 公式的期望列
    /// (同一帧从字体度量独立复算,不抄实现):返回 (内容列 x, 期望列 x,
    /// 全部 section 颜色)。
    fn list_alignment(
        text: &str,
        content_char: usize,
        nesting_spaces: usize,
        task: bool,
    ) -> (f32, f32, Vec<egui::Color32>) {
        let ctx = egui::Context::default();
        let mut slot = None;
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let job = active_block_job(ui, text, 480.0);
            let colors: Vec<egui::Color32> = job.sections.iter().map(|s| s.format.color).collect();
            let style = egui_markdown_style::global_style(ui.ctx());
            let font = egui::FontId::new(
                crate::theme::editor_font_size(ui.ctx()),
                crate::fonts::preview_body_family(ui.ctx()),
            );
            let width_of = |s: &str| {
                ui.ctx()
                    .fonts_mut(|f| s.chars().map(|c| f.glyph_width(&font, c)).sum::<f32>())
            };
            let space_w = width_of(" ");
            // vendored layout.rs ListMarker 分支的同一公式(独立复算)
            let expected = (nesting_spaces / 2) as f32 * space_w * 2.0
                + width_of("99. ")
                + style.list.gap
                + if task { width_of("☐ ") } else { 0.0 };
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            let pos = galley.pos_from_cursor(egui::text::CCursor::new(content_char));
            slot = Some((pos.min.x, expected, colors));
        })
        .drop_without_applying_deltas();
        slot.expect("run_ui 闭包必已执行")
    }

    /// 列表项活动块的内容列 == 富渲染内容列(±0.5px),前缀幽灵化、内容
    /// 正文色 —— 点击进编辑不再横向跳变。
    #[test]
    fn active_block_list_item_keeps_rich_render_column() {
        for (text, content_char, nesting, task) in [
            ("- 项目一", 2, 0, false),
            ("  - 嵌套项", 4, 2, false),
            ("12. 有序条目", 4, 0, false),
            ("- [ ] 待办事项", 6, 0, true),
        ] {
            let (x, expected, colors) = list_alignment(text, content_char, nesting, task);
            assert!(
                (x - expected).abs() < 0.5,
                "{text:?} 内容列 {x} != 富渲染公式列 {expected}"
            );
            // 前缀 section(marker 起)幽灵化:α=110;末个内容 section 不幽灵
            assert_eq!(colors[0].a(), 110, "{text:?} marker 应幽灵化");
            assert_eq!(
                colors.last().unwrap().a(),
                255,
                "{text:?} 内容应保持正文不透明"
            );
        }
    }

    /// 列表项内容里的行内构造照样成形(LP2-4 与 LP2-5 叠加不互斥)。
    #[test]
    fn active_block_list_content_keeps_inline_styles() {
        let (job, expected) = styled_job("- 项目 `代码`");
        // 字节:-(0) 空格(1) 项目(2..8) 空格(8) `(9) 代码(10..16) `(16)
        let code = &section_at(&job, 10).format;
        assert_eq!(code.background, expected.code_bg, "列表内容里的药丸");
        assert_eq!(code.color, expected.code_fg);
        assert_eq!(section_at(&job, 0).format.color.a(), 110, "marker 幽灵化");
    }

    /// 真机字体栈(Inter + Noto CJK 等,`fonts::install`)下的列表对位:默认
    /// 字体绿、真字体红曾真机复现(2026-10-20),字体回退链改变空格/槽位
    /// 度量时对位公式必须依旧成立。无 CJK 字体的环境跳过(#43 口径)。
    #[test]
    fn active_block_list_alignment_with_installed_fonts() {
        if crate::fonts::install(&egui::Context::default()).is_none() {
            return; // 无字体资产的环境(部分 CI)跳过
        }
        let ctx = egui::Context::default();
        assert!(
            crate::fonts::install(&ctx).is_some(),
            "字体资产存在时应安装成功"
        );
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        for ppp in [1.0, 1.25] {
            ctx.set_pixels_per_point(ppp);
            let mut slot = None;
            ctx.run_ui(egui::RawInput::default(), |ui| {
                let text = "- 项目一";
                let job = active_block_job(ui, text, 480.0);
                let style = egui_markdown_style::global_style(ui.ctx());
                let font = egui::FontId::new(
                    crate::theme::editor_font_size(ui.ctx()),
                    crate::fonts::preview_body_family(ui.ctx()),
                );
                let width_of = |s: &str| {
                    ui.ctx()
                        .fonts_mut(|f| s.chars().map(|c| f.glyph_width(&font, c)).sum::<f32>())
                };
                let expected = width_of("99. ") + style.list.gap;
                let galley = ui.fonts_mut(|f| f.layout_job(job));
                let pos = galley.pos_from_cursor(egui::text::CCursor::new(2));
                slot = Some((pos.min.x, expected));
            })
            .drop_without_applying_deltas();
            let (x, expected) = slot.expect("run_ui 闭包必已执行");
            assert!(
                (x - expected).abs() < 0.5,
                "ppp={ppp}: 内容列 {x} != 富渲染公式列 {expected}"
            );
        }
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

    /// 首块必须真的渲染在视口顶(写作/分栏两档同守):估高剔除臂的
    /// 上缘判定曾不带容差,ScrollArea 内容 cursor 与 clip 缘之间的亚像素
    /// 舍入差(实测 0.1px)让首块被判「整块在视口上方」,从此永远走占位
    /// 臂不渲染(known_h 恒 0,误判自我维持)—— 列顶部整块空白,即
    /// 「预览上面多了很多空行」。守门走真实 app 整帧:只有生产面板组合
    /// 才复现那 0.1px;手搭 Panel 复刻测不到。
    #[test]
    fn first_block_renders_at_top_in_live_and_split() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 968.0));
        for mode in [
            crate::live::RenderMode::Live,
            crate::live::RenderMode::Split,
        ] {
            let mut app = crate::LaterMdApp::default();
            app.state.render_mode = mode;
            // 文档首块是 H1「LaterMD」(初始示例文档),富渲染成独立标题。
            let mut heading: Option<egui::Rect> = None;
            // 6 帧:分栏对照列走 `show_collapsible` 滑入动画(PR#258),
            // 前几帧面板还没展开,首块字形要等动画到位才出现。
            for _ in 0..6 {
                // eframe 0.36:logic(归约)与 ui(绘制)两段,缺一会空帧。
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| app.reduce(ui.ctx()),
                )
                .drop_without_applying_deltas();
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| app.draw(ui),
                );
                for clipped in &output.shapes {
                    if let egui::epaint::Shape::Text(t) = &clipped.shape {
                        // 首块 H1 的富渲染文本恰为「LaterMD」(代码块正文
                        // 也含这串,必须精确等值);中央/对照列都在 x ≥ 240
                        // (排除侧栏同名词),跳过顶部模式切换条所在带
                        // (y < 40)。
                        if t.galley.text() == "LaterMD" && t.pos.x >= 240.0 && t.pos.y >= 40.0 {
                            heading = Some(egui::Rect::from_min_size(t.pos, t.galley.size()));
                        }
                    }
                }
                output.drop_without_applying_deltas();
            }
            let heading = heading.unwrap_or_else(|| {
                panic!("{mode:?}:首块 H1 必须渲染在列顶(修复前被亚像素误判吃掉)");
            });
            assert!(
                heading.top() < 200.0,
                "{mode:?}:首块应贴列顶,实测 {heading:?}"
            );
        }
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
    /// 立即显示 TextEdit；活动块只能由单击或明确的程序跳转设置。
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
    /// 消失；用户可通过单击其他块或切换模式主动结束当前块。
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
        for (index, range) in live.blocks.clone().into_iter().enumerate() {
            let text = &editor.text()[range];
            live.rich_block_with_base(index, text, None);
        }
        let cached = live
            .rich_cache
            .get(&1)
            .map(|entry| Arc::clone(&entry.1))
            .expect("第二块已建立缓存");

        editor.insert_chars(0, "前缀 ");
        live.sync(&editor, None);

        let second = live.blocks[1].clone();
        let rendered = live.rich_block_with_base(1, &editor.text()[second], None);
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
                visible_bytes: block.len(),
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
                visible_bytes: block.len(),
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
                &AiState::default(),
                &mut cursor,
                &mut selection_mirror,
                &mut live,
                egui::Id::new("live-test"),
                false,
                false,
                true,
                None,
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

    /// 从帧输出抽出全部 CopyText 命令的载荷(本模块测试共用的取证 helper)。
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

    /// 指针序列:移入 → 按下 → 抬起(与 `ui::editor_menu` 测试里的
    /// `click_frames` 同手法三帧;egui 的 click 判定发生在抬起帧)。
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
        /// 表格工具条本帧的图标 rect 清单(最后写入帧)。
        toolbar_rects: Vec<egui::Rect>,
        copied: Vec<String>,
        opened: Vec<String>,
        texts: Vec<(String, egui::Rect)>,
        /// galley 文本 + 全部字形的屏幕坐标(行序拼接,与渲染文本字符序
        /// 对齐):定位「点第二段第二行首字」这类目标,不必模拟字体度量。
        glyphs: Vec<(String, Vec<egui::Pos2>)>,
        /// galley 文本 + (字形字符, 屏幕坐标) 对(行序):按字符找列位用
        /// —— 富渲染 galley 的空段(列表 marker leading/trailing)与换行
        /// 不产字形,字符序号 ≠ 字形序号,必须带 chr 找。
        glyph_chars: Vec<(String, Vec<(char, egui::Pos2)>)>,
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
                    &AiState::default(),
                    cursor,
                    &mut selection_mirror,
                    live,
                    egui::Id::new("live-copy-test"),
                    false,
                    false,
                    true,
                    None,
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
        let mut glyph_chars = Vec::new();
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
                glyph_chars.push((
                    t.galley.text().to_owned(),
                    t.galley
                        .rows
                        .iter()
                        .flat_map(|row| row.glyphs.iter())
                        .map(|glyph| (glyph.chr, t.pos + glyph.pos.to_vec2()))
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
            button_rects: crate::ui::rich_widgets::copy_button_probe(ctx).1,
            toolbar_rects: crate::ui::rich_widgets::table_toolbar_probe(ctx),
            copied,
            opened,
            texts,
            glyphs,
            glyph_chars,
            messages,
            selection_mirror,
        }
    }

    /// 列表项点击保持位置(LP2-5 的全帧守门):生产 `super::ui` 一路渲染,
    /// 浏览态量「列表内容列 − 段落内容列」的富渲染差(= marker 槽位 + gap),
    /// 点击进编辑后再量活动态同一差值 —— 两者必须相等(±0.75px):活动块
    /// 的列表前缀按 vendored 同式对位,不再「往前缩」。对照差取的是**列表
    /// 相对段落的差**,TextEdit 内边距等系统性偏移被基线吸收。
    #[test]
    fn live_list_click_keeps_content_column() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("正文段落。\n\n- 项目甲,点击进入编辑\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();

        // 浏览帧:富渲染的段落与列表项都在 texts/glyphs 里。列表块 galley 的
        // 文本含 marker("• "),首个 CJK 字形 x = 富渲染内容列;段落块同法。
        let browse = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        // 取「首个 CJK 字形」的 x 作内容列:富渲染 galley 文本含 marker
        // ("• " 在内容前,不产 CJK 字形),按字符找列位必须带 chr —— 富渲染
        // 的空段(leading/trailing)与换行不产字形,字符序号 ≠ 字形序号。
        let first_cjk_x = |frame: &LiveFrame, needle: &str| -> Option<f32> {
            frame
                .glyph_chars
                .iter()
                .find(|(text, _)| text.contains(needle))
                .and_then(|(_, pairs)| {
                    pairs
                        .iter()
                        .find(|(chr, _)| ('\u{4E00}'..='\u{9FFF}').contains(chr))
                        .map(|(_, pos)| pos.x)
                })
        };
        let rich_delta = {
            let para_x = first_cjk_x(&browse, "正文段落").expect("段落应有内容字形");
            let item_x = first_cjk_x(&browse, "项目甲").expect("列表项应有内容字形");
            item_x - para_x
        };
        assert!(
            rich_delta > 8.0,
            "富渲染列表内容列应明显缩进于段落(实测 {rich_delta})"
        );

        // 点击列表项内容首字 → 进编辑;稳定帧量活动块的内容列。
        let click_pos = browse
            .glyphs
            .iter()
            .find(|(text, _)| text.contains("项目甲"))
            .and_then(|(_, glyphs)| glyphs.first().copied())
            .expect("列表项内容首字坐标");
        for events in click_events(click_pos) {
            live_frame(
                &ctx,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
        }
        let item_active = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let item_content_x = item_active
            .glyphs
            .iter()
            .find(|(text, _)| text.starts_with("- 项目甲"))
            .and_then(|(text, glyphs)| {
                // 活动块 galley 文本 = 块源码;跳过前缀 "- " 后的首个字形
                let skip = "- ".chars().count();
                glyphs
                    .iter()
                    .skip(skip)
                    .find(|_| true)
                    .or_else(|| glyphs.first())
                    .copied()
                    .map(|pos| {
                        let _ = text;
                        pos.x
                    })
            })
            .expect("活动块应有内容字形");

        // 点击段落 → 段落进编辑,量活动段落的正文列(同一系统性偏移)。
        let para_click = item_active
            .glyphs
            .iter()
            .find(|(text, _)| text.contains("正文段落"))
            .and_then(|(_, glyphs)| glyphs.first().copied())
            .expect("段落内容首字坐标");
        for events in click_events(para_click) {
            live_frame(
                &ctx,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
        }
        let para_active = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let para_content_x = para_active
            .glyphs
            .iter()
            .find(|(text, _)| text.starts_with("正文段落"))
            .and_then(|(_, glyphs)| glyphs.first().copied())
            .map(|pos| pos.x)
            .expect("活动段落应有内容字形");

        let active_delta = item_content_x - para_content_x;
        assert!(
            (active_delta - rich_delta).abs() < 0.75,
            "列表项活动态内容列应与富渲染同位:富渲染差 {rich_delta:.2}px,活动态差 {active_delta:.2}px"
        );
    }

    // —— LP2-7 徽标手柄与快捷菜单 ——

    /// 级别直选:H1→H6、H6→正文、正文→H3;幂等(目标==现状不动);光
    /// 标按前缀差平移;列表项不动。
    #[test]
    fn set_block_heading_level_targets_directly() {
        let mut editor = EditorBuffer::new("# 标题内容\n\n- 列表项\n\n正文段落\n");
        let mut live = LiveState::default();
        live.sync(&editor, None);
        macro_rules! block_text {
            ($i:expr) => {
                BlockBuffer::slice(editor.text(), &live.blocks[$i].clone()).to_owned()
            };
        }

        // H1→H6:前缀改写,光标 +5(4→9)
        set_block_heading_level(&mut editor, &mut live, 0, Some(6), 4);
        live.sync(&editor, None);
        assert_eq!(block_text!(0), "###### 标题内容\n\n");
        assert_eq!(live.pending_caret, Some((0, 9)), "光标随前缀平移");
        // H6→H6:幂等不动(修订号不前进)
        let rev = editor.revision();
        set_block_heading_level(&mut editor, &mut live, 0, Some(6), 9);
        assert_eq!(editor.revision(), rev, "同级别幂等");
        // H6→正文:前缀剥掉,光标 -7(9→2)
        set_block_heading_level(&mut editor, &mut live, 0, None, 9);
        live.sync(&editor, None);
        assert_eq!(block_text!(0), "标题内容\n\n");
        assert_eq!(live.pending_caret, Some((0, 2)));
        // 正文→H3:插入前缀,光标 +4(2→6)
        set_block_heading_level(&mut editor, &mut live, 0, Some(3), 2);
        live.sync(&editor, None);
        assert_eq!(block_text!(0), "### 标题内容\n\n");
        assert_eq!(live.pending_caret, Some((0, 6)));
        assert!(!live.caret_follow, "切换不拽视口");

        // 列表项:不可升级也不可转正文
        let list_index = live
            .block_containing(editor.text().find("列表项").unwrap())
            .unwrap();
        let before = block_text!(list_index);
        let rev = editor.revision();
        set_block_heading_level(&mut editor, &mut live, list_index, Some(1), 0);
        set_block_heading_level(&mut editor, &mut live, list_index, None, 0);
        assert_eq!(editor.revision(), rev, "列表项不动");
        assert_eq!(block_text!(list_index), before);
    }

    /// 全帧:点活动块的圆点手柄 → 弹快捷菜单 → 点「H2」行 → 源码前缀
    /// H1→H2,块保持活动、光标平移。手柄在内容左缘 - 徽标槽 + 3 的
    /// 12×14 芯片内(gutter=true 的帧)。
    #[test]
    fn live_badge_menu_converts_heading_level() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("# 标题一\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();

        // 徽标槽只在写作栏开启(LP2-7):本测试的帧带 gutter=true。
        let run = |events: Vec<egui::Event>,
                   editor: &mut EditorBuffer,
                   preview: &mut PreviewState,
                   cursor: &mut OutlineCursor,
                   live: &mut LiveState|
         -> Vec<(String, egui::Rect)> {
            let mut outbox = Vec::new();
            let mut selection = None;
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
                        &AiState::default(),
                        cursor,
                        &mut selection,
                        live,
                        egui::Id::new("badge-cycle-test"),
                        false,
                        false,
                        true,
                        None,
                        false,
                        true,
                        &crate::keymap::Keymap::builtin(),
                        &mut outbox,
                    );
                },
            );
            let texts: Vec<(String, egui::Rect)> = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Text(t) => Some((
                        t.galley.text().to_owned(),
                        egui::Rect::from_min_size(t.pos, t.galley.size()),
                    )),
                    _ => None,
                })
                .collect();
            output.drop_without_applying_deltas();
            texts
        };

        // 进编辑:直接指定活动块(点进块等价路径已由其它测试覆盖)
        live.active = Some(0);
        live.browse_only = false;
        live.pending_caret = Some((0, 2));
        let _ = run(
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let texts = run(
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let edit_rect = texts
            .iter()
            .find(|(t, _)| t == "# 标题一")
            .map(|(_, r)| *r)
            .expect("活动块 TextEdit 形状(槽内容左缘 = 其左缘)");
        // 手柄芯片中心(12×14,左缘 = 内容左缘 - 槽宽 + 3)
        let chip = egui::pos2(
            edit_rect.left() - crate::live::BADGE_GUTTER + 3.0 + 6.0,
            edit_rect.top() + 1.0 + 7.0,
        );
        for events in click_events(chip) {
            let _ = run(events, &mut editor, &mut preview, &mut cursor, &mut live);
        }
        // 菜单帧:应出现 H1..H6/正文/加粗条目(手柄点击 → Popup 打开)
        let menu_texts = run(
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        for expected in ["H1", "H2", "H6", "正文", "加粗"] {
            assert!(
                menu_texts.iter().any(|(t, _)| t == expected),
                "快捷菜单应含 {expected} 条目"
            );
        }
        // 点「H2」条目:菜单标签 galley 中心
        let h2 = menu_texts
            .iter()
            .find(|(t, _)| t == "H2")
            .map(|(_, r)| r.center())
            .expect("菜单 H2 行");
        for events in click_events(h2) {
            let _ = run(events, &mut editor, &mut preview, &mut cursor, &mut live);
        }
        assert_eq!(editor.text(), "## 标题一\n", "菜单选 H2 后前缀改写");
        assert_eq!(live.active, Some(0), "块保持活动");
        // 转换的光标经 pending_caret 落地:再跑一帧读 TextEdit state
        let _ = run(
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let state = egui::widgets::text_edit::TextEditState::load(
            &ctx,
            egui::Id::new("badge-cycle-test").with(("live-block", 0)),
        )
        .expect("活动块 TextEdit state");
        assert_eq!(
            state.cursor.char_range().map(|r| r.primary.index.0),
            Some(3),
            "光标随前缀平移(2→3)"
        );
    }

    /// H1 点击落点(LP2-5 后续,2026-10-21 用户实测「点到 M 后面、光标落
    /// 在 e」):vendored 命中表给 job 文本序号,标题 `# ` 前缀被剥进样式
    /// —— 不经映射表就差 2 字符。守门:点在富渲染 H1 的「M|D」之间,进
    /// 编辑后光标必须落在源码 `# LaterMD` 的字符 8(D 之前 = M 之后)。
    #[test]
    fn live_heading_click_lands_after_clicked_glyph() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("# LaterMD\n\n正文段落。\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();

        let browse = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        // 富渲染 H1 galley 里找「D」的字形坐标(点 M 与 D 之间 = D 左缘;
        // galley 文本不含 `#` —— 前缀被剥进样式,这正是本修复的根源)
        let click_pos = browse
            .glyph_chars
            .iter()
            .find(|(text, _)| text.contains("LaterMD"))
            .and_then(|(_, pairs)| pairs.iter().find(|(chr, _)| *chr == 'D'))
            .map(|(_, pos)| *pos)
            .expect("H1 富渲染应有 D 字形");
        for events in click_events(click_pos) {
            live_frame(
                &ctx,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
        }
        assert_eq!(live.active, Some(0), "点击应进入 H1 块");
        assert_eq!(
            live.pending_caret,
            Some((0, 8)),
            "光标应落在「M 之后」(D 之前,块内字符 8);旧口径落 6(r)或 5(e)"
        );
    }

    /// 活动块与富渲染**绝对**同 x(LP2-5 收尾):普通段落点击进编辑后,
    /// 首个 CJK 字形的屏幕 x 与富渲染帧相等(±0.6px)—— TextEdit 水平内
    /// 距归零,不再「轻微右移」。列表项同断言。
    #[test]
    fn live_active_block_aligns_absolutely_with_rich_render() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("正文段落。\n\n- 项目甲\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();

        let browse = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let first_cjk_x = |frame: &LiveFrame, needle: &str| -> Option<f32> {
            frame
                .glyph_chars
                .iter()
                .find(|(text, _)| text.contains(needle))
                .and_then(|(_, pairs)| {
                    pairs
                        .iter()
                        .find(|(chr, _)| ('\u{4E00}'..='\u{9FFF}').contains(chr))
                        .map(|(_, pos)| pos.x)
                })
        };
        let rich_para_x = first_cjk_x(&browse, "正文段落").expect("富渲染段落");
        let rich_item_x = first_cjk_x(&browse, "项目甲").expect("富渲染列表项");

        // 点段落 → 活动态首 CJK x == 富渲染段落 x(±0.6px,内距归零)
        let para_click = browse
            .glyph_chars
            .iter()
            .find(|(text, _)| text.contains("正文段落"))
            .and_then(|(_, pairs)| {
                pairs
                    .iter()
                    .find(|(chr, _)| ('\u{4E00}'..='\u{9FFF}').contains(chr))
                    .map(|(_, pos)| *pos)
            })
            .expect("段落点击点");
        for events in click_events(para_click) {
            live_frame(
                &ctx,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
        }
        let para_active = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let active_para_x = first_cjk_x(&para_active, "正文段落").expect("活动段落");
        assert!(
            (active_para_x - rich_para_x).abs() < 0.6,
            "段落活动态首字 x {active_para_x} 应与富渲染 {rich_para_x} 绝对同位(±0.6px)"
        );

        // 点列表项 → 活动态内容首字 x == 富渲染列表项 x
        let item_click = browse
            .glyph_chars
            .iter()
            .find(|(text, _)| text.contains("项目甲"))
            .and_then(|(_, pairs)| {
                pairs
                    .iter()
                    .find(|(chr, _)| ('\u{4E00}'..='\u{9FFF}').contains(chr))
                    .map(|(_, pos)| *pos)
            })
            .expect("列表项点击点");
        for events in click_events(item_click) {
            live_frame(
                &ctx,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
        }
        let item_active = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let active_item_x = first_cjk_x(&item_active, "项目甲").expect("活动列表项");
        assert!(
            (active_item_x - rich_item_x).abs() < 0.6,
            "列表项活动态内容 x {active_item_x} 应与富渲染 {rich_item_x} 绝对同位(±0.6px)"
        );
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
            2,
            "非活动代码块:复制 + 关闭两枚按钮:{:?}",
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

    // —— 块级改写:关闭代码块 / 表格工具条(2026-10-23,用户需求)——

    /// 关闭代码块的改写语义:去围栏留内容,块尾分隔空行原样保留;
    /// 光标落块首。与 [`set_block_heading_level`] 同一改写通道。
    #[test]
    fn close_code_block_strips_fences_keeps_content() {
        let mut editor =
            EditorBuffer::new("# 标题\n\n```rust\nfn a(){}\nlet b = 2;\n```\n\n正文\n");
        let mut live = LiveState::default();
        live.sync(&editor, None);
        assert_eq!(live.blocks.len(), 3);
        close_code_block(&mut editor, &mut live, 1);
        assert_eq!(editor.text(), "# 标题\n\nfn a(){}\nlet b = 2;\n\n正文\n");
        assert_eq!(live.pending_caret, Some((1, 0)), "光标落块首");
    }

    /// 流式未闭合(AI 写入中)与内容行里嵌 ``` 的围栏:关闭点了也不动
    /// (闭合判定只认「标记之外整行空白」的行,防误剥)。
    #[test]
    fn close_code_block_ignores_unclosed_and_embedded_fence() {
        let mut editor = EditorBuffer::new("段落\n\n```rust\nfn a(){}\n");
        let mut live = LiveState::default();
        live.sync(&editor, None);
        let code_index = live.blocks.len() - 1;
        close_code_block(&mut editor, &mut live, code_index);
        assert_eq!(
            editor.text(),
            "段落\n\n```rust\nfn a(){}\n",
            "未闭合围栏不动"
        );

        let mut editor = EditorBuffer::new("```text\nsee ``` below\n```\n\n正文\n");
        let mut live = LiveState::default();
        live.sync(&editor, None);
        close_code_block(&mut editor, &mut live, 0);
        assert_eq!(editor.text(), "see ``` below\n\n正文\n");
    }

    /// 空围栏不被解析器独立成块(实测并入前一段落):合并未遂块上改写
    /// 是保守 no-op —— 没有完整围栏可去,什么都不发生。
    #[test]
    fn close_code_block_ignores_empty_fence_merged_into_paragraph() {
        let mut editor = EditorBuffer::new("标题\n\n```rust\n```\n\n正文\n");
        let mut live = LiveState::default();
        live.sync(&editor, None);
        assert_eq!(live.blocks.len(), 2, "空围栏并入前一块");
        close_code_block(&mut editor, &mut live, 0);
        assert_eq!(editor.text(), "标题\n\n```rust\n```\n\n正文\n", "no-op");
    }

    /// 表格末尾加行:空单元格、列数对齐表头,落在最后一根内容行之后
    /// (块尾分隔之前)。
    #[test]
    fn table_add_row_appends_aligned_empty_row() {
        let mut editor = EditorBuffer::new("| a | b |\n| --- | --- |\n| 1 | 2 |\n\n正文\n");
        let mut live = LiveState::default();
        live.sync(&editor, None);
        table_add_row(&mut editor, &mut live, 0);
        assert_eq!(
            editor.text(),
            "| a | b |\n| --- | --- |\n| 1 | 2 |\n|  |  |\n\n正文\n"
        );
    }

    /// 表格右侧加列:表头/数据行补空单元格,分隔行补 `---`;对齐标记
    /// 之类的既有内容不动。
    #[test]
    fn table_add_col_appends_cell_to_every_row() {
        let mut editor = EditorBuffer::new("| a | b |\n| :--- | ---: |\n| 1 | 2 |\n\n正文\n");
        let mut live = LiveState::default();
        live.sync(&editor, None);
        table_add_col(&mut editor, &mut live, 0);
        assert_eq!(
            editor.text(),
            "| a | b |  |\n| :--- | ---: | --- |\n| 1 | 2 |  |\n\n正文\n"
        );
    }

    /// 非表格块(段落):加了也不动,不产出脏源码。
    #[test]
    fn table_ops_ignore_malformed_blocks() {
        let mut editor = EditorBuffer::new("正文段落\n");
        let mut live = LiveState::default();
        live.sync(&editor, None);
        table_add_row(&mut editor, &mut live, 0);
        table_add_col(&mut editor, &mut live, 0);
        assert_eq!(editor.text(), "正文段落\n", "段落不是表格,原样不动");
    }

    /// 确认动作分派:删表整块移除,前后块之间恰好剩一个空行。
    #[test]
    fn apply_pending_block_action_deletes_table() {
        let mut editor = EditorBuffer::new("标题\n\n| a | b |\n| --- | --- |\n\n正文\n");
        let mut live = LiveState::default();
        live.sync(&editor, None);
        apply_pending_block_action(&mut editor, &mut live, PendingBlockAction::TableDelete(1));
        assert_eq!(editor.text(), "标题\n\n正文\n");
    }

    /// 点头部关闭按钮:挂待确认、文本不动、不切入编辑(关闭与复制同属
    /// 头部按钮语义,点击排除在「整块点击进编辑」之外)。
    #[test]
    fn live_close_button_pends_dialog_without_rewriting() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("# 标题\n\n正文段落。\n\n```rust\nfn a(){}\n```\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();
        let before = editor.text().to_owned();

        let frame = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert_eq!(frame.button_rects.len(), 2, "复制 + 关闭");
        let close_target = frame.button_rects[1].center();
        for events in click_events(close_target) {
            live_frame(
                &ctx,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
        }
        assert_eq!(
            live.pending_block_action,
            Some(PendingBlockAction::CodeClose(2)),
            "关闭点击挂待确认弹窗"
        );
        assert_eq!(editor.text(), before, "挂起帧不得直接改写");
        assert_eq!(live.active, None, "点关闭按钮不得切入编辑态");

        // 确认分派:去围栏留内容(弹窗按钮的点击走 layout 既有测试,
        // 这里直驱分派函数)
        apply_pending_block_action(&mut editor, &mut live, PendingBlockAction::CodeClose(2));
        assert_eq!(editor.text(), "# 标题\n\n正文段落。\n\nfn a(){}\n");
    }

    /// 表格行解析:单元格文本(trim 后)与块内字节区间;分隔行无单元格。
    #[test]
    fn table_rows_parses_cell_ranges() {
        let rows = table_rows("| a | b |\n| --- | --- |\n| 1 | 2 |").unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].header);
        assert_eq!(rows[0].cells[0].0, "a");
        assert_eq!(rows[0].cells[1].0, "b");
        assert_eq!(rows[0].cells[1].1, 5..8, "单元格区间 = 管道之间的原始文本");
        assert!(rows[1].separator);
        assert_eq!(rows[2].cells[0].0, "1");
    }

    /// 活动表格:pending_caret 落进单元格 → 聚焦该单元格 → 键入直接
    /// splice 进共享缓冲的对应单元格(原样编辑,不塌成整块源码)。
    #[test]
    fn active_table_cell_edit_splices_source() {
        let ctx = egui::Context::default();
        let mut editor =
            EditorBuffer::new("# 标题\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n正文\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState {
            active: Some(1),
            ..LiveState::default()
        };
        // 落点 = 表头行 " b " 单元格内末尾(块内字符偏移 6 是 'b')
        live.pending_caret = Some((1, 6));

        let frame = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert!(frame.toolbar_rects.is_empty(), "活动表格不出 titlebar");
        assert_eq!(live.pending_caret, None, "落点已消费");

        // 聚焦单元格键入:splice 进 " b " 单元格,其余行列原样
        live_frame(
            &ctx,
            vec![egui::Event::Text("X".to_owned())],
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        // 原文 " b " 末尾插入 X → " b X"(尾随空格保留在 X 前)
        assert!(
            editor.text().contains("| a | b X|"),
            "键入落进 b 单元格:{:?}",
            editor.text()
        );
        assert!(
            editor.text().contains("| 1 | 2 |"),
            "其他行列不受影响:{:?}",
            editor.text()
        );
        assert_eq!(live.active, Some(1), "编辑中块保持活动");
    }

    /// 活动表格回到富渲染:焦点离开后单元格文本如实渲染(编辑生效)。
    #[test]
    fn table_rows_roundtrip_after_cell_edit() {
        let rows = table_rows("| a | b X |\n| --- | --- |").unwrap();
        assert_eq!(rows[0].cells[1].0, "b X");
    }

    #[test]
    fn table_rows_use_block_byte_ranges_for_unicode_and_crlf() {
        let block = "| 中文 | 😀 |\r\n| --- | --- |\r\n| 长 | xxxxx |";
        let rows = table_rows(block).unwrap();
        assert_eq!(rows[0].cells[0].1, 1..9);
        assert_eq!(rows[0].cells[1].1, 10..16);
        assert_eq!(&block[rows[2].cells[1].1.clone()], " xxxxx ");
    }

    /// titlebar 宽度跟表格实际宽:宽屏下四枚图标贴在表格宽度附近,
    /// 不铺满整行(悬浮/铺满两种旧形态的回归守门)。表格逐单元格出
    /// 文本形状,拿不到整表矩形 —— 参照系用显式宽屏:旧形态的图标
    /// 会贴到视口右缘(~1100+),新形态贴表格实测宽(窄表 ~200 内)。
    #[test]
    fn table_titlebar_width_follows_table() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("标题\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n正文\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();
        let mut outbox = Vec::new();
        let mut selection = None;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |panel| {
                super::ui(
                    panel,
                    &mut editor,
                    &mut preview,
                    &AiState::default(),
                    &mut cursor,
                    &mut selection,
                    &mut live,
                    egui::Id::new("live-titlebar-width"),
                    false,
                    false,
                    true,
                    None,
                    false,
                    false,
                    &crate::keymap::Keymap::builtin(),
                    &mut outbox,
                );
            },
        );
        output.drop_without_applying_deltas();
        // 帧号已前进,按「最后写入者即本帧」取原始探针
        let rects = crate::ui::rich_widgets::table_toolbar_probe(&ctx);
        assert_eq!(rects.len(), 4, "四枚图标:{rects:?}");
        let icons_right = rects[3].right();
        assert!(
            icons_right < 400.0,
            "titlebar 贴表格实测宽,不铺满 1200 宽视口:{icons_right}"
        );
    }

    /// 表格 titlebar(常驻,代码块头部同款):静帧即在场,四枚图标贴右;
    /// 点「加行」落一行;titlebar 图标点击不切入编辑。
    #[test]
    fn live_table_titlebar_shows_and_add_row_works() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("标题\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n正文\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();

        // 静帧:titlebar 常驻,四枚图标在场(悬浮条形态被否,改常驻 titlebar)
        let frame = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert_eq!(
            frame.toolbar_rects.len(),
            4,
            "加行/加列/复制/删除四枚图标:{:?}",
            frame.toolbar_rects
        );
        // 四枚图标同一行(titlebar 带内),不是浮在远处
        let bar_top = frame.toolbar_rects[0].top();
        for rect in &frame.toolbar_rects[1..] {
            assert!((rect.top() - bar_top).abs() < 1.0, "图标同一行:{rect:?}");
        }

        // 点「加行」(首枚):表格末尾落一行;不切入编辑态。
        let add_row = frame.toolbar_rects[0].center();
        for events in click_events(add_row) {
            live_frame(
                &ctx,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
        }
        assert!(
            editor.text().contains("| 1 | 2 |\n|  |  |"),
            "点加行必须落一行:{:?}",
            editor.text()
        );
        assert_eq!(live.active, None, "点工具条不得切入编辑态");
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
        assert_eq!(frame.button_rects.len(), 2);

        // 点击代码正文(galley rect 中心):离右上角复制/关闭按钮足够远。
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

    /// 浏览态**单击**即进编辑(2026-10-19 拍板,取代早期「双击才进写作」):
    /// 打开文档后第一下点击就落在点击字形上,不再要求先双击唤醒 —— 点哪
    /// 编哪是 MarkText 手感的底线。禅定锁定仍由 browse_enabled 关死入口
    /// (锁定测试另在 zen 侧钉住)。
    #[test]
    fn browse_single_click_enters_edit_at_clicked_glyph() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("第一段\n\n第二段\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState {
            browse_only: true,
            browse_enabled: true,
            ..LiveState::default()
        };

        // 浏览态首帧:无活动块,不抢焦点
        let frame = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert!(live.active.is_none(), "浏览态首帧无活动块");
        let (_, glyphs) = frame
            .glyphs
            .iter()
            .find(|(text, _)| text.trim() == "第二段")
            .expect("第二段已富渲染");
        let target = glyphs[0] + egui::vec2(1.0, 0.0);

        // 单击三帧(移入/按下/抬起):第二段在点击帧直接进编辑,光标落点击字形
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
        assert!(!live.browse_only, "单击退出浏览态");
        assert_eq!(live.active, Some(1), "单击切入编辑态:{live:?}");
        assert_eq!(
            live.pending_caret,
            Some((1, 0)),
            "光标落点击字形(第二段首)而非块尾:{:?}",
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
        let rects = crate::ui::rich_widgets::task_checkbox_probe(&ctx);
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
        let (block_rendered, _, _, _, _, _) =
            live.rich_block_with_base(0, BlockBuffer::slice(editor.text(), &live.blocks[0]), None);
        assert!(
            block_rendered.contains("task://c"),
            "切换后块改写即时反映勾选态:{block_rendered}"
        );
    }

    /// #51 M3:Live 富渲染块里的 ```mermaid 围栏出图(生产入口 `ui` 经
    /// `RichLinkHandler` 的 mermaid 臂接入);解析失败的围栏回落源码(探针
    /// 不出节点),其余块(代码块复制按钮等)不受牵连。
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

    /// 本地文档链接在 Live 富渲染块点击:拦下发 [`Message::DocLinkClicked`]
    /// (应用内跳转,`state::open_doc_link` 消费),不交系统浏览器 ——
    /// 相对路径交给浏览器只会失败。块编辑状态同样不被切换。
    #[test]
    fn live_local_doc_link_click_emits_doc_link_message() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("[跳转](./笔记.md)\n\n后续。\n");
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
            .find(|(text, _)| text.contains("跳转"))
            .expect("本地链接文本已渲染");

        let mut messages = Vec::new();
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
            messages.extend(frame.messages);
            opened.extend(frame.opened);
        }
        assert_eq!(
            messages,
            vec![Message::DocLinkClicked {
                href: "./笔记.md".to_owned()
            }],
            "本地链接点击发应用内跳转消息"
        );
        assert!(opened.is_empty(), "不走系统浏览器:{opened:?}");
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
                    &AiState::default(),
                    cursor,
                    &mut selection_mirror,
                    live,
                    egui::Id::new(FADE_EDITOR_ID),
                    false,
                    false,
                    true,
                    None,
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

    /// 滚动测试文档:连续的长普通段落,由**软换行自动折行**产生至少
    /// `count` 个视觉行;末尾再放一个独立尾块。普通 Markdown 的源码换行
    /// 现在按空白布局,测试夹具不能再用 100 个 `\n` 假装视觉行。
    fn scroll_doc_lines(count: usize) -> String {
        let mut text = String::new();
        let filler = "这是一段用于滚动导航的连续普通文本,用于产生稳定的软折行视觉行。";
        for i in 0..count {
            text.push_str(&format!("行 {i:03},{}\n", filler.repeat(20)));
        }
        text.push_str("\n\n尾块\n");
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
        id: egui::Id,
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
        let mut response_id = egui::Id::NULL;
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
                response_id = super::ui(
                    ui,
                    editor,
                    preview,
                    &AiState::default(),
                    cursor,
                    &mut selection_mirror,
                    live,
                    id,
                    show_typewriter,
                    show_focus,
                    true,
                    None,
                    false,
                    false,
                    &crate::keymap::Keymap::builtin(),
                    &mut outbox,
                )
                .id;
            },
        );
        output.drop_without_applying_deltas();
        ctx.read_response(response_id).map(|response| response.rect)
    }

    /// 光标行的屏幕 y:活动态普通正文按 Markdown 软换行排版,不能再用
    /// `\n` 数量估行。测试探针用稳定的字符密度近似视觉行数,只用于
    /// 断言「滚入视口」这类宽容几何契约;生产滚动仍读取真实 Galley。
    fn caret_screen_y(rect: egui::Rect, block_text: &str, caret: usize) -> f32 {
        const APPROX_CHARS_PER_ROW: usize = 80;
        let chars = block_text.chars().count().max(1);
        let rows = (chars.div_ceil(APPROX_CHARS_PER_ROW)).max(1) as f32;
        let line = (caret.min(chars) / APPROX_CHARS_PER_ROW) as f32;
        rect.top() + line.min(rows - 1.0) * (rect.height() / rows)
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
            0.0,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        );

        // ↑ 在块首 → 路由到段落块可见末尾(幻影行之前的最后一个字符;
        // visible_char_len 语义,方向键不再先穿过块尾的空行幻影行)
        let _ = live_scroll_frame(
            &ctx,
            egui::Id::new(SCROLL_EDITOR_ID),
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
        let visible_end = super::visible_char_len(&editor.text()[live.blocks[0].clone()]);
        assert_eq!(
            live.pending_caret,
            Some((0, visible_end)),
            "交接点 = 上一块可见末尾"
        );

        // 落地帧:光标写进块 0 持久 state + 同帧请求滚动 + 焦点转移
        let _ = live_scroll_frame(
            &ctx,
            egui::Id::new(SCROLL_EDITOR_ID),
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
        assert_eq!(caret, visible_end, "光标落在上一块可见末尾");
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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

    /// 跨块路由的四个方向与 Shift 守卫(2026-10-19 写作模式去隔离):
    /// 块首 ↑/← 去上一块**可见末尾**;可见末尾 ↓/→ **一次按键**即去下一块
    /// 开头(块尾幻影行不再是必经站,键盘不再发粘);按住 Shift 一律不路由
    /// —— 选区扩展不能跨块(选区活在单个 TextEdit 的 state 里),宁可原地
    /// 不动也不打断用户正在拉的选择。
    #[test]
    fn live_arrow_keys_route_across_blocks_with_shift_guard() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("甲段\n\n乙段\n\n丙段\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState {
            active: Some(1),
            ..LiveState::default()
        };
        let id = scroll_block_id(1);
        let editor_id = egui::Id::new(SCROLL_EDITOR_ID);
        let inject = |ctx: &egui::Context, caret: usize| {
            let mut st = egui::widgets::text_edit::TextEditState::default();
            st.cursor.set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(caret),
            )));
            st.store(ctx, id);
            ctx.memory_mut(|mem| mem.request_focus(id));
        };
        let shift_key = |key: egui::Key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                shift: true,
                ..Default::default()
            },
        };
        let mut tick = 0.0;
        macro_rules! frame {
            ($events:expr) => {{
                tick += 0.1;
                live_scroll_frame(
                    &ctx,
                    editor_id,
                    tick,
                    $events,
                    &mut editor,
                    &mut preview,
                    &mut cursor,
                    &mut live,
                    false,
                    false,
                )
            }};
        }

        // ← 在块首 → 上一块可见末尾
        inject(&ctx, 0);
        let _ = frame!(Vec::new());
        let visible_0 = super::visible_char_len(&editor.text()[live.blocks[0].clone()]);
        assert_eq!(visible_0, 2, "甲段块的可见末尾(两个幻影换行之前)");
        let _ = frame!(vec![arrow(egui::Key::ArrowLeft)]);
        assert_eq!(
            live.pending_caret,
            Some((0, visible_0)),
            "← 块首路由到上一块可见末尾"
        );
        let _ = frame!(Vec::new());
        assert_eq!(scroll_block_caret(&ctx, 0), Some(visible_0), "落进上一块");

        // → 在可见末尾 → 下一块开头(不先穿幻影行)
        let _ = frame!(vec![arrow(egui::Key::ArrowRight)]);
        assert_eq!(
            live.pending_caret,
            Some((1, 0)),
            "→ 可见末尾路由到下一块开头"
        );
        let _ = frame!(Vec::new());
        assert_eq!(scroll_block_caret(&ctx, 1), Some(0), "落进下一块");

        // ↓ 在可见末尾 → 下一块开头:一次按键直达,不再「先落到幻影空行、
        // 再按一次才出去」的粘滞
        inject(&ctx, visible_0);
        let _ = frame!(Vec::new());
        let _ = frame!(vec![arrow(egui::Key::ArrowDown)]);
        assert_eq!(
            live.pending_caret,
            Some((2, 0)),
            "↓ 可见末尾路由到下一块开头"
        );
        let _ = frame!(Vec::new());
        assert_eq!(scroll_block_caret(&ctx, 2), Some(0), "落进块 2");

        // Shift 守卫:块首 Shift+← 不路由(选区扩展保持原状)
        live.active = Some(1);
        inject(&ctx, 0);
        let _ = frame!(Vec::new());
        // 真机序列:winit 先发 ModifiersChanged 再发按键(headless 的 Key
        // 事件自带 modifiers 不进 input.modifiers,egui 只认前者)
        let shift_mods = egui::Modifiers {
            shift: true,
            ..Default::default()
        };
        let _ = frame!(vec![
            egui::Event::ModifiersChanged(shift_mods),
            shift_key(egui::Key::ArrowLeft),
        ]);
        assert_eq!(live.pending_caret, None, "Shift 按住时不路由");
        assert_eq!(live.active, Some(1), "活动块原地不动");
    }

    /// 块切换视口锚定(2026-10-19 写作模式防跳变):点击富渲染块进编辑时,
    /// 块从富渲染切到源码形态高度必变,不补偿则视口内内容整体平移 ——
    /// 「点哪跳哪」。契约:点击帧按预测把 ScrollArea 偏移先行修正,落地帧
    /// 按实测光标行残差校正,终态 = 源码光标行的屏幕位置钉在被点击渲染行
    /// 的位置上。场景同时压住两个位移源:上一活动块(标题)塌缩的位移 +
    /// 目标块自身切换的高度差;目标点取长段落的第二行,行上方的行高差给
    /// 位移一个稳定量(单行段两个位移源会互相抵消成 ~0,测不出补偿)。
    #[test]
    fn live_click_anchor_keeps_clicked_row_stationary() {
        let ctx = egui::Context::default();
        ctx.style_mut_of(egui::Theme::Dark, |style| {
            style.scroll_animation = egui::style::ScrollAnimation::none();
        });
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        // 目标段两侧用代码 fence 断开 flush:它的文本 galley 独立成形状
        // (段落会被合并进同一 galley,形状级 rect 拿不到中间段落的行)。
        // 点击目标 = 文档中段的「靶段」(长段落,软折多行),前后用 fence
        // 断开 flush 让它的 galley 独立成形状。整个场景先滚轮下滚把靶段
        // 送进视口中部再点击:文档顶是钳位边界,顶部场景里向上补偿的位移
        // 会被 clamp 吃掉 —— 中段点击才是锚定的主战场。
        let filler = "乙段:这一段只是填充正文,".repeat(6);
        let target_para = "靶段:这一段只是被点击的正文,".repeat(5);
        // 塌缩位移源 = H1 下的代码 fence(块 1):内距归零(LP2-6)后普通
        // 段落与标题的源码↔富渲染高度已相等(补偿量归零是改善不是回归),
        // 唯代码块富渲染带复制头/内距,与源码态保持真实高度差 —— 补偿场
        // 景用它当位移源。
        let mut text =
            format!("# 标题\n\n```text\n filler \n```\n\n{filler}\n\n```text\n filler \n```\n");
        for i in 0..10 {
            text.push_str(&format!("\n中段{i:02},这一行只是垫开距离。\n"));
        }
        text.push_str(&format!(
            "\n```text\n filler \n```\n\n{target_para}\n\n```text\n filler \n```\n"
        ));
        for i in 0..30 {
            text.push_str(&format!("\n尾段{i:02},这一行只是垫高内容。\n"));
        }
        let id = egui::Id::new("live-anchor-click");
        let mut editor = EditorBuffer::new(&text);
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        // 活动块 = 代码 fence(块 1,源码形态):塌缩位移源,见文档注释
        let mut live = LiveState {
            active: Some(1),
            ..LiveState::default()
        };
        live.pending_caret = Some((1, 0));
        let mut outbox = Vec::new();
        let mut selection = None;
        // 返回 (全部文本形状 rect, 滚动探针偏移, live::ui 响应矩形 = 活动
        // 块 TextEdit 矩形, 行几何探针命中行顶)。行几何用 vendored
        // `row_rect_at_pos` 在帧内取证 —— epaint 字形 y 是行内局部坐标,
        // 从形状/字形反推行顶不可靠,而它正是生产锚定用的同一查询。
        let row_probe = std::cell::Cell::new(false);
        let probe_block = std::cell::Cell::new(0usize);
        /// 单帧取证:文本形状 rect、滚动探针偏移、活动块 TextEdit 矩形、
        /// 行几何探针命中行顶。
        struct AnchorFrame {
            texts: Vec<(String, egui::Rect)>,
            offset: f32,
            active_rect: Option<egui::Rect>,
            row_top: Option<f32>,
        }
        let mut run = |time: f64,
                       events: Vec<egui::Event>,
                       editor: &mut EditorBuffer,
                       preview: &mut PreviewState,
                       cursor: &mut OutlineCursor,
                       live: &mut LiveState|
         -> AnchorFrame {
            let mut texts = Vec::new();
            let mut response_id = egui::Id::NULL;
            let mut row_top = None;
            let output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |panel| {
                    response_id = super::ui(
                        panel,
                        editor,
                        preview,
                        &AiState::default(),
                        cursor,
                        &mut selection,
                        live,
                        id,
                        false,
                        false,
                        true,
                        None,
                        false,
                        false,
                        &crate::keymap::Keymap::builtin(),
                        &mut outbox,
                    )
                    .id;
                    if row_probe.get() {
                        let probe_id = id.with(("live-render", probe_block.get()));
                        if let Some(block_rect) = egui_markdown::block_span_rects(panel, probe_id)
                            .and_then(|blocks| blocks.first().map(|b| b.rect))
                        {
                            // 长段落 0.75 块高落在第二视觉行内;x 取行内 40px
                            // —— 行首 3px 会点中**软换行边界字符**(行 N 末尾
                            // = 行 N+1 开头),egui 对边界光标画在上一行行尾,
                            // 与富渲染「下一行开头」天然差一行,锚定契约的
                            // 「点哪行钉哪行」对边界字符不成立
                            let pos = egui::pos2(
                                block_rect.min.x + 40.0,
                                block_rect.min.y + block_rect.height() * 0.75,
                            );
                            row_top = egui_markdown::row_rect_at_pos(panel, probe_id, pos)
                                .map(|row| row.min.y);
                        }
                    }
                },
            );
            for clipped in &output.shapes {
                if let egui::Shape::Text(t) = &clipped.shape {
                    texts.push((
                        t.galley.text().to_owned(),
                        egui::Rect::from_min_size(t.pos, t.galley.size()),
                    ));
                }
            }
            let offset = ctx
                .data(|d| d.get_temp::<LiveScrollProbe>(super::scroll_probe_id(id)))
                .map(|probe| probe.offset)
                .unwrap_or(0.0);
            output.drop_without_applying_deltas();
            AnchorFrame {
                texts,
                offset,
                active_rect: ctx.read_response(response_id).map(|r| r.rect),
                row_top,
            }
        };

        // 稳态:活动块 0(标题源码)已就位,live-src-h 已记账;滚轮把靶段
        // 送进视口中部(负 delta = 向下滚),再从它的形状矩形推出第二行的
        // 点击点,行探针取回那一行的精确屏幕顶
        let target_block = {
            let _ = run(
                0.0,
                Vec::new(),
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
            let _ = run(
                0.1,
                Vec::new(),
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
            let idx = live
                .blocks
                .iter()
                .position(|range| editor.text()[range.clone()].contains("靶段"))
                .expect("靶段成块");
            probe_block.set(idx);
            idx
        };
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -120.0),
            phase: egui::TouchPhase::Move,
            modifiers: Default::default(),
        };
        // 滚轮把靶段送进视口(负 delta = 向下滚),滚到中部就停:滚轮帧的
        // 形状 rect 是上一帧几何(滚轮当帧的滚动下一帧才进布局),必须在
        // 停稳后的空转帧里同几何取证
        let mut settled = false;
        for i in 0..12 {
            let frame = run(
                0.2 + f64::from(i) * 0.1,
                vec![
                    egui::Event::PointerMoved(egui::pos2(400.0, 300.0)),
                    wheel.clone(),
                ],
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
            if let Some(rect) = frame
                .texts
                .iter()
                .find(|(t, _)| t.contains("靶段"))
                .map(|(_, r)| *r)
            {
                if rect.min.y > 200.0 && rect.max.y < 420.0 {
                    settled = true;
                    break;
                }
            }
        }
        assert!(settled, "滚轮把靶段送进视口中部");
        // 空转到几何稳定:块间有估高沉降(#111 B 的占位/实测交替会把内容
        // 逐帧微移),滚动刚停时块还会漂几帧 —— 循环空转,直到靶段的形状
        // rect 连续两帧不动,再取点击点。行探针同帧自洽取证(块左缘 +
        // 3px、0.75 块高落在第二视觉行内),点击点用同一稳定 rect 推出。
        let mut offset_before = 0.0;
        let mut target = None;
        let mut clicked_row_top = None;
        let mut prev_min_y = f32::NAN;
        let mut tick = 1.4;
        for _ in 0..20 {
            row_probe.set(true);
            let frame = run(
                tick,
                Vec::new(),
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
            );
            row_probe.set(false);
            tick += 0.1;
            offset_before = frame.offset;
            let rect = frame
                .texts
                .iter()
                .find(|(t, _)| t.contains("靶段"))
                .expect("靶段独立成形状")
                .1;
            if frame.row_top.is_some() && (rect.min.y - prev_min_y).abs() <= 0.01 {
                clicked_row_top = frame.row_top;
                // 与行探针同 x:行内 40px,避开软换行边界字符(见行探针注释)
                target = Some(egui::pos2(
                    rect.min.x + 40.0,
                    rect.min.y + rect.height() * 0.75,
                ));
                break;
            }
            prev_min_y = rect.min.y;
        }
        let target = target.expect("几何在 20 帧内稳定");
        let clicked_row_top = clicked_row_top.expect("行探针命中被点击行");
        assert!(
            offset_before > 50.0,
            "已离开文档顶,向上补偿不被钳位:{offset_before}"
        );

        // 点击三帧(移入/按下/抬起):第三帧 activate + 预测偏移落账
        let click = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let _ = run(
            tick,
            vec![egui::Event::PointerMoved(target)],
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        tick += 0.1;
        let _ = run(
            tick,
            vec![click(target, true)],
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        tick += 0.1;
        let _ = run(
            tick,
            vec![click(target, false)],
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        tick += 0.1;
        assert_eq!(live.active, Some(target_block), "点击切入目标块:{live:?}");
        assert!(
            live.toggle_anchor.is_some(),
            "点击行几何已捕获、锚定计划已挂载:{live:?}"
        );
        // 位移在点击帧末写进 ScrollArea state,落地帧的闭包才按新偏移布局
        // —— 探针口径读落地帧
        let offset_after_shift = run(
            tick,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        )
        .offset;
        tick += 0.1;
        assert!(
            (offset_after_shift - offset_before).abs() > 1.0,
            "锚定补偿真的动了偏移(前 {offset_before} 后 {offset_after_shift})"
        );

        // 残差校正帧:偏移经「预测 + 残差」两次修正后,点击字符所在的
        // 源码行与点击行顶对齐。终态量法:TextEdit 矩形顶(帧内响应)+
        // 该块同参 shaping 的光标行顶(与生产共用 predict_source_layout)
        // == 被点击渲染行顶,容差 2px。
        tick += 0.1;
        let measure_frame = run(
            tick,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let active_rect = measure_frame.active_rect.expect("活动块 TextEdit 响应可读");
        // 点击字符在源码里的行顶(块内偏移 → 同参 shaping 预测的 galley
        // 行顶),加上 TextEdit 矩形顶即它的屏幕位置;预测与生产共用同一
        // 函数,差异只剩布局实测 vs 理论 —— 残差校正正是为此存在,终态
        // 容差 2px。
        let caret_char = {
            let state = egui::widgets::text_edit::TextEditState::load(
                &ctx,
                id.with(("live-block", target_block)),
            )
            .expect("目标块 TextEdit state 已建立");
            state.cursor.char_range().expect("落地光标").primary.index.0
        };
        let mut src_caret_top_rel = 0.0f32;
        let probe_out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                let block_text = &editor.text()[live.blocks[target_block].clone()];
                let view_bytes = block_text.trim_end_matches('\n').len();
                let (_h, caret_top) =
                    super::predict_source_layout(ui, &block_text[..view_bytes], caret_char);
                src_caret_top_rel = caret_top;
            },
        );
        probe_out.drop_without_applying_deltas();
        let caret_row_screen_top = active_rect.top() + src_caret_top_rel;
        assert!(
            (caret_row_screen_top - clicked_row_top).abs() <= 2.0,
            "被点击行钉在原地:源码光标行顶 {caret_row_screen_top} vs 点击行顶 {clicked_row_top}"
        );
    }

    /// 活动块形态切换零展开(#157):幻影行裁剪 + 行距同源后,无行内标记
    /// 的普通段落富渲染 ↔ 源码 TextEdit **块高几乎不变**(残差只剩
    /// TextEdit 上下内距 4px 与整像素圆整)—— 点击进块的「展开感」来自
    /// 块高突变,本测试把它钉死在常数级。
    #[test]
    fn live_plain_paragraph_toggle_keeps_block_height() {
        let ctx = egui::Context::default();
        let mut text = String::from("第一段,这一段只是普通正文。\n\n");
        for i in 0..20 {
            text.push_str(&format!("垫段{i:02},这一行只是垫高内容。\n\n"));
        }
        let mut editor = EditorBuffer::new(&text);
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();
        let id = egui::Id::new("live-zero-expand");

        // 无活动块帧:块 0 富渲染,实测高记在 live-render-h
        let _ = live_scroll_frame(
            &ctx,
            id,
            0.0,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        );
        let rich_h = ctx
            .data(|d| d.get_temp::<f32>(id.with(("live-render-h", 0))))
            .expect("富渲染实测高已记账");

        // 激活落地帧:块 0 源码 TextEdit,实测高记在 live-src-h
        live.active = Some(0);
        live.pending_caret = Some((0, 0));
        let _ = live_scroll_frame(
            &ctx,
            id,
            0.1,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
            false,
            false,
        );
        let src_h = ctx
            .data(|d| d.get_temp::<f32>(id.with(("live-src-h", 0))))
            .expect("源码实测高已记账");
        assert!(
            (src_h - rich_h).abs() <= 5.0,
            "普通段落切形态块高不变:富渲染 {rich_h}px vs 源码 {src_h}px(残差应只剩内距常数)"
        );
    }

    /// 块尾 Enter 的新行必须**可见可写**(幻影行裁剪的光标延伸规则):
    /// 裁剪把块尾空行藏起来,但 Enter 在块尾新建的行恰在幻影区 —— 光标
    /// 所在行延伸可见,接着敲的字落在新行上(新段落),而不是被吞回块尾。
    #[test]
    fn live_enter_at_block_end_reveals_new_line() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("第一段\n\n第二段\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState {
            active: Some(0),
            ..LiveState::default()
        };
        let id = egui::Id::new("live-enter-visible").with(("live-block", 0));
        let mut st = egui::widgets::text_edit::TextEditState::default();
        st.cursor.set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(3),
        )));
        st.store(&ctx, id);
        ctx.memory_mut(|mem| mem.request_focus(id));
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        let mut frame = |time: f64, events: Vec<egui::Event>| {
            live_scroll_frame(
                &ctx,
                egui::Id::new("live-enter-visible"),
                time,
                events,
                &mut editor,
                &mut preview,
                &mut cursor,
                &mut live,
                false,
                false,
            )
        };
        let _ = frame(0.0, Vec::new());
        let _ = frame(0.1, vec![enter]);
        let _ = frame(0.2, Vec::new());
        let caret = egui::widgets::text_edit::TextEditState::load(&ctx, id)
            .and_then(|state| state.cursor.char_range().map(|range| range.primary.index.0));
        assert_eq!(caret, Some(4), "光标落在新行(块尾 Enter)");

        // 新行敲字:落在光标处(第一个换行之后),新段落成立
        let _ = frame(0.3, vec![egui::Event::Text("新".into())]);
        // 段落一 + 新行「新」 + 空行 + 段落二:Enter 插入的换行成为新行的
        // 行尾,原有块尾空行保留,新字落在光标行
        assert_eq!(
            editor.text(),
            "第一段\n新\n\n第二段\n",
            "块尾 Enter 后的新行可写,新字不被吞回块尾"
        );
    }

    /// 渲染形态原地编辑(#158):标题块激活后,编辑视图的标题行与富渲染
    /// **同字体同字号同行盒**(形状高度差 ≤2px = 整像素圆整),块顶挂
    /// H1 徽标;普通段落不挂徽标。文本与缓冲不变 —— 「点进去还是那个
    /// 标题,只是多了个类型标签」。
    #[test]
    fn live_heading_edit_view_matches_render_and_shows_badge() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("# 标题甲乙丙\n\n正文段。\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState::default();

        // 浏览态帧:富渲染标题(纯文本形状,无 # 前缀、无徽标)
        let frame = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let rich_rect = frame
            .texts
            .iter()
            .find(|(t, _)| t.contains("标题甲乙丙"))
            .expect("富渲染标题成形状")
            .1;
        assert!(!frame.texts.iter().any(|(t, _)| t == "H1"), "浏览态无徽标");

        // 激活标题块:编辑视图标题行与富渲染同高;H1 徽标出现
        live.active = Some(0);
        live.pending_caret = Some((0, 0));
        let frame = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        let edit_rect = frame
            .texts
            .iter()
            .find(|(t, _)| t.contains("标题甲乙丙"))
            .expect("编辑视图标题成形状")
            .1;
        assert!(
            (edit_rect.height() - rich_rect.height()).abs() <= 2.0,
            "标题编辑行与渲染行同高:编辑 {:.1}px vs 渲染 {:.1}px",
            edit_rect.height(),
            rich_rect.height()
        );
        // 徽标已是圆点手柄(LP2-7,无文字芯片);其交互由
        // live_badge_menu_converts_heading_level 全帧覆盖。

        // 普通段落激活:同样只有圆点手柄,无块型文字芯片
        live.active = Some(1);
        live.pending_caret = Some((1, 0));
        let frame = live_frame(
            &ctx,
            Vec::new(),
            &mut editor,
            &mut preview,
            &mut cursor,
            &mut live,
        );
        assert!(
            !frame
                .texts
                .iter()
                .any(|(t, _)| matches!(t.as_str(), "H1" | "H2" | "代码" | "引用" | "表格")),
            "手柄无文字芯片(类型改在菜单里显示)"
        );
    }

    /// ATX 前缀可见且可编辑(#159):①前缀段是幽灵色(文本色低透明度,
    /// 不是弱色小字);②光标放块首敲 `#`,标题升 H2 —— 文本真实改写,
    /// 徽标跟成 H2。改级别 = 直接编辑前缀,不需要额外交互。
    #[test]
    fn live_heading_marker_ghost_visible_and_level_editable() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("# 标题甲乙丙\n\n正文段。\n");
        let mut preview = PreviewState::new(&editor);
        let mut cursor = OutlineCursor::default();
        let mut live = LiveState {
            active: Some(0),
            ..LiveState::default()
        };
        let id = egui::Id::new("live-marker-edit").with(("live-block", 0));
        let mut st = egui::widgets::text_edit::TextEditState::default();
        st.cursor.set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(0),
        )));
        st.store(&ctx, id);
        ctx.memory_mut(|mem| mem.request_focus(id));
        let mut frame = |time: f64, events: Vec<egui::Event>| {
            let mut outbox = Vec::new();
            let mut selection = None;
            let output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |panel| {
                    super::ui(
                        panel,
                        &mut editor,
                        &mut preview,
                        &AiState::default(),
                        &mut cursor,
                        &mut selection,
                        &mut live,
                        egui::Id::new("live-marker-edit"),
                        false,
                        false,
                        true,
                        None,
                        false,
                        false,
                        &crate::keymap::Keymap::builtin(),
                        &mut outbox,
                    );
                },
            );
            let texts: Vec<(String, egui::Color32)> = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(t) => Some((
                        t.galley.text().to_owned(),
                        t.galley
                            .job
                            .sections
                            .first()
                            .map(|s| s.format.color)
                            .unwrap_or_default(),
                    )),
                    _ => None,
                })
                .collect();
            output.drop_without_applying_deltas();
            texts
        };

        // 稳态:标题编辑视图渲染,前缀段为幽灵色(低透明度文本色)
        let _ = frame(0.0, Vec::new());
        let texts = frame(0.1, Vec::new());
        let marker_color = texts
            .iter()
            .find(|(t, _)| t.starts_with('#'))
            .map(|(_, color)| *color)
            .expect("标题编辑视图含 # 前缀");
        assert!(
            (60..200).contains(&marker_color.a()),
            "前缀是幽灵色(文本色降透明度),不是弱色实心/全文色:{marker_color:?}"
        );

        // 块首敲 #:升 H2 —— 文本真实改写(galley 文本即编辑缓冲切片)
        let _ = frame(0.2, vec![egui::Event::Text("#".into())]);
        let texts = frame(0.3, Vec::new());
        // 编辑视图文本以 "## " 开头 = 源码真实改写;手柄无文字芯片
        // (LP2-7),级别即源码前缀本身
        assert!(
            texts.iter().any(|(t, _)| t.starts_with("## 标题甲乙丙")),
            "块首敲 # 真实改写源码:编辑视图应含 ## 前缀"
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
                    egui::Id::new(SCROLL_EDITOR_ID),
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
        // 软换行首次切入后的布局沉降可能在接管期首帧修正一次内容高;
        // 先消费这一帧,再钉住后续空闲帧不得继续被光标拽回。
        let mut stable = settled;
        for i in 0..5 {
            let after = live_scroll_frame(
                &ctx,
                egui::Id::new(SCROLL_EDITOR_ID),
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
            if i == 0 {
                stable = after;
                continue;
            }
            assert!(
                (after - stable).abs() <= 1.5,
                "接管期空闲帧不得拽回(稳定于 {stable:.1},现在 {after:.1})"
            );
        }

        // 下一次导航帧:恢复 Follow,光标行回目标带
        let _ = live_scroll_frame(
            &ctx,
            egui::Id::new(SCROLL_EDITOR_ID),
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
                egui::Id::new(SCROLL_EDITOR_ID),
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
            egui::Id::new(SCROLL_EDITOR_ID),
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
                    &AiState::default(),
                    cursor,
                    &mut selection_mirror,
                    live,
                    egui::Id::new(FOCUS_EDITOR_ID),
                    false,
                    show_focus,
                    true,
                    None,
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
                    &AiState::default(),
                    &mut cursor,
                    &mut selection_mirror,
                    &mut live,
                    egui::Id::new(FOCUS_EDITOR_ID),
                    typewriter,
                    focus,
                    true,
                    None,
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
        let (rendered, _, _, _, _, _) = live.rich_block_with_base(0, doc, None);
        assert!(rendered.contains("[完](<hl://>)"), "高亮层先跑:{rendered}");
        assert!(rendered.contains("task://"), "任务层后跑:{rendered}");
        assert!(!rendered.contains("=="), "标记被改写消费,不回流:{rendered}");

        // 缓存:同修订号直取同一份
        assert_eq!(
            live.rich_block_with_base(0, doc, None).0,
            rendered,
            "同修订号缓存命中"
        );

        // 零变化护栏:无 == 无任务标记的块恒等
        assert_eq!(
            live.rich_block_with_base(1, "普通段落。\n", None)
                .0
                .as_ref(),
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
        let body = egui::Color32::from_rgb(0x11, 0x22, 0x33);
        let handler =
            RichLinkHandler::new(TaskProbeDomain::Live(0), false, body, Default::default());
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
                crate::ui::rich_widgets::highlight_bg_color(ui.visuals()),
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
                &AiState::default(),
                &mut cursor,
                &mut selection_mirror,
                &mut live,
                egui::Id::new("live-highlight-test"),
                false,
                false,
                true,
                None,
                false,
                false,
                &crate::keymap::Keymap::builtin(),
                &mut outbox,
            );
        });
        let hl = crate::ui::rich_widgets::highlight_bg_color(&egui::Visuals::dark());
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
                &AiState::default(),
                &mut cursor,
                &mut sink,
                &mut live,
                egui::Id::new("gap-test"),
                false,
                false,
                true,
                None,
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

    // —— 相对图片 file:// 改写(live-unify-plan P1)——

    /// base_dir 是 rich_cache 的维度(R4):同一块源码,换文档目录后必须
    /// 重新改写(图片 URI 跟着换),不换则命中旧缓存;未落盘(None)无锚点
    /// 原样渲染。换目录/首次落盘两个失效方向都钉住。
    #[test]
    fn rich_block_cache_invalidates_when_base_dir_changes() {
        let doc = "![红点](./foo.assets/dot.png)\n";
        let mut live = LiveState {
            block_hashes: vec![source_hash(doc)],
            ..LiveState::default()
        };
        let a = std::path::Path::new("/tmp/latermd-docs-a");
        let b = std::path::Path::new("/tmp/latermd-docs-b");

        // 未落盘:无锚点不改写
        let (none_rendered, _, _, _, _, _) = live.rich_block_with_base(0, doc, None);
        assert_eq!(none_rendered.as_ref(), doc, "未落盘不改写");

        // 落盘到 a:URI 指向 a
        let (a_rendered, _, _, _, _, _) = live.rich_block_with_base(0, doc, Some(a));
        assert!(
            a_rendered.contains("file:///tmp/latermd-docs-a/foo.assets/dot.png"),
            "落盘后应改写成 a 目录的 file:// URI:{a_rendered}"
        );

        // 同目录再取:命中缓存(同一份渲染串)
        let (again, _, _, _, _, _) = live.rich_block_with_base(0, doc, Some(a));
        assert!(Arc::ptr_eq(&a_rendered, &again), "同目录命中缓存");

        // 换到 b:缓存失效重算,URI 跟着换
        let (b_rendered, _, _, _, _, _) = live.rich_block_with_base(0, doc, Some(b));
        assert!(
            b_rendered.contains("file:///tmp/latermd-docs-b/foo.assets/dot.png"),
            "换目录后应重新改写:{b_rendered}"
        );
    }

    /// 写作模式下相对图片出图(P1 验收,照预览
    /// local_file_image_paints_through_loaders 改写):文档目录作 base_dir,
    /// `./foo.assets/dot.png` 经第 5 层改写成 file:// 后由加载器画出纹理。
    #[test]
    fn live_local_file_image_paints_through_loaders() {
        let dir = std::env::temp_dir().join(format!("latermd-live-img-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("foo.assets")).unwrap();
        image::RgbImage::from_pixel(2, 2, image::Rgb([200, 40, 40]))
            .save(dir.join("foo.assets/dot.png"))
            .unwrap();

        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);

        let textured_rects = |output: &eframe::egui::FullOutput| -> usize {
            fn collect(shape: &egui::epaint::Shape, out: &mut usize) {
                match shape {
                    egui::epaint::Shape::Rect(rect) => {
                        // 0.36 起矩形纹理走 `brush`(纯色矩形/文字均为
                        // None),带 brush 的矩形即图片
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
            let mut editor = EditorBuffer::new(doc);
            let mut preview = PreviewState::new(&editor);
            let mut cursor = OutlineCursor::default();
            let mut live = LiveState {
                browse_only: true,
                browse_enabled: true,
                ..LiveState::default()
            };
            let mut selection = None;
            let mut outbox = Vec::new();
            ctx.run_ui(egui::RawInput::default(), |panel| {
                super::ui(
                    panel,
                    &mut editor,
                    &mut preview,
                    &AiState::default(),
                    &mut cursor,
                    &mut selection,
                    &mut live,
                    egui::Id::new("live-img-test"),
                    false,
                    false,
                    true,
                    Some(dir.as_path()),
                    false,
                    false,
                    &crate::keymap::Keymap::builtin(),
                    &mut outbox,
                );
            })
        };

        // 基线:无图文档出不了带纹理的矩形(防御断言),浮窗/滚动区首帧
        // 只完成注册,与其它无头测试同节奏跑满三帧。
        for _ in 0..3 {
            let output = render("# 基线\n\n正文,没有图片。\n");
            assert_eq!(textured_rects(&output), 0, "无图文档不应有图片纹理");
            output.drop_without_applying_deltas();
        }

        let mut painted = false;
        for _ in 0..200 {
            let output = render("![红点](./foo.assets/dot.png)\n");
            let textured = textured_rects(&output);
            output.drop_without_applying_deltas();
            if textured > 0 {
                painted = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            painted,
            "相对路径图片应经 file:// 画出图片纹理(写作模式出图)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // —— 流式帧 heal(live-unify-plan P1/R1)——

    /// 流式帧 heal 的两条验收:
    /// ① 未闭合 fence 的流式帧不出现残缺排版 —— fence 块在浏览态渲染成
    ///   代码块(内容 + 语言标签),围栏标记不作正文画出。实测记录:
    ///   CommonMark 下 pulldown 对未闭合 fence 本就在文末闭合,heal 前后
    ///   同形 —— 本条钉的是观感结果,不依赖实现路径。
    /// ② heal 接线的红绿锚:未闭合 ** 且块尾无换行(流式中途帧)时,
    ///   heal=true 补闭合、星号按标记隐去;heal=false 字面画出。拆掉
    ///   `.heal(heal)` 接线,②当场红。
    /// (另记 vendored heal 的既有边界,预览同样存在、不属本阶段:尾带
    /// 换行的未闭合 ** 会被把闭合符追加到换行之后,CommonMark 行首
    /// flanking 规则使其闭合不了 —— fence 不受影响。)
    #[test]
    fn live_streaming_frame_heals_unclosed_marks() {
        let ctx = egui::Context::default();
        let render = |doc: &str, heal: bool, id: &str| -> Vec<String> {
            let mut editor = EditorBuffer::new(doc);
            let mut preview = PreviewState::new(&editor);
            let mut cursor = OutlineCursor::default();
            let mut live = LiveState {
                browse_only: true,
                browse_enabled: true,
                ..LiveState::default()
            };
            let mut selection = None;
            let mut outbox = Vec::new();
            let output = ctx.run_ui(egui::RawInput::default(), |panel| {
                super::ui(
                    panel,
                    &mut editor,
                    &mut preview,
                    &AiState::default(),
                    &mut cursor,
                    &mut selection,
                    &mut live,
                    egui::Id::new(id),
                    false,
                    false,
                    true,
                    None,
                    heal,
                    false,
                    &crate::keymap::Keymap::builtin(),
                    &mut outbox,
                );
            });
            let mut texts = Vec::new();
            for clipped in &output.shapes {
                if let egui::epaint::Shape::Text(t) = &clipped.shape {
                    texts.push(t.galley.text().to_owned());
                }
            }
            output.drop_without_applying_deltas();
            texts
        };

        // ① 未闭合 fence:代码块照常渲染,围栏标记不漏进正文
        let frame = render(
            "正文在前。\n\n```rust\nfn main() {\n",
            true,
            "live-heal-fence",
        );
        assert!(
            !frame.iter().any(|text| text.contains("```")),
            "围栏标记不得当正文画出:{frame:?}"
        );
        assert!(
            frame.iter().any(|text| text.contains("fn main() {")),
            "fence 块内容应照常渲染:{frame:?}"
        );
        assert!(
            frame.iter().any(|text| text.contains("rust")),
            "fence 语言标签应照常渲染:{frame:?}"
        );

        // ② 未闭合 **(流式中途,块尾无换行):heal=true 补闭合
        let healed = render("正文 **加粗中", true, "live-heal-bold");
        assert!(
            !healed.iter().any(|text| text.contains("**")),
            "heal 帧里未闭合 ** 应补闭合(标记不显示):{healed:?}"
        );
        let raw = render("正文 **加粗中", false, "live-raw-bold");
        assert!(
            raw.iter().any(|text| text.contains("**")),
            "对照帧(heal=false)星号按字面渲染:{raw:?}"
        );
    }

    // —— emoji inline widget 探针与手型抑制(live-unify-plan P1)——

    /// 悬停 emoji 光标保持 Default(vendored 对悬停中的 inline widget 无条件
    /// 置 PointingHand,帧末按探针区块压回);同文档悬停普通链接仍
    /// PointingHand(vendored 默认不受牵连)。两个方向都钉住。
    #[test]
    fn live_emoji_hover_keeps_default_cursor_while_links_keep_pointing_hand() {
        let ctx = egui::Context::default();
        let doc = "开头 😀 结尾 [跳转链接](https://example.com/target) 完。\n";
        let editor_id = egui::Id::new("live-emoji-test");
        /// 夹具产物:emoji 探针区块 + 光标形态 + 文本 galley(链接定位用)。
        #[derive(Debug)]
        struct EmojiProbe {
            rects: Vec<egui::Rect>,
            cursor: egui::CursorIcon,
            galleys: Vec<(egui::Pos2, std::sync::Arc<egui::Galley>)>,
        }
        let frame = |events: Vec<egui::Event>| -> EmojiProbe {
            let mut editor = EditorBuffer::new(doc);
            let mut preview = PreviewState::new(&editor);
            let mut cursor = OutlineCursor::default();
            let mut live = LiveState {
                browse_only: true,
                browse_enabled: true,
                ..LiveState::default()
            };
            let mut selection = None;
            let mut outbox = Vec::new();
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
            let output = ctx.run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |panel| {
                    super::ui(
                        panel,
                        &mut editor,
                        &mut preview,
                        &AiState::default(),
                        &mut cursor,
                        &mut selection,
                        &mut live,
                        editor_id,
                        false,
                        false,
                        true,
                        None,
                        false,
                        false,
                        &crate::keymap::Keymap::builtin(),
                        &mut outbox,
                    );
                },
            );
            fn collect(
                shape: &egui::epaint::Shape,
                galleys: &mut Vec<(egui::Pos2, std::sync::Arc<egui::Galley>)>,
            ) {
                match shape {
                    egui::epaint::Shape::Text(t) => galleys.push((t.pos, t.galley.clone())),
                    egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| collect(s, galleys)),
                    _ => {}
                }
            }
            let mut galleys = Vec::new();
            for clipped in &output.shapes {
                collect(&clipped.shape, &mut galleys);
            }
            let rects = ctx
                .data(|d| d.get_temp::<Vec<egui::Rect>>(super::emoji_probe_id(editor_id)))
                .unwrap_or_default();
            let cursor = output.platform_output.cursor_icon;
            output.drop_without_applying_deltas();
            EmojiProbe {
                rects,
                cursor,
                galleys,
            }
        };
        // 行带找法照预览 row_band_of:`needle` 首字的字形行带(0 宽 caret
        // rect),命中点向右挪半个字形落进字形内部。
        let band_of =
            |galleys: &[(egui::Pos2, std::sync::Arc<egui::Galley>)], needle: &str| -> egui::Rect {
                for (origin, galley) in galleys {
                    if let Some(byte) = galley.text().find(needle) {
                        let chars = galley.text()[..byte].chars().count();
                        let band = galley.pos_from_cursor(egui::text::CCursor::new(chars));
                        return egui::Rect::from_min_max(
                            egui::pos2(origin.x + band.left(), origin.y + band.top()),
                            egui::pos2(origin.x + band.right(), origin.y + band.bottom()),
                        );
                    }
                }
                panic!("{needle:?} 不在任何文本 shape 内");
            };

        let probe = frame(Vec::new());
        assert_eq!(probe.rects.len(), 1, "文档只有一枚 emoji:{probe:?}");
        let emoji_center = probe.rects[0].center();
        let band = band_of(&probe.galleys, "跳转链接");
        let link_center = egui::pos2(band.left() + 8.0, band.center().y);

        let on_emoji = frame(vec![egui::Event::PointerMoved(emoji_center)]);
        assert_eq!(on_emoji.cursor, egui::CursorIcon::Default, "emoji 不吃手型");
        let on_link = frame(vec![egui::Event::PointerMoved(link_center)]);
        assert_eq!(
            on_link.cursor,
            egui::CursorIcon::PointingHand,
            "普通链接的手型不受牵连"
        );
    }

    // —— 浏览态跳转「只滚不编辑」(live-unify-plan P3/D2)——

    /// D2:锁定浏览(禅定)下 `cursor.jump_to` **只滚动** —— 不产活动块、
    /// 不写光标字节、不抢焦点,但视口真的滚到了目标块。非锁定(写作浏览
    /// 态)维持老语义:跳转即落点与进编辑(#60/#42 的契约不动)。
    #[test]
    fn locked_browse_jump_only_scrolls_while_unlocked_still_enters_edit() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0));
        let mut text = String::new();
        for i in 0..80 {
            text.push_str(&format!("段{i:02} 的一行正文。\n\n"));
        }
        let target_byte = text.find("段60").expect("文档含目标段");

        let run = |locked: bool, jump: Option<usize>| -> (LiveState, Option<usize>, f32) {
            let mut editor = EditorBuffer::new(&text);
            let mut preview = PreviewState::new(&editor);
            let mut cursor = OutlineCursor {
                jump_to: jump,
                ..OutlineCursor::default()
            };
            let mut live = LiveState::default();
            live.sync(&editor, None);
            live.browse_only = true;
            live.browse_enabled = !locked;
            live.browse_locked = locked;
            let mut selection = None;
            let mut outbox = Vec::new();
            let id = egui::Id::new(if locked { "live-locked" } else { "live-open" });
            for _ in 0..2 {
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |panel| {
                        super::ui(
                            panel,
                            &mut editor,
                            &mut preview,
                            &AiState::default(),
                            &mut cursor,
                            &mut selection,
                            &mut live,
                            id,
                            false,
                            false,
                            true,
                            None,
                            false,
                            false,
                            &crate::keymap::Keymap::builtin(),
                            &mut outbox,
                        );
                    },
                );
                output.drop_without_applying_deltas();
            }
            let probe = ctx
                .data(|d| d.get_temp::<LiveScrollProbe>(super::scroll_probe_id(id)))
                .expect("帧末写了滚动探针");
            // 视口/high 一起核对:滚到了目标块,且内容确实高于视口
            // (否则「offset > 0」可能是别的原因)。
            assert!(probe.viewport.height() > 0.0, "{probe:?}");
            assert!(
                probe.content_height > probe.viewport.height(),
                "内容高于视口:{probe:?}"
            );
            (live, cursor.byte, probe.offset)
        };

        let target_char = text[..target_byte].chars().count();
        // ① 锁定浏览:不产活动块、不写光标字节,但滚动发生了
        let (live, cur_byte, offset) = run(true, Some(target_char));
        assert!(live.active.is_none(), "锁定浏览不进编辑:{live:?}");
        assert!(live.browse_only, "仍是浏览态");
        assert_eq!(cur_byte, None, "不写光标字节");
        assert!(offset > 0.0, "视口滚到了目标块附近:{offset}");
        // ② 非锁定:照旧切块进编辑(活动块 = 目标块)
        let (live_open, _, _) = run(false, Some(target_char));
        assert_eq!(
            live_open.active,
            Some(live_open.block_containing(target_byte).unwrap()),
            "未锁定时跳转仍落块进编辑:{live_open:?}"
        );
    }

    /// 滚轮锚点的对齐落地(Split 对照列同步滚动的数学钉子):同一目标块
    /// 只差 `anchor_align_y`,视口相对高度相差 100 → 落定后的滚动偏移恰差
    /// 100;顶对齐(rel=0)比无意图的最小滚动走得更远(块已在视口内时
    /// 最小滚动为 0)。目标块选**首屏内**:块真实渲染、无估高漂移,断言
    /// 得以钉到亚像素。每次 run 换全新 Context —— ScrollArea 偏移是按
    /// editor id 持久化的,共用 Context 会把上一轮的落点带进下一轮。
    /// 意图同帧消费即清,不残留到后续帧。
    #[test]
    fn locked_jump_landing_aligns_block_top_to_anchor_rel() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let text = "段落甲,这一段只是填充正文。\n\n".repeat(40);
        let target_block = 3;

        let run = |rel: Option<f32>| -> (LiveState, f32) {
            let ctx = egui::Context::default();
            let mut editor = EditorBuffer::new(&text);
            let mut preview = PreviewState::new(&editor);
            let mut cursor = OutlineCursor::default();
            let mut live = LiveState::default();
            live.sync(&editor, None);
            live.browse_only = true;
            live.browse_enabled = false;
            live.browse_locked = true;
            live.pending_caret = Some((target_block, 0));
            live.caret_follow = true;
            live.anchor_align_y = rel;
            let mut selection = None;
            let mut outbox = Vec::new();
            let id = egui::Id::new("live-anchor-align");
            let mut probe = None;
            for _ in 0..3 {
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |panel| {
                        super::ui(
                            panel,
                            &mut editor,
                            &mut preview,
                            &AiState::default(),
                            &mut cursor,
                            &mut selection,
                            &mut live,
                            id,
                            false,
                            false,
                            true,
                            None,
                            false,
                            false,
                            &crate::keymap::Keymap::builtin(),
                            &mut outbox,
                        );
                    },
                );
                output.drop_without_applying_deltas();
                probe = Some(
                    ctx.data(|d| d.get_temp::<LiveScrollProbe>(super::scroll_probe_id(id)))
                        .expect("帧末写了滚动探针"),
                );
            }
            assert!(
                live.pending_caret.is_none() && live.anchor_align_y.is_none(),
                "意图同帧消费即清:{live:?}"
            );
            (live, probe.unwrap().offset)
        };

        let (live_a, offset_rel0) = run(Some(0.0));
        assert!(live_a.active.is_none(), "锁定浏览不进编辑");
        let (_, offset_rel100) = run(Some(100.0));
        let (_, offset_none) = run(None);

        assert!(
            offset_rel0 > 100.0,
            "顶对齐真的滚了(目标块不在文档顶):{offset_rel0}"
        );
        assert!(
            (offset_rel0 - offset_rel100 - 100.0).abs() < 0.5,
            "rel 差 100 → 偏移恰差 100:{offset_rel0} vs {offset_rel100}"
        );
        assert!(
            offset_none.abs() < 0.5,
            "块已在视口内时最小滚动不动:{offset_none}"
        );
    }

    // —— AI 指令卡与 ai:// 链接(live-unify-plan P2/D1)——

    /// AI 指令卡在 Live 富渲染块出卡(P2 验收,照预览
    /// ai_block_renders_card_without_panic 改写跑 live::ui 路径):```ai
    /// 围栏经 with_ai 的 handler 认领 block widget,画出卡片标题/指令
    /// 原文/执行按钮/状态行。
    #[test]
    fn live_ai_block_renders_card() {
        let ctx = egui::Context::default();
        let render = |doc: &str| -> Vec<String> {
            let mut editor = EditorBuffer::new(doc);
            let mut preview = PreviewState::new(&editor);
            let mut cursor = OutlineCursor::default();
            let mut live = LiveState {
                browse_only: true,
                browse_enabled: true,
                ..LiveState::default()
            };
            let mut selection = None;
            let mut outbox = Vec::new();
            let output = ctx.run_ui(egui::RawInput::default(), |panel| {
                super::ui(
                    panel,
                    &mut editor,
                    &mut preview,
                    &AiState::default(),
                    &mut cursor,
                    &mut selection,
                    &mut live,
                    egui::Id::new("live-ai-card"),
                    false,
                    false,
                    true,
                    None,
                    false,
                    false,
                    &crate::keymap::Keymap::builtin(),
                    &mut outbox,
                );
            });
            let mut texts = Vec::new();
            for clipped in &output.shapes {
                if let egui::epaint::Shape::Text(t) = &clipped.shape {
                    texts.push(t.galley.text().to_owned());
                }
            }
            output.drop_without_applying_deltas();
            texts
        };

        let all = render("# 文档\n\n```ai\n总结本文要点\n```\n");
        let joined = all.join("\n");
        for piece in ["AI 指令", "总结本文要点", "执行", "未执行"] {
            assert!(
                all.contains(&piece.to_owned()),
                "卡片应渲染 {piece}:{joined:?}"
            );
        }
        assert!(
            !all.iter().any(|text| text.contains("```")),
            "围栏标记不得当正文画出(卡片认领了围栏):{joined:?}"
        );
    }

    /// `ai://` 链接点击在 Live 下可达(P2 验收):真实事件流(悬停→按下→
    /// 抬起)落在链接字形上,vendored 命中把 click 路由进 handler,消息进
    /// outbox;浏览态下单击不连带切入编辑(点击归链接)。
    #[test]
    fn live_ai_link_click_sends_message() {
        let ctx = egui::Context::default();
        let doc = "开头 [续写](<ai://write?prompt=%E7%BB%AD%E5%86%99>) 结尾。\n";
        let editor_id = egui::Id::new("live-ai-link");
        // 一帧渲染 + 消息累积。live::ui 每帧各自建 outbox,点击帧的
        // drain 把消息带出。
        let frame = |events: Vec<egui::Event>| -> (Vec<String>, Vec<Message>, bool) {
            let mut editor = EditorBuffer::new(doc);
            let mut preview = PreviewState::new(&editor);
            let mut cursor = OutlineCursor::default();
            let mut live = LiveState {
                browse_only: true,
                browse_enabled: true,
                ..LiveState::default()
            };
            let mut selection = None;
            let mut outbox = Vec::new();
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
            let output = ctx.run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |panel| {
                    super::ui(
                        panel,
                        &mut editor,
                        &mut preview,
                        &AiState::default(),
                        &mut cursor,
                        &mut selection,
                        &mut live,
                        editor_id,
                        false,
                        false,
                        true,
                        None,
                        false,
                        false,
                        &crate::keymap::Keymap::builtin(),
                        &mut outbox,
                    );
                },
            );
            let mut texts = Vec::new();
            for clipped in &output.shapes {
                if let egui::epaint::Shape::Text(t) = &clipped.shape {
                    texts.push(t.galley.text().to_owned());
                }
            }
            let browse_kept = live.browse_only && live.active.is_none();
            output.drop_without_applying_deltas();
            (texts, outbox, browse_kept)
        };
        // 链接字形中心:渲染文本里「续写」的行带(galley 文本即链接文字)。
        let (texts, _, _) = frame(Vec::new());
        let galley = texts
            .iter()
            .find(|text| text.contains("续写"))
            .expect("链接文字已渲染");
        assert!(
            galley.contains("开头") && galley.contains("结尾"),
            "{galley:?}"
        );

        // 找「续写」的屏幕矩形:重新渲一帧按字形逐个量(galley pos +
        // rect 布局),取「续」字形的包围盒中心。
        let (_, _, _) = frame(Vec::new());
        let mut link_center = None;
        {
            let mut editor = EditorBuffer::new(doc);
            let mut preview = PreviewState::new(&editor);
            let mut cursor = OutlineCursor::default();
            let mut live = LiveState {
                browse_only: true,
                browse_enabled: true,
                ..LiveState::default()
            };
            let mut selection = None;
            let mut outbox = Vec::new();
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |panel| {
                    super::ui(
                        panel,
                        &mut editor,
                        &mut preview,
                        &AiState::default(),
                        &mut cursor,
                        &mut selection,
                        &mut live,
                        editor_id,
                        false,
                        false,
                        true,
                        None,
                        false,
                        false,
                        &crate::keymap::Keymap::builtin(),
                        &mut outbox,
                    );
                },
            );
            for clipped in &output.shapes {
                if let egui::epaint::Shape::Text(t) = &clipped.shape {
                    if t.galley.text().contains("续写") {
                        let byte = t.galley.text().find("续写").unwrap();
                        let chars = t.galley.text()[..byte].chars().count();
                        let band = t.galley.pos_from_cursor(egui::text::CCursor::new(chars));
                        link_center = Some(egui::pos2(
                            t.pos.x + band.center().x + 4.0,
                            t.pos.y + band.center().y,
                        ));
                    }
                }
            }
            output.drop_without_applying_deltas();
        }
        let link_center = link_center.expect("链接字形应可定位");

        // 事件帧序:悬停一帧 → 按下一帧 → 抬起一帧(13a 纪律)。
        let primary = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let mut messages = Vec::new();
        let mut browse_kept = true;
        for events in [
            vec![egui::Event::PointerMoved(link_center)],
            vec![primary(link_center, true)],
            vec![primary(link_center, false)],
        ] {
            let (_, outbox, kept) = frame(events);
            messages.extend(outbox);
            browse_kept &= kept;
        }
        assert_eq!(
            messages,
            vec![Message::AiLinkClicked {
                prompt: Ok("续写".into())
            }],
            "ai:// 点击应经 Live handler 产出流式续写消息"
        );
        assert!(browse_kept, "点击归链接,浏览态不连带切入编辑");
    }

    // —— 同一文档渲两份(三态视图 PR1 的可重入化)——

    /// 两个实例(主写作列 + 只读对照列)同帧渲染同一份文档:**各自的逐帧
    /// 命中表不被对方清掉**。
    ///
    /// vendored 的 `MarkdownLabel` 把块表与命中 galley 按 label id 单槽
    /// 存放,并在每次 `render` 开头清空。两列的 label id 由各自的
    /// `editor_id` 派生(见 [`super::ui`] 的 id 约定),因此先画的那一列
    /// 在帧末仍能读到自己的记录 —— 否则 `click_caret_local` 会退化到
    /// 「落块尾」的兜底。
    #[test]
    fn two_instances_keep_their_own_hit_tables() {
        let ctx = egui::Context::default();
        let doc = "第一段正文，用于命中定位。\n\n第二段正文。\n";
        let mut editor = EditorBuffer::new(doc);
        let mut preview = PreviewState::new(&editor);
        let mut main_live = LiveState::default();
        let mut right_live = LiveState::default();
        for live in [&mut main_live, &mut right_live] {
            live.browse_only = true;
            live.browse_enabled = false;
            live.browse_locked = true;
        }
        let main_id = egui::Id::new("main-column");
        // 对照列:同一派生规则、不同命名空间(tab_id 之外的 salt)
        let right_id = egui::Id::new("compare-column").with(1_u64);

        // 两列先后渲染(只读对照列在后,与生产顺序一致)
        let run = |editor: &mut EditorBuffer,
                   preview: &mut PreviewState,
                   live: &mut LiveState,
                   id: egui::Id|
         -> (Vec<egui::Rect>, Vec<String>) {
            let mut outbox = Vec::new();
            let mut selection = None;
            let mut cursor = OutlineCursor::default();
            let mut hit_after = Vec::new();
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let _ = super::ui(
                    ui,
                    editor,
                    preview,
                    &AiState::default(),
                    &mut cursor,
                    &mut selection,
                    live,
                    id,
                    false,
                    false,
                    true,
                    None,
                    false,
                    false,
                    &crate::keymap::Keymap::builtin(),
                    &mut outbox,
                );
                // 帧内读取:本列刚画完,块表应仍在;后画的另一列不得清掉它
                hit_after = egui_markdown::block_span_rects(ui, id.with(("live-render", 0)))
                    .map(|entries| entries.into_iter().map(|entry| entry.rect).collect())
                    .unwrap_or_default();
            });
            let texts: Vec<String> = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Text(t) => Some(t.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
            output.drop_without_applying_deltas();
            (hit_after, texts)
        };
        let (main_blocks, main_texts) = run(&mut editor, &mut preview, &mut main_live, main_id);
        assert!(!main_blocks.is_empty(), "主列渲出块几何:{main_texts:?}");
        let (right_blocks, right_texts) = run(&mut editor, &mut preview, &mut right_live, right_id);
        assert!(!right_blocks.is_empty(), "对照列渲出块几何:{right_texts:?}");
        // 再渲一次主列:它的块表必须仍然可读(证明两列 id 不串台)
        let (main_again, _) = run(&mut editor, &mut preview, &mut main_live, main_id);
        assert!(
            !main_again.is_empty(),
            "后画的对照列没清掉主列的命中表:{main_texts:?}"
        );
        // 关键断言:两列的 id 不相撞
        assert_ne!(
            main_id.with(("live-render", 0)),
            right_id.with(("live-render", 0)),
            "两列的块 label id 必须不同"
        );
        assert!(
            !main_texts.is_empty() && !right_texts.is_empty(),
            "两列都真的画出了内容"
        );
    }

    /// 两列的滚动状态互相独立(`ScrollArea` id 带 `editor_id`):只给其中一列
    /// 下跳转目标,另一列必须停在原处。写死 id 时两列共用一份 ScrollArea
    /// 记忆,表现为「翻对照列,主编辑区跟着跳」。
    ///
    /// 取证路径与 P3/D2 的 `locked_browse_jump_*` 同一条(经 `cursor.jump_to`
    /// → 锁定浏览落地分支 → 块级滚动),只读帧末探针。
    #[test]
    fn two_instances_have_independent_scroll_state() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0));
        let mut text = String::new();
        for i in 0..80 {
            text.push_str(&format!(
                "段{i:02} 的一行正文。

"
            ));
        }
        let target_byte = text.find("段60").expect("文档含目标段");
        let target_char = text[..target_byte].chars().count();
        let main_id = egui::Id::new("compare-main");
        let right_id = egui::Id::new("compare-column").with(1_u64);

        let run = |editor: &mut EditorBuffer,
                   preview: &mut PreviewState,
                   live: &mut LiveState,
                   id: egui::Id,
                   jump: Option<usize>|
         -> f32 {
            live.sync(editor, None);
            live.browse_only = true;
            live.browse_enabled = false;
            live.browse_locked = true;
            let mut cursor = OutlineCursor {
                jump_to: jump,
                ..OutlineCursor::default()
            };
            let mut selection = None;
            let mut outbox = Vec::new();
            for _ in 0..2 {
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |panel| {
                        let _ = super::ui(
                            panel,
                            editor,
                            preview,
                            &AiState::default(),
                            &mut cursor,
                            &mut selection,
                            live,
                            id,
                            false,
                            false,
                            true,
                            None,
                            false,
                            false,
                            &crate::keymap::Keymap::builtin(),
                            &mut outbox,
                        );
                    },
                );
                output.drop_without_applying_deltas();
            }
            ctx.data(|d| {
                d.get_temp::<LiveScrollProbe>(super::scroll_probe_id(id))
                    .map(|probe| probe.offset)
                    .unwrap_or(-1.0)
            })
        };

        let mut editor = EditorBuffer::new(&text);
        let mut preview = PreviewState::new(&editor);
        let mut main_live = LiveState::default();
        let mut right_live = LiveState::default();
        let main_offset = run(
            &mut editor,
            &mut preview,
            &mut main_live,
            main_id,
            Some(target_char),
        );
        let right_offset = run(&mut editor, &mut preview, &mut right_live, right_id, None);
        assert!(
            main_offset > 0.0,
            "主列收到跳转目标 ⇒ 偏移 > 0(实测 {main_offset})"
        );
        assert_eq!(
            right_offset, 0.0,
            "对照列从未收到目标 ⇒ 停在原处(共用记忆时会被主列带走)"
        );
    }
}
