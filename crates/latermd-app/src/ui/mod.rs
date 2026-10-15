//! UI 子模块。绘制都在这里;状态归约只在 `App::logic`(铁律,docs/adr-005 §2.3)。

pub mod about;
pub mod ai_card;
pub mod editor;
pub mod editor_menu;
pub mod emoji_data;
pub mod emoji_panel;
pub mod fade;
pub mod focus;
pub mod format_bar;
pub mod gutter;
pub mod icons;
pub mod image_dialog;
pub mod layout;
pub mod menubar;
pub mod mermaid;
pub mod minimap;
pub mod preview;
pub mod quick_open;
pub mod rich_widgets;
pub mod selection_ai;
pub mod sidebar;
pub mod tabs;
pub mod titlebar;
pub mod tokens;
pub mod typewriter;
pub mod workbench;
pub mod zen_nav;

/// #39 M1 切换卡顿取证 harness:纯测试模块,生产构建不编译。
#[cfg(test)]
mod tab_switch_perf;

/// #59 perf-round M1 全面取证 harness(大文档编辑帧/滚动稳态帧/冷首切分解/
/// 应用启动):纯测试模块,生产构建不编译。
#[cfg(test)]
mod perf_finding;
