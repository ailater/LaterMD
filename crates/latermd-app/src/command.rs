//! 统一命令层(docs/roadmap.md P0「快捷键」)。
//!
//! [`Command`] 是全部用户命令的唯一清单:label、快捷键与 [`Message`] 映射
//! 只在这里定义一份,顶部菜单栏(`ui::menubar`)、编辑面板工具栏按钮与
//! `App::logic` 的快捷键消费三处入口共用。命令的执行(弹框 + IO + 状态
//! 变更)全部发生在 [`crate::state::State::apply`] 的归约里;本模块与 UI
//! 一样只产出消息。
//!
//! 平台自适应:`Modifiers::COMMAND` 在 Windows/Linux 是 Ctrl、macOS 是 Cmd
//! (egui 内建),菜单里经 `Context::format_shortcut` 按平台显示。

use crate::compose::FormatAction;
use crate::file::FileCmd;
use crate::state::Message;
use eframe::egui::{self, Modifiers};

/// 命令分组(#54 快捷键蒙层):命令注册表的既有归属收编成一个维度,
/// 单一事实源仍是本模块(`Command::group`)——蒙层、设置页「快捷键」与
/// 快速打开的命令组共用它,不另抄清单。
///
/// 口径沿用菜单栏(`ui::menubar` 的子菜单)与格式工具条(`ui::format_bar`)
/// 的既有归属;「其他」兜底当前无人认领的命令(新命令忘了归类时落这里,
/// `groups_cover_every_command_exactly_once` 钉住全集不丢)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandGroup {
    /// 文件(menubar「文件」:新建/打开/快速打开/保存/另存为)。
    File,
    /// 编辑(menubar「编辑」:复制行/查找/替换)。
    Edit,
    /// 格式(格式工具条的十五条动作 + 插入图片/Emoji 两个对话框类入口)。
    Format,
    /// 视图(menubar「视图」的两条 + 预览栏/Live/禅定三个布局开关)。
    View,
    /// 标签(menubar 挂在「文件」尾部的三条;蒙层独立成组,容量余裕比菜单宽)。
    Tab,
    /// 导出(menubar「导出」)。
    Export,
    /// AI(menubar「AI」)。
    Ai,
    /// 兜底:未归类的命令。
    Other,
}

impl CommandGroup {
    /// 展示顺序(蒙层从左到右、从上到下)。
    pub const ALL: [CommandGroup; 8] = [
        Self::File,
        Self::Edit,
        Self::Format,
        Self::View,
        Self::Tab,
        Self::Export,
        Self::Ai,
        Self::Other,
    ];

    /// 分组显示名。
    pub fn label(self) -> &'static str {
        match self {
            Self::File => "文件",
            Self::Edit => "编辑",
            Self::Format => "格式",
            Self::View => "视图",
            Self::Tab => "标签",
            Self::Export => "导出",
            Self::Ai => "AI",
            Self::Other => "其他",
        }
    }
}

/// 用户命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// 新建空文档。
    New,
    /// 打开已有文件。
    Open,
    /// 快速打开…(#24):居中浮层模糊搜文件树全部 md(打开)与命令全集
    /// (执行)。命令与键位先注册(本条),浮层状态机与绘制在 C3 接入,
    /// 归约暂只翻转最小标志。
    QuickOpen,
    /// 保存;从未落盘时等价于另存为。
    Save,
    /// 另存为(总是弹框)。
    SaveAs,
    /// 导出当前文档为 HTML(派生物,不触碰文档落盘身份)。
    ExportHtml,
    /// 导出当前文档为 PDF(A4,嵌入系统 CJK 字体;派生物,不触碰文档
    /// 落盘身份)。不绑默认快捷键(Ctrl/Cmd+E 已被导出 HTML 占用),只从
    /// 菜单「导出」与侧边栏按钮触发。
    ExportPdf,
    /// 明暗主题互换;定向选择仍走工具栏「设置」菜单。
    ToggleTheme,
    /// 侧边栏展开/折叠。
    ToggleSidebar,
    /// AI:Mock 流式续写(P1 联调入口,decisions-pending #3)。流式进行中
    /// 再次触发在归约侧被忽略;不绑快捷键,避免与现有键位冲突。
    AiMockStream,
    /// AI:生成 commit message(P1):读仓库未提交改动(staged 优先),
    /// 合成 conventional 中文 subject 并弹建议对话框。流式进行中触发
    /// 同样被忽略(与 AiMockStream 共用防重入)。
    AiCommitMessage,
    /// AI:生成摘要(P1):文档全文喂 provider,3-5 条要点以引用块形式
    /// 流式追加到文档末尾;文档已含「AI 摘要」节则先移除再插入。流式
    /// 进行中触发同样被忽略(共用防重入)。
    AiSummary,
    /// 切到下一个标签(P1.5「多标签」,Ctrl/Cmd+Tab 循环)。
    TabNext,
    /// 关闭当前标签(脏则确认模态;Ctrl/Cmd+W)。
    TabClose,
    /// 恢复最近关闭的标签(#45,Cmd/Ctrl+Shift+T,浏览器/VS Code 同款):
    /// 从关闭栈弹出最近一条**已落盘**路径重开;未落盘新标签关闭不入栈,
    /// 跨会话不恢复(取舍见 preview-typography-and-keymap-plan §3.3)。
    TabRestore,
    /// 三态视图循环切档(三态视图):`源码 → 写作 → 源码+对照 → 源码`,
    /// 三档共用一个 rope buffer,切档不丢光标也不丢 undo 栈。默认键
    /// Cmd/Ctrl+/(与主流编辑器的「切换注释」同键位,工作台里没有注释语义)。
    /// 命令 id 沿用两态时代的 `toggle_live_preview`(keymap.json 零迁移)。
    ToggleLivePreview,
    /// 对照列开合(三态视图):非分栏 ⇄ 分栏。标题栏 PanelRight 按钮与
    /// `Cmd/Ctrl+Alt+R`(P4 退役 ToggleRightPreview 后空出的旧键位)两个入口。
    ToggleCompare,
    // —— 格式(docs/ui-shell-redesign.md §6.3,左侧即工具条顺序)——
    /// 加粗。Ctrl/Cmd+B — 这里兑现了 command.rs 原先「Ctrl+B 留给将来的
    /// 加粗」的注释,ToggleSidebar 因此早在当初就避开了 Ctrl+B。
    FormatBold,
    /// 斜体。
    FormatItalic,
    /// 删除线。
    FormatStrike,
    /// 行内代码。反引号;`Cmd+E` 已被 ExportHtml 占,取 VS Code 同款。
    FormatInlineCode,
    /// 链接。
    FormatLink,
    /// 一级标题。
    FormatH1,
    /// 二级标题。
    FormatH2,
    /// 三级标题。
    FormatH3,
    /// 引用。
    FormatQuote,
    /// 围栏代码块(info string 空)。
    FormatCodeBlock,
    /// 分割线。
    FormatDivider,
    /// 2×2 表格骨架。
    FormatTable,
    /// 无序列表。Shift+8 是 VS Code 同款。
    FormatBullet,
    /// 有序列表。
    FormatOrdered,
    /// 任务列表(三态循环)。
    FormatTask,
    /// 插入图片(docs/image-plan.md A 段):开「图片框」对话框,填 alt 与
    /// URL 后插入 `![alt](url)`。
    ///
    /// 与上面十六条不同 —— 那些点了就直接改文档,这条要先收两个输入,因此
    /// 归约是「开对话框」而不是「执行格式」。它也没有
    /// [`Self::format_action`]:真正的文本动作由 `compose::insert_image` 承担。
    ImageInsert,
    /// 插入 Emoji(docs/emoji-plan.md E1):开 Emoji 面板,点选即在光标处
    /// 插入纯 Unicode 字符。与 `ImageInsert` 同属「对话框类动作」—— emoji
    /// 字符无法从 text+sel 推导,不走 `format_action`;真正的文本动作由
    /// `compose::insert_emoji` 承担。
    EmojiPicker,
    /// 复制选中(无选中复制当前行)——编辑器语义,坤哥 2026-09-29 指令。
    DuplicateSelection,
    /// 复制当前行(选区多行时复制全部涉及行)。
    DuplicateLine,
    /// 打开文档内查找条(Ctrl+F)。
    FindInDoc,
    /// 打开查找条的替换行(Ctrl+H,查找条同开);已展开时再按收起替换行。
    ReplaceInDoc,
    /// 「跳转到行」浮条(#60 M1,Ctrl+G):输入行号回车,编辑器光标落该
    /// 行行首并滚动到可见;源码/Live 两模式同一 `jump_to` 入口。
    GotoLine,
    /// 插入/更新目录(#66 M2):有可收录标题(h1–h3)的文档,首次执行在
    /// 文档头插入标记包围的 TOC 块,再执行整块替换既有块(识别与替换
    /// 口径 decisions-pending #127,单步 undo);无可收录标题只落提示行。
    /// 归位「编辑」:与复制行/查找/替换/跳转同族,都是不作用选区的文档级
    /// 辅助动作(格式菜单十五条全是选区/光标处的格式化)。
    InsertToc,
    /// 禅定模式(§7)。F11:`KeyboardShortcut` 允许无修饰的 F1-F12。
    ToggleZen,
    /// 打字机模式(#64 M1):光标所在行滚动保持视口 1/3 线(源码与 Live
    /// 两模式同一开关)。Ctrl/Cmd+Alt+W:出厂表 W 键无第二个占用者
    /// (Ctrl/Cmd+W 是关标签,差一个 Alt 由「修饰键个数降序」的消费顺序
    /// 共存,与 EmojiPicker/ExportHtml 同款),Alt 层字母 F/E/O/V/X/A/S/T
    /// 已被菜单标题助记与主题键占用(#119/#45 K1),W 空闲 —— 避让口径
    /// 与冲突审计见 decisions-pending #121。
    TypewriterToggle,
    /// 专注模式(#64 M2):Live 模式淡化非活动块(活动块与光标邻块保持
    /// 全对比度;纯绘制层,淡化块仍可点击进入编辑)。源码模式不接线。
    /// Ctrl/Cmd+Alt+D(D = Dim,淡化非活动块):出厂表 Ctrl/Cmd+Alt 层
    /// 只有 R/W(切预览栏/打字机),D 空闲;含 Alt 的绑定按保守口径避开
    /// 菜单标题助记集(F/E/O/V/X/A/S),D 不在其中(menubar 标题审计
    /// 测试钉住)—— 首选 F 因「文件」标题撞键被该审计否决,避让口径与
    /// 冲突审计见 decisions-pending #122。
    FocusModeToggle,
    /// Minimap 缩略图开关(#67 M2):源码模式编辑区右缘缩略导航的显隐,
    /// 落 `theme.show_minimap`(#55 M2 既有配置,设置外观页复选框三方
    /// 一致)。Ctrl/Cmd+Alt+M(M = Minimap 词首):出厂表 Ctrl/Cmd+Alt
    /// 层现有 C/W/D/R(目录/打字机/专注/预览栏)之外 M 空闲,且 M 不在
    /// 菜单标题助记集(F/E/O/V/X/A/S)—— 避让口径与冲突审计见
    /// decisions-pending #129。
    ToggleMinimap,
}

impl Command {
    /// 文件组:菜单「文件」子菜单与工具栏按钮共用的顺序。
    pub const FILE: [Command; 4] = [Self::New, Self::Open, Self::Save, Self::SaveAs];

    /// 全部命令(快捷键设置页与绑定表遍历的顺序,见 `crate::keymap`)。
    ///
    /// 顺序 = UI 上的自然归属:文件 → 视图 → AI → 标签 → 格式按工具条分组
    /// 从左到右。
    pub const ALL: [Command; 44] = [
        Self::New,
        Self::Open,
        Self::QuickOpen,
        Self::Save,
        Self::SaveAs,
        Self::ExportHtml,
        Self::ExportPdf,
        Self::ToggleTheme,
        Self::ToggleSidebar,
        Self::AiMockStream,
        Self::AiCommitMessage,
        Self::AiSummary,
        Self::TabNext,
        Self::TabClose,
        Self::TabRestore,
        Self::ToggleLivePreview,
        Self::ToggleCompare,
        Self::FormatBold,
        Self::FormatItalic,
        Self::FormatStrike,
        Self::FormatInlineCode,
        Self::FormatLink,
        Self::FormatH1,
        Self::FormatH2,
        Self::FormatH3,
        Self::FormatQuote,
        Self::FormatCodeBlock,
        Self::FormatDivider,
        Self::FormatTable,
        Self::FormatBullet,
        Self::FormatOrdered,
        Self::FormatTask,
        Self::ImageInsert,
        Self::EmojiPicker,
        Self::DuplicateSelection,
        Self::DuplicateLine,
        Self::FindInDoc,
        Self::ReplaceInDoc,
        Self::GotoLine,
        Self::InsertToc,
        Self::ToggleZen,
        Self::TypewriterToggle,
        Self::FocusModeToggle,
        Self::ToggleMinimap,
    ];

    /// 格式命令 → 对应的动作,非格式命令为 `None`。
    pub fn format_action(self) -> Option<FormatAction> {
        Some(match self {
            Self::FormatBold => FormatAction::Bold,
            Self::FormatItalic => FormatAction::Italic,
            Self::FormatStrike => FormatAction::Strike,
            Self::FormatInlineCode => FormatAction::InlineCode,
            Self::FormatLink => FormatAction::Link,
            Self::FormatH1 => FormatAction::H1,
            Self::FormatH2 => FormatAction::H2,
            Self::FormatH3 => FormatAction::H3,
            Self::FormatQuote => FormatAction::Quote,
            Self::FormatCodeBlock => FormatAction::CodeBlock,
            Self::FormatDivider => FormatAction::Divider,
            Self::FormatTable => FormatAction::Table,
            Self::FormatBullet => FormatAction::Bullet,
            Self::FormatOrdered => FormatAction::Ordered,
            Self::FormatTask => FormatAction::Task,
            Self::DuplicateSelection => FormatAction::DuplicateSelection,
            Self::DuplicateLine => FormatAction::DuplicateLine,
            _ => return None,
        })
    }

    /// 命令的分组(蒙层/设置页「快捷键」的展示维度)。全部命令都有归属,
    /// 漏归类的落 [`CommandGroup::Other`] 兜底。
    pub fn group(self) -> CommandGroup {
        match self {
            Self::New | Self::Open | Self::QuickOpen | Self::Save | Self::SaveAs => {
                CommandGroup::File
            }
            Self::DuplicateSelection
            | Self::DuplicateLine
            | Self::FindInDoc
            | Self::ReplaceInDoc
            | Self::GotoLine
            | Self::InsertToc => CommandGroup::Edit,
            Self::FormatBold
            | Self::FormatItalic
            | Self::FormatStrike
            | Self::FormatInlineCode
            | Self::FormatLink
            | Self::FormatH1
            | Self::FormatH2
            | Self::FormatH3
            | Self::FormatQuote
            | Self::FormatCodeBlock
            | Self::FormatDivider
            | Self::FormatTable
            | Self::FormatBullet
            | Self::FormatOrdered
            | Self::FormatTask
            | Self::ImageInsert
            | Self::EmojiPicker => CommandGroup::Format,
            Self::ToggleTheme
            | Self::ToggleSidebar
            | Self::ToggleLivePreview
            | Self::ToggleCompare
            | Self::ToggleZen
            | Self::TypewriterToggle
            | Self::FocusModeToggle
            | Self::ToggleMinimap => CommandGroup::View,
            Self::TabNext | Self::TabClose | Self::TabRestore => CommandGroup::Tab,
            Self::ExportHtml | Self::ExportPdf => CommandGroup::Export,
            Self::AiMockStream | Self::AiCommitMessage | Self::AiSummary => CommandGroup::Ai,
        }
    }

    /// 稳定 id:快捷键表 `keymap.json` 的键。命令的显示名会随文案调整,
    /// id 不随,存档才不会因改 label 而失效。
    pub fn id(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Open => "open",
            Self::QuickOpen => "quick_open",
            Self::Save => "save",
            Self::SaveAs => "save_as",
            Self::ExportHtml => "export_html",
            Self::ExportPdf => "export_pdf",
            Self::ToggleTheme => "toggle_theme",
            Self::ToggleSidebar => "toggle_sidebar",
            Self::AiMockStream => "ai_mock_stream",
            Self::AiCommitMessage => "ai_commit_message",
            Self::AiSummary => "ai_summary",
            Self::TabNext => "tab_next",
            Self::TabClose => "tab_close",
            Self::TabRestore => "tab_restore",
            Self::ToggleLivePreview => "toggle_live_preview",
            Self::ToggleCompare => "toggle_compare",
            Self::FormatBold => "format_bold",
            Self::FormatItalic => "format_italic",
            Self::FormatStrike => "format_strike",
            Self::FormatInlineCode => "format_inline_code",
            Self::FormatLink => "format_link",
            Self::FormatH1 => "format_h1",
            Self::FormatH2 => "format_h2",
            Self::FormatH3 => "format_h3",
            Self::FormatQuote => "format_quote",
            Self::FormatCodeBlock => "format_code_block",
            Self::FormatDivider => "format_divider",
            Self::FormatTable => "format_table",
            Self::FormatBullet => "format_bullet",
            Self::FormatOrdered => "format_ordered",
            Self::FormatTask => "format_task",
            Self::ImageInsert => "image_insert",
            Self::EmojiPicker => "emoji_picker",
            Self::DuplicateSelection => "duplicate_selection",
            Self::DuplicateLine => "duplicate_line",
            Self::FindInDoc => "find_in_doc",
            Self::ReplaceInDoc => "replace_in_doc",
            Self::GotoLine => "goto_line",
            Self::InsertToc => "insert_toc",
            Self::ToggleZen => "toggle_zen",
            Self::TypewriterToggle => "toggle_typewriter",
            Self::FocusModeToggle => "toggle_focus_mode",
            Self::ToggleMinimap => "toggle_minimap",
        }
    }

    /// 菜单与按钮的显示名。
    pub fn label(self) -> &'static str {
        match self {
            Self::New => "新建",
            Self::Open => "打开",
            Self::QuickOpen => "快速打开…",
            Self::Save => "保存",
            Self::SaveAs => "另存为",
            Self::ExportHtml => "导出 HTML",
            Self::ExportPdf => "导出 PDF",
            Self::ToggleTheme => "切换主题",
            Self::ToggleSidebar => "切换侧边栏",
            Self::AiMockStream => "AI: Mock 流式续写",
            Self::AiCommitMessage => "AI: 生成 commit message",
            Self::AiSummary => "AI: 生成摘要",
            Self::TabNext => "下一个标签",
            Self::TabClose => "关闭标签",
            Self::TabRestore => "恢复关闭的标签",
            // 标签列宽固定(SETTINGS_LABEL_W),写长了会撑破快捷键页的两列对齐
            Self::ToggleLivePreview => "切换视图",
            Self::ToggleCompare => "对照列",
            Self::FormatBold => FormatAction::Bold.label(),
            Self::FormatItalic => FormatAction::Italic.label(),
            Self::FormatStrike => FormatAction::Strike.label(),
            Self::FormatInlineCode => FormatAction::InlineCode.label(),
            Self::FormatLink => FormatAction::Link.label(),
            Self::FormatH1 => FormatAction::H1.label(),
            Self::FormatH2 => FormatAction::H2.label(),
            Self::FormatH3 => FormatAction::H3.label(),
            Self::FormatQuote => FormatAction::Quote.label(),
            Self::FormatCodeBlock => FormatAction::CodeBlock.label(),
            Self::FormatDivider => FormatAction::Divider.label(),
            Self::FormatTable => FormatAction::Table.label(),
            Self::FormatBullet => FormatAction::Bullet.label(),
            Self::FormatOrdered => FormatAction::Ordered.label(),
            Self::FormatTask => FormatAction::Task.label(),
            // 图片单独取名:工具条按钮显示的是 `FormatAction::Image` 的
            // 「图片」,命令层要的是动作名「插入图片」(快捷键设置页里
            // 「图片」两个字说不清是干什么的)
            Self::ImageInsert => "插入图片",
            Self::EmojiPicker => "插入 Emoji",
            Self::DuplicateSelection => "复制选中(Ctrl+D)",
            Self::DuplicateLine => "复制当前行",
            Self::FindInDoc => "查找",
            Self::ReplaceInDoc => "替换",
            Self::GotoLine => "跳转到行",
            Self::InsertToc => "插入目录",
            Self::ToggleZen => "禅定模式",
            Self::TypewriterToggle => "打字机模式",
            Self::FocusModeToggle => "专注模式",
            Self::ToggleMinimap => "Minimap 缩略图",
        }
    }

    /// **出厂默认**的快捷键;`None` = 不绑定(菜单里只显示名字)。
    ///
    /// 实际生效的键位在 [`crate::keymap::Keymap`](用户可改,`keymap.json`);
    /// 这里只是默认值来源。全部文件/视图命令都有默认绑定,菜单栏负责展示
    /// 以保证可发现性;AI 命令一律 `None`:联调入口不抢键位,等 provider
    /// 选型定案再定。
    ///
    /// ToggleSidebar 取 Ctrl/Cmd+\\ 而非更常见的 Ctrl+B:Markdown 工作台的
    /// Ctrl+B 要留给将来的加粗(与主流 Markdown 编辑器一致)。
    pub fn default_shortcut(self) -> Option<egui::KeyboardShortcut> {
        let shortcut = match self {
            Self::New => egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::N),
            Self::Open => egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::O),
            // 快速打开(#24):VS Code / 主流编辑器同款;出厂表 P 键无占用者
            Self::QuickOpen => egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::P),
            Self::Save => egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::S),
            Self::SaveAs => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::S)
            }
            Self::ExportHtml => egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::E),
            // 主题 → Alt+T(preview-typography-and-keymap-plan.md §3.2 定案):
            // Cmd/Ctrl+Shift+T 让位给 TabRestore(浏览器「恢复关闭标签」同款,
            // #45 K2)。Alt 系与 VS Code「颜色主题」习惯相通;Alt 是修饰键,
            // `bindable()` 不需要开后门。旧 keymap.json 里值仍等于旧默认
            // (Cmd/Ctrl+Shift+T)的条目由 `keymap::load_from` 迁移到新默认。
            Self::ToggleTheme => egui::KeyboardShortcut::new(Modifiers::ALT, egui::Key::T),
            Self::ToggleSidebar => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Backslash)
            }
            Self::TabNext => egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Tab),
            Self::TabClose => egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::W),
            // 恢复关闭的标签(#45 K2):浏览器/VS Code 同款。K1 已把主题
            // 改排 Alt+T 让出此键位,出厂表占用者就是本命令。
            Self::TabRestore => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::T)
            }
            Self::ToggleLivePreview => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Slash)
            }
            Self::FormatBold => egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::B),
            Self::FormatItalic => egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::I),
            Self::FormatStrike => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::X)
            }
            Self::FormatInlineCode => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Backtick)
            }
            // 链接加 Shift 而非 VS Code 的裸 Ctrl+K:本项目 Ctrl+K 被快捷
            // 键设置的捕获模式测试当作「任意空闲键」,占上去会让那条回归失真。
            Self::FormatLink => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::K)
            }
            Self::FormatH1 => egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Num1),
            Self::FormatH2 => egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Num2),
            Self::FormatH3 => egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Num3),
            Self::FormatQuote => egui::KeyboardShortcut::new(
                Modifiers::COMMAND | Modifiers::SHIFT,
                egui::Key::Period,
            ),
            Self::FormatCodeBlock => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::C)
            }
            Self::FormatBullet => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::Num8)
            }
            Self::FormatOrdered => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::Num7)
            }
            Self::FormatTask => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::Num9)
            }
            // 图片:加 Shift 是因为 Ctrl/Cmd+I 已是斜体(与链接同款处理:
            // 占用键位前先看格式命令已经占过什么)。
            Self::ImageInsert => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::I)
            }
            // Emoji 面板:Ctrl/Cmd+Shift+E 在出厂表里无占用者(与导出的
            // Ctrl/Cmd+E 只差一个 Shift;消费顺序按修饰键个数降序,Shift
            // 组合先被问到,两者互不抢 —— 与 SaveAs 之于 Save 同款共存,
            // 见 `emoji_shortcut_overlaps_export_but_does_not_conflict`)。
            Self::EmojiPicker => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::E)
            }
            Self::DuplicateSelection => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::D)
            }
            Self::DuplicateLine => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::D)
            }
            Self::FindInDoc => egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::F),
            // Ctrl+H:Win/Linux 主流编辑器的替换键位,出厂表无占用者。mac
            // 例外:⌘H 被 winit 默认应用菜单的「隐藏窗口」(Hide,
            // `sel!(hide:)`)持有,Cocoa 主菜单按键匹配先于 egui —— 按键
            // 到不了应用,命令成了死键且副作用是整个窗口消失(winit
            // `menu.rs` 默认建菜单,eframe 未关闭)。mac 出厂改排 ⌥⌘F
            // (VS Code / Sublime 的 mac 替换键惯例,与 ⌘F 查找同族);
            // 老档 Cmd+H 的值感知迁移见 `keymap::load_from`,取舍全文见
            // decisions-pending #114。
            Self::ReplaceInDoc => {
                if cfg!(target_os = "macos") {
                    egui::KeyboardShortcut::new(Modifiers::ALT | Modifiers::COMMAND, egui::Key::F)
                } else {
                    egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::H)
                }
            }
            // Ctrl/Cmd+G(#60 M1):VS Code 的跳转到行键位;出厂表 G 键无占用者
            // (mac 侧 ⌘G 与系统「查找下一个」惯例的取舍见 decisions-pending #113)
            Self::GotoLine => egui::KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::G),
            // 插入目录(#66 M2):Ctrl/Cmd+Alt+C(C = Contents)。出厂表
            // Ctrl/Cmd+Alt 层的字母键现有 R/W/D/M(对照列开合/打字机/专注/
            // minimap);R 在 P4 摘除 ToggleRightPreview 后由 ToggleCompare
            // 复用,**不是空键**。C 不在其中,故空闲(#9 口径的撞键
            // 核查在 tests);含 Alt 的绑定按保守口径避开
            // 菜单标题助记集(F/E/O/V/X/A/S),C 不在其中(与 #122 同款审计)。
            // 与 Ctrl/Cmd+Shift+C(围栏代码块)同键不同修饰,egui 的
            // `matches_logically` 只把 Shift 当「可忽略的额外键」,Alt 必须
            // 显式按下 —— 两个方向互不误触(egui 0.36.2 modifiers.rs:
            // matches_logically 实读核验)。
            Self::InsertToc => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::ALT, egui::Key::C)
            }
            // 对照列开合(三态视图):Cmd/Ctrl+Alt+R —— P4 退役
            // ToggleRightPreview 后空出的键位,肌肉记忆延续到同一块屏幕区域。
            Self::ToggleCompare => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::ALT, egui::Key::R)
            }
            Self::ToggleZen => egui::KeyboardShortcut::new(Modifiers::NONE, egui::Key::F11),
            // 打字机模式(#64 M1):Ctrl/Cmd+Alt+W。出厂表整条 Shortcut 无
            // 占用者;与 Ctrl/Cmd+W(关标签)只差一个 Alt,消费顺序按修饰键
            // 个数降序互不抢(#9 口径的撞键核查在 tests)。Alt 层纯字母
            // (F/E/O/V/X/A/S/T)是菜单标题助记与主题键的命名空间,避让
            // 口径见 decisions-pending #121。
            Self::TypewriterToggle => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::ALT, egui::Key::W)
            }
            // 专注模式(#64 M2):Ctrl/Cmd+Alt+D。出厂表 Ctrl/Cmd+Alt 层
            // 只有 R(对照列开合)与 W(打字机),D 在该层空闲(#9 口径的
            // 撞键核查在 tests);含 Alt 的绑定按保守口径避开菜单标题助记
            // 集(F/E/O/V/X/A/S),D 不在其中 —— 首选 F 被该审计否决
            // (menubar 标题审计),取舍见 decisions-pending #122。
            Self::FocusModeToggle => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::ALT, egui::Key::D)
            }
            // Minimap 开关(#67 M2):Ctrl/Cmd+Alt+M。出厂表 Ctrl/Cmd+Alt 层
            // 现有 R/W/D/C(对照列开合/打字机/专注/插入目录),M 在该层空闲
            // (#9 口径的撞键核查在 tests);含 Alt 的绑定按保守口径避开
            // 菜单标题助记集(F/E/O/V/X/A/S),M 不在其中(与 #121/#122/
            // #127 同款审计,menubar 的标题审计测试共同钉住)。
            Self::ToggleMinimap => {
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::ALT, egui::Key::M)
            }
            // 无快捷键。前三条是 AI 联调入口:不抢键位,等 provider 选型
            // 定案再定;FormatDivider/FormatTable 只从工具条按钮触发
            // (插画布性质的动作,不像加粗那样高频到需要键位);ExportPdf
            // 不强求键位 —— Ctrl/Cmd+E 已被导出 HTML 占用,强凑一对键的
            // 收益盖不过撞键风险,只挂菜单与侧边栏按钮。
            Self::AiMockStream
            | Self::AiCommitMessage
            | Self::AiSummary
            | Self::FormatDivider
            | Self::FormatTable
            | Self::ExportPdf => return None,
        };
        Some(shortcut)
    }

    /// 图标(工具栏按钮用;`ui::icons` 自绘,不依赖字体)。
    pub fn icon(self) -> crate::ui::icons::Icon {
        use crate::ui::icons::Icon;
        match self {
            Self::New => Icon::New,
            Self::Open => Icon::Open,
            // 快速打开是「搜文件」,与文档内查找共用放大镜
            Self::QuickOpen => Icon::Search,
            Self::Save => Icon::Save,
            Self::SaveAs => Icon::SaveAs,
            // 导出两项**刻意用不同图标**(2026-10-08 症状 B):左栏顶段是
            // 6 枚纯图标无文字并排,原先两者共用 `Icon::Export` → 相邻两枚
            // 一模一样,tooltip 是唯一区分手段,实际不可辨识。
            // 语义也站得住:HTML = 下载一个文件(箭头入托盘),PDF = 打印
            // (机身出纸)。共享白名单见本文件测试
            // `intentional_icon_sharing_is_the_whole_set`。
            Self::ExportHtml => Icon::Export,
            Self::ExportPdf => Icon::Print,
            Self::ToggleTheme => Icon::Theme,
            Self::ToggleSidebar => Icon::Sidebar,
            Self::AiMockStream | Self::AiCommitMessage | Self::AiSummary => Icon::Ai,
            Self::TabNext | Self::TabClose | Self::TabRestore => Icon::Files,
            Self::ToggleLivePreview => Icon::Zen,
            Self::ToggleCompare => Icon::PanelRight,
            Self::FormatBold
            | Self::FormatItalic
            | Self::FormatStrike
            | Self::FormatH1
            | Self::FormatH2
            | Self::FormatH3 => Icon::Table,
            Self::FormatInlineCode => Icon::CodeInline,
            Self::FormatLink => Icon::Link,
            Self::FormatQuote => Icon::Quote,
            Self::FormatCodeBlock => Icon::CodeBlock,
            Self::FormatDivider => Icon::Divider,
            Self::FormatTable => Icon::Table,
            Self::FormatBullet => Icon::BulletList,
            Self::FormatOrdered => Icon::OrderedList,
            Self::FormatTask => Icon::TaskList,
            Self::ImageInsert => Icon::Image,
            Self::EmojiPicker => Icon::Emoji,
            Self::DuplicateSelection
            | Self::DuplicateLine
            | Self::FindInDoc
            | Self::ReplaceInDoc
            | Self::GotoLine => Icon::Search,
            // 与大纲页签同源(icons 无目录专用形;「多命令一图标」先例:
            // ToggleLivePreview/ToggleZen 同源)。缩进列表即 TOC 的形。
            Self::InsertToc => Icon::Outline,
            Self::ToggleZen => Icon::Zen,
            // 与视图组两个开关共用禅定同心圆(icons 无打字机专用形;既有
            // 「多命令一图标」先例:ToggleLivePreview/ToggleZen 同源)
            Self::TypewriterToggle => Icon::Zen,
            // 与视图组开关同源(#64 M1 同款先例:icons 无专注专用形)
            Self::FocusModeToggle => Icon::Zen,
            // 与视图组开关同源(#67 M2 同款先例:icons 无 minimap 专用形,
            // 只进菜单/快捷键/快速打开,不新增工具栏按钮)
            Self::ToggleMinimap => Icon::Zen,
        }
    }

    /// 归约入口:命令翻成状态消息,执行在 `State::apply`。
    pub fn message(self) -> Message {
        match self {
            Self::New => Message::FileCommand(FileCmd::New),
            Self::Open => Message::FileCommand(FileCmd::Open),
            Self::QuickOpen => Message::ToggleQuickOpen,
            Self::Save => Message::FileCommand(FileCmd::Save),
            Self::SaveAs => Message::FileCommand(FileCmd::SaveAs),
            Self::ExportHtml => Message::ExportHtml,
            Self::ExportPdf => Message::ExportPdf,
            Self::ToggleTheme => Message::ToggleTheme,
            Self::ToggleSidebar => Message::SidebarToggled,
            Self::AiMockStream => Message::AiStart,
            Self::AiCommitMessage => Message::AiCommitRequested,
            Self::AiSummary => Message::AiSummaryRequested,
            Self::ImageInsert => Message::ImageDialogOpened,
            Self::EmojiPicker => Message::EmojiPickerToggle(true),
            Self::ToggleLivePreview => Message::ToggleLivePreview,
            Self::ToggleCompare => Message::CompareColumnToggled,
            Self::TabNext => Message::TabNext,
            Self::TabClose => Message::TabCloseActive,
            Self::TabRestore => Message::TabRestore,
            Self::ToggleZen => Message::ZenToggled,
            // 菜单/快捷键是「翻转」语义(现值在 State 里,消息层拿不到),
            // 走无参翻转消息(与 ToggleLivePreview/ZenToggled 同构);带值
            // 的 TypewriterToggled(bool) 归设置页复选框专用。
            Self::TypewriterToggle => Message::ToggleTypewriter,
            Self::FocusModeToggle => Message::ToggleFocusMode,
            // 与打字机/专注同构:菜单/快捷键是「翻转」语义(现值在 State,
            // 消息层拿不到),走无参翻转消息;带值的 ShowMinimapToggled(bool)
            // 归设置外观页复选框专用(#55 M2 既有)。
            Self::ToggleMinimap => Message::ToggleMinimap,
            // 十六条格式动作一条 match 收干:动作枚举已经在 cmd 里定死了,
            // 这里只把它装进消息,语义一律看 `compose::apply`
            Self::FormatBold
            | Self::FormatItalic
            | Self::FormatStrike
            | Self::FormatInlineCode
            | Self::FormatLink
            | Self::FormatH1
            | Self::FormatH2
            | Self::FormatH3
            | Self::FormatQuote
            | Self::FormatCodeBlock
            | Self::FormatDivider
            | Self::FormatTable
            | Self::FormatBullet
            | Self::FormatOrdered
            | Self::FormatTask
            | Self::DuplicateSelection
            | Self::DuplicateLine => Message::FormatRequested(self.format_action().unwrap()),
            Self::FindInDoc => Message::FindBarToggled(true),
            Self::ReplaceInDoc => Message::ReplaceBarToggled(true),
            Self::GotoLine => Message::GotoBarToggled(true),
            Self::InsertToc => Message::InsertToc,
        }
    }
}

/// 从本帧输入消费全部命令快捷键,返回被触发的命令。
///
/// 键位取自 `keymap`(用户可改)而非出厂默认;**消费顺序按修饰键个数降序**
/// —— 通用化了原先「SaveAs 必须先于 Save」的特例:`consume_shortcut` 底层的
/// `matches_logically` 忽略多余 Shift,先问 Ctrl/Cmd+S 的话 Ctrl/Cmd+Shift+S
/// 会被它抢先吃掉(egui 文档要求 most specific first)。
///
/// 只在 `App::logic` 调用:logic 先于 `App::ui` 运行,本帧按键事件此刻
/// 可见;消费即从输入流移除,TextEdit 即使聚焦也收不到。普通字符输入
/// (无 COMMAND 修饰)不匹配任何绑定,原样放行给控件。
pub fn poll_shortcuts(ctx: &egui::Context, keymap: &crate::keymap::Keymap) -> Vec<Command> {
    let mut bound: Vec<(Command, crate::keymap::Shortcut)> = Command::ALL
        .iter()
        .filter_map(|cmd| keymap.get(*cmd).map(|shortcut| (*cmd, shortcut)))
        .collect();
    // 修饰键多的先匹配(most specific first);同位数保持 ALL 的顺序,
    // 排序用稳定排序
    bound.sort_by_key(|(_, shortcut)| std::cmp::Reverse(modifier_count(shortcut.modifiers)));
    bound
        .into_iter()
        .filter_map(|(cmd, shortcut)| {
            ctx.input_mut(|input| input.consume_shortcut(&shortcut.keyboard()))
                .then_some(cmd)
        })
        .collect()
}

/// 修饰键个数(决定消费优先级)。
fn modifier_count(modifiers: egui::Modifiers) -> u32 {
    u32::from(modifiers.command)
        + u32::from(modifiers.shift)
        + u32::from(modifiers.alt)
        + u32::from(modifiers.mac_cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::Keymap;
    use egui::{Event, Key, RawInput};

    fn key_event(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// Ctrl/Cmd+Shift+S 只触发 SaveAs 一条;`matches_logically` 忽略多余
    /// Shift,若先消费 Save 会误中(顺序约束见 [`POLL_ORDER`] 文档)。
    #[test]
    fn shift_save_fires_only_save_as() {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            RawInput {
                events: vec![key_event(Key::S, Modifiers::COMMAND | Modifiers::SHIFT)],
                ..Default::default()
            },
            |ui| {
                assert_eq!(
                    poll_shortcuts(ui.ctx(), &Keymap::builtin()),
                    vec![Command::SaveAs]
                );
            },
        );
        output.drop_without_applying_deltas();
    }

    #[test]
    fn plain_save_fires_only_save() {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            RawInput {
                events: vec![key_event(Key::S, Modifiers::COMMAND)],
                ..Default::default()
            },
            |ui| {
                assert_eq!(
                    poll_shortcuts(ui.ctx(), &Keymap::builtin()),
                    vec![Command::Save]
                );
            },
        );
        output.drop_without_applying_deltas();
    }

    /// Emoji 面板键位的撞键核查(emoji-plan §6.3 / decisions-pending #9
    /// 口径):出厂表里无占用者;它与导出(Ctrl/Cmd+E)只差一个 Shift,
    /// `consume_shortcut` 的 `matches_logically` 会忽略多余 Shift —— 靠
    /// 「修饰键个数降序」的消费顺序共存(与 SaveAs/Save 同款):Shift 组合
    /// 先被问到,两个键各自只触发一条命令。
    #[test]
    fn emoji_shortcut_overlaps_export_but_does_not_conflict() {
        let emoji = crate::keymap::Shortcut {
            modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
            key: Key::E,
        };
        // #9 口径的静态核查:撞键检测按整条 Shortcut 相等,出厂表无人占用
        assert_eq!(
            Keymap::builtin().conflict(Command::EmojiPicker, emoji),
            None,
            "Ctrl/Cmd+Shift+E 不该撞任何出厂键位"
        );

        for (key, modifiers, expected) in [
            (
                Key::E,
                Modifiers::COMMAND | Modifiers::SHIFT,
                vec![Command::EmojiPicker],
            ),
            (Key::E, Modifiers::COMMAND, vec![Command::ExportHtml]),
        ] {
            let ctx = egui::Context::default();
            let output = ctx.run_ui(
                RawInput {
                    events: vec![key_event(key, modifiers)],
                    ..Default::default()
                },
                |ui| {
                    assert_eq!(poll_shortcuts(ui.ctx(), &Keymap::builtin()), expected);
                },
            );
            output.drop_without_applying_deltas();
        }
    }

    /// 全部命令的绑定都能被各自按键触发,且消费一次后同帧不回流。
    #[test]
    fn every_command_fires_exactly_once() {
        let ctx = egui::Context::default();
        let bindings = [
            (Command::New, Key::N, Modifiers::COMMAND),
            (Command::Open, Key::O, Modifiers::COMMAND),
            (Command::ExportHtml, Key::E, Modifiers::COMMAND),
            (Command::ToggleTheme, Key::T, Modifiers::ALT),
            (Command::ToggleSidebar, Key::Backslash, Modifiers::COMMAND),
        ];
        for (cmd, key, modifiers) in bindings {
            let output = ctx.run_ui(
                RawInput {
                    events: vec![key_event(key, modifiers)],
                    ..Default::default()
                },
                |ui| {
                    let ctx = ui.ctx().clone();
                    assert_eq!(poll_shortcuts(&ctx, &Keymap::builtin()), vec![cmd]);
                    assert!(
                        poll_shortcuts(&ctx, &Keymap::builtin()).is_empty(),
                        "同一帧重复消费"
                    );
                },
            );
            output.drop_without_applying_deltas();
        }
    }

    /// 无修饰的普通字符不是任何命令的快捷键(不劫持 TextEdit 输入的
    /// 第一道保证;第二道是 logic 阶段消费、ui 阶段控件见不到,见
    /// `ui::layout` 的集成测试)。
    #[test]
    fn bare_keys_do_not_fire() {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            RawInput {
                events: vec![
                    key_event(Key::S, Modifiers::NONE),
                    key_event(Key::N, Modifiers::NONE),
                    key_event(Key::E, Modifiers::NONE),
                    key_event(Key::T, Modifiers::NONE),
                ],
                ..Default::default()
            },
            |ui| {
                assert!(poll_shortcuts(ui.ctx(), &Keymap::builtin()).is_empty());
            },
        );
        output.drop_without_applying_deltas();
    }

    /// 命令到消息的映射:文件组与导出走既有消息,两个开关是新消息。
    #[test]
    fn message_mapping_covers_all_commands() {
        assert_eq!(Command::New.message(), Message::FileCommand(FileCmd::New));
        assert_eq!(Command::Open.message(), Message::FileCommand(FileCmd::Open));
        assert_eq!(Command::Save.message(), Message::FileCommand(FileCmd::Save));
        assert_eq!(
            Command::SaveAs.message(),
            Message::FileCommand(FileCmd::SaveAs)
        );
        assert_eq!(Command::ExportHtml.message(), Message::ExportHtml);
        assert_eq!(Command::ExportPdf.message(), Message::ExportPdf);
        assert_eq!(Command::ToggleTheme.message(), Message::ToggleTheme);
        assert_eq!(Command::ToggleSidebar.message(), Message::SidebarToggled);
        assert_eq!(Command::AiMockStream.message(), Message::AiStart);
        assert_eq!(
            Command::AiCommitMessage.message(),
            Message::AiCommitRequested
        );
        assert_eq!(Command::AiSummary.message(), Message::AiSummaryRequested);
    }

    /// AI 命令不绑快捷键(ask 约束:避免与现有键位冲突),其余命令全部有绑定。
    #[test]
    fn only_ai_commands_lack_shortcut() {
        for cmd in [
            Command::New,
            Command::Open,
            Command::Save,
            Command::SaveAs,
            Command::ExportHtml,
            Command::ToggleTheme,
            Command::ToggleSidebar,
        ] {
            assert!(cmd.default_shortcut().is_some(), "{cmd:?} 应有默认快捷键");
        }
        assert_eq!(Command::AiMockStream.default_shortcut(), None);
        assert_eq!(Command::AiCommitMessage.default_shortcut(), None);
        assert_eq!(Command::AiSummary.default_shortcut(), None);
        // 导出 PDF 刻意不绑默认键:Ctrl/Cmd+E 归 HTML,不强凑键位;仍可从
        // 菜单/侧边栏触发,快捷键设置页可自行改绑。
        assert_eq!(Command::ExportPdf.default_shortcut(), None);
    }

    /// 改绑生效:把「保存」改到 Ctrl+K 后,原 Ctrl+S 不再触发任何命令,
    /// 新键位触发保存 —— 键位来自 keymap 而不是硬编码。
    #[test]
    fn rebound_shortcut_replaces_default() {
        let mut keymap = Keymap::builtin();
        keymap.set(
            Command::Save,
            Some(crate::keymap::Shortcut {
                modifiers: Modifiers::COMMAND,
                key: Key::K,
            }),
        );

        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            RawInput {
                events: vec![key_event(Key::S, Modifiers::COMMAND)],
                ..Default::default()
            },
            |ui| {
                assert!(poll_shortcuts(ui.ctx(), &keymap).is_empty(), "旧键位已解绑");
            },
        );
        output.drop_without_applying_deltas();

        let output = ctx.run_ui(
            RawInput {
                events: vec![key_event(Key::K, Modifiers::COMMAND)],
                ..Default::default()
            },
            |ui| {
                assert_eq!(poll_shortcuts(ui.ctx(), &keymap), vec![Command::Save]);
            },
        );
        output.drop_without_applying_deltas();
    }

    /// 未绑定的命令不消费任何键(清除绑定 = 只能从菜单触发)。
    #[test]
    fn unbound_command_consumes_nothing() {
        let mut keymap = Keymap::builtin();
        keymap.set(Command::Save, None);
        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            RawInput {
                events: vec![key_event(Key::S, Modifiers::COMMAND)],
                ..Default::default()
            },
            |ui| {
                assert!(poll_shortcuts(ui.ctx(), &keymap).is_empty());
            },
        );
        output.drop_without_applying_deltas();
    }

    /// 替换命令(#17;#60 M2 mac 改排):出厂即绑且只触发这一条;命令
    /// 映射到替换条打开消息(开/关语义在归约侧翻转发)。出厂键位按编译
    /// 目标断言:mac = ⌥⌘F(⌘H 被 winit 默认菜单 Hide 项持有,#114),
    /// 其余 = Ctrl/Cmd+H,两形态都在各自平台 CI 上跑(与
    /// `alt_platform_text_per_target` 同口径)。
    #[test]
    fn replace_shortcut_fires_replace_command() {
        let shortcut = Command::ReplaceInDoc
            .default_shortcut()
            .expect("替换命令有出厂键位");
        let as_binding = crate::keymap::Shortcut {
            modifiers: shortcut.modifiers,
            key: shortcut.logical_key,
        };
        if cfg!(target_os = "macos") {
            assert_eq!(shortcut.modifiers, Modifiers::ALT | Modifiers::COMMAND);
            assert_eq!(shortcut.logical_key, Key::F, "mac 出厂 = ⌥⌘F");
        } else {
            assert_eq!(shortcut.modifiers, Modifiers::COMMAND);
            assert_eq!(shortcut.logical_key, Key::H, "Win/Linux 出厂 = Ctrl/Cmd+H");
        }
        assert_eq!(
            Keymap::builtin().get(Command::ReplaceInDoc),
            Some(as_binding),
            "keymap 出厂表与命令层默认一致"
        );
        assert_eq!(
            Keymap::builtin().conflict(Command::ReplaceInDoc, as_binding),
            None,
            "替换出厂键不撞任何其他出厂键位"
        );

        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            RawInput {
                events: vec![key_event(shortcut.logical_key, shortcut.modifiers)],
                ..Default::default()
            },
            |ui| {
                assert_eq!(
                    poll_shortcuts(ui.ctx(), &Keymap::builtin()),
                    vec![Command::ReplaceInDoc]
                );
            },
        );
        output.drop_without_applying_deltas();
        assert_eq!(
            Command::ReplaceInDoc.message(),
            Message::ReplaceBarToggled(true)
        );
    }

    /// 跳转命令(#60 M1):Ctrl/Cmd+G 出厂即绑、出厂表无 G 键占用者
    /// (#9 口径的撞键核查)、真按键只触发这一条;命令映射到跳转浮条
    /// 打开消息,分组归「编辑」(菜单入口与蒙层分组同一事实源)。
    #[test]
    fn goto_line_shortcut_fires_goto_command() {
        let ctrl_g = crate::keymap::Shortcut {
            modifiers: Modifiers::COMMAND,
            key: Key::G,
        };
        assert_eq!(
            Command::GotoLine.default_shortcut().map(|shortcut| {
                crate::keymap::Shortcut {
                    modifiers: shortcut.modifiers,
                    key: shortcut.logical_key,
                }
            }),
            Some(ctrl_g),
            "GotoLine 出厂默认 = Ctrl/Cmd+G"
        );
        assert_eq!(Keymap::builtin().get(Command::GotoLine), Some(ctrl_g));
        assert_eq!(
            Keymap::builtin().conflict(Command::GotoLine, ctrl_g),
            None,
            "Ctrl/Cmd+G 不该撞任何出厂键位"
        );
        assert_eq!(
            Command::GotoLine.group(),
            CommandGroup::Edit,
            "跳转是编辑命令(菜单「编辑」/蒙层同一归属)"
        );

        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            RawInput {
                events: vec![key_event(Key::G, Modifiers::COMMAND)],
                ..Default::default()
            },
            |ui| {
                assert_eq!(
                    poll_shortcuts(ui.ctx(), &Keymap::builtin()),
                    vec![Command::GotoLine]
                );
            },
        );
        output.drop_without_applying_deltas();
        assert_eq!(Command::GotoLine.message(), Message::GotoBarToggled(true));
    }

    /// 插入目录(#66 M2)默认键位:Ctrl/Cmd+Alt+C,出厂表整条 Shortcut 无
    /// 占用者(#9 口径的撞键核查);与 Ctrl/Cmd+Shift+C(围栏代码块)同键
    /// 不同修饰,`matches_logically` 的 Alt 必须显式按下(只忽略多余
    /// Shift),双向不误触 —— Shift 组合按键不触发本命令、Alt 组合按键不
    /// 触发代码块,两条对照都实测。含 Alt 的绑定避开菜单标题助记集
    /// (F/E/O/V/X/A/S),C 不在其中(#122 同款避让,menubar 的标题审计
    /// 测试共同钉住)。真按键只触发这一条,消息映射到
    /// `Message::InsertToc`,分组归「编辑」。
    #[test]
    fn insert_toc_shortcut_is_ctrl_alt_c_and_conflict_free() {
        let ctrl_alt_c = crate::keymap::Shortcut {
            modifiers: Modifiers::COMMAND | Modifiers::ALT,
            key: Key::C,
        };
        assert_eq!(
            Command::InsertToc.default_shortcut().map(|shortcut| {
                crate::keymap::Shortcut {
                    modifiers: shortcut.modifiers,
                    key: shortcut.logical_key,
                }
            }),
            Some(ctrl_alt_c),
            "InsertToc 出厂默认 = Ctrl/Cmd+Alt+C"
        );
        assert_eq!(Keymap::builtin().get(Command::InsertToc), Some(ctrl_alt_c));
        assert_eq!(
            Keymap::builtin().conflict(Command::InsertToc, ctrl_alt_c),
            None,
            "Ctrl/Cmd+Alt+C 不该撞任何出厂键位"
        );
        assert_eq!(
            Command::InsertToc.group(),
            CommandGroup::Edit,
            "插入目录是编辑命令(菜单「编辑」/蒙层同一归属)"
        );

        // Alt 组合先被问到(修饰键个数降序),只触发本命令;Shift 同键对照
        // 不误触(matches_logically 对 Alt 的显式要求,双向实证)
        for (key, modifiers, expected) in [
            (
                Key::C,
                Modifiers::COMMAND | Modifiers::ALT,
                vec![Command::InsertToc],
            ),
            (
                Key::C,
                Modifiers::COMMAND | Modifiers::SHIFT,
                vec![Command::FormatCodeBlock],
            ),
        ] {
            let ctx = egui::Context::default();
            let output = ctx.run_ui(
                RawInput {
                    events: vec![key_event(key, modifiers)],
                    ..Default::default()
                },
                |ui| {
                    assert_eq!(
                        poll_shortcuts(ui.ctx(), &Keymap::builtin()),
                        expected,
                        "{modifiers:?}+C 的触发面"
                    );
                },
            );
            output.drop_without_applying_deltas();
        }
        assert_eq!(Command::InsertToc.message(), Message::InsertToc);
    }

    /// 打字机模式(#64 M1)默认键位:Ctrl/Cmd+Alt+W,出厂表无第二个占用者
    /// (#9 口径的撞键核查);与 Ctrl/Cmd+W(关标签)只差一个 Alt,靠
    /// 「修饰键个数降序」的消费顺序共存(与 EmojiPicker/ExportHtml 同款
    /// —— Shift 组合先被问到,这里 Alt 组合同理)。Alt 层纯字母(F/E/O/
    /// V/X/A/S/T)是菜单标题助记与主题键(#119/#45 K1)的命名空间,W 在
    /// 那一层空闲(避让口径见 decisions-pending #121)。真按键只触发这一
    /// 条,消息映射到无参翻转消息。
    #[test]
    fn typewriter_shortcut_is_ctrl_alt_w_and_conflict_free() {
        let ctrl_alt_w = crate::keymap::Shortcut {
            modifiers: Modifiers::COMMAND | Modifiers::ALT,
            key: Key::W,
        };
        assert_eq!(
            Command::TypewriterToggle
                .default_shortcut()
                .map(|shortcut| {
                    crate::keymap::Shortcut {
                        modifiers: shortcut.modifiers,
                        key: shortcut.logical_key,
                    }
                }),
            Some(ctrl_alt_w),
            "TypewriterToggle 出厂默认 = Ctrl/Cmd+Alt+W"
        );
        assert_eq!(
            Keymap::builtin().get(Command::TypewriterToggle),
            Some(ctrl_alt_w)
        );
        assert_eq!(
            Keymap::builtin().conflict(Command::TypewriterToggle, ctrl_alt_w),
            None,
            "Ctrl/Cmd+Alt+W 不该撞任何出厂键位"
        );
        assert_eq!(
            Command::TypewriterToggle.group(),
            CommandGroup::View,
            "打字机是视图命令(菜单「视图」/蒙层同一归属)"
        );

        // Alt 组合先被问到,Ctrl+W 不得误吞
        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            RawInput {
                events: vec![key_event(Key::W, Modifiers::COMMAND | Modifiers::ALT)],
                ..Default::default()
            },
            |ui| {
                assert_eq!(
                    poll_shortcuts(ui.ctx(), &Keymap::builtin()),
                    vec![Command::TypewriterToggle]
                );
            },
        );
        output.drop_without_applying_deltas();
        assert_eq!(
            Command::TypewriterToggle.message(),
            Message::ToggleTypewriter
        );
    }

    /// 专注模式(#64 M2)默认键位:Ctrl/Cmd+Alt+D,出厂表 Ctrl/Cmd+Alt
    /// 层 D 无第二个占用者(#9 口径的撞键核查);含 Alt 的绑定按保守口径
    /// 避开菜单标题助记集(F/E/O/V/X/A/S),D 不在其中 —— 首选 F 因
    /// 「文件」标题撞键被 menubar 标题审计否决(取舍见 decisions-pending
    /// #122)。三修饰组合按「修饰键个数降序」先被问到,与 Ctrl/Cmd+D
    /// (复制选中)只差一个 Alt,两者共存(与打字机之于关标签同款)。
    /// 真按键只触发这一条,消息映射到无参翻转消息。
    #[test]
    fn focus_mode_shortcut_is_ctrl_alt_d_and_conflict_free() {
        let ctrl_alt_d = crate::keymap::Shortcut {
            modifiers: Modifiers::COMMAND | Modifiers::ALT,
            key: Key::D,
        };
        assert_eq!(
            Command::FocusModeToggle.default_shortcut().map(|shortcut| {
                crate::keymap::Shortcut {
                    modifiers: shortcut.modifiers,
                    key: shortcut.logical_key,
                }
            }),
            Some(ctrl_alt_d),
            "FocusModeToggle 出厂默认 = Ctrl/Cmd+Alt+D"
        );
        assert_eq!(
            Keymap::builtin().get(Command::FocusModeToggle),
            Some(ctrl_alt_d)
        );
        assert_eq!(
            Keymap::builtin().conflict(Command::FocusModeToggle, ctrl_alt_d),
            None,
            "Ctrl/Cmd+Alt+D 不该撞任何出厂键位"
        );
        assert_eq!(
            Command::FocusModeToggle.group(),
            CommandGroup::View,
            "专注模式是视图命令(菜单「视图」/蒙层同一归属)"
        );

        // Alt 组合先被问到;同帧只触发本命令,不连带复制选中的 Ctrl/Cmd+D
        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            RawInput {
                events: vec![key_event(Key::D, Modifiers::COMMAND | Modifiers::ALT)],
                ..Default::default()
            },
            |ui| {
                assert_eq!(
                    poll_shortcuts(ui.ctx(), &Keymap::builtin()),
                    vec![Command::FocusModeToggle]
                );
            },
        );
        output.drop_without_applying_deltas();
        assert_eq!(Command::FocusModeToggle.message(), Message::ToggleFocusMode);
    }

    /// Minimap 开关(#67 M2)默认键位:Ctrl/Cmd+Alt+M,出厂表 Ctrl/Cmd+Alt
    /// 层 M 无第二个占用者(#9 口径的撞键核查);含 Alt 的绑定按保守口径
    /// 避开菜单标题助记集(F/E/O/V/X/A/S),M 不在其中(#121/#122/#127
    /// 同款避让,menubar 的标题审计测试共同钉住)。三修饰组合按「修饰键
    /// 个数降序」先被问到,与任何无 Alt 的 M 键组合(出厂表无)互不抢。
    /// 真按键只触发这一条,消息映射到无参翻转消息,分组归「视图」。
    #[test]
    fn minimap_shortcut_is_ctrl_alt_m_and_conflict_free() {
        let ctrl_alt_m = crate::keymap::Shortcut {
            modifiers: Modifiers::COMMAND | Modifiers::ALT,
            key: Key::M,
        };
        assert_eq!(
            Command::ToggleMinimap.default_shortcut().map(|shortcut| {
                crate::keymap::Shortcut {
                    modifiers: shortcut.modifiers,
                    key: shortcut.logical_key,
                }
            }),
            Some(ctrl_alt_m),
            "ToggleMinimap 出厂默认 = Ctrl/Cmd+Alt+M"
        );
        assert_eq!(
            Keymap::builtin().get(Command::ToggleMinimap),
            Some(ctrl_alt_m)
        );
        assert_eq!(
            Keymap::builtin().conflict(Command::ToggleMinimap, ctrl_alt_m),
            None,
            "Ctrl/Cmd+Alt+M 不该撞任何出厂键位"
        );
        assert_eq!(
            Command::ToggleMinimap.group(),
            CommandGroup::View,
            "Minimap 开关是视图命令(菜单「视图」/蒙层同一归属)"
        );

        // Alt 组合先被问到;同帧只触发本命令
        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            RawInput {
                events: vec![key_event(Key::M, Modifiers::COMMAND | Modifiers::ALT)],
                ..Default::default()
            },
            |ui| {
                assert_eq!(
                    poll_shortcuts(ui.ctx(), &Keymap::builtin()),
                    vec![Command::ToggleMinimap]
                );
            },
        );
        output.drop_without_applying_deltas();
        assert_eq!(Command::ToggleMinimap.message(), Message::ToggleMinimap);
    }

    /// 键位改排(#45 K1,preview-typography §3.2 定案):主题出厂键 = Alt+T,
    /// 不撞任何出厂键位;Cmd/Ctrl+Shift+T 的占用者 == TabRestore(#45 K2
    /// 落地后的联动断言,浏览器「恢复关闭标签」同款),且不与任何其他
    /// 出厂键位冲突。
    #[test]
    fn theme_is_alt_t_and_restore_slot_is_free() {
        let alt_t = crate::keymap::Shortcut {
            modifiers: Modifiers::ALT,
            key: Key::T,
        };
        assert_eq!(
            Command::ToggleTheme.default_shortcut().map(|shortcut| {
                crate::keymap::Shortcut {
                    modifiers: shortcut.modifiers,
                    key: shortcut.logical_key,
                }
            }),
            Some(alt_t),
            "ToggleTheme 出厂默认 = Alt+T"
        );
        assert_eq!(Keymap::builtin().get(Command::ToggleTheme), Some(alt_t));
        // #9 口径的撞键核查:Alt+T 不撞任何出厂键位
        assert_eq!(
            Keymap::builtin().conflict(Command::ToggleTheme, alt_t),
            None,
            "Alt+T 不该撞任何出厂键位"
        );

        let restore_slot = crate::keymap::Shortcut {
            modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
            key: Key::T,
        };
        assert_eq!(
            Keymap::builtin().get(Command::TabRestore),
            Some(restore_slot),
            "Cmd/Ctrl+Shift+T 的占用者 = TabRestore(#45 K2)"
        );
        assert_eq!(
            Keymap::builtin().conflict(Command::TabRestore, restore_slot),
            None,
            "Cmd/Ctrl+Shift+T 不该撞任何其他出厂键位"
        );
    }

    /// #45 K2 默认键位:Cmd/Ctrl+Shift+T 真按下只触发 TabRestore 一条
    /// (K1 已把主题挪到 Alt+T,该键位无第二个消费者);消息映射到
    /// `Message::TabRestore`。
    #[test]
    fn restore_shortcut_fires_only_tab_restore() {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            RawInput {
                events: vec![key_event(Key::T, Modifiers::COMMAND | Modifiers::SHIFT)],
                ..Default::default()
            },
            |ui| {
                assert_eq!(
                    poll_shortcuts(ui.ctx(), &Keymap::builtin()),
                    vec![Command::TabRestore]
                );
            },
        );
        output.drop_without_applying_deltas();
        assert_eq!(Command::TabRestore.message(), Message::TabRestore);
    }

    /// Alt+T 真按键只触发主题切换:`matches_logically` 对「显式不要
    /// Ctrl/Cmd」的组合要求事件确实没按 Ctrl/Cmd,故 Cmd/Ctrl+Alt+T 不会
    /// 误中;Alt 层的其它命令(打字机 W / 专注 D)不同键,互不抢。
    #[test]
    fn alt_t_fires_only_toggle_theme() {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            RawInput {
                events: vec![key_event(Key::T, Modifiers::ALT)],
                ..Default::default()
            },
            |ui| {
                assert_eq!(
                    poll_shortcuts(ui.ctx(), &Keymap::builtin()),
                    vec![Command::ToggleTheme]
                );
            },
        );
        output.drop_without_applying_deltas();
    }

    /// 快速打开(#24 C2)默认键位:Cmd/Ctrl+P,出厂表无第二个占用者
    /// (撞键拒绝口径,decisions-pending.md 现状登记在 #22「界面打磨
    /// 批次」;本文件既有测试注释引作 #9,与该文件现状不符)。真按键
    /// 只触发这一条,消息映射到 `Message::ToggleQuickOpen`。
    #[test]
    fn quick_open_is_ctrl_p_and_conflict_free() {
        let ctrl_p = crate::keymap::Shortcut {
            modifiers: Modifiers::COMMAND,
            key: Key::P,
        };
        assert_eq!(
            Command::QuickOpen.default_shortcut().map(|shortcut| {
                crate::keymap::Shortcut {
                    modifiers: shortcut.modifiers,
                    key: shortcut.logical_key,
                }
            }),
            Some(ctrl_p),
            "QuickOpen 出厂默认 = Cmd/Ctrl+P"
        );
        assert_eq!(Keymap::builtin().get(Command::QuickOpen), Some(ctrl_p));
        assert_eq!(
            Keymap::builtin().conflict(Command::QuickOpen, ctrl_p),
            None,
            "Cmd/Ctrl+P 不该撞任何出厂键位"
        );

        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            RawInput {
                events: vec![key_event(Key::P, Modifiers::COMMAND)],
                ..Default::default()
            },
            |ui| {
                assert_eq!(
                    poll_shortcuts(ui.ctx(), &Keymap::builtin()),
                    vec![Command::QuickOpen]
                );
            },
        );
        output.drop_without_applying_deltas();
        assert_eq!(Command::QuickOpen.message(), Message::ToggleQuickOpen);
    }

    /// ALL 数组与命令集同步:新命令忘了进 ALL 的话,快捷键派发、设置页
    /// 遍历与 keymap 存档都会漏掉它。长度与无重复钉在这里,加命令时随
    /// 实现更新。
    #[test]
    fn all_commands_listed_exactly_once() {
        assert_eq!(Command::ALL.len(), 44);
        let mut ids: Vec<_> = Command::ALL.iter().map(|cmd| cmd.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Command::ALL.len(), "ALL 里不得有重复命令");
    }

    /// 分组维度覆盖全集:每条命令恰好归入一个组(#54 M2 蒙层的「单一
    /// 事实源」断言基础——蒙层不另抄清单,分组从本表派生)。
    #[test]
    fn every_command_has_a_group() {
        for cmd in Command::ALL {
            let group = cmd.group();
            assert!(
                CommandGroup::ALL.contains(&group),
                "{cmd:?} 的分组 {group:?} 不在展示序里"
            );
        }
        // 兜底组当前无人认领:有命令落进去说明新命令忘了归类(落兜底可以,
        // 但必须是显式的决定——出现时更新本断言)
        let others: Vec<_> = Command::ALL
            .iter()
            .filter(|cmd| cmd.group() == CommandGroup::Other)
            .collect();
        assert!(others.is_empty(), "未归类的命令:{others:?}");
    }

    /// 否决线自证(#54 M2):出厂表里**每一条**绑定,真按键都只触发它所
    /// 属的命令一次——蒙层接线前后跑同一测试,逐项不变。无绑定的命令
    /// (AI 三条 + PDF/分割线/表格)不参与(无键可按)。
    #[test]
    fn every_bound_command_fires_from_its_own_binding() {
        let keymap = Keymap::builtin();
        for cmd in Command::ALL {
            let Some(shortcut) = keymap.get(cmd) else {
                continue;
            };
            let ctx = egui::Context::default();
            let output = ctx.run_ui(
                RawInput {
                    events: vec![key_event(shortcut.key, shortcut.modifiers)],
                    ..Default::default()
                },
                |ui| {
                    assert_eq!(
                        poll_shortcuts(ui.ctx(), &keymap),
                        vec![cmd],
                        "{} 的绑定 {} 应只触发它自己",
                        cmd.id(),
                        shortcut.platform_text()
                    );
                },
            );
            output.drop_without_applying_deltas();
        }
    }

    /// **命令图标的共享集合必须恰好等于白名单**(2026-10-08 症状 B 守门)。
    ///
    /// 起因:左栏顶段是 6 枚**纯图标无文字**并排,而 `ExportHtml` 与
    /// `ExportPdf` **共用 `Icon::Export`** —— 相邻两枚长得一模一样,tooltip
    /// 是唯一区分手段,实际不可辨识。已拆开(HTML=下载/托盘,PDF=打印/出纸)。
    ///
    /// **为什么是「恰好等于」而不是「不许共享」**:共享本身不是错。
    /// `Icon::Zen` 被 5 个视图开关共用、`Icon::Search` 被 3 个查找类命令
    /// 共用 —— 它们**只出现在带文字标签的菜单里**,共用无害甚至加强一致性。
    /// 真正有害的是**纯图标区里的相邻共用**。
    ///
    /// 所以本测试把「有意共享」列成**显式白名单**:将来新增命令若意外复用了
    /// 某个图标,实际共享集合就多出一项 → 变红 → 逼作者登记「为什么这次共用
    /// 无害」或换图标。比逐个 case 断言图标名更耐改(命令增删不会误红),
    /// 又比不管强(新意外共用必红)。
    ///
    /// 比对用**命令 id 分组**而非 `Icon` 本身:`Icon` 未实现 `Ord`,排序不可用;
    /// 而命令 id 是稳定字符串,分组后排序即可。
    #[test]
    fn intentional_icon_sharing_is_the_whole_set() {
        use std::collections::BTreeMap;

        // 图标 → 共享它的命令 id
        let mut by_icon: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
        for cmd in Command::ALL {
            by_icon
                .entry(icon_key(cmd.icon()))
                .or_default()
                .push(cmd.id());
        }
        let mut actual: Vec<Vec<&'static str>> = by_icon
            .into_values()
            .filter(|cmds| cmds.len() > 1)
            .collect();
        actual.sort_unstable();

        // 有意共享的分组。**新增共享必须先在这里登记**,否则本测试红。
        //
        // ⚠️ 这份白名单是**实测抄下来的**,不是推出来的 —— 首版凭印象写,
        // 立刻被本测试打回(漏了 `Icon::Table` 的 4 个格式命令、
        // `Icon::Search` 的 3 个查找命令,id 也猜错两处)。共享面比想象大,
        // 但**全部只出现在带文字标签的菜单里**,故无害(见上方理由)。
        let mut expected: Vec<Vec<&'static str>> = vec![
            // AI 三命令同属菜单「AI」一段,共用便于识别该段归属
            vec!["ai_mock_stream", "ai_commit_message", "ai_summary"],
            // 七个格式命令共用一个图标;其中 B/I/S/H1-H3 在格式工具条里
            // 走**富文本字形**(`format_bar::button` 的 rich 分支),不碰图标,
            // 故工具条上从不出现「七枚一样」的场面
            vec![
                "format_bold",
                "format_italic",
                "format_strike",
                "format_h1",
                "format_h2",
                "format_h3",
                "format_table",
            ],
            // 六个查找/复制类命令共用放大镜,均在带文字的菜单与快速打开里
            vec![
                "quick_open",
                "duplicate_selection",
                "duplicate_line",
                "find_in_doc",
                "replace_in_doc",
                "goto_line",
            ],
            // 三个标签动作共用,只在标签条(× close 是自绘)与菜单里
            vec!["tab_next", "tab_close", "tab_restore"],
            // 五个视图开关共用,只在带文字的「视图」菜单与快速打开里出现
            vec![
                "toggle_live_preview",
                "toggle_zen",
                "toggle_typewriter",
                "toggle_focus_mode",
                "toggle_minimap",
            ],
        ];
        expected.sort_unstable();

        assert_eq!(
            actual, expected,
            "命令图标的共享集合变了(见上面 actual/expected)。\
             若是新的**有意**共享(只出现在带文字的菜单里),请把它的命令 id \
             分组加进 expected 并注明理由;\
             若是**意外**复用 —— 尤其当它会落到纯图标区(左栏顶段/标题栏)造成\
             相邻两枚看起来一样 —— 请给该命令换一个专属图标。"
        );
    }

    /// 图标的分组键:用**变体名**(`Debug`)而非整数判别式。
    ///
    /// 两个理由:① `Icon` 未实现 `Ord`,`BTreeMap` 排不了;② 整数判别式
    /// 跨版本不保证稳定(`std::mem::discriminant` 文档明说),而
    /// `transmute` 到 `u64` 在枚举大小 < 8 时是 UB —— 变体名既稳定又安全。
    /// 键只用于**同一进程内**给哈希表定序,真正的比对在上面的命令 id 分组。
    fn icon_key(icon: crate::ui::icons::Icon) -> String {
        format!("{icon:?}")
    }
}
