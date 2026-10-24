//! 应用状态与消息骨架(docs/adr-005 §5)。
//!
//! 归约铁律:状态变更只发生在 `App::logic` 调用的 [`State::apply`];
//! `App::ui` 只读状态、只产出 [`Message`]。本文件目前落了侧边栏、文件
//! 操作与大纲三组条目,`file_tree` / `search` 等字段随对应模块接入时
//! 增量加入,完整规划见 docs/adr-005 §5.1。
//!
//! 例外:编辑器缓冲、预览快照(含大纲)与 [`OutlineCursor`] 由 `ui::editor`
//! 原地维护 —— `TextEdit` 是立即模式控件,必须拿到 `&mut` 缓冲才能绘制
//! (AGENTS.md §8「UI 与状态机天然耦合」),快照与光标又是它的派生缓存,
//! 归约进下一帧反而让预览滞后一帧。
//!
//! AI 流式追加的 dirty 与 undo 语义(P1):[`Message::AiChunk`] 走
//! `EditorBuffer::insert_chars` —— dirty 照常置位(AI 写入是真实内容,
//! 保存前与手敲同责),修订号照常推进(预览每 chunk 重建一次快照)。
//! undo 栈是 TextEdit 内建 undoer 的快照,看不到程序化插入:流式结束后的
//! 第一次 Ctrl+Z 会整体回退到最近一次用户编辑的快照(表现为「一步撤销
//! 整段 AI 续写」,redo 可恢复),用户在流式期间的手敲一并被归入同一步。
//! 在途流**绑定发起它的标签**([`State::ai_active_tab`]):切标签/开新标签
//! 不改写入目标也不中断 —— 多标签下用户理应能边等流式边编辑别的文档;
//! 只有发起标签被关闭时才作废(chunk 无处可写,见 [`State::remove_tab`])。

use crate::ai::AiState;
use crate::ai_config::AiConfig;
use crate::ai_key::AiKeyState;
use crate::bed::{BedState, BedUploadPurpose};
use crate::cli_install::CliInstallState;
use crate::clipboard::ClipboardState;
use crate::command::Command;
use crate::export;
use crate::file::{self, FileCmd};
use crate::filetree::{FileTreeSettings, FileTreeState};
use crate::git_panel::GitPanelState;
use crate::keymap::{Keymap, Shortcut};
use crate::layout::LayoutSettings;
use crate::live::RenderMode;
use crate::mcp::McpState;
use crate::search::{ReplacePlan, ReplaceReport, SearchState};
use crate::settings::SettingsState;
use crate::shortcut_overlay::ShortcutOverlayState;
use crate::tabs::{DraftRecovery, TabState, TabsState};
use crate::theme::{
    clamp_editor_font_size, clamp_line_height, Density, SkinCatalog, ThemeMode, ThemeSettings,
};
use crate::ui::emoji_panel::EmojiPanelState;
use crate::ui::image_dialog::ImageDialogState;
use crate::ui::quick_open::QuickOpenState;
use crate::ui::zen_nav::ZenNavState;
use latermd_editor::EditorBuffer;
use latermd_mcp::McpConfig;
use latermd_md::OutlineItem;
use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 搜索输入去抖间隔(roadmap 阶段 3 的既定值)。
const DEBOUNCE: Duration = Duration::from_millis(300);

/// 「跟随系统」模式下探测系统主题的间隔。
///
/// 1 秒:切系统主题后最迟 1 秒跟上,又不至于每帧查一次 dbus/注册表
/// (Linux 无统一规范,查询走 freedesktop portal,roadmap 风险 #8)。
const SYSTEM_THEME_POLL: Duration = Duration::from_secs(1);

/// 组装 AI 续写 prompt 时的文档尾部上限(字符):MockProvider 只按关键词
/// 选脚本,真实 provider 的上下文窗口截断是 app 层的职责(latermd-ai
/// trait 契约),先按 2k 字符封顶。
const AI_PROMPT_TAIL_CHARS: usize = 2000;

/// 摘要节的标题文本:插入固定用二级标题,与
/// [`latermd_md::heading_section_span`] 的定位键(层级 + 文本精确匹配)
/// 保持一致 —— 重复生成时凭它移除旧节,避免摘要堆积。
const AI_SUMMARY_HEADING: &str = "AI 摘要";

/// Emoji 面板「最近使用」的容量:去重置顶后截断(E2 起随 settings.json
/// 持久化,docs/emoji-plan.md §6.3)。
const EMOJI_RECENT_CAP: usize = 16;

/// 自动保存的停顿阈值(#18):上次缓冲改动后静置 30s 落一份 draft。
/// 取「停顿」而不是「周期」—— 敲字中途写盘毫无意义,还和保存的原子
/// rename 抢同一份 fsync。
const AUTOSAVE_IDLE: Duration = Duration::from_secs(30);

/// draft 文件的后缀(含点):`<doc>.latermd-draft`,追加在完整文件名之后
/// 而不是替换既有扩展名 —— `a.md` 落成 `a.md.latermd-draft`,文件树/搜索
/// 按扩展名过滤时天然排除(`Path::extension` 取的是最后一段)。git 状态
/// 面板按它滤掉 untracked 里的 draft(git2 如实上报,产品口径在 app 侧)。
pub(crate) const DRAFT_SUFFIX: &str = ".latermd-draft";

/// 未命名文档的 draft 子目录(挂在配置目录下,与 settings.json 同处):
/// 没有落盘身份就没有同目录锚点,状态目录是唯一确定归属的地方。
const DRAFTS_DIR: &str = "drafts";

/// 文件栏「新建」草稿的基础名(不带扩展名;预填时按盘上现状让开撞名)。
const NEW_FILE_BASE_NAME: &str = "新建文档";

/// 预填名让开撞名:`新建文档.md` → `新建文档-1.md` …… 与
/// [`crate::filetree::create_markdown`] 的让开算法同款,只是这里**只算
/// 名字不建文件** —— 真正建文件时它会再让开一次(浮窗开着期间用户自己
/// 抢先把那个名字建了),两道闸各管各的,不会互相失效。
fn unique_draft(dir: &Path, base: &str) -> String {
    let wanted = format!("{base}.md");
    let mut candidate = wanted.clone();
    let mut seq: u32 = 0;
    while dir.join(&candidate).exists() {
        seq += 1;
        candidate = format!("{base}-{seq}.md");
    }
    candidate
}

/// key 闸门拦下时的状态栏文案:指路设置菜单 → AI Provider 浮窗。
const AI_KEY_MISSING_NOTICE: &str = "未配置 API key(设置 → AI Provider)";

/// 选区续写的插入点陈旧文案(#61 M2):流进行中文档被编辑、偏移见证失配
/// 时作废流落此提示(落在发起标签,decisions-pending #116)。
const AI_STALE_INSERT_NOTICE: &str = "文档在续写期间被编辑,插入点已失效,续写已停止";

/// 润色发起时没有有效选区的文案(#61 M3):润色的素材就是选区,与续写
/// 「回退文档末」不同,没有素材只能落提示拒绝(decisions-pending #117)。
const AI_POLISH_EMPTY_SELECTION_NOTICE: &str = "请先选中要润色的文本";

/// 润色发起时选区全空白的文案(#61 M3)。
const AI_POLISH_BLANK_SELECTION_NOTICE: &str = "选中的内容是空白,没有可润色的文本";

/// 润色发起时选区超上下文上限的文案前缀/后缀(#61 M3):预算读「上下文
/// 大小」配置(#58/#110),中间填「约 N KB」。素材不能截断(截断 = 确认
/// 后丢内容),超限只能拒绝(decisions-pending #117)。
const AI_POLISH_TOO_LONG_PREFIX: &str = "选中文本超过润色上限(约 ";
const AI_POLISH_TOO_LONG_SUFFIX: &str = "KB),请缩短后重试";

/// 润色确认时的选区陈旧文案(#61 M3):浮窗期间文档被编辑、修订号见证
/// 失配 → 拒绝替换不盲写(照 #17 口径,落在发起标签)。
const AI_POLISH_STALE_NOTICE: &str = "文档在润色期间被编辑,选区位置已失效,替换已取消";

/// 润色流收尾时结果为空的文案(#61 M3):空草稿开不出可确认的浮窗。
const AI_POLISH_EMPTY_RESULT_NOTICE: &str = "润色结果为空,已放弃本次替换";

/// 侧边栏功能页签。
///
/// `Serialize/Deserialize`:外壳布局要记住「上次停在哪个视图」
/// (`layout.json`,docs/ui-shell-redesign.md §10),变体名即存档值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarTab {
    /// 文件树(P0 基础版)。
    Files,
    /// 全文搜索(P1)。
    Search,
    /// 文档大纲(P0 廉价版:点击跳编辑器光标)。
    Outline,
    /// Git(P2:改动列表 + diff + 确认式回滚 + 历史)。
    Git,
    /// 反向链接(P3 #15:谁链接了当前文档,点击跳来源)。
    Backlinks,
}

impl SidebarTab {
    /// 页签栏顺序。
    pub const ALL: [SidebarTab; 5] = [
        Self::Outline,
        Self::Files,
        Self::Search,
        Self::Git,
        Self::Backlinks,
    ];

    /// 页签栏显示名。
    pub fn label(self) -> &'static str {
        match self {
            Self::Files => "文件",
            Self::Search => "搜索",
            Self::Outline => "大纲",
            Self::Git => "Git",
            Self::Backlinks => "链接",
        }
    }

    /// 页签图标(`ui::icons` 自绘)。
    pub fn icon(self) -> crate::ui::icons::Icon {
        use crate::ui::icons::Icon;
        match self {
            Self::Files => Icon::Files,
            Self::Search => Icon::Search,
            Self::Outline => Icon::Outline,
            Self::Git => Icon::Git,
            // 复用工具条的链环:反向链接的语义就是「链接」,零新增图标
            Self::Backlinks => Icon::Link,
        }
    }
}

/// 文档派生视图快照:**大纲**与编辑器修订号绑定(P5 瘦身后的全部内容)。
///
/// 只在编辑器修订号前进(或整篇换入)时重建;`outline` 的 `span` 直接索引
/// `text` 这份快照(该字段 `cfg(test)` —— 生产不再付整篇深拷贝,故此处
/// 不用 intra-doc 链接,否则 `cargo doc` 报 unresolved link)。
///
/// **P5(live-unify)之前这里还产一份整篇渲染文本与四层偏移映射** —— 它们
/// 只为右栏对照预览存在,而预览已退役:写作渲染在 Live 侧**按块**做同样的
/// 五层改写(`LiveState::rich_block_with_base`,结果进块级 `rich_cache`),
/// 与之共享的是 `latermd_md` 的纯函数与 `ui::rich_widgets` 的绘制件,不是
/// 这份整篇产物。删掉后每次修订号前进省四遍全文扫描 + 四份映射分配。
pub struct PreviewState {
    /// 当前快照的**源文本**(大纲 `span` 索引它,与编辑器缓冲同源)。
    ///
    /// 渲染文本的偏移口径与源码不同(五层改写会位移),故这份拷贝只作
    /// 「大纲 span 锚定的文本」与快照一致性断言的锚点;渲染侧另有块级
    /// 改写产物(`LiveState::rich_block_with_base`),不读这里。
    ///
    /// **只在 `#[cfg(test)]` 下存在**:生产路径无任何消费者(全仓只有
    /// 测试读它),而它每次修订号前进都要付一次整篇 `String` 深拷贝 ——
    /// 敲一个字拷一篇,纯属为测试付费。
    #[cfg(test)]
    pub text: String,
    /// 快照对应的 [`EditorBuffer::revision`]。
    pub synced_rev: u64,
    /// 文档大纲,与 `text` 同一次重建产出,`span` 直接索引该文本。
    pub outline: Vec<OutlineItem>,
    /// `outline` 对应的修订号;`None` = 尚未算过。
    ///
    /// 与 [`Self::synced_rev`] **分开记**:大纲在「不显示」期间会被跳过若干
    /// 次修订(见 [`Self::sync_outline`]),只有一个同步位就认不出「切出来
    /// 时它已经旧了」,会一直拿旧大纲糊弄读者。
    pub(crate) outline_rev: Option<u64>,
    /// 侧栏大纲页**此刻是否正在显示**;由 `end_of_logic` 每帧写入。
    ///
    /// 初值 `true` 是给既有语义兜底:`PreviewState::new` 是「整篇换入」,
    /// 那时算一次大纲正是应有之义(换入后大纲必然要重扫)。测试里直接走
    /// `new`/`rebuild` 的路径也靠它保持「大纲随编辑同帧重算」的既有断言。
    pub(crate) outline_shown: bool,
    //
    // P5(live-unify)瘦身:`rendered` 与四层渲染映射(`offset_map` /
    // `highlight_map` / `emoji_map` / `task_map`)、`scroll_target` 已删。
    // 它们只为右栏预览存在;写作渲染在 Live 侧**按块**做同样的五层改写
    // (`LiveState::rich_block_with_base`),共享的是 `latermd_md` 的纯函数与
    // `ui::rich_widgets` 的绘制件,不是这份整篇产物。删后每次修订号前进
    // 省掉四遍全文扫描 + 四份映射分配;大纲只需要下面的 `outline`。
}

impl PreviewState {
    /// 以编辑器当前内容建立快照(文本 + 大纲)。
    ///
    /// P5 起**不再产出渲染文本**:整篇改写链(cached 四层 + 图片 URI)的
    /// 唯一消费者是已退役的右栏预览,每次修订号前进都重算它们是纯浪费。
    /// Live 侧按块改写并缓存结果(`rich_cache`),稳态帧零解析;大纲仍在
    /// 这里一次性算出侧栏要用。
    ///
    /// 源文本只在 `#[cfg(test)]` 下留一份拷贝(理由见字段注记);`outline`
    /// 直接吃缓冲的 `&str`,生产构建因此**零拷贝**。
    pub fn new(editor: &EditorBuffer) -> Self {
        let text = editor.text();
        // 顺序曾固定为 **wikilink 展开 → 高亮 `==` 链接改写 → emoji 链接
        // 改写 → 任务 checkbox 链接改写**,那条顺序纪律搬去了
        // `LiveState::rich_block_with_base` 的层序注释(五层含图片改写)。
        Self {
            outline: latermd_md::outline(text),
            #[cfg(test)]
            text: text.to_owned(),
            synced_rev: editor.revision(),
            outline_rev: Some(editor.revision()),
            outline_shown: true,
        }
    }

    /// 修订号前进后重建快照;空闲帧不得调用(会白白重解析全文)。
    pub fn rebuild(&mut self, editor: &EditorBuffer) {
        *self = Self::new(editor);
    }

    /// **击键路径**的快照同步(每帧一次,与 `rebuild` 的区别只在要不要重扫
    /// 标题):大纲**只在其正在显示时**跟着修订号重算。
    ///
    /// 实测依据(release,两处互相印证):
    /// * 独立探针量 `latermd_md::outline` 本身:200KB 1.21ms、1MB 6.89ms,
    ///   线性外推 634KB ≈ **4.3ms**;
    /// * `ui::tab_switch_perf::typing_finding_report` 量整帧:634KB 击键帧
    ///   在「侧栏停文件树」与「侧栏停大纲页」两档下差 **5.5ms**(Source
    ///   5.61 / 11.12ms,Live 10.50 / 15.70ms)。
    ///
    /// 而侧栏停在文件树 / 搜索 / Git 时这份结果**没有任何消费者** —— 敲
    /// 一个字扫一篇纯属白付。
    ///
    /// 不显示期间**不推进** `outline_rev`:同步位一推进就再没人知道大纲旧
    /// 了。留着旧值,大纲被切出来的那一帧由这里的比对补算(可见性由
    /// `end_of_logic` 每帧写入,补算最晚晚一帧,用户感知不到)。
    pub fn sync_outline(&mut self, editor: &EditorBuffer) {
        let rev = editor.revision();
        if self.synced_rev != rev {
            self.synced_rev = rev;
            #[cfg(test)]
            {
                self.text = editor.text().to_owned();
            }
        }
        if self.outline_shown && self.outline_rev != Some(rev) {
            self.outline = latermd_md::outline(editor.text());
            self.outline_rev = Some(rev);
        }
    }
}

/// 大纲面板与编辑器之间的光标协调,只存偏移、不含 egui 类型。
///
/// `ui` 产出 [`Message::OutlineItemClicked`]、`logic` 归约成 `jump_to`,
/// `ui::editor` 消费时改写 TextEdit 持久光标并交还焦点;此后每帧把实际
/// 光标位置回填到 `byte`,供大纲「当前小节」高亮。
#[derive(Default)]
pub struct OutlineCursor {
    /// 待应用的跳转目标(字符偏移,与 `CCursor.index` 同语义),消费即清空。
    pub jump_to: Option<usize>,
    /// 编辑器当前光标字节位置(上一帧值);`None` = 尚无光标信息。
    pub byte: Option<usize>,
}

/// 应用根状态。
/// 文档内查找条状态(#17):`hits` 是当前 query 的大小写不敏感全量命中
/// (字符区间);`hit` 是当前停在第几个(0-based,`None` 未定位);替换行
/// (Ctrl+H)是查找条的第二行,`replace_open` 只控制它展不展开。
#[derive(Debug, Default, PartialEq)]
pub struct FindBarState {
    pub open: bool,
    /// 替换行是否展开。整条关闭(Esc / ✕)时随 `open` 一并收起。
    pub replace_open: bool,
    pub query: String,
    /// 替换词。输入只更新按钮可用性,不自动改写文档;点「替换 / 全部」
    /// 才消费。
    pub replacement: String,
    pub hits: Vec<Range<usize>>,
    pub hit: Option<usize>,
    /// 命中缓存的**新鲜度见证**:`find_rescan` 扫描那一刻当前标签的
    /// (tab id, revision)。`EditorBuffer` 的全部内容写路径(键入/undo/
    /// redo/`replace_all`/外部重载)都推修订号,故「同标签同 rev ⇒ 文本
    /// 未变」;替换入口据此判缓存过期,过期先重扫(见
    /// [`State::refresh_stale_find_hits`])。crate 内可见只因 UI 无头
    /// 测试要用 struct 字面量构造本结构,外部无消费者。
    pub(crate) scanned_for: Option<(u64, u64)>,
    /// 卡片相对右上锚点的用户拖动偏移(#72 M2,把手累计):`(0,0)` = 原
    /// 锚点(与改前逐字节一致)。会话内保持(关了再开不重置),**不持久化**。
    pub drag_offset: eframe::egui::Vec2,
    /// 打开时的「给查找框焦点」意图:归约置位一次,UI 绘制查找框时消费
    /// (请求焦点后清除)。放在 state 而非 UI 局部,是因为 Ctrl+F 的落点
    /// 在归约(命令 → Message),那一刻 UI 还没画。
    pub want_focus: bool,
}

/// 「跳转到行」浮条状态(#60 M1,Ctrl+G):全局一条(与查找条同款,不随
/// 标签),`input` 是行号草稿(UI 原地持有,与替换词同款分工)。回车经
/// [`Message::GotoLineRequested`] 在归约里钳制并跳转,跳成后浮条关闭;
/// 与查找条互斥(decisions-pending #113):一侧打开即收起另一侧。
#[derive(Debug, Default, PartialEq)]
pub struct GotoBarState {
    pub open: bool,
    /// 行号输入草稿;打开时清空,非数字回车在 UI 侧拦下(不发消息)。
    pub input: String,
    /// 卡片相对右上锚点的用户拖动偏移(#72 M2,与 [`FindBarState`] 的
    /// `drag_offset` 同款):会话内保持,不持久化。
    pub drag_offset: eframe::egui::Vec2,
}

pub struct State {
    /// 外壳布局(左右两栏展开与否 + 左栏视图 + `layout.json` 存档,
    /// docs/ui-shell-redesign.md §10)。`left` / `right` 直接喂给各自
    /// `Panel::show_collapsible` 的 `&mut bool`:面板把手在 `ui` 里原地翻转,
    /// 命令层与自绘标题栏的切换走消息在 `logic` 归约。
    pub layout: LayoutSettings,
    /// 外壳布局的**上次写盘快照**:与 `layout` 比对决定是否真的落盘,避免
    /// 每帧 serialize + fs::write。写盘失败时不更新(下次比对仍不等,自动重试)。
    layout_written: LayoutSettings,
    /// 多标签(每个标签持有自己的缓冲/预览/大纲光标/落盘身份,#11)。
    pub tabs: TabsState,
    /// 文档内查找条(#17 最小版,Ctrl+F):全局一条(不随标签),命中
    /// 缓存随 query/文档变化重扫;跳转经 `pending_selection`(字符偏移,
    /// 与格式动作同一契约)。
    pub find: FindBarState,
    /// 「跳转到行」浮条(#60 M1,Ctrl+G):全局一条,与查找条互斥。
    pub goto: GotoBarState,
    /// 「快速打开」浮层(#24,Cmd/Ctrl+P):open/query/selected 与文件
    /// 候选快照。查询词与选中下标归 UI 原地持有(与 `image_dialog` /
    /// `emoji` 持草稿同款分工),开关与快照在归约([`Message::ToggleQuickOpen`]
    /// → [`State::toggle_quick_open`])。
    pub quick_open: QuickOpenState,
    /// 在途 AI 流的发起标签 id;`None` = 无流。发起时锁定,收尾
    /// (成功/失败/作废)清除 —— [`Message::AiChunk`] / [`Message::AiDone`]
    /// 的写入目标由它决定,与 `tabs.active` 无关:切标签不中断也不改道。
    pub ai_active_tab: Option<u64>,
    /// 编辑器右键菜单的在途编辑动作(`Message::EditAction` 的记账队列,
    /// context-menu-plan §3 M1):归约只入队,`LaterMdApp::reduce` 每帧
    /// 取走推合成事件 —— 事件必须进 logic() 阶段的输入流,ui() 阶段推
    /// 的当帧末被清空(时序红线,`edit_events` 模块文档)。取走即清,
    /// 跨帧不滞留;同帧多条动作(连点菜单)按序排队,不互相顶掉。
    pub(crate) pending_edit_action: Vec<crate::edit_events::EditAction>,
    /// 待写入系统剪贴板的文本(`Message::FileTreeCopyPath` /
    /// `Message::CopyText` 的落点):state 没有 `egui::Context`,
    /// `LaterMdApp::reduce` 每帧取走 `ctx.copy_text`,取走即清。
    pub(crate) pending_copy_text: Option<String>,
    /// 在途流的插入点(#61 M2,选区续写专用);`None` = chunk 写发起标签
    /// 文档末尾(既有语义,存量入口不挂它)。与 [`State::ai_active_tab`]
    /// 同生命周期:发起时置入,成功/失败/作废一并清除。
    pub ai_insert_at: Option<AiInsertPoint>,
    /// 选区润色会话(#61 M3):`Some` = 浮窗在场(流式进行中草稿增长,
    /// 流结束后待确认)。与 [`State::ai_insert_at`] 互斥 —— 润色的 chunk
    /// 改道进会话草稿,不写文档;生命周期:发起时置入,AiDone 后保留
    /// (待确认),确认/放弃/流失败/发起标签关闭时清除。发起新流(续写/
    /// 摘要/ai:// 链接/再次润色)一律顶掉旧会话(纯 UI 状态,零文档改动)。
    pub ai_polish: Option<AiPolishSession>,
    /// 文件树(Files 页签):根目录、最近列表与懒加载缓存。
    pub file_tree: FileTreeState,
    /// Git 面板(Git 页签 + Files 页角标,P2):状态/历史/diff 快照与
    /// 确认式回滚;刷新时机见 `git_panel` 模块文档。
    pub git: GitPanelState,
    /// 全文搜索(Search 页签):输入去抖、后台服务与结果缓存。
    pub search: SearchState,
    /// 反向链接(Backlinks 页签,#15):目标快照比对 + 防抖 + 后台直扫。
    pub backlinks: crate::backlink_panel::BacklinkState,
    /// AI 流式(MockProvider,P1 联调):provider + 接收端 + 防重入标志。
    pub ai: AiState,
    /// 后台任务完成时的统一 UI 唤醒器。生产者只拿 clone/普通回调，业务
    /// 状态仍由本线程在归约阶段收流并修改。
    pub(crate) repaint: crate::repaint::RepaintNotifier,
    /// AI Provider 凭据设置区(P2「凭据管理」):草稿、三态与凭据操作集,
    /// 读写全部经 latermd-creds 在归约发生。
    pub ai_key: AiKeyState,
    /// MCP server 运行时(配置 + 后台线程 + 调用计数,docs/mcp-plan.md)。
    pub mcp: McpState,
    /// 快捷键绑定表(用户可改,`keymap.json`;命令层从它读实际键位)。
    pub keymap: Keymap,
    /// 编辑器渲染模式(P3 Live Preview 的那个标志;源码 ↔ Live 共用同一
    /// rope buffer,切换无恢复逻辑)。
    pub render_mode: RenderMode,
    /// **本次进入分栏档是从哪一档切过来的**(2026-10-19 坤哥:「源码模式从
    /// 右往左是之前的模式,别改」)。进场动画按它分两条通道:
    /// `Live` → 列整块盖在编辑区上向右推进;`Source`/其他 → 走 egui 原生的
    /// 「从右缘向左进入」。
    ///
    /// 为什么必须存来源而不是在 UI 侧推断:两条路径的**起点视觉完全一样**
    /// (编辑区都是满宽的源码 TextEdit),单看当前 `render_mode` 无法区分
    /// 「写作 → 分栏」与「源码 → 分栏」—— 而它们要的动画方向恰好相反。
    ///
    /// 只在 `set_render_mode` 的进场分支写一次,之后整个进场动画期间
    /// 保持不变(动画跨帧,中途被改会让列在两个方向间抽搐)。
    pub split_enter_from: RenderMode,
    /// 设置对话框(外观 / 快捷键 / AI / MCP / 图片 五页)。
    pub settings: SettingsState,
    /// 「关于 LaterMD」对话框(#71 M1):开关在归约,内容只读;M2
    /// 「检查更新」的在途/结果状态在此扩展。
    pub about: crate::ui::about::AboutState,
    /// 图床(docs/image-plan.md C 段):profile 列表(`beds.json`)与在途
    /// 上传的接收端;发起/收流的归约见 [`State::request_image_upload`] /
    /// [`State::finish_image_upload`]。
    pub bed: BedState,
    /// 图片框对话框(docs/image-plan.md A 段):草稿 alt/url 归它持有,
    /// 归约只置 `open` 与消费插入,UI 经 `&mut` 改草稿 —— 与 `SettingsState`
    /// 持草稿同款分工(`TextEdit` 是立即模式控件,草稿必须能就地 `&mut`)。
    pub image_dialog: ImageDialogState,
    /// 「插入 Emoji」面板(docs/emoji-plan.md E1):open/query/group/recent。
    /// 归约置 `open`,UI 经 `&mut` 改 `query` / `group`(与 `image_dialog`
    /// 持草稿同款分工);点选插入与「最近使用」维护都在归约。
    pub emoji: EmojiPanelState,
    /// 剪贴板图片读取(docs/image-plan.md D 段):后台线程 + channel 的
    /// 接收端,生命周期与 `BedState` 同构(发起/收流/收尾三原语)。
    pub clipboard: ClipboardState,
    /// 命令行工具安装(2026-10-23 mac 反馈 #3):后台线程 + channel 的
    /// 接收端,与 `clipboard` 同构;发起入口只在 macOS 原生菜单。
    pub cli_install: CliInstallState,
    /// 快捷键蒙层(#54 M1):长按平台主修饰键 3s 的检测状态机。会话级
    /// 状态,不持久化——挂在 State 上随帧推进,切窗口/切标签不重建,
    /// 失焦帧由取消判定收口(不存在「失忆后拿残影计时」的面)。
    pub shortcut_overlay: ShortcutOverlayState,
    /// 禅定悬停标签导航(#57 M1):显隐去抖与动画值。会话级状态,不持久
    /// 化;由 `ui::zen_nav` 在禅定帧推进(UI 原地持有,与 quick_open 的
    /// 查询草稿同款分工),退出禅定时在归约复位(悬停态不跨禅定会话)。
    pub zen_nav: ZenNavState,
    /// 最近一次 AI 生成的 commit message 建议;`Some` = 建议浮窗可见。
    /// 经 [`Message::AiCommitFinished`] 置入,浮窗「关闭」或下一次生成
    /// 发起时替换/清除。
    pub ai_commit_suggestion: Option<String>,
    /// 主题(外壳 visuals 与 MarkdownStyle 的唯一事实源);每帧由 `logic`
    /// 投影到 context,切换即时生效。
    pub theme: ThemeSettings,
    /// 皮肤目录的内容(`themes/*.ron`);选皮肤时从它取内容。
    pub skins: SkinCatalog,
    /// 「跟随系统」的最近一次检测结果;`None` = 尚未检测或检测失败。
    /// 主题检测要查系统设置(Linux 走 dbus),故结果缓存在这里、由
    /// [`State::poll_system_theme`] 节流刷新,不每帧探测。
    pub system_theme: Option<ThemeMode>,
    /// 系统主题探测是否可用(设置页据此提示「检测不可用,已回落手动」)。
    pub system_theme_ok: bool,
    /// 下一次探测系统主题的时刻;`None` = 未启用跟随系统(零轮询)。
    pub system_theme_due: Option<std::time::Instant>,
    /// 主题落盘目录;`None` = 平台默认。仅为测试注入临时目录而存在,
    /// 生产恒为 `None`。
    pub(crate) settings_dir: Option<PathBuf>,
    /// 本帧被切出的标签**稳定 id**(#18):帧末归约为它落 draft(切换即
    /// 落,不等停顿 —— 切走后它不可见,是防丢的主要对象)。存 id 而非
    /// 索引,与 `confirm_close` / `ai_active_tab` 同手法:模态与关闭入口
    /// 会使索引漂移,id 不会。同帧二次切换只记最后一位切出者,更早的
    /// 由停顿路径兜底。消费即清。
    autosave_switch_out: Option<u64>,
    /// 选区 AI 浮标最近点选的动作(#61 M1,会话级):M1 只交付浮标入口,
    /// 文本语义在 M2(续写接选区尾)/M3(润色确认浮窗)接线;归约先记账,
    /// 链路测试据此断言浮标 → 消息通路。不是持久化偏好,不落盘。
    pub selection_ai_last: Option<SelectionAiAction>,
}

/// 文档落盘身份 + 未保存镜像。
///
/// `dirty` 是 [`EditorBuffer::is_dirty`] 的镜像而非第二个真源:任何编辑路径
/// (按键、IME、undo/redo)都必然先落进缓冲,因此只有缓冲自己的标志可靠;
/// 这里由 [`State::end_of_logic`] 每帧单向刷新,勿手工置位。
pub struct DocumentState {
    /// 当前文档路径;`None` = 新建后尚未保存过。
    pub path: Option<PathBuf>,
    /// 是否有未保存修改(窗口标题与工具栏的 `*` 由它驱动)。
    pub dirty: bool,
    /// 最近一次文件操作的失败提示;下一次成功操作或用户点掉时清空。
    pub notice: Option<String>,
}

impl DocumentState {
    /// 未落盘文档的显示名;另存为对话框预填名见 [`file::UNTITLED_FILE_NAME`]。
    const UNTITLED: &str = "Untitled";

    /// 文件名显示基础名(未落盘为「未命名」),**不带** dirty 星。标签的
    /// 别名显示(#37 `TabState::label_base`)与重命名浮窗预填复用它。
    pub fn base_name(&self) -> String {
        self.path
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| Self::UNTITLED.to_owned())
    }

    /// 文件名显示(未落盘为「未命名」),dirty 追加 `*`。
    pub fn display_name(&self) -> String {
        let name = self.base_name();
        if self.dirty {
            format!("{name}*")
        } else {
            name
        }
    }

    /// 窗口标题。
    pub fn window_title(&self) -> String {
        format!("LaterMD — {}", self.display_name())
    }
}

/// 初始文档:中英混排 + 标题/列表/表格/代码块,首跑即可肉眼核对预览。
const SAMPLE_MD: &str = r#"# LaterMD

欢迎!This is a live preview. 左侧编辑源码,右侧实时同步。

## 常用元素

- 列表 item
- [ ] 任务 task

**粗体**、*斜体*、`inline code` 与 [链接](https://github.com/ailater/LaterMd)。

```rust
fn main() {
    println!("你好, LaterMD!");
}
```

| 列甲 | 列乙 |
|---|---|
| 1 | 2 |
"#;

impl Default for State {
    fn default() -> Self {
        Self {
            layout: LayoutSettings::default(),
            layout_written: LayoutSettings::default(),
            tabs: TabsState::new(SAMPLE_MD),
            find: FindBarState::default(),
            goto: GotoBarState::default(),
            quick_open: QuickOpenState::default(),
            ai_active_tab: None,
            pending_edit_action: Vec::new(),
            pending_copy_text: None,
            ai_insert_at: None,
            ai_polish: None,
            file_tree: FileTreeState::default(),
            git: GitPanelState::default(),
            search: SearchState::default(),
            backlinks: crate::backlink_panel::BacklinkState::default(),
            ai: AiState::default(),
            repaint: crate::repaint::RepaintNotifier::default(),
            ai_key: AiKeyState::default(),
            mcp: McpState::default(),
            render_mode: RenderMode::default(),
            // 冷启动恢复到分栏档时初值取 `Source` ⇒ 走 egui 原生的从右缘
            // 进入。语义上正确:「向右滑」表达的是「你刚从写作档切过来」,
            // 而启动恢复并没有"上一档"这回事,不该播那条动画。真正的进场
            // 由 `set_render_mode` 覆写这个值。
            split_enter_from: RenderMode::Source,
            keymap: Keymap::builtin(),
            settings: SettingsState::default(),
            about: crate::ui::about::AboutState::default(),
            bed: BedState::default(),
            image_dialog: ImageDialogState::default(),
            emoji: EmojiPanelState::default(),
            clipboard: ClipboardState::default(),
            cli_install: CliInstallState::default(),
            shortcut_overlay: ShortcutOverlayState::default(),
            zen_nav: ZenNavState::default(),
            ai_commit_suggestion: None,
            theme: ThemeSettings::default(),
            skins: SkinCatalog::default(),
            system_theme: None,
            system_theme_ok: false,
            system_theme_due: None,
            settings_dir: default_settings_dir(),
            autosave_switch_out: None,
            selection_ai_last: None,
        }
    }
}

/// 测试态把默认 settings_dir 钉到**进程级临时目录**:裸 `State::default()`
/// 的测试(200+ 处)落到真实用户配置目录——实测 `cargo test` 并行线程
/// 并发写同一份 `file_tree.json` 把它撕裂(trailing characters),测试
/// 的临时路径还反过来污染了用户的树根。显式注入 `settings_dir` 的测试
/// 不受影响;生产构建恒为 `None`(回落平台配置目录,行为不变)。
#[cfg(test)]
fn default_settings_dir() -> Option<std::path::PathBuf> {
    Some(std::env::temp_dir().join(format!("latermd-test-settings-{}", std::process::id())))
}

#[cfg(not(test))]
fn default_settings_dir() -> Option<std::path::PathBuf> {
    None
}

/// 选区 AI 浮标的两动作(#61 M1):TextEdit 选中内容后浮标点开的菜单项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionAiAction {
    /// 续写:从选区尾继续写(M2 接线既有 AI 流通道)。
    Continue,
    /// 润色:改写选中内容,确认后整段替换选区(M3 接线确认浮窗)。
    Polish,
}

impl SelectionAiAction {
    /// 浮标菜单文案。
    pub fn label(self) -> &'static str {
        match self {
            Self::Continue => "AI 续写",
            Self::Polish => "AI 润色",
        }
    }

    /// 全部动作:菜单渲染与测试穷举共用,出现顺序即菜单行序。
    pub const ALL: [SelectionAiAction; 2] =
        [SelectionAiAction::Continue, SelectionAiAction::Polish];
}

/// 选区续写的流内插入点(#61 M2):**随流携带**的目标写入偏移。
///
/// 既有 AI 流(AiStart / ai:// 链接 / 摘要)不携带它 —— chunk 照旧写在
/// 发起标签文档末尾,行为零变化;选区续写发起时捕获选区尾偏移挂进
/// [`State::ai_insert_at`],此后每个 chunk 落在该偏移处并随写入量推进,
/// 流内始终锁在同一条缝里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiInsertPoint {
    /// 下一个 chunk 的落点(字符偏移):发起时的选区尾 + 补段空行数,
    /// 每写一块推进 delta 的字符数(字符而非字节,CJK 边界安全)。
    pub offset: usize,
    /// 修订号见证(#17 口径):发起归约(含补段空行)完成时的缓冲修订号。
    /// chunk 到达时缓冲修订号与之不符 = 用户在流中编辑过、偏移已漂移
    /// → 作废流并提示,绝不把内容写进过期的位置;流自身的写入随写随刷
    /// (AI 写不算陈旧)。
    pub rev: u64,
}

/// 选区润色会话(#61 M3):从发起(`SelectionAiActionRequested { Polish }`)
/// 到确认/放弃之间存活的浮窗状态。与 [`State::ai_insert_at`] 互斥:润色的
/// chunk **不写文档**,改道进 `draft` —— 流通道(防重入/AiDone/AiFailed
/// 生命周期)与续写共用同一套,只按会话有无分流写入目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiPolishSession {
    /// 发起标签稳定 id:确认替换与失败提示按它定位标签,不随
    /// `tabs.active` 漂移(与 `ai_active_tab` / `confirm_close` 同手法)。
    pub tab_id: u64,
    /// 待替换的选区(文档坐标**字符区间**,发起帧捕获后有序化)。确认
    /// 时按它定点替换;修订号见证失配则整个作废,绝不按过期坐标盲写。
    pub selection: (usize, usize),
    /// 修订号见证(#17 口径,照续写的插入点见证):发起归约完成时的缓冲
    /// 修订号。确认时缓冲修订号与之不符 = 浮窗期间文档被编辑过、选区
    /// 位置不可信 → 提示不盲写(decisions-pending #117)。浮窗期不锁
    /// 编辑器,用户随时可以打字,这道闸就是「编辑过就重来一次」的口径。
    pub rev: u64,
    /// 流式累积的润色草稿:chunk 逐块追加(浮窗实时显示),收尾后即
    /// 「确认/放弃」的替换素材。
    pub draft: String,
}

/// UI 事件消息:`ui` 产出、`logic` 消费(docs/adr-005 §5.1/§5.2)。
///
/// 后续变体(`SearchQueryChanged` …)随搜索模块接入加入;后台任务的结果
/// 回传也走同一入口。
// 不再整体 Copy:`OutlineItemClicked` 携带 `Range<usize>`(Clone 但非 Copy)。
// 不再 Eq:`AiConfigSaved` 携带 AiConfig(含 f32 采样参数,无 Eq)。
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// 切换侧边栏页签。
    SidebarTabChanged(SidebarTab),
    /// 请求执行文件命令(对话框与 IO 在归约中发生)。
    FileCommand(FileCmd),
    /// 关闭提示行。
    NoticeDismissed,
    /// 导出当前文档为 HTML(弹保存对话框,不触碰文档落盘身份)。
    ExportHtml,
    /// 导出当前文档为 PDF(弹保存对话框,经 headless 管线渲染并嵌入系统
    /// CJK 字体;同样不触碰文档落盘身份)。字体全失配或编码失败落提示行。
    ExportPdf,
    /// 切换明暗主题(设置菜单产出);归约里改状态并即时落盘。
    ThemeChanged(ThemeMode),
    /// 明暗主题互换(命令层「切换主题」的快捷键/菜单入口;定向选择走
    /// [`Message::ThemeChanged`])。
    ToggleTheme,
    /// 切换左侧导航栏展开/折叠(命令层 `Ctrl/Cmd+\` 与自绘标题栏「关闭
    /// 左侧」两个入口;面板把手自行翻转不走消息)。
    SidebarToggled,
    /// 切换禅定模式(docs/ui-shell-redesign.md §7;M4 实现,命令层先挂上)。
    ZenToggled,
    /// 切换「快速打开」浮层(#24):C2 的最小状态只翻转可见标志;
    /// 查询词、选中项与候选快照等完整状态与浮层绘制在 C3 接入。
    ToggleQuickOpen,
    /// 请求一次 Markdown 格式动作(docs/ui-shell-redesign.md §6.4)。
    ///
    /// 工具条按钮与快捷键两个入口同源;真正的语义全在
    /// [`crate::compose::apply`],归约侧只负责取选区、调它、把结果写回
    /// 缓冲并把新选区挂到 `TabState::pending_selection`。
    FormatRequested(crate::compose::FormatAction),
    /// 编辑器右键菜单的编辑动作(撤销/重做/剪切/复制/粘贴/全选,
    /// context-menu-plan §3 M1)。归约只记账到 `pending_edit_action`,
    /// 合成事件的推入留在持有 `egui::Context` 的 `LaterMdApp::reduce`
    /// (logic() 阶段,时序红线见 `edit_events` 模块文档)。
    EditAction(crate::edit_events::EditAction),
    /// 打开「图片框」对话框(工具条 Image 按钮 / `Cmd/Ctrl+Shift+I` 产出,
    /// docs/image-plan.md A 段)。归约置 `image_dialog.open` 并按选区预填
    /// alt —— 选中文字就是要保留的替代文字(`compose::insert_image` 的约定)。
    ImageDialogOpened,
    /// 图片框点「插入」:归约里走 [`crate::compose::insert_image`] 写入活动
    /// 标签,新选区落在 alt 位;对话框关闭并清空草稿。url 为空是防御分支
    /// (UI 已禁用按钮),不动文档只关框。
    ImageInserted {
        alt: String,
        url: String,
    },
    /// 图片框点「取消」:仅关闭对话框并清空草稿,文档与选区不动。
    ImageDialogClosed,
    /// 图片框点「浏览…」(docs/image-plan.md B 段本地文件来源):归约里弹
    /// 图片选择框,选中即**复制**进 `<doc名>.assets/`(撞名 -1/-2 改名,
    /// 绝不覆盖),把相对地址回填 url 草稿;alt 为空时补文件名。文档未
    /// 落盘只落提示不复制(`.assets/` 必须与文档同目录,没有目录就没有
    /// 锚点)。对话框保持打开等用户点「插入」—— 插入仍是唯一的文本写入。
    ImageFilePickRequested,
    /// 图片框点「选文件并上传…」(docs/image-plan.md C 段图床来源):归约里
    /// 弹图片选择框,选中即**关框**并发起后台上传(阻塞的 ureq 在后台线程,
    /// UI 不卡)。取消选择则一切不动。收尾见
    /// [`Message::ImageUploadFinished`]。
    ImageUploadRequested,
    /// Ctrl+V 且剪贴板无文本(docs/image-plan.md D 段):发起后台剪贴板
    /// 图片读取(arboard,X11 握手是阻塞 IO,不进归约)。收尾见
    /// [`Message::ImagePasteFinished`]。
    ImagePasteRequested,
    /// 剪贴板读取收尾(后台线程经 channel 回传):`Ok(png 字节)` 落
    /// `.assets/` 并在当前光标处插相对路径引用(alt 空,粘贴图无可推断的
    /// 说明);`Err` 只落提示行,文档与选区绝不动(与图床上传失败同口径,
    /// image-plan §4.3)。
    ImagePasteFinished {
        /// 成功 = PNG 字节(解码与重编码已在后台线程完成);失败 = 面向
        /// 用户的错误文案。
        result: Result<Vec<u8>, String>,
    },
    /// 请求安装命令行工具(2026-10-23 mac 反馈 #3;macOS 原生菜单
    /// 「安装命令行工具…」产出,其余平台无入口):归约校验运行环境
    /// (仅 .app 打包运行才装)后发起后台安装线程,osascript 的管理员
    /// 授权弹窗在后台线程,不卡 UI。收尾见
    /// [`Message::CliInstallFinished`]。
    // 唯一的生产构造方在 mac_menu(macOS cfg);Linux 上由归约测试构造。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    InstallCliRequested,
    /// 命令行安装收尾(后台线程经 channel 回传):结果只落提示行,
    /// 文档与一切编辑状态不动(与图床上传失败同口径);重试 = 再点
    /// 一次菜单(`ln -sf` 覆盖式,幂等)。
    CliInstallFinished {
        /// 成功 = 软链已建好;失败 = 面向用户的错误文案。
        result: Result<(), String>,
    },
    /// 拖入图片文件(docs/image-plan.md D 段):归约里读文件、过白名单与
    /// 5MB 上限、落 `.assets/` 并在光标处插引用。文件读取是本地磁盘
    /// (毫秒级),同步做,不上后台线程 —— 与文件树打开文件同口径。
    ImageFileDropped(PathBuf),
    /// 开/关「插入 Emoji」面板(docs/emoji-plan.md E1):工具条笑脸按钮 /
    /// `Cmd/Ctrl+Shift+E` 产出 `true`,Esc / 点选插入后的关闭产出 `false`。
    /// 归约只翻 `open`(开时清搜索词,分类与最近使用沿用)。
    EmojiPickerToggle(bool),
    /// Emoji 面板点选,载荷为该格的字符:归约走 [`crate::compose::insert_emoji`]
    /// 写入活动标签(新选区 collapsed 落在 emoji 之后)、关闭面板并记入
    /// 「最近使用」(去重置顶)。空载荷是防御分支,不动文档只关面板。
    EmojiInserted(String),
    /// 后台上传收尾(后台线程经 channel 回传,每帧归约收流翻成此消息):
    /// **只接受最新序号**(`seq` 与当前序号相等才处理,防旧请求覆盖 ——
    /// 照抄 AI 流式防重入手法,旧结果静默丢弃)。`Ok(url)` 按发起时的用途
    /// 落地:插入型把 URL 写进**发起标签**(新选区落 alt 位),测试型只在
    /// 设置页回显;`Err` 只落提示行,**绝不动文档与选区**(image-plan §4.3)。
    ImageUploadFinished {
        /// 发起时分配的序号。
        seq: u64,
        /// 成功 = 可直接落 Markdown 的 URL;失败 = 面向用户的错误文案。
        result: Result<String, String>,
    },
    /// 保存图床 profile(设置「图片」页「保存」):归一化 → 新 profile 分配
    /// id → token(若填)写系统凭据(service=latermd-bed / account=id)→
    /// 落 `beds.json`。token 不落任何文件;落盘失败不改内存列表。
    BedProfileSaved {
        /// 草稿定稿的 profile。
        profile: latermd_bed::BedProfile,
        /// 新 token;`None`/空白 = 不改已存凭据。
        token: Option<String>,
    },
    /// 删除图床 profile:`beds.json` 移除 + 系统凭据删除(幂等)。
    BedProfileDeleted {
        id: String,
    },
    /// 设置页「测试上传」:弹图片选择框,选中即后台上传,结果回显在该页
    /// (不插入任何文档)。
    BedTestUploadRequested {
        profile_id: String,
    },
    /// 请求为文件树选择新根目录(归约里弹目录对话框)。
    FileTreeRootPick,
    /// 把文件树根目录切到最近列表中的某一项(不经对话框)。
    FileTreeRootSelected(PathBuf),
    /// 文件栏「新建」按钮:挂起新建浮窗,载荷是**目标目录**(当前文档所在
    /// 目录,无已落盘文档时回落文件树根)。无根目录时不发此消息。
    FileTreeNewFileRequested(PathBuf),
    /// 新建浮窗「确定」:在 [`Self::FileTreeNewFileRequested`] 给的目录里
    /// 建空 Markdown 文件、失效该目录树缓存、开新标签打开。失败只落提示行。
    FileTreeNewFileConfirmed,
    /// 新建浮窗「取消」。
    FileTreeNewFileCancelled,
    /// 文件树右键菜单「新建文件夹」(context-menu-plan §3 M2):归约挂
    /// `file_tree.new_folder` 浮窗,确认走 [`crate::filetree::create_folder`]。
    FileTreeNewFolderRequested(PathBuf),
    /// 新建文件夹浮窗点「创建」:落盘目录 + 失效缓存 + 展开。
    FileTreeNewFolderConfirmed,
    /// 新建文件夹浮窗点「取消」。
    FileTreeNewFolderCancelled,
    /// 文件树右键菜单「重命名」(**真实文件改名**,与标签 #37 的显示
    /// 别名相区分):归约挂 `file_tree.rename` 浮窗,确认走
    /// [`crate::filetree::rename_entry`]。
    FileTreeRenameRequested(PathBuf),
    /// 重命名浮窗点「确定」:合法性校验 + `fs::rename` + 打开中标签
    /// 换绑新路径(缓冲/dirty 保留)+ named_draft 迁移。
    FileTreeRenameConfirmed,
    /// 重命名浮窗点「取消」。
    FileTreeRenameCancelled,
    /// 文件树右键菜单「删除」:归约挂 `file_tree.delete_confirm` 确认
    /// 浮窗;真正的删除走系统废纸篓(`trash_delete`),可恢复。
    FileTreeDeleteRequested(PathBuf),
    /// 删除确认浮窗点「删除」:打开中 dirty 文件拒绝;干净标签先关闭
    /// 再删;删目录连子文档的标签一起关。
    FileTreeDeleteConfirmed,
    /// 删除确认浮窗点「取消」。
    FileTreeDeleteCancelled,
    /// 复制路径(文件树行 / 标签右键共用):`relative` 为真时取相对
    /// 文件树根的路径(不在根下则原样绝对路径)。文本经
    /// `pending_copy_text` 出口落系统剪贴板。
    FileTreeCopyPath {
        path: PathBuf,
        relative: bool,
    },
    /// 在系统文件管理器中显示(Linux 打开父目录,mac/win 定位文件)。
    FileTreeReveal(PathBuf),
    /// 通用「复制文本到剪贴板」出口(state 无 `egui::Context`,
    /// `LaterMdApp::reduce` 每帧取走 `ctx.copy_text`)。预览右键
    /// 「复制链接/图片地址」同走这条。
    CopyText(String),
    /// 文件栏「刷新」按钮:重扫**根 + 当前展开中的全部目录**(树是懒加载
    /// 缓存,外部增删改文件在 UI 层看不出来),并按新状态刷新 Git 角标 ——
    /// 外部 `git add/commit` 后角标同样滞后。归约里同步做完,UI 无状态。
    FileTreeRefreshRequested,
    /// 点击文件树目录行,载荷为目录路径。
    FileTreeToggled(PathBuf),
    /// 点击文件树文件行,载荷为文件路径。
    FileSelected(PathBuf),
    /// 搜索输入变化(文本/大小写开关由 `ui` 原地写入 `SearchState`,消息
    /// 本身无载荷):归约里取消旧搜索并顺延去抖。
    /// 查找条开/关(Ctrl+F / ✕ / Esc)。开时预填当前选区文字(编辑器惯例)。
    FindBarToggled(bool),
    /// 查找词变化:重扫命中并跳第一个。
    FindQueryChanged(String),
    /// 跳上/下一个命中(Enter / Shift+Enter,环绕)。
    FindNext {
        backwards: bool,
    },
    /// 替换行开/关(Ctrl+H):已全开时视为「再按一次」,收替换行、查找条
    /// 保留;否则展开替换行,查找条未开则同开(选区预填与重扫复用
    /// [`Message::FindBarToggled`] 的口径)。
    ReplaceBarToggled(bool),
    /// 替换当前命中(`find.hit` 指向)为替换词,随后跳下一个(环绕)。
    ReplaceCurrent,
    /// 全部替换:按当前命中一次写入(单条 undo),随后重扫。
    ReplaceAllInDoc,
    /// 「跳转到行」浮条开/关(#60 M1,Ctrl+G):开时清行号草稿并收起查找
    /// 条(两者互斥,decisions-pending #113);关即关(Esc / ✕)。
    GotoBarToggled(bool),
    /// 跳转到行,载荷为 1 起行号(UI 侧已解析,非数字不发):归约里钳制到
    /// 1..=总行数、光标落该行行首(`cursor.jump_to`),浮条关闭。空文档
    /// 总行数为 1,任何载荷都跳第 1 行。
    GotoLineRequested {
        line: usize,
    },
    /// 插入/更新目录(#66 M2,命令层 `InsertToc` 与菜单/快捷键的共同归约
    /// 入口):有可收录标题(一至三级)则在文档头插入标记包围的 TOC 块
    /// (首次)或整块替换既有块(再执行,单次定点写入 = 单步 undo,口径
    /// decisions-pending #127);无可收录标题只落提示行,文档不动。
    InsertToc,
    SearchQueryChanged,
    /// 左栏自动展开并停 Search 页(标题栏搜索胶囊聚焦/输入变化时由
    /// `ui` 发):对齐 VS Code「唤起搜索即见结果」的动线。归约在禅定
    /// 模式下忽略 —— 禅定期间 `left=false` 是被 `enter_zen` 借走的临时
    /// 态,擅自翻开既破坏沉浸,还会被落盘快照悄悄还原。
    SearchPanelRevealed,
    /// 侧边栏「全部替换」按钮:归约里按当前命中折算受影响文件数,挂起
    /// 确认浮窗(`search.confirm_replace`)。跨文件写盘属破坏性操作,
    /// 与关脏标签 / Git 回滚同一套确认纪律。
    SearchReplaceAllRequested,
    /// 确认浮窗「全部替换」:归约逐文件执行替换并产出回执
    /// (`search.last_replace`),随后重发一次搜索让命中列表收敛。
    SearchReplaceAllConfirmed,
    /// 确认浮窗「取消」:只撤浮窗,不动任何文件。
    SearchReplaceAllCancelled,
    /// 去抖到点,按当前输入与根目录发起搜索。
    SearchRequested,
    /// 反向链接防抖到点(#15):按当前(根, 文档)发起后台扫描。归约侧
    /// 产出(`layout.rs` 的 reduce,与 `SearchRequested` 同款),UI 不直接发。
    BacklinksRequested,
    /// 点击搜索结果,载荷为(文件路径, 1 起行号):归约里打开该文件并把
    /// 光标跳到行首。
    SearchResultClicked(PathBuf, usize),
    /// 点击大纲条目,载荷为标题的源码字节区间。
    OutlineItemClicked(Range<usize>),
    /// 点击预览里的 `[[wikilink]]`,载荷为目标文档名:归约里在文档库内找同名
    /// 文档并打开(找不着落提示行,不静默无反应)。反向链接面板的来源跳转
    /// (#15)同走此消息 —— 载荷是来源相对路径剥 md 后缀
    /// (`crate::backlink_panel::jump_target`),与 `[[..]]` 目标写法对称。
    WikilinkClicked {
        target: String,
    },
    /// 点击指向本地文档的普通链接(`[标题](./note.md)` / `(doc.md#锚点)` /
    /// `(#锚点)`,载荷为**原始 href**):归约里按 [`crate::doclink`] 解析到
    /// 本地文档并打开,带锚点则落标题行。只在 `classify == Local` 时发出
    /// —— 外部 scheme 照走系统浏览器,应用内协议各有归属。
    DocLinkClicked {
        href: String,
    },
    /// 发起 AI Mock 流式续写(命令层入口);流式进行中在归约里被忽略
    /// (防重入,见 [`AiState::start_with_repaint`])。
    AiStart,
    /// AI 流式的一个增量块,载荷为要追加到文档末尾的原文。后台线程产出,
    /// 每帧由归约侧从 channel 收流翻成本消息(见 `State::poll_ai`)。
    AiChunk {
        delta: String,
    },
    /// AI 流式成功收尾。
    AiDone,
    /// AI 流式失败,载荷为面向用户的错误描述(provider 契约:done 且
    /// delta 非空)。失败文本不写入文档。
    AiFailed(String),
    /// 点击 ai:// 链接,载荷为 [`crate::ai_link::parse`] 的结果:`Ok` 为解码
    /// 后的提示词,归约走与 [`Message::AiStart`] 同一条流式启动路径(防重入
    /// 同样生效);`Err` 为未实现动作 / 解析失败的提示语,落状态栏不执行。
    AiLinkClicked {
        prompt: Result<String, String>,
    },
    /// 请求生成 commit message(命令层入口):staged diff 优先、无 staged
    /// 用 working tree diff,喂 provider 合成单行 subject。流式进行中在
    /// 归约里被忽略(防重入,与 [`Message::AiStart`] 同一道闸)。
    AiCommitRequested,
    /// 选区 AI 浮标菜单点选(#61 M1),载荷为所选动作与点选帧的选区
    /// (文档坐标字符区间,源码模式是 TextEdit 持久选区、Live 模式是活动
    /// 块内选区换算到全文的坐标;`None` = 点选帧没读到选区)。M2 起「续写」
    /// 在归约里接线流式通道:选区就是捕获素材,随消息走而不读
    /// `TabState::selection`(该字段只有源码模式回填,Live 下是陈旧值);
    /// M3 起「润色」接线确认浮窗(发起润色流,结果进浮窗不进文档)。
    SelectionAiActionRequested {
        action: SelectionAiAction,
        selection: Option<(usize, usize)>,
    },
    /// 确认润色浮窗(#61 M3):整段替换选区为润色结果。替换前比对手续
    /// 修订号见证(浮窗期间文档被编辑 → 提示不盲写),替换走 rope 定点
    /// 编辑(TextEdit 内建 undoer 一次 Ctrl+Z 回原状)并把新选区挂
    /// `pending_selection`。
    SelectionAiPolishConfirmed,
    /// 放弃润色浮窗(#61 M3):关窗零改动 —— 文档、选区、修订号一概不动;
    /// 流式进行中放弃 = 作废在途流(剩余 chunk 丢弃)。
    SelectionAiPolishDismissed,
    /// 点击预览/Live 富渲染里的任务列表 checkbox(#63),载荷为该任务标记
    /// `[` 的**源码**字节偏移(ui 层已把链接载荷坐标逆穿映射):归约把
    /// 标记中段的勾选字符翻转(` `↔`x`),单次 rope 定点编辑、单步 undo;
    /// 形态核验不过(偏移过期/标记已被改写)静默忽略,不写错位置。
    TaskCheckboxToggled {
        /// `[` 在源码中的字节偏移。
        byte: usize,
    },
    /// 请求生成摘要(命令层入口):文档全文喂 provider,移除旧「AI 摘要」
    /// 节后在文档末尾以引用块形式流式追加新要点。流式进行中在归约里被
    /// 忽略(防重入,同一道闸)。
    AiSummaryRequested,
    /// commit message 后台任务完成。序号用于丢弃迟到结果，标签 id 用于把
    /// 错误写回发起文档而不是用户当前切到的其他标签。
    AiCommitFinished {
        seq: u64,
        tab_id: u64,
        result: Result<String, String>,
    },
    /// 关闭 commit message 建议浮窗。
    AiCommitDismissed,
    /// 保存 AI API key 草稿到系统凭据(设置浮窗「保存」):归约里经
    /// latermd-creds 写入,成功置已配置并清空草稿(UI 不回显值);空白
    /// 拒绝;后端失败落提示行并转 [`Message::AiKeyBackendUnavailable`]。
    AiKeySaved,
    /// 从系统凭据删除 AI API key(设置浮窗「清除」):幂等,成功置
    /// 未配置;后端失败同 [`Message::AiKeySaved`] 的降级。
    AiKeyCleared,
    /// 凭据后端不可用:置不可用状态,设置浮窗状态行提示回退环境变量。
    /// 归约内部的结果消息(保存/清除失败时再归约),UI 不直接产出。
    AiKeyBackendUnavailable,
    /// 保存 AI 配置(设置页 AI 区「保存」):归一化 → 落 `ai.json` → 即时
    /// 重装配 provider(无需重启)。失败只落提示行,不改内存配置。
    AiConfigSaved(AiConfig),
    /// 设置页「获取模型列表」(#58 M2):归约里按草稿装配列表来源并发起
    /// 后台拉取(std 线程 + mpsc,与 AI 流式同款,零 tokio)。Mock 不联网
    /// 直接忽略(UI 已禁用按钮,防御);拉取中忽略(防重入)。收尾见
    /// [`Message::AiModelsFetchFinished`]。
    AiModelsFetchRequested,
    /// 模型列表收尾(后台线程经 channel 回传,每帧归约收流翻成此消息):
    /// `Ok(列表)` 填充下拉候选并清错误行;`Err(文案)` 显示错误行,旧候选
    /// 保留。消息只可能来自当前在途请求 —— 防重入在 `start` 把关(在途
    /// 忽略,接收端不中途替换),见 `ModelListState`。
    AiModelsFetchFinished {
        /// 成功 = 保序去重后的模型名列表;失败 = 面向用户的错误文案。
        result: Result<Vec<String>, String>,
    },
    /// 给某命令绑定新键位(快捷键页捕获到按键后由归约侧产出)。撞键时
    /// **拒绝**并落提示 —— 不静默抢占另一个命令的键位。
    KeymapAssign {
        cmd: Command,
        shortcut: Shortcut,
    },
    /// 清除某命令的键位(此后只能从菜单 / 工具栏触发)。
    KeymapCleared(Command),
    /// 某命令的键位恢复出厂。
    KeymapReset(Command),
    /// 全部键位恢复出厂。
    KeymapResetAll,
    /// 三态视图循环切档:`源码 → 写作 → 源码+对照 → 源码`,只翻标志,
    /// 不碰缓冲与光标。`Ctrl/Cmd+/` 与视图菜单的快捷入口(标题栏三段开关
    /// 走 [`Message::SetRenderMode`] 直达目标档,不循环)。
    ToggleLivePreview,
    /// 直接把视图设到指定档(三段开关的「点谁到谁」语义)。
    SetRenderMode(crate::live::RenderMode),
    /// 对照列开合:非分栏档 ⇄ 分栏档(标题栏 PanelRight 按钮入口)。
    /// 开 = 切到分栏;关 = 回源码。写作档点击先切到分栏档(中央变源码)。
    CompareColumnToggled,
    /// 打开设置对话框并切到指定分页(工具栏齿轮 / 菜单「设置…」入口)。
    SettingsOpened(crate::settings::SettingsTab),
    /// 打开「关于 LaterMD」对话框(帮助菜单入口);归约只置 `about.open`。
    AboutOpened,
    /// 关闭「关于 LaterMD」(窗 X / 蒙层点击 / Esc 三出口在 UI 层汇成此
    /// 消息);归约只翻 `about.open`。在途检查不随关窗作废(照模型列表
    /// 拉取:收流照常落地,重开可见)。
    AboutClosed,
    /// 关于窗「检查更新」(#71 M2):发起后台拉取(std 线程 + mpsc,照
    /// `AiModelsFetchRequested` 同款,零 tokio)。拉取中忽略(防重入)。
    /// 收尾见 [`Message::AboutUpdateCheckFinished`]。
    AboutUpdateCheckRequested,
    /// 检查更新收尾(后台线程经 channel 回传,每帧归约收流翻成此消息):
    /// 成功按版本比较落 最新/有更新/无法判断,失败落一句话错误(可重试)。
    /// 消息只可能来自当前在途请求 —— 防重入在 `start` 把关(见
    /// `ui::about::UpdateCheckState`)。
    AboutUpdateCheckFinished {
        /// 成功 = 最新 release 的 tag 原文(请求成功但响应缺 `tag_name`
        /// 为 `None`,归「无法判断」);失败 = 面向用户的错误文案。
        result: Result<Option<String>, String>,
    },
    /// 激活某标签(标签条点击 / 文件树与搜索跳转的已开路径)。
    TabActivate(usize),
    /// 请求关闭某标签:脏则弹确认模态,干净直接关。
    TabCloseRequested(usize),
    /// 关闭当前标签(Ctrl+W 的归约入口)。
    TabCloseActive,
    /// 确认模态里确认关闭(丢弃该标签未保存的修改)。
    TabCloseConfirmed,
    /// 确认模态取消,不关。
    TabCloseCancelled,
    /// 批量关闭(#37 标签条右键菜单):`kind` 以 `index` 处的**被右键标签**
    /// 为基准(菜单弹出帧的索引快照,归约里立即换算成稳定 id 队列,不等
    /// 索引存活到下一帧)。目标为空时(如最左标签的「关闭左侧」)UI 侧
    /// 已禁用,这里防御性 no-op。
    TabBatchCloseRequested {
        kind: crate::tabs::BatchClose,
        index: usize,
    },
    /// 右键菜单「重命名」(#37,**显示别名**语义):打开重命名浮窗,目标
    /// 是 `index` 处**被右键的标签**(菜单弹出帧的索引快照,归约立即转
    /// 稳定 id)。过期索引 no-op。别名只改标签条显示文本 —— 不改盘上
    /// 文件名、不影响保存目标。
    TabRenameRequested {
        index: usize,
    },
    /// 重命名浮窗「确定」:草稿 trim 后非空才置别名;空名拒绝并提示
    /// (浮窗保留)。目标标签已被关掉时 no-op。
    TabRenameConfirmed,
    /// 重命名浮窗「取消」。
    TabRenameCancelled,
    /// 切换标签条标题宽度模式(#37 右键菜单「缩短标题/完整标题」):
    /// 整条标签条统一生效。只改显示偏好并落盘 settings.json —— 不动
    /// 任何标签的路径/缓冲/dirty/别名,更不触碰盘上文件。
    TabTitleWidthChanged(crate::theme::TitleWidthMode),
    /// 切到下一个标签(Ctrl/Cmd+Tab 循环)。
    TabNext,
    /// 恢复最近关闭的标签(#45,Cmd/Ctrl+Shift+T):从关闭栈弹出最近一条
    /// 已落盘路径按 `open_path` 既有语义重开(已开则激活,不开第二个)。
    /// 成功(含激活)出栈,连续触发逐条回走整条栈;失败(文件被外部删/移,
    /// 提示行说明)同样出栈并继续下一条(decisions-pending #70);栈空 no-op。
    TabRestore,
    /// 恢复条「恢复」(#18):载荷为标签稳定 id。把盘上孤儿 draft 的内容
    /// 读进该标签缓冲并置 dirty(整篇替换,Ctrl+Z 一步回退),随后删
    /// draft、清待恢复状态。读取失败不 panic,提示行说明(见
    /// [`State::recover_draft`])。
    DraftRecovered {
        tab_id: u64,
    },
    /// 恢复条「丢弃」(#18):载荷为标签稳定 id。直接删该标签的孤儿
    /// draft、清待恢复状态;缓冲本就是盘上版本,一动不动。提示行留痕
    /// 供追溯。
    DraftDiscarded {
        tab_id: u64,
    },
    /// 点击 Git 页改动列表里的文件,载荷为相对仓库根的路径:归约里选中
    /// 并读它的 diff(列表外的过期路径被忽略)。
    GitFileSelected(String),
    /// 点击历史提交行，展开该提交的文件变更列表。
    GitHistorySelected(String),
    /// 点击历史提交中的文件，加载 parent → commit diff。
    GitCommitFileSelected {
        commit_hash: String,
        path: String,
    },
    /// 点击「回滚此文件」,载荷为相对仓库根的路径:归约里只置确认模态,
    /// checkout 在用户显式确认之后。
    GitCheckoutRequested(String),
    /// 确认模态里确认回滚:执行 latermd-git 唯一的写操作并立即刷新状态;
    /// 失败文案进提示行。
    GitCheckoutConfirmed,
    /// 确认模态取消,不触碰工作区。
    GitCheckoutCancelled,
    /// 切换 Git 页 diff 区视图(#53 M2):双栏/统一。纯显示偏好,不落盘
    /// (会话内状态,重启回默认双栏),归约只写 `git.diff_view`。
    GitDiffViewChanged(crate::git_panel::DiffView),
    /// 关闭主工作区 Git 审阅,返回当前编辑器。
    GitDiffWorkspaceClosed,
    /// 查看单个文件的全部修改记录,载荷为**绝对路径**(文件树右键 / 标签
    /// 右键的入口手上只有绝对路径):归约换成相对仓库根的路径后查询,并
    /// 把侧栏切到 Git 页的「文件历史」分区。文件不在仓库内时留提示
    /// 行,不查。
    GitFileHistoryRequested(PathBuf),
    /// 刷新 Git 页(顶部刷新按钮 / 展开「存储库」分区触发):立即重读
    /// 未提交改动、提交历史与文件历史(文件历史按当前文档同步,见
    /// [`GitPanelState::history_target`])。
    GitRefreshRequested,
    /// 保存 MCP 配置(设置页 MCP 页「保存」):归一化 → 落 `mcp.json` → 按
    /// 开关起停后台服务(**默认关闭**,开了才监听回环端口)。
    McpConfigSaved(McpConfig),
    /// 选择皮肤(设置页外观页下拉);`None` = 出厂默认正文样式。
    ThemeSkinSelected(Option<String>),
    /// 把当前正文样式导出成皮肤文件(`themes/<name>.ron`)并选中它。
    ThemeSkinExported {
        name: String,
    },
    /// 切换界面密度(宽松 / 标准)。
    ThemeDensityChanged(Density),
    /// 编辑器与预览正文的**基准字号**变化(#23 F2):外观页滑杆拖动中值
    /// 变化的每帧产出,归约里钳进 12..=24(pt,CJK 可读性下限)后落
    /// `theme.editor_font_size` 并即时写 settings.json(与切密度同款通路;
    /// 滑杆自带 range,但钳制是**不依赖 UI** 的防线 —— 消息可被测试或其他
    /// 调用方直塞越界值)。F3 才把字段投影到渲染。
    EditorFontSizeChanged(f32),
    /// 正文行距倍率变化(#23 F2):同上,钳进 1.2..=2.0。
    EditorLineHeightChanged(f32),
    /// 源码 minimap 开关设值(#55 M2):外观页复选框产出,落
    /// `theme.show_minimap` 并即时写 settings.json(与排版偏好同款通路;
    /// 全局偏好,非每标签)。
    ShowMinimapToggled(bool),
    MinimapAutoChanged(bool),
    /// 源码 minimap 开关翻转(#67 M2):视图菜单条目/快捷键产出 —— 菜单
    /// 与快捷键是「翻转」语义(现值在 State,消息层拿不到),与设置页的
    /// 带值消息([`Message::ShowMinimapToggled`])分立;归约同样即时写
    /// settings.json(与 `ToggleTypewriter` 同构)。
    ToggleMinimap,
    /// 打字机模式开关设值(#64 M1):外观页复选框产出,落
    /// `theme.show_typewriter` 并即时写 settings.json(minimap 开关
    /// 同款通路;全局偏好,源码与 Live 两模式共用一个开关)。
    TypewriterToggled(bool),
    /// 打字机模式开关翻转(#64 M1):视图菜单条目/快捷键产出 —— 菜单与
    /// 快捷键是「翻转」语义(现值在 State,消息层拿不到),与设置页的
    /// 带值消息([`Message::TypewriterToggled`])分立;归约同样即时写
    /// settings.json。
    ToggleTypewriter,
    /// 专注模式开关设值(#64 M2):外观页复选框产出,落
    /// `theme.show_focus_mode` 并即时写 settings.json(typewriter 开关
    /// 同款通路;全局偏好,仅 Live 模式淡化非活动块)。
    FocusModeToggled(bool),
    /// 专注模式开关翻转(#64 M2):视图菜单条目/快捷键产出 —— 菜单与
    /// 快捷键是「翻转」语义(现值在 State,消息层拿不到),与设置页的
    /// 带值消息([`Message::FocusModeToggled`])分立;归约同样即时写
    /// settings.json。
    ToggleFocusMode,
    /// 选区 AI 浮标开关设值(#61 配置面):外观页复选框产出,落
    /// `theme.show_selection_ai` 并即时写 settings.json(minimap 开关
    /// 同款通路;关闭后同样两个动作仍可从右键菜单发起)。
    SelectionAiToggled(bool),
    /// 禅定导航显示方式切换(#57 M2):外观页三态选择产出,落
    /// `theme.zen_nav` 并即时写 settings.json(与 minimap 开关同款通路)。
    /// 归约同时复位会话级悬停态:新模式从干净状态起算(常显不继承旧会话
    /// 的去抖残值,关闭后再开悬停不带着残置可见位)。
    ZenNavModeChanged(crate::theme::ZenNavMode),
    /// 设置页改「默认视图模式」(写作/源码/对照/上次选择):落
    /// `theme.default_view_mode` 并即时写 settings.json。只影响下次启动
    /// (`State::restore_layout` 消费),当前会话的视图不动 —— 改偏好不
    /// 等于要立刻换视图。
    DefaultViewModeChanged(crate::theme::DefaultViewMode),
}

/// 字符偏移 → 字节偏移(替换命中落 `String` 前的换算;落在多字节字符
/// 中间时归到起点,与 `EditorBuffer::byte_to_char` 同口径,越界钳到末尾)。
fn char_to_byte(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map_or(text.len(), |(byte, _)| byte)
}

/// 命中列表里的去重文件路径(保序):替换按文件粒度执行,一次替换覆盖
/// 该文件当前文本上的全部命中。后台扫描按文件整读整发,命中天然按文件
/// 连续,`contains` 去重只是防御(上限 [`crate::search::MAX_HITS`],平方
/// 量级无虞)。
fn unique_search_files(hits: &[crate::search::SearchResult]) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for hit in hits {
        if !paths.contains(&hit.path) {
            paths.push(hit.path.clone());
        }
    }
    paths
}

/// 按配置草稿装配模型列表来源(`request_ai_models` 的纯映射,单测不出网):
/// Mock 不联网返回 `None`;需要 key 的 provider 带上凭据(缺失给空串,
/// 由后台线程回「未配置 API key」文案);Ollama 本地无鉴权。base_url
/// 原样透传 —— 尾斜杠与空白由 latermd-ai 的端点拼装吃掉(与各 adapter
/// `url()` 同口径)。
fn models_source_for(
    config: &AiConfig,
    api_key: Option<String>,
) -> Option<latermd_ai::ModelsSource> {
    use latermd_ai::ModelsSource;
    match config.provider {
        crate::ai_config::ProviderKind::Mock => None,
        crate::ai_config::ProviderKind::OpenAiCompatible => Some(ModelsSource::OpenAiCompatible {
            base_url: config.base_url.clone(),
            api_key: api_key.unwrap_or_default(),
        }),
        crate::ai_config::ProviderKind::Anthropic => Some(ModelsSource::Anthropic {
            base_url: config.base_url.clone(),
            api_key: api_key.unwrap_or_default(),
        }),
        crate::ai_config::ProviderKind::Ollama => Some(ModelsSource::Ollama {
            base_url: config.base_url.clone(),
        }),
    }
}

impl State {
    /// 消费一条消息,变更状态。只允许在 `App::logic` 调用。
    pub fn apply(&mut self, message: Message) {
        match message {
            Message::SidebarTabChanged(tab) => {
                self.layout.left_view = tab;
                if tab != SidebarTab::Git {
                    self.git.close_diff_workspace();
                }
                // 切到 Git 页立即刷新:页签可能停了很久,轮询周期外的快照
                // 会误导回滚决策
                if tab == SidebarTab::Git {
                    self.refresh_git();
                }
            }
            Message::FileCommand(cmd) => self.run_file_cmd(cmd),
            Message::EditAction(action) => self.pending_edit_action.push(action),
            Message::NoticeDismissed => self.tabs.current_mut().document.notice = None,
            Message::ExportHtml => self.run_export_html(),
            Message::ExportPdf => self.run_export_pdf(),
            Message::ThemeChanged(mode) => self.change_theme(mode),
            Message::ToggleTheme => self.change_theme(self.theme.mode.opposite()),
            Message::SidebarToggled => self.toggle_left_panel(),
            Message::ZenToggled => self.toggle_zen(),
            Message::ToggleQuickOpen => self.toggle_quick_open(),
            Message::FormatRequested(action) => self.apply_format(action),
            Message::FindBarToggled(open) => self.toggle_find(open),
            Message::FindQueryChanged(query) => self.find_query_changed(query),
            Message::FindNext { backwards } => self.find_next(backwards),
            Message::ReplaceBarToggled(open) => self.toggle_replace(open),
            Message::ReplaceCurrent => self.replace_current(),
            Message::ReplaceAllInDoc => self.replace_all_in_doc(),
            Message::GotoBarToggled(open) => self.toggle_goto(open),
            Message::GotoLineRequested { line } => self.goto_line(line),
            Message::InsertToc => self.insert_toc(),
            Message::ImageDialogOpened => self.open_image_dialog(),
            Message::ImageInserted { alt, url } => self.insert_image(&alt, &url),
            Message::ImageDialogClosed => self.close_image_dialog(),
            Message::ImageFilePickRequested => self.pick_image_file(),
            Message::ImageUploadRequested => self.request_image_upload(),
            Message::ImageUploadFinished { seq, result } => self.finish_image_upload(seq, result),
            Message::ImagePasteRequested => self.request_image_paste(),
            Message::ImagePasteFinished { result } => self.finish_image_paste(result),
            Message::InstallCliRequested => self.request_cli_install(),
            Message::CliInstallFinished { result } => self.finish_cli_install(result),
            Message::ImageFileDropped(path) => self.drop_image_file(&path),
            Message::EmojiPickerToggle(open) => self.toggle_emoji_panel(open),
            Message::EmojiInserted(emoji) => self.insert_emoji(&emoji),
            Message::BedProfileSaved { profile, token } => self.save_bed_profile(profile, token),
            Message::BedProfileDeleted { id } => self.delete_bed_profile(id),
            Message::BedTestUploadRequested { profile_id } => self.test_bed_upload(profile_id),
            Message::FileTreeRootPick => self.pick_file_tree_root(),
            Message::FileTreeRootSelected(dir) => self.change_file_tree_root(dir),
            Message::FileTreeNewFileRequested(dir) => self.request_new_file(dir),
            Message::FileTreeNewFileConfirmed => self.confirm_new_file(),
            Message::FileTreeNewFileCancelled => self.file_tree.new_file = None,
            Message::FileTreeNewFolderRequested(dir) => self.request_new_folder(dir),
            Message::FileTreeNewFolderConfirmed => self.confirm_new_folder(),
            Message::FileTreeNewFolderCancelled => self.file_tree.new_folder = None,
            Message::FileTreeRenameRequested(path) => self.request_rename(path),
            Message::FileTreeRenameConfirmed => self.confirm_rename(),
            Message::FileTreeRenameCancelled => self.file_tree.rename = None,
            Message::FileTreeDeleteRequested(path) => self.request_delete(path),
            Message::FileTreeDeleteConfirmed => self.confirm_delete(),
            Message::FileTreeDeleteCancelled => self.file_tree.delete_confirm = None,
            Message::FileTreeCopyPath { path, relative } => self.copy_path(path, relative),
            Message::FileTreeReveal(path) => self.reveal_path(path),
            Message::CopyText(text) => self.pending_copy_text = Some(text),

            Message::FileTreeRefreshRequested => self.refresh_file_tree(),
            Message::FileTreeToggled(dir) => self.file_tree.toggle(&dir),
            Message::FileSelected(path) => {
                self.open_path(&path);
            }
            Message::SearchQueryChanged => {
                self.search.input_changed(DEBOUNCE);
                // 搜索词有变化即视为「在搜索」:左栏自动展开到 Search 页
                // (禅定模式在归约内豁免)。侧边栏输入框触发的同一条消息
                // 在此是幂等无操作。
                self.reveal_search_panel();
            }
            Message::SearchPanelRevealed => self.reveal_search_panel(),
            Message::SearchReplaceAllRequested => self.request_search_replace_all(),
            Message::SearchReplaceAllConfirmed => self.run_search_replace_all(),
            Message::SearchReplaceAllCancelled => self.search.confirm_replace = None,
            Message::SearchRequested => self.start_search(),
            Message::BacklinksRequested => {
                self.backlinks.start_with_repaint(
                    self.file_tree.root.as_deref(),
                    self.tabs.current().document.path.as_deref(),
                    self.repaint.clone(),
                );
            }
            Message::SearchResultClicked(path, line_no) => {
                self.open_search_hit(&path, line_no);
            }
            Message::OutlineItemClicked(span) => {
                // 大纲点击只驱动一处:编辑器/写作块跳光标。右栏预览退役
                // (live-unify P5)后不再有第二处消费 —— 写作模式与禅定同走
                // Live 浏览态的块级滚动(`live::ui` 的 `jump_to`),归约侧不再
                // 挂跨帧的悬置滚动目标。
                self.jump_cursor_to_heading(span);
            }
            Message::WikilinkClicked { target } => self.open_wikilink(&target),
            Message::DocLinkClicked { href } => self.open_doc_link(&href),
            Message::AiStart => self.start_ai_stream(),
            Message::AiChunk { delta } => self.append_ai_delta(&delta),
            Message::AiDone => {
                // 润色收尾(#61 M3):流结束但会话保留 —— 浮窗从「流式中」
                // 转「待确认」;空草稿开不出可确认的浮窗,按失败落提示。
                // 提示落发起标签:定位必须在绑定清除**之前**(与 AiFailed
                // 同纪律,清了 ai_active_tab 就只能 fallback 到当前标签)。
                let empty_polish = self
                    .ai_polish
                    .as_ref()
                    .is_some_and(|session| session.draft.trim().is_empty());
                self.ai.finish();
                if empty_polish {
                    self.ai_origin_tab_mut().document.notice =
                        Some(AI_POLISH_EMPTY_RESULT_NOTICE.to_owned());
                }
                self.ai_active_tab = None;
                self.ai_insert_at = None;
                if empty_polish {
                    self.ai_polish = None;
                }
            }
            Message::AiFailed(error) => {
                self.ai.finish();
                // 失败信息属于发起流的标签(绑定清除前定位),不属于此刻的
                // active
                self.ai_origin_tab_mut().document.notice = Some(error);
                self.ai_active_tab = None;
                self.ai_insert_at = None;
                // 润色流失败:浮窗一并关闭,半截草稿不留
                self.ai_polish = None;
                // 失败不算完成:指令卡状态随 last_prompt 清空回到未执行
                self.ai.forget_last_prompt();
            }
            Message::AiLinkClicked { prompt } => match prompt {
                Ok(prompt) => {
                    // key 闸门与防重入同判:流式中静默忽略,无 key 才落提示
                    if !self.ai.is_streaming() && self.ai_key_gate() {
                        self.start_ai_stream_with_prompt(&prompt);
                    }
                }
                Err(reason) => self.tabs.current_mut().document.notice = Some(reason),
            },
            Message::AiCommitRequested => self.request_commit_message(),
            Message::SelectionAiActionRequested { action, selection } => {
                // 记账保留(M1 链路测试的通路断言);续写(M2)与润色
                // (M3)在归约里各自接线流式通道。
                self.selection_ai_last = Some(action);
                match action {
                    SelectionAiAction::Continue => self.start_selection_ai_continue(selection),
                    SelectionAiAction::Polish => self.start_selection_ai_polish(selection),
                }
            }
            Message::SelectionAiPolishConfirmed => self.confirm_selection_ai_polish(),
            Message::SelectionAiPolishDismissed => {
                // 放弃 = 关窗零改动;流式进行中放弃还要作废在途流
                // (剩余 chunk 丢弃),文档此刻仍一字未动。
                if self.ai_polish.is_some() {
                    self.abort_ai_stream();
                }
            }
            Message::TaskCheckboxToggled { byte } => self.toggle_task_checkbox(byte),
            Message::AiSummaryRequested => self.request_summary(),
            Message::AiCommitFinished {
                seq,
                tab_id,
                result,
            } => {
                // 只有当前最新任务能收口；迟到 worker 不得覆盖新建议。
                if self.ai.finish_commit(seq) {
                    match result {
                        Ok(subject) => self.ai_commit_suggestion = Some(subject),
                        Err(error) => {
                            // 用户可能已切标签，错误必须回到发起文档；标签已
                            // 关闭或线程异常无标签时静默丢弃，避免污染别的文档。
                            if let Some(index) = self.tabs.index_by_id(tab_id) {
                                self.tabs.tabs[index].document.notice = Some(error);
                            }
                        }
                    }
                }
            }
            Message::AiCommitDismissed => self.ai_commit_suggestion = None,
            Message::AiKeySaved => {
                if let Err(err) = self.ai_key.save() {
                    self.ai_key_failure(err);
                }
            }
            Message::AiKeyCleared => {
                if let Err(err) = self.ai_key.clear() {
                    self.ai_key_failure(err);
                }
            }
            Message::AiKeyBackendUnavailable => self.ai_key.backend_ok = false,
            Message::AiConfigSaved(config) => self.apply_ai_config(config),
            Message::AiModelsFetchRequested => self.request_ai_models(),
            Message::AiModelsFetchFinished { result } => self.settings.models.finish(result),
            Message::McpConfigSaved(config) => self.apply_mcp_config(config),
            Message::ThemeSkinSelected(name) => self.apply_skin(name),
            Message::ThemeSkinExported { name } => self.export_skin(&name),
            Message::ThemeDensityChanged(density) => {
                self.theme.density = density;
                self.persist_theme();
            }
            Message::EditorFontSizeChanged(size) => {
                self.theme.editor_font_size = clamp_editor_font_size(size);
                self.persist_theme();
            }
            Message::EditorLineHeightChanged(ratio) => {
                self.theme.line_height = clamp_line_height(ratio);
                self.persist_theme();
            }
            Message::ShowMinimapToggled(show) => {
                self.theme.show_minimap = show;
                self.persist_theme();
            }
            Message::MinimapAutoChanged(auto) => {
                self.theme.minimap_auto = auto;
                self.persist_theme();
            }
            Message::ToggleMinimap => {
                self.theme.show_minimap = !self.theme.show_minimap;
                self.persist_theme();
            }
            Message::TypewriterToggled(on) => {
                self.theme.show_typewriter = on;
                self.persist_theme();
            }
            Message::SelectionAiToggled(on) => {
                self.theme.show_selection_ai = on;
                self.persist_theme();
            }
            Message::ToggleTypewriter => {
                self.theme.show_typewriter = !self.theme.show_typewriter;
                self.persist_theme();
            }
            Message::FocusModeToggled(on) => {
                self.theme.show_focus_mode = on;
                self.persist_theme();
            }
            Message::ToggleFocusMode => {
                self.theme.show_focus_mode = !self.theme.show_focus_mode;
                self.persist_theme();
            }
            Message::ZenNavModeChanged(mode) => {
                self.theme.zen_nav = mode;
                self.zen_nav = ZenNavState::default();
                self.persist_theme();
            }
            Message::DefaultViewModeChanged(mode) => {
                self.theme.default_view_mode = mode;
                self.persist_theme();
            }
            Message::ToggleLivePreview => self.cycle_render_mode(),
            Message::SetRenderMode(mode) => self.set_render_mode(mode),
            Message::CompareColumnToggled => {
                let target = if self.render_mode == crate::live::RenderMode::Split {
                    crate::live::RenderMode::Source
                } else {
                    crate::live::RenderMode::Split
                };
                self.set_render_mode(target);
            }
            Message::KeymapAssign { cmd, shortcut } => self.assign_shortcut(cmd, shortcut),
            Message::KeymapCleared(cmd) => {
                self.keymap.set(cmd, None);
                self.persist_keymap();
            }
            Message::KeymapReset(cmd) => {
                self.keymap.reset(cmd);
                self.persist_keymap();
            }
            Message::KeymapResetAll => {
                self.keymap.reset_all();
                self.persist_keymap();
            }
            Message::SettingsOpened(tab) => {
                self.settings.open = true;
                self.settings.tab = tab;
            }
            Message::AboutOpened => self.about.open = true,
            Message::AboutClosed => self.about.open = false,
            Message::AboutUpdateCheckRequested => {
                // 网络检查完成后主动唤醒；关于窗结果仍经消息归约更新。
                self.about.update.start_with_repaint(self.repaint.clone());
            }
            Message::AboutUpdateCheckFinished { result } => self.about.update.finish(result),
            Message::TabActivate(index) => self.switch_active(index),
            Message::TabCloseRequested(index) => self.request_close_tab(index),
            Message::TabCloseActive => {
                let active = self.tabs.active;
                self.request_close_tab(active);
            }
            Message::TabCloseConfirmed => {
                // 按稳定 id 定位确认目标:模态是非阻塞 Window,打开期间其他
                // 关闭入口会使索引漂移,按索引确认会关错标签。id 失效(目标
                // 已被其他路径关闭,`TabsState::remove` 已同步撤下确认)则 no-op。
                if let Some(index) = self
                    .tabs
                    .confirm_close
                    .take()
                    .and_then(|id| self.tabs.index_by_id(id))
                {
                    self.remove_tab(index);
                }
                // 批量关闭进行中:确认一个继续下一个(队首正是刚确认的 id,
                // 已被移除,推进循环自然跳过它)。单发关闭时队列已空,空转。
                self.advance_batch_close();
            }
            Message::TabCloseCancelled => {
                self.tabs.confirm_close = None;
                // 任一次取消(#37 批量):立即终止剩余队列 —— 已确认关闭的
                // 保持关闭,不假装可回滚;被拒绝的标签保持打开。
                self.tabs.pending_close.clear();
            }
            Message::TabBatchCloseRequested { kind, index } => {
                self.request_batch_close(kind, index)
            }
            Message::TabRenameRequested { index } => self.request_tab_rename(index),
            Message::TabRenameConfirmed => self.confirm_tab_rename(),
            Message::TabRenameCancelled => self.tabs.rename = None,
            Message::TabTitleWidthChanged(mode) => {
                // 纯显示偏好:只动 theme 字段并落盘(与切密度同款),任何
                // 标签的路径/缓冲/dirty/别名一概不碰
                self.theme.tab_title_width = mode;
                self.persist_theme();
            }
            Message::TabNext => {
                let next = self.tabs.next_index();
                self.switch_active(next);
            }
            Message::TabRestore => self.restore_tab(),
            Message::DraftRecovered { tab_id } => self.recover_draft(tab_id),
            Message::DraftDiscarded { tab_id } => self.discard_draft(tab_id),
            Message::GitFileSelected(path) => self.git.select(&path),
            Message::GitHistorySelected(hash) => {
                if let Some(root) = self.git.repo_root.clone() {
                    self.git.select_commit(&root, &hash);
                }
            }
            Message::GitCommitFileSelected { commit_hash, path } => {
                if let Some(root) = self.git.repo_root.clone() {
                    self.git.select_commit_file(&root, &commit_hash, &path);
                }
            }
            Message::GitCheckoutRequested(path) => self.git.request_checkout(path),
            Message::GitCheckoutConfirmed => {
                // 回滚目标与仓库根在 confirm_checkout 内被 take,先留档供
                // 成功后的「当前文档一致性」判定
                let target = self
                    .git
                    .confirm_checkout
                    .clone()
                    .zip(self.git.repo_root.clone());
                match self.git.confirm_checkout(self.file_tree.root.as_deref()) {
                    Some(error) => self.tabs.current_mut().document.notice = Some(error),
                    None => {
                        if let Some((rel, root)) = target {
                            self.after_git_checkout(&root.join(&rel));
                        }
                    }
                }
            }
            Message::GitCheckoutCancelled => self.git.cancel_checkout(),
            Message::GitDiffViewChanged(view) => self.git.diff_view = view,
            Message::GitDiffWorkspaceClosed => self.git.close_diff_workspace(),
            Message::GitFileHistoryRequested(path) => {
                // 与切到 Git 页同一口径:先刷新拿到 repo_root(页签可能停
                // 了很久),再换相对路径查询
                self.layout.left_view = SidebarTab::Git;
                self.refresh_git();
                match self.git.relative_path(&path) {
                    Some(relative) => self.git.load_file_history(&relative),
                    None => {
                        self.tabs.current_mut().document.notice =
                            Some("该文件不在当前 Git 仓库内,查不到修改记录".to_owned());
                    }
                }
            }
            Message::GitRefreshRequested => self.refresh_git(),
        }
    }

    /// 立即刷新 Git 状态。触发点:切到 Git 页、文件树换根、回滚完成、
    /// 顶部刷新按钮 / 展开「存储库」分区,以及归约侧的到点轮询(见
    /// `ui::layout::reduce`)。刷新顺带把「文件历史」分区同步到当前文档:
    /// 文档没换不动(显式查询不被轮询顶掉),换了文档跟随加载。
    pub fn refresh_git(&mut self) {
        let root = self.file_tree.root.clone();
        let doc_rel = self.current_doc_relative();
        self.git.refresh(root.as_deref(), doc_rel);
    }

    /// 异步发起一次 Git 刷新(**3 秒节流轮询专用**):IO 进后台线程,本帧
    /// 立即返回。实测一轮 12ms、带文件历史 86ms,同步跑就是每 3 秒掉
    /// 1–5 帧;结果由 [`Self::poll_git`] 在归约末尾收。
    ///
    /// 文件历史的跟随策略与同步路径同源([`GitPanelState::history_target`]):
    /// 记账留在 UI 线程,加载随其他 IO 一起进后台。
    pub fn start_git_refresh(&mut self) {
        let root = self.file_tree.root.clone();
        let doc_rel = self.current_doc_relative();
        // 周期刷新完成后由后台线程主动要一帧；点击刷新仍保留同步即时反馈。
        self.git
            .start_refresh_with_repaint(root.as_deref(), doc_rel, self.repaint.clone());
    }

    /// 归约末尾收一次后台 Git 结果(非阻塞)。
    pub fn poll_git(&mut self) {
        self.git.poll_refresh();
    }

    /// 当前文档相对仓库根的路径(文件历史跟随的对象)。
    fn current_doc_relative(&self) -> Option<String> {
        self.tabs
            .current()
            .document
            .path
            .clone()
            .and_then(|doc| self.git.relative_path(&doc))
    }

    /// 回滚成功后的编辑器一致性(`Message::GitCheckoutConfirmed` 的归约
    /// 尾步):目标不是当前文档则无事;是则分两种情况——
    ///
    /// * 非 dirty:缓冲本与磁盘一致,磁盘已回 HEAD,重读换入(预览同帧
    ///   联动),否则编辑器将显示已丢弃的工作区版本;
    /// * dirty:磁盘回 HEAD 但**保留**编辑器里未保存的稿子(静默丢稿风险
    ///   大于不一致风险,与「关闭脏标签要确认」同哲学),提示行告知
    ///   「保存会写回」——否则一次 Ctrl+S 就静默反转回滚,用户毫不知情。
    fn after_git_checkout(&mut self, file: &Path) {
        // 回滚目标可能在任意标签打开(不必是当前标签):找到才处理
        let Some(index) = self.tabs.find_by_path(file) else {
            return;
        };
        if self.tabs.tabs[index].editor.is_dirty() {
            // 与单标签时代同哲学:静默丢稿的代价大于不一致,保留未保存稿
            self.tabs.tabs[index].document.notice = Some(format!(
                "已回滚 {}:该标签里未保存的修改仍保留,保存(Ctrl+S)会把它们写回",
                file.display()
            ));
        } else {
            match file::read(file) {
                Ok(text) => self.tabs.tabs[index].load(Some(file.to_path_buf()), &text),
                Err(error) => {
                    self.tabs.tabs[index].document.notice = Some(error.to_string());
                }
            }
        }
    }

    /// `Message::AiKeySaved` / `AiKeyCleared` 的失败归约:消毒后的错误文案
    /// 进提示行(文案由 latermd-creds 保证不含凭据值);后端类失败再归约
    /// [`Message::AiKeyBackendUnavailable`] 置不可用状态(状态行提示回退
    /// 环境变量)。空白拒绝等非后端错误只落提示行,不动可用性。
    fn ai_key_failure(&mut self, err: latermd_creds::CredentialError) {
        self.tabs.current_mut().document.notice = Some(err.to_string());
        if matches!(err, latermd_creds::CredentialError::Backend { .. }) {
            self.apply(Message::AiKeyBackendUnavailable);
        }
    }

    /// AI 命令发起前的 key 闸门:provider 需要 key 且 latermd-creds →
    /// `LATERMD_AI_API_KEY`(latermd-creds 定死的顺序)都解析不到时,落
    /// 状态栏提示并返回 `false`。调用点必须在各命令归约的**最前面**、任何
    /// 副作用(补空行/移除旧摘要节/采 diff)之前,被拦下的命令不留痕迹。
    /// 当前 Mock provider 无 key 也能跑(闸门直通);选了 OpenAI 兼容端点后
    /// 闸门自动生效(decisions-pending #21 的四入口归约不变)。
    fn ai_key_gate(&mut self) -> bool {
        if !self.ai.requires_key() || self.ai_key.creds.ai_api_key().is_some() {
            return true;
        }
        self.tabs.current_mut().document.notice = Some(AI_KEY_MISSING_NOTICE.to_owned());
        false
    }

    /// 生成 commit message(`Message::AiCommitRequested` 的归约):本线程只
    /// 定位仓库并捕获标签/预算；采 diff、拼 prompt 和 provider 请求全部
    /// 进后台。结果经 [`Message::AiCommitFinished`] 再归约一次落 state。
    ///
    /// 仓库定位:当前文档所在目录优先,退文件树根(`git diff` 在仓库子目录
    /// 里跑也返回全仓改动);两者皆无 → 提示行。流式或 commit 任务进行中
    /// 忽略重复请求，避免同一 provider 同时发起相互竞争的生成。
    fn request_commit_message(&mut self) {
        if self.ai.is_streaming() || self.ai.is_commit_running() || !self.ai_key_gate() {
            return;
        }
        let Some(dir) = self
            .tabs
            .current()
            .document
            .path
            .as_deref()
            .and_then(Path::parent)
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .or_else(|| self.file_tree.root.clone())
        else {
            self.tabs.current_mut().document.notice =
                Some("生成 commit message 需要先保存文档或设置文件树根目录".to_owned());
            return;
        };
        let tab_id = self.tabs.current().id;
        let context_budget = self.ai.config.context_budget();
        // Git 读取和真实 provider HTTP 都可能阻塞；worker 只回传结果，
        // 完成后通知器唤醒 UI，再由 Finished 分支定点落标签状态。
        match self
            .ai
            .start_commit(dir, tab_id, context_budget, self.repaint.clone())
        {
            Ok(true) => self.ai_commit_suggestion = None,
            Ok(false) => {}
            Err(error) => self.tabs.current_mut().document.notice = Some(error),
        }
    }

    /// 生成摘要(`Message::AiSummaryRequested` 的归约):移除旧摘要节 →
    /// 全文拼 prompt → 复用流式通道发块,「## AI 摘要」标题与空行在发起
    /// 时落到文档末尾,要点 chunk(`> - …` 引用块行)经 [`Message::AiChunk`]
    /// 追加长在标题下。移除旧节走 AST 定位 + rope 删除,且只发生在归约里
    /// (铁律三);key 闸门在最前,被拦时连旧节都不动;流式进行中忽略
    /// (防重入,与其它 AI 命令同一道闸)。
    fn request_summary(&mut self) {
        if self.ai.is_streaming() || !self.ai_key_gate() {
            return;
        }
        let text = self.tabs.current().editor.text().to_owned();
        if text.trim().is_empty() {
            self.tabs.current_mut().document.notice = Some("文档为空,没有可摘要的内容".to_owned());
            return;
        }
        // 先移除旧节再取全文:摘要不总结自己;定位用移除前的快照,span 与
        // 此刻缓冲一致(同帧无人改它)
        if let Some(span) = latermd_md::heading_section_span(&text, 2, AI_SUMMARY_HEADING) {
            let tab = self.tabs.current_mut();
            let range = tab.editor.byte_to_char(span.start)..tab.editor.byte_to_char(span.end);
            tab.editor.remove_chars(range);
        }
        // prompt 预算走「上下文大小」配置(decisions-pending #110):
        // 未配置(None)在 latermd-ai 落回 32KB 现状默认,行为不变
        let prompt = latermd_ai::summary_prompt_with_budget(
            self.tabs.current().editor.text(),
            self.ai.config.context_budget(),
        );
        // 共用流式入口(二次防重入 + 补空行 + 发起);标题在发起后、首个
        // chunk 到达前的同一归约里插入,流式块自然接在标题与空行之后
        self.start_ai_stream_with_prompt(&prompt);
        let tab = self.tabs.current_mut();
        tab.editor.insert_chars(
            tab.editor.len_chars(),
            &format!("## {AI_SUMMARY_HEADING}\n\n"),
        );
    }

    /// 发起 AI Mock 流式续写(`Message::AiStart` 的归约)。key 闸门与防
    /// 重入在最前(被拦的命令不补空行、不留任何痕迹);成功发起前把文档
    /// 收成「以空行结尾」,让续写从新段落开始 —— 直接拼在末行会与 AI 首行
    /// 粘连成一行。
    fn start_ai_stream(&mut self) {
        if self.ai.is_streaming() || !self.ai_key_gate() {
            return;
        }
        self.ensure_trailing_blank_line();
        // prompt 透传文档尾部,provider 按原样消费(latermd-ai trait 契约:
        // 截断与拼装是调用方的职责)
        let text = self.tabs.current().editor.text();
        let skip = text.chars().count().saturating_sub(AI_PROMPT_TAIL_CHARS);
        let prompt = format!(
            "请续写以下文档内容:\n{}",
            text.chars().skip(skip).collect::<String>()
        );
        self.start_ai_stream_with_prompt(&prompt);
    }

    /// 流式启动的共用入口,两个来源:菜单命令(带文档尾部的 prompt)与
    /// ai:// 链接点击(链接里的提示词,原样透传)。防重入在此把关,流式
    /// 进行中连「补空行」都不发生;key 闸门在两处入口的归约最前面
    /// (见 [`Self::ai_key_gate`]),走到这里必然已过闸。发起成功即把流
    /// 锁定到当前标签([`State::ai_active_tab`]),此后 chunk 的写入目标
    /// 不随 `tabs.active` 漂移。
    fn start_ai_stream_with_prompt(&mut self, prompt: &str) {
        if self.ai.is_streaming() {
            return;
        }
        self.ensure_trailing_blank_line();
        // provider 仍在自己的线程产出 chunk；通知器仅在 chunk 入队后唤醒
        // 下一帧，文档写入继续统一走 Message 归约。
        if self.ai.start_with_repaint(prompt, self.repaint.clone()) {
            // 新流顶掉待确认的润色浮窗(纯 UI 状态,零文档改动):否则
            // 旧会话挂着、新流的 chunk 会被 [`Self::append_ai_delta`] 误分
            // 进旧草稿。
            self.ai_polish = None;
            self.ai_active_tab = Some(self.tabs.current().id);
        }
    }

    /// 文档非空且不以空行结尾时,补成恰好一个空行(空文档/已空行结尾不动)。
    fn ensure_trailing_blank_line(&mut self) {
        let text = self.tabs.current_mut().editor.text();
        let pad = if text.is_empty() || text.ends_with("\n\n") {
            0
        } else if text.ends_with('\n') {
            1
        } else {
            2
        };
        if pad > 0 {
            let tab = self.tabs.current_mut();
            tab.editor
                .insert_chars(tab.editor.len_chars(), &"\n".repeat(pad));
        }
    }

    /// 选区续写(`SelectionAiActionRequested { Continue }` 的归约,#61 M2):
    /// 捕获选区尾偏移 + 修订号见证(#17 口径)挂进 [`State::ai_insert_at`],
    /// 经既有流式通道从选区尾插入点续写;防重入与 key 闸门照旧,被拦的
    /// 命令连补行都不发生(与其它 AI 入口同一纪律)。
    ///
    /// 发起时在选区尾**两侧锚定补段空行**(`ensure_trailing_blank_line`
    /// 的文档末语义平移到插入点:AI 首行独立成段、末行不与尾后文粘连),
    /// 见证修订号取补行之后 —— 补行是流的一部分,不算用户编辑。
    fn start_selection_ai_continue(&mut self, selection: Option<(usize, usize)>) {
        if self.ai.is_streaming() || !self.ai_key_gate() {
            return;
        }
        // 选区是 #38 同源的字符区间(primary/secondary 无序);塌缩/缺失
        // (浮标只在有选区时在场,这里是防御)退回文档末续写 —— 动作语义
        // 仍是「续写」,只是落点回退(decisions-pending #116)。
        let Some((a, b)) = selection.filter(|(a, b)| a != b) else {
            self.start_ai_stream();
            return;
        };
        let (start, tail) = (a.min(b), a.max(b));
        let tab = self.tabs.current_mut();
        let text = tab.editor.text();
        // 选中文本进 prompt(截断按字符,CJK 不拆半);偏移经 char_to_byte
        // 换算,恒落在字符边界。
        let selected = &text[tab.editor.char_to_byte(start)..tab.editor.char_to_byte(tail)];
        let skip = selected
            .chars()
            .count()
            .saturating_sub(AI_PROMPT_TAIL_CHARS);
        let prompt = format!(
            "请从以下选中文本之后继续写这份文档:\n{}",
            selected.chars().skip(skip).collect::<String>()
        );
        // 锚定补段空行(**前后两侧**,与文档末路径同一矩阵:0 个补二、
        // 1 个补一、已空行不补):前侧隔开选区前文,后侧隔开尾后文,
        // chunk 落在两组补行之间,流完即成规范段落,AI 首末行都不与
        // 既有文本粘连。文末选区无后侧(与文档末路径逐字符等价)。
        let newlines_before = text[..tab.editor.char_to_byte(tail)]
            .chars()
            .rev()
            .take_while(|&c| c == '\n')
            .count();
        let pad = match newlines_before {
            0 => 2,
            1 => 1,
            _ => 0,
        };
        let pad_after = if tail == tab.editor.len_chars() {
            0
        } else {
            match text[tab.editor.char_to_byte(tail)..]
                .chars()
                .take_while(|&c| c == '\n')
                .count()
            {
                0 => 2,
                1 => 1,
                _ => 0,
            }
        };
        tab.editor.insert_chars(tail, &"\n".repeat(pad));
        // 后侧补行插在前侧之后(尾 + 前侧数),插入点正好落在两组之间
        tab.editor.insert_chars(tail + pad, &"\n".repeat(pad_after));
        let insert = AiInsertPoint {
            offset: tail + pad,
            rev: tab.editor.revision(),
        };
        let tab_id = tab.id;
        // 选区续写与普通流共用主动唤醒；chunk 仍定点写回发起标签。
        if self.ai.start_with_repaint(&prompt, self.repaint.clone()) {
            // 新流顶掉待确认的润色浮窗(与 [`Self::start_ai_stream_with_prompt`]
            // 同口径,防 chunk 误分进旧草稿)
            self.ai_polish = None;
            self.ai_active_tab = Some(tab_id);
            self.ai_insert_at = Some(insert);
        }
    }

    /// 选区润色(`SelectionAiActionRequested { Polish }` 的归约,#61 M3):
    /// 选中文本进 prompt 发起润色流,chunk 逐块进 [`State::ai_polish`] 的
    /// 草稿(**不写文档**),收尾后浮窗「确认/放弃」。防重入与 key 闸门
    /// 照旧;素材校验按序落提示:无选区/塌缩、全空白、超上下文上限
    /// (#58 预算,素材不可截断)——被拦的命令零副作用。
    ///
    /// 与续写的差异:没有选区不能回退文档末(替换没有「文档末」语义),
    /// 只能拒绝;发起不改缓冲(不补行),修订号见证即发起帧真值。
    fn start_selection_ai_polish(&mut self, selection: Option<(usize, usize)>) {
        if self.ai.is_streaming() || !self.ai_key_gate() {
            return;
        }
        let Some((a, b)) = selection.filter(|(a, b)| a != b) else {
            self.tabs.current_mut().document.notice =
                Some(AI_POLISH_EMPTY_SELECTION_NOTICE.to_owned());
            return;
        };
        let tab = self.tabs.current_mut();
        let len = tab.editor.len_chars();
        // 消息携带的偏移按发起帧真值钳界(陈旧消息的防御;字符偏移,
        // char_to_byte 恒落字符边界,CJK/emoji 不截半)
        let (start, end) = (a.min(b).min(len), a.max(b).min(len));
        let selected = tab.editor.text()
            [tab.editor.char_to_byte(start)..tab.editor.char_to_byte(end)]
            .to_owned();
        if selected.trim().is_empty() {
            tab.document.notice = Some(AI_POLISH_BLANK_SELECTION_NOTICE.to_owned());
            return;
        }
        let budget = latermd_ai::polish_budget(self.ai.config.context_budget());
        if selected.len() > budget {
            tab.document.notice = Some(format!(
                "{AI_POLISH_TOO_LONG_PREFIX}{}{AI_POLISH_TOO_LONG_SUFFIX}",
                budget / 1024
            ));
            return;
        }
        let prompt = latermd_ai::polish_prompt(&selected);
        let session = AiPolishSession {
            tab_id: tab.id,
            selection: (start, end),
            rev: tab.editor.revision(),
            draft: String::new(),
        };
        let tab_id = tab.id;
        // 润色草稿的 chunk 到达后主动唤醒，确认前绝不写回原文。
        if self.ai.start_with_repaint(&prompt, self.repaint.clone()) {
            self.ai_polish = Some(session);
            self.ai_active_tab = Some(tab_id);
            self.ai_insert_at = None;
        }
    }

    /// 确认润色(`SelectionAiPolishConfirmed` 的归约,#61 M3):整段替换
    /// 选区为润色结果。替换前比对手续修订号见证(#17 口径,照 M2 的插入
    /// 点见证)—— 浮窗期不锁编辑器,用户编辑过就拒绝盲写;替换走
    /// `replace_range` 的 rope 定点编辑(一次归约一次写入,TextEdit 内建
    /// undoer 一次 Ctrl+Z 回原状),新选区落润色结果经
    /// `pending_selection` 回填。
    fn confirm_selection_ai_polish(&mut self) {
        let Some(session) = self.ai_polish.take() else {
            return;
        };
        let Some(index) = self.tabs.index_by_id(session.tab_id) else {
            // 发起标签已被关闭(正常流程 remove_tab 已先行作废会话,防御)
            return;
        };
        let tab = &mut self.tabs.tabs[index];
        if tab.editor.revision() != session.rev {
            tab.document.notice = Some(AI_POLISH_STALE_NOTICE.to_owned());
            return;
        }
        let (start, end) = session.selection;
        let polished = session.draft;
        tab.editor.replace_range(start..end, &polished);
        tab.pending_selection = Some((start, start + polished.chars().count()));
    }

    /// 翻转任务列表勾选态(`Message::TaskCheckboxToggled` 的归约,#63)。
    ///
    /// `byte` 指向源码中标记的 `[`;翻转 = 把中段字符 ` `↔`x`(`X` 视作
    /// 已勾,取消写回空格,勾选统一写小写 `x`,与 GFM 主流一致)。
    /// **单次** [`EditorBuffer::replace_range`] 定点编辑(与 #61 M3 润色
    /// 替换同款):TextEdit 内建 undoer 按绘制帧取快照,一次 Ctrl+Z 即回
    /// 原,不拆成逐字符写入。落点先经 [`latermd_md::is_task_marker_start`]
    /// (pulldown 事件重扫,含字符边界防御)核验,不过即静默返回 ——
    /// 点击帧到归约帧之间文档被并发编辑时偏移可能过期,行内代码/普通
    /// 文本里的 `[ ]` 也在这里被挡住,宁可不写也不写错位置。
    fn toggle_task_checkbox(&mut self, byte: usize) {
        let editor = &mut self.tabs.current_mut().editor;
        let text = editor.text();
        // pulldown 事件口径重扫核验(单一解析器纪律):过期偏移落在行内
        // 代码/普通文本的 `[ ]` 上时不命中,宁可不写也不写错位置。
        if !latermd_md::is_task_marker_start(text, byte) {
            return;
        }
        // 勾选方向按当前标记实况读(三字符全 ASCII,pulldown 已核验过形态)。
        let marker = &text.as_bytes()[byte..byte + 3];
        let replacement = match marker {
            b"[ ]" => 'x',
            _ => ' ',
        };
        let char_at = editor.byte_to_char(byte);
        editor.replace_range(char_at + 1..char_at + 2, &replacement.to_string());
    }

    /// 写入 AI 增量块到**发起标签**(`Message::AiChunk` 的归约)。写入目标
    /// 按 [`State::ai_active_tab`] 定位而非当前标签 —— 流式期间切标签,块
    /// 仍长在发起它的文档上(关键回归测试
    /// `ai_stream_writes_to_origin_tab_not_active`)。落点按序分流:润色
    /// 会话在途(#61 M3)chunk 进浮窗草稿不写文档;[`State::ai_insert_at`]
    /// `Some`(选区续写)写插入点并随写推进,修订号与见证不符即流中编辑
    /// → 作废流并提示(decisions-pending #116),绝不写进过期位置;
    /// `None`(存量入口)写文档末尾,行为零变化。发起标签已不存在时
    /// (理论上 `remove_tab` 已先行作废流)丢弃块并作废,绝不落到别的
    /// 标签。走 `insert_chars` 的增量 splice 双写,dirty 与修订号照常推进
    /// —— AI 写进来的是真实内容,保存前与手敲同责;undo 语义见模块文档。
    fn append_ai_delta(&mut self, delta: &str) {
        if self.ai_polish.is_some() {
            // 润色流:delta 进浮窗草稿,文档一字不动;发起标签失效
            // (标签被关闭,理论流程已先行作废)时防御性作废
            if self.ai_stream_tab_index().is_none() {
                self.abort_ai_stream();
                return;
            }
            if let Some(session) = self.ai_polish.as_mut() {
                session.draft.push_str(delta);
            }
            return;
        }
        let Some(index) = self.ai_stream_tab_index() else {
            self.abort_ai_stream();
            return;
        };
        let Some((offset, rev)) = self
            .ai_insert_at
            .as_ref()
            .map(|point| (point.offset, point.rev))
        else {
            let tab = &mut self.tabs.tabs[index];
            tab.editor.insert_chars(tab.editor.len_chars(), delta);
            return;
        };
        // 修订号见证:与发起(或上一块写入)时不符 = 用户在流中编辑过,
        // 偏移已不可信。中止比钳制诚实:钳到文档末是静默写错位置,按编辑
        // 量平移要 diff 整篇,都不如让用户重来一次(decisions-pending #116)。
        if self.tabs.tabs[index].editor.revision() != rev {
            self.abort_ai_stream();
            self.tabs.tabs[index].document.notice = Some(AI_STALE_INSERT_NOTICE.to_owned());
            return;
        }
        let tab = &mut self.tabs.tabs[index];
        tab.editor
            .insert_chars(offset.min(tab.editor.len_chars()), delta);
        if let Some(point) = self.ai_insert_at.as_mut() {
            point.offset += delta.chars().count();
            point.rev = tab.editor.revision();
        }
    }

    /// 在途流发起标签的索引;id 失效(标签已被移除)返回 `None`。
    fn ai_stream_tab_index(&self) -> Option<usize> {
        self.ai_active_tab.and_then(|id| self.tabs.index_by_id(id))
    }

    /// 在途流的发起标签(可变);id 失效时退回当前标签(防御性兜底,
    /// 正常流程 `remove_tab` 已把流作废,收尾消息不会晚于标签移除到达)。
    fn ai_origin_tab_mut(&mut self) -> &mut crate::tabs::TabState {
        let index = self.ai_stream_tab_index().unwrap_or(self.tabs.active);
        &mut self.tabs.tabs[index]
    }

    /// 作废在途流(收尾标志 + 指令卡状态键 + 发起标签绑定 + 插入点 +
    /// 润色会话一并清)。
    fn abort_ai_stream(&mut self) {
        self.ai.finish();
        self.ai.forget_last_prompt();
        self.ai_active_tab = None;
        self.ai_insert_at = None;
        self.ai_polish = None;
    }

    /// 收流(每帧归约调用一次):把 AI channel 里积压的 chunk 翻成消息,
    /// 由调用方(`ui::layout::reduce`)并入本帧的归约队列。
    pub fn poll_ai(&mut self) -> Vec<Message> {
        self.ai.poll()
    }

    /// 剪贴板图片读取的收流(与 [`Self::poll_bed`] 同分工)。
    pub fn poll_clipboard(&mut self) -> Vec<Message> {
        self.clipboard.poll()
    }

    /// 命令行工具安装的收流(与 [`Self::poll_clipboard`] 同分工)。
    pub fn poll_cli_install(&mut self) -> Vec<Message> {
        self.cli_install.poll()
    }

    /// 当前应渲染的明暗:`System` 已在 [`ThemeMode::resolve`] 里落到确定值,
    /// 绘制前一律用它,不直接读 `theme.mode`。
    pub fn resolved_theme(&self) -> ThemeMode {
        self.theme.mode.resolve(
            self.system_theme,
            self.system_theme.unwrap_or(ThemeMode::Dark),
        )
    }

    /// 立即探测一次系统主题(启动与刚切到「跟随系统」时用)。
    pub fn refresh_system_theme(&mut self) {
        self.system_theme = crate::theme::detect_system_mode();
        self.system_theme_ok = self.system_theme.is_some();
    }

    /// 节流刷新系统主题:只有「跟随系统」模式才轮询,其余模式零开销
    /// (探测要查系统设置,不该在没用到它的时候白跑)。
    ///
    /// 返回下一次探测时刻(供 `ui::layout::reduce` 要帧);非跟随系统返回
    /// `None`,让 egui 收敛到深度空闲。
    pub fn poll_system_theme(&mut self, now: std::time::Instant) -> Option<std::time::Instant> {
        if self.theme.mode != ThemeMode::System {
            self.system_theme_due = None;
            return None;
        }
        if self.system_theme_due.is_none_or(|due| due <= now) {
            self.refresh_system_theme();
            self.system_theme_due = Some(now + SYSTEM_THEME_POLL);
        }
        self.system_theme_due
    }

    /// 启动装载:`ai.json`(AI provider 参数)、`keymap.json`(键位)与
    /// `mcp.json`(MCP 开关/端口/工具权限),并据此装配 provider 运行时与
    /// MCP 服务。与主题/文件树设置同处调用(见 `main`)。无配置目录(极简
    /// 环境)时保持内存默认,不报错。
    ///
    /// MCP 只在配置里 `enabled` 时才起线程 —— 默认配置是关闭,故全新安装
    /// 不会静默监听端口。
    pub fn load_preferences(&mut self) {
        let Some(dir) = self.config_dir() else {
            return;
        };
        let config = AiConfig::load_from(&dir);
        let key = self.ai_key.creds.ai_api_key();
        self.ai.set_provider(config, key.as_deref());
        self.keymap = Keymap::load_from(&dir);
        // 图床 profile 列表(beds.json);token 在钥匙串,启动不读(上传时
        // 后台线程按需取)
        self.bed.profiles = BedState::load_from(&dir);
        // 出厂预设色板(U0,theme_presets):铺进皮肤目录一次(同名文件
        // 不存在才写,用户改过的预设不被顶掉),再扫描 —— 首次安装即可
        // 在设置页看到九套预设,不需要先手动导出一次皮肤
        crate::theme_presets::install_to(&dir);
        // 皮肤目录(`themes/*.rom`)与选中的皮肤内容:皮肤文件是唯一事实源,
        // 内存里只留载入后的样式
        self.skins = SkinCatalog::load_from(&dir);
        let skin = self.theme.skin.clone();
        self.theme.select_skin(skin.as_deref(), &self.skins);
        // Emoji「最近使用」随 settings.json 回来(E2,与主题同文件同路);
        // 手改配置存多的部分截到容量,两处同步收敛到同一份
        self.emoji.recent = self.theme.emoji_recent.clone();
        self.emoji.recent.truncate(EMOJI_RECENT_CAP);
        self.theme.emoji_recent = self.emoji.recent.clone();
        // 跟随系统:启动即探测一次,否则首帧只能靠 fallback
        if self.theme.mode == ThemeMode::System {
            self.refresh_system_theme();
        }
        let mcp = McpConfig::load_from(&dir);
        self.mcp.set_root(self.file_tree.root.clone());
        // MCP 绑定在后台完成，成功或失败都会主动唤醒下一帧更新状态行。
        self.mcp
            .apply_config_with_repaint(mcp, self.repaint.clone());
    }

    /// 配置目录:测试注入的 `settings_dir` 优先,否则平台默认目录。
    fn config_dir(&self) -> Option<PathBuf> {
        self.settings_dir.clone().or_else(crate::theme::config_dir)
    }

    /// 快捷键落盘;失败只落提示行(改键已在内存生效,不回滚 —— 落盘失败
    /// 不该让用户白按一次)。
    fn persist_keymap(&mut self) {
        let Some(dir) = self.config_dir() else {
            return;
        };
        if let Err(error) = self.keymap.save_to(&dir) {
            self.tabs.current_mut().document.notice = Some(format!("快捷键保存失败:{error}"));
        }
    }

    /// 绑定新键位:不可绑定(裸字母)与撞键都**拒绝**并给可行动的提示,
    /// 不静默抢占另一个命令的键位。
    fn assign_shortcut(&mut self, cmd: Command, shortcut: Shortcut) {
        if !shortcut.bindable() {
            self.tabs.current_mut().document.notice = Some(
                "请带上 Ctrl/Cmd、Shift 或 Alt 等修饰键 —— 裸字母会被编辑器当输入吞掉".to_owned(),
            );
            return;
        }
        if let Some(holder) = self.keymap.conflict(cmd, shortcut) {
            self.tabs.current_mut().document.notice = Some(format!(
                "{} 已被「{}」占用,未修改",
                shortcut.platform_text(),
                holder.label()
            ));
            return;
        }
        self.keymap.set(cmd, Some(shortcut));
        self.persist_keymap();
    }

    /// 翻转左栏(导航)可见性。写盘不在这里 —— 帧末的比对写统一负责
    /// (见 `end_of_logic`),既是面板把手那条不产消息的路径也要被写到。
    fn toggle_left_panel(&mut self) {
        self.layout.left = !self.layout.left;
    }

    /// 进/出禅定(§7)。进出各走 `LayoutSettings` 上那两个同名方法 —— 快照
    /// 怎么存怎么还原由它们自己说了算,`State` 不插手第二个真相源。
    fn toggle_zen(&mut self) {
        if self.layout.zen {
            self.layout.exit_zen();
            // 退出即复位悬停导航(#57):显隐判定随布局分叉失效,残置的可见位
            // 会在重进禅定时凭空闪现一列(动画残值由绘制侧的全隐帧钉回 0)。
            self.zen_nav = ZenNavState::default();
            // P3(live-unify):禅定帧强制过 browse_enabled=false(点击
            // 都不进编辑),退出即还原浏览优先的交互语义 —— 与切入 Live 的
            // 既有口径同源。browse_only 不动:若禅定前正在写作(活动块仍在),
            // 下一帧 live::ui 的既定翻转(浏览态遇活动块即写作)自愈。
            self.tabs.current_mut().live.browse_enabled = true;
        } else {
            self.layout.enter_zen();
        }
    }

    /// 当前编辑目标的 widget id(`Message::EditAction` 推合成事件前归还
    /// 焦点的对象,context-menu-plan §3 M1):源码模式 = 源码 TextEdit;
    /// Live = 活动块 TextEdit,无活动块 = 空文档块,浏览态 = `None`
    /// (无处还焦,动作作废 —— 浏览态没有可消费事件的编辑器)。
    pub(crate) fn editor_focus_id(&self) -> Option<eframe::egui::Id> {
        let tab = self.tabs.current();
        let base = crate::ui::editor::tab_editor_id(tab.id);
        if self.render_mode != crate::live::RenderMode::Live {
            return Some(base);
        }
        if tab.live.browse_only {
            return None;
        }
        Some(match tab.live.active {
            Some(index) => base.with(("live-block", index)),
            None => base.with("live-empty"),
        })
    }

    /// 请求一次 Markdown 格式动作(§6.4 链路)。
    ///
    /// Live 模式走 `Self::apply_format_live`(块内定点写回);源码模式:
    /// 语义全在 [`crate::compose::apply`],这里负责三件事:取当前选区 →
    /// 调用 → 把新文本写回缓冲并把新选区挂 `pending_selection`。选区为
    /// `None`(UI 还没回填过)时按纯光标(0,0)处理。
    ///
    /// 写回用 `replace_all` 而非定点 `replace_range`:`compose::apply` 的
    /// 产出是**整篇**新文本,要拿到定点 delta 得自己去做 diff(复杂度与
    /// 收益不成比例)。代价是 `TextEdit` 内建 undoer 的快照被整篇重建打碎,
    /// Ctrl+Z 可能一次回退一整次格式操作 —— 已知并接受(§9 R3)。
    fn apply_format(&mut self, action: crate::compose::FormatAction) {
        if self.render_mode == crate::live::RenderMode::Live {
            self.apply_format_live(action);
            return;
        }
        let tab = self.tabs.current_mut();
        let selection = tab.selection.unwrap_or((0, 0));
        let start = selection.0.min(selection.1);
        let stop = selection.0.max(selection.1);
        let (text, new_selection) = crate::compose::apply(action, tab.editor.text(), start..stop);
        tab.editor.replace_all(&text);
        tab.pending_selection = Some((new_selection.start, new_selection.end));
        // 整篇替换后,按旧文本折出的 `cursor.byte` 对新文本可能不再是字符
        // 边界(状态栏同帧就会拿它切片,2026-09-27 实测崩溃)。byte_to_char
        // 把非边界归到所属字符起点(ropey 语义),char_to_byte 再折回本文本
        // 的边界字节;下一帧编辑器照常回填准确值。
        if let Some(byte) = tab.cursor.byte {
            tab.cursor.byte = Some(tab.editor.char_to_byte(tab.editor.byte_to_char(byte)));
        }
    }

    /// Live 模式的格式动作:对**活动块**切片做块内 `compose::apply`,
    /// `replace_range` 定点写回(与 AI 润色/任务勾选同一条通道,TextEdit
    /// 内建 undo 单步可退),新选区经 `pending_caret` + `pending_range` 落
    /// 回活动块 —— 内容仍选中,连点第二次才 toggle 得掉(与源码模式
    /// `wrap` 的严判定同语义)。
    ///
    /// 防御(live 下点格式动作曾是现行 bug:拿源码模式的陈旧选区、甚至
    /// 从 (0,0) 盲插 `**`):活动块或选区缺失、选区与活动块不相交,一律
    /// 放弃 —— live 的选区只可能来自活动块(每帧由 `live::ui` 镜像进
    /// `tab.selection`),拿不到就不猜。
    fn apply_format_live(&mut self, action: crate::compose::FormatAction) {
        let tab = self.tabs.current_mut();
        // 块表以「最后一次绘制帧」的文本为基准;消息队列里若排着更早的
        // 改文消息(如连点两次加粗),此刻的块表已过期 —— 先按当前缓冲
        // 重同步(幂等,修订号没动就是空操作),活动块也随 cursor.byte
        // 重新定位,后续连点才能落在正确的块上。
        tab.live.sync(&tab.editor, tab.cursor.byte);
        let Some(index) = tab.live.active else {
            return;
        };
        let Some((sel_start, sel_stop)) = tab.selection else {
            return;
        };
        let (sel_start, sel_stop) = (sel_start.min(sel_stop), sel_start.max(sel_stop));
        let range = tab.live.blocks[index].clone();
        let base = tab.editor.byte_to_char(range.start);
        let len = tab.live.block_char_len(&tab.editor, index);
        if sel_stop <= base || sel_start >= base + len {
            return;
        }
        let local_start = sel_start.saturating_sub(base).min(len);
        let local_stop = sel_stop.saturating_sub(base).min(len);
        let block_text = tab.editor.text()[range].to_owned();
        let (new_block, new_selection) =
            crate::compose::apply(action, &block_text, local_start..local_stop);
        tab.editor.replace_range(base..base + len, &new_block);
        // 新选区(块内坐标)落回活动块。不写 `pending_selection` —— Live
        // 不消费它,挂到切回源码才触发是惊扰;caret_follow = false,与
        // 「点击进编辑不拽视口」同口径:选区本就是用户刚框出来的。
        tab.live.pending_caret = Some((index, new_selection.start));
        tab.live.pending_range = Some((new_selection.start, new_selection.end));
        tab.live.caret_follow = false;
        // 大纲/状态栏的同帧口径:光标落到内容起点(下一帧镜像再校准)。
        tab.cursor.byte = Some(tab.editor.char_to_byte(base + new_selection.start));
    }

    /// 查找条开/关:开时按当前选区预填查找词(选中即所要找的,编辑器
    /// 惯例)并重扫;关时清定位。开时收起「跳转到行」浮条(两者互斥,
    /// decisions-pending #113)。
    fn toggle_find(&mut self, open: bool) {
        self.find.open = open;
        if open {
            self.find.want_focus = true;
            self.goto.open = false;
            let tab = self.tabs.current();
            if let Some((start, stop)) = tab.selection {
                let (start, stop) = (start.min(stop), start.max(stop));
                if start < stop {
                    let text: String = tab
                        .editor
                        .text()
                        .chars()
                        .skip(start)
                        .take(stop - start)
                        .collect();
                    self.find.query = text;
                }
            }
            self.find_rescan();
        } else {
            self.find.hit = None;
            // 整条关闭(Esc / ✕):替换行随之收起,下次 Ctrl+F 打开不带它
            self.find.replace_open = false;
        }
    }

    /// 查找词变化:重扫 + 跳第一个命中(有命中才写 pending_selection,
    /// 空命中只清定位,不动光标)。
    fn find_query_changed(&mut self, query: String) {
        self.find.query = query;
        self.find_rescan();
    }

    /// 重扫命中:大小写不敏感的子串全量扫描,命中是**字符区间**
    /// (`pending_selection` 的契约单位)。query 为空只清结果。
    fn find_rescan(&mut self) {
        // 本函数是命中缓存的唯一生产者,扫前先记新鲜度见证 —— 空结果的
        // 缓存同样「对当前文本新鲜」。替换入口据此判过期。
        let tab = self.tabs.current();
        self.find.scanned_for = Some((tab.id, tab.editor.revision()));
        let query = self.find.query.clone();
        self.find.hits.clear();
        self.find.hit = None;
        if query.is_empty() {
            return;
        }
        let text: String = self.tabs.current().editor.text().to_owned();
        let haystack: String = text.to_lowercase();
        let needle = query.to_lowercase();
        let mut byte_hits = Vec::new();
        let mut from = 0;
        while let Some(idx) = haystack[from..].find(&needle) {
            byte_hits.push(from + idx);
            from += idx + needle.len().max(1);
        }
        // 字节起点 → 字符区间(lowercase 后长度可能变,如 İ → i̇,故
        // 字节命中映射回原文用 char_indices 累计换算)
        let mut char_of_byte = std::collections::HashMap::new();
        for (char_idx, (byte, _)) in text.char_indices().enumerate() {
            char_of_byte.insert(byte, char_idx);
        }
        let needle_chars = needle.chars().count();
        for byte_start in byte_hits {
            // 命中字节起点必须落在原文边界上;极端 Unicode 折叠错位时跳过
            // 该命中(大小写折叠改变长度,边界对不齐),宁可少报不错报
            if let Some(&start) = char_of_byte.get(&byte_start) {
                self.find.hits.push(start..start + needle_chars);
            }
        }
        if !self.find.hits.is_empty() {
            self.find.hit = Some(0);
            let first = self.find.hits[0].clone();
            self.tabs.current_mut().pending_selection = Some((first.start, first.end));
        }
    }

    /// 跳上/下一个命中(环绕),经 `pending_selection` 把选区(光标)搬
    /// 过去,下一帧编辑器写回并滚入视口(#29 的跟随链路)。
    fn find_next(&mut self, backwards: bool) {
        let len = self.find.hits.len();
        if len == 0 {
            return;
        }
        let next = match self.find.hit {
            None => {
                if backwards {
                    len - 1
                } else {
                    0
                }
            }
            Some(h) => {
                if backwards {
                    (h + len - 1) % len
                } else {
                    (h + 1) % len
                }
            }
        };
        self.find.hit = Some(next);
        let range = self.find.hits[next].clone();
        self.tabs.current_mut().pending_selection = Some((range.start, range.end));
    }

    /// 替换行开/关(Ctrl+H,#17)。`open = true` 且查找条与替换行都已展开
    /// 时视为「再按一次」:收替换行、查找条保留;否则展开替换行,查找条
    /// 未开则同开(选区预填与重扫复用 `toggle_find`)。`open = false`
    /// 显式收替换行,查找条不动。
    fn toggle_replace(&mut self, open: bool) {
        if open && self.find.open && self.find.replace_open {
            self.find.replace_open = false;
            return;
        }
        if open && !self.find.open {
            self.toggle_find(true);
        }
        self.find.replace_open = open;
    }

    /// 「跳转到行」浮条开/关(#60 M1,Ctrl+G)。开时清行号草稿并收起
    /// 查找条(含替换行,与 Esc 关整条同款清理;两者互斥,decisions-
    /// pending #113);关即关,不动其他浮层。
    fn toggle_goto(&mut self, open: bool) {
        self.goto.open = open;
        if open {
            self.goto.input.clear();
            self.find.open = false;
            self.find.replace_open = false;
            self.find.hit = None;
        }
    }

    /// 跳转到行(#60 M1,`Message::GotoLineRequested` 的归约):1 起行号
    /// 钳制到 1..=总行数(总行数与源码行号槽同一口径:1 + '\n' 数,空
    /// 文档 = 1),越界钳最近端;光标落该行行首 —— 经
    /// [`OutlineCursor::jump_to`] 交给 `ui::editor`(源码模式:覆写持久
    /// 光标 + scroll_to_rect 滚动,大纲/搜索跳转同一通道)或 `live::ui`
    /// (Live 模式:切块 + `pending_caret` 路由)。本函数不写文本,缓冲
    /// 与撤销栈分毫不动;跳成后浮条关闭(输入一并清空)。
    fn goto_line(&mut self, raw: usize) {
        let tab = self.tabs.current_mut();
        let total = 1 + tab.editor.text().bytes().filter(|b| *b == b'\n').count();
        let line = raw.clamp(1, total);
        let byte = tab.editor.line_to_byte(line - 1);
        tab.cursor.jump_to = Some(tab.editor.byte_to_char(byte));
        self.goto.open = false;
        self.goto.input.clear();
    }

    /// 插入/更新目录([`Message::InsertToc`],#66 M2)的归约:
    ///
    /// 1. 全文走大纲通道取标题;无可收录标题只落提示行,文档/选区/修订号
    ///    一概不动(与 AI 摘要的空文档防御同款;全无标题与「只有四级及
    ///    更深标题」两种提示分开,口径 decisions-pending #127);
    /// 2. 有可收录标题:组装完整 TOC 块(标记行包围,`latermd_md::toc_block`,
    ///    锚点为 M1 `generate_toc` 口径),既有块(`toc_region_span` 标记
    ///    识别)**整块替换**,否则插入文档头 —— 一次
    ///    [`EditorBuffer::replace_range`] 定点写入,TextEdit 内建 undoer 的
    ///    快照按绘制帧落,一次 Ctrl+Z 整体回原状(#17 全部替换 / #61 润色
    ///    确认同语义,无头实证见 `ui::editor` 的 TOC undo 测试);区域内的
    ///    手工改动随整块覆盖(同 #127);
    /// 3. 写入后光标折叠到块首(`cursor.jump_to`,源码/Live 两模式同一
    ///    入口,写回帧视口随光标滚到块首),`cursor.byte` 按字符边界折算
    ///    (`apply_format` 同款);dirty 由 `EditorBuffer::touch` 置位,走
    ///    正常保存链路。
    fn insert_toc(&mut self) {
        let text = self.tabs.current().editor.text().to_owned();
        let items = latermd_md::outline(&text);
        let notice = if items.is_empty() {
            Some("文档没有标题,无法生成目录".to_owned())
        } else if !items
            .iter()
            .any(|item| item.level <= latermd_md::TOC_MAX_DEPTH)
        {
            Some("目录只收录一至三级标题,当前文档没有可收录的标题".to_owned())
        } else {
            None
        };
        if let Some(notice) = notice {
            self.tabs.current_mut().document.notice = Some(notice);
            return;
        }
        let block = latermd_md::toc_block(&items);
        // 既有块整块替换;无块则插入文档头。替换文本恰为块本体(块外字节
        // 零增删,反复执行不涨空行);首插多补一个空行把块与正文隔开。
        let (byte_start, byte_end, pad) = match latermd_md::toc_region_span(&text) {
            Some(span) => (span.start, span.end, ""),
            None => (0, 0, "\n"),
        };
        let tab = self.tabs.current_mut();
        let start = tab.editor.byte_to_char(byte_start);
        let end = tab.editor.byte_to_char(byte_end);
        tab.editor
            .replace_range(start..end, &format!("{block}{pad}"));
        tab.cursor.jump_to = Some(start);
        if let Some(byte) = tab.cursor.byte {
            tab.cursor.byte = Some(tab.editor.char_to_byte(tab.editor.byte_to_char(byte)));
        }
    }

    /// 替换前刷新过期的命中缓存(#17 评审修复):`hits`/`hit` 只在查找词
    /// 变化与切/关标签时重扫,同标签文本被格式化(`apply_format` 整篇
    /// 改写)、undo/redo 或外部重载改写后**不重扫** —— 旧字符偏移在新文本
    /// 上可能仍界内但已错位,`replace_all_in_doc` 的越界防线只挡 `end`
    /// 超长,挡不住界内漂移,替换词会写进错误位置(查找侧同款过期只让
    /// 光标跳错,替换把它升级成破坏性写入)。
    ///
    /// 按 `scanned_for` 见证判过期(`EditorBuffer` 全部写路径都推 rev,
    /// 「同标签同 rev ⇒ 文本未变」),新鲜则零扰动返回。过期先重扫;
    /// `find_rescan` 会把定位重置回首命中,先记下当前命中的字符锚点,
    /// 扫完按「start ≥ 锚点」找回原定位 —— 找不到(该命中已被改写消失)
    /// 顺取首条,写入位置永远是当前文本上的真实命中。重扫附带的
    /// `pending_selection` 定位副作用存还,替换后的选区行为不变。
    fn refresh_stale_find_hits(&mut self) {
        let tab = self.tabs.current();
        let scanned_for = (tab.id, tab.editor.revision());
        if self.find.scanned_for == Some(scanned_for) {
            return;
        }
        let anchor = self
            .find
            .hit
            .and_then(|index| self.find.hits.get(index))
            .map(|range| range.start);
        let selection = self.tabs.current().pending_selection;
        self.find_rescan();
        self.tabs.current_mut().pending_selection = selection;
        if let Some(anchor) = anchor {
            if !self.find.hits.is_empty() {
                self.find.hit = Some(
                    self.find
                        .hits
                        .iter()
                        .position(|hit| hit.start >= anchor)
                        .unwrap_or(0),
                );
            }
        }
    }

    /// 单个替换(#17):把当前命中替换为替换词,经 [`EditorBuffer::replace_range`]
    /// 定点写入缓冲(dirty / rev 由 `touch` 维护,预览快照随 rev 联动);
    /// 随后重扫并把定位推到**替换文本之后**的第一个命中 —— 锚点取替换词
    /// 末尾而非命中起点,替换词自身含查找词(needle → needleX)时刚写入
    /// 的命中不会被再次选中;之后无命中则环绕回首,全空则清定位。
    fn replace_current(&mut self) {
        self.refresh_stale_find_hits();
        let Some(index) = self.find.hit else { return };
        let Some(range) = self.find.hits.get(index).cloned() else {
            return;
        };
        let replacement = self.find.replacement.clone();
        {
            let tab = self.tabs.current_mut();
            tab.editor
                .replace_range(range.start..range.end, &replacement);
            // 局部编辑改变长度,旧字节光标可能漂出字符边界(状态栏同帧
            // 切片,`apply_format` 同款钳制)
            if let Some(byte) = tab.cursor.byte {
                tab.cursor.byte = Some(tab.editor.char_to_byte(tab.editor.byte_to_char(byte)));
            }
        }
        let anchor = range.start + replacement.chars().count();
        self.find_rescan();
        self.find.hit = (!self.find.hits.is_empty()).then(|| {
            self.find
                .hits
                .iter()
                .position(|hit| hit.start >= anchor)
                .unwrap_or(0)
        });
        if let Some(index) = self.find.hit {
            let range = self.find.hits[index].clone();
            self.tabs.current_mut().pending_selection = Some((range.start, range.end));
        }
    }

    /// 全部替换(#17):按当前命中从后往前生成新文本,**一次** `replace_all`
    /// 写入(整篇替换路径,与 `apply_format` 同款)—— undo 栈只多一份快照,
    /// 一次 Ctrl+Z 整体回原状(egui undoer 看不到程序化写入,快照按绘制帧
    /// 落,与 AI 流式追加同语义;无头实证见 `ui::editor` 的 replace_all
    /// undo 测试),绝无逐命中循环写入。
    fn replace_all_in_doc(&mut self) {
        // 缓存可能对着被格式化/撤销/重载改写前的文本,先刷新再消费 ——
        // 旧偏移错位时下面的越界防线挡不住界内漂移
        self.refresh_stale_find_hits();
        if self.find.hits.is_empty() {
            return;
        }
        let replacement = self.find.replacement.clone();
        let mut text = self.tabs.current().editor.text().to_owned();
        // 命中是字符区间,落 `String::replace_range` 前换算字节边界;从后
        // 往前替换,前面的命中偏移不受影响。越界命中(扫描后文档又变短,
        // 如大小写折叠错位或换标签竞态)整条跳过:`char_to_byte` 会把越界
        // 钳到文末,不跳就是把替换词追加到结尾 —— 宁可少报不错报,不 panic。
        let char_count = text.chars().count();
        for range in self.find.hits.iter().rev() {
            if range.end > char_count {
                continue;
            }
            let byte_start = char_to_byte(&text, range.start);
            let byte_end = char_to_byte(&text, range.end);
            text.replace_range(byte_start..byte_end, &replacement);
        }
        {
            let tab = self.tabs.current_mut();
            tab.editor.replace_all(&text);
            if let Some(byte) = tab.cursor.byte {
                tab.cursor.byte = Some(tab.editor.char_to_byte(tab.editor.byte_to_char(byte)));
            }
        }
        self.find_rescan();
    }

    /// 换入不同标签后按「query 变化」同口径重扫命中(#17 M2):`hits`/
    /// `hit` 缓存对着**旧标签的文本**,不重扫则计数与「替换/全部」作用的
    /// 区间都指向旧文档 —— 越界命中还会被钳到文末,把替换词追加进新文档
    /// (见 `replace_all_in_doc` 的跳过防线,这里从源头掐掉)。查找条收起
    /// 时不扫:重新展开(Ctrl+F/Ctrl+H)必经 `toggle_find` 重扫,关闭期
    /// 扫描只是白付。
    fn rescan_find_after_tab_switch(&mut self, previous_tab: u64) {
        if self.find.open && self.tabs.current().id != previous_tab {
            self.find_rescan();
        }
    }

    /// 快速打开(#24)的开关归约:开 = 拍文件快照(`latermd_search::list_files`,
    /// 与文件树 `find_by_name`、MCP 同一份实现)+ 复位查询词/选中项;关 =
    /// 只翻标志。列表目录是本地磁盘毫秒级 IO,同步在归约做 —— 与文件树
    /// `ensure_loaded` 同口径。
    fn toggle_quick_open(&mut self) {
        if self.quick_open.open {
            self.quick_open.close();
        } else {
            self.quick_open
                .open_snapshot(self.file_tree.root.as_deref());
        }
    }

    /// 打开图片框(docs/image-plan.md A 段):alt 按当前选区预填 —— 选中
    /// 文字就是要保留的替代文字(`compose::insert_image` 的文档约定),
    /// url 清空由用户填。
    fn open_image_dialog(&mut self) {
        let tab = self.tabs.current();
        let selection = tab.selection.unwrap_or((0, 0));
        let start = selection.0.min(selection.1);
        let stop = selection.0.max(selection.1);
        let text = tab.editor.text();
        // 选区是字符偏移,取文字前折成字节(compose 全族同款口径)。
        let alt = if stop > start {
            text.get(tab.editor.char_to_byte(start)..tab.editor.char_to_byte(stop))
                .unwrap_or("")
                .to_owned()
        } else {
            String::new()
        };
        self.image_dialog = ImageDialogState {
            open: true,
            alt,
            url: String::new(),
            // 图床选择沿用上次(关框时保留),没有则由 UI 默认选第一个
            bed: self
                .image_dialog
                .bed
                .clone()
                .or_else(|| self.bed.profiles.first().map(|profile| profile.id.clone())),
        };
    }

    /// 图片框「插入」:写回路径与 [`Self::apply_format`] 同款(整篇替换 +
    /// `pending_selection` 回填 + `cursor.byte` 边界折算),新选区落在
    /// alt 位便于直接覆写;完成后关框清草稿。
    fn insert_image(&mut self, alt: &str, url: &str) {
        self.close_image_dialog();
        // 防御:地址为空 UI 侧「插入」按钮已禁用,这里兜底不动文档。
        if url.trim().is_empty() {
            return;
        }
        let index = self.tabs.active;
        self.insert_image_at(index, alt, url);
    }

    /// 把 `![alt](url)` 写进**指定标签**(图床上传收尾的写入路径:目标按
    /// 发起标签定位而非当前标签,与 [`Self::append_ai_delta`] 同不变量)。
    fn insert_image_at(&mut self, index: usize, alt: &str, url: &str) {
        let tab = &mut self.tabs.tabs[index];
        let selection = tab.selection.unwrap_or((0, 0));
        let start = selection.0.min(selection.1);
        let stop = selection.0.max(selection.1);
        let (text, new_selection) =
            crate::compose::insert_image(tab.editor.text(), start..stop, url, alt);
        tab.editor.replace_all(&text);
        tab.pending_selection = Some((new_selection.start, new_selection.end));
        if let Some(byte) = tab.cursor.byte {
            tab.cursor.byte = Some(tab.editor.char_to_byte(tab.editor.byte_to_char(byte)));
        }
    }

    /// 关闭图片框并清空草稿(插入与取消共用);图床下拉的选择**保留** ——
    /// 换个位置插图时大概率还是同一个图床,不该每次重选。
    fn close_image_dialog(&mut self) {
        let bed = self.image_dialog.bed.clone();
        self.image_dialog = ImageDialogState {
            bed,
            ..ImageDialogState::default()
        };
    }

    /// Emoji 面板开/关(docs/emoji-plan.md E1):开时清搜索词(上次的
    /// 搜索词对下一次插入没有意义,与图片框关框清草稿同一取舍),分类与
    /// 「最近使用」沿用。
    fn toggle_emoji_panel(&mut self, open: bool) {
        self.emoji.open = open;
        if open {
            self.emoji.query.clear();
        }
    }

    /// Emoji 面板点选(docs/emoji-plan.md §6.4):插入 → 关面板 → 记
    /// 「最近使用」。写回路径与 [`Self::apply_format`] 同款(整篇替换 +
    /// `pending_selection` 回填 + `cursor.byte` 边界折算);打碎 TextEdit
    /// 内建 undo 的代价同 §9 R3,已知并接受(emoji-plan §7 #5)。
    fn insert_emoji(&mut self, emoji: &str) {
        self.emoji.open = false;
        // 防御:空载荷(UI 不会产出)不动文档,只关面板
        if emoji.is_empty() {
            return;
        }
        let tab = self.tabs.current_mut();
        let selection = tab.selection.unwrap_or((0, 0));
        let start = selection.0.min(selection.1);
        let stop = selection.0.max(selection.1);
        let (text, new_selection) =
            crate::compose::insert_emoji(tab.editor.text(), start..stop, emoji);
        tab.editor.replace_all(&text);
        tab.pending_selection = Some((new_selection.start, new_selection.end));
        if let Some(byte) = tab.cursor.byte {
            tab.cursor.byte = Some(tab.editor.char_to_byte(tab.editor.byte_to_char(byte)));
        }
        // 最近使用:去重置顶、封顶截断;同步进 theme 后走主题的既有落盘
        // 路径写 settings.json(E2,docs/emoji-plan.md §6.3「与 ThemeSettings
        // 同路」)。落盘失败只落提示行,插入本身照常生效 —— 与切主题同口径。
        let recent = &mut self.emoji.recent;
        recent.retain(|seen| seen != emoji);
        recent.insert(0, emoji.to_owned());
        recent.truncate(EMOJI_RECENT_CAP);
        self.theme.emoji_recent = self.emoji.recent.clone();
        self.persist_theme();
    }

    /// 「浏览…」(`Message::ImageFilePickRequested` 的归约):弹图片选择框,
    /// 选中即复制进 `.assets/` 并回填草稿(docs/image-plan.md B 段)。对话框
    /// 保持打开 —— 浏览只产地址,文本写入仍归「插入」。
    ///
    /// 复制发生在浏览时而非插入时:url 栏回填的是**真实落盘地址**(撞名
    /// 后缀已定),用户所见即所插;代价是浏览后取消会在 `.assets/` 留一份
    /// 未引用文件 —— 该目录本就是文档的附件区,孤儿文件不破坏任何引用,
    /// 取舍已登记 decisions-pending #37。
    fn pick_image_file(&mut self) {
        let Some(doc) = self.tabs.current().document.path.clone() else {
            self.tabs.current_mut().document.notice =
                Some("请先保存文档再插入本地图片 —— 图片要复制到文档旁的 .assets/ 目录".to_owned());
            return;
        };
        let start = file::start_dir(Some(&doc));
        if let Some(picked) = file::pick_image_dialog(&start) {
            self.import_image_file(&picked);
        }
    }

    /// 浏览结果落草稿(与 rfd 弹框拆开,便于无头单测直接喂路径)。
    fn import_image_file(&mut self, picked: &Path) {
        let Some(doc) = self.tabs.current().document.path.clone() else {
            return; // pick 侧已提示过;这里是防御(两次点击之间文档被换)
        };
        match crate::assets::import_file(&doc, picked) {
            Ok(stored) => {
                let dialog = &mut self.image_dialog;
                dialog.url = stored.url;
                // alt 空着才补:用户手填(或选区预填)的优先
                if dialog.alt.trim().is_empty() {
                    dialog.alt = stored
                        .file_name
                        .rsplit_once('.')
                        .map(|(stem, _)| stem.to_owned())
                        .unwrap_or(stored.file_name);
                }
            }
            Err(error) => self.tabs.current_mut().document.notice = Some(error.to_string()),
        }
    }

    /// 图片框「选文件并上传…」(`Message::ImageUploadRequested` 的归约,
    /// docs/image-plan.md C 段):选文件 → 关框 → 发起后台上传。alt 在此刻
    /// 定格(空则补文件名,与「浏览…」同口径);上传绑定当前标签,收尾时
    /// 无论用户切到哪个标签都写回发起标签。未选图床 / 未配置 profile 只落
    /// 提示,不开文件框。
    fn request_image_upload(&mut self) {
        if self.bed.is_uploading() {
            return; // UI 已禁用按钮,防御旧消息重放
        }
        let Some(profile_id) = self.image_dialog.bed.clone() else {
            self.tabs.current_mut().document.notice =
                Some("请先在 设置 → 图片 里配置图床".to_owned());
            return;
        };
        let Some(profile) = self.bed.profile(&profile_id).cloned() else {
            self.tabs.current_mut().document.notice =
                Some("所选图床不存在,请在 设置 → 图片 里检查".to_owned());
            return;
        };
        let start = file::start_dir(self.tabs.current().document.path.as_deref());
        let Some(picked) = file::pick_image_dialog(&start) else {
            return; // 取消:框不关、状态不动
        };
        let alt = {
            let dialog = &self.image_dialog;
            let fallback = picked
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            let alt = dialog.alt.trim().to_owned();
            if alt.is_empty() {
                fallback
            } else {
                alt
            }
        };
        // 关框:上传成功即自动插入,这里没有留给用户点的「插入」;图床选择
        // 由 close 的保留语义带走
        self.close_image_dialog();
        self.tabs.current_mut().document.notice =
            Some(format!("正在上传到 {}…", profile.display_name()));
        let tab_id = self.tabs.current().id;
        // 上传网络 IO 在后台线程；结果入队后主动唤醒一帧，再由归约插入。
        self.bed.start_notifying(
            profile,
            picked,
            BedUploadPurpose::Insert { alt },
            Some(tab_id),
            self.repaint.clone(),
        );
    }

    /// 上传收尾(`Message::ImageUploadFinished` 的归约):序号不匹配(旧
    /// 请求的结果)静默丢弃;匹配则按用途分流 —— 插入型写**发起标签**
    /// (标签已被关则丢弃),测试型回显设置页。失败只落提示行,文档与
    /// 选区绝不动(image-plan §4.3:失败的唯一后果是不插入文本)。
    fn finish_image_upload(&mut self, seq: u64, result: Result<String, String>) {
        if !self.bed.finish(seq) {
            return; // 旧结果:已被更新的请求取代,丢弃
        }
        match self.bed.take_purpose() {
            Some(BedUploadPurpose::Insert { alt }) => {
                let index = self
                    .bed
                    .take_upload_tab()
                    .and_then(|id| self.tabs.index_by_id(id));
                let Some(index) = index else {
                    return; // 发起标签已关:无处可写,丢弃(与 AI 流式同款)
                };
                match result {
                    Ok(url) => {
                        self.insert_image_at(index, &alt, &url);
                        self.tabs.tabs[index].document.notice = None;
                    }
                    Err(error) => self.tabs.tabs[index].document.notice = Some(error),
                }
            }
            Some(BedUploadPurpose::Test { profile_name }) => {
                self.bed.last_test = Some((profile_name, result));
            }
            None => {}
        }
    }

    /// 发起剪贴板图片读取(`Message::ImagePasteRequested` 的归约,
    /// docs/image-plan.md D 段):spawn 后台线程,立即返回。防重入不拦 ——
    /// 新读取直接换接收端,旧线程的结果无处可去(与图床上传同手法)。
    fn request_image_paste(&mut self) {
        // 系统剪贴板读取和 PNG 编码可能阻塞；完成后主动唤醒一帧收口。
        self.clipboard.start_with_repaint(self.repaint.clone());
    }

    /// 发起命令行工具安装(`Message::InstallCliRequested` 的归约,
    /// 2026-10-23 mac 反馈 #3):仅 .app 打包运行才装 —— 开发裸跑装出去
    /// 的软链指向 target/debug 的临时二进制,重编译即失效。osascript 的
    /// 管理员授权弹窗与等待都在后台线程,归约立即返回;防重入不拦
    /// (换接收端,与剪贴板读取同手法)。
    fn request_cli_install(&mut self) {
        let bundled = std::env::current_exe()
            .ok()
            .filter(|exe| crate::cli_install::is_bundled_macos_app(exe));
        let Some(exe) = bundled else {
            self.tabs.current_mut().document.notice =
                Some("命令行工具仅在 .app 安装运行时提供(开发运行不安装)".to_owned());
            return;
        };
        self.cli_install
            .start_with_repaint(self.repaint.clone(), exe);
    }

    /// 命令行安装收尾(`Message::CliInstallFinished` 的归约):结果只落
    /// 提示行,失败不动任何状态(重试 = 再点一次菜单,`ln -sf` 幂等)。
    fn finish_cli_install(&mut self, result: Result<(), String>) {
        self.cli_install.finish();
        let notice = match result {
            Ok(()) => "已安装 latermd 命令到 /usr/local/bin —— 新开终端即可 \
                       latermd 文档.md 打开文档"
                .to_owned(),
            Err(error) => format!("命令行工具安装失败:{error}"),
        };
        self.tabs.current_mut().document.notice = Some(notice);
    }

    /// 剪贴板读取收尾(`Message::ImagePasteFinished` 的归约):PNG 字节过
    /// 5MB 上限 → 落 `.assets/`(剪贴板没有原名,合成 `粘贴图片-<时间戳>`)
    /// → 当前光标处插相对路径引用(alt 空 —— 粘贴图没有可推断的说明,
    /// 用户回头补)。失败只落提示行,文档与选区绝不动(image-plan §4.3
    /// 同口径:失败的唯一后果是不插入文本)。
    fn finish_image_paste(&mut self, result: Result<Vec<u8>, String>) {
        self.clipboard.finish();
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(error) => {
                self.tabs.current_mut().document.notice = Some(error);
                return;
            }
        };
        let Some(doc) = self.tabs.current().document.path.clone() else {
            self.tabs.current_mut().document.notice =
                Some("请先保存文档再粘贴图片 —— 图片要存到文档旁的 .assets/ 目录".to_owned());
            return;
        };
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let name = crate::assets::pasted_image_name(nanos);
        match crate::assets::store_pasted_image(&doc, &name, &bytes) {
            Ok(stored) => {
                let index = self.tabs.active;
                self.insert_image_at(index, "", &stored.url);
            }
            // 归约侧先建目录再写盘,超限在写盘之前就拒了,不会留半个 .assets/
            Err(error) => self.tabs.current_mut().document.notice = Some(error),
        }
    }

    /// 拖入图片文件(`Message::ImageFileDropped` 的归约,docs/image-plan.md
    /// D 段):读文件 → 白名单 + 5MB 判定 → 落 `.assets/`(原名,撞名 -1
    /// 后缀)→ 光标处插引用。读的是本地磁盘(毫秒级,与文件树打开文件
    /// 同口径),同步做;白名单外或超限**只弹 notice,不改文档**。
    fn drop_image_file(&mut self, path: &Path) {
        let Some(doc) = self.tabs.current().document.path.clone() else {
            self.tabs.current_mut().document.notice =
                Some("请先保存文档再拖入图片 —— 图片要复制到文档旁的 .assets/ 目录".to_owned());
            return;
        };
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.tabs.current_mut().document.notice =
                    Some(format!("读取拖入文件失败 {}: {error}", path.display()));
                return;
            }
        };
        if let Err(notice) = crate::assets::check_dropped_file(path, bytes.len() as u64) {
            self.tabs.current_mut().document.notice = Some(notice);
            return;
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        match crate::assets::store(&doc, &name, &bytes) {
            Ok(stored) => {
                let index = self.tabs.active;
                self.insert_image_at(index, "", &stored.url);
            }
            Err(error) => self.tabs.current_mut().document.notice = Some(error.to_string()),
        }
    }

    /// 保存图床 profile(`Message::BedProfileSaved` 的归约):token 先写
    /// 系统凭据(失败落提示但**继续** —— profile 本身无秘密,存下来用户
    /// 稍后重存 token 即可),再 upsert 进 `beds.json`(落盘失败不改内存
    /// 列表,与 AI/MCP 配置同哲学)。
    fn save_bed_profile(&mut self, mut profile: latermd_bed::BedProfile, token: Option<String>) {
        profile.normalize();
        if profile.id.is_empty() {
            profile.id = crate::bed::new_profile_id();
        }
        if let Some(token) = token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            if let Err(error) =
                latermd_creds::set_secret(crate::bed::CREDS_SERVICE, &profile.id, token)
            {
                self.tabs.current_mut().document.notice = Some(format!(
                    "token 未保存({error});图床配置仍已保存,请重试 token"
                ));
            }
        }
        let mut profiles = self.bed.profiles.clone();
        match profiles.iter_mut().find(|slot| slot.id == profile.id) {
            Some(slot) => *slot = profile.clone(),
            None => profiles.push(profile.clone()),
        }
        if let Some(dir) = self.config_dir() {
            if let Err(error) = BedState::save_to(&profiles, &dir) {
                self.tabs.current_mut().document.notice = Some(format!("图床配置保存失败:{error}"));
                return;
            }
        }
        self.bed.profiles = profiles;
    }

    /// 删除图床 profile(`Message::BedProfileDeleted` 的归约):凭据删除
    /// 幂等(失败只提示,token 留在钥匙串无害);`beds.json` 落盘失败不改
    /// 内存列表。
    fn delete_bed_profile(&mut self, id: String) {
        if let Err(error) = latermd_creds::delete_secret(crate::bed::CREDS_SERVICE, &id) {
            self.tabs.current_mut().document.notice =
                Some(format!("图床 token 删除失败({error}),配置继续删除"));
        }
        let profiles: Vec<latermd_bed::BedProfile> = self
            .bed
            .profiles
            .iter()
            .filter(|profile| profile.id != id)
            .cloned()
            .collect();
        if let Some(dir) = self.config_dir() {
            if let Err(error) = BedState::save_to(&profiles, &dir) {
                self.tabs.current_mut().document.notice = Some(format!("图床配置保存失败:{error}"));
                return;
            }
        }
        self.bed.profiles = profiles;
    }

    /// 测试上传(`Message::BedTestUploadRequested` 的归约):选文件并发起
    /// 后台上传,结果回显设置页(不插入任何文档)。上传中忽略(UI 已禁用
    /// 按钮,防御)。
    fn test_bed_upload(&mut self, profile_id: String) {
        if self.bed.is_uploading() {
            return;
        }
        let Some(profile) = self.bed.profile(&profile_id).cloned() else {
            return;
        };
        let start = file::start_dir(self.tabs.current().document.path.as_deref());
        let Some(picked) = file::pick_image_dialog(&start) else {
            return;
        };
        self.bed.last_test = None; // 清旧回显,避免误读为本次结果
        let profile_name = profile.display_name().to_owned();
        // 图床连通性测试与真实上传共用通知机制，完成后无需满帧轮询。
        self.bed.start_notifying(
            profile,
            picked,
            BedUploadPurpose::Test { profile_name },
            None,
            self.repaint.clone(),
        );
    }

    /// 图床上传收流(每帧归约调用一次):channel 里的结果翻成消息,由调用
    /// 方并入本帧归约队列(与 [`Self::poll_ai`] 同分工)。
    pub fn poll_bed(&mut self) -> Vec<Message> {
        self.bed.poll()
    }

    /// 装载布局后恢复编辑模式。内存 Default 保留源码测试夹具；真实启动
    /// 无模式记录时进入写作。缓冲与撤销栈始终不重建。
    pub fn restore_layout(&mut self, layout: LayoutSettings) {
        let mode = layout.render_mode.unwrap_or(RenderMode::Live);
        self.layout = layout;
        let mode = match self.theme.default_view_mode {
            crate::theme::DefaultViewMode::Live => RenderMode::Live,
            crate::theme::DefaultViewMode::Source => RenderMode::Source,
            crate::theme::DefaultViewMode::Split => RenderMode::Split,
            crate::theme::DefaultViewMode::Last => mode,
        };
        self.render_mode = RenderMode::Source;
        // 旧存档只有 source / live 两档;split 是新值,同一条装载路径。
        if mode != RenderMode::Source {
            // Live 一档(Split 要先经过 Live 才能到,与点开关的循环同序)
            self.cycle_render_mode();
            if mode == RenderMode::Split {
                self.cycle_render_mode();
            }
        }
    }

    /// 循环切档(`Ctrl/Cmd+/` 与视图菜单):按 [`Self::set_render_mode`]
    /// 推进一档,交接逻辑全在那边。
    fn cycle_render_mode(&mut self) {
        self.set_render_mode(self.render_mode.cycle());
    }

    /// 把视图设到指定档。只翻标志 + 按档位做必要的块表/插入点交接,不做
    /// 「离开某模式时右栏该不该回来」那类快照补工 —— 三态互斥,右栏的
    /// 开关就是 `render_mode` 本身。目标等于当前档时是 no-op。
    fn set_render_mode(&mut self, target: RenderMode) {
        if self.render_mode == target {
            return;
        }
        // 先退出禅定再切换,避免禅定布局被写模式借用。
        if self.layout.zen {
            self.toggle_zen();
        }
        let entering_live = self.render_mode == RenderMode::Source && target == RenderMode::Live;
        let leaving_live = self.render_mode == RenderMode::Live && target != RenderMode::Live;
        // 进场来源只记「切进来之前是哪一档」。分栏动画按它分通道(见
        // `split_enter_from` 文档):写作档切入走向右滑,源码档切入保留
        // 旧观感(从右缘向左进)。目标不是 Split 时**不写** —— 留着上一次的
        // 值即可,真正读它的只有进场那几帧。
        if target == RenderMode::Split {
            self.split_enter_from = self.render_mode;
        }
        self.render_mode = target;
        self.layout.render_mode = Some(self.render_mode);
        match self.render_mode {
            RenderMode::Live => {
                let tab = self.tabs.current_mut();
                // 打开/切换文档没有源码插入点时保持浏览态；显式从源码切入
                // 且已有插入点时，把它交给对应 Live 块。
                tab.live.browse_enabled = true;
                tab.live.active = None;
                tab.live.pending_caret = None;
                tab.live.pending_range = None;
                let byte = tab.cursor.byte;
                // 从源码档切入时保留源码插入点；新打开/切换文档时
                // cursor.byte 通常为空，仍保持浏览态，等待用户单击进编辑。
                if entering_live {
                    if let Some(byte) = byte {
                        tab.live.browse_only = false;
                        tab.live.sync(&tab.editor, Some(byte));
                        if let Some(index) = tab.live.block_containing(byte) {
                            let local = tab.editor.byte_to_char(byte)
                                - tab.editor.byte_to_char(tab.live.blocks[index].start);
                            tab.live.active = Some(index);
                            tab.live.pending_caret = Some((index, local));
                            tab.live.caret_follow = false;
                        } else {
                            tab.live.browse_only = true;
                        }
                    } else {
                        tab.live.browse_only = true;
                    }
                } else {
                    tab.live.browse_only = true;
                }
                // 小文档立即建立块表；大文档把解析留给 Live 首帧，避免
                // 状态归约与模式动画同帧阻塞 UI。显式保留插入点时已在上面
                // 同步；浏览态也建立块表以便首帧直接显示富渲染。
                if tab.live.blocks.is_empty() && tab.editor.text().len() <= 64 * 1024 {
                    tab.live.sync(&tab.editor, None);
                }
            }
            // Split 的中央仍是源码 TextEdit(与 Source 同一个 widget id),
            // 光标/选区无需交接;右栏是只读渲染,状态每帧由 UI 侧置位。
            RenderMode::Split | RenderMode::Source => {
                if leaving_live {
                    // 离开 Live 前把最后一个活动插入点交给源码 TextEdit。源码和
                    // Live 使用不同的稳定 widget id，若不显式写回，切换后 egui
                    // 会显示文本却丢掉原来的插入位置和焦点。
                    let tab = self.tabs.current_mut();
                    if let Some(byte) = tab.cursor.byte {
                        let char_idx = tab.editor.byte_to_char(byte);
                        tab.pending_selection = Some((char_idx, char_idx));
                    }
                }
            }
        }
    }

    /// 主题落盘;失败只落提示行(切换已在内存生效,不回滚)。
    fn persist_theme(&mut self) {
        if let Err(error) = self.theme.save_to(self.settings_dir.as_deref()) {
            self.tabs.current_mut().document.notice = Some(error.to_string());
        }
    }

    /// 选皮肤:内容从目录里取(名字不在目录中则回落默认),随后落盘。
    fn apply_skin(&mut self, name: Option<String>) {
        let catalog = self.skins.clone();
        self.theme.select_skin(name.as_deref(), &catalog);
        self.persist_theme();
    }

    /// 导出当前正文样式为皮肤文件 → 重扫目录 → 选中它(所见即所得:导出的
    /// 就是此刻看到的样式)。
    fn export_skin(&mut self, name: &str) {
        let Some(dir) = self.config_dir() else {
            self.tabs.current_mut().document.notice =
                Some("找不到配置目录,无法导出皮肤".to_owned());
            return;
        };
        let style = self.theme.markdown_style();
        match crate::theme::export_skin(&dir, name, &style, self.theme.shell_palette()) {
            Ok(path) => {
                self.skins = SkinCatalog::load_from(&dir);
                let stem = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.apply_skin(Some(stem));
                self.tabs.current_mut().document.notice =
                    Some(format!("已导出皮肤:{}", path.display()));
            }
            Err(error) => {
                self.tabs.current_mut().document.notice = Some(error);
            }
        }
    }

    /// 保存 MCP 配置(设置页「保存」):归一化 → 落 `mcp.json` → 按开关起停
    /// 后台服务。**默认关闭**,开启才监听回环端口(docs/mcp-plan.md §5)。
    ///
    /// 与 `apply_ai_config` 同款落盘优先:持久化失败就不改内存配置,避免
    /// 「界面显示已开启、重启却回到关闭」。
    fn apply_mcp_config(&mut self, config: McpConfig) {
        if let Some(dir) = self.config_dir() {
            let mut saved = config.clone();
            saved.normalize();
            if let Err(error) = saved.save_to(&dir) {
                self.tabs.current_mut().document.notice = Some(format!("MCP 配置保存失败:{error}"));
                return;
            }
        }
        // 服务以「当前文件树根」为检索边界:换根时同步给运行中的 server
        self.mcp.set_root(self.file_tree.root.clone());
        // 设置页重启 MCP 后不做帧轮询；绑定结果到达时由通知器唤醒。
        self.mcp
            .apply_config_with_repaint(config, self.repaint.clone());
    }

    /// 保存 AI 配置(设置页「保存」):归一化 → 落 `ai.json` → 即时重装配
    /// provider 运行时(无需重启)。落盘失败则**不**改内存配置 —— 否则
    /// 界面显示已生效、重启却回到旧值。
    fn apply_ai_config(&mut self, mut config: AiConfig) {
        config.normalize();
        if let Some(dir) = self.config_dir() {
            if let Err(error) = config.save_to(&dir) {
                self.tabs.current_mut().document.notice = Some(format!("AI 配置保存失败:{error}"));
                return;
            }
        }
        let key = self.ai_key.creds.ai_api_key();
        self.ai.set_provider(config, key.as_deref());
    }

    /// 「获取模型列表」(`Message::AiModelsFetchRequested` 的归约):按
    /// 草稿装配列表来源并发起后台拉取。Mock 不联网忽略;拉取中忽略
    /// (UI 已禁用按钮,防御)。key 沿 AI 命令同一通道(latermd-creds →
    /// 环境变量)读取,缺失不在此拦 —— 让后台线程回「未配置 API key」
    /// 文案,错误行显示(错误路径是本功能的一等公民,见 #58 M2 任务书)。
    fn request_ai_models(&mut self) {
        let draft = self.settings.ai_draft.clone();
        let key = self.ai_key.creds.ai_api_key();
        let Some(source) = models_source_for(&draft, key) else {
            return;
        };
        // HTTP 拉取完成后主动唤醒一帧；列表和错误仍只在消息归约中落地。
        self.settings
            .models
            .start_with_repaint(source, self.repaint.clone());
    }

    /// 模型列表收流(每帧归约调用一次):channel 里的结果翻成消息,由
    /// 调用方并入本帧归约队列(与 [`Self::poll_ai`] 同分工)。
    pub fn poll_models(&mut self) -> Vec<Message> {
        self.settings.models.poll()
    }

    /// 检查更新收流(#71 M2,与 [`Self::poll_models`] 同分工):关于窗
    /// 后台线程的结果翻成消息,由 `ui::layout::reduce` 并入本帧归约。
    pub fn poll_about(&mut self) -> Vec<Message> {
        self.about.update.poll()
    }

    /// 切换主题(设置菜单的归约):改状态并即时落盘(重启保持);投影到
    /// context 由每帧的 `theme.apply` 完成。落盘失败只落提示行,切换本身
    /// 照常生效 —— 持久化失败不该牺牲本次会话的可用性。
    fn change_theme(&mut self, mode: ThemeMode) {
        self.theme.mode = mode;
        // 切到「跟随系统」立即探测一次:否则要等下一个轮询点才生效
        if mode == ThemeMode::System {
            self.refresh_system_theme();
            self.system_theme_due = None;
        }
        if let Err(error) = self.theme.save_to(self.settings_dir.as_deref()) {
            self.tabs.current_mut().document.notice = Some(error.to_string());
        }
    }

    /// 打开 `[[wikilink]]` 指向的文档:在文档库根下按文件名找(忽略大小写与
    /// `.md`/`.markdown` 扩展名差异),找到即走与文件树点击同一条 `open_path`
    /// (路径去重、脏标签规则都在那里);找不着落提示行。
    ///
    /// 没选文档库根时直接提示 —— 与 MCP 工具同口径:没有检索范围就不猜。
    fn open_wikilink(&mut self, target: &str) {
        let Some(root) = self.file_tree.root.clone() else {
            self.tabs.current_mut().document.notice =
                Some("未设置文件树根目录,无法跳转链接".to_owned());
            return;
        };
        match crate::filetree::find_by_name(&root, target) {
            Some(path) => {
                self.open_path(&path);
            }
            None => {
                self.tabs.current_mut().document.notice =
                    Some(format!("文档库里没有「{target}」这篇文档"));
            }
        }
    }

    /// 打开指向本地文档的普通链接(`Message::DocLinkClicked` 的归约,
    /// [`crate::doclink`] 的解析落点):相对路径按当前文档目录、库根相对与
    /// 全库按名依次兜底;带锚点则打开后把光标落到标题行。找不到落提示行,
    /// 与 [`Self::open_wikilink`] 同一纪律 —— 不静默无反应。
    ///
    /// 纯锚点(`#标题`)不开标签,直接在**当前**文档内跳:TOC 生成的
    /// `[标题](#锚点)` 目录链接是第一用户。锚点定位以打开后的编辑器缓冲
    /// 为准(脏缓冲比盘上新,以用户看见的为准);找不到锚点同样落提示,
    /// 文档照常已打开。
    fn open_doc_link(&mut self, href: &str) {
        let link = crate::doclink::parse_local(href);
        let current = self.tabs.current().document.path.clone();
        let root = self.file_tree.root.clone();
        match crate::doclink::resolve(current.as_deref(), root.as_deref(), &link) {
            crate::doclink::Resolution::Miss(message) => {
                self.tabs.current_mut().document.notice = Some(message);
            }
            crate::doclink::Resolution::AnchorHere { anchor } => {
                self.jump_current_to_anchor(&anchor);
            }
            crate::doclink::Resolution::Found { path, anchor } => {
                let index = match self.tabs.find_by_path(&path) {
                    Some(index) => index,
                    None => match file::read(&path) {
                        Ok(text) => self.spawn_tab(Some(path.clone()), &text),
                        Err(error) => {
                            self.tabs.current_mut().document.notice = Some(error.to_string());
                            return;
                        }
                    },
                };
                self.switch_active(index);
                self.file_tree.expand_ancestors_of(&path);
                if let Some(anchor) = anchor {
                    self.jump_current_to_anchor(&anchor);
                }
            }
        }
    }

    /// 当前标签内按锚点跳标题(`open_doc_link` 的锚点半程):找不到落提示
    /// 行并返回 false,命中走 [`Self::jump_cursor_to_heading`](剥前导换行、
    /// 钳制过期 span,与大纲点击同一落点语义)。
    fn jump_current_to_anchor(&mut self, anchor: &str) -> bool {
        let text = self.tabs.current().editor.text().to_owned();
        match crate::doclink::find_anchor(&text, anchor) {
            Some(span) => {
                self.jump_cursor_to_heading(span);
                true
            }
            None => {
                self.tabs.current_mut().document.notice =
                    Some(format!("链接锚点「{anchor}」没有对应的标题"));
                false
            }
        }
    }

    /// 把编辑器光标跳到标题行首(大纲点击的归约)。
    ///
    /// span 平铺不变量使标题 span 可能吸收前一块尾部的换行(实测 `## X` 的
    /// span 起于其前的空行),跳过换行让光标落在标题行首。区间来自点击帧
    /// 的快照,同帧编辑器面板仍可能改动文本,归约时缓冲或已变短:切片前
    /// 按当前长度钳制,`byte_to_char` 把落在字符中间的偏移归到起点。
    fn jump_cursor_to_heading(&mut self, span: Range<usize>) {
        let tab = self.tabs.current_mut();
        let mut byte = span.start.min(tab.editor.text().len());
        for b in &tab.editor.text().as_bytes()[byte..] {
            match b {
                b'\n' | b'\r' => byte += 1,
                _ => break,
            }
        }
        tab.cursor.jump_to = Some(tab.editor.byte_to_char(byte));
    }

    /// 帧末刷新派生状态。`App::logic` 每帧调用一次。
    pub fn end_of_logic(&mut self) {
        let dirty = self.tabs.current().editor.is_dirty();
        self.tabs.current_mut().document.dirty = dirty;
        // 后台 Git 刷新收口(与反向链接同款:帧末非阻塞收一次,迟到结果
        // 按代际号丢弃)。没有在途刷新时本次是空转的一次原子读。
        self.poll_git();
        // 自动保存帧末归约(#18):停顿/切出判定 + 落 draft。时刻参数化,
        // 单测注入不真等 30s。
        self.autosave_pass(std::time::Instant::now());
        // 大纲的重建闸门(击键路径的实测账,见
        // [`PreviewState::sync_outline`]):只有侧栏**正在显示大纲页**时才让
        // 标题扫描跟着修订号跑。禅定档没有侧栏,同样不算。
        //
        // 写在帧末归约里是因为可见性归 `layout` 管,而 `sync_outline` 的调用
        // 点在 `ui::editor` / `live::ui`(绘制侧)—— 绘制前的这一帧写入,当帧
        // 绘制即可见,不晚一帧。
        let outline_shown =
            self.layout.left && !self.layout.zen && self.layout.left_view == SidebarTab::Outline;
        self.tabs.current_mut().preview.outline_shown = outline_shown;
        // 文件树懒加载落点:根 + 展开中目录的子项缓存补齐(键缺席才 IO)。
        self.file_tree.ensure_loaded();
        // 搜索结果收流:非阻塞收空 channel(重绘驱动见 `ui::layout::reduce`)。
        self.search.poll_hits();
        // 反向链接(#15):目标快照比对(变化才防抖重扫)+ 结果收流。
        self.sync_backlink_target();
        self.backlinks.poll();
        // MCP:收绑定结果与调用计数(两者都是非阻塞的轻量检查)
        self.mcp.poll();
        // 外壳布局落盘的唯一写入点:与上次写下的一份比对,变了才写。
        //
        // **为什么是这里而不是各条消息**:左右两栏的面板把手在 `ui` 里原地
        // 翻转 `&mut bool`(egui 内建的收起/拖回动画都在那条路径上),
        // 根本不产消息 —— 只在归约侧写盘会把拖把手这个最常用的入口漏掉。
        // 比对写排除了闲置帧的重复 IO(每帧 serialize + write 不可接受)。
        if self.layout != self.layout_written {
            let snapshot = self.layout.clone();
            if self.layout.save_to(self.settings_dir.as_deref()).is_ok() {
                self.layout_written = snapshot;
            }
        }
    }

    /// 自动保存的帧末归约(#18,`end_of_logic` 的一部分):
    ///
    /// 1. 逐标签两件小事:刷新「上次缓冲改动时刻」—— AI 流可写非活动
    ///    标签,按标签各自比对修订号,不能只看当前;顺手撤下「用户已直接
    ///    编辑」的恢复条(见循环内注释,#18 恢复条的隐性裁决语义);
    /// 2. **切出标签**:本帧被切走的脏标签即刻落 draft,不等停顿(切走后
    ///    它不可见,是防丢的主要对象);
    /// 3. **当前标签**:dirty 且距上次改动 ≥ [`AUTOSAVE_IDLE`] 才落。
    ///
    /// 两条写盘路径都受「同一修订号已落过则跳过」约束;写入失败只落提示行,
    /// 绝不打断编辑。时刻由调用方传入(生产取真实时钟,单测注入)。
    fn autosave_pass(&mut self, now: std::time::Instant) {
        for tab in &mut self.tabs.tabs {
            // 用户无视恢复条直接编辑(缓冲脏)→ 撤下恢复条:已隐性选择以
            // 盘上版本续写,条再留着只会诱导一次「拿旧稿盖掉新稿」的误点。
            // draft **文件**不删 —— 防丢镜像交停顿/切出路径照常接管(本函数
            // 下半段就是它们)。AI 流写给非活动标签同样命中:文档一旦有了
            // 新的去向,旧稿的裁决权已经过期。
            if tab.recover.is_some() && tab.editor.is_dirty() {
                tab.recover = None;
            }
            let rev = tab.editor.revision();
            if rev != tab.autosave.seen_rev {
                tab.autosave.seen_rev = rev;
                tab.autosave.last_edit = Some(now);
            }
        }
        // 切出者按稳定 id 找回:同帧的关闭路径可能已把它移除,找不到即
        // 作废 —— 关闭钩子自己会清 draft。
        if let Some(id) = self.autosave_switch_out.take() {
            if let Some(index) = self.tabs.index_by_id(id) {
                if Self::tab_needs_draft(&self.tabs.tabs[index]) {
                    self.write_draft(index);
                }
            }
        }
        let active = self.tabs.active;
        let due = self.tabs.tabs[active]
            .autosave
            .last_edit
            .is_some_and(|at| now.duration_since(at) >= AUTOSAVE_IDLE);
        if Self::tab_needs_draft(&self.tabs.tabs[active]) && due {
            self.write_draft(active);
        }
    }

    /// draft 触发条件的公共判定:缓冲脏,且当前修订号还没落过盘。
    fn tab_needs_draft(tab: &TabState) -> bool {
        tab.editor.is_dirty() && tab.autosave.saved_rev != Some(tab.editor.revision())
    }

    /// 活动标签下一次「停顿落盘」的到点时刻(#18 帧饥饿修复的重绘驱动
    /// 数据源,`ui::layout::reduce` 消费):有待落的 draft 且已记到上次
    /// 编辑时刻,才存在值得醒着等的帧;`None` = 无事可等(不脏 / 同修订
    /// 号已落 / 尚无编辑时刻)。**只看活动标签**——停顿路径只写它,非活动
    /// 标签的落盘走切出即写,而切出必发生在用户动作的帧里,天然有帧。
    pub fn next_autosave_due(&self) -> Option<std::time::Instant> {
        let tab = self.tabs.current();
        if !Self::tab_needs_draft(tab) {
            return None;
        }
        tab.autosave.last_edit.map(|at| at + AUTOSAVE_IDLE)
    }

    /// 孤儿 draft 检测(#18 恢复条):文档旁是否遗留 `<doc>.latermd-draft`。
    /// 一次 `metadata` 同时回答存在性与 mtime(比 exists + metadata 少一半
    /// 系统调用,也免去两步之间文件消失的竞态);mtime 取不到(平台不支持)
    /// 以「存在」为准,`None` 容之。只探不读 —— 内容等用户点「恢复」才读,
    /// 打开文档的热路径上不多付一次全文 IO。
    fn detect_orphan_draft(doc: &Path) -> Option<DraftRecovery> {
        let path = Self::named_draft_path(doc);
        let metadata = std::fs::metadata(&path).ok()?;
        Some(DraftRecovery {
            mtime: metadata.modified().ok(),
            path,
        })
    }

    /// 恢复条「恢复」的归约([`Message::DraftRecovered`]):draft 内容读进
    /// 该标签缓冲并置 dirty,随后删 draft、清待恢复状态。
    ///
    /// * 写入走 [`EditorBuffer::replace_all`] —— 与格式动作/AI 写回同一条
    ///   「既有缓冲替换路径」:dirty 置位、修订号前进(预览快照随之重建),
    ///   Ctrl+Z 一步回退到打开时的盘上版本(undo 快照被整篇替换打碎成
    ///   一步,§9 R3 已知接受的同款代价);
    /// * 读取失败(坏内容/权限/文件已被外部删走)不 panic:提示行带路径与
    ///   原因,**draft 文件保留现场**,待恢复状态撤下 —— 恢复条是对用户的
    ///   承诺,留一个永远兑现不了的按钮只会反复戳同一下。
    fn recover_draft(&mut self, tab_id: u64) {
        let Some(index) = self.tabs.index_by_id(tab_id) else {
            return; // 标签已关:迟到消息 no-op,与 TabCloseConfirmed 同手法
        };
        let Some(recover) = self.tabs.tabs[index].recover.clone() else {
            return; // 已裁决过:重复消息 no-op
        };
        match file::read(&recover.path) {
            Ok(text) => {
                let tab = &mut self.tabs.tabs[index];
                tab.editor.replace_all(&text);
                // 光标字节快照按旧文本折出,整篇换入后可能落在字符中间
                // (状态栏同帧切片崩溃的同款教训,见 apply_format)。
                if let Some(byte) = tab.cursor.byte {
                    tab.cursor.byte = Some(tab.editor.char_to_byte(tab.editor.byte_to_char(byte)));
                }
                tab.recover = None;
                tab.document.notice = Some("已恢复未保存草稿(Ctrl+Z 可撤销)".to_owned());
                // 镜像已进缓冲,盘上不再需要;删失败静默 —— 保存/关闭的
                // 清理钩子还会按记忆落点兜底一次。
                let _ = std::fs::remove_file(&recover.path);
            }
            Err(error) => {
                let tab = &mut self.tabs.tabs[index];
                tab.recover = None;
                tab.document.notice = Some(format!(
                    "草稿恢复失败(文件保留在 {}):{error}",
                    recover.path.display()
                ));
            }
        }
    }

    /// 恢复条「丢弃」的归约([`Message::DraftDiscarded`]):删 draft、清待
    /// 恢复状态。缓冲本就是盘上版本、一动不动 —— 无可撤销是因为无可损失
    /// (拒绝一盘未保存稿,等价于它从未发生);提示行带路径留痕供追溯。
    fn discard_draft(&mut self, tab_id: u64) {
        let Some(index) = self.tabs.index_by_id(tab_id) else {
            return;
        };
        let Some(recover) = self.tabs.tabs[index].recover.take() else {
            return;
        };
        let _ = std::fs::remove_file(&recover.path);
        self.tabs.tabs[index].document.notice =
            Some(format!("已丢弃未保存草稿({})", recover.path.display()));
    }

    /// 命名文档的 draft 落点:`<doc>.latermd-draft`,追加完整文件名之后
    /// (保留 `a.md` 原名,不替换扩展名)。
    fn named_draft_path(path: &Path) -> PathBuf {
        let mut name = path.as_os_str().to_os_string();
        name.push(DRAFT_SUFFIX);
        PathBuf::from(name)
    }

    /// 未命名文档的 draft 落点:配置目录 `drafts/untitled-<标签id>`。
    /// id 稳定且不复用,多个未命名标签互不覆盖。无配置目录(极简环境)
    /// 返回 `None` —— 沿用「偏好不落盘不报错」的既有先例,静默放弃
    /// draft 而不是反复提示。
    fn untitled_draft_path(&self, tab_id: u64) -> Option<PathBuf> {
        self.config_dir().map(|dir| {
            dir.join(DRAFTS_DIR)
                .join(format!("untitled-{tab_id}{DRAFT_SUFFIX}"))
        })
    }

    /// 标签的 draft 落点:有落盘身份与原文件同目录,否则进状态目录。
    fn draft_path_for(&self, tab: &TabState) -> Option<PathBuf> {
        match tab.document.path.as_deref() {
            Some(path) => Some(Self::named_draft_path(path)),
            None => self.untitled_draft_path(tab.id),
        }
    }

    /// 把一个标签的缓冲全量落成 draft(调用方已判定该写)。走 [`file::write`]
    /// 的原子落盘,失败时盘上旧 draft 完好。父目录先建 —— 只为状态目录
    /// 的 `drafts/` 而设(命名文档的父目录即文档所在目录,幂等空操作);
    /// 建目录失败不单独报,让随后的 write 报真实原因。
    fn write_draft(&mut self, index: usize) {
        let Some(path) = self.draft_path_for(&self.tabs.tabs[index]) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tab = &mut self.tabs.tabs[index];
        match file::write(&path, tab.editor.text()) {
            Ok(()) => {
                tab.autosave.draft_path = Some(path);
                tab.autosave.saved_rev = Some(tab.editor.revision());
            }
            Err(error) => tab.document.notice = Some(format!("自动保存失败:{error}")),
        }
    }

    fn run_file_cmd(&mut self, cmd: FileCmd) {
        match cmd {
            FileCmd::New => {
                self.git.close_diff_workspace();
                // 多标签:新建永远开新标签,当前标签的未保存稿不受影响
                let index = self.spawn_tab(None, "");
                // 新建是明确的写入意图:按「默认模式」设置落档并把插入点落
                // 入空编辑器,用户按键即可开始,不受当前浏览/对照档遮挡。
                // 「上次选择」取创建时刻的会话档 —— 正在写作时新建,新文档
                // 直接落在可写的空文档块,不再退回浏览态等单击。
                let target = match self.theme.default_view_mode {
                    crate::theme::DefaultViewMode::Live => RenderMode::Live,
                    crate::theme::DefaultViewMode::Source => RenderMode::Source,
                    crate::theme::DefaultViewMode::Split => RenderMode::Split,
                    crate::theme::DefaultViewMode::Last => self.render_mode,
                };
                if target == RenderMode::Live {
                    // 已在写作档时 set_render_mode 是 no-op,但新标签的
                    // browse_only 出厂即浏览态,必须显式翻成写作。
                    self.set_render_mode(RenderMode::Live);
                    let tab = &mut self.tabs.tabs[index];
                    tab.live.browse_only = false;
                    tab.focus_live_on_next_frame = true;
                } else {
                    self.set_render_mode(target);
                    self.tabs.tabs[index].focus_editor_on_next_frame = true;
                }
            }
            FileCmd::Open => {
                self.git.close_diff_workspace();
                let start = file::start_dir(self.tabs.current().document.path.as_deref());
                if let Some(path) = file::open_dialog(&start) {
                    self.open_path(&path);
                }
            }
            FileCmd::Save => {
                let target = match self.tabs.current().document.path.clone() {
                    Some(path) => Some(path),
                    None => file::save_dialog(&file::start_dir(None), file::UNTITLED_FILE_NAME),
                };
                if let Some(path) = target {
                    self.save_to(path);
                }
            }
            FileCmd::SaveAs => {
                self.git.close_diff_workspace();
                let start = file::start_dir(self.tabs.current().document.path.as_deref());
                let default = self
                    .tabs
                    .current()
                    .document
                    .path
                    .as_deref()
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| file::UNTITLED_FILE_NAME.to_owned());
                if let Some(path) = file::save_dialog(&start, &default) {
                    self.save_to(path);
                }
            }
        }
    }

    /// 读盘并**开新标签**换入(多标签语义:打开不覆盖当前标签)。读取失败
    /// 只落提示行,当前标签的缓冲与 dirty 不受影响。返回是否成功
    /// (#45 TabRestore 据此决定关闭栈条目的去留)。
    ///
    /// 同时展开该文件在树内的祖先目录:无论从文件树、菜单还是搜索跳转打开,
    /// 该文件的高亮行都应当在 Files 页里可见。
    fn open_in_tab(&mut self, path: &Path) -> bool {
        match file::read(path) {
            Ok(text) => {
                self.spawn_tab(Some(path.to_path_buf()), &text);
                self.file_tree.expand_ancestors_of(path);
                true
            }
            Err(error) => {
                self.tabs.current_mut().document.notice = Some(error.to_string());
                false
            }
        }
    }

    /// 按路径打开文档的统一入口(菜单「打开」/ 文件树点击 / 搜索跳转):
    /// 该路径已在某标签打开则**激活它**(路径去重,同一路径至多一个标签),
    /// 否则读盘开新标签。返回是否成功(已开激活也算成功)。
    fn open_path(&mut self, path: &Path) -> bool {
        self.git.close_diff_workspace();
        if let Some(index) = self.tabs.find_by_path(path) {
            self.switch_active(index);
            true
        } else {
            self.open_in_tab(path)
        }
    }

    /// 恢复最近关闭的标签(`Message::TabRestore` 的归约,#45):关闭栈
    /// 后进先出,栈顶路径按 [`State::open_path`] 既有语义重开 —— 已在别处
    /// 打开则激活它,不开第二个。成功(含激活)即返回,该条出栈,连续
    /// 触发可逐条回走完整条栈;失败(文件被外部删/移,提示行已说明)同样
    /// 出栈并继续下一条 —— 死条目留在栈顶会让之后的每次恢复都先撞一次
    /// 失败,「回走整条栈」被堵死(decisions-pending #70)。栈空 no-op。
    fn restore_tab(&mut self) {
        while let Some(path) = self.tabs.recently_closed.pop() {
            if self.open_path(&path) {
                return;
            }
        }
    }

    /// 开新标签的统一入口。不动在途 AI 流:流绑定发起标签
    /// ([`State::ai_active_tab`]),新标签不是它的写入目标。开新标签同样
    /// 把原标签切出(#18):打开另一文件/新建时,旧标签的脏缓冲与显式切换
    /// 同样值得一份 draft,记切出 id 交帧末归约处理。
    ///
    /// 新标签认领文档路径的同时检测孤儿 draft(#18 恢复条):文件树点击、
    /// 打开对话框、搜索跳转、wikilink 跳转全部汇进这里,一处检测全覆盖;
    /// 命中置待恢复状态(路径 + mtime),恢复条在编辑区上方等用户裁决。
    /// 未命名新标签无路径可锚,不检测 —— 其 draft 按标签 id 命名,跨会话
    /// 对不上号(decisions-pending #63 已登记该局限)。
    fn spawn_tab(&mut self, path: Option<PathBuf>, text: &str) -> usize {
        let previous = self.tabs.current().id;
        self.autosave_switch_out = Some(previous);
        let index = self.tabs.open_tab(path, text);
        if let Some(tab) = self.tabs.tabs.get_mut(index) {
            if let Some(doc) = tab.document.path.clone() {
                tab.recover = Self::detect_orphan_draft(&doc);
            }
        }
        self.rescan_find_after_tab_switch(previous);
        // 换了文档:「文件历史」的文档同步位失效,下一次刷新跟随新文档
        // (右键显式查询只保到用户主动换 tab 为止)
        self.git.file_history_synced_doc = None;
        index
    }

    /// 激活某标签。不动在途 AI 流:chunk 的写入目标由发起标签 id 决定,
    /// 与当前标签无关(多标签 #11 的核心不变量)。切出脏标签时记下它的
    /// 稳定 id —— 帧末 `autosave_pass` 为它落 draft(#18 切换即落)。
    fn switch_active(&mut self, index: usize) {
        let previous = self.tabs.current().id;
        let changed = index != self.tabs.active && index < self.tabs.tabs.len();
        if changed {
            self.autosave_switch_out = Some(previous);
        }
        self.tabs.activate(index);
        self.rescan_find_after_tab_switch(previous);
        if changed {
            // 换了文档:「文件历史」的文档同步位失效(理由同 spawn_tab)
            self.git.file_history_synced_doc = None;
        }
    }

    /// 关闭请求(标签条 × / Ctrl+W):脏标签先弹确认模态,干净标签直接关。
    /// 确认目标存**稳定 id** 而非索引 —— 模态是非阻塞 Window,打开期间
    /// 其他关闭入口会使索引漂移,按漂移后的索引确认会关错标签
    /// (与 `ai_active_tab` 同手法,见 [`TabsState::confirm_close`])。
    fn request_close_tab(&mut self, index: usize) {
        if index >= self.tabs.tabs.len() {
            return;
        }
        if self.tabs.tabs[index].editor.is_dirty() {
            self.tabs.confirm_close = Some(self.tabs.tabs[index].id);
        } else {
            self.remove_tab(index);
        }
    }

    /// 真正移除(确认后或干净标签)。关掉的是在途流的发起标签则作废流
    /// —— 剩余 chunk 无处可写,落到任何别的标签都是写错文档。关闭同时
    /// 清掉该标签的 draft(#18):丢弃(确认关闭)与干净标签都不该在盘上
    /// 留冗余镜像 —— 记忆落点 + 按落盘身份/标签 id 推导的落点都删,防
    /// 「写过但记忆被换入重置」的漏网;文件不存在是常态,删失败静默。
    fn remove_tab(&mut self, index: usize) {
        if let Some(tab) = self.tabs.tabs.get(index) {
            let stale = tab.autosave.draft_path.clone();
            let derived = match tab.document.path.as_deref() {
                Some(path) => Some(Self::named_draft_path(path)),
                None => self.untitled_draft_path(tab.id),
            };
            for victim in [stale, derived].into_iter().flatten() {
                let _ = std::fs::remove_file(victim);
            }
        }
        let closing_stream_origin = self.ai_stream_tab_index() == Some(index);
        let closing_id = self.tabs.tabs.get(index).map(|tab| tab.id);
        let closing_polish_target = self
            .ai_polish
            .as_ref()
            .is_some_and(|session| Some(session.tab_id) == closing_id);
        let previous = self.tabs.current().id;
        self.tabs.remove(index);
        if closing_stream_origin {
            self.abort_ai_stream();
        } else if closing_polish_target {
            // 待确认的润色浮窗指向被关标签:确认目标已不存在,作废会话
            // (此刻流已结束,只清会话本身)
            self.ai_polish = None;
        }
        self.rescan_find_after_tab_switch(previous);
    }

    /// 批量关闭请求(`Message::TabBatchCloseRequested` 的归约,#37 右键
    /// 菜单):以被右键的标签为基准换算目标 id 队列,然后逐个走**单标签
    /// 关闭的同一条通路**(`remove_tab` / `confirm_close`):干净标签直接
    /// 关,脏标签弹同一个确认模态,确认一个关一个。
    ///
    /// 新的批量请求取代旧队列与进行中的确认模态:被顶掉的模态目标若属
    /// 新队列会再次弹出 —— 没有确认被吞,也没有标签被悄悄关掉。空的
    /// 目标队列(UI 已禁用对应菜单项)是 no-op,但仍会顶掉旧批量:菜单
    /// 动作本身就是新的用户意图。
    fn request_batch_close(&mut self, kind: crate::tabs::BatchClose, index: usize) {
        self.tabs.pending_close = self.tabs.batch_close_targets(kind, index);
        self.tabs.confirm_close = None;
        self.advance_batch_close();
    }

    /// 推进批量关闭队列(#37):队首 id 已不在(被其他路径关掉)跳过;干净
    /// 标签直接移除(同单标签关闭);脏标签把 `confirm_close` 指到它并
    /// 停下,由 [`Message::TabCloseConfirmed`] 确认后再回到这里继续、
    /// [`Message::TabCloseCancelled`] 清空队列终止。队列走空即批量自然
    /// 结束。AI 流保护继承自 `remove_tab`:关到发起标签即作废流,迟到
    /// chunk 在 `append_ai_delta` 里被丢弃,不落到别的标签。
    fn advance_batch_close(&mut self) {
        while let Some(&id) = self.tabs.pending_close.first() {
            let Some(index) = self.tabs.index_by_id(id) else {
                self.tabs.pending_close.remove(0);
                continue;
            };
            if self.tabs.tabs[index].editor.is_dirty() {
                self.tabs.confirm_close = Some(id);
                return;
            }
            self.tabs.pending_close.remove(0);
            self.remove_tab(index);
        }
    }

    /// 打开标签重命名浮窗(#37 右键菜单「重命名」,**显示别名**语义):
    /// 目标索引立即换算成稳定 id(菜单弹出帧的快照,归约时标签可能已变),
    /// 草稿预填该标签当前显示基础名(不带 dirty 星)。新请求顶掉旧浮窗
    /// (同一时间至多一个重命名)。过期索引 no-op。
    fn request_tab_rename(&mut self, index: usize) {
        if let Some(tab) = self.tabs.tabs.get(index) {
            let tab_id = tab.id;
            let draft = tab.label_base();
            self.tabs.rename = Some(crate::tabs::TabRename {
                tab_id,
                draft,
                focus_pending: true,
            });
        }
    }

    /// 确认重命名(#37):草稿 trim 后非空 → 置该标签的显示别名;空 →
    /// 拒绝并提示(浮窗与草稿保留,UI 的「确定」按钮在空草稿时本就禁用,
    /// 这里兜住绕过 UI 直发的消息)。别名是纯显示层,成功与否都**不动**
    /// path/缓冲/dirty/保存目标。目标标签已被关掉(`TabsState::remove`
    /// 已撤下浮窗,这里是防御)时 no-op。
    fn confirm_tab_rename(&mut self) {
        let Some(rename) = self.tabs.rename.take() else {
            return;
        };
        let Some(index) = self.tabs.index_by_id(rename.tab_id) else {
            return;
        };
        let name = rename.draft.trim();
        if name.is_empty() {
            self.tabs.rename = Some(rename);
            self.tabs.tabs[index].document.notice =
                Some("标签名不能为空;不改请点「取消」".to_owned());
            return;
        }
        self.tabs.tabs[index].alias = Some(name.to_owned());
    }

    /// 去抖到点发起搜索(`Message::SearchRequested` 的归约)。无根目录
    /// 不发起(该情形下 `ui` 层就不该发消息,这里防御性短路)。
    fn start_search(&mut self) {
        if let Some(root) = self.file_tree.root.clone() {
            // 搜索线程每次命中或结束后主动唤醒一帧，结果仍在帧末统一收流。
            self.search.start_with_repaint(&root, self.repaint.clone());
        } else {
            self.search.reset();
        }
    }

    /// 左栏自动展开并停 Search 页([`Message::SearchPanelRevealed`] 与
    /// [`Message::SearchQueryChanged`] 的公共归约)。禅定模式例外:
    /// `left=false` 是被 `enter_zen` 借走的临时态,擅自翻开既破坏沉浸,
    /// 又会被 `save_to` 的快照还原悄悄吞掉 —— 静默不展开。
    fn reveal_search_panel(&mut self) {
        if self.layout.zen {
            return;
        }
        self.layout.left = true;
        self.layout.left_view = SidebarTab::Search;
    }

    /// 「全部替换」请求([`Message::SearchReplaceAllRequested`] 的归约):
    /// 按当前命中折算受影响文件数挂起确认浮窗。零命中/无根短路(UI 侧
    /// 按钮已禁用,这里防御一致)。
    fn request_search_replace_all(&mut self) {
        if self.file_tree.root.is_none() || !self.search.can_replace_all() {
            return;
        }
        let root = self.file_tree.root.clone().expect("checked above");
        let paths = unique_search_files(&self.search.hits);
        if paths.is_empty() {
            return;
        }
        self.search.confirm_replace = Some(ReplacePlan {
            query: self.search.to_query(&root),
            replacement: self.search.replace_with.clone(),
            paths,
            generation: self.search.generation,
        });
    }

    /// 确认后的「全部替换」([`Message::SearchReplaceAllConfirmed`] 的归约):
    /// 对命中涉及的每个文件执行替换。
    ///
    /// - **已打开的标签改缓冲**:脏稿在替换后保持脏(用户未保存的修改
    ///   原样保留,落盘交给用户自己保存,盘上此刻**没有**该文件的新态);
    ///   原本干净的标签发生内容变更后同样置脏,关闭与草稿恢复才不会丢失替换。
    /// - **未打开的直接改盘**:仅接受合法 UTF-8(替换基于 Unicode 语义,
    ///   lossy 转换会把坏字节 U+FFFD 写回磁盘);读/写失败逐文件记账,
    ///   不回滚已成功的文件 —— 回执如实分列,由用户决定下一步。
    /// - 零命中的文件(命中快照过期)一个字节都不动。
    ///
    /// 完成后重发一次搜索(VS Code 同款):命中列表即时收敛,替干净的
    /// 文件从列表消失 —— 回执与列表互证。回执挂在 `last_replace` 供
    /// 状态行展示,下次输入变化才清。
    fn run_search_replace_all(&mut self) {
        let Some(plan) = self.search.confirm_replace.take() else {
            return;
        };
        let Some(root) = self.file_tree.root.clone() else {
            return;
        };
        if !self.search.can_replace_all()
            || plan.generation != self.search.generation
            || plan.query != self.search.to_query(&root)
            || plan.replacement != self.search.replace_with
        {
            return;
        }
        let query = plan.query;
        let replacement = plan.replacement;
        let paths = plan.paths;
        let mut report = ReplaceReport::default();
        for path in paths {
            let display = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            match self.tabs.find_by_path(&path) {
                Some(index) => {
                    let text = self.tabs.tabs[index].editor.text().to_owned();
                    match latermd_search::replace_in_text(&query, &replacement, &text) {
                        Ok((new_text, count)) if count > 0 => {
                            {
                                let tab = &mut self.tabs.tabs[index];
                                tab.editor.replace_all(&new_text);
                                // 整篇替换改变长度,旧字节光标可能漂出字符
                                // 边界(replace_all_in_doc 同款钳制)
                                if let Some(byte) = tab.cursor.byte {
                                    tab.cursor.byte = Some(
                                        tab.editor.char_to_byte(tab.editor.byte_to_char(byte)),
                                    );
                                }
                            }
                            report.files += 1;
                            report.replacements += count;
                        }
                        Ok(_) => {} // 命中快照过期:该文件当前文本零命中
                        Err(error) => report.failed.push((display, error.to_string())),
                    }
                }
                None => {
                    let outcome = std::fs::read(&path)
                        .map_err(|error| error.to_string())
                        .and_then(|bytes| {
                            String::from_utf8(bytes).map_err(|_| "非 UTF-8 文档,已跳过".to_owned())
                        })
                        .and_then(|text| {
                            latermd_search::replace_in_text(&query, &replacement, &text)
                                .map_err(|error| error.to_string())
                                .map(|(new_text, count)| (new_text, count, text))
                        });
                    match outcome {
                        Ok((new_text, count, _)) if count > 0 => {
                            match file::write_as("替换", &path, &new_text) {
                                Ok(()) => {
                                    report.files += 1;
                                    report.replacements += count;
                                }
                                Err(error) => report.failed.push((display, error.to_string())),
                            }
                        }
                        Ok(_) => {}
                        Err(reason) => report.failed.push((display, reason)),
                    }
                }
            }
        }
        self.search.last_replace = Some(report);
        self.start_search();
    }

    /// 反向链接的触发判定(#15):「(文件树根, 当前文档路径)」与面板的
    /// [`crate::backlink_panel::BacklinkState::requested_for`] 比对,变了才
    /// invalidate 顺延防抖 —— 快照比对而非逐入口插桩,打开/切换/保存/
    /// 另存/换根一处全覆盖(decisions-pending #88)。根或文档缺失时复位:
    /// 无目标可扫,面板给引导提示。文档**内容**变化不在此键上,不重扫
    /// (反向链接匹配路径,不匹配正文)。
    fn sync_backlink_target(&mut self) {
        let target = match (&self.file_tree.root, &self.tabs.current().document.path) {
            (Some(root), Some(doc)) => Some((root.clone(), doc.clone())),
            _ => None,
        };
        if self.backlinks.requested_for == target {
            return;
        }
        self.backlinks.requested_for = target.clone();
        match target {
            Some(_) => self.backlinks.invalidate(DEBOUNCE),
            None => self.backlinks.reset(),
        }
    }

    /// 点击搜索结果(`Message::SearchResultClicked` 的归约):文件与当前
    /// 不同则先打开(读盘失败只落提示行,不跳转),再把光标跳到命中行
    /// 行首——经 [`OutlineCursor::jump_to`] 由 `ui::editor` 覆写持久光标
    /// 并交还焦点。
    ///
    /// 行号来自点击时刻的搜索快照,打开后文件可能比搜索时短(外部修改、
    /// 或命中的本就是尚未落盘的另一版本):1 起行号先转 0 起,再由
    /// [`EditorBuffer::line_to_byte`] 按当前缓冲钳制到文末,绝不 panic
    /// (大纲跳转 span 过期的同款教训)。
    fn open_search_hit(&mut self, path: &Path, line_no: usize) {
        let index = match self.tabs.find_by_path(path) {
            Some(index) => index, // 已开:直接激活并跳行
            None => match file::read(path) {
                Ok(text) => self.spawn_tab(Some(path.to_path_buf()), &text),
                Err(error) => {
                    self.tabs.current_mut().document.notice = Some(error.to_string());
                    return; // 读盘失败:notice 已带原因,不跳
                }
            },
        };
        self.switch_active(index);
        self.file_tree.expand_ancestors_of(path);
        let tab = self.tabs.current_mut();
        // 行号来自点击时刻的搜索快照,文件可能已变短:line_to_byte 内部钳制
        let byte = tab.editor.line_to_byte(line_no.saturating_sub(1));
        tab.cursor.jump_to = Some(tab.editor.byte_to_char(byte));
    }

    /// 弹目录对话框选文件树根目录(Files 页「选择…」按钮的归约)。起始目录
    /// 取最近根或当前文档所在目录,均已校验存在(rfd 对不存在目录的行为未定义)。
    fn pick_file_tree_root(&mut self) {
        let start = self
            .file_tree
            .recents
            .first()
            .cloned()
            .or_else(|| {
                self.tabs
                    .current()
                    .document
                    .path
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
            })
            .filter(|dir| dir.is_dir())
            .unwrap_or_else(|| file::start_dir(None));
        if let Some(dir) = file::pick_folder_dialog(&start) {
            self.change_file_tree_root(dir);
        }
    }

    /// 挂起新建文件浮窗(`Message::FileTreeNewFileRequested` 的归约):
    /// 目标目录取消息里的路径 —— **当前文档所在目录**,这样「在 docs 里
    /// 看着文件树,新建的也在 docs」;目录不存在(文档已被外部删/移)时
    /// 回落文件树根,再没有根就只落提示行,不挂浮窗。
    ///
    /// 草稿预填一个**当下不撞名**的名字(`新建文档.md` / `新建文档-1.md`):
    /// 预填空串等于每次都要手打,预填固定串则每次都要先删一遍。
    fn request_new_file(&mut self, dir: PathBuf) {
        let Some(dir) = dir
            .is_dir()
            .then_some(dir)
            .or_else(|| self.file_tree.root.clone())
        else {
            self.tabs.current_mut().document.notice =
                Some("先在文件栏选择一个文件夹,再新建文档".to_owned());
            return;
        };
        let draft = unique_draft(&dir, NEW_FILE_BASE_NAME);
        self.file_tree.new_file = Some(crate::filetree::NewFileDraft {
            dir,
            draft,
            focus_pending: true,
        });
    }

    /// 确认新建:落盘空 Markdown → 失效该目录树缓存(否则本会话看不见)→
    /// 展开祖先并开新标签打开。失败只落提示行,浮窗保留可改名重试
    /// (目标目录被外部删掉这类错误重试无意义,但文案统一带路径,用户
    /// 自己判断)。无浮窗时 no-op。
    fn confirm_new_file(&mut self) {
        let Some(new_file) = self.file_tree.new_file.clone() else {
            return;
        };
        match crate::filetree::create_markdown(&new_file.dir, &new_file.draft) {
            Ok(path) => {
                self.file_tree.new_file = None;
                self.file_tree.invalidate(&new_file.dir);
                self.open_in_tab(&path);
            }
            Err(error) => {
                self.tabs.current_mut().document.notice = Some(error.to_string());
            }
        }
    }

    /// 挂起新建文件夹浮窗(context-menu-plan §3 M2)。目标目录口径与
    /// [`Self::request_new_file`] 相同:载荷目录不存在时回落根。
    fn request_new_folder(&mut self, dir: PathBuf) {
        let Some(dir) = dir
            .is_dir()
            .then_some(dir)
            .or_else(|| self.file_tree.root.clone())
        else {
            self.tabs.current_mut().document.notice =
                Some("先在文件栏选择一个文件夹,再新建".to_owned());
            return;
        };
        let draft = unique_draft(&dir, "新建文件夹");
        self.file_tree.new_folder = Some(crate::filetree::NewFileDraft {
            dir,
            draft,
            focus_pending: true,
        });
    }

    /// 确认新建文件夹:落盘目录 → 失效缓存 → 展开到它。失败只落提示行,
    /// 浮窗保留可改名重试(与新建文件同口径)。
    fn confirm_new_folder(&mut self) {
        let Some(draft) = self.file_tree.new_folder.clone() else {
            return;
        };
        match crate::filetree::create_folder(&draft.dir, &draft.draft) {
            Ok(path) => {
                self.file_tree.new_folder = None;
                self.file_tree.invalidate(&draft.dir);
                self.file_tree.toggle(&path);
                self.refresh_git();
            }
            Err(error) => {
                self.tabs.current_mut().document.notice = Some(error.to_string());
            }
        }
    }

    /// 挂起重命名浮窗:草稿预填现名(不含目录),改完落回同目录。
    fn request_rename(&mut self, path: PathBuf) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.file_tree.rename = Some(crate::filetree::RenameDraft {
            from: path,
            draft: name,
            focus_pending: true,
        });
    }

    /// 确认重命名(真实文件改名,context-menu-plan §3 M2):
    /// `filetree::rename_entry` 校验 + `fs::rename`,成功后——
    /// 失效父目录缓存、展开新路径;**打开中的标签换绑新路径**(缓冲与
    /// dirty 原样保留,保存目标跟移,窗口标题/状态栏自然跟随);改名
    /// 目标是目录时,其下打开中的文档按前缀整体搬移;命名草稿
    /// (`<doc>.latermd-draft`)随路径迁移。失败只落提示行,浮窗保留
    /// (磁盘不动,改名可重试)。
    fn confirm_rename(&mut self) {
        let Some(draft) = self.file_tree.rename.clone() else {
            return;
        };
        let was_dir = draft.from.is_dir();
        match crate::filetree::rename_entry(&draft.from, &draft.draft) {
            Ok(new_path) => {
                self.file_tree.rename = None;
                if let Some(parent) = draft.from.parent() {
                    self.file_tree.invalidate(parent);
                }
                self.file_tree.expand_ancestors_of(&new_path);
                // 标签换绑:精确匹配(文件)或前缀匹配(目录,连同子树)。
                // named_draft 落点跟路径走,旧草稿文件一并迁移。
                for tab in &mut self.tabs.tabs {
                    let Some(tab_path) = tab.document.path.clone() else {
                        continue;
                    };
                    let retargeted = if was_dir {
                        tab_path
                            .strip_prefix(&draft.from)
                            .ok()
                            .map(|rest| new_path.join(rest))
                    } else if tab_path == draft.from {
                        Some(new_path.clone())
                    } else {
                        None
                    };
                    if let Some(new_tab_path) = retargeted {
                        let old_draft = Self::named_draft_path(&tab_path);
                        if old_draft.exists() {
                            let _ =
                                std::fs::rename(&old_draft, Self::named_draft_path(&new_tab_path));
                        }
                        tab.document.path = Some(new_tab_path);
                    }
                }
                self.refresh_git();
            }
            Err(error) => {
                self.tabs.current_mut().document.notice = Some(error.to_string());
            }
        }
    }

    /// 挂起删除确认浮窗。
    fn request_delete(&mut self, path: PathBuf) {
        self.file_tree.delete_confirm = Some(path);
    }

    /// 确认删除(context-menu-plan §3 M2):remover 参数化 —— 生产传
    /// [`crate::filetree::trash_delete`](系统废纸篓 可恢复),无头测试
    /// 注入 spy(trash 不进测试,不污染用户废纸篓/CI 环境)。
    ///
    /// 口径(decisions-pending #144):目标(或其子树,删目录时)有
    /// **dirty 打开标签 → 拒绝**,确认浮窗保留、提示先保存或关闭 ——
    /// 删掉磁盘文件而留下脏缓冲,会让「保存」把已删文件原样写回,比
    /// 拒绝更危险;**干净打开标签 → 先关闭再删**(标签留着会指向已不
    /// 存在的路径,保存又把文件变回来)。删除目标本身不存在(外部已
    /// 删)按成功收尾:浮窗关、缓存失效,树刷新后条目自然消失。
    fn confirm_delete_with(&mut self, remover: &dyn Fn(&Path) -> Result<(), String>) {
        let Some(path) = self.file_tree.delete_confirm.clone() else {
            return;
        };
        let under_target = |tab_path: &Path| tab_path == path || tab_path.starts_with(&path);
        let dirty_open = self.tabs.tabs.iter().any(|tab| {
            tab.document.path.as_deref().is_some_and(&under_target) && tab.document.dirty
        });
        if dirty_open {
            self.tabs.current_mut().document.notice = Some(format!(
                "{} 有未保存的修改,请先保存或关闭再删除",
                path.display()
            ));
            return;
        }
        // 关闭目标(及子树)上的干净标签:从后往前移,索引不漂移。
        // AI 在途流的作废保护走 remove_tab 既有逻辑。
        for index in (0..self.tabs.tabs.len()).rev() {
            let is_target = self.tabs.tabs[index]
                .document
                .path
                .as_deref()
                .is_some_and(&under_target);
            if is_target {
                self.remove_tab(index);
            }
        }
        if let Err(error) = remover(&path) {
            // 废纸篓失败(权限/被占用):文件还在,落提示行,浮窗保留
            self.tabs.current_mut().document.notice = Some(error);
            return;
        }
        self.file_tree.delete_confirm = None;
        if let Some(parent) = path.parent() {
            self.file_tree.invalidate(parent);
        }
        self.refresh_git();
    }

    /// 生产删除:走系统废纸篓。
    fn confirm_delete(&mut self) {
        self.confirm_delete_with(&crate::filetree::trash_delete);
    }

    /// 复制路径:相对口径 = 相对文件树根(不在根下则原样绝对路径)。
    /// 文本落 `pending_copy_text`,`LaterMdApp::reduce` 每帧写剪贴板。
    fn copy_path(&mut self, path: PathBuf, relative: bool) {
        let text = if relative {
            self.file_tree
                .root
                .as_deref()
                .and_then(|root| path.strip_prefix(root).ok())
                .map(Path::to_string_lossy)
                .map(|cow| cow.into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned())
        } else {
            path.to_string_lossy().into_owned()
        };
        self.pending_copy_text = Some(text);
    }

    /// 在系统文件管理器中显示。spawn 失败只落提示行。
    fn reveal_path(&mut self, path: PathBuf) {
        if let Err(error) = crate::filetree::reveal_in_file_manager(&path) {
            self.tabs.current_mut().document.notice = Some(error);
        }
    }

    /// 重扫文件树(`Message::FileTreeRefreshRequested` 的归约):丢全部子项
    /// 缓存,下一帧归约的 `ensure_loaded` 即按「根 + 展开中」重列。
    ///
    /// Git 角标一并刷新 —— 外部 `git add` / `commit` 之后角标同样滞后,
    /// 只刷树不刷角标会让用户以为刷新没生效。
    ///
    /// 时刻取自本函数(同 `autosave_pass` 口径在 `end_of_logic` 取时):
    /// 单测不靠真等反馈窗口,只断言起点已置位,窗口边界由
    /// [`crate::filetree::FileTreeState::refreshing`] 注入时刻来验。
    fn refresh_file_tree(&mut self) {
        self.file_tree.begin_refresh(std::time::Instant::now());
        self.refresh_git();
    }

    /// 换根并持久化(对话框与最近列表两个入口共用);落盘失败只落提示行,
    /// 本次会话的文件树照常可用。搜索一并复位:旧根的结果在新根下相对
    /// 路径失真,留着只会误导。Git 状态立即按新根刷新(角标与 Git 页
    /// 不留旧仓库的快照)。
    fn change_file_tree_root(&mut self, dir: PathBuf) {
        self.file_tree.set_root(dir);
        // MCP 的检索边界跟着换根(共享句柄,服务不必重启)
        self.mcp.set_root(self.file_tree.root.clone());
        self.search.reset();
        self.refresh_git();
        if let Err(error) =
            FileTreeSettings::from(&self.file_tree).save_to(self.settings_dir.as_deref())
        {
            self.tabs.current_mut().document.notice = Some(error.to_string());
        }
    }

    /// 写盘成功后复位 dirty 并认领新路径;失败只落提示行。
    ///
    /// 路径去重(与打开侧三入口同一不变量「同一路径至多一个标签」):目标
    /// 路径已在**另一**标签打开时拒绝认领并落提示 —— 认领会让同一文件占
    /// 两个标签,此后两边各保存一次就互相静默覆盖。拒绝发生在写盘之前,
    /// 盘上内容与另一标签的缓冲都不被触碰;保存到本标签已持有的路径
    /// (常规 Ctrl+S / 同路径另存为)不受影响。
    fn save_to(&mut self, path: PathBuf) {
        if self
            .tabs
            .find_by_path(&path)
            .is_some_and(|index| index != self.tabs.active)
        {
            self.tabs.current_mut().document.notice = Some(format!(
                "{} 已在另一标签打开;请先关闭该标签或另选保存路径",
                path.display()
            ));
            return;
        }
        match file::write(&path, self.tabs.current_mut().editor.text()) {
            Ok(()) => {
                // 正常保存即清 draft(#18):记忆中的落点 + 按新路径推导的
                // 落点各删一份(另存为换过路径时两者不同)。文件不存在是
                // 常态,删失败静默 —— draft 只是冗余镜像,清不掉顶多留一份
                // 孤儿,恢复条会兜底。
                let tab = self.tabs.current_mut();
                let stale = tab.autosave.draft_path.take();
                tab.editor.clear_dirty();
                tab.document.path = Some(path.clone());
                tab.document.notice = None;
                for victim in [stale, Some(Self::named_draft_path(&path))]
                    .into_iter()
                    .flatten()
                {
                    let _ = std::fs::remove_file(victim);
                }
            }
            Err(error) => self.tabs.current_mut().document.notice = Some(error.to_string()),
        }
    }

    /// 导出 HTML(消息归约):弹保存对话框,把当前缓冲渲染成完整 HTML 落盘。
    /// 导出物是派生物:文档路径与 dirty 均不动。
    fn run_export_html(&mut self) {
        let start = file::start_dir(self.tabs.current().document.path.as_deref());
        let default = export::default_name(self.tabs.current().document.path.as_deref());
        if let Some(path) = export::save_dialog(&start, &default) {
            self.export_html_to(&path);
        }
    }

    /// 渲染并写出;失败只落提示行。绕开对话框直测落盘路径,单独成函数供测试。
    fn export_html_to(&mut self, path: &Path) {
        let html = latermd_export::export_html(self.tabs.current_mut().editor.text());
        match file::write_as("导出", path, &html) {
            Ok(()) => self.tabs.current_mut().document.notice = None,
            Err(error) => self.tabs.current_mut().document.notice = Some(error.to_string()),
        }
    }

    /// 导出 PDF(消息归约):弹保存对话框,把当前缓冲经 headless 渲染管线
    /// (`latermd_render` IR → krilla)渲染成 A4 PDF 落盘。与 HTML 同语义:
    /// 派生物,文档路径与 dirty 均不动。
    fn run_export_pdf(&mut self) {
        let start = file::start_dir(self.tabs.current().document.path.as_deref());
        let default = export::default_pdf_name(self.tabs.current().document.path.as_deref());
        if let Some(path) = export::pdf_save_dialog(&start, &default) {
            self.export_pdf_to(&path);
        }
    }

    /// 渲染并写出 PDF;字体全失配、编码失败或落盘失败都只落提示行,不
    /// panic、不产出残缺文件(落盘走原子写)。绕开对话框直测,与
    /// `export_html_to` 同构。
    fn export_pdf_to(&mut self, path: &Path) {
        // 字体发现与界面侧同一张候选表(latermd-export 单一来源):预览
        // 与导出命中同一个 CJK face。读文件是注入的,生产传 fs::read。
        let notice = match latermd_export::discover_cjk_fonts(
            latermd_export::CJK_SYSTEM_CANDIDATES,
            &|font_path| std::fs::read(font_path),
        ) {
            Err(error) => Some(error.to_string()),
            Ok(fonts) => {
                match latermd_export::export_pdf(
                    self.tabs.current().editor.text(),
                    &latermd_export::PdfExportOptions::new(fonts),
                ) {
                    Ok(bytes) => match file::write_bytes_as("导出", path, &bytes) {
                        Ok(()) => None,
                        Err(error) => Some(error.to_string()),
                    },
                    Err(error) => Some(error.to_string()),
                }
            }
        };
        self.tabs.current_mut().document.notice = notice;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::FormatAction;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("latermd-state-{}-{name}", std::process::id()))
    }

    /// 测试取命名草稿落点(named_draft_path 私有,不为此扩 API 面)。
    fn named_draft_path_for_test(path: &Path) -> PathBuf {
        let mut name = path.as_os_str().to_os_string();
        name.push(".latermd-draft");
        PathBuf::from(name)
    }

    /// 等待 AI commit 后台任务并把回传消息走真实归约。目录扫描与 Mock
    /// subject 都在工作线程，测试不能再假设 `AiCommitRequested` 同步完成。
    fn settle_ai_commit(state: &mut State) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while state.ai.is_commit_running() && std::time::Instant::now() < deadline {
            for message in state.poll_ai() {
                state.apply(message);
            }
            if state.ai.is_commit_running() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert!(
            !state.ai.is_commit_running(),
            "AI commit 后台任务应在时限内收口"
        );
    }

    /// 右键菜单编辑动作(context-menu-plan §3 M1):归约只记账到
    /// `pending_edit_action`,合成事件的推入由 `LaterMdApp::reduce` 在
    /// 持有 `egui::Context` 的层完成(layout.rs 侧有对应的出口测试)。
    #[test]
    fn edit_action_message_parks_into_pending() {
        let mut state = State::default();
        assert!(state.pending_edit_action.is_empty());
        state.apply(Message::EditAction(crate::edit_events::EditAction::Undo));
        state.apply(Message::EditAction(crate::edit_events::EditAction::Paste));
        assert_eq!(
            state.pending_edit_action,
            vec![
                crate::edit_events::EditAction::Undo,
                crate::edit_events::EditAction::Paste,
            ],
            "同帧多条动作按序排队,不互相顶掉"
        );
    }

    /// 焦点目标矩阵:源码 = 源码 TextEdit;Live = 活动块 / 空文档块;
    /// 浏览态 = None(动作作废,无可消费事件的编辑器)。
    #[test]
    fn editor_focus_id_follows_render_mode() {
        use crate::live::RenderMode;
        let mut state = State::default();
        let base = crate::ui::editor::tab_editor_id(state.tabs.current().id);

        state.render_mode = RenderMode::Source;
        assert_eq!(state.editor_focus_id(), Some(base));

        state.render_mode = RenderMode::Live;
        state.tabs.current_mut().live.browse_only = true;
        assert_eq!(state.editor_focus_id(), None, "浏览态无处还焦");

        state.tabs.current_mut().live.browse_only = false;
        assert_eq!(
            state.editor_focus_id(),
            Some(base.with("live-empty")),
            "无活动块 = 空文档块"
        );

        state.tabs.current_mut().live.active = Some(2);
        assert_eq!(
            state.editor_focus_id(),
            Some(base.with(("live-block", 2))),
            "活动块 id 与 live.rs 的 TextEdit id 同构"
        );

        // 分栏档:中央是源码 TextEdit,还焦目标同 Source(右栏只读,不参与)
        state.render_mode = RenderMode::Split;
        assert_eq!(state.editor_focus_id(), Some(base));
    }

    #[test]
    fn display_name_and_window_title_track_dirty() {
        let mut document = DocumentState {
            path: Some(PathBuf::from("/docs/LaterMD 指南.md")),
            dirty: false,
            notice: None,
        };
        assert_eq!(document.window_title(), "LaterMD — LaterMD 指南.md");
        document.dirty = true;
        assert_eq!(document.window_title(), "LaterMD — LaterMD 指南.md*");

        document = DocumentState {
            path: None,
            dirty: true,
            notice: None,
        };
        assert_eq!(document.display_name(), "Untitled*");
    }

    /// 文件树右键「重命名」(context-menu-plan §3 M2 验收 4):真实改名
    /// + 打开中标签换绑(缓冲与 dirty 保留)+ named_draft 迁移。
    #[test]
    fn tree_rename_retargets_open_tab_and_moves_draft() {
        let dir = temp_path("tree-rename");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("旧.md");
        std::fs::write(&file, "正文").unwrap();
        let draft_file = named_draft_path_for_test(&file);
        std::fs::write(&draft_file, "未保存草稿").unwrap();

        let mut state = State::default();
        state.apply(Message::FileSelected(file.clone()));
        // 弄脏缓冲,验证换绑后 dirty 保留
        state.tabs.current_mut().document.dirty = true;

        state.apply(Message::FileTreeRenameRequested(file.clone()));
        assert!(state.file_tree.rename.is_some());
        state.file_tree.rename.as_mut().unwrap().draft = "新.md".into();
        state.apply(Message::FileTreeRenameConfirmed);

        assert!(!file.exists(), "旧路径消失");
        let new_file = dir.join("新.md");
        assert!(new_file.exists());
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(new_file.as_path()),
            "打开中标签换绑新路径"
        );
        assert!(state.tabs.current().document.dirty, "缓冲 dirty 保留");
        assert!(
            named_draft_path_for_test(&new_file).exists(),
            "named_draft 随路径迁移"
        );
        assert!(!draft_file.exists(), "旧 draft 不残留");
        assert!(state.file_tree.rename.is_none(), "成功后浮窗撤下");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 改名撞名:磁盘不动、浮窗保留、提示行落地 —— 可改名重试。
    #[test]
    fn tree_rename_collision_keeps_dialog_and_disk() {
        let dir = temp_path("tree-rename-collision");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("甲.md");
        std::fs::write(&file, "").unwrap();
        std::fs::write(dir.join("乙.md"), "").unwrap();

        let mut state = State::default();
        state.apply(Message::FileSelected(file.clone()));
        state.apply(Message::FileTreeRenameRequested(file.clone()));
        state.file_tree.rename.as_mut().unwrap().draft = "乙.md".into();
        state.apply(Message::FileTreeRenameConfirmed);

        assert!(file.exists(), "磁盘不动");
        assert!(state.file_tree.rename.is_some(), "浮窗保留可重试");
        assert!(
            state.tabs.current().document.notice.is_some(),
            "撞名原因落提示行"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 删除守卫(context-menu-plan §3 M2 验收 5):dirty 打开 → 拒绝,
    /// 磁盘与标签都不动;remover 经注入,不真碰废纸篓。
    #[test]
    fn tree_delete_refused_while_dirty_open() {
        let dir = temp_path("tree-delete-dirty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("脏.md");
        std::fs::write(&file, "").unwrap();

        let mut state = State::default();
        state.apply(Message::FileSelected(file.clone()));
        state.tabs.current_mut().document.dirty = true;

        let removed = std::rc::Rc::new(std::cell::Cell::new(false));
        let removed_in = removed.clone();
        let spy = move |_path: &Path| {
            removed_in.set(true);
            Ok(())
        };
        state.apply(Message::FileTreeDeleteRequested(file.clone()));
        state.confirm_delete_with(&spy);

        assert!(file.exists(), "dirty 打开时磁盘不动");
        assert!(!removed.get(), "未调用 remover");
        assert!(
            state
                .tabs
                .tabs
                .iter()
                .filter(|tab| tab.document.path.as_deref() == Some(file.as_path()))
                .count()
                == 1,
            "标签不关"
        );
        assert!(state.file_tree.delete_confirm.is_some(), "确认浮窗保留");
        assert!(state.tabs.current().document.notice.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 干净打开的标签:先关标签再删;remover 收到目标路径;
    /// 删除失败(remover 报错)时浮窗保留。
    #[test]
    fn tree_delete_closes_clean_tab_then_removes() {
        let dir = temp_path("tree-delete-clean");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("净.md");
        std::fs::write(&file, "").unwrap();

        let mut state = State::default();
        state.apply(Message::FileSelected(file.clone()));

        let removed_path = std::rc::Rc::new(std::cell::RefCell::new(None::<PathBuf>));
        let removed_in = removed_path.clone();
        let spy = move |path: &Path| {
            *removed_in.borrow_mut() = Some(path.to_path_buf());
            Ok(())
        };
        state.apply(Message::FileTreeDeleteRequested(file.clone()));
        state.confirm_delete_with(&spy);

        assert_eq!(
            removed_path.borrow().as_deref(),
            Some(file.as_path()),
            "remover 收到目标路径"
        );
        assert_eq!(state.tabs.tabs.len(), 1, "干净标签已关(只剩兜底空标签)");
        assert!(state.file_tree.delete_confirm.is_none());
        let _ = std::fs::remove_dir_all(&dir);

        // remover 失败:文件还在、浮窗保留、提示行落地
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, "").unwrap();
        let mut state = State::default();
        state.apply(Message::FileTreeDeleteRequested(file.clone()));
        let fail = |_path: &Path| -> Result<(), String> { Err("被占用".into()) };
        state.confirm_delete_with(&fail);
        assert!(file.exists());
        assert!(state.file_tree.delete_confirm.is_some());
        assert!(state.tabs.current().document.notice.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 复制路径:根下取相对,根外回落绝对。
    #[test]
    fn tree_copy_path_relative_to_root_falls_back_absolute() {
        let mut state = State::default();
        let root = temp_path("tree-copy-root");
        let inside = root.join("子").join("文.md");
        state.file_tree.root = Some(root.clone());
        state.apply(Message::FileTreeCopyPath {
            path: inside.clone(),
            relative: true,
        });
        assert_eq!(
            state.pending_copy_text.as_deref(),
            Some("子/文.md"),
            "根下取相对路径"
        );
        state.apply(Message::FileTreeCopyPath {
            path: inside.clone(),
            relative: false,
        });
        assert_eq!(
            state.pending_copy_text.as_deref(),
            Some(inside.to_string_lossy().as_ref()),
            "绝对口径原样"
        );
        let outside = PathBuf::from("/其他/地方.md");
        state.apply(Message::FileTreeCopyPath {
            path: outside.clone(),
            relative: true,
        });
        assert_eq!(
            state.pending_copy_text.as_deref(),
            Some(outside.to_string_lossy().as_ref()),
            "根外回落绝对路径"
        );
    }

    /// 命令层开关消息:主题互换(翻转 + 落盘)与侧边栏翻转,都走完整归约。
    #[test]
    fn toggle_messages_flip_theme_and_sidebar() {
        let dir = temp_path("toggle-dir");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        let visible_before = state.layout.left;

        state.apply(Message::ToggleTheme);
        assert_eq!(state.theme.mode, ThemeMode::Dark, "默认浅色 → 深色");
        assert!(dir.join("settings.json").exists(), "互换同样持久化");
        state.apply(Message::ToggleTheme);
        assert_eq!(state.theme.mode, ThemeMode::Light, "再切回浅色");

        state.apply(Message::SidebarToggled);
        assert_eq!(state.layout.left, !visible_before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 「关闭左栏」按钮翻的只是 `layout.left` 这一个 bool(P4 起右栏对照
    /// 预览退役,live-unify-plan;右栏开关整体不存在了)。
    #[test]
    fn sidebar_toggle_flips_left_only() {
        let mut state = State::default();
        assert!(state.layout.left, "出厂侧栏全开");

        state.apply(Message::SidebarToggled);
        assert!(!state.layout.left, "收左栏");

        state.apply(Message::SidebarToggled);
        assert!(state.layout.left, "再开左栏");
    }

    /// 快速打开(#24)的开关归约:`ToggleQuickOpen` 翻转可见标志,再翻
    /// 一次回闭;打开时快照/查询词/选中项一并复位,关闭只翻标志(C3 起
    /// 快照落在 `QuickOpenState`,不再只是 C2 的裸 bool)。
    #[test]
    fn toggle_quick_open_flips_flag_only() {
        let mut state = State::default();
        assert!(!state.quick_open.open, "出厂关闭");

        state.apply(Message::ToggleQuickOpen);
        assert!(state.quick_open.open);
        assert!(state.quick_open.files.is_empty(), "无根:文件组为空");
        assert_eq!(state.quick_open.query, "");
        assert_eq!(state.quick_open.selected, 0);

        state.apply(Message::ToggleQuickOpen);
        assert!(!state.quick_open.open, "再按一次关闭");
    }

    /// 面板开合落到 `layout.json`,重启(`load_from`)后逐项一致。写盘在帧末
    /// 统一发生,故这里必须显式跑一次 `end_of_logic`。
    #[test]
    fn layout_panel_state_persists_across_reload() {
        let dir = temp_path("layout-dir");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        state.apply(Message::SidebarToggled);
        state.apply(Message::SidebarTabChanged(SidebarTab::Outline));
        state.end_of_logic();

        let restored = LayoutSettings::load_from(&dir).unwrap();
        assert!(!restored.left, "左栏收起被记住");
        assert_eq!(
            restored.left_view,
            SidebarTab::Outline,
            "停在哪个视图也记住"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **面板把手那条路径也要写盘**:拖把手 / 点收缩箭头是 egui 在 `ui` 里
    /// 原地翻转 `&mut bool`,不产任何消息 —— 写盘若挂在消息归约上,最常用
    /// 的入口会被漏掉。这里直接改 bool 模拟它。
    #[test]
    fn handle_flip_without_message_still_persists() {
        let dir = temp_path("layout-handle");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        // 不经过任何 Message,模拟 ui 侧 show_collapsible 的原地翻转
        state.layout.left = false;
        state.end_of_logic();

        assert!(
            !LayoutSettings::load_from(&dir).unwrap().left,
            "把手翻转同样落到 layout.json"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 闲置帧不写盘(`end_of_logic` 每帧都跑,无脑写就是每帧 serialize +
    /// fs::write —— 一个字没敲的空闲frame也不停砸磁盘)。以文件 mtime 佐证。
    #[test]
    fn idle_frames_do_not_rewrite_layout_json() {
        let dir = temp_path("layout-idle");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        // 首次变更催生写盘(未变过则连文件都不建 —— 全新安装不该凭空多出
        // 一个 json,那是下一次真正改动的事)
        assert!(!dir.join("layout.json").exists(), "出厂态不写盘");
        state.apply(Message::SidebarToggled);
        state.end_of_logic();

        let path = dir.join("layout.json");
        let first = std::fs::metadata(&path).unwrap().modified().unwrap();

        // 后续若干帧状态一字未变
        for _ in 0..5 {
            state.end_of_logic();
        }
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            first,
            "状态未变则不再写盘"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 打开:内容进**新标签**、dirty 复位、路径认领、预览快照同帧联动;
    /// 原标签的未保存稿不受影响(多标签 #11:「新建/打开」不再覆盖当前缓冲)。
    #[test]
    fn open_loads_buffer_and_syncs_preview() {
        let path = temp_path("open.md");
        std::fs::write(&path, "# 磁盘标题\r\nCRLF 行").unwrap();

        let mut state = State::default();
        state.tabs.current_mut().editor.insert_chars(0, "草稿"); // 制造未保存状态
        state.apply(Message::FileCommand(FileCmd::New)); // 多标签:开新标签,不拦
        assert_eq!(state.tabs.tabs.len(), 2, "新建开出新标签");
        assert_eq!(state.tabs.current_mut().editor.text(), "", "新标签为空");
        assert!(
            state.tabs.tabs[0].editor.text().starts_with("草稿"),
            "原标签草稿原样保留"
        );
        state.apply(Message::TabActivate(0));
        state.tabs.current_mut().editor.clear_dirty();
        state.open_in_tab(&path);
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            "# 磁盘标题\r\nCRLF 行",
            "CRLF 原样进缓冲"
        );
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(path.as_path())
        );
        assert!(!state.tabs.current().document.dirty);
        let (preview_text, editor_text) = {
            let tab = state.tabs.current();
            (tab.preview.text.clone(), tab.editor.text().to_owned())
        };
        assert_eq!(preview_text, editor_text, "预览快照已联动");
        let (rev, editor_rev) = {
            let tab = state.tabs.current();
            (tab.preview.synced_rev, tab.editor.revision())
        };
        assert_eq!(rev, editor_rev);
        assert_eq!(state.tabs.current().preview.outline.len(), 1);
        assert_eq!(state.tabs.current().preview.outline[0].text, "磁盘标题");
        let _ = std::fs::remove_file(&path);
    }

    /// 初始文档的大纲:示例文档的两个标题,span 索引快照文本。
    #[test]
    fn default_state_outline_matches_sample() {
        let state = State::default();
        let outline = &state.tabs.current().preview.outline;
        let levels: Vec<u8> = outline.iter().map(|item| item.level).collect();
        assert_eq!(levels, vec![1, 2]);
        assert_eq!(outline[0].text, "LaterMD");
        assert_eq!(outline[1].text, "常用元素");
        assert!(state.tabs.current().preview.text[outline[1].span.clone()].contains("## 常用元素"));
    }

    // —— 大纲的重建闸门(击键路径的性能守卫)——
    //
    // 判据取「读者看得见的东西」(大纲里有没有刚敲进去的那个标题),不取
    // 内部同步位 —— 同步位跟着实现走,实现改了判据就跟着改,守不住。

    /// 加一段新标题到文末的公共动作:推进修订号,并把当帧的快照同步跑完
    /// (与 `ui::editor` / `live::ui` 帧末那一行同款)。
    fn append_heading_and_sync(state: &mut State, heading: &str) {
        let tab = state.tabs.current_mut();
        let at = tab.editor.len_chars();
        tab.editor
            .insert_chars(at, &format!("\n\n{heading} 后加的标题\n"));
        tab.preview.sync_outline(&tab.editor);
    }

    fn outline_has(state: &State, text: &str) -> bool {
        state
            .tabs
            .current()
            .preview
            .outline
            .iter()
            .any(|item| item.text.contains(text))
    }

    /// 侧栏**正在显示大纲页**时,敲进去的新标题当帧就出现在大纲里
    /// (既有语义:大纲随编辑同帧重算)。
    #[test]
    fn outline_rescans_on_edit_while_shown() {
        let mut state = State::default();
        state.layout.left = true;
        state.layout.left_view = SidebarTab::Outline;
        state.end_of_logic();
        assert!(
            state.tabs.current().preview.outline_shown,
            "闸门由 end_of_logic 打开"
        );

        append_heading_and_sync(&mut state, "#");
        assert!(
            outline_has(&state, "后加的标题"),
            "显示中的大纲必须跟着编辑走"
        );
    }

    /// 侧栏停在**别的页**时,击键不付那次全篇标题扫描 —— 634KB 文档上
    /// `latermd_md::outline` 约 4.3ms(独立探针,线性外推),此刻还没有
    /// 消费者。
    #[test]
    fn outline_skips_rescan_while_sidebar_shows_other_tab() {
        let mut state = State::default();
        state.layout.left = true;
        state.layout.left_view = SidebarTab::Files;
        state.end_of_logic();
        assert!(
            !state.tabs.current().preview.outline_shown,
            "闸门由 end_of_logic 关上"
        );

        append_heading_and_sync(&mut state, "#");
        assert!(
            !outline_has(&state, "后加的标题"),
            "不显示时不重扫:敲一个字不必扫一篇"
        );

        // 切回大纲页:下一帧补算,读者看到的是新大纲
        state.layout.left_view = SidebarTab::Outline;
        state.end_of_logic();
        let tab = state.tabs.current_mut();
        tab.preview.sync_outline(&tab.editor);
        assert!(
            outline_has(&state, "后加的标题"),
            "切回大纲页必须补上跳过期间的改动"
        );
    }

    /// 侧栏整个收起时同样不扫(没有面板在显示大纲)。
    #[test]
    fn outline_skips_rescan_while_sidebar_collapsed() {
        let mut state = State::default();
        state.layout.left = false;
        state.layout.left_view = SidebarTab::Outline;
        state.end_of_logic();

        append_heading_and_sync(&mut state, "#");
        assert!(!outline_has(&state, "后加的标题"), "侧栏收起也不重扫");
    }

    /// 禅定档没有侧栏,同样不算。
    #[test]
    fn outline_skips_rescan_in_zen() {
        let mut state = State::default();
        state.layout.left = true;
        state.layout.left_view = SidebarTab::Outline;
        state.layout.zen = true;
        state.end_of_logic();

        append_heading_and_sync(&mut state, "#");
        assert!(!outline_has(&state, "后加的标题"), "禅定档不重扫");
    }

    // —— 任务列表 checkbox 回写(#63)——

    /// 切换语义:未勾 → 写小写 `x`;已勾(小写/大写 X)→ 写空格;嵌套
    /// 列表的深层任务按偏移精确命中自己那行;每次切换修订号恰前进。
    #[test]
    fn task_toggle_flips_marker_state() {
        let mut state = State::default();
        let source = "- [ ] 首项\n- [x] 次项\n\n  - [X] 深层\n\n中文 😀 段\n\n- [ ] 末项\n";
        state.tabs.current_mut().editor.replace_all(source);
        let marker = |needle: &str| source.find(needle).expect("needle in source");

        // 未勾 → 勾
        state.apply(Message::TaskCheckboxToggled {
            byte: marker("[ ] 首项"),
        });
        assert_eq!(
            state.tabs.current().editor.text(),
            "- [x] 首项\n- [x] 次项\n\n  - [X] 深层\n\n中文 😀 段\n\n- [ ] 末项\n"
        );
        // 已勾(小写)→ 取消
        state.apply(Message::TaskCheckboxToggled {
            byte: marker("[x] 次项"),
        });
        assert_eq!(
            state.tabs.current().editor.text(),
            "- [x] 首项\n- [ ] 次项\n\n  - [X] 深层\n\n中文 😀 段\n\n- [ ] 末项\n"
        );
        // 已勾(大写 X)→ 取消;嵌套深层任务命中自己
        state.apply(Message::TaskCheckboxToggled {
            byte: marker("[X] 深层"),
        });
        assert_eq!(
            state.tabs.current().editor.text(),
            "- [x] 首项\n- [ ] 次项\n\n  - [ ] 深层\n\n中文 😀 段\n\n- [ ] 末项\n"
        );
        // CJK/emoji 之后的任务(多字节边界后的偏移)
        state.apply(Message::TaskCheckboxToggled {
            byte: marker("[ ] 末项"),
        });
        assert_eq!(
            state.tabs.current().editor.text(),
            "- [x] 首项\n- [ ] 次项\n\n  - [ ] 深层\n\n中文 😀 段\n\n- [x] 末项\n"
        );
    }

    /// 形态核验(ByteIndex 边界防御):越界、落在多字节字符中间、非任务
    /// 三字符(`[a]`、普通文本、行内代码形态)一律静默忽略 —— 文本与
    /// 修订号零变化(过期偏移宁可不写也不写错位置)。
    #[test]
    fn task_toggle_rejects_stale_or_malformed_offsets() {
        let mut state = State::default();
        let source = "中文段 😀。\n\n行内 `[ ]` 代码与普通 [ ] 括号\n";
        state.tabs.current_mut().editor.replace_all(source);
        let rev = state.tabs.current().editor.revision();
        let mid_cjk = source.find("文").map(|at| at + 1).expect("cjk");
        for byte in [
            usize::MAX,
            source.len() + 10,
            mid_cjk,                      // 「文」的第二字节:非边界
            source.find("[ ]").unwrap(),  // 行内代码里的:三字符但……
            source.find("[").unwrap(),    // 同上(首个 [)
            source.find("普通").unwrap(), // 纯文本
        ] {
            state.apply(Message::TaskCheckboxToggled { byte });
        }
        assert_eq!(
            state.tabs.current().editor.text(),
            source,
            "全部形态核验不过,一字不改"
        );
        assert_eq!(
            state.tabs.current().editor.revision(),
            rev,
            "零改动不得推进修订号"
        );
    }

    /// 源码零扰动(#48 落盘口径):切换前后全文恰差一个字符,其余逐字节
    /// 不变;保存落盘字节与切换后的源码一致,改写零外泄。
    #[test]
    fn task_toggle_touches_exactly_one_byte() {
        let mut state = State::default();
        let source = "# 标题\n\n- [ ] 任务 😀\n";
        state.tabs.current_mut().editor.replace_all(source);
        let byte = source.find("[ ]").expect("task");
        state.apply(Message::TaskCheckboxToggled { byte });
        let toggled = state.tabs.current().editor.text().to_owned();
        assert_eq!(toggled, "# 标题\n\n- [x] 任务 😀\n");
        // 逐字节 diff:恰一处不同且都是 ASCII 单字符
        let diff = source
            .bytes()
            .zip(toggled.bytes())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(diff, 1, "恰一个字节不同");
        assert_eq!(source.len(), toggled.len());

        let path = temp_path("task-toggle.md");
        state.save_to(path.clone());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            toggled.as_bytes(),
            "盘上字节 == 切换后的源码"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// 大纲点击归约:跳过 span 吸收的前置换行落到标题行首,并按当前缓冲
    /// 把字节偏移换成字符偏移(示例文档在目标前有 CJK,两者必然不同)。
    #[test]
    fn outline_click_converts_to_char_offset_on_heading_line() {
        let mut state = State::default();
        let span = state.tabs.current().preview.outline[1].span.clone();
        state.apply(Message::OutlineItemClicked(span));

        let heading_byte = state
            .tabs
            .current_mut()
            .editor
            .text()
            .find("## 常用元素")
            .unwrap();
        let jump = state
            .tabs
            .current_mut()
            .cursor
            .jump_to
            .expect("已设置跳转目标");
        assert_eq!(
            jump,
            state.tabs.current_mut().editor.byte_to_char(heading_byte)
        );
        assert!(jump < heading_byte, "目标前有 CJK,字符偏移必须小于字节偏移");
        let bytes = state.tabs.current().editor.text().as_bytes();
        assert_ne!(
            bytes[state.tabs.current().editor.char_to_byte(jump)],
            b'\n',
            "落在标题行首"
        );
    }

    /// 大纲点击归约的过期 span:消息产自上一帧快照,同帧编辑可能已把缓冲
    /// 删短,越界 start 不得 panic,钳制后跳到当前文档末尾。
    #[test]
    fn outline_click_with_stale_span_clamps_to_text_end() {
        let mut state = State::default();
        let stale = state.tabs.current().preview.outline[1].span.clone();
        state.tabs.current_mut().editor.replace_all("短");
        assert!(
            stale.start > state.tabs.current_mut().editor.text().len(),
            "前置:span 确已越界"
        );

        state.apply(Message::OutlineItemClicked(stale));
        {
            let tab = state.tabs.current();
            assert_eq!(
                tab.cursor.jump_to,
                Some(tab.editor.len_chars()),
                "钳制到末尾(byte_to_char 再把字节偏移换成字符偏移)"
            );
        }
    }

    /// 保存:字节原样落盘、dirty 复位;写入失败保留 dirty 并给出带路径的提示。
    #[test]
    fn save_writes_bytes_and_resets_dirty() {
        let path = temp_path("save.md");
        let mut state = State::default();
        state.tabs.current_mut().editor.insert_chars(0, "改动\r\n");
        assert!(state.tabs.current_mut().editor.is_dirty());

        state.save_to(path.clone());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            state.tabs.current().editor.text().as_bytes()
        );
        assert!(!state.tabs.current_mut().editor.is_dirty());
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(path.as_path())
        );
        let _ = std::fs::remove_file(&path);

        // 目录不存在 → 失败路径:dirty 保留,提示含路径
        state.tabs.current_mut().editor.insert_chars(0, "再改");
        state.save_to(PathBuf::from("/latermd/no/such/dir.md"));
        assert!(state.tabs.current_mut().editor.is_dirty());
        let notice = state.tabs.current().document.notice.as_deref().unwrap();
        assert!(notice.contains("dir.md"), "{notice}");
    }

    /// 帧末刷新把缓冲的 dirty 镜像到文档状态(窗口标题的唯一数据来源)。
    #[test]
    fn end_of_logic_mirrors_editor_dirty() {
        let mut state = State::default();
        state.end_of_logic();
        assert!(!state.tabs.current().document.dirty);
        state.tabs.current_mut().editor.insert_chars(0, "x");
        assert!(!state.tabs.current().document.dirty, "编辑动作本身不动镜像");
        state.end_of_logic();
        assert!(state.tabs.current().document.dirty);
    }

    // ---- 自动保存(#18)----------------------------------------------------
    // 时刻全部经 `autosave_pass(now)` 注入,不真等 30s;沙盒目录自清理。

    /// 停顿触发:编辑后静置 ≥30s 落 draft(内容 = 缓冲全量),不满阈值
    /// 一帧都不多写。
    #[test]
    fn autosave_writes_draft_after_idle_pause() {
        let dir = temp_path("autosave-idle");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("doc.md");
        let draft = dir.join("doc.md.latermd-draft");
        let mut state = State::default();
        state.tabs.current_mut().document.path = Some(doc);
        state
            .tabs
            .current_mut()
            .editor
            .insert_chars(0, "手敲的未保存内容");

        let t0 = std::time::Instant::now();
        state.autosave_pass(t0); // 帧末记账 last_edit;30s 未满
        assert!(!draft.exists(), "编辑当帧不写");
        state.autosave_pass(t0 + Duration::from_secs(29));
        assert!(!draft.exists(), "29s 仍不满阈值");
        state.autosave_pass(t0 + Duration::from_secs(30));
        assert_eq!(
            std::fs::read_to_string(&draft).unwrap(),
            state.tabs.current().editor.text(),
            "draft 内容 = 缓冲全量"
        );
        assert!(state.tabs.current().document.notice.is_none(), "成功无提示");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 切标签触发:被切出的脏标签即刻落 draft,不等停顿;开新标签
    /// (`FileCmd::New` / 打开文件的 spawn 路径)同样把原标签切出,同落。
    #[test]
    fn autosave_writes_draft_on_tab_switch_out() {
        let dir = temp_path("autosave-switch");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let draft = dir.join("doc.md.latermd-draft");
        let mut state = State::default();
        state.tabs.current_mut().document.path = Some(dir.join("doc.md"));
        state
            .tabs
            .current_mut()
            .editor
            .insert_chars(0, "切走前的改动");
        state.tabs.open_tab(None, ""); // 直接开第二个(不经消息,见后半段)
        state.apply(Message::TabActivate(0)); // 切回 0:切出的新标签不脏,无事
        let t0 = std::time::Instant::now();
        state.autosave_pass(t0);
        assert!(!draft.exists(), "距上次改动 0s,停顿路径未到点");

        state.apply(Message::TabActivate(1)); // 切出脏的 0 号标签
        state.autosave_pass(t0 + Duration::from_millis(10)); // 仍远不满 30s
        assert_eq!(
            std::fs::read_to_string(&draft).unwrap(),
            state.tabs.tabs[0].editor.text(),
            "切出即落,不等停顿"
        );

        // 开新标签的 spawn 路径:先在当前标签(1 号)弄出脏改动再开新标签,
        // 验证 spawn 同样触发「切出即落」
        let second = dir.join("second.md.latermd-draft");
        state.tabs.current_mut().document.path = Some(dir.join("second.md"));
        state.tabs.current_mut().editor.insert_chars(0, "第二条");
        state.apply(Message::FileCommand(FileCmd::New)); // 开新标签 = 切出
        state.autosave_pass(t0 + Duration::from_millis(20));
        assert!(second.exists(), "spawn 开新标签同样触发切出落盘");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 正常保存即清:save_to 成功后 draft 消失(记忆落点 + 新路径推导
    /// 两处都清)。
    #[test]
    fn autosave_draft_cleared_after_save() {
        let dir = temp_path("autosave-save");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("doc.md");
        let draft = dir.join("doc.md.latermd-draft");
        let mut state = State::default();
        state.tabs.current_mut().document.path = Some(doc.clone());
        state
            .tabs
            .current_mut()
            .editor
            .insert_chars(0, "落过 draft 的改动");
        let t0 = std::time::Instant::now();
        state.autosave_pass(t0);
        state.autosave_pass(t0 + Duration::from_secs(31));
        assert!(draft.exists());

        state.save_to(doc);
        assert!(!draft.exists(), "保存成功即清 draft");
        assert!(!state.tabs.current_mut().editor.is_dirty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 关闭即清:脏标签经确认模态关闭后 draft 消失(丢弃不留镜像)。
    #[test]
    fn autosave_draft_cleared_after_tab_close() {
        let dir = temp_path("autosave-close");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let draft = dir.join("doc.md.latermd-draft");
        let mut state = State::default();
        state.tabs.current_mut().document.path = Some(dir.join("doc.md"));
        state
            .tabs
            .current_mut()
            .editor
            .insert_chars(0, "将被丢弃的改动");
        let t0 = std::time::Instant::now();
        state.autosave_pass(t0);
        state.autosave_pass(t0 + Duration::from_secs(31));
        assert!(draft.exists());

        state.apply(Message::TabCloseActive); // 脏 → 弹确认
        assert!(state.tabs.confirm_close.is_some());
        state.apply(Message::TabCloseConfirmed);
        assert!(!draft.exists(), "确认关闭即清 draft");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 同一修订号不重写:落过之后外部改动 draft 文件,后续帧末(哪怕
    /// 早已超过停顿阈值)不覆盖它;新编辑推进修订号才重新落盘。
    #[test]
    fn autosave_skips_rewrite_for_same_revision() {
        let dir = temp_path("autosave-rev");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let draft = dir.join("doc.md.latermd-draft");
        let mut state = State::default();
        state.tabs.current_mut().document.path = Some(dir.join("doc.md"));
        state.tabs.current_mut().editor.insert_chars(0, "第一版");
        let t0 = std::time::Instant::now();
        state.autosave_pass(t0);
        state.autosave_pass(t0 + Duration::from_secs(31));
        assert_eq!(
            std::fs::read_to_string(&draft).unwrap(),
            state.tabs.current().editor.text()
        );

        std::fs::write(&draft, "外部改动,重写会覆盖我").unwrap();
        state.autosave_pass(t0 + Duration::from_secs(120));
        assert_eq!(
            std::fs::read_to_string(&draft).unwrap(),
            "外部改动,重写会覆盖我",
            "同修订号跳过重写"
        );

        // 新编辑推进修订号:恢复重写(证明上一段不是恒真)
        state.tabs.current_mut().editor.insert_chars(0, "第二版:");
        state.autosave_pass(t0 + Duration::from_secs(121));
        state.autosave_pass(t0 + Duration::from_secs(152));
        assert_eq!(
            std::fs::read_to_string(&draft).unwrap(),
            state.tabs.current().editor.text()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 未命名文档的 draft 落状态目录 `drafts/`(配置目录注入沙盒),
    /// 文件名带标签 id,多个未命名标签互不覆盖。
    #[test]
    fn autosave_untitled_draft_goes_to_config_drafts_dir() {
        let dir = temp_path("autosave-untitled");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        let tab_id = state.tabs.current().id;
        state.tabs.current_mut().editor.insert_chars(0, "未命名稿");
        let t0 = std::time::Instant::now();
        state.autosave_pass(t0);
        assert!(!dir.join("drafts").exists(), "不满阈值不落文件");
        state.autosave_pass(t0 + Duration::from_secs(31));
        let draft = dir
            .join("drafts")
            .join(format!("untitled-{tab_id}.latermd-draft"));
        assert_eq!(
            std::fs::read_to_string(&draft).unwrap(),
            state.tabs.current().editor.text(),
            "未命名 draft 落 config_dir()/drafts/,按标签 id 命名"
        );

        // 未命名标签关闭 → 状态目录里的 draft 同步删
        state.apply(Message::TabCloseActive);
        state.apply(Message::TabCloseConfirmed);
        assert!(!draft.exists(), "未命名标签关闭即清状态目录 draft");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 写入失败不 panic、旧 draft 保留:落点被同名目录占据时原子写失败,
    /// 只落提示行;此前在别处落成的 draft 分毫未动。
    #[test]
    fn autosave_write_failure_keeps_old_draft_and_notifies() {
        let dir = temp_path("autosave-fail");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("dir1")).unwrap();
        std::fs::create_dir_all(dir.join("dir2")).unwrap();
        let first_draft = dir.join("dir1").join("doc.md.latermd-draft");
        let mut state = State::default();
        state.tabs.current_mut().document.path = Some(dir.join("dir1").join("doc.md"));
        state
            .tabs
            .current_mut()
            .editor
            .insert_chars(0, "第一处成功的内容");
        let first_text = state.tabs.current().editor.text().to_owned();
        let t0 = std::time::Instant::now();
        state.autosave_pass(t0);
        state.autosave_pass(t0 + Duration::from_secs(31));
        assert!(first_draft.exists());

        // 换路径:新 draft 落点被目录占据 → 原子写失败(rename 顶不动目录)
        std::fs::create_dir_all(dir.join("dir2").join("doc.md.latermd-draft")).unwrap();
        state.tabs.current_mut().document.path = Some(dir.join("dir2").join("doc.md"));
        state.tabs.current_mut().editor.insert_chars(0, "更多");
        state.autosave_pass(t0 + Duration::from_secs(40)); // 记账新 last_edit
        state.autosave_pass(t0 + Duration::from_secs(71)); // 到点,写失败
        let notice = state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .unwrap_or_default();
        assert!(notice.contains("自动保存失败"), "失败走提示行: {notice}");
        assert_eq!(
            std::fs::read_to_string(&first_draft).unwrap(),
            first_text,
            "旧 draft 保留,失败不破坏既有文件"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 下次停顿落盘到点(`next_autosave_due`,#18 帧饥饿修复的重绘驱动
    /// 数据源):干净缓冲、尚未跑过帧末归约(无编辑时刻)、落盘后修订号
    /// 追平——三种状态都无事可等;唯独「脏且未落且已记时刻」给出
    /// `last_edit + AUTOSAVE_IDLE`。写失败(saved_rev 不追平)时到点已
    /// 过期,返回值仍是该过期时刻,由调用方决定钳制重试节奏。
    #[test]
    fn autosave_next_due_reports_pending_deadline_only() {
        let dir = temp_path("autosave-due");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut state = State::default();
        assert_eq!(state.next_autosave_due(), None, "干净缓冲无事可等");
        state.tabs.current_mut().document.path = Some(dir.join("doc.md"));
        state.tabs.current_mut().editor.insert_chars(0, "待落");
        assert_eq!(
            state.next_autosave_due(),
            None,
            "尚无编辑时刻(停顿判定本就不成立,与 autosave_pass 同口径)"
        );
        let t0 = std::time::Instant::now();
        state.autosave_pass(t0); // 帧末记账 last_edit
        assert_eq!(
            state.next_autosave_due(),
            Some(t0 + AUTOSAVE_IDLE),
            "待落 + 已记时刻 → 上次编辑加停顿阈值"
        );
        state.autosave_pass(t0 + Duration::from_secs(30)); // 到点落盘
        assert_eq!(
            state.next_autosave_due(),
            None,
            "saved_rev 追平后不再有待落,重绘驱动自然收敛到深度空闲"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- 孤儿 draft 恢复条(#18 恢复/丢弃)-------------------------------
    // 检测挂在 spawn_tab(新标签认领文档路径的唯一汇点),这里全部经真实的
    // 打开消息(`FileSelected`)走完整链路;沙盒目录自清理。

    /// 检测:打开文档时文档旁有遗留 draft → 新标签置待恢复状态(落点 =
    /// 文档全名追加 `.latermd-draft`、mtime 可得);无 draft 的文档与未命名
    /// 新标签不置;已开标签的激活分支(open_path 的路径去重)不重新检测。
    #[test]
    fn recovery_detects_orphan_draft_when_tab_claims_path() {
        let dir = temp_path("recover-detect");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("笔记.md");
        std::fs::write(&doc, "盘上版本").unwrap();
        let draft = dir.join("笔记.md.latermd-draft");
        std::fs::write(&draft, "崩溃前的稿子").unwrap();
        let clean = dir.join("干净.md");
        std::fs::write(&clean, "无草稿").unwrap();

        let mut state = State::default();
        state.apply(Message::FileSelected(doc.clone()));
        let recover = state
            .tabs
            .current()
            .recover
            .as_ref()
            .expect("文档旁有 draft → 置待恢复");
        assert_eq!(recover.path, draft, "落点 = 文档全名追加后缀,不改扩展名");
        assert_eq!(
            recover.mtime,
            std::fs::metadata(&draft).unwrap().modified().ok(),
            "记录检测时刻的 mtime(取不到才允许 None)"
        );
        assert!(
            !state.tabs.current().editor.is_dirty(),
            "刚打开是干净缓冲,不因待恢复变脏"
        );

        state.apply(Message::FileSelected(clean));
        assert!(
            state.tabs.current().recover.is_none(),
            "无 draft 的文档不置待恢复"
        );
        state.apply(Message::FileCommand(FileCmd::New));
        assert!(
            state.tabs.current().recover.is_none(),
            "未命名新标签无路径可锚,不检测"
        );

        // 已开标签的激活分支不重新检测:把 1 号标签(笔记.md)的待恢复状态
        // 清零后再打开同一路径,只激活、不复活恢复条
        state.tabs.tabs[1].recover = None;
        state.apply(Message::FileSelected(doc));
        assert_eq!(state.tabs.active, 1, "路径去重:只激活已开标签");
        assert!(
            state.tabs.current().recover.is_none(),
            "激活已开标签不重新检测"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 恢复:draft 内容读进缓冲、dirty 置位(整篇替换路径,修订号前进供
    /// 预览重建)、draft 删除、待恢复状态清空;过期的标签 id 是 no-op。
    #[test]
    fn recovery_restores_content_marks_dirty_and_deletes_draft() {
        let dir = temp_path("recover-restore");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("doc.md");
        std::fs::write(&doc, "盘上版本").unwrap();
        let draft = dir.join("doc.md.latermd-draft");
        std::fs::write(&draft, "崩溃前的稿子\n有第二行").unwrap();
        let mut state = State::default();
        state.apply(Message::FileSelected(doc));
        let rev_before = state.tabs.current().editor.revision();
        let tab_id = state.tabs.current().id;

        state.apply(Message::DraftRecovered { tab_id: 9999 });
        assert!(
            state.tabs.current().recover.is_some(),
            "过期标签 id 是 no-op,不动真状态"
        );

        state.apply(Message::DraftRecovered { tab_id });
        let tab = state.tabs.current();
        assert_eq!(
            tab.editor.text(),
            "崩溃前的稿子\n有第二行",
            "draft 并入缓冲"
        );
        assert!(tab.editor.is_dirty(), "恢复后 dirty 置位");
        assert!(
            tab.editor.revision() > rev_before,
            "修订号前进,预览快照的重建时机"
        );
        assert!(tab.recover.is_none(), "待恢复状态清空");
        assert!(!draft.exists(), "恢复完成即删 draft");
        let notice = tab.document.notice.as_deref().unwrap_or_default();
        assert!(
            notice.contains("已恢复未保存草稿") && notice.contains("Ctrl+Z"),
            "成功留痕并告知可撤销: {notice}"
        );
        // 重复消息不再生效(状态已清):缓冲与盘面都不再变化
        state.apply(Message::DraftRecovered { tab_id });
        assert_eq!(state.tabs.current().editor.text(), "崩溃前的稿子\n有第二行");
        assert!(!draft.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 丢弃:draft 删除、待恢复状态清空;缓冲保持盘上版本、不置 dirty;
    /// 提示行留痕(可追溯)。
    #[test]
    fn recovery_discard_deletes_draft_and_keeps_buffer() {
        let dir = temp_path("recover-discard");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("doc.md");
        std::fs::write(&doc, "盘上版本").unwrap();
        let draft = dir.join("doc.md.latermd-draft");
        std::fs::write(&draft, "将被丢弃的稿子").unwrap();
        let mut state = State::default();
        state.apply(Message::FileSelected(doc));
        let tab_id = state.tabs.current().id;

        state.apply(Message::DraftDiscarded { tab_id });
        let tab = state.tabs.current();
        assert!(!draft.exists(), "丢弃即删 draft");
        assert!(tab.recover.is_none(), "待恢复状态清空");
        assert_eq!(tab.editor.text(), "盘上版本", "缓冲不动");
        assert!(!tab.editor.is_dirty(), "丢弃不产生未保存态");
        assert!(
            tab.document
                .notice
                .as_deref()
                .unwrap_or_default()
                .contains("已丢弃未保存草稿"),
            "提示行留痕供追溯"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// draft 读取失败(坏内容)不 panic 且有提示:非 UTF-8 的 draft 恢复
    /// 不崩溃,提示行带路径与原因;draft 文件保留现场、缓冲保持盘上版本、
    /// 待恢复状态撤下(永远兑现不了的恢复条不再反复戳用户)。
    #[test]
    fn recovery_with_corrupt_draft_notifies_without_panicking() {
        let dir = temp_path("recover-corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("doc.md");
        std::fs::write(&doc, "盘上版本").unwrap();
        let draft = dir.join("doc.md.latermd-draft");
        std::fs::write(&draft, [0xC4, 0xE3, 0xBA, 0xC3]).unwrap(); // GBK「你好」
        let mut state = State::default();
        state.apply(Message::FileSelected(doc));
        let tab_id = state.tabs.current().id;

        state.apply(Message::DraftRecovered { tab_id });
        let tab = state.tabs.current();
        let notice = tab.document.notice.as_deref().unwrap_or_default();
        assert!(notice.contains("草稿恢复失败"), "失败走提示行: {notice}");
        assert!(
            notice.contains("doc.md.latermd-draft"),
            "提示带路径: {notice}"
        );
        assert_eq!(tab.editor.text(), "盘上版本", "坏内容不进缓冲");
        assert!(!tab.editor.is_dirty());
        assert!(draft.exists(), "读取失败的 draft 保留现场");
        assert!(tab.recover.is_none(), "恢复条撤下");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 用户无视恢复条直接编辑:恢复条撤下(隐性选择以盘上版本续写),
    /// draft 文件保留 —— 停顿/切出路径照常接管,30s 后新缓冲照常覆盖镜像。
    #[test]
    fn recovery_bar_dismissed_by_direct_edit_but_draft_kept() {
        let dir = temp_path("recover-edit");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("doc.md");
        std::fs::write(&doc, "盘上版本").unwrap();
        let draft = dir.join("doc.md.latermd-draft");
        std::fs::write(&draft, "旧稿").unwrap();
        let mut state = State::default();
        state.apply(Message::FileSelected(doc));
        assert!(state.tabs.current().recover.is_some());

        state.tabs.current_mut().editor.insert_chars(0, "我直接改:");
        let t0 = std::time::Instant::now();
        state.autosave_pass(t0); // 帧末记账 + 撤条判定
        assert!(state.tabs.current().recover.is_none(), "直接编辑即撤恢复条");
        assert!(draft.exists(), "draft 不随撤条删除");
        assert_eq!(
            std::fs::read_to_string(&draft).unwrap(),
            "旧稿",
            "不满停顿阈值,镜像尚未被覆盖"
        );

        state.autosave_pass(t0 + Duration::from_secs(31));
        assert_eq!(
            std::fs::read_to_string(&draft).unwrap(),
            "我直接改:盘上版本",
            "停顿路径照常另写,旧稿被新缓冲顶替"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 恢复作用于**载荷指定的标签**而非当前活动标签:消息是绘制帧的
    /// 下一帧才归约的,期间用户可能已切走 —— 按 id 定位才不会把稿子灌进
    /// 别的文档。
    #[test]
    fn recovery_targets_payload_tab_not_active() {
        let dir = temp_path("recover-target");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("doc.md");
        std::fs::write(&doc, "盘上版本").unwrap();
        let draft = dir.join("doc.md.latermd-draft");
        std::fs::write(&draft, "稿子").unwrap();
        let mut state = State::default();
        state.apply(Message::FileSelected(doc));
        let tab_id = state.tabs.current().id;
        // 模拟:点击「恢复」的下一帧前切到了别的标签
        state.apply(Message::FileCommand(FileCmd::New));
        assert_eq!(
            state.tabs.active, 2,
            "0 号是出厂示例,1 号是刚开的 doc,新标签 2 号"
        );

        state.apply(Message::DraftRecovered { tab_id });
        assert_eq!(state.tabs.tabs[1].editor.text(), "稿子", "稿子灌回载荷标签");
        assert_eq!(
            state.tabs.current().editor.text(),
            "",
            "当前标签(新建空文档)不被殃及"
        );
        assert!(!draft.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 主题切换归约:状态翻转 + settings.json 落盘(注入临时目录,不碰
    /// 真实平台配置);成功路径无提示。
    #[test]
    fn theme_change_updates_state_and_persists() {
        let dir = temp_path("theme-dir");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };

        state.apply(Message::ThemeChanged(ThemeMode::Light));
        assert_eq!(state.theme.mode, ThemeMode::Light);
        assert!(state.tabs.current().document.notice.is_none());
        // "light" 必须在盘上,重启 load 才能还原
        let json = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(json.contains("\"light\""), "{json}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// MCP 端到端:设置页「保存」→ 落 `mcp.json` → 后台线程真的监听 → 用
    /// 真实 TCP 连接发一次 `initialize` 并收到应答。
    ///
    /// 这是 app 侧唯一会**真占端口**的测试,端口取 18731(远离默认 8731,
    /// 避免与本机已运行的应用实例打架);结束前显式 `stop`,不留监听。
    #[test]
    fn mcp_config_saved_starts_server_and_answers_over_http() {
        let dir = temp_path("mcp-http");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        state.apply(Message::McpConfigSaved(McpConfig {
            enabled: true,
            http_port: 18731,
            ..McpConfig::default()
        }));
        assert!(dir.join("mcp.json").exists(), "配置落盘");
        assert!(state.tabs.current().document.notice.is_none());

        // 绑定在后台线程发生,轮询等结果(正常毫秒级)
        for _ in 0..100 {
            state.mcp.poll();
            if matches!(state.mcp.status, crate::mcp::McpStatus::Listening(_)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            state.mcp.status,
            crate::mcp::McpStatus::Listening(18731),
            "状态行应显示真实监听端口"
        );

        let mut stream = std::net::TcpStream::connect(("127.0.0.1", 18731)).unwrap();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        std::io::Write::write_all(&mut stream, request.as_bytes()).unwrap();
        let mut response = String::new();
        std::io::Read::read_to_string(&mut stream, &mut response).unwrap();
        assert!(response.contains("2025-06-18"), "{response}");

        state.mcp.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// MCP 的检索边界跟着文件树根走(共享句柄,服务不重启):换根后
    /// `mcp.root()` 即为新根。
    #[test]
    fn mcp_root_follows_file_tree_root() {
        let dir = temp_path("mcp-root");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        assert_eq!(state.mcp.root(), None);
        state.apply(Message::FileTreeRootSelected(dir.clone()));
        assert_eq!(state.mcp.root(), Some(dir.clone()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 换皮肤:目录里的 `.ron` 内容进内存并落 `settings.json`(皮肤文件是
    /// 唯一事实源,配置只存名字)。
    #[test]
    fn theme_skin_selected_loads_style_and_persists() {
        let dir = temp_path("skin-select");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        let style = egui_markdown_style::MarkdownStyle {
            block_spacing: 21.0,
            ..egui_markdown_style::MarkdownStyle::default()
        };
        crate::theme::export_skin(&dir, "暗夜", &style, crate::theme::ShellPalette::default())
            .unwrap();
        state.skins = SkinCatalog::load_from(&dir);
        assert_eq!(state.skins.skins.len(), 1);

        state.apply(Message::ThemeSkinSelected(Some("暗夜".to_owned())));
        assert_eq!(state.theme.markdown_style().block_spacing, 21.0);
        assert_eq!(state.theme.skin.as_deref(), Some("暗夜"));
        let json = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(json.contains("暗夜"), "{json}");

        // 选回出厂默认:内存样式与配置里的名字一起清空
        state.apply(Message::ThemeSkinSelected(None));
        assert_eq!(state.theme.skin, None);
        assert_eq!(
            state.theme.markdown_style(),
            crate::theme::default_markdown_style()
        );

        // 选一个目录里没有的皮肤:回落默认而不是留在「选了不存在的」
        state.apply(Message::ThemeSkinSelected(Some("不存在".to_owned())));
        assert_eq!(state.theme.skin, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 导出皮肤:写文件 → 重扫目录 → 自动选中(所见即所得)。
    #[test]
    fn theme_skin_export_writes_file_and_selects_it() {
        let dir = temp_path("skin-export");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        state.apply(Message::ThemeSkinExported {
            name: "我的".to_owned(),
        });
        assert_eq!(state.theme.skin.as_deref(), Some("我的"));
        assert!(dir.join("themes").join("我的.ron").exists());
        assert!(state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("已导出皮肤")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 密度切换落盘;重启(`load_preferences`)后仍是所选档。
    #[test]
    fn theme_density_persists_across_reload() {
        let dir = temp_path("density");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        // #34:默认档 = Compact(2026-09-29 改名换档后的新「标准」)
        assert_eq!(Density::default(), Density::Compact);
        state.apply(Message::ThemeDensityChanged(Density::Compact));
        assert_eq!(state.theme.density, Density::Compact);

        // 重启路径:主题由 `main` 的 `ThemeSettings::load` 装载(不经过
        // `load_preferences`),这里按同源的 load_from 复现
        let reloaded_theme = ThemeSettings::load_from(&dir).unwrap();
        assert_eq!(reloaded_theme.density, Density::Compact);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #23 F2:字号/行距消息归约落 `theme` 对应字段并即时写 settings.json
    /// (与切密度同款通路);重启(load_from)后值仍在。只动各自字段 ——
    /// 密度与其余主题偏好不受影响(与密度选择并存、互不影响)。
    #[test]
    fn editor_font_prefs_messages_update_fields_and_persist() {
        let dir = temp_path("font-prefs-persist");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        assert_eq!(state.theme.editor_font_size, 15.0);
        assert_eq!(state.theme.line_height, 1.5);
        let density_before = state.theme.density;

        state.apply(Message::EditorFontSizeChanged(18.0));
        assert_eq!(state.theme.editor_font_size, 18.0);
        state.apply(Message::EditorLineHeightChanged(1.8));
        assert_eq!(state.theme.line_height, 1.8);
        // 互不影响:改字号不动行距,反之亦然;密度档全程不变
        assert_eq!(state.theme.editor_font_size, 18.0);
        assert_eq!(state.theme.density, density_before);

        // 重启路径:主题由 `main` 的 `ThemeSettings::load` 装载(不经过
        // `load_preferences`),这里按同源的 load_from 复现(与切密度测试同款)
        let reloaded_theme = ThemeSettings::load_from(&dir).unwrap();
        assert_eq!(reloaded_theme.editor_font_size, 18.0);
        assert_eq!(reloaded_theme.line_height, 1.8);
        assert_eq!(reloaded_theme.density, density_before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #23 F2:消息可被直塞越界值(滑杆 range 只是 UI 侧约束,归约层是
    /// 独立防线)—— 字号钳进 12..=24、行距钳进 1.2..=2.0;`inf` 钳到端点、
    /// `NaN` 回落默认(防线在 `theme::clamp_*` 纯函数:NaN 对 f32 clamp 是
    /// 穿透的)。界内值(含非整数字号,UI 的 integer 步进只是滑杆层约束)
    /// 原样通过 —— 钳制管边界不管粒度。
    #[test]
    fn editor_font_prefs_out_of_range_messages_are_clamped() {
        let dir = temp_path("font-prefs-clamp");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        for (size, want) in [
            (99.0_f32, 24.0),
            (f32::INFINITY, 24.0),
            (0.0, 12.0),
            (-5.0, 12.0),
            (f32::NEG_INFINITY, 12.0),
            (f32::NAN, 15.0),
            // 端点本身合法,clamp 不动;界内非整数原样通过(不偷偷 round)
            (12.0, 12.0),
            (24.0, 24.0),
            (18.5, 18.5),
        ] {
            state.apply(Message::EditorFontSizeChanged(size));
            assert_eq!(state.theme.editor_font_size, want, "{size} -> {want}");
        }
        for (ratio, want) in [
            (99.0_f32, 2.0),
            (f32::INFINITY, 2.0),
            (0.0, 1.2),
            (f32::NEG_INFINITY, 1.2),
            (f32::NAN, 1.5),
            (1.2, 1.2),
            (2.0, 2.0),
            (1.75, 1.75),
        ] {
            state.apply(Message::EditorLineHeightChanged(ratio));
            assert_eq!(state.theme.line_height, want, "{ratio} -> {want}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #55 M2:minimap 开关消息归约落 `theme.show_minimap` 并即时写
    /// settings.json;重启(load_from)后值仍在。只动该字段,排版偏好与
    /// 密度不受影响(与 #23 排版偏好同款通路)。
    #[test]
    fn show_minimap_toggle_updates_field_and_persists() {
        let dir = temp_path("show-minimap-toggle");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        assert!(state.theme.show_minimap, "出厂默认开");
        let font_before = state.theme.editor_font_size;

        state.apply(Message::ShowMinimapToggled(false));
        assert!(!state.theme.show_minimap);
        state.apply(Message::ShowMinimapToggled(true));
        assert!(state.theme.show_minimap, "往返翻回开");
        state.apply(Message::ShowMinimapToggled(false));

        let reloaded = ThemeSettings::load_from(&dir).unwrap();
        assert!(!reloaded.show_minimap, "重启后仍为关");
        assert_eq!(reloaded.editor_font_size, font_before, "排版偏好不受影响");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #67 M2:minimap 的**无参翻转**消息(视图菜单条目/快捷键路径,
    /// `Command::ToggleMinimap.message()`)归约翻转 `theme.show_minimap`
    /// 并即时写 settings.json;与带值消息同字段同通路 —— 菜单、快捷键、
    /// 设置外观页复选框三方一致(打字机 `ToggleTypewriter` 的同款断面)。
    #[test]
    fn minimap_menu_toggle_flips_field_and_persists() {
        let dir = temp_path("minimap-menu-toggle");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        assert!(state.theme.show_minimap, "出厂默认开");

        // 菜单/快捷键路径的消息 == Command::ToggleMinimap 的映射
        assert_eq!(
            crate::command::Command::ToggleMinimap.message(),
            Message::ToggleMinimap
        );
        state.apply(Message::ToggleMinimap);
        assert!(!state.theme.show_minimap, "开 → 翻转 → 关");
        state.apply(Message::ToggleMinimap);
        assert!(state.theme.show_minimap, "关 → 翻转 → 开");
        state.apply(Message::ToggleMinimap);

        let reloaded = ThemeSettings::load_from(&dir).unwrap();
        assert!(!reloaded.show_minimap, "菜单开关即时落盘,重启后仍为关");
        state.apply(Message::MinimapAutoChanged(false));
        let manual = ThemeSettings::load_from(&dir).unwrap();
        assert!(!manual.minimap_auto && !manual.show_minimap);
        state.apply(Message::ShowMinimapToggled(true));
        let enabled = ThemeSettings::load_from(&dir).unwrap();
        assert!(
            enabled.show_minimap && !enabled.minimap_auto,
            "总开关不覆盖手动常显选择"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #64 M1:打字机开关消息(带值设值 + 无参翻转)归约落
    /// `theme.show_typewriter` 并即时写 settings.json;重启(load_from)
    /// 后值仍在。只动该字段,minimap 与排版偏好不受影响(#55 同款通路)。
    /// 无参翻转是「视图」菜单/键位路径的消息(#67 M1 链路断言补段,与
    /// 专注模式 `ToggleFocusMode` 的同款断面对称)。
    #[test]
    fn show_typewriter_toggle_updates_field_and_persists() {
        let dir = temp_path("show-typewriter-toggle");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        assert!(!state.theme.show_typewriter, "出厂默认关");
        let font_before = state.theme.editor_font_size;

        state.apply(Message::TypewriterToggled(true));
        assert!(state.theme.show_typewriter);
        state.apply(Message::TypewriterToggled(false));
        assert!(!state.theme.show_typewriter, "往返翻回关");
        state.apply(Message::TypewriterToggled(true));

        // 菜单/快捷键无参翻转:当前为开,翻即取反,再翻回开
        state.apply(Message::ToggleTypewriter);
        assert!(!state.theme.show_typewriter);
        state.apply(Message::ToggleTypewriter);
        assert!(state.theme.show_typewriter, "再翻回开");

        let reloaded = ThemeSettings::load_from(&dir).unwrap();
        assert!(reloaded.show_typewriter, "重启后仍为开");
        assert_eq!(reloaded.editor_font_size, font_before, "排版偏好不受影响");
        assert!(reloaded.show_minimap, "minimap 开关不受影响");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #61 配置面:选区 AI 浮标开关消息归约落 `theme.show_selection_ai`
    /// 并即时写 settings.json;重启(load_from)后值仍在。出厂默认开
    /// (浮标是选区 AI 的唯一鼠标入口);旧 settings.json 缺字段由
    /// struct 级 serde(default) 走 `Default`(同为开),升级零变化。
    #[test]
    fn selection_ai_toggle_updates_field_and_persists() {
        let dir = temp_path("selection-ai-toggle");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        assert!(state.theme.show_selection_ai, "出厂默认开");

        state.apply(Message::SelectionAiToggled(false));
        assert!(!state.theme.show_selection_ai);
        state.apply(Message::SelectionAiToggled(true));
        assert!(state.theme.show_selection_ai, "往返翻回开");
        state.apply(Message::SelectionAiToggled(false));

        let reloaded = ThemeSettings::load_from(&dir).unwrap();
        assert!(!reloaded.show_selection_ai, "重启后仍为关");
        assert!(reloaded.show_minimap, "minimap 开关不受影响");
        // 旧档缺字段回落 Default:开
        let legacy_dir = dir.join("legacy");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        std::fs::write(legacy_dir.join("settings.json"), r#"{"mode":"light"}"#).unwrap();
        let upgraded = ThemeSettings::load_from(&legacy_dir).unwrap();
        assert!(upgraded.show_selection_ai, "旧档缺字段应回落默认开");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #64 M2:专注模式开关消息(带值设值 + 无参翻转)归约落
    /// `theme.show_focus_mode` 并即时写 settings.json;重启(load_from)后
    /// 值仍在。只动该字段,打字机与排版偏好不受影响(#55 同款通路)。
    #[test]
    fn show_focus_mode_toggle_updates_field_and_persists() {
        let dir = temp_path("show-focus-mode-toggle");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        assert!(!state.theme.show_focus_mode, "出厂默认关");
        let font_before = state.theme.editor_font_size;

        // 设置页带值消息:设值即落
        state.apply(Message::FocusModeToggled(true));
        assert!(state.theme.show_focus_mode);
        state.apply(Message::FocusModeToggled(false));
        assert!(!state.theme.show_focus_mode, "往返翻回关");

        // 菜单/快捷键无参翻转:翻即取反
        state.apply(Message::ToggleFocusMode);
        assert!(state.theme.show_focus_mode);
        state.apply(Message::ToggleFocusMode);
        assert!(!state.theme.show_focus_mode, "再翻回关");

        // 持久化以最后一次翻转为准(关),重启读档一致
        state.apply(Message::FocusModeToggled(true));
        let reloaded = ThemeSettings::load_from(&dir).unwrap();
        assert!(reloaded.show_focus_mode, "重启后仍为开");
        assert!(!reloaded.show_typewriter, "打字机开关不受影响");
        assert_eq!(reloaded.editor_font_size, font_before, "排版偏好不受影响");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #57 M2:禅定导航方式消息归约落 `theme.zen_nav` 并即时写
    /// settings.json;重启(load_from)后值仍在。切换同时复位会话级悬停
    /// 态 —— 残置的去抖计数/可见位不带入新模式(常显→悬停不闪 200ms
    /// 残显,悬停→常显干净淡入)。只动该字段,排版偏好与 minimap 不受
    /// 影响(与 #55 同款通路)。
    #[test]
    fn zen_nav_mode_change_updates_field_persists_and_resets_session() {
        let dir = temp_path("zen-nav-mode-change");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        assert_eq!(
            state.theme.zen_nav,
            crate::theme::ZenNavMode::Hover,
            "出厂默认悬停"
        );
        let font_before = state.theme.editor_font_size;

        state.zen_nav.visible = true;
        state.zen_nav.outside_frames = 7;
        state.apply(Message::ZenNavModeChanged(crate::theme::ZenNavMode::Always));
        assert_eq!(state.theme.zen_nav, crate::theme::ZenNavMode::Always);
        assert_eq!(
            state.zen_nav,
            crate::ui::zen_nav::ZenNavState::default(),
            "切换即复位会话级悬停态"
        );

        state.apply(Message::ZenNavModeChanged(crate::theme::ZenNavMode::Off));
        assert_eq!(state.theme.zen_nav, crate::theme::ZenNavMode::Off);

        let reloaded = ThemeSettings::load_from(&dir).unwrap();
        assert_eq!(
            reloaded.zen_nav,
            crate::theme::ZenNavMode::Off,
            "重启后仍为关"
        );
        assert_eq!(reloaded.editor_font_size, font_before, "排版偏好不受影响");
        assert!(reloaded.show_minimap, "minimap 开关不受影响");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `[[wikilink]]`:命中即打开同名文档(与文件树点击同一条路径),找不到
    /// 落提示行 —— 不静默无反应。
    #[test]
    fn wikilink_opens_matching_document_or_notices() {
        let dir = temp_path("wikilink");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("架构决策.md"), "# 架构\n").unwrap();
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        state.apply(Message::FileTreeRootSelected(dir.clone()));

        state.apply(Message::WikilinkClicked {
            target: "架构决策".into(),
        });
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(dir.join("架构决策.md").as_path()),
            "按文件名命中"
        );
        assert!(state.tabs.current().document.notice.is_none());

        // 找不到:提示行给出文档名
        state.apply(Message::WikilinkClicked {
            target: "不存在".into(),
        });
        assert!(state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("不存在")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `[[note.md]]`(目标自带 `.md` 后缀)点击同样打开 note.md —— 正向
    /// 解析与反向链接扫描的后缀容错对齐(#87):反向早已计入,点击此前
    /// 未必打得开;找不到仍落提示行,不静默。
    #[test]
    fn wikilink_with_md_suffix_still_opens_the_document() {
        let dir = temp_path("wikilink-suffix");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("架构决策.md"), "# 架构\n").unwrap();
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        state.apply(Message::FileTreeRootSelected(dir.clone()));

        state.apply(Message::WikilinkClicked {
            target: "架构决策.md".into(),
        });
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(dir.join("架构决策.md").as_path()),
            "目标带 .md 后缀也按 stem 打开"
        );
        assert!(state.tabs.current().document.notice.is_none());

        // 剥后缀后仍无命中:提示行带目标原文,不静默无反应
        state.apply(Message::WikilinkClicked {
            target: "不存在.md".into(),
        });
        assert!(state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("不存在.md")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 未选文档库根时点 wikilink:提示「先选根目录」,不猜路径。
    #[test]
    fn wikilink_without_root_notices() {
        let mut state = State::default();
        state.apply(Message::WikilinkClicked {
            target: "任何文档".into(),
        });
        assert!(state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("未设置文件树根目录")));
    }

    /// 本地文档链接(`Message::DocLinkClicked`):相对路径按当前文档目录
    /// 解析并打开;带锚点则光标落到标题行;找不到落提示行 —— 不静默。
    #[test]
    fn doc_link_opens_relative_target_and_jumps_to_anchor() {
        let dir = temp_path("doclink");
        std::fs::create_dir_all(dir.join("子目录")).unwrap();
        // canonicalize 落地:doclink 解析命中会 canonicalize(macOS /tmp 在
        // /private 符号链接之后,形态漂移会同时打歪路径相等断言与已开标签
        // 去重),期望基线须同形态。
        let dir = std::fs::canonicalize(dir).unwrap();
        std::fs::write(
            dir.join("子目录/正文.md"),
            "# 开头\n\n正文\n\n## 安装指南\n\n步骤\n",
        )
        .unwrap();
        std::fs::write(dir.join("子目录/doc.md"), "# 主文档\n").unwrap();
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        state.apply(Message::FileTreeRootSelected(dir.clone()));
        state.open_path(&dir.join("子目录/doc.md"));
        assert!(state.tabs.current().document.notice.is_none());

        // 相对路径 + 锚点:打开目标文档,光标落「安装指南」标题
        state.apply(Message::DocLinkClicked {
            href: "正文.md#安装指南".into(),
        });
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(dir.join("子目录/正文.md").as_path()),
            "相对当前文档目录解析"
        );
        assert!(
            state.tabs.current().cursor.jump_to.is_some(),
            "锚点落标题行"
        );

        // 已开的文档再点(不带锚点):激活同一标签,不重复开
        let before = state.tabs.tabs.len();
        let kept = state.tabs.current().cursor.jump_to;
        state.apply(Message::DocLinkClicked {
            href: "./正文.md".into(),
        });
        assert_eq!(state.tabs.tabs.len(), before, "路径去重,不开第二个标签");
        assert_eq!(
            state.tabs.current().cursor.jump_to,
            kept,
            "无锚点不产生新跳转"
        );

        // 找不到:提示带原文路径
        state.apply(Message::DocLinkClicked {
            href: "缺失.md".into(),
        });
        assert!(state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("缺失.md")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 纯锚点链接:不开标签,当前文档内把光标跳到标题;锚点不存在落提示。
    /// TOC 生成的 `[标题](#锚点)` 目录链接走的就是这一条。
    #[test]
    fn anchor_only_link_jumps_within_current_document() {
        let dir = temp_path("doclink-anchor");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("主文档.md"),
            "# 目录\n\n[跳转](#安装指南)\n\n## 安装指南\n\n步骤\n",
        )
        .unwrap();
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        state.apply(Message::FileTreeRootSelected(dir.clone()));
        state.open_path(&dir.join("主文档.md"));

        state.apply(Message::DocLinkClicked {
            href: "#安装指南".into(),
        });
        assert!(
            state.tabs.current().cursor.jump_to.is_some(),
            "文档内跳标题"
        );
        assert!(
            state.tabs.current().document.notice.is_none(),
            "命中不落提示"
        );

        state.apply(Message::DocLinkClicked {
            href: "#不存在的标题".into(),
        });
        assert!(state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("不存在的标题")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// TOC 生成的 slug 锚点(`#my-head` 这类拉丁形态)与 find_anchor 的
    /// slug 规则互认 —— 应用自己产出的目录链接必须点得回。
    #[test]
    fn toc_slug_anchors_resolve_back_to_headings() {
        let dir = temp_path("doclink-toc");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("toc.md"),
            "# Top\n\n- [指南](#my-head)\n\n## My Head\n\n内容\n",
        )
        .unwrap();
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        state.apply(Message::FileTreeRootSelected(dir.clone()));
        state.open_path(&dir.join("toc.md"));

        state.apply(Message::DocLinkClicked {
            href: "#my-head".into(),
        });
        assert!(state.tabs.current().cursor.jump_to.is_some());
        assert!(state.tabs.current().document.notice.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 反向链接的触发状态机(#15):帧末快照比对 —— 目标(根, 文档)变化才
    /// 防抖排程,同目标重复跑帧不重排(防抖合并且不满帧空转);到点归约
    /// 发起后台扫描,收流后结果整表刷新;切换文档触发重扫并清旧结果。
    #[test]
    fn backlinks_rescan_follows_document_and_root_changes() {
        let dir = temp_path("backlinks-state");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("note.md"), "目标文档\n").unwrap();
        std::fs::write(dir.join("src.md"), "见 [[note]]\n").unwrap();
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        state.apply(Message::FileTreeRootSelected(dir.clone()));
        state.apply(Message::FileSelected(dir.join("note.md")));

        // 帧 1:目标从 None → (根, note.md),防抖排程
        state.end_of_logic();
        assert_eq!(
            state.backlinks.requested_for,
            Some((dir.clone(), dir.join("note.md"))),
            "目标快照记录(根, 当前文档)"
        );
        assert!(state.backlinks.debounce_due.is_some());
        let first_due = state.backlinks.debounce_due.unwrap();

        // 帧 2:目标未变,不重排(否则每次 end_of_logic 都顺延,永远到不了点)
        std::thread::sleep(std::time::Duration::from_millis(5));
        state.end_of_logic();
        assert_eq!(
            state.backlinks.debounce_due,
            Some(first_due),
            "同目标重复跑帧不得顺延防抖计时"
        );

        // 到点归约(生产在 `ui::layout::reduce`,测试直发):后台扫描
        state.apply(Message::BacklinksRequested);
        assert_eq!(
            state.backlinks.status,
            crate::backlink_panel::BacklinkStatus::Scanning
        );
        assert_eq!(state.backlinks.debounce_due, None, "发起即清计时");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while state.backlinks.status == crate::backlink_panel::BacklinkStatus::Scanning
            && std::time::Instant::now() < deadline
        {
            state.end_of_logic(); // 收流在帧末
            if state.backlinks.status == crate::backlink_panel::BacklinkStatus::Scanning {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert_eq!(
            state.backlinks.status,
            crate::backlink_panel::BacklinkStatus::Finished
        );
        assert_eq!(state.backlinks.links.len(), 1);
        assert_eq!(
            state.backlinks.links[0].path,
            PathBuf::from("src.md"),
            "src.md 链接了 note.md"
        );

        // 切换文档:帧末快照变化 → 旧结果清空、重新排程(防抖合并)
        state.apply(Message::FileSelected(dir.join("src.md")));
        state.end_of_logic();
        assert!(state.backlinks.links.is_empty(), "目标变更即清旧结果");
        assert!(state.backlinks.debounce_due.is_some());
        state.apply(Message::BacklinksRequested);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while state.backlinks.status == crate::backlink_panel::BacklinkStatus::Scanning
            && std::time::Instant::now() < deadline
        {
            state.end_of_logic();
            if state.backlinks.status == crate::backlink_panel::BacklinkStatus::Scanning {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert_eq!(
            state.backlinks.status,
            crate::backlink_panel::BacklinkStatus::Finished
        );
        assert!(
            state.backlinks.links.is_empty(),
            "src.md 无人链接,零命中如实刷新"
        );

        // 换根:目标快照变化 → 重新排程(旧根的结果不再可信)
        state.apply(Message::FileTreeRootSelected(dir.join("不存在子目录")));
        std::fs::create_dir_all(dir.join("不存在子目录")).unwrap();
        state.end_of_logic();
        assert_ne!(
            state.backlinks.requested_for,
            Some((dir.clone(), dir.join("src.md"))),
            "换根后目标快照已更新"
        );
        assert!(state.backlinks.debounce_due.is_some(), "换根触发重扫");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 反向链接面板的点击跳转(#15):消息路由走 WikilinkClicked →
    /// `open_wikilink` 同一条链路 —— 命中即打开来源(含子目录与 `.md`
    /// 后缀容错);来源已打开时切标签而非双开;来源文件已删除落弱提示,
    /// 不崩溃。
    #[test]
    fn backlink_jump_opens_source_switches_tab_and_notices_when_missing() {
        let dir = temp_path("backlinks-jump");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("note.md"), "目标文档\n").unwrap();
        std::fs::write(dir.join("sub/b.md"), "子目录来源 [[note]]\n").unwrap();
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        state.apply(Message::FileTreeRootSelected(dir.clone()));
        state.apply(Message::FileSelected(dir.join("note.md")));

        // 面板行点击:载荷是 jump_target("sub/b.md") = "sub/b"
        state.apply(Message::WikilinkClicked {
            target: "sub/b".into(),
        });
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(dir.join("sub/b.md").as_path()),
            "子目录来源按相对路径命中"
        );
        // 基数 = 初始未命名标签 + note.md + 来源 sub/b.md
        assert_eq!(state.tabs.tabs.len(), 3, "来源未开,开新标签");

        // 回到目标文档后再点同一来源:切标签,不开第二个
        state.apply(Message::TabActivate(1));
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(dir.join("note.md").as_path()),
            "前置:当前停在目标文档"
        );
        state.apply(Message::WikilinkClicked {
            target: "sub/b".into(),
        });
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(dir.join("sub/b.md").as_path()),
            "已开来源被激活"
        );
        assert_eq!(state.tabs.tabs.len(), 3, "不双开同一文档");

        // 来源文件已被删除:落弱提示,不崩溃
        std::fs::remove_file(dir.join("sub/b.md")).unwrap();
        state.apply(Message::TabActivate(1));
        state.apply(Message::WikilinkClicked {
            target: "sub/b".into(),
        });
        assert!(
            state
                .tabs
                .current()
                .document
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("sub/b")),
            "来源缺失给弱提示:{:?}",
            state.tabs.current().document.notice
        );
        assert_eq!(state.tabs.tabs.len(), 3, "缺失分支不开标签");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 切 Live Preview 只翻标志:文本、修订号、dirty 都不动(共用同一 rope
    /// buffer 的可观测证据 —— 切模式没有任何「搬运」)。
    #[test]
    fn toggling_live_preview_only_flips_the_flag() {
        let mut state = State::default();
        let before = state.tabs.current().editor.text().to_owned();
        let rev = state.tabs.current().editor.revision();

        state.apply(Message::ToggleLivePreview);
        assert_eq!(state.render_mode, RenderMode::Live);
        assert_eq!(state.tabs.current().editor.text(), before);
        assert_eq!(state.tabs.current().editor.revision(), rev);
        assert!(!state.tabs.current().editor.is_dirty());
        assert!(state.tabs.current().live.blocks.len() > 1, "块表已建");
        assert!(
            state.tabs.current().live.browse_only,
            "切入 Live 默认浏览态"
        );

        // 三态循环:源码 → 写作 → 源码+对照 → 源码(两态时代的「再切回
        // 源码」现在要两步)
        state.apply(Message::ToggleLivePreview);
        assert_eq!(state.render_mode, RenderMode::Split);
        state.apply(Message::ToggleLivePreview);
        assert_eq!(state.render_mode, RenderMode::Source);
    }

    #[test]
    fn switching_modes_preserves_the_writing_caret() {
        let mut state = State::default();
        let tab = state.tabs.current_mut();
        tab.editor.replace_all("第一段\n\n第二段\n");
        let byte = tab.editor.text().find("第二").expect("第二段存在");
        let char_idx = tab.editor.byte_to_char(byte);
        tab.cursor.byte = Some(byte);

        // 源码 → Live:已有源码插入点直接成为活动块的 pending caret。
        state.apply(Message::ToggleLivePreview);
        let tab = state.tabs.current();
        assert_eq!(state.render_mode, RenderMode::Live);
        assert!(!tab.live.browse_only, "显式切换应保留正在写作的插入点");
        assert_eq!(tab.live.active, Some(1));
        assert_eq!(
            tab.live.pending_caret,
            Some((1, 0)),
            "Live 活动块收到源码插入点"
        );

        // Live → 分栏 → 源码:插入点在下一次回到源码时交给源码 TextEdit
        // (分栏档的中央本就是源码 TextEdit,不消费 pending_selection)。
        state.apply(Message::ToggleLivePreview);
        assert_eq!(state.render_mode, RenderMode::Split);
        state.apply(Message::ToggleLivePreview);
        let tab = state.tabs.current();
        assert_eq!(state.render_mode, RenderMode::Source);
        assert_eq!(
            tab.pending_selection,
            Some((char_idx, char_idx)),
            "源码 TextEdit 收到 Live 插入点"
        );
    }

    /// 新建文档跟随「默认模式」设置(theme.rs `DefaultViewMode`):写作/
    /// 源码/对照显式落档,「上次选择」取创建时刻的会话档。写作档的新文档
    /// 直接可写(不落浏览态),焦点请求挂在空文档块上。
    #[test]
    fn new_document_follows_default_view_mode() {
        // 出厂「上次选择」:Live 会话里新建 → 仍在写作,且新文档不落浏览态。
        let mut state = State::default();
        state.apply(Message::ToggleLivePreview);
        assert_eq!(state.render_mode, RenderMode::Live);
        state.apply(Message::FileCommand(FileCmd::New));
        assert_eq!(state.render_mode, RenderMode::Live);
        assert!(
            !state.tabs.current().live.browse_only,
            "新建直落写作态,不等单击"
        );
        assert!(
            state.tabs.current().focus_live_on_next_frame,
            "焦点请求落 Live 空文档块"
        );
        assert!(state.tabs.current().editor.text().is_empty(), "新标签为空");

        // 显式源码:新建进源码,焦点走源码 TextEdit(既有行为)。
        state.theme.default_view_mode = crate::theme::DefaultViewMode::Source;
        state.apply(Message::FileCommand(FileCmd::New));
        assert_eq!(state.render_mode, RenderMode::Source);
        assert!(state.tabs.current().focus_editor_on_next_frame);

        // 显式写作:当前在源码档也切进写作。
        state.theme.default_view_mode = crate::theme::DefaultViewMode::Live;
        state.apply(Message::FileCommand(FileCmd::New));
        assert_eq!(state.render_mode, RenderMode::Live);
        assert!(!state.tabs.current().live.browse_only);

        // 显式对照:新建进分栏,中央即源码 TextEdit。
        state.theme.default_view_mode = crate::theme::DefaultViewMode::Split;
        state.apply(Message::FileCommand(FileCmd::New));
        assert_eq!(state.render_mode, RenderMode::Split);
        assert!(state.tabs.current().focus_editor_on_next_frame);
    }

    #[test]
    fn fresh_start_uses_writing_and_comparison_is_a_source_tool() {
        let mut state = State::default();
        state.restore_layout(LayoutSettings::fresh_install());
        assert_eq!(state.theme.mode, ThemeMode::Light);
        assert_eq!(state.layout.left_view, SidebarTab::Outline);
        let text = state.tabs.current().editor.text().to_owned();
        let revision = state.tabs.current().editor.revision();
        let tab_id = state.tabs.current().id;
        assert_eq!(state.render_mode, RenderMode::Live);
        // 三态循环:写作 → 分栏 → 源码(三态时代的完整一圈)
        state.apply(Message::ToggleLivePreview);
        assert_eq!(state.render_mode, RenderMode::Split);
        state.apply(Message::ToggleLivePreview);
        assert_eq!(state.render_mode, RenderMode::Source);
        assert_eq!(state.tabs.current().id, tab_id);
        assert_eq!(state.tabs.current().editor.text(), text);
        assert_eq!(state.tabs.current().editor.revision(), revision);
        assert!(!state.tabs.current().editor.is_dirty());
    }

    #[test]
    fn writing_and_source_preferences_survive_restart_and_legacy_migration() {
        let dir = temp_path("writing-layout");
        std::fs::create_dir_all(&dir).unwrap();
        for right in [false, true] {
            // 旧版本没有模式字段,升级默认写作;旧版 `right` 键(右栏对照
            // 预览,P4 退役)按 serde(default) 结构体级口径**静默忽略** ——
            // 升级不因多出来的键失败。
            std::fs::write(dir.join("layout.json"), format!(r#"{{"right":{right}}}"#)).unwrap();
            let mut state = State::default();
            state.restore_layout(LayoutSettings::load_from(&dir).unwrap());
            assert_eq!(state.render_mode, RenderMode::Live);
            // 三档都要往返:save → 重启 → 同档(含新增的 split 档)
            for mode in [RenderMode::Live, RenderMode::Split, RenderMode::Source] {
                while state.render_mode != mode {
                    state.apply(Message::ToggleLivePreview);
                }
                state.layout.save_to(Some(&dir)).unwrap();
                let mut restarted = State::default();
                restarted.restore_layout(LayoutSettings::load_from(&dir).unwrap());
                assert_eq!(restarted.render_mode, mode);
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mode_commands_exit_zen_first() {
        let mut state = State::default();
        state.apply(Message::ZenToggled);
        state.apply(Message::ToggleLivePreview);
        assert!(!state.layout.zen, "模式命令先退出禅定");
        assert_eq!(state.render_mode, RenderMode::Live);
        state.apply(Message::ZenToggled);
        state.apply(Message::ZenToggled);
        state.apply(Message::ZenToggled);
        // 写作 → 分栏 → 源码(三态循环回到源码档)
        state.apply(Message::ToggleLivePreview);
        state.apply(Message::ToggleLivePreview);
        assert!(!state.layout.zen && state.render_mode == RenderMode::Source);
    }

    #[test]
    fn saving_in_writing_zen_keeps_source_comparison_preference() {
        let dir = temp_path("writing-zen-layout");
        let mut state = State::default();
        state.layout.left = false;
        state.apply(Message::ToggleLivePreview);
        state.apply(Message::ZenToggled);
        state.layout.save_to(Some(&dir)).unwrap();
        let mut restarted = State::default();
        restarted.restore_layout(LayoutSettings::load_from(&dir).unwrap());
        assert_eq!(restarted.render_mode, RenderMode::Live);
        assert!(!restarted.layout.left && !restarted.layout.zen);
        // 写作 → 分栏 → 源码(三态循环)
        restarted.apply(Message::ToggleLivePreview);
        restarted.apply(Message::ToggleLivePreview);
        assert_eq!(restarted.render_mode, RenderMode::Source);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 系统主题只在「跟随系统」模式轮询:其余模式返回 None(egui 得以收敛
    /// 到深度空闲),跟随模式返回下一次探测时刻。
    #[test]
    fn system_theme_polls_only_when_following_system() {
        let mut state = State::default();
        assert_eq!(
            state.poll_system_theme(std::time::Instant::now()),
            None,
            "默认浅色不轮询"
        );
        state.apply(Message::ThemeChanged(ThemeMode::System));
        let now = std::time::Instant::now();
        let due = state.poll_system_theme(now);
        assert!(due.is_some(), "跟随系统模式给出下一次探测时刻");
        assert!(due.unwrap_or(now) > now);
        // 未到点不重复探测(同一 due 原样返回)
        assert_eq!(state.poll_system_theme(now), due);
    }

    /// 落盘失败(目录路径被同名文件占据):切换照常生效,失败带路径进提示行。
    #[test]
    fn theme_save_failure_lands_in_notice_but_mode_still_changes() {
        let blocker = temp_path("theme-blocker");
        std::fs::write(&blocker, b"x").unwrap();
        let mut state = State {
            settings_dir: Some(blocker.clone()),
            ..State::default()
        };

        state.apply(Message::ThemeChanged(ThemeMode::Light));
        assert_eq!(state.theme.mode, ThemeMode::Light, "持久化失败不影响切换");
        let notice = state.tabs.current().document.notice.as_deref().unwrap();
        assert!(notice.contains("主题保存失败"), "{notice}");
        assert!(notice.contains("theme-blocker"), "{notice}");
        let _ = std::fs::remove_file(&blocker);
    }

    /// 文件树消息链:换根(持久化 + 最近列表)、展开翻转、懒加载在帧末补
    /// 齐子项、点击文件换入缓冲并展开祖先;dirty 时点击文件被拦。
    #[test]
    fn file_tree_messages_drive_root_toggle_and_open() {
        let dir = temp_path("filetree-root");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/note.md"), "# 树内标题\n").unwrap();
        std::fs::write(dir.join("top.md"), "# 顶层\n").unwrap();

        let settings_dir = temp_path("filetree-settings");
        let mut state = State {
            settings_dir: Some(settings_dir.clone()),
            ..State::default()
        };
        state.apply(Message::FileTreeRootSelected(dir.clone()));
        assert_eq!(state.file_tree.root.as_deref(), Some(dir.as_path()));
        assert_eq!(state.file_tree.recents, vec![dir.clone()]);
        assert!(
            settings_dir.join("file_tree.json").exists(),
            "最近目录持久化"
        );

        // 懒加载:换根不列举,帧末归约才列根级子项
        assert!(state.file_tree.children.is_empty());
        state.end_of_logic();
        let root_children = state.file_tree.children.get(&dir).unwrap();
        assert_eq!(
            root_children
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["docs", "top.md"]
        );

        // 展开翻转:toggle 后帧末补齐该目录子项
        state.apply(Message::FileTreeToggled(dir.join("docs")));
        state.end_of_logic();
        assert!(state
            .file_tree
            .children
            .contains_key(dir.join("docs").as_path()));

        // 点击文件:换入缓冲、树内祖先展开(为高亮行可见)
        let note = dir.join("docs/note.md");
        state.apply(Message::FileSelected(note.clone()));
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(note.as_path())
        );
        assert_eq!(state.tabs.current_mut().editor.text(), "# 树内标题\n");
        assert_eq!(
            state.file_tree.expanded.get(dir.join("docs").as_path()),
            Some(&true),
            "打开的文件的父目录已展开"
        );

        // 多标签:dirty 时点击树上另一文件不再拦截,而是另开新标签;
        // 原标签的草稿与落盘身份原样保留
        state.tabs.current_mut().editor.insert_chars(0, "草稿");
        state.apply(Message::FileSelected(dir.join("top.md")));
        assert_eq!(
            state.tabs.tabs.len(),
            3,
            "另开了一个新标签(此前 note 占一个)"
        );
        assert_eq!(
            state.tabs.tabs[1].document.path.as_deref(),
            Some(note.as_path()),
            "note 标签身份不动"
        );
        assert!(
            state.tabs.tabs[1].editor.text().starts_with("草稿"),
            "草稿保留"
        );
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(dir.join("top.md").as_path()),
            "当前切到新标签"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&settings_dir);
    }

    /// 文件栏「新建」全链路:请求挂浮窗(预填名让开盘上撞名)→ 确认落盘
    /// 空 md → **该目录树缓存失效后帧末即列出新文件** → 开新标签打开,
    /// 且目录是**当前文档所在目录**而不是树根。
    ///
    /// 「树里看得见」这一条是本测试的重点:缓存不失效的话整条功能在用户
    /// 眼里就是「文件建出来了但树上没有」,而落盘与开标签都照样成功 ——
    /// 只断言「文件存在」会漏掉这个缺陷。
    #[test]
    fn new_file_creates_in_current_dir_and_shows_in_tree() {
        let dir = temp_path("new-file");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        let settings_dir = temp_path("new-file-settings");
        let mut state = State {
            settings_dir: Some(settings_dir.clone()),
            ..State::default()
        };
        state.apply(Message::FileTreeRootSelected(dir.clone()));
        state.end_of_logic();
        let existing = dir.join("docs/笔记.md");
        std::fs::write(&existing, "# 已有\n").unwrap();
        // 当前文档落在 docs/ 下 → 新建也该落在这里
        state.apply(Message::FileSelected(existing.clone()));
        state.end_of_logic();

        state.apply(Message::FileTreeNewFileRequested(dir.join("docs")));
        let pending = state.file_tree.new_file.clone().expect("浮窗已挂起");
        assert_eq!(pending.dir, dir.join("docs"), "目标是当前文档所在目录");

        // 改成确定的名字再确认
        if let Some(new_file) = state.file_tree.new_file.as_mut() {
            new_file.draft = "新文档".to_owned();
        }
        state.apply(Message::FileTreeNewFileConfirmed);
        assert_eq!(state.file_tree.new_file, None, "成功后关浮窗");

        let created = dir.join("docs/新文档.md");
        assert!(created.is_file(), "空 md 落盘:{}", created.display());
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(created.as_path()),
            "开新标签打开"
        );
        assert_eq!(
            state.file_tree.expanded.get(dir.join("docs").as_path()),
            Some(&true),
            "祖先展开 → 新文件在树里高亮可见"
        );

        state.end_of_logic();
        let docs = state
            .file_tree
            .children
            .get(dir.join("docs").as_path())
            .expect("帧末已重列");
        let names: Vec<&str> = docs.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"新文档.md"),
            "新文件在本会话的树里可见(缓存失效生效):{names:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&settings_dir);
    }

    /// 预填名让开盘上撞名:已有 `新建文档.md` 时预填带 `-1` 的那个(而不是
    /// 让用户每次先删一遍预填名)。
    #[test]
    fn new_file_prefill_yields_to_existing_name() {
        let dir = temp_path("new-file-prefill");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("新建文档.md"), "x").unwrap();
        let mut state = State::default();
        state.apply(Message::FileTreeRootSelected(dir.clone()));

        state.apply(Message::FileTreeNewFileRequested(dir.clone()));
        assert_eq!(
            state.file_tree.new_file.as_ref().unwrap().draft,
            "新建文档-1.md",
            "预填名让开既有文件"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 新建的两条失败路径都不落文件:空名 → 浮窗保留(可改名重试);目标目录
    /// 被外部删掉 → 提示带路径。
    #[test]
    fn new_file_rejects_blank_name_and_missing_dir() {
        let dir = temp_path("new-file-bad");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut state = State::default();
        state.apply(Message::FileTreeRootSelected(dir.clone()));

        // 空名:浮窗保留,不落文件
        state.apply(Message::FileTreeNewFileRequested(dir.clone()));
        state.file_tree.new_file.as_mut().unwrap().draft = "   ".to_owned();
        state.apply(Message::FileTreeNewFileConfirmed);
        assert!(state.file_tree.new_file.is_some(), "空名保留浮窗供改名重试");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "空名一个文件都没落"
        );

        // 目录已被外部删掉:确认时提示带路径
        let gone = dir.join("gone");
        std::fs::create_dir_all(&gone).unwrap();
        state.file_tree.new_file = Some(crate::filetree::NewFileDraft {
            dir: gone.clone(),
            draft: "x".to_owned(),
            focus_pending: true,
        });
        std::fs::remove_dir_all(&gone).unwrap();
        state.apply(Message::FileTreeNewFileConfirmed);
        let notice = state.tabs.current().document.notice.as_deref().unwrap();
        assert!(notice.contains("新建失败"), "{notice}");
        assert!(notice.contains("x.md"), "提示带实际落点:{notice}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 「刷新」重扫根 + 展开中目录:外部新建的文件随即出现在树里,未展开
    /// 的兄弟目录不重扫(懒加载语义不被破坏)。
    #[test]
    fn refresh_rescans_visible_dirs_only() {
        let dir = temp_path("tree-refresh");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("open")).unwrap();
        std::fs::create_dir_all(dir.join("shut")).unwrap();
        let settings_dir = temp_path("tree-refresh-settings");
        let mut state = State {
            settings_dir: Some(settings_dir.clone()),
            ..State::default()
        };
        state.apply(Message::FileTreeRootSelected(dir.clone()));
        state.apply(Message::FileTreeToggled(dir.join("open")));
        state.end_of_logic();
        let root_before = state.file_tree.children[&dir].entries.len();

        // 外部(别的程序/终端)在三处各建一个文件
        std::fs::write(dir.join("外部根.md"), "x").unwrap();
        std::fs::write(dir.join("open/外部子.md"), "x").unwrap();
        std::fs::write(dir.join("shut/外部藏.md"), "x").unwrap();
        assert_eq!(
            state.file_tree.children[&dir].entries.len(),
            root_before,
            "刷新前缓存不动(这正是需要刷新的原因)"
        );

        state.apply(Message::FileTreeRefreshRequested);
        // 反馈窗口起点已置:即便重扫在同一帧就做完,spinner 也有
        // `REFRESH_FEEDBACK_MIN` 的时长可转(坤哥反馈「点了没个动态」)。
        let started = state.file_tree.refresh_started.expect("刷新必须开反馈窗口");
        assert!(state.file_tree.refreshing(started), "刚点的这一刻在窗口内");
        assert!(
            !state
                .file_tree
                .refreshing(started + crate::filetree::REFRESH_FEEDBACK_MIN),
            "窗口下限一到即退出(否则 spinner 永不落回图标)"
        );

        state.end_of_logic();
        let root_names: Vec<&str> = state.file_tree.children[&dir]
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert!(root_names.contains(&"外部根.md"), "{root_names:?}");
        let open_names: Vec<&str> = state.file_tree.children[&dir.join("open")]
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert!(open_names.contains(&"外部子.md"), "{open_names:?}");
        assert!(
            !state.file_tree.children.contains_key(&dir.join("shut")),
            "未展开目录不因刷新被拉进内存(懒加载不被破坏)"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&settings_dir);
    }

    /// 导出:写出的是完整 HTML 文档(标题取自缓冲当前内容),且不触碰文档
    /// 身份 —— 路径不被认领、dirty 不被清、失败提示带路径。
    #[test]
    fn export_writes_html_without_touching_document_identity() {
        let path = temp_path("export.html");
        let mut state = State::default();
        state
            .tabs
            .current_mut()
            .editor
            .insert_chars(0, "# 导出标题\n");
        assert!(state.tabs.current_mut().editor.is_dirty());

        state.export_html_to(&path);
        let html = std::fs::read_to_string(&path).unwrap();
        assert!(html.starts_with("<!DOCTYPE html>"), "{html}");
        assert!(html.contains("<h1 id=\"导出标题\">导出标题</h1>"), "{html}");
        assert!(html.contains("max-width: 46em"), "{html}");
        // 派生物:dirty 保留、路径不认领
        assert!(state.tabs.current_mut().editor.is_dirty());
        assert_eq!(state.tabs.current().document.path, None);
        let _ = std::fs::remove_file(&path);

        state.export_html_to(&PathBuf::from("/latermd/no/such/dir.html"));
        let notice = state.tabs.current().document.notice.as_deref().unwrap();
        assert!(notice.contains("导出失败"), "{notice}");
        assert!(notice.contains("dir.html"), "{notice}");
    }

    /// 导出 PDF:写出的字节是合法 PDF(%PDF 头 / %%EOF 尾)且嵌入了字体
    /// 程序;派生物语义与 HTML 一致(路径不认领、dirty 不清、成功清提示)。
    /// 本机无 CJK 候选时改为断言「明确的字体错误」分支:不 panic、不落
    /// 残缺文件(发现层的注入式命中/降级断言在 latermd-export 侧)。
    #[test]
    fn export_pdf_writes_bytes_or_reports_missing_cjk() {
        let path = temp_path("export.pdf");
        let mut state = State::default();
        state
            .tabs
            .current_mut()
            .editor
            .insert_chars(0, "# 导出标题\n\n中文正文 English 混排。\n");
        assert!(state.tabs.current_mut().editor.is_dirty());

        state.export_pdf_to(&path);
        match std::fs::read(&path) {
            Ok(bytes) => {
                assert!(
                    bytes.starts_with(b"%PDF-"),
                    "PDF 头缺失:{:?}",
                    &bytes[..bytes.len().min(8)]
                );
                let mut end = bytes.len();
                while end > 0 && matches!(bytes[end - 1], b'\n' | b'\r') {
                    end -= 1;
                }
                assert!(bytes[..end].ends_with(b"%%EOF"));
                assert!(
                    bytes.windows(9).any(|w| w == b"/FontFile"),
                    "嵌入字体程序缺失 —— 中文将在缺字体的机器上变方框"
                );
                assert!(
                    state.tabs.current_mut().editor.is_dirty(),
                    "派生物不清 dirty"
                );
                assert_eq!(state.tabs.current().document.path, None, "不认领路径");
                assert_eq!(
                    state.tabs.current().document.notice,
                    None,
                    "成功路径应清提示行"
                );
            }
            Err(_) => {
                let notice = state
                    .tabs
                    .current()
                    .document
                    .notice
                    .as_deref()
                    .unwrap_or("");
                assert!(
                    notice.contains("中文字体"),
                    "无 CJK 候选应落「未找到中文字体」提示,实为:{notice}"
                );
            }
        }
        let _ = std::fs::remove_file(&path);
    }

    /// 搜索点击归约:换文件 + 跳到命中行行首。行号 1 起 → rope 行 0 起,
    /// 目标是第 2 行(含 CJK)行首的字符偏移。
    #[test]
    fn search_click_opens_file_and_jumps_to_hit_line() {
        let dir = temp_path("search-click");
        std::fs::create_dir_all(&dir).unwrap();
        let note = dir.join("note.md");
        std::fs::write(&note, "# 甲\n你好 world\n第三行\n").unwrap();

        let mut state = State::default();
        state.apply(Message::SearchResultClicked(note.clone(), 2));

        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(note.as_path())
        );
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            "# 甲\n你好 world\n第三行\n"
        );
        let line2_byte = state.tabs.current_mut().editor.line_to_byte(1);
        assert_eq!(line2_byte, 6, "前置:首行「# 甲\\n」共 6 字节");
        {
            let tab = state.tabs.current();
            assert_eq!(
                tab.cursor.jump_to,
                Some(tab.editor.byte_to_char(line2_byte)),
                "光标(字符偏移)落在第二行行首"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 已打开文件上的点击:不重开、只更新跳转目标(路径相同即跳过 IO)。
    #[test]
    fn search_click_on_current_file_only_jumps() {
        let dir = temp_path("search-same");
        std::fs::create_dir_all(&dir).unwrap();
        let note = dir.join("note.md");
        std::fs::write(&note, "一\n二\n三\n").unwrap();

        let mut state = State::default();
        state.apply(Message::SearchResultClicked(note.clone(), 1));
        assert_eq!(state.tabs.current_mut().cursor.jump_to, Some(0));
        state.apply(Message::SearchResultClicked(note.clone(), 3));
        {
            let tab = state.tabs.current();
            assert_eq!(
                tab.cursor.jump_to,
                Some(tab.editor.byte_to_char(tab.editor.line_to_byte(2))),
                "同行内再次点击,光标前进到第三行"
            );
        }
        assert!(
            state.tabs.current().document.notice.is_none(),
            "未发生 IO 失败"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 过期行号(搜索后文件变短/被外部修改):钳制到文末,不 panic。
    #[test]
    fn search_click_with_stale_line_clamps_to_end() {
        let dir = temp_path("search-stale");
        std::fs::create_dir_all(&dir).unwrap();
        let note = dir.join("note.md");
        std::fs::write(&note, "只有一行\n").unwrap();

        let mut state = State::default();
        state.apply(Message::SearchResultClicked(note.clone(), 999));
        {
            let tab = state.tabs.current();
            assert_eq!(
                tab.cursor.jump_to,
                Some(tab.editor.len_chars()),
                "行号远超行数,钳制到文末"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 点击指向已消失的文件:读盘失败落提示行,不换文档、不设跳转。
    #[test]
    fn search_click_on_missing_file_notifies_without_jump() {
        let mut state = State::default();
        let before = state.tabs.current_mut().editor.text().to_owned();
        state.apply(Message::SearchResultClicked(
            PathBuf::from("/latermd/no/such/hit.md"),
            1,
        ));
        assert!(
            state.tabs.current().document.notice.is_some(),
            "读盘失败有提示"
        );
        assert_eq!(state.tabs.current().document.path, None, "文档身份未变");
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            before,
            "缓冲未被触碰"
        );
        assert_eq!(state.tabs.current_mut().cursor.jump_to, None, "不设跳转");
    }

    /// 搜索唤起左栏:输入变化(SearchQueryChanged)与胶囊聚焦
    /// (SearchPanelRevealed)都把左栏展开并停 Search 页;禅定模式豁免
    /// (left 是被 enter_zen 借走的临时态,不擅自翻开)。
    #[test]
    fn search_input_reveals_sidebar_and_zen_is_exempt() {
        let mut state = State::default();
        assert_eq!(state.layout.left_view, SidebarTab::Files);
        state.apply(Message::SearchQueryChanged);
        assert!(state.layout.left, "输入搜索词左栏必须展开");
        assert_eq!(state.layout.left_view, SidebarTab::Search);

        // 收起后再聚焦同样展开(点进胶囊但还没输入的场合)
        state.layout.left = false;
        state.apply(Message::SearchPanelRevealed);
        assert!(state.layout.left);

        // 禅定豁免:两种消息都不动左栏
        state.layout.left = false;
        state.apply(Message::ZenToggled);
        assert!(state.layout.zen);
        state.apply(Message::SearchQueryChanged);
        state.apply(Message::SearchPanelRevealed);
        assert!(!state.layout.left, "禅定下不展开左栏");
        assert_eq!(state.layout.left_view, SidebarTab::Search, "目标页不回退");
    }

    /// 全部替换链路(未打开的文件走磁盘):请求挂确认浮窗(文件数按命中
    /// 路径去重折算)→ 取消只撤浮窗一字不改 → 确认后逐文件落盘、零命中
    /// 文件原样、回执记账、重发搜索(命中清空待流式回填)。
    #[test]
    fn search_replace_all_flow_over_disk_files() {
        let dir = temp_path("search-replace");
        std::fs::create_dir_all(&dir).unwrap();
        let note = dir.join("note.md");
        std::fs::write(&note, "foo bar\nfoo again\n").unwrap();
        let other = dir.join("other.md");
        std::fs::write(&other, "没有命中\n").unwrap();

        let mut state = State::default();
        state.file_tree.root = Some(dir.clone());
        state.search.query = "foo".into();
        state.search.replace_with = "baz".into();
        state.search.status = crate::search::SearchStatus::Finished;
        // 命中快照直接注填(后台流式收流不在单测里等);other.md 入列但
        // 内容零命中 —— 替换时它一个字节都不动
        state.search.hits = vec![
            crate::search::SearchResult {
                path: note.clone(),
                line_no: 1,
                line_text: "foo bar".into(),
                match_span: 0..0,
            },
            crate::search::SearchResult {
                path: note.clone(),
                line_no: 2,
                line_text: "foo again".into(),
                match_span: 0..0,
            },
            crate::search::SearchResult {
                path: other.clone(),
                line_no: 1,
                line_text: "没有命中".into(),
                match_span: 0..0,
            },
        ];

        state.apply(Message::SearchReplaceAllRequested);
        assert_eq!(
            state.search.confirm_replace.as_ref().unwrap().paths.len(),
            2,
            "按路径去重折算"
        );

        state.apply(Message::SearchReplaceAllCancelled);
        assert_eq!(state.search.confirm_replace, None);
        assert_eq!(
            std::fs::read_to_string(&note).unwrap(),
            "foo bar\nfoo again\n",
            "取消不落盘"
        );

        state.apply(Message::SearchReplaceAllRequested);
        state.apply(Message::SearchReplaceAllConfirmed);
        assert_eq!(state.search.confirm_replace, None);
        assert_eq!(
            std::fs::read_to_string(&note).unwrap(),
            "baz bar\nbaz again\n"
        );
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "没有命中\n");
        let report = state.search.last_replace.as_ref().unwrap();
        assert_eq!((report.files, report.replacements), (1, 2));
        assert!(report.failed.is_empty());
        assert!(
            state.search.hits.is_empty()
                || state.search.status == crate::search::SearchStatus::Running,
            "确认后重发搜索,命中清空待流式回填"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 打开中的标签改缓冲不直接写盘:脏稿替换后保持脏(用户未保存修改
    /// 原样保留,磁盘不动);干净稿发生替换后必须变脏。
    #[test]
    fn search_replace_respects_open_tabs_dirty_state() {
        let dir = temp_path("search-replace-tabs");
        std::fs::create_dir_all(&dir).unwrap();
        let clean = dir.join("clean.md");
        std::fs::write(&clean, "alpha beta\n").unwrap();
        let dirty = dir.join("dirty.md");
        std::fs::write(&dirty, "beta 原稿\n").unwrap();

        let mut state = State::default();
        state.file_tree.root = Some(dir.clone());
        state.apply(Message::SearchResultClicked(clean.clone(), 1));
        state.apply(Message::SearchResultClicked(dirty.clone(), 1));
        // 当前标签(dirty.md)制造未保存修改
        state.tabs.current_mut().editor.insert_chars(0, "用户改动 ");
        assert!(state.tabs.current().editor.is_dirty());

        state.search.query = "beta".into();
        state.search.replace_with = "gamma".into();
        state.search.status = crate::search::SearchStatus::Finished;
        state.search.hits = vec![
            crate::search::SearchResult {
                path: clean.clone(),
                line_no: 1,
                line_text: "alpha beta".into(),
                match_span: 0..0,
            },
            crate::search::SearchResult {
                path: dirty.clone(),
                line_no: 1,
                line_text: "beta 原稿".into(),
                match_span: 0..0,
            },
        ];
        state.apply(Message::SearchReplaceAllRequested);
        state.apply(Message::SearchReplaceAllConfirmed);

        // 磁盘:两个文件都未写(替换发生在缓冲)
        assert_eq!(
            std::fs::read_to_string(&clean).unwrap(),
            "alpha beta\n",
            "干净标签的落盘交给用户保存链路"
        );
        assert_eq!(std::fs::read_to_string(&dirty).unwrap(), "beta 原稿\n");
        // 原本干净的标签:替换在缓冲,尚未保存。
        let clean_index = state.tabs.find_by_path(&clean).unwrap();
        assert_eq!(state.tabs.tabs[clean_index].editor.text(), "alpha gamma\n");
        assert!(
            state.tabs.tabs[clean_index].editor.is_dirty(),
            "缓冲尚未保存,替换后必须变脏"
        );
        // 脏标签(当前):用户改动保留 + 替换落进缓冲 + 仍脏
        assert_eq!(state.tabs.current().editor.text(), "用户改动 gamma 原稿\n");
        assert!(state.tabs.current().editor.is_dirty(), "脏稿保持脏");
        let report = state.search.last_replace.as_ref().unwrap();
        assert_eq!((report.files, report.replacements), (2, 2));
        state.apply(Message::TabCloseRequested(clean_index));
        assert!(
            state.tabs.confirm_close.is_some(),
            "替换后关闭必须确认未保存修改"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 非 UTF-8 文档跳过不写盘(替换基于 Unicode 语义,lossy 会把坏字节
    /// 写回磁盘),回执逐文件记因;UTF-8 邻居照常替换。
    #[test]
    fn search_replace_skips_non_utf8_with_report() {
        let dir = temp_path("search-replace-utf8");
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.md");
        std::fs::write(&good, "foo\n").unwrap();
        let bin = dir.join("bin.md");
        std::fs::write(&bin, b"foo \xff\xfe tail\n").unwrap();

        let mut state = State::default();
        state.file_tree.root = Some(dir.clone());
        state.search.query = "foo".into();
        state.search.replace_with = "baz".into();
        state.search.status = crate::search::SearchStatus::Finished;
        state.search.hits = vec![
            crate::search::SearchResult {
                path: good.clone(),
                line_no: 1,
                line_text: "foo".into(),
                match_span: 0..0,
            },
            crate::search::SearchResult {
                path: bin.clone(),
                line_no: 1,
                line_text: "foo …".into(),
                match_span: 0..0,
            },
        ];
        state.apply(Message::SearchReplaceAllRequested);
        assert_eq!(
            state.search.confirm_replace.as_ref().unwrap().paths.len(),
            2
        );
        state.apply(Message::SearchReplaceAllConfirmed);

        assert_eq!(std::fs::read_to_string(&good).unwrap(), "baz\n");
        assert_eq!(
            std::fs::read(&bin).unwrap(),
            b"foo \xff\xfe tail\n",
            "非 UTF-8 一个字节不动"
        );
        let report = state.search.last_replace.as_ref().unwrap();
        assert_eq!(report.files, 1);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, "bin.md");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_replace_rejects_incomplete_or_stale_confirmation() {
        let dir = temp_path("search-replace-stale");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("note.md");
        std::fs::write(&path, "foo\n").unwrap();
        let mut state = State::default();
        state.file_tree.root = Some(dir.clone());
        state.search.query = "foo".into();
        state.search.replace_with = "bar".into();
        state.search.hits = vec![crate::search::SearchResult {
            path: path.clone(),
            line_no: 1,
            line_text: "foo".into(),
            match_span: 0..0,
        }];
        for status in [
            crate::search::SearchStatus::Idle,
            crate::search::SearchStatus::Running,
        ] {
            state.search.status = status;
            state.apply(Message::SearchReplaceAllRequested);
            assert!(state.search.confirm_replace.is_none());
            state.apply(Message::SearchReplaceAllConfirmed);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "foo\n");
        }
        state.search.status = crate::search::SearchStatus::Finished;
        state.search.truncated = true;
        state.apply(Message::SearchReplaceAllRequested);
        assert!(state.search.confirm_replace.is_none());
        state.search.truncated = false;
        for change in 0..5 {
            state.apply(Message::SearchReplaceAllRequested);
            assert!(state.search.confirm_replace.is_some());
            match change {
                0 => state.search.replace_with = "changed".into(),
                1 => state.search.query = "other".into(),
                2 => state.search.generation += 1,
                3 => state.file_tree.root = Some(dir.join("other")),
                _ => state.search.truncated = true,
            }
            state.apply(Message::SearchReplaceAllConfirmed);
            assert!(state.search.confirm_replace.is_none());
            assert!(state.search.last_replace.is_none());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "foo\n");
            state.search.query = "foo".into();
            state.search.replace_with = "bar".into();
            state.search.truncated = false;
            state.file_tree.root = Some(dir.clone());
        }
        state.apply(Message::SearchReplaceAllRequested);
        state.apply(Message::SearchQueryChanged);
        state.apply(Message::SearchReplaceAllConfirmed);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "foo\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn search_replace_uses_frozen_target_paths() {
        let dir = temp_path("search-replace-frozen");
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.md");
        let b = dir.join("b.md");
        std::fs::write(&a, "foo\n").unwrap();
        std::fs::write(&b, "foo\n").unwrap();
        let mut state = State::default();
        state.file_tree.root = Some(dir.clone());
        state.search.query = "foo".into();
        state.search.replace_with = "bar".into();
        state.search.status = crate::search::SearchStatus::Finished;
        let hit = |path: PathBuf| crate::search::SearchResult {
            path,
            line_no: 1,
            line_text: "foo".into(),
            match_span: 0..0,
        };
        state.search.hits = vec![hit(a.clone())];
        state.apply(Message::SearchReplaceAllRequested);
        state.search.hits.push(hit(b.clone()));
        state.apply(Message::SearchReplaceAllConfirmed);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "bar\n");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "foo\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 多标签:dirty 时点击搜索结果不再拦截,而是另开新标签跳转;
    /// 原标签的草稿与身份原样保留(与文件树点击同语义)。
    #[test]
    fn search_click_while_dirty_opens_new_tab() {
        let dir = temp_path("search-dirty");
        std::fs::create_dir_all(&dir).unwrap();
        let note = dir.join("note.md");
        std::fs::write(&note, "目标\n").unwrap();

        let mut state = State::default();
        state.tabs.current_mut().editor.insert_chars(0, "草稿");
        assert!(state.tabs.current_mut().editor.is_dirty());

        state.apply(Message::SearchResultClicked(note.clone(), 1));
        assert_eq!(state.tabs.tabs.len(), 2, "另开新标签");
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(note.as_path()),
            "当前切到新标签"
        );
        assert!(state.tabs.current().cursor.jump_to.is_some(), "已设跳转");
        assert_eq!(state.tabs.tabs[0].document.path, None, "原标签身份不动");
        assert!(
            state.tabs.tabs[0].editor.text().starts_with("草稿"),
            "草稿保留"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 去抖两段归约:输入变化清结果并顺延计时;到点发起(有根)Running,
    /// 帧末收流落回 Idle 且命中在缓存;无根时到点是复位而非发起。
    #[test]
    fn search_messages_drive_debounce_and_start() {
        let dir = temp_path("search-flow");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.md"), "latermd 独占一行\n没有\n").unwrap();

        let mut state = State::default();
        state.file_tree.root = Some(dir.clone());
        state.search.query = "latermd".into();
        // 预置旧搜索残留:归约必须清掉
        state.search.hits.push(crate::search::SearchResult {
            path: dir.join("stale.md"),
            line_no: 1,
            line_text: "旧根的残留".into(),
            match_span: 0..0,
        });

        state.apply(Message::SearchQueryChanged);
        assert!(state.search.debounce_due.is_some(), "去抖计时已顺延");
        assert!(state.search.hits.is_empty(), "旧结果被清");

        state.apply(Message::SearchRequested);
        assert!(state.search.is_running(), "有根 + 合法模式 → Running");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while state.search.is_running() && std::time::Instant::now() < deadline {
            state.end_of_logic();
            if state.search.is_running() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert_eq!(
            state.search.status,
            crate::search::SearchStatus::Finished,
            "自然结束"
        );
        assert_eq!(state.search.hits.len(), 1);
        assert_eq!(state.search.hits[0].line_no, 1);

        // 无根:到点发起是防御性复位,不进后台
        let mut rootless = State::default();
        rootless.search.query = "latermd".into();
        rootless.apply(Message::SearchQueryChanged);
        rootless.apply(Message::SearchRequested);
        assert!(!rootless.search.is_running());
        assert_eq!(rootless.search.debounce_due, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 换根复位搜索:结果与去抖计时一并丢弃,绝不重发(新根由用户重新输入)。
    #[test]
    fn changing_tree_root_resets_search() {
        let dir = temp_path("search-reset-root");
        std::fs::create_dir_all(&dir).unwrap();
        let settings_dir = temp_path("search-reset-settings");
        let mut state = State {
            settings_dir: Some(settings_dir.clone()),
            ..State::default()
        };
        state.search.query = "latermd".into();
        state.search.debounce_due = Some(std::time::Instant::now());
        state.search.hits.push(crate::search::SearchResult {
            path: dir.join("old.md"),
            line_no: 1,
            line_text: "旧根结果".into(),
            match_span: 0..0,
        });

        state.apply(Message::FileTreeRootSelected(dir.clone()));
        assert_eq!(state.search.debounce_due, None, "去抖计时被清");
        assert!(state.search.hits.is_empty(), "旧根结果被清");
        assert!(!state.search.is_running());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&settings_dir);
    }

    /// 模型列表来源的纯映射(`request_ai_models` 的装配段,单测不出网):
    /// Mock 不联网返回 None;需要 key 的 provider 带凭据,缺失给空串(由
    /// 后台线程回「未配置 API key」);Ollama 无 key;端点原样透传。
    #[test]
    fn models_source_maps_provider_and_key() {
        use latermd_ai::ModelsSource;
        let config = AiConfig {
            provider: crate::ai_config::ProviderKind::Mock,
            ..AiConfig::default()
        };
        assert!(models_source_for(&config, None).is_none(), "Mock 不拉取");

        let config = AiConfig {
            provider: crate::ai_config::ProviderKind::OpenAiCompatible,
            base_url: "https://api.deepseek.com/v1/".to_owned(),
            model: "deepseek-chat".to_owned(),
            context_kb: 0,
        };
        assert_eq!(
            models_source_for(&config, Some("placeholder-key".to_owned())),
            Some(ModelsSource::OpenAiCompatible {
                base_url: "https://api.deepseek.com/v1/".to_owned(),
                api_key: "placeholder-key".to_owned(),
            })
        );
        assert_eq!(
            models_source_for(&config, None),
            Some(ModelsSource::OpenAiCompatible {
                base_url: "https://api.deepseek.com/v1/".to_owned(),
                api_key: String::new(),
            }),
            "key 缺失给空串,不拦发起"
        );

        let config = AiConfig {
            provider: crate::ai_config::ProviderKind::Anthropic,
            base_url: "https://api.anthropic.com".to_owned(),
            model: "claude-sonnet-4-5".to_owned(),
            context_kb: 0,
        };
        assert_eq!(
            models_source_for(&config, Some("placeholder-key".to_owned())),
            Some(ModelsSource::Anthropic {
                base_url: "https://api.anthropic.com".to_owned(),
                api_key: "placeholder-key".to_owned(),
            })
        );

        let config = AiConfig {
            provider: crate::ai_config::ProviderKind::Ollama,
            base_url: "http://127.0.0.1:11434".to_owned(),
            model: "llama3.1".to_owned(),
            context_kb: 0,
        };
        assert_eq!(
            models_source_for(&config, None),
            Some(ModelsSource::Ollama {
                base_url: "http://127.0.0.1:11434".to_owned(),
            }),
            "Ollama 无鉴权,key 有无都不参与"
        );
    }

    /// 「获取模型列表」归约全链路(假装配,零网络):Mock 草稿不发起;
    /// 联网型草稿发起后收流归约填候选、清错误行;失败文案落错误行。
    #[test]
    fn ai_models_fetch_request_gate_and_finish_landing() {
        let mut state = State::default();
        state.ai_key.creds = latermd_creds::Credentials::in_memory();

        // Mock 草稿:不发起(UI 已禁用,归约防御)
        state.apply(Message::AiModelsFetchRequested);
        assert!(!state.settings.models.is_fetching(), "Mock 不发起拉取");

        // OpenAI 兼容草稿 + 假装配(即时回两个模型):发起 → 收流 → 落地
        state.settings.ai_draft.provider = crate::ai_config::ProviderKind::OpenAiCompatible;
        state.settings.models.spawner = std::sync::Arc::new(|_source, tx| {
            std::thread::Builder::new()
                .spawn(move || {
                    let _ = tx.send(Ok(vec!["fake-a".to_owned(), "fake-b".to_owned()]));
                })
                .ok()
        });
        state.apply(Message::AiModelsFetchRequested);
        assert!(state.settings.models.is_fetching(), "联网型草稿发起拉取");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut messages = Vec::new();
        while messages.is_empty() && std::time::Instant::now() < deadline {
            messages = state.poll_models();
            if messages.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        for message in messages {
            state.apply(message);
        }
        assert!(!state.settings.models.is_fetching(), "结果落地后回到空闲");
        assert_eq!(
            state.settings.models.options,
            vec!["fake-a".to_owned(), "fake-b".to_owned()],
            "候选列表填充"
        );
        assert!(state.settings.models.error.is_none(), "成功清错误行");

        // 失败结果:文案落错误行,旧候选保留(上次拉到的列表仍可用)
        state.settings.models.spawner = std::sync::Arc::new(|_source, tx| {
            std::thread::Builder::new()
                .spawn(move || {
                    let _ = tx.send(Err("获取模型列表失败:HTTP 401:bad key".to_owned()));
                })
                .ok()
        });
        state.apply(Message::AiModelsFetchRequested);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut messages = Vec::new();
        while messages.is_empty() && std::time::Instant::now() < deadline {
            messages = state.poll_models();
            if messages.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        for message in messages {
            state.apply(message);
        }
        assert_eq!(
            state.settings.models.error.as_deref(),
            Some("获取模型列表失败:HTTP 401:bad key"),
            "失败文案落错误行"
        );
        assert_eq!(
            state.settings.models.options,
            vec!["fake-a".to_owned(), "fake-b".to_owned()],
            "失败不清旧候选"
        );
    }

    /// AI 流式全链路归约:发起(补空行 + 流式标志)、防重入、chunk 追加、
    /// 收尾。provider 用零间隔,流会瞬时跑完,归约侧按消息逐条喂。
    #[test]
    fn ai_messages_stream_append_and_reentry_guard() {
        let mut state = State::default();
        // 快速 provider:测试不等 3-5 秒的联调节奏
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        let base = state.tabs.current_mut().editor.text().to_owned();

        state.apply(Message::AiStart);
        assert!(state.ai.is_streaming(), "发起后流式标志置位");
        assert!(
            state.tabs.current_mut().editor.text().ends_with("\n\n"),
            "示例文档不以空行结尾,发起时补成空行"
        );

        // 防重入:流式中再次触发被忽略(标志仍置位、只发起了这一个流)
        state.apply(Message::AiStart);
        assert!(state.ai.is_streaming());

        // 收流到自然结束:chunk 追加到末尾,AiDone 收尾清标志
        let mut done = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !done && std::time::Instant::now() < deadline {
            for message in state.poll_ai() {
                if matches!(message, Message::AiDone | Message::AiFailed(_)) {
                    done = true;
                }
                state.apply(message);
            }
            if !done {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert!(done, "流在超时前自然收尾");
        assert!(!state.ai.is_streaming(), "AiDone 归约清流式标志");

        let text = state.tabs.current_mut().editor.text();
        assert!(text.starts_with(&base), "已有内容原样保留在头部");
        assert!(text.len() > base.len() + 2, "AI 文本已追加");
        assert!(
            state.tabs.current_mut().editor.is_dirty(),
            "AI 写入置 dirty"
        );
        assert!(state.tabs.current_mut().editor.revision() > 0);

        // 收尾后可再次发起(防重入不拦新流)
        state.apply(Message::AiStart);
        assert!(state.ai.is_streaming());
        state.ai.finish();
    }

    /// 关键回归(#11 核心不变量):在途 AI 流**绑定发起标签** —— 流式期间
    /// 开新标签,chunk 仍长在发起标签的文档末尾,新标签零污染;收尾清除
    /// 绑定。单标签时代的「换文档作废流」语义随多标签废弃:用户理应能
    /// 边等流式边在别的标签干活。
    #[test]
    fn ai_stream_writes_to_origin_tab_not_active() {
        let mut state = State::default();
        // 慢 provider:保证测试在流自然收尾前完成开新标签(20ms × 30-50 块)
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::from_millis(20),
        ));

        state.apply(Message::AiStart);
        assert!(state.ai.is_streaming());
        let origin_id = state.ai_active_tab.expect("发起时锁定标签 id");
        assert_eq!(origin_id, state.tabs.current().id, "发起标签即当前标签");
        let origin_base = state.tabs.current_mut().editor.text().to_owned();

        // 前置:流确实在产块(此时的块经归约落进发起标签)
        let first_chunk = poll_until_first_ai_chunk(&mut state);
        assert!(!first_chunk.is_empty(), "前置:开新标签前已产出正文块");

        // 开新标签:当前缓冲换走,流不中断、写入目标不改道
        state.apply(Message::FileCommand(FileCmd::New));
        assert_eq!(state.tabs.tabs.len(), 2);
        assert!(state.ai.is_streaming(), "开新标签不作废在途流");
        assert_eq!(state.ai_active_tab, Some(origin_id), "写入目标仍是发起标签");

        // 收流到自然结束:全部 chunk 落发起标签,新标签保持空白
        drain_ai_stream(&mut state);
        assert!(!state.ai.is_streaming());
        assert_eq!(state.ai_active_tab, None, "收尾清除发起标签绑定");
        assert!(
            state.tabs.tabs[0].editor.text().len() > origin_base.len(),
            "发起标签吃到全部续写"
        );
        assert!(
            state.tabs.tabs[0].editor.is_dirty(),
            "AI 写入置发起标签 dirty"
        );
        assert_eq!(
            state.tabs.tabs[1].editor.text(),
            "",
            "新标签零污染(这正是旧语义要防的「chunk 写错文档」)"
        );
    }

    /// AiChunk 追加语义:精确接在文档末尾;空 delta 是无操作(不推进修订号)。
    /// 真实链路里 chunk 只来自在途流(`poll_ai`),故先发起建立标签绑定。
    #[test]
    fn ai_chunk_appends_at_end_and_empty_delta_is_noop() {
        let mut state = State::default();
        state.apply(Message::AiStart);
        assert_eq!(
            state.ai_active_tab,
            Some(state.tabs.current().id),
            "前置:发起已锁定写入目标"
        );
        state.apply(Message::AiChunk {
            delta: "续写".into(),
        });
        assert!(state.tabs.current_mut().editor.text().ends_with("续写"));
        assert!(state.tabs.current_mut().editor.is_dirty());

        let (rev, len) = (
            state.tabs.current_mut().editor.revision(),
            state.tabs.current_mut().editor.len_chars(),
        );
        state.apply(Message::AiChunk {
            delta: String::new(),
        });
        assert_eq!(
            state.tabs.current_mut().editor.revision(),
            rev,
            "空 delta 不推进修订号"
        );
        assert_eq!(state.tabs.current_mut().editor.len_chars(), len);
    }

    /// 发起归约的空行补齐矩阵:空文档不动、单换行补一个、空行结尾不动、
    /// 行中结尾补两个(让 AI 首行独立成段)。
    #[test]
    fn ai_start_normalizes_trailing_blank_line() {
        let cases: [(&str, usize); 4] = [
            ("", 0),       // 空文档不动
            ("甲\n", 1),   // 单换行 → 补一个变空行
            ("甲\n\n", 0), // 已是空行结尾
            ("甲乙", 2),   // 行中 → 补两个
        ];
        for (initial, expected_pad) in cases {
            let mut state = State::default();
            state.tabs.current_mut().editor.load(initial);
            state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
                std::time::Duration::ZERO,
            ));
            state.start_ai_stream();
            let expected = format!("{initial}{}", "\n".repeat(expected_pad));
            assert!(
                state
                    .tabs
                    .current_mut()
                    .editor
                    .text()
                    .starts_with(&expected),
                "初始 {initial:?} 的补行结果应为 {expected:?},实际 {:?}",
                &state.tabs.current_mut().editor.text()[..expected.len()]
            );
            state.ai.finish();
        }
    }

    /// AiFailed 归约:清流式标志 + 错误描述进提示行,文档不被触碰。
    #[test]
    fn ai_failed_finishes_stream_and_lands_in_notice() {
        let mut state = State::default();
        state.apply(Message::AiStart);
        assert!(state.ai.is_streaming());
        let before = state.tabs.current_mut().editor.text().to_owned();

        state.apply(Message::AiFailed("额度用尽".into()));
        assert!(!state.ai.is_streaming(), "失败同样收尾");
        assert_eq!(
            state.tabs.current().document.notice.as_deref(),
            Some("额度用尽")
        );
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            before,
            "失败文本不写入文档"
        );
        // 失败不算完成:指令卡的状态键一并清空(卡片回「未执行」)
        assert_eq!(state.ai.last_prompt, None);
        assert_eq!(state.ai_active_tab, None, "失败清除发起标签绑定");

        // 多标签回归:失败提示属于发起流的标签,不属于此刻的 active
        state.spawn_tab(None, "第二篇");
        state.apply(Message::TabActivate(0));
        state.apply(Message::AiStart);
        assert_eq!(state.ai_active_tab, Some(state.tabs.tabs[0].id));
        state.apply(Message::TabActivate(1));
        state.apply(Message::AiFailed("跨标签失败".into()));
        assert_eq!(
            state.tabs.tabs[0].document.notice.as_deref(),
            Some("跨标签失败"),
            "提示落在发起标签"
        );
        assert!(
            state.tabs.tabs[1].document.notice.is_none(),
            "此刻的 active 标签不被打扰"
        );
    }

    /// ai:// 链接点击归约:Ok(prompt) 复用流式启动路径(补空行 + 发起),
    /// chunk 照常归约追加;流式进行中再点击被防重入忽略(连补空行都不发生,
    /// 与 AiStart 的入口检查同一处);Err 落提示行且不动流式生命周期。
    #[test]
    fn ai_link_clicked_streams_with_link_prompt_and_reentry_is_ignored() {
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        let base = state.tabs.current_mut().editor.text().to_owned();

        state.apply(Message::AiLinkClicked {
            prompt: Ok("续写一段 Markdown 介绍".into()),
        });
        assert!(state.ai.is_streaming(), "链接点击发起了流");
        assert!(
            state.tabs.current_mut().editor.text().ends_with("\n\n"),
            "发起时补成空行结尾,与 AiStart 同语义"
        );

        let mut done = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !done && std::time::Instant::now() < deadline {
            for message in state.poll_ai() {
                if matches!(message, Message::AiDone | Message::AiFailed(_)) {
                    done = true;
                }
                state.apply(message);
            }
            if !done {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert!(done, "流在超时前自然收尾");
        assert!(!state.ai.is_streaming());
        assert!(
            state.tabs.current_mut().editor.text().len() > base.len() + 2,
            "AI 文本已追加"
        );
        assert!(
            state.tabs.current_mut().editor.is_dirty(),
            "AI 写入置 dirty"
        );
        // 状态键:自然收尾保留最近 prompt,指令卡凭它显示「已完成」
        assert_eq!(
            state.ai.last_prompt.as_deref(),
            Some("续写一段 Markdown 介绍")
        );

        // 防重入:慢 provider 发起后再点链接,忽略且不补空行
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::from_millis(50),
        ));
        state.apply(Message::AiStart);
        let streaming_text = state.tabs.current_mut().editor.text().to_owned();
        state.apply(Message::AiLinkClicked {
            prompt: Ok("流式中再来一次".into()),
        });
        assert!(state.ai.is_streaming());
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            streaming_text,
            "流式中点击链接被忽略:文档一字未动"
        );
        state.ai.finish();

        // Err:未实现动作 / 解析失败的提示语直接落状态栏,不发起、不碰文档
        let mut idle = State::default();
        idle.apply(Message::AiLinkClicked {
            prompt: Err("未实现的 AI 动作:summarize".into()),
        });
        assert!(!idle.ai.is_streaming(), "解析失败不发起流");
        assert_eq!(
            idle.tabs.current().document.notice.as_deref(),
            Some("未实现的 AI 动作:summarize")
        );
    }

    /// 收流到结束(自然收尾**或**陈旧中止):流式标志清零即停。与
    /// [`drain_ai_stream`] 的差别在退出条件 —— 陈旧中止不产 AiDone/
    /// AiFailed(块被静默丢弃),按标志判停才不会空转到超时。
    fn drain_ai_until_idle(state: &mut State) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while state.ai.is_streaming() && std::time::Instant::now() < deadline {
            for message in state.poll_ai() {
                state.apply(message);
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!state.ai.is_streaming(), "流已收尾(自然或中止)");
    }

    /// 选区续写(M2)Mock 流端到端:点「续写」→ 捕获选区尾插入点(修订
    /// 号见证)→ 流逐块精确落在插入点 → 收尾清绑定。本例选区在段中
    /// (「中点」),前后文必须分毫不动;选中文本进 prompt 一并断言。
    #[test]
    fn selection_ai_continue_streams_at_selection_tail_mid_document() {
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        // 「开头一句。」= 5 字符,选区 (5,11) = 「选中的句子。」,尾 = 11
        state
            .tabs
            .current_mut()
            .editor
            .load("开头一句。选中的句子。结尾一句。");

        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Continue,
            selection: Some((5, 11)),
        });
        assert!(state.ai.is_streaming(), "点续写发起流");
        assert_eq!(
            state.selection_ai_last,
            Some(SelectionAiAction::Continue),
            "M1 记账保留(链路通路断言)"
        );
        let insert = state.ai_insert_at.expect("选区续写挂插入点");
        assert_eq!(
            insert.offset, 13,
            "选区尾 11 + 锚定补段空行 2(尾前字符非换行)"
        );
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            "开头一句。选中的句子。\n\n\n\n结尾一句。",
            "发起时在选区尾两侧补段空行(前二后二),余下文本原样后移"
        );
        assert_eq!(
            insert.rev,
            state.tabs.current_mut().editor.revision(),
            "见证修订号 = 补行后的缓冲修订号"
        );
        let prompt = state.ai.last_prompt.clone().expect("发起记录 prompt");
        assert!(
            prompt.contains("选中的句子。"),
            "选中文本进 prompt(实测 {prompt:?})"
        );

        drain_ai_stream(&mut state);
        assert_eq!(state.ai_active_tab, None, "收尾清标签绑定");
        assert_eq!(state.ai_insert_at, None, "收尾清插入点");
        let text = state.tabs.current_mut().editor.text().to_owned();
        let middle = text
            .strip_prefix("开头一句。选中的句子。\n\n")
            .and_then(|rest| rest.strip_suffix("\n\n结尾一句。"))
            .expect("续写精确落在选区尾,前后文分毫不动");
        assert!(!middle.is_empty(), "AI 文本已插入缝中");
        assert!(
            state.tabs.current_mut().editor.is_dirty(),
            "AI 写入置 dirty"
        );
        assert!(
            state.tabs.current().document.notice.is_none(),
            "干净完成无提示"
        );
    }

    /// 文末前(尾后仍有正文)与纯 CJK 选区:补行矩阵与字符偏移边界 ——
    /// CJK 文档按字符计偏移,补行/插入都不截半字符。
    #[test]
    fn selection_ai_continue_tail_before_end_and_cjk_offsets() {
        // 文末前:选区 (0,4) = 「甲乙丙丁」,尾 = 4,尾后还有「\n结尾」
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        state.tabs.current_mut().editor.load("甲乙丙丁\n结尾");
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Continue,
            selection: Some((0, 4)),
        });
        let insert = state.ai_insert_at.expect("挂插入点");
        assert_eq!(insert.offset, 6, "尾 4 + 前侧补段空行 2");
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            "甲乙丙丁\n\n\n\n结尾",
            "尾后单个换行再补 1 凑成空行(后侧共 2),插入点两侧各隔一个空行"
        );
        drain_ai_stream(&mut state);
        let text = state.tabs.current_mut().editor.text().to_owned();
        let middle = text
            .strip_prefix("甲乙丙丁\n\n")
            .and_then(|rest| rest.strip_suffix("\n\n结尾"))
            .expect("文末前:AI 落选区尾与既有尾段之间,两侧各隔空行");
        assert!(!middle.is_empty());

        // 纯 CJK:选区 (0,4) = 「你好世界」,尾字符是「,」→ 补 2;
        // 偏移全按字符(char_to_byte 恒在边界),结果不截半
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        state.tabs.current_mut().editor.load("你好世界,这是尾句");
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Continue,
            selection: Some((0, 4)),
        });
        let insert = state.ai_insert_at.expect("挂插入点");
        assert_eq!(insert.offset, 6, "尾 4 + 补段空行 2");
        drain_ai_stream(&mut state);
        let text = state.tabs.current_mut().editor.text().to_owned();
        let middle = text
            .strip_prefix("你好世界\n\n")
            .and_then(|rest| rest.strip_suffix("\n\n,这是尾句"))
            .expect("CJK:AI 落在字符边界,两侧补行隔开既有文本,逗号与尾句原样保留");
        assert!(!middle.is_empty());
    }

    /// 选区一路选到文档末 = 「文末」:行为与既有文档末续写(AiStart)完全
    /// 等价 —— 同一补行矩阵、同一落点、同一段 Mock 文本;存量路径自身
    /// 不挂插入点(否决线:零行为变化)。
    #[test]
    fn selection_ai_continue_at_document_end_matches_legacy_path() {
        let mut selected = State::default();
        let mut legacy = State::default();
        for state in [&mut selected, &mut legacy] {
            state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
                std::time::Duration::ZERO,
            ));
            state.tabs.current_mut().editor.load("正文一段");
        }
        selected.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Continue,
            selection: Some((0, 4)),
        });
        assert_eq!(
            selected.ai_insert_at.map(|point| point.offset),
            Some(6),
            "尾 4 = 文档末,补段空行 2 后落点 6"
        );
        legacy.apply(Message::AiStart);
        assert_eq!(
            legacy.ai_insert_at, None,
            "存量文档末路径不挂插入点(否决线)"
        );

        drain_ai_stream(&mut selected);
        drain_ai_stream(&mut legacy);
        assert_eq!(
            selected.tabs.current_mut().editor.text(),
            legacy.tabs.current_mut().editor.text(),
            "选到文末的续写结果与 AiStart 逐字符一致(两路 prompt 都无脚本关键词,Mock 同文)"
        );
    }

    /// 无选区/塌缩选区(防御分支:浮标只在有选区时在场,消息仍可能被
    /// 直接触发)退回文档末续写:动作语义不变,落点回退,不挂插入点
    /// (decisions-pending #116)。
    #[test]
    fn selection_ai_continue_without_selection_falls_back_to_document_end() {
        for selection in [None, Some((3, 3))] {
            let mut state = State::default();
            state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
                std::time::Duration::ZERO,
            ));
            state.tabs.current_mut().editor.load("三字标题");
            state.apply(Message::SelectionAiActionRequested {
                action: SelectionAiAction::Continue,
                selection,
            });
            assert!(state.ai.is_streaming(), "selection={selection:?} 仍发起");
            assert_eq!(
                state.ai_insert_at, None,
                "selection={selection:?} 回退文档末,不挂插入点"
            );
            assert!(
                state
                    .tabs
                    .current_mut()
                    .editor
                    .text()
                    .starts_with("三字标题\n\n"),
                "selection={selection:?} 按文档末口径补行"
            );
            state.ai.finish();
        }
    }

    /// 插入点随流推进:手动喂 AiChunk 走同一归约(不收真实流),逐块断言
    /// 偏移按 delta 字符数推进、见证修订号随自身写入刷新(否则第二块会被
    /// 误判陈旧);空 delta 无操作。
    #[test]
    fn selection_ai_insert_point_advances_chunk_by_chunk() {
        let mut state = State::default();
        state.tabs.current_mut().editor.load("ONE\nTWO");
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Continue,
            selection: Some((0, 3)),
        });
        assert_eq!(
            state.ai_insert_at.unwrap().offset,
            5,
            "尾 3 + 前侧补段空行 2"
        );
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            "ONE\n\n\n\nTWO",
            "尾后原有单个换行再补 1 凑成空行:插入点两侧各隔一个空行"
        );

        state.apply(Message::AiChunk {
            delta: "甲".into()
        });
        assert_eq!(state.tabs.current_mut().editor.text(), "ONE\n\n甲\n\nTWO");
        assert_eq!(state.ai_insert_at.unwrap().offset, 6, "推进 delta 字符数");
        assert_eq!(
            state.ai_insert_at.unwrap().rev,
            state.tabs.current_mut().editor.revision(),
            "见证随自身写入刷新"
        );

        state.apply(Message::AiChunk {
            delta: "乙丙".into(),
        });
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            "ONE\n\n甲乙丙\n\nTWO"
        );
        state.apply(Message::AiChunk {
            delta: "丁".into()
        });
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            "ONE\n\n甲乙丙丁\n\nTWO",
            "第三块照常写入:上一块刷新过的见证有效"
        );

        let (rev, offset) = {
            let tab = state.tabs.current_mut();
            (
                tab.editor.revision(),
                state.ai_insert_at.map(|point| point.offset).unwrap(),
            )
        };
        state.apply(Message::AiChunk {
            delta: String::new(),
        });
        assert_eq!(
            state.tabs.current_mut().editor.revision(),
            rev,
            "空 delta 不推进修订号"
        );
        assert_eq!(
            state.ai_insert_at.unwrap().offset,
            offset,
            "空 delta 不推进插入点"
        );
        state.ai.finish();
    }

    /// 选区续写期间的 #11 不变量:流式进行中开新标签,chunk 仍按插入点
    /// 落在发起标签,新标签零污染,插入点绑定不动(与文档末路径的关键
    /// 回归 `ai_stream_writes_to_origin_tab_not_active` 同型)。
    #[test]
    fn selection_ai_stream_writes_to_origin_tab_at_insert_point() {
        let mut state = State::default();
        state.tabs.current_mut().editor.load("ONE\nTWO");
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Continue,
            selection: Some((0, 3)),
        });
        let origin_id = state.ai_active_tab.expect("发起锁定标签");
        let insert_before = state.ai_insert_at;

        state.apply(Message::FileCommand(FileCmd::New));
        assert_eq!(state.ai_active_tab, Some(origin_id), "切标签不改写入目标");
        assert_eq!(state.ai_insert_at, insert_before, "切标签不动插入点");

        state.apply(Message::AiChunk {
            delta: "甲".into()
        });
        assert_eq!(
            state.tabs.tabs[0].editor.text(),
            "ONE\n\n甲\n\nTWO",
            "chunk 仍落发起标签的插入点"
        );
        assert_eq!(state.tabs.tabs[1].editor.text(), "", "新标签零污染");
        state.ai.finish();
    }

    /// 陈旧防御(#61 M2):流进行中用户编辑 → 缓冲修订号偏离见证 → 下一块
    /// 到达即作废流并落提示,**不写任何内容到漂移后的位置**(钳到文档末是
    /// 静默写错位置,decisions-pending #116 选了中止)。
    #[test]
    fn selection_ai_continue_aborts_when_document_edited_mid_stream() {
        let mut state = State::default();
        // 慢 provider:保证编辑发生在任何块被归约之前
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::from_millis(20),
        ));
        state
            .tabs
            .current_mut()
            .editor
            .load("开头一句。选中的句子。结尾一句。");
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Continue,
            selection: Some((5, 11)),
        });
        assert!(state.ai.is_streaming());

        // 用户在流中编辑:文首插入挪动后续一切,插入点偏移全部漂移
        let edited = {
            let tab = state.tabs.current_mut();
            tab.editor.insert_chars(0, "用户新增");
            tab.editor.text().to_owned()
        };
        drain_ai_until_idle(&mut state);

        assert_eq!(
            state.tabs.current_mut().editor.text(),
            edited,
            "陈旧中止:块一个都没落,文档停在用户编辑后的样子"
        );
        assert_eq!(
            state.tabs.current().document.notice.as_deref(),
            Some("文档在续写期间被编辑,插入点已失效,续写已停止"),
            "中止提示落发起标签"
        );
        assert_eq!(state.ai_active_tab, None, "作废清标签绑定");
        assert_eq!(state.ai_insert_at, None, "作废清插入点");
        assert_eq!(state.ai.last_prompt, None, "作废清指令卡状态键");
    }

    /// 选区续写的闸门矩阵:key 闸门(provider 需 key 且凭据缺失)拦下时
    /// 零副作用 —— 不发起、不补行、落「未配置 key」提示;流式进行中再点
    /// 续写被防重入忽略(不二次补行、不顶掉在途流)。
    #[test]
    fn selection_ai_continue_respects_key_gate_and_reentry() {
        std::env::remove_var(latermd_creds::API_KEY_ENV);
        let mut state = State::default();
        state.ai_key.creds = latermd_creds::Credentials::in_memory();
        state.ai.set_provider(
            AiConfig {
                provider: crate::ai_config::ProviderKind::OpenAiCompatible,
                base_url: "https://api.deepseek.com/v1".to_owned(),
                model: "deepseek-chat".to_owned(),
                context_kb: 0,
            },
            None,
        );
        assert!(
            state.ai_key.creds.ai_api_key().is_none(),
            "前置:内存凭据与环境变量都无 key"
        );
        state.tabs.current_mut().editor.load("甲乙");
        let (text, rev) = {
            let tab = state.tabs.current();
            (tab.editor.text().to_owned(), tab.editor.revision())
        };

        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Continue,
            selection: Some((0, 2)),
        });
        assert!(!state.ai.is_streaming(), "无 key 不发起");
        assert_eq!(
            state.tabs.current().document.notice.as_deref(),
            Some("未配置 API key(设置 → AI Provider)"),
            "拦截文案与其它 AI 入口同源"
        );
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            text,
            "被拦命令零副作用:文档一字未动"
        );
        assert_eq!(
            state.tabs.current_mut().editor.revision(),
            rev,
            "补行也未发生"
        );

        // 换 Mock(无需 key)发起,再点续写:防重入忽略,不二次补行
        state.ai.set_provider(AiConfig::default(), None);
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Continue,
            selection: Some((0, 2)),
        });
        assert!(state.ai.is_streaming());
        let after_start = state.tabs.current_mut().editor.text().to_owned();
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Continue,
            selection: Some((0, 2)),
        });
        assert!(state.ai.is_streaming(), "在途流不被顶掉");
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            after_start,
            "流式中再点被忽略:文档(含首次补行)不再变动"
        );
        state.ai.finish();
    }

    /// 选区润色(M3)Mock 流端到端:点「润色」→ 选中文本进 prompt、
    /// 发起润色会话 → chunk 逐块进浮窗草稿(**文档一字不动**)→ 收尾后
    /// 会话留在待确认态,草稿与 `MockProvider::mock_polish` 的产物逐字符
    /// 一致(浮窗正文就是这份草稿,UI 侧链路见 `ui::layout` 的浮窗测试)。
    #[test]
    fn selection_ai_polish_streams_into_draft_and_waits_confirmation() {
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        // 「开头一句。」= 5 字符,选区 (5,11) = 「选中的句子。」
        state
            .tabs
            .current_mut()
            .editor
            .load("开头一句。选中的句子。结尾一句。");
        let before = state.tabs.current_mut().editor.text().to_owned();
        let rev_before = state.tabs.current().editor.revision();

        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((5, 11)),
        });
        assert!(state.ai.is_streaming(), "点润色发起流");
        assert_eq!(state.selection_ai_last, Some(SelectionAiAction::Polish));
        let session = state.ai_polish.clone().expect("挂润色会话");
        assert_eq!(session.selection, (5, 11), "会话记住发起帧选区");
        assert_eq!(session.rev, rev_before, "见证 = 发起帧修订号(发起不补行)");
        assert_eq!(
            state.ai_insert_at, None,
            "润色与续写插入点互斥(不写文档路径)"
        );
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            before,
            "发起不改文档(不补行)"
        );
        let prompt = state.ai.last_prompt.clone().expect("发起记录 prompt");
        assert!(
            prompt.starts_with(
                latermd_ai::polish_prompt("")
                    .trim_end_matches('\n')
                    .trim_end()
            ),
            "prompt 以润色指令头开头(实测 {prompt:?})"
        );

        // 流式中 chunk 进草稿,文档分毫不动
        drain_ai_stream(&mut state);
        let session = state.ai_polish.clone().expect("收尾后会话保留(待确认)");
        assert!(!state.ai.is_streaming(), "流已收尾");
        assert_eq!(
            state.ai_active_tab, None,
            "收尾清标签绑定(会话自己带 tab_id)"
        );
        assert_eq!(
            session.draft,
            latermd_ai::MockProvider::new().mock_polish(&prompt),
            "浮窗草稿 == Mock 润色产物(端到端)"
        );
        assert!(!session.draft.is_empty(), "草稿非空");
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            before,
            "全程文档一字未动(待确认才谈替换)"
        );
        assert!(state.tabs.current().document.notice.is_none(), "干净收尾");
    }

    /// 确认替换(#61 M3):整段替换选区为润色结果 —— 内容逐字符、新选区
    /// 落在结果全文、dirty 置位;随后「放弃」消息是防御 no-op(会话已清)。
    #[test]
    fn selection_ai_polish_confirm_replaces_selection_exactly() {
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        state
            .tabs
            .current_mut()
            .editor
            .load("开头。选中的  句子。。结尾。");
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((3, 12)),
        });
        drain_ai_stream(&mut state);
        let polished = state.ai_polish.clone().expect("待确认会话").draft;
        assert_ne!(polished, "选中的  句子。。", "Mock 确实清理了缺陷文本");

        state.apply(Message::SelectionAiPolishConfirmed);
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            format!("开头。{polished}结尾。"),
            "整段替换,前后文分毫不动"
        );
        assert_eq!(
            state.tabs.current().pending_selection,
            Some((3, 3 + polished.chars().count())),
            "新选区落润色结果全文(字符口径)"
        );
        assert_eq!(state.ai_polish, None, "确认即关窗");
        assert!(
            state.tabs.current_mut().editor.is_dirty(),
            "替换置 dirty(与手敲同责)"
        );

        // 会话已清后的确认/放弃是防御 no-op
        let text = state.tabs.current_mut().editor.text().to_owned();
        state.apply(Message::SelectionAiPolishConfirmed);
        state.apply(Message::SelectionAiPolishDismissed);
        assert_eq!(state.tabs.current_mut().editor.text(), text);
    }

    /// 放弃零改动(#61 M3):待确认放弃 = 关窗,文档/修订号/dirty/选区
    /// 一概不动;流式中放弃 = 作废在途流,剩余 chunk 丢弃、文档仍不动。
    #[test]
    fn selection_ai_polish_dismiss_leaves_document_untouched() {
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        state
            .tabs
            .current_mut()
            .editor
            .load("开头。选中的句子。结尾。");
        let (before, rev_before) = {
            let tab = state.tabs.current();
            (tab.editor.text().to_owned(), tab.editor.revision())
        };

        // 流式中放弃:作废流,后续 chunk 不进草稿
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((3, 9)),
        });
        assert!(state.ai.is_streaming());
        state.apply(Message::SelectionAiPolishDismissed);
        assert!(!state.ai.is_streaming(), "在途流已作废");
        assert_eq!(state.ai_polish, None, "会话已清");
        assert_eq!(state.ai_active_tab, None);
        assert_eq!(state.ai.last_prompt, None, "作废清指令卡状态键");
        // 作废后残余 chunk 到达:无会话无绑定,防御性作废,文档不动
        state.apply(Message::AiChunk {
            delta: "残余块".into(),
        });
        let tab = state.tabs.current();
        assert_eq!(tab.editor.text(), before, "放弃后文档一字未动");
        assert_eq!(tab.editor.revision(), rev_before);
        assert!(!tab.editor.is_dirty());
        assert_eq!(tab.pending_selection, None, "选区不动");
        assert_eq!(state.ai_polish, None);
    }

    /// 待确认放弃:浮窗开着(流已收尾)直接放弃 —— 与流式中放弃不同,
    /// 没有流可作废,纯关窗;文档与修订号纹丝不动。
    #[test]
    fn selection_ai_polish_dismiss_after_done_is_pure_close() {
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        state
            .tabs
            .current_mut()
            .editor
            .load("开头。选中的句子。结尾。");
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((3, 9)),
        });
        drain_ai_stream(&mut state);
        assert!(state.ai_polish.is_some());
        let (before, rev_before, dirty_before) = {
            let tab = state.tabs.current();
            (
                tab.editor.text().to_owned(),
                tab.editor.revision(),
                tab.editor.is_dirty(),
            )
        };

        state.apply(Message::SelectionAiPolishDismissed);
        assert_eq!(state.ai_polish, None, "会话清空");
        let tab = state.tabs.current();
        assert_eq!(tab.editor.text(), before, "文档一字未动");
        assert_eq!(tab.editor.revision(), rev_before, "修订号不前进");
        assert_eq!(tab.editor.is_dirty(), dirty_before, "dirty 不变");
        assert_eq!(tab.pending_selection, None);
    }

    /// 陈旧见证(#61 M3,照 #17 口径):浮窗期间文档被编辑 → 确认被拦,
    /// 提示落发起标签,**绝不按过期坐标盲写**;会话作废(重选重润重来)。
    #[test]
    fn selection_ai_polish_confirm_rejected_when_document_edited_mid_window() {
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        state
            .tabs
            .current_mut()
            .editor
            .load("开头。选中的句子。结尾。");
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((3, 9)),
        });
        drain_ai_stream(&mut state);
        assert!(state.ai_polish.is_some());

        // 浮窗期不锁编辑器:用户在文首插入,选区坐标全部漂移
        let edited = {
            let tab = state.tabs.current_mut();
            tab.editor.insert_chars(0, "用户新增");
            tab.editor.text().to_owned()
        };

        state.apply(Message::SelectionAiPolishConfirmed);
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            edited,
            "陈旧拦截:替换一个字都没落,文档停在用户编辑后的样子"
        );
        assert_eq!(
            state.tabs.current().document.notice.as_deref(),
            Some("文档在润色期间被编辑,选区位置已失效,替换已取消"),
        );
        assert_eq!(state.ai_polish, None, "拦截即作废会话(不盲写也不留窗)");
    }

    /// CJK + emoji 选区(#61 M3):选区含多字节字符,发起(prompt 组装)、
    /// 草稿追加、确认替换全链路按字符偏移走,不截半字符;替换后文档仍是
    /// 合法 UTF-8 且前后文原样。
    #[test]
    fn selection_ai_polish_cjk_emoji_selection_survives_roundtrip() {
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        // 「开头。」= 3 字符;选区 (3,11) = 「好🦀了  吧。。」(8 字符,🦀 占
        // 1 字符 4 字节);文档 = 3 + 8 + 3 = 14 字符
        state
            .tabs
            .current_mut()
            .editor
            .load("开头。好🦀了  吧。。结尾。");
        assert_eq!(state.tabs.current().editor.len_chars(), 14);
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((11, 3)),
        });
        let session = state.ai_polish.clone().expect("反向选区同样发起");
        assert_eq!(session.selection, (3, 11), "选区有序化");
        let prompt = state.ai.last_prompt.clone().expect("prompt 记录");
        assert!(
            prompt.contains("好🦀了"),
            "多字节素材完整进 prompt(实测 {prompt:?})"
        );
        drain_ai_stream(&mut state);
        let polished = state.ai_polish.clone().expect("待确认").draft;

        state.apply(Message::SelectionAiPolishConfirmed);
        let text = state.tabs.current_mut().editor.text().to_owned();
        assert_eq!(
            text,
            format!("开头。{polished}结尾。"),
            "CJK/emoji 选区整段替换不截半"
        );
        // 替换后 UTF-8 仍合法(String 本身即证明);前后文逐字符原样
        assert!(text.starts_with("开头。") && text.ends_with("结尾。"));
        assert_eq!(
            state.tabs.current().pending_selection,
            Some((3, 3 + polished.chars().count())),
            "新选区按字符口径落结果"
        );
    }

    /// 错误路径(#61 M3):塌缩/空白选区与超上下文上限一律落显文案、
    /// 零副作用(不发流、不挂会话、不动文档)。
    #[test]
    fn selection_ai_polish_error_paths_notice_without_side_effects() {
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        state
            .tabs
            .current_mut()
            .editor
            .load("开头。中文素材。结尾。");
        let (before, rev_before) = {
            let tab = state.tabs.current();
            (tab.editor.text().to_owned(), tab.editor.revision())
        };

        // 塌缩选区(防御:浮标只在有选区时在场)
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((4, 4)),
        });
        assert_eq!(
            state.tabs.current().document.notice.as_deref(),
            Some("请先选中要润色的文本"),
        );
        assert!(!state.ai.is_streaming());
        assert_eq!(state.ai_polish, None);

        // 全空白选区(空格 + 换行):素材校验拒绝
        let mut blank = State::default();
        blank.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        blank.tabs.current_mut().editor.load("  \n\n素材。");
        blank.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((0, 4)),
        });
        assert_eq!(
            blank.tabs.current().document.notice.as_deref(),
            Some("选中的内容是空白,没有可润色的文本"),
        );
        assert!(!blank.ai.is_streaming());
        assert_eq!(blank.ai_polish, None);

        // 超上下文上限(#58 预算):配置 1KB,素材 3KB+ 拒绝
        let mut big = State::default();
        big.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        big.ai_key.creds = latermd_creds::Credentials::in_memory();
        big.ai.set_provider(
            AiConfig {
                provider: crate::ai_config::ProviderKind::Mock,
                base_url: String::new(),
                model: String::new(),
                context_kb: 1,
            },
            None,
        );
        let long_tail = "尾部哨兵";
        let long = format!("{}{long_tail}", "字".repeat(2000));
        big.tabs.current_mut().editor.load(&long);
        big.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((0, big.tabs.current().editor.len_chars())),
        });
        assert_eq!(
            big.tabs.current().document.notice.as_deref(),
            Some("选中文本超过润色上限(约 1KB),请缩短后重试"),
            "预算读「上下文大小」配置,超限拒绝不截断"
        );
        assert!(!big.ai.is_streaming());
        assert_eq!(big.ai_polish, None);
        assert_eq!(
            big.tabs.current_mut().editor.text(),
            long,
            "零副作用:文档原样(素材绝不被截断)"
        );

        // 三条错误路径都不推进修订号
        assert_eq!(state.tabs.current_mut().editor.revision(), rev_before);
        assert_eq!(state.tabs.current_mut().editor.text(), before);
    }

    /// key 闸门与防重入(#61 M3,照 M2 同源矩阵):provider 需 key 且凭据
    /// 缺失 → 拦截文案零副作用;流式进行中(含续写在途)再点润色被忽略;
    /// 待确认浮窗在场发起新流被顶掉(防 chunk 误分进旧草稿)。
    #[test]
    fn selection_ai_polish_respects_key_gate_reentry_and_session_swap() {
        std::env::remove_var(latermd_creds::API_KEY_ENV);
        let mut state = State::default();
        state.ai_key.creds = latermd_creds::Credentials::in_memory();
        state.ai.set_provider(
            AiConfig {
                provider: crate::ai_config::ProviderKind::OpenAiCompatible,
                base_url: "https://api.deepseek.com/v1".to_owned(),
                model: "deepseek-chat".to_owned(),
                context_kb: 0,
            },
            None,
        );
        state.tabs.current_mut().editor.load("甲乙丙丁");
        let (text, rev) = {
            let tab = state.tabs.current();
            (tab.editor.text().to_owned(), tab.editor.revision())
        };

        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((0, 2)),
        });
        assert!(!state.ai.is_streaming(), "无 key 不发起");
        assert_eq!(
            state.tabs.current().document.notice.as_deref(),
            Some("未配置 API key(设置 → AI Provider)"),
            "拦截文案与其它 AI 入口同源"
        );
        assert_eq!(state.ai_polish, None);
        let tab = state.tabs.current();
        assert_eq!(tab.editor.text(), text, "零副作用");
        assert_eq!(tab.editor.revision(), rev);

        // 流式进行中(续写在途)再点润色:防重入忽略,不顶掉在途流
        state.ai.set_provider(AiConfig::default(), None);
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Continue,
            selection: Some((0, 2)),
        });
        assert!(state.ai.is_streaming());
        assert_eq!(state.ai_insert_at.map(|p| p.offset), Some(4));
        let (text, insert) = (
            state.tabs.current_mut().editor.text().to_owned(),
            state.ai_insert_at,
        );
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((0, 2)),
        });
        assert!(state.ai.is_streaming(), "在途续写不被顶掉");
        assert_eq!(state.ai_insert_at, insert, "插入点不动");
        assert_eq!(state.ai_polish, None, "润色没有发起");
        assert_eq!(state.tabs.current_mut().editor.text(), text);
        state.abort_ai_stream();

        // 待确认浮窗在场发起新润色:旧会话被顶掉(新会话接管浮窗),
        // 旧草稿随之丢弃(纯 UI 状态)。选区取「丙丁」(续写补行后的文档
        // 是「甲乙\n\n\n\n丙丁」,中段是空白素材)
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((0, 2)),
        });
        assert_eq!(state.ai_polish.as_ref().map(|s| s.selection), Some((0, 2)));
        drain_ai_stream(&mut state);
        assert!(state.ai_polish.is_some(), "第一个会话待确认");
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((6, 8)),
        });
        assert!(
            state.ai_polish.as_ref().is_some_and(|s| s.draft.is_empty()),
            "新会话顶掉旧会话(草稿清零重来)"
        );
        drain_ai_stream(&mut state);
        assert_ne!(
            state.ai_polish.as_ref().unwrap().selection,
            (0, 2),
            "浮窗归属新选区"
        );
        state.abort_ai_stream();
    }

    /// 流失败(#61 M3):润色流失败 → 浮窗关闭、半截草稿不留,错误文案
    /// 落发起标签(切走标签后失败仍落发起方)。
    #[test]
    fn selection_ai_polish_failed_closes_window_and_reports_to_origin_tab() {
        let mut state = State::default();
        state.tabs.current_mut().editor.load("开头。素材。结尾。");
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((3, 5)),
        });
        assert!(state.ai_polish.is_some());
        state.apply(Message::AiChunk {
            delta: "半截".into(),
        });
        state.apply(Message::AiFailed("端点超时".into()));
        assert!(!state.ai.is_streaming());
        assert_eq!(state.ai_polish, None, "失败关浮窗,半截草稿不留");
        assert_eq!(
            state.tabs.current().document.notice.as_deref(),
            Some("端点超时"),
        );
    }

    /// 润色结果为空(#61 M3):收尾草稿全空白 → 开不出可确认的浮窗,
    /// 会话作废、文案落**发起标签**(绑定清除前定位,切走标签也落对)。
    #[test]
    fn selection_ai_polish_empty_result_closes_window_and_reports_to_origin_tab() {
        let mut state = State::default();
        state.tabs.current_mut().editor.load("开头。素材。结尾。");
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((3, 5)),
        });
        assert!(state.ai_polish.is_some());
        // 切到新标签再收尾:空结果提示必须落发起标签,不是当前标签
        state.apply(Message::FileCommand(FileCmd::New));
        state.apply(Message::AiChunk {
            delta: "  \n".into(),
        });
        state.apply(Message::AiDone);
        assert!(!state.ai.is_streaming());
        assert_eq!(state.ai_polish, None, "空结果关窗");
        assert_eq!(
            state.tabs.tabs[0].document.notice.as_deref(),
            Some("润色结果为空,已放弃本次替换"),
            "文案落发起标签"
        );
        assert_eq!(state.tabs.tabs[1].document.notice, None, "当前标签零误伤");
    }

    /// chunk 分流(#61 M3):润色流进行中开新标签再收 chunk,草稿仍长在
    /// 会话里、新标签零污染(与 `ai_stream_writes_to_origin_tab` 同型)。
    #[test]
    fn selection_ai_polish_chunks_flow_to_draft_across_tab_switch() {
        let mut state = State::default();
        state.tabs.current_mut().editor.load("开头。素材。结尾。");
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((3, 5)),
        });
        let origin_id = state.ai_polish.clone().unwrap().tab_id;
        let before = state.tabs.current_mut().editor.text().to_owned();

        state.apply(Message::FileCommand(FileCmd::New));
        state.apply(Message::AiChunk {
            delta: "甲".into()
        });
        state.apply(Message::AiChunk {
            delta: "乙".into()
        });
        assert_eq!(
            state.ai_polish.clone().unwrap().draft,
            "甲乙",
            "chunk 逐块进草稿"
        );
        assert_eq!(
            state.ai_polish.clone().unwrap().tab_id,
            origin_id,
            "会话绑定发起标签"
        );
        assert_eq!(
            state.tabs.tabs[0].editor.text(),
            before,
            "发起标签文档一字未动"
        );
        assert_eq!(state.tabs.tabs[1].editor.text(), "", "新标签零污染");
        state.apply(Message::AiDone);
        assert!(
            state.ai_polish.is_some_and(|s| s.draft == "甲乙"),
            "收尾后会话保留待确认"
        );
    }

    /// 关闭发起标签(#61 M3):流式中关闭作废整条链(既有 `remove_tab`
    /// 行为,现在连会话一起清);待确认时关闭也会话作废 —— 浮窗不能指向
    /// 已不存在的标签空转。
    #[test]
    fn selection_ai_polish_session_dies_with_origin_tab() {
        // 流式中关闭
        let mut state = State::default();
        state.tabs.current_mut().editor.load("开头。素材。结尾。");
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((3, 5)),
        });
        assert!(state.ai.is_streaming());
        state.apply(Message::TabCloseActive);
        assert_eq!(state.ai_polish, None, "流式中关闭作废会话");
        assert!(!state.ai.is_streaming());

        // 待确认时关闭
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        state.tabs.current_mut().editor.load("开头。素材。结尾。");
        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: Some((3, 5)),
        });
        drain_ai_stream(&mut state);
        assert!(state.ai_polish.is_some());
        state.apply(Message::TabCloseActive);
        assert_eq!(
            state.ai_polish, None,
            "待确认关闭同样作废(浮窗不指向已关标签)"
        );
    }

    /// 选区 AI 浮标菜单点选的记账归约(M1 契约,行序保持)。M3 起润色
    /// 已接线确认浮窗,有效选区的发起/确认/放弃语义由下方 M3 专项测试
    /// 覆盖;这里断言「点选即记账」的通路原样保留 —— 无选区(防御分支:
    /// 浮标只在有选区时在场)润色拒绝并发起,文档零改动。
    #[test]
    fn selection_ai_polish_message_records_action_without_touching_document() {
        let mut state = State::default();
        assert_eq!(state.selection_ai_last, None);
        let (text, rev) = {
            let tab = state.tabs.current();
            (tab.editor.text().to_owned(), tab.editor.revision())
        };

        state.apply(Message::SelectionAiActionRequested {
            action: SelectionAiAction::Polish,
            selection: None,
        });
        assert_eq!(state.selection_ai_last, Some(SelectionAiAction::Polish));
        assert!(!state.ai.is_streaming(), "无选区不发起(素材校验拒绝)");
        assert_eq!(state.ai_polish, None, "无选区不挂会话");
        let tab = state.tabs.current();
        assert_eq!(tab.editor.text(), text, "文档一字未动");
        assert_eq!(tab.editor.revision(), rev, "修订号不前进");
        assert_eq!(tab.selection, None, "选区不动");
        assert_eq!(
            tab.document.notice.as_deref(),
            Some("请先选中要润色的文本"),
            "空选区文案与 M3 错误路径同源"
        );
    }

    /// 在临时目录里跑 git(测试数据装配);失败即 panic。
    fn run_git(dir: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(["-c", "user.name=LaterMD", "-c", "user.email=latermd@test"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} 失败: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// 建带一笔提交 + 一个工作区改动的一次性仓库;返回其路径。
    fn git_repo_with_dirty_file(name: &str) -> PathBuf {
        let dir = temp_path(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run_git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("a.md"), "HEAD 版本\n").unwrap();
        run_git(&dir, &["add", "."]);
        run_git(&dir, &["commit", "-q", "-m", "init"]);
        std::fs::write(dir.join("a.md"), "工作区乱改\n").unwrap();
        dir
    }

    /// Git 消息全链路归约:换根即刷新(角标就位)→ 选中读 diff → 回滚
    /// 请求只置模态 → 取消不动文件 → 再请求 + 确认恢复 HEAD 并刷新
    /// (干净列表、选中失效、模态关闭)。
    #[test]
    fn git_messages_drive_select_confirm_and_checkout() {
        let dir = git_repo_with_dirty_file("git-flow");
        let settings_dir = temp_path("git-flow-settings");
        let mut state = State {
            settings_dir: Some(settings_dir.clone()),
            ..State::default()
        };

        // 换根触发刷新:改动列表与绝对路径角标就位
        state.apply(Message::FileTreeRootSelected(dir.clone()));
        assert_eq!(state.git.error, None);
        assert_eq!(state.git.entries.len(), 1);
        assert_eq!(state.git.entries[0].path, "a.md");
        assert_eq!(
            state.git.badge_for(&dir.join("a.md")),
            Some(latermd_git::StatusKind::Modified)
        );

        // 选中:diff 就位
        state.apply(Message::GitFileSelected("a.md".to_owned()));
        assert_eq!(state.git.selected.as_deref(), Some("a.md"));
        assert!(state.git.diff_workspace_open, "选中后进入主工作区审阅");
        assert!(state.git.diff.contains("+工作区乱改"), "{}", state.git.diff);

        state.apply(Message::GitDiffWorkspaceClosed);
        assert!(!state.git.diff_workspace_open, "关闭后返回编辑器");
        assert_eq!(state.git.selected.as_deref(), Some("a.md"));
        state.apply(Message::GitFileSelected("a.md".to_owned()));
        assert!(state.git.diff_workspace_open, "再次选中重新打开审阅");

        // #53 M2 视图切换:纯显示偏好,归约只写字段,不动 diff 数据
        state.apply(Message::GitDiffViewChanged(
            crate::git_panel::DiffView::Unified,
        ));
        assert_eq!(state.git.diff_view, crate::git_panel::DiffView::Unified);
        assert_eq!(state.git.selected.as_deref(), Some("a.md"), "切换不动选中");
        state.apply(Message::GitDiffViewChanged(
            crate::git_panel::DiffView::Split,
        ));
        assert_eq!(state.git.diff_view, crate::git_panel::DiffView::Split);

        // 离开 Git 页只退出审阅,不影响已缓存的选中文件。
        state.apply(Message::SidebarTabChanged(SidebarTab::Files));
        assert!(!state.git.diff_workspace_open);

        // 过期路径:不顶掉选中
        state.apply(Message::GitFileSelected("stale.md".to_owned()));
        assert_eq!(state.git.selected.as_deref(), Some("a.md"));

        // 回滚请求只置模态;取消不动文件
        state.apply(Message::GitCheckoutRequested("a.md".to_owned()));
        assert_eq!(state.git.confirm_checkout.as_deref(), Some("a.md"));
        state.apply(Message::GitCheckoutCancelled);
        assert_eq!(state.git.confirm_checkout, None);
        assert!(std::fs::read_to_string(dir.join("a.md"))
            .unwrap()
            .contains("乱改"));

        // 确认:恢复 HEAD、刷新后列表干净、选中失效、模态关闭
        state.apply(Message::GitCheckoutRequested("a.md".to_owned()));
        state.apply(Message::GitCheckoutConfirmed);
        assert_eq!(
            std::fs::read_to_string(dir.join("a.md")).unwrap(),
            "HEAD 版本\n",
            "确认后文件恢复 HEAD"
        );
        assert_eq!(state.git.confirm_checkout, None);
        assert!(state.git.entries.is_empty(), "回滚后工作区干净");
        assert_eq!(state.git.selected, None, "选中随 clean 失效");
        assert!(state.git.badges.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&settings_dir);
    }

    /// 文件历史入口:右键「查看文件历史」→ 侧栏切到 Git 页 + 查询就位;
    /// 仓库外的文件不查,只留提示行。选中改动文件时,文件历史跟随选中项。
    #[test]
    fn git_file_history_request_loads_commits_and_follows_selection() {
        let dir = git_repo_with_dirty_file("git-file-history");
        let settings_dir = temp_path("git-file-history-settings");
        let mut state = State {
            settings_dir: Some(settings_dir.clone()),
            ..State::default()
        };
        state.apply(Message::FileTreeRootSelected(dir.clone()));

        state.apply(Message::GitFileHistoryRequested(dir.join("a.md")));
        assert_eq!(state.layout.left_view, SidebarTab::Git, "顺带切到 Git 页");
        let history = state.git.file_history.as_ref().expect("文件历史就位");
        assert_eq!(history.path, "a.md");
        assert_eq!(history.error, None);
        assert_eq!(
            history
                .commits
                .iter()
                .map(|commit| commit.subject.as_str())
                .collect::<Vec<_>>(),
            vec!["init"],
            "a.md 只被 init 改过"
        );

        // 仓库外文件:不查,提示行说明
        let outside = temp_path("git-file-history-outside").join("x.md");
        state.apply(Message::GitFileHistoryRequested(outside));
        assert!(
            state
                .tabs
                .current()
                .document
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("不在当前 Git 仓库")),
            "{:?}",
            state.tabs.current().document.notice
        );

        // 选中改动列表里的文件:文件历史跟随选中项
        state.apply(Message::GitFileSelected("a.md".to_owned()));
        assert_eq!(
            state.git.file_history.as_ref().map(|h| h.path.as_str()),
            Some("a.md"),
            "选中即查选中项的历史"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&settings_dir);
        let _ = std::fs::remove_dir_all(temp_path("git-file-history-outside"));
    }

    /// 文件历史跟随当前 tab 文档:打开文档后刷新即查该文档;右键显式
    /// 查询在文档不换时被轮询刷新保留;换文档后跟随新文档。
    #[test]
    fn file_history_follows_active_document_and_explicit_query_survives_poll() {
        let dir = git_repo_with_dirty_file("git-follow-doc");
        // 提交一个 b.md,供右键显式查询
        std::fs::write(dir.join("b.md"), "乙\n").unwrap();
        run_git(&dir, &["add", "b.md"]);
        run_git(&dir, &["commit", "-q", "-m", "加 b"]);

        let settings_dir = temp_path("git-follow-doc-settings");
        let mut state = State {
            settings_dir: Some(settings_dir.clone()),
            ..State::default()
        };
        state.apply(Message::FileTreeRootSelected(dir.clone()));

        // 打开 a.md(当前 tab 文档)→ 刷新后文件历史跟随 a.md
        state.tabs.current_mut().editor.clear_dirty(); // 放行 open_from 的 unsaved_guard
        state.apply(Message::FileSelected(dir.join("a.md")));
        state.refresh_git();
        assert_eq!(
            state.git.file_history.as_ref().map(|h| h.path.as_str()),
            Some("a.md"),
            "跟随当前文档"
        );

        // 右键显式查 b.md:文档没换,轮询刷新不顶掉
        state.apply(Message::GitFileHistoryRequested(dir.join("b.md")));
        assert_eq!(
            state.git.file_history.as_ref().map(|h| h.path.as_str()),
            Some("b.md")
        );
        state.refresh_git();
        assert_eq!(
            state.git.file_history.as_ref().map(|h| h.path.as_str()),
            Some("b.md"),
            "显式查询不被轮询顶掉"
        );
        // 重复激活当前 tab 不算换文档,显式查询仍应保留。
        state.apply(Message::FileSelected(dir.join("a.md")));
        state.refresh_git();
        assert_eq!(
            state.git.file_history.as_ref().map(|h| h.path.as_str()),
            Some("b.md"),
            "重复激活当前 tab 不应覆盖显式查询"
        );

        // tab 换回 a.md → 刷新后跟随 a.md
        state.apply(Message::FileSelected(dir.join("b.md")));
        state.apply(Message::FileSelected(dir.join("a.md")));
        state.refresh_git();
        assert_eq!(
            state.git.file_history.as_ref().map(|h| h.path.as_str()),
            Some("a.md"),
            "换文档跟随新文档"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&settings_dir);
    }

    /// 回滚目标正是编辑器当前文档(非 dirty):确认后编辑器重读磁盘的
    /// HEAD 版本、预览同帧联动——不重载则编辑器仍显示已丢弃的工作区版本。
    #[test]
    fn git_checkout_of_current_document_reloads_editor() {
        let dir = git_repo_with_dirty_file("git-reload");
        let file = dir.join("a.md");
        let mut state = State::default();
        state.file_tree.root = Some(dir.clone());
        state.refresh_git();
        state.tabs.current_mut().editor.clear_dirty(); // 放行 open_from 的 unsaved_guard
        state.apply(Message::FileSelected(file.clone()));
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            "工作区乱改\n",
            "前置:编辑器持有工作区版"
        );

        state.apply(Message::GitCheckoutRequested("a.md".to_owned()));
        state.apply(Message::GitCheckoutConfirmed);
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            "HEAD 版本\n",
            "编辑器随磁盘回滚重载"
        );
        assert_eq!(
            state.tabs.current().preview.text,
            "HEAD 版本\n",
            "预览快照同帧联动"
        );
        assert!(!state.tabs.current_mut().editor.is_dirty());
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(file.as_path())
        );
        assert!(
            state.tabs.current().document.notice.is_none(),
            "干净重载无提示"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回滚目标正是当前文档且编辑器 dirty:未保存的稿子保留(绝不静默
    /// 丢稿),磁盘已回 HEAD,提示行说明「保存会写回」;非当前文档的回滚
    /// 则完全不触碰编辑器。
    #[test]
    fn git_checkout_keeps_dirty_buffer_and_skips_unrelated_editor() {
        let dir = git_repo_with_dirty_file("git-dirty");
        let file = dir.join("a.md");
        let mut state = State::default();
        state.file_tree.root = Some(dir.clone());
        state.refresh_git();
        state.tabs.current_mut().editor.clear_dirty();
        state.apply(Message::FileSelected(file.clone()));
        state
            .tabs
            .current_mut()
            .editor
            .insert_chars(0, "未保存草稿\n"); // dirty
        assert!(state.tabs.current_mut().editor.is_dirty());

        state.apply(Message::GitCheckoutRequested("a.md".to_owned()));
        state.apply(Message::GitCheckoutConfirmed);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "HEAD 版本\n",
            "磁盘已回滚"
        );
        assert!(
            state
                .tabs
                .current_mut()
                .editor
                .text()
                .starts_with("未保存草稿"),
            "缓冲里的未保存稿保留"
        );
        let notice = state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .expect("有针对性提示");
        assert!(notice.contains("写回"), "{notice}");
        // 预览快照未被重置:dirty 分支不走 load_document(重载是 clean 分支
        // 的行为);快照推进交给下一帧编辑器面板的修订号检查
        assert_ne!(
            state.tabs.current().preview.text,
            "HEAD 版本\n",
            "未走重载路径"
        );

        // 回滚目标不是当前文档:编辑器与文档身份都不动
        std::fs::write(dir.join("b.md"), "HEAD 版本\n").unwrap();
        run_git(&dir, &["add", "b.md"]);
        run_git(&dir, &["commit", "-q", "-m", "b"]);
        std::fs::write(dir.join("b.md"), "乱改\n").unwrap();
        state.refresh_git();
        state.apply(Message::GitCheckoutRequested("b.md".to_owned()));
        state.apply(Message::GitCheckoutConfirmed);
        assert!(
            state
                .tabs
                .current_mut()
                .editor
                .text()
                .starts_with("未保存草稿"),
            "非当前文档的回滚不触碰编辑器"
        );
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(file.as_path())
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 切到 Git 页立即刷新:停了很久的快照不用于回滚决策。
    #[test]
    fn switching_to_git_tab_refreshes_state() {
        let dir = git_repo_with_dirty_file("git-tab");
        let mut state = State::default();
        state.file_tree.root = Some(dir.clone());
        assert!(state.git.entries.is_empty(), "前置:尚未刷过");

        state.apply(Message::SidebarTabChanged(SidebarTab::Git));
        assert_eq!(state.git.entries.len(), 1, "切页签即刷新");
        state.apply(Message::SidebarTabChanged(SidebarTab::Files));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 非 git 目录换根:Git 状态降级为提示文案,角标清空(文件树无角标);
    /// 从有状态仓库切到普通目录,旧角标不残留。
    #[test]
    fn non_git_root_degrades_git_state() {
        let repo = git_repo_with_dirty_file("git-degrade-repo");
        let plain = temp_path("git-degrade-plain");
        let _ = std::fs::remove_dir_all(&plain);
        std::fs::create_dir_all(&plain).unwrap();

        let settings_dir = temp_path("git-degrade-settings");
        let mut state = State {
            settings_dir: Some(settings_dir.clone()),
            ..State::default()
        };
        state.apply(Message::FileTreeRootSelected(repo.clone()));
        assert!(
            state.git.badge_for(&repo.join("a.md")).is_some(),
            "前置:仓库根有角标"
        );

        state.apply(Message::FileTreeRootSelected(plain.clone()));
        let error = state.git.error.as_deref().expect("降级文案");
        assert!(error.contains("不是"), "{error}");
        assert!(state.git.badges.is_empty(), "旧角标不残留");
        assert_eq!(state.git.selected, None);

        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&plain);
        let _ = std::fs::remove_dir_all(&settings_dir);
    }

    /// commit message 全链路:已保存文档 + 暂存改动 → 建议(单行、conventional
    /// 前缀、含文件名)→ 关闭清空;干净仓库与无法定位目录分别落提示行。
    #[test]
    fn ai_commit_message_flow_notices_and_dismissal() {
        let dir = temp_path("ai-commit-repo");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run_git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("README.md"), "# 后加的说明\n").unwrap();
        run_git(&dir, &["add", "."]);

        let mut state = State::default();
        state.tabs.current_mut().document.path = Some(dir.join("README.md"));

        state.apply(Message::AiCommitRequested);
        settle_ai_commit(&mut state);
        let subject = state.ai_commit_suggestion.clone().expect("已生成建议");
        assert_eq!(subject, "docs: 新增README.md");
        assert!(!subject.contains('\n'), "单行 subject");

        state.apply(Message::AiCommitDismissed);
        assert_eq!(state.ai_commit_suggestion, None, "关闭清空建议");

        // 干净仓库(全部提交):无未提交改动 → 提示,不出建议
        run_git(&dir, &["commit", "-q", "-m", "init"]);
        state.apply(Message::AiCommitRequested);
        settle_ai_commit(&mut state);
        assert_eq!(state.ai_commit_suggestion, None);
        let notice = state.tabs.current().document.notice.as_deref().unwrap();
        assert!(notice.contains("没有未提交的改动"), "{notice}");

        // 无文档且无文件树根:无法定位仓库目录
        let mut rootless = State::default();
        rootless.apply(Message::AiCommitRequested);
        assert_eq!(rootless.ai_commit_suggestion, None);
        let notice = rootless.tabs.current().document.notice.as_deref().unwrap();
        assert!(notice.contains("文件树"), "{notice}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// commit 后台等待期间切标签：失败提示必须回到发起标签，不污染用户
    /// 后来切到的文档。这是同步实现改成异步后新增的标签归属边界。
    #[test]
    fn ai_commit_result_stays_with_the_origin_tab() {
        let dir = temp_path("ai-commit-origin-tab");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run_git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("README.md"), "# 已提交\n").unwrap();
        run_git(&dir, &["add", "."]);
        run_git(&dir, &["commit", "-q", "-m", "init"]);

        let mut state = State::default();
        let origin_id = state.tabs.current().id;
        state.tabs.current_mut().document.path = Some(dir.join("README.md"));
        state.apply(Message::AiCommitRequested);
        state.spawn_tab(None, "第二篇");
        settle_ai_commit(&mut state);

        let origin = state.tabs.index_by_id(origin_id).unwrap();
        assert!(state.tabs.tabs[origin]
            .document
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("没有未提交的改动")));
        assert!(
            state.tabs.current().document.notice.is_none(),
            "当前标签不接收其他标签的迟到错误"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 防重入:续写流在途时请求生成 commit message 被静默忽略(无建议、无
    /// 提示);收尾后同一请求照常出建议。
    #[test]
    fn ai_commit_request_ignored_while_streaming() {
        let dir = temp_path("ai-commit-reentry");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run_git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("README.md"), "# 说明\n").unwrap();
        run_git(&dir, &["add", "."]);

        let mut state = State::default();
        // 慢 provider:保证测试在流自然收尾前完成断言
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::from_millis(50),
        ));
        state.tabs.current_mut().document.path = Some(dir.join("README.md"));

        state.apply(Message::AiStart);
        assert!(state.ai.is_streaming(), "前置:流在途");
        state.apply(Message::AiCommitRequested);
        assert_eq!(state.ai_commit_suggestion, None, "流式中请求被忽略");
        assert!(
            state.tabs.current().document.notice.is_none(),
            "忽略是静默的"
        );
        state.ai.finish();

        state.apply(Message::AiCommitRequested);
        settle_ai_commit(&mut state);
        assert!(state.ai_commit_suggestion.is_some(), "收尾后同一请求生效");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 轮询等首个 `AiChunk` 落地(把该块**归约进文档**后返回)。
    ///
    /// [`MockProvider`](latermd_ai::MockProvider) 的 `stream_complete` 是
    /// **真线程 + `thread::sleep(interval)`**,块是异步送达的。「睡固定
    /// 60ms 然后断言有块」等于赌 runner 的线程调度:CI runner 负载高时
    /// 首块可能晚于 60ms 才到,测试就红在与被测行为无关的地方
    /// (实测 macOS runner 上 `closing_stream_origin_tab_aborts_stream`
    /// 与 `ai_stream_writes_to_origin_tab_not_active` 连续两轮都红在
    /// 这个前置上,而本地 Linux 931 条全绿)。
    ///
    /// 改为「轮询到块为止,deadline 兜底」:赌的是**最终会来块**这件
    /// 事实(那是 Mock 的契约),不是它在哪一毫秒来。返回首块内容供
    /// 调用方进一步断言。
    fn poll_until_first_ai_chunk(state: &mut State) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let mut first = None;
            for message in state.poll_ai() {
                if let Message::AiChunk { delta } = &message {
                    first = first.or_else(|| Some(delta.clone()));
                }
                state.apply(message);
            }
            if let Some(delta) = first {
                return delta;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "10s 内未收到首个 AiChunk(Mock 契约破裂,非时序问题)"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// 把在途流收干净(收流到结束块并逐条归约),供摘要链路测试复用;
    /// 超时退出循环后断言流式标志已清。
    fn drain_ai_stream(state: &mut State) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let mut done = false;
            for message in state.poll_ai() {
                if matches!(message, Message::AiDone | Message::AiFailed(_)) {
                    done = true;
                }
                state.apply(message);
            }
            if done || std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!state.ai.is_streaming(), "流已收尾");
    }

    /// 摘要全链路(文档已含旧摘要节):发起时旧节先被移除,新「## AI 摘要」
    /// 标题落在文档末尾,流式块以引用块行长在标题下;收尾后全文恰好一个
    /// 「AI 摘要」标题,旧要点不残留,正文原样保留。
    #[test]
    fn ai_summary_replaces_old_section_and_streams_new_points() {
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        state
            .tabs
            .current_mut()
            .editor
            .load("# 设计\n\n正文段落。\n\n## AI 摘要\n\n> - 旧要点甲\n> - 旧要点乙\n");

        state.apply(Message::AiSummaryRequested);
        assert!(state.ai.is_streaming(), "发起后流式标志置位");
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            "# 设计\n\n正文段落。\n\n## AI 摘要\n\n",
            "发起时旧节已移除,新标题与空行就位"
        );

        drain_ai_stream(&mut state);
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            "# 设计\n\n正文段落。\n\n## AI 摘要\n\n> - 设计\n> - 正文段落。\n",
            "要点引用块长在标题下,旧要点不残留"
        );
        let summary_headings = latermd_md::outline(state.tabs.current_mut().editor.text())
            .into_iter()
            .filter(|item| item.text == AI_SUMMARY_HEADING)
            .count();
        assert_eq!(summary_headings, 1, "摘要标题恰好一个,不堆积");
        assert!(
            state.tabs.current_mut().editor.is_dirty(),
            "AI 写入置 dirty"
        );

        // 再来一次:新节替换上一轮的节,仍然恰好一个
        state.apply(Message::AiSummaryRequested);
        drain_ai_stream(&mut state);
        let summary_headings = latermd_md::outline(state.tabs.current_mut().editor.text())
            .into_iter()
            .filter(|item| item.text == AI_SUMMARY_HEADING)
            .count();
        assert_eq!(summary_headings, 1, "重复生成不堆积");
        assert_eq!(
            state
                .tabs
                .current_mut()
                .editor
                .text()
                .matches("## AI 摘要")
                .count(),
            1,
            "标题文本也只出现一次"
        );
    }

    /// 文档没有旧摘要节:正文原样保留,新节直接追加到末尾(空行分隔)。
    #[test]
    fn ai_summary_appends_when_document_has_no_section() {
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        state
            .tabs
            .current_mut()
            .editor
            .load("# 标题甲\n\n段落甲。\n");

        state.apply(Message::AiSummaryRequested);
        assert!(state.ai.is_streaming());
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            "# 标题甲\n\n段落甲。\n\n## AI 摘要\n\n",
            "无旧节可移除,标题直接接在补齐的空行后"
        );

        drain_ai_stream(&mut state);
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            "# 标题甲\n\n段落甲。\n\n## AI 摘要\n\n> - 标题甲\n> - 段落甲。\n"
        );
        assert!(
            state
                .tabs
                .current_mut()
                .editor
                .text()
                .starts_with("# 标题甲\n\n段落甲。"),
            "正文原样"
        );
    }

    /// 空文档:提示行拦下,不发起流、不落标题。
    #[test]
    fn ai_summary_on_empty_document_notices_without_stream() {
        let mut state = State::default();
        state.tabs.current_mut().editor.load("");
        state.apply(Message::AiSummaryRequested);
        assert!(!state.ai.is_streaming(), "空文档不发起流");
        assert_eq!(state.tabs.current_mut().editor.text(), "", "文档未被触碰");
        assert_eq!(
            state.tabs.current().document.notice.as_deref(),
            Some("文档为空,没有可摘要的内容")
        );
    }

    /// 上下文大小配置驱动摘要截断(#58 M3,decisions-pending #110):显式
    /// KB 预算下,发起的 prompt 按新预算截断、说明报真实预算、预算外内容
    /// 不混入(全程 Mock,不出网)。
    #[test]
    fn ai_summary_honors_context_kb_budget() {
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        state.ai.config.context_kb = 4;
        let long_doc = format!("{}\n尾部哨兵", "字".repeat(4000)); // 12KB+
        state.tabs.current_mut().editor.load(&long_doc);

        state.apply(Message::AiSummaryRequested);
        assert!(state.ai.is_streaming());
        let prompt = state.ai.last_prompt.clone().expect("摘要走流式入口");
        assert!(prompt.contains("已截断至约 4KB"), "{prompt}");
        assert!(!prompt.contains("尾部哨兵"), "预算外内容不进 prompt");
        assert!(
            prompt.len() > 4096 && prompt.len() < 4096 + 1024,
            "prompt 应按 4KB 预算截断:实际 {} 字节",
            prompt.len()
        );
        drain_ai_stream(&mut state);
    }

    /// 否决线(decisions-pending #110):上下文大小未配置(默认 0)时,
    /// 摘要 prompt 仍按现状 32KB 截断并注明 —— 未配置的截断行为不变。
    #[test]
    fn ai_summary_default_context_keeps_legacy_truncation() {
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::ZERO,
        ));
        assert_eq!(state.ai.config.context_kb, 0, "前置:未配置");
        state.tabs.current_mut().editor.load(&"汉".repeat(20000)); // 60KB,超现状 32KB 预算

        state.apply(Message::AiSummaryRequested);
        assert!(state.ai.is_streaming());
        let prompt = state.ai.last_prompt.clone().expect("摘要走流式入口");
        assert!(prompt.contains("已截断至约 32KB"), "{prompt}");
        assert!(prompt.len() < 32 * 1024 + 1024, "仍按 32KB 预算截断");
        drain_ai_stream(&mut state);
    }

    /// 上下文大小配置驱动 commit 截断(#58 M3):两个文件的未提交改动
    /// (a.md 改写 12KB + zz.md 删除),现状默认(0)下 16KB 预算装得下
    /// 全 diff → 尾部文件的删除动作行进 prompt(Mock 合成「清理」);显式
    /// 4KB 预算把尾部文件截掉 → 同一仓库合成「更新」。Mock 关键词规则
    /// 可观测,git diff 是本地子进程,全程不出网。
    #[test]
    fn ai_commit_message_honors_context_kb_budget() {
        let dir = temp_path("ai-commit-context");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run_git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("a.md"), "初版\n").unwrap();
        std::fs::write(dir.join("zz.md"), "将被删除\n").unwrap();
        run_git(&dir, &["add", "."]);
        run_git(&dir, &["commit", "-q", "-m", "init"]);
        // 未提交改动:改写 a.md(12KB,压预算)+ 删除 zz.md(判别动作行)
        std::fs::write(dir.join("a.md"), "字".repeat(4000)).unwrap();
        std::fs::remove_file(dir.join("zz.md")).unwrap();

        // 默认(未配置,0):全 diff ≈ 12.3KB < 16KB,删除动作行进 prompt
        let mut state = State::default();
        state.tabs.current_mut().document.path = Some(dir.join("a.md"));
        state.apply(Message::AiCommitRequested);
        settle_ai_commit(&mut state);
        assert_eq!(
            state.ai_commit_suggestion.as_deref(),
            Some("docs: 清理a.md"),
            "默认 16KB 预算下尾部删除行在场(全 diff 进 prompt)"
        );

        // 显式 4KB:12KB 的 a.md 内容把 zz.md 的删除行挤出预算
        state.ai.config.context_kb = 4;
        state.apply(Message::AiCommitRequested);
        settle_ai_commit(&mut state);
        assert_eq!(
            state.ai_commit_suggestion.as_deref(),
            Some("docs: 更新a.md"),
            "4KB 预算下删除动作行被截掉,只剩 a.md 的更新"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 防重入(双向):续写流在途时摘要请求被静默忽略 —— 连移除旧节/插
    /// 标题都不发生;摘要流在途时续写命令同样被忽略(同一道闸)。
    #[test]
    fn ai_summary_and_stream_reentry_guard_each_other() {
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::from_millis(50),
        ));
        state
            .tabs
            .current_mut()
            .editor
            .load("# 甲\n\n## AI 摘要\n\n> - 旧要点\n");

        // 续写流在途 → 摘要请求忽略:文档一字未动(旧节还在)
        state.apply(Message::AiStart);
        assert!(state.ai.is_streaming(), "前置:续写流在途");
        let before = state.tabs.current_mut().editor.text().to_owned();
        state.apply(Message::AiSummaryRequested);
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            before,
            "流式中摘要请求不碰文档"
        );
        assert!(
            state.tabs.current().document.notice.is_none(),
            "忽略是静默的"
        );
        assert!(state.ai.is_streaming(), "原流继续在途");
        state.ai.finish();

        // 摘要流在途 → 续写命令忽略
        state.apply(Message::AiSummaryRequested);
        assert!(state.ai.is_streaming(), "摘要流已发起(旧节此刻已移除)");
        assert!(
            !state.tabs.current_mut().editor.text().contains("旧要点"),
            "前置:旧节确实移除了"
        );
        let before = state.tabs.current_mut().editor.text().to_owned();
        state.apply(Message::AiStart);
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            before,
            "流式中续写命令不碰文档"
        );
        state.ai.finish();
    }

    /// 多标签关闭流程:脏标签 × → 确认模态 → 确认即移除;取消则保留。
    #[test]
    fn dirty_tab_close_requires_confirmation() {
        let mut state = State::default();
        let index = state.spawn_tab(None, "第二篇");
        state.tabs.current_mut().editor.insert_chars(0, "草稿");
        assert!(state.tabs.current_mut().editor.is_dirty());

        state.apply(Message::TabCloseRequested(index));
        assert_eq!(
            state.tabs.confirm_close,
            Some(state.tabs.tabs[index].id),
            "脏标签先弹确认(目标存稳定 id)"
        );
        assert_eq!(state.tabs.tabs.len(), 2, "未确认前不移除");

        state.apply(Message::TabCloseCancelled);
        assert_eq!(state.tabs.confirm_close, None);
        assert_eq!(state.tabs.tabs.len(), 2, "取消后保留");

        state.apply(Message::TabCloseRequested(index));
        state.apply(Message::TabCloseConfirmed);
        assert_eq!(state.tabs.tabs.len(), 1, "确认后移除");
        assert!(
            state.tabs.tabs[0].editor.text().contains("LaterMD"),
            "回到的是原来的标签(SAMPLE 文档)"
        );
    }

    /// 回归(独立评审 high):确认模态存稳定 id —— 模态开着期间关掉更靠前
    /// 的干净标签使索引漂移,「确认关闭」必须仍关掉模态所问的那个标签。
    /// 修复前按漂移索引移除:用户对 B 确认「关闭并丢弃」,实际被静默丢弃
    /// 的是漂移到该索引的另一个脏标签 C 的未保存修改。
    #[test]
    fn confirm_close_survives_index_drift_from_other_close() {
        let mut state = State::default();
        // A(干净,索引0)/ B(脏,索引1)/ C(脏,索引2):评审给出的复现序列
        state.spawn_tab(None, "B 的正文");
        state.spawn_tab(None, "C 的正文");
        let b_id = state.tabs.tabs[1].id;
        let c_id = state.tabs.tabs[2].id;
        state.apply(Message::TabActivate(1));
        state.tabs.current_mut().editor.insert_chars(0, "B 草稿");
        state.apply(Message::TabActivate(2));
        state.tabs.current_mut().editor.insert_chars(0, "C 草稿");
        assert!(!state.tabs.tabs[0].editor.is_dirty(), "前置:A 干净");

        // 点 B 的 ×:模态开在 B 上
        state.apply(Message::TabCloseRequested(1));
        assert_eq!(state.tabs.confirm_close, Some(b_id));
        assert_eq!(
            state.tabs.confirm_close_tab().map(|tab| tab.id),
            Some(b_id),
            "模态文案来源正是 B"
        );

        // 模态开着,用户用 Ctrl+W(等价 TabCloseRequested)关掉干净的 A:
        // B/C 索引各前移一位,确认目标不随索引漂移
        state.apply(Message::TabCloseRequested(0));
        assert_eq!(state.tabs.tabs.len(), 2, "A 干净,直接关");
        assert_eq!(state.tabs.confirm_close, Some(b_id), "确认目标按 id 不漂移");

        // 确认关闭:关掉的必须是 B;C 连同它的未保存修改原样保留
        state.apply(Message::TabCloseConfirmed);
        assert_eq!(state.tabs.tabs.len(), 1);
        assert_eq!(
            state.tabs.tabs[0].id, c_id,
            "留在原地的是 C(修复前它被静默关掉)"
        );
        assert!(state.tabs.tabs[0].editor.text().starts_with("C 草稿"));
        assert_eq!(state.tabs.confirm_close, None);
    }

    /// 模态目标本身被其他路径关闭(保存后变干净再 Ctrl+W 直关):确认随
    /// 移除一并撤下,迟到的「确认关闭」是 no-op,不会误伤漂移到该位置的
    /// 别的标签。
    #[test]
    fn confirm_close_invalidated_when_target_closed_elsewhere() {
        let mut state = State::default();
        state.spawn_tab(None, "草稿标签");
        state.tabs.current_mut().editor.insert_chars(0, "草稿");
        state.apply(Message::TabCloseRequested(1));
        assert_eq!(
            state.tabs.confirm_close,
            Some(state.tabs.tabs[1].id),
            "前置:模态已开在草稿标签上"
        );

        // 模态开着,但用户先保存(模态不阻塞快捷键)再按 Ctrl+W:目标已
        // 干净,第二次请求直接移除,确认一并失效
        state.tabs.tabs[1].editor.clear_dirty();
        state.apply(Message::TabCloseRequested(1));
        assert_eq!(state.tabs.tabs.len(), 1, "干净标签直关");
        assert_eq!(state.tabs.confirm_close, None, "确认随目标移除撤下");
        assert_eq!(state.tabs.confirm_close_tab().map(|tab| tab.id), None);

        // 模态已不存在:迟到的确认消息不得关掉任何标签
        state.apply(Message::TabCloseConfirmed);
        assert_eq!(state.tabs.tabs.len(), 1, "no-op,兜底标签未被误伤");
    }

    /// 批量关闭干净标签(#37):右键非活动的中间标签「关闭左侧」→ 左侧
    /// 全部移除、基准与右侧幸存,幸存者 TabId 不变(id 是编辑器 widget 与
    /// undo 状态的锚,批量关闭不得重排)。
    #[test]
    fn batch_close_left_of_non_active_anchor_keeps_survivor_ids() {
        let mut state = State::default();
        state.spawn_tab(None, "乙");
        state.spawn_tab(None, "丙"); // active = 2(丙)
        let ids: Vec<u64> = state.tabs.tabs.iter().map(|tab| tab.id).collect();

        // 右键乙(索引 1,非活动)关闭其左侧:只有甲
        state.apply(Message::TabBatchCloseRequested {
            kind: crate::tabs::BatchClose::Left,
            index: 1,
        });
        assert!(state.tabs.pending_close.is_empty(), "全干净,队列一次走完");
        assert_eq!(state.tabs.tabs.len(), 2, "只关甲");
        assert_eq!(state.tabs.tabs[0].id, ids[1], "乙幸存且 id 不变");
        assert_eq!(state.tabs.tabs[1].id, ids[2], "丙幸存且 id 不变");
        assert!(state.tabs.tabs[0].editor.text().contains("乙"));
        assert!(state.tabs.current().editor.text().contains("丙"));
    }

    /// 多脏标签逐个确认(#37):队列按顺序停在第一个脏标签的确认模态上,
    /// 确认一个关一个、模态挪到下一个;每个模态只问它自己的目标,后续
    /// 弹窗不得影响已完成的关闭;基准标签(丙,右键目标)始终保留。
    #[test]
    fn batch_close_confirms_each_dirty_tab_in_turn() {
        let mut state = State::default();
        state.spawn_tab(None, "乙");
        state.spawn_tab(None, "丙");
        let dirty0 = state.tabs.tabs[0].id; // 甲(SAMPLE)
        let dirty1 = state.tabs.tabs[1].id; // 乙
        let anchor = state.tabs.tabs[2].id; // 丙:右键目标,干净
        state.apply(Message::TabActivate(0));
        state.tabs.current_mut().editor.insert_chars(0, "甲的草稿");
        state.apply(Message::TabActivate(1));
        state.tabs.current_mut().editor.insert_chars(0, "乙的草稿");
        assert!(state.tabs.tabs[0].editor.is_dirty());
        assert!(state.tabs.tabs[1].editor.is_dirty());
        assert!(!state.tabs.tabs[2].editor.is_dirty());

        // 右键丙「关闭其他」:队列 = [甲, 乙](基准不含)
        state.apply(Message::TabBatchCloseRequested {
            kind: crate::tabs::BatchClose::Others,
            index: 2,
        });
        // 队头甲脏 → 模态问甲,一个都不先关
        assert_eq!(state.tabs.confirm_close, Some(dirty0));
        assert_eq!(state.tabs.tabs.len(), 3, "确认前不移除");

        state.apply(Message::TabCloseConfirmed);
        assert_eq!(state.tabs.tabs.len(), 2, "甲确认后移除");
        assert_eq!(state.tabs.confirm_close, Some(dirty1), "模态挪到乙");

        state.apply(Message::TabCloseConfirmed);
        assert_eq!(state.tabs.tabs.len(), 1, "乙确认后移除");
        assert_eq!(state.tabs.tabs[0].id, anchor, "留下的正是基准丙");
        assert!(state.tabs.pending_close.is_empty(), "队列走空");
        assert_eq!(state.tabs.confirm_close, None);
    }

    /// 批量关闭中取消(#37):任一次取消立即终止剩余队列,已确认关闭的
    /// 保持关闭(不回滚),未处理的标签保持打开;迟到的确认消息 no-op。
    #[test]
    fn batch_close_cancel_stops_remaining_and_keeps_confirmed() {
        let mut state = State::default();
        state.spawn_tab(None, "乙");
        state.spawn_tab(None, "丙");
        let a = state.tabs.tabs[0].id;
        let b = state.tabs.tabs[1].id;
        let c = state.tabs.tabs[2].id;
        state.apply(Message::TabActivate(0));
        state.tabs.current_mut().editor.insert_chars(0, "甲的草稿");
        state.apply(Message::TabActivate(1));
        state.tabs.current_mut().editor.insert_chars(0, "乙的草稿");
        state.apply(Message::TabActivate(2)); // active 回丙

        state.apply(Message::TabBatchCloseRequested {
            kind: crate::tabs::BatchClose::All,
            index: 0,
        });
        assert_eq!(state.tabs.confirm_close, Some(a), "模态先问甲");
        state.apply(Message::TabCloseConfirmed); // 甲确认关闭
        assert_eq!(state.tabs.confirm_close, Some(b), "模态再问乙");

        state.apply(Message::TabCloseCancelled); // 用户拒关乙
        assert_eq!(state.tabs.confirm_close, None);
        assert!(state.tabs.pending_close.is_empty(), "剩余队列终止");
        assert_eq!(state.tabs.tabs.len(), 2, "已确认的甲保持关闭");
        assert_eq!(state.tabs.tabs[0].id, b, "乙保持打开");
        assert_eq!(state.tabs.tabs[1].id, c, "丙保持打开");

        state.apply(Message::TabCloseConfirmed);
        assert_eq!(state.tabs.tabs.len(), 2, "迟到确认不再关任何标签");
    }

    /// 关闭全部到最后(#37):最后一批移除把标签关空,`TabsState::remove`
    /// 兜底补一个新的空标签 —— 「关掉全部仍留一个空标签」与单标签时代
    /// 同口径,且新标签 id 不与任何旧 id 复用。
    #[test]
    fn batch_close_all_leaves_single_fresh_empty_tab() {
        let mut state = State::default();
        state.spawn_tab(None, "乙");
        let old_ids: Vec<u64> = state.tabs.tabs.iter().map(|tab| tab.id).collect();
        state.apply(Message::TabBatchCloseRequested {
            kind: crate::tabs::BatchClose::All,
            index: 0,
        });
        // 首标签 SAMPLE 起始不脏 → 直接关;乙干净 → 直接关;关空兜底
        assert_eq!(state.tabs.tabs.len(), 1, "关空后剩一个兜底标签");
        assert_eq!(state.tabs.current().editor.text(), "", "兜底标签为空");
        assert_eq!(state.tabs.current().document.path, None, "兜底标签未命名");
        assert!(
            !old_ids.contains(&state.tabs.current().id),
            "兜底标签 id 不复用"
        );
    }

    /// 批量确认与单发关闭交错(#37「确认结果不可被后续弹窗覆盖」):批量
    /// 模态开着时,用户经 × / Ctrl+W 关掉别的**干净**标签 —— 单发直关不
    /// 动批量模态;随后确认批量目标,剩余队列继续走完,每个确认只关它
    /// 问的那个标签。
    #[test]
    fn batch_close_modal_survives_interleaved_clean_close() {
        let mut state = State::default();
        state.spawn_tab(None, "乙(干净)");
        state.spawn_tab(None, "丙(基准)");
        let a = state.tabs.tabs[0].id;
        let c = state.tabs.tabs[2].id;
        state.apply(Message::TabActivate(0));
        state.tabs.current_mut().editor.insert_chars(0, "甲的草稿");

        // 右键丙「关闭其他」:队列 [甲(脏), 乙(净)],模态先问甲
        state.apply(Message::TabBatchCloseRequested {
            kind: crate::tabs::BatchClose::Others,
            index: 2,
        });
        assert_eq!(state.tabs.confirm_close, Some(a));

        // 模态非阻塞:用户顺手关掉干净的乙(单发直关)
        state.apply(Message::TabCloseRequested(1));
        assert_eq!(state.tabs.tabs.len(), 2, "乙单发直关");
        assert_eq!(state.tabs.confirm_close, Some(a), "批量模态目标不受影响");

        state.apply(Message::TabCloseConfirmed); // 确认的正是模态所问的甲
        assert_eq!(state.tabs.tabs.len(), 1);
        assert_eq!(state.tabs.tabs[0].id, c, "基准丙幸存");
        assert!(
            state.tabs.pending_close.is_empty(),
            "队列里的乙已被单发关掉,自然跳过"
        );
    }

    /// 批量模态被单发**脏**关闭顶掉(#37):确认先答单发的那个,批量随后
    /// 从自己的队首继续 —— 被顶掉的模态目标重新弹出,没有任何标签未经
    /// 自己的确认被关掉。
    #[test]
    fn batch_close_resumes_after_interleaved_dirty_close() {
        let mut state = State::default();
        state.spawn_tab(None, "乙(脏)");
        state.spawn_tab(None, "丙(基准)");
        let a = state.tabs.tabs[0].id;
        let b = state.tabs.tabs[1].id;
        let c = state.tabs.tabs[2].id;
        state.apply(Message::TabActivate(0));
        state.tabs.current_mut().editor.insert_chars(0, "甲的草稿");
        state.apply(Message::TabActivate(1));
        state.tabs.current_mut().editor.insert_chars(0, "乙的草稿");

        // 右键丙「关闭其他」:队列 [甲(脏), 乙(脏)],模态先问甲
        state.apply(Message::TabBatchCloseRequested {
            kind: crate::tabs::BatchClose::Others,
            index: 2,
        });
        assert_eq!(state.tabs.confirm_close, Some(a));

        // 模态非阻塞:用户 × 掉脏的乙 → 单发模态顶掉批量模态
        state.apply(Message::TabCloseRequested(1));
        assert_eq!(state.tabs.confirm_close, Some(b), "单发请求顶掉批量模态");

        state.apply(Message::TabCloseConfirmed); // 先答单发的乙
        assert_eq!(state.tabs.tabs.len(), 2, "乙确认关闭");
        assert_eq!(
            state.tabs.confirm_close,
            Some(a),
            "批量从自己的队首(甲)重新弹模态"
        );

        state.apply(Message::TabCloseConfirmed); // 再答批量问的甲
        assert_eq!(state.tabs.tabs.len(), 1);
        assert_eq!(state.tabs.tabs[0].id, c, "基准丙幸存");
        assert!(state.tabs.pending_close.is_empty());
    }

    /// 批量关闭携带 AI 流的标签(#37):发起标签被移除即作废流(复用
    /// `remove_tab` 的既有作废),迟到的 chunk 丢弃,绝不落到幸存标签,
    /// 幸存标签 id 稳定。
    #[test]
    fn batch_close_aborts_stream_and_late_chunk_misses_survivors() {
        let mut state = State::default();
        // 慢 provider 保证流在批量关闭发起时仍在途
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::from_millis(20),
        ));
        state.spawn_tab(None, "幸存者"); // active = 1,流发起标签是 0
        state.apply(Message::TabActivate(0));
        state.apply(Message::AiStart);
        assert!(state.ai.is_streaming());
        let origin_id = state.ai_active_tab.expect("发起时锁定标签 id");
        let survivor_id = state.tabs.tabs[1].id;

        // 右键幸存者(索引 1,非活动)「关闭其他」:队列 = [发起标签]。
        // 发起标签因流式补行/写入变脏 → 先弹确认;确认移除即作废流。
        state.apply(Message::TabBatchCloseRequested {
            kind: crate::tabs::BatchClose::Others,
            index: 1,
        });
        assert_eq!(state.tabs.confirm_close, Some(origin_id));
        state.apply(Message::TabCloseConfirmed);
        assert!(!state.ai.is_streaming(), "发起标签被关,流随之作废");
        assert_eq!(state.ai_active_tab, None, "绑定一并清除");
        assert_eq!(state.tabs.tabs.len(), 1, "幸存者保留");
        assert_eq!(state.tabs.current().id, survivor_id, "幸存者 id 稳定");

        // 迟到 chunk:写入目标已失效,丢弃,不写进幸存标签
        let before = state.tabs.current_mut().editor.text().to_owned();
        state.apply(Message::AiChunk {
            delta: "迟到的续写".into(),
        });
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            before,
            "迟到 chunk 不得写入幸存标签"
        );
        assert_eq!(state.ai_active_tab, None);
    }

    /// 标签重命名(#37,**显示别名**语义)成功路径:右键非活动标签 → 浮窗
    /// 预填其当前显示基础名 → 改草稿(UI 原地改的等价直写)→ 确认置别名,
    /// 该标签显示名变为别名;**作用范围是纯显示层** —— path/缓冲/dirty
    /// 逐一不变,别的标签不受影响。
    #[test]
    fn tab_rename_sets_alias_display_only() {
        let mut state = State::default();
        state.spawn_tab(Some(PathBuf::from("/docs/note.md")), "笔记正文");
        state.apply(Message::TabActivate(0)); // active = 默认标签,右键目标是 note(非活动)
        let (path_before, text_before, dirty_before) = (
            state.tabs.tabs[1].document.path.clone(),
            state.tabs.tabs[1].editor.text().to_owned(),
            state.tabs.tabs[1].editor.is_dirty(),
        );

        state.apply(Message::TabRenameRequested { index: 1 });
        let target_id = state.tabs.tabs[1].id;
        let rename = state.tabs.rename.as_ref().unwrap();
        assert_eq!(rename.tab_id, target_id, "目标按稳定 id 记(非活动标签)");
        assert_eq!(rename.draft, "note.md", "预填显示基础名(不带 dirty 星)");

        state.tabs.rename.as_mut().unwrap().draft = "  我的笔记  ".to_owned();
        state.apply(Message::TabRenameConfirmed);
        assert_eq!(state.tabs.tabs[1].alias.as_deref(), Some("我的笔记"));
        assert_eq!(state.tabs.rename, None, "确认后浮窗撤下");
        assert_eq!(state.tabs.tabs[1].display_name(), "我的笔记", "显示名=别名");
        // 作用范围:纯显示层,三个不变式逐一核对
        assert_eq!(state.tabs.tabs[1].document.path, path_before);
        assert_eq!(state.tabs.tabs[1].editor.text(), text_before);
        assert_eq!(state.tabs.tabs[1].editor.is_dirty(), dirty_before);
        assert_eq!(state.tabs.tabs[0].alias, None, "别的标签不受影响");

        // 落盘标签的预填:别名优先于文件名
        state.apply(Message::TabRenameRequested { index: 0 });
        assert_eq!(
            state.tabs.rename.as_ref().unwrap().draft,
            state.tabs.tabs[0].document.base_name(),
            "无别名时预填文件名"
        );
        state.apply(Message::TabRenameCancelled);
        assert_eq!(state.tabs.rename, None, "取消撤下浮窗");
        assert_eq!(state.tabs.tabs[0].alias, None, "取消不置别名");
    }

    /// 重命名空白拒绝(#37 别名):草稿 trim 后为空 → 不置别名、浮窗与草稿
    /// 保留(UI 的「确定」在空草稿时已禁用,这里兜直发消息),提示行说明;
    /// path/缓冲/dirty 同样分毫不动。
    #[test]
    fn tab_rename_rejects_blank_name() {
        let mut state = State::default();
        let index = state.spawn_tab(Some(PathBuf::from("/docs/note.md")), "正文");
        let (path_before, text_before, dirty_before) = (
            state.tabs.tabs[index].document.path.clone(),
            state.tabs.tabs[index].editor.text().to_owned(),
            state.tabs.tabs[index].editor.is_dirty(),
        );

        state.apply(Message::TabRenameRequested { index });
        state.tabs.rename.as_mut().unwrap().draft = "   ".to_owned();
        state.apply(Message::TabRenameConfirmed);
        assert_eq!(state.tabs.tabs[index].alias, None, "空名不置别名");
        let rename = state.tabs.rename.as_ref().expect("浮窗保留");
        assert_eq!(rename.draft, "   ", "草稿原样保留");
        let notice = state.tabs.tabs[index].document.notice.as_deref().unwrap();
        assert!(notice.contains("不能为空"), "提示说明原因:{notice}");
        assert_eq!(state.tabs.tabs[index].document.path, path_before);
        assert_eq!(state.tabs.tabs[index].editor.text(), text_before);
        assert_eq!(state.tabs.tabs[index].editor.is_dirty(), dirty_before);

        // 补上合法名再确认:一次拒绝不废整个流程
        state.tabs.rename.as_mut().unwrap().draft = "新名".to_owned();
        state.apply(Message::TabRenameConfirmed);
        assert_eq!(state.tabs.tabs[index].alias.as_deref(), Some("新名"));
    }

    /// 重命名作用范围(#37 别名):别名盖住显示后,**保存目标不变** ——
    /// Ctrl+S 仍写原路径、盘上文件名不变、目录里不出现以别名命名的文件;
    /// 另存为对话框的预填名也不跟别名走(仍取文件名)。
    #[test]
    fn tab_rename_alias_keeps_save_target_and_disk_untouched() {
        let dir = temp_path("rename-alias");
        std::fs::create_dir_all(&dir).unwrap();
        let note = dir.join("note.md");
        std::fs::write(&note, "盘上内容\n").unwrap();

        let mut state = State::default();
        state.open_path(&note);
        state.apply(Message::TabRenameRequested {
            index: state.tabs.active,
        });
        state.tabs.rename.as_mut().unwrap().draft = "我的笔记".to_owned();
        state.apply(Message::TabRenameConfirmed);
        assert_eq!(
            state.tabs.current().display_name(),
            "我的笔记",
            "前置:别名生效"
        );

        // 编辑后保存:写盘目标必须是原路径(别名不是文件名)
        state.tabs.current_mut().editor.insert_chars(0, "新内容 ");
        state.apply(Message::FileCommand(FileCmd::Save));
        assert_eq!(std::fs::read_to_string(&note).unwrap(), "新内容 盘上内容\n");
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(note.as_path()),
            "保存不改落盘身份"
        );
        let entries: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            entries,
            vec!["note.md".to_owned()],
            "目录里只有原文件,别名不落盘"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 重命名目标失效(#37 别名):浮窗开着时目标被其他入口关掉 → 浮窗随
    /// 移除撤下,迟到的确认是 no-op;过期索引的请求也是 no-op。
    #[test]
    fn tab_rename_target_closed_or_stale_index_is_noop() {
        let mut state = State::default();
        state.spawn_tab(None, "乙");
        state.apply(Message::TabRenameRequested { index: 1 });
        let target_id = state.tabs.tabs[1].id;
        assert_eq!(state.tabs.rename.as_ref().unwrap().tab_id, target_id);

        // 浮窗非阻塞:用户顺手关掉目标(干净标签直关)
        state.apply(Message::TabCloseRequested(1));
        assert_eq!(state.tabs.rename, None, "浮窗随目标移除撤下");
        let survivors: Vec<Option<String>> = state
            .tabs
            .tabs
            .iter()
            .map(|tab| tab.alias.clone())
            .collect();
        state.apply(Message::TabRenameConfirmed);
        assert_eq!(
            state
                .tabs
                .tabs
                .iter()
                .map(|tab| tab.alias.clone())
                .collect::<Vec<_>>(),
            survivors,
            "迟到确认不给任何标签置别名"
        );

        // 过期索引(菜单弹出帧到点击之间标签被关):no-op
        state.apply(Message::TabRenameRequested { index: 99 });
        assert_eq!(state.tabs.rename, None);
    }

    /// 重名别名不冲突(#37 别名 vs 文件改名):别名是纯显示文本,两个标签
    /// 允许同名别名(文件改名的「同名已存在即拒绝」在别名语义下不存在),
    /// 各自的落盘身份与缓冲互不牵连。
    #[test]
    fn tab_rename_duplicate_alias_is_allowed() {
        let mut state = State::default();
        let a = state.spawn_tab(Some(PathBuf::from("/docs/a.md")), "甲正文");
        let b = state.spawn_tab(Some(PathBuf::from("/docs/b.md")), "乙正文");
        for index in [a, b] {
            state.apply(Message::TabRenameRequested { index });
            state.tabs.rename.as_mut().unwrap().draft = "同名笔记".to_owned();
            state.apply(Message::TabRenameConfirmed);
        }
        assert_eq!(state.tabs.tabs[a].alias.as_deref(), Some("同名笔记"));
        assert_eq!(state.tabs.tabs[b].alias.as_deref(), Some("同名笔记"));
        assert_eq!(
            state.tabs.tabs[b].document.path,
            Some(PathBuf::from("/docs/b.md")),
            "各自的落盘身份不受同名别名影响"
        );
        assert_eq!(state.tabs.tabs[b].editor.text(), "乙正文");
    }

    /// 标题宽度模式切换(#37):偏好落 settings.json 且重启(重新 load)后
    /// 保持;切换前后每个标签的路径/缓冲/dirty/别名逐项不变 —— 纯显示层,
    /// 不碰任何文档状态,更不触碰盘上文件(目录里只有 settings.json)。
    #[test]
    fn tab_title_width_switch_persists_without_touching_tabs() {
        let dir = temp_path("title-width-state");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        let a = state.spawn_tab(Some(PathBuf::from("/docs/甲.md")), "甲正文");
        state.tabs.tabs[a].document.dirty = true;
        state.tabs.tabs[a].alias = Some("甲的别名".to_owned());
        let _b = state.spawn_tab(None, "未落盘草稿");
        let before: Vec<(Option<PathBuf>, String, bool, Option<String>)> = state
            .tabs
            .tabs
            .iter()
            .map(|tab| {
                (
                    tab.document.path.clone(),
                    tab.editor.text().to_owned(),
                    tab.document.dirty,
                    tab.alias.clone(),
                )
            })
            .collect();
        let ids_before: Vec<u64> = state.tabs.tabs.iter().map(|tab| tab.id).collect();

        state.apply(Message::TabTitleWidthChanged(
            crate::theme::TitleWidthMode::Short,
        ));
        assert_eq!(
            state.theme.tab_title_width,
            crate::theme::TitleWidthMode::Short
        );
        state.apply(Message::TabTitleWidthChanged(
            crate::theme::TitleWidthMode::Full,
        ));
        assert_eq!(
            state.theme.tab_title_width,
            crate::theme::TitleWidthMode::Full
        );

        let after: Vec<(Option<PathBuf>, String, bool, Option<String>)> = state
            .tabs
            .tabs
            .iter()
            .map(|tab| {
                (
                    tab.document.path.clone(),
                    tab.editor.text().to_owned(),
                    tab.document.dirty,
                    tab.alias.clone(),
                )
            })
            .collect();
        assert_eq!(after, before, "切换不动路径/缓冲/dirty/别名");
        assert_eq!(
            state.tabs.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            ids_before,
            "标签 id 不变"
        );
        assert!(
            state.tabs.tabs[a].document.notice.is_none(),
            "落盘成功不落提示"
        );

        // 持久化:settings.json 里落了最后一次切换的值,重新 load 拿回 Short
        state.apply(Message::TabTitleWidthChanged(
            crate::theme::TitleWidthMode::Short,
        ));
        let json = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            json.contains(r#""tab_title_width": "short""#),
            "settings.json 落了缩短模式:{json}"
        );
        assert_eq!(
            crate::theme::ThemeSettings::load_from(&dir)
                .unwrap()
                .tab_title_width,
            crate::theme::TitleWidthMode::Short,
            "重启后保持用户选择"
        );
        let entries = std::fs::read_dir(&dir)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            entries
                .iter()
                .all(|entry| entry.file_name() == "settings.json"),
            "切换不写任何文档文件,目录里只有 settings.json"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归(独立评审 medium):另存为的目标已在**另一**标签打开时,拒绝
    /// 认领路径且不写盘 —— 维持「同一路径至多一个标签」,两个标签不再各
    /// 保存一次就互相静默覆盖;保存到本标签已持有的路径(常规 Ctrl+S)
    /// 不受去重影响。
    #[test]
    fn save_to_path_open_in_other_tab_is_refused() {
        let dir = temp_path("saveas-dup");
        std::fs::create_dir_all(&dir).unwrap();
        let note = dir.join("note.md");
        std::fs::write(&note, "盘上内容\n").unwrap();

        let mut state = State::default();
        state.open_path(&note); // note 占一个标签并激活
        let note_tab = state.tabs.active;

        // 回到未命名草稿标签,把它另存为到 note.md(对话框结果的等价直调)
        state.apply(Message::TabActivate(0));
        state.tabs.current_mut().editor.insert_chars(0, "草稿");
        state.save_to(note.clone());
        let notice = state.tabs.current().document.notice.as_deref().unwrap();
        assert!(notice.contains("note.md"), "{notice}");
        assert!(notice.contains("另一标签"), "{notice}");
        assert_eq!(
            state.tabs.current().document.path,
            None,
            "草稿标签未认领该路径"
        );
        assert_eq!(
            state.tabs.tabs[note_tab].document.path.as_deref(),
            Some(note.as_path()),
            "另一标签的落盘身份不动"
        );
        assert_eq!(
            std::fs::read_to_string(&note).unwrap(),
            "盘上内容\n",
            "拒绝发生在写盘之前,盘上内容不被触碰"
        );
        assert_eq!(state.tabs.tabs.len(), 2, "标签数不变");

        // 对照:保存到自己已持有的路径照常落盘(常规 Ctrl+S 语义)
        state.apply(Message::TabActivate(note_tab));
        state.tabs.current_mut().editor.insert_chars(0, "改后");
        state.save_to(note.clone());
        assert_eq!(
            std::fs::read(&note).unwrap(),
            state.tabs.current_mut().editor.text().as_bytes()
        );
        assert_eq!(state.tabs.current().document.notice, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ctrl+Tab 循环切换;切换不作废在途流(流绑定发起标签,见
    /// `ai_stream_writes_to_origin_tab_not_active`)。
    #[test]
    fn tab_next_cycles_and_stream_keeps_running() {
        let mut state = State::default();
        state.spawn_tab(None, "第二篇");
        state.spawn_tab(None, "第三篇");
        assert_eq!(state.tabs.active, 2);

        state.apply(Message::TabNext);
        assert_eq!(state.tabs.active, 0, "循环回第一个");
        state.apply(Message::TabNext);
        assert_eq!(state.tabs.active, 1);

        // 在途流随切换继续:写入目标仍是发起标签
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::new());
        state.apply(Message::AiStart);
        assert!(state.ai.is_streaming());
        let origin = state.ai_active_tab;
        state.apply(Message::TabNext);
        assert!(state.ai.is_streaming(), "切标签不作废在途流");
        assert_eq!(state.ai_active_tab, origin, "写入目标不改道");
        state.apply(Message::AiDone);
    }

    /// 关闭在途流的发起标签:流作废 —— 剩余 chunk 无处可写,落到任何
    /// 别的标签都是写错文档;兜底空标签保持空白。
    #[test]
    fn closing_stream_origin_tab_aborts_stream() {
        let mut state = State::default();
        state.ai.runtime = crate::ai::AiRuntime::Mock(latermd_ai::MockProvider::with_interval(
            std::time::Duration::from_millis(20),
        ));
        state.apply(Message::AiStart);
        assert!(state.ai.is_streaming());
        let first_chunk = poll_until_first_ai_chunk(&mut state);
        assert!(!first_chunk.is_empty(), "前置:流在产块");

        // AI 写入已置 dirty → 关闭走确认模态
        state.apply(Message::TabCloseRequested(0));
        assert_eq!(
            state.tabs.confirm_close,
            Some(state.tabs.tabs[0].id),
            "发起标签已脏,先弹确认"
        );
        state.apply(Message::TabCloseConfirmed);
        assert!(!state.ai.is_streaming(), "发起标签被关,流作废");
        assert_eq!(state.ai_active_tab, None, "绑定一并清除");
        assert_eq!(state.tabs.tabs.len(), 1, "回到唯一的兜底空标签");

        // 给 worker 留出再发几块的时间:接收端已 drop,不得再有正文块
        std::thread::sleep(std::time::Duration::from_millis(150));
        let messages = state.poll_ai();
        assert!(
            messages
                .iter()
                .all(|m| !matches!(m, Message::AiChunk { .. })),
            "作废后 poll 不再产出正文块,实际 {messages:?}"
        );
        assert_eq!(
            state.tabs.current_mut().editor.text(),
            "",
            "兜底空标签未被 AI 追加"
        );
    }

    /// 路径去重:同一路径无论从哪个入口打开(打开归约 / 文件树点击 /
    /// 搜索跳转),至多占一个标签;再次打开是激活而非新建。
    #[test]
    fn opening_same_path_twice_yields_single_tab() {
        let dir = temp_path("dedup-open");
        std::fs::create_dir_all(&dir).unwrap();
        let note = dir.join("note.md");
        std::fs::write(&note, "# 去重\n").unwrap();

        let mut state = State::default();
        state.open_path(&note);
        assert_eq!(state.tabs.tabs.len(), 2, "首次打开开新标签");

        // 文件树点击同路径:激活已有标签,不再新建
        state.spawn_tab(None, "第三篇");
        assert_eq!(state.tabs.tabs.len(), 3);
        state.apply(Message::FileSelected(note.clone()));
        assert_eq!(state.tabs.tabs.len(), 3, "同路径不新建标签");
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(note.as_path()),
            "已切到该路径的标签"
        );

        // 搜索跳转同语义
        state.apply(Message::SearchResultClicked(note.clone(), 1));
        assert_eq!(state.tabs.tabs.len(), 3, "搜索点击同路径仍不新建");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 切标签后的编辑只落 active 标签:另一标签的文本、dirty 镜像与预览
    /// 快照都不动(每标签一套缓冲/快照/光标,换标签零拷贝)。
    #[test]
    fn editing_after_switch_only_touches_active_tab() {
        let mut state = State::default();
        let origin_text = state.tabs.current_mut().editor.text().to_owned();
        let origin_rev = state.tabs.current().preview.synced_rev;
        state.spawn_tab(None, "第二篇");
        assert_eq!(state.tabs.active, 1);

        state
            .tabs
            .current_mut()
            .editor
            .insert_chars(0, "只属于第二篇");
        state.end_of_logic();
        assert!(
            state.tabs.tabs[1].editor.text().starts_with("只属于第二篇"),
            "active 标签吃到编辑"
        );
        assert!(state.tabs.tabs[1].document.dirty, "dirty 镜像只落 active");
        assert_eq!(
            state.tabs.tabs[0].editor.text(),
            origin_text,
            "另一标签文本不动"
        );
        assert!(!state.tabs.tabs[0].document.dirty);
        assert_eq!(
            state.tabs.tabs[0].preview.synced_rev, origin_rev,
            "另一标签的预览快照不动"
        );
    }

    /// 格式工具条的**归约那一半**(§6.4 链路中段):`FormatRequested(Bold)`
    /// → 缓冲变成 `**甲乙丙**`,新选区挂到 `pending_selection` 等 UI 回填。
    ///
    /// UI 那一半(谁把它写回 `TextEdit` 的持久 cursor)由
    /// `ui::layout::clicking_bold_in_a_real_frame_requests_format` 与
    /// `ui::editor` 的 `write_selection` 覆盖 —— 两段分开钉,是因为
    /// `TextEdit` 会自行归一化 `CCursorRange`,合并测会耦死在它的行为上。
    #[test]
    fn format_requested_rewrites_buffer_and_stages_selection() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("甲乙丙");
        state.tabs.current_mut().document.dirty = false;
        state.tabs.current_mut().selection = Some((0, 3));

        state.apply(Message::FormatRequested(FormatAction::Bold));

        let tab = state.tabs.current();
        assert_eq!(tab.editor.text(), "**甲乙丙**", "整段被包裹");
        assert_eq!(
            tab.pending_selection,
            Some((2, 5)),
            "新选区覆盖包裹后的三个字,由 UI 下一帧写回"
        );
        let staged = tab.pending_selection;
        // dirty 是帧末 `end_of_logic` 才从 editor 镜像到 document 的,这里
        // 不能跳步直接读(`end_of_logic_mirrors_editor_dirty` 已钉过)。
        state.end_of_logic();
        assert!(state.tabs.current().document.dirty, "格式动作算编辑 → 标脏");

        // 再点一次应当脱掉标记:证明 pending_selection 会被 UI 抄成下一次
        // 的输入(连点两次 = 上一次附录预期)。
        state.tabs.current_mut().selection = staged;
        state.apply(Message::FormatRequested(FormatAction::Bold));
        assert_eq!(
            state.tabs.current().editor.text(),
            "甲乙丙",
            "选区正好框住内层 → toggle off"
        );
    }

    /// Live 模式:格式动作落在**活动块内**(2026-10-10)。链路:`live::ui`
    /// 把活动块选区镜像进 `tab.selection`(文档坐标)→ 归约对块切片
    /// apply → `replace_range` 定点写回 → `pending_caret/pending_range`
    /// 落回「内容仍选中」。此前 live 下点加粗会拿源码模式的陈旧选区
    /// (甚至从 (0,0))盲插 `**`,是本分支堵掉的现行 bug。
    #[test]
    fn live_format_applies_inside_the_active_block() {
        let mut state = State {
            render_mode: crate::live::RenderMode::Live,
            ..Default::default()
        };
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("hello world\n\n后文");
        let tab = state.tabs.current_mut();
        tab.live.sync(&tab.editor, None);
        tab.live.active = Some(0);
        tab.selection = Some((0, 5)); // "hello"

        state.apply(Message::FormatRequested(FormatAction::Bold));

        let tab = state.tabs.current();
        assert_eq!(
            tab.editor.text(),
            "**hello** world\n\n后文",
            "块内包裹,块外一字不动"
        );
        assert_eq!(
            tab.live.pending_caret,
            Some((0, 2)),
            "光标落内容起点(** 之后)"
        );
        assert_eq!(tab.live.pending_range, Some((2, 7)), "内容仍选中");
        assert!(!tab.live.caret_follow, "工具条触发不拽视口");
        assert_eq!(tab.pending_selection, None, "Live 不走 pending_selection");
        assert_eq!(tab.cursor.byte, Some(2), "大纲/状态栏光标同帧跟上内容起点");
    }

    /// Live 守门:无活动块时格式动作 no-op —— 不许拿源码模式的陈旧选区
    /// (或 (0,0))在任意位置盲写标记。
    #[test]
    fn live_format_without_active_block_is_a_noop() {
        let mut state = State {
            render_mode: crate::live::RenderMode::Live,
            ..Default::default()
        };
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("hello world\n\n后文");
        let tab = state.tabs.current_mut();
        tab.live.sync(&tab.editor, None);
        // live.active 刻意保持 None:没有块获得焦点;selection 是残留值
        tab.selection = Some((0, 5));

        state.apply(Message::FormatRequested(FormatAction::Bold));

        let tab = state.tabs.current();
        assert_eq!(tab.editor.text(), "hello world\n\n后文", "无活动块不写");
        assert!(tab.live.pending_caret.is_none());
        assert!(tab.live.pending_range.is_none());
    }

    /// Live 守门:选区与活动块不相交(过期镜像/跨块残留)同样 no-op。
    #[test]
    fn live_format_with_selection_outside_active_block_is_a_noop() {
        let mut state = State {
            render_mode: crate::live::RenderMode::Live,
            ..Default::default()
        };
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("hello world\n\n后文");
        let tab = state.tabs.current_mut();
        tab.live.sync(&tab.editor, None);
        tab.live.active = Some(1); // 活动块是「后文」
        tab.selection = Some((0, 5)); // 选的却是块 0 的 "hello"

        state.apply(Message::FormatRequested(FormatAction::Bold));

        assert_eq!(state.tabs.current().editor.text(), "hello world\n\n后文");
    }

    /// 连点两次加粗 = toggle off:第一次落地的「内容仍选中」经镜像回填
    /// 成第二次的输入;第二次归约前块表已因上次写回过期 —— 入口的
    /// `live.sync` 重同步让动作落在**新块表**上(连点链路)。
    #[test]
    fn live_format_toggles_off_on_second_click() {
        let mut state = State {
            render_mode: crate::live::RenderMode::Live,
            ..Default::default()
        };
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("甲乙丙\n\n后文");
        let tab = state.tabs.current_mut();
        tab.live.sync(&tab.editor, None);
        tab.live.active = Some(0);
        tab.selection = Some((0, 3));

        state.apply(Message::FormatRequested(FormatAction::Bold));
        assert_eq!(state.tabs.current().editor.text(), "**甲乙丙**\n\n后文");

        // 下一帧 UI 落地 + 镜像:块内选区 (2,5) 原样回填(块基 0,文档
        // 坐标同值)—— 与 live::ui 每帧镜像同一只手。
        state.tabs.current_mut().selection = Some((2, 5));
        state.apply(Message::FormatRequested(FormatAction::Bold));

        assert_eq!(
            state.tabs.current().editor.text(),
            "甲乙丙\n\n后文",
            "选区正被 ** 包着 → toggle off"
        );
    }

    /// 选区尚未被 UI 回填过(`None`)时按纯光标处理,不 panic、不越界。
    #[test]
    fn format_requested_without_selection_falls_back_to_caret() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("只有一行");
        assert_eq!(state.tabs.current().selection, None, "出厂尚未渲染过");

        state.apply(Message::FormatRequested(FormatAction::H2));
        let tab = state.tabs.current();
        assert_eq!(tab.editor.text(), "## 只有一行", "光标在 0 → 作用于首行");
        assert!(tab.pending_selection.is_some());
    }

    /// 格式动作只写当前标签:另一个标签的文本、预览快照、pending 都不动。
    #[test]
    fn format_requested_only_touches_active_tab() {
        let mut state = State::default();
        state.tabs.open_tab(None, "第二篇");
        assert_eq!(state.tabs.active, 1);
        state.tabs.current_mut().selection = Some((0, 3));

        state.apply(Message::FormatRequested(FormatAction::Bold));

        assert_eq!(state.tabs.tabs[1].editor.text(), "**第二篇**");
        assert!(state.tabs.tabs[1].pending_selection.is_some());
        assert_eq!(
            state.tabs.tabs[0].pending_selection, None,
            "另一个标签没收到待写回选区"
        );
        assert_eq!(
            state.tabs.tabs[1].pending_selection,
            Some((2, 5)),
            "字符偏移:三个汉字被包在两字符标记里"
        );
    }

    /// 图片框开框:alt 按选区预填(选中的文字就是替代文字),url 清空,
    /// `open` 置位 —— `compose::insert_image` 文档里「归约侧预填」的约定。
    #[test]
    fn image_dialog_open_prefills_alt_from_selection() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("甲乙丙丁");
        state.tabs.current_mut().selection = Some((0, 3));

        state.apply(Message::ImageDialogOpened);

        assert!(state.image_dialog.open);
        assert_eq!(state.image_dialog.alt, "甲乙丙");
        assert_eq!(state.image_dialog.url, "", "url 恒由用户填");
    }

    /// 图片框插入:`![alt](url)` 写入、新选区落在 alt 位(跳过 `![` 两个
    /// 字符)、对话框关闭并清空草稿;写回路径与格式动作同源。
    #[test]
    fn image_inserted_writes_markdown_and_closes_dialog() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("开头\n");
        state.tabs.current_mut().selection = Some((3, 3));

        state.apply(Message::ImageInserted {
            alt: "示意图".to_owned(),
            url: "https://x/y.png".to_owned(),
        });

        let tab = state.tabs.current();
        assert_eq!(tab.editor.text(), "开头\n![示意图](https://x/y.png)");
        assert_eq!(
            tab.pending_selection,
            Some((5, 8)),
            "新选区落在 alt 位(跳过 `![`),便于直接覆写"
        );
        assert!(!state.image_dialog.open);
        assert!(state.image_dialog.alt.is_empty() && state.image_dialog.url.is_empty());
    }

    /// 图片框插入的防御分支:url 为空(UI 已禁用按钮)不动文档,只关框。
    #[test]
    fn image_inserted_with_empty_url_touches_nothing() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("甲乙丙");
        state.image_dialog.open = true;

        state.apply(Message::ImageInserted {
            alt: String::new(),
            url: "  ".to_owned(),
        });

        assert_eq!(state.tabs.current().editor.text(), "甲乙丙");
        assert_eq!(state.tabs.current().pending_selection, None);
        assert!(!state.image_dialog.open);
    }

    /// 图片框取消:只关框清草稿,文档与选区不动。
    #[test]
    fn image_dialog_closed_touches_document_not() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("甲乙丙");
        state.image_dialog = ImageDialogState {
            open: true,
            alt: "示意图".to_owned(),
            url: "https://x/y.png".to_owned(),
            bed: Some("p-1".to_owned()),
        };

        state.apply(Message::ImageDialogClosed);

        assert!(!state.image_dialog.open);
        assert_eq!(state.image_dialog.alt, "");
        assert_eq!(state.tabs.current().editor.text(), "甲乙丙");
        assert_eq!(
            state.image_dialog.bed,
            Some("p-1".to_owned()),
            "关框保留图床选择,下次开框沿用"
        );
    }

    /// Emoji 面板开/关:开时清搜索词、分类沿用;关只翻 open。
    #[test]
    fn emoji_panel_toggle_keeps_group_clears_query() {
        let mut state = State::default();
        state.emoji.group = 3;
        state.emoji.query = "旧搜索".to_owned();

        state.apply(Message::EmojiPickerToggle(true));
        assert!(state.emoji.open);
        assert_eq!(state.emoji.group, 3, "分类沿用上次");
        assert_eq!(state.emoji.query, "", "搜索词每次开框重置");

        state.apply(Message::EmojiPickerToggle(false));
        assert!(!state.emoji.open);
        assert_eq!(
            state.tabs.current().editor.text(),
            SAMPLE_MD,
            "开关不动文档"
        );
    }

    /// Emoji 插入:字符写入、新选区 collapsed 落在 emoji 之后、面板关闭、
    /// 「最近使用」去重置顶(写回路径与格式动作同源)。
    #[test]
    fn emoji_inserted_writes_closes_panel_and_records_recent() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("开头\n中文");
        state.tabs.current_mut().selection = Some((3, 3));
        state.emoji.open = true;

        state.apply(Message::EmojiInserted("😀".to_owned()));

        let tab = state.tabs.current();
        assert_eq!(tab.editor.text(), "开头\n😀中文");
        assert_eq!(
            tab.pending_selection,
            Some((4, 4)),
            "光标 collapsed 落在 emoji 之后(字符偏移)"
        );
        assert!(!state.emoji.open, "点选插入后关闭面板");
        assert_eq!(state.emoji.recent, vec!["😀".to_owned()]);

        // 再点新的一枚、又点回旧的:去重、新的在前
        state.emoji.open = true;
        state.apply(Message::EmojiInserted("🚀".to_owned()));
        state.emoji.open = true;
        state.apply(Message::EmojiInserted("😀".to_owned()));
        assert_eq!(
            state.emoji.recent,
            vec!["😀".to_owned(), "🚀".to_owned()],
            "最近使用去重置顶"
        );
    }

    /// Emoji 插入的防御分支:空载荷(UI 不会产出)不动文档只关面板;
    /// 有选区时选中内容被替换(compose 层语义,这里钉归约侧连通)。
    #[test]
    fn emoji_inserted_defends_empty_payload_and_replaces_selection() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("甲乙丙");
        state.tabs.current_mut().selection = Some((0, 3));
        state.emoji.open = true;

        state.apply(Message::EmojiInserted(String::new()));
        assert_eq!(state.tabs.current().editor.text(), "甲乙丙", "空载荷不产文");
        assert!(!state.emoji.open);

        state.apply(Message::EmojiInserted("🚀".to_owned()));
        let tab = state.tabs.current();
        assert_eq!(tab.editor.text(), "🚀", "选中内容被替换");
        assert_eq!(tab.pending_selection, Some((1, 1)));
    }

    /// 「最近使用」上限(E2:16 枚):第 17 枚进来时最早的一枚被挤掉,
    /// 顺序保持新的在前。
    #[test]
    fn emoji_recent_truncates_to_cap() {
        let mut state = State::default();
        // 17 枚互不相同的载荷;每次插入都会关面板,故逐枚重开
        let batch = [
            "😀", "😃", "😄", "😁", "😆", "😅", "😂", "😉", "😊", "😍", "😘", "😋", "😛", "😜",
            "😏", "😒", "😬",
        ];
        assert_eq!(batch.len(), EMOJI_RECENT_CAP + 1);
        for emoji in batch {
            state.emoji.open = true;
            state.apply(Message::EmojiInserted(emoji.to_owned()));
        }
        assert_eq!(state.emoji.recent.len(), EMOJI_RECENT_CAP, "封顶截断");
        assert_eq!(state.emoji.recent[0], "😬", "最新的一枚在前");
        assert_eq!(
            state.emoji.recent[EMOJI_RECENT_CAP - 1],
            "😃",
            "最早的一枚(😀)被挤掉"
        );
    }

    /// 「最近使用」持久化(E2,docs/emoji-plan.md §6.3):插入即随主题
    /// 路径落 settings.json;重启(main 装载 ThemeSettings → load_preferences
    /// 回装)后顺序保持。
    #[test]
    fn emoji_recent_persists_and_reloads() {
        let dir = temp_path("emoji-recent");
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        for emoji in ["🚀", "😀"] {
            state.emoji.open = true;
            state.apply(Message::EmojiInserted(emoji.to_owned()));
        }
        assert!(state.tabs.current().document.notice.is_none(), "落盘无提示");

        let json = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            json.contains("emoji_recent"),
            "随 settings.json 落盘:{json}"
        );
        let reloaded_theme = ThemeSettings::load_from(&dir).unwrap();
        assert_eq!(
            reloaded_theme.emoji_recent,
            vec!["😀".to_owned(), "🚀".to_owned()],
            "重启装载:新的在前"
        );

        // 重启路径复现(main 的 LaterMdApp::new:先装 theme 再 load_preferences)
        let mut restarted = State {
            theme: reloaded_theme,
            settings_dir: Some(dir.clone()),
            ..State::default()
        };
        restarted.load_preferences();
        assert_eq!(
            restarted.emoji.recent,
            vec!["😀".to_owned(), "🚀".to_owned()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 浏览本地图片(B 段):复制进 `<doc名>.assets/`,url 回填相对地址,
    /// 空.alt 补文件名(去扩展名);对话框保持打开,文档一字未动。
    #[test]
    fn image_file_import_copies_and_backfills_draft() {
        let dir = temp_path("image-pick");
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("笔记.md");
        std::fs::write(&doc, "# 笔记").unwrap();
        let source = dir.join("截图.png");
        std::fs::write(&source, b"png-bytes").unwrap();

        let mut state = State::default();
        state.tabs.current_mut().document.path = Some(doc.clone());
        state.image_dialog.open = true;
        state.import_image_file(&source);

        assert_eq!(
            std::fs::read(dir.join("笔记.assets/截图.png")).unwrap(),
            b"png-bytes".to_vec(),
            "文件复制进资产目录"
        );
        assert_eq!(state.image_dialog.url, "./笔记.assets/截图.png");
        assert_eq!(state.image_dialog.alt, "截图", "空 alt 补文件名(去扩展名)");
        assert!(state.image_dialog.open, "对话框保持打开等「插入」");
        assert_eq!(state.tabs.current().editor.text(), SAMPLE_MD, "文档未动");

        // 已有 alt(选区预填/手填)不被文件名覆盖
        state.image_dialog.alt = "手填的说明".to_owned();
        let second = dir.join("截图.png"); // 撞名 → -1
        state.import_image_file(&second);
        assert_eq!(state.image_dialog.url, "./笔记.assets/截图-1.png");
        assert_eq!(state.image_dialog.alt, "手填的说明");

        // 回填的相对地址走「插入」:全链路(浏览 → 复制 → 插入)落进文档
        state.apply(Message::ImageInserted {
            alt: state.image_dialog.alt.clone(),
            url: state.image_dialog.url.clone(),
        });
        assert!(
            state
                .tabs
                .current()
                .editor
                .text()
                .contains("![手填的说明](./笔记.assets/截图-1.png)"),
            "插入的是回填的相对地址"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 未落盘文档点「浏览…」:只提示不弹框不复制(`.assets/` 与文档同目录,
    /// 没有目录就没有锚点)—— 判定在 rfd 之前,无头环境可测完整消息。
    #[test]
    fn image_file_pick_on_unsaved_doc_only_notices() {
        let mut state = State::default();
        state.image_dialog.open = true;

        state.apply(Message::ImageFilePickRequested);

        let notice = state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .unwrap_or("");
        assert!(notice.contains("保存"), "提示指路先保存:{notice}");
        assert_eq!(state.image_dialog.url, "", "草稿未被回填");
        assert!(state.image_dialog.open, "对话框保持打开");
    }

    /// 图床上传收尾(C 段归约层,免网络):成功按 Insert 用途把 URL 写进
    /// **发起标签**(选区落 alt 位),失败只落 notice、文档与选区一字不动
    /// (image-plan §4.3)。发起标签已关则结果丢弃。
    #[test]
    fn upload_finish_inserts_on_success_and_only_notices_on_failure() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("正文");
        let tab_id = state.tabs.current().id;
        // 伪造收尾消息:bed 层(发起线程/seq/channel)已由 bed.rs 单测覆盖,
        // 这里钉归约侧对结果的处置 —— 直接调归约入口
        let url = "https://cdn.example/x.png".to_owned();
        state.bed.upload_seq = 1;
        state.bed.purpose = Some(crate::bed::BedUploadPurpose::Insert {
            alt: "示意".to_owned(),
        });
        state.bed.upload_tab = Some(tab_id);
        // 手动造在途态(rx 空壳),finish 只看 seq
        state.bed.rx = Some(std::sync::mpsc::channel::<crate::bed::UploadResult>().1);

        state.apply(Message::ImageUploadFinished {
            seq: 1,
            result: Ok(url.clone()),
        });
        assert!(
            state
                .tabs
                .current()
                .editor
                .text()
                .contains("![示意](https://cdn.example/x.png)"),
            "成功插入发起标签"
        );
        assert!(!state.bed.is_uploading());

        // 失败路径:新标签、新请求,失败 → 文档不动,只 notice
        state.tabs.current_mut().editor.replace_all("干净文档");
        state.bed.upload_seq = 2;
        state.bed.purpose = Some(crate::bed::BedUploadPurpose::Insert {
            alt: "x".to_owned(),
        });
        state.bed.upload_tab = Some(state.tabs.current().id);
        state.bed.rx = Some(std::sync::mpsc::channel::<crate::bed::UploadResult>().1);
        state.apply(Message::ImageUploadFinished {
            seq: 2,
            result: Err("图床返回 HTTP 401:bad token".to_owned()),
        });
        assert_eq!(
            state.tabs.current().editor.text(),
            "干净文档",
            "失败不改文档"
        );
        assert_eq!(state.tabs.current().selection, None, "选区不动");
        assert_eq!(
            state.tabs.current().document.notice.as_deref(),
            Some("图床返回 HTTP 401:bad token")
        );
        assert!(!state.bed.is_uploading(), "失败同样收尾");
    }

    /// 旧 seq 的结果静默丢弃(防旧覆盖,归约层):文档、notice、在途状态
    /// 一概不动 —— 旧结果连提示都不该弹。
    #[test]
    fn stale_upload_result_is_silently_dropped() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("旧");
        state.bed.upload_seq = 2; // 当前最新序号是 2(第 1 次的结果迟到)
        state.bed.purpose = None;
        state.apply(Message::ImageUploadFinished {
            seq: 1,
            result: Ok("https://cdn/late.png".to_owned()),
        });
        assert_eq!(state.tabs.current().editor.text(), "旧");
        assert_eq!(state.tabs.current().document.notice, None, "不弹提示");
    }

    /// 发起标签已关:结果无处可写,丢弃且不 panic、不留 notice。
    #[test]
    fn upload_result_for_closed_tab_is_dropped() {
        let mut state = State::default();
        state.bed.upload_seq = 1;
        state.bed.purpose = Some(crate::bed::BedUploadPurpose::Insert {
            alt: "a".to_owned(),
        });
        state.bed.upload_tab = Some(9999); // 不存在的标签 id
        state.bed.rx = Some(std::sync::mpsc::channel::<crate::bed::UploadResult>().1);
        state.apply(Message::ImageUploadFinished {
            seq: 1,
            result: Ok("https://cdn/x.png".to_owned()),
        });
        assert_eq!(
            state.tabs.current().editor.text(),
            SAMPLE_MD,
            "写不进任何别的标签"
        );
        assert!(!state.bed.is_uploading(), "在途状态照常收口");
    }

    /// 测试上传收尾:结果回显设置页(last_test),不插入任何文档。
    #[test]
    fn test_upload_result_only_echoes_in_settings() {
        let mut state = State::default();
        state.bed.upload_seq = 3;
        state.bed.purpose = Some(crate::bed::BedUploadPurpose::Test {
            profile_name: "我的 SM.MS".to_owned(),
        });
        state.bed.upload_tab = None;
        state.bed.rx = Some(std::sync::mpsc::channel::<crate::bed::UploadResult>().1);
        let before = state.tabs.current().editor.text().to_owned();
        state.apply(Message::ImageUploadFinished {
            seq: 3,
            result: Ok("https://cdn/t.png".to_owned()),
        });
        assert_eq!(state.tabs.current().editor.text(), before, "文档不动");
        assert_eq!(
            state.bed.last_test,
            Some(("我的 SM.MS".to_owned(), Ok("https://cdn/t.png".to_owned())))
        );
    }

    /// 图床配置保存/删除(归约层):upsert 落 beds.json,token 从不落盘;
    /// 删除同步移除条目。落盘目录用注入的 settings_dir(无头可测)。
    #[test]
    fn bed_profile_save_and_delete_persist_without_tokens() {
        let dir = temp_path("beds-persist");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut state = State {
            settings_dir: Some(dir.clone()),
            ..State::default()
        };

        // 新增:归一化生效(名次空白被 trim),id 由归约分配
        let mut profile = latermd_bed::BedProfile::preset_smms();
        profile.id = String::new();
        profile.name = "  我的 SM.MS  ".to_owned();
        state.apply(Message::BedProfileSaved {
            profile: profile.clone(),
            token: Some("placeholder-token".to_owned()),
        });
        assert_eq!(state.bed.profiles.len(), 1);
        let saved = state.bed.profiles[0].clone();
        assert_eq!(saved.name, "我的 SM.MS");
        assert!(!saved.id.is_empty(), "新 profile 分配了 id");
        let raw = std::fs::read_to_string(dir.join("beds.json")).unwrap();
        assert!(!raw.contains("placeholder-token"), "token 绝不落盘:{raw}");
        assert!(!raw.contains("token"), "没有 token 字段:{raw}");
        assert!(raw.contains("${TOKEN}"), "占位符原样保留");

        // 修改:同名 id upsert,不新增条目
        let mut edited = saved.clone();
        edited.name = "改名".to_owned();
        state.apply(Message::BedProfileSaved {
            profile: edited,
            token: None,
        });
        assert_eq!(state.bed.profiles.len(), 1, "upsert 不新增");
        assert_eq!(state.bed.profiles[0].name, "改名");

        // 删除:条目移除,beds.json 同步
        state.apply(Message::BedProfileDeleted {
            id: state.bed.profiles[0].id.clone(),
        });
        assert!(state.bed.profiles.is_empty());
        let raw = std::fs::read_to_string(dir.join("beds.json")).unwrap();
        assert_eq!(raw, "[]");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 未配图床就点上传:指路设置页的提示,文档与对话框不动。
    #[test]
    fn upload_without_profile_only_notices() {
        let mut state = State::default();
        state.image_dialog.open = true;
        state.image_dialog.bed = None;

        state.apply(Message::ImageUploadRequested);

        let notice = state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .unwrap_or("");
        assert!(notice.contains("设置 → 图片"), "指路设置:{notice}");
        assert!(state.image_dialog.open, "对话框不动");
        assert!(!state.bed.is_uploading());
    }

    // —— D 段归约(docs/image-plan.md §3.D)——

    /// 剪贴板粘贴收尾(D 段归约层):PNG 字节落 `.assets/`(合成名
    /// `粘贴图片-<时间戳>`),光标处插**空 alt** 的相对路径引用;剪贴板
    /// 读取线程由 clipboard.rs 单测覆盖,这里钉归约对结果的处置。
    #[test]
    fn paste_finish_stores_bytes_and_inserts_relative_reference() {
        let dir = temp_path("paste-finish");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("笔记.md");
        let mut state = State::default();
        state.tabs.current_mut().document.path = Some(doc.clone());
        state.tabs.current_mut().editor.replace_all("正文");
        state.tabs.current_mut().selection = Some((2, 2));
        // 手动造在途态:finish 只收口接收端
        state.clipboard.rx = Some(std::sync::mpsc::channel::<Result<Vec<u8>, String>>().1);

        state.apply(Message::ImagePasteFinished {
            result: Ok(b"png-bytes".to_vec()),
        });

        let text = state.tabs.current().editor.text();
        let expected = "正文![](./笔记.assets/粘贴图片-";
        assert!(
            text.starts_with(expected),
            "空 alt + 合成名相对引用插在光标处:{text}"
        );
        assert!(text.ends_with(".png)"), "扩展名恒 png:{text}");
        // 文件真的落了盘,字节原样
        let stored = std::fs::read_dir(dir.join("笔记.assets")).unwrap();
        let files: Vec<_> = stored
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files.len(), 1, "恰一份落盘:{files:?}");
        let saved = std::fs::read(dir.join("笔记.assets").join(&files[0])).unwrap();
        assert_eq!(saved, b"png-bytes".to_vec());
        assert!(!state.clipboard.is_reading(), "收尾后空闲");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 剪贴板读取失败:只落 notice,文档、选区、pending_selection 一字不动
    /// (image-plan §4.3 同口径:失败的唯一后果是不插入文本)。
    #[test]
    fn paste_failure_only_notices() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("原样");
        state.tabs.current_mut().selection = Some((0, 2));
        state.clipboard.rx = Some(std::sync::mpsc::channel::<Result<Vec<u8>, String>>().1);

        state.apply(Message::ImagePasteFinished {
            result: Err("剪贴板里没有可用的图片".to_owned()),
        });

        assert_eq!(state.tabs.current().editor.text(), "原样");
        assert_eq!(state.tabs.current().selection, Some((0, 2)), "选区不动");
        assert_eq!(state.tabs.current().pending_selection, None);
        assert_eq!(
            state.tabs.current().document.notice.as_deref(),
            Some("剪贴板里没有可用的图片")
        );
        assert!(!state.clipboard.is_reading());
    }

    /// 命令行安装请求(非 .app 运行,测试进程必是裸二进制):只落提示,
    /// 不起安装线程。两平台 CI 的测试进程都不满足 bundle 判定,断言
    /// 因此确定可复现。
    #[test]
    fn install_cli_request_outside_bundle_only_notices() {
        let mut state = State::default();
        state.apply(Message::InstallCliRequested);
        assert!(!state.cli_install.is_installing(), "裸二进制不发起安装线程");
        let notice = state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .unwrap_or("");
        assert!(notice.contains(".app"), "提示指向 .app 安装运行:{notice}");
    }

    /// 命令行安装收尾:成功/失败都只落提示行,接收端清空、编辑状态
    /// 一字不动(与图床上传失败同口径)。
    #[test]
    fn install_cli_finish_only_notices_both_ways() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("原样");
        state.cli_install.rx = Some(std::sync::mpsc::channel::<Result<(), String>>().1);

        state.apply(Message::CliInstallFinished { result: Ok(()) });
        assert!(!state.cli_install.is_installing(), "收尾清接收端");
        let notice = state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .unwrap_or("");
        assert!(
            notice.contains("/usr/local/bin") && notice.contains("latermd"),
            "成功提示指路:{notice}"
        );

        state.apply(Message::CliInstallFinished {
            result: Err("已取消授权,未安装".to_owned()),
        });
        assert_eq!(
            state.tabs.current().document.notice.as_deref(),
            Some("命令行工具安装失败:已取消授权,未安装")
        );
        assert_eq!(state.tabs.current().editor.text(), "原样", "文档不动");
    }

    /// 超过 5MB 的剪贴板图片:拒绝并提示,不落盘(字节入口的上限判定)。
    #[test]
    fn oversized_paste_is_rejected_without_writing() {
        let dir = temp_path("paste-oversize");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("d.md");
        let mut state = State::default();
        state.tabs.current_mut().document.path = Some(doc.clone());
        state.tabs.current_mut().editor.replace_all("内容");
        state.clipboard.rx = Some(std::sync::mpsc::channel::<Result<Vec<u8>, String>>().1);

        let big = vec![0u8; crate::assets::MAX_IMAGE_BYTES as usize + 1];
        state.apply(Message::ImagePasteFinished { result: Ok(big) });

        assert_eq!(state.tabs.current().editor.text(), "内容", "不插");
        let notice = state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .unwrap_or("");
        assert!(notice.contains("超过 5 MB"), "{notice}");
        assert!(!dir.join("d.assets").exists(), "不留半个资产目录");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 未落盘文档粘贴图片:提示先保存(`.assets/` 要与文档同目录),
    /// 不插入、不落盘。剪贴板读取照常收口(下次粘贴不受卡)。
    #[test]
    fn paste_on_unsaved_doc_only_notices() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("草稿");
        state.clipboard.rx = Some(std::sync::mpsc::channel::<Result<Vec<u8>, String>>().1);

        state.apply(Message::ImagePasteFinished {
            result: Ok(b"png".to_vec()),
        });

        assert_eq!(state.tabs.current().editor.text(), "草稿");
        let notice = state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .unwrap_or("");
        assert!(notice.contains("保存"), "{notice}");
        assert!(!state.clipboard.is_reading());
    }

    /// 拖入图片文件(D 段归约层):白名单内 → 读盘 → 复制进 `.assets/`
    /// (原名)→ 光标处插空 alt 相对引用;文件在资产目录里时直接复用
    /// (import_file 的既有语义,拖拽不该堆副本)。
    #[test]
    fn dropped_image_stores_and_inserts_reference() {
        let dir = temp_path("drop-image");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("笔记.md");
        std::fs::write(&doc, "# x").unwrap();
        let source = dir.join("拖入图.png");
        std::fs::write(&source, b"dropped-bytes").unwrap();
        let mut state = State::default();
        state.tabs.current_mut().document.path = Some(doc.clone());
        state.tabs.current_mut().editor.replace_all("行尾");
        state.tabs.current_mut().selection = Some((2, 2));

        state.apply(Message::ImageFileDropped(source.clone()));

        assert_eq!(
            state.tabs.current().editor.text(),
            "行尾![](./笔记.assets/拖入图.png)"
        );
        assert_eq!(
            std::fs::read(dir.join("笔记.assets/拖入图.png")).unwrap(),
            b"dropped-bytes".to_vec()
        );
        assert_eq!(state.tabs.current().document.notice, None, "成功不弹提示");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 拖入白名单外文件(归约侧兜底:layout 过滤在 extension 层,这里钉
    /// 归约对扩展名判定失败路径的行为):提示点名格式,不读盘不落盘。
    #[test]
    fn dropped_non_image_only_notices() {
        let dir = temp_path("drop-nonimage");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("d.md");
        let stray = dir.join("说明.txt");
        std::fs::write(&stray, b"text").unwrap();
        let mut state = State::default();
        state.tabs.current_mut().document.path = Some(doc);
        state.tabs.current_mut().editor.replace_all("原文");

        state.apply(Message::ImageFileDropped(stray));

        assert_eq!(state.tabs.current().editor.text(), "原文");
        let notice = state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .unwrap_or("");
        assert!(notice.contains("txt"), "{notice}");
        assert!(!dir.join("d.assets").exists(), "不留半个资产目录");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 拖入超 5MB 图片:提示带实际大小,不落盘不插入。
    #[test]
    fn dropped_oversized_image_only_notices() {
        let dir = temp_path("drop-oversize");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("d.md");
        let big = dir.join("大图.png");
        std::fs::write(&big, vec![0u8; crate::assets::MAX_IMAGE_BYTES as usize + 1]).unwrap();
        let mut state = State::default();
        state.tabs.current_mut().document.path = Some(doc);
        state.tabs.current_mut().editor.replace_all("原文");

        state.apply(Message::ImageFileDropped(big));

        assert_eq!(state.tabs.current().editor.text(), "原文");
        let notice = state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .unwrap_or("");
        assert!(notice.contains("超过 5 MB"), "{notice}");
        assert!(!dir.join("d.assets").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 未落盘文档拖入图片:提示先保存,不弹别的。
    #[test]
    fn drop_on_unsaved_doc_only_notices() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("草稿");

        state.apply(Message::ImageFileDropped(PathBuf::from("/tmp/x.png")));

        let notice = state
            .tabs
            .current()
            .document
            .notice
            .as_deref()
            .unwrap_or("");
        assert!(notice.contains("保存"), "{notice}");
        assert_eq!(state.tabs.current().editor.text(), "草稿");
    }

    /// 替换行开/关(#17 M1):Ctrl+H 查找条同开替换行展开,再按收替换行
    /// 留查找条;Esc 关整条时替换行一并收起;Ctrl+F 不带出替换行。
    #[test]
    fn replace_bar_toggle_semantics() {
        let mut state = State::default();
        assert!(!state.find.open && !state.find.replace_open);

        state.apply(Message::ReplaceBarToggled(true));
        assert!(
            state.find.open && state.find.replace_open,
            "Ctrl+H:查找条同开、替换行展开"
        );

        state.apply(Message::ReplaceBarToggled(true));
        assert!(
            state.find.open && !state.find.replace_open,
            "再按 Ctrl+H:收替换行,查找条保留"
        );

        state.apply(Message::ReplaceBarToggled(true));
        assert!(state.find.open && state.find.replace_open);

        state.apply(Message::FindBarToggled(false));
        assert!(
            !state.find.open && !state.find.replace_open,
            "Esc 关整条,替换行随之收起"
        );

        state.apply(Message::FindBarToggled(true));
        assert!(
            state.find.open && !state.find.replace_open,
            "Ctrl+F 打开不带替换行"
        );
    }

    /// 跳转浮条开/关与互斥(#60 M1):Ctrl+G 开浮条清草稿并收查找条;
    /// Ctrl+F 开查找条收浮条;Esc 关浮条不动查找条(本来就关)。
    #[test]
    fn goto_bar_toggle_semantics_and_mutual_exclusion() {
        let mut state = State::default();
        assert!(!state.goto.open);

        // 开浮条:草稿清空、查找条收起
        state.find.open = true;
        state.find.replace_open = true;
        state.goto.input.push_str("99");
        state.apply(Message::GotoBarToggled(true));
        assert!(state.goto.open, "Ctrl+G 开浮条");
        assert!(state.goto.input.is_empty(), "打开即清行号草稿");
        assert!(
            !state.find.open && !state.find.replace_open && state.find.hit.is_none(),
            "互斥:开浮条收起查找条(含替换行)"
        );

        // 反向互斥:开查找条收浮条
        state.apply(Message::FindBarToggled(true));
        assert!(state.find.open && !state.goto.open, "Ctrl+F 收浮条");

        // Esc 关浮条:关是普通关闭,不连带翻转查找条
        state.apply(Message::GotoBarToggled(true));
        state.apply(Message::GotoBarToggled(false));
        assert!(!state.goto.open);
        assert!(!state.find.open, "浮条与查找条仍互斥,关浮条不再开别的");
    }

    /// 跳转语义(#60 M1):1 起行号钳制(0 → 第 1 行、超尾 → 最后一行)、
    /// 光标落该行**行首**、空文档跳第 1 行;跳转不写文本(修订号不动),
    /// 跳成后浮条关闭并清草稿。
    #[test]
    fn goto_line_clamps_and_lands_at_line_start() {
        let mut state = State::default();
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("alpha\nbeta\ngamma\n");
        state.tabs.current_mut().editor.clear_dirty();
        {
            let editor = &state.tabs.current().editor;
            assert_eq!(editor.line_to_byte(1), 6, "前置:第 2 行行首字节");
            assert_eq!(editor.line_to_byte(3), 17, "前置:第 4 行(末尾空行)行首字节");
        }

        let jump = |state: &mut State, line: usize| -> Option<usize> {
            state.goto.open = true;
            state.goto.input = line.to_string();
            let rev = state.tabs.current().editor.revision();
            state.apply(Message::GotoLineRequested { line });
            assert_eq!(state.tabs.current().editor.revision(), rev, "跳转不写文本");
            assert!(
                !state.goto.open && state.goto.input.is_empty(),
                "跳成即关浮条"
            );
            state.tabs.current().cursor.jump_to
        };

        assert_eq!(jump(&mut state, 2), Some(6), "第 2 行 → 行首字符偏移");
        assert_eq!(jump(&mut state, 0), Some(0), "0 越界钳到第 1 行行首");
        assert_eq!(jump(&mut state, 99), Some(17), "超尾钳到最后一行行首");
        assert_eq!(jump(&mut state, 1), Some(0), "第 1 行行首");
        assert!(
            state.tabs.current().editor.text().starts_with("alpha"),
            "文档未动"
        );

        // 空文档:总行数 1,任何载荷都跳第 1 行(偏移 0)。默认标签带
        // SAMPLE_MD,先显式清空再验。
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("");
        state.apply(Message::GotoLineRequested { line: 7 });
        assert_eq!(
            state.tabs.current().cursor.jump_to,
            Some(0),
            "空文档钳到第 1 行行首"
        );
        assert!(!state.goto.open, "空文档跳转同样关闭浮条");
    }

    // —— 插入/更新目录(#66 M2,Message::InsertToc 归约)——

    /// 首次执行:TOC 块(标记包围)插到文档头,块与正文之间补一个空行;
    /// dirty 置位走正常保存链路,光标折叠到块首(`jump_to` 通道),不落
    /// 提示。CJK 标题的锚点为 M1 slug 口径。
    #[test]
    fn insert_toc_first_run_inserts_block_at_head() {
        let mut state = State::default();
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("中文前言\n\n# 甲标题\n\n正文。\n\n## 乙标题\n");
        state.tabs.current_mut().editor.clear_dirty();

        state.apply(Message::InsertToc);

        let tab = state.tabs.current();
        assert_eq!(
            tab.editor.text(),
            "<!-- TOC -->\n- [甲标题](#甲标题)\n  - [乙标题](#乙标题)\n<!-- /TOC -->\n\n中文前言\n\n# 甲标题\n\n正文。\n\n## 乙标题\n",
            "首插 = 块落文档头 + 补空行隔开正文,原文零改动"
        );
        assert!(tab.editor.is_dirty(), "TOC 写入置位 dirty(正常保存链路)");
        assert_eq!(tab.cursor.jump_to, Some(0), "光标折叠到块首");
        assert_eq!(tab.document.notice, None, "成功不落提示");
    }

    /// 再执行:识别既有块**整块替换**,不堆叠第二份;标题变动后内容随之
    /// 更新,块外字节零增删;内容已同步时再执行幂等(同内容替换零漂移)。
    #[test]
    fn insert_toc_second_run_replaces_block_and_follows_heading_changes() {
        let mut state = State::default();
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("# 旧标题\n\n正文。\n\n## 小节\n");
        state.apply(Message::InsertToc);
        let first = state.tabs.current().editor.text().to_owned();
        assert_eq!(
            first,
            "<!-- TOC -->\n- [旧标题](#旧标题)\n  - [小节](#小节)\n<!-- /TOC -->\n\n# 旧标题\n\n正文。\n\n## 小节\n",
            "前置:首插形态"
        );

        // 改标题后原样再执行:块内容更新,文档里始终只有一对标记
        state
            .tabs
            .current_mut()
            .editor
            .replace_all(&first.replace("旧标题", "新标题"));
        state.apply(Message::InsertToc);
        let text = state.tabs.current().editor.text().to_owned();
        assert!(
            text.contains("- [新标题](#新标题)"),
            "TOC 内容随标题更新:{text}"
        );
        assert!(!text.contains("旧标题"), "旧锚点整块消失");
        assert_eq!(text.matches("<!-- TOC -->").count(), 1, "替换不堆叠新块");
        assert_eq!(
            text,
            "<!-- TOC -->\n- [新标题](#新标题)\n  - [小节](#小节)\n<!-- /TOC -->\n\n# 新标题\n\n正文。\n\n## 小节\n",
            "块外字节(正文与标题行)零增删"
        );

        // 内容已同步时再执行:幂等
        state.apply(Message::InsertToc);
        assert_eq!(state.tabs.current().editor.text(), text, "同内容替换零漂移");
    }

    /// 手工改过 TOC 区域再执行 = 整块覆盖(#127 口径):标记之间的手工内容
    /// 不保留,块外正文(含块后既有空行)不受影响。
    #[test]
    fn insert_toc_overwrites_hand_edited_region_wholesale() {
        let mut state = State::default();
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("# 甲\n\n<!-- TOC -->\n手写的目录行(不想保留)\n<!-- /TOC -->\n\n正文。\n");

        state.apply(Message::InsertToc);

        assert_eq!(
            state.tabs.current().editor.text(),
            "# 甲\n\n<!-- TOC -->\n- [甲](#甲)\n<!-- /TOC -->\n\n正文。\n",
            "手工内容整块覆盖;块在原位替换,块前标题与块后正文(含既有空行)不动"
        );
    }

    /// 无可收录标题:只落提示行,文档/修订号/光标一动不动。全无标题与
    /// 「只有四级及更深标题」两种提示分开(#127)。
    #[test]
    fn insert_toc_without_usable_headings_only_notices() {
        let mut state = State::default();
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("只有段落,没有标题。\n");
        state.tabs.current_mut().editor.clear_dirty();

        state.apply(Message::InsertToc);

        let tab = state.tabs.current();
        assert_eq!(
            tab.document.notice,
            Some("文档没有标题,无法生成目录".to_owned())
        );
        assert_eq!(tab.editor.text(), "只有段落,没有标题。\n", "文档不动");
        assert!(!tab.editor.is_dirty(), "提示路径不置脏");
        assert_eq!(tab.cursor.jump_to, None, "不发生跳转");

        // 只有一级 + 四级:大纲非空,但深度内只有一级可收录 —— 有标题可
        // 收录,正常生成(对照:四级单独存在才落第二种提示)
        let mut state = State::default();
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("#### 只有四级小节\n");
        state.apply(Message::InsertToc);
        assert_eq!(
            state.tabs.current().document.notice,
            Some("目录只收录一至三级标题,当前文档没有可收录的标题".to_owned()),
            "深度内无可收录条目,提示与全无标题分开"
        );
        assert_eq!(
            state.tabs.current().editor.text(),
            "#### 只有四级小节\n",
            "文档不动"
        );
    }

    /// 单个替换(#17 M1):定点改写文本、置脏推进修订号、重扫计数回落、
    /// hit 推进到下一个并经 `pending_selection` 跟过去;全部替换完清定位。
    #[test]
    fn replace_current_rewrites_text_advances_hit_and_rescans() {
        let mut state = State::default();
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("needle one\nneedle two\nneedle three");
        state.tabs.current_mut().editor.clear_dirty();
        state.apply(Message::FindBarToggled(true));
        state.apply(Message::FindQueryChanged("needle".to_owned()));
        assert_eq!(state.find.hits.len(), 3);
        state.find.replacement = "pin".to_owned();

        let rev = state.tabs.current().editor.revision();
        state.apply(Message::ReplaceCurrent);
        assert_eq!(
            state.tabs.current().editor.text(),
            "pin one\nneedle two\nneedle three"
        );
        assert!(state.tabs.current().editor.is_dirty(), "替换置脏");
        assert!(
            state.tabs.current().editor.revision() > rev,
            "修订号推进(预览快照联动)"
        );
        assert_eq!(state.find.hits.len(), 2, "重扫计数回落");
        assert_eq!(state.find.hit, Some(0), "定位推进到下一个命中");
        assert_eq!(
            state.tabs.current().pending_selection,
            Some((8, 14)),
            "选区跟到下一个命中"
        );

        state.apply(Message::ReplaceCurrent);
        assert_eq!(
            state.tabs.current().editor.text(),
            "pin one\npin two\nneedle three"
        );
        assert_eq!(state.find.hit, Some(0));

        state.apply(Message::ReplaceCurrent);
        assert_eq!(
            state.tabs.current().editor.text(),
            "pin one\npin two\npin three"
        );
        assert_eq!(state.find.hits.len(), 0, "全部替换完清空");
        assert_eq!(state.find.hit, None);
    }

    /// 最后一个命中替换后定位环绕回首(「替换并跳下一个」的末尾语义)。
    #[test]
    fn replace_current_wraps_to_first_after_last_hit() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("a X b X c X d");
        state.apply(Message::FindBarToggled(true));
        state.apply(Message::FindQueryChanged("X".to_owned()));
        state.apply(Message::FindNext { backwards: false });
        state.apply(Message::FindNext { backwards: false });
        assert_eq!(state.find.hit, Some(2));
        state.find.replacement = "Y".to_owned();

        state.apply(Message::ReplaceCurrent);
        assert_eq!(state.tabs.current().editor.text(), "a X b X c Y d");
        assert_eq!(state.find.hit, Some(0), "末尾替换后无更晚命中,环绕回首");
        assert_eq!(state.tabs.current().pending_selection, Some((2, 3)));
    }

    /// 替换词自身含查找词(needle → needleX):锚点取替换词末尾,刚写入
    /// 的命中不被再次选中,定位推进到真正的下一个。
    #[test]
    fn replace_current_skips_replacement_containing_query() {
        let mut state = State::default();
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("needle one\nneedle two");
        state.apply(Message::FindBarToggled(true));
        state.apply(Message::FindQueryChanged("needle".to_owned()));
        state.find.replacement = "needleX".to_owned();

        state.apply(Message::ReplaceCurrent);
        assert_eq!(
            state.tabs.current().editor.text(),
            "needleX one\nneedle two"
        );
        // 新文本里 needleX 的前缀也算命中,但 hit 必须跳过它
        assert_eq!(state.find.hits.len(), 2);
        assert_eq!(state.find.hit, Some(1), "不回吸刚写入的命中");
        assert_eq!(state.tabs.current().pending_selection, Some((12, 18)));
    }

    /// 无命中时单个替换是幂等无操作:不改文本、不推进修订号。
    #[test]
    fn replace_current_without_hits_is_noop() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("稳定文本");
        state.apply(Message::FindBarToggled(true));
        state.apply(Message::FindQueryChanged("zzz".to_owned()));
        assert_eq!(state.find.hit, None);
        state.find.replacement = "别的".to_owned();

        let rev = state.tabs.current().editor.revision();
        state.apply(Message::ReplaceCurrent);
        assert_eq!(state.tabs.current().editor.text(), "稳定文本");
        assert_eq!(state.tabs.current().editor.revision(), rev);
    }

    /// 全部替换(#17 M1 基础版):按命中一次写入(修订号恰 +1)、大小写
    /// 不敏感全量覆盖、多字节替换词不错位、随后重扫。
    #[test]
    fn replace_all_rewrites_once_and_rescans() {
        let mut state = State::default();
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("foo a\nfoo b\nFOO c");
        state.apply(Message::FindBarToggled(true));
        state.apply(Message::FindQueryChanged("foo".to_owned()));
        assert_eq!(state.find.hits.len(), 3);
        state.find.replacement = "bar".to_owned();

        let rev = state.tabs.current().editor.revision();
        state.apply(Message::ReplaceAllInDoc);
        assert_eq!(state.tabs.current().editor.text(), "bar a\nbar b\nbar c");
        assert_eq!(
            state.tabs.current().editor.revision(),
            rev + 1,
            "单次写入:整篇一次 replace_all,一次 touch"
        );
        assert_eq!(state.find.hits.len(), 0, "替换完重扫无残留");
        assert!(state.tabs.current().editor.is_dirty());

        // CJK:查找词与替换词都是多字节,偏移不错位
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("中文 换行\n替换 换行");
        state.apply(Message::FindQueryChanged("换行".to_owned()));
        assert_eq!(state.find.hits.len(), 2);
        state.find.replacement = "行尾".to_owned();
        state.apply(Message::ReplaceAllInDoc);
        assert_eq!(state.tabs.current().editor.text(), "中文 行尾\n替换 行尾");
    }

    /// 全部替换的偏移换算(#17 M2):查找词/替换词含 CJK(3 字节)与
    /// emoji(4 字节)时**逐字符**断言结果 —— 字节口径错位会切坏相邻
    /// 字符,字符向量一个不差才算对;并覆盖「替换比查找长」方向(后续
    /// 命中偏移整体推移)与替换后仍有尾随内容的情形。
    #[test]
    fn replace_all_multibyte_offsets_are_char_accurate() {
        let mut state = State::default();
        // 命中跨「文(3B)+ 🎉(4B)」= 字节 3..10 的非对齐区间,替换词同构
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("文🎉甲 文🎉乙 文🎉丙");
        state.apply(Message::FindBarToggled(true));
        state.apply(Message::FindQueryChanged("文🎉".to_owned()));
        assert_eq!(state.find.hits.len(), 3);
        state.find.replacement = "字✨".to_owned();
        state.apply(Message::ReplaceAllInDoc);
        assert_eq!(
            state
                .tabs
                .current()
                .editor
                .text()
                .chars()
                .collect::<Vec<_>>(),
            "字✨甲 字✨乙 字✨丙".chars().collect::<Vec<_>>(),
            "CJK+emoji 混排逐字符正确"
        );

        // 替换比查找长(中 3B → 日本語 9B):后续命中与尾随内容不错位
        state.tabs.current_mut().editor.replace_all("中a中b中c尾巴");
        state.apply(Message::FindQueryChanged("中".to_owned()));
        assert_eq!(state.find.hits.len(), 3);
        state.find.replacement = "日本語".to_owned();
        state.apply(Message::ReplaceAllInDoc);
        assert_eq!(
            state
                .tabs
                .current()
                .editor
                .text()
                .chars()
                .collect::<Vec<_>>(),
            "日本語a日本語b日本語c尾巴".chars().collect::<Vec<_>>(),
            "增长替换后的命中与尾随内容逐字符正确"
        );

        // 反向:emoji 查找词收缩为单字节 ASCII(4B → 1B)
        state.tabs.current_mut().editor.replace_all("x🎉y🎉z末");
        state.apply(Message::FindQueryChanged("🎉".to_owned()));
        assert_eq!(state.find.hits.len(), 2);
        state.find.replacement = "!".to_owned();
        state.apply(Message::ReplaceAllInDoc);
        assert_eq!(
            state
                .tabs
                .current()
                .editor
                .text()
                .chars()
                .collect::<Vec<_>>(),
            "x!y!z末".chars().collect::<Vec<_>>(),
            "收缩替换逐字符正确"
        );
    }

    /// 越界命中(#17 M2):命中缓存比文档新(扫描后文本变短的竞态)时
    /// 整条跳过,不把替换词钳到文末追加 —— 宁可少报不错报,不 panic;
    /// 界内命中照常替换。
    #[test]
    fn replace_all_skips_out_of_bounds_hits_without_appending() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("短文");
        state.apply(Message::FindBarToggled(true));
        // 不经扫描直接塞入过期命中:首条在界内,后两条越界
        state.find.query = "过期".to_owned();
        state.find.hits = vec![0..2, 5..7, 100..102];
        state.find.hit = Some(0);
        state.find.replacement = "新".to_owned();

        state.apply(Message::ReplaceAllInDoc);
        assert_eq!(
            state
                .tabs
                .current()
                .editor
                .text()
                .chars()
                .collect::<Vec<_>>(),
            "新".chars().collect::<Vec<_>>(),
            "界内命中替换,越界命中不追加(无防线会变「短文新…」)"
        );
        assert_eq!(state.find.hits.len(), 0, "重扫按新文本与真 query 走");
        assert_eq!(state.find.hit, None);
    }

    /// 格式动作改写同标签文本后的缓存过期(#17 评审修复):查找条开着有
    /// 命中 → 对命中之前的内容应用格式(`apply_format` 整篇 replace_all,
    /// 推 rev 不重扫)→ 点「全部」—— 旧偏移在新文本上**仍界内但错位**,
    /// 越界防线只挡 end 超长挡不住界内漂移。入口按 `scanned_for` 见证
    /// 判过期先重扫,替换词落在当前文本的真实命中上。
    #[test]
    fn replace_all_refreshes_stale_hits_after_format_rewrites_text() {
        let mut state = State::default();
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("plain needle x");
        state.apply(Message::FindBarToggled(true));
        state.apply(Message::FindQueryChanged("needle".to_owned()));
        assert_eq!(state.find.hits.len(), 1);
        state.find.replacement = "pin".to_owned();

        // 选中 plain 加粗:文本变长 4 字符,needle 后移 —— 旧命中区间
        // 6..12 在新文本上界内但指向 "**eedl" 附近,正是防线挡不住的漂移
        state.tabs.current_mut().selection = Some((0, 5));
        state.apply(Message::FormatRequested(crate::compose::FormatAction::Bold));
        assert_eq!(state.tabs.current().editor.text(), "**plain** needle x");

        state.apply(Message::ReplaceAllInDoc);
        assert_eq!(
            state.tabs.current().editor.text(),
            "**plain** pin x",
            "替换按重扫后的新命中走,不按旧偏移错位写入"
        );
    }

    /// 整篇缓冲被改写(undo/外部重载/恢复条都落 `EditorBuffer` 的整篇
    /// replace_all,`load` 内部同款)后的双入口过期重扫:「替换当前」按
    /// 旧命中的字符锚点找回定位(文本在命中前变长时锚点精确命中),不再
    /// 依赖过期索引;「全部」按新命中全量走。
    #[test]
    fn replace_refreshes_stale_hits_after_whole_buffer_rewrite() {
        let mut state = State::default();
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("aa needle bb needle");
        state.apply(Message::FindBarToggled(true));
        state.apply(Message::FindQueryChanged("needle".to_owned()));
        state.apply(Message::FindNext { backwards: false });
        assert_eq!(state.find.hit, Some(1), "定位推进到第二个命中");
        state.find.replacement = "pin".to_owned();

        // 命中之前加一个字符:第二个 needle 后移到 14,旧定位区间 13..19
        // 在新文本上界内但错位(会切进 " needl")
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("aaa needle bb needle");
        state.apply(Message::ReplaceCurrent);
        assert_eq!(
            state.tabs.current().editor.text(),
            "aaa needle bb pin",
            "锚点找回定位,替换词落在真实命中上"
        );

        // 再整篇改写后点「全部」:同样先重扫再消费
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("xx needle yy needle zz");
        state.apply(Message::ReplaceAllInDoc);
        assert_eq!(
            state.tabs.current().editor.text(),
            "xx pin yy pin zz",
            "全部替换按重扫后的新命中走"
        );
    }

    /// 全部替换后的状态维护(#17 M2):替换词不含查找词 → 计数清零、定位
    /// 清空;含查找词(needle→needleX)→ 按新文本重扫计数、定位回首条、
    /// `pending_selection` 仍是字符区间,Enter 前进并在末项后环绕。
    #[test]
    fn replace_all_recounts_hits_and_keeps_navigation_legal() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("foo a\nfoo b");
        state.apply(Message::FindBarToggled(true));
        state.apply(Message::FindQueryChanged("foo".to_owned()));
        state.find.replacement = "bar".to_owned();
        state.apply(Message::ReplaceAllInDoc);
        assert_eq!(state.tabs.current().editor.text(), "bar a\nbar b");
        assert_eq!(state.find.hits.len(), 0, "替换词不含查找词:清零");
        assert_eq!(state.find.hit, None, "定位清空");

        // needle → needleX:新文本里的 needleX 前缀也是命中
        state
            .tabs
            .current_mut()
            .editor
            .replace_all("needle a\nneedle b");
        state.apply(Message::FindQueryChanged("needle".to_owned()));
        assert_eq!(state.find.hits.len(), 2);
        state.find.replacement = "needleX".to_owned();
        state.apply(Message::ReplaceAllInDoc);
        assert_eq!(state.tabs.current().editor.text(), "needleX a\nneedleX b");
        assert_eq!(state.find.hits.len(), 2, "按新文本重扫");
        assert_eq!(state.find.hit, Some(0), "定位回首条");
        assert_eq!(
            state.tabs.current().pending_selection,
            Some((0, 6)),
            "pending_selection 仍是字符区间"
        );

        state.apply(Message::FindNext { backwards: false });
        assert_eq!(state.find.hit, Some(1));
        state.apply(Message::FindNext { backwards: false });
        assert_eq!(state.find.hit, Some(0), "末项后环绕回首");
    }

    /// 空 query / 无命中(#17 M2):全部替换是幂等无操作 —— 文本不动、
    /// 修订号不推、不置脏,再次触发仍无副作用。
    #[test]
    fn replace_all_without_hits_is_idempotent_noop() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("稳定文本");
        state.tabs.current_mut().editor.clear_dirty();

        // 空 query:重扫即清,hits 恒空
        state.apply(Message::FindBarToggled(true));
        state.apply(Message::FindQueryChanged(String::new()));
        state.find.replacement = "别的".to_owned();
        let rev = state.tabs.current().editor.revision();
        state.apply(Message::ReplaceAllInDoc);
        state.apply(Message::ReplaceAllInDoc);
        assert_eq!(state.tabs.current().editor.text(), "稳定文本");
        assert_eq!(state.tabs.current().editor.revision(), rev, "不推修订号");
        assert!(!state.tabs.current().editor.is_dirty(), "不置脏");

        // 有 query 无命中:同口径
        state.apply(Message::FindQueryChanged("zzz".to_owned()));
        assert_eq!(state.find.hits.len(), 0);
        state.apply(Message::ReplaceAllInDoc);
        assert_eq!(state.tabs.current().editor.text(), "稳定文本");
        assert_eq!(state.tabs.current().editor.revision(), rev);
    }

    /// 切标签的命中缓存失效(#17 M2):作用于 active tab —— 换入标签即按
    /// 「query 变化」同口径对新文本重扫,旧文档的命中不跨标签携带;越界
    /// 旧命中不会经「全部」把替换词追加进更短的新文档;关掉查找条期间
    /// 切换不扫,重开(Ctrl+F/Ctrl+H)时补扫。
    #[test]
    fn switching_tabs_rescans_find_hits_for_new_document() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("aaa aaa");
        state.tabs.open_tab(None, "短"); // 直接开第二个(不经消息,见 #18 测试同款)
        state.apply(Message::TabActivate(0));

        state.apply(Message::FindBarToggled(true));
        state.apply(Message::FindQueryChanged("aaa".to_owned()));
        assert_eq!(state.find.hits.len(), 2);
        state.find.replacement = "b".to_owned();

        state.apply(Message::TabActivate(1));
        assert_eq!(state.find.hits.len(), 0, "换入短文档即按新文本重扫");
        state.apply(Message::ReplaceAllInDoc);
        assert_eq!(
            state.tabs.current().editor.text(),
            "短",
            "无命中无操作,不追加污染"
        );

        state.apply(Message::TabActivate(0));
        assert_eq!(state.find.hits.len(), 2, "换回原文档命中恢复");

        // 关条期间切换不扫;重开走 toggle_find 补扫,替换用的必是新计数
        state.apply(Message::FindBarToggled(false));
        assert_eq!(state.find.hits.len(), 2, "关条只清定位,缓存留着也无害");
        state.apply(Message::TabActivate(1));
        state.apply(Message::TabActivate(0));
        assert_eq!(
            state.find.hits.len(),
            2,
            "关条期间切换未重扫(计数仍是旧文档的)"
        );
        state.apply(Message::FindBarToggled(true));
        assert_eq!(state.find.hits.len(), 2, "重开补扫同一文档,计数一致");
        state.apply(Message::TabActivate(1));
        assert_eq!(state.find.hits.len(), 0, "重开后切走照常失效");
    }

    /// 关闭当前标签(#17 M2):当前指针移交给邻标签时,命中缓存同样按
    /// 新文档重扫 —— remove 路径与 activate 路径一个口径。
    #[test]
    fn closing_tab_rescans_find_hits_for_new_document() {
        let mut state = State::default();
        state.tabs.current_mut().editor.replace_all("甲甲");
        state.tabs.open_tab(None, "乙乙");
        state.apply(Message::TabActivate(1));
        state.apply(Message::FindBarToggled(true));
        state.apply(Message::FindQueryChanged("乙".to_owned()));
        assert_eq!(state.find.hits.len(), 2);

        // 干净标签直接关(脏确认模态不涉),当前指针落到 0 号
        state.apply(Message::TabCloseRequested(1));
        assert_eq!(state.tabs.tabs.len(), 1);
        assert_eq!(state.find.hits.len(), 0, "移交后的当前标签按新文档重扫");
        assert_eq!(state.tabs.current().editor.text(), "甲甲");
    }

    /// #45 TabRestore 主链路:关 N 个(混合已落盘/未落盘)后连续 restore,
    /// 后进先出且未落盘的被跳过(根本不入栈),已落盘的逐条回来到「回到
    /// 原状」;栈走空后再按是 no-op。恢复 = 重新读盘,内容以盘上为准。
    #[test]
    fn tab_restore_reopens_closed_tabs_lifo() {
        let dir = temp_path("tab-restore-lifo");
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.md");
        let b = dir.join("b.md");
        std::fs::write(&a, "甲的盘上内容").unwrap();
        std::fs::write(&b, "乙的盘上内容").unwrap();

        let mut state = State::default(); // 0 号 = SAMPLE_MD(未落盘)
        state.spawn_tab(Some(a.clone()), "甲的旧缓冲");
        state.spawn_tab(None, "未落盘草稿");
        state.spawn_tab(Some(b.clone()), "乙的旧缓冲");
        assert_eq!(state.tabs.tabs.len(), 4);

        // 从右往左全关:b(已落盘)→ 草稿(未落盘)→ a(已落盘)→ SAMPLE(未落盘)
        for index in (0..4).rev() {
            state.apply(Message::TabCloseRequested(index));
        }
        assert_eq!(
            state.tabs.recently_closed,
            vec![b.clone(), a.clone()],
            "只有已落盘的两条入栈,b 先关在栈底,a 后关在栈顶"
        );
        assert_eq!(state.tabs.tabs.len(), 1, "关空兜底补一个空标签");

        // restore 1:栈顶 a 回来(最近关闭的先恢复;读的是盘上内容,不是
        // 关闭前的旧缓冲 —— 重开 = 重新读盘)
        state.apply(Message::TabRestore);
        assert_eq!(state.tabs.tabs.len(), 2);
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(a.as_path())
        );
        assert_eq!(state.tabs.current().editor.text(), "甲的盘上内容");

        // restore 2:b 回来,逐条回走整条栈
        state.apply(Message::TabRestore);
        assert_eq!(state.tabs.tabs.len(), 3);
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(b.as_path())
        );
        assert_eq!(state.tabs.current().editor.text(), "乙的盘上内容");
        assert!(state.tabs.recently_closed.is_empty(), "两条都消费完");

        // restore 3:栈空 no-op —— 标签数、当前标签、提示行都分毫不动
        let notice_before = state.tabs.current().document.notice.clone();
        state.apply(Message::TabRestore);
        assert_eq!(state.tabs.tabs.len(), 3);
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(b.as_path())
        );
        assert_eq!(state.tabs.current().document.notice, notice_before);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #45:重开的路径已在别的标签打开 → 走 `open_path` 既有语义**激活它**,
    /// 不开第二个;该栈条照样出栈(激活即成功)。
    #[test]
    fn tab_restore_activates_already_open_tab_instead_of_duplicating() {
        let dir = temp_path("tab-restore-dup");
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("doc.md");
        std::fs::write(&doc, "正文").unwrap();

        let mut state = State::default();
        state.spawn_tab(Some(doc.clone()), "正文");
        state.apply(Message::TabCloseRequested(1)); // 关 doc → 栈 = [doc]
        state.spawn_tab(None, "别的工作");
        // 用户又通过别的入口(文件树/搜索)把同一文件打开了
        state.apply(Message::FileSelected(doc.clone()));
        let same_path = |tabs: &crate::tabs::TabsState| -> usize {
            tabs.tabs
                .iter()
                .filter(|tab| tab.document.path.as_deref() == Some(doc.as_path()))
                .count()
        };
        assert_eq!(same_path(&state.tabs), 1, "前置:该路径恰好一个标签");

        state.apply(Message::TabRestore);
        assert_eq!(same_path(&state.tabs), 1, "恢复不开第二个同路径标签");
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(doc.as_path())
        );
        assert!(state.tabs.recently_closed.is_empty(), "激活即成功,条目出栈");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #45:重开失败(文件被外部删/移)—— 提示行按既有口径说明,该条
    /// **丢弃并继续下一条**(decisions-pending #70):一次触发跨过死条目
    /// 把能恢复的恢复出来,死条目不留在栈顶堵住后续恢复。
    #[test]
    fn tab_restore_failed_path_noticed_dropped_and_continues() {
        let dir = temp_path("tab-restore-fail");
        std::fs::create_dir_all(&dir).unwrap();
        let alive = dir.join("alive.md");
        let dead = dir.join("dead.md");
        std::fs::write(&alive, "还活着").unwrap();
        // dead.md 不落盘:模拟关闭后被外部删除

        let mut state = State::default();
        state.spawn_tab(Some(alive.clone()), "还活着");
        state.spawn_tab(Some(dead.clone()), "已被外部删除");
        state.apply(Message::TabCloseRequested(1)); // 关 alive → 栈 = [alive]
        state.apply(Message::TabCloseRequested(1)); // 关 dead(漂移到 1)→ 栈 = [alive, dead]
        assert_eq!(state.tabs.recently_closed.len(), 2);

        state.apply(Message::TabRestore);
        // dead 打不开:notice 落在失败时刻的当前标签(读盘失败不开新标签),
        // 按既有「打开失败 <路径>」口径点名;条目已丢
        let notice = state
            .tabs
            .tabs
            .iter()
            .find_map(|tab| tab.document.notice.clone())
            .unwrap_or_default();
        assert!(
            notice.contains("打开失败") && notice.contains(dead.display().to_string().as_str()),
            "提示行按既有口径点名失败路径,实际是:{notice}"
        );
        // 同一次触发继续往下走:alive 恢复成功
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(alive.as_path())
        );
        assert!(
            state.tabs.recently_closed.is_empty(),
            "死条目丢弃、活条目消费"
        );

        // 再按一次:栈已空,no-op(notice 不被清空也不叠加新失败)
        let len = state.tabs.tabs.len();
        state.apply(Message::TabRestore);
        assert_eq!(state.tabs.tabs.len(), len);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #45:从未关闭过任何标签(栈空)时 TabRestore 是 no-op,不弹提示、
    /// 不动标签数与当前指针。
    #[test]
    fn tab_restore_empty_stack_is_noop() {
        let mut state = State::default();
        state.spawn_tab(None, "草稿");
        state.apply(Message::TabActivate(0));
        let before = state.tabs.tabs.len();
        state.apply(Message::TabRestore);
        assert_eq!(state.tabs.tabs.len(), before);
        assert_eq!(state.tabs.active, 0);
        assert_eq!(state.tabs.current().document.notice, None, "栈空不产生提示");
        assert!(state.tabs.recently_closed.is_empty());
    }
}
