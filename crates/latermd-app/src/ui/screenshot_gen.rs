//! README 截图生成器(无头):跑完整应用帧,软件光栅化(含**字体图集
//! 纹理采样**)输出 PNG。跑法:
//!
//! ```bash
//! cargo test -p latermd-app --release screenshots -- --ignored --nocapture
//! ```
//!
//! 产出写到 `assets/screenshot/`(目录在仓库内,README 引用相对路径)。
//!
//! 为什么是测试而不是 example:bin-only crate 的 example 链不到库内部
//! (`LaterMdApp`/`State` 都在 bin 里),`#[cfg(test)]` 模块才有全量访问
//! —— 照 `perf_finding`/`tab_switch_perf` 的取证先例。
//!
//! ## 光栅化口径(如实记录,不冒充 GPU)
//!
//! - **文字保真**:egui 的字形墨迹在字体图集纹理里自带 AA(alpha 覆盖),
//!   采样图集后文字边缘是真实的抗锯齿;
//! - **几何硬边**:tessellation 关 feathering(与像素验收测试同款),
//!   轴对齐矩形(本 UI 的绝大多数)硬边不可见;圆/斜线(箭头、圆角)
//!   无 AA,README 尺寸下可接受;
//! - **仅字体图集纹理被采样**(`TextureId::Default`);标题栏 logo 等
//!   管理纹理按顶点色不透明处理(占位),截图不含位图图片的文档场景
//!   不受影响;
//! - 直 alpha 源覆盖混合,画家序遍历 mesh —— 与像素验收的
//!   `final_covered_color` 同语义,只是逐像素而非采样点。

use eframe::egui;
use egui::epaint::{ClippedPrimitive, Primitive, TextureId};

use crate::state::Message;
use crate::theme::{ThemeMode, ThemeSettings};
use crate::LaterMdApp;

/// 截图分辨率(ppp=1.0;README 两栏展示够用)。
const SIZE: (u32, u32) = (1440, 900);

/// 演示文档:截图里看得见的全部构造(标题/段落/表格/代码/mermaid/
/// 任务列表/引用/行内格式),内容为真实功能描述而非占位文本。
const DEMO_DOC: &str = concat!(
    "# LaterMD 使用指南\n",
    "\n",
    "跨平台 Markdown 知识工作台:写作即渲染,单一解析器,版本化。\n",
    "\n",
    "## 核心能力\n",
    "\n",
    "- **Live Preview**:浏览态富渲染,点进即编辑\n",
    "- `latermd 文档.md` 命令行直开(三平台)\n",
    "- 双向链接 `[[另一篇笔记]]` 与全库搜索\n",
    "\n",
    "## 渲染管线\n",
    "\n",
    "```mermaid\n",
    "flowchart TD\n",
    "  A[源码缓冲] --> B{块级切分}\n",
    "  B -->|富渲染| C[浏览态]\n",
    "  B -->|光标块| D[编辑态]\n",
    "  C --> E[缓存与视口剔除]\n",
    "  D --> E\n",
    "```\n",
    "\n",
    "## 快捷键\n",
    "\n",
    "| 动作 | 快捷键 |\n",
    "|---|---|\n",
    "| 快速打开 | Cmd/Ctrl+P |\n",
    "| 切换主题 | Alt+T |\n",
    "| 退出禅定 | Esc |\n",
    "\n",
    "## 待办示例\n",
    "\n",
    "- [x] 大纲面板\n",
    "- [ ] mermaid 语法扩面\n",
    "\n",
    "> 「刷新或重开即恢复」类渲染问题,欢迎带截图反馈。\n",
    "\n",
    "```rust\n",
    "fn main() {\n",
    "    println!(\"你好,LaterMD\");\n",
    "}\n",
    "```\n",
);

/// 源码模式演示文档(带 gutter 行号的编辑视图)。
const SOURCE_DOC: &str = concat!(
    "# 源码模式\n",
    "\n",
    "写作 / 源码 / 分栏三档随时切换,共用同一缓冲与撤销栈。\n",
    "\n",
    "```mermaid\n",
    "flowchart LR\n",
    "  写作 --> 源码 --> 分栏\n",
    "```\n",
    "\n",
    "第三段正文:行内 **加粗**、*斜体*、`行内代码` 与 [链接](https://md.ailater.com)。\n",
);

/// 一张截图的场景与落盘名。
struct Shot {
    name: &'static str,
    doc: &'static str,
    dark: bool,
    source_mode: bool,
    outline: bool,
}

/// 生成全部截图。CJK 字体缺失时跳过(与像素验收同口径,不硬红)。
#[test]
#[ignore = "截图生成;跑法:cargo test -p latermd-app --release screenshots -- --ignored --nocapture"]
fn screenshots_for_readme() {
    let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/screenshot");
    std::fs::create_dir_all(&out_dir).expect("建 assets/screenshot 目录");

    let shots = [
        Shot {
            name: "live-dark-outline",
            doc: DEMO_DOC,
            dark: true,
            source_mode: false,
            outline: true,
        },
        Shot {
            name: "live-light-outline",
            doc: DEMO_DOC,
            dark: false,
            source_mode: false,
            outline: true,
        },
        Shot {
            name: "source-dark",
            doc: SOURCE_DOC,
            dark: true,
            source_mode: true,
            outline: false,
        },
    ];
    for shot in shots {
        let ctx = egui::Context::default();
        if crate::fonts::install(&ctx).is_none() {
            eprintln!("本机无 CJK 候选字体,截图生成跳过");
            return;
        }
        let png = render_app_frame(&ctx, &shot);
        let path = out_dir.join(format!("{}.png", shot.name));
        png.save(&path).expect("写 PNG");
        println!("screenshot: {}", path.display());
    }
}

/// 跑完整应用(归约+绘制)取一帧,光栅化为 RGBA 图像。
fn render_app_frame(ctx: &egui::Context, shot: &Shot) -> image::RgbaImage {
    let mut app = LaterMdApp::default();
    let index = app.state.tabs.open_tab(None, shot.doc);
    app.state.apply(Message::TabActivate(index));
    app.state.render_mode = if shot.source_mode {
        crate::live::RenderMode::Source
    } else {
        crate::live::RenderMode::Live
    };
    if shot.outline {
        app.state.layout.left = true;
    }
    let mode = if shot.dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    // 主题源头是 State(`app.draw` 每帧按 state 重投 visuals),只 apply
    // ctx 会被下一帧覆盖 —— 两处都要设(首轮 apply 让首帧预热即带对皮肤)。
    app.state.theme.mode = mode;
    ThemeSettings::default().apply(ctx, mode);
    ctx.options_mut(|o| o.tessellation_options.feathering = false);

    let screen = egui::Rect::from_min_size(
        egui::pos2(0.0, 0.0),
        egui::vec2(SIZE.0 as f32, SIZE.1 as f32),
    );
    let raw = egui::RawInput {
        screen_rect: Some(screen),
        ..egui::RawInput::default()
    };
    // 预热:字形按需光栅进图集,同文档多帧后图集稳定,末帧才是完整墨迹。
    for _ in 0..4 {
        ctx.run_ui(raw.clone(), |ui| {
            app.reduce(ui.ctx());
            app.draw(ui);
        })
        .drop_without_applying_deltas();
    }
    let mut output = ctx.run_ui(raw, |ui| {
        app.reduce(ui.ctx());
        app.draw(ui);
    });
    let clipped = ctx.tessellate(std::mem::take(&mut output.shapes), 1.0);
    output.drop_without_applying_deltas();
    rasterize(ctx, &clipped)
}

/// 软件光栅化:画家序遍历剪裁 mesh,字体图集纹理采样 + 直 alpha 覆盖。
fn rasterize(ctx: &egui::Context, clipped: &[ClippedPrimitive]) -> image::RgbaImage {
    // 图集快照先取(`FontsView::image` 给整幅 ColorImage;字形覆盖在
    // alpha 通道,借用结束后 mesh 采样不再需要 ctx)。
    let atlas = ctx.fonts(|fonts| fonts.image());
    let mut buffer = vec![egui::Color32::from_rgb(0, 0, 0); (SIZE.0 as usize) * (SIZE.1 as usize)];
    for piece in clipped {
        let Primitive::Mesh(mesh) = &piece.primitive else {
            continue;
        };
        let clip = piece.clip_rect;
        for tri in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = [
                mesh.vertices[tri[0] as usize],
                mesh.vertices[tri[1] as usize],
                mesh.vertices[tri[2] as usize],
            ];
            rasterize_triangle(&mut buffer, &atlas, mesh.texture_id, &clip, [a, b, c]);
        }
    }
    let mut png = image::RgbaImage::new(SIZE.0, SIZE.1);
    for (pixel, rgba) in png.pixels_mut().zip(buffer) {
        *pixel = image::Rgba([rgba.r(), rgba.g(), rgba.b(), rgba.a()]);
    }
    png
}

/// 单三角形光栅化:边函数覆盖 × 重心插值(uv + 顶点色),图集像素 alpha
/// 乘进源色后源覆盖混合。窗口外/剪裁外的像素点直接跳过。
fn rasterize_triangle(
    buffer: &mut [egui::Color32],
    atlas: &egui::ColorImage,
    texture: TextureId,
    clip: &egui::Rect,
    [a, b, c]: [egui::epaint::Vertex; 3],
) {
    let min_x = a
        .pos
        .x
        .min(b.pos.x)
        .min(c.pos.x)
        .floor()
        .max(clip.min.x)
        .max(0.0);
    let max_x = a
        .pos
        .x
        .max(b.pos.x)
        .max(c.pos.x)
        .ceil()
        .min(clip.max.x)
        .min(SIZE.0 as f32);
    let min_y = a
        .pos
        .y
        .min(b.pos.y)
        .min(c.pos.y)
        .floor()
        .max(clip.min.y)
        .max(0.0);
    let max_y = a
        .pos
        .y
        .max(b.pos.y)
        .max(c.pos.y)
        .ceil()
        .min(clip.max.y)
        .min(SIZE.1 as f32);
    if min_x >= max_x || min_y >= max_y {
        return;
    }
    let area =
        (b.pos.x - a.pos.x) * (c.pos.y - a.pos.y) - (c.pos.x - a.pos.x) * (b.pos.y - a.pos.y);
    if area.abs() < f32::EPSILON {
        return;
    }
    let inv_area = 1.0 / area;
    let sample = |uv: egui::Pos2| -> u8 {
        if texture != TextureId::default() {
            return 255;
        }
        // 插值舍入可能压线到 1.0,钳回最后一张纹素,避免图集边缘越界。
        let x = ((uv.x * atlas.size[0] as f32) as usize).min(atlas.size[0] - 1);
        let y = ((uv.y * atlas.size[1] as f32) as usize).min(atlas.size[1] - 1);
        atlas
            .pixels
            .get(y * atlas.size[0] + x)
            .map(|color| color.a())
            .unwrap_or(255)
    };
    let mut y = min_y;
    while y < max_y {
        let mut x = min_x;
        while x < max_x {
            // 像素中心采样(与 GPU 光栅化的质心口径近似)。
            let p = egui::pos2(x + 0.5, y + 0.5);
            let w0 = (b.pos.x - p.x) * (c.pos.y - p.y) - (c.pos.x - p.x) * (b.pos.y - p.y);
            let w1 = (c.pos.x - p.x) * (a.pos.y - p.y) - (a.pos.x - p.x) * (c.pos.y - p.y);
            let w2 = (a.pos.x - p.x) * (b.pos.y - p.y) - (b.pos.x - p.x) * (a.pos.y - p.y);
            let (w0, w1, w2) = (w0 * inv_area, w1 * inv_area, w2 * inv_area);
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                x += 1.0;
                continue;
            }
            let uv = egui::Pos2::new(
                a.uv.x * w0 + b.uv.x * w1 + c.uv.x * w2,
                a.uv.y * w0 + b.uv.y * w1 + c.uv.y * w2,
            );
            let ink = sample(uv) as f32 / 255.0;
            let lerp =
                |ca: u8, cb: u8, cc: u8| (ca as f32 * w0 + cb as f32 * w1 + cc as f32 * w2) as u8;
            let src = egui::Color32::from_rgba_unmultiplied(
                lerp(a.color.r(), b.color.r(), c.color.r()),
                lerp(a.color.g(), b.color.g(), c.color.g()),
                lerp(a.color.b(), b.color.b(), c.color.b()),
                ((lerp(a.color.a(), b.color.a(), c.color.a()) as f32) * ink) as u8,
            );
            blend(buffer, p, src);
            x += 1.0;
        }
        y += 1.0;
    }
}

/// 直 alpha 源覆盖混合。
fn blend(buffer: &mut [egui::Color32], p: egui::Pos2, src: egui::Color32) {
    let (x, y) = (p.x as usize, p.y as usize);
    if x >= SIZE.0 as usize || y >= SIZE.1 as usize {
        return;
    }
    let mut dst = buffer[y * SIZE.0 as usize + x];
    if dst.a() == 0 || src.a() == 255 {
        dst = src;
    } else {
        let sa = src.a() as f32 / 255.0;
        let mix = |s: u8, d: u8| (s as f32 * sa + d as f32 * (1.0 - sa)).round() as u8;
        dst = egui::Color32::from_rgba_unmultiplied(
            mix(src.r(), dst.r()),
            mix(src.g(), dst.g()),
            mix(src.b(), dst.b()),
            255,
        );
    }
    buffer[y * SIZE.0 as usize + x] = dst;
}
