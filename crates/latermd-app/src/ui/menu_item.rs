//! 弹出菜单项按钮(自绘)。
//!
//! 为什么不用 `egui::Button`:右键菜单/下拉菜单的条目此前都是裸
//! `ui.button(...)`,egui 的 Button 把**行盒**(链头 Inter 的 16px 度量)
//! 在行内做几何居中,而 CJK 字形的**实墨**在行盒内本就偏上 —— 真机逐行
//! 量像素(2026-10-15,坤哥反馈「hover 的灰影不居中」;两张用户截图与本机
//! 隔离实例交叉证实,紧凑密度行高 25.2px):
//!
//! | 来源 | 行(hover 矩形) | 实墨 | 上留白 / 下留白 |
//! |---|---|---|---|
//! | 坤哥截图 1(900×600) | 26px | 13px | 4 / 9 |
//! | 坤哥截图 2(863×586) | 25px | 13px | 2 / 10(抗锯齿口径) |
//! | 本机 release 二进制(0.0.5 后) | 25px | 14px | 3 / 8 |
//!
//! 根因与 `font_metrics_repro`(#43 M1 取证)同源但方向相反:行盒
//! 按 Inter ascent(0.969em)落基线,CJK 表意字形墨迹占「基线上 ~0.9em、
//! 基线下 ~0」(字形 quad 底部的降部衬垫对汉字是空的),墨迹中心因此比
//! 行盒中心高 ~2.5px;行高(25.2px)与行盒(16px)之差被对称平分,
//! 「几何居中」于是观感仍偏上 —— 与 #248 弹窗输入框「几何居中不够」
//! 同一类问题。**无头测量会漏掉它**:`Galley::mesh_bounds` 是字形 quad
//! 顶点包围盒,quad 上下对称(5.6/5.6)而实墨偏上 —— 真机栅格化像素
//! 才是观感判据(#248 同款教训)。
//!
//! egui 的 Button 没有垂直微调 API(#248 已验证,`Atom`/`Button` 均无),
//! 故照 #248 的补偿思路自绘:行盒仍由本组件按同一套 spacing 规则分配
//! (行高、内边距、hover/active 底色、圆角、expansion 全部走
//! `visuals.widgets` 出厂值,观感与 egui Button 一致),只把 galley 相对
//! 行中心下移 `MENU_INK_DY`。
//!
//! 适用边界:纯文本条目(全项目右键/下拉菜单的 CJK 标签)。带快捷键
//! 尾注的行(编辑菜单编辑组、menubar 下拉)混排拉丁字形,拉丁墨迹
//! (大写字母/数字)在行盒内的偏心与 CJK 相反,实测合成偏心 ≈ −0.6px
//! (2026-10-15 release 二进制 Alt+F 下拉逐行量),维持 egui Button。

use eframe::egui;

/// 菜单标签 galley 相对行中心的**下移补偿**(px)。
///
/// 取 2.5:真机三源实测偏心 −2.1 ~ −3.5(上表),取中位;补偿后
/// 「重命名」行实测上/下留白 5.5 / 5.5(±0.6 为字形差异:打开/重命名/
/// 删除各自墨迹包络略有出入,原生菜单同样存在)。
///
/// **依赖前提**:Body 13px、行高 = 链头 Inter 行盒 16px、密度档不改
/// 字号。改字号或字体栈都要真机重量(`import` 截窗口逐行量像素,不要
/// 信无头 mesh —— 见模块注释)。
const MENU_INK_DY: f32 = 2.5;

/// 弹出菜单里的纯文本条目(可点)。
pub(crate) fn button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    item(ui, text, true)
}

/// 弹出菜单里的纯文本条目,可禁用(禁用时只悬停不可点,弱色文字)。
pub(crate) fn button_enabled(ui: &mut egui::Ui, enabled: bool, text: &str) -> egui::Response {
    item(ui, text, enabled)
}

fn item(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let response = ui
        .add_enabled_ui(enabled, |ui| {
            let font = egui::TextStyle::Body.resolve(ui.style());
            let color = ui.visuals().widgets.inactive.text_color();
            let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
            // 与 egui Button 同一套尺寸规则(menu_style 把 button_padding 收成
            // (2,0),行高下限 = interact_size.y):行高不变,只动文字纵向位置。
            let pad = ui.style().spacing.button_padding;
            let size = egui::vec2(
                galley.size().x + 2.0 * pad.x,
                (galley.size().y + 2.0 * pad.y).max(ui.spacing().interact_size.y),
            );
            let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
            if ui.is_rect_visible(rect) {
                // 视觉照抄 egui ButtonStyle:底色取 weak_bg_fill(menu_style 已把
                // inactive 做成透明、hover/active 用主题投影值),随 state 的
                // expansion 外扩、圆角随 state;禁用态走 Noninteractive
                // (weak_bg 透明,不画底,文字由 disable() 的透明度压灰)。
                let visuals = ui.style().interact(&response);
                if ui.is_enabled() && visuals.weak_bg_fill != egui::Color32::TRANSPARENT {
                    ui.painter().rect_filled(
                        rect.expand(visuals.expansion),
                        visuals.corner_radius,
                        visuals.weak_bg_fill,
                    );
                }
                let galley_pos = egui::pos2(
                    rect.left() + pad.x,
                    rect.center().y - galley.size().y / 2.0 + MENU_INK_DY,
                );
                ui.painter().galley(galley_pos, galley, color);
            }
            response
        })
        .inner;
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, RawInput, Rect, Vec2};

    fn probe_rect_id() -> egui::Id {
        egui::Id::new("menu-item-probe-rect")
    }

    /// 走真实 popup 管线(`Response::context_menu` → egui menu_style)渲染
    /// 一枚菜单项并 hover 它,返回(galley 屏幕位置与尺寸, hover 底色矩形,
    /// 条目响应矩形)。
    fn render_hovered_item(
        ctx: &egui::Context,
        label: &str,
    ) -> (egui::Pos2, egui::Vec2, Option<egui::Rect>, egui::Rect) {
        let pos = egui::pos2(100.0, 260.0);
        let frame = |events: Vec<Event>, t: f64| {
            let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(900.0, 600.0));
            let output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    time: Some(t),
                    ..Default::default()
                },
                |ui| {
                    // 生产 shell 投影的几何项(apply_shell_to 私有,按 tokens
                    // 复刻)× 出厂默认紧凑密度(apply_density ×0.7)。
                    for theme in [egui::Theme::Light, egui::Theme::Dark] {
                        ctx.style_mut_of(theme, |style| {
                            style.spacing.interact_size = egui::vec2(36.0, 36.0) * 0.7;
                            style.spacing.button_padding = egui::vec2(12.0, 8.0) * 0.7;
                            style.spacing.item_spacing = egui::vec2(8.0, 3.0) * 0.7;
                        });
                    }
                    egui::CentralPanel::default().show(ui, |ui| {
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(
                            Rect::from_min_size(egui::pos2(40.0, 240.0), Vec2::new(200.0, 40.0)),
                        ));
                        let row = child.selectable_label(false, "AGENTS.md");
                        row.context_menu(|ui| {
                            let response = button(ui, label);
                            ctx.data_mut(|d| {
                                d.insert_temp(probe_rect_id(), response.rect);
                            });
                        });
                    });
                },
            );
            let shapes = output.shapes.clone();
            output.drop_without_applying_deltas();
            shapes
        };
        let _ = frame(vec![Event::PointerMoved(pos)], 0.0);
        // 右键打开弹层
        let _ = frame(
            vec![Event::PointerButton {
                pos,
                button: PointerButton::Secondary,
                pressed: true,
                modifiers: Default::default(),
            }],
            0.1,
        );
        let _ = frame(
            vec![Event::PointerButton {
                pos,
                button: PointerButton::Secondary,
                pressed: false,
                modifiers: Default::default(),
            }],
            0.2,
        );
        // 指针移到条目中心(hover 态)。弹层首个 sizing pass 会翻转弹出
        // 方向,矩形要逐帧重读,连移 3 帧等位置收敛。
        let mut shapes = Vec::new();
        for t in 3..=5 {
            let t = f64::from(t) * 0.1;
            let item_rect = ctx
                .data(|d| d.get_temp::<egui::Rect>(probe_rect_id()))
                .expect("菜单打开后条目矩形应已记录");
            shapes = frame(vec![Event::PointerMoved(item_rect.center())], t);
        }

        let rect = ctx
            .data(|d| d.get_temp::<egui::Rect>(probe_rect_id()))
            .expect("条目矩形应存在");
        let mut galley: Option<(egui::Pos2, egui::Vec2)> = None;
        let mut hover_bg: Option<egui::Rect> = None;
        for s in &shapes {
            match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text == label => {
                    galley = Some((t.pos, t.galley.size()));
                }
                // hover 底色:与条目行相交的**行尺寸级**实心矩形(menu_style
                // 下 inactive 透明,只有 hover/active 画底;排除面板/弹层
                // 全幅底色);底色随 state expansion(默认 1.0)四周外扩。
                egui::Shape::Rect(r)
                    if r.fill != egui::Color32::TRANSPARENT
                        && r.rect.intersects(rect)
                        && (r.rect.height() - rect.height()).abs() <= 2.6
                        && r.rect.width() <= rect.width() + 2.6 =>
                {
                    hover_bg = Some(r.rect);
                }
                _ => {}
            }
        }
        let (pos, size) = galley.expect("菜单标签 galley 应已绘制");
        (pos, size, hover_bg, rect)
    }

    /// 守门:菜单标签的**行盒**相对行中心下移 `MENU_INK_DY` —— 行盒几何
    /// 居中的偏上观感(模块注释的三源取证)由此修正。
    ///
    /// 断言的是**放置契约**(galley 位置公式)而非栅格化墨迹:无头拿不到
    /// 真实栅格化像素(#248 教训,mesh/atlas 都与实墨有系统偏差),真机
    /// 观感由 X11 截图逐行量像素把守(修复前后:3/8 → ≈5.5/5.5)。
    /// 任何把补偿改没(置 0)或改方向的改动在此红。
    #[test]
    fn menu_item_galley_sits_ink_dy_below_row_center() {
        // 补偿量本身钉在实测区间(三源取证 −2.1 ~ −3.5 → 取 2.5):置 0 或
        // 改出区间都是红 —— 断言放置公式与常量挂钩,常量归零时公式断言
        // 会自我满足(0 == 0),这条区间断言才是真正的红绿锚。
        assert!(
            (1.0..=4.0).contains(&MENU_INK_DY),
            "MENU_INK_DY = {MENU_INK_DY} 落出实测区间 [1.0, 4.0] ——              补偿被移除/改写,必须重新真机量像素再改"
        );
        let ctx = egui::Context::default();
        let _cjk = crate::fonts::install(&ctx);
        let (galley_pos, galley_size, hover_bg, rect) = render_hovered_item(&ctx, "重命名");

        // hover 态必须真的画了底(前置:指针确实落在条目上)。
        let hover_rect = hover_bg.expect("hover 态应绘制条目底色");
        assert!(
            hover_rect.intersects(rect) && (hover_rect.height() - rect.height()).abs() <= 2.0,
            "hover 底色矩形 {:?} 应与条目矩形 {:?} 同行",
            hover_rect,
            rect
        );
        // 行高口径不变:仍为 interact_size.y(紧凑 25.2),不是自创高度。
        assert!(
            (hover_rect.height() - 36.0_f32 * 0.7).abs() < 0.6,
            "菜单行高 {:.1} 应保持 interact_size.y×密度(25.2)—— 行高改变会连带改变补偿量的观感",
            hover_rect.height()
        );
        // 放置契约:galley 顶 = 行中心 − 行盒高/2 + MENU_INK_DY。
        let geometric_top = hover_rect.center().y - galley_size.y / 2.0;
        assert!(
            (galley_pos.y - geometric_top - MENU_INK_DY).abs() <= 0.05,
            "菜单标签 galley 顶 {:.2} 应等于几何居中位 {:.2} + MENU_INK_DY({:.1}),\
             实测偏差 {:+.2} —— 补偿被移除/改向/行盒规则变了",
            galley_pos.y,
            geometric_top,
            MENU_INK_DY,
            galley_pos.y - geometric_top - MENU_INK_DY
        );
        // galley 仍在行内(下移后不越行底):留白 ≥ 2px。
        let bottom_gap = hover_rect.bottom() - (galley_pos.y + galley_size.y);
        assert!(
            bottom_gap >= 2.0,
            "galley 底部距行底 {:.1}px < 2px —— 补偿过大,文字贴行底",
            bottom_gap
        );
    }

    /// 禁用条目:不画底色、响应不可点(只悬停)。
    #[test]
    fn menu_item_disabled_has_no_background_and_no_click() {
        let ctx = egui::Context::default();
        let _cjk = crate::fonts::install(&ctx);
        let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(900.0, 600.0));
        let mut clicked = false;
        for _ in 0..3 {
            let output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    events: vec![Event::PointerButton {
                        pos: egui::pos2(120.0, 265.0),
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    }],
                    time: Some(0.1),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(
                            Rect::from_min_size(egui::pos2(40.0, 240.0), Vec2::new(200.0, 40.0)),
                        ));
                        // 条目必须画在 child 里,点击坐标 (120,265) 才会命中
                        // —— 「禁用不可点」的断言因此非空证。
                        let response = button_enabled(&mut child, false, "打开");
                        ctx.data_mut(|d| {
                            d.insert_temp(probe_rect_id(), response.rect);
                        });
                        if response.clicked() {
                            clicked = true;
                        }
                    });
                },
            );
            // 只查条目矩形位置的底色(面板/弹层自身底色不算)。
            let item = ctx.data(|d| d.get_temp::<Rect>(probe_rect_id()));
            let bg = item.is_some_and(|item| {
                output.shapes.iter().any(|s| {
                    matches!(
                        &s.shape,
                        egui::Shape::Rect(r) if r.fill != egui::Color32::TRANSPARENT
                            && r.rect.intersects(item)
                            && (r.rect.height() - item.height()).abs() <= 2.0
                    )
                })
            });
            output.drop_without_applying_deltas();
            assert!(!bg, "禁用条目不应绘制底色");
        }
        assert!(!clicked, "禁用条目不应响应点击");
    }
}
