//! 后台任务唤醒 egui 的应用层适配。
//!
//! 核心 crate 不依赖 egui；需要通知界面时只拿本类型导出的普通回调。
//! 多条结果在 UI 真正开始下一帧前会合并成一次唤醒，避免搜索命中或 AI
//! chunk 密集到达时反复向事件循环投递相同请求。

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

type Callback = Arc<dyn Fn() + Send + Sync>;

struct Shared {
    pending: AtomicBool,
    callback: Mutex<Option<Callback>>,
}

/// 可跨线程克隆的重绘通知器。
///
/// `bind_context` 由应用启动/帧入口提供 egui context；后台线程只调用
/// `notify`。通知器默认无绑定，因此无头测试和纯状态测试不会触碰 UI。
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
    /// 绑定当前应用的 egui context。输入来自 `CreationContext` 或帧入口，
    /// 之后所有既有 clone 都会共享这份回调；绑定本身不请求新帧。
    pub fn bind_context(&self, context: &eframe::egui::Context) {
        let context = context.clone();
        if let Ok(mut callback) = self.shared.callback.lock() {
            *callback = Some(Arc::new(move || context.request_repaint()));
        }
    }

    /// 标记 UI 已开始处理一次后台事件批次。必须在 drain channel 之前调：
    /// 清零后新到的事件会再次唤醒，因此不会丢掉 drain 期间的竞态消息。
    pub fn begin_drain(&self) {
        self.shared.pending.store(false, Ordering::Release);
    }

    /// 后台生产者发送结果成功后调用。首条结果请求一帧，后续结果在 UI
    /// `begin_drain` 前合并，避免一次搜索产生数百个系统重绘事件。
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
            // 尚未绑定 context 时不能留下“已通知”假状态；启动完成后第一条
            // 真正的后台结果仍应有机会唤醒 UI。
            self.shared.pending.store(false, Ordering::Release);
        }
    }

    /// 导出与 egui 解耦的回调，供 `latermd-search` 这类核心 crate 使用。
    /// 回调只负责合并并唤醒，不读取或修改任何业务状态。
    pub fn callback(&self) -> Callback {
        let notifier = self.clone();
        Arc::new(move || notifier.notify())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

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
}
