//! Compact, grouped preferences using the existing settings messages.
use super::*;
use crate::ui::workbench::{centered_label, label_at, separator};
use egui::{Color32, Rect, Sense};

pub(super) fn style(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 28.0;
    style.spacing.extra_text_line_spacing = 0.0;
    style.spacing.slider_width = 140.0;
    style.spacing.slider_rail_height = 4.0;
    style.visuals.handle_shape = egui::style::HandleShape::Circle;
    for text in [egui::TextStyle::Body, egui::TextStyle::Button] {
        style
            .text_styles
            .insert(text, egui::FontId::proportional(13.0));
    }
    style.text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::new(18.0, egui::FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(11.0));
    for widget in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
    ] {
        widget.corner_radius = 6.into();
    }
}

pub(super) fn navigation(ui: &mut egui::Ui, tab: SettingsTab, selected: bool) -> egui::Response {
    let colors = crate::theme::shell(ui);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            6.0,
            if selected {
                colors.selected_bg
            } else {
                colors.hover
            },
        );
    }
    tab.icon().draw(
        ui.painter(),
        egui::pos2(rect.left() + 17.0, rect.center().y),
        15.0,
        if selected {
            colors.accent
        } else {
            colors.secondary
        },
    );
    label_at(
        ui,
        rect.left() + 34.0,
        rect.center().y,
        tab.label(),
        13.0,
        colors.text,
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            selected,
            tab.label(),
        )
    });
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(title)
            .size(12.0)
            .family(crate::fonts::semibold_family(ui.ctx())),
    );
    let shell = crate::theme::shell(ui);
    egui::Frame::NONE
        .fill(crate::theme::shell(ui).chrome)
        .stroke(egui::Stroke::new(
            1.0 / ui.ctx().pixels_per_point(),
            shell.border,
        ))
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add(ui);
        });
}

fn rule(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .line_segment([r.left_center(), r.right_center()], separator(ui));
}

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(92.0, 28.0), Sense::hover());
        label_at(
            ui,
            rect.left(),
            rect.center().y,
            label,
            13.0,
            ui.visuals().text_color(),
        );
        add(ui);
    });
}

fn segments(ui: &mut egui::Ui, labels: &[&str], selected: usize) -> Option<usize> {
    let colors = crate::theme::shell(ui);
    let width = (ui.available_width() / labels.len() as f32).clamp(48.0, 82.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(width * labels.len() as f32, 28.0),
        Sense::hover(),
    );
    ui.painter().rect_filled(rect, 6.0, colors.hover);
    let mut chosen = None;
    for (index, label) in labels.iter().enumerate() {
        let part = Rect::from_min_size(
            rect.min + egui::vec2(width * index as f32, 0.0),
            egui::vec2(width, 28.0),
        )
        .shrink(2.0);
        let response = ui.interact(part, ui.id().with(("segment", label)), Sense::click());
        if selected == index || response.has_focus() {
            ui.painter().rect_filled(part, 4.0, colors.content);
            ui.painter()
                .rect_stroke(part, 4.0, separator(ui), egui::StrokeKind::Inside);
        }
        centered_label(
            ui,
            part,
            label,
            if selected == index {
                colors.text
            } else {
                colors.secondary
            },
        );
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                true,
                selected == index,
                *label,
            )
        });
        if response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
            && selected != index
        {
            chosen = Some(index);
        }
    }
    chosen
}

fn theme_choices(ui: &mut egui::Ui, theme: &ThemeSettings, outbox: &mut Vec<Message>) {
    let shell = crate::theme::shell(ui);
    ui.horizontal(|ui| {
        for mode in ThemeMode::ALL {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(96.0, 80.0), Sense::click());
            let selected = mode == theme.mode;
            let thumb =
                Rect::from_min_size(rect.min + egui::vec2(2.0, 2.0), egui::vec2(92.0, 54.0));
            let dark = mode == ThemeMode::Dark;
            let preview = theme.shell_palette().colors(dark);
            let paper = preview.content;
            ui.painter().rect_filled(thumb, 6.0, paper);
            let side =
                Rect::from_min_max(thumb.min, egui::pos2(thumb.left() + 25.0, thumb.bottom()));
            ui.painter().rect_filled(side, 5.0, preview.sidebar);
            if mode == ThemeMode::System {
                ui.painter().rect_filled(
                    Rect::from_min_max(thumb.center_top(), thumb.max),
                    5.0,
                    theme.shell_palette().dark.content,
                );
            }
            for (line, width) in [(0.0, 37.0), (1.0, 46.0), (2.0, 30.0)] {
                let pos = thumb.min + egui::vec2(33.0, 17.0 + line * 8.0);
                ui.painter().rect_filled(
                    Rect::from_min_size(pos, egui::vec2(width, 2.0)),
                    1.0,
                    preview.secondary,
                );
            }
            ui.painter().rect_stroke(
                thumb,
                6.0,
                if selected {
                    egui::Stroke::new(2.0, shell.accent)
                } else {
                    separator(ui)
                },
                egui::StrokeKind::Outside,
            );
            let label = Rect::from_min_max(egui::pos2(rect.left(), rect.top() + 58.0), rect.max);
            centered_label(
                ui,
                label,
                mode.label(),
                if selected {
                    shell.text
                } else {
                    shell.secondary
                },
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    true,
                    selected,
                    mode.label(),
                )
            });
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
                && !selected
            {
                outbox.push(Message::ThemeChanged(mode));
            }
        }
    });
}

fn toggle(ui: &mut egui::Ui, label: &str, help: &str, value: bool) -> bool {
    let colors = crate::theme::shell(ui);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 43.0), Sense::click());
    label_at(ui, rect.left(), rect.top() + 12.0, label, 13.0, colors.text);
    label_at(
        ui,
        rect.left(),
        rect.top() + 32.0,
        help,
        11.0,
        colors.secondary,
    );
    let switch = Rect::from_center_size(
        egui::pos2(rect.right() - 17.0, rect.center().y),
        egui::vec2(30.0, 18.0),
    );
    ui.painter().rect_filled(
        switch,
        9.0,
        if value { colors.accent } else { colors.hover },
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            switch.expand(2.0),
            10.0,
            egui::Stroke::new(1.0, colors.accent),
            egui::StrokeKind::Outside,
        );
    }
    ui.painter().circle_filled(
        egui::pos2(
            if value {
                switch.right() - 9.0
            } else {
                switch.left() + 9.0
            },
            switch.center().y,
        ),
        7.0,
        Color32::WHITE,
    );
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, value, label));
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

pub(super) fn appearance(
    ui: &mut egui::Ui,
    settings: &mut SettingsState,
    theme: &ThemeSettings,
    skins: &SkinCatalog,
    system_theme_ok: bool,
    resolved: ThemeMode,
    outbox: &mut Vec<Message>,
) {
    ui.heading("外观");
    ui.label(
        egui::RichText::new("让工作台适合你的阅读与写作习惯。")
            .size(12.0)
            .weak(),
    );
    section(ui, "工作台皮肤", |ui| {
        ui.small("窗口、侧栏、编辑区、控件与文档统一换肤。");
        skins::choices(ui, theme, skins, outbox);
    });
    section(ui, "主题", |ui| {
        theme_choices(ui, theme, outbox);
        if theme.mode == ThemeMode::System {
            ui.small(if system_theme_ok {
                format!("跟随系统 · 当前为{}", resolved.label())
            } else {
                "无法读取系统主题，使用上次选择。".into()
            });
        }
        rule(ui);
        row(ui, "界面密度", |ui| {
            if let Some(i) = segments(
                ui,
                &["宽松", "标准"],
                if theme.density == Density::Standard {
                    0
                } else {
                    1
                },
            ) {
                outbox.push(Message::ThemeDensityChanged(if i == 0 {
                    Density::Standard
                } else {
                    Density::Compact
                }));
            }
        });
    });
    section(ui, "文字与排版", |ui| {
        ui.small("界面：San Francisco / 苹方 · 源码：SF Mono");
        rule(ui);
        row(ui, "字号", |ui| {
            let mut value = theme.editor_font_size;
            if ui
                .add(
                    egui::Slider::new(&mut value, EDITOR_FONT_SIZE_MIN..=EDITOR_FONT_SIZE_MAX)
                        .integer()
                        .suffix(" pt"),
                )
                .changed()
            {
                outbox.push(Message::EditorFontSizeChanged(value));
            }
        });
        row(ui, "行距", |ui| {
            let mut value = theme.line_height;
            if ui
                .add(
                    egui::Slider::new(&mut value, LINE_HEIGHT_MIN..=LINE_HEIGHT_MAX)
                        .fixed_decimals(1)
                        .step_by(0.1)
                        .suffix(" 倍"),
                )
                .changed()
            {
                outbox.push(Message::EditorLineHeightChanged(value));
            }
        });
    });
    section(ui, "写作", |ui| {
        if toggle(
            ui,
            "缩略导航",
            "在源码右侧显示文档缩略图",
            theme.show_minimap,
        ) {
            outbox.push(Message::ShowMinimapToggled(!theme.show_minimap));
        }
        ui.add_enabled_ui(theme.show_minimap, |ui| {
            if toggle(
                ui,
                "自动适应",
                "短文或窄栏时收起缩略图，给正文更多空间",
                theme.minimap_auto,
            ) {
                outbox.push(Message::MinimapAutoChanged(!theme.minimap_auto));
            }
        });
        rule(ui);
        if toggle(
            ui,
            "打字机模式",
            "让光标行保持在视口上方三分之一处",
            theme.show_typewriter,
        ) {
            outbox.push(Message::TypewriterToggled(!theme.show_typewriter));
        }
        rule(ui);
        if toggle(
            ui,
            "专注模式",
            "在 Live 中淡化其他段落",
            theme.show_focus_mode,
        ) {
            outbox.push(Message::FocusModeToggled(!theme.show_focus_mode));
        }
        rule(ui);
        if toggle(
            ui,
            "选区 AI 浮标",
            "选中文字后浮出「AI 续写 / AI 润色」入口",
            theme.show_selection_ai,
        ) {
            outbox.push(Message::SelectionAiToggled(!theme.show_selection_ai));
        }
        rule(ui);
        row(ui, "禅定导航", |ui| {
            let modes = ZenNavMode::ALL;
            let labels: Vec<_> = modes.iter().map(|mode| mode.label()).collect();
            if let Some(i) = segments(
                ui,
                &labels,
                modes.iter().position(|m| *m == theme.zen_nav).unwrap_or(0),
            ) {
                outbox.push(Message::ZenNavModeChanged(modes[i]));
            }
        });
        rule(ui);
        // 左栏外壳样式(mac 专项,2026-10-24 坤哥指令「左侧文件导航跟别的
        // 平台不一致,弄一个配置项」):工作台 = 顶部工作区下拉 + 文字页签
        // (mac 出厂默认,升级零变化);图标栏 = Windows/Linux 同款左缘
        // rail。切完即生效,下一帧 `ui::layout::draw` 按新值换外壳。
        row(ui, "侧栏外壳", |ui| {
            let styles = crate::theme::SidebarStyle::ALL;
            let labels: Vec<_> = styles.iter().map(|style| style.label()).collect();
            if let Some(i) = segments(
                ui,
                &labels,
                styles
                    .iter()
                    .position(|s| *s == theme.sidebar_style)
                    .unwrap_or(0),
            ) {
                outbox.push(Message::SidebarStyleChanged(styles[i]));
            }
        });
        ui.small("工作台=顶部工作区下拉+文字页签;图标栏=左缘图标轨道(其他平台同款)。");
    });
    section(ui, "分享皮肤", |ui| {
        row(ui, "导出皮肤", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut settings.skin_export_name)
                    .hint_text("皮肤名称")
                    .desired_width(130.0),
            );
            let name = settings.skin_export_name.trim();
            if ui
                .add_enabled(!name.is_empty(), egui::Button::new("导出"))
                .clicked()
            {
                outbox.push(Message::ThemeSkinExported {
                    name: name.to_owned(),
                });
            }
        });
    });
    ui.add_space(8.0);
    ui.small(format!(
        "渲染后端 · {}",
        crate::renderer_label(std::env::var("LATERMD_RENDERER").ok().as_deref())
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouped_preferences_keep_theme_and_writing_controls_connected() {
        for dark in [false, true] {
            let ctx = egui::Context::default();
            ctx.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            let mut state = crate::state::State::default();
            state.theme.mode = ThemeMode::System;
            state.theme.show_minimap = false;
            let mut frame = |events| {
                let mut messages = Vec::new();
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(560.0, 1200.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        style(ui);
                        appearance(
                            ui,
                            &mut state.settings,
                            &state.theme,
                            &state.skins,
                            true,
                            ThemeMode::Dark,
                            &mut messages,
                        );
                    },
                );
                let text_positions: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|s| match &s.shape {
                        egui::Shape::Text(text) => Some((
                            text.galley.text().to_owned(),
                            text.pos + text.galley.size() / 2.0,
                        )),
                        _ => None,
                    })
                    .collect();
                output.textures_delta.clear();
                (text_positions, messages)
            };
            let (texts, messages) = frame(vec![]);
            assert!(messages.is_empty());
            for (label, expected) in [
                ("深色", Message::ThemeChanged(ThemeMode::Dark)),
                ("缩略导航", Message::ShowMinimapToggled(true)),
                ("打字机模式", Message::TypewriterToggled(true)),
                ("专注模式", Message::FocusModeToggled(true)),
            ] {
                let pos = texts.iter().find(|(text, _)| text == label).unwrap().1;
                let (_, messages) = frame(vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
                assert_eq!(
                    messages,
                    vec![expected],
                    "{label} must use its existing settings message"
                );
            }
        }
    }
}
