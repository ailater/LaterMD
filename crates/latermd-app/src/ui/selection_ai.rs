//! 选区 AI 浮标(#61 M1):TextEdit 选中一段内容后,选区尾端附近浮出
//! AI 小标识,点开「AI 续写 / AI 润色」两动作菜单。
//!
//! 源码模式(`ui::editor`)与 Live 模式活动块(`live`)共用本模块:调用点
//! 各自从 TextEdit 输出算出**选区尾端**光标条的屏幕矩形,可见性收在纯函数
//! [`badge_visible`],绘制与命中统一走 [`show`]。
//!
//! 层级纪律(13a 教训):浮标与菜单行的命中区都在调用点的**同层末尾**注册
//! (`Ui::interact`,调用点位于各自 ScrollArea 之后)—— egui 同层命中平局
//! 取后注册者,浮标盖过正文;矩形之外的点击/拖选/打字行为分毫不变。绝不
//! 放 Foreground 层 Area:hit_test 的跨层屏蔽会吞掉周边点击(`ui::titlebar`
//! ::edge_resize_zones 的实测教训)。浮标无键盘语义、不 `request_focus`,
//! 编辑器焦点全程不动,出现期间打字正常。
//!
//! widget id 全部挂在**标签稳定的 editor_id** 之下,不含内容长度/hash
//! (AGENTS §6.7);尾端矩形每帧从本帧 galley 重算,无跨帧陈旧偏移(#17)。
//!
//! 交付面 = 源码 + Live。右栏纯预览的选区活在 vendored 标签层(egui
//! `LabelSelectionState`,app 侧读不到偏移与尾端矩形),M1 不覆盖
//! (decisions-pending #115 如实记录)。

use crate::state::{Message, SelectionAiAction};
use eframe::egui;

/// 浮标 chip 尺寸:够放「AI」两字符,不遮正文。
pub(crate) const BADGE_W: f32 = 28.0;
pub(crate) const BADGE_H: f32 = 17.0;
/// 浮标与选区尾端光标条的横向间隙。
const TAIL_GAP_X: f32 = 5.0;
/// 两动作菜单的行高/整宽/内边距。
const MENU_ROW_H: f32 = 22.0;
const MENU_W: f32 = 84.0;
const MENU_PAD: f32 = 3.0;
/// 菜单到浮标的纵向间隙。
const MENU_GAP_Y: f32 = 4.0;
/// 贴裁剪边的最小余量(半出界的浮标不如不出)。
const CLIP_MARGIN: f32 = 2.0;

/// 「拖选进行中」判定:`dragged_id` 存在且**不属于本浮标**。浮标/菜单行
/// 以 `Sense::click_and_drag` 注册(见 [`show`]),按在浮标上的帧
/// `dragged_id` 是浮标自己 —— 那是点按不是拖选,不算数。
pub(crate) fn is_floater_id(editor_id: egui::Id, id: egui::Id) -> bool {
    id == editor_id.with("selection-ai-badge")
        || (0..SelectionAiAction::ALL.len())
            .any(|index| id == editor_id.with(("selection-ai-action", index)))
}

/// 浮标可见性判定(#61 M1 口径,纯函数):持久选区两端不同(#38 复制按钮
/// 同源读法)、编辑器(源码 TextEdit / Live 活动块)持焦点、且本帧无(非
/// 浮标的)拖拽进行中 —— 三者同时成立才弹。
///
/// 「拖拽进行中」以 `ctx.dragged_id()` 为准:拖选期间 TextEdit 是被拖的
/// widget,指针**释放帧** dragged_id 已清空,浮标当帧即出 —— 即「以指针
/// 释放帧起算」(任务书留判自选,decisions-pending #115)。
pub(crate) fn badge_visible(
    selection: Option<(usize, usize)>,
    focused: bool,
    dragging: bool,
) -> bool {
    !dragging && focused && selection.is_some_and(|(primary, secondary)| primary != secondary)
}

/// 选区尾端光标条的屏幕矩形 → 浮标矩形:同行右侧、垂直居中;右侧越出
/// 裁剪区时左移贴边。
fn badge_rect(tail: egui::Rect, clip: egui::Rect) -> egui::Rect {
    let y = tail.center().y - BADGE_H / 2.0;
    let x = (tail.max.x + TAIL_GAP_X).min((clip.right() - CLIP_MARGIN - BADGE_W).max(clip.left()));
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(BADGE_W, BADGE_H))
}

/// 两动作菜单矩形:浮标正下方,下方放不下(视口底)翻上方,横向钳进裁剪区。
fn menu_rect(badge: egui::Rect, clip: egui::Rect) -> egui::Rect {
    let size = egui::vec2(
        MENU_W,
        MENU_ROW_H * SelectionAiAction::ALL.len() as f32 + MENU_PAD * 2.0,
    );
    let below = badge.left_bottom() + egui::vec2(0.0, MENU_GAP_Y);
    let min = if below.y + size.y > clip.bottom() - CLIP_MARGIN {
        egui::pos2(badge.left(), badge.top() - MENU_GAP_Y - size.y)
    } else {
        below
    };
    let x = min.x.clamp(
        clip.left() + CLIP_MARGIN,
        (clip.right() - CLIP_MARGIN - size.x).max(clip.left()),
    );
    egui::Rect::from_min_size(egui::pos2(x, min.y), size)
}

/// 每帧写进 data temp 的取证探针(照 minimap/zen_nav 先例,生产只写不读):
/// 供无头测试断言「未选中时零浮标元素(否决线)/ 选中时浮标与菜单矩形 /
/// 点击不吞输入」。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Probe {
    /// 浮标 chip 的屏幕矩形;`None` = 本帧未画(零浮标元素)。
    pub(crate) badge: Option<egui::Rect>,
    /// 两动作菜单的屏幕矩形;`None` = 本帧未画。
    pub(crate) menu: Option<egui::Rect>,
}

/// 探针的 data key(挂标签稳定 editor_id,切标签互不惊扰)。
pub(crate) fn probe_id(editor_id: egui::Id) -> egui::Id {
    editor_id.with("selection-ai-probe")
}

/// 本帧浮标命中的全部矩形(浮标 + 打开时的菜单):Live 侧
/// `clicked_for_edit` 据此排除「点在浮标上」的点击,不误进块编辑。
pub(crate) fn hit_rects(ctx: &egui::Context, editor_id: egui::Id) -> Vec<egui::Rect> {
    let probe: Probe = ctx.data(|d| d.get_temp(probe_id(editor_id)).unwrap_or_default());
    [probe.badge, probe.menu].into_iter().flatten().collect()
}

/// 菜单开合标志的 data key(挂标签稳定 editor_id)。
fn menu_open_id(editor_id: egui::Id) -> egui::Id {
    editor_id.with("selection-ai-menu-open")
}

/// 绘制浮标与(打开时的)两动作菜单。**必须在 ScrollArea 之后的同层
/// 调用**(13a 纪律);`anchor` = 选区尾端光标条的屏幕矩形(调用点经
/// [`badge_visible`] 判可见后给出),`selection` = 点选帧的选区字符区间
/// (**文档坐标**:源码模式是 TextEdit 持久选区原值,Live 模式是活动块
/// 内选区加块基换算的全文偏移),随动作消息带走 —— M2 归约凭它捕获插入
/// 点,不读 `TabState::selection`(Live 不回填该字段)。`clip` = 编辑视口
/// 矩形(浮标/菜单只在视口内出,随滚动出视口即隐),`None` = 本帧隐藏。
///
/// 焦点契约:egui 0.36 内建清焦(`surrender_focus_on: Presses` —— 按在
/// 别处且本 widget 未悬停即交出焦点)发生在 widget 创建段,「点浮标」
/// 那一帧会在本函数之前吃掉编辑器焦点。调用方须在判可见性**之前**按
/// [`hit_rects`](上一帧的浮标/菜单矩形)把焦点还给编辑器(layout.rs
/// `keep_find_focus` 同款手法):浮标不抢焦点,点完菜单接着打字。
pub(crate) fn show(
    ui: &egui::Ui,
    editor_id: egui::Id,
    selection: Option<(usize, usize)>,
    anchor: Option<egui::Rect>,
    clip: egui::Rect,
    outbox: &mut Vec<Message>,
) {
    let ctx = ui.ctx();
    let menu_key = menu_open_id(editor_id);
    let mut menu_open: bool = ctx.data_mut(|d| d.get_temp(menu_key).unwrap_or(false));

    // 隐藏路径(无选区 / 拖拽中 / 失焦 / 尾端在视口外):菜单合拢、探针
    // 清零,一个形状都不画、一个命中区都不注册。
    let badge = anchor
        .map(|tail| badge_rect(tail, clip))
        .filter(|badge| clip.contains_rect(*badge));
    let Some(badge) = badge else {
        if menu_open {
            ctx.data_mut(|d| d.insert_temp(menu_key, false));
        }
        ctx.data_mut(|d| d.insert_temp(probe_id(editor_id), Probe::default()));
        return;
    };

    // 浮标本体:点击开/合菜单。sense 取 click_and_drag 而非 click —— egui
    // 0.36 的命中裁决按 click/drag **分榜**取最近(hittest.rs:「Report
    // both hits, e.g. the top Button and the ScrollArea behind it」),click
    // 单 sense 的浮标会赢得 click 榜、却把 drag 榜留给下面的 TextEdit,按
    // 在浮标上的一下仍会经拖选路径塌缩选区;双 sense 让浮标在两榜都以后
    // 注册者居上,TextEdit 分毫不沾。interact 无键盘语义,不主动碰焦点。
    let badge_response = ui.interact(
        badge,
        editor_id.with("selection-ai-badge"),
        egui::Sense::click_and_drag(),
    );
    if badge_response.hovered() {
        ui.output_mut(|out| out.cursor_icon = egui::CursorIcon::PointingHand);
    }
    if badge_response.clicked() {
        menu_open = !menu_open;
    }
    let menu = menu_open.then(|| menu_rect(badge, clip));

    // 菜单开着时,浮标与菜单之外的按下帧即合拢(选区塌缩走隐藏路径,
    // 这里兜「点击不塌缩选区的位置」)。
    if menu_open
        && ui.input(|input| {
            input.pointer.primary_pressed()
                && input.pointer.interact_pos().is_some_and(|pos| {
                    !badge.contains(pos) && !menu.is_some_and(|rect| rect.contains(pos))
                })
        })
    {
        menu_open = false;
    }

    // —— 绘制(调用点在 ScrollArea 之后,后画者居上,浮标盖住正文)——
    let painter = ui.painter_at(clip);
    let dark = ui.visuals().dark_mode;
    painter.rect_filled(
        badge,
        egui::CornerRadius::same(4),
        crate::ui::rich_widgets::ai_link_color(dark),
    );
    painter.text(
        badge.center(),
        egui::Align2::CENTER_CENTER,
        "AI",
        egui::FontId::proportional(10.5),
        egui::Color32::WHITE,
    );

    let mut fired: Option<SelectionAiAction> = None;
    if let Some(menu) = menu {
        painter.rect_filled(menu, egui::CornerRadius::same(4), ui.visuals().panel_fill);
        painter.rect_stroke(
            menu,
            egui::CornerRadius::same(4),
            ui.visuals().widgets.noninteractive.bg_stroke,
            egui::StrokeKind::Outside,
        );
        for (index, action) in SelectionAiAction::ALL.iter().enumerate() {
            let row = egui::Rect::from_min_size(
                egui::pos2(
                    menu.left() + MENU_PAD,
                    menu.top() + MENU_PAD + MENU_ROW_H * index as f32,
                ),
                egui::vec2(menu.width() - MENU_PAD * 2.0, MENU_ROW_H),
            );
            // 行 id 按菜单行序(ALL 是 const,行序稳定),不含任何内容派生;
            // sense 双榜同浮标(见上)。
            let response = ui.interact(
                row,
                editor_id.with(("selection-ai-action", index)),
                egui::Sense::click_and_drag(),
            );
            if response.hovered() {
                painter.rect_filled(
                    row,
                    egui::CornerRadius::same(3),
                    ui.visuals().widgets.hovered.weak_bg_fill,
                );
                ui.output_mut(|out| out.cursor_icon = egui::CursorIcon::PointingHand);
            }
            painter.text(
                egui::pos2(row.left() + crate::ui::tokens::SPACE_SM, row.center().y),
                egui::Align2::LEFT_CENTER,
                action.label(),
                egui::FontId::proportional(12.0),
                ui.visuals().text_color(),
            );
            if response.clicked() {
                fired = Some(*action);
            }
        }
    }
    if fired.is_some() {
        menu_open = false;
    }
    if let Some(action) = fired {
        outbox.push(Message::SelectionAiActionRequested { action, selection });
    }

    ctx.data_mut(|d| d.insert_temp(menu_key, menu_open));
    ctx.data_mut(|d| {
        d.insert_temp(
            probe_id(editor_id),
            Probe {
                badge: Some(badge),
                menu: menu_open.then(|| menu_rect(badge, clip)),
            },
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 可见性判定矩阵:光标塌缩/无选区/失焦/拖拽中一律不弹;选区 + 焦点
    /// + 无拖拽成立。
    #[test]
    fn badge_visible_requires_selection_focus_and_no_drag() {
        assert!(
            !badge_visible(Some((3, 3)), true, false),
            "塌缩光标不算选中"
        );
        assert!(!badge_visible(None, true, false));
        assert!(!badge_visible(Some((1, 4)), false, false), "失焦即隐");
        assert!(!badge_visible(Some((1, 4)), true, true), "拖拽进行中不弹");
        assert!(badge_visible(Some((1, 4)), true, false));
        assert!(badge_visible(Some((9, 2)), true, false), "反向选区同样成立");
    }

    /// 浮标几何:同行右侧垂直居中;尾端贴右缘时左移收进裁剪区;菜单默认
    /// 在下方,视口底翻上方,横向钳进裁剪区。
    #[test]
    fn badge_and_menu_rects_stay_inside_clip() {
        let clip = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        let tail = egui::Rect::from_min_max(egui::pos2(100.0, 40.0), egui::pos2(100.0, 60.0));
        let badge = badge_rect(tail, clip);
        assert_eq!(badge.left(), 105.0, "尾端右侧 + 5px 间隙");
        assert_eq!(badge.center().y, tail.center().y, "与尾端行垂直居中");
        assert!(clip.contains_rect(badge));

        let tail_edge = egui::Rect::from_min_max(egui::pos2(398.0, 40.0), egui::pos2(398.0, 60.0));
        let badge = badge_rect(tail_edge, clip);
        assert!(
            badge.right() <= clip.right() - CLIP_MARGIN,
            "越界左移贴边(实测 right={})",
            badge.right()
        );

        let badge_mid =
            egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(BADGE_W, BADGE_H));
        let menu = menu_rect(badge_mid, clip);
        assert!(
            menu.top() >= badge_mid.bottom() + MENU_GAP_Y - 0.01,
            "默认在下方"
        );
        assert!(clip.contains_rect(menu));
        let badge_low =
            egui::Rect::from_min_size(egui::pos2(100.0, 290.0), egui::vec2(BADGE_W, BADGE_H));
        let menu = menu_rect(badge_low, clip);
        assert!(
            menu.bottom() <= badge_low.top(),
            "下方放不下翻上方(实测 menu={menu:?})"
        );
        assert!(clip.contains_rect(menu));
    }
}
