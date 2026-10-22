//! 侧边栏反向链接面板的状态机(#15 BK2,docs/auto-plan.md)。
//!
//! **扫描核心在 `latermd_search::backlinks`**(BK1 的单一实现,`.gitignore`
//! 语义与匹配口径都在那里);本模块只留**面板关注点**:目标快照比对、防抖
//! 计时、后台线程与结果缓存 —— 与 [`crate::search`] 的分工同构。
//!
//! 触发口径(decisions-pending #88):**帧末快照比对**,不逐入口插桩。
//! `State::end_of_logic` 把「(文件树根, 当前文档路径)」与
//! [`BacklinkState::requested_for`] 比对,变了才 [`BacklinkState::invalidate`]
//! 顺延防抖 —— 打开/切换/保存/另存/换根全部入口一处覆盖,与
//! `layout_written` 比对写盘同手法。文档**内容**变化不触发:反向链接匹配
//! 的是路径不是正文,每次敲字全仓重扫不值得。
//!
//! 后台形态:一次性直扫 + 单条结果经 channel 回传(bed.rs 的手法):新请求
//! `start` 时换新 channel 并推进序号,旧线程的结果要么发往已无人接收的
//! 旧 channel(发送失败即退出),要么带旧序号被 [`BacklinkState::poll`]
//! 丢弃 —— 不需要 SearchService 的代际原子,也没有流式增量。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use crate::repaint::RepaintNotifier;
use latermd_search::{backlinks, BacklinkOutcome, MAX_HITS};
// 面板行的载荷类型,消费方(`ui::sidebar`)从本模块取(与 `crate::search`
// re-export latermd_search 类型的同款分工)
pub use latermd_search::Backlink;

/// 反向链接面板状态机的当前档位。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum BacklinkStatus {
    /// 无扫描在途(初始、目标变更后的防抖等待、被复位)。
    #[default]
    Idle,
    /// 后台线程在跑。
    Scanning,
    /// 一次扫描自然结束(零命中时「没有文档链接到这里」的提示靠它与
    /// Idle 区分)。
    Finished,
    /// 后台线程启动失败或意外中断;消息可直接展示。
    Failed(String),
}

/// 反向链接面板状态:目标快照 + 防抖计时 + 后台接收端 + 结果缓存。
#[derive(Debug, Default)]
pub struct BacklinkState {
    /// 上次请求扫描的目标(文件树根, 当前文档路径);`requested_for` 是
    /// **帧末比对键**,变化即 invalidate。存快照而不是逐入口插桩的写入,
    /// 覆盖全部文档变更入口(见模块文档)。
    pub requested_for: Option<(PathBuf, PathBuf)>,
    /// 防抖到点时刻;目标每次变化顺延,发起或复位时清空。
    pub debounce_due: Option<Instant>,
    /// 最新一次发起的序号:只接受与它相等的结果(防旧覆盖)。
    /// crate 内可见只因 UI 无头测试要用 struct 字面量构造本结构(与
    /// `FindBarState::scanned_for` 同先例),外部无消费者。
    pub(crate) scan_seq: u64,
    /// 在途扫描的接收端;`None` = 空闲。可见性同上。
    pub(crate) rx: Option<Receiver<(u64, BacklinkOutcome)>>,
    /// 已收到的反向链接(上限 [`MAX_HITS`])。
    pub links: Vec<Backlink>,
    /// 条数达到上限被截断(提示行数据源)。
    pub truncated: bool,
    pub status: BacklinkStatus,
}

impl BacklinkState {
    /// 扫描目标(根或文档)变化:作废旧结果、回 Idle、顺延防抖 `delay`。
    /// 在途线程不必显式取消 —— 它的结果发往旧 channel,新 `start` 换新
    /// channel 后旧结果无处可去(见模块文档)。
    pub fn invalidate(&mut self, delay: Duration) {
        self.links.clear();
        self.truncated = false;
        self.status = BacklinkStatus::Idle;
        self.debounce_due = Some(Instant::now() + delay);
    }

    /// 整体复位(无根或文档未落盘):同 [`BacklinkState::invalidate`] 但
    /// **不再排程** —— 没有可扫的目标,重扫由下一次目标变更触发。
    pub fn reset(&mut self) {
        self.links.clear();
        self.truncated = false;
        self.status = BacklinkStatus::Idle;
        self.debounce_due = None;
    }

    /// 防抖到点发起(`Message::BacklinksRequested` 的归约):根与文档都
    /// 在才扫;否则复位(目标缺失时面板靠 `ui` 层的引导提示,不扫)。
    /// spawn 失败当场落 Failed,不留卡死的「扫描中」。
    #[cfg(test)]
    pub fn start(&mut self, root: Option<&Path>, doc: Option<&Path>) {
        self.start_with_repaint(root, doc, RepaintNotifier::default());
    }

    /// 发起扫描并在单条扫描结果入队后唤醒 UI。根与文档来自当前工作区，
    /// 结果仍由 [`Self::poll`] 按序号验新鲜度后写入面板状态。
    pub fn start_with_repaint(
        &mut self,
        root: Option<&Path>,
        doc: Option<&Path>,
        repaint: RepaintNotifier,
    ) {
        self.debounce_due = None;
        let (Some(root), Some(doc)) = (root, doc) else {
            self.reset();
            return;
        };
        self.links.clear();
        self.truncated = false;
        self.scan_seq += 1;
        let seq = self.scan_seq;
        let (tx, rx) = mpsc::channel();
        let root = root.to_path_buf();
        let doc = doc.to_path_buf();
        let spawn = std::thread::Builder::new()
            .name("latermd-backlinks".to_owned())
            .spawn(move || {
                let outcome = backlinks(&root, &doc, || false, MAX_HITS);
                if tx.send((seq, outcome)).is_ok() {
                    repaint.notify();
                }
            });
        match spawn {
            Ok(_join) => {
                self.rx = Some(rx);
                self.status = BacklinkStatus::Scanning;
            }
            Err(error) => {
                self.rx = None;
                self.status = BacklinkStatus::Failed(format!("反向链接线程启动失败: {error}"));
            }
        }
    }

    /// 每帧归约末尾收一次结果:非阻塞,拿到即整表替换;序号不匹配的迟到
    /// 结果丢弃。断连却没等到结果(worker 异常退出)落 Failed —— 「扫描中」
    /// 绝不能卡死。
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else {
            return;
        };
        match rx.try_recv() {
            Ok((seq, outcome)) => {
                if seq == self.scan_seq {
                    self.links = outcome.backlinks;
                    self.truncated = outcome.truncated;
                    self.status = BacklinkStatus::Finished;
                }
                // 这条 channel 已交出唯一结果(旧代迟到也一并收口)
                self.rx = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.rx = None;
                self.status = BacklinkStatus::Failed("反向链接扫描意外中断".to_owned());
            }
        }
    }

    /// 是否仍在等待扫描(进行中或防抖排程中;驱动重绘的判据)。
    #[cfg(test)]
    pub fn is_scanning(&self) -> bool {
        self.status == BacklinkStatus::Scanning || self.debounce_due.is_some()
    }
}

/// 来源路径 → 跳转目标串:保留目录、末段剥一个 `.md`/`.markdown` 后缀
/// (`a.md` → `a`,`notes/c.md` → `notes/c`)。与 `[[wikilink]]` 目标写法
/// 对称,`find_by_name` 的两把钥匙(文件名 stem / 相对路径)都吃得下。
pub(crate) fn jump_target(path: &Path) -> String {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return path.to_string_lossy().into_owned();
    };
    let stem = name
        .strip_suffix(".markdown")
        .or_else(|| name.strip_suffix(".md"))
        .unwrap_or(name);
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => {
            format!("{}/{}", dir.to_string_lossy(), stem)
        }
        _ => stem.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Message;
    use std::time::Duration;

    /// 进程内唯一的临时目录;测试自删。
    fn temp_vault(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "latermd-backlinkpanel-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 反向链接样本库:`note.md` 被 `a.md` 与子目录 `sub/b.md` 引用。
    fn vault(name: &str) -> PathBuf {
        let root = temp_vault(name);
        std::fs::write(root.join("note.md"), "目标文档\n").unwrap();
        std::fs::write(root.join("a.md"), "第一行\n见 [[note]]\n").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/b.md"), "子目录来源 [[note.md]]\n").unwrap();
        root
    }

    /// 轮询收流到 Finished(线程异步,限时等待)。
    fn wait_finished(state: &mut BacklinkState) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while state.status == BacklinkStatus::Scanning && Instant::now() < deadline {
            state.poll();
            if state.status == BacklinkStatus::Scanning {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    /// 状态机主链路:发起 → Scanning → 后台线程直扫 → poll 整表替换,
    /// 结果按 BK1 的排序口径(来源路径, 行号, 目标)。
    #[test]
    fn start_scans_in_background_and_poll_replaces_results() {
        let root = vault("chain");
        let mut state = BacklinkState::default();

        state.start(Some(&root), Some(&root.join("note.md")));
        assert_eq!(state.status, BacklinkStatus::Scanning);
        assert!(state.is_scanning());
        assert!(state.debounce_due.is_none(), "发起即清去抖计时");

        wait_finished(&mut state);
        assert_eq!(state.status, BacklinkStatus::Finished);
        assert!(!state.is_scanning(), "收尾后不再驱动重绘");
        let got: Vec<_> = state
            .links
            .iter()
            .map(|link| (link.path.to_string_lossy().replace('\\', "/"), link.line_no))
            .collect();
        assert_eq!(
            got,
            vec![("a.md".to_owned(), 2), ("sub/b.md".to_owned(), 1)],
            "两条引用按来源路径排序"
        );
        assert!(state
            .links
            .iter()
            .all(|link| link.line_text.contains("[[note")));
        assert!(!state.truncated);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 切换文档触发重扫 + 防抖合并:目标变更 invalidate 清旧结果并顺延
    /// 计时;防抖窗口内再变更,计时被**顺延**而非叠加两次扫描;到点只扫
    /// 最后一个目标。发起后再次 invalidate(目标又变)使在途结果作废,
    /// 旧 channel 被替换,新扫描的结果是唯一能到达的。
    #[test]
    fn doc_change_invalidates_reschedules_and_supersedes() {
        let root = vault("rescan");
        let mut state = BacklinkState::default();
        let note = root.join("note.md");
        let plain = root.join("a.md"); // a.md 没有反向链接(它链接别人)

        // 目标变更:清结果 + Idle + 计时置位
        state.invalidate(Duration::from_millis(300));
        assert_eq!(state.status, BacklinkStatus::Idle);
        assert!(state.debounce_due.is_some());
        let first_due = state.debounce_due.unwrap();

        // 防抖合并:窗口内再次变更,计时顺延(合并成一次扫描)
        std::thread::sleep(Duration::from_millis(5));
        state.invalidate(Duration::from_millis(300));
        assert!(
            state.debounce_due.unwrap() > first_due,
            "二次变更顺延计时,两次变更合并为一次扫描"
        );

        // 到点只扫最后目标:note 有引用
        state.start(Some(&root), Some(&note));
        wait_finished(&mut state);
        assert_eq!(state.links.len(), 2);

        // 切到无引用文档:invalidate 清掉旧结果;到点重扫为空表 + Finished
        state.invalidate(Duration::from_millis(0));
        assert!(state.links.is_empty(), "目标变更即清旧结果");
        state.start(Some(&root), Some(&plain));
        wait_finished(&mut state);
        assert_eq!(state.status, BacklinkStatus::Finished);
        assert!(state.links.is_empty(), "a.md 无人链接,零命中如实显示");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 防旧覆盖:第一次扫描在途时换目标再发起,旧 channel 被替换 —— 即使
    /// 旧结果已发出也无处可去,能到达 poll 的只有最新序号的结果。
    #[test]
    fn stale_scan_result_is_dropped_by_seq() {
        let root = vault("stale");
        let mut state = BacklinkState::default();
        state.start(Some(&root), Some(&root.join("note.md"))); // seq 1
        state.start(Some(&root), Some(&root.join("a.md"))); // seq 2 取代之
        wait_finished(&mut state);
        assert_eq!(state.status, BacklinkStatus::Finished);
        assert!(
            state.links.is_empty(),
            "只有 seq 2(a.md,零引用)的结果能到达;seq 1 的两条被丢弃"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 目标缺失(无根 / 文档未落盘):start 复位而非扫描,不再排程。
    #[test]
    fn start_without_target_resets() {
        let root = vault("no-target");
        let mut state = BacklinkState::default();
        state.invalidate(Duration::from_millis(300));
        state.start(None, Some(&root.join("note.md")));
        assert_eq!(state.status, BacklinkStatus::Idle);
        assert_eq!(state.debounce_due, None, "无可扫目标不再排程");
        state.invalidate(Duration::from_millis(300));
        state.start(Some(&root), None);
        assert_eq!(state.status, BacklinkStatus::Idle);
        assert_eq!(state.debounce_due, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 断连兜底:发送端没发出结果就退出 → 补 Failed,「扫描中」不卡死。
    #[test]
    fn disconnected_without_result_fails() {
        let (_tx, rx): (mpsc::Sender<(u64, BacklinkOutcome)>, _) = mpsc::channel();
        drop(_tx);
        let mut state = BacklinkState {
            scan_seq: 3,
            rx: Some(rx),
            status: BacklinkStatus::Scanning,
            ..BacklinkState::default()
        };
        state.poll();
        assert_eq!(
            state.status,
            BacklinkStatus::Failed("反向链接扫描意外中断".to_owned())
        );
        assert!(!state.is_scanning());
    }

    /// poll 空闲时(无接收端)是无操作,不 panic。
    #[test]
    fn poll_when_idle_is_noop() {
        let mut state = BacklinkState::default();
        state.poll();
        assert_eq!(state.status, BacklinkStatus::Idle);
    }

    /// 跳转目标串:剥后缀、留目录、无后缀原样、Windows 分隔符不参与。
    #[test]
    fn jump_target_strips_extension_keeps_dir() {
        assert_eq!(jump_target(Path::new("a.md")), "a");
        assert_eq!(jump_target(Path::new("b.markdown")), "b");
        assert_eq!(jump_target(Path::new("sub/c.md")), "sub/c");
        assert_eq!(
            jump_target(Path::new("深 度/笔 记.markdown")),
            "深 度/笔 记"
        );
        assert_eq!(jump_target(Path::new("noext")), "noext");
    }

    /// Message 路由的载荷类型在编译期对齐(点击行 → WikilinkClicked)。
    #[test]
    fn click_message_carries_jump_target() {
        let link = Backlink {
            path: PathBuf::from("sub/b.md"),
            line_no: 1,
            target: "note".to_owned(),
            line_text: "子目录来源 [[note.md]]".to_owned(),
        };
        assert_eq!(
            Message::WikilinkClicked {
                target: jump_target(&link.path)
            },
            Message::WikilinkClicked {
                target: "sub/b".to_owned()
            }
        );
    }
}
