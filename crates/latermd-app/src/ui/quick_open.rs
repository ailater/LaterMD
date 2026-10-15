//! 「快速打开」浮层(#24 C3,Cmd/Ctrl+P)。
//!
//! ## 分工(与 `ui::image_dialog` / `ui::emoji_panel` 同一套写法)
//!
//! 本模块只收输入、只发消息:查询词 `query` 与选中下标 `selected` 归 UI
//! 原地持有(立即模式控件,草稿必须能就地 `&mut`);开关浮层与文件候选
//! 快照在归约([`QuickOpenState::open_snapshot`],由
//! `Message::ToggleQuickOpen` 触发)。选中文件发 [`Message::FileSelected`]
//! —— 与文件树点击**同一条** `open_path` 入口;选中命令直接发
//! `cmd.message()`;两者都补发一条 `ToggleQuickOpen` 关浮层。
//!
//! ## 数据源
//!
//! 文件 = `latermd_search::list_files`(与文件树 `find_by_name`、MCP
//! `list_files` 同一份实现,尊重 `.gitignore`,条目上限 500);开面板那
//! 一刻拍快照。排序用 [`crate::fuzzy`](#24 C1)的子序列打分;空 query
//! 时文件按 `list_files` 的字母序、命令按 `Command::ALL` 序各取前
//! [`MAX_PER_GROUP`] 条。上限截断(`truncated`)时照常工作,只是只搜已
//! 列出的条目(浮层底部提示行说明降级)。命令 = `Command::ALL` 全集按
//! `label()` 模糊匹配,行首画 `Command::icon()`。
//!
//! ## 尺寸铁律(COMMON,历史反馈环教训)
//!
//! 浮窗 `fixed_size`([`WINDOW_W`] × [`WINDOW_H`]):**宽度由此定死**
//! (输入框 `desired_width(f32::INFINITY)` 撑满,实测恒 560),高度端
//! egui 对不可调尺寸的窗口按内容自适应(egui `Resize::end` 的「Probably
//! a window」分支),但被 `WINDOW_H` 封顶 —— 每组条数又有 [`MAX_PER_GROUP`]
//! 上限,窗口高度**不随文件总数增长**。结果列表
//! `ScrollArea::max_height`([`LIST_MAX_H`])限高,长列表滚动。
//! **不使用**「内容定尺寸 + `available_*` + `auto_shrink([false, false])`」
//! 的无界组合。
//!
//! ## 键盘接管(方向键不被 TextEdit 抢走)
//!
//! 输入框用 [`QUICK_OPEN_EVENT_FILTER`] 把上下方向键与 Esc **锁**在输入
//! 框上(`Focus::begin_pass` 因此不会拿它们做焦点导航/清焦),`return_key`
//! 关掉回车换行 —— 之后浮层以非消费读(读事件流)取用:↑↓ 跨组循环
//! 移动、Enter 确认、Esc 关闭。这是查找条 `FIND_BAR_EVENT_FILTER` 的同款
//! 手法,即任务书「consume_key 或等价手段」里的等价手段,取其已在本仓库
//! 无头验证过的先例。焦点钉在输入框上:丢焦点(点击面板外 / Tab 移走)
//! 下一帧夺回,浮层在位时键盘永远属于它。

use std::path::{Path, PathBuf};

use crate::command::Command;
use crate::fuzzy;
use crate::state::Message;
use crate::ui::tokens;
use eframe::egui;

/// 浮窗定宽(COMMON 尺寸铁律:宽度定死)。
const WINDOW_W: f32 = 560.0;
/// 浮窗高度上限(外框,含标题栏):窗口高度按内容自适应(短列表不空撑),
/// 但内容布局区被它封顶 —— 最坏内容 ≈ 标题栏 24 + 内边距 12 + 输入行
/// 26 + 间距 6 + 列表限高 [`LIST_MAX_H`] + 截断提示 20。
const WINDOW_H: f32 = 430.0;
/// 结果列表限高(COMMON 尺寸铁律:ScrollArea max_height,超出滚动)。
const LIST_MAX_H: f32 = 320.0;
/// 每组最多展示条数(文件组与命令组各自的上限)。
const MAX_PER_GROUP: usize = 8;

/// 查询输入框的稳定 id:焦点钉在它上面(丢失即夺回)。
fn input_id() -> egui::Id {
    egui::Id::new("quick-open-query")
}

/// 输入框的事件过滤:上下方向键与 Esc 锁给输入框(单行内均为 no-op,锁的
/// 目的是挡住 `Focus::begin_pass` 的焦点导航/清焦),左右方向键留给查询
/// 词内移光标,Tab 放行(移动焦点,下一帧由焦点钉夺回)。查找条
/// `FIND_BAR_EVENT_FILTER` 的同款配置。
const QUICK_OPEN_EVENT_FILTER: egui::EventFilter = egui::EventFilter {
    tab: false,
    horizontal_arrows: true,
    vertical_arrows: true,
    escape: true,
};

/// 快速打开浮层状态(归约开/关与快照,UI 持查询词与选中下标)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuickOpenState {
    /// 浮层是否可见(`Message::ToggleQuickOpen` 翻转)。
    pub open: bool,
    /// 查询草稿(UI 原地持有;开面板时归约清空)。
    pub query: String,
    /// 选中行下标(**扁平序**:文件组在前、命令组在后,分组标题不占位)。
    pub selected: usize,
    /// 文件候选快照:相对 [`Self::root`] 的路径,`list_files` 的字母序;
    /// 开面板那一刻拍照,会话内不失效(面板开着换根是边角,重开即刷新)。
    pub files: Vec<PathBuf>,
    /// 快照对应的遍历根(选中文件时 join 回绝对路径);`None` = 未选根,
    /// 文件组为空,只剩命令组。
    pub root: Option<PathBuf>,
    /// `list_files` 达条目上限被截断:照常工作,但只搜已列出的条目
    /// (浮层底部提示行说明降级)。
    pub truncated: bool,
}

impl QuickOpenState {
    /// 打开浮层并拍文件快照(归约侧调用;IO 与文件树 `ensure_loaded` 同
    /// 口径 —— 本地磁盘毫秒级,同步在归约做)。查询词与选中项一并复位:
    /// 上次的搜索词对下一次打开没有意义(与 Emoji 面板开时清词同款取舍)。
    pub fn open_snapshot(&mut self, root: Option<&Path>) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.root = root.map(Path::to_path_buf);
        self.files.clear();
        self.truncated = false;
        let Some(root) = root else {
            return;
        };
        if let Ok(outcome) =
            latermd_search::list_files(root, None, None, latermd_search::MAX_LIST_ENTRIES)
        {
            self.files = outcome
                .entries
                .iter()
                .filter(|entry| !entry.is_dir && latermd_search::is_markdown(&entry.path))
                .map(|entry| entry.path.clone())
                .collect();
            self.truncated = outcome.truncated;
        }
    }

    /// 关浮层(只翻标志,快照与查询词留待下次开时复位)。
    pub fn close(&mut self) {
        self.open = false;
    }
}

/// 可见行(扁平序:文件组在上、命令组在下,分组标题不占选择位)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    /// `QuickOpenState::files` 的下标。
    File(usize),
    Cmd(Command),
}

/// 按 query 计算可见行:文件组用 [`fuzzy::rank`] 排序取前
/// [`MAX_PER_GROUP`],命令组按 `label()` 同款;空 query 时 rank 全量原序
/// 返回 —— 文件的输入序即 `list_files` 的字母序,命令即 `Command::ALL`
/// 序。纯函数,单测直接注入文件清单。
fn visible_rows(query: &str, files: &[PathBuf]) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let texts: Vec<String> = files
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    rows.extend(
        fuzzy::rank(query, &texts)
            .into_iter()
            .take(MAX_PER_GROUP)
            .map(|(index, _)| Row::File(index)),
    );
    let labels: Vec<&str> = Command::ALL.iter().map(|cmd| cmd.label()).collect();
    rows.extend(
        fuzzy::rank(query, &labels)
            .into_iter()
            .take(MAX_PER_GROUP)
            .map(|(index, _)| Row::Cmd(Command::ALL[index])),
    );
    rows
}

/// 本帧某裸键(无修饰键)是否按下。读事件流而非 `key_pressed` 是为了
/// 排除带修饰的组合(Shift+Enter / Cmd+↓ 不属于浮层的导航语义)。
fn bare_key_pressed(input: &egui::InputState, key: egui::Key) -> bool {
    input.events.iter().any(|event| {
        matches!(
            event,
            egui::Event::Key {
                key: pressed,
                pressed: true,
                modifiers,
                ..
            } if *pressed == key && modifiers.is_none()
        )
    })
}

/// 画浮层,返回浮窗响应(无头测试定位浮层矩形用,生产忽略)。
///
/// Enter / 点击行的执行都在消息里:文件 → [`Message::FileSelected`]
/// (与文件树点击同一条 `open_path` 入口),命令 → `cmd.message()`;
/// 两者随后补发 `ToggleQuickOpen` 关浮层 —— 唯独 `Command::QuickOpen`
/// 自身不补:它的消息就是这条翻转,补发会把「关」翻回「开」。
pub fn panel(
    ui: &mut egui::Ui,
    state: &mut QuickOpenState,
    outbox: &mut Vec<Message>,
) -> egui::Response {
    // 键盘读在输入框绘制**之前**:事件此帧尚未被任何控件消费,读取恒
    // 可见(过滤锁的方向键即使随后被输入框消费,也已在它之前读到)。
    let (down, up, enter, escape) = ui.input(|input| {
        (
            bare_key_pressed(input, egui::Key::ArrowDown),
            bare_key_pressed(input, egui::Key::ArrowUp),
            bare_key_pressed(input, egui::Key::Enter),
            bare_key_pressed(input, egui::Key::Escape),
        )
    });
    let rows = visible_rows(state.query.trim(), &state.files);
    // 选中下标夹进可见行数(查询变化把列表缩短时),空列表归零。
    state.selected = if rows.is_empty() {
        0
    } else {
        state.selected.min(rows.len() - 1)
    };
    // ↑↓ 跨组循环:扁平序整体移动,头尾相接。
    if !rows.is_empty() {
        if down {
            state.selected = (state.selected + 1) % rows.len();
        }
        if up {
            state.selected = (state.selected + rows.len() - 1) % rows.len();
        }
    }
    if escape {
        outbox.push(Message::ToggleQuickOpen);
    }
    let mut activate: Option<Row> = None;
    if enter && !rows.is_empty() {
        activate = rows.get(state.selected).copied();
    }

    // 焦点钉:丢焦点(点击面板外 / Tab 移走)下一帧夺回;首帧由此获得
    // 焦点(`request_focus` 经 `id_next_frame` 于下一 pass 生效)。
    if !ui.ctx().memory(|memory| memory.has_focus(input_id())) {
        ui.ctx()
            .memory_mut(|memory| memory.request_focus(input_id()));
    }

    let window = crate::ui::workbench::centered_dialog(ui, "快速打开")
        .collapsible(false)
        .frame(crate::ui::workbench::dialog_frame(ui))
        // COMMON 尺寸铁律:宽度定死、高度封顶自适应,列表限高滚动不撑窗
        .fixed_size(egui::vec2(WINDOW_W, WINDOW_H))
        .show(ui.ctx(), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.query)
                    .id(input_id())
                    .event_filter(QUICK_OPEN_EVENT_FILTER)
                    .return_key(None::<egui::KeyboardShortcut>)
                    .hint_text("文件名或命令…")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(tokens::SPACE_SM);
            egui::ScrollArea::vertical()
                .id_salt("quick-open-results")
                .max_height(LIST_MAX_H)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if state.root.is_none() {
                        ui.weak("未选择文件树根目录:文件组为空,可继续执行命令");
                    }
                    if rows.is_empty() {
                        ui.weak("无匹配");
                    }
                    let mut file_header = false;
                    let mut cmd_header = false;
                    for (position, row) in rows.iter().enumerate() {
                        let selected = position == state.selected;
                        let response = match row {
                            Row::File(index) => {
                                if !file_header {
                                    ui.strong("文件");
                                    file_header = true;
                                }
                                ui.selectable_label(
                                    selected,
                                    state.files[*index].display().to_string(),
                                )
                            }
                            Row::Cmd(cmd) => {
                                if !cmd_header {
                                    ui.strong("命令");
                                    cmd_header = true;
                                }
                                ui.horizontal(|ui| {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(tokens::ICON, tokens::ICON),
                                        egui::Sense::hover(),
                                    );
                                    cmd.icon().draw(
                                        ui.painter(),
                                        rect.center(),
                                        tokens::ICON_SM,
                                        ui.visuals().text_color(),
                                    );
                                    ui.selectable_label(selected, cmd.label())
                                })
                                .inner
                            }
                        };
                        // 选中行滚动进视野:键盘跨组移动时不被列表高度藏住
                        if selected {
                            response.scroll_to_me(None);
                        }
                        if response.clicked() {
                            activate = Some(*row);
                        }
                    }
                });
            if state.truncated {
                ui.add_space(tokens::SPACE_XS);
                ui.weak(format!(
                    "文件树条目达上限({}),仅搜索已列出的条目",
                    latermd_search::MAX_LIST_ENTRIES
                ));
            }
        });
    // Enter 与点击行汇入同一条执行路径。
    match activate {
        Some(Row::File(index)) => {
            if let Some(root) = state.root.as_deref() {
                outbox.push(Message::FileSelected(root.join(&state.files[index])));
                outbox.push(Message::ToggleQuickOpen);
            }
        }
        Some(Row::Cmd(cmd)) => {
            outbox.push(cmd.message());
            // QuickOpen 自身的消息就是这条翻转,再补一条会把「关」翻回
            // 「开」(两条消息同帧先后归约)
            if cmd != Command::QuickOpen {
                outbox.push(Message::ToggleQuickOpen);
            }
        }
        None => {}
    }
    window
        .map(|window| window.response)
        .expect("浮层必然绘制(无 hidden 条件)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LaterMdApp;
    use egui::{Event, Modifiers, RawInput, Rect};

    /// 进程内唯一的临时文档库;测试自删。
    fn vault(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("latermd-quickopen-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// 清掉临时文档库(配置目录是库内 `_settings/`,一并带走)。
    fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(root);
    }

    fn bare_key(key: egui::Key) -> Vec<Event> {
        [true, false]
            .into_iter()
            .map(|pressed| Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            })
            .collect()
    }

    /// 一帧完整 eframe 顺序(先 `reduce` 后 `draw`)。与 `ui::layout` 测试
    /// 同一手法:面板的键盘处理发生在 `draw`,消息要下一帧 `reduce` 才
    /// 落地,断言前必须再跑一帧。
    fn frame(app: &mut LaterMdApp, ctx: &egui::Context, events: Vec<Event>) {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let output = ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |ui| {
                app.reduce(ui.ctx());
                app.draw(ui);
            },
        );
        output.drop_without_applying_deltas();
    }

    /// 打开浮层并跑完 sizing 帧(浮窗/滚动区前几帧不参与命中,与
    /// emoji_panel 测试同口径),返回 app。配置目录注入库内 `_settings/`
    /// (帧末落盘不碰真实平台配置,与 find 测试同款;快照先于任何帧拍下,
    /// 且非 md 会被快照过滤,不影响文件组)。
    fn opened_panel(root: &Path) -> (LaterMdApp, egui::Context, PathBuf) {
        let mut app = LaterMdApp::default();
        app.state.settings_dir = Some(root.join("_settings"));
        app.state.file_tree.set_root(root.to_path_buf());
        app.state.apply(Message::ToggleQuickOpen);
        let ctx = egui::Context::default();
        for _ in 0..5 {
            frame(&mut app, &ctx, Vec::new());
        }
        (app, ctx, root.to_path_buf())
    }

    /// 开面板快照:只收 Markdown(相对路径,字母序)、记截断标志;无根时
    /// 文件组为空、浮层照常打开。
    #[test]
    fn open_snapshot_keeps_markdown_relative_sorted() {
        let root = vault("snapshot");
        touch(&root, "zz_note.md", "z");
        touch(&root, "aa.md", "a");
        touch(&root, "pic.png", "p");
        touch(&root, "sub/deep.md", "d");
        touch(&root, "sub/raw.txt", "t");
        std::fs::create_dir(root.join("emptydir")).unwrap();

        let mut state = QuickOpenState::default();
        state.open_snapshot(Some(&root));
        assert!(state.open);
        assert_eq!(state.query, "");
        assert_eq!(state.selected, 0);
        assert_eq!(
            state.files,
            vec![
                PathBuf::from("aa.md"),
                PathBuf::from("sub/deep.md"),
                PathBuf::from("zz_note.md")
            ]
        );
        assert!(!state.truncated);

        state.query = "stale".to_owned();
        state.selected = 2;
        state.open_snapshot(None);
        assert!(state.files.is_empty(), "无根:文件组为空");
        assert_eq!(state.query, "", "重开复位查询词");
        assert_eq!(state.selected, 0, "重开复位选中项");
        assert!(!state.truncated);
        cleanup(&root);
    }

    /// 条目上限降级:文档库超出 `MAX_LIST_ENTRIES` 时快照截断并立
    /// `truncated` 标志(浮层照常工作,只搜已列条目)。
    #[test]
    fn open_snapshot_flags_truncation() {
        let root = vault("truncated");
        for index in 0..=latermd_search::MAX_LIST_ENTRIES {
            touch(&root, &format!("f{index:03}.md"), "x");
        }
        let mut state = QuickOpenState::default();
        state.open_snapshot(Some(&root));
        assert!(state.truncated);
        assert_eq!(state.files.len(), latermd_search::MAX_LIST_ENTRIES);
        cleanup(&root);
    }

    /// 可见行:空 query = 文件字母序(快照序)+ 命令 ALL 序,各取前
    /// [`MAX_PER_GROUP`];有 query = 两组各自模糊命中;组内排序与
    /// [`crate::fuzzy::rank`] 一致(分数降序、同分稳定)。
    #[test]
    fn visible_rows_group_order_and_caps() {
        let files: Vec<PathBuf> = ["aaa_note.md", "zzz_note.md", "readme.md"]
            .iter()
            .map(PathBuf::from)
            .collect();
        // 空 query:文件原序(即快照的字母序),命令 = ALL 前 8 条
        let rows = visible_rows("", &files);
        assert_eq!(
            rows.iter().take(3).collect::<Vec<_>>(),
            vec![&Row::File(0), &Row::File(1), &Row::File(2),]
        );
        assert_eq!(rows[3], Row::Cmd(Command::New), "命令组从 ALL 序第一位开始");
        assert_eq!(rows.len(), 3 + MAX_PER_GROUP);

        // 查询命中两组:文件组在前(zzz_note 起始位更靠前、同 run 结构,
        // 与 aaa_note 同分时按稳定序 aaa 在前)
        let rows = visible_rows("note", &files);
        assert_eq!(
            rows,
            vec![Row::File(0), Row::File(1)],
            "note 只命中两个文件,命令标签无命中"
        );

        // 查询只命中命令组
        let rows = visible_rows("禅定", &files);
        assert_eq!(rows, vec![Row::Cmd(Command::ToggleZen)]);

        // 无命中
        assert!(visible_rows("查无此物", &files).is_empty());

        // 组上限:100 个同前缀文件也只列前 MAX_PER_GROUP 条
        let many: Vec<PathBuf> = (0..100)
            .map(|index| PathBuf::from(format!("note{index:02}.md")))
            .collect();
        assert_eq!(
            visible_rows("", &many).len(),
            MAX_PER_GROUP + MAX_PER_GROUP,
            "两组各取前 {MAX_PER_GROUP}"
        );
    }

    /// 主链路(run_ui 无头,完整 reduce→draw 帧):开面板 → 输入查询 →
    /// ↑(循环到尾行)↓↓(跨组往返)→ Enter 打开文件 → 浮层关闭;再开
    /// 已开文件 → 激活既有标签(不开第二个);Esc 关闭且不再产生消息。
    #[test]
    fn open_type_navigate_enter_opens_file_then_escape() {
        let root = vault("flow");
        touch(&root, "aaa_note.md", "# aaa\n");
        touch(&root, "zzz_note.md", "# zzz\n");
        touch(&root, "other.md", "# other\n");
        let (mut app, ctx, root) = opened_panel(&root);

        // 输入查询:focus 由焦点钉落在输入框上,Event::Text 进查询词
        for ch in ["n", "o", "t", "e"] {
            frame(&mut app, &ctx, vec![Event::Text(ch.to_owned())]);
            frame(&mut app, &ctx, Vec::new());
        }
        assert_eq!(app.state.quick_open.query, "note");
        // 命中两个文件(快照字母序:aaa=0, other=1, zzz=2;两处 note 的
        // 起始位相同 → 同分稳定序 aaa 在前),命令无命中
        assert_eq!(
            visible_rows("note", &app.state.quick_open.files),
            vec![Row::File(0), Row::File(2)]
        );
        assert_eq!(app.state.quick_open.selected, 0);

        // ↑ 循环到尾行(0 → 1),↓ 回首(1 → 0)
        frame(&mut app, &ctx, bare_key(egui::Key::ArrowUp));
        frame(&mut app, &ctx, Vec::new());
        assert_eq!(app.state.quick_open.selected, 1, "↑ 从头循环到尾");
        frame(&mut app, &ctx, bare_key(egui::Key::ArrowDown));
        frame(&mut app, &ctx, Vec::new());
        assert_eq!(app.state.quick_open.selected, 0, "↓ 回到首行");

        // Enter:发 FileSelected + ToggleQuickOpen,下一帧归约打开文件并关浮层
        frame(&mut app, &ctx, bare_key(egui::Key::Enter));
        assert_eq!(
            app.outbox,
            vec![
                Message::FileSelected(root.join("aaa_note.md")),
                Message::ToggleQuickOpen,
            ]
        );
        frame(&mut app, &ctx, Vec::new());
        assert!(!app.state.quick_open.open, "Enter 后浮层关闭");
        assert_eq!(
            app.state.tabs.current().document.path.as_deref(),
            Some(root.join("aaa_note.md").as_path()),
            "首行文件被打开并激活"
        );
        assert_eq!(app.state.tabs.current().editor.text(), "# aaa\n");
        let tabs_after_open = app.state.tabs.tabs.len();

        // 重开浮层选同一文件:走 open_path 的激活分支,不开第二个标签
        app.state.apply(Message::ToggleQuickOpen);
        for _ in 0..3 {
            frame(&mut app, &ctx, Vec::new());
        }
        for ch in ["z", "z"] {
            frame(&mut app, &ctx, vec![Event::Text(ch.to_owned())]);
            frame(&mut app, &ctx, Vec::new());
        }
        frame(&mut app, &ctx, bare_key(egui::Key::Enter));
        frame(&mut app, &ctx, Vec::new());
        assert_eq!(
            app.state.tabs.current().document.path.as_deref(),
            Some(root.join("zzz_note.md").as_path())
        );
        assert_eq!(
            app.state.tabs.tabs.len(),
            tabs_after_open + 1,
            "新文件开一个标签;总数 = 首开 1 + 本次 1"
        );
        assert!(!app.state.quick_open.open);

        // Esc:打开状态下关闭,关闭后不再产生消息
        app.state.apply(Message::ToggleQuickOpen);
        frame(&mut app, &ctx, Vec::new());
        frame(&mut app, &ctx, bare_key(egui::Key::Escape));
        assert_eq!(app.outbox, vec![Message::ToggleQuickOpen]);
        frame(&mut app, &ctx, Vec::new());
        assert!(!app.state.quick_open.open);
        frame(&mut app, &ctx, bare_key(egui::Key::Escape));
        assert!(app.outbox.is_empty(), "浮层关着:Esc 不再触发翻转");
        cleanup(&root);
    }

    /// 命令执行链路:输入命中命令 → Enter 发 `cmd.message()` 并关浮层;
    /// 选中 `Command::QuickOpen` 自身只发一条翻转(补第二条会把「关」翻
    /// 回「开」);无匹配行时 Enter 无消息。
    #[test]
    fn command_rows_execute_via_message_and_close() {
        let root = vault("commands");
        let (mut app, ctx, _root) = opened_panel(&root);

        // 查找命令:Enter 后查找条开、浮层关
        for ch in ["查", "找"] {
            frame(&mut app, &ctx, vec![Event::Text(ch.to_owned())]);
            frame(&mut app, &ctx, Vec::new());
        }
        frame(&mut app, &ctx, bare_key(egui::Key::Enter));
        assert_eq!(
            app.outbox,
            vec![Message::FindBarToggled(true), Message::ToggleQuickOpen,]
        );
        frame(&mut app, &ctx, Vec::new());
        assert!(app.state.find.open, "命令经 message() 执行");
        assert!(!app.state.quick_open.open);

        // QuickOpen 自身:Enter 只发一条翻转 → 净效果是关(不是关了又开)
        app.state.apply(Message::ToggleQuickOpen);
        for _ in 0..3 {
            frame(&mut app, &ctx, Vec::new());
        }
        for ch in ["快", "速", "打", "开"] {
            frame(&mut app, &ctx, vec![Event::Text(ch.to_owned())]);
            frame(&mut app, &ctx, Vec::new());
        }
        app.outbox.clear();
        frame(&mut app, &ctx, bare_key(egui::Key::Enter));
        assert_eq!(app.outbox, vec![Message::ToggleQuickOpen], "不补第二条翻转");
        frame(&mut app, &ctx, Vec::new());
        assert!(!app.state.quick_open.open, "净效果:关");

        // 无匹配:Enter 无消息
        app.state.apply(Message::ToggleQuickOpen);
        for _ in 0..3 {
            frame(&mut app, &ctx, Vec::new());
        }
        for ch in ["查", "无", "此", "物"] {
            frame(&mut app, &ctx, vec![Event::Text(ch.to_owned())]);
            frame(&mut app, &ctx, Vec::new());
        }
        app.outbox.clear();
        frame(&mut app, &ctx, bare_key(egui::Key::Enter));
        assert!(app.outbox.is_empty(), "空列表 Enter 是 no-op");
        cleanup(&root);
    }

    /// 尺寸铁律(COMMON):长列表下浮层 rect 恒在视口内、宽恒为
    /// [`WINDOW_W`]、高不超 [`WINDOW_H`] 且**不随文件总数增长**(组上限
    /// + 列表限高把高度钉住);帧间 rect 稳定;首帧聚焦由焦点钉落进输入框。
    #[test]
    fn window_stays_fixed_and_inside_viewport_with_long_list() {
        let root = vault("sizing");
        for index in 0..40 {
            touch(&root, &format!("note{index:02}.md"), "x");
        }
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0));

        let mut state = QuickOpenState::default();
        state.open_snapshot(Some(&root));
        // 空 query:8 文件 + 8 命令 + 两个组标题 ≈ 18 行 > 320px 限高,
        // 滚动区内滚动,浮层不随之长高
        let long = rect_after(&ctx, &screen, &mut state, 6);
        let viewport = ctx.viewport_rect();
        assert!(
            long.width() <= WINDOW_W + 1.0 && long.width() >= WINDOW_W - 1.0,
            "浮层宽恒为 {WINDOW_W}(实测 {})",
            long.width()
        );
        assert!(
            long.height() <= WINDOW_H,
            "浮层高 {} 不超上限 {WINDOW_H}",
            long.height()
        );
        assert!(
            viewport.contains_rect(long),
            "浮层 {long:?} 完整落在视口 {viewport:?} 内"
        );

        // 文件总数翻倍(仍远超组上限 8):rect 逐项一致 —— 高度不随
        // 文件总数增长,只随「可见行数」增长,而可见行被组上限钉死
        state.files.extend((40..80).map(|index| {
            let name = format!("note{index:02}.md");
            PathBuf::from(name)
        }));
        let doubled = rect_after(&ctx, &screen, &mut state, 4);
        assert_eq!(
            (long.min, long.max),
            (doubled.min, doubled.max),
            "文件总数 40 → 80,浮层 rect 不变"
        );

        // 短列表:高度自适应收缩(不空撑),宽度仍钉死
        state.files.truncate(2);
        let short = rect_after(&ctx, &screen, &mut state, 4);
        assert!(
            short.height() < long.height(),
            "短列表高度自适应收缩({} < {})",
            short.height(),
            long.height()
        );
        assert!(
            (short.width() - WINDOW_W).abs() <= 1.0,
            "宽度与列表长度无关,恒 {WINDOW_W}"
        );

        // 首帧聚焦:focus 由面板自己钉进输入框(无外部请求)
        assert!(
            ctx.memory(|memory| memory.has_focus(input_id())),
            "开面板后焦点在查询输入框"
        );
        cleanup(&root);
    }

    /// 连跑 `frames` 帧取浮窗矩形(浮窗与滚动区前几帧是 sizing pass,
    /// 与 emoji_panel 点击测试同口径)。
    fn rect_after(
        ctx: &egui::Context,
        screen: &Rect,
        state: &mut QuickOpenState,
        frames: usize,
    ) -> Rect {
        let mut rect = Rect::NOTHING;
        for _ in 0..frames {
            let output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(*screen),
                    ..Default::default()
                },
                |ui| rect = panel(ui, state, &mut Vec::new()).rect,
            );
            output.drop_without_applying_deltas();
        }
        assert!(rect.is_finite(), "拿到了浮窗矩形");
        rect
    }
}
