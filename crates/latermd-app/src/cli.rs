//! 启动参数解析:系统把「双击的 .md 文件」交给本应用的方式只有一条 ——
//! `latermd <文件路径>`,三个平台的关联注册最终都落到这条 argv 上
//! (Linux `.desktop` 的 `%F`、macOS `Info.plist` 的文档类型、Windows
//! 注册表的 `shell\open\command`)。
//!
//! 本模块是那条链路的**纯函数核**:只认参数、不碰 IO、不看进程环境,
//! 入参(`args` 与 `cwd`)全部由调用方注入 —— 无头测试因此可以在任意
//! 平台环境下钉死同一组口径。**用 `OsString` 而非 `String` 走 argv**:
//! `std::env::args()` 遇到非 UTF-8 参数会 panic,而中文/生僻文件名的
//! 用户里这不是假想场景。
//!
//! 三条口径:
//! - **`-` 开头的参数一律当 flag 跳过**,不判它是不是已知 flag —— 新增
//!   开关不该在这里补一遍白名单(那样漏一个就变成"打开一个名为
//!   `--foo` 的文件")。三个调用方(桌面启动器/Finder/资源管理器)都不会
//!   在文件路径前插 flag,所以放行未识别的 flag 没有任何实际收益。
//! - **只收 Markdown 扩展名**,判据复用 `file::is_markdown_path`,与打开
//!   对话框、文件树、搜索共用同一张表(`latermd_search::MARKDOWN_EXTENSIONS`)。
//!   非 md 的参数不静默丢弃,原样进 `rejected` 交调用方提示。
//! - **相对路径转绝对**(`cwd` 为 `None` 时原样保留):多标签的路径去重
//!   (`tabs::find_by_path`)按 `PathBuf` 相等比较,命令行传的 `note.md`
//!   与之后从文件树点出来的 `/home/x/note.md` 不等号 → 同一文件开两个
//!   标签。绝对化在**启动这一刻**完成,避免 cwd 后续变化。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// 参数解析的结果:采用哪些、拒收哪些。
///
/// 拒收项存在的意义是**让用户看得见** —— `latermd foo.txt` 静默开空窗
/// 与"参数被系统吃了"症状完全一样,分不开。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OpenTargets {
    /// 应当打开的 Markdown 路径(绝对化、顺序即 argv 顺序)。
    pub markdown: Vec<PathBuf>,
    /// 因扩展名不是 Markdown 而拒收的路径(仅供提示)。
    pub rejected: Vec<PathBuf>,
}

/// 解析 argv(`args_os()` 的完整结果,**含 `argv[0]`**)。
///
/// `cwd` 用于把相对路径绝对化;`None` 表示当前工作目录取不到
/// (进程 cwd 可能已被删除),此时相对路径原样保留。
pub fn parse_open_targets(args: &[OsString], cwd: Option<&Path>) -> OpenTargets {
    let mut targets = OpenTargets::default();
    for arg in args.iter().skip(1) {
        // 空参数不是路径(某些 shell 会在无参时传空串),跳过而非当成
        // 「当前目录」——后者会打开一个目录并在读取时报出一句费解的错误。
        if arg.is_empty() || arg.as_os_str().to_string_lossy().starts_with('-') {
            continue;
        }
        let path = PathBuf::from(arg);
        // 拒收项同样绝对化:提示里出现 `e.mdx` 而不是 `/home/kun/vault/e.mdx`
        // 时,用户没法判断是哪个文件 —— 尤其 cwd 恰好是家目录时,
        // `e.mdx` 看上去完全合理。
        let path = absolute(&path, cwd);
        if crate::file::is_markdown_path(&path) {
            targets.markdown.push(path);
        } else {
            targets.rejected.push(path);
        }
    }
    targets
}

/// 绝对化:已是绝对路径原样返回;相对路径拼在 `cwd` 之后。`cwd` 缺席时
/// 原样保留(交由 `file::read` 报错,那时它还能给出真实原因)。
fn absolute(path: &Path, cwd: Option<&Path>) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match cwd {
        Some(dir) => dir.join(path),
        None => path.to_path_buf(),
    }
}

/// 把拒收项拼成一句终端提示(无拒收项返回 `None`)。
///
/// 措辞不带"不支持"这种结论语气:调用方只是没让本应用打开它,文件本身
/// 可能已被别的应用正常打开。
pub fn rejection_notice(rejected: &[PathBuf]) -> Option<String> {
    match rejected {
        [] => None,
        [one] => Some(format!("LaterMD: 忽略非 Markdown 文件 {}", one.display())),
        many => Some(format!(
            "LaterMD: 忽略 {} 个非 Markdown 文件(首个: {})",
            many.len(),
            many[0].display()
        )),
    }
}

/// 便于测试与调用方书写的字符串切片入口。
///
/// 仅测试使用:生产路径的 argv 是 `std::env::args_os()` 给的 `OsString`,
/// 没人需要这个转换 —— 不加 `cfg(test)` 会在非测试构建里报 dead_code,
/// 而 clippy `-D warnings` 把它当错误。
#[cfg(test)]
pub(crate) fn owned_args<I, S>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    args.into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(args: &[&str]) -> OpenTargets {
        parse_open_targets(&owned_args(args), Some(Path::new("/home/kun/vault")))
    }

    /// 双击 .md 的主路径:一个相对文件名被绝对化并采用。
    #[test]
    fn single_relative_path_is_absolutized_and_accepted() {
        let got = targets(&["latermd", "note.md"]);
        assert_eq!(got.markdown, vec![PathBuf::from("/home/kun/vault/note.md")]);
        assert!(got.rejected.is_empty());
    }

    /// 已是绝对路径时原样保留 —— `Path::join` 会把它当子目录拼坏,
    /// 这个分支必须单测(绝对路径 join 相对路径是 std 的既定行为)。
    #[test]
    fn absolute_path_is_left_alone() {
        let got = targets(&["latermd", "/tmp/a.md"]);
        assert_eq!(got.markdown, vec![PathBuf::from("/tmp/a.md")]);
    }

    /// flag 与程序名都不进结果,且**不占拒收**(它们不是"文件")。
    /// `--mcp-stdio` 的完整判定在 main,这里只保证它被安静跳过。
    #[test]
    fn flags_and_program_name_are_skipped_silently() {
        let got = targets(&["latermd", "--mcp-stdio", "--renderer=glow", "-w", "b.md"]);
        assert_eq!(got.markdown, vec![PathBuf::from("/home/kun/vault/b.md")]);
        assert!(
            got.rejected.is_empty(),
            "flag 不该被当成拒收文件:{:?}",
            got.rejected
        );
    }

    /// 扩展名白名单只认 md / markdown,大小写按文件系统习惯忽略。
    #[test]
    fn only_md_and_markdown_are_accepted() {
        let got = targets(&["latermd", "a.md", "b.markdown", "c.MD", "d.txt", "e.mdx"]);
        assert_eq!(
            got.markdown,
            vec![
                PathBuf::from("/home/kun/vault/a.md"),
                PathBuf::from("/home/kun/vault/b.markdown"),
                PathBuf::from("/home/kun/vault/c.MD"),
            ]
        );
        assert_eq!(
            got.rejected,
            vec![
                PathBuf::from("/home/kun/vault/d.txt"),
                PathBuf::from("/home/kun/vault/e.mdx"),
            ],
            "拒收项要绝对化,提示才指得清是哪个文件"
        );
    }

    /// 多文件保持 argv 顺序(决定标签栏排列,系统不会替你排序)。
    #[test]
    fn multiple_paths_keep_argv_order() {
        let got = targets(&["latermd", "/z.md", "/a.md", "/m.md"]);
        assert_eq!(
            got.markdown,
            vec![
                PathBuf::from("/z.md"),
                PathBuf::from("/a.md"),
                PathBuf::from("/m.md")
            ]
        );
    }

    /// 空格与非 UTF-8 文件名必须原样穿过:`OsString` 通道的意义就在这。
    ///
    /// 换成 `String` 的实现在这里会 panic(`std::env::args` 的行为),
    /// 所以判据是「非 UTF-8 字节逐字节回来」而不是「字符串相等」。
    /// 非 UTF-8 字节构造只在 Unix 有意义(Windows 的路径是 UTF-16,
    /// 不存在非法序列),故该半段 `#[cfg(unix)]`。
    #[test]
    fn spaces_and_non_utf8_survive_intact() {
        let args = vec![OsString::from("latermd"), OsString::from("我的 笔记.md")];
        let got = parse_open_targets(&args, Some(Path::new("/v")));
        assert_eq!(got.markdown, vec![PathBuf::from("/v/我的 笔记.md")]);

        #[cfg(unix)]
        {
            use std::os::unix::ffi::{OsStrExt, OsStringExt};
            // 0xFF 在 UTF-8 里非法,任何走 String 的中间层都会失败或丢字节。
            let raw = OsString::from_vec(vec![b'/', b'x', 0xFF, b'.', b'm', b'd']);
            let got = parse_open_targets(
                &[OsString::from("latermd"), raw.clone()],
                Some(Path::new("/v")),
            );
            assert_eq!(got.markdown.len(), 1, "非法 UTF-8 也该被当路径收下");
            assert_eq!(
                got.markdown[0].as_os_str().as_bytes(),
                b"/x\xff.md",
                "字节必须逐字节回来,一个字节都不能丢"
            );
        }
    }

    /// cwd 取不到时相对路径原样保留,绝不退化成空路径或根目录。
    #[test]
    fn missing_cwd_keeps_relative_path() {
        let got = parse_open_targets(&owned_args(["latermd", "note.md"]), None);
        assert_eq!(got.markdown, vec![PathBuf::from("note.md")]);
    }

    /// 空参数不是路径:某些 shell 无参时传空串,当成路径会打开当前目录。
    #[test]
    fn empty_arg_is_not_a_path() {
        let got = targets(&["latermd", "", "a.md"]);
        assert_eq!(got.markdown, vec![PathBuf::from("/home/kun/vault/a.md")]);
        assert!(got.rejected.is_empty());
    }

    /// 无参数启动(点启动器图标)产出两个空列表 —— 不开文件、不提示。
    #[test]
    fn no_arguments_yields_nothing() {
        let got = targets(&["latermd"]);
        assert_eq!(got, OpenTargets::default());
        assert_eq!(rejection_notice(&got.rejected), None);
    }

    /// 拒收提示:单条给路径,多条给计数 + 首个,零条不提示。
    #[test]
    fn rejection_notice_covers_zero_one_many() {
        assert_eq!(rejection_notice(&[]), None);
        assert_eq!(
            rejection_notice(&[PathBuf::from("/a.txt")]),
            Some("LaterMD: 忽略非 Markdown 文件 /a.txt".to_owned())
        );
        let many = rejection_notice(&[PathBuf::from("/a.txt"), PathBuf::from("/b.png")])
            .expect("多条也要提示");
        assert!(many.contains("2 个"), "{many}");
        assert!(many.contains("/a.txt"), "{many}");
    }
}
