//! 文件树(P0 基础版,docs/roadmap.md):懒加载 + `.gitignore` + 大目录截断。
//!
//! 遍历用 `ignore::WalkBuilder` 且只列一层(`max_depth(1)`),天然遵循
//! `.gitignore`;目录展开时才发生 IO(懒加载),结果缓存在 `children`,
//! 稳态帧零文件系统调用。展开状态自管 `HashMap<PathBuf, bool>`,渲染用
//! `ui.indent` + `selectable_label` 而非 `CollapsingHeader`(ADR-005 §4.1:
//! 动态目录下后者的内部 ID 不可控)。
//!
//! 持久化与主题同思路:平台配置目录下手写 JSON(`file_tree.json`),serde
//! 往返、缺项回落默认;坏文件只告警不挡启动。

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};

use crate::file::MARKDOWN_EXTENSIONS;
use crate::theme;

/// 持久化文件名,与 `settings.json` 同目录。
const SETTINGS_FILE: &str = "file_tree.json";

/// 新建文件时未指定后缀的缺省扩展名(不带点)。树只列 [`MARKDOWN_EXTENSIONS`]
/// 里的扩展名,补别的后缀等于建一个树里看不见的文件。
const DEFAULT_MD_EXT: &str = "md";

/// 「刷新」反馈窗口的最短时长(spinner 至少转这么久才退回图标)。
///
/// 取 450ms:比一次眨眼略长、又能被感知为「刚刚发生」而不是「卡住了」。
/// **它不是超时上限** —— 超了也照常转,直到重扫真正做完(见
/// [`FileTreeState::begin_refresh`] 里为什么需要它)。
pub const REFRESH_FEEDBACK_MIN: std::time::Duration = std::time::Duration::from_millis(450);

/// 单个目录在树里的直接子项显示上限(大目录保护:遍历整层收集计数,但
/// UI 只渲染前这么多项,剩余进截断提示行)。
pub const MAX_CHILDREN: usize = 500;

/// 最近目录保留条数。
pub const MAX_RECENTS: usize = 8;

/// 文件树运行态:根目录 + 展开状态 + 子项缓存(均为会话内状态,不持久化;
/// 持久化的是 [`FileTreeSettings`] 的根目录与最近列表)。
#[derive(Debug, Default)]
pub struct FileTreeState {
    /// 当前根目录;`None` = 未选择(Files 页显示引导)。
    pub root: Option<PathBuf>,
    /// 最近打开过的根目录(最新在前),供侧边栏快速切换。
    pub recents: Vec<PathBuf>,
    /// 目录展开状态(懒加载标志:置 `true` 后由 [`FileTreeState::ensure_loaded`]
    /// 在下一帧归约时列子项)。
    pub expanded: HashMap<PathBuf, bool>,
    /// 已列举的子项缓存。键缺席 + 对应目录展开中 = 待列举。
    pub children: HashMap<PathBuf, DirChildren>,
    /// 新建文件浮窗的输入状态;`Some` 时 UI 显示浮窗。同一时间至多一个。
    pub new_file: Option<NewFileDraft>,
    /// 新建文件夹浮窗的输入状态(context-menu-plan §3 M2);字段语义与
    /// [`NewFileDraft`] 相同,只是确认走 `create_folder`(不补 `.md`)。
    /// 同一时间至多一个,与 `new_file` 互斥由入口菜单天然保证(同一行
    /// 的菜单一次只发一条请求)。
    pub new_folder: Option<NewFileDraft>,
    /// 重命名浮窗的输入状态(context-menu-plan §3 M2):目标按**稳定
    /// 路径**存(浮窗开着期间树可以随便动,「改哪个」不漂移)。
    pub rename: Option<RenameDraft>,
    /// 删除确认的目标路径;`Some` 时 UI 显示确认浮窗。真正的删除在
    /// 归约确认后经 `trash_delete` 走系统废纸篓。
    pub delete_confirm: Option<PathBuf>,
    /// 「刷新」按钮的反馈窗口起点(见 [`FileTreeState::begin_refresh`])。
    pub refresh_started: Option<std::time::Instant>,
}

/// 重命名浮窗的输入状态(`Message::FileTreeRenameRequested` 的归约
/// [`crate::filetree::rename_entry`]):`from` 是磁盘上的现路径,`draft`
/// 是新名(纯名字,不含目录;改名固定在本目录内进行)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameDraft {
    /// 改名前的完整路径(文件或目录)。
    pub from: PathBuf,
    /// 新名字草稿。
    pub draft: String,
}

/// 新建文件浮窗的输入状态(#see `Message::FileTreeNewFileRequested` 的
/// 归约 [`crate::filetree::create_markdown`])。
///
/// 目标目录按**稳定路径**存而非索引:浮窗开着期间用户可以继续在树里点
/// 别的行、切根目录,存索引会让「建到哪里」漂移到别处。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFileDraft {
    /// 建文件的目录(当前文档所在目录,或文件树根)。
    pub dir: PathBuf,
    /// 文件名草稿(可带可不带 `.md`,`create_markdown` 会补)。
    pub draft: String,
}

impl FileTreeState {
    /// 换根:root 生效、置顶去重进最近列表、丢弃全部展开与缓存(旧树的
    /// 展开状态对新根无意义)。
    pub fn set_root(&mut self, root: PathBuf) {
        self.recents.retain(|dir| *dir != root);
        self.recents.insert(0, root.clone());
        self.recents.truncate(MAX_RECENTS);
        self.root = Some(root);
        self.expanded.clear();
        self.children.clear();
    }

    /// 翻转目录展开状态(点击目录行)。
    pub fn toggle(&mut self, dir: &Path) {
        let open = self.expanded.entry(dir.to_path_buf()).or_insert(false);
        *open = !*open;
    }

    /// 展开文件在树内的全部祖先目录(文件打开后高亮行可见的前提);
    /// 文件不在当前根之下则不动。
    pub fn expand_ancestors_of(&mut self, file: &Path) {
        let Some(root) = self.root.as_deref() else {
            return;
        };
        let mut dir = file.parent();
        while let Some(current) = dir {
            if current == root || !current.starts_with(root) {
                break;
            }
            self.expanded.insert(current.to_path_buf(), true);
            dir = current.parent();
        }
    }

    /// 丢掉某目录的子项缓存,使其下次 [`Self::ensure_loaded`] 重新列举。
    ///
    /// 外部写盘(新建文件)后调用:缓存是懒加载的产物,不在这里丢,新文件
    /// 在本会话里永远不出现(树又不是每帧重扫)。只丢这一项 —— 兄弟目录
    /// 的缓存与本次写盘无关,重扫它们只是白花 IO。
    pub fn invalidate(&mut self, dir: &Path) {
        self.children.remove(dir);
    }

    /// 丢掉缓存并开一段反馈窗口(点「刷新」的归约落点)。
    ///
    /// 清空 `children` 就是精确的「重扫全部可见项」—— 它的键**只可能**
    /// 是根与 `expanded` 里为真的目录(唯一的写入口是
    /// [`Self::ensure_loaded`],`invalidate` 只做删除),不存在需要保留
    /// 的不可见项;下一次 [`Self::ensure_loaded`] 即按这几项重列。
    ///
    /// **为什么要一段人为的最短时长**:重扫是同步的,且发生在
    /// `App::logic` 的 `end_of_logic` 里 —— 也就是**绘制之前**。小目录
    /// 重扫只有几毫秒,若 spinner 的显示与否直接绑「有没有 IO 在飞」,
    /// 它会同帧生灭:用户点了、屏幕动都不动,等于没反馈。故把窗口下限
    /// 拉到 [`REFRESH_FEEDBACK_MIN`],让「点了确实在动」这件事可见。
    ///
    /// `now` 参数化(同 [`crate::state`] 的 `autosave_pass` 口径):单测
    /// 注入时刻,不真等。
    pub fn begin_refresh(&mut self, now: std::time::Instant) {
        self.children.clear();
        self.refresh_started = Some(now);
    }

    /// 是否处于刷新反馈窗口内(UI 据此在图标与 spinner 之间切换)。
    ///
    /// 纯函数且**接受时刻**:窗口是否过期由调用方的时钟说了算,本方法
    /// 不读系统时钟 —— 否则单测没法构造「刚好在窗口内」这一档。
    pub fn refreshing(&self, now: std::time::Instant) -> bool {
        self.refresh_started
            .is_some_and(|start| now.duration_since(start) < REFRESH_FEEDBACK_MIN)
    }

    /// 补齐「根目录 + 展开中目录」的子项缓存。每帧归约末尾调用:键缺席才
    /// 发生 IO,稳态零开销;这正是懒加载的落点(点击目录只翻转标志,列举
    /// 延迟到下一次归约)。
    pub fn ensure_loaded(&mut self) {
        let Self {
            root,
            expanded,
            children,
            ..
        } = self;
        if let Some(root) = root {
            ensure_children(children, root);
        }
        for (dir, open) in expanded {
            if *open {
                ensure_children(children, dir);
            }
        }
    }
}

/// 缓存缺席才列举。
fn ensure_children(children: &mut HashMap<PathBuf, DirChildren>, dir: &Path) {
    children
        .entry(dir.to_path_buf())
        .or_insert_with(|| list_children(dir, MAX_CHILDREN));
}

/// 一个目录的直接子项(已过滤排序截断)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirChildren {
    /// 可见子项:目录在前、名称不区分大小写字典序。
    pub entries: Vec<TreeEntry>,
    /// 超过 [`MAX_CHILDREN`] 被截断的子项数(截断提示行数据源)。
    pub truncated: usize,
}

/// 树里的一行:目录或 Markdown 文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub path: PathBuf,
    /// 显示名(目录名或文件名,含扩展名)。
    pub name: String,
    pub is_dir: bool,
}

/// 列举一个目录的直接子项(`max_depth(1)` 语义):
///
/// - 遵循 `.gitignore` 与隐藏文件过滤(`WalkBuilder` 默认;`require_git(false)`
///   让无 `.git` 的普通目录也吃到自己写的 `.gitignore`,符合知识库直觉);
/// - 只保留目录与 `.md` / `.markdown` 文件;`*.assets` 目录(文档图片附件,
///   docs/image-plan.md B 段)整个不出现;
/// - 目录在前、名称不区分大小写字典序;超出 `cap` 的截断并计数。
///
/// `cap` 参数化只为测试可注入小值;生产恒为 [`MAX_CHILDREN`]。
/// 在文档库根下按**名称**找文档:`[[wikilink]]` 的解析落点(P3 双向链接)。
///
/// 匹配口径:文件名(去扩展名)与目标**忽略大小写全等**,扩展名须是
/// `.md` / `.markdown`;目标自带 `.md` / `.markdown` 后缀且无精确命中时,
/// 剥一个后缀按 stem 再试一轮(与反向链接扫描同口径,#87——`[[note.md]]`
/// 计入反向,点击同样要能打开)。目标带 `/` 时按相对路径直取(先原样、再
/// 补扩展名),不吃剥后缀的模糊匹配。
///
/// 遍历复用 `latermd_search`(与侧边栏搜索、MCP `list_files` 同一份实现),
/// 因此同样尊重 `.gitignore`;超过条目上限的部分找不到 —— 与搜索结果的截断
/// 同语义,不谎称全库精确。
pub fn find_by_name(root: &Path, target: &str) -> Option<PathBuf> {
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    // ① 带路径分隔符:按相对路径直取(先原样、再补扩展名)
    if target.contains('/') || target.contains('\\') {
        for candidate in [
            root.join(target),
            root.join(format!("{target}.md")),
            root.join(format!("{target}.markdown")),
        ] {
            if candidate.is_file() && latermd_search::is_markdown(&candidate) {
                return Some(candidate);
            }
        }
    }
    // ② 按文件名全库找(忽略大小写):精确 stem 命中在前,目标带 md 后缀
    // 时剥后缀的容错命中只补位(#87)——两轮各取列表首个,既有优先级不变
    let outcome =
        latermd_search::list_files(root, None, None, latermd_search::MAX_LIST_ENTRIES).ok()?;
    let stem_hit = |wanted: &str| {
        outcome
            .entries
            .iter()
            .filter(|entry| !entry.is_dir && latermd_search::is_markdown(&entry.path))
            .find(|entry| {
                entry
                    .path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_lowercase())
                    .as_deref()
                    == Some(wanted)
            })
            .map(|entry| root.join(&entry.path))
    };
    let stripped = strip_md_suffix(target);
    stem_hit(&target.to_lowercase()).or_else(|| stem_hit(&stripped?.to_lowercase()))
}

/// 剥掉目标末尾的一个 `.md` / `.markdown` 后缀(与反向链接扫描的
/// `strip_md_extension` 同口径,#87);没有后缀或剥完为空返回 `None`。
fn strip_md_suffix(target: &str) -> Option<&str> {
    let stem = target
        .strip_suffix(".markdown")
        .or_else(|| target.strip_suffix(".md"))?;
    (!stem.is_empty()).then_some(stem)
}

pub fn list_children(dir: &Path, cap: usize) -> DirChildren {
    let mut dirs: Vec<TreeEntry> = Vec::new();
    let mut files: Vec<TreeEntry> = Vec::new();
    // 遍历错误(权限、目录消失)跳过该条目:树是导航手段,不因个别
    // 条目不可读而整树失败
    for entry in WalkBuilder::new(dir)
        .max_depth(Some(1))
        .require_git(false)
        .build()
        .filter_map(Result::ok)
    {
        // depth 0 是根自身;max_depth(1) 之下其余即直接子项
        if entry.depth() == 0 {
            continue;
        }
        let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
        // 图片资产目录(`foo.assets/`,docs/image-plan.md B 段)是文档的
        // 私有附件区:树是文档导航,不是文件管理器,把它整个藏掉。
        if is_dir
            && entry
                .file_name()
                .to_string_lossy()
                .ends_with(crate::assets::ASSETS_DIR_SUFFIX)
        {
            continue;
        }
        if !is_dir && !is_markdown(entry.path()) {
            continue;
        }
        let item = TreeEntry {
            path: entry.path().to_path_buf(),
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir,
        };
        (if is_dir { &mut dirs } else { &mut files }).push(item);
    }
    let by_name = |a: &TreeEntry, b: &TreeEntry| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    };
    dirs.sort_by(by_name);
    files.sort_by(by_name);
    dirs.append(&mut files);
    let truncated = dirs.len().saturating_sub(cap);
    dirs.truncate(cap);
    DirChildren {
        entries: dirs,
        truncated,
    }
}

/// 扩展名是否 Markdown(与打开对话框的过滤器同一张清单)。
fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            MARKDOWN_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

/// 在 `dir` 下新建一个空 Markdown 文件,返回落盘路径。
///
/// 名称口径(与 [`crate::assets`] 的图片入库同名逻辑同款,只是没有扩展名
/// 白名单那一层):
///
/// - 空名(trim 后)拒绝 —— 调用方该禁用确认按钮,这里兜住直发的消息;
/// - 含路径分隔符或 `.`/`..` 拒绝(不允许借文件名逃出目录);
/// - 缺后缀补 `.md`;**已有扩展名但不是 Markdown 也照样补**(树只列
///   Markdown,建 `x.txt` 等于建一个用户看不见的文件);
/// - 撞名按 `-1`/`-2` 递增让开,不静默覆盖已有文档(覆盖是破坏性的,
///   而这里只是「点了一下新建」)。
///
/// 写盘失败带路径,由调用方原样落提示行。
pub fn create_markdown(dir: &Path, name: &str) -> Result<PathBuf, CreateError> {
    let trimmed = name.trim();
    if !valid_file_name(trimmed) {
        return Err(CreateError {
            path: dir.join(name),
            source: "文件名不能为空,也不能包含路径分隔符".into(),
        });
    }
    let wanted = if is_markdown(Path::new(trimmed)) {
        trimmed.to_owned()
    } else {
        format!("{trimmed}.{DEFAULT_MD_EXT}")
    };
    let path = dir.join(unique_name(dir, &wanted));
    std::fs::write(&path, b"").map_err(|source| CreateError {
        path: path.clone(),
        source: source.into(),
    })?;
    Ok(path)
}

/// 纯文件名:非空、不是 `.`/`..`、不带路径分隔符(与 `assets::valid_name`
/// 同口径;此处另有一份是因为那是图片入库的私有判据,不该由它决定
/// 文档能不能建)。UI 层用它给「创建」按钮判禁用,与 [`create_markdown`]
/// 的拒绝条件是同一份判据 —— 两处各写一遍必然漂移。
/// 在 `dir` 下新建一个文件夹(context-menu-plan §3 M2 右键菜单
/// 「新建文件夹」):名字合法性同 [`valid_file_name`],已存在同名条目
/// 拒绝(不像文件那样 `-1` 让开 —— 目录让开成 `新建文件夹-1` 反而
/// 迷惑,菜单入口草稿由归约用 `unique_draft` 预填,撞名是用户手改的
/// 后果,如实报错)。
pub fn create_folder(dir: &Path, name: &str) -> Result<PathBuf, CreateDirError> {
    let name = name.trim();
    if !valid_file_name(name) {
        return Err(CreateDirError {
            path: dir.to_path_buf(),
            source: "名字不能为空,也不能包含路径分隔符".into(),
        });
    }
    let target = dir.join(name);
    std::fs::create_dir(&target).map_err(|source| CreateDirError {
        path: target.clone(),
        source: Box::new(source),
    })?;
    Ok(target)
}

/// 真实改名(context-menu-plan §3 M2,与标签「重命名」的显示别名语义
/// 相区分):新名合法性校验 + 同目录拼接 + 撞名拒绝,`fs::rename` 落盘。
/// 文件与目录通用(目录改名整棵子树一起搬,同盘 rename 是常数成本)。
/// 返回新路径;失败时磁盘不动,浮窗保留可改名重试。
pub fn rename_entry(old: &Path, new_name: &str) -> Result<PathBuf, RenameError> {
    let new_name = new_name.trim();
    if !valid_file_name(new_name) {
        return Err(RenameError::InvalidName);
    }
    let Some(parent) = old.parent() else {
        return Err(RenameError::InvalidName);
    };
    let target = parent.join(new_name);
    if target.exists() {
        return Err(RenameError::Exists(target));
    }
    std::fs::rename(old, &target).map_err(|source| RenameError::Io {
        path: old.to_path_buf(),
        source: Box::new(source),
    })?;
    Ok(target)
}

/// 移入系统废纸篓(trash crate,freedesktop/回收站/废纸篓三平台各自
/// 的标准位置)。**不是 unlink** —— 可恢复是删除确认之外的最后一道
/// 保险。生产路径;无头测试经注入的 remover 走归约逻辑,不真调它
/// (避免污染用户废纸篓 / CI 环境)。
pub fn trash_delete(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|error| format!("删除失败 {}: {error}", path.display()))
}

/// 在系统文件管理器中显示该路径:macOS `open -R` 与 Windows
/// `explorer /select,` 都能定位并选中文件;Linux 没有跨桌面环境的
/// 「选中文件」标准(各 DE 各自的 D-Bus 接口),退而打开**父目录**。
/// spawn 后即忘(不等待、不回收:一次定位一个短命子进程,僵尸进程
/// 由 init 兜底,不值得为它立 reap 机制)。
pub fn reveal_in_file_manager(path: &Path) -> Result<(), String> {
    reveal_command(path)
        .map(|_| ())
        .map_err(|error| format!("打开文件管理器失败: {error}"))
}

#[cfg(target_os = "macos")]
fn reveal_command(path: &Path) -> std::io::Result<std::process::Child> {
    std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn()
}

#[cfg(target_os = "windows")]
fn reveal_command(path: &Path) -> std::io::Result<std::process::Child> {
    std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn reveal_command(path: &Path) -> std::io::Result<std::process::Child> {
    // Linux 没有跨桌面环境的「选中文件」标准,退而打开父目录
    let dir = path.parent().unwrap_or(path);
    std::process::Command::new("xdg-open").arg(dir).spawn()
}

pub fn valid_file_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\\')
}

/// 撞名递增让开:`x.md` 已在则试 `x-1.md`,再撞试 `x-2.md`。与
/// `assets::unique_name` 同款算法(`-N` 插在扩展名前)。
fn unique_name(dir: &Path, name: &str) -> String {
    let mut candidate = name.to_owned();
    let mut seq: u32 = 0;
    while dir.join(&candidate).exists() {
        seq += 1;
        candidate = match name.rsplit_once('.') {
            Some((stem, ext)) => format!("{stem}-{seq}.{ext}"),
            None => format!("{name}-{seq}"),
        };
    }
    candidate
}

/// 新建文件夹失败:与 [`CreateError`] 同形,文案区分动作。
#[derive(Debug)]
pub struct CreateDirError {
    path: PathBuf,
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl fmt::Display for CreateDirError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "新建文件夹失败 {}: {}", self.path.display(), self.source)
    }
}

/// 改名失败:名字非法 / 目标已存在 / 磁盘 IO,调用方原样落提示行。
#[derive(Debug)]
pub enum RenameError {
    /// 新名非法(空、路径分隔符、`.`/`..`)。
    InvalidName,
    /// 同目录下已有同名条目(改名绝不覆盖)。
    Exists(PathBuf),
    /// 磁盘 IO 失败。
    Io {
        path: PathBuf,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl fmt::Display for RenameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => write!(f, "名字不能为空,也不能包含路径分隔符"),
            Self::Exists(path) => write!(f, "同名条目已存在:{}", path.display()),
            Self::Io { path, source } => write!(f, "改名失败 {}: {}", path.display(), source),
        }
    }
}

/// 新建 Markdown 失败:带路径与原因,调用方原样落提示行。
#[derive(Debug)]
pub struct CreateError {
    path: PathBuf,
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl fmt::Display for CreateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "新建失败 {}: {}", self.path.display(), self.source)
    }
}

/// 持久化的文件树设置(根目录 + 最近列表)。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileTreeSettings {
    /// 上次会话的根目录,启动即恢复。
    pub root: Option<PathBuf>,
    /// 最近根目录列表(最新在前)。
    pub recents: Vec<PathBuf>,
}

impl FileTreeSettings {
    /// 启动装载:无文件用默认;坏文件终端告警后回落默认(挡启动不值得)。
    pub fn load() -> Self {
        let Some(dir) = theme::config_dir() else {
            return Self::default();
        };
        match Self::load_from(&dir) {
            Ok(settings) => settings,
            Err(LoadError::Missing) => Self::default(),
            Err(LoadError::Corrupt(source)) => {
                eprintln!("LaterMD: 文件树设置解析失败,已回落默认: {source}");
                Self::default()
            }
        }
    }

    /// 落盘到 `<dir>/file_tree.json`;`dir` 为 `None` 时用平台默认目录。
    /// 目录不存在则创建。失败带路径,提示行可直接展示。
    pub fn save_to(&self, dir: Option<&Path>) -> Result<(), SaveError> {
        let Some(dir) = dir.map(Path::to_path_buf).or_else(theme::config_dir) else {
            return Err(SaveError {
                path: PathBuf::from(SETTINGS_FILE),
                source: "找不到平台配置目录(HOME/APPDATA 均未设置)".into(),
            });
        };
        let json = serde_json::to_string_pretty(self).map_err(|source| SaveError {
            path: dir.join(SETTINGS_FILE),
            source: Box::new(source),
        })?;
        std::fs::create_dir_all(&dir).map_err(|source| SaveError {
            path: dir.clone(),
            source: Box::new(source),
        })?;
        std::fs::write(dir.join(SETTINGS_FILE), json.as_bytes()).map_err(|source| SaveError {
            path: dir.join(SETTINGS_FILE),
            source: Box::new(source),
        })
    }

    fn load_from(dir: &Path) -> Result<Self, LoadError> {
        let path = dir.join(SETTINGS_FILE);
        let bytes = std::fs::read(&path).map_err(|source| match source.kind() {
            io::ErrorKind::NotFound => LoadError::Missing,
            _ => LoadError::Corrupt(format!("{}: {}", path.display(), source)),
        })?;
        serde_json::from_slice(&bytes)
            .map_err(|source| LoadError::Corrupt(format!("{}: {}", path.display(), source)))
    }
}

impl From<&FileTreeState> for FileTreeSettings {
    fn from(state: &FileTreeState) -> Self {
        Self {
            root: state.root.clone(),
            recents: state.recents.clone(),
        }
    }
}

impl From<FileTreeSettings> for FileTreeState {
    /// 启动恢复:只取根目录与最近列表,展开状态从零开始(根的一级子项由
    /// 首帧 `ensure_loaded` 列出)。
    fn from(settings: FileTreeSettings) -> Self {
        Self {
            root: settings.root,
            recents: settings.recents,
            ..Self::default()
        }
    }
}

/// 读设置的失败情形(与 `crate::theme` 同构)。
#[derive(Debug)]
enum LoadError {
    Missing,
    Corrupt(String),
}

/// 写设置失败:带路径,可直接进提示行。
#[derive(Debug)]
pub struct SaveError {
    path: PathBuf,
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "文件树设置保存失败 {}: {}",
            self.path.display(),
            self.source
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 右键菜单的文件操作(context-menu-plan §3 M2 验收 4)。
    #[test]
    fn create_folder_validates_and_persists() {
        let dir = temp_tree("mkdir");
        let made = create_folder(&dir, "笔记").expect("合法名可建");
        assert_eq!(made, dir.join("笔记"));
        assert!(made.is_dir());
        // 非法名与撞名拒绝,磁盘不动
        assert!(create_folder(&dir, "").is_err());
        assert!(create_folder(&dir, "a/b").is_err());
        assert!(create_folder(&dir, "笔记").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_entry_moves_and_rejects_collision() {
        let dir = temp_tree("rename");
        let file = dir.join("旧名.md");
        std::fs::write(&file, "内容").unwrap();
        let new_path = rename_entry(&file, "新名.md").expect("改名成功");
        assert_eq!(new_path, dir.join("新名.md"));
        assert!(new_path.exists());
        assert!(!file.exists());
        assert_eq!(std::fs::read_to_string(&new_path).unwrap(), "内容");
        // 撞名 / 非法名 / 原文件缺失,磁盘不动
        std::fs::write(dir.join("占位.md"), "").unwrap();
        assert!(matches!(
            rename_entry(&new_path, "占位.md"),
            Err(RenameError::Exists(_))
        ));
        assert!(matches!(
            rename_entry(&new_path, ""),
            Err(RenameError::InvalidName)
        ));
        assert!(matches!(
            rename_entry(&new_path, "x/y"),
            Err(RenameError::InvalidName)
        ));
        let gone = dir.join("不存在.md");
        assert!(matches!(
            rename_entry(&gone, "随便.md"),
            Err(RenameError::Io { .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_entry_moves_directory_wholesale() {
        let dir = temp_tree("rename-dir");
        let sub = dir.join("旧目录");
        std::fs::create_dir_all(sub.join("子")).unwrap();
        std::fs::write(sub.join("子/文.md"), "x").unwrap();
        let new_sub = rename_entry(&sub, "新目录").expect("目录改名成功");
        assert!(new_sub.join("子/文.md").exists(), "子树整体搬移");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 进程内唯一的临时目录;测试自删。
    fn temp_tree(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("latermd-filetree-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(root: &Path, name: &str) {
        std::fs::write(root.join(name), b"x").unwrap();
    }

    fn entry_names(children: &DirChildren) -> Vec<&str> {
        children.entries.iter().map(|e| e.name.as_str()).collect()
    }

    /// 过滤与排序:目录在前(不区分大小写字典序),文件只留 Markdown;
    /// `.gitignore` 条目与隐藏文件不出现(`require_git(false)` 让无 `.git`
    /// 的目录同样吃 `.gitignore`);`*.assets` 图片附件目录不出现。
    #[test]
    fn list_children_keeps_dirs_and_markdown_only() {
        let dir = temp_tree("list");
        std::fs::write(dir.join(".gitignore"), b"node_modules/\n").unwrap();
        for name in ["b.md", "z.md", "c.txt", ".hidden.md", "a.markdown", "A.MD"] {
            touch(&dir, name);
        }
        std::fs::create_dir(dir.join("node_modules")).unwrap();
        touch(&dir.join("node_modules"), "inner.md");
        std::fs::create_dir(dir.join("docs")).unwrap();
        touch(&dir.join("docs"), "nested.md");
        std::fs::create_dir(dir.join("笔记.assets")).unwrap();
        touch(&dir.join("笔记.assets"), "截图.png");
        std::fs::create_dir(dir.join("assets 汇总")).unwrap();

        let children = list_children(&dir, MAX_CHILDREN);
        // 大小写不敏感字典序:a.markdown < A.MD("a" < "d" 于第 4 字符);
        // `笔记.assets` 不出现,`assets 汇总`(不以 .assets 结尾)保留
        assert_eq!(
            entry_names(&children),
            ["assets 汇总", "docs", "a.markdown", "A.MD", "b.md", "z.md"]
        );
        assert_eq!(children.truncated, 0);
        assert!(
            !children.entries.iter().any(|e| e.name.ends_with(".assets")),
            "资产目录整个不出现"
        );
        // 深层文件没有出现:确实只有一层
        assert!(!children.entries.iter().any(|e| e.name == "nested.md"));
        // 目录与文件的分组标记供渲染区分点击行为(前两个是目录,文件在后)
        assert!(children.entries[0].is_dir && children.entries[1].is_dir);
        assert!(!children.entries[2].is_dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// draft 镜像不入文件树(#18):`x.md.latermd-draft` 的扩展名是
    /// latermd-draft,不在 Markdown 清单里,被 `is_markdown` 天然排除 ——
    /// 钉住该行为,防止将来清单口径变化把防丢镜像漏进导航树。
    #[test]
    fn list_children_excludes_draft_files() {
        let dir = temp_tree("draft");
        touch(&dir, "x.md");
        touch(&dir, "x.md.latermd-draft");

        let children = list_children(&dir, MAX_CHILDREN);
        assert_eq!(entry_names(&children), ["x.md"], "draft 不入树");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 截断:超出 cap 的子项计数进 `truncated`,排序在前 cap 项保留。
    #[test]
    fn list_children_truncates_beyond_cap() {
        let dir = temp_tree("truncate");
        std::fs::create_dir(dir.join("dir")).unwrap();
        for index in 0..5 {
            touch(&dir, &format!("f{index}.md"));
        }

        let children = list_children(&dir, 3);
        assert_eq!(children.entries.len(), 3, "目录在前占首位");
        assert_eq!(entry_names(&children), ["dir", "f0.md", "f1.md"]);
        assert_eq!(children.truncated, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 不存在的目录列举为空而不是 panic(外部删除的目录在树里自然消失)。
    #[test]
    fn list_children_of_missing_dir_is_empty() {
        let children = list_children(Path::new("/latermd/no/such/dir"), MAX_CHILDREN);
        assert!(children.entries.is_empty());
        assert_eq!(children.truncated, 0);
    }

    /// 换根:最近列表置顶去重并截断;展开与缓存全部丢弃。
    #[test]
    fn set_root_refreshes_recents_and_drops_caches() {
        let mut tree = FileTreeState::default();
        let a = PathBuf::from("/vault/a");
        let b = PathBuf::from("/vault/b");
        tree.set_root(a.clone());
        tree.set_root(b.clone());
        tree.set_root(a.clone()); // 重复选择 → 置顶而不是重复入列

        assert_eq!(tree.root.as_deref(), Some(a.as_path()));
        assert_eq!(tree.recents, vec![a.clone(), b]);

        tree.toggle(&a);
        tree.children.insert(
            a.join("sub"),
            DirChildren {
                entries: vec![TreeEntry {
                    path: a.join("sub/x.md"),
                    name: "x.md".into(),
                    is_dir: false,
                }],
                truncated: 0,
            },
        );
        let c = PathBuf::from("/vault/c");
        tree.set_root(c.clone());
        assert_eq!(tree.root.as_deref(), Some(c.as_path()));
        assert!(tree.expanded.is_empty(), "旧根的展开状态不带入新根");
        assert!(tree.children.is_empty(), "旧根的子项缓存不带入新根");
    }

    /// 祖先展开:只展开根之内的目录,根自身不需要标志;根外文件不动。
    #[test]
    fn expand_ancestors_only_within_root() {
        let mut tree = FileTreeState::default();
        tree.set_root(PathBuf::from("/vault"));
        tree.expand_ancestors_of(Path::new("/vault/docs/deep/note.md"));
        assert_eq!(
            tree.expanded,
            HashMap::from([
                (PathBuf::from("/vault/docs"), true),
                (PathBuf::from("/vault/docs/deep"), true),
            ])
        );

        tree.expand_ancestors_of(Path::new("/elsewhere/note.md"));
        assert_eq!(tree.expanded.len(), 2, "根外文件的祖先不进展开表");

        let mut bare = FileTreeState::default();
        bare.expand_ancestors_of(Path::new("/x/y.md"));
        assert!(bare.expanded.is_empty(), "无根时无操作");
    }

    /// 懒加载落点:`ensure_loaded` 只补「根 + 展开中」目录的缓存,缺席才 IO;
    /// 已缓存(含空目录)的条目不再动。
    #[test]
    fn ensure_loaded_lists_only_missing_expanded_dirs() {
        let root = temp_tree("lazy");
        std::fs::create_dir(root.join("docs")).unwrap();
        touch(&root.join("docs"), "a.md");
        touch(&root, "top.md");

        let mut tree = FileTreeState::default();
        tree.set_root(root.clone());
        assert!(tree.children.is_empty(), "set_root 不列举,列举延迟到归约");

        tree.ensure_loaded();
        let root_children = tree.children.get(&root).expect("根子项已列举");
        assert_eq!(entry_names(root_children), ["docs", "top.md"]);

        tree.toggle(&root.join("docs"));
        tree.ensure_loaded();
        let docs = tree
            .children
            .get(root.join("docs").as_path())
            .expect("展开后才列举");
        assert_eq!(entry_names(docs), ["a.md"]);

        // 未展开目录不列举;再跑一遍不新增条目(稳态零 IO 的缓存语义)
        assert!(!tree.children.contains_key(&root.join("nope")));
        let before = tree.children.len();
        tree.ensure_loaded();
        assert_eq!(tree.children.len(), before);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 持久化往返:根与最近列表逐项一致;`FileTreeState` 双向转换只带这两项。
    #[test]
    fn settings_round_trip_and_state_conversion() {
        let dir = temp_tree("settings");
        let settings = FileTreeSettings {
            root: Some(PathBuf::from("/vault/docs")),
            recents: vec![PathBuf::from("/vault/docs"), PathBuf::from("/notes")],
        };
        settings.save_to(Some(&dir)).unwrap();
        assert_eq!(FileTreeSettings::load_from(&dir).unwrap(), settings);
        let _ = std::fs::remove_dir_all(&dir);

        let tree = FileTreeState::from(settings.clone());
        assert_eq!(tree.root, settings.root);
        assert_eq!(tree.recents, settings.recents);
        assert!(tree.expanded.is_empty() && tree.children.is_empty());
        assert_eq!(FileTreeSettings::from(&tree), settings);
    }

    /// 手改配置只留一个键:serde(default) 缺项回落,不整体失败。
    #[test]
    fn partial_json_fills_defaults() {
        let dir = temp_tree("partial");
        std::fs::write(dir.join(SETTINGS_FILE), br#"{"root":"/vault"}"#).unwrap();
        assert_eq!(
            FileTreeSettings::load_from(&dir).unwrap(),
            FileTreeSettings {
                root: Some(PathBuf::from("/vault")),
                recents: Vec::new(),
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 新建:落盘为 Markdown、缺后缀补 `.md`、撞名让开(不覆盖既有文档)。
    ///
    /// 上界独立于实现:这里断言的是盘上真实存在的文件名,不引用任何
    /// 参与造名的常量 —— `unique_name` 改算法(比如换成 `_1`)时断言会红,
    /// 而不会跟着一起变。
    #[test]
    fn create_markdown_writes_file_and_yields_on_collision() {
        let dir = temp_tree("create");

        let first = create_markdown(&dir, "笔记").unwrap();
        assert_eq!(first, dir.join("笔记.md"), "缺后缀补 .md");
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "", "空文档");
        assert!(first.is_file(), "真的落盘了,不只是返回路径");

        // 撞名:既有 `笔记.md` 不许被覆盖,让开到 `-1`
        let second = create_markdown(&dir, "笔记.md").unwrap();
        assert_ne!(second, first, "撞名不得返回同一路径");
        assert_eq!(
            std::fs::read_to_string(&first).unwrap(),
            "",
            "既有文件内容零变化(不静默覆盖)"
        );

        // 非 Markdown 后缀也补 `.md`:树只列 Markdown,建 `x.txt` 用户看不见
        let third = create_markdown(&dir, "x.txt").unwrap();
        assert_eq!(third.file_name().unwrap(), "x.txt.md");

        // 非法名拒绝(不落任何文件)
        for bad in ["", "   ", "a/b", "a\\b", ".", ".."] {
            assert!(create_markdown(&dir, bad).is_err(), "{bad:?} 应被拒绝");
        }
        assert_eq!(
            list_children(&dir, MAX_CHILDREN).entries.len(),
            3,
            "非法名一个都没落盘"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 新建后树里要能立刻看到:缓存是懒加载的,不显式失效则新文件在本会话
    /// 里永远不出现(`ensure_loaded` 只补缺席的键)。
    #[test]
    fn invalidate_makes_new_file_visible_after_reload() {
        let dir = temp_tree("invalidate");
        std::fs::create_dir(dir.join("docs")).unwrap();

        let mut tree = FileTreeState::default();
        tree.set_root(dir.clone());
        tree.toggle(&dir.join("docs"));
        tree.ensure_loaded();
        assert_eq!(entry_names(&tree.children[&dir]), ["docs"]);
        assert_eq!(
            entry_names(&tree.children[&dir.join("docs")]),
            Vec::<&str>::new()
        );

        let created = create_markdown(&dir.join("docs"), "新文档").unwrap();
        tree.ensure_loaded();
        assert!(
            entry_names(&tree.children[&dir.join("docs")]).is_empty(),
            "未失效前缓存不重扫,新文件不可见(这是本测试要防的回归)"
        );

        tree.invalidate(&dir.join("docs"));
        tree.ensure_loaded();
        assert_eq!(
            entry_names(&tree.children[&dir.join("docs")]),
            ["新文档.md"],
            "失效后下一次归约即列出新文件"
        );
        // 只丢指定目录:兄弟目录的缓存不动
        assert!(tree.children.contains_key(&dir), "兄弟目录缓存保留");
        assert!(created.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod wikilink_tests {
    use super::*;

    fn vault(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("latermd-wiki-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("子目录")).unwrap();
        std::fs::write(dir.join("架构决策.md"), "# 架构\n").unwrap();
        std::fs::write(dir.join("子目录/Note One.markdown"), "# note\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored.md\n").unwrap();
        std::fs::write(dir.join("ignored.md"), "x").unwrap();
        dir
    }

    /// 按文件名找:忽略大小写、两种扩展名都认、嵌套目录也能命中。
    #[test]
    fn find_by_name_matches_stem_case_insensitively() {
        let root = vault("stem");
        assert_eq!(
            find_by_name(&root, "架构决策"),
            Some(root.join("架构决策.md"))
        );
        assert_eq!(
            find_by_name(&root, "NOTE ONE"),
            Some(root.join("子目录/Note One.markdown")),
            "忽略大小写 + 嵌套 + .markdown"
        );
        assert_eq!(find_by_name(&root, "不存在"), None);
        assert_eq!(find_by_name(&root, "  "), None, "空目标");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `.gitignore` 排除的文件不当跳转目标(与搜索同一套语义)。
    #[test]
    fn find_by_name_respects_gitignore() {
        let root = vault("ignored");
        assert_eq!(find_by_name(&root, "ignored"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 带路径分隔符的目标按相对路径直取。
    #[test]
    fn find_by_name_accepts_relative_paths() {
        let root = vault("path");
        assert_eq!(
            find_by_name(&root, "子目录/Note One"),
            Some(root.join("子目录/Note One.markdown"))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #87 后缀容错:目标自带 `.md` / `.markdown` 后缀也按 stem 打开 ——
    /// 与反向链接扫描口径对齐(`[[note.md]]` 计入反向,点击此前打不开)。
    #[test]
    fn find_by_name_strips_md_suffix_for_bare_targets() {
        let root = vault("suffix");
        assert_eq!(
            find_by_name(&root, "架构决策.md"),
            Some(root.join("架构决策.md")),
            "`[[note.md]]` 打开 note.md(此前 stem 对不上,返回 None)"
        );
        assert_eq!(
            find_by_name(&root, "Note One.markdown"),
            Some(root.join("子目录/Note One.markdown")),
            ".markdown 后缀同剥,忽略大小写与嵌套不变"
        );
        assert_eq!(
            find_by_name(&root, "架构决策"),
            Some(root.join("架构决策.md")),
            "`[[note]]` 不带后缀,既有口径不变"
        );
        assert_eq!(
            find_by_name(&root, "ignored.md"),
            None,
            "剥后缀后的 stem 命中同样过不了 .gitignore"
        );
        assert_eq!(find_by_name(&root, ".md"), None, "剥完为空不参与匹配");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 精确 stem 命中优先于剥后缀命中:两轮各取列表首个,`note.md.md`
    /// (stem 恰为「note.md」)被精确轮拿走,不因列表序在前的 `a/note.md`
    /// 剥后缀先命中而错开 —— 既有「列表首个命中」优先级不变。
    #[test]
    fn find_by_name_prefers_exact_stem_over_stripped() {
        let root = vault("priority");
        std::fs::create_dir(root.join("a")).unwrap();
        std::fs::write(root.join("a/note.md"), "x").unwrap();
        std::fs::write(root.join("note.md.md"), "x").unwrap();
        // list_files 按相对路径排序,a/note.md 排在 note.md.md 之前
        assert_eq!(
            find_by_name(&root, "note.md"),
            Some(root.join("note.md.md")),
            "精确 stem 命中先于剥后缀命中,不被列表顺序抢走"
        );
        assert_eq!(
            find_by_name(&root, "note"),
            Some(root.join("a/note.md")),
            "无精确命中时剥后缀轮照旧取列表首个"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 路径式目标口径不动:直取命中照旧;直取不中也不回落到剥后缀的
    /// 模糊匹配(盘上是 `.markdown` 时,`子目录/Note One.md` 不再被打开)。
    #[test]
    fn find_by_name_path_style_targets_do_not_strip_suffix() {
        let root = vault("path-suffix");
        assert_eq!(
            find_by_name(&root, "子目录/Note One.markdown"),
            Some(root.join("子目录/Note One.markdown")),
            "原样直取命中(既有行为)"
        );
        assert_eq!(
            find_by_name(&root, "子目录/Note One.md"),
            None,
            "路径式不剥后缀:盘上只有 .markdown,直取不中即 None"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
