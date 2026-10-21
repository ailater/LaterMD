//! Markdown 格式动作的**纯函数层**(docs/ui-shell-redesign.md §6.2)。
//!
//! 所有语义都在这里,`ui` 只负责画按钮、`logic` 只负责把结果写回缓冲:
//! `apply` 不吃 egui、不吃 IO,输入字符串与**字符**偏移,输出新字符串与
//! 新选区。十七条规则因此全都能落到单测上。
//!
//! ## 偏移一律是字符
//!
//! CJK 占三字节,按字节切会 panic 在字符中间。`EditorBuffer` 已有
//! `char_to_byte` / `byte_to_char`,`TextEdit` 的 `CCursor.index` 本身也
//! 是字符偏移 —— 字符偏移是三方的最小公约数。本模块内部只在最后合成
//! 字符串时用到字节边界,对外一律字符。
//!
//! ## Undo 粒度
//!
//! 走本模块的写入会打碎 `TextEdit` 内建 undoer 的快照,Ctrl+Z 可能一次
//! 回退一整次格式操作而非逐字。已知并接受(§9 R3,与 AI 流式首次 Ctrl+Z
//! 整段回退同款边界);单测因此只钉「文本与选区正确」,不钉 undo 粒度。

use std::ops::Range;

use latermd_md::{inline_style_spans, InlineStyleKind, InlineStyleSpan};

/// 格式动作。十七个,与工具条四组按钮一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FormatAction {
    /// `**粗体**`
    Bold,
    /// `*斜体*`
    Italic,
    /// `~~删除线~~`
    Strike,
    /// `` `行内代码` ``
    InlineCode,
    /// `[标题](url)`
    Link,
    /// `# `
    H1,
    /// `## `
    H2,
    /// `### `
    H3,
    /// 正文:去掉行前缀。
    Plain,
    /// `> `
    Quote,
    /// 围栏代码块(info string 空)。
    CodeBlock,
    /// `---`
    Divider,
    /// 2×2 表格骨架。
    Table,
    /// `![alt](url)` 图片(docs/image-plan.md A 段)。
    ///
    /// 与其余十六条不同:它需要 alt 与 url **两个**用户输入,工具条按钮与
    /// 快捷键都只**开对话框**,不直接改文档 —— 真正的写入走
    /// [`insert_image`]。列在动作枚举里,是为了让「图标 / 提示 / 键位 /
    /// 分组」四处与别的动作同构。
    Image,
    /// 复制选中内容(无选中=复制当前行),Ctrl+D。
    DuplicateSelection,
    /// 复制当前行,Ctrl+Shift+D。
    DuplicateLine,
    /// `- `
    Bullet,
    /// `1. `
    Ordered,
    /// `- [ ] ` → `- [x] ` → `- ` 三态循环。
    Task,
}

impl FormatAction {
    /// 悬浮提示文案(`ui::format_bar` 拼上快捷键后显示)。
    pub fn label(self) -> &'static str {
        use FormatAction::*;
        match self {
            Bold => "加粗",
            Italic => "斜体",
            Strike => "删除线",
            InlineCode => "行内代码",
            Link => "链接",
            H1 => "一级标题",
            H2 => "二级标题",
            H3 => "三级标题",
            Plain => "正文",
            Quote => "引用",
            CodeBlock => "代码块",
            Divider => "分割线",
            Table => "表格",
            Image => "图片",
            DuplicateSelection => "复制选中",
            DuplicateLine => "复制当前行",
            Bullet => "无序列表",
            Ordered => "有序列表",
            Task => "任务列表",
        }
    }

    /// 工具条顺序:组内按 `ALL` 出现顺序,组间按
    /// [`FormatGroup::ALL`]。
    pub const ALL: [FormatAction; 17] = [
        FormatAction::Bold,
        FormatAction::Italic,
        FormatAction::Strike,
        FormatAction::InlineCode,
        FormatAction::Link,
        FormatAction::H1,
        FormatAction::H2,
        FormatAction::H3,
        FormatAction::Plain,
        FormatAction::Quote,
        FormatAction::CodeBlock,
        FormatAction::Divider,
        FormatAction::Table,
        FormatAction::Image,
        FormatAction::Bullet,
        FormatAction::Ordered,
        FormatAction::Task,
    ];
}

/// 工具条的四个视觉分组。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatGroup {
    /// 行内:B / I / S / 行内代码 / 链接。
    Inline,
    /// 标题:H1 / H2 / H3 / 正文。
    Heading,
    /// 块:引用 / 代码块 / 分割线 / 表格 / 图片。
    Block,
    /// 列表:无序 / 有序 / 任务。
    List,
}

impl FormatGroup {
    /// 自左至右的顺序;组之间画竖分隔条,最后一组之后不画。
    pub const ALL: [FormatGroup; 4] = [
        FormatGroup::Inline,
        FormatGroup::Heading,
        FormatGroup::Block,
        FormatGroup::List,
    ];

    /// **窄档直出**的组(2026-10-08 S2-2;2026-10-10 起是三档里的最窄
    /// 一档,宽中两档见 `ui::format_bar::tier_plan`)。行内组 + 列表组 ——
    ///
    /// 依据 ui-polish §1.2「工具栏只留高频动作,其余收进设置,可发现性由
    /// 菜单栏负责」:行内五项(B / I / S / 行内代码 / 链接)是日常打字时的
    /// 高频动作,列表三项(无序 / 有序 / 任务)是写清单时的常用动作。标题与
    /// 块是**按段落语境**才用的低频动作,全部平铺时它们与行内组视觉权重
    /// 相同,等于把低频动作提到与高频同等显眼的位置。
    ///
    /// **可发现性不丢**:`FORMAT_MENU`(menubar.rs)早已收了全部 17 个格式
    /// 动作,菜单栏才是全量入口,工具条只是快捷面。
    ///
    /// **为什么列表组也直出**(首版只留行内组,被守门测试当场打回):
    /// `Task` 是十七个动作里唯一的**多步交互** —— 点一次循环
    /// `- [ ] ` → `- [x] ` → `- `。它进菜单后每次点击都会关闭菜单,一次
    /// 状态切换要「开菜单 + 点」×3 = **6 次点击**,反而比改版前的 3 次更
    /// 差。按「点击总次数」算,列表组才是高频。
    /// 这条是 `ui/layout.rs` 的 `task_button_cycling_on_cjk_never_panics`
    /// 逼出来的 —— 守门测试第一次跑就红(`探针拿到 Task 按钮:[0.0 0.0]`,
    /// 菜单关着时探针根本不触发)。**护栏测试确实能挡住设计缺陷**,
    /// 这次它不是事后补的,是当场拦下的。
    pub const DIRECT: [FormatGroup; 2] = [FormatGroup::Inline, FormatGroup::List];

    /// 进「更多」溢出菜单的组(2026-10-08 S2-2,窄档;中档只收块组,
    /// 宽档不画菜单),顺序即菜单内分段顺序。
    pub const OVERFLOW: [FormatGroup; 2] = [FormatGroup::Heading, FormatGroup::Block];

    /// 该组的动作,顺序即按钮顺序。
    pub fn actions(self) -> &'static [FormatAction] {
        // 各组的 `FormatAction::ALL` 前缀/片段;写成切片以免每帧分配。
        match self {
            FormatGroup::Inline => &FormatAction::ALL[0..5],
            FormatGroup::Heading => &FormatAction::ALL[5..9],
            FormatGroup::Block => &FormatAction::ALL[9..14],
            FormatGroup::List => &FormatAction::ALL[14..17],
        }
    }
}

/// 2×2 表格骨架。列宽由用户内容撑,这里只保证 GFM 认得出是表格。
const TABLE_SKELETON: &str = "| 列 1 | 列 2 |\n| --- | --- |\n|  |  |\n|  |  |\n";

/// 链接 placeholders:新建链接时 url 占位成 `https://`,让用户直接覆写。
const LINK_URL: &str = "https://";

/// 行内格式的**语义化 toggle**(LP2-10):选区落在同类型行内 span 的构造
/// 内(含定界符,部分选中也算)、或完整覆盖它的,拆掉该 span 的定界符;
/// 不命中返回 `None`,调用方回落到既有包裹路径。选区同时压到多个 span
/// 时取**最小构造**(最内层)。
///
/// 之前的 toggle 只认「选区紧贴定界符」,选中部分内容或连定界符一起选
/// 都会再包一层 —— 用户实测「选中行内代码再点图标反而叠两层」。本函数
/// 判定基于 [`inline_style_spans`](同渲染一套 pulldown options),与渲染
/// 同语义:未配平的定界符不产 span,自然走包裹。
fn unwrap_inline_kind(
    text: &str,
    sel: Range<usize>,
    kinds: &[InlineStyleKind],
) -> Option<(String, Range<usize>)> {
    let bytes = text.as_bytes();
    let char_at = |byte: usize| text[..byte.min(text.len())].chars().count();
    let delim = |kind: InlineStyleKind, byte: u8| match kind {
        InlineStyleKind::Code => byte == b'`',
        InlineStyleKind::Strong | InlineStyleKind::Emphasis => byte == b'*' || byte == b'_',
        InlineStyleKind::Strikethrough => byte == b'~',
        InlineStyleKind::Highlight => byte == b'=',
        InlineStyleKind::LinkText => false,
    };
    let mut best: Option<(Range<usize>, usize, usize, InlineStyleSpan)> = None;
    for span in inline_style_spans(text) {
        if !kinds.contains(&span.kind) {
            continue;
        }
        let mut open = span.range.start;
        while open > 0 && delim(span.kind, bytes[open - 1]) {
            open -= 1;
        }
        let mut close = span.range.end;
        while close < bytes.len() && delim(span.kind, bytes[close]) {
            close += 1;
        }
        // 两侧都得真有定界符;少一边说明定界符被用户改坏,别动它
        if open == span.range.start || close == span.range.end {
            continue;
        }
        let (c_start, c_end) = (char_at(open), char_at(close));
        let covers_construct = c_start >= sel.start.min(sel.end) && c_end <= sel.end.max(sel.start);
        let within_construct = sel.start >= c_start && sel.end <= c_end;
        if !(within_construct || covers_construct) {
            continue;
        }
        let smaller = best
            .as_ref()
            .map(|(_, _, _, best_span)| span.range.len() < best_span.range.len())
            .unwrap_or(true);
        if smaller {
            best = Some((open..close, c_start, c_end, span));
        }
    }
    let (construct, .., span) = best?;
    let mut out = String::with_capacity(text.len() - (construct.end - construct.start));
    out.push_str(&text[..construct.start]);
    out.push_str(&text[span.range.start..span.range.end]);
    out.push_str(&text[construct.end..]);
    // 新选区 = 新文本中的**内容**区(定界符已剥):内容顶到构造起点,
    // 长度 = 原内容字符数 —— 直接用新文本重算,规避旧文本偏移换算。
    let content_start = char_at(construct.start);
    let content_end = content_start + text[span.range.start..span.range.end].chars().count();
    Some((out, content_start..content_end))
}

/// 图片 placeholder:同 `LINK_URL`,工具条/快捷键直触(不经对话框)时用它。
///
/// 与链接共用同一个字面量而非另定常量:两者的「待覆写占位」语义一样,
/// 分家只会让将来改占位时漏改一处。
const IMAGE_URL: &str = LINK_URL;

/// 应用一次格式动作,返回**(新文本, 新选区)**。
///
/// `sel` 是字符区间,`start == end` 表示纯光标。返回选区同样是字符区间:
/// 行内类一律落在「内容处」,方便接着打字或叠加别的格式。
pub fn apply(action: FormatAction, text: &str, sel: Range<usize>) -> (String, Range<usize>) {
    use FormatAction::*;
    let view = View::new(text, sel.clone());
    match action {
        Bold => unwrap_inline_kind(text, sel, &[InlineStyleKind::Strong])
            .unwrap_or_else(|| view.wrap("**", "**")),
        Italic => unwrap_inline_kind(text, sel, &[InlineStyleKind::Emphasis])
            .unwrap_or_else(|| view.wrap("*", "*")),
        Strike => unwrap_inline_kind(text, sel, &[InlineStyleKind::Strikethrough])
            .unwrap_or_else(|| view.wrap("~~", "~~")),
        InlineCode => unwrap_inline_kind(text, sel, &[InlineStyleKind::Code])
            .unwrap_or_else(|| view.wrap("`", "`")),
        Link => view.link(),
        H1 => view.set_prefix("# "),
        H2 => view.set_prefix("## "),
        H3 => view.set_prefix("### "),
        Plain => view.set_prefix(""),
        Quote => view.set_prefix("> "),
        Bullet => view.set_prefix("- "),
        Ordered => view.set_prefix("1. "),
        Task => view.cycle_task(),
        CodeBlock => view.insert_block("```\n", "\n```\n"),
        Divider => view.insert_block("", "---\n"),
        Table => view.insert_block("", TABLE_SKELETON),
        Image => view.image(IMAGE_URL, None),
        DuplicateSelection => view.duplicate_selection(),
        DuplicateLine => view.duplicate_line(),
    }
}

/// 插入 `![alt](url)`,返回**(新文本, 新选区)**;新选区落在 **alt** 上。
///
/// 这是「图片框」对话框三条来源(网络地址 / 本地文件 / 图床上传)共同的
/// 汇流点(docs/image-plan.md §2):对话框只产出 `(url, alt)`,文本动作只有
/// 这一个,因此能被单测穷尽。
///
/// 新选区落在 alt 而不是整段:alt 是给人读的说明,比 url 更需要立刻填,
/// 留选区让用户直接覆写(与 [`View::link`] 落在 url 上是同一个「落在最需
/// 要马上写的地方」的判据 —— 链接的标题通常已有选区,图片的 alt 通常空着)。
///
/// 选区非空的情形:`sel` 覆盖的文字被替换掉(而不是被当 alt)—— 调用方要
/// 保留选中文字应当自己传进 `alt`(归约侧 [`crate::state`] 开对话框时就是
/// 这么预填的)。
pub fn insert_image(text: &str, sel: Range<usize>, url: &str, alt: &str) -> (String, Range<usize>) {
    View::new(text, sel).image(url, Some(alt))
}

/// 在选区处插入 emoji 字符,替换选中内容,新选区 collapsed 落在 emoji
/// 之后(docs/emoji-plan.md §6.1)。
///
/// 与 [`insert_image`] 同属「对话框类动作」—— emoji 字符无法从
/// `text + sel` 推导,必须由面板提供,故不进 [`FormatAction`](那条边界
/// 的完整论证见 emoji-plan §6.1)。选区是字符偏移;`emoji` 以 `chars`
/// 计数,单组件 emoji(含非 BMP)与旗帜的双码位都天然 CJK 安全。
pub fn insert_emoji(text: &str, sel: Range<usize>, emoji: &str) -> (String, Range<usize>) {
    let view = View::new(text, sel);
    let (a, b) = (view.byte_of(view.start), view.byte_of(view.stop));
    let (left, right) = (&text[..a], &text[b..]);
    let caret = left.chars().count() + emoji.chars().count();
    (format!("{left}{emoji}{right}"), caret..caret)
}

/// 已知的**行前缀**,按长度降序 —— 匹配时要先试长的(`### ` 先于 `# `)。
const LINE_PREFIXES: [&str; 8] = ["### ", "## ", "# ", "- [x] ", "- [ ] ", "> ", "- ", "1. "];

/// 去掉 `line` 的行前缀(有则去一个最长的),剩下的正文。
fn strip_line_prefix(line: &str) -> &str {
    LINE_PREFIXES
        .iter()
        .filter(|prefix| prefix.len() <= line.len())
        .find(|prefix| line.starts_with(**prefix))
        .map(|prefix| &line[prefix.len()..])
        .unwrap_or(line)
}

/// `line` 当前的行前缀,没有则空串。
fn line_prefix_of(line: &str) -> &'static str {
    LINE_PREFIXES
        .iter()
        .filter(|prefix| prefix.len() <= line.len())
        .find(|prefix| line.starts_with(**prefix))
        .copied()
        .unwrap_or("")
}

/// 文本 + 选区的一次性视图,字符偏移。
///
/// 每个处理方法消耗 `self` 返回 `(String, Range<usize>)`,避免半路的中间
/// 偏移漏出去。
struct View<'a> {
    text: &'a str,
    /// 已归一化:start ≤ stop,且不越界。
    start: usize,
    stop: usize,
}

impl<'a> View<'a> {
    fn new(text: &'a str, sel: Range<usize>) -> Self {
        let len = text.chars().count();
        let start = sel.start.min(len);
        let stop = sel.end.min(len).max(start);
        Self { text, start, stop }
    }

    /// 纯光标(无选区)。
    fn caret_only(&self) -> bool {
        self.start == self.stop
    }

    /// 字符偏移 → 字节偏移。
    ///
    /// 产出恒为字符边界 —— 本模块全部 `text[a..b]` 切片的唯一边界源头
    /// (`nth` 产出的本就是边界,floor 是防御层)。将来若有人改动换算,
    /// 错位输入只会语义偏移,不会 panic。
    fn byte_of(&self, char_idx: usize) -> usize {
        let byte = self
            .text
            .char_indices()
            .nth(char_idx)
            .map_or(self.text.len(), |(byte, _)| byte);
        self.text.floor_char_boundary(byte)
    }

    /// 选区所在行的**行尾(不含换行符)**,以及行尾之后到下一个换行
    /// (或文末)的位置。
    ///
    /// 两个产出恒为字符边界:`rfind('\n')`/`find('\n')` 落在 `\n` 上,
    /// `idx + 1` 与 `bytes.end + offset` 因此也是边界 —— `set_prefix` /
    /// `cycle_task` 直接拿它们切 `&text[a..b]` 是安全的。
    fn selected_line_bytes(&self) -> (usize, usize) {
        let bytes = self.byte_of(self.start)..self.byte_of(self.stop);
        let line_start = self.text[..bytes.start]
            .rfind('\n')
            .map_or(0, |idx| idx + 1);
        let line_end = self.text[bytes.end..]
            .find('\n')
            .map_or(self.text.len(), |offset| bytes.end + offset);
        (line_start, line_end)
    }

    /// 选区所在行的结束位置,**含**行尾换行符 —— 插入类模板落在它之后。
    fn current_line_end_inclusive(&self) -> usize {
        let line_end = self.selected_line_bytes().1;
        self.text[line_end..]
            .find('\n')
            .map_or(self.text.len(), |offset| line_end + offset + 1)
    }

    /// **行内包裹**:无选区插成对标记、光标放中间;有选区则包裹;已被同
    /// 标记紧贴包裹则去包裹(toggle off)。
    fn wrap(self, open: &'static str, close: &'static str) -> (String, Range<usize>) {
        let (a, b) = (self.byte_of(self.start), self.byte_of(self.stop));
        let (left, mid, right) = (&self.text[..a], &self.text[a..b], &self.text[b..]);

        if self.caret_only() {
            let pos = left.chars().count() + open.chars().count();
            return (format!("{left}{open}{close}{right}"), pos..pos);
        }

        // toggle off 的判定要严:`left` 以 open 结尾且 `right` 以 close
        // 开头,两者同时成立才算「选区正被这一对包着」。只有一边成立时
        // 用户框的是别的东西(比如 `**a**` 里的 `a`),应当继续包裹。
        if left.ends_with(open) && right.starts_with(close) {
            let trimmed_left = &left[..left.len() - open.len()];
            let trimmed_right = &right[close.len()..];
            let start = trimmed_left.chars().count();
            let end = start + mid.chars().count();
            return (format!("{trimmed_left}{mid}{trimmed_right}"), start..end);
        }

        let start = left.chars().count() + open.chars().count();
        let end = start + mid.chars().count();
        (format!("{left}{open}{mid}{close}{right}"), start..end)
    }

    /// 链接:`[选中](url)`,新选区落在 url 上便于直接覆写;无选区时标题为空。
    fn link(self) -> (String, Range<usize>) {
        let (a, b) = (self.byte_of(self.start), self.byte_of(self.stop));
        let (left, mid, right) = (&self.text[..a], &self.text[a..b], &self.text[b..]);

        // 光标 vs 选区只影响"标题是否为空",其余完全一致 —— 一条路径足矣
        let url_start = left.chars().count() + 1 + mid.chars().count() + 2;
        let out = format!("{left}[{mid}]({LINK_URL}){right}");
        let url_end = url_start + LINK_URL.chars().count();
        (out, url_start..url_end)
    }

    /// 图片:`![alt](url)`,新选区落在 alt 上。
    ///
    /// `alt` 为 `None` 时取**选中文字**当 alt(与 [`Self::link`] 同手法);
    /// 传 `Some` 则由调用方定(对话框已让用户填过),此时选中文字被替换。
    fn image(self, url: &str, alt: Option<&str>) -> (String, Range<usize>) {
        let (a, b) = (self.byte_of(self.start), self.byte_of(self.stop));
        let (left, mid, right) = (&self.text[..a], &self.text[a..b], &self.text[b..]);
        let alt = alt.unwrap_or(mid);
        let out = format!("{left}![{alt}]({url}){right}");
        // 选区落在 alt:跳过 `![` 两个字符
        let start = left.chars().count() + 2;
        let end = start + alt.chars().count();
        (out, start..end)
    }

    /// 复制选中内容:选区副本插到选区尾,光标落副本尾(编辑器惯例)。
    /// 无选中时回落复制当前行(VSCode Ctrl+D 同款语义)。
    fn duplicate_selection(self) -> (String, Range<usize>) {
        if self.caret_only() {
            return self.duplicate_line();
        }
        let (a, b) = (self.byte_of(self.start), self.byte_of(self.stop));
        let (left, mid, right) = (&self.text[..a], &self.text[a..b], &self.text[b..]);
        let cursor = left.chars().count() + mid.chars().count() * 2;
        (format!("{left}{mid}{mid}{right}"), cursor..cursor)
    }

    /// 复制当前行:光标/选区涉及的行整体复制一份插到下方(含行尾换行,
    /// 末行无换行则补一个让副本独立成行);光标落副本区同列。多行选区
    /// 按 VSCode 语义复制全部涉及行。
    fn duplicate_line(self) -> (String, Range<usize>) {
        let (a, b) = (self.byte_of(self.start), self.byte_of(self.stop));
        let line_start = self.text[..a].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_end = self
            .text
            .get(b..)
            .and_then(|rest| rest.find('\n').map(|i| b + i + 1))
            .unwrap_or(self.text.len());
        let block = &self.text[line_start..line_end];
        let tail_is_eol = block.ends_with('\n');
        let mut out = String::with_capacity(self.text.len() + block.len() + 1);
        out.push_str(&self.text[..line_end]);
        out.push_str(block);
        if !tail_is_eol {
            out.push('\n');
        }
        out.push_str(&self.text[line_end..]);
        let inserted = block.chars().count() + usize::from(!tail_is_eol);
        let cursor = self.stop + inserted;
        (out, cursor..cursor)
    }

    /// **行前缀类**:作用于选区覆盖的所有整行。已是该前缀 → 去掉
    /// (toggle);是别的前缀 → 替换为新前缀(不叠加);`prefix` 为空即
    /// 「正文」:一律去前缀。
    fn set_prefix(self, prefix: &'static str) -> (String, Range<usize>) {
        let (line_start, line_end) = self.selected_line_bytes();
        let (head, lines, tail) = (
            &self.text[..line_start],
            &self.text[line_start..line_end],
            &self.text[line_end..],
        );

        let mut out = String::with_capacity(self.text.len() + 64);
        out.push_str(head);
        for line in lines.split_inclusive('\n') {
            let body = line.strip_suffix('\n').unwrap_or(line);
            let eol = if body.len() == line.len() { "" } else { "\n" };
            let current = line_prefix_of(line);
            // toggle:已经是目标前缀就不再叠一次
            let next = if current == prefix { "" } else { prefix };
            out.push_str(next);
            out.push_str(strip_line_prefix(body));
            out.push_str(eol);
        }
        out.push_str(tail);

        let start = head.chars().count();
        // 新选区覆盖改动过的整段行,便于立刻再点别的格式
        let end = out.chars().count() - tail.chars().count();
        (out, start..end)
    }

    /// 任务列表三态:`- [ ] ` → `- [x] ` → `- ` → `- [ ] `。
    ///
    /// 每行各自判定自己的当前态,混选(有的已勾有的没勾)时整体推进一档。
    fn cycle_task(self) -> (String, Range<usize>) {
        let view = View {
            text: self.text,
            start: self.start,
            stop: self.stop,
        };
        let (line_start, line_end) = view.selected_line_bytes();
        let (head, lines, tail) = (
            &self.text[..line_start],
            &self.text[line_start..line_end],
            &self.text[line_end..],
        );

        let mut out = String::with_capacity(self.text.len() + 64);
        out.push_str(head);
        for line in lines.split_inclusive('\n') {
            let body = line.strip_suffix('\n').unwrap_or(line);
            let eol = if body.len() == line.len() { "" } else { "\n" };
            let next = match body {
                b if b.starts_with("- [ ] ") => ["- [x] ", &b["- [ ] ".len()..]].concat(),
                b if b.starts_with("- [x] ") => ["- ", &b["- [x] ".len()..]].concat(),
                b if b.starts_with("- ") => ["- [ ] ", &b["- ".len()..]].concat(),
                b => ["- [ ] ", strip_line_prefix(b)].concat(),
            };
            out.push_str(&next);
            out.push_str(eol);
        }
        out.push_str(tail);

        let start = head.chars().count();
        let end = out.chars().count() - tail.chars().count();
        (out, start..end)
    }

    /// **插入类**:在当前行**下方**插入块级模板。
    ///
    /// `open` / `close` 之间放选中内容 —— 代码块用它,分割线与表格的上半
    /// 为空。模板前补一个空行(除非已在行首),避免粘在上一行正文里。
    fn insert_block(self, open: &'static str, close: &'static str) -> (String, Range<usize>) {
        // 模板落在「当前行之后」,即行尾换行符的后头 —— 用行尾(不含换行)
        // 会让模板插进当前行中间。
        let line_end = self.current_line_end_inclusive();
        let (head, tail) = self.text.split_at(line_end);
        let (a, b) = (self.byte_of(self.start), self.byte_of(self.stop));
        let mid = &self.text[a..b];

        // 补行:块级元素必须与上文空一行,否则 `---` 会被上一行吃掉变成
        // setext 标题、代码块会粘在正文里。已在行首(head 空)或已有空行
        // 时不补 —— 后者是幂等的第二次插入。
        let gap = if head.is_empty() || head.ends_with("\n\n") {
            ""
        } else if head.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        let prefix = format!("{head}{gap}");
        let body_start = prefix.chars().count() + open.chars().count();
        let body_end = body_start + mid.chars().count();
        let out = format!("{prefix}{open}{mid}{close}{tail}");
        (out, body_start..body_end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold(text: &str, sel: Range<usize>) -> (String, Range<usize>) {
        apply(FormatAction::Bold, text, sel)
    }

    /// 按**字符**取选区覆盖的文字 —— 选区是字符偏移,CJK 下 `&out[sel]`
    /// 会切在多字节序列中间 panic。
    fn sel_text(out: &str, sel: Range<usize>) -> String {
        out.chars()
            .skip(sel.start)
            .take(sel.end - sel.start)
            .collect()
    }

    /// `**x**`:无选区插成对标记 + 光标居中;有选区包裹;再点去包裹。
    #[test]
    fn bold_inserts_wraps_and_unwraps() {
        let (out, sel) = bold("abc", 1..1);
        assert_eq!(out, "a****bc");
        assert_eq!(sel, 3..3, "光标落在两对标记中间");

        let (out, sel) = bold("abc", 0..3);
        assert_eq!(out, "**abc**");
        assert_eq!(sel, 2..5, "新选区覆盖包裹后的内容");

        // LP2-10:选中内容(不含标记)再点加粗也走语义化 toggle 拆标记,
        // 新选区 = 新文本中的内容区(定界符已剥,内容顶到构造起点)
        let (out, sel) = bold("**abc**", 2..5);
        assert_eq!(out, "abc", "选区落在加粗构造内 → 拆掉标记");
        assert_eq!(sel, 0..3, "新选区 = 内容区");
    }

    /// 并未被包裹时继续包裹:`**abc**` 里框住 `a` 再点加粗拆掉这层加粗
    /// (LP2-10 语义化 toggle,「再点一次取消」直觉成立);未配平的孤立
    /// 定界符不产 span,继续包裹。
    #[test]
    fn nested_selection_wraps_again() {
        // 选区含定界符(整段):同样拆这层,不叠外层
        let (out, sel) = bold("**abc**", 0..7);
        assert_eq!(out, "abc", "选区覆盖整段(含标记)→ 拆掉这层");
        assert_eq!(sel, 0..3, "新选区 = 内容区");
        // 未配平(`**` 只剩单侧):不产 span,继续包裹
        let (out, sel) = bold("**abc", 0..5);
        assert_eq!(out, "****abc**", "未配平构造走包裹");
        assert_eq!(sel, 2..7, "包裹路径的新选区覆盖内容+闭标记");
        // 已配平再点不再叠第二层:选区含构造即拆
        let (out, sel) = bold("**abc**", 2..5);
        assert_eq!(out, "abc", "已配平构造,选区在内 → 拆");
        assert_eq!(sel, 0..3);
    }

    /// CJK:全部走字符偏移,不在多字节序列中间切。
    #[test]
    fn multi_byte_offsets_are_char_based() {
        let (out, sel) = bold("你好世界", 1..3);
        assert_eq!(out, "你**好世**界");
        assert_eq!(sel, 3..5);

        let (out, sel) = apply(FormatAction::InlineCode, "中文 `代码` 混排", 4..6);
        assert_eq!(out, "中文 代码 混排", "选区在行内代码构造内 → 拆掉反引号");
        assert_eq!(sel, 3..5, "去包裹后选区仍是那两个字");
    }

    /// LP2-10 语义化 toggle 的判定面:部分选中拆整对;嵌套构造拆内层
    /// (本动作对应的那层);选区在构造外不动它(走包裹)。
    #[test]
    fn inline_toggle_partial_and_cross_construct() {
        // 部分选中:只选「代码」的一半,点行内代码图标 → 整对拆掉
        let (out, sel) = apply(FormatAction::InlineCode, "看 `代码` 尾", 3..4);
        assert_eq!(out, "看 代码 尾", "部分选中拆整对");
        assert_eq!(sel, 2..4, "新选区 = 新文本中的内容区");

        // 嵌套(粗体套代码):选区含两层定界符 → 拆本动作对应层(代码)
        let (out, sel) = apply(FormatAction::InlineCode, "**`x`**", 0..7);
        assert_eq!(out, "**x**", "拆代码层");
        assert_eq!(sel, 2..3);

        // 选区在构造外:不动,走包裹(选中「看」字,反引号只包它)
        let (out, sel) = apply(FormatAction::InlineCode, "看 `代码` 尾", 0..1);
        assert_eq!(out, "`看` `代码` 尾", "选区在构造外 → 包裹选中文字");
        assert_eq!(sel, 1..2);
    }

    /// 标题:作用于选区覆盖的所有整行;已是该前缀 → 去掉;是别的 → 替换。
    #[test]
    fn heading_applies_to_every_selected_line() {
        let (out, _) = apply(FormatAction::H2, "甲乙\n丙丁", 0..0);
        assert_eq!(out, "## 甲乙\n丙丁", "纯光标只作用于当前行");

        let (out, sel) = apply(FormatAction::H2, "甲乙\n丙丁", 1..4);
        assert_eq!(out, "## 甲乙\n## 丙丁", "跨行选区覆盖两行整行");
        assert_eq!(sel, 0..out.chars().count(), "新选区覆盖改动过的整段");

        let (out, _) = apply(FormatAction::H2, "## 甲乙", 0..0);
        assert_eq!(out, "甲乙", "已是 H2 → 去掉");

        let (out, _) = apply(FormatAction::H1, "## 甲乙", 0..0);
        assert_eq!(out, "# 甲乙", "H2 → H1 是替换不是叠加");

        let (out, _) = apply(FormatAction::Plain, "### 甲乙", 0..0);
        assert_eq!(out, "甲乙", "正文一律去前缀");
    }

    /// 无序列表点有序 → 换成 `1. `,不叠加成 `- 1. `。
    #[test]
    fn list_prefix_replaces_rather_than_stacks() {
        let (out, _) = apply(FormatAction::Ordered, "- 甲\n- 乙", 0..0);
        assert_eq!(out, "1. 甲\n- 乙", "只作用于光标所在行");

        let (out, _) = apply(FormatAction::Bullet, "1. 甲", 0..0);
        assert_eq!(out, "- 甲");
    }

    /// 任务三态:`- [ ] ` → `- [x] ` → `- ` → `- [ ] `。
    #[test]
    fn task_cycles_through_three_states() {
        let states = ["- [ ] 甲", "- [x] 甲", "- 甲"];
        let mut text = "- 甲".to_owned();
        for expected in states.iter().cycle().take(6) {
            let (out, sel) = apply(FormatAction::Task, &text, 0..0);
            assert_eq!(&out, expected, "从 {text:?} 推进一档");
            text = out;
            let _ = sel;
        }
    }

    /// 引用:toggle 语义同其它行前缀。
    #[test]
    fn quote_toggles() {
        let (out, _) = apply(FormatAction::Quote, "甲", 0..1);
        assert_eq!(out, "> 甲");
        let (out, _) = apply(FormatAction::Quote, "> 甲", 0..3);
        assert_eq!(out, "甲");
    }

    /// 插入类:代码块包住选区;分割线与表格插在当前行下方。
    #[test]
    fn inserts_land_below_the_current_line() {
        let (out, sel) = apply(FormatAction::CodeBlock, "甲\n乙丙", 0..0);
        assert_eq!(out, "甲\n\n```\n\n```\n乙丙");
        let body = &out[sel.clone()];
        assert_eq!(body, "", "代码块 info 位为空且无选区内容");

        let (out, _) = apply(FormatAction::CodeBlock, "SELECT 1", 0..8);
        assert!(out.contains("```\nSELECT 1\n```\n"), "{out}");

        let (out, _) = apply(FormatAction::Divider, "甲", 0..0);
        assert_eq!(out, "甲\n\n---\n");

        let (out, _) = apply(FormatAction::Table, "甲", 0..0);
        assert!(out.ends_with(TABLE_SKELETON), "{out}");
    }

    /// 链接:选区当标题,新选区落在 url 占位上以便直接覆写。
    #[test]
    fn link_uses_selection_as_title_and_selects_url() {
        let (out, sel) = apply(FormatAction::Link, "甲乙", 0..2);
        assert_eq!(out, "[甲乙](https://)");
        let selected: String = out
            .chars()
            .skip(sel.start)
            .take(sel.end - sel.start)
            .collect();
        assert_eq!(selected, "https://", "url 占位被选中");

        let (out, sel) = apply(FormatAction::Link, "", 0..0);
        assert_eq!(out, "[](https://)");
        let selected: String = out
            .chars()
            .skip(sel.start)
            .take(sel.end - sel.start)
            .collect();
        assert_eq!(selected, "https://");
    }

    /// 图片(`insert_image`):空文档 / 有选区 / 行内 / 行尾四种情形,新选区
    /// 一律落在 alt 上(alt 是给人读的,比 url 更需要立刻覆写)。
    #[test]
    fn image_inserts_with_selection_on_alt() {
        // 空文档:整段凭空出现,alt 被选中
        let (out, sel) = insert_image("", 0..0, "https://x/y.png", "示意图");
        assert_eq!(out, "![示意图](https://x/y.png)");
        assert_eq!(sel_text(&out, sel), "示意图");

        // 有选区:选中文字被替换(alt 由调用方给定,不取自选区)
        let (out, sel) = insert_image("甲乙丙", 0..3, "u", "图");
        assert_eq!(out, "![图](u)");
        assert_eq!(sel_text(&out, sel), "图");

        // 行内:两侧正文都不动(插入不凭空造空格)
        let (out, sel) = insert_image("ab cd", 3..3, "u", "x");
        assert_eq!(out, "ab ![x](u)cd");
        assert_eq!(sel_text(&out, sel.clone()), "x");
        assert_eq!(sel.start, 5, "跳过 `![` 两个字符");

        // 行尾(CJK):末尾追加,偏移按字符
        let (out, sel) = insert_image("中文行", 3..3, "u", "图");
        assert_eq!(out, "中文行![图](u)");
        assert_eq!(sel_text(&out, sel), "图");
    }

    /// 工具条/快捷键直触(不经对话框)的骨架:`![选中](https://)`。
    ///
    /// 与 `Link` 一样留 url 占位待覆写;alt 取选中文字 —— 框住一段话再点图
    /// 片,那句话就该是图片说明。
    #[test]
    fn image_action_inserts_a_skeleton_using_selection_as_alt() {
        let (out, sel) = apply(FormatAction::Image, "甲乙", 0..2);
        assert_eq!(out, "![甲乙](https://)");
        assert_eq!(sel_text(&out, sel), "甲乙", "新选区落在 alt(选中文字)");

        let (out, sel) = apply(FormatAction::Image, "", 0..0);
        assert_eq!(out, "![](https://)");
        assert_eq!(sel, 2..2, "无选区时光标停在 alt 开头等着填");
    }

    /// emoji 插入(`insert_emoji`):空文档 / 有选区 / 行内 / 行尾(CJK
    /// 混排)四种情形(docs/emoji-plan.md §8)—— 选中内容被替换,新选区
    /// collapsed 落在 emoji 之后;偏移全按字符,不在多字节序列中间切。
    #[test]
    fn emoji_inserts_collapse_caret_after_the_glyph() {
        // 空文档:整串凭空出现,光标在其后
        let (out, sel) = insert_emoji("", 0..0, "🚀");
        assert_eq!(out, "🚀");
        assert_eq!(sel, 1..1, "光标落在 emoji 之后");

        // 有选区:选中文字被替换(不是保留在旁)
        let (out, sel) = insert_emoji("甲乙丙", 0..3, "🚀");
        assert_eq!(out, "🚀");
        assert_eq!(sel, 1..1);

        // 行内:两侧正文不动,光标按字符落位(CJK 语境下非 BMP 同样安全)
        let (out, sel) = insert_emoji("中文text", 2..2, "😀");
        assert_eq!(out, "中文😀text");
        assert_eq!(sel, 3..3);

        // 行尾(CJK 混排 + 双码位旗帜):光标跨过两枚旗面字符
        let (out, sel) = insert_emoji("中文行尾", 4..4, "🇨🇳");
        assert_eq!(out, "中文行尾🇨🇳");
        assert_eq!(sel, 6..6, "旗帜是两个码位,光标落在整面旗之后");

        // 防御:越界选区按端点收敛,不 panic(UI 侧选区可能来自过期快照);
        // 空载荷是归约侧的兜底分支,这里同样自洽
        let (out, sel) = insert_emoji("甲乙", 99..99, "🚀");
        assert_eq!(out, "甲乙🚀");
        assert_eq!(sel, 3..3);
        let (out, sel) = insert_emoji("甲乙", 0..0, "");
        assert_eq!(out, "甲乙");
        assert_eq!(sel, 0..0);
    }

    /// 空文本 / 越界选区不 panic,且结果自洽(防御:UI 侧选区可能来自过期快照)。
    #[test]
    fn degenerate_inputs_do_not_panic() {
        for action in FormatAction::ALL {
            let (out, sel) = apply(action, "", 0..0);
            assert!(sel.start <= sel.end, "{action:?}: {sel:?}");
            assert!(sel.end <= out.chars().count(), "{action:?}: {sel:?} 越界");

            let (out2, sel2) = apply(action, "甲乙", 99..99);
            assert!(sel2.start <= sel2.end && sel2.end <= out2.chars().count());
        }
    }

    /// 任务列表 CJK 回归(2026-09-27 用户实测崩溃的语义层覆盖):纯中文行
    /// 三态循环各两次、空选区、跨多行含 CJK、行中光标(非行首)。全链路
    /// (真实按钮 + 归约 + 状态栏)的对应回归在 `ui::layout` 测试。
    #[test]
    fn task_on_pure_cjk_in_three_selection_shapes() {
        // 纯中文行,行中光标:三态循环两整圈(六次)后回到起点
        let mut text = "- 甲乙丙".to_owned();
        for expected in ["- [ ] 甲乙丙", "- [x] 甲乙丙", "- 甲乙丙"]
            .iter()
            .cycle()
            .take(6)
        {
            let (out, _) = apply(FormatAction::Task, &text, 3..3);
            assert_eq!(&out, expected, "从 {text:?} 推进一档");
            text = out;
        }

        // 跨多行含 CJK:覆盖的整行各自推进,选区不越界
        let (out, sel) = apply(FormatAction::Task, "甲乙\n丙丁戊\n己", 2..6);
        assert_eq!(out, "- [ ] 甲乙\n- [ ] 丙丁戊\n己");
        assert!(sel.end <= out.chars().count(), "{sel:?} 越界");

        // 空文本:无行可作用,不 panic 也不产文(行类动作的既有语义)
        let (out, _) = apply(FormatAction::Task, "", 0..0);
        assert_eq!(out, "");
    }

    /// 分组切片与 `ALL` 对齐:`actions()` 是 `ALL` 的**连续分区**,既不重
    /// 复也不遗漏。曾并存一张反向的 `FormatAction::group()`,但它除了重复
    /// 这张表、还两面漂移的可能之外再无用处,已删。
    #[test]
    fn group_slices_partition_all() {
        let flat: Vec<FormatAction> = FormatGroup::ALL
            .iter()
            .flat_map(|group| group.actions().iter().copied())
            .collect();
        assert_eq!(flat, FormatAction::ALL.to_vec());
    }

    #[test]
    fn duplicate_selection_inserts_copy_after_selection() {
        let (text, sel) = apply(FormatAction::DuplicateSelection, "甲乙丙", 1..2);
        assert_eq!(text, "甲乙乙丙");
        assert_eq!(sel, 3..3);
    }

    #[test]
    fn duplicate_line_handles_cjk_and_final_line() {
        let (text, sel) = apply(FormatAction::DuplicateLine, "甲行\n乙行", 1..1);
        assert_eq!(text, "甲行\n甲行\n乙行");
        assert_eq!(sel, 4..4);
    }
}
