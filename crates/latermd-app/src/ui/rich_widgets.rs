//! 富渲染共享件(右栏预览与 Live 写作渲染两条路径都用的部分)。
//!
//! 2026-10-14 从 [`crate::ui::preview`] 搬出(live-unify-plan P0):右栏对照
//! 预览要退役,但这些件 —— 代码块复制头、任务 checkbox、`==高亮==` 排版段、
//! 相对图片 `file://` 改写、几枚主题色 —— 在 Live 写作渲染里同样要用,
//! 不能跟着面板一起删。搬出来的收益是 `live.rs` 不再靠 `crate::ui::preview`
//! 拿它们,两栏共用同一份实现而不是各画一套。
//!
//! 本模块**不依赖** `crate::ui::preview`,也不依赖 `LiveState`:依赖 egui、
//! vendored link 协议常量、`latermd_md` 与 tokens —— 依赖方向单向向下。
//!
//! 四组按主题分:①链接与荧光底色的取色;②相对图片 `file://` 改写;③任务
//! checkbox;④代码块复制头。③④都是「透明占位上的自绘 widget」,共享同一条
//! 纪律:不注册 egui widget,而是把本帧几何写进 `egui::Context` 临时槽(探针),
//! 点击在帧末读原始指针事件判定 —— 列表项内 inline widget 的 vendored
//! section/token 映射到不了 handler(#63 实测)。

use eframe::egui;
use std::borrow::Cow;
use std::ops::Range;
use std::path::Path;

/// ai:// 链接的样式色(紫罗兰,与默认超链接色区分),按明暗主题取两档。
/// `pub(crate)`:选区 AI 浮标(#61)同源取色 —— AI 专属色只有一个真源。
pub(crate) fn ai_link_color(dark_mode: bool) -> egui::Color32 {
    if dark_mode {
        egui::Color32::from_rgb(0xC9, 0x9B, 0xF5)
    } else {
        egui::Color32::from_rgb(0x8B, 0x2F, 0xC9)
    }
}

/// `[[wikilink]]` 的链接色(青绿,与 ai:// 的紫罗兰区分),按明暗主题取两档。
pub(crate) fn wiki_link_color(dark_mode: bool) -> egui::Color32 {
    if dark_mode {
        egui::Color32::from_rgb(0x6C, 0xD4, 0xC0)
    } else {
        egui::Color32::from_rgb(0x0F, 0x7A, 0x66)
    }
}

/// `==高亮==` 底色的混色锚(#65 M2):荧光笔语义,色相固定黄,不随主题;
/// 明度端取当前主题的 `extreme_bg_color`(文本背景最远端)推导 —— 一条
/// 推导式出明暗两档,不硬编码双份十六进制(#122 同族:对比度下限由
/// `highlight_bg_meets_contrast_floor` 测试钉住 ≥ 4.5:1)。从 `visuals`
/// 推导还顺带吃进皮肤/主题覆盖:用户换了 extreme_bg,高亮跟着换。
const HIGHLIGHT_TINT: egui::Color32 = egui::Color32::from_rgb(255, 230, 0);
/// 黄的混入比例:出厂明主题(白底)出 ≈#fff8b3 淡黄(经典 mark 观感),
/// 出厂暗主题(灰阶 10 底)出 ≈#544c07 暗橄榄。HTML 导出的内嵌 CSS
/// (latermd-export)写的是同一推导式的产物,两边同源。
const HIGHLIGHT_TINT_RATIO: f32 = 0.30;
/// 高亮底色四周外扩(像素),与行内代码观感一致
/// (`InlineCodeStyle::default().expand_bg`)。
pub(crate) const HIGHLIGHT_BG_EXPAND: f32 = 3.0;
/// 行高下限的绝对补偿,复制 vendored `layout.rs` 的
/// `LINE_HEIGHT_FLOOR_SLACK_PX`(私有常量,epaint 整像素行高吸附的最坏
/// 下探 + 1/32 pt 量化)。
pub(crate) const LINE_HEIGHT_FLOOR_SLACK_PX: f32 = 0.75;

/// `==高亮==` 底色:30% 荧光黄 × 70% 主题背景端,逐通道按 u8 四舍五入。
/// `pub(crate)`:Live 富渲染块与测试同源取色 —— 高亮底色只有一个真源。
pub(crate) fn highlight_bg_color(visuals: &egui::Visuals) -> egui::Color32 {
    let mix = |tint: u8, base: u8| -> u8 {
        (tint as f32 * HIGHLIGHT_TINT_RATIO + base as f32 * (1.0 - HIGHLIGHT_TINT_RATIO)).round()
            as u8
    };
    egui::Color32::from_rgb(
        mix(HIGHLIGHT_TINT.r(), visuals.extreme_bg_color.r()),
        mix(HIGHLIGHT_TINT.g(), visuals.extreme_bg_color.g()),
        mix(HIGHLIGHT_TINT.b(), visuals.extreme_bg_color.b()),
    )
}

/// `hl://` 链接的排版段(#65 M2):正文色文字 + 推导底色,行高/底对齐与
/// vendored 给普通链接段用的 `base_format` 同式。行高公式复制 vendored
/// `line_height_for`(私有,不能调),由
/// `highlight_section_matches_plain_text_metrics` 钉住:同一 Ui 里正文
/// section 的行高与本段相等,vendored 公式变化当场红。
pub(crate) fn append_highlight_section(
    ui: &egui::Ui,
    text: &str,
    job: &mut egui::text::LayoutJob,
    font: &egui::FontId,
) {
    let style = egui_markdown::global_style(ui.ctx());
    let line_height = (font.size * style.line_height_ratio)
        .max(font.size * style.min_line_height_em + LINE_HEIGHT_FLOOR_SLACK_PX);
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color: ui.visuals().text_color(),
            background: highlight_bg_color(ui.visuals()),
            expand_bg: HIGHLIGHT_BG_EXPAND,
            valign: egui::Align::BOTTOM,
            line_height: Some(line_height),
            ..egui::TextFormat::default()
        },
    );
}

/// 把渲染文本里的**相对图片地址**改写成 `file://` 绝对 URI(docs/image-plan.md
/// §4.1,B 段唯一硬骨头)。
///
/// 文档里存相对路径是对的(`./foo.assets/x.png` 随目录走,可移植),但
/// vendored 层把 url 原样喂 `egui::Image::new`,egui 没有「文档目录」概念,
/// 相对地址一律加载失败。改写发生在**喂给预览之前的字符串层**,源码与
/// `PreviewState` 一字不动;vendored 一行不动(等上游 `Token::Image` 加
/// `base_dir`,vendor/README.md 已登记待上游化)。
///
/// `base_dir` 为 `None`(文档未落盘)或空串(裸相对文件名,`Path::parent`
/// 的产物)时原样借回:没有锚点可拼,交给加载器按失败处理,与改写前一致。
pub(crate) fn resolve_relative_images<'a>(
    text: &'a str,
    base_dir: Option<&'a Path>,
) -> Cow<'a, str> {
    let rewrites = image_rewrites(text, base_dir);
    if rewrites.is_empty() {
        return Cow::Borrowed(text);
    }
    // 拼串交给 `OffsetMap::apply`:改写清单只有一份,「渲染串长什么样」与
    // 「偏移怎么换算」(map_source_offset 的第二层)不漂移。
    Cow::Owned(latermd_md::OffsetMap::from_rewrites(rewrites).apply(text))
}

/// 相对图片地址的改写清单(改写逻辑见 [`resolve_relative_images`]):每处
/// 是一个 `latermd_md::Rewrite`(与 wikilink 展开层**同一结构**),字符串
/// 改写与偏移映射(`map_source_offset`)共用同一份,两处不漂移。
pub(crate) fn image_rewrites(text: &str, base_dir: Option<&Path>) -> Vec<latermd_md::Rewrite> {
    let Some(base_dir) = base_dir.filter(|dir| !dir.as_os_str().is_empty()) else {
        return Vec::new();
    };
    inline_image_dests(text)
        .into_iter()
        .filter(|(_, dest, _)| is_relative(dest))
        .map(|(span, dest, wrapped)| {
            let mut uri = file_uri(base_dir, &dest);
            // 原 `<…>` 形式保持包裹;拼出来的 URI 含空格时裸目标语法会被
            // 截断,也要包。目标本身就在 `(` 与 `)` 之间,替换串带尖括号
            // 仍是合法 Markdown。
            if wrapped || uri.chars().any(char::is_whitespace) {
                uri = format!("<{uri}>");
            }
            latermd_md::Rewrite {
                span,
                replacement: uri,
            }
        })
        .collect()
}

/// 扫出全部**内联图片** `![alt](dest)` / `![alt](<dest> "title")` 的目标:
/// `(字节区间, 目标文本, 是否 <…> 包裹)`。区间含 `<>` 包裹(替换时原样
/// 保持包裹形式),不含两侧圆括号。
///
/// 与 `latermd_md::wikilinks` 同款扫描纪律:围栏代码块内不认(代码里的
/// `![](x)` 是字面文本);引用式图片 `![alt][ref]` 目标不在行内,不认;
/// **链接** `[t](x)` 不是图片,不认(链接点击走浏览器,拼 file:// 反而坏)。
/// alt 内嵌套 `![![a](u)](v)` 会把内层也扫出来 —— 内层本来就是独立图片,
/// 正该改写。
fn inline_image_dests(text: &str) -> Vec<(Range<usize>, String, bool)> {
    let mut dests = Vec::new();
    let mut in_code = false;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let rest = &text[index..];
        let line_start = index == 0 || bytes[index - 1] == b'\n';
        if line_start {
            let trimmed = rest.trim_start_matches([' ', '\t']);
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_code = !in_code;
            }
        }
        if !in_code && rest.starts_with("![") {
            // alt 到下一个 `]`(CommonMark 裸 alt 不含 `]`);其后紧跟 `(` 才是
            // 内联图片,否则是引用式或残缺写法,跳过这两个字符继续。
            if let Some(alt_len) = rest[2..].find(']') {
                let paren = 2 + alt_len + 1;
                if rest[paren..].starts_with('(') {
                    if let Some((span, dest, wrapped)) = dest_span(rest, paren + 1) {
                        dests.push((index + span.start..index + span.end, dest, wrapped));
                        index += span.end;
                        continue;
                    }
                }
            }
            index += 2;
            continue;
        }
        let step = rest.chars().next().map_or(1, char::len_utf8);
        index += step;
    }
    dests
}

/// `(` 之后的目标段:跳过前导空白;`<…>` 形式取尖括号内(区间**含**尖括号),
/// 裸形式到首个空白或 `)`。空目标(`![]()`)返回 `None`。
fn dest_span(text: &str, after_paren: usize) -> Option<(Range<usize>, String, bool)> {
    let bytes = text.as_bytes();
    let mut i = after_paren;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    match bytes.get(i) {
        Some(b'<') => {
            let close = i + 1 + text[i + 1..].find('>')?;
            Some((i..close + 1, text[i + 1..close].to_owned(), true))
        }
        Some(_) => {
            let stop = text[i..]
                .find(|c: char| c.is_whitespace() || c == ')')
                .map_or(text.len(), |offset| i + offset);
            (stop > i).then(|| (i..stop, text[i..stop].to_owned(), false))
        }
        None => None,
    }
}

/// 目标是否**相对**:无 scheme(含 `wiki://`、`data:`、`ai://`)、非 Windows
/// 盘符、非 `/` 绝对路径、非纯锚点。首段含 `:` 即视为有 scheme ——
/// `https:`、`wiki:`、`C:` 一网打尽。
fn is_relative(dest: &str) -> bool {
    !dest.is_empty()
        && !dest.starts_with('#')
        && !dest.starts_with('/')
        && !dest.split('/').next().unwrap_or(dest).contains(':')
}

/// 相对目标 → `file://` URI:按文档目录拼绝对路径(词法拼接,不做
/// canonicalize —— 那是磁盘 IO 且会解析符号链接,渲染定位不需要)。
/// 路径分隔符统一 `/`,Windows 盘符路径补前导 `/`(`file:///C:/…`,
/// egui FileLoader 的解析约定)。
fn file_uri(base_dir: &Path, dest: &str) -> String {
    let cleaned = dest.trim_start_matches("./");
    let path = base_dir.join(cleaned);
    let unified = path.to_string_lossy().replace('\\', "/");
    if unified.starts_with('/') {
        format!("file://{unified}")
    } else {
        format!("file:///{unified}")
    }
}

// —— 任务列表 checkbox(#63)——

/// checkbox 探针条目的坐标域(#63):载荷偏移在哪套坐标系里,消费侧据此
/// 换算回源码。两面板同帧写进同一份全局探针(几何共享),载荷坐标各说
/// 各话,必须带上出身才不会串台。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TaskProbeDomain {
    /// 右栏预览:emoji 层输出坐标,消费时逆穿 `offset_map` +
    /// `highlight_map` + `emoji_map`。
    Preview,
    /// 编辑器 Live 富渲染块:块内坐标,消费时加记录时的块首字节偏移。
    Live(usize),
}

/// 解析 `task://` 链接载荷:`Some((勾选态, emoji 层输出坐标偏移))`;
/// 形态不符(非本 scheme / 首字符非 u,c / 余非数字)返回 `None`。
/// 勾选态只在绘制时消费(画空框还是对勾);回写按偏移重新核验当前标记
/// 形态,不信任这个态(点击帧到归约帧之间文档可能已变)。
pub(crate) fn parse_task_href(href: &str) -> Option<(bool, usize)> {
    let payload = href.strip_prefix(latermd_md::TASK_SCHEME)?;
    let (checked, digits) = payload.split_at(1.min(payload.len()));
    let checked = match checked {
        "c" => true,
        "u" => false,
        _ => return None,
    };
    digits.parse::<usize>().ok().map(|offset| (checked, offset))
}

/// 在透明占位区上画 checkbox(#63):方框 + 已勾时叠 `Icon::Check` 对勾。
/// 边长取占位区高的 0.72(行高内舒适,不顶满),垂直居中、水平靠左
/// (与「行首标记」的视觉锚一致);颜色走正文色系(两主题各自成立),
/// 已勾的对勾用 tokens::OK 强调。悬停 = 指针落在占位区内:边框换 hover
/// 色 + 背景微亮(立即模式直接读指针位置,不注册 widget —— 点击命中
/// 读原始指针事件,见 [`task_click_hit`],13a 同层纪律:命中区即占位区)。
pub(crate) fn paint_task_checkbox(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    href: &str,
    domain: TaskProbeDomain,
) {
    let Some((checked, _)) = parse_task_href(href) else {
        return;
    };
    let side = rect.height() * 0.72;
    if side <= 0.0 {
        return;
    }
    record_task_checkbox(ui.ctx(), rect, href, domain);
    let box_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - side / 2.0),
        egui::vec2(side, side),
    );
    if !ui.is_rect_visible(box_rect) {
        return;
    }
    let hovered = ui
        .input(|input| input.pointer.latest_pos())
        .is_some_and(|pos| rect.contains(pos));
    let stroke = egui::Stroke::new(
        if hovered { 2.0 } else { 1.0 },
        if hovered {
            ui.visuals().widgets.hovered.fg_stroke.color
        } else {
            ui.visuals().text_color()
        },
    );
    if hovered {
        ui.painter().rect_filled(
            box_rect,
            crate::ui::tokens::RADIUS_SM,
            ui.visuals().widgets.hovered.bg_fill,
        );
    }
    ui.painter().rect_stroke(
        box_rect,
        crate::ui::tokens::RADIUS_SM,
        stroke,
        egui::StrokeKind::Inside,
    );
    if checked {
        crate::ui::icons::Icon::Check.draw(
            ui.painter(),
            box_rect.center(),
            side,
            crate::ui::tokens::OK,
        );
    }
}

/// checkbox 探针在 egui data 的键(#63,照 [`copy_button_rects_id`] 的
/// data 手法):本帧各 checkbox 的(占位区 rect, 链接载荷, 坐标域)。逐个
/// 追加、帧号核对。**点击不走 vendored 链接命中**——列表项的布局里
/// section_to_token 与 egui 0.36 galley 的空文本 section 会错位,vendored
/// `handle_hover`/`handle_click` 对列表项内的 inline widget 到不了
/// handler(实测两处:预览与 Live 富渲染块同样不可达,#63 live 测试
/// 断零消息证实);照 #38 复制按钮与 #61 浮标的同款纪律读原始指针:
/// [`task_click_hit`] 在渲染侧帧末判定,Live 列「点击进编辑」在
/// [`task_checkbox_rects`] 里排除 checkbox 命中。
fn task_checkbox_probe_id() -> egui::Id {
    egui::Id::new("latermd-task-checkbox-probe")
}

/// 追加一枚本帧 checkbox(帧号变了就重开清单,一帧一份)。预览与 Live
/// 两面板同帧都写进同一份全局清单(与 copy_button_rects 同语义);载荷
/// 坐标各说各话,出身记进坐标域,消费侧按域过滤(几何判定只看屏幕位置)。
fn record_task_checkbox(
    ctx: &egui::Context,
    rect: egui::Rect,
    href: &str,
    domain: TaskProbeDomain,
) {
    let probe_id = task_checkbox_probe_id();
    let frame = ctx.cumulative_pass_nr();
    let mut entries = ctx
        .data(|d| d.get_temp::<(u64, Vec<(egui::Rect, String, TaskProbeDomain)>)>(probe_id))
        .filter(|(seen, _)| *seen == frame)
        .map(|(_, entries)| entries)
        .unwrap_or_default();
    entries.push((rect, href.to_owned(), domain));
    ctx.data_mut(|d| d.insert_temp(probe_id, (frame, entries)));
}

/// 当前帧的 checkbox (rect, 载荷, 坐标域) 清单(帧号核对通过的才返回)。
/// 零 checkbox 帧返回空 —— 上一帧的旧 rect 不得再拦 Live 的「点击进编辑」。
pub(crate) fn task_checkbox_entries(
    ctx: &egui::Context,
) -> Vec<(egui::Rect, String, TaskProbeDomain)> {
    ctx.data(|d| {
        d.get_temp::<(u64, Vec<(egui::Rect, String, TaskProbeDomain)>)>(task_checkbox_probe_id())
            .filter(|(seen, _)| *seen == ctx.cumulative_pass_nr())
            .map(|(_, entries)| entries)
            .unwrap_or_default()
    })
}

/// 当前帧的 checkbox rect 清单(只留几何):Live 列「点击进编辑」的
/// 排除判定用(点击归 checkbox 切换,不连带进编辑)。几何排除不分面板。
pub(crate) fn task_checkbox_rects(ctx: &egui::Context) -> Vec<egui::Rect> {
    task_checkbox_entries(ctx)
        .into_iter()
        .map(|(rect, _, _)| rect)
        .collect()
}

/// 帧后读法(帧号已前进,核对必失配):测试按「最后写入者即本帧」拿
/// 最后写入一帧的 rect 清单。
#[cfg(test)]
pub(crate) fn task_checkbox_probe(ctx: &egui::Context) -> Vec<egui::Rect> {
    ctx.data(|d| {
        d.get_temp::<(u64, Vec<(egui::Rect, String, TaskProbeDomain)>)>(task_checkbox_probe_id())
            .map(|(_, entries)| entries.into_iter().map(|(rect, _, _)| rect).collect())
            .unwrap_or_default()
    })
}

/// checkbox 的原始指针点击命中(#63):本帧主键 click(egui 已按拖动阈值
/// 判定,块内拖选不算 click)且抬起点落在本帧某枚探针 rect 内 → 返回
/// 该枚的(链接载荷, 坐标域)。与 Live 列 `clicked_for_edit` 同款读法
/// (primary_clicked + interact_pos),不挂 egui widget、不依赖 vendored 的
/// section/token 映射。坐标域随条目带回:预览只认 [`TaskProbeDomain::
/// Preview`]、Live 只认 `Live(_)` 各自消费,同帧互不串台。
pub(crate) fn task_click_hit(ui: &egui::Ui) -> Option<(String, TaskProbeDomain)> {
    if !ui.input(|input| input.pointer.primary_clicked()) {
        return None;
    }
    let pos = ui.input(|input| input.pointer.interact_pos())?;
    task_checkbox_entries(ui.ctx())
        .into_iter()
        .find(|(rect, _, _)| rect.contains(pos))
        .map(|(_, href, domain)| (href, domain))
}

/// checkbox 点击的焦点暂存键(#63):egui 0.36 的让焦
/// (`SurrenderFocusOn::Presses`)发生在**按下帧**的 widget 绘制段,
/// 释放帧帧首已查无可焦者 —— 按下帧帧首的持焦者快照存这里,释放帧
/// 点击命中时归还(layout.rs `keep_find_focus` 同款:绘制段之后的
/// `request_focus` 不再被同一帧的让焦吃掉)。
fn task_focus_stash_id() -> egui::Id {
    egui::Id::new("latermd-task-focus-stash")
}

/// 按下帧调用(#63):帧首持焦者进暂存,供同次点击的释放帧归还。一帧
/// 只收首份快照(预览面板先画,持焦者尚未被让;Live 列后画时焦点可能
/// 已被按下帧的让焦清掉,不得覆盖),释放帧消费,拖选不放则自然过期。
pub(crate) fn stash_focus_on_press(ctx: &egui::Context) {
    if !ctx.input(|input| input.pointer.primary_pressed()) {
        return;
    }
    let frame = ctx.cumulative_pass_nr();
    let seen: Option<(u64, Option<egui::Id>)> = ctx.data(|d| d.get_temp(task_focus_stash_id()));
    if seen.is_some_and(|(seen_frame, _)| seen_frame == frame) {
        return;
    }
    let focused = ctx.memory(|mem| mem.focused());
    ctx.data_mut(|d| d.insert_temp(task_focus_stash_id(), (frame, focused)));
}

/// 点击命中帧调用(#63):把按下帧持焦者还回去(`request_focus` 的落地
/// 在下一帧;暂存为空或持焦者已消亡则不动 —— egui 对消亡 id 的
/// request_focus 是无害 no-op)。
pub(crate) fn restore_stashed_focus(ctx: &egui::Context) {
    let focused: Option<(u64, Option<egui::Id>)> = ctx.data(|d| d.get_temp(task_focus_stash_id()));
    if let Some((_, Some(id))) = focused {
        ctx.memory_mut(|mem| mem.request_focus(id));
    }
}

// —— 代码块复制头(docs/auto-plan.md #38)——

/// 「已复制」✓ 反馈的存活窗口:点击后停留 1.5s 自行消退,不需要用户再
/// 交互确认。
const COPIED_FEEDBACK: std::time::Duration = std::time::Duration::from_millis(1500);
/// 点击帧后再排一短帧:点击帧里按钮已按普通态画完,✓ 要等下一帧才上屏,
/// 不主动排程的话它得等到下一次用户输入。
const COPIED_FEEDBACK_DELAY: std::time::Duration = std::time::Duration::from_millis(16);

/// 反馈态在 egui data 的键:全局单份「最近复制块的指纹 + 到期时刻」。
/// 按块内容而非 widget id 认领 —— vendored 挂载点的子 Ui id 按帧内序号
/// 自动分配,文档编辑后块序平移会让 id 键控的反馈错位到别的块;内容指纹
/// 最多让同文本多块同显 ✓(与 ```ai 卡片按指令文本认领状态同款简化)。
fn copied_feedback_id() -> egui::Id {
    egui::Id::new("latermd-code-copy-feedback")
}

/// 探针在 egui data 的键:本帧各复制按钮的 rect(照 vendored
/// `section_anchors` 的 data 手法 —— 生产侧写入开销一次 Vec)。读法见
/// [`copy_button_probe`]/[`copy_button_rects`]。每帧首写按帧号重置。
fn copy_button_rects_id() -> egui::Id {
    egui::Id::new("latermd-code-copy-button-rects")
}

/// 原始探针:(最后写入帧号, 该帧按钮 rect 清单)。写入发生在渲染帧内,
/// 而测试在 `run_ui` 返回之后才读 —— 那时帧号已前进(`end_pass` 处 +1),
/// 帧号核对必然失配,故测试按「最后写入者即本帧」直接取清单。
pub(crate) fn copy_button_probe(ctx: &egui::Context) -> (u64, Vec<egui::Rect>) {
    ctx.data(|d| {
        d.get_temp::<(u64, Vec<egui::Rect>)>(copy_button_rects_id())
            .unwrap_or_default()
    })
}

/// 当前帧的按钮 rect 清单(帧号核对通过的才返回):供 Live 列「点击进
/// 编辑」在帧内排除按钮命中(live.rs `clicked_for_edit`)。零按钮帧返回
/// 空 —— 比如全部代码块都转入编辑态时,上一帧的旧 rect 不得再拦点击。
pub(crate) fn copy_button_rects(ctx: &egui::Context) -> Vec<egui::Rect> {
    let (frame, rects) = copy_button_probe(ctx);
    if frame == ctx.cumulative_pass_nr() {
        rects
    } else {
        Vec::new()
    }
}

fn code_fingerprint(code: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    code.hash(&mut hasher);
    hasher.finish()
}

/// 代码块头部一行(常驻):复制按钮 + 语言标签,挂在 vendored
/// `MarkdownLabel::code_block_buttons` 挂载点 —— 该挂载点上游自带(subtree
/// 引入即有),普通代码块右上角悬浮调用,回调直接收到 `(块源文本, 语言)`,
/// 无需从 `code_block_spans` 切偏移;```ai 指令卡走 `block_code_widget` 另
/// 一条路,不经过这里。
///
/// 点击 → `Context::copy_text` 整块源文本(vendored parser 已去块尾换行)
/// → 按钮进入 ✓ 反馈态。无语言/空块同样给按钮:复制空串是合法操作,
/// 交互一致比按内容藏按钮好猜。
pub(crate) fn code_copy_buttons(ui: &mut egui::Ui, code: &str, lang: &str) {
    let now = std::time::Instant::now();
    let until = ui
        .ctx()
        .data(|d| d.get_temp::<(u64, std::time::Instant)>(copied_feedback_id()))
        .filter(|(hash, until)| *hash == code_fingerprint(code) && *until > now)
        .map(|(_, until)| until);
    let copied = until.is_some();

    // 子 Ui 是 right_to_left(Center):先分配的贴右缘,按钮在语言标签右侧。
    let size = egui::vec2(
        crate::ui::tokens::ICON + 2.0 * crate::ui::tokens::SPACE_XS,
        20.0,
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(
                rect,
                crate::ui::tokens::RADIUS_SM,
                ui.visuals().widgets.hovered.bg_fill,
            );
        }
        let (icon, color) = if copied {
            (crate::ui::icons::Icon::Check, crate::ui::tokens::OK)
        } else {
            (crate::ui::icons::Icon::Copy, ui.visuals().weak_text_color())
        };
        icon.draw(painter, rect.center(), crate::ui::tokens::ICON, color);
    }

    // 探针:记录本帧按钮几何,帧号变了就重开清单(一帧一份)。
    let probe_id = copy_button_rects_id();
    let frame = ui.ctx().cumulative_pass_nr();
    let mut rects = ui
        .ctx()
        .data(|d| d.get_temp::<(u64, Vec<egui::Rect>)>(probe_id))
        .filter(|(seen, _)| *seen == frame)
        .map(|(_, rects)| rects)
        .unwrap_or_default();
    rects.push(rect);
    ui.ctx()
        .data_mut(|d| d.insert_temp(probe_id, (frame, rects)));

    // 语言标签:info string 首词(完整 info 可带 `title=` 等元数据,首词才是
    // 语言);无语言(裸围栏/缩进块,vendored 侧落到空串)不画标签。
    if let Some(word) = lang.split_whitespace().next() {
        ui.label(egui::RichText::new(word).small().weak())
            .on_hover_text("代码块语言");
    }

    if response.clicked() {
        ui.ctx().copy_text(code.to_owned());
        let deadline = now + COPIED_FEEDBACK;
        ui.ctx()
            .data_mut(|d| d.insert_temp(copied_feedback_id(), (code_fingerprint(code), deadline)));
        ui.ctx().request_repaint_after(COPIED_FEEDBACK_DELAY);
    }

    // hover 文案放最后(on_hover_text 消费 response)。
    if let Some(until) = until {
        // 到点排程一帧,让 ✓ 自行消退;静止窗口里 egui 不会再醒。
        ui.ctx()
            .request_repaint_after(until.saturating_duration_since(now));
        response.on_hover_text("已复制");
    } else {
        response.on_hover_text("复制代码");
    }
}
