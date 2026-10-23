//! 设置类 JSON 的原子落盘。
//!
//! 为什么存在:`std::fs::write` 是「打开(截断)→写」两步,两个写者并发时
//! (测试并行线程、双实例)可能交错成「短内容 + 长文件旧尾巴」,读侧解析
//! 直接失败——实测 `file_tree.json` 被并发测试写撕裂,启动报 trailing
//! characters。这里统一改成「写同目录临时文件 → rename」:同一文件系统内
//! rename 是原子的,读侧看到的要么是旧完整内容、要么是新完整内容。

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// 同进程写者计数:临时文件名里带上它,两个线程并发写同一设置时
/// 各用各的临时文件,不会互相截断。
static WRITER_SEQ: AtomicU64 = AtomicU64::new(0);

/// 原子写 `bytes` 到 `path`:先写同目录临时文件,再 rename 就位。
/// 目录由调用方负责创建(与 `std::fs::write` 语义一致);rename 失败时
/// 清掉半成品临时文件。
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("latermd");
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "设置路径没有父目录"))?;
    let seq = WRITER_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.tmp-{}-{seq}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(source) => {
            let _ = std::fs::remove_file(&tmp);
            Err(source)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 原子写的核心契约:反复「长内容 → 短内容」交替落盘,文件内容必须
    /// 与最后一次写入完全一致(裸 write 在并发下撕裂出旧尾巴,见模块注释)。
    #[test]
    fn overwrite_leaves_no_trailing_bytes() {
        let dir = std::env::temp_dir().join(format!("latermd-persist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("probe.json");
        for len in [64usize, 16, 128, 9, 64] {
            let payload = vec![b'x'; len];
            write_atomic(&path, &payload).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), payload, "长度 {len}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
