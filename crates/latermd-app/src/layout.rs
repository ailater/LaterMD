//! 外壳布局状态与持久化(docs/ui-shell-redesign.md §10)。
//!
//! 左右两个 `Panel::show_collapsible` 的 `&mut bool` 住在这里;左侧页签也一并
//! 收在本结构体,使「停在哪个视图」与「面板开着与否」同属一份存档。
//!
//! **为什么单开 `layout.json` 而不塞 `settings.json`**:后者已承载主题与皮肤
//! 选择,再塞面板可见性会让「换主题」和「收面板」两个无关动作共用一份
//! 存档,`select_skin` 那套「皮肤文件是唯一事实源」的口径被稀释
//! (decisions-pending #24 的教训)。落盘风格与 `filetree::FileTreeSettings`
//! 完全一致:serde 往返、`#[serde(default)]` 缺项回落、坏文件只告警不挡启动。

use crate::state::SidebarTab;
use crate::theme;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::result::Result as StdResult;

use serde::{Deserialize, Serialize};

/// 持久化文件名,与 `settings.json` 同目录。
const SETTINGS_FILE: &str = "layout.json";

/// 进入禅定前的面板快照(左栏开合);退出时还原(§7)。
///
/// 规格 §7 原写三元组 `(left, right, editor_hidden)`;右栏对照预览退役
/// (live-unify-plan P4)后只剩左栏一元,`editor_hidden` 在禅定下恒 `true`
/// (**藏编辑器就是禅定的定义 itself**),把常量写进存档会在每次读它时都
/// 骗人一次,不留假字段。
///
/// **只快照面板开合而不是整个 `LayoutSettings`**:多记一个字段就多一条
/// 「进/出漏抄某一项」的路。
pub type PreZen = Option<bool>;

/// 持久化的外壳布局。字段各自直接喂给 UI,不额外镜像。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutSettings {
    /// 左栏(导航)是否展开。
    pub left: bool,
    /// 上次主动选择的编辑模式；旧配置/首次启动进入写作。
    pub render_mode: Option<crate::live::RenderMode>,
    /// 禅定模式(§7):三栏让位、预览占满内容区。
    ///
    /// **`skip`(读写两端都跳过)而不是 `default`**:它是**当前会话的临时
    /// 沉浸态**,不是像 left 那样的长期布局偏好。跨会话保留会让下次
    /// 启动的窗口在用户没要求的情况下直接进入禅定,而配套的 `pre_zen` 快照
    /// 按同一口径不落盘 —— 那次会话里退出禅定只剩「一律全开」这一条兜底,
    /// 用户被静默改了布局还回不到原样。
    ///
    /// 落到字段上而不是在 `save_to` 里事后改写,`save_to` 因此回到「所见即
    /// 所写」—— 少一处「 save 与 struct 定义不同步」的可能。
    #[serde(skip)]
    pub zen: bool,
    /// 左栏上次停留的视图。
    pub left_view: SidebarTab,
    /// 进入禅定前的面板快照(§7)。同上口径:`skip`,不落盘。
    #[serde(skip)]
    pub pre_zen: PreZen,
    /// 上次退出时窗口是否最大化(2026-09-29 坤哥指令「记住上次是全屏还是
    /// 窗口」)。字段级 `default`:旧 layout.json 没有它,缺项回落 false
    /// 而不是整表 Corrupt 丢弃 left。
    #[serde(default)]
    pub maximized: bool,
    /// 左栏(导航)宽度。`None` = 从未拖过,用 egui 默认。恢复走首帧前
    /// `insert_persisted` 塞 PanelState(egui 的 store 是私有的)。
    #[serde(default)]
    pub left_width: Option<f32>,
}

/// 出厂布局:三栏全开(旧行为),停文件树。
impl Default for LayoutSettings {
    fn default() -> Self {
        Self {
            left: true,
            render_mode: None,
            zen: false,
            left_view: SidebarTab::Files,
            pre_zen: None,
            maximized: false,
            left_width: None,
        }
    }
}

impl LayoutSettings {
    /// 新安装停大纲(直奔写作);Default 保留旧配置缺项的兼容口径。
    pub fn fresh_install() -> Self {
        Self {
            left_view: SidebarTab::Outline,
            ..Self::default()
        }
    }

    /// 启动装载:无文件/坏文件都回落默认(坏件终端告警,挡启动不值得)。
    pub fn load() -> Self {
        let Some(dir) = theme::config_dir() else {
            return Self::fresh_install();
        };
        match Self::load_from(&dir) {
            Ok(settings) => settings,
            Err(LoadError::Missing) => Self::fresh_install(),
            Err(LoadError::Corrupt(source)) => {
                eprintln!("LaterMD: 布局设置解析失败,已回落默认: {source}");
                Self::fresh_install()
            }
        }
    }

    /// 落盘到 `<dir>/layout.json`;`dir` 为 `None` 时用平台默认目录。
    /// 目录不存在则创建。失败带路径,提示行可直接展示。
    ///
    /// `zen` 与 `pre_zen` 都是 `#[serde(skip)]`(理由见各自字段注释),而禅定
    /// 期间 `left` 已被改成 `false` —— 那是**状态被临时借用**,
    /// 不是用户改了偏好。此刻若有别的原因触发保存(切左栏视图等),照直写
    /// 会把「三栏全关」钉进磁盘:下次启动既没有侧栏也没有菜单栏入口
    /// (禅定下两者都在),用户面对一个近乎空的窗口。故这里按快照还原后再写。
    pub fn save_to(&self, dir: Option<&Path>) -> StdResult<(), SaveError> {
        let Some(dir) = dir.map(Path::to_path_buf).or_else(theme::config_dir) else {
            return Err(SaveError {
                path: PathBuf::from(SETTINGS_FILE),
                source: "找不到平台配置目录(HOME/APPDATA 均未设置)".into(),
            });
        };
        let mut saved = self.clone();
        if let Some(left) = saved.pre_zen {
            saved.left = left;
        }
        let json = serde_json::to_string_pretty(&saved).map_err(|source| SaveError {
            path: dir.join(SETTINGS_FILE),
            source: Box::new(source),
        })?;
        std::fs::create_dir_all(&dir).map_err(|source| SaveError {
            path: dir.clone(),
            source: Box::new(source),
        })?;
        std::fs::write(dir.join(SETTINGS_FILE), json.as_bytes()).map_err(|source| SaveError {
            path: dir.join(SETTINGS_FILE),
            source: Box::new(source),
        })
    }

    /// 测试与 `main` 的装载取同一份实现(`load()` 就是它 + 失败回落)。
    pub(crate) fn load_from(dir: &Path) -> StdResult<Self, LoadError> {
        let path = dir.join(SETTINGS_FILE);
        let bytes = std::fs::read(&path).map_err(|source| match source.kind() {
            io::ErrorKind::NotFound => LoadError::Missing,
            _ => LoadError::Corrupt(format!("{}: {}", path.display(), source)),
        })?;
        serde_json::from_slice(&bytes)
            .map_err(|source| LoadError::Corrupt(format!("{}: {}", path.display(), source)))
    }

    /// 进禅定:存快照 → 关两栏 → 置标志(§7)。
    ///
    /// **已经在禅定里再调一次是 no-op**:`pre_zen` 已 `Some` 就不再覆写,
    /// 否则第二册快照会把第一册的布局吞掉,退出时还原到「进禅定之后」的样子
    /// (三关),等于永久丢掉了用户的原始布局。
    pub fn enter_zen(&mut self) {
        if self.pre_zen.is_some() {
            return;
        }
        self.pre_zen = Some(self.left);
        self.left = false;
        self.zen = true;
    }

    /// 退出禅定:按快照逐项还原(§7)。**必须还原到进入前的状态**,不能一律
    /// 全开 —— 用户原本关着左侧写,退出禅定却蹦出侧栏是意外行为
    /// (decisions-pending #29「宁可多一次操作,不静默改变用户状态」同款)。
    ///
    /// 没有快照(脏启动 / `zen` 从存档读出来而快照被丢)时**退化为三栏全开**
    /// 而不是什么都不做:后者会把用户永久困在禅定里。
    pub fn exit_zen(&mut self) {
        match self.pre_zen.take() {
            Some(left) => self.left = left,
            // 没有快照(脏启动 / `zen` 从存档读出来而快照被丢)时退化为侧栏
            // 全开,而不是什么都不做 —— 后者会把用户永久困在禅定里。
            None => self.left = true,
        }
        self.zen = false;
    }
}

/// 读设置的失败情形(与 `crate::theme` / `crate::filetree` 同构)。
///
/// `pub(crate)`:布局的内存态与存档态要能互比(重启恢复测试的落点),
/// 不必像 `SaveError` 那样面向用户提示。
#[derive(Debug)]
pub(crate) enum LoadError {
    Missing,
    Corrupt(String),
}

/// 写设置失败:带路径,可直接进提示行。
#[derive(Debug)]
pub struct SaveError {
    path: PathBuf,
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "布局设置保存失败 {}: {}",
            self.path.display(),
            self.source
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("latermd-layout-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// 往返无损:改过的三态 + 视图都回来。
    ///
    /// `zen` 取 false:`Zen` 不落盘(见下一个用例),带着 `zen: true` 进往返
    /// 必然不相等 —— 那是设计如此,不是 bug。
    /// 旧 layout.json 没有 maximized 字段(2026-09-29 新增):缺项必须回落
    /// false 且**不整表 Corrupt** —— 否则用户升级后 left 偏好被静默清掉,
    /// 比丢 maximized 严重得多。旧版的 `right`/`right_width` 键(右栏对照
    /// 预览,live-unify-plan P4 退役)按 `#[serde(default)]` 的结构体级口径
    /// **静默忽略** —— 无需迁移,读进来即弃。
    #[test]
    fn legacy_json_without_maximized_still_loads() {
        let dir = dir("legacy-no-max");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(SETTINGS_FILE),
            r#"{"left":false,"right":true,"right_width":321.5,"left_view":"outline"}"#,
        )
        .unwrap();
        let loaded = LayoutSettings::load_from(&dir).unwrap();
        assert!(!loaded.left, "旧字段不受影响");
        assert!(!loaded.maximized, "缺项 maximized 回落 false");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// maximized 往返:保存后重读不丢(端到端的落盘在 state 侧走
    /// end_of_logic 比对写,与本模块的 save_to/load_from 同链路)。
    #[test]
    fn maximized_roundtrips() {
        let dir = dir("max-roundtrip");
        let settings = LayoutSettings {
            maximized: true,
            ..Default::default()
        };
        settings.save_to(Some(&dir)).unwrap();
        assert!(LayoutSettings::load_from(&dir).unwrap().maximized);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn roundtrip_preserves_every_field() {
        let dir = dir("roundtrip");
        let settings = LayoutSettings {
            left: false,
            render_mode: None,
            zen: false,
            left_view: SidebarTab::Outline,
            ..Default::default()
        };
        settings.save_to(Some(&dir)).unwrap();
        assert_eq!(LayoutSettings::load_from(&dir).unwrap(), settings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **禅定不跨会话**:`zen` 是本次会话的临时沉浸态,不像 left/right 那样
    /// 是长期布局偏好。一旦写进 `layout.json`,下次启动的窗口会在用户没要求
    /// 的情况下直接进入禅定。且那时候 `pre_zen` 已被 `skip`(更不会被写出),
    /// 退出禅定只剩「一律全开」这一条兜底 —— 用户被静默改了布局还找不回来。
    #[test]
    fn zen_and_snapshot_are_never_persisted() {
        let dir = dir("zen-ephemeral");
        let mut settings = LayoutSettings::default();
        settings.enter_zen();
        assert!(settings.zen && settings.pre_zen.is_some());

        settings.save_to(Some(&dir)).unwrap();
        let raw = std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(!raw.contains("zen"), "写出的 JSON 里不该有 zen 字段:{raw}");
        assert!(!raw.contains("pre_zen"), "快照同口径不落盘:{raw}");

        let loaded = LayoutSettings::load_from(&dir).unwrap();
        assert!(!loaded.zen, "读回来的窗口不是禅定的");
        assert_eq!(loaded.pre_zen, None, "读回来没有脏快照");
        assert!(loaded.left, "左栏按快照还原着写,不写关闭态");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 禅定期间触发保存(例如期间切了左栏视图):写出去的是**快照里的布局**
    /// 而不是当下那个被借用的 `false/false`。这是 `save_to` 特意多看一眼
    /// `pre_zen` 的唯一原因 —— 漏了它,用户会发现下次启动既没有侧栏也没有
    /// 菜单栏入口(禅定下两者都在),面对一个近乎空的窗口。
    #[test]
    fn saving_during_zen_writes_the_snapshot_columns() {
        let dir = dir("zen-save");
        let mut settings = LayoutSettings {
            left: false,
            ..Default::default()
        };
        settings.enter_zen();
        assert!(!settings.left, "禅定期间左栏被关了");

        settings.save_to(Some(&dir)).unwrap();
        let loaded = LayoutSettings::load_from(&dir).unwrap();
        assert!(!loaded.left, "照快照写:禅定期间的借用不落盘");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 进出禅定:退出一律**还原到进入前的组合**,不一律全开 —— 用户原本
    /// 关着左栏写,退出禅定却蹦出侧栏是意外行为(decisions-pending #29 同款)。
    #[test]
    fn exit_zen_restores_the_pre_zen_combination() {
        for left in [true, false] {
            let mut settings = LayoutSettings {
                left,
                ..Default::default()
            };
            settings.enter_zen();
            assert!(!settings.left && settings.zen);
            settings.exit_zen();
            assert_eq!(
                (settings.left, settings.zen),
                (left, false),
                "左栏开合 {left} 逐项还原"
            );
            assert_eq!(settings.pre_zen, None, "退出后快照清掉");
        }
    }

    /// 重复进入是 no-op:第二册快照不能吞掉第一册的原始布局,否则退出时
    /// 还原到「进禅定之后」的三关状态 = 永久丢掉用户的布局。
    #[test]
    fn entering_zen_twice_keeps_the_original_snapshot() {
        let mut settings = LayoutSettings {
            left: false,
            ..Default::default()
        };
        settings.enter_zen();
        settings.enter_zen();
        assert_eq!(settings.pre_zen, Some(false));
        settings.exit_zen();
        assert!(!settings.left);
    }

    /// 没有快照(例如这份 `zen` 是手改出来的)时退出退化为三栏全开,而不是
    /// 什么都不做 —— 后者会把用户永久困在禅定里。
    #[test]
    fn exit_zen_without_snapshot_falls_back_to_all_columns() {
        let mut settings = LayoutSettings {
            zen: true,
            left: false,
            ..Default::default()
        };
        settings.exit_zen();
        assert!(!settings.zen);
        assert!(settings.left, "退路是侧栏全开");
    }

    /// 出厂布局侧栏全开、停文件树 —— 与旧行为一致,升级用户不会被直接
    /// 扔进空窗口(左栏收起又没菜单栏入口会很难找回)。
    #[test]
    fn default_is_three_columns_open_on_files() {
        let settings = LayoutSettings::default();
        assert!(settings.left);
        assert!(!settings.zen);
        assert_eq!(settings.left_view, SidebarTab::Files);
    }

    /// 缺项回落:手改配置只留一个键,其余取默认而不是整体失败
    /// (与 `filetree::FileTreeSettings` 同款口径)。
    #[test]
    fn partial_json_fills_defaults() {
        let dir = dir("partial");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SETTINGS_FILE), br#"{"left": false}"#).unwrap();

        let settings = LayoutSettings::load_from(&dir).unwrap();
        assert!(!settings.left, "显式值生效");
        assert_eq!(settings.left_view, SidebarTab::Files);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 坏文件不 panic 也不静默:解析失败返回 `Corrupt`,`load()` 据此回落
    /// 默认并告警(用户对两种失败的处置不同,不能混)。
    #[test]
    fn corrupt_json_is_reported_not_swallowed() {
        let dir = dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SETTINGS_FILE), b"{ not json").unwrap();
        let loaded = LayoutSettings::load_from(&dir);
        assert!(
            matches!(loaded, Err(LoadError::Corrupt(_))),
            "坏文件要能被识别(由 load() 告警后回落默认),实际 {loaded:?}"
        );
        // 源串随变体走 Debug(不像 SaveError 那样面向提示行做 Display:读它的
        // 是看日志的开发者)。务必带上**路径与行列**,否则坏在哪无从定位。
        let debug = format!("{:?}", LayoutSettings::load_from(&dir).unwrap_err());
        assert!(debug.contains("layout.json"), "要带文件名:{debug}");
        assert!(
            debug.contains("line 1 column 3"),
            "要带 serde 的行列:{debug}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 文件缺失是常态而非错误(`Missing` 不进告警路径,首次启动才干净)。
    #[test]
    fn missing_file_is_distinct_from_corrupt() {
        let dir = dir("missing");
        let loaded = LayoutSettings::load_from(&dir);
        assert!(
            matches!(loaded, Err(LoadError::Missing)),
            "文件缺失是常态而非错误,实际 {loaded:?}"
        );
    }
}
