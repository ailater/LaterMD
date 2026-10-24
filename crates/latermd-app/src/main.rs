//! LaterMD 应用入口。
//!
//! 渲染后端选择见 docs/adr-002 §3.4:默认 wgpu;`LATERMD_RENDERER=glow` 仅在
//! 启用 `glow` feature 的构建中生效(驱动黑名单逃生口,不进主产物)。
//! App trait 采用 egui 0.36 的 `logic` / `ui` 二分,三栏布局见 `ui::layout`
//! (docs/adr-005)。

// Windows 发布版按 GUI 子系统链接,双击 exe 不再弹控制台黑框;debug 构建
// 保留控制台,stdout/stderr 可见。MCP stdio 通道不受影响:客户端以管道
// 方式拉起子进程,GUI 子系统下重定向的 stdio 句柄照常可读写。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod ai_config;
mod ai_key;
mod ai_link;
mod assets;
mod backlink_panel;
mod bed;
mod cli;
mod cli_install;
mod clipboard;
mod command;
mod compose;
mod doclink;
mod edit_events;
#[cfg(test)]
mod editor_typography_acceptance;
mod export;
mod file;
mod filetree;
#[cfg(test)]
mod font_metrics_repro;
mod fonts;
mod fuzzy;
mod git_diff;
mod git_panel;
mod git_split_diff;
mod instance;
mod keymap;
mod layout;
mod live;
#[cfg(target_os = "macos")]
mod mac_menu;
// 守门跑在所有平台:它读的是 manifest 里 **macOS 段** 的 feature 分档,
// 而本机(Linux)编不到那段 —— 正因为编不到才要在这里守。
#[cfg(test)]
mod macos_feature_gate;
mod mcp;
#[cfg(test)]
mod packaging_assoc;
mod persist;
#[cfg(test)]
mod preview_pixel_acceptance;
#[cfg(test)]
mod preview_typography_acceptance;
mod repaint;
mod search;
mod settings;
mod shortcut_overlay;
mod state;
mod tabs;
mod theme;
mod theme_presets;
mod ui;

use eframe::egui;
use latermd_mcp::{McpConfig, Server};
use std::path::PathBuf;

/// MCP stdio 通道的开关参数:`latermd --mcp-stdio`(docs/mcp-plan.md §2)。
pub(crate) const MCP_STDIO_ARG: &str = "--mcp-stdio";

/// headless MCP 通道:`latermd --mcp-stdio`,客户端以子进程方式拉起
/// (`claude mcp add latermd -- latermd --mcp-stdio`)。
///
/// 不进 GUI 事件循环、不初始化窗口;stdin 关闭即退出。**不看 `mcp.json`
/// 的 enabled** —— 用户显式用这个参数启动就是一次授权,而 `enabled` 管的是
/// 「GUI 进程内是否自动监听端口」这一件不同的事。
///
/// 文档库根取环境变量 `LATERMD_MCP_ROOT`(headless 没有文件树 UI 可交互);
/// 缺省时工具照常返回「未设置文件树根目录」。
fn run_mcp_stdio() -> eframe::Result<()> {
    let config = theme::config_dir()
        .as_deref()
        .map(McpConfig::load_from)
        .unwrap_or_default();
    let root = std::env::var("LATERMD_MCP_ROOT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from);
    let server = Server::new(root, config);
    match latermd_mcp::transport::stdio::serve(&server) {
        Ok(()) => Ok(()),
        Err(error) => {
            eprintln!("LaterMD: MCP stdio 通道退出: {error}");
            Err(eframe::Error::AppCreation(Box::new(error)))
        }
    }
}

fn main() -> eframe::Result<()> {
    // 子进程式 MCP 通道优先:它不是 GUI 会话,不该初始化窗口与字体,
    // 也**不该参与单实例交接** —— 客户端会同时拉起多个 MCP 子进程
    // (每个会话一个),第二个被判「已在运行」而直接退出,等于工具失灵。
    // 故此分支必须在认领之前。
    if std::env::args_os().any(|arg| arg == MCP_STDIO_ARG) {
        return run_mcp_stdio();
    }
    // 「双击 .md」的三段链路第二段:把 argv 里的路径收下。
    let targets = cli::parse_open_targets(
        &std::env::args_os().collect::<Vec<_>>(),
        std::env::current_dir().ok().as_deref(),
    );
    if let Some(notice) = cli::rejection_notice(&targets.rejected) {
        eprintln!("{notice}");
    }
    // 第三段:交给已有实例就没什么可做了,本进程退出。
    let instance = match instance::try_claim(targets.markdown, theme::config_dir().as_deref()) {
        instance::Claim::Primary(primary) => primary,
        instance::Claim::HandedOff => return Ok(()),
    };
    let boot_paths = instance.paths.clone();
    let renderer = match std::env::var("LATERMD_RENDERER").as_deref() {
        #[cfg(feature = "glow")]
        Ok("glow") => eframe::Renderer::Glow,
        _ => eframe::Renderer::Wgpu,
    };
    // 原生装饰逃生口(docs/ui-shell-redesign.md §2 D1):变量精确为 1 时走
    // 系统标题栏,其余(含未设)去装饰、由 `ui::titlebar` 自绘接管。
    let native_chrome =
        native_decorations(std::env::var("LATERMD_NATIVE_DECORATIONS").ok().as_deref());
    // 布局提前装载:窗口最大化状态(上次退出时)要在 NativeOptions 里就位,
    // 进 run_native 闭包就晚了 —— 窗口创建后再改尺寸会闪一次窗口态切换。
    let boot_layout = layout::LayoutSettings::load();
    let opts = eframe::NativeOptions {
        renderer,
        // 窗口图标(auto-plan #16):解码成功才挂,失败回落 egui 默认图标
        viewport: viewport_builder(assets::window_icon(), boot_layout.maximized, native_chrome),
        ..Default::default()
    };
    eframe::run_native(
        "LaterMD",
        opts,
        Box::new(move |cc| {
            if fonts::install(&cc.egui_ctx).is_none() {
                // M0 验证 UI 已退役,字体失配只在终端告警,不静默吞掉
                eprintln!("LaterMD: 未找到候选 CJK 字体,中文将显示为方块");
            }
            // 图片加载器(docs/image-plan.md B 段):vendored 层的 `images`
            // feature 把 `Token::Image` 画成 `egui::Image`,egui 本体不带任何
            // loader —— 不装这条,本地 file:// 与网络图片都只会是破图占位。
            // file(读盘)+ image(解码)服务本地 .assets/,http 顺带让 A 段
            // 的网络地址也出图(ehttp 原生后端复用已在依赖树里的 ureq)。
            egui_extras::install_image_loaders(&cc.egui_ctx);
            let theme = theme::ThemeSettings::load();
            let system = theme::detect_system_mode();
            // 文件树设置(上次根目录 + 最近列表)同样启动即恢复
            let file_tree = filetree::FileTreeSettings::load();
            // 外壳布局(左右两栏开着与否 + 左栏停在哪个视图)同上,M1 起持久化;
            // maximized 已在 opts 装载过(boot_layout),这里复用同一份
            let layout = boot_layout.clone();
            // 面板宽度恢复(左栏 nav + 只读对照列):egui 面板宽度存在其内部
            // persisted memory(重启即丢),这里在首帧前塞回去。outer_rect 只
            // 可靠使用**尺寸**(egui 注释原话),位置给 ZERO。
            for (bar_id, width) in [
                (egui::Id::new("nav"), boot_layout.left_width),
                (
                    egui::Id::new(crate::ui::layout::COMPARE_PANEL_ID),
                    boot_layout.live_right_width,
                ),
            ] {
                if let Some(w) = width.filter(|w| w.is_finite() && *w > 0.0) {
                    cc.egui_ctx.data_mut(|d| {
                        d.insert_persisted(
                            bar_id,
                            egui::PanelState {
                                outer_rect: egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(w, 0.0),
                                ),
                            },
                        )
                    });
                }
            }
            let mut app =
                LaterMdApp::from_boot(theme, file_tree, layout, boot_paths.clone(), *instance);
            #[cfg(target_os = "macos")]
            {
                app.native_menu = mac_menu::NativeMenu::install(&cc.egui_ctx, &app.state);
            }
            app.frameless = !native_chrome;
            app.state.system_theme = system;
            app.state.system_theme_ok = system.is_some();
            // load_preferences 已恢复完整皮肤，首帧直接使用保存的应用色板。
            app.state
                .theme
                .apply(&cc.egui_ctx, app.state.resolved_theme());
            Ok(Box::new(app))
        }),
    )
}

/// Settings 面板显示的当前渲染后端(AGENTS.md §5)。只镜像 `main` 的启动
/// 选择,同一套规则:仅 `glow` feature 构建且变量精确为 `glow` 才是 glow,
/// 未设/值不认/feature 未开一律 wgpu。`env` 由调用方传入以便无头测试。
pub(crate) fn renderer_label(env: Option<&str>) -> &'static str {
    match env {
        #[cfg(feature = "glow")]
        Some("glow") => "glow",
        _ => "wgpu",
    }
}

/// macOS 默认使用系统窗口按钮;其它平台仍支持原生装饰逃生口。
fn native_decorations(env: Option<&str>) -> bool {
    cfg!(target_os = "macos") || env == Some("1")
}

/// 视口构建(与 `main` 的启动选择同源):窗口图标仅在解码成功(`Some`)时
/// 挂上,失败回落无图标的默认 builder,启动路径零额外风险(auto-plan #16)。
/// 三个入参由调用方传入,Some/None 两分支因此可在无头单测里穷尽。
fn viewport_builder(
    icon: Option<egui::IconData>,
    maximized: bool,
    native_chrome: bool,
) -> egui::ViewportBuilder {
    let builder = egui::ViewportBuilder::default()
        // 三栏的最小可用宽度:侧边栏下限 160 + 编辑器 500 + 预览余量(docs/adr-005)
        .with_min_inner_size([900.0, 600.0])
        .with_decorations(native_chrome)
        .with_maximized(maximized);
    let builder = if cfg!(target_os = "macos") && native_chrome {
        builder
            .with_fullsize_content_view(true)
            .with_title_shown(false)
            .with_titlebar_shown(false)
            .with_titlebar_buttons_shown(true)
            .with_movable_by_background(false)
    } else {
        builder
    };
    match icon {
        Some(icon) => builder.with_icon(icon),
        None => builder,
    }
}

/// 应用根:状态 + 待归约消息队列。归约在 [`eframe::App::logic`],绘制在
/// [`eframe::App::ui`]。后台任务通道(P1,docs/adr-005 §5.2)将来汇入同一队列。
#[derive(Default)]
struct LaterMdApp {
    #[cfg(target_os = "macos")]
    native_menu: Option<mac_menu::NativeMenu>,
    state: state::State,
    outbox: Vec<state::Message>,
    /// 单实例主控:启动参数的路径已由 [`LaterMdApp::from_boot`] 打开,
    /// 这里只负责收后续实例转发来的路径(`instance::Primary::take_incoming`,
    /// 每帧 `reduce` 消费)。
    ///
    /// `None` = 本进程不是主实例(无配置目录、绑不上锁、或点启动器图标
    /// 开的新窗口)。持锁者一旦被丢弃会立刻删锁关掉单实例能力,所以必须
    /// 由 App 全程持有到 `Drop`。
    instance: Option<instance::Primary>,
    /// 最近一次下发给原生窗口的标题缓存;仅用于跳过重复的 set_title。
    window_title: String,
    /// 无边框模式(自绘标题栏 + 边缘缩放命令区);`main` 启动时按
    /// `LATERMD_NATIVE_DECORATIONS` 读一次,原生装饰路径不画任何自绘 chrome。
    frameless: bool,
    /// 仅供测试的探针:`draw` 时把格式工具条每个按钮的矩形吐出来。
    ///
    /// 生产恒为 `None`(零开销)。存在的理由与 `ui::sidebar` 的
    /// `SidebarBands` 相同:那些手绘按钮没有可从外部读的名字,而坐标由
    /// 布局演算决定 —— 想让无头测试点到真按钮,只能让这条路走一遭。
    #[cfg(test)]
    format_probe: Option<Box<dyn FnMut(crate::compose::FormatAction, egui::Rect)>>,
    /// 仅供测试的探针:`draw_zen` 时把正文层实测到的内容宽度吐出来。
    ///
    /// 同上。720 限宽发生在 `CentralPanel` 内部的 `vertical_centered` 那一
    /// 层,从外面读不到 —— 想在测试里验「限宽真的生效」。要么在这里留个口,
    /// 要么在测试里重演同一段布局(而后者验证不了 `draw_zen` 有没有真调)。
    #[cfg(test)]
    zen_probe: Option<Box<dyn FnMut(f32)>>,
    /// 仅供测试的探针:`ui::zen_nav` 画出的悬停标签导航列矩形。
    ///
    /// 同上:行是手绘 widget,坐标由动画位移决定,无头测试要点击真行只能
    /// 让这条路走一遭;全隐帧不调用——测试据此断言「零导航元素」。
    #[cfg(test)]
    zen_nav_probe: Option<Box<dyn FnMut(egui::Rect)>>,
}

impl LaterMdApp {
    /// 以启动时装载的主题与文件树设置建应用(重启保持)。`Default` 恒为
    /// 使用出厂设置且不走磁盘,仅供测试。
    ///
    /// `boot_paths` 是「双击 .md」链路第三段:启动参数里的路径,**在建
    /// 构期内**就归约掉(`FileSelected`),于是首帧呈现时内容已在,没有
    /// 「空窗闪一下再跳到文档」的一帧。走 `FileSelected` 与文件树点击
    /// 同一条 `open_path`(同路径激活已有标签、展开树内祖先目录),
    /// 不走任何旁路。
    ///
    /// `instance` 是单实例主控:它只负责收后续实例转发来的路径,由每帧
    /// `reduce` 消费;启动参数里的路径已经在上面落地,故 `paths` 字段
    /// 在交接后即被消费掉。
    fn from_boot(
        theme: theme::ThemeSettings,
        file_tree: filetree::FileTreeSettings,
        layout: layout::LayoutSettings,
        boot_paths: Vec<PathBuf>,
        instance: instance::Primary,
    ) -> Self {
        let mut app = Self::default();
        app.state.theme = theme;
        app.state.file_tree = file_tree.into();
        app.state.restore_layout(layout);
        // 凭据状态启动即探测:设置浮窗状态行首见即真(后端不可用的
        // Linux 环境直接给回退提示,而不是「未配置」的误报)
        // 先探测凭据后端,再装载 AI 配置 —— 装配 provider 运行时要读 key
        app.state.ai_key.probe();
        app.state.load_preferences();
        app.instance = Some(instance);
        for path in boot_paths {
            app.state.apply(state::Message::FileSelected(path));
        }
        app
    }
}

#[cfg(test)]
mod tests {
    use super::{native_decorations, renderer_label, viewport_builder, MCP_STDIO_ARG};
    use crate::instance;
    use std::path::PathBuf;

    /// 临时目录 + 真笔记文件:`from_boot` 要经 `open_path` 读盘,用假路径
    /// 只会验到「读取失败落提示行」,验不出「内容真的到了编辑器」。
    struct Vault(PathBuf);

    impl Vault {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "latermd-boot-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).expect("建临时目录");
            Self(base)
        }

        fn note(&self, name: &str, body: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, body).expect("写临时笔记");
            path
        }
    }

    impl Drop for Vault {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 认领一个不带收件端的主实例当构造参数(`from_boot` 的第五参)。
    /// 用真实 `try_claim` 而非手工构造:手工造出来的 `Primary` 不是生产
    /// 会有的那一个(此前教训:裸调单函数 + 手工造状态 = 测不存在的路径)。
    fn primary_without_incoming() -> instance::Primary {
        let claim = instance::try_claim(Vec::new(), None);
        match claim {
            instance::Claim::Primary(primary) => *primary,
            instance::Claim::HandedOff => panic!("无配置目录时不该交接"),
        }
    }

    /// 「双击 a.md」→ 首帧呈现时标签已在,内容已读进缓冲。
    ///
    /// 这是本模块的**主守门**:走真实 `from_boot`(生产唯一构造器)+
    /// 真实文件落盘。此前变异验证过:把 `from_boot` 里那圈 `apply` 删掉,
    /// 本测试当场红(标签数不变、缓冲是空的)—— 突变前它是恒绿的。
    #[test]
    fn boot_path_is_open_before_first_frame() {
        let vault = Vault::new("boot");
        let note = vault.note("a.md", "# 标题\n\n正文。\n");

        let app = super::LaterMdApp::from_boot(
            super::theme::ThemeSettings::default(),
            super::filetree::FileTreeSettings::default(),
            super::layout::LayoutSettings::default(),
            vec![note.clone()],
            primary_without_incoming(),
        );

        assert_eq!(
            app.state.tabs.current().document.path.as_deref(),
            Some(note.as_path()),
            "启动参数里的路径应成为当前标签"
        );
        assert_eq!(
            app.state.tabs.current().editor.text(),
            "# 标题\n\n正文。\n",
            "内容必须真读进缓冲,不只是路径登记上了"
        );
    }

    /// 多个启动参数路径 → 各开一个标签,顺序即 argv 顺序。
    #[test]
    fn multiple_boot_paths_open_one_tab_each() {
        let vault = Vault::new("multi");
        let first = vault.note("one.md", "# 一\n");
        let second = vault.note("two.md", "# 二\n");

        let app = super::LaterMdApp::from_boot(
            super::theme::ThemeSettings::default(),
            super::filetree::FileTreeSettings::default(),
            super::layout::LayoutSettings::default(),
            vec![first.clone(), second.clone()],
            primary_without_incoming(),
        );

        // 出厂就有一个未命名标签,故是 1 + 2。
        assert_eq!(app.state.tabs.tabs.len(), 3, "两个参数各开一个标签");
        assert_eq!(
            app.state.tabs.current().document.path.as_deref(),
            Some(second.as_path()),
            "argv 后者成为当前标签(与标签栏顺序一致)"
        );
    }

    /// 无参数启动(点启动器图标)→ 仍是出厂那一个未命名标签,不开文件。
    /// 这条守的是「单实例改造没顺手把空启动也改了」。
    #[test]
    fn empty_boot_leaves_factory_state() {
        let app = super::LaterMdApp::from_boot(
            super::theme::ThemeSettings::default(),
            super::filetree::FileTreeSettings::default(),
            super::layout::LayoutSettings::default(),
            Vec::new(),
            primary_without_incoming(),
        );
        assert_eq!(app.state.tabs.tabs.len(), 1, "出厂就一个未命名标签");
        assert!(app.state.tabs.current().document.path.is_none());
    }

    /// 同一路径给两次 → 只一个标签(`open_path` 的去重语义,启动路径
    /// 与文件树点击共用)。系统在某些平台上确实会这么传。
    #[test]
    fn duplicate_boot_path_yields_single_tab() {
        let vault = Vault::new("dup");
        let note = vault.note("same.md", "# 同一篇\n");

        let app = super::LaterMdApp::from_boot(
            super::theme::ThemeSettings::default(),
            super::filetree::FileTreeSettings::default(),
            super::layout::LayoutSettings::default(),
            vec![note.clone(), note.clone()],
            primary_without_incoming(),
        );
        assert_eq!(app.state.tabs.tabs.len(), 2, "出厂 1 + 去重后只加 1");
    }

    /// 判定与 `main` 的启动选择同规则:认不认 `glow` 取决于 glow feature
    /// (两条产线都会跑到对应分支);值精确匹配小写,大小写变体不认。
    #[test]
    fn renderer_label_follows_feature_and_exact_env_value() {
        #[cfg(feature = "glow")]
        assert_eq!(renderer_label(Some("glow")), "glow");
        #[cfg(not(feature = "glow"))]
        assert_eq!(renderer_label(Some("glow")), "wgpu");
        assert_eq!(renderer_label(Some("wgpu")), "wgpu");
        assert_eq!(renderer_label(Some("GLOW")), "wgpu");
        assert_eq!(renderer_label(None), "wgpu");
    }

    /// 原生装饰逃生口与变量精确匹配:仅 `1` 回落系统标题栏,未设、`0`、
    /// 大小写变体都走自绘(与 `LATERMD_RENDERER` 同款口径)。
    #[test]
    fn native_decorations_requires_exact_env_value() {
        assert!(native_decorations(Some("1")));
        for value in [Some("0"), Some("true"), None] {
            assert_eq!(native_decorations(value), cfg!(target_os = "macos"));
        }
    }

    /// `--mcp-stdio` 只在精确匹配时生效(其它参数照常进 GUI);参数常量与
    /// main 的判定同源,避免两边写死两份字符串。
    #[test]
    fn mcp_stdio_arg_is_matched_exactly() {
        let args = |args: &[&str]| {
            args.iter()
                .map(|arg| (*arg).to_owned())
                .any(|arg| arg == MCP_STDIO_ARG)
        };
        assert!(args(&["latermd", "--mcp-stdio"]));
        assert!(!args(&["latermd"]));
        assert!(!args(&["latermd", "--mcp-stdio=1"]), "精确匹配");
    }

    /// 视口构建的图标分支(auto-plan #16):`Some` → 图标挂上且像素原样;
    /// `None` → icon 缺位回落默认 builder,不 panic。最小尺寸/装饰/最大化
    /// 三个既有启动参数接线前后同值,Some/None 两分支一致 —— 接线不改
    /// 变原启动语义。
    #[test]
    fn viewport_builder_attaches_icon_only_when_some() {
        let icon = super::egui::IconData {
            width: 2,
            height: 2,
            rgba: vec![7; 16],
        };

        let with = viewport_builder(Some(icon.clone()), true, false);
        let attached = with.icon.as_ref().expect("Some 应挂上图标");
        assert_eq!((attached.width, attached.height), (2, 2));
        assert_eq!(attached.rgba, icon.rgba, "像素原样透传");

        let without = viewport_builder(None, true, false);
        assert!(without.icon.is_none(), "None 回落默认 builder,无图标");

        for builder in [with, without] {
            assert_eq!(builder.maximized, Some(true));
            assert_eq!(builder.decorations, Some(false));
            assert_eq!(
                builder.min_inner_size,
                Some(super::egui::vec2(900.0, 600.0)),
                "既有最小尺寸不动"
            );
        }
    }
}
