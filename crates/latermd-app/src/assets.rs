//! 本地图片落 `<doc名>.assets/`(docs/image-plan.md B 段 + D 段纯函数层)。
//!
//! 目录约定:文档 `foo.md` 的图片都住它旁边的 `foo.assets/`。文档里存
//! **相对**路径(`./foo.assets/x.png`),文档目录整体搬走、换机器、进 Git
//! 仓库都仍然有效 —— 这是 B 段「可移植」验收的根基;预览侧再把它解析成
//! 绝对 URI(`ui::rich_widgets::image_rewrites`),两侧各管一半。
//!
//! 命名:保留原名,撞名时 `x.png` → `x-1.png` → `x-2.png`(永不覆盖既有
//! 文件)。落文档的地址遇空格/括号以 `<…>` 包裹 —— CommonMark 的目标语法
//! 允许其中的空格,与 `latermd_md::expand_wikilinks` 同一手法。
//!
//! D 段(粘贴/拖拽)共用同一套落盘:白名单 PNG/JPEG/WebP/GIF、5MB 上限、
//! 剪贴板文件名合成见下方纯函数区。

use std::fmt;
use std::path::{Path, PathBuf};

// 窗口图标的数据载体(egui 经 eframe re-export,与 main.rs 同一手法)。
use eframe::egui;

/// 资产目录后缀:`foo.md` → `foo.assets/`。文件树按它过滤该目录本身。
pub const ASSETS_DIR_SUFFIX: &str = ".assets";

/// 文档 `foo.md` 对应的资产目录 `<doc目录>/foo.assets/`。
///
/// 未落盘语义(路径为空)不在这里兜底:调用方(归约)对未保存文档先落
/// 提示,不给它编造目录。
pub fn assets_dir(doc_path: &Path) -> PathBuf {
    let dir = doc_path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stem = doc_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    dir.join(format!("{stem}{ASSETS_DIR_SUFFIX}"))
}

/// 一次成功的落盘结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    /// 资产目录里的最终文件名(撞名时与原名不同,带 `-N` 后缀)。
    pub file_name: String,
    /// 插进文档的相对地址:`./<doc名>.assets/<file_name>`,含空格/括号时
    /// 以 `<…>` 包裹。
    pub url: String,
}

/// 把图片字节存进文档的资产目录(撞名自动改名,绝不覆盖)。
///
/// D 段粘贴/拖拽的剪贴板字节也从这里进(文件名由调用方合成)。
pub fn store(doc_path: &Path, original_name: &str, bytes: &[u8]) -> Result<Stored, StoreError> {
    if !valid_name(original_name) {
        return Err(invalid_name(doc_path, original_name));
    }
    let dir = assets_dir(doc_path);
    std::fs::create_dir_all(&dir).map_err(|source| StoreError {
        path: dir.clone(),
        source: Box::new(source),
    })?;
    let name = unique_name(&dir, original_name);
    std::fs::write(dir.join(&name), bytes).map_err(|source| StoreError {
        path: dir.join(&name),
        source: Box::new(source),
    })?;
    Ok(Stored {
        url: relative_url(&dir, &name),
        file_name: name,
    })
}

/// 从磁盘文件导入(B 段「浏览…」入口):复制进资产目录。**选中的文件本来就
/// 在该目录里时直接复用**,不复制出第二份 —— 用户重开图片框反复浏览同一张
/// 图是常态,复制会平白堆出 `x-1.png`、`x-2.png`。
pub fn import_file(doc_path: &Path, source: &Path) -> Result<Stored, StoreError> {
    let dir = assets_dir(doc_path);
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| valid_name(name))
        .ok_or_else(|| invalid_name(source, ""))?;
    if source.parent() == Some(dir.as_path()) {
        return Ok(Stored {
            url: relative_url(&dir, &name),
            file_name: name,
        });
    }
    let bytes = std::fs::read(source).map_err(|source_err| StoreError {
        path: source.to_path_buf(),
        source: Box::new(source_err),
    })?;
    store(doc_path, &name, &bytes)
}

/// 名字不合法的拒绝错误(为空、`.`/`..`、带路径分隔符 —— rfd 给的是纯
/// 文件名,这里防御 D 段的合成名)。
fn invalid_name(path: &Path, name: &str) -> StoreError {
    StoreError {
        path: path.join(name),
        source: Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "图片文件名不合法(为空或带路径分隔符)",
        )),
    }
}

/// `x.png` 已存在时的下一个候选:`x-1.png`,再撞 `x-2.png`……
/// 扩展名之前插后缀,保持可读与可识别格式(合法性已在 [`store`] 入口
/// 拒过,这里只管计数)。
fn unique_name(dir: &Path, name: &str) -> String {
    let mut candidate = name.to_owned();
    let mut seq = 0;
    while dir.join(&candidate).exists() {
        seq += 1;
        candidate = with_counter(name, seq);
    }
    candidate
}

/// 纯文件名:非空、不是 `.`/`..`、不带路径分隔符。
fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\\')
}

/// `x.png` → `x-1.png`(无扩展名则整体加后缀)。
fn with_counter(name: &str, seq: u32) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}-{seq}.{ext}"),
        None => format!("{name}-{seq}"),
    }
}

/// 资产目录里的文件名 → 落文档的相对地址。空格/括号会截断 CommonMark 的
/// 裸目标语法,包 `<…>`;中文等其余字符裸放(与 A 段「不转义」口径一致)。
fn relative_url(dir: &Path, file_name: &str) -> String {
    let dir_name = dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let url = format!("./{dir_name}/{file_name}");
    let bare_breaker = |c: char| c.is_whitespace() || c == '(' || c == ')';
    if url.chars().any(bare_breaker) {
        format!("<{url}>")
    } else {
        url
    }
}

/// 落盘失败:带路径,提示行可直接展示。
#[derive(Debug)]
pub struct StoreError {
    path: PathBuf,
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "图片存入失败 {}: {}", self.path.display(), self.source)
    }
}

// —— D 段纯函数层(docs/image-plan.md §3.D)——
// 粘贴与拖拽共用一张「白名单 + 大小上限 + 文件名合成」表:判定全部是
// 纯函数,UI 侧只负责取字节与调它们,单测因此能穷尽。

/// 图片大小上限:**5 MB**(auto-plan #21 既定口径,超限弹提示不落盘)。
pub const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

/// 扩展名(小写、无点)是否在 D 段白名单内。
///
/// 与 [`crate::file::IMAGE_EXTENSIONS`](rfd 对话框的同一张清单)同源:
/// 对话框能选到的 = 粘贴/拖拽肯收的 = 预览 `image` feature 解得了码的。
/// 清单本身全小写,入参先归一(大写扩展名 `X.PNG` 是 Windows 常态)。
pub fn is_allowed_extension(ext: &str) -> bool {
    let ext = ext.to_lowercase();
    crate::file::IMAGE_EXTENSIONS.contains(&ext.as_str())
}

/// 路径的小写扩展名(无点);无扩展名返回空串。
pub fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// 拖入文件的收货判定:`Ok(())` 收下,`Err(文案)` 是弹给用户的提示。
///
/// 扩展名不在白名单 / 超过 [`MAX_IMAGE_BYTES`] 都**不改文档**,只弹
/// notice(image-plan §3.D「白名单外或超 5MB 弹 notice」)。大小在拿到
/// 字节之前先问(拖拽场景 UI 侧有元数据,能提前拒),字节入口
/// ([`store_pasted_image`])在拿到字节后再核一次。
pub fn check_dropped_file(path: &Path, size: u64) -> Result<(), String> {
    let ext = extension_of(path);
    if !is_allowed_extension(&ext) {
        return Err(format!(
            "不支持的图片格式 .{ext}(仅 PNG/JPEG/WebP/GIF),未插入"
        ));
    }
    if size > MAX_IMAGE_BYTES {
        return Err(format!(
            "图片超过 5 MB 上限({:.1} MB),未插入",
            size as f64 / (1024.0 * 1024.0)
        ));
    }
    Ok(())
}

/// 剪贴板图片的默认文件名:`粘贴图片-<纳秒时间戳>.png`(十六进制)。
///
/// 剪贴板字节没有原名可保留;arboard 的 Linux 后端只认 `image/png`
/// (x11.rs get_image 固定按 PNG 解码),Windows 的 CF_DIB/CF_BITMAP
/// 也是转成 RGBA 像素回来 —— 出口统一重编码成 PNG(见
/// `clipboard.rs`),扩展名因此恒 `png`。时间戳防同秒多次粘贴撞名
/// (撞名本身也有 -1 后缀兜底,这里是让名字本身就散开)。
pub fn pasted_image_name(nanos: u128) -> String {
    format!("粘贴图片-{nanos:x}.png")
}

/// 剪贴板字节的落盘入口:先过大小上限,再走 [`store`](撞名后缀、
/// `<…>` 包裹等语义与本地文件同一条路径)。
///
/// 失败文案面向提示行:超限 / 名字不合法 / 写盘失败,调用方只弹 notice。
pub fn store_pasted_image(doc_path: &Path, name: &str, bytes: &[u8]) -> Result<Stored, String> {
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(format!(
            "剪贴板图片超过 5 MB 上限({:.1} MB),未插入",
            bytes.len() as f64 / (1024.0 * 1024.0)
        ));
    }
    store(doc_path, name, bytes).map_err(|error| error.to_string())
}

// —— 窗口图标(auto-plan #16,M1 纯解码 + M2 接线)——
// 本节把 PNG 素材解成 `egui::IconData` 纯数据;接线点在 main.rs 的
// `viewport_builder`(M2),MCP stdio headless 分支不经过这里。

/// 窗口图标素材字节:`assets/logo/deliverables/png/icon-64.png`。
/// 相对路径由 `include_bytes!` 编译期校验 —— 素材被挪走即编译红。
const WINDOW_ICON_PNG: &[u8] = include_bytes!("../../../assets/logo/deliverables/png/icon-64.png");

/// 解码窗口图标为 RGBA(64×64)。
///
/// 解码任一步失败返回 `None` 并终端告警,调用方(main.rs `viewport_builder`)
/// 回落 egui 默认图标,不拦启动;本函数绝不 unwrap/expect/panic。
pub fn window_icon() -> Option<egui::IconData> {
    let image = match image::load_from_memory(WINDOW_ICON_PNG) {
        Ok(image) => image,
        Err(error) => {
            eprintln!("LaterMD: 窗口图标解码失败,回落默认图标: {error}");
            return None;
        }
    };
    let (width, height) = (image.width(), image.height());
    Some(egui::IconData {
        width,
        height,
        rgba: image.to_rgba8().into_raw(),
    })
}

// —— 标题栏左上角品牌标识(#74)——
// 与窗口图标同源不同用:窗口图标要小(64,任务栏/Alt-Tab 缩略),
// 标题栏标识按 2x 上采样到 256 存,缩到 18px 绘制时纹理不过采样。

/// 品牌标识素材字节:`assets/logo/deliverables/png/icon-256.png`。
/// 路径由 `include_bytes!` 编译期校验 —— 素材被挪走即编译红。
const BRAND_LOGO_PNG: &[u8] = include_bytes!("../../../assets/logo/deliverables/png/icon-256.png");

/// 解码品牌标识为 `egui::ColorImage`(256×256),供 `ctx.load_texture` 用。
///
/// 与 [`window_icon`] 同一失败口径:任一步失败返回 `None` + 终端告警,
/// 调用方(`ui::titlebar`)回落矢量 `Icon::Files`,绝不 panic 拦启动。
pub fn brand_logo_image() -> Option<egui::ColorImage> {
    let image = match image::load_from_memory(BRAND_LOGO_PNG) {
        Ok(image) => image,
        Err(error) => {
            eprintln!("LaterMD: 品牌标识解码失败,回落默认图标: {error}");
            return None;
        }
    };
    let size = [image.width() as usize, image.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        size,
        &image.to_rgba8().into_raw(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("latermd-assets-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn doc(dir: &Path, name: &str) -> PathBuf {
        dir.join(name)
    }

    /// 目录与地址推导:`foo.md` → `<dir>/foo.assets/`,URL 带 `./` 前缀。
    #[test]
    fn assets_dir_and_url_derive_from_doc_stem() {
        let dir = temp_dir("derive");
        let doc = doc(&dir, "笔记.md");
        assert_eq!(assets_dir(&doc), dir.join("笔记.assets"));

        let stored = store(&doc, "图.png", b"png").unwrap();
        assert_eq!(stored.file_name, "图.png");
        assert_eq!(stored.url, "./笔记.assets/图.png", "无空格不包 <>");
        assert_eq!(
            std::fs::read(dir.join("笔记.assets/图.png")).unwrap(),
            b"png".to_vec()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 撞名递增:同名第二份得 `-1`,第三份 `-2`;既有 `x-1.png` 占位时跳到
    /// `-2`;扩展名之前插后缀。绝不覆盖既有文件。
    #[test]
    fn name_conflicts_get_counter_suffix() {
        let dir = temp_dir("conflict");
        let doc = doc(&dir, "d.md");
        let first = store(&doc, "x.png", b"1").unwrap();
        assert_eq!(first.file_name, "x.png");
        let second = store(&doc, "x.png", b"2").unwrap();
        assert_eq!(second.file_name, "x-1.png", "第二份 -1");
        // 既有占位:预放 x-1 与 x-2,下一次新名字直接 x-3?不 —— 逐个试,
        // x/x-1/x-2 都在则 x-3
        std::fs::write(dir.join("d.assets/x-2.png"), b"z").unwrap();
        let third = store(&doc, "x.png", b"3").unwrap();
        assert_eq!(third.file_name, "x-3.png");
        assert_eq!(
            std::fs::read(dir.join("d.assets/x.png")).unwrap(),
            b"1".to_vec(),
            "原文件未被覆盖"
        );
        // 无扩展名文件:后缀加在整体上
        let bare = store(&doc, "plain", b"4").unwrap();
        assert_eq!(bare.file_name, "plain");
        let bare_again = store(&doc, "plain", b"5").unwrap();
        assert_eq!(bare_again.file_name, "plain-1", "无扩展名整体加后缀");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 文件名含空格/括号:URL 包 `<…>`;中文不包。
    #[test]
    fn url_wraps_only_when_bare_syntax_would_break() {
        let dir = temp_dir("wrap");
        let doc = doc(&dir, "d.md");
        let spaced = store(&doc, "屏幕 截图 (1).png", b"x").unwrap();
        assert_eq!(spaced.url, "<./d.assets/屏幕 截图 (1).png>");
        let cjk = store(&doc, "示意图.png", b"x").unwrap();
        assert_eq!(cjk.url, "./d.assets/示意图.png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 重复浏览同一张图:已在资产目录里的文件直接复用,不复制第二份。
    #[test]
    fn import_reuses_file_already_in_assets_dir() {
        let dir = temp_dir("reuse");
        let doc = doc(&dir, "d.md");
        let outside = dir.join("src.png");
        std::fs::write(&outside, b"png").unwrap();

        let first = import_file(&doc, &outside).unwrap();
        assert_eq!(first.url, "./d.assets/src.png");

        let inside = assets_dir(&doc).join("src.png");
        let again = import_file(&doc, &inside).unwrap();
        assert_eq!(again.url, "./d.assets/src.png", "复用,不出现 -1 副本");
        let entries: Vec<_> = std::fs::read_dir(assets_dir(&doc))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["src.png".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 不合法文件名(空 / `..` / 带分隔符)拒绝且不落盘。
    #[test]
    fn invalid_names_are_rejected() {
        let dir = temp_dir("invalid");
        let doc = doc(&dir, "d.md");
        for bad in ["", "..", "a/b.png", "a\\b.png"] {
            assert!(store(&doc, bad, b"x").is_err(), "{bad:?} 应被拒绝");
        }
        assert!(
            !assets_dir(&doc).exists(),
            "拒绝发生在建目录之前,不留半个资产目录"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 错误文案带路径,提示行可直接展示。
    #[test]
    fn error_message_names_the_path() {
        let error = store(Path::new("/latermd/无此目录链/x.md"), "a.png", b"x")
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(error.contains("图片存入失败"), "{error}");
        assert!(error.contains("x.assets"), "{error}");
    }

    // —— D 段纯函数层单测(docs/image-plan.md §3.D 验收)——

    /// 白名单判定:扩展名大小写不敏感;白名单外(含可解码但不在清单的
    /// bmp/tiff、无扩展名)一律拒。清单与 rfd 对话框同一张,两者不会漂移。
    #[test]
    fn whitelist_accepts_png_jpeg_webp_gif_case_insensitively() {
        for ok in ["png", "PNG", "Jpg", "jpeg", "webp", "GIF"] {
            assert!(is_allowed_extension(ok), "{ok} 应在白名单");
        }
        for bad in ["", "bmp", "tiff", "svg", "avif", "heic", "md"] {
            assert!(!is_allowed_extension(bad), "{bad:?} 应被拒");
        }
    }

    /// 大小上限判定:恰好 5MB 收下,超一个字节拒;拒的文案带「5 MB」
    /// 与实际大小,弹出的提示能自查。
    #[test]
    fn size_limit_is_five_mebibytes() {
        let file = Path::new("x.png");
        assert!(
            check_dropped_file(file, MAX_IMAGE_BYTES).is_ok(),
            "恰好 5MB 收下"
        );
        let rejected = check_dropped_file(file, MAX_IMAGE_BYTES + 1).unwrap_err();
        assert!(rejected.contains("5 MB"), "{rejected}");
        assert!(rejected.contains("5.0 MB"), "{rejected} 实际大小入文案");
        // 字节入口同一条上限(剪贴板场景拿不到元数据,只能在拿到字节后核)
        let big = vec![0u8; MAX_IMAGE_BYTES as usize + 1];
        let error = store_pasted_image(Path::new("d.md"), "p.png", &big).unwrap_err();
        assert!(error.contains("超过 5 MB"), "{error}");
    }

    /// 白名单外的扩展名:提示点名格式与清单,不落盘也不改文档。
    #[test]
    fn non_whitelisted_extension_is_named_in_notice() {
        let error = check_dropped_file(Path::new("图.bmp"), 128).unwrap_err();
        assert!(error.contains("bmp"), "{error}");
        assert!(error.contains("PNG/JPEG/WebP/GIF"), "{error}");
    }

    /// 剪贴板文件名:十六进制时间戳防同秒撞名,扩展名恒 png;落盘后 URL
    /// 走相对路径,与本地文件同一条通道。撞名时 -1 后缀照常生效。
    #[test]
    fn pasted_names_are_unique_and_conflicts_get_suffix() {
        let a = pasted_image_name(0x1234);
        let b = pasted_image_name(0x5678);
        assert_eq!(a, "粘贴图片-1234.png");
        assert_ne!(a, b, "时间戳不同即不同名");

        let dir = temp_dir("paste");
        let doc = doc(&dir, "d.md");
        let first = store_pasted_image(&doc, &a, b"png").unwrap();
        assert_eq!(first.url, "./d.assets/粘贴图片-1234.png");
        // 同名第二份:撞名后缀在粘贴入口照常生效
        let second = store_pasted_image(&doc, &a, b"png2").unwrap();
        assert_eq!(second.file_name, "粘贴图片-1234-1.png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // —— 窗口图标(auto-plan #16 M1 验收)——

    /// 窗口图标解码:素材在库(include_bytes 编译期校验,路径被挪则编译
    /// 先红)、尺寸 64×64、rgba 长度恰 64*64*4;返回 `None` 即测试红。
    #[test]
    fn window_icon_decodes_to_64x64_rgba() {
        let icon = window_icon().expect("窗口图标素材应能解码");
        assert_eq!(icon.width, 64);
        assert_eq!(icon.height, 64);
        assert_eq!(icon.rgba.len(), 64 * 64 * 4);
    }

    /// 品牌标识解码:256×256(2x 上采样,缩到 18px 绘制时不过采样)、
    /// 且**不是纯色** —— 纯色断言独立于被断言的绘制实现,素材误换成
    /// 一张 1×1 占位图 / 全透明图时这里当场红,而不是让标题栏默默画出
    /// 一个空方块。
    #[test]
    fn brand_logo_decodes_to_256_and_is_not_flat() {
        let image = brand_logo_image().expect("品牌标识素材应能解码");
        assert_eq!(image.size, [256, 256]);
        let first = &image.pixels[..image.pixels.len().min(4)];
        assert!(
            image.pixels.iter().any(|p| *p != first[0]),
            "品牌标识不应是纯色(素材误换占位图?)"
        );
    }
}
