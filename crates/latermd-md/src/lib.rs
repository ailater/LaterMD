#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! 拥有型 Markdown 文档模型与大纲数据层。
//!
//! 包裹 vendored [`egui_markdown`] 的解析器(pulldown-cmark 前端),把借用型
//! [`egui_markdown::parser::parse`] 的结果转成拥有型 [`MarkdownDoc`],文档模型
//! 因此可以脱离源字符串的生命周期,存进编辑器缓冲或跨线程传递。
//!
//! 数据层 crate:不依赖 egui/eframe;把 token 流翻译为绘制是 UI 侧的职责。

use std::collections::HashSet;
use std::ops::Range;

use egui_markdown::types::{TableData, Token};
use pulldown_cmark::CowStr;

/// 大纲条目,对应一个标题 token。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineItem {
    /// 标题层级(1–6)。
    pub level: u8,
    /// 标题纯文本,不含 `#` 标记。
    pub text: String,
    /// 标题在 [`MarkdownDoc::text`] 中的字节区间,大纲点击跳转用它定位光标。
    pub span: Range<usize>,
}

/// 拥有型文档模型:源文本、token 流与平行的源码区间。
///
/// `tokens` 与 `spans` 等长且区间连续(继承自 vendored 解析器的不变量),任意
/// 字节偏移都落在恰好一个 token 的区间内 —— 大纲跳转与 Live Preview 依赖这一点。
pub struct MarkdownDoc {
    /// 源文本。
    pub text: String,
    /// 拥有型 token 流,与 `spans` 平行。
    pub tokens: Vec<Token<'static>>,
    /// 各 token 的源码字节区间,与 `tokens` 平行。
    pub spans: Vec<Range<usize>>,
}

impl MarkdownDoc {
    /// 提取标题大纲。
    ///
    /// 只遍历顶层 token,不深入表格单元格;凡 [`Token::Text`] 的样式带 heading
    /// 层级即产出一条。同一标题行被内联格式(行内代码/链接)拆出的多个
    /// token 片段按行合并回一条,口径与导出 HTML 的标题 id 同源(共享
    /// 实现见下方私有 `outline_from`)。
    pub fn outline(&self) -> Vec<OutlineItem> {
        outline_from(&self.tokens, &self.spans)
    }
}

/// 提取标题大纲(借用型快路径)。
///
/// 与 [`MarkdownDoc::outline`] 产出一致,但不做借转拥有的整篇 token 拷贝,
/// 只为命中的标题分配字符串。编辑器每次修订号前进都要重算大纲,应走这一
/// 条;已持有 [`MarkdownDoc`] 的离线场景用方法形态即可。
pub fn outline(text: &str) -> Vec<OutlineItem> {
    let md = egui_markdown::parser::parse(text);
    outline_from(&md.tokens, &md.spans)
}

/// 顶层 token 流 → 标题大纲(两条 [`outline`] 通道的共享实现)。
///
/// vendored 解析器把标题行的内联结构切成多个顶层 token:`Event::Code` 的
/// 样式被重置为纯 `inline_code`(`heading: None`),把 `# 标题含 \`code\`
/// 内联` 切成「标题含 」「code」「 内联」三段;链接则整体成一条
/// [`Token::Link`](heading 字段保留)。逐条直取会让一条标题裂成多条、
/// 文本各剩片段,TOC 与应用内锚点随之和导出 HTML 的标题 id(导出侧按
/// 事件流把 Text+Code 拼回全文再取 slug,见 latermd-export
/// `attach_heading_ids`)对不上。
///
/// 合并口径:标题正文片段([`Token::Text`] 带 heading)接续或新开一条
/// **未闭合 run**;run 内的行内代码片段并入当前条(归属只能靠 run 判定,
/// 样式里没有 heading);标题内的链接文本并入([`Token::Link`] 自带
/// heading,链接开头的标题由它起 run)。**换行 token 关闭 run** ——
/// vendored 解析器在每个标题前都插入换行(`ensure_double_newline`),段落
/// 里的行内代码因此绝不会被误吸。标题以行内代码**开头**时正文片段之前
/// 没有可开的 run,向前回吸同行的行内代码/链接片段(回吸同样必停在换行
/// 或非内联 token 上)。`span` 取各片段的并集;图片的 alt 不进文本(与
/// 导出标题纯文本口径一致)。
fn outline_from(tokens: &[Token<'_>], spans: &[Range<usize>]) -> Vec<OutlineItem> {
    let mut items: Vec<OutlineItem> = Vec::new();
    // 未闭合标题 run 在 `items` 里的下标;None = 当前不在任何标题行内。
    let mut open: Option<usize> = None;
    for (position, (token, span)) in tokens.iter().zip(spans).enumerate() {
        let extend =
            |items: &mut Vec<OutlineItem>, index: usize, text: &str, span: &Range<usize>| {
                items[index].text.push_str(text);
                items[index].span.end = items[index].span.end.max(span.end);
            };
        match token {
            Token::Text { text, style } if style.heading.is_some() => {
                let level = style.heading.unwrap();
                match open {
                    Some(index) if items[index].level == level => {
                        extend(&mut items, index, text, span)
                    }
                    _ => {
                        let mut item = OutlineItem {
                            level,
                            text: text.to_string(),
                            span: span.clone(),
                        };
                        // 行首片段是内联代码(`# \`init\` 脚本`):向前回吸
                        // 同一标题行内、位于正文之前的行内代码/链接文本。
                        let mut back = position;
                        while back > 0 {
                            back -= 1;
                            match &tokens[back] {
                                Token::Text { text: prev, style }
                                    if style.inline_code && style.heading.is_none() =>
                                {
                                    item.text.insert_str(0, prev);
                                    item.span.start = spans[back].start;
                                }
                                Token::Link {
                                    text: prev,
                                    heading: Some(prev_level),
                                    ..
                                } if *prev_level == level => {
                                    item.text.insert_str(0, prev);
                                    item.span.start = spans[back].start;
                                }
                                _ => break,
                            }
                        }
                        items.push(item);
                        open = Some(items.len() - 1);
                    }
                }
            }
            Token::Text { text, style } if style.inline_code && open.is_some() => {
                extend(&mut items, open.unwrap(), text, span)
            }
            Token::Link {
                text,
                heading: Some(level),
                ..
            } => {
                match open {
                    Some(index) if items[index].level == *level => {
                        extend(&mut items, index, text, span)
                    }
                    _ => {
                        // 标题以链接开头(`# [甲](x) 乙`):链接即 run 起点。
                        items.push(OutlineItem {
                            level: *level,
                            text: text.to_string(),
                            span: span.clone(),
                        });
                        open = Some(items.len() - 1);
                    }
                }
            }
            _ => open = None,
        }
    }
    items
}

/// 把文档切成**块**(P3 Live Preview 的骨架):返回每个块的源码字节区间。
///
/// 三条规定(由 Live Preview 的编辑语义倒逼出来):
///
/// 1. **覆盖全文、连续、无重叠**:块区间首尾相接且并集等于整篇 —— 光标块
///    在 Live Preview 下是被编辑的区间,若有字节落在任何块之外,在那儿敲
///    一个字符就会**静默丢失**。
/// 2. **块间空行归前一块**:段落之间的 `\n\n` 收进上一块的尾部(最后一块吃
///    到文末),这样每块的源码都是可直接独立解析的片段,富渲染也不会因为
///    前导空行在顶部空出一段。
/// 3. **块级 token 独占一块**:代码块、表格、分隔线、标题各自成块 —— 它们
///    内部的结构(表格行、代码缩进)不该被相邻段落的源码混进来。
///
/// 空文档返回空表;纯空行文档整篇算一块(编辑需要落点)。
pub fn blocks(text: &str) -> Vec<Range<usize>> {
    let md = egui_markdown::parser::parse(text);
    // ① 先按 token 划出「内容段」(记录内容起点与 span 终点),Newline 单独
    //    处理 —— 它的字节在 ② 里随边界分配。
    // 每段记 (内容起点, 内容终点, 首个 token 的 span 终点):第三元用于
    // ①′ 的构造归属 —— span 会吸收前一块尾部的空行(起点可能落在前一个
    // 构造的区间里),而 span 终点恒在本构造内。
    let mut segments: Vec<(usize, usize, usize)> = Vec::new();
    let mut i = 0;
    while i < md.tokens.len() {
        match &md.tokens[i] {
            Token::Newline => {
                i += 1;
            }
            // 块级:独占一段(代码块从此包含它的 ``` 起始 fence)。起点同样
            // 要跳过前导空行 —— 否则分隔符会归到代码块头上,富渲染时顶部
            // 凭空空出一段
            Token::CodeBlock { .. } | Token::Table(_) | Token::HorizontalRule => {
                let span = &md.spans[i];
                segments.push((skip_newlines(text, span.start), span.end, span.end));
                i += 1;
            }
            Token::Text { style, .. } if style.heading.is_some() => {
                let span = &md.spans[i];
                segments.push((content_start(text, span, &md.tokens[i]), span.end, span.end));
                i += 1;
            }
            // 其余:连续的非块级 token 合并成一段(列表项与它的文本同段)
            _ => {
                let start = content_start(text, &md.spans[i], &md.tokens[i]);
                let mut end = md.spans[i].end;
                let mut j = i + 1;
                while j < md.tokens.len() && !is_segment_break(&md.tokens[j]) {
                    end = md.spans[j].end;
                    j += 1;
                }
                segments.push((start, end, md.spans[i].end));
                i = j;
            }
        }
    }
    if segments.is_empty() {
        // 没有内容 token(空文档 / 纯空行):整篇一块,保证有编辑落点
        return if text.is_empty() {
            Vec::new()
        } else {
            // 不用 `vec![0..len]`:clippy 的 single_range_in_vec_init 会把它
            // 读成「长度为 1 的 Range 序列」的误写;这里确实只要一个区间
            std::iter::once(0..text.len()).collect()
        };
    }
    // ①′ 标记字节归属(vendored span 的缺口):Start/End 内联事件(如
    //    `**`/`_`/`~~`)在 vendored 解析里只翻样式、**不产生 token**,
    //    它们的字节落进任何 token span 之外 —— ② 的连续化会把这类孤儿
    //    字节整段归给**前一块**(块尾吞到下一块内容起点),于是
    //    「**粗体**、…」段被切成「…\n\n**」+「粗体**、…」:Live 富渲染
    //    丢开标记(** 变字面星号)、点击标题进编辑丢 `#`(2026-10-10
    //    坤哥报告的两个症状,默认示例文档即复现)。
    //    补救:原生 pulldown 再走一遍,取每个**块级构造**(标题/段落/
    //    列表项/代码块/表格/分隔线)的完整区间 —— 构造 span 天然包含
    //    自己的行内标记;同一构造的内容段统一吸附到该构造的起/终,
    //    让每个 Live 编辑块都能独立解析。
    let constructs = construct_spans(text);
    let mut snapped: Vec<(usize, usize)> = Vec::with_capacity(segments.len());
    let mut seg_index = 0_usize;
    while seg_index < segments.len() {
        let (start, end, _) = segments[seg_index];
        // 归属按**首个 token 的 span 终点**判:span 起点可能吸收了前一块
        // 尾部的空行而落在上一个构造的区间里(span 终点恒在本构造内)。
        let first_span_end = segments[seg_index].2;
        let construct_index = constructs.partition_point(|c| c.end < first_span_end);
        let containing = constructs
            .get(construct_index)
            .filter(|c| first_span_end > c.start && first_span_end <= c.end);
        let Some(construct) = containing else {
            snapped.push((start, end));
            seg_index += 1;
            continue;
        };
        // 本构造名下的连续内容段(按同一判据)
        let mut last = seg_index;
        while last + 1 < segments.len() {
            let next_span_end = segments[last + 1].2;
            let in_same = next_span_end > construct.start && next_span_end <= construct.end;
            if !in_same {
                break;
            }
            last += 1;
        }
        // 块内容区间 = 构造区间本身:标题/段落的行内标记(`#`/`**`)都在
        // 构造 span 里,构造起点即内容起点(pulldown 的容器 span 不含前导
        // 空行);构造内多段(软换行拆行)同属一个区间,天然合并。
        snapped.push((construct.start, construct.end));
        seg_index = last + 1;
    }
    let segments = snapped.as_slice();
    // ② 连续化:每块**吃到下一块的内容起点**(块间空行归前一块),最后一块
    //    到文末。
    let mut blocks = Vec::with_capacity(segments.len());
    let mut cursor = 0_usize;
    for (index, (start, _end)) in segments.iter().enumerate() {
        let block_end = if index + 1 == segments.len() {
            text.len()
        } else {
            segments[index + 1].0.max(*start)
        };
        let block_start = cursor.min(*start);
        if block_end > block_start {
            blocks.push(block_start..block_end);
            cursor = block_end;
        }
    }
    blocks
}

/// 原生 pulldown 视角的**块级构造**区间(标题/段落/列表项/代码块/表格/
/// 分隔线/引用),按文档序升序。构造 span 含自己的行内标记(`**`/`#`),
/// 正是 ①′ 要补的归属真相。列表按**项**切(与既有分块口径一致:每项
/// 可独立进 Live 编辑);引用块整块一段。任务标记、嵌套构造都在所属
/// 项/段内,不单列。
///
/// 事件区间口径(0.13 实测):容器 Start 事件区间可能为空或只指首字节,
/// 故对构造内**所有事件**取 min(start)/max(end) 累计。
fn construct_spans(text: &str) -> Vec<Range<usize>> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut spans: Vec<Range<usize>> = Vec::new();
    // 列表项、引用和脚注定义包含嵌套块,必须保留外层构造的源码标记。
    struct Open {
        start: usize,
        end: usize,
    }
    let mut stack: Vec<Open> = Vec::new();
    let mut in_paragraph = false;
    let mut para = (usize::MAX, 0_usize);
    let mut code_start: Option<usize> = None;

    let push_block = |start: usize, end: usize, spans: &mut Vec<Range<usize>>| {
        if end > start {
            spans.push(start..end);
        }
    };

    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        // 内层构造闭合时向父层传递范围,避免每个事件扫描所有祖先。
        if let Some(open) = stack.last_mut() {
            open.start = open.start.min(range.start);
            open.end = open.end.max(range.end);
        }
        if in_paragraph {
            para.0 = para.0.min(range.start);
            para.1 = para.1.max(range.end);
        }
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    in_paragraph = true;
                    para = (usize::MAX, 0);
                }
                Tag::Item | Tag::BlockQuote(_) | Tag::FootnoteDefinition(_) => {
                    stack.push(Open {
                        start: range.start,
                        end: range.end,
                    });
                }
                Tag::CodeBlock(kind) => {
                    code_start = Some(if kind.is_indented() {
                        physical_line_start(text, range.start)
                    } else {
                        range.start
                    });
                }
                Tag::List(_) | Tag::Table(_) | Tag::TableHead | Tag::TableRow | Tag::TableCell => {}
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    if in_paragraph && para.1 > para.0 {
                        push_block(para.0, para.1, &mut spans);
                    }
                    in_paragraph = false;
                }
                TagEnd::Item | TagEnd::BlockQuote(_) | TagEnd::FootnoteDefinition => {
                    if let Some(open) = stack.pop() {
                        if let Some(parent) = stack.last_mut() {
                            parent.start = parent.start.min(open.start);
                            parent.end = parent.end.max(open.end);
                        }
                        push_block(open.start, open.end.max(range.end), &mut spans);
                    }
                }
                TagEnd::Heading(_) => {
                    push_block(range.start, range.end, &mut spans);
                }
                TagEnd::CodeBlock => {
                    let start = code_start.take().unwrap_or(range.start);
                    push_block(start, range.end, &mut spans);
                }
                TagEnd::Table => {
                    push_block(range.start, range.end, &mut spans);
                }
                _ => {}
            },
            Event::Rule => {
                push_block(range.start, range.end, &mut spans);
            }
            _ => {}
        }
    }
    spans.sort_by_key(|r| r.start);
    // 只合并**真重叠**(引用块与其内列表项:两套构造各记一遍);相邻留
    // 缝的(标题/段落/项)保持独立 —— 缝里是块间空行,归前一块(②)。
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(spans.len());
    for r in spans {
        match merged.last_mut() {
            Some(last) if r.start < last.end => {
                last.end = last.end.max(r.end);
            }
            _ => merged.push(r),
        }
    }
    merged
}

/// 从 `pos` 定位到所在物理行起点;缩进代码的 pulldown 起始 span
/// 从首行内容开始,独立重解析必须保留行首四个空格。
fn physical_line_start(text: &str, pos: usize) -> usize {
    text[..pos.min(text.len())]
        .rfind('\n')
        .map_or(0, |newline| newline + 1)
}
/// 从 `pos` 起跳过连续的换行与回车(CRLF 的 `\r` 也算分隔符的一部分)。
fn skip_newlines(text: &str, mut pos: usize) -> usize {
    let bytes = text.as_bytes();
    while pos < bytes.len() && (bytes[pos] == b'\n' || bytes[pos] == b'\r') {
        pos += 1;
    }
    pos.min(text.len())
}

/// 段的**内容起点**。
///
/// 不能直接拿 span 起点:vendored 的平铺 span 会吸收前一块尾部的换行(后一段
/// 的 span 里含着 `\n\n` 前缀),代码块闭合的 ``` 也会被算进后一段的 span。
/// 这里用 token 自带的文本在 span 区间内定位真正的起点;块级 token(代码块
/// 的 fence 属于内容)没有这一步,直接用 span 起点。
fn content_start(text: &str, span: &Range<usize>, token: &Token<'_>) -> usize {
    let needle: Option<&str> = match token {
        Token::Text { text, .. } => Some(text.as_ref()),
        Token::ListMarker { marker, .. } => Some(marker.as_ref()),
        Token::Link { text, .. } => Some(text.as_ref()),
        Token::Image { alt, .. } => Some(alt.as_ref()),
        _ => None,
    };
    let end = span.end.min(text.len());
    match needle {
        Some(needle) if !needle.is_empty() && span.start <= end => text[span.start..end]
            .find(needle)
            .map_or(span.start, |offset| span.start + offset),
        _ => span.start,
    }
}

/// 是否要在此 token 处断开内容段(空行与块级 token 都算)。
fn is_segment_break(token: &Token<'_>) -> bool {
    match token {
        Token::Newline => true,
        Token::CodeBlock { .. } | Token::Table(_) | Token::HorizontalRule => true,
        Token::Text { style, .. } => style.heading.is_some(),
        _ => false,
    }
}

/// 活动块**内联标记**字符的字节区间(LP2-1 半隐藏):升序、互不重叠、
/// 相邻段已合并。
///
/// 「标记」= 渲染后不产生正文的源码字符:强调/粗体/删除线的 `*`/`_`/`~~`
/// 定界符、行内代码的反引号、链接/图片的 `[`、`](目标 "标题")`、`!`、
/// autolink 的 `<`/`>`。判定完全跟随 pulldown-cmark(与 vendored 解析器
/// 同一套 options,铁律「单一解析器」):未闭合的 `**` 是普通文本不算
/// 标记;围栏/缩进代码块的内容是 `Event::Text`,不算标记;嵌套强调每层
/// 定界符都计入(`***x***` 合并出前后各一段 `***`)。块级标记(`#`、
/// `>`、列表符、表格竖线)不在本口径内 —— LP2-1 只做内联。
///
/// 输入应是**单个块**的源码([`blocks`] 保证块可独立解析);返回区间是
/// 块内偏移。pulldown 的 Start/End 事件区间覆盖整个构造(0.13 实测),
/// 所以标记 = 「构造区间 − 内部内容区间」的缝隙。
pub fn inline_marks(text: &str) -> Vec<Range<usize>> {
    inline_marks_with_pairs(text).segments
}

/// 一对成对的内联标记(LP2-2 选区扩展):一个内联构造两侧的标记,连同
/// 整个构造的范围。嵌套构造(`***x***`、链接文本里的强调)每层各成一
/// 对,区间彼此重叠是正常的 —— 它们本来就不是同一对。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkPair {
    /// 完整构造(开标记 + 内容 + 闭标记),选区扩展的目标区间。
    pub construct: Range<usize>,
    /// 开侧标记,如 `**`;链接的开侧是 `[`。
    pub opening: Range<usize>,
    /// 闭侧标记;链接的闭侧是 `](目标 "标题")` 整段。
    pub closing: Range<usize>,
}

/// [`inline_marks`] 的全量产出:半隐藏用的合并标记段(LP2-1)+ 选区
/// 扩展用的成对表(LP2-2)。两份表出自**同一次**解析,缓存侧按修订号
/// 一起换新,不会出现「段与配对来自不同版本文本」的错位。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InlineMarks {
    /// 合并后的标记段(升序、互不重叠、相邻已合并)—— 半隐藏绘制的粒度。
    pub segments: Vec<Range<usize>>,
    /// 成对标记表 —— 配对显形与选区扩展的依据。
    pub pairs: Vec<MarkPair>,
}

/// [`inline_marks`] 的成对扩展(LP2-2):同一次 pulldown 走查,除合并
/// 标记段外,把每个定界构造的开/闭缝隙记成一对([`MarkPair`])。
///
/// 单侧孤标记(未闭合的 `**`、`` ` ``、`[`)在 pulldown 语义里是普通文
/// 本,既不产段也不产对 —— 选区扩展对它们天然退化为「不扩展」。
pub fn inline_marks_with_pairs(text: &str) -> InlineMarks {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    // 与 vendored parser.rs 同一套 options:预览把 `~~` 渲染成删除线、把
    // `[x](y)` 渲染成链接,半隐藏的判定必须与渲染同一语义,否则会出现
    // 「遮了不渲染的字符 / 漏了渲染的定界符」
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_TASKLISTS);

    // 开着的定界构造:起点 + 内部已覆盖(内容)区间。闭合时把缝隙记为标记。
    let mut open: Vec<(usize, Vec<Range<usize>>)> = Vec::new();
    let mut marks = Vec::new();
    let mut pairs = Vec::new();

    for (event, span) in Parser::new_ext(text, options).into_offset_iter() {
        match event {
            Event::Start(
                Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Link { .. }
                | Tag::Image { .. },
            ) => {
                // 本构造区间先计入外层(嵌套强调之于链接文本),再开自己的栈帧
                if let Some((_, covered)) = open.last_mut() {
                    covered.push(span.clone());
                }
                open.push((span.start, Vec::new()));
            }
            Event::End(
                TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Link
                | TagEnd::Image,
            ) => {
                let Some((start, covered)) = open.pop() else {
                    continue; // 解析器事件配平,这里只是防御
                };
                let end = span.end;
                let mut covered = covered;
                covered.sort_by_key(|range| range.start);
                let mut gaps = Vec::new();
                let mut cursor = start;
                for inner in covered {
                    let inner = inner.start.max(start)..inner.end.min(end);
                    if inner.start > cursor {
                        gaps.push(cursor..inner.start);
                    }
                    cursor = cursor.max(inner.end);
                }
                if cursor < end {
                    gaps.push(cursor..end);
                }
                // 首末缝隙即开/闭两侧标记(它们天然贴着构造两端);只剩一个
                // 缝隙的退化构造(如空文本链接)拆不出两侧,不成对
                if gaps.len() >= 2 {
                    let closing = gaps.last().expect("len >= 2").clone();
                    pairs.push(MarkPair {
                        construct: start..end,
                        opening: gaps[0].clone(),
                        closing,
                    });
                }
                marks.extend(gaps);
                // 本构造(含它自己的标记)对外层是内容
                if let Some((_, parent)) = open.last_mut() {
                    parent.push(start..end);
                }
            }
            // 行内代码是单事件(区间含反引号与内容):两端的反引号串是标记
            Event::Code(_) => {
                let bytes = text.as_bytes();
                let (start, end) = (span.start, span.end);
                let mut ticks_start = start;
                while ticks_start < end && bytes[ticks_start] == b'`' {
                    ticks_start += 1;
                }
                let mut ticks_end = end;
                while ticks_end > ticks_start && bytes[ticks_end - 1] == b'`' {
                    ticks_end -= 1;
                }
                if start < ticks_start {
                    marks.push(start..ticks_start);
                }
                if ticks_end < end {
                    marks.push(ticks_end..end);
                }
                if start < ticks_start && ticks_end < end {
                    pairs.push(MarkPair {
                        construct: start..end,
                        opening: start..ticks_start,
                        closing: ticks_end..end,
                    });
                }
                if let Some((_, covered)) = open.last_mut() {
                    covered.push(span);
                }
            }
            // 其余事件(Text/SoftBreak/块级 Start/End…):开着的构造把它们
            // 视作内容;块级事件不会出现在内联构造内部,压栈无副作用
            _ => {
                if let Some((_, covered)) = open.last_mut() {
                    covered.push(span);
                }
            }
        }
    }

    // 排序 + 合并相邻/重叠段:`***` 会得到外层与内层各一片标记
    marks.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in marks {
        if range.is_empty() {
            continue;
        }
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    InlineMarks {
        segments: merged,
        pairs,
    }
}

/// 活动块「输入后自动成形」的样式区间(LP2-4):每个**已配平**的内联构造,
/// 给出应套用其渲染格式的**内容**字节区间 —— 区间落在定界符内侧,定界符
/// 本身仍归 LP2-1 半隐藏层淡化,成形与淡化两层职责互不侵吞。
///
/// 判定与 [`inline_marks_with_pairs`] 同一套 pulldown-cmark options(铁律
/// 「单一解析器」):未配平的 `**`/`` ` ``/`[` 在 pulldown 语义里是普通文
/// 本,不出区间 —— 即「打完闭标记的那一刻才成形」,正是「各种输入后自动
/// 变成对应格式」的语义。`==高亮==` 不是 CommonMark 构造,复用
/// [`highlight_spans`] 的扫描(连带它自己的豁免清单:与代码/链接/强调
/// 重叠的整对作废)。
///
/// 输入应是单个块的源码([`blocks`] 保证块可独立解析),返回区间是块内
/// 偏移、按 start 升序。嵌套构造(`***x***`、链接文字里的强调)每层各出
/// 一条区间,重叠是正常的 —— 消费侧按原子字节段合并属性;唯一例外是行
/// 内代码:它整段自成样式、不吃外层强调,与 vendored 渲染「Code 事件
/// 重置 TokenStyle」同一语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineStyleKind {
    /// `**粗体**` / `__粗体__`
    Strong,
    /// `*斜体*` / `_斜体_`
    Emphasis,
    /// `~~删除线~~`
    Strikethrough,
    /// `` `行内代码` ``(内容区间,反引号不在内)
    Code,
    /// `[文字](目标)` 的文字部分与 autolink 的 URL。
    LinkText,
    /// `==高亮==`(扩展语法,非 CommonMark)。
    Highlight,
}

/// [`inline_style_spans`] 的一条产出:构造类型 + 应套用该格式的字节区间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineStyleSpan {
    /// 应套用格式的字节区间(定界符内侧)。
    pub range: Range<usize>,
    /// 构造类型。
    pub kind: InlineStyleKind,
}

/// 抽出全部已配平内联构造的样式区间(LP2-4,详见 [`InlineStyleKind`])。
///
/// 快路径:文本不含任一定界字符(`*` `_` `~` `` ` `` `[` `<` 与 `==`)时
/// 不启动解析,直接空表 —— 纯正文零开销。
pub fn inline_style_spans(text: &str) -> Vec<InlineStyleSpan> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    if !text
        .bytes()
        .any(|b| matches!(b, b'*' | b'_' | b'~' | b'`' | b'[' | b'<'))
        && !text.contains("==")
    {
        return Vec::new();
    }

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_TASKLISTS);

    // 开放构造栈。强调族闭合时按「构造两端的定界符长跑」剥内容:`***x***`
    // 里 Strong 与 Emphasis 的构造区间彼此覆盖,gap 推导会双双落空,而
    // `*`/`_`/`~` 的开闭长跑是唯一稳定事实;链接族相反,文字里的 `]` 与
    // 目标里的 `[]()` 剥不出长跑,只有「内容 gap」推导可靠(与
    // [`inline_marks_with_pairs`] 同式)。各用各的。
    enum Frame {
        /// 强调族:(类型, 构造区间)。
        Emph(InlineStyleKind, Range<usize>),
        /// 链接族:(是否链接而非图片, 构造区间, 内部内容累计)。
        Link(bool, Range<usize>, Vec<Range<usize>>),
    }
    let mut stack: Vec<Frame> = Vec::new();
    let mut spans: Vec<InlineStyleSpan> = Vec::new();
    let feed = |stack: &mut Vec<Frame>, range: Range<usize>| {
        if let Some(Frame::Link(_, _, covered)) = stack.last_mut() {
            covered.push(range);
        }
    };

    for (event, span) in Parser::new_ext(text, options).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                feed(&mut stack, span.clone());
                let frame = match tag {
                    Tag::Strong => Frame::Emph(InlineStyleKind::Strong, span.clone()),
                    Tag::Emphasis => Frame::Emph(InlineStyleKind::Emphasis, span.clone()),
                    Tag::Strikethrough => Frame::Emph(InlineStyleKind::Strikethrough, span.clone()),
                    Tag::Link { .. } => Frame::Link(true, span.clone(), Vec::new()),
                    Tag::Image { .. } => Frame::Link(false, span.clone(), Vec::new()),
                    _ => continue,
                };
                stack.push(frame);
            }
            Event::End(tag_end) => {
                let frame = match tag_end {
                    TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough => stack.pop(),
                    TagEnd::Link | TagEnd::Image => stack.pop(),
                    _ => None,
                };
                match frame {
                    Some(Frame::Emph(kind, range)) => {
                        let bytes = text.as_bytes();
                        let tilde = kind == InlineStyleKind::Strikethrough;
                        let delim = |byte: u8| {
                            if tilde {
                                byte == b'~'
                            } else {
                                byte == b'*' || byte == b'_'
                            }
                        };
                        let mut start = range.start;
                        while start < range.end && delim(bytes[start]) {
                            start += 1;
                        }
                        let mut end = range.end;
                        while end > start && delim(bytes[end - 1]) {
                            end -= 1;
                        }
                        if start < end {
                            spans.push(InlineStyleSpan {
                                range: start..end,
                                kind,
                            });
                        }
                        feed(&mut stack, range);
                    }
                    Some(Frame::Link(is_link, range, covered)) => {
                        // gap 推导:首末 gap 之间的就是链接文字(嵌套内容
                        // 不是 gap)。图片 alt 不出样式 —— 富渲染整块换成
                        // 图片 widget,活动块保持源码观感。
                        let mut covered = covered;
                        covered.sort_by_key(|inner| inner.start);
                        let mut gaps = Vec::new();
                        let mut cursor = range.start;
                        for inner in covered {
                            let inner = inner.start.max(range.start)..inner.end.min(range.end);
                            if inner.start > cursor {
                                gaps.push(cursor..inner.start);
                            }
                            cursor = cursor.max(inner.end);
                        }
                        if cursor < range.end {
                            gaps.push(cursor..range.end);
                        }
                        if is_link && gaps.len() >= 2 {
                            let content = gaps[0].end..gaps[gaps.len() - 1].start;
                            if !content.is_empty() {
                                spans.push(InlineStyleSpan {
                                    range: content,
                                    kind: InlineStyleKind::LinkText,
                                });
                            }
                        }
                        feed(&mut stack, range);
                    }
                    _ => {}
                }
            }
            // 行内代码是单事件(区间含反引号与内容):两端的反引号串剥掉,
            // 内容区间成形。空代码(``` `` ``` 之间无字)不出区间。
            Event::Code(_) => {
                let bytes = text.as_bytes();
                let mut ticks_start = span.start;
                while ticks_start < span.end && bytes[ticks_start] == b'`' {
                    ticks_start += 1;
                }
                let mut ticks_end = span.end;
                while ticks_end > ticks_start && bytes[ticks_end - 1] == b'`' {
                    ticks_end -= 1;
                }
                if ticks_start < ticks_end {
                    spans.push(InlineStyleSpan {
                        range: ticks_start..ticks_end,
                        kind: InlineStyleKind::Code,
                    });
                }
                feed(&mut stack, span);
            }
            _ => feed(&mut stack, span),
        }
    }

    // `==高亮==` 扩展语法:区间含两侧 `==`(ASCII,各 2 字节),内容在中间。
    for highlight in highlight_spans(text) {
        spans.push(InlineStyleSpan {
            range: highlight.span.start + 2..highlight.span.end - 2,
            kind: InlineStyleKind::Highlight,
        });
    }
    spans.sort_by_key(|span| span.range.start);
    spans
}

/// 触发选区扩展的指针手势(LP2-2)。扩展是**一次性**的:只在事件帧传入
/// [`mark_interaction`],静止帧传 `None` —— 选区不会被持续吸附到标记对。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkGesture {
    /// 双击:egui 已在帧内选出词,当前选区即词区间;扩展到包含它的
    /// **最内层**标记对(嵌套时选最小的一对)。
    DoubleClick,
    /// 拖选松手:`anchor` 是按下时光标所在的块内字符偏移。选区整个含在
    /// 标记对内、且锚点压着某一侧标记(含边界,单字符标记只有边界可压)
    /// 时,扩展到该对。
    DragRelease {
        /// 按下帧的塌缩光标(块内字符偏移)。
        anchor: usize,
    },
}

/// 标记与光标/选区的交互决策(LP2-2)。
///
/// - `revealed`:与 `InlineMarks::segments` 平行的显形表。除 LP2-1 的
///   「贴上/相交显形」外,被触碰标记的**配对另一侧**所在段一并显形
///   (识别配对);
/// - `expanded`:扩展后的选区(块内**字符偏移**,TextEdit 的域),无扩
///   展时为 `None`。选区已含完整标记对时结果是同一区间(幂等)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkInteraction {
    /// 与 `segments` 平行:`true` = 该段显形。
    pub revealed: Vec<bool>,
    /// 扩展后的选区(字符偏移);`None` = 本帧不扩展。
    pub expanded: Option<Range<usize>>,
}

/// 计算标记的显形集合与选区扩展(LP2-2 的纯函数核心)。
///
/// 输入的 `caret`/`other`/`anchor` 是块内**字符偏移**(TextEdit 的光标
/// 域),`marks` 是 [`inline_marks_with_pairs`] 的产出(字节域),函数内
/// 部完成两域换算 —— 调用方不需要自己折算 CJK 偏移。
pub fn mark_interaction(
    text: &str,
    marks: &InlineMarks,
    caret: usize,
    other: usize,
    gesture: Option<MarkGesture>,
) -> MarkInteraction {
    let caret_byte = char_to_byte(text, caret);
    let other_byte = char_to_byte(text, other);
    let collapsed = caret_byte == other_byte;
    // 「触碰」口径与 LP2-1 显形一致:塌缩光标紧邻也算(点击落在标记字形
    // 上时光标停在其边界),非空选区取开区间相交。
    let touches = |range: &Range<usize>| {
        if collapsed {
            range.start <= caret_byte && caret_byte <= range.end
        } else {
            let (low, high) = (caret_byte.min(other_byte), caret_byte.max(other_byte));
            range.start < high && low < range.end
        }
    };
    let within = |outer: &Range<usize>, inner: &Range<usize>| {
        outer.start <= inner.start && inner.end <= outer.end
    };
    let mut revealed = Vec::with_capacity(marks.segments.len());
    for segment in &marks.segments {
        let partner = marks.pairs.iter().any(|pair| {
            (touches(&pair.opening) || touches(&pair.closing))
                && (within(segment, &pair.opening) || within(segment, &pair.closing))
        });
        revealed.push(touches(segment) || partner);
    }

    // 选区扩展:只在手势帧;候选对必须把当前选区整个含住(跨对选区不吸
    // 附),拖选还要求锚点压着标记。取最内层(构造最短)的一对。
    let expanded = gesture.and_then(|gesture| {
        let selection = caret_byte.min(other_byte)..caret_byte.max(other_byte);
        let mut best: Option<&MarkPair> = None;
        for pair in &marks.pairs {
            if !within(&pair.construct, &selection) {
                continue;
            }
            let anchored = match gesture {
                MarkGesture::DoubleClick => true,
                MarkGesture::DragRelease { anchor } => {
                    let anchor = char_to_byte(text, anchor);
                    (pair.opening.start <= anchor && anchor <= pair.opening.end)
                        || (pair.closing.start <= anchor && anchor <= pair.closing.end)
                }
            };
            if !anchored {
                continue;
            }
            let better = best.is_none_or(|current| {
                pair.construct.len() < current.construct.len()
                    || (pair.construct.len() == current.construct.len()
                        && pair.opening.len() + pair.closing.len()
                            > current.opening.len() + current.closing.len())
            });
            if better {
                best = Some(pair);
            }
        }
        best.map(|pair| {
            byte_to_char(text, pair.construct.start)..byte_to_char(text, pair.construct.end)
        })
    });
    MarkInteraction { revealed, expanded }
}

/// 块内字符偏移 → 字节偏移(越界钳到文末)。
fn char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(byte, _)| byte)
}

/// 块内字节偏移 → 字符偏移(按「在该字节之前开始的字符数」计)。
fn byte_to_char(text: &str, byte_index: usize) -> usize {
    text.char_indices()
        .take_while(|(byte, _)| *byte < byte_index)
        .count()
}

/// `[[wikilink]]` 展开成的链接 scheme(P3「双向链接」)。
///
/// 预览层按它拦截点击(打开同名文档),非本 scheme 的链接仍交系统浏览器。
pub const WIKI_SCHEME: &str = "wiki://";

/// 一条 `[[wikilink]]`:目标文档名与它在源码中的字节区间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wikilink {
    /// 目标文档名(`[[目标]]` 或 `[[目标|显示名]]` 的前半段)。
    pub target: String,
    /// 链接显示名;省略显示名时与目标同名。
    pub label: String,
    /// `[[` 起到 `]]` 止的源码区间。
    pub span: Range<usize>,
}

/// 抽出全部 `[[wikilink]]`。
///
/// **围栏代码块内的 `[[…]]` 不算链接** —— Rust 的 `arr[[0]]`、嵌套容器字面量
/// 都长这样,展开会把代码改坏。判定与 CommonMark 一致:以 ``` / ~~~ 围栏
/// 切换「代码中」状态。
pub fn wikilinks(text: &str) -> Vec<Wikilink> {
    let mut links = Vec::new();
    let mut in_code = false;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let rest = &text[index..];
        // 行首围栏切换代码态
        let line_start = index == 0 || bytes[index - 1] == b'\n';
        if line_start {
            let trimmed = rest.trim_start_matches([' ', '\t']);
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_code = !in_code;
            }
        }
        if !in_code && rest.starts_with("[[") {
            if let Some(end) = rest.find("]]") {
                let inner = &rest[2..end];
                if !inner.contains('\n') && !inner.contains('[') {
                    let (target, label) = match inner.split_once('|') {
                        Some((target, label)) => (target.trim(), label.trim()),
                        None => (inner.trim(), inner.trim()),
                    };
                    if !target.is_empty() {
                        links.push(Wikilink {
                            target: target.to_owned(),
                            label: label.to_owned(),
                            span: index..index + end + 2,
                        });
                    }
                }
                index += end + 2;
                continue;
            }
        }
        // 按字符前进(不切断 UTF-8)
        let step = text[index..].chars().next().map_or(1, char::len_utf8);
        index += step;
    }
    links
}

/// 一处「区间替换」改写:源文本的 `[span]` 段在渲染文本里被 `replacement`
/// 顶替。改写层(wikilink 展开,以及将来叠加同层的 emoji 短码改写)以它为
/// 原子 —— 同一份清单既拼出渲染文本又回答偏移换算(见 [`OffsetMap`]),
/// 两侧永不漂移。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewrite {
    /// 被替换的源码字节区间。清单内各区间须升序且互不重叠。
    pub span: Range<usize>,
    /// 替换后的渲染文本(在渲染串里占据该位置的整段)。
    pub replacement: String,
}

/// 「源码偏移 ↔ 渲染偏移」映射表(LP2-4):描述「改写层做了什么」,供预览
/// 侧消费点(大纲跳转、section anchor)把源码偏移换算后再用。**只描述改写、
/// 不做改写** —— 源码缓冲、rope、字节偏移、撤销栈一律不动,映射不回写任何
/// 文本;它是与渲染文本同一次产出的**只读派生物**。
///
/// 换算语义(与「区间替换」的形状对齐):
///
/// - 区间之外的偏移,按前方各区间的累积长度差平移;
/// - 落在区间内的源偏移,映到该段在渲染串里的**起点** —— 改写段内部没有
///   更细的锚点,落点即段首(反向同理:渲染段内 → 源码段首);
/// - 偏移恰在区间端点上按「区间外」处理(端点属于后文)。
///
/// **可组合**:每层改写各自产出一张表,第二层的区间基于第一层的**输出
/// 文本**(改写按顺序串行发生),消费点把偏移按同一顺序串行穿过即可:
/// `map2.source_to_rendered(map1.source_to_rendered(off))`。后续 #48 的
/// emoji 短码改写与 wikilink 展开同层叠加时,照此再串一张表,无需预合并
/// (预合并要把第二层区间反解回源码坐标,复杂度换不来收益)。app 侧
/// `PreviewState::rendered` 的生产链(wikilink 展开 → 相对图片 URI 改写)
/// 即此口径的实例。
#[derive(Debug, Clone)]
pub struct OffsetMap {
    rewrites: Vec<Rewrite>,
}

impl OffsetMap {
    /// 空表:恒等映射(无改写层)。
    pub fn empty() -> Self {
        Self {
            rewrites: Vec::new(),
        }
    }

    /// 从区间替换清单构造。清单须按 `span` 升序且互不重叠(生产者
    /// [`expand_wikilinks_with_map`] 与改写扫描器都天然满足)。
    pub fn from_rewrites(rewrites: Vec<Rewrite>) -> Self {
        debug_assert!(
            rewrites
                .windows(2)
                .all(|pair| pair[0].span.end <= pair[1].span.start),
            "改写清单须升序且互不重叠: {rewrites:?}"
        );
        Self { rewrites }
    }

    /// 是否恒等(没有任何改写)。
    pub fn is_identity(&self) -> bool {
        self.rewrites.is_empty()
    }

    /// 改写清单(测试契约用:断言区间形状与字符边界;生产侧不该读它,
    /// 消费偏移换算而非清单本身)。
    #[cfg(test)]
    pub fn rewrites_for_test(&self) -> &[Rewrite] {
        &self.rewrites
    }

    /// 把清单应用到源文本,拼出渲染文本 —— 与偏移换算共用同一份数据,
    /// 「渲染串长什么样」与「偏移怎么换算」是单一事实来源。
    pub fn apply(&self, text: &str) -> String {
        if self.rewrites.is_empty() {
            return text.to_owned();
        }
        let mut out = String::with_capacity(text.len() + 64);
        let mut cursor = 0;
        for rewrite in &self.rewrites {
            out.push_str(&text[cursor..rewrite.span.start]);
            out.push_str(&rewrite.replacement);
            cursor = rewrite.span.end;
        }
        out.push_str(&text[cursor..]);
        out
    }

    /// 源码字节偏移 → 渲染字节偏移。
    pub fn source_to_rendered(&self, source_offset: usize) -> usize {
        let mut delta: i64 = 0;
        for rewrite in &self.rewrites {
            let old_len = (rewrite.span.end - rewrite.span.start) as i64;
            if rewrite.span.end <= source_offset {
                delta += rewrite.replacement.len() as i64 - old_len;
            } else if rewrite.span.start > source_offset {
                break;
            } else {
                // 区间内:该段在渲染串里的起点(前方位移已累积进 delta)
                return rewrite.span.start + delta.max(0) as usize;
            }
        }
        (source_offset as i64 + delta).max(0) as usize
    }

    /// 渲染字节偏移 → 源码字节偏移([`Self::source_to_rendered`] 的逆:区间
    /// 外平移互逆、段首互为原像;改写段内部没有更细锚点,映回源码段首)。
    pub fn rendered_to_source(&self, rendered_offset: usize) -> usize {
        let mut delta: i64 = 0;
        for rewrite in &self.rewrites {
            let old_len = (rewrite.span.end - rewrite.span.start) as i64;
            let rendered_start = rewrite.span.start as i64 + delta;
            let rendered_end = rendered_start + rewrite.replacement.len() as i64;
            if rendered_end <= rendered_offset as i64 {
                delta += rewrite.replacement.len() as i64 - old_len;
            } else if rendered_start > rendered_offset as i64 {
                break;
            } else {
                return rewrite.span.start;
            }
        }
        (rendered_offset as i64 - delta).max(0) as usize
    }
}

/// 把 `[[目标]]` 展开成 Markdown 链接 `[显示名](<wiki://目标>)`,供**预览
/// 渲染**使用;源码本身一字不改(roadmap P0 验收:「`.md` 保持原样」)。
///
/// 目标用尖括号包裹:CommonMark 的 `<…>` 链接目标允许空格,中文与带空格的
/// 文档名才不会被解析器截断。
pub fn expand_wikilinks(text: &str) -> String {
    expand_wikilinks_with_map(text).0
}

/// [`expand_wikilinks`] 带偏移映射的版本:同一次扫描既产出渲染文本,也产出
/// 「源码偏移 ↔ 渲染偏移」表(LP2-4)。渲染串由映射表自己拼出(`OffsetMap::
/// apply`),**不存在「公式推导长度」这条近似路径** —— 预览侧消费(大纲跳
/// 转/section anchor 的偏移换算)拿到的映射与渲染文本天然逐字节一致。
pub fn expand_wikilinks_with_map(text: &str) -> (String, OffsetMap) {
    let rewrites: Vec<Rewrite> = wikilinks(text)
        .into_iter()
        .map(|link| Rewrite {
            replacement: format!("[{}](<{}{}>)", link.label, WIKI_SCHEME, link.target),
            span: link.span,
        })
        .collect();
    let map = OffsetMap::from_rewrites(rewrites);
    let rendered = map.apply(text);
    (rendered, map)
}

/// emoji 链接改写产出链接的 scheme(#48 B1):预览层按前缀拦截(inline
/// widget 画纹理、点击吞掉),非本 scheme 的链接照常走默认行为。
pub const EMOJI_SCHEME: &str = "emoji://";

/// 把 `covered` 覆盖的 emoji 改写成 `[原文](<emoji://原文>)`,供**预览渲染**
/// 使用;源码一字不动 —— 与 [`expand_wikilinks`] 同一承诺、同一层叠加。
/// 载荷用原文直书而非百分号编码:尖括号目标允许除换行与 `>` 外的一切
/// 字符,emoji 覆盖集不含这两者,反向解码 = 剥 [`EMOJI_SCHEME`] 前缀,
/// 与 `wiki://` 的既有口径一致。
///
/// 叠加顺序固定:**wikilink 展开 → 本层 → 相对图片 URI**。wikilink 展开把
/// `[[X]]` 变成链接后,本层才能把它的文本/目标整体豁免;反过来会把 wikilink
/// 拆坏(裸 `[[…]]` 在 pulldown 眼里不是链接)。
///
/// 豁免纪律(照 [`wikilinks`] 与 app 侧 `inline_image_dests` 的先例,机制上
/// 直接走与渲染同一套 pulldown-cmark —— 铁律「单一解析器」,LP2-1
/// `inline_marks` 的同款做法):
///
/// - 围栏与缩进代码块内不改写(代码块里没有链接语法,改写会字面显示),
///   区间含 fence 行与 info string;
/// - 行内代码 `` `…` `` 内不改写;
/// - 已在链接/图片的**文本或目标**里不改写 —— 嵌套链接不是合法
///   CommonMark,改写会把既有链接拆坏(展开过的 wikilink 靠这条被保护);
/// - HTML 块、行内 HTML 标签、脚注引用与脚注定义(含 `[^标签]:`)不改写。
///
/// 快路径:文本不含任何覆盖枚的首字符时不启动解析,直接恒等返回 ——
/// 多数文档没有 emoji,不该为它付整篇解析。
pub fn expand_emoji_links(text: &str, covered: &HashSet<&str>) -> (String, OffsetMap) {
    let map = OffsetMap::from_rewrites(emoji_rewrites(text, covered));
    (map.apply(text), map)
}

/// [`expand_emoji_links`] 的扫描核心(私有):覆盖枚的区间替换清单。
fn emoji_rewrites(text: &str, covered: &HashSet<&str>) -> Vec<Rewrite> {
    use pulldown_cmark::{Event, Options, Parser, Tag};

    // 覆盖集的形状:首字符集(快路径粗筛)与最长枚的字符数(前缀长优先
    // 匹配 —— 旗帜与带 FE0F 的形态是两字符,短匹配会把它们的尾巴吃剩)。
    let mut first_chars = HashSet::new();
    let mut max_chars = 0_usize;
    for glyph in covered {
        let mut chars = glyph.chars();
        if let Some(first) = chars.next() {
            first_chars.insert(first);
            max_chars = max_chars.max(1 + chars.count());
        }
    }
    if max_chars == 0 || !text.chars().any(|c| first_chars.contains(&c)) {
        return Vec::new();
    }

    // 与 vendored parser.rs 同一套 options:渲染认得的构造这里才认得,
    // 否则会出现「改写了不渲染成链接的字符 / 漏豁免渲染认的构造」。
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_TASKLISTS);

    // 豁免区间:pulldown 0.13 的 Start 事件区间覆盖整个构造(含代码块的
    // fence 与链接的目标部分;LP2-1 inline_marks 依赖的同一事实,本轮探针
    // 复证)。脚注定义的区间含标签行,正文一并豁免是保守方向(漏改写只是
    // 该处黑白,不改写错才拆语法)。
    let mut exempt: Vec<Range<usize>> = Vec::new();
    for (event, span) in Parser::new_ext(text, options).into_offset_iter() {
        match event {
            Event::Start(
                Tag::CodeBlock { .. }
                | Tag::Link { .. }
                | Tag::Image { .. }
                | Tag::HtmlBlock
                | Tag::FootnoteDefinition(_),
            )
            | Event::Code(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::FootnoteReference(_) => exempt.push(span),
            _ => {}
        }
    }
    exempt.sort_unstable_by_key(|span| span.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in exempt {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }

    let mut rewrites = Vec::new();
    let mut skip = 0_usize;
    let mut index = 0_usize;
    // 各候选前缀的字节长度表,循环外建好复用(每位置最多 max_chars 个)
    let mut prefix: Vec<usize> = Vec::with_capacity(max_chars);
    while index < text.len() {
        while skip < merged.len() && merged[skip].end <= index {
            skip += 1;
        }
        if skip < merged.len() && merged[skip].start <= index {
            index = merged[skip].end;
            continue;
        }
        prefix.clear();
        let mut bytes = 0_usize;
        for c in text[index..].chars().take(max_chars) {
            bytes += c.len_utf8();
            prefix.push(bytes);
        }
        let mut matched = 0_usize;
        for len in prefix.iter().rev() {
            if covered.contains(&text[index..index + *len]) {
                matched = *len;
                break;
            }
        }
        if matched > 0 {
            let glyph = &text[index..index + matched];
            rewrites.push(Rewrite {
                span: index..index + matched,
                replacement: format!("[{glyph}](<{EMOJI_SCHEME}{glyph}>)"),
            });
            index += matched;
        } else {
            index += text[index..].chars().next().map_or(1, char::len_utf8);
        }
    }
    rewrites
}

/// 任务列表 checkbox 链接改写产出链接的 scheme(#63):预览层按前缀拦截
/// (inline widget 画 checkbox、点击吞掉并回写源码),非本 scheme 的链接
/// 照常走默认行为。载荷形态 `task://u<偏移>` / `task://c<偏移>`:`u`/`c`
/// 是未勾/已勾态(渲染侧据此画空框或对勾),`<偏移>` 是**本层输入文本**
/// 中 `[` 的字节偏移 —— 消费点按改写顺序逆穿各层映射换算回源码坐标。
pub const TASK_SCHEME: &str = "task://";

/// 任务 checkbox 的透明占位文字(#63):链接文本本体,渲染时透明追加,
/// 推进宽度与「vendored 原生把任务标记画成 `☑ ` 两字形」同量级(符号 +
/// 尾空格),改写前后的文本流视觉密度不漂移。真正的 checkbox 由 app 侧
/// 在占位区上自绘。
const TASK_PLACEHOLDER: &str = "☐ ";

/// 把任务列表标记 `[ ]`/`[x]`(含大写 `[X]`)改写成
/// `[☐ ](<task://u偏移>)` 形态的链接,供**预览/Live 富渲染**使用;源码
/// 一字不动 —— 与 [`expand_emoji_links`] 同一承诺、再叠一层。改写后
/// pulldown 不再产出 TaskListMarker 事件(标记变成了链接),渲染从
/// vendored 的字面 `☑ `/`☐ ` 符号换成 app 侧 inline widget 自绘的
/// 可点 checkbox。
///
/// **判定即豁免**:改写区间只取 pulldown `TaskListMarker` 事件的 range
/// (实测 pulldown 0.13.4:range 恰覆盖三个字符,标记后无空格的形态如
/// `- [ ]无空格` 不产出事件)。因此代码块/行内代码/普通文本里的 `[ ]`
/// 天然不改写,非任务列表文档恒等返回 —— 这就是「行首定义」的口径:
/// 以 pulldown 事件为准,不在 app 侧二次猜列表缩进与 `-`/`*`/`+` 前缀。
///
/// 快路径:文本不含 `[` 时不启动解析,直接恒等返回。
pub fn expand_task_links(text: &str) -> (String, OffsetMap) {
    let map = OffsetMap::from_rewrites(task_rewrites(text));
    (map.apply(text), map)
}

/// [`expand_task_links`] 的扫描核心(私有):TaskListMarker 事件的区间
/// 替换清单。与 emoji 层不同,本层不需要豁免区间 —— 命中集本身就是
/// 「确认过是任务标记」的区间,再减豁免只是重复 pulldown 的工作。
fn task_rewrites(text: &str) -> Vec<Rewrite> {
    use pulldown_cmark::{Event, Parser};

    if !text.contains('[') {
        return Vec::new();
    }
    let mut rewrites = Vec::new();
    for (event, span) in Parser::new_ext(text, task_options()).into_offset_iter() {
        let Event::TaskListMarker(checked) = event else {
            continue;
        };
        rewrites.push(Rewrite {
            replacement: format!(
                "[{TASK_PLACEHOLDER}](<{TASK_SCHEME}{}{}>)",
                if checked { 'c' } else { 'u' },
                span.start
            ),
            span,
        });
    }
    rewrites
}

/// 与 vendored parser.rs 同一套 options(emoji 层同款纪律):渲染认得
/// 的构造这里才认得,ENABLE_TASKLISTS 不开就全漏。
fn task_options() -> pulldown_cmark::Options {
    let mut options = pulldown_cmark::Options::empty();
    options.insert(pulldown_cmark::Options::ENABLE_STRIKETHROUGH);
    options.insert(pulldown_cmark::Options::ENABLE_TABLES);
    options.insert(pulldown_cmark::Options::ENABLE_FOOTNOTES);
    options.insert(pulldown_cmark::Options::ENABLE_TASKLISTS);
    options
}

/// 判定 `byte` 是否恰为某个任务列表标记 `[` 的字节位置(pulldown 事件
/// 口径,与 [`expand_task_links`] 同一次语义)。回写源码前的最终核验
/// (#63):点击消息携带的偏移可能因并发编辑过期,与其猜「三字符形态」,
/// 不如重扫一遍 —— 行内代码、代码块、普通文本里的 `[ ]` 都不会命中,
/// 误触在归约层被这条闸住。点击是低频事件,一次全文解析(与 emoji 层
/// 同量级)可接受。
pub fn is_task_marker_start(text: &str, byte: usize) -> bool {
    use pulldown_cmark::{Event, Parser};

    if !text.contains('[') || byte >= text.len() || !text.is_char_boundary(byte) {
        return false;
    }
    Parser::new_ext(text, task_options())
        .into_offset_iter()
        .any(|(event, span)| matches!(event, Event::TaskListMarker(_)) && span.start == byte)
}

/// `==高亮==` 改写产出链接的 scheme(#65):预览层按前缀拦截,非本 scheme
/// 的链接照常走默认行为。载荷为空 —— 链接**文字本体**就是高亮内容(与
/// emoji/task 的「scheme 后带载荷」不同),无解码步骤,也没有编码约束。
pub const HIGHLIGHT_SCHEME: &str = "hl://";

/// 一处 `==高亮==`:内部文本与它在源码中的字节区间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    /// `==` 与 `==` 之间的文本(原样,不 trim)。
    pub inner: String,
    /// 开 `==` 起到闭 `==` 止的源码区间(含两侧标记)。
    pub span: Range<usize>,
}

/// 抽出全部成对的 `==高亮==`(#10 [`wikilinks`] 先例)。
///
/// 口径(自选,登记 decisions-pending #123):
///
/// - **贴字定界(flanking)**:开 `==` 的后一字符必须非空白(char 级
///   判定),闭 `==` 的前一字符必须非空白(ASCII 级判定,全角空格等
///   多字节空白视作内容)—— `== ==`、`x== `、`==x ==` 都不配对;
/// - **最左最近配对、不嵌套**:开标与第一个通过闭标 flanking 的 `==`
///   配对;该对因内容/豁免作废时,开标按字面、闭标不回收 ——
///   `==a ==b== c==` 只高亮 `b`;
/// - **内部禁**空、`\n`(行内构造不跨块,wikilink 内部禁换行同一口径)、
///   `[`/`]`(改写形态是链接文字 [`expand_highlight_links`],未配平的
///   方括号会把链接拆成字面)与 `==`(嵌套形态);
/// - **豁免与渲染同一语义**(照 [`expand_emoji_links`] 的豁免纪律:同一套
///   pulldown-cmark options):候选区间(含两侧标记)与代码块/行内代码/
///   链接/图片/HTML/脚注/**强调类**的任一区间重叠即整对作废。比 emoji 层
///   多豁免强调类,是渠道形态决定的 —— vendored 渲染对链接文字只取其中
///   **最后一个** Text/Code 事件(parser.rs `in_link` 分支),多段内容的
///   链接会**丢字**而非仅丢样式,宁缺勿错。
pub fn highlight_spans(text: &str) -> Vec<HighlightSpan> {
    highlight_candidates(text)
        .into_iter()
        .map(|(span, inner)| HighlightSpan { inner, span })
        .collect()
}

/// 把 `==高亮==` 改写成 `[高亮](<hl://>)`,供**预览渲染**使用;源码一字
/// 不动 —— 与 [`expand_wikilinks`] 同一承诺、同一层叠加。
///
/// 叠加顺序固定:**wikilink 展开 → 本层 → emoji 链接改写 → 任务 checkbox
/// 改写**。排在 wikilink 之后:展开出的链接文本/目标里的 `==` 靠豁免清单
/// 不误伤(裸改写会把既有链接拆坏);排在 emoji 之前:`==😀==` 先成
/// `hl://` 链接,emoji 层的「链接文本/目标整体豁免」顺势保护它 —— 高亮
/// 保住、内部 emoji 以字形直显(不再做成纹理 widget)。
///
/// 快路径:文本不含 `==` 时不启动解析,直接恒等返回。
pub fn expand_highlight_links(text: &str) -> (String, OffsetMap) {
    let rewrites: Vec<Rewrite> = highlight_candidates(text)
        .into_iter()
        .map(|(span, inner)| Rewrite {
            replacement: format!("[{inner}](<{HIGHLIGHT_SCHEME}>)"),
            span,
        })
        .collect();
    let map = OffsetMap::from_rewrites(rewrites);
    (map.apply(text), map)
}

/// [`highlight_spans`] / [`expand_highlight_links`] 的扫描核心(私有):
/// `(含两侧标记的源码区间, 内部文本)` 清单,升序且互不重叠。
fn highlight_candidates(text: &str) -> Vec<(Range<usize>, String)> {
    if !text.contains("==") {
        return Vec::new();
    }
    let exempt = highlight_exempt_ranges(text);
    let overlaps_exempt = |candidate: &Range<usize>| {
        exempt
            .iter()
            .any(|range| range.start < candidate.end && candidate.start < range.end)
    };
    let bytes = text.as_bytes();
    let mut candidates = Vec::new();
    let mut index = 0_usize;
    'scan: while index + 2 <= text.len() {
        if !text[index..].starts_with("==") {
            index += text[index..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        // 开标 flanking:后一字符必须非空白(文末视为空白)
        if text[index + 2..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
        {
            index += 2;
            continue;
        }
        // 闭标:第一个「前驱非空白」的 ==(ASCII 级判定,多字节字节的
        // 尾巴天然非 ASCII 空白);它就是本对的定终闭标
        let mut probe = index + 2;
        while let Some(rel) = text[probe..].find("==") {
            let close = probe + rel;
            if !bytes[close - 1].is_ascii_whitespace() {
                let end = close + 2;
                let inner = &text[index + 2..close];
                let ok = !inner.is_empty()
                    && !inner.contains('\n')
                    && !inner.contains('[')
                    && !inner.contains(']')
                    && !inner.contains("==")
                    && !overlaps_exempt(&(index..end));
                if ok {
                    candidates.push((index..end, inner.to_owned()));
                    index = end;
                    continue 'scan;
                }
                // 首个合法闭标即定终:整对作废,开标按字面,闭标不回收
                break;
            }
            probe = close + 2;
        }
        index += 2;
    }
    candidates
}

/// 高亮层的豁免区间(私有):与 [`expand_emoji_links`] 同一豁免清单再叠加
/// 强调类(Start 事件区间覆盖整个构造,pulldown 0.13 实测事实,emoji 层与
/// LP2-1 `inline_marks` 都依赖)。清单不与 emoji 层共享参数化:两边不同
/// (emoji 不豁免强调),两份各自贴着各自的承诺。
fn highlight_exempt_ranges(text: &str) -> Vec<Range<usize>> {
    use pulldown_cmark::{Event, Options, Parser, Tag};

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut exempt: Vec<Range<usize>> = Vec::new();
    for (event, span) in Parser::new_ext(text, options).into_offset_iter() {
        match event {
            Event::Start(
                Tag::CodeBlock { .. }
                | Tag::Link { .. }
                | Tag::Image { .. }
                | Tag::HtmlBlock
                | Tag::FootnoteDefinition(_)
                | Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough,
            )
            | Event::Code(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::FootnoteReference(_) => exempt.push(span),
            _ => {}
        }
    }
    exempt.sort_unstable_by_key(|span| span.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in exempt {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// 定位指定标题的「节」在源文本中的字节区间:从该标题起到下一个**不深于**
/// 它的标题前(无则到文末)。更深层级的标题(`###`)是该节的子内容,一并
/// 属于节;标题文本按 `trim` 后全等匹配,层级精确匹配。
///
/// 起点与终点都取大纲条目的 `span.start`:平铺 span 会吸收前一块尾部的
/// 换行(含标题前的空行),因此移除该区间会连同节的前导空行一起带走,
/// 删后正文仍以合法换行结尾、下一个标题前也仍留有自己的空行。
pub fn heading_section_span(text: &str, level: u8, heading: &str) -> Option<Range<usize>> {
    let outline = outline(text);
    let index = outline
        .iter()
        .position(|item| item.level == level && item.text.trim() == heading)?;
    let start = outline[index].span.start;
    let end = outline[index + 1..]
        .iter()
        .find(|item| item.level <= level)
        .map_or_else(|| text.len(), |next| next.span.start);
    Some(start..end)
}

/// TOC(目录)生成的深度上限:收录 1–3 级标题(#66 M1,口径登记
/// decisions-pending #126)。消费方 [`generate_toc`] 与测试读同一常量,
/// 要调层级改这一处即可。
pub const TOC_MAX_DEPTH: u8 = 3;

/// 标题纯文本 → 锚点 slug(无去重,导出 HTML 的标题 id 与 TOC 链接共用)。
///
/// 规则(GitHub 风格 + CJK 保留,自选口径登记 decisions-pending #126;
/// 导出现状 `export_html` 原本无锚点,HTML 侧的 id 由 latermd-export 按本
/// 函数补齐,两侧一致性由构造保证):
///
/// - Unicode 空白 → `-`(逐字符替换,连续空白产出连续 `-`);
/// - `alphanumeric`(含 CJK、假名等)与 `-`/`_` 保留,字母走
///   `to_lowercase`(CJK 无大小写,原样通过);
/// - 其余(标点/emoji/符号)删除;
/// - **不 trim**:首尾空白产出首尾 `-`(`# 🚀 Launch` → `-launch`,
///   与 GitHub anchor 同款),规则纯函数化,逐字符可断言。
pub fn heading_slug(text: &str) -> String {
    let mut slug = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_whitespace() {
            slug.push('-');
        } else if c.is_alphanumeric() || c == '-' || c == '_' {
            slug.extend(c.to_lowercase());
        }
    }
    slug
}

/// 标题 slug 分配器:同一份文档序贯分配,重复标题去重(`x` → `x-1` →
/// `x-2`,GitHub 口径)。
///
/// TOC 生成与导出 id 注入**必须各自持有一个实例、但对同一标题全集按同一
/// 顺序调用** —— 去重计数不分层级,深度外的标题([`TOC_MAX_DEPTH`] 之外)
/// 也要过一遍才能让两侧锚点不漂移([`generate_toc`] 内部已按此口径)。
#[derive(Debug, Default, Clone)]
pub struct Slugger {
    /// 已分配的 slug 集合(`insert` 语义:命中即返回 false 触发加后缀重试)。
    seen: HashSet<String>,
}

impl Slugger {
    /// 空分配器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 为下一条标题分配去重后的 slug。
    pub fn slug(&mut self, text: &str) -> String {
        let base = heading_slug(text);
        let mut candidate = base.clone();
        let mut serial = 0_usize;
        while !self.seen.insert(candidate.clone()) {
            serial += 1;
            candidate = format!("{base}-{serial}");
        }
        candidate
    }
}

/// 大纲 → TOC markdown 文本(#66 M1 纯函数):层级缩进列表
/// `- [标题](#锚点)`,每行以 `\n` 收尾。
///
/// 口径(登记 decisions-pending #126):
///
/// - **深度上限** [`TOC_MAX_DEPTH`](=3):h4–h6 不进目录;
/// - **锚点**与导出 HTML 的标题 id 同一条规则:全部条目(**含 depth 外**)
///   按文档序消费同一 [`Slugger`] —— 去重计数与导出侧对齐,只是 depth 外
///   的条目不产出文本行;
/// - **缩进** = 2 空格 × (`level` − 入选条目的最小层级):相对层级保持,
///   且首行永远零缩进 —— 按绝对层级缩进会让 h3 开头的文档首行吃 4 空格,
///   在 CommonMark 里落成缩进代码块;
/// - **空文档/无标题**(空切片或无 depth 内条目)返回**空串**,不做提示
///   (提示属 UI,消费方拿空串自行决定);
/// - 显示文本转义 `[`/`]`/`\`,含字面方括号的标题不拆坏链接语法。
///
/// 标题纯文本来自 [`outline`] 通道:含内联格式的标题行已按行合并成一条,
/// 锚点与导出 HTML 的标题 id 同源(同一份全文取 slug)。纯 emoji
/// 标题 slug 为空,产出的链接是合法语法 `(#)`——点击无效但文本合法。
pub fn generate_toc(items: &[OutlineItem]) -> String {
    let Some(base) = items
        .iter()
        .map(|item| item.level)
        .filter(|level| *level <= TOC_MAX_DEPTH)
        .min()
    else {
        return String::new();
    };
    let mut slugger = Slugger::new();
    let mut out = String::new();
    for item in items {
        // 去重计数吃全部标题(与导出侧 id 注入同序同集),只收录 depth 内条目
        let anchor = slugger.slug(&item.text);
        if item.level > TOC_MAX_DEPTH {
            continue;
        }
        for _ in 0..item.level - base {
            out.push_str("  ");
        }
        out.push_str("- [");
        out.push_str(&toc_label(&item.text));
        out.push_str("](#");
        out.push_str(&anchor);
        out.push_str(")\n");
    }
    out
}

/// TOC 条目显示文本的链接转义(私有):`[`/`]`/`\` 前置反斜杠。
fn toc_label(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '[' | ']' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// TOC 块的开始标记行(LaterMD 生成形态,HTML 注释;#66 M2 的区域识别
/// 口径登记 decisions-pending #127)。
pub const TOC_BEGIN_MARKER: &str = "<!-- TOC -->";

/// TOC 块的结束标记行(与 [`TOC_BEGIN_MARKER`] 配对)。
pub const TOC_END_MARKER: &str = "<!-- /TOC -->";

/// 大纲 → 完整 TOC 块(#66 M2「插入/更新目录」的写入形态):[`generate_toc`]
/// 产出被 [`TOC_BEGIN_MARKER`]/[`TOC_END_MARKER`] 两行包围,行以 `\n` 收尾,
/// 产出的块可被 [`toc_region_span`] 原样识别回来 —— 写入与识别同一事实源,
/// 不存在「生成的块认不出」的漂移面。
///
/// 空大纲(无 depth 内标题)产出的块只有两行空壳标记;消费方应先判
/// 「无可收录标题」并落 UI 提示,不要把空壳块写进文档(提示属消费方,
/// 与 [`generate_toc`] 回空串的口径同源)。
pub fn toc_block(items: &[OutlineItem]) -> String {
    format!(
        "{TOC_BEGIN_MARKER}\n{}{TOC_END_MARKER}\n",
        generate_toc(items)
    )
}

/// 在 `text` 中定位既有 TOC 块的字节区间(整块替换的识别口径):第一行
/// 「整行恰为 [`TOC_BEGIN_MARKER`]」的行起,到其后第一行「整行恰为
/// [`TOC_END_MARKER`]」的行(**含行尾换行**)止。
///
/// 口径(decisions-pending #127):
///
/// - 行内容经 `trim` 全等比较 —— 容忍行尾空白与 CRLF 的 `\r`,不容忍
///   标记与其它文本混排一行;
/// - 取**第一对**配对标记;有头无尾、或结束标记都出现在开始标记之前,
///   视为无块(`None`,消费方按「首次插入」处理);
/// - 已知边界:标记行写在围栏代码块内同样会被识别(不做围栏感知,与
///   doctoc 等工具同宽);区域内的手工改动由消费方整块替换覆盖。
pub fn toc_region_span(text: &str) -> Option<Range<usize>> {
    let mut offset = 0;
    let mut begin: Option<usize> = None;
    for line in text.split_inclusive('\n') {
        match (begin, line.trim()) {
            (None, marker) if marker == TOC_BEGIN_MARKER => begin = Some(offset),
            (Some(start), marker) if marker == TOC_END_MARKER => {
                return Some(start..offset + line.len());
            }
            _ => {}
        }
        offset += line.len();
    }
    None
}

/// 解析 Markdown 文本,产出拥有型文档模型(统一入口)。
///
/// 代价是两次拷贝:源文本进 `String`,借用型 `CowStr::Borrowed` 转堆上的
/// `CowStr::Boxed`;换来的是结果不借用任何外部数据。
pub fn parse(text: &str) -> MarkdownDoc {
    let md = egui_markdown::parser::parse(text);
    MarkdownDoc {
        tokens: md.tokens.iter().map(token_to_owned).collect(),
        spans: md.spans,
        text: text.to_string(),
    }
}

/// 借用型 token 转 `Token<'static>`。
///
/// 做法与 vendor `label.rs` 的 `tokens_to_owned` 一致:所有 `CowStr` 一律转
/// `CowStr::Boxed`(堆分配,不指向源文本),Table 递归处理单元格。
fn token_to_owned(token: &Token<'_>) -> Token<'static> {
    match token {
        Token::Newline => Token::Newline,
        Token::Text { text, style } => Token::Text {
            text: cowstr_to_owned(text),
            style: style.clone(),
        },
        Token::CodeBlock { text, language } => Token::CodeBlock {
            text: cowstr_to_owned(text),
            language: language.as_ref().map(cowstr_to_owned),
        },
        Token::Link {
            text,
            href,
            title,
            heading,
        } => Token::Link {
            text: cowstr_to_owned(text),
            href: cowstr_to_owned(href),
            title: title.as_ref().map(cowstr_to_owned),
            heading: *heading,
        },
        Token::ListMarker {
            marker,
            indent_level,
        } => Token::ListMarker {
            marker: cowstr_to_owned(marker),
            indent_level: *indent_level,
        },
        Token::Image { alt, url, title, link } => Token::Image {
            alt: cowstr_to_owned(alt),
            url: cowstr_to_owned(url),
            title: title.as_ref().map(cowstr_to_owned),
            link: link.as_ref().map(cowstr_to_owned),
        },
        Token::Table(data) => Token::Table(TableData {
            alignments: data.alignments.clone(),
            headers: data
                .headers
                .iter()
                .map(|cell| cell.iter().map(token_to_owned).collect())
                .collect(),
            rows: data
                .rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| cell.iter().map(token_to_owned).collect())
                        .collect()
                })
                .collect(),
        }),
        Token::HorizontalRule => Token::HorizontalRule,
        Token::BlockquoteStart => Token::BlockquoteStart,
        Token::BlockquoteEnd => Token::BlockquoteEnd,
        Token::TaskListMarker {
            checked,
            indent_level,
        } => Token::TaskListMarker {
            checked: *checked,
            indent_level: *indent_level,
        },
        Token::FootnoteRef { label } => Token::FootnoteRef {
            label: cowstr_to_owned(label),
        },
        Token::FootnoteDef { label } => Token::FootnoteDef {
            label: cowstr_to_owned(label),
        },
    }
}

fn cowstr_to_owned(s: &CowStr<'_>) -> CowStr<'static> {
    CowStr::Boxed(s.to_string().into_boxed_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// span 覆盖源文本 `needle` 的字节区间。
    fn covers(doc: &MarkdownDoc, span: &Range<usize>, needle: &str) -> bool {
        let Some(start) = doc.text.find(needle) else {
            return false;
        };
        let end = start + needle.len();
        span.start <= start && span.end >= end
    }

    /// wikilink:普通链接、`[[目标|显示名]]`、中文与带空格的目标。
    #[test]
    fn wikilinks_capture_target_label_and_span() {
        let text = "见 [[架构决策]] 与 [[Note One|笔记一]]、[[中文 文档]]。";
        let links = wikilinks(text);
        let targets: Vec<&str> = links.iter().map(|link| link.target.as_str()).collect();
        let labels: Vec<&str> = links.iter().map(|link| link.label.as_str()).collect();
        assert_eq!(targets, vec!["架构决策", "Note One", "中文 文档"]);
        assert_eq!(labels, vec!["架构决策", "笔记一", "中文 文档"]);
        for link in &links {
            assert!(
                text[link.span.start..link.span.end].starts_with("[["),
                "区间应对齐 [[…]]"
            );
            assert!(text[link.span.start..link.span.end].ends_with("]]"));
        }
    }

    /// 围栏代码块内的 `[[…]]` 不是链接(Rust 的 `arr[[0]]` 会被误伤)。
    #[test]
    fn wikilinks_skip_fenced_code_blocks() {
        let text = "正文 [[目标]]\n\n```rust\nlet v = arr[[0]];\n```\n\n尾 [[另一个]]\n";
        let links = wikilinks(text);
        let targets: Vec<&str> = links.iter().map(|link| link.target.as_str()).collect();
        assert_eq!(targets, vec!["目标", "另一个"]);
    }

    /// 展开:变成 `<wiki://目标>` 形式的链接,源码其它部分逐字保留。
    #[test]
    fn expand_wikilinks_rewrites_only_the_links() {
        let text = "见 [[架构决策]] 的下一节。";
        let expanded = expand_wikilinks(text);
        assert_eq!(expanded, "见 [架构决策](<wiki://架构决策>) 的下一节。");
        // 无链接时原样返回(不产生无谓拷贝路径上的差异)
        assert_eq!(expand_wikilinks("没有链接"), "没有链接");
        // 展开结果真能被解析成链接 token(否则拦截无从谈起)
        let doc = parse(&expanded);
        assert!(doc
            .tokens
            .iter()
            .any(|token| matches!(token, Token::Link { .. })));
    }

    /// LP2-4 偏移映射 · **无展开**:没有 wikilink 时表为恒等,`apply` 原样
    /// 返回,全部偏移双向换算幂等。
    #[test]
    fn offset_map_is_identity_without_rewrites() {
        let text = "# 标题\n\n正文没有链接。\n";
        let (rendered, map) = expand_wikilinks_with_map(text);
        assert_eq!(rendered, text, "无链接:渲染串即源码");
        assert!(map.is_identity());
        assert_eq!(map.apply(text), text);
        for offset in [0, 1, 7, text.len()] {
            assert_eq!(map.source_to_rendered(offset), offset);
            assert_eq!(map.rendered_to_source(offset), offset);
        }
        // 空表(无改写层的组合另一侧)同样恒等
        let empty = OffsetMap::empty();
        assert_eq!(empty.source_to_rendered(5), 5);
        assert_eq!(empty.rendered_to_source(5), 5);
    }

    /// LP2-4 偏移映射 · **wikilink 展开**:映射与展开输出同源 —— `apply`
    /// 重建的渲染串与旧 `expand_wikilinks` 逐字节一致;链接之后的源偏移平移
    /// 到渲染串里同一文本处,反向换算回到原处;段内偏移归段首;段首/段末
    /// 两个端点互为原像。
    #[test]
    fn offset_map_translates_through_wikilink_expansion() {
        let source = "见 [[架构决策]] 再谈。\n\n## 后续标题\n\n正文。\n";
        let (rendered, map) = expand_wikilinks_with_map(source);
        // 单一事实来源:渲染串由映射表自己拼出,与独立入口的展开一致
        assert_eq!(rendered, expand_wikilinks(source));

        let heading_src = source.find("## 后续标题").expect("heading in source");
        let heading_out = rendered.find("## 后续标题").expect("heading in rendered");
        assert_eq!(
            map.source_to_rendered(heading_src),
            heading_out,
            "链接之后的偏移按累积位移平移"
        );
        assert_eq!(
            map.rendered_to_source(heading_out),
            heading_src,
            "反向换算回到源码原处"
        );

        // 展开段内的偏移(源→渲染):归段首(改写段内没有更细的锚点)
        let link_start = source.find("[[").expect("wikilink");
        let link_end = source.find("]]").expect("wikilink end") + 2;
        assert_eq!(map.source_to_rendered(link_start + 3), link_start);
        // 渲染段内(渲染→源):映回源码段首
        let rendered_seg_start = rendered.find("[架构决策]").expect("expanded segment");
        assert_eq!(map.rendered_to_source(rendered_seg_start + 5), link_start);
        // 段首互为原像、段末按「区间外」平移互逆
        assert_eq!(map.source_to_rendered(link_start), rendered_seg_start);
        assert_eq!(
            map.rendered_to_source(map.source_to_rendered(link_end)),
            link_end
        );

        // 链接之前的偏移原样(位移尚未累积)
        assert_eq!(map.source_to_rendered(0), 0);
        assert_eq!(map.source_to_rendered(link_start), rendered_seg_start);
        assert_eq!(map.rendered_to_source(rendered_seg_start), link_start);
    }

    /// LP2-4 偏移映射 · **多链接**:每处 wikilink 各自贡献位移,第二个链接
    /// 之后的偏移按两条展开的累积差平移;两个展开段之间的文本仍原样。
    #[test]
    fn offset_map_handles_multiple_links() {
        let source = "首段 [[甲]] 中段文字。\n\n次段 [[乙]] 尾段。\n\n## 标题\n";
        let (rendered, map) = expand_wikilinks_with_map(source);
        assert_eq!(rendered, expand_wikilinks(source));

        // 两个展开段的渲染起点
        let first_out = rendered.find("[甲]").expect("first expanded");
        let second_out = rendered.find("[乙]").expect("second expanded");
        let first_src = source.find("[[甲]]").expect("first link");
        let second_src = source.find("[[乙]]").expect("second link");
        assert_eq!(map.source_to_rendered(first_src), first_out);
        assert_eq!(map.source_to_rendered(second_src), second_out);

        // 两段之间的文本(第一条展开已平移、第二条未):换算后仍在渲染串
        // 里同一文本处
        let between_src = source.find("中段文字").expect("text between links");
        let between_out = rendered.find("中段文字").expect("same text in rendered");
        assert_eq!(map.source_to_rendered(between_src), between_out);

        // 尾部标题:两条展开的累积位移
        let heading_src = source.find("## 标题").expect("heading in source");
        let heading_out = rendered.find("## 标题").expect("heading in rendered");
        assert_eq!(map.source_to_rendered(heading_src), heading_out);
        assert_eq!(map.rendered_to_source(heading_out), heading_src);
        assert_eq!(
            map.rendered_to_source(second_out + 1),
            second_src,
            "第二个渲染段内映回其源码段首"
        );
    }

    /// LP2-4 偏移映射 · **带显示名**:`[[目标|显示名]]` 展开段的长度由真实
    /// 替换串决定(label 与 target 不同长),映射按实际字节计,不存在
    /// 「公式推导长度」的近似路径;CJK 多字节目标同样按字节平移。
    #[test]
    fn offset_map_labeled_wikilink_uses_actual_replacement_length() {
        let source = "见 [[Note One|笔记]] 与 [[中文 文档]]。\n\n## 后\n";
        let (rendered, map) = expand_wikilinks_with_map(source);
        assert_eq!(rendered, expand_wikilinks(source));
        assert!(
            rendered
                .starts_with("见 [笔记](<wiki://Note One>) 与 [中文 文档](<wiki://中文 文档>)。"),
            "带显示名与带空格目标按真实替换串展开:{rendered}"
        );

        // 标题在两条展开之后:按两条真实替换串的累积长度差平移
        let heading_src = source.find("## 后").expect("heading in source");
        let heading_out = rendered.find("## 后").expect("heading in rendered");
        assert_eq!(map.source_to_rendered(heading_src), heading_out);
        assert_eq!(map.rendered_to_source(heading_out), heading_src);

        // 累积位移非零(两条展开都变长),且正反换算在文档尾端仍闭合
        assert_ne!(heading_src, heading_out, "展开确实改变了偏移");
        assert_eq!(
            map.source_to_rendered(source.len()),
            rendered.len(),
            "文末偏移平移到渲染串末尾"
        );
        assert_eq!(map.rendered_to_source(rendered.len()), source.len());
    }

    /// LP2-4 偏移映射 · **可组合口径**:两层改写各自一张表,第二层区间基于
    /// 第一层输出,消费点按顺序串行穿过 —— 与 `PreviewState::rendered` 的
    /// 生产链(wikilink 展开 → 相对图片 URI 改写)同构。这条用两张人造表
    /// 把口径本身钉死(emoji 短码 #48 落地时照此再串一张)。
    #[test]
    fn offset_map_composes_by_serial_pass_through() {
        // 第一层:wikilink 展开(真实生产者)
        let source = "见 [[架构决策]] 后续。\n\n## 标题\n";
        let (layer1_text, layer1) = expand_wikilinks_with_map(source);
        // 第二层:在第一层输出之上再改写一处(占位 emoji 短码形态)
        let needle = "后续";
        let needle_at = layer1_text.find(needle).expect("needle in layer1 text");
        let layer2 = OffsetMap::from_rewrites(vec![Rewrite {
            span: needle_at..needle_at + needle.len(),
            replacement: "\u{1f600}续".to_owned(),
        }]);
        let final_text = layer2.apply(&layer1_text);

        // 串行穿过:标题偏移先平移过 wikilink 展开,再平移过第二层改写
        let heading_src = source.find("## 标题").expect("heading");
        let after1 = layer1.source_to_rendered(heading_src);
        let after2 = layer2.source_to_rendered(after1);
        assert_eq!(
            final_text.find("## 标题"),
            Some(after2),
            "两层串行穿过的落点就是最终文本里的位置"
        );
        // 反向同理,两段逆穿回源码
        assert_eq!(
            layer1.rendered_to_source(layer2.rendered_to_source(after2)),
            heading_src
        );
        // 第二层区间内的偏移归段首(第一层终点即第二层段首)
        assert_eq!(
            layer2.source_to_rendered(needle_at + 2),
            needle_at,
            "第二层区间内归该层段首(第一层坐标)"
        );
    }

    // —— expand_emoji_links(#48 B1 预览 emoji 链接改写)——

    /// 测试用覆盖集。
    fn cover(glyphs: &[&'static str]) -> HashSet<&'static str> {
        glyphs.iter().copied().collect()
    }

    /// 渲染串里的全部 `(链接文本, href)`,按文档序(表格单元格递归)。
    fn link_pairs(doc: &MarkdownDoc) -> Vec<(String, String)> {
        fn collect(tokens: &[Token<'_>], out: &mut Vec<(String, String)>) {
            for token in tokens {
                match token {
                    Token::Link { text, href, .. } => {
                        out.push((text.to_string(), href.to_string()));
                    }
                    Token::Table(data) => {
                        for cell in &data.headers {
                            collect(cell, out);
                        }
                        for row in &data.rows {
                            for cell in row {
                                collect(cell, out);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        collect(&doc.tokens, &mut out);
        out
    }

    /// 正文/标题/列表/引用/表格单元里的 emoji 都改写成链接,且改写结果
    /// 真能被解析成 `emoji://` 链接(预览拦截的前提)。
    #[test]
    fn expand_emoji_links_rewrites_body_heading_list_quote() {
        let covered = cover(&["😀", "🚀"]);
        let text = "前 😀 中\n\n# 标题 🚀\n\n- 项 😀\n\n> 引 💡引用 😀\n\n| a 😀 | b |\n|---|---|\n| 🚀 | 2 |\n";
        let (rendered, map) = expand_emoji_links(text, &covered);
        assert_eq!(
            rendered,
            "前 [😀](<emoji://😀>) 中\n\n# 标题 [🚀](<emoji://🚀>)\n\n- 项 [😀](<emoji://😀>)\n\n> 引 💡引用 [😀](<emoji://😀>)\n\n| a [😀](<emoji://😀>) | b |\n|---|---|\n| [🚀](<emoji://🚀>) | 2 |\n"
        );
        assert_eq!(map.rendered_to_source(rendered.len()), text.len());
        assert_eq!(
            link_pairs(&parse(&rendered)),
            vec![
                ("😀".to_owned(), "emoji://😀".to_owned()),
                ("🚀".to_owned(), "emoji://🚀".to_owned()),
                ("😀".to_owned(), "emoji://😀".to_owned()),
                ("😀".to_owned(), "emoji://😀".to_owned()),
                ("😀".to_owned(), "emoji://😀".to_owned()),
                ("🚀".to_owned(), "emoji://🚀".to_owned()),
            ]
        );
    }

    /// 代码区豁免:``` 与 ~~~ 围栏(含 info string)、缩进代码块、行内代码
    /// 内的 emoji 一律不改写;围栏外的裸 emoji 照常改写。
    #[test]
    fn expand_emoji_links_skips_code_blocks_and_inline_code() {
        let covered = cover(&["😀"]);
        let text = "```rust\nlet s = \"😀\";\n```\n\n~~~\n~~~ 内 😀\n~~~\n\n    缩进 😀 代码\n\n行内 `code 😀` 后 😀\n";
        let (rendered, map) = expand_emoji_links(text, &covered);
        assert_eq!(
            rendered,
            "```rust\nlet s = \"😀\";\n```\n\n~~~\n~~~ 内 😀\n~~~\n\n    缩进 😀 代码\n\n行内 `code 😀` 后 [😀](<emoji://😀>)\n"
        );
        assert_eq!(map.rewrites_for_test().len(), 1);

        // emoji 只在代码区:首字符命中快路径、完整解析启动,但全部豁免 → 恒等
        let only_code = "```rust\nlet s = \"😀\";\n```\n";
        let (rendered, map) = expand_emoji_links(only_code, &covered);
        assert_eq!(rendered, only_code);
        assert!(map.is_identity());
    }

    /// 既有链接的文本与目标里的 emoji 不改写(嵌套链接不是合法
    /// CommonMark);链接前后的裸 emoji 照常改写。
    #[test]
    fn expand_emoji_links_skips_existing_link_text_and_destinations() {
        let covered = cover(&["😀", "🌞"]);
        let text = "首 😀 与 [链 😀 接](https://e.com/😀) 与 ![图 🌞](x.png) 尾 😀\n";
        let (rendered, _) = expand_emoji_links(text, &covered);
        assert_eq!(
            rendered,
            "首 [😀](<emoji://😀>) 与 [链 😀 接](https://e.com/😀) 与 ![图 🌞](x.png) 尾 [😀](<emoji://😀>)\n"
        );
    }

    /// 与 wikilink 层叠加(wikilink 先、emoji 后):展开出的链接文本/目标
    /// 整体豁免不拆坏;两层偏移映射串行穿过落在渲染串同一文本处。
    #[test]
    fn expand_emoji_links_stacks_after_wikilink_expansion() {
        let covered = cover(&["😀"]);
        let source = "见 [[目标😀]] 与 😀\n\n## 标题\n";
        let (layer1, map1) = expand_wikilinks_with_map(source);
        let (rendered, map2) = expand_emoji_links(&layer1, &covered);
        assert_eq!(
            rendered,
            "见 [目标😀](<wiki://目标😀>) 与 [😀](<emoji://😀>)\n\n## 标题\n"
        );

        let heading_src = source.find("## 标题").expect("heading in source");
        let after1 = map1.source_to_rendered(heading_src);
        let heading_out = rendered.find("## 标题").expect("heading in rendered");
        assert_eq!(map2.source_to_rendered(after1), heading_out);
        assert_eq!(
            map1.rendered_to_source(map2.rendered_to_source(heading_out)),
            heading_src,
            "两层逆穿回源码原处"
        );
    }

    /// 恒等路径:无覆盖枚首字符的文档、覆盖集之外的 emoji、空覆盖集,
    /// 一律原样返回且映射为恒等(快路径不启动解析)。
    #[test]
    fn expand_emoji_links_identity_without_covered_hits() {
        let covered = cover(&["😀"]);
        for text in ["没有 emoji 的普通文档\n\n## 标题\n", "覆盖集外的 🥶 直显\n"]
        {
            let (rendered, map) = expand_emoji_links(text, &covered);
            assert_eq!(rendered, text);
            assert!(map.is_identity());
        }
        let (rendered, map) = expand_emoji_links("文档 😀", &cover(&[]));
        assert_eq!(rendered, "文档 😀");
        assert!(map.is_identity());
    }

    /// CJK 混排边界:改写区间都落在字符边界上;连续 emoji 逐枚改写;
    /// 两字符枚(旗帜、带 FE0F)长优先匹配,裸形(无 FE0F)不吃错。
    #[test]
    fn expand_emoji_links_cjk_boundaries_and_multichar_glyphs() {
        let covered = cover(&["😀", "🇨🇳", "✌️"]);
        let text = "中😀文😀😀两枚🇨🇳与✌️收尾";
        let (rendered, map) = expand_emoji_links(text, &covered);
        assert_eq!(
            rendered,
            "中[😀](<emoji://😀>)文[😀](<emoji://😀>)[😀](<emoji://😀>)两枚[🇨🇳](<emoji://🇨🇳>)与[✌️](<emoji://✌️>)收尾"
        );
        for rewrite in map.rewrites_for_test() {
            assert!(text.is_char_boundary(rewrite.span.start));
            assert!(text.is_char_boundary(rewrite.span.end));
        }

        // 裸 ✌(无 FE0F)不在覆盖集,不改写
        let (rendered, _) = expand_emoji_links("裸 ✌ 不改写", &covered);
        assert_eq!(rendered, "裸 ✌ 不改写");
    }

    /// HTML 与脚注豁免:HTML 块、脚注引用 `[^😀]`、脚注定义(含标签行)
    /// 全部原样;普通段落照常改写。
    #[test]
    fn expand_emoji_links_skips_html_and_footnotes() {
        let covered = cover(&["😀"]);
        let text = "<div>\n块 😀\n</div>\n\n引用[^😀]。\n\n[^😀]: 定义 😀\n\n尾 😀\n";
        let (rendered, map) = expand_emoji_links(text, &covered);
        assert_eq!(
            rendered,
            "<div>\n块 😀\n</div>\n\n引用[^😀]。\n\n[^😀]: 定义 😀\n\n尾 [😀](<emoji://😀>)\n"
        );
        assert_eq!(map.rewrites_for_test().len(), 1);
    }

    /// 幂等:自己产出的 `[😀](<emoji://😀>)` 是链接构造,二次穿过整体
    /// 豁免,不再改写(流式重建反复穿过同层的稳态保证)。
    #[test]
    fn expand_emoji_links_is_idempotent_on_own_output() {
        let covered = cover(&["😀", "🚀"]);
        let (once, _) = expand_emoji_links("a 😀 b 🚀", &covered);
        let (twice, map) = expand_emoji_links(&once, &covered);
        assert_eq!(twice, once);
        assert!(map.is_identity());
    }

    /// 偏移映射:emoji 层单独一张表 —— 改写段内归段首、段外平移、文末
    /// 闭合(与 wikilink 层同语义,消费点串行穿过)。
    #[test]
    fn expand_emoji_links_map_translates_offsets() {
        let covered = cover(&["😀"]);
        let source = "前 😀 中\n\n## 标题\n";
        let (rendered, map) = expand_emoji_links(source, &covered);
        let heading_src = source.find("## 标题").expect("heading");
        assert_eq!(
            map.source_to_rendered(heading_src),
            rendered.find("## 标题").expect("heading in rendered")
        );
        assert_eq!(map.rendered_to_source(rendered.len()), source.len());
        let emoji_at = source.find("😀").expect("emoji");
        assert_eq!(map.source_to_rendered(emoji_at + 1), emoji_at, "段内归段首");
        assert_eq!(map.source_to_rendered(0), 0, "段前原样");
    }

    // —— expand_task_links(#63 任务列表 checkbox 链接改写)——

    /// 任务标记逐个改写成 `task://` 链接:占位、勾选态、载荷偏移齐全,且
    /// 改写结果真能被解析成链接(预览拦截的前提);无序 `-`、有序 `1.`、
    /// 嵌套缩进、`*`/`+` 前缀、引用内任务全认(pulldown 事件口径)。
    #[test]
    fn expand_task_links_rewrites_markers_into_links() {
        let text = "- [ ] 待办\n- [x] 已办\n\n1. [ ] 有序\n\n  * [X] 大写\n\n> + [x] 引用内\n";
        let (rendered, map) = expand_task_links(text);
        assert_eq!(
            rendered,
            "- [☐ ](<task://u2>) 待办\n- [☐ ](<task://c15>) 已办\n\n1. [☐ ](<task://u30>) 有序\n\n  * [☐ ](<task://c46>) 大写\n\n> + [☐ ](<task://c62>) 引用内\n"
        );
        assert_eq!(map.rewrites_for_test().len(), 5);
        // 改写产物再解析:五个链接,不再有 TaskListMarker token。
        let doc = parse(&rendered);
        assert!(doc
            .tokens
            .iter()
            .all(|token| !matches!(token, Token::TaskListMarker { .. })));
        let hrefs: Vec<&str> = doc
            .tokens
            .iter()
            .filter_map(|token| match token {
                Token::Link { href, .. } => Some(href.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(
            hrefs,
            vec![
                "task://u2",
                "task://c15",
                "task://u30",
                "task://c46",
                "task://c62"
            ]
        );
    }

    /// 判定即豁免(#10 同款否决线):代码块/行内代码/普通文本里的 `[ ]`
    /// 不是任务标记,一律不改写;标记后无空格的形态(pulldown 不产出
    /// 事件)同样不改写。
    #[test]
    fn expand_task_links_skips_non_task_brackets() {
        for text in [
            "```rust\nlet a = [ ];\nlet b = [x];\n```\n",
            "行内 `[ ]` 与 `[x]` 代码\n",
            "普通段落的 [ ] 方括号 与数组 arr[0]\n",
            "- [ ]无空格不是任务(pulldown 口径)\n- [] 也不是\n",
        ] {
            let (rendered, map) = expand_task_links(text);
            assert_eq!(rendered, text, "非任务形态不得改写: {text:?}");
            assert!(map.is_identity());
        }
    }

    /// 空任务(纯 `- [ ]` 无文字)、CJK 与 emoji 混排任务文本都改写,
    /// 载荷偏移按字节计(多字节字符之后的任务不漂移)。
    #[test]
    fn expand_task_links_handles_empty_cjk_and_emoji_tasks() {
        let text = "- [ ]\n\n中文 😀 任务\n\n- [ ] emoji 😀 任务\n- [x] 完成\n";
        let (rendered, map) = expand_task_links(text);
        assert_eq!(
            rendered,
            "- [☐ ](<task://u2>)\n\n中文 😀 任务\n\n- [☐ ](<task://u29>) emoji 😀 任务\n- [☐ ](<task://c53>) 完成\n"
        );
        // 偏移落在字符边界上(改写区间是 pulldown 事件的 range)。
        for rewrite in map.rewrites_for_test() {
            assert!(text.is_char_boundary(rewrite.span.start));
            assert!(text.is_char_boundary(rewrite.span.end));
        }
        assert_eq!(text.as_bytes()[29], b'[');
        assert_eq!(text.as_bytes()[53], b'[');
    }

    /// 与前两层叠加(顺序 wikilink → emoji → task):改写段互不拆坏,三层
    /// 逆穿回源码原处;正向串穿落在渲染串同一文本处(大纲跳转口径)。
    #[test]
    fn expand_task_links_stacks_after_wikilink_and_emoji_layers() {
        let covered = cover(&["😀"]);
        let source = "见 [[目标]] 与 😀\n\n- [ ] 任务\n\n## 标题\n";
        let (layer1, map1) = expand_wikilinks_with_map(source);
        let (layer2, map2) = expand_emoji_links(&layer1, &covered);
        let (rendered, map3) = expand_task_links(&layer2);
        assert_eq!(
            rendered,
            "见 [目标](<wiki://目标>) 与 [😀](<emoji://😀>)\n\n- [☐ ](<task://u60>) 任务\n\n## 标题\n"
        );

        // 正向:源码的标题偏移 → 三层串穿 → 渲染串的标题偏移。
        let heading_src = source.find("## 标题").expect("heading in source");
        let heading_out = rendered.find("## 标题").expect("heading in rendered");
        let after1 = map1.source_to_rendered(heading_src);
        let after2 = map2.source_to_rendered(after1);
        assert_eq!(map3.source_to_rendered(after2), heading_out);
        // 逆向:任务载荷偏移(u60,layer2 坐标)→ 两层逆穿 → 源码的 `[`。
        let task_src = source.find("[ ]").expect("task marker in source");
        assert_eq!(
            map1.rendered_to_source(map2.rendered_to_source(60)),
            task_src,
            "任务载荷两层逆穿回源码原处"
        );
    }

    /// 恒等路径:无 `[` 文档快路径直接返回;含 `[` 但全是非任务形态的文档
    /// 解析后零改写(非任务文档零改动的否决线)。
    #[test]
    fn expand_task_links_identity_on_non_task_documents() {
        let (rendered, map) = expand_task_links("# 标题\n\n正文段落,没有方括号\n");
        assert_eq!(rendered, "# 标题\n\n正文段落,没有方括号\n");
        assert!(map.is_identity());
        let (rendered, map) = expand_task_links("链接 [文字](https://a.com) 与图片 ![x](y.png)\n");
        assert_eq!(rendered, "链接 [文字](https://a.com) 与图片 ![x](y.png)\n");
        assert!(map.is_identity());
    }

    /// 幂等:自己产出的 `[☐ ](<task://…>)` 是链接构造,二次穿过时该处
    /// 不再是任务标记,不再改写(快路径含 `[` 会启动解析,但零命中)。
    #[test]
    fn expand_task_links_is_idempotent_on_own_output() {
        let (_, map) = expand_task_links("- [ ] 一次\n");
        let once = map.apply("- [ ] 一次\n");
        let (twice, map2) = expand_task_links(&once);
        assert_eq!(twice, once);
        assert!(map2.is_identity());
    }

    /// 回写侧核验:真任务的 `[` 命中;行内代码/普通文本的 `[ ]`、标记的
    /// 第二字符、越界与多字节中间一律不命中(过期偏移闸门)。
    #[test]
    fn is_task_marker_start_matches_only_real_markers() {
        let text = "普通 [ ] 括号\n\n- [ ] 任务\n\n```rust\nlet a = [ ];\n```\n";
        let real = text.find("[ ] 任务").expect("real task");
        assert!(is_task_marker_start(text, real));
        let plain = text.find("[ ]").expect("first bracket pair");
        assert_ne!(plain, real);
        assert!(!is_task_marker_start(text, plain), "普通文本的 [ ] 不命中");
        let code = text
            .find("let a = [ ]")
            .map(|at| at + "let a = ".len())
            .expect("code");
        assert!(!is_task_marker_start(text, code), "代码块内的 [ ] 不命中");
        assert!(!is_task_marker_start(text, real + 1), "标记内部不命中");
        assert!(!is_task_marker_start(text, usize::MAX));
        let cjk = "中文段落\n\n- [ ] 任务\n";
        let mid = cjk.find("文").map(|at| at + 1).expect("cjk mid");
        assert!(!is_task_marker_start(cjk, mid), "多字节字符中间不命中");
        assert!(is_task_marker_start(cjk, cjk.find("[ ]").expect("task")));
        assert!(!is_task_marker_start("无方括号文档", 0), "快路径不命中");
    }

    // —— highlight_spans / expand_highlight_links(#65 高亮扩展语法)——

    /// 期望表断言:(区间字面, 内部文本) 双重核对 —— span 切片与 inner 都
    /// 必须对上,字节手算错当场失败。
    fn assert_highlights(text: &str, spans: &[HighlightSpan], expected: &[(&str, &str)]) {
        assert_eq!(
            spans.len(),
            expected.len(),
            "对数不符:{spans:?} vs {expected:?}"
        );
        for (span, (whole, inner)) in spans.iter().zip(expected) {
            assert_eq!(&text[span.span.clone()], *whole);
            assert_eq!(span.inner, *inner);
        }
    }

    /// 成对 + 多个同段 + CJK/emoji 混排:区间落在字符边界上,`==` 不吃
    /// 多字节字符的尾巴。
    #[test]
    fn highlight_spans_pairs_cjk_emoji_and_multiple() {
        let text = "中文==高亮==与==重点 😀==收尾,再来 ==两段== 同段 ==并排==。";
        let spans = highlight_spans(text);
        assert_highlights(
            text,
            &spans,
            &[
                ("==高亮==", "高亮"),
                ("==重点 😀==", "重点 😀"),
                ("==两段==", "两段"),
                ("==并排==", "并排"),
            ],
        );
        for span in &spans {
            assert!(text.is_char_boundary(span.span.start));
            assert!(text.is_char_boundary(span.span.end));
        }
    }

    /// 未闭合、空标记、贴不上字(空白邻接):一律不成对,标记按字面。
    #[test]
    fn highlight_spans_skip_unclosed_empty_and_blank() {
        for text in [
            "==未闭合到底",
            "开头就收 x==",
            "====",
            "== ==",
            "==\t==",
            "==x ==", // 闭标前驱是空白,贴不上字
            "x == ",  // 开标后随是空白
            "",
        ] {
            assert!(highlight_spans(text).is_empty(), "{text:?} 不应产出高亮");
        }
    }

    /// 嵌套口径 = 最左最近配对、首个合法闭标即定终:`==a ==b== c==` 只高亮
    /// `b`(外层开标的首个合法闭标已定终,作废后闭标不回收);单词内的
    /// `==` 照常配对。
    #[test]
    fn highlight_spans_leftmost_pairing_not_nested() {
        let text = "==a ==b== c==";
        assert_highlights(text, &highlight_spans(text), &[("==b==", "b")]);

        let text = "x==y==z";
        assert_highlights(text, &highlight_spans(text), &[("==y==", "y")]);
    }

    /// 跨行与方括号:内部含换行、`[` 或 `]` 整对作废(wikilink 未展开形态
    /// `==[[目标]]==` 同样作废);作废后的后文照常参与配对。
    #[test]
    fn highlight_spans_reject_cross_line_and_brackets() {
        for text in ["==跨\n行==", "==a [b]==", "==a ] b==", "==[[目标]]=="] {
            assert!(highlight_spans(text).is_empty(), "{text:?} 整对作废");
        }
        let text = "==[x](y)== 与 ==好== 混排";
        assert_highlights(text, &highlight_spans(text), &[("==好==", "好")]);
        let text = "==a ] b== 后 ==好==";
        assert_highlights(text, &highlight_spans(text), &[("==好==", "好")]);
    }

    /// 代码区豁免且不误伤:围栏/缩进代码块里的 `==` 原样,行内代码夹心
    /// 整对作废,块外的 `==` 照常配对。
    #[test]
    fn highlight_spans_skip_code_blocks_and_inline_code() {
        let text = "```rust\nlet a == b; // ==x==\n```\n\n尾 ==真高亮==\n";
        assert_highlights(text, &highlight_spans(text), &[("==真高亮==", "真高亮")]);

        let text = "    缩进 ==x== 代码\n\n尾 ==真高亮==\n";
        assert_highlights(text, &highlight_spans(text), &[("==真高亮==", "真高亮")]);

        // 行内代码整体是一条豁免区间:覆盖它的候选作废,圈外的不受牵连
        assert!(highlight_spans("`==x==`").is_empty(), "行内代码内不成对");
        let text = "==a `b` c== 与 ==真高亮==";
        assert_highlights(text, &highlight_spans(text), &[("==真高亮==", "真高亮")]);
        let text = "前 ==真高亮== 与 `==x==` 后";
        assert_highlights(text, &highlight_spans(text), &[("==真高亮==", "真高亮")]);
    }

    /// 链接/图片/强调/HTML/脚注豁免:候选区间与这些构造重叠即整对作废
    /// (渠道是链接文字,vendored 对链接文字只取最后一个 Text 事件,混入
    /// 富构造会丢字);圈外的 `==真高亮==` 不受牵连。
    #[test]
    fn highlight_spans_skip_links_emphasis_html_and_footnotes() {
        for text in [
            "[==x==](u) 与 ==真高亮==",
            "![alt ==x==](i.png) 与 ==真高亮==",
            "==**b**== 与 ==真高亮==",
            "==a <b>c== 与 ==真高亮==",
            "引用[^1]。\n\n[^1]: 定义 ==x== 这里\n\n尾 ==真高亮==\n",
            "<div>\n==x==\n</div>\n\n尾 ==真高亮==\n",
        ] {
            let spans = highlight_spans(text);
            assert_eq!(spans.len(), 1, "{text:?}");
            assert_eq!(spans[0].inner, "真高亮");
        }
    }

    /// 展开:变成 `[高亮](<hl://>)` 链接,其余字节逐字保留;改写产物真能
    /// 解析成 `hl://` 链接(预览拦截的前提)。
    #[test]
    fn expand_highlight_links_rewrites_into_links() {
        let text = "重点 ==高亮内容== 收尾。";
        let (rendered, map) = expand_highlight_links(text);
        assert_eq!(rendered, "重点 [高亮内容](<hl://>) 收尾。");
        assert_eq!(map.rewrites_for_test().len(), 1);
        let doc = parse(&rendered);
        let hrefs: Vec<&str> = doc
            .tokens
            .iter()
            .filter_map(|token| match token {
                Token::Link { href, .. } => Some(href.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(hrefs, vec![HIGHLIGHT_SCHEME]);

        // 无 == 文档:恒等返回,映射为空(快路径不启动解析)
        let (rendered, map) = expand_highlight_links("# 没有高亮\n\n正文。\n");
        assert_eq!(rendered, "# 没有高亮\n\n正文。\n");
        assert!(map.is_identity());
    }

    /// 偏移映射:改写段内归段首、段外平移、文末闭合(与既有层同语义)。
    #[test]
    fn expand_highlight_links_map_translates_offsets() {
        let source = "前 ==高亮== 中\n\n## 标题\n";
        let (rendered, map) = expand_highlight_links(source);
        let heading_src = source.find("## 标题").expect("heading");
        assert_eq!(
            map.source_to_rendered(heading_src),
            rendered.find("## 标题").expect("heading in rendered")
        );
        assert_eq!(map.rendered_to_source(rendered.len()), source.len());
        let mark_at = source.find("==高亮==").expect("mark");
        assert_eq!(map.source_to_rendered(mark_at + 3), mark_at, "段内归段首");
        assert_eq!(map.rendered_to_source(map.source_to_rendered(0)), 0);
    }

    /// 四层生产顺序(wikilink → highlight → emoji → task)串行叠加:高亮
    /// 保住、wikilink 照常展开、`==😀==` 里的 emoji 以字形直显不被二次
    /// 改写;标题偏移四层正穿落在渲染串同一文本处、逆穿回源码。
    #[test]
    fn expand_highlight_links_stacks_in_production_order() {
        let covered = cover(&["😀"]);
        let source = "==高亮😀== 与 [[目标]]\n\n## 标题\n";
        let (layer1, map1) = expand_wikilinks_with_map(source);
        let (layer2, map2) = expand_highlight_links(&layer1);
        let (layer3, map3) = expand_emoji_links(&layer2, &covered);
        let (rendered, map4) = expand_task_links(&layer3);
        assert_eq!(
            rendered, "[高亮😀](<hl://>) 与 [目标](<wiki://目标>)\n\n## 标题\n",
            "emoji 层对 hl:// 链接文本整体豁免,😀 不再改成 emoji://"
        );

        let heading_src = source.find("## 标题").expect("heading in source");
        let heading_out = rendered.find("## 标题").expect("heading in rendered");
        let after1 = map1.source_to_rendered(heading_src);
        let after2 = map2.source_to_rendered(after1);
        let after3 = map3.source_to_rendered(after2);
        assert_eq!(map4.source_to_rendered(after3), heading_out);
        assert_eq!(
            map1.rendered_to_source(
                map2.rendered_to_source(
                    map3.rendered_to_source(map4.rendered_to_source(heading_out))
                )
            ),
            heading_src,
            "四层逆穿回源码原处"
        );
    }

    /// 幂等:自己产出的 `[x](<hl://>)` 不含 `==`,二次穿过恒等(快路径
    /// 直接返回,流式重建反复穿过同层的稳态保证)。
    #[test]
    fn expand_highlight_links_is_idempotent_on_own_output() {
        let (once, _) = expand_highlight_links("a ==x== b ==y==");
        let (twice, map) = expand_highlight_links(&once);
        assert_eq!(twice, once);
        assert!(map.is_identity());
    }

    /// 块划分的自检:区间首尾相接、并集等于整篇 —— Live Preview 下光标块
    /// 是被编辑的区间,任何落在块外的字节都会在敲键时静默丢失。
    fn assert_covers_text(text: &str, blocks: &[Range<usize>]) {
        let mut cursor = 0;
        for block in blocks {
            assert_eq!(block.start, cursor, "块区间不连续: {blocks:?}");
            assert!(block.end > block.start, "空块区间: {block:?}");
            cursor = block.end;
        }
        assert_eq!(cursor, text.len(), "未覆盖到文末: {blocks:?}");
    }

    /// 段落按空行切分,块间空行归前一块(每块可独立解析)。
    #[test]
    fn blocks_split_paragraphs_and_own_trailing_blank_line() {
        let text = "# 标题\n\n正文一段\n\n正文二段\n";
        let blocks = blocks(text);
        let slices: Vec<&str> = blocks.iter().map(|b| &text[b.start..b.end]).collect();
        assert_eq!(slices, vec!["# 标题\n\n", "正文一段\n\n", "正文二段\n"]);
        assert_covers_text(text, &blocks);
    }

    /// 代码块整块成一段:**含闭合 fence**(vendored 的 span 只到内容末尾,
    /// 直接拿 span 当边界会把 ``` 切到下一块)。
    #[test]
    fn blocks_keep_fenced_code_block_intact() {
        let text = "前言\n\n```rust\nfn main() {}\n```\n\n后记\n";
        let blocks = blocks(text);
        let slices: Vec<&str> = blocks.iter().map(|b| &text[b.start..b.end]).collect();
        assert_eq!(
            slices,
            vec!["前言\n\n", "```rust\nfn main() {}\n```\n\n", "后记\n"]
        );
        assert_covers_text(text, &blocks);
    }

    /// 多行引用必须作为一个完整 Live 块,不能按引用内部的段落/换行
    /// 拆散;点击任意引用行都应在同一活动块内编辑。
    #[test]
    fn blocks_keep_multiline_blockquote_together() {
        let text = "> 第一行引用\n> 第二行引用\n> 第三行引用\n\n尾段\n";
        let ranges = blocks(text);
        let slices: Vec<&str> = ranges.iter().map(|range| &text[range.clone()]).collect();
        assert_eq!(
            slices,
            vec!["> 第一行引用\n> 第二行引用\n> 第三行引用\n\n", "尾段\n"]
        );
        assert_covers_text(text, &ranges);
    }

    /// 表格与列表:表格整块;列表**每项**一块(每项都可单独进 Live 编辑)。
    #[test]
    fn blocks_cover_tables_and_list_items() {
        let text = "| a | b |\n|---|---|\n| 1 | 2 |\n\n尾段\n";
        let table_blocks = blocks(text);
        assert_eq!(table_blocks.len(), 2, "{table_blocks:?}");
        assert!(text[table_blocks[0].start..table_blocks[0].end].starts_with("| a |"));
        assert_covers_text(text, &table_blocks);

        let list = "- 一\n- 二\n\n尾\n";
        let list_blocks = blocks(list);
        let slices: Vec<&str> = list_blocks.iter().map(|b| &list[b.start..b.end]).collect();
        assert_eq!(slices, vec!["- 一\n", "- 二\n\n", "尾\n"]);
        assert_covers_text(list, &list_blocks);
    }

    /// 行内/标题标记归属回归(2026-10-10 坤哥两个真机症状):
    /// 「**粗体**、…」段的 `**` 与「## 常用元素」的 `##` 必须在**自己的
    /// 块**里。修复前 vendored span 不含 Start/End 内联事件的字节,孤儿
    /// 标记被连续化整段归给**前一块** —— Live 富渲染丢开标记(字面
    /// `**`)、点标题进编辑丢 `#`(在上一块尾渲染成字面 `##`,即
    /// 「# 号在上面,不在当前的框里面」)。默认示例文档即复现。
    #[test]
    fn blocks_keep_inline_and_heading_markers_in_their_own_block() {
        let text = "## 常用元素\n\n- 列表 item\n\n**粗体**、*斜体*。\n";
        let blocks = blocks(text);
        let slices: Vec<&str> = blocks.iter().map(|b| &text[b.start..b.end]).collect();
        assert_eq!(
            slices,
            vec!["## 常用元素\n\n", "- 列表 item\n\n", "**粗体**、*斜体*。\n"],
            "标记必须随自己的块"
        );
        assert_covers_text(text, &blocks);
    }

    #[test]
    fn blocks_keep_indented_code_syntax() {
        for code in ["    x\n    y\n\n", "\tx\n\ty\n\n"] {
            let text = format!("intro\n\n{code}tail\n");
            let ranges = blocks(&text);
            let slices: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
            assert_eq!(slices, vec!["intro\n\n", code, "tail\n"]);
            assert_covers_text(&text, &ranges);
            let parsed = egui_markdown::parser::parse(slices[1]);
            assert!(parsed
                .tokens
                .iter()
                .any(|t| matches!(t, Token::CodeBlock { .. })));
        }
    }

    #[test]
    fn blocks_keep_footnote_definitions_atomic() {
        for definition in [
            "[^1]: **bold**",
            "[^1]: a\n\n    **last**",
            "[^1]: first\n\n    - a\n      - nested\n    - b",
        ] {
            for suffix in ["", "\n\ntail\n"] {
                let text = format!("intro\n\n{definition}{suffix}");
                let ranges = blocks(&text);
                let slices: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
                let complete = if suffix.is_empty() {
                    definition.to_owned()
                } else {
                    format!("{definition}\n\n")
                };
                let mut expected = vec!["intro\n\n", complete.as_str()];
                if !suffix.is_empty() {
                    expected.push("tail\n");
                }
                assert_eq!(slices, expected);
                assert_covers_text(&text, &ranges);
            }
        }
    }

    #[test]
    fn blocks_keep_styled_heading_and_nested_containers() {
        for construct in [
            "## **bold** and `code` [link](https://example.com)\n\n",
            "> quote\n>\n> - **bold**\n>   - nested\n\n",
            "- outer\n  - **nested**\n\n",
        ] {
            let text = format!("intro\n\n{construct}tail\n");
            let ranges = blocks(&text);
            let slices: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
            assert_eq!(slices, vec!["intro\n\n", construct, "tail\n"]);
            assert_covers_text(&text, &ranges);
        }
    }

    /// 边界:空文档无块;纯空行整篇一块(编辑需要落点);单段无换行也成块。
    #[test]
    fn blocks_handle_empty_and_degenerate_input() {
        assert!(blocks("").is_empty());
        let blank = "\n\n\n";
        assert_eq!(blocks(blank), vec![0..blank.len()]);
        let single = "hello world";
        assert_eq!(blocks(single), vec![0..single.len()]);
    }

    /// CRLF 与中文:块的字节边界仍连续覆盖(CRLF 的 `\r` 属于行内容)。
    #[test]
    fn blocks_cover_crlf_and_cjk_text() {
        let text = "中文标题\r\n\r\n正文内容\r\n";
        let blocks = blocks(text);
        assert_covers_text(text, &blocks);
        assert!(!blocks.is_empty());
    }

    #[test]
    fn outline_levels_h1_to_h6() {
        let src = "# one\n\n## two\n\n### three\n\n#### four\n\n##### five\n\n###### six";
        let doc = parse(src);
        let outline = doc.outline();
        let levels: Vec<u8> = outline.iter().map(|item| item.level).collect();
        assert_eq!(levels, vec![1, 2, 3, 4, 5, 6]);
        let texts: Vec<&str> = outline.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(texts, vec!["one", "two", "three", "four", "five", "six"]);
        for item in &outline {
            assert!(
                covers(
                    &doc,
                    &item.span,
                    &format!("{} {}", "#".repeat(item.level as usize), item.text)
                ),
                "span {:?} misses heading {:?}",
                item.span,
                item.text
            );
        }
    }

    #[test]
    fn outline_merges_inline_code_in_heading() {
        let src = "# 标题含 `code` 内联\n\n正文 `x` 段落。\n\n## 下一个\n";
        let items = outline(src);
        // 一条标题,段落里的行内代码不被误吸
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].level, 1);
        assert_eq!(items[0].text, "标题含 code 内联");
        assert_eq!(items[1].level, 2);
        assert_eq!(items[1].text, "下一个");
        // span 覆盖标题行原文(spans 连续铺盖,与单 token 标题同口径:
        // 起点含 `# ` 标记),点击跳转与节定位用
        assert_eq!(&src[items[0].span.clone()], "# 标题含 `code` 内联");
    }

    #[test]
    fn outline_heading_leading_inline_code() {
        let src = "# `init` 脚本说明\n";
        let items = outline(src);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "init 脚本说明");
        assert_eq!(&src[items[0].span.clone()], "# `init` 脚本说明");
    }

    #[test]
    fn outline_heading_link_text_joined() {
        let src = "# [甲](https://example.com) 乙\n\n正文\n";
        let items = outline(src);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "甲 乙");
    }

    #[test]
    fn outline_method_matches_free_fn_with_inline_code() {
        let src = "# 标题含 `code` 内联\n\n## `init` 脚本\n";
        assert_eq!(parse(src).outline(), outline(src));
    }

    #[test]
    fn outline_chinese_headings() {
        let src = "# 架构设计\n\n正文段落。\n\n## 中文二级标题\n";
        let doc = parse(src);
        let outline = doc.outline();
        assert_eq!(outline.len(), 2);
        assert_eq!(
            (outline[0].level, outline[0].text.as_str()),
            (1, "架构设计")
        );
        assert_eq!(
            (outline[1].level, outline[1].text.as_str()),
            (2, "中文二级标题")
        );
        assert!(covers(&doc, &outline[0].span, "# 架构设计"));
        assert!(covers(&doc, &outline[1].span, "## 中文二级标题"));
    }

    #[test]
    fn outline_empty_without_headings() {
        let src = "只有段落和 **粗体**。\n\n- 列表项\n\n```rust\nfn main() {}\n```\n\n> 引用\n";
        let doc = parse(src);
        assert!(!doc.tokens.is_empty());
        assert!(doc.outline().is_empty());
        assert!(outline(src).is_empty());
    }

    /// 借用型快路径与拥有型方法形态产出一致,两处实现不得漂移。
    #[test]
    fn borrowed_outline_matches_doc_outline() {
        let src = "# 甲\n\n## 乙 *强调*\n\n正文 ### 非标题\n";
        assert_eq!(outline(src), parse(src).outline());
    }

    /// 节定位(AI 摘要旧节清理的依据,边界行为经探针实测):起点吸收标题
    /// 前的空行,终点到下一同级/更浅标题前 —— 删除区间后,正文与下一个
    /// 标题之间仍留有换行,标题保持合法。
    #[test]
    fn heading_section_span_bounds_middle_section() {
        let src = "# 甲\n\n正文甲。\n\n## AI 摘要\n\n> - 要点\n\n## 乙\n\n正文乙。\n";
        let span = heading_section_span(src, 2, "AI 摘要").expect("定位到摘要节");
        assert_eq!(&src[span.clone()], "\n\n## AI 摘要\n\n> - 要点\n");
        let remainder = format!("{}{}", &src[..span.start], &src[span.end..]);
        assert_eq!(remainder, "# 甲\n\n正文甲。\n## 乙\n\n正文乙。\n");
        // 删除后的文本里旧节彻底消失,其余标题原样
        assert!(!remainder.contains("AI 摘要"));
        assert_eq!(outline(&remainder).len(), 2);
    }

    /// 更深层级的标题(`###`)是节的子内容,一并属于节;节在文末时区间
    /// 到文末,起点同样吸收前导空行。
    #[test]
    fn heading_section_span_swallows_subheadings_and_tail() {
        // ### 子节归入 ## AI 摘要节
        let src = "# 甲\n\n## AI 摘要\n\n### 子节\n\n内容\n\n## 乙\n\n正文乙\n";
        let span = heading_section_span(src, 2, "AI 摘要").expect("定位到摘要节");
        assert_eq!(&src[span.clone()], "\n\n## AI 摘要\n\n### 子节\n\n内容");

        // 文末节:删完只剩前文,正文结尾不带多余换行
        let src = "正文\n\n## AI 摘要\n\n> - 只有一条要点";
        let span = heading_section_span(src, 2, "AI 摘要").expect("定位到摘要节");
        assert_eq!(&src[span.clone()], "\n\n## AI 摘要\n\n> - 只有一条要点");
        assert_eq!(&src[..span.start], "正文");
    }

    /// 没有目标标题、标题文本不精确匹配或层级不符时返回 `None`
    /// (调用方据此跳过移除,直接追加新节)。
    #[test]
    fn heading_section_span_absent_or_mismatched_is_none() {
        assert_eq!(heading_section_span("# 甲\n\n正文\n", 2, "AI 摘要"), None);
        // 文本变体不算同一节:保守匹配,避免误删用户手写内容
        let src = "# 甲\n\n## AI 摘要(旧)\n\n> - 要点\n";
        assert_eq!(heading_section_span(src, 2, "AI 摘要"), None);
        // 层级不同不算:用户改写成 # / ### 后的旧节不在此口径内
        let src = "# AI 摘要\n\n> - 要点\n";
        assert_eq!(heading_section_span(src, 2, "AI 摘要"), None);
        assert!(heading_section_span(src, 1, "AI 摘要").is_some());
    }

    #[test]
    fn spans_parallel_and_contiguous() {
        let docs = [
            "# 标题\n\n段落 **粗体** 与 `code`。\n\n- a\n- b\n",
            "| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn f() {}\n```\n",
            "> 引用\n\n[^1]: 脚注\n\n引用[^1]。\n\n---\n",
            "无格式文本",
            "",
        ];
        for src in docs {
            let doc = parse(src);
            assert_eq!(doc.tokens.len(), doc.spans.len(), "not parallel: {src:?}");
            let mut prev_end = 0;
            for (i, span) in doc.spans.iter().enumerate() {
                assert_eq!(span.start, prev_end, "span {i} not contiguous: {src:?}");
                assert!(span.end >= span.start, "span {i} inverted: {src:?}");
                assert!(span.end <= src.len(), "span {i} beyond source: {src:?}");
                prev_end = span.end;
            }
        }
    }

    // —— inline_marks(LP2-1 活动块内联标记半隐藏)——

    /// 期望表断言:(字节起点, 字面文本) 双重核对 —— 区间序列与切片内容都
    /// 必须对上,CJK 多字节的字节数写错时切片对照当场失败,不靠手算。
    fn assert_marks(text: &str, marks: &[Range<usize>], expected: &[(usize, &str)]) {
        let labeled: Vec<Range<usize>> = expected
            .iter()
            .map(|(start, label)| *start..*start + label.len())
            .collect();
        assert_eq!(marks, labeled, "期望字面 {expected:?}");
        for (range, (_, label)) in marks.iter().zip(expected) {
            assert_eq!(&text[range.clone()], *label);
        }
    }

    /// 输出的形状永远合法:升序、互不重叠、非空、不越界。
    fn assert_well_formed(text: &str, marks: &[Range<usize>]) {
        let mut prev_end = 0;
        for range in marks {
            assert!(!range.is_empty(), "空区间: {marks:?}");
            assert!(range.start >= prev_end, "重叠/乱序: {marks:?}");
            assert!(range.end <= text.len(), "越界: {marks:?}");
            prev_end = range.end;
        }
    }

    /// 四种强调定界符 + 删除线:开/闭定界符各自成段,内容不标。
    #[test]
    fn inline_marks_covers_emphasis_delimiters() {
        let text = "**b** *i* _u_ ~~s~~";
        let marks = inline_marks(text);
        assert_marks(
            text,
            &marks,
            &[
                (0, "**"),
                (3, "**"),
                (6, "*"),
                (8, "*"),
                (10, "_"),
                (12, "_"),
                (14, "~~"),
                (17, "~~"),
            ],
        );
        assert_well_formed(text, &marks);
    }

    /// 嵌套强调(`***x***`):内外层定界符合并成前后各一整段 `***`。
    #[test]
    fn inline_marks_merges_nested_emphasis() {
        let text = "***both***";
        assert_marks(text, &inline_marks(text), &[(0, "***"), (7, "***")]);

        // 强调里套行内代码:代码反引号与强调定界符都标,内容不标
        let text = "*a `b` c*";
        assert_marks(
            text,
            &inline_marks(text),
            &[(0, "*"), (3, "`"), (5, "`"), (8, "*")],
        );
    }

    /// 行内代码:只标两端反引号串;双反引号包裹(内容本身含反引号)不误伤。
    #[test]
    fn inline_marks_tags_code_backticks_only() {
        let text = "`code` 与 ``a ` b``";
        let marks = inline_marks(text);
        assert_marks(text, &marks, &[(0, "`"), (5, "`"), (11, "``"), (18, "``")]);
        // 内容不算标记
        assert!(!marks.iter().any(|r| &text[r.clone()] == "code"));
        assert!(!marks.iter().any(|r| text[r.clone()].contains('a')));
    }

    /// 链接与图片:括号与目标部分是标记,链接文本不标;autolink 只标尖括号。
    #[test]
    fn inline_marks_tags_link_brackets_and_destination() {
        let text = "[点我](https://e.com)";
        let marks = inline_marks(text);
        assert_marks(text, &marks, &[(0, "["), (7, "](https://e.com)")]);

        let text = "![alt](img.png)";
        assert_marks(text, &inline_marks(text), &[(0, "!["), (5, "](img.png)")]);

        let text = "<https://auto.link>";
        assert_marks(text, &inline_marks(text), &[(0, "<"), (18, ">")]);

        // 链接文本里套粗体:粗体定界符与链接括号都标,且合并不重复
        let text = "[**b**](u)";
        assert_marks(text, &inline_marks(text), &[(0, "[**"), (4, "**](u)")]);
    }

    /// CJK 混排与 CRLF:字节区间落在字符边界上,判定不受多字节字符影响。
    #[test]
    fn inline_marks_handles_cjk_and_crlf() {
        let text = "**中文**与*混排*和[链接](u)";
        let marks = inline_marks(text);
        assert_marks(
            text,
            &marks,
            &[
                (0, "**"),
                (8, "**"),
                (13, "*"),
                (20, "*"),
                (24, "["),
                (31, "](u)"),
            ],
        );
        assert_well_formed(text, &marks);
        for range in &marks {
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
        }

        assert_marks(
            "**b**\r\n尾行",
            &inline_marks("**b**\r\n尾行"),
            &[(0, "**"), (3, "**")],
        );
    }

    /// 围栏/缩进代码块内容不算标记(里面的 `*`、`[` 是代码,遮了就改坏语义)。
    #[test]
    fn inline_marks_skips_code_block_contents() {
        let text = "```rust\na * b [x](y) **z**\n```\n";
        assert!(inline_marks(text).is_empty());

        let text = "    indented *x* code\n";
        assert!(inline_marks(text).is_empty());

        // 块外的散置星号是普通文本(CommonMark:两侧无紧贴内容则不构成强调)
        assert!(inline_marks("a * b").is_empty());
    }

    /// 未闭合的定界符是普通文本(pulldown 语义),不算标记。
    #[test]
    fn inline_marks_skips_unclosed_markers() {
        assert!(inline_marks("未闭合 **bold").is_empty());
        assert!(inline_marks("`unclosed code").is_empty());
        assert!(inline_marks("[unclosed link").is_empty());
    }

    /// 列表/引用块内的行内标记照标(块级标记本身 `- `、`> ` 不标)。
    #[test]
    fn inline_marks_inside_list_and_quote_blocks() {
        let text = "- 项目 **粗**\n";
        let marks = inline_marks(text);
        assert_marks(text, &marks, &[(9, "**"), (14, "**")]);
        assert!(marks[0].start > 2, "列表符 `- ` 不是标记: {marks:?}");

        let text = "> 引用 *i*\n";
        let marks = inline_marks(text);
        assert_marks(text, &marks, &[(9, "*"), (11, "*")]);
        assert!(marks[0].start > 2, "引用符 `> ` 不是标记: {marks:?}");
    }

    /// 空输入与纯文本:无标记。
    #[test]
    fn inline_marks_empty_and_plain_text() {
        assert!(inline_marks("").is_empty());
        assert!(inline_marks("普通段落,没有标记。").is_empty());
    }

    // —— inline_style_spans(LP2-4 活动块内联构造自动成形)——

    fn span_of(range: Range<usize>, kind: InlineStyleKind) -> InlineStyleSpan {
        InlineStyleSpan { range, kind }
    }

    /// 六种构造各出一条**内容**区间:定界符一律不在区间内。
    #[test]
    fn inline_styles_cover_common_constructs() {
        let text = "**b** *i* ~~s~~ `c` [t](u) ==h==";
        assert_eq!(
            inline_style_spans(text),
            vec![
                span_of(2..3, InlineStyleKind::Strong),
                span_of(7..8, InlineStyleKind::Emphasis),
                span_of(12..13, InlineStyleKind::Strikethrough),
                span_of(17..18, InlineStyleKind::Code),
                span_of(21..22, InlineStyleKind::LinkText),
                span_of(29..30, InlineStyleKind::Highlight),
            ]
        );
    }

    /// 未配平的定界符是普通文本:打完闭标记才成形(「输入后自动变成对应
    /// 格式」的语义边界)。
    #[test]
    fn inline_styles_unclosed_constructs_stay_plain() {
        assert!(inline_style_spans("").is_empty());
        assert!(inline_style_spans("普通正文,没有任何定界符。").is_empty());
        assert!(inline_style_spans("**b `c [t](u ==h").is_empty());
    }

    /// 下划线变体与 CJK 内容:区间按字节,不按字符数。
    #[test]
    fn inline_styles_underscore_and_cjk() {
        assert_eq!(
            inline_style_spans("__b__ _斜体_"),
            vec![
                span_of(2..3, InlineStyleKind::Strong),
                span_of(7..13, InlineStyleKind::Emphasis), // `_`(6) + 6 字节 CJK(7..13)
            ]
        );
    }

    /// 嵌套强调(`***x***`):两层构造区间彼此覆盖,各自剥出同一内容区间。
    #[test]
    fn inline_styles_nested_emphasis_each_layer() {
        assert_eq!(
            inline_style_spans("***x***"),
            vec![
                span_of(3..4, InlineStyleKind::Strong),
                span_of(3..4, InlineStyleKind::Emphasis),
            ]
        );
        // 强调里套行内代码:两层各出区间(消费侧让代码整段自成样式)
        assert_eq!(
            inline_style_spans("*a `b` c*"),
            vec![
                span_of(1..8, InlineStyleKind::Emphasis),
                span_of(4..5, InlineStyleKind::Code),
            ]
        );
    }

    /// 链接文字与 autolink:文字区间成形,`](目标)` 不在内。
    #[test]
    fn inline_styles_link_text_and_autolink() {
        assert_eq!(
            inline_style_spans("看 [文档](https://a.b) 与 <https://c.d>"),
            vec![
                span_of(5..11, InlineStyleKind::LinkText), // 「文档」6 字节
                span_of(31..42, InlineStyleKind::LinkText), // autolink 的 URL
            ]
        );
    }

    /// 图片 alt 不出样式:富渲染整块换成图片 widget,活动块保持源码观感。
    #[test]
    fn inline_styles_image_alt_stays_plain() {
        assert!(inline_style_spans("![alt](x.png)").is_empty());
    }

    /// 高亮豁免:反引号内的 `==` 不成形,与渲染同一语义(整体只有代码区间)。
    #[test]
    fn inline_styles_highlight_exempts_code() {
        assert_eq!(
            inline_style_spans("`==x==`"),
            vec![span_of(1..6, InlineStyleKind::Code)]
        );
    }

    /// 围栏代码块内容是 Event::Text,不出样式区间(```rs\nlet x = 1;\n```)。
    #[test]
    fn inline_styles_skip_code_block_contents() {
        assert!(inline_style_spans("```rs\nlet a = *b*;\n```").is_empty());
    }

    // —— inline_mark_pairs / mark_interaction(LP2-2 选区扩展与配对显形)——

    /// 期望表断言配对:(构造字面, 开标记字面, 闭标记字面) 三重核对。
    fn assert_pairs(text: &str, pairs: &[MarkPair], expected: &[(&str, &str, &str)]) {
        assert_eq!(
            pairs.len(),
            expected.len(),
            "对数不符:{pairs:?} vs {expected:?}"
        );
        for (pair, (whole, opening, closing)) in pairs.iter().zip(expected) {
            assert_eq!(&text[pair.construct.clone()], *whole);
            assert_eq!(&text[pair.opening.clone()], *opening);
            assert_eq!(&text[pair.closing.clone()], *closing);
        }
    }

    /// 成对匹配:强调/行内代码/链接/图片/autolink 各自成对,开闭两侧
    /// 字面与构造整段都对得上。
    #[test]
    fn mark_pairs_match_constructs() {
        let text = "**b** `c` [t](u) ![a](i.png) <https://x.y>";
        let marks = inline_marks_with_pairs(text);
        assert_pairs(
            text,
            &marks.pairs,
            &[
                ("**b**", "**", "**"),
                ("`c`", "`", "`"),
                ("[t](u)", "[", "](u)"),
                ("![a](i.png)", "![", "](i.png)"),
                ("<https://x.y>", "<", ">"),
            ],
        );
        // 段表与 LP2-1 完全一致(重构不得改半隐藏行为)
        assert_eq!(marks.segments, inline_marks(text));
    }

    /// 嵌套:`***x***` 内外层各成一对(pulldown 给内层 Strong 与外层
    /// Emphasis 不同的构造区间);链接文本里套强调,两对各自完整。
    #[test]
    fn mark_pairs_cover_nested_constructs_per_level() {
        let text = "***x***";
        let marks = inline_marks_with_pairs(text);
        assert_pairs(
            text,
            &marks.pairs,
            &[("**x**", "**", "**"), ("***x***", "*", "*")],
        );

        let text = "[**b**](u)";
        let marks = inline_marks_with_pairs(text);
        assert_pairs(
            text,
            &marks.pairs,
            &[("**b**", "**", "**"), ("[**b**](u)", "[", "](u)")],
        );
    }

    /// 单侧孤标记(未闭合)的退化:pulldown 视作普通文本,无段也无对,
    /// 显形/扩展随之整体退化为「什么都不发生」。
    #[test]
    fn mark_pairs_skip_single_sided_orphans() {
        for text in ["未闭合 **bold", "`半截代码", "[没目标"] {
            let marks = inline_marks_with_pairs(text);
            assert!(marks.segments.is_empty(), "{text:?}");
            assert!(marks.pairs.is_empty(), "{text:?}");
        }
        // 空文本链接拆不出两侧,不成对(段仍在,半隐藏不受影响)
        let marks = inline_marks_with_pairs("[](u)");
        assert!(marks.pairs.is_empty());
        assert_eq!(marks.segments.len(), 1, "{:?}", marks.segments);
    }

    /// 配对显形:光标/选区压住一侧标记,另一侧所在段同时显形 —— 这是
    /// LP2-1 规则(只显形被压住的段)之上的增量。
    #[test]
    fn mark_interaction_reveals_partner_side() {
        let text = "**b** x *i*";
        let marks = inline_marks_with_pairs(text);

        // 塌缩光标压住开 `**`(字节 0..2,取边界 1):开闭两段都显形,
        // 无关的 `*i*` 两段保持半隐藏
        let out = mark_interaction(text, &marks, 1, 1, None);
        assert_eq!(out.revealed, vec![true, true, false, false]);
        assert_eq!(out.expanded, None, "无手势不扩展");

        // 非空选区只盖住开 `**`(字符 0..2):同样两段显形
        let out = mark_interaction(text, &marks, 0, 2, None);
        assert_eq!(out.revealed, vec![true, true, false, false]);

        // 选区只盖内容 `b`(字符 2..3):不贴标记,全隐藏(LP2-1 口径保持)
        let out = mark_interaction(text, &marks, 2, 3, None);
        assert_eq!(out.revealed, vec![false, false, false, false]);
    }

    /// 双击扩展:词选区扩到包含它的**最内层**标记对;跨对/无对不扩展。
    #[test]
    fn mark_interaction_expands_double_click_to_innermost_pair() {
        let text = "[**b**](u) 和 `c`";
        let marks = inline_marks_with_pairs(text);

        // 双击词 `b`(字符 3..4)→ 最内层是粗体对,不是外层链接对
        let out = mark_interaction(text, &marks, 3, 4, Some(MarkGesture::DoubleClick));
        assert_eq!(out.expanded, Some(1..6), "扩到 **b**,不带链接括号");

        // 双击链接外的词 `和`(字符 11..12):无对包含,不扩展
        let out = mark_interaction(text, &marks, 11, 12, Some(MarkGesture::DoubleClick));
        assert_eq!(out.expanded, None);

        // 跨对选区(选了链接一半+一半正文)不吸附
        let text2 = "**ab** cd *ef*";
        let marks2 = inline_marks_with_pairs(text2);
        let out = mark_interaction(text2, &marks2, 0, 9, Some(MarkGesture::DoubleClick));
        assert_eq!(out.expanded, None);
    }

    /// 拖选扩展:锚点压着标记(含边界,单字符标记只有边界可压)且选区
    /// 整个含在构造内才扩展;从内容中部发起、或拖出构造外,保持原选区。
    #[test]
    fn mark_interaction_expands_drag_only_from_mark_anchor() {
        let text = "前缀 **abcd** 后缀";
        let marks = inline_marks_with_pairs(text);
        // 字符:`前缀 `(3) + `**`(2) + `abcd`(4) + `**`(2) + ...
        // 开 `**` = 字符 3..5,内容 5..9,构造 3..11

        // 锚点在开 `**` 边界(字符 5 = opening.end),拖到内容中部松手
        let out = mark_interaction(
            text,
            &marks,
            5,
            7,
            Some(MarkGesture::DragRelease { anchor: 5 }),
        );
        assert_eq!(out.expanded, Some(3..11), "从标记内侧发起 → 扩到整对");

        // 锚点在内容中部:普通拖选,不吸附
        let out = mark_interaction(
            text,
            &marks,
            6,
            8,
            Some(MarkGesture::DragRelease { anchor: 6 }),
        );
        assert_eq!(out.expanded, None);

        // 锚点压标记但拖出构造(选区含构造外的前缀):不吸附
        let out = mark_interaction(
            text,
            &marks,
            0,
            6,
            Some(MarkGesture::DragRelease { anchor: 5 }),
        );
        assert_eq!(out.expanded, None);
    }

    /// 选区已含标记:显形两段全亮;扩展幂等(结果就是当前区间)。
    #[test]
    fn mark_interaction_idempotent_when_selection_already_covers_pair() {
        let text = "**b** x";
        let marks = inline_marks_with_pairs(text);
        let out = mark_interaction(text, &marks, 0, 5, Some(MarkGesture::DoubleClick));
        assert_eq!(out.expanded, Some(0..5));
        assert_eq!(out.revealed, vec![true, true]);
    }

    /// CJK:字符偏移输入与字节区间标记在函数内部正确换算。
    #[test]
    fn mark_interaction_converts_cjk_char_and_byte_domains() {
        let text = "**中文** 尾";
        // 字符:0-1 `**`、2-3 中文、4-5 `**`;字节:开 0..2、闭 8..10
        let marks = inline_marks_with_pairs(text);
        assert_pairs(text, &marks.pairs, &[("**中文**", "**", "**")]);

        // 光标停在闭 `**` 首边界(字符 4 = 字节 8):开闭两段都显形
        let out = mark_interaction(text, &marks, 4, 4, None);
        assert_eq!(out.revealed, vec![true, true]);

        // 双击词「中文」(字符 2..4)→ 扩到字符 0..6
        let out = mark_interaction(text, &marks, 2, 4, Some(MarkGesture::DoubleClick));
        assert_eq!(out.expanded, Some(0..6));
    }

    // —— heading_slug / Slugger / generate_toc(#66 M1 TOC 生成纯函数)——

    #[test]
    fn heading_slug_keeps_cjk_lowercases_ascii_and_maps_spaces() {
        // CJK 保留原样
        assert_eq!(heading_slug("架构设计"), "架构设计");
        assert_eq!(heading_slug("中文 标题"), "中文-标题");
        // ASCII 小写化 + 空格 → 连字符
        assert_eq!(heading_slug("Hello World"), "hello-world");
        assert_eq!(heading_slug("API Design"), "api-design");
        // 下划线与已有连字符保留
        assert_eq!(heading_slug("snake_case-name"), "snake_case-name");
        // 连续空白逐个映射,不合并
        assert_eq!(heading_slug("a  b"), "a--b");
    }

    /// 特殊字符与 emoji 逐字符断言:标点删除、emoji 删除、`+`/`&`/`.` 的
    /// 产物按规则手工展开核对(含首尾空白不 trim 的产出)。
    #[test]
    fn heading_slug_punctuation_and_emoji_char_by_char() {
        // `+`/`&` 删,两处空格各变一个 `-`
        assert_eq!(heading_slug("C++ & Rust"), "c--rust");
        // `.` 删
        assert_eq!(heading_slug("v1.2"), "v12");
        // `/` 删
        assert_eq!(heading_slug("a/b"), "ab");
        // emoji 删,其后的空格照常变 `-`(GitHub anchor `-launch` 同款形态)
        assert_eq!(heading_slug("🚀 Launch"), "-launch");
        // 前导空格同样产出首 `-`
        assert_eq!(heading_slug(" Launch"), "-launch");
        // 纯 emoji:全部删除 → 空 slug(已知边界,TOC 产出 (#))
        assert_eq!(heading_slug("🚀🚀"), "");
        // 全角标点(非 alphanumeric)删,CJK 正文保留
        assert_eq!(heading_slug("标题:副题"), "标题副题");
        // 空输入
        assert_eq!(heading_slug(""), "");
    }

    /// 重复标题去重:第二个同名标题加 `-1`,第三个加 `-2`;与真实标题
    /// `x-1` 撞车时继续加后缀不回吸。
    #[test]
    fn slugger_dedupes_repeated_headings() {
        let mut slugger = Slugger::new();
        assert_eq!(slugger.slug("同"), "同");
        assert_eq!(slugger.slug("同"), "同-1");
        assert_eq!(slugger.slug("同"), "同-2");
        assert_eq!(slugger.slug("同-1"), "同-1-1", "与已分配的后缀撞车继续加");
        assert_eq!(slugger.slug("别的"), "别的");
    }

    /// 多级标题:1–3 级进 TOC,2 空格 × 相对层级缩进;h4–h6 被深度上限
    /// 滤除,但仍消耗去重计数(与导出侧 id 注入对齐的前提)。
    #[test]
    fn generate_toc_levels_indent_and_depth_cap() {
        let items = vec![
            OutlineItem {
                level: 1,
                text: "甲".into(),
                span: 0..0,
            },
            OutlineItem {
                level: 2,
                text: "乙".into(),
                span: 0..0,
            },
            OutlineItem {
                level: 3,
                text: "丙".into(),
                span: 0..0,
            },
            OutlineItem {
                level: 4,
                text: "丁".into(),
                span: 0..0,
            },
            OutlineItem {
                level: 5,
                text: "戊".into(),
                span: 0..0,
            },
            OutlineItem {
                level: 6,
                text: "己".into(),
                span: 0..0,
            },
        ];
        assert_eq!(
            generate_toc(&items),
            "- [甲](#甲)\n  - [乙](#乙)\n    - [丙](#丙)\n"
        );
        // depth 外的重复标题照样吃计数:后续「丙」的第二个拿到 -1
        let items = vec![
            OutlineItem {
                level: 3,
                text: "丙".into(),
                span: 0..0,
            },
            OutlineItem {
                level: 4,
                text: "丙".into(),
                span: 0..0,
            },
            OutlineItem {
                level: 3,
                text: "丙".into(),
                span: 0..0,
            },
        ];
        assert_eq!(
            generate_toc(&items),
            "- [丙](#丙)\n- [丙](#丙-2)\n",
            "h4 的同名标题消耗「丙-1」但不产出文本行,第二个 h3 拿「丙-2」"
        );
    }

    /// 文档从更深层级开头时,缩进基线取入选条目的最小层级:首行永远零
    /// 缩进,不会在文件顶端落成缩进代码块;中间缺级照实缩进。
    #[test]
    fn generate_toc_indent_baseline_is_min_selected_level() {
        let items = vec![
            OutlineItem {
                level: 2,
                text: "壹".into(),
                span: 0..0,
            },
            OutlineItem {
                level: 3,
                text: "贰".into(),
                span: 0..0,
            },
            OutlineItem {
                level: 3,
                text: "叁".into(),
                span: 0..0,
            },
        ];
        assert_eq!(
            generate_toc(&items),
            "- [壹](#壹)\n  - [贰](#贰)\n  - [叁](#叁)\n"
        );
        // 跳级:h1 直接到 h3,相对层级 2 → 4 空格
        let items = vec![
            OutlineItem {
                level: 1,
                text: "甲".into(),
                span: 0..0,
            },
            OutlineItem {
                level: 3,
                text: "丙".into(),
                span: 0..0,
            },
        ];
        assert_eq!(generate_toc(&items), "- [甲](#甲)\n    - [丙](#丙)\n");
    }

    /// 空文档/无标题文档:空切片与全 depth 外条目都返回空串(不产提示,
    /// 提示属消费方 UI)。
    #[test]
    fn generate_toc_empty_input_yields_empty_string() {
        assert_eq!(generate_toc(&[]), "");
        let deep_only = vec![OutlineItem {
            level: 4,
            text: "深".into(),
            span: 0..0,
        }];
        assert_eq!(generate_toc(&deep_only), "");
    }

    /// 重复标题去重与显示文本方括号转义:`[x]` 标题产出 `\[x\]`,链接
    /// 语法不被拆坏;纯 emoji 标题产空锚点 `(#)`。
    #[test]
    fn generate_toc_dedupes_and_escapes_labels() {
        let items = vec![
            OutlineItem {
                level: 2,
                text: "同名".into(),
                span: 0..0,
            },
            OutlineItem {
                level: 2,
                text: "同名".into(),
                span: 0..0,
            },
            OutlineItem {
                level: 2,
                text: "勾选 [x] 态".into(),
                span: 0..0,
            },
            OutlineItem {
                level: 2,
                text: "🚀🚀".into(),
                span: 0..0,
            },
        ];
        assert_eq!(
            generate_toc(&items),
            "- [同名](#同名)\n- [同名](#同名-1)\n- [勾选 \\[x\\] 态](#勾选-x-态)\n- [🚀🚀](#)\n"
        );
    }

    /// 端到端:真实 `outline()` 通道 → TOC。含内联格式的标题行按 vendored
    /// 解析器拆成多条(既有廉价口径),此处如实钉住行为。
    #[test]
    fn generate_toc_feeds_from_real_outline_channel() {
        let src = "# 后来的指南\n\n正文。\n\n## 中文 标题\n\n### 🚀 Launch\n\n## **粗**体标题\n\n#### 太深\n";
        let toc = generate_toc(&outline(src));
        // 内联标题 `## **粗**体标题` 被解析器拆成「粗」「体标题」两段,
        // outline 按行合并回一条(与导出 HTML 标题 id 同源口径)
        assert_eq!(
            toc,
            "- [后来的指南](#后来的指南)\n  - [中文 标题](#中文-标题)\n    - [🚀 Launch](#-launch)\n  - [粗体标题](#粗体标题)\n"
        );
    }

    /// 拥有型的意义:文档模型活得比源字符串久。
    #[test]
    fn doc_outlives_source() {
        let doc = {
            let src = "# 临时标题".to_string();
            parse(&src)
        };
        assert_eq!(doc.outline()[0].text, "临时标题");
        assert_eq!(doc.text, "# 临时标题");
    }

    // —— TOC_BLOCK / TOC_REGION_SPAN(#66 M2 插入与整块替换的识别口径)——

    /// 含行内代码的标题只产一条 TOC 条目,锚点 slug 取全文(与导出 HTML
    /// 标题 id 同源,`inline_code_in_heading_joins_title` 是导出侧的对照)。
    #[test]
    fn generate_toc_inline_code_heading_is_one_entry() {
        let src = "# 标题含 `code` 内联\n\n## 子节\n";
        let toc = generate_toc(&outline(src));
        assert_eq!(
            toc,
            "- [标题含 code 内联](#标题含-code-内联)\n  - [子节](#子节)\n"
        );
    }

    /// `toc_block` 包住 `generate_toc` 产出,且被 `toc_region_span` 原样
    /// 识别回来 —— 写入与识别同一事实源(模块文档的构造性一致断言)。
    #[test]
    fn toc_block_round_trips_through_region_span() {
        let src = "# 甲\n\n## 乙\n";
        let block = toc_block(&outline(src));
        assert_eq!(
            block,
            "<!-- TOC -->\n- [甲](#甲)\n  - [乙](#乙)\n<!-- /TOC -->\n"
        );
        let text = format!("前言。\n\n{block}正文。\n");
        let span = toc_region_span(&text).expect("生成的块应被识别回来");
        assert_eq!(&text[span.clone()], block, "区间恰为块本体(含尾换行)");
        assert!(text.is_char_boundary(span.start) && text.is_char_boundary(span.end));
    }

    /// 空大纲产出空壳块(消费方负责先判空,不把壳写进文档)。
    #[test]
    fn toc_block_empty_outline_is_marker_shell_only() {
        assert_eq!(toc_block(&[]), "<!-- TOC -->\n<!-- /TOC -->\n");
        assert_eq!(
            toc_block(&outline("#### 只有大标题档\n")),
            "<!-- TOC -->\n<!-- /TOC -->\n"
        );
    }

    /// 区间识别:正文夹块取「第一对」标记,行尾空白与 CRLF 的 `\r` 容忍;
    /// span 端点都是字符边界(CJK 夹块)。
    #[test]
    fn toc_region_span_tolerates_whitespace_and_crlf() {
        // CRLF 文档(Windows 原样进出,file 层不做换行转换):整块含 `\r\n`
        let crlf = "前文甲\r\n<!-- TOC -->\r\n- [甲](#甲)\r\n<!-- /TOC -->\r\n后文乙\r\n";
        let span = toc_region_span(crlf).expect("CRLF 下的标记可识别");
        assert!(crlf[span.clone()].starts_with("<!-- TOC -->\r\n"));
        assert!(crlf[span].ends_with("<!-- /TOC -->\r\n"));

        // 行尾空白容忍 + 块后无尾换行的文末块:区间到文末收口
        let tail = "开头\n<!-- TOC --> \n- [甲](#甲)\n<!-- /TOC -->";
        let span = toc_region_span(tail).expect("文末块可识别");
        assert_eq!(span.start, "开头\n".len());
        assert_eq!(span.end, tail.len(), "末行无换行时区间到文末");

        // CJK 夹块:字节端点必须落在字符边界上
        let cjk = "中文甲乙丙\n<!-- TOC -->\n<!-- /TOC -->\n中文丁戊\n";
        let span = toc_region_span(cjk).expect("CJK 夹块可识别");
        assert!(cjk.is_char_boundary(span.start) && cjk.is_char_boundary(span.end));
    }

    /// 识别的反面:无开始标记、有头无尾、结束标记先于开始标记,都按
    /// 「无块」处理(消费方走首次插入);嵌套/重复时取第一对。
    #[test]
    fn toc_region_span_requires_paired_markers() {
        assert_eq!(toc_region_span("# 没有标记\n\n正文\n"), None);
        assert_eq!(
            toc_region_span("<!-- TOC -->\n- [甲](#甲)\n"),
            None,
            "有头无尾"
        );
        assert_eq!(
            toc_region_span("<!-- /TOC -->\n正文\n<!-- TOC -->\n"),
            None,
            "结束标记先于开始标记不成对"
        );
        // 第一对生效:第二个开始标记在第一对之外时被无视
        let text = "x\n<!-- TOC -->\n<!-- /TOC -->\n<!-- TOC -->\n孤行\n";
        let span = toc_region_span(text).expect("第一对生效");
        assert_eq!(&text[span], "<!-- TOC -->\n<!-- /TOC -->\n");
    }

    /// 替换口径的收敛性:块内手工乱改后再「识别 → 整块替换」两轮,文本
    /// 稳定不再漂(替换文本恰为块本体,块外字节零增删)。
    #[test]
    fn replace_round_trip_is_stable() {
        let items = outline("# 甲\n\n## 乙\n");
        let mut text = "前言\n\n".to_owned();
        text.push_str(&toc_block(&items));
        text.push_str("后记\n");
        // 手工把块内改成乱内容,再按口径替换回生成块
        let span = toc_region_span(&text).expect("前置:块可识别");
        text.replace_range(span, "<!-- TOC -->\n随手乱写的一行\n<!-- /TOC -->\n");
        let block = toc_block(&items);
        let span = toc_region_span(&text).expect("手工块同样可识别");
        text.replace_range(span, &block);
        assert!(text.contains(&block));
        assert!(!text.contains("随手乱写"));
        // 再执行一轮(内容相同):文本不再变化
        let span = toc_region_span(&text).expect("前置:块仍可识别");
        let before = text.clone();
        text.replace_range(span, &block);
        assert_eq!(text, before, "同内容反复替换零漂移");
    }

    /// 借转拥有不丢数据:表格的表头/行/单元格逐层对照。
    #[test]
    fn table_survives_owned_conversion() {
        let doc = parse("| 列甲 | 列乙 |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n");
        let borrowed = egui_markdown::parser::parse(&doc.text);
        let owned_table = doc
            .tokens
            .iter()
            .find_map(|t| match t {
                Token::Table(data) => Some(data),
                _ => None,
            })
            .expect("owned table token");
        let borrowed_table = borrowed
            .tokens
            .iter()
            .find_map(|t| match t {
                Token::Table(data) => Some(data),
                _ => None,
            })
            .expect("borrowed table token");
        assert_eq!(owned_table.alignments, borrowed_table.alignments);
        assert_eq!(owned_table.headers.len(), borrowed_table.headers.len());
        assert_eq!(owned_table.rows.len(), borrowed_table.rows.len());
        let cell_text = |data: &TableData<'_>, row: usize, col: usize| -> String {
            data.rows[row][col]
                .iter()
                .map(|t| t.text().to_string())
                .collect()
        };
        assert_eq!(
            cell_text(owned_table, 0, 0),
            cell_text(borrowed_table, 0, 0)
        );
        assert_eq!(
            cell_text(owned_table, 1, 1),
            cell_text(borrowed_table, 1, 1)
        );
    }

    /// 块级 heal 契约(live-unify-plan P1/R1):AI 流式的残缺尾部以「块」为
    /// 单位喂给 Live 富渲染(`MarkdownLabel::heal(true)` 逐块开启),heal 必须
    /// 在**块片段**上就把残缺补齐。达标即块级 heal 够用,不必退回「流式期间
    /// 整篇 heal 后再切块」—— 那条的 healed 偏移与编辑缓冲的原始字节对不上,
    /// Live 的块表(编辑安全底线:块区间必须落在原始文本上)不可用。
    #[test]
    fn block_level_heal_closes_streaming_fragments() {
        // 残缺 fence(流式最常见):尾部块补闭合。流式尾部即文档尾,块级
        // 补的位置与整篇 heal 相同,两者渲染等价。
        let streaming = "# 标题\n\n正文。\n\n```rust\nfn main() {\n";
        let tail = blocks(streaming).last().expect("流式尾部成块").clone();
        let tail = &streaming[tail];
        assert!(
            tail.starts_with("```rust"),
            "未闭合 fence 的尾部自成一块:{tail:?}"
        );
        assert_eq!(egui_markdown::heal(tail).into_owned(), format!("{tail}```"));
        // 残缺表格(流式次常见):表头已出、分隔行只流到一半 → 补完整列数,
        // 解析出 Table token(截断的是**数据行**时 heal 不动它 —— 渲染仍合法,
        // 不构成残缺排版,见 vendored heal_table 的契约)。
        let partial = "| 列甲 | 列乙 |\n|---";
        let healed = egui_markdown::heal(partial).into_owned();
        assert!(
            parse(&healed)
                .tokens
                .iter()
                .any(|token| matches!(token, Token::Table(_))),
            "半截分隔行应补齐成可解析表格:{healed:?}"
        );
        // 未闭合 **:补对侧标记
        assert_eq!(
            egui_markdown::heal("加粗 **重点").into_owned(),
            "加粗 **重点**"
        );
        // 完整块零改动(稳态帧恒等,Cow::Borrowed 不付分配)
        let complete = "## 小节\n\n普通段落,标记完整闭合 **粗**。\n";
        for range in blocks(complete) {
            let block = &complete[range];
            assert_eq!(
                egui_markdown::heal(block).into_owned(),
                block,
                "完整块 heal 恒等:{block:?}"
            );
        }
    }
}
