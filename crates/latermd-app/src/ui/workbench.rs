//! macOS 工作台外壳。系统负责窗口按钮,应用负责导航与文档工具。

use crate::command::Command;
use crate::settings::SettingsTab;
use crate::state::{Message, SidebarTab, State};
use crate::ui::icons::Icon;
use eframe::egui::{self, Align2, Rect, Sense};

pub const HEADER_H: f32 = 40.0;
const TRAFFIC_LIGHTS_W: f32 = 76.0;
const HEADER_RIGHT_W: f32 = 388.0;
/// 固定保留在标签滚动区外的拖窗空白。
pub(super) const HEADER_DRAG_W: f32 = 64.0;

/// 三栏共同遵守的编辑区宽度预算（含内边距）。
pub(crate) const EDITOR_MIN_W: f32 = 380.0;

/// 临时收窄窗口只改变显示宽度，不覆盖用户拖出的偏好；放大时自动恢复。
pub(crate) fn restore_panel_width(ui: &egui::Ui, name: &str, preferred: Option<f32>) {
    let id = egui::Id::new(name);
    let resizing = ui
        .read_response(id.with("__resize"))
        .is_some_and(|r| r.dragged() || r.drag_stopped());
    if !resizing {
        if let (Some(mut panel), Some(width)) = (
            egui::PanelState::load(ui.ctx(), id),
            preferred.filter(|v| v.is_finite() && *v > 0.0),
        ) {
            panel.outer_rect.max.x = panel.outer_rect.min.x + width;
            ui.ctx().data_mut(|data| data.insert_persisted(id, panel));
        }
    }
}

pub(crate) fn remember_panel_width(
    ui: &egui::Ui,
    name: &str,
    open: bool,
    preferred: &mut Option<f32>,
) {
    let id = egui::Id::new(name);
    let changed = ui
        .read_response(id.with("__resize"))
        .is_some_and(|r| r.drag_stopped());
    if open && (preferred.is_none() || changed) {
        if let Some(panel) = egui::PanelState::load(ui.ctx(), id) {
            *preferred = Some(panel.size().x);
        }
    }
}

/// 系统按钮的中心换算到 egui 坐标；AppKit 使用 point，egui 还可能有
/// 用户缩放，两者不能直接混用。无原生窗口的无头帧仍用标题栏中心。
fn header_center_y(ui: &egui::Ui, bar: Rect) -> f32 {
    #[cfg(target_os = "macos")]
    if let Some(center) = native_traffic_light_center(ui.ctx()) {
        if (0.0..=bar.height()).contains(&center) {
            return bar.top() + center;
        }
    }
    let _ = ui;
    bar.center().y
}

#[cfg(target_os = "macos")]
fn native_traffic_light_center(ctx: &egui::Context) -> Option<f32> {
    use objc2_app_kit::{NSApplication, NSWindowButton};
    use objc2_foundation::MainThreadMarker;
    let app = NSApplication::sharedApplication(MainThreadMarker::new()?);
    // mainWindow 保持文档窗口身份，不随打开文件对话框的 keyWindow 改变。
    let window = app.mainWindow().or_else(|| app.keyWindow())?;
    let content = window.contentView()?;
    let button = window.standardWindowButton(NSWindowButton::CloseButton)?;
    let bounds = content.bounds();
    let rect = button.convertRect_toView(button.bounds(), Some(&content));
    // 先在 contentView 的坐标系中处理翻转；backing 坐标方向未必相同。
    let backing_scale = content.convertRectToBacking(bounds).size.height / bounds.size.height;
    let middle = rect.origin.y + rect.size.height / 2.0;
    let top_down = if content.isFlipped() {
        middle - bounds.origin.y
    } else {
        bounds.origin.y + bounds.size.height - middle
    };
    Some((top_down * backing_scale) as f32 / ctx.pixels_per_point())
}

/// 仅用于独立工具栏标签：按字形墨迹居中，避开中英文字体不同的行盒留白。
/// 不修改正文的排版、基线或字体度量。
pub(crate) fn centered_label(ui: &egui::Ui, rect: Rect, label: &str, color: egui::Color32) {
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), egui::FontId::proportional(12.0), color);
    let pos = egui::pos2(
        rect.center().x - galley.size().x / 2.0,
        rect.center().y - galley.mesh_bounds.center().y,
    );
    ui.painter().galley(pos, galley, color);
}

/// UI chrome uses visible glyph bounds; document text keeps its normal baseline.
pub(crate) fn label_at(
    ui: &egui::Ui,
    left: f32,
    center_y: f32,
    text: &str,
    size: f32,
    color: egui::Color32,
) -> Rect {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), egui::FontId::proportional(size), color);
    let pos = egui::pos2(left, center_y - galley.mesh_bounds.center().y);
    let rect = Rect::from_min_size(pos, galley.size());
    ui.painter().galley(pos, galley, color);
    rect
}

pub(crate) fn small_icon(ui: &mut egui::Ui, icon: Icon, tip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), Sense::click());
    let colors = crate::theme::shell(ui);
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(rect, 5.0, colors.hover);
    }
    icon.draw(ui.painter(), rect.center(), 15.0, colors.secondary);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip));
    response
        .on_hover_text(tip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub(crate) fn status_bar(ui: &mut egui::Ui, state: &State, position: Option<(usize, usize)>) {
    let colors = crate::theme::shell(ui);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), Sense::hover());
    let y = rect.center().y;
    let tab = state.tabs.current();
    let (mcp, mcp_color) = match &state.mcp.status {
        crate::mcp::McpStatus::Listening(port) => (format!("MCP · {port}"), colors.secondary),
        crate::mcp::McpStatus::Failed(_) => ("MCP · 启动失败".into(), crate::ui::tokens::WARN),
        crate::mcp::McpStatus::Starting => ("MCP · 启动中".into(), colors.secondary),
        crate::mcp::McpStatus::Disabled => ("MCP 关闭".into(), colors.secondary),
    };
    let ai = if state.ai.is_streaming() {
        format!("{} · 生成中", state.ai.provider_label())
    } else {
        state.ai.provider_label().to_owned()
    };
    let mut right = rect.right() - 4.0;
    for (index, (text, color)) in [
        (mcp.as_str(), mcp_color),
        (ai.as_str(), colors.secondary),
        (state.theme.mode.label(), colors.secondary),
    ]
    .into_iter()
    .enumerate()
    {
        if index > 0 && rect.width() < crate::ui::tokens::STATUSBAR_DROP_AUX_W {
            break;
        }
        let text = crate::ui::tabs::elide_text(ui, text, &egui::FontId::proportional(11.0), 150.0);
        let galley = ui
            .painter()
            .layout_no_wrap(text, egui::FontId::proportional(11.0), color);
        right -= galley.size().x;
        ui.painter().galley(
            egui::pos2(right, y - galley.mesh_bounds.center().y),
            galley,
            color,
        );
        right -= 18.0;
    }
    let left = rect.left() + 4.0;
    let mut x = left;
    let saved = if tab.document.path.is_none() {
        "未存储"
    } else if tab.editor.is_dirty() {
        "未保存"
    } else {
        "已保存"
    };
    let mut details = vec![saved.to_owned()];
    if let Some((line, col)) = position {
        details.push(format!("行 {line} · 列 {col}"));
    }
    details.push(format!("{} 字", tab.editor.text().chars().count()));
    for (index, text) in details.into_iter().enumerate() {
        let galley =
            ui.painter()
                .layout_no_wrap(text, egui::FontId::proportional(11.0), colors.secondary);
        if x + galley.size().x > right {
            break;
        }
        if index > 0 {
            ui.painter().line_segment(
                [egui::pos2(x - 8.0, y - 4.0), egui::pos2(x - 8.0, y + 4.0)],
                separator(ui),
            );
        }
        let width = galley.size().x;
        ui.painter().galley(
            egui::pos2(x, y - galley.mesh_bounds.center().y),
            galley,
            colors.secondary,
        );
        x += width + 16.0;
    }
}

/// 分隔线只在鼠标靠近可拖动边界时显形；平时让面板靠底色和留白自然
/// 分组，避免三栏被硬线切碎。命中区域仍由 egui Panel 保持不变。
pub fn separator(ui: &egui::Ui) -> egui::Stroke {
    let colors = crate::theme::shell(ui);
    let visible = divider_is_hot(ui);
    egui::Stroke::new(
        1.0 / ui.ctx().pixels_per_point(),
        colors
            .border
            .gamma_multiply(if visible { 0.72 } else { 0.06 }),
    )
}

fn divider_is_hot(ui: &egui::Ui) -> bool {
    let resizing = ["nav", "preview"].into_iter().any(|name| {
        ui.read_response(egui::Id::new(name).with("__resize"))
            .is_some_and(|response| response.dragged() || response.drag_stopped())
    });
    if resizing {
        return true;
    }

    let Some(pointer) = ui.ctx().input(|input| input.pointer.hover_pos()) else {
        return false;
    };
    const SLOP: f32 = 7.0;
    ["nav", "preview"].into_iter().any(|name| {
        let Some(panel) = egui::PanelState::load(ui.ctx(), egui::Id::new(name)) else {
            return false;
        };
        let rect = panel.outer_rect;
        let near_vertical_edge =
            (pointer.x - rect.left()).abs() <= SLOP || (pointer.x - rect.right()).abs() <= SLOP;
        near_vertical_edge && pointer.y >= rect.top() - SLOP && pointer.y <= rect.bottom() + SLOP
    })
}

/// 浮窗统一的 macOS 风格卡片：柔和边界、内容色底和充足内距。
///
/// 2026-10-15 观感改版（对齐现代 Web 弹窗）：圆角提到 14、内距到 20，
/// 描边从「随分隔线热度变化」改为**常量** border 档（浮窗不是面板边界,
/// 描边不该跟 `divider_is_hot` 联动闪变），另加一层低偏移柔和投影把
/// 浮层从底面上托起来。颜色全部取自当前主题 token,明暗各自成立。
pub(crate) fn dialog_frame(ui: &egui::Ui) -> egui::Frame {
    let colors = crate::theme::shell(ui);
    egui::Frame::window(ui.style())
        .fill(colors.content)
        .stroke(egui::Stroke::new(
            1.0 / ui.ctx().pixels_per_point(),
            colors.border,
        ))
        .corner_radius(egui::CornerRadius::same(14))
        .inner_margin(egui::Margin::same(20))
        .shadow(egui::Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: egui::Color32::from_black_alpha(38),
        })
}

/// 弹窗统一的**初始位置**:视口正中。
///
/// 全项目弹窗都从这里起手([`dialog_frame`] 配套),别再各写各的:
/// 漏 `pivot` 会让 egui 按**左上角**对齐锚点(窗向右下偏出屏幕),完全不
/// 设位置则落到 `automatic_area_position` 的层叠算法上(同一屏开过几个
/// 弹窗后,新弹窗会叠在前一个上面),而硬编码 `[80,120]` 会在宽屏上偏到
/// 左上角 —— 这三种都实测踩过。
///
/// 用 `default_pos` 而非 `fixed_pos`:居中是**首次打开**的落点,用户拖走
/// 后由 Area 按窗口 id 记忆(与 macOS/VS Code 一致),关掉再开才回到中间。
/// 真要每帧钉死在某处(find/goto 浮卡那样)才用 `fixed_pos`,但那会连带
/// `movable(false)`,拖不动。
pub(crate) fn centered_dialog<'a>(ui: &egui::Ui, title: &'a str) -> egui::Window<'a> {
    // pivot 决定锚点落在窗的哪一角:居中锚点 + 视口中心 = 窗口居中,
    // 与窗口实际尺寸无关(默认 LEFT_TOP 会让窗往右下甩)。
    egui::Window::new(title)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ui.ctx().viewport_rect().center())
}

/// 对话框标题行(现代 Web 弹窗式):加粗标题居左,× 关闭钮居右,下方
/// 一条细分隔线把标题区与正文分开。返回 × 是否被点 —— 与 footer 的
/// 「取消」同语义,由调用方并入取消通路(浮窗本身不发消息)。
///
/// 行内用 `right_to_left(Align::Min)` 把 × 推到右缘:cross 取 Min 而非
/// Center,子 Ui 贴内容高、不申领 Window 剩余高度(destructive_confirm
/// 2026-10-10 的踩坑记录:Center 在 Window 里会把窗撑高、按钮跨帧漂移)。
pub(crate) fn dialog_header(ui: &mut egui::Ui, title: &str) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title)
                .size(crate::ui::tokens::text::TITLE)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            close = small_icon(ui, crate::ui::icons::Icon::Close, "关闭").clicked();
        });
    });
    ui.add_space(crate::ui::tokens::SPACE_XS);
    ui.separator();
    ui.add_space(crate::ui::tokens::SPACE_SM);
    close
}

/// 对话框页脚分隔线:按钮行与正文之间的一条细线(与 [`dialog_header`]
/// 的标题分隔线同一档,页脚右侧放按钮行由各对话框自己排)。
pub(crate) fn dialog_footer_divider(ui: &mut egui::Ui) {
    ui.add_space(crate::ui::tokens::SPACE_SM);
    ui.separator();
    ui.add_space(crate::ui::tokens::SPACE_SM);
}

fn action(ui: &mut egui::Ui, rect: Rect, icon: Icon, tip: &str, selected: bool) -> bool {
    let response = ui.allocate_rect(rect, Sense::click());
    let colors = crate::theme::shell(ui);
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(rect, 6.0, colors.hover);
    }
    let ink = if selected {
        colors.text
    } else {
        colors.secondary
    };
    // 侧栏按钮使用细线框与内分隔,不再画厚实心条。
    if matches!(icon, Icon::Sidebar) {
        let bounds = Rect::from_center_size(rect.center(), egui::vec2(17.0, 13.0));
        let stroke = egui::Stroke::new(1.2, ink);
        ui.painter()
            .rect_stroke(bounds, 2.5, stroke, egui::StrokeKind::Inside);
        let x = bounds.left() + 5.0;
        if selected {
            let area = if icon == Icon::Sidebar {
                Rect::from_min_max(bounds.min, egui::pos2(x, bounds.bottom()))
            } else {
                Rect::from_min_max(egui::pos2(x, bounds.top()), bounds.max)
            };
            ui.painter()
                .rect_filled(area.shrink(1.5), 1.0, ink.gamma_multiply(0.15));
        }
        ui.painter().line_segment(
            [
                egui::pos2(x, bounds.top() + 1.0),
                egui::pos2(x, bounds.bottom() - 1.0),
            ],
            stroke,
        );
    } else {
        icon.draw(ui.painter(), rect.center(), 16.0, ink);
    }
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, tip));
    response
        .on_hover_text(tip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// 空白处可拖窗;所有命令仍走既有消息归约,不抢编辑器焦点。
pub fn header(ui: &mut egui::Ui, state: &mut State, outbox: &mut Vec<Message>) {
    let bar = ui.max_rect();
    let center_y = header_center_y(ui, bar);
    let drag_rect = Rect::from_min_max(
        bar.left_top() + egui::vec2(TRAFFIC_LIGHTS_W, 0.0),
        bar.right_bottom(),
    );
    let drag = ui.allocate_rect(drag_rect, Sense::click_and_drag());
    if drag.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if drag.double_clicked() {
        let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    }
    let slot = |x: f32, w: f32| {
        Rect::from_center_size(egui::pos2(x + w / 2.0, center_y), egui::vec2(w, 28.0))
    };
    let colors = crate::theme::shell(ui);
    let right = bar.right() - 12.0;
    let search_width = if bar.width() < 1100.0 { 28.0 } else { 196.0 };
    let right_start = right - (HEADER_RIGHT_W - 196.0 + search_width);
    let sidebar_x = bar.left() + TRAFFIC_LIGHTS_W + 6.0;
    let divider_x = sidebar_x + 40.0;
    let logo_size = crate::ui::tokens::BRAND_LOGO;
    let logo_y = center_y - logo_size / 2.0;
    let logo = Rect::from_min_size(
        egui::pos2(sidebar_x + 40.0 + 12.0, logo_y),
        egui::vec2(logo_size, logo_size),
    );
    // macOS 已经由系统标题栏提供应用身份，工作台标题栏不再重复显示
    // LaterMD 品牌图标；Windows/Linux 保留原有品牌区布局。
    if !cfg!(target_os = "macos") {
        crate::ui::titlebar::paint_brand_logo(ui, logo);
    }
    let title_left = if cfg!(target_os = "macos") {
        divider_x
    } else {
        logo.right() + crate::ui::tokens::SPACE_SM
    };
    if action(
        ui,
        slot(sidebar_x, 28.0),
        Icon::Sidebar,
        "显示 / 隐藏侧栏",
        state.layout.left,
    ) {
        outbox.push(Command::ToggleSidebar.message());
    }
    ui.painter().line_segment(
        [
            egui::pos2(divider_x, center_y - 8.0),
            egui::pos2(divider_x, center_y + 8.0),
        ],
        separator(ui),
    );
    let title_rect = Rect::from_min_max(
        egui::pos2(title_left, center_y - 14.0),
        egui::pos2(right_start - HEADER_DRAG_W, center_y + 14.0),
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
        let workspace = state
            .file_tree
            .root
            .as_deref()
            .map(crate::ui::sidebar::recent_label)
            .unwrap_or_else(|| "LaterMD".to_owned());
        let font = egui::FontId::proportional(12.0);
        let title = crate::ui::tabs::elide_text(ui, &workspace, &font, title_rect.width().max(0.0));
        ui.painter().with_clip_rect(title_rect).text(
            title_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            title,
            font,
            colors.text,
        );
    }
    mode_switch(ui, slot(right_start, 100.0), state, outbox);
    let search_rect = slot(right_start + 112.0, search_width);
    if search_width < 100.0 {
        compact_search(ui, search_rect, state, outbox);
    } else {
        search_field(ui, search_rect, &mut state.search, outbox);
    }
    ui.painter().line_segment(
        [
            egui::pos2(right - 72.0, center_y - 8.0),
            egui::pos2(right - 72.0, center_y + 8.0),
        ],
        separator(ui),
    );
    if action(ui, slot(right - 28.0, 28.0), Icon::Settings, "设置", false) {
        outbox.push(Message::SettingsOpened(SettingsTab::Appearance));
    }
}

/// 窄窗口保留搜索入口，点击进入既有全文搜索页。
pub(super) fn compact_search(
    ui: &mut egui::Ui,
    rect: Rect,
    state: &State,
    outbox: &mut Vec<Message>,
) {
    if action(
        ui,
        rect,
        Icon::Search,
        "搜索文档",
        state.layout.left_view == SidebarTab::Search && state.layout.left,
    ) {
        if !state.layout.left {
            outbox.push(Message::SidebarToggled);
        }
        outbox.push(Message::SidebarTabChanged(SidebarTab::Search));
    }
}

pub(super) fn mode_switch(ui: &mut egui::Ui, rect: Rect, state: &State, outbox: &mut Vec<Message>) {
    let colors = crate::theme::shell(ui);
    let live = state.render_mode == crate::live::RenderMode::Live;
    ui.painter().rect_filled(rect, 7.0, colors.hover);
    for (index, label) in ["写作", "源码"].into_iter().enumerate() {
        let part = Rect::from_min_size(
            rect.min + egui::vec2(index as f32 * rect.width() / 2.0, 0.0),
            egui::vec2(rect.width() / 2.0, rect.height()),
        )
        .shrink(2.0);
        let selected = (index == 0) == live;
        let response = ui.allocate_rect(part, Sense::click());
        if selected {
            ui.painter().rect_filled(part, 5.0, colors.content);
            ui.painter()
                .rect_stroke(part, 5.0, separator(ui), egui::StrokeKind::Inside);
        } else if response.hovered() {
            ui.painter()
                .rect_filled(part, 5.0, crate::theme::shell(ui).chrome);
        }
        centered_label(
            ui,
            part,
            label,
            if selected {
                colors.text
            } else {
                colors.secondary
            },
        );
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, label)
        });
        if response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(if index == 0 {
                "写作：直接编辑正文，当前段落显示 Markdown"
            } else {
                "源码：编辑 Markdown，可按需打开对照预览"
            })
            .clicked()
            && !selected
        {
            outbox.push(Message::ToggleLivePreview);
        }
    }
}

fn search_field(
    ui: &mut egui::Ui,
    rect: Rect,
    search: &mut crate::search::SearchState,
    outbox: &mut Vec<Message>,
) {
    let colors = crate::theme::shell(ui);
    let id = ui.make_persistent_id("workbench-search");
    let focused = ui.memory(|m| m.has_focus(id));
    ui.painter().rect_filled(rect, 6.0, colors.content);
    ui.painter().rect_stroke(
        rect,
        6.0,
        if focused {
            egui::Stroke::new(1.0, colors.accent.gamma_multiply(0.65))
        } else {
            separator(ui)
        },
        egui::StrokeKind::Inside,
    );
    Icon::Search.draw(
        ui.painter(),
        egui::pos2(rect.left() + 13.0, rect.center().y),
        14.0,
        colors.secondary,
    );
    let has_query = !search.query.is_empty();
    let edit_rect = Rect::from_min_max(
        egui::pos2(rect.left() + 27.0, rect.top() + 5.0),
        egui::pos2(
            rect.right() - if has_query { 25.0 } else { 8.0 },
            rect.bottom() - 5.0,
        ),
    );
    let response = ui.put(
        edit_rect,
        egui::TextEdit::singleline(&mut search.query)
            .id(id)
            .font(egui::FontId::proportional(12.0))
            .hint_text("搜索文档…")
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .vertical_align(egui::Align::Center),
    );
    if response.changed() {
        outbox.push(Message::SearchQueryChanged);
    }
    response.on_hover_text("全文搜索 · 结果显示在左侧搜索页");
    if has_query
        && action(
            ui,
            Rect::from_center_size(
                egui::pos2(rect.right() - 13.0, rect.center().y),
                egui::vec2(20.0, 20.0),
            ),
            Icon::Close,
            "清空搜索",
            false,
        )
    {
        search.query.clear();
        outbox.push(Message::SearchQueryChanged);
    }
}

/// 目录名、最近目录和打开目录入口合成一个控件，长路径只在悬停时显示。
pub fn workspace_picker(
    ui: &mut egui::Ui,
    tree: &crate::filetree::FileTreeState,
    outbox: &mut Vec<Message>,
) {
    let colors = crate::theme::shell(ui);
    let label = tree
        .root
        .as_deref()
        .map(crate::ui::sidebar::recent_label)
        .unwrap_or_else(|| "打开文件夹…".to_owned());
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), Sense::click());
    ui.painter().rect_filled(
        rect,
        6.0,
        if response.hovered() {
            colors.hover
        } else {
            colors.content.gamma_multiply(0.45)
        },
    );
    ui.painter()
        .rect_stroke(rect, 6.0, separator(ui), egui::StrokeKind::Inside);
    Icon::FolderClosed.draw(
        ui.painter(),
        egui::pos2(rect.left() + 15.0, rect.center().y),
        15.0,
        colors.secondary,
    );
    let font = egui::FontId::proportional(12.0);
    let text = crate::ui::tabs::elide_text(ui, &label, &font, (rect.width() - 58.0).max(0.0));
    ui.painter().text(
        egui::pos2(rect.left() + 30.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        font,
        colors.text,
    );
    let c = egui::pos2(rect.right() - 13.0, rect.center().y);
    ui.painter().add(egui::Shape::line(
        vec![
            c + egui::vec2(-3.0, -1.5),
            c + egui::vec2(0.0, 1.5),
            c + egui::vec2(3.0, -1.5),
        ],
        egui::Stroke::new(1.2, colors.secondary),
    ));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, &label));
    let response = response
        .on_hover_text(
            tree.root
                .as_deref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "选择 Markdown 文件夹".to_owned()),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    egui::Popup::menu(&response).show(|ui| {
        ui.set_min_width((rect.width() - 16.0).max(120.0));
        for dir in &tree.recents {
            if ui
                .selectable_label(
                    tree.root.as_deref() == Some(dir.as_path()),
                    crate::ui::sidebar::recent_label(dir),
                )
                .on_hover_text(dir.display().to_string())
                .clicked()
            {
                outbox.push(Message::FileTreeRootSelected(dir.clone()));
                ui.close();
            }
        }
        if !tree.recents.is_empty() {
            ui.separator();
        }
        if ui.button("打开文件夹…").clicked() {
            outbox.push(Message::FileTreeRootPick);
            ui.close();
        }
    });
    ui.add_space(10.0);
}

/// 返回导航占用的矩形,内容面板接在下方并独立滚动。
pub fn navigation(ui: &mut egui::Ui, active: SidebarTab, outbox: &mut Vec<Message>) -> Rect {
    let colors = crate::theme::shell(ui);
    let (bar, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), Sense::hover());
    let more_width = 28.0;
    let width = (bar.width() - more_width - 4.0) / 3.0;
    for (index, tab) in [SidebarTab::Outline, SidebarTab::Files, SidebarTab::Search]
        .into_iter()
        .enumerate()
    {
        let rect = Rect::from_min_size(
            bar.min + egui::vec2(index as f32 * width, 0.0),
            egui::vec2(width - 2.0, 28.0),
        );
        let response = ui.interact(
            rect,
            ui.id().with(("workbench-view", index)),
            Sense::click(),
        );
        if tab == active || response.hovered() || response.has_focus() {
            ui.painter().rect_filled(
                rect,
                5.0,
                if tab == active {
                    colors.selected_bg
                } else {
                    colors.hover
                },
            );
        }
        centered_label(
            ui,
            rect,
            tab.label(),
            if tab == active {
                colors.text
            } else {
                colors.secondary
            },
        );
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                true,
                tab == active,
                tab.label(),
            )
        });
        if response.on_hover_text(tab.label()).clicked() {
            outbox.push(Message::SidebarTabChanged(tab));
        }
    }
    let rect = Rect::from_min_size(
        egui::pos2(bar.right() - more_width, bar.top()),
        egui::vec2(more_width, 28.0),
    );
    let response = ui.interact(rect, ui.id().with("workbench-tools"), Sense::click());
    let selected = matches!(active, SidebarTab::Git | SidebarTab::Backlinks);
    if selected || response.hovered() || response.has_focus() {
        ui.painter().rect_filled(
            rect,
            5.0,
            if selected {
                colors.selected_bg
            } else {
                colors.hover
            },
        );
    }
    for offset in [-5.0, 0.0, 5.0] {
        ui.painter().circle_filled(
            rect.center() + egui::vec2(offset, 0.0),
            1.3,
            colors.secondary,
        );
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "更多视图"));
    let response = response.on_hover_text("更多视图 · Git / 反向链接");
    egui::Popup::menu(&response).show(|ui| {
        for tab in [SidebarTab::Git, SidebarTab::Backlinks] {
            if ui
                .selectable_label(
                    active == tab,
                    if tab == SidebarTab::Backlinks {
                        "反向链接"
                    } else {
                        tab.label()
                    },
                )
                .clicked()
            {
                outbox.push(Message::SidebarTabChanged(tab));
                ui.close();
            }
        }
    });
    ui.add_space(8.0);
    bar
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, Pos2};

    /// 弹窗初始位置守门:首次打开必须**几何居中**于视口。
    ///
    /// 判据是窗口矩形中心与视口中心的距离,而不是「pivot 设了没有」——
    /// 后者会被实现细节(谁调 helper)牵着走,前者是用户实际看到的东西。
    /// 覆盖两类历史坑:① 只给 `default_pos` 不给 `pivot`(锚点落在左上角,
    /// 整窗向右下甩);② 干脆不设位置(落到 `automatic_area_position` 的
    /// 层叠位)。两个尺寸差异极大的弹窗都测:窄高(AI 润色 380×240)与
    /// 宽矮(commit),两者偏了才会暴露「按宽度对半」的错误实现。
    #[test]
    fn centered_dialog_lands_window_center_on_viewport_center() {
        for title in ["AI: commit message", "AI 润色", "关闭标签"] {
            let ctx = egui::Context::default();
            let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
            let mut win = None;
            // 跑两帧:首帧是 sizing pass,Area 尺寸此时才落定
            for _ in 0..2 {
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(viewport),
                        ..Default::default()
                    },
                    |ui| {
                        centered_dialog(ui, title)
                            .resizable(false)
                            .show(ui.ctx(), |ui| {
                                ui.label("正文");
                            });
                        win = ui.ctx().memory(|m| m.area_rect(egui::Id::new(Some(title))));
                    },
                )
                .drop_without_applying_deltas();
            }
            let win = win.expect("浮窗 Area 已注册");
            // 容差 1.0 而非 0:egui 的 `round_ui` 把坐标对齐到半像素网格
            // (`left_top_pos` 走它),奇数尺寸的窗会差 0.5px,视觉不可见。
            let drift = (win.center() - viewport.center()).length();
            assert!(
                drift <= 1.0,
                "{title} 未居中:窗中心 {:?} vs 视口中心 {:?}(偏移 {drift:.2})",
                win.center(),
                viewport.center()
            );
        }
    }

    /// 反向守门:所有**弹窗**都必须经 `centered_dialog` 起手,漏一处就
    /// 会退回自动布局/角落硬编码。用源码扫描兜住「新加弹窗忘了居中」——
    /// 这类退化编译器不管,只有几何断言会,而几何断言逐个弹窗写一遍成本
    /// 太高。**浮卡例外**:find / goto 浮层是贴编辑器右上角的工具卡
    /// (`fixed_pos` + `RIGHT_TOP` pivot),不是弹窗,居中反而是错的。
    #[test]
    fn every_dialog_window_starts_from_centered_dialog() {
        /// 例外:① 编辑器浮卡(工具卡贴边,居中即错),按浮窗标题白名单
        /// 放行;② helper 自身的 `Window::new(title)`(非字面量标题)。
        const OVERLAY_CARDS: [&str; 2] = ["文档内查找", "跳转到行"];
        const SELF: &str = "<非字面量标题>";

        /// 递归收集 `src/` 下所有 `.rs`(子目录必须下潜:第一版只扫顶层,
        /// `src/ui/` 整个漏掉,改坏 `ui/tabs.rs` 的弹窗测试照样绿 ——
        /// 「守门没守住」比没守门更坏,靠红绿验证才发现)。
        fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    rust_files(&path, out);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    out.push(path);
                }
            }
        }

        let mut files = Vec::new();
        rust_files(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        assert!(!files.is_empty(), "src 下没扫到 .rs 文件(路径失效?)");

        let mut offenders = Vec::new();
        for path in files {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (i, line) in src.lines().enumerate() {
                let trimmed = line.trim();
                if !trimmed.starts_with("egui::Window::new(") {
                    continue;
                }
                // 解析窗口标题字面量(形如 egui::Window::new("设置"));
                // 解析不出字面量 = 形参标题,只可能是 helper 自己
                let title = trimmed
                    .split_once('"')
                    .and_then(|(_, rest)| rest.split_once('"'))
                    .map(|(title, _)| title)
                    .unwrap_or(SELF);
                if OVERLAY_CARDS.contains(&title) || title == SELF {
                    continue;
                }
                offenders.push(format!(
                    "{}:{}:{}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    i + 1,
                    title
                ));
            }
        }
        assert!(
            offenders.is_empty(),
            "这些弹窗没用 centered_dialog(会飘到角落/自动层叠位):{}",
            offenders.join(", ")
        );
    }

    fn click(pos: Pos2) -> Vec<Event> {
        vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn compact_navigation_fits_narrow_sidebar_and_switches_views() {
        for width in [156.0, 216.0, 376.0] {
            for (index, tab) in [SidebarTab::Outline, SidebarTab::Files, SidebarTab::Search]
                .into_iter()
                .enumerate()
            {
                let ctx = egui::Context::default();
                let mut outbox = Vec::new();
                let mut bar = Rect::NOTHING;
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            egui::vec2(width, 400.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        bar = navigation(ui, SidebarTab::Files, &mut outbox);
                    },
                )
                .drop_without_applying_deltas();
                assert!(bar.height() <= 32.0 && bar.width() <= width);
                let pos = egui::pos2(
                    bar.left() + (index as f32 + 0.5) * (bar.width() - 32.0) / 3.0,
                    bar.center().y,
                );
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            egui::vec2(width, 400.0),
                        )),
                        events: click(pos),
                        ..Default::default()
                    },
                    |ui| {
                        navigation(ui, SidebarTab::Files, &mut outbox);
                    },
                )
                .drop_without_applying_deltas();
                assert!(outbox.contains(&Message::SidebarTabChanged(tab)));
            }
        }
    }

    #[test]
    fn more_views_keeps_git_and_backlinks_reachable() {
        for (label, tab) in [
            ("Git", SidebarTab::Git),
            ("反向链接", SidebarTab::Backlinks),
        ] {
            let ctx = egui::Context::default();
            let mut outbox = Vec::new();
            let mut bar = Rect::NOTHING;
            let mut frame = |events| {
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            egui::vec2(216.0, 400.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        bar = navigation(ui, SidebarTab::Files, &mut outbox);
                    },
                )
            };
            frame(Vec::new()).drop_without_applying_deltas();
            frame(click(egui::pos2(202.0, 14.0))).drop_without_applying_deltas();
            let output = frame(Vec::new());
            let target = output
                .shapes
                .iter()
                .find_map(|shape| {
                    if let egui::Shape::Text(text) = &shape.shape {
                        (text.galley.text() == label).then(|| text.pos + text.galley.size() / 2.0)
                    } else {
                        None
                    }
                })
                .expect("更多菜单保留两个工具入口");
            output.drop_without_applying_deltas();
            frame(click(target)).drop_without_applying_deltas();
            assert!(outbox.contains(&Message::SidebarTabChanged(tab)));
        }
    }

    #[test]
    fn mode_segment_only_switches_when_selecting_the_other_mode() {
        for dark in [false, true] {
            for live in [false, true] {
                let ctx = egui::Context::default();
                ctx.set_visuals(if dark {
                    egui::Visuals::dark()
                } else {
                    egui::Visuals::light()
                });
                let mut state = State::default();
                state.render_mode = if live {
                    crate::live::RenderMode::Live
                } else {
                    crate::live::RenderMode::Source
                };
                let mut outbox = Vec::new();
                let rect = Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(100.0, 28.0));
                for events in [
                    vec![],
                    click(egui::pos2(if !live { 95.0 } else { 45.0 }, 34.0)),
                ] {
                    ctx.run_ui(
                        egui::RawInput {
                            events,
                            ..Default::default()
                        },
                        |ui| mode_switch(ui, rect, &state, &mut outbox),
                    )
                    .drop_without_applying_deltas();
                }
                assert!(outbox.is_empty(), "点击当前模式不能反转");
                ctx.run_ui(
                    egui::RawInput {
                        events: click(egui::pos2(if !live { 45.0 } else { 95.0 }, 34.0)),
                        ..Default::default()
                    },
                    |ui| mode_switch(ui, rect, &state, &mut outbox),
                )
                .drop_without_applying_deltas();
                assert!(matches!(outbox.as_slice(), [Message::ToggleLivePreview]));
            }
        }
    }

    #[test]
    fn search_typing_and_clear_keep_the_existing_query_message() {
        let ctx = egui::Context::default();
        let mut search = crate::search::SearchState::default();
        let mut outbox = Vec::new();
        let rect = Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(196.0, 28.0));
        let mut frame = |events| {
            ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| search_field(ui, rect, &mut search, &mut outbox),
            )
            .drop_without_applying_deltas();
        };
        frame(vec![]);
        frame(click(egui::pos2(65.0, 34.0)));
        frame(vec![Event::Text("文档".to_owned())]);
        frame(vec![]);
        frame(click(egui::pos2(203.0, 34.0)));
        assert!(search.query.is_empty());
        assert_eq!(
            outbox
                .iter()
                .filter(|m| matches!(m, Message::SearchQueryChanged))
                .count(),
            2
        );
    }
    // 2026-10-10 起状态栏全平台同实现,测试解除 mac 门控;无系统 CJK
    // 字体的环境(精简 CI)优雅跳过 —— 打包兜底字体在的机器照常覆盖
    #[test]
    fn status_glyphs_stay_centered_and_do_not_overlap_when_resized() {
        for scale in [1.0, 1.5, 2.0] {
            for width in [360.0, 900.0] {
                let ctx = egui::Context::default();
                if crate::fonts::install(&ctx).is_none() {
                    return;
                }
                ctx.set_pixels_per_point(scale);
                // Apply egui's pending zoom before measuring viewport geometry.
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 100.0),
                        )),
                        ..Default::default()
                    },
                    |_| {},
                )
                .drop_without_applying_deltas();
                let mut state = State::default();
                state.tabs.current_mut().document.path = Some(std::path::PathBuf::from(
                    "这是一个很长的文件名称-system-font-alignment.md",
                ));
                let mut center = 0.0;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 100.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        center = ui.cursor().top() + 12.0;
                        status_bar(ui, &state, Some((12, 8)));
                    },
                );
                output.textures_delta.clear();
                let mut bounds = Vec::new();
                for shape in &output.shapes {
                    if let egui::Shape::Text(text) = &shape.shape {
                        let r = text.galley.mesh_bounds.translate(text.pos.to_vec2());
                        assert!((r.center().y - center).abs() * scale <= 0.5);
                        assert!(
                            r.left() >= 0.0 && r.right() <= width,
                            "{}: {r:?}, width={width}, scale={scale}",
                            text.galley.text()
                        );
                        bounds.push(r);
                    }
                }
                output.textures_delta.clear();
                assert!(bounds.len() >= 2);
                bounds.sort_by(|a, b| a.left().total_cmp(&b.left()));
                for pair in bounds.windows(2) {
                    assert!(pair[0].right() < pair[1].left());
                }
            }
        }
    }

    // 验证 macOS 的实际中英文字体；其他平台 CI 不要求预装 CJK 系统字体。
    #[cfg(target_os = "macos")]
    #[test]
    fn source_and_live_glyphs_share_the_segment_center() {
        for scale in [1.0, 1.5, 2.0] {
            let ctx = egui::Context::default();
            crate::fonts::install(&ctx).expect("alignment test requires installed CJK fonts");
            ctx.set_pixels_per_point(scale);
            let rect = Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(100.0, 28.0));
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                mode_switch(ui, rect, &State::default(), &mut Vec::new());
            });
            let mut centers = Vec::new();
            fn inspect(shape: &egui::Shape, centers: &mut Vec<f32>) {
                match shape {
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            inspect(shape, centers);
                        }
                    }
                    egui::Shape::Text(text) if ["写作", "源码"].contains(&text.galley.text()) =>
                    {
                        let mut bounds = Rect::NOTHING;
                        for row in &text.galley.rows {
                            for glyph in &row.glyphs {
                                let min = text.pos
                                    + row.pos.to_vec2()
                                    + glyph.pos.to_vec2()
                                    + glyph.uv_rect.offset;
                                bounds |= Rect::from_min_size(min, glyph.uv_rect.size);
                            }
                        }
                        centers.push(bounds.center().y);
                    }
                    _ => {}
                }
            }
            for shape in &output.shapes {
                inspect(&shape.shape, &mut centers);
            }
            output.textures_delta.clear();
            assert_eq!(centers.len(), 2);
            for y in centers {
                assert!(
                    (y - rect.center().y).abs() * scale <= 0.5,
                    "glyph center {y} must match control center {} at scale {scale}",
                    rect.center().y
                );
            }
        }
    }
}
