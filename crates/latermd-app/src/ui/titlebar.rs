//! 自绘标题栏与无边框窗口骨架(docs/ui-shell-redesign.md §3,决策 D1)。
//!
//! 无边框(`ViewportBuilder::with_decorations(false)`)后窗口 chrome 全部由
//! 本模块接管:36px 标题栏(拖拽 / 双击最大化 / 六按钮)+ 屏幕四边四角的
//! 透明缩放命令区。`LATERMD_NATIVE_DECORATIONS=1` 时两块都不绘制
//! (逃生口,行为 = 原生装饰的现状),开关在 `main` 启动时读一次。
//!
//! 命中矩形划分、缩放方向、最大化图标这些**纯几何/纯逻辑**抽成函数供
//! 单测钉住;`ui` 的交互区用 `Ui::allocate_rect`(绝对摆放)直接消费
//! 同一批纯函数,测试与绘制零偏差。

use crate::command::Command;
use crate::search::SearchState;
use crate::settings::SettingsTab;
use crate::state::{Message, State};
use crate::ui::icons::Icon;
use crate::ui::tokens::{DANGER, ICON, RADIUS_MD, RADIUS_SM, SPACE_SM};
use eframe::egui::{self, Color32, CursorIcon, PointerButton, Pos2, Rect, ResizeDirection};
use eframe::egui::{Sense, ViewportCommand};

/// 四边缩放命中条厚度。
pub const EDGE_T: f32 = 6.0;
/// 四角缩放命中块边长(盖住边条交叠,角上命中对角方向)。
pub const CORNER_T: f32 = 12.0;

/// 标题栏右端七个窗口按钮,从左到右(docs/ui-shell-redesign.md §3.1 顺序;
/// 齿轮为 2026-09-27 用户指令新增,见 decisions-pending #31)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleButton {
    /// 关闭/打开左栏(翻转侧边栏可见性)。
    PanelLeft,
    Zen,
    /// 设置:左键开默认页,右键四页直达(原 M2 左栏底段设置行迁来)。
    Settings,
    /// 最小化。
    Minimize,
    /// 最大化 / 还原(按当前状态切图标)。
    Maximize,
    /// 关闭窗口。
    Close,
}

/// 标题按钮的绘制顺序(= 命中矩形从左到右的顺序)。
pub const TITLE_BUTTONS: [TitleButton; 6] = [
    TitleButton::PanelLeft,
    TitleButton::Zen,
    TitleButton::Settings,
    TitleButton::Minimize,
    TitleButton::Maximize,
    TitleButton::Close,
];

/// 按钮图标;最大化态的 [`TitleButton::Maximize`] 画还原图标
/// (条件与 `ctx.input(|i| i.viewport().maximized)` 的取值一一对应)。
pub fn icon_of(button: TitleButton, maximized: bool) -> Icon {
    match button {
        TitleButton::PanelLeft => Icon::Sidebar,
        TitleButton::Zen => Icon::Zen,
        TitleButton::Settings => Icon::Settings,
        TitleButton::Minimize => Icon::Minimize,
        TitleButton::Maximize if maximized => Icon::Restore,
        TitleButton::Maximize => Icon::Maximize,
        TitleButton::Close => Icon::Close,
    }
}

/// 七个按钮的命中矩形:`WINDOW_BTN` 整块从标题栏右缘连续向左排,垂直
/// 居中,无间隙(VS Code / Chrome 同款)。绘制与测试共用本函数。
pub fn button_rects(bar: Rect) -> [Rect; 6] {
    let top = bar.center().y - crate::ui::tokens::WINDOW_BTN.y / 2.0;
    let mut rects = [Rect::NOTHING; 6];
    for (i, rect) in rects.iter_mut().enumerate() {
        let left = bar.right() - (TITLE_BUTTONS.len() - i) as f32 * crate::ui::tokens::WINDOW_BTN.x;
        *rect = Rect::from_min_size(Pos2::new(left, top), crate::ui::tokens::WINDOW_BTN);
    }
    rects
}

/// 屏幕四周的八个缩放命中区:四边 6px 条 + 四角 12px 块(角块盖住边条,
/// 同一命中点只归属一个方向)。`from_two_pos` 自带归一,窗口极小不翻转。
pub fn edge_zones(screen: Rect) -> [(ResizeDirection, Rect); 8] {
    let (l, t, r, b) = (screen.left(), screen.top(), screen.right(), screen.bottom());
    [
        (
            ResizeDirection::North,
            Rect::from_two_pos(
                Pos2::new(l + CORNER_T, t),
                Pos2::new(r - CORNER_T, t + EDGE_T),
            ),
        ),
        (
            ResizeDirection::South,
            Rect::from_two_pos(
                Pos2::new(l + CORNER_T, b - EDGE_T),
                Pos2::new(r - CORNER_T, b),
            ),
        ),
        (
            ResizeDirection::West,
            Rect::from_two_pos(
                Pos2::new(l, t + CORNER_T),
                Pos2::new(l + EDGE_T, b - CORNER_T),
            ),
        ),
        (
            ResizeDirection::East,
            Rect::from_two_pos(
                Pos2::new(r - EDGE_T, t + CORNER_T),
                Pos2::new(r, b - CORNER_T),
            ),
        ),
        (
            ResizeDirection::NorthWest,
            Rect::from_two_pos(Pos2::new(l, t), Pos2::new(l + CORNER_T, t + CORNER_T)),
        ),
        (
            ResizeDirection::NorthEast,
            Rect::from_two_pos(Pos2::new(r - CORNER_T, t), Pos2::new(r, t + CORNER_T)),
        ),
        (
            ResizeDirection::SouthWest,
            Rect::from_two_pos(Pos2::new(l, b - CORNER_T), Pos2::new(l + CORNER_T, b)),
        ),
        (
            ResizeDirection::SouthEast,
            Rect::from_two_pos(Pos2::new(r - CORNER_T, b - CORNER_T), Pos2::new(r, b)),
        ),
    ]
}

/// 缩放命中区对应的系统光标。
fn cursor_of(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    }
}

/// 屏幕四周的透明缩放命令区;命中即发 `BeginResize(方向)`。
///
/// **必须画在 `ui::layout::draw` 的最后,且不建独立 Area。** egui 0.36
/// 里各 Panel 与根 `Ui` 共用同一绘制层,而命中测试会把「更高层里靠近
/// 指针的 widget」所在层之外整体排除(`hit_test.rs` 的 included_layers
/// 启发式):zones 一旦放进 Foreground 层的 Area(无论共用一个还是八区
/// 各一个),panel 内容在离边缘一个命中半径之外的按钮点击也会被整体吞掉
/// (实测,见 `edge_zones_and_titlebar_coexist` 这条回归)。同一层内则按
/// 距离裁决、同距时后画者胜 —— zones 在最后分配,边缘 6/12px 内缩放
/// 手势稳定获胜,按钮等内容正常交互。`Sense::drag()`(纯拖拽感知):egui
/// 对只感知拖拽的 widget 按下当帧即判拖拽(interaction.rs),`BeginResize`
/// 因此是「按下即缩放」的原生手感,不必等位移越过点击容差。
pub fn edge_resize_zones(ui: &mut egui::Ui) {
    let screen = ui.ctx().input(|i| i.viewport_rect());
    for (direction, rect) in edge_zones(screen) {
        let response = ui
            .allocate_rect(rect, Sense::drag())
            .on_hover_cursor(cursor_of(direction));
        if response.drag_started_by(PointerButton::Primary) {
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::BeginResize(direction));
        }
    }
}

/// 命令箱右缘的 x:紧贴最左那枚窗口按钮再往左留一个 gap。
///
/// 与 [`button_rects`] 共用同一基准(两者必须同步;PTC_%s 断言在
/// `tests::command_box_does_not_overlap_window_buttons`)。
pub fn command_box_right_edge(bar: Rect) -> f32 {
    button_rects(bar)[0].left() - crate::ui::tokens::TITLE_CMD_TO_BTN
}

/// 是否绘制命令箱。
///
/// 窄窗口下命令箱会挤掉左段标题 —— 与其画一个 346px 的控件再去挤标题,
/// 不如**整体不画**:搜索能力经左栏 Search 页签 / Ctrl+P 仍在,不丢。
pub fn shows_command_box(bar: Rect) -> bool {
    bar.width() >= crate::ui::tokens::TITLE_CMD_MAX_W
}

/// 命令箱整体矩形(纵向铺满标题栏,高度回调方裁到 TITLE_CMD_H)。
pub fn command_box_rect(bar: Rect) -> Rect {
    let right = command_box_right_edge(bar);
    let width = if bar.width() < 1100.0 {
        crate::ui::tokens::TITLE_VIEW_W + crate::ui::tokens::TITLE_CMD_GAP + 28.0
    } else {
        crate::ui::tokens::TITLE_CMD_W
    };
    Rect::from_min_max(
        Pos2::new(right - width, bar.top()),
        Pos2::new(right, bar.bottom()),
    )
}

/// 命令箱内第 `index` 个槽位的矩形(垂直居中,高 `TITLE_CMD_H`)。
///
/// **自右往左排**:0 = 搜索胶囊(贴右缘),1 = 源码/Live 切换。理由是 ego
/// 的焦点链按注册顺序推进 —— 从右往左读是中文 UI 的常态,Tab 也应如此。
pub fn command_box_slot_rect(cbox: Rect, index: usize) -> Rect {
    const SLOTS: usize = 2;
    debug_assert!(index < SLOTS, "命令箱只有 {SLOTS} 个槽位");
    let search_w =
        cbox.width() - crate::ui::tokens::TITLE_VIEW_W - crate::ui::tokens::TITLE_CMD_GAP;
    let view_w = crate::ui::tokens::TITLE_VIEW_W;
    let gap = crate::ui::tokens::TITLE_CMD_GAP;
    let inset_y = (cbox.height() - crate::ui::tokens::TITLE_CMD_H) / 2.0;
    let right = cbox.right() - index as f32 * (search_w + gap);
    let width = if index == 0 { search_w } else { view_w };
    Rect::from_min_max(
        Pos2::new(right - width, cbox.top() + inset_y),
        Pos2::new(right, cbox.bottom() - inset_y),
    )
}

/// 左段品牌标识的矩形:距左缘与距汉堡同为 [`SPACE_SM`](贴角会读成
/// 「钉在角落」,2026-10-12 坤哥反馈),垂直居中于标题栏。
pub fn brand_logo_rect(bar: Rect) -> Rect {
    let size = crate::ui::tokens::BRAND_LOGO;
    Rect::from_min_size(
        Pos2::new(bar.left() + SPACE_SM, bar.center().y - size / 2.0),
        egui::vec2(size, size),
    )
}

/// 标题栏菜单按钮的矩形:位于 logo 右侧,与品牌区保持统一间距。
pub fn menu_button_rect(bar: Rect) -> Rect {
    let size = crate::ui::tokens::WINDOW_BTN;
    let logo = brand_logo_rect(bar);
    Rect::from_min_size(
        Pos2::new(logo.right() + SPACE_SM, bar.center().y - size.y / 2.0),
        size,
    )
}

/// logo 位于菜单按钮左侧,标题跟随菜单按钮右侧,品牌区只绘制一枚图片。
fn brand_logo_and_title(bar: Rect) -> (Rect, Pos2) {
    let logo = brand_logo_rect(bar);
    let text_pos = Pos2::new(menu_button_rect(bar).right() + SPACE_SM, bar.center().y);
    (logo, text_pos)
}

/// 在给定矩形绘制品牌图片；素材不可用时回落到矢量图标。
pub(crate) fn paint_brand_logo(ui: &mut egui::Ui, rect: Rect) {
    ui.allocate_rect(rect, Sense::hover());
    let painter = ui.painter();
    match brand_logo_texture(ui.ctx()) {
        Some(texture) => egui::Image::new(&texture)
            .fit_to_exact_size(rect.size())
            .paint_at(ui, rect),
        None => Icon::Files.draw(
            painter,
            rect.center(),
            rect.height(),
            crate::ui::tokens::accent(ui),
        ),
    }
}

/// 取品牌标识纹理(带缓存)。
///
/// 缓存挂在 `ctx.data`(egui `TempStorage`,随 `Context` 生命周期),**只在
/// 首帧 load 一次**;之后每帧只拿句柄。解码失败时**不缓存** —— 返回
/// `None` 让调用方回落矢量图标,素材修好后下一帧自愈,不必重启。
///
/// `TempStorage` 按类型取值:存 `Option<TextureHandle>` 就必须按
/// `Option<TextureHandle>` 读,类型不一致会**静默返回 `None`**(踩过一次:
/// 存 `Option<usize>` 按 `usize` 读,每帧都判定「没缓存」)。这里存读
/// 两侧逐字一致。
pub(crate) fn brand_logo_texture(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let key = egui::Id::new("latermd.brand_logo.texture");
    if let Some(cached) = ctx.data(|d| d.get_temp::<Option<egui::TextureHandle>>(key)) {
        return cached;
    }
    let handle = crate::assets::brand_logo_image()
        .map(|image| ctx.load_texture("latermd-brand-logo", image, egui::TextureOptions::LINEAR));
    // 缓存 Some 分支即可:None 时每帧重试一次解码(失败路径只打一行
    // eprintln,素材缺失属开发期常态,不值得为它加一层「已知失败」状态)。
    if handle.is_some() {
        ctx.data_mut(|d| d.insert_temp(key, handle.clone()));
    }
    handle
}

/// 标题栏内容(挂在 `Panel::top("titlebar")` 内,定高 `TITLEBAR_H`,
/// panel frame 内边距须为 0,命中矩形才与右缘对齐)。
pub fn ui(ui: &mut egui::Ui, state: &mut State, outbox: &mut Vec<Message>) {
    let bar = ui.max_rect();
    let ctx = ui.ctx().clone();
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));

    // 背景:整条标题栏可拖拽 / 双击最大化。先铺背景,按钮后画 —— 同层
    // 命中测试后画者优先,按钮矩形天然盖过背景。click_and_drag 双感知时
    // egui 把「按下后位移越过点击容差(6px)」才判为拖拽,StartDrag 因此
    // 在真正开拖的那帧发出;静按压仍是 click,双击最大化由此可用。
    let background = ui.allocate_rect(bar, Sense::click_and_drag());
    if background.drag_started_by(PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if background.double_clicked_by(PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    }

    // 左缘固定为「logo + 汉堡」：展开菜单时也保留品牌标识，避免标题栏
    // 左侧突然跳位。所有元素共用标题栏中心线。
    let (logo_rect, text_pos) = brand_logo_and_title(bar);
    let menu_rect = menu_button_rect(bar);
    paint_brand_logo(ui, logo_rect);
    crate::ui::menubar::hamburger_button_at(ui, menu_rect);

    // 菜单标签行(2026-10-12,坤哥指定形态):汉堡点开后**长在标题栏
    // 条内**(左中段),不弹浮层；展开期间标题/页签让位。
    let row_open = crate::ui::menubar::menu_row_open(&ctx);
    // 展开时保留左侧 logo 与汉堡；标签行从汉堡右侧开始，避免覆盖品牌区。
    if row_open {
        crate::ui::menubar::menu_row(
            ui,
            bar,
            &state.keymap,
            state,
            outbox,
            None::<fn(usize, egui::Id, Rect)>,
        );
    }

    // 左段:「LaterMD — 文档名*」。文案取 `window_title()`,与原生窗口标题
    // 同一数据源。展开菜单时只让出标题/页签区域。
    if !row_open && ui.is_rect_visible(bar) {
        let mut font = egui::TextStyle::Button.resolve(ui.style());
        font.family = crate::fonts::semibold_family(&ctx);
        let right = if shows_command_box(bar) {
            command_box_rect(bar).left()
        } else {
            button_rects(bar)[0].left()
        };
        let title_rect = Rect::from_min_max(
            egui::pos2(text_pos.x, bar.center().y - 14.0),
            egui::pos2(
                right - crate::ui::workbench::HEADER_DRAG_W,
                bar.center().y + 14.0,
            ),
        );
        if !state.layout.zen && crate::ui::tabs::visible(&state.tabs) {
            crate::ui::tabs::header(
                ui,
                title_rect,
                &state.tabs,
                state.theme.tab_title_width,
                state.file_tree.root.as_deref(),
                outbox,
            );
        } else {
            let title = crate::ui::tabs::elide_text(
                ui,
                &state.tabs.current().document.window_title(),
                &font,
                title_rect.width().max(0.0),
            );
            ui.painter().with_clip_rect(title_rect).text(
                text_pos,
                egui::Align2::LEFT_CENTER,
                title,
                font,
                ui.visuals().text_color(),
            );
        }
    }

    let rects = button_rects(bar);
    if !row_open && shows_command_box(bar) {
        command_box(ui, &ctx, state, command_box_rect(bar), outbox);
    }
    let toggle_shortcut = shortcut_of(state, &ctx, Command::ToggleSidebar);
    let zen_shortcut = shortcut_of(state, &ctx, Command::ToggleZen);
    for (button, rect) in TITLE_BUTTONS.into_iter().zip(rects) {
        window_button(
            ui,
            &ctx,
            button,
            rect,
            maximized,
            state.layout.left,
            state.layout.zen,
            toggle_shortcut.as_deref(),
            zen_shortcut.as_deref(),
            outbox,
        );
    }
}

/// 命令箱内的**搜索胶囊**(槽位 0,贴右缘)。
///
/// 它是**真实可输入的 `TextEdit`**,不是「点一下弹到别处」的假控件:
/// `search.query` 由这里直接改写并通过 [`Message::SearchQueryChanged`]
/// 触发 300ms 去抖搜索。
///
/// 视觉与交互对齐 mac 工作台 `workbench::search_field`(2026-10-10 精修
/// 平移,#166):放大镜 + 聚焦强调描边 + 有词时清空入口;提示语从
/// 「搜索 / 跳转…」改为实际用途「搜索文档…」(胶囊只做全文搜索,跳转
/// 另有 Ctrl+G/查找浮层,旧提示语名不副实)。
pub(super) fn search_capsule(
    ui: &mut egui::Ui,
    rect: Rect,
    search: &mut SearchState,
    outbox: &mut Vec<Message>,
) {
    let colors = crate::theme::shell(ui);
    let id = ui.make_persistent_id("titlebar-search");
    let focused = ui.memory(|m| m.has_focus(id));
    let radius = RADIUS_MD * 2.0; // 完全圆角胶囊
    let painter = ui.painter();
    painter.rect_filled(rect, radius, colors.content);
    painter.rect_stroke(
        rect,
        radius,
        if focused {
            egui::Stroke::new(1.0, colors.accent.gamma_multiply(0.65))
        } else {
            egui::Stroke::new(1.0, colors.border.gamma_multiply(0.45))
        },
        egui::StrokeKind::Inside,
    );
    crate::ui::icons::Icon::Search.draw(
        painter,
        egui::pos2(rect.left() + 12.0, rect.center().y),
        13.0,
        colors.secondary,
    );
    let has_query = !search.query.is_empty();
    let edit_rect = Rect::from_min_max(
        egui::pos2(rect.left() + 24.0, rect.top() + 2.0),
        egui::pos2(
            rect.right() - if has_query { 22.0 } else { 6.0 },
            rect.bottom() - 2.0,
        ),
    );
    let edit = egui::TextEdit::singleline(&mut search.query)
        .id(id)
        .font(egui::FontId::proportional(12.0))
        .hint_text(crate::ui::tokens::TITLE_SEARCH_HINT)
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::ZERO)
        .vertical_align(egui::Align::Center);
    // `Ui::put` 把 widget 摆到绝对矩形:命令箱先顺序算矩形再塞 widget,
    // put 是把 TextEdit 放进指定 rect 的唯一办法。
    let response = ui.put(edit_rect, edit);
    if response.changed() {
        outbox.push(Message::SearchQueryChanged);
    }
    // 聚焦即展开左栏 Search 页:点进胶囊还没输入的场合也「在搜索」,
    // 结果面板应当就位(输入变化由 `SearchQueryChanged` 的归约兜底)。
    // 禅定模式在归约内豁免,这里无条件发。
    if response.gained_focus() {
        outbox.push(Message::SearchPanelRevealed);
    }
    response.on_hover_text("全文搜索(结果在左栏「搜索」页)");
    if has_query {
        let clear_rect = Rect::from_center_size(
            egui::pos2(rect.right() - 11.0, rect.center().y),
            egui::vec2(18.0, 18.0),
        );
        let clear = ui.interact(clear_rect, id.with("clear"), egui::Sense::click());
        let painter = ui.painter();
        if ui.is_rect_visible(clear_rect) {
            if clear.hovered() {
                painter.rect_filled(clear_rect, crate::ui::tokens::RADIUS_SM, colors.hover);
            }
            crate::ui::icons::Icon::Close.draw(
                painter,
                clear_rect.center(),
                11.0,
                colors.secondary,
            );
        }
        clear.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "清空搜索"));
        if clear.clicked() {
            search.query.clear();
            outbox.push(Message::SearchQueryChanged);
        }
        clear.on_hover_text("清空搜索");
    }
}

/// 标题栏右端命令箱(docs/ui-shell-redesign-v2.md §5.6):搜索胶囊 +
/// 源码/Live 两段切换。调用方须先用 [`shows_command_box`] 判过宽度。
fn command_box(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    state: &mut State,
    cbox: Rect,
    outbox: &mut Vec<Message>,
) {
    let _ = ctx;
    let slot0 = command_box_slot_rect(cbox, 0);
    if slot0.width() < 100.0 {
        crate::ui::workbench::compact_search(ui, slot0, state, outbox);
    } else {
        search_capsule(ui, slot0, &mut state.search, outbox);
    }
    let slot1 = command_box_slot_rect(cbox, 1);
    view_switch(ui, slot1, state, outbox);
}

/// 源码 / Live 两段切换(槽位 1)。
///
/// 分段开关(VS Code / Zed 同款):**选中段有一层实心底**,非选中段
/// 只有文字;点击任一段 → [`Message::ToggleLivePreview`]
/// (与菜单栏「视图 → 写作模式」同一命令,不另开入口)。
pub(super) fn view_switch(ui: &mut egui::Ui, rect: Rect, state: &State, outbox: &mut Vec<Message>) {
    crate::ui::workbench::mode_switch(ui, rect, state, outbox);
}

/// 某条命令当前绑的键位(用户可改,与 settings 快捷键页同源)。
fn shortcut_of(state: &State, ctx: &egui::Context, cmd: Command) -> Option<String> {
    state
        .keymap
        .get(cmd)
        .map(|shortcut| ctx.format_shortcut(&shortcut.keyboard()))
}

/// 单个窗口按钮:命中区整块 `WINDOW_BTN`,hover 浅底,关闭键 hover 用
/// 警示色。动作只发视口命令与 [`Message`],不在 UI 侧改状态。
///
/// `zen_open` 只影响禅定键的**著色**:它是个纯 toggle,没有禁用态, 仅靠
/// 图标著色区分「当前是否在禅定里」(accent = 在)。
#[allow(clippy::too_many_arguments)]
fn window_button(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    button: TitleButton,
    rect: Rect,
    maximized: bool,
    sidebar_open: bool,
    zen_open: bool,
    toggle_shortcut: Option<&str>,
    zen_shortcut: Option<&str>,
    outbox: &mut Vec<Message>,
) {
    let response = ui.allocate_rect(rect, Sense::click());
    let hovered = response.hovered();

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if hovered {
            let fill = if button == TitleButton::Close {
                DANGER
            } else {
                ui.visuals().widgets.hovered.bg_fill
            };
            painter.rect_filled(rect, RADIUS_SM, fill);
        }
        let color = if button == TitleButton::Close && hovered {
            Color32::WHITE
        } else if button == TitleButton::Zen && zen_open {
            // 禅定键是唯一带「当前模式」语义的窗口按钮:进入后用强调色,让
            // 「我在哪儿 / 怎么回去」在这一颗图标上自解释。
            crate::ui::tokens::accent(ui)
        } else {
            ui.visuals().text_color()
        };
        icon_of(button, maximized).draw(painter, rect.center(), ICON, color);
    }

    // 悬浮提示在前(消费 response 返回同型值),动作判定在后。
    let response = match button {
        TitleButton::PanelLeft => {
            let action = if sidebar_open {
                "关闭左侧"
            } else {
                "打开左侧"
            };
            response.on_hover_text(match toggle_shortcut {
                Some(shortcut) => format!("{action}({shortcut})"),
                None => action.to_owned(),
            })
        }
        // 与 `PanelLeft` 同款:键位文案取自 keymap(用户可改),不硬编码 F11。
        TitleButton::Zen => {
            let action = if zen_open {
                "退出禅定"
            } else {
                "禅定模式"
            };
            response.on_hover_text(match zen_shortcut {
                Some(shortcut) => format!("{action}({shortcut})"),
                None => action.to_owned(),
            })
        }
        TitleButton::Settings => {
            // 右键四页直达随 settings 行一并迁来(不丢能力,decisions-pending
            // #31)。`context_menu` 消费 Response、返回弹层打开状态,借 clone
            // 注册菜单,原 response 继续供悬浮/点击判定用。条目用 menu_item
            // (自绘,墨迹重心补偿):CJK 标签在 egui Button 的 hover 灰影里
            // 偏上 ~2.5px(menu_item 模块注释有取证)。
            response.clone().context_menu(|ui| {
                for tab in SettingsTab::ALL {
                    if crate::ui::menu_item::button(ui, tab.label()).clicked() {
                        outbox.push(Message::SettingsOpened(tab));
                    }
                }
            });
            response.on_hover_text("设置(右键直达各页)")
        }
        TitleButton::Minimize => response.on_hover_text("最小化"),
        TitleButton::Maximize => {
            response.on_hover_text(if maximized { "还原" } else { "最大化" })
        }
        TitleButton::Close => response.on_hover_text("关闭"),
    };
    match button {
        TitleButton::PanelLeft if response.clicked() => outbox.push(Message::SidebarToggled),
        TitleButton::Zen if response.clicked() => outbox.push(Message::ZenToggled),
        TitleButton::Settings if response.clicked() => {
            outbox.push(Message::SettingsOpened(SettingsTab::Appearance));
        }
        TitleButton::Minimize if response.clicked() => {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        }
        TitleButton::Maximize if response.clicked() => {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }
        TitleButton::Close if response.clicked() => {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;
    use crate::ui::tokens::TITLEBAR_H;
    use egui::{Event, RawInput, ViewportCommand};

    const SCREEN: Rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(900.0, 600.0));

    fn click(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// 跑一帧标题栏,返回视口命令(标题命令不在本模块,应为空)。
    fn frame(
        ctx: &egui::Context,
        state: &mut State,
        outbox: &mut Vec<Message>,
        events: Vec<Event>,
    ) -> Vec<ViewportCommand> {
        let output = ctx.run_ui(
            RawInput {
                events,
                screen_rect: Some(SCREEN),
                ..Default::default()
            },
            |ui| super::ui(ui, state, outbox),
        );
        let commands = output
            .viewport_output
            .values()
            .flat_map(|viewport| viewport.commands.iter())
            .cloned()
            .collect::<Vec<_>>();
        output.drop_without_applying_deltas();
        commands
    }

    /// 跑一帧边缘命令区,返回视口命令。
    fn zone_frame(ctx: &egui::Context, events: Vec<Event>) -> Vec<ViewportCommand> {
        let output = ctx.run_ui(
            RawInput {
                events,
                screen_rect: Some(SCREEN),
                ..Default::default()
            },
            edge_resize_zones,
        );
        let commands = output
            .viewport_output
            .values()
            .flat_map(|viewport| viewport.commands.iter())
            .cloned()
            .collect::<Vec<_>>();
        output.drop_without_applying_deltas();
        commands
    }

    /// 复刻 `draw` 的骨架顺序跑一帧:标题栏 panel 在前、边缘命令区最后
    /// (与生产同序),返回该帧是否产出 `SidebarToggled`。
    fn shell_frame(ctx: &egui::Context, events: Vec<Event>) -> (bool, Vec<ViewportCommand>) {
        let mut state = State::default();
        let mut outbox = Vec::new();
        let output = ctx.run_ui(
            RawInput {
                events,
                screen_rect: Some(SCREEN),
                ..Default::default()
            },
            |ui| {
                egui::Panel::top("titlebar")
                    .exact_size(crate::ui::tokens::TITLEBAR_H)
                    .frame(
                        egui::Frame::default()
                            .inner_margin(egui::Margin::ZERO)
                            .fill(ui.visuals().panel_fill),
                    )
                    .show(ui, |ui| super::ui(ui, &mut state, &mut outbox));
                edge_resize_zones(ui);
            },
        );
        let commands = output
            .viewport_output
            .values()
            .flat_map(|viewport| viewport.commands.iter())
            .cloned()
            .collect::<Vec<_>>();
        output.drop_without_applying_deltas();
        (outbox.contains(&Message::SidebarToggled), commands)
    }

    /// logo 位于菜单按钮左侧,标题跟随菜单按钮右侧,品牌区只绘制一枚图片。
    #[test]
    fn brand_logo_sits_before_menu_and_pushes_title_right() {
        let bar = Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, TITLEBAR_H));
        let (logo, text_pos) = super::brand_logo_and_title(bar);

        assert_eq!(logo.left(), bar.left() + SPACE_SM);
        assert_eq!(super::menu_button_rect(bar).left(), logo.right() + SPACE_SM);
        assert!(bar.contains_rect(logo));
        assert_eq!(text_pos.x, super::menu_button_rect(bar).right() + SPACE_SM);
        assert!(text_pos.x >= super::menu_button_rect(bar).right());
        assert_eq!(logo.center().y, bar.center().y);
    }

    /// 菜单行标签从汉堡右侧开始,避免覆盖 logo 和汉堡。
    #[test]
    fn menu_row_starts_after_brand_controls() {
        let bar = Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, TITLEBAR_H));
        let logo = super::brand_logo_rect(bar);
        let menu = super::menu_button_rect(bar);
        assert_eq!(menu.left(), logo.right() + SPACE_SM);
        assert_eq!(menu.center().y, bar.center().y);
    }

    ///
    /// 这条钉的是「左上角画的是真 logo,不是那个通用文件夹矢量图标」。
    /// 回落路径(`brand_logo_image()` 返回 `None` →画`Icon::Files`)在这里
    /// 表现为**没有纹理** → 当场红,而不是让「换了张图结果左上角还是旧
    /// 图标」这种事只靠肉眼发现。
    #[test]
    fn brand_logo_texture_is_loaded_into_the_context() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        let mut outbox = Vec::new();
        frame(&ctx, &mut state, &mut outbox, Vec::new());
        let key = egui::Id::new("latermd.brand_logo.texture");
        assert!(
            ctx.data(|d| d.get_temp::<Option<egui::TextureHandle>>(key))
                .flatten()
                .is_some(),
            "标题栏首帧后应缓存品牌标识纹理(否则左上角画的是回落矢量图标)"
        );
    }

    /// 七按钮从右缘等宽连续排布:无重叠、右缘贴齐、垂直居中,顺序与
    /// `TITLE_BUTTONS` 一致(左 → 右)。
    #[test]
    fn button_rects_tile_from_the_right_edge() {
        let bar = Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 36.0));
        let rects = button_rects(bar);
        let btn = crate::ui::tokens::WINDOW_BTN;

        for (i, rect) in rects.iter().enumerate() {
            assert_eq!((rect.width(), rect.height()), (btn.x, btn.y));
            assert_eq!(
                rect.right(),
                bar.right() - (5 - i) as f32 * btn.x,
                "第 {i} 个按钮右缘(6 键,P4 起无右栏键)"
            );
            assert_eq!(rect.center().y, bar.center().y, "垂直居中");
            if i > 0 {
                assert!(
                    rect.left() >= rects[i - 1].right(),
                    "按钮 {i} 与前一个不重叠(贴边允许)"
                );
            }
        }
        assert_eq!(rects[5].right(), bar.right(), "关闭键贴右缘");
        assert_eq!(rects[0].left(), bar.right() - 6.0 * btn.x, "左起第一个");
        // 齿轮在最小化左侧(2026-09-27 用户指令:设置入口挪标题栏右端)
        let settings = TITLE_BUTTONS
            .iter()
            .position(|b| *b == TitleButton::Settings)
            .unwrap();
        let minimize = TITLE_BUTTONS
            .iter()
            .position(|b| *b == TitleButton::Minimize)
            .unwrap();
        assert!(settings < minimize, "齿轮在最小化左侧");
    }

    /// 最大化按钮的图标随窗口状态切换,其余按钮不受影响。
    #[test]
    fn maximize_icon_switches_with_window_state() {
        assert_eq!(icon_of(TitleButton::Maximize, false), Icon::Maximize);
        assert_eq!(icon_of(TitleButton::Maximize, true), Icon::Restore);
        for button in TITLE_BUTTONS
            .iter()
            .filter(|b| **b != TitleButton::Maximize)
        {
            assert_eq!(icon_of(*button, false), icon_of(*button, true));
        }
    }

    /// 八个命中区:边条 6px 厚且让开角块、角块 12px 见方且贴角、方向与
    /// 所在边一致、八区两两不重叠。
    #[test]
    fn edge_zones_geometry_and_directions() {
        let zones = edge_zones(SCREEN);
        assert_eq!(zones.len(), 8);

        let zone = |direction| zones.iter().find(|(d, _)| *d == direction).unwrap().1;
        // 四角块:12×12 贴角
        assert_eq!(
            zone(ResizeDirection::NorthWest),
            Rect::from_min_size(Pos2::ZERO, egui::vec2(CORNER_T, CORNER_T))
        );
        assert_eq!(
            zone(ResizeDirection::SouthEast),
            Rect::from_min_size(
                Pos2::new(SCREEN.right() - CORNER_T, SCREEN.bottom() - CORNER_T),
                egui::vec2(CORNER_T, CORNER_T)
            )
        );
        // 北边条:6px 厚,水平让开两个角块
        let north = zone(ResizeDirection::North);
        assert_eq!(north.height(), EDGE_T);
        assert_eq!(north.left(), CORNER_T);
        assert_eq!(north.right(), SCREEN.right() - CORNER_T);
        // 东边条:6px 厚,垂直让开两个角块
        let east = zone(ResizeDirection::East);
        assert_eq!(east.width(), EDGE_T);
        assert_eq!(east.top(), CORNER_T);
        assert_eq!(east.bottom(), SCREEN.bottom() - CORNER_T);
        // 八区两两不重叠(贴边允许):同一命中点只归属一个方向
        let overlaps = |a: Rect, b: Rect| {
            a.min.x < b.max.x && b.min.x < a.max.x && a.min.y < b.max.y && b.min.y < a.max.y
        };
        for (i, (_, a)) in zones.iter().enumerate() {
            for (_, b) in zones.iter().skip(i + 1) {
                assert!(!overlaps(*a, *b), "{a:?} 与 {b:?} 内部不应重叠");
            }
        }
    }

    /// 无头跑完整标题栏:按下标题区发 `StartDrag`,双击发 `Maximized`,
    /// 三个窗口按钮发 `Minimized` / `Maximized` / `Close`;左/右栏钮发
    /// 消息(归约在 `State::apply`),禅定占位不触发。
    #[test]
    fn titlebar_interactions_send_commands_and_messages() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        let mut outbox = Vec::new();

        // 纯渲染帧:除首帧 egui 自发的主题同步外,不发任何视口命令
        let first = frame(&ctx, &mut state, &mut outbox, Vec::new());
        assert!(
            first
                .iter()
                .all(|c| matches!(c, ViewportCommand::SetTheme(_))),
            "{first:?}"
        );

        // 拖拽:标题区(避开右端按钮排)按下并拖过点击容差(6px)后,
        // 开拖帧发 StartDrag;静按压仍是 click(双击最大化的前提)
        let grab = Pos2::new(SCREEN.center().x, 18.0);
        frame(
            &ctx,
            &mut state,
            &mut outbox,
            vec![Event::PointerMoved(grab)],
        );
        frame(&ctx, &mut state, &mut outbox, vec![click(grab, true)]);
        let commands = frame(
            &ctx,
            &mut state,
            &mut outbox,
            vec![Event::PointerMoved(grab + egui::vec2(12.0, 0.0))],
        );
        assert!(
            commands.contains(&ViewportCommand::StartDrag),
            "{commands:?}"
        );
        frame(
            &ctx,
            &mut state,
            &mut outbox,
            vec![click(grab + egui::vec2(12.0, 0.0), false)],
        );

        // 双击:快速两击,第二击的抬起帧发 Maximized(true)(无头视口默认
        // 非最大化;帧行间隔 1/60s,远小于双击判定窗口)
        let mut double_click_commands = Vec::new();
        for pressed in [true, false, true, false] {
            double_click_commands.extend(frame(
                &ctx,
                &mut state,
                &mut outbox,
                vec![click(grab, pressed)],
            ));
        }
        assert!(
            double_click_commands.contains(&ViewportCommand::Maximized(true)),
            "双击应发 Maximized(true),实际 {double_click_commands:?}"
        );

        // 六按钮逐个点击:命中矩形即纯函数给出的划分(绘制同源)
        let rects = button_rects(SCREEN);
        let press = |i: usize, state: &mut State, outbox: &mut Vec<Message>| {
            let center = rects[i].center();
            frame(&ctx, state, outbox, vec![Event::PointerMoved(center)]);
            frame(&ctx, state, outbox, vec![click(center, true)]);
            frame(&ctx, state, outbox, vec![click(center, false)])
        };

        press(0, &mut state, &mut outbox); // 关闭左栏 → 消息
        assert_eq!(outbox, vec![Message::SidebarToggled]);
        // 禅定键(M4 实装):与左栏那颗同为「布局入口」——产消息而非视口
        // 命令。区别在于进/出的快照怎么存怎么还原由 `LayoutSettings` 自己
        // 裁决,本模块连左栏当前是什么状态都不必知道。
        let commands = press(1, &mut state, &mut outbox);
        assert!(commands.is_empty(), "禅定键不发视口命令");
        // 齿轮(2026-09-27 迁自左栏底段设置行):左键开默认页,不发视口命令
        let commands = press(2, &mut state, &mut outbox);
        assert!(commands.is_empty(), "齿轮不发视口命令");
        assert_eq!(
            outbox,
            vec![
                Message::SidebarToggled,
                Message::ZenToggled,
                Message::SettingsOpened(crate::settings::SettingsTab::Appearance)
            ]
        );
        assert!(press(3, &mut state, &mut outbox).contains(&ViewportCommand::Minimized(true)));
        assert!(press(4, &mut state, &mut outbox).contains(&ViewportCommand::Maximized(true)));
        assert!(press(5, &mut state, &mut outbox).contains(&ViewportCommand::Close));
    }

    /// 八个边缘命令区:命中即发对应方向的 BeginResize。
    #[test]
    fn edge_zone_presses_send_begin_resize() {
        let ctx = egui::Context::default();
        // 热身一帧:Area 首遍为 sizing pass,widget 不参与命中测试
        zone_frame(&ctx, Vec::new());
        for (direction, rect) in edge_zones(SCREEN) {
            let center = rect.center();
            zone_frame(&ctx, vec![Event::PointerMoved(center)]);
            let commands = zone_frame(&ctx, vec![click(center, true)]);
            assert!(
                commands.contains(&ViewportCommand::BeginResize(direction)),
                "{direction:?} 命中区按下即发 BeginResize,实际 {commands:?}"
            );
            zone_frame(&ctx, vec![click(center, false)]);
        }
    }

    /// **跨层命中回归**(zones 曾放 Foreground 层 Area,把命中半径之外
    /// 的 panel 按钮点击整体吞掉):按生产顺序同帧渲染标题栏 panel + 边缘
    /// 命令区,按钮点击必须产消息、边缘按下必须发 BeginResize —— 两类
    /// 交互共存。
    #[test]
    fn edge_zones_and_titlebar_coexist() {
        let ctx = egui::Context::default();
        // 帧序与点击协议同 menubar 测试:先移动、再按下、后抬起
        let bar = Rect::from_min_max(Pos2::ZERO, Pos2::new(900.0, crate::ui::tokens::TITLEBAR_H));
        let button_center = button_rects(bar)[0].center();
        shell_frame(&ctx, Vec::new());
        shell_frame(&ctx, vec![Event::PointerMoved(button_center)]);
        shell_frame(&ctx, vec![click(button_center, true)]);
        let (clicked, _) = shell_frame(&ctx, vec![click(button_center, false)]);
        assert!(clicked, "边缘命令区不得吞掉标题栏按钮的点击");

        // 换一个 ctx 只点边缘:北边条中心(x 让开角块),按下当帧即发
        // BeginResize(North)
        let ctx = egui::Context::default();
        let north_center = egui::pos2(SCREEN.center().x, EDGE_T / 2.0);
        shell_frame(&ctx, Vec::new());
        shell_frame(&ctx, vec![Event::PointerMoved(north_center)]);
        let (_, commands) = shell_frame(&ctx, vec![click(north_center, true)]);
        assert!(
            commands.contains(&ViewportCommand::BeginResize(ResizeDirection::North)),
            "北边条按下应发 BeginResize(North),实际 {commands:?}"
        );
    }
    // —— 标题栏命令箱(docs/ui-shell-redesign-v2.md §5.6)——

    /// 宽 / 窄两条样本上的几何:命令箱不与七枚窗口按钮重叠、两槽位不
    /// 重叠、都落在标题栏高度内。
    ///
    /// **断言用的期待值取硬编码/分量组合,不读 `TITLE_CMD_W` 反推 ——
    /// 否则改 token 时绘制与断言一起变,断言自我满足。
    #[test]
    fn command_box_does_not_overlap_window_buttons() {
        // 900px 是 M0 的默认窗口宽,也是真机截图那一档
        let bar = Rect::from_min_size(Pos2::ZERO, egui::vec2(1240.0, TITLEBAR_H));
        let buttons = button_rects(bar);
        let cbox = command_box_rect(bar);
        let slot0 = command_box_slot_rect(cbox, 0);
        let slot1 = command_box_slot_rect(cbox, 1);

        // 命令箱整体在七枚按钮之左,且留了间隙
        assert!(shows_command_box(bar), "900px 应有命令箱");
        assert!(
            cbox.right() <= buttons[0].left(),
            "命令箱右缘 {:?} 不得越过最左按钮 {:?}",
            cbox.right(),
            buttons[0].left()
        );
        assert_eq!(
            buttons[0].left() - cbox.right(),
            crate::ui::tokens::TITLE_CMD_TO_BTN,
            "间隙固定为 TITLE_CMD_TO_BTN"
        );

        // 两槽位:0 贴右缘(搜索),1 在其左;互不重叠
        assert_eq!(slot0.right(), cbox.right(), "槽位 0 贴命令箱右缘");
        assert_eq!(slot0.width(), crate::ui::tokens::TITLE_SEARCH_W);
        assert_eq!(slot1.width(), crate::ui::tokens::TITLE_VIEW_W);
        assert!(
            slot1.right() <= slot0.left(),
            "槽位 1({slot1:?}) 须在槽位 0 左侧"
        );

        // 高度:两槽位等高 = TITLE_CMD_H,且垂直居中于这条 36px 标题栏
        assert_eq!(slot0.height(), crate::ui::tokens::TITLE_CMD_H);
        assert_eq!(slot1.height(), crate::ui::tokens::TITLE_CMD_H);
        let expected_top = (TITLEBAR_H - crate::ui::tokens::TITLE_CMD_H) / 2.0;
        assert!((slot0.top() - expected_top).abs() < 0.01, "{slot0:?}");
        assert!((slot1.top() - expected_top).abs() < 0.01, "{slot1:?}");
    }

    /// 命令箱总宽 = 搜索 + 间隙 + 切换;任何一个分量改了必须同步改这条。
    #[test]
    fn command_box_total_width_is_the_sum_of_its_parts() {
        assert_eq!(crate::ui::tokens::TITLE_CMD_W, 416.0, "实例值钉死");
        assert_eq!(
            crate::ui::tokens::TITLE_CMD_W,
            crate::ui::tokens::TITLE_SEARCH_W
                + crate::ui::tokens::TITLE_CMD_GAP
                + crate::ui::tokens::TITLE_VIEW_W
        );
    }

    /// 窄窗口整体不画命令箱 —— 与其挤掉左段标题,不如留着这份空间给标题
    /// (搜索能力经左栏 Search 页 / Ctrl+P 仍在,不丢)。
    #[test]
    fn command_box_is_dropped_on_narrow_windows() {
        // 900px 仍画在命令箱变宽的预算内(三段切换 76 → 150);500px 不画。
        let wide = Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, TITLEBAR_H));
        let narrow = Rect::from_min_size(Pos2::ZERO, egui::vec2(500.0, TITLEBAR_H));
        assert!(shows_command_box(wide), "900px 仍画命令箱");
        assert!(!shows_command_box(narrow), "500px 窗口不该画命令箱");
        // 阈值处的行为:-1px 不画,0px 画(边界选一边,不含糊)
        let at = Rect::from_min_size(
            Pos2::ZERO,
            egui::vec2(crate::ui::tokens::TITLE_CMD_MAX_W, TITLEBAR_H),
        );
        let below = Rect::from_min_size(
            Pos2::ZERO,
            egui::vec2(crate::ui::tokens::TITLE_CMD_MAX_W - 1.0, TITLEBAR_H),
        );
        assert!(shows_command_box(at), "正好等于阈值 → 画");
        assert!(!shows_command_box(below));
    }

    /// 无头跑命令箱:点 Live 段发 ToggleLivePreview,搜索框输入落到
    /// 变化发 SearchQueryChanged。
    #[test]
    fn titlebar_view_switch_and_search_are_wired() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        let mut outbox = Vec::new();
        let bar = Rect::from_min_max(Pos2::ZERO, Pos2::new(1240.0, TITLEBAR_H));
        let cbox = command_box_rect(bar);
        let switch = command_box_slot_rect(cbox, 1);
        let capsule = command_box_slot_rect(cbox, 0);
        // `super::ui` 读 `ui.max_rect()` 当整条标题栏,直接挂在根 Ui 上会
        // 拿到 900×600 —— 这里必须先把 max_rect 限到 36px 那条,坐标才与
        // `command_box_rect(bar)` 同源。
        let frame_fn = |state: &mut State, outbox: &mut Vec<Message>, events: Vec<Event>| {
            let output = ctx.run_ui(
                RawInput {
                    events,
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1240.0, 600.0))),
                    ..Default::default()
                },
                |ui| {
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar));
                    super::ui(&mut child, state, outbox);
                },
            );
            output.drop_without_applying_deltas();
        };

        frame_fn(&mut state, &mut outbox, Vec::new());
        // 点写作段(左半)→ ToggleLivePreview
        let live_center = egui::pos2(switch.left() + switch.width() * 0.25, switch.center().y);
        frame_fn(
            &mut state,
            &mut outbox,
            vec![Event::PointerMoved(live_center)],
        );
        frame_fn(&mut state, &mut outbox, vec![click(live_center, true)]);
        frame_fn(&mut state, &mut outbox, vec![click(live_center, false)]);
        assert!(
            outbox.contains(&Message::ToggleLivePreview),
            "点 Live 段应发 ToggleLivePreview,实际 {outbox:?}"
        );

        // 打字进搜索胶囊:先把焦点给 TextEdit(点它),再 Event::Text
        outbox.clear();
        let cap_center = capsule.center();
        frame_fn(
            &mut state,
            &mut outbox,
            vec![Event::PointerMoved(cap_center)],
        );
        frame_fn(&mut state, &mut outbox, vec![click(cap_center, true)]);
        frame_fn(&mut state, &mut outbox, vec![click(cap_center, false)]);
        assert!(
            outbox.contains(&Message::SearchPanelRevealed),
            "标题栏搜索框获得焦点应展开左侧搜索页,实际 {outbox:?}"
        );
        for ch in ["l", "a", "t"] {
            frame_fn(&mut state, &mut outbox, vec![Event::Text(ch.to_owned())]);
            frame_fn(&mut state, &mut outbox, Vec::new());
        }
        assert_eq!(state.search.query, "lat", "输入应落到 search.query");
        assert!(
            outbox.contains(&Message::SearchQueryChanged),
            "输入变化应发 SearchQueryChanged,实际 {outbox:?}"
        );
    }
}
