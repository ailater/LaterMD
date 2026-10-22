//! 文件命令:新建 / 打开 / 保存 / 另存为(docs/roadmap.md P0「文件操作」)。
//!
//! 读写都是 UTF-8 字节原样进出:不做 CRLF↔LF 转换、不补尾换行
//! (roadmap P0 验收「`.md` 文件保持原样(无格式化篡改)」)。对话框用 rfd
//! 同步版,在 `App::logic` 的归约里弹出——原生模态对话框本来就会阻塞
//! 事件循环,弹框期间本应用没有需要继续绘制的状态。
//!
//! 命令的 label 与快捷键统一收在 [`crate::command`] —— 单一事实源,不再
//! 分散;本模块只承载文件域的执行细节(对话框、读写、错误)。

use std::fmt;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 对话框过滤器接受的 Markdown 扩展名(同步给 rfd,不带点)。
///
/// 唯一事实源在 `latermd-search`:文件树、搜索与 MCP 共用同一张清单
/// (docs/mcp-plan.md §3 方案 A —— 两份清单必然漂移)。
pub use latermd_search::MARKDOWN_EXTENSIONS;

/// 判断路径是否是 LaterMD 支持直接打开的 Markdown 文件。
///
/// 扩展名清单与打开对话框、文件树和全文搜索共用；大小写差异按文件系统
/// 习惯忽略，例如 `README.MD` 也应能通过拖放打开。
pub fn is_markdown_path(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        let extension = extension.to_string_lossy();
        MARKDOWN_EXTENSIONS
            .iter()
            .any(|allowed| extension.eq_ignore_ascii_case(allowed))
    })
}

/// 未落盘文档在另存为对话框里的预填文件名。
pub const UNTITLED_FILE_NAME: &str = "Untitled.md";

/// 图片选择对话框接受的扩展名(docs/image-plan.md D 段白名单口径:预览
/// 解码按这张清单开 `image` feature,对话框与解码同一张表)。
pub const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];

/// 文件命令。UI 只产出,执行(弹框 + IO + 状态变更)在 [`crate::state::State::apply`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileCmd {
    /// 新建空文档。
    New,
    /// 打开已有文件。
    Open,
    /// 保存;从未落盘时等价于另存为。
    Save,
    /// 另存为(总是弹框)。
    SaveAs,
}

/// 起始目录:当前文档所在目录;未落盘或路径无父目录时退回进程工作目录。
pub fn start_dir(current: Option<&Path>) -> PathBuf {
    current
        .and_then(Path::parent)
        // 相对文件名 "note.md" 的 parent 是 "",不是可用目录
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default()
}

/// 打开对话框,选一个已存在的 Markdown 文件;取消返回 `None`。
pub fn open_dialog(start_dir: &Path) -> Option<PathBuf> {
    markdown_dialog(start_dir).pick_file()
}

/// 另存为对话框,预填 `default_name`;取消返回 `None`。
pub fn save_dialog(start_dir: &Path, default_name: &str) -> Option<PathBuf> {
    markdown_dialog(start_dir)
        .set_file_name(default_name)
        .save_file()
}

fn markdown_dialog(start_dir: &Path) -> rfd::FileDialog {
    rfd::FileDialog::new()
        .add_filter("Markdown", &MARKDOWN_EXTENSIONS)
        .set_directory(start_dir)
}

/// 图片选择对话框(docs/image-plan.md B 段「本地文件」来源):取消返回
/// `None`。选中后的复制进 `.assets/` 与地址回填在归约(见
/// `State::pick_image_file`),对话框只负责选。
pub fn pick_image_dialog(start_dir: &Path) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("图片", &IMAGE_EXTENSIONS)
        .set_directory(start_dir)
        .pick_file()
}

/// 目录选择对话框:文件树根目录用(`ui::sidebar` 发消息,归约里弹出)。
/// 取消返回 `None`。起始目录必须存在,由调用方保证。
pub fn pick_folder_dialog(start_dir: &Path) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_directory(start_dir)
        .pick_folder()
}

/// 文件操作失败:带动作与路径,提示行可直接展示。
#[derive(Debug)]
pub struct FileError {
    op: &'static str,
    path: PathBuf,
    source: io::Error,
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}失败 {}: {}",
            self.op,
            self.path.display(),
            self.source
        )
    }
}

fn err(op: &'static str, path: &Path, source: io::Error) -> FileError {
    FileError {
        op,
        path: path.to_path_buf(),
        source,
    }
}

/// UTF-8 读全文。非 UTF-8 文件直接报错而不是替换字符:替换后一旦保存,
/// 原编码内容就被永久毁掉;P0 明确只支持 UTF-8。
pub fn read(path: &Path) -> Result<String, FileError> {
    let bytes = std::fs::read(path).map_err(|source| err("打开", path, source))?;
    String::from_utf8(bytes).map_err(|_| {
        err(
            "打开",
            path,
            io::Error::new(io::ErrorKind::InvalidData, "不是 UTF-8 编码"),
        )
    })
}

/// UTF-8 写全文,字节原样落盘:不做换行符转换,不补尾换行。失败提示的
/// 动作名固定为「保存」;导出等其它写路径用 [`write_as`] 指定动作名。
pub fn write(path: &Path, text: &str) -> Result<(), FileError> {
    write_as("保存", path, text)
}

/// [`fn@write`] 的动作名可指定版本,失败提示形如「导出失败 路径: 原因」。
///
/// 原子落盘语义见 [`write_bytes_as`]。
pub fn write_as(op: &'static str, path: &Path, text: &str) -> Result<(), FileError> {
    write_bytes_as(op, path, text.as_bytes())
}

/// [`write_as`] 的二进制版本(PDF 等非 UTF-8 载荷),同一套原子落盘:
/// 先写目标同目录的临时文件并 fsync,再 rename 顶替。直接的
/// `std::fs::write` 是先截断再写,写入中途 ENOSPC 或进程被杀时磁盘上的
/// 原版本已毁,正文文档不可承受;同目录保证 rename 不跨文件系统,POSIX
/// 与 Windows(标准库内部 `MOVEFILE_REPLACE_EXISTING`)都是原子替换。
pub fn write_bytes_as(op: &'static str, path: &Path, bytes: &[u8]) -> Result<(), FileError> {
    let dir = path
        .parent()
        // 相对文件名 "note.md" 的 parent 是 "",临时文件落到当前目录
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".to_owned());
    // 同进程串行保存不会撞名,计数器只是给并发场景兜底
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = dir.join(format!(
        ".{file_name}.{}-{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
    ));

    let result = (|| -> io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        // 目标已存在时保留其权限位(如 0600 的私有笔记);失败不阻断保存
        if let Ok(metadata) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&tmp, metadata.permissions());
        }
        std::fs::rename(&tmp, path)
    })();
    if let Err(source) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(err(op, path, source));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 进程内唯一且不冲突的临时路径;测试自删。
    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("latermd-file-{}-{name}", std::process::id()))
    }

    #[test]
    fn markdown_path_accepts_supported_extensions_case_insensitively() {
        assert!(is_markdown_path(Path::new("README.md")));
        assert!(is_markdown_path(Path::new("README.MARKDOWN")));
        assert!(!is_markdown_path(Path::new("image.png")));
        assert!(!is_markdown_path(Path::new("README.md.bak")));
    }

    /// 写出→读回逐字节一致:UTF-8、CRLF、LF 混排与尾随空行都原样保留。
    #[test]
    fn write_then_read_is_byte_exact() {
        let path = temp_path("roundtrip.md");
        let text = "# 标题\r\n\r\nCRLF 段落\nLF 段落\n\n";
        write(&path, text).unwrap();
        assert_eq!(read(&path).unwrap(), text);
        // 再对一次字节,确认没经过任何换行规范化
        assert_eq!(std::fs::read(&path).unwrap(), text.as_bytes());
        let _ = std::fs::remove_file(&path);
    }

    /// 二进制写路径(PDF 等非 UTF-8 载荷):字节流逐个原样落盘,不经任何
    /// 文本规范化;失败提示与文本路径同格式(动作名 + 路径)。
    #[test]
    fn write_bytes_as_is_byte_exact_for_binary_payload() {
        let path = temp_path("bytes.bin");
        let payload: &[u8] = &[0x25, 0x50, 0x44, 0x46, 0x2D, 0x00, 0xFF, 0xFE, 0x0A];
        write_bytes_as("导出", &path, payload).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), payload);
        let _ = std::fs::remove_file(&path);

        let error =
            write_bytes_as("导出", Path::new("/latermd/no/such/dir.bin"), payload).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("导出失败"), "{message}");
        assert!(message.contains("dir.bin"), "{message}");
    }

    /// 非 UTF-8 文件报错并把路径带进提示,而不是静默替换成 U+FFFD。
    #[test]
    fn read_rejects_non_utf8_with_path_in_message() {
        let path = temp_path("gbk.md");
        std::fs::write(&path, [0xC4, 0xE3, 0xBA, 0xC3]).unwrap(); // GBK「你好」
        let error = read(&path).unwrap_err().to_string();
        assert!(error.contains("gbk.md"), "提示应含路径: {error}");
        assert!(error.contains("UTF-8"), "提示应说明原因: {error}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_missing_file_names_the_path() {
        let error = read(Path::new("/latermd/不存在.md"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("不存在.md"), "{error}");
    }

    /// 覆盖已有文件:旧内容被完整替换,且目录里不残留临时文件。
    #[test]
    fn overwrite_replaces_content_without_leaving_tmp() {
        let dir = temp_path("atomic-dir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        std::fs::write(&path, "旧内容,足够长以证明不是部分写入").unwrap();

        write(&path, "# 新内容\r\n").unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            "# 新内容\r\n".as_bytes().to_vec()
        );
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "doc.md")
            .collect();
        assert!(leftovers.is_empty(), "临时文件未清理: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 顶替失败(目标被同名目录占据):报错带原路径,既有目录不被破坏,
    /// 失败的临时文件也不残留。
    #[test]
    fn failed_write_keeps_target_and_cleans_tmp() {
        let dir = temp_path("atomic-blocker");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("占用"), b"keep").unwrap();

        let error = write(&dir, "正文").unwrap_err().to_string();
        assert!(error.contains("保存失败"), "{error}");
        assert!(error.contains("atomic-blocker"), "{error}");
        assert_eq!(
            std::fs::read(dir.join("占用")).unwrap(),
            b"keep".as_slice(),
            "目录内容未被触碰"
        );
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "占用")
            .collect();
        assert!(leftovers.is_empty(), "失败的临时文件应清理: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 顶替已有文件时保留其权限位(Unix:0600 的私有笔记保存后仍是 0600)。
    #[cfg(unix)]
    #[test]
    fn overwrite_preserves_existing_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_path("atomic-perm.md");
        std::fs::write(&path, "私有").unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(&path, permissions).unwrap();

        write(&path, "仍私有").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "权限位应从原文件继承");
        let _ = std::fs::remove_file(&path);
    }
}
