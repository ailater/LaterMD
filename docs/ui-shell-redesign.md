# 外壳重构规格：右导航 + 源码 + 只读预览三分栏

日期：2026-09-26
状态：待坤哥拍板（D1–D5 五个决策点，均已给出默认选择，见 §2）
关联：[adr-005](adr-005-layout-and-sidebar.md)（布局与二分）、[ui-polish.md](ui-polish.md)（图标/工具栏/状态栏/设置）、[ui-design.md](ui-design.md)（视觉词库与 token）、[decisions-pending #28](decisions-pending.md)（WorkBuddy 风外壳已落地的事）

> 本文只写**规格与决策**，不重复 ADR 的论证。
> 触发：坤哥看当前界面后要求「三分栏：左导航 / 中源码 / 右只读渲染；左侧顶动作、中文件树、底设置；
> 中间源码栏顶部要有 Markdown 快捷工具条；右上角补 关闭左侧 / 关闭右侧；再加一个只显示右预览的禅定模式」。

---

## 1. 目标形态

```
┌───────────────────────────────────────────────────────────────────────┐
│ ⠿  a.md *                  ⌈左│ ⌈右│ ⦿ │ ⚙ │ ─ │ ⤢ │ ✕ │  ← ① 自绘标题栏
├───────────────────────────────────────────────────────────────────────┤
│ 文件 ▾ 导出 ▾ 视图 ▾ AI ▾ 设置 ▾                                      │  ← ② 菜单栏(D2)
├──────────┬──────────────────────────┬─────────────────────────────────┤
│ ③ 动作行 │ ④ 标签条                 │ ⑥ 只读预览                      │
│ 新建打保存│ [a.md ×][b.md ×]        │                                 │
│ 导出     ├──────────────────────────┤  # 标题                          │
│ ───────  │ ⑤ B I S ‹› " ¶ ≡ H1 H2 …  │  正文渲染结果（不可编辑）        │
│ ④ 视图导航│ ─────────────────────── │                                 │
│ ▸ 文件树 │ # 标题                    │  ```rust                        │
│   搜索   │ 正文源码…                 │  fn main() {}                   │
│   大纲   │                          │  ```                             │
│   Git    │                          │                                 │
│          │                          │                                 │
│ (文件树) │                          │                                 │
├──────────┴──────────────────────────┴─────────────────────────────────┤
│ a.md · 行 12:4 · 1,204 字 · 深色 · wgpu · AI:Mock · MCP:关              │
└───────────────────────────────────────────────────────────────────────┘
```

> 2026-09-27 修订（用户指令，decisions-pending #31）：左栏底段「⚙ 设置」行取消，
> 设置入口挪到标题栏右端齿轮（上图的 ⚙，最小化左侧）；状态栏横跨全窗底部
> （在左右栏之前画）。原 M2 版图见 git 历史。

**禅定模式（⑥ 独占全窗）**：①②③④ 全部退场，只剩居中的只读渲染正文（限宽 720px），
右上角浮出「退出禅定」。

### 与现状的差距（逐条对照）

| 坤哥要的 | 现状 | 改造量 |
|---|---|---|
| 三分栏 | 已是三栏（`Panel::left` × 2 + `CentralPanel`） | 小：把「编辑器」与「预览」的左右关系对调（见 §4） |
| 左导航顶动作 → 中文件树 → 底设置 | 左栏是「页签栏 + 页内容」，无文件动作、无设置入口 | 中：按 §5 三段重写 |
| 中间源码栏顶部 Markdown 工具条 | 中间栏顶部只有文件动作按钮，没有格式动作 | 大：新增 `compose.rs` + `ui/format_bar.rs`（§6） |
| 右上 关闭左侧 / 关闭右侧 | 无（现有နျ `ToggleSidebar` 只管左侧，且藏在菜单/工具栏） | 中：自绘标题栏（§3） |
| 禅定模式 | 无 | 中：新增第四种可见性组合（§7） |
| 右侧只读 + 后期可视化编辑 | 已经是只读（vendored `MarkdownLabel`） | 零；接口预留见 §8 |

---

## 2. 决策点（坤哥拍板用，括号内是我的默认）

| # | 决策 | 默认选择 | 代价 / 备选 |
|---|---|---|---|
| **D1** | **自绘无边框标题栏** vs **保留原生装饰 + 在菜单栏右端放面板开关** | **自绘**（`decorations(false)` + 自绘 36px 条，含 最小化/关闭左/关闭右/最大化/关闭 五个按钮） | 自绘要自己兜三个平台的拖拽缩放（见 §3.3 风险）。备选：保留原生装饰，开关按钮退到菜单栏右端——位置感从「右上角」降到「第二行右侧」。**逃生口**：`LATERMD_NATIVE_DECORATIONS=1` 回落原生装饰（与既有 `LATERMD_RENDERER=glow` 同构） |
| **D2** | 菜单栏是否并入标题栏的 ☰ | **保留独立菜单栏行**（文件/导出/视图/AI/设置） | 多占 ~28px。合并后垂直空间给正文，但 13 个命令的可发现性从「一行可见」降到「藏进 ☰」，违反 ui-polish §1.2 定的分工。合并留作二期可选项 |
| **D3** | 左栏形态：**VS Code activity bar**（左侧再劈一条 48px 图标条 + 二级侧栏）vs **单栏三段式**（顶动作 / 中视图 / 底设置） | **单栏三段式** | activity bar 要两个 panel + 状态更复杂，收益是「面板切换更省地方」；本产品只有 4 个视图，三段式足够且改动集中在 `ui/sidebar.rs` |
| **D4** | 禅定是否连标题栏一起隐藏 | **保留标题栏**（鼠标移到顶部 2s 后淡出，进入时不淡出） | 全无 chrome 的「字 processor」更纯粹，但「怎么退出」就只剩 Esc，可发现性差且 macOS 全屏手势会打架。淡出留 v2 |
| **D5** | 右预览的「后期可视化编辑」现在是否预留接口 | **预留位置不实现**（只在文档里钉死接入点与不变量） | 实现 = Live Preview v2 起跳，属 P3 深水区剩余项，本轮不碰 |

---

## 3. ① 自绘标题栏（D1）

### 3.1 结构

```
│ ⠿ 文档名*                                    │ ┃左 │ ┃右 │ ⦿ │ ─ │ ⤢ │ ✕ │
```
- 左：应用标记（自绘方角矩形/或用既有 `Icon::Files`）+ 当前标签文件名 + dirty 星号（复用 `DocumentState::display_name`）。
- 右：五个自绘按钮，**顺序与语义**：

| 按钮 | 语义 | hover 提示 |
|---|---|---|
| `┃左` | 左侧导航 开/关（直接翻转 `LayoutState::left`） | 「关闭左侧 / 打开左侧（Ctrl+\）」 |
| `┃右` | 右侧预览 开/关 | 「关闭右侧（Ctrl+Alt+R）」 |
| `⦿` | 禅定模式 | 「禅定模式（F11）」 |
| `⚙` | 设置（2026-09-27 增，左键默认页 / 右键四页直达，decisions-pending #31） | 「设置（右键直达各页）」 |
| `─` | 最小化 | — |
| `⤢` | 最大化 / 还原（按当前状态显示不同图标） | — |
| `✕` | 关闭窗口 | — |

> 七个按钮排成一组；`✕` 的 hover 底取 `DANGER`（既有 token，`ui/tokens.rs`），
> 其余取 `hover` token。这是仿 VS Code / Chrome 的一致写法，不需要新增语义色。

### 3.2 实现要点（已核实的 egui 0.36.2 API）

- `main.rs`：`ViewportBuilder::default().with_decorations(false)`（`egui/src/viewport.rs:366`）。
- 拖窗：`ViewportCommand::StartDrag`（`viewport.rs:1102`）；绑在自己的 titlebar 背景上，
  `Sense::click_and_drag()`，双击非按钮区则 `ViewportCommand::Maximized(!maximized)`。
- 最大化 / 最小化：`ViewportCommand::Maximized(bool)` / `Minimized(bool)`（`viewport.rs:1130-1137`）；
  图标要不要显示「还原」，读 `ctx.input(|i| i.viewport().maximized)`（`data/input/viewport_info.rs:72`）。
- 关闭：`ViewportCommand::Close`。

### 3.3 已知风险（写进 [风险登记册](#9-风险) R1）

1. **无装饰后失去系统 resize 手柄。** 补救：在内容四周留 6px 命中区，8 个方向各自发
   `ViewportCommand::BeginResize(ResizeDirection)`（`viewport.rs:1123`）。
   `egui-winit` 已实现二者（`egui-winit/src/lib.rs:1750/1784`），且 X11 的 `StartDrag`
   内部已有 `has_focus()` 保护 —— 这条坑由上游兜住了，不用自己处理。
2. **macOS**：无装饰窗口丢掉红黄绿灯的圆角与阴影观感；`.with_has_shadow(true)` +
   `fullsize_content_view` 是苹果的常见搭配，但要真机调。
3. **Win11**：无装饰窗口的圆角由 DWM 决定，` WindowLevel` 无需动。

---

## 4. ④ 三分栏的 Panel 排布

**改成** `Panel::left("nav")` → `Panel::right(COMPARE_PANEL_ID)` → `CentralPanel`（编辑器）。

> 顺序铁律（AGENTS/adr-005 §3.2）不变：先加的最外层，`CentralPanel` 必须最后。
> 编辑器进 `CentralPanel` 才是「吃掉剩余宽度」的那个 —— 中间栏理当随左右开关伸缩。
>
> **2026-10-15 三态视图后记**：右栏不再是「可独立开关的对照预览面板」，而是三态视图的
> **分栏档**（`render_mode == RenderMode::Split`）—— 三档互斥，右栏的开合就是 `render_mode`
> 本身，没有额外的开合 bool。它渲染的是 **Live 富渲染本身**（`crate::live::ui`），只读呈现。
> 独立的 `ui::preview` 渲染链路已随 live-unify P4/P5 删除（AGENTS.md 铁律 4）。

```rust
// ui/layout.rs :: draw
egui::Panel::top("titlebar").show(ui, |ui| ui::titlebar::ui(ui, ...));   // D1
egui::Panel::top("menubar").show(ui, |ui| ui::menubar::ui(...));         // D2 保留
egui::Panel::bottom("statusbar").show(...)  // 必须在 nav/preview 之前：先画者占满全窗横向（2026-09-27 修复）

egui::Panel::left("nav")
    .resizable(true).default_size(240.0).size_range(180.0..=420.0)
    .show_collapsible(ui, &mut layout.left, |ui| ui::sidebar::ui(...));  // §5

// 只在分栏档出现(且审阅 Git diff 工作区时让位);渲染 live::ui 的只读浏览态
if split && !git_diff_workspace {
    egui::Panel::right(COMPARE_PANEL_ID)
        .resizable(true).show_separator_line(false)
        .size_range(tokens::PREVIEW_MIN_W..=preview_max)   // 左缘分隔线走自绘
        .frame(Frame::default().inner_margin(Margin::symmetric(8, 8))
                               .fill(theme::shell(ui).content).stroke(Stroke::NONE))
        .show(ui, |ui| crate::live::ui(.., &mut tab.live_right, ..));
}

egui::CentralPanel::default()
    .frame(Frame::default().fill(theme::content_fill(ui.visuals().dark_mode)))
    .show(ui, |ui| { ui::tabs::ui(...); ui::format_bar::ui(...); ui::editor::ui(...); });
```

**关键点：`show_collapsible` 直接吃掉 `&mut bool`。** 右上角的「关闭左侧/右侧」只需
`layout.left = !layout.left`，滑动动画、拖边收起、拖回郭把手全部由 egui 内建提供
（`containers/panel.rs:451`），不需要自己写**任何**可见性代码。这是对 ADR-005 §3.3
既定决策的直接复用，不要改成手写 `if visible {}`（`if` 分支会丢掉面板 persisted 宽度与动画）。

`Command::ToggleSidebar` 语义更新为「切换左侧导航」，保留既有 `Ctrl/Cmd+\`
（`Ctrl+B` 留给将来的加粗 —— command.rs 里早有这条注释，本轮正好兑现，见 §6.3）。

---

## 5. ③ 左栏三段式重构（D3）

替换 `ui/sidebar.rs` 的「页签栏 + 页内容」，改为自上而下三段：

| 段 | 高度 | 内容 |
|---|---|---|
| **顶段·动作行** | 固定 ~32px | **文件动作全集**：新建 / 打开 / 保存 / 另存为 / 导出 HTML —— 五个图标按钮（`Command::FILE` + `ExportHtml`；2026-09-27 起是文件动作的**唯一常驻按钮入口**，decisions-pending #32）。横向 `horizontal_wrapped`，窄时换行不裁切 |
| **次段·视图导航** | 固定 ~4 行 | 文件树 / 搜索 / 大纲 / Git —— 竖排列表行，**整行选中态**：`selected_bg` 底 + 左侧 2px `accent` 竖条（照抄 #28 已画的页签选中态）

                                                                                                   点击只发 `Message::SidebarTabChanged(tab)` |
| **中段·视图内容** | 吃掉剩余 | Files=根目录行+懒加载树 / Search=输入+开关+流式结果 / Outline=标题树+当前小节高亮 / Git=改动+diff+历史。全部沿用现有实现，只是包在 `ScrollArea` 里 |
| ~~底段·设置~~ | — | **2026-09-27 撤销**（用户指令，decisions-pending #31）：齿轮挪到标题栏右端，左栏收回三段（顶动作 / 视图导航 / 中段），`NAV_BOTTOM_H` 随之删除 |

**中段吃掉剩余高度的做法**（egui 里沒有 flex-grow）：

```rust
top_actions(ui, ...);
view_nav(ui, ...);
ui.separator();
ScrollArea::vertical()
    .id_salt("nav-body")
    .max_height(ui.available_height())             // 显式吃掉剩余（至左栏底部）
    .show(ui, |ui| match view { /* 四页 */ });
```

不用嵌套 `Panel`（会造成 widget id 与 z-order 意外），也不用 `bottom_up`（左右 snap 会让
网段的阅读顺序与代码顺序相反，后续读代码的人必踩）。

> **可发现性守恒**：原 top menubar 的命令一个不删。2026-09-27 修订（decisions-pending
> #32）：编辑器区顶部的文件工具栏（`ui/toolbar.rs`，ui-polish 批次产物）与左栏顶段
> 重复，**整体退役**；文件动作收口为左栏顶段图标版 + 菜单栏菜单项两个入口。工具栏的
> AI 下拉不补（菜单栏「AI」已有），`ToggleSidebar`/`ToggleTheme` 不补（标题栏 `┃左`、
> 菜单栏「视图」与设置浮窗外观页已有）；`document.notice` 提示行迁编辑器面板顶。

---

## 6. ⑤ 编辑器顶部的 Markdown 格式工具条

### 6.1 动作清单（四组，从左到右）

| 组 | 动作 | 形态 |
|---|---|---|
| 行内 | 加粗 / 斜体 / 删除线 / 行内代码 / 链接 | 加粗=RichText `B`(strong)、斜体=RichText `I`(italic)、删除线=RichText `S`(strikethrough)、代码=自绘 `‹›`、链接=自绘链环 |
| 标题 | H1 / H2 / H3 / 正文 | RichText 文字（`H1`/`H2`/`H3`/`¶`），同上避免字形依赖问题下方说明 |
| 块 | 引用 / 代码块 / 分割线 / 表格 | 自绘线段图标 |
| 列表 | 无序 / 有序 / 任务 | 自绘线段图标 |

> **为什么 B/I/S/H1 用文字而不是自绘图标**：这三个是「形态」抽象概念，自绘只能画出缺乏辨识度的矩形；
> 而 `RichText::strong()/italics()/strikethrough()` 是 egui 内建富文本能力，零字形依赖风险
> （斜体在极小数字体下可能退化为常规字形，可接受 —— 每个按钮都带 tooltip）。
> 其余用自绘线段，遵守 ui-polish §1.1「图标是矢量自绘，不是字体字符」。

**新增 `Icon` 变体**（`ui/icons.rs`，全部线段/矩形自绘）：
`Minimize` `Maximize` `Restore` `Close` `PanelLeft` `Zen`
（无 `PanelRight` —— 右栏开合已并入三态视图的分栏档，独立开关与按钮随 live-unify P4 删除）
`Link` `CodeInline` `Quote` `CodeBlock` `Divider` `Table` `BulletList` `OrderedList` `TaskList`。

### 6.2 新的业务逻辑层 `crates/latermd-app/src/compose.rs`（纯函数 + 单测）

动作必须做成**不依赖 egui 的纯函数**，才测得了：

```rust
pub enum FormatAction { Bold, Italic, Strike, InlineCode, Link,
                        H1, H2, H3, Plain, Quote, CodeBlock, Divider, Table,
                        Bullet, Ordered, Task }

/// text + 选区（字符偏移）→ 新文本 + 新选区（字符偏移）
pub fn apply(action: FormatAction, text: &str, sel: Range<usize>) -> (String, Range<usize>);
```

语义规则（写死，测试钉住）：

| 场景 | 行为 |
|---|---|
| **行内类** + 无选区 | 光标处插入成对标记 `**`/`*` / `~~` / `` ` `` / `[](url)`，新选区=两标记**之间**（光标落在中间，可直接打字） |
| **行内类** + 有选区 | **包裹**：`**选中**`，新选区=包裹后的整段文本（保留选中便于接着点别的格式） |
| **行内类** + 选区已被同标记包裹 | **去包裹**（toggle off）：再次点加粗把 `**x**` 变回 `x` |
| **行前缀类**（H1-H3/引用/列表） | 作用于选区覆盖的**所有整行**。已是该前缀 → 去掉（toggle）；是别的前缀 → **替换**为新的（`- ` 点有序 → `1. `，不当成叠加） |
| **插入类**（代码块/分割线/表格/链接） | 在当前行下方插入块级模板；代码块 info string 空、表格 2×2 骨架 |
| **任务列表** | `- [ ] `；已是 `- [ ] ` → 变 `- [x] ` → 变 `- `（三态循环） |
| **多字节安全** | 全部走**字符**偏移（`EditorBuffer` 已有 `char_to_byte` / `byte_to_char`），索引 rope 时按字符换算 |

### 6.3 快捷键（新增 `Command`，落 `keymap.json` 可改）

| Command id | 默认键 | 备注 |
|---|---|---|
| `format_bold` | `Cmd/Ctrl+B` | command.rs 原本就注释「Ctrl+B 留给将来的加粗」，本轮兑现 |
| `format_italic` | `Cmd/Ctrl+I` | |
| `format_strike` | `Cmd/Ctrl+Shift+X` | |
| `format_inline_code` | `` Cmd/Ctrl+` `` | `Cmd+E` 已被 ExportHtml 占，取 VS Code 同款反引号 |
| `format_link` | `Cmd/Ctrl+K` | |
| `format_h1/h2/h3` | `Cmd/Ctrl+1/2/3` | |
| `format_bullet` | `Cmd/Ctrl+Shift+8` | VS Code 同款 |
| `format_ordered` | `Cmd/Ctrl+Shift+7` | |
| `format_task` | `Cmd/Ctrl+Shift+9` | |
| `format_quote` | `Cmd/Ctrl+Shift+.` | |
| `format_code_block` | `Cmd/Ctrl+Shift+C` | |
| `toggle_live_preview` | `Cmd/Ctrl+/` | 三态循环：源码 → 写作 → 分栏 → 源码（原 `toggle_right_preview` 的 `Cmd/Ctrl+Alt+R` 已随该命令删除） |
| `toggle_zen` | `F11` | `Shortcut::bindable` 允许无修饰的 F1-F12 ✅ |

`format_*` 全部**不绑**工具栏口诀外的 menu 项（工具条 + 快捷键两处入口足够，
菜单栏里重复 12 行会把 menu 撑高）。

### 6.4 选区怎么从 TextEdit 回到 State

**问题**：工具条按钮被点击时编辑器已失焦，而选区活在 `TextEdit` 的持久 widget state 里，
`State::apply` 拿不到。

**解法（沿用既有 pattern，不引入 ctx 依赖）**：
`ui::editor` 每帧把当前 `CCursorRange` 回填到 tab 上的 `TabState::selection: Option<(usize,usize)>`
（字符区间；与既有 `OutlineCursor::byte` 的「UI 每帧回填」同一手法）。

```
ui::format_bar 点击 → Message::FormatRequested(FormatAction)
     → State::apply 读 tabs.current().selection（可能是 None = 无选区，取 (caret, caret)）
     → compose::apply(...) → editor.replace_all / 定点插入 → TabState::pending_selection = Some(new)
     → 下一帧 ui::editor 把 pending_selection 写回 TextEdit 持久 cursor 并还焦
```

`replace_all` 已存在于 `latermd-editor`；定点插入/替换可在 `EditorBuffer` 上加一个
`replace_range(char_range, text)`（复用既有 `remove_chars` + `insert_chars`，一行的事）。
**不用在意 redo 栈**：`TextEdit` 内建 undoer 的快照被打碎会丢 undo —— 这点已知，写进 §9 R3。

---

## 7. ⓪ 禅定模式

```rust
pub struct LayoutState {
    pub left:  bool,
    pub right: bool,
    pub editor_hidden: bool,   // 禅定 = 三个全 false + 预览居中
    pub zen: bool,
    /// 进入禅定前的 snapshot，退出时原样还原
    pre_zen: Option<(bool, bool, bool)>,
}
```

- 进入：存 snapshot → `left=false, right=false, editor_hidden=true` → 预览直接由
  `CentralPanel` 渲染（此时它独占全窗），正文限宽 **720px** 居中（ui-design.md §1.2 的「沉浸」参数）。
- 退出：`Esc` / `F11` / 右上浮出的「退出禅定」按钮 → 从 `pre_zen` 还原三者的组合。
  **必须还原到进入前的状态**，不能一律全开 —— 用户原本关着左侧写，退出禅定却蹦出侧栏是意外行为
  （与 decisions-pending #29「写盘前拒绝」同款哲学：寧可多一次操作，不静默改变用户状态）。
- 持久化：见 §10。

---

## 8. ⑥ 右预览只读 + 可视化编辑的接入点（D5，本轮不实现）

> **2026-10-15 三态视图后记**：本节原写的「`ui/preview.rs` 右预览面板」已不存在。只读对照现在是
> **分栏档右栏**，渲染 `crate::live::ui` 的只读浏览态：每帧置 `browse_only + !browse_enabled +
> browse_locked`（点击/双击/跳转都不进编辑、不抢焦点），**不写回 State**，与禅定浏览态同款手法。
> 它跟随光标滚动**一次**（源码侧光标移动才下滚动目标，消费后即清），之后保留自己的滚动位置。
> `ui::preview` 模块与其整条渲染链路已删（live-unify P4/P5）。

- **只读**已经是现状：vendored `MarkdownLabel` 是纯渲染 widget，无编辑可能。本轮只补一条**显式语义**：
  把「预览只读」写进 `ui/preview.rs` 的模块文档，并把粘贴/输入事件在预览区…不需要额外拦截（控件是无感的）。
- **将来可视化编辑的三个既有接入点**（本轮只登记，不动代码）：
  1. `latermd_md::blocks`（已有）—— 块划分；
  2. vendored `render_token_range` + Token source_span（已有）—— token↔源码定位；
  3. `LinkHandler` 五级扩展点（已有，#11/#12）—— 内联批注、`ai://` 指令都从这里长出来。
- **必须守住的不变量**（写进 `ui/preview.rs` 文档注释）：
  > 可视化编辑与源码/ Live Preview 共享**同一个 `EditorBuffer` 与同一套 undo 语义**，
  > 差别只在 `RenderMode` 的那个标志。任何「预览自己持有一份文本」的写法直接驳回
  > （roadmap 阶段 5 铁律 + AGENTS §6.3）。

---

## 9. 风险

| # | 风险 | 等级 | 缓解 |
|---|---|---|---|
| **R1** | 自绘无边框窗口在 mac/Win/Linux 三家的 resize 手柄、阴影、圆角行为不一（§3.3） | 中 | 四边 6px 命中区 + `BeginResize`；`LATERMD_NATIVE_DECORATIONS=1` 逃生口；真机三项进 [acceptance-checklist.md](acceptance-checklist.md) |
| **R2** | 12 个 `format_*` 快捷键与既有绑定撞键（如 `Cmd+E` 已属导出） | 低 | 已逐条比对现有 13 条默认值；落地时 `Keymap` 的撞键拒绝机制（既有能力）会兜底，且在 §6.3 列出了规避后的键位 |
| **R3** | `compose` 改写文本会打碎 `TextEdit` 内建 undoer 的快照，Ctrl+Z 可能一次回退一整次格式操作（而非逐字） | 中 | 已知并接受（与 AI 流式首次 Ctrl+Z 整段回退同款已知边界，decisions-pending #10 已有先例）；写进 settings→外观的提示，并在单测里钉住「格式后文本正确」而非「undo 粒度」 |
| **R4** | 左栏三段式在 180px 下限宽度下 IO 拥挤（五个图标 + 四行导航 + 底段） | 低 | 顶段 `horizontal_wrapped` 换行；左栏下限从 160 提到 **180**（ADR-005 的 160 是三栏旧下限） |
| **R5** | 禅定模式与 macOS 原生全屏冲突（绿键全屏 vs F11 禅定） | 低 | 禅定不动 `ViewportCommand::Fullscreen`，只是隐藏面板 —— 两者正交，互不影响 |

---

## 10. 状态与持久化

新增 `crates/latermd-app/src/layout.rs`（纯 Rust，`serde` 已有依赖）：

```rust
#[derive(Serialize, Deserialize, Default)]
pub struct LayoutSettings {
    pub left: bool,
    pub right: bool,
    pub zen: bool,
    pub left_view: SidebarTab,   // 上次停在哪一页
}
```

落 **`layout.json`**（与 `settings.json` / `keymap.json` / `ai.json` / `mcp.json` 同级）。
**不塞进 `settings.json`**：后者已承载主题+皮肤选择，再塞面板可见性会让「换主题」和「收面板」
两个无关动作共用一份存档，`select_skin` 那套「文件是唯一事实源」的口径会被稀释
（decisions-pending #24 的教训）。

`Window` 极小宽度不变（`ViewportBuilder::with_min_inner_size([900.0, 600.0])`）：
180 + 编辑器最小可用宽 460 + 260，900 仍成立。

---

## 11. Token 增补（`ui/tokens.rs`）

| token | 值 | 用途 |
|---|---|---|
| `TITLEBAR_H` | 36.0 | 自绘标题栏 |
| `SIDEBAR_MIN_W` | 180.0 | 左栏下限（R4） |
| `PREVIEW_DEFAULT_W` | 420.0 | 分栏档右栏（只读对照列）初始宽 |
| `FORMAT_BAR_H` | 30.0 | 格式工具条 |
| `NAV_ROW_H` | 26.0 | 左栏导航行高 |
| ~~`NAV_BOTTOM_H`~~ | ~~28.0~~ | 已随底段设置行撤销删除（2026-09-27，decisions-pending #31） |
| `ZEN_TEXT_W` | 720.0 | 禅定正文限宽 |
| `WINDOW_BTN` | 32×24 | 标题栏右侧按钮命中区（Win 风整块，mac/Linux 同款收统一） |

> 顺手订正：`ui-polish.md` §2 表里写 `TOOLBAR_H = 32`，`tokens.rs` 实际是 **28**。
> 本轮以代码为准，并把 ui-polish.md 回写成 28（见 §13 待办 T1）。

---

## 12. 落地分期（按 8.5 工作日估）

| 里程碑 | 内容 | 依赖 | 周期 |
|---|---|---|---|
| **M1** 标题栏 + 三栏重排 | `ui/titlebar.rs`（自绘 + 六按钮 + 拖窗 + 边缘 resize 命中区）、`layout.rs` 改 nav/right/central 三 panel、`LayoutState` 骨架 | D1 | 1.5d |
| **M2** 左栏三段式 | `ui/sidebar.rs` 重写：顶动作 / 视图导航 / ScrollArea 中段 / 底设置；新增 6 个 `Icon` | M1 | 1.5d |
| **M3** 格式工具条 | `compose.rs`（纯函数 + 12 用例单测）、`ui/format_bar.rs`、12 个 `Command` + 键位、`EditorBuffer::replace_range`、`TabState::selection/pending_selection` 回填链路 | D3 | 3d |
| **M4** 禅定 | 进出 `pre_zen` 快照、限宽 720 居中、退出三入口 | M1 | 1d |
| **M5** 收口 | 六项门禁 + 像素验收截图 + 期 `layout.json` 持久化 + 文档回写 | 全部 | 1.5d |

**落地记录（2026-09-27，M5 收口棒）**：M1–M4 合入 main 后的第二批用户反馈两项已处理——
①文件工具栏退役、文件动作收口左栏图标版（§5 / decisions-pending #32）；
②任务列表 CJK 崩溃：根因不在 `compose.rs`（字符口径全程安全、不可 panic），而是格式归约
整篇替换文本后，状态栏（绘制序先于编辑器）同帧拿按**旧文本**折出的 `cursor.byte` 切
**新文本**，`byte index not a char boundary` panic。修复 = `cursor_position` 收缩到字符
边界 + `apply_format` 把失效字节重折算；全链路回归（真实 Task 按钮 ×7 连点 + 归约 +
状态栏）与 compose 层三情形（空选区 / 跨多行 CJK / 行中光标）测试钉住。

**每个里程碑出口都要跑**（AGENTS §8 的既有约定，CI 只在 main 跑）：
`cargo fmt --all --check`、三轮 `cargo clippy --workspace --all-targets -D warnings`
（default / `--no-default-features` / `--all-features`）、`cargo test --workspace --all-features`、
`cargo doc --no-deps --all-features`。

**验收（可勾选，逐条）**：

- [ ] 三个开关键位各自翻转对应面板；同时关左右 → 只剩编辑器（第四种形态可用）
- [ ] 禅定进入/退出后，三个面板的组合与进入前**逐项一致**
- [ ] `compose::apply` 12 组语义全覆盖（含 CJK 多字节、空选区、toggle off、跨行前缀）
- [ ] 左栏 180px 下限下无裁切、换行正常
- [ ] 标题栏：拖动可移动窗口；双击标题区最大化/还原；六按钮 hover/按下两态齐全
- [x] 明/暗两套主题下像素采样验收（照 #28 的做法：`import` 截图 + 采样 RGB;2026-09-27 完成,证据见 [m5-acceptance.md](m5-acceptance.md)）
- [ ] 触摸屏/高分屏无回归（本机 1.0 ppi 无法验，进人工清单）

---

## 13. 配套待办（本轮捎带）

| # | 待办 | 落点 |
|---|---|---|
| T1 | `ui-polish.md` §2 的 `TOOLBAR_H` 32 → 28（与代码对齐） | docs |
| T2 | `adr-005` §3.2 的「`Panel::left` × 2 + `CentralPanel`」补一句：右上角（现在是右侧 `Panel::right` + 中间 `CentralPanel` = 编辑器）的具体分工 | docs |
| T3 | `acceptance-checklist.md` 增三条真机项：Win/mac/Linux 的无边框拖窗与边缘 resize | docs |
| T4 | roadmap「当前位置」加一行：2026-09-26 外壳重构（auto-plan #13） | docs |

---

## 14. 明确不做

- **True WYSIWYG**（AGENTS §7 已否决，§8 的三个接入点服务的是 Live Preview v2，不是 Word 形态）
- 左侧再劈一条 activity bar（D3 已否）
- 标签拖拽排序、面板自由停靠/浮出（超出 `Panel` 能力，需 docking 库）
- 「禅定」里隐藏标题栏 + 自动淡出（D4 留 v2）
- 皮肤市场 / 外壳随皮肤换色（批次 C，屡次重申不做）
