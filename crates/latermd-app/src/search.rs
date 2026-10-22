//! 侧边栏搜索面板的状态机(P1 侧边栏搜索,docs/roadmap.md 阶段 3)。
//!
//! **遍历与匹配的核心已下沉到 `latermd-search`**(docs/mcp-plan.md §3 方案
//! A):MCP 的 `search_docs` 与侧边栏共用同一份实现,`.gitignore` 语义与
//! 截断上限才不会漂移。本模块只留**面板关注点**:输入镜像、去抖计时、
//! 结果缓存与状态机,三个原语(发起/接收/取消)直接 re-export。
//!
//! 取消的代际号语义、channel 背压、单文件上限等都在 `latermd-search` 的
//! 模块文档里,此处不重复。
//!
//! 根��录不存在时遍历为空、直接收到 `Done`(零命中),不视为错误——根
//! 目录的存在性由文件树侧保证。

use std::path::Path;
use std::time::Instant;

use crate::repaint::RepaintNotifier;

pub use latermd_search::{
    SearchError, SearchEvent, SearchQuery, SearchResult, SearchService, MAX_HITS,
};

/// 搜索面板状态机的当前档位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchStatus {
    /// 无搜索在途(初始、输入去抖中、被取消或换根复位)。
    Idle,
    /// 后台线程在跑、结果持续流入。
    Running,
    /// 一次搜索自然结束(与 Idle 分开,零命中时的「无结果」提示靠它区分
    /// 于「还没搜过」)。
    Finished,
    /// 正则编译失败;消息可直接展示,改输入即恢复。
    Invalid(String),
}

/// 搜索面板状态:服务句柄 + 输入镜像 + 去抖计时 + 结果缓存。
///
/// 输入文本(`query` / `case_insensitive`)由 `ui` 层的 TextEdit/checkbox
/// 原地改写(同编辑器缓冲的例外:这类控件必须拿 `&mut`),变更当帧发
/// `Message::SearchQueryChanged`;归约里取消旧搜索并把 `debounce_due`
/// 顺延 300ms,到点由归约侧(`layout.rs` 的 reduce,每帧必跑、不看
/// Search 页是否可见)发 `Message::SearchRequested`,归约再调
/// [`SearchState::start`]。
#[derive(Debug)]
pub struct SearchState {
    /// 后台搜索服务(单线程持有,见 [`SearchService`] 文档)。
    pub service: SearchService,
    /// 输入框文本(正则模式)。
    pub query: String,
    /// 大小写不敏感开关。
    pub case_insensitive: bool,
    /// 去抖到点时刻;输入每次变化顺延,发起成功或取消时清空。
    pub debounce_due: Option<Instant>,
    /// 已收到的命中(上限 [`MAX_HITS`])。
    pub hits: Vec<SearchResult>,
    /// 命中数到达上限被截断(提示行数据源)。
    pub truncated: bool,
    pub status: SearchStatus,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            service: SearchService::new(),
            query: String::new(),
            case_insensitive: false,
            debounce_due: None,
            hits: Vec::new(),
            truncated: false,
            status: SearchStatus::Idle,
        }
    }
}

impl SearchState {
    /// 输入变化(`Message::SearchQueryChanged` 的归约):取消旧搜索、
    /// 丢弃其结果,并把去抖计时顺延 `delay`。
    pub fn input_changed(&mut self, delay: std::time::Duration) {
        self.service.cancel();
        self.hits.clear();
        self.truncated = false;
        self.status = SearchStatus::Idle;
        self.debounce_due = Some(Instant::now() + delay);
    }

    /// 整体复位(文件树换根):作废服务、清空结果与去抖计时。与
    /// [`SearchState::input_changed`] 的区别是绝不重发——新根下的搜索由
    /// 用户重新输入发起,旧根的结果相对路径已失真,不该留着。
    pub fn reset(&mut self) {
        self.service.cancel();
        self.hits.clear();
        self.truncated = false;
        self.status = SearchStatus::Idle;
        self.debounce_due = None;
    }

    /// 去抖到点发起(`Message::SearchRequested` 的归约)。空模式与非法
    /// 正则同步处理,不进后台线程；命中和完成事件用 `repaint` 唤醒 UI。
    pub fn start(&mut self, root: &Path, repaint: RepaintNotifier) {
        self.debounce_due = None;
        if self.query.is_empty() {
            return;
        }
        let query = SearchQuery {
            root: root.to_path_buf(),
            pattern: self.query.clone(),
            case_insensitive: self.case_insensitive,
        };
        self.hits.clear();
        self.truncated = false;
        match self.service.spawn_with_notify(query, repaint.callback()) {
            Ok(()) => self.status = SearchStatus::Running,
            Err(SearchError::InvalidPattern(msg)) => self.status = SearchStatus::Invalid(msg),
            Err(error) => self.status = SearchStatus::Invalid(error.to_string()),
        }
    }

    /// 每帧归约末尾收一遍 channel(收空即返):命中入缓存,`Done` 落回
    /// Idle;到 [`MAX_HITS`] 即置截断标志并取消后台搜索。
    pub fn poll_hits(&mut self) {
        while let Some(event) = self.service.try_recv() {
            match event {
                SearchEvent::Hit(hit) => {
                    if self.hits.len() >= MAX_HITS {
                        self.truncated = true;
                        self.service.cancel();
                        self.status = SearchStatus::Finished;
                        return;
                    }
                    self.hits.push(hit);
                }
                SearchEvent::Done => {
                    self.status = SearchStatus::Finished;
                    return;
                }
            }
        }
    }

    /// 结果是否仍在流入。仅供测试等待；生产界面由事件通知唤醒。
    #[cfg(test)]
    pub fn is_running(&self) -> bool {
        self.status == SearchStatus::Running
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 进程内唯一的临时目录;测试自删。
    fn temp_vault(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("latermd-searchstate-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 轮询到结果上限即截断并取消后台搜索:缓存恰为 MAX_HITS 条,标志
    /// 置位,状态回 Finished,之后不再有事件。
    #[test]
    fn poll_hits_truncates_at_cap_and_cancels() {
        let root = temp_vault("truncate");
        let lines = "latermd 行\n".repeat(MAX_HITS + 50);
        std::fs::write(root.join("many.md"), lines).unwrap();

        let mut search = SearchState {
            query: "latermd".into(),
            ..SearchState::default()
        };
        search.start(&root, RepaintNotifier::default());
        assert_eq!(search.status, SearchStatus::Running);

        let deadline = Instant::now() + Duration::from_secs(10);
        while search.is_running() && Instant::now() < deadline {
            search.poll_hits();
            if search.is_running() {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(search.status, SearchStatus::Finished, "截断即本轮结束");
        assert_eq!(search.hits.len(), MAX_HITS);
        assert!(search.truncated);

        // 已取消:不再有任何事件(含 Done)
        std::thread::sleep(Duration::from_millis(20));
        search.poll_hits();
        assert_eq!(search.hits.len(), MAX_HITS, "截断后不再追加");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 面板状态机:输入变化取消旧搜索并顺延去抖;空模式不发起;非法
    /// 正则同步落 Invalid 且不产生事件。
    #[test]
    fn search_state_debounce_and_invalid_pattern() {
        let root = temp_vault("state-machine");
        std::fs::write(root.join("a.md"), "LaterMD 是 Markdown 工作台\n").unwrap();
        let mut search = SearchState::default();

        // 输入变化:清空 + 计时置位
        search.input_changed(Duration::from_millis(300));
        assert!(search.debounce_due.is_some());
        assert_eq!(search.status, SearchStatus::Idle);

        // 空模式:start 是无操作,计时清空
        search.start(&root, RepaintNotifier::default());
        assert_eq!(search.debounce_due, None);
        assert_eq!(search.status, SearchStatus::Idle);
        assert!(search.hits.is_empty());

        // 非法正则:同步 Invalid,不进后台
        search.query = "(".into();
        search.input_changed(Duration::from_millis(300));
        search.start(&root, RepaintNotifier::default());
        assert!(matches!(search.status, SearchStatus::Invalid(_)));
        assert_eq!(search.service.try_recv(), None, "发起失败不应有事件");

        // 合法模式:Running,自然结束落 Finished,命中保留在缓存
        search.query = "LaterMD".into();
        search.start(&root, RepaintNotifier::default());
        assert_eq!(search.status, SearchStatus::Running);
        let deadline = Instant::now() + Duration::from_secs(10);
        while search.is_running() && Instant::now() < deadline {
            search.poll_hits();
            if search.is_running() {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(search.status, SearchStatus::Finished);
        assert_eq!(search.hits.len(), 1);
        assert_eq!(search.hits[0].line_no, 1);

        // 再输入变化:旧结果清空、服务作废
        search.input_changed(Duration::from_millis(300));
        assert!(search.hits.is_empty());
        assert_eq!(search.status, SearchStatus::Idle);
        let _ = std::fs::remove_dir_all(&root);
    }
}
