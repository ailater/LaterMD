//! 菜单系统:全部命令的可发现性入口(docs/roadmap.md P0「快捷键」;
//! 全量覆盖矩阵与豁免口径见 docs/menu-coverage.md)。
//!
//! **2026-10-12 改版(汉堡 + 标题栏内联标签行,坤哥指定)**:原顶部常驻
//! 菜单行(`文件(F) 编辑(E) …` 一整条 TopPanel)退役,改为标题栏左缘一枚
//! 汉堡按钮([`Icon::Hamburger`]);点击后**在标题栏条内**展开菜单组名
//! 标签行(文件 / 编辑 / … / 帮助,不弹浮层),点组名才在其下方下拉该组
//! 条目。双平台一致(macOS 的系统级 NSMenu 是平台惯例 + 标准编辑键的
//! 载体,原样保留,见 `mac_menu`);常驻行撤掉后编辑区上沿净高 +26px。
//!
//! 菜单结构仍是**常量清单**([`MENUS`]:标题 + 助记字母 + 分组段):绘制
//! 与覆盖测试共用同一事实源,新命令忘进菜单时 `every_command_has_a_menu_
//! entry` 红。分组规范:同类聚组、段与段之间分隔线、常用在前;顺序
//! 文件 → 编辑 → 格式 → 视图 → 导出 → AI → 设置 → 帮助(高频编辑动作
//! 在前,派生动作在后,帮助居末位是桌面惯例)。键位文本取自 `keymap`
//! (用户可改,与工具栏按钮 tooltip 的 decisions-pending #32 同源先例
//! 一致)—— 改键后两处同时变。
//!
//! #62 M2 的助记键在汉堡形态下的口径:egui 0.36.2 **不支持** `&X` 助记
//! 语法(egui/epaint 全 crate `mnemonic` 零命中,`Button`/`MenuButton`
//! 文本路径不解析 `&`,核验记录见 decisions-pending #119),故在菜单层
//! 自建轻量助记:
//! * 标题层:Alt+字母 打开标题栏标签行并落到对应菜单组(`Popup::open_id`
//!   驱动 egui 公开的 popup 记忆,严格匹配「仅 Alt」——Ctrl+Alt+R 等
//!   组合不误触);
//! * 条目层:标签行展开时裸字母触发当前组的对应条目并收起(Windows 惯例);
//!   单独按下或释放 Alt 不触发菜单，只有明确的 Alt+字母组合才打开。
//!
//! 助记字母与 keymap 的冲突审计由 `title_mnemonics_unique_and_clear_of_
//! keymap_alt_bindings` 遍历断言零冲突;macOS 无 Alt 助记惯例,整个
//! 助记层(显示后缀 + 键盘触发)在 mac 编译目标下关闭。
//!
//! 点击只产出 [`Message`],执行在 `App::logic` 归约 —— 与快捷键入口
//! (`crate::command::poll_shortcuts`)殊途同归。宿主在 `ui::titlebar`
//! (自绘标题栏左缘)。

use crate::command::Command;
use crate::keymap::Keymap;
use crate::live::RenderMode;
use crate::settings::SettingsTab;
use crate::state::{Message, State};
use crate::ui::icons::Icon;
use crate::ui::tokens;

use eframe::egui::{self, Sense};

/// 「文件」两段:文件动作(常用序:新建 → 打开 → 快速打开 → 保存 →
/// 另存为)+ 标签命令挂尾部(编辑器惯例:文件 → 关闭标签页),与
/// Ctrl+Tab / Ctrl+W / Ctrl+Shift+T 的快捷键入口互为可发现性。
const FILE_MENU: [&[Command]; 2] = [
    &[
        Command::New,
        Command::Open,
        Command::QuickOpen,
        Command::Save,
        Command::SaveAs,
    ],
    &[Command::TabNext, Command::TabClose, Command::TabRestore],
];

/// 「编辑」一段:文档内动作(undo/redo 是 TextEdit 内建,不列)。插入
/// 目录(#66 M2)同段:与复制行/查找/替换/跳转同族的不作用选区的文档级
/// 辅助动作(归位口径 decisions-pending #127)。
const EDIT_MENU: [&[Command]; 1] = [&[
    Command::DuplicateSelection,
    Command::DuplicateLine,
    Command::FindInDoc,
    Command::ReplaceInDoc,
    Command::GotoLine,
    Command::InsertToc,
]];

/// 「格式」五段:行内 / 标题 / 块 / 列表四段与格式工具条
/// (`ui::format_bar` 的 `FormatGroup`)同一口径;末尾「插入」段收两个
/// 对话框类动作(图片要 alt+url、Emoji 要点选,点了不改文档,与上面
/// 十五条直接改文档的格式动作隔开)。
const FORMAT_MENU: [&[Command]; 5] = [
    &[
        Command::FormatBold,
        Command::FormatItalic,
        Command::FormatStrike,
        Command::FormatInlineCode,
        Command::FormatLink,
    ],
    &[Command::FormatH1, Command::FormatH2, Command::FormatH3],
    &[
        Command::FormatQuote,
        Command::FormatCodeBlock,
        Command::FormatDivider,
        Command::FormatTable,
    ],
    &[
        Command::FormatBullet,
        Command::FormatOrdered,
        Command::FormatTask,
    ],
    &[Command::ImageInsert, Command::EmojiPicker],
];

/// 「视图」两段:布局/外观开关(打字机 #64 与专注模式 #64 M2、Live 同段,
/// 同为编辑区呈现方式开关;minimap #67 M2 归此段「专注」之后「主题」
/// 之前 —— 编辑区呈现方式开关聚在主题互换之前,位置取舍见
/// decisions-pending #129);禅定模式是整套面板组合(不是普通开关),
/// 独立一段隔开。
const VIEW_MENU: [&[Command]; 2] = [
    &[
        Command::ToggleSidebar,
        Command::ToggleLivePreview,
        Command::TypewriterToggle,
        Command::FocusModeToggle,
        Command::ToggleMinimap,
        Command::ToggleTheme,
    ],
    &[Command::ToggleZen],
];

/// 「导出」一段:两者都是派生物,不触碰文档落盘身份。
const EXPORT_MENU: [&[Command]; 1] = [&[Command::ExportHtml, Command::ExportPdf]];

/// 「AI」一段。
const AI_MENU: [&[Command]; 1] = [&[
    Command::AiMockStream,
    Command::AiCommitMessage,
    Command::AiSummary,
]];

/// 菜单清单:数组顺序即菜单栏从左到右的显示顺序,段内顺序即条目顺序;
/// 第二项是该菜单标题的助记字母(Alt 命名空间,**全局唯一**,与 keymap
/// 全部 Alt 类绑定的冲突审计见模块测试)。「设置」「帮助」不是命令
/// (直达设置页 / 关于窗,照「设置」先例单独绘制),不进本表。
pub(crate) const MENUS: [(&str, char, &[&[Command]]); 6] = [
    ("文件", 'F', &FILE_MENU),
    ("编辑", 'E', &EDIT_MENU),
    ("格式", 'O', &FORMAT_MENU),
    ("视图", 'V', &VIEW_MENU),
    ("导出", 'X', &EXPORT_MENU),
    ("AI", 'A', &AI_MENU),
];

/// 「设置」菜单的标题助记字母(与 [`MENUS`] 同一 Alt 命名空间)。
const SETTINGS_MNEMONIC: char = 'S';

/// 「帮助」菜单(#71 M1)的标题助记字母(H = Help 词首;与 [`MENUS`]
/// 的 F/E/O/V/X/A/S 及 `SETTINGS_MNEMONIC` 在 Alt 命名空间互不撞,keymap
/// 出厂 Alt 系七条也没有 Alt+H —— 冲突审计见
/// `title_mnemonics_unique_and_clear_of_keymap_alt_bindings`)。
const HELP_MNEMONIC: char = 'H';

/// 「帮助」菜单条目「关于 LaterMD…」的条目助记(A = About 词首;帮助
/// 菜单内唯一)。
const ABOUT_MNEMONIC: char = 'A';

/// 「设置」菜单两个直达页的条目助记(该菜单内唯一)。
const SETTINGS_TAB_MNEMONICS: [(SettingsTab, char); 2] =
    [(SettingsTab::Appearance, 'A'), (SettingsTab::Keymap, 'K')];

/// 菜单条目的助记字母(**菜单内**唯一;菜单展开时裸字母触发)。字母是
/// Windows 惯例位(中文菜单取英义词首/次字母),数字 1/2/3 给三档标题
/// (与快捷键 Ctrl+1/2/3 同族)。与 keymap 的关系:keymap 出厂表无任何
/// 裸字母绑定(`Shortcut::bindable` 拒绝),两个命名空间零交集 —— 冲突
/// 面只存在于标题层(Alt+字母),那层由 `MENUS` 表单独审计。
fn command_mnemonic(cmd: Command) -> char {
    match cmd {
        Command::New => 'N',
        Command::Open => 'O',
        Command::QuickOpen => 'Q',
        Command::Save => 'S',
        Command::SaveAs => 'A',
        Command::TabNext => 'T',
        Command::TabClose => 'C',
        Command::TabRestore => 'R',
        Command::DuplicateSelection => 'D',
        Command::DuplicateLine => 'L',
        Command::FindInDoc => 'F',
        Command::ReplaceInDoc => 'H',
        Command::GotoLine => 'G',
        // T = TOC 词首;编辑菜单内 D/L/F/H/G 均未占 T,菜单内唯一
        Command::InsertToc => 'T',
        Command::FormatBold => 'B',
        Command::FormatItalic => 'I',
        Command::FormatStrike => 'K',
        Command::FormatInlineCode => 'C',
        Command::FormatLink => 'L',
        Command::FormatH1 => '1',
        Command::FormatH2 => '2',
        Command::FormatH3 => '3',
        Command::FormatQuote => 'Q',
        Command::FormatCodeBlock => 'E',
        Command::FormatDivider => 'D',
        Command::FormatTable => 'T',
        Command::FormatBullet => 'U',
        Command::FormatOrdered => 'O',
        Command::FormatTask => 'W',
        Command::ImageInsert => 'P',
        Command::EmojiPicker => 'M',
        Command::ToggleSidebar => 'S',
        Command::ToggleLivePreview => 'V',
        // W = typewriter 的词尾;视图菜单内 S/V/T 已被占用,菜单内唯一
        // (条目层助记是菜单内命名空间,与全局 Alt 层无关)
        Command::TypewriterToggle => 'W',
        // F = Focus 的词首;视图菜单内 S/V/W/T/Z 均未占 F,菜单内唯一
        Command::FocusModeToggle => 'F',
        // M = Minimap 的词首;视图菜单内 S/V/W/F/T/Z 均未占 M,菜单内唯一
        Command::ToggleMinimap => 'M',
        Command::ToggleTheme => 'T',
        Command::ToggleZen => 'Z',
        Command::ExportHtml => 'H',
        Command::ExportPdf => 'P',
        Command::AiMockStream => 'M',
        Command::AiCommitMessage => 'C',
        Command::AiSummary => 'S',
    }
}

/// 标题栏内标签行的助记后缀与正文**同字号**:无任何行高/对齐覆写时,
/// egui 行内混排的 Inter 基线只比回退 CJK 基线高 1px(无头 galley 探针
/// 实测),拉丁字母的墨高本来就矮于汉字 —— 同字号恰好落在中文软件菜单
/// 「文件(F)」的标准形态上。缩号 + 行高补偿都试过:Inter 与 CJK 行盒
/// metrics 不同,egui 的 BOTTOM 底对齐≠基线对齐,缩号反而把后缀吊离
/// 汉字底线(2026-10-12 坤哥指出「英文明显高」);line_height 补偿又
/// 会连带字形栅格尺寸,一并放弃,回到无覆写同字号。mac 不显示后缀。
const MNEMONIC_SCALE: f32 = 1.0;

/// 显示名 + 助记后缀:Win/Linux 显示「文件(F)」形态(mac 不显示)。
/// egui 的下划线只能作用于整段 `RichText`,无法只给单个字母画线
/// (epaint 无部分字形装饰),故用括号后缀 —— 中文 Windows 软件的标准
/// 形态。`ui.style()` 决定正文字号 —— 菜单条与条目按钮同吃 Button
/// 样式,两侧自然一致。
fn mnemonic_label(ui: &egui::Ui, label: &str, letter: char) -> egui::text::LayoutJob {
    use egui::text::{TextFormat, TextWrapping};
    let base = egui::TextStyle::Button.resolve(ui.style());
    let color = ui.visuals().text_color();
    let mut job = egui::text::LayoutJob {
        wrap: TextWrapping::no_max_width(),
        ..egui::text::LayoutJob::default()
    };
    job.append(label, 0.0, TextFormat::simple(base.clone(), color));
    if !cfg!(target_os = "macos") {
        let suffix = format!("({letter})");
        let small = egui::FontId::new(base.size * MNEMONIC_SCALE, base.family.clone());
        job.append(&suffix, 0.0, TextFormat::simple(small, color));
    }
    job
}

/// 布偶开关命令的当前值(菜单条目勾选态;#67 M2):`Some(true/false)` =
/// 开/关,`None` = 非开关类命令(菜单条目不带勾选列)。
///
/// 口径:只覆盖**有布尔语义**的开关 —— 视图菜单的侧边栏/Live/
/// 打字机/专注/minimap/禅定;「切换主题」是明暗互换而非开/关(哪个算
/// 「开」没有自然答案),不加勾选态。真值从 [`State`] 单一事实源取,
/// 与设置页/标题栏按钮同源(取舍见 decisions-pending #129)。
pub(crate) fn toggle_checked(cmd: Command, state: &State) -> Option<bool> {
    Some(match cmd {
        Command::ToggleSidebar => state.layout.left,
        Command::ToggleLivePreview => state.render_mode == RenderMode::Live,
        Command::TypewriterToggle => state.theme.show_typewriter,
        Command::FocusModeToggle => state.theme.show_focus_mode,
        Command::ToggleMinimap => state.theme.show_minimap,
        Command::ToggleZen => state.layout.zen,
        _ => return None,
    })
}

/// 勾选前缀:开 = `✓ `(U+2713 + 空格),关 = 两个半角空格占位(开关条目
/// 间的文字主体对齐;✓ 与两空格的字宽随字体略有出入,观感留真机核对)。
/// `None` = 非开关条目,无前缀。
fn check_prefix(checked: Option<bool>) -> &'static str {
    match checked {
        Some(true) => "✓ ",
        Some(false) => "  ",
        None => "",
    }
}

/// egui `Key` → 助记字母。`Key::name` 对字母/数字键恰好返回单字符
/// ("A".."Z"、"0".."9"),其余键名多字符,自然被过滤。
fn key_letter(key: egui::Key) -> Option<char> {
    let name = key.name();
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(single), None) => Some(single.to_ascii_uppercase()),
        _ => None,
    }
}

/// 本帧「仅 Alt + 字母」按键(标题层助记的触发面)。严格匹配:`matches_logically`
/// 会忽略事件多余的 Alt(egui 的宽松语义),而助记必须**只在**纯 Alt 时触发,
/// 否则 Ctrl+Alt+R(切换预览栏)会同时炸开菜单 —— 这里用位相等收死。
fn alt_menu_letter(ctx: &egui::Context) -> Option<char> {
    if cfg!(target_os = "macos") {
        return None;
    }
    ctx.input(|input| {
        input.events.iter().find_map(|event| match event {
            egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } if *modifiers == egui::Modifiers::ALT => key_letter(*key),
            _ => None,
        })
    })
}

/// 本帧裸字母按键(条目层助记的触发面;菜单展开时才被 [`fire_item_letter`]
/// 消费,平时裸字母归编辑器输入)。
fn bare_key_letter(ctx: &egui::Context) -> Option<char> {
    if cfg!(target_os = "macos") {
        return None;
    }
    ctx.input(|input| {
        input.events.iter().find_map(|event| match event {
            egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } if modifiers.is_none() => key_letter(*key),
            _ => None,
        })
    })
}

/// 消费触发助记的那次按键(Key 事件连同同字母的 Text 事件):不消费的话,
/// 部分平台上 Alt+字母会带 `Event::Text` 流进聚焦的 TextEdit 插出字符
/// (egui-winit 的 `is_cmd` 不含 Alt)。
fn consume_letter_events(ctx: &egui::Context, letter: char) {
    let upper = letter.to_ascii_uppercase();
    let lower = letter.to_ascii_lowercase();
    ctx.input_mut(|input| {
        input.events.retain(|event| match event {
            egui::Event::Key { key, .. } => key_letter(*key).is_none_or(|ch| ch != upper),
            egui::Event::Text(text) => {
                let text = text.as_str();
                text.is_empty() || !text.chars().all(|ch| ch == lower || ch == upper)
            }
            _ => true,
        });
    });
}

/// 菜单展开帧的条目层助记:本帧裸字母命中本菜单某条目时,发消息并收起
/// 菜单。只在命中的菜单/条目上消费事件 —— 未命中字母原样放行(菜单开着
/// 时编辑器照常收字的现状不被扩大)。
fn fire_item_letter(ui: &egui::Ui, sections: &[&[Command]], outbox: &mut Vec<Message>) -> bool {
    let Some(letter) = bare_key_letter(ui.ctx()) else {
        return false;
    };
    for cmd in sections.iter().flat_map(|section| section.iter().copied()) {
        if command_mnemonic(cmd) == letter {
            consume_letter_events(ui.ctx(), letter);
            outbox.push(cmd.message());
            ui.close();
            return true;
        }
    }
    false
}

/// 「设置」菜单展开帧的条目层助记(与 [`fire_item_letter`] 同口径,目标
/// 是设置页直达而非命令)。
fn fire_settings_item_letter(ui: &egui::Ui, outbox: &mut Vec<Message>) -> bool {
    let Some(letter) = bare_key_letter(ui.ctx()) else {
        return false;
    };
    for (tab, mnemonic) in SETTINGS_TAB_MNEMONICS {
        if mnemonic == letter {
            consume_letter_events(ui.ctx(), letter);
            outbox.push(Message::SettingsOpened(tab));
            ui.close();
            return true;
        }
    }
    false
}

/// 「帮助」菜单展开帧的条目层助记(与 [`fire_settings_item_letter`] 同
/// 口径,目标是关于窗而非命令;#71 M1)。
fn fire_help_item_letter(ui: &egui::Ui, outbox: &mut Vec<Message>) -> bool {
    let Some(letter) = bare_key_letter(ui.ctx()) else {
        return false;
    };
    if ABOUT_MNEMONIC == letter {
        consume_letter_events(ui.ctx(), letter);
        outbox.push(Message::AboutOpened);
        ui.close();
        return true;
    }
    false
}

/// 标题栏菜单标签行的开合态(汉堡点击 toggle):纯 UI 会话态挂 egui
/// 临时数据,不进 `State` 存档,重启自然归零。
fn menu_row_open_id() -> egui::Id {
    egui::Id::new("menubar-row-open")
}

pub(crate) fn menu_row_open(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<bool>(menu_row_open_id()).unwrap_or(false))
}

fn set_menu_row_open(ctx: &egui::Context, open: bool) {
    ctx.data_mut(|data| data.insert_temp(menu_row_open_id(), open));
}

/// Alt+字母 命中组的「待弹出」暂存:汉堡层写入,标签行层读走并对该组
/// 执行 `Popup::open_id` —— 组名按钮的 popup id 要到按钮绘制时才存在,
/// 跨层经 temp 交接(一次性,读后即清)。
fn alt_pending_id() -> egui::Id {
    egui::Id::new("menubar-alt-pending")
}

/// 标题栏左缘的汉堡菜单按钮(绝对矩形自绘):点击开合**标题栏内**
/// 的菜单标签行([`menu_row`],坤哥 2026-10-12 指定形态 —— 点汉堡不出
/// 浮层,标签行就地长在标题栏条内);Alt+字母 展开标签行并直接弹出
/// 对应组。单击 Alt 不打开任何东西(误触史与退役理由见模块文档)。
pub(crate) fn hamburger_button_at(ui: &mut egui::Ui, rect: egui::Rect) -> egui::Response {
    // 自绘交互底而非 `egui::Button`:AtomLayout 把 frame 内边距加在
    // min_size 之外(outer = 内容 + margin),`ui.put` 32×24 实际分到
    // 32×32 的命中/绘制矩形,图标中心随之整体下坠 4px、命中区侵入下方
    // 标签行。`allocate_rect` 保证响应矩形与传入矩形严格相等。
    let response = ui.allocate_rect(rect, Sense::click());
    let colors = crate::theme::shell(ui);
    if response.hovered() || menu_row_open(ui.ctx()) {
        ui.painter()
            .rect_filled(rect, crate::ui::tokens::RADIUS_SM, colors.hover);
    }
    paint_hamburger_icon(ui, &response);
    let ctx = ui.ctx();
    if response.clicked() {
        set_menu_row_open(ctx, !menu_row_open(ctx));
    }
    if let Some(letter) = alt_menu_letter(ctx) {
        if let Some(section) = section_index_of(letter) {
            set_menu_row_open(ctx, true);
            ctx.data_mut(|data| data.insert_temp(alt_pending_id(), Some(section)));
            consume_letter_events(ctx, letter);
        }
    }
    response.on_hover_text("菜单")
}

/// 汉堡图标叠画在按钮中心(响应矩形即传入矩形,中心即标题栏中心线)。
/// 图标全集走 painter 自绘(icons.rs 铁律),不吃字体字形。
fn paint_hamburger_icon(ui: &egui::Ui, response: &egui::Response) {
    if !ui.is_rect_visible(response.rect) {
        return;
    }
    let visuals = ui.visuals();
    let color = if response.hovered() || menu_row_open(ui.ctx()) {
        visuals.widgets.hovered.fg_stroke.color
    } else {
        visuals.widgets.inactive.fg_stroke.color
    };
    Icon::Hamburger.draw(ui.painter(), response.rect.center(), 16.0, color);
}

/// 标题栏内的菜单标签行:八个组名按钮(文件…帮助)横排,点击某组即
/// 在其下方弹出条目下拉(egui `menu_button` 内建:点开、再点收、点外
/// 收)。宿主 `ui::titlebar`:展开态下 logo/标题/页签让位,标签行独占
/// 标题栏左中段。
///
/// 收层口径:条目点击(指针或裸字母助记)选中即整行收起(菜单栏惯例:
/// 用完就走);下拉全关后点击行外区域同样收起 —— 与 Windows 菜单栏的
/// 「点外面就收」一致。Esc 只关下拉(egui 内建),标签行保留,再点
/// 汉堡收。
pub(crate) fn menu_row(
    ui: &mut egui::Ui,
    bar: egui::Rect,
    keymap: &Keymap,
    state: &State,
    outbox: &mut Vec<Message>,
    mut probe: Option<impl FnMut(usize, egui::Id, egui::Rect)>,
) {
    let ctx = ui.ctx().clone();
    let menu_rect = crate::ui::titlebar::menu_button_rect(bar);
    // 行起点贴汉堡右缘 4px(2026-10-12 坤哥「左侧文件可以再往左点」;
    // 原为 SPACE_SM=6,叠上标签内边距后首个「文件」距汉堡图标过远)。
    let row_rect = egui::Rect::from_min_max(
        egui::pos2(menu_rect.right() + tokens::SPACE_XS, bar.top()),
        egui::pos2(bar.right(), bar.bottom()),
    );
    let mut popup_ids: Vec<(usize, egui::Id)> = Vec::new();
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(row_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    row.set_clip_rect(row_rect.intersect(ui.clip_rect()));
    // 标签按钮之间采用紧凑的 4px 节奏，避免标题栏展开后横向过松。
    row.spacing_mut().item_spacing.x = tokens::SPACE_XS;
    // 标签按钮内边距局部收紧(2026-10-12 坤哥「菜单的间距太宽了」):
    // 全局 button_padding 是输入框档(shadcn px-3=12,theme 投影),菜单
    // 标签吃它会把文本间隙撑到 ~22px。菜单栏惯例是标签近贴、hover 底
    // 轻贴文字,仅本行局部改写,输入框/其余按钮不受影响。
    row.style_mut().spacing.button_padding = egui::vec2(tokens::SPACE_XS, 1.0);
    // Alt+字母 的待弹组:读走即清(Option<usize> 存取,读后写回 None)
    let pending = ctx.data_mut(|data| data.get_temp::<Option<usize>>(alt_pending_id()).flatten());
    if pending.is_some() {
        ctx.data_mut(|data| data.insert_temp(alt_pending_id(), None::<usize>));
    }
    for index in 0..section_count() {
        let (title, mnemonic) = section_title(index);
        let response = row
            .menu_button(mnemonic_label(&row, title, mnemonic), |ui| {
                let used = draw_group_content(ui, index, keymap, state, outbox);
                if used {
                    ui.close();
                    set_menu_row_open(ui.ctx(), false);
                }
            })
            .response;
        let popup_id = egui::containers::Popup::default_response_id(&response);
        if let Some(probe) = probe.as_mut() {
            probe(index, popup_id, response.rect);
        }
        popup_ids.push((index, popup_id));
        if pending == Some(index) {
            egui::containers::Popup::open_id(&ctx, popup_id);
        }
    }
    drop(row);

    // 下拉全关 + 指针点击在行外 → 收层。条目点击帧不触发(popup 记忆
    // 在绘制期仍为开态,自动关闭发生在本帧绘制之后,下一帧才见全关)。
    let any_open = popup_ids
        .iter()
        .any(|(_, id)| egui::containers::Popup::is_id_open(ui.ctx(), *id));
    if !any_open
        && ui.input(|i| i.pointer.any_click())
        && ui
            .input(|i| i.pointer.interact_pos())
            .is_none_or(|pos| !row_rect.contains(pos) && !menu_rect.contains(pos))
    {
        set_menu_row_open(ui.ctx(), false);
    }
}

/// 标签行第 `index` 组的下拉内容:条目渲染 + 条目层助记;返回是否有
/// 条目被选中(指针点击或裸字母),供收层判断。
fn draw_group_content(
    ui: &mut egui::Ui,
    index: usize,
    keymap: &Keymap,
    state: &State,
    outbox: &mut Vec<Message>,
) -> bool {
    if index < MENUS.len() {
        let used = draw_sections(ui, MENUS[index].2, keymap, state, outbox);
        fire_item_letter(ui, MENUS[index].2, outbox) || used
    } else if index == MENUS.len() {
        // 设置:菜单保留两个直达页(外观 / 快捷键),完整五页由工具栏齿轮开
        let mut used = false;
        for (tab, mnemonic) in SETTINGS_TAB_MNEMONICS {
            if ui
                .button(mnemonic_label(ui, tab.label(), mnemonic))
                .clicked()
            {
                outbox.push(Message::SettingsOpened(tab));
                used = true;
            }
        }
        fire_settings_item_letter(ui, outbox) || used
    } else {
        // 帮助(#71 M1):非命令菜单,条目直达关于窗
        let mut used = false;
        if ui
            .button(mnemonic_label(ui, "关于 LaterMD…", ABOUT_MNEMONIC))
            .clicked()
        {
            outbox.push(Message::AboutOpened);
            used = true;
        }
        fire_help_item_letter(ui, outbox) || used
    }
}

/// 组总数 = [`MENUS`] 六组 + 设置 + 帮助。
fn section_count() -> usize {
    MENUS.len() + 2
}

/// 组序号 → (标题, 助记)。设置/帮助排在 [`MENUS`] 之后(菜单末位惯例)。
fn section_title(index: usize) -> (&'static str, char) {
    match index {
        i if i < MENUS.len() => (MENUS[i].0, MENUS[i].1),
        i if i == MENUS.len() => ("设置", SETTINGS_MNEMONIC),
        _ => ("帮助", HELP_MNEMONIC),
    }
}

/// 助记字母 → 组序号(Alt+字母 落组;无命中 None,如 Alt+T 主题键)。
fn section_index_of(letter: char) -> Option<usize> {
    (0..section_count()).find(|index| section_title(*index).1 == letter)
}

/// 一份菜单的分组段:段间画分隔线(首段之前不画),段内逐条渲染。
/// 返回是否有条目被点击 —— 弹层是 `CloseOnClickOutside`,点击收层由
/// 调用方据此显式 `ui.close()`。独立成函数是为了让无头测试不经过弹层
/// (未展开时不执行内容)就能渲染全部条目 —— 分组渲染不 panic 的断言面。
fn draw_sections(
    ui: &mut egui::Ui,
    sections: &[&[Command]],
    keymap: &Keymap,
    state: &State,
    outbox: &mut Vec<Message>,
) -> bool {
    let mut clicked_any = false;
    for (index, section) in sections.iter().enumerate() {
        if index > 0 {
            ui.separator();
        }
        for cmd in *section {
            if item(ui, *cmd, keymap, toggle_checked(*cmd, state), outbox).clicked() {
                clicked_any = true;
            }
        }
    }
    clicked_any
}

/// 单个菜单项:显示名(带助记后缀,开关类条目前置勾选列 `checked`)+
/// 当前键位(未绑快捷键的命令只显示名字);点击发消息。返回按钮响应,
/// 独立成函数便于点击测试定位。
pub fn item(
    ui: &mut egui::Ui,
    cmd: Command,
    keymap: &Keymap,
    checked: Option<bool>,
    outbox: &mut Vec<Message>,
) -> egui::Response {
    let label = format!("{}{}", check_prefix(checked), cmd.label());
    let mut button = egui::Button::new(mnemonic_label(ui, &label, command_mnemonic(cmd)));
    if let Some(shortcut) = keymap.get(cmd) {
        button = button.shortcut_text(ui.ctx().format_shortcut(&shortcut.keyboard()));
    }
    let response = ui.add(button);
    if response.clicked() {
        outbox.push(cmd.message());
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandGroup;
    use crate::state::Message;
    use egui::{Event, FullOutput, PointerButton, RawInput, Rect};
    use std::cell::Cell;

    /// 命令分组 → 该组命令应挂的菜单标题。Tab 组挂「文件」尾部是既有口径
    /// (`CommandGroup::Tab` 文档:编辑器惯例,蒙层才独立成组);其余组与
    /// 菜单一一对应 —— 菜单归属与命令注册表同一事实源,蒙层/设置页共用。
    fn expected_menu(group: CommandGroup) -> &'static str {
        match group {
            CommandGroup::File | CommandGroup::Tab => "文件",
            CommandGroup::Edit => "编辑",
            CommandGroup::Format => "格式",
            CommandGroup::View => "视图",
            CommandGroup::Export => "导出",
            CommandGroup::Ai => "AI",
            // every_command_has_a_group(command.rs)已钉住无人落 Other,
            // 这里只做映射完备,不兜底
            CommandGroup::Other => "其他",
        }
    }

    /// 菜单常量里收集到的 (菜单标题, 命令) 全集。
    fn menu_entries() -> Vec<(&'static str, Command)> {
        let mut entries = Vec::new();
        for (title, _, sections) in MENUS {
            for section in sections {
                for cmd in *section {
                    entries.push((title, *cmd));
                }
            }
        }
        entries
    }

    /// 点击菜单项发出的消息就是该命令的归约入口;仅渲染不产生消息。
    #[test]
    fn clicking_item_sends_command_message() {
        let ctx = egui::Context::default();
        let mut outbox = Vec::new();
        let keymap = Keymap::builtin();
        let rect = Cell::new(Rect::NOTHING);

        // 第一帧只渲染,借 Cell 拿到条目的屏幕位置
        ctx.run_ui(RawInput::default(), |ui| {
            rect.set(item(ui, Command::ToggleSidebar, &keymap, None, &mut outbox).rect);
        })
        .drop_without_applying_deltas();
        assert!(outbox.is_empty(), "仅渲染不产生消息");

        // 第二帧在条目中心按下并抬起 → clicked
        let center = rect.get().center();
        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        ctx.run_ui(
            RawInput {
                events: vec![Event::PointerMoved(center), click(true), click(false)],
                ..Default::default()
            },
            |ui| {
                item(ui, Command::ToggleSidebar, &keymap, None, &mut outbox);
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(outbox, vec![Message::SidebarToggled]);
    }

    /// AI 菜单项(无快捷键的命令之一)正常渲染并可点击发起:item() 的
    /// shortcut 分支在 None 时不得触碰 format_shortcut。
    #[test]
    fn ai_item_renders_without_shortcut_and_sends_start() {
        let ctx = egui::Context::default();
        let mut outbox = Vec::new();
        let keymap = Keymap::builtin();
        let rect = Cell::new(Rect::NOTHING);

        ctx.run_ui(RawInput::default(), |ui| {
            rect.set(item(ui, Command::AiMockStream, &keymap, None, &mut outbox).rect);
        })
        .drop_without_applying_deltas();

        let center = rect.get().center();
        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        ctx.run_ui(
            RawInput {
                events: vec![Event::PointerMoved(center), click(true), click(false)],
                ..Default::default()
            },
            |ui| {
                item(ui, Command::AiMockStream, &keymap, None, &mut outbox);
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(outbox, vec![Message::AiStart]);
    }

    /// 「编辑」菜单含全部五条文档内动作(菜单归属与命令注册表同一事实源,
    /// 蒙层/设置页共用)。
    #[test]
    fn edit_menu_lists_every_edit_command() {
        let edit: Vec<Command> = EDIT_MENU.iter().flat_map(|s| s.iter().copied()).collect();
        assert!(edit.contains(&Command::GotoLine), "跳转到行进「编辑」菜单");
        for cmd in &edit {
            assert_eq!(
                cmd.group(),
                CommandGroup::Edit,
                "{cmd:?} 挂在「编辑」菜单就必须是 Edit 组"
            );
        }
        // 反向补漏:注册表里每一条 Edit 组命令都必须在菜单里(新命令忘记
        // 加菜单时在这里红)
        for cmd in Command::ALL {
            if cmd.group() == CommandGroup::Edit {
                assert!(
                    edit.contains(&cmd),
                    "{cmd:?} 是 Edit 组命令却不在「编辑」菜单"
                );
            }
        }
    }

    /// 全命令覆盖断言(矩阵的测试面,docs/menu-coverage.md):遍历
    /// `Command::ALL`,除矩阵标注豁免项外每条命令都恰好出现在一个菜单里
    /// 一次。当前豁免清单为空(注册表不含上下文类浮标动作,全量命令都
    /// 适合进菜单);新命令加进 ALL 而忘进 MENUS 时这里红。
    #[test]
    fn every_command_has_a_menu_entry() {
        let entries = menu_entries();
        for cmd in Command::ALL {
            let count = entries.iter().filter(|(_, listed)| *listed == cmd).count();
            assert_eq!(
                count, 1,
                "{cmd:?} 应恰好出现在一个菜单里一次(实际 {count} 次);\
                 豁免口径见 docs/menu-coverage.md"
            );
        }
    }

    /// 菜单归属与命令分组一致(分组规范的测试面):每条命令挂的菜单 ==
    /// `Command::group()` 对应菜单,Tab 组挂「文件」尾部是 `CommandGroup`
    /// 文档的既定豁免。菜单常量里出现「不属于这个菜单的命令」时红。
    #[test]
    fn menu_placement_matches_command_group() {
        for (title, cmd) in menu_entries() {
            assert_eq!(
                title,
                expected_menu(cmd.group()),
                "{cmd:?}({}) 应挂在「{}」菜单",
                cmd.group().label(),
                expected_menu(cmd.group())
            );
        }
    }

    /// 分组渲染不 panic:全部菜单的分组段展开渲染(`menu_button` 未展开时
    /// 闭包不执行,所以直接调 `draw_sections`),纯渲染不产生消息;整条
    /// 菜单栏(收起态)在**明暗两主题**下各渲染一帧(#62 M2 排版统一的
    /// 不 panic 断言面)。
    #[test]
    fn all_menu_sections_render_without_panic() {
        let keymap = Keymap::builtin();
        let state = State::default();
        let mut outbox = Vec::new();
        for (_, _, sections) in MENUS {
            let ctx = egui::Context::default();
            let output = ctx.run_ui(RawInput::default(), |ui| {
                draw_sections(ui, sections, &keymap, &state, &mut outbox);
            });
            output.drop_without_applying_deltas();
        }
        assert!(outbox.is_empty(), "纯渲染不产生消息");
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            let ctx = egui::Context::default();
            ctx.set_theme(theme);
            let bar = Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, crate::ui::tokens::TITLEBAR_H),
            );
            let output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    super::hamburger_button_at(ui, crate::ui::titlebar::menu_button_rect(bar));
                    if super::menu_row_open(ui.ctx()) {
                        super::menu_row(
                            ui,
                            bar,
                            &keymap,
                            &state,
                            &mut outbox,
                            None::<fn(usize, egui::Id, Rect)>,
                        );
                    }
                },
            );
            output.drop_without_applying_deltas();
            assert!(outbox.is_empty(), "菜单栏收起态渲染不产生消息({theme:?})");
        }
    }

    /// 新增菜单入口(格式 17 条 + 视图 3 条,以及既有全部条目)逐条点击
    /// 一次,发出的消息就是该命令的归约入口 —— 与快捷键触发
    /// (`command::poll_shortcuts` → `cmd.message()`)殊途同归。
    #[test]
    fn clicking_every_menu_item_sends_its_command_message() {
        for (_, _, sections) in MENUS {
            for cmd in sections.iter().flat_map(|s| s.iter().copied()) {
                let ctx = egui::Context::default();
                let mut outbox = Vec::new();
                let keymap = Keymap::builtin();
                let rect = Cell::new(Rect::NOTHING);
                ctx.run_ui(RawInput::default(), |ui| {
                    rect.set(item(ui, cmd, &keymap, None, &mut outbox).rect);
                })
                .drop_without_applying_deltas();
                assert!(outbox.is_empty(), "{cmd:?} 仅渲染不产生消息");

                let center = rect.get().center();
                let click = |pressed| Event::PointerButton {
                    pos: center,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                };
                ctx.run_ui(
                    RawInput {
                        events: vec![Event::PointerMoved(center), click(true), click(false)],
                        ..Default::default()
                    },
                    |ui| {
                        item(ui, cmd, &keymap, None, &mut outbox);
                    },
                )
                .drop_without_applying_deltas();
                assert_eq!(
                    outbox,
                    vec![cmd.message()],
                    "{cmd:?} 的菜单入口应发出该命令的归约消息"
                );
            }
        }
    }

    /// 菜单项右侧的键位文本与 keymap 同源(照 #32 tooltip 同源先例):
    /// 出厂绑定如实显示,用户改绑后跟随新键位、旧键位文本消失。抽查
    /// 三面:Save(常用带修饰键)、GotoLine(近期新增)、ExportPdf(出厂
    /// 无绑定的对照 —— 无绑定条目不该挤出任何键位文本)。
    #[test]
    fn menu_item_shortcuts_follow_keymap() {
        fn shape_text(output: &FullOutput) -> String {
            output
                .shapes
                .iter()
                .filter_map(|clipped| {
                    let egui::epaint::Shape::Text(text) = &clipped.shape else {
                        return None;
                    };
                    Some(text.galley.job.text.clone())
                })
                .collect::<Vec<_>>()
                .join("\n")
        }

        let keymap = Keymap::builtin();
        let save_key = keymap
            .get(Command::Save)
            .expect("Save 出厂有绑定")
            .keyboard();
        let goto_key = keymap
            .get(Command::GotoLine)
            .expect("GotoLine 出厂有绑定")
            .keyboard();

        let ctx = egui::Context::default();
        let mut outbox = Vec::new();
        let output = ctx.run_ui(RawInput::default(), |ui| {
            item(ui, Command::Save, &keymap, None, &mut outbox);
            item(ui, Command::GotoLine, &keymap, None, &mut outbox);
        });
        let expected_save = ctx.format_shortcut(&save_key);
        let expected_goto = ctx.format_shortcut(&goto_key);
        let text = shape_text(&output);
        output.drop_without_applying_deltas();
        assert!(
            text.contains(&expected_save),
            "保存条目应显示出厂键位 {expected_save:?}(实际绘制:{text:?})"
        );
        assert!(
            text.contains(&expected_goto),
            "跳转条目应显示出厂键位 {expected_goto:?}(实际绘制:{text:?})"
        );

        // 用户改绑:保存改 Ctrl+K 后,菜单项跟随新键位,旧键位文本不再出现
        let mut rebound = Keymap::builtin();
        rebound.set(
            Command::Save,
            Some(crate::keymap::Shortcut {
                modifiers: egui::Modifiers::COMMAND,
                key: egui::Key::K,
            }),
        );
        let new_key = rebound.get(Command::Save).expect("改绑后有绑定").keyboard();
        let ctx = egui::Context::default();
        let old_text = expected_save.clone();
        let mut outbox = Vec::new();
        let output = ctx.run_ui(RawInput::default(), |ui| {
            item(ui, Command::Save, &rebound, None, &mut outbox);
        });
        let expected_new = ctx.format_shortcut(&new_key);
        let text = shape_text(&output);
        output.drop_without_applying_deltas();
        assert!(
            text.contains(&expected_new),
            "改绑后菜单项应显示 {expected_new:?}(实际绘制:{text:?})"
        );
        assert!(
            !text.contains(&old_text),
            "改绑后旧键位 {old_text:?} 不该继续显示(实际绘制:{text:?})"
        );

        // 无绑定对照:ExportPdf 出厂无键位(command.rs 刻意不绑),条目
        // 只显示名字 —— 渲染面由 ai_item_renders_without_shortcut… 同款
        // None 分支覆盖,这里钉住「出厂确实无绑定」这一前提
        assert_eq!(keymap.get(Command::ExportPdf), None);
    }

    // —— #62 M2:Alt 助记键(冲突审计 + 触发行为 + 排版统一)——

    /// 全部菜单标题助记字母(含「设置」「帮助」),供冲突审计与显示断言共用。
    fn title_letters() -> Vec<char> {
        MENUS
            .iter()
            .map(|(_, letter, _)| *letter)
            .chain([SETTINGS_MNEMONIC, HELP_MNEMONIC])
            .collect()
    }

    /// **冲突审计(本模块核心红线)**:标题助记字母全局唯一,且逐一对照
    /// keymap 出厂表的全部 Alt 类绑定 —— 非 mac 上任何「modifiers 含 Alt」
    /// 的绑定,其主键都不得出现在标题字母集(当前占用者:Alt+T 主题
    /// #45、Ctrl+Alt+R 预览栏);mac 的 ⌥⌘F(替换,带 ⌘)按编译目标豁免
    /// ——mac 不启用助记层(决策 #119),且严格位匹配下纯 Alt+F 与
    /// ⌥⌘F 本就不是同一组合。遍历断言,非恒真:给「格式」分了 F、或将来
    /// 新命令绑了 Alt+F,这里红。
    #[test]
    fn title_mnemonics_unique_and_clear_of_keymap_alt_bindings() {
        let letters = title_letters();
        let mut sorted = letters.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            letters.len(),
            "标题助记字母全局唯一:{letters:?}"
        );

        let keymap = Keymap::builtin();
        for cmd in Command::ALL {
            let Some(shortcut) = keymap.get(cmd) else {
                continue;
            };
            // 纯 Alt 绑定(任何平台都与助记层同组合,必须避开);带 Ctrl/Cmd
            // 的 Alt 组合在非 mac 上也一并避开(保守口径,防将来把检测改宽)
            let clashes = shortcut.modifiers.alt
                && (!cfg!(target_os = "macos")
                    || shortcut.modifiers.matches_exact(egui::Modifiers::ALT));
            if !clashes {
                continue;
            }
            let key_letter = super::key_letter(shortcut.key);
            if let Some(letter) = key_letter {
                assert!(
                    !letters.contains(&letter),
                    "{} 绑定 {} 与菜单标题助记 {} 撞键",
                    cmd.id(),
                    shortcut.platform_text(),
                    letter
                );
            }
        }
    }

    /// 条目助记在**每个菜单内**唯一(跨菜单允许重复:裸字母按当前展开的
    /// 菜单局部解析);同时钉住「keymap 出厂表无裸字母绑定」—— 条目层
    /// 裸字母命名空间与快捷键命名空间零交集的前提(`bindable` 拒绝裸
    /// 字母是机制,这里是出厂表的事实断言)。
    #[test]
    fn item_mnemonics_unique_per_menu_and_keymap_has_no_bare_letters() {
        for (title, _, sections) in MENUS {
            let letters: Vec<char> = sections
                .iter()
                .flat_map(|section| section.iter().copied())
                .map(command_mnemonic)
                .collect();
            let mut sorted = letters.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(
                sorted.len(),
                letters.len(),
                "「{title}」菜单内条目助记唯一:{letters:?}"
            );
        }
        let settings_letters: Vec<char> = SETTINGS_TAB_MNEMONICS
            .iter()
            .map(|(_, letter)| *letter)
            .collect();
        let mut sorted = settings_letters.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), settings_letters.len(), "「设置」菜单内唯一");

        for cmd in Command::ALL {
            let Some(shortcut) = Keymap::builtin().get(cmd) else {
                continue;
            };
            // 条目助记的命名空间是裸「字母/数字」(key_letter 能映射的键);
            // 无修饰功能键(F11 禅定)不在其中,与助记层零交集
            if shortcut.modifiers.is_none() && super::key_letter(shortcut.key).is_some() {
                panic!(
                    "{} 的出厂绑定 {} 是裸字母/数字键 —— 会与条目助记(展开态裸字母)互抢",
                    cmd.id(),
                    shortcut.platform_text()
                );
            }
        }
    }

    /// 助记后缀的平台口径(#62 M2)+ 缩号口径(2026-10-12 中英文等高):
    /// macOS 无 Alt 助记惯例(菜单助记符由系统菜单层处理,自绘菜单不显示),
    /// job 只有正文一段;其余平台渲染「文件(F)」形态,后缀段字号 =
    /// 正文 × [`MNEMONIC_SCALE`](Inter 字母墨高高于 CJK 汉字,缩号取平)。
    /// 按编译目标断言,两侧在各自平台 CI 上跑。
    #[test]
    fn mnemonic_suffix_per_platform() {
        let ctx = egui::Context::default();
        let mut job = None;
        let output = ctx.run_ui(RawInput::default(), |ui| {
            job = Some(mnemonic_label(ui, "文件", 'F'));
        });
        output.drop_without_applying_deltas();
        let job = job.expect("job 已产出");
        if cfg!(target_os = "macos") {
            assert_eq!(job.text, "文件", "mac 不显示助记后缀");
            assert_eq!(job.sections.len(), 1, "mac 只有一段正文");
        } else {
            assert_eq!(job.text, "文件(F)", "非 mac 渲染助记后缀");
            // MNEMONIC_SCALE = 1.0 时正文与后缀 TextFormat 全等,egui
            // `LayoutJob::append` 把相邻同格式段合并成一段 —— 这是纯
            // 优化,渲染逐字一致;若将来恢复异号后缀,这里会回到 2 段。
            assert_eq!(job.sections.len(), 1, "同字号后缀并入正文段");
            assert_eq!(
                job.sections[0].format.font_id.size,
                egui::TextStyle::Button
                    .resolve(&egui::Style::default())
                    .size,
                "助记后缀与正文同字号(基线只差 1px,见 MNEMONIC_SCALE 注释)"
            );
        }
    }

    fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn key_release(key: egui::Key, modifiers: egui::Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers,
        }
    }

    /// 标题栏菜单的驱动帧:复刻 `ui::titlebar` 的调用序(汉堡 → 读开态 →
    /// 展开时画标签行),收集各组 popup id 与矩形。bar 固定 900×TITLEBAR_H,
    /// 与 `titlebar::menu_button_rect` 的取值口径同源。
    struct MenuFrame {
        row_open: bool,
        sections: Vec<(usize, egui::Id, Rect)>,
        outbox: Vec<Message>,
        shapes: Vec<egui::epaint::ClippedShape>,
    }

    fn menu_frame(ctx: &egui::Context, events: Vec<Event>) -> MenuFrame {
        menu_frame_at_width(ctx, events, 900.0)
    }

    fn menu_frame_at_width(ctx: &egui::Context, events: Vec<Event>, width: f32) -> MenuFrame {
        let keymap = Keymap::builtin();
        let state = State::default();
        let bar = Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width, crate::ui::tokens::TITLEBAR_H),
        );
        let mut outbox = Vec::new();
        let mut sections = Vec::new();
        let output = ctx.run_ui(
            RawInput {
                events,
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                super::hamburger_button_at(ui, crate::ui::titlebar::menu_button_rect(bar));
                if super::menu_row_open(ui.ctx()) {
                    super::menu_row(
                        ui,
                        bar,
                        &keymap,
                        &state,
                        &mut outbox,
                        Some(|index, id, rect| sections.push((index, id, rect))),
                    );
                }
            },
        );
        let output_shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        MenuFrame {
            row_open: super::menu_row_open(ctx),
            sections,
            outbox,
            shapes: output_shapes,
        }
    }

    /// 在本帧 shapes 里找含 `needle` 的文本矩形(条目定位用)。
    fn find_text_rect(frame: &MenuFrame, needle: &str) -> Option<Rect> {
        frame.shapes.iter().find_map(|clipped| {
            let egui::epaint::Shape::Text(text) = &clipped.shape else {
                return None;
            };
            text.galley
                .job
                .text
                .contains(needle)
                .then(|| clipped.shape.visual_bounding_rect())
        })
    }

    /// 各文本 job 的 (全文, 正文段字号) 断面(排版统一断言用)。
    fn job_sections(shapes: &[egui::epaint::ClippedShape], needle: &str) -> Vec<(String, f32)> {
        shapes
            .iter()
            .filter_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                let job = &text.galley.job;
                job.sections
                    .first()
                    .filter(|_| job.text.contains(needle))
                    .map(|section| (job.text.clone(), section.format.font_id.size))
            })
            .collect()
    }

    fn section_id(frame: &MenuFrame, index: usize) -> egui::Id {
        frame
            .sections
            .iter()
            .find(|(i, _, _)| *i == index)
            .expect("组名按钮本帧渲染")
            .1
    }

    fn popup_open(ctx: &egui::Context, id: &egui::Id) -> bool {
        egui::containers::Popup::is_id_open(ctx, *id)
    }

    /// 指针在 `pos` 处单击:moved / press / release **分三帧**注入 ——
    /// egui 要求「按下帧之前指针已停在目标上」,同帧三连发不算点击
    /// (layout.rs 面板层点击测试同款节奏,实证记录见其文档)。
    fn click_frames(ctx: &egui::Context, pos: egui::Pos2) -> Vec<Message> {
        let click = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let mut outbox = Vec::new();
        // 三帧各自的消息都要收(条目点击的消息落在 release 帧)
        outbox.extend(menu_frame(ctx, vec![Event::PointerMoved(pos)]).outbox);
        outbox.extend(menu_frame(ctx, vec![click(true)]).outbox);
        outbox.extend(menu_frame(ctx, vec![click(false)]).outbox);
        outbox
    }

    /// 点击汉堡 → 标题栏内展开标签行(不弹任何下拉);再点 → 收起。
    #[test]
    fn hamburger_click_toggles_menu_row() {
        let ctx = egui::Context::default();
        let bar = Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, crate::ui::tokens::TITLEBAR_H),
        );
        let hamburger = crate::ui::titlebar::menu_button_rect(bar).center();
        click_frames(&ctx, hamburger);
        let frame = menu_frame(&ctx, vec![]);
        assert!(frame.row_open, "点击汉堡展开标签行");
        assert!(
            frame.outbox.is_empty(),
            "开合标签行不产生命令消息:{:?}",
            frame.outbox
        );
        let frame = menu_frame(&ctx, vec![]);
        assert!(frame.row_open, "空帧保持展开");
        for (_, id, _) in &frame.sections {
            assert!(!popup_open(&ctx, id), "展开标签行不自动弹下拉");
        }
        click_frames(&ctx, hamburger);
        let frame = menu_frame(&ctx, vec![]);
        assert!(!frame.row_open, "再点汉堡收起标签行");
    }

    /// Alt+F 展开标签行并**直接弹出**「文件」组(标题层助记的行为面):
    /// 其他组不弹。Alt+E/Alt+S(设置组)同理。Alt+T(主题键 #45)不开任何
    /// 东西 —— 字母避让审计的行为面。
    #[test]
    fn alt_letter_opens_matching_menu_only() {
        let ctx = egui::Context::default();
        menu_frame(&ctx, vec![key_event(egui::Key::F, egui::Modifiers::ALT)]);
        let frame = menu_frame(&ctx, vec![]);
        assert_eq!(
            frame.row_open,
            !cfg!(target_os = "macos"),
            "Alt+F 仅在非 macOS 平台展开标签行"
        );
        if !cfg!(target_os = "macos") {
            assert!(
                popup_open(&ctx, &section_id(&frame, 0)),
                "Alt+F 弹出「文件」"
            );
            for (index, id, _) in &frame.sections {
                if *index != 0 {
                    assert!(!popup_open(&ctx, id), "Alt+F 不该顺带弹出第 {index} 组");
                }
            }
        }

        // Alt+E 落「编辑」
        let ctx = egui::Context::default();
        menu_frame(&ctx, vec![key_event(egui::Key::E, egui::Modifiers::ALT)]);
        let frame = menu_frame(&ctx, vec![]);
        if !cfg!(target_os = "macos") {
            assert!(frame.row_open);
            assert!(
                popup_open(&ctx, &section_id(&frame, 1)),
                "Alt+E 弹出「编辑」"
            );
        }

        // Alt+S 落「设置」(非 MENUS 组同样可达)
        let ctx = egui::Context::default();
        menu_frame(&ctx, vec![key_event(egui::Key::S, egui::Modifiers::ALT)]);
        let frame = menu_frame(&ctx, vec![]);
        if !cfg!(target_os = "macos") {
            assert!(frame.row_open);
            assert!(
                popup_open(&ctx, &section_id(&frame, MENUS.len())),
                "Alt+S 弹出「设置」"
            );
        }

        // Alt+T 是主题切换的出厂键(#45 K1),不分配给任何菜单
        let ctx = egui::Context::default();
        menu_frame(&ctx, vec![key_event(egui::Key::T, egui::Modifiers::ALT)]);
        let frame = menu_frame(&ctx, vec![]);
        assert!(!frame.row_open, "Alt+T 不展开标签行");

        // 严格匹配的对照:Ctrl+Alt+R(切换预览栏)带 Ctrl,不是纯 Alt
        let ctx = egui::Context::default();
        menu_frame(
            &ctx,
            vec![key_event(
                egui::Key::R,
                egui::Modifiers::COMMAND | egui::Modifiers::ALT,
            )],
        );
        let frame = menu_frame(&ctx, vec![]);
        assert!(!frame.row_open, "Ctrl+Alt+R 是命令快捷键,不触发菜单助记");
    }

    /// 基础交互核验:点组名开它的下拉(再点收);Esc 关下拉但标签行保留;
    /// 下拉关闭后点击行外区域收起整行 —— Windows 菜单栏的「点外面就收」。
    #[test]
    fn open_menu_closes_on_escape_and_outside_click() {
        if cfg!(target_os = "macos") {
            // mac 无 Alt 助记层,本测试的展开手段(Alt+F)不可达;mac 的
            // 点击路径已由 `hamburger_click_toggles_menu_row` 与设置/帮助
            // 点击测试覆盖。
            return;
        }
        let ctx = egui::Context::default();
        menu_frame(&ctx, vec![key_event(egui::Key::F, egui::Modifiers::ALT)]);
        let frame = menu_frame(&ctx, vec![]);
        assert!(
            popup_open(&ctx, &section_id(&frame, 0)),
            "前置:「文件」已弹"
        );

        // Esc:下拉收,标签行保留
        menu_frame(
            &ctx,
            vec![key_event(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        let frame = menu_frame(&ctx, vec![]);
        assert!(!popup_open(&ctx, &section_id(&frame, 0)), "Esc 关下拉");
        assert!(frame.row_open, "Esc 不收标签行");

        // 点组名开对应下拉:点「编辑」
        let edit_rect = frame.sections.iter().find(|(i, _, _)| *i == 1).unwrap().2;
        click_frames(&ctx, edit_rect.center());
        let frame = menu_frame(&ctx, vec![]);
        assert!(popup_open(&ctx, &section_id(&frame, 1)), "点组名弹「编辑」");
        assert!(
            !popup_open(&ctx, &section_id(&frame, 0)),
            "「文件」随之收起"
        );
        assert!(frame.row_open, "切组不收标签行");

        // 下拉全关后点击行外 → 收整行:先 Esc 关「编辑」,再点远处
        menu_frame(
            &ctx,
            vec![key_event(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        let far = egui::Pos2::new(700.0, 400.0);
        click_frames(&ctx, far);
        let frame = menu_frame(&ctx, vec![]);
        assert!(!frame.row_open, "下拉全关后点行外收起标签行");
    }

    /// 单击 Alt 不再打开任何东西(2026-10-12 坤哥报告的两条误触的退役面):
    /// (a) 裸 Alt 按下→抬起不展开;(b) Ctrl 按住时 Alt 按下→抬起不展开
    /// —— Deepin 工作区切换(Ctrl+Alt+方向键)不夺焦点,应用侧收到完整的
    /// 修饰键序列,旧武装状态机在 Alt 抬起帧误判成「单击」;(c) 失焦场景
    /// 的孤立 Alt release 不展开(#69 原案,武装链路退役后天然成立,钉住
    /// 防回归)。Alt+字母 仍展开并弹组(见
    /// `alt_letter_opens_matching_menu_only`),与本条互不干扰。
    #[test]
    fn alt_release_never_opens_menu() {
        // (a) 裸 Alt 按下→抬起
        let ctx = egui::Context::default();
        menu_frame(
            &ctx,
            vec![key_event(egui::Key::AltLeft, egui::Modifiers::NONE)],
        );
        let frame = menu_frame(
            &ctx,
            vec![key_release(egui::Key::AltLeft, egui::Modifiers::ALT)],
        );
        assert!(!frame.row_open, "单击 Alt 不展开标签行");

        // (b) Ctrl 按住时 Alt 按下→抬起(Deepin 工作区切换的按键序列)
        let ctx = egui::Context::default();
        menu_frame(
            &ctx,
            vec![key_event(egui::Key::ControlLeft, egui::Modifiers::COMMAND)],
        );
        menu_frame(
            &ctx,
            vec![key_event(
                egui::Key::AltLeft,
                egui::Modifiers::COMMAND | egui::Modifiers::ALT,
            )],
        );
        let frame = menu_frame(
            &ctx,
            vec![key_release(
                egui::Key::AltLeft,
                egui::Modifiers::COMMAND | egui::Modifiers::ALT,
            )],
        );
        assert!(!frame.row_open, "Ctrl+Alt 组合的 Alt 抬起不展开标签行");

        // (c) Alt 按下后失焦、抬起来自切回(#69 场景)
        let ctx = egui::Context::default();
        menu_frame(
            &ctx,
            vec![key_event(egui::Key::AltLeft, egui::Modifiers::NONE)],
        );
        menu_frame(&ctx, vec![Event::WindowFocused(false)]);
        menu_frame(&ctx, vec![Event::WindowFocused(true)]);
        let frame = menu_frame(
            &ctx,
            vec![key_release(egui::Key::AltLeft, egui::Modifiers::ALT)],
        );
        assert!(!frame.row_open, "孤立 Alt release 不展开标签行");
    }

    /// 标签行展开态的裸字母触发条目(条目层助记):Alt+E 开「编辑」→
    /// 展开帧按 F → 发出「查找」的归约消息,下拉与标签行一并收起;同字母
    /// 的 Key 与 Text 事件都被消费(不落进聚焦的编辑器)。
    #[test]
    fn bare_letter_in_open_menu_fires_item_and_closes() {
        let ctx = egui::Context::default();
        menu_frame(&ctx, vec![key_event(egui::Key::E, egui::Modifiers::ALT)]);
        let frame = menu_frame(&ctx, vec![]);
        if cfg!(target_os = "macos") {
            assert!(!frame.row_open, "macOS 无 Alt 助记层");
            return;
        }
        let edit_id = section_id(&frame, 1);
        assert!(popup_open(&ctx, &edit_id), "前置:「编辑」已弹");

        // 裸 F → 命中「查找」(F),发消息并收起
        let frame = menu_frame(
            &ctx,
            vec![
                key_event(egui::Key::F, egui::Modifiers::NONE),
                Event::Text("f".to_owned()),
            ],
        );
        assert_eq!(
            frame.outbox,
            vec![Message::FindBarToggled(true)],
            "展开的「编辑」组里按 F 应触发「查找」"
        );
        assert!(!popup_open(&ctx, &edit_id), "条目触发后下拉收起");
        assert!(!frame.row_open, "条目触发后标签行一并收起");
    }

    /// 「设置」组直达页的真实点击路径(#67 M1 抽查):「设置」不在
    /// [`MENUS`] 表里(非命令,直达设置页),`clicking_every_menu_item…`
    /// 不遍历它 —— 这里走完整路径:点汉堡展开 → 点「设置」组名 → 展开帧
    /// 定位条目文本矩形 → 指针逐帧点击 → 发出 [`Message::SettingsOpened`]
    /// 的对应页并整行收起;消息再喂进 `State::apply` 落出打开的页签。
    #[test]
    fn settings_menu_direct_tabs_click_through_and_reduce() {
        for (needle, expected) in [
            ("外观", SettingsTab::Appearance),
            ("快捷键", SettingsTab::Keymap),
        ] {
            let ctx = egui::Context::default();
            let bar = Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, crate::ui::tokens::TITLEBAR_H),
            );
            let hamburger = crate::ui::titlebar::menu_button_rect(bar).center();
            click_frames(&ctx, hamburger);
            let frame = menu_frame(&ctx, vec![]);
            assert!(frame.row_open, "前置:标签行已展开");
            let settings_rect = frame
                .sections
                .iter()
                .find(|(i, _, _)| *i == MENUS.len())
                .unwrap()
                .2;
            click_frames(&ctx, settings_rect.center());
            let frame = menu_frame(&ctx, vec![]);
            let settings_id = section_id(&frame, MENUS.len());
            assert!(popup_open(&ctx, &settings_id), "前置:「设置」下拉已开");

            // 展开帧 ×2:popup 记忆开态到闭包真正绘制隔一帧(实证节奏),
            // 取末帧 shapes 定位条目矩形
            let mut rect = None;
            for _ in 0..2 {
                let frame = menu_frame(&ctx, vec![]);
                assert!(frame.outbox.is_empty(), "展开不点击不产生消息");
                rect = find_text_rect(&frame, needle);
                if rect.is_some() {
                    break;
                }
            }
            let rect = rect.unwrap_or_else(|| panic!("{needle:?} 条目未绘制"));

            // moved → press → release,点击条目中心(消息落在 release 帧,
            // 由 click_frames 累积返回)
            let clicked = click_frames(&ctx, rect.center());
            assert_eq!(
                clicked,
                vec![Message::SettingsOpened(expected)],
                "{needle:?} 条目点击应发出直达该页的消息"
            );
            let frame = menu_frame(&ctx, vec![]);
            assert!(!frame.row_open, "条目选中后标签行整行收起");

            // 归约一段:直达页消息落 State,设置窗开在对应页签
            let mut state = crate::state::State::default();
            state.apply(Message::SettingsOpened(expected));
            assert!(state.settings.open, "归约后设置窗打开");
            assert_eq!(state.settings.tab, expected, "归约后落在直达页签");
        }
    }

    /// 「帮助」组(#71 M1)的完整路径:点汉堡展开 → 点「帮助」组名 →
    /// 真实点击「关于 LaterMD…」条目 → 发出 [`Message::AboutOpened`] →
    /// 归约后 `about.open` 为真(点击 → 消息 → 归约一段齐;帮助非命令
    /// 菜单,`clicking_every_menu_item…` 不遍历它,这里补齐真实路径)。
    #[test]
    fn help_menu_about_entry_click_through_and_reduce() {
        let ctx = egui::Context::default();
        let bar = Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, crate::ui::tokens::TITLEBAR_H),
        );
        let hamburger = crate::ui::titlebar::menu_button_rect(bar).center();
        click_frames(&ctx, hamburger);
        let frame = menu_frame(&ctx, vec![]);
        assert!(frame.row_open, "前置:标签行已展开");
        let help_rect = frame
            .sections
            .iter()
            .find(|(i, _, _)| *i == MENUS.len() + 1)
            .unwrap()
            .2;
        click_frames(&ctx, help_rect.center());
        let frame = menu_frame(&ctx, vec![]);
        let help_id = section_id(&frame, MENUS.len() + 1);
        assert!(popup_open(&ctx, &help_id), "前置:「帮助」下拉已开");

        // 展开帧 ×2(设置组同款节奏),定位「关于 LaterMD…」条目
        let mut rect = None;
        for _ in 0..2 {
            let frame = menu_frame(&ctx, vec![]);
            assert!(frame.outbox.is_empty(), "展开不点击不产生消息");
            rect = find_text_rect(&frame, "关于 LaterMD…");
            if rect.is_some() {
                break;
            }
        }
        let rect = rect.expect("「关于 LaterMD…」条目未绘制");
        let clicked = click_frames(&ctx, rect.center());
        assert_eq!(
            clicked,
            vec![Message::AboutOpened],
            "「关于 LaterMD…」条目点击应发出开窗消息"
        );
        let frame = menu_frame(&ctx, vec![]);
        assert!(!frame.row_open, "条目选中后标签行整行收起");

        // 归约一段:消息落 State,关于窗开
        let mut state = crate::state::State::default();
        state.apply(Message::AboutOpened);
        assert!(state.about.open, "归约后关于窗打开");
    }

    /// 帮助组展开态的裸字母触发(条目层助记):Alt+H 展开并弹「帮助」→
    /// 展开帧按 A → 发出 [`Message::AboutOpened`],下拉与标签行一并收起。
    #[test]
    fn bare_letter_in_open_help_menu_fires_about() {
        let ctx = egui::Context::default();
        menu_frame(&ctx, vec![key_event(egui::Key::H, egui::Modifiers::ALT)]);
        let frame = menu_frame(&ctx, vec![]);
        if cfg!(target_os = "macos") {
            assert!(!frame.row_open, "macOS 无 Alt 助记层");
            return;
        }
        let help_id = section_id(&frame, MENUS.len() + 1);
        assert!(popup_open(&ctx, &help_id), "前置:「帮助」已弹");

        // 裸 A → 命中「关于 LaterMD…」,发消息并收起
        let frame = menu_frame(
            &ctx,
            vec![
                key_event(egui::Key::A, egui::Modifiers::NONE),
                Event::Text("a".to_owned()),
            ],
        );
        assert_eq!(
            frame.outbox,
            vec![Message::AboutOpened],
            "展开的「帮助」组里按 A 应触发「关于 LaterMD…」"
        );
        assert!(!popup_open(&ctx, &help_id), "条目触发后下拉收起");
        assert!(!frame.row_open, "标签行一并收起");
    }

    /// 开关类菜单条目的勾选态(#67 M2):`toggle_checked` 从 [`State`] 取
    /// 真值(与设置页/标题栏按钮同一事实源),布偶开关命令全覆盖、非开关
    /// 命令(含明暗互换的「切换主题」)`None`;勾选前缀只在开态渲染 ✓,
    /// 关态为占位空格、非开关条目无前缀。
    #[test]
    fn view_menu_toggle_items_show_check_state() {
        // 真值层:出厂默认下各开关的当前值(与 ThemeSettings/LayoutSettings
        // 的出厂字段一致)
        let state = State::default();
        assert_eq!(toggle_checked(Command::ToggleSidebar, &state), Some(true));
        assert_eq!(
            toggle_checked(Command::ToggleLivePreview, &state),
            Some(false),
            "出厂源码模式,Live 未开"
        );
        assert_eq!(
            toggle_checked(Command::TypewriterToggle, &state),
            Some(false)
        );
        assert_eq!(
            toggle_checked(Command::FocusModeToggle, &state),
            Some(false)
        );
        assert_eq!(toggle_checked(Command::ToggleMinimap, &state), Some(true));
        assert_eq!(toggle_checked(Command::ToggleZen, &state), Some(false));
        // 明暗互换不是开/关,主题条目无勾选语义;非视图命令同样 None
        assert_eq!(toggle_checked(Command::ToggleTheme, &state), None);
        assert_eq!(toggle_checked(Command::Save, &state), None);

        // 勾选态随状态翻转:minimap 关掉后,同一命令取到 Some(false)
        let mut state = state;
        state.apply(Message::ToggleMinimap);
        assert_eq!(
            toggle_checked(Command::ToggleMinimap, &state),
            Some(false),
            "归约翻转后勾选态立即跟随(三方一致的菜单侧)"
        );

        // 渲染层:✓ 前缀只出现在 Some(true) 的条目文本里
        fn item_text(cmd: Command, checked: Option<bool>) -> String {
            let ctx = egui::Context::default();
            let output = ctx.run_ui(RawInput::default(), |ui| {
                item(ui, cmd, &Keymap::builtin(), checked, &mut Vec::new());
            });
            let text = output
                .shapes
                .iter()
                .filter_map(|clipped| {
                    let egui::epaint::Shape::Text(text) = &clipped.shape else {
                        return None;
                    };
                    Some(text.galley.job.text.clone())
                })
                .collect::<Vec<_>>()
                .join("\n");
            output.drop_without_applying_deltas();
            text
        }
        let on = item_text(Command::ToggleMinimap, Some(true));
        assert!(
            on.contains("✓") && on.contains("Minimap 缩略图"),
            "开态条目应渲染 ✓ 前缀 + 带助记后缀的显示名(实际:{on:?})"
        );
        let off = item_text(Command::ToggleMinimap, Some(false));
        assert!(
            !off.contains("✓") && off.contains("Minimap 缩略图"),
            "关态条目不渲染 ✓(占位空格保持开关条目对齐;实际:{off:?})"
        );
        let plain = item_text(Command::ToggleTheme, None);
        assert!(!plain.contains("✓"), "非开关条目无勾选前缀(实际:{plain:?})");
    }

    /// minimap 菜单开关的**全链路**(#67 M2):菜单条目真实点击 → 发出
    /// 翻转消息 → `State::apply` 翻转 `theme.show_minimap` → settings.json
    /// 即时落盘(重启 load_from 仍在)。勾选态取值与翻转后的字段一致 ——
    /// 菜单、快捷键、设置外观页复选框三方共用同一字段同一归约。
    #[test]
    fn minimap_menu_item_click_flips_theme_and_persists() {
        let dir =
            std::env::temp_dir().join(format!("latermd-menubar-minimap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut state = State::default();
        state.settings_dir = Some(dir.clone());
        assert!(state.theme.show_minimap, "出厂默认开");
        assert_eq!(toggle_checked(Command::ToggleMinimap, &state), Some(true));

        // 两帧点击:渲染帧拿矩形,点击帧按下并抬起
        let ctx = egui::Context::default();
        let keymap = Keymap::builtin();
        let mut outbox = Vec::new();
        let rect = Cell::new(Rect::NOTHING);
        ctx.run_ui(RawInput::default(), |ui| {
            rect.set(
                item(
                    ui,
                    Command::ToggleMinimap,
                    &keymap,
                    toggle_checked(Command::ToggleMinimap, &state),
                    &mut outbox,
                )
                .rect,
            );
        })
        .drop_without_applying_deltas();
        assert!(outbox.is_empty(), "仅渲染不产生消息");
        let center = rect.get().center();
        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        ctx.run_ui(
            RawInput {
                events: vec![Event::PointerMoved(center), click(true), click(false)],
                ..Default::default()
            },
            |ui| {
                item(
                    ui,
                    Command::ToggleMinimap,
                    &keymap,
                    toggle_checked(Command::ToggleMinimap, &state),
                    &mut outbox,
                );
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(outbox, vec![Message::ToggleMinimap], "菜单点击发出翻转消息");

        // 归约 + 落盘:字段翻转、勾选态跟随、settings.json 重启仍在
        state.apply(Message::ToggleMinimap);
        assert!(!state.theme.show_minimap, "菜单开关翻转字段");
        assert_eq!(
            toggle_checked(Command::ToggleMinimap, &state),
            Some(false),
            "翻转后勾选态一致"
        );
        let reloaded = crate::theme::ThemeSettings::load_from(&dir).unwrap();
        assert!(!reloaded.show_minimap, "菜单开关即时落盘,重启后仍为关");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 排版统一(#62 M2,2026-10-12 汉堡化后改为 job 断面):组名行标题
    /// 与条目按钮同走 egui `Button` 的 `TextStyle::Button` 档 —— 两侧
    /// galley 的**正文段**字号必须一致(守门将来一侧改字号另一侧没跟),
    /// 助记后缀段一律缩号 [`MNEMONIC_SCALE`](中英文等高的实现面);
    /// 窄窗口(400px)展开标签行渲染不 panic(溢出=裁切)。
    #[test]
    fn menubar_and_item_share_font_size_and_narrow_render_is_safe() {
        let keymap = Keymap::builtin();
        let ctx = egui::Context::default();
        let bar = Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, crate::ui::tokens::TITLEBAR_H),
        );
        click_frames(&ctx, crate::ui::titlebar::menu_button_rect(bar).center());
        let frame = menu_frame(&ctx, vec![]);
        assert!(frame.row_open, "前置:标签行已展开");
        let title = job_sections(&frame.shapes, "文件")
            .into_iter()
            .next()
            .expect("标签行渲染出带助记后缀的组名");
        let title_size = title.1;

        let entry_frame = {
            let mut outbox = Vec::new();
            let output = ctx.run_ui(RawInput::default(), |ui| {
                item(ui, Command::Save, &keymap, None, &mut outbox);
            });
            (output.shapes.clone(), output.drop_without_applying_deltas())
        };
        let entry = job_sections(&entry_frame.0, "保存")
            .into_iter()
            .next()
            .expect("菜单条目渲染出带助记后缀的显示名");
        assert_eq!(
            title_size, entry.1,
            "组名行标题与条目正文字号一致(排版统一,当前 Button 档)"
        );

        // 窄窗口(400px)展开标签行渲染不 panic(溢出=裁切)
        let ctx = egui::Context::default();
        let bar = Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(400.0, crate::ui::tokens::TITLEBAR_H),
        );
        click_frames(&ctx, crate::ui::titlebar::menu_button_rect(bar).center());
        set_menu_row_open(&ctx, true);
        let narrow = menu_frame_at_width(&ctx, vec![], 400.0);
        assert_eq!(narrow.sections.len(), section_count());
    }
}
