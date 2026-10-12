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

use crate::backlink_panel::{jump_target, Backlink, BacklinkState, BacklinkStatus};
use crate::command::Command;
use crate::filetree::{DirChildren, FileTreeState, TreeEntry};
use crate::git_panel::{DiffView, GitPanelState};
use crate::git_split_diff::{inline_segments, line_no_text, split_rows, SplitRow, MAX_SPLIT_PAIRS};
use crate::keymap::Keymap;
use crate::search::{SearchResult, SearchState, SearchStatus, MAX_HITS};
use crate::state::{Message, SidebarTab};
use crate::ui::icons::Icon;
use crate::ui::tokens::{self, RADIUS_SM, SPACE_XS};
use latermd_git::{CommitInfo, DiffHunk, DiffLine, DiffLineKind, FileDiff, FileStatus, StatusKind};
use latermd_md::OutlineItem;

use eframe::egui;
use std::path::{Path, PathBuf};

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

/// rail 内单枚按钮的矩形(纯几何,`draw_rail` 绘制与无头测试共用一份)。
///
/// 为什么要有这个函数:rail 里的按钮是逐枚 `allocate_rect` 摆**绝对**
/// 矩形,不像流式布局那样自带「下一个在哪」。测试若自己按 `RAIL_GAP` 复
/// 算一遍,就把摆位规则抄成了两份 —— 改 GAP 时绘制与断言会一起漂移,
/// 断言自我满足(docs/ui-shell-redesign-v2.md §2.1 的同款纪律)。
///
/// index 自上而下、自 rail 顶缘起算;横向在场带宽内居中(宽自
/// [`tokens::RAIL_W`] 读,不写死数字 —— 2026-10-09 由 48 收到 40)。
fn rail_item_rect(rail: egui::Rect, index: usize) -> egui::Rect {
    let item = tokens::RAIL_ITEM;
    let left = rail.left() + (rail.width() - item) * 0.5;
    let top = rail.top() + (item + tokens::RAIL_GAP) * index as f32;
    egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(item, item))
}

/// 绘制左缘 rail(2026-10-08 决策):**完全取代**单行横排 `view_nav` 与原
/// 顶部 `top_actions`,画 [`SidebarTab::ALL`] 五视图。原下组(文件动作
/// 六枚 + 分隔线)**2026-10-12 应坤哥要求整组裁撤** —— 文件动作仍有
/// 菜单「文件」「导出」、快捷键与快速打开三个入口,rail 回归纯视图切换。
///
/// 每一枚只 `allocate_rect` **一次**并返回同一个 `Response`,命中
/// 判定与点击判定都读它 —— 同一矩形分配两次会叠出两层命中,后画的那个
/// 吞掉前一个的 `clicked()`(egui 同层「后画者胜」),首版草稿踩过。
///
/// 选中态口径随排布翻转:rail 是竖排,原 `view_nav` 的「下缘横条」这时
/// 读起来像「它下面那一枚的顶边」,故换回「**左缘竖条**」—— 竖排态下
/// 竖条的「所属行」读得出来,这也正是 `NAV_ROW_H` 时代那个形状的原意。
fn draw_rail(
    panel: &mut egui::Ui,
    rail: egui::Rect,
    active: SidebarTab,
    outbox: &mut Vec<Message>,
) {
    panel
        .painter()
        .rect_filled(rail, 0.0, crate::theme::shell(panel).rail);

    for (index, tab) in SidebarTab::ALL.into_iter().enumerate() {
        let rect = rail_item_rect(rail, index);
        let response = panel.allocate_rect(rect, egui::Sense::click());
        if panel.is_rect_visible(rect) {
            paint_rail_item(panel, rect, tab, tab == active, response.hovered());
        }
        if response.on_hover_text(tab.label()).clicked() {
            outbox.push(Message::SidebarTabChanged(tab));
        }
    }
}

/// rail 单枚的绘制:选中 = 实心底 + 左缘竖条;hover = 内建 hover 底;
/// 常态只画图标。
fn paint_rail_item(
    ui: &egui::Ui,
    rect: egui::Rect,
    tab: SidebarTab,
    selected: bool,
    hovered: bool,
) {
    let painter = ui.painter();
    let visuals = ui.visuals();
    let radius = egui::CornerRadius::same(tokens::RADIUS_SM as u8);
    if selected {
        let shell = crate::theme::shell(ui);
        painter.rect_filled(rect, radius, shell.selected_bg);
        painter.rect_filled(
            egui::Rect::from_min_size(
                rect.left_top(),
                egui::vec2(tokens::NAV_BAR_W, rect.height()),
            ),
            radius,
            tokens::accent(ui),
        );
    } else if hovered {
        painter.rect_filled(rect, radius, visuals.widgets.hovered.bg_fill);
    }
    let color = if selected {
        tokens::accent(ui)
    } else {
        visuals.text_color()
    };
    tab.icon()
        .draw(painter, rect.center(), tokens::NAV_TAB_ICON, color);
}

/// rail 带本身:占 sidebar 矩形左端 [`tokens::RAIL_W`] 宽、纵贯到底。
/// 绘制与测试共用,理由同 [`rail_item_rect`]。
fn rail_strip(sidebar: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(sidebar.left(), sidebar.top()),
        egui::pos2(sidebar.left() + tokens::RAIL_W, sidebar.bottom()),
    )
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
    backlinks: &BacklinkState,
    outbox: &mut Vec<Message>,
) -> SidebarBands {
    let right = panel.max_rect().right();

    // rail 占左端 RAIL_W 宽的一条纵带,余下宽度才是 nav / body。刻意**不**
    // 开第四个 `Panel::left`:那会让左栏总占宽 240 → 288,中央编辑区净
    // 损 48px —— 省下的纵向 chrome 会以横向的形式赔回去。
    //
    // rail 先画、body 的 ScrollArea 后加:后者按 available_width 演算,
    // 天然从 rail 右缘起排,不必手动给它减 RAIL_W。
    let strip = rail_strip(panel.max_rect());
    draw_rail(panel, strip, *active_tab, outbox);

    // body 必须显式下到一个**限定过的子 Ui**里,不能指望它自己往后排:
    // `draw_rail` 用的是逐枚 `allocate_rect` 摆**绝对**矩形,**不吃游标**
    // —— 光标仍停在 sidebar 顶缘,若直接顺势加 ScrollArea,它会自 chromium
    // 上一枚 rail 按钮之后往下延伸(body.top 实测 351px,即 rail 底)。
    //
    // `new_child` + `max_rect` 把 body 横向截到 rail 右缘之右、纵向仍自顶部
    // 起排 —— 这才是「rail 与 body 左右并排」。
    let body_left = strip.right();
    let body_rect = egui::Rect::from_min_max(
        egui::pos2(body_left, panel.max_rect().top()),
        egui::pos2(right, panel.max_rect().bottom()),
    );
    let mut body = panel.new_child(egui::UiBuilder::new().max_rect(body_rect));
    draw_body(
        &mut body,
        active_tab,
        file_tree,
        current_file,
        outline,
        search,
        git,
        backlinks,
        false,
        outbox,
    );

    // 子 Ui 不会撑开父面板，保留 main 的拖宽修复。
    panel.expand_to_include_rect(panel.max_rect());

    SidebarBands {
        // 顶段已随 2026-10-08 改版消失(退化为零高矩形),保留字段只为
        // 不破坏既有 `SidebarBands` 的形状。
        top: egui::Rect::from_min_size(strip.left_top(), egui::vec2(0.0, 0.0)),
        // nav 字段在新版里承载 rail 带(纵贯到底,不再是横向条带)。
        nav: strip,
        body: body_rect,
    }
}

/// macOS 文字导航与视图内容,不占用独立图标轨道。
#[allow(clippy::too_many_arguments)]
pub fn workbench_ui(
    panel: &mut egui::Ui,
    active_tab: &mut SidebarTab,
    file_tree: &FileTreeState,
    current_file: Option<&Path>,
    outline: OutlineView<'_>,
    search: &mut SearchState,
    git: &GitPanelState,
    backlinks: &BacklinkState,
    outbox: &mut Vec<Message>,
) -> SidebarBands {
    let top = egui::Rect::from_min_size(panel.cursor().min, egui::Vec2::ZERO);
    panel.add_space(8.0);
    crate::ui::workbench::workspace_picker(panel, file_tree, outbox);
    let nav = crate::ui::workbench::navigation(panel, *active_tab, outbox);
    // 导航与内容使用两个明确的子 Ui。直接复用父 Ui 会让纵向滚动区域
    // 从面板顶端重新计算可视区，切到搜索/反向链接时首行被标题栏裁切。
    let body = panel.available_rect_before_wrap();
    let body_rect = egui::Rect::from_min_max(
        body.left_top(),
        egui::pos2(body.right(), panel.max_rect().bottom()),
    );
    let mut body_ui = panel.new_child(egui::UiBuilder::new().max_rect(body_rect));
    draw_body(
        &mut body_ui,
        active_tab,
        file_tree,
        current_file,
        outline,
        search,
        git,
        backlinks,
        true,
        outbox,
    );
    panel.expand_to_include_rect(panel.max_rect());
    SidebarBands { top, nav, body }
}

#[allow(clippy::too_many_arguments)]
fn draw_body(
    panel: &mut egui::Ui,
    active_tab: &mut SidebarTab,
    file_tree: &FileTreeState,
    current_file: Option<&Path>,
    outline: OutlineView<'_>,
    search: &mut SearchState,
    git: &GitPanelState,
    backlinks: &BacklinkState,
    workbench: bool,
    outbox: &mut Vec<Message>,
) {
    egui::ScrollArea::vertical()
        .id_salt("nav-body")
        .auto_shrink([false, false])
        .max_height(panel.available_height())
        .show(panel, |ui| match *active_tab {
            SidebarTab::Files => files_panel(ui, file_tree, current_file, git, workbench, outbox),
            SidebarTab::Search => search_panel(ui, search, file_tree.root.as_deref(), outbox),
            SidebarTab::Outline => outline_panel(ui, outline, outbox),
            SidebarTab::Git => git_panel(ui, git, outbox),
            SidebarTab::Backlinks => backlinks_panel(
                ui,
                backlinks,
                file_tree.root.as_deref(),
                current_file,
                outbox,
            ),
        });
}

/// 左栏三段各自的竖直区间,自上而下互不重叠。
///
/// 由 [`ui`] 顺手测出来返回:`view_nav` 是手绘 widget(无可读名字),要算
/// 「第 N 枚 tab 的 x」只能靠 [`SidebarTab::ALL`] 的顺序 + `NAV_TAB_H`
/// 自己推(无头测试里的 `nav_tab_center` 就是这么算的;它是 `#[cfg(test)]`,
/// 故此处不能用文档链接指它 —— 生产构建里没有这个 item,写了会触发
/// `broken_intra_doc_links` 警告)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarBands {
    /// 顶段:高频文件动作(2026-10-08 改版起恒为零高矩形;2026-10-12 rail
    /// 下组文件动作亦整组裁撤,左栏只剩 rail 五视图,字段仅为形状兼容保留)。
    pub top: egui::Rect,
    /// 次段:视图导航(2026-10-08 起为单行五 tab,`NAV_TAB_H` 高)。
    pub nav: egui::Rect,
    /// 中段:当前视图内容,吃掉剩余高度(至左栏底部)。
    pub body: egui::Rect,
}

/// 左栏顶部高频文件动作(**2026-10-08 起退役**)。
///
/// 原为 [Command::FILE] + 导出两项共六枚横排常驻,是文件动作的按钮入口
/// (decisions-pending #32)。后迁入 [`draw_rail`] 下组,而下组本身
/// **2026-10-12 亦整组裁撤** —— 文件动作的常驻按钮入口就此归零,只剩
/// 菜单「文件」「导出」、快捷键与快速打开;rail 只剩五枚视图 tab。
///
/// **保留而不删**的理由同 `tokens::NAV_ROW_H`:历史 commit 的测试引用它,
/// 删掉会让旧 commit 无法 `git bisect` 复现。新代码不许新增调用方。
#[allow(dead_code)]
fn top_actions(panel: &mut egui::Ui, outbox: &mut Vec<Message>) {
    top_actions_with_probe(panel, outbox, None);
}

/// 同 [`top_actions`],额外把每个按钮的 `(命令, 矩形)` 交给 `probe`。
///
/// 无头测试量按钮位置用(与 `ui::format_bar::ui_with_probe` 同款手法):
/// 按钮坐标由 `horizontal_wrapped` 的换行演算决定,手搓必然与真实帧错位。
#[allow(dead_code)]
fn top_actions_with_probe(
    panel: &mut egui::Ui,
    outbox: &mut Vec<Message>,
    probe: Option<&mut dyn FnMut(Command, egui::Rect)>,
) {
    let mut probe = probe;
    panel.horizontal_wrapped(|ui| {
        for cmd in Command::FILE
            .iter()
            .copied()
            .chain([Command::ExportHtml, Command::ExportPdf])
        {
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

/// 左栏次段的视图导航:**单行横排五个图标 tab**(**2026-10-08 起退役**)。
///
/// **2026-10-08 第一次改版**(docs/ui-shell-redesign-v2.md §2):原为竖排
/// 五行 ×`NAV_ROW_H` = 130px 常驻,而它下面才是主工作区(文件树)。改为
/// 单行后 nav 段 26px,省下的 104px 全给中段 —— 侧栏可视高度 +48%。
///
/// 选中态从「浅色底 + 左侧 2px 竖条」换成「**下缘 2px 强调色横条**」:
/// 横排时竖条会指向相邻 tab(五枚 26px 宽的 tab 挤在 180px 窄栏里,
/// 竖条的「所属行」视觉上已经读不出来了),横条才与「页签」同构。
/// 横条口径与 `ui::tabs` 的页签选中下划线一致(ui-polish §1.2 同款)。
///
/// 无障碍口径:每个 tab 仍带 tooltip(图标不自解释,ui-polish §1.1 铁律),
/// 且命中区是整枚 26×26 而非仅图标。
///
/// **2026-10-08 第二次改版**:五枚 tab 与顶段六枚文件动作一并迁入
/// [`draw_rail`],横排 nav 随之退役。**保留而不删**的理由同
/// `tokens::NAV_ROW_H`:历史 commit 的测试引用它,删掉会让旧 commit 无法
/// `git bisect` 复现。新代码不许新增调用方。
#[allow(dead_code)]
fn view_nav(panel: &mut egui::Ui, active: SidebarTab, outbox: &mut Vec<Message>) {
    // 五枚 tab 一行放得下(5×26 + 4×2 = 138 < SIDEBAR_MIN_W 180),
    // 故不换行;逐枚 `allocate_rect` 摆位而非 `horizontal`,理由与
    // `top_actions` 的探针同款:手绘 widget 才有确定的 x 供无头测试定位。
    let size = egui::vec2(crate::ui::tokens::NAV_TAB_H, crate::ui::tokens::NAV_TAB_H);
    let mut x = panel.max_rect().left();
    let top = panel.cursor().top();
    for tab in SidebarTab::ALL {
        let rect = egui::Rect::from_min_size(egui::pos2(x, top), size);
        // 命中区铺满整枚 tab(不是仅图标),`is_rect_visible` 之外也照发
        // 点击 —— 被裁掉的 tab 不该吃掉指针。
        let response = panel
            .allocate_rect(rect, egui::Sense::click())
            .on_hover_text(tab.label());
        if panel.is_rect_visible(rect) {
            paint_nav_tab(panel, rect, tab, tab == active, response.hovered());
        }
        if response.clicked() {
            outbox.push(Message::SidebarTabChanged(tab));
        }
        x += size.x + crate::ui::tokens::NAV_TAB_GAP;
    }
    // 推进游标到本段底部:逐枚 `allocate_rect` 吃的是**绝对**矩形,不吃
    // 游标,不手动推的话后面的 ScrollArea 会与 nav 段重叠。
    panel.set_min_height(top + size.y);
}

/// 单枚视图 tab 的绘制:圆角底(选中/hover)+居中图标+下缘横条。
///
/// 手绘而非 `Button`:`Button` 的文字排版与本项目的图标自绘铁律
/// (ui-polish §1.1)不合,且拿不到「下缘横条」这种自定义选中标记。
///
/// **2026-10-08 起随 `view_nav` 一并退役**,画选中标记的职责转到
/// [`paint_rail_item`]。保留理由同 `tokens::NAV_ROW_H`。
#[allow(dead_code)]
fn paint_nav_tab(ui: &egui::Ui, rect: egui::Rect, tab: SidebarTab, selected: bool, hovered: bool) {
    let painter = ui.painter();
    let visuals = ui.visuals();
    // `CornerRadius` 收 u8,token 是 f32 —— 这里取整。RADIUS_SM=4 本来
    // 就是整数,取整无损;真要改成非整数 token 时这条要重新核。
    let radius = egui::CornerRadius::same(crate::ui::tokens::RADIUS_SM as u8);
    if selected {
        let selected_bg = crate::theme::shell(ui).selected_bg;
        painter.rect_filled(rect, radius, selected_bg);
        // 下缘 2px 强调色横条:贴着 tab 底缘,宽度取 tab 宽的 60%,
        // 视觉上是一枚「下划线」而非「边框」,与页签选中同款。
        let bar_w = rect.width() * 0.6;
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(
                    rect.center().x - bar_w / 2.0,
                    rect.bottom() - crate::ui::tokens::NAV_BAR_W,
                ),
                egui::vec2(bar_w, crate::ui::tokens::NAV_BAR_W),
            ),
            radius,
            crate::ui::tokens::accent(ui),
        );
    } else if hovered {
        // hover 用内建 hover 底(不自己调色,跟随主题)。
        painter.rect_filled(rect, radius, visuals.widgets.hovered.bg_fill);
    }
    let color = if selected {
        crate::ui::tokens::accent(ui)
    } else {
        visuals.text_color()
    };
    tab.icon().draw(
        painter,
        rect.center(),
        crate::ui::tokens::NAV_TAB_ICON,
        color,
    );
}

/// 单枚视图 tab 的中心点(相对 panel 左缘与 nav 段顶缘),供无头测试定位。
///
/// 顺序是 [`SidebarTab::ALL`]、边长与间隙是 `NAV_TAB_H` / `NAV_TAB_GAP`,
/// 与 [`view_nav`] 的摆位同源地算出来即可,不必把矩形一路传出去。
#[cfg(test)]
pub(crate) fn nav_tab_center(left: f32, top: f32, tab: SidebarTab) -> egui::Pos2 {
    let index = SidebarTab::ALL
        .iter()
        .position(|candidate| *candidate == tab)
        .expect("tab 必在 ALL 里");
    let pitch = crate::ui::tokens::NAV_TAB_H + crate::ui::tokens::NAV_TAB_GAP;
    egui::pos2(
        left + crate::ui::tokens::NAV_TAB_H * 0.5 + pitch * index as f32,
        top + crate::ui::tokens::NAV_TAB_H * 0.5,
    )
}

/// 图标 + 文字的整行按钮,高度可调。
///
/// **当前无生产调用方**:2026-10-08 视图导航从竖排五行改为单行图标 tab
/// (`view_nav` + `paint_nav_tab`)后,本函数随之退役。它与
/// `ui::outline`/Git 面板里的行式列表不同类(那些走各自的 `*_row`),留着
/// 是纯死代码 —— 但删函数要连同其唯一的历史价值(整行选中态的画法)
/// 一起搬走,故先在 `paint_nav_tab` 的文档里留了口径指针。
///
/// **保留而不删**的理由同 `tokens::NAV_ROW_H`:历史 commit 的测试引用它,
/// 删掉会让旧 commit 无法 `git bisect` 复现。新代码不许新增调用方。
#[allow(dead_code)]
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
            let selected_bg = crate::theme::shell(ui).selected_bg;
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

/// Search 页:搜索行(折叠箭头 + 输入 + 三开关)+ 可选替换行 + 状态行
/// + 按文件分组的结果列表。
///
/// 开关对齐 VS Code:「Aa」区分大小写、「ab」全字匹配、正则 `.*`;三者
/// 都是选中态实底的 `selectable_label` 小片,翻转即重发
/// [`Message::SearchQueryChanged`](去抖 300ms 重搜,不用按回车)。
///
/// 「全部替换」是跨文件写盘的破坏性操作:按钮只发
/// [`Message::SearchReplaceAllRequested`],归约挂起确认浮窗
/// (`ui::layout` 绘制),确认后逐文件替换并产出回执(状态行)。
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

    search_controls(panel, search, outbox);
    search_results(panel, search, root, outbox);
}

/// `search_controls` 渲染出的控件句柄:`row` 是搜索行五件套(箭头/输入/
/// 三模式片),`replace_all` 是展开替换行时才在场的「全部替换」图标按钮。
/// 生产路径只渲染不读回;字段供无头测试按句柄定位/点击(自绘图标按钮无
/// 文本 shape 可找),allow 而非拆函数 —— 拆出去测试就得复刻一份布局。
#[allow(dead_code)]
pub struct SearchControlsHandles {
    pub row: [egui::Response; 5],
    pub replace_all: Option<egui::Response>,
}

fn search_controls(
    panel: &mut egui::Ui,
    search: &mut SearchState,
    outbox: &mut Vec<Message>,
) -> SearchControlsHandles {
    // 行高必须**自适应**:按钮自然高随字体行高变(真机 CJK 比 egui 内置
    // 字体高 ~4px),任何写死的高度都会让固定尺寸分配溢出居中、控件逐个
    // 阶梯下移(真机截图回归:「不在一条线上」)。horizontal 自带交叉轴
    // 居中且行高取最高控件;别用 horizontal_centered,它会占满父级全部
    // 高度,把替换行和结果列表挤出布局(渲染测试当场红)。
    //
    // 结构(VS Code 同款):箭头列在最左,右侧竖排「搜索行 + 可选替换行」
    // —— 展开时行组变高,箭头居中到两行的正中。
    //
    // egui 水平布局的交叉轴居中参照随放置顺序漂移(行内控件自然高不同时
    // 逐个下沉,真机截图回归),所以两处都对齐做**等高化**:行内控件统一
    // min_size 高 = 字体行高 + 按钮内边距;箭头高度用上一帧行组高(帧内
    // 布局稳定后一帧收敛,与 egui 折叠动画同思路)。
    let row_h = ui_row_height(panel);
    let mut row: [Option<egui::Response>; 5] = [const { None }; 5];
    let mut replace_all: Option<egui::Response> = None;
    let rows_h_id = egui::Id::new("latermd-search-rows-h");
    let rows_h = panel
        .ctx()
        .memory(|m| m.data.get_temp::<f32>(rows_h_id).unwrap_or(row_h));
    panel.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.spacing_mut().button_padding.x = 2.0;
        ui.spacing_mut().item_spacing.y = 2.0;
        let mode_w = 24.0;
        let (rect, chevron) =
            ui.allocate_exact_size(egui::vec2(22.0, rows_h.max(row_h)), egui::Sense::click());
        if chevron.hovered() || chevron.has_focus() {
            ui.painter()
                .rect_filled(rect, RADIUS_SM, ui.visuals().widgets.hovered.bg_fill);
        }
        ui.painter().add(egui::Shape::convex_polygon(
            arrow_vertices(rect.center(), 3.5, search.replace_open).to_vec(),
            ui.visuals().text_color(),
            egui::Stroke::NONE,
        ));
        chevron
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "展开替换"));
        let chevron = chevron.on_hover_text(if search.replace_open {
            "收起替换"
        } else {
            "展开替换"
        });
        if chevron.clicked() {
            search.replace_open = !search.replace_open;
        }
        let right_rows_rect = ui.vertical(|ui| {
            // 搜索行:输入框占剩余宽,为右侧三枚模式片预留;模式片只钉宽、
            // 高度统一到 row_h(等高化,见 ui_row_height)。
            row = ui
                .horizontal(|ui| {
                    let input_width = (ui.available_width() - 3.0 * (mode_w + 2.0)).max(1.0);
                    let input = egui::TextEdit::singleline(&mut search.query)
                        .id_salt("search-input")
                        .hint_text(if search.regex {
                            "正则表达式…"
                        } else {
                            "搜索…"
                        })
                        .desired_width(input_width)
                        // 垂直 margin 归零 + vertical_align(Center):出厂
                        // frame(上下各 2px)让文字**贴顶**排,渲染中心与按钮
                        // 错开 —— 截图回归的「不在一条线上」。
                        .margin(egui::Margin::symmetric(4, 0))
                        .vertical_align(egui::Align::Center)
                        .min_size(egui::vec2(0.0, row_h))
                        .show(ui)
                        .response
                        .response;
                    if input.changed() {
                        outbox.push(Message::SearchQueryChanged);
                    }
                    let case = ui
                        .add(
                            egui::Button::selectable(!search.case_insensitive, "Aa")
                                .min_size(egui::vec2(mode_w, row_h)),
                        )
                        .on_hover_text("区分大小写");
                    if case.clicked() {
                        search.case_insensitive = !search.case_insensitive;
                        outbox.push(Message::SearchQueryChanged);
                    }
                    let word = ui
                        .add(
                            egui::Button::selectable(search.whole_word, "ab")
                                .min_size(egui::vec2(mode_w, row_h)),
                        )
                        .on_hover_text("全字匹配");
                    if word.clicked() {
                        search.whole_word = !search.whole_word;
                        outbox.push(Message::SearchQueryChanged);
                    }
                    let regex = ui
                        .add(
                            egui::Button::selectable(search.regex, ".*")
                                .min_size(egui::vec2(mode_w, row_h)),
                        )
                        .on_hover_text("使用正则表达式");
                    if regex.clicked() {
                        search.regex = !search.regex;
                        outbox.push(Message::SearchQueryChanged);
                    }
                    [chevron, input, case, word, regex]
                })
                .inner
                .map(Some);
            // 替换行:替换框与「全部替换」同一行(VS Code 同位),按钮只发
            // 请求,确认浮窗在归约侧;零命中/搜索未完禁用。
            if search.replace_open {
                ui.horizontal(|ui| {
                    // 图标按钮占位按 row_h 预留(VS Code 同款 replace-all
                    // 图标,宽度只占一格,语义靠 tooltip)
                    let all_w = row_h + 2.0;
                    let box_w = (ui.available_width() - all_w).max(1.0);
                    if egui::TextEdit::singleline(&mut search.replace_with)
                        .id_salt("search-replace-input")
                        .hint_text(if search.regex {
                            "替换为…($1 引用捕获组)"
                        } else {
                            "替换为…"
                        })
                        .desired_width(box_w)
                        .margin(egui::Margin::symmetric(4, 0))
                        .vertical_align(egui::Align::Center)
                        .min_size(egui::vec2(0.0, row_h))
                        .show(ui)
                        .response
                        .changed()
                    {
                        search.confirm_replace = None;
                    }
                    let replace_enabled = search.can_replace_all();
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(row_h, row_h), egui::Sense::click());
                    let painter = ui.painter();
                    if resp.hovered() && replace_enabled {
                        painter.rect_filled(rect, RADIUS_SM, ui.visuals().widgets.hovered.bg_fill);
                    }
                    let color = if replace_enabled {
                        ui.visuals().text_color()
                    } else {
                        ui.visuals().weak_text_color()
                    };
                    Icon::ReplaceAll.draw(painter, rect.center(), tokens::ICON, color);
                    resp.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            replace_enabled,
                            "全部替换",
                        )
                    });
                    let resp = resp.on_hover_text(format!(
                        "全部替换:对当前 {} 条命中的所在文件执行(有确认浮窗)",
                        search.hits.len()
                    ));
                    if resp.clicked() && replace_enabled {
                        outbox.push(Message::SearchReplaceAllRequested);
                    }
                    replace_all = Some(resp);
                });
            }
        });
        let rows_h = right_rows_rect.response.rect.height();
        ui.ctx()
            .memory_mut(|m| m.data.insert_temp(rows_h_id, rows_h));
    });
    SearchControlsHandles {
        row: row.map(|slot| slot.expect("搜索行控件必须全部渲染")),
        replace_all,
    }
}

/// 搜索行内控件的统一高度:字体行高 + 按钮上下内边距 + 边框余量,再以
/// 22px 兜底。所有行内控件 min_size 到同一高度 —— egui 水平布局的交叉
/// 轴居中参照会随放置顺序漂移,等高是唯一稳的同心线做法。
fn ui_row_height(ui: &egui::Ui) -> f32 {
    let font_row = ui.text_style_height(&egui::TextStyle::Body);
    (font_row + 2.0 * ui.spacing().button_padding.y + 2.0).max(22.0)
}

fn search_results(
    panel: &mut egui::Ui,
    search: &mut SearchState,
    root: &Path,
    outbox: &mut Vec<Message>,
) {
    match &search.status {
        SearchStatus::Running => {
            panel.weak(format!("搜索中…(已 {} 条)", search.hits.len()));
        }
        SearchStatus::Finished if search.hits.is_empty() => {
            panel.weak("无结果");
        }
        SearchStatus::Finished => {
            let files = hit_groups(&search.hits).len();
            panel.weak(format!("{files} 个文件中 {} 条结果", search.hits.len()));
        }
        SearchStatus::Invalid(msg) => {
            panel.weak(format!("⚠ {msg}"));
        }
        SearchStatus::Idle => {}
    }
    // 替换回执:成功计数 + 逐文件失败原因(读/写盘失败、非 UTF-8 跳过)
    if let Some(report) = &search.last_replace {
        panel.weak(format!(
            "已替换 {} 个文件中的 {} 处匹配",
            report.files, report.replacements
        ));
        if !report.failed.is_empty() {
            panel.weak(format!("✕ {} 个文件未替换:", report.failed.len()));
            for (path, reason) in &report.failed {
                panel.weak(format!("  {path}:{reason}"));
            }
        }
    }

    egui::ScrollArea::vertical()
        .id_salt("search-results-scroll")
        .auto_shrink([false, false])
        .show(panel, |ui| {
            if search.hits.is_empty() && matches!(search.status, SearchStatus::Idle) {
                ui.weak("输入搜索词,回车不必按——300ms 停顿后自动搜索");
            }
            // 按文件分组(VS Code 同款):文件头带命中数、可折叠,行内
            // 只剩「行号 + 摘要」。命中快照按文件连续,分组是 O(n) 一遍扫。
            for (path, group) in hit_groups(&search.hits) {
                let relative = path.strip_prefix(root).unwrap_or(path);
                let header =
                    egui::WidgetText::from(format!("{} ({})", relative.display(), group.len()));
                egui::CollapsingHeader::new(header)
                    .id_salt(relative)
                    .default_open(true)
                    .show(ui, |ui| {
                        for hit in group {
                            search_row(ui, hit, outbox);
                        }
                    });
            }
            if search.truncated {
                ui.weak(format!(
                    "已达 {} 条上限,后续命中未显示(输入更精确的模式收窄)",
                    MAX_HITS
                ));
            }
        });
}

/// 命中按文件分组:返回(路径, 该文件的命中切片)序列。后台扫描按文件
/// 整读整发,同一文件的命中在快照里天然连续;防御性的 `contains` 去重
/// 不需要 —— 分组只依赖连续性,即使将来乱序也只是多几个文件头。
fn hit_groups(hits: &[SearchResult]) -> Vec<(&Path, &[SearchResult])> {
    let mut groups: Vec<(&Path, &[SearchResult])> = Vec::new();
    let mut start = 0;
    while start < hits.len() {
        let path = hits[start].path.as_path();
        let mut end = start + 1;
        while end < hits.len() && hits[end].path == hits[start].path {
            end += 1;
        }
        groups.push((path, &hits[start..end]));
        start = end;
    }
    groups
}

/// 单条命中:「行号 + 命中行摘要」;路径与命中数在文件头(分组视图)。
/// 整块一个可选中标签,点击发消息(打开 + 跳行在 `logic` 归约)。
/// 返回行响应,独立成函数便于点击测试定位。
///
/// 命中片段用**强调色浅底 + 下划线**的行内效果突出(VS Code 搜索列表
/// 同款):`LayoutJob` 把行切成「行号 / 匹配前 / 匹配 / 匹配后」四段,
/// 只有匹配段带背景色。`SelectableLabel` 接受 `LayoutJob`,无需自绘。
fn search_row(ui: &mut egui::Ui, hit: &SearchResult, outbox: &mut Vec<Message>) -> egui::Response {
    let response = ui.add(search_row_widget(ui, hit));
    if response.clicked() {
        outbox.push(Message::SearchResultClicked(hit.path.clone(), hit.line_no));
    }
    response
}

/// 组装一行命中的 widget。选中态恒 false:命中列表没有「当前项」,
/// 底色变化只由 hover 负责。
fn search_row_widget(ui: &egui::Ui, hit: &SearchResult) -> impl egui::Widget + use<> {
    let font_id = egui::TextStyle::Button.resolve(ui.style());
    let text = ui.visuals().text_color();
    let dim = ui.visuals().weak_text_color();
    // 强调色 22% 透明度做浅底:与选中底(selected_bg)同族但更轻,
    // 不会与整行 hover 底混淆(那是中性灰)。
    let accent = tokens::accent(ui);
    let highlight_bg = accent.gamma_multiply(0.22);
    let highlight_ul = egui::Stroke::new(1.0, accent.gamma_multiply(0.65));
    let strong = ui.visuals().strong_text_color();
    let hit = hit.clone();
    move |ui: &mut egui::Ui| -> egui::Response {
        let (line, span) = snippet_with_span(&hit.line_text, &hit.match_span);
        let mut job = egui::text::LayoutJob::default();
        let base = egui::TextFormat {
            font_id: font_id.clone(),
            color: text,
            ..Default::default()
        };
        job.append(
            &format!("{}  ", hit.line_no),
            0.0,
            egui::TextFormat {
                color: dim,
                ..base.clone()
            },
        );
        // 零宽区间(空匹配)或快照截断导致区间越界时,整行退化为无高亮
        let valid = !line.is_empty()
            && span.start < span.end
            && span.end <= line.len()
            && line.is_char_boundary(span.start)
            && line.is_char_boundary(span.end);
        if valid {
            job.append(&line[..span.start], 0.0, base.clone());
            job.append(
                &line[span.clone()],
                0.0,
                egui::TextFormat {
                    background: highlight_bg,
                    underline: highlight_ul,
                    color: strong,
                    ..base.clone()
                },
            );
            job.append(&line[span.end..], 0.0, base);
        } else {
            job.append(&line, 0.0, base);
        }
        ui.add(egui::Button::selectable(false, job))
    }
}

/// 命中行摘要(纯字符串版,backlinks 等无高亮场景用)。
fn snippet(line: &str) -> String {
    snippet_with_span(line, &(0..0)).0
}

/// 命中行摘要:按字符截断(不切断多字节序列),超长加省略号。
/// 同时返回匹配区间在**截断后**文本里的字节区间:截断只发生在行尾,
/// 起点在窗口内则区间原样保留,越界(匹配被截掉)返回 `0..0` 退化为
/// 无高亮 —— 不能把高亮错位地画到省略号上。
fn snippet_with_span(
    line: &str,
    span: &std::ops::Range<usize>,
) -> (String, std::ops::Range<usize>) {
    if line.chars().count() <= SNIPPET_MAX_CHARS {
        return (line.to_owned(), span.clone());
    }
    let cut: String = line.chars().take(SNIPPET_MAX_CHARS).collect();
    let limit = cut.len();
    if span.start < limit && span.end <= limit {
        (format!("{cut}…"), span.clone())
    } else {
        (format!("{cut}…"), 0..0)
    }
}

/// Backlinks 页(#15):状态行 + 「谁链接了当前文档」列表。
///
/// 数据是 `BacklinkState` 的只读快照(触发与扫描都在归约侧,见
/// `crate::backlink_panel` 模块文档),本层零 IO;点击只发
/// [`Message::WikilinkClicked`],打开与切标签在归约的 `open_wikilink`。
/// 无根/文档未落盘/扫描中/无引用四种空态给弱提示。
fn backlinks_panel(
    panel: &mut egui::Ui,
    backlinks: &BacklinkState,
    root: Option<&Path>,
    current_file: Option<&Path>,
    outbox: &mut Vec<Message>,
) {
    let Some(_root) = root else {
        panel.weak("反向链接需要一个根目录:先在「文件」页选择");
        return;
    };
    let Some(_doc) = current_file else {
        panel.weak("保存文档后,这里会列出谁链接了它");
        return;
    };
    // 防抖排程中与进行中都算「扫描中」(300ms 窗口里面板不该空白)
    if backlinks.status == BacklinkStatus::Scanning || backlinks.debounce_due.is_some() {
        panel.weak("扫描中…");
    } else {
        match &backlinks.status {
            BacklinkStatus::Finished if backlinks.links.is_empty() => {
                panel.weak("没有文档链接到这里");
            }
            BacklinkStatus::Finished => {
                panel.weak(format!("共 {} 处引用", backlinks.links.len()));
            }
            BacklinkStatus::Failed(msg) => {
                panel.weak(format!("⚠ {msg}"));
            }
            BacklinkStatus::Idle | BacklinkStatus::Scanning => {}
        }
    }

    egui::ScrollArea::vertical()
        .id_salt("backlinks-scroll")
        .auto_shrink([false, false])
        .show(panel, |ui| {
            for link in &backlinks.links {
                backlink_row(ui, link, outbox);
            }
            if backlinks.truncated {
                ui.weak(format!("已达 {} 条上限,后续引用未显示", MAX_HITS));
            }
        });
}

/// 单条反向链接:第一行「相对根的来源路径:行号」,第二行命中行摘要;
/// 整块一个 SelectableLabel,点击发 [`Message::WikilinkClicked`] 走
/// `open_wikilink` 同一条跳转链路(已开切标签、缺失落提示)。返回行响应,
/// 独立成函数便于点击测试定位。
fn backlink_row(ui: &mut egui::Ui, link: &Backlink, outbox: &mut Vec<Message>) -> egui::Response {
    let text = format!(
        "{}:{}\n{}",
        link.path.display(),
        link.line_no,
        snippet(&link.line_text)
    );
    let response = ui.selectable_label(false, text);
    if response.clicked() {
        outbox.push(Message::WikilinkClicked {
            target: jump_target(&link.path),
        });
    }
    response
}

/// Files 页:顶部根目录选择行 + 懒加载目录树。
fn files_panel(
    panel: &mut egui::Ui,
    tree: &FileTreeState,
    current_file: Option<&Path>,
    git: &GitPanelState,
    workbench: bool,
    outbox: &mut Vec<Message>,
) {
    // 根目录行:最近列表下拉(有历史才有)+ 选目录 / 新建 / 刷新三枚图标
    // 按钮。行宽只有面板宽,三枚按钮按 TOOLBAR_H 见方摆,图标语义靠
    // tooltip 与辅助技术标签(见 `icon_button`)区分。
    if !workbench {
        root_row(panel, tree, current_file, outbox);
    }
    egui::ScrollArea::vertical()
        .id_salt("file-tree-scroll")
        // 不收缩宽度:长文件名在行内省略号截断(#40),不换行也不撑宽面板
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
                ui.weak(if workbench {
                    "从上方打开文件夹，浏览 Markdown 文档"
                } else {
                    "选择一个目录作为文件树根(点上方「选择…」)"
                });
            }
        });
}

/// 「新建」浮窗:文件名输入 + 目标目录回显 + 创建/取消。
///
/// 只收草稿,**不碰 IO** —— 落盘、缓存失效与开标签都在归约
/// (`State::confirm_new_file`)。「创建」在草稿 trim 后为空、或含路径
/// 分隔符时禁用:这两种名 `filetree::create_markdown` 必拒,让按钮先灰
/// 掉比让用户点了吃一条错误提示友好。归约侧仍防御性拒绝直发的消息。
///
/// 返回(创建, 取消)响应供无头测试定位(与 `tabs::rename_dialog` 同款)。
pub(crate) fn new_file_dialog(
    ui: &mut egui::Ui,
    new_file: &mut crate::filetree::NewFileDraft,
) -> (egui::Response, egui::Response) {
    let mut buttons = None;
    egui::Window::new("新建文档")
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ui.ctx().viewport_rect().center())
        .collapsible(false)
        .resizable(false)
        .frame(crate::ui::workbench::dialog_frame(ui))
        .show(ui.ctx(), |ui| {
            ui.label("文件名");
            ui.text_edit_singleline(&mut new_file.draft)
                .on_hover_text("可省略 .md 后缀;已存在同名文件时自动让开");
            ui.add_space(tokens::SPACE_XS);
            ui.weak(format!("位置:{}", new_file.dir.display()));
            ui.add_space(tokens::SPACE_SM);
            ui.separator();
            ui.horizontal(|ui| {
                let name = new_file.draft.trim();
                let ready = crate::filetree::valid_file_name(name);
                let confirm = ui
                    .add_enabled(ready, egui::Button::new("创建"))
                    .on_disabled_hover_text("文件名不能为空,也不能包含路径分隔符");
                let cancel = ui.button("取消");
                buttons = Some((confirm, cancel));
            });
        });
    buttons.expect("浮窗必然绘制创建/取消按钮")
}

/// Files 页顶部根目录行:目录名(最近列表下拉 / 无历史时纯文本)+ 三枚
/// 见方图标按钮(选文件夹 / 新建 / 刷新)。行高只有面板宽,三枚按钮
/// 各自 `TOOLBAR_H` 见方,图标语义靠 tooltip 与辅助技术标签区分
/// (`icon_button` 两者都给,不是只画个图标了事)。
///
/// 独立成函数并返回三枚响应:无头测试要按矩形点它们,而 `files_panel`
/// 内部布局拿不到单枚按钮的矩形。
fn root_row(
    ui: &mut egui::Ui,
    tree: &FileTreeState,
    current_file: Option<&Path>,
    outbox: &mut Vec<Message>,
) -> RootRowHandles {
    let root_hover = tree.root.as_deref().map(|path| path.display().to_string());
    let mut handles = RootRowHandles {
        pick: None,
        new: None,
        refresh: None,
    };
    ui.horizontal(|ui| {
        // 目录名区先吃剩余宽度:ComboBox 默认 `width` 是"整行可用宽",
        // 三枚按钮会被直接挤出侧栏(最窄 180px 时实测按钮右缘到 208)。
        // 故按 `available_width` 减去三枚按钮的预算来给目录名区定宽 ——
        // 单枚实测宽为 `ICON + 2 * SPACE_SM`;间隔按**三个**:行内共
        // 四个元素(目录名 + 三按钮),`item_spacing` 落在每一对之间,漏算
        // 一个就会在最小宽度下溢出(实测 180px 时右缘 188)。
        let buttons_w =
            3.0 * (tokens::ICON + 2.0 * tokens::SPACE_SM) + 3.0 * ui.spacing().item_spacing.x;
        let label_w = (ui.available_width() - buttons_w).max(tokens::ICON);
        let selected = root_label(tree);
        if tree.recents.is_empty() {
            // 无最近列表:纯文本占位,同样吃掉 label_w(否则它也撑满整行)
            let response = ui.add_sized(
                [label_w, tokens::TOOLBAR_H],
                egui::Label::new(&selected).selectable(false),
            );
            if let Some(hover) = &root_hover {
                response.on_hover_text(hover);
            }
        } else {
            let response = egui::ComboBox::from_id_salt("file-tree-roots")
                .selected_text(selected)
                .width(label_w)
                .show_ui(ui, |ui| {
                    for dir in &tree.recents {
                        recent_row(ui, tree, dir, outbox);
                    }
                })
                .response;
            if let Some(hover) = &root_hover {
                response.on_hover_text(hover);
            }
        }
        let pick = crate::ui::icons::icon_button(ui, Icon::Open, "选择文件夹…");
        if pick.clicked() {
            outbox.push(Message::FileTreeRootPick);
        }
        // 新建落在「当前文档所在目录」,没有落盘文档才回落根 —— 决定
        // 目录的活归归约(它手上有完整的文档状态),这里只发请求。
        let new = crate::ui::icons::icon_button(ui, Icon::New, "在当前目录新建 Markdown 文档");
        if new.clicked() {
            if let Some(dir) = new_file_target(current_file, tree.root.as_deref()) {
                outbox.push(Message::FileTreeNewFileRequested(dir));
            }
        }
        let refresh = crate::ui::icons::busy_icon_button(
            ui,
            Icon::Reset,
            tree.refreshing(std::time::Instant::now()),
            "正在刷新目录树…",
            "刷新目录树",
        );
        if refresh.clicked() {
            outbox.push(Message::FileTreeRefreshRequested);
        }
        handles = RootRowHandles {
            pick: Some(pick),
            new: Some(new),
            refresh: Some(refresh),
        };
    });
    handles
}

/// 根目录行三枚按钮的响应(见 [`root_row`])。三枚**每帧都在场**,故用
/// `Option` 而非 `Default`:无头测试按「点这个矩形 → 断言发了这条消息」
/// 驱动,缺席比「退化成一个零点矩形」更早暴露漏画。
///
/// 字段只被测试读(`files_panel` 忽略返回值 —— 点击判定在 `root_row` 内
/// 就地做了),故按目标条件豁免 dead_code:`cargo build` / clippy 的非测试
/// 目标会因字段无读点告警,而门禁跑 `--all-targets` 时测试是读者。
pub struct RootRowHandles {
    #[cfg_attr(not(test), allow(dead_code))]
    pub pick: Option<egui::Response>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub new: Option<egui::Response>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub refresh: Option<egui::Response>,
}

/// recents 下拉项:显示名只取末级文件夹名(`recent_label`,与 root_label
/// 同口径)——路径中间段不进 UI,用户目录名可能含个人信息(#32 I2);
/// 全路径下沉到该项的 hover tooltip,重名末级名靠 tooltip 区分。独立成
/// 函数与 tree_row 等同例,便于无头测试直接驱动(ComboBox 弹层交互不在
/// 无头覆盖范围,根目录选择走 state::tests)。
fn recent_row(
    ui: &mut egui::Ui,
    tree: &FileTreeState,
    dir: &Path,
    outbox: &mut Vec<Message>,
) -> egui::Response {
    let selected = tree.root.as_deref() == Some(dir);
    let response = ui
        .selectable_label(selected, recent_label(dir))
        .on_hover_text(dir.display().to_string());
    if response.clicked() {
        // ComboBox 默认 CloseOnClick:点击项后弹层自动收起
        outbox.push(Message::FileTreeRootSelected(dir.to_owned()));
    }
    response
}

/// 根目录行的显示名。
fn root_label(tree: &FileTreeState) -> String {
    tree.root
        .as_deref()
        .and_then(last_segment)
        .unwrap_or_else(|| "未选择根目录".to_owned())
}

/// 「新建」按钮的目标目录:当前文档所在目录,无落盘文档(或文档在根之外)
/// 时回落文件树根;两者都没有返回 `None`(按钮此时不发消息)。
///
/// **「当前目录」的口径在此定义,归约只做兜底**(目录已被外部删掉时回落
/// 根)。放在 UI 层是因为这里已经同时握有 `current_file` 与 `root`,
/// 而归约侧还得再查一遍文档状态 —— 两处各算一次迟早会算出不同答案。
fn new_file_target(current_file: Option<&Path>, root: Option<&Path>) -> Option<PathBuf> {
    current_file
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(|| root.map(Path::to_path_buf))
}

/// 路径末级名(文件夹名);根路径 `/` 等无末段时为 None。
fn last_segment(path: &Path) -> Option<String> {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// recents 项显示名:末级文件夹名;末段拿不到(如根路径 `/`)时回退
/// 全路径——此时路径本身无中间段,不构成泄漏。
pub(super) fn recent_label(dir: &Path) -> String {
    last_segment(dir).unwrap_or_else(|| dir.display().to_string())
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
    // 行首装饰(展开三角 + folder/file 图标)都不用文本字符:
    // U+25B8/U+25BE 在 Inter 与出厂字体链上均无字形,目录行名字前渲染成
    // 方框(用户实测,PR #53 起纯绘制)。行文本以前导空格占位(空格任何
    // 字体都有,目录/文件天然对齐):头两个空格给展开三角,其后空格数按
    // 图标槽宽度折算,名字从图标右侧起;三角与图标画在占位区,纯绘制不
    // 依赖字体——与 #31 行号槽同思路。
    let selected = !entry.is_dir && current_file == Some(entry.path.as_path());
    let badge = if entry.is_dir {
        None
    } else {
        git.badge_for(&entry.path)
    };
    let response = badged_row_label(ui, selected, tree_row_text(ui, &entry.name), badge)
        .on_hover_text(row_tooltip(&entry.name, badge));
    if entry.is_dir {
        paint_tree_arrow(ui, &response, open);
    }
    paint_tree_entry_icon(ui, &response, entry.is_dir, open);
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

/// 文件树行的显示文本:前导空格数由「三角槽(两个空格)+ 图标槽 + 间隙」
/// 与正文字体空格 advance 折算而来,再扣除 `selectable_label` 的框内边距
/// (`button_padding.x`,出厂 style 4px / 外壳 style 12px 两套都成立),
/// 保证名字首字不与图标重叠;下限两个空格保住三角的占位。
fn tree_row_text(ui: &egui::Ui, name: &str) -> String {
    let space_w = body_space_width(ui).max(0.1);
    let inset = ui.style().spacing.button_padding.x.max(0.0);
    let icon_right = 2.0 * space_w + crate::ui::tokens::SPACE_XS + crate::ui::tokens::ICON_SM;
    let pad =
        (((icon_right + crate::ui::tokens::SPACE_XS - inset) / space_w).ceil() as usize).max(2);
    format!("{}{}", " ".repeat(pad), name)
}

/// 行首 folder/file 图标(自绘,ui-polish §1.1):目录行随开合切换闭合/
/// 开口文件夹,文件行画折角纸页,槽位紧跟展开三角之后,目录/文件行纵向
/// 对齐。尺寸取 `ICON_SM`(页签档,行高内留上下边);颜色取非交互前景
/// (与三角同源随主题),hover/选中只改文本底色,图标几何不动。
fn paint_tree_entry_icon(ui: &egui::Ui, row: &egui::Response, is_dir: bool, open: bool) {
    if !ui.is_rect_visible(row.rect) {
        return;
    }
    let space_w = body_space_width(ui);
    let icon = if is_dir {
        if open {
            crate::ui::icons::Icon::FolderOpen
        } else {
            crate::ui::icons::Icon::FolderClosed
        }
    } else {
        crate::ui::icons::Icon::File
    };
    icon.draw(
        ui.painter(),
        egui::pos2(
            row.rect.left()
                + 2.0 * space_w
                + crate::ui::tokens::SPACE_XS
                + crate::ui::tokens::ICON_SM / 2.0,
            row.rect.center().y,
        ),
        crate::ui::tokens::ICON_SM,
        ui.visuals().widgets.noninteractive.fg_stroke.color,
    );
}

/// 正文空格的 advance(px)。展开三角与图标槽的定位基准:随正文字号
/// 缩放,不随平台字体走样(egui 的 galley 缓存让重复测量只是查表)。
fn body_space_width(ui: &egui::Ui) -> f32 {
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    ui.fonts_mut(|f| {
        f.layout_no_wrap(" ".to_owned(), font_id, egui::Color32::WHITE)
            .rect
            .width()
    })
}

/// 目录行的展开三角:画在行首两个空格占位区的中点上,收起朝右、展开
/// 朝下。尺寸基准是空格 advance(随正文字号缩放,不随平台字体走样),
/// 颜色取弱前景(与行号槽同源);纯 `Shape::convex_polygon`,零字形依赖。
fn paint_tree_arrow(ui: &egui::Ui, row: &egui::Response, open: bool) {
    let space_w = body_space_width(ui);
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
        // 改动列表与选中文件 diff 区之间的节间留白:两侧都是内容 → gap 档。
        panel.add_space(tokens::gap::XS);
        // 文件名 + 视图切换同一行:文件名等宽,右侧「双栏/统一」两态
        // 切换(#53 M2),点击发消息、归约写 `git.diff_view`(本层只读)
        panel.horizontal(|ui| {
            ui.monospace(selected);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                diff_view_toggle(ui, git.diff_view, outbox);
            });
        });
        diff_area(panel, git);
        if panel.button("回滚此文件…").clicked() {
            outbox.push(Message::GitCheckoutRequested(selected.to_owned()));
        }
    }

    // diff 区/回滚按钮与历史折叠区之间的节间留白:同上,gap 档。
    panel.add_space(tokens::gap::XS);
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
    let response = badged_row_label(ui, selected, entry.path.clone(), Some(entry.code))
        .on_hover_text(row_tooltip(&entry.path, Some(entry.code)));
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

/// diff 区(#53 M2):按 `diff_view` 分流。双栏吃结构化 `diff_lines`;
/// 统一沿用既有 [`diff_view`] 文本路径,行为不变。双栏的兜底口径:结构化
/// 未就位(选中后取失败)回落统一文本渲染——错误文案、截断提示都在
/// `diff` 字符串里,可见性不降级(现状口径不破)。
fn diff_area(panel: &mut egui::Ui, git: &GitPanelState) {
    match git.diff_view {
        DiffView::Unified => unified_diff_area(panel, git),
        DiffView::Split => match &git.diff_lines {
            Some(file) if file.binary => {
                panel.weak(latermd_git::DIFF_BINARY_PLACEHOLDER);
            }
            // 空 hunks ⇔ 统一侧的空文本(两 API 同源);首行即超 64KB 的
            // 病态 diff 会空 hunks + truncated,走下面整行渲染 + 提示
            Some(file) if file.hunks.is_empty() && !file.truncated => {
                panel.weak("无文本改动");
            }
            Some(file) => split_diff_view(panel, file),
            None => unified_diff_area(panel, git),
        },
    }
}

/// 统一视图的空态判定(既有口径原样):空文本 = 无文本改动。
fn unified_diff_area(panel: &mut egui::Ui, git: &GitPanelState) {
    if git.diff.is_empty() {
        panel.weak("无文本改动");
    } else {
        diff_view(panel, &git.diff);
    }
}

/// 「双栏/统一」两态切换(#53 M2):当前态高亮,点击发
/// [`Message::GitDiffViewChanged`]。返回 (双栏, 统一) 的响应矩形供
/// 测试定位点击(right_to_left 布局里先加的在最右,视觉顺序「双栏 统一」)。
fn diff_view_toggle(
    ui: &mut egui::Ui,
    current: DiffView,
    outbox: &mut Vec<Message>,
) -> (egui::Rect, egui::Rect) {
    let unified = ui.selectable_label(
        current == DiffView::Unified,
        egui::RichText::new("统一").small(),
    );
    if unified.clicked() {
        outbox.push(Message::GitDiffViewChanged(DiffView::Unified));
    }
    let split = ui.selectable_label(
        current == DiffView::Split,
        egui::RichText::new("双栏").small(),
    );
    if split.clicked() {
        outbox.push(Message::GitDiffViewChanged(DiffView::Split));
    }
    (split.rect, unified.rect)
}

/// 双栏 diff 视图(#53 M2):GitHub 式左右对齐。左右各带行号列(右对齐)
/// 与 +/− 沟槽标记,整行底色增绿删红([`diff_row_bg`]),「单删+单增」
/// 紧邻对再叠行内词级第二级底色(#53 M3,`git_split_diff::inline_change`);
/// hunk 头 `@@` 跨双栏整行;长行单行截断(…)+ 悬停看全文;行对上限
/// [`MAX_SPLIT_PAIRS`] 超限截断 + 显式提示,防大 diff 卡帧。行对来自
/// `git_split_diff` 纯配对层;取舍见 docs/decisions-pending #100/#101。
fn split_diff_view(panel: &mut egui::Ui, file: &FileDiff) {
    let split = split_rows(file);
    egui::ScrollArea::vertical()
        .id_salt("git-diff-split-scroll")
        .auto_shrink([false, false])
        .max_height(200.0)
        .show(panel, |ui| {
            let geom = RowGeometry::of(ui, file);
            for row in &split.rows {
                paint_split_row(ui, row, &geom);
            }
        });
    split_overflow_hints(panel, file, split.dropped_pairs);
}

/// 双栏视图的截断提示行(M1 的 ~64KB 行级截断 + M2 的行对上限),显式
/// 告知内容不完整;返回所画提示的合并矩形(无提示返回 `None`,测试断言
/// 用)。与统一视图「提示在截断文本尾部」同语义,双栏的结构化数据源
/// 没有尾部可拼,独立成行。
fn split_overflow_hints(
    ui: &mut egui::Ui,
    file: &FileDiff,
    dropped_pairs: usize,
) -> Option<egui::Rect> {
    let mut rect = None;
    if file.truncated {
        rect = Some(ui.weak(latermd_git::DIFF_TRUNCATION_PLACEHOLDER).rect);
    }
    if dropped_pairs > 0 {
        let hint = ui.weak(format!(
            "…还有 {dropped_pairs} 行对未显示(双栏视图上限 {MAX_SPLIT_PAIRS})"
        ));
        rect = Some(match rect {
            Some(previous) => previous.union(hint.rect),
            None => hint.rect,
        });
    }
    rect
}

/// 一帧双栏渲染的公共几何(等宽字体、行高、行号列宽、沟槽宽)。行号列
/// 宽按双侧最大行号的位数 × 数字宽计算(`ui::gutter` 同款,位数跨档才
/// 变宽,右对齐不抖)。
struct RowGeometry {
    /// 等宽字体(Monospace 档,#50 的 editor-mono 族经投影同源)。
    font: egui::FontId,
    /// 单行行高(行盒恒定,配对行不因内容折行变高——长行走截断)。
    row_height: f32,
    /// 行号列宽(含右侧留白)。
    gutter_w: f32,
    /// +/− 沟槽列宽。
    sign_w: f32,
    /// 行号前景(弱化)。
    weak: egui::Color32,
}

impl RowGeometry {
    fn of(ui: &egui::Ui, file: &FileDiff) -> Self {
        let font = egui::FontSelection::Style(egui::TextStyle::Monospace).resolve(ui.style());
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
        let digit = ui.fonts_mut(|fonts| fonts.glyph_width(&font, '0'));
        // 双侧最大行号:hunk 头的范围和就是行号覆盖上界
        let max_lineno = file.hunks.iter().fold(0u32, |acc, hunk| {
            acc.max(hunk.old_start.saturating_add(hunk.old_lines))
                .max(hunk.new_start.saturating_add(hunk.new_lines))
        });
        let digits = digit_count(max_lineno);
        Self {
            gutter_w: digits as f32 * digit + 2.0 * SPACE_XS,
            sign_w: digit + SPACE_XS,
            font,
            row_height,
            weak: ui.visuals().weak_text_color(),
        }
    }
}

/// 总行号 → 十进制位数(0 至少 1 位;`ui::gutter` 同款算法,双栏行号列
/// 自用一份——gutter 的是 crate 私有)。
fn digit_count(mut n: u32) -> usize {
    let mut digits = 1;
    while n >= 10 {
        n /= 10;
        digits += 1;
    }
    digits
}

/// 双栏行底色:语义色(增 [`tokens::OK`]/删 [`tokens::DANGER`])向面板
/// 底色(`visuals().panel_fill`)的伽马插值,**单一公式**明暗两主题各自
/// 成立——明色下 ≈ GitHub #e6ffec/#ffebe0 的淡着色观感,暗色自然得到
/// 深绿/深红,不是两套硬编码取色。浓淡只由 [`DIFF_ROW_TINT`] 一个常量
/// 控制。
fn diff_row_bg(ui: &egui::Ui, tint: egui::Color32) -> egui::Color32 {
    diff_tint_bg(ui, tint, DIFF_ROW_TINT)
}

/// 语义色按占比向面板底色插值([`diff_row_bg`] 的参数化本体):整行一档
/// ([`DIFF_ROW_TINT`]),行内词级区段深一档([`DIFF_INLINE_TINT`])。
fn diff_tint_bg(ui: &egui::Ui, tint: egui::Color32, ratio: f32) -> egui::Color32 {
    tint.lerp_to_gamma(ui.visuals().panel_fill, 1.0 - ratio)
}

/// 行底色的语义色占比(0..1):16% 增/删语义色 + 84% 面板底色,明暗主题
/// 下都在「可读出着色语义」与「不压正文对比」之间(数值推断,真机目视
/// 留人工,见 #53 notes)。
const DIFF_ROW_TINT: f32 = 0.16;

/// 行内词级区段的第二级语义色占比(#53 M3):比整行底色深一档,同一
/// 公式推导(35% vs 16%),GitHub word-diff 的「整行淡底 + 行内浓底」
/// 层级;不下划线(与超链接/拼写检查的视觉惯例冲突)。数值推断,真机
/// 目视留人工(见 #53 notes)。
const DIFF_INLINE_TINT: f32 = 0.35;

/// 双栏一侧(左 = 旧侧,右 = 新侧)。
enum DiffSide {
    /// 左栏:上下文行与删除行。
    Old,
    /// 右栏:上下文行与新增行。
    New,
}

/// 画双栏的一行:hunk 头跨整行(淡底 + `@@` 蓝),行对左右各画一侧。
/// 布局始终推进(行盒恒定),绘制做视口裁剪(`is_rect_visible`,与
/// `ui::gutter` 同精神)。返回行响应(悬停全文挂在其上,测试用它取行
/// 矩形)。
fn paint_split_row(ui: &mut egui::Ui, row: &SplitRow<'_>, geom: &RowGeometry) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, geom.row_height), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    match row {
        SplitRow::Header(hunk) => {
            ui.painter()
                .rect_filled(rect.shrink(0.5), RADIUS_SM, ui.visuals().faint_bg_color);
            ui.painter().text(
                egui::pos2(rect.left() + SPACE_XS, rect.center().y),
                egui::Align2::LEFT_CENTER,
                hunk_header_text(hunk),
                geom.font.clone(),
                DIFF_HUNK,
            );
        }
        SplitRow::Pair { old, new, inline } => {
            let half = rect.width() / 2.0;
            let left = egui::Rect::from_min_max(
                rect.left_top(),
                egui::pos2(rect.left() + half, rect.bottom()),
            );
            let right = egui::Rect::from_min_max(
                egui::pos2(rect.left() + half, rect.top()),
                rect.right_bottom(),
            );
            // 行内词级区段(#53 M3):配对层只对「单删+单增」紧邻对产出,
            // 两侧各自取本侧区间
            let (inline_old, inline_new) = match inline {
                Some(change) => (Some(&change.old), Some(&change.new)),
                None => (None, None),
            };
            let elided_old = paint_split_side(ui, left, *old, DiffSide::Old, inline_old, geom);
            let elided_new = paint_split_side(ui, right, *new, DiffSide::New, inline_new, geom);
            // 长行被截断的一侧:悬停看全文(整行响应挂 tooltip,两则并排)
            match (elided_old, elided_new) {
                (None, None) => {}
                (old_text, new_text) => {
                    let mut tooltip = String::new();
                    if let Some(text) = old_text {
                        tooltip.push_str(&format!("− {text}\n"));
                    }
                    if let Some(text) = new_text {
                        tooltip.push_str(&format!("+ {text}"));
                    }
                    return response.on_hover_text(tooltip.trim_end());
                }
            }
        }
    }
    response
}

/// 画双栏一侧:整侧底色(删除红/新增绿)+ 行号(右对齐弱化)+ 沟槽标记
/// (+/−)+ 正文(等宽、单行截断);本侧有行内词级区段(#53 M3)时,
/// 区段字符用更深的第二级底色(`TextFormat::background`,epaint 画在
/// 字形之下的 mesh 里)。返回被截断一侧的全文(悬停提示用;未截断/空侧
/// 返回 `None`)。行内不做 byte 切:截断由 epaint 的 elision 在字形层
/// 完成,区段切分由 [`inline_segments`] 在 char 边界完成。
fn paint_split_side(
    ui: &egui::Ui,
    cell: egui::Rect,
    line: Option<&DiffLine>,
    side: DiffSide,
    inline: Option<&std::ops::Range<usize>>,
    geom: &RowGeometry,
) -> Option<String> {
    let line = line?;
    // 配对层保证:左栏只收上下文/删除行,右栏只收上下文/新增行
    let (lineno, marker, tint) = match (side, line.kind) {
        (DiffSide::Old, DiffLineKind::Context) => (line.old_lineno, None, None),
        (DiffSide::Old, DiffLineKind::Deleted) => (
            line.old_lineno,
            Some(('−', DIFF_REMOVED)),
            Some(tokens::DANGER),
        ),
        (DiffSide::New, DiffLineKind::Context) => (line.new_lineno, None, None),
        (DiffSide::New, DiffLineKind::Added) => {
            (line.new_lineno, Some(('+', DIFF_ADDED)), Some(tokens::OK))
        }
        // 删除行画右栏/新增行画左栏:配对层不产生,防御跳过
        _ => return None,
    };
    let painter = ui.painter();
    if let Some(tint) = tint {
        painter.rect_filled(cell, 0.0, diff_row_bg(ui, tint));
    }
    // 行号:右对齐贴行号列右缘(空侧 = 空串,不画)
    let lineno_text = line_no_text(lineno);
    if !lineno_text.is_empty() {
        painter.text(
            egui::pos2(cell.left() + geom.gutter_w - SPACE_XS, cell.center().y),
            egui::Align2::RIGHT_CENTER,
            lineno_text,
            geom.font.clone(),
            geom.weak,
        );
    }
    // 沟槽标记:行号列右侧一格
    if let Some((marker, color)) = marker {
        painter.text(
            egui::pos2(cell.left() + geom.gutter_w, cell.center().y),
            egui::Align2::LEFT_CENTER,
            marker.to_string(),
            geom.font.clone(),
            color,
        );
    }
    // 正文:单行截断(…);悬停全文由调用方挂行响应。行内词级区段用带
    // background 的独立 section(前缀/区段/后缀三段),整行仍是同一
    // LayoutJob——行盒、elision、字形换行均与 M2 路径同构
    let text_width = (cell.width() - geom.gutter_w - geom.sign_w - SPACE_XS).max(0.0);
    let inline_range = inline.filter(|range| !range.is_empty());
    let galley = match (tint, inline_range) {
        (Some(tint), Some(range)) => {
            let (prefix, mid, suffix) = inline_segments(&line.text, range);
            let plain =
                egui::text::TextFormat::simple(geom.font.clone(), ui.visuals().text_color());
            let mut highlighted = plain.clone();
            highlighted.background = diff_tint_bg(ui, tint, DIFF_INLINE_TINT);
            let mut job = egui::text::LayoutJob::default();
            if !prefix.is_empty() {
                job.append(prefix, 0.0, plain.clone());
            }
            job.append(mid, 0.0, highlighted);
            if !suffix.is_empty() {
                job.append(suffix, 0.0, plain);
            }
            job.wrap = egui::text::TextWrapping::truncate_at_width(text_width);
            ui.fonts_mut(|fonts| fonts.layout_job(job))
        }
        _ => {
            let mut job = egui::text::LayoutJob::simple(
                line.text.clone(),
                geom.font.clone(),
                ui.visuals().text_color(),
                text_width,
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(text_width);
            ui.fonts_mut(|fonts| fonts.layout_job(job))
        }
    };
    let pos = egui::pos2(
        cell.left() + geom.gutter_w + geom.sign_w,
        cell.center().y - galley.size().y / 2.0,
    );
    let elided = galley.elided;
    painter.galley(pos, galley, ui.visuals().text_color());
    elided.then(|| line.text.clone())
}

/// hunk 头文本:`@@ -old_start,old_lines +new_start,new_lines @@`(与
/// unified 输出同格式;单行范围也带计数,不追 git 的省略逗号形态)。
fn hunk_header_text(hunk: &DiffHunk) -> String {
    format!(
        "@@ -{},{} +{},{} @@",
        hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
    )
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

/// 「正文 + 彩色单字母角标」的可选中行,文件树文件行与 Git 页改动行共用。
///
/// 单行截断(#40):两态(有/无角标)都走 `TextWrapMode::Truncate`——窄栏里
/// 名字省略号截尾不换行,行高恒定;角标经 `Button::right_text`(内部
/// `Atom::grow`)贴行尾,名字的截断宽度先扣除角标固有宽,省略号永远挤
/// 不掉角标。选中/悬停样式与 `ui.selectable_label` 同源——后者在 egui 0.36
/// 就是 `Button::selectable(..).ui(..)` 的别名,本函数只是同一颗 Button 多
/// 挂了截断与右侧原子。
fn badged_row_label(
    ui: &mut egui::Ui,
    selected: bool,
    text: String,
    badge: Option<StatusKind>,
) -> egui::Response {
    let mut button = egui::Button::selectable(selected, text).truncate();
    if let Some(kind) = badge {
        button =
            button.right_text(egui::RichText::new(format!("{kind}")).color(status_color(kind)));
    }
    ui.add(button)
}

/// 行 hover tooltip(#40):完整文件名——截断行由此看全名(recents 下拉的
/// 先例同款口径);带 Git 角标的行附状态说明,角标字母不必心算。
fn row_tooltip(name: &str, badge: Option<StatusKind>) -> String {
    match badge {
        Some(kind) => format!("{name}({})", status_label(kind)),
        None => name.to_owned(),
    }
}

/// Git 角标字母的中文说明(M/A/U/D/?)。
fn status_label(kind: StatusKind) -> &'static str {
    match kind {
        StatusKind::Modified => "已修改",
        StatusKind::Added => "已暂存",
        StatusKind::Unmerged => "冲突",
        StatusKind::Deleted => "已删除",
        StatusKind::Untracked => "未跟踪",
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

    /// 快照层护栏(PR #53 之前的缺陷回归):行首的展开三角、folder/file
    /// 图标必须是矢量 Shape,文本层只有名字(与 Git 角标);▸/▾ 之类图标
    /// 字符一旦回流文本层,断言当场红。同帧顺带量几何:装饰(三角 + 图标)
    /// 的右沿不得越过名字首字的墨迹左沿(`mesh_bounds` 不含前导空格,量
    /// 的是真实墨迹)。明暗两套 visuals 各跑一遍。
    #[test]
    fn tree_row_decorations_are_painted_not_glyph_text() {
        let (tree, root) = sample_tree();
        let children = tree.children.get(&root).unwrap();
        for dark in [true, false] {
            let ctx = egui::Context::default();
            ctx.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            for entry in &children.entries {
                let output = ctx.run_ui(RawInput::default(), |ui| {
                    tree_row(
                        ui,
                        &tree,
                        entry,
                        None,
                        &GitPanelState::default(),
                        &mut Vec::new(),
                    );
                });
                let mut texts = Vec::new();
                let mut decor_right = f32::MIN;
                let mut name_left = f32::MAX;
                for clipped in &output.shapes {
                    match &clipped.shape {
                        egui::epaint::Shape::Text(text) => {
                            texts.push(text.galley.job.text.clone());
                            name_left = name_left.min(text.visual_bounding_rect().left());
                        }
                        shape => {
                            decor_right = decor_right.max(shape.visual_bounding_rect().right())
                        }
                    }
                }
                assert!(
                    texts.iter().any(|t| t.contains(&entry.name)),
                    "{} 名字应仍在文本层:{texts:?}",
                    entry.name
                );
                assert!(
                    texts.iter().all(|t| {
                        !t.contains('▸')
                            && !t.contains('▾')
                            && !t.contains('📁')
                            && !t.contains('📄')
                    }),
                    "图标字符不得回流文本层:{texts:?}"
                );
                assert!(
                    decor_right < name_left,
                    "{} 行首装饰右沿 {decor_right} 不得压住名字墨迹左沿 {name_left}",
                    entry.name
                );
                output.drop_without_applying_deltas();
            }
        }
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

    /// 一帧里全部文本 shape 的字符串:可见文本与 tooltip 都算(断言按
    /// 内容区分,不区分层)。
    fn shape_texts(output: &egui::FullOutput) -> Vec<String> {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect()
    }

    /// 一帧里全部文本 shape 的**可见**字符串:按 glyph 层逐字拼接(#40 起
    /// 截断行走 elide 路径,`galley.job.text` 恒保留完整原文——Label 的
    /// elided-tooltip 正是靠它;省略号 `…` 在字形层是真实字符)。可见层
    /// 断言(截没截、截断后的样子)必须用这份,不能拿 job.text 恒真。
    fn visible_shape_texts(output: &egui::FullOutput) -> Vec<String> {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) => Some(
                    text.galley
                        .rows
                        .iter()
                        .map(|row| row.row.text())
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect()
    }

    /// recents 项显示名(#32 I2):只取末级文件夹名——注入带个人称谓
    /// 中间段的假路径,「坤哥」不得进显示名;重名末级在显示层不消歧
    /// (靠 tooltip 区分);根路径无末段时回退全路径(自身无中间段)。
    #[test]
    fn recent_label_uses_last_segment() {
        assert_eq!(recent_label(Path::new("/home/坤哥/工作/vault")), "vault");
        assert_eq!(
            recent_label(Path::new("/data/坤哥/备份/vault")),
            "vault",
            "重名末级在显示层不做消歧"
        );
        assert_eq!(recent_label(Path::new("/")), "/");
    }

    /// recents 下拉项的可见文本与 hover tooltip(#32 I2):无指针帧的
    /// 文本层只有末级名,路径中间段(坤哥/工作/备份)不得出现;悬停并
    /// 推过 egui 的 tooltip 延迟(默认 0.5s 且指针须静止,`RawInput.time`
    /// 逐帧推进)后,全路径只以 tooltip 文本出现,重名末级由此区分。
    #[test]
    fn recents_show_last_segment_with_full_path_on_hover() {
        let tree = FileTreeState {
            recents: vec![
                PathBuf::from("/home/坤哥/工作/vault"),
                PathBuf::from("/data/坤哥/备份/vault"),
            ],
            ..FileTreeState::default()
        };
        let ctx = egui::Context::default();
        let mut outbox = Vec::new();
        let rects = [Cell::new(Rect::NOTHING), Cell::new(Rect::NOTHING)];
        let render = |ui: &mut egui::Ui, outbox: &mut Vec<Message>| {
            for dir in &tree.recents {
                recent_row(ui, &tree, dir, outbox);
            }
        };

        // 帧 1(t=0,无指针):可见文本层只有末级名
        let output = ctx.run_ui(
            RawInput {
                time: Some(0.0),
                ..Default::default()
            },
            |ui| {
                for (dir, rect) in tree.recents.iter().zip(&rects) {
                    rect.set(recent_row(ui, &tree, dir, &mut outbox).rect);
                }
            },
        );
        let texts = shape_texts(&output);
        output.drop_without_applying_deltas();
        assert!(outbox.is_empty(), "仅渲染不产生消息");
        assert!(
            texts.iter().filter(|t| t.contains("vault")).count() >= 2,
            "两条 recents 都显示末级名:{texts:?}"
        );
        assert!(
            texts.iter().all(|t| !t.contains('坤')),
            "路径中间段不得进可见文本:{texts:?}"
        );

        // 帧 2(t=1):指针移入第一条;tooltip 有静止延迟,本帧不显示
        let center = rects[0].get().center();
        ctx.run_ui(
            RawInput {
                time: Some(1.0),
                events: vec![Event::PointerMoved(center)],
                ..Default::default()
            },
            |ui| render(ui, &mut outbox),
        )
        .drop_without_applying_deltas();

        // 帧 3-4(t=2,3):静止超延迟,悬停项的全路径只在 tooltip 里出现
        let mut tooltip_texts = Vec::new();
        for time in [2.0, 3.0] {
            let output = ctx.run_ui(
                RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ui| render(ui, &mut outbox),
            );
            tooltip_texts.extend(shape_texts(&output));
            output.drop_without_applying_deltas();
        }
        assert!(
            tooltip_texts
                .iter()
                .any(|t| t.contains("/home/坤哥/工作/vault")),
            "悬停项的全路径进 tooltip:{tooltip_texts:?}"
        );

        // 帧 5-7:指针移到第二条(重名末级),tooltip 换成它自己的全路径
        let center = rects[1].get().center();
        ctx.run_ui(
            RawInput {
                time: Some(4.0),
                events: vec![Event::PointerMoved(center)],
                ..Default::default()
            },
            |ui| render(ui, &mut outbox),
        )
        .drop_without_applying_deltas();
        let mut second_texts = Vec::new();
        for time in [5.0, 6.0] {
            let output = ctx.run_ui(
                RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ui| render(ui, &mut outbox),
            );
            second_texts.extend(shape_texts(&output));
            output.drop_without_applying_deltas();
        }
        assert!(
            second_texts
                .iter()
                .any(|t| t.contains("/data/坤哥/备份/vault")),
            "重名末级靠各自 tooltip 区分:{second_texts:?}"
        );
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
            match_span: 0..0,
        };
        let mut outbox = Vec::new();
        let rect = Cell::new(Rect::NOTHING);

        // 第一帧只渲染,拿行位置
        ctx.run_ui(RawInput::default(), |ui| {
            rect.set(search_row(ui, &hit, &mut outbox).rect);
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
                search_row(ui, &hit, &mut outbox);
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(
            outbox,
            vec![Message::SearchResultClicked(root.join("docs/note.md"), 7)]
        );
    }

    /// 命中按文件分组:连续段归一组,路径切换即开新组;空列表零组。
    /// (分组是结果列表文件头的数据源,乱序快照只会多几个文件头。)
    #[test]
    fn hit_groups_split_on_path_change() {
        let a = PathBuf::from("a.md");
        let b = PathBuf::from("b.md");
        let hit = |path: &Path, line_no: usize| SearchResult {
            path: path.to_path_buf(),
            line_no,
            line_text: String::new(),
            match_span: 0..0,
        };
        let hits = vec![hit(&a, 1), hit(&a, 3), hit(&b, 2), hit(&a, 5)];
        let groups = hit_groups(&hits);
        assert_eq!(
            groups
                .iter()
                .map(|(path, group)| (*path, group.len()))
                .collect::<Vec<_>>(),
            vec![(&*a, 2), (&*b, 1), (&*a, 1)],
            "分组按连续段切,乱序回落只是多几个文件头"
        );
        assert!(hit_groups(&[]).is_empty());
    }

    /// 侧栏搜索页一次完整渲染包含 VS Code 对齐的三个模式片、替换输入与
    ///「全部替换」图标入口(图标无文字,由句柄测试断言);交互消息的归约
    /// 链路由 state 测试覆盖。
    #[test]
    fn search_panel_renders_modes_and_replace_controls() {
        let ctx = egui::Context::default();
        let root = PathBuf::from("/vault");
        let mut search = SearchState {
            query: "needle".into(),
            replace_open: true,
            replace_with: "pin".into(),
            hits: vec![SearchResult {
                path: root.join("note.md"),
                line_no: 1,
                line_text: "needle".into(),
                match_span: 0..0,
            }],
            status: SearchStatus::Finished,
            ..SearchState::default()
        };
        let mut outbox = Vec::new();
        let mut replace_all_present = false;
        let output = ctx.run_ui(RawInput::default(), |ui| {
            let handles = search_controls(ui, &mut search, &mut outbox);
            replace_all_present = handles.replace_all.is_some();
        });
        let texts = shape_texts(&output);
        output.drop_without_applying_deltas();
        assert!(outbox.is_empty(), "仅渲染不产生消息");
        assert!(replace_all_present, "展开替换时应有「全部替换」图标按钮");
        for expected in ["needle", "Aa", "ab", ".*", "pin"] {
            assert!(
                texts.iter().any(|text| text.contains(expected)),
                "搜索页应渲染 {expected:?}, 实际文本:{texts:?}"
            );
        }
    }

    #[test]
    fn search_controls_stay_on_one_line_without_overlap() {
        for dark in [false, true] {
            for sidebar_width in [tokens::SIDEBAR_MIN_W, 240.0, 400.0] {
                for query in ["", "needle", "很长的搜索文本 Aa ab .* ".repeat(20).as_str()] {
                    let ctx = egui::Context::default();
                    ctx.set_visuals(if dark {
                        egui::Visuals::dark()
                    } else {
                        egui::Visuals::light()
                    });
                    let mut search = SearchState {
                        query: query.into(),
                        ..SearchState::default()
                    };
                    let mut outbox = Vec::new();
                    let mut rects = [Rect::NOTHING; 5];
                    // 包含 rail、面板右边距和滚动条的最窄内容预算。
                    let width = sidebar_width
                        - tokens::RAIL_W
                        - 8.0
                        - ctx
                            .style_of(if dark {
                                egui::Theme::Dark
                            } else {
                                egui::Theme::Light
                            })
                            .spacing
                            .scroll
                            .allocated_width();
                    let bounds =
                        Rect::from_min_size(egui::pos2(50.0, 20.0), egui::vec2(width, 200.0));
                    let output = ctx.run_ui(
                        RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(600.0, 400.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            let mut content = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
                            rects = search_controls(&mut content, &mut search, &mut outbox)
                                .row
                                .map(|response| response.rect);
                        },
                    );
                    for rect in rects {
                        assert!(rect.width() > 0.0, "控件有可用宽度: {rect:?}");
                        assert!(
                            rect.left() >= bounds.left() - 0.5
                                && rect.right() <= bounds.right() + 0.5,
                            "控件不得越出侧栏: {rect:?}, bounds={bounds:?}"
                        );
                        assert!(
                            (rect.center().y - rects[1].center().y).abs() < 1.0,
                            "所有控件应在同一行: {rects:?}"
                        );
                        assert!(
                            (rect.center().y - rects[1].center().y).abs() <= 0.5,
                            "垂直中心线必须重合(截图回归:按钮整体下移): {rects:?}"
                        );
                    }
                    for pair in rects.windows(2) {
                        assert!(
                            pair[0].right() <= pair[1].left(),
                            "控件有序且不重叠: {rects:?}"
                        );
                    }
                    assert!(rects[1].width() >= 20.0, "最窄侧栏仍可输入: {rects:?}");
                    for (label, rect) in ["Aa", "ab", ".*"].into_iter().zip(&rects[2..]) {
                        let text_rect = output
                            .shapes
                            .iter()
                            .find_map(|shape| match &shape.shape {
                                egui::epaint::Shape::Text(text)
                                    if text.galley.job.text == label =>
                                {
                                    let text_rect = text.visual_bounding_rect();
                                    assert!(
                                        shape.clip_rect.contains_rect(text_rect),
                                        "按钮文本未被裁剪: {label}"
                                    );
                                    Some(text_rect)
                                }
                                _ => None,
                            })
                            .expect("模式按钮文本可见");
                        assert!(
                            rect.contains_rect(text_rect),
                            "按钮文字不越出点击区域: {label}"
                        );
                    }
                    output.drop_without_applying_deltas();
                    assert!(outbox.is_empty());
                }
            }
        }
    }

    /// 展开替换后的两行布局(VS Code 同款):「全部替换」与替换框同一行,
    /// 折叠箭头居中在两行的正中。箭头高度走上一帧记忆,跑两帧收敛。
    #[test]
    fn search_replace_row_inline_and_arrow_centered() {
        let ctx = egui::Context::default();
        let mut search = SearchState {
            replace_open: true,
            ..SearchState::default()
        };
        let mut outbox = Vec::new();
        let mut arrow_y = None;
        let mut all_rect = None;
        let mut text_y: Vec<(String, f32)> = Vec::new();
        for _ in 0..2 {
            let output = ctx.run_ui(RawInput::default(), |ui| {
                let handles = search_controls(ui, &mut search, &mut outbox);
                arrow_y = Some(handles.row[0].rect.center().y);
                all_rect = handles.replace_all.map(|response| response.rect);
            });
            text_y = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) => {
                        let label = text.galley.job.text.as_str();
                        matches!(label, "搜索…" | "替换为…")
                            .then(|| (label.to_owned(), text.visual_bounding_rect().center().y))
                    }
                    _ => None,
                })
                .collect();
            output.drop_without_applying_deltas();
        }
        let arrow_y = arrow_y.expect("箭头 rect");
        let y_of = |label: &str| {
            text_y
                .iter()
                .find(|(text, _)| text == label)
                .map(|(_, y)| *y)
                .unwrap_or_else(|| panic!("{label} 应可见"))
        };
        let search_y = y_of("搜索…");
        let replace_y = y_of("替换为…");
        // 「全部替换」已是图标(无文字):断言图标按钮存在、与替换框同一行、
        // 且宽度是一格图标而非宽文字钮
        let all_rect = all_rect.expect("全部替换图标按钮应可见");
        assert!(
            (all_rect.center().y - replace_y).abs() < 1.5,
            "全部替换图标应与替换框同一行: 图标 y={}, 框 y={replace_y}",
            all_rect.center().y
        );
        assert!(
            all_rect.width() <= 30.0,
            "全部替换应为窄图标按钮而非宽文字钮: 宽 {}",
            all_rect.width()
        );
        assert!(
            (arrow_y - (search_y + replace_y) / 2.0).abs() <= 2.5,
            "箭头应居中于两行正中: 箭头 y={arrow_y}, 两行文字中点 {}",
            (search_y + replace_y) / 2.0
        );
        assert!(outbox.is_empty());
    }

    #[test]
    fn search_controls_chevron_click_toggles_replace() {
        let ctx = egui::Context::default();
        let mut search = SearchState::default();
        let mut outbox = Vec::new();
        let mut center = egui::Pos2::ZERO;
        for expected in [true, false] {
            ctx.run_ui(RawInput::default(), |ui| {
                center = search_controls(ui, &mut search, &mut outbox).row[0]
                    .rect
                    .center();
            })
            .drop_without_applying_deltas();
            ctx.run_ui(
                RawInput {
                    events: vec![
                        Event::PointerMoved(center),
                        Event::PointerButton {
                            pos: center,
                            button: PointerButton::Primary,
                            pressed: true,
                            modifiers: Default::default(),
                        },
                        Event::PointerButton {
                            pos: center,
                            button: PointerButton::Primary,
                            pressed: false,
                            modifiers: Default::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ui| {
                    search_controls(ui, &mut search, &mut outbox);
                },
            )
            .drop_without_applying_deltas();
            assert_eq!(search.replace_open, expected);
            assert!(outbox.is_empty(), "展开替换不触发搜索");
        }
    }

    /// 行内高亮(#199 追加):命中行渲染出「匹配段背景底」—— 帧的 shapes
    /// 里应出现 accent 色系矩形(强调色 22% 透明度);非命中段无底色。
    /// span 越界/零宽时整行退化无高亮,不 panic。
    #[test]
    fn search_row_paints_highlight_only_on_match() {
        let hit = |span: std::ops::Range<usize>| SearchResult {
            path: PathBuf::from("/vault/note.md"),
            line_no: 3,
            line_text: "找到 rust 语言".into(),
            match_span: span,
        };
        for (name, span, expect_bg) in [
            ("正常区间", 6..10, true),
            ("零宽区间", 0..0, false),
            ("越界区间", 999..1024, false),
        ] {
            let ctx = egui::Context::default();
            let h = hit(span);
            let output = ctx.run_ui(RawInput::default(), |ui| {
                let mut outbox = Vec::new();
                search_row(ui, &h, &mut outbox);
            });
            // 高亮底走 LayoutJob 的段 background(tessellate 前就在 Text
            // shape 的 galley 里),按「某段 background 非透明」判定;零宽/
            // 越界 span 的行没有任何带底段。
            let has_bg = output.shapes.iter().any(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return false;
                };
                text.galley
                    .job
                    .sections
                    .iter()
                    .any(|section| section.format.background != egui::Color32::TRANSPARENT)
            });
            output.drop_without_applying_deltas();
            assert_eq!(
                has_bg, expect_bg,
                "{name}: 高亮底 {has_bg}, 期望 {expect_bg}"
            );
        }
    }

    #[test]
    fn search_row_survives_the_real_sidebar_path() {
        for sidebar_width in [tokens::SIDEBAR_MIN_W, 240.0, 320.0] {
            let ctx = egui::Context::default();
            let (tree, _root) = sample_tree();
            let output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(sidebar_width + 200.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let mut open = true;
                    egui::Panel::left("nav-search-row")
                        .resizable(true)
                        .default_size(sidebar_width)
                        .size_range(tokens::SIDEBAR_MIN_W..=400.0)
                        .frame(
                            egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin {
                                left: 0,
                                ..egui::Margin::symmetric(8, 2)
                            }),
                        )
                        .show_collapsible(ui, &mut open, |ui| {
                            let mut search = SearchState {
                                query: "needle".into(),
                                ..SearchState::default()
                            };
                            let backlinks = BacklinkState::default();
                            let mut outbox = Vec::new();
                            let mut tab = SidebarTab::Search;
                            super::ui(
                                ui,
                                &mut tab,
                                &tree,
                                None,
                                OutlineView {
                                    items: &[],
                                    cursor_byte: None,
                                },
                                &mut search,
                                &GitPanelState::default(),
                                &backlinks,
                                &mut outbox,
                            );
                        });
                },
            );
            let mut centers: Vec<(String, f32)> = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Text(text)
                        if matches!(
                            text.galley.job.text.as_str(),
                            "needle" | "Aa" | "ab" | ".*"
                        ) =>
                    {
                        Some((
                            text.galley.job.text.clone(),
                            text.visual_bounding_rect().center().y,
                        ))
                    }
                    _ => None,
                })
                .collect();
            output.drop_without_applying_deltas();
            centers.sort_by(|a, b| a.0.cmp(&b.0));
            assert_eq!(
                centers.len(),
                4,
                "四个文本都应可见(搜索词 + 三枚按钮): {centers:?}"
            );
            let (_, base_y) = centers[0];
            for (label, y) in &centers {
                assert!(
                    (y - base_y).abs() < 1.0,
                    "「{label}」与搜索词不在一条线上(y={y}, 基准={base_y}, 侧栏 {sidebar_width}px)"
                );
            }
        }
    }

    #[test]
    fn search_panel_clicks_toggle_modes_and_request_replace() {
        let ctx = egui::Context::default();
        let root = PathBuf::from("/vault");
        let mut search = SearchState {
            query: "needle".into(),
            replace_open: true,
            hits: vec![SearchResult {
                path: root.join("note.md"),
                line_no: 1,
                line_text: "needle".into(),
                match_span: 0..0,
            }],
            status: SearchStatus::Finished,
            ..SearchState::default()
        };
        let mut outbox = Vec::new();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 600.0));
        // 全部替换已是图标按钮(无文字),单独立按返回句柄点击;其余按文本定位
        for target in ["Aa", "ab", ".*", "replace-all-icon"] {
            let mut center = None;
            for _ in 0..3 {
                let output = ctx.run_ui(
                    RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| {
                        let handles = search_controls(ui, &mut search, &mut outbox);
                        if target == "replace-all-icon" {
                            center = handles
                                .replace_all
                                .as_ref()
                                .map(|response| response.rect.center());
                        }
                    },
                );
                if target != "replace-all-icon" {
                    center = output.shapes.iter().find_map(|shape| match &shape.shape {
                        egui::epaint::Shape::Text(text) if text.galley.job.text == target => {
                            Some(text.visual_bounding_rect().center())
                        }
                        _ => None,
                    });
                }
                output.drop_without_applying_deltas();
            }
            let center = center.expect("按钮可点击位置");
            let events = vec![
                Event::PointerMoved(center),
                Event::PointerButton {
                    pos: center,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                Event::PointerButton {
                    pos: center,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ];
            ctx.run_ui(
                RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| {
                    search_controls(ui, &mut search, &mut outbox);
                },
            )
            .drop_without_applying_deltas();
            assert_eq!(
                outbox,
                vec![if target == "replace-all-icon" {
                    Message::SearchReplaceAllRequested
                } else {
                    Message::SearchQueryChanged
                }]
            );
            outbox.clear();
        }
        assert!(!search.case_insensitive);
        assert!(search.whole_word);
        assert!(search.regex);
    }

    /// 无引用/失败各渲染一帧不 panic,弱提示进文本层;有结果时来源文件名、
    /// 行号与摘要可见,超长摘要按字符截断加省略号。
    #[test]
    fn backlinks_panel_renders_states_without_panic() {
        let ctx = egui::Context::default();
        let root = PathBuf::from("/vault");
        let doc = root.join("note.md");

        // 渲染矩阵的单一场景:根/文档是否存在 + 面板状态 + 期望弱提示
        struct Case<'a> {
            name: &'static str,
            root: Option<&'a Path>,
            doc: Option<&'a Path>,
            state: BacklinkState,
            hint: &'static str,
        }
        let cases = vec![
            Case {
                name: "无根",
                root: None,
                doc: Some(&doc),
                state: BacklinkState::default(),
                hint: "反向链接需要一个根目录",
            },
            Case {
                name: "未落盘",
                root: Some(&root),
                doc: None,
                state: BacklinkState::default(),
                hint: "保存文档后",
            },
            Case {
                name: "扫描中",
                root: Some(&root),
                doc: Some(&doc),
                state: BacklinkState {
                    status: BacklinkStatus::Scanning,
                    ..BacklinkState::default()
                },
                hint: "扫描中",
            },
            Case {
                name: "防抖排程中",
                root: Some(&root),
                doc: Some(&doc),
                state: BacklinkState {
                    debounce_due: Some(
                        std::time::Instant::now() + std::time::Duration::from_secs(1),
                    ),
                    ..BacklinkState::default()
                },
                hint: "扫描中",
            },
            Case {
                name: "无引用",
                root: Some(&root),
                doc: Some(&doc),
                state: BacklinkState {
                    status: BacklinkStatus::Finished,
                    ..BacklinkState::default()
                },
                hint: "没有文档链接到这里",
            },
            Case {
                name: "失败",
                root: Some(&root),
                doc: Some(&doc),
                state: BacklinkState {
                    status: BacklinkStatus::Failed("线程没起来".to_owned()),
                    ..BacklinkState::default()
                },
                hint: "⚠ 线程没起来",
            },
        ];
        for case in cases {
            let output = ctx.run_ui(RawInput::default(), |ui| {
                backlinks_panel(ui, &case.state, case.root, case.doc, &mut Vec::new());
            });
            let texts = shape_texts(&output);
            output.drop_without_applying_deltas();
            assert!(
                texts.iter().any(|t| t.contains(case.hint)),
                "{}:弱提示应进文本层:{texts:?}",
                case.name
            );
        }

        // 有结果:来源路径:行号 + 摘要可见;超长摘要截断(带省略号)。
        let mut state = BacklinkState {
            status: BacklinkStatus::Finished,
            truncated: true,
            ..BacklinkState::default()
        };
        state.links.push(Backlink {
            path: PathBuf::from("sub/a.md"),
            line_no: 2,
            target: "note".to_owned(),
            line_text: "界".repeat(SNIPPET_MAX_CHARS + 30),
        });
        let output = ctx.run_ui(RawInput::default(), |ui| {
            backlinks_panel(ui, &state, Some(&root), Some(&doc), &mut Vec::new());
        });
        let texts = shape_texts(&output);
        output.drop_without_applying_deltas();
        assert!(
            texts.iter().any(|t| t.contains("sub/a.md:2")),
            "来源路径与行号可见:{texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains('…')),
            "超长摘要截断:{texts:?}"
        );
        assert!(
            texts
                .iter()
                .all(|t| t.matches('界').count() <= SNIPPET_MAX_CHARS),
            "长行的摘要按字符截断,不整段进文本层:{texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("上限")),
            "截断提示行可见:{texts:?}"
        );
    }

    /// 点击反向链接行:发出携带跳转目标串的 WikilinkClicked(来源相对
    /// 路径剥 md 后缀);仅渲染不产生消息。
    #[test]
    fn clicking_backlink_row_sends_wikilink_message() {
        let ctx = egui::Context::default();
        let link = Backlink {
            path: PathBuf::from("sub/b.md"),
            line_no: 1,
            target: "note".to_owned(),
            line_text: "子目录来源 [[note.md]]".to_owned(),
        };
        let mut outbox = Vec::new();
        let rect = Cell::new(Rect::NOTHING);

        ctx.run_ui(RawInput::default(), |ui| {
            rect.set(backlink_row(ui, &link, &mut outbox).rect);
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
                backlink_row(ui, &link, &mut outbox);
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(
            outbox,
            vec![Message::WikilinkClicked {
                target: "sub/b".to_owned()
            }],
            "点击走 WikilinkClicked,载荷是剥后缀的来源相对路径"
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
        let degraded = GitPanelState {
            error: Some("当前目录不是 Git 仓库".to_owned()),
            ..GitPanelState::default()
        };
        let empty = GitPanelState::default();
        let truncated = GitPanelState {
            truncated: 7,
            ..GitPanelState::default()
        };
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

    /// #53 M2 探针共享 fixture:1 个 hunk(上下文行 + 删除行 + 新增行)。
    /// 配对层产出 = hunk 头 + 上下文对 + (删除|新增) 同对。
    fn split_fixture() -> FileDiff {
        FileDiff {
            hunks: vec![DiffHunk {
                old_start: 1,
                old_lines: 2,
                new_start: 1,
                new_lines: 2,
                lines: vec![
                    DiffLine {
                        old_lineno: Some(1),
                        new_lineno: Some(1),
                        kind: DiffLineKind::Context,
                        text: "上下文".to_owned(),
                    },
                    DiffLine {
                        old_lineno: Some(2),
                        new_lineno: None,
                        kind: DiffLineKind::Deleted,
                        text: "旧".to_owned(),
                    },
                    DiffLine {
                        old_lineno: None,
                        new_lineno: Some(2),
                        kind: DiffLineKind::Added,
                        text: "新".to_owned(),
                    },
                ],
            }],
            binary: false,
            truncated: false,
        }
    }

    /// #53 M2 双栏渲染探针(像素采样,#38/#50 先例):已知 diff 逐行画进
    /// 无头帧,帧后 tessellate 取最终覆盖色——(删除|新增) 对左半红系、
    /// 右半绿系、上下文行两侧无整行底色、行号列有墨;行对数与行高恒定。
    /// 明暗两主题各跑一轮(不 panic + 语义都成立)。
    #[test]
    fn split_diff_rows_paint_semantic_backgrounds_in_both_themes() {
        use crate::preview_pixel_acceptance::{color_dist, final_covered_color};
        use egui::epaint::Mesh;
        use egui::UiBuilder;

        let file = split_fixture();
        let split = crate::git_split_diff::split_rows(&file);
        for dark in [true, false] {
            let ctx = egui::Context::default();
            ctx.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            // 关羽化(#41 同款):透明渐变边缘会污染采样读色
            ctx.options_mut(|o| o.tessellation_options.feathering = false);
            let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(360.0, 300.0));
            let mut rects: Vec<Rect> = Vec::new();
            let mut gutter_w = 0.0;
            let mut panel_fill = egui::Color32::BLACK;
            let mut output = ctx.run_ui(RawInput::default(), |panel| {
                panel_fill = panel.visuals().panel_fill;
                let mut ui = panel.new_child(UiBuilder::new().max_rect(screen));
                let geom = RowGeometry::of(&ui, &file);
                gutter_w = geom.gutter_w;
                for row in &split.rows {
                    rects.push(paint_split_row(&mut ui, row, &geom).rect);
                }
            });
            let clipped = std::mem::take(&mut output.shapes);
            let primitives = ctx.tessellate(clipped, 1.0);
            output.drop_without_applying_deltas();
            let meshes: Vec<&Mesh> = primitives
                .iter()
                .filter_map(|cp| match &cp.primitive {
                    egui::epaint::Primitive::Mesh(mesh) => Some(mesh),
                    _ => None,
                })
                .collect();
            let theme = if dark { "暗色" } else { "亮色" };

            // 行对数与行盒:1 hunk 头 + 上下文对 + (删除|新增) 对,行高恒定
            assert_eq!(rects.len(), 3, "{theme}:hunk 头 + 2 行对");
            let row_h = rects[0].height();
            assert!(row_h > 0.0);
            assert!(
                rects.iter().all(|r| r.height() == row_h),
                "{theme}:行盒恒定(长行走截断,不折行)"
            );

            // 底色采样:每半的近右缘(远离行号/沟槽/短正文,纯底色区)
            let sample = |p: egui::Pos2| final_covered_color(&meshes, p);
            let edge = |row: Rect, half: f32, right: bool| {
                let x = if right {
                    row.left() + 2.0 * half - 6.0
                } else {
                    row.left() + half - 6.0
                };
                sample(egui::pos2(x, row.center().y))
            };
            // 上下文对:两侧都不画整行底(露画布或面板底色)
            let context = rects[1];
            let half = context.width() / 2.0;
            for right in [false, true] {
                assert!(
                    edge(context, half, right).is_none_or(|c| color_dist(c, panel_fill) <= 12),
                    "{theme}:上下文行不该有语义底色({right:?})"
                );
            }
            // (删除|新增) 对:左半红系、右半绿系,都区别于面板底色
            let pair = rects[2];
            let half = pair.width() / 2.0;
            let left_bg = edge(pair, half, false).expect("删除侧画了整行底");
            let right_bg = edge(pair, half, true).expect("新增侧画了整行底");
            assert!(
                color_dist(left_bg, panel_fill) > 35,
                "{theme}:删除侧红系着色 {left_bg:?} vs {panel_fill:?}"
            );
            assert!(
                i32::from(left_bg.r()) - i32::from(left_bg.g()) > 8,
                "{theme}:删除侧偏红 {left_bg:?}"
            );
            assert!(
                color_dist(right_bg, panel_fill) > 35,
                "{theme}:新增侧绿系着色 {right_bg:?} vs {panel_fill:?}"
            );
            assert!(
                i32::from(right_bg.g()) - i32::from(right_bg.r()) > 8,
                "{theme}:新增侧偏绿 {right_bg:?}"
            );

            // 行号列有墨:(删除|新增) 对两侧行号槽(右对齐数字)至少一个采样
            // 点被文本覆盖,且颜色可与本侧底色区分
            for right in [false, true] {
                let cell_left = pair.left() + if right { half } else { 0.0 };
                let side_bg = if right { right_bg } else { left_bg };
                let mut ink = false;
                let mut x = cell_left + 4.0;
                while x <= cell_left + gutter_w - 2.0 {
                    if let Some(color) = sample(egui::pos2(x, pair.center().y)) {
                        if color_dist(color, side_bg) > 40 {
                            ink = true;
                            break;
                        }
                    }
                    x += 2.0;
                }
                assert!(ink, "{theme}:行号列应有墨(right={right})");
            }
        }
    }

    /// #53 M3 探针 fixture:上下文对 + 「单删+单增」对(公共前后缀各 4
    /// 字符,区段 xxxx/yyyy)+ 一对多块(2删1增,按 #101 降级为行级)。
    /// 两块之间有上下文行分隔(连续非上下文行是同一块)。
    fn inline_fixture() -> FileDiff {
        FileDiff {
            hunks: vec![DiffHunk {
                old_start: 1,
                old_lines: 6,
                new_start: 1,
                new_lines: 5,
                lines: vec![
                    DiffLine {
                        old_lineno: Some(1),
                        new_lineno: Some(1),
                        kind: DiffLineKind::Context,
                        text: "ctx".to_owned(),
                    },
                    DiffLine {
                        old_lineno: Some(2),
                        new_lineno: None,
                        kind: DiffLineKind::Deleted,
                        text: "AAAAxxxxBBBB".to_owned(),
                    },
                    DiffLine {
                        old_lineno: None,
                        new_lineno: Some(2),
                        kind: DiffLineKind::Added,
                        text: "AAAAyyyyBBBB".to_owned(),
                    },
                    DiffLine {
                        old_lineno: Some(3),
                        new_lineno: Some(3),
                        kind: DiffLineKind::Context,
                        text: "mid".to_owned(),
                    },
                    DiffLine {
                        old_lineno: Some(4),
                        new_lineno: None,
                        kind: DiffLineKind::Deleted,
                        text: "pp".to_owned(),
                    },
                    DiffLine {
                        old_lineno: Some(5),
                        new_lineno: None,
                        kind: DiffLineKind::Deleted,
                        text: "qq".to_owned(),
                    },
                    DiffLine {
                        old_lineno: None,
                        new_lineno: Some(4),
                        kind: DiffLineKind::Added,
                        text: "rr".to_owned(),
                    },
                ],
            }],
            binary: false,
            truncated: false,
        }
    }

    /// #53 M3 行内词级高亮渲染探针(顶点级,非像素采样):第二级底色以
    /// `TextFormat::background` 直接进 galley mesh,故从 tessellate 后的
    /// 顶点里按色找——①删/增两侧都出现第二级底色(公式与整行底色同源,
    /// 占比 [`DIFF_INLINE_TINT`]);②其 x 完全落在对应侧的正文区(前缀
    /// 字符之后、本侧右缘之前),不遮行号列/沟槽、不越双栏分界;③其 y
    /// 只出现在「单删+单增」对的行盒内(一对多块按 #101 降级,无第二级);
    /// ④行对数与行高恒定(区段不改变行对结构)。明暗两主题各跑一轮。
    #[test]
    fn split_inline_highlight_paints_second_level_only_on_unambiguous_pairs() {
        use crate::preview_pixel_acceptance::color_dist;
        use egui::epaint::Mesh;
        use egui::UiBuilder;

        let file = inline_fixture();
        let split = crate::git_split_diff::split_rows(&file);
        for dark in [true, false] {
            let ctx = egui::Context::default();
            ctx.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            ctx.options_mut(|o| o.tessellation_options.feathering = false);
            let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(360.0, 300.0));
            let mut rects: Vec<Rect> = Vec::new();
            let mut gutter_w = 0.0;
            let mut sign_w = 0.0;
            let mut digit = 0.0;
            let mut panel_fill = egui::Color32::BLACK;
            let mut inline_danger = egui::Color32::BLACK;
            let mut inline_ok = egui::Color32::BLACK;
            let mut output = ctx.run_ui(RawInput::default(), |panel| {
                panel_fill = panel.visuals().panel_fill;
                let mut ui = panel.new_child(UiBuilder::new().max_rect(screen));
                let geom = RowGeometry::of(&ui, &file);
                gutter_w = geom.gutter_w;
                sign_w = geom.sign_w;
                digit = ui.fonts_mut(|fonts| fonts.glyph_width(&geom.font, '0'));
                inline_danger = diff_tint_bg(&ui, tokens::DANGER, DIFF_INLINE_TINT);
                inline_ok = diff_tint_bg(&ui, tokens::OK, DIFF_INLINE_TINT);
                for row in &split.rows {
                    rects.push(paint_split_row(&mut ui, row, &geom).rect);
                }
            });
            let clipped = std::mem::take(&mut output.shapes);
            let primitives = ctx.tessellate(clipped, 1.0);
            output.drop_without_applying_deltas();
            let vertices: Vec<egui::epaint::Vertex> = primitives
                .iter()
                .filter_map(|cp| match &cp.primitive {
                    egui::epaint::Primitive::Mesh(Mesh { vertices, .. }) => {
                        Some(vertices.iter().copied())
                    }
                    _ => None,
                })
                .flatten()
                .collect();
            let theme = if dark { "暗色" } else { "亮色" };

            // ④ 行对结构:1 hunk 头 + 5 行对,行盒恒定
            assert_eq!(rects.len(), 6, "{theme}:hunk 头 + 5 行对");
            let row_h = rects[0].height();
            assert!(rects.iter().all(|r| r.height() == row_h));

            // 第二级底色顶点(按公式同源色匹配;首尾字形 expand_bg ±1px)
            let inline_of = |color: egui::Color32| {
                vertices
                    .iter()
                    .filter(|v| color_dist(v.color, color) <= 6)
                    .collect::<Vec<_>>()
            };
            let old_side = inline_of(inline_danger);
            let new_side = inline_of(inline_ok);
            // ① 两侧都有第二级底色
            assert!(!old_side.is_empty(), "{theme}:删除侧应有行内区段底色");
            assert!(!new_side.is_empty(), "{theme}:新增侧应有行内区段底色");
            // 第二级必须比整行第一级更深(距面板底色更远),层级可辨
            let row_danger = diff_row_bg_formula(panel_fill, tokens::DANGER);
            assert!(
                color_dist(inline_danger, panel_fill) > color_dist(row_danger, panel_fill) + 8,
                "{theme}:第二级应明显深于整行底色"
            );

            // 「单删+单增」对是第 3 行(rects[2]);一对多的两对在
            // rects[4]/[5](rects[3] 是块间上下文对)
            let pair = rects[2];
            let half = pair.width() / 2.0;
            for (side_name, side_verts, cell_left) in [
                ("旧", &old_side, pair.left()),
                ("新", &new_side, pair.left() + half),
            ] {
                // ③ y 只在该行盒内(±2px:background 向字形盒四周外扩
                // expand_bg=1px,贴边即越 1px):一对多块(rects[4]/[5])
                // 因此不可能混进第二级顶点
                let ys: Vec<f32> = side_verts.iter().map(|v| v.pos.y).collect();
                assert!(
                    ys.iter()
                        .all(|y| pair.top() - 2.0 <= *y && *y <= pair.bottom() + 2.0),
                    "{theme}:{side_name}侧第二级底色越出行盒"
                );
                // ② x 落在本侧正文区:前缀 AAAA(4 字符)之后、本侧右缘前,
                // 不遮行号列与沟槽(正文起点 = 行号列 + 沟槽之后)
                let text_x = cell_left + gutter_w + sign_w;
                let xs: Vec<f32> = side_verts.iter().map(|v| v.pos.x).collect();
                let min_x = xs.iter().cloned().fold(f32::INFINITY, f32::min);
                let max_x = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                assert!(
                    min_x > text_x + 2.0 * digit,
                    "{theme}:{side_name}侧区段应在前缀字符之后(min_x={min_x}, text_x={text_x})"
                );
                assert!(
                    max_x < text_x + 10.0 * digit,
                    "{theme}:{side_name}侧区段应不超过 4 字符宽(max_x={max_x})"
                );
                assert!(
                    max_x < cell_left + half,
                    "{theme}:{side_name}侧区段不得越过双栏分界"
                );
                assert!(min_x < max_x, "{theme}:{side_name}侧区段应有宽度");
            }

            // 一对多块的行(rects[4]/[5])整行第一级底色仍在(降级 = 保持
            // M2 行级,不是丢失着色):距面板底色足够远的语义色顶点存在
            let row_danger = diff_row_bg_formula(panel_fill, tokens::DANGER);
            for degraded in [rects[4], rects[5]] {
                let has_row_bg = vertices.iter().any(|v| {
                    color_dist(v.color, row_danger) <= 6
                        && degraded.top() <= v.pos.y
                        && v.pos.y <= degraded.bottom()
                        && v.pos.x < degraded.left() + half
                });
                assert!(
                    has_row_bg,
                    "{theme}:一对多块的删除侧应保持整行底色(M2 行级不降级丢失)"
                );
            }
        }
    }

    /// 测试内复算的整行第一级底色([`diff_row_bg`] 的公式镜像,不吃 &Ui)。
    fn diff_row_bg_formula(panel_fill: egui::Color32, tint: egui::Color32) -> egui::Color32 {
        tint.lerp_to_gamma(panel_fill, 1.0 - DIFF_ROW_TINT)
    }

    /// #53 M2 视图切换:双栏/统一两视图都渲染非空;结构化通道未就位
    /// (`diff_lines = None`)时双栏回落统一文本,错误文案在 diff 区仍可见
    /// (现状口径不破);空 hunks 双栏给「无文本改动」。
    #[test]
    fn diff_area_renders_nonempty_in_both_views_with_fallback() {
        use egui::UiBuilder;

        let mut git = GitPanelState::default();
        git.entries.push(FileStatus {
            path: "a.md".to_owned(),
            code: StatusKind::Modified,
        });
        git.selected = Some("a.md".to_owned());
        git.diff = "@@ -1,2 +1,2 @@\n-旧\n+新\n".to_owned();
        git.diff_lines = Some(split_fixture());

        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(360.0, 600.0));
        let mut heights = Vec::new();
        for view in [DiffView::Split, DiffView::Unified] {
            git.diff_view = view;
            let mut height = 0.0;
            ctx.run_ui(RawInput::default(), |panel| {
                let mut ui = panel.new_child(UiBuilder::new().max_rect(screen));
                diff_area(&mut ui, &git);
                height = ui.min_rect().height();
            })
            .drop_without_applying_deltas();
            heights.push(height);
        }
        assert!(heights[0] > 20.0, "双栏渲染非空:{}", heights[0]);
        assert!(heights[1] > 20.0, "统一渲染非空:{}", heights[1]);

        // 结构化取失败(diff_lines=None)+ diff=错误文案:回落统一文本渲染
        git.diff_view = DiffView::Split;
        git.diff_lines = None;
        git.diff = "生成 diff 失败: 病态仓库".to_owned();
        let mut height = 0.0;
        ctx.run_ui(RawInput::default(), |panel| {
            let mut ui = panel.new_child(UiBuilder::new().max_rect(screen));
            diff_area(&mut ui, &git);
            height = ui.min_rect().height();
        })
        .drop_without_applying_deltas();
        assert!(height > 10.0, "错误文案在双栏视图仍可见:{}", height);

        // 空 hunks(无文本改动):「无文本改动」占位,不 panic
        git.diff_lines = Some(FileDiff {
            hunks: Vec::new(),
            binary: false,
            truncated: false,
        });
        ctx.run_ui(RawInput::default(), |panel| {
            let mut ui = panel.new_child(UiBuilder::new().max_rect(screen));
            diff_area(&mut ui, &git);
        })
        .drop_without_applying_deltas();
    }

    /// #53 M2 切换按钮:点击「双栏/统一」发 [`Message::GitDiffViewChanged`],
    /// 当前态高亮由 `selectable_label` 自带;仅渲染不产消息。
    #[test]
    fn diff_view_toggle_clicks_send_messages() {
        let ctx = egui::Context::default();
        let mut outbox = Vec::new();
        let rects = Cell::new((Rect::NOTHING, Rect::NOTHING));

        // 帧 1:当前统一(双栏未选中)——仅渲染不产消息,顺带拿按钮矩形
        ctx.run_ui(RawInput::default(), |ui| {
            rects.set(diff_view_toggle(ui, DiffView::Unified, &mut outbox));
        })
        .drop_without_applying_deltas();
        assert!(outbox.is_empty(), "仅渲染不产生消息");
        let (split_rect, unified_rect) = rects.get();
        assert!(split_rect.width() > 0.0 && unified_rect.width() > 0.0);

        // 帧 2:点「双栏」→ GitDiffViewChanged(Split)
        let center = split_rect.center();
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
                diff_view_toggle(ui, DiffView::Unified, &mut outbox);
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(
            outbox,
            vec![Message::GitDiffViewChanged(DiffView::Split)],
            "{outbox:?}"
        );

        // 帧 3:点「统一」(当前已是双栏)→ GitDiffViewChanged(Unified)
        outbox.clear();
        let center = unified_rect.center();
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
                diff_view_toggle(ui, DiffView::Split, &mut outbox);
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(
            outbox,
            vec![Message::GitDiffViewChanged(DiffView::Unified)],
            "{outbox:?}"
        );
    }

    /// #53 M2 行对上限:超限的大 diff 双栏渲染不 panic(视口裁剪),显式
    /// 提示行非空;未超限且 M1 未截断时无提示。
    #[test]
    fn split_view_cap_renders_hint_and_big_diff_does_not_panic() {
        use egui::UiBuilder;

        // MAX+5 行上下文 → 配对层截到 MAX、dropped 5(纯层已钉,此处验渲染)
        let file = FileDiff {
            hunks: vec![DiffHunk {
                old_start: 1,
                old_lines: (MAX_SPLIT_PAIRS + 5) as u32,
                new_start: 1,
                new_lines: (MAX_SPLIT_PAIRS + 5) as u32,
                lines: (1..=(MAX_SPLIT_PAIRS + 5) as u32)
                    .map(|n| DiffLine {
                        old_lineno: Some(n),
                        new_lineno: Some(n),
                        kind: DiffLineKind::Context,
                        text: format!("行{n}"),
                    })
                    .collect(),
            }],
            binary: false,
            truncated: false,
        };
        let split = crate::git_split_diff::split_rows(&file);
        assert_eq!(split.dropped_pairs, 5, "纯层口径先验一把");

        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(360.0, 300.0));
        let mut height = 0.0;
        let mut hint = None;
        ctx.run_ui(RawInput::default(), |panel| {
            let mut ui = panel.new_child(UiBuilder::new().max_rect(screen));
            split_diff_view(&mut ui, &file);
            height = ui.min_rect().height();
            hint = split_overflow_hints(&mut ui, &file, split.dropped_pairs);
        })
        .drop_without_applying_deltas();
        assert!(height > 100.0, "2000 行对的大 diff 渲染出内容:{}", height);
        let hint = hint.expect("超限必有提示行");
        assert!(hint.width() > 0.0 && hint.height() > 0.0);

        // 未超限且 M1 未截断:无提示行
        let small = split_fixture();
        let mut none = None;
        ctx.run_ui(RawInput::default(), |panel| {
            let mut ui = panel.new_child(UiBuilder::new().max_rect(screen));
            none = split_overflow_hints(&mut ui, &small, 0);
        })
        .drop_without_applying_deltas();
        assert!(none.is_none(), "未截断不该有提示");
    }

    /// #40 单行截断:窄栏(120px)下长中文文件名行不换行——行高与同栏
    /// 短名目录行、宽栏同名行一致恒定,名字文本以省略号截尾且全名不进
    /// 可见层;Git 角标仍可见、画在名字右侧且贴行尾(名字的截断宽度先
    /// 扣除角标固有宽,省略号挤不掉角标);宽栏下全名完整显示。
    #[test]
    fn tree_row_truncates_long_names_in_narrow_panel() {
        let long_name = format!("{}笔记.md", "超长中文文件名".repeat(3));
        let root = PathBuf::from("/vault");
        let mut tree = FileTreeState {
            root: Some(root.clone()),
            ..FileTreeState::default()
        };
        let dir = TreeEntry {
            path: root.join("docs"),
            name: "docs".to_owned(),
            is_dir: true,
        };
        let file = TreeEntry {
            path: root.join(&long_name),
            name: long_name.clone(),
            is_dir: false,
        };
        tree.children.insert(
            root.clone(),
            DirChildren {
                entries: vec![dir, file.clone()],
                truncated: 0,
            },
        );
        let mut git = GitPanelState::default();
        git.badges.insert(file.path.clone(), StatusKind::Modified);

        let run = |width: f32| {
            let ctx = egui::Context::default();
            let rects = Cell::new((Rect::NOTHING, Rect::NOTHING));
            let output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 400.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let children = tree.children.get(&root).unwrap();
                    let outbox = &mut Vec::new();
                    let dir_rect = tree_row(ui, &tree, &children.entries[0], None, &git, outbox);
                    let file_rect = tree_row(ui, &tree, &children.entries[1], None, &git, outbox);
                    rects.set((dir_rect.rect, file_rect.rect));
                },
            );
            // 可见文本按 glyph 层拼接;文本矩形按可见文本匹配(job.text 恒含
            // 完整原文,拿它匹配会把被 elide 掉的名字也算「画出来了」)
            let texts = visible_shape_texts(&output);
            let text_rect = |pred: &dyn Fn(&str) -> bool| -> Rect {
                let mut found = Rect::NOTHING;
                for clipped in &output.shapes {
                    if let egui::epaint::Shape::Text(text) = &clipped.shape {
                        let visible: String =
                            text.galley.rows.iter().map(|row| row.row.text()).collect();
                        if pred(&visible) {
                            found = text.visual_bounding_rect();
                        }
                    }
                }
                found
            };
            let badge = text_rect(&|t: &str| t.trim() == "M");
            let truncated_name = text_rect(&|t: &str| t.trim_start().starts_with("超长"));
            output.drop_without_applying_deltas();
            (texts, rects.get(), badge, truncated_name)
        };

        // —— 窄栏 120px:名字只容得下前几个字,省略号截尾 ——
        let (narrow_texts, (narrow_dir, narrow_file), badge, truncated_name) = run(120.0);
        let name = narrow_texts
            .iter()
            .find(|t| t.trim_start().starts_with("超长"))
            .expect("长名字应仍在文本层");
        assert!(name.ends_with('…'), "窄栏名字以省略号截尾:{name:?}");
        assert!(
            !name.contains(&long_name),
            "全名不得整体进窄栏可见层:{name:?}"
        );
        assert!(
            narrow_texts.iter().any(|t| t.trim() == "M"),
            "角标仍可见:{narrow_texts:?}"
        );
        // 行高恒定:目录短名行 == 文件长名行(长名不再换行撑高)
        assert!(
            (narrow_dir.height() - narrow_file.height()).abs() < 0.5,
            "窄栏行高恒定:目录 {narrow_dir:?} vs 文件 {narrow_file:?}"
        );
        // 角标贴行尾:画在名字右侧,距行右沿只剩按钮内边距量级
        assert!(badge != Rect::NOTHING && truncated_name != Rect::NOTHING);
        assert!(
            badge.left() > truncated_name.right(),
            "角标在名字右侧:角标 {badge:?} 名字 {truncated_name:?}"
        );
        assert!(
            badge.right() <= narrow_file.right() && narrow_file.right() - badge.right() < 12.0,
            "角标贴行尾:角标右沿 {} 行右沿 {}",
            badge.right(),
            narrow_file.right()
        );

        // —— 宽栏 500px:全名完整,行高与窄栏一致(不随栏宽/名字长度变化)——
        let (wide_texts, (_, wide_file), _, _) = run(500.0);
        let full = wide_texts
            .iter()
            .find(|t| t.contains(&long_name))
            .expect("宽栏应显示全名");
        assert!(!full.ends_with('…'), "宽栏不截断:{full:?}");
        assert!(
            (wide_file.height() - narrow_file.height()).abs() < 0.5,
            "行高不随栏宽变化:宽 {wide_file:?} 窄 {narrow_file:?}"
        );
    }

    /// #40 hover tooltip:窄栏截断行的完整文件名只出现在 tooltip 文本层
    /// (`everything_is_visible` 是 egui 自己的 UI 测试手法,免 tooltip 延迟,
    /// 同 top_actions 先例);带 Git 角标的行 tooltip 附状态说明。
    #[test]
    fn hovering_truncated_tree_row_shows_full_name_tooltip() {
        let long_name = format!("{}笔记.md", "超长中文文件名".repeat(3));
        let root = PathBuf::from("/vault");
        let mut tree = FileTreeState {
            root: Some(root.clone()),
            ..FileTreeState::default()
        };
        let file = TreeEntry {
            path: root.join(&long_name),
            name: long_name.clone(),
            is_dir: false,
        };
        tree.children.insert(
            root.clone(),
            DirChildren {
                entries: vec![file.clone()],
                truncated: 0,
            },
        );
        let mut git = GitPanelState::default();
        git.badges.insert(file.path.clone(), StatusKind::Modified);

        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(120.0, 400.0));
        let render = |ui: &mut egui::Ui| {
            let children = tree.children.get(&root).unwrap();
            tree_row(ui, &tree, &children.entries[0], None, &git, &mut Vec::new())
        };
        let ctx = egui::Context::default();
        let row_rect = Cell::new(Rect::NOTHING);
        ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| row_rect.set(render(ui).rect),
        )
        .drop_without_applying_deltas();
        let center = row_rect.get().center();

        ctx.memory_mut(|mem| mem.set_everything_is_visible(true));
        // 两帧:hover 判定用上一帧的指针位置,tooltip 在下一帧才渲染。
        // 断言用**可见**文本:行自身的名字在 120px 下被 elide,可见层没有
        // 全名,全名只能来自 tooltip 的字形(job.text 恒留原文,拿它断言恒真)
        let mut texts = Vec::new();
        for _ in 0..2 {
            let output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    events: vec![Event::PointerMoved(center)],
                    ..Default::default()
                },
                |ui| {
                    render(ui);
                },
            );
            texts = visible_shape_texts(&output);
            output.drop_without_applying_deltas();
        }
        assert!(
            texts.iter().any(|t| t.contains(&long_name)),
            "截断行的全名进 tooltip:{texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("已修改")),
            "带角标行 tooltip 附状态说明:{texts:?}"
        );
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

    /// 视图 tab 顺序 helper:一枚占 `NAV_TAB_H` 宽,自左向右不重叠;第 5
    /// 枚是反向链接(#15,追加在尾部不打乱既有四枚)。
    ///
    /// **2026-10-08 随 `view_nav` 改单行而从 y 轴改为 x 轴**:横排后
    /// 「第 N 枚的 y」恒等于段中线,已无区分度,能区分的只有 x。
    #[test]
    fn nav_tab_center_x_follows_tab_order() {
        let size = crate::ui::tokens::NAV_TAB_H;
        let pitch = size + crate::ui::tokens::NAV_TAB_GAP;
        assert_eq!(
            nav_tab_center(0.0, 0.0, SidebarTab::Outline),
            egui::pos2(size * 0.5, size * 0.5),
            "第一枚贴着左缘"
        );
        assert_eq!(
            nav_tab_center(0.0, 0.0, SidebarTab::Git).x,
            size * 0.5 + pitch * 3.0,
            "Git 是第四枚"
        );
        assert_eq!(
            nav_tab_center(0.0, 0.0, SidebarTab::Backlinks).x,
            size * 0.5 + pitch * 4.0,
            "反向链接是第五枚"
        );
        let xs = SidebarTab::ALL.map(|tab| nav_tab_center(0.0, 0.0, tab).x);
        assert!(
            xs.windows(2).all(|pair| pair[1] > pair[0]),
            "五枚自左向右:{xs:?}"
        );
    }

    /// 五枚 tab 一行放得下(窄栏下限 180px 也不换行/不裁切)—— 这是
    /// 「nav 段只有 26px」这个改版的**前提断言**:放不下就得退回竖排,
    /// 省下 104px 的收益随之归零。
    #[test]
    fn five_nav_tabs_fit_the_narrowest_sidebar() {
        let pitch = crate::ui::tokens::NAV_TAB_H + crate::ui::tokens::NAV_TAB_GAP;
        let total = crate::ui::tokens::NAV_TAB_H * 5.0 + crate::ui::tokens::NAV_TAB_GAP * 4.0;
        assert!(
            total <= crate::ui::tokens::SIDEBAR_MIN_W,
            "五 tab 合计 {total}px 须 ≤ 窄栏下限 {}px(pitch={pitch})",
            crate::ui::tokens::SIDEBAR_MIN_W
        );
    }

    /// **2026-10-08 第二次改版后的两位式骨架**:rail 占左端一条纵带、
    /// body 吃掉余下全部宽度与高度,顶部**零** chrome。
    ///
    /// 取代了原「三段自上而下」的断言(顶段 / nav / body),因为那一版的
    /// 顶段与 nav 段都已迁入 rail —— 现在左栏里不存在任何横向条带。
    /// 新断言锁的是这次改版的**收益**:body 必须一路顶到面板顶端
    /// (top = sidebar.top),任何「又加回来一行 chrome」的回退都会被拦下。
    #[test]
    fn rail_and_body_split_the_sidebar_left_to_right() {
        let ctx = egui::Context::default();
        let (tree, _root) = sample_tree();
        let mut bands = None;
        let sidebar = Cell::new(Rect::NOTHING);
        ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(crate::ui::tokens::SIDEBAR_MIN_W, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                sidebar.set(ui.max_rect());
                bands = Some(ui_whole(
                    ui,
                    SidebarTab::Files,
                    &tree,
                    &GitPanelState::default(),
                ));
            },
        )
        .drop_without_applying_deltas();
        let sidebar = sidebar.get();
        let bands = bands.unwrap();

        // rail 贴左缘、宽恰 RAIL_W、纵贯到底。
        assert_eq!(
            bands.nav.left(),
            sidebar.left(),
            "rail 贴着左栏左缘(旧语义=nav 段,现承载 rail)"
        );
        // 上界**硬编码 56px**,刻意不复用 `RAIL_W`:带宽由 `rail_strip`
        // 报告,与绘制同源 —— 若上界也读 token,把 RAIL_W 调大会让绘制与
        // 断言一起变宽 → 断言自我满足(2026-10-08 实测把 RAIL_W 改成 96
        // 模拟回退,断言照样绿)。硬编码让「rail 不该宽于 ~56px」成为独立
        // 于实现的承诺:rail 是贴边的图标带,宽度膨胀就是在挤内容。
        const RAIL_LIMIT_PX: f32 = 56.0;
        assert!(
            bands.nav.width() <= RAIL_LIMIT_PX,
            "rail 宽 {}px 须 ≤ {RAIL_LIMIT_PX}px(贴边图标带不该再宽):{:?}",
            bands.nav.width(),
            bands.nav
        );
        assert!(
            bands.nav.bottom() >= sidebar.bottom() - 1.0,
            "rail 纵贯到左栏底部:{:?} vs {:?}",
            bands.nav,
            sidebar
        );

        // body 紧接 rail 右缘、且高度足饇:竖直方向的 chrome 已全部消失。
        // 这是本次改版的收益断言 —— 旧版里 body.top 被顶段(31px)+ nav
        // (26px)压到 y≈57 之下。
        assert!(
            (bands.body.left() - bands.nav.right()).abs() < 1.0,
            "body 紧接 rail 右缘:body.left={} rail.right={}",
            bands.body.left(),
            bands.nav.right()
        );
        const TOP_CHROME_LIMIT_PX: f32 = 2.0;
        assert!(
            bands.body.top() - sidebar.top() <= TOP_CHROME_LIMIT_PX,
            "左栏顶部 chrome 归零:body.top={} sidebar.top={}(差 {}px,须 ≤ \
             {TOP_CHROME_LIMIT_PX})",
            bands.body.top(),
            sidebar.top(),
            bands.body.top() - sidebar.top(),
        );

        // top 段已无意义(无横向条带),钉成退化矩形以免有人误读它。
        assert_eq!(
            bands.top.height(),
            0.0,
            "顶段已随 2026-10-08 改版消失,退化为零高矩形"
        );
    }

    /// **2026-10-09 坤哥截图反馈「距左侧边距太宽」的守卫**。
    ///
    /// 上面那条 `rail_and_body_split_the_sidebar_left_to_right` 直驱
    /// `sidebar::ui`,**绕过了承载它的 `Panel::left("nav")`** —— 所以它
    /// 只能证明「rail 贴着左栏内容区的左缘」,证明不了「贴着窗口左缘」。
    /// 而坤哥看到的那 7px 白边,恰恰来自 panel 自己的 `inner_margin.left`:
    /// egui `Frame::side_top_panel` 默认 `symmetric(8,2)`。
    ///
    /// 本测试把真实的 nav panel 组合跑一遍,断言 **rail 带左缘 == 窗口
    /// 左缘**。这样任何人给 nav panel 加回 margin(或改回默认 frame)
    /// 都会当场红 —— 那段白边是「让人觉得图标浮在内容里、而不是贴窗口边」
    /// 的直接原因,必须有人守。
    ///
    /// 上界同样**硬编码**(3px 容差),刻意不从 `Margin` 反推:若把
    /// margin 从 0 调回 8 而断言也跟着读同一个数,rail.left() 与 window
    /// 左缘会一起右移,断言自我满足(2026-10-08 那次 RAIL_W 的教训)。
    #[test]
    fn nav_panel_has_no_left_margin_so_rail_touches_the_window_edge() {
        let ctx = egui::Context::default();
        let (tree, _root) = sample_tree();
        let mut bands = None;
        let window = Cell::new(Rect::NOTHING);
        ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                window.set(ui.max_rect());
                // 复刻 layout.rs 里 nav panel 的构造。刻意**重新**写一遍
                // 而不是调用生产函数:生产那条路径要整个 `LaterMdApp`,
                // 拉进来会让这条断言变成集成测试,失去定位力。
                // 两处若不同步,本测试会红 —— 这正是要的耦合。
                let mut open = true;
                egui::Panel::left("nav探头")
                    .resizable(true)
                    .default_size(240.0)
                    .size_range(crate::ui::tokens::SIDEBAR_MIN_W..=400.0)
                    .frame(
                        egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin {
                            left: 0,
                            ..egui::Margin::symmetric(8, 2)
                        }),
                    )
                    .show_collapsible(ui, &mut open, |ui| {
                        bands = Some(ui_whole(
                            ui,
                            SidebarTab::Files,
                            &tree,
                            &GitPanelState::default(),
                        ));
                    });
            },
        )
        .drop_without_applying_deltas();
        let window = window.get();
        let bands = bands.unwrap();

        // 窗口左缘是 ui.max_rect().left()(= 0)。rail 必须从这里开始。
        const FLUSH_LIMIT_PX: f32 = 3.0;
        assert!(
            (bands.nav.left() - window.left()).abs() <= FLUSH_LIMIT_PX,
            "rail 必须与窗口左缘齐平:{:?} vs 窗口 {:?}(差 {}px,须 ≤ \
             {FLUSH_LIMIT_PX}px)。panel 的 inner_margin.left 会把 rail \
             推离窗边、让图标看着浮在内容里",
            bands.nav,
            window,
            bands.nav.left() - window.left(),
        );

        // 顺带钉住带宽:`RAIL_W` 2026-10-09 由 48 收到 40。上界硬编码,
        // 理由同上条的 56 —— 不读 token,否则两者一起变宽就抓不到回退。
        const RAIL_LIMIT_PX: f32 = 44.0;
        assert!(
            bands.nav.width() <= RAIL_LIMIT_PX,
            "rail 宽 {}px 须 ≤ {RAIL_LIMIT_PX}px(贴边图标带不该再宽):{:?}",
            bands.nav.width(),
            bands.nav
        );
    }

    /// 点 rail 上「搜索」tab:走完整 `sidebar::ui` 多帧请求,只发一条
    /// `SidebarTabChanged`。
    ///
    /// 直接驱 `sidebar::ui`(而非孤立 tab)是有意的:rail 上的五枚 tab
    /// 与 body 的 ScrollArea 都在左栏这一层,只测孤立按钮会漏掉「谁抢走了
    /// 这次点击」—— 三栏重排时已经在标题栏上踩过一次。
    ///
    /// **2026-10-08 改竖排**:目标点改用 [`rail_item_rect`] 现算(第 2 枚),
    /// 与绘制同源 —— 旧版用横排的 `nav_tab_center` 递推 x,rail 里那样算
    /// 出来的点会落在全然不同的按钮上(实测点到第 1 枚 Files)。
    #[test]
    fn clicking_nav_tab_switches_view() {
        let (tree, _root) = sample_tree();
        let ctx = egui::Context::default();
        let mut outbox = Vec::new();
        let mut active = SidebarTab::Files;
        let mut search = SearchState::default();
        let git = GitPanelState::default();
        let rail = Cell::new(Rect::NOTHING);
        let screen = Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(crate::ui::tokens::SIDEBAR_MIN_W, 600.0),
        );

        ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                rail.set(ui_whole(ui, SidebarTab::Files, &tree, &git).nav);
            },
        )
        .drop_without_applying_deltas();

        // rail 第 3 枚 = 上组的「搜索」(SidebarTab::ALL 的第 3 项)。
        let rail = rail.get();
        let target = rail_item_rect(rail, 2).center();
        assert!(
            rail.contains(target),
            "目标点须落在 rail 带内:{target:?} vs {rail:?}"
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
                    screen_rect: Some(screen),
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
            "rail 第 3 枚是「搜索」:应只发这一条切换消息"
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
        let backlinks = BacklinkState::default();
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
            &backlinks,
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
        let backlinks = BacklinkState::default();
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
            &backlinks,
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
        for cmd in Command::FILE
            .iter()
            .copied()
            .chain([Command::ExportHtml, Command::ExportPdf])
        {
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

    /// 文件栏根目录行:三枚图标按钮各自点得动,消息不串;且在**最窄**
    /// 侧栏里三枚都还在可视范围内(旧实现是一个文字按钮,加到三枚后才有
    /// 宽度预算这回事)。
    ///
    /// 几何断言按 [`tokens::ICON`] 与 `TOOLBAR_H` 独立算,不复用
    /// `icon_button` 内部的尺寸表达式 —— 那样断言就变成自我满足。
    #[test]
    fn root_row_icon_buttons_send_their_own_messages() {
        let root = PathBuf::from("/vault");
        let docs = root.join("docs");
        for width in [tokens::SIDEBAR_MIN_W, 240.0, 400.0] {
            let mut tree = FileTreeState {
                root: Some(root.clone()),
                recents: vec![root.clone()],
                ..FileTreeState::default()
            };
            tree.children.insert(root.clone(), DirChildren::default());
            let mut outbox = Vec::new();
            let mut rects: [Rect; 3] = [Rect::NOTHING; 3];
            let bounds = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(width, 200.0));
            let ctx = egui::Context::default();

            let render = |ui: &mut egui::Ui, outbox: &mut Vec<Message>, slots: &mut [Rect; 3]| {
                let mut content = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
                let handles = root_row(
                    &mut content,
                    &tree,
                    Some(docs.join("note.md").as_path()),
                    outbox,
                );
                for (slot, response) in slots.iter_mut().zip([
                    handles.pick.expect("选目录按钮在场"),
                    handles.new.expect("新建按钮在场"),
                    handles.refresh.expect("刷新按钮在场"),
                ]) {
                    *slot = response.rect;
                }
            };

            ctx.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width + 40.0, 400.0),
                    )),
                    ..Default::default()
                },
                |ui| render(ui, &mut outbox, &mut rects),
            )
            .drop_without_applying_deltas();
            assert!(outbox.is_empty(), "仅渲染不产生消息");

            for rect in rects {
                assert!(rect.width() > 0.0, "按钮有可用宽度:{rect:?}");
                assert!(
                    rect.left() >= bounds.left() - 0.5 && rect.right() <= bounds.right() + 0.5,
                    "按钮不得越出侧栏(宽 {width}):{rect:?},bounds={bounds:?}"
                );
                assert!(
                    (rect.height() - crate::ui::tokens::TOOLBAR_H).abs() < 0.5,
                    "按钮高取工具栏档位:{rect:?}"
                );
            }
            for pair in rects.windows(2) {
                assert!(pair[0].right() <= pair[1].left(), "有序不重叠:{rects:?}");
            }

            let click_at = |rect: Rect| {
                let center = rect.center();
                let click = |pressed| Event::PointerButton {
                    pos: center,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                };
                vec![Event::PointerMoved(center), click(true), click(false)]
            };

            // 三枚按钮各点一次,消息必须是三条互不相同的
            let clicked: Vec<Rect> = rects.to_vec();
            for (rect, expected) in clicked.iter().zip([
                Message::FileTreeRootPick,
                Message::FileTreeNewFileRequested(docs.clone()),
                Message::FileTreeRefreshRequested,
            ]) {
                outbox.clear();
                ctx.run_ui(
                    RawInput {
                        events: click_at(*rect),
                        ..Default::default()
                    },
                    |ui| render(ui, &mut outbox, &mut rects),
                )
                .drop_without_applying_deltas();
                assert_eq!(
                    outbox,
                    vec![expected],
                    "点 {rect:?} 只发它自己那条消息,不得串"
                );
            }
        }
    }

    /// 「新建」的目标目录口径:当前文档所在目录优先;没有落盘文档才回落
    /// 树根;两者皆无返回 `None`(按钮不得发消息)。
    #[test]
    fn new_file_target_prefers_current_doc_dir() {
        let root = Path::new("/vault");
        assert_eq!(
            new_file_target(Some(Path::new("/vault/docs/deep/note.md")), Some(root)),
            Some(PathBuf::from("/vault/docs/deep")),
            "当前文档所在目录优先于树根"
        );
        assert_eq!(
            new_file_target(None, Some(root)),
            Some(PathBuf::from("/vault")),
            "无落盘文档回落树根"
        );
        assert_eq!(new_file_target(None, None), None, "两者皆无则不发消息");
    }

    /// 刷新反馈窗口内:刷新按钮的**命中矩形与空闲态全等**(外壳拆得对),
    /// 且**仍可点**(禁用会让按钮灰掉像坏了,而刷新途中再刷一次是合理诉求)。
    ///
    /// 矩形全等这条挡的是「忙碌态换了控件导致手感/占位都变了」:两处各写
    /// 一遍外壳时,改 hover 底或尺寸很容易只改一边。
    #[test]
    fn refresh_button_keeps_geometry_and_stays_clickable_while_busy() {
        let root = PathBuf::from("/vault");
        let mut idle_tree = FileTreeState {
            root: Some(root.clone()),
            recents: vec![root.clone()],
            ..FileTreeState::default()
        };
        idle_tree
            .children
            .insert(root.clone(), DirChildren::default());
        // 反馈窗口刚开:与空闲态唯一的差别就是 `refresh_started`
        let busy_tree = FileTreeState {
            root: idle_tree.root.clone(),
            recents: idle_tree.recents.clone(),
            expanded: idle_tree.expanded.clone(),
            children: idle_tree.children.clone(),
            new_file: None,
            refresh_started: Some(std::time::Instant::now()),
        };

        let bounds = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(240.0, 200.0));
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(280.0, 400.0));
        let raw = |events: Vec<Event>| RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        // `screen_rect` 每一轮都给成同一个:它决定 `available_width`,
        // 进而决定目录名区宽度与三枚按钮的 x。少给这一项,点击就落在
        // 按钮之外,断言会以「按钮点不动」的假象红 —— 那是在测布局漂移。
        let run = |ctx: &egui::Context,
                   tree: &FileTreeState,
                   outbox: &mut Vec<Message>,
                   events: Vec<Event>,
                   slot: &mut Rect| {
            let mut out = ctx.run_ui(raw(events), |ui| {
                let mut content = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
                *slot = root_row(&mut content, tree, None, outbox)
                    .refresh
                    .expect("刷新按钮在场")
                    .rect;
            });
            // 要读下一轮的 rect,不能一丢了之;`clear` 是
            // `drop_without_applying_deltas` 的同一件事(直接 drop 未应用
            // 的 textures delta 会 panic)。
            out.textures_delta.clear();
        };

        // **必须复用同一个 ctx**:egui 冷启动的首帧不处理交互,点击要到
        // 下一帧才生效(既有 `root_row_icon_buttons_*` 测试同款)。
        // 每轮都 `Context::default()` 的话,点的那帧就是冷首帧,断言会以
        // 「按钮点不动」的假象红 —— 测的其实是 ctx 的预热,不是按钮。
        let ctx = egui::Context::default();
        let mut idle_rect = Rect::NOTHING;
        run(
            &ctx,
            &idle_tree,
            &mut Vec::new(),
            Vec::new(),
            &mut idle_rect,
        );
        let mut busy_rect = Rect::NOTHING;
        run(
            &ctx,
            &busy_tree,
            &mut Vec::new(),
            Vec::new(),
            &mut busy_rect,
        );
        assert_eq!(
            idle_rect, busy_rect,
            "忙碌态换图形不换占位:否则按钮在刷新时会跳一下"
        );

        // 忙碌态仍可点:点它仍发刷新消息(而不是被静默吞掉)
        let center = busy_rect.center();
        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let mut outbox = Vec::new();
        let mut clicked = Rect::NOTHING;
        run(
            &ctx,
            &busy_tree,
            &mut outbox,
            vec![Event::PointerMoved(center), click(true), click(false)],
            &mut clicked,
        );
        assert_eq!(clicked, busy_rect, "点击帧布局不得漂移");
        assert_eq!(
            outbox,
            vec![Message::FileTreeRefreshRequested],
            "刷新途中再点仍是刷新,不该被禁用吞掉"
        );
    }

    /// rail 五枚视图 tab 在同一条纵带上排得下。
    ///
    /// **2026-10-12 下组六枚文件动作裁撤**后这条从「11 枚的前提断言」退成
    /// 下限守门:再加常驻按钮就得重新过一遍高度预算(挤不下就得给 rail
    /// 自己做滚动或分两列,收益随之打折)。
    ///
    /// 上界取侧栏常见高度而非某个 rail 尺寸 token:「一行占用多少」与
    /// 「剩下多少给内容」是两件独立的事(同 §2.1 的纪律)。
    #[test]
    fn every_rail_item_fits_the_sidebar_height() {
        let pitch = crate::ui::tokens::RAIL_ITEM + crate::ui::tokens::RAIL_GAP;
        let count = SidebarTab::ALL.len();
        let total = pitch * count as f32;
        // 600px 栏高留出 1 行以上余量;1500 是实测侧栏可达高度的上放大镜。
        const COLUMN_H: f32 = 600.0;
        assert!(
            total <= COLUMN_H,
            "rail {count} 枚合计 {total}px 须 ≤ 栏高 {COLUMN_H}px(pitch={pitch})"
        );
        assert_eq!(
            count, 5,
            "rail 只挂五枚视图 tab;再加常驻按钮先过上方高度预算"
        );
    }
}
