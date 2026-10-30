//! 主题系统(P0 批次 A + P2.5 批次 B):Light/Dark/跟随系统 三态 + 皮肤文件 + 密度。
//!
//! [`ThemeSettings`] 是主题的唯一事实源,向三处投影(`ThemeSettings::apply`):
//!
//! - **外壳**:`egui::Context::set_theme` 切换 egui 自带的 light/dark 双套
//!   `Style`/`Visuals`(egui 0.36 按主题各持一份,面板/控件全部跟随);
//! - **外壳 token**:密度(标准/紧凑)改写 spacing 与圆角 —— 批次 B 的视觉
//!   打磨落点；皮肤色板统一覆盖全应用，不做每控件粒度自定义;
//! - **编辑器**:字号(#23 F3)投 `TextStyle::Monospace` 档;行距(#50 M2)
//!   投 `spacing.extra_text_line_spacing`(TextEdit 行盒的绝对像素加值,
//!   `max(0, 字号×行距 − 自然行高)`,自然行高经 `ctx.fonts` 现算);
//! - **正文**:vendored `MarkdownStyle` 装进 context 默认槽
//!   (`egui_markdown_style::set_style`),其颜色字段本就成对设计
//!   (`color_dark`/`color_light`),渲染时按 `ui.visuals().dark_mode` 自动取值;
//!   代码高亮不指定 theme 时同样按 dark/light 自动选择。
//!
//! 联动零额外成本:MarkdownStyle 布局缓存 hash 已含 dark_mode,切换自动失效。
//!
//! 三态里的 `System` 由**调用方**解析(见 [`detect_system_mode`] 与
//! `crate::state` 的轮询):本模块不持有检测结果 —— 主题检测要查系统设置
//! (Linux 走 dbus),不该每帧发生。
//!
//! 持久化:平台配置目录下手写路径(不引目录库),JSON 经 serde 往返;皮肤
//! 文件则是同目录下 `themes/*.ron`(皮肤可手写、可分享,故用 RON 而非 JSON:
//! 支持注释与无引号键名)。

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use eframe::egui::Color32;
use egui_markdown_style::MarkdownStyle;
use serde::{Deserialize, Serialize};

use crate::ui::tokens;

/// 配置文件名,落在平台配置目录下。
const SETTINGS_FILE: &str = "settings.json";

/// 皮肤目录名；新 RON 包含正文和应用色板，兼容旧 MarkdownStyle 文件。
pub const THEMES_DIR: &str = "themes";

/// 明暗模式(用户的选择,含「跟随系统」这一非确定值)。serde 小写
/// (`"light"`/`"dark"`/`"system"`);默认浅色,启动首帧前投影主题避免闪变。
///
/// `System` **不是**一种可渲染的模式:绘制前必须经 [`ThemeMode::resolve`]
/// 落到 Light/Dark(roadmap 风险 #8:Linux 无统一规范,检测可能失灵,届时
/// 由 resolve 的 `detected: None` 分支回落)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    /// 浅色。
    #[default]
    Light,
    /// 深色。
    Dark,
    /// 跟随系统(Windows 注册表 / macOS NSUserDefaults / Linux freedesktop
    /// portal)。检测不到时按 `fallback` 走。
    System,
}

impl ThemeMode {
    /// 设置页里的可选项顺序。
    pub const ALL: [ThemeMode; 3] = [Self::Light, Self::Dark, Self::System];

    /// 设置页显示名。
    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "浅色",
            Self::Dark => "深色",
            Self::System => "跟随系统",
        }
    }

    /// 把模式解析成一个**可渲染**的明暗值:定向选择原样返回;`System` 取
    /// 检测结果,`detected` 为 `None`(未检测或检测失败)时回落 `fallback`。
    ///
    /// `fallback` 由调用方传最近一次成功的检测值(没有则给出厂默认),这样
    /// 一次检测失败不会让界面在深浅之间跳变。
    pub fn resolve(self, detected: Option<ThemeMode>, fallback: ThemeMode) -> ThemeMode {
        match self {
            Self::Light => Self::Light,
            Self::Dark => Self::Dark,
            Self::System => match detected {
                Some(Self::Light) | Some(Self::Dark) => detected.unwrap_or(fallback),
                // None 与 System 本身都不是可渲染值
                _ => match fallback {
                    Self::Light | Self::Dark => fallback,
                    _ => Self::Dark,
                },
            },
        }
    }

    /// 反向模式:「切换主题」命令用;设置页的定向选择直接给目标模式。
    /// `System` 的反向由调用方按当前解析结果决定(见 `crate::state`)。
    pub fn opposite(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark | Self::System => Self::Light,
        }
    }
}

/// 探测系统当前是深是浅。
///
/// 失败与 `Mode::Unspecified` 都返回 `None` —— Linux 上没有统一规范
/// (GNOME gsettings / KDE / portal 各异),拿不到答案时**如实返回不知道**,
/// 由调用方回落到上一次的手动选择,不做猜测。
pub fn detect_system_mode() -> Option<ThemeMode> {
    match dark_light::detect() {
        Ok(dark_light::Mode::Dark) => Some(ThemeMode::Dark),
        Ok(dark_light::Mode::Light) => Some(ThemeMode::Light),
        // Unspecified:系统给了「未指定」;Err:dbus/注册表不可用
        Ok(dark_light::Mode::Unspecified) | Err(_) => None,
    }
}

/// 界面密度(批次 B 的视觉打磨项之一;2026-09-29 坤哥指令改名换档:
/// 原「紧凑」升格为新「标准」并成为默认,原「标准」改叫「宽松」)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    /// 宽松:原「标准」档,1.0 间距与控件高度。serde 值沿用 `standard`,
    /// 已落盘的 settings.json 无需迁移(语义映射:旧标准→宽松,观感不变)。
    Standard,
    /// 标准:原「紧凑」档,更窄的项间距、更小的按钮内边距与圆角,
    /// 长文档一屏多看几行 —— 现在的出厂默认。
    #[default]
    Compact,
}

impl Density {
    /// 设置页可选项顺序(显示顺序:宽松 → 标准)。
    pub const ALL: [Density; 2] = [Self::Standard, Self::Compact];

    pub fn label(self) -> &'static str {
        match self {
            Self::Standard => "宽松",
            Self::Compact => "标准",
        }
    }
}

/// 标签条标题宽度显示模式(#37 右键菜单「缩短标题/完整标题」,整条
/// 标签条统一生效):`Short` 像浏览器标签一样**实际收窄 chip 宽**并按
/// Unicode 字符边界加省略号;`Full` 按完整标题测宽,放不下沿既有单行
/// 水平滚动(#11 口径)。纯显示偏好 —— 切换不动任何标签的路径/缓冲/
/// dirty,也不改盘上文件名。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TitleWidthMode {
    /// 缩短:chip 收窄到标签条可用空间,保留最小宽与可点的关闭按钮。
    Short,
    /// 完整:按完整标题测宽。默认值 —— 即 #11 已交付的既有观感,升级
    /// 不改变默认行为,缩短模式由用户显式开启。
    #[default]
    Full,
}

impl TitleWidthMode {
    /// 设置页可选项顺序(缩短 → 完整,与标签条右键菜单条目同序)。
    pub const ALL: [TitleWidthMode; 2] = [Self::Short, Self::Full];

    /// 右键菜单条目文案(菜单显示名与本枚举同源,不再各写一份字面量)。
    pub fn label(self) -> &'static str {
        match self {
            Self::Short => "缩短标题",
            Self::Full => "完整标题",
        }
    }
}

/// 禅定模式左缘标签导航的显示方式(#57 M2 三态配置):悬停唤出 / 常显 /
/// 关闭。纯显示偏好,只影响禅定帧(`draw_zen`),非禅定渲染零关联。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZenNavMode {
    /// 悬停唤出(#57 M1 既有行为):指针移近左缘唤出,离开去抖后隐藏。
    /// 默认值 —— 升级不改变默认行为,常显/关闭由用户显式开启(取舍登记
    /// decisions-pending #107)。
    #[default]
    Hover,
    /// 常显:进入禅定即显示导航列,不随指针移动隐藏。与悬停共用同一渲染
    /// 件,只是显隐条件不同(恒真,无感应区判定、无去抖)。
    Always,
    /// 关闭:完全不渲染 —— 零路径早退,不进任何绘制分支(无形状无命中)。
    Off,
}

impl ZenNavMode {
    /// 设置页可选项顺序(悬停 → 常显 → 关闭)。
    pub const ALL: [ZenNavMode; 3] = [Self::Hover, Self::Always, Self::Off];

    /// 设置页显示名。
    pub fn label(self) -> &'static str {
        match self {
            Self::Hover => "悬停唤出",
            Self::Always => "常显",
            Self::Off => "关闭",
        }
    }
}

/// 左栏外壳样式(mac 专项,2026-10-24 坤哥指令):工作台(mac 原生 chrome
/// 搭配的「工作区下拉 + 文字页签」顶部导航)与图标栏(左缘 rail 五视图,
/// Windows/Linux 外壳同款)。
///
/// **仅 macOS 生效**:其余平台的外壳本就是图标栏,该字段在那些平台上不
/// 改变任何行为(设置行也只在 mac 渲染)。出厂默认随平台:mac 保持工作台
/// (升级零变化,2026-10-10 #166 引入的既定形态),其他平台默认图标栏;
/// 用户显式选择后照 settings.json 走。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarStyle {
    /// 工作台:顶部工作区下拉 + 大纲/文件/搜索文字页签。
    #[cfg_attr(target_os = "macos", default)]
    Workbench,
    /// 图标栏:左缘 rail 图标轨道,与其他平台一致。
    #[cfg_attr(not(target_os = "macos"), default)]
    Rail,
}

impl SidebarStyle {
    /// 设置页可选项顺序(工作台 → 图标栏,当前默认在前)。
    pub const ALL: [SidebarStyle; 2] = [Self::Workbench, Self::Rail];

    /// 设置页显示名。
    pub fn label(self) -> &'static str {
        match self {
            Self::Workbench => "工作台",
            Self::Rail => "图标栏",
        }
    }
}

/// 启动时的默认视图模式(写作 / 源码 / 对照 / 上次选择)。作用于启动装载
/// (`State::restore_layout`)与新建文档(`FileCmd::New`);会话内的模式切换
/// 仍实时记进布局存档,供「上次选择」档恢复。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DefaultViewMode {
    /// 每次启动/新建进入写作(Live 富渲染)。
    Live,
    /// 每次启动/新建进入源码。
    Source,
    /// 每次启动/新建进入对照(源码 + 右侧只读列)。
    Split,
    /// 启动恢复上次退出时的视图;新建取创建时刻的会话档(出厂默认 ——
    /// 与既有行为一致:布局存档没记模式时回落写作)。升级用户不被默默
    /// 改行为,换档是显式选择。
    #[default]
    Last,
}

impl DefaultViewMode {
    /// 设置页可选项顺序(写作 → 源码 → 对照 → 上次选择)。
    pub const ALL: [DefaultViewMode; 4] = [Self::Live, Self::Source, Self::Split, Self::Last];

    /// 设置页显示名(与顶栏模式切换条同词:写作/源码/对照)。
    pub fn label(self) -> &'static str {
        match self {
            Self::Live => "写作",
            Self::Source => "源码",
            Self::Split => "对照",
            Self::Last => "上次选择",
        }
    }
}

/// 排版偏好(#23)的字号取值域(pt,闭区间):下限 12 是中文可读性下限
/// (auto-plan #23),上限 24。默认 15。F2 的滑杆 range 与本常量同源。
pub const EDITOR_FONT_SIZE_MIN: f32 = 12.0;
pub const EDITOR_FONT_SIZE_MAX: f32 = 24.0;
pub const EDITOR_FONT_SIZE_DEFAULT: f32 = 15.0;

/// 排版偏好(#23)的正文行距倍率取值域(闭区间):下限 1.2,上限 2.0,
/// 默认 1.5(中文可读区间,preview-typography §2.2(3))。
pub const LINE_HEIGHT_MIN: f32 = 1.2;
pub const LINE_HEIGHT_MAX: f32 = 2.0;
pub const LINE_HEIGHT_DEFAULT: f32 = 1.5;

/// f32 偏好值钳制:`NaN` 回落默认(`f32::clamp` 对 NaN 是穿透的,这里
/// 是渲染前的最后防线);inf 与越界值钳到闭区间端点。
fn clamp_pref(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_nan() {
        fallback
    } else {
        value.clamp(min, max)
    }
}

/// 字号钳制(纯函数,单测锚点;#23):把任意手改值收进
/// [`EDITOR_FONT_SIZE_MIN`]..=[`EDITOR_FONT_SIZE_MAX`]。
pub fn clamp_editor_font_size(value: f32) -> f32 {
    clamp_pref(
        value,
        EDITOR_FONT_SIZE_MIN,
        EDITOR_FONT_SIZE_MAX,
        EDITOR_FONT_SIZE_DEFAULT,
    )
}

/// 行距倍率钳制(纯函数,单测锚点;#23):把任意手改值收进
/// [`LINE_HEIGHT_MIN`]..=[`LINE_HEIGHT_MAX`]。
pub fn clamp_line_height(value: f32) -> f32 {
    clamp_pref(value, LINE_HEIGHT_MIN, LINE_HEIGHT_MAX, LINE_HEIGHT_DEFAULT)
}

/// 主题设置:模式 + 皮肤 + 密度 + 排版偏好 + 正文样式覆盖。缺省字段(含
/// 整个 `overrides`)回落默认,手改的配置文件缺项不致整体解析失败。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSettings {
    /// 明暗模式(外壳与正文共同跟随)。
    pub mode: ThemeMode,
    /// 选中的皮肤文件名(不含扩展名);`None` = 用 `overrides` 或出厂默认。
    pub skin: Option<String>,
    /// 界面密度。
    pub density: Density,
    /// 标签条标题宽度模式(#37)。落 settings.json 与主题同路;旧文件缺
    /// 该项由 `#[serde(default)]` 回落 `Full`(完整,既有观感)。
    pub tab_title_width: TitleWidthMode,
    /// 编辑器与预览正文的**基准字号**(pt,#23):标题按比例放大,本字段
    /// 只定正文基准。有效范围 12..=24(闭区间,CJK 可读性下限),默认 15。
    /// 生效链路(F3):编辑器经 `apply` 的 `apply_font_size` 投影到
    /// `TextStyle::Monospace` 档;写作渲染(Live 富渲染块 —— 三态视图的
    /// 对照列就是同一渲染的只读档,故只有这一条链路)经 `crate::live::ui`
    /// 的显式 `FontId`(size 取自 [`editor_font_size`],标题按 `heading.scales`
    /// 比例放大,行高随字号重算)。
    pub editor_font_size: f32,
    /// 正文行距倍率(#23),有效范围 1.2..=2.0(闭区间),默认 1.5。
    /// 是**用户偏好**:预览侧 `apply` 在皮肤/overrides 之上再覆盖 vendored
    /// `MarkdownStyle::line_height_ratio`(出厂 1.30,preview-typography
    /// §1.2 的「正文观感不变」口径,见 F3 的行距覆盖决策),用户调低
    /// (如 1.2)时 `effective_markdown_style` 的 CJK 行高下限(本机
    /// CJK face 行高,约 1.448em)在更下层兜底;编辑器侧(#50 M2,坤哥
    /// 2026-10-02 反馈「每行间距也太小」,decisions-pending #73 方案②
    /// 转正)经 `apply_font_size` 投影成 `spacing.extra_text_line_spacing`
    /// 的绝对像素加值,调低时被自然行高 clamp 到 0。
    pub line_height: f32,
    /// 源码模式右缘 minimap 开关(#55 M2):全局偏好,照 #23 排版偏好在
    /// `ThemeSettings` 落 settings.json 的先例(旧档缺字段由 struct 级
    /// `#[serde(default)]` 兜底)。默认**开**(与 VS Code 等编辑器出厂
    /// 一致,取舍登记 decisions-pending #105);关闭时编辑器走与从前
    /// 逐字节相同的路径(零 minimap 元素,ui::editor 的否决线)。
    pub show_minimap: bool,
    /// 短文或窄栏自动隐藏缩略图；关闭此项可始终显示。
    pub minimap_auto: bool,
    /// 打字机模式开关(#64 M1):开启后光标所在行滚动保持视口 1/3 线
    /// (源码与 Live 两模式;全局偏好,照 #55 minimap 开关同款通路落
    /// settings.json,旧档缺字段由 struct 级 `#[serde(default)]` 兜底)。
    /// 默认**关** —— 它改变滚动行为,出厂不替用户决定;关闭时编辑器滚动
    /// 路径与从前逐字节相同(否决线,decisions-pending #121)。
    pub show_typewriter: bool,
    /// 专注模式开关(#64 M2):开启后 Live 模式淡化非活动块(遮罩纯绘制,
    /// 照 #81/#55 通路落 settings.json,旧档缺字段由 struct 级
    /// `#[serde(default)]` 兜底)。默认**关** —— 强视觉改变,出厂不替用户
    /// 决定;关闭时 Live 绘制路径与从前逐像素相同(否决线,
    /// decisions-pending #122)。仅 Live 模式生效:源码是单个 TextEdit,
    /// 分段淡化不接线(边界同登记 #122)。
    pub show_focus_mode: bool,
    /// 选区 AI 浮标开关(#61 M1 的配置面):选中文字后在选区尾端浮出
    /// 「AI 续写 / AI 润色」小标识。全局偏好,照 #55 minimap 开关同款通路
    /// 落 settings.json(旧档缺字段由 struct 级 `#[serde(default)]` 兜底)。
    /// 默认**开** —— 浮标是选区 AI 的唯一鼠标入口;关闭后同样两个动作仍
    /// 可从右键菜单发起,浮标与命中区一个形状都不注册(走 show 的隐藏
    /// 路径,与无选区同口径清探针)。
    pub show_selection_ai: bool,
    /// 禅定模式左缘标签导航的显示方式(#57 M2):全局偏好,照 #55 minimap
    /// 开关先例落 settings.json(旧档缺字段由 struct 级 `#[serde(default)]`
    /// 兜底回悬停)。关闭档在禅定帧走零路径(不进任何绘制分支)。
    pub zen_nav: ZenNavMode,
    /// 左栏外壳样式(mac 专项,见 [`SidebarStyle`]):全局偏好,照 #57
    /// zen_nav 同款通路落 settings.json(旧档缺字段由 struct 级
    /// `#[serde(default)]` 兜底)。出厂默认随平台(mac 工作台 / 其余图标
    /// 栏),仅在 macOS 上改变行为。
    pub sidebar_style: SidebarStyle,
    /// 启动默认视图模式(写作/源码/对照/上次选择):启动装载与新建文档
    /// 共同遵循;
    /// 会话内切档不受限。出厂「上次选择」= 既有行为(升级用户零变化);
    /// 旧 settings.json 缺字段由 struct 级 `#[serde(default)]` 兜底。
    pub default_view_mode: DefaultViewMode,
    /// 正文样式覆盖;`None` = 出厂默认(见 [`default_markdown_style`])。
    pub overrides: Option<MarkdownStyle>,
    /// Emoji 面板「最近使用」(docs/emoji-plan.md §6.3):新的在前、去重、
    /// 容量上限见 `state::EMOJI_RECENT_CAP`。落 settings.json 是 E2 的既定
    /// 路线(「与 ThemeSettings 同路」)—— 本结构即 settings.json 的载荷,
    /// 主题动作落盘时顺带持久化,不另开文件;旧文件缺该项由
    /// `#[serde(default)]` 补空。
    pub emoji_recent: Vec<String>,
    /// 当前皮肤的**内容**:由 `skin` 名字从磁盘载入,**不落盘**
    /// (避免同一份样式在 settings.json 与皮肤文件里各存一份、改了一处另一处
    /// 不跟着变)。
    #[serde(skip)]
    pub skin_style: Option<MarkdownStyle>,
    /// 应用皮肤的明暗色板，与正文一起从 RON 加载。
    #[serde(skip)]
    pub skin_shell: Option<ShellPalette>,
}

/// 手动实现而非 derive:f32 字段的派生默认只能是 0.0,而 #23 的排版偏好
/// 默认是 15.0/1.5(serde 的 struct 级 `#[serde(default)]` 缺字段时正从
/// 这里取值,旧 settings.json 的回落路径与 Rust 侧 `ThemeSettings::default()`
/// 因此同源)。枚举字段仍取各自 `#[default]`,与派生语义一致。
impl Default for ThemeSettings {
    fn default() -> Self {
        Self {
            mode: ThemeMode::default(),
            skin: None,
            density: Density::default(),
            tab_title_width: TitleWidthMode::default(),
            editor_font_size: EDITOR_FONT_SIZE_DEFAULT,
            line_height: LINE_HEIGHT_DEFAULT,
            show_minimap: true,
            minimap_auto: true,
            show_typewriter: false,
            show_focus_mode: false,
            show_selection_ai: true,
            zen_nav: ZenNavMode::default(),
            sidebar_style: SidebarStyle::default(),
            default_view_mode: DefaultViewMode::default(),
            overrides: None,
            emoji_recent: Vec::new(),
            skin_style: None,
            skin_shell: None,
        }
    }
}

/// 出厂默认正文样式:vendored 默认之上开表格边框与底色(#30)。
///
/// vendored `TableStyle` 的默认 `stroke_width = 0.0` 直接不画线、两底色
/// 开关默认关(能力在、默认关),预览表格因此长期无边框无底色。这是 app
/// 侧的默认取值决策,不动 vendored:线宽 1px,圆角与控件圆角同源
/// (`tokens::RADIUS_MD`);线色由 vendored 渲染时取
/// `widgets.noninteractive.bg_stroke.color`,表头/隔行底色取
/// `visuals.faint_bg_color`,明暗两套 visuals 自动适配,app 不另配颜色。
pub fn default_markdown_style() -> MarkdownStyle {
    // 代码块默认与正文同级可读字号；10pt 在中文和高 DPI 屏上过细。
    let mut style = MarkdownStyle {
        code_font_size: 13.0,
        ..MarkdownStyle::default()
    };
    // 代码块采用 macOS 风格的宽松内距与圆角，避免代码贴边、边框过硬。
    style.code_block.padding = [12.0, 10.0, 12.0, 10.0];
    style.code_block.corner_radius = tokens::RADIUS_MD;
    style.code_block.stroke_width = 1.0;
    // 标题上方呼吸感(2026-10-30 设计审查):vendored 出厂 4px 偏贴,
    // 提到 10px 让 H1/H2 与上一段明显拉开一档。vendored 默认不动,与
    // 代码块内距同款 app 侧覆盖;十套预设经 theme_presets::base 同参。
    style.heading_space_above = 10.0;
    style.table.stroke_width = 1.0;
    style.table.corner_radius = tokens::RADIUS_MD;
    style.table.header_fill = true;
    style.table.zebra_fill = true;
    style
}

/// 装机环境修正后的生效样式(#43 M2):把本机 CJK 回退 face 的实际行高
/// 写进 `min_line_height_em`(行高下限)。该下限是字体链的物理属性而非
/// 用户偏好,取「用户配置与物理需求较大者」—— 用户显式调高(>CJK 行高,
/// 如 1.6)完全生效,调低(如 1.2)被物理需求兜底,否则行盒装不下 CJK
/// 字形的行高需求,越界墨迹被相邻行/后续块背景遮挡(「显示不全」)。
/// 本机无 CJK 候选(或表值解析失败)时原样返回,行为与修复前一致。
pub fn effective_markdown_style(ctx: &egui::Context, mut style: MarkdownStyle) -> MarkdownStyle {
    // 旧皮肤文件保留颜色和自定义值；只迁移原先过小的出厂代码样式与出厂
    // 标题节奏(2026-10-30 起出厂 10px,旧档带着 4px 出厂值落盘的照
    // code_font_size 先例一并迁;用户手改的其他值不动)。
    if style.code_font_size == 10.0 {
        style.code_font_size = 13.0;
    }
    if style.heading_space_above == 4.0 {
        style.heading_space_above = 10.0;
    }
    if style.code_block.padding == [4.0, 6.0, 12.0, 6.0] {
        style.code_block.padding = [12.0, 10.0, 12.0, 10.0];
        style.code_block.corner_radius = tokens::RADIUS_MD;
    }
    if let Some(floor) = crate::fonts::line_height_floor_em(ctx) {
        style.min_line_height_em = style.min_line_height_em.max(floor);
    }
    style
}

impl ThemeSettings {
    /// 启动时装载:平台默认目录;首次运行(无文件)静默用默认,文件在但解析
    /// 失败则终端告警后回落默认 —— 坏配置不该挡住应用启动。
    pub fn load() -> Self {
        let Some(dir) = config_dir() else {
            return Self::default();
        };
        match Self::load_from(&dir) {
            Ok(settings) => settings,
            Err(LoadError::Missing) => Self::default(),
            Err(LoadError::Corrupt(source)) => {
                eprintln!("LaterMD: 主题配置解析失败,已回落默认: {source}");
                Self::default()
            }
        }
    }

    /// 把主题投影到 context:切 egui 主题(外壳)+ 密度 token + 字号档与
    /// 行距投影(编辑器)+ 安装正文样式(预览,含行距覆盖)。幂等且带
    /// staleness 检查,每帧调用时空闲帧近零开销。egui 0.36 中 theme 属于
    /// options 数据,`logic` 阶段写入合法(不是绘制)。
    ///
    /// `resolved` 是 [`ThemeMode::resolve`] 的结果(已把 `System` 落到确定的
    /// 明暗):本模块不做系统检测 —— 那要查 dbus/注册表,不该每帧发生。
    pub fn apply(&self, ctx: &egui::Context, resolved: ThemeMode) {
        let theme = match resolved {
            ThemeMode::Light | ThemeMode::System => egui::Theme::Light,
            ThemeMode::Dark => egui::Theme::Dark,
        };
        if ctx.theme() != theme {
            ctx.set_theme(theme);
        }
        apply_shell(ctx, self.shell_palette());
        apply_density(ctx, self.density);
        apply_font_size(
            ctx,
            clamp_editor_font_size(self.editor_font_size),
            clamp_line_height(self.line_height),
        );
        // #23 F3:行距倍率在皮肤/overrides 选出的生效样式**之上**再覆盖
        // —— 滑杆是显式用户意图,优先级高于「皮肤是一整套显式选择」。
        // 由此生效的 ratio 恒等于 `theme.line_height`(默认 1.5,vendored
        // 出厂 1.30 只是未接线时代的基线);`effective_markdown_style` 的
        // CJK 行高下限照旧兜在更下层,用户调低(如 1.2)时物理需求胜出。
        let mut wanted = self.markdown_style();
        wanted.line_height_ratio = clamp_line_height(self.line_height);
        let wanted = effective_markdown_style(ctx, wanted);
        if *egui_markdown_style::global_style(ctx) != wanted {
            egui_markdown_style::set_style(ctx, wanted);
        }
    }

    /// 应用与正文共用一次皮肤选择；旧预设按名称补齐外壳，用户文件不改写。
    pub fn shell_palette(&self) -> ShellPalette {
        self.skin_shell
            .or_else(|| {
                self.skin
                    .as_deref()
                    .and_then(crate::theme_presets::shell_palette)
            })
            .unwrap_or_default()
    }

    /// 生效的正文样式:皮肤 > `overrides` > 出厂默认([`default_markdown_style`])。
    ///
    /// 皮肤优先的理由:皮肤是用户显式选的「这一整套」,手改 settings.json 的
    /// `overrides` 是兜底通道(旧配置与手工微调),两者都出现时以显式选择为准。
    pub fn markdown_style(&self) -> MarkdownStyle {
        self.skin_style
            .clone()
            .or_else(|| self.overrides.clone())
            .unwrap_or_else(default_markdown_style)
    }

    /// 装上选中的皮肤内容;名字不在目录里时清空(皮肤被删/改名后自动回落
    /// 默认,不留在「选了一个不存在的皮肤」的状态)。
    pub fn select_skin(&mut self, name: Option<&str>, catalog: &SkinCatalog) {
        match name {
            None => {
                self.skin = None;
                self.skin_style = None;
                self.skin_shell = None;
            }
            Some(name) => match catalog.find(name) {
                Some(skin) => {
                    self.skin = Some(name.to_owned());
                    self.skin_style = Some(skin.style.clone());
                    self.skin_shell = skin.shell;
                }
                None => {
                    self.skin = None;
                    self.skin_style = None;
                    self.skin_shell = None;
                }
            },
        }
    }

    /// 落盘到 `<dir>/settings.json`;`dir` 为 `None` 时用平台默认目录。
    /// 目录不存在则创建。失败带路径,提示行可直接展示。
    pub fn save_to(&self, dir: Option<&Path>) -> Result<(), SaveError> {
        let Some(dir) = dir.map(Path::to_path_buf).or_else(config_dir) else {
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
        crate::persist::write_atomic(&dir.join(SETTINGS_FILE), json.as_bytes()).map_err(|source| {
            SaveError {
                path: dir.join(SETTINGS_FILE),
                source: Box::new(source),
            }
        })
    }

    /// 把排版偏好(字号/行距,#23)钳进合法范围。settings.json 可手改,
    /// 越界值(99/0 等)直进渲染会失控 —— F2 滑杆自带 range,但这里是不
    /// 依赖 UI 的防线,语义集中在 [`clamp_editor_font_size`]/
    /// [`clamp_line_height`] 一处。
    pub fn clamp_font_prefs(&mut self) {
        self.editor_font_size = clamp_editor_font_size(self.editor_font_size);
        self.line_height = clamp_line_height(self.line_height);
    }

    /// 从指定目录读取;`dir` 存在性由调用方保证语义(不存在 = 首次运行)。
    /// `pub(crate)`:测试与启动路径都按目录注入,不走平台默认目录。
    /// 读取后经 [`ThemeSettings::clamp_font_prefs`] 统一钳制 —— 手改盘上
    /// 文件写出的越界字号/行距不进渲染。
    pub(crate) fn load_from(dir: &Path) -> Result<Self, LoadError> {
        let path = dir.join(SETTINGS_FILE);
        let bytes = std::fs::read(&path).map_err(|source| match source.kind() {
            io::ErrorKind::NotFound => LoadError::Missing,
            _ => LoadError::Corrupt(format!("{}: {}", path.display(), source)),
        })?;
        let mut settings: Self = serde_json::from_slice(&bytes)
            .map_err(|source| LoadError::Corrupt(format!("{}: {}", path.display(), source)))?;
        settings.clamp_font_prefs();
        Ok(settings)
    }
}

/// 全应用语义颜色。正文、导航、工具栏和弹窗共享同一色板。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellTokens {
    /// 窗口顶栏与分组底色。
    pub chrome: Color32,
    /// 经典布局的图标轨道。
    pub rail: Color32,
    /// 侧边栏 / 顶栏 / 菜单栏的底。
    pub sidebar: Color32,
    /// 编辑器与预览的内容区底。
    pub content: Color32,
    /// 主文字。
    pub text: Color32,
    /// 次要文字(提示行、占位)。
    pub secondary: Color32,
    /// 悬停底(白底上)。
    pub hover: Color32,
    /// 选中底(浅蓝,列表/页签选中)。
    pub selected_bg: Color32,
    /// 强调(链接、选中文字、活动页签)。
    pub accent: Color32,
    /// 分隔线与描边(弱,尽量少用)。
    pub border: Color32,
    /// 代码块 / 行内代码底。
    pub code_bg: Color32,
    /// 更弱一档的底(斑马纹、禁用区)。
    pub faint: Color32,
}

/// 取当前明暗的 shell token。
pub fn shell_tokens(dark: bool) -> ShellTokens {
    if cfg!(target_os = "macos") {
        return macos_shell_tokens(dark);
    }
    if dark {
        ShellTokens {
            chrome: window_fill(dark),
            rail: tokens::rail_fill(dark),
            // 2026-10-08 S2-3:`#1B1C1F`(原 `#202124`)。与 content
            // `#292A2D` 的对比度 1.122:1 → **1.196:1**,每通道差 9 → 14。
            // 取值理由:暗色下三栏(侧栏 / 编辑器 / 预览)的可辨边界靠
            // 「侧栏比内容更沉」这一条线索,差 9 时并排看几乎是一片;拉到
            // 14 后侧栏明确退到背景层,内容区浮起来。上限不再往上推 ——
            // 再深就与窗口底色撞上,侧栏会显得"挖了个洞"。
            sidebar: Color32::from_rgb(0x1B, 0x1C, 0x1F),
            content: Color32::from_rgb(0x29, 0x2A, 0x2D),
            text: Color32::from_rgb(0xE8, 0xEA, 0xED),
            secondary: Color32::from_rgb(0x9A, 0xA0, 0xA6),
            hover: Color32::from_rgb(0x35, 0x37, 0x3A),
            selected_bg: Color32::from_rgb(0x2B, 0x3F, 0x5E),
            accent: Color32::from_rgb(0x6C, 0x9F, 0xFF),
            border: Color32::from_rgb(0x3C, 0x40, 0x43),
            code_bg: Color32::from_rgb(0x23, 0x24, 0x27),
            // 与 content 每通道差 ~10:对齐导出 CSS 暗色表头口径(#161b22
            // vs #0d1117)。曾取 content+1,表头/斑马底人眼不可辨。
            faint: Color32::from_rgb(0x32, 0x34, 0x38),
        }
    } else {
        ShellTokens {
            chrome: window_fill(dark),
            rail: tokens::rail_fill(dark),
            // 2026-10-08 S2-3:`#EDEFF2`(原 `#F2F3F5`)。与 content `#FFFFFF`
            // 的对比度 1.110:1 → **1.135:1**,每通道差 13 → 18。
            // 理由与暗色同一条线索(侧栏退到背景层),但浅色下不能一味
            // 加深 —— 加到 #E4E6EA 就与 border `#E5E6E8` 撞色,侧栏里的
            // 分隔线会消失(浅色下分隔线比底色差更重要)。#EDEFF2 是
            // 「仍浅于 border 一档」的最深值,由
            // `light_sidebar_stays_lighter_than_border` 钉住。
            //
            // **hover 仍是 `#F2F3F5`**:它比新 sidebar 深,于是 hover 在
            // 侧栏里表现为「一块更深的斑」而非「提亮」。这是有意的 ——
            // 侧栏整体退到背景层后,hover 若也提亮会与选中态(selected_bg
            // #E1EFFF)争夺注意力。
            sidebar: Color32::from_rgb(0xED, 0xEF, 0xF2),
            content: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            text: Color32::from_rgb(0x1F, 0x23, 0x29),
            secondary: Color32::from_rgb(0x64, 0x6A, 0x73),
            hover: Color32::from_rgb(0xF2, 0xF3, 0xF5),
            selected_bg: Color32::from_rgb(0xE1, 0xEF, 0xFF),
            accent: Color32::from_rgb(0x33, 0x70, 0xFF),
            border: Color32::from_rgb(0xE5, 0xE6, 0xEB),
            code_bg: Color32::from_rgb(0xF5, 0xF6, 0xF7),
            // 与导出 HTML 的 th 底同值(latermd-export CSS #f6f8fa):
            // 预览与导出同观感,预览不再弱于导出。
            faint: Color32::from_rgb(0xF6, 0xF8, 0xFA),
        }
    }
}

/// macOS 外壳保持中性,强调色只用于链接、选中与操作。
fn macos_shell_tokens(dark: bool) -> ShellTokens {
    let rgb = |r, g, b| Color32::from_rgb(r, g, b);
    if dark {
        ShellTokens {
            chrome: window_fill(dark),
            rail: tokens::rail_fill(dark),
            sidebar: rgb(36, 36, 38),
            content: rgb(30, 30, 32),
            text: rgb(245, 245, 247),
            secondary: rgb(163, 163, 170),
            hover: rgb(52, 52, 56),
            selected_bg: rgb(48, 61, 78),
            accent: rgb(10, 132, 255),
            border: rgb(62, 62, 66),
            code_bg: rgb(40, 40, 44),
            faint: rgb(43, 43, 46),
        }
    } else {
        ShellTokens {
            chrome: window_fill(dark),
            rail: tokens::rail_fill(dark),
            sidebar: rgb(240, 240, 242),
            content: Color32::WHITE,
            text: rgb(29, 29, 31),
            secondary: rgb(106, 106, 112),
            hover: rgb(229, 229, 233),
            selected_bg: rgb(220, 231, 244),
            accent: rgb(0, 122, 255),
            border: rgb(216, 216, 220),
            code_bg: rgb(245, 245, 247),
            faint: rgb(246, 248, 250),
        }
    }
}

pub fn window_fill(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(40, 40, 42)
    } else {
        Color32::from_rgb(245, 245, 247)
    }
}

/// 内容区的底(编辑器与预览面板显式 `.fill`;侧栏吃 `panel_fill`)。
#[cfg(test)]
pub fn content_fill(dark: bool) -> Color32 {
    shell_tokens(dark).content
}

/// 可分享的应用皮肤同时提供明、暗两套；明暗模式独立切换。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellPalette {
    pub light: ShellTokens,
    pub dark: ShellTokens,
}

impl Default for ShellPalette {
    fn default() -> Self {
        Self {
            light: shell_tokens(false),
            dark: shell_tokens(true),
        }
    }
}

impl ShellPalette {
    pub fn colors(self, dark: bool) -> ShellTokens {
        if dark {
            self.dark
        } else {
            self.light
        }
    }
}

/// 只读当前窗口的皮肤；避免进程级全局变量影响多窗口/无头测试。
pub fn shell(ui: &egui::Ui) -> ShellTokens {
    ui.ctx()
        .data(|data| data.get_temp::<ShellPalette>(egui::Id::new("latermd-shell")))
        .unwrap_or_default()
        .colors(ui.visuals().dark_mode)
}

/// 颜色变化才投影，空闲帧不推进 style 版本。重置密度缓存，以免换肤恢复默认尺寸。
fn apply_shell(ctx: &egui::Context, palette: ShellPalette) {
    let id = egui::Id::new("latermd-shell");
    let changed = ctx.data_mut(|data| {
        if data.get_temp::<ShellPalette>(id) == Some(palette) {
            return false;
        }
        data.insert_temp(id, palette);
        data.remove::<Density>(egui::Id::new("latermd-density"));
        true
    });
    if changed {
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            ctx.style_mut_of(theme, |style| {
                apply_shell_to(style, palette.colors(theme == egui::Theme::Dark))
            });
        }
    }
}

fn apply_shell_to(style: &mut egui::Style, c: ShellTokens) {
    let v = &mut style.visuals;
    v.panel_fill = c.sidebar;
    v.window_fill = c.content;
    v.weak_text_color = Some(c.secondary);

    v.extreme_bg_color = c.code_bg;
    // Markdown code blocks read this dedicated visual instead of
    // `extreme_bg_color`; keep it on the same semantic token so a skin switch
    // cannot leave the code surface behind in egui's default gray.
    v.code_bg_color = c.code_bg;
    v.faint_bg_color = c.faint;
    v.hyperlink_color = c.accent;
    // 选区:浅蓝底、正文色文字。egui 0.36 里 `selection.stroke` 兼任
    // **选中文字的颜色**(TextEdit 把选中字形重涂成它,选中态 label 的
    // 前景也取它),设成 NONE 会让选中文字全透明——蓝底上看不见字。
    v.selection.bg_fill = c.selected_bg;
    v.selection.stroke = egui::Stroke::new(1.0, c.text);
    // 圆角/输入框几何:U0 token 投影(tokens.rs 是唯一数字真源)。
    // 控件圆角 RADIUS_MD;输入框高度 = interact_size.y(egui 里 TextEdit
    // 无独立高度字段,点击类控件的最小高度统一取它),内边距 =
    // button_padding(TextEdit 与按钮共用)。紧凑密度的缩放在
    // `apply_density` 里以同一组 token 为基准,两处不散落。
    let radius = egui::CornerRadius::same(tokens::RADIUS_MD as u8);
    style.spacing.interact_size = egui::vec2(tokens::INPUT_H, tokens::INPUT_H);
    style.spacing.button_padding = egui::vec2(tokens::INPUT_PAD_X, tokens::INPUT_PAD_Y);
    // 小字号(FONT_SM)落 Small 档:提示行/状态栏取它,Body 13 不动
    // (字号用户设置是 roadmap 专题 #23,与本投影解耦)
    if let Some(small) = style.text_styles.get_mut(&egui::TextStyle::Small) {
        small.size = tokens::FONT_SM;
    }
    // 0.36 的窗口/菜单圆角字段已不在 Visuals/Spacing 的公开面,浮窗圆角
    // 走 egui 出厂值,不做覆盖
    // 滚动条:出厂 12px 偏粗,WorkBuddy 是细浅条
    style.spacing.scroll.bar_width = 8.0;
    style.spacing.scroll.bar_inner_margin = 4.0;
    style.spacing.scroll.bar_outer_margin = 2.0;
    // 分隔线弱化:panel 之间靠底色分区,线只在必要时出现
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, c.border.gamma_multiply(0.58));
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, c.text);
    v.widgets.noninteractive.corner_radius = radius;
    let widgets = [
        // inactive 底色不能全透明:egui 的 Slider 轨道硬绑
        // `widgets.inactive.bg_fill`(slider.rs rect_filled),透明 = 轨道
        // 隐形、只剩手柄漂浮(坤哥 2026-09-29 截图「AI 页滑块错乱」实锤)。
        // 取 border 档:轨道=分隔线语义可辨,按钮静止底色从全透明变极淡
        // 分隔线色,与 hover 同量级,平面观感保留。
        (&mut v.widgets.inactive, c.text, c.border),
        (&mut v.widgets.hovered, c.text, c.hover),
        (&mut v.widgets.active, c.accent, c.selected_bg),
        (&mut v.widgets.open, c.text, c.hover),
    ];
    for (widget, fg, bg) in widgets {
        widget.fg_stroke = egui::Stroke::new(1.0, fg);
        widget.bg_fill = bg;
        widget.weak_bg_fill = bg;
        widget.corner_radius = radius;
        widget.bg_stroke = egui::Stroke::NONE;
    }
    if cfg!(target_os = "macos") {
        // egui 的正文标题取 active 前景;操作强调由独立 accent token 承担。
    }
    // 输入框/按钮内的弱文字(占位符)用次要色
    style.visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
}

/// 密度 → egui style token。
///
/// 只在**密度真的变了**时写 style(写入会让 egui 重算布局缓存),且一律以
/// `egui::Style::default()` 的出厂值为基准做缩放 —— 不基于「当前值」再乘,
/// 否则标准↔紧凑来回切会逐次累积。
fn apply_density(ctx: &egui::Context, density: Density) {
    let id = egui::Id::new("latermd-density");
    let changed = ctx.data_mut(|data| {
        let changed = data.get_temp::<Density>(id) != Some(density);
        data.insert_temp(id, density);
        changed
    });
    if !changed {
        return;
    }
    let (scale, rounding_scale) = match density {
        Density::Standard => (1.0_f32, 1.0_f32),
        // 紧凑:间距/控件高度压到七成,圆角略收 —— 长文档一屏多看几行,
        // 但不改字号(改字号会牺牲中文可读性)
        Density::Compact => (0.7_f32, 0.8_f32),
    };
    let base = egui::Style::default();
    let margin = base.spacing.window_margin;
    ctx.all_styles_mut(|style| {
        // U0 token 系(控件高度/内边距/圆角)的缩放基准是**设计值**而非
        // egui 出厂值:`apply_shell_to` 已把两套 style 投影成 INPUT_H/PAD/
        // RADIUS_MD,若这里从出厂值(18/4/1/5)缩放,标准档会把投影顶回去,
        // 两处投影互相打架。紧凑档 = 设计值 × 0.7(圆角 × 0.8)。
        let radius = egui::CornerRadius::same((tokens::RADIUS_MD * rounding_scale) as u8);
        style.spacing.interact_size = egui::vec2(tokens::INPUT_H, tokens::INPUT_H) * scale;
        style.spacing.button_padding = egui::vec2(tokens::INPUT_PAD_X, tokens::INPUT_PAD_Y) * scale;
        style.spacing.item_spacing = base.spacing.item_spacing * scale;
        style.spacing.indent = base.spacing.indent * scale;
        // 滚动条:紧凑模式下收窄,让出的宽度归正文(egui 0.36 的滚动条
        // 参数在 `spacing.scroll` 里,不再是单个 bar_width 字段)
        style.spacing.scroll.bar_width = base.spacing.scroll.bar_width * scale;
        style.spacing.scroll.bar_inner_margin = base.spacing.scroll.bar_inner_margin * scale;
        // Margin 的字段是 i8(egui 0.36),先转 f32 再缩放取整
        let scaled = |value: i8| (f32::from(value) * scale).round() as i8;
        style.spacing.window_margin = egui::Margin {
            left: scaled(margin.left),
            right: scaled(margin.right),
            top: scaled(margin.top),
            bottom: scaled(margin.bottom),
        };
        // 圆角:同上,紧凑档直接从 RADIUS_MD 缩放(标准档投影见
        // apply_shell_to;WidgetVisuals 不是 Copy,逐个赋值)
        let widgets = &mut style.visuals.widgets;
        for widget in [
            &mut widgets.noninteractive,
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
            &mut widgets.open,
        ] {
            widget.corner_radius = radius;
        }
    });
}

/// 字号偏好(#23 F3)与行距投影(#50 M2)在 egui data 槽的键:
/// `apply_font_size` 写入(经 staleness 检查),预览正文侧的
/// [`editor_font_size`] 读取。槽值是 `(字号, 行距, 投影族, fonts 就绪)`:
/// 族随字体安装状态而变(#50 M1),行距与 fonts 就绪标志(#50 M2,见
/// [`apply_font_size`])进同一键控 —— size/ratio/族任一变化或 fonts
/// 转为就绪的当帧即重投影。
fn font_size_id() -> egui::Id {
    egui::Id::new("latermd-editor-font-size")
}

/// 当前生效的正文基准字号(#23 F3):预览正文(vendored `MarkdownLabel`
/// 的显式 `FontId`)与编辑器 Monospace 档投影同源。
///
/// 生产路径恒有 `ThemeSettings::apply` 先行(启动 main 一次 + 每帧 logic);
/// 未投影过的 context(无头测试直渲预览)回落出厂默认 15pt —— 比回落
/// egui Body 档出厂值更接近真机口径。自建显式 `FontId` 的取证测试
/// (#43 系列)不受本读侧影响。
pub fn editor_font_size(ctx: &egui::Context) -> f32 {
    ctx.data(|data| {
        data.get_temp::<(f32, f32, egui::FontFamily, bool)>(font_size_id())
            .map(|(size, _, _, _)| size)
            .unwrap_or(EDITOR_FONT_SIZE_DEFAULT)
    })
}

/// 编辑器档在 `size` 字号下的自然行高(pt,#50 M2):与 egui TextEdit
/// 行高公式的输入同源(builder.rs `row_height(font_id)`,同一 FontId 同一
/// ppp 同一取整),投影后的行盒因此精确等于「自然行高 + extra」。
/// `None` = fonts 未就绪(首帧前,见 [`fonts_ready`]),调用方跳过本轮
/// 行距投影。
fn natural_row_height(ctx: &egui::Context, size: f32) -> Option<f32> {
    if !fonts_ready(ctx) {
        return None;
    }
    let font = egui::FontId::new(size, crate::fonts::editor_mono_family(ctx));
    Some(ctx.fonts_mut(|fonts| fonts.row_height(&font)))
}

/// 行距投影的像素换算(#50 M2,纯函数,单测锚点):字号 × 用户行距超出
/// 自然行高的部分才是可加值。clamp ≥ 0:自然行高是行盒物理下限,用户
/// 调低(如 1.2)时投影为 0,绝不为负 —— 负 extra 会压缩行盒造成行间
/// 重叠与字形裁切,与预览侧 `min_line_height_em` 的兜底同语义。
fn editor_extra_line_spacing(size: f32, ratio: f32, natural: f32) -> f32 {
    (size * ratio - natural).max(0.0)
}

/// fonts 就绪标志(egui data 槽):`ctx.fonts` 自首个 run_ui 的
/// begin_pass 实例化字体起才可用(egui 0.36 契约,提前调用 panic),
/// 首个 pass 开始时由 [`install_fonts_ready_hook`] 注册的回调置位。
/// 行距投影以此区分「首帧前的启动装载」(main 创建回调,fonts 未就绪)
/// 与「帧内 logic」(每帧,fonts 已就绪)。
fn fonts_ready(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<bool>(fonts_ready_id()).unwrap_or(false))
}

fn fonts_ready_id() -> egui::Id {
    egui::Id::new("latermd-fonts-ready")
}

/// 注册「首个 pass 置位 fonts 就绪」的回调(幂等:标志槽已存在则不再
/// 注册)。回调触发点在 begin_pass(字体实例化)之后、当帧 logic 之前,
/// 因此首帧 logic 的 `apply_font_size` 即可经 `ctx.fonts` 现算自然行高,
/// 首帧渲染(TextEdit 读 spacing)之前行距投影必然补上。
fn install_fonts_ready_hook(ctx: &egui::Context) {
    let id = fonts_ready_id();
    let installed = ctx.data_mut(|data| {
        let installed = data.get_temp::<bool>(id).is_some();
        if !installed {
            data.insert_temp(id, false);
        }
        installed
    });
    if !installed {
        ctx.on_begin_pass(
            "latermd-fonts-ready",
            Arc::new(move |ui: &mut egui::Ui| {
                ui.ctx().data_mut(|data| data.insert_temp(id, true));
            }),
        );
    }
}

/// 字号偏好(#23 F3)→ egui style 的 `TextStyle::Monospace` 档投影:编辑器
/// 面板(源码模式 TextEdit、行号槽、Live 模式活动块源码)字体恒取
/// Monospace 档,改档位 size 即改编辑器字号。#50 M1 起族一并投影:
/// 有 CJK 且编辑器专用等宽族注册成功时用 [`crate::fonts::
/// editor_mono_family`](`editor-mono`,链头与 `FontFamily::Monospace` 同
/// 为内置 Hack、纯 ASCII 排版逐像素一致,链尾 CJK 副本行 metrics 对齐链
/// 头、混排基线对齐),否则保持 `FontFamily::Monospace`(降级 = 修复前
/// 行为)。字号语义不变:唯一事实源仍是本档位的 size 字段,行号槽与
/// Live 活动块经 `FontSelection::Style(Monospace)` 自动跟随。
///
/// #50 M2 起行距(`ratio`,坤哥 2026-10-02「每行间距也太小」,#73 方案②
/// 转正)随同槽键控一并投影:`spacing.extra_text_line_spacing` =
/// `editor_extra_line_spacing(size, ratio, 自然行高)`。TextEdit 的行盒
/// = 自然行高 + extra(egui builder.rs 同一公式同一输入),因此精确等于
/// `字号 × 行距`;行号槽/Live 活动块读同一 TextEdit galley,自动跟随。
///
/// 与 [`apply_density`] 同款 staleness 检查:`apply` 每帧调用,值没变就
/// 不写 style(`style_mut_of` 即使值相同也推进 style 版本、作废布局缓存,
/// 滑杆拖动帧之外不得付这笔开销);字号/行距/族任一变化(滑杆/载入新
/// settings/字体安装完成)当帧即重投影 —— 不放进 `apply_shell` 那类
/// 一次性投影(ui.data 标志只挡第一次,后续滑杆变化会哑掉),而是像密度
/// 一样按值键控。fonts 就绪标志在键控里:自然行高必须经 `ctx.fonts`
/// 现算,而 egui 契约它在首个 run_ui 之前不可用 —— 首帧前的启动装载
/// (main 创建回调)本轮跳过行距投影(字号档不依赖 fonts,照常投影),
/// fonts 就绪的下一帧(即首个 pass 的 logic,回调已在 begin_pass 后置位)
/// 因键控差异必然重投影,赶在首帧渲染之前。
fn apply_font_size(ctx: &egui::Context, size: f32, ratio: f32) {
    let family = crate::fonts::editor_mono_family(ctx);
    let ready = fonts_ready(ctx);
    let id = font_size_id();
    let changed = ctx.data_mut(|data| {
        let changed = data.get_temp::<(f32, f32, egui::FontFamily, bool)>(id)
            != Some((size, ratio, family.clone(), ready));
        data.insert_temp(id, (size, ratio, family.clone(), ready));
        changed
    });
    if !changed {
        return;
    }
    let extra = natural_row_height(ctx, size)
        .map(|natural| editor_extra_line_spacing(size, ratio, natural));
    if extra.is_none() {
        install_fonts_ready_hook(ctx);
    }
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        ctx.style_mut_of(theme, |style| {
            if let Some(mono) = style.text_styles.get_mut(&egui::TextStyle::Monospace) {
                mono.size = size;
                mono.family = family.clone();
            }
            if let Some(extra) = extra {
                style.spacing.extra_text_line_spacing = extra;
            }
        });
    }
}

/// 一份皮肤：显示名、正文样式，以及可选的整应用明暗色板。
#[derive(Debug, Clone, PartialEq)]
pub struct Skin {
    pub name: String,
    pub style: MarkdownStyle,
    pub shell: Option<ShellPalette>,
}

/// 新文件包含完整应用色板；旧的 MarkdownStyle RON 仍可直接读取。
#[derive(Debug, Serialize, Deserialize)]
pub struct SkinFile {
    pub markdown: MarkdownStyle,
    pub shell: ShellPalette,
}

fn parse_skin(
    text: &str,
) -> Result<(MarkdownStyle, Option<ShellPalette>), ron::error::SpannedError> {
    // MarkdownStyle 使用 serde(default)，直接 fallback 会把损坏的新文件吞成默认样式。
    // 先识别包裹字段，完整皮肤的缺字段/错误颜色必须报错。
    let value: ron::Value = ron::from_str(text)?;
    let wrapped = matches!(&value, ron::Value::Map(map) if map.iter().any(|(key, _)| {
        matches!(key, ron::Value::String(key) if key == "markdown" || key == "shell")
    }));
    if wrapped {
        ron::from_str::<SkinFile>(text).map(|file| (file.markdown, Some(file.shell)))
    } else {
        ron::from_str::<MarkdownStyle>(text).map(|style| (style, None))
    }
}

/// 皮肤目录的内容(启动扫描一次,换皮肤/导出时重扫)。
///
/// 为什么是目录扫描而不是配置里列清单:皮肤是**文件**,用户可以直接把别人
/// 给的 `.ron` 丢进目录,不该还得再改一次 settings.json。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkinCatalog {
    /// 按名字排序。
    pub skins: Vec<Skin>,
}

impl SkinCatalog {
    /// 扫描 `<dir>/themes/*.ron`;目录不存在视为空(首次运行)。坏文件跳过
    /// 并在终端告警 —— 一个坏皮肤不该挡住应用启动。
    pub fn load_from(dir: &Path) -> Self {
        let themes = dir.join(THEMES_DIR);
        let Ok(entries) = std::fs::read_dir(&themes) else {
            return Self::default();
        };
        let mut skins: Vec<Skin> = entries
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "ron"))
            .filter_map(|entry| {
                let path = entry.path();
                let text = std::fs::read_to_string(&path).ok()?;
                let parsed = parse_skin(&text);
                match parsed {
                    Ok((style, shell)) => Some(Skin {
                        name: entry
                            .path()
                            .file_stem()
                            .map(|stem| stem.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        style,
                        shell,
                    }),
                    Err(error) => {
                        eprintln!(
                            "LaterMD: 皮肤文件解析失败,已跳过 {}: {error}",
                            path.display()
                        );
                        None
                    }
                }
            })
            .collect();
        skins.sort_by(|left, right| left.name.cmp(&right.name));
        Self { skins }
    }

    pub fn find(&self, name: &str) -> Option<&Skin> {
        self.skins.iter().find(|skin| skin.name == name)
    }
}

/// 皮肤文件名:清掉路径分隔符与 Windows 保留字符,空名给默认名。
///
/// 名字直接来自用户输入且要拼进路径,`..` 与 `/` 必须挡住(与 MCP 的路径
/// 校验同一思路:别让用户输入的字符串变成路径遍历)。
pub fn skin_file_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '.' => '_',
            other => other,
        })
        .collect();
    if cleaned.is_empty() {
        "my-theme".to_owned()
    } else {
        cleaned
    }
}

/// 导出完整应用皮肤(RON)，同时保存正文与外壳；目录不存在则创建。
pub fn export_skin(
    dir: &Path,
    name: &str,
    style: &MarkdownStyle,
    shell: ShellPalette,
) -> Result<PathBuf, String> {
    let path = dir
        .join(THEMES_DIR)
        .join(format!("{}.ron", skin_file_name(name)));
    let text = ron::ser::to_string_pretty(
        &SkinFile {
            markdown: style.clone(),
            shell,
        },
        ron::ser::PrettyConfig::default(),
    )
    .map_err(|error| format!("皮肤序列化失败: {error}"))?;
    std::fs::create_dir_all(path.parent().unwrap_or(dir))
        .map_err(|error| format!("{}: {error}", dir.display()))?;
    std::fs::write(&path, text.as_bytes())
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(path)
}

/// 读配置的失败情形(公开:`load_from` 已是 `pub(crate)`,私有返回类型会
/// 触发 private_interfaces)。
#[derive(Debug)]
pub(crate) enum LoadError {
    /// 文件不存在:首次运行,正常路径。
    Missing,
    /// 文件在但读不出/解析不了,字符串已带路径与原因。
    Corrupt(String),
}

/// 写配置失败:带路径,可直接进提示行(与 `crate::file::FileError` 同构,
/// 单独成类型是因为这里只此一处落盘且无 `io::Error` 之外的固定动作名)。
#[derive(Debug)]
pub struct SaveError {
    path: PathBuf,
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "主题保存失败 {}: {}", self.path.display(), self.source)
    }
}

/// 平台配置目录(手写,不引目录库):
/// Linux/其余 Unix `$XDG_CONFIG_HOME/latermd`(未设置或为空串时
/// `$HOME/.config/latermd`,按 XDG 规范);macOS
/// `$HOME/Library/Application Support/latermd`;Windows `%APPDATA%\latermd`。
///
/// 文件树的最近目录持久化(`crate::filetree`)落在同一目录,故 crate 内共享。
pub(crate) fn config_dir() -> Option<PathBuf> {
    config_dir_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
        std::env::var_os("APPDATA"),
    )
}

/// [`config_dir`] 的纯函数核,env 值由参数注入(测试不碰进程环境变量)。
fn config_dir_from(
    xdg: Option<OsString>,
    home: Option<OsString>,
    appdata: Option<OsString>,
) -> Option<PathBuf> {
    let into_path = |value: OsString| PathBuf::from(value);
    if cfg!(target_os = "macos") {
        home.map(into_path)
            .map(|home| home.join("Library/Application Support/latermd"))
    } else if cfg!(target_os = "windows") {
        appdata
            .map(into_path)
            .map(|appdata| appdata.join("latermd"))
    } else {
        xdg.filter(|dir| !dir.is_empty())
            .map(into_path)
            .or_else(|| home.map(into_path).map(|home| home.join(".config")))
            .map(|dir| dir.join("latermd"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("latermd-theme-{}-{name}", std::process::id()))
    }

    /// Linux 分支:XDG 优先;空串按 XDG 规范视同未设置,回落 ~/.config;
    /// HOME 也无则给不出目录。
    /// (macOS/Windows 分支是各两行的 join,由 `cfg!` 选择,本机测不到。)
    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn config_dir_prefers_xdg_then_home() {
        let xdg = |v: &str| Some(OsString::from(v));
        let home = || Some(OsString::from("/home/u"));

        assert_eq!(
            config_dir_from(xdg("/custom/cfg"), home(), None),
            Some(PathBuf::from("/custom/cfg/latermd"))
        );
        assert_eq!(
            config_dir_from(xdg(""), home(), None),
            Some(PathBuf::from("/home/u/.config/latermd")),
            "XDG 空串 = 未设置"
        );
        assert_eq!(
            config_dir_from(None, home(), None),
            Some(PathBuf::from("/home/u/.config/latermd"))
        );
        assert_eq!(config_dir_from(None, None, None), None);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn config_dir_uses_application_support_on_macos() {
        assert_eq!(
            config_dir_from(
                Some("/ignored/xdg".into()),
                Some("/Users/test".into()),
                None
            ),
            Some(PathBuf::from(
                "/Users/test/Library/Application Support/latermd"
            ))
        );
        assert_eq!(
            config_dir_from(Some("/ignored/xdg".into()), None, None),
            None
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn config_dir_uses_appdata_on_windows() {
        assert_eq!(
            config_dir_from(
                Some("/ignored/xdg".into()),
                Some("/ignored/home".into()),
                Some("C:/Users/test/AppData/Roaming".into())
            ),
            Some(PathBuf::from("C:/Users/test/AppData/Roaming").join("latermd"))
        );
        assert_eq!(
            config_dir_from(None, Some("/ignored/home".into()), None),
            None
        );
    }

    /// 往返:模式与 overrides(含颜色对、字号、代码语言等全部字段)逐项一致;
    /// 目录不存在时 save 会建目录。
    #[test]
    fn save_load_round_trip_preserves_settings() {
        use egui_markdown_style::{HeadingStyle, InlineCodeStyle};

        let dir = temp_dir("roundtrip/nested");
        let defaults = MarkdownStyle::default();
        let settings = ThemeSettings {
            mode: ThemeMode::Light,
            overrides: Some(MarkdownStyle {
                heading: HeadingStyle {
                    scales: [2.0, 1.35, 1.2, 1.1, 1.05, 1.0],
                },
                block_spacing: 13.0,
                inline_code: InlineCodeStyle {
                    color_light: egui::Color32::from_rgb(1, 2, 3),
                    ..defaults.inline_code
                },
                default_code_language: "rust".to_owned(),
                ..defaults
            }),
            ..ThemeSettings::default()
        };

        settings.save_to(Some(&dir)).unwrap();
        assert_eq!(ThemeSettings::load_from(&dir).unwrap(), settings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Emoji「最近使用」随 settings.json 往返(E2,docs/emoji-plan.md
    /// §6.3):顺序保持(新的在前),旧存档缺该项回落空、不致解析失败。
    #[test]
    fn emoji_recent_round_trips_and_defaults_to_empty() {
        let dir = temp_dir("emoji-recent");
        let settings = ThemeSettings {
            emoji_recent: vec!["🚀".to_owned(), "😀".to_owned(), "🇨🇳".to_owned()],
            ..ThemeSettings::default()
        };

        settings.save_to(Some(&dir)).unwrap();
        assert_eq!(ThemeSettings::load_from(&dir).unwrap(), settings);

        // 旧版 settings.json 没有 emoji_recent:serde(default) 兜底
        std::fs::write(dir.join(SETTINGS_FILE), br#"{"mode":"dark"}"#).unwrap();
        assert!(ThemeSettings::load_from(&dir)
            .unwrap()
            .emoji_recent
            .is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 标签条标题宽度模式(#37)随 settings.json 往返;旧文件缺该项回落
    /// `Full`(完整 = #11 既有观感,升级不改变默认行为)。
    #[test]
    fn title_width_round_trips_and_old_settings_fall_back() {
        let dir = temp_dir("title-width");
        let settings = ThemeSettings {
            tab_title_width: TitleWidthMode::Short,
            ..ThemeSettings::default()
        };
        settings.save_to(Some(&dir)).unwrap();
        assert_eq!(ThemeSettings::load_from(&dir).unwrap(), settings);
        // 落盘的 JSON 里是可读的小写值(手改配置可辨认)
        let json = std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(json.contains(r#""tab_title_width": "short""#), "{json}");

        // 旧版 settings.json 没有 tab_title_width:serde(default) 兜底回 Full
        std::fs::write(dir.join(SETTINGS_FILE), br#"{"mode":"dark"}"#).unwrap();
        assert_eq!(
            ThemeSettings::load_from(&dir).unwrap().tab_title_width,
            TitleWidthMode::Full
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 左栏外壳样式(mac 专项)随 settings.json 往返;旧文件缺该项由
    /// `#[serde(default)]` 兜底回随平台的出厂默认(mac = 工作台,升级
    /// 零变化;其余平台 = 图标栏,与本平台既有外壳一致)。
    #[test]
    fn sidebar_style_round_trips_and_old_settings_fall_back() {
        let dir = temp_dir("sidebar-style");
        let settings = ThemeSettings {
            sidebar_style: SidebarStyle::Rail,
            ..ThemeSettings::default()
        };
        settings.save_to(Some(&dir)).unwrap();
        assert_eq!(ThemeSettings::load_from(&dir).unwrap(), settings);
        // 落盘的 JSON 里是可读的小写值(手改配置可辨认)
        let json = std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(json.contains(r#""sidebar_style": "rail""#), "{json}");

        // 旧版 settings.json 没有 sidebar_style:兜底回随平台默认
        std::fs::write(dir.join(SETTINGS_FILE), br#"{"mode":"dark"}"#).unwrap();
        assert_eq!(
            ThemeSettings::load_from(&dir).unwrap().sidebar_style,
            SidebarStyle::default()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `SidebarStyle` 的出厂默认随平台:mac 出工作台(升级零变化),其余
    /// 平台出图标栏(与两平台既有外壳一致)。cfg_attr 钉住,防止将来有人
    /// 把 `#[default]` 挪到单一变体上悄悄改掉另一平台的开箱形态。
    #[test]
    fn sidebar_style_default_follows_platform() {
        let expected = if cfg!(target_os = "macos") {
            SidebarStyle::Workbench
        } else {
            SidebarStyle::Rail
        };
        assert_eq!(SidebarStyle::default(), expected);
    }

    /// #23 排版偏好默认值:字号 15.0pt、行距 1.5 —— 默认值落在手动
    /// `Default` 实现里(derive 给不出非零 f32),手动实现既服务 Rust 侧
    /// `default()`,也是 serde 缺字段回落的取值源,断言钉住两头共用的值。
    #[test]
    fn font_prefs_default_to_15pt_and_1_5() {
        assert_eq!(
            ThemeSettings::default().editor_font_size,
            EDITOR_FONT_SIZE_DEFAULT
        );
        assert_eq!(ThemeSettings::default().line_height, LINE_HEIGHT_DEFAULT);
    }

    /// #55 M2:minimap 开关随 settings.json 往返;默认开(与 VS Code 等
    /// 编辑器出厂一致,取舍见 decisions-pending #105);旧文件缺该项由
    /// serde(default) 兜底回开。
    #[test]
    fn minimap_auto_defaults_and_explicit_override_survive_settings() {
        let old: ThemeSettings = serde_json::from_str(r#"{"show_minimap":false}"#).unwrap();
        assert!(!old.show_minimap && old.minimap_auto);
        let manual = ThemeSettings {
            minimap_auto: false,
            ..Default::default()
        };
        let restored: ThemeSettings =
            serde_json::from_str(&serde_json::to_string(&manual).unwrap()).unwrap();
        assert!(!restored.minimap_auto && restored.show_minimap);
    }

    #[test]
    fn show_minimap_round_trips_and_old_settings_default_on() {
        assert!(ThemeSettings::default().show_minimap, "出厂默认开");

        let dir = temp_dir("show-minimap");
        let settings = ThemeSettings {
            show_minimap: false,
            ..ThemeSettings::default()
        };
        settings.save_to(Some(&dir)).unwrap();
        assert_eq!(ThemeSettings::load_from(&dir).unwrap(), settings);
        let json = std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(json.contains(r#""show_minimap": false"#), "{json}");

        // 旧版 settings.json 没有 show_minimap:serde(default) 兜底回 true
        std::fs::write(dir.join(SETTINGS_FILE), br#"{"mode":"dark"}"#).unwrap();
        assert!(ThemeSettings::load_from(&dir).unwrap().show_minimap);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #64 M1:打字机模式开关随 settings.json 往返;默认**关**(它改变
    /// 滚动行为,出厂不替用户决定,取舍见 decisions-pending #121);旧文件
    /// 缺该项由 serde(default) 兜底回关(关闭 = 现状,升级零行为变化)。
    #[test]
    fn show_typewriter_round_trips_and_old_settings_default_off() {
        assert!(!ThemeSettings::default().show_typewriter, "出厂默认关");

        let dir = temp_dir("show-typewriter");
        let settings = ThemeSettings {
            show_typewriter: true,
            ..ThemeSettings::default()
        };
        settings.save_to(Some(&dir)).unwrap();
        assert_eq!(ThemeSettings::load_from(&dir).unwrap(), settings);
        let json = std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(json.contains(r#""show_typewriter": true"#), "{json}");

        // 旧版 settings.json 没有 show_typewriter:serde(default) 兜底回关
        std::fs::write(dir.join(SETTINGS_FILE), br#"{"mode":"dark"}"#).unwrap();
        assert!(!ThemeSettings::load_from(&dir).unwrap().show_typewriter);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #64 M2:专注模式开关随 settings.json 往返;默认**关**(强视觉改变,
    /// 出厂不替用户决定,取舍见 decisions-pending #122);旧文件缺该项由
    /// serde(default) 兜底回关(关闭 = 逐像素现状,升级零行为变化)。
    #[test]
    fn show_focus_mode_round_trips_and_old_settings_default_off() {
        assert!(!ThemeSettings::default().show_focus_mode, "出厂默认关");

        let dir = temp_dir("show-focus-mode");
        let settings = ThemeSettings {
            show_focus_mode: true,
            ..ThemeSettings::default()
        };
        settings.save_to(Some(&dir)).unwrap();
        assert_eq!(ThemeSettings::load_from(&dir).unwrap(), settings);
        let json = std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(json.contains(r#""show_focus_mode": true"#), "{json}");

        // 旧版 settings.json 没有 show_focus_mode:serde(default) 兜底回关
        std::fs::write(dir.join(SETTINGS_FILE), br#"{"mode":"dark"}"#).unwrap();
        assert!(!ThemeSettings::load_from(&dir).unwrap().show_focus_mode);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #57 M2:禅定导航显示方式随 settings.json 往返(三态逐个,serde 值
    /// 可读);默认悬停(M1 已交付的既有行为,升级不改变默认,取舍见
    /// decisions-pending #107);旧文件缺该项由 serde(default) 兜底回悬停。
    #[test]
    fn zen_nav_mode_round_trips_and_old_settings_default_hover() {
        assert_eq!(
            ThemeSettings::default().zen_nav,
            ZenNavMode::Hover,
            "出厂默认悬停"
        );

        let dir = temp_dir("zen-nav-mode");
        for (mode, json_value) in [
            (ZenNavMode::Hover, "hover"),
            (ZenNavMode::Always, "always"),
            (ZenNavMode::Off, "off"),
        ] {
            let settings = ThemeSettings {
                zen_nav: mode,
                ..ThemeSettings::default()
            };
            settings.save_to(Some(&dir)).unwrap();
            assert_eq!(ThemeSettings::load_from(&dir).unwrap(), settings);
            let json = std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
            assert!(
                json.contains(&format!(r#""zen_nav": "{json_value}""#)),
                "{json}"
            );
        }

        // 旧版 settings.json 没有 zen_nav:serde(default) 兜底回悬停
        std::fs::write(dir.join(SETTINGS_FILE), br#"{"mode":"dark"}"#).unwrap();
        assert_eq!(
            ThemeSettings::load_from(&dir).unwrap().zen_nav,
            ZenNavMode::Hover
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #23 排版偏好随 settings.json 往返:界内自定义值逐项一致(在界内,
    /// `load_from` 的钳制是恒等变换,不吞用户的合法选择);落盘 JSON 字段
    /// 名与数值可读,手改可辨认。
    #[test]
    fn font_prefs_round_trip_preserves_in_range_values() {
        let dir = temp_dir("font-prefs-roundtrip");
        let settings = ThemeSettings {
            editor_font_size: 18.0,
            line_height: 1.7,
            ..ThemeSettings::default()
        };
        settings.save_to(Some(&dir)).unwrap();
        let loaded = ThemeSettings::load_from(&dir).unwrap();
        assert_eq!(loaded.editor_font_size, 18.0);
        assert_eq!(loaded.line_height, 1.7);
        let json = std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(json.contains(r#""editor_font_size": 18.0"#), "{json}");
        assert!(json.contains(r#""line_height": 1.7"#), "{json}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #23 旧版 settings.json(升级前落盘)没有这两个新字段:`#[serde(default)]`
    /// 兜底,解析不失败且回落默认 —— 与 `emoji_recent`/`tab_title_width`
    /// 当时的升级路径同口径。
    #[test]
    fn font_prefs_missing_fields_fall_back_to_defaults() {
        let dir = temp_dir("font-prefs-legacy");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(SETTINGS_FILE),
            br#"{"mode":"dark","density":"compact"}"#,
        )
        .unwrap();
        let loaded = ThemeSettings::load_from(&dir).unwrap();
        assert_eq!(loaded.editor_font_size, EDITOR_FONT_SIZE_DEFAULT);
        assert_eq!(loaded.line_height, LINE_HEIGHT_DEFAULT);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #23 手改 settings.json 写出越界值(字号 99/0/负数、行距 99/0):
    /// `load_from` 反序列化后统一钳进界 —— 越界值不得进渲染。
    #[test]
    fn font_prefs_out_of_range_are_clamped_on_load() {
        for (json, want_font, want_line_height) in [
            (r#"{"editor_font_size": 99, "line_height": 99}"#, 24.0, 2.0),
            (r#"{"editor_font_size": 0, "line_height": 0}"#, 12.0, 1.2),
            (
                r#"{"editor_font_size": -5.0, "line_height": -3.0}"#,
                12.0,
                1.2,
            ),
            // 端点本身合法,clamp 不动
            (r#"{"editor_font_size": 12, "line_height": 1.2}"#, 12.0, 1.2),
            (r#"{"editor_font_size": 24, "line_height": 2.0}"#, 24.0, 2.0),
        ] {
            let dir = temp_dir("font-prefs-clamp");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(SETTINGS_FILE), json.as_bytes()).unwrap();
            let loaded = ThemeSettings::load_from(&dir).unwrap();
            assert_eq!(loaded.editor_font_size, want_font, "{json}");
            assert_eq!(loaded.line_height, want_line_height, "{json}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// #23 钳制纯函数的边界行为(不经过 JSON,直接调函数):闭区间的
    /// 端点保留;`inf` 钳到端点;`NaN` 回落默认 —— `f32::clamp` 对 NaN 是
    /// 穿透的,不设防线 NaN 会一路进渲染。
    #[test]
    fn clamp_font_prefs_functions_handle_bounds_inf_and_nan() {
        assert_eq!(clamp_editor_font_size(15.0), 15.0, "界内恒等");
        assert_eq!(clamp_editor_font_size(f32::INFINITY), 24.0);
        assert_eq!(clamp_editor_font_size(f32::NEG_INFINITY), 12.0);
        assert_eq!(
            clamp_editor_font_size(f32::NAN),
            EDITOR_FONT_SIZE_DEFAULT,
            "NaN 回落默认而非穿透"
        );
        assert_eq!(clamp_line_height(1.5), 1.5, "界内恒等");
        assert_eq!(clamp_line_height(f32::INFINITY), 2.0);
        assert_eq!(
            clamp_line_height(f32::NAN),
            LINE_HEIGHT_DEFAULT,
            "NaN 回落默认而非穿透"
        );
        assert_eq!(clamp_line_height(f32::NEG_INFINITY), 1.2, "负无穷钳到下界");
    }

    #[test]
    fn skin_switch_updates_app_surfaces_and_preserves_density_and_typography() {
        let ctx = egui::Context::default();
        let mut settings = ThemeSettings {
            editor_font_size: 19.0,
            line_height: 1.8,
            ..Default::default()
        };
        settings.apply(&ctx, ThemeMode::Light);
        let geometry = ctx.global_style().spacing.clone();
        for name in [Some("Nord"), Some("Rosé Pine"), None] {
            settings.skin = name.map(str::to_owned);
            settings.apply(&ctx, ThemeMode::Light);
            let palette = settings.shell_palette();
            for mode in [egui::Theme::Light, egui::Theme::Dark] {
                let style = ctx.style_of(mode);
                let colors = palette.colors(mode == egui::Theme::Dark);
                assert_eq!(style.visuals.panel_fill, colors.sidebar);
                assert_eq!(style.visuals.window_fill, colors.content);
                assert_eq!(style.visuals.hyperlink_color, colors.accent);
                assert_eq!(style.visuals.weak_text_color(), colors.secondary);
                assert_eq!(style.visuals.selection.bg_fill, colors.selected_bg);
                assert_eq!(style.spacing.interact_size, geometry.interact_size);
                assert_eq!(style.spacing.item_spacing, geometry.item_spacing);
                assert_eq!(style.text_styles[&egui::TextStyle::Monospace].size, 19.0);
            }
            let output = ctx.run_ui(Default::default(), |ui| {
                assert_eq!(shell(ui), palette.light, "手绘控件与 egui 控件必须共享颜色");
            });
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn full_skin_roundtrip_keeps_shell_when_renamed_and_legacy_keeps_document_edits() {
        let dir = std::env::temp_dir().join(format!("latermd-full-skin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let style = MarkdownStyle {
            block_spacing: 23.0,
            ..default_markdown_style()
        };
        let palette = crate::theme_presets::shell_palette("Nord").unwrap();
        let path = export_skin(&dir, "Nord copy", &style, palette).unwrap();
        std::fs::rename(path, dir.join("themes/Custom.ron")).unwrap();
        let legacy = ron::ser::to_string(&style).unwrap();
        std::fs::write(dir.join("themes/Nord.ron"), &legacy).unwrap();
        let catalog = SkinCatalog::load_from(&dir);
        let mut settings = ThemeSettings::default();
        settings.select_skin(Some("Custom"), &catalog);
        assert_eq!(settings.shell_palette(), palette);
        assert_eq!(settings.markdown_style(), style);
        settings.save_to(Some(&dir)).unwrap();
        let mut loaded = ThemeSettings::load_from(&dir).unwrap();
        let name = loaded.skin.clone();
        loaded.select_skin(name.as_deref(), &catalog);
        assert_eq!(loaded.shell_palette(), palette);
        settings.select_skin(Some("Nord"), &catalog);
        assert_eq!(settings.shell_palette(), palette, "旧预设自动获得外壳");
        assert_eq!(settings.markdown_style(), style, "保留用户正文修改");
        assert_eq!(
            std::fs::read_to_string(dir.join("themes/Nord.ron")).unwrap(),
            legacy
        );
        settings.select_skin(None, &catalog);
        assert_eq!(settings.shell_palette(), ShellPalette::default());
        assert_eq!(settings.markdown_style(), default_markdown_style());
        assert!(
            parse_skin("(markdown: (), shell: ())").is_err(),
            "坏的完整皮肤不能吞成默认正文"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// #23 F3:字号投影到 `TextStyle::Monospace` 档(编辑器面板/行号槽/
    /// Live 活动块源码的字体档),Light/Dark 两套 style 都生效;族保持
    /// 等宽族(CJK 回退挂其链尾,只改 size 不换族);**值变了即重投影**
    /// —— staleness 检查按值键控而非一次性标志(apply_shell 式),滑杆
    /// 每拖一步都当帧生效。读侧 [`editor_font_size`] 与投影同源。
    #[test]
    fn apply_projects_font_size_to_monospace_style_and_reprojects_on_change() {
        let ctx = egui::Context::default();
        let mono_of = |ctx: &egui::Context| {
            ctx.style_of(egui::Theme::Dark)
                .text_styles
                .get(&egui::TextStyle::Monospace)
                .expect("出厂 Monospace 档恒存在")
                .clone()
        };
        let factory = mono_of(&ctx);
        assert_ne!(
            factory.size, 18.0,
            "防御:出厂 Monospace 档恰为 18 时本测试失去鉴别力(egui 0.36 实为 13.0)"
        );
        assert_eq!(
            factory.family,
            egui::FontFamily::Monospace,
            "防御:出厂档即等宽族"
        );

        // 投影 18:两套 style 的档位 size 换成用户值,族不动
        ThemeSettings {
            editor_font_size: 18.0,
            ..ThemeSettings::default()
        }
        .apply(&ctx, ThemeMode::Dark);
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let style = ctx.style_of(theme);
            let mono = style
                .text_styles
                .get(&egui::TextStyle::Monospace)
                .expect("出厂 Monospace 档恒存在");
            assert_eq!(mono.size, 18.0, "{theme:?}: Monospace 档投影用户字号");
            assert_eq!(
                mono.family,
                egui::FontFamily::Monospace,
                "{theme:?}: 投影只动 size,族保持等宽族(CJK 回退链挂其链尾)"
            );
        }
        assert_eq!(
            editor_font_size(&ctx),
            18.0,
            "读侧与投影同源(预览 FontId 的 size 来源)"
        );

        // 同一 context 上换字号(滑杆第二步/载入新 settings):当帧重投影
        for (size, expected) in [(13.0, 13.0), (24.0, 24.0), (12.0, 12.0)] {
            ThemeSettings {
                editor_font_size: size,
                ..ThemeSettings::default()
            }
            .apply(&ctx, ThemeMode::Dark);
            assert_eq!(
                mono_of(&ctx).size,
                expected,
                "字号变更必须即时重投影,不是只投影一次的一次性标志"
            );
        }
        assert_eq!(editor_font_size(&ctx), 12.0);
    }

    /// #23 F3:行距覆盖优先级 —— 滑杆(显式用户意图)、皮肤、overrides、
    /// 出厂默认,从左到右依次让位。overrides 里显式写的 `line_height_ratio`
    /// 同样被用户值盖过,但 overrides 的**其他**字段(block_spacing)原样生效。
    #[test]
    fn apply_overrides_line_height_ratio_over_skins_and_overrides() {
        let ctx = egui::Context::default();
        // 出厂默认(用户没动过滑杆,1.5):生效 ratio 即默认偏好,不是 vendored 的 1.30
        ThemeSettings::default().apply(&ctx, ThemeMode::Dark);
        assert_eq!(
            egui_markdown_style::global_style(&ctx).line_height_ratio,
            LINE_HEIGHT_DEFAULT,
            "默认安装的生效行距 = 用户偏好的默认 1.5"
        );

        // overrides 显式写 1.30(vendored 出厂值):用户 1.7 仍覆盖之
        let overrides = MarkdownStyle {
            block_spacing: 11.0,
            line_height_ratio: 1.30,
            ..MarkdownStyle::default()
        };
        ThemeSettings {
            line_height: 1.7,
            overrides: Some(overrides),
            ..ThemeSettings::default()
        }
        .apply(&ctx, ThemeMode::Dark);
        let installed = egui_markdown_style::global_style(&ctx);
        assert_eq!(
            installed.line_height_ratio, 1.7,
            "用户行距在 overrides 之上再覆盖"
        );
        assert_eq!(
            installed.block_spacing, 11.0,
            "overrides 的其他字段不被行距覆盖殃及"
        );

        // 皮肤(优先级最高的样式源)同样只输给行距覆盖:皮肤里调的
        // line_height_ratio 在皮肤被选中期间不生效,滑杆是唯一行距真源。
        let skin = MarkdownStyle {
            block_spacing: 19.0,
            line_height_ratio: 1.30,
            ..MarkdownStyle::default()
        };
        let settings = ThemeSettings {
            line_height: 1.5,
            skin: Some("我的皮肤".to_owned()),
            skin_style: Some(skin),
            ..ThemeSettings::default()
        };
        settings.apply(&ctx, ThemeMode::Dark);
        let installed = egui_markdown_style::global_style(&ctx);
        assert_eq!(
            installed.block_spacing, 19.0,
            "皮肤的其他字段仍生效(优先级链未被整体推翻)"
        );
        assert_eq!(
            installed.line_height_ratio, 1.5,
            "行距唯一真源是用户滑杆,皮肤值不生效"
        );
    }

    /// #23 F5:行距滑杆与标题呼吸间距**正交** —— `apply` 的行距覆盖只写
    /// `line_height_ratio` 一个字段,`heading_space_above`(F4 vendored
    /// 新字段,标题上方 spacer 行)属于「节奏/间距」维,不随行距覆盖被
    /// 吞掉或顶掉。两字段语义正交:行距管**行内**(行盒高随字号重算,
    /// `line_height_for` 按各自 span 字号求),标题间距管**块间**
    /// (spacer 行高 = `block_spacing + heading_space_above` 定值,不吃行距
    /// 倍率)。双向钉:动滑杆不动标题间距;换皮肤改标题间距不动行距。
    /// 出厂默认(无皮肤无 overrides)时两者经同一条链路带 app 出厂值
    /// 落进 context(heading_space_above 10.0 + 用户行距默认)。
    #[test]
    fn line_height_slider_and_heading_space_above_are_orthogonal() {
        let ctx = egui::Context::default();
        // 出厂链路:app 出厂默认(10.0,2026-10-30 自 vendored 4.0 提档)
        // 应穿透到生效样式
        ThemeSettings::default().apply(&ctx, ThemeMode::Dark);
        let factory = egui_markdown_style::global_style(&ctx);
        assert_eq!(
            factory.heading_space_above, 10.0,
            "app 出厂默认穿透出厂链路"
        );
        assert_eq!(
            factory.line_height_ratio, LINE_HEIGHT_DEFAULT,
            "出厂行距仍由用户偏好的默认值决定"
        );

        // 皮肤带自定义标题间距:行距滑杆覆盖行距字段,标题间距原样生效
        let breathing_skin = MarkdownStyle {
            heading_space_above: 11.0,
            block_spacing: 12.0,
            line_height_ratio: 1.30, // 皮肤显式写的行距,必须让位给滑杆
            ..MarkdownStyle::default()
        };
        let skin_settings = |slider: f32| ThemeSettings {
            line_height: slider,
            skin: Some("呼吸皮肤".to_owned()),
            skin_style: Some(breathing_skin.clone()),
            ..ThemeSettings::default()
        };
        for slider in [1.2, 1.5, 1.8, 2.0] {
            skin_settings(slider).apply(&ctx, ThemeMode::Dark);
            let installed = egui_markdown_style::global_style(&ctx);
            assert_eq!(
                installed.line_height_ratio,
                clamp_line_height(slider),
                "行距随滑杆变化"
            );
            assert_eq!(
                installed.heading_space_above, 11.0,
                "滑杆怎么动都不吞标题呼吸间距"
            );
        }

        // 反向:标题间距随皮肤变(9.0→25.0),行距保持滑杆值不动。
        // 旧出厂值 4.0 不进本表 —— 它会被生效链路迁到 10.0,见下方迁移断言。
        for heading_space in [0.0, 5.0, 9.0, 25.0, 40.0] {
            let mut style = breathing_skin.clone();
            style.heading_space_above = heading_space;
            ThemeSettings {
                line_height: 1.7,
                skin: Some("呼吸皮肤".to_owned()),
                skin_style: Some(style),
                ..ThemeSettings::default()
            }
            .apply(&ctx, ThemeMode::Dark);
            let installed = egui_markdown_style::global_style(&ctx);
            assert_eq!(
                installed.heading_space_above, heading_space,
                "标题间距随皮肤生效"
            );
            assert_eq!(installed.line_height_ratio, 1.7, "标题间距怎么变都不动行距");
        }

        // 旧出厂节奏迁移:皮肤/overrides 带着旧出厂值 4.0 落盘时,生效
        // 链路抬到新默认 10.0(与 code_font_size 10→13 同款无感升级);
        // 0.0 等非出厂的显式值不迁(上一环已验)。
        let mut old_factory_skin = breathing_skin.clone();
        old_factory_skin.heading_space_above = 4.0;
        ThemeSettings {
            line_height: 1.5,
            skin: Some("旧出厂皮肤".to_owned()),
            skin_style: Some(old_factory_skin),
            ..ThemeSettings::default()
        }
        .apply(&ctx, ThemeMode::Dark);
        assert_eq!(
            egui_markdown_style::global_style(&ctx).heading_space_above,
            10.0,
            "旧出厂 4.0 经生效链路迁到 10.0"
        );
    }

    /// #23 F3:排版偏好与密度互不覆盖 —— 密度切换(重投影 spacing token)
    /// 不得重置字号档与正文样式;反之改字号/行距不得重置密度投影;来回切
    /// 各自恢复,互不累积。`apply_density` 与 `apply_font_size` 是两个独立
    /// 的 staleness 槽,style 写入面不相交(spacing / text_styles+正文样式)。
    #[test]
    fn font_prefs_and_density_do_not_clobber_each_other() {
        let ctx = egui::Context::default();
        let factory_item_spacing = ctx.style_of(egui::Theme::Dark).spacing.item_spacing;
        let mono_of = |ctx: &egui::Context| {
            ctx.style_of(egui::Theme::Dark)
                .text_styles
                .get(&egui::TextStyle::Monospace)
                .expect("出厂 Monospace 档恒存在")
                .size
        };
        let ratio_of =
            |ctx: &egui::Context| egui_markdown_style::global_style(ctx).line_height_ratio;
        let prefs = |size: f32, ratio: f32, density: Density| ThemeSettings {
            editor_font_size: size,
            line_height: ratio,
            density,
            ..ThemeSettings::default()
        };

        // 起点:紧凑密度 + 自定义排版偏好,三者同时生效
        prefs(18.0, 1.7, Density::Compact).apply(&ctx, ThemeMode::Dark);
        let compacted_item_spacing = ctx.style_of(egui::Theme::Dark).spacing.item_spacing;
        assert!(
            compacted_item_spacing.y < factory_item_spacing.y,
            "防御:紧凑档间距确比出厂小"
        );
        assert_eq!(mono_of(&ctx), 18.0);
        assert_eq!(ratio_of(&ctx), 1.7);

        // 切密度(字号/行距保持):密度重投影,不得重置字号档与正文样式
        prefs(18.0, 1.7, Density::Standard).apply(&ctx, ThemeMode::Dark);
        assert_eq!(
            ctx.style_of(egui::Theme::Dark).spacing.item_spacing,
            factory_item_spacing,
            "宽松档恢复出厂间距"
        );
        assert_eq!(mono_of(&ctx), 18.0, "切密度不得重置字号投影");
        assert_eq!(ratio_of(&ctx), 1.7, "切密度不得重置行距覆盖");

        // 反向:改字号/行距(密度不动),不得重置密度投影
        prefs(12.0, 1.2, Density::Standard).apply(&ctx, ThemeMode::Dark);
        assert_eq!(mono_of(&ctx), 12.0);
        assert_eq!(ratio_of(&ctx), 1.2);
        assert_eq!(
            ctx.style_of(egui::Theme::Dark).spacing.item_spacing,
            factory_item_spacing,
            "改字号/行距不得重置密度投影"
        );

        // 再切回紧凑:两侧各自恢复,互不累积
        prefs(18.0, 1.7, Density::Compact).apply(&ctx, ThemeMode::Dark);
        assert_eq!(
            ctx.style_of(egui::Theme::Dark).spacing.item_spacing,
            compacted_item_spacing
        );
        assert_eq!(mono_of(&ctx), 18.0);
        assert_eq!(ratio_of(&ctx), 1.7);
    }

    /// #23 F3 读侧兜底:未投影过的 context(无头测试直渲预览、未跑
    /// `ThemeSettings::apply`)回落出厂默认字号,而不是 egui Body 档的
    /// 出厂值 —— 预览 FontId 的 size 在两条路径(投影过/未投影)下都来自
    /// 本读侧,兜底值即用户偏好的出厂值。
    #[test]
    fn editor_font_size_reader_falls_back_to_default_before_projection() {
        let ctx = egui::Context::default();
        assert_eq!(
            editor_font_size(&ctx),
            EDITOR_FONT_SIZE_DEFAULT,
            "未投影过的 context 回落出厂默认 15pt"
        );
    }

    /// 首次运行:目录/文件不存在 → Missing;`load` 语义是回落默认,这里只测
    /// 判别本身(不调 `load`,它走真实平台目录)。
    #[test]
    fn missing_file_is_missing_not_corrupt() {
        assert!(matches!(
            ThemeSettings::load_from(&temp_dir("absent")),
            Err(LoadError::Missing)
        ));
    }

    /// 坏 JSON 判为 Corrupt 且消息带路径;不 panic。
    #[test]
    fn corrupt_json_is_reported_with_path() {
        let dir = temp_dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SETTINGS_FILE), b"{oops").unwrap();
        match ThemeSettings::load_from(&dir) {
            Err(LoadError::Corrupt(message)) => {
                assert!(message.contains(SETTINGS_FILE), "{message}")
            }
            other => panic!("期望 Corrupt,得到 {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 手改配置只写 mode:serde(default) 让缺省字段回落,overrides 为 None。
    #[test]
    fn partial_json_fills_defaults() {
        let dir = temp_dir("partial");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SETTINGS_FILE), br#"{"mode":"light"}"#).unwrap();
        assert_eq!(
            ThemeSettings::load_from(&dir).unwrap(),
            ThemeSettings {
                mode: ThemeMode::Light,
                ..ThemeSettings::default()
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 投影:切 egui 主题(context 随即按 light/dark 选 visuals),并把
    /// overrides 装进 context 默认槽;默认深色;幂等。#23 F3 起生效样式
    /// 的 `line_height_ratio` 一律被用户偏好覆盖(overrides 的 1.30 也
    /// 盖成默认 1.5),其余字段逐项保留。
    #[test]
    fn apply_switches_theme_and_installs_markdown_style() {
        let ctx = egui::Context::default();
        assert_eq!(ctx.theme(), egui::Theme::Dark, "egui 默认深色");

        let overrides = MarkdownStyle {
            block_spacing: 11.0,
            ..Default::default()
        };
        ThemeSettings {
            mode: ThemeMode::Light,
            overrides: Some(overrides),
            ..ThemeSettings::default()
        }
        .apply(&ctx, ThemeMode::Light);
        assert_eq!(ctx.theme(), egui::Theme::Light);
        assert!(!ctx.global_style().visuals.dark_mode);
        assert_eq!(egui_markdown_style::global_style(&ctx).block_spacing, 11.0);

        // 无 overrides 时装的就是出厂默认 + 用户行距覆盖;重复 apply 幂等
        ThemeSettings::default().apply(&ctx, ThemeMode::Dark);
        assert_eq!(ctx.theme(), egui::Theme::Dark);
        let mut expected = default_markdown_style();
        expected.line_height_ratio = LINE_HEIGHT_DEFAULT;
        assert_eq!(
            *egui_markdown_style::global_style(&ctx),
            expected,
            "生效样式 = 出厂默认,唯 line_height_ratio 被用户偏好覆盖"
        );
        ThemeSettings::default().apply(&ctx, ThemeMode::Dark);
    }

    /// #30 回归防线:出厂默认正文样式的表格边框与底色可见 —— vendored 渲染
    /// 只在 `stroke_width > 0.0` 时画线、`header_fill`/`zebra_fill` 为 true
    /// 时铺底色,字段被 reset 回 vendored 默认(如改回
    /// `MarkdownStyle::default()`)时此测试红。圆角与控件圆角同源 token。
    #[test]
    fn default_markdown_style_draws_table_borders() {
        let style = ThemeSettings::default().markdown_style();
        assert!(style.table.stroke_width > 0.0, "0.0 = vendored 不画线");
        assert_eq!(
            style.table.corner_radius,
            tokens::RADIUS_MD,
            "与控件圆角同源"
        );
        assert!(style.table.header_fill, "表头行底色开启");
        assert!(style.table.zebra_fill, "数据区隔行底色开启");
    }

    /// #30 评审修复回归:表头/斑马底(faint)必须**看得见**,不只是画得出
    /// ——与 content 每通道差 ≥5(评审实测旧深色值差 1/通道,亮度差 ~0.4%,
    /// 低于均匀大色块的感知阈,底色事实上隐形;浅色旧值 Δ=(5,4,3) 也弱于
    /// 导出 CSS 的 th 底)。浅色并与导出 HTML 的 th 底同值,预览不弱于导出。
    /// 取色源 `visuals.faint_bg_color`(vendored `paint_header_fill` 与
    /// `egui_extras` striped 都从它取)随投影一并钉住。
    #[test]
    fn faint_table_fill_is_visible_against_content() {
        for dark in [true, false] {
            let token = shell_tokens(dark);
            let faint = token.faint.to_array();
            let content = token.content.to_array();
            for channel in 0..3 {
                let delta = (i16::from(faint[channel]) - i16::from(content[channel])).abs();
                assert!(
                    delta >= 5,
                    "dark={dark} 通道 {channel}:faint {faint:?} vs content {content:?},Δ={delta},底色不可辨"
                );
            }
        }
        assert_eq!(
            shell_tokens(false).faint,
            Color32::from_rgb(0xF6, 0xF8, 0xFA),
            "浅色与导出 CSS 的 th 底(#f6f8fa)同源"
        );

        let ctx = egui::Context::default();
        ThemeSettings::default().apply(&ctx, ThemeMode::Dark);
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let visuals = ctx.style_of(theme).visuals.clone();
            assert_eq!(
                visuals.faint_bg_color,
                shell_tokens(visuals.dark_mode).faint,
                "{theme:?}: 投影后的取色源与 token 一致"
            );
        }
    }

    /// 三态解析:定向选择原样返回;`System` 取检测结果,检测不到回落
    /// fallback(Linux 无统一规范时的既定行为,roadmap 风险 #8)。
    #[test]
    fn resolve_maps_system_to_detection_or_fallback() {
        assert_eq!(
            ThemeMode::Light.resolve(Some(ThemeMode::Dark), ThemeMode::Dark),
            ThemeMode::Light,
            "定向选择不看系统"
        );
        assert_eq!(
            ThemeMode::System.resolve(Some(ThemeMode::Light), ThemeMode::Dark),
            ThemeMode::Light
        );
        assert_eq!(
            ThemeMode::System.resolve(None, ThemeMode::Dark),
            ThemeMode::Dark,
            "检测失败回落手动值"
        );
        // fallback 本身是 System 时(极端手改配置)给确定值,不把 System 传下去
        assert_eq!(
            ThemeMode::System.resolve(None, ThemeMode::System),
            ThemeMode::Dark
        );
    }

    /// 系统探测在本机不 panic;结果只可能是 None 或确定的明暗(Unspecified
    /// 与 Err 都已折叠为 None)。真机判定的实测记录由人工补。
    #[test]
    fn system_detection_never_panics() {
        let detected = detect_system_mode();
        assert!(detected.is_none_or(|mode| mode == ThemeMode::Light || mode == ThemeMode::Dark));
    }

    /// 皮肤文件:导出 → 扫描 → 命中;名字里的路径分隔符被清掉(用户输入
    /// 直接拼进路径,必须挡住 `..`)。
    #[test]
    fn skin_round_trip_through_themes_dir() {
        let dir = temp_dir("skins");
        let style = MarkdownStyle {
            block_spacing: 17.0,
            ..MarkdownStyle::default()
        };
        let path = export_skin(&dir, "我的皮肤", &style, ShellPalette::default()).unwrap();
        assert!(path.ends_with("我的皮肤.ron"), "{path:?}");

        let catalog = SkinCatalog::load_from(&dir);
        assert_eq!(catalog.skins.len(), 1);
        assert_eq!(catalog.skins[0].name, "我的皮肤");
        assert_eq!(catalog.skins[0].style.block_spacing, 17.0);
        assert!(catalog.find("不存在").is_none());

        // 选中 → 内容进内存;选一个不存在的名字 → 回落无皮肤
        let mut settings = ThemeSettings::default();
        settings.select_skin(Some("我的皮肤"), &catalog);
        assert_eq!(settings.markdown_style().block_spacing, 17.0);
        settings.select_skin(Some("不存在"), &catalog);
        assert_eq!(settings.skin, None);
        assert_eq!(settings.markdown_style(), default_markdown_style());

        // 坏文件与空目录都不 panic
        std::fs::write(dir.join(THEMES_DIR).join("bad.ron"), b"(((").unwrap();
        assert_eq!(SkinCatalog::load_from(&dir).skins.len(), 1, "坏文件跳过");
        assert_eq!(SkinCatalog::load_from(&temp_dir("absent")).skins.len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 皮肤文件名清洗:`..` 与各路分隔符都换掉,空名给默认。
    #[test]
    fn skin_file_name_is_sanitized() {
        assert_eq!(skin_file_name("  "), "my-theme");
        assert_eq!(skin_file_name("../../etc/passwd"), "______etc_passwd");
        assert_eq!(skin_file_name("a:b?c"), "a_b_c");
    }

    /// 密度:切到紧凑后间距真变小,切回标准恢复出厂值(不累积缩放)。
    /// 宽松(原「标准」1.0)档配置:U0 token 投影类测试的设计值基准。
    /// #34 后 `ThemeSettings::default()` = Compact(新「标准」),凡断言
    /// 「token 全量投影」的测试须显式取 1.0 档,勿再依赖 default。
    fn spacious_settings() -> ThemeSettings {
        ThemeSettings {
            density: Density::Standard,
            ..ThemeSettings::default()
        }
    }

    #[test]
    fn density_compacts_spacing_and_restores() {
        let ctx = egui::Context::default();
        let standard = ctx.style_of(egui::Theme::Dark).spacing.item_spacing;

        let compact = ThemeSettings {
            density: Density::Compact,
            ..ThemeSettings::default()
        };
        compact.apply(&ctx, ThemeMode::Dark);
        let compacted = ctx.style_of(egui::Theme::Dark).spacing.item_spacing;
        assert!(compacted.y < standard.y, "{compacted:?} vs {standard:?}");

        spacious_settings().apply(&ctx, ThemeMode::Dark);
        let restored = ctx.style_of(egui::Theme::Dark).spacing.item_spacing;
        assert_eq!(restored, standard, "来回切换不累积缩放");
    }

    /// 选区文字可见性回归:egui 0.36 的 `selection.stroke.color` 同时是
    /// **选中文字的颜色**(TextEdit 把选中字形重涂成它,选中态 label 的
    /// 前景也取它)。曾设成 `Stroke::NONE`,选中文字全透明,蓝底上看不见
    /// 字;两套主题的选中文字都必须不透明、且与选区底色不同。
    #[test]
    fn selection_text_stays_visible_in_both_themes() {
        let ctx = egui::Context::default();
        ThemeSettings::default().apply(&ctx, ThemeMode::Dark);
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let visuals = ctx.style_of(theme).visuals.clone();
            let text = visuals.selection.stroke.color;
            assert_eq!(text.a(), 255, "{theme:?}: 选中文字颜色不透明");
            assert_ne!(
                text, visuals.selection.bg_fill,
                "{theme:?}: 选中文字与选区底色不同"
            );
        }
    }

    /// U0 token 投影:明暗两套 style 的控件圆角/输入框高度与内边距都取
    /// `tokens` 常量(tokens.rs 是唯一数字真源,`apply_shell` 后不再有
    /// 硬编码 6/18/4/1 的影子)。两套必须**同值** —— token 不分明暗。
    #[test]
    fn u0_tokens_project_into_both_styles() {
        let ctx = egui::Context::default();
        spacious_settings().apply(&ctx, ThemeMode::Dark);
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let style = ctx.style_of(theme);
            assert_eq!(
                style.spacing.interact_size.y,
                tokens::INPUT_H,
                "{theme:?}: 输入框高度 = interact_size.y"
            );
            assert_eq!(
                style.spacing.button_padding.x,
                tokens::INPUT_PAD_X,
                "{theme:?}: 输入框水平内边距"
            );
            assert_eq!(
                style.spacing.button_padding.y,
                tokens::INPUT_PAD_Y,
                "{theme:?}: 输入框垂直内边距"
            );
            for widget in [
                style.visuals.widgets.inactive.corner_radius,
                style.visuals.widgets.hovered.corner_radius,
                style.visuals.widgets.active.corner_radius,
                style.visuals.widgets.open.corner_radius,
            ] {
                assert_eq!(
                    widget,
                    egui::CornerRadius::same(tokens::RADIUS_MD as u8),
                    "{theme:?}: 控件圆角 = RADIUS_MD"
                );
            }
            let small = style
                .text_styles
                .get(&egui::TextStyle::Small)
                .expect("出厂 Small 档存在");
            assert_eq!(small.size, tokens::FONT_SM, "{theme:?}: Small 字号");
        }
    }

    /// 密度档不被 U0 token 破坏(#24 口径):紧凑档的控件高度/内边距/
    /// 圆角仍按 0.7/0.7/0.8 缩放——但基准从「egui 出厂值」改为「U0 设计值」
    /// (36/12/8/6),切换回标准恢复设计值本身,来回切不累积。
    #[test]
    fn u0_tokens_survive_density_round_trip() {
        let ctx = egui::Context::default();
        let compact = ThemeSettings {
            density: Density::Compact,
            ..ThemeSettings::default()
        };
        compact.apply(&ctx, ThemeMode::Dark);
        let style = ctx.style_of(egui::Theme::Dark);
        assert_eq!(style.spacing.interact_size.y, tokens::INPUT_H * 0.7);
        assert_eq!(style.spacing.button_padding.x, tokens::INPUT_PAD_X * 0.7);
        assert_eq!(style.spacing.button_padding.y, tokens::INPUT_PAD_Y * 0.7);
        assert_eq!(
            style.visuals.widgets.inactive.corner_radius,
            egui::CornerRadius::same((tokens::RADIUS_MD * 0.8) as u8)
        );

        spacious_settings().apply(&ctx, ThemeMode::Dark);
        let style = ctx.style_of(egui::Theme::Dark);
        assert_eq!(
            style.spacing.interact_size.y,
            tokens::INPUT_H,
            "回标准不累积"
        );
        assert_eq!(style.spacing.button_padding.x, tokens::INPUT_PAD_X);
    }

    /// 明暗两套 visuals 下,工具条按钮与输入框在真帧里渲染不 panic,且
    /// **按钮**实测高度不低于 `INPUT_H`(egui 的 Button 显式以
    /// `interact_size.y` 为最小高度;TextEdit 的高度公式是「行高+内边距」,
    /// 不走 `interact_size`,其高度由此单独断言:不低于其自然行高,证明
    /// 投影没有把布局压坏)。
    #[test]
    fn toolbar_and_textedit_render_in_both_visuals() {
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let ctx = egui::Context::default();
            spacious_settings().apply(
                &ctx,
                if theme == egui::Theme::Light {
                    ThemeMode::Light
                } else {
                    ThemeMode::Dark
                },
            );
            let mut edit = String::from("预览文本");
            let mut button_height = 0.0;
            let mut edit_height = 0.0;
            ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.horizontal(|ui| {
                    ui.set_min_height(crate::ui::tokens::TOOLBAR_H);
                    let response = ui.button("工具条按钮");
                    button_height = response.rect.height();
                    let response = ui.add(egui::TextEdit::singleline(&mut edit).hint_text("占位"));
                    edit_height = response.rect.height();
                });
            })
            .drop_without_applying_deltas();
            assert!(
                button_height >= tokens::INPUT_H,
                "{theme:?}: 按钮高 {button_height} >= INPUT_H {}",
                tokens::INPUT_H
            );
            assert!(
                edit_height > 0.0 && edit_height.is_finite(),
                "{theme:?}: 输入框高度有效(实际 {edit_height})"
            );
        }
    }

    /// #50 M2 测试 context:与生产同构的两步投影 —— 先「启动装载」
    /// apply(此时 fonts 未就绪,egui 契约 `ctx.fonts` 自首个 run_ui 起
    /// 才可用,行距投影本轮跳过并布防 fonts-ready 回调),再跑一帧
    /// run_ui 且在闭包内 apply(= 每帧 logic:fonts 已在 begin_pass
    /// 实例化、回调已置位,行距投影当帧补上)。行距投影因此已生效。
    fn projected_ctx(size: f32, ratio: f32, dark: bool) -> egui::Context {
        let settings = || ThemeSettings {
            editor_font_size: size,
            line_height: ratio,
            ..ThemeSettings::default()
        };
        let mode = if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        };
        let ctx = egui::Context::default();
        settings().apply(&ctx, mode);
        ctx.run_ui(egui::RawInput::default(), |ui| {
            settings().apply(ui.ctx(), mode);
        })
        .drop_without_applying_deltas();
        ctx
    }

    /// 滑杆拖动当帧生效(#50 M2,#23 F3 手法):投影值精确等于
    /// `max(0, 字号×行距 − 自然行高)` 且非零(出厂 15pt/1.5 在内置
    /// Hack 链头 1.164em 行高下投影为正 —— 值非恒真);字号/行距任一
    /// 变化不 run_ui 直接 apply 即当帧重投影;来回拖动不累积;明暗两套
    /// style 同值。
    #[test]
    fn line_spacing_projects_exact_value_and_reprojects_same_frame() {
        let extra_of = |ctx: &egui::Context| {
            for theme in [egui::Theme::Light, egui::Theme::Dark] {
                let extra = ctx.style_of(theme).spacing.extra_text_line_spacing;
                assert_eq!(
                    extra,
                    ctx.style_of(egui::Theme::Dark)
                        .spacing
                        .extra_text_line_spacing,
                    "{theme:?}: 行距投影必须明暗同值(与字号档同款两套齐写)"
                );
            }
            ctx.style_of(egui::Theme::Dark)
                .spacing
                .extra_text_line_spacing
        };
        let settings = |size: f32, ratio: f32| ThemeSettings {
            editor_font_size: size,
            line_height: ratio,
            ..ThemeSettings::default()
        };

        let ctx = projected_ctx(15.0, 1.5, true);
        let natural = natural_row_height(&ctx, 15.0).expect("首帧后 fonts 已就绪");
        let expected = editor_extra_line_spacing(15.0, 1.5, natural);
        assert_eq!(extra_of(&ctx), expected, "投影值与公式精确一致");
        assert!(
            expected > 0.0,
            "出厂 15pt/1.5 在编辑器等宽链头下投影应为正,实测 {expected}"
        );

        // 行距拖动(1.5 → 2.0 → 1.2):不 run_ui,当帧即重投影
        for ratio in [2.0, 1.2, 1.5] {
            settings(15.0, ratio).apply(&ctx, ThemeMode::Dark);
            assert_eq!(
                extra_of(&ctx),
                editor_extra_line_spacing(15.0, ratio, natural),
                "行距 {ratio}: 拖动当帧生效(非一次性投影)"
            );
        }
        assert_eq!(extra_of(&ctx), expected, "回拖到出厂值不累积");

        // 字号拖动(15 → 18 → 12):自然行高随字号重算,行距同帧跟随
        for size in [18.0, 12.0] {
            settings(size, 1.5).apply(&ctx, ThemeMode::Dark);
            let natural = natural_row_height(&ctx, size).expect("fonts 已就绪");
            assert_eq!(
                extra_of(&ctx),
                editor_extra_line_spacing(size, 1.5, natural),
                "字号 {size}: 字号×行距与自然行高都在变,投影当帧换算"
            );
        }
    }

    /// #50 M2 clamp 下限:纯函数在「字号×行距不足自然行高」的域内投影
    /// 为 0、绝不为负(负值会压缩行盒,行间重叠与字形裁切的根源);
    /// 边界(恰好等于)为 0,正值域保持差值。
    #[test]
    fn extra_line_spacing_clamps_at_zero_below_natural_row_height() {
        assert_eq!(
            editor_extra_line_spacing(15.0, 1.5, 17.5),
            5.0,
            "正差值原样保留"
        );
        assert_eq!(
            editor_extra_line_spacing(15.0, 1.2, 18.0),
            0.0,
            "字号×行距恰等于自然行高 → 投影 0"
        );
        assert_eq!(
            editor_extra_line_spacing(12.0, 1.2, 20.0),
            0.0,
            "字号×行距低于自然行高 → 钳 0,不产生负 extra"
        );
        for size in [12.0, 15.0, 24.0] {
            for ratio in [1.2, 1.5, 2.0] {
                for natural in [0.5, 13.0, 17.5, 30.0] {
                    let extra = editor_extra_line_spacing(size, ratio, natural);
                    assert!(
                        extra >= 0.0,
                        "{size}×{ratio} vs 自然行高 {natural}: extra {extra} 不得为负"
                    );
                }
            }
        }
    }

    /// #50 M2:行距投影不影响键控槽的读侧语义 —— [`editor_font_size`]
    /// 仍从共用槽取字号(槽扩容后读侧同步);未投影 context 回落默认。
    #[test]
    fn shared_staleness_slot_keeps_editor_font_size_reader_working() {
        let ctx = projected_ctx(18.0, 1.7, true);
        assert_eq!(editor_font_size(&ctx), 18.0, "共用槽扩容后读侧仍取字号");

        // 行距变化(字号不变)也走同一槽:读侧不受扰动
        ThemeSettings {
            editor_font_size: 18.0,
            line_height: 1.3,
            ..ThemeSettings::default()
        }
        .apply(&ctx, ThemeMode::Dark);
        assert_eq!(editor_font_size(&ctx), 18.0);

        let fresh = egui::Context::default();
        assert_eq!(editor_font_size(&fresh), EDITOR_FONT_SIZE_DEFAULT);
    }

    // —— S2-3 三栏可辨性(2026-10-08)——

    /// sRGB 相对亮度(WCAG 2.x 定义)。
    fn rel_luminance(c: Color32) -> f32 {
        let channel = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let [r, g, b, _] = c.to_array();
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    /// WCAG 对比度(两色取亮者作分子)。
    fn contrast_ratio(a: Color32, b: Color32) -> f32 {
        let (la, lb) = (rel_luminance(a), rel_luminance(b));
        let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// 三栏的**背景**必须两两可辨 —— 这是 S2-3 改版的全部目的。
    ///
    /// 阈值 1.10:1 是实测反推的:改版前浅色侧栏 vs 内容 = 1.110:1,
    /// 暗色 = 1.122:1,两者的共同 complaint 是「并排看几乎是一片」。
    /// 取 **1.12:1** 作下界,把两侧都抬到刚好脱离那片「看不出差」的区间,
    /// 同时离「明显分层」还远 —— 侧栏是背景层,不该抢内容的注意力。
    ///
    /// 参照物:WCAG 1.4.11 非文本对比度要求 3:1,但那条针对**UI 组件边界**
    /// (按钮、输入框的可点击轮廓),不适用于「相邻面板的背景分区」——
    /// 按 3:1 做会把侧栏压成深灰,产品观感受损。故本断言只锁「可辨」,
    /// 不锁「必须达到无障碍标准」,两者不是一回事。
    #[test]
    fn sidebar_and_content_are_distinguishable_in_both_themes() {
        // macOS 深色以细分隔线划分面板,浅色仍靠两块底色。
        // 保留跨平台旧外壳的色块对比下限。
        const MIN: f32 = 1.12;
        for (label, dark) in [("light", false), ("dark", true)] {
            let t = shell_tokens(dark);
            let ratio = if cfg!(target_os = "macos") && dark {
                contrast_ratio(t.border, t.sidebar)
            } else {
                contrast_ratio(t.sidebar, t.content)
            };
            assert!(
                ratio >= MIN,
                "{label}: 侧栏 vs 内容区对比度 {ratio:.3}:1 < {MIN}:1 —— \
                 三栏会并成一片(S2-3 的存在意义就是防这个)"
            );
        }
    }

    /// 浅色下侧栏必须**浅于 border**:侧栏里的分隔线是「比底色深一档」的
    /// 画法,底色一旦追平或深过分隔线,线就消失、侧栏结构塌掉。
    ///
    /// 这条是 S2-3 浅色侧栏只能到 `#EDEFF2` 的直接原因 —— 再深一档就到
    /// `#E4E6EA`,与 border `#E5E6E8` 撞色。断言把这条约束钉住,防止
    /// 将来有人为了「更明显的分层」继续加深而悄悄毁掉分隔线。
    #[test]
    fn light_sidebar_stays_lighter_than_border() {
        let t = shell_tokens(false);
        let sb = rel_luminance(t.sidebar);
        let bd = rel_luminance(t.border);
        assert!(
            sb > bd,
            "浅色侧栏亮度 {sb:.4} 须高于 border 亮度 {bd:.4} —— \
             否则侧栏内的分隔线不可辨(sidebar={:?} border={:?})",
            t.sidebar,
            t.border
        );
    }

    /// 侧栏与内容区的差必须落在「可辨」与「不过分」之间 —— 双侧断言。
    ///
    /// 上界防的是另一个方向的坑:把 sidebar 一路加深到接近窗口底色,
    /// 侧栏会读成「挖了个洞」而不是「背景层」,三栏从「分不开」变成
    /// 「侧栏太重」。上界按每通道差设(明暗各一条),因为侧栏与内容的
    /// 色差本质是「同一灰阶上的档位差」,用通道差表达比对比度直观。
    #[test]
    fn sidebar_content_delta_stays_in_the_readable_band() {
        // (暗, 最大通道差下限, 上限)
        let bands = if cfg!(target_os = "macos") {
            [("dark", true, 5.0, 10.0), ("light", false, 12.0, 18.0)]
        } else {
            [("dark", true, 12.0, 18.0), ("light", false, 16.0, 22.0)]
        };
        for (label, dark, lo, hi) in bands {
            let t = shell_tokens(dark);
            let delta = t
                .sidebar
                .to_array()
                .iter()
                .zip(t.content.to_array().iter())
                .map(|(a, b)| f32::from(a.abs_diff(*b)))
                .fold(0.0f32, f32::max);
            assert!(
                (lo..=hi).contains(&delta),
                "{label}: 侧栏/内容每通道差 {delta} 不在 [{lo}, {hi}] 区间"
            );
        }
    }
}
