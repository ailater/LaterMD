//! 本地文档链接:普通 Markdown 链接指向**本地文档**时的解析与跳转。
//!
//! 与 `ai_link`(`ai://`)、`latermd_md::WIKI_SCHEME`(`[[wikilink]]`)并列的
//! 第三类链接入口:`[标题](./note.md)`、`[标题](doc.md#锚点)`、`[标题](#锚点)`
//! 这类写法在点击时不应交给系统浏览器(相对路径交给浏览器只会失败),而应
//! 解析到本地文档库中的文档并在应用内打开。解析是**点击时**对本地文档集合
//! 的直取与扫描(当前文档目录 → 库根 → 全库按名),没有常驻索引 —— 与
//! `[[wikilink]]` 的 `filetree::find_by_name` 同一成本口径,编辑器场景文档
//! 量级下扫描是毫秒级。
//!
//! 锚点匹配与 TOC 生成(`latermd_md::generate_toc`)同一条 slug 规则:应用
//! 自己生成的 `[标题](#锚点)` 目录链接必须能点回对应标题,这是本模块的
//! 一等公民场景,不是附加能力。
//!
//! 纯逻辑模块,不 import egui;UI 侧(live/preview 的 handler)只做分类
//! 与消息投递,归约在 `state::open_doc_link`。

use std::path::{Path, PathBuf};

use latermd_md::{heading_slug, outline, Slugger};

/// 应用内协议前缀(与 `ui::ai_card::is_openable_href` 的排除名单同一份,
/// 单一事实来源在这里):它们的 click 各有归属,AI 流/开文档/字形/
/// checkbox/底色,不该被本地链接解析或浏览器路径碰。
const APP_SCHEMES: [&str; 5] = [
    crate::ai_link::SCHEME,
    latermd_md::WIKI_SCHEME,
    latermd_md::EMOJI_SCHEME,
    latermd_md::TASK_SCHEME,
    latermd_md::HIGHLIGHT_SCHEME,
];

/// href 的三分归类:点击路径据此分流。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HrefKind {
    /// 应用内协议(`ai://` `wiki://` `emoji://` `task://` `hl://`):handler
    /// 自己消费,本模块不碰。
    AppScheme,
    /// 带外部 scheme(`http(s)` `mailto` `ftp` …):交系统浏览器,vendored
    /// 默认行为不变。
    External,
    /// 本地文档链接:相对/绝对路径、`file://` URI、纯锚点 —— 应用内跳转。
    Local,
}

/// 分类一个 href(先 trim)。判定顺序:应用内协议 → Windows 盘符 → 通用
/// scheme → 其余一律 Local(无 scheme 的路径与 `#锚点` 是常态)。
///
/// 盘符必须在通用 scheme 之前:`C:\docs\a.md` 的 `C:` 形似 scheme,单字母
/// 后跟 `\` 或 `/` 分隔的按盘符路径处理;真正的单字母 scheme(`c:x` 无分隔)
/// 极罕见,归 External 无害。
pub fn classify(href: &str) -> HrefKind {
    let href = href.trim();
    if APP_SCHEMES.iter().any(|scheme| href.starts_with(scheme)) {
        return HrefKind::AppScheme;
    }
    let bytes = href.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return HrefKind::Local;
    }
    if let Some(colon) = scheme_colon(href) {
        // file:// 是本地路径的 URI 外衣(点击语义仍是应用内打开),不算外部
        return if href[..colon].eq_ignore_ascii_case("file") {
            HrefKind::Local
        } else {
            HrefKind::External
        };
    }
    HrefKind::Local
}

/// href 是否带 URL scheme(`letter [alnum+.-]* ':'`),有则返回冒号字节位。
fn scheme_colon(href: &str) -> Option<usize> {
    let bytes = href.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    for (index, byte) in bytes.iter().enumerate().skip(1) {
        if *byte == b':' {
            return Some(index);
        }
        if !(byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.')) {
            return None;
        }
    }
    None
}

/// 拆开后的本地链接:路径部分(可为空 = 纯锚点)与锚点。
///
/// 只按**第一个** `#` 直拆;路径里的字面 `#` 应写作 `%23`(与浏览器同口径),
/// 不做反向猜测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocLink {
    /// 路径部分,原样保留(trim 过);空串 = 纯锚点(当前文档内跳)。
    pub path: String,
    /// `#` 后的锚点,原样保留;`None` = 不带锚点。
    pub anchor: Option<String>,
}

/// 把 Local href 拆成 [`DocLink`]。对任意输入都有定义(拆不动就是无锚点
/// 的路径),调用方不必先分类再解析。
pub fn parse_local(href: &str) -> DocLink {
    let href = href.trim();
    match href.split_once('#') {
        Some((path, anchor)) => DocLink {
            path: path.trim().to_owned(),
            anchor: (!anchor.is_empty()).then(|| anchor.to_owned()),
        },
        None => DocLink {
            path: href.to_owned(),
            anchor: None,
        },
    }
}

/// 解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// 已定位到盘上文档;`anchor` 交给归约侧在打开后定位标题。
    Found {
        path: PathBuf,
        anchor: Option<String>,
    },
    /// 纯锚点:在**当前**文档内跳,不开新标签。
    AnchorHere { anchor: String },
    /// 解析失败,载荷为面向用户的提示文案(与 wikilink 找不到同一纪律:
    /// 落提示行,不静默无反应)。
    Miss(String),
}

/// 解析 [`DocLink`] 到本地文档。
///
/// 路径候选按序试:**原样** → 百分号解码(浏览器习惯把空格写成 `%20`,
/// pulldown-cmark 不解码,原文命中失败时解码补位);`file://` URI 整体视为
/// 绝对路径(只认解码后的路径部分)。每个候选字符串依次拼这些基底:
///
/// 1. **当前文档目录**(Markdown 链接的标准语义);
/// 2. **文档库根**(库根相对写法,如 `docs/note.md`);
/// 3. 候选自身有根(绝对路径)时 `Path::join` 自动落回候选本身,无需特判。
///
/// 每个拼出的路径试「原样 → 补 `.md` → 补 `.markdown`」三形态,须真实存在
/// 且是 Markdown(`latermd_search::is_markdown`,与文件树/搜索同口径)。
/// 全部落空且候选不含路径分隔符时,退 `filetree::find_by_name` 全库按名
/// 找(`[标题](笔记名)` 的宽容写法,与 `[[wikilink]]` 同通道)。
pub fn resolve(current_doc: Option<&Path>, root: Option<&Path>, link: &DocLink) -> Resolution {
    let anchor = link
        .anchor
        .as_deref()
        .map(str::trim)
        .filter(|anchor| !anchor.is_empty())
        .map(str::to_owned);
    let raw = link.path.trim();
    if raw.is_empty() {
        return match anchor {
            Some(anchor) => Resolution::AnchorHere { anchor },
            None => Resolution::Miss("链接地址为空".to_owned()),
        };
    }

    let mut candidates = vec![raw.to_owned()];
    if let Some(rest) = raw.strip_prefix("file://") {
        // file:///C:/…(Windows file URL)的盘符前多一道斜杠,剥掉
        let decoded = percent_decode(rest);
        candidates = vec![strip_slash_before_drive(&decoded)];
    } else {
        let decoded = percent_decode(raw);
        if decoded != raw {
            candidates.push(decoded);
        }
    }

    let mut bases: Vec<Option<&Path>> = Vec::new();
    if let Some(doc) = current_doc {
        bases.push(doc.parent());
    }
    if let Some(root) = root {
        if bases.last() != Some(&Some(root)) {
            bases.push(Some(root));
        }
    }
    if candidates.iter().any(|c| Path::new(c).has_root()) {
        bases.push(None);
    }

    for base in &bases {
        for candidate in &candidates {
            let joined = match base {
                Some(base) => base.join(candidate),
                None => PathBuf::from(candidate),
            };
            for tried in [
                joined.clone(),
                with_extension_appended(&joined, "md"),
                with_extension_appended(&joined, "markdown"),
            ] {
                if tried.is_file() && latermd_search::is_markdown(&tried) {
                    // 命中即 canonicalize:`./`、`../` 拼出来的路径要落成
                    // 规整形态,已开标签的路径去重(`tabs::find_by_path`)才
                    // 认得出 —— 不规整会在点击时开出第二个标签。局限:文档
                    // 库本身在符号链接之后时,canonical 结果与文件树展示的
                    // 链接路径可能不同形,去重会漏;属罕见部署,不为它把
                    // canonical 键位引进共享的 find_by_path。
                    return Resolution::Found {
                        path: canonical_found(&tried),
                        anchor,
                    };
                }
            }
        }
    }

    // 全库按名兜底:与 `[[wikilink]]` 同一解析通道(忽略大小写、扩展名
    // 容错);带路径分隔符的候选已在前两轮按路径直取过,不重复。
    if let Some(root) = root {
        if !raw.contains('/') && !raw.contains('\\') {
            for candidate in &candidates {
                if let Some(path) = crate::filetree::find_by_name(root, candidate) {
                    return Resolution::Found { path, anchor };
                }
            }
        }
    }

    Resolution::Miss(format!("链接指向的文档不存在:{raw}"))
}

/// 在文档文本里按锚点找标题,返回标题条目的源码 span(光标跳转用它,
/// `state::jump_cursor_to_heading` 会剥掉 span 吸收的前导换行)。
///
/// 两条匹配规则,命中先后按文档序:
///
/// 1. **slug 全等**:与 TOC 生成同一 [`Slugger`] 序贯分配(重复标题
///    `x` → `x-1` 的去重后缀因此对得上),锚点侧同走 [`heading_slug`]
///    (比较天然大小写不敏感,CJK 原样保留);
/// 2. **标题纯文本忽略大小写全等**:手写锚点常直接抄标题原文,
///    `[安装](#安装指南)` 不该因 slug 规则记不清而失败。
pub fn find_anchor(text: &str, anchor: &str) -> Option<std::ops::Range<usize>> {
    let wanted = anchor.trim();
    if wanted.is_empty() {
        return None;
    }
    let wanted_slug = heading_slug(wanted);
    let wanted_lower = wanted.to_lowercase();
    let mut slugger = Slugger::new();
    for item in outline(text) {
        let slug = slugger.slug(&item.text);
        let plain = item.text.trim().to_lowercase();
        if slug == wanted_slug || plain == wanted_lower {
            return Some(item.span);
        }
    }
    None
}

/// 百分号解码(失败/含非法序列时按 UTF-8 lossy 回退,绝不让坏字节进路径)。
fn percent_decode(text: &str) -> String {
    percent_encoding::percent_decode_str(text)
        .decode_utf8_lossy()
        .into_owned()
}

/// `file:///C:/…` 形态剥掉盘符前的斜杠(Windows file URL 惯例)。
fn strip_slash_before_drive(path: &str) -> String {
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        return path[1..].to_owned();
    }
    path.to_owned()
}

/// 命中路径的规整:canonicalize 抹掉 `./`、`../`(开标签的路径去重认规整
/// 路径);失败(竞态删除)退原路径。Windows 的 canonicalize 会带 `\\?\`
/// verbatim 前缀,剥掉盘符形态(UNC 的 `\\?\UNC\` 罕见,保留原样)。
fn canonical_found(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(canonical) => {
            let text = canonical.to_string_lossy();
            if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
                PathBuf::from(format!(r"\\{rest}"))
            } else if let Some(rest) = text.strip_prefix(r"\\?\") {
                PathBuf::from(rest)
            } else {
                canonical
            }
        }
        Err(_) => path.to_path_buf(),
    }
}

/// 在文件名后追加扩展名(`a` → `a.md`;已有扩展名的 `a.txt` → `a.txt.md`,
/// 会被随后的 `is_markdown` 拒掉,与 `filetree::find_by_name` 的路径直取
/// 同口径)。
fn with_extension_appended(path: &Path, ext: &str) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".");
    os.push(ext);
    PathBuf::from(os)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("latermd-doclink-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // canonicalize 落地:resolve 命中路径会 canonicalize,macOS 的 /tmp
        // 在 /private 符号链接之后,期望基线若还是 /tmp 形态逐路径相等断言
        // 必错(Linux 无符号链接,canonicalize 是恒等);Windows 下本套测试
        // 不进 CI 矩阵。
        fs::canonicalize(&dir).unwrap()
    }

    // —— classify ——

    #[test]
    fn classify_sorts_external_app_and_local() {
        assert_eq!(classify("https://example.com/a"), HrefKind::External);
        assert_eq!(classify("http://example.com"), HrefKind::External);
        assert_eq!(classify("mailto:a@b.c"), HrefKind::External);
        assert_eq!(classify("ftp://x/y"), HrefKind::External);
        for scheme in APP_SCHEMES {
            assert_eq!(classify(&format!("{scheme}0")), HrefKind::AppScheme);
        }
        for local in [
            "note.md",
            "./sub/note.md",
            "../up/note.md",
            "/abs/note.md",
            "#锚点",
            "file:///tmp/note.md",
            "C:\\docs\\note.md",
            "C:/docs/note.md",
            "my%20note.md",
        ] {
            assert_eq!(classify(local), HrefKind::Local, "{local}");
        }
    }

    #[test]
    fn classify_single_letter_without_separator_is_external() {
        // 单字母无分隔:形似 scheme,不冒认盘符
        assert_eq!(classify("c:foo"), HrefKind::External);
    }

    // —— parse_local ——

    #[test]
    fn parse_local_splits_path_and_anchor() {
        assert_eq!(
            parse_local("doc.md#安装"),
            DocLink {
                path: "doc.md".into(),
                anchor: Some("安装".into())
            }
        );
        assert_eq!(
            parse_local("#only"),
            DocLink {
                path: String::new(),
                anchor: Some("only".into())
            }
        );
        assert_eq!(
            parse_local(" ./a b.md "),
            DocLink {
                path: "./a b.md".into(),
                anchor: None
            }
        );
        // 尾随 # 视为无锚点
        assert_eq!(
            parse_local("doc.md#"),
            DocLink {
                path: "doc.md".into(),
                anchor: None
            }
        );
    }

    // —— resolve ——

    fn link(path: &str, anchor: Option<&str>) -> DocLink {
        DocLink {
            path: path.to_owned(),
            anchor: anchor.map(str::to_owned),
        }
    }

    #[test]
    fn resolve_prefers_document_relative_then_root() {
        let dir = temp_dir("rel-root");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/note.md"), "# N\n").unwrap();
        fs::write(dir.join("top.md"), "# T\n").unwrap();
        let doc = dir.join("sub/doc.md");

        // 文档相对直取
        assert_eq!(
            resolve(Some(&doc), Some(&dir), &link("note.md", None)),
            Resolution::Found {
                path: dir.join("sub/note.md"),
                anchor: None
            }
        );
        // `./` 规整:开标签去重要认规整路径
        assert_eq!(
            resolve(Some(&doc), Some(&dir), &link("./note.md", None)),
            Resolution::Found {
                path: dir.join("sub/note.md"),
                anchor: None
            }
        );
        // `../` 逃出子目录
        assert_eq!(
            resolve(Some(&doc), Some(&dir), &link("../top.md", None)),
            Resolution::Found {
                path: dir.join("top.md"),
                anchor: None
            }
        );
        // 文档目录没有、库根有:根相对兜底(带分隔符,按路径直取)
        assert_eq!(
            resolve(Some(&doc), Some(&dir), &link("top.md", None)),
            Resolution::Found {
                path: dir.join("top.md"),
                anchor: None
            }
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_tolerates_extensions_and_percent_and_file_uri() {
        let dir = temp_dir("tolerance");
        fs::write(dir.join("my note.md"), "# N\n").unwrap();
        fs::write(dir.join("plain.markdown"), "# P\n").unwrap();
        let doc = dir.join("doc.md");

        // 扩展名补齐:不带后缀也能命中 .md / .markdown
        assert!(matches!(
            resolve(Some(&doc), Some(&dir), &link("my note", None)),
            Resolution::Found { .. }
        ));
        assert!(matches!(
            resolve(Some(&doc), Some(&dir), &link("plain.md", None)),
            Resolution::Found { .. }
        ));
        // 百分号解码补位(%20 → 空格)
        assert!(matches!(
            resolve(Some(&doc), Some(&dir), &link("my%20note.md", None)),
            Resolution::Found { .. }
        ));
        // file:// 绝对 URI
        let uri = format!("file://{}", percent_encode_path(&dir.join("my note.md")));
        assert!(matches!(
            resolve(Some(&doc), Some(&dir), &link(&uri, None)),
            Resolution::Found { .. }
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    /// 最小 file URL 编码(空格 → %20),测试专用。
    fn percent_encode_path(path: &Path) -> String {
        path.to_string_lossy()
            .chars()
            .map(|c| {
                if c == ' ' {
                    "%20".to_owned()
                } else {
                    c.to_string()
                }
            })
            .collect()
    }

    #[test]
    fn resolve_falls_back_to_name_search_without_separator() {
        let dir = temp_dir("name-fallback");
        fs::create_dir_all(dir.join("deep/nested")).unwrap();
        fs::write(dir.join("deep/nested/隔离笔记.md"), "# I\n").unwrap();
        let doc = dir.join("doc.md");

        // 文档目录与库根直取都没有,按名全库找(忽略大小写)
        assert!(matches!(
            resolve(Some(&doc), Some(&dir), &link("隔离笔记", None)),
            Resolution::Found { .. }
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_anchors_and_misses() {
        let dir = temp_dir("anchor-miss");
        fs::write(dir.join("doc.md"), "# D\n").unwrap();
        let doc = dir.join("doc.md");

        // 纯锚点:当前文档内跳(路径为空才是锚点语义)
        let anchor_only = DocLink {
            path: String::new(),
            anchor: Some("安装".into()),
        };
        assert_eq!(
            resolve(Some(&doc), Some(&dir), &anchor_only),
            Resolution::AnchorHere {
                anchor: "安装".into()
            }
        );
        // 空地址:落提示,不静默
        assert_eq!(
            resolve(Some(&doc), Some(&dir), &link("", None)),
            Resolution::Miss("链接地址为空".to_owned())
        );
        // 找不到:提示带原文路径
        match resolve(Some(&doc), Some(&dir), &link("missing.md", None)) {
            Resolution::Miss(message) => assert!(message.contains("missing.md")),
            other => panic!("应为 Miss:{other:?}"),
        }
        // 非文档扩展名不冒认(存在但不是 .md)
        fs::write(dir.join("pic.png"), b"png").unwrap();
        assert!(matches!(
            resolve(Some(&doc), Some(&dir), &link("pic.png", None)),
            Resolution::Miss(_)
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_without_base_direct_takes_absolute() {
        let dir = temp_dir("absolute");
        fs::write(dir.join("abs.md"), "# A\n").unwrap();
        // 无文档无根:绝对路径仍可直取
        assert_eq!(
            resolve(
                None,
                None,
                &link(dir.join("abs.md").to_str().unwrap(), None)
            ),
            Resolution::Found {
                path: dir.join("abs.md"),
                anchor: None
            }
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // —— find_anchor ——

    #[test]
    fn find_anchor_matches_slug_plain_and_dedup() {
        let text = "# 安装指南\n\n正文\n\n## 重复\n\n甲\n\n## 重复\n\n乙\n\n## My Head\n\n丙\n";
        // 标题 span 会吸收前一块尾部的换行,绝对字节起点随解析器口径漂移;
        // 断言用次序与包含性,不钉具体数字。
        let start = |anchor: &str| find_anchor(text, anchor).map(|span| span.start);
        let (intro, dup1, dup2, latin) = (
            start("安装指南").unwrap(),
            start("重复").unwrap(),
            start("重复-1").unwrap(),
            start("My Head").unwrap(),
        );
        assert!(
            intro < dup1 && dup1 < dup2 && dup2 < latin,
            "四个锚点按文档序"
        );
        // intro 从文首起:span 语义直验,标题行原样开头
        assert!(text[intro..].starts_with("# 安装指南"));
        // CJK slug(GitHub 风格保留 CJK)与 TOC 生成同规则;拉丁标题大小写
        // 不敏感,手抄原文也可
        assert_eq!(find_anchor(text, "my-head").map(|s| s.start), Some(latin));
        assert_eq!(find_anchor(text, "my head").map(|s| s.start), Some(latin));
        assert_eq!(start("没有的标题"), None);
        assert_eq!(find_anchor(text, "  "), None, "空锚点不匹配");
    }

    /// 含行内代码的标题按合并后的全文取 slug 命中(与 TOC/导出 HTML 同
    /// 源口径;拆条时代这条链接在应用内永远解析不到)。
    #[test]
    fn find_anchor_heading_with_inline_code() {
        let text = "# 标题含 `code` 内联\n\n正文\n";
        let span = find_anchor(text, "标题含-code-内联").expect("全文 slug 命中");
        assert!(text[span].starts_with("# 标题含"));
        // 手抄标题原文(忽略行内代码记号)也能命中
        assert!(find_anchor(text, "标题含 code 内联").is_some());
        // 旧的拆条片段 slug 不再产出
        assert_eq!(find_anchor(text, "标题含"), None);
    }
}
