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
use crate::repaint::RepaintNotifier;
use crate::search::SearchState;
use crate::settings::SettingsState;
use crate::tabs::TabsState;
use crate::theme::{Density, SkinCatalog, ThemeMode, ThemeSettings};
use crate::ui::image_dialog::ImageDialogState;
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

/// key 闸门拦下时的状态栏文案:指路设置菜单 → AI Provider 浮窗。
const AI_KEY_MISSING_NOTICE: &str = "未配置 API key(设置 → AI Provider)";

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
}

impl SidebarTab {
    /// 页签栏顺序。
    pub const ALL: [SidebarTab; 4] = [Self::Files, Self::Search, Self::Outline, Self::Git];

    /// 页签栏显示名。
    pub fn label(self) -> &'static str {
        match self {
            Self::Files => "文件",
            Self::Search => "搜索",
            Self::Outline => "大纲",
            Self::Git => "Git",
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
        }
    }
}

/// 文档派生视图快照:预览文本 + 大纲,与编辑器修订号绑定。
///
/// 只在编辑器修订号前进(或整篇换入)时重建,与预览同步是同一时机;
/// vendored 层内部还会按 text hash 二次缓存,空闲帧零开销。
pub struct PreviewState {
    /// 当前快照的**源文本**(大纲 `span` 索引它,与编辑器缓冲同源)。
    ///
    /// 渲染走 [`Self::rendered`] —— wikilink 展开会改变偏移,渲染文本不能
    /// 与源码偏移混用。生产路径当前只读 `rendered`,`text` 是快照真源与
    /// 「空闲帧不重建」这类不变量的锚点。
    #[allow(dead_code)]
    pub text: String,
    /// 快照对应的 [`EditorBuffer::revision`]。
    pub synced_rev: u64,
    /// 文档大纲,与 `text` 同一次重建产出,`span` 直接索引该文本。
    pub outline: Vec<OutlineItem>,
    /// 喂给预览的**渲染文本**:`[[wikilink]]` 已展开成 `wiki://` 链接
    /// (P3 双向链接)。源码 `text` 一字不改 —— 展开只影响渲染。
    pub rendered: String,
    /// 待滚动到的字节偏移(大纲点击交下来的目标),由预览绘制消费一次。
    /// 属 UI 关注点(同侧边栏把手与键位捕获),不进归约:滚动位置不是文档状态。
    pub scroll_target: Option<usize>,
}

impl PreviewState {
    /// 以编辑器当前内容建立快照(文本 + 大纲)。
    pub fn new(editor: &EditorBuffer) -> Self {
        let text = editor.text().to_owned();
        Self {
            // 展开放在重建里而不是每帧:wikilink 展开要遍历全文,空闲帧
            // 不该付这个代价(与「修订号前进才重建」同一条规则)
            rendered: latermd_md::expand_wikilinks(&text),
            outline: latermd_md::outline(&text),
            text,
            synced_rev: editor.revision(),
            scroll_target: None,
        }
    }

    /// 修订号前进后重建快照;空闲帧不得调用(会白白重解析全文)。
    pub fn rebuild(&mut self, editor: &EditorBuffer) {
        *self = Self::new(editor);
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
/// 文档内查找条状态(#17 最小版):`hits` 是当前 query 的大小写不敏感
/// 全量命中(字符区间);`hit` 是当前停在第几个(0-based,`None` 未定位)。
#[derive(Debug, Default, PartialEq)]
pub struct FindBarState {
    pub open: bool,
    pub query: String,
    pub hits: Vec<Range<usize>>,
    pub hit: Option<usize>,
}

pub struct State {
    /// 后台结果到达时唤醒 egui；内部合并重复通知，不承载业务数据。
    repaint: RepaintNotifier,
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
    /// 在途 AI 流的发起标签 id;`None` = 无流。发起时锁定,收尾
    /// (成功/失败/作废)清除 —— [`Message::AiChunk`] / [`Message::AiDone`]
    /// 的写入目标由它决定,与 `tabs.active` 无关:切标签不中断也不改道。
    pub ai_active_tab: Option<u64>,
    /// 文件树(Files 页签):根目录、最近列表与懒加载缓存。
    pub file_tree: FileTreeState,
    /// Git 面板(Git 页签 + Files 页角标,P2):状态/历史/diff 快照与
    /// 确认式回滚;刷新时机见 `git_panel` 模块文档。
    pub git: GitPanelState,
    /// 全文搜索(Search 页签):输入去抖、后台服务与结果缓存。
    pub search: SearchState,
    /// AI 流式(MockProvider,P1 联调):provider + 接收端 + 防重入标志。
    pub ai: AiState,
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
    /// 设置对话框(外观 / 快捷键 / AI / MCP / 图片 五页)。
    pub settings: SettingsState,
    /// 图床(docs/image-plan.md C 段):profile 列表(`beds.json`)与在途
    /// 上传的接收端;发起/收流的归约见 [`State::request_image_upload`] /
    /// [`State::finish_image_upload`]。
    pub bed: BedState,
    /// 图片框对话框(docs/image-plan.md A 段):草稿 alt/url 归它持有,
    /// 归约只置 `open` 与消费插入,UI 经 `&mut` 改草稿 —— 与 `SettingsState`
    /// 持草稿同款分工(`TextEdit` 是立即模式控件,草稿必须能就地 `&mut`)。
    pub image_dialog: ImageDialogState,
    /// 剪贴板图片读取(docs/image-plan.md D 段):后台线程 + channel 的
    /// 接收端,生命周期与 `BedState` 同构(发起/收流/收尾三原语)。
    pub clipboard: ClipboardState,
    /// 最近一次 AI 生成的 commit message 建议;`Some` = 建议浮窗可见。
    /// 经 [`Message::AiCommitFinished`] 的成功结果置入,浮窗「关闭」或
    /// 下一次生成时替换/清除。
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
    const UNTITLED: &str = "未命名";

    /// 文件名显示(未落盘为「未命名」),dirty 追加 `*`。
    pub fn display_name(&self) -> String {
        let name = self
            .path
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| Self::UNTITLED.to_owned());
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
            repaint: RepaintNotifier::default(),
            layout: LayoutSettings::default(),
            layout_written: LayoutSettings::default(),
            tabs: TabsState::new(SAMPLE_MD),
            find: FindBarState::default(),
            ai_active_tab: None,
            file_tree: FileTreeState::default(),
            git: GitPanelState::default(),
            search: SearchState::default(),
            ai: AiState::default(),
            ai_key: AiKeyState::default(),
            mcp: McpState::default(),
            render_mode: RenderMode::default(),
            keymap: Keymap::builtin(),
            settings: SettingsState::default(),
            bed: BedState::default(),
            image_dialog: ImageDialogState::default(),
            clipboard: ClipboardState::default(),
            ai_commit_suggestion: None,
            theme: ThemeSettings::default(),
            skins: SkinCatalog::default(),
            system_theme: None,
            system_theme_ok: false,
            system_theme_due: None,
            settings_dir: None,
        }
    }
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
    /// 切换明暗主题(设置菜单产出);归约里改状态并即时落盘。
    ThemeChanged(ThemeMode),
    /// 明暗主题互换(命令层「切换主题」的快捷键/菜单入口;定向选择走
    /// [`Message::ThemeChanged`])。
    ToggleTheme,
    /// 切换左侧导航栏展开/折叠(命令层 `Ctrl/Cmd+\` 与自绘标题栏「关闭
    /// 左侧」两个入口;面板把手自行翻转不走消息)。
    SidebarToggled,
    /// 切换右侧只读预览栏(自绘标题栏「关闭右侧」入口,
    /// docs/ui-shell-redesign.md §3.1;M1 起接 `LayoutSettings::right`)。
    RightPanelToggled,
    /// 切换禅定模式(docs/ui-shell-redesign.md §7;M4 实现,命令层先挂上)。
    ZenToggled,
    /// 请求一次 Markdown 格式动作(docs/ui-shell-redesign.md §6.4)。
    ///
    /// 工具条按钮与快捷键两个入口同源;真正的语义全在
    /// [`crate::compose::apply`],归约侧只负责取选区、调它、把结果写回
    /// 缓冲并把新选区挂到 `TabState::pending_selection`。
    FormatRequested(crate::compose::FormatAction),
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
    /// 拖入图片文件(docs/image-plan.md D 段):归约里读文件、过白名单与
    /// 5MB 上限、落 `.assets/` 并在光标处插引用。文件读取是本地磁盘
    /// (毫秒级),同步做,不上后台线程 —— 与文件树打开文件同口径。
    ImageFileDropped(PathBuf),
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
    SearchQueryChanged,
    /// 去抖到点,按当前输入与根目录发起搜索。
    SearchRequested,
    /// 点击搜索结果,载荷为(文件路径, 1 起行号):归约里打开该文件并把
    /// 光标跳到行首。
    SearchResultClicked(PathBuf, usize),
    /// 点击大纲条目,载荷为标题的源码字节区间。
    OutlineItemClicked(Range<usize>),
    /// 点击预览里的 `[[wikilink]]`,载荷为目标文档名:归约里在文档库内找同名
    /// 文档并打开(找不着落提示行,不静默无反应)。
    WikilinkClicked {
        target: String,
    },
    /// 发起 AI Mock 流式续写(命令层入口);流式进行中在归约里被忽略
    /// (防重入,见 [`AiState::start`])。
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
    /// 请求生成摘要(命令层入口):文档全文喂 provider,移除旧「AI 摘要」
    /// 节后在文档末尾以引用块形式流式追加新要点。流式进行中在归约里被
    /// 忽略(防重入,同一道闸)。
    AiSummaryRequested,
    /// commit message 后台任务结束。请求序号用于拒绝旧任务迟到结果；
    /// 成功载荷为单行 subject，失败载荷回到发起标签提示行；标签已关闭则
    /// 只收口任务，不把迟到错误写进其他文档。
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
    /// 源码模式 ↔ Live Preview 互换(P3):只翻标志,不碰缓冲与光标。
    ToggleLivePreview,
    /// 打开设置对话框并切到指定分页(工具栏齿轮 / 菜单「设置…」入口)。
    SettingsOpened(crate::settings::SettingsTab),
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
    /// 切到下一个标签(Ctrl/Cmd+Tab 循环)。
    TabNext,
    /// 点击 Git 页改动列表里的文件,载荷为相对仓库根的路径:归约里选中
    /// 并读它的 diff(列表外的过期路径被忽略)。
    GitFileSelected(String),
    /// 点击「回滚此文件」,载荷为相对仓库根的路径:归约里只置确认模态,
    /// checkout 在用户显式确认之后。
    GitCheckoutRequested(String),
    /// 确认模态里确认回滚:执行 latermd-git 唯一的写操作并立即刷新状态;
    /// 失败文案进提示行。
    GitCheckoutConfirmed,
    /// 确认模态取消,不触碰工作区。
    GitCheckoutCancelled,
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
}

impl State {
    /// 将应用的 egui context 绑定到后台通知器。输入来自启动闭包或当前
    /// logic 帧，只建立唤醒通道，不触发重绘也不修改业务状态。
    pub fn bind_repaint_context(&self, context: &eframe::egui::Context) {
        self.repaint.bind_context(context);
    }

    /// logic 帧开始前解除上一批通知的合并标志。必须先于所有 channel
    /// drain 调用，让 drain 期间新到的结果能够再请求下一帧。
    pub fn begin_background_drain(&self) {
        self.repaint.begin_drain();
    }

    /// 消费一条消息,变更状态。只允许在 `App::logic` 调用。
    pub fn apply(&mut self, message: Message) {
        match message {
            Message::SidebarTabChanged(tab) => {
                self.layout.left_view = tab;
                // 切到 Git 页立即刷新:页签可能停了很久,轮询周期外的快照
                // 会误导回滚决策
                if tab == SidebarTab::Git {
                    self.refresh_git();
                }
            }
            Message::FileCommand(cmd) => self.run_file_cmd(cmd),
            Message::NoticeDismissed => self.tabs.current_mut().document.notice = None,
            Message::ExportHtml => self.run_export_html(),
            Message::ThemeChanged(mode) => self.change_theme(mode),
            Message::ToggleTheme => self.change_theme(self.theme.mode.opposite()),
            Message::SidebarToggled => self.toggle_left_panel(),
            Message::RightPanelToggled => self.toggle_right_panel(),
            Message::ZenToggled => self.toggle_zen(),
            Message::FormatRequested(action) => self.apply_format(action),
            Message::FindBarToggled(open) => self.toggle_find(open),
            Message::FindQueryChanged(query) => self.find_query_changed(query),
            Message::FindNext { backwards } => self.find_next(backwards),
            Message::ImageDialogOpened => self.open_image_dialog(),
            Message::ImageInserted { alt, url } => self.insert_image(&alt, &url),
            Message::ImageDialogClosed => self.close_image_dialog(),
            Message::ImageFilePickRequested => self.pick_image_file(),
            Message::ImageUploadRequested => self.request_image_upload(),
            Message::ImageUploadFinished { seq, result } => self.finish_image_upload(seq, result),
            Message::ImagePasteRequested => self.request_image_paste(),
            Message::ImagePasteFinished { result } => self.finish_image_paste(result),
            Message::ImageFileDropped(path) => self.drop_image_file(&path),
            Message::BedProfileSaved { profile, token } => self.save_bed_profile(profile, token),
            Message::BedProfileDeleted { id } => self.delete_bed_profile(id),
            Message::BedTestUploadRequested { profile_id } => self.test_bed_upload(profile_id),
            Message::FileTreeRootPick => self.pick_file_tree_root(),
            Message::FileTreeRootSelected(dir) => self.change_file_tree_root(dir),
            Message::FileTreeToggled(dir) => self.file_tree.toggle(&dir),
            Message::FileSelected(path) => self.open_path(&path),
            Message::SearchQueryChanged => self.search.input_changed(DEBOUNCE),
            Message::SearchRequested => self.start_search(),
            Message::SearchResultClicked(path, line_no) => {
                self.open_search_hit(&path, line_no);
            }
            Message::OutlineItemClicked(span) => {
                // 编辑器跳光标 + 预览滚到该标题(P3「大纲预览跳转」):同一个
                // span 两处消费,预览侧在绘制时换算成 y
                self.tabs.current_mut().preview.scroll_target = Some(span.start);
                self.jump_cursor_to_heading(span);
            }
            Message::WikilinkClicked { target } => self.open_wikilink(&target),
            Message::AiStart => self.start_ai_stream(),
            Message::AiChunk { delta } => self.append_ai_delta(&delta),
            Message::AiDone => {
                self.ai.finish();
                self.ai_active_tab = None;
            }
            Message::AiFailed(error) => {
                self.ai.finish();
                // 失败信息属于发起流的标签(绑定清除前定位),不属于此刻的
                // active
                self.ai_origin_tab_mut().document.notice = Some(error);
                self.ai_active_tab = None;
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
            Message::AiSummaryRequested => self.request_summary(),
            Message::AiCommitFinished {
                seq,
                tab_id,
                result,
            } => {
                // 只有最新请求能清任务并落结果；被替换任务的迟到消息没有
                // 修改建议浮窗或提示行的权限。
                if self.ai.finish_commit(seq) {
                    let Some(index) = self.tabs.index_by_id(tab_id) else {
                        return;
                    };
                    match result {
                        Ok(subject) => self.ai_commit_suggestion = Some(subject),
                        Err(error) => self.tabs.tabs[index].document.notice = Some(error),
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
            Message::McpConfigSaved(config) => self.apply_mcp_config(config),
            Message::ThemeSkinSelected(name) => self.apply_skin(name),
            Message::ThemeSkinExported { name } => self.export_skin(&name),
            Message::ThemeDensityChanged(density) => {
                self.theme.density = density;
                self.persist_theme();
            }
            Message::ToggleLivePreview => self.toggle_live_preview(),
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
            }
            Message::TabCloseCancelled => self.tabs.confirm_close = None,
            Message::TabNext => {
                let next = self.tabs.next_index();
                self.switch_active(next);
            }
            Message::GitFileSelected(path) => self.git.select(&path),
            Message::GitCheckoutRequested(path) => self.git.request_checkout(path),
            Message::GitCheckoutConfirmed => {
                // 回滚目标与仓库根在 confirm_checkout 内被 take,先留档供
                // 成功后的「当前文档一致性」判定
                let target = self
                    .git
                    .confirm_checkout
                    .clone()
                    .zip(self.git.repo_root.clone());
                let result = self.git.confirm_checkout();
                // checkout 本身是用户确认后的单文件写；随后的 status/log/diff
                // 重读走后台单飞，避免确认点击后继续阻塞 UI。
                self.refresh_git();
                match result {
                    Some(error) => self.tabs.current_mut().document.notice = Some(error),
                    None => {
                        if let Some((rel, root)) = target {
                            self.after_git_checkout(&root.join(&rel));
                        }
                    }
                }
            }
            Message::GitCheckoutCancelled => self.git.cancel_checkout(),
        }
    }

    /// 请求后台刷新 Git 状态。触发点:切到 Git 页、文件树换根、回滚完成,
    /// 以及归约侧的到点轮询。任务单飞，换根/重复请求只保留最新结果。
    pub fn refresh_git(&mut self) {
        let root = self.file_tree.root.clone();
        self.git.request_refresh(root, self.repaint.clone());
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

    /// 生成 commit message(`Message::AiCommitRequested` 的归约):这里只
    /// 定位仓库并发起后台任务；采 diff、拼 prompt、真实 HTTP 全部在线程
    /// 中完成，结果经 [`Message::AiCommitFinished`] 回到归约。
    ///
    /// 仓库定位:当前文档所在目录优先,退文件树根(`git diff` 在仓库子目录
    /// 里跑也返回全仓改动);两者皆无 → 提示行。流式或 commit 任务进行中
    /// 忽略重复触发，避免多个请求争用同一建议浮窗。
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
        self.ai_commit_suggestion = None;
        let tab_id = self.tabs.current().id;
        if let Err(error) = self.ai.start_commit(dir, tab_id, self.repaint.clone()) {
            self.tabs.current_mut().document.notice = Some(error);
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
        let prompt = latermd_ai::summary_prompt(self.tabs.current().editor.text());
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
        if self.ai.start(prompt, self.repaint.clone()) {
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

    /// 追加 AI 增量块到**发起标签**文档末尾(`Message::AiChunk` 的归约)。
    /// 写入目标按 [`State::ai_active_tab`] 定位而非当前标签 —— 流式期间
    /// 切标签,块仍长在发起它的文档上(关键回归测试
    /// `ai_stream_writes_to_origin_tab_not_active`)。发起标签已不存在时
    /// (理论上 `remove_tab` 已先行作废流)丢弃块并作废,绝不落到别的
    /// 标签。走 `insert_chars` 的增量 splice 双写,dirty 与修订号照常推进
    /// —— AI 写进来的是真实内容,保存前与手敲同责;undo 语义见模块文档。
    fn append_ai_delta(&mut self, delta: &str) {
        let Some(index) = self.ai_stream_tab_index() else {
            self.abort_ai_stream();
            return;
        };
        let tab = &mut self.tabs.tabs[index];
        tab.editor.insert_chars(tab.editor.len_chars(), delta);
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

    /// 作废在途流(收尾标志 + 指令卡状态键 + 发起标签绑定一并清)。
    fn abort_ai_stream(&mut self) {
        self.ai.finish();
        self.ai.forget_last_prompt();
        self.ai_active_tab = None;
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
        // 跟随系统:启动即探测一次,否则首帧只能靠 fallback
        if self.theme.mode == ThemeMode::System {
            self.refresh_system_theme();
        }
        let mcp = McpConfig::load_from(&dir);
        self.mcp.set_root(self.file_tree.root.clone());
        self.mcp.apply_config(mcp, self.repaint.clone());
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

    /// 翻转右栏(只读预览)可见性;写盘同上由帧末统一负责。
    fn toggle_right_panel(&mut self) {
        self.layout.right = !self.layout.right;
    }

    /// 进/出禅定(§7)。进出各走 `LayoutSettings` 上那两个同名方法 —— 快照
    /// 怎么存怎么还原由它们自己说了算,`State` 不插手第二个真相源。
    fn toggle_zen(&mut self) {
        if self.layout.zen {
            self.layout.exit_zen();
        } else {
            self.layout.enter_zen();
        }
    }

    /// 请求一次 Markdown 格式动作(§6.4 链路)。
    ///
    /// 语义全在 [`crate::compose::apply`];这里负责三件事:取当前选区 →
    /// 调用 → 把新文本写回缓冲并把新选区挂 `pending_selection`。选区为
    /// `None`(UI 还没回填过)时按纯光标(0,0)处理。
    ///
    /// 写回用 `replace_all` 而非定点 `replace_range`:`compose::apply` 的
    /// 产出是**整篇**新文本,要拿到定点 delta 得自己去做 diff(复杂度与
    /// 收益不成比例)。代价是 `TextEdit` 内建 undoer 的快照被整篇重建打碎,
    /// Ctrl+Z 可能一次回退一整次格式操作 —— 已知并接受(§9 R3)。
    fn apply_format(&mut self, action: crate::compose::FormatAction) {
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

    /// 查找条开/关:开时按当前选区预填查找词(选中即所要找的,编辑器
    /// 惯例)并重扫;关时清定位。
    fn toggle_find(&mut self, open: bool) {
        self.find.open = open;
        if open {
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
        self.bed.start(
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
        self.clipboard.start(self.repaint.clone());
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
        self.bed.start(
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

    /// 切模式:只翻标志。切到 Live 时顺带按当前光标定位活动块(首次进入
    /// 就有可编辑的块,而不是「点一下才出现」)。
    fn toggle_live_preview(&mut self) {
        self.render_mode = self.render_mode.opposite();
        if self.render_mode == RenderMode::Live {
            let tab = self.tabs.current_mut();
            let byte = tab.cursor.byte;
            tab.live.sync(&tab.editor, byte);
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
        match crate::theme::export_skin(&dir, name, &style) {
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
        self.mcp.apply_config(config, self.repaint.clone());
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
            Some(path) => self.open_path(&path),
            None => {
                self.tabs.current_mut().document.notice =
                    Some(format!("文档库里没有「{target}」这篇文档"));
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
        // 文件树懒加载落点:根 + 展开中目录的子项缓存补齐(键缺席才 IO)。
        self.file_tree.ensure_loaded();
        // 搜索结果收流:非阻塞收空 channel(重绘驱动见 `ui::layout::reduce`)。
        self.search.poll_hits();
        // Git 后台快照只在归约帧换入；旧根/旧序号结果由状态机拒绝。
        self.git.poll_refresh(self.repaint.clone());
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

    fn run_file_cmd(&mut self, cmd: FileCmd) {
        match cmd {
            FileCmd::New => {
                // 多标签:新建永远开新标签,当前标签的未保存稿不受影响
                self.spawn_tab(None, "");
            }
            FileCmd::Open => {
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
    /// 只落提示行,当前标签的缓冲与 dirty 不受影响。
    ///
    /// 同时展开该文件在树内的祖先目录:无论从文件树、菜单还是搜索跳转打开,
    /// 该文件的高亮行都应当在 Files 页里可见。
    fn open_in_tab(&mut self, path: &Path) {
        match file::read(path) {
            Ok(text) => {
                self.spawn_tab(Some(path.to_path_buf()), &text);
                self.file_tree.expand_ancestors_of(path);
            }
            Err(error) => self.tabs.current_mut().document.notice = Some(error.to_string()),
        }
    }

    /// 按路径打开文档的统一入口(菜单「打开」/ 文件树点击 / 搜索跳转):
    /// 该路径已在某标签打开则**激活它**(路径去重,同一路径至多一个标签),
    /// 否则读盘开新标签。
    fn open_path(&mut self, path: &Path) {
        if let Some(index) = self.tabs.find_by_path(path) {
            self.switch_active(index);
        } else {
            self.open_in_tab(path);
        }
    }

    /// 开新标签的统一入口。不动在途 AI 流:流绑定发起标签
    /// ([`State::ai_active_tab`]),新标签不是它的写入目标。
    fn spawn_tab(&mut self, path: Option<PathBuf>, text: &str) -> usize {
        self.tabs.open_tab(path, text)
    }

    /// 激活某标签。不动在途 AI 流:chunk 的写入目标由发起标签 id 决定,
    /// 与当前标签无关(多标签 #11 的核心不变量)。
    fn switch_active(&mut self, index: usize) {
        self.tabs.activate(index);
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
    /// —— 剩余 chunk 无处可写,落到任何别的标签都是写错文档。
    fn remove_tab(&mut self, index: usize) {
        let closing_stream_origin = self.ai_stream_tab_index() == Some(index);
        self.tabs.remove(index);
        if closing_stream_origin {
            self.abort_ai_stream();
        }
    }

    /// 去抖到点发起搜索(`Message::SearchRequested` 的归约)。无根目录
    /// 不发起(该情形下 `ui` 层就不该发消息,这里防御性短路)。
    fn start_search(&mut self) {
        if let Some(root) = self.file_tree.root.clone() {
            self.search.start(&root, self.repaint.clone());
        } else {
            self.search.reset();
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

    /// 换根并持久化(对话框与最近列表两个入口共用);落盘失败只落提示行,
    /// 本次会话的文件树照常可用。搜索一并复位:旧根的结果在新根下相对
    /// 路径失真,留着只会误导。Git 状态立即按新根刷新(角标与 Git 页
    /// 不留旧仓库的快照)。
    fn change_file_tree_root(&mut self, dir: PathBuf) {
        self.file_tree.set_root(dir);
        // MCP 的检索边界跟着换根(共享句柄,服务不必重启)
        self.mcp.set_root(self.file_tree.root.clone());
        self.search.reset();
        // 新根后台快照回来前先撤掉旧仓库角标，不能把旧路径状态短暂贴到
        // 新文件树上；在途结果仍由请求序号拒绝。
        self.git.invalidate_snapshot();
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
                self.tabs.current_mut().editor.clear_dirty();
                self.tabs.current_mut().document.path = Some(path);
                self.tabs.current_mut().document.notice = None;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::FormatAction;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("latermd-state-{}-{name}", std::process::id()))
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
        assert_eq!(document.display_name(), "未命名*");
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
        assert_eq!(state.theme.mode, ThemeMode::Light, "默认深色 → 浅色");
        assert!(dir.join("settings.json").exists(), "互换同样持久化");
        state.apply(Message::ToggleTheme);
        assert_eq!(state.theme.mode, ThemeMode::Dark, "再切回深色");

        state.apply(Message::SidebarToggled);
        assert_eq!(state.layout.left, !visible_before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 两个「关闭栏」按钮各自只翻自己那一个 bool,互不牵连(M1 验收点:
    /// 同时关左右 → 只剩编辑器)。
    #[test]
    fn panel_toggles_are_independent() {
        let mut state = State::default();
        assert!(state.layout.left && state.layout.right, "出厂三栏全开");

        state.apply(Message::SidebarToggled);
        assert!(!state.layout.left && state.layout.right, "只收左栏");

        state.apply(Message::RightPanelToggled);
        assert!(!state.layout.left && !state.layout.right, "左右都收");

        state.apply(Message::SidebarToggled);
        assert!(state.layout.left && !state.layout.right, "只开左栏");
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
        state.layout.right = false;
        state.end_of_logic();

        assert!(
            !LayoutSettings::load_from(&dir).unwrap().right,
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
        crate::theme::export_skin(&dir, "暗夜", &style).unwrap();
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

    /// 大纲点击同时驱动两处:编辑器跳光标(既有)+ 预览滚到该标题(P3)。
    #[test]
    fn outline_click_sets_cursor_jump_and_preview_scroll() {
        let mut state = State::default();
        state.apply(Message::OutlineItemClicked(10..19));
        assert_eq!(state.tabs.current().preview.scroll_target, Some(10));
        assert!(
            state.tabs.current().cursor.jump_to.is_some(),
            "编辑器侧照旧跳光标"
        );
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

        state.apply(Message::ToggleLivePreview);
        assert_eq!(state.render_mode, RenderMode::Source);
    }

    /// 系统主题只在「跟随系统」模式轮询:其余模式返回 None(egui 得以收敛
    /// 到深度空闲),跟随模式返回下一次探测时刻。
    #[test]
    fn system_theme_polls_only_when_following_system() {
        let mut state = State::default();
        assert_eq!(
            state.poll_system_theme(std::time::Instant::now()),
            None,
            "默认深色不轮询"
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
        assert!(html.contains("<h1>导出标题</h1>"), "{html}");
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
        });

        state.apply(Message::FileTreeRootSelected(dir.clone()));
        assert_eq!(state.search.debounce_due, None, "去抖计时被清");
        assert!(state.search.hits.is_empty(), "旧根结果被清");
        assert!(!state.search.is_running());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&settings_dir);
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
        std::thread::sleep(std::time::Duration::from_millis(60));
        assert!(
            state
                .poll_ai()
                .iter()
                .any(|m| matches!(m, Message::AiChunk { .. })),
            "前置:开新标签前已产出正文块"
        );

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

    /// 收完当前 Git 单飞任务及其排队补跑。测试只等待并调用真实 poll，
    /// 不直接触碰快照字段，确保序号校验和异步应用路径都被覆盖。
    fn drain_git_refresh(state: &mut State) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while state.git.is_refreshing() && std::time::Instant::now() < deadline {
            state.git.poll_refresh(state.repaint.clone());
            if state.git.is_refreshing() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert!(!state.git.is_refreshing(), "Git 后台刷新应收口");
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
        drain_git_refresh(&mut state);
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
        assert!(state.git.diff.contains("+工作区乱改"), "{}", state.git.diff);

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
        drain_git_refresh(&mut state);
        assert_eq!(
            std::fs::read_to_string(dir.join("a.md"))
                .unwrap()
                .replace("\r\n", "\n"),
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

    /// 回滚目标正是编辑器当前文档(非 dirty):确认后编辑器重读磁盘的
    /// HEAD 版本、预览同帧联动——不重载则编辑器仍显示已丢弃的工作区版本。
    #[test]
    fn git_checkout_of_current_document_reloads_editor() {
        let dir = git_repo_with_dirty_file("git-reload");
        let file = dir.join("a.md");
        let mut state = State::default();
        state.file_tree.root = Some(dir.clone());
        state.refresh_git();
        drain_git_refresh(&mut state);
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
            state.tabs.current_mut().editor.text().replace("\r\n", "\n"),
            "HEAD 版本\n",
            "编辑器随磁盘回滚重载"
        );
        assert_eq!(
            state.tabs.current().preview.text.replace("\r\n", "\n"),
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
        drain_git_refresh(&mut state);
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
            std::fs::read_to_string(&file)
                .unwrap()
                .replace("\r\n", "\n"),
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
        drain_git_refresh(&mut state);
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
        drain_git_refresh(&mut state);
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
        drain_git_refresh(&mut state);
        assert!(
            state.git.badge_for(&repo.join("a.md")).is_some(),
            "前置:仓库根有角标"
        );

        state.apply(Message::FileTreeRootSelected(plain.clone()));
        drain_git_refresh(&mut state);
        let error = state.git.error.as_deref().expect("降级文案");
        assert!(error.contains("不是"), "{error}");
        assert!(state.git.badges.is_empty(), "旧角标不残留");
        assert_eq!(state.git.selected, None);

        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&plain);
        let _ = std::fs::remove_dir_all(&settings_dir);
    }

    /// 收完一次 AI commit 后台任务，把结果消息逐条交回真实 reducer。
    /// 测试用轮询只负责等待线程，不绕过生产路径的序号校验与状态收口。
    fn drain_ai_commit(state: &mut State) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while state.ai.is_commit_running() && std::time::Instant::now() < deadline {
            for message in state.poll_ai() {
                state.apply(message);
            }
            if state.ai.is_commit_running() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert!(!state.ai.is_commit_running(), "AI commit 后台任务应收口");
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
        drain_ai_commit(&mut state);
        let subject = state.ai_commit_suggestion.clone().expect("已生成建议");
        assert_eq!(subject, "docs: 新增README.md");
        assert!(!subject.contains('\n'), "单行 subject");

        state.apply(Message::AiCommitDismissed);
        assert_eq!(state.ai_commit_suggestion, None, "关闭清空建议");

        // 干净仓库(全部提交):无未提交改动 → 提示,不出建议
        run_git(&dir, &["commit", "-q", "-m", "init"]);
        state.apply(Message::AiCommitRequested);
        drain_ai_commit(&mut state);
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

    /// commit 后台等待期间切标签：失败提示回到发起标签，不污染用户后来
    /// 切到的文档；这是同步实现改成异步后必须守住的标签归属边界。
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
        drain_ai_commit(&mut state);

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
        drain_ai_commit(&mut state);
        assert!(state.ai_commit_suggestion.is_some(), "收尾后同一请求生效");
        let _ = std::fs::remove_dir_all(&dir);
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
        std::thread::sleep(std::time::Duration::from_millis(60));
        assert!(
            state
                .poll_ai()
                .iter()
                .any(|m| matches!(m, Message::AiChunk { .. })),
            "前置:流在产块"
        );

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
}
