//! 禅定模式的左缘标签导航(#57 M1 悬停唤出;M2 三态配置:悬停/常显/关闭)。
//! 点击跳转标签;悬停态下鼠标移近左缘唤出、离开去抖后即隐,常显态进禅定
//! 即显示,关闭态零路径不渲染。
//!
//! 层级纪律(13a 教训,auto-plan #57 写死):感应区与导航列都留在
//! `draw_zen` 的**同一层**。感应区是纯几何判定(只读指针位置,不注册任何
//! widget,零命中面积、零命中屏蔽);导航列用根 `ui` 的 `allocate_rect` +
//! 同层子 `Ui` 摆进绝对矩形——**不得**搬进 Foreground 层的 Area:egui
//! 0.36 的命中测试会把「更高层里靠近指针的 widget」所在层之外整体排除
//! (hit_test.rs 的 included_layers 启发式),导航列会跨层吞掉禅定正文与
//! 右上退出钮的点击(titlebar.rs `edge_resize_zones` 文档的实测教训)。
//! 同层则按距离裁决、同距后画者胜:导航列画在 CentralPanel 之后盖住正文,
//! 边缘缩放区仍最后分配,左缘 6px 缩放手势照常获胜。
//!
//! 三态共用同一渲染件(任务书钉死):分叉只在 [`ui_with_probe`] 开头的
//! 显隐判定一步——悬停走 [`step`](指针几何 × 去抖),常显恒真(列是常驻
//! chrome 而非指针的衍生显示态,无感应区、无去抖),关闭首行早退(零路径
//! = 不读指针、不推进判定、不注册任何形状与命中,与「全隐帧零形状」同一
//! 条红线)。判定之后的动画与绘制(淡入滑入、行、滚动)三态完全共用。
//!
//! 悬停态的显隐判定是纯函数(指针位置 × 感应区/导航列矩形 × 去抖计数):
//! 指针移动本身产生事件帧驱动它,不为此排程 repaint;唤出/隐藏的淡入位移
//! 动画由 egui 动画管理器自驱(进行中自动 request_repaint)。唤出期间
//! 不抢键盘焦点(列内无 TextEdit、不 request_focus),键盘与命令快捷键
//! 照常到达;只有点击导航行本身发 [`Message::TabActivate`]。

use crate::state::Message;
use crate::tabs::TabsState;
use crate::theme::ZenNavMode;
use crate::ui::{fade, tokens};
use eframe::egui;

/// 左缘感应区宽度(任务书建议 12–24px:再窄难以无意划过命中,再宽会把
/// 「阅读时停在最左侧」误判成唤出意图)。
pub const EDGE_W: f32 = 16.0;
/// 导航列宽。装得下常规文件名 + 脏星,长名走省略号截断(与标签条同款)。
pub const NAV_W: f32 = 220.0;
/// 唤出/隐藏动画的位移分量:自左缘滑入的像素数,与整体淡入(0.15s,
/// `ui::fade`)叠加。纯淡入在深色正文上「凭空浮现」,一点位移给出「从
/// 边缘抽出」的方向感。
pub const SLIDE_PX: f32 = 24.0;
/// 隐藏去抖帧数(~200ms @60Hz):指针离开感应区/导航列后保持这么多帧
/// 再隐,吸收沿左缘抖动与「贴边划过」的瞬时进出;期间指针回来即清零。
pub const HIDE_DEBOUNCE_FRAMES: u32 = 12;

/// 导航列必须覆盖感应区:唤出后指针自感应区滑进列内,两者连续不断链
/// (去抖只兜边界抖动,不兜结构性空洞)。编译期钉死。
const _: () = assert!(NAV_W > EDGE_W);

/// 感应区矩形:左缘竖条,上界让出禅定同样保留的标题栏(`top` 之下才
/// 感应——标题栏是拖窗区,贴顶划过不该唤出导航)。
pub fn edge_zone(screen: egui::Rect, top: f32) -> egui::Rect {
    egui::Rect::from_two_pos(
        egui::pos2(screen.left(), top),
        egui::pos2(screen.left() + EDGE_W, screen.bottom()),
    )
}

/// 导航列的驻位矩形(完全展开时的位置;动画期间的实际矩形是它向左位移
/// 的插值)。显隐判定与绘制都以它为基准。
pub fn nav_rect(screen: egui::Rect, top: f32) -> egui::Rect {
    egui::Rect::from_two_pos(
        egui::pos2(screen.left(), top),
        egui::pos2(screen.left() + NAV_W, screen.bottom()),
    )
}

/// 第 `index` 行的矩形(滚动归零时)。行由流式布局堆叠(行间 item_spacing
/// 已压为 0),与绘制同源地算出来,供无头测试定位真按钮。
#[cfg(test)]
pub(crate) fn row_rect(nav: egui::Rect, index: usize) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(nav.left(), nav.top() + tokens::NAV_ROW_H * index as f32),
        egui::vec2(nav.width(), tokens::NAV_ROW_H),
    )
}

/// 指针是否在「感应区 ∪ 导航列」内。`nav` 传 [`egui::Rect::NOTHING`]
/// 即导航列不参与判定(全隐时它不占屏幕,见 [`step`] 的非对称规则)。
pub fn proximity(pointer: Option<egui::Pos2>, zone: egui::Rect, nav: egui::Rect) -> bool {
    match pointer {
        Some(p) => zone.contains(p) || nav.contains(p),
        None => false,
    }
}

/// 显隐判定一步(纯函数):`(当前可见位, 连续在外帧数, 指针, 感应区,
/// 导航列)` → `(新可见位, 新计数)`。
///
/// 非对称规则:唤出只认**感应区**(或仍在屏幕上的导航列);全隐的导航列
/// 不占屏幕,指针停在它的驻位矩形里不该凭空唤出——调用方在全隐时传
/// `NOTHING`。进入即显示(去抖只在隐藏侧:唤出要即时,迟疑的感应区等于
/// 没有感应区),离开后 [`HIDE_DEBOUNCE_FRAMES`] 帧内保持显示,期间回来
/// 清零重计。
pub fn step(
    visible: bool,
    outside_frames: u32,
    pointer: Option<egui::Pos2>,
    zone: egui::Rect,
    nav: egui::Rect,
) -> (bool, u32) {
    if proximity(pointer, zone, nav) {
        (true, 0)
    } else {
        let outside_frames = outside_frames.saturating_add(1);
        (
            visible && outside_frames < HIDE_DEBOUNCE_FRAMES,
            outside_frames,
        )
    }
}

/// 左缘标签导航的会话级状态(不持久化;退出禅定即复位,见
/// `State::toggle_zen`;切换三态配置也复位,见 `Message::ZenNavModeChanged`
/// 的归约)。每帧由 `ui::zen_nav::ui_with_probe` 推进——悬停态的显隐是
/// 指针几何的衍生显示态(常显态 `visible` 被钉在 true,判定不参与),
/// 与 quick_open 的查询草稿同款归 UI 原地持有。
#[derive(Debug, Clone, PartialEq)]
pub struct ZenNavState {
    /// 去抖后的可见位(显隐判定输出,动画目标;常显态恒 true)。
    pub visible: bool,
    /// 连续「指针在感应区与导航列之外」的帧计数。
    pub outside_frames: u32,
    /// 上一帧动画值(>0 = 导航列仍占据屏幕,显隐判定的导航列矩形按它取)。
    pub alpha: f32,
}

impl Default for ZenNavState {
    fn default() -> Self {
        Self {
            visible: false,
            outside_frames: 0,
            alpha: 0.0,
        }
    }
}

/// 绘制左缘标签导航(生产入口;探针恒 `None`,零开销)。
pub fn ui(
    panel: &mut egui::Ui,
    mode: ZenNavMode,
    frameless: bool,
    tabs: &TabsState,
    zen_nav: &mut ZenNavState,
    outbox: &mut Vec<Message>,
) {
    ui_with_probe(
        panel,
        mode,
        frameless,
        tabs,
        zen_nav,
        outbox,
        None::<fn(egui::Rect)>,
    );
}

/// 同 [`ui`],额外把**实际画出来的**导航列矩形交给 `probe`(只供无头
/// 测试定位;全隐帧与关闭档不调用——测试据此断言零导航元素)。
pub fn ui_with_probe(
    panel: &mut egui::Ui,
    mode: ZenNavMode,
    frameless: bool,
    tabs: &TabsState,
    zen_nav: &mut ZenNavState,
    outbox: &mut Vec<Message>,
    probe: Option<impl FnMut(egui::Rect)>,
) {
    let screen = panel.max_rect();
    let top = if frameless { tokens::TITLEBAR_H } else { 0.0 };
    let rest = nav_rect(screen, top);

    // 显隐判定按模式分叉(三态共用其后的一切):悬停 = 指针几何 × 去抖
    // (M1 判定);常显 = 恒真(列是常驻 chrome,与指针无关);关闭 = 零
    // 路径早退 —— 不读指针、不推进判定、不注册任何形状与命中区,与
    // 「全隐帧零形状」同一条红线(否决线:探针零命中)。
    let (visible, outside_frames) = match mode {
        ZenNavMode::Off => return,
        ZenNavMode::Always => (true, 0),
        ZenNavMode::Hover => {
            let zone = edge_zone(screen, top);
            // 显隐判定:导航列「占据屏幕」按上一帧动画值与可见位取(隐藏
            // 动画进行中它仍在屏幕上,指针回到列内要能留住)。
            let nav_hit = if zen_nav.visible || zen_nav.alpha > 0.0 {
                rest
            } else {
                egui::Rect::NOTHING
            };
            let pointer = panel.ctx().input(|input| input.pointer.latest_pos());
            step(
                zen_nav.visible,
                zen_nav.outside_frames,
                pointer,
                zone,
                nav_hit,
            )
        }
    };
    zen_nav.visible = visible;
    zen_nav.outside_frames = outside_frames;

    // 动画:可见期淡入滑入(egui 动画管理器自驱要帧)。全隐帧把动画器
    // 钉回 0——上一次禅定会话的残值会在重进禅定时凭空闪一列淡出残影。
    let id = egui::Id::new("zen-nav-fade");
    let alpha = if visible || zen_nav.alpha > 0.0 {
        fade::crossfade(panel.ctx(), id, visible)
    } else {
        panel.ctx().animate_bool_with_time(id, false, 0.0)
    };
    zen_nav.alpha = alpha;

    // 全隐 = 零形状零命中:不进任何绘制分支(非禅定模式更是根本不调本
    // 函数,三栏渲染逐像素不受影响)。
    if alpha <= 0.0 {
        return;
    }

    // 实际矩形:自左缘滑入(位移与淡入同一 alpha 插值)。
    let rect = rest.translate(egui::vec2(-SLIDE_PX * (1.0 - alpha), 0.0));

    // 整列容器(同层绝对摆放):空行区(标签少于列高时的下方空白)点击
    // 落在本容器上而不是穿透到正文;行在其后分配,同层同距后画者胜,
    // 行点击优先于容器。
    panel.allocate_rect(rect, egui::Sense::click());

    let mut column = panel.new_child(
        egui::UiBuilder::new()
            .id_salt("zen-nav")
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    if let Some(mut probe) = probe {
        probe(rect);
    }
    if !column.is_rect_visible(rect) {
        return;
    }
    // 整列透明度跟随动画:背景、边线、行、悬停底一起淡入。
    column.set_opacity(alpha);
    let painter = column.painter();
    painter.rect_filled(rect, 0.0, column.visuals().panel_fill);
    painter.vline(
        rect.right(),
        rect.y_range(),
        column.visuals().widgets.noninteractive.bg_stroke,
    );

    egui::ScrollArea::vertical()
        .id_salt("zen-nav-scroll")
        .auto_shrink([false, false])
        .show(&mut column, |ui| {
            // 行间零间距:行矩形 = 驻位矩形自上而下等分(测试按 row_rect 定位)。
            ui.spacing_mut().item_spacing.y = 0.0;
            for (index, tab) in tabs.tabs.iter().enumerate() {
                row(ui, index, &tab.display_name(), index == tabs.active, outbox);
            }
        });
}

/// 单条标签行:整行可点(`Sense::click` 打在 allocate 上,与侧栏视图
/// 导航行同款手法);当前标签 = selected_bg 底 + 左侧 2px 强调色竖条 +
/// 强调色文字,脏标记是显示名自带的 `*`(与标签条同一显示口径)。点击
/// 发 [`Message::TabActivate`],跳转在既有归约。
fn row(ui: &mut egui::Ui, index: usize, label: &str, selected: bool, outbox: &mut Vec<Message>) {
    let size = egui::vec2(ui.available_width().max(0.0), tokens::NAV_ROW_H);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    // 省略号截断要先算(painter 借用期间不能再 &mut ui)。
    let font = egui::TextStyle::Button.resolve(ui.style());
    let budget = (rect.width() - tokens::SPACE_SM * 2.0).max(0.0);
    let text = crate::ui::tabs::elide_text(ui, label, &font, budget);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let visuals = ui.visuals();
        if selected {
            painter.rect_filled(rect, 0.0, crate::theme::shell(ui).selected_bg);
            painter.rect_filled(
                egui::Rect::from_min_size(
                    rect.left_top(),
                    egui::vec2(tokens::NAV_BAR_W, rect.height()),
                ),
                0.0,
                tokens::accent(ui),
            );
        } else if response.hovered() {
            painter.rect_filled(rect, 0.0, visuals.widgets.hovered.bg_fill);
        }
        let color = if selected {
            tokens::accent(ui)
        } else {
            visuals.text_color()
        };
        painter.text(
            egui::pos2(rect.left() + tokens::SPACE_SM, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            font,
            color,
        );
    }
    if response.clicked() {
        outbox.push(Message::TabActivate(index));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone() -> egui::Rect {
        edge_zone(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0)),
            36.0,
        )
    }

    fn nav() -> egui::Rect {
        nav_rect(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0)),
            36.0,
        )
    }

    /// 感应区与导航列的几何:都贴左缘、让出标题栏;导航列比感应区宽
    /// (编译期不变量,见常量区的 const assert),行自列顶起堆。
    #[test]
    fn geometry_hugs_left_edge_below_titlebar() {
        assert_eq!(zone().left_top(), egui::pos2(0.0, 36.0));
        assert_eq!(zone().width(), EDGE_W);
        assert_eq!(nav().left_top(), egui::pos2(0.0, 36.0));
        assert_eq!(nav().width(), NAV_W);
        assert_eq!(
            row_rect(nav(), 2).top(),
            nav().top() + tokens::NAV_ROW_H * 2.0,
            "行矩形自列顶等分堆叠"
        );
    }

    /// 判定矩阵:指针位置 × (感应区/导航列/两者之外/无指针) × 导航列
    /// 是否在屏。全隐的导航列不参与判定(非对称规则)。
    #[test]
    fn proximity_matrix() {
        let (zone, nav) = (zone(), nav());
        // 感应区内(贴左缘中部):恒显示
        assert!(proximity(Some(egui::pos2(8.0, 300.0)), zone, nav));
        // 导航列内(感应区之外的部分):列在屏才显示
        assert!(proximity(Some(egui::pos2(100.0, 300.0)), zone, nav));
        assert!(!proximity(
            Some(egui::pos2(100.0, 300.0)),
            zone,
            egui::Rect::NOTHING
        ));
        // 两者之外:不显示
        for outside in [egui::pos2(400.0, 300.0), egui::pos2(8.0, 20.0)] {
            assert!(!proximity(Some(outside), zone, nav), "{outside:?}");
        }
        // 无指针(窗口失焦/指针离开视口):不显示
        assert!(!proximity(None, zone, nav));
    }

    /// 去抖序列:进入即显示;离开后保持 HIDE_DEBOUNCE_FRAMES−1 帧,到点
    /// 隐藏;去抖中途回来清零;隐藏态不受「指针在驻位矩形」影响(列不在
    /// 屏,不唤出);计数饱和不溢出。
    #[test]
    fn step_debounces_hide_and_reshows_on_reentry() {
        let (zone, nav) = (zone(), nav());
        let inside = Some(egui::pos2(8.0, 300.0));
        let outside = Some(egui::pos2(400.0, 300.0));

        // 初始(隐藏、无指针)保持隐藏
        assert_eq!(step(false, 0, None, zone, nav), (false, 1));

        // 进入感应区:立即显示
        assert_eq!(step(false, 3, inside, zone, nav), (true, 0));

        // 离开:第 1..=HIDE_DEBOUNCE_FRAMES-1 帧仍显示,第 HIDE 帧隐藏
        let (mut visible, mut frames) = (true, 0);
        for n in 1..HIDE_DEBOUNCE_FRAMES {
            (visible, frames) = step(visible, frames, outside, zone, nav);
            assert!(visible, "离开后第 {n} 帧仍在去抖窗口内");
            assert_eq!(frames, n);
        }
        (visible, frames) = step(visible, frames, outside, zone, nav);
        assert!(!visible, "连续在外到 HIDE_DEBOUNCE_FRAMES 帧即隐藏");
        assert_eq!(frames, HIDE_DEBOUNCE_FRAMES);

        // 去抖中途回来:清零并保持显示
        let (visible, frames) = step(true, HIDE_DEBOUNCE_FRAMES - 1, inside, zone, nav);
        assert_eq!((visible, frames), (true, 0));

        // 已隐藏后指针停在驻位矩形(列不在屏,NOTHING):不凭空唤出
        let (visible, _) = step(
            false,
            HIDE_DEBOUNCE_FRAMES,
            Some(egui::pos2(100.0, 300.0)),
            zone,
            egui::Rect::NOTHING,
        );
        assert!(!visible);

        // 长期在外计数饱和(不 panic、不回绕)
        let (_, frames) = step(false, u32::MAX, outside, zone, nav);
        assert_eq!(frames, u32::MAX);
    }

    /// 无头帧冒烟(与 `format_bar` 的模块测试同款:生产入口 `ui` 直接走一
    /// 遭):指针在外零形状零命中,移近左缘当帧唤出并画出标签行,点击行发
    /// [`Message::TabActivate`],列内空白点击不穿透也不发消息。
    #[test]
    fn panel_paints_only_near_left_edge_and_click_activates() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let tabs = TabsState::new("# 禅定");
        fn frame(
            ctx: &egui::Context,
            screen: egui::Rect,
            events: Vec<egui::Event>,
            tabs: &TabsState,
            zen_nav: &mut ZenNavState,
            outbox: &mut Vec<Message>,
        ) -> Vec<egui::epaint::ClippedShape> {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| super::ui(ui, ZenNavMode::Hover, true, tabs, zen_nav, outbox),
            );
            let shapes = output.shapes.clone();
            output.drop_without_applying_deltas();
            shapes
        }
        let mut zen_nav = ZenNavState::default();
        let mut outbox = Vec::new();

        // 指针在外:零形状(零命中面)。
        let shapes = frame(
            &ctx,
            screen,
            vec![egui::Event::PointerMoved(egui::pos2(900.0, 400.0))],
            &tabs,
            &mut zen_nav,
            &mut outbox,
        );
        assert!(shapes.is_empty(), "全隐帧零形状:{shapes:?}");
        assert!(!zen_nav.visible);

        // 移近左缘:当帧唤出,标签行画出来(未命名标签 + 标题栏让位后的列顶)。
        let shapes = frame(
            &ctx,
            screen,
            vec![egui::Event::PointerMoved(egui::pos2(8.0, 400.0))],
            &tabs,
            &mut zen_nav,
            &mut outbox,
        );
        assert!(zen_nav.visible);
        let painted: Vec<String> = shapes
            .iter()
            .filter_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                Some(text.galley.job.text.clone())
            })
            .collect();
        assert!(
            painted.iter().any(|t| t.contains("Untitled")),
            "唤出帧画出标签行:{painted:?}"
        );

        // 点击第一行:发 TabActivate(0);命中前需一帧 sizing pass。
        let nav = nav_rect(screen, tokens::TITLEBAR_H);
        let row0 = row_rect(nav, 0).center();
        let click = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(
            &ctx,
            screen,
            vec![egui::Event::PointerMoved(row0)],
            &tabs,
            &mut zen_nav,
            &mut outbox,
        );
        frame(
            &ctx,
            screen,
            vec![click(row0, true)],
            &tabs,
            &mut zen_nav,
            &mut outbox,
        );
        frame(
            &ctx,
            screen,
            vec![click(row0, false)],
            &tabs,
            &mut zen_nav,
            &mut outbox,
        );
        assert_eq!(outbox, vec![Message::TabActivate(0)]);

        // 列内空白(行下方)点击:不穿透也不发消息(outbox 不增长)。
        let blank = egui::pos2(nav.center().x, nav.bottom() - 20.0);
        frame(
            &ctx,
            screen,
            vec![egui::Event::PointerMoved(blank)],
            &tabs,
            &mut zen_nav,
            &mut outbox,
        );
        frame(
            &ctx,
            screen,
            vec![click(blank, true)],
            &tabs,
            &mut zen_nav,
            &mut outbox,
        );
        frame(
            &ctx,
            screen,
            vec![click(blank, false)],
            &tabs,
            &mut zen_nav,
            &mut outbox,
        );
        assert_eq!(outbox, vec![Message::TabActivate(0)], "仅点击导航行生效");
    }

    /// 三态(#57 M2):常显无视指针几何 —— 指针在外、乃至无指针(窗口
    /// 失焦)都当帧唤出并画出标签行(动画器首调直落端点,无淡入闪烁);
    /// 关闭零路径 —— 指针贴在感应区也不读不画(零形状、零探针、会话态
    /// 保持出厂、零消息)。悬停态对同一指针序列的唤出由
    /// `panel_paints_only_near_left_edge_and_click_activates` 覆盖,三处
    /// 合起来构成「同一指针输入下三态行为互异」的非恒真矩阵。
    #[test]
    fn always_mode_ignores_pointer_and_off_mode_is_zero_path() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let tabs = TabsState::new("# 禅定");

        // 常显:指针在外 → 当帧唤出;无指针 → 同样显示。
        for events in [
            vec![egui::Event::PointerMoved(egui::pos2(900.0, 400.0))],
            Vec::new(),
        ] {
            let ctx = egui::Context::default();
            let mut zen_nav = ZenNavState::default();
            let mut outbox = Vec::new();
            let mut hits = 0u32;
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| {
                    super::ui_with_probe(
                        ui,
                        ZenNavMode::Always,
                        true,
                        &tabs,
                        &mut zen_nav,
                        &mut outbox,
                        Some(|_| hits += 1),
                    );
                },
            );
            let shapes = output.shapes.clone();
            output.drop_without_applying_deltas();
            assert_eq!(hits, 1, "常显当帧即画");
            assert!(zen_nav.visible);
            assert_eq!(zen_nav.outside_frames, 0, "常显不走去抖路径");
            let painted: Vec<String> = shapes
                .iter()
                .filter_map(|clipped| {
                    let egui::epaint::Shape::Text(text) = &clipped.shape else {
                        return None;
                    };
                    Some(text.galley.job.text.clone())
                })
                .collect();
            assert!(
                painted.iter().any(|t| t.contains("Untitled")),
                "常显画出标签行:{painted:?}"
            );
        }

        // 关闭:指针贴在感应区,零形状零探针零推进。
        let ctx = egui::Context::default();
        let mut zen_nav = ZenNavState::default();
        let mut outbox = Vec::new();
        let mut hits = 0u32;
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                events: vec![egui::Event::PointerMoved(egui::pos2(8.0, 400.0))],
                ..Default::default()
            },
            |ui| {
                super::ui_with_probe(
                    ui,
                    ZenNavMode::Off,
                    true,
                    &tabs,
                    &mut zen_nav,
                    &mut outbox,
                    Some(|_| hits += 1),
                );
            },
        );
        assert!(output.shapes.is_empty(), "关闭档零形状");
        output.drop_without_applying_deltas();
        assert_eq!(hits, 0, "零导航元素(否决线)");
        assert_eq!(zen_nav, ZenNavState::default(), "零路径:不推进任何判定");
        assert!(outbox.is_empty());
    }
}
