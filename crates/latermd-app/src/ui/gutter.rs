//! 源码模式行号槽(#31):`TextEdit` 左侧的纯绘制列。
//!
//! 画在 #29 的外层 ScrollArea 闭包内 —— 内容坐标天然随滚动平移,零同步
//! 成本;y 取本帧 `output.galley` 里各**逻辑行**首 visual row(`\n` 之后
//! 的那个 row)的行矩形中心,折行段的行号只标在段首 visual row,不重复;
//! 宽度 = 总行数十进制位数 × 数字宽 + 左右留白,位数跨档才变宽,右对齐
//! 因此不抖。只绘制视口内可见的行:行号是前缀计数,扫描是 O(rows) 的
//! 布尔累计(5000 行也远低于 galley 排版本身的成本),**绘制**严格限定
//! 在 clip rect(即 ScrollArea 视口)命中的行。行号不 Sense 指针、不拦
//! 截交互(「点击行号选整行」未做,取舍见任务 notes)。

use crate::ui::tokens;
use eframe::egui;

/// 槽内左右留白;右缘与编辑器文本之间另由 `item_spacing.x` 提供 token 间距。
const PAD_X: f32 = tokens::SPACE_XS;

/// galley row 是否某逻辑行的首 visual row:首行,或上一 row 以 `\n` 结尾
/// (折行拆出来的续行不是新逻辑行)。
fn is_line_start(galley: &egui::Galley, idx: usize) -> bool {
    idx == 0 || galley.rows[idx - 1].ends_with_newline
}

/// 总行数 → 十进制位数(空文档按 1 行计,至少 1 位)。
fn digit_count(total_lines: usize) -> usize {
    let mut digits = 1;
    let mut n = total_lines.max(1);
    while n >= 10 {
        n /= 10;
        digits += 1;
    }
    digits
}

/// 行号槽宽度:位数 × 数字宽(Monospace 等宽,'0' 即代表)+ 左右留白。
pub(crate) fn width(ui: &egui::Ui, total_lines: usize) -> f32 {
    let font = egui::FontSelection::Style(egui::TextStyle::Monospace).resolve(ui.style());
    let digit = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    digit_count(total_lines) as f32 * digit + 2.0 * PAD_X
}

/// 在 ScrollArea 闭包内、TextEdit 展示之后画行号。
///
/// * `slot`:槽位矩形(布局让位时分配,宽即 [`width`]);
/// * `cursor_line`:光标所在逻辑行(0-based),accent 高亮;`None` 不高亮。
pub(crate) fn paint(
    ui: &egui::Ui,
    output: &egui::widgets::text_edit::TextEditOutput,
    slot: egui::Rect,
    cursor_line: Option<usize>,
) {
    let galley = &output.galley;
    let base = output.galley_pos;
    let clip = ui.clip_rect();
    let plain = ui.visuals().widgets.noninteractive.fg_stroke.color;
    let hot = tokens::accent(ui);
    let font = egui::FontSelection::Style(egui::TextStyle::Monospace).resolve(ui.style());
    let digits_right = slot.right() - PAD_X;
    let painter = ui.painter();

    let mut line = 0usize;
    for idx in 0..galley.rows.len() {
        if !is_line_start(galley, idx) {
            continue;
        }
        line += 1;
        let row = galley.rows[idx].rect();
        let top = base.y + row.top();
        // 视口裁剪:行底还压着视口顶 → 半露仍可见;行顶越过视口底 →
        // 后面的行只会更低(y 单调不减),直接停。
        if top + row.height() < clip.top() {
            continue;
        }
        if top > clip.bottom() {
            break;
        }
        let color = if cursor_line == Some(line - 1) {
            hot
        } else {
            plain
        };
        painter.text(
            egui::pos2(digits_right, base.y + row.center().y),
            egui::Align2::RIGHT_CENTER,
            line.to_string(),
            font.clone(),
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::{LiveState, RenderMode};
    use crate::state::{OutlineCursor, PreviewState};
    use crate::ui::editor::{tab_editor_id, CursorChannel};
    use latermd_editor::EditorBuffer;

    /// 跑一帧源码模式编辑面板,返回该帧 shapes(行号取证)。视口 800×600
    /// 与 editor.rs 的滚动测试同口径(10000×10000 测不到视口裁剪)。
    /// `pending` 走 §6.4 写回通道,用于把光标落到已知位置。
    fn frame_shapes(
        ctx: &egui::Context,
        editor: &mut EditorBuffer,
        now: f64,
        pending: Option<(usize, usize)>,
        events: Vec<egui::Event>,
    ) -> Vec<egui::epaint::ClippedShape> {
        let mut preview = PreviewState::new(editor);
        let mut cursor = OutlineCursor::default();
        let mut selection = None;
        let mut pending = pending;
        let mut live = LiveState::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let output = ctx.run_ui(
            egui::RawInput {
                events,
                time: Some(now),
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                crate::ui::editor::ui(
                    ui,
                    editor,
                    &mut preview,
                    CursorChannel {
                        cursor: &mut cursor,
                        selection: &mut selection,
                        pending: &mut pending,
                    },
                    &mut live,
                    RenderMode::Source,
                    tab_editor_id(1),
                    // 行号槽测试不涉 minimap/打字机/专注:都关(现状路径)。
                    false,
                    false,
                    false,
                    None,
                    false,
                    &crate::keymap::Keymap::builtin(),
                    &mut Vec::new(),
                    &[],
                );
            },
        );
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        shapes
    }

    /// 单文档单帧的便捷形态(多帧测试直接用 [`frame_shapes`])。
    fn once(text: &str, now: f64) -> Vec<egui::epaint::ClippedShape> {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new(text);
        frame_shapes(&ctx, &mut editor, now, None, Vec::new())
    }

    /// shapes 里的行号字:galley 文本是纯 ASCII 数字(测试文档不含 ASCII
    /// 数字,纯数字形状唯一来源就是行号槽)。返回 (行号文本, 屏幕矩形, 颜色)。
    fn digits(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Rect, egui::Color32)> {
        let mut found = vec![];
        for clipped in shapes {
            if let egui::Shape::Text(t) = &clipped.shape {
                let text = t.galley.text();
                if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
                    let rect = egui::Rect::from_min_size(t.pos, t.galley.size());
                    let color = t.galley.job.sections[0].format.color;
                    found.push((text.to_owned(), rect, color));
                }
            }
        }
        found
    }

    /// shapes 里编辑器正文的 galley 形状(TextEdit 整段文本一个 shape,
    /// 以文档标记字定位)。从它能读到行矩形,做对齐断言的「真值」侧。
    fn editor_text<'a>(
        shapes: &'a [egui::epaint::ClippedShape],
        marker: &str,
    ) -> &'a egui::epaint::TextShape {
        shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(t) if t.galley.text().contains(marker) => Some(t),
                _ => None,
            })
            .expect("编辑器正文 galley 已绘制")
    }

    /// 逻辑行(1-based)→ 其首 visual row 的下标(与绘制侧 `is_line_start`
    /// 同一定义,供测试换算真值)。
    fn logical_anchor_row(galley: &egui::Galley, line: usize) -> Option<usize> {
        let mut seen = 0usize;
        for idx in 0..galley.rows.len() {
            if is_line_start(galley, idx) {
                seen += 1;
                if seen == line {
                    return Some(idx);
                }
            }
        }
        None
    }

    /// ① 行号 y 与 galley 逻辑行首 visual row 对齐:数字矩形中心 y 逐行
    /// 等于 `galley_pos.y + 行矩形中心 y`(同一帧的 galley 度量),且
    /// 右缘对齐、值连续。
    #[test]
    fn digits_align_with_galley_line_tops() {
        let text = vec!["很普通的一行"; 20].join("\n");
        let shapes = once(&text, 0.0);
        let editor = editor_text(&shapes, "很普通");
        assert_eq!(editor.galley.rows.len(), 20, "短行不折行,visual == 逻辑");

        let found = digits(&shapes);
        assert_eq!(found.len(), 20, "20 行全部可见(20 行高 < 600px 视口)");
        let mut by_y = found;
        by_y.sort_by(|a, b| a.1.top().total_cmp(&b.1.top()));
        for (idx, (label, rect, _)) in by_y.iter().enumerate() {
            assert_eq!(label, &(idx + 1).to_string(), "自上而下行号连续");
            let row = editor.galley.rows[idx].rect();
            let expected = editor.pos.y + row.center().y;
            let actual = rect.center().y;
            assert!(
                (actual - expected).abs() < 0.51,
                "第 {} 行号中心 y={actual},galley 行首中心 y={expected}",
                idx + 1
            );
        }
        let right = by_y[0].1.right();
        assert!(
            by_y.iter()
                .all(|(_, r, _)| (r.right() - right).abs() < 0.51),
            "行号右对齐,右缘一致"
        );
    }

    /// ② 折行文档:行号数 = 逻辑行数 ≠ visual 行数。一段超宽长文折成多
    /// 个 visual row,行号只画「1」;混合文档中折行段也只占一个行号。
    #[test]
    fn wrapped_document_numbers_logical_lines_only() {
        let long_paragraph = "很长的段落".repeat(60);
        let shapes = once(&long_paragraph, 0.0);
        let editor = editor_text(&shapes, "很长的段落");
        assert!(
            editor.galley.rows.len() > 1,
            "超宽长文确实折行了(visual 行数 {})",
            editor.galley.rows.len()
        );
        let found = digits(&shapes);
        assert_eq!(found.len(), 1, "一个逻辑行只画一个行号");
        assert_eq!(found[0].0, "1");

        // 混合:三行,第二行折成多个 visual row —— 行号仍是 1、2、3
        let mixed = format!("短一\n{}\n短二", "甲乙丙丁".repeat(50));
        let shapes = once(&mixed, 0.0);
        let editor = editor_text(&shapes, "甲乙丙丁");
        assert!(
            editor.galley.rows.len() > 3,
            "第二行折行使 visual 行数超过逻辑行数(实际 {})",
            editor.galley.rows.len()
        );
        let mut labels: Vec<String> = digits(&shapes).into_iter().map(|(l, _, _)| l).collect();
        labels.sort();
        assert_eq!(labels, vec!["1", "2", "3"], "行号数 == 逻辑行数");
    }

    /// ③ 宽度随 9→10 行位数变化:位数跳档时行号变宽一档、右缘不动;
    /// 纯函数侧同时钉 digit_count 的档位表。
    #[test]
    fn width_grows_one_digit_when_line_count_crosses_ten() {
        for (lines, digits_expect) in [
            (0, 1),
            (1, 1),
            (9, 1),
            (10, 2),
            (99, 2),
            (100, 3),
            (1000, 4),
        ] {
            assert_eq!(digit_count(lines), digits_expect, "{lines} 行的位数");
        }

        let nine = once(&["行"; 9].join("\n"), 0.0);
        let ten = once(&["行"; 10].join("\n"), 0.0);
        let nine_digits = digits(&nine);
        let ten_digits = digits(&ten);
        assert_eq!(nine_digits.len(), 9);
        assert!(
            ten_digits.iter().any(|(l, _, _)| l == "10"),
            "第 10 行有行号"
        );

        let single = nine_digits[0].1; // 9 行文档:所有行号都是一位
        let ten_single = ten_digits
            .iter()
            .find(|(l, _, _)| l == "1")
            .map(|(_, r, _)| *r)
            .expect("\"1\" 已绘制");
        let double = ten_digits
            .iter()
            .find(|(l, _, _)| l == "10")
            .map(|(_, r, _)| *r)
            .expect("\"10\" 已绘制");
        assert!(
            double.width() > single.width() + 2.0,
            "两位行号比一位宽一档({} > {} + 2)",
            double.width(),
            single.width()
        );
        // 槽左锚定:位宽跳档时整列右移恰好一个数字宽、TextEdit 同步让位;
        // 同帧内所有行号共享同一右缘 —— 「防跳动」= 档内纹丝不动、跨档
        // 一次性让位,不逐行漂移。
        assert!(
            (double.right() - ten_single.right()).abs() < 0.51,
            "同帧行号右缘一致({} vs {})",
            double.right(),
            ten_single.right()
        );
        assert!(
            double.left() < ten_single.left() - 2.0,
            "两位数向左扩一档({} < {} - 2)",
            double.left(),
            ten_single.left()
        );
        let digit_step = double.width() - ten_single.width();
        let column_shift = ten_single.right() - single.right();
        assert!(
            (column_shift - digit_step).abs() < 0.51,
            "跨档整列右移一个数字宽(实测 {column_shift},数字宽 {digit_step})"
        );
        // TextEdit 让位:行号槽宽了,编辑器文本左缘同步右移一个数字宽
        let editor_left = |shapes: &[_], marker: &str| editor_text(shapes, marker).pos.x;
        let shift = editor_left(&ten, "行") - editor_left(&nine, "行");
        assert!(
            (digit_step - 1.0..digit_step + 1.0).contains(&shift),
            "编辑器文本左缘右移一个数字宽(实测 {shift}px,数字宽 {digit_step}px)"
        );
    }

    /// 超长文档只画视口可见行(#31 性能红线:500 行文档不得画 500 个行号),
    /// 且从 1 开始连续 —— 视口停在文档顶。
    #[test]
    fn long_document_paints_only_visible_lines() {
        let text = (0..500)
            .map(|_| "普通的一行")
            .collect::<Vec<_>>()
            .join("\n");
        let shapes = once(&text, 0.0);
        let found = digits(&shapes);
        assert!(
            (20..45).contains(&found.len()),
            "600px 视口约容纳 30 行,实测 {} 个行号",
            found.len()
        );
        let mut by_y = found;
        by_y.sort_by(|a, b| a.1.top().total_cmp(&b.1.top()));
        assert_eq!(by_y[0].0, "1", "视口在文档顶,首行行号是 1");
        for (idx, (label, _, _)) in by_y.iter().enumerate() {
            assert_eq!(label, &(idx + 1).to_string(), "可见区间行号连续");
        }
    }

    /// 滚动帧:行号与文本同处一个 ScrollArea 闭包,滚轮把内容推离顶部后,
    /// 可见行号区间从 1 之外开始、数量仍是视口规模,且与 galley 行首
    /// 中心的对齐关系在滚动后保持(与 editor.rs 滚轮测试同款事件序列,
    /// 滚动落账后补两帧再取证)。
    #[test]
    fn scrolled_viewport_keeps_digits_aligned_and_culled() {
        let ctx = egui::Context::default();
        let text = (0..500)
            .map(|_| "普通的一行")
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = EditorBuffer::new(&text);
        let wheel = || {
            vec![
                egui::Event::PointerMoved(egui::pos2(400.0, 200.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -120.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let _ = frame_shapes(&ctx, &mut editor, 0.0, None, Vec::new());
        for i in 0..10 {
            let _ = frame_shapes(&ctx, &mut editor, 0.1 + f64::from(i) * 0.1, None, wheel());
        }
        // 偏移在 ScrollArea::end 落账、hover 判定滞后一帧,补两帧再取证
        let _ = frame_shapes(&ctx, &mut editor, 1.2, None, Vec::new());
        let _ = frame_shapes(&ctx, &mut editor, 1.3, None, Vec::new());
        let shapes = frame_shapes(&ctx, &mut editor, 1.4, None, Vec::new());

        let editor_text = editor_text(&shapes, "普通的一行");
        let mut by_y = digits(&shapes);
        by_y.sort_by(|a, b| a.1.top().total_cmp(&b.1.top()));
        assert!(
            (20..45).contains(&by_y.len()),
            "滚动后仍只画视口规模(实测 {} 个)",
            by_y.len()
        );
        let first: usize = by_y[0].0.parse().expect("行号是数字");
        assert!(first > 1, "视口已离开文档顶,首见行号 >1(实测 {first})");
        for pair in by_y.windows(2) {
            let a: usize = pair[0].0.parse().unwrap();
            let b: usize = pair[1].0.parse().unwrap();
            assert_eq!(b, a + 1, "滚动后行号仍连续");
        }
        // 对齐保持:每个可见行号中心 y == 该逻辑行首 visual row 中心
        for (label, rect, _) in &by_y {
            let line: usize = label.parse().unwrap();
            let anchor = logical_anchor_row(&editor_text.galley, line).expect("逻辑行存在");
            let row = editor_text.galley.rows[anchor].rect();
            let expected = editor_text.pos.y + row.center().y;
            assert!(
                (rect.center().y - expected).abs() < 0.51,
                "滚动后第 {line} 行号中心 {} 与 galley 行首中心 {} 对齐",
                rect.center().y,
                expected
            );
        }
    }

    /// 空文档画「1」。
    #[test]
    fn empty_document_paints_line_one() {
        let found = digits(&once("", 0.0));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "1");
    }

    /// ④ 明暗两套渲染各三帧不 panic,且每帧行号都在(三帧手法)。
    #[test]
    fn light_and_dark_visuals_paint_three_frames_each_without_panic() {
        for dark in [true, false] {
            let ctx = egui::Context::default();
            ctx.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            let mut editor = EditorBuffer::new("明暗各三帧\n不 panic\n");
            for step in 0..3 {
                let shapes =
                    frame_shapes(&ctx, &mut editor, f64::from(step) * 0.1, None, Vec::new());
                assert!(
                    !digits(&shapes).is_empty(),
                    "{} 第 {step} 帧行号已绘制",
                    if dark { "暗色" } else { "亮色" }
                );
            }
        }
    }

    /// 光标所在逻辑行的行号用 accent 色,其余行不用(暗色默认下的
    /// `tokens::accent` 取当前应用皮肤)。光标经 §6.4 写回通道落到第 3 行首。
    #[test]
    fn cursor_line_digit_gets_accent_color() {
        let ctx = egui::Context::default();
        let mut editor = EditorBuffer::new("甲\n乙\n丙\n丁\n");
        let _ = frame_shapes(&ctx, &mut editor, 0.0, Some((4, 4)), Vec::new());
        let found = digits(&frame_shapes(&ctx, &mut editor, 0.1, None, Vec::new()));

        let accent = crate::theme::shell_tokens(true).accent;
        assert!(
            found.iter().any(|(l, _, c)| l == "3" && *c == accent),
            "光标行的行号是 accent 色"
        );
        assert!(
            found
                .iter()
                .filter(|(l, _, _)| l != "3")
                .all(|(_, _, c)| *c != accent),
            "其余行号保持 noninteractive 色"
        );
    }
}
