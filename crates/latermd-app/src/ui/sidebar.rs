//! 侧边栏:页签栏 + 当前页签内容。文件树(P0 基础版:懒加载 + 截断 +
//! 点击打开 + 当前文件高亮)、全文搜索(P1:去抖 + 流式结果 + 点击跳行)
//! 与大纲(P0 廉价版:点击跳编辑器光标)均接真数据。
//!
//! 文件树渲染自管展开状态(`FileTreeState::expanded`)+ `ui.indent` 缩进,
//! 不用 `CollapsingHeader`(ADR-005 §4.1);行点击只发消息,子项列举在
//! `logic` 归约的 `FileTreeState::ensure_loaded`(懒加载落点)。
//!
//! 搜索面板的去抖:`ui` 把输入变化原地写入 `SearchState` 并发
//! `SearchQueryChanged`,`logic` 归约顺延去抖计时;到点发起在归约侧
//! (`layout.rs` 的 reduce,每帧必跑、不看本面板是否可见——输入后切走
//! 页签也照常搜),本层只把输入与开关写进状态。
//!
//! Git 页(P2)与文件树角标:数据全部来自 `GitPanelState` 的只读快照
//! (刷新时机在归约侧),本层零 git 调用;回滚按钮只发消息,checkout
//! 在确认模态之后(见 `ui::layout`)。

use crate::command::Command;
use crate::filetree::{DirChildren, FileTreeState, TreeEntry};
use crate::git_panel::GitPanelState;
use crate::keymap::Keymap;
use crate::search::{SearchResult, SearchState, SearchStatus, MAX_HITS};
use crate::state::{Message, SidebarTab};
use latermd_git::{CommitInfo, FileStatus, StatusKind};
use latermd_md::OutlineItem;

use eframe::egui;
use std::path::Path;

/// 大纲层级每深一级的缩进宽度(px)。
const OUTLINE_INDENT: f32 = 14.0;

/// 搜索结果行摘要的最大字符数(命中行可能是长段落,截断保列表可读)。
const SNIPPET_MAX_CHARS: usize = 120;

/// 大纲页的只读数据:标题快照 + 编辑器当前光标(两者分属 `State` 的
/// `preview` 与 `cursor` 字段,侧边栏只读;打包纯粹为收敛 `ui` 的参数)。
pub struct OutlineView<'a> {
    pub items: &'a [OutlineItem],
    pub cursor_byte: Option<usize>,
}

/// 绘制左栏(docs/ui-shell-redesign.md §5,D3 已拍板;2026-09-27 修订,
/// 见 decisions-pending #31):顶段高频文件动作 → 次段四行视图导航 →
/// 中段视图内容(吃掉剩余高度)。原「底段设置」行已迁至标题栏右端齿轮。
///
/// 中段「吃掉剩余」由 `ScrollArea` 的 `max_height = available` 承担,不上
/// 嵌套 `Panel`(会造成 widget id 与 z-order 意外),也不用 `bottom_up`
/// (左右 snap 会让各段的阅读顺序与代码顺序相反)。
///
/// `active_tab`、`search` 与 `outbox` 是从 `App` 上解构出的不相交借用,使本
/// 函数可以与 `show_collapsible` 原地持有的 `&mut bool` 并存(见
/// `layout.rs`)。`file_tree`、`git`、`outline` 与 `current_file` 只读:交互
/// 全部经由消息归约;`search` 的输入文本/开关由本层原地改写(TextEdit/
/// checkbox 控件的 `&mut` 要求,同编辑器缓冲的例外),取消与发起都在归约。
// 参数各属不同页签的状态,打包成结构只是造出人为聚合(vendored 层同款
// allow 先例见 egui_markdown/src/label.rs)
#[allow(clippy::too_many_arguments)]
pub fn ui(
    panel: &mut egui::Ui,
    active_tab: &mut SidebarTab,
    file_tree: &FileTreeState,
    current_file: Option<&Path>,
    outline: OutlineView<'_>,
    search: &mut SearchState,
    git: &GitPanelState,
    outbox: &mut Vec<Message>,
) -> SidebarBands {
    let left = panel.max_rect().left();
    let right = panel.max_rect().right();
    let band = |top: f32, bottom: f32| {
        egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom))
    };

    let y0 = panel.cursor().top();
    top_actions(panel, outbox);
    let y1 = panel.cursor().top();
    view_nav(panel, *active_tab, outbox);
    let y2 = panel.cursor().top();
    panel.separator();
    let y3 = panel.cursor().top();
    egui::ScrollArea::vertical()
        .id_salt("nav-body")
        .auto_shrink([false, false])
        .max_height(panel.available_height())
        .show(panel, |ui| match *active_tab {
            SidebarTab::Files => files_panel(ui, file_tree, current_file, git, outbox),
            SidebarTab::Search => search_panel(ui, search, file_tree.root.as_deref(), outbox),
            SidebarTab::Outline => outline_panel(ui, outline, outbox),
            SidebarTab::Git => git_panel(ui, git, outbox),
        });
    let y4 = panel.cursor().top();

    SidebarBands {
        top: band(y0, y1),
        nav: band(y1, y2),
        body: band(y3, y4),
    }
}

/// 左栏三段各自的竖直区间,自上而下互不重叠。
///
/// 由 [`ui`] 顺手测出来返回:`nav_row` 是手绘 widget(无可读名字),要算
/// 「第 N 行的 y」只能靠 [`SidebarTab::ALL`] 的顺序 + `NAV_ROW_H` 自己推。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarBands {
    /// 顶段:高频文件动作。
    pub top: egui::Rect,
    /// 次段:四行视图导航(每行 `NAV_ROW_H` 高)。
    pub nav: egui::Rect,
    /// 中段:当前视图内容,吃掉剩余高度(至左栏底部)。
    pub body: egui::Rect,
}

/// 单条视图导航行的**竖直中心 y**(相对传进来的 `panel`)。
///
/// 给无头测试定位用:行本身是 `allocate_exact_size` 的手绘 widget,没有可
/// 从外部读的名字;而顺序是 `SidebarTab::ALL`、行高是 `NAV_ROW_H`,与绘制
/// 同源地算出来即可,不必把矩形一路传出去。
// 只有无头测试用;生产路径靠 `SidebarBands` 定位。
#[cfg(test)]
pub(crate) fn nav_row_center_y(top: f32, tab: SidebarTab) -> f32 {
    let index = SidebarTab::ALL
        .iter()
        .position(|candidate| *candidate == tab)
        .expect("tab 必在 ALL 里");
    top + crate::ui::tokens::NAV_ROW_H * (index as f32 + 0.5)
}

/// 左栏高频文件动作(docs/ui-shell-redesign.md §5 顶段;2026-09-27 起
/// 是文件动作的**唯一常驻按钮入口** —— 编辑器区顶部的文件工具栏退役,
/// actions 收口到本栏,decisions-pending #32)。
///
/// 全集 = [`Command::FILE`] + `ExportHtml`;AI 与视图开关不在此列
/// (AI 在菜单栏「AI」,视图开关在标题栏按钮)。菜单栏的菜单项一个不删
/// —— 这里是快捷入口,不是唯一入口(ui-polish §1.2「菜单栏负责全部」)。
/// `horizontal_wrapped`:左栏拖到 180px 下限时换行而不是溢出裁切(R4)。
fn top_actions(panel: &mut egui::Ui, outbox: &mut Vec<Message>) {
    top_actions_with_probe(panel, outbox, None);
}

/// 同 [`top_actions`],额外把每个按钮的 `(命令, 矩形)` 交给 `probe`。
///
/// 无头测试量按钮位置用(与 `ui::format_bar::ui_with_probe` 同款手法):
/// 按钮坐标由 `horizontal_wrapped` 的换行演算决定,手搓必然与真实帧错位。
fn top_actions_with_probe(
    panel: &mut egui::Ui,
    outbox: &mut Vec<Message>,
    probe: Option<&mut dyn FnMut(Command, egui::Rect)>,
) {
    let mut probe = probe;
    panel.horizontal_wrapped(|ui| {
        for cmd in Command::FILE.iter().copied().chain([Command::ExportHtml]) {
            let response =
                crate::ui::icons::icon_button(ui, cmd.icon(), &tooltip_of(cmd, &Keymap::builtin()));
            if let Some(probe) = probe.as_deref_mut() {
                probe(cmd, response.rect);
            }
            if response.clicked() {
                outbox.push(cmd.message());
            }
        }
    });
}

/// 按钮悬浮提示:命令名 + 出厂键位(键位可改,这里取的是出厂默认,够
/// 指路就够;改键在设置「快捷键」页)。
fn tooltip_of(cmd: Command, keymap: &Keymap) -> String {
    match keymap.get(cmd) {
        Some(shortcut) => format!("{}({})", cmd.label(), shortcut.platform_text()),
        None => cmd.label().to_owned(),
    }
}

/// 左栏次段的视图导航:四行竖排,**整行选中态** —— selected_bg 底 +
/// 左侧 2px 强调色竖条(照抄 tabs 的页签选中态,不用重boBox 再辨证)。
///
/// 点击只发 [`Message::SidebarTabChanged`],切换本身在归约。
fn view_nav(panel: &mut egui::Ui, active: SidebarTab, outbox: &mut Vec<Message>) {
    for tab in SidebarTab::ALL {
        let response = nav_row(panel, tab, tab == active);
        if response.clicked() {
            outbox.push(Message::SidebarTabChanged(tab));
        }
    }
}

/// 单条导航行:图标 + 文字占满整行宽。返回响应以便点击测试定位。
fn nav_row(ui: &mut egui::Ui, tab: SidebarTab, selected: bool) -> egui::Response {
    icon_label_row(
        ui,
        tab.icon(),
        tab.label(),
        selected,
        crate::ui::tokens::NAV_ROW_H,
    )
}

/// 图标 + 文字的整行按钮,高度可调(左栏次段视图导航用)。
///
/// 手绘而非 `Button` 是为了「整行选中态」:浅色底打满可用宽 + 左侧 2px
/// 强调色竖条(ui-polish 页签选中态的同款口径)。`Sense::click` 打在
/// `allocate_exact_size` 上而非某个子控件,因此整行任意位置都能点 —— 这
/// 也是它比起 `horizontal` 容器响应的差别:后者只有不可交互的 `label`,
/// 压根收不到点击。
fn icon_label_row(
    ui: &mut egui::Ui,
    icon: crate::ui::icons::Icon,
    label: &str,
    selected: bool,
    height: f32,
) -> egui::Response {
    let size = egui::vec2(ui.available_width().max(0.0), height);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let visuals = ui.visuals();
        // 整行选中态:selected_bg + 左侧 2px 竖条(与 tabs 的页签选中同款,
        // `widgets.hovered` 在浅色下恰好等于侧栏底色,选中行会整体消失);
        // hover 用内建 hover 底
        if selected {
            let accent = crate::ui::tokens::accent(ui);
            let selected_bg = crate::theme::shell_tokens(visuals.dark_mode).selected_bg;
            painter.rect_filled(rect, 0.0, selected_bg);
            painter.rect_filled(
                egui::Rect::from_min_size(
                    rect.left_top(),
                    egui::vec2(crate::ui::tokens::NAV_BAR_W, height),
                ),
                0.0,
                accent,
            );
        } else if response.hovered() {
            painter.rect_filled(rect, 0.0, visuals.widgets.hovered.bg_fill);
        }
        let color = if selected {
            crate::ui::tokens::accent(ui)
        } else {
            visuals.text_color()
        };
        let text_pos = egui::pos2(
            rect.left()
                + crate::ui::tokens::SPACE_SM
                + crate::ui::tokens::ICON_SM
                + crate::ui::tokens::SPACE_XS,
            rect.center().y,
        );
        icon.draw(
            painter,
            egui::pos2(
                rect.left() + crate::ui::tokens::SPACE_SM + crate::ui::tokens::ICON_SM / 2.0,
                rect.center().y,
            ),
            crate::ui::tokens::ICON_SM,
            color,
        );
        painter.text(
            text_pos,
            egui::Align2::LEFT_CENTER,
            label,
            egui::TextStyle::Button.resolve(ui.style()),
            color,
        );
    }
    response
}

/// 大纲列表。数据来自 [`crate::state::PreviewState`] 的快照,文档变化时随
/// 预览同一时机重算,空闲帧零开销。
fn outline_panel(panel: &mut egui::Ui, outline: OutlineView<'_>, outbox: &mut Vec<Message>) {
    let OutlineView { items, cursor_byte } = outline;
    egui::ScrollArea::vertical()
        .id_salt("outline-scroll")
        // 不收缩宽度,让长标题换行而不是把面板撑宽
        .auto_shrink([false, false])
        .show(panel, |ui| {
            if items.is_empty() {
                ui.weak("无标题:文档里还没有 Markdown 标题");
            }
            let active = active_index(items, cursor_byte);
            for (index, item) in items.iter().enumerate() {
                outline_row(ui, item, active == Some(index), outbox);
            }
        });
}

/// 光标所在的当前小节:起始位置不晚于光标的最后一个标题。
///
/// 光标落在正文段落里时高亮其所属小节(与常见编辑器大纲一致);无光标
/// 信息或光标在首个标题之前则不高亮。
fn active_index(outline: &[OutlineItem], cursor_byte: Option<usize>) -> Option<usize> {
    cursor_byte.and_then(|cursor| outline.iter().rposition(|item| item.span.start <= cursor))
}

/// 单条大纲:按层级缩进的 SelectableLabel,点击发消息(跳转在 `logic`
/// 归约 + `ui::editor` 应用)。返回标签响应,独立成函数便于点击测试定位。
fn outline_row(
    ui: &mut egui::Ui,
    item: &OutlineItem,
    selected: bool,
    outbox: &mut Vec<Message>,
) -> egui::Response {
    let response = ui
        .horizontal(|ui| {
            ui.add_space(OUTLINE_INDENT * item.level.saturating_sub(1) as f32);
            ui.selectable_label(selected, &item.text)
        })
        .inner;
    if response.clicked() {
        outbox.push(Message::OutlineItemClicked(item.span.clone()));
    }
    response
}

/// Search 页:输入行(正则 + 大小写开关)+ 状态行 + 结果列表。
///
/// 无根目录时只显示引导(搜索范围复用文件树的根,不另设第二份)。
fn search_panel(
    panel: &mut egui::Ui,
    search: &mut SearchState,
    root: Option<&Path>,
    outbox: &mut Vec<Message>,
) {
    let Some(root) = root else {
        panel.weak("搜索需要一个根目录:先在「文件」页选择");
        return;
    };

    panel.horizontal(|ui| {
        let edited = egui::TextEdit::singleline(&mut search.query)
            .id_salt("search-input")
            .hint_text("正则表达式…")
            .desired_width(f32::INFINITY)
            .show(ui)
            .response
            .changed();
        let toggled = ui.checkbox(&mut search.case_insensitive, "Aa").changed();
        if edited || toggled {
            outbox.push(Message::SearchQueryChanged);
        }
    });
    match &search.status {
        SearchStatus::Running => {
            panel.weak(format!("搜索中…(已 {} 条)", search.hits.len()));
        }
        SearchStatus::Finished if search.hits.is_empty() => {
            panel.weak("无命中");
        }
        SearchStatus::Invalid(msg) => {
            panel.weak(format!("⚠ {msg}"));
        }
        SearchStatus::Idle | SearchStatus::Finished => {}
    }

    egui::ScrollArea::vertical()
        .id_salt("search-results-scroll")
        .auto_shrink([false, false])
        .show(panel, |ui| {
            if search.hits.is_empty() && matches!(search.status, SearchStatus::Idle) {
                ui.weak("输入搜索词,回车不必按——300ms 停顿后自动搜索");
            }
            for hit in &search.hits {
                search_row(ui, hit, root, outbox);
            }
            if search.truncated {
                ui.weak(format!(
                    "已达 {} 条上限,后续命中未显示(输入更精确的模式收窄)",
                    MAX_HITS
                ));
            }
        });
}

/// 单条结果:第一行「相对根的路径:行号」,第二行命中行摘要;整块一个
/// SelectableLabel,点击发消息(打开 + 跳行在 `logic` 归约)。
/// 返回行响应,独立成函数便于点击测试定位。
fn search_row(
    ui: &mut egui::Ui,
    hit: &SearchResult,
    root: &Path,
    outbox: &mut Vec<Message>,
) -> egui::Response {
    let relative = hit.path.strip_prefix(root).unwrap_or(hit.path.as_path());
    let text = format!(
        "{}:{}\n{}",
        relative.display(),
        hit.line_no,
        snippet(&hit.line_text)
    );
    let response = ui.selectable_label(false, text);
    if response.clicked() {
        outbox.push(Message::SearchResultClicked(hit.path.clone(), hit.line_no));
    }
    response
}

/// 命中行摘要:按字符截断(不切断多字节序列),超长加省略号。
fn snippet(line: &str) -> String {
    if line.chars().count() <= SNIPPET_MAX_CHARS {
        line.to_owned()
    } else {
        let cut: String = line.chars().take(SNIPPET_MAX_CHARS).collect();
        format!("{cut}…")
    }
}

/// Files 页:顶部根目录选择行 + 懒加载目录树。
fn files_panel(
    panel: &mut egui::Ui,
    tree: &FileTreeState,
    current_file: Option<&Path>,
    git: &GitPanelState,
    outbox: &mut Vec<Message>,
) {
    // 根目录行:最近列表下拉(有历史才有)+ 选新目录按钮
    let root_hover = tree.root.as_deref().map(|path| path.display().to_string());
    panel.horizontal(|ui| {
        let selected = root_label(tree);
        if tree.recents.is_empty() {
            let response = ui.weak(&selected);
            if let Some(hover) = &root_hover {
                response.on_hover_text(hover);
            }
        } else {
            let response = egui::ComboBox::from_id_salt("file-tree-roots")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for dir in &tree.recents {
                        if ui
                            .selectable_label(
                                tree.root.as_deref() == Some(dir.as_path()),
                                dir.display().to_string(),
                            )
                            .clicked()
                        {
                            // ComboBox 默认 CloseOnClick:点击项后弹层自动收起
                            outbox.push(Message::FileTreeRootSelected(dir.clone()));
                        }
                    }
                })
                .response;
            if let Some(hover) = &root_hover {
                response.on_hover_text(hover);
            }
        }
        if ui.small_button("选择…").clicked() {
            outbox.push(Message::FileTreeRootPick);
        }
    });

    egui::ScrollArea::vertical()
        .id_salt("file-tree-scroll")
        // 不收缩宽度,让长文件名换行而不是把面板撑宽
        .auto_shrink([false, false])
        .show(panel, |ui| match tree.root.as_deref() {
            Some(root) => match tree.children.get(root) {
                Some(children) => {
                    tree_rows(ui, tree, children, current_file, git, outbox);
                }
                None => {
                    ui.weak("载入中…");
                }
            },
            None => {
                ui.weak("选择一个目录作为文件树根(点上方「选择…」)");
            }
        });
}

/// 根目录行的显示名。
fn root_label(tree: &FileTreeState) -> String {
    tree.root
        .as_deref()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "未选择根目录".to_owned())
}

/// 一组子项的各行 + 截断提示行。
fn tree_rows(
    ui: &mut egui::Ui,
    tree: &FileTreeState,
    children: &DirChildren,
    current_file: Option<&Path>,
    git: &GitPanelState,
    outbox: &mut Vec<Message>,
) {
    for entry in &children.entries {
        tree_row(ui, tree, entry, current_file, git, outbox);
    }
    if children.truncated > 0 {
        ui.weak(format!("…还有 {} 项未显示", children.truncated));
    }
}

/// 单行:目录点击翻转展开(箭头随状态),文件点击发打开消息;当前文档
/// 高亮,文件名右侧跟 Git 状态角标(P2)。展开的目录子树用 `ui.indent`
/// 缩进,path 作 id 源保证跨帧稳定。返回行响应,独立成函数便于点击测试
/// 定位。
fn tree_row(
    ui: &mut egui::Ui,
    tree: &FileTreeState,
    entry: &TreeEntry,
    current_file: Option<&Path>,
    git: &GitPanelState,
    outbox: &mut Vec<Message>,
) -> egui::Response {
    let open = tree.expanded.get(&entry.path).copied().unwrap_or(false);
    // 展开箭头不用文本字符:U+25B8/U+25BE 在 Inter 与出厂字体链上均无
    // 字形,目录行名字前渲染成方框(用户实测)。行文本统一以两个空格缩进
    // 占位(空格任何字体都有,目录/文件天然对齐),目录行的三角画在占位区,
    // 纯绘制不依赖字体——与 #31 行号槽同思路。
    let selected = !entry.is_dir && current_file == Some(entry.path.as_path());
    let badge = if entry.is_dir {
        None
    } else {
        git.badge_for(&entry.path)
    };
    let response = badged_row_label(ui, selected, format!("  {}", entry.name), badge);
    if entry.is_dir {
        paint_tree_arrow(ui, &response, open);
    }
    if response.clicked() {
        if entry.is_dir {
            outbox.push(Message::FileTreeToggled(entry.path.clone()));
        } else {
            outbox.push(Message::FileSelected(entry.path.clone()));
        }
    }
    if entry.is_dir && open {
        ui.indent(entry.path.clone(), |ui| {
            match tree.children.get(&entry.path) {
                Some(children) => {
                    tree_rows(ui, tree, children, current_file, git, outbox);
                }
                None => {
                    ui.weak("载入中…");
                }
            }
        });
    }
    response
}

/// 目录行的展开三角:画在行首两个空格占位区的中点上,收起朝右、展开
/// 朝下。尺寸基准是空格 advance(随正文字号缩放,不随平台字体走样),
/// 颜色取弱前景(与行号槽同源);纯 `Shape::convex_polygon`,零字形依赖。
fn paint_tree_arrow(ui: &egui::Ui, row: &egui::Response, open: bool) {
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let space_w = ui.fonts_mut(|f| {
        f.layout_no_wrap(" ".to_owned(), font_id, egui::Color32::WHITE)
            .rect
            .width()
    });
    let center = egui::pos2(row.rect.left() + space_w, row.rect.center().y);
    let r = space_w * 0.62;
    ui.painter().add(egui::Shape::convex_polygon(
        arrow_vertices(center, r, open),
        ui.visuals().widgets.noninteractive.fg_stroke.color,
        egui::Stroke::NONE,
    ));
}

/// 三角顶点:展开朝下、收起朝右。独立成纯函数,无头单测锁两个朝向的
/// 几何约定(箭头从无字形的文本字符改为纯绘制后的防回归)。
fn arrow_vertices(center: egui::Pos2, r: f32, open: bool) -> Vec<egui::Pos2> {
    if open {
        vec![
            center + egui::vec2(-r, -r * 0.8),
            center + egui::vec2(r, -r * 0.8),
            center + egui::vec2(0.0, r * 0.9),
        ]
    } else {
        vec![
            center + egui::vec2(-r * 0.8, -r),
            center + egui::vec2(-r * 0.8, r),
            center + egui::vec2(r * 0.9, 0.0),
        ]
    }
}

/// Git 页:降级提示,或「改动列表 + 选中文件 diff + 回滚按钮 + 历史
/// 折叠区」。数据是 `GitPanelState` 的只读快照(刷新在归约侧),交互全部
/// 经由消息;回滚按钮只发 [`Message::GitCheckoutRequested`],确认模态在
/// `ui::layout`。
fn git_panel(panel: &mut egui::Ui, git: &GitPanelState, outbox: &mut Vec<Message>) {
    if let Some(error) = &git.error {
        panel.weak(format!("⚠ {error}"));
        return;
    }

    panel.label(format!("改动({})", git.entries.len()));
    egui::ScrollArea::vertical()
        .id_salt("git-status-scroll")
        .auto_shrink([false, false])
        .max_height(160.0)
        .show(panel, |ui| {
            if git.entries.is_empty() {
                ui.weak("工作区干净,没有未提交的改动");
            }
            for entry in &git.entries {
                git_status_row(ui, git, entry, outbox);
            }
            if git.truncated > 0 {
                ui.weak(format!("…还有 {} 项未显示", git.truncated));
            }
        });

    if let Some(selected) = git.selected.as_deref() {
        panel.add_space(4.0);
        panel.monospace(selected);
        if git.diff.is_empty() {
            panel.weak("无文本改动");
        } else {
            diff_view(panel, &git.diff);
        }
        if panel.button("回滚此文件…").clicked() {
            outbox.push(Message::GitCheckoutRequested(selected.to_owned()));
        }
    }

    panel.add_space(4.0);
    egui::CollapsingHeader::new(format!("历史({})", git.commits.len()))
        .id_salt("git-history")
        .default_open(true)
        .show(panel, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("git-log-scroll")
                .auto_shrink([false, false])
                .max_height(220.0)
                .show(ui, |ui| {
                    if git.commits.is_empty() {
                        ui.weak("没有提交历史");
                    }
                    for commit in &git.commits {
                        commit_row(ui, commit, git.fetched_at);
                    }
                });
        });
}

/// 单条改动:彩色状态字母 + 相对仓库根路径;点击选中(高亮当前选中项)。
/// 返回行响应,独立成函数便于点击测试定位。
fn git_status_row(
    ui: &mut egui::Ui,
    git: &GitPanelState,
    entry: &FileStatus,
    outbox: &mut Vec<Message>,
) -> egui::Response {
    let selected = git.selected.as_deref() == Some(entry.path.as_str());
    let response = badged_row_label(ui, selected, entry.path.clone(), Some(entry.code));
    if response.clicked() {
        outbox.push(Message::GitFileSelected(entry.path.clone()));
    }
    response
}

/// 单条历史:短 hash + subject + 相对时间。P2 点击不跳转(工作区检出到
/// 历史版本是 P3 以后的命题),纯展示。
fn commit_row(ui: &mut egui::Ui, commit: &CommitInfo, now_epoch: i64) {
    ui.horizontal(|ui| {
        ui.monospace(egui::RichText::new(&commit.short_hash).weak());
        ui.label(&commit.subject);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.weak(relative_time(commit.time, now_epoch));
        });
    });
}

/// 只读等宽 diff 视图:+ 行绿、- 行红、`@@` 行蓝、文件头弱化;横向不
/// 折行(`ScrollArea::both`,长行滚动)。文本已由 latermd-git 截断在
/// ~64KB,行数有界。
fn diff_view(panel: &mut egui::Ui, diff: &str) {
    egui::ScrollArea::both()
        .id_salt("git-diff-scroll")
        .auto_shrink([false, false])
        .max_height(200.0)
        .show(panel, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for line in diff.lines() {
                let color = if line.starts_with('+') {
                    DIFF_ADDED
                } else if line.starts_with('-') {
                    DIFF_REMOVED
                } else if line.starts_with('@') {
                    DIFF_HUNK
                } else {
                    ui.visuals().weak_text_color()
                };
                ui.monospace(egui::RichText::new(line).color(color));
            }
        });
}

/// 状态角标配色:M 黄(改)、A 绿(增)、U/D 红(冲突/删)、? 灰(未跟踪)。
/// 固定中间亮度,深浅主题下均可读。
fn status_color(kind: StatusKind) -> egui::Color32 {
    match kind {
        StatusKind::Modified => egui::Color32::from_rgb(235, 180, 60),
        StatusKind::Added => egui::Color32::from_rgb(96, 200, 120),
        StatusKind::Unmerged | StatusKind::Deleted => egui::Color32::from_rgb(235, 96, 96),
        StatusKind::Untracked => egui::Color32::from_rgb(150, 158, 168),
    }
}

/// diff 行的三档语义色(+/-/@@),文件头与其余走弱化前景。
const DIFF_ADDED: egui::Color32 = egui::Color32::from_rgb(96, 200, 120);
const DIFF_REMOVED: egui::Color32 = egui::Color32::from_rgb(235, 96, 96);
const DIFF_HUNK: egui::Color32 = egui::Color32::from_rgb(96, 150, 235);

/// 「正文 + 彩色单字母角标」的可选中行,文件树文件行与 Git 页改动行共用
/// (egui 0.36 的 `IntoAtoms` 元组语法:正文正常前景,角标按状态着色)。
fn badged_row_label(
    ui: &mut egui::Ui,
    selected: bool,
    text: String,
    badge: Option<StatusKind>,
) -> egui::Response {
    match badge {
        Some(kind) => ui.selectable_label(
            selected,
            (
                text,
                egui::RichText::new(format!(" {kind}")).color(status_color(kind)),
            ),
        ),
        None => ui.selectable_label(selected, text),
    }
}

/// 提交时间 →「x 分钟前」式相对时间(与刷新时刻的差,分钟级精度)。
/// 单位逐级放大:刚刚/分钟/小时/天/月/年;时钟偏移的负差按「刚刚」。
fn relative_time(then: i64, now: i64) -> String {
    let delta = (now - then).max(0);
    let minutes = delta / 60;
    let hours = delta / 3600;
    let days = delta / 86400;
    if minutes < 1 {
        "刚刚".to_owned()
    } else if hours < 1 {
        format!("{minutes} 分钟前")
    } else if days < 1 {
        format!("{hours} 小时前")
    } else if days <= 30 {
        format!("{days} 天前")
    } else if days <= 365 {
        format!("{} 个月前", days / 30)
    } else {
        format!("{} 年前", days / 365)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filetree::{DirChildren, TreeEntry};
    use egui::{Event, PointerButton, RawInput, Rect};
    use std::cell::Cell;
    use std::path::PathBuf;

    fn item(level: u8, text: &str, start: usize) -> OutlineItem {
        OutlineItem {
            level,
            text: text.to_owned(),
            span: start..start + text.len(),
        }
    }

    /// 测试用最小树:根下「docs」目录(展开,子项已缓存)+「note.md」文件
    /// + 截断计数,覆盖三种行的渲染路径。
    fn sample_tree() -> (FileTreeState, PathBuf) {
        let root = PathBuf::from("/vault");
        let docs = root.join("docs");
        let mut tree = FileTreeState {
            root: Some(root.clone()),
            ..FileTreeState::default()
        };
        tree.expanded.insert(docs.clone(), true);
        tree.children.insert(docs.clone(), DirChildren::default());
        tree.children.insert(
            root.clone(),
            DirChildren {
                entries: vec![
                    TreeEntry {
                        path: docs,
                        name: "docs".to_owned(),
                        is_dir: true,
                    },
                    TreeEntry {
                        path: root.join("note.md"),
                        name: "note.md".to_owned(),
                        is_dir: false,
                    },
                ],
                truncated: 4,
            },
        );
        (tree, root)
    }

    /// 展开三角几何:展开朝下、收起朝右。行首箭头原是文本字符
    /// (U+25B8/U+25BE 在 Inter 与出厂字体链无字形,目录行渲染成方框),
    /// 改为纯绘制后此测试锁住两个朝向的顶点约定。
    #[test]
    fn tree_arrow_vertices_point_in_direction() {
        let c = egui::pos2(10.0, 10.0);
        let r = 2.0;
        let open = arrow_vertices(c, r, true);
        let closed = arrow_vertices(c, r, false);
        assert!(
            open.iter().any(|p| p.y > c.y) && open.iter().all(|p| p.y <= c.y + r * 0.9 + 1e-6),
            "展开三角下指:越过中心下方的顶点存在且不越过顶点"
        );
        assert!(
            closed.iter().any(|p| p.x > c.x) && closed.iter().all(|p| p.x <= c.x + r * 0.9 + 1e-6),
            "收起三角右指:越过中心右侧的顶点存在且不越过顶点"
        );
    }

    /// 点击树行:文件行发打开消息、目录行发翻转展开消息;仅渲染不产生
    /// 消息。带 Git 角标的文件行走同一渲染路径(角标着色无断言,点击行为
    /// 与无角标一致是本测试的对象)。
    /// (ComboBox 弹层交互不在无头测试覆盖范围,根目录选择走 state::tests。)
    #[test]
    fn clicking_tree_rows_sends_messages() {
        let (tree, root) = sample_tree();
        let file = root.join("note.md");
        let docs = root.join("docs");
        let ctx = egui::Context::default();
        let mut outbox = Vec::new();
        let file_rect = Cell::new(Rect::NOTHING);
        let dir_rect = Cell::new(Rect::NOTHING);
        let mut git = GitPanelState::default();
        git.badges.insert(file.clone(), StatusKind::Modified);

        // 第一帧只渲染,借 Cell 拿到两行的屏幕位置
        ctx.run_ui(RawInput::default(), |ui| {
            let children = tree.children.get(&root).unwrap();
            dir_rect.set(tree_row(ui, &tree, &children.entries[0], None, &git, &mut outbox).rect);
            file_rect.set(tree_row(ui, &tree, &children.entries[1], None, &git, &mut outbox).rect);
        })
        .drop_without_applying_deltas();
        assert!(outbox.is_empty(), "仅渲染不产生消息");

        let click_at = |rect: &Cell<Rect>| {
            let center = rect.get().center();
            let click = |pressed| Event::PointerButton {
                pos: center,
                button: PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            vec![Event::PointerMoved(center), click(true), click(false)]
        };
        let render = |ui: &mut egui::Ui, outbox: &mut Vec<Message>| {
            let children = tree.children.get(&root).unwrap();
            tree_rows(ui, &tree, children, None, &git, outbox);
        };

        // 第二帧点文件行 → FileSelected
        ctx.run_ui(
            RawInput {
                events: click_at(&file_rect),
                ..Default::default()
            },
            |ui| render(ui, &mut outbox),
        )
        .drop_without_applying_deltas();
        assert_eq!(outbox, vec![Message::FileSelected(file)]);

        // 第三帧点目录行 → FileTreeToggled
        outbox.clear();
        ctx.run_ui(
            RawInput {
                events: click_at(&dir_rect),
                ..Default::default()
            },
            |ui| render(ui, &mut outbox),
        )
        .drop_without_applying_deltas();
        assert_eq!(outbox, vec![Message::FileTreeToggled(docs)]);
    }

    /// 当前小节判定:光标在正文里高亮所属标题,首个标题之前/无光标不高亮。
    #[test]
    fn active_index_follows_cursor_section() {
        let outline = vec![item(1, "甲", 0), item(2, "乙", 10), item(3, "丙", 20)];
        assert_eq!(active_index(&outline, None), None);
        assert_eq!(active_index(&outline, Some(0)), Some(0), "恰在标题起点");
        assert_eq!(active_index(&outline, Some(5)), Some(0), "甲的正文");
        assert_eq!(active_index(&outline, Some(10)), Some(1));
        assert_eq!(active_index(&outline, Some(29)), Some(2));
    }

    /// 点击大纲条目:发出的消息携带该标题的源码区间;仅渲染不产生消息。
    #[test]
    fn clicking_item_sends_span_message() {
        let ctx = egui::Context::default();
        let items = [item(1, "标题一", 0), item(2, "标题二", 10)];
        let mut outbox = Vec::new();
        let rect = Cell::new(Rect::NOTHING);

        // 第一帧只渲染,借 Cell 拿到条目的屏幕位置
        ctx.run_ui(RawInput::default(), |ui| {
            rect.set(outline_row(ui, &items[1], false, &mut outbox).rect);
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
                outline_row(ui, &items[1], false, &mut outbox);
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(outbox, vec![Message::OutlineItemClicked(10..19)]);
    }

    /// 点击搜索结果行:发出携带(路径, 行号)的消息;仅渲染不产生消息。
    #[test]
    fn clicking_search_row_sends_message() {
        let ctx = egui::Context::default();
        let root = PathBuf::from("/vault");
        let hit = SearchResult {
            path: root.join("docs/note.md"),
            line_no: 7,
            line_text: "命中这一行".to_owned(),
        };
        let mut outbox = Vec::new();
        let rect = Cell::new(Rect::NOTHING);

        // 第一帧只渲染,拿行位置
        ctx.run_ui(RawInput::default(), |ui| {
            rect.set(search_row(ui, &hit, &root, &mut outbox).rect);
        })
        .drop_without_applying_deltas();
        assert!(outbox.is_empty());

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
                search_row(ui, &hit, &root, &mut outbox);
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(
            outbox,
            vec![Message::SearchResultClicked(root.join("docs/note.md"), 7)]
        );
    }

    /// Git 面板渲染矩阵:降级文案、干净工作区、改动列表 + 选中 diff +
    /// 历史,三条路径都不 panic;点击改动行发 GitFileSelected。
    #[test]
    fn git_panel_renders_states_and_clicking_row_sends_message() {
        let mut git = GitPanelState::default();
        git.entries.push(FileStatus {
            path: "docs/note.md".to_owned(),
            code: StatusKind::Modified,
        });
        git.selected = Some("docs/note.md".to_owned());
        git.diff = "@@ -1 +1 @@\n-旧\n+新\n".to_owned();
        git.commits.push(CommitInfo {
            hash: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            short_hash: "0123456".to_owned(),
            subject: "初稿".to_owned(),
            author: "LaterMD <latermd@test>".to_owned(),
            time: 1_700_000_000,
        });

        let ctx = egui::Context::default();
        let mut outbox = Vec::new();
        let row_rect = Cell::new(Rect::NOTHING);

        // 帧 1:正常态整体渲染 + 拿改动行位置
        ctx.run_ui(RawInput::default(), |ui| {
            git_panel(ui, &git, &mut outbox);
        })
        .drop_without_applying_deltas();
        assert!(outbox.is_empty(), "仅渲染不产生消息");
        ctx.run_ui(RawInput::default(), |ui| {
            row_rect.set(git_status_row(ui, &git, &git.entries[0], &mut outbox).rect);
        })
        .drop_without_applying_deltas();

        // 帧 2:点击改动行 → GitFileSelected
        let center = row_rect.get().center();
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
                git_status_row(ui, &git, &git.entries[0], &mut outbox);
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(
            outbox,
            vec![Message::GitFileSelected("docs/note.md".to_owned())]
        );

        // 降级态(非 git 仓库)、空历史与截断态:只渲染降级文案/提示行,不 panic
        let mut degraded = GitPanelState::default();
        degraded.error = Some("当前目录不是 Git 仓库".to_owned());
        let empty = GitPanelState::default();
        let mut truncated = GitPanelState::default();
        truncated.truncated = 7;
        ctx.run_ui(RawInput::default(), |ui| {
            git_panel(ui, &degraded, &mut Vec::new());
        })
        .drop_without_applying_deltas();
        ctx.run_ui(RawInput::default(), |ui| {
            git_panel(ui, &empty, &mut Vec::new());
        })
        .drop_without_applying_deltas();
        ctx.run_ui(RawInput::default(), |ui| {
            git_panel(ui, &truncated, &mut Vec::new());
        })
        .drop_without_applying_deltas();
    }

    /// 相对时间:单位逐级放大,时钟偏移的负差按「刚刚」。
    #[test]
    fn relative_time_scales_units() {
        assert_eq!(relative_time(1_000, 1_000), "刚刚");
        assert_eq!(relative_time(1_050, 1_000), "刚刚", "负差按刚刚");
        assert_eq!(relative_time(1_000 - 30, 1_000), "刚刚", "不足一分钟");
        assert_eq!(relative_time(1_000 - 60, 1_000), "1 分钟前");
        assert_eq!(relative_time(1_000 - 3_600, 1_000), "1 小时前");
        assert_eq!(relative_time(1_000 - 86_400, 1_000), "1 天前");
        assert_eq!(relative_time(1_000 - 86_400 * 45, 1_000), "1 个月前");
        assert_eq!(relative_time(1_000 - 86_400 * 400, 1_000), "1 年前");
    }

    /// 摘要截断按字符不切断多字节序列,短行原样返回。
    #[test]
    fn snippet_truncates_by_chars() {
        assert_eq!(snippet("短行"), "短行");
        let long = "界".repeat(SNIPPET_MAX_CHARS + 10);
        let cut = snippet(&long);
        assert!(cut.ends_with('…'));
        assert_eq!(cut.chars().count(), SNIPPET_MAX_CHARS + 1);
    }

    // —— M2 三段式(docs/ui-shell-redesign.md §5)——

    /// 四段顺序 helper:一行一 `NAV_ROW_H`,自上而下不重叠。
    #[test]
    fn nav_row_center_y_follows_tab_order() {
        let row = crate::ui::tokens::NAV_ROW_H;
        assert_eq!(
            nav_row_center_y(100.0, SidebarTab::Files),
            100.0 + row * 0.5
        );
        assert_eq!(
            nav_row_center_y(100.0, SidebarTab::Git),
            100.0 + row * 3.5,
            "Git 是第四行"
        );
        let ys = SidebarTab::ALL.map(|tab| nav_row_center_y(0.0, tab));
        assert!(
            ys.windows(2).all(|pair| pair[1] > pair[0]),
            "四行自上而下:{ys:?}"
        );
    }

    /// 三段式骨架:三段自上而下排满左栏、互不重叠,中段吃掉全部剩余高度
    /// (M2 验收点;底段设置行已迁至标题栏齿轮,decisions-pending #31)。
    /// 宽度下限 180px 时同样成立 —— 顶段在窄栏里会换行变高,中段让出的
    /// 高度随之变少。
    #[test]
    fn three_bands_fill_the_panel_top_down() {
        let ctx = egui::Context::default();
        let (tree, _root) = sample_tree();
        let height = Cell::new(0.0f32);
        let mut bands = None;
        ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(crate::ui::tokens::SIDEBAR_MIN_W, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                height.set(ui.max_rect().bottom());
                bands = Some(ui_whole(
                    ui,
                    SidebarTab::Files,
                    &tree,
                    &GitPanelState::default(),
                ));
            },
        )
        .drop_without_applying_deltas();
        let bands = bands.unwrap();

        assert!(bands.top.top() < bands.nav.top(), "顶段在次段之上");
        assert!(bands.nav.top() < bands.body.top(), "次段在中段之上");
        assert!(
            bands.body.bottom() >= height.get() - 1.0,
            "中段吃到左栏底部(不再为底段预留):body.bottom={} panel.bottom={}",
            bands.body.bottom(),
            height.get()
        );
    }

    /// 点「搜索」行的手写命中区:走完整 `sidebar::ui` 三帧请求,只发一条
    /// `SidebarTabChanged`。
    ///
    /// 直接驱 `sidebar::ui`(而非孤立 `nav_row`)是有意的:顶段的五个文件
    /// 动作按钮紧贴次段,只测孤立行会漏掉「谁抢走了这次点击」—— 三栏重排
    /// 时已经在标题栏上踩过一次。
    #[test]
    fn clicking_nav_row_switches_view() {
        let (tree, _root) = sample_tree();
        let ctx = egui::Context::default();
        let mut outbox = Vec::new();
        let mut active = SidebarTab::Files;
        let mut search = SearchState::default();
        let git = GitPanelState::default();
        let nav_top = Cell::new(0.0f32);

        ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(crate::ui::tokens::SIDEBAR_MIN_W, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                nav_top.set(ui_whole(ui, SidebarTab::Files, &tree, &git).nav.top());
            },
        )
        .drop_without_applying_deltas();

        let target = egui::pos2(
            crate::ui::tokens::SIDEBAR_MIN_W / 2.0,
            nav_row_center_y(nav_top.get(), SidebarTab::Search),
        );
        let click = |pressed| Event::PointerButton {
            pos: target,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        // 前两遍是 sizing / 未交互遍,widget 尚不参与命中测试
        for events in [
            Vec::new(),
            Vec::new(),
            vec![Event::PointerMoved(target)],
            vec![click(true)],
            vec![click(false)],
        ] {
            ctx.run_ui(
                RawInput {
                    events,
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(crate::ui::tokens::SIDEBAR_MIN_W, 600.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    ui_whole_with(ui, &mut active, &mut search, &tree, &git, &mut outbox);
                },
            )
            .drop_without_applying_deltas();
        }
        assert_eq!(
            outbox,
            vec![Message::SidebarTabChanged(SidebarTab::Search)],
            "导航行点击只发一条切换消息"
        );
    }

    /// 只读一版的整栏渲染:无 piercing、无 outbox,专供量取三段矩形。
    fn ui_whole(
        ui: &mut egui::Ui,
        tab: SidebarTab,
        tree: &FileTreeState,
        git: &GitPanelState,
    ) -> SidebarBands {
        let mut tab = tab;
        let mut search = SearchState::default();
        let mut outbox = Vec::new();
        super::ui(
            ui,
            &mut tab,
            tree,
            None,
            OutlineView {
                items: &[],
                cursor_byte: None,
            },
            &mut search,
            git,
            &mut outbox,
        )
    }

    /// 带状态与 outbox 的整栏渲染(点击测试走这条)。
    fn ui_whole_with(
        ui: &mut egui::Ui,
        active: &mut SidebarTab,
        search: &mut SearchState,
        tree: &FileTreeState,
        git: &GitPanelState,
        outbox: &mut Vec<Message>,
    ) {
        super::ui(
            ui,
            active,
            tree,
            None,
            OutlineView {
                items: &[],
                cursor_byte: None,
            },
            search,
            git,
            outbox,
        );
    }

    // —— M5 收口:top_actions 是文件动作的唯一常驻按钮入口(编辑器区顶
    // 部的文件工具栏退役,decisions-pending #32;原 `ui::toolbar` 的测试
    // 迁移至此)——

    /// 全量命令的 tooltip 口径:命令名 + 出厂键位(键位可改,出厂默认够
    /// 指路;原 `toolbar::button` 的同款口径)。
    #[test]
    fn top_actions_tooltip_carries_label_and_builtin_shortcut() {
        let keymap = Keymap::builtin();
        for cmd in Command::FILE.iter().copied().chain([Command::ExportHtml]) {
            let tooltip = tooltip_of(cmd, &keymap);
            assert!(
                tooltip.starts_with(cmd.label()),
                "{cmd:?}: tooltip 应以命令名开头:{tooltip}"
            );
            if let Some(shortcut) = keymap.get(cmd) {
                let platform = shortcut.platform_text();
                assert!(
                    tooltip.contains(&platform),
                    "{cmd:?}: tooltip 应带键位 {platform}:{tooltip}"
                );
            }
        }
    }

    /// 全量文件动作按钮逐个点得动:真实 `top_actions_with_probe` 路径下
    /// 点击各发对应命令消息(原 `toolbar_button_sends_command_message` 的
    /// 全量版;仅渲染帧不产消息)。
    #[test]
    fn clicking_every_top_action_sends_its_command() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let commands: Vec<Command> = Command::FILE
            .iter()
            .copied()
            .chain([Command::ExportHtml])
            .collect();
        let ctx = egui::Context::default();
        let rects = Rc::new(RefCell::new(Vec::<(Command, Rect)>::new()));

        // 帧 1:探针拿按钮矩形(仅渲染,顺带断言不产消息)
        {
            let sink = rects.clone();
            let output = ctx.run_ui(RawInput::default(), |ui| {
                super::top_actions_with_probe(
                    ui,
                    &mut Vec::new(),
                    Some(&mut |cmd, rect| {
                        sink.borrow_mut().push((cmd, rect));
                    }),
                );
            });
            output.drop_without_applying_deltas();
        }
        let rects = rects.borrow().clone();
        assert_eq!(
            rects.iter().map(|(cmd, _)| *cmd).collect::<Vec<_>>(),
            commands,
            "按钮全集 = Command::FILE + ExportHtml,不多不少"
        );

        // 每按钮三帧(moved / press / release):点击发出对应消息
        let click = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        for (cmd, rect) in rects {
            let mut outbox = Vec::new();
            let center = rect.center();
            for events in [
                vec![Event::PointerMoved(center)],
                vec![click(center, true)],
                vec![click(center, false)],
            ] {
                let output = ctx.run_ui(
                    RawInput {
                        events,
                        ..Default::default()
                    },
                    |ui| super::top_actions_with_probe(ui, &mut outbox, None),
                );
                output.drop_without_applying_deltas();
            }
            assert_eq!(outbox, vec![cmd.message()], "{cmd:?} 按钮点击出对应消息");
        }
    }

    /// hover 出 tooltip:指针停在按钮上时,tooltip 文本(含出厂键位)真实
    /// 渲染出来(`everything_is_visible` 是 egui 自己的 UI 测试手法,免去
    /// tooltip 延迟)。
    #[test]
    fn hovering_top_action_shows_shortcut_tooltip() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let ctx = egui::Context::default();
        let save_rect = Rc::new(RefCell::new(Rect::NOTHING));
        {
            let sink = save_rect.clone();
            let output = ctx.run_ui(RawInput::default(), |ui| {
                super::top_actions_with_probe(
                    ui,
                    &mut Vec::new(),
                    Some(&mut |cmd, rect| {
                        if cmd == Command::Save {
                            *sink.borrow_mut() = rect;
                        }
                    }),
                );
            });
            output.drop_without_applying_deltas();
        }
        let center = save_rect.borrow().center();
        assert!(center.x > 0.0, "探针拿到保存按钮");

        ctx.memory_mut(|mem| mem.set_everything_is_visible(true));
        // 两帧:hover 判定用上一帧的指针位置,tooltip 在下一帧才渲染
        let mut shapes = Vec::new();
        for events in [
            vec![Event::PointerMoved(center)],
            vec![Event::PointerMoved(center)],
        ] {
            let output = ctx.run_ui(
                RawInput {
                    events,
                    ..Default::default()
                },
                |ui| super::top_actions_with_probe(ui, &mut Vec::new(), None),
            );
            shapes = output.shapes.clone();
            output.drop_without_applying_deltas();
        }
        let shortcut = Keymap::builtin()
            .get(Command::Save)
            .map(|s| s.platform_text())
            .unwrap_or_default();
        let expected = format!("保存({shortcut})");
        let painted: Vec<String> = shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect();
        assert!(
            painted.iter().any(|t| t.contains(&expected)),
            "tooltip 应含 {expected:?},实际画出的文本:{painted:?}"
        );
    }
}
