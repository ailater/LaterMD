//! macOS 原生菜单与 egui 消息入口。命令分组复用旧菜单的 MENUS，
//! AppKit 回调只入队并唤醒帧，执行仍走应用的归约/编辑器事件路径。

use std::cell::RefCell;

use eframe::egui::{self, Key, Modifiers};
use objc2::rc::Retained;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem, NSPasteboard, NSPasteboardTypeString,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSObject, NSObjectProtocol, NSProcessInfo, NSString,
};

use crate::command::Command;
use crate::keymap::Shortcut;
use crate::settings::SettingsTab;
use crate::state::{Message, State};
use crate::ui::menubar::{toggle_checked, MENUS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditAction {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Command(Command),
    Settings(SettingsTab),
    About,
    Edit(EditAction),
    /// 安装命令行工具(2026-10-23 mac 反馈 #3):软链 latermd 到
    /// /usr/local/bin,管理员授权在后台线程的 osascript 里弹。无快捷键。
    InstallCli,
}

struct TargetIvars {
    ctx: egui::Context,
    actions: RefCell<Vec<Action>>,
    pending: RefCell<Vec<Action>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements. The target and its
    // RefCells live on the AppKit main thread; no Rust state is borrowed by UI callbacks.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetIvars]
    struct MenuTarget;

    unsafe impl NSObjectProtocol for MenuTarget {}

    impl MenuTarget {
        #[unsafe(method(latermdMenuAction:))]
        fn activate(&self, sender: &NSMenuItem) {
            let action = usize::try_from(sender.tag()).ok()
                .and_then(|index| self.ivars().actions.borrow().get(index).copied());
            if let Some(action) = action {
                self.ivars().pending.borrow_mut().push(action);
                self.ivars().ctx.request_repaint();
            }
        }
    }
);

/// 保留 target 的所有权：NSMenuItem 的 target 是弱引用。
/// Headless tests 的 App::default 不安装菜单，也不碰全局 NSApplication。
pub struct NativeMenu {
    target: Retained<MenuTarget>,
    items: Vec<(Retained<NSMenuItem>, Action)>,
    edits: RefCell<Vec<EditAction>>,
}

fn item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<objc2::runtime::Sel>,
) -> Retained<NSMenuItem> {
    // SAFETY: selectors are either our typed callback or standard AppKit actions.
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str(title),
            action,
            ns_string!(""),
        )
    }
}

fn submenu(mtm: MainThreadMarker, parent: &NSMenu, title: &str) -> Retained<NSMenu> {
    let menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(title));
    let entry = item(mtm, title, None);
    entry.setSubmenu(Some(&menu));
    parent.addItem(&entry);
    menu
}

impl NativeMenu {
    pub fn install(ctx: &egui::Context, state: &State) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let target = MenuTarget::alloc(mtm).set_ivars(TargetIvars {
            ctx: ctx.clone(),
            actions: RefCell::default(),
            pending: RefCell::default(),
        });
        // SAFETY: NSObject's init initializes the allocated subclass.
        let target = unsafe { msg_send![super(target), init] };
        let mut this = Self {
            target,
            items: Vec::new(),
            edits: RefCell::default(),
        };
        NSProcessInfo::processInfo().setProcessName(ns_string!("LaterMD"));
        let app = NSApplication::sharedApplication(mtm);
        let bar = NSMenu::new(mtm);
        let application = submenu(mtm, &bar, "LaterMD");
        this.add(mtm, &application, "关于 LaterMD…", Action::About);
        application.addItem(&NSMenuItem::separatorItem(mtm));
        this.add(
            mtm,
            &application,
            "设置…",
            Action::Settings(SettingsTab::Appearance),
        );
        this.add(mtm, &application, "安装命令行工具…", Action::InstallCli);
        application.addItem(&NSMenuItem::separatorItem(mtm));
        let services = submenu(mtm, &application, "服务");
        // SAFETY: AppKit retains the menu; all calls are on the main thread.
        app.setServicesMenu(Some(&services));
        for (label, selector, shortcut) in [
            (
                "隐藏 LaterMD",
                sel!(hide:),
                Some((Key::H, Modifiers::COMMAND)),
            ),
            (
                "隐藏其他",
                sel!(hideOtherApplications:),
                Some((Key::H, Modifiers::COMMAND | Modifiers::ALT)),
            ),
            ("显示全部", sel!(unhideAllApplications:), None),
            (
                "退出 LaterMD",
                sel!(terminate:),
                Some((Key::Q, Modifiers::COMMAND)),
            ),
        ] {
            let entry = item(mtm, label, Some(selector));
            set_shortcut(
                &entry,
                shortcut.map(|(key, modifiers)| Shortcut { key, modifiers }),
            );
            application.addItem(&entry);
        }
        for (title, _, sections) in MENUS {
            let menu = submenu(mtm, &bar, if title == "视图" { "显示" } else { title });
            if title == "编辑" {
                for (label, action) in [
                    ("撤销", EditAction::Undo),
                    ("重做", EditAction::Redo),
                    ("剪切", EditAction::Cut),
                    ("复制", EditAction::Copy),
                    ("粘贴", EditAction::Paste),
                    ("全选", EditAction::SelectAll),
                ] {
                    this.add(mtm, &menu, label, Action::Edit(action));
                }
                menu.addItem(&NSMenuItem::separatorItem(mtm));
            }
            for (index, section) in sections.iter().enumerate() {
                if index > 0 {
                    menu.addItem(&NSMenuItem::separatorItem(mtm));
                }
                for &command in *section {
                    this.add(mtm, &menu, command.label(), Action::Command(command));
                }
            }
        }
        let settings = submenu(mtm, &bar, "设置");
        for tab in SettingsTab::ALL {
            this.add(mtm, &settings, tab.label(), Action::Settings(tab));
        }
        let windows = submenu(mtm, &bar, "窗口");
        let minimize = item(mtm, "最小化", Some(sel!(performMiniaturize:)));
        set_shortcut(
            &minimize,
            Some(Shortcut {
                key: Key::M,
                modifiers: Modifiers::COMMAND,
            }),
        );
        windows.addItem(&minimize);
        windows.addItem(&item(mtm, "关闭窗口", Some(sel!(performClose:))));
        let help = submenu(mtm, &bar, "帮助");
        this.add(mtm, &help, "关于 LaterMD…", Action::About);
        app.setMainMenu(Some(&bar));
        if let Some(first) = bar.itemAtIndex(0) {
            first.setTitle(ns_string!("LaterMD"));
        }
        this.sync(state);
        Some(this)
    }

    fn add(&mut self, mtm: MainThreadMarker, menu: &NSMenu, label: &str, action: Action) {
        let entry = item(mtm, label, Some(sel!(latermdMenuAction:)));
        let mut actions = self.target.ivars().actions.borrow_mut();
        entry.setTag(actions.len() as isize);
        actions.push(action);
        // SAFETY: target implements latermdMenuAction: and lives as long as the app.
        unsafe {
            entry.setTarget(Some(&self.target));
        }
        menu.addItem(&entry);
        self.items.push((entry, action));
    }

    pub fn sync(&self, state: &State) {
        for (entry, action) in &self.items {
            let binding = binding(*action, state);
            // 改键捕获时让 AppKit 放行按键给 egui，而不是吞掉作为命令。
            set_shortcut(
                entry,
                if state.settings.capture.is_some() {
                    None
                } else {
                    binding
                },
            );
            let checked = match action {
                Action::Command(cmd) => toggle_checked(*cmd, state).unwrap_or(false),
                _ => false,
            };
            entry.setState(isize::from(checked));
        }
    }

    pub fn drain(&self, ctx: &egui::Context, outbox: &mut Vec<Message>) {
        let pending = std::mem::take(&mut *self.target.ivars().pending.borrow_mut());
        for action in pending {
            if let Action::Edit(edit) = action {
                self.edits.borrow_mut().push(edit);
            } else {
                dispatch(action, ctx, outbox);
            }
        }
    }

    pub fn finish_frame(&self, ctx: &egui::Context, outbox: &mut Vec<Message>, state: &State) {
        // 编辑动作在全局快捷键归约后交给聚焦的 TextEdit，防止菜单「撤销」
        // 合成的 Cmd+Z 被用户改绑的全局命令再次消费。
        for edit in std::mem::take(&mut *self.edits.borrow_mut()) {
            dispatch(Action::Edit(edit), ctx, outbox);
        }
        if !outbox.is_empty() {
            ctx.request_repaint();
        }
        self.sync(state);
    }
}

fn binding(action: Action, state: &State) -> Option<Shortcut> {
    let (key, modifiers) = match action {
        Action::Command(command) => return state.keymap.get(command),
        Action::Settings(SettingsTab::Appearance) => (Key::Comma, Modifiers::COMMAND),
        Action::Edit(action) => match action {
            EditAction::Undo => (Key::Z, Modifiers::COMMAND),
            EditAction::Redo => (Key::Z, Modifiers::COMMAND | Modifiers::SHIFT),
            EditAction::Cut => (Key::X, Modifiers::COMMAND),
            EditAction::Copy => (Key::C, Modifiers::COMMAND),
            EditAction::Paste => (Key::V, Modifiers::COMMAND),
            EditAction::SelectAll => (Key::A, Modifiers::COMMAND),
        },
        _ => return None,
    };
    // 用户给命令改绑后，内建编辑项不能抢占同一组合键。
    let shortcut = Shortcut { key, modifiers };
    (!Command::ALL
        .iter()
        .any(|&cmd| state.keymap.get(cmd) == Some(shortcut)))
    .then_some(shortcut)
}

/// 读系统剪贴板文本(`edit_events::system_paste_text` 的 mac 实现):
/// 原生菜单「粘贴」与编辑器右键菜单共用同一条读取路径,与 egui-winit
/// 在 Ctrl+V 的同步读取同口径。
pub(crate) fn pasteboard_text() -> Option<String> {
    let board = NSPasteboard::generalPasteboard();
    // SAFETY: Apple's constant identifies the UTF-8 text pasteboard type.
    let text = unsafe { board.stringForType(NSPasteboardTypeString) }?;
    Some(text.to_string())
}

fn dispatch(action: Action, ctx: &egui::Context, outbox: &mut Vec<Message>) {
    match action {
        Action::Command(cmd) => outbox.push(cmd.message()),
        Action::Settings(tab) => outbox.push(Message::SettingsOpened(tab)),
        Action::About => outbox.push(Message::AboutOpened),
        Action::InstallCli => outbox.push(Message::InstallCliRequested),
        Action::Edit(edit) => {
            let event = match edit {
                EditAction::Copy => egui::Event::Copy,
                EditAction::Cut => egui::Event::Cut,
                EditAction::Paste => {
                    // 与 egui-winit 一致：文本同步从系统剪贴板读入；
                    // 无文本时复用后台图片读取，避免菜单粘贴丢失图片能力。
                    // 读取实现在 `edit_events::system_paste_text`(mac 分支
                    // 委托到这里),与编辑器右键菜单共用同一条路径。
                    match crate::edit_events::system_paste_text() {
                        Some(text) => egui::Event::Paste(text),
                        None => {
                            outbox.push(Message::ImagePasteRequested);
                            return;
                        }
                    }
                }
                _ => egui::Event::Key {
                    key: if edit == EditAction::SelectAll {
                        Key::A
                    } else {
                        Key::Z
                    },
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers {
                        shift: edit == EditAction::Redo,
                        ..Modifiers::COMMAND
                    },
                },
            };
            ctx.input_mut(|input| input.events.push(event));
        }
    }
}

fn set_shortcut(item: &NSMenuItem, shortcut: Option<Shortcut>) {
    let (key, flags) = shortcut
        .map(native_shortcut)
        .unwrap_or_else(|| (String::new(), NSEventModifierFlags::empty()));
    let key = NSString::from_str(&key);
    if *item.keyEquivalent() != *key {
        item.setKeyEquivalent(&key);
    }
    if item.keyEquivalentModifierMask() != flags {
        item.setKeyEquivalentModifierMask(flags);
    }
}

fn native_shortcut(shortcut: Shortcut) -> (String, NSEventModifierFlags) {
    let mut flags = NSEventModifierFlags::empty();
    let m = shortcut.modifiers;
    if m.command || m.mac_cmd {
        flags |= NSEventModifierFlags::Command;
    }
    if m.ctrl {
        flags |= NSEventModifierFlags::Control;
    }
    if m.alt {
        flags |= NSEventModifierFlags::Option;
    }
    if m.shift {
        flags |= NSEventModifierFlags::Shift;
    }
    let special = match shortcut.key {
        Key::ArrowUp => Some('\u{f700}'),
        Key::ArrowDown => Some('\u{f701}'),
        Key::ArrowLeft => Some('\u{f702}'),
        Key::ArrowRight => Some('\u{f703}'),
        Key::F1 => Some('\u{f704}'),
        Key::F2 => Some('\u{f705}'),
        Key::F3 => Some('\u{f706}'),
        Key::F4 => Some('\u{f707}'),
        Key::F5 => Some('\u{f708}'),
        Key::F6 => Some('\u{f709}'),
        Key::F7 => Some('\u{f70a}'),
        Key::F8 => Some('\u{f70b}'),
        Key::F9 => Some('\u{f70c}'),
        Key::F10 => Some('\u{f70d}'),
        Key::F11 => Some('\u{f70e}'),
        Key::F12 => Some('\u{f70f}'),
        Key::Insert => Some('\u{f727}'),
        Key::Delete => Some('\u{f728}'),
        Key::Home => Some('\u{f729}'),
        Key::End => Some('\u{f72b}'),
        Key::PageUp => Some('\u{f72c}'),
        Key::PageDown => Some('\u{f72d}'),
        Key::Colon => Some(':'),
        Key::Comma => Some(','),
        Key::Minus => Some('-'),
        Key::Period => Some('.'),
        Key::Plus => Some('+'),
        Key::Equals => Some('='),
        Key::Semicolon => Some(';'),
        Key::Backslash => Some('\\'),
        Key::Slash => Some('/'),
        Key::Pipe => Some('|'),
        Key::Questionmark => Some('?'),
        Key::Exclamationmark => Some('!'),
        Key::OpenBracket => Some('['),
        Key::CloseBracket => Some(']'),
        Key::OpenCurlyBracket => Some('{'),
        Key::CloseCurlyBracket => Some('}'),
        Key::Backtick => Some('`'),
        Key::Quote => Some('\''),
        Key::Tab => Some('\t'),
        Key::Enter => Some('\r'),
        Key::Escape => Some('\u{1b}'),
        Key::Backspace => Some('\u{7f}'),
        Key::Space => Some(' '),
        _ => None,
    };
    let key = special
        .map(|c| c.to_string())
        .unwrap_or_else(|| shortcut.key.name().to_lowercase());
    (key, flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_actions_reach_document_settings_and_about() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        let mut outbox = Vec::new();
        state.tabs.current_mut().editor.replace_all("中文 document");
        state.tabs.current_mut().selection = Some((0, 2));
        dispatch(Action::Command(Command::FormatBold), &ctx, &mut outbox);
        for message in outbox.drain(..) {
            state.apply(message);
        }
        assert_eq!(state.tabs.current().editor.text(), "**中文** document");
        for tab in SettingsTab::ALL {
            dispatch(Action::Settings(tab), &ctx, &mut outbox);
            for message in outbox.drain(..) {
                state.apply(message);
            }
            assert!(state.settings.open);
            assert_eq!(state.settings.tab, tab);
        }
        dispatch(Action::About, &ctx, &mut outbox);
        for message in outbox.drain(..) {
            state.apply(message);
        }
        assert!(state.about.open);
    }

    #[test]
    fn native_shortcuts_cover_defaults_and_follow_rebinding() {
        let mut state = State::default();
        for cmd in Command::ALL {
            if let Some(shortcut) = binding(Action::Command(cmd), &state) {
                let (key, _) = native_shortcut(shortcut);
                assert_eq!(
                    key.chars().count(),
                    1,
                    "{cmd:?} has invalid AppKit key {key:?}"
                );
            }
        }
        let custom = Shortcut {
            key: Key::Z,
            modifiers: Modifiers::COMMAND,
        };
        state.keymap.set(Command::Save, Some(custom));
        assert_eq!(
            binding(Action::Command(Command::Save), &state),
            Some(custom)
        );
        assert_eq!(binding(Action::Edit(EditAction::Undo), &state), None);
        state.keymap.set(Command::Save, None);
        assert_eq!(binding(Action::Command(Command::Save), &state), None);
        assert_eq!(
            binding(Action::Edit(EditAction::Undo), &state),
            Some(custom)
        );
        let (key, flags) = native_shortcut(Shortcut {
            key: Key::Period,
            modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
        });
        assert_eq!(key, ".");
        assert!(flags.contains(NSEventModifierFlags::Command | NSEventModifierFlags::Shift));
    }

    #[test]
    fn native_edit_actions_reach_focused_text_edit() {
        let ctx = egui::Context::default();
        let mut text = "中文 document".to_owned();
        let mut outbox = Vec::new();
        let mut time = 0.0;
        let mut frame = |action: Option<EditAction>, events: Vec<egui::Event>| {
            time += 1.0;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    if let Some(action) = action {
                        dispatch(Action::Edit(action), ui.ctx(), &mut outbox);
                    }
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut text).id(egui::Id::new("menu-edit-test")),
                    );
                    response.request_focus();
                },
            );
            output.textures_delta.clear();
            output
        };
        let _ = frame(None, vec![]);
        let _ = frame(Some(EditAction::SelectAll), vec![]);
        let copied = frame(Some(EditAction::Copy), vec![]);
        assert!(copied.platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::CopyText(value) if value == "中文 document")
        }));
        let _ = frame(Some(EditAction::Cut), vec![]);
        let _ = frame(Some(EditAction::Undo), vec![]);
        let restored = frame(Some(EditAction::Copy), vec![]);
        assert!(restored.platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::CopyText(value) if value == "中文 document")
        }));
        let _ = frame(Some(EditAction::Redo), vec![]);
        let copied = frame(Some(EditAction::Copy), vec![]);
        assert!(!copied.platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::CopyText(value) if !value.is_empty())
        }));
        assert!(text.is_empty());
        assert!(outbox.is_empty());
    }
}
