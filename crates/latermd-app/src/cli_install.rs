//! macOS 命令行入口的安装核(2026-10-23 mac 反馈 #3:终端里
//! `latermd xxx.md` 直开文档)。
//!
//! ## 现状与缺口
//!
//! argv 解析与单实例移交早已在库([`crate::cli`],「双击 .md」三段链路
//! 的第二/三段):`latermd <路径.md>` 从命令行跑起来本就走得通,缺的
//! 只是 macOS 上**没有命令在 PATH 里** —— .app 拖装不往 /usr/local/bin
//! 放任何东西(Linux 走 deb 装 /usr/bin,Windows 无此惯例)。本模块补
//! 「把软链放进 PATH」这一步:软链直指 .app 内的二进制,文件参数由
//! 既有单实例协议转交给运行中的窗口(没有窗口就自己成为主实例)。
//!
//! ## 两层引号
//!
//! 路径要穿过两层语法:先入 POSIX shell([AppleScript `do shell script`]
//! 的载荷,单引号包裹、内嵌单引号 `'\''` 转义),再入 AppleScript 字符串
//! (双引号包裹、`\` 与 `"` 反斜杠转义)。两个转义各自独立成纯函数并
//! 逐字钉死 —— 应用装在带空格/引号的路径(如 `~/My Apps/`)下也不许
//! 拼出歪命令。
//!
//! ## 卡帧纪律
//!
//! osascript 会弹管理员授权窗并**等用户输密码**,绝不能在归约里同步做:
//! 与剪贴板图片读取([`crate::clipboard`])同款,spawn 后台线程 + mpsc,
//! 每帧归约 `try_recv`([`CliInstallState::poll`]),结果只落提示行。
//!
//! ## 平台口径
//!
//! 本模块不 cfg:纯函数(命令拼装/转义/bundle 判定/进程执行)全平台
//! 编译全平台测试(mac CI 编得到执行路径,Linux CI 编得到解析与生命
//! 周期);只有发起入口(mac 原生菜单「安装命令行工具…」)在
//! `crate::mac_menu`(macOS cfg 模块,跨平台 rustdoc 解析不到,不做成
//! 链接)里。开发裸跑(非 .app)不安装:装出去的软链
//! 指向 target/debug 的临时二进制,重编译即失效,归约侧先过
//! [`is_bundled_macos_app`] 再放行。
//!
//! [AppleScript `do shell script`]: https://developer.apple.com/library/archive/documentation/AppleScript/Conceptual/AppleScriptLangGuide/reference/ASLR_cmds.html

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use crate::repaint::RepaintNotifier;
use crate::state::Message;

/// 软链落点:全 mac 的默认 PATH 都含它(Apple Silicon 亦然),目录不存在
/// 时由安装命令 `mkdir -p` 创建(带管理员授权)。
pub(crate) const LINK_DIR: &str = "/usr/local/bin";

/// 链接名与打包产物二进制同名(build.sh 的 `-p latermd-app --bin
/// latermd` 钉的同一条产线);`ln` 覆盖式重建,重复点菜单幂等。
pub(crate) const LINK_NAME: &str = "latermd";

/// POSIX 单引号转义:除内嵌单引号(`'\''` 出坑再入坑)外任何字符在
/// 单引号里都安全;返回值两侧自带引号。
pub(crate) fn shell_single_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// AppleScript 字符串字面量:双引号包裹,`\` 与 `"` 前加反斜杠。
pub(crate) fn apple_string_literal(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// 安装载荷的 shell 命令:`mkdir -p` 兜底目录,再 `-f` 覆盖式重建软链
/// (升级重装后 .app 路径不变,软链无需先删)。
pub(crate) fn install_shell_command(exe: &Path) -> String {
    format!(
        "mkdir -p {LINK_DIR} && ln -sf {} {LINK_DIR}/{LINK_NAME}",
        shell_single_quote(&exe.to_string_lossy())
    )
}

/// 完整 osascript 参数:管理员授权执行安装命令(授权弹窗由 osascript
/// 自己弹,密码不经过本进程)。
pub(crate) fn osascript_argv(shell_command: &str) -> Vec<String> {
    vec![
        "-e".to_owned(),
        format!(
            "do shell script {} with administrator privileges",
            apple_string_literal(shell_command)
        ),
    ]
}

/// exe 是否从 .app bundle 里跑出来的:路径里存在连续的
/// `*.app / Contents / MacOS` 三段。开发裸跑(target/debug/latermd)
/// 与裸二进制都不满足,归约侧据此拒绝安装。
pub(crate) fn is_bundled_macos_app(exe: &Path) -> bool {
    let names: Vec<String> = exe
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    names.windows(3).any(|window| {
        window[0].ends_with(".app") && window[1] == "Contents" && window[2] == "MacOS"
    })
}

/// 跑一个外部命令并归一化结果;失败优先回传 stderr(授权拒绝、目录
/// 不写这类可读信息都在那里),空 stderr 回退到退出码。
pub(crate) fn run_command(program: &str, args: &[String]) -> Result<(), String> {
    match std::process::Command::new(program).args(args).output() {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            Err(if stderr.is_empty() {
                format!("{program} 退出码非零({})", output.status)
            } else {
                stderr
            })
        }
        Err(error) => Err(format!("无法启动 {program}:{error}")),
    }
}

/// 一次安装的执行体:osascript 弹管理员授权跑安装命令。非 macOS 直接
/// 报错(发起入口本就只在 mac 菜单,这里兜住归约测试与误触发)。
pub(crate) fn run_install(shell_command: &str) -> Result<(), String> {
    if cfg!(not(target_os = "macos")) {
        return Err("命令行工具安装仅在 macOS 提供".to_owned());
    }
    run_command("/usr/bin/osascript", &osascript_argv(shell_command))
        .map_err(|error| friendly_install_error(&error))
}

/// osascript 报错的翻译:用户取消授权(关掉密码窗)是最常见的合法
/// 退出,原文「execution error: User canceled. (-128)」对中文用户不
/// 友好;其余原样透传。
pub(crate) fn friendly_install_error(raw: &str) -> String {
    if raw.contains("User canceled") {
        "已取消授权,未安装".to_owned()
    } else {
        raw.to_owned()
    }
}

/// 一次安装的结果载荷:成功即已建链,失败为面向用户的错误文案。
type InstallResult = Result<(), String>;

/// 命令行安装状态机:与 [`crate::clipboard::ClipboardState`] 同构的
/// 发起/收流/收尾三原语,一次「菜单点选 → 后台授权安装 → 落提示行」
/// 的生命周期。
#[derive(Default)]
pub struct CliInstallState {
    /// 在途安装的接收端;`None` = 空闲。`pub(crate)` 仅为测试注入空壳
    /// (钉归约侧收尾而不真起线程,与 `ClipboardState.rx` 同手法)。
    pub(crate) rx: Option<Receiver<InstallResult>>,
}

impl CliInstallState {
    /// 是否有安装在途(测试断言用)。
    #[cfg(test)]
    pub fn is_installing(&self) -> bool {
        self.rx.is_some()
    }

    /// 发起后台安装并在结果入队后唤醒 UI。重复点选直接换接收端,旧
    /// 线程的结果无处可去(与剪贴板读取同手法)—— 授权弹窗同一时刻
    /// 只有一个,新点选以最新一次为准。
    pub fn start_with_repaint(&mut self, repaint: RepaintNotifier, exe: PathBuf) {
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let shell_command = install_shell_command(&exe);
        let spawn = std::thread::Builder::new()
            .name("latermd-cli-install".to_owned())
            .spawn(move || {
                let result = run_install(&shell_command);
                // 发送失败(已被更新的点选替换)即静默退出,不滞留
                if tx.send(result).is_ok() {
                    repaint.notify();
                }
            });
        if spawn.is_err() {
            // 线程都起不来:当场收口,不留卡死的「安装中」
            self.rx = None;
        }
    }

    /// 非阻塞收流,结果翻成 [`Message::CliInstallFinished`]。一次安装
    /// 恰产一条结果,故 `match` 而非循环;断连没等到结果(worker 异常
    /// 退出)按失败收尾,「安装中」绝不能卡死(与
    /// `ClipboardState::poll` 同分工)。
    pub fn poll(&mut self) -> Vec<Message> {
        let Some(rx) = &self.rx else {
            return Vec::new();
        };
        let result = match rx.try_recv() {
            Ok(result) => Some(result),
            // 结果未到:留着接收端,下一帧再问
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("命令行安装意外中断".to_owned())),
        };
        result
            .map(|result| vec![Message::CliInstallFinished { result }])
            .unwrap_or_default()
    }

    /// 收尾(归约侧调用):清接收端,允许下一次发起。
    pub fn finish(&mut self) {
        self.rx = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 规范路径的安装命令逐字钉死:目录兜底、覆盖式软链、路径单引号。
    #[test]
    fn install_shell_command_is_pinned_for_canonical_path() {
        assert_eq!(
            install_shell_command(Path::new(
                "/Applications/LaterMD.app/Contents/MacOS/latermd"
            )),
            "mkdir -p /usr/local/bin && ln -sf \
             '/Applications/LaterMD.app/Contents/MacOS/latermd' /usr/local/bin/latermd"
        );
    }

    /// 路径带空格与内嵌单引号:shell 侧单引号转义必须出坑再入坑,
    /// 命令结构(两侧 && 与落点)不变。
    #[test]
    fn hostile_path_is_shell_quoted() {
        let command = install_shell_command(Path::new(
            "/Users/some one/My Apps/Lat'erMD.app/Contents/MacOS/latermd",
        ));
        assert!(
            command.contains(r#"'/Users/some one/My Apps/Lat'\''erMD.app/Contents/MacOS/latermd'"#),
            "空格保留、单引号转义:{command}"
        );
        assert!(
            command.contains(" && ln -sf ") && command.ends_with("/usr/local/bin/latermd"),
            "结构不变:{command}"
        );
    }

    /// AppleScript 字符串转义:反斜杠与双引号都加倍;单引号原样
    /// (它对 AppleScript 无义,shell 那层才需要转义)。
    #[test]
    fn apple_string_literal_escapes_backslash_and_double_quote() {
        assert_eq!(apple_string_literal("plain"), "\"plain\"");
        assert_eq!(apple_string_literal(r"a\b"), r#""a\\b""#);
        assert_eq!(apple_string_literal("a\"b"), r#""a\"b""#);
        assert_eq!(apple_string_literal("it's"), "\"it's\"");
    }

    /// osascript 参数逐字钉死:`-e` 一条,管理员授权后缀不可少
    /// (少了就是无授权静默失败)。
    #[test]
    fn osascript_argv_is_pinned() {
        assert_eq!(
            osascript_argv("ln -sf '/a b' /usr/local/bin/latermd"),
            vec![
                "-e".to_owned(),
                "do shell script \"ln -sf '/a b' /usr/local/bin/latermd\" \
                 with administrator privileges"
                    .to_owned(),
            ]
        );
    }

    /// bundle 判定:三段连续(`*.app/Contents/MacOS`)才算;资源目录、
    /// 缺 Contents、开发裸跑、裸二进制都不算。
    #[test]
    fn bundle_detection_requires_app_contents_macos_adjacency() {
        let bundled = [
            "/Applications/LaterMD.app/Contents/MacOS/latermd",
            "/Users/x/My Apps/LaterMD.app/Contents/MacOS/latermd",
            "/opt/L.app/Contents/MacOS/anything",
        ];
        for path in bundled {
            assert!(
                is_bundled_macos_app(Path::new(path)),
                "{path} 应识别为 bundle"
            );
        }
        let loose = [
            "/home/x/LaterMD/target/debug/latermd",
            "/Applications/LaterMD.app/Contents/Resources/latermd",
            "/Applications/LaterMD.app/MacOS/latermd",
            "/Applications/LaterMD.app/Contents/Contents/MacOS/latermd",
            "/latermd",
        ];
        for path in loose {
            assert!(
                !is_bundled_macos_app(Path::new(path)),
                "{path} 不应识别为 bundle"
            );
        }
    }

    /// run_command 三分支穷尽:成功 / 非零退出(优先 stderr)/ 启动失败。
    /// /bin/true、/bin/false、/bin/sh 在 mac 与 Linux 都在,全平台可测。
    #[test]
    fn run_command_covers_success_stderr_and_launch_failure() {
        assert!(run_command("/bin/true", &[]).is_ok());
        assert_eq!(
            run_command(
                "/bin/sh",
                &["-c".to_owned(), "echo boom 1>&2; exit 3".to_owned()]
            ),
            Err("boom".to_owned()),
            "非零退出优先回传 stderr"
        );
        let failure = run_command("/bin/false", &[]).unwrap_err();
        assert!(
            failure.contains("退出码非零"),
            "空 stderr 回退退出码:{failure}"
        );
        let missing = run_command("/nonexistent-latermd-probe", &[]).unwrap_err();
        assert!(missing.contains("无法启动"), "启动失败可辨:{missing}");
    }

    /// 非 mac 平台 run_install 直接拒绝,不碰 osascript(发起入口本就
    /// 只在 mac 菜单);mac 上不跑这条 —— 真弹授权窗不是测试该做的事。
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn run_install_refuses_off_macos() {
        assert_eq!(
            run_install("true"),
            Err("命令行工具安装仅在 macOS 提供".to_owned())
        );
    }

    /// 授权取消的报错翻译;其余原样。
    #[test]
    fn canceled_authorization_gets_friendly_text() {
        assert_eq!(
            friendly_install_error("execution error: User canceled. (-128)"),
            "已取消授权,未安装"
        );
        assert_eq!(
            friendly_install_error("ln: /usr/local/bin/latermd: Operation not permitted"),
            "ln: /usr/local/bin/latermd: Operation not permitted"
        );
    }

    /// poll 翻消息(注入式,Linux/mac 都跑):恰一条 `CliInstallFinished`,
    /// 收尾后空闲;结果载荷原样透传不反转。
    #[test]
    fn poll_yields_exactly_one_injected_result_and_finishes() {
        let (tx, rx) = mpsc::channel::<InstallResult>();
        let mut install = CliInstallState { rx: Some(rx) };
        assert!(install.is_installing());
        assert!(install.poll().is_empty(), "结果未到,下一帧再问");
        tx.send(Ok(())).unwrap();
        drop(tx);
        assert_eq!(
            install.poll(),
            vec![Message::CliInstallFinished { result: Ok(()) }]
        );
        install.finish();
        assert!(!install.is_installing());
        assert!(install.poll().is_empty(), "收尾后不再翻消息");
    }

    /// 断连兜底:发送端没发结果就退出 → 补一条失败,「安装中」不卡死。
    #[test]
    fn disconnected_without_result_synthesizes_failure() {
        let (_tx, rx) = mpsc::channel::<InstallResult>();
        drop(_tx);
        let mut install = CliInstallState { rx: Some(rx) };
        assert_eq!(
            install.poll(),
            vec![Message::CliInstallFinished {
                result: Err("命令行安装意外中断".to_owned())
            }]
        );
    }

    /// 生命周期(非 mac):start 在 Linux 上由 run_install 立刻报错,
    /// 有限时间内收到恰一条失败结果;线程路径与换端语义在这里走通。
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn lifecycle_on_linux_yields_exactly_one_failure() {
        let mut install = CliInstallState::default();
        install.start_with_repaint(
            RepaintNotifier::default(),
            PathBuf::from("/Applications/LaterMD.app/Contents/MacOS/latermd"),
        );
        assert!(install.is_installing(), "发起即安装中");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut messages = Vec::new();
        while messages.is_empty() && std::time::Instant::now() < deadline {
            messages.extend(install.poll());
            if messages.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert_eq!(messages.len(), 1, "一次安装恰一条结果:{messages:?}");
        assert!(
            matches!(
                &messages[0],
                Message::CliInstallFinished { result: Err(text) } if text.contains("macOS")
            ),
            "非 mac 平台拒绝安装:{messages:?}"
        );
        install.finish();
        assert!(!install.is_installing(), "收尾后空闲");
    }
}
