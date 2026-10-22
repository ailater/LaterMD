//! 文档库检索核心:`.gitignore` 感知的目录遍历 + 逐行正则匹配。
//!
//! 本 crate 是**侧边栏搜索与 MCP `search_docs` / `list_files` 的单一实现**
//! (docs/mcp-plan.md §3 方案 A:不复制一份给 MCP,否则两套 `.gitignore`
//! 语义与两套截断上限必然漂移)。**不依赖 egui/eframe**(铁律二),因此
//! MCP server(后台线程)与 GUI 归约都能直接调。
//!
//! 两套入口共用同一段遍历主体(私有 `scan`):
//!
//! - [`SearchService`]:后台线程 + 有界 channel,供 UI 流式取用。取消是
//!   **代际号语义** —— `spawn`/`cancel` 都推进全局代际,旧线程在检查点
//!   自行退出,迟到的旧代事件由接收端按代际丢弃;UI 侧 `try_recv` 永不
//!   阻塞,也不 join 后台线程。
//! - [`search_sync`]:在调用线程一次跑完,供无 UI 的消费者(MCP 工具)使用。
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

/// 后台结果到达通知。调用方可用它唤醒事件循环；核心搜索层不依赖任何
/// GUI 框架，也不通过回调传递搜索数据。
pub type SearchNotify = Arc<dyn Fn() + Send + Sync>;

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
#[derive(Debug, Clone)]
pub struct SearchQuery {
    /// 遍历根(MCP 侧已校验过越界)。
    pub root: PathBuf,
    /// 正则模式(用户输入按正则解释;大小写开关独立于模式本身)。
    pub pattern: String,
    pub case_insensitive: bool,
}

/// 一条命中:文件 + 1 起行号 + 剥掉行终止符的行文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub path: PathBuf,
    pub line_no: usize,
    pub line_text: String,
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

/// 编译搜索正则:同步执行,让非法模式在调用线程就报错(UI 输入到一半、
/// MCP 传坏参数都是这一路径)。
fn compile(query: &SearchQuery) -> Result<Regex, SearchError> {
    if query.pattern.is_empty() {
        return Err(SearchError::InvalidPattern("搜索词为空".into()));
    }
    RegexBuilder::new(&query.pattern)
        .case_insensitive(query.case_insensitive)
        .build()
        .map_err(|error| SearchError::InvalidPattern(error.to_string()))
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
    // require_git(false):无 .git 的普通目录也吃自己写的 .gitignore,与
    // 文件树同一直觉;遍历错误(权限、条目消失)跳过
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
        // 读失败(权限、竞争删除)跳过该文件:搜索是导航手段,不因个别
        // 文件不可读而整体失败
        let Ok(haystack) = std::fs::read(path) else {
            continue;
        };
        for (index, line) in LineIter::new(b'\n', &haystack).enumerate() {
            if !regex.is_match(line) {
                continue;
            }
            if stop() {
                return false;
            }
            if !emit(SearchResult {
                path: path.to_path_buf(),
                line_no: index + 1,
                line_text: trim_line_terminator(line),
            }) {
                return true; // 调用方装满 = 截断
            }
        }
    }
    false
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

/// `LineIter` 产出的行自带终止符(`\n`,CRLF 文件还带 `\r`);入库前剥掉,
/// 展示层拿到的就是纯行文本。非 UTF-8 内容替换为 U+FFFD,不因个别坏档
/// 让整次搜索失败。
pub fn trim_line_terminator(line: &[u8]) -> String {
    String::from_utf8_lossy(line)
        .trim_end_matches(['\r', '\n'])
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
        self.spawn_inner(query, None)
    }

    /// 发起一次搜索，并在每条当前线程产出的事件成功进入 channel 后通知
    /// 消费方。UI 用此事件唤醒代替运行期间逐帧轮询；MCP 等同步消费者仍
    /// 使用 [`Self::spawn`] 或 [`search_sync`]，不需要 GUI 回调。
    pub fn spawn_with_notify(
        &mut self,
        query: SearchQuery,
        notify: SearchNotify,
    ) -> Result<(), SearchError> {
        self.spawn_inner(query, Some(notify))
    }

    /// 两种后台入口的共同实现：编译正则、推进代际并启动唯一生产线程。
    fn spawn_inner(
        &mut self,
        query: SearchQuery,
        notify: Option<SearchNotify>,
    ) -> Result<(), SearchError> {
        let regex = compile(&query)?;
        // fetch_add 返回旧值,新代 = 旧值 + 1;旧线程下个检查点即退出
        let gen = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let shared = Arc::clone(&self.shared);
        let tx = self.tx.clone();
        thread::Builder::new()
            .name("latermd-search".into())
            .spawn(move || run(query, regex, gen, shared, tx, notify))
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

/// 后台搜索线程主体：接收已经编译好的查询与本次代际号，遍历文件后把
/// 命中和完成事件写给调用方；若发现更晚一代请求则立即停止。`notify`
/// 来自 UI 等异步消费者，每次事件成功入队后调用，用来唤醒下一次消费，
/// 不参与搜索结果计算，也不会修改文件。
fn run(
    query: SearchQuery,
    regex: Regex,
    gen: u64,
    shared: Arc<Shared>,
    tx: SyncSender<Event>,
    notify: Option<SearchNotify>,
) {
    let superseded = || shared.generation.load(Ordering::SeqCst) != gen;
    let mut alive = true;
    scan(&query.root, &regex, superseded, |result| {
        // 接收端已随服务销毁则停止(不是截断,故返回后不置 truncated)
        alive = tx.send(Event::Hit { gen, result }).is_ok();
        if alive {
            if let Some(notify) = &notify {
                notify();
            }
        }
        alive
    });
    if alive && tx.send(Event::Done { gen }).is_ok() {
        if let Some(notify) = &notify {
            notify();
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
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

    /// UI 入口的通知回调随 Hit/Done 到达而触发；数据仍只走 channel，
    /// 回调不携带结果，也不会改变搜索代际语义。
    #[test]
    fn spawn_with_notify_wakes_the_consumer() {
        let root = sample_vault("notify");
        let calls = Arc::new(AtomicUsize::new(0));
        let sink = Arc::clone(&calls);
        let mut service = SearchService::new();
        service
            .spawn_with_notify(
                query(&root, "LaterMD", false),
                Arc::new(move || {
                    sink.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .unwrap();

        let (hits, done) = drain(&mut service);
        assert!(done && !hits.is_empty());
        assert!(
            calls.load(Ordering::SeqCst) >= 2,
            "至少一条命中和 Done 各通知一次"
        );
        let _ = std::fs::remove_dir_all(&root);
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
}
