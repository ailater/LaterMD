//! 可配置的快捷键表(docs/ui-polish.md §5「快捷键」页)。
//!
//! P0 的键位硬编码在 [`crate::command::Command::default_shortcut`];本模块把它
//! 变成**数据**:一张按命令索引的绑定表 + `keymap.json` 持久化 + 冲突检测。
//! 命令层 [`crate::command::poll_shortcuts`] 从此读实际绑定,不再读默认值。
//!
//! 规则:
//! * 一个命令最多一个绑定;未绑定(`None`)即只能从菜单/工具栏触发;
//! * 新绑定与其它命令撞键 → 拒绝并告知撞的是哪个命令(不静默抢占,否则
//!   用户会莫名丢失另一个命令的键位);
//! * 必须带修饰键(`Ctrl/Cmd`、`Shift`、`Alt`)或功能键 —— 裸字母是编辑器
//!   的输入,绑了就再也打不出那个字;
//! * 文件格式是「命令 id → 键位字符串」的 map,缺项回落默认,手改坏行不
//!   致整表失效。

use crate::command::Command;
use eframe::egui::{self, Modifiers};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

/// 落盘文件名(平台配置目录,与 `settings.json` 同级)。
const KEYMAP_FILE: &str = "keymap.json";

/// 快捷键:修饰键组合 + 主键。与 `egui::KeyboardShortcut` 一一对应,单独成
/// 结构是为了能 `Display` / `FromStr`(egui 只能单向格式化给用户看)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: Modifiers,
    pub key: egui::Key,
}

impl Shortcut {
    /// 转成 egui 的快捷键形态(供 `format_shortcut` 与 `consume_shortcut`)。
    pub fn keyboard(self) -> egui::KeyboardShortcut {
        egui::KeyboardShortcut::new(self.modifiers, self.key)
    }

    /// 是否可绑定:至少一个修饰键,或是功能键(裸字母/数字会吞掉输入,
    /// 方向键等留给编辑器导航,故只允许 F1-F12 这一类)。
    pub fn bindable(self) -> bool {
        if !self.modifiers.is_none() {
            return true;
        }
        matches!(
            self.key,
            egui::Key::F1
                | egui::Key::F2
                | egui::Key::F3
                | egui::Key::F4
                | egui::Key::F5
                | egui::Key::F6
                | egui::Key::F7
                | egui::Key::F8
                | egui::Key::F9
                | egui::Key::F10
                | egui::Key::F11
                | egui::Key::F12
        )
    }

    /// 平台化显示:`Cmd` / `Ctrl` / `⌥` 随编译目标(与 egui 的
    /// `format_shortcut` 同一口径,但这里还要能被 [`parse_shortcut`] 反解;
    /// mac 上 Alt 显示 ⌥,菜单里 egui 原生就是 ⌥T,#45 K1 对齐该口径)。
    pub fn platform_text(self) -> String {
        let mut text = String::new();
        let modifiers = self.modifiers;
        if modifiers.command {
            text.push_str(if cfg!(target_os = "macos") {
                "Cmd+"
            } else {
                "Ctrl+"
            });
        }
        if modifiers.alt {
            text.push_str(if cfg!(target_os = "macos") {
                "\u{2325}+"
            } else {
                "Alt+"
            });
        }
        if modifiers.shift {
            text.push_str("Shift+");
        }
        text.push_str(&key_text(self.key));
        text
    }
}

impl fmt::Display for Shortcut {
    /// 与 [`Shortcut::platform_text`] 同形态(跨平台存档用固定别名亦可,
    /// 见 [`parse_shortcut`] 的宽松解析)。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.platform_text())
    }
}

/// 解析键位文本。宽松:修饰键别名(`Ctrl`/`Cmd`/`Control` 同义,
/// `Alt`/`Option`/`⌥` 同义)与大小写都不敏感;`Ctrl+Shift+S` 与
/// `shift+ctrl+s` 等价。解析不了返回 `None`,由调用方落提示。
pub fn parse_shortcut(text: &str) -> Option<Shortcut> {
    let mut modifiers = Modifiers::NONE;
    let mut key = None;
    for part in text.split('+') {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "cmd" | "control" | "command" => modifiers |= Modifiers::COMMAND,
            // ⌥ 是 mac 显示形态(platform_text),非 ASCII,to_ascii_lowercase
            // 原样通过
            "alt" | "option" | "\u{2325}" => modifiers |= Modifiers::ALT,
            "shift" => modifiers |= Modifiers::SHIFT,
            other => {
                if key.is_some() {
                    return None; // 两个主键
                }
                key = Some(parse_key(other)?);
            }
        }
    }
    Some(Shortcut {
        modifiers,
        key: key?,
    })
}

/// 主键名的反向解析(与 [`key_text`] 严格互逆)。
fn parse_key(text: &str) -> Option<egui::Key> {
    let lower = text.to_ascii_lowercase();
    if lower.len() == 1 {
        let ch = lower.chars().next()?;
        return match ch {
            '0' => Some(egui::Key::Num0),
            '1' => Some(egui::Key::Num1),
            '2' => Some(egui::Key::Num2),
            '3' => Some(egui::Key::Num3),
            '4' => Some(egui::Key::Num4),
            '5' => Some(egui::Key::Num5),
            '6' => Some(egui::Key::Num6),
            '7' => Some(egui::Key::Num7),
            '8' => Some(egui::Key::Num8),
            '9' => Some(egui::Key::Num9),
            'a'..='z' => Some(upper_key(ch)?),
            _ => match ch {
                '/' => Some(egui::Key::Slash),
                '\\' => Some(egui::Key::Backslash),
                ',' => Some(egui::Key::Comma),
                '.' => Some(egui::Key::Period),
                '-' => Some(egui::Key::Minus),
                '=' => Some(egui::Key::Equals),
                ';' => Some(egui::Key::Semicolon),
                '[' => Some(egui::Key::OpenBracket),
                ']' => Some(egui::Key::CloseBracket),
                _ => None,
            },
        };
    }
    Some(match lower.as_str() {
        "space" => egui::Key::Space,
        "tab" => egui::Key::Tab,
        "enter" | "return" => egui::Key::Enter,
        "esc" | "escape" => egui::Key::Escape,
        "backspace" => egui::Key::Backspace,
        "delete" | "del" => egui::Key::Delete,
        "home" => egui::Key::Home,
        "end" => egui::Key::End,
        "pageup" => egui::Key::PageUp,
        "pagedown" => egui::Key::PageDown,
        "insert" | "ins" => egui::Key::Insert,
        "up" => egui::Key::ArrowUp,
        "down" => egui::Key::ArrowDown,
        "left" => egui::Key::ArrowLeft,
        "right" => egui::Key::ArrowRight,
        "backslash" => egui::Key::Backslash,
        "slash" => egui::Key::Slash,
        "comma" => egui::Key::Comma,
        "period" => egui::Key::Period,
        "minus" => egui::Key::Minus,
        "plus" | "equals" => egui::Key::Equals,
        _ => function_key(&lower)?,
    })
}

/// F1-F12。
fn function_key(lower: &str) -> Option<egui::Key> {
    let digits = lower.strip_prefix('f')?;
    let index: u8 = digits.parse().ok()?;
    Some(match index {
        1 => egui::Key::F1,
        2 => egui::Key::F2,
        3 => egui::Key::F3,
        4 => egui::Key::F4,
        5 => egui::Key::F5,
        6 => egui::Key::F6,
        7 => egui::Key::F7,
        8 => egui::Key::F8,
        9 => egui::Key::F9,
        10 => egui::Key::F10,
        11 => egui::Key::F11,
        12 => egui::Key::F12,
        _ => return None,
    })
}

/// 字母键:`Key::A` … `Key::Z`(egui 无「按字符取键」的 API,逐字母列出)。
fn upper_key(ch: char) -> Option<egui::Key> {
    Some(match ch {
        'a' => egui::Key::A,
        'b' => egui::Key::B,
        'c' => egui::Key::C,
        'd' => egui::Key::D,
        'e' => egui::Key::E,
        'f' => egui::Key::F,
        'g' => egui::Key::G,
        'h' => egui::Key::H,
        'i' => egui::Key::I,
        'j' => egui::Key::J,
        'k' => egui::Key::K,
        'l' => egui::Key::L,
        'm' => egui::Key::M,
        'n' => egui::Key::N,
        'o' => egui::Key::O,
        'p' => egui::Key::P,
        'q' => egui::Key::Q,
        'r' => egui::Key::R,
        's' => egui::Key::S,
        't' => egui::Key::T,
        'u' => egui::Key::U,
        'v' => egui::Key::V,
        'w' => egui::Key::W,
        'x' => egui::Key::X,
        'y' => egui::Key::Y,
        'z' => egui::Key::Z,
        _ => return None,
    })
}

/// [`parse_key`] 的逆:键 → 文本。
fn key_text(key: egui::Key) -> String {
    match key {
        egui::Key::Num0 => "0".to_owned(),
        egui::Key::Num1 => "1".to_owned(),
        egui::Key::Num2 => "2".to_owned(),
        egui::Key::Num3 => "3".to_owned(),
        egui::Key::Num4 => "4".to_owned(),
        egui::Key::Num5 => "5".to_owned(),
        egui::Key::Num6 => "6".to_owned(),
        egui::Key::Num7 => "7".to_owned(),
        egui::Key::Num8 => "8".to_owned(),
        egui::Key::Num9 => "9".to_owned(),
        egui::Key::Space => "Space".to_owned(),
        egui::Key::Tab => "Tab".to_owned(),
        egui::Key::Enter => "Enter".to_owned(),
        egui::Key::Escape => "Esc".to_owned(),
        egui::Key::Backspace => "Backspace".to_owned(),
        egui::Key::Delete => "Delete".to_owned(),
        egui::Key::Home => "Home".to_owned(),
        egui::Key::End => "End".to_owned(),
        egui::Key::PageUp => "PageUp".to_owned(),
        egui::Key::PageDown => "PageDown".to_owned(),
        egui::Key::Insert => "Insert".to_owned(),
        egui::Key::ArrowUp => "Up".to_owned(),
        egui::Key::ArrowDown => "Down".to_owned(),
        egui::Key::ArrowLeft => "Left".to_owned(),
        egui::Key::ArrowRight => "Right".to_owned(),
        egui::Key::Backslash => "Backslash".to_owned(),
        egui::Key::Slash => "Slash".to_owned(),
        egui::Key::Comma => "Comma".to_owned(),
        egui::Key::Period => "Period".to_owned(),
        egui::Key::Minus => "Minus".to_owned(),
        egui::Key::Equals => "Equals".to_owned(),
        egui::Key::Semicolon => "Semicolon".to_owned(),
        egui::Key::OpenBracket => "OpenBracket".to_owned(),
        egui::Key::CloseBracket => "CloseBracket".to_owned(),
        egui::Key::F1 => "F1".to_owned(),
        egui::Key::F2 => "F2".to_owned(),
        egui::Key::F3 => "F3".to_owned(),
        egui::Key::F4 => "F4".to_owned(),
        egui::Key::F5 => "F5".to_owned(),
        egui::Key::F6 => "F6".to_owned(),
        egui::Key::F7 => "F7".to_owned(),
        egui::Key::F8 => "F8".to_owned(),
        egui::Key::F9 => "F9".to_owned(),
        egui::Key::F10 => "F10".to_owned(),
        egui::Key::F11 => "F11".to_owned(),
        egui::Key::F12 => "F12".to_owned(),
        other => format!("{other:?}"),
    }
}

/// 绑定表:按 [`Command::ALL`] 顺序一一对应的可选绑定。
///
/// 用定长数组而非 map:命令集是编译期闭合的枚举,数组的「全覆盖」由
/// 类型保证(不会出现某个命令查不到绑定的情况)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keymap {
    /// 命令 id → 键位文本;缺项 = 用默认;值为空串 = 用户主动清除。
    bindings: BTreeMap<String, String>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::builtin()
    }
}

impl Keymap {
    /// 出厂默认值:全部取命令层的默认绑定。
    pub fn builtin() -> Self {
        let mut bindings = BTreeMap::new();
        for cmd in Command::ALL {
            if let Some(shortcut) = cmd.default_shortcut() {
                bindings.insert(
                    cmd.id().to_owned(),
                    Shortcut {
                        modifiers: shortcut.modifiers,
                        key: shortcut.logical_key,
                    }
                    .platform_text(),
                );
            }
        }
        Self { bindings }
    }

    /// 某命令当前的实际绑定;`None` = 未绑定(只能从菜单触发)。
    pub fn get(&self, cmd: Command) -> Option<Shortcut> {
        self.bindings.get(cmd.id()).and_then(|text| {
            // 手改坏行:解析不了按未绑定处理(不整表失效,不 panic)
            parse_shortcut(text)
        })
    }

    /// 撞键检测:返回占用同一键位的另一个命令。
    pub fn conflict(&self, cmd: Command, shortcut: Shortcut) -> Option<Command> {
        Command::ALL
            .into_iter()
            .find(|other| *other != cmd && self.get(*other) == Some(shortcut))
    }

    /// 写入绑定;`None` = 清除。不做冲突检测(那是归约层的职责,要落在
    /// 提示行里告诉用户撞了谁)。
    pub fn set(&mut self, cmd: Command, shortcut: Option<Shortcut>) {
        match shortcut {
            Some(shortcut) => {
                self.bindings
                    .insert(cmd.id().to_owned(), shortcut.platform_text());
            }
            None => {
                self.bindings.remove(cmd.id());
            }
        }
    }

    /// 单条恢复出厂。
    pub fn reset(&mut self, cmd: Command) {
        match cmd.default_shortcut() {
            Some(shortcut) => self.set(
                cmd,
                Some(Shortcut {
                    modifiers: shortcut.modifiers,
                    key: shortcut.logical_key,
                }),
            ),
            None => self.set(cmd, None),
        }
    }

    /// 全部恢复出厂。
    pub fn reset_all(&mut self) {
        *self = Self::builtin();
    }

    /// 与出厂默认是否有差异(设置页用于决定「全部重置」是否可点)。
    pub fn is_default(&self) -> bool {
        *self == Self::builtin()
    }

    /// 从目录读取;文件缺失 = 出厂默认。解析失败同样回落默认(坏配置不该
    /// 挡住启动,与 `theme::ThemeSettings::load` 同口径)。
    pub fn load_from(dir: &Path) -> Self {
        let path = dir.join(KEYMAP_FILE);
        let Ok(bytes) = std::fs::read(&path) else {
            return Self::builtin();
        };
        match serde_json::from_slice::<Keymap>(&bytes) {
            Ok(mut keymap) => {
                // 新命令加入后要向旧 keymap.json 增量补默认,但绝不能覆盖
                // 用户已有绑定(包括主动清除后缺项的旧语义)。只有「命令 id
                // 完全不存在」才补;这样 Ctrl+D/Ctrl+Shift+D/Ctrl+F 随升级
                // 自动可用,旧配置不需要手动点「全部恢复默认」。
                let mut changed = false;
                // #45 K1:主题默认键 Cmd/Ctrl+Shift+T → Alt+T。历史上增量
                // 写回会让老档里躺着「值恰等于旧默认」的 toggle_theme 条目
                // (那是当年落盘的出厂值,不是用户手笔)——不迁的话该键位
                // 一直被占,TabRestore(#45 K2)对老用户永远是哑键,且消费
                // 顺序在前的主题会抢先吞键。只迁「值 == 旧默认」的条目;
                // 用户真正的自定义(别的键)与主动清除(空串)分毫不动,
                // 与「不覆盖用户绑定」同口径。取舍全文见 decisions-pending
                // #69。
                let retired_theme_default = Shortcut {
                    modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
                    key: egui::Key::T,
                };
                let new_theme_default =
                    Command::ToggleTheme
                        .default_shortcut()
                        .map(|shortcut| Shortcut {
                            modifiers: shortcut.modifiers,
                            key: shortcut.logical_key,
                        });
                let stored_theme = keymap
                    .bindings
                    .get(Command::ToggleTheme.id())
                    .and_then(|text| parse_shortcut(text));
                if stored_theme == Some(retired_theme_default) {
                    if let Some(shortcut) = new_theme_default {
                        keymap.bindings.insert(
                            Command::ToggleTheme.id().to_owned(),
                            shortcut.platform_text(),
                        );
                        changed = true;
                    }
                }
                // M2(#60):mac 替换键改排 ⌘H → ⌥⌘F 的同构迁移(#45 K1
                // 先例)。⌘H 被 winit 默认菜单的 Hide 项持有(mac 死键,
                // decisions-pending #114),改排只发生在 mac 出厂表 —— 其余
                // 平台新默认 == 旧默认,`new != retired` 条件天然不命中,
                // Win/Linux 老档里的 Ctrl+H(当年落盘的出厂值)分毫不动,
                // 因此不需要平台 cfg 门,两条 CI 都能实测迁移本体。只迁
                // 「值 == 旧默认」的条目:用户真自定义与主动清除分毫不动。
                let retired_replace_default = Shortcut {
                    modifiers: Modifiers::COMMAND,
                    key: egui::Key::H,
                };
                let new_replace_default =
                    Command::ReplaceInDoc
                        .default_shortcut()
                        .map(|shortcut| Shortcut {
                            modifiers: shortcut.modifiers,
                            key: shortcut.logical_key,
                        });
                let stored_replace = keymap
                    .bindings
                    .get(Command::ReplaceInDoc.id())
                    .and_then(|text| parse_shortcut(text));
                if stored_replace == Some(retired_replace_default)
                    && new_replace_default != Some(retired_replace_default)
                {
                    if let Some(shortcut) = new_replace_default {
                        keymap.bindings.insert(
                            Command::ReplaceInDoc.id().to_owned(),
                            shortcut.platform_text(),
                        );
                        changed = true;
                    }
                }
                for cmd in Command::ALL {
                    if !keymap.bindings.contains_key(cmd.id()) {
                        if let Some(shortcut) = cmd.default_shortcut() {
                            keymap.bindings.insert(
                                cmd.id().to_owned(),
                                Shortcut {
                                    modifiers: shortcut.modifiers,
                                    key: shortcut.logical_key,
                                }
                                .platform_text(),
                            );
                            changed = true;
                        }
                    }
                }
                if changed {
                    if let Err(error) = keymap.save_to(dir) {
                        eprintln!("LaterMD: 新快捷键默认值写回失败: {error}");
                    }
                }
                keymap
            }
            Err(source) => {
                eprintln!("LaterMD: 快捷键配置解析失败,已回落默认: {source}");
                Self::builtin()
            }
        }
    }

    /// 落盘。目录不存在则创建。
    pub fn save_to(&self, dir: &Path) -> Result<(), String> {
        let path = dir.join(KEYMAP_FILE);
        let json = serde_json::to_string_pretty(self)
            .map_err(|source| format!("{}: {source}", path.display()))?;
        std::fs::create_dir_all(dir).map_err(|source| format!("{}: {source}", dir.display()))?;
        crate::persist::write_atomic(&path, json.as_bytes())
            .map_err(|source| format!("{}: {source}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Key;

    fn shortcut(key: Key) -> Shortcut {
        Shortcut {
            modifiers: Modifiers::COMMAND,
            key,
        }
    }

    /// 往返:默认表的每条绑定都能解析回同一个键位(显示文本可逆)。
    #[test]
    fn builtin_bindings_round_trip() {
        let keymap = Keymap::builtin();
        for cmd in Command::ALL {
            let Some(shortcut) = keymap.get(cmd) else {
                continue;
            };
            assert_eq!(
                parse_shortcut(&shortcut.platform_text()),
                Some(shortcut),
                "{cmd:?} 的键位文本可反解"
            );
        }
    }

    /// 解析宽松:大小写与别名不敏感,顺序无关。
    #[test]
    fn parse_is_lenient() {
        let expected = Shortcut {
            modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
            key: Key::S,
        };
        for text in ["Ctrl+Shift+S", "shift+cmd+s", "CTRL + SHIFT + s"] {
            assert_eq!(parse_shortcut(text), Some(expected), "{text}");
        }
        assert_eq!(
            parse_shortcut("F5"),
            Some(Shortcut {
                modifiers: Modifiers::NONE,
                key: Key::F5
            })
        );
        // ⌥ 是 mac 显示形态(platform_text 在 mac 输出 ⌥+T),存档/手改
        // 都要能反解
        assert_eq!(
            parse_shortcut("\u{2325}+T"),
            Some(Shortcut {
                modifiers: Modifiers::ALT,
                key: Key::T
            })
        );
        assert_eq!(parse_shortcut(""), None, "空串");
        assert_eq!(parse_shortcut("Ctrl+"), None, "缺主键");
        assert_eq!(parse_shortcut("Ctrl+S+X"), None, "两个主键");
        assert_eq!(parse_shortcut("Ctrl+Qwerty"), None, "认不出的键名");
    }

    /// 可绑定性:带修饰键即可;裸字母不可(会吞掉输入);功能键例外。
    #[test]
    fn bindable_requires_modifier_or_function_key() {
        assert!(shortcut(Key::S).bindable());
        assert!(!Shortcut {
            modifiers: Modifiers::NONE,
            key: Key::S
        }
        .bindable());
        assert!(Shortcut {
            modifiers: Modifiers::NONE,
            key: Key::F8
        }
        .bindable());
    }

    /// 改绑 / 清除 / 单条重置 / 全部重置,以及差异判定。
    #[test]
    fn set_clear_and_reset() {
        let mut keymap = Keymap::builtin();
        assert!(keymap.is_default());

        keymap.set(Command::Save, Some(shortcut(Key::K)));
        assert_eq!(keymap.get(Command::Save), Some(shortcut(Key::K)));
        assert!(!keymap.is_default());

        keymap.set(Command::Save, None);
        assert_eq!(keymap.get(Command::Save), None, "清除后未绑定");

        keymap.reset(Command::Save);
        assert_eq!(keymap.get(Command::Save), Some(shortcut(Key::S)));

        keymap.set(Command::Open, Some(shortcut(Key::K)));
        keymap.reset_all();
        assert!(keymap.is_default());
    }

    /// 撞键检测:同键位被另一命令占用即报出占用者;自己改自己不算撞。
    #[test]
    fn conflict_reports_holder() {
        let keymap = Keymap::builtin();
        assert_eq!(
            keymap.conflict(Command::SaveAs, shortcut(Key::S)),
            Some(Command::Save)
        );
        assert_eq!(keymap.conflict(Command::Save, shortcut(Key::S)), None);
    }

    /// 落盘往返 + 缺项回落默认 + 坏 JSON 不 panic。
    #[test]
    fn save_load_round_trip_and_corrupt_falls_back() {
        let dir = std::env::temp_dir().join(format!("latermd-keymap-{}", std::process::id()));
        let mut keymap = Keymap::builtin();
        keymap.set(Command::ExportHtml, Some(shortcut(Key::K)));
        keymap.save_to(&dir).unwrap();
        let loaded = Keymap::load_from(&dir);
        assert_eq!(loaded.get(Command::ExportHtml), Some(shortcut(Key::K)));
        // 缺项(手改删掉一行)回落默认
        assert_eq!(loaded.get(Command::Save), Some(shortcut(Key::S)));
        assert_eq!(
            loaded.get(Command::DuplicateSelection),
            Some(shortcut(Key::D))
        );
        assert_eq!(
            loaded.get(Command::DuplicateLine),
            Some(Shortcut {
                modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
                key: Key::D,
            })
        );
        assert_eq!(loaded.get(Command::FindInDoc), Some(shortcut(Key::F)));

        std::fs::write(dir.join(KEYMAP_FILE), b"{oops").unwrap();
        assert_eq!(Keymap::load_from(&dir), Keymap::builtin());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 替换命令(#17 M1)进出厂默认表;旧 `keymap.json` 增量迁移只给
    /// **缺失**的 command id 补默认,用户已有绑定绝不覆盖。
    /// (#60 M2:出厂默认按平台分化——mac = ⌥⌘F,其余 = Ctrl/Cmd+H,
    /// 按编译目标断言,两侧都在各自平台的 CI 上跑。)
    #[test]
    fn replace_in_doc_default_and_incremental_migration() {
        let replace_default = if cfg!(target_os = "macos") {
            Shortcut {
                modifiers: Modifiers::ALT | Modifiers::COMMAND,
                key: Key::F,
            }
        } else {
            shortcut(Key::H)
        };
        assert_eq!(
            Keymap::builtin().get(Command::ReplaceInDoc),
            Some(replace_default)
        );

        let dir =
            std::env::temp_dir().join(format!("latermd-keymap-replace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 旧版 json(替换命令尚不存在时的形态):无 replace_in_doc 条目
        std::fs::write(
            dir.join(KEYMAP_FILE),
            br#"{"bindings": {"save": "Ctrl+S"}}"#,
        )
        .unwrap();
        let loaded = Keymap::load_from(&dir);
        assert_eq!(
            loaded.get(Command::ReplaceInDoc),
            Some(replace_default),
            "缺失 id 补默认,旧配置无需手动重置"
        );
        assert_eq!(
            loaded.get(Command::Save),
            Some(shortcut(Key::S)),
            "既有条目原样保留"
        );

        // 用户已改绑:load_from 不得用默认值覆盖
        std::fs::write(
            dir.join(KEYMAP_FILE),
            br#"{"bindings": {"replace_in_doc": "Ctrl+R"}}"#,
        )
        .unwrap();
        let loaded = Keymap::load_from(&dir);
        assert_eq!(
            loaded.get(Command::ReplaceInDoc),
            Some(shortcut(Key::R)),
            "用户绑定不被默认覆盖"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// mac 替换键改排(⌘H → ⌥⌘F,#60 M2)的旧档迁移回归,照 #45 K1
    /// 主题迁移的形态分叉:老档躺着旧默认 Cmd+H → 迁 ⌥⌘F 且写回落盘;
    /// 别名/大小写变体同样按解析值识别;用户自定义与主动清除分毫不动;
    /// 缺失 id 走既有增量补默认。旧默认只在 mac 出厂表改排——其余平台
    /// 新默认 == 旧默认,迁移条件天然不命中,「Ctrl+H 出厂值落盘」的老档
    /// 保持原样(即现行默认,行为不变),该分支按编译目标断言。
    #[test]
    fn replace_default_migration_on_load() {
        let dir =
            std::env::temp_dir().join(format!("latermd-keymap-replace-m2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let retired = Shortcut {
            modifiers: Modifiers::COMMAND,
            key: Key::H,
        };
        let mac_default = Shortcut {
            modifiers: Modifiers::ALT | Modifiers::COMMAND,
            key: Key::F,
        };

        // 形态 ①:老档躺着旧默认值(当年增量写回落盘的出厂值)。mac:
        // 迁 ⌥⌘F 且写回;其余平台:新默认 == 旧默认,不迁也不必迁
        std::fs::write(
            dir.join(KEYMAP_FILE),
            br#"{"bindings": {"save": "Ctrl+S", "replace_in_doc": "Ctrl+H"}}"#,
        )
        .unwrap();
        let loaded = Keymap::load_from(&dir);
        let persisted = std::fs::read_to_string(dir.join(KEYMAP_FILE)).unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(
                loaded.get(Command::ReplaceInDoc),
                Some(mac_default),
                "旧默认 Cmd+H 迁到 ⌥⌘F"
            );
            let reloaded: Keymap = serde_json::from_str(&persisted).unwrap();
            assert_eq!(
                reloaded.get(Command::ReplaceInDoc),
                Some(mac_default),
                "迁移结果已写回 keymap.json:{persisted}"
            );
        } else {
            assert_eq!(
                loaded.get(Command::ReplaceInDoc),
                Some(retired),
                "非 mac:Ctrl+H 仍是现行默认,原样保留"
            );
        }
        assert_eq!(loaded.get(Command::Save), Some(shortcut(Key::S)));

        // 形态 ②:别名/大小写变体的旧默认(手改档)同样按解析值识别
        std::fs::write(
            dir.join(KEYMAP_FILE),
            br#"{"bindings": {"replace_in_doc": "cmd+h"}}"#,
        )
        .unwrap();
        let loaded = Keymap::load_from(&dir);
        if cfg!(target_os = "macos") {
            assert_eq!(loaded.get(Command::ReplaceInDoc), Some(mac_default));
        } else {
            assert_eq!(loaded.get(Command::ReplaceInDoc), Some(retired));
        }

        // 形态 ③:用户自定义键位 → 绝不覆盖(两侧平台同口径)
        std::fs::write(
            dir.join(KEYMAP_FILE),
            br#"{"bindings": {"replace_in_doc": "Ctrl+R"}}"#,
        )
        .unwrap();
        assert_eq!(
            Keymap::load_from(&dir).get(Command::ReplaceInDoc),
            Some(shortcut(Key::R)),
            "用户自定义不被迁移覆盖"
        );

        // 形态 ④:主动清除(空串)→ 保持未绑定,不借迁移复活
        std::fs::write(
            dir.join(KEYMAP_FILE),
            br#"{"bindings": {"replace_in_doc": ""}}"#,
        )
        .unwrap();
        assert_eq!(
            Keymap::load_from(&dir).get(Command::ReplaceInDoc),
            None,
            "主动清除的绑定不因默认改排复活"
        );

        // 形态 ⑤:老档没有该 id → 既有增量补默认直接补上当前默认
        std::fs::write(
            dir.join(KEYMAP_FILE),
            br#"{"bindings": {"save": "Ctrl+S"}}"#,
        )
        .unwrap();
        let current_default = if cfg!(target_os = "macos") {
            mac_default
        } else {
            retired
        };
        assert_eq!(
            Keymap::load_from(&dir).get(Command::ReplaceInDoc),
            Some(current_default),
            "缺失 id 补当前默认"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 跳转命令(#60 M1)进出厂默认表;旧 `keymap.json` 的增量迁移只给
    /// **缺失**的 command id 补默认,用户已有绑定绝不覆盖(与替换命令
    /// 同一口径,#17)。
    #[test]
    fn goto_line_default_and_incremental_migration() {
        // 出厂默认:Ctrl/Cmd+G
        assert_eq!(
            Keymap::builtin().get(Command::GotoLine),
            Some(shortcut(Key::G))
        );

        let dir = std::env::temp_dir().join(format!("latermd-keymap-goto-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 旧版 json(跳转命令尚不存在时的形态):无 goto_line 条目
        std::fs::write(
            dir.join(KEYMAP_FILE),
            br#"{"bindings": {"save": "Ctrl+S"}}"#,
        )
        .unwrap();
        let loaded = Keymap::load_from(&dir);
        assert_eq!(
            loaded.get(Command::GotoLine),
            Some(shortcut(Key::G)),
            "缺失 id 补默认,旧配置无需手动重置"
        );
        assert_eq!(
            loaded.get(Command::Save),
            Some(shortcut(Key::S)),
            "既有条目原样保留"
        );

        // 用户已改绑:load_from 不得用默认值覆盖
        std::fs::write(
            dir.join(KEYMAP_FILE),
            br#"{"bindings": {"goto_line": "Ctrl+J"}}"#,
        )
        .unwrap();
        let loaded = Keymap::load_from(&dir);
        assert_eq!(
            loaded.get(Command::GotoLine),
            Some(shortcut(Key::J)),
            "用户绑定不被默认覆盖"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Alt 的平台显示(#45 K1):Win/Linux = `Alt+T`,mac = `⌥+T`(与 egui
    /// `format_shortcut` 的 mac 符号口径对齐);按编译目标断言,两侧都在
    /// 各自平台的 CI 上跑。
    #[test]
    fn alt_platform_text_per_target() {
        let alt_t = Shortcut {
            modifiers: Modifiers::ALT,
            key: Key::T,
        };
        if cfg!(target_os = "macos") {
            assert_eq!(alt_t.platform_text(), "\u{2325}+T");
        } else {
            assert_eq!(alt_t.platform_text(), "Alt+T");
        }
    }

    /// 主题默认键改排(#45 K1)的旧档迁移回归:旧默认值(Cmd/Ctrl+Shift+T)
    /// 迁到 Alt+T 且写回落盘;用户自定义与主动清除分毫不动;缺失 id 走
    /// 既有增量补默认。取舍口径见 decisions-pending #69。
    #[test]
    fn theme_default_migration_on_load() {
        let dir =
            std::env::temp_dir().join(format!("latermd-keymap-theme-k1-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let alt_t = Shortcut {
            modifiers: Modifiers::ALT,
            key: Key::T,
        };
        let ctrl_shift_t = Shortcut {
            modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
            key: Key::T,
        };

        // 形态 ①:老档躺着旧默认值(当年增量写回落盘的出厂值)→ 迁 Alt+T,
        // 其他条目不动,迁移结果写回 keymap.json
        std::fs::write(
            dir.join(KEYMAP_FILE),
            br#"{"bindings": {"save": "Ctrl+S", "toggle_theme": "Ctrl+Shift+T"}}"#,
        )
        .unwrap();
        let loaded = Keymap::load_from(&dir);
        assert_eq!(
            loaded.get(Command::ToggleTheme),
            Some(alt_t),
            "旧默认值迁到新默认 Alt+T"
        );
        assert_eq!(loaded.get(Command::Save), Some(shortcut(Key::S)));
        // 迁移要写回落盘(下次启动不再重复迁移):按再次装载核验,不比对
        // 原文文本——Alt 的显示文本随平台是 Alt+/⌥
        let persisted = std::fs::read_to_string(dir.join(KEYMAP_FILE)).unwrap();
        let reloaded: Keymap = serde_json::from_str(&persisted).unwrap();
        assert_eq!(
            reloaded.get(Command::ToggleTheme),
            Some(alt_t),
            "迁移结果已写回 keymap.json:{persisted}"
        );

        // 形态 ②:别名/大小写变体的旧默认(手改档)同样按解析值识别迁移
        std::fs::write(
            dir.join(KEYMAP_FILE),
            br#"{"bindings": {"toggle_theme": "cmd+shift+t"}}"#,
        )
        .unwrap();
        assert_eq!(
            Keymap::load_from(&dir).get(Command::ToggleTheme),
            Some(alt_t),
            "别名形态的旧默认同样迁移"
        );

        // 形态 ③:用户自定义键位 → 绝不覆盖(#17 同口径)
        std::fs::write(
            dir.join(KEYMAP_FILE),
            br#"{"bindings": {"toggle_theme": "Ctrl+F9"}}"#,
        )
        .unwrap();
        let loaded = Keymap::load_from(&dir);
        assert_eq!(
            loaded.get(Command::ToggleTheme),
            Some(Shortcut {
                modifiers: Modifiers::COMMAND,
                key: Key::F9
            }),
            "用户自定义不被默认覆盖"
        );

        // 形态 ④:主动清除(空串)→ 保持未绑定,不借迁移复活
        std::fs::write(
            dir.join(KEYMAP_FILE),
            br#"{"bindings": {"toggle_theme": ""}}"#,
        )
        .unwrap();
        assert_eq!(
            Keymap::load_from(&dir).get(Command::ToggleTheme),
            None,
            "主动清除的绑定不因默认改排复活"
        );

        // 形态 ⑤:老档没有该 id → 既有增量补默认直接补上新默认
        std::fs::write(
            dir.join(KEYMAP_FILE),
            br#"{"bindings": {"save": "Ctrl+S"}}"#,
        )
        .unwrap();
        assert_eq!(
            Keymap::load_from(&dir).get(Command::ToggleTheme),
            Some(alt_t),
            "缺失 id 补新默认"
        );

        // 迁移后 Cmd/Ctrl+Shift+T 的占用者 = TabRestore(#45 K2 已落地,
        // 该键位由它合法持有),主题不再抢键 —— 存量档迁移后两键各归其主
        std::fs::write(
            dir.join(KEYMAP_FILE),
            br#"{"bindings": {"toggle_theme": "Ctrl+Shift+T"}}"#,
        )
        .unwrap();
        let loaded = Keymap::load_from(&dir);
        let holders: Vec<Command> = Command::ALL
            .into_iter()
            .filter(|cmd| loaded.get(*cmd) == Some(ctrl_shift_t))
            .collect();
        assert_eq!(
            holders,
            vec![Command::TabRestore],
            "Cmd/Ctrl+Shift+T 只被 TabRestore 持有(增量补默认的合法占用)"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
