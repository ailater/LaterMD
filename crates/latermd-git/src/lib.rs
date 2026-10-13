#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! 只读 Git 能力(git2 封装,docs/roadmap.md 阶段 4「P2 版本层」)。
//!
//! 定位:以仓库根目录为参数的纯函数集合,供 UI 层查询状态、历史、diff、
//! blame,外加唯一一个写操作——丢弃工作区改动、恢复 HEAD 版本的单文件
//! checkout。不依赖 egui(铁律:业务逻辑不依赖 UI 框架,AGENTS.md §3)。
//!
//! 口径:
//! * 所有函数接受**仓库根**(.git 所在目录),不向上层目录探测;传入非
//!   git 目录一律返回 Err,由调用方降级为面板提示,绝不 panic。
//! * 未提交的工作区改动参与 status 与 diff;blame 只基于 HEAD 提交内容
//!   (libgit2 限制,工作区未提交的行不参与行级归属)。
//! * 错误类型是面向用户的中文描述,可直接上提示行。
//! * 不做 remote 操作(fetch/push/pull 不在 P2 范围),故 git2 关掉了
//!   ssh/https 传输 feature。

use std::fmt;
use std::path::{Path, PathBuf};

use git2::build::CheckoutBuilder;
use git2::{Delta, DiffFindOptions, DiffOptions, Patch, Repository, Signature, StatusOptions};

/// diff 输出的字节上限(截断保护,~64KB)。
const MAX_DIFF_BYTES: usize = 64 * 1024;
/// 截断时追加的提示行(与 MAX_DIFF_BYTES 对应,改上限记得同步文案)。
const TRUNCATION_NOTICE: &str = "\n…(diff 超过 64KB,已截断)\n";
/// pathspec 命中二进制文件(无文本 diff)时的占位输出。
const BINARY_NOTICE: &str = "(二进制文件,无文本 diff)\n";
/// 行级结构([`FileDiff`])命中二进制文件时的占位文案,渲染层直接显示;
/// 与 BINARY_NOTICE 是同一份字面量文案(测试
/// `notices_share_text_between_unified_and_structured` 钉住,改文案两处
/// 一起改)。注:diff_file 现状对二进制实际输出 libgit2 原生
/// "Binary files … differ" 文本,不走 BINARY_NOTICE 分支。
pub const DIFF_BINARY_PLACEHOLDER: &str = "(二进制文件,无文本 diff)";
/// 行级结构([`FileDiff`])超上限截断时的提示文案,渲染层追加;
/// 与 [`diff_file`] 的 TRUNCATION_NOTICE 同一来源(同上被测试钉住)。
pub const DIFF_TRUNCATION_PLACEHOLDER: &str = "…(diff 超过 64KB,已截断)";

/// 单文件状态码,与 `git status --short` 语义一致:
/// `M` 已修改、`A` 已暂存新增、`U` 未合并(冲突)、`D` 已删除、`?` 未跟踪。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum StatusKind {
    /// 工作区或暂存区内容有修改(含重命名、类型变更)。
    Modified,
    /// 已 add 进暂存区的新文件。
    Added,
    /// 合并冲突中(unmerged)。
    Unmerged,
    /// 已删除(暂存区或工作区)。
    Deleted,
    /// 未跟踪的新文件(untracked)。
    Untracked,
}

impl fmt::Display for StatusKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let letter = match self {
            StatusKind::Modified => 'M',
            StatusKind::Added => 'A',
            StatusKind::Unmerged => 'U',
            StatusKind::Deleted => 'D',
            StatusKind::Untracked => '?',
        };
        write!(f, "{letter}")
    }
}

/// 一个文件的状态条目。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FileStatus {
    /// 相对仓库根的路径,POSIX 分隔符。
    pub path: String,
    /// 状态码。
    pub code: StatusKind,
}

/// [`status`] 的返回:改动条目 + 截断计数。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StatusSnapshot {
    /// 改动条目,按路径排序,至多调用方给的 `limit` 条。
    pub entries: Vec<FileStatus>,
    /// 超出上限被丢弃的条数;`0` = 快照完整。被截断的文件不出现在
    /// 条目里(UI 侧随之无角标、不可选中),由调用方提示行兜底。
    pub truncated: usize,
}

/// 一条提交记录。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CommitInfo {
    /// 完整 hash(40 位十六进制)。
    pub hash: String,
    /// 短 hash(与 git 默认 core.abbrev 一致的 7 位,不做仓库内唯一性消歧)。
    pub short_hash: String,
    /// 首行提交说明。
    pub subject: String,
    /// 作者,`名字 <邮箱>` 格式(缺失部分自动省略)。
    pub author: String,
    /// 提交时间,Unix 秒(UI 层自行格式化为本地时区)。
    pub time: i64,
}

/// 一行的 blame 归属。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BlameLine {
    /// 行号,从 1 开始。
    pub line_no: u32,
    /// 该行最后修改提交的短 hash。
    pub short_hash: String,
    /// 该提交的首行说明。
    pub subject: String,
}

/// 行级 diff 中一行的分类。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum DiffLineKind {
    /// 上下文行:两侧都有,`old_lineno`/`new_lineno` 均有效。
    Context,
    /// 删除行:只在旧侧,`old_lineno` 有效、`new_lineno` 为 `None`。
    Deleted,
    /// 新增行:只在新侧,`new_lineno` 有效、`old_lineno` 为 `None`。
    Added,
}

/// 行级 diff 的一行。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DiffLine {
    /// 旧文件行号(1 起);删除行与上下文行有,新增行为 `None`。
    pub old_lineno: Option<u32>,
    /// 新文件行号(1 起);新增行与上下文行有,删除行为 `None`。
    pub new_lineno: Option<u32>,
    /// 行分类。
    pub kind: DiffLineKind,
    /// 行文本(已去行尾 `\n`/`\r`,UTF-8 字符边界完整,不会截半字符)。
    pub text: String,
}

/// 行级 diff 的一个 hunk:hunk 头的行号范围 + 顺序行序列。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DiffHunk {
    /// 旧侧起始行号(hunk 头 `@@ -old_start,old_lines`;全新文件为 0)。
    pub old_start: u32,
    /// 旧侧行数。
    pub old_lines: u32,
    /// 新侧起始行号(`@@ +new_start,new_lines`;整文件删除为 0)。
    pub new_start: u32,
    /// 新侧行数。
    pub new_lines: u32,
    /// 行序列,按 diff 输出顺序(上下文/删除/新增交错)。
    pub lines: Vec<DiffLine>,
}

/// 单文件的行级结构化 diff([`diff_file_lines`] 的返回)。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FileDiff {
    /// hunk 序列;无改动或二进制文件时为空。
    pub hunks: Vec<DiffHunk>,
    /// pathspec 命中二进制文件(无文本 diff):`hunks` 为空,渲染层显示
    /// [`DIFF_BINARY_PLACEHOLDER`]。
    pub binary: bool,
    /// 输出超过 ~64KB 上限在行边界截断(整行丢弃,不截半字符):渲染层
    /// 追加 [`DIFF_TRUNCATION_PLACEHOLDER`]。
    pub truncated: bool,
}

/// [`log`] 的默认条数上限。
pub const DEFAULT_LOG_LIMIT: usize = 50;

/// [`status`] 的默认条数上限。与 app 侧文件树(`MAX_CHILDREN`)、搜索
/// (`MAX_HITS`)的截断先例同量级:列表逐行渲染、每帧全量布局,500 行
/// 是「大仓库不卡帧」与「日常改动全可见」的折中。
pub const DEFAULT_STATUS_LIMIT: usize = 500;

/// 从任意目录向上探测 Git 仓库的工作区根(`.git` 所在目录),起点自身
/// 是仓库根也命中。UI 的文件树根可能是仓库子目录(例如选了 `docs/`),
/// 其余 API 只认仓库根,先经本函数换算。裸仓库(只有 `.git` 内容、无
/// 工作区)返回 Err——本 crate 的全部操作都针对工作区文件。
pub fn discover(start: &Path) -> Result<PathBuf, String> {
    let root = Repository::discover(start)
        .map_err(|error| format!("当前目录不是 Git 仓库: {error}"))
        .and_then(|repo| {
            repo.workdir()
                .map(Path::to_path_buf)
                .ok_or_else(|| "当前是裸仓库,没有工作区文件".to_owned())
        })?;
    // libgit2 resolves symlinks (e.g. macOS /var -> /private/var), while the
    // file tree and open tabs retain the path selected by the user. Preserve
    // that spelling so status badges and checkout notifications match them.
    let absolute = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("无法读取当前目录: {error}"))?
            .join(start)
    };
    let physical_root = root.canonicalize().unwrap_or_else(|_| root.clone());
    for ancestor in absolute.ancestors() {
        if ancestor.canonicalize().ok().as_deref() == Some(physical_root.as_path()) {
            return Ok(ancestor.to_path_buf());
        }
    }
    Ok(root)
}

/// 读取仓库全部改动(含未跟踪文件),按路径排序,至多 `limit` 条,超出
/// 部分计入 `truncated`(与 log/diff 的上限同款口径:UI 逐行渲染 + 每帧
/// 全量布局,无界列表会在超大仓库卡帧)。截断发生在排序之后,保留的是
/// 字典序最小的前 `limit` 条。
///
/// 未跟踪目录会递归展开到逐个文件,便于文件树打标;`.gitignore` 命中的
/// 文件按 git 惯例不出现;非 UTF-8 文件名同样不出现(极罕见,libgit2
/// 拿不到 &str 形式的路径)。
pub fn status(root: &Path, limit: usize) -> Result<StatusSnapshot, String> {
    let repo = open_repo(root)?;
    let mut options = StatusOptions::new();
    options.include_untracked(true).recurse_untracked_dirs(true);
    let statuses = repo
        .statuses(Some(&mut options))
        .map_err(|error| format!("读取仓库状态失败: {error}"))?;
    let mut entries: Vec<FileStatus> = statuses
        .iter()
        .filter_map(|entry| {
            entry.path().ok().map(|path| FileStatus {
                path: path.to_owned(),
                code: status_kind(entry.status()),
            })
        })
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let truncated = entries.len().saturating_sub(limit);
    entries.truncate(limit);
    Ok(StatusSnapshot { entries, truncated })
}

/// 近期提交列表,新提交在前,最多 `limit` 条;还没有任何提交的空仓库
/// 返回空列表而不是错误。排序取「拓扑 + 时间」组合:拓扑保证 parent
/// 永远排在 child 之后,时间解决同层先后(纯时间排序在同一秒内的多笔
/// 提交间顺序不稳定)。
pub fn log(root: &Path, limit: usize) -> Result<Vec<CommitInfo>, String> {
    let repo = open_repo(root)?;
    // `is_empty` may report false for an unborn non-master branch. Inspect
    // HEAD directly; revwalk can otherwise turn UnbornBranch into NotFound.
    match repo.head() {
        Ok(_) => {}
        Err(error) if error.code() == git2::ErrorCode::UnbornBranch => return Ok(Vec::new()),
        Err(error) => return Err(format!("读取提交历史失败: {error}")),
    }
    let mut revwalk = repo
        .revwalk()
        .map_err(|error| format!("读取提交历史失败: {error}"))?;
    revwalk
        .push_head()
        .map_err(|error| format!("读取提交历史失败: {error}"))?;
    revwalk
        .set_sorting(git2::Sort::TIME | git2::Sort::TOPOLOGICAL)
        .map_err(|error| format!("读取提交历史失败: {error}"))?;
    let mut commits = Vec::new();
    for oid in revwalk.take(limit) {
        let oid = oid.map_err(|error| format!("遍历提交历史失败: {error}"))?;
        let commit = repo
            .find_commit(oid)
            .map_err(|error| format!("读取提交失败: {error}"))?;
        commits.push(commit_info(&commit)?);
    }
    Ok(commits)
}

/// 一条提交树 diff 中的文件变更类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitChangeKind {
    /// 文件在提交中新增。
    Added,
    /// 文件内容或模式被修改。
    Modified,
    /// 文件在提交中删除。
    Deleted,
    /// 文件被重命名，可同时包含内容修改。
    Renamed,
    /// 文件类型在提交中改变，例如普通文件变为目录。
    TypeChanged,
}

/// 一条提交树 diff 中的文件变更。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFileChange {
    /// 变更后的路径；删除变更使用删除前的路径。
    pub path: String,
    /// 重命名变更的旧路径；其他变更为 `None`。
    pub old_path: Option<String>,
    /// 变更类型。
    pub kind: CommitChangeKind,
}

/// 一次提交的文件变更列表及其提交上下文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFiles {
    /// 被查询的提交。
    pub commit: CommitInfo,
    /// 提交的第一父提交；初始提交没有父提交。
    pub parent: Option<CommitInfo>,
    /// 按路径排序的文件变更，最多包含调用方指定的 `limit` 条。
    pub entries: Vec<CommitFileChange>,
    /// 排序后因 `limit` 被省略的剩余文件数。
    pub truncated: usize,
}

/// 一次提交中单个文件的 unified 与结构化 diff。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitDiff {
    /// unified diff 文本，沿用约 64 KiB 的截断保护。
    pub text: String,
    /// 与 unified 文本同一份 patch 产生的行级结构化 diff。
    pub lines: FileDiff,
}

/// 列出提交相对第一父提交的文件变更。
///
/// 提交对象支持完整或短十六进制 hash。初始提交相对空树比较；merge
/// 提交只比较第一父提交。重命名检测在本函数和 [`commit_file_diff`]
/// 中使用同一套显式配置。列举文件只读取 diff delta，不为每个文件生成
/// patch 或做二进制内容检测。
///
/// `limit` 在排序之后生效；`truncated` 是未返回的剩余条数，可通过增大
/// `limit` 再次调用获取后续文件。
///
/// # Errors
///
/// 仓库、hash、提交对象或提交树不可读时返回面向用户的错误字符串。
pub fn commit_files(root: &Path, commit_hash: &str, limit: usize) -> Result<CommitFiles, String> {
    let repo = open_repo(root)?;
    let commit = resolve_commit(&repo, commit_hash)?;
    let parent = if commit.parent_count() == 0 {
        None
    } else {
        Some(
            commit
                .parent(0)
                .map_err(|error| format!("读取提交第一父提交失败: {error}"))?,
        )
    };
    let parent_info = parent
        .as_ref()
        .map(|parent| commit_info(parent))
        .transpose()?;
    let commit_info = commit_info(&commit)?;
    let diff = commit_tree_diff(&repo, &commit, true)?;

    let mut entries = Vec::with_capacity(diff.deltas().len());
    for delta in diff.deltas() {
        let old_path = delta.old_file().path_bytes().map(path_bytes_to_string);
        let new_path = delta.new_file().path_bytes().map(path_bytes_to_string);
        let path = new_path.or_else(|| old_path.clone());
        let Some(path) = path else {
            // A tree entry without a UTF-8-compatible path cannot be represented
            // by the public String DTO. Keep the rest of the diff usable.
            continue;
        };
        let kind = commit_change_kind(delta.status());
        entries.push(CommitFileChange {
            path,
            old_path: (delta.status() == Delta::Renamed)
                .then_some(old_path)
                .flatten(),
            kind,
        });
    }
    entries.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.old_path.cmp(&right.old_path))
    });
    let truncated = entries.len().saturating_sub(limit);
    entries.truncate(limit);

    Ok(CommitFiles {
        commit: commit_info,
        parent: parent_info,
        entries,
        truncated,
    })
}

/// 读取提交中一个文件的 unified 与结构化 diff。
///
/// `path` 按完整路径字符串精确匹配提交 diff 的新路径；删除文件匹配
/// 旧路径，重命名还接受旧路径作为别名。不会把 `*`、`?` 或路径前缀当作
/// glob/pathspec。返回的两个视图由同一个 libgit2 patch 生成，保留现有
/// [`FileDiff`] 的 UTF-8、CJK、行边界和约 64 KiB 截断口径。
///
/// # Errors
///
/// 仓库、hash、提交对象或提交树不可读，或提交中没有精确匹配的路径时，
/// 返回面向用户的错误字符串。
pub fn commit_file_diff(root: &Path, commit_hash: &str, path: &str) -> Result<CommitDiff, String> {
    let repo = open_repo(root)?;
    let commit = resolve_commit(&repo, commit_hash)?;
    let diff = commit_tree_diff(&repo, &commit, false)?;
    let index = diff
        .deltas()
        .enumerate()
        .find_map(|(index, delta)| delta_matches_path(&delta, path).then_some(index))
        .ok_or_else(|| format!("提交中不存在文件路径: {path}"))?;
    let delta = diff
        .get_delta(index)
        .ok_or_else(|| "读取提交文件变更失败: delta 不存在".to_owned())?;
    let mut lines = FileDiff {
        hunks: Vec::new(),
        binary: false,
        truncated: false,
    };
    let mut text = String::new();
    match Patch::from_diff(&diff, index).map_err(|error| format!("生成提交 diff 失败: {error}"))?
    {
        None => {
            // Some libgit2 versions represent a binary patch as None. The
            // selected diff was created with binary checking enabled, so this
            // fallback distinguishes it from a metadata-only rename/mode
            // change without inspecting patches for unrelated files.
            if delta.flags().contains(git2::DiffFlags::BINARY) {
                lines.binary = true;
                text.push_str(BINARY_NOTICE);
            }
        }
        Some(mut patch) => {
            let buf = patch
                .to_buf()
                .map_err(|error| format!("生成提交 diff 失败: {error}"))?;
            text = truncate_diff(&String::from_utf8_lossy(&buf));
            let _ = collect_diff_lines(&mut patch, &mut lines, 0)?;
        }
    }
    Ok(CommitDiff { text, lines })
}

/// 单文件历史：列出改动过 `path` 的提交，新在前。
///
/// 语义对齐 `git log -- <path>`（**不带 `--follow`**）：按提交 diff 的新旧
/// 路径精确匹配（与 [`commit_file_diff`] 同一判定），重命名之后的历史不跨
/// 名跟踪 —— 改名前的提交不在结果里。merge 提交只与第一父提交比较（与
/// [`commit_files`] 同口径）。
///
/// 路径在仓库里从未出现过（含未跟踪的新文件）返回**空列表**而不是错误；
/// 空仓库同样返回空列表。
///
/// `limit` 是结果条数上限。为避免「一个冷门文件」在超大仓库上把整条历史
/// 走完，遍历另受 [`FILE_LOG_SCAN_LIMIT`] 限制：扫够这么多提交仍没凑满
/// `limit` 条就按已有结果返回（结果可能少于 `limit`，也可能为空）。
///
/// # Errors
///
/// 仓库不可读、HEAD 不可解析或提交树 diff 失败时返回面向用户的错误字符串。
pub fn file_log(root: &Path, path: &str, limit: usize) -> Result<Vec<CommitInfo>, String> {
    let repo = open_repo(root)?;
    match repo.head() {
        Ok(_) => {}
        Err(error) if error.code() == git2::ErrorCode::UnbornBranch => return Ok(Vec::new()),
        Err(error) => return Err(format!("读取提交历史失败: {error}")),
    }
    let mut revwalk = repo
        .revwalk()
        .map_err(|error| format!("读取提交历史失败: {error}"))?;
    revwalk
        .push_head()
        .map_err(|error| format!("读取提交历史失败: {error}"))?;
    revwalk
        .set_sorting(git2::Sort::TIME | git2::Sort::TOPOLOGICAL)
        .map_err(|error| format!("读取提交历史失败: {error}"))?;
    let mut commits = Vec::new();
    for oid in revwalk.take(FILE_LOG_SCAN_LIMIT) {
        if commits.len() >= limit {
            break;
        }
        let oid = oid.map_err(|error| format!("遍历提交历史失败: {error}"))?;
        let commit = repo
            .find_commit(oid)
            .map_err(|error| format!("读取提交失败: {error}"))?;
        if commit_touches_path(&repo, &commit, path)? {
            commits.push(commit_info(&commit)?);
        }
    }
    Ok(commits)
}

/// [`file_log`] 遍历提交数的上限。
///
/// 命中提交往往稀疏（一个文件可能几百笔提交才动一次），不设上限时「查
/// 一个 2019 年改过的文件」会把整条历史走完。2000 是「够覆盖日常仓库的
/// 全生命周期」与「单次查询不卡 UI」的折中——与 status/log 的截断同精神，
/// 只是这里的代价是结果不完整而非列表不完整。
pub const FILE_LOG_SCAN_LIMIT: usize = 2_000;

/// 提交相对第一父提交是否改动了 `path`。
///
/// 先用 pathspec 让 libgit2 剪掉无关子树（只比 pathspec 覆盖的路径），再
/// 按 [`delta_matches_path`] 精确判定——pathspec 是 glob 语义，路径里带
/// `*`/`?` 时只靠它会误命中。初始提交没有父提交，与空树比较。
fn commit_touches_path<'repo>(
    repo: &'repo Repository,
    commit: &git2::Commit<'repo>,
    path: &str,
) -> Result<bool, String> {
    let parent_tree = if commit.parent_count() == 0 {
        None
    } else {
        let parent = commit
            .parent(0)
            .map_err(|error| format!("读取提交第一父提交失败: {error}"))?;
        Some(
            parent
                .tree()
                .map_err(|error| format!("读取提交父 tree 失败: {error}"))?,
        )
    };
    let tree = commit
        .tree()
        .map_err(|error| format!("读取提交 tree 失败: {error}"))?;
    let mut options = DiffOptions::new();
    options
        .pathspec(path)
        .skip_binary_check(true)
        .include_typechange(true);
    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut options))
        .map_err(|error| format!("计算提交 tree diff 失败: {error}"))?;
    Ok(diff.deltas().any(|delta| delta_matches_path(&delta, path)))
}

/// 将提交 hash 限定为完整或短十六进制对象名，再解析为提交对象。
fn resolve_commit<'repo>(
    repo: &'repo Repository,
    commit_hash: &str,
) -> Result<git2::Commit<'repo>, String> {
    let commit_hash = commit_hash.trim();
    if commit_hash.is_empty()
        || commit_hash.len() > 40
        || !commit_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(format!("无效的提交 hash: {commit_hash:?}"));
    }
    let commit = if commit_hash.len() == 40 {
        let oid = git2::Oid::from_str(commit_hash)
            .map_err(|error| format!("无效的提交 hash {commit_hash:?}: {error}"))?;
        repo.find_commit(oid)
    } else {
        repo.find_commit_by_prefix(commit_hash)
    };
    commit.map_err(|error| format!("找不到提交 {commit_hash:?}: {error}"))
}

/// 生成提交信息 DTO；与历史列表保持同一元数据口径。
fn commit_info(commit: &git2::Commit<'_>) -> Result<CommitInfo, String> {
    Ok(CommitInfo {
        hash: commit.id().to_string(),
        short_hash: short_hash(commit)?,
        subject: commit.summary().ok().flatten().unwrap_or("").to_owned(),
        author: signature_display(&commit.author()),
        time: commit.time().seconds(),
    })
}

/// 生成提交第一父树到提交树的 diff，并统一启用重命名检测。
///
/// `skip_binary_check` 让列举 delta 的路径只经过 tree diff，不为所有文件
/// 计算二进制状态；单文件 API 在选中 delta 后再由 `Patch` 读取该文件。
fn commit_tree_diff<'repo>(
    repo: &'repo Repository,
    commit: &git2::Commit<'repo>,
    skip_binary_check: bool,
) -> Result<git2::Diff<'repo>, String> {
    let parent_tree = if commit.parent_count() == 0 {
        None
    } else {
        let parent = commit
            .parent(0)
            .map_err(|error| format!("读取提交第一父提交失败: {error}"))?;
        Some(
            parent
                .tree()
                .map_err(|error| format!("读取提交父 tree 失败: {error}"))?,
        )
    };
    let commit_tree = commit
        .tree()
        .map_err(|error| format!("读取提交 tree 失败: {error}"))?;
    let mut options = DiffOptions::new();
    options
        .include_typechange(true)
        .include_typechange_trees(true)
        .ignore_filemode(false);
    if skip_binary_check {
        options.skip_binary_check(true);
    }
    let mut diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&commit_tree), Some(&mut options))
        .map_err(|error| format!("计算提交 tree diff 失败: {error}"))?;

    // Keep this configuration explicit. Both public history APIs call this
    // helper, so a rename with edits has exactly one classification threshold
    // and copies never turn into surprising extra entries.
    let mut find_options = DiffFindOptions::new();
    find_options
        .renames(true)
        .copies(false)
        .renames_from_rewrites(false)
        .break_rewrites(false)
        .rename_threshold(50);
    diff.find_similar(Some(&mut find_options))
        .map_err(|error| format!("检测提交重命名失败: {error}"))?;
    Ok(diff)
}

/// 将 libgit2 路径字节转换为 DTO 可传输的字符串。
fn path_bytes_to_string(path: &[u8]) -> String {
    String::from_utf8_lossy(path).into_owned()
}

/// 将一个 delta 映射为公开的提交变更分类。
fn commit_change_kind(delta: Delta) -> CommitChangeKind {
    match delta {
        Delta::Added => CommitChangeKind::Added,
        Delta::Deleted => CommitChangeKind::Deleted,
        Delta::Renamed => CommitChangeKind::Renamed,
        Delta::Typechange => CommitChangeKind::TypeChanged,
        Delta::Modified
        | Delta::Copied
        | Delta::Unmodified
        | Delta::Ignored
        | Delta::Unreadable
        | Delta::Untracked
        | Delta::Conflicted => CommitChangeKind::Modified,
    }
}

/// 以新路径为主精确匹配 delta；删除只匹配旧路径，重命名额外接受旧路径。
fn delta_matches_path(delta: &git2::DiffDelta<'_>, path: &str) -> bool {
    let old_path = delta.old_file().path_bytes().map(path_bytes_to_string);
    let new_path = delta.new_file().path_bytes().map(path_bytes_to_string);
    match delta.status() {
        Delta::Deleted => old_path.as_deref() == Some(path),
        Delta::Renamed => new_path.as_deref() == Some(path) || old_path.as_deref() == Some(path),
        _ => new_path.as_deref() == Some(path),
    }
}

/// 单文件 HEAD 与工作区(含暂存区)的 unified diff。
///
/// 无改动、文件不在 HEAD 与工作区时返回空串,由调用方显示「无改动」;
/// 空仓库(没有 HEAD 提交)返回 Err。未跟踪文件不参与 diff(与
/// `git diff` 一致)。pathspec 命中二进制文件时输出占位提示;输出超过
/// ~64KB 上限时截断并追加提示行。
pub fn diff_file(root: &Path, path: &str) -> Result<String, String> {
    let repo = open_repo(root)?;
    let tree = head_tree(&repo)?;
    let diff = head_to_workdir_diff(&repo, &tree, path)?;
    let mut text = String::new();
    for index in 0..diff.deltas().len() {
        match Patch::from_diff(&diff, index).map_err(|error| format!("生成 diff 失败: {error}"))?
        {
            // None = 该 delta 无文本 diff(二进制文件)
            None => text.push_str(BINARY_NOTICE),
            Some(mut patch) => {
                let buf = patch
                    .to_buf()
                    .map_err(|error| format!("生成 diff 失败: {error}"))?;
                text.push_str(&String::from_utf8_lossy(&buf));
            }
        }
    }
    Ok(truncate_diff(&text))
}

/// 单文件 HEAD 与工作区(含暂存区)的行级结构化 diff(双栏对比视图的
/// 数据源)。
///
/// 仓库打开、pathspec 路径解析、HEAD 对 workdir+index 的 diff 口径与
/// [`diff_file`] 完全一致:无改动返回空 `hunks`(由调用方显示「无改动」);
/// 空仓库返回 Err;未跟踪文件不参与;pathspec 命中二进制文件时 `binary`
/// 置位、`hunks` 为空。行号是 1 起的真实文件行号(新增行只有新侧、删除
/// 行只有旧侧、上下文行两侧都有),hunk 头的行号范围与 unified 输出一致。
/// 输出超过 ~64KB 上限时在行边界截断(整行丢弃,绝不截半字符)并置
/// `truncated`。
pub fn diff_file_lines(root: &Path, path: &str) -> Result<FileDiff, String> {
    let repo = open_repo(root)?;
    let tree = head_tree(&repo)?;
    let diff = head_to_workdir_diff(&repo, &tree, path)?;
    let mut result = FileDiff {
        hunks: Vec::new(),
        binary: false,
        truncated: false,
    };
    // 与 diff_file 同款字节预算,pathspec 命中多个 delta 时跨文件续算
    let mut used = 0usize;
    for index in 0..diff.deltas().len() {
        if result.truncated {
            break;
        }
        match Patch::from_diff(&diff, index).map_err(|error| format!("生成 diff 失败: {error}"))?
        {
            // None = 该 delta 无文本 diff(二进制文件)
            None => result.binary = true,
            Some(mut patch) => used = collect_diff_lines(&mut patch, &mut result, used)?,
        }
    }
    Ok(result)
}

/// 单文件的行级归属,基于 HEAD 提交内容;未提交的工作区行不参与
/// (libgit2 的 blame 不看工作区)。文件在 HEAD 中不存在(未跟踪或
/// 未提交过)返回 Err。
pub fn blame_file(root: &Path, path: &str) -> Result<Vec<BlameLine>, String> {
    let repo = open_repo(root)?;
    let blame = repo
        .blame_file(Path::new(path), None)
        .map_err(|error| format!("blame 失败: {error}"))?;
    let mut lines = Vec::new();
    let mut line_no: usize = 1;
    while let Some(hunk) = blame.get_line(line_no) {
        let commit_id = hunk.final_commit_id();
        let (short_hash, subject) = if commit_id.is_zero() {
            // libgit2 理论上不产出未提交行,防御性兜底
            ("0".repeat(7), "未提交的改动".to_owned())
        } else {
            let commit = repo
                .find_commit(commit_id)
                .map_err(|error| format!("读取 blame 提交失败: {error}"))?;
            (
                short_hash(&commit)?,
                commit.summary().ok().flatten().unwrap_or("").to_owned(),
            )
        };
        lines.push(BlameLine {
            line_no: line_no as u32,
            short_hash,
            subject,
        });
        line_no += 1;
    }
    Ok(lines)
}

/// 丢弃该文件的工作区改动,恢复为 HEAD 版本(等价 `git checkout -- <path>`,
/// 已删除的文件同样会被恢复)。
///
/// **本 crate 唯一的写操作,且不可逆(改动没有回收站):必须由 UI 侧弹
/// 确认模态、用户显式确认之后才允许调用。**
///
/// `path` 按 git pathspec 语义匹配;对未跟踪文件调用是无害的空操作。
pub fn checkout_file(root: &Path, path: &str) -> Result<(), String> {
    let repo = open_repo(root)?;
    let mut builder = CheckoutBuilder::new();
    builder.path(path).force();
    repo.checkout_head(Some(&mut builder))
        .map_err(|error| format!("恢复文件失败: {error}"))
}

/// HEAD 提交的 tree(diff 的对比基准)。空仓库(没有 HEAD 提交)返回
/// Err——diff_file 与 diff_file_lines 共用的前置检查。
fn head_tree(repo: &Repository) -> Result<git2::Tree<'_>, String> {
    if repo
        .is_empty()
        .map_err(|error| format!("检查仓库状态失败: {error}"))?
    {
        return Err("仓库还没有任何提交,没有 HEAD 可对比".to_owned());
    }
    repo.head()
        .and_then(|head| head.peel_to_tree())
        .map_err(|error| {
            if error.code() == git2::ErrorCode::UnbornBranch {
                "仓库还没有任何提交,没有 HEAD 可对比".to_owned()
            } else {
                format!("读取 HEAD 失败: {error}")
            }
        })
}

/// 按 pathspec 算 HEAD tree 对 workdir(含 index)的 diff;diff_file 与
/// diff_file_lines 共用,保证两套输出基于同一份 diff。
fn head_to_workdir_diff<'a>(
    repo: &'a Repository,
    tree: &'a git2::Tree<'_>,
    path: &str,
) -> Result<git2::Diff<'a>, String> {
    let mut options = DiffOptions::new();
    options.pathspec(path);
    repo.diff_tree_to_workdir_with_index(Some(tree), Some(&mut options))
        .map_err(|error| format!("计算 diff 失败: {error}"))
}

/// 走 patch 的行回调把内容行收进 `result`,按 hunk 分组;`used` 是之前
/// delta 已累计的近似输出字节数(返回值供后续 delta 续算)。累计超过
/// [`MAX_DIFF_BYTES`] 即在行边界截断:引发超限的行整行丢弃、`truncated`
/// 置位,之后回调空转不再累计——libgit2 的 print 回调返回 false 会变成
/// GIT_EUSER 错误,空转(数据本就在内存 patch 里)比吞错误干净。
fn collect_diff_lines(
    patch: &mut Patch<'_>,
    result: &mut FileDiff,
    mut used: usize,
) -> Result<usize, String> {
    // 当前 hunk 的头四元组;libgit2 对 hunk 内每一行都回传同一组值,
    // 变化即开新 hunk。
    let mut current: Option<(u32, u32, u32, u32)> = None;
    let mut callback = |_delta: git2::DiffDelta<'_>,
                        hunk: Option<git2::DiffHunk<'_>>,
                        line: git2::DiffLine<'_>|
     -> bool {
        if line.origin() == 'B' {
            // libgit2 的二进制提示行("Binary files a/x and b/x differ"):
            // 无文本 diff,置标记、不产生行条目。实测 libgit2 1.9.7 对
            // 二进制文件的 Patch::from_diff 返回 Some 而非 None,二进制
            // 靠这里检测(外层 None 分支只是兜底)。
            result.binary = true;
            return true;
        }
        if !matches!(line.origin(), ' ' | '+' | '-') {
            // 文件头(F)/hunk 头(H)与 "\ No newline at end of file"
            // 之类的 EOF 提示行(=<>):不是文件内容行,无行号意义,
            // 不产生条目。
            return true;
        }
        let mut text = String::from_utf8_lossy(line.content()).into_owned();
        if text.ends_with('\n') {
            text.pop();
        }
        if text.ends_with('\r') {
            text.pop();
        }
        // 每行 = 前缀 1 + 文本 + 换行 1,与 unified 文本的字节数同量级;
        // 不含文件头/hunk 头,是防无界列表的保护值,不追求与 diff_file
        // 的截断点逐字节一致。
        used += text.len() + 2;
        if used > MAX_DIFF_BYTES {
            result.truncated = true;
            return true;
        }
        let header = match hunk {
            Some(h) => (h.old_start(), h.old_lines(), h.new_start(), h.new_lines()),
            // 内容行必在 hunk 内,这只是防御
            None => (0, 0, 0, 0),
        };
        if current != Some(header) {
            current = Some(header);
            result.hunks.push(DiffHunk {
                old_start: header.0,
                old_lines: header.1,
                new_start: header.2,
                new_lines: header.3,
                lines: Vec::new(),
            });
        }
        let kind = match line.origin() {
            '+' => DiffLineKind::Added,
            '-' => DiffLineKind::Deleted,
            _ => DiffLineKind::Context,
        };
        result
            .hunks
            .last_mut()
            .expect("hunk 已在上一分支压入")
            .lines
            .push(DiffLine {
                old_lineno: line.old_lineno(),
                new_lineno: line.new_lineno(),
                kind,
                text,
            });
        true
    };
    patch
        .print(&mut callback)
        .map_err(|error| format!("生成 diff 失败: {error}"))?;
    Ok(used)
}

/// 打开仓库根;非 git 目录返回面向用户的错误。
fn open_repo(root: &Path) -> Result<Repository, String> {
    Repository::open(root).map_err(|error| format!("不是可用的 Git 仓库: {error}"))
}

/// libgit2 状态标志收敛为单字母状态码;同一文件多种标志并存时,
/// 冲突 > 未跟踪 > 暂存新增 > 删除 > 修改。
fn status_kind(flags: git2::Status) -> StatusKind {
    if flags.contains(git2::Status::CONFLICTED) {
        StatusKind::Unmerged
    } else if flags.contains(git2::Status::WT_NEW) {
        StatusKind::Untracked
    } else if flags.contains(git2::Status::INDEX_NEW) {
        StatusKind::Added
    } else if flags.intersects(git2::Status::INDEX_DELETED | git2::Status::WT_DELETED) {
        StatusKind::Deleted
    } else {
        StatusKind::Modified
    }
}

/// 提交作者的单行展示;名字或邮箱缺失时省略对应部分。
fn signature_display(signature: &Signature<'_>) -> String {
    match (signature.name().ok(), signature.email().ok()) {
        (Some(name), Some(email)) => format!("{name} <{email}>"),
        (Some(name), None) => name.to_owned(),
        (None, Some(email)) => email.to_owned(),
        (None, None) => String::new(),
    }
}

/// 提交的短 hash(libgit2 默认 core.abbrev,一般 7 位)。
fn short_hash(commit: &git2::Commit<'_>) -> Result<String, String> {
    commit
        .as_object()
        .short_id()
        .map_err(|error| format!("生成短 hash 失败: {error}"))
        .map(|buf| String::from_utf8_lossy(&buf).into_owned())
}

/// 超过 [`MAX_DIFF_BYTES`] 时在字符边界截断并追加提示行。
fn truncate_diff(text: &str) -> String {
    if text.len() <= MAX_DIFF_BYTES {
        return text.to_owned();
    }
    let mut cut = MAX_DIFF_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut truncated = text[..cut].to_owned();
    truncated.push_str(TRUNCATION_NOTICE);
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 建一次性临时仓库(每次全新目录,测试间无共享状态)。fixture 用
    /// 系统 git CLI 装配(与 latermd-app 的 git_diff.rs 同模式),被测
    /// API 全部走 latermd-git 自身。
    fn temp_repo(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("latermd-gitcrate-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Pin the initial branch: libgit2's empty-repo detection differs for
        // an unborn main branch, so developer-global Git settings must not
        // decide whether the empty-history regression is exercised.
        run_git(&dir, &["init", "-q", "--initial-branch=main"]);
        dir
    }

    /// 在临时仓库里跑 git;失败即 panic(fixture 装配错误)。
    fn run_git(dir: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
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

    /// 全部暂存并提交一条。
    fn commit_all(dir: &Path, message: &str) {
        run_git(dir, &["add", "."]);
        run_git(dir, &["commit", "-q", "-m", message]);
    }

    /// A symlinked workspace must keep the same path spelling as the file tree.
    #[test]
    #[cfg(unix)]
    fn discover_preserves_symlinked_workspace_paths() {
        let dir = temp_repo("discover-alias");
        let alias = dir.with_extension("alias");
        let _ = std::fs::remove_file(&alias);
        std::os::unix::fs::symlink(&dir, &alias).unwrap();
        std::fs::create_dir(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/a.md"), "before\n").unwrap();
        commit_all(&dir, "init");
        std::fs::write(alias.join("docs/a.md"), "after\n").unwrap();

        let root = discover(&alias.join("docs")).unwrap();
        assert_eq!(root, alias);
        let changes = status(&root, DEFAULT_STATUS_LIMIT).unwrap();
        assert_eq!(changes.entries.len(), 1);
        assert_eq!(root.join(&changes.entries[0].path), alias.join("docs/a.md"));
        assert_eq!(changes.entries[0].code, StatusKind::Modified);

        std::fs::remove_file(alias).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 四种日常状态各自成码,结果按路径排序。
    #[test]
    fn status_reports_codes_sorted_by_path() {
        let dir = temp_repo("status");
        std::fs::write(dir.join("a.md"), "一\n").unwrap();
        std::fs::write(dir.join("b.md"), "二\n").unwrap();
        commit_all(&dir, "init");

        std::fs::write(dir.join("a.md"), "一\n改\n").unwrap(); // M
        std::fs::write(dir.join("new.md"), "新\n").unwrap();
        run_git(&dir, &["add", "new.md"]); // A
        std::fs::remove_file(dir.join("b.md")).unwrap(); // D
        std::fs::write(dir.join("untracked.md"), "?\n").unwrap(); // ?

        let pairs: Vec<(String, String)> = status(&dir, DEFAULT_STATUS_LIMIT)
            .unwrap()
            .entries
            .into_iter()
            .map(|entry| (entry.path, entry.code.to_string()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("a.md".to_owned(), "M".to_owned()),
                ("b.md".to_owned(), "D".to_owned()),
                ("new.md".to_owned(), "A".to_owned()),
                ("untracked.md".to_owned(), "?".to_owned()),
            ],
            "{pairs:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 上限:超出 limit 的条目按字典序截断并计数;恰好等于 limit 不计
    /// 截断;limit 0 是空列表 + 全量计数。
    #[test]
    fn status_truncates_entries_beyond_limit() {
        let dir = temp_repo("status-cap");
        for i in 0..5 {
            std::fs::write(dir.join(format!("f{i}.md")), "?\n").unwrap();
        }

        let snapshot = status(&dir, 3).unwrap();
        assert_eq!(snapshot.entries.len(), 3);
        assert_eq!(snapshot.truncated, 2);
        assert_eq!(snapshot.entries[0].path, "f0.md", "字典序最小的保留");

        let exact = status(&dir, 5).unwrap();
        assert_eq!(exact.entries.len(), 5);
        assert_eq!(exact.truncated, 0, "恰好等于上限不算截断");

        let none = status(&dir, 0).unwrap();
        assert!(none.entries.is_empty());
        assert_eq!(none.truncated, 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 历史新提交在前,limit 生效,元数据完整。
    #[test]
    fn log_lists_newest_first_with_metadata() {
        let dir = temp_repo("log");
        std::fs::write(dir.join("a.md"), "一\n").unwrap();
        commit_all(&dir, "第一笔");
        std::fs::write(dir.join("a.md"), "一\n二\n").unwrap();
        commit_all(&dir, "第二笔");
        std::fs::write(dir.join("a.md"), "一\n二\n三\n").unwrap();
        commit_all(&dir, "第三笔");

        let commits = log(&dir, 2).unwrap();
        assert_eq!(commits.len(), 2, "limit 生效");
        assert_eq!(commits[0].subject, "第三笔");
        assert_eq!(commits[1].subject, "第二笔");
        assert_eq!(commits[0].hash.len(), 40);
        assert!(commits[0].short_hash.len() >= 7);
        assert!(
            commits[0].hash.starts_with(&commits[0].short_hash),
            "short_hash 必须是 hash 前缀"
        );
        assert!(
            commits[0].author.contains("LaterMD"),
            "{}",
            commits[0].author
        );
        assert!(commits[0].time >= commits[1].time, "新提交不早于旧提交");

        assert!(log(&dir, 0).unwrap().is_empty());
        assert_eq!(log(&dir, DEFAULT_LOG_LIMIT).unwrap().len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 提交文件历史 API 覆盖初始提交、排序/limit、父提交元数据与短 hash。
    #[test]
    fn commit_files_initial_and_sorted_limit() {
        let dir = temp_repo("history-files");
        std::fs::write(dir.join("z.md"), "旧 z\n").unwrap();
        std::fs::write(dir.join("a.md"), "旧 a\n").unwrap();
        commit_all(&dir, "initial");
        let initial_hash = git_stdout(&dir, &["rev-parse", "HEAD"]);
        let initial = commit_files(&dir, &initial_hash, usize::MAX).unwrap();
        assert!(initial.parent.is_none(), "初始提交相对空树没有父提交");
        assert_eq!(initial.entries.len(), 2);
        assert_eq!(initial.entries[0].path, "a.md");
        assert_eq!(initial.entries[0].kind, CommitChangeKind::Added);
        assert_eq!(initial.entries[1].path, "z.md");
        assert!(initial.entries.iter().all(|entry| entry.old_path.is_none()));

        std::fs::write(dir.join("m.md"), "新增 m\n").unwrap();
        std::fs::write(dir.join("a.md"), "修改 a\n").unwrap();
        std::fs::remove_file(dir.join("z.md")).unwrap();
        commit_all(&dir, "sorted changes");
        let hash = git_stdout(&dir, &["rev-parse", "HEAD"]);
        let limited = commit_files(&dir, &hash, 2).unwrap();
        assert_eq!(limited.entries.len(), 2);
        assert_eq!(limited.truncated, 1);
        assert_eq!(limited.entries[0].path, "a.md");
        assert_eq!(limited.entries[0].kind, CommitChangeKind::Modified);
        assert_eq!(limited.entries[1].path, "m.md");
        assert_eq!(limited.entries[1].kind, CommitChangeKind::Added);
        assert_eq!(commit_files(&dir, &hash, 0).unwrap().truncated, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 重命名检测对含内容修改的实际 rename 生效,新旧路径均可读取同一 patch。
    #[test]
    fn commit_history_detects_rename_with_edits_and_old_alias() {
        let dir = temp_repo("history-rename");
        std::fs::write(dir.join("old name.md"), "第一行\n第二行\n第三行\n第四行\n").unwrap();
        commit_all(&dir, "before rename");
        run_git(&dir, &["mv", "old name.md", "new name.md"]);
        std::fs::write(
            dir.join("new name.md"),
            "第一行\n第二行已改\n第三行\n第四行\n",
        )
        .unwrap();
        commit_all(&dir, "rename with edit");
        let hash = git_stdout(&dir, &["rev-parse", "HEAD"]);
        let files = commit_files(&dir, &hash, 10).unwrap();
        assert_eq!(files.entries.len(), 1, "应合并成一个 rename delta");
        assert_eq!(
            files.entries[0],
            CommitFileChange {
                path: "new name.md".to_owned(),
                old_path: Some("old name.md".to_owned()),
                kind: CommitChangeKind::Renamed,
            }
        );

        let new_diff = commit_file_diff(&dir, &hash, "new name.md").unwrap();
        let old_diff = commit_file_diff(&dir, &hash, "old name.md").unwrap();
        assert!(new_diff.text.contains("第二行已改"), "{new_diff:?}");
        assert_eq!(new_diff, old_diff, "重命名旧路径别名应复用同一 delta");
        assert!(commit_file_diff(&dir, &hash, "new").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// metadata-only rename 与 mode-only 变更都保留 unified 元信息,不会被当作 binary。
    #[test]
    #[cfg(unix)]
    fn commit_file_diff_handles_metadata_only_changes_without_binary() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_repo("history-metadata");
        std::fs::write(dir.join("old.md"), "同一内容\n").unwrap();
        commit_all(&dir, "before metadata changes");
        run_git(&dir, &["mv", "old.md", "new.md"]);
        commit_all(&dir, "metadata rename");
        let rename_hash = git_stdout(&dir, &["rev-parse", "HEAD"]);
        let rename_files = commit_files(&dir, &rename_hash, 10).unwrap();
        assert_eq!(rename_files.entries.len(), 1);
        assert_eq!(rename_files.entries[0].kind, CommitChangeKind::Renamed);
        let rename_diff = commit_file_diff(&dir, &rename_hash, "new.md").unwrap();
        assert!(!rename_diff.lines.binary, "{rename_diff:?}");
        assert!(
            rename_diff.text.contains("rename from"),
            "{}",
            rename_diff.text
        );
        assert!(rename_diff.lines.hunks.is_empty(), "{rename_diff:?}");

        let mut permissions = std::fs::metadata(dir.join("new.md")).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(dir.join("new.md"), permissions).unwrap();
        commit_all(&dir, "mode change");
        let mode_hash = git_stdout(&dir, &["rev-parse", "HEAD"]);
        let mode_diff = commit_file_diff(&dir, &mode_hash, "new.md").unwrap();
        assert!(!mode_diff.lines.binary, "{mode_diff:?}");
        assert!(mode_diff.text.contains("old mode"), "{}", mode_diff.text);
        assert!(mode_diff.lines.hunks.is_empty(), "{mode_diff:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_history_reports_binary_only_for_selected_diff() {
        let dir = temp_repo("history-binary");
        std::fs::write(dir.join("data.bin"), [1u8, 0, 2, 0]).unwrap();
        commit_all(&dir, "binary before");
        std::fs::write(dir.join("data.bin"), [3u8, 0, 4, 0]).unwrap();
        commit_all(&dir, "binary after");
        let hash = git_stdout(&dir, &["rev-parse", "HEAD"]);
        let files = commit_files(&dir, &hash, 10).unwrap();
        assert_eq!(files.entries.len(), 1);
        assert_eq!(files.entries[0].kind, CommitChangeKind::Modified);
        let diff = commit_file_diff(&dir, &hash, "data.bin").unwrap();
        assert!(diff.lines.binary, "{diff:?}");
        assert!(diff.lines.hunks.is_empty(), "{diff:?}");
        assert!(diff.text.contains("Binary files"), "{}", diff.text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 提交文件 diff 沿用统一文本与行级结构的 64 KiB 截断及 UTF-8 逻辑。
    #[test]
    fn commit_file_diff_truncates_large_utf8_patch() {
        let dir = temp_repo("history-cap");
        std::fs::write(dir.join("big.md"), "旧\n").unwrap();
        commit_all(&dir, "small");
        let contents: Vec<String> = (0..30_000).map(|i| format!("长行内容{i:05} 🚀")).collect();
        std::fs::write(dir.join("big.md"), format!("{}\n", contents.join("\n"))).unwrap();
        commit_all(&dir, "large");
        let hash = git_stdout(&dir, &["rev-parse", "HEAD"]);
        let diff = commit_file_diff(&dir, &hash, "big.md").unwrap();
        assert!(diff.text.len() <= MAX_DIFF_BYTES + TRUNCATION_NOTICE.len());
        assert!(diff.text.contains("已截断"));
        assert!(diff.lines.truncated);
        assert!(!diff.lines.binary);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 路径匹配是字面精确匹配,支持中文/emoji/括号与通配符字面量文件名。
    #[test]
    fn commit_file_diff_matches_literal_unicode_metacharacter_path() {
        let dir = temp_repo("history-path");
        let name = "中文🚀[x]*(y)?.md";
        std::fs::write(dir.join(name), "旧内容\n").unwrap();
        commit_all(&dir, "path before");
        std::fs::write(dir.join(name), "新内容 👨‍👩‍👧\n").unwrap();
        commit_all(&dir, "path after");
        let hash = git_stdout(&dir, &["rev-parse", "HEAD"]);
        let diff = commit_file_diff(&dir, &hash, name).unwrap();
        assert!(diff.text.contains("新内容 👨‍👩‍👧"), "{}", diff.text);
        assert!(commit_file_diff(&dir, &hash, "中文🚀[x]*").is_err());
        assert!(commit_file_diff(&dir, &hash, "中文🚀[x]*(y)?.md/child").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// hash 解析支持完整/短十六进制值,坏 hash 与缺失路径均返回明确错误。
    #[test]
    fn commit_history_rejects_bad_hash_and_missing_path() {
        let dir = temp_repo("history-errors");
        std::fs::write(dir.join("a.md"), "a\n").unwrap();
        commit_all(&dir, "one");
        let full = git_stdout(&dir, &["rev-parse", "HEAD"]);
        let short = &full[..7];
        assert_eq!(commit_files(&dir, short, 10).unwrap().commit.hash, full);
        assert!(commit_files(&dir, "not-a-hash", 10)
            .unwrap_err()
            .contains("无效的提交 hash"));
        assert!(commit_files(&dir, &format!("{full}0"), 10)
            .unwrap_err()
            .contains("无效的提交 hash"));
        assert!(commit_file_diff(&dir, &full, "missing.md")
            .unwrap_err()
            .contains("不存在文件路径"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// merge 提交只比较第一父提交,不把第二父分支的独有文件误列入。
    #[test]
    fn commit_history_merge_uses_first_parent() {
        let dir = temp_repo("history-merge");
        std::fs::write(dir.join("base.md"), "base\n").unwrap();
        commit_all(&dir, "base");
        run_git(&dir, &["checkout", "-q", "-b", "side"]);
        std::fs::write(dir.join("side.md"), "side\n").unwrap();
        commit_all(&dir, "side");
        run_git(&dir, &["checkout", "-q", "main"]);
        std::fs::write(dir.join("main.md"), "main\n").unwrap();
        commit_all(&dir, "main");
        run_git(&dir, &["merge", "--no-ff", "-q", "side", "-m", "merge"]);
        let merge_hash = git_stdout(&dir, &["rev-parse", "HEAD"]);
        let files = commit_files(&dir, &merge_hash, 10).unwrap();
        assert_eq!(files.parent.as_ref().unwrap().subject, "main");
        assert_eq!(
            files
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["side.md"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 历史 API 只读对象数据库,不受未提交工作区影响且不改 HEAD/index。
    #[test]
    fn commit_history_ignores_worktree_and_preserves_head_index() {
        let dir = temp_repo("history-read-only");
        std::fs::write(dir.join("doc.md"), "提交内容\n").unwrap();
        commit_all(&dir, "committed");
        let hash = git_stdout(&dir, &["rev-parse", "HEAD"]);
        let head_before = hash.clone();
        let index_before = std::fs::read(dir.join(".git/index")).unwrap();
        std::fs::write(dir.join("doc.md"), "工作区未提交内容\n").unwrap();
        std::fs::write(dir.join("untracked.md"), "未跟踪\n").unwrap();

        let files = commit_files(&dir, &hash, 10).unwrap();
        assert_eq!(files.entries.len(), 1);
        assert_eq!(files.entries[0].path, "doc.md");
        let diff = commit_file_diff(&dir, &hash, "doc.md").unwrap();
        assert!(diff.text.contains("提交内容"), "{}", diff.text);
        assert!(!diff.text.contains("工作区未提交内容"), "{}", diff.text);
        assert_eq!(git_stdout(&dir, &["rev-parse", "HEAD"]), head_before);
        assert_eq!(std::fs::read(dir.join(".git/index")).unwrap(), index_before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_on_repo_without_commits_is_empty() {
        let dir = temp_repo("empty-log");
        assert!(log(&dir, DEFAULT_LOG_LIMIT).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 单文件历史只含改动过该文件的提交,新在前;limit 生效;路径从未在
    /// 仓库里出现过是空列表(不是错误)。
    #[test]
    fn file_log_lists_only_commits_touching_path() {
        let dir = temp_repo("file-log");
        std::fs::write(dir.join("a.md"), "一\n").unwrap();
        std::fs::write(dir.join("b.md"), "一\n").unwrap();
        commit_all(&dir, "init");
        std::fs::write(dir.join("b.md"), "一\n二\n").unwrap();
        commit_all(&dir, "改 b");
        std::fs::write(dir.join("a.md"), "一\n二\n").unwrap();
        commit_all(&dir, "改 a");

        let subjects = |path: &str, limit: usize| -> Vec<String> {
            file_log(&dir, path, limit)
                .unwrap()
                .into_iter()
                .map(|commit| commit.subject)
                .collect()
        };
        assert_eq!(subjects("a.md", DEFAULT_LOG_LIMIT), vec!["改 a", "init"]);
        assert_eq!(subjects("b.md", DEFAULT_LOG_LIMIT), vec!["改 b", "init"]);
        assert_eq!(subjects("a.md", 1), vec!["改 a"], "limit 生效");
        assert!(
            subjects("no-such.md", DEFAULT_LOG_LIMIT).is_empty(),
            "未出现过的路径是空列表"
        );
        assert_eq!(file_log(&dir, "a.md", 0).unwrap().len(), 0, "limit 0 是空");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 空仓库的单文件历史是空列表(与 log 同款降级,不是错误)。
    #[test]
    fn file_log_on_repo_without_commits_is_empty() {
        let dir = temp_repo("empty-file-log");
        assert!(file_log(&dir, "a.md", DEFAULT_LOG_LIMIT)
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 现状钉子:file_log 不带 --follow,重命名前的提交不属于新名。改名
    /// 那一笔仍在新名的历史里(该提交删除旧名、新增新名)。
    #[test]
    fn file_log_does_not_follow_renames() {
        let dir = temp_repo("file-log-rename");
        std::fs::write(dir.join("old.md"), "一\n").unwrap();
        commit_all(&dir, "init old");
        run_git(&dir, &["mv", "old.md", "new.md"]);
        commit_all(&dir, "改名");

        let new: Vec<String> = file_log(&dir, "new.md", DEFAULT_LOG_LIMIT)
            .unwrap()
            .into_iter()
            .map(|commit| commit.subject)
            .collect();
        assert_eq!(new, vec!["改名"], "{new:?}");
        let old: Vec<String> = file_log(&dir, "old.md", DEFAULT_LOG_LIMIT)
            .unwrap()
            .into_iter()
            .map(|commit| commit.subject)
            .collect();
        assert_eq!(old, vec!["改名", "init old"], "旧名历史含改名那一笔");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// diff 是 HEAD 对工作区;干净文件返回空串。
    #[test]
    fn diff_file_shows_head_vs_workdir_and_clean_is_empty() {
        let dir = temp_repo("diff");
        std::fs::write(dir.join("a.md"), "旧\n").unwrap();
        std::fs::write(dir.join("clean.md"), "不变\n").unwrap();
        commit_all(&dir, "init");

        assert_eq!(diff_file(&dir, "clean.md").unwrap(), "", "无改动是空串");

        std::fs::write(dir.join("a.md"), "旧\n新行\n").unwrap();
        let diff = diff_file(&dir, "a.md").unwrap();
        assert!(diff.contains("+++ b/a.md"), "{diff}");
        assert!(diff.contains("+新行"), "{diff}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 病态大 diff 在 ~64KB 截断。
    #[test]
    fn diff_file_truncates_near_64kb() {
        let dir = temp_repo("diff-cap");
        std::fs::write(dir.join("big.md"), "小\n").unwrap();
        commit_all(&dir, "init");
        std::fs::write(dir.join("big.md"), "很长的中文内容。".repeat(50_000)).unwrap();

        let diff = diff_file(&dir, "big.md").unwrap();
        assert!(
            diff.len() <= MAX_DIFF_BYTES + TRUNCATION_NOTICE.len(),
            "必须截断: {}",
            diff.len()
        );
        assert!(diff.contains("已截断"), "{diff}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 跑 git 拿 stdout(行级 diff 与 git CLI 的对照 oracle 用;fixture
    /// 装配仍走 run_git)。
    fn git_stdout(dir: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} 失败: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("git 输出应为 UTF-8")
            .trim_end_matches(['\r', '\n'])
            .to_owned()
    }

    /// 解析 `git diff --no-color HEAD -- <path>`(单文件输出)的 unified
    /// 文本为 hunk/行结构,作行级 API 的对照 oracle:文件头/索引行与
    /// "\ No newline" 提示行不产生条目,与实现对 EOFNL 行的口径一致。
    fn parse_unified(text: &str) -> Vec<DiffHunk> {
        let mut hunks = Vec::new();
        let (mut old_no, mut new_no) = (0u32, 0u32);
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("@@ ") {
                // "@@ -1,3 +1,3 @@ 上下文" → "-1,3 +1,3"
                let body = rest.split(" @@").next().unwrap_or_default();
                let mut specs = body.split_whitespace();
                let old = specs
                    .next()
                    .unwrap_or_default()
                    .strip_prefix('-')
                    .unwrap_or_default();
                let new = specs
                    .next()
                    .unwrap_or_default()
                    .strip_prefix('+')
                    .unwrap_or_default();
                let range = |spec: &str| match spec.split_once(',') {
                    Some((start, count)) => (start.parse().unwrap(), count.parse().unwrap()),
                    None => (spec.parse().unwrap(), 1),
                };
                let (old_start, old_lines) = range(old);
                let (new_start, new_lines) = range(new);
                hunks.push(DiffHunk {
                    old_start,
                    old_lines,
                    new_start,
                    new_lines,
                    lines: Vec::new(),
                });
                old_no = old_start;
                new_no = new_start;
            } else if let Some(hunk) = hunks.last_mut() {
                // 首个 @@ 之前的文件头("--- a/x" 等)进不了这个分支
                let kind = match line.as_bytes().first() {
                    Some(b' ') => DiffLineKind::Context,
                    Some(b'+') => DiffLineKind::Added,
                    Some(b'-') => DiffLineKind::Deleted,
                    _ => continue, // "\ No newline" 提示行等
                };
                let text = line[1..].to_owned();
                let (old_lineno, new_lineno) = match kind {
                    DiffLineKind::Context => (Some(old_no), Some(new_no)),
                    DiffLineKind::Added => (None, Some(new_no)),
                    DiffLineKind::Deleted => (Some(old_no), None),
                };
                match kind {
                    DiffLineKind::Context => {
                        old_no += 1;
                        new_no += 1;
                    }
                    DiffLineKind::Added => new_no += 1,
                    DiffLineKind::Deleted => old_no += 1,
                }
                hunk.lines.push(DiffLine {
                    old_lineno,
                    new_lineno,
                    kind,
                    text,
                });
            }
        }
        hunks
    }

    /// 两套输出的占位文案必须同源:unified 的 BINARY/TRUNCATION 与行级
    /// 结构的 PLACEHOLDER 是同一份字面量的两处排版(行级无换行),改一处
    /// 不改另一处这里会红。
    #[test]
    fn notices_share_text_between_unified_and_structured() {
        assert_eq!(BINARY_NOTICE, format!("{DIFF_BINARY_PLACEHOLDER}\n"));
        assert_eq!(
            TRUNCATION_NOTICE,
            format!("\n{DIFF_TRUNCATION_PLACEHOLDER}\n")
        );
    }

    /// 修改行场景:hunk 头、行号(删除行只有旧侧、新增行只有新侧、上下文
    /// 双侧)与行分类;无改动文件是空 hunks;空仓库是显式 Err(与
    /// diff_file 同款文案)。
    #[test]
    fn diff_file_lines_reports_hunks_lines_and_kinds() {
        let dir = temp_repo("diff-lines");
        let error = diff_file_lines(&dir, "a.md").unwrap_err();
        assert!(error.contains("没有任何提交"), "{error}");
        std::fs::write(dir.join("a.md"), "一\n二\n三\n").unwrap();
        std::fs::write(dir.join("clean.md"), "不变\n").unwrap();
        commit_all(&dir, "init");

        let clean = diff_file_lines(&dir, "clean.md").unwrap();
        assert_eq!(clean.hunks, Vec::new(), "无改动是空 hunks");
        assert!(!clean.binary && !clean.truncated);

        std::fs::write(dir.join("a.md"), "一\n二改\n三\n").unwrap();
        let got = diff_file_lines(&dir, "a.md").unwrap();
        assert_eq!(
            got.hunks,
            vec![DiffHunk {
                old_start: 1,
                old_lines: 3,
                new_start: 1,
                new_lines: 3,
                lines: vec![
                    DiffLine {
                        old_lineno: Some(1),
                        new_lineno: Some(1),
                        kind: DiffLineKind::Context,
                        text: "一".to_owned(),
                    },
                    DiffLine {
                        old_lineno: Some(2),
                        new_lineno: None,
                        kind: DiffLineKind::Deleted,
                        text: "二".to_owned(),
                    },
                    DiffLine {
                        old_lineno: None,
                        new_lineno: Some(2),
                        kind: DiffLineKind::Added,
                        text: "二改".to_owned(),
                    },
                    DiffLine {
                        old_lineno: Some(3),
                        new_lineno: Some(3),
                        kind: DiffLineKind::Context,
                        text: "三".to_owned(),
                    },
                ],
            }],
            "{got:?}"
        );
        assert!(!got.binary && !got.truncated);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 纯新增文件(暂存区新文件参与 diff):hunk 旧侧 0,0,全部 Added,
    /// 新侧行号从 1 连续。
    #[test]
    fn diff_file_lines_on_added_file_is_all_additions() {
        let dir = temp_repo("diff-lines-add");
        std::fs::write(dir.join("a.md"), "一\n").unwrap();
        commit_all(&dir, "init");
        std::fs::write(dir.join("new.md"), "新1\n新2\n").unwrap();
        run_git(&dir, &["add", "new.md"]);

        let got = diff_file_lines(&dir, "new.md").unwrap();
        assert_eq!(got.hunks.len(), 1);
        let hunk = &got.hunks[0];
        assert_eq!((hunk.old_start, hunk.old_lines), (0, 0), "旧侧不存在");
        assert_eq!((hunk.new_start, hunk.new_lines), (1, 2));
        let added: Vec<(Option<u32>, Option<u32>, &str)> = hunk
            .lines
            .iter()
            .map(|line| (line.old_lineno, line.new_lineno, line.text.as_str()))
            .collect();
        assert_eq!(
            added,
            vec![(None, Some(1), "新1"), (None, Some(2), "新2")],
            "{added:?}"
        );
        assert!(got.hunks[0]
            .lines
            .iter()
            .all(|line| line.kind == DiffLineKind::Added));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 纯删除行:删除行只有旧侧行号,其余为上下文,新侧行号重排。
    #[test]
    fn diff_file_lines_on_deleted_line() {
        let dir = temp_repo("diff-lines-del");
        std::fs::write(dir.join("a.md"), "一\n二\n三\n四\n五\n").unwrap();
        commit_all(&dir, "init");
        std::fs::write(dir.join("a.md"), "一\n二\n四\n五\n").unwrap();

        let got = diff_file_lines(&dir, "a.md").unwrap();
        assert_eq!(got.hunks.len(), 1);
        let hunk = &got.hunks[0];
        assert_eq!(
            (
                hunk.old_start,
                hunk.old_lines,
                hunk.new_start,
                hunk.new_lines
            ),
            (1, 5, 1, 4)
        );
        let summary: Vec<(Option<u32>, Option<u32>, DiffLineKind, &str)> = hunk
            .lines
            .iter()
            .map(|line| {
                (
                    line.old_lineno,
                    line.new_lineno,
                    line.kind,
                    line.text.as_str(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (Some(1), Some(1), DiffLineKind::Context, "一"),
                (Some(2), Some(2), DiffLineKind::Context, "二"),
                (Some(3), None, DiffLineKind::Deleted, "三"),
                (Some(4), Some(3), DiffLineKind::Context, "四"),
                (Some(5), Some(4), DiffLineKind::Context, "五"),
            ],
            "{summary:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CJK 与 emoji(含 ZWJ 序列)行文本逐字符正确:content 按 UTF-8
    /// 字符边界整行透传,不存在截半字符。
    #[test]
    fn diff_file_lines_preserves_cjk_and_emoji_chars() {
        let dir = temp_repo("diff-lines-cjk");
        std::fs::write(dir.join("cjk.md"), "中文 🚀 内容\n第二行\n").unwrap();
        commit_all(&dir, "init");
        let replacement = "第二行改成 👨‍👩‍👧 家庭";
        std::fs::write(dir.join("cjk.md"), format!("中文 🚀 内容\n{replacement}\n")).unwrap();

        let got = diff_file_lines(&dir, "cjk.md").unwrap();
        let added: Vec<&DiffLine> = got.hunks[0]
            .lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Added)
            .collect();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].text, replacement);
        assert_eq!(
            added[0].text.chars().collect::<Vec<_>>(),
            replacement.chars().collect::<Vec<_>>(),
            "逐字符相等"
        );
        assert!(
            added[0].text.len() > added[0].text.chars().count(),
            "确有多字节字符且未被截半: {} 字节 / {} 字符",
            added[0].text.len(),
            added[0].text.chars().count()
        );
        assert_eq!(
            added[0].text.chars().filter(|c| *c == '\u{200d}').count(),
            2,
            "ZWJ 序列完整"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 二进制文件:无文本 diff,binary 置位、hunks 为空。现状注意:
    /// libgit2 1.9.7 下 diff_file 对二进制输出原生 "Binary files … differ"
    /// 文本而不是 BINARY_NOTICE 占位(from_diff 不返回 None),行级 API
    /// 按 origin='B' 检测,比文本口径更结构化;diff_file 行为保持不动。
    #[test]
    fn diff_file_lines_marks_binary_files() {
        let dir = temp_repo("diff-lines-bin");
        std::fs::write(dir.join("bin.bin"), [1u8, 0, 2, 0]).unwrap();
        commit_all(&dir, "init");
        std::fs::write(dir.join("bin.bin"), [3u8, 0, 4, 0]).unwrap();

        let got = diff_file_lines(&dir, "bin.bin").unwrap();
        assert!(got.binary, "{got:?}");
        assert_eq!(got.hunks, Vec::new());
        assert!(!got.truncated);
        assert!(
            diff_file(&dir, "bin.bin").unwrap().contains("Binary files"),
            "钉住 diff_file 对二进制的现状输出(libgit2 原生文本)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 超过 ~64KB 上限在行边界截断:整行丢弃(保留行与文件行全等,UTF-8
    /// 不截半字符)、行号连续无跳号、truncated 置位。
    #[test]
    fn diff_file_lines_truncates_on_line_boundary() {
        let dir = temp_repo("diff-lines-cap");
        std::fs::write(dir.join("big.md"), "小\n").unwrap();
        commit_all(&dir, "init");
        let file_lines: Vec<String> = (0..30_000).map(|i| format!("长行内容{i:05}")).collect();
        std::fs::write(dir.join("big.md"), format!("{}\n", file_lines.join("\n"))).unwrap();

        let got = diff_file_lines(&dir, "big.md").unwrap();
        assert!(got.truncated, "30_000 行必然超预算");
        assert!(!got.binary);
        assert_eq!(got.hunks.len(), 1);
        let lines = &got.hunks[0].lines;
        assert!(
            lines.len() < 30_001 && lines.len() > 1_000,
            "在预算处停止: {}",
            lines.len()
        );

        assert!(
            lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Deleted && line.text == "小"),
            "旧内容行仍在"
        );
        let added: Vec<&DiffLine> = lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Added)
            .collect();
        assert!(!added.is_empty());
        // 新侧行号从 1 连续无跳号:行边界丢行,而非把某行截成两半
        let line_nos: Vec<u32> = added.iter().filter_map(|line| line.new_lineno).collect();
        assert_eq!(
            line_nos,
            (1..=line_nos.len() as u32).collect::<Vec<_>>(),
            "行号连续"
        );
        // 每个保留行与文件里对应行全等(UTF-8 字符边界完好)
        for line in &added {
            assert_eq!(
                line.text,
                file_lines[line.new_lineno.expect("上面已过滤") as usize - 1],
                "行 {} 必须是完整行",
                line.new_lineno.expect("上面已过滤")
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 抽样与 git CLI 对照:hunk 头、行号与分类逐项等于
    /// `git diff --no-color HEAD -- <path>` 的解析结果(修改/删除/新增
    /// 三个样例钉住)。
    #[test]
    fn diff_file_lines_matches_git_cli_output() {
        let dir = temp_repo("diff-lines-cli");
        std::fs::write(dir.join("mod.md"), "一\n二\n三\n").unwrap();
        std::fs::write(dir.join("del.md"), "一\n二\n三\n四\n五\n").unwrap();
        commit_all(&dir, "init");
        std::fs::write(dir.join("mod.md"), "一\n二改\n三\n").unwrap();
        std::fs::write(dir.join("del.md"), "一\n二\n四\n五\n").unwrap();
        std::fs::write(dir.join("add.md"), "新1\n新2\n").unwrap();
        run_git(&dir, &["add", "add.md"]);

        for path in ["mod.md", "del.md", "add.md"] {
            let got = diff_file_lines(&dir, path).unwrap();
            let cli = git_stdout(&dir, &["diff", "--no-color", "HEAD", "--", path]);
            assert_eq!(
                got.hunks,
                parse_unified(&cli),
                "path={path} 与 git CLI 输出不一致\ncli={cli}"
            );
            assert!(!got.binary && !got.truncated);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// blame 行级归属各自的提交;未提交过的文件是显式 Err。
    #[test]
    fn blame_attributes_lines_to_their_commits() {
        let dir = temp_repo("blame");
        std::fs::write(dir.join("doc.md"), "第一行\n").unwrap();
        commit_all(&dir, "初稿");
        std::fs::write(dir.join("doc.md"), "第一行\n第二行\n").unwrap();
        commit_all(&dir, "追加第二行");

        let commits = log(&dir, DEFAULT_LOG_LIMIT).unwrap();
        let newest = &commits[0]; // 追加第二行
        let oldest = &commits[1]; // 初稿

        let lines = blame_file(&dir, "doc.md").unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].line_no, 1);
        assert_eq!(lines[0].short_hash, oldest.short_hash);
        assert_eq!(lines[0].subject, "初稿");
        assert_eq!(lines[1].line_no, 2);
        assert_eq!(lines[1].short_hash, newest.short_hash);
        assert_eq!(lines[1].subject, "追加第二行");

        std::fs::write(dir.join("fresh.md"), "新\n").unwrap();
        assert!(blame_file(&dir, "fresh.md").is_err(), "未提交文件无 blame");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// checkout 恢复被修改与被删除的文件到 HEAD 版本。
    #[test]
    fn checkout_file_restores_head_version() {
        let dir = temp_repo("checkout");
        std::fs::write(dir.join("a.md"), "HEAD 版本\n").unwrap();
        commit_all(&dir, "init");

        std::fs::write(dir.join("a.md"), "工作区乱改\n").unwrap();
        checkout_file(&dir, "a.md").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.md")).unwrap(),
            "HEAD 版本\n"
        );

        std::fs::remove_file(dir.join("a.md")).unwrap();
        checkout_file(&dir, "a.md").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.md")).unwrap(),
            "HEAD 版本\n",
            "删除的文件同样可恢复"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 非 git 目录:全部 API 返回 Err,不 panic(优雅降级的前提)。
    #[test]
    fn non_git_directory_degrades_to_error_not_panic() {
        let dir =
            std::env::temp_dir().join(format!("latermd-gitcrate-{}-plain", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        assert!(status(&dir, DEFAULT_STATUS_LIMIT).is_err());
        assert!(log(&dir, DEFAULT_LOG_LIMIT).is_err());
        assert!(file_log(&dir, "a.md", DEFAULT_LOG_LIMIT).is_err());
        assert!(diff_file(&dir, "a.md").is_err());
        assert!(diff_file_lines(&dir, "a.md").is_err());
        assert!(blame_file(&dir, "a.md").is_err());
        assert!(checkout_file(&dir, "a.md").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// discover 从仓库子目录向上找到工作区根;裸仓库没有工作区,显式 Err。
    #[test]
    fn discover_walks_up_to_workdir_root() {
        let dir = temp_repo("discover");
        std::fs::create_dir_all(dir.join("docs/deep")).unwrap();
        std::fs::write(dir.join("docs/deep/a.md"), "一\n").unwrap();

        assert_eq!(discover(&dir).unwrap(), dir, "起点即仓库根");
        assert_eq!(
            discover(&dir.join("docs/deep")).unwrap(),
            dir,
            "子目录向上探测"
        );

        let bare =
            std::env::temp_dir().join(format!("latermd-gitcrate-{}-bare", std::process::id()));
        let _ = std::fs::remove_dir_all(&bare);
        std::fs::create_dir_all(&bare).unwrap();
        run_git(&bare, &["init", "-q", "--bare", "."]);
        assert!(discover(&bare).is_err(), "裸仓库无工作区");
        let _ = std::fs::remove_dir_all(&bare);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
