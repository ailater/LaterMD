//! 设置对话框(docs/ui-polish.md §5)。
//!
//! 形态:左侧竖排分页 + 右侧内容区,单个 `egui::Window`。内容区按分区
//! 渲成卡片(2026-10-10 卡片点样:比窗底浅一档的圆角面板 + 强调色竖条标题,形态对齐
//! LaterScreen 设置面板的点样)。
//!
//! **为什么从工具栏的「设置」菜单升级成对话框**:菜单里只能塞几行
//! (主题列表 + 一个入口),而快捷键页有 10 行、AI 页有一串字段 —— 塞进
//! 菜单既画不下,也违背「菜单是对话框以外的轻量入口」这一通用范式。
//! 原「AI Provider」浮窗随之退役,其内容(`ai_key::key_editor`)嵌入 AI 页。
//!
//! 归约铁律照旧:本模块只绘制与产出 [`Message`];落盘、改状态、凭据读写
//! 全在 `State::apply`。两处**原地**例外(纯 UI 关注点):当前页签与
//! 「正在捕获哪个命令的键位」,同侧边栏把手与 `SearchState` 输入的口径。

use crate::ai::AiState;
use crate::ai_config::{AiConfig, ProviderKind, CONTEXT_KB_MAX};
use crate::ai_key::{self, AiKeyState};
use crate::command::Command;
use crate::keymap::Keymap;
use crate::mcp::McpState;
use crate::state::Message;
use crate::theme::{
    Density, SkinCatalog, ThemeMode, ThemeSettings, ZenNavMode, EDITOR_FONT_SIZE_MAX,
    EDITOR_FONT_SIZE_MIN, LINE_HEIGHT_MAX, LINE_HEIGHT_MIN,
};
use crate::ui::icons;
use eframe::egui;
use latermd_ai::{ModelsResult, ModelsSource};
use latermd_mcp::{McpConfig, ToolKind};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;

mod macos;
mod skins;

/// 设置页。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    /// 外观(主题 + 渲染后端只读)。
    #[default]
    Appearance,
    /// 快捷键(可改绑)。
    Keymap,
    /// AI provider、端点与模型。
    Ai,
    /// MCP server(规划态)。
    Mcp,
    /// 图片(图床 profile 列表 / 增删改 / 测试上传,docs/image-plan.md C 段)。
    Image,
}

impl SettingsTab {
    /// 左侧分页顺序。
    pub const ALL: [SettingsTab; 5] = [
        Self::Appearance,
        Self::Keymap,
        Self::Ai,
        Self::Mcp,
        Self::Image,
    ];

    /// 分页显示名。
    pub fn label(self) -> &'static str {
        match self {
            Self::Appearance => "外观",
            Self::Keymap => "快捷键",
            Self::Ai => "AI",
            Self::Mcp => "MCP",
            Self::Image => "图片",
        }
    }

    /// 分页图标(与工具栏同一套自绘图标)。
    pub fn icon(self) -> icons::Icon {
        match self {
            Self::Appearance => icons::Icon::Theme,
            Self::Keymap => icons::Icon::Reset,
            Self::Ai => icons::Icon::Ai,
            Self::Mcp => icons::Icon::Git,
            Self::Image => icons::Icon::Image,
        }
    }
}

/// 设置对话框状态。
pub struct SettingsState {
    /// 是否可见(工具栏齿轮/菜单入口翻开,窗口 X 关闭 —— 原地翻转)。
    pub open: bool,
    /// 当前分页。
    pub tab: SettingsTab,
    /// 正在捕获键位的命令;`Some` 时该行显示「按下新键位…」,归约侧
    /// (`ui::layout::reduce`)从本帧输入取键。
    pub capture: Option<Command>,
    /// 页内提示(撞键、不可绑定等);下一次操作或关页时清空。
    pub notice: Option<String>,
    /// AI 页的草稿:编辑原地发生,「保存」才发消息落盘。
    pub ai_draft: AiConfig,
    /// MCP 页的草稿:同理,「保存」才落盘并起停服务。
    pub mcp_draft: McpConfig,
    /// 外观页「导出皮肤」的名字输入框(纯 UI 关注点,导出动作走消息)。
    pub skin_export_name: String,
    /// 图片页的图床编辑草稿;`None` = 不在编辑。保存才发消息落盘。
    pub bed_draft: Option<BedDraft>,
    /// AI 页「获取模型列表」的状态机(在途接收端 + 候选列表 + 错误行)。
    pub models: ModelListState,
}

/// 图床页的编辑草稿:profile 本体 + 两个只属编辑期的字段(`headers` 的
/// 多行文本形态与 token 输入)。保存时合并成 [`Message::BedProfileSaved`]。
#[derive(Debug, Clone, PartialEq)]
pub struct BedDraft {
    /// 正在编辑的 profile(新增时来自预置模板,id 为空)。
    pub profile: latermd_bed::BedProfile,
    /// 请求头的多行文本形态,每行 `名: 值`(值可含 `${TOKEN}`)。
    pub headers_text: String,
    /// 新 token 输入;留空 = 不改已存凭据。已存值**不回显**(读都不读)。
    pub token: String,
}

impl BedDraft {
    /// 从既有 profile 建编辑草稿(「编辑」入口)。
    fn from_profile(profile: &latermd_bed::BedProfile) -> Self {
        Self {
            profile: profile.clone(),
            headers_text: headers_to_text(&profile.headers),
            token: String::new(),
        }
    }
}

/// 请求头对 → 多行文本(每行 `名: 值`)。
fn headers_to_text(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 多行文本 → 请求头对;空行跳过,缺 `:` 或名字为空的行报行号。
fn headers_from_text(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut headers = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(format!("第 {} 行请求头缺少「:」(格式:名: 值)", index + 1));
        };
        let name = name.trim();
        if name.is_empty() {
            return Err(format!("第 {} 行请求头名字为空", index + 1));
        }
        headers.push((name.to_owned(), value.trim().to_owned()));
    }
    Ok(headers)
}

/// 模型列表拉取的线程装配点:生产 [`latermd_ai::fetch_models`];测试注入
/// 假装配(定时回固定结果,零网络),与 `AiKeyState::creds` 同款注入点。
pub(crate) type ModelsSpawner =
    Arc<dyn Fn(ModelsSource, mpsc::Sender<ModelsResult>) -> Option<JoinHandle<()>> + Send + Sync>;

/// AI 页「获取模型列表」的状态机(#58 M2):在途接收端 + 候选列表 + 错误行。
///
/// 与 `crate::ai`(流式)和 `crate::bed::BedState`(上传)同款「发起 /
/// 接收 / 收尾」三原语,但只服务设置页,不碰任何文档。防重入口径随
/// `AiState`(流式):在途时 `start` 被忽略(UI 禁用按钮是第一道防线),
/// 接收端不中途替换,能到达归约的只有当前在途请求的结果,无需消息层
/// 序号。「获取中」绝不卡死:线程异常退出(channel 断连且未交出结果)
/// 由 [`ModelListState::poll`] 补一条失败。
pub struct ModelListState {
    /// 在途拉取的接收端;`None` = 空闲。
    pub(crate) rx: Option<Receiver<ModelsResult>>,
    /// 本次 channel 是否已交出过结果:断连兜底(线程 panic 没发出结果)
    /// 的判定依据(与 `BedState::saw_result` 同口径,跨 poll 存续)。
    pub(crate) saw_result: bool,
    /// 最近一次成功拉到的候选列表(保序去重);失败保留旧值,成功覆盖。
    pub options: Vec<String>,
    /// 最近一次失败的错误行文案;成功与再次发起时清空。
    pub error: Option<String>,
    /// 线程装配点(见 [`ModelsSpawner`])。
    pub(crate) spawner: ModelsSpawner,
}

impl Default for ModelListState {
    fn default() -> Self {
        Self {
            rx: None,
            saw_result: false,
            options: Vec::new(),
            error: None,
            spawner: Arc::new(latermd_ai::fetch_models),
        }
    }
}

impl ModelListState {
    /// 是否有拉取在途(驱动 UI 禁用按钮 + 持续重绘)。
    pub fn is_fetching(&self) -> bool {
        self.rx.is_some()
    }

    /// 发起(归约侧调用):替换接收端并经装配点 spawn。在途时忽略(UI 已
    /// 禁用按钮,防御);spawn 失败当场按失败收尾,不留卡死的「获取中」。
    /// 发起即清旧错误行 —— 用户已重新行动,旧行是过期信息。
    pub(crate) fn start(&mut self, source: ModelsSource) {
        if self.rx.is_some() {
            return;
        }
        self.error = None;
        self.saw_result = false;
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        if (self.spawner)(source, tx).is_none() {
            self.rx = None;
            self.error = Some("获取模型列表失败:无法启动后台线程".to_owned());
        }
    }

    /// 非阻塞收空 channel(每帧归约调用),结果翻成
    /// [`Message::AiModelsFetchFinished`];生命周期收口在归约侧的
    /// [`ModelListState::finish`](与 `AiState::poll` 同分工)。断连却没
    /// 等到结果(线程异常退出)补一条失败:「获取中」标志绝不能卡死,
    /// 否则按钮永久禁用。
    pub(crate) fn poll(&mut self) -> Vec<Message> {
        let Some(rx) = &self.rx else {
            return Vec::new();
        };
        let mut messages = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(result) => {
                    self.saw_result = true;
                    messages.push(Message::AiModelsFetchFinished { result });
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.saw_result {
                        messages.push(Message::AiModelsFetchFinished {
                            result: Err("获取模型列表失败:后台线程意外中断".to_owned()),
                        });
                    }
                    break;
                }
            }
        }
        messages
    }

    /// 收尾(`Message::AiModelsFetchFinished` 的归约):清在途状态并落地
    /// 结果 —— 成功覆盖候选列表并清错误行;失败只落错误行,旧候选保留
    /// (上次拉到的列表仍然可用,比清空更友好)。
    pub(crate) fn finish(&mut self, result: ModelsResult) {
        self.rx = None;
        self.saw_result = false;
        match result {
            Ok(list) => {
                self.options = list;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            open: false,
            tab: SettingsTab::Appearance,
            capture: None,
            notice: None,
            ai_draft: AiConfig::default(),
            mcp_draft: McpConfig::default(),
            skin_export_name: String::new(),
            bed_draft: None,
            models: ModelListState::default(),
        }
    }
}

/// #70 M2:设置页统一两列行 —— 标签列左对齐、定宽
/// ([`crate::ui::tokens::SETTINGS_LABEL_W`]),控件列同起点且跨分页一致。
///
/// 此前只有快捷键/图床编辑器两处 Grid,其余约 12 处 `ui.horizontal` 内联
/// 「标签 + 控件」,各行标签与控件起点随文字长短漂移。不用 `egui::Grid`:
/// Grid 列宽随内容自适应,五个分页各一张表只能在页内对齐、跨页漂移,
/// 多行控件(图床请求头)还会把同行标签顶到格顶。定宽标签列 +
/// `horizontal` 交叉居中让全部分页共用一条基准线,行高统一
/// `interact_size`,标签与输入框内文本同字号同垂直中心 → 基线对齐。
pub(crate) fn settings_row<R>(
    ui: &mut egui::Ui,
    label: &str,
    add_control: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let mut control = None;
    ui.horizontal(|ui| {
        let label = ui.add(egui::Label::new(label).truncate());
        // 标签列补齐到统一宽:egui 的 allocate_ui_* 家族推进父 cursor 用
        // 的是内容 min_rect 而非 desired 宽(ui.rs `scope_dyn` 实读),
        // 定宽分配会退化成「标签实际宽」。这里量出标签宽后用零高占位
        // 把「标签 + 余量」钉成 [`SETTINGS_LABEL_W`],控件列起点恒定
        let pad = (crate::ui::tokens::SETTINGS_LABEL_W - label.rect.width()).max(0.0);
        ui.allocate_exact_size(egui::vec2(pad, 0.0), egui::Sense::hover());
        control = Some(add_control(ui));
    });
    control.expect("horizontal 闭包必被调用")
}

/// 分区卡片(2026-10-10 点样):比窗底浅一档的圆角面板 + 细描边,标题行 = 强调色
/// 小竖条 + 加粗小字。卡片只管观感,不管行为 —— 闭包里照旧走
/// [`settings_row`] 两列与消息归约,任何一行都不会因进卡而改语义。
///
/// 填充/描边取 [`crate::theme::ShellTokens`] 的 `faint`/`border`(与
/// 窗底 `content` 同一套明暗投影):卡片与窗底的层次靠同一套 token 的
/// 明度差表达,不另立色相 —— design-system-plan §5.3 的口径。
fn card<R>(
    ui: &mut egui::Ui,
    shell: &crate::theme::ShellTokens,
    title: &str,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::NONE
        .fill(shell.faint)
        .stroke(egui::Stroke::new(1.0, shell.border.gamma_multiply(0.45)))
        .corner_radius(egui::CornerRadius::same(crate::ui::tokens::RADIUS_LG as u8))
        .inner_margin(egui::Margin::same(crate::ui::tokens::SETTINGS_CARD_PAD))
        .outer_margin(egui::Margin {
            bottom: crate::ui::tokens::SETTINGS_CARD_GAP,
            ..Default::default()
        })
        .show(ui, |ui| {
            // 撑满内容区宽:卡片是一整块分区面,不随内容缩窄
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(
                    crate::ui::tokens::SETTINGS_CARD_BAR,
                    egui::Sense::hover(),
                );
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::same(2),
                    crate::ui::tokens::accent(ui),
                );
                ui.strong(egui::RichText::new(title).size(crate::ui::tokens::FONT_SM));
            });
            ui.add_space(crate::ui::tokens::SPACE_XS);
            body(ui)
        })
        .inner
}

/// 主操作按钮(各页的「保存」):强调色实心底 + 反色文字,禁用态仍由
/// `enabled` 表达(egui 出厂灰化)。反色文字按明暗各取一档:浅色强调
/// #3370FF 配白字(对比约 4.6:1);暗色强调 #6C9FFF 偏亮,白字只有
/// 约 2:1,配 shell 最沉的 `sidebar` 反而读得清。
pub(crate) fn primary_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let ink = if ui.visuals().dark_mode {
        crate::theme::shell(ui).sidebar
    } else {
        egui::Color32::WHITE
    };
    let button = egui::Button::new(egui::RichText::new(text).color(ink).strong())
        .fill(crate::ui::tokens::accent(ui))
        .corner_radius(egui::CornerRadius::same(crate::ui::tokens::RADIUS_SM as u8));
    ui.add_enabled(enabled, button)
}

/// 绘制设置对话框;返回窗口关闭按钮的响应,**窗口未绘制(已关闭)时为
/// `None`**(测试据此断言「关闭后不再进入绘制路径」)。
// 参数各自属于 State 的不同字段,打包成结构会造出人为聚合(vendored 层
// 同款 allow 先例见 egui_markdown/src/label.rs)
#[allow(clippy::too_many_arguments)]
pub fn dialog(
    ui: &mut egui::Ui,
    settings: &mut SettingsState,
    theme: &ThemeSettings,
    skins: &SkinCatalog,
    system_theme_ok: bool,
    resolved: ThemeMode,
    keymap: &Keymap,
    ai: &AiState,
    ai_key: &mut AiKeyState,
    mcp: &McpState,
    bed: &mut crate::bed::BedState,
    outbox: &mut Vec<Message>,
) -> Option<egui::Response> {
    let mut open = settings.open;
    let mut close = None;
    // #70 M1:整窗显式随当前明暗主题取 shell 色(生产路径 `theme.apply`
    // 每帧已把 `window_fill` 投成同一值,这里再取一次是让「不依赖投影
    // 也在场」成为本窗自身性质,明暗各走各的 token,不写死任何一档)
    let shell = crate::theme::shell(ui);
    let mac = cfg!(target_os = "macos");
    let frame = if mac {
        egui::Frame::window(ui.style())
            .fill(shell.content)
            .inner_margin(0)
            .corner_radius(12)
            .stroke(crate::ui::workbench::separator(ui))
    } else {
        egui::Frame::window(ui.style()).fill(shell.content)
    };
    // 显式 id(find/goto 浮层同款):egui 0.36 按标题文本派生 Area id,
    // 标题一改拖动位置记忆就丢;显式 id 让窗口状态与文案解耦,测试也
    // 能按名取 Area 矩形。初始位置(pivot + default_pos)走 centered_dialog。
    crate::ui::workbench::centered_dialog(ui, "设置")
        .id(egui::Id::new("settings-dialog"))
        .default_size(crate::ui::tokens::SETTINGS_DEFAULT_SIZE)
        .min_size(if mac {
            egui::vec2(640.0, 420.0)
        } else {
            egui::Vec2::ZERO
        })
        .title_bar(!mac)
        .collapsible(false)
        .resizable(true)
        .open(&mut open)
        // 观感基线(#70 M1):窗底显式取当前主题的 content 色;圆角/阴影/
        // 窗口内边距仍走 egui 出厂 window 档(与 quick_open 等其余浮窗同源)
        .frame(frame)
        .show(ui.ctx(), |ui| {
            if mac {
                macos::style(ui);
                egui::Panel::top("settings-heading")
                    .exact_size(44.0)
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 8)))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(80.0, 28.0), egui::Sense::hover());
                            crate::ui::workbench::label_at(
                                ui,
                                rect.left(),
                                rect.center().y,
                                "设置",
                                14.0,
                                shell.text,
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if crate::ui::workbench::small_icon(
                                        ui,
                                        icons::Icon::Close,
                                        "关闭设置",
                                    )
                                    .clicked()
                                    {
                                        settings.open = false;
                                    }
                                },
                            );
                        });
                    });
            }
            // 骨架:底部按钮条 + 左分页列 + 中央滚动区,全部用 `exact_size`
            // 的 Panel 定形。此前用 `ui.horizontal` + `available_height()` +
            // `auto_shrink([false,false])` 的组合,ScrollArea 会请求全部可用
            // 宽,而 Window 宽又由内容决定 —— 二者互相喂,每帧把窗口撑大一
            // 圈,直到横贯全屏(2026-09-26 实测弹窗被拉成 1920x200 的扁条,
            // 分页列被挤没)。Panel 定形后各区域尺寸与内容解耦,反馈消失。
            // 尺寸/留白数字一律出自 ui::tokens 的设置弹窗观感基线(#70 M1)。
            egui::Panel::bottom("settings-footer")
                .exact_size(crate::ui::tokens::SETTINGS_FOOTER_H)
                .frame(
                    egui::Frame::default()
                        .fill(shell.content)
                        .corner_radius(if mac {
                            egui::CornerRadius {
                                sw: 12,
                                se: 12,
                                ..egui::CornerRadius::ZERO
                            }
                        } else {
                            egui::CornerRadius::ZERO
                        })
                        .inner_margin(crate::ui::tokens::SETTINGS_FOOTER_MARGIN),
                )
                .show(ui, |ui| {
                    if !mac {
                        ui.separator();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close = Some(ui.button("关闭"));
                        if mac && settings.tab == SettingsTab::Appearance {
                            ui.weak(egui::RichText::new("更改即时生效").size(11.0));
                        }
                        ui.add_space(crate::ui::tokens::SPACE_SM);
                        // 左端版本号:右到左布局里嵌一层左到右,吃掉余宽即
                        // 落到最左(2026-10-10 卡片点样,对齐 LaterScreen 的页脚信息位)
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.weak(concat!("LaterMD v", env!("CARGO_PKG_VERSION")));
                        });
                    });
                });
            // 左:竖排分页(固定宽;WorkBuddy 观感 = 分页列吃侧栏灰、内容区吃窗底)
            egui::Panel::left("settings-tabs")
                .exact_size(crate::ui::tokens::SETTINGS_TABS_W)
                .frame(
                    egui::Frame::default()
                        .fill(shell.sidebar)
                        .inner_margin(egui::Margin::same(crate::ui::tokens::SETTINGS_TABS_PAD)),
                )
                .show(ui, |ui| {
                    for tab in SettingsTab::ALL {
                        if (if mac {
                            macos::navigation(ui, tab, settings.tab == tab)
                        } else {
                            icons::icon_tab(ui, tab.icon(), tab.label(), settings.tab == tab)
                        })
                        .clicked()
                        {
                            settings.tab = tab;
                            // 换页清空上一页的残留提示与捕获状态
                            settings.notice = None;
                            settings.capture = None;
                        }
                    }
                });
            // 右:当前页内容(可滚动,长表单不被窗口裁掉;父级已有界,
            // auto_shrink([false,false]) 只作用于面板内部,不再反哺窗口尺寸)
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::default()
                        .inner_margin(egui::Margin::same(crate::ui::tokens::SETTINGS_BODY_PAD)),
                )
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("settings-body")
                        .auto_shrink([false, false])
                        .show(ui, |ui| match settings.tab {
                            SettingsTab::Appearance => appearance(
                                ui,
                                settings,
                                theme,
                                skins,
                                system_theme_ok,
                                resolved,
                                outbox,
                            ),
                            SettingsTab::Keymap => keymap_page(ui, settings, keymap, outbox),
                            SettingsTab::Ai => ai_page(ui, settings, ai, ai_key, outbox),
                            SettingsTab::Mcp => mcp_page(ui, settings, mcp, outbox),
                            SettingsTab::Image => image_page(ui, settings, bed, outbox),
                        });
                });
        });
    settings.open &= open;
    close
}

/// 外观页:主题三态 + 皮肤 + 密度 + 渲染后端只读。
fn appearance(
    ui: &mut egui::Ui,
    settings: &mut SettingsState,
    theme: &ThemeSettings,
    skins: &SkinCatalog,
    system_theme_ok: bool,
    resolved: ThemeMode,
    outbox: &mut Vec<Message>,
) {
    if cfg!(target_os = "macos") {
        macos::appearance(
            ui,
            settings,
            theme,
            skins,
            system_theme_ok,
            resolved,
            outbox,
        );
        return;
    }
    appearance_legacy(
        ui,
        settings,
        theme,
        skins,
        system_theme_ok,
        resolved,
        outbox,
    );
}

/// 非 macOS 的外观页(#169 拆出 legacy 名分):卡片化正文在此。
fn appearance_legacy(
    ui: &mut egui::Ui,
    settings: &mut SettingsState,
    theme: &ThemeSettings,
    skins: &SkinCatalog,
    system_theme_ok: bool,
    resolved: ThemeMode,
    outbox: &mut Vec<Message>,
) {
    let shell = crate::theme::shell(ui);
    ui.heading("外观");
    ui.add_space(crate::ui::tokens::SPACE_SM);
    // #70 M2:本页全部配置行走 settings_row 两列(标签列定宽、控件列同
    // 起点);原「整句标签 + 下一行控件」的区块说明拆出的文字保留在行下
    // weak 行,信息零丢失。2026-10-10 起按卡片点样分区再归入卡片。
    card(ui, &shell, "主题与皮肤", |ui| {
        settings_row(ui, "主题", |ui| theme_previews(ui, theme, outbox));
        ui.weak("外壳与正文、代码块同帧联动。");
        if theme.mode == ThemeMode::System {
            // #33:跟随系统时明暗来自系统检测,把解析值显出来——否则用户以为
            // 「我选的深色被重置了」(实际是系统侧变了/检测值与期望不符)
            ui.weak(format!(
                "跟随系统中:当前解析为「{}」;想固定明暗请直接选深色/浅色",
                resolved.label()
            ));
        }
        if theme.mode == ThemeMode::System && !system_theme_ok {
            // 检测不到就直说:Linux 无统一规范(roadmap 风险 #8),此时回落的
            // 是上一次的手动选择,不该让用户以为「跟随系统」正在生效
            ui.colored_label(
                crate::ui::tokens::WARN,
                "本机读不到系统主题设置,已回落到手动值",
            );
        }
        ui.label("工作台皮肤");
        skins::choices(ui, theme, skins, outbox);
        ui.weak("窗口、侧栏、编辑区、控件与文档统一换肤。");
        settings_row(ui, "导出皮肤", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut settings.skin_export_name)
                    .hint_text("皮肤名")
                    .desired_width(140.0),
            );
            let name = settings.skin_export_name.trim();
            if ui
                .add_enabled(!name.is_empty(), egui::Button::new("导出当前样式"))
                .clicked()
            {
                outbox.push(Message::ThemeSkinExported {
                    name: name.to_owned(),
                });
            }
        });
        ui.weak("导出到配置目录 themes/ 下,改名或删文件即增删皮肤。");
    });

    card(ui, &shell, "排版", |ui| {
        settings_row(ui, "界面密度", |ui| {
            for density in Density::ALL {
                if ui
                    .selectable_label(theme.density == density, density.label())
                    .clicked()
                {
                    outbox.push(Message::ThemeDensityChanged(density));
                }
            }
        });
        ui.weak("密度改间距与控件尺寸;字号在下方单独设置。");

        // 排版偏好(#23 F2+F3):滑杆从 theme 读**回显副本**,拖动中值变化的
        // 每帧都发消息,归约落字段并写 settings.json,下一帧滑杆位置即归约后
        // 的值 —— 与上方主题/密度选择的「即时生效」同款(不是 AI 页的草稿
        // 模式:字号/行距是拖一下就该看到的偏好,没有「保存」步骤)。渲染投影
        // 自 F3 起生效:`ThemeSettings::apply` 每帧把字号投到 Monospace 档
        // (编辑器)并覆盖 markdown style 的行距倍率(预览)。
        settings_row(ui, "字号", |ui| {
            let mut font_size = theme.editor_font_size;
            let response = ui.add(
                egui::Slider::new(&mut font_size, EDITOR_FONT_SIZE_MIN..=EDITOR_FONT_SIZE_MAX)
                    .integer(),
            );
            if response.changed() {
                outbox.push(Message::EditorFontSizeChanged(font_size));
            }
            response
        })
        .on_hover_text("正文基准字号,作用于编辑器与预览(标题按比例放大)。下限 12 是中文(CJK)可读性下限,不再往下放。");
        settings_row(ui, "行距", |ui| {
            let mut line_height = theme.line_height;
            let response = ui.add(
                egui::Slider::new(&mut line_height, LINE_HEIGHT_MIN..=LINE_HEIGHT_MAX)
                    .fixed_decimals(1)
                    .step_by(0.1),
            );
            if response.changed() {
                outbox.push(Message::EditorLineHeightChanged(line_height));
            }
            response
        })
        .on_hover_text("正文行距倍率(如 1.5 = 1.5 倍字号)。中文可读区间通常在 1.5–1.8。");
    });

    card(ui, &shell, "编辑区", |ui| {
        // #55 M2:minimap 开关与排版滑杆同款「回显副本 + changed() 即时发消息」
        // 模式(不是 AI/MCP 页的草稿模式:开关拨一下就该看到)。全局偏好,
        // 所有标签同开同关。
        settings_row(ui, "Minimap", |ui| {
            let mut show_minimap = theme.show_minimap;
            let response = ui.checkbox(&mut show_minimap, "显示源码侧缩略导航(编辑区右缘)");
            if response.changed() {
                outbox.push(Message::ShowMinimapToggled(show_minimap));
            }
        });
        settings_row(ui, "自动适应", |ui| {
            let mut auto = theme.minimap_auto;
            if ui
                .add_enabled(
                    theme.show_minimap,
                    egui::Checkbox::new(&mut auto, "短文或窄栏时收起缩略图"),
                )
                .changed()
            {
                outbox.push(Message::MinimapAutoChanged(auto));
            }
        });
        // #64 M1:打字机模式开关(#55 同款「回显副本 + changed() 即时发消息」
        // 模式)。全局偏好,源码与 Live 两模式共用;默认关(改变滚动行为的
        // 功能出厂不替用户决定,取舍见 decisions-pending #121)。
        settings_row(ui, "打字机模式", |ui| {
            let mut show_typewriter = theme.show_typewriter;
            let response = ui.checkbox(&mut show_typewriter, "光标行保持视口 1/3 线");
            if response.changed() {
                outbox.push(Message::TypewriterToggled(show_typewriter));
            }
        });
        // #64 M2:专注模式开关(#55 同款「回显副本 + changed() 即时发消息」
        // 模式)。全局偏好;Live 模式淡化非活动块,源码模式不接线(单
        // TextEdit 无法分段淡化,decisions-pending #122);默认关。
        settings_row(ui, "专注模式", |ui| {
            let mut show_focus_mode = theme.show_focus_mode;
            let response = ui.checkbox(&mut show_focus_mode, "Live 下淡化光标块之外的块");
            if response.changed() {
                outbox.push(Message::FocusModeToggled(show_focus_mode));
            }
        });
        // #57 M2:左缘标签导航三态,与主题/密度选择同款 selectable_label
        // (即时生效,非草稿模式 —— 显示偏好拨一下就该看到)。默认悬停,
        // 常显/关闭是显式选择(取舍见 decisions-pending #107)。
        settings_row(ui, "默认模式", |ui| {
            for mode in crate::theme::DefaultViewMode::ALL {
                if ui
                    .selectable_label(theme.default_view_mode == mode, mode.label())
                    .clicked()
                {
                    outbox.push(Message::DefaultViewModeChanged(mode));
                }
            }
        });
        ui.weak("应用启动与新建文档时进入的视图；上次选择跟随当前会话的模式。");

        settings_row(ui, "禅定模式", |ui| {
            for mode in ZenNavMode::ALL {
                if ui
                    .selectable_label(theme.zen_nav == mode, mode.label())
                    .clicked()
                {
                    outbox.push(Message::ZenNavModeChanged(mode));
                }
            }
        });
        ui.weak("左缘标签导航:悬停=移近左缘唤出;常显=进入禅定即显示;关闭=不渲染。");
    });

    card(ui, &shell, "渲染", |ui| {
        // 只读信息(AGENTS.md §5):后端是编译期 feature + 启动环境变量的
        // 决策,这里只显示不切换
        settings_row(ui, "渲染后端", |ui| {
            ui.weak(crate::renderer_label(
                std::env::var("LATERMD_RENDERER").ok().as_deref(),
            ));
        });
    });
}

/// 主题三态的**可点击小预览图**(形态对齐 macOS 设置页 `settings/macos.rs`
/// 的 `theme_choices`,#169 精修移植):纸面 + 侧栏条 + 三行文本的迷你工作台,
/// 选中态强调色描边。「跟随系统」用明暗对半表达。与 mac 版的差异:预览色
/// 直接取 [`ThemeSettings::shell_palette`] 的当前皮肤色板，
/// 换皮肤/调色后缩略图跟着走。
fn theme_previews(ui: &mut egui::Ui, theme: &ThemeSettings, outbox: &mut Vec<Message>) {
    let shell = crate::theme::shell(ui);
    ui.horizontal(|ui| {
        for mode in ThemeMode::ALL {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(84.0, 72.0), egui::Sense::click());
            let selected = mode == theme.mode;
            let thumb =
                egui::Rect::from_min_size(rect.min + egui::vec2(2.0, 2.0), egui::vec2(80.0, 48.0));
            let painter = ui.painter();
            // 纸面:跟随系统 = 左浅右暗对半;浅/深各取真实 shell 底
            let dark_paper = theme.shell_palette().dark;
            let light_paper = theme.shell_palette().light;
            let paper = |r: egui::Rect, dark: bool| {
                painter.rect_filled(
                    r,
                    5.0,
                    if dark {
                        dark_paper.content
                    } else {
                        light_paper.content
                    },
                );
                painter.rect_filled(
                    egui::Rect::from_min_max(r.min, egui::pos2(r.left() + 22.0, r.bottom())),
                    5.0,
                    if dark {
                        dark_paper.sidebar
                    } else {
                        light_paper.sidebar
                    },
                );
            };
            match mode {
                ThemeMode::Dark => paper(thumb, true),
                ThemeMode::System => {
                    // 跟随系统 = 左浅右暗对半
                    paper(thumb, false);
                    let right = egui::Rect::from_min_max(
                        egui::pos2(thumb.center().x, thumb.top()),
                        thumb.max,
                    );
                    painter.rect_filled(right, 5.0, dark_paper.content);
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            right.min,
                            egui::pos2(right.left() + 11.0, right.bottom()),
                        ),
                        5.0,
                        dark_paper.sidebar,
                    );
                }
                ThemeMode::Light => paper(thumb, false),
            }
            // 三行正文示意:行墨取所在纸面的 border 档,跨纸面的「跟随
            // 系统」用中间灰(两侧纸面上都读得出)
            let ink = match mode {
                ThemeMode::Dark => dark_paper.border,
                ThemeMode::Light => light_paper.border,
                ThemeMode::System => egui::Color32::from_gray(140),
            };
            for (line, width) in [(0.0, 32.0), (1.0, 40.0), (2.0, 26.0)] {
                let pos = thumb.min + egui::vec2(29.0, 13.0 + line * 8.0);
                painter.rect_filled(
                    egui::Rect::from_min_size(pos, egui::vec2(width, 2.0)),
                    1.0,
                    ink,
                );
            }
            painter.rect_stroke(
                thumb,
                5.0,
                if selected {
                    egui::Stroke::new(2.0, shell.accent)
                } else {
                    egui::Stroke::new(1.0, shell.border.gamma_multiply(0.45))
                },
                egui::StrokeKind::Outside,
            );
            // 题注:缩略图下居中
            let label_rect =
                egui::Rect::from_min_max(egui::pos2(rect.left(), rect.top() + 52.0), rect.max);
            let ink = if selected {
                shell.text
            } else {
                shell.secondary
            };
            let galley = painter.layout_no_wrap(
                mode.label().to_owned(),
                egui::FontId::proportional(12.0),
                ink,
            );
            painter.galley(
                egui::pos2(
                    label_rect.center().x - galley.size().x / 2.0,
                    label_rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                ink,
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    true,
                    selected,
                    mode.label(),
                )
            });
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
                && !selected
            {
                outbox.push(Message::ThemeChanged(mode));
            }
        }
    });
}

/// 快捷键页:命令 + 键位 + 改键/清除/重置,含撞键提示。
fn keymap_page(
    ui: &mut egui::Ui,
    settings: &mut SettingsState,
    keymap: &Keymap,
    outbox: &mut Vec<Message>,
) {
    let shell = crate::theme::shell(ui);
    ui.heading("快捷键");
    ui.weak("点击键位按钮后按下新组合;Esc 取消,Backspace 清除绑定。");
    ui.add_space(crate::ui::tokens::SPACE_SM);

    if let Some(notice) = settings.notice.as_deref() {
        ui.colored_label(crate::ui::tokens::WARN, notice);
    }

    // #70 M2:原 Grid(命令/键位/清除/重置 四列)语义等价迁移到统一两列
    // helper —— 命令标签进标签列,其余三段控件依序进控件列;捕获态与
    // notice 的原地翻转逻辑分毫未动。Grid 退役后不再有表 id。
    // 2026-10-10 卡片点样:37 行命令整体入卡,「全部恢复默认」留在卡外(全页动作,
    // 不属于任何单条命令)。
    card(ui, &shell, "命令", |ui| {
        for cmd in Command::ALL {
            settings_row(ui, cmd.label(), |ui| {
                let capturing = settings.capture == Some(cmd);
                let text = keymap
                    .get(cmd)
                    .map(|shortcut| shortcut.platform_text())
                    .unwrap_or_else(|| "未绑定".to_owned());
                // 键位按钮统一最小宽:37 行的「键位 / 清除 / 重置」排成等宽
                // 三段(原 Grid 的列对齐观感),否则按钮随组合键文字长短伸缩,
                // 右侧两列呈锯齿。捕获态文案缩短 ——「Esc 取消」已在页首说明
                let button = if capturing {
                    // 捕获中:按钮本体即提示,再点一次取消
                    egui::Button::new(
                        egui::RichText::new("按下新键位…").color(crate::ui::tokens::accent(ui)),
                    )
                } else {
                    egui::Button::new(text)
                }
                .min_size(egui::vec2(crate::ui::tokens::SETTINGS_KEY_W, 0.0));
                if ui.add(button).clicked() {
                    // 原地翻转:捕获是纯 UI 关注点,键位落盘在归约
                    settings.capture = if capturing { None } else { Some(cmd) };
                    settings.notice = None;
                }
                if ui.small_button("清除").clicked() {
                    outbox.push(Message::KeymapCleared(cmd));
                }
                if ui.small_button("重置").clicked() {
                    outbox.push(Message::KeymapReset(cmd));
                }
            });
        }
    });

    let reset_all = ui.add_enabled(!keymap.is_default(), egui::Button::new("全部恢复默认"));
    if reset_all.clicked() {
        outbox.push(Message::KeymapResetAll);
    }
}

/// AI 页:provider / 端点 / 模型 / key。
///
/// provider 是唯一开关(接口方式随 provider 派生,decisions-pending #94),
/// 下拉四选一;采样参数/system prompt/超时/流式已随「配置页参数精简」
/// 移除(decisions-pending #108),请求侧按内部默认装配(见
/// `crate::ai::set_provider`)。「获取模型列表」(#58 M2)按 provider 拉
/// 取可用模型,成功后模型名下方出现候选下拉(手输框保留,点选才覆盖)。
fn ai_page(
    ui: &mut egui::Ui,
    settings: &mut SettingsState,
    ai: &AiState,
    ai_key: &mut AiKeyState,
    outbox: &mut Vec<Message>,
) {
    let shell = crate::theme::shell(ui);
    ui.heading("AI");
    ui.add_space(crate::ui::tokens::SPACE_SM);
    // 字段级借用拆分:候选列表只读,草稿可变(下同 key_editor 的 creds)
    let models = &settings.models;
    let draft = &mut settings.ai_draft;
    // Mock 不联网也不读参数:端点/模型整块灰显,避免「配了半天没生效」;
    // Ollama 连本机服务,参数照常参与
    let editable = draft.provider.uses_settings();

    card(ui, &shell, "服务", |ui| {
        let mut provider = draft.provider;
        settings_row(ui, "Provider", |ui| {
            egui::ComboBox::from_id_salt("settings-ai-provider")
                .selected_text(provider.label())
                .show_ui(ui, |ui| {
                    for kind in ProviderKind::ALL {
                        ui.selectable_value(&mut provider, kind, kind.label());
                    }
                });
        });
        if provider != draft.provider {
            // 切换时端点/模型的出厂值跟随;手改过的字段不动
            draft.adopt_provider_defaults(provider);
        }
        ui.weak(draft.provider.description());
    });

    card(ui, &shell, "端点与模型", |ui| {
        let factory = draft.provider.factory();
        ui.add_enabled_ui(editable, |ui| {
            settings_row(ui, "Base URL", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut draft.base_url)
                        .hint_text(factory.base_url.as_str())
                        .desired_width(f32::INFINITY),
                );
            });
            settings_row(ui, "模型", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut draft.model)
                        .hint_text(factory.model.as_str())
                        .desired_width(f32::INFINITY),
                );
            });
            // 拉取成功后的候选下拉:与手输框并存,点选才覆盖草稿;手输名不
            // 在列表时回显带「(手输)」标记,不强制改写(decisions-pending #109)
            if !models.options.is_empty() {
                let known = models.options.iter().any(|name| name == &draft.model);
                let selected = if known {
                    draft.model.clone()
                } else if draft.model.trim().is_empty() {
                    format!("已获取 {} 个模型,点选填入", models.options.len())
                } else {
                    format!("{}(手输)", draft.model)
                };
                settings_row(ui, "已获取", |ui| {
                    egui::ComboBox::from_id_salt("settings-ai-models-known")
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            for name in &models.options {
                                ui.selectable_value(&mut draft.model, name.clone(), name);
                            }
                        });
                });
            }
        });

        // 上下文大小(#58 M3,decisions-pending #110):进入 prompt 的文档/
        // diff 字节上限,0 = 跟随现状默认(摘要 32KB / commit diff 16KB)。
        // prompt 组装在 provider 之前,Mock 同样生效,不随 provider 灰显。
        settings_row(ui, "上下文大小(KB)", |ui| {
            ui.add(egui::Slider::new(&mut draft.context_kb, 0..=CONTEXT_KB_MAX).text("KB"));
        });
        ui.weak(
            "0 = 跟随默认(摘要文档 32KB / commit diff 16KB);\
             超出上限的内容截断后进请求,约 1KB ≈ 250-350 token。",
        );

        // 「获取模型列表」(#58 M2):Mock 不联网禁用并说明;拉取中禁用防重入,
        // 按钮文字就是拉取中状态(与 bed 测试上传同款禁用 + 悬停说明)
        ui.add_space(crate::ui::tokens::SPACE_SM);
        let fetching = models.is_fetching();
        let hover = if fetching {
            Some("正在拉取模型列表…")
        } else if !editable {
            Some("Mock 不联网,无需获取模型列表")
        } else {
            None
        };
        let fetch = ui.add_enabled(
            editable && !fetching,
            egui::Button::new(if fetching {
                "获取中…"
            } else {
                "获取模型列表"
            }),
        );
        // on_disabled_hover_text 消费 Response:先算悬停文案再一次性挂上
        let fetch = match hover {
            Some(text) => fetch.on_disabled_hover_text(text),
            None => fetch,
        };
        if editable && !fetching && fetch.clicked() {
            outbox.push(Message::AiModelsFetchRequested);
        }
        if let Some(error) = &models.error {
            ui.colored_label(crate::ui::tokens::WARN, error);
        }

        ui.add_space(crate::ui::tokens::SPACE_SM);
        ui.horizontal(|ui| {
            let dirty = *draft != ai.config;
            let save = primary_button(ui, "保存", dirty);
            if save.clicked() {
                outbox.push(Message::AiConfigSaved(draft.clone()));
            }
            let revert = ui.add_enabled(dirty, egui::Button::new("放弃修改"));
            if revert.clicked() {
                *draft = ai.config.clone();
            }
        });
        ui.weak(format!(
            "当前生效:{} · {}",
            ai.provider_label(),
            ai.config.model
        ));
        // 端点只在联网型 provider 下有意义(Mock 不联网,不显示以免误导;
        // Ollama 连本机服务,算联网型)
        if ai.config.connects_network() {
            ui.weak(ai.config.base_url_trimmed());
        }
    });

    card(ui, &shell, "凭据", |ui| {
        ai_key::key_editor(ui, ai_key, outbox)
    });
}

/// MCP 页:开关 + 端口 + 工具权限 + 状态行 + 调用计数。
///
/// 与 AI 页同款「草稿 + 保存」分工:开关勾选不立刻生效,要点「保存」才落
/// `mcp.json` 并起停后台服务 —— 端口与工具权限都是「改了要重启监听」的
/// 动作,让用户显式确认比静默生效可预期。
fn mcp_page(
    ui: &mut egui::Ui,
    settings: &mut SettingsState,
    mcp: &McpState,
    outbox: &mut Vec<Message>,
) {
    let shell = crate::theme::shell(ui);
    ui.heading("MCP");
    ui.weak(
        "应用开着就能被本机其他 AI(Claude Code / Cursor 等)调用,检索这个文档库。\
         全部工具只读,写入仍只能由你在界面里操作。",
    );
    ui.add_space(crate::ui::tokens::SPACE_SM);

    let draft = &mut settings.mcp_draft;
    card(ui, &shell, "服务", |ui| {
        settings_row(ui, "MCP 服务", |ui| {
            ui.checkbox(&mut draft.enabled, "启用本地 MCP 服务(默认关闭)");
        });
        ui.add_enabled_ui(draft.enabled, |ui| {
            settings_row(ui, "HTTP 端口", |ui| {
                ui.add(egui::DragValue::new(&mut draft.http_port).range(McpConfig::PORT_RANGE));
                ui.weak("只监听 127.0.0.1,外部机器连不上");
            });
        });
    });

    card(ui, &shell, "工具权限", |ui| {
        ui.weak("关掉的工具对客户端就不存在:");
        for kind in ToolKind::ALL {
            let mut on = draft.tool_enabled(kind);
            let label = format!("{} — {}", kind.name(), kind.description());
            if ui.checkbox(&mut on, label).changed() {
                draft.tools.insert(kind.name().to_owned(), on);
            }
        }
    });

    card(ui, &shell, "运行状态", |ui| {
        settings_row(ui, "状态", |ui| {
            // 失败态用警示色:端口被占用是最常见的失败,不能混在普通文字里
            let label = mcp.status.label();
            if matches!(mcp.status, crate::mcp::McpStatus::Failed(_)) {
                ui.colored_label(crate::ui::tokens::WARN, label);
            } else {
                ui.label(label);
            }
        });
        settings_row(ui, "检索范围", |ui| match mcp.root() {
            Some(root) => {
                ui.weak(root.display().to_string());
            }
            None => {
                ui.colored_label(
                    crate::ui::tokens::WARN,
                    "未设置文件树根目录:先选一个目录,否则所有工具都会拒绝执行。",
                );
            }
        });

        if !mcp.counts.is_empty() {
            ui.add_space(crate::ui::tokens::SPACE_SM);
            ui.strong("本次运行调用次数:");
            for kind in ToolKind::ALL {
                if let Some(count) = mcp.counts.get(kind.name()) {
                    ui.horizontal(|ui| {
                        ui.monospace(kind.name());
                        ui.weak(count.to_string());
                    });
                }
            }
        }

        // 客户端接线示例:用户照抄即可,省去查文档
        if mcp.config.enabled {
            ui.add_space(crate::ui::tokens::SPACE_SM);
            settings_row(ui, "客户端配置", |ui| {
                let url = format!("http://127.0.0.1:{}/mcp", mcp.config.http_port);
                ui.monospace(&url);
                if ui.small_button("复制").clicked() {
                    ui.ctx().copy_text(url.clone());
                }
            });
            ui.weak("stdio 方式(客户端自己拉起进程): latermd --mcp-stdio");
        }
    });

    ui.horizontal(|ui| {
        let dirty = *draft != mcp.config;
        let save = primary_button(ui, "保存", dirty);
        if save.clicked() {
            outbox.push(Message::McpConfigSaved(draft.clone()));
        }
        let revert = ui.add_enabled(dirty, egui::Button::new("放弃修改"));
        if revert.clicked() {
            *draft = mcp.config.clone();
        }
        if dirty {
            ui.weak("保存后服务按新配置重启");
        }
    });
    ui.weak("完整设计:docs/mcp-plan.md(路径不得越出文件树根、无写工具)。");
}

/// 图片页:图床 profile 列表 / 新增 / 编辑 / 删除 / 测试上传
/// (docs/image-plan.md C 段)。
///
/// 与 AI/MCP 页同款「草稿 + 保存」分工:编辑期改动全在 [`BedDraft`],点
/// 「保存」才发 [`Message::BedProfileSaved`](token 一并写系统凭据,绝不落
/// beds.json)。「测试上传」选文件后走与图片框同一条后台上传链路,结果
/// 回显在本页,不插入任何文档。
fn image_page(
    ui: &mut egui::Ui,
    settings: &mut SettingsState,
    bed: &mut crate::bed::BedState,
    outbox: &mut Vec<Message>,
) {
    let shell = crate::theme::shell(ui);
    ui.heading("图片 · 图床");
    ui.weak(
        "图片框里的「上传」把图片发给图床,返回的 URL 插进文档。\
         token 存系统凭据,配置文件里只有 ${TOKEN} 占位符。",
    );
    ui.add_space(crate::ui::tokens::SPACE_SM);

    if let Some(notice) = settings.notice.as_deref() {
        ui.colored_label(crate::ui::tokens::WARN, notice);
    }

    // 2026-10-10 卡片点样:列表、回显与新增入口同卡(都属「现有图床」这一块);编辑器
    // 单独成卡,标题随新增/编辑切换。
    card(ui, &shell, "图床", |ui| {
        if bed.profiles.is_empty() {
            ui.weak("还没有图床,从下面的模板新增一个。");
        }
        // 克隆迭代:循环体只发消息不改列表,列表变更在归约
        for profile in bed.profiles.clone() {
            ui.horizontal(|ui| {
                ui.strong(profile.display_name());
                ui.weak(profile.body.label());
                if ui.small_button("编辑").clicked() {
                    settings.bed_draft = Some(BedDraft::from_profile(&profile));
                    settings.notice = None;
                }
                let testing = bed.is_uploading();
                let test = ui.add_enabled(!testing, egui::Button::new("测试上传"));
                if testing {
                    test.on_disabled_hover_text("有上传进行中,稍候");
                } else if test.clicked() {
                    outbox.push(Message::BedTestUploadRequested {
                        profile_id: profile.id.clone(),
                    });
                }
                if ui.small_button("删除").clicked() {
                    outbox.push(Message::BedProfileDeleted {
                        id: profile.id.clone(),
                    });
                }
            });
        }

        // 测试上传回显:成功给可复制的 URL,失败给警示色原因
        if let Some((name, result)) = &bed.last_test {
            match result {
                Ok(url) => {
                    ui.label(format!("测试上传({name})成功:"));
                    ui.horizontal(|ui| {
                        ui.monospace(url);
                        if ui.small_button("复制").clicked() {
                            ui.ctx().copy_text(url.clone());
                        }
                    });
                }
                Err(error) => {
                    ui.colored_label(
                        crate::ui::tokens::WARN,
                        format!("测试上传({name})失败:{error}"),
                    );
                }
            }
        }
        if bed.is_uploading() {
            ui.colored_label(crate::ui::tokens::accent(ui), "上传中…");
        }

        // 新增入口:编辑器开着时收起(编辑态不该再点出第二个草稿)
        if settings.bed_draft.is_none() {
            ui.add_space(crate::ui::tokens::SPACE_SM);
            settings_row(ui, "新增图床", |ui| {
                for (label, preset) in [
                    ("SM.MS", latermd_bed::BedProfile::preset_smms()),
                    ("GitHub", latermd_bed::BedProfile::preset_github()),
                    ("自定义", latermd_bed::BedProfile::preset_custom()),
                ] {
                    if ui.button(label).clicked() {
                        settings.bed_draft = Some(BedDraft {
                            headers_text: headers_to_text(&preset.headers),
                            profile: preset,
                            token: String::new(),
                        });
                        settings.notice = None;
                    }
                }
            });
        }
    });

    if let Some(draft) = settings.bed_draft.as_mut() {
        let title = if draft.profile.id.is_empty() {
            "新增图床"
        } else {
            "编辑图床"
        };
        // 关闭编辑器(保存成功/取消)由闭包外的 result 带回 —— 闭包里借不出 settings
        let mut close_editor = false;
        card(ui, &shell, title, |ui| {
            close_editor = bed_editor(ui, &mut settings.notice, draft, outbox);
        });
        if close_editor {
            settings.bed_draft = None;
        }
    }
}

/// 图床编辑器本体(新增与编辑共用;卡片标题由调用方按 `draft.profile.id`
/// 取「新增图床」/「编辑图床」)。`notice` 是页内提示槽(校验失败留在
/// 编辑器里改);返回 `true` = 保存成功或取消,调用方据此收起编辑器。
fn bed_editor(
    ui: &mut egui::Ui,
    notice: &mut Option<String>,
    draft: &mut BedDraft,
    outbox: &mut Vec<Message>,
) -> bool {
    let profile = &mut draft.profile;
    // #70 M2:原两列 Grid 迁到统一 helper(标签列定宽与其他分页同起点);
    // 「请求头」行的多行输入由 `horizontal` 交叉居中自然落位
    settings_row(ui, "名称", |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut profile.name)
                .hint_text("如 我的 SM.MS")
                .desired_width(360.0),
        );
    });
    settings_row(ui, "API 地址", |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut profile.api_url)
                .hint_text("https://…/upload;可用 ${NAME} 代替本次文件名")
                .desired_width(360.0),
        );
    });
    settings_row(ui, "表单字段名", |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut profile.file_field)
                .hint_text("SM.MS=smfile,Lsky=file")
                .desired_width(360.0),
        );
    });
    settings_row(ui, "URL 取值路径", |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut profile.url_path)
                .hint_text("返回 JSON 里的点分路径,如 data.url")
                .desired_width(360.0),
        );
    });
    settings_row(ui, "URL 前缀", |ui| {
        let mut prefix = profile.url_prefix.clone().unwrap_or_default();
        let response = ui.add(
            egui::TextEdit::singleline(&mut prefix)
                .hint_text("返回路径而非完整 URL 时拼在前面;留空不拼")
                .desired_width(360.0),
        );
        if response.changed() {
            profile.url_prefix = Some(prefix);
        }
    });
    settings_row(ui, "编码方式", |ui| {
        egui::ComboBox::from_id_salt("bed-body-style")
            .selected_text(profile.body.label())
            .show_ui(ui, |ui| {
                for style in [
                    latermd_bed::BedBody::Multipart,
                    latermd_bed::BedBody::Base64Json,
                ] {
                    ui.selectable_value(&mut profile.body, style, style.label());
                }
            });
    });
    settings_row(ui, "请求头", |ui| {
        ui.add(
            egui::TextEdit::multiline(&mut draft.headers_text)
                .hint_text("每行一条「名: 值」,值可写 ${TOKEN}")
                .desired_rows(3)
                .desired_width(360.0),
        );
    });
    settings_row(ui, "Token", |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut draft.token)
                .password(true)
                .hint_text("保存时写入系统凭据;留空 = 不改已存值")
                .desired_width(360.0),
        );
    });

    ui.add_space(crate::ui::tokens::SPACE_SM);
    let mut close = false;
    ui.horizontal(|ui| {
        if primary_button(ui, "保存", true).clicked() {
            // 先把多行文本并进 profile 再校验;失败留在编辑器里改
            match headers_from_text(&draft.headers_text) {
                Ok(headers) => {
                    let mut final_profile = profile.clone();
                    final_profile.headers = headers;
                    final_profile.normalize();
                    if final_profile.api_url.is_empty() || final_profile.url_path.is_empty() {
                        *notice = Some("API 地址与 URL 取值路径必填".to_owned());
                    } else {
                        outbox.push(Message::BedProfileSaved {
                            profile: final_profile,
                            token: Some(draft.token.clone()),
                        });
                        close = true;
                    }
                }
                Err(error) => *notice = Some(error),
            }
        }
        if ui.button("取消").clicked() {
            close = true;
        }
    });
    close
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::Shortcut;
    use crate::state::State;
    use egui::RawInput;

    /// 测试口径的 resolved(与 draw 一致);State 解构前调用。
    fn resolved_theme_for_test(state: &State) -> ThemeMode {
        state.resolved_theme()
    }

    fn render(state: &mut State) {
        let ctx = egui::Context::default();
        let system_theme_ok = state.system_theme_ok;
        let resolved = state.resolved_theme();
        // 借用拆分:settings / ai_key / bed 可变,其余只读(与 draw 的口径一致)
        let State {
            settings,
            ai_key,
            ai,
            mcp,
            bed,
            keymap,
            theme,
            skins,
            ..
        } = state;
        let mut outbox = Vec::new();
        let output = ctx.run_ui(RawInput::default(), |ui| {
            dialog(
                ui,
                settings,
                theme,
                skins,
                system_theme_ok,
                resolved,
                keymap,
                ai,
                ai_key,
                mcp,
                bed,
                &mut outbox,
            );
        });
        output.drop_without_applying_deltas();
    }

    /// 五个分页各渲一帧不 panic(含 AI 页的禁用态、MCP 页的规划态与图片
    /// 页的空列表/编辑草稿态)。
    #[test]
    fn every_tab_renders() {
        let mut state = State::default();
        state.settings.open = true;
        for tab in SettingsTab::ALL {
            state.settings.tab = tab;
            state.settings.bed_draft = None;
            render(&mut state);
        }
        // AI 页四个 provider 各再渲一帧:说明文案/出厂值提示/参数区显隐
        // 随 provider 变化,都要走一遍真实绘制路径
        state.settings.tab = SettingsTab::Ai;
        for provider in ProviderKind::ALL {
            state.settings.ai_draft = AiConfig {
                provider,
                ..AiConfig::default()
            };
            render(&mut state);
        }
        // 图片页带草稿与已存 profile 的形态也渲一帧
        state.settings.tab = SettingsTab::Image;
        state.bed.profiles = vec![latermd_bed::BedProfile::preset_smms()];
        state.bed.last_test = Some(("SM.MS".to_owned(), Err("HTTP 401:bad token".to_owned())));
        state.settings.bed_draft = Some(BedDraft::from_profile(
            &latermd_bed::BedProfile::preset_github(),
        ));
        render(&mut state);
    }

    /// #70 M2 对齐探针:在 CentralPanel 画布直渲一个页面,收集全部
    /// TextShape 的(trim 后文本, 位置)。画布给足高度 —— 快捷键页 37 行
    /// 超出常规视口的部分会被 clip 剔除形状。2 帧取末帧避开首帧 warm-up
    /// (同款手法见 `appearance_page_renders_font_prefs_sliders`)。
    /// 页面文本位置 + 矩形形状(按钮框等)双探针。
    fn collect_page_texts(
        ctx: &egui::Context,
        mut draw: impl FnMut(&mut egui::Ui),
    ) -> (Vec<(String, egui::Pos2)>, Vec<egui::Rect>) {
        let mut out = (Vec::new(), Vec::new());
        for _ in 0..2 {
            let mut texts = Vec::new();
            let mut rects = Vec::new();
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(1200.0, 1800.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| draw(ui));
                },
            );
            let mut output = output;
            output.textures_delta.clear();
            for clipped in &output.shapes {
                match &clipped.shape {
                    egui::epaint::Shape::Text(t) => {
                        texts.push((t.galley.text().trim().to_owned(), t.pos));
                    }
                    // 按钮背景是圆角 RectShape(羽化前的形状层);隔条/滑轨
                    // 等细长矩形由宽度判据天然排除
                    egui::epaint::Shape::Rect(r)
                        if (18.0..=44.0).contains(&r.rect.height()) && r.rect.width() >= 30.0 =>
                    {
                        rects.push(r.rect);
                    }
                    _ => {}
                }
            }
            out = (texts, rects);
        }
        out
    }

    /// 文本形状定位:精确匹配(trim 后全等)首个命中;找不到即 panic
    /// (同时充当「控件在场」断言)。
    fn pos_of(texts: &[(String, egui::Pos2)], needle: &str) -> egui::Pos2 {
        texts
            .iter()
            .find(|(t, _)| t == needle)
            .map(|(_, p)| *p)
            .unwrap_or_else(|| panic!("文本「{needle}」未渲出,页面文本:{texts:?}"))
    }

    /// 文本形状的**全部**命中位置(标签与输入框 hint 同名时用这个区分)。
    fn positions_of(texts: &[(String, egui::Pos2)], needle: &str) -> Vec<egui::Pos2> {
        texts
            .iter()
            .filter(|(t, _)| t == needle)
            .map(|(_, p)| *p)
            .collect()
    }

    /// 一组文本的 x 坐标全部一致(标签列左对齐 / 控件列同起点的判据;
    /// 0.25px 容差吸收布局取整)。
    fn assert_same_x(page: &str, kind: &str, xs: Vec<f32>) {
        let (min, max) = xs
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(*x), hi.max(*x)));
        assert!(
            max - min <= 0.25,
            "{page}:{kind} 列起点不齐,x 范围 [{min},{max}]"
        );
    }

    /// Legacy appearance and the four shared form pages keep their columns.
    /// The grouped macOS appearance page is covered in settings::macos::tests.
    /// #70 M2:五分页全部配置行两列对齐。四层断言:
    /// ①每页所有行标签的文本 x 全等(标签列左对齐);
    /// ②同类控件的文本 x 全等(selectable 选项 / 复选框文字 / 输入框
    ///   hint / 小按钮,各按自身内边距成列);
    /// ③同行「标签 ↔ 控件内文本」y 差 ≤2px(同基线 —— 输入框内文字
    ///   与行标签文字垂直对齐);
    /// ④跨分页标签列同一起点(控件列由「标签列定宽 + 同一列间隙」推出
    ///   也跨页一致)。
    #[test]
    fn every_tab_rows_align_two_columns() {
        // —— 外观 ——
        let mut state = State::default();
        let ctx = egui::Context::default();
        state.theme.apply(&ctx, state.theme.mode);
        let skins = state.skins.clone();
        let system_theme_ok = state.system_theme_ok;
        let resolved = state.resolved_theme();
        let (texts, _) = {
            let State {
                settings, theme, ..
            } = &mut state;
            collect_page_texts(&ctx, |ui| {
                appearance_legacy(
                    ui,
                    settings,
                    theme,
                    &skins,
                    system_theme_ok,
                    resolved,
                    &mut Vec::new(),
                );
            })
        };
        let appearance_labels = [
            "主题",
            "工作台皮肤",
            "导出皮肤",
            "界面密度",
            "字号",
            "行距",
            "Minimap",
            "打字机模式",
            "专注模式",
            "禅定模式",
            "渲染后端",
        ];
        assert_same_x(
            "外观",
            "行标签",
            appearance_labels
                .iter()
                .map(|l| pos_of(&texts, l).x)
                .collect(),
        );
        // 控件在场(逐项):三态选项与复选框文字全部真实渲出。
        // 「控件列同起点」取**每行首控件**(主题=浅色 / 密度=宽松 /
        // 禅定=悬停唤出):行内后续选项天然右排,不参与同列断言
        // 主题行已换「可点击小预览图」(题注居中在缩略图下,不再参与列
        // 对齐);密度/禅定仍是 selectable 首选项,继续钉同一起点
        let first_selectable = ["宽松", "悬停唤出"];
        for opt in ["浅色", "深色", "跟随系统", "标准", "常显", "关闭"] {
            pos_of(&texts, opt);
        }
        assert_same_x(
            "外观",
            "selectable 首选项",
            first_selectable
                .iter()
                .map(|l| pos_of(&texts, l).x)
                .collect(),
        );
        let checkbox_texts = [
            "显示源码侧缩略导航(编辑区右缘)",
            "光标行保持视口 1/3 线",
            "Live 下淡化光标块之外的块",
        ];
        assert_same_x(
            "外观",
            "复选框文字",
            checkbox_texts.iter().map(|l| pos_of(&texts, l).x).collect(),
        );
        // 主题行是小预览图:题注在缩略图下方(行内偏下),标签行垂直居中
        // 于行 —— 二者天然不共线,改断「题注在标签行之下、同行不漂移」
        assert!(
            pos_of(&texts, "浅色").y > pos_of(&texts, "主题").y,
            "主题预览图题注应落在行标签下方"
        );
        assert!(
            (pos_of(&texts, "Minimap").y - pos_of(&texts, checkbox_texts[0]).y).abs() <= 2.0,
            "「Minimap」行标签与复选框文字基线差 >2px"
        );
        // 滑杆回显值仍在场(控件齐全的一环)
        pos_of(&texts, "15");
        pos_of(&texts, "1.5");
        let label_x = pos_of(&texts, appearance_labels[0]).x;

        // —— 快捷键 ——
        let keymap = Keymap::builtin();
        let mut settings = SettingsState::default();
        let mut outbox = Vec::new();
        let (texts, rects) = collect_page_texts(&ctx, |ui| {
            keymap_page(ui, &mut settings, &keymap, &mut outbox);
        });
        assert!(outbox.is_empty(), "渲染不产出消息");
        assert_same_x(
            "快捷键",
            "命令标签",
            Command::ALL
                .iter()
                .map(|c| pos_of(&texts, c.label()).x)
                .collect(),
        );
        // 控件列:每行「清除 / 重置」小按钮文字同起点(37 行全查),
        // 已绑定命令的键位文本也落在同一起点
        assert_same_x(
            "快捷键",
            "清除按钮",
            texts
                .iter()
                .filter(|(t, _)| t == "清除")
                .map(|(_, p)| p.x)
                .collect(),
        );
        assert_same_x(
            "快捷键",
            "重置按钮",
            texts
                .iter()
                .filter(|(t, _)| t == "重置")
                .map(|(_, p)| p.x)
                .collect(),
        );
        let shortcut = keymap
            .get(Command::Save)
            .expect("出厂 Save 有绑定")
            .platform_text();
        // 键位文本在等宽按钮内水平居中(egui 按钮排版),列对齐按任务书
        // 口径断「控件 rect 左缘」:挑出全部等宽键位按钮框(宽 ≈
        // SETTINGS_KEY_W 的矩形),37 枚左缘一致
        let key_button_lefts: Vec<f32> = rects
            .iter()
            .filter(|r| (r.width() - crate::ui::tokens::SETTINGS_KEY_W).abs() <= 2.0)
            .map(|r| r.left())
            .collect();
        assert_eq!(
            key_button_lefts.len(),
            Command::ALL.len(),
            "等宽键位按钮矩形应每行一枚:{rects:?}"
        );
        assert_same_x("快捷键", "键位按钮框", key_button_lefts);
        assert!(
            (pos_of(&texts, Command::Save.label()).y - pos_of(&texts, &shortcut).y).abs() <= 2.0,
            "「{}」行标签与键位文本基线差 >2px",
            Command::Save.label()
        );
        pos_of(&texts, "全部恢复默认");
        assert!(
            (label_x - pos_of(&texts, Command::ALL[0].label()).x).abs() <= 0.25,
            "外观页与快捷键页标签列起点不一致"
        );

        // —— AI(切到联网型 provider,输入框真实渲出)——
        state.settings.ai_draft.provider = ProviderKind::OpenAiCompatible;
        state.settings.models = ModelListState::default();
        let (texts, _) = {
            let State {
                settings,
                ai_key,
                ai,
                ..
            } = &mut state;
            collect_page_texts(&ctx, |ui| {
                ai_page(ui, settings, ai, ai_key, &mut Vec::new());
            })
        };
        let factory = state.settings.ai_draft.provider.factory();
        let ai_labels = ["Provider", "Base URL", "模型", "上下文大小(KB)", "API key"];
        assert_same_x(
            "AI",
            "行标签",
            ai_labels.iter().map(|l| pos_of(&texts, l).x).collect(),
        );
        let base_url_hint = factory.base_url.as_str();
        let model_hint = factory.model.as_str();
        // 「API key」出现两次:标签列一次、key_editor 输入框 hint 一次;
        // 第二个就是控件列里的 hint
        let api_key_hits = positions_of(&texts, "API key");
        assert_eq!(
            api_key_hits.len(),
            2,
            "「API key」应有标签与 hint 两处:{texts:?}"
        );
        assert_same_x(
            "AI",
            "输入框 hint",
            vec![
                pos_of(&texts, base_url_hint).x,
                pos_of(&texts, model_hint).x,
                api_key_hits[1].x,
            ],
        );
        assert!(
            (pos_of(&texts, "Base URL").y - pos_of(&texts, base_url_hint).y).abs() <= 2.0,
            "「Base URL」行标签与输入框内文本基线差 >2px"
        );
        pos_of(&texts, "获取模型列表");
        assert!(
            (label_x - pos_of(&texts, ai_labels[0]).x).abs() <= 0.25,
            "外观页与 AI 页标签列起点不一致"
        );

        // —— MCP(草稿启用 + 已存配置启用,两段行都在场)——
        let mut settings = SettingsState::default();
        settings.mcp_draft.enabled = true;
        let mut mcp = McpState::default();
        mcp.config.enabled = true;
        let (texts, _) = collect_page_texts(&ctx, |ui| {
            mcp_page(ui, &mut settings, &mcp, &mut Vec::new());
        });
        let mcp_labels = ["MCP 服务", "HTTP 端口", "状态", "检索范围", "客户端配置"];
        assert_same_x(
            "MCP",
            "行标签",
            mcp_labels.iter().map(|l| pos_of(&texts, l).x).collect(),
        );
        pos_of(&texts, "启用本地 MCP 服务(默认关闭)");
        pos_of(&texts, "只监听 127.0.0.1,外部机器连不上");
        assert!(
            (label_x - pos_of(&texts, mcp_labels[0]).x).abs() <= 0.25,
            "外观页与 MCP 页标签列起点不一致"
        );

        // —— 图片(编辑草稿态:字段全空,hint 全部在场)——
        let mut settings = SettingsState::default();
        let mut bed = crate::bed::BedState::default();
        let mut profile = latermd_bed::BedProfile::preset_custom();
        profile.name.clear();
        profile.api_url.clear();
        profile.file_field.clear();
        profile.url_path.clear();
        profile.url_prefix = None;
        settings.bed_draft = Some(BedDraft {
            profile,
            headers_text: String::new(),
            token: String::new(),
        });
        let (texts, _) = collect_page_texts(&ctx, |ui| {
            image_page(ui, &mut settings, &mut bed, &mut Vec::new());
        });
        let bed_labels = [
            "名称",
            "API 地址",
            "表单字段名",
            "URL 取值路径",
            "URL 前缀",
            "编码方式",
            "请求头",
            "Token",
        ];
        assert_same_x(
            "图片",
            "行标签",
            bed_labels.iter().map(|l| pos_of(&texts, l).x).collect(),
        );
        let bed_hints = [
            "如 我的 SM.MS",
            "https://…/upload;可用 ${NAME} 代替本次文件名",
            "SM.MS=smfile,Lsky=file",
            "返回 JSON 里的点分路径,如 data.url",
            "返回路径而非完整 URL 时拼在前面;留空不拼",
            "每行一条「名: 值」,值可写 ${TOKEN}",
            "保存时写入系统凭据;留空 = 不改已存值",
        ];
        assert_same_x(
            "图片",
            "输入框 hint",
            bed_hints.iter().map(|h| pos_of(&texts, h).x).collect(),
        );
        assert!(
            (pos_of(&texts, "名称").y - pos_of(&texts, bed_hints[0]).y).abs() <= 2.0,
            "「名称」行标签与输入框内文本基线差 >2px"
        );
        pos_of(&texts, "保存");
        pos_of(&texts, "取消");
        assert!(
            (label_x - pos_of(&texts, bed_labels[0]).x).abs() <= 0.25,
            "外观页与图片页标签列起点不一致"
        );

        // —— 图片(无草稿态:「新增图床」行同列)——
        settings.bed_draft = None;
        let (texts, _) = collect_page_texts(&ctx, |ui| {
            image_page(ui, &mut settings, &mut bed, &mut Vec::new());
        });
        assert!(
            (label_x - pos_of(&texts, "新增图床").x).abs() <= 0.25,
            "无草稿态「新增图床」行标签偏离统一列起点"
        );
        pos_of(&texts, "SM.MS");
        pos_of(&texts, "GitHub");
        pos_of(&texts, "自定义");
    }

    /// #58 渲染探针:AI 页只剩 provider/Base URL/模型/上下文大小(#58 M3
    /// 新增的请求预算滑杆)/key,被删参数控件
    /// (Temperature/Top-P/Max tokens/超时/流式接收/System prompt)不再
    /// 渲染。无头渲真实 AI 页数帧,收集全部 `TextShape` 的逻辑文本做
    /// 双向断言(保留项在场 + 被删项绝迹 —— 后者同时钉住「当前生效」行
    /// 不再报流式形态)。2026-10-10 起按卡片点样直渲 `ai_page` 而非整窗:设置窗 440px 视口
    /// 装不下三张卡,保存行落在 ScrollArea 折叠线以下会被裁掉 —— 页面内容
    /// 的在场断言归页面直渲,整窗的「渲不 panic」由 `every_tab_renders`
    /// 覆盖(与外观页滑杆/开关探针同口径)。
    #[test]
    fn ai_page_renders_kept_fields_without_removed_parameter_widgets() {
        let mut state = State::default();
        state.settings.open = true;
        state.settings.tab = SettingsTab::Ai;
        // 切到可编辑 provider:Base URL/模型输入框真实渲出(参数区旧位置)
        state.settings.ai_draft.provider = ProviderKind::OpenAiCompatible;
        let mut all_text = String::new();
        render_ai_page_collect(&mut state, 2, &mut all_text);
        assert!(!all_text.is_empty(), "两帧后仍无文本形状:无头管线异常");
        for kept in ["Provider", "Base URL", "模型", "上下文大小", "保存"] {
            assert!(
                all_text.contains(kept),
                "AI 页应保留 \"{kept}\":\n{all_text}"
            );
        }
        for removed in [
            "Temperature",
            "Top-P",
            "Max tokens",
            "超时",
            "流式接收",
            "System prompt",
        ] {
            assert!(
                !all_text.contains(removed),
                "AI 页不应再渲染 \"{removed}\":\n{all_text}"
            );
        }
    }

    // ---- 「获取模型列表」(#58 M2):状态机 + 渲染探针(全部零网络;
    // 假装配注入点与 AiKeyState::creds 同理念,真实端点验证留人工) ----

    /// 定时回固定结果的假装配:spawn 真线程(验线程机制)但绝不碰网络。
    fn fake_spawner(delay: std::time::Duration, result: ModelsResult) -> ModelsSpawner {
        Arc::new(move |_source, tx| {
            let result = result.clone();
            std::thread::Builder::new()
                .name("fake-models-spawner".into())
                .spawn(move || {
                    std::thread::sleep(delay);
                    let _ = tx.send(result);
                })
                .ok()
        })
    }

    /// 轮询到第一条结果消息(线程异步,限时 5s)。
    fn wait_result(models: &mut ModelListState) -> Vec<Message> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut messages = Vec::new();
        while messages.is_empty() && std::time::Instant::now() < deadline {
            messages = models.poll();
            if messages.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        messages
    }

    /// 状态机生命周期:start 置在途、poll 非阻塞收结果、finish 落地
    /// (成功覆盖候选清错误行;失败落错误行保旧候选)。
    #[test]
    fn model_list_state_lifecycle_start_poll_finish() {
        let mut models = ModelListState::default();
        assert!(!models.is_fetching());
        assert!(models.poll().is_empty(), "空闲 poll 为空");

        let source = ModelsSource::Ollama {
            base_url: "http://模型列表测试.invalid".to_owned(),
        };
        models.spawner = fake_spawner(
            std::time::Duration::from_millis(20),
            Ok(vec!["m1".to_owned(), "m2".to_owned()]),
        );
        models.start(source.clone());
        assert!(models.is_fetching(), "发起后置在途");

        let messages = wait_result(&mut models);
        assert_eq!(
            messages,
            vec![Message::AiModelsFetchFinished {
                result: Ok(vec!["m1".to_owned(), "m2".to_owned()])
            }]
        );
        models.finish(Ok(vec!["m1".to_owned(), "m2".to_owned()]));
        assert!(!models.is_fetching(), "收尾回空闲");
        assert_eq!(models.options, vec!["m1".to_owned(), "m2".to_owned()]);
        assert!(models.error.is_none(), "成功清错误行");

        // 失败:文案落错误行,旧候选保留(上次拉到的列表仍可用)
        models.spawner = fake_spawner(
            std::time::Duration::from_millis(20),
            Err("获取模型列表失败:HTTP 401:bad key".to_owned()),
        );
        models.start(source);
        let messages = wait_result(&mut models);
        assert_eq!(messages.len(), 1);
        models.finish(Err("获取模型列表失败:HTTP 401:bad key".to_owned()));
        assert!(!models.is_fetching());
        assert_eq!(
            models.error.as_deref(),
            Some("获取模型列表失败:HTTP 401:bad key")
        );
        assert_eq!(
            models.options,
            vec!["m1".to_owned(), "m2".to_owned()],
            "失败不清旧候选"
        );
    }

    /// 防重入:在途时再次发起被忽略(第一次的接收端不被替换,结果照常
    /// 交付);收尾后可重新发起。UI 禁用按钮是第一道防线,归约侧忽略是
    /// 同一语义的第二道(与 `AiState` 的 streaming 防重入口径一致)。
    #[test]
    fn model_list_state_ignores_start_while_inflight() {
        let mut models = ModelListState::default();
        let source = ModelsSource::Ollama {
            base_url: "http://模型列表测试.invalid".to_owned(),
        };
        models.spawner = fake_spawner(
            std::time::Duration::from_millis(80),
            Ok(vec!["old".to_owned()]),
        );
        models.start(source.clone());
        models.spawner = fake_spawner(std::time::Duration::ZERO, Ok(vec!["new".to_owned()]));
        models.start(source.clone());
        assert!(models.is_fetching());

        let messages = wait_result(&mut models);
        assert_eq!(
            messages,
            vec![Message::AiModelsFetchFinished {
                result: Ok(vec!["old".to_owned()])
            }],
            "在途时第二次发起被忽略,交付的是第一次的结果"
        );
        models.finish(Ok(vec!["old".to_owned()]));
        assert!(!models.is_fetching());

        // 空闲后再发起:新结果照常可达
        models.spawner = fake_spawner(std::time::Duration::ZERO, Ok(vec!["fresh".to_owned()]));
        models.start(source);
        assert_eq!(
            wait_result(&mut models),
            vec![Message::AiModelsFetchFinished {
                result: Ok(vec!["fresh".to_owned()])
            }]
        );
    }

    /// 断连兜底:发送端没发出结果就退出(线程异常/panic 的可观察形态就是
    /// channel 断连)→ 补一条失败,「获取中」不卡死;已交出过结果再断连
    /// 不重复报错(与 `BedState::saw_result` 同口径)。
    #[test]
    fn model_list_state_disconnected_without_result_synthesizes_failure() {
        let (tx, rx) = mpsc::channel();
        drop(tx);
        let mut models = ModelListState {
            rx: Some(rx),
            ..ModelListState::default()
        };
        assert_eq!(
            models.poll(),
            vec![Message::AiModelsFetchFinished {
                result: Err("获取模型列表失败:后台线程意外中断".to_owned())
            }]
        );

        // 已交出过结果:断连不再追加第二条失败
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(vec!["x".to_owned()])).unwrap();
        drop(tx);
        let mut models = ModelListState {
            rx: Some(rx),
            ..ModelListState::default()
        };
        assert_eq!(models.poll().len(), 1, "结果照常交付");
        assert!(models.poll().is_empty(), "断连不重复补失败");
    }

    /// 渲 AI 页若干帧并把全部 TextShape 的文本拼进 `sink`(time 逐帧推进,
    /// 无头老坑见 `ai_page_renders_kept_fields…`)。直渲 `ai_page`(同
    /// `appearance` 探针:设置窗视口外的内容不进 clip,直渲控件最聚焦)。
    fn render_ai_page_collect(state: &mut State, frames: usize, sink: &mut String) {
        let ctx = egui::Context::default();
        state.theme.apply(&ctx, state.theme.mode);
        let mut now = 0.0_f64;
        for _ in 0..frames {
            now += 0.1;
            let State {
                settings,
                ai_key,
                ai,
                ..
            } = state;
            let mut outbox = Vec::new();
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(1200.0, 800.0),
                    )),
                    time: Some(now),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        ai_page(ui, settings, ai, ai_key, &mut outbox);
                    });
                },
            );
            output.textures_delta.clear();
            for clipped in &output.shapes {
                if let egui::epaint::Shape::Text(text) = &clipped.shape {
                    sink.push_str(text.galley.text());
                    sink.push('\n');
                }
            }
        }
    }

    /// 按钮状态机四态真实渲出,且拉取期间 UI 不阻塞:慢装配(300ms)在途
    /// 时连续五帧的总耗时远小于装配延迟(同步等待会 ≥300ms),「获取中…」
    /// 可见;成功帧出现候选下拉与手输回显;失败帧错误文案落地。禁用态的
    /// 「点击不产消息」防线在归约侧
    /// (`state::tests::ai_models_fetch_request_gate_and_finish_landing`)。
    #[test]
    fn ai_page_renders_fetch_states_and_stays_responsive_while_fetching() {
        let mut state = State::default();
        state.ai_key.creds = latermd_creds::Credentials::in_memory();
        state.settings.ai_draft.provider = ProviderKind::OpenAiCompatible;

        // 空闲:按钮文案在场
        let mut text = String::new();
        render_ai_page_collect(&mut state, 2, &mut text);
        assert!(text.contains("获取模型列表"), "空闲按钮在场:{text}");

        // 发起(慢装配 300ms):连续五帧远小于装配延迟 = UI 没有同步等待
        state.settings.models.spawner = fake_spawner(
            std::time::Duration::from_millis(300),
            Ok(vec!["fake-model".to_owned()]),
        );
        state.apply(Message::AiModelsFetchRequested);
        assert!(state.settings.models.is_fetching());

        let mut text = String::new();
        let start = std::time::Instant::now();
        render_ai_page_collect(&mut state, 5, &mut text);
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(200),
            "拉取中五帧耗时 {elapsed:?},UI 疑似被后台拉取阻塞"
        );
        assert!(text.contains("获取中…"), "拉取中状态可见:{text}");
        assert!(
            !text.contains("获取模型列表"),
            "拉取中原按钮文案不出现:{text}"
        );

        // 结果到达:收流归约 → 成功帧出现候选下拉;出厂模型名不在候选,
        // 回显「(手输)」标记(不强制改写,decisions-pending #109)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.settings.models.is_fetching() && std::time::Instant::now() < deadline {
            for message in state.poll_models() {
                state.apply(message);
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!state.settings.models.is_fetching(), "结果已落地");
        let mut text = String::new();
        render_ai_page_collect(&mut state, 2, &mut text);
        assert!(text.contains("已获取"), "成功后出现候选下拉:{text}");
        assert!(text.contains("(手输)"), "手输名不在候选时回显标记:{text}");

        // 手输名恰在候选里:回显名字本身,无标记
        state.settings.ai_draft.model = "fake-model".to_owned();
        let mut text = String::new();
        render_ai_page_collect(&mut state, 2, &mut text);
        assert!(text.contains("fake-model"), "{text}");
        assert!(!text.contains("(手输)"), "候选内的名字不带标记:{text}");

        // 失败态:错误文案上屏(警示色错误行)
        state.settings.models.spawner = fake_spawner(
            std::time::Duration::ZERO,
            Err("获取模型列表失败:HTTP 401:bad key".to_owned()),
        );
        state.apply(Message::AiModelsFetchRequested);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.settings.models.is_fetching() && std::time::Instant::now() < deadline {
            for message in state.poll_models() {
                state.apply(message);
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let mut text = String::new();
        render_ai_page_collect(&mut state, 2, &mut text);
        assert!(
            text.contains("获取模型列表失败:HTTP 401:bad key"),
            "失败文案上屏:{text}"
        );
        assert!(text.contains("已获取"), "失败帧旧候选下拉保留:{text}");
    }

    /// #23 F2:外观页两根排版滑杆真实渲出 —— 「字号」「行距」标签与 theme
    /// 当前回显值进入末帧 shapes 的**精确** Text 形状(galley 文本恰等于
    /// 标签/值,不与区块标题整句撞车),滑杆轨道(rail)至少两根;无拖动的
    /// 初始帧渲染不产出消息(改动只经拖动时的 `changed()` 走归约)。两轮
    /// 回显值(出厂 15/1.5 与自定义 20/1.8)证明滑杆的值绑定在 theme 字段
    /// 上(回显来自归约侧真值,而不是 UI 本地草稿)。
    ///
    /// 直接渲 `appearance()` 而不是整个 `dialog`:设置窗 default_size 440px
    /// 视口只装到「排版」区块标题,滑杆在 ScrollArea 视口之下(首屏要滚一下
    /// 才看到,产品上可接受);clip 之外 tessellation 会剔除形状,滑杆就取
    /// 不到断言了。在整窗里的「渲不 panic」由 `every_tab_renders` 覆盖,本
    /// 测试只断言滑杆本体。与 #35 同款环境:先跑主题投影(滑杆轨道填充色
    /// 硬绑 `widgets.inactive.bg_fill`)。
    #[test]
    fn appearance_page_renders_font_prefs_sliders() {
        let mut state = State::default();
        let ctx = egui::Context::default();
        state.theme.apply(&ctx, state.theme.mode);

        for (font_size, line_height) in [(15.0_f32, 1.5_f32), (20.0, 1.8)] {
            state.theme.editor_font_size = font_size;
            state.theme.line_height = line_height;
            let mut last_shapes = Vec::new();
            let mut last_outbox = Vec::new();
            let system_theme_ok = state.system_theme_ok;
            let resolved = resolved_theme_for_test(&state);
            let skins = state.skins.clone();
            // 3 帧取末帧:appearance() 直接渲没有 Window fade-in,多帧只为
            // 避开可能的布局 warm-up 首帧(egui Slider 的 DragValue 编辑态)
            for _ in 0..3 {
                let State {
                    settings, theme, ..
                } = &mut state;
                let mut outbox = Vec::new();
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::pos2(0.0, 0.0),
                            egui::vec2(1200.0, 800.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        // egui 0.36:面板嵌套 Ui 用 `show`(旧 `show_inside`
                        // 已弃用);这里只借一层全屏 Ui 当画布,不参与布局
                        egui::CentralPanel::default().show(ui, |ui| {
                            appearance(
                                ui,
                                settings,
                                theme,
                                &skins,
                                system_theme_ok,
                                resolved,
                                &mut outbox,
                            );
                        });
                    },
                );
                // shapes move 出前清 textures delta(直接 drop 带 delta 会 panic)
                let mut output = output;
                output.textures_delta.clear();
                last_shapes = output.shapes;
                last_outbox = outbox;
            }

            // 精确匹配:滑杆标签是独立 Label 形状,值是独立 DragValue 形状,
            // 二者的 galley 文本恰等于「字号」/「行距」/数值本身;区块标题
            // (整句「排版(…)」)不会以纯标签形态出现,contains 会撞车,
            // 这里用 trim 后全等
            let text_shapes: Vec<&str> = last_shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Text(t) => Some(t.galley.text().trim()),
                    _ => None,
                })
                .collect();
            for label in ["字号", "行距"] {
                assert!(
                    text_shapes.contains(&label),
                    "{font_size}/{line_height}: 滑杆标签「{label}」未渲出,文本形状:{text_shapes:?}"
                );
            }
            for value in [format!("{font_size}"), format!("{line_height}")] {
                assert!(
                    text_shapes.contains(&value.as_str()),
                    "{font_size}/{line_height}: 回显值「{value}」未渲出,文本形状:{text_shapes:?}"
                );
            }
            // 轨道:横向细长填充矩形(与 #35 同款判据),主题区分隔线等同类
            // 形状只会多算不会少算,断言 ≥2
            use egui::epaint::Shape;
            let rails = last_shapes
                .iter()
                .filter(|clipped| {
                    matches!(&clipped.shape, Shape::Rect(r)
                        if r.rect.height() <= 12.0 && r.rect.width() > 40.0)
                })
                .count();
            assert!(
                rails >= 2,
                "{font_size}/{line_height}: 滑杆轨道不足 2 根({rails})"
            );
            assert!(
                last_outbox.is_empty(),
                "无拖动的渲染帧不产出消息(值只在 changed() 时发)"
            );
        }
    }

    /// #70 M1:设置窗外壳随明暗主题。明/暗两轮各走真实 `theme.apply`
    /// 投影后渲整窗,对 tessellate 后的**最终覆盖色**做像素采样(#53
    /// split-diff / preview_pixel_acceptance 手法,羽化关):分页列空带露
    /// `sidebar` 色,底部按钮条露窗底 `content` 色(其 frame 透明),中央
    /// 内容区多数采样点露 `content` 色(其余被文字/控件墨迹占据)。
    /// 两主题的采样色互不相同 —— 钉住「不写死任何一档」。
    #[test]
    fn settings_shell_follows_light_and_dark_themes() {
        use crate::preview_pixel_acceptance::{color_dist, final_covered_color};
        use egui::epaint::Mesh;

        let mut tabs_colors = Vec::new();
        let mut footer_colors = Vec::new();
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let mut state = State::default();
            state.settings.open = true;
            state.theme.mode = mode;
            let ctx = egui::Context::default();
            // 关羽化(#41/#53 同款):透明渐变边缘会污染采样读色
            ctx.options_mut(|o| o.tessellation_options.feathering = false);
            state.theme.apply(&ctx, mode);
            let system_theme_ok = state.system_theme_ok;
            let mut now = 0.0_f64;
            let mut last_shapes = Vec::new();
            for _ in 0..5 {
                now += 0.1; // Area fade-in 时钟:不给 time 首帧内容整体 noop(无头老坑)
                let State {
                    settings,
                    ai_key,
                    ai,
                    mcp,
                    bed,
                    keymap,
                    theme,
                    skins,
                    ..
                } = &mut state;
                let mut outbox = Vec::new();
                let mut output = ctx.run_ui(
                    RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::pos2(0.0, 0.0),
                            egui::vec2(1200.0, 800.0),
                        )),
                        time: Some(now),
                        ..Default::default()
                    },
                    |ui| {
                        dialog(
                            ui,
                            settings,
                            theme,
                            skins,
                            system_theme_ok,
                            mode,
                            keymap,
                            ai,
                            ai_key,
                            mcp,
                            bed,
                            &mut outbox,
                        );
                    },
                );
                output.textures_delta.clear();
                last_shapes = output.shapes;
            }
            let window = ctx
                .memory(|memory| memory.area_rect(egui::Id::new("settings-dialog")))
                .expect("五帧后设置窗 Area 状态在场");
            let primitives = ctx.tessellate(last_shapes, 1.0);
            let shell = crate::theme::shell_tokens(matches!(mode, ThemeMode::Dark));
            let sample = |p: egui::Pos2| {
                // 带 scissor 的采样器(与 PR#169 独立同修):epaint 的
                // tessellate 只对矩形做粗剔除(tessellator.rs
                // `tessellate_rect`),真裁剪在渲染器侧按
                // `ClippedPrimitive::clip_rect` 下 scissor。直接摊平 mesh
                // 采样会「看穿」滚动视口 —— 2026-10-10 起,卡片是大块
                // 不透明矩形,第一块跨过折叠线就暴露了这个口径差。
                // ScrollArea meshes extend outside its viewport. Sample only
                // the primitives whose scissor includes the point, like wgpu.
                let meshes: Vec<&Mesh> = primitives
                    .iter()
                    .filter(|cp| cp.clip_rect.contains(p))
                    .filter_map(|cp| match &cp.primitive {
                        egui::epaint::Primitive::Mesh(mesh) => Some(mesh),
                        _ => None,
                    })
                    .collect();
                final_covered_color(&meshes, p)
                    .unwrap_or_else(|| panic!("{mode:?}:采样点 {p:?} 无覆盖(窗体未渲染或坐标出窗)"))
            };

            // 分页列:五枚分页按钮(约 5×28+间距)以下的空带,露 sidebar 底色
            let tabs = sample(egui::pos2(
                window.left() + crate::ui::tokens::SETTINGS_TABS_W * 0.5,
                window.bottom() - crate::ui::tokens::SETTINGS_FOOTER_H - 8.0,
            ));
            assert!(
                color_dist(tabs, shell.sidebar) <= 2,
                "{mode:?}:分页列应露 sidebar 底色,{tabs:?} vs {:?}",
                shell.sidebar
            );

            // 底部按钮条:frame 透明,窗底 content 色透出;采样点取条带中部
            // 左半(远离右缘「关闭」按钮与条带顶部的分隔线)
            let footer = sample(egui::pos2(
                window.left() + window.width() * 0.25,
                window.bottom() - crate::ui::tokens::SETTINGS_FOOTER_H * 0.5,
            ));
            assert!(
                color_dist(footer, shell.content) <= 2,
                "{mode:?}:按钮条应露窗底 content 色,{footer:?} vs {:?}",
                shell.content
            );

            // 中央内容区:6×8 网格采样,多数点露窗底 content 色(卡片之间
            // 的空隙)或卡片底 faint 色(2026-10-10 起,卡片化后内容区大部分被卡片
            // 覆盖)。两色都出自同一套明暗 shell 投影 —— 若窗底写死暗色,
            // 浅色轮这两档都会大面积偏黑,这里的多数派断言当场红
            let left = window.left() + crate::ui::tokens::SETTINGS_TABS_W + 24.0;
            let right = window.right() - 24.0; // 避开右缘滚动条
            let top = window.top() + 64.0; // 避开标题栏与页头
            let bottom = window.bottom() - crate::ui::tokens::SETTINGS_FOOTER_H - 16.0;
            let mut hits = 0;
            let mut total = 0;
            for iy in 0..6 {
                for ix in 0..8 {
                    let t = |n: usize, max: usize| (n as f32 + 0.5) / max as f32;
                    let p = egui::pos2(
                        left + (right - left) * t(ix, 8),
                        top + (bottom - top) * t(iy, 6),
                    );
                    if p.y < bottom {
                        total += 1;
                        let c = sample(p);
                        if color_dist(c, shell.content) <= 2
                            || color_dist(c, shell.faint) <= 2
                            || (cfg!(target_os = "macos")
                                && color_dist(
                                    c,
                                    crate::theme::window_fill(matches!(mode, ThemeMode::Dark)),
                                ) <= 2)
                        {
                            hits += 1;
                        }
                    }
                }
            }
            assert!(
                total > 0 && hits * 2 > total,
                "{mode:?}:内容区 {hits}/{total} 点露 content/faint 色,窗底疑似未随主题"
            );

            tabs_colors.push(tabs);
            footer_colors.push(footer);
        }
        assert_ne!(tabs_colors[0], tabs_colors[1], "明暗两档分页列底色应不同");
        assert_ne!(footer_colors[0], footer_colors[1], "明暗两档窗底色应不同");
    }

    /// 关闭时窗口不进入绘制路径(不 panic、不改 open 标志)。
    #[test]
    fn closed_dialog_renders_nothing() {
        let mut state = State::default();
        assert!(!state.settings.open);
        render(&mut state);
        assert!(!state.settings.open, "渲染不翻转开关");
    }

    /// #55 M2:外观页「编辑器」分区真实渲出 minimap 复选框的标签文本
    /// (与 #23 滑杆测试同款:直接渲 `appearance`,避开整窗 440px 视口
    /// 对 ScrollArea 的裁剪);两轮回显值(开/关)证明复选框绑定在
    /// theme 字段上,无交互帧不产出消息(翻转只经 changed() 走归约)。
    #[test]
    fn appearance_page_renders_minimap_toggle_bound_to_theme() {
        let mut state = State::default();
        let ctx = egui::Context::default();
        state.theme.apply(&ctx, state.theme.mode);

        for show in [true, false] {
            state.theme.show_minimap = show;
            let skins = state.skins.clone();
            let mut last_shapes = Vec::new();
            let mut last_outbox = Vec::new();
            let system_theme_ok = state.system_theme_ok;
            let resolved = resolved_theme_for_test(&state);
            for _ in 0..3 {
                let State {
                    settings, theme, ..
                } = &mut state;
                let mut outbox = Vec::new();
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::pos2(0.0, 0.0),
                            egui::vec2(1200.0, 800.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            appearance(
                                ui,
                                settings,
                                theme,
                                &skins,
                                system_theme_ok,
                                resolved,
                                &mut outbox,
                            );
                        });
                    },
                );
                let mut output = output;
                output.textures_delta.clear();
                last_shapes = output.shapes;
                last_outbox = outbox;
            }
            let texts: Vec<&str> = last_shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Text(t) => Some(t.galley.text().trim()),
                    _ => None,
                })
                .collect();
            assert!(
                texts.contains(&if cfg!(target_os = "macos") {
                    "缩略导航"
                } else {
                    "Minimap"
                }),
                "show={show}: 两列行标签「Minimap」未渲出,文本形状:{texts:?}"
            );
            assert!(last_outbox.is_empty(), "show={show}: 无交互帧不产出消息");
        }
    }

    /// 主题预览图按模式画各自的纸面(2026-10-10 坤哥实机报告的回归:
    /// 深色缩略图曾误画浅色纸 —— `paper(thumb, false)` 写死所致)。
    /// 判据:暗色 content 纸面矩形 ≥2 块(「深色」整张 + 「跟随系统」
    /// 右半),浅色纸面同理 ≥2 块;行墨不走当前主题 border(深色缩略图
    /// 上浅灰墨才是对的,这里数 dark border 矩形 ≥2 同步钉住)。
    #[test]
    fn theme_previews_render_each_mode_with_its_own_paper() {
        // 直渲 appearance_legacy:预览图住在非 mac 的 legacy 页,macOS 上
        // `appearance` 分发到 macos::appearance(原生页无预览),走分发器
        // 会在 mac CI 数到 0 块暗纸(#177 实测)
        let mut state = State::default();
        let ctx = egui::Context::default();
        state.theme.apply(&ctx, state.theme.mode);
        let skins = state.skins.clone();
        let system_theme_ok = state.system_theme_ok;
        let resolved = resolved_theme_for_test(&state);
        let mut last_shapes = Vec::new();
        for _ in 0..2 {
            let State {
                settings, theme, ..
            } = &mut state;
            let mut outbox = Vec::new();
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(1200.0, 800.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        appearance_legacy(
                            ui,
                            settings,
                            theme,
                            &skins,
                            system_theme_ok,
                            resolved,
                            &mut outbox,
                        );
                    });
                },
            );
            let mut output = output;
            output.textures_delta.clear();
            last_shapes = output.shapes;
        }
        let dark = crate::theme::shell_tokens(true);
        let light = crate::theme::shell_tokens(false);
        let count = |want: egui::Color32| {
            last_shapes
                .iter()
                .filter(|clipped| {
                    matches!(&clipped.shape, egui::epaint::Shape::Rect(r) if r.fill == want)
                })
                .count()
        };
        assert!(
            count(dark.content) >= 2,
            "暗色纸面矩形应 ≥2(深色整张+跟随系统右半),实测 {}",
            count(dark.content)
        );
        assert!(
            count(dark.sidebar) >= 2,
            "暗色侧栏条应 ≥2,实测 {}",
            count(dark.sidebar)
        );
        assert!(
            count(dark.border) >= 2,
            "暗色行墨应 ≥2(深色缩略图上的三行示意),实测 {}",
            count(dark.border)
        );
        assert!(
            count(light.content) >= 2,
            "浅色纸面矩形应 ≥2(浅色整张+跟随系统左半),实测 {}",
            count(light.content)
        );
    }

    /// #57 M2:外观页真实渲出禅定导航三态选择(与 minimap 测试同款:直接
    /// 渲 `appearance`,避开整窗 440px 视口对 ScrollArea 的裁剪)。三轮回显
    /// (悬停/常显/关)里三态标签**每轮都在场**——选择器不随选中值增删
    /// 选项,只换高亮;无交互帧不产出消息(切换只经 clicked() 走归约)。
    #[test]
    fn appearance_page_renders_zen_nav_selector_bound_to_theme() {
        let mut state = State::default();
        let ctx = egui::Context::default();
        state.theme.apply(&ctx, state.theme.mode);

        for mode in ZenNavMode::ALL {
            state.theme.zen_nav = mode;
            let skins = state.skins.clone();
            let mut last_shapes = Vec::new();
            let mut last_outbox = Vec::new();
            let system_theme_ok = state.system_theme_ok;
            let resolved = resolved_theme_for_test(&state);
            for _ in 0..3 {
                let State {
                    settings, theme, ..
                } = &mut state;
                let mut outbox = Vec::new();
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::pos2(0.0, 0.0),
                            egui::vec2(1200.0, 800.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            appearance(
                                ui,
                                settings,
                                theme,
                                &skins,
                                system_theme_ok,
                                resolved,
                                &mut outbox,
                            );
                        });
                    },
                );
                let mut output = output;
                output.textures_delta.clear();
                last_shapes = output.shapes;
                last_outbox = outbox;
            }
            let texts: Vec<&str> = last_shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Text(t) => Some(t.galley.text().trim()),
                    _ => None,
                })
                .collect();
            for label in ZenNavMode::ALL.map(|mode| mode.label()) {
                assert!(
                    texts.contains(&label),
                    "{mode:?}: 三态选项「{label}」未渲出,文本形状:{texts:?}"
                );
            }
            assert!(last_outbox.is_empty(), "{mode:?}: 无交互帧不产出消息");
        }
    }

    /// 页签图标与显示名一一对应,且都有图标(不出现空图标页)。
    #[test]
    fn every_tab_has_label_and_icon() {
        for tab in SettingsTab::ALL {
            assert!(!tab.label().is_empty());
            let _ = tab.icon();
        }
    }

    /// AI 页草稿:默认等于出厂配置;OpenAI 兼容/Anthropic「需要 key」为真
    /// (驱动参数区可用与命令闸门);Ollama 不要 key 但参数照常参与。
    #[test]
    fn ai_draft_defaults_and_provider_switch() {
        let mut settings = SettingsState::default();
        assert_eq!(settings.ai_draft, AiConfig::default());
        assert!(!settings.ai_draft.provider.requires_key());
        for kind in [ProviderKind::OpenAiCompatible, ProviderKind::Anthropic] {
            settings.ai_draft.provider = kind;
            assert!(kind.requires_key());
            assert!(kind.uses_settings());
        }
        settings.ai_draft.provider = ProviderKind::Ollama;
        assert!(!settings.ai_draft.provider.requires_key());
        assert!(settings.ai_draft.provider.uses_settings());
    }

    /// MCP 页草稿:默认与 `McpConfig::default` 一致(关闭 + 默认端口 + 五
    /// 工具全开);关掉一个工具后 draft 立即反映(保存才生效到服务)。
    #[test]
    fn mcp_draft_defaults_and_tool_toggle() {
        let mut settings = SettingsState::default();
        assert_eq!(settings.mcp_draft, McpConfig::default());
        assert!(!settings.mcp_draft.enabled);
        settings
            .mcp_draft
            .tools
            .insert(ToolKind::GitStatus.name().to_owned(), false);
        assert!(!settings.mcp_draft.tool_enabled(ToolKind::GitStatus));
        assert!(settings.mcp_draft.tool_enabled(ToolKind::SearchDocs));
    }

    /// MCP 页在「未设根」的初始态下渲染不 panic,并给出警示文案(该页
    /// 是唯一会把「根缺失」摆到台面上的地方)。
    #[test]
    fn mcp_page_renders_without_root() {
        let ctx = egui::Context::default();
        let mut settings = SettingsState::default();
        let mcp = McpState::default();
        let mut outbox = Vec::new();
        let output = ctx.run_ui(RawInput::default(), |ui| {
            mcp_page(ui, &mut settings, &mcp, &mut outbox);
        });
        output.drop_without_applying_deltas();
        assert!(outbox.is_empty(), "渲染不产出消息");
    }

    /// 快捷键页的捕获状态是纯 UI 关注点:点一次进入捕获,再点一次退出。
    #[test]
    fn capture_toggles_in_place() {
        let ctx = egui::Context::default();
        let mut settings = SettingsState::default();
        let mut outbox = Vec::new();
        let keymap = Keymap::builtin();
        let mut rect = egui::Rect::NOTHING;

        let output = ctx.run_ui(RawInput::default(), |ui| {
            settings.capture = Some(Command::Save);
            keymap_page(ui, &mut settings, &keymap, &mut outbox);
            // 拿第一行键位按钮的位置:复用同一渲染路径的第二帧定位
            rect = ui.min_rect();
        });
        output.drop_without_applying_deltas();
        assert!(settings.capture.is_some(), "渲染不改变捕获状态");
        let _ = rect;
    }

    /// 快捷键页对快速打开(#24 C2)可见可改绑:整页按 `Command::ALL`
    /// 遍历渲染一帧不 panic,QuickOpen 行因出厂有绑定而显示键位文本
    /// (非「未绑定」占位)。
    #[test]
    fn keymap_page_renders_with_quick_open_bound() {
        let keymap = Keymap::builtin();
        assert!(
            keymap.get(Command::QuickOpen).is_some(),
            "QuickOpen 出厂有绑定,行上显示键位而非「未绑定」"
        );
        let ctx = egui::Context::default();
        let mut settings = SettingsState::default();
        let mut outbox = Vec::new();
        ctx.run_ui(RawInput::default(), |ui| {
            keymap_page(ui, &mut settings, &keymap, &mut outbox);
        })
        .drop_without_applying_deltas();
        assert!(outbox.is_empty(), "渲染不产出消息");
    }

    /// 快捷键行的键位文本:已绑定显示组合键,未绑定显示「未绑定」。
    #[test]
    fn shortcut_text_shows_binding_or_placeholder() {
        let keymap = Keymap::builtin();
        assert!(keymap.get(Command::Save).is_some());
        let mut cleared = keymap.clone();
        cleared.set(Command::Save, None);
        assert_eq!(cleared.get(Command::Save), None);

        // 平台化文本能被反解(改键后持久化-重载闭环的前提)
        let shortcut = Shortcut {
            modifiers: egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            key: egui::Key::K,
        };
        assert_eq!(
            crate::keymap::parse_shortcut(&shortcut.platform_text()),
            Some(shortcut)
        );
    }

    /// 请求头多行文本 ↔ 结构对的往返:空行跳过、值里的 `${TOKEN}` 原样
    /// 保留、缺 `:` 或空名字报行号。
    #[test]
    fn headers_text_round_trips() {
        let headers = vec![
            ("Authorization".to_owned(), "Bearer ${TOKEN}".to_owned()),
            ("Accept".to_owned(), "application/json".to_owned()),
        ];
        let text = headers_to_text(&headers);
        assert_eq!(
            text,
            "Authorization: Bearer ${TOKEN}\nAccept: application/json"
        );
        assert_eq!(headers_from_text(&text).unwrap(), headers);
        // 空行与首尾空白宽容
        assert_eq!(
            headers_from_text("\n  A:  x  \n\n").unwrap(),
            vec![("A".to_owned(), "x".to_owned())]
        );
        // 坏行报行号(1 起)
        assert!(headers_from_text("A: 1\n没冒号的行")
            .unwrap_err()
            .contains("第 2 行"));
        assert!(headers_from_text(": v").unwrap_err().contains("名字为空"));
    }

    /// 图片页整页渲染(空列表 / 有 profile + 测试回显两态)不 panic、不产
    /// 消息;编辑草稿的 Grid 表单也在其中。
    #[test]
    fn image_page_renders_without_messages() {
        let ctx = egui::Context::default();
        let mut settings = SettingsState::default();
        let mut bed = crate::bed::BedState::default();
        let mut outbox = Vec::new();
        ctx.run_ui(RawInput::default(), |ui| {
            image_page(ui, &mut settings, &mut bed, &mut outbox);
        })
        .drop_without_applying_deltas();
        assert!(outbox.is_empty(), "渲染不产出消息");

        let mut smms = latermd_bed::BedProfile::preset_smms();
        smms.id = "p-1".to_owned();
        bed.profiles = vec![smms];
        bed.last_test = Some(("SM.MS".to_owned(), Ok("https://cdn/x.png".to_owned())));
        settings.bed_draft = Some(BedDraft {
            profile: latermd_bed::BedProfile::preset_custom(),
            headers_text: "Authorization: Bearer ${TOKEN}".to_owned(),
            token: String::new(),
        });
        ctx.run_ui(RawInput::default(), |ui| {
            image_page(ui, &mut settings, &mut bed, &mut outbox);
        })
        .drop_without_applying_deltas();
        assert!(outbox.is_empty(), "回显态同样只渲染");
    }
}
