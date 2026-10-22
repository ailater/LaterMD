//! 剪贴板图片读取(docs/image-plan.md D 段)。
//!
//! ## 为什么绕开 egui 内建剪贴板
//!
//! egui 0.36.2 的剪贴板面只有**出**没有**进**:`Context::copy_text` /
//! `copy_image` 写,读仅 `egui-winit` 在 Ctrl+V 时同步 `get_text` 翻成
//! `Event::Paste(String)`(egui-winit/src/clipboard.rs 的 `Clipboard::get`
//! 只查文本;`set_image` 有、`get_image` 无)。剪贴板里的图片字节要自己
//! 取 —— 直接依赖 `arboard` 3.6.1(它已在依赖树里:eframe 默认开
//! `egui-winit/clipboard` → `arboard/image-data`,Cargo.lock 与
//! `cargo tree -i arboard` 均已核对,本模块把既有传递依赖提升为直接依赖,
//! 零新增编译面),登记见 ADR-004。
//!
//! ## 卡帧纪律
//!
//! arboard 的读 + 解码是阻塞 IO(X11 上要与剪贴板宿主进程握手),绝不能
//! 在归约里同步做:与 AI 流式 / 图床上传同款,spawn 后台线程 + mpsc,
//! 每帧归约 `try_recv`([`ClipboardState::poll`])。无序号防重入 —— 一次
//! 粘贴只产生一条结果,线程被替换时旧 channel 被 drop,结果无处可去
//! (与 `BedState` 换接收端同手法)。
//!
//! ## 编码口径
//!
//! arboard 回来的是 RGBA 像素(`ImageData`),落盘要文件字节:统一重编码
//! 成 **PNG**(白名单内、无损、三平台路径一致;Linux X11 后端读进来的
//! 本来就是 PNG,Windows 的 DIB 解码后重编码一次,macOS NSImage 同理)。
//! 编码用 `image` 0.25 —— 与 latermd-app 既有依赖同一个 crate 同一条
//! 版本线(png feature 已开)。

use std::sync::mpsc::{self, Receiver, TryRecvError};

use crate::repaint::RepaintNotifier;
use crate::state::Message;

/// 一次剪贴板读取的结果载荷:PNG 字节(成功)或面向用户的错误文案。
type ClipResult = Result<Vec<u8>, String>;

/// 剪贴板图片状态机:一次「发起 → 后台读 → 收流」的生命周期。
///
/// 发起([`Self::start`])在归约(Ctrl+V 且剪贴板无文本时),收流
/// ([`Self::poll`])每帧归约里跑,结果翻成 [`Message::ImagePasteFinished`]。
#[derive(Default)]
pub struct ClipboardState {
    /// 在途读取的接收端;`None` = 空闲。`pub(crate)` 仅为测试注入空壳
    /// (钉归约侧收尾而不真起线程,与 `BedState.rx` 同手法)。
    pub(crate) rx: Option<Receiver<ClipResult>>,
}

impl ClipboardState {
    /// 是否有读取在途(测试观察用；重绘由结果入队后的通知驱动)。
    #[cfg(test)]
    pub fn is_reading(&self) -> bool {
        self.rx.is_some()
    }

    /// 发起后台读取:spawn 线程做 arboard `get_image` + PNG 编码,立即
    /// 返回。已在读取中则**替换**旧 channel(旧线程的结果无处可去,与
    /// `BedState::start_with` 换接收端同手法)—— 粘贴是高频动作,等旧
    /// 结果不如直接以最新一次为准。`repaint` 只在结果成功入队后唤醒 UI。
    pub fn start(&mut self, repaint: RepaintNotifier) {
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let spawn = std::thread::Builder::new()
            .name("latermd-clipboard-image".to_owned())
            .spawn(move || {
                let result = read_clipboard_image();
                // 发送失败(已被更新的读取替换)即静默退出,不滞留
                if tx.send(result).is_ok() {
                    // 图片读取可能等待外部剪贴板进程；结果到达才唤醒 UI，
                    // 等待期间不再用满帧轮询消耗 CPU。
                    repaint.notify();
                }
            });
        if spawn.is_err() {
            // 线程都起不来:当场收口,不留卡死的「读取中」
            self.rx = None;
        }
    }

    /// 非阻塞收流,结果翻成 [`Message::ImagePasteFinished`]。一次读取恰产
    /// 一条结果,故 `match` 而非循环(与 AI/图床的「收空 channel」不同,
    /// 这里没有多块流)。接收端的清理由归约侧收尾
    /// (`State::finish_image_paste`)做,这里只翻消息 —— 状态变更只发生在
    /// `apply`,与 `AiState::poll` 同分工。
    pub fn poll(&mut self) -> Vec<Message> {
        let Some(rx) = &self.rx else {
            return Vec::new();
        };
        let result = match rx.try_recv() {
            Ok(result) => Some(result),
            // 结果未到:留着接收端,下一帧再问
            Err(TryRecvError::Empty) => None,
            // 断连却没等到结果(worker 异常退出):按失败收尾,
            // 「读取中」绝不能卡死(否则 Ctrl+V 永久失灵)
            Err(TryRecvError::Disconnected) => Some(Err("剪贴板图片读取意外中断".to_owned())),
        };
        result
            .map(|result| vec![Message::ImagePasteFinished { result }])
            .unwrap_or_default()
    }

    /// 收尾(归约侧调用):清接收端,允许下一次发起。
    pub fn finish(&mut self) {
        self.rx = None;
    }
}

/// 后台线程的一次完整读取:arboard 取 RGBA → PNG 编码。
/// 错误文案面向提示行;「剪贴板里没有图片」是最常见的合法空态
/// (用户按 Ctrl+V 时剪贴板只有文本),文案口径与降级路径对齐。
fn read_clipboard_image() -> ClipResult {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|error| format!("无法访问系统剪贴板:{error}"))?;
    let image = clipboard
        .get_image()
        .map_err(|error| format!("剪贴板里没有可用的图片({error})"))?;
    encode_png(&image)
}

/// RGBA 像素 → PNG 字节。
fn encode_png(image: &arboard::ImageData) -> ClipResult {
    use image::ImageEncoder as _;
    if image.bytes.is_empty() || image.width == 0 || image.height == 0 {
        return Err("剪贴板图片数据为空".to_owned());
    }
    let mut png = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut png);
    encoder
        .write_image(
            image.bytes.as_ref(),
            image.width as u32,
            image.height as u32,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|error| format!("图片编码失败:{error}"))?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PNG 编码往返:RGBA 四像素编成 PNG,首字节签名正确(解码侧是
    /// egui_extras 的 loader,这里只验「真是 PNG 且不 panic」)。
    #[test]
    fn encodes_rgba_pixels_into_png_bytes() {
        let image = arboard::ImageData {
            width: 2,
            height: 2,
            bytes: std::borrow::Cow::Borrowed(&[
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ]),
        };
        let png = encode_png(&image).unwrap();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    }

    /// 空数据(零尺寸 / 空字节)拒绝编码,不 panic。
    #[test]
    fn empty_image_data_is_rejected() {
        for bad in [
            (0, 0, &[][..]),
            (0, 2, &[1, 2, 3, 4][..]),
            (2, 0, &[1, 2, 3, 4][..]),
        ] {
            let image = arboard::ImageData {
                width: bad.0,
                height: bad.1,
                bytes: std::borrow::Cow::Borrowed(bad.2),
            };
            assert!(encode_png(&image).is_err(), "{bad:?} 应被拒绝");
        }
    }

    /// poll 翻消息:启动后(无剪贴板环境也能跑通线程),有限时间内收到
    /// 恰一条 `ImagePasteFinished`;收尾后 `is_reading` 归 false、poll 空。
    /// 结果本身(Ok/Err)随环境而变,不断言方向 —— 断言的是生命周期:
    /// 恰一条、不卡死、可收口。
    #[test]
    fn poll_yields_exactly_one_result_and_finishes() {
        let mut clipboard = ClipboardState::default();
        assert!(!clipboard.is_reading());
        clipboard.start(RepaintNotifier::default());
        assert!(clipboard.is_reading(), "发起即读取中");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut messages = Vec::new();
        while messages.is_empty() && std::time::Instant::now() < deadline {
            messages.extend(clipboard.poll());
            if messages.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert_eq!(messages.len(), 1, "一次读取恰一条结果:{messages:?}");
        assert!(matches!(messages[0], Message::ImagePasteFinished { .. }));
        clipboard.finish();
        assert!(!clipboard.is_reading(), "收尾后空闲");
        assert!(clipboard.poll().is_empty(), "收尾后不再翻消息");
    }

    /// 断连兜底:发送端没发结果就退出(worker 异常的可观察形态是 channel
    /// 断连)→ 补一条失败,「读取中」不卡死。
    #[test]
    fn disconnected_without_result_synthesizes_failure() {
        let (_tx, rx): (mpsc::Sender<ClipResult>, Receiver<ClipResult>) = mpsc::channel();
        drop(_tx);
        let mut clipboard = ClipboardState { rx: Some(rx) };
        assert_eq!(
            clipboard.poll(),
            vec![Message::ImagePasteFinished {
                result: Err("剪贴板图片读取意外中断".to_owned())
            }]
        );
    }
}
