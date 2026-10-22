//! HTML / PDF 导出命令(docs/roadmap.md「导出」线)。
//!
//! 与 `file` 的文件命令同构:UI 只产出 [`Message::ExportHtml`](crate::state::Message::ExportHtml)
//! / [`Message::ExportPdf`](crate::state::Message::ExportPdf),弹框 + IO
//! 在归约里发生(同步 rfd 对话框阻塞事件循环的取舍见 `file` 模块文档)。
//! 导出物是编辑缓冲的派生物:不认领文档路径、不清 dirty。命令 label 与
//! 快捷键(Ctrl/Cmd+E 归 HTML;PDF 不绑默认键)统一收在 [`crate::command`]。

use std::path::{Path, PathBuf};

/// 导出对话框过滤器接受的扩展名(同步给 rfd,不带点)。
pub const HTML_EXTENSIONS: [&str; 2] = ["html", "htm"];

/// 未落盘文档在导出对话框里的预填文件名。
pub const UNTITLED_EXPORT_NAME: &str = "Untitled.html";

/// 导出文件名预填:当前文档名换 `.html` 扩展;未落盘用 [`UNTITLED_EXPORT_NAME`]。
pub fn default_name(document: Option<&Path>) -> String {
    document
        .and_then(Path::file_stem)
        .map(|stem| format!("{}.html", stem.to_string_lossy()))
        .unwrap_or_else(|| UNTITLED_EXPORT_NAME.to_owned())
}

/// 导出保存对话框,预填 `default_name`;取消返回 `None`。
pub fn save_dialog(start_dir: &Path, default_name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("HTML", &HTML_EXTENSIONS)
        .set_directory(start_dir)
        .set_file_name(default_name)
        .save_file()
}

/// PDF 导出对话框过滤器接受的扩展名(同步给 rfd,不带点)。
pub const PDF_EXTENSIONS: [&str; 1] = ["pdf"];

/// 未落盘文档在 PDF 导出对话框里的预填文件名。
pub const UNTITLED_PDF_EXPORT_NAME: &str = "Untitled.pdf";

/// PDF 导出文件名预填:当前文档名换 `.pdf` 扩展;未落盘用
/// [`UNTITLED_PDF_EXPORT_NAME`]。
pub fn default_pdf_name(document: Option<&Path>) -> String {
    document
        .and_then(Path::file_stem)
        .map(|stem| format!("{}.pdf", stem.to_string_lossy()))
        .unwrap_or_else(|| UNTITLED_PDF_EXPORT_NAME.to_owned())
}

/// PDF 导出保存对话框,预填 `default_pdf_name`;取消返回 `None`。
pub fn pdf_save_dialog(start_dir: &Path, default_name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("PDF", &PDF_EXTENSIONS)
        .set_directory(start_dir)
        .set_file_name(default_name)
        .save_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_name_follows_document_stem() {
        assert_eq!(
            default_name(Some(Path::new("/docs/指南 v2.md"))),
            "指南 v2.html"
        );
        assert_eq!(default_name(None), UNTITLED_EXPORT_NAME);
    }

    /// PDF 预填与 HTML 同构:文档名换 `.pdf` 扩展(原扩展整个替换,不是
    /// 追加),未落盘回退「未命名.pdf」。
    #[test]
    fn default_pdf_name_replaces_extension_and_falls_back() {
        assert_eq!(
            default_pdf_name(Some(Path::new("/docs/指南 v2.md"))),
            "指南 v2.pdf"
        );
        // 无扩展名文档(如另存时没写扩展)同样得到 .pdf 后缀
        assert_eq!(
            default_pdf_name(Some(Path::new("/docs/无扩展名"))),
            "无扩展名.pdf"
        );
        assert_eq!(default_pdf_name(None), UNTITLED_PDF_EXPORT_NAME);
    }
}
