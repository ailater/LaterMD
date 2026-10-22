//! 「关于 LaterMD」对话框(#71 M1):帮助菜单「关于 LaterMD…」的落点。
//!
//! 内容五件:应用名 + 版本(`CARGO_PKG_VERSION`,workspace 单一事实源)+
//! 一句话定位(AGENTS.md §1)+ 仓库链接(egui `open_url` 交系统浏览器,
//! 与预览外链同路)+ MIT 许可证。观感复用 #70 设置弹窗基线:窗底显式取
//! 当前主题 shell 色、两列行走 `settings::settings_row`(标签列定宽)、
//! 间距用外壳 token,明暗主题各自成立。
//!
//! M2「检查更新」(#71):按钮态机 空闲 → 检查中(禁用防连点)→ 最新 /
//! 有更新(新版本号 +「前往下载」`open_url` 到 releases 页)/ 无法判断
//! (tag 非 semver 或响应缺 tag)/ 失败(一句话,可重试)。HTTP 在 std
//! 后台线程(ureq GET releases/latest,**必须带 User-Agent** —— GitHub
//! API 拒绝无 UA 请求),结果经 mpsc 回 UI 走消息归约,UI 线程零阻塞
//! (照 `settings::ModelListState` 三原语先例);版本比较是纯函数
//! ([`compare_release_tag`]),不引 semver crate。
//!
//! 关闭语义照既有浮窗:全窗蒙层点击关(快捷键蒙层同款 scrim)、标题栏 X
//! 关(egui `Window::open` 内建)、Esc 关(`ui::layout::reduce` 消费裸 Esc,
//! 润色确认浮窗同款)。三条出口都汇成 `crate::state::Message::AboutClosed`
//! 在归约落地 —— 本模块只绘制与发消息,不碰状态(铁律)。

use crate::repaint::RepaintNotifier;
use crate::state::Message;
use eframe::egui;
use std::cmp::Ordering;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// 仓库主页;链接点击经 egui `open_url` 打开。
pub const REPO_URL: &str = "https://github.com/ailater/LaterMd";
/// 仓库链接的显示文案(裸域名形,不带协议头,与浏览器地址栏习惯一致)。
pub const REPO_LABEL: &str = "github.com/ailater/LaterMd";

/// releases 页(「前往下载」的落点):永远重定向到最新一个 release 的
/// 网页,与检查更新请求的 API 端点([`RELEASES_LATEST_URL`])同源不同层。
pub const RELEASES_URL: &str = "https://github.com/ailater/LaterMd/releases/latest";

/// 一句话定位(AGENTS.md §1 产品定义的首句)。
pub const TAGLINE: &str = "跨平台、版本化、可对话、可演化的 Markdown 知识工作台";

/// GitHub releases「最新一个」API 端点。
const RELEASES_LATEST_URL: &str = "https://api.github.com/repos/ailater/LaterMd/releases/latest";
/// 检查更新超时:一次元数据 GET,10s 只兜底死连接(比模型列表拉取的
/// 30s 紧 —— 用户就等在对话框前)。
const UPDATE_TIMEOUT_SECS: u64 = 10;

/// 关于对话框状态:开/关与检查更新态机全在归约(`Message::AboutOpened` /
/// `AboutClosed` / `AboutUpdateCheck*`),UI 只读。关窗不清在途与结果
/// (照 `ModelListState`:设置页关了拉取照常收流落地,重开可见)。
#[derive(Default)]
pub struct AboutState {
    /// 对话框是否可见。
    pub open: bool,
    /// 「检查更新」状态机(M2)。
    pub update: UpdateCheckState,
}

/// 检查更新的一次结果载荷:成功 = 最新 release 的 tag 原文(如
/// `v0.0.5`);请求成功但响应里取不到 `tag_name` 为 `Ok(None)`(归
/// 「无法判断」,不冒充失败);失败 = 面向用户的一句话错误文案。
pub type UpdateCheckResult = Result<Option<String>, String>;

/// 检查更新的线程装配点:生产 [`spawn_release_check`];测试注入假装配
/// (定时回固定结果,零网络),与 `settings::ModelsSpawner` 同款注入点。
pub(crate) type UpdateSpawner =
    Arc<dyn Fn(Sender<UpdateCheckResult>) -> Option<JoinHandle<()>> + Send + Sync>;

/// 检查更新的结果态。**「检查中」不在本枚举** —— 在途由
/// [`UpdateCheckState::is_checking`](即接收端在场:is_checking 的实现是
/// `rx.is_some()`)单一表达,结果只在收尾写入(与 `ModelListState` 的
/// error/options 同构,不设双份在途标志)。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UpdatePhase {
    /// 尚未检查(或正在检查中 —— 两者都无结果可示)。
    #[default]
    Idle,
    /// 最新发布不高于当前版本。
    UpToDate,
    /// 有更新:`tag` 是最新 release 的 tag 原文(显示用,含 v 前缀)。
    Available {
        /// 最新 release 的 tag(如 `v0.0.5`)。
        tag: String,
    },
    /// 无法判断:tag 非 semver(显示原文)或响应缺 `tag_name`。
    Unknown {
        /// 取到的 tag 原文;`None` = 响应里根本没有 `tag_name`。
        tag: Option<String>,
    },
    /// 失败(网络/HTTP/解析错误的一句话,可重试)。
    Failed {
        /// 面向用户的错误文案。
        error: String,
    },
}

/// 「检查更新」的状态机(#71 M2):在途接收端 + 结果态,与
/// `settings::ModelListState` 同款「发起 / 接收 / 收尾」三原语。发起与
/// 收尾都在归约;防重入靠 `start` 把关(在途忽略 + 接收端不中途替换,
/// 能到达归约的只有当前在途请求的结果);线程异常退出(channel 断连且
/// 未交出结果)由 [`UpdateCheckState::poll`] 补一条失败。
pub struct UpdateCheckState {
    /// 在途检查的接收端;`None` = 空闲。
    pub(crate) rx: Option<Receiver<UpdateCheckResult>>,
    /// 本次 channel 是否已交出过结果:断连兜底的判定依据(跨 poll 存续)。
    pub(crate) saw_result: bool,
    /// 最近一次检查的结果态;发起时清回 Idle。
    pub phase: UpdatePhase,
    /// 线程装配点(见 [`UpdateSpawner`])。
    pub(crate) spawner: UpdateSpawner,
}

impl Default for UpdateCheckState {
    fn default() -> Self {
        Self {
            rx: None,
            saw_result: false,
            phase: UpdatePhase::Idle,
            spawner: Arc::new(spawn_release_check),
        }
    }
}

impl UpdateCheckState {
    /// 是否有检查在途，供按钮禁用；完成帧由结果通知主动唤醒。
    pub fn is_checking(&self) -> bool {
        self.rx.is_some()
    }

    /// 发起(归约侧调用):清旧结果、换接收端并经装配点 spawn。在途时
    /// 忽略(UI 已禁用按钮,防御);spawn 失败当场按失败收尾,不留卡死
    /// 的「检查中」。
    #[cfg(test)]
    pub(crate) fn start(&mut self) {
        self.start_with_repaint(RepaintNotifier::default());
    }

    /// 发起版本检查并在网络结果转交给 UI channel 后主动唤醒一帧。检查
    /// 结果来自 GitHub Release，显示状态仍由消息归约统一更新。
    pub(crate) fn start_with_repaint(&mut self, repaint: RepaintNotifier) {
        if self.rx.is_some() {
            return;
        }
        self.phase = UpdatePhase::Idle;
        self.saw_result = false;
        let Ok((tx, rx)) = repaint.notifying_channel("latermd-update-repaint") else {
            self.phase = UpdatePhase::Failed {
                error: "检查更新失败:无法启动通知线程".to_owned(),
            };
            return;
        };
        self.rx = Some(rx);
        if (self.spawner)(tx).is_none() {
            self.rx = None;
            self.phase = UpdatePhase::Failed {
                error: "检查更新失败:无法启动后台线程".to_owned(),
            };
        }
    }

    /// 非阻塞收空 channel(每帧归约调用),结果翻成
    /// [`Message::AboutUpdateCheckFinished`];断连却没等到结果(线程异常
    /// 退出)补一条失败 —— 「检查中」绝不能卡死,否则按钮永久禁用。
    pub(crate) fn poll(&mut self) -> Vec<Message> {
        let Some(rx) = &self.rx else {
            return Vec::new();
        };
        let mut messages = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(result) => {
                    self.saw_result = true;
                    messages.push(Message::AboutUpdateCheckFinished { result });
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.saw_result {
                        messages.push(Message::AboutUpdateCheckFinished {
                            result: Err("检查更新失败:后台线程意外中断".to_owned()),
                        });
                    }
                    break;
                }
            }
        }
        messages
    }

    /// 收尾(`Message::AboutUpdateCheckFinished` 的归约):清在途状态,
    /// 成功按 [`compare_release_tag`] 归入最新/有更新/无法判断(tag 非
    /// semver 或响应缺 `tag_name` 都是无法判断,不冒充失败),失败落
    /// 一句话错误。
    pub(crate) fn finish(&mut self, result: UpdateCheckResult) {
        self.rx = None;
        self.saw_result = false;
        self.phase = match result {
            Ok(Some(tag)) => match compare_release_tag(&tag, env!("CARGO_PKG_VERSION")) {
                Some(Ordering::Greater) => UpdatePhase::Available { tag },
                Some(Ordering::Less | Ordering::Equal) => UpdatePhase::UpToDate,
                None => UpdatePhase::Unknown { tag: Some(tag) },
            },
            Ok(None) => UpdatePhase::Unknown { tag: None },
            Err(error) => UpdatePhase::Failed { error },
        };
    }
}

/// 比较 release tag 与当前版本:剥 `v`/`V` 前缀、截断 `-rc.1`/`+build`
/// 等后缀后按 `major.minor.patch` 三段数字比较;任一侧解析不出三段
/// 数字返回 `None`(调用方归「无法判断」)。不引 semver crate(发布
/// 渠道的 tag 就是纯三段,预发布段不参与排序)。两侧对称剥前缀 ——
/// 纯函数不假设调用方形态(当前版本常量不带 v,但不写死这个前提)。
pub fn compare_release_tag(tag: &str, current: &str) -> Option<Ordering> {
    Some(parse_version_triple(strip_v(tag))?.cmp(&parse_version_triple(strip_v(current))?))
}

/// trim + 剥 `v`/`V` 前缀。
fn strip_v(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix(['v', 'V']).unwrap_or(s)
}

/// `major.minor.patch` 三段 u64;段数不足/超出、任一段非数字都拒绝。
fn parse_version_triple(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    (parts.next().is_none()).then_some((major, minor, patch))
}

/// 解析 `releases/latest` 响应体取 `tag_name`(trim 后非空;结构不对 /
/// 缺字段 / 非字符串都返回 `None`,由调用方归「无法判断」或错误文案)。
pub fn parse_release_tag(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let tag = value.get("tag_name")?.as_str()?.trim();
    (!tag.is_empty()).then(|| tag.to_owned())
}

/// 生产装配:spawn std 线程 GET [`RELEASES_LATEST_URL`],结果经 `tx`
/// 回传,线程句柄交还调用方(测试 join;生产丢弃 —— 接收端被新请求替换
/// 后发送失败即静默收尾)。真机验证留人工,无头测试只走假装配。
pub(crate) fn spawn_release_check(tx: Sender<UpdateCheckResult>) -> Option<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("latermd-update-check".into())
        .spawn(move || {
            let _ = tx.send(run_release_check());
        })
        .ok()
}

/// 后台线程的一次完整检查:请求 → 状态码 → 解析。错误收敛为面向用户的
/// 一句话(不带响应体 —— GitHub 的错误 JSON 很长,对用户无信息量)。
fn run_release_check() -> UpdateCheckResult {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(UPDATE_TIMEOUT_SECS)))
        .timeout_recv_response(Some(Duration::from_secs(UPDATE_TIMEOUT_SECS)))
        // 整个 body 的总量预算:releases JSON 约几 KB,同量级兜底即可,
        // 别让死连接永久挂住后台线程
        .timeout_recv_body(Some(Duration::from_secs(UPDATE_TIMEOUT_SECS)))
        .http_status_as_error(false)
        .build()
        .into();
    let response = agent
        .get(RELEASES_LATEST_URL)
        // GitHub API 拒绝无 User-Agent 的请求(403),必须带
        .header("User-Agent", concat!("LaterMD/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|error| format!("检查更新失败:{error}"))?;
    if !response.status().is_success() {
        return Err(format!("检查更新失败:HTTP {}", response.status().as_u16()));
    }
    let body = response
        .into_body()
        .read_to_string()
        .map_err(|error| format!("检查更新失败:{error}"))?;
    // 缺 `tag_name` / 结构不对都归 Ok(None) →「无法判断」,只有网络与
    // HTTP 层才算失败
    Ok(parse_release_tag(&body))
}

/// 对话框最小宽:关于窗是只读速览卡,一行定位句 + 两列行在此宽内单行
/// 放下;非 resizable,实际宽 = max(此值, 内容宽)。
const ABOUT_MIN_W: f32 = 420.0;

/// 画关于窗,返回**是否请求关闭**(蒙层点击或标题栏 X)。调用方
/// (`ui::layout::draw_overlay_dialogs`)把它翻成
/// `crate::state::Message::AboutClosed` 发归约;「检查更新」按钮点击翻成
/// `Message::AboutUpdateCheckRequested` 进 `outbox`。本函数不改任何状态。
pub fn dialog(ui: &mut egui::Ui, about: &AboutState, outbox: &mut Vec<Message>) -> bool {
    let ctx = ui.ctx().clone();
    let screen = ctx.viewport_rect();

    // 全窗蒙层(快捷键蒙层同款手法与色值):盖住下层、点击即关。先画,
    // 窗卡片后画盖在其上 —— 点卡片不误关,点卡片外的蒙层底才关。
    let mut scrim_clicked = false;
    egui::Area::new(egui::Id::new("about-scrim"))
        .interactable(true)
        .fixed_pos(egui::Pos2::ZERO)
        .show(&ctx, |ui| {
            let dark = ui.visuals().dark_mode;
            let tint = if dark {
                egui::Color32::from_black_alpha(crate::shortcut_overlay::SCRIM_ALPHA_DARK)
            } else {
                egui::Color32::from_white_alpha(crate::shortcut_overlay::SCRIM_ALPHA_LIGHT)
            };
            ui.painter()
                .rect_filled(screen, egui::CornerRadius::ZERO, tint);
            if ui.allocate_rect(screen, egui::Sense::click()).clicked() {
                scrim_clicked = true;
            }
        });

    let mut open = true;
    // 窗底、边界和内距统一走工作台浮窗卡片样式;初始位置走
    // centered_dialog(显式 id 让拖动位置记忆与文案解耦)。
    crate::ui::workbench::centered_dialog(ui, "关于 LaterMD")
        // 显式 id(设置窗同款):窗口拖动位置记忆与文案解耦
        .id(egui::Id::new("about-dialog"))
        .min_width(ABOUT_MIN_W)
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .frame(crate::ui::workbench::dialog_frame(ui))
        .show(&ctx, |ui| {
            ui.add_space(crate::ui::tokens::SPACE_SM);
            // 应用名 + 版本居中成组(关于窗惯例:身份信息居中,数据行两列)
            ui.vertical_centered(|ui| {
                ui.heading("LaterMD");
                ui.label(format!("版本 {}", env!("CARGO_PKG_VERSION")));
            });
            ui.add_space(crate::ui::tokens::SPACE_SM);
            ui.label(TAGLINE);
            ui.add_space(crate::ui::tokens::SPACE_MD);
            ui.separator();
            ui.add_space(crate::ui::tokens::SPACE_SM);
            // 两列行复用设置页基线:标签列定宽,控件列同起点
            crate::settings::settings_row(ui, "仓库", |ui| {
                ui.hyperlink_to(REPO_LABEL, REPO_URL);
            });
            crate::settings::settings_row(ui, "许可协议", |ui| {
                ui.label("MIT License");
            });
            ui.add_space(crate::ui::tokens::SPACE_SM);
            ui.separator();
            ui.add_space(crate::ui::tokens::SPACE_SM);
            // 检查更新(#71 M2):按钮态机 + 结果行。在途禁用防连点
            // (模型列表按钮同款禁用 + 悬停说明),失败后按钮变「重试」。
            let checking = about.update.is_checking();
            let retry = matches!(about.update.phase, UpdatePhase::Failed { .. });
            crate::settings::settings_row(ui, "检查更新", |ui| {
                let label = if checking {
                    "检查中…"
                } else if retry {
                    "重试"
                } else {
                    "检查更新"
                };
                let button = ui.add_enabled(!checking, egui::Button::new(label));
                let button = if checking {
                    button.on_disabled_hover_text("正在检查更新…")
                } else {
                    button
                };
                if !checking && button.clicked() {
                    outbox.push(Message::AboutUpdateCheckRequested);
                }
            });
            match &about.update.phase {
                // Idle 也覆盖「正在检查中」:在途时无结果可示
                UpdatePhase::Idle => {}
                UpdatePhase::UpToDate => {
                    ui.weak(format!("已是最新版本(v{})", env!("CARGO_PKG_VERSION")));
                }
                UpdatePhase::Available { tag } => {
                    ui.horizontal(|ui| {
                        ui.label(format!("有新版本 {tag}"));
                        ui.hyperlink_to("前往下载", RELEASES_URL);
                    });
                }
                UpdatePhase::Unknown { tag } => {
                    if let Some(tag) = tag {
                        ui.weak(format!(
                            "无法判断是否需要更新(最新发布 {tag} 不是语义化版本号)"
                        ));
                    } else {
                        ui.weak("无法判断是否需要更新(最新发布信息里没有版本号)");
                    }
                }
                UpdatePhase::Failed { error } => {
                    ui.colored_label(crate::ui::tokens::WARN, error);
                }
            }
            ui.add_space(crate::ui::tokens::SPACE_SM);
        });
    scrim_clicked || !open
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Message, State};
    use egui::{Event, PointerButton, RawInput};
    use std::cell::Cell;
    use std::sync::mpsc;

    /// 一帧的可见文本(shapes 的 galley 原文拼接;galley.job.text 恒是
    /// 完整原文,截断不影响断言)。egui 0.36 的 `Window` 首帧只注册
    /// Area、次帧才画内容(无头实测 shapes 首帧无文本),取证一律取
    /// 预热后的帧。
    fn dialog_frame_text(ctx: &egui::Context, about: &AboutState, events: Vec<Event>) -> String {
        let mut close = false;
        let mut outbox = Vec::new();
        let output = ctx.run_ui(
            RawInput {
                events,
                ..Default::default()
            },
            |ui| close = dialog(ui, about, &mut outbox),
        );
        let _ = close;
        let text = output
            .shapes
            .iter()
            .filter_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                Some(text.galley.job.text.clone())
            })
            .collect::<Vec<_>>()
            .join("\n");
        output.drop_without_applying_deltas();
        text
    }

    /// 五件内容 + 检查更新按钮全部渲染:应用名、版本号、一句话定位、
    /// 仓库链接、MIT 许可证、「检查更新」。版本号断言用编译期注入的
    /// `CARGO_PKG_VERSION` —— 渲染侧漏画版本行(或写死成别的)时这里
    /// 红,非恒真。
    #[test]
    fn renders_name_version_tagline_repo_link_and_license() {
        for dark in [true, false] {
            let ctx = egui::Context::default();
            ctx.set_theme(if dark {
                egui::Theme::Dark
            } else {
                egui::Theme::Light
            });
            let about = AboutState::default();
            // 首帧预热(Area 注册),次帧起有内容
            let _ = dialog_frame_text(&ctx, &about, Vec::new());
            let text = dialog_frame_text(&ctx, &about, Vec::new());
            assert!(text.contains("LaterMD"), "应用名在场(实际:{text:?})");
            assert!(
                text.contains(env!("CARGO_PKG_VERSION")),
                "版本号 {} 在场(实际:{text:?})",
                env!("CARGO_PKG_VERSION")
            );
            assert!(text.contains(TAGLINE), "一句话定位在场(实际:{text:?})");
            assert!(text.contains(REPO_LABEL), "仓库链接文案在场(实际:{text:?})");
            assert!(text.contains("MIT"), "MIT 许可证在场(实际:{text:?})");
            assert!(text.contains("检查更新"), "检查更新按钮在场(实际:{text:?})");
            // 纯渲染不请求关闭(run_ui 交回 FullOutput,布尔经 Cell 带出;
            // 输出必须显式消费,字体纹理 delta 直接丢弃会 panic)
            let close = Cell::new(false);
            let mut outbox = Vec::new();
            let output = ctx.run_ui(RawInput::default(), |ui| {
                close.set(dialog(ui, &about, &mut outbox));
            });
            output.drop_without_applying_deltas();
            assert!(!close.get(), "纯渲染不请求关闭({dark:?} 主题)");
            assert!(outbox.is_empty(), "纯渲染不发消息");
        }
    }

    /// 仓库链接点击走系统浏览器:egui `Hyperlink` 内建 `open_url`,断言
    /// 从 platform_output 的 `OpenUrl` 命令取证(live.rs 复制按钮同款)。
    #[test]
    fn repo_link_click_opens_url() {
        let ctx = egui::Context::default();
        let about = AboutState::default();
        // 首帧预热(窗 Area 注册),次帧定位链接文本矩形
        let _ = dialog_frame_text(&ctx, &about, Vec::new());
        let output = ctx.run_ui(RawInput::default(), |ui| {
            let mut outbox = Vec::new();
            dialog(ui, &about, &mut outbox);
        });
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        let rect = shapes
            .iter()
            .find_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                text.galley
                    .job
                    .text
                    .contains(REPO_LABEL)
                    .then(|| clipped.shape.visual_bounding_rect())
            })
            .expect("链接文本已渲染");

        let center = rect.center();
        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let output = ctx.run_ui(
            RawInput {
                events: vec![Event::PointerMoved(center), click(true), click(false)],
                ..Default::default()
            },
            |ui| {
                let mut outbox = Vec::new();
                dialog(ui, &about, &mut outbox);
            },
        );
        let urls: Vec<String> = output
            .platform_output
            .commands
            .iter()
            .filter_map(|cmd| match cmd {
                egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        assert_eq!(
            urls,
            vec![REPO_URL.to_owned()],
            "点击仓库链接应经 open_url 打开 {REPO_URL}"
        );
    }

    /// 蒙层点击请求关闭:点窗卡片外的蒙层底(屏幕角落)→ `dialog` 返回
    /// true;不点击的帧返回 false。X 按钮是 egui `Window::open` 内建
    /// widget(无公开矩形探针,设置窗先例同样未无头点击),其关闭与蒙层
    /// 共用同一返回通道。
    #[test]
    fn scrim_click_requests_close() {
        let ctx = egui::Context::default();
        let about = AboutState::default();
        // 首帧预热(Area 注册),空帧:不关
        let close = Cell::new(false);
        let mut outbox = Vec::new();
        let output = ctx.run_ui(RawInput::default(), |ui| {
            close.set(dialog(ui, &about, &mut outbox))
        });
        output.drop_without_applying_deltas();
        assert!(!close.get(), "无交互不请求关闭");
        let close = Cell::new(false);
        let output = ctx.run_ui(RawInput::default(), |ui| {
            close.set(dialog(ui, &about, &mut Vec::new()))
        });
        output.drop_without_applying_deltas();
        assert!(!close.get(), "预热后的空帧同样不请求关闭");

        // 点屏幕角落(窗卡片之外):请求关闭
        let corner = ctx.viewport_rect().right_bottom() - egui::vec2(4.0, 4.0);
        let click = |pressed| Event::PointerButton {
            pos: corner,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let close = Cell::new(false);
        let output = ctx.run_ui(
            RawInput {
                events: vec![Event::PointerMoved(corner), click(true), click(false)],
                ..Default::default()
            },
            |ui| close.set(dialog(ui, &about, &mut Vec::new())),
        );
        output.drop_without_applying_deltas();
        assert!(close.get(), "点击蒙层底应请求关闭");
    }

    /// 开/关归约:两条消息只翻 `about.open`,别的状态不动。
    #[test]
    fn opened_closed_messages_flip_about_state() {
        let mut state = State::default();
        assert!(!state.about.open, "出厂关");
        state.apply(Message::AboutOpened);
        assert!(state.about.open, "AboutOpened 归约打开");
        state.apply(Message::AboutClosed);
        assert!(!state.about.open, "AboutClosed 归约关闭");
        // 幂等方向:关着再关、开着再开不炸
        state.apply(Message::AboutClosed);
        state.apply(Message::AboutOpened);
        assert!(state.about.open);
    }

    // ---- #71 M2:版本比较与 tag 解析(纯函数,零网络) ----

    /// tag 剥 v/V 前缀按三段数字比较:newer/older/equal 三向齐全;数字
    /// 按数值比(1.10 > 1.9,不按字典序);两侧都可以带 v 前缀与首尾
    /// 空白;`-rc`/`+build` 后缀截断不参与。
    #[test]
    fn compare_release_tag_orders_versions() {
        use Ordering::*;
        // newer
        assert_eq!(compare_release_tag("v0.0.5", "0.0.4"), Some(Greater));
        assert_eq!(compare_release_tag("1.10.0", "1.9.0"), Some(Greater));
        assert_eq!(compare_release_tag("v2.0.0", "v1.99.99"), Some(Greater));
        assert_eq!(compare_release_tag(" v0.1.0 ", "0.0.4"), Some(Greater));
        assert_eq!(compare_release_tag("V0.0.5", "0.0.4"), Some(Greater));
        assert_eq!(compare_release_tag("0.0.5", "V0.0.4"), Some(Greater));
        // 预发布/构建后缀截断:rc 也是该主版本
        assert_eq!(compare_release_tag("v0.0.5-rc.1", "0.0.4"), Some(Greater));
        assert_eq!(compare_release_tag("v0.0.5-rc.1", "0.0.5"), Some(Equal));
        assert_eq!(compare_release_tag("0.0.4+build.2", "0.0.4"), Some(Equal));
        // older / equal
        assert_eq!(compare_release_tag("v0.0.3", "0.0.4"), Some(Less));
        assert_eq!(compare_release_tag("v0.0.4", "0.0.4"), Some(Equal));
        assert_eq!(compare_release_tag("10.0.0", "9.0.0"), Some(Greater));
    }

    /// 畸形 tag(非 semver)一律 None → 调用方归「无法判断」:
    /// 非数字段、段数不足或超出、空串、纯前缀。
    #[test]
    fn compare_release_tag_rejects_malformed() {
        for tag in [
            "latest",
            "v1.2",
            "1.2",
            "v1.2.3.4",
            "",
            "  ",
            "v",
            "v0.0",
            "vX.Y.Z",
            "v0.0.x",
            "v-1.2.3",
            "release-1.2.3",
        ] {
            assert_eq!(
                compare_release_tag(tag, "0.0.4"),
                None,
                "畸形 tag {tag:?} 应判 None"
            );
        }
        // 当前版本一侧同样参与校验(防御:CARGO_PKG_VERSION 恒合法)
        assert_eq!(compare_release_tag("v0.0.5", "oops"), None);
    }

    /// releases/latest 响应解析:真实形态(带 url/assets 等无关字段)取
    /// `tag_name` 并 trim;缺字段/非字符串/空白/坏 JSON/非对象根都
    /// 返回 None。
    #[test]
    fn parse_release_tag_extracts_tag_name() {
        let sample = r#"{
            "url": "https://api.github.com/repos/ailater/LaterMd/releases/1",
            "html_url": "https://github.com/ailater/LaterMd/releases/tag/v0.0.5",
            "tag_name": " v0.0.5 ",
            "name": "LaterMD 0.0.5",
            "draft": false,
            "prerelease": false,
            "assets": []
        }"#;
        assert_eq!(parse_release_tag(sample), Some("v0.0.5".to_owned()));
        assert_eq!(
            parse_release_tag(r#"{"tag_name":"1.2.3"}"#),
            Some("1.2.3".to_owned())
        );
        assert_eq!(parse_release_tag(r#"{"name":"no tag here"}"#), None);
        assert_eq!(parse_release_tag(r#"{"tag_name":42}"#), None);
        assert_eq!(parse_release_tag(r#"{"tag_name":"   "}"#), None);
        assert_eq!(parse_release_tag("not json at all"), None);
        assert_eq!(parse_release_tag(r#"["v0.0.5"]"#), None);
    }

    // ---- #71 M2:态机(注入假装配,零网络) ----

    /// 定时回固定结果的假装配:spawn 真线程(验线程机制)但绝不碰网络
    /// (settings.rs 模型列表测试同款)。
    fn fake_spawner(delay: std::time::Duration, result: UpdateCheckResult) -> UpdateSpawner {
        Arc::new(move |tx: mpsc::Sender<UpdateCheckResult>| {
            let result = result.clone();
            std::thread::Builder::new()
                .name("fake-update-check".into())
                .spawn(move || {
                    std::thread::sleep(delay);
                    let _ = tx.send(result);
                })
                .ok()
        })
    }

    /// 轮询到第一条结果消息(线程异步,限时;照 settings.rs `wait_result`
    /// 同款 —— poll 只翻消息不清在途,收尾是 `finish` 的事)。
    fn wait_result(update: &mut UpdateCheckState) -> Vec<Message> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut messages = Vec::new();
        while messages.is_empty() && std::time::Instant::now() < deadline {
            messages = update.poll();
            if messages.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        messages
    }

    /// 发起 → 在途防重入 → 收流 → 收尾的全链:结果落 `phase`,收流产出
    /// `AboutUpdateCheckFinished`,空闲后按钮回可用。
    #[test]
    fn update_check_start_poll_finish_flow() {
        let mut update = UpdateCheckState {
            spawner: fake_spawner(std::time::Duration::ZERO, Ok(Some("v9.9.9".to_owned()))),
            ..UpdateCheckState::default()
        };
        update.start();
        assert!(update.is_checking(), "发起后置在途");
        assert_eq!(update.phase, UpdatePhase::Idle, "发起清回 Idle");

        // 在途忽略再发起(防重入)
        update.start();
        assert!(update.is_checking(), "在途不换接收端");

        let messages = wait_result(&mut update);
        assert_eq!(
            messages,
            vec![Message::AboutUpdateCheckFinished {
                result: Ok(Some("v9.9.9".to_owned()))
            }],
            "收流产出收尾消息"
        );
        let Message::AboutUpdateCheckFinished { result } = messages.into_iter().next().unwrap()
        else {
            unreachable!("上文已断言唯一消息是收尾消息");
        };
        update.finish(result);
        assert_eq!(
            update.phase,
            UpdatePhase::Available {
                tag: "v9.9.9".to_owned()
            },
            "远端更高 → 有更新"
        );
        assert!(!update.is_checking(), "收尾清在途");
    }

    /// 收尾四分支:等版本/最新、tag 非 semver → 无法判断、缺 tag →
    /// 无法判断、失败 → 一句话错误。
    #[test]
    fn update_check_finish_phases() {
        let cases: Vec<(UpdateCheckResult, UpdatePhase)> = vec![
            (
                Ok(Some(format!("v{}", env!("CARGO_PKG_VERSION")))),
                UpdatePhase::UpToDate,
            ),
            (Ok(Some("v0.0.1".to_owned())), UpdatePhase::UpToDate),
            (
                Ok(Some("nightly".to_owned())),
                UpdatePhase::Unknown {
                    tag: Some("nightly".to_owned()),
                },
            ),
            (Ok(None), UpdatePhase::Unknown { tag: None }),
            (
                Err("检查更新失败:HTTP 403".to_owned()),
                UpdatePhase::Failed {
                    error: "检查更新失败:HTTP 403".to_owned(),
                },
            ),
        ];
        for (result, want) in cases {
            let mut update = UpdateCheckState::default();
            update.finish(result);
            assert_eq!(update.phase, want, "收尾分支 {want:?}");
            assert!(!update.is_checking(), "收尾清在途");
        }
    }

    /// 断连兜底:线程 spawn 后直接丢弃发送端(没交出结果),poll 补一条
    /// 失败 —— 「检查中」绝不能卡死。
    #[test]
    fn update_check_disconnect_falls_back_to_failure() {
        let mut update = UpdateCheckState {
            spawner: Arc::new(|_tx: mpsc::Sender<UpdateCheckResult>| {
                // 线程里什么都不发,channel 随线程退出断连
                std::thread::Builder::new()
                    .name("fake-update-silent".into())
                    .spawn(|| {})
                    .ok()
            }),
            ..UpdateCheckState::default()
        };
        update.start();
        // poll 只翻消息不清在途(清在途是 finish 的事),这里只取消息
        let messages = wait_result(&mut update);
        assert_eq!(
            messages,
            vec![Message::AboutUpdateCheckFinished {
                result: Err("检查更新失败:后台线程意外中断".to_owned())
            }],
            "断连未交结果应补失败"
        );
    }

    /// spawn 失败当场按失败收尾,不留卡死的「检查中」。
    #[test]
    fn update_check_spawn_failure_lands_error() {
        let mut update = UpdateCheckState {
            spawner: Arc::new(|_tx| None),
            ..UpdateCheckState::default()
        };
        update.start();
        assert!(!update.is_checking(), "spawn 失败不留在途");
        assert_eq!(
            update.phase,
            UpdatePhase::Failed {
                error: "检查更新失败:无法启动后台线程".to_owned()
            }
        );
    }

    /// 归约接线:`AboutUpdateCheckRequested` 发起(用假装配,零网络),
    /// 在途再请求被忽略;`AboutUpdateCheckFinished` 收尾落 phase。
    #[test]
    fn update_check_messages_reduce() {
        let mut state = State::default();
        state.about.update.spawner =
            fake_spawner(std::time::Duration::ZERO, Ok(Some("v0.0.1".to_owned())));
        state.apply(Message::AboutUpdateCheckRequested);
        assert!(state.about.update.is_checking(), "归约发起检查");
        // 在途防重入:第二次请求直接忽略
        state.apply(Message::AboutUpdateCheckRequested);
        assert!(state.about.update.is_checking());

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut messages = Vec::new();
        while messages.is_empty() && std::time::Instant::now() < deadline {
            messages = state.poll_about();
            if messages.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert!(!messages.is_empty(), "限时内收到结果消息");
        for message in messages {
            state.apply(message);
        }
        assert!(
            !state.about.update.is_checking(),
            "收尾消息经归约 finish 清在途"
        );
        assert_eq!(
            state.about.update.phase,
            UpdatePhase::UpToDate,
            "0.0.1 不高于当前 → 已是最新"
        );
    }

    // ---- #71 M2:UI 态机渲染 ----

    /// 各 phase 的可见文本与按钮文案:检查中禁用按钮且点击不发消息;
    /// 有更新显示新版本号 + 前往下载(点击经 open_url 打开 releases 页);
    /// 失败显示错误一句话 + 按钮变「重试」;已是最新有对应文案。
    #[test]
    fn update_ui_follows_phase() {
        let ctx = egui::Context::default();
        // 预热帧(窗 Area 注册)
        let _ = dialog_frame_text(&ctx, &AboutState::default(), Vec::new());

        // a) 有更新:tag 与「前往下载」在场,点击经 OpenUrl 打开 releases 页
        let available = AboutState {
            update: UpdateCheckState {
                phase: UpdatePhase::Available {
                    tag: "v9.9.9".to_owned(),
                },
                ..UpdateCheckState::default()
            },
            ..AboutState::default()
        };
        let text = dialog_frame_text(&ctx, &available, Vec::new());
        assert!(text.contains("v9.9.9"), "新版本号在场(实际:{text:?})");
        assert!(text.contains("前往下载"), "前往下载在场(实际:{text:?})");
        let output = ctx.run_ui(RawInput::default(), |ui| {
            let mut outbox = Vec::new();
            dialog(ui, &available, &mut outbox);
        });
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        let rect = shapes
            .iter()
            .find_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                text.galley
                    .job
                    .text
                    .contains("前往下载")
                    .then(|| clipped.shape.visual_bounding_rect())
            })
            .expect("「前往下载」已渲染");
        let center = rect.center();
        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let output = ctx.run_ui(
            RawInput {
                events: vec![Event::PointerMoved(center), click(true), click(false)],
                ..Default::default()
            },
            |ui| {
                let mut outbox = Vec::new();
                dialog(ui, &available, &mut outbox);
            },
        );
        let urls: Vec<String> = output
            .platform_output
            .commands
            .iter()
            .filter_map(|cmd| match cmd {
                egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        assert_eq!(
            urls,
            vec![RELEASES_URL.to_owned()],
            "前往下载打开 releases 页"
        );

        // b) 检查中:按钮禁用,点按钮位置不发消息
        let mut checking = AboutState::default();
        checking.update.spawner = fake_spawner(std::time::Duration::from_secs(30), Ok(None));
        checking.update.start();
        let text = dialog_frame_text(&ctx, &checking, Vec::new());
        assert!(text.contains("检查中…"), "按钮文案切检查中(实际:{text:?})");
        let mut outbox = Vec::new();
        let output = ctx.run_ui(RawInput::default(), |ui| {
            dialog(ui, &checking, &mut outbox);
        });
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        let rect = shapes
            .iter()
            .find_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                text.galley
                    .job
                    .text
                    .contains("检查中…")
                    .then(|| clipped.shape.visual_bounding_rect())
            })
            .expect("「检查中…」按钮已渲染");
        let center = rect.center();
        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let output = ctx.run_ui(
            RawInput {
                events: vec![Event::PointerMoved(center), click(true), click(false)],
                ..Default::default()
            },
            |ui| {
                let mut outbox = Vec::new();
                dialog(ui, &checking, &mut outbox);
            },
        );
        output.drop_without_applying_deltas();
        assert!(
            outbox.is_empty(),
            "禁用按钮点击不发消息(防连点,实际:{outbox:?})"
        );

        // c) 失败:错误一句话在场,按钮变「重试」
        let failed = AboutState {
            update: UpdateCheckState {
                phase: UpdatePhase::Failed {
                    error: "检查更新失败:HTTP 403".to_owned(),
                },
                ..UpdateCheckState::default()
            },
            ..AboutState::default()
        };
        let text = dialog_frame_text(&ctx, &failed, Vec::new());
        assert!(text.contains("HTTP 403"), "错误文案在场(实际:{text:?})");
        assert!(text.contains("重试"), "失败后按钮变重试(实际:{text:?})");

        // d) 已是最新 / 无法判断
        let up_to_date = AboutState {
            update: UpdateCheckState {
                phase: UpdatePhase::UpToDate,
                ..UpdateCheckState::default()
            },
            ..AboutState::default()
        };
        let text = dialog_frame_text(&ctx, &up_to_date, Vec::new());
        assert!(text.contains("已是最新版本"), "已是最新文案(实际:{text:?})");
        let unknown = AboutState {
            update: UpdateCheckState {
                phase: UpdatePhase::Unknown {
                    tag: Some("nightly".to_owned()),
                },
                ..UpdateCheckState::default()
            },
            ..AboutState::default()
        };
        let text = dialog_frame_text(&ctx, &unknown, Vec::new());
        assert!(
            text.contains("无法判断") && text.contains("nightly"),
            "无法判断带 tag 原文(实际:{text:?})"
        );
    }

    /// 空闲态点「检查更新」按钮:发出 `AboutUpdateCheckRequested`(消息
    /// 层发起,UI 不直接起线程)。定位按钮矩形取**最后一个**含该文本的
    /// Text shape —— 行标签「检查更新」与按钮文本同名,绘制顺序标签
    /// 在前、按钮在后,取末位才落在按钮上。
    #[test]
    fn update_button_click_sends_requested_message() {
        let ctx = egui::Context::default();
        let about = AboutState::default();
        // 预热帧(窗 Area 注册),次帧定位按钮
        let _ = dialog_frame_text(&ctx, &about, Vec::new());
        let output = ctx.run_ui(RawInput::default(), |ui| {
            let mut outbox = Vec::new();
            dialog(ui, &about, &mut outbox);
        });
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        let rect = shapes
            .iter()
            .rev()
            .find_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                text.galley
                    .job
                    .text
                    .contains("检查更新")
                    .then(|| clipped.shape.visual_bounding_rect())
            })
            .expect("「检查更新」按钮已渲染");
        let center = rect.center();
        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let mut outbox = Vec::new();
        let output = ctx.run_ui(
            RawInput {
                events: vec![Event::PointerMoved(center), click(true), click(false)],
                ..Default::default()
            },
            |ui| {
                dialog(ui, &about, &mut outbox);
            },
        );
        output.drop_without_applying_deltas();
        assert_eq!(
            outbox,
            vec![Message::AboutUpdateCheckRequested],
            "点击检查更新按钮发请求消息"
        );
    }
}
