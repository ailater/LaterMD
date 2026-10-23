//! 图床的应用侧接线(docs/image-plan.md C 段)。
//!
//! 分工与 [`crate::ai`] 同款「发起 / 接收 / 收尾」三原语:
//! * **发起**([`BedState::start_notifying`]):归约里 spawn 后台线程,阻塞的
//!   ureq 上传(含读文件、读钥匙串)全部离 UI 线程;channel 无界,
//!   UI 侧只在每帧归约里 `try_recv`。
//! * **接收**([`BedState::poll`]):非阻塞收空 channel,结果翻成
//!   [`Message::ImageUploadFinished`]。
//! * **收尾**(`State::finish_image_upload`):**只接受最新序号**的结果
//!   (防旧请求覆盖 —— 同款手法照抄 AI 流式的防重入;旧结果静默丢弃,
//!   不插文本不弹错),成功按用途插入文档或回显测试结果。
//!
//! 失败的唯一后果是**不插入文本**:文档与选区绝不动(image-plan §4.3),
//! 只落提示行。上传绑定发起标签([`BedState::upload_tab`]):期间切标签
//! 不改写入目标,发起标签被关则结果丢弃(与 `ai_active_tab` 同不变量)。
//!
//! token 一律经 latermd-creds(service=[`CREDS_SERVICE`]、account=profile
//! id)在**后台线程**读取,`beds.json` 里只有占位符,绝无明文。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use latermd_bed::{BedProfile, BedUploader, UreqUploader};

use crate::repaint::RepaintNotifier;
use crate::state::Message;

/// profile 列表落盘文件(与 settings.json 同目录)。
const BEDS_FILE: &str = "beds.json";

/// latermd-creds 的 service 名(docs/image-plan.md §3.C 定死):
/// account = profile id,凭据只进系统库。
pub const CREDS_SERVICE: &str = "latermd-bed";

/// 新 profile 的 id:纳秒时间戳 + 进程内计数。规格口径是「uuid」—— 这里
/// 取其**唯一**语义而非 RFC 4122 形态(id 同时是 creds 的 account 键,
/// 可读性比格式合规更有用),不为此引 uuid crate。
pub fn new_profile_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("bed-{nanos:x}-{}", SEQ.fetch_add(1, Ordering::Relaxed))
}

/// 一次上传完成后的用途:决定 [`Message::ImageUploadFinished`] 的归约走向。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BedUploadPurpose {
    /// 插入发起标签的文档(图片框「上传」入口);`alt` 在发起时定格。
    Insert { alt: String },
    /// 设置页「测试上传」:只回显结果,不碰任何文档。
    Test { profile_name: String },
}

/// 后台线程的单次上传结果载荷。
pub(crate) type UploadResult = (u64, Result<String, String>);

/// 图床状态机:profile 列表 + 在途上传的接收端。
#[derive(Default)]
pub struct BedState {
    /// 已保存的 profile(`beds.json`)。
    pub profiles: Vec<BedProfile>,
    /// 最新一次发起的序号:只接受与它相等的结果(防旧覆盖)。
    pub(crate) upload_seq: u64,
    /// 在途上传的接收端;`None` = 空闲。
    pub(crate) rx: Option<Receiver<UploadResult>>,
    /// 最新一次请求的用途(与 `upload_seq` 配对,收尾时消费)。
    pub(crate) purpose: Option<BedUploadPurpose>,
    /// 插入型上传的发起标签 id(测试上传为 `None`)。
    pub(crate) upload_tab: Option<u64>,
    /// 本次 channel 是否已交出过结果:断连兜底(线程 panic 没发出结果)
    /// 的判定依据,见 [`BedState::poll`]。
    saw_result: bool,
    /// 最近一次测试上传的结果(设置页回显):profile 名 + 成功 URL /
    /// 失败原因。
    pub last_test: Option<(String, Result<String, String>)>,
}

impl BedState {
    /// 从目录读 `beds.json`;文件缺失或解析失败 = 空列表(坏配置不挡启动,
    /// 与 `ai_config::AiConfig::load_from` 同口径)。
    pub fn load_from(dir: &Path) -> Vec<BedProfile> {
        let Ok(bytes) = std::fs::read(dir.join(BEDS_FILE)) else {
            return Vec::new();
        };
        match serde_json::from_slice(&bytes) {
            Ok(profiles) => profiles,
            Err(source) => {
                eprintln!("LaterMD: 图床配置解析失败,已忽略: {source}");
                Vec::new()
            }
        }
    }

    /// 落盘 `beds.json`;目录不存在则创建。
    pub fn save_to(profiles: &[BedProfile], dir: &Path) -> Result<(), String> {
        let path = dir.join(BEDS_FILE);
        let json = serde_json::to_string_pretty(profiles)
            .map_err(|source| format!("{}: {source}", path.display()))?;
        std::fs::create_dir_all(dir).map_err(|source| format!("{}: {source}", dir.display()))?;
        std::fs::write(&path, json.as_bytes())
            .map_err(|source| format!("{}: {source}", path.display()))
    }

    /// 是否有上传在途，供 UI 禁用重复提交；完成帧由结果通知主动唤醒。
    pub fn is_uploading(&self) -> bool {
        self.rx.is_some()
    }

    /// 按 id 找 profile。
    pub fn profile(&self, id: &str) -> Option<&BedProfile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    /// 生产归约入口：上传参数来自图片对话框或图床测试，后台结果成功入队
    /// 后主动唤醒 UI；真正的插入、提示和状态迁移仍由归约侧完成。
    pub fn start_notifying(
        &mut self,
        profile: BedProfile,
        path: PathBuf,
        purpose: BedUploadPurpose,
        tab: Option<u64>,
        repaint: RepaintNotifier,
    ) {
        self.start_with_notify(
            profile,
            path,
            purpose,
            tab,
            Arc::new(UreqUploader::new()),
            repaint,
        );
    }

    /// [`BedState::start_notifying`] 的注入版:测试塞假 uploader,免真实网络。
    #[cfg(test)]
    pub(crate) fn start_with(
        &mut self,
        profile: BedProfile,
        path: PathBuf,
        purpose: BedUploadPurpose,
        tab: Option<u64>,
        uploader: Arc<dyn BedUploader>,
    ) {
        self.start_with_notify(
            profile,
            path,
            purpose,
            tab,
            uploader,
            RepaintNotifier::default(),
        );
    }

    /// 上传实现的统一入口。测试可注入 uploader，生产可同时注入通知器；
    /// 返回值通过 channel 交给 UI 线程，不在工作线程改编辑器或设置状态。
    fn start_with_notify(
        &mut self,
        profile: BedProfile,
        path: PathBuf,
        purpose: BedUploadPurpose,
        tab: Option<u64>,
        uploader: Arc<dyn BedUploader>,
        repaint: RepaintNotifier,
    ) {
        self.upload_seq += 1;
        let seq = self.upload_seq;
        self.purpose = Some(purpose);
        self.upload_tab = tab;
        self.saw_result = false;
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        // 不 join:线程只往 channel 发一条结果,发送失败(接收端已被更新的
        // 请求替换)即退出 —— 与 AI 流式 worker 同款,不滞留句柄
        let spawn = std::thread::Builder::new()
            .name("latermd-bed-upload".to_owned())
            .spawn(move || {
                let result = run_upload(uploader.as_ref(), &profile, &path);
                if tx.send((seq, result)).is_ok() {
                    repaint.notify();
                }
            });
        if spawn.is_err() {
            // 线程都起不来:当场按失败收尾,不留卡死的「上传中」
            self.rx = None;
            self.purpose = None;
        }
    }

    /// 非阻塞收空 channel,结果翻成 [`Message::ImageUploadFinished`]。
    /// 生命周期的收口(清 rx/purpose)在归约侧的 finish —— 状态变更只发
    /// 生在 `apply`(与 `AiState::poll` 同分工)。
    ///
    /// 断连却没等到结果(worker 异常退出)补一条失败:上传中标志绝不能
    /// 卡死,否则按钮永久禁用。
    pub fn poll(&mut self) -> Vec<Message> {
        let Some(rx) = &self.rx else {
            return Vec::new();
        };
        let mut messages = Vec::new();
        loop {
            match rx.try_recv() {
                Ok((seq, result)) => {
                    self.saw_result = true;
                    messages.push(Message::ImageUploadFinished { seq, result });
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.saw_result {
                        messages.push(Message::ImageUploadFinished {
                            seq: self.upload_seq,
                            result: Err("图床上传意外中断".to_owned()),
                        });
                    }
                    break;
                }
            }
        }
        messages
    }

    /// 收尾(归约侧在 seq 匹配时调用):清在途状态,交出本次请求的用途
    /// 与发起标签。`accepts(seq)` 为假表示旧结果,调用方不进本方法。
    pub(crate) fn finish(&mut self, seq: u64) -> bool {
        if seq != self.upload_seq {
            return false;
        }
        self.rx = None;
        self.saw_result = false;
        true
    }

    /// 收尾时消费的用途(与 `finish` 配对;`finish` 已确认 seq 匹配)。
    pub(crate) fn take_purpose(&mut self) -> Option<BedUploadPurpose> {
        self.purpose.take()
    }

    /// 收尾时消费的发起标签 id。
    pub(crate) fn take_upload_tab(&mut self) -> Option<u64> {
        self.upload_tab.take()
    }
}

/// 后台线程的一次完整上传:读钥匙串 → 读文件 → 上传。全部阻塞 IO 都在
/// 这条线程上,失败文案直接面向提示行。
fn run_upload(
    uploader: &dyn BedUploader,
    profile: &BedProfile,
    path: &Path,
) -> Result<String, String> {
    let token = latermd_creds::get_secret(CREDS_SERVICE, &profile.id)
        .ok()
        .flatten();
    let bytes =
        std::fs::read(path).map_err(|error| format!("读取图片失败 {}: {error}", path.display()))?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    uploader
        .upload(profile, token.as_deref(), &bytes, &name)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Sender;
    use std::time::Duration;

    /// 假上传器:按文件名返回固定结果,不碰网络。`Uploads` 记录收到的
    /// 调用参数供断言。
    struct MockUploader {
        results: std::sync::Mutex<Vec<(String, Option<String>, String)>>,
    }

    impl MockUploader {
        fn shared() -> Arc<Self> {
            Arc::new(Self {
                results: std::sync::Mutex::new(Vec::new()),
            })
        }
    }

    impl BedUploader for MockUploader {
        fn upload(
            &self,
            profile: &BedProfile,
            token: Option<&str>,
            _bytes: &[u8],
            file_name: &str,
        ) -> Result<String, latermd_bed::BedError> {
            self.results.lock().unwrap().push((
                profile.id.clone(),
                token.map(str::to_owned),
                file_name.to_owned(),
            ));
            match file_name {
                "fail.png" => Err(latermd_bed::BedError::HttpStatus {
                    code: 500,
                    detail: "boom".to_owned(),
                }),
                name => Ok(format!("https://bed.example/{name}")),
            }
        }
    }

    fn profile(id: &str) -> BedProfile {
        let mut profile = BedProfile::preset_smms();
        profile.id = id.to_owned();
        profile
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("latermd-bed-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn temp_image(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, b"png-bytes").unwrap();
        path
    }

    /// 收流直到拿到一条 ImageUploadFinished(线程异步,轮询有限时)。
    fn wait_finished(bed: &mut BedState) -> Message {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if let Some(message) = bed.poll().pop() {
                return message;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("10s 内没等到上传结果");
    }

    /// beds.json 往返:字段无损;文件内容不含 token 字样(除 `${TOKEN}`
    /// 占位符外)。token 存取走钥匙串,与此文件无关。
    #[test]
    fn beds_json_round_trip_without_tokens() {
        let dir = temp_dir("roundtrip");
        let mut profiles = vec![profile("p-1"), BedProfile::preset_github()];
        profiles[1].id = "p-2".to_owned();
        BedState::save_to(&profiles, &dir).unwrap();
        let raw = std::fs::read_to_string(dir.join(BEDS_FILE)).unwrap();
        assert!(!raw.contains("\"token\""), "没有 token 字段:{raw}");
        assert_eq!(BedState::load_from(&dir), profiles);
        // 目录缺失/文件缺失 = 空列表;坏 JSON 也是空列表 + 不 panic
        assert!(BedState::load_from(Path::new("/latermd/不存在目录")).is_empty());
        std::fs::write(dir.join(BEDS_FILE), b"{ not json").unwrap();
        assert!(BedState::load_from(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 全链路(免网络):start → 后台线程 → poll 翻成 ImageUploadFinished,
    /// 载荷带最新 seq;上传器收到的文件名来自路径,token 读取失败(无
    /// 钥匙串环境)不拦上传(SM.MS 之外的免凭据图床是合法配置)。
    #[test]
    fn start_polls_finished_with_latest_seq() {
        let dir = temp_dir("chain");
        let image = temp_image(&dir, "图.png");
        let mock = MockUploader::shared();
        let mut bed = BedState::default();
        bed.start_with(
            profile("p-1"),
            image,
            BedUploadPurpose::Insert {
                alt: "示意".to_owned(),
            },
            None,
            mock.clone(),
        );
        assert!(bed.is_uploading());
        assert_eq!(
            wait_finished(&mut bed),
            Message::ImageUploadFinished {
                seq: 1,
                result: Ok("https://bed.example/图.png".to_owned())
            }
        );
        let calls = mock.results.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "p-1");
        assert_eq!(calls[0].2, "图.png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 防旧覆盖:新请求把序号推到 2 并**替换接收端**,第一次请求的 channel
    /// 随之被丢弃 —— 它的结果(无论是否已发出)无处可去,能到达归约的只有
    /// seq=2;`finish` 对旧 seq 拒收。两次发起之间不 poll,保证第一次的
    /// channel 先被替换(结果到达路径只剩第二条)。
    #[test]
    fn stale_results_are_rejected_by_seq() {
        let dir = temp_dir("stale");
        let first = temp_image(&dir, "a.png");
        let second = temp_image(&dir, "b.png");
        let mut bed = BedState::default();
        bed.start_with(
            profile("p-1"),
            first,
            BedUploadPurpose::Test {
                profile_name: "一".to_owned(),
            },
            None,
            MockUploader::shared(),
        );
        bed.start_with(
            profile("p-1"),
            second,
            BedUploadPurpose::Test {
                profile_name: "二".to_owned(),
            },
            None,
            MockUploader::shared(),
        );
        assert!(bed.is_uploading());
        // 只等一条结果(第二条的);第一条即使已发出也被替换丢弃
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut got = Vec::new();
        while got.is_empty() && std::time::Instant::now() < deadline {
            got.extend(bed.poll());
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            got,
            vec![Message::ImageUploadFinished {
                seq: 2,
                result: Ok("https://bed.example/b.png".to_owned())
            }],
            "只有最新请求的结果能到达"
        );
        // 旧 seq 拒收(消息层兜底:即使将来有同 channel 复用的改法也不回退)
        assert!(!bed.finish(1), "旧序号拒收");
        assert!(bed.is_uploading(), "拒收旧结果不清在途状态");
        assert!(bed.finish(2));
        assert!(!bed.is_uploading(), "最新序号收尾后空闲");
        assert!(matches!(
            bed.take_purpose(),
            Some(BedUploadPurpose::Test { .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 断连兜底:发送端没发出结果就退出(worker 异常/panic 的可观察形态
    /// 就是 channel 断连)→ 补一条失败,且 seq 是最新值 —— 否则「上传中」
    /// 永久卡死,按钮再不可点。
    #[test]
    fn disconnected_without_result_synthesizes_failure() {
        let (tx, rx): (Sender<UploadResult>, Receiver<UploadResult>) = mpsc::channel();
        drop(tx);
        let mut bed = BedState {
            upload_seq: 7,
            purpose: Some(BedUploadPurpose::Test {
                profile_name: "n".to_owned(),
            }),
            rx: Some(rx),
            ..BedState::default()
        };
        assert_eq!(
            bed.poll(),
            vec![Message::ImageUploadFinished {
                seq: 7,
                result: Err("图床上传意外中断".to_owned())
            }]
        );
        assert!(bed.finish(7));
    }

    /// 失败结果照常翻消息(归约侧只弹 notice 的载荷来源)。
    #[test]
    fn failed_upload_maps_to_err_payload() {
        let dir = temp_dir("fail");
        let image = temp_image(&dir, "fail.png");
        let mut bed = BedState::default();
        bed.start_with(
            profile("p-1"),
            image,
            BedUploadPurpose::Test {
                profile_name: "t".to_owned(),
            },
            None,
            MockUploader::shared(),
        );
        assert_eq!(
            wait_finished(&mut bed),
            Message::ImageUploadFinished {
                seq: 1,
                result: Err("图床返回 HTTP 500:boom".to_owned())
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
