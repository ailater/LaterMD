//! #39 M1「切换卡顿取证」的无头测量 harness(纯测试模块,生产构建不编译)。
//!
//! 手法照抄库内测试的 egui 上下文用法:整帧走生产路径
//! [`LaterMdApp::draw`](同 `ui/layout.rs` 测试的 `draw_frames`),单件预览/
//! 编辑器走 `ctx.run_ui`(同 `ui/preview.rs` 测试)。不触碰 vendored 一行,
//! 分段计时全部用公开 API(`egui_markdown::{heal, parse}`、
//! `latermd_md::{expand_wikilinks, outline}`)在 app 侧复现同一变换。
//!
//! 四路测量:
//! **A 归约/快照侧**(打开与编辑修订号前进时付,不在切换帧):expand_wikilinks /
//! outline / open_tab 整篇换入;
//! **B 预览渲染侧**(每帧):heal / parse / 第 5 层相对图片改写 /
//! [`MarkdownLabel`] 冷帧(缓存 miss 全量)与热帧(命中);
//! **C 编辑器侧**:multiline `TextEdit` 冷帧(文本变化触发整篇 layout)与热帧;
//! **D 整帧(生产路径)**:tab A 稳态 → `TabActivate` → B 首帧/次帧/稳态 →
//! 往返 A 首帧,与 B/C 的微基准数字互相咬合,核验 vendored temp memory 缓存
//! 在切换帧的命中/miss 行为;
//! **E 规模放大**:20000 行样本的整帧与单件切换/往返,含 pre-M2 常量 id
//! 单槽机制的旧路径对照行(切回必全量 miss = 用户「来回翻长文档」的旧成本)。
//!
//! 跑法(计时口径 = release;debug 构建数字大 ~10×,只可作相对对照):
//! `cargo test -p latermd-app --release tab_switch -- --test-threads=1 --ignored --nocapture`;
//! 两个回归断言测试(不加 `--ignored`)随常规门禁跑,防缓存行为悄悄回归。

use std::borrow::Cow;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use eframe::egui::{self, Id, RawInput, Rect, ScrollArea};
use egui_markdown::MarkdownLabel;

use crate::state::Message;
use crate::LaterMdApp;

/// 无头屏幕:与真实窗口(900×600 起,常规更大)同量级,保证 TextEdit 行数
/// 与预览视口不是退化值。
fn screen() -> Rect {
    Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(1280.0, 800.0))
}

/// 任务指定的样本规模:2000 行。
const SAMPLE_LINES: usize = 2000;

/// 中英混排样本(标题/段落/代码块/表格/列表/引用循环,含 wikilink 与相对
/// 图片),`seed` 区分两个文档(同构不同文,模拟「tab A ↔ tab B」)。
fn sample_doc(seed: u32, target_lines: usize) -> String {
    let mut doc = String::with_capacity(160 * 1024);
    let mut lines = 0;
    let mut section = 0;
    while lines < target_lines {
        section += 1;
        let zh = match section % 3 {
            0 => "渲染层缓存与视口剔除",
            1 => "解析器的分段契约",
            _ => "编辑器缓冲与撤销栈",
        };
        let _ = writeln!(doc, "## 第 {seed}-{section} 节 {zh} (section {section})");
        let _ = writeln!(doc);
        let _ = writeln!(
            doc,
            "这是一段用于测量的中文正文,混排 English words 与 **加粗**、\
             `行内代码`、[链接](https://example.com/{seed}/{section})、\
             [[设计决策-{seed}-{section}]] 与 ![配图](./{seed}.assets/fig-{section}.png)。"
        );
        let _ = writeln!(doc);
        let _ = writeln!(doc, "```rust");
        let _ = writeln!(
            doc,
            "fn claim_{section}(log: &Log, seq: u64) -> Result<(), Error> {{"
        );
        for step in 0..12 {
            let _ = writeln!(doc, "    let step_{step} = log.tail()?;");
        }
        let _ = writeln!(doc, "    log.insert(seq)");
        let _ = writeln!(doc, "}}");
        let _ = writeln!(doc, "```");
        let _ = writeln!(doc);
        let _ = writeln!(doc, "| 列甲 | 列乙 | 列丙 |");
        let _ = writeln!(doc, "|---|---|---|");
        for row in 1..=2 {
            let _ = writeln!(doc, "| 单元 {row}-{seed} | cell {row} | {row}{row} |");
        }
        let _ = writeln!(doc);
        let _ = writeln!(
            doc,
            "- 列表项一 {}\n- 列表项二\n  - 嵌套项 {section}\n- 列表项三",
            section
        );
        let _ = writeln!(doc);
        let _ = writeln!(
            doc,
            "> 引用块 {section}:检验块级容器的排版与换行开销,中英混排 mixed text。\n"
        );
        lines += 30;
    }
    doc
}

/// 预览单件一帧,pre-M2 生产同配置(ScrollArea + 常量 id + wrap + heal)。
/// M2 起生产 id 已按 tab 分槽(见 [`label_frame_with_id`]),本函数保留常量
/// id 作为旧路径单槽机制的基线探针:回归哨的 miss 断言与 [E] 的旧路径
/// 对照行都靠它复现「切文本必全量重解析」的切换帧成本。返回耗时。
fn label_frame(ctx: &egui::Context, rendered: &str) -> Duration {
    label_frame_with_id(ctx, &Id::new("preview-md"), rendered)
}

/// [`label_frame`] 的 id 参数化变体:M2 起生产 id 含 tab 维度
/// (`tab_preview_id` = "preview-md".with(tab_id)),往返命中断言需要分别
/// 驱动两个 tab 的缓存槽位。
fn label_frame_with_id(ctx: &egui::Context, id: &egui::Id, rendered: &str) -> Duration {
    let start = Instant::now();
    ctx.run_ui(
        RawInput {
            screen_rect: Some(screen()),
            ..Default::default()
        },
        |ui| {
            ScrollArea::vertical()
                .id_salt(id.with("scroll"))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // 与生产 preview.rs 同一 id 命名空间:切换文本时的缓存
                    // 行为(同 id 覆盖/hash 不匹配 → miss)与生产一致。
                    // defer_offscreen_highlight 同步生产(#59 M2 起预览开启,
                    // miss 帧视口外段免 syntect,几何零变化)。
                    MarkdownLabel::new(*id, rendered)
                        .wrap()
                        .heal(true)
                        .defer_offscreen_highlight(true)
                        .show(ui);
                });
        },
    )
    .drop_without_applying_deltas();
    start.elapsed()
}

/// 编辑器单件一帧,生产同配置(multiline + Monospace + 不限宽 + ScrollArea)。
/// 文本 clone 在计时之外,只量 TextEdit 本身。返回耗时。
fn editor_frame(ctx: &egui::Context, text: &str) -> Duration {
    let mut buffer = text.to_owned();
    let start = Instant::now();
    ctx.run_ui(
        RawInput {
            screen_rect: Some(screen()),
            ..Default::default()
        },
        |ui| {
            ScrollArea::vertical()
                .id_salt("perf-editor-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::TextEdit::multiline(&mut buffer)
                        .id(Id::new("perf-editor"))
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .desired_rows(20)
                        .show(ui);
                });
        },
    )
    .drop_without_applying_deltas();
    start.elapsed()
}

/// 生产整帧一帧(LaterMdApp::draw,含全部面板)。返回耗时。
fn app_frame(app: &mut LaterMdApp, ctx: &egui::Context) -> Duration {
    let start = Instant::now();
    ctx.run_ui(
        RawInput {
            screen_rect: Some(screen()),
            ..Default::default()
        },
        |ui| app.draw(ui),
    )
    .drop_without_applying_deltas();
    start.elapsed()
}

/// 生产完整帧(**归约 + 绘制**,与 `eframe::App::update` 同序)。
///
/// [F] 必须用这一条:[D]/[E] 只量绘制(`app_frame`),而本轮省下的那一笔
/// 是**归约侧**的闸门(`end_of_logic` 写 `PreviewState::outline_shown`)——
/// 只跑绘制的话闸门永远是 `PreviewState::new` 留下的初值,量出来的两组
/// 数字没有差别(曾因此误判「优化没效果」)。
fn app_frame_full(app: &mut LaterMdApp, ctx: &egui::Context) -> Duration {
    let start = Instant::now();
    ctx.run_ui(
        RawInput {
            screen_rect: Some(screen()),
            ..Default::default()
        },
        |ui| {
            app.reduce(ui.ctx());
            app.draw(ui);
        },
    )
    .drop_without_applying_deltas();
    start.elapsed()
}

/// 连跑 `frames` 帧不计时(预热:字体图集、面板/控件持久状态)。
fn warmup_frames(app: &mut LaterMdApp, ctx: &egui::Context, frames: usize) {
    for _ in 0..frames {
        app_frame(app, ctx);
    }
}

/// [`warmup_frames`] 的「归约 + 绘制」版,配 [`app_frame_full`] 用。
fn warmup_frames_full(app: &mut LaterMdApp, ctx: &egui::Context, frames: usize) {
    for _ in 0..frames {
        app_frame_full(app, ctx);
    }
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

fn fmt_us(d: Duration) -> String {
    let us = d.as_nanos() as f64 / 1_000.0;
    if us < 1_000.0 {
        format!("{us:.1} µs")
    } else {
        format!("{:.2} ms", us / 1_000.0)
    }
}

fn row(name: &str, d: Duration) {
    println!("  {name:<58} {}", fmt_us(d));
}

/// `#[ignore]` 完整取证:打印五段耗时表(数字对比落档 docs/auto-plan.md
/// #39 行,修法取舍见 docs/decisions-pending.md #60)。
#[test]
#[ignore = "取证报告;跑法:cargo test -p latermd-app --release tab_switch -- --test-threads=1 --ignored --nocapture"]
fn tab_switch_finding_report() {
    let doc_a = sample_doc(1, SAMPLE_LINES);
    let doc_b = sample_doc(2, SAMPLE_LINES);
    println!(
        "==== M1 切换卡顿取证:样本 {SAMPLE_LINES} 行(doc_a {} 字节 / doc_b {} 字节,同一 ctx)====",
        doc_a.len(),
        doc_b.len()
    );

    println!("[A] 归约/快照侧(打开与编辑时付,不在切换帧)");
    let rendered_a = latermd_md::expand_wikilinks(&doc_a);
    let rendered_b = latermd_md::expand_wikilinks(&doc_b);
    row("expand_wikilinks(全文展开)", {
        let mut samples = Vec::new();
        for _ in 0..5 {
            let start = Instant::now();
            let out = latermd_md::expand_wikilinks(std::hint::black_box(&doc_a));
            std::hint::black_box(&out);
            samples.push(start.elapsed());
        }
        median(samples)
    });
    row("outline(全文标题扫描)", {
        let mut samples = Vec::new();
        for _ in 0..5 {
            let start = Instant::now();
            let out = latermd_md::outline(std::hint::black_box(&doc_a));
            std::hint::black_box(&out);
            samples.push(start.elapsed());
        }
        median(samples)
    });

    println!("[B] 预览渲染侧(每帧);heal 完整文档应为 Cow::Borrowed");
    let healed = match egui_markdown::heal(&doc_a) {
        Cow::Borrowed(_) => {
            println!("  heal(完整文档) => Cow::Borrowed(恒等零拷贝)");
            Cow::<str>::Borrowed(&doc_a)
        }
        Cow::Owned(owned) => {
            println!("  heal(完整文档) => Cow::Owned(异常,本文档本不该被补闭合)");
            Cow::Owned(owned)
        }
    };
    row("heal(完整文档逐行扫描)", {
        let mut samples = Vec::new();
        for _ in 0..20 {
            let start = Instant::now();
            let out = egui_markdown::heal(std::hint::black_box(&doc_a));
            std::hint::black_box(&out);
            samples.push(start.elapsed());
        }
        median(samples)
    });
    row("parse(heal 后全文)", {
        let mut samples = Vec::new();
        for _ in 0..10 {
            let start = Instant::now();
            let out = egui_markdown::parse(std::hint::black_box(healed.as_ref()));
            std::hint::black_box(&out);
            samples.push(start.elapsed());
        }
        median(samples)
    });
    row("image_rewrites+apply(有相对图,Some(base))", {
        let mut samples = Vec::new();
        for _ in 0..20 {
            let start = Instant::now();
            // Live 第 5 层同款两步:改写清单 → OffsetMap::apply
            let rewrites = crate::ui::rich_widgets::image_rewrites(
                std::hint::black_box(&doc_a),
                Some(std::path::Path::new("/home/u/docs")),
            );
            let out = latermd_md::OffsetMap::from_rewrites(rewrites).apply(&doc_a);
            std::hint::black_box(&out);
            samples.push(start.elapsed());
        }
        median(samples)
    });
    row("image_rewrites+apply(无相对图,借回)", {
        let mut samples = Vec::new();
        for _ in 0..20 {
            let start = Instant::now();
            // Live 第 5 层同款两步:改写清单 → OffsetMap::apply
            let rewrites = crate::ui::rich_widgets::image_rewrites(
                std::hint::black_box(&doc_b),
                Some(std::path::Path::new("/home/u/docs")),
            );
            let out = latermd_md::OffsetMap::from_rewrites(rewrites).apply(&doc_b);
            std::hint::black_box(&out);
            samples.push(start.elapsed());
        }
        median(samples)
    });

    println!("[B2] MarkdownLabel 单件(pre-M2 常量 id 口径,id=preview-md,同一 ctx 连续帧)");
    let ctx = egui::Context::default();
    // 预热一帧:字体图集等一次性初始化不混进冷帧数字(只冷在文档缓存)。
    ctx.run_ui(RawInput::default(), |_| {})
        .drop_without_applying_deltas();
    let cold_a = label_frame(&ctx, &rendered_a);
    row("冷首帧 A(parse+layout+高亮全量)", cold_a);
    let hot_a = {
        let mut samples = Vec::new();
        for _ in 0..7 {
            samples.push(label_frame(&ctx, &rendered_a));
        }
        median(samples)
    };
    row("稳态帧 A(缓存命中,7 帧中位)", hot_a);
    let first_b = label_frame(&ctx, &rendered_b);
    row("切到 B 首帧(同 id 换文本 = miss)", first_b);
    let hot_b = {
        let mut samples = Vec::new();
        for _ in 0..7 {
            samples.push(label_frame(&ctx, &rendered_b));
        }
        median(samples)
    };
    row("稳态帧 B(缓存命中)", hot_b);
    let back_a = label_frame(&ctx, &rendered_a);
    row("切回 A 首帧(往返:flush 段缓存残留与否)", back_a);
    let ctx_fresh = egui::Context::default();
    ctx_fresh
        .run_ui(RawInput::default(), |_| {})
        .drop_without_applying_deltas();
    let cold_b_fresh = label_frame(&ctx_fresh, &rendered_b);
    row("对照:B 在全新 ctx 的冷首帧", cold_b_fresh);

    println!("[C] TextEdit 单件(生产配置,同一 ctx)");
    let ctx = egui::Context::default();
    ctx.run_ui(RawInput::default(), |_| {})
        .drop_without_applying_deltas();
    let editor_cold = editor_frame(&ctx, &doc_a);
    row("冷首帧 A(整篇 layout)", editor_cold);
    let editor_hot = {
        let mut samples = Vec::new();
        for _ in 0..7 {
            samples.push(editor_frame(&ctx, &doc_a));
        }
        median(samples)
    };
    row("稳态帧 A(galley 缓存命中)", editor_hot);
    let editor_switch = editor_frame(&ctx, &doc_b);
    row("换到 B 首帧(文本变化 = 整篇 layout)", editor_switch);
    let editor_back = editor_frame(&ctx, &doc_a);
    row("换回 A 首帧(往返)", editor_back);

    println!("[D] 整帧(生产路径 LaterMdApp::draw,含全部面板)");
    let mut app = LaterMdApp::default();
    let ctx = egui::Context::default();
    let open_start = Instant::now();
    let index_a = app.state.tabs.open_tab(None, &doc_a);
    let open_a = open_start.elapsed();
    let open_start = Instant::now();
    let index_b = app.state.tabs.open_tab(None, &doc_b);
    let open_b = open_start.elapsed();
    row("open_tab A(读文本建预览快照,同步)", open_a);
    row("open_tab B(同上)", open_b);
    app.state.apply(Message::TabActivate(index_a));
    warmup_frames(&mut app, &ctx, 3);
    let steady_a = {
        let mut samples = Vec::new();
        for _ in 0..5 {
            samples.push(app_frame(&mut app, &ctx));
        }
        median(samples)
    };
    row("稳态帧 A(5 帧中位)", steady_a);
    let reduce_start = Instant::now();
    app.state.apply(Message::TabActivate(index_b));
    let reduce = reduce_start.elapsed();
    row("归约 TabActivate(1)(switch_active 本体)", reduce);
    let first_frame = app_frame(&mut app, &ctx);
    row("切换后首帧", first_frame);
    let second_frame = app_frame(&mut app, &ctx);
    row("切换后次帧", second_frame);
    let steady_b = {
        let mut samples = Vec::new();
        for _ in 0..5 {
            samples.push(app_frame(&mut app, &ctx));
        }
        median(samples)
    };
    row("稳态帧 B(5 帧中位)", steady_b);
    app.state.apply(Message::TabActivate(index_a));
    let back_first = app_frame(&mut app, &ctx);
    row("往返切回 A 首帧", back_first);

    println!("[E] 规模放大(切换首帧是否随文档规模线性;用户的「明显卡顿」按此口径外推)");
    let big_a = sample_doc(9, 20_000);
    let big_rendered = latermd_md::expand_wikilinks(&big_a);
    let ctx_big = egui::Context::default();
    ctx_big
        .run_ui(RawInput::default(), |_| {})
        .drop_without_applying_deltas();
    let mut app_big = LaterMdApp::default();
    let big_index_a = app_big.state.tabs.open_tab(None, &big_a);
    let big_index_b = app_big.state.tabs.open_tab(None, &sample_doc(8, 20_000));
    app_big.state.apply(Message::TabActivate(big_index_a));
    warmup_frames(&mut app_big, &ctx_big, 3);
    let big_steady = {
        let mut samples = Vec::new();
        for _ in 0..5 {
            samples.push(app_frame(&mut app_big, &ctx_big));
        }
        median(samples)
    };
    row("20000 行整帧稳态", big_steady);
    app_big.state.apply(Message::TabActivate(big_index_b));
    let big_first = app_frame(&mut app_big, &ctx_big);
    row("20000 行切换后首帧", big_first);
    let big_second = app_frame(&mut app_big, &ctx_big);
    row("20000 行切换后次帧", big_second);
    // M2 追加:大文档往返切回(用户反馈「切换卡顿」的主场景 —— 来回翻
    // 已打开过的长文档)。M2 起切回命中本 tab 缓存槽位,机制见
    // preview_cache_survives_tab_roundtrip_with_per_tab_ids。
    app_big.state.apply(Message::TabActivate(big_index_a));
    let big_back = app_frame(&mut app_big, &ctx_big);
    row("20000 行往返切回 A 首帧", big_back);
    let label_big_steady = {
        let mut samples = Vec::new();
        for _ in 0..7 {
            samples.push(label_frame(&ctx_big, &big_rendered));
        }
        median(samples)
    };
    row("20000 行 MarkdownLabel 稳态(单件)", label_big_steady);
    let label_big_switch = label_frame(
        &ctx_big,
        &latermd_md::expand_wikilinks(&sample_doc(7, 20_000)),
    );
    row("20000 行 MarkdownLabel 换文本首帧(单件)", label_big_switch);
    // 旧路径(pre-M2 常量 id 单槽)对照:上一帧 doc7 已把单槽覆盖,此刻
    // 切回 big 文档必然全量 miss —— 用户「来回翻长文档」在旧路径的真实
    // 成本;per-tab 分槽后同场景见上面的「20000 行往返切回 A 首帧」。
    let label_big_back_old_path = label_frame(&ctx_big, &big_rendered);
    row(
        "20000 行旧路径(常量 id)往返切回 A 首帧(单件)",
        label_big_back_old_path,
    );

    println!("==== 结论速读(数字解释见报告)====");
    println!(
        "  切换首帧 {}/稳态帧 = {:.1}×(A→B);往返首帧 {}/稳态帧 A = {:.1}×",
        fmt_us(first_frame),
        first_frame.as_secs_f64() / steady_a.as_secs_f64(),
        fmt_us(back_first),
        back_first.as_secs_f64() / steady_a.as_secs_f64(),
    );
}

/// [F] 击键路径取证:修订号前进那一帧比稳态帧贵多少。
///
/// 与 [D] 的切换帧互补 —— 切换是「一次性」成本,击键是**持续**成本:打字
/// 期间每一帧都要付。三档口径分开记:
/// * `稳态帧`:文本没变(`LiveState::sync` / `PreviewState::rebuild` 都被
///   修订号守卫挡住,只剩渲染);
/// * `击键帧`:插入一个字符后的那一帧(守卫放行 → `blocks` 全篇 parse +
///   `outline` 全篇扫描 + 文本 widget 整篇 layout);
/// * `增量`:两者之差,即「打一个字多付的」。
///
/// 跑法同 [D](计时口径 = release)。
#[test]
#[ignore = "取证报告;跑法:cargo test -p latermd-app --release typing_finding -- --test-threads=1 --ignored --nocapture"]
fn typing_finding_report() {
    for lines in [2_000usize, 20_000] {
        for mode in [
            crate::live::RenderMode::Source,
            crate::live::RenderMode::Live,
        ] {
            // 侧栏停在文件树 = 大纲没有消费者(闸门关);停在大纲页 = 有消费者
            // (闸门开,击键必须重扫)。两者之差就是 `PreviewState::sync_outline`
            // 省下的那一笔。
            for view in [
                crate::state::SidebarTab::Files,
                crate::state::SidebarTab::Outline,
            ] {
                let doc = sample_doc(11, lines);
                let mut app = LaterMdApp::default();
                let ctx = egui::Context::default();
                let index = app.state.tabs.open_tab(None, &doc);
                app.state.apply(Message::TabActivate(index));
                app.state.render_mode = mode;
                app.state.layout.left_view = view;
                warmup_frames_full(&mut app, &ctx, 3);

                let steady = {
                    let mut samples = Vec::new();
                    for _ in 0..5 {
                        samples.push(app_frame_full(&mut app, &ctx));
                    }
                    median(samples)
                };
                // 连续 5 次击键:每次都推进修订号,守卫都放行。插入本身(rope
                // splice + 镜像 splice)单列,不混进帧的账。
                let mut typed = Vec::new();
                let mut splice = Vec::new();
                for _ in 0..5 {
                    let tab = app.state.tabs.current_mut();
                    let at = tab.editor.len_chars();
                    let t = Instant::now();
                    tab.editor.insert_chars(at, "字");
                    splice.push(t.elapsed());
                    typed.push(app_frame_full(&mut app, &ctx));
                }
                let typed = median(typed);
                println!(
                "[F] {lines} 行 / {mode:?} / 侧栏 {view:?} ({:.0} KB):稳态 {} 击键帧 {} 增量 {} (插入本身 {})",
                doc.len() as f64 / 1024.0,
                fmt_us(steady),
                fmt_us(typed),
                fmt_us(typed.saturating_sub(steady)),
                fmt_us(median(splice)),
            );
            }
        }
    }
}

/// 完整文档(无残缺构造)上 `heal` 必须是恒等零拷贝(`Cow::Borrowed`):
/// 它在预览路径**每帧**跑,一旦开始产出 `Owned`,每帧多一次全文拼接,且
/// heal 产物的 hash 与源文本不同会连带放大下游成本。这是 heal 的语义契约
/// (vendor parser.rs 测试同款口径),在此以 2000 行样本钉住。
#[test]
fn heal_complete_document_is_identity_and_borrowed() {
    let doc = sample_doc(3, 400);
    assert!(
        matches!(egui_markdown::heal(&doc), Cow::Borrowed(_)),
        "完整文档上的 heal 不应产出 Owned(零拷贝契约被破坏)"
    );
}

/// 缓存行为的回归哨(#39 M1 测量结论的固化,数字口径见 `tab_switch_finding_report`):
/// - **稳态帧必须命中 vendored temp memory 缓存**:热帧只做 galley/绘制,
///   冷帧要做 heal+hash+parse+owned tokens+分段 layout+syntect 高亮,量级差
///   两个数量级。稳态帧若贵到冷帧的两成以上,说明每帧在偷偷全量重解析。
/// - **切换文本后首帧等于 miss 量级**:同 id 换文本必然 hash 不匹配 → 全量
///   重建(vendored 层单槽缓存的既定行为;M2 的改造落在 app 侧把槽位按
///   tab 分开,单槽语义不变,本断言维持原向)。
///
///   判据是 miss 帧与稳态帧的**倍数**,而这个倍数**随构建档位浮动**
///   (2026-10-16 重标定:dev 依赖栈开 `-O3` 后,稳态帧被加速得比 miss 帧
///   更多,倍数从 >5× 掉到 3.9×,原阈值 5.0 于是无理由地红)。倍数的语义
///   不变量是「miss 比 hit 贵一个量级」;改构建配置后要重新标定,别把红
///   当成性能退化。
#[test]
fn markdown_cache_hits_in_steady_state_and_misses_on_text_change() {
    let rendered_a = latermd_md::expand_wikilinks(&sample_doc(4, 300));
    let rendered_b = latermd_md::expand_wikilinks(&sample_doc(5, 300));
    let ctx = egui::Context::default();
    ctx.run_ui(RawInput::default(), |_| {})
        .drop_without_applying_deltas();

    let cold = label_frame(&ctx, &rendered_a);
    let mut hot = Vec::new();
    for _ in 0..7 {
        hot.push(label_frame(&ctx, &rendered_a));
    }
    let hot = median(hot);
    assert!(
        hot.as_secs_f64() < cold.as_secs_f64() * 0.2,
        "稳态帧 {hot:?} 应远低于冷帧 {cold:?}(缓存命中的时间证据;若红了说明每帧全量重解析)"
    );

    let switched = label_frame(&ctx, &rendered_b);
    // 3.0 的标定(2026-10-16,dev 依赖栈 `opt-level = 3`):稳态 ~520µs、
    // 换文本 ~2.0ms,实测比值 3.75–3.90,取 3.0 留约 25% 余量。
    //
    // 不用 `cold` 当尺子:它是**首帧**,46ms 里绝大部分是字体装载与
    // syntect `SyntaxSet` 的一次性初始化,不是 miss 路径的成本 —— 拿它比
    // 会把「预热」误读成「未命中」。
    assert!(
        switched.as_secs_f64() > hot.as_secs_f64() * 3.0,
        "换文本首帧 {switched:?} 应是 miss 量级(≫ 稳态 {hot:?},冷帧 {cold:?});若跌到同量级,说明切文本路径已有缓存,回归断言需要跟着改向"
    );
}

/// #39 M2 回归哨(切 tab 往返不重解析):per-tab widget id 下,两个 tab 的
/// vendored 缓存槽位互不覆盖,切走再切回必须命中自己槽位的既有产物 ——
/// 往返首帧与稳态同量级,而不是重新全量解析。egui 0.36 temp memory 无按帧
/// 回收(egui src/util/id_type_map.rs:temp 仅在 clear/remove 时清),缓存
/// 跨帧存活是这一机制的前提;时间断言口径同上一条(命中 < 冷帧 20%)。
#[test]
fn preview_cache_survives_tab_roundtrip_with_per_tab_ids() {
    let rendered_a = latermd_md::expand_wikilinks(&sample_doc(6, 300));
    let rendered_b = latermd_md::expand_wikilinks(&sample_doc(7, 300));
    let ctx = egui::Context::default();
    ctx.run_ui(RawInput::default(), |_| {})
        .drop_without_applying_deltas();

    let id_a = Id::new("preview-md").with(1_u64);
    let id_b = Id::new("preview-md").with(2_u64);

    let cold_a = label_frame_with_id(&ctx, &id_a, &rendered_a);
    let cold_b = label_frame_with_id(&ctx, &id_b, &rendered_b);
    // A→B→A 往返:切回 A 的一帧,不得重新全量解析
    let back_a = label_frame_with_id(&ctx, &id_a, &rendered_a);
    assert!(
        back_a.as_secs_f64() < cold_a.as_secs_f64() * 0.2,
        "往返切回 A 首帧 {back_a:?} 应命中 A 自己的缓存槽位(冷帧 {cold_a:?} 的 20% 以内);若红了说明 B 的渲染覆盖/清掉了 A 的缓存"
    );
    // 再切回 B 同样命中:B 的槽位也没被 A 的往返破坏
    let back_b = label_frame_with_id(&ctx, &id_b, &rendered_b);
    assert!(
        back_b.as_secs_f64() < cold_b.as_secs_f64() * 0.2,
        "再切回 B 首帧 {back_b:?} 应命中 B 自己的缓存槽位(冷帧 {cold_b:?} 的 20% 以内)"
    );
}
