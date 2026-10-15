//! macOS 工作台外壳。系统负责窗口按钮,应用负责导航与文档工具。

use crate::command::Command;
use crate::settings::SettingsTab;
use crate::state::{Message, SidebarTab, State};
use crate::ui::icons::Icon;
use eframe::egui::{self, Align2, Rect, Sense};

pub const HEADER_H: f32 = 40.0;
const TRAFFIC_LIGHTS_W: f32 = 76.0;
const HEADER_RIGHT_W: f32 = 388.0;
/// 源码 / 写作 / 源码+对照 三段切换的总宽(三段各 ~50)。
const MODE_SWITCH_W: f32 = 150.0;
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

/// 可拖动面板的名字清单(`divider_is_hot` 与分隔线绘制共用):左栏导航 +
/// 只读对照列(三态视图右栏)。
const RESIZABLE_PANELS: [&str; 2] = ["nav", crate::ui::layout::COMPARE_PANEL_ID];

fn divider_is_hot(ui: &egui::Ui) -> bool {
    let resizing = RESIZABLE_PANELS.into_iter().any(|name| {
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
    RESIZABLE_PANELS.into_iter().any(|name| {
        let Some(panel) = egui::PanelState::load(ui.ctx(), egui::Id::new(name)) else {
            return false;
        };
        let rect = panel.outer_rect;
        let near_vertical_edge =
            (pointer.x - rect.left()).abs() <= SLOP || (pointer.x - rect.right()).abs() <= SLOP;
        near_vertical_edge && pointer.y >= rect.top() - SLOP && pointer.y <= rect.bottom() + SLOP
    })
}

/// 对照列左缘的**常显**分隔线:源码区与只读对照列之间的那条边界。
///
/// **为什么必须自绘而不开 egui 的 `show_separator_line(true)`**:
/// ① 它的空闲色硬绑 `visuals.widgets.noninteractive.bg_stroke` —— 那个槽位
/// 同时是 Markdown 表格边框与隔行底色的取色源(`theme.rs` §noninteractive
/// 注释),为一条面板边界去动它 = 顺带改掉全部表格观感,且本函数无法只改
/// 自己那一份(样式是全局单值)。② 它画在 `parent_ui.painter()` 上,而
/// `parent_ui` 的 painter 位于**其后添加的 panel 之下** —— 对照列是 ④、
/// 中央编辑区是 ⑤,后者的 `Frame::fill` 会把线整根盖掉,开了等于没开。
///
/// 故走 `layer_painter` 在所有 panel 之上单画一根竖线。取值与「间隔 vs
/// 边界」的判据见 [`crate::ui::tokens::PREVIEW_DIVIDER_IDLE`]。
///
/// **必须在全部 panel 之后调用**(与 `edge_resize_zones` 同一约束):layer
/// painter 的层序由 `Order` 决定,同一 `Order::Middle` 内按 id 排序,这里
/// 用独立 id 开一层,天然压过所有 panel 的 painter。
///
/// `panel_id` 显式入参:本函数不假设面板叫什么(对照列与旧的「对照预览」
/// 是两代实现,名字不同)。
pub(crate) fn panel_divider(ui: &egui::Ui, panel_id: &str, open: bool) {
    if !open {
        return;
    }
    let id = egui::Id::new(panel_id);
    // **边界位置的取法是三段递降,每一段都是被实测逼出来的**(2026-10-13
    // 坤哥反馈「线不跟着我拉的框走」):
    //
    // ① 拖动中 → 用 `interact_pointer_pos()`,**不能用** `Response.rect`
    //    也不能用 `PanelState`。egui 的执行顺序是
    //    「`resize_panel(old_outer_rect)` 交互 → 若在拖则按 pointer 重算
    //    `outer_size`」(`panel.rs` 735-800),两者**都不是**本帧的新边界:
    //    `PanelState` 在拖动期间被 `resize_drag_in_progress` 守卫跳过写入
    //    (存的是拖动前的宽度,以支持「拖到最小再拖回来」),`Response.rect`
    //    又是拿旧 `outer_rect` 算的。实测探针:五步拖动两者恒为 980.0。
    //    而 `Panel::right` 的左缘**在拖动时就是指针 x 本身**,故直接取之。
    // ② 空闲 → 用 `PanelState` 的左缘(response 交互带在上一帧算完,同样滞后,
    //    且 `size_range` 的 clamp 会让实际左缘 ≠ 指针 x)。
    // ③ 都不成(panel 尚未 show 过)→ 不画线。
    let resize = ui.read_response(id.with("__resize"));
    let dragging = resize
        .as_ref()
        .is_some_and(|r| r.dragged() || r.drag_stopped());
    let pointer_x = resize
        .as_ref()
        .filter(|_| dragging)
        .and_then(|r| r.interact_pointer_pos())
        .map(|p| p.x);
    let Some(panel) = egui::PanelState::load(ui.ctx(), id) else {
        return;
    };
    let edge = pointer_x.unwrap_or(panel.outer_rect.left());
    let (top, bottom) = (panel.outer_rect.top(), panel.outer_rect.bottom());
    if bottom - top <= 0.0
        || !ui.clip_rect().intersects(egui::Rect::from_min_max(
            egui::pos2(edge, top),
            egui::pos2(edge, bottom),
        ))
    {
        return;
    }
    let colors = crate::theme::shell(ui);
    // hover 用**两个通道**反馈:①线宽 1px → 3px(主信号)②色深朝 `border`
    // 本色靠(辅助)。只改颜色时,1px 线的深浅变化在指针扫过时几乎捕捉不到
    // (2026-10-13 坤哥实测「没看见 hover 加粗」),VS Code 的 sash 正是靠
    // 加粗给主反馈。hot 走**几何判定**,让测试在裸调路径下也成立。
    let grab = crate::ui::tokens::PREVIEW_DIVIDER_GRAB;
    let hot = ui
        .ctx()
        .input(|input| input.pointer.hover_pos())
        .is_some_and(|p| (p.x - edge).abs() <= grab && p.y >= top && p.y <= bottom);
    let ppp = ui.ctx().pixels_per_point();
    let stroke = egui::Stroke::new(
        if hot {
            crate::ui::tokens::PREVIEW_DIVIDER_HOT_W
        } else {
            1.0
        } / ppp,
        // 朝**底色**插值而非 `gamma_multiply` —— 后者朝黑缩放,浅色皮肤上
        // 「变淡」会变成「变深」,空闲态反比 hover 态显眼。
        colors.border.lerp_to_gamma(
            colors.content,
            if hot {
                crate::ui::tokens::PREVIEW_DIVIDER_HOT
            } else {
                crate::ui::tokens::PREVIEW_DIVIDER_IDLE
            },
        ),
    );
    let x = edge - 0.5 * stroke.width;
    ui.ctx()
        .layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            id.with("__divider"),
        ))
        .vline(x, egui::Rangef::new(top, bottom), stroke);
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
/// **标题色显式取正文主文字色,不走 `RichText::strong()` 的默认前景**
/// (2026-10-15 坤哥「重命名是蓝的、删除是深色,蓝色像链接」):强调色在
/// 本项目里已被链接 / 活动页签 / 选中文字占用,标题再吃它就成了第三种
/// 「可点」暗示。层级靠**字号 + 字重 + 上下留白**表达,不靠颜色。
///
/// 行内用 `right_to_left(Align::Min)` 把 × 推到右缘:cross 取 Min 而非
/// Center,子 Ui 贴内容高、不申领 Window 剩余高度(destructive_confirm
/// 2026-10-10 的踩坑记录:Center 在 Window 里会把窗撑高、按钮跨帧漂移)。
pub(crate) fn dialog_header(ui: &mut egui::Ui, title: &str) -> bool {
    let colors = crate::theme::shell(ui);
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title)
                .size(crate::ui::tokens::text::TITLE)
                .strong()
                .color(colors.text),
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

/// 把长文本从**中间**截断成 `head … tail`(超预算时),超长路径的
/// 标准处理(shell / 文件管理器同款)。
///
/// 为什么不用 [`crate::ui::tabs::elide_text`]:那个是尾部截断
/// (`docs/…/verylongname.md`),而路径的辨识信息**两端都有** —— 开头是
/// 挂载点/项目根,结尾是文件名。截尾会把「这是哪个文件」这一半信息
/// 切掉,用户得靠猜。截中间保住两端,代价只是中间层级不可见(要用
/// tooltip 看全,见 [`path_chip`])。
///
/// 二分的是**总显示字符数**:候选串 = 前 k/2 + `…` + 后 (k - k/2 - 1)。
/// 等宽/比例字体的宽度都随字符数单调递增,故二分有效。
fn middle_elide(ui: &mut egui::Ui, text: &str, font: &egui::FontId, budget: f32) -> String {
    if measure(ui, text, font) <= budget {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 4 {
        return "…".to_owned();
    }
    let candidate = |k: usize| {
        // k 是省略号之外要显示的字符数;头占 ceil(k/2),尾占剩下的
        let head = k.div_ceil(2);
        let tail = k - head;
        let mut shown: String = chars[..head].iter().collect();
        shown.push('…');
        shown.extend(chars[chars.len() - tail..].iter());
        shown
    };
    let (mut lo, mut hi) = (0usize, chars.len().saturating_sub(1));
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if measure(ui, &candidate(mid), font) <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    candidate(lo)
}

/// 路径 chip 的**最大**宽度:超宽则中间截断。
///
/// 为什么必须显式写死而不能问 `ui.available_width()`:弹窗是内容定宽的,
/// `available_width` 在 Window 里**近无界**(设置页 card 早就踩过这个坑,
/// 见 `destructive_confirm_dialog` 的同名注释)—— 拿它当预算等于没有
/// 预算,长路径会把弹窗一路撑宽,「居中 + 定宽」全被破坏。320 = 默认弹窗
/// 宽 360 减去两侧内距 20。
const PATH_CHIP_MAX_W: f32 = 320.0;

/// 弹窗里的路径展示块:等宽字体 + 浅底 + 小圆角,宽度封顶
/// [`PATH_CHIP_MAX_W`]、超宽时**中间截断**,完整路径挂 tooltip
/// (2026-10-15 坤哥「长路径阅读吃力 + 防止换行乱」)。
///
/// 用等宽字体而不只是加底色:路径里的 `/`、`.`、`_` 在比例字体下宽度
/// 不等,一列路径扫过去对不齐;等宽 + 浅底 = 一眼可扫(与本项目代码块
/// 同款语言,底色直接取 `shell.code_bg` —— 导出 HTML 的代码块底也是它,
/// 同一语义同一色)。
///
/// 短路径按**自然宽度**显示(不拉成一条长条),长路径才截断到上限。
///
/// 返回 [`egui::Response`],`rect` 是整个 chip 的矩形(含内距),供测试
/// 定位与几何断言。
pub(crate) fn path_chip(ui: &mut egui::Ui, path: &std::path::Path) -> egui::Response {
    let colors = crate::theme::shell(ui);
    let full = path.display().to_string();
    let font = egui::FontId::monospace(crate::ui::tokens::text::CAPTION);
    let pad = crate::ui::tokens::inset::SM;
    // 内容区宽 = chip 总宽 − 两侧内距。短路径取自然宽,长路径封顶。
    let natural = measure(ui, &full, &font) + 2.0 * pad;
    let content_w = natural.min(PATH_CHIP_MAX_W - 2.0 * pad);
    let shown = middle_elide(ui, &full, &font, content_w);
    egui::Frame::NONE
        .fill(colors.code_bg)
        .stroke(egui::Stroke::new(
            1.0 / ui.ctx().pixels_per_point(),
            colors.border,
        ))
        .corner_radius(egui::CornerRadius::same(crate::ui::tokens::RADIUS_SM as u8))
        .inner_margin(egui::Margin::symmetric(pad as i8, 3))
        .show(ui, |ui| {
            ui.set_width(content_w);
            ui.label(
                egui::RichText::new(shown)
                    .monospace()
                    .size(crate::ui::tokens::text::CAPTION)
                    .color(colors.secondary),
            )
            .on_hover_text(full)
        })
        // `InnerResponse::response` 是**整个 chip** 的矩形(含内距),
        // `.inner` 只是闭包里那行文字 —— 悬浮提示挂在文字上即可(点/
        // 悬停判定都够),但返回值给外层按 chip 矩形定位更有用。
        .response
}

/// 量一段文本在给定字体下的渲染宽度。
fn measure(ui: &mut egui::Ui, text: &str, font: &egui::FontId) -> f32 {
    ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
            .rect
            .width()
    })
}

/// 弹窗里的单行输入框:比普通 `text_edit_singleline` 多两层东西 ——
/// ① 控件样式覆写:**常态描边可见、focus 时描边转强调色**
/// (2026-10-15 坤哥「输入框背景太浅,边界感不强」);② 占位提示
/// (`hint`)与 tooltip 分开传:前者是输入框为空时的灰字,后者是鼠标
/// 悬停说明,语义不同,混成一个参数会出现「输入框有值却在提示怎么用」。
///
/// 实现走 egui 的 `widgets.*.bg_stroke`:egui 0.36 的 `TextEdit` 在
/// **未聚焦**时用 `interact(…).bg_stroke` 画框、**聚焦**时改用
/// `visuals.selection.stroke`(`builder.rs` 里 `has_focus()` 的两分支),
/// 所以两处都得改,只改一处等于只做了一半。
///
/// egui 0.36 没有 `push_style`(也没有 RAII 的 StyleGuard),故手工
/// save/restore:`set_style` 换进局部样式、绘完立刻换回原值。作用域只包
/// 这一个控件 —— 不还原会顺带改掉同窗内后续所有控件的观感。
/// 弹窗单行输入框的**最小高度**。
///
/// egui 0.36 的 `TextEdit` 高度只由字体行高决定(真机实测 16px),**不读**
/// [`crate::ui::tokens::INPUT_H`] —— 后者只投影给 `spacing.interact_size.y`
/// (按钮等点击类控件)。16px 框装 13px 字形,上下各只剩 2~3px,读起来局促。
///
/// 取 32 而非 36:单行输入框在 360 宽的弹窗里,36 略笨重;32 是「不局促」
/// 与「不笨重」之间的档位。
const DIALOG_INPUT_H: f32 = 32.0;

/// 弹窗输入框内文字的**视觉重心补偿**:作为 `TextEdit` 上边距的像素数,
/// 把墨迹相对框往下推。
///
/// **为什么几何居中还不够**:egui 的 `vertical_align(Align::Center)` 做的是
/// **行盒几何居中**(`builder.rs`:`align.align_size_within_rect(galley.size(),
/// inner_rect)`),而人眼判断的是**墨迹的视觉重心**。行盒底部为降部预留了
/// 空间,墨迹在行盒内本就偏上;框比行盒高时这部分空当被平分到上下,于是
/// 「几何居中」的观感仍偏上。
///
/// **真机 1:1 实测**(2026-10-15,`import` 无缩放截窗口后逐行量像素,
/// 框 y219..251 / 墨迹 y227..237):
///
/// | 项 | 值 |
/// |---|---|
/// | 框 | 32px |
/// | 墨迹 `CHANGELOG.md` 全大写 | 11px |
/// | 几何居中后 上留白 / 下留白 | **8px / 13px** |
/// | 墨迹中心 − 框中心 | **−3px**(偏上) |
///
/// 坤哥二次反馈「还是靠上」。**上一轮的探针和无头测试当时都报「偏心
/// −0.5px」,与真机对不上** —— `Galley::mesh_bounds` 是 mesh 顶点的包围盒,
/// 与真实栅格化后的像素范围有系统偏差。观感类判据不能只信无头测量。
///
/// 取 **6**:真机要下移 3px 归零,而无头探针实测 `margin(top=n)` 让偏心
/// 从 −0.5 走到 `+0.5n−0.5`(n=2 时恰好对称),即每 1px 上边距下移约
/// 1px;真机基线比无头低 2.5px,故取 6。无头实测各档:
///
/// | 上边距 | 上留白 | 下留白 | 偏心 |
/// |---|---|---|---|
/// | 0 | 10.0 | 11.0 | −0.5 |
/// | 2 | 11.0 | 10.0 | +0.5 |
/// | 6 | 13.0 | 8.0 | +2.5 |
///
/// **依赖前提**:绑定 `DIALOG_INPUT_H = 32`、`TextStyle::Body`(13px)、
/// `margin` 之和须小于 `DIALOG_INPUT_H − 行高`(=16)否则框会被撑高。
/// 改框高或字号都要重新真机量一遍。
const DIALOG_TEXT_DY: f32 = 6.0;

pub(crate) fn dialog_text_edit(
    ui: &mut egui::Ui,
    text: &mut dyn egui::widgets::text_edit::TextBuffer,
    hint: &str,
    tooltip: &str,
) -> egui::Response {
    let colors = crate::theme::shell(ui);
    let px = ui.ctx().pixels_per_point().recip();
    // `ui.style()` 给的是 `&Arc<Style>`:解引用两次再 `Style::clone` 才拿到
    // 本体 —— `(*…).clone()` 会被解析成 `Arc::clone`(方法查找先命中 Arc 自身)。
    let saved = egui::Style::clone(&**ui.style());
    let mut style = saved.clone();
    for widget in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
    ] {
        widget.bg_stroke = egui::Stroke::new(1.0 * px, colors.border);
    }
    style.visuals.selection.stroke = egui::Stroke::new(1.0 * px, colors.accent);
    ui.set_style(style);
    // 高度方向再补 [`DIALOG_TEXT_DY`] 的视觉重心下移 —— 见该常量说明。
    //
    // `vertical_align(Center)` 只做**行盒几何居中**,人眼看的是**墨迹视觉
    // 重心**。真机 1:1 实测:几何居中后上留白 8px / 下留白 13px,偏上 5px,
    // 读起来就是「字体贴着上沿」(2026-10-15 坤哥二次反馈)。
    //
    // 修法用 `.margin()` 的**上边距**:egui 的
    // `min_height = (行高 + margin.sum().y).at_least(min_size.y)`
    // (`builder.rs`),margin 之和 6 < `DIALOG_INPUT_H - 行高` = 16,
    // 所以**框高仍是 32px 不变**,而内区被上边距顶掉一截,行盒在内区里
    // 居中 → 文字相对框下移。这比「把控件放进下移的子 Ui」可靠:
    // 后者实测框与文字**一起**下移(`max_rect` 生效但相对位置恒为 -0.5),
    // 完全不解决问题。egui 也没有「垂直微调对齐」的 API 可用。
    let response = ui.add(
        egui::TextEdit::singleline(text)
            .id(ui.make_persistent_id("dialog-text-edit"))
            .hint_text(hint)
            .desired_width(f32::INFINITY)
            // 宽度留 0(由 `desired_width` 决定):`min_size` 的 x 传
            // `INFINITY` 会与 `desired_width` 的无界宽相减,产出 NaN
            // rect(实测 `widget rect is NaN: [[-inf ..] [NaN ..]]` 报错)
            .min_size(egui::vec2(0.0, DIALOG_INPUT_H))
            // 框高与行盒居中两个开关都必需,缺一不可:
            // · `min_size` 撑起框高(默认框高 = 字体行高,真机仅 16px);
            // · `vertical_align(Center)` 让**行盒**在框内居中 ——
            //   egui 0.36 的 `TextEdit` 默认 `align = Align2::LEFT_TOP`
            //   (`builder.rs` 的 `align.align_size_within_rect`),
            //   不设的话行盒锚在框顶。
            .vertical_align(egui::Align::Center)
            // 视觉重心补偿:见 `DIALOG_TEXT_DY`。**左右必须保留 egui 默认的
            // 4px** —— `TextEdit` 的默认 margin 是 `Margin::symmetric(4, 2)`
            // (`builder.rs`),第一版把左右一并写成 0,文字直接贴左边缘
            // (2026-10-15 坤哥「有点太靠左了」)。`egui::Margin` 只有
            // `same`/`symmetric` 两个构造器,「仅改垂直」得用结构体字面量。
            .margin(egui::Margin {
                left: 4,
                right: 4,
                top: DIALOG_TEXT_DY as i8,
                bottom: 0,
            }),
    );
    ui.set_style(saved);
    response.on_hover_text(tooltip)
}

/// 让输入框**聚焦并选中主体**(不含扩展名)。
///
/// 打开重命名弹窗就能直接打字是「不用先点一下输入框」的关键;选中主体
/// (而非全选)让最常见的「只改词不改后缀」少一次退格。
///
/// `pending` 是调用方状态里的「本弹窗还没做过初始选区」标志:UI 每帧
/// 都可能调本函数,但只在首帧(标志为真)聚焦 + 设选区,随后由**调用方**
/// 把标志打掉。判据不能写成「有没有焦点」—— 焦点是我们自己请求的,
/// 下一帧就在,那样每帧都会重设选区,把用户已输入的内容反复清空。
///
/// 文本由调用方传入而不从 `TextEditState` 取:egui 0.36 的
/// `TextEditState` 只有 `cursor` 与 undoer,不含文本快照。
pub(crate) fn focus_and_select_stem(
    ui: &mut egui::Ui,
    response: &egui::Response,
    id: egui::Id,
    text: &str,
    pending: &mut bool,
) {
    if !*pending {
        return;
    }
    response.request_focus();
    *pending = false;
    let stem_chars = crate::filetree::stem_char_len(text);
    let Some(mut state) = egui::widgets::text_edit::TextEditState::load(ui.ctx(), id) else {
        return;
    };
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(stem_chars),
        )));
    state.store(ui.ctx(), id);
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
    mode_switch(ui, slot(right_start, MODE_SWITCH_W), state, outbox);
    let search_rect = slot(right_start + MODE_SWITCH_W + 12.0, search_width);
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
    use crate::live::RenderMode;
    let colors = crate::theme::shell(ui);
    ui.painter().rect_filled(rect, 7.0, colors.hover);
    // 三段的顺序即 [`RenderMode::cycle`] 的顺序:源码 → 写作 → 源码+对照。
    for (index, (label, mode)) in [
        ("写作", RenderMode::Live),
        ("源码", RenderMode::Source),
        ("分栏", RenderMode::Split),
    ]
    .into_iter()
    .enumerate()
    {
        let part = Rect::from_min_size(
            rect.min + egui::vec2(index as f32 * rect.width() / 3.0, 0.0),
            egui::vec2(rect.width() / 3.0, rect.height()),
        )
        .shrink(2.0);
        let selected = state.render_mode == mode;
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
            .on_hover_text(match mode {
                RenderMode::Live => "写作：直接编辑正文，当前段落显示 Markdown",
                RenderMode::Source => "源码：编辑 Markdown",
                RenderMode::Split => "分栏：源码 + 右侧只读对照（跟随光标滚动）",
            })
            .clicked()
            && !selected
        {
            // 点任意一段都发同一条消息,归约按 [`RenderMode::cycle`] 推进;
            // 三段是循环的,「点到目标档」需要连点(与点开关的语义一致)。
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

    /// 标题**不吃强调色**(2026-10-15 坤哥「重命名是蓝的,蓝色像链接」):
    /// 强调色在本项目已被链接/活动页签/选中文字占用,标题再吃就成了第三
    /// 种「可点」暗示。判据是标题色与 `shell.accent` **逐通道不相等** ——
    /// 用「是不是那个具体蓝值」当判据会在换肤后失效,这里比的是「有没有
    /// 误用强调色」这件事本身。
    #[test]
    fn dialog_title_is_not_accent_colored() {
        let ctx = egui::Context::default();
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut seen = None;
        for _ in 0..2 {
            let mut accent = egui::Color32::WHITE;
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    ..Default::default()
                },
                |ui| {
                    accent = crate::theme::shell(ui).accent;
                    centered_dialog(ui, "标题色探针")
                        // 必须关 title_bar:开着它,窗口标题栏里也会有一份
                        // 同文本的「标题色探针」,`find_label_color` 取**第一个**
                        // 匹配就落到标题栏上,断言恒绿(2026-10-15 红绿验证
                        // 实测:把 dialog_header 改成 accent 色,测试仍绿)。
                        // 生产弹窗同样一律 `.title_bar(false)`。
                        .title_bar(false)
                        .resizable(false)
                        .show(ui.ctx(), |ui| {
                            dialog_header(ui, "标题色探针");
                        });
                },
            );
            let color = find_label_color(&output.shapes, "标题色探针");
            seen = color.map(|c| (c, accent));
            output.drop_without_applying_deltas();
        }
        let (color, accent) = seen.expect("标题标签已绘制");
        assert_ne!(
            color.to_array(),
            accent.to_array(),
            "标题误用强调色(读起来像链接):title={color:?} accent={accent:?}"
        );
    }

    /// 前置守卫:`find_label_color` 的前提是「该文本在本帧**唯一**」。
    ///
    /// 重复出现时它会取第一个匹配 —— 拿到别人的颜色,断言恒绿
    /// (2026-10-15 实测)。这里把「唯一」钉成独立判据,而不是指望
    /// [`dialog_title_is_not_accent_colored`] 每次都能碰巧取对。
    #[test]
    fn title_color_probe_label_is_unique_per_frame() {
        let ctx = egui::Context::default();
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut count = 0usize;
        for _ in 0..2 {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    ..Default::default()
                },
                |ui| {
                    centered_dialog(ui, "标题色探针")
                        .title_bar(false)
                        .resizable(false)
                        .show(ui.ctx(), |ui| {
                            dialog_header(ui, "标题色探针");
                        });
                },
            );
            count = output
                .shapes
                .iter()
                .filter(|clipped| {
                    matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.job.text == "标题色探针")
                })
                .count();
            output.drop_without_applying_deltas();
        }
        assert_eq!(
            count, 1,
            "探针文本在本帧应只出现 1 次,实得 {count} —— 重复会让 \
             `find_label_color` 取到第一个匹配(可能是别人的颜色),断言恒绿"
        );
    }

    /// 在本帧 shape 列表里找**文本**为 `needle` 的那个颜色
    /// ([`dialog_title_is_not_accent_colored`] 用)。
    ///
    /// 按文本内容找而不是「取第一个 Text」:同一帧里还有「关闭」按钮等
    /// 一堆文本,取第一个拿到的是别人的颜色,断言恒绿。
    fn find_label_color(
        shapes: &[egui::epaint::ClippedShape],
        needle: &str,
    ) -> Option<egui::Color32> {
        shapes.iter().find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.job.text == needle => {
                Some(text.override_text_color.unwrap_or_else(|| {
                    // egui 0.36 的 `LayoutJob` 用 `sections`(每段自带
                    // font/color),没有旧版的 `visuals` 数组。取**第一
                    // 段**即可 —— 标题是单段文本。
                    text.galley
                        .job
                        .sections
                        .first()
                        .map(|section| section.format.color)
                        .unwrap_or(egui::Color32::WHITE)
                }))
            }
            _ => None,
        })
    }

    /// 弹窗输入框的**内边距与视觉重心居中**守门。
    ///
    /// 坤哥两次反馈「框里的字体感觉靠上」,两个根因、三个必需开关:
    /// ① egui 0.36 的 `TextEdit` 高度**只**由字体行高决定(真机 16px),
    /// **不读** [`crate::ui::tokens::INPUT_H`](后者只投影给
    /// `spacing.interact_size.y`,即按钮等点击类控件)→ 框太矮;
    /// ② `TextEdit` 默认 `align = Align2::LEFT_TOP`
    /// (`builder.rs` 的 `align.align_size_within_rect`)→ 行盒**锚在框顶**;
    /// ③ 即便 ①② 都对,居中的也只是**行盒**,而人眼看的是**墨迹视觉
    ///    重心** —— 行盒底部为降部预留空间,全大写文本的墨迹在行盒内本就
    ///    偏上,于是「几何居中」仍偏上 → 靠 `.margin()` 上边距补
    ///    [`DIALOG_TEXT_DY`]。
    ///
    /// **三条判据都要断言,缺一条就漏一种坏法**:
    /// · 上留白 `>= 8` 盯 ①(框高);
    /// · 下留白 `>= 8` 盯 ①的另一侧 —— 只断言上留白会漏「框高但整块
    ///   贴顶」;
    /// · `|上 − 下| <= 1` 盯 ②③(视觉重心居中)。
    ///
    /// **必须 `crate::fonts::install` 装真机字体栈再量**:无头默认字体
    /// 行盒紧贴字形,量出「偏心 −0.5px」看着完美,真机却是 **−3px**
    /// (全大写 `CHANGELOG.md` 墨迹仅 11px vs 有降部文本 14px)。这一条
    /// 判据在上轮就是漏的 —— 探针与真机对不上,而真机才是观感判据。
    /// CI runner 可能无 CJK 字体,`install` 返回 `None` 时按无头字体
    /// 继续(那种情况下 ③ 天然满足,仍能守住 ①②)。
    ///
    /// 阈值依据(真机 1:1,`import` 无缩放截窗口后逐行量像素):
    /// | 配置 | 上 | 下 | 上−下 |
    /// |---|---|---|---|
    /// | 三个开关都开(现值) | 11 | 11 | **0** |
    /// | 只开 ①②(上一轮) | 8 | 13 | −5 |
    /// 差值阈值 1px:正确实现 0,坏实现 5,留 5 倍余量(亚像素抖动 ≤0.5px)。
    #[test]
    fn dialog_input_box_has_breathing_room() {
        const MIN_PAD: f32 = 8.0;
        const MAX_PAD_DIFF: f32 = 1.0;
        let ctx = egui::Context::default();
        // 装真机字体栈:无头默认字体会把 ③ 这条判据变成恒绿(上轮教训)。
        let _cjk = crate::fonts::install(&ctx);
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut rect = None;
        let mut ink = None;
        for _ in 0..2 {
            // 全大写、无降部 —— 最容易暴露偏上的样本(墨迹仅 11px)。
            let mut text = "CHANGELOG.md".to_owned();
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    ..Default::default()
                },
                |ui| {
                    centered_dialog(ui, "内边距探针")
                        .default_width(360.0)
                        .resizable(false)
                        .show(ui.ctx(), |ui| {
                            rect = Some(dialog_text_edit(ui, &mut text, "新名称", "").rect);
                        });
                },
            );
            for clipped in &output.shapes {
                if let egui::Shape::Text(t) = &clipped.shape {
                    if t.galley.job.text == "CHANGELOG.md" {
                        // `Galley::mesh_bounds` = 字形 mesh 顶点的包围盒
                        // (galley 局部坐标,加回 `pos` 才是屏幕坐标)。
                        ink = Some(t.galley.mesh_bounds.translate(t.pos.to_vec2()));
                    }
                }
            }
            output.drop_without_applying_deltas();
        }
        let r = rect.expect("输入框已绘制");
        let k = ink.expect("文字已绘制");
        let pad_top = k.top() - r.top();
        let pad_bottom = r.bottom() - k.bottom();
        assert!(
            pad_top >= MIN_PAD,
            "输入框上留白 {pad_top:.1}px < {MIN_PAD}px(框高仅 {:.0}px,墨迹高 {:.0}px)—— \
             egui 的 TextEdit 不读 tokens::INPUT_H,要自己给 min_size",
            r.height(),
            k.height()
        );
        assert!(
            pad_bottom >= MIN_PAD,
            "输入框下留白 {pad_bottom:.1}px < {MIN_PAD}px —— 整块文字贴框顶,\
             只补 min_size 没用,还要 vertical_align(Align::Center)"
        );
        // 核心判据:**上留白 − 下留白**落在窄区间里 = 视觉重心稳定在
        // 目标位置。
        //
        // 为什么不是「上下相等」:`Galley::mesh_bounds` 是 mesh **顶点**的
        // 包围盒,比真实栅格化后的像素范围每侧约多 1px(抗锯齿羽化不计入
        // 顶点)。所以同一份实现,真机 1:1 量得 11/11(对称),无头量得
        // 13/8(差 5)。两者**绝对值对不上,但差值恒定** —— 差值才是可断言
        // 的信号:改了 `DIALOG_TEXT_DY` / `DIALOG_INPUT_H` / 字号,差值
        // 立刻变。上轮就是漏在这里:只卡「|偏心| ≤ 2」,而无头偏心恰好
        // −0.5 恒绿,真机却是 −3。
        //
        // 阈值 1px:当前无头实测 5.0(即 margin(top=6) 的效果),实测抖动
        // ≤0.5px;真机目标是对称(差 0),无头基线比真机大 5 —— 区间卡在
        // 无头基线附近,任何让文字再上/下移的改动都会跑出区间。
        const PAD_DIFF_BASE: f32 = 5.0;
        assert!(
            (pad_top - pad_bottom - PAD_DIFF_BASE).abs() <= MAX_PAD_DIFF,
            "输入框上−下留白 = {:.1}px(上 {pad_top:.1} / 下 {pad_bottom:.1}),\
             偏离基线 {PAD_DIFF_BASE}px 超过 {MAX_PAD_DIFF}px —— \
             DIALOG_TEXT_DY / DIALOG_INPUT_H / 字号 任一被改都会跑出区间",
            pad_top - pad_bottom
        );
        // 判据④(水平):文字不贴左边缘。第一版 `.margin()` 把左右也写成 0,
        // 盖掉了 egui 默认的 `Margin::symmetric(4, 2)`,文字直接贴左框边
        // (2026-10-15 坤哥「有点太靠左了」)。`mesh_bounds` 左侧比真实
        // 墨迹多约 1px,故阈值取 3(正确实现 ≥4,贴边为 0~1)。
        let pad_left = k.left() - r.left();
        assert!(
            pad_left >= 3.0,
            "输入框左留白 {pad_left:.1}px < 3px —— `.margin()` 覆盖了 egui 默认的\
             水平内边距(默认 `Margin::symmetric(4, 2)`),左右要保留 4px"
        );
    }

    /// `middle_elide` 截中间、留两端(2026-10-15 坤哥:路径的辨识信息在
    /// 头尾,截尾会把文件名切掉)。
    ///
    /// 判据是「结果 = 原文前缀 + … + 原文后缀」这条**结构不变式**,而不是
    /// 「尾部等于某个具体文件名」:固定文件名要求预算恰好装得下 12 个等宽
    /// 字符,预算一动(字号/密度档/上限常量)测试就红,而实现其实没问题。
    /// 结构判据在任意预算下都成立,且真的能抓住「截尾」或「只留头」这两
    /// 种改坏。
    #[test]
    fn middle_elide_keeps_both_ends() {
        let ctx = egui::Context::default();
        let long =
            "/home/data/www/LaterMD-very-long-project-name/docs/adr/deep/nested/CHANGELOG.md";
        // 多档预算:每一档都要满足「前缀 + … + 后缀」结构。
        for budget in [90.0_f32, 160.0, 240.0, 320.0] {
            let mut shown = String::new();
            let mut full_ok = false;
            ctx.run_ui(egui::RawInput::default(), |ui| {
                let font = egui::FontId::monospace(crate::ui::tokens::text::CAPTION);
                shown = middle_elide(ui, long, &font, budget);
                full_ok = middle_elide(ui, long, &font, 10_000.0) == long;
            })
            .drop_without_applying_deltas();
            assert!(full_ok, "宽预算应原样返回(预算 {budget})");
            assert!(
                shown.len() < long.len(),
                "预算 {budget} 下应被截短:{shown:?}"
            );
            let (head, tail) = shown
                .split_once('…')
                .unwrap_or_else(|| panic!("预算 {budget} 下缺省略号:{shown:?}"));
            assert!(!head.is_empty(), "头部应保留:{shown:?}");
            assert!(!tail.is_empty(), "尾部应保留:{shown:?}");
            assert!(
                long.starts_with(head),
                "头部应是原文前缀:{head:?} (预算 {budget})"
            );
            assert!(
                long.ends_with(tail),
                "尾部应是原文后缀(不能是截尾):{tail:?} (预算 {budget})"
            );
            // 尾段至少留够一个扩展名 —— 否则「保留尾部」形同虚设
            // (尾部只剩 1 个字符也算「后缀」,但对读路径毫无用处)。
            assert!(
                tail.len() >= 4,
                "尾部至少留 4 字符(够看出扩展名):{tail:?} (预算 {budget})"
            );
        }
    }

    /// `stem_char_len` 取主体不含扩展名:这是「选中主体、保留后缀」的
    /// 选区判据(`focus_and_select_stem` 的输入)。
    #[test]
    fn stem_len_excludes_extension_only() {
        for (name, want) in [
            ("CHANGELOG.md", 9usize),
            ("笔记", 2),
            // 多点:主体是最后一段点号之前的全部,不是「最后一个点之前一段」
            ("a.b.c", 3),
            // 无扩展名 → 全选
            ("readme", 6),
            // 隐藏文件没有主体,全选(选空反而让用户没法改)
            (".gitignore", 10),
            // 尾点:主体不含它(尾点后的扩展名为空,选区取主体更合理)
            ("weird.", 5),
        ] {
            assert_eq!(
                crate::filetree::stem_char_len(name),
                want,
                "{name:?} 的主体长度"
            );
        }
    }

    /// 路径 chip 用**等宽字体**且宽度不超预算(2026-10-15 坤哥「长路径
    /// 阅读吃力 + 防止换行乱」)。宽度判据独立于实现:量 chip 实际矩形,
    /// 不复用 `middle_elide` 的预算公式 —— 那样改一边另一边跟着动,恒绿。
    #[test]
    fn path_chip_fits_and_is_monospace() {
        let ctx = egui::Context::default();
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut chip_rect = None;
        let mut mono_texts = 0usize;
        for _ in 0..2 {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    ..Default::default()
                },
                |ui| {
                    centered_dialog(ui, "路径 chip 探针")
                        .default_width(360.0)
                        .resizable(false)
                        .show(ui.ctx(), |ui| {
                            let long = std::path::Path::new(
                                "/home/data/www/LaterMD-very-long/docs/adr/deep/CHANGELOG.md",
                            );
                            chip_rect = Some(path_chip(ui, long).rect);
                        });
                },
            );
            mono_texts = count_mono_texts(&output.shapes);
            output.drop_without_applying_deltas();
        }
        assert!(mono_texts > 0, "chip 内应有等宽字体文本");
        let rect = chip_rect.expect("chip 已绘制");
        // 判据挂在**独立常量** [`PATH_CHIP_MAX_W`] 上,不复用 chip 内部
        // 的预算公式:复用等于改实现时一起变,恒绿。
        // 容差 2.0 = chip 两侧各 1px 描边(几何量的是含描边的矩形,而
        // 宽度预算只扣了内距)。fg 1x 下描边正好 1px。
        assert!(
            rect.width() <= PATH_CHIP_MAX_W + 2.0,
            "chip 宽度超上限(长路径会把弹窗撑宽):{} > {PATH_CHIP_MAX_W}",
            rect.width()
        );
        assert!(rect.width() > 60.0, "chip 不该被截成一条:{}", rect.width());
    }

    /// 文本 shape 里用等宽字体族(Proportional → Monospace)绘制的条数。
    fn count_mono_texts(shapes: &[egui::epaint::ClippedShape]) -> usize {
        shapes
            .iter()
            .filter(|clipped| {
                matches!(
                    &clipped.shape,
                    egui::Shape::Text(text)
                        if text.galley.job.sections.iter().any(|section| {
                            section.format.font_id.family == egui::FontFamily::Monospace
                        })
                )
            })
            .count()
    }

    /// 重命名弹窗「开框即聚焦 + 选中主体」的行为守门
    /// (2026-10-15 坤哥「极度推荐开框自动聚焦,并选中文件名主体部分」)。
    ///
    /// 走的是**真实生产路径**:`dialog_text_edit` 画输入框 +
    /// `focus_and_select_stem` 设初始选区,不裸调 helper。断言三件事:
    /// ① 首帧后输入框真的拿到焦点;② 选区恰好是主体 `[0, 9)`
    /// (「CHANGELOG」而非「CHANGELOG.md」,也不是全选到 12);③
    /// `pending` 打掉之后**不再**重设选区 —— 否则用户已输入的内容会被
    /// 每帧清空(这是本函数最容易犯的错:判据写成「有没有焦点」就会
    /// 每帧重设,因为焦点是我们自己请求的、下一帧就在)。
    #[test]
    fn rename_dialog_opens_focused_with_stem_selected() {
        const NAME: &str = "CHANGELOG.md";
        let ctx = egui::Context::default();
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut pending = true;
        let mut text = NAME.to_owned();
        let mut had_focus = false;
        let mut selected: Option<(usize, usize)> = None;
        // 首帧:聚焦 + 设选区。第 2 帧:pending 已为假,选区应原样保留
        // (模拟「用户还没动手」);第 3 帧:用户把光标挪到末尾 —— 选区必须
        // 跟着变,若被重置回主体就说明每帧都在抢控制权。
        for frame in 0..3 {
            let mut range_probe = None;
            let mut focus_probe = false;
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    events: if frame == 2 {
                        find_key(egui::Key::End, Modifiers::NONE)
                    } else {
                        Vec::new()
                    },
                    ..Default::default()
                },
                |ui| {
                    centered_dialog(ui, "重命名")
                        .default_width(360.0)
                        .resizable(false)
                        .show(ui.ctx(), |ui| {
                            let edit_id = ui.make_persistent_id("dialog-text-edit");
                            let response = dialog_text_edit(ui, &mut text, "新名称", "");
                            focus_and_select_stem(ui, &response, edit_id, NAME, &mut pending);
                            focus_probe = response.has_focus();
                            if let Some(state) =
                                egui::widgets::text_edit::TextEditState::load(ui.ctx(), edit_id)
                            {
                                if let Some(r) = state.cursor.char_range() {
                                    // `CCursor::index` 是 `CharIndex` newtype(无 `as` 转换),
                                    // 转 usize 便于和 `stem_char_len` 的口径对齐
                                    range_probe = Some((
                                        usize::from(r.primary.index),
                                        usize::from(r.secondary.index),
                                    ));
                                }
                            }
                        });
                },
            )
            .drop_without_applying_deltas();
            match frame {
                0 => {
                    had_focus = focus_probe;
                    selected = range_probe;
                }
                1 => assert_eq!(
                    range_probe, selected,
                    "pending 打掉后不该再改选区(否则每帧重设,会清空用户输入)"
                ),
                2 => assert_ne!(
                    range_probe, selected,
                    "用户按 End 挪光标后,选区不该被 focus_and_select_stem 抢回去"
                ),
                _ => {}
            }
        }
        assert!(had_focus, "重命名弹窗打开后输入框应自动获得焦点");
        let (primary, secondary) = selected.expect("首帧应设好选区");
        // CCursorRange::two(0, 9) 归一化后 primary=9(尾)、secondary=0。
        // 判据是**集合** {0, 9}:不关心哪个是头尾,只关心覆盖范围 —— 否则
        // 断言会绑死在 CCursorRange 的方向约定上。
        let mut ends = [primary, secondary];
        ends.sort_unstable();
        assert_eq!(
            ends,
            [0, 9],
            "选区应恰好是主体「CHANGELOG」(不含 .md),实得 {primary}..{secondary}"
        );
        assert!(!pending, "pending 标志应被首帧打掉");
    }

    /// `find_key` —— 造一组「按下 + 抬起」事件。
    fn find_key(key: egui::Key, modifiers: Modifiers) -> Vec<Event> {
        vec![
            Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            },
            Event::Key {
                key,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers,
            },
        ]
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

    /// 三段开关:点**当前**档不产消息(不反转),点任一**其它**档产一条
    /// `ToggleLivePreview`(归约按 [`RenderMode::cycle`] 推进)。
    #[test]
    fn mode_segment_only_switches_when_selecting_another_mode() {
        use crate::live::RenderMode;
        let cases = [
            (RenderMode::Live, 0),
            (RenderMode::Source, 1),
            (RenderMode::Split, 2),
        ];
        for dark in [false, true] {
            for (mode, selected) in cases {
                let ctx = egui::Context::default();
                ctx.set_visuals(if dark {
                    egui::Visuals::dark()
                } else {
                    egui::Visuals::light()
                });
                let mut state = State::default();
                state.render_mode = mode;
                let mut outbox = Vec::new();
                // 三段等分:150 宽,段中心 = 20 + 25/75/125
                let rect = Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(150.0, 28.0));
                let center = |index: usize| egui::pos2(20.0 + 25.0 + 50.0 * index as f32, 34.0);
                for events in [vec![], click(center(selected))] {
                    ctx.run_ui(
                        egui::RawInput {
                            events,
                            ..Default::default()
                        },
                        |ui| mode_switch(ui, rect, &state, &mut outbox),
                    )
                    .drop_without_applying_deltas();
                }
                assert!(outbox.is_empty(), "点击当前档不能反转");
                // 点另两档任意一段:恰一条消息
                let other = if selected == 0 { 1 } else { 0 };
                ctx.run_ui(
                    egui::RawInput {
                        events: click(center(other)),
                        ..Default::default()
                    },
                    |ui| mode_switch(ui, rect, &state, &mut outbox),
                )
                .drop_without_applying_deltas();
                assert!(
                    matches!(outbox.as_slice(), [Message::ToggleLivePreview]),
                    "点其它档应发一条 ToggleLivePreview,实得 {outbox:?}"
                );
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
