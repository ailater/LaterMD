//! 多标签页(docs/auto-plan.md #11「multi-tabs」)。
//!
//! 每个标签持有**自己的一套**文档状态:落盘身份、编辑器缓冲、预览快照、
//! 大纲光标 —— 换标签零拷贝,只是换"当前指针"。编辑器 widget 的 TextEdit
//! 状态(光标/undo)按标签的稳定 `id` 隔离(`ui::editor` 用 id 派生 widget
//! id),不随标签增删错位。
//!
//! 语义约定(auto-plan 规格 + 既有哲学的延伸):
//! * 打开文件**永远开新标签**,已在某标签打开(`find_by_path` 路径去重)
//!   则直接激活它 —— 多标签下「新建/打开」不再覆盖当前缓冲,
//!   `unsaved_guard` 的拦截对象消失了;
//! * 关闭**脏**标签必须显式确认(模态文案同回滚确认的不可逆警示),
//!   静默丢稿的代价大于多一次点击;
//! * 在途 AI 流**绑定发起标签的 id**(`State::ai_active_tab`):换标签/
//!   开新标签不改写入目标也不中断;只有发起标签被关闭才作废(剩余块
//!   无处可写,写进任何别的标签都是写错文档);
//! * 批量关闭(#37 右键菜单)以**被右键的标签**为基准,队列存稳定 id、
//!   归约逐个走单标签关闭的同一条脏确认通路(见
//!   `State::advance_batch_close`):确认一个关一个,任一次取消立即
//!   终止剩余队列 —— 已确认关闭的不回滚,被拒绝的保持打开。

use crate::live::LiveState;
use crate::state::{DocumentState, OutlineCursor, PreviewState};
use latermd_editor::EditorBuffer;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// 单个标签的自动保存记忆(#18):draft 落点、已落盘修订号、上次缓冲改动
/// 时刻。判定与写入都在 [`crate::state::State`] 的帧末归约
/// (`State::autosave_pass`),本结构只记账,不含任何 IO。
#[derive(Debug, Default)]
pub struct TabAutosave {
    /// 上次成功写出的 draft 落点;`None` = 本标签从未写过。保存/关闭的
    /// 清理钩子按它删文件 —— 不从落盘身份现算,「另存为换路径后旧位置的
    /// draft」才找得回来。
    pub draft_path: Option<PathBuf>,
    /// 已落盘的缓冲修订号;与当前修订号相同即跳过重写(防每帧空转重写)。
    /// 写失败不记 —— 下次触发照常重试。
    pub saved_rev: Option<u64>,
    /// 上次缓冲改动时刻(手敲/IME/AI 流式都算);`None` = 尚无编辑,
    /// 停顿判定不满足。
    pub last_edit: Option<Instant>,
    /// 帧末比对用的「上次见到的修订号」:与当前修订号不同即发生过改动,
    /// 刷新 `last_edit`。AI 流可写非活动标签,故按标签各自记账。
    pub(crate) seen_rev: u64,
}

/// 孤儿 draft 的待恢复状态(#18 恢复条)。
///
/// 新标签认领文档路径时若文档旁有遗留的 `<doc>.latermd-draft`,检测结果
/// 挂在标签上,编辑区上方的恢复条据此渲染;「恢复 / 丢弃」的归约消费即清
/// ([`crate::state::State::recover_draft`] / [`discard_draft`](crate::state::State::discard_draft))。
/// 用户无视恢复条直接编辑(缓冲变脏)时也在帧末撤下 —— 那是隐性选择了
/// 以盘上版本续写,条留着只会诱导一次「拿旧稿盖掉新稿」的误点;draft
/// **文件**不随撤条删除,停顿/切出路径照常接管。
#[derive(Debug, Clone, PartialEq)]
pub struct DraftRecovery {
    /// draft 落点(检测时已确认存在)。
    pub path: PathBuf,
    /// 检测时刻的 mtime,恢复条显示「保存于何时」用。文件系统不给
    /// (`modified()` 失败)以「存在」为准,`None` 容之(文案落「保存时间
    /// 未知」),不为一个展示字段放弃整条恢复能力。
    pub mtime: Option<std::time::SystemTime>,
}

/// 标签重命名(#37 右键菜单「重命名」,显示别名语义)的输入状态:
/// 目标按**稳定 id** 记 + 草稿文本(UI 原地改,与 `ImageDialogState` 同款)。
///
/// 目标存 id 而非索引的理由与 `confirm_close` 相同:浮窗是非阻塞 Window,
/// 打开期间其他关闭入口会使索引漂移。
#[derive(Debug, Clone, PartialEq)]
pub struct TabRename {
    /// 正在改名的标签稳定 id。
    pub tab_id: u64,
    /// 输入草稿;开框时预填该标签当前显示基础名(不带 dirty 星)。
    pub draft: String,
    /// 输入框还没做过「聚焦 + 选中主体」,语义与
    /// [`crate::filetree::RenameDraft::focus_pending`] 同款(标签改名
    /// 无扩展名,选区即全选)。
    pub focus_pending: bool,
}

/// 单个标签的全部文档状态。
pub struct TabState {
    /// 稳定 id:编辑器 widget id 与测试定位都用它,不随标签增删变化。
    pub id: u64,
    /// 标签显示别名(#37「重命名」,**纯显示层**):`Some` = 标签条显示它
    /// 而非文件名。恒为 trim 后非空(归约 `confirm_tab_rename` 保证);
    /// `None` = 显示文件名/「未命名」。不动 path/缓冲/dirty/保存目标。
    pub alias: Option<String>,
    /// 落盘身份 + dirty 镜像 + 提示行。
    pub document: DocumentState,
    /// 待恢复的孤儿 draft(#18 恢复条);`None` = 无待裁决草稿。
    pub recover: Option<DraftRecovery>,
    /// 编辑器缓冲(该标签的正文真源)。
    pub editor: EditorBuffer,
    /// 预览快照(含大纲)。
    pub preview: PreviewState,
    /// 大纲↔编辑器光标协调。
    pub cursor: OutlineCursor,
    /// 自动保存记忆(#18)。
    pub autosave: TabAutosave,
    /// 编辑器选区的**字符**区间`(起, 止)`,由 `ui::editor` 每帧回填
    /// (Live 模式由 `live::ui` 从活动块镜像,无活动块即 `None`)。
    ///
    /// 存在的理由:工具条按钮被点中的时候编辑器已经失焦,而 `TextEdit` 的
    /// 选区活在其持久 widget state 里,归约侧拿不到 —— 于是 UI 每帧把它
    /// 抄到这里(与 `OutlineCursor::byte` 同一手法)。`None` = 这一帧还没
    /// 渲染过。
    pub selection: Option<(usize, usize)>,
    /// 待写回的新选区(格式动作产出),由 `ui::editor` 下一帧消费。
    ///
    /// 链路见 docs/ui-shell-redesign.md §6.4:`FormatRequested` → 归约 →
    /// 这里 → UI 写入 `TextEdit` 持久 cursor + 还焦。**仅源码模式消费**:
    /// Live 模式的落点走 `LiveState::pending_caret/pending_range`。
    pub pending_selection: Option<(usize, usize)>,
    /// Live Preview 的块表与活动块(仅在 Live 模式下使用)。
    pub live: LiveState,
    /// 只读对照列的块表(三态视图的「源码+对照」右栏)。
    ///
    /// **必须是独立的一份**:它与 [`Self::live`] 渲染同一份缓冲,却各有自己的
    /// id 命名空间(对照列传入独立的 editor id)。共用会让
    /// vendored 的 per-label 布局缓存与逐帧命中表互相清除,且只读列的写入会
    /// 移动主编辑区的可编辑块。浏览标志由使用方每帧置位,与禅定的临时浏览
    /// 同款手法。
    pub live_right: LiveState,
    /// 新建空文档首帧请求编辑器焦点，消费一次后清除。
    pub focus_editor_on_next_frame: bool,
}

impl TabState {
    /// 以初始内容建标签(`TabsState` 的唯一入口,`next_id` 在外层统一发)。
    fn new(id: u64, path: Option<PathBuf>, text: &str) -> Self {
        let editor = EditorBuffer::new(text);
        let mut live = LiveState::default();
        live.browse_only = true;
        live.browse_enabled = true;
        // 只读对照列同一套初值:浏览优先,等使用方按模式接管。
        let mut live_right = LiveState::default();
        live_right.browse_only = true;
        live_right.browse_enabled = true;
        Self {
            id,
            alias: None,
            document: DocumentState {
                path,
                dirty: false,
                notice: None,
            },
            recover: None,
            preview: PreviewState::new(&editor),
            cursor: OutlineCursor::default(),
            autosave: TabAutosave::default(),
            selection: None,
            pending_selection: None,
            live,
            live_right,
            editor,
            focus_editor_on_next_frame: false,
        }
    }

    /// 整篇换入并复位文档身份(读盘成功后的换入;同 `State::load_document`
    /// 的旧语义,但只作用于本标签)。
    ///
    /// 自动保存记忆随换入重置内容侧(`saved_rev`/`last_edit`/`seen_rev`),
    /// `draft_path` **保留** —— 它是清理钩子找回旧位置 draft 的唯一线索
    /// (回滚后保存一次即清)。`recover`(待恢复的孤儿 draft)同样不动:
    /// 换入的是磁盘内容,盘旁的遗留草稿与「用户还没裁决」这一事实都还在。
    /// 盘上的 draft 文件不动:换入意味着旧编辑已被用户确认丢弃,但误删
    /// 防丢镜像的代价远大于多留一份冗余。
    pub fn load(&mut self, path: Option<PathBuf>, text: &str) {
        self.editor.load(text);
        self.document.path = path;
        self.document.notice = None;
        self.preview.rebuild(&self.editor);
        self.live.reset();
        self.live_right.reset();
        let kept_draft = self.autosave.draft_path.take();
        self.autosave = TabAutosave {
            draft_path: kept_draft,
            ..TabAutosave::default()
        };
    }

    /// 标签显示基础名(**不带** dirty 星):别名优先,否则文件名/「未命名」。
    /// 重命名浮窗的预填草稿取它(带星的名字回填进输入框会混入非用户输入)。
    pub fn label_base(&self) -> String {
        self.alias
            .clone()
            .unwrap_or_else(|| self.document.base_name())
    }

    /// 标签条显示名(#37 重命名):基础名 + dirty 星。别名是纯显示层,
    /// 落盘身份的窗口标题仍走 [`DocumentState::window_title`],显示文件名。
    pub fn display_name(&self) -> String {
        let base = self.label_base();
        if self.document.dirty {
            format!("{base}*")
        } else {
            base
        }
    }
}

/// 关闭栈(#45 TabRestore)容量上限:超出淘汰最旧(浏览器无上限,这里
/// 封顶防长会话无限增长,规格见 preview-typography-and-keymap-plan §3.3)。
const RECENTLY_CLOSED_CAP: usize = 20;

/// 批量关闭的类别(#37 标签条右键菜单):以**被右键的标签**为基准,
/// 左/右/其他都不含基准标签本身。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchClose {
    /// 关闭基准标签左侧的全部标签。
    Left,
    /// 关闭基准标签右侧的全部标签。
    Right,
    /// 关闭除基准标签外的全部标签。
    Others,
    /// 关闭全部标签(关空由 `TabsState::remove` 兜底补一个空标签)。
    All,
}

/// 标签集合:全部标签 + 当前指针 + 关闭确认 + id 发放器。
pub struct TabsState {
    /// 打开的标签(至少一个;关闭最后一个即换入新的空标签)。
    pub tabs: Vec<TabState>,
    /// 当前标签索引。
    pub active: usize,
    /// 待确认关闭的脏标签**稳定 id**;`Some` 时 UI 显示确认模态。不存索引:
    /// 模态是非阻塞 Window,打开期间其他关闭入口(标签条 × / Ctrl+W)还会
    /// 动标签列表使索引漂移,按漂移索引确认会关错标签;id 不随增删漂移
    /// (与在途 AI 流的 `State::ai_active_tab` 同手法)。
    pub confirm_close: Option<u64>,
    /// 批量关闭(#37)的剩余目标(稳定 id,按关闭顺序);非空即批量进行中。
    /// 归约逐个消费:干净标签直接移除,脏标签把 `confirm_close` 指到它等
    /// 确认;用户取消即清空本队列(已关闭的不回滚)。存 id 而非索引,与
    /// `confirm_close` 同理由。
    pub pending_close: Vec<u64>,
    /// 重命名浮窗(#37「重命名」)的输入状态;`Some` 时 UI 显示浮窗。
    /// 同一时间至多一个(新请求顶掉旧浮窗)。
    pub rename: Option<TabRename>,
    /// 最近关闭标签的已落盘路径(#45 TabRestore 的关闭栈,后进先出):
    /// [`TabsState::remove`](Self::remove)(唯一摘除点)入栈,**只记已落盘
    /// 路径** —— 未落盘的新标签重开拿不回内容,不入栈;封顶
    /// [`RECENTLY_CLOSED_CAP`] 淘汰最旧。不做跨会话持久化(标签会话本就
    /// 不落盘)。
    pub recently_closed: Vec<PathBuf>,
    /// 下一个标签的 id(自增,不复用 —— 关了再开新标签,编辑器 undo/光标
    /// 状态必须是全新的)。
    next_id: u64,
}

impl Default for TabsState {
    fn default() -> Self {
        // State::default 会立即用 SAMPLE_MD 建 tags;这里给空标签兜底,
        // 保证「至少一个标签」的不变量在任何构造路径下都成立。
        Self::new("")
    }
}

impl TabsState {
    /// 以初始内容建第一个标签。
    pub fn new(initial: &str) -> Self {
        Self {
            tabs: vec![TabState::new(1, None, initial)],
            active: 0,
            confirm_close: None,
            pending_close: Vec::new(),
            rename: None,
            recently_closed: Vec::new(),
            next_id: 2,
        }
    }

    /// 当前标签。
    pub fn current(&self) -> &TabState {
        &self.tabs[self.active]
    }

    /// 当前标签(可变)。
    pub fn current_mut(&mut self) -> &mut TabState {
        let active = self.active;
        &mut self.tabs[active]
    }

    /// 某路径已在哪个标签打开(按落盘身份逐字节比较)。
    pub fn find_by_path(&self, path: &Path) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.document.path.as_deref() == Some(path))
    }

    /// 稳定 id 对应的标签索引;id 不在(标签已移除)返回 `None`。在途 AI 流
    /// 的写入目标按 id 而非索引定位 —— 索引随标签增删漂移,id 不会。
    pub fn index_by_id(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    /// 待确认关闭的标签(确认模态的文案来源);id 已失效(目标被移除)返回
    /// `None`,模态随之不再渲染。
    pub fn confirm_close_tab(&self) -> Option<&TabState> {
        let id = self.confirm_close?;
        self.tabs.iter().find(|tab| tab.id == id)
    }

    /// 开新标签(换入 `text`)并激活,返回新标签索引。打开文件**永远走
    /// 这里**,当前标签的缓冲与 dirty 不受影响。
    pub fn open_tab(&mut self, path: Option<PathBuf>, text: &str) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(TabState::new(id, path, text));
        self.active = self.tabs.len() - 1;
        self.active
    }

    /// 激活标签(越界忽略 —— 标签条渲染与消息都来自同一帧快照,理论不会
    /// 越界,防御性短路)。切换会作废在途 AI 流(归约侧负责)。
    pub fn activate(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
        }
    }

    /// 下一个标签的索引(Ctrl/Cmd+Tab 循环;不改动状态,归约侧拿去走
    /// `switch_active` 以统一作废在途流)。
    pub fn next_index(&self) -> usize {
        if self.tabs.is_empty() {
            0
        } else {
            (self.active + 1) % self.tabs.len()
        }
    }

    /// 批量关闭(#37)的目标 id 列表:`kind` 以 `index` 处的标签为基准,
    /// 左/右/其他都不含基准;顺序即关闭顺序(从左到右)。索引过期(菜单
    /// 弹出到点击之间标签已被关掉)返回空 —— 宁可 no-op 也不按漂移索引
    /// 关错标签。
    pub fn batch_close_targets(&self, kind: BatchClose, index: usize) -> Vec<u64> {
        if index >= self.tabs.len() {
            return Vec::new();
        }
        let ids =
            |range: std::ops::Range<usize>| self.tabs[range].iter().map(|tab| tab.id).collect();
        match kind {
            BatchClose::Left => ids(0..index),
            BatchClose::Right => ids(index + 1..self.tabs.len()),
            BatchClose::Others => self
                .tabs
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != index)
                .map(|(_, tab)| tab.id)
                .collect(),
            BatchClose::All => ids(0..self.tabs.len()),
        }
    }

    /// 移除标签(调用方保证脏确认已过)。关掉最后一个即换入新的空标签;
    /// 当前指针跟着修正(关的是当前或更靠前的标签时前移一位)。待确认
    /// 关闭/待改名的正是被移除的标签时,对应状态一并撤下 —— 目标已没了,
    /// 模态/浮窗不再显示,迟到的确认消息变成 no-op 而不是作用到漂移到该
    /// 索引的别的标签。
    ///
    /// 已落盘的被移除标签同时进关闭栈(#45 TabRestore):这里是唯一的
    /// 摘除点,全部关闭入口(单发确认/批量/兜底)都经此入栈,无需在归约
    /// 层到处打补丁。未落盘新标签不入栈(重开拿不回内容);封顶淘汰最旧。
    pub fn remove(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let removed = self.tabs[index].id;
        if let Some(path) = self.tabs[index].document.path.clone() {
            self.recently_closed.push(path);
            if self.recently_closed.len() > RECENTLY_CLOSED_CAP {
                self.recently_closed.remove(0);
            }
        }
        self.tabs.remove(index);
        if self.confirm_close == Some(removed) {
            self.confirm_close = None;
        }
        if self
            .rename
            .as_ref()
            .is_some_and(|rename| rename.tab_id == removed)
        {
            self.rename = None;
        }
        if self.tabs.is_empty() {
            let id = self.next_id;
            self.next_id += 1;
            self.tabs.push(TabState::new(id, None, ""));
            self.active = 0;
            return;
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if index < self.active {
            self.active -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab_paths(tabs: &TabsState) -> Vec<Option<String>> {
        tabs.tabs
            .iter()
            .map(|tab| {
                tab.document
                    .path
                    .as_ref()
                    .map(|path| path.display().to_string())
            })
            .collect()
    }

    /// 「至少一个标签」不变量:关到空自动补新的空标签,id 不复用。
    #[test]
    fn closing_last_tab_spawns_fresh_empty_one() {
        let mut tabs = TabsState::new("第一篇");
        let first_id = tabs.current().id;
        tabs.open_tab(None, "第二篇");
        tabs.remove(0);
        assert_eq!(tabs.tabs.len(), 1);
        assert_eq!(tabs.current().editor.text(), "第二篇");

        tabs.remove(0);
        assert_eq!(tabs.tabs.len(), 1, "关空后仍有标签");
        assert_eq!(tabs.current().editor.text(), "", "兜底标签为空");
        assert_ne!(tabs.current().id, first_id, "id 不复用,undo/光标全新");
    }

    /// 移除当前之前的标签,active 前移;移除之后的标签,active 不动。
    #[test]
    fn remove_adjusts_active_pointer() {
        let mut tabs = TabsState::new("甲");
        tabs.open_tab(None, "乙");
        tabs.open_tab(None, "丙");
        assert_eq!(tabs.active, 2);

        tabs.remove(0); // 移除甲:丙的索引 2→1
        assert_eq!(tabs.active, 1);
        assert_eq!(tabs.current().editor.text(), "丙");

        tabs.remove(1); // 移除丙(当前):active 钳到最后
        assert_eq!(tabs.active, 0);
        assert_eq!(tabs.current().editor.text(), "乙");
    }

    /// 按路径找标签;找不到返回 None。
    #[test]
    fn find_by_path_matches_document_identity() {
        let path = Path::new("/docs/a.md");
        let mut tabs = TabsState::new("甲");
        assert_eq!(tabs.find_by_path(path), None);
        tabs.current_mut().document.path = Some(path.to_path_buf());
        assert_eq!(tabs.find_by_path(path), Some(0));
    }

    /// 开新标签不覆盖当前缓冲;新标签自动激活。
    #[test]
    fn open_tab_activates_and_preserves_current() {
        let mut tabs = TabsState::new("甲的正文");
        tabs.open_tab(Some(PathBuf::from("/b.md")), "乙的正文");
        assert_eq!(tabs.active, 1);
        assert_eq!(tabs.current().editor.text(), "乙的正文");
        tabs.activate(0);
        assert_eq!(tabs.current().editor.text(), "甲的正文");
        assert_eq!(tab_paths(&tabs), vec![None, Some("/b.md".to_owned())]);
    }

    /// TabState::load 换入内容并重建预览(大纲与新文本同源)。
    #[test]
    fn tab_load_resyncs_preview_and_outline() {
        let mut tab = TabState::new(1, None, "旧内容");
        tab.load(Some(PathBuf::from("/x.md")), "# 新标题\n\n正文");
        assert_eq!(tab.editor.text(), "# 新标题\n\n正文");
        assert_eq!(tab.preview.outline.len(), 1, "大纲随换入重建");
        assert_eq!(tab.preview.outline[0].text, "新标题");
        assert_eq!(tab.document.path, Some(PathBuf::from("/x.md")));
    }

    /// 批量关闭目标(#37):以被右键的标签为基准,左/右/其他都不含基准,
    /// 全部含基准;返回的是稳定 id。中间标签做基准覆盖非活动目标场景
    /// (active 指向别处,目标计算与 active 无关)。
    #[test]
    fn batch_close_targets_around_anchor() {
        let mut tabs = TabsState::new("甲");
        tabs.open_tab(None, "乙");
        tabs.open_tab(None, "丙");
        tabs.open_tab(None, "丁");
        tabs.activate(3); // active 在最右,基准取中间的乙(非活动)
        let ids: Vec<u64> = tabs.tabs.iter().map(|tab| tab.id).collect();
        let anchor = 1;

        assert_eq!(
            tabs.batch_close_targets(BatchClose::Left, anchor),
            ids[0..1]
        );
        assert_eq!(
            tabs.batch_close_targets(BatchClose::Right, anchor),
            ids[2..4]
        );
        assert_eq!(
            tabs.batch_close_targets(BatchClose::Others, anchor),
            vec![ids[0], ids[2], ids[3]]
        );
        assert_eq!(tabs.batch_close_targets(BatchClose::All, anchor), ids);
    }

    /// 首/尾边界:最左标签无「左侧」目标,最右标签无「右侧」目标;唯一
    /// 标签的「其他」为空(菜单项据此禁用,归约侧空队列即 no-op)。
    #[test]
    fn batch_close_targets_first_last_boundaries() {
        let mut tabs = TabsState::new("甲");
        assert!(
            tabs.batch_close_targets(BatchClose::Others, 0).is_empty(),
            "唯一标签无「其他」目标"
        );
        assert!(
            tabs.batch_close_targets(BatchClose::Left, 0).is_empty(),
            "最左标签无「左侧」目标"
        );
        tabs.open_tab(None, "乙");
        assert!(
            tabs.batch_close_targets(BatchClose::Right, 1).is_empty(),
            "最右标签无「右侧」目标"
        );
        // 关闭全部对唯一/多个标签都有目标(关空由 remove 兜底补空标签)
        assert_eq!(tabs.batch_close_targets(BatchClose::All, 0).len(), 2);
    }

    /// 索引过期(菜单点击落到已被关闭的标签)→ 空队列,不误伤现存标签。
    #[test]
    fn batch_close_targets_stale_index_yields_empty() {
        let mut tabs = TabsState::new("甲");
        tabs.open_tab(None, "乙");
        for kind in [
            BatchClose::Left,
            BatchClose::Right,
            BatchClose::Others,
            BatchClose::All,
        ] {
            assert!(
                tabs.batch_close_targets(kind, 9).is_empty(),
                "{kind:?} 过期索引一律空"
            );
        }
        assert_eq!(tabs.tabs.len(), 2, "不误伤现存标签");
    }

    /// 显示名(#37 重命名,别名语义):别名优先于文件名;dirty 星照旧追加;
    /// 无别名回归文件名行为;label_base(浮窗预填源)永不带星。
    #[test]
    fn display_name_prefers_alias_and_appends_dirty_star() {
        let mut tab = TabState::new(1, Some(PathBuf::from("/docs/note.md")), "正文");
        assert_eq!(tab.label_base(), "note.md", "无别名显示文件名");
        tab.alias = Some("我的笔记".to_owned());
        assert_eq!(tab.label_base(), "我的笔记", "别名优先");
        assert_eq!(tab.display_name(), "我的笔记");
        tab.document.dirty = true;
        assert_eq!(tab.display_name(), "我的笔记*", "dirty 星照旧追加");
        tab.alias = None;
        assert_eq!(tab.label_base(), "note.md", "清除别名回到文件名");
        assert_eq!(tab.display_name(), "note.md*");
        // 未命名标签也可起别名(起名对象是标签,不是文件)
        let mut unnamed = TabState::new(2, None, "");
        unnamed.alias = Some("草稿".to_owned());
        assert_eq!(unnamed.display_name(), "草稿");
    }

    /// 重命名浮窗目标被移除:浮窗状态一并撤下(与 confirm_close 同款),
    /// 迟到的确认消息无从指认,不会把别名设到漂移到该索引的别的标签。
    #[test]
    fn remove_cancels_pending_rename_of_closed_tab() {
        let mut tabs = TabsState::new("甲");
        tabs.open_tab(None, "乙");
        let target = tabs.tabs[1].id;
        tabs.rename = Some(TabRename {
            tab_id: target,
            draft: "新名字".to_owned(),
            focus_pending: true,
        });
        tabs.remove(1);
        assert_eq!(tabs.rename, None, "浮窗随目标移除撤下");
        // 幸存标签不背别名
        assert_eq!(tabs.tabs[0].alias, None);
        // 移除别的标签不撤浮窗
        tabs.rename = Some(TabRename {
            tab_id: tabs.tabs[0].id,
            draft: "甲的新名".to_owned(),
            focus_pending: true,
        });
        let survivor = tabs.tabs[0].id;
        tabs.open_tab(None, "丙");
        tabs.remove(2);
        assert!(
            tabs.rename.is_some_and(|rename| rename.tab_id == survivor),
            "关掉无关标签不动浮窗"
        );
    }

    /// 关闭栈(#45):只记已落盘路径,后进先出;未落盘新标签不入栈;
    /// 关空兜底补的空标签同样无路径不污染栈。
    #[test]
    fn remove_pushes_closed_paths_lifo_ignoring_unsaved() {
        let mut tabs = TabsState::new("未落盘草稿");
        tabs.open_tab(Some(PathBuf::from("/a.md")), "甲");
        tabs.open_tab(None, "又一个未落盘");
        tabs.open_tab(Some(PathBuf::from("/b.md")), "乙");
        assert!(tabs.recently_closed.is_empty(), "未关闭前栈为空");

        tabs.remove(0); // 未落盘草稿:不入栈
        assert!(tabs.recently_closed.is_empty());
        tabs.remove(0); // /a.md
        tabs.remove(0); // 未落盘:不入栈
        tabs.remove(0); // /b.md(关空,兜底补空标签)
        assert_eq!(
            tabs.recently_closed,
            vec![PathBuf::from("/a.md"), PathBuf::from("/b.md")],
            "后关的在栈顶,未落盘的两条都没进栈"
        );
    }

    /// 封顶淘汰最旧:第 21 条入栈时第 1 条被挤掉,栈长恒 ≤ 20。
    #[test]
    fn recently_closed_cap_evicts_oldest() {
        let mut tabs = TabsState::new("起点");
        // 22 个带路径标签全部关掉(起点 + 22 个 doc 共 23 次移除;关空由
        // remove 兜底补空标签,tabs 永不清零,故按固定次数关):
        // 22 条路径入栈,超出上限 2 条
        for i in 0..=(RECENTLY_CLOSED_CAP + 1) {
            tabs.open_tab(Some(PathBuf::from(format!("/doc-{i}.md"))), "x");
        }
        for _ in 0..tabs.tabs.len() {
            tabs.remove(0);
        }
        assert_eq!(tabs.recently_closed.len(), RECENTLY_CLOSED_CAP);
        assert_eq!(
            tabs.recently_closed.first(),
            Some(&PathBuf::from("/doc-2.md")),
            "最旧的 /doc-0.md 与 /doc-1.md 被淘汰"
        );
        assert_eq!(
            tabs.recently_closed.last(),
            Some(&PathBuf::from("/doc-21.md")),
            "最新的一条在栈顶"
        );
    }
}
