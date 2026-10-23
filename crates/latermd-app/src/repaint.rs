//! 后台任务唤醒 egui 的应用层适配。
//!
//! 业务与核心 crate 不直接依赖 egui；后台生产者只持有本类型或普通回调。
//! 多条结果在 UI 开始下一轮收流前合并为一次唤醒，避免搜索命中和 AI
//! 分块密集到达时反复向事件循环投递相同请求。

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

type Callback = Arc<dyn Fn() + Send + Sync>;

struct Shared {
    pending: AtomicBool,
    callback: Mutex<Option<Callback>>,
}

/// 可跨线程克隆的重绘通知器。
///
/// UI 帧入口把 egui context 绑定进共享回调；后台线程只负责调用
/// [`Self::notify`]，不读取或修改任何界面状态。默认未绑定，便于无头测试。
#[derive(Clone)]
pub struct RepaintNotifier {
    shared: Arc<Shared>,
}

impl Default for RepaintNotifier {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared {
                pending: AtomicBool::new(false),
                callback: Mutex::new(None),
            }),
        }
    }
}

impl fmt::Debug for RepaintNotifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RepaintNotifier")
            .field("pending", &self.shared.pending.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl RepaintNotifier {
    /// 绑定当前应用的 egui context。输入来自每帧归约入口，返回值供所有
    /// 已存在的 clone 共享；绑定动作本身不会额外请求一帧。
    pub fn bind_context(&self, context: &eframe::egui::Context) {
        let context = context.clone();
        if let Ok(mut callback) = self.shared.callback.lock() {
            *callback = Some(Arc::new(move || context.request_repaint()));
        }
    }

    /// 标记 UI 即将收取后台事件。必须在 drain channel 前调用：先清零后，
    /// drain 期间新到的消息仍会重新唤醒，因此不会丢掉竞态结果。
    pub fn begin_drain(&self) {
        self.shared.pending.store(false, Ordering::Release);
    }

    /// 后台生产者成功发送结果后调用。首条结果请求一帧，同批后续结果在
    /// [`Self::begin_drain`] 前合并，降低事件循环和 CPU 的空转压力。
    pub fn notify(&self) {
        if self.shared.pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let callback = self
            .shared
            .callback
            .lock()
            .ok()
            .and_then(|callback| callback.clone());
        if let Some(callback) = callback {
            callback();
        } else {
            // 冷启动首帧前还没有 context，不能留下“已通知”的假状态；首帧
            // 会主动收流，之后到达的结果仍应拥有一次真实唤醒机会。
            self.shared.pending.store(false, Ordering::Release);
        }
    }

    /// 导出与 egui 解耦的普通回调。核心 crate 收到业务结果并成功入队后
    /// 调用它；回调只负责合并通知，不产生写盘、网络或业务状态副作用。
    pub fn callback(&self) -> Callback {
        let notifier = self.clone();
        Arc::new(move || notifier.notify())
    }

    /// 创建带主动唤醒的 channel。返回的发送端交给既有后台生产者，接收端
    /// 仍由 UI 非阻塞轮询；中间桥接线程只在转发成功后请求重绘，并在任一
    /// 端断开时退出。线程启动失败原样返回，由调用方落各自的失败状态。
    pub fn notifying_channel<T: Send + 'static>(
        &self,
        thread_name: &str,
    ) -> std::io::Result<(Sender<T>, Receiver<T>)> {
        let (producer_tx, producer_rx) = mpsc::channel();
        let (ui_tx, ui_rx) = mpsc::channel();
        let notifier = self.clone();
        std::thread::Builder::new()
            .name(thread_name.to_owned())
            .spawn(move || {
                loop {
                    match producer_rx.recv() {
                        Ok(value) => {
                            if ui_tx.send(value).is_err() {
                                break;
                            }
                            notifier.notify();
                        }
                        Err(_) => {
                            // 生产线程异常退出且没发终止消息时，也要给 UI 一
                            // 帧识别 Disconnected，避免“进行中”永久卡死。
                            notifier.notify();
                            break;
                        }
                    }
                }
            })?;
        Ok((producer_tx, ui_rx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    /// 同一批后台结果只触发一次回调；UI 开始 drain 后下一批能够再次唤醒。
    #[test]
    fn notifications_coalesce_until_the_next_drain() {
        let notifier = RepaintNotifier::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let sink = Arc::clone(&calls);
        *notifier.shared.callback.lock().unwrap() = Some(Arc::new(move || {
            sink.fetch_add(1, Ordering::SeqCst);
        }));

        notifier.notify();
        notifier.notify();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "同批通知合并");

        notifier.begin_drain();
        notifier.notify();
        assert_eq!(calls.load(Ordering::SeqCst), 2, "下一批重新唤醒");
    }

    /// 桥接 channel 不改变业务载荷，并且只在结果成功交给 UI 后发通知。
    #[test]
    fn notifying_channel_forwards_value_and_wakes() {
        let notifier = RepaintNotifier::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let sink = Arc::clone(&calls);
        *notifier.shared.callback.lock().unwrap() = Some(Arc::new(move || {
            sink.fetch_add(1, Ordering::SeqCst);
        }));
        let (tx, rx) = notifier.notifying_channel("latermd-repaint-test").unwrap();

        tx.send(42_u8).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(42));
        // recv 在值入队瞬间就返回，桥接线程的 notify() 在其后才执行，
        // 两者之间是调度窗口（高核数机器上可稳定复现）；轮询等待回调落地
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while calls.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "转发成功后应唤醒 UI");
    }
}
