//! 编辑器右键菜单的剪贴板/撤销动作:合成事件通道(macOS 原生菜单同源)。
//!
//! 剪切/复制/粘贴/撤销/重做/全选**不在 `Command` 命令表里** —— 它们由
//! `TextEdit` 内建消费对应的 `egui::Event`(egui 0.36.2 builder.rs:1114-1145;
//! 撤销/重做走 `Cmd/Ctrl+Z`、全选走 `Cmd/Ctrl+A`,物理按键时 egui-winit
//! 推的就是同一批事件)。右键菜单点这些条目 = 把同一批事件**合成**推入
//! 输入流,TextEdit 内建的剪贴板写出、选区删除与 undoer 快照全部照常工作
//! —— 不经 compose 层,撤销链不被打碎(mac_menu::dispatch 已走此路在产)。
//!
//! ## 时序红线
//!
//! 合成事件必须在 `logic()` 阶段推入:ui() 阶段推的事件当帧末被清空,
//! 下一帧 `begin_pass` 不复存在。菜单条目的点击发生在 ui()(弹层渲染帧),
//! 消息经 outbox 到**下一帧** logic():归约只记账
//! (`State::pending_edit_action` 记账位),`LaterMdApp::reduce` 在 apply 循环后
//! 取走并调 [`dispatch`]。推事件前把焦点还给编辑目标 —— 点击弹层条目的
//! press 帧已把编辑器焦点清掉,而 TextEdit 只在持焦时消费事件;归约在
//! ui() 之前跑,同帧 TextEdit 即可消费。
//!
//! 已知边界:Undo/Redo/SelectAll 以合成按键进入输入流,`poll_shortcuts`
//! 也会看到它们 —— 出厂键位表没有绑定 `Cmd/Ctrl+Z` 与 `Cmd/Ctrl+A` 的
//! 命令,默认无副作用;用户若手动把某命令绑到这两键,右键菜单动作会
//! 连带触发该命令(记 decisions-pending #145,不改)。

use eframe::egui::{self, Event, Key, Modifiers};

use crate::state::Message;

/// 编辑器级编辑动作(右键菜单「编辑」组)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    /// 撤销(`Cmd/Ctrl+Z` 合成按键,TextEdit undoer 消费)。
    Undo,
    /// 重做(`Cmd/Ctrl+Shift+Z`)。
    Redo,
    /// 剪切(`Event::Cut`:TextEdit 复制选区到剪贴板并删除,撤销链完整)。
    Cut,
    /// 复制(`Event::Copy`)。
    Copy,
    /// 粘贴(`Event::Paste`,文本来自系统剪贴板,见 [`system_paste_text`])。
    Paste,
    /// 全选(`Cmd/Ctrl+A` 合成按键)。
    SelectAll,
}

impl EditAction {
    /// 菜单文案。
    pub fn label(self) -> &'static str {
        match self {
            Self::Undo => "撤销",
            Self::Redo => "重做",
            Self::Cut => "剪切",
            Self::Copy => "复制",
            Self::Paste => "粘贴",
            Self::SelectAll => "全选",
        }
    }

    /// 菜单展示用的出厂快捷键(这组动作由 TextEdit 内建识别,**不经
    /// keymap** —— 与 macOS 原生菜单同口径,显示与物理按键始终一致)。
    pub fn shortcut(self) -> egui::KeyboardShortcut {
        let modifiers = match self {
            Self::Redo => Modifiers::COMMAND | Modifiers::SHIFT,
            Self::Undo | Self::Cut | Self::Copy | Self::Paste | Self::SelectAll => {
                Modifiers::COMMAND
            }
        };
        let key = match self {
            Self::Undo | Self::Redo => Key::Z,
            Self::Cut => Key::X,
            Self::Copy => Key::C,
            Self::Paste => Key::V,
            Self::SelectAll => Key::A,
        };
        egui::KeyboardShortcut::new(modifiers, key)
    }

    /// 全部动作:菜单渲染与测试穷举共用,出现顺序即菜单行序。
    pub const ALL: [EditAction; 6] = [
        Self::Undo,
        Self::Redo,
        Self::Cut,
        Self::Copy,
        Self::Paste,
        Self::SelectAll,
    ];

    /// 是否需要调用方先备好粘贴文本(只有 [`Self::Paste`])。
    pub fn needs_paste_text(self) -> bool {
        matches!(self, Self::Paste)
    }

    fn synthetic_event(self) -> Option<Event> {
        match self {
            Self::Copy => Some(Event::Copy),
            Self::Cut => Some(Event::Cut),
            Self::Undo | Self::Redo | Self::SelectAll => Some(Event::Key {
                key: if self == Self::SelectAll {
                    Key::A
                } else {
                    Key::Z
                },
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers {
                    shift: self == Self::Redo,
                    ..Modifiers::COMMAND
                },
            }),
            // 粘贴的载荷(剪贴板文本)由调用方提供,不走这里
            Self::Paste => None,
        }
    }
}

/// 推合成事件进当前帧输入流。`paste_text` 是粘贴载荷(生产 =
/// [`system_paste_text`],测试注入);粘贴无文本时返回兜底消息
/// (`ImagePasteRequested`,交给既有图片粘贴通道,mac_menu 同款),
/// 其余动作返回 `None`。调用方负责先 apply 返回的兜底消息。
pub fn dispatch(
    action: EditAction,
    ctx: &egui::Context,
    paste_text: Option<String>,
    focus: egui::Id,
) -> Option<Message> {
    ctx.memory_mut(|mem| mem.request_focus(focus));
    let event = match action {
        EditAction::Paste => match paste_text {
            Some(text) if !text.is_empty() => Event::Paste(text),
            // 剪贴板无文本(或只有图片):回落图片粘贴通道
            _ => return Some(Message::ImagePasteRequested),
        },
        other => other.synthetic_event().expect("非 Paste 动作都有合成事件"),
    };
    ctx.input_mut(|input| input.events.push(event));
    None
}

/// 读系统剪贴板文本(生产路径)。macOS 走 NSPasteboard(与原生菜单同源);
/// Linux/Windows 走 arboard —— 与 egui-winit 在 Ctrl+V 时的同步读取同一条
/// 路径、同一个阻塞风险面(X11 上要与剪贴板宿主进程握手),在 logic()
/// 阶段做与 mac_menu 先例一致;若真要后台化,剪贴板图片读取(clipboard.rs)
/// 的线程 + mpsc 手法现成。
#[cfg(target_os = "macos")]
pub fn system_paste_text() -> Option<String> {
    crate::mac_menu::pasteboard_text()
}

#[cfg(not(target_os = "macos"))]
pub fn system_paste_text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::RawInput;

    /// dispatch 推事件 + 归还焦点,均在同帧 run_ui 内可见。
    #[test]
    fn dispatch_pushes_synthetic_events() {
        let ctx = egui::Context::default();
        let focus = egui::Id::new("editor-under-test");
        let output = ctx.run_ui(RawInput::default(), |ui| {
            // 撤销:Cmd/Ctrl+Z 合成按键
            assert!(dispatch(EditAction::Undo, ui.ctx(), None, focus).is_none());
            assert!(ui.input(|input| input.events.iter().any(|event| matches!(
                event,
                Event::Key {
                    key: Key::Z,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.command && !modifiers.shift
            ))));
            // 重做:Cmd/Ctrl+Shift+Z
            assert!(dispatch(EditAction::Redo, ui.ctx(), None, focus).is_none());
            assert!(ui.input(|input| input.events.iter().any(|event| matches!(
                event,
                Event::Key {
                    key: Key::Z,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.command && modifiers.shift
            ))));
            // 全选:Cmd/Ctrl+A
            assert!(dispatch(EditAction::SelectAll, ui.ctx(), None, focus).is_none());
            assert!(ui.input(|input| input.events.iter().any(|event| matches!(
                event,
                Event::Key {
                    key: Key::A,
                    pressed: true,
                    ..
                }
            ))));
            // 复制/剪切:内建事件
            assert!(dispatch(EditAction::Copy, ui.ctx(), None, focus).is_none());
            assert!(ui.input(|input| input.events.contains(&Event::Copy)));
            assert!(dispatch(EditAction::Cut, ui.ctx(), None, focus).is_none());
            assert!(ui.input(|input| input.events.contains(&Event::Cut)));
            // 粘贴:载荷文本进事件
            assert!(dispatch(EditAction::Paste, ui.ctx(), Some("文本".into()), focus).is_none());
            assert!(ui.input(|input| input
                .events
                .iter()
                .any(|event| matches!(event, Event::Paste(text) if text == "文本"))));
            // 焦点已归还(request_focus 即刻生效)
            assert!(ui.ctx().memory(|mem| mem.has_focus(focus)));
        });
        output.drop_without_applying_deltas();
    }

    /// 粘贴无文本(剪贴板为空或只有图片):回落图片粘贴通道(mac_menu
    /// 同款),不推粘贴事件。
    #[test]
    fn paste_without_text_falls_back_to_image_paste() {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(RawInput::default(), |ui| {
            for paste_text in [None, Some(String::new())] {
                let fallback = dispatch(EditAction::Paste, ui.ctx(), paste_text, egui::Id::NULL);
                assert_eq!(fallback, Some(Message::ImagePasteRequested));
                assert!(!ui.input(|input| input
                    .events
                    .iter()
                    .any(|event| { matches!(event, Event::Paste(_)) })));
            }
        });
        output.drop_without_applying_deltas();
    }

    /// 快捷键显示与出厂口径一致(不经 keymap,显示即物理按键)。
    #[test]
    fn shortcut_display_covers_all_actions() {
        for action in EditAction::ALL {
            let shortcut = action.shortcut();
            assert!(
                shortcut.modifiers.command,
                "{} 应带主修饰键",
                action.label()
            );
        }
        assert_eq!(EditAction::Paste.shortcut().logical_key, Key::V);
        assert_eq!(EditAction::SelectAll.shortcut().logical_key, Key::A);
        assert_eq!(EditAction::Redo.shortcut().logical_key, Key::Z);
    }
}
