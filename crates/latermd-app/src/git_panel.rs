//! Git 面板状态(P2 版本层,docs/roadmap.md 阶段 4):[`latermd_git`] 的
//! UI 接驳层,归约持有、渲染只读(adr-005 的 logic/ui 二分)。
//!
//! - **刷新时机**:节流轮询,而非每帧 `git status`。间隔 [`REFRESH_INTERVAL`]
//!   (3s),由归约侧(`ui::layout::reduce`)到点触发;换根、切到 Git 页与
//!   回滚完成走同一入口立即刷新。
//!
//!   **轮询走后台线程**([`GitPanelState::start_refresh`]),交互触发的
//!   立即刷新走同步入口([`GitPanelState::refresh`])。
//!
//!   > 2026-10-17 修订:原文判定「status/log 是毫秒级本地读,不值得上后台
//!   > 线程」,被实测推翻 —— 730 提交的仓库上,`discover+status+log` 一轮
//!   > **12ms**,带「文件历史」(`file_log` 内部扫 2000 个提交做 tree diff)
//!   > 一轮 **86ms**,在 UI 线程同步跑就是每 3 秒掉 1 帧到 5 帧。
//!   > IO 采集仍是同一份 `collect_git_data`,两条路径共用。
//!
//!   后台化的三条纪律(与 `mcp` / 反向链接面板同款三段式:发起 → 轮询 →
//!   收口):① 代际号 `refresh_seq`,迟到的结果丢弃;② 断连落降级,绝不留
//!   「刷新中」的卡死态;③ 在途期间每 50ms 要一帧收结果,收完即回到 3s
//!   长间隔,egui 仍收敛到深度空闲。
//!
//!   轮询**不显示「刷新中」**:每 3 秒闪一下的指示器只是噪声,手工刷新
//!   按钮本来就是同步的即时反馈。
//! - **条数上限**:改动列表截断在 [`latermd_git::DEFAULT_STATUS_LIMIT`]
//!   (500,与文件树/搜索的截断先例同量级),超出计 `truncated`、渲染提示
//!   行;被截断文件无角标、不可选中/回滚。防的是超大仓库的无界列表卡帧
//!   (log/diff 各有 50 条/64KB 上限,status 同款口径)。
//! - **降级**:文件树根不是 git 仓库时 `error` 置文案、角标与列表清空,
//!   Git 页显示提示而非报错;降级后**停止轮询**(重探由换根/切 Git 页触发,
//!   egui 得以收敛到深度空闲),无根时静默清空、由渲染层给引导。
//! - **回滚**:[`latermd_git::checkout_file`] 是 latermd-git 唯一写操作,
//!   本状态只把它暴露在确认模态之后(`confirm_checkout` 由 UI 点按钮置入,
//!   用户显式确认才执行)。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use latermd_git::{CommitFileChange, CommitInfo, FileDiff, FileStatus, StatusKind};

/// 主工作区当前正在审阅的 diff 来源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffTarget {
    /// 当前工作区相对 HEAD 的未提交改动。
    WorkingTree { path: String },
    /// 某次历史提交相对其第一父提交的改动。
    Commit { hash: String, path: String },
}

/// 单文件历史查询结果([`latermd_git::file_log`] 的返回 + 查询上下文)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHistory {
    /// 查询的文件(相对仓库根)。
    pub path: String,
    /// 改动过该文件的提交,新在前;条数上限同 [`latermd_git::DEFAULT_LOG_LIMIT`]。
    pub commits: Vec<CommitInfo>,
    /// 查询失败时的用户可见文案;成功为 `None`。
    pub error: Option<String>,
}

/// 当前展开的历史提交及其文件清单。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistorySelection {
    /// 提交元数据。
    pub commit: CommitInfo,
    /// 比较基准；初始提交没有父提交，表示空树。
    pub parent: Option<CommitInfo>,
    /// 该提交涉及的文件。
    pub files: Vec<CommitFileChange>,
    /// 超出列表上限的文件数。
    pub truncated: usize,
    /// 加载失败时的用户可见文案。
    pub error: Option<String>,
}

/// Git 状态自动刷新间隔。3s 在「角标足够新」与「零感开销」之间取中:
/// 每次刷新是两次毫秒级本地读(status + log),空闲时每 3s 唤醒一帧。
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(3);

/// Git 页 diff 区的视图模式(#53 M2):双栏对比(默认)或统一文本。
///
/// 会话内状态,**不持久化**——重启回默认双栏;取舍见
/// docs/decisions-pending #100。统一视图沿用既有文本渲染路径,行为不变。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DiffView {
    /// 双栏对比(默认):GitHub 式左右对齐,左删右增,行对配对见
    /// `git_split_diff`。
    #[default]
    Split,
    /// 统一文本:既有单栏路径(`Self::diff` 的逐行着色渲染)。
    Unified,
}

/// Git 面板状态:一次刷新的快照(status/log/diff 缓存)+ 确认式回滚。
///
/// 全部字段由归约维护,渲染层只读。`repo_root` 是 [`latermd_git::discover`]
/// 的结果——文件树根可能是仓库子目录,状态路径统一记「相对仓库根」,
/// 角标按需拼成绝对路径与文件树条目匹配。
#[derive(Debug)]
pub struct GitPanelState {
    /// 仓库根(discover 结果);`None` = 无根 / 非 git 仓库 / 尚未刷过。
    pub repo_root: Option<PathBuf>,
    /// 降级文案(非 git 仓库等);`Some` = Git 页显示它,列表与历史为空。
    pub error: Option<String>,
    /// 改动列表(相对仓库根、按路径排序,至多
    /// [`latermd_git::DEFAULT_STATUS_LIMIT`] 条,超出计 [`Self::truncated`])。
    pub entries: Vec<FileStatus>,
    /// 超出条数上限被截断的改动数(渲染层给「…还有 N 项未显示」提示;
    /// 被截断文件无角标、不可选中/回滚,与文件树/搜索的同款降级)。
    pub truncated: usize,
    /// 状态角标:绝对路径 → 状态码,文件树 Files 页与 Git 页共用。
    pub badges: HashMap<PathBuf, StatusKind>,
    /// 近期提交(`latermd_git::log` 原样,新在前;log 失败静默为空——
    /// status 成功即仓库可用,历史失败无独立展示价值)。
    pub commits: Vec<CommitInfo>,
    /// 刷新时刻的 Unix 秒,相对时间渲染的基准(分钟级精度,无需逐帧取)。
    pub fetched_at: i64,
    /// 选中的文件(相对仓库根);`None` = 未选中,diff 区不显示。
    pub selected: Option<String>,
    /// 选中文件的 diff 文本(错误文案也进这里,只读区直接可见);
    /// 空串 = 无文本改动。
    pub diff: String,
    /// 选中文件的结构化行级 diff(`latermd_git::diff_file_lines` 的结果,
    /// #53 M2 双栏视图的数据源);`None` = 未取到(选中失效或结构化 API
    /// 失败——后者时双栏回落渲染 `diff` 文本,错误文案仍进 diff 区可见,
    /// 现状口径不破)。与 `diff` 在 [`Self::reload_diff`] 同步双取。
    pub diff_lines: Option<FileDiff>,
    /// diff 区视图模式(#53 M2):双栏(默认)/统一;会话内切换,不持久化。
    pub diff_view: DiffView,
    /// 主工作区当前 diff 来源;`None` 表示尚未选中文件。
    pub diff_target: Option<DiffTarget>,
    /// 当前展开的历史提交及其文件清单。
    pub history: Option<HistorySelection>,
    /// 单文件历史(「文件历史」分区);`None` = 还没查过(渲染层
    /// 给「右键文件 → 查看文件历史」引导)。
    pub file_history: Option<FileHistory>,
    /// [`Self::history_target`] 上次同步到的当前文档(相对仓库根);
    /// 文档没换就不动「文件历史」——保住右键「查看文件历史」等显式查询
    /// 不被轮询顶掉,换了文档才跟随加载新文档的历史。
    pub(crate) file_history_synced_doc: Option<String>,
    /// 是否把选中的 diff 放进主工作区审阅。会话内状态,重启回编辑器。
    pub diff_workspace_open: bool,
    /// 待确认回滚的文件(相对仓库根);`Some` = 确认模态可见。
    pub confirm_checkout: Option<String>,
    /// 下次自动刷新时刻;到点由归约侧轮询触发(见模块文档)。
    pub refresh_due: Instant,
    /// 后台刷新在途(驱动"刷新中"指示与在途期间的要帧)。
    pub(crate) refreshing: bool,
    /// 后台刷新的代际号:迟到结果按号丢弃(与反向链接的 `scan_seq` 同款)。
    ///
    /// `pub(crate)` 只为让测试能用 `..GitPanelState::default()` 构造 ——
    /// 私有字段进不了函数式记录更新语法。它们不该被业务代码读写。
    pub(crate) refresh_seq: u64,
    /// 后台刷新的接收端;每条 channel 只交出一个结果。
    pub(crate) refresh_rx: Option<Receiver<(u64, Result<GitRefreshData, String>)>>,
}

/// 本次刷新对「改动列表选中项」的处置。
///
/// 三态而非二态:刷新前**没有选中项**(用户正在看某条历史提交的文件,
/// 见 [`GitPanelState::select_commit_file`])与「选中项被外部提交/回滚顶
/// 掉」是两件事 —— 前者必须什么都不动,否则 3 秒轮询会把用户刚打开的
/// 历史 diff 关掉(2026-10-17 修:早期版本把「未选中」当成「已失效」
/// 处理,轮询静默关闭了提交文件审阅工作区)。
pub(crate) enum SelectionOutcome {
    /// 刷新前没有选中改动列表项:选中区与主工作区审阅态都保持原样。
    Untouched,
    /// 原选中项仍在改动列表里:连带取回它的 diff(内容随编辑而变)。
    /// 路径不必带回 —— 它没变,仍是在 [`GitPanelState::selected`]。
    Kept {
        diff: String,
        lines: Option<FileDiff>,
    },
    /// 原选中项已离开改动列表:选中、diff 与工作区审阅态一并作废。
    Dropped,
}

/// 一次刷新的**全部 IO 产物**:discover → status → (选中项仍在则读 diff)
/// → log → 跟随当前文档的文件历史。
///
/// 采集只有这一份实现([`collect_git_data`]):同步入口与后台线程共用,
/// 防两套 discover/status/log 顺序漂移(与 `latermd-search` 的
/// `walk_markdown` 同款纪律)。
pub(crate) struct GitRefreshData {
    repo_root: PathBuf,
    entries: Vec<FileStatus>,
    truncated: usize,
    commits: Vec<CommitInfo>,
    selection: SelectionOutcome,
    /// 跟随当前文档的文件历史;`None` = 本次不查(文档没换)。
    file_history: Option<FileHistory>,
}

/// 采集一次刷新的全部 IO。**不触碰任何面板状态**,故可安全跑在后台线程。
fn collect_git_data(
    tree_root: &Path,
    selected: Option<&str>,
    tracked_history: Option<&str>,
) -> Result<GitRefreshData, String> {
    let repo_root = latermd_git::discover(tree_root)?;
    let mut snapshot = latermd_git::status(&repo_root, latermd_git::DEFAULT_STATUS_LIMIT)?;
    // draft 是编辑器的私有防丢镜像(#18),不是用户眼里的「仓库改动」:
    // git2 的 untracked 会如实列出它(git status 同样列),这里按产品口径
    // 滤掉 —— 与文件树/搜索的过滤同一精神。被滤条目仍占 status 的条数
    // 上限槽位;draft 数量至多等于打开的脏标签数,不为它引入第二次查询。
    snapshot
        .entries
        .retain(|entry| !entry.path.ends_with(crate::state::DRAFT_SUFFIX));
    // 选中项仍留在列表里才顺带取回它的 diff(内容随编辑而变);已离开列表
    // 则作废;刷新前就未选中则什么都不做。
    let selection = match selected {
        None => SelectionOutcome::Untouched,
        Some(path) if snapshot.entries.iter().any(|entry| entry.path == path) => {
            SelectionOutcome::Kept {
                // 错误文案进只读区,用户可见(与 `reload_diff` 同口径)
                diff: latermd_git::diff_file(&repo_root, path).unwrap_or_else(|error| error),
                lines: latermd_git::diff_file_lines(&repo_root, path).ok(),
            }
        }
        Some(_) => SelectionOutcome::Dropped,
    };
    let file_history = tracked_history.map(|path| {
        match latermd_git::file_log(&repo_root, path, latermd_git::DEFAULT_LOG_LIMIT) {
            Ok(commits) => FileHistory {
                path: path.to_owned(),
                commits,
                error: None,
            },
            Err(error) => FileHistory {
                path: path.to_owned(),
                commits: Vec::new(),
                error: Some(error),
            },
        }
    });
    Ok(GitRefreshData {
        commits: latermd_git::log(&repo_root, latermd_git::DEFAULT_LOG_LIMIT).unwrap_or_default(),
        repo_root,
        entries: snapshot.entries,
        truncated: snapshot.truncated,
        selection,
        file_history,
    })
}

impl Default for GitPanelState {
    /// 空快照;`refresh_due` 取当前时刻——首轮轮询即刻到点,有根就先刷一次。
    fn default() -> Self {
        Self {
            repo_root: None,
            error: None,
            entries: Vec::new(),
            truncated: 0,
            badges: HashMap::new(),
            commits: Vec::new(),
            fetched_at: 0,
            selected: None,
            diff: String::new(),
            diff_lines: None,
            diff_view: DiffView::Split,
            diff_target: None,
            history: None,
            file_history: None,
            file_history_synced_doc: None,
            diff_workspace_open: false,
            confirm_checkout: None,
            refresh_due: Instant::now(),
            refreshing: false,
            refresh_seq: 0,
            refresh_rx: None,
        }
    }
}

impl GitPanelState {
    /// 立即刷新(换根、切到 Git 页、回滚完成、顶部刷新按钮共用):discover
    /// → status → log → (跟随查)文件历史,任一步失败即降级并把旧快照作废;
    /// `tree_root` 为 `None` 时静默清空。选中项若已离开改动列表(外部提交
    /// /回滚)一并失效。
    ///
    /// `doc_rel` 是当前文档换算的相对仓库根路径(无文档 `None`);文件历史
    /// 的跟随由 [`Self::history_target`] 统一裁决(无 IO)。
    pub fn refresh(&mut self, tree_root: Option<&Path>, doc_rel: Option<String>) {
        self.refresh_due = Instant::now() + REFRESH_INTERVAL;
        // 同步刷新拿到的是最新数据:作废在途的后台结果,免得它晚一步盖回来
        // (用户切到 Git 页/点刷新时,后台那一轮多半刚开始)。
        self.refresh_rx = None;
        self.refreshing = false;
        let Some(root) = tree_root else {
            self.degrade(None);
            return;
        };
        let history = self.history_target(doc_rel);
        let selected = self.selected.clone();
        match collect_git_data(root, selected.as_deref(), history.as_deref()) {
            Ok(data) => self.apply_refresh(data),
            Err(error) => self.degrade(Some(error)),
        }
    }

    /// 异步发起一次刷新(**轮询路径专用**):IO 全交给后台线程。
    ///
    /// 实测(730 提交的仓库):一轮 `discover+status+log` 12ms;若「文件历史」
    /// 正在跟随当前文档,再加 `file_log` 一轮到 **86ms** —— 在 UI 线程同步
    /// 跑就是每 3 秒掉 1 帧到 5 帧,而用户此刻只是在写字。
    ///
    /// 交互触发的立即刷新仍走同步的 [`Self::refresh`](两者共用
    /// `collect_git_data`):用户点了按钮就该当场看到结果,引入"加载中"反
    /// 而让单击动作失去反馈。
    pub fn start_refresh(&mut self, tree_root: Option<&Path>, doc_rel: Option<String>) {
        self.refresh_due = Instant::now() + REFRESH_INTERVAL;
        let Some(root) = tree_root else {
            self.degrade(None);
            return;
        };
        // 文件历史只能**先记账、后加载**:记账(改同步位)是纯内存操作,
        // 必须留在本线程 —— 后台线程拿到的是结果的一份拷贝,改不到状态;
        // 而加载本身正是那 86ms 的 IO,必须留在后台。故此处只取目标。
        let (selected, tracked) = (self.selected.clone(), self.history_target(doc_rel));
        let root = root.to_path_buf();
        self.refresh_seq += 1;
        let seq = self.refresh_seq;
        let (tx, rx) = mpsc::channel();
        let spawn = std::thread::Builder::new()
            .name("latermd-git-refresh".to_owned())
            .spawn(move || {
                let outcome = collect_git_data(&root, selected.as_deref(), tracked.as_deref());
                let _ = tx.send((seq, outcome));
            });
        match spawn {
            Ok(_join) => {
                self.refresh_rx = Some(rx);
                self.refreshing = true;
            }
            Err(error) => {
                // spawn 失败不留"刷新中"的卡死态:当场降级,下一轮轮询再试
                self.refresh_rx = None;
                self.degrade(Some(format!("Git 刷新线程启动失败: {error}")));
            }
        }
    }

    /// 每帧归约末尾收一次后台结果:非阻塞;代际号不匹配的迟到结果丢弃。
    /// 断连却没等到结果(worker 异常退出)落降级 —— 「刷新中」不能卡死。
    pub fn poll_refresh(&mut self) {
        let Some(rx) = &self.refresh_rx else {
            return;
        };
        match rx.try_recv() {
            Ok((seq, result)) => {
                // 这条 channel 已交出唯一结果(旧代迟到也一并收口)
                self.refresh_rx = None;
                self.refreshing = false;
                if seq == self.refresh_seq {
                    match result {
                        Ok(data) => self.apply_refresh(data),
                        Err(error) => self.degrade(Some(error)),
                    }
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.refresh_rx = None;
                self.refreshing = false;
                self.degrade(Some("Git 刷新意外中断".to_owned()));
            }
        }
    }

    /// 应用一次采集结果(同步/异步共用):入库时刻才是 `fetched_at`。
    fn apply_refresh(&mut self, data: GitRefreshData) {
        self.fetched_at = unix_now();
        self.repo_root = Some(data.repo_root.clone());
        self.error = None;
        self.badges = data
            .entries
            .iter()
            .map(|entry| (data.repo_root.join(&entry.path), entry.code))
            .collect();
        self.entries = data.entries;
        self.truncated = data.truncated;
        self.commits = data.commits;
        match data.selection {
            SelectionOutcome::Untouched => {}
            SelectionOutcome::Kept { diff, lines } => {
                self.diff = diff;
                self.diff_lines = lines;
            }
            SelectionOutcome::Dropped => {
                // 选中项已离开改动列表(外部提交/回滚):连工作区审阅态一起作废
                self.selected = None;
                self.diff.clear();
                self.diff_lines = None;
                self.diff_target = None;
                self.diff_workspace_open = false;
            }
        }
        if let Some(history) = data.file_history {
            self.file_history = Some(history);
        }
    }

    /// 本次刷新该查哪个文件的历史,顺带把文档同步位推进到 `doc_rel`
    /// (**无 IO**,同步/异步两条路径共用,防跟随策略漂移)。
    ///
    /// * 文档没换:沿用「自动跟随当前文档」的那条(见 [`Self::tracked_history`]);
    /// * 文档换了(含换后无文档):同步位跟随,本次查新文档;旧的历史结果
    ///   保留到新结果取回再替换(后台路径下不会有一帧显示「空历史」)。
    ///
    /// 换 tab 时 `State` 主动把同步位清 `None`,下一次刷新因此必然走
    /// 「换了文档」分支,跟随新文档。
    fn history_target(&mut self, doc_rel: Option<String>) -> Option<String> {
        if doc_rel == self.file_history_synced_doc {
            self.tracked_history()
        } else {
            self.file_history_synced_doc = doc_rel.clone();
            doc_rel
        }
    }

    /// 本次刷新要跟随查历史的文件:只有「自动跟随当前文档」的那条才随
    /// 刷新重查。右键显式查看的另一文件由
    /// `file_history.path != file_history_synced_doc` 标记,保留给用户连续
    /// 浏览;否则 3 秒轮询会把显式选择悄悄顶回当前 tab。
    fn tracked_history(&self) -> Option<String> {
        self.file_history
            .as_ref()
            .map(|history| history.path.clone())
            .filter(|path| self.file_history_synced_doc.as_deref() == Some(path.as_str()))
    }

    /// 选中改动列表里的一个文件(`Message::GitFileSelected` 的归约):
    /// 选中并读 diff。不在当前列表里的路径(过期消息)忽略。
    pub fn select(&mut self, path: &str) {
        if !self.entries.iter().any(|entry| entry.path == path) {
            return;
        }
        self.selected = Some(path.to_owned());
        self.history = None;
        self.diff_target = Some(DiffTarget::WorkingTree {
            path: path.to_owned(),
        });
        self.diff_workspace_open = true;
        self.reload_diff();
        // 「文件历史」分区跟随选中项:选中谁就查谁的历史
        self.load_file_history(path);
    }

    /// 查询单文件的全部修改记录(`latermd_git::file_log`,「文件历史」
    /// 分区的数据源)。无仓库根(未刷新/降级)时留错误文案,
    /// 不 panic;查询失败同样留文案,由渲染层显示。
    pub fn load_file_history(&mut self, path: &str) {
        let result = match self.repo_root.clone() {
            None => Err("当前目录不在 Git 仓库内".to_owned()),
            Some(root) => latermd_git::file_log(&root, path, latermd_git::DEFAULT_LOG_LIMIT),
        };
        self.file_history = Some(match result {
            Ok(commits) => FileHistory {
                path: path.to_owned(),
                commits,
                error: None,
            },
            Err(error) => FileHistory {
                path: path.to_owned(),
                commits: Vec::new(),
                error: Some(error),
            },
        });
    }

    /// 加载一条历史提交的文件清单，不改变工作区文件选择。
    pub fn select_commit(&mut self, root: &Path, hash: &str) {
        if self
            .history
            .as_ref()
            .is_some_and(|history| history.commit.hash == hash)
        {
            self.history = None;
            return;
        }
        match latermd_git::commit_files(root, hash, latermd_git::DEFAULT_STATUS_LIMIT) {
            Ok(files) => {
                self.history = Some(HistorySelection {
                    commit: files.commit,
                    parent: files.parent,
                    files: files.entries,
                    truncated: files.truncated,
                    error: None,
                });
            }
            Err(error) => {
                self.history = Some(HistorySelection {
                    commit: CommitInfo {
                        hash: hash.to_owned(),
                        short_hash: hash.chars().take(7).collect(),
                        subject: String::new(),
                        author: String::new(),
                        time: 0,
                    },
                    parent: None,
                    files: Vec::new(),
                    truncated: 0,
                    error: Some(error),
                });
            }
        }
    }

    /// 选择历史提交中的文件并把其 parent → commit diff 放进主工作区。
    pub fn select_commit_file(&mut self, root: &Path, hash: &str, path: &str) {
        let Some(history) = self.history.as_ref() else {
            return;
        };
        if history.commit.hash != hash || !history.files.iter().any(|file| file.path == path) {
            return;
        }
        match latermd_git::commit_file_diff(root, hash, path) {
            Ok(diff) => {
                self.selected = None;
                self.diff = diff.text;
                self.diff_lines = Some(diff.lines);
                self.diff_target = Some(DiffTarget::Commit {
                    hash: hash.to_owned(),
                    path: path.to_owned(),
                });
                self.diff_workspace_open = true;
            }
            Err(error) => {
                self.selected = None;
                self.diff = error;
                self.diff_lines = None;
                self.diff_target = Some(DiffTarget::Commit {
                    hash: hash.to_owned(),
                    path: path.to_owned(),
                });
                self.diff_workspace_open = true;
            }
        }
    }

    /// 关闭主工作区 Git 审阅,保留选中项与 diff 快照供再次打开。
    pub fn close_diff_workspace(&mut self) {
        self.diff_workspace_open = false;
    }
    /// 点「回滚此文件」(`Message::GitCheckoutRequested` 的归约):只置确认
    /// 模态。真正的写操作在用户显式确认之后([`Self::confirm_checkout`])。
    pub fn request_checkout(&mut self, path: String) {
        if self.entries.iter().any(|entry| entry.path == path) {
            self.confirm_checkout = Some(path);
        }
    }

    /// 确认模态取消(`Message::GitCheckoutCancelled` 的归约)。
    pub fn cancel_checkout(&mut self) {
        self.confirm_checkout = None;
    }

    /// 确认模态的「回滚」(`Message::GitCheckoutConfirmed` 的归约):执行
    /// latermd-git 唯一的写操作并立即刷新状态;模态总是关闭(成功失败都
    /// 不再挂着)。失败返回文案给调用方进提示行,刷新照做保真。
    pub fn confirm_checkout(&mut self, tree_root: Option<&Path>) -> Option<String> {
        let path = self.confirm_checkout.take()?;
        let root = self.repo_root.clone()?;
        let error = latermd_git::checkout_file(&root, &path).err();
        // 走**同步**刷新(用户点了按钮就该当场看到结果),且文档同步位保持
        // 不变 —— 回滚不换文档,文件历史沿用「自动跟随当前文档」的那条
        let doc_rel = self.file_history_synced_doc.clone();
        self.refresh(tree_root, doc_rel);
        error
    }

    /// 文件树角标查询:绝对路径在改动列表中的状态码。
    pub fn badge_for(&self, file: &Path) -> Option<StatusKind> {
        self.badges.get(file).copied()
    }

    /// 相对仓库根路径 → 绝对路径(`None` = 无仓库根)。确认模态与回滚
    /// 归约都用它比较「回滚目标是否编辑器当前文档」,与角标同一拼接口径。
    pub fn absolute_path(&self, rel: &str) -> Option<PathBuf> {
        self.repo_root.as_ref().map(|root| root.join(rel))
    }

    /// 绝对路径 → 相对仓库根路径(POSIX 分隔符);`None` = 无仓库根或该路径
    /// 不在仓库内。文件历史入口(文件树右键、当前文档)手上是绝对路径,
    /// 而 latermd-git 全部 API 只认相对仓库根的路径。
    pub fn relative_path(&self, absolute: &Path) -> Option<String> {
        let root = self.repo_root.as_ref()?;
        let relative = absolute.strip_prefix(root).ok()?;
        Some(relative.to_string_lossy().replace('\\', "/"))
    }

    /// 自动刷新是否到点(归约侧每帧问一次)。
    pub fn due(&self) -> bool {
        self.refresh_due <= Instant::now()
    }

    /// 距下次刷新的时长(egui `request_repaint_after` 的参数:空闲不来帧,
    /// 轮询依赖显式要帧)。
    pub fn until_refresh(&self) -> Duration {
        self.refresh_due.saturating_duration_since(Instant::now())
    }

    /// 作废旧快照;`error` 为 `None` 时是无根的静默清空。
    fn degrade(&mut self, error: Option<String>) {
        self.error = error;
        self.repo_root = None;
        self.entries.clear();
        self.truncated = 0;
        self.badges.clear();
        self.commits.clear();
        self.history = None;
        // 文件历史一并作废(换根/降级后旧路径不再有意义);同步位也清零,
        // 重探成功后按当前文档重新跟随
        self.file_history = None;
        self.file_history_synced_doc = None;
        self.selected = None;
        self.diff.clear();
        self.diff_lines = None;
        self.diff_target = None;
        self.diff_workspace_open = false;
        self.confirm_checkout = None;
    }

    /// 按当前选中项重读 diff(#53 M2 起双取):`diff_file` 的统一文本维持
    /// 既有口径(错误文案进 `diff` 只读区可见),`diff_file_lines` 的结构化
    /// 结果喂双栏视图。两 API 基于同一份 libgit2 diff,毫秒级本地读,
    /// 双取成本可忽略;结构化侧失败只置 `None`(双栏回落统一文本渲染)。
    fn reload_diff(&mut self) {
        match (&self.repo_root, &self.selected) {
            (Some(root), Some(path)) => {
                self.diff = match latermd_git::diff_file(root, path) {
                    Ok(text) => text,
                    Err(error) => error, // 错误文案进只读区,用户可见
                };
                self.diff_lines = latermd_git::diff_file_lines(root, path).ok();
            }
            _ => {
                self.diff.clear();
                self.diff_lines = None;
            }
        }
    }
}

/// 当前 Unix 秒;系统时钟早于 epoch(不可能的病态)按 0。
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// 建一次性临时仓库并提交一个文件(每次全新目录,测试间无共享状态)。
    /// fixture 用系统 git CLI 装配(与 latermd-git 同模式),被测逻辑走
    /// GitPanelState 自身。
    fn temp_repo(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("latermd-gitpanel-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("a.md"), "HEAD 版本\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "init"]);
        dir
    }

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(["-c", "user.name=LaterMD", "-c", "user.email=latermd@test"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} 失败: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// 刷新填充:改动列表、绝对路径角标与历史;子目录作根同样命中(discover)。
    #[test]
    fn refresh_populates_entries_badges_and_log() {
        let dir = temp_repo("refresh");
        std::fs::write(dir.join("a.md"), "改\n").unwrap(); // M
        std::fs::write(dir.join("new.md"), "?\n").unwrap(); // ?
        std::fs::create_dir_all(dir.join("docs")).unwrap();

        let mut git = GitPanelState::default();
        git.refresh(Some(&dir), None);
        assert_eq!(git.error, None);
        assert_eq!(git.repo_root.as_deref(), Some(dir.as_path()));
        let paths: Vec<&str> = git.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["a.md", "new.md"], "按路径排序");
        assert_eq!(git.badge_for(&dir.join("a.md")), Some(StatusKind::Modified));
        assert_eq!(
            git.badge_for(&dir.join("new.md")),
            Some(StatusKind::Untracked)
        );
        assert_eq!(git.badge_for(&dir.join("docs")), None, "目录无角标");
        assert_eq!(git.commits.len(), 1);
        assert_eq!(git.commits[0].subject, "init");
        assert!(git.fetched_at > 1_700_000_000);

        // 文件树根是仓库子目录:仍从仓库根取状态,角标按绝对路径命中
        git.refresh(Some(&dir.join("docs")), None);
        assert_eq!(git.repo_root.as_deref(), Some(dir.as_path()));
        assert_eq!(git.badge_for(&dir.join("a.md")), Some(StatusKind::Modified));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// draft 镜像不入改动列表(#18):git status 的 untracked 如实列出
    /// `*.latermd-draft`(2026-10-01 实测 `?? a.md.latermd-draft`),刷新处
    /// 按产品口径滤掉 —— 改动列表与文件树角标都不出现。
    #[test]
    fn refresh_hides_draft_files_from_changes_and_badges() {
        let dir = temp_repo("draft-filter");
        std::fs::write(dir.join("a.md"), "改\n").unwrap(); // M
        std::fs::write(dir.join("a.md.latermd-draft"), "自动保存的镜像\n").unwrap(); // ??

        let mut git = GitPanelState::default();
        git.refresh(Some(&dir), None);
        let paths: Vec<&str> = git.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["a.md"], "draft 不入改动列表");
        assert_eq!(git.badge_for(&dir.join("a.md")), Some(StatusKind::Modified));
        assert_eq!(
            git.badge_for(&dir.join("a.md.latermd-draft")),
            None,
            "draft 无角标,文件树不标脏"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 非 git 目录:降级文案 + 角标与历史清空(文件树无角标、Git 页提示)。
    #[test]
    fn refresh_on_non_git_dir_degrades() {
        let plain =
            std::env::temp_dir().join(format!("latermd-gitpanel-{}-plain", std::process::id()));
        let _ = std::fs::remove_dir_all(&plain);
        std::fs::create_dir_all(&plain).unwrap();

        let mut git = GitPanelState::default();
        git.refresh(Some(&plain), None);
        let error = git.error.as_deref().expect("降级文案");
        assert!(error.contains("不是"), "{error}");
        assert_eq!(git.repo_root, None);
        assert!(git.entries.is_empty() && git.badges.is_empty());
        assert!(git.commits.is_empty());

        // 从有状态降级:旧快照必须作废,不留误导性角标
        let dir = temp_repo("degrade-from");
        git.refresh(Some(&dir), None);
        assert!(git.badge_for(&dir.join("no-such.md")).is_none());
        let _ = std::fs::remove_dir_all(&plain);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 无根刷新:静默清空(无降级文案,由渲染层给「先选根目录」引导)。
    #[test]
    fn refresh_without_root_clears_silently() {
        let dir = temp_repo("rootless");
        let mut git = GitPanelState::default();
        git.refresh(Some(&dir), None);
        assert!(!git.entries.is_empty() || git.commits.is_empty() || git.badges.is_empty());

        git.refresh(None, None);
        assert_eq!(git.error, None, "无根不是错误");
        assert!(git.entries.is_empty() && git.badges.is_empty());
        assert!(git.commits.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 选中与 diff:列表内路径读出 diff 内容;列表外路径(过期消息)忽略。
    #[test]
    fn select_reads_diff_and_ignores_stale_paths() {
        let dir = temp_repo("select");
        std::fs::write(dir.join("a.md"), "HEAD 版本\n新行\n").unwrap();

        let mut git = GitPanelState::default();
        git.refresh(Some(&dir), None);
        git.select("a.md");
        assert_eq!(git.selected.as_deref(), Some("a.md"));
        assert!(git.diff_workspace_open, "选中文件自动进入主工作区审阅");
        assert!(git.diff.contains("+新行"), "{}", git.diff);

        git.close_diff_workspace();
        assert!(!git.diff_workspace_open, "关闭审阅不清除选中和 diff");
        assert_eq!(git.selected.as_deref(), Some("a.md"));
        git.select("a.md");
        assert!(git.diff_workspace_open, "再次选中可重新打开审阅");

        git.select("not-in-list.md");
        assert_eq!(
            git.selected.as_deref(),
            Some("a.md"),
            "过期路径不顶掉选中项"
        );

        // 无改动的文件不在列表里,选不中;列表外的干净文件没有 diff 区
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #53 M2 双取通道:选中后 `diff`(统一文本)与 `diff_lines`(结构化)
    /// 同帧就位且内容一致(同一处改动两套口径都可见);默认视图是双栏;
    /// 选中项随 clean 失效时两条通道一起清空。
    #[test]
    fn select_fills_both_diff_channels_and_split_is_default() {
        let dir = temp_repo("dual-diff");
        std::fs::write(dir.join("a.md"), "HEAD 版本\n新行\n").unwrap();

        let mut git = GitPanelState::default();
        assert_eq!(git.diff_view, DiffView::Split, "默认双栏");
        assert_eq!(git.diff_lines, None, "未选中无结构化 diff");
        git.refresh(Some(&dir), None);
        git.select("a.md");
        assert!(git.diff.contains("+新行"), "{}", git.diff);
        let structured = git.diff_lines.as_ref().expect("结构化通道就位");
        assert!(!structured.hunks.is_empty(), "同一处改动结构化可见");
        assert!(
            structured
                .hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .any(|line| line.kind == latermd_git::DiffLineKind::Added && line.text == "新行"),
            "{structured:?}"
        );

        // 回滚到 HEAD → 选中项失效,两条通道一起清空
        git.request_checkout("a.md".to_owned());
        git.confirm_checkout(Some(&dir));
        assert_eq!(git.selected, None);
        assert_eq!(git.diff, "");
        assert_eq!(git.diff_lines, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 历史链路:select_commit 展开文件清单(初始提交 parent=None);
    /// select_commit_file 打开主工作区、来源标记为 Commit、工作区选中
    /// 让位;坏 hash 留错误文案;切回工作区文件清历史展开。
    #[test]
    fn select_commit_expands_files_and_commit_file_opens_workspace() {
        use latermd_git::CommitChangeKind;
        let dir = temp_repo("history");
        std::fs::write(dir.join("b.md"), "新增\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "第二笔"]);
        std::fs::write(dir.join("a.md"), "改\n").unwrap();

        let mut git_state = GitPanelState::default();
        git_state.refresh(Some(&dir), None);
        let hash = git_state.commits[0].hash.clone();

        git_state.select_commit(&dir, &hash);
        let history = git_state.history.as_ref().expect("历史展开就位");
        assert_eq!(history.commit.subject, "第二笔");
        assert!(history.parent.is_some(), "第二笔有父提交");
        assert!(history.error.is_none());
        assert!(
            history
                .files
                .iter()
                .any(|file| file.path == "b.md" && file.kind == CommitChangeKind::Added),
            "{:?}",
            history.files
        );

        // 同一条历史再次点击只收起,不会重复加载或保持展开。
        git_state.select_commit(&dir, &hash);
        assert!(git_state.history.is_none(), "同一条历史应可折叠");
        git_state.select_commit(&dir, &hash);
        assert!(git_state.history.is_some(), "再次点击应重新展开");

        git_state.select_commit_file(&dir, &hash, "b.md");
        assert!(git_state.diff_workspace_open, "历史文件进入主工作区");
        assert_eq!(
            git_state.diff_target,
            Some(DiffTarget::Commit {
                hash: hash.clone(),
                path: "b.md".to_owned()
            })
        );
        assert_eq!(git_state.selected, None, "历史目标下工作区选中让位");
        assert!(git_state.diff.contains("+新增"), "{}", git_state.diff);
        assert!(git_state.diff_lines.is_some(), "双栏通道就位");

        // 坏 hash:错误文案进历史展开,不清旧状态
        git_state.select_commit(&dir, "not-a-hash");
        let history = git_state.history.as_ref().expect("失败也保留展开位");
        assert!(history.error.is_some(), "{:?}", history.error);

        // 切回工作区文件:历史展开清空,来源切回 WorkingTree
        git_state.refresh(Some(&dir), None);
        git_state.select("a.md");
        assert_eq!(git_state.history, None, "工作区选择清历史展开");
        assert_eq!(
            git_state.diff_target,
            Some(DiffTarget::WorkingTree {
                path: "a.md".to_owned()
            })
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回滚链:request 只置模态;confirm 恢复 HEAD 并刷新(改动列表里
    /// 选中项随 clean 失效清空);cancel 不动文件。
    #[test]
    fn checkout_request_confirm_and_cancel() {
        let dir = temp_repo("checkout");
        std::fs::write(dir.join("a.md"), "工作区乱改\n").unwrap();

        let mut git = GitPanelState::default();
        git.refresh(Some(&dir), None);
        git.select("a.md");

        git.request_checkout("a.md".to_owned());
        assert_eq!(
            git.confirm_checkout.as_deref(),
            Some("a.md"),
            "请求只置模态"
        );
        assert!(
            std::fs::read_to_string(dir.join("a.md"))
                .unwrap()
                .contains("乱改"),
            "确认前文件未被触碰"
        );

        git.cancel_checkout();
        assert_eq!(git.confirm_checkout, None);
        assert!(std::fs::read_to_string(dir.join("a.md"))
            .unwrap()
            .contains("乱改"));

        git.request_checkout("a.md".to_owned());
        assert_eq!(git.confirm_checkout(Some(&dir)), None, "成功无错误文案");
        assert_eq!(
            std::fs::read_to_string(dir.join("a.md")).unwrap(),
            "HEAD 版本\n",
            "确认后恢复 HEAD"
        );
        assert_eq!(git.confirm_checkout, None, "模态已关闭");
        assert!(git.entries.is_empty(), "回滚后工作区干净");
        assert_eq!(git.selected, None, "选中项随 clean 失效清空");
        assert!(git.badges.is_empty(), "角标同步清空");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 大量改动:条目截断到上限、truncated 计数就位,角标只覆盖保留的
    /// 条目(被截断文件无角标,由提示行兜底)。
    #[test]
    fn refresh_truncates_entries_beyond_limit() {
        let dir = temp_repo("truncate");
        for i in 0..(latermd_git::DEFAULT_STATUS_LIMIT + 3) {
            std::fs::write(dir.join(format!("f{i:03}.md")), "?\n").unwrap();
        }

        let mut git = GitPanelState::default();
        git.refresh(Some(&dir), None);
        assert_eq!(
            git.entries.len(),
            latermd_git::DEFAULT_STATUS_LIMIT,
            "条目截断到上限"
        );
        assert_eq!(git.truncated, 3, "超出部分计数");
        assert_eq!(git.badges.len(), latermd_git::DEFAULT_STATUS_LIMIT);
        assert_eq!(git.entries[0].path, "f000.md", "字典序最小的保留");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 文件历史:只含改动过该文件的提交;无仓库根留错误文案;选中改动
    /// 文件时无条件跟随;刷新重查已跟踪的文件。
    #[test]
    fn file_history_tracks_one_file_and_follows_selection() {
        let dir = temp_repo("file-history");
        std::fs::write(dir.join("b.md"), "二\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "加 b"]);
        std::fs::write(dir.join("a.md"), "改\n").unwrap();

        let mut git_state = GitPanelState::default();
        git_state.refresh(Some(&dir), None);
        assert_eq!(git_state.file_history, None, "未查过是空");

        git_state.load_file_history("a.md");
        let history = git_state.file_history.as_ref().expect("文件历史就位");
        assert_eq!(history.path, "a.md");
        assert_eq!(history.error, None);
        assert_eq!(
            history
                .commits
                .iter()
                .map(|commit| commit.subject.as_str())
                .collect::<Vec<_>>(),
            vec!["init"],
            "只含改动 a.md 的提交"
        );

        git_state.load_file_history("b.md");
        let b = git_state.file_history.as_ref().expect("b 的历史");
        assert_eq!(
            b.commits
                .iter()
                .map(|commit| commit.subject.as_str())
                .collect::<Vec<_>>(),
            vec!["加 b"]
        );

        // 选中改动文件:「文件历史」分区无条件跟随选中项
        git_state.select("a.md");
        assert_eq!(
            git_state.file_history.as_ref().map(|h| h.path.as_str()),
            Some("a.md"),
            "选中即查选中项的历史"
        );

        // 刷新重查已跟踪的文件:外部新提交后历史跟着变
        std::fs::write(dir.join("b.md"), "二改\n").unwrap();
        git(&dir, &["add", "b.md"]);
        git(&dir, &["commit", "-q", "-m", "再改 b"]);
        git_state.refresh(Some(&dir), None);
        let b = git_state.file_history.as_ref().expect("刷新后仍在");
        assert_eq!(b.path, "a.md", "刷新不清目标");
        assert_eq!(git_state.commits[0].subject, "再改 b", "仓库历史已含新提交");

        // 无仓库根:错误文案,不 panic
        let mut orphan = GitPanelState::default();
        orphan.load_file_history("a.md");
        let error = orphan
            .file_history
            .as_ref()
            .and_then(|history| history.error.clone())
            .expect("无仓库根留文案");
        assert!(error.contains("Git 仓库"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 绝对路径 → 相对仓库根路径:无仓库根 / 仓库外路径都是 `None`。
    #[test]
    fn relative_path_requires_repo_root_and_prefix() {
        let mut git = GitPanelState::default();
        assert_eq!(git.relative_path(Path::new("/repo/a.md")), None);
        git.repo_root = Some(PathBuf::from("/repo"));
        assert_eq!(
            git.relative_path(Path::new("/repo/docs/a.md")),
            Some("docs/a.md".to_owned())
        );
        assert_eq!(
            git.relative_path(Path::new("/elsewhere/a.md")),
            None,
            "仓库外路径"
        );
    }

    /// 相对路径 → 绝对路径:有仓库根才可换算。
    #[test]
    fn absolute_path_requires_repo_root() {
        let mut git = GitPanelState::default();
        assert_eq!(git.absolute_path("a.md"), None);
        git.repo_root = Some(PathBuf::from("/repo"));
        assert_eq!(
            git.absolute_path("docs/a.md"),
            Some(PathBuf::from("/repo/docs/a.md"))
        );
    }

    /// 到点与周期:refresh 顺延一个周期,due 到点翻转;until_refresh 单调递减。
    #[test]
    fn due_and_until_refresh_track_interval() {
        let dir = temp_repo("due");
        let mut git = GitPanelState::default();
        assert!(git.due(), "初始即到点(默认 Instant)");

        git.refresh(Some(&dir), None);
        assert!(!git.due(), "刷新后未到点");
        let first = git.until_refresh();
        assert!(first <= REFRESH_INTERVAL && first > REFRESH_INTERVAL / 2);
        std::thread::sleep(Duration::from_millis(20));
        assert!(git.until_refresh() < first, "剩余时长随时间递减");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 轮询到后台结果到位。小仓库 IO 是微秒级,这里只是把「结果终会到」
    /// 变成可断言的事实;超时报错而非静默放过(静默的超时会把"卡住"
    /// 洗成"通过了")。
    fn settle(git: &mut GitPanelState) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while git.refreshing {
            assert!(Instant::now() < deadline, "后台刷新超时未交回结果");
            git.poll_refresh();
            if git.refreshing {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    /// 直接往接收端塞一条结果(绕过线程与 IO),用于测代际号这条**单靠真实
    /// 仓库几乎走不到**的分支:`state_seq` 是面板当前的代际号,`result_seq`
    /// 是结果自称的代际号,两者不等即"迟到的结果"。
    fn inject(
        git: &mut GitPanelState,
        state_seq: u64,
        result_seq: u64,
        result: Result<GitRefreshData, String>,
    ) {
        let (tx, rx) = mpsc::channel();
        git.refresh_rx = Some(rx);
        git.refreshing = true;
        git.refresh_seq = state_seq;
        tx.send((result_seq, result)).unwrap();
    }

    /// 一条假结果:仓库根与改动列表都指向凭空的路径,便于断言"有没有入库"。
    fn fake_data(root: &str, entry: &str) -> GitRefreshData {
        GitRefreshData {
            repo_root: PathBuf::from(root),
            entries: vec![FileStatus {
                path: entry.to_owned(),
                code: StatusKind::Modified,
            }],
            truncated: 0,
            commits: Vec::new(),
            selection: SelectionOutcome::Untouched,
            file_history: None,
        }
    }

    /// 后台刷新与同步刷新落到**同一份快照**(两条路径共用 `collect_git_data`,
    /// 差别只在 IO 跑在哪个线程)。
    #[test]
    fn async_refresh_settles_to_the_same_snapshot_as_sync() {
        let dir = temp_repo("async-same");
        std::fs::write(dir.join("a.md"), "改\n").unwrap();
        std::fs::write(dir.join("new.md"), "?\n").unwrap();

        let mut sync = GitPanelState::default();
        sync.refresh(Some(&dir), None);

        let mut background = GitPanelState::default();
        background.start_refresh(Some(&dir), None);
        assert!(background.refreshing, "发起即在途");
        assert_eq!(background.repo_root, None, "IO 未回不先发半成品快照");
        settle(&mut background);

        assert!(!background.refreshing, "结果收到即收口");
        assert!(background.refresh_rx.is_none(), "接收端已回收");
        assert_eq!(background.repo_root, sync.repo_root);
        assert_eq!(background.entries, sync.entries);
        assert_eq!(background.badges, sync.badges);
        assert_eq!(background.commits, sync.commits);
        assert_eq!(background.truncated, sync.truncated);
        assert!(background.fetched_at > 1_700_000_000, "入库时刻已记");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// IO 失败(非 git 目录)同样收口:不留「刷新中」的卡死态,旧快照作废。
    #[test]
    fn async_refresh_on_non_git_dir_degrades_without_sticking() {
        let plain =
            std::env::temp_dir().join(format!("latermd-gitpanel-{}-nongit", std::process::id()));
        let _ = std::fs::remove_dir_all(&plain);
        std::fs::create_dir_all(&plain).unwrap();

        let mut git = GitPanelState::default();
        git.start_refresh(Some(&plain), None);
        settle(&mut git);
        assert!(!git.refreshing, "失败也要收口");
        assert!(git.refresh_rx.is_none(), "失败也回收接收端");
        assert!(git.error.is_some(), "降级文案就位: {:?}", git.error);
        assert_eq!(git.repo_root, None);
        assert!(git.entries.is_empty());
        let _ = std::fs::remove_dir_all(&plain);
    }

    /// 代际号:迟到的结果丢弃,当代结果入库。丢号是"换根/连开两次刷新"
    /// 下的真实风险,但真实仓库上根本不可复现 —— 故直接注入。
    #[test]
    fn stale_generation_result_is_discarded_but_current_one_applies() {
        let mut stale = GitPanelState::default();
        inject(&mut stale, 2, 1, Ok(fake_data("/ghost", "ghost.md")));
        stale.poll_refresh();
        assert!(!stale.refreshing, "迟到的结果也收口");
        assert!(stale.refresh_rx.is_none(), "迟到的结果也回收接收端");
        assert_eq!(stale.repo_root, None, "迟到的结果不得入库");
        assert!(stale.entries.is_empty(), "迟到的结果不得入库");

        let mut fresh = GitPanelState::default();
        inject(&mut fresh, 2, 2, Ok(fake_data("/ghost", "ghost.md")));
        fresh.poll_refresh();
        assert_eq!(fresh.repo_root.as_deref(), Some(Path::new("/ghost")));
        assert_eq!(fresh.entries.len(), 1, "同代际的结果正常入库");
        assert!(!fresh.refreshing);
    }

    /// worker 异常退出(通道断连且没拿到结果):落降级,而不是永远「刷新中」。
    #[test]
    fn disconnected_worker_degrades_instead_of_sticking() {
        let mut git = GitPanelState::default();
        let (tx, rx) = mpsc::channel::<(u64, Result<GitRefreshData, String>)>();
        drop(tx);
        git.refresh_rx = Some(rx);
        git.refreshing = true;
        git.refresh_seq = 1;
        git.repo_root = Some(PathBuf::from("/repo"));
        git.entries.push(FileStatus {
            path: "a.md".to_owned(),
            code: StatusKind::Modified,
        });

        git.poll_refresh();
        assert!(!git.refreshing, "断连不得卡在刷新中");
        assert!(git.refresh_rx.is_none());
        assert_eq!(git.error.as_deref(), Some("Git 刷新意外中断"));
        assert_eq!(git.repo_root, None, "旧快照一并作废");
        assert!(git.entries.is_empty());
    }

    /// 无选中时刷新**不得**关掉历史提交文件的审阅工作区:三条路径(同步
    /// 刷新 / 异步刷新)都要保住。早期版本把「未选中」当成「选中项已失效」
    /// 处理,3 秒轮询会静默关掉用户刚打开的 diff。
    #[test]
    fn refresh_without_selection_keeps_commit_file_workspace() {
        let dir = temp_repo("commit-workspace");
        std::fs::write(dir.join("b.md"), "新增\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "第二笔"]);
        std::fs::write(dir.join("a.md"), "改\n").unwrap();

        let mut git_state = GitPanelState::default();
        git_state.refresh(Some(&dir), None);
        let hash = git_state.commits[0].hash.clone();
        git_state.select_commit(&dir, &hash);
        git_state.select_commit_file(&dir, &hash, "b.md");
        let target = || {
            Some(DiffTarget::Commit {
                hash: hash.clone(),
                path: "b.md".to_owned(),
            })
        };
        assert_eq!(git_state.selected, None, "看历史文件时改动列表无选中");

        git_state.refresh(Some(&dir), None);
        assert!(git_state.diff_workspace_open, "同步刷新不动工作区");
        assert_eq!(git_state.diff_target, target());
        assert!(git_state.diff.contains("+新增"), "{}", git_state.diff);

        git_state.start_refresh(Some(&dir), None);
        settle(&mut git_state);
        assert!(git_state.diff_workspace_open, "异步刷新不动工作区");
        assert_eq!(git_state.diff_target, target());
        assert!(git_state.diff.contains("+新增"), "{}", git_state.diff);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 选中项被外部提交/回滚顶掉时,异步路径同样作废选中与工作区审阅。
    /// (上一条守的是「没选中」,这条守的是「选中过但没了」—— 两者共用
    /// 一个 `is_none()` 判断就会一起错。)
    #[test]
    fn async_refresh_drops_selection_that_left_the_list() {
        let dir = temp_repo("async-drop");
        std::fs::write(dir.join("a.md"), "改\n").unwrap();

        let mut panel = GitPanelState::default();
        panel.refresh(Some(&dir), None);
        panel.select("a.md");
        assert_eq!(panel.selected.as_deref(), Some("a.md"));
        assert!(panel.diff_workspace_open, "选中即进工作区");

        // 外部把改动提交掉:该文件离开改动列表
        git(&dir, &["add", "a.md"]);
        git(&dir, &["commit", "-q", "-m", "外部提交"]);
        panel.start_refresh(Some(&dir), None);
        settle(&mut panel);

        assert_eq!(panel.selected, None, "离开列表即作废");
        assert!(panel.entries.is_empty(), "工作区已干净");
        assert!(!panel.diff_workspace_open, "工作区审阅一并关闭");
        assert_eq!(panel.diff_target, None);
        assert!(panel.diff.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 同步刷新作废在途的后台结果:用户在途期间切到 Git 页(同步刷新)后,
    /// 后台那一轮的旧数据不得晚一步盖回来。
    #[test]
    fn sync_refresh_invalidates_in_flight_background_result() {
        let dir = temp_repo("in-flight");
        std::fs::write(dir.join("a.md"), "改\n").unwrap();

        let mut panel = GitPanelState::default();
        let (tx, rx) = mpsc::channel();
        panel.refresh_rx = Some(rx);
        panel.refreshing = true;
        panel.refresh_seq = 1;

        panel.refresh(Some(&dir), None);
        assert!(!panel.refreshing, "同步刷新即在途作废");
        assert!(panel.refresh_rx.is_none(), "接收端已丢弃");
        assert_eq!(panel.repo_root.as_deref(), Some(dir.as_path()));

        // 后台那一轮姗姗来迟:通道已断,它连送都送不进来(`send` 失败正是
        // worker 侧那句 `let _ = tx.send(..)` 的落点),新数据因此稳住
        let delivered = tx.send((1, Ok(fake_data("/ghost", "ghost.md"))));
        assert!(delivered.is_err(), "在途结果已无处可送");
        panel.poll_refresh();
        assert_eq!(
            panel.repo_root.as_deref(),
            Some(dir.as_path()),
            "同步刷新的新数据稳住"
        );
        assert_eq!(
            panel.entries.first().map(|entry| entry.path.as_str()),
            Some("a.md")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 文件历史跟随:异步路径与同步路径落到同一状态。记账(改同步位)必须
    /// 留在 UI 线程 —— 后台线程拿到的是状态的拷贝,改不到真状态。
    #[test]
    fn async_refresh_follows_document_switch_like_sync() {
        let dir = temp_repo("follow");
        std::fs::write(dir.join("b.md"), "二\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "加 b"]);

        let mut background = GitPanelState::default();
        background.refresh(Some(&dir), Some("a.md".to_owned()));
        assert_eq!(
            background.file_history.as_ref().map(|h| h.path.as_str()),
            Some("a.md"),
            "首次刷新跟随当前文档"
        );
        // 换文档:State 在切 tab 时把同步位清 None(见 `switch_active`)
        background.file_history_synced_doc = None;
        background.start_refresh(Some(&dir), Some("b.md".to_owned()));
        assert_eq!(
            background.file_history_synced_doc.as_deref(),
            Some("b.md"),
            "记账不等 IO,发起时即生效"
        );
        settle(&mut background);
        assert_eq!(
            background.file_history.as_ref().map(|h| h.path.as_str()),
            Some("b.md"),
            "新文档的历史由后台带回"
        );

        let mut sync = GitPanelState::default();
        sync.refresh(Some(&dir), Some("a.md".to_owned()));
        sync.file_history_synced_doc = None;
        sync.refresh(Some(&dir), Some("b.md".to_owned()));
        assert_eq!(
            sync.file_history.as_ref().map(|h| h.path.as_str()),
            Some("b.md")
        );
        assert_eq!(
            sync.file_history_synced_doc,
            background.file_history_synced_doc
        );

        // 右键显式查了另一个文件:文档没换,轮询不得把它顶回当前文档
        background.load_file_history("a.md");
        background.file_history_synced_doc = Some("b.md".to_owned());
        background.start_refresh(Some(&dir), Some("b.md".to_owned()));
        settle(&mut background);
        assert_eq!(
            background.file_history.as_ref().map(|h| h.path.as_str()),
            Some("a.md"),
            "显式查询不被 3 秒轮询顶掉"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
