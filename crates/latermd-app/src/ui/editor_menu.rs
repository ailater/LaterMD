//! 编辑器右键菜单条目表(源码模式与 Live 活动块共用一份,
//! context-menu-plan §3 M1)。
//!
//! 条目全部映射既有通道,零新增动作系统:编辑组走 `edit_events` 合成
//! 事件(macOS 原生菜单同源);格式组复用 `Command::FormatXxx` →
//! `menubar::item`(键位标注与菜单栏同源);文档组是普通命令;AI 组
//! 复用选区浮标(`selection_ai`)的请求消息。快捷键显示用 `keymap`
//! 实际绑定 —— 用户改过键,右键菜单跟着变,与菜单栏一致。
//!
//! 条目取舍(规划 §1「明确不做」):不塞全量 44 条命令 —— 右键菜单的
//! 价值是高频短路径,全量检索由菜单栏与快速打开承担。

use eframe::egui;

use crate::command::Command;
use crate::edit_events::EditAction;
use crate::keymap::Keymap;
use crate::state::{Message, SelectionAiAction};

/// 格式子菜单的次级条目(H1-H3 到任务列表,加两个对话框类插入入口)。
/// 前五条(加粗/斜体/删除线/行内代码/链接)直接摆在一级,详见 [`build`]。
const FORMAT_SECONDARY: [Command; 12] = [
    Command::FormatH1,
    Command::FormatH2,
    Command::FormatH3,
    Command::FormatQuote,
    Command::FormatCodeBlock,
    Command::FormatDivider,
    Command::FormatTable,
    Command::FormatBullet,
    Command::FormatOrdered,
    Command::FormatTask,
    Command::ImageInsert,
    Command::EmojiPicker,
];

/// 文档组:不作用选区的文档级辅助动作,与菜单栏「编辑」组同族。
const DOC_ITEMS: [Command; 4] = [
    Command::DuplicateLine,
    Command::FindInDoc,
    Command::ReplaceInDoc,
    Command::GotoLine,
];

/// 构建菜单内容。`selection` 是当前选区(字符区间,None = 无);
/// `live` 为真时是 Live 活动块菜单 —— 插入图片/Emoji 两项禁用(它们的
/// 归约路径是整篇 `replace_all` + `pending_selection`,Live 不消费
/// `pending_selection`,写回会在切回源码时惊扰光标;块内定点插入留给
/// 后续)。
pub(crate) fn build(
    ui: &mut egui::Ui,
    keymap: &Keymap,
    selection: Option<(usize, usize)>,
    live: bool,
    outbox: &mut Vec<Message>,
) {
    let has_selection = selection.is_some_and(|(a, b)| a != b);

    // 编辑组:撤销/重做/剪切/复制/粘贴/全选。剪切/复制无选区时禁用;
    // 其余恒可用(undoer 无公开 peek,无步可退时点击无害)。
    for action in EditAction::ALL {
        let enabled = match action {
            EditAction::Cut | EditAction::Copy => has_selection,
            EditAction::Undo | EditAction::Redo | EditAction::Paste | EditAction::SelectAll => true,
        };
        let mut button = egui::Button::new(action.label())
            .shortcut_text(ui.ctx().format_shortcut(&action.shortcut()));
        if !enabled {
            button = button.sense(egui::Sense::hover());
        }
        let response = ui.add_enabled(enabled, button);
        if response.clicked() {
            outbox.push(Message::EditAction(action));
        }
    }

    ui.separator();

    // 格式组:高频五条直出,其余进「更多格式」子菜单(与格式工具条的
    // 「更多」同手法)。菜单渲染在弹层里,子菜单嵌套由 egui 内建。
    for cmd in [
        Command::FormatBold,
        Command::FormatItalic,
        Command::FormatStrike,
        Command::FormatInlineCode,
        Command::FormatLink,
    ] {
        crate::ui::menubar::item(ui, cmd, keymap, None, outbox);
    }
    ui.menu_button("更多格式", |ui| {
        for cmd in FORMAT_SECONDARY {
            let enabled = format_item_enabled(cmd, live);
            if enabled {
                crate::ui::menubar::item(ui, cmd, keymap, None, outbox);
            } else {
                let response = ui.add_enabled(
                    false,
                    egui::Button::new(cmd.label()).shortcut_text(egui::RichText::new("")),
                );
                response.on_disabled_hover_text("Live 模式暂不支持,切回源码模式使用");
            }
        }
    });

    ui.separator();

    // 文档组。
    for cmd in DOC_ITEMS {
        crate::ui::menubar::item(ui, cmd, keymap, None, outbox);
    }

    // AI 组:有选区时才出现(无选区续写/润色无从谈起,选区浮标同口径)。
    // 复用 selection_ai 的请求消息:同一归约入口,浮标与右键菜单两入口
    // 一条通道。
    if let (true, Some(selection)) = (has_selection, selection) {
        ui.separator();
        for action in SelectionAiAction::ALL {
            if ui.button(action.label()).clicked() {
                outbox.push(Message::SelectionAiActionRequested {
                    action,
                    selection: Some(selection),
                });
            }
        }
    }
}

/// TextEdit 响应的右键挂点(源码编辑器与 Live 活动块/空文档块共用,
/// context-menu-plan §3 M1)。
///
/// **先落光标,选区内右键保留选区**(VS Code 口径)。这里有个 egui 的
/// 原生行为必须绕开:`TextEdit` 的 pointer_interaction(text_cursor_state.rs)
/// 对 `any_pressed` 生效、**不分主副键** —— 右键 press 帧它就把选区塌缩
/// 到落点。因此「选区内右键保留选区」不能读本帧 state(已是塌缩值),
/// 要按**上一帧末**的选区快照判定:press/release 两帧各判一次,命中即把
/// 快照选区写回(press 帧当场还原,肉眼无塌缩闪烁),未命中落点保持
/// TextEdit 的原生塌缩。菜单挂在同一响应上(clone,tabs.rs 同手法:
/// context_menu 消费 Response);菜单打开期间闭包逐帧重入,条目可用性
/// 按当前持久 state 实时读。
pub(crate) fn attach(
    ui: &egui::Ui,
    output: &egui::widgets::text_edit::TextEditOutput,
    keymap: &Keymap,
    live: bool,
    outbox: &mut Vec<Message>,
) {
    let response = output.response.response.clone();
    let id = response.id;
    // press 帧:secondary_pressed 且指针在本响应上(release 帧用
    // secondary_clicked);两帧同一段判定,幂等。
    let press_on_me = response.hovered()
        && ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Secondary));
    if press_on_me || response.secondary_clicked() {
        // 不能用 `interact_pointer_pos()`:同帧 press+release 的帧上
        // interact_id 已被 release 清掉,返回 None(实测)。latest_pos 是
        // 本帧指针最后位置,press/release 两帧都取得到。
        if let Some(pos) = ui.input(|input| input.pointer.latest_pos()) {
            let caret = output
                .galley
                .cursor_from_pos(pos - output.galley_pos)
                .index
                .0;
            let prev_key = id.with("ctx-menu-prev-sel");
            let prev: Option<(usize, usize)> = ui
                .ctx()
                .data(|d| d.get_temp::<Option<(usize, usize)>>(prev_key))
                .flatten();
            // 快照按 (primary, secondary) 原样存(two(min,max) 的主在 max),
            // 判定与写回前统一归一化。
            let kept = prev.is_some_and(|(a, b)| {
                let (a, b) = (a.min(b), a.max(b));
                a < caret && caret < b
            });
            if kept {
                let (a, b) = prev.expect("kept 为真则快照在场");
                let (a, b) = (a.min(b), a.max(b));
                // 与 editor.rs `write_selection` 同一写回:持久 state 换
                // 光标 + 请求焦点(press 帧写回即生效,不等下一帧镜像)。
                let mut state = output.state.clone();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(a),
                        egui::text::CCursor::new(b),
                    )));
                state.store(ui.ctx(), id);
            }
            ui.ctx().memory_mut(|mem| mem.request_focus(id));
        }
    }
    // 帧末快照:本帧(含可能的写回)的最终选区是下一帧的判定基准。
    // attach 排在 TextEdit::show 之后,load 拿到的就是本帧最终值。
    let final_sel = egui::widgets::text_edit::TextEditState::load(ui.ctx(), id)
        .and_then(|state| state.cursor.char_range())
        .map(|range| (range.primary.index.0, range.secondary.index.0));
    ui.ctx()
        .data_mut(|d| d.insert_temp(id.with("ctx-menu-prev-sel"), final_sel));

    response.context_menu(|ui| {
        let menu_selection = output
            .state
            .cursor
            .char_range()
            .map(|range| (range.primary.index.0, range.secondary.index.0));
        build(ui, keymap, menu_selection, live, outbox);
    });
}

/// 「更多格式」子菜单条目可用性:Live 模式下图片/Emoji 插入禁用
/// (整篇 replace_all + `pending_selection` 的写回路径与 Live 块内定点
/// 写回不兼容,理由见 [`build`] 文档)。独立成函数便于无头穷举钉住。
pub(crate) fn format_item_enabled(cmd: Command, live: bool) -> bool {
    !(live && matches!(cmd, Command::ImageInsert | Command::EmojiPicker))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, Pos2, RawInput};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// 帧输出里全部文本 galley 的内容(menubar 同款抽取)。
    fn shapes_text(output: &egui::FullOutput) -> String {
        output
            .shapes
            .iter()
            .filter_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                Some(text.galley.job.text.clone())
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 找到文本内容恰为 `label` 的字形(菜单条目名),返回其屏幕中心。
    /// 点击目标取文字本身 —— 按钮矩形包含文字,文字上的点击必落在按钮内。
    fn label_center(output: &egui::FullOutput, label: &str) -> Option<Pos2> {
        output.shapes.iter().find_map(|clipped| {
            let egui::epaint::Shape::Text(text) = &clipped.shape else {
                return None;
            };
            // menubar::item 的条目名带助记后缀「(B)」(mac 不带),整段是
            // 一个 LayoutJob 文本;恰好等于 label 或 label+助记后缀才算命中
            // —— 前缀裸匹配会误中「复制当前行」。
            let job_text = text.galley.job.text.as_str();
            if job_text != label
                && !job_text
                    .strip_prefix(label)
                    .is_some_and(|rest| rest.starts_with('('))
            {
                return None;
            }
            Some(
                text.galley
                    .mesh_bounds
                    .translate(text.pos.to_vec2())
                    .center(),
            )
        })
    }

    /// 三帧点击序列(移动 → 按下 → 释放),与 preview.rs `click_events`
    /// 同手法。
    fn click_frames(pos: Pos2, button: PointerButton) -> Vec<Vec<Event>> {
        let click = |pos, pressed| Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Default::default(),
        };
        vec![
            vec![Event::PointerMoved(pos)],
            vec![click(pos, true)],
            vec![click(pos, false)],
        ]
    }

    fn build_frame(
        ctx: &egui::Context,
        selection: Option<(usize, usize)>,
        live: bool,
        outbox: &Rc<RefCell<Vec<Message>>>,
        events: Vec<Event>,
    ) -> egui::FullOutput {
        let keymap = Keymap::builtin();
        let outbox = outbox.clone();
        ctx.run_ui(
            RawInput {
                events,
                ..Default::default()
            },
            |ui| build(ui, &keymap, selection, live, &mut outbox.borrow_mut()),
        )
    }

    #[test]
    fn edit_group_lists_all_actions_and_format_doc_groups() {
        let ctx = egui::Context::default();
        let outbox = Rc::new(RefCell::new(Vec::new()));
        let output = build_frame(&ctx, Some((0, 3)), false, &outbox, Vec::new());
        let text = shapes_text(&output);
        for action in EditAction::ALL {
            assert!(text.contains(action.label()), "缺 {}", action.label());
        }
        for cmd in [
            Command::FormatBold,
            Command::FormatItalic,
            Command::FormatStrike,
            Command::FormatInlineCode,
            Command::FormatLink,
            Command::DuplicateLine,
            Command::FindInDoc,
            Command::ReplaceInDoc,
            Command::GotoLine,
        ] {
            assert!(text.contains(cmd.label()), "缺 {}", cmd.label());
        }
        assert!(text.contains("更多格式"));
        output.drop_without_applying_deltas();
    }

    #[test]
    fn clicking_copy_and_paste_push_edit_action_messages() {
        let ctx = egui::Context::default();
        let outbox = Rc::new(RefCell::new(Vec::new()));
        let first = build_frame(&ctx, Some((0, 3)), false, &outbox, Vec::new());
        let copy_pos = label_center(&first, "复制").expect("「复制」条目在场");
        let paste_pos = label_center(&first, "粘贴").expect("「粘贴」条目在场");
        first.drop_without_applying_deltas();

        // 复制:三帧点击(移动/按下/释放),消息只经 outbox,事件推送在
        // 归约侧(不在本菜单的职责内)。
        for events in click_frames(copy_pos, PointerButton::Primary) {
            build_frame(&ctx, Some((0, 3)), false, &outbox, events).drop_without_applying_deltas();
        }
        assert_eq!(
            outbox.borrow().last(),
            Some(&Message::EditAction(EditAction::Copy))
        );
        for events in click_frames(paste_pos, PointerButton::Primary) {
            build_frame(&ctx, Some((0, 3)), false, &outbox, events).drop_without_applying_deltas();
        }
        assert_eq!(
            outbox.borrow().last(),
            Some(&Message::EditAction(EditAction::Paste))
        );
    }

    #[test]
    fn cut_and_copy_disabled_without_selection() {
        let ctx = egui::Context::default();
        let outbox = Rc::new(RefCell::new(Vec::new()));
        let first = build_frame(&ctx, None, false, &outbox, Vec::new());
        let copy_pos = label_center(&first, "复制").expect("「复制」条目在场(禁用态仍渲染)");
        first.drop_without_applying_deltas();
        for events in click_frames(copy_pos, PointerButton::Primary) {
            build_frame(&ctx, None, false, &outbox, events).drop_without_applying_deltas();
        }
        assert!(outbox.borrow().is_empty(), "无选区点「复制」不得产出消息");
    }

    #[test]
    fn format_bold_click_pushes_format_requested() {
        let ctx = egui::Context::default();
        let outbox = Rc::new(RefCell::new(Vec::new()));
        let first = build_frame(&ctx, None, false, &outbox, Vec::new());
        let bold_pos = label_center(&first, "加粗").expect("「加粗」直出条目在场");
        first.drop_without_applying_deltas();
        for events in click_frames(bold_pos, PointerButton::Primary) {
            build_frame(&ctx, None, false, &outbox, events).drop_without_applying_deltas();
        }
        assert_eq!(
            outbox.borrow().as_slice(),
            [Message::FormatRequested(crate::compose::FormatAction::Bold)]
        );
    }

    #[test]
    fn ai_rows_only_with_nonempty_selection() {
        let ctx = egui::Context::default();
        let outbox = Rc::new(RefCell::new(Vec::new()));
        // 无选区:AI 组整组不渲染
        let none = build_frame(&ctx, None, false, &outbox, Vec::new());
        assert!(!shapes_text(&none).contains(SelectionAiAction::Continue.label()));
        none.drop_without_applying_deltas();
        // 有选区:AI 组在场,点击发 selection_ai 的既有请求消息
        let some = build_frame(&ctx, Some((1, 4)), false, &outbox, Vec::new());
        let pos = label_center(&some, SelectionAiAction::Continue.label()).expect("AI 续写在场");
        some.drop_without_applying_deltas();
        for events in click_frames(pos, PointerButton::Primary) {
            build_frame(&ctx, Some((1, 4)), false, &outbox, events).drop_without_applying_deltas();
        }
        assert!(matches!(
            outbox.borrow().last(),
            Some(Message::SelectionAiActionRequested { action, selection })
                if *action == SelectionAiAction::Continue && *selection == Some((1, 4))
        ));
    }

    #[test]
    fn live_mode_disables_image_and_emoji_insert() {
        for cmd in FORMAT_SECONDARY {
            assert!(
                format_item_enabled(cmd, false),
                "源码模式全部可用:{}",
                cmd.label()
            );
        }
        for cmd in [Command::ImageInsert, Command::EmojiPicker] {
            assert!(
                !format_item_enabled(cmd, true),
                "Live 模式应禁用:{}",
                cmd.label()
            );
        }
        for cmd in FORMAT_SECONDARY {
            if matches!(cmd, Command::ImageInsert | Command::EmojiPicker) {
                continue;
            }
            assert!(
                format_item_enabled(cmd, true),
                "Live 模式其余格式可用:{}",
                cmd.label()
            );
        }
    }
}
