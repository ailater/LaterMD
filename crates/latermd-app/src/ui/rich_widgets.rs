//! 富渲染共享件(Live 写作渲染用的部分,右栏同路径复用)。
//!
//! 2026-10-14 从 `ui::preview`（已于 P5 删除） 搬出(live-unify-plan P0):右栏对照
//! 预览要退役,但这些件 —— 代码块复制头、任务 checkbox、`==高亮==` 排版段、
//! 相对图片 `file://` 改写、几枚主题色 —— 在 Live 写作渲染里同样要用,
//! 不能跟着面板一起删。搬出来的收益是 `live.rs` 不再靠 `crate::ui::preview`
//! 拿它们,共用同一份实现而不是各画一套。
//!
//! > **「两条路径」的说法已作废(2026-10-28 订正)**:本句定稿于三态视图
//! > (#250/#252)之前,那时右栏是「整篇单标签」的另一条渲染链路。三态
//! > 之后右栏渲染的**就是 `live::ui` 本身**(只读分栏档),全项目富渲染只有
//! > `live::ui` 一条(AGENTS.md 铁律四)—— 本模块的件不再被两条路径共用,
//! > 而是被同一条路径在左栏可写/右栏只读两种形态下共用。
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

/// 相对图片地址的改写清单:每处
/// 是一个 `latermd_md::Rewrite`(与 wikilink 展开层**同一结构**),字符串
/// 改写与偏移映射(`map_source_offset`)共用同一份,两处不漂移。
pub(crate) fn image_rewrites(text: &str, base_dir: Option<&Path>) -> Vec<latermd_md::Rewrite> {
    // div-center 块优先:整块改写,块内其它两路扫描的命中全部让位 ——
    // 清单须升序且互不重叠(`OffsetMap::from_rewrites` 有 debug 断言),
    // 整块 span 已覆盖内部的图片行/`<img>` 标签,后者必须滤掉。
    let center = div_center_rewrites(text, base_dir);
    let covered = |start: usize| center.iter().any(|rewrite| rewrite.span.contains(&start));
    let mut rewrites = Vec::new();
    if let Some(base_dir) = base_dir.filter(|dir| !dir.as_os_str().is_empty()) {
        rewrites = inline_image_dests(text)
            .into_iter()
            .filter(|(span, _, _)| !covered(span.start))
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
            .collect();
    }
    // 块级 HTML `<img>` 的形态改写与相对地址无关(未落盘也改形态),
    // 不受 base_dir 早退约束;清单须按 span 升序,三路扫描合并后统一排。
    rewrites.extend(
        html_img_rewrites(text, base_dir)
            .into_iter()
            .filter(|rewrite| !covered(rewrite.span.start)),
    );
    rewrites.extend(center);
    rewrites.sort_unstable_by_key(|rewrite| rewrite.span.start);
    rewrites
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

/// 块级 HTML 内联图片的改写清单:`<img src="…">` / `<img src='…'>` 整个
/// 标签换成等价 Markdown 图片 `![alt](dest)` —— vendored 渲染层把块级
/// HTML 当字面文本画(`Event::Html` → `Token::Text`),`<img>` 因此从不出
/// 图;换成 Markdown 图片后走既有 `Token::Image` 通道,与 `![](x)` 同源。
/// src 相对时按文档目录拼 `file://` 绝对 URI(与内联图片改写同一口径)。
///
/// 只认小写 `img`(HTML 不区分大小写,但生产端(`srcset`、README、HTML
/// 输出)与主流书写习惯是小写;扩大字母集会拉宽无谓的误配面);围栏代码
/// 块内的 `<img>` 是字面文本,不认;`src` 缺失或值为空不认。
pub(crate) fn html_img_rewrites(text: &str, base_dir: Option<&Path>) -> Vec<latermd_md::Rewrite> {
    html_img_tags(text)
        .into_iter()
        .filter_map(|(span, src, alt)| {
            if src.is_empty() {
                return None;
            }
            let dest = resolve_image_dest(&src, base_dir);
            let replacement = if dest.chars().any(char::is_whitespace) {
                format!("![{alt}](<{dest}>)")
            } else {
                format!("![{alt}]({dest})")
            };
            Some(latermd_md::Rewrite { span, replacement })
        })
        .collect()
}

/// 相对图片目标按文档目录拼 `file://` 绝对 URI;文档未落盘(None)保持
/// 相对 —— 不出图但形态仍转。html_img_rewrites 与 div_center_rewrites
/// 同一份口径。
fn resolve_image_dest(dest: &str, base_dir: Option<&Path>) -> String {
    if is_relative(dest) {
        match base_dir.filter(|dir| !dir.as_os_str().is_empty()) {
            Some(base) => file_uri(base, dest),
            None => dest.to_owned(),
        }
    } else {
        dest.to_owned()
    }
}

/// 「div/p align="center" 包裹的纯图片块」的改写清单(GitHub README 惯例):
///
/// ```text
/// <div align="center">
///   ![LaterMD](assets/logo.png)
/// </div>
/// ```
///
/// CommonMark 把 `<div>` 开启的 HTML 块当字面文本,块内的 Markdown 图片
/// 不解析;vendored 渲染层因此把整块画成三行文字。本扫描把**整块**换成
/// 带居中 title 的 Markdown 图片行(`![alt](dest "latermd:center")`,
/// 约定见 vendored `types::CENTERED_IMAGE_TITLE`)—— 图片照常出,且按
/// title 在渲染列内水平居中,`align="center"` 的表达意图得以落地。
///
/// 识别纪律:开标签 `<div…>` / `<p…>`,align 只认引号形式且值须为
/// center;中间每一行(去首尾空白)必须整行是一张图片 —— Markdown 图片
/// 或 `<img>` 标签 —— 至少一行;闭标签 `</div>` / `</p>` 与开标签同 tag;
/// **空行即不认**(空行已把 HTML 块截断,语义不再是一个 div);围栏代码
/// 块内不认(与 [`inline_image_dests`] 同款)。内容里的相对目标与
/// [`html_img_rewrites`] 同口径按文档目录拼 `file://`。
fn div_center_rewrites(text: &str, base_dir: Option<&Path>) -> Vec<latermd_md::Rewrite> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut offsets = Vec::with_capacity(lines.len());
    let mut acc = 0;
    for line in &lines {
        offsets.push(acc);
        acc += line.len();
    }
    let mut rewrites = Vec::new();
    let mut in_code = false;
    let mut index = 0;
    while index < lines.len() {
        let trimmed = lines[index].trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code = !in_code;
        }
        if !in_code {
            if let Some((consumed, images)) = match_div_center_block(&lines[index..]) {
                let replacement = images
                    .into_iter()
                    .map(|(alt, dest)| {
                        let dest = resolve_image_dest(&dest, base_dir);
                        if dest.chars().any(char::is_whitespace) {
                            format!(
                                "![{alt}](<{dest}> \"{}\")",
                                egui_markdown::types::CENTERED_IMAGE_TITLE
                            )
                        } else {
                            format!(
                                "![{alt}]({dest} \"{}\")",
                                egui_markdown::types::CENTERED_IMAGE_TITLE
                            )
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                rewrites.push(latermd_md::Rewrite {
                    span: offsets[index]..offsets[index] + consumed,
                    replacement,
                });
                // 消费掉的块内没有围栏(开/闭标签与图片行都不是 ```),状态不变。
                index += consumed;
                continue;
            }
        }
        index += 1;
    }
    rewrites
}

/// 从 `lines` 首行起配一个 div-center 块。命中返回**(消耗字节数, 图片
/// (alt, dest) 清单)**;不命中返回 None —— 调用方按普通行继续走。
fn match_div_center_block(lines: &[&str]) -> Option<(usize, Vec<(String, String)>)> {
    let opener = lines.first()?.trim();
    let rest_div = opener.strip_prefix("<div");
    let rest_p = opener.strip_prefix("<p");
    let (tag, body) = match (rest_div, rest_p) {
        (Some(rest), _) => ("div", rest),
        (_, Some(rest)) => ("p", rest),
        (None, None) => return None,
    };
    // `<p` 后必须紧跟空白或 `>`(`<picture>`、`<param>` 不误配)。
    if !body.starts_with([' ', '\t']) && !body.starts_with('>') {
        return None;
    }
    let close = opener.rfind('>')?;
    if close != opener.len() - 1 {
        return None;
    }
    // body 已含收尾 `>`(strip 前缀后的余量),属性体去掉它再解析。
    let attrs = &body[..body.len() - 1];
    if quoted_attr(attrs, "align").as_deref() != Some("center") {
        return None;
    }
    let mut images = Vec::new();
    let mut consumed = lines[0].len();
    for line in &lines[1..] {
        let trimmed = line.trim();
        if trimmed == format!("</{tag}>") {
            if images.is_empty() {
                return None;
            }
            // 闭标签只消费到行尾、不带换行 —— 换行留给后续内容,
            // 替换串不与之粘连。
            return Some((consumed + trimmed.len(), images));
        }
        if trimmed.is_empty() {
            return None;
        }
        let image = markdown_image_line(trimmed).or_else(|| html_img_line(trimmed))?;
        images.push(image);
        consumed += line.len();
    }
    None
}

/// 行级「纯 Markdown 图片」判定:整行(已去首尾空白)是 `![alt](dest)`,
/// 目标可选 `<…>` 包裹与 `"title"` 后缀。返回 (alt, dest)。
fn markdown_image_line(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("![")?;
    let close_alt = rest.find(']')?;
    let alt = &rest[..close_alt];
    let after = rest[close_alt + 1..].strip_prefix('(')?;
    let (dest_range, dest, _wrapped) = dest_span(after, 0)?;
    let mut tail = after[dest_range.end..].trim_start();
    if let Some(quoted) = tail.strip_prefix('"') {
        let end = quoted.find('"')?;
        tail = quoted[end + 1..].trim_start();
    }
    (tail == ")").then(|| (alt.to_owned(), dest))
}

/// 行级「纯 `<img>` 标签」判定:整行(已去首尾空白)是一个 `<img …>` /
/// `<img … />` 标签,`>` 收尾且其后无内容。返回 (alt, dest);src 缺失或
/// 为空不认(与 [`html_img_tags`] 同口径)。
fn html_img_line(line: &str) -> Option<(String, String)> {
    let body = line.strip_prefix("<img")?;
    let close = body.rfind('>')?;
    if close != body.len() - 1 {
        return None;
    }
    // `<img` 后必须紧跟空白或 `>`(`<imgx>` 不误配)。
    let attrs = &body[..close];
    if !attrs.starts_with([' ', '\t']) {
        return None;
    }
    let src = quoted_attr(attrs, "src")?;
    if src.is_empty() {
        return None;
    }
    let alt = quoted_attr(attrs, "alt").unwrap_or_default();
    Some((alt, src))
}

/// 扫出块级 HTML 里的全部 `<img …>` 标签:`(整个标签的字节区间, src 值,
/// alt 值)`。与 [`inline_image_dests`] 同款扫描纪律:围栏代码块内不认。
/// 区间覆盖 `<` 到 `>`(含两端),替换时整标签顶掉。自闭合(`<img … />`)
/// 与 HTML 式非闭合(`<img …>`)都成立 —— `<` 起点到首个 `>` 即标签体。
/// 属性值只认引号形式(`"…"` / `'…'`),alt 缺省为空。
fn html_img_tags(text: &str) -> Vec<(Range<usize>, String, String)> {
    let mut tags = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut in_code = false;
    while index < bytes.len() {
        let rest = &text[index..];
        let line_start = index == 0 || bytes[index - 1] == b'\n';
        if line_start {
            let trimmed = rest.trim_start_matches([' ', '\t']);
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_code = !in_code;
            }
        }
        if !in_code && rest.starts_with("<img") {
            let is_tag =
                matches!(rest.as_bytes().get(4), Some(b) if b.is_ascii_whitespace() || *b == b'>');
            if is_tag {
                if let Some(close) = rest.find('>') {
                    // `close` 是 `>` 相对 `rest` 的偏移;`<img` 占 4 字节且
                    // 不含 `>`,故 close ≥ 4,标签体与全长由此得。
                    let body = &rest[4..close];
                    let src = quoted_attr(body, "src");
                    let alt = quoted_attr(body, "alt").unwrap_or_default();
                    let full = close + 1;
                    tags.push((index..index + full, src.unwrap_or_default(), alt));
                    index += full;
                    continue;
                }
            }
            index += 4;
            continue;
        }
        let step = rest.chars().next().map_or(1, char::len_utf8);
        index += step;
    }
    tags
}

/// 标签体内取引号属性值:`src="…"` / `src='…'`(键后允许任意空白)。
/// 未写或裸值(`src=x.png`,非法 HTML)返回 `None`。
fn quoted_attr(body: &str, key: &str) -> Option<String> {
    let bytes = body.as_bytes();
    let mut i = 0;
    while let Some(pos) = body[i..].find(key) {
        let at = i + pos;
        // 键必须整体:前一字节不能是属性名成分(`data-src` 里的 `src` 不认)
        let prev_ok = at == 0 || !bytes[at - 1].is_ascii_alphanumeric() && bytes[at - 1] != b'-';
        let after = at + key.len();
        let next_ok = bytes
            .get(after)
            .is_some_and(|b| b.is_ascii_whitespace() || *b == b'=');
        if prev_ok && next_ok {
            let mut j = after;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if bytes.get(j) == Some(&b'=') {
                let mut k = j + 1;
                while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                if let Some(&quote) = bytes.get(k) {
                    if quote == b'"' || quote == b'\'' {
                        if let Some(end) = body[k + 1..].find(quote as char) {
                            return Some(body[k + 1..k + 1 + end].to_owned());
                        }
                        // 引号未闭合:属性吃到标签尾(`rest.find('>')` 已保证
                        // `>` 在引号外,这里只会发生在残缺写法上)
                        return None;
                    }
                }
                return None;
            }
        }
        i = at + key.len();
    }
    None
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
    /// 整篇单标签形态下的全局坐标(载荷偏移相对整篇渲染文本)。
    ///
    /// 2026-10-28 订正:旧注释写「右栏预览(P4 已退役,仅测试路径保留)」,
    /// 前半句已不成立 —— 三态视图(#250/#252)让右栏以「分栏档」回归,渲染
    /// 的是 `live::ui` 的块级标签(坐标域同样是 [`Self::Live`]),按 P6 定稿
    /// 口径一律不写「已退役」。**后半句仍成立**:本变体现在只被
    /// `ui/mermaid.rs` 的测试用来构造「整篇渲染」的用例,生产侧零使用。
    #[cfg(test)]
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
/// section/token 映射。坐标域随条目带回:整篇基准只认 [`TaskProbeDomain::
/// Preview`]、Live 只认 [`TaskProbeDomain::Live`] 各自消费,同帧互不串台。
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

// —— Live 表格工具条探针(2026-10-23)——
// 与复制头探针同手法:绘制帧写入本帧 rect 清单,`clicked_for_edit` 按它
// 排除「点击进编辑」。工具条悬浮在表格块上缘之外,点击通常落不进块矩形;
// 首块贴视口顶时工具条被钳进块内,这份清单兜住那条边。

fn table_toolbar_rects_id() -> egui::Id {
    egui::Id::new("latermd-table-toolbar-rects")
}

/// 记录本帧表格工具条(含各图标按钮)的 rect;帧号变了重开清单。
pub(crate) fn push_table_toolbar_rects(ctx: &egui::Context, rects: Vec<egui::Rect>) {
    let id = table_toolbar_rects_id();
    let frame = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(id, (frame, rects)));
}

/// 原始探针(最后写入帧的清单,不做帧号核对):测试在 run_ui 之后读取,
/// 那时帧号已前进,按「最后写入者即本帧」取。仅测试消费(`table_toolbar_rects`
/// 才是生产读法,带帧号核对),cfg(test) 挡 dead-code。
#[cfg(test)]
pub(crate) fn table_toolbar_probe(ctx: &egui::Context) -> Vec<egui::Rect> {
    ctx.data(|d| d.get_temp::<(u64, Vec<egui::Rect>)>(table_toolbar_rects_id()))
        .map(|(_, rects)| rects)
        .unwrap_or_default()
}

/// 本帧表格工具条 rect 清单(帧号核对通过才返回)。
pub(crate) fn table_toolbar_rects(ctx: &egui::Context) -> Vec<egui::Rect> {
    let id = table_toolbar_rects_id();
    let frame = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<(u64, Vec<egui::Rect>)>(id))
        .filter(|(seen, _)| *seen == frame)
        .map(|(_, rects)| rects)
        .unwrap_or_default()
}

fn code_fingerprint(code: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    code.hash(&mut hasher);
    hasher.finish()
}

/// 代码块头部一行(常驻):复制按钮 + 关闭按钮(可选)+ 语言标签,挂在
/// vendored `MarkdownLabel::code_block_buttons` 挂载点 —— 该挂载点上游
/// 自带(subtree 引入即有),普通代码块右上角悬浮调用,回调直接收到
/// `(块源文本, 语言)`,无需从 `code_block_spans` 切偏移;```ai 指令卡走
/// `block_code_widget` 另一条路,不经过这里。
///
/// 点击复制 → `Context::copy_text` 整块源文本(vendored parser 已去块尾
/// 换行)→ 按钮进入 ✓ 反馈态。无语言/空块同样给按钮:复制空串是合法
/// 操作,交互一致比按内容藏按钮好猜。
///
/// **关闭按钮**(2026-10-23,用户需求):复制左侧的 ×,点击把块序号推进
/// `sink`,由 Live 层(live.rs)弹确认后把围栏改写成普通文本 —— 头部只
/// 管「在哪画、点没点」,改写落点(块区间)只有 Live 层知道。
pub(crate) struct CodeClose<'a> {
    pub index: usize,
    pub sink: &'a std::rc::Rc<std::cell::RefCell<Vec<usize>>>,
}

pub(crate) fn code_header_buttons(
    ui: &mut egui::Ui,
    code: &str,
    lang: &str,
    close: Option<CodeClose<'_>>,
) {
    let now = std::time::Instant::now();
    let until = ui
        .ctx()
        .data(|d| d.get_temp::<(u64, std::time::Instant)>(copied_feedback_id()))
        .filter(|(hash, until)| *hash == code_fingerprint(code) && *until > now)
        .map(|(_, until)| until);
    let copied = until.is_some();

    // 子 Ui 是 right_to_left(Center):先分配的贴右缘 —— 复制最右,关闭
    // 在复制左边,语言标签再往左(需求口径「放到复制左边」)。
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
    if response.clicked() {
        ui.ctx().copy_text(code.to_owned());
        let deadline = now + COPIED_FEEDBACK;
        ui.ctx()
            .data_mut(|d| d.insert_temp(copied_feedback_id(), (code_fingerprint(code), deadline)));
        ui.ctx().request_repaint_after(COPIED_FEEDBACK_DELAY);
    }

    // 关闭按钮:复制左侧的 ×。点击不就地改写(本函数拿不到块区间),
    // 只把块序号推给 Live 层。
    let mut close_rect = None;
    if let Some(close) = close {
        let (crect, close_response) = ui.allocate_exact_size(size, egui::Sense::click());
        if ui.is_rect_visible(crect) {
            let painter = ui.painter();
            if close_response.hovered() {
                painter.rect_filled(
                    crect,
                    crate::ui::tokens::RADIUS_SM,
                    ui.visuals().widgets.hovered.bg_fill,
                );
            }
            // 删除字形与表格工具条同款(2026-10-26 坤哥「弄个一样的」):
            // 两处破坏性入口统一垃圾桶,悬停/底色行为本就同参。
            crate::ui::icons::Icon::Trash.draw(
                painter,
                crect.center(),
                crate::ui::tokens::ICON,
                ui.visuals().weak_text_color(),
            );
        }
        if close_response.clicked() {
            close.sink.borrow_mut().push(close.index);
            ui.ctx().request_repaint();
        }
        close_response.on_hover_text("删除代码块围栏,转为普通文本");
        close_rect = Some(crect);
    }

    // 语言标签:info string 首词(完整 info 可带 `title=` 等元数据,首词才是
    // 语言);无语言(裸围栏/缩进块,vendored 侧落到空串)不画标签。
    if let Some(word) = lang.split_whitespace().next() {
        ui.label(egui::RichText::new(word).small().weak())
            .on_hover_text("代码块语言");
    }

    // 探针:记录本帧头部按钮几何(复制 + 关闭),帧号变了就重开清单
    // (一帧一份)。「点击进编辑」按这份清单排除 —— 点头部按钮是按钮
    // 语义,不连带切入编辑。
    let probe_id = copy_button_rects_id();
    let frame = ui.ctx().cumulative_pass_nr();
    let mut rects = ui
        .ctx()
        .data(|d| d.get_temp::<(u64, Vec<egui::Rect>)>(probe_id))
        .filter(|(seen, _)| *seen == frame)
        .map(|(_, rects)| rects)
        .unwrap_or_default();
    rects.push(rect);
    if let Some(crect) = close_rect {
        rects.push(crect);
    }
    ui.ctx()
        .data_mut(|d| d.insert_temp(probe_id, (frame, rects)));

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

/// 只读列/估高臂用的头部:只有复制 + 语言,无关闭按钮。静态引用形态,
/// 供 `MarkdownLabel::code_block_buttons(&fn)` 直接挂载。
pub(crate) fn code_copy_buttons(ui: &mut egui::Ui, code: &str, lang: &str) {
    code_header_buttons(ui, code, lang, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live 第 5 层([`crate::live::LiveState::rich_block_with_base`])的实
    /// 际写法:先取改写清单,再交给 `OffsetMap::apply` 拼串。下面的
    /// `rewrite` 复刻这两步,测试由此直接钉**生产路径的两件 religion**,
    /// 而不是某个包装函数。
    fn rewrite(text: &str, base_dir: Option<&Path>) -> String {
        let rewrites = image_rewrites(text, base_dir);
        if rewrites.is_empty() {
            return text.to_owned();
        }
        latermd_md::OffsetMap::from_rewrites(rewrites).apply(text)
    }

    /// 块级 HTML `<img>` 改写:整个标签换成等价 Markdown 图片,相对 src 按
    /// 文档目录拼 `file://`。README 头部 `<img src="…" width="128">` 即此
    /// 形态 —— 换掉 `width` 这类 Markdown 图片表达不了的属性(点击渲染层
    /// 用 `max_width` 收列,不受影响)。
    #[test]
    fn html_img_tags_become_markdown_images() {
        let base = Path::new("/home/u/LaterMD");
        let out = rewrite(
            "<img src=\"assets/logo/deliverables/png/icon-256.png\" alt=\"LaterMD\" width=\"128\">",
            Some(base),
        );
        assert_eq!(
            out,
            "![LaterMD](file:///home/u/LaterMD/assets/logo/deliverables/png/icon-256.png)"
        );
        // 单引号 / 自闭合 / alt 缺省 / 含空格路径
        let out = rewrite("<img src='./d/屏幕 截图.png' />", Some(base));
        assert_eq!(out, "![](<file:///home/u/LaterMD/d/屏幕 截图.png>)");
        // 绝对 URL 不拼 base;未落盘(None)仍改形态、相对地址原样保留
        let out = rewrite("<img src=\"https://e.com/a.png\">", None);
        assert_eq!(out, "![](https://e.com/a.png)");
        assert_eq!(
            rewrite("<img src=\"rel/a.png\" alt=\"x\">", None),
            "![x](rel/a.png)"
        );
        // 周边文本逐字节保留
        let doc = "前文\n\n<img src=\"a.png\">\n\n后文";
        assert_eq!(
            rewrite(doc, Some(base)),
            "前文\n\n![](file:///home/u/LaterMD/a.png)\n\n后文"
        );
    }

    /// div/p align="center" 包裹的纯图片块:整块换成带居中 title 的
    /// Markdown 图片行 —— CommonMark 本不解析 HTML 块内 Markdown,靠
    /// 本层落地 GitHub README 的居中图写法(vendored 端按
    /// `CENTERED_IMAGE_TITLE` title 居中)。
    #[test]
    fn div_center_block_becomes_centered_markdown_images() {
        let base = Path::new("/home/u/docs");
        // 基本形态:file:// 目标原样带上,title 挂居中约定值
        let doc = "<div align=\"center\">\n  ![LaterMD](file:///x/i.png)\n</div>";
        assert_eq!(
            rewrite(doc, Some(base)),
            "![LaterMD](file:///x/i.png \"latermd:center\")"
        );
        // `<img>` 标签内容与相对目标:标签转 Markdown,相对按文档目录拼 file://
        let doc = "<p align=\"center\">\n  <img src=\"logo/i.png\" alt=\"标\">\n</p>";
        assert_eq!(
            rewrite(doc, Some(base)),
            "![标](file:///home/u/docs/logo/i.png \"latermd:center\")"
        );
        // 多图各成一行;align 单引号 + 附加属性都认
        let doc = "<div class=\"banner\" align='center'>\n![a](x.png)\n![b](y.png)\n</div>";
        assert_eq!(
            rewrite(doc, None),
            "![a](x.png \"latermd:center\")\n![b](y.png \"latermd:center\")"
        );
        // 目标含空格:源里本就要 `<>` 包裹(裸空格目标是非法 Markdown),
        // 替换串保持包裹
        let doc = "<div align=\"center\">\n![a](<my img.png>)\n</div>";
        assert_eq!(rewrite(doc, None), "![a](<my img.png> \"latermd:center\")");
        // 块外的普通相对图片不受牵连,仍走内联改写(清单不重叠)
        let doc = "<div align=\"center\">\n![a](x.png)\n</div>\n\n![b](y.png)";
        assert_eq!(
            rewrite(doc, Some(base)),
            "![a](file:///home/u/docs/x.png \"latermd:center\")\n\n![b](file:///home/u/docs/y.png)"
        );
    }

    /// div-center 扫描的边界:align 非 center、空行截断、空 div、非图片
    /// 内容行、代码块内、闭标签缺位 —— 一律不改写(整块保持字面文本,
    /// 与改写前的现状行为一致)。
    #[test]
    fn div_center_scan_boundaries() {
        // align 非 center / 缺失
        assert_eq!(
            rewrite("<div align=\"left\">\n![a](x.png)\n</div>", None),
            "<div align=\"left\">\n![a](x.png)\n</div>"
        );
        assert_eq!(
            rewrite("<div>\n![a](x.png)\n</div>", None),
            "<div>\n![a](x.png)\n</div>"
        );
        // 空行截断 HTML 块(语义已不是一个 div)
        assert_eq!(
            rewrite("<div align=\"center\">\n\n![a](x.png)\n</div>", None),
            "<div align=\"center\">\n\n![a](x.png)\n</div>"
        );
        // 空 div / 中间夹了非图片文本行 / 开闭标签不同 tag
        assert_eq!(
            rewrite("<div align=\"center\">\n</div>", None),
            "<div align=\"center\">\n</div>"
        );
        assert_eq!(
            rewrite("<div align=\"center\">\n正文\n</div>", None),
            "<div align=\"center\">\n正文\n</div>"
        );
        assert_eq!(
            rewrite("<div align=\"center\">\n![a](x.png)\n</p>", None),
            "<div align=\"center\">\n![a](x.png)\n</p>"
        );
        // 闭标签缺位(文件截断):不认
        assert_eq!(
            rewrite("<div align=\"center\">\n![a](x.png)", None),
            "<div align=\"center\">\n![a](x.png)"
        );
        // 围栏代码块内的 div-center 是字面文本
        let doc = "```\n<div align=\"center\">\n![a](x.png)\n</div>\n```";
        assert_eq!(rewrite(doc, None), doc);
        // 标签边界:`<picture>`、`<imgx>` 不是开/内容标签
        assert_eq!(
            rewrite("<picture>\n![a](x.png)\n</picture>", None),
            "<picture>\n![a](x.png)\n</picture>"
        );
    }

    /// `<img>` 扫描的边界:围栏代码块内是字面文本不认;非 img 标签
    /// (`<image>`、`<imgx>`)不认;`src` 缺失/裸值/`data-src` 假属性不认。
    #[test]
    fn html_img_scan_boundaries() {
        let base = Path::new("/home/u/docs");
        // 围栏内不认
        let doc = "```\n<img src=\"a.png\">\n```\n\n<img src=\"b.png\">";
        assert_eq!(
            rewrite(doc, Some(base)),
            "```\n<img src=\"a.png\">\n```\n\n![](file:///home/u/docs/b.png)"
        );
        // 标签名必须精确:<img 后跟字母/斜杠不是 img 标签
        assert_eq!(
            rewrite("<image src=\"a.png\">", Some(base)),
            "<image src=\"a.png\">"
        );
        assert_eq!(
            rewrite("<imgx src=\"a.png\">", Some(base)),
            "<imgx src=\"a.png\">"
        );
        // src 缺失 / 裸值(非法 HTML)/ 键被前缀污染
        assert_eq!(rewrite("<img alt=\"a\">", Some(base)), "<img alt=\"a\">");
        assert_eq!(rewrite("<img src=a.png>", Some(base)), "<img src=a.png>");
        assert_eq!(
            rewrite("<img data-src=\"a.png\">", Some(base)),
            "<img data-src=\"a.png\">"
        );
        // 混排:同一文档既有 `![](x)` 又有 `<img>`,两路改写共存且区间互不重叠
        let doc = "![一](a.png)\n\n<img src=\"b.png\" alt=\"二\">";
        assert_eq!(
            rewrite(doc, Some(base)),
            "![一](file:///home/u/docs/a.png)\n\n![二](file:///home/u/docs/b.png)"
        );
    }

    /// 相对图片改写:A 段自产形态(`./foo.assets/中文.png`)、上级目录、
    /// `<…>` 包裹保持包裹、多张图与周边文本逐字节保留;无相对图与未落盘
    /// 时零改写(清单为空 ≡ 文本原样,不产生分配)。
    ///
    /// 这一组随 live-unify P5 从已删的 `ui::preview` 迁来:整篇改写链删除
    /// 后改写由 Live 按块接管(对照预览现为三态分栏档的只读 Live),这四条
    /// 判定用例随函数留在原地。
    #[test]
    fn relative_images_resolve_against_base_dir() {
        let base = Path::new("/home/u/docs");
        let out = rewrite("![图](./foo.assets/中文.png)", Some(base));
        assert_eq!(out, "![图](file:///home/u/docs/foo.assets/中文.png)");
        let out = rewrite("![a](../img/x.png)", Some(base));
        assert_eq!(out, "![a](file:///home/u/docs/../img/x.png)");
        let out = rewrite("![a](<./d/屏幕 截图.png>)", Some(base));
        assert_eq!(out, "![a](<file:///home/u/docs/d/屏幕 截图.png>)");
        let doc = "前文\n\n![一](a.png)中间![二](<b c.png>)\n\n后文";
        let out = rewrite(doc, Some(base));
        assert_eq!(
            out,
            "前文\n\n![一](file:///home/u/docs/a.png)中间![二](<file:///home/u/docs/b c.png>)\n\n后文"
        );
        // 无相对图 / 未落盘:改写清单为空 ≡ 零改写
        let clean = "![x](https://e.com/a.png) 与 [链](rel.md)";
        assert!(image_rewrites(clean, Some(base)).is_empty());
        assert!(image_rewrites("![x](a.png)", None).is_empty());
    }

    /// 不该改写的一律不动:绝对 URL、`wiki://`(wikilink 展开产物)、绝对
    /// 路径、锚点、空目标、围栏代码块内的 `![]()`、链接本身、`data:` 与
    /// Windows 盘符路径。
    #[test]
    fn non_relative_and_non_image_targets_stay_put() {
        let base = Path::new("/home/u/docs");
        let doc = "![a](https://e.com/x.png?w=1)\n![b](<wiki://架构决策>)\n\
                   ![c](/abs/path.png)\n![d](#anchor)\n![]()\n[e 链](./rel.md)\n\
                   ![f](data:image/png;base64,AAAA)\n![g](C:/win.png)\n\n\
                   ```rust\nlet x = ![](inner.png);\n```\n\n![h](ok.png)";
        let out = rewrite(doc, Some(base));
        let expected = doc.replace("![h](ok.png)", "![h](file:///home/u/docs/ok.png)");
        assert_eq!(out, expected);
    }

    /// 存盘 URL 往返:图片落盘产出的相对地址(含空格时包 `<>`)能还原成
    /// 绝对 URI —— 存盘文本与渲染改写两侧的约定互相咬合(`crate::assets`
    /// 单测钉落盘,这里钉「这条地址进渲染后能出图」)。
    #[test]
    fn stored_url_round_trips_into_file_uri() {
        let base = Path::new("/home/u/docs");
        let stored = "<./笔记.assets/屏幕 截图 (1).png>";
        let doc = format!("![alt]({stored})");
        let out = rewrite(&doc, Some(base));
        assert_eq!(
            out,
            "![alt](<file:///home/u/docs/笔记.assets/屏幕 截图 (1).png>)"
        );
    }

    /// 带标题的内联图片:`![a](./x.png "标题")` 目标到空白即止,标题保留。
    #[test]
    fn titled_image_keeps_its_title() {
        let base = Path::new("/d");
        let out = rewrite("![a](./x.png \"题\")", Some(base));
        assert_eq!(out, "![a](file:///d/x.png \"题\")");
    }
}
