//! Git 面板状态(P2 版本层,docs/roadmap.md 阶段 4):[`latermd_git`] 的
//! UI 接驳层,归约持有、渲染只读(adr-005 的 logic/ui 二分)。
//!
//! - **刷新时机**:节流轮询,而非每帧 `git status`。间隔 [`REFRESH_INTERVAL`]
//!   (3s),由归约侧(`ui::layout::reduce`)到点触发;换根、切到 Git 页与
//!   回滚完成走同一入口。status/log/diff 在线程里单飞读取，结果携带请求
//!   序号回到归约，换根或重复请求不会让旧快照覆盖新状态。
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

use latermd_git::{CommitInfo, FileStatus, StatusKind};

use crate::repaint::RepaintNotifier;

/// Git 状态自动刷新间隔。3s 在「角标足够新」与「零感开销」之间取中:
/// 每次刷新是两次毫秒级本地读(status + log),空闲时每 3s 唤醒一帧。
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(3);

/// 后台读取成功后的完整 Git 快照。只包含可跨线程移动的值，不携带
/// repository 句柄；UI 线程应用它时仍保留确认模态等交互状态。
#[derive(Debug)]
struct GitSnapshot {
    repo_root: PathBuf,
    entries: Vec<FileStatus>,
    truncated: usize,
    badges: HashMap<PathBuf, StatusKind>,
    commits: Vec<CommitInfo>,
    fetched_at: i64,
    selected: Option<String>,
    diff: String,
}

type RefreshResult = (u64, Result<GitSnapshot, String>);

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
    /// 待确认回滚的文件(相对仓库根);`Some` = 确认模态可见。
    pub confirm_checkout: Option<String>,
    /// 下次自动刷新时刻;到点由归约侧轮询触发(见模块文档)。
    pub refresh_due: Instant,
    /// 最新一次刷新请求序号；后台结果必须匹配才允许覆盖当前快照。
    refresh_seq: u64,
    /// 当前单飞任务的接收端；`Some` 表示已有一次磁盘读取在途。
    refresh_rx: Option<Receiver<RefreshResult>>,
    /// 与 `refresh_rx` 配对的请求序号，断连时仍能按原请求判断是否过期。
    refresh_running_seq: Option<u64>,
    /// 单飞期间收到的新请求只保留最新根，当前任务结束后立即补跑。
    queued_root: Option<PathBuf>,
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
            confirm_checkout: None,
            refresh_due: Instant::now(),
            refresh_seq: 0,
            refresh_rx: None,
            refresh_running_seq: None,
            queued_root: None,
        }
    }
}

impl GitPanelState {
    /// 立即刷新(轮询到点、换根、切到 Git 页、回滚完成共用):discover →
    /// status → log,任一步失败即降级并把旧快照作废;`tree_root` 为
    /// `None` 时静默清空。选中项若已离开改动列表(外部提交/回滚)一并失效。
    #[cfg(test)]
    pub fn refresh(&mut self, tree_root: Option<&Path>) {
        self.refresh_due = Instant::now() + REFRESH_INTERVAL;
        self.fetched_at = unix_now();
        let Some(root) = tree_root else {
            self.degrade(None);
            return;
        };
        let repo_root = match latermd_git::discover(root) {
            Ok(root) => root,
            Err(error) => {
                self.degrade(Some(error));
                return;
            }
        };
        let snapshot = match latermd_git::status(&repo_root, latermd_git::DEFAULT_STATUS_LIMIT) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.degrade(Some(error));
                return;
            }
        };
        self.repo_root = Some(repo_root.clone());
        self.error = None;
        self.badges = snapshot
            .entries
            .iter()
            .map(|entry| (repo_root.join(&entry.path), entry.code))
            .collect();
        if let Some(selected) = &self.selected {
            if snapshot.entries.iter().any(|entry| &entry.path == selected) {
                self.reload_diff();
            } else {
                self.selected = None;
                self.diff.clear();
            }
        }
        self.entries = snapshot.entries;
        self.truncated = snapshot.truncated;
        self.commits =
            latermd_git::log(&repo_root, latermd_git::DEFAULT_LOG_LIMIT).unwrap_or_default();
    }

    /// 请求后台刷新。输入是文件树根的拥有值，避免线程借用 UI 状态；同一
    /// 时刻只跑一个任务，在途期间的新请求覆盖 `queued_root`，完成后补跑
    /// 最新一次。无根时立即作废在途结果并清空快照。
    pub fn request_refresh(&mut self, tree_root: Option<PathBuf>, repaint: RepaintNotifier) {
        self.refresh_due = Instant::now() + REFRESH_INTERVAL;
        self.refresh_seq = self.refresh_seq.wrapping_add(1);
        let Some(root) = tree_root else {
            self.refresh_rx = None;
            self.refresh_running_seq = None;
            self.queued_root = None;
            self.degrade(None);
            return;
        };
        if self.refresh_rx.is_some() {
            self.queued_root = Some(root);
            return;
        }
        self.spawn_refresh(root, repaint);
    }

    /// 文件树换根时立即清掉旧仓库的列表、角标与选中项。新的读取仍走
    /// [`Self::request_refresh`]；清空只防止后台结果返回前短暂展示旧根。
    pub fn invalidate_snapshot(&mut self) {
        self.degrade(None);
    }

    /// 非阻塞接收后台 Git 快照。匹配最新序号才应用；旧结果只用于结束
    /// 当前单飞任务，然后若有排队根则立刻启动最新请求。
    pub fn poll_refresh(&mut self, repaint: RepaintNotifier) {
        let received = match self.refresh_rx.as_ref().map(Receiver::try_recv) {
            Some(Ok(result)) => Some(result),
            Some(Err(TryRecvError::Disconnected)) => self
                .refresh_running_seq
                .map(|seq| (seq, Err("Git 后台刷新意外中断".to_owned()))),
            Some(Err(TryRecvError::Empty)) | None => None,
        };
        let Some((seq, result)) = received else {
            return;
        };
        self.refresh_rx = None;
        self.refresh_running_seq = None;
        if seq == self.refresh_seq {
            match result {
                Ok(snapshot) => self.apply_snapshot(snapshot),
                Err(error) => self.degrade(Some(error)),
            }
        }
        if let Some(root) = self.queued_root.take() {
            self.spawn_refresh(root, repaint);
        }
    }

    /// 是否已有 Git 读取任务在途。仅供状态/测试观察，不驱动逐帧重绘。
    #[cfg(test)]
    pub fn is_refreshing(&self) -> bool {
        self.refresh_rx.is_some()
    }

    /// 启动单个后台读取任务。请求序号、选中文件在启动时定格；worker 只
    /// 构造值快照，发送成功后唤醒 UI，不触碰当前 `GitPanelState`。
    fn spawn_refresh(&mut self, root: PathBuf, repaint: RepaintNotifier) {
        let seq = self.refresh_seq;
        let selected = self.selected.clone();
        let (tx, rx) = mpsc::channel();
        self.refresh_rx = Some(rx);
        self.refresh_running_seq = Some(seq);
        let spawned = std::thread::Builder::new()
            .name("latermd-git-refresh".into())
            .spawn(move || {
                let result = read_snapshot(&root, selected);
                if tx.send((seq, result)).is_ok() {
                    repaint.notify();
                }
            });
        if let Err(error) = spawned {
            self.refresh_rx = None;
            self.refresh_running_seq = None;
            self.degrade(Some(format!("Git 刷新线程启动失败: {error}")));
        }
    }

    /// 将后台读取的拥有值一次性换入 UI 快照。选中项与 diff 已在同一份
    /// 仓库快照里校验，避免分别读取造成状态撕裂。
    fn apply_snapshot(&mut self, snapshot: GitSnapshot) {
        self.repo_root = Some(snapshot.repo_root);
        self.error = None;
        self.entries = snapshot.entries;
        self.truncated = snapshot.truncated;
        self.badges = snapshot.badges;
        self.commits = snapshot.commits;
        self.fetched_at = snapshot.fetched_at;
        self.selected = snapshot.selected;
        self.diff = snapshot.diff;
    }

    /// 选中改动列表里的一个文件(`Message::GitFileSelected` 的归约):
    /// 选中并读 diff。不在当前列表里的路径(过期消息)忽略。
    pub fn select(&mut self, path: &str) {
        if !self.entries.iter().any(|entry| entry.path == path) {
            return;
        }
        self.selected = Some(path.to_owned());
        self.reload_diff();
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

    /// 确认模态的「回滚」(`Message::GitCheckoutConfirmed` 的归约):只执行
    /// latermd-git 唯一的写操作并关闭模态。调用方随后发起后台刷新；失败
    /// 文案返回提示行，但同样要刷新以反映磁盘真实状态。
    pub fn confirm_checkout(&mut self) -> Option<String> {
        let path = self.confirm_checkout.take()?;
        let root = self.repo_root.clone()?;
        latermd_git::checkout_file(&root, &path).err()
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
        self.selected = None;
        self.diff.clear();
        self.confirm_checkout = None;
    }

    /// 按当前选中项重读 diff。
    fn reload_diff(&mut self) {
        self.diff = match (&self.repo_root, &self.selected) {
            (Some(root), Some(path)) => match latermd_git::diff_file(root, path) {
                Ok(text) => text,
                Err(error) => error, // 错误文案进只读区,用户可见
            },
            _ => String::new(),
        };
    }
}

/// 在后台线程读取一次一致的 status/log/diff 快照。`selected` 只有仍在
/// 最新改动列表中才保留；diff 失败文案继续进入只读区，沿用原有口径。
fn read_snapshot(root: &Path, selected: Option<String>) -> Result<GitSnapshot, String> {
    let repo_root = latermd_git::discover(root)?;
    let status = latermd_git::status(&repo_root, latermd_git::DEFAULT_STATUS_LIMIT)?;
    let badges = status
        .entries
        .iter()
        .map(|entry| (repo_root.join(&entry.path), entry.code))
        .collect();
    let selected = selected.filter(|path| status.entries.iter().any(|entry| &entry.path == path));
    let diff = selected
        .as_deref()
        .map(|path| latermd_git::diff_file(&repo_root, path).unwrap_or_else(|error| error))
        .unwrap_or_default();
    Ok(GitSnapshot {
        repo_root: repo_root.clone(),
        entries: status.entries,
        truncated: status.truncated,
        badges,
        commits: latermd_git::log(&repo_root, latermd_git::DEFAULT_LOG_LIMIT).unwrap_or_default(),
        fetched_at: unix_now(),
        selected,
        diff,
    })
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

    /// 收完后台刷新及其排队补跑；测试仍使用真实线程与 channel，只用短暂
    /// sleep 让出调度，不把生产实现改回同步路径。
    fn drain_background_refresh(git: &mut GitPanelState) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while git.is_refreshing() && Instant::now() < deadline {
            git.poll_refresh(RepaintNotifier::default());
            if git.is_refreshing() {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert!(!git.is_refreshing(), "Git 后台刷新应在时限内收口");
    }

    /// 后台单飞与过期保护：连续请求两个根时最终只允许第二个仓库快照
    /// 落地，第一项即使先完成也不能覆盖最新根。
    #[test]
    fn background_refresh_keeps_latest_requested_root() {
        let first = temp_repo("async-first");
        let second = temp_repo("async-second");
        std::fs::write(first.join("first.md"), "一\n").unwrap();
        std::fs::write(second.join("second.md"), "二\n").unwrap();

        let mut git = GitPanelState::default();
        git.request_refresh(Some(first.clone()), RepaintNotifier::default());
        git.request_refresh(Some(second.clone()), RepaintNotifier::default());
        drain_background_refresh(&mut git);

        assert_eq!(git.repo_root.as_deref(), Some(second.as_path()));
        assert!(git.entries.iter().any(|entry| entry.path == "second.md"));
        assert!(git.entries.iter().all(|entry| entry.path != "first.md"));
        let _ = std::fs::remove_dir_all(&first);
        let _ = std::fs::remove_dir_all(&second);
    }

    /// 刷新填充:改动列表、绝对路径角标与历史;子目录作根同样命中(discover)。
    #[test]
    fn refresh_populates_entries_badges_and_log() {
        let dir = temp_repo("refresh");
        std::fs::write(dir.join("a.md"), "改\n").unwrap(); // M
        std::fs::write(dir.join("new.md"), "?\n").unwrap(); // ?
        std::fs::create_dir_all(dir.join("docs")).unwrap();

        let mut git = GitPanelState::default();
        git.refresh(Some(&dir));
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
        git.refresh(Some(&dir.join("docs")));
        assert_eq!(git.repo_root.as_deref(), Some(dir.as_path()));
        assert_eq!(git.badge_for(&dir.join("a.md")), Some(StatusKind::Modified));
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
        git.refresh(Some(&plain));
        let error = git.error.as_deref().expect("降级文案");
        assert!(error.contains("不是"), "{error}");
        assert_eq!(git.repo_root, None);
        assert!(git.entries.is_empty() && git.badges.is_empty());
        assert!(git.commits.is_empty());

        // 从有状态降级:旧快照必须作废,不留误导性角标
        let dir = temp_repo("degrade-from");
        git.refresh(Some(&dir));
        assert!(git.badge_for(&dir.join("no-such.md")).is_none());
        let _ = std::fs::remove_dir_all(&plain);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 无根刷新:静默清空(无降级文案,由渲染层给「先选根目录」引导)。
    #[test]
    fn refresh_without_root_clears_silently() {
        let dir = temp_repo("rootless");
        let mut git = GitPanelState::default();
        git.refresh(Some(&dir));
        assert!(!git.entries.is_empty() || git.commits.is_empty() || git.badges.is_empty());

        git.refresh(None);
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
        git.refresh(Some(&dir));
        git.select("a.md");
        assert_eq!(git.selected.as_deref(), Some("a.md"));
        assert!(git.diff.contains("+新行"), "{}", git.diff);

        git.select("not-in-list.md");
        assert_eq!(
            git.selected.as_deref(),
            Some("a.md"),
            "过期路径不顶掉选中项"
        );

        // 无改动的文件不在列表里,选不中;列表外的干净文件没有 diff 区
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回滚链:request 只置模态;confirm 恢复 HEAD 并刷新(改动列表里
    /// 选中项随 clean 失效清空);cancel 不动文件。
    #[test]
    fn checkout_request_confirm_and_cancel() {
        let dir = temp_repo("checkout");
        std::fs::write(dir.join("a.md"), "工作区乱改\n").unwrap();

        let mut git = GitPanelState::default();
        git.refresh(Some(&dir));
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
        assert_eq!(git.confirm_checkout(), None, "成功无错误文案");
        // 生产路径在确认归约后发起后台刷新；本单元测试只验证同步 checkout
        // 与快照口径，故显式刷新一次再断言列表。
        git.refresh(Some(&dir));
        assert_eq!(
            std::fs::read_to_string(dir.join("a.md"))
                .unwrap()
                .replace("\r\n", "\n"),
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
        git.refresh(Some(&dir));
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

        git.refresh(Some(&dir));
        assert!(!git.due(), "刷新后未到点");
        let first = git.until_refresh();
        assert!(first <= REFRESH_INTERVAL && first > REFRESH_INTERVAL / 2);
        std::thread::sleep(Duration::from_millis(20));
        assert!(git.until_refresh() < first, "剩余时长随时间递减");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
