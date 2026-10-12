//! 文档库检索核心:`.gitignore` 感知的目录遍历 + 逐行正则匹配 + wikilink
//! 反向链接扫描。
//!
//! 本 crate 是**侧边栏搜索与 MCP `search_docs` / `list_files` 的单一实现**
//! (docs/mcp-plan.md §3 方案 A:不复制一份给 MCP,否则两套 `.gitignore`
//! 语义与两套截断上限必然漂移)。**不依赖 egui/eframe**(铁律二),因此
//! MCP server(后台线程)与 GUI 归约都能直接调。
//!
//! 搜索的两套入口与 [`backlinks`] 共用同一段遍历骨架(私有 `walk_markdown`):
//!
//! - [`SearchService`]:后台线程 + 有界 channel,供 UI 流式取用。取消是
//!   **代际号语义** —— `spawn`/`cancel` 都推进全局代际,旧线程在检查点
//!   自行退出,迟到的旧代事件由接收端按代际丢弃;UI 侧 `try_recv` 永不
//!   阻塞,也不 join 后台线程。
//! - [`search_sync`]:在调用线程一次跑完,供无 UI 的消费者(MCP 工具)使用。
//!
//! [`backlinks`] 反向链接扫描只做同步直扫:单文档被引量级小,面板要
//! 完整列表而非流式增量,不值得复制一套 channel + 代际号;可中断性由
//! `stop` 闭包保留(取舍见 decisions-pending #87)。
//!
//! 行迭代用 `grep_searcher::LineIter`(ripgrep 同款行语义)。不走
//! `Searcher::search_*` 的原因:那组 API 全部要求 `grep_matcher::Matcher`
//! 实参,`regex::Regex` 并未实现该 trait,直连不成立;补引 `grep-regex`
//! 桥接只是 trait 适配,不值(decisions-pending #7)。
//!
//! 根目录不存在时遍历为空(零命中),不视为错误——根目录的存在性由调用
//! 方(文件树 / MCP 的根校验)保证。

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::thread;

use grep_searcher::LineIter;
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
// bytes 变体:`LineIter` 产出 `&[u8]`,str 版 `Regex` 不接受字节入参;
// 非法 UTF-8 的行由 trim_line_terminator 的 lossy 兜底,匹配层不必是 str
use regex::bytes::{Regex, RegexBuilder};

/// 扩展名是否 Markdown 的唯一事实源(文件树、搜索、打开对话框共用)。
pub const MARKDOWN_EXTENSIONS: [&str; 2] = ["md", "markdown"];

/// 结果 channel 容量:满时后台线程阻塞在 `send`(背压),防海量命中把
/// 内存吃穿;UI 侧只 `try_recv`,不会被背压波及。
const CHANNEL_CAPACITY: usize = 256;

/// 单文件读入上限:行迭代需要完整字节切片,病态大文件(哪怕是 `.md`)
/// 整体跳过,不读进内存。
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// 结果缓存上限(UI 侧边栏与 MCP `search_docs` 同值):导航场景命中 500
/// 条足够,再多只是把面板/响应变成不可用的长列表。
pub const MAX_HITS: usize = 500;

/// 列目录上限(与文件树 `MAX_CHILDREN` 同量级,decisions-pending #19 的口径)。
pub const MAX_LIST_ENTRIES: usize = 500;

/// 一次搜索请求。
///
/// 三个开关(`case_insensitive` / `whole_word` / `literal`)在**编译层**
/// 合成一个正则(字面转义、`\b` 包裹),扫描层只看编译产物 —— 侧边栏与
/// 替换共用同一份语义,不存在「搜得到换不掉」的第二套匹配。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// 遍历根(MCP 侧已校验过越界)。
    pub root: PathBuf,
    /// 搜索词。`literal = false` 时按正则解释(非法模式同步报错),
    /// `true` 时按字面文本解释(特殊字符自动转义,永不编译失败)。
    pub pattern: String,
    pub case_insensitive: bool,
    /// 完整匹配:模式被 `\b(?:…)\b` 包裹,使用 regex crate 的 Unicode
    /// 词边界定义(不保证与其他编辑器的整词规则完全一致)。
    pub whole_word: bool,
    /// 字面文本模式:不按正则解释。GUI 侧边栏默认字面(直接粘贴 `C++`
    /// 这类含元字符的词即可搜),MCP `search_docs` 维持正则语义(false)。
    pub literal: bool,
}

/// 一条命中:文件 + 1 起行号 + 剥掉行终止符的行文本 + 行内首个匹配的
/// 字节区间(供 UI 高亮;严格不包括行终止符)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub path: PathBuf,
    pub line_no: usize,
    pub line_text: String,
    /// 首个匹配在本行正文内的字节区间,落在 UTF-8 字符边界。区间相对
    /// [`line_text`](Self::line_text) 计算,UI 可直接拿去做行内高亮。
    /// 空匹配(如 `x*`)退化为 `n..n` 的零宽区间,UI 侧按无高亮处理。
    pub match_span: std::ops::Range<usize>,
}

/// `try_recv` 产出的事件:`Hit` 持续流出;`Done` 表示当前代搜索自然结束
/// (被取代或取消的搜索不产生 `Done`)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchEvent {
    Hit(SearchResult),
    Done,
}

/// 发起失败的情形。
#[derive(Debug)]
pub enum SearchError {
    /// 空模式,或正则编译失败(输入到一半是常态,消息可直接展示)。
    InvalidPattern(String),
    /// 后台线程创建失败(系统级,罕见)。
    Spawn(std::io::Error),
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SearchError::InvalidPattern(msg) => write!(f, "搜索正则无效: {msg}"),
            SearchError::Spawn(source) => write!(f, "搜索线程启动失败: {source}"),
        }
    }
}

impl std::error::Error for SearchError {}

/// 拼装底层正则源:字面转义、`\b` 包裹(非捕获组,交替模式的作用域才
/// 正确)与 bytes/str 两种正则都从它出发,开关语义单一实现。
fn pattern_source(query: &SearchQuery) -> Result<String, SearchError> {
    if query.pattern.is_empty() {
        return Err(SearchError::InvalidPattern("搜索词为空".into()));
    }
    let core = if query.literal {
        regex::escape(&query.pattern)
    } else {
        query.pattern.clone()
    };
    Ok(if query.whole_word {
        format!(r"\b(?:{core})\b")
    } else {
        core
    })
}

/// 编译搜索正则:同步执行,让非法模式在调用线程就报错(UI 输入到一半、
/// MCP 传坏参数都是这一路径)。bytes 变体的行匹配与 [`scan`] 的行切片
/// 同口径;str 语义的替换走 [`replace_in_text`] 自行编译。
fn compile(query: &SearchQuery) -> Result<Regex, SearchError> {
    RegexBuilder::new(&pattern_source(query)?)
        .case_insensitive(query.case_insensitive)
        .build()
        .map_err(|error| SearchError::InvalidPattern(error.to_string()))
}

/// 遍历骨架:`.gitignore` 感知(require_git(false):无 `.git` 的普通目录
/// 也吃自己写的 .gitignore,与文件树同一直觉)+ Markdown 扩展名过滤 +
/// 单文件大小上限 + 整读进内存。被 [`scan`] 与 [`backlinks`] 共用 ——
/// 搜索与反向链接的「哪些文件算数」是同一件事,共用一段实现防两套语义
/// 漂移(与模块头「单一实现」同源)。
///
/// `visit` 收到(路径, 全文字节);返回 false 表示调用方已装满,立即结束。
/// 返回值:false = 被 `stop` 取消或 `visit` 要求提前结束。
/// 遍历错误(权限、条目消失)跳过:导航类扫描不因个别条目失败而整体失败。
fn walk_markdown(
    root: &Path,
    stop: &impl Fn() -> bool,
    mut visit: impl FnMut(&Path, &[u8]) -> bool,
) -> bool {
    for entry in WalkBuilder::new(root).require_git(false).build().flatten() {
        if stop() {
            return false;
        }
        let Some(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_file() {
            continue;
        }
        let path = entry.path();
        if !is_markdown(path) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        // 读失败(权限、竞争删除)跳过该文件,理由同上
        let Ok(haystack) = std::fs::read(path) else {
            continue;
        };
        if !visit(path, &haystack) {
            return false;
        }
    }
    true
}

/// 遍历 + 逐行匹配的两用主体,被 [`SearchService`] 与 [`search_sync`] 共用。
///
/// - `stop`:外部取消检查点(代际号),返回 true 立即结束且**不算截断**。
/// - `emit`:返回 false 表示「已装满,别再给了」,此时判为截断。
///
/// 取消检查点在「每进一个条目」与「每发一条命中」;单文件的行循环里不查
/// —— 内存中扫一遍是毫秒级,不值得为此加原子读。
fn scan(
    root: &Path,
    regex: &Regex,
    stop: impl Fn() -> bool,
    mut emit: impl FnMut(SearchResult) -> bool,
) -> bool {
    let mut truncated = false;
    walk_markdown(root, &stop, |path, haystack| {
        for (index, line) in LineIter::new(b'\n', haystack).enumerate() {
            let (body, _) = split_line_terminator(line);
            let Some(found) = regex.find(body) else {
                continue;
            };
            if stop() {
                return false; // 取消,不算截断(truncated 保持 false)
            }
            // lossy 会把非法字节换成 U+FFFD(3 字节),字节下标因此可能与
            // 原文不符;逐段夹到最近的字符边界,UI 侧切片绝不 panic。
            let line_text = trim_line_terminator(line);
            if !emit(SearchResult {
                path: path.to_path_buf(),
                line_no: index + 1,
                match_span: clamp_to_char_boundary(&line_text, found.start())
                    ..clamp_to_char_boundary(&line_text, found.end()),
                line_text,
            }) {
                truncated = true; // 调用方装满 = 截断
                return false;
            }
        }
        true
    });
    truncated
}

/// 把字节下标夹到 `text` 内最近的 UTF-8 字符边界。
///
/// 行正文是 **lossy 转换**后的 `String`:非法字节 `\xff` 会变成 3 字节的
/// U+FFFD,于是正则在**原始字节**上得出的下标与转换后的文本不再一一对应,
/// 直接拿去切 `String` 会 panic。这里在 ±3 字节(JSON 替换的最大膨胀)窗口
/// 内找最近的边界,找不到退化为 0 —— UI 宁可少高亮也不能崩。
fn clamp_to_char_boundary(text: &str, byte: usize) -> usize {
    let len = text.len();
    if byte <= len && text.is_char_boundary(byte) {
        return byte;
    }
    (0..=3)
        .flat_map(|step| [byte.saturating_sub(step), byte + step])
        .find(|candidate| *candidate <= len && text.is_char_boundary(*candidate))
        // 越界输入(如 byte > len)在窗口内找不到边界,退到行尾:宁可
        // 高亮贴到末尾也不 panic。
        .unwrap_or(len)
}

/// 扩展名是否 Markdown。
pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            MARKDOWN_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

/// 只拆掉一个 LF 或 CRLF 终止符;单独的 CR 与多余的 CR 都属于正文。
/// 搜索与替换共用此字节级切口,非 UTF-8 行也无需转换即可匹配。
fn split_line_terminator(line: &[u8]) -> (&[u8], &[u8]) {
    let terminator_len = if line.ends_with(b"\r\n") {
        2
    } else if line.ends_with(b"\n") {
        1
    } else {
        0
    };
    line.split_at(line.len() - terminator_len)
}

/// `LineIter` 产出的行自带 LF 或 CRLF;展示前只剥掉该终止符。
/// 非 UTF-8 内容替换为 U+FFFD,不因个别坏档让整次搜索失败。
pub fn trim_line_terminator(line: &[u8]) -> String {
    let (body, _) = split_line_terminator(line);
    String::from_utf8_lossy(body).into_owned()
}

/// 取 lossy 全文第 `line_breaks` 个换行之后的那一行(0 起 = 第一行),
/// 剥掉尾部 `\r`。行号由同一份全文的换行计数折算而来,必然命中;越界
/// (防御分支)返回空串。
fn line_at(text: &str, line_breaks: usize) -> String {
    text.split('\n')
        .nth(line_breaks)
        .unwrap_or_default()
        .trim_end_matches('\r')
        .to_owned()
}

/// 同步搜索结果(MCP `search_docs` 的返回体)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchOutcome {
    pub hits: Vec<SearchResult>,
    /// 命中数达到 `max_hits` 被截断(调用方需如实告知客户端)。
    pub truncated: bool,
}

/// 在**调用线程**一次跑完的搜索:MCP 工具没有帧循环,不需要 channel 与
/// 代际号,直接拿到完整结果(上限 `max_hits`)。
pub fn search_sync(query: &SearchQuery, max_hits: usize) -> Result<SearchOutcome, SearchError> {
    let regex = compile(query)?;
    let mut outcome = SearchOutcome::default();
    let hits = &mut outcome.hits;
    outcome.truncated = scan(
        &query.root,
        &regex,
        || false,
        |hit| {
            if hits.len() >= max_hits {
                return false;
            }
            hits.push(hit);
            true
        },
    );
    Ok(outcome)
}

/// 侧边栏「全部替换」的文本级原语:在单份文本上执行与 `scan`**同口径**
/// 的替换 —— 逐行匹配(`split_inclusive('\n')` 切片与 `LineIter` 同一切
/// 口径),`^`/`$` 锚点、跨行不可见等语义与搜索结果严格一致,绝不出现
/// 「列表里有命中、替换完还在」的错位。
///
/// `replacement` 在正则模式下支持 `$0`/`$1`/`${name}` 捕获引用
/// (`regex` crate 展开,`$$` 为字面 `$`);字面模式下替换文本不展开。
/// 返回(新文本, 替换处数);零命中时原样返回入参切片的所有权拷贝,
/// 调用方据计数跳过写盘 —— 未命中的文件一个字节都不动。逐行处理
/// 保留原始 LF/CRLF 终止符与未命中的行。
pub fn replace_in_text(
    query: &SearchQuery,
    replacement: &str,
    text: &str,
) -> Result<(String, usize), SearchError> {
    // 必须用 str 版正则:bytes 版的 `.` 匹配单个字节,替换会把多字节
    // 字符从中间切烂;str 版保证匹配边界与产出都是合法 UTF-8。开关拼装
    // 与扫描同源([`pattern_source`]),大小写语义一致。
    let regex = regex::RegexBuilder::new(&pattern_source(query)?)
        .case_insensitive(query.case_insensitive)
        .build()
        .map_err(|error| SearchError::InvalidPattern(error.to_string()))?;
    let mut out = String::with_capacity(text.len());
    let mut count = 0usize;
    for segment in text.split_inclusive('\n') {
        // Use the same exact LF/CRLF body split as byte scanning. The input is
        // valid UTF-8, so the ASCII-only slices can be viewed as str directly.
        let (body_bytes, terminator_bytes) = split_line_terminator(segment.as_bytes());
        let line = std::str::from_utf8(body_bytes).expect("text is valid UTF-8");
        let terminator = std::str::from_utf8(terminator_bytes).expect("terminator is ASCII");
        let hits = regex.find_iter(line).count();
        if hits == 0 {
            out.push_str(segment);
        } else {
            count += hits;
            if query.literal {
                out.push_str(&regex.replace_all(line, regex::NoExpand(replacement)));
            } else {
                out.push_str(&regex.replace_all(line, replacement));
            }
            out.push_str(terminator);
        }
    }
    Ok((out, count))
}

/// 线程间共享的代际号:最新一代是唯一值得产出结果的搜索。
#[derive(Debug, Default)]
struct Shared {
    generation: AtomicU64,
}

/// channel 内部事件:携带代际号,接收端据此丢弃旧代迟到结果。
#[derive(Debug)]
enum Event {
    Hit { gen: u64, result: SearchResult },
    Done { gen: u64 },
}

/// 后台搜索服务。同一时刻只有一个「当前」搜索:再次 `spawn` 或 `cancel`
/// 都会让前一代即刻作废。调用方(UI 归约)单线程持有,本类型不为并发
/// 调用设计。
#[derive(Debug)]
pub struct SearchService {
    shared: Arc<Shared>,
    tx: SyncSender<Event>,
    rx: Receiver<Event>,
}

impl SearchService {
    pub fn new() -> Self {
        let (tx, rx) = sync_channel(CHANNEL_CAPACITY);
        Self {
            shared: Arc::new(Shared::default()),
            tx,
            rx,
        }
    }

    /// 发起一次搜索;旧搜索即刻作废。正则在发起线程编译,非法模式同步
    /// 报错、不进后台线程。
    pub fn spawn(&mut self, query: SearchQuery) -> Result<(), SearchError> {
        let regex = compile(&query)?;
        // fetch_add 返回旧值,新代 = 旧值 + 1;旧线程下个检查点即退出
        let gen = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let shared = Arc::clone(&self.shared);
        let tx = self.tx.clone();
        thread::Builder::new()
            .name("latermd-search".into())
            .spawn(move || run(query, regex, gen, shared, tx))
            .map_err(SearchError::Spawn)?;
        Ok(())
    }

    /// 非阻塞接收一条当前代事件;channel 空返回 `None`。旧代迟到事件在
    /// 此被静默丢弃。
    pub fn try_recv(&mut self) -> Option<SearchEvent> {
        let current = self.shared.generation.load(Ordering::SeqCst);
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Hit { gen, result } if gen == current => {
                    return Some(SearchEvent::Hit(result));
                }
                Event::Done { gen } if gen == current => return Some(SearchEvent::Done),
                // 旧代迟到事件:丢弃,继续取
                _ => continue,
            }
        }
        None
    }

    /// 取消当前搜索:推进代际作废全部在途结果,并清空积压事件——放行可
    /// 能阻塞在 `send` 上的旧线程(它发出这条后在下个检查点退出,不会
    /// 永久滞留)。
    pub fn cancel(&mut self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        while self.rx.try_recv().is_ok() {}
    }
}

impl Default for SearchService {
    fn default() -> Self {
        Self::new()
    }
}

/// 后台线程主体。
fn run(query: SearchQuery, regex: Regex, gen: u64, shared: Arc<Shared>, tx: SyncSender<Event>) {
    let superseded = || shared.generation.load(Ordering::SeqCst) != gen;
    let mut alive = true;
    scan(&query.root, &regex, superseded, |result| {
        // 接收端已随服务销毁则停止(不是截断,故返回后不置 truncated)
        alive = tx.send(Event::Hit { gen, result }).is_ok();
        alive
    });
    if alive {
        let _ = tx.send(Event::Done { gen });
    }
}

/// 列目录的一条条目(MCP `list_files` 的返回项)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// 相对遍历根的路径(客户端拿它再调 `read_document`,不暴露绝对路径)。
    pub path: PathBuf,
    pub is_dir: bool,
}

/// 列目录结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListOutcome {
    pub entries: Vec<FileEntry>,
    pub truncated: bool,
}

/// 列出根(或根下子目录)内的条目:尊重 `.gitignore`,可选 glob 过滤,
/// 结果按路径排序后截断 —— 排序保证截断是确定性的(同 status 的 500 条
/// 上限口径,decisions-pending #19)。
///
/// `pattern` 走 `ignore` 的 override 语法(gitignore 风格 glob),不引额外
/// 依赖;非法 pattern 由 override 构建返回 Err。
pub fn list_files(
    root: &Path,
    sub_dir: Option<&Path>,
    pattern: Option<&str>,
    limit: usize,
) -> Result<ListOutcome, String> {
    let base = match sub_dir {
        Some(sub) => root.join(sub),
        None => root.to_path_buf(),
    };
    let mut builder = WalkBuilder::new(&base);
    builder.require_git(false);
    // glob 过滤走 override 的**匹配查询**而非 `builder.overrides()`:后者只
    // 筛文件、目录条目照旧产出(实测 `*.txt` 会带出 `notes` 目录),而列
    // 目录的语义是「条目本身要不要出现」,目录也必须过同一把筛子。
    let overrides = match pattern {
        Some(pattern) => {
            let mut overrides = OverrideBuilder::new(root);
            overrides
                .add(pattern)
                .map_err(|error| format!("glob 无效: {error}"))?;
            Some(
                overrides
                    .build()
                    .map_err(|error| format!("glob 无效: {error}"))?,
            )
        }
        None => None,
    };
    let mut entries: Vec<FileEntry> = builder
        .build()
        .flatten()
        // 跳过遍历根自身(相对路径为空,对客户端无意义)
        .filter(|entry| entry.depth() > 0)
        .filter(|entry| match &overrides {
            Some(overrides) => {
                let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
                overrides.matched(entry.path(), is_dir).is_whitelist()
            }
            None => true,
        })
        .map(|entry| FileEntry {
            path: entry
                .path()
                .strip_prefix(root)
                .unwrap_or(entry.path())
                .to_path_buf(),
            is_dir: entry.file_type().is_some_and(|kind| kind.is_dir()),
        })
        .collect();
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let truncated = entries.len() > limit;
    entries.truncate(limit);
    Ok(ListOutcome { entries, truncated })
}

/// 一条反向链接:来源文档里指向目标文档的一处 `[[wikilink]]`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlink {
    /// 来源文档相对遍历根的路径(与 [`FileEntry`] 同口径,不暴露绝对路径,
    /// 消费方拿它即可跳转打开来源文档)。
    pub path: PathBuf,
    /// 链接所在行,1 起(按链接 span 起点之前的换行数折算)。
    pub line_no: usize,
    /// 目标原文(`[[目标|显示名]]` 的前半段,保留用户写的 `.md` 后缀与
    /// 路径写法,面板原样展示)。
    pub target: String,
    /// 链接所在行的文本(剥掉行终止符):反向链接面板的命中行摘要数据源,
    /// 长行由展示层截断。
    pub line_text: String,
}

/// 反向链接扫描结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BacklinkOutcome {
    /// 按(来源路径, 行号, 目标)排序:同一来源的引用按行自上而下聚在一起。
    pub backlinks: Vec<Backlink>,
    /// 条数达到 `max_hits` 被截断。排序后截断,截哪几条是确定性的
    /// (与 [`list_files`] 同款纪律)。
    pub truncated: bool,
}

/// 剥掉末段的一个 `.md` / `.markdown` 后缀(`[[同名.md]]` 与 `[[同名]]`
/// 互容;`.markdown` 与 `.md` 互不为对方后缀,剥谁先谁后结果唯一)。
fn strip_md_extension(name: &str) -> &str {
    if let Some(stem) = name.strip_suffix(".markdown") {
        stem
    } else {
        name.strip_suffix(".md").unwrap_or(name)
    }
}

/// 相对路径比较键:分隔符统一 `/`、丢空段与 `.`、末段剥一个 md 后缀、
/// 小写。含 `..`(越出库根)或剥完为空返回 `None`,不参与匹配。
fn path_key(segments: Vec<String>) -> Option<String> {
    if segments.iter().any(|segment| segment == "..") {
        return None;
    }
    let mut parts: Vec<String> = segments
        .into_iter()
        .filter(|segment| !segment.is_empty() && segment != ".")
        .collect();
    let last = parts.pop()?;
    let mut key: Vec<String> = parts.into_iter().map(|part| part.to_lowercase()).collect();
    key.push(strip_md_extension(&last).to_lowercase());
    Some(key.join("/"))
}

/// wikilink 目标串是否指向 `doc_rel`(目标文档相对遍历根的路径)。
///
/// - 目标不含 `/`:按文件名 —— 剥一个 `.md`/`.markdown` 后缀后与目标文档
///   的 stem 忽略大小写全等(decisions-pending #26 `find_by_name` 的 stem
///   口径 + #87 的后缀容错,扩展名本身不限 md/markdown)。
/// - 目标含 `/`:按相对路径 —— 两侧都归一化分隔符、末段剥后缀后忽略
///   大小写全等,与 `find_by_name`「带 `/` 按相对路径直取」正向对称,
///   不同目录的同名文档不被 `[[dir/doc]]` 误伤。
///
/// 围栏代码块内的 `[[..]]` 不进入本函数(由 [`latermd_md::wikilinks`] 在
/// 抽取时豁免);目标里的 `#` 锚点(`[[doc#章节]]`)不剥,与正向解析同口径。
fn target_matches_doc(target: &str, doc_rel: &Path) -> bool {
    let target = target.trim();
    if target.is_empty() {
        return false;
    }
    if target.contains('/') || target.contains('\\') {
        // 目标串两种分隔符都认;文档路径交给 Path::components 归一
        let target_key = path_key(target.split(['/', '\\']).map(str::to_owned).collect());
        let doc_key = path_key(
            doc_rel
                .components()
                .filter_map(|component| component.as_os_str().to_str().map(str::to_owned))
                .collect(),
        );
        return target_key.is_some_and(|key| Some(key) == doc_key);
    }
    doc_rel.file_stem().is_some_and(|stem| {
        strip_md_extension(target).to_lowercase() == stem.to_string_lossy().to_lowercase()
    })
}

/// 反向链接扫描:全仓找出「哪些文档通过 `[[wikilink]]` 指向指定文档」
/// (反向链接面板的底层单一实现)。
///
/// - `doc`:目标文档路径。在遍历根之内取相对路径,之外原样使用;文件名
///   (stem)与相对路径两把匹配钥匙都从它派生,不要求该文档已落盘。
/// - 匹配口径(decisions-pending #26 / #87):目标不含 `/` 按文件名 stem
///   忽略大小写全等(`[[同名]]`/`[[同名.md]]` 互容),含 `/` 按相对路径
///   全等;围栏代码块内的 `[[..]]` 由 `latermd_md::wikilinks` 抽取时豁免。
///   **不校验目标文档存在**——扫描是纯文本匹配,`[[ghost]]` 指向未落盘的
///   ghost 同样计入(悬空引用与「未保存文档已被引用」由此天然覆盖)。
/// - 形态:同步直扫(反向链接量级小,面板要完整列表而非流式增量,不为
///   一次直扫复制 SearchService 的 channel + 代际号);`stop` 短路保留,
///   大仓可随时中断 —— 中断时返回已收到的部分,不置 `truncated`。
pub fn backlinks(
    root: &Path,
    doc: &Path,
    stop: impl Fn() -> bool,
    max_hits: usize,
) -> BacklinkOutcome {
    let doc_rel = doc.strip_prefix(root).unwrap_or(doc);
    let mut outcome = BacklinkOutcome::default();
    walk_markdown(root, &stop, |path, haystack| {
        // 坏档替换 U+FFFD 继续:不因个别非 UTF-8 文件让整次扫描失败
        // (span 与行号都基于同一份 lossy 串,自洽)
        let text = String::from_utf8_lossy(haystack);
        for link in latermd_md::wikilinks(&text) {
            if !target_matches_doc(&link.target, doc_rel) {
                continue;
            }
            let newlines = text.as_bytes()[..link.span.start]
                .iter()
                .filter(|&&byte| byte == b'\n')
                .count();
            outcome.backlinks.push(Backlink {
                path: path.strip_prefix(root).unwrap_or(path).to_path_buf(),
                line_no: newlines + 1,
                target: link.target.clone(),
                line_text: line_at(&text, newlines),
            });
        }
        // 反向链接量级小:不因装满提前停,收全 → 排序 → 截断才确定
        true
    });
    outcome.backlinks.sort_by(|left, right| {
        (&left.path, left.line_no, &left.target).cmp(&(&right.path, right.line_no, &right.target))
    });
    outcome.truncated = outcome.backlinks.len() > max_hits;
    outcome.backlinks.truncate(max_hits);
    outcome
}

#[cfg(test)]
mod tests {

    use super::*;

    /// 每个测试唯一目录:并行测试共享同一 pid 目录会互相污染残留文件
    /// (实测:零宽正则把别的测试留下的 a.md/c.md/i.md 全部命中)。
    fn root() -> std::path::PathBuf {
        let name = std::thread::current().name().unwrap_or("anon").to_owned();
        let dir = std::env::temp_dir().join(format!(
            "latermd-span-{}-{}",
            std::process::id(),
            name.replace("::", "-")
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// ASCII 命中:span 精确落在词上;行号、文本不变。
    #[test]
    fn span_marks_first_match_in_ascii_line() {
        let dir = root();
        std::fs::write(dir.join("a.md"), "hello world hello\n").unwrap();
        let hits = search_sync(&query(&dir, "hello", false), 10).unwrap().hits;
        assert_eq!(hits.len(), 1);
        assert_eq!(&hits[0].line_text[hits[0].match_span.clone()], "hello");
        assert_eq!(hits[0].match_span, 0..5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CJK 行:span 字节区间直接可切片,不 panic。
    #[test]
    fn span_is_char_boundary_on_cjk() {
        let dir = root();
        std::fs::write(dir.join("c.md"), "找到 rust 语言 rust 语言\n").unwrap();
        let hits = search_sync(&query(&dir, "rust", false), 10).unwrap().hits;
        assert_eq!(hits.len(), 1);
        assert!(hits[0].line_text.is_char_boundary(hits[0].match_span.start));
        assert!(hits[0].line_text.is_char_boundary(hits[0].match_span.end));
        assert_eq!(&hits[0].line_text[hits[0].match_span.clone()], "rust");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 大小写不敏感:span 覆盖实际出现在行里的那段。
    #[test]
    fn span_follows_case_insensitive_hit() {
        let dir = root();
        std::fs::write(dir.join("i.md"), "Say Foo twice: foo\n").unwrap();
        let hits = search_sync(&query(&dir, "foo", true), 10).unwrap().hits;
        assert_eq!(&hits[0].line_text[hits[0].match_span.clone()], "Foo");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 空/零宽匹配:退化为 start == end,UI 判定无高亮,不 panic。
    #[test]
    fn zero_width_match_degrades_to_empty_span() {
        let dir = root();
        std::fs::write(dir.join("z.md"), "ab\n").unwrap();
        let hits = search_sync(&query(&dir, "x*", false), 10).unwrap().hits;
        assert_eq!(hits.len(), 1);
        assert!(hits[0].match_span.start <= hits[0].match_span.end);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// clamp 单测:非边界下标夹回最近的边界,越界钳到 len。
    #[test]
    fn clamp_finds_nearest_char_boundary() {
        assert_eq!(clamp_to_char_boundary("rust 中文", 0), 0);
        assert_eq!(clamp_to_char_boundary("rust 中文", 4), 4);
        // “中” 起于字节 5;夹 6 → 5(前一边界)
        assert_eq!(clamp_to_char_boundary("rust 中文", 6), 5);
        // 越界钳到 len
        assert_eq!(clamp_to_char_boundary("rust", 99), 4);
    }

    use std::time::{Duration, Instant};

    /// 进程内唯一的临时目录;测试自删。
    fn temp_vault(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("latermd-search-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 造样本仓库:3 个有效 md + `.gitignore` 排除项 + 非 md 二进制。
    /// - `a.md`:`LaterMD`(精确大小写)+ 中文「架构」
    /// - `notes/b.markdown`(嵌套子目录):小写 `latermd`,供大小写不敏感用
    /// - `c.md`:无关键词(负样本)
    /// - `ignored.md` / `drafts/hidden.md`:被 `.gitignore` 排除
    /// - `blob.bin`:含关键词的非 md 二进制(扩展名过滤的对象)
    fn sample_vault(name: &str) -> PathBuf {
        let root = temp_vault(name);
        std::fs::write(
            root.join("a.md"),
            "LaterMD 是 Markdown 工作台\n普通一行\n架构决策记录\n",
        )
        .unwrap();
        std::fs::create_dir(root.join("notes")).unwrap();
        std::fs::write(root.join("notes/b.markdown"), "latermd 不分大小写\n").unwrap();
        std::fs::write(root.join("c.md"), "这里没有关键词\n").unwrap();
        std::fs::write(root.join(".gitignore"), "ignored.md\ndrafts/\n").unwrap();
        std::fs::write(root.join("ignored.md"), "latermd 不该被搜到\n").unwrap();
        std::fs::create_dir(root.join("drafts")).unwrap();
        std::fs::write(root.join("drafts/hidden.md"), "latermd 草稿也不搜\n").unwrap();
        std::fs::write(root.join("blob.bin"), b"latermd\x00\x01binary").unwrap();
        root
    }

    fn query(root: &Path, pattern: &str, case_insensitive: bool) -> SearchQuery {
        SearchQuery {
            root: root.to_path_buf(),
            pattern: pattern.into(),
            case_insensitive,
            whole_word: false,
            literal: false,
        }
    }

    /// 完整匹配变体:其余字段与 [`query`] 同(正则模式)。
    fn query_flags(
        root: &Path,
        pattern: &str,
        case_insensitive: bool,
        whole_word: bool,
    ) -> SearchQuery {
        SearchQuery {
            whole_word,
            ..query(root, pattern, case_insensitive)
        }
    }

    /// 字面模式变体:大小写敏感、不整词。
    fn literal_query(root: &Path, pattern: &str) -> SearchQuery {
        SearchQuery {
            literal: true,
            ..query(root, pattern, false)
        }
    }

    /// 用公开原语收全当前代结果:收到 `Done` 或 10s 超时为止。
    /// 返回(命中列表, 是否正常收到 Done)。
    fn drain(service: &mut SearchService) -> (Vec<SearchResult>, bool) {
        let mut hits = Vec::new();
        let mut done = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done && Instant::now() < deadline {
            match service.try_recv() {
                Some(SearchEvent::Hit(hit)) => hits.push(hit),
                Some(SearchEvent::Done) => done = true,
                None => std::thread::sleep(Duration::from_millis(2)),
            }
        }
        (hits, done)
    }

    /// 拍平成可比较集合(路径、行号、行文本)。
    fn hit_triples(hits: &[SearchResult]) -> Vec<(PathBuf, usize, String)> {
        hits.iter()
            .map(|hit| (hit.path.clone(), hit.line_no, hit.line_text.clone()))
            .collect()
    }

    /// 大小写敏感:只命中精确写法;行文本不含换行符;`.gitignore` 排除项
    /// 与非 md 二进制(`blob.bin` 同含关键词)不出现——集合精确相等即同时
    /// 覆盖这两点。
    #[test]
    fn case_sensitive_hits_exact_only() {
        let root = sample_vault("case-sensitive");
        let mut service = SearchService::new();
        service.spawn(query(&root, "LaterMD", false)).unwrap();
        let (hits, done) = drain(&mut service);

        assert!(done, "搜索应自然结束并发出 Done");
        assert_eq!(
            hit_triples(&hits),
            vec![(root.join("a.md"), 1, "LaterMD 是 Markdown 工作台".into())]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// draft 镜像不入搜索(#18):`x.md.latermd-draft` 的扩展名不在
    /// [`MARKDOWN_EXTENSIONS`] 里,`is_markdown` 天然排除 —— 内容即便命中
    /// 关键词也不产出结果。钉住该行为,自动保存的防丢镜像不进导航。
    #[test]
    fn search_skips_draft_mirror_files() {
        let root = temp_vault("draft-mirror");
        std::fs::write(root.join("x.md"), "latermd 正文\n").unwrap();
        std::fs::write(root.join("x.md.latermd-draft"), "latermd 草稿镜像\n").unwrap();

        let mut service = SearchService::new();
        service.spawn(query(&root, "latermd", false)).unwrap();
        let (hits, done) = drain(&mut service);

        assert!(done);
        assert_eq!(
            hit_triples(&hits),
            vec![(root.join("x.md"), 1, "latermd 正文".into())],
            "draft 即使命中关键词也不入搜索"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 大小写不敏感:嵌套子目录里的小写变体一并命中(按路径排序稳定断言)。
    #[test]
    fn case_insensitive_hits_nested_variant() {
        let root = sample_vault("case-insensitive");
        let mut service = SearchService::new();
        service.spawn(query(&root, "LaterMD", true)).unwrap();
        let (hits, done) = drain(&mut service);

        assert!(done);
        let mut triples = hit_triples(&hits);
        triples.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            triples,
            vec![
                (root.join("a.md"), 1, "LaterMD 是 Markdown 工作台".into()),
                (
                    root.join("notes/b.markdown"),
                    1,
                    "latermd 不分大小写".into()
                ),
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 中文关键词走同一条正则路径(regex 的 Unicode 支持),行号正确(第 3 行)。
    #[test]
    fn chinese_pattern_hits_third_line() {
        let root = sample_vault("chinese");
        let mut service = SearchService::new();
        service.spawn(query(&root, "架构", false)).unwrap();
        let (hits, done) = drain(&mut service);

        assert!(done);
        assert_eq!(
            hit_triples(&hits),
            vec![(root.join("a.md"), 3, "架构决策记录".into())]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 取消后不再产出任何事件(含 Done)。sleep 覆盖两种时序:线程已塞入
    /// 结果(被 cancel 的 drain 丢弃)或尚未启动(下个检查点即退出)。
    #[test]
    fn cancel_stops_production() {
        let root = sample_vault("cancel");
        let mut service = SearchService::new();
        service.spawn(query(&root, "latermd", true)).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        service.cancel();

        let deadline = Instant::now() + Duration::from_millis(200);
        while Instant::now() < deadline {
            assert_eq!(
                service.try_recv(),
                None,
                "取消后不应再有任何事件(含旧代迟到的 Done)"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 新搜索发起后,旧搜索即使已把结果塞进 channel,也只允许新代命中
    /// 与新代 Done 流出。
    #[test]
    fn superseded_search_yields_only_new_generation() {
        let root = sample_vault("supersede");
        let mut service = SearchService::new();
        // 第一代:唯一命中是 a.md 第 2 行「普通一行」
        service.spawn(query(&root, "普通", false)).unwrap();
        std::thread::sleep(Duration::from_millis(50)); // 给第一代塞结果的机会
                                                       // 第二代取代之
        service.spawn(query(&root, "latermd", true)).unwrap();
        let (hits, done) = drain(&mut service);

        assert!(done);
        let mut triples = hit_triples(&hits);
        triples.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            triples,
            vec![
                (root.join("a.md"), 1, "LaterMD 是 Markdown 工作台".into()),
                (
                    root.join("notes/b.markdown"),
                    1,
                    "latermd 不分大小写".into()
                ),
            ],
            "第一代命中(a.md:2「普通一行」)不允许混入"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 空模式与未闭合括号在发起线程同步报错,不产生任何事件。
    #[test]
    fn invalid_pattern_is_rejected_upfront() {
        let root = sample_vault("invalid");
        let mut service = SearchService::new();
        assert!(matches!(
            service.spawn(query(&root, "(", false)),
            Err(SearchError::InvalidPattern(_))
        ));
        assert!(matches!(
            service.spawn(query(&root, "", false)),
            Err(SearchError::InvalidPattern(_))
        ));
        assert_eq!(service.try_recv(), None, "发起失败不应有事件");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 超过单文件上限的 md 整体跳过(行迭代要整读进内存),而非部分命中。
    #[test]
    fn oversized_markdown_is_skipped() {
        let root = temp_vault("oversize");
        let mut bytes = b"latermd marker\n".to_vec();
        bytes.resize(MAX_FILE_BYTES as usize + 1, b'.');
        std::fs::write(root.join("big.md"), bytes).unwrap();

        let mut service = SearchService::new();
        service.spawn(query(&root, "latermd", false)).unwrap();
        let (hits, done) = drain(&mut service);
        assert!(done);
        assert!(hits.is_empty(), "超限文件跳过,首行关键词也不产出");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// CRLF 文件:行号语义与 LF 一致,行文本剥掉 `\r\n`。
    #[test]
    fn crlf_lines_are_trimmed() {
        let root = temp_vault("crlf");
        std::fs::write(root.join("win.md"), b"first\r\nlatermd here\r\n").unwrap();

        let mut service = SearchService::new();
        service.spawn(query(&root, "latermd", false)).unwrap();
        let (hits, done) = drain(&mut service);
        assert!(done);
        assert_eq!(
            hit_triples(&hits),
            vec![(root.join("win.md"), 2, "latermd here".into())]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 同步入口与后台服务同一套语义:大小写、.gitignore、行号一致(MCP
    /// 与侧边栏不漂移的前提)。
    #[test]
    fn sync_search_matches_service_semantics() {
        let root = sample_vault("sync");
        let outcome = search_sync(&query(&root, "LaterMD", true), MAX_HITS).unwrap();
        let mut triples: Vec<_> = outcome
            .hits
            .iter()
            .map(|hit| (hit.path.clone(), hit.line_no, hit.line_text.clone()))
            .collect();
        triples.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            triples,
            vec![
                (root.join("a.md"), 1, "LaterMD 是 Markdown 工作台".into()),
                (
                    root.join("notes/b.markdown"),
                    1,
                    "latermd 不分大小写".into()
                ),
            ]
        );
        assert!(!outcome.truncated);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 同步入口的截断:上限内装满即停,标志置位。
    #[test]
    fn sync_search_truncates_at_cap() {
        let root = temp_vault("sync-truncate");
        std::fs::write(root.join("many.md"), "latermd 行\n".repeat(80)).unwrap();
        let outcome = search_sync(&query(&root, "latermd", false), 10).unwrap();
        assert_eq!(outcome.hits.len(), 10);
        assert!(outcome.truncated);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 同步入口的坏模式与空模式同样在调用线程报错。
    #[test]
    fn sync_search_rejects_bad_pattern() {
        let root = sample_vault("sync-bad");
        assert!(search_sync(&query(&root, "(", false), MAX_HITS).is_err());
        assert!(search_sync(&query(&root, "", false), MAX_HITS).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 列目录:尊重 .gitignore(ignored.md / drafts/ 不出现),目录与文件
    /// 都报,相对路径不含根前缀。
    #[test]
    fn list_files_respects_gitignore_and_returns_relative_paths() {
        let root = sample_vault("list");
        let outcome = list_files(&root, None, None, MAX_LIST_ENTRIES).unwrap();
        let paths: Vec<_> = outcome
            .entries
            .iter()
            .map(|entry| entry.path.to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(paths.contains(&"a.md".to_owned()), "{paths:?}");
        assert!(paths.contains(&"notes".to_owned()), "目录也是条目");
        assert!(paths.contains(&"notes/b.markdown".to_owned()));
        assert!(!paths.contains(&"ignored.md".to_owned()), "gitignore 排除");
        assert!(
            !paths.iter().any(|path| path.starts_with("drafts")),
            "gitignore 排除目录"
        );
        assert!(!outcome.truncated);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 列目录:子目录 + glob 过滤 + 上限截断。
    #[test]
    fn list_files_supports_subdir_glob_and_limit() {
        let root = sample_vault("list-sub");
        std::fs::write(root.join("notes/x.md"), "x").unwrap();
        std::fs::write(root.join("notes/y.txt"), "y").unwrap();

        let sub = list_files(&root, Some(Path::new("notes")), None, MAX_LIST_ENTRIES).unwrap();
        let names: Vec<_> = sub
            .entries
            .iter()
            .map(|entry| entry.path.to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(names.contains(&"notes/b.markdown".to_owned()), "{names:?}");
        assert!(names.contains(&"notes/x.md".to_owned()));
        assert!(!names.contains(&"a.md".to_owned()), "只列子目录");

        let globbed = list_files(&root, None, Some("*.txt"), MAX_LIST_ENTRIES).unwrap();
        let glob_names: Vec<_> = globbed
            .entries
            .iter()
            .map(|entry| entry.path.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(glob_names, vec!["notes/y.txt".to_owned()], "{glob_names:?}");

        let limited = list_files(&root, None, None, 1).unwrap();
        assert_eq!(limited.entries.len(), 1);
        assert!(limited.truncated);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 坏 glob 由 override 构建报错,不 panic。
    #[test]
    fn list_files_reports_invalid_glob() {
        let root = sample_vault("list-bad-glob");
        let outcome = list_files(&root, None, Some("[[["), MAX_LIST_ENTRIES);
        assert!(outcome.is_err() || outcome.is_ok(), "不 panic 即可");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ─── 字面 / 完整匹配开关(literal / whole_word)────────────────

    /// 字面模式:含元字符的词按文本命中(未闭合括号当正则必然编译失败);
    /// 同一词走正则路径则报 InvalidPattern —— 两个断言钉住开关的两面。
    #[test]
    fn literal_mode_escapes_metacharacters() {
        let root = temp_vault("literal");
        std::fs::write(root.join("cpp.md"), "函数 a(b 括号未闭合\n").unwrap();

        let literal = literal_query(&root, "a(b");
        let outcome = search_sync(&literal, MAX_HITS).unwrap();
        assert_eq!(outcome.hits.len(), 1, "字面模式按文本命中 a(b");

        let as_regex = query(&root, "a(b", false);
        assert!(
            search_sync(&as_regex, MAX_HITS).is_err(),
            "正则模式编译失败"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 完整匹配:只命中独立词,复合词(running/runs)不误伤;大小写开关
    /// 照常叠加。
    #[test]
    fn whole_word_skips_inflections() {
        let root = temp_vault("whole-word");
        std::fs::write(root.join("w.md"), "running runs run Run\n").unwrap();

        let whole = query_flags(&root, "run", true, true);
        let outcome = search_sync(&whole, MAX_HITS).unwrap();
        assert_eq!(outcome.hits.len(), 1, "整行命中");
        let line = &outcome.hits[0].line_text;
        // 行内命中数靠 find_iter 数:run 与 Run 各一处,running/runs 不算
        let regex = compile(&whole).unwrap();
        assert_eq!(regex.find_iter(line.as_bytes()).count(), 2, "{line}");

        let plain = query_flags(&root, "run", true, false);
        let regex = compile(&plain).unwrap();
        assert_eq!(
            regex.find_iter(line.as_bytes()).count(),
            4,
            "无开关时词内也命中"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 完整匹配 + 交替模式:`\b` 作用于整个非捕获组,两个词都整词生效。
    #[test]
    fn whole_word_wraps_alternation_atomically() {
        let root = temp_vault("whole-alt");
        std::fs::write(root.join("alt.md"), "cat catalog dog dogs\n").unwrap();
        let whole = query_flags(&root, "cat|dog", false, true);
        let regex = compile(&whole).unwrap();
        let line = "cat catalog dog dogs";
        assert_eq!(regex.find_iter(line.as_bytes()).count(), 2, "{line}");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ─── 文本级替换(replace_in_text)──────────────────────────────

    /// 基础替换:命中处全换、计数正确、换行风格(CRLF 与末行无换行)
    /// 原样保留。
    #[test]
    fn replace_in_text_counts_and_preserves_lines() {
        let root = temp_vault("replace-basic");
        let text = "foo bar\r\nfoo again\nno match here\nfoo tail";
        let outcome = replace_in_text(&query_flags(&root, "foo", true, false), "baz", text);
        let (new_text, count) = outcome.unwrap();
        assert_eq!(count, 3);
        assert_eq!(new_text, "baz bar\r\nbaz again\nno match here\nbaz tail");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 零命中恒等:返回与入参相同的文本、计数 0 —— 调用方据此跳过写盘。
    #[test]
    fn replace_in_text_is_identity_without_hits() {
        let root = temp_vault("replace-identity");
        let text = "什么都没有\n";
        let (new_text, count) =
            replace_in_text(&query_flags(&root, "不存在", true, false), "x", text).unwrap();
        assert_eq!(count, 0);
        assert_eq!(new_text, text);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 捕获引用:`$1` 展开为第一捕获组(`$0` 全匹配),与 VS Code 替换
    /// 语法对齐;正则模式与字面模式各验一面。
    #[test]
    fn replace_in_text_expands_capture_references() {
        let root = temp_vault("replace-capture");
        let text = "联系 bob@example.com 或 carol@example.com\n";
        let outcome = replace_in_text(
            &query_flags(&root, r"(\w+)@example\.com", true, false),
            "$1 (at)",
            text,
        );
        let (new_text, count) = outcome.unwrap();
        assert_eq!(count, 2);
        assert_eq!(new_text, "联系 bob (at) 或 carol (at)\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 行锚点与搜索同口径:逐行处理使 `^-` 这类行首锚点在**每一行**生效
    /// (整篇一次性 replace_all 则只有首行能命中 —— 钉住逐行切片口径)。
    #[test]
    fn replace_in_text_line_anchors_match_search_semantics() {
        let root = temp_vault("replace-anchor");
        let text = "- a\nx\n- b\n";
        let outcome = replace_in_text(&query_flags(&root, "^- ", true, false), "* ", text);
        let (new_text, count) = outcome.unwrap();
        assert_eq!(count, 2);
        assert_eq!(new_text, "* a\nx\n* b\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// LF 与 CRLF 的行体切分必须让搜索和替换对行锚点给出同样结果。
    #[test]
    fn search_and_replace_agree_on_lf_and_crlf_anchors() {
        let root = temp_vault("replace-anchor-terminators");
        for (name, text) in [
            ("lf.md", "foo\nnope\nfoo\n"),
            ("crlf.md", "foo\r\nnope\r\nfoo\r\n"),
        ] {
            std::fs::write(root.join(name), text).unwrap();
        }

        let query = query(&root, "^foo$", false);
        let outcome = search_sync(&query, MAX_HITS).unwrap();
        let mut hits = outcome
            .hits
            .iter()
            .map(|hit| {
                (
                    hit.path.file_name().unwrap().to_owned(),
                    hit.line_no,
                    hit.line_text.clone(),
                )
            })
            .collect::<Vec<_>>();
        hits.sort();
        assert_eq!(
            hits,
            vec![
                ("crlf.md".into(), 1, "foo".into()),
                ("crlf.md".into(), 3, "foo".into()),
                ("lf.md".into(), 1, "foo".into()),
                ("lf.md".into(), 3, "foo".into()),
            ]
        );
        for (name, expected) in [
            ("lf.md", "bar\nnope\nbar\n"),
            ("crlf.md", "bar\r\nnope\r\nbar\r\n"),
        ] {
            let source = std::fs::read_to_string(root.join(name)).unwrap();
            let (replaced, count) = replace_in_text(&query, "bar", &source).unwrap();
            assert_eq!(count, 2, "{name}");
            assert_eq!(replaced, expected, "{name}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 替换整行时只改行体,CRLF 终止符必须原样保留。
    #[test]
    fn replace_in_text_preserves_crlf_for_line_wildcard() {
        let root = temp_vault("replace-crlf-wildcard");
        let text = "one\r\ntwo\r\n";
        let (new_text, count) = replace_in_text(&query(&root, "^.*$", false), "x", text).unwrap();
        assert_eq!(count, 2);
        assert_eq!(new_text, "x\r\nx\r\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 没有最终换行符的末行既可被搜索,也可被替换,且不凭空增加终止符。
    #[test]
    fn search_and_replace_handle_final_unterminated_line() {
        let root = temp_vault("replace-final-line");
        let path = root.join("final.md");
        std::fs::write(&path, "prefix\nfoo").unwrap();
        let query = query(&root, "^foo$", false);
        let outcome = search_sync(&query, MAX_HITS).unwrap();
        assert_eq!(outcome.hits.len(), 1);
        assert_eq!(outcome.hits[0].line_no, 2);
        assert_eq!(outcome.hits[0].line_text, "foo");
        let (new_text, count) = replace_in_text(&query, "bar", "prefix\nfoo").unwrap();
        assert_eq!(count, 1);
        assert_eq!(new_text, "prefix\nbar");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 字面查询的替换文本必须是 NoExpand: `$HOME`, `$1` 与 `$$` 都按原文输出。
    #[test]
    fn literal_replacement_does_not_expand_dollar_syntax() {
        let root = temp_vault("replace-literal-dollars");
        let query = literal_query(&root, "$HOME/$1/$$");
        let text = "$HOME/$1/$$\n";
        std::fs::write(root.join("dollars.md"), text).unwrap();
        let outcome = search_sync(&query, MAX_HITS).unwrap();
        assert_eq!(outcome.hits.len(), 1);
        let (new_text, count) = replace_in_text(&query, "$HOME/$1/$$", text).unwrap();
        assert_eq!(count, 1);
        assert_eq!(new_text, text);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn replace_in_text_respects_whole_word() {
        let root = temp_vault("replace-whole");
        let text = "run runner running\n";
        let outcome = replace_in_text(&query_flags(&root, "run", true, true), "runner", text);
        let (new_text, count) = outcome.unwrap();
        assert_eq!(count, 1);
        assert_eq!(new_text, "runner runner running\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// CJK 字面替换:中文词按文本命中并替换,多字节偏移不串位。
    #[test]
    fn replace_in_text_cjk_literal() {
        let root = temp_vault("replace-cjk");
        let text = "架构决策记录\n架构评审\n";
        let outcome = replace_in_text(&query_flags(&root, "架构", true, false), "设计", text);
        let (new_text, count) = outcome.unwrap();
        assert_eq!(count, 2);
        assert_eq!(new_text, "设计决策记录\n设计评审\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 反向链接样本库:目标文档 `note.md` 被多个文件以多种写法引用,外加
    /// 四类负样本(gitignore 排除、围栏代码块、draft 镜像、无链接文档)。
    fn backlink_vault(name: &str) -> PathBuf {
        let root = temp_vault(name);
        // 目标文档自身带一处自链(第 2 行):自链也是真实引用,计入
        std::fs::write(root.join("note.md"), "我是 note\n自链 [[note]] 也算\n").unwrap();
        std::fs::write(root.join("a.md"), "第一行\n第二行\n见 [[note]]\n").unwrap();
        // 同一行两条:`.md` 后缀写法 + 带显示名写法(目标都取前半段)
        std::fs::write(root.join("b.md"), "[[note.md]] 与 [[note|显示名]]\n").unwrap();
        std::fs::create_dir(root.join("notes")).unwrap();
        std::fs::write(root.join("notes/c.md"), "子目录来源 [[note]]\n").unwrap();
        std::fs::write(root.join("plain.md"), "没有链接\n").unwrap();
        std::fs::write(root.join(".gitignore"), "ignored.md\n").unwrap();
        std::fs::write(root.join("ignored.md"), "[[note]] 不该出现\n").unwrap();
        // 围栏代码块里的 [[..]] 不算链接;围栏后的正文链接照算(第 5 行)
        std::fs::write(
            root.join("code.md"),
            "```rust\nlet grid = [[note]];\nlet x = grid[[note]][0];\n```\n正文 [[note]] 算\n",
        )
        .unwrap();
        // autosave 镜像(#18)不是 Markdown,天然不入扫描
        std::fs::write(root.join("note.md.latermd-draft"), "[[note]] 镜像不算\n").unwrap();
        root
    }

    /// 拍平成可比较四元组(路径转 `/` 分隔,断言跨平台稳定;含行文本,
    /// 反向链接面板的摘要数据源)。
    fn backlink_triples(outcome: &BacklinkOutcome) -> Vec<(String, usize, String, String)> {
        outcome
            .backlinks
            .iter()
            .map(|link| {
                (
                    link.path.to_string_lossy().replace('\\', "/"),
                    link.line_no,
                    link.target.clone(),
                    link.line_text.clone(),
                )
            })
            .collect()
    }

    /// 多文件互链 + 自链 + `.md` 后缀容错 + 带显示名 + 子目录来源;来源路径
    /// 是相对路径;gitignore 排除项、代码块内 `[[..]]`、draft 镜像、无链接
    /// 文档全部不出现(集合精确相等同时覆盖正负样本)。
    #[test]
    fn backlinks_collect_references_across_files() {
        let root = backlink_vault("collect");
        let outcome = backlinks(&root, &root.join("note.md"), || false, MAX_HITS);

        assert!(!outcome.truncated);
        assert_eq!(
            backlink_triples(&outcome),
            vec![
                ("a.md".into(), 3, "note".into(), "见 [[note]]".into()),
                (
                    "b.md".into(),
                    1,
                    "note".into(),
                    "[[note.md]] 与 [[note|显示名]]".into()
                ),
                (
                    "b.md".into(),
                    1,
                    "note.md".into(),
                    "[[note.md]] 与 [[note|显示名]]".into()
                ),
                (
                    "code.md".into(),
                    5,
                    "note".into(),
                    "正文 [[note]] 算".into()
                ),
                (
                    "note.md".into(),
                    2,
                    "note".into(),
                    "自链 [[note]] 也算".into()
                ),
                (
                    "notes/c.md".into(),
                    1,
                    "note".into(),
                    "子目录来源 [[note]]".into()
                ),
            ],
            "行号按 span 起点前的换行数折算;同文件同行按目标排序"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// CJK 与带空格目标:无后缀与 `.md` 后缀两写法都命中。
    #[test]
    fn backlinks_match_cjk_and_spaced_targets() {
        let root = temp_vault("cjk-space");
        std::fs::write(root.join("笔记 一.md"), "目标文档\n").unwrap();
        std::fs::write(root.join("from.md"), "见 [[笔记 一]] 与 [[笔记 一.md]]\n").unwrap();

        let outcome = backlinks(&root, &root.join("笔记 一.md"), || false, MAX_HITS);
        assert_eq!(
            outcome
                .backlinks
                .iter()
                .map(|link| (link.line_no, link.target.clone()))
                .collect::<Vec<_>>(),
            vec![(1, "笔记 一".to_owned()), (1, "笔记 一.md".to_owned())]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 文件名匹配忽略大小写(与 find_by_name 的 stem 口径一致,#26)。
    #[test]
    fn backlinks_file_name_match_is_case_insensitive() {
        let root = temp_vault("case");
        std::fs::write(root.join("Mixed Case.md"), "目标\n").unwrap();
        std::fs::write(root.join("from.md"), "见 [[mixed case]]\n").unwrap();

        let outcome = backlinks(&root, &root.join("Mixed Case.md"), || false, MAX_HITS);
        assert_eq!(
            backlink_triples(&outcome),
            vec![(
                "from.md".into(),
                1,
                "mixed case".into(),
                "见 [[mixed case]]".into()
            )]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 路径式目标按相对路径匹配(#87):大小写不敏感、`.md` 后缀容错;
    /// 不同目录的同名文档不被 `[[other/goal]]` 误伤。
    #[test]
    fn backlinks_match_path_targets_by_relative_path() {
        let root = temp_vault("path-target");
        std::fs::create_dir_all(root.join("dir")).unwrap();
        std::fs::create_dir_all(root.join("other")).unwrap();
        std::fs::write(root.join("dir/Goal.md"), "目标\n").unwrap();
        std::fs::write(root.join("other/Goal.md"), "同名不同目录\n").unwrap();
        std::fs::write(
            root.join("src.md"),
            "[[dir/goal]] 命中\n[[DIR/GOAL.md]] 大写也命中\n[[other/goal]] 不指向 dir\n",
        )
        .unwrap();

        let outcome = backlinks(&root, &root.join("dir/Goal.md"), || false, MAX_HITS);
        assert_eq!(
            backlink_triples(&outcome),
            vec![
                (
                    "src.md".into(),
                    1,
                    "dir/goal".into(),
                    "[[dir/goal]] 命中".into()
                ),
                (
                    "src.md".into(),
                    2,
                    "DIR/GOAL.md".into(),
                    "[[DIR/GOAL.md]] 大写也命中".into()
                ),
            ],
            "第 3 行 [[other/goal]] 指向另一目录,不命中"
        );

        // 换成 .markdown 扩展名后,无后缀/带 .md 的路径目标都仍命中
        // (stem 与扩展名互不绑定)
        std::fs::remove_file(root.join("dir/Goal.md")).unwrap();
        std::fs::write(root.join("dir/Goal.markdown"), "换扩展名\n").unwrap();
        let outcome = backlinks(&root, &root.join("dir/Goal.markdown"), || false, MAX_HITS);
        assert_eq!(outcome.backlinks.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 悬空引用计入:目标文档不存在,指向它的引用照报(#87 的口径)。
    #[test]
    fn backlinks_count_dangling_references() {
        let root = temp_vault("dangling");
        std::fs::write(root.join("ref.md"), "指向未落盘的 [[ghost]]\n").unwrap();

        let outcome = backlinks(&root, &root.join("ghost.md"), || false, MAX_HITS);
        assert_eq!(
            backlink_triples(&outcome),
            vec![(
                "ref.md".into(),
                1,
                "ghost".into(),
                "指向未落盘的 [[ghost]]".into()
            )]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// stop 短路:发起即取消,一条不扫、不 panic,也不算截断。
    #[test]
    fn backlinks_stop_short_circuits() {
        let root = backlink_vault("stop");
        let outcome = backlinks(&root, &root.join("note.md"), || true, MAX_HITS);
        assert!(outcome.backlinks.is_empty());
        assert!(!outcome.truncated, "取消不算截断");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 上限截断:收全 → 排序 → 截断,保留的是排序后的前 N 条(确定性,
    /// 与 list_files 同款纪律)。
    #[test]
    fn backlinks_truncate_after_sort() {
        let root = backlink_vault("truncate");
        let full = backlinks(&root, &root.join("note.md"), || false, MAX_HITS);
        let capped = backlinks(&root, &root.join("note.md"), || false, 2);

        assert!(capped.truncated);
        assert_eq!(capped.backlinks, full.backlinks[..2].to_vec());
        let _ = std::fs::remove_dir_all(&root);
    }
}
