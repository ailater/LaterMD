//! mermaid 围栏代码块的自定义 block widget(docs/auto-plan.md #51 M3)。
//!
//! 渲染链路:vendored `LinkHandler::is_block_code_widget`(vendor/README.md
//! 差异表 #7 的代码块级扩展点,```ai 指令卡同款)把 info string 首词为
//! `mermaid` 的围栏切出文本 galley,交给本模块 [`block_widget`] —— 解析与
//! 布局在 `latermd-render::mermaid`(M1,铁律 2:不含 egui 类型),这里只做
//! 「绘制指令 → egui painter」的翻译与缓存。一切失败路径(语法超出 v1
//! 子集、Live 流式围栏未闭合/heal 后仍非法、布局检测到环)回落为源码
//! 高亮代码块,绝不 panic。
//!
//! widget id 纪律(AGENTS.md §6.7):全部 id/缓存键由「外层 label id +
//! 块序号」派生,绝不含 `content.len()`;内容变化只改缓存条目携带的
//! 内容哈希(照 vendored 代码块 galley 缓存的 `hash_code_block_context`
//! 手法),流式追加 token 不清空任何相邻缓存。

use eframe::egui;
use egui::epaint::Shape;
use egui_markdown::layout::highlight_code;
use latermd_render::mermaid as md;
use latermd_render::mermaid::{Diagram, TextMeasurer};
use std::cell::RefCell;

/// 回落态提示行文案(样式见 [`paint_fallback`]:small + weak,克制;
/// hover 显示具体解析错误)。改文案只动这一处。
pub(crate) const FALLBACK_NOTE: &str = "mermaid 图形渲染不可用,已按源码显示";

/// 连线基准线宽(px;`==>` 粗线取两倍,随整图 scale 同步缩放)。
const EDGE_W: f32 = 1.5;
/// 虚线的实段长(px,随整图 scale 同步缩放)。
const DASH_LEN: f32 = 6.0;
/// 虚线的空段长(px,随整图 scale 同步缩放)。
const DASH_GAP: f32 = 4.0;
/// 箭头翼长(px,随整图 scale 同步缩放)。
const ARROW: f32 = 8.0;
/// 圆角矩形节点的圆角(px;跑道形圆角取盒高一半,菱形无圆角)。
const RADIUS_NODE: f32 = 6.0;
/// 边标签垫底矩形向四周扩展的半量(px)。
const LABEL_PAD: f32 = 3.0;

/// info string 首词判定:与 ```ai 指令卡(`ui::ai_card::is_instruction_info`)
/// 同一口径,`mermaid` 后跟元数据(如 `mermaid title=x`)不命中。
pub(crate) fn is_mermaid_info(language: Option<&str>) -> bool {
    language.is_some_and(|info| info.split_whitespace().next() == Some("mermaid"))
}

/// mermaid 块查看态(2026-10-24,需求「可切换代码/预览,预览只读」)的
/// temp 键:`false`(默认)= 图表(渲染图,整块只读);`true` = 源码
/// (高亮代码块,头带只读、代码体照常点进编辑)。会话级 UI 态,不持久化,
/// 键与布局缓存同源(外层 label id + 块序号)。
fn source_view_key(ui: &egui::Ui, index: usize) -> egui::Id {
    mermaid_widget_id(ui, index).with("src-view")
}

/// 头带高度:vendored `render_code_block` 的 CODE_BLOCK_HEADER_HEIGHT 是
/// 24,但那边按钮只有一枚且无选中底片,24 里居中仅剩 2px 上缝;mermaid
/// 头带是「切换组+删除+复制」三组,选中片贴着顶边显挤(2026-10-26 坤哥
/// 「按钮太靠上了」)——放宽到 28,居中后上下各 4px 呼吸。
const HEADER_HEIGHT: f32 = 28.0;

/// mermaid 块渲染入口(vendored `block_code_widget` 的分派目标)。
///
/// `index` 是本帧文档序的 mermaid 块序号(由调用方 handler 按渲染顺序
/// 递增,照 ```ai 卡的 `card_count` 模式)—— widget id 与缓存键的稳定
/// 成分,内容变化不改变它。`text` 是围栏内纯文本(vendored parser 已去
/// 块尾换行,不经任何 Markdown 再解析,铁律 1)。`close` 是可写列传入的
/// 删除入口(预览/估高臂为 None):进既有 CodeClose 确认弹窗通道。
///
/// 代码块同款外框(2026-10-26,坤哥「样式弄成和代码块类似」):代码底 +
/// 描边 + 圆角 + 顶部 24px 头带,功能按钮全在头带**右侧** —— 左→右:
/// 图表/源码切换、删除(🗑)、复制(⧉);绘图区垫「画纸」底色与块底
/// 区分。图表态含解析回落(回落显示的也是源码,但**意图**仍是图表)
/// 整块只读 —— 点击落图不进编辑,改图走「源码」钮;源码态只免头带,
/// 点代码体即进编辑。
pub(crate) fn block_widget(
    ui: &mut egui::Ui,
    index: usize,
    text: &str,
    close: Option<crate::ui::rich_widgets::CodeClose<'_>>,
) -> egui::Response {
    let style = egui_markdown::global_style(ui.ctx());
    let p = &style.code_block.padding;
    let font = diagram_font(ui);
    let view_key = source_view_key(ui, index);
    let show_source = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(view_key))
        .unwrap_or(false);
    let top = ui.cursor().top();
    let stroke = egui::Stroke::new(
        style.code_block.stroke_width,
        ui.visuals().widgets.noninteractive.bg_stroke.color,
    );
    let frame = egui::Frame::NONE
        .fill(ui.visuals().code_bg_color)
        .stroke(stroke)
        .corner_radius(style.code_block.corner_radius)
        .inner_margin(egui::Margin {
            left: p[0] as i8,
            top: (p[1] + HEADER_HEIGHT) as i8,
            right: p[2] as i8,
            bottom: p[3] as i8,
        });
    let frame_response = frame.show(ui, |ui| {
        // 撑满整列:图表态本就按可用宽画,源码态同宽 —— 两态切换框宽
        // 零变化(与 vendored 非收缩代码块同款)。
        ui.set_min_width(ui.available_width());
        if show_source {
            paint_source(ui, index, text, None, true)
        } else {
            match cached_diagram(ui, index, text, &font) {
                Ok(diagram) => paint_diagram(ui, index, &diagram, &font),
                Err(err) => paint_fallback(ui, index, text, &err),
            }
        }
    });
    let outer = frame_response.response.rect;
    // 头带覆盖在顶部内距带上(right_to_left,代码块头部同款):先加的
    // 贴右 —— 复制最右、删除次之、图表/源码切换组最左。lang 标签传空
    // 串隐藏:切换钮已表明内容,重复的「mermaid」只挤位置。
    let header_rect = egui::Rect::from_min_max(
        egui::pos2(outer.left(), outer.top()),
        egui::pos2(outer.right(), outer.top() + HEADER_HEIGHT),
    );
    let mut header_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(header_rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    crate::ui::rich_widgets::code_header_buttons(&mut header_ui, text, "", close);
    // right_to_left 先加贴右:欲读序「图表 源码」,先源码后图表。
    if view_toggle(
        &mut header_ui,
        show_source,
        "源码",
        true,
        "显示 mermaid 源码(点击代码进入编辑)",
    ) {
        header_ui.ctx().data_mut(|d| d.insert_temp(view_key, true));
        header_ui.ctx().request_repaint();
    }
    if view_toggle(
        &mut header_ui,
        !show_source,
        "图表",
        false,
        "渲染为流程图(只读)",
    ) {
        header_ui.ctx().data_mut(|d| d.insert_temp(view_key, false));
        header_ui.ctx().request_repaint();
    }
    let bottom = ui.cursor().top().max(top + 1.0);
    let full = egui::Rect::from_min_max(
        egui::pos2(ui.min_rect().left(), top),
        egui::pos2(ui.min_rect().right(), bottom),
    );
    register_read_only_rect(ui.ctx(), if show_source { header_rect } else { full });
    frame_response.response
}

/// 查看态小切换钮:`selected` 决定强调与弱化,`is_source` 只用于测试探针
/// 标注。命中返回 true(切换与重绘由调用方处理)。固定 20px 高自绘
/// (表格工具条同手法):selectable_label 的行盒吃主题行距投影,在 24px
/// 头带里会撑到 29px 溢出(2026-10-26 实测)。
fn view_toggle(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    is_source: bool,
    hint: &str,
) -> bool {
    let font = egui::TextStyle::Small.resolve(ui.style());
    let text_color = if selected {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let text_size = ui.fonts_mut(|f| {
        f.layout_no_wrap(label.to_owned(), font.clone(), text_color)
            .size()
    });
    let size = (text_size + egui::vec2(2.0 * crate::ui::tokens::SPACE_XS, 0.0))
        .max(egui::vec2(20.0, 20.0));
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let bg = if selected {
            Some(ui.visuals().selection.bg_fill)
        } else if response.hovered() {
            Some(ui.visuals().widgets.hovered.bg_fill)
        } else {
            None
        };
        if let Some(bg) = bg {
            painter.rect_filled(rect, crate::ui::tokens::RADIUS_SM, bg);
        }
        let galley = painter.layout(label.to_owned(), font, text_color, f32::INFINITY);
        painter.galley(rect.center() - galley.size() / 2.0, galley, text_color);
    }
    register_toggle_rect(ui.ctx(), rect, is_source);
    let clicked = response.clicked();
    response.on_hover_text(hint);
    clicked
}

/// 节点标签/边标签字体:预览正文同源(用户字号偏好 + 预览专用族,
/// #43 M2 的 CJK 逐族回退副本;bold 族有方块教训,绝不用 bold)。
fn diagram_font(ui: &egui::Ui) -> egui::FontId {
    egui::FontId::new(
        crate::theme::editor_font_size(ui.ctx()),
        crate::fonts::preview_body_family(ui.ctx()),
    )
}

// —— 解析布局缓存 ——

/// 缓存条目:内容哈希 + 字体指纹命中才复用(字号偏好/字体族变化会
/// 改变真实文字测量,必须重排)。`Err` 同样缓存——回落块每帧重解析
/// 失败是纯浪费,流式追加时内容哈希自然前进。
#[derive(Clone)]
struct CachedDiagram {
    content_hash: u64,
    font: egui::FontId,
    result: Result<Diagram, String>,
}

fn content_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// mermaid 块 widget/缓存键:外层 label id + 块序号(预览的 label id 含
/// tab 维度,Live 含块维度,两面板天然不撞)。绝不含内容长度/哈希
/// (AGENTS.md §6.7)。
fn mermaid_widget_id(ui: &egui::Ui, index: usize) -> egui::Id {
    ui.id().with(("mermaid-diagram", index))
}

fn cached_diagram(
    ui: &mut egui::Ui,
    index: usize,
    text: &str,
    font: &egui::FontId,
) -> Result<Diagram, String> {
    let hash = content_hash(text);
    let key = mermaid_widget_id(ui, index);
    if let Some(hit) = ui
        .data(|d| d.get_temp::<CachedDiagram>(key))
        .filter(|c| c.content_hash == hash && c.font == *font)
    {
        return hit.result;
    }
    // 真实字体度量注入布局(egui 0.36 的 glyph_width/row_height 借 &mut,
    // `TextMeasurer::measure` 又是 &self,RefCell 收口这处唯一的不对称):
    // 节点盒宽即文字实宽 + 内边距,不吃 M1 字符计数估算的逐字误差。
    let result = ui.ctx().fonts_mut(|fonts| {
        let measurer = FontMeasurer {
            fonts: RefCell::new(fonts),
            font_id: font.clone(),
        };
        md::render_with(text, &measurer).map_err(|err| err.to_string())
    });
    ui.data_mut(|d| {
        d.insert_temp(
            key,
            CachedDiagram {
                content_hash: hash,
                font: font.clone(),
                result: result.clone(),
            },
        )
    });
    result
}

/// [`TextMeasurer`] 的 egui 实现:预览族逐字符 advance 求和 + 同族行高。
/// `FontsView` 经 `Context::fonts_mut` 闭包取得(egui 0.36 未顶层重导出)。
struct FontMeasurer<'fonts, 'ctx> {
    fonts: RefCell<&'fonts mut egui::epaint::text::FontsView<'ctx>>,
    font_id: egui::FontId,
}

impl TextMeasurer for FontMeasurer<'_, '_> {
    fn measure(&self, text: &str) -> md::Size {
        let mut fonts = self.fonts.borrow_mut();
        let w: f32 = text
            .chars()
            .map(|c| fonts.glyph_width(&self.font_id, c))
            .sum();
        let h = fonts.row_height(&self.font_id);
        md::Size { w, h }
    }
}

// —— 图形绘制 ——

/// 把 M1 绘制指令翻译为 painter 调用。层序按指令的 z 槽:边 → 盒 →
/// 文字;颜色全部取当帧 visuals(明暗自适应,不落任何 RGB 字面值);
/// widget 高度 = 布局高度 × scale,宽度方向整图等比缩进面板宽、窄于
/// 面板时水平居中留白(取舍登记 decisions-pending)。
fn paint_diagram(
    ui: &mut egui::Ui,
    index: usize,
    diagram: &Diagram,
    font: &egui::FontId,
) -> egui::Response {
    let avail = ui.available_width().max(0.0);
    let bounds = diagram.bounds;
    // 布局产物的防御性收口:非有限/非正尺寸不画(留白占位),绝不 panic。
    if !bounds.w.is_finite() || !bounds.h.is_finite() || bounds.w <= 0.0 || bounds.h <= 0.0 {
        return ui
            .allocate_exact_size(egui::vec2(avail, 0.0), egui::Sense::hover())
            .1;
    }
    let scale = (avail / bounds.w).min(1.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(avail, bounds.h * scale), egui::Sense::hover());
    // 视口剔除(AGENTS.md §6.2):空间照常推进,paint 跳过。
    if !ui.is_rect_visible(rect) {
        return response;
    }
    // 「画纸」底色(2026-10-26,坤哥「绘图的背景色和这个块的背景色区分
    // 下」):外框是代码底,绘图区垫 **内容区底**(window_fill —— 亮色
    // 纯白 / 暗色 #292A2D,皮肤里与代码底恒反差;extreme_bg 在本应用皮肤
    // 里与代码底同 token,不可用),小圆角成画布感。
    ui.painter()
        .rect_filled(rect, 4.0, ui.visuals().window_fill);
    // 当帧 visuals 取色(须在 painter 借用 ui 之前取全):节点盒与既有
    // 卡片(AI 指令卡/代码块)同档的 inactive 底 + inactive 描边;连线/
    // 箭头取弱化前景,文字取正文色,边标签垫底用 faint 底。
    let fill = ui.visuals().widgets.inactive.bg_fill;
    let stroke = egui::Stroke::new(
        ui.visuals().widgets.inactive.bg_stroke.width.max(1.0),
        ui.visuals().widgets.inactive.bg_stroke.color,
    );
    let line_color = ui.visuals().weak_text_color();
    let text_color = ui.visuals().text_color();
    let label_bg = ui.visuals().faint_bg_color;

    let painter = ui.painter_at(rect);
    // 图内容水平居中(窄于面板时两侧留白;超面板宽时 scale 已夹到 1 以下)。
    let left = rect.min.x + ((avail - bounds.w * scale) / 2.0).max(0.0);
    let to_screen = |pt: md::Point| egui::pos2(left + pt.x * scale, rect.min.y + pt.y * scale);

    // 层 1:连线(实/虚/粗)与箭头。箭头方向取自最后两个折点,提前拷出
    // (实线分支 move 折线点集)。
    for edge in &diagram.edges {
        let pts: Vec<egui::Pos2> = edge.points.iter().map(|&p| to_screen(p)).collect();
        let tip_pair = pts.len().checked_sub(2).map(|i| (pts[i], pts[i + 1]));
        let width = EDGE_W
            * scale
            * if edge.style == md::LineStyle::Thick {
                2.0
            } else {
                1.0
            };
        let stroke = egui::Stroke::new(width, line_color);
        match edge.style {
            md::LineStyle::Dashed => painter.extend(Shape::dashed_line(
                &pts,
                stroke,
                DASH_LEN * scale,
                DASH_GAP * scale,
            )),
            md::LineStyle::Solid | md::LineStyle::Thick => {
                painter.line(pts, stroke);
            }
        }
        if edge.arrow {
            if let Some((tip, prev)) = tip_pair {
                paint_arrow(&painter, tip, prev, line_color, ARROW * scale);
            }
        }
    }

    // 层 2:节点盒(圆角矩形/跑道形/菱形)。
    let mut nodes: Vec<NodeProbe> = Vec::with_capacity(diagram.nodes.len());
    for node in &diagram.nodes {
        let r = egui::Rect::from_min_size(
            to_screen(node.rect.min),
            egui::vec2(node.rect.size.w * scale, node.rect.size.h * scale),
        );
        match node.shape {
            md::Shape::RoundedRect => {
                painter.rect(
                    r,
                    RADIUS_NODE * scale,
                    fill,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
            md::Shape::Stadium => {
                painter.rect(r, r.height() / 2.0, fill, stroke, egui::StrokeKind::Inside);
            }
            md::Shape::Diamond => {
                // 四顶点为包围盒各边中点(M1 语义);顺时针喂给凸多边形。
                let (cx, cy) = (r.center().x, r.center().y);
                painter.add(Shape::convex_polygon(
                    vec![
                        egui::pos2(cx, r.top()),
                        egui::pos2(r.right(), cy),
                        egui::pos2(cx, r.bottom()),
                        egui::pos2(r.left(), cy),
                    ],
                    fill,
                    stroke,
                ));
            }
        }
        nodes.push(NodeProbe {
            id: node.id.clone(),
            rect: r,
            text: egui::Rect::NAN,
        });
    }

    // 层 3:文字(节点标签最后画,压在盒上;边标签带垫底)。
    let label_font = egui::FontId::new((font.size * scale).max(6.0), font.family.clone());
    for (node, probe) in diagram.nodes.iter().zip(&mut nodes) {
        if node.label.is_empty() {
            continue;
        }
        let galley = painter.layout(
            node.label.clone(),
            label_font.clone(),
            text_color,
            f32::INFINITY,
        );
        // 布局测量与绘制同族同字号(未缩放时),文字落在内边距盒内;
        // 缩放路径下字形 advance 与盒同比缩小,残差由验收测试的宽松口径覆盖。
        let pos = to_screen(node.text_anchor) - galley.size() / 2.0;
        probe.text = egui::Rect::from_min_size(pos, galley.size());
        painter.galley(pos, galley, text_color);
    }
    for edge in &diagram.edges {
        let Some((label, anchor)) = edge.label.as_ref().zip(edge.label_anchor) else {
            continue;
        };
        let galley = painter.layout(label.clone(), label_font.clone(), text_color, f32::INFINITY);
        let pos = to_screen(anchor) - galley.size() / 2.0;
        let pad = egui::vec2(LABEL_PAD, LABEL_PAD);
        painter.rect_filled(
            egui::Rect::from_min_size(pos - pad, galley.size() + 2.0 * pad),
            RADIUS_NODE * 0.5,
            label_bg,
        );
        painter.galley(pos, galley, text_color);
    }

    write_probe(
        ui,
        index,
        ProbeBlock {
            rendered: true,
            error: None,
            source_view: false,
            nodes,
            edges: diagram.edges.len(),
        },
    );
    response
}

/// 终点实心三角箭头(方向由最后两个折点决定;零长度方向跳过)。
fn paint_arrow(
    painter: &egui::Painter,
    tip: egui::Pos2,
    prev: egui::Pos2,
    color: egui::Color32,
    wing: f32,
) {
    let dir = (tip - prev).normalized();
    if dir.length() <= 0.0 {
        return;
    }
    let perp = egui::vec2(-dir.y, dir.x);
    let base = tip - dir * wing;
    painter.add(Shape::convex_polygon(
        vec![tip, base + perp * wing * 0.5, base - perp * wing * 0.5],
        color,
        egui::Stroke::NONE,
    ));
}

// —— 源码视图与回落(共用嵌套 label 渲染)——

/// 嵌套 `MarkdownLabel` 渲染 mermaid 源码围栏。嵌套 label **不传
/// link_handler** → `is_block_code_widget` 恒 false → mermaid 围栏走
/// vendored 普通代码块路径:语法高亮与复制按钮(`code_copy_buttons`)
/// 原样复用,且天然不可能递归回本 widget。
///
/// 两个消费方:`source`(用户切到「源码」视图,无提示行,块可点进
/// 编辑)与 `fallback`(解析/布局失败含流式围栏未闭合的回落,带提示
/// 行,整块只读)。
/// 源码体:高亮 galley 直接铺在块外框的内容区(2026-10-26 外框上收后
/// 不再嵌套 MarkdownLabel —— 那会画出第二层代码框;超宽横向滚动,与
/// 代码块正文同管线同参)。
fn paint_source(
    ui: &mut egui::Ui,
    index: usize,
    text: &str,
    note: Option<(&str, &str)>,
    source_view: bool,
) -> egui::Response {
    let note_response = note.map(|(note_text, err)| {
        ui.label(egui::RichText::new(note_text).small().weak())
            .on_hover_text(err)
    });
    let style = egui_markdown::global_style(ui.ctx());
    let mut job = highlight_code(ui, text, "mermaid", style.code_font_size, None);
    job.wrap.max_width = f32::INFINITY;
    job.break_on_newline = true;
    job.keep_trailing_whitespace = true;
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    egui::ScrollArea::horizontal()
        .id_salt(mermaid_widget_id(ui, index).with("src-scroll"))
        .show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
            ui.painter()
                .galley(rect.min, galley, ui.visuals().text_color());
        });
    write_probe(
        ui,
        index,
        ProbeBlock {
            rendered: false,
            error: note.map(|(_, err)| err.to_owned()),
            source_view,
            nodes: Vec::new(),
            edges: 0,
        },
    );
    // 提示行响应只是悬停提示载体,块交互响应取外框本体的。
    note_response.unwrap_or_else(|| ui.response())
}

/// 解析/布局失败(含 Live 流式围栏未闭合)的回落:一行克制提示
/// (small + weak,hover 可看具体错误)+ 源码视图。
fn paint_fallback(ui: &mut egui::Ui, index: usize, text: &str, err: &str) -> egui::Response {
    paint_source(ui, index, text, Some((FALLBACK_NOTE, err)), false)
}

// —— 探针(生产只写不读;无头验收读,照 copy_button_probe 手法) ——

/// 单节点探针:盒屏幕矩形 + 标签 galley 屏幕矩形。
struct NodeProbe {
    id: String,
    rect: egui::Rect,
    text: egui::Rect,
}

/// 单块探针载荷。
struct ProbeBlock {
    rendered: bool,
    error: Option<String>,
    /// 本帧处于「源码」视图(用户切换;回落态为 false —— 回落显示源码
    /// 但意图仍是图表)。
    source_view: bool,
    nodes: Vec<NodeProbe>,
    edges: usize,
}

/// 探针在 egui data 的键:(帧号, 本帧各 mermaid 块探针清单)。全局单份,
/// 帧号变了就整帧重开;测试在帧外读,按「最后写入者即本帧」取。
fn probe_key() -> egui::Id {
    egui::Id::new("latermd-mermaid-probe")
}

fn write_probe(ui: &egui::Ui, index: usize, block: ProbeBlock) {
    let key = probe_key();
    let frame = ui.ctx().cumulative_pass_nr();
    let widget_id = mermaid_widget_id(ui, index);
    let mut blocks = ui
        .ctx()
        .data(|d| d.get_temp::<(u64, Vec<ProbeSnapshot>)>(key))
        .filter(|(seen, _)| *seen == frame)
        .map(|(_, blocks)| blocks)
        .unwrap_or_default();
    blocks.push(ProbeSnapshot {
        widget_id,
        rendered: block.rendered,
        error: block.error,
        source_view: block.source_view,
        nodes: block
            .nodes
            .into_iter()
            .map(|n| (n.id, n.rect, n.text))
            .collect(),
        edges: block.edges,
    });
    ui.ctx().data_mut(|d| d.insert_temp(key, (frame, blocks)));
}

/// 探针快照:widget id(断言 id 稳定性用)+ 渲染态 + 节点几何。
/// 仅测试构建存在消费(照 `ScrollProbe` 模式豁免 dead_code)。
#[derive(Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct ProbeSnapshot {
    pub(crate) widget_id: egui::Id,
    pub(crate) rendered: bool,
    pub(crate) error: Option<String>,
    pub(crate) source_view: bool,
    pub(crate) nodes: Vec<(String, egui::Rect, egui::Rect)>,
    pub(crate) edges: usize,
}

// —— 只读排除与切换钮探针(照 copy_button / table_toolbar 同手法)——

fn read_only_rects_id() -> egui::Id {
    egui::Id::new("latermd-mermaid-readonly-rects")
}

/// 记录本帧 mermaid 块的只读矩形(图表态=整块,源码态=仅头带);帧号
/// 变了重开清单。`live::clicked_for_edit` 按它把「预览只读」落地:点击
/// 落图(或头带)不连带切入编辑。`pub(crate)` 供 live 侧测试注入。
pub(crate) fn register_read_only_rect(ctx: &egui::Context, rect: egui::Rect) {
    let id = read_only_rects_id();
    let frame = ctx.cumulative_pass_nr();
    let mut rects = ctx
        .data(|d| d.get_temp::<(u64, Vec<egui::Rect>)>(id))
        .filter(|(seen, _)| *seen == frame)
        .map(|(_, rects)| rects)
        .unwrap_or_default();
    rects.push(rect);
    ctx.data_mut(|d| d.insert_temp(id, (frame, rects)));
}

/// 本帧 mermaid 只读矩形清单(帧号核对通过才返回;生产读法)。
pub(crate) fn read_only_rects(ctx: &egui::Context) -> Vec<egui::Rect> {
    let id = read_only_rects_id();
    let frame = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<(u64, Vec<egui::Rect>)>(id))
        .filter(|(seen, _)| *seen == frame)
        .map(|(_, rects)| rects)
        .unwrap_or_default()
}

/// 原始探针(最后写入帧,不做帧号核对):测试在 run_ui 之后读取。
#[cfg(test)]
pub(crate) fn read_only_probe(ctx: &egui::Context) -> Vec<egui::Rect> {
    ctx.data(|d| d.get_temp::<(u64, Vec<egui::Rect>)>(read_only_rects_id()))
        .map(|(_, rects)| rects)
        .unwrap_or_default()
}

fn toggle_probe_id() -> egui::Id {
    egui::Id::new("latermd-mermaid-toggle-rects")
}

/// 记录本帧「图表/源码」切换钮的矩形与身份(测试找按钮用)。
fn register_toggle_rect(ctx: &egui::Context, rect: egui::Rect, is_source: bool) {
    let id = toggle_probe_id();
    let frame = ctx.cumulative_pass_nr();
    let mut rects = ctx
        .data(|d| d.get_temp::<(u64, Vec<(egui::Rect, bool)>)>(id))
        .filter(|(seen, _)| *seen == frame)
        .map(|(_, rects)| rects)
        .unwrap_or_default();
    rects.push((rect, is_source));
    ctx.data_mut(|d| d.insert_temp(id, (frame, rects)));
}

/// 原始探针(最后写入帧,不做帧号核对):测试在 run_ui 之后读取。
#[cfg(test)]
pub(crate) fn toggle_probe(ctx: &egui::Context) -> Vec<(egui::Rect, bool)> {
    ctx.data(|d| d.get_temp::<(u64, Vec<(egui::Rect, bool)>)>(toggle_probe_id()))
        .map(|(_, rects)| rects)
        .unwrap_or_default()
}

/// 测试读侧:本帧(最后写入帧)的全部 mermaid 块探针(preview/live
/// 入口的接线测试也读它)。
#[cfg(test)]
pub(crate) fn read_probe(ctx: &egui::Context) -> Vec<ProbeSnapshot> {
    ctx.data(|d| {
        d.get_temp::<(u64, Vec<ProbeSnapshot>)>(probe_key())
            .map(|(_, blocks)| blocks)
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts;
    use crate::theme::{ThemeMode, ThemeSettings};
    use eframe::egui::RawInput;
    use egui_markdown::MarkdownLabel;

    /// TD 验收样例(与 M1 单测同图):4 盒 4 边,三种形状 + 两条带标签边。
    const TD_DOC: &str = concat!(
        "正文段落,mermaid 验收。\n\n",
        "```mermaid\n",
        "flowchart TD\n",
        "  A[开始] --> B{判断}\n",
        "  B -->|是| C(处理)\n",
        "  B -->|否| D[结束]\n",
        "  C --> D\n",
        "```\n\n",
        "收尾正文。\n",
    );

    /// LR 验收样例:同图换向。
    const LR_DOC: &str = concat!(
        "```mermaid\n",
        "flowchart LR\n",
        "  A[开始] --> B{判断}\n",
        "  B -->|是| C(处理)\n",
        "  B -->|否| D[结束]\n",
        "  C --> D\n",
        "```\n",
    );

    /// 回落验收样例:sequenceDiagram(v1 不支持的图类型,显式 Err)。
    const SEQ_DOC: &str = "```mermaid\nsequenceDiagram\nA->>B: hi\n```\n";

    /// 否决线文档:rust 围栏 + 无语言围栏 + 正文 + 表格,零 mermaid。
    const PLAIN_DOC: &str = concat!(
        "正文段落,否决线验收。\n\n",
        "```rust\nfn main() {\n    println!(\"你好,世界\");\n}\n```\n\n",
        "```\n无语言围栏,按默认语言高亮。\n```\n\n",
        "| 列一 | 列二 |\n|---|---|\n| 数据甲 | 数值一 |\n\n",
        "收尾正文段落。\n",
    );

    /// 无 CJK 环境跳过(#43 口径:像素/几何验收没有混排对象时不硬红)。
    fn cjk_fonts_missing() -> bool {
        fonts::install(&egui::Context::default()).is_none()
    }

    /// 一帧渲染的证据:mermaid 探针、mesh 不透明顶点色集合、断言用色、
    /// 复制按钮 rect、code_bg 色像素的包围盒(普通代码块背景签名)。
    struct FrameEvidence {
        probe: Vec<ProbeSnapshot>,
        colors: Vec<egui::Color32>,
        node_fill: egui::Color32,
        code_bg: egui::Color32,
        /// 绘图区画纸底(2026-10-26 外框验收:与块代码底恒可区分)。
        paper: egui::Color32,
        copy_buttons: Vec<egui::Rect>,
        code_bg_bbox: Option<(f32, f32, f32, f32)>,
        /// code_bg 顶点清单(2026-10-26 mermaid 外框也涂代码底后,整体
        /// bbox 是多块的并集;按 y 分隔单块的断言用这份原始清单)。
        code_bg_vertices: Vec<(f32, f32)>,
    }

    /// 生产链路渲染一帧:出厂字体 + `ThemeSettings::default().apply` +
    /// `MarkdownLabel`(+ 两栏共用的 [`crate::ui::ai_card::RichLinkHandler`],
    /// 不开 AI 时 mermaid 分派与原 LiveMermaidHandler 逐字节等价)。
    /// `with_handler = false` 是「接入前」对照轮(否决线用)。
    fn render_frame(
        ctx: &egui::Context,
        dark: bool,
        doc: &str,
        label_id: &'static str,
        with_handler: bool,
    ) -> FrameEvidence {
        render_frame_events(ctx, dark, doc, label_id, with_handler, Vec::new())
    }

    /// [`render_frame`] 的注入事件版:切换钮点击等交互验收用(照
    /// `fallback_copy_button_copies_mermaid_source` 的点击注入手法)。
    fn render_frame_events(
        ctx: &egui::Context,
        dark: bool,
        doc: &str,
        label_id: &'static str,
        with_handler: bool,
        events: Vec<egui::Event>,
    ) -> FrameEvidence {
        ThemeSettings::default().apply(
            ctx,
            if dark {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            },
        );
        ctx.options_mut(|o| o.tessellation_options.feathering = false);
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(700.0, 900.0));
        let raw = RawInput {
            screen_rect: Some(screen),
            events,
            ..RawInput::default()
        };
        let (mut node_fill, mut code_bg, mut paper) = (
            egui::Color32::BLACK,
            egui::Color32::WHITE,
            egui::Color32::WHITE,
        );
        let mut output = ctx.run_ui(raw, |panel| {
            node_fill = panel.visuals().widgets.inactive.bg_fill;
            code_bg = panel.visuals().code_bg_color;
            paper = panel.visuals().window_fill;
            let font = egui::FontId::new(
                crate::theme::editor_font_size(panel.ctx()),
                fonts::preview_body_family(panel.ctx()),
            );
            let label = MarkdownLabel::new(egui::Id::new(label_id), doc)
                .font(font)
                .wrap();
            if with_handler {
                // 链式调用让 handler 借用只活在本语句内。
                let handler = crate::ui::ai_card::RichLinkHandler::new(
                    crate::ui::rich_widgets::TaskProbeDomain::Preview,
                    dark,
                    panel.visuals().text_color(),
                    Default::default(),
                );
                label.link_handler(&handler).show(panel);
            } else {
                label.show(panel);
            }
        });
        let primitives = ctx.tessellate(std::mem::take(&mut output.shapes), 1.0);
        output.drop_without_applying_deltas();
        let mut colors = Vec::new();
        let mut bbox: Option<(f32, f32, f32, f32)> = None;
        let mut code_bg_vertices = Vec::new();
        for cp in &primitives {
            if let egui::epaint::Primitive::Mesh(mesh) = &cp.primitive {
                for vertex in &mesh.vertices {
                    if vertex.color.a() > 200 && !colors.contains(&vertex.color) {
                        colors.push(vertex.color);
                    }
                    if vertex.color == code_bg {
                        let (x, y) = (vertex.pos.x, vertex.pos.y);
                        code_bg_vertices.push((x, y));
                        bbox = Some(match bbox {
                            None => (x, y, x, y),
                            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                        });
                    }
                }
            }
        }
        FrameEvidence {
            probe: read_probe(ctx),
            colors,
            node_fill,
            code_bg,
            paper,
            copy_buttons: crate::ui::rich_widgets::copy_button_probe(ctx).1,
            code_bg_bbox: bbox,
            code_bg_vertices,
        }
    }

    fn fresh_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        assert!(
            fonts::install(&ctx).is_some(),
            "调用方须先经 cjk_fonts_missing() 跳过"
        );
        ctx
    }

    /// 宽松盒包含断言(±1px 量化容差):内矩形完全在外矩形内。
    fn rect_inside(inner: egui::Rect, outer: egui::Rect, eps: f32) -> bool {
        inner.min.x >= outer.min.x - eps
            && inner.min.y >= outer.min.y - eps
            && inner.max.x <= outer.max.x + eps
            && inner.max.y <= outer.max.y + eps
    }

    /// #51 M3 验收 1(TD):盒数/边数正确、节点盒纵坐标随边递增、
    /// 文字在盒内(宽松),明暗两主题各一轮,节点盒底色取当帧 visuals
    /// (两主题不同 = 明暗自适应的像素证据,非硬编码)。
    #[test]
    fn td_geometry_and_ink_in_both_visuals() {
        if cjk_fonts_missing() {
            eprintln!("本机无 CJK 候选字体,mermaid 验收无混排对象,跳过");
            return;
        }
        let mut fills = Vec::new();
        for dark in [true, false] {
            let name = if dark { "暗色" } else { "亮色" };
            let ctx = fresh_ctx();
            let ev = render_frame(&ctx, dark, TD_DOC, "mermaid-td-acceptance", true);
            fills.push(ev.node_fill);
            let [block] = ev.probe.as_slice() else {
                panic!(
                    "{name}:应有且仅有一个 mermaid 块探针,实际 {:?}",
                    ev.probe.len()
                );
            };
            assert!(block.rendered, "{name}:合法 flowchart 应出图");
            assert_eq!(
                (block.nodes.len(), block.edges),
                (4, 4),
                "{name}:TD 样例应 4 盒 4 边"
            );
            let center = |id: &str| {
                block
                    .nodes
                    .iter()
                    .find(|(nid, _, _)| nid == id)
                    .unwrap_or_else(|| panic!("{name}:缺节点 {id}"))
                    .1
                    .center()
            };
            // 纵坐标随边递增:A→B→C→D 分层严格下行(同层 C/D 在此图不同层)。
            for (upper, lower) in [("A", "B"), ("B", "C"), ("C", "D")] {
                assert!(
                    center(lower).y > center(upper).y,
                    "{name}:TD 应自上而下({upper} y={:?} < {lower} y={:?})",
                    center(upper).y,
                    center(lower).y
                );
            }
            // 文字在盒内(宽松 ±1px:布局测量与 galley 同字号同族,
            // 未缩放路径下应当严格落进内边距区)。
            for (id, node_rect, text_rect) in &block.nodes {
                assert!(
                    rect_inside(*text_rect, *node_rect, 1.0),
                    "{name}:节点 {id} 文字 {text_rect:?} 应在盒 {node_rect:?} 内"
                );
            }
            // 像素层:节点盒底色出现在 mesh(盒画出来了),文字色顶点存在。
            assert!(
                ev.colors.contains(&ev.node_fill),
                "{name}:mesh 应含节点盒底色 {:?}(实际 {:?})",
                ev.node_fill,
                ev.colors
            );
        }
        assert_ne!(
            fills[0], fills[1],
            "明暗两主题的节点盒底色应不同(visuals 自适应)"
        );
    }

    /// #51 M3 验收 2(LR):同图换向,节点盒横坐标随边递增,明暗两轮。
    #[test]
    fn lr_geometry_flows_rightward_in_both_visuals() {
        if cjk_fonts_missing() {
            eprintln!("本机无 CJK 候选字体,mermaid 验收无混排对象,跳过");
            return;
        }
        for dark in [true, false] {
            let name = if dark { "暗色" } else { "亮色" };
            let ctx = fresh_ctx();
            let ev = render_frame(&ctx, dark, LR_DOC, "mermaid-lr-acceptance", true);
            let [block] = ev.probe.as_slice() else {
                panic!("{name}:应有且仅有一个 mermaid 块探针");
            };
            assert!(block.rendered, "{name}:LR 样例应出图");
            assert_eq!((block.nodes.len(), block.edges), (4, 4), "{name}:4 盒 4 边");
            let center = |id: &str| {
                block
                    .nodes
                    .iter()
                    .find(|(nid, _, _)| nid == id)
                    .unwrap_or_else(|| panic!("{name}:缺节点 {id}"))
                    .1
                    .center()
            };
            for (left, right) in [("A", "B"), ("B", "C"), ("C", "D")] {
                assert!(
                    center(right).x > center(left).x,
                    "{name}:LR 应从左到右({left} x={:?} < {right} x={:?})",
                    center(left).x,
                    center(right).x
                );
            }
            for (id, node_rect, text_rect) in &block.nodes {
                assert!(
                    rect_inside(*text_rect, *node_rect, 1.0),
                    "{name}:节点 {id} 文字 {text_rect:?} 应在盒 {node_rect:?} 内"
                );
            }
        }
    }

    /// #51 M3 验收 3(回落):不支持类型(sequenceDiagram)按源码代码块
    /// 渲染——提示行 hover 带错误、代码块背景与复制按钮都在、没有节点
    /// 盒底色(未出图),明暗两轮。
    #[test]
    fn unsupported_diagram_falls_back_to_source_block() {
        if cjk_fonts_missing() {
            eprintln!("本机无 CJK 候选字体,mermaid 回落验收跳过");
            return;
        }
        for dark in [true, false] {
            let name = if dark { "暗色" } else { "亮色" };
            let ctx = fresh_ctx();
            let ev = render_frame(&ctx, dark, SEQ_DOC, "mermaid-seq-fallback", true);
            let [block] = ev.probe.as_slice() else {
                panic!("{name}:应有且仅有一个 mermaid 块探针");
            };
            assert!(!block.rendered, "{name}:sequenceDiagram 应回落源码");
            let err = block.error.as_deref().unwrap_or_default();
            assert!(!err.is_empty(), "{name}:回落探针应带解析错误");
            assert!(
                err.starts_with("mermaid"),
                "{name}:错误信息应可读(实际 {err})"
            );
            assert!(block.nodes.is_empty(), "{name}:回落态不应有节点探针");
            // 源码代码块签名:code_bg 色确实进了 mesh(回落块真的按代码
            // 块画了,不是只占位),且包围盒是 ≥60px 宽的矩形背景。
            assert!(
                ev.colors.contains(&ev.code_bg),
                "{name}:mesh 应含代码块背景色 {:?}",
                ev.code_bg
            );
            let (x0, y0, x1, y1) = ev
                .code_bg_bbox
                .unwrap_or_else(|| panic!("{name}:回落块应有代码块背景色"));
            assert!(
                x1 - x0 >= 60.0 && y1 - y0 >= 20.0,
                "{name}:代码块背景应成块(实际 {x1:.0}×{y1:.0})"
            );
            // 复制按钮经嵌套 label 的 code_block_buttons 挂载(仍可用)。
            assert!(!ev.copy_buttons.is_empty(), "{name}:回落块应保留复制按钮");
        }
    }

    /// 代码块同款外框验收(2026-10-26,坤哥「样式弄成和代码块类似 +
    /// 功能按钮都弄到右侧 + 绘图背景与块底区分」):块底 = 代码底,绘图
    /// 区垫画纸底(extreme_bg,与代码底恒反差);头带三组功能钮(图表/
    /// 源码切换、删除、复制)全部落在头带(顶部 24px)**右半**。
    #[test]
    fn mermaid_block_has_code_frame_with_right_aligned_header() {
        if cjk_fonts_missing() {
            eprintln!("本机无 CJK 字体,mermaid 外框验收跳过");
            return;
        }
        let ctx = fresh_ctx();
        let ev = render_frame(&ctx, false, TD_DOC, "mermaid-chrome", true);
        let [block] = ev.probe.as_slice() else {
            panic!("应有且仅有一个 mermaid 块探针");
        };
        assert!(block.rendered, "合法图应出图");
        // 块底 = 代码底;画纸底在场且与块底、节点底均可区分。
        assert!(ev.colors.contains(&ev.code_bg), "外框应涂代码底");
        assert!(
            ev.colors.contains(&ev.paper),
            "绘图区应垫画纸底 {:?}(实际 {:?})",
            ev.paper,
            ev.colors
        );
        assert_ne!(ev.paper, ev.code_bg, "画纸底应与块底可区分");
        assert_ne!(ev.paper, ev.node_fill, "画纸底应与节点底可区分");
        // 头带几何:整体代码底 bbox ≈ 外框;全部按钮 rect 落在顶部 24px
        // 带的右半(预览 handler 不挂删除 → 探针只有复制一枚 + 切换两枚)。
        let (bx0, by0, bx1, _) = ev.code_bg_bbox.expect("外框应有代码底");
        assert!(
            bx1 - bx0 >= 300.0,
            "外框应撑满整列(实际 {bx0:.0}..{bx1:.0})"
        );
        for rect in &ev.copy_buttons {
            assert!(
                rect.top() >= by0 - 2.0 && rect.bottom() <= by0 + 26.0,
                "按钮应落在头带({rect:?} vs 带顶 {by0:.0})"
            );
            assert!(
                rect.left() > (bx0 + bx1) * 0.5,
                "按钮应在外框右半({rect:?} vs 中线 {:.0})",
                (bx0 + bx1) * 0.5
            );
        }
        for (rect, _) in toggle_probe(&ctx) {
            assert!(
                rect.top() >= by0 - 2.0 && rect.bottom() <= by0 + 26.0,
                "切换钮应落在头带({rect:?} vs 带顶 {by0:.0})"
            );
            assert!(
                rect.left() > (bx0 + bx1) * 0.5,
                "切换钮应在外框右半({rect:?})"
            );
        }
        assert_eq!(ev.copy_buttons.len(), 1, "预览面只有复制钮");
        assert_eq!(toggle_probe(&ctx).len(), 2, "图表/源码两枚切换钮");
    }

    /// 删除通道(2026-10-26):可写列 handler 挂 code_close 后,块头第二
    /// 枚按钮(删除,探针紧随复制之后)点击把块序号推进 CodeClose sink
    /// —— 预览面(未挂)无删除钮。
    #[test]
    fn mermaid_header_delete_pushes_block_index_to_sink() {
        if cjk_fonts_missing() {
            eprintln!("本机无 CJK 字体,mermaid 删除验收跳过");
            return;
        }
        use std::cell::RefCell;
        use std::rc::Rc;
        let ctx = fresh_ctx();
        ThemeSettings::default().apply(&ctx, ThemeMode::Dark);
        ctx.options_mut(|o| o.tessellation_options.feathering = false);
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(700.0, 900.0));
        let sink = Rc::new(RefCell::new(Vec::new()));
        let render = |events: Vec<egui::Event>| {
            let raw = RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            };
            let out = ctx.run_ui(raw, |panel| {
                let handler = crate::ui::ai_card::RichLinkHandler::new(
                    crate::ui::rich_widgets::TaskProbeDomain::Preview,
                    false,
                    egui::Color32::WHITE,
                    Default::default(),
                )
                .with_code_close(7, Rc::clone(&sink));
                MarkdownLabel::new(egui::Id::new("mermaid-delete"), TD_DOC)
                    .font(egui::FontId::new(
                        crate::theme::editor_font_size(panel.ctx()),
                        fonts::preview_body_family(panel.ctx()),
                    ))
                    .wrap()
                    .link_handler(&handler)
                    .show(panel);
            });
            out.drop_without_applying_deltas();
        };
        render(Vec::new());
        let buttons = crate::ui::rich_widgets::copy_button_probe(&ctx).1;
        assert!(buttons.len() >= 2, "头带应有复制 + 删除两枚:{buttons:?}");
        let close_rect = buttons[1];
        render(vec![
            egui::Event::PointerMoved(close_rect.center()),
            egui::Event::PointerButton {
                pos: close_rect.center(),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerButton {
                pos: close_rect.center(),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ]);
        assert_eq!(
            sink.borrow().as_slice(),
            &[7],
            "点删除钮应把块序号推进 CodeClose sink"
        );
    }

    /// 源码态下点删除钮,源码视图标志不得被重置(2026-10-26 真机
    /// 「取消删除后视图跳回图表」回归)。
    #[test]
    fn delete_click_keeps_source_view_state() {
        if cjk_fonts_missing() {
            eprintln!("本机无 CJK 字体,mermaid 删除回归跳过");
            return;
        }
        use std::cell::RefCell;
        use std::rc::Rc;
        let ctx = fresh_ctx();
        ThemeSettings::default().apply(&ctx, ThemeMode::Dark);
        ctx.options_mut(|o| o.tessellation_options.feathering = false);
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(700.0, 900.0));
        let sink = Rc::new(RefCell::new(Vec::new()));
        let render = |events: Vec<egui::Event>| {
            let raw = RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            };
            let out = ctx.run_ui(raw, |panel| {
                let handler = crate::ui::ai_card::RichLinkHandler::new(
                    crate::ui::rich_widgets::TaskProbeDomain::Preview,
                    false,
                    egui::Color32::WHITE,
                    Default::default(),
                )
                .with_code_close(3, Rc::clone(&sink));
                MarkdownLabel::new(egui::Id::new("mermaid-del-src"), TD_DOC)
                    .font(egui::FontId::new(
                        crate::theme::editor_font_size(panel.ctx()),
                        fonts::preview_body_family(panel.ctx()),
                    ))
                    .wrap()
                    .link_handler(&handler)
                    .show(panel);
            });
            out.drop_without_applying_deltas();
        };
        let click = |pos: egui::Pos2| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ]
        };
        render(Vec::new());
        assert!(!read_probe(&ctx)[0].source_view, "默认图表态");
        // 切「源码」。
        let src_toggle = toggle_probe(&ctx)
            .into_iter()
            .find(|(_, is_source)| *is_source)
            .map(|(rect, _)| rect)
            .expect("块头应挂出「源码」切换钮");
        render(click(src_toggle.center()));
        render(Vec::new());
        assert!(read_probe(&ctx)[0].source_view, "已切到源码态");
        // 点删除钮(按钮探针第二枚,复制之后)→ sink 进块序号,且视图
        // 仍是源码态。
        let buttons = crate::ui::rich_widgets::copy_button_probe(&ctx).1;
        assert!(buttons.len() >= 2, "头带应有复制+删除:{buttons:?}");
        render(click(buttons[1].center()));
        render(Vec::new());
        assert_eq!(sink.borrow().as_slice(), &[3], "删除点击应进 sink");
        assert!(
            read_probe(&ctx)[0].source_view,
            "删除点击后源码态不得被重置"
        );
    }

    /// #51 M3 验收 4(回落复制可用):点回落块头部的复制按钮 → 整块
    /// mermaid 源码进剪贴板(与普通代码块同一条 `code_copy_buttons` 路)。
    #[test]
    fn fallback_copy_button_copies_mermaid_source() {
        if cjk_fonts_missing() {
            eprintln!("本机无 CJK 候选字体,mermaid 复制验收跳过");
            return;
        }
        let ctx = fresh_ctx();
        ThemeSettings::default().apply(&ctx, ThemeMode::Dark);
        ctx.options_mut(|o| o.tessellation_options.feathering = false);
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(700.0, 900.0));
        // 第一帧:拿复制按钮位置(探针)。
        let first = ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..RawInput::default()
            },
            |panel| {
                let handler = crate::ui::ai_card::RichLinkHandler::new(
                    crate::ui::rich_widgets::TaskProbeDomain::Preview,
                    false,
                    egui::Color32::WHITE,
                    Default::default(),
                );
                MarkdownLabel::new(egui::Id::new("mermaid-copy"), SEQ_DOC)
                    .font(egui::FontId::new(
                        crate::theme::editor_font_size(panel.ctx()),
                        fonts::preview_body_family(panel.ctx()),
                    ))
                    .wrap()
                    .link_handler(&handler)
                    .show(panel);
            },
        );
        first.drop_without_applying_deltas();
        let (_, buttons) = crate::ui::rich_widgets::copy_button_probe(&ctx);
        let Some(button) = buttons.first().copied() else {
            panic!("回落块应挂出复制按钮");
        };
        // 第二帧:在按钮中心完成一次主键点击。
        let click = button.center();
        let second = ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                events: vec![
                    egui::Event::PointerMoved(click),
                    egui::Event::PointerButton {
                        pos: click,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::default(),
                    },
                    egui::Event::PointerButton {
                        pos: click,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
                ..RawInput::default()
            },
            |panel| {
                let handler = crate::ui::ai_card::RichLinkHandler::new(
                    crate::ui::rich_widgets::TaskProbeDomain::Preview,
                    false,
                    egui::Color32::WHITE,
                    Default::default(),
                );
                MarkdownLabel::new(egui::Id::new("mermaid-copy"), SEQ_DOC)
                    .font(egui::FontId::new(
                        crate::theme::editor_font_size(panel.ctx()),
                        fonts::preview_body_family(panel.ctx()),
                    ))
                    .wrap()
                    .link_handler(&handler)
                    .show(panel);
            },
        );
        let copied = second
            .platform_output
            .commands
            .iter()
            .find_map(|cmd| match cmd {
                egui::OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            });
        assert_eq!(
            copied.as_deref(),
            Some("sequenceDiagram\nA->>B: hi"),
            "点击复制应把 mermaid 源码整块送进剪贴板"
        );
        second.drop_without_applying_deltas();
    }

    /// #51 M3 验收 5(流式):围栏未闭合 → 源码回落;补全后**当帧**切到
    /// 图形;全程 widget id 稳定(不含内容长度,§6.7)。
    #[test]
    fn streaming_frame_switches_from_fallback_to_diagram_with_stable_id() {
        if cjk_fonts_missing() {
            eprintln!("本机无 CJK 候选字体,mermaid 流式验收跳过");
            return;
        }
        let ctx = fresh_ctx();
        ThemeSettings::default().apply(&ctx, ThemeMode::Dark);
        ctx.options_mut(|o| o.tessellation_options.feathering = false);
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(700.0, 900.0));
        let partial = "```mermaid\nflowchart TD\nA[开";
        let complete = "```mermaid\nflowchart TD\nA[开始] --> B[收尾]\n```\n";
        let mut widget_ids = Vec::new();
        for (step, doc) in [("未闭合", partial), ("补全", complete)] {
            let output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..RawInput::default()
                },
                |panel| {
                    let handler = crate::ui::ai_card::RichLinkHandler::new(
                        crate::ui::rich_widgets::TaskProbeDomain::Preview,
                        false,
                        egui::Color32::WHITE,
                        Default::default(),
                    );
                    MarkdownLabel::new(egui::Id::new("mermaid-stream"), doc)
                        .font(egui::FontId::new(
                            crate::theme::editor_font_size(panel.ctx()),
                            fonts::preview_body_family(panel.ctx()),
                        ))
                        .wrap()
                        .link_handler(&handler)
                        .show(panel);
                },
            );
            output.drop_without_applying_deltas();
            let blocks = read_probe(&ctx);
            let [block] = blocks.as_slice() else {
                panic!("{step}:应有且仅有一个 mermaid 块探针");
            };
            widget_ids.push(block.widget_id);
            match step {
                "未闭合" => {
                    assert!(
                        !block.rendered,
                        "流式未闭合围栏应按普通代码块回落(探针 {:?})",
                        block.error
                    );
                }
                _ => {
                    assert!(block.rendered, "围栏合法后当帧应切到图形");
                    assert_eq!((block.nodes.len(), block.edges), (2, 1));
                }
            }
        }
        assert_eq!(
            widget_ids[0], widget_ids[1],
            "内容变化不得改变 widget id(§6.7)"
        );
    }

    /// #51 M3 否决线:非 mermaid 文档(其他语言围栏/无语言围栏/表格/
    /// 正文)在「接入 mermaid handler」与「未接入」两轮的绘制输出逐
    /// 顶点一致——mermaid 扩展对无关文档零像素扰动(vendored 代码块
    /// 路径与视口剔除结构未动)。
    #[test]
    fn non_mermaid_document_renders_identically_with_and_without_mermaid_gate() {
        if cjk_fonts_missing() {
            eprintln!("本机无 CJK 候选字体,否决线验收跳过");
            return;
        }
        let ctx = fresh_ctx();
        let mesh_digest = |primitives: &[egui::epaint::ClippedPrimitive]| {
            primitives
                .iter()
                .map(|cp| match &cp.primitive {
                    egui::epaint::Primitive::Mesh(mesh) => (
                        mesh.vertices
                            .iter()
                            .map(|v| (v.pos.x, v.pos.y, v.color))
                            .collect::<Vec<_>>(),
                        mesh.indices.clone(),
                    ),
                    egui::epaint::Primitive::Callback(_) => (Vec::new(), Vec::new()),
                })
                .collect::<Vec<_>>()
        };
        let run = |with_handler: bool| {
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(700.0, 1600.0),
                    )),
                    ..RawInput::default()
                },
                |panel| {
                    let font = egui::FontId::new(
                        crate::theme::editor_font_size(panel.ctx()),
                        fonts::preview_body_family(panel.ctx()),
                    );
                    let label = MarkdownLabel::new(egui::Id::new("mermaid-veto"), PLAIN_DOC)
                        .font(font)
                        .wrap();
                    if with_handler {
                        let handler = crate::ui::ai_card::RichLinkHandler::new(
                            crate::ui::rich_widgets::TaskProbeDomain::Preview,
                            false,
                            egui::Color32::WHITE,
                            Default::default(),
                        );
                        label.link_handler(&handler).show(panel);
                    } else {
                        label.show(panel);
                    }
                },
            );
            let primitives = ctx.tessellate(std::mem::take(&mut output.shapes), 1.0);
            output.drop_without_applying_deltas();
            mesh_digest(&primitives)
        };
        let without = run(false);
        let with_gate = run(true);
        assert_eq!(
            without, with_gate,
            "非 mermaid 文档的绘制输出应与接入 mermaid 前逐顶点一致"
        );
        assert!(
            without.iter().any(|(verts, _)| !verts.is_empty()),
            "对照轮应有绘制内容"
        );
    }

    /// #51 M3 否决线(混排文档):mermaid 块混入文档时,普通代码块的
    /// code_bg 背景矩形宽高不变(只允许位置平移)。
    #[test]
    fn plain_code_block_geometry_invariant_when_mermaid_present() {
        if cjk_fonts_missing() {
            eprintln!("本机无 CJK 候选字体,混排否决线验收跳过");
            return;
        }
        let rust_doc = "```rust\nfn a() {}\nfn b() {}\nfn c() {}\n```\n";
        let mixed_doc = concat!(
            "```mermaid\nflowchart TD\nA --> B\n```\n\n",
            "```rust\nfn a() {}\nfn b() {}\nfn c() {}\n```\n",
        );
        let ctx = fresh_ctx();
        let plain = render_frame(&ctx, false, rust_doc, "mermaid-mixed-plain", true);
        let mixed = render_frame(&ctx, false, mixed_doc, "mermaid-mixed-mixed", true);
        let (px0, py0, px1, py1) = plain.code_bg_bbox.expect("纯 rust 文档应有代码块背景");
        // mermaid 外框(2026-10-26)也涂代码底,整体 bbox 是「mermaid 框 ∪
        // rust 块」的并集 —— 按探针里 mermaid 块的最低节点 y 分隔,只取
        // 其下方的顶点还原 rust 块自身的几何。
        let mermaid_bottom = mixed
            .probe
            .iter()
            .filter(|b| b.rendered)
            .flat_map(|b| b.nodes.iter().map(|(_, r, _)| r.max.y))
            .fold(0.0f32, f32::max)
            + 20.0;
        let mut rust_vertices = mixed
            .code_bg_vertices
            .iter()
            .filter(|(_, y)| *y > mermaid_bottom);
        let (mut mx0, mut my0, mut mx1, mut my1) = rust_vertices
            .next()
            .map_or((0.0, 0.0, 0.0, 0.0), |(x, y)| (*x, *y, *x, *y));
        for (x, y) in rust_vertices {
            mx0 = mx0.min(*x);
            my0 = my0.min(*y);
            mx1 = mx1.max(*x);
            my1 = my1.max(*y);
        }
        assert!(
            ((px1 - px0) - (mx1 - mx0)).abs() < 1.0,
            "代码块背景宽应不变(纯 {px1:.0}-{px0:.0} vs 混 {mx1:.0}-{mx0:.0})"
        );
        assert!(
            ((py1 - py0) - (my1 - my0)).abs() < 1.0,
            "代码块背景高应不变(纯 {py1:.0}-{py0:.0} vs 混 {my1:.0}-{my0:.0})"
        );
        assert!(my0 > py0, "混排文档里 rust 块应被 mermaid 块下推(位置平移)");
        assert!(
            mixed.probe.iter().any(|b| b.rendered),
            "混排文档的 mermaid 块应出图"
        );
    }

    /// 查看态切换验收(2026-10-24「可切换代码/预览」):默认图表;点「源码」
    /// 钮当帧切到源码视图(嵌套高亮代码块,无解析错误);点「图表」切回,
    /// widget id 全程稳定。点击注入照 `fallback_copy_button` 的手法。
    #[test]
    fn view_toggles_between_diagram_and_source_and_back() {
        if cjk_fonts_missing() {
            eprintln!("本机无 CJK 字体,mermaid 切换验收跳过");
            return;
        }
        let ctx = fresh_ctx();
        // 帧 1:默认图表态。
        let ev = render_frame(&ctx, false, TD_DOC, "mermaid-view-toggle", true);
        let [block] = ev.probe.as_slice() else {
            panic!("应有且仅有一个 mermaid 块探针");
        };
        assert!(block.rendered, "默认出图");
        assert!(!block.source_view, "默认图表态");
        let id_first = block.widget_id;
        let source_toggle = toggle_probe(&ctx)
            .into_iter()
            .find(|(_, is_source)| *is_source)
            .map(|(rect, _)| rect)
            .expect("块头应挂出「源码」切换钮");

        // 点击帧的标志位在帧尾翻转,视图在**下一帧**物化(生产环境
        // request_repaint 自动补帧,用户无感)—— 每次点击后渲染一帧
        // 静默帧再断言。
        let click = |pos: egui::Pos2| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ]
        };
        let quiet =
            |ctx: &egui::Context| render_frame(ctx, false, TD_DOC, "mermaid-view-toggle", true);
        // 帧 2:点「源码」;帧 3(静默)物化为源码视图。
        render_frame_events(
            &ctx,
            false,
            TD_DOC,
            "mermaid-view-toggle",
            true,
            click(source_toggle.center()),
        );
        let ev = quiet(&ctx);
        let [block] = ev.probe.as_slice() else {
            panic!("切换后仍应有恰好一个探针");
        };
        assert!(!block.rendered, "源码态不出图");
        assert!(block.source_view, "探针应标记源码视图");
        assert!(
            block.error.is_none(),
            "源码视图是用户选择,不是解析失败:{:?}",
            block.error
        );
        assert_eq!(block.widget_id, id_first, "切换不改 widget id(缓存纪律)");
        // 源码视图真的画成了代码块:code_bg 进 mesh 且成块。
        assert!(ev.code_bg_bbox.is_some(), "源码视图应渲染为代码块背景");
        let diagram_toggle = toggle_probe(&ctx)
            .into_iter()
            .find(|(_, is_source)| !*is_source)
            .map(|(rect, _)| rect)
            .expect("源码态块头应挂出「图表」切换钮");

        // 帧 4:点「图表」;帧 5(静默)切回出图。
        render_frame_events(
            &ctx,
            false,
            TD_DOC,
            "mermaid-view-toggle",
            true,
            click(diagram_toggle.center()),
        );
        let ev = quiet(&ctx);
        let [block] = ev.probe.as_slice() else {
            panic!("切回后仍应有恰好一个探针");
        };
        assert!(block.rendered, "切回图表态出图");
        assert!(!block.source_view);
        assert_eq!(block.widget_id, id_first, "往返切换 id 稳定");
    }

    /// 只读范围验收(「预览只读」):图表态的只读矩形覆盖整块(含图与
    /// 头带);源码态只覆盖头带(代码体不登记,点进去编辑)。回调态的回落
    /// 与图表态同宽(整块只读)。
    #[test]
    fn read_only_rect_spans_full_block_in_diagram_view_and_header_only_in_source_view() {
        if cjk_fonts_missing() {
            eprintln!("本机无 CJK 字体,mermaid 只读范围验收跳过");
            return;
        }
        let ctx = fresh_ctx();
        // 帧外读原始探针(生产读法带帧号核对,帧外恒空)。
        let ro_height = |ctx: &egui::Context| {
            read_only_probe(ctx)
                .iter()
                .map(|rect| rect.height())
                .fold(0.0_f32, f32::max)
        };
        // 图表态:整块(头 + 图,TD 图 4 层必然显著高于头带)。
        render_frame(&ctx, false, TD_DOC, "mermaid-ro-full", true);
        let diagram_ro = ro_height(&ctx);
        assert!(
            diagram_ro > 80.0,
            "图表态只读矩形应覆盖整块(实际高 {diagram_ro:.0}px)"
        );
        // 回落态(sequenceDiagram):同为图表意图,整块只读。
        render_frame(&ctx, false, SEQ_DOC, "mermaid-ro-fallback", true);
        let fallback_ro = ro_height(&ctx);
        assert!(
            fallback_ro > 60.0,
            "回落态只读矩形应覆盖整块(实际高 {fallback_ro:.0}px)"
        );
        // 源码态:点「源码」后补一帧静默帧物化(标志位帧尾翻转)。
        let source_toggle = toggle_probe(&ctx)
            .into_iter()
            .find(|(_, is_source)| *is_source)
            .map(|(rect, _)| rect)
            .expect("前置:切换钮在场");
        render_frame_events(
            &ctx,
            false,
            SEQ_DOC,
            "mermaid-ro-fallback",
            true,
            vec![
                egui::Event::PointerMoved(source_toggle.center()),
                egui::Event::PointerButton {
                    pos: source_toggle.center(),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
                egui::Event::PointerButton {
                    pos: source_toggle.center(),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        render_frame(&ctx, false, SEQ_DOC, "mermaid-ro-fallback", true);
        assert!(
            read_probe(&ctx).iter().all(|block| block.source_view),
            "前置:已切到源码视图"
        );
        let source_ro = ro_height(&ctx);
        assert!(
            source_ro < 40.0,
            "源码态只读矩形应只剩头带(实际高 {source_ro:.0}px)"
        );
    }
}
