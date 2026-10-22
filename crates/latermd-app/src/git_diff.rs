//! 读取仓库未提交改动(菜单「AI: 生成 commit message」的数据源)。
//!
//! P2 才引入 `latermd-git`(git2);这里用 `std::process::Command` 直接跑
//! `git diff`,staged 优先、无 staged 回落 working tree。调用方把整条
//! commit 建议流水线放在后台线程，因此大型仓库和真实网络请求都不阻塞
//! UI 归约。untracked 文件不出现在 `git diff` 里,按任务口径不做。

use std::path::Path;
use std::process::Command;

/// 读取 `dir` 所在仓库的未提交改动:staged diff 非空则用之,否则
/// working tree diff。失败(无 git / 不在仓库内 / 非 0 退出)返回带
/// stderr 摘要的错误描述,面向提示行;两边都干净时返回空串。
pub fn uncommitted_diff(dir: &Path) -> Result<String, String> {
    let staged = run_diff(dir, &["--staged"])?;
    if !staged.trim().is_empty() {
        return Ok(staged);
    }
    run_diff(dir, &[])
}

fn run_diff(dir: &Path, extra: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        // quotepath=false:中文文件名不转义成八进制,喂给模型才可读
        .args([
            "-c",
            "core.quotepath=false",
            "--no-pager",
            "diff",
            "--no-color",
        ])
        .args(extra)
        .current_dir(dir)
        .output()
        .map_err(|error| format!("无法启动 git: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(format!("git diff 失败: {stderr}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 建一次性临时仓库(每次全新目录,测试间无共享状态)。
    fn temp_repo(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("latermd-gitdiff-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        dir
    }

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(["-c", "user.name=LaterMD", "-c", "user.email=latermd@test"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} 失败: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// staged 优先:暂存区有改动时不看 working tree,未暂存的修改不掺入。
    #[test]
    fn staged_diff_wins_over_working_tree() {
        let dir = temp_repo("staged");
        std::fs::write(dir.join("base.md"), "基础\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "init"]);

        std::fs::write(dir.join("staged.md"), "暂存的新文件\n").unwrap();
        git(&dir, &["add", "staged.md"]);
        // 修改已暂存文件但不 add:working tree 改动,staged diff 不应包含
        std::fs::write(dir.join("base.md"), "基础\n工作区又改了\n").unwrap();

        let diff = uncommitted_diff(&dir).unwrap();
        assert!(diff.contains("staged.md"), "{diff}");
        assert!(
            !diff.contains("工作区又改了"),
            "staged 优先,working tree 改动不掺入:{diff}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 无 staged 回落 working tree;两边都干净时返回空串(由调用方提示)。
    #[test]
    fn falls_back_to_working_tree_and_empty_when_clean() {
        let dir = temp_repo("worktree");
        std::fs::write(dir.join("a.md"), "一\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "init"]);

        assert_eq!(uncommitted_diff(&dir).unwrap(), "", "干净仓库无 diff");

        std::fs::write(dir.join("a.md"), "一\n二\n").unwrap();
        let diff = uncommitted_diff(&dir).unwrap();
        assert!(diff.contains("+二"), "{diff}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 非 git 目录:错误描述含 git 字样,面向提示行。
    #[test]
    fn non_repo_dir_is_error() {
        let dir =
            std::env::temp_dir().join(format!("latermd-gitdiff-{}-plain", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let error = uncommitted_diff(&dir).unwrap_err();
        assert!(error.contains("git"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
