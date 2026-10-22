//! MCP server 的运行时与状态(docs/mcp-plan.md §5 的 UI 侧)。
//!
//! 归约铁律照旧:本模块**只在归约里被调**(启停线程、落盘配置),UI 只
//! 展示 [`McpState`] 的字段并产出 `Message::McpConfigSaved`。
//!
//! 服务跑在**后台线程**上,三条约束:
//! - 线程不与 UI 共享可变状态,只共享三样东西:文档库根([`SharedRoot`])、
//!   调用计数(锁保护的表)、停止标志(原子布尔)。
//! - 绑定结果经一次性 channel 回传:端口被占用时状态行显示真实原因,而不是
//!   「开了但没开」的假象。
//! - 停止靠标志位 + 非阻塞 accept 轮询(见 `transport::http`),关闭设置页
//!   或退出应用时不 join 线程。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use latermd_mcp::{CallCounts, McpConfig, Server, SharedRoot};

use crate::repaint::RepaintNotifier;

/// 服务状态:设置页状态行与底部状态栏的唯一数据源。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum McpStatus {
    /// 未启用(默认)。
    #[default]
    Disabled,
    /// 已发起线程、等待端口绑定结果。
    Starting,
    /// 正在监听回环端口。
    Listening(u16),
    /// 端口占用等失败;文案可直接显示。
    Failed(String),
}

impl McpStatus {
    /// 状态行文案(设置页与状态栏共用,不各写一份)。
    pub fn label(&self) -> String {
        match self {
            Self::Disabled => "已关闭".to_owned(),
            Self::Starting => "启动中…".to_owned(),
            Self::Listening(port) => format!("监听 127.0.0.1:{port}"),
            Self::Failed(reason) => format!("启动失败: {reason}"),
        }
    }
}

/// MCP 运行时状态。
#[derive(Debug)]
pub struct McpState {
    /// 当前生效的配置(`mcp.json` 的镜像)。
    pub config: McpConfig,
    pub status: McpStatus,
    /// 每工具的累计调用次数(只读展示;服务关闭后保留,便于复盘)。
    pub counts: BTreeMap<String, u64>,
    /// 与后台 server 共享的文档库根:文件树换目录时改这里即可。
    root: SharedRoot,
    /// 运行中的线程停止标志;`None` = 没在跑。
    stop: Option<Arc<AtomicBool>>,
    /// 计数表句柄(绑定成功后拿到)。
    counts_handle: Option<CallCounts>,
    /// 绑定结果的一次性接收端。
    ready: Option<Receiver<Result<CallCounts, String>>>,
    /// 实际监听端口(Start 时记下,避免等待期间改端口导致状态行说谎)。
    listening_port: u16,
}

impl Default for McpState {
    fn default() -> Self {
        Self {
            config: McpConfig::default(),
            status: McpStatus::Disabled,
            counts: BTreeMap::new(),
            root: Arc::new(std::sync::Mutex::new(None)),
            stop: None,
            counts_handle: None,
            ready: None,
            listening_port: McpConfig::DEFAULT_PORT,
        }
    }
}

impl McpState {
    /// 应用新配置:**先停后起**(端口或工具开关变化都必须换线程重建 server)。
    /// 落盘由归约侧负责(本模块不碰磁盘,便于无头测试)。
    #[cfg(test)]
    pub fn apply_config(&mut self, config: McpConfig) {
        self.apply_config_with_repaint(config, RepaintNotifier::default());
    }

    /// 应用配置并让后台绑定结果主动唤醒 UI。配置来自设置页或启动恢复；
    /// 返回的监听状态和错误仍由 [`Self::poll`] 在 UI 线程落地。
    pub fn apply_config_with_repaint(&mut self, config: McpConfig, repaint: RepaintNotifier) {
        self.stop();
        self.config = config;
        self.config.normalize();
        if self.config.enabled {
            self.start(repaint);
        } else {
            self.status = McpStatus::Disabled;
        }
    }

    /// 文件树换根:直接改共享句柄,服务不必重启。
    pub fn set_root(&mut self, root: Option<PathBuf>) {
        if let Ok(mut slot) = self.root.lock() {
            *slot = root;
        }
    }

    /// 当前文档库根(设置页提示用)。
    pub fn root(&self) -> Option<PathBuf> {
        self.root.lock().ok().and_then(|slot| slot.clone())
    }

    /// 停止服务:置标志位即返回,线程在下一个 accept 轮询点(≤50ms)自行
    /// 退出。计数保留 —— 关掉服务不该抹掉「刚才被调了多少次」的事实。
    pub fn stop(&mut self) {
        if let Some(flag) = self.stop.take() {
            flag.store(true, Ordering::SeqCst);
        }
        self.ready = None;
        self.counts_handle = None;
    }

    /// 每帧收一次绑定结果并刷新计数(两者都极轻:一次 try_recv + 一次锁)。
    pub fn poll(&mut self) {
        if let Some(rx) = &self.ready {
            match rx.try_recv() {
                Ok(Ok(counts)) => {
                    self.counts_handle = Some(counts);
                    self.status = McpStatus::Listening(self.listening_port);
                    self.ready = None;
                }
                Ok(Err(reason)) => {
                    self.status = McpStatus::Failed(reason);
                    self.ready = None;
                    self.stop = None;
                }
                // 还没绑定完(Empty)或线程已退出(Disconnected):保持当前状态
                Err(_) => {}
            }
        }
        if let Some(handle) = &self.counts_handle {
            self.counts = Server::snapshot_counts(handle);
        }
    }

    /// 起后台线程。绑定失败不在这里报 —— 结果走 channel,由 [`Self::poll`]
    /// 落到状态行(让 UI 看到真实原因)。
    fn start(&mut self, repaint: RepaintNotifier) {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        self.ready = Some(rx);
        self.listening_port = self.config.http_port;
        self.status = McpStatus::Starting;

        let port = self.config.http_port;
        let config = self.config.clone();
        let root = Arc::clone(&self.root);
        let flag = Arc::clone(&stop);
        let spawned = std::thread::Builder::new()
            .name("latermd-mcp".into())
            .spawn(move || {
                match latermd_mcp::transport::http::bind(port) {
                    Ok(listener) => {
                        let server = Server::with_root_handle(root, config);
                        if tx.send(Ok(server.counts())).is_ok() {
                            repaint.notify();
                        }
                        if let Err(error) =
                            latermd_mcp::transport::http::serve_with(listener, server, flag)
                        {
                            eprintln!("LaterMD: MCP 服务线程退出: {error}");
                        }
                    }
                    Err(error) => {
                        // 端口占用是最常见的一种:文案要带端口,便于用户改
                        if tx
                            .send(Err(format!("{port} 端口无法绑定: {error}")))
                            .is_ok()
                        {
                            repaint.notify();
                        }
                    }
                }
            });
        match spawned {
            Ok(_) => self.stop = Some(stop),
            Err(error) => {
                self.status = McpStatus::Failed(format!("线程启动失败: {error}"));
                self.ready = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use latermd_mcp::ToolKind;

    /// 状态文案:关闭 / 启动中 / 监听 / 失败四态各一句,不重复造字符串。
    #[test]
    fn status_labels_cover_all_states() {
        assert_eq!(McpStatus::Disabled.label(), "已关闭");
        assert_eq!(McpStatus::Starting.label(), "启动中…");
        assert_eq!(McpStatus::Listening(8731).label(), "监听 127.0.0.1:8731");
        assert!(McpStatus::Failed("端口占用".into())
            .label()
            .contains("端口占用"));
    }

    /// 默认态:关闭、默认端口、五工具全开(与 `McpConfig::default` 同口径)。
    #[test]
    fn default_state_is_disabled() {
        let state = McpState::default();
        assert_eq!(state.status, McpStatus::Disabled);
        assert!(!state.config.enabled);
        assert_eq!(state.config.http_port, McpConfig::DEFAULT_PORT);
        for kind in ToolKind::ALL {
            assert!(state.config.tool_enabled(kind));
        }
    }

    /// 关闭配置:不起线程,状态回落 Disabled(开关是「关」时零副作用)。
    #[test]
    fn applying_disabled_config_does_not_start() {
        let mut state = McpState::default();
        state.apply_config(McpConfig::default());
        assert_eq!(state.status, McpStatus::Disabled);
        assert!(state.stop.is_none());
    }

    /// 越界端口在应用时被钳进合法区间(手改 mcp.json 的脏数据不该让服务
    /// 起不来,也不该占特权端口)。
    #[test]
    fn applying_config_normalizes_port() {
        let mut state = McpState::default();
        let config = McpConfig {
            enabled: false,
            http_port: 80,
            ..McpConfig::default()
        };
        state.apply_config(config);
        assert_eq!(state.config.http_port, *McpConfig::PORT_RANGE.start());
    }

    /// 换根只改共享句柄:不影响运行状态,也不重启服务。
    #[test]
    fn set_root_updates_shared_handle() {
        let mut state = McpState::default();
        assert_eq!(state.root(), None);
        state.set_root(Some(PathBuf::from("/vault")));
        assert_eq!(state.root(), Some(PathBuf::from("/vault")));
        assert_eq!(state.status, McpStatus::Disabled, "换根不起停服务");
    }

    /// 停止是幂等的:重复调用不 panic,计数保留(关闭服务不抹掉历史)。
    #[test]
    fn stop_is_idempotent_and_keeps_counts() {
        let mut state = McpState::default();
        state.counts.insert("search_docs".to_owned(), 3);
        state.stop();
        state.stop();
        assert_eq!(state.counts.get("search_docs"), Some(&3));
        assert!(state.stop.is_none());
    }
}
