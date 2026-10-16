//! #111 B(视口外估高延迟布局)的无头回归哨兵(纯测试模块)。
//!
//! 剔除哨兵:屏外块不再 shape —— 以渲染臂注册的块命中表为证:滚到底部
//! 时,顶部的块**没有**本帧命中表(被估高占位),视口内的块有;滚回顶部
//! 反向同理。命中表只由真实渲染臂(`MarkdownLabel::show`)建立,估高占位
//! 不建,存在性即「本帧是否真实渲染」的判据。
//!
//! 滚动锚定哨兵:同视口稳态帧 offset 不得漂移 —— 占位块替换真实块的帧
//! 里,上方块差值已被 `estimate_drift` 抵消,可见内容钉在原处。
//!
//! 滚动的注入方式:走生产的锁定浏览跳转链(`cursor.jump_to` → 块级
//! `ui.scroll_to_rect`),与 D2 的 `locked_browse_jump_*` 测试同一条通路;
//! 跳转动画(~0.25s)需要推进 `RawInput.time` 并多帧收敛,无头 dt=0 时动画
//! 永不完成 —— 这也是本组帧序里有收敛帧的原因。

use super::*;

/// 多段文档(中文正文 + 围栏代码,块高差异明显,滚动位移可观)。
fn sample_doc(sections: usize) -> String {
    let mut text = String::new();
    for i in 0..sections {
        let _ = std::fmt::Write::write_fmt(
            &mut text,
            format_args!(
                "## 段 {i}\n\n这是第 {i} 段的中文正文,长到可以滚动。\n\n\
                 ```rust\nfn step_{i}(log: &Log) -> u64 {{\n    log.tail()\n}}\n```\n\n"
            ),
        );
    }
    text
}

/// 剔除哨兵:锁定浏览跳到文末块(生产的滚动通路 `cursor.jump_to` → 块级
/// `scroll_to_rect`,与 D2 测试同一条链),顶部的块无本帧命中表(被估高
/// 占位),视口内的块有;再跳回文首,反向同理。证「屏外不 shape」。
#[test]
fn offscreen_blocks_are_placeholder_not_shaped() {
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0));
    let text = sample_doc(60);
    let mut editor = EditorBuffer::new(&text);
    let mut preview = PreviewState::new(&editor);
    let mut live = LiveState::default();
    live.sync(&editor, None);
    live.browse_only = true;
    live.browse_enabled = false;
    live.browse_locked = true;
    let id = egui::Id::new("offscreen-cull");
    let total = live.blocks.len();
    assert!(total > 20, "样本文档要有足够的块(实测 {total})");

    // 目标:文末块的块内字符偏移。
    let tail_byte = live.blocks[total - 1].start + 1;
    let tail_char = editor.byte_to_char(tail_byte);
    let head_char = 0;

    let mut presence: Vec<Vec<bool>> = Vec::new();
    let mut probes = Vec::new();
    let mut time = 0.0f64;
    // 帧序:顶部稳态 → 跳文末 → 文末稳态 → 跳文首 → 文首稳态。
    for jump in [None, Some(tail_char), None, Some(head_char), None] {
        let mut cursor = OutlineCursor {
            jump_to: jump,
            ..OutlineCursor::default()
        };
        let mut selection = None;
        let mut outbox = Vec::new();
        let mut per_block: Vec<bool> = Vec::new();
        // 跳转的 scroll_to_rect 走 egui 滚动动画(默认 ~0.25s):推进时间并
        // 多渲几帧让动画收敛,稳态帧读表(只收最后一帧 —— 前几帧混合了
        // 滚动中几何)。
        for pass in 0..8 {
            time += 0.1;
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(time),
                    ..Default::default()
                },
                |panel| {
                    let _ = super::ui(
                        panel,
                        &mut editor,
                        &mut preview,
                        &AiState::default(),
                        &mut cursor,
                        &mut selection,
                        &mut live,
                        id,
                        false,
                        false,
                        None,
                        false,
                        &crate::keymap::Keymap::builtin(),
                        &mut outbox,
                    );
                    if pass == 7 {
                        for index in 0..live.blocks.len() {
                            let exists = egui_markdown::block_span_rects_present(
                                panel,
                                id.with(("live-render", index)),
                            );
                            per_block.push(exists);
                        }
                    }
                },
            );
            output.drop_without_applying_deltas();
        }
        let probe = ctx.data(|d| {
            d.get_temp::<LiveScrollProbe>(super::scroll_probe_id(id))
                .unwrap_or(LiveScrollProbe {
                    offset: -1.0,
                    viewport: egui::Rect::NOTHING,
                    content_height: 0.0,
                })
        });
        probes.push(probe);
        presence.push(std::mem::take(&mut per_block));
    }

    let top = &presence[0];
    let tail = &presence[1];
    let back_top = &presence[3];

    assert!(top[0], "顶部视口:首块真实渲染");
    assert!(!top[total - 1], "顶部视口:末块应被估高占位");
    assert!(
        !tail[0],
        "跳到文末(offset {:?}):首块应被估高占位(仍建表 = 剔除未生效,块 {total})",
        probes[1].offset
    );
    assert!(tail[total - 1], "跳到文末:末块在视口内,真实渲染");
    assert!(back_top[0], "跳回文首:首块恢复真实渲染");
    assert!(!back_top[total - 1], "跳回文首:末块再次被估高占位");
}

/// 滚动锚定哨兵:同视口稳态帧 offset 不得漂移 —— 占位块替换真实块的帧
/// 里,上方块差值已被 `estimate_drift` 抵消,可见内容钉在原处。
#[test]
fn offscreen_anchored_scroll_does_not_drift_in_steady_state() {
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0));
    let text = sample_doc(60);
    let mut editor = EditorBuffer::new(&text);
    let mut preview = PreviewState::new(&editor);
    let mut live = LiveState::default();
    live.sync(&editor, None);
    live.browse_only = true;
    live.browse_enabled = false;
    live.browse_locked = true;
    let id = egui::Id::new("offscreen-anchor");
    let total = live.blocks.len();

    // 跳到文档中部块(锁定浏览 → scroll_to_rect),推进动画收敛后,连续
    // 两帧同视口:第二帧相对第一帧的 offset 漂移必须 < 0.5px。
    let mid_block = total / 2;
    let mid_byte = live.blocks[mid_block].start + 1;
    let mid_char = editor.byte_to_char(mid_byte);

    let mut cursor = OutlineCursor {
        jump_to: Some(mid_char),
        ..OutlineCursor::default()
    };
    let mut selection = None;
    let mut outbox = Vec::new();
    let mut offsets = Vec::new();
    let mut time = 0.0f64;
    // 帧序:跳转帧(动画启动)→ 6 帧收敛(时间推进)→ 2 帧稳态采样。
    for pass in 0..9 {
        time += 0.1;
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                ..Default::default()
            },
            |panel| {
                let _ = super::ui(
                    panel,
                    &mut editor,
                    &mut preview,
                    &AiState::default(),
                    &mut cursor,
                    &mut selection,
                    &mut live,
                    id,
                    false,
                    false,
                    None,
                    false,
                    &crate::keymap::Keymap::builtin(),
                    &mut outbox,
                );
            },
        );
        output.drop_without_applying_deltas();
        if pass >= 7 {
            let probe = ctx.data(|d| {
                d.get_temp::<LiveScrollProbe>(super::scroll_probe_id(id))
                    .unwrap_or(LiveScrollProbe {
                        offset: -1.0,
                        viewport: egui::Rect::NOTHING,
                        content_height: 0.0,
                    })
            });
            offsets.push(probe.offset);
        }
    }
    assert_eq!(offsets.len(), 2);
    assert!(
        (offsets[1] - offsets[0]).abs() < 0.5,
        "同视口稳态帧 offset 不得漂移(实测 {:+.2})",
        offsets[1] - offsets[0]
    );
    assert!(
        offsets[0] > 0.0,
        "跳到中部后应离开文档顶(实测 {:?})",
        offsets[0]
    );
}
