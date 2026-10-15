//! 编辑器顶部的标签条(#11「multi-tabs」,docs/auto-plan.md 规格)。
//!
//! 形态:一排 chip(文件名 + dirty 星 + 关闭 ×)。**单个未命名且干净的空
//! 标签不画条** —— 此时标签条零信息量,省一行高度;一旦有第二个标签或
//! 当前文档落盘,条即出现,关闭入口(× 与 Ctrl+W)随之可用。
//!
//! 交互:点击名字区 = 激活;点击 × = 请求关闭(脏标签由归约侧弹确认模态,
//! 见 `State::request_close_tab`);右键 = 批量操作菜单(#37:关闭左侧/
//! 右侧/全部/其他,以**被右键的标签**为基准;「重命名」是**显示别名**——
//! 只改本条 chip 的显示文本,不改盘上文件名与保存路径;「缩短标题/
//! 完整标题」是整条标签条的宽度模式,同样纯显示层)。悬停任何 chip 给出
//! 完整标题 + 文件路径 tooltip(缩短模式下被省略号吃掉的部分在这里看全)。
//! chip 自绘(与工具栏图标按钮同一套手法),选中态用填充底色 —— 与侧
//! 边栏页签的下划线区分层级。

use crate::state::Message;
use crate::tabs::{TabRename, TabsState};
use crate::theme::TitleWidthMode;
use crate::ui::tokens::{SPACE_SM, SPACE_XS};
use eframe::egui::{self, Sense};

/// chip 高度(与格式条/工作台头部同一 28pt 点击档;2026-10-10 mac 精修
/// (#166/#169)全平台化,原非 mac 24pt 退役)。
const CHIP_H: f32 = 28.0;
/// 关闭 × 的方框边长。
const CLOSE: f32 = 18.0;
/// chip 里文字之外的固定开销:左内边距、文档图标位(图标 13 加两侧余量)、
/// 文字与关闭钮的间隙、关闭钮、右内边距之和。chip 总宽减它就是文本可用宽,
/// 即省略号截断的预算。
const CHIP_CHROME: f32 = SPACE_SM + 22.0 + SPACE_XS + CLOSE + SPACE_SM;
/// 缩短模式下单个 chip 的最小宽(#37):装得下「…」+ 关闭按钮,还给
/// 一两个汉字的辨识余量。预算再紧也不收窄到它之下 —— 保不住最小宽,
/// 关闭按钮就会被挤到点不中;溢出交给既有单行水平滚动。
const CHIP_MIN_W: f32 = 56.0;

pub(crate) fn visible(tabs: &TabsState) -> bool {
    tabs.tabs.len() > 1 || tabs.tabs[0].document.path.is_some() || tabs.current().editor.is_dirty()
}

/// 绘制标签条;返回是否实际绘制(单个未命名空标签不画,测试据此断言)。
/// `mode` 是标题宽度模式(#37):Full 按完整标题测宽,Short 按可用空间
/// 收窄 chip(见 [`plan_widths`])。
pub fn ui(
    panel: &mut egui::Ui,
    tabs: &TabsState,
    mode: TitleWidthMode,
    file_tree_root: Option<&std::path::Path>,
    outbox: &mut Vec<Message>,
) -> bool {
    if !visible(tabs) {
        return false;
    }
    // 预算(标签条视口宽)必须在 ScrollArea 外取:横向滚动的内部 ui 可用
    // 宽是「想多宽有多宽」的内容宽,拿来当预算会把收窄算法喂撑死。
    let budget = panel.available_width();
    // 标签放不下时水平滚动而非换行(同 vendored 表格的横向滚动手法):
    // 换行会让标签条高度随标签数成倍增长,把编辑区顶得上下跳;单行 +
    // 滚动(垂直滚轮在仅水平可滚的 ScrollArea 里自动转为水平)高度恒定。
    // 完整模式溢出滚动;缩短模式收窄到最小宽后仍溢出也走这里兜底。
    let reveal_id = panel.make_persistent_id("tabs-reveal");
    let reveal_key = (tabs.current().id, tabs.tabs.len(), budget.to_bits());
    let reveal = panel.data_mut(|data| {
        let changed = data.get_temp::<(u64, usize, u32)>(reveal_id) != Some(reveal_key);
        data.insert_temp(reveal_id, reveal_key);
        changed
    });
    egui::ScrollArea::horizontal()
        .id_salt("tabs-bar")
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .auto_shrink([false, true])
        .show(panel, |ui| {
            let widths = plan_widths(ui, tabs, mode, budget);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (index, width) in widths.iter().enumerate() {
                    let response = chip(ui, tabs, index, mode, *width, file_tree_root, outbox);
                    if reveal && index == tabs.active {
                        response.scroll_to_me(Some(egui::Align::Center));
                    }
                }
            });
        });
    true
}

/// 顶部文档区，列表按钮不参与横向滚动。拖窗区域由外层单独预留。
pub fn header(
    parent: &mut egui::Ui,
    rect: egui::Rect,
    tabs: &TabsState,
    mode: TitleWidthMode,
    file_tree_root: Option<&std::path::Path>,
    outbox: &mut Vec<Message>,
) {
    let list_rect =
        egui::Rect::from_min_max(egui::pos2(rect.right() - CHIP_H, rect.top()), rect.max);
    let add_rect = egui::Rect::from_min_max(
        egui::pos2(rect.right() - CHIP_H * 2.0 - 2.0, rect.top()),
        egui::pos2(rect.right() - CHIP_H - 2.0, rect.bottom()),
    );
    let strip =
        egui::Rect::from_min_max(rect.min, egui::pos2(add_rect.left() - 4.0, rect.bottom()));
    let mut child = parent.new_child(
        egui::UiBuilder::new()
            .id_salt("header-document-tabs")
            .max_rect(strip)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(strip.intersect(parent.clip_rect()));
    ui(&mut child, tabs, mode, file_tree_root, outbox);

    let add_response = parent.allocate_rect(add_rect, Sense::click());
    let colors = crate::theme::shell(parent);
    if add_response.hovered() || add_response.has_focus() {
        parent.painter().rect_filled(add_rect, 5.0, colors.hover);
    }
    let center = add_rect.center();
    let stroke = egui::Stroke::new(1.4, colors.secondary);
    parent.painter().line_segment(
        [center - egui::vec2(5.0, 0.0), center + egui::vec2(5.0, 0.0)],
        stroke,
    );
    parent.painter().line_segment(
        [center - egui::vec2(0.0, 5.0), center + egui::vec2(0.0, 5.0)],
        stroke,
    );
    add_response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "新建文档"));
    if add_response.clicked() {
        outbox.push(Message::FileCommand(crate::file::FileCmd::New));
    }
    add_response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("新建文档");

    let response = parent.allocate_rect(list_rect, Sense::click());
    if response.hovered() {
        parent.painter().rect_filled(list_rect, 5.0, colors.hover);
    }
    // 自绘向下箭头，不依赖字体 glyph。
    let c = list_rect.center();
    parent.painter().add(egui::Shape::line(
        vec![
            c + egui::vec2(-4.0, -2.0),
            c + egui::vec2(0.0, 2.0),
            c + egui::vec2(4.0, -2.0),
        ],
        egui::Stroke::new(1.3, colors.secondary),
    ));
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "已打开的文档"));
    egui::Popup::menu(&response).show(|ui| {
        ui.set_min_width(220.0);
        ui.label("已打开的文档");
        ui.separator();
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                for (index, tab) in tabs.tabs.iter().enumerate() {
                    let label = elide_text(ui, &tab.display_name(), &tab_font(ui), 300.0);
                    let response = ui.selectable_label(index == tabs.active, label);
                    if response.clicked() {
                        outbox.push(Message::TabActivate(index));
                        ui.close();
                    }
                    if let Some(path) = &tab.document.path {
                        response.on_hover_text(path.display().to_string());
                    }
                }
            });
    });
    response.on_hover_text("已打开的文档");
}

/// 文本的实测显示宽(不换行;宽度只由字形决定,颜色不影响布局)。
fn text_width(ui: &mut egui::Ui, text: &str, font: &egui::FontId) -> f32 {
    ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
            .rect
            .width()
    })
}

/// 每个标签 chip 的绘制宽(#37 标题宽度模式)。Full = 完整标题实测宽
/// (`CHIP_CHROME` + 全名宽);Short = [`share_widths`] 在预算内分配。
fn plan_widths(ui: &mut egui::Ui, tabs: &TabsState, mode: TitleWidthMode, budget: f32) -> Vec<f32> {
    let font = tab_font(ui);
    let full: Vec<f32> = tabs
        .tabs
        .iter()
        .map(|tab| CHIP_CHROME + text_width(ui, &tab.display_name(), &font))
        .collect();
    match mode {
        TitleWidthMode::Full => full,
        TitleWidthMode::Short => share_widths(&full, budget),
    }
}

/// 缩短模式的宽度分配:总需要不超预算时各取完整宽(没必要缩);超了则
/// 每个 chip 先保 [`CHIP_MIN_W`],但不超过自身完整宽,余量按 max-min
/// 公平追加 —— 需要少的先满足,长标题们平分剩余空间,与浏览器标签条同
/// 观感。保底之和已超预算时维持保底宽,溢出由外层单行滚动兜底(不换行、
/// 不挤关闭按钮)。
fn share_widths(full: &[f32], budget: f32) -> Vec<f32> {
    if full.iter().sum::<f32>() <= budget {
        return full.to_vec();
    }
    let mut widths: Vec<f32> = full.iter().map(|w| w.min(CHIP_MIN_W)).collect();
    let mut remaining = budget - widths.iter().sum::<f32>();
    if remaining <= 0.0 {
        return widths;
    }
    // 追加余量(cap = 完整宽 - 已分)升序一趟:cap 小的先领满自己的 cap,
    // 否则取当前平分额 —— 标准的 max-min 公平分配。
    let mut order: Vec<usize> = (0..full.len()).collect();
    order.sort_by(|a, b| (full[*a] - widths[*a]).total_cmp(&(full[*b] - widths[*b])));
    let mut left = full.len() as f32;
    for index in order {
        let cap = full[index] - widths[index];
        if cap > 0.0 && left > 0.0 {
            let add = cap.min(remaining / left);
            widths[index] += add;
            remaining -= add;
        }
        left -= 1.0;
    }
    widths
}

fn tab_font(_ui: &egui::Ui) -> egui::FontId {
    // 12pt 文件名(#169 精修档):比正文小一号,标签条是「索引」不是正文
    let _ = _ui;
    egui::FontId::proportional(12.0)
}

/// 按实测宽截断文本并补省略号(#37 缩短模式):在 **Unicode 字符(char)
/// 边界**二分出「前缀 + …」实测宽 ≤ `budget` 的最长前缀。中文/emoji 绝
/// 不会在字节中间被切开(输出是 Rust `String`,半字符本就不可表示);
/// 全名放得下时原样返回(不加省略号)。`budget` 连一个省略号都放不下时
/// 仍返回单个省略号(`CHIP_MIN_W` 保证实际到不了这一步,纯防御)。
/// 禅定的悬停标签导航列(#57)复用同一截断(同一显示口径)。
pub(crate) fn elide_text(
    ui: &mut egui::Ui,
    text: &str,
    font: &egui::FontId,
    budget: f32,
) -> String {
    if text_width(ui, text, font) <= budget {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    // 候选前缀 + 省略号(k = 显示的字符数;至多 len-1,截断必然带省略号)
    let candidate = |k: usize| {
        let mut shown: String = chars[..k].iter().collect();
        shown.push('…');
        shown
    };
    let (mut lo, mut hi) = (0usize, chars.len().saturating_sub(1));
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if text_width(ui, &candidate(mid), font) <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    candidate(lo)
}

/// 右键菜单条目的响应组(`context_menu_items` 的返回值;生产闭包忽略,
/// 无头测试借此定位各条目矩形,与 `image_dialog::dialog` 返回按钮响应
/// 同款手法)。字段只在测试读取(与 `PreviewState::text` 同款豁免)。
#[allow(dead_code)]
pub(crate) struct TabMenuItems {
    pub close_left: egui::Response,
    pub close_right: egui::Response,
    pub close_all: egui::Response,
    pub close_others: egui::Response,
    pub rename: egui::Response,
    pub short_title: egui::Response,
    pub full_title: egui::Response,
    pub copy_path: egui::Response,
    pub copy_relative: egui::Response,
    pub reveal: egui::Response,
}

/// 单条菜单项:可用性由调用方判定(无目标/已是当前模式即禁用),禁用态
/// 给悬停说明。
fn menu_item(ui: &mut egui::Ui, label: &str, enabled: bool, disabled_hint: &str) -> egui::Response {
    let response = ui.add_enabled(enabled, egui::Button::new(label));
    if enabled {
        response
    } else {
        response.on_disabled_hover_text(disabled_hint)
    }
}

/// 右键标签的批量操作菜单(#37)。目标是**被右键的标签本身**(`index`,
/// 菜单弹出帧的快照),不是当前活动标签;关闭左侧/右侧/其他都以它为
/// 基准并保留它。无目标可关时(最左标签的「关闭左侧」等)禁用对应条目,
/// 归约侧对空队列防御性 no-op。「重命名」(显示别名)对任何标签可用——
/// 含未命名标签。「缩短标题/完整标题」是**整条标签条**的宽度模式:与被
/// 右键的标签无关(全局偏好),当前模式的那项禁用(再点无意义),切换
/// 走归约落盘 settings.json。
fn context_menu_items(
    ui: &mut egui::Ui,
    tabs: &TabsState,
    index: usize,
    mode: TitleWidthMode,
    file_tree_root: Option<&std::path::Path>,
    outbox: &mut Vec<Message>,
) -> TabMenuItems {
    let close_left = menu_item(ui, "关闭左侧", index > 0, "左侧没有标签");
    if close_left.clicked() {
        outbox.push(Message::TabBatchCloseRequested {
            kind: crate::tabs::BatchClose::Left,
            index,
        });
    }
    let close_right = menu_item(ui, "关闭右侧", index + 1 < tabs.tabs.len(), "右侧没有标签");
    if close_right.clicked() {
        outbox.push(Message::TabBatchCloseRequested {
            kind: crate::tabs::BatchClose::Right,
            index,
        });
    }
    let close_all = menu_item(ui, "关闭全部", true, "");
    if close_all.clicked() {
        outbox.push(Message::TabBatchCloseRequested {
            kind: crate::tabs::BatchClose::All,
            index,
        });
    }
    let close_others = menu_item(ui, "关闭其他", tabs.tabs.len() > 1, "没有其他标签");
    if close_others.clicked() {
        outbox.push(Message::TabBatchCloseRequested {
            kind: crate::tabs::BatchClose::Others,
            index,
        });
    }
    ui.separator();
    let rename = menu_item(ui, "重命名", true, "");
    if rename.clicked() {
        outbox.push(Message::TabRenameRequested { index });
    }
    ui.separator();
    let short_title = menu_item(
        ui,
        TitleWidthMode::Short.label(),
        mode != TitleWidthMode::Short,
        "当前已是此模式",
    );
    if short_title.clicked() {
        outbox.push(Message::TabTitleWidthChanged(TitleWidthMode::Short));
    }
    let full_title = menu_item(
        ui,
        TitleWidthMode::Full.label(),
        mode != TitleWidthMode::Full,
        "当前已是此模式",
    );
    if full_title.clicked() {
        outbox.push(Message::TabTitleWidthChanged(TitleWidthMode::Full));
    }
    ui.separator();
    // 文件级两条(context-menu-plan §3 M4):目标 = 被右键标签的落盘路径,
    // 与文件树右键共用 FileTreeCopyPath / FileTreeReveal 消息与归约实现
    // (消息名带 FileTree 是历史命名,语义就是「对某路径做 X」)。未落盘
    // 标签没有路径,禁用并说明;在文件树根之下才出现「复制相对路径」。
    let tab_path = tabs
        .tabs
        .get(index)
        .and_then(|tab| tab.document.path.clone());
    let copy_path = menu_item(ui, "复制路径", tab_path.is_some(), "该标签尚未保存到磁盘");
    if let (true, Some(path)) = (copy_path.clicked(), tab_path.clone()) {
        outbox.push(Message::FileTreeCopyPath {
            path,
            relative: false,
        });
    }
    let relative_ready = tab_path.as_deref().is_some_and(|path| {
        file_tree_root.is_some_and(|root| root != path && path.starts_with(root))
    });
    let copy_relative = menu_item(
        ui,
        "复制相对路径",
        relative_ready,
        if tab_path.is_none() {
            "该标签尚未保存到磁盘"
        } else {
            "不在文件树根目录之下"
        },
    );
    let reveal = menu_item(
        ui,
        "在文件管理器中显示",
        tab_path.is_some(),
        "该标签尚未保存到磁盘",
    );
    if let (true, Some(path)) = (reveal.clicked(), tab_path.clone()) {
        outbox.push(Message::FileTreeReveal(path));
    }
    if let (true, Some(path)) = (copy_relative.clicked(), tab_path.clone()) {
        outbox.push(Message::FileTreeCopyPath {
            path,
            relative: true,
        });
    }

    TabMenuItems {
        close_left,
        close_right,
        close_all,
        close_others,
        rename,
        short_title,
        full_title,
        copy_path,
        copy_relative,
        reveal,
    }
}

/// 单个标签 chip:名字区点击激活,× 区点击请求关闭。`width` 是本帧的
/// 绘制宽(`plan_widths` 按宽度模式分配),文本按它减 [`CHIP_CHROME`]
/// 的预算显示 —— 放不下时按字符边界加省略号(仅缩短模式会出现这种情况;
/// 完整模式的 width 恒等于完整标题实测宽)。
fn chip(
    ui: &mut egui::Ui,
    tabs: &TabsState,
    index: usize,
    mode: TitleWidthMode,
    width: f32,
    file_tree_root: Option<&std::path::Path>,
    outbox: &mut Vec<Message>,
) -> egui::Response {
    let tab = &tabs.tabs[index];
    let selected = index == tabs.active;
    let name = tab.display_name();
    let text_color = if selected {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let font = tab_font(ui);
    // 缩短模式按预算截断;完整模式 width 即完整宽,elide 必然原样返回
    // (两模式统一走这里,免得完整模式再留一条不经测宽的旁路)。
    let shown = if mode == TitleWidthMode::Short {
        elide_text(ui, &name, &font, (width - CHIP_CHROME).max(0.0))
    } else {
        name.clone()
    };
    let size = egui::vec2(width.max(CHIP_CHROME), CHIP_H);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let close_rect = egui::Rect::from_min_size(
        egui::pos2(
            rect.right() - SPACE_SM - CLOSE,
            rect.center().y - CLOSE / 2.0,
        ),
        egui::vec2(CLOSE, CLOSE),
    );
    // 两模式都给完整标题 + 路径 tooltip(#37):缩短模式被省略号截掉的
    // 部分在这里看全;未落盘的标签如实说「尚未保存」,不编路径。
    let tip = match tab.document.path.as_ref() {
        Some(path) => format!("{name}\n{}", path.display()),
        None => format!("{name}\n尚未保存到磁盘"),
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
    let response = response.on_hover_text(tip);

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        // 中性选中观感(2026-10-10 mac 精修全平台化,#166/#169):淡底 +
        // 正文中性色,蓝色只留给选中/链接/操作 —— 标签条不再叠加下划线与
        // 蓝字重复强调。未选中悬停浅灰。
        let shell = crate::theme::shell(ui);
        let hover_bg = shell.hover;
        let bg = if selected {
            shell.content
        } else if response.hovered() {
            hover_bg
        } else {
            egui::Color32::TRANSPARENT
        };
        painter.rect_filled(rect, 6.0, bg);
        crate::ui::icons::Icon::File.draw(
            painter,
            egui::pos2(rect.left() + 14.0, rect.center().y),
            13.0,
            shell.secondary,
        );
        let galley =
            painter.layout_no_wrap(shown.clone(), egui::FontId::proportional(12.0), text_color);
        painter.galley(
            egui::pos2(
                rect.left() + 26.0,
                rect.center().y - galley.mesh_bounds.center().y,
            ),
            galley,
            text_color,
        );
        // 关闭 ×:悬停该 chip 时才上色(常驻会显得噪)
        let cross = if response.hovered() {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        if response.hovered() || selected {
            crate::ui::icons::Icon::Close.draw(painter, close_rect.center(), 14.0, cross);
        } else if tab.editor.is_dirty() {
            // 未悬停时脏标记用小圆点:关闭钮位置让位给「有未保存」状态
            painter.circle_filled(close_rect.center(), 2.5, cross);
        }
    }

    if response.clicked() {
        // 命中判定用本帧指针位置:落在 × 区 = 关闭,落在名字区 = 激活
        let close_hit = response
            .interact_pointer_pos()
            .is_some_and(|pos| close_rect.expand(2.0).contains(pos));
        if close_hit {
            outbox.push(Message::TabCloseRequested(index));
        } else {
            outbox.push(Message::TabActivate(index));
        }
    }

    // 右键菜单(#37):挂在被右键的 chip 上(目标即该标签,非当前活动)。
    // `context_menu` 消费 Response,借 clone 注册、原 response 继续供上面的
    // 点击判定用(与标题栏设置键右键直达同手法)。
    response.clone().context_menu(|ui| {
        context_menu_items(ui, tabs, index, mode, file_tree_root, outbox);
    });
    response
}

/// 重命名浮窗(#37「重命名」,**显示别名**语义):单行输入 + 确定/取消。
/// 草稿由 UI 原地改(`TabRename.draft`,与 `ImageDialogState` 同款);草稿
/// trim 后为空时「确定」禁用(归约侧对绕过 UI 的消息仍防御性拒绝)。
///
/// `file_label` 是目标标签的落盘身份(文件名或「尚未保存」),原样展示在
/// 浮窗里 —— 既明示本操作**不改盘上文件**的作用范围,也让别名盖住 chip
/// 后用户仍有地方看见真实文件名。返回(确定, 取消, ×)响应供测试定位
/// (与 `image_dialog::dialog` 同款手法,× 与「取消」同语义)。
pub(crate) fn rename_dialog(
    ui: &mut egui::Ui,
    rename: &mut TabRename,
    file_label: &str,
) -> (egui::Response, egui::Response, bool) {
    let mut buttons = None;
    let mut close = false;
    egui::Window::new("重命名标签")
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ui.ctx().viewport_rect().center())
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .frame(crate::ui::workbench::dialog_frame(ui))
        .show(ui.ctx(), |ui| {
            close = crate::ui::workbench::dialog_header(ui, "重命名标签");
            ui.label("标签显示名");
            ui.text_edit_singleline(&mut rename.draft)
                .on_hover_text("只改此标签的显示名;文件名与保存路径不变");
            ui.add_space(crate::ui::tokens::SPACE_XS);
            ui.weak(format!("文件:{file_label}(重命名不改动它)"));
            crate::ui::workbench::dialog_footer_divider(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                let ready = !rename.draft.trim().is_empty();
                let confirm = ui
                    .add_enabled(ready, egui::Button::new("确定"))
                    .on_disabled_hover_text("名称不能为空;不改请点「取消」");
                ui.add_space(crate::ui::tokens::SPACE_MD);
                let cancel = ui.button("取消");
                buttons = Some((confirm, cancel));
            });
        });
    let (confirm, cancel) = buttons.expect("浮窗必然绘制确定/取消按钮");
    (confirm, cancel, close)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::TabsState;
    use egui::{Event, PointerButton, RawInput, Rect};
    use std::cell::Cell;

    /// 单个未命名空标签不画条(零信息量);落盘或变脏后条出现。
    #[test]
    fn bar_hidden_for_single_empty_tab_only() {
        let ctx = egui::Context::default();
        let mut tabs = TabsState::new("");
        let mut outbox = Vec::new();

        let output = ctx.run_ui(RawInput::default(), |ui| {
            assert!(
                !super::ui(ui, &tabs, TitleWidthMode::Full, None, &mut outbox),
                "单个未命名空标签不画条"
            );
        });
        output.drop_without_applying_deltas();

        tabs.current_mut().editor.insert_chars(0, "写了字");
        let output = ctx.run_ui(RawInput::default(), |ui| {
            assert!(
                super::ui(ui, &tabs, TitleWidthMode::Full, None, &mut outbox),
                "dirty 后条出现"
            );
        });
        output.drop_without_applying_deltas();

        // 落盘后即使不脏也显示(有关闭入口的信息量)
        tabs.current_mut().editor.clear_dirty();
        tabs.current_mut().document.path = Some(std::path::PathBuf::from("/a.md"));
        let output = ctx.run_ui(RawInput::default(), |ui| {
            assert!(super::ui(
                ui,
                &tabs,
                TitleWidthMode::Full,
                None,
                &mut outbox
            ));
        });
        output.drop_without_applying_deltas();
    }

    /// 标签多到放不下时保持单行水平滚动,不换行:同一组标签在窄/宽容器里
    /// 条高一致(换行实现的高度随标签数成倍增长,编辑区会被顶得上下跳)。
    /// 缩短模式同样不换行 —— 收窄算法把 chip 压到最小宽后,溢出交给滚动。
    #[test]
    fn bar_keeps_single_line_when_chips_overflow() {
        let ctx = egui::Context::default();
        let mut tabs = TabsState::new("");
        for i in 0..6 {
            tabs.open_tab(
                Some(std::path::PathBuf::from(format!(
                    "一个很长很长的文档标题第{i}篇.md"
                ))),
                "",
            );
        }
        let mut outbox = Vec::new();
        for mode in [TitleWidthMode::Full, TitleWidthMode::Short] {
            let heights = [220.0, 2000.0].map(|width| {
                let mut height = None;
                let output = ctx.run_ui(RawInput::default(), |ui| {
                    ui.set_max_width(width);
                    assert!(
                        super::ui(ui, &tabs, mode, None, &mut outbox),
                        "多标签必画条"
                    );
                    height = Some(ui.min_rect().height());
                });
                output.drop_without_applying_deltas();
                height.unwrap()
            });
            assert_eq!(heights[0], heights[1], "{mode:?}:窄容器不换行,条高恒定");
            // 单行高度与 chip 高度同量级(留行距与滚动条余量),远小于 6 行
            assert!(heights[1] < 2.0 * CHIP_H, "{mode:?}:条高 {}", heights[1]);
        }
    }

    /// chip 交互:点名字区发 TabActivate,点 × 区发 TabCloseRequested。
    #[test]
    fn chip_click_zones_send_different_messages() {
        let ctx = egui::Context::default();
        let tabs = TabsState::new("第一篇");
        let mut tabs = tabs;
        tabs.open_tab(None, "第二篇");
        let mut outbox = Vec::new();
        let rect = Cell::new(Rect::NOTHING);

        // 第一帧拿 chip 0 的位置(画在原始位置;宽度按完整标题实测)
        let output = ctx.run_ui(RawInput::default(), |ui| {
            let widths = plan_widths(ui, &tabs, TitleWidthMode::Full, ui.available_width());
            chip(
                ui,
                &tabs,
                0,
                TitleWidthMode::Full,
                widths[0],
                None,
                &mut outbox,
            );
            rect.set(ui.min_rect());
        });
        output.drop_without_applying_deltas();
        let rect = rect.get();
        let name_pos = egui::pos2(rect.left() + 3.0, rect.center().y);
        let close_pos = egui::pos2(rect.right() - SPACE_SM - CLOSE / 2.0, rect.center().y);
        let click = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let draw = |events: Vec<Event>, outbox: &mut Vec<Message>| {
            ctx.run_ui(
                RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    let widths = plan_widths(ui, &tabs, TitleWidthMode::Full, ui.available_width());
                    chip(ui, &tabs, 0, TitleWidthMode::Full, widths[0], None, outbox);
                },
            )
            .drop_without_applying_deltas();
        };

        // 点名字区 → TabActivate(0)
        for events in [
            vec![Event::PointerMoved(name_pos)],
            vec![click(name_pos, true)],
            vec![click(name_pos, false)],
        ] {
            draw(events, &mut outbox);
        }
        assert_eq!(outbox, vec![Message::TabActivate(0)]);
        outbox.clear();

        // 点 × 区 → TabCloseRequested(0)
        for events in [
            vec![Event::PointerMoved(close_pos)],
            vec![click(close_pos, true)],
            vec![click(close_pos, false)],
        ] {
            draw(events, &mut outbox);
        }
        assert_eq!(outbox, vec![Message::TabCloseRequested(0)]);
    }

    /// 右键菜单动作消息(#37):四个批量动作都携带**被右键标签**的索引
    /// (非当前活动标签),点击才发消息。菜单直接渲染(与 menubar::item
    /// 的测法同款):条目响应里有矩形,按中心点击。
    #[test]
    fn context_menu_actions_carry_right_clicked_tab_index() {
        let ctx = egui::Context::default();
        let mut tabs = TabsState::new("甲");
        tabs.open_tab(None, "乙");
        tabs.open_tab(None, "丙");
        tabs.activate(2); // active = 丙,右键目标是乙(索引 1,非活动)
        let cases = [
            ("左", crate::tabs::BatchClose::Left),
            ("右", crate::tabs::BatchClose::Right),
            ("全", crate::tabs::BatchClose::All),
            ("他", crate::tabs::BatchClose::Others),
        ];
        for (name, kind) in cases {
            let mut outbox = Vec::new();
            let rect = Cell::new(Rect::NOTHING);
            ctx.run_ui(RawInput::default(), |ui| {
                let items =
                    context_menu_items(ui, &tabs, 1, TitleWidthMode::Full, None, &mut outbox);
                rect.set(match kind {
                    crate::tabs::BatchClose::Left => items.close_left.rect,
                    crate::tabs::BatchClose::Right => items.close_right.rect,
                    crate::tabs::BatchClose::All => items.close_all.rect,
                    crate::tabs::BatchClose::Others => items.close_others.rect,
                });
            })
            .drop_without_applying_deltas();
            assert!(outbox.is_empty(), "{name}:仅渲染不发消息");

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
                    context_menu_items(ui, &tabs, 1, TitleWidthMode::Full, None, &mut outbox);
                },
            )
            .drop_without_applying_deltas();
            assert_eq!(
                outbox,
                vec![Message::TabBatchCloseRequested { kind, index: 1 }],
                "{name}:消息携带被右键标签的索引"
            );
        }
    }

    /// 首/尾/唯一边界(#37):无目标可关的菜单项禁用(点击也不发消息),
    /// 有目标的照常可用。顺序 = 左侧 / 右侧 / 全部 / 其他。
    #[test]
    fn context_menu_disables_items_without_targets() {
        let ctx = egui::Context::default();

        let assert_items = |tabs: &TabsState, index: usize, want: [bool; 4]| {
            let mut outbox = Vec::new();
            let rects = Cell::new(Vec::<Rect>::new());
            ctx.run_ui(RawInput::default(), |ui| {
                let items =
                    context_menu_items(ui, tabs, index, TitleWidthMode::Full, None, &mut outbox);
                assert_eq!(
                    [
                        items.close_left.enabled(),
                        items.close_right.enabled(),
                        items.close_all.enabled(),
                        items.close_others.enabled()
                    ],
                    want
                );
                rects.set(
                    [
                        items.close_left.rect,
                        items.close_right.rect,
                        items.close_all.rect,
                        items.close_others.rect,
                    ]
                    .to_vec(),
                );
            })
            .drop_without_applying_deltas();
            // 逐条点击**禁用**项:一律不发消息
            for (rect, enabled) in rects.take().into_iter().zip(want) {
                if enabled {
                    continue;
                }
                let center = rect.center();
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
                        context_menu_items(
                            ui,
                            tabs,
                            index,
                            TitleWidthMode::Full,
                            None,
                            &mut outbox,
                        );
                    },
                )
                .drop_without_applying_deltas();
            }
            assert!(outbox.is_empty(), "禁用项点击零消息,实际 {outbox:?}");
        };

        // 唯一标签:左/右/其他无目标;全部仍有目标(关空兜底补空标签)
        let single = TabsState::new("仅此一篇");
        assert_items(&single, 0, [false, false, true, false]);

        let mut tabs = TabsState::new("甲");
        tabs.open_tab(None, "乙");
        tabs.open_tab(None, "丙");
        // 最左:左侧无目标;最右:右侧无目标;中间:全有目标
        assert_items(&tabs, 0, [false, true, true, true]);
        assert_items(&tabs, 1, [true, true, true, true]);
        assert_items(&tabs, 2, [true, false, true, true]);
    }

    /// 菜单骨架(#37):「重命名」已交付(显示别名)—— 恒可用,点击发
    /// `TabRenameRequested` 且携带**被右键标签**的索引;缩短/完整标题仍由
    /// 菜单(#37):「重命名」已交付(显示别名)—— 恒可用,点击发
    /// `TabRenameRequested` 且携带**被右键标签**的索引;标题宽度模式是
    /// 全局偏好,当前模式的那项禁用(点击也不发消息),另一项的切换
    /// 动作由下方专项测试覆盖。
    #[test]
    fn context_menu_rename_clicks_request_with_right_clicked_index() {
        let ctx = egui::Context::default();
        let mut tabs = TabsState::new("甲");
        tabs.open_tab(None, "乙");
        tabs.activate(1); // active = 乙,右键目标是甲(索引 0,非活动)
        let mut outbox = Vec::new();
        let rename_rect = Cell::new(Rect::NOTHING);
        let current_mode_rect = Cell::new(Rect::NOTHING);
        ctx.run_ui(RawInput::default(), |ui| {
            let items = context_menu_items(ui, &tabs, 0, TitleWidthMode::Full, None, &mut outbox);
            assert!(items.rename.enabled(), "重命名对任何标签可用");
            assert!(items.short_title.enabled(), "完整模式下「缩短标题」可切换");
            assert!(!items.full_title.enabled(), "已是完整模式,「完整标题」禁用");
            rename_rect.set(items.rename.rect);
            current_mode_rect.set(items.full_title.rect);
        })
        .drop_without_applying_deltas();
        assert!(outbox.is_empty(), "仅渲染不发消息");

        // 当前模式项:禁用,点击零消息(再点无意义)
        let center = current_mode_rect.get().center();
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
                context_menu_items(ui, &tabs, 0, TitleWidthMode::Full, None, &mut outbox);
            },
        )
        .drop_without_applying_deltas();
        assert!(outbox.is_empty(), "当前模式项禁用,点击零消息");

        // 「重命名」点击 → TabRenameRequested { index: 被右键的 0 }
        let center = rename_rect.get().center();
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
                context_menu_items(ui, &tabs, 0, TitleWidthMode::Full, None, &mut outbox);
            },
        )
        .drop_without_applying_deltas();
        assert_eq!(
            outbox,
            vec![Message::TabRenameRequested { index: 0 }],
            "重命名点击携带被右键标签的索引"
        );
    }

    /// 标题宽度模式的菜单切换(#37):点击「缩短标题/完整标题」发携带目标
    /// 模式的 `TabTitleWidthChanged`,不带标签索引(整条标签条的全局偏好);
    ///已是目标模式的那项禁用。
    #[test]
    fn context_menu_title_width_items_switch_mode() {
        let ctx = egui::Context::default();
        let tabs = TabsState::new("甲");
        for (from, switch_to) in [
            (TitleWidthMode::Full, TitleWidthMode::Short),
            (TitleWidthMode::Short, TitleWidthMode::Full),
        ] {
            let mut outbox = Vec::new();
            let target_rect = Cell::new(Rect::NOTHING);
            ctx.run_ui(RawInput::default(), |ui| {
                let items = context_menu_items(ui, &tabs, 0, from, None, &mut outbox);
                let (target, current) = match switch_to {
                    TitleWidthMode::Short => (items.short_title, items.full_title),
                    TitleWidthMode::Full => (items.full_title, items.short_title),
                };
                assert!(target.enabled(), "{from:?}:可切到 {switch_to:?}");
                assert!(!current.enabled(), "{from:?}:当前模式项禁用");
                target_rect.set(target.rect);
            })
            .drop_without_applying_deltas();

            let center = target_rect.get().center();
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
                    context_menu_items(ui, &tabs, 0, from, None, &mut outbox);
                },
            )
            .drop_without_applying_deltas();
            assert_eq!(
                outbox,
                vec![Message::TabTitleWidthChanged(switch_to)],
                "{from:?} → 点菜单切到 {switch_to:?}"
            );
        }
    }

    /// 重命名浮窗(#37):空/纯空白草稿禁用「确定」(禁用态有说明),非空
    /// 启用;「取消」恒可点。渲染不 panic。
    #[test]
    fn rename_dialog_blank_draft_disables_confirm() {
        let ctx = egui::Context::default();
        for draft in ["", "   \t"] {
            let mut rename = TabRename {
                tab_id: 7,
                draft: draft.to_owned(),
            };
            let mut seen = None;
            for _ in 0..3 {
                let output = ctx.run_ui(RawInput::default(), |ui| {
                    let (confirm, cancel, _x) = rename_dialog(ui, &mut rename, "a.md");
                    seen = Some((confirm.enabled(), cancel.enabled()));
                });
                output.drop_without_applying_deltas();
            }
            let (confirm_enabled, cancel_enabled) = seen.expect("至少跑了一帧");
            assert!(!confirm_enabled, "空白草稿 {draft:?} 禁用确定");
            assert!(cancel_enabled, "取消恒可点");
        }
        let mut rename = TabRename {
            tab_id: 7,
            draft: "  笔记  ".to_owned(),
        };
        let mut confirm_enabled = false;
        for _ in 0..3 {
            let output = ctx.run_ui(RawInput::default(), |ui| {
                let (confirm, _, _x) = rename_dialog(ui, &mut rename, "a.md");
                confirm_enabled = confirm.enabled();
            });
            output.drop_without_applying_deltas();
        }
        assert!(confirm_enabled, "trim 后非空即可确定");
    }

    /// 右键标签弹菜单(#37 装配):secondary 点击**标签条上**的标签(chip
    /// 经 `ui` 的真实入口渲染,含 ScrollArea)后 egui 侧确有弹层打开。
    /// 菜单条目本身的行为由上面的直渲染测试覆盖。
    #[test]
    fn right_click_on_chip_opens_context_menu() {
        let ctx = egui::Context::default();
        let mut tabs = TabsState::new("甲");
        tabs.open_tab(None, "乙");
        let mut outbox = Vec::new();
        let rect = Cell::new(Rect::NOTHING);

        ctx.run_ui(RawInput::default(), |ui| {
            assert!(
                super::ui(ui, &tabs, TitleWidthMode::Full, None, &mut outbox),
                "多标签必画条"
            );
            rect.set(ui.min_rect());
        })
        .drop_without_applying_deltas();
        assert!(!egui::containers::Popup::is_any_open(&ctx), "前置:无弹层");

        // 标签条最左侧必是第一个 chip(名字区靠左):取条左端内侧一点
        let pos = egui::pos2(rect.get().left() + 2.0, rect.get().top() + CHIP_H / 2.0);
        let click = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Default::default(),
        };
        // 右键帧:secondary 点击让 context_menu 置开;下一帧空输入渲染,
        // 弹层内容(菜单条目)实际画出来。
        ctx.run_ui(
            RawInput {
                events: vec![Event::PointerMoved(pos), click(true), click(false)],
                ..Default::default()
            },
            |ui| {
                super::ui(ui, &tabs, TitleWidthMode::Full, None, &mut outbox);
            },
        )
        .drop_without_applying_deltas();
        ctx.run_ui(RawInput::default(), |ui| {
            super::ui(ui, &tabs, TitleWidthMode::Full, None, &mut outbox);
        })
        .drop_without_applying_deltas();
        assert!(
            egui::containers::Popup::is_any_open(&ctx),
            "右键后菜单弹层打开"
        );
        assert!(outbox.is_empty(), "右键本身不发消息");
    }

    /// 缩短模式量出的标签宽 < 完整模式宽度(#37 验收:宽度差断言,非仅
    /// 文本断言)。同一组长标题标签、同一窄预算:分配出的宽各低于完整宽、
    /// 不低于最小宽、总和收进预算;chip 按分配宽**实际渲染**(实测矩形
    /// 宽 = 分配值,证明缩小的是标签本体而非只有文本)。
    #[test]
    fn short_mode_plans_narrower_chips_than_full_mode() {
        let ctx = egui::Context::default();
        let mut tabs = TabsState::new("");
        for i in 0..4 {
            tabs.open_tab(
                Some(std::path::PathBuf::from(format!(
                    "一篇标题很长很长的文档第{i}号.md"
                ))),
                "",
            );
        }
        let budget = 300.0_f32;
        let full = Cell::new(Vec::<f32>::new());
        let shared = Cell::new(Vec::<f32>::new());
        ctx.run_ui(RawInput::default(), |ui| {
            full.set(plan_widths(ui, &tabs, TitleWidthMode::Full, budget));
            shared.set(plan_widths(ui, &tabs, TitleWidthMode::Short, budget));
        })
        .drop_without_applying_deltas();
        let (full, shared) = (full.take(), shared.take());

        assert!(
            full.iter().sum::<f32>() > budget,
            "前置:完整宽度溢出预算(收窄才有意义)"
        );
        for (index, (want_full, got)) in full.iter().zip(&shared).enumerate() {
            assert!(
                got < want_full,
                "chip {index}:缩短 {got} 应小于完整 {want_full}"
            );
            assert!(
                got >= &CHIP_MIN_W,
                "chip {index}:不低于最小宽 {CHIP_MIN_W},实际 {got}"
            );
        }
        assert!(
            shared.iter().sum::<f32>() <= budget,
            "缩短模式总宽收进预算 {},实际 {}",
            budget,
            shared.iter().sum::<f32>()
        );

        // chip 实测渲染宽 = 分配宽(标签本体真的变窄了)
        let rendered = Cell::new(0.0_f32);
        ctx.run_ui(RawInput::default(), |ui| {
            chip(
                ui,
                &tabs,
                0,
                TitleWidthMode::Short,
                shared[0],
                None,
                &mut Vec::new(),
            );
            rendered.set(ui.min_rect().width());
        })
        .drop_without_applying_deltas();
        assert_eq!(rendered.get(), shared[0], "chip 按分配宽实际渲染");
        // 对照:完整模式的 chip 用完整宽渲染,量出的是完整宽
        ctx.run_ui(RawInput::default(), |ui| {
            chip(
                ui,
                &tabs,
                0,
                TitleWidthMode::Full,
                full[0],
                None,
                &mut Vec::new(),
            );
            rendered.set(ui.min_rect().width());
        })
        .drop_without_applying_deltas();
        assert_eq!(rendered.get(), full[0]);
        assert!(
            shared[0] < full[0],
            "宽度差:缩短 {} < 完整 {}",
            shared[0],
            full[0]
        );
    }

    /// 预算充足时缩短模式不缩(各取完整宽,观感与完整模式一致);预算吃紧
    /// 时收窄压力按 max-min 公平落位:完整宽低于平分额的短标签拿满自然宽
    /// (cap 封顶退出分配),长标题独自承担全部挤压。
    #[test]
    fn short_mode_keeps_full_width_when_budget_affords() {
        let ctx = egui::Context::default();
        let mut tabs = TabsState::new("");
        tabs.open_tab(Some(std::path::PathBuf::from("/短.md")), "");
        tabs.open_tab(
            Some(std::path::PathBuf::from("/这是一篇标题很长的文档.md")),
            "",
        );
        let full = Cell::new(Vec::<f32>::new());
        let relaxed = Cell::new(Vec::<f32>::new());
        let tight = Cell::new(Vec::<f32>::new());
        ctx.run_ui(RawInput::default(), |ui| {
            let full_widths = plan_widths(ui, &tabs, TitleWidthMode::Full, 0.0);
            full.set(full_widths.clone());
            relaxed.set(plan_widths(
                ui,
                &tabs,
                TitleWidthMode::Short,
                full_widths.iter().sum::<f32>() + 500.0,
            ));
            // 吃紧预算:两标签的完整宽都超出平分额 → 双双收窄、平分余量
            tight.set(plan_widths(
                ui,
                &tabs,
                TitleWidthMode::Short,
                full_widths.iter().sum::<f32>() * 0.8,
            ));
        })
        .drop_without_applying_deltas();
        let (relaxed_v, full_v) = (relaxed.take(), full.take());
        assert_eq!(relaxed_v, full_v, "预算充足不缩,观感与完整模式一致");
        let (tight, full) = (tight.take(), full_v);
        assert_eq!(
            tight[0], full[0],
            "吃紧预算下短标签(完整宽 < 平分额)拿满自然宽:需要的先满足"
        );
        assert_eq!(
            tight[1], full[1],
            "次短标签同理拿满自然宽(cap 封顶退出分配)"
        );
        assert!(
            tight[2] < full[2],
            "长标题收窄承担全部挤压:{} < {}",
            tight[2],
            full[2]
        );
    }

    /// 省略号截断(#37):按 Unicode 字符边界 —— 输出必是原串的字符前缀 +
    /// 单个「…」,中文与 emoji(ZWJ 序列、旗帜)都不 panic、不出半个字符;
    /// 「前缀+省略号」实测宽不超预算;放得下时原样返回(无省略号)。
    #[test]
    fn elide_respects_char_boundaries_and_width_budget() {
        let ctx = egui::Context::default();
        let titles = [
            "一篇标题很长很长的中文文档.md".repeat(3),
            "👨‍👩‍👧全家福emoji标题🇨🇳🚀🚀🚀🎉".repeat(2),
            "mixed中English混合标题很很长很很长很很长".repeat(2),
        ];
        for title in titles {
            ctx.run_ui(RawInput::default(), |ui| {
                let font = egui::TextStyle::Button.resolve(ui.style());
                let full_w = text_width(ui, &title, &font);
                // 预算从 0 到全宽扫几档:任何档都不 panic、宽度合规
                for budget in [0.0, 8.0, 30.0, 80.0, full_w / 2.0, full_w] {
                    let shown = elide_text(ui, &title, &font, budget);
                    let original: Vec<char> = title.chars().collect();
                    let shown_chars: Vec<char> = shown.chars().collect();
                    if budget >= full_w {
                        assert_eq!(shown, title, "放得下原样返回");
                        continue;
                    }
                    assert_eq!(*shown_chars.last().unwrap(), '…', "截断必带省略号");
                    assert!(
                        shown_chars.len() < original.len() + 1,
                        "输出不长于原串+省略号"
                    );
                    // 字符前缀 + 省略号(逐 char 相等,绝无字节中间切开)
                    assert!(
                        shown_chars[..shown_chars.len() - 1] == original[..shown_chars.len() - 1],
                        "输出是原串的字符前缀"
                    );
                    let width = text_width(ui, &shown, &font);
                    if shown_chars.len() > 1 {
                        assert!(
                            width <= budget + 0.5,
                            "「{shown}」实测宽 {width} 超预算 {budget}"
                        );
                    }
                }
            })
            .drop_without_applying_deltas();
        }
    }

    /// 缩短模式的端到端渲染(#37):窄视口里长中文/emoji 标题不 panic、
    /// 画面上实际画出的是省略号版文本(含「…」、不含被截掉的尾部),
    /// 完整模式画面含完整标题;两种模式的条高都保持单行。
    #[test]
    fn short_mode_bar_elides_long_titles_end_to_end() {
        let ctx = egui::Context::default();
        let mut tabs = TabsState::new("");
        let title = "👨‍👩‍👧一篇标题很长很长的中文文档🚀🚀.md";
        tabs.open_tab(Some(std::path::PathBuf::from(format!("/docs/{title}"))), "");
        let mut outbox = Vec::new();
        let mut texts = |mode| {
            let mut painted = String::new();
            let height = Cell::new(0.0_f32);
            let output = ctx.run_ui(RawInput::default(), |ui| {
                ui.set_max_width(160.0);
                assert!(super::ui(ui, &tabs, mode, None, &mut outbox));
                height.set(ui.min_rect().height());
            });
            for clipped in &output.shapes {
                if let egui::epaint::Shape::Text(shape) = &clipped.shape {
                    painted.push_str(&shape.galley.job.text);
                }
            }
            output.drop_without_applying_deltas();
            (painted, height.get())
        };
        let (full_text, full_h) = texts(TitleWidthMode::Full);
        assert!(full_text.contains(title), "完整模式画面含完整标题");
        assert!(!full_text.contains('…'), "完整模式不加省略号");
        let (short_text, short_h) = texts(TitleWidthMode::Short);
        assert!(short_text.contains('…'), "缩短模式画面含省略号");
        assert!(
            !short_text.contains("中文文档🚀🚀.md"),
            "缩短模式画面不含被截断的尾部"
        );
        assert!(short_h < 2.0 * CHIP_H, "缩短模式条高 {}", short_h);
        assert!(full_h < 2.0 * CHIP_H, "完整模式条高 {}", full_h);
        assert!(outbox.is_empty(), "渲染不发消息");
    }

    /// 窄窗口下的最小宽与关闭按钮(#37 验收):视口比「最小宽之和」还窄,
    /// chip 仍保持最小宽(分配不跌破),且 × 命中区可点 —— 点击 chip 右端
    /// 关闭位发出 `TabCloseRequested`。
    #[test]
    fn narrow_viewport_keeps_min_width_and_working_close_button() {
        let ctx = egui::Context::default();
        let mut tabs = TabsState::new("");
        for i in 0..3 {
            tabs.open_tab(
                Some(std::path::PathBuf::from(format!(
                    "标题很长的文档第{i}篇.md"
                ))),
                "",
            );
        }
        let mut outbox = Vec::new();
        let shared = Cell::new(Vec::<f32>::new());
        let rect = Cell::new(Rect::NOTHING);
        // 视口 90px < 3×最小宽(168px):保底之和超预算,维持最小宽 + 滚动
        ctx.run_ui(RawInput::default(), |ui| {
            ui.set_max_width(90.0);
            shared.set(plan_widths(
                ui,
                &tabs,
                TitleWidthMode::Short,
                ui.available_width(),
            ));
            rect.set(ui.min_rect());
        })
        .drop_without_applying_deltas();
        for (index, width) in shared.take().iter().enumerate() {
            assert!(
                *width >= CHIP_MIN_W,
                "chip {index} 保持最小宽 {CHIP_MIN_W},实际 {width}"
            );
        }

        // 全条渲染后点第一个 chip 的右端关闭位:× 始终保留且可点
        let close_pos = egui::pos2(
            rect.get().left() + CHIP_MIN_W - SPACE_SM - CLOSE / 2.0,
            rect.get().top() + CHIP_H / 2.0,
        );
        let click = |pressed| Event::PointerButton {
            pos: close_pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        for events in [
            vec![Event::PointerMoved(close_pos)],
            vec![click(true)],
            vec![click(false)],
        ] {
            ctx.run_ui(
                RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_max_width(90.0);
                    super::ui(ui, &tabs, TitleWidthMode::Short, None, &mut outbox);
                },
            )
            .drop_without_applying_deltas();
        }
        assert_eq!(
            outbox,
            vec![Message::TabCloseRequested(0)],
            "窄窗口下关闭按钮仍可点"
        );
    }

    /// 两模式悬停 tooltip(#37):hover chip 一段时间后弹出 tooltip,画面
    /// 文本里能找到**完整标题**与**文件路径**(缩短模式下被省略号截掉的
    /// 部分从这里看全);未落盘的标签如实显示「尚未保存到磁盘」。
    #[test]
    fn hovering_chip_shows_tooltip_with_full_title_and_path() {
        let long_name = "一篇标题很长很长的中文文档🚀.md";
        for (mode, name) in [
            (TitleWidthMode::Full, "完整模式"),
            (TitleWidthMode::Short, "缩短模式"),
        ] {
            let ctx = egui::Context::default();
            // 第一个(也是唯一)标签就是长标题标签:chip 必在标签条最左,
            // hover 位置不必依赖前面的 chip 宽度
            let mut tabs = TabsState::new("");
            tabs.current_mut().document.path =
                Some(std::path::PathBuf::from(format!("/docs/{long_name}")));
            let mut outbox = Vec::new();
            let rect = Cell::new(Rect::NOTHING);
            ctx.run_ui(RawInput::default(), |ui| {
                ui.set_max_width(140.0); // 缩短模式下必触发截断
                assert!(super::ui(ui, &tabs, mode, None, &mut outbox));
                rect.set(ui.min_rect());
            })
            .drop_without_applying_deltas();

            // 悬停 chip 的名字区。egui 的 tooltip 要求指针**静止**超过
            // tooltip_delay(0.5s)才弹:第一帧把指针移过去,之后帧不再发任何
            // 指针事件(位置保持),只推时间 —— 每帧都发 PointerMoved 会不断
            // 重置「距上次移动」计时,tooltip 永远不出现。
            let hover_pos = egui::pos2(rect.get().left() + 4.0, rect.get().top() + CHIP_H / 2.0);
            let mut painted = String::new();
            let mut now = 0.0_f64;
            for frame in 0..12 {
                now += 0.15;
                let events = if frame == 0 {
                    vec![Event::PointerMoved(hover_pos)]
                } else {
                    Vec::new()
                };
                let output = ctx.run_ui(
                    RawInput {
                        events,
                        time: Some(now),
                        ..Default::default()
                    },
                    |ui| {
                        ui.set_max_width(140.0);
                        super::ui(ui, &tabs, mode, None, &mut outbox);
                    },
                );
                painted.clear();
                for clipped in &output.shapes {
                    if let egui::epaint::Shape::Text(shape) = &clipped.shape {
                        painted.push_str(&shape.galley.job.text);
                    }
                }
                output.drop_without_applying_deltas();
                if painted.contains("/docs/") {
                    break;
                }
            }
            assert!(
                painted.contains(long_name),
                "{name}:tooltip 画面含完整标题(chip 上被截断显示,tooltip 看全)"
            );
            assert!(painted.contains("/docs/"), "{name}:tooltip 画面含文件路径");
            assert!(outbox.is_empty(), "悬停不发消息");
        }

        // 未落盘的标签如实显示「尚未保存到磁盘」,不编造路径。第二个标签
        // 让标签条必画(单空标签不画条),hover 的 chip 0 同样未落盘。
        let ctx = egui::Context::default();
        let mut tabs = TabsState::new("");
        tabs.open_tab(None, "未落盘的草稿");
        let mut outbox = Vec::new();
        let rect = Cell::new(Rect::NOTHING);
        ctx.run_ui(RawInput::default(), |ui| {
            assert!(super::ui(
                ui,
                &tabs,
                TitleWidthMode::Full,
                None,
                &mut outbox
            ));
            rect.set(ui.min_rect());
        })
        .drop_without_applying_deltas();
        let hover_pos = egui::pos2(rect.get().left() + 4.0, rect.get().top() + CHIP_H / 2.0);
        let mut painted = String::new();
        let mut now = 0.0_f64;
        for frame in 0..12 {
            now += 0.15;
            let events = if frame == 0 {
                vec![Event::PointerMoved(hover_pos)]
            } else {
                Vec::new()
            };
            let output = ctx.run_ui(
                RawInput {
                    events,
                    time: Some(now),
                    ..Default::default()
                },
                |ui| {
                    super::ui(ui, &tabs, TitleWidthMode::Full, None, &mut outbox);
                },
            );
            painted.clear();
            for clipped in &output.shapes {
                if let egui::epaint::Shape::Text(shape) = &clipped.shape {
                    painted.push_str(&shape.galley.job.text);
                }
            }
            output.drop_without_applying_deltas();
            if painted.contains("尚未保存到磁盘") {
                break;
            }
        }
        assert!(
            painted.contains("尚未保存到磁盘"),
            "未落盘标签的 tooltip 如实说明"
        );
    }

    /// 右键菜单 M4(context-menu-plan §3 M4):复制路径/复制相对路径/
    /// 在文件管理器中显示 —— 与文件树右键共用消息与归约实现。
    #[test]
    fn context_menu_file_items_follow_tab_path() {
        use crate::tabs::TabsState;
        use std::path::PathBuf;

        let ctx = egui::Context::default();
        // 落盘标签:三枚条目全部可用
        // 用 open_tab 建落盘标签(spawn 语义在 state 层,这里直接构造)
        let mut tabs = TabsState::new("旧");
        let file_index = tabs.open_tab(Some(PathBuf::from("/root/文档.md")), "文档");
        let mut outbox = Vec::new();
        let items = Cell::new(None);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            items.set(Some(context_menu_items(
                ui,
                &tabs,
                file_index,
                TitleWidthMode::Full,
                None,
                &mut outbox,
            )));
        });
        output.drop_without_applying_deltas();
        let items = items.into_inner().expect("已渲染");
        assert!(items.copy_path.enabled(), "落盘标签可复制路径");
        assert!(items.reveal.enabled(), "落盘标签可在文件管理器显示");
        assert!(!items.copy_relative.enabled(), "无文件树根时相对路径不可用");

        // 在文件树根下:相对路径可用,点击产出 relative 消息
        let mut outbox = Vec::new();
        let root = PathBuf::from("/root");
        let probed = Cell::new(egui::Rect::NOTHING);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let items = context_menu_items(
                ui,
                &tabs,
                file_index,
                TitleWidthMode::Full,
                Some(root.as_path()),
                &mut outbox,
            );
            assert!(items.copy_relative.enabled(), "根下可复制相对路径");
            probed.set(items.copy_relative.rect);
        });
        output.drop_without_applying_deltas();
        let rect = probed.get();
        let click = |pressed| Event::PointerButton {
            pos: rect.center(),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        for events in [
            vec![Event::PointerMoved(rect.center()), click(true)],
            vec![click(false)],
        ] {
            let output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    context_menu_items(
                        ui,
                        &tabs,
                        file_index,
                        TitleWidthMode::Full,
                        Some(root.as_path()),
                        &mut outbox,
                    );
                },
            );
            output.drop_without_applying_deltas();
        }
        assert_eq!(
            outbox,
            vec![Message::FileTreeCopyPath {
                path: PathBuf::from("/root/文档.md"),
                relative: true,
            }]
        );

        // 未落盘标签:三枚条目全部禁用
        let tabs = TabsState::new("草稿");
        let mut outbox = Vec::new();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let items = context_menu_items(ui, &tabs, 0, TitleWidthMode::Full, None, &mut outbox);
            assert!(!items.copy_path.enabled());
            assert!(!items.copy_relative.enabled());
            assert!(!items.reveal.enabled());
        });
        output.drop_without_applying_deltas();
    }
}
