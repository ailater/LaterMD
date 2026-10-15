# ADR-005: 三栏布局与侧边栏

日期: 2026-09-24
状态: 已接受
关联: [[adr-001-gui-and-architecture]]、[[adr-002-platform-renderer-wysiwyg]]、[[adr-003-renderer-and-ecosystem-audit]]

> 本 ADR 全部 API 结论均由 egui 0.36.2 源码核对。**用户初版方案中有两处 API 在 0.36.2 已不存在**，已在 §2 标出。

---

## 1. 决策摘要

| 议题 | 结论 |
|---|---|
| 布局实现 | **`Panel::left` × 2 + `CentralPanel`**，不用手写 split |
| `SidePanel` | ❌ **egui 0.36.2 已移除**，改用 `Panel`（0.34.0 弃用，0.35.0 删除） |
| `App::update` | ❌ **已移除**，改为 `logic` / `ui` 二分 |
| 侧边栏折叠 | 用 **`Panel::show_collapsible`**（自带滑动动画 + 拖拽把手），不用手写 `bool` |
| 大纲（Outline） | **前移到 P0**（廉价版），P3 再补预览滚动跳转 |
| `source_span` | 在 **vendor 阶段一次性加入**，同时服务大纲与 Live Preview |

---

## 2. 必须纠正的 API（初版方案中的写法会编译失败）

### 2.1 `egui::SidePanel` 不存在

全仓库搜索 egui 0.36.2 的所有 `.rs` 文件：

```
TOTAL SidePanel in repo: 0
```

时间线（根 `CHANGELOG.md` 第 273 行，**0.34.0 - 2026-03-26**）：

> ### Changed panel API
> `SidePanel` and `TopBottomPanel` are deprecated, replaced by a single `Panel`.

0.34.3 中残留的最后形态：

```rust
#[deprecated = "Use Panel::left or Panel::right instead"]
pub type SidePanel = super::Panel;
```

到 0.35.0 连这个别名也没了（实测各版本计数：0.34.3 = 1，0.35.0 = 0）。

### 2.2 尺寸 API 改名

| 初版写法（0.33 时代） | 0.36.2 实际 |
|---|---|
| `.default_width(f32)` | **`.default_size(f32)`** |
| `.width_range(a..=b)` | **`.size_range(impl Into<Rangef>)`** |
| `SidePanel::left(...)` | **`Panel::left(...)`** |

`Panel` 是**方向无关**的统一 API —— `default_size` 对纵向面板是宽度、对横向面板是高度，这正是本次合并的目的。

### 2.3 `App::update` 不存在

`crates/eframe/src/epi.rs`（0.36.2）定义的是：

```rust
fn logic(&mut self, ctx: &egui::Context, frame: &mut Frame) { ... }
fn ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame);
```

原文（PR #7775）：

> You may **NOT** show any ui or do any painting during the call to `Self::logic`.
> While the window is hidden, `eframe` runs no egui pass at all … and calls this via `Context::run_logic` instead.

**这对状态管理有直接影响**：你的 reducer 必须放在 `logic` 里，且严格不能碰 UI。`ui` 只负责渲染。

> `App::ui` 拿到的是 **`&mut Ui`** 而非 `&Context`，所以整个三栏骨架都活在**同一个根 `Ui`** 内部。

### 2.4 仍然可用的（别误改）

| API | 状态 |
|---|---|
| `CentralPanel` | ✅ **未弃用**。只有 `show_inside` 重命名为 `show` |
| `Frame::central_panel(style)` | ✅ 存在（`containers/frame.rs:191`） |
| `CentralPanel` 必须最后添加 | ✅ 仍成立，见 §3.1 |

---

## 3. 布局实现

### 3.1 顺序铁律

`containers/panel.rs` 模块头原文：

> The order in which you add panels matter! The first panel you add will always be the **outermost**, and the last you add will always be the **innermost**.
>
> You must never open one top-level panel from within another panel. Add one panel, then the next.
>
> ⚠ Always add any `CentralPanel` last.

### 3.2 最终代码

> **2026-09-27 修订（外壳重构落地后的 panel 分工；形态与论证见
> [ui-shell-redesign.md](ui-shell-redesign.md) §1 布局图与 §4，此处只记结论）**：
> 下方代码块是 2026-09-24 初版（编辑器为第二个 `Panel::left`、预览占
> `CentralPanel`）。外壳重构后左右关系对调、编辑器回到中央，现行顺序
> （`crates/latermd-app/src/ui/layout.rs` `draw`）：
>
> | 序 | panel | 内容 | 理由（出处见规格） |
> |---|---|---|---|
> | 1 | `Panel::top("titlebar")` 36px | 自绘标题栏：文档名 + 右端六钮（┃左 / ⦿禅定 / ⚙设置 / ─ / ⤢ / ✕） | D1 无边框自绘；仅默认无边框模式画，`LATERMD_NATIVE_DECORATIONS=1` 时整条不渲染。**2026-10-15**：┃右 钮随右栏独立开关删除（live-unify P4），视图切换改由标题栏三段开关（写作/源码/分栏）+ 左端搜索胶囊承担 |
> | 2 | `Panel::top("menubar")` | 菜单栏，全部命令的可发现性入口 | D2 保留独立菜单栏行 |
> | 3 | `Panel::bottom("statusbar")` | 状态栏，**横跨全窗底部** | 必须在 nav/preview **之前**画：先画者占满全窗横向，后画只占中央残余区（`d797451` 修复，decisions-pending #31） |
> | 4 | `Panel::left("nav")`（180..=400，`show_collapsible`） | 左栏三段式：顶动作行 / 视图导航 / ScrollArea 中段 | D3 单栏三段式；下限 160→180（R4） |
> | 5 | `Panel::right(COMPARE_PANEL_ID)`（`PREVIEW_MIN_W..=preview_max`）——**仅分栏档出现** | 只读对照列，渲染 `live::ui` 的只读浏览态（同一份文档的 Live 富渲染） | `Panel::right` 必须先于 `CentralPanel`（先加的最外层）。**2026-10-15**：不再是带独立开合 bool 的「对照预览面板」，开合即 `render_mode == Split`；独立渲染链路 `ui::preview` 已删（live-unify P4/P5，AGENTS.md 铁律 4） |
> | 6 | `CentralPanel` = 编辑器 | 标签条 / 提示行 / 格式工具条 / 源码编辑区 | 顺序铁律仍成立：`CentralPanel` 最后加，编辑器吃剩余宽度，左右开合只让中间伸缩。编辑器曾改用 `Panel::left("editor")`，致中央残余区无人认领出现第四条黑条，`d797451` 回归 `CentralPanel` |
> | 7 | 浮层（`egui::Window`）与 `edge_resize_zones` | commit 建议 / 设置 / 回滚 / 关标签确认浮窗；四边四角 resize 命中区 | Area 层不参与 panel 嵌套；命中区必须**最后分配**（同层命中后分配者胜，见 `ui/titlebar.rs` 模块文档） |
>
> 禅定模式是**另一套 panel 组合**（titlebar + 限宽 720 的 `CentralPanel`），不是给
> 三栏各加可见性开关（规格 §7）。§3.3 的 `show_collapsible` 结论不变，直接复用。
>
> **2026-09-28 补记（源码编辑区的滚动与光标跟随 —— UI 关注点口径，auto-plan #29 落地，
> `fe3ee00`+`49634d6`）**：
> - 表中第 6 行「源码编辑区」内部现为 `egui::ScrollArea::vertical()
>   .auto_shrink([false, false])`（id 挂 editor_id，**每标签一套滚动位置**）包住
>   multiline `TextEdit`。egui 0.36.2 的 multiline TextEdit **无内建滚动**（内部无
>   ScrollArea，widget 随内容长高，builder.rs 自述「always show everything in
>   multiline」），`desired_rows` 是**下限**而非上限——没有外层 ScrollArea 时超出面板
>   的内容被 CentralPanel 裁掉（a3f760f 起即如此，#29 补上）。`desired_rows` 取视口
>   行数，空文档铺满、长文档自然长高不被压扁。
> - **光标跟随是 UI 关注点，app 侧自管**：实测 0.36.2 的 TextEdit 内建跟随
>   `scroll_to_rect` 发生在 **Atom paint 阶段**，晚于外层 `ScrollArea::end` 的消费点，
>   请求永不落地（Ctrl+End 后光标到文末但 `offset_target=None` 实证）。因此在
>   ScrollArea 闭包内自管：仅 ① `format_result`/`jump_to` **写回帧**（取写回目标，
>   不等下一帧光标状态）与 ② 键盘导航键（ArrowUp/Down/Home/End/PageUp/PageDown）
>   **且编辑器持焦点**时，对该帧 galley 光标行 rect（含折行）调 `scroll_to_rect`，
>   标志帧内即焚——滚轮/拖滚动条/空闲帧一概不触发，**不抢用户徒手滚动**（首版
>   「每帧 diff 光标」方案曾引入「滚动后自动滚回鼠标位置」回归，`49634d6` 整段重写）。
> - **状态归属（呼应 §2.3 logic/ui 二分）**：滚动偏移留 ScrollArea 内建 state（id
>   每标签一套），跟随标志是闭包内帧局部变量、无跨帧存储，均**不进 State 归约**。
>   Live 模式（live.rs 自有 ScrollArea `"live-preview"`）不共用此路径；其活动块打字
>   跟随、pending_caret 跨块路由与大纲 jump_to 不被消费（留存到切回 Source 才应用）
>   存在同款时序缺口，为既有现状，留后续任务（见 auto-plan #29 收官记录）。

```rust
// 初版记录（2026-09-24；已被上方 2026-09-27 修订取代，保留作历史对照）。
// crates/latermd-app/src/ui/layout.rs
use eframe::egui;

impl eframe::App for LaterMdApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 只做状态归约：消费 message channel、跑 Git/搜索后台任务结果。
        // 严格禁止在此绘制任何 UI。
        self.reduce_messages(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // ① 最外层：侧边栏（可折叠）
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(240.0)
            .size_range(160.0..=400.0)
            .show_collapsible(ui, &mut self.state.sidebar.visible, |ui| {
                self.sidebar_ui(ui);
            });

        // ② 次外层：编辑器
        egui::Panel::left("editor")
            .resizable(true)
            .default_size(500.0)
            .show(ui, |ui| {
                self.editor_ui(ui);
            });

        // ③ 必须最后：预览
        egui::CentralPanel::default().show(ui, |ui| {
            self.preview_ui(ui);
        });
    }
}
```

### 3.3 为什么用 `show_collapsible` 而非手写 `bool`

初版方案写的是「用一个 `bool` 状态控制是否 `show`」。可行，但等于重写 egui 已提供的东西：

```rust
pub fn show_collapsible<R>(
    self, ui: &mut Ui, is_expanded: &mut bool,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> Option<InnerResponse<R>>
```

源码注释里最关键的一句：

> During the animation `add_contents` runs against the real panel, and the panel slides off-screen toward its fixed edge (clipped against the parent). **The parent only reserves the _visible_ portion, so neighboring widgets follow.**

手写 `if visible { panel.show(...) }` 的结果是：**切换时 editor 面板会瞬间跳位**。用 `show_collapsible` 则是平滑滑动且相邻面板跟随。

附带行为（均来自源码）：

| 行为 | 说明 |
|---|---|
| 完全折叠保留细把手 | 用户可从边缘拖回来 |
| `.drag_to_open(false)` 可关闭该把手 | — |
| `.resizable(true)` 时双击调整边 | 翻转 `*is_expanded` |
| 拖动越过最小尺寸 | 自动翻转 `*is_expanded` |
| 返回值 `Option<...>` | 完全关闭时为 `None` |

---

## 4. 侧边栏三个 Tab

### 4.1 文件树

**依赖**（已核对版本）：

| crate | 版本 | 说明 |
|---|---|---|
| `ignore` | **0.4.33** (2026-08-04) | ripgrep 的目录遍历库，天然支持 `.gitignore` |
| `notify` | 8.2.0 | 外部变更监听（ADR-004 已定） |

关键设计（沿用初版方案，这些判断是对的）：

- **懒加载**：只在目录展开时才遍历子项
- **`.gitignore` 支持**：`ignore::WalkBuilder` 天然支持，否则 `node_modules` / `target` 会拖垮 UI
- **渲染用 `ui.indent` + `ui.selectable_label`**，不用 `CollapsingHeader` —— 后者在动态目录下的内部 ID 管理不好控制
- **右键菜单**：`response.context_menu()` 做新建/重命名/删除/在文件管理器打开

**性能边界**：大仓库（10 万+ 文件）下首次遍历仍可能卡顿。初版提出的两条路（异步遍历+增量更新 / 限制深度+只显示 md）**都保留，先做后者**（可配置的过滤器是 10 行代码，异步遍历是一个子系统）。

### 4.2 搜索

依赖版本核对结果，有一处要改：

| crate | 版本 | 最后更新 | 建议 |
|---|---|---|---|
| `grep-searcher` | 0.1.17 | 2026-07-15 | ✅ 用 |
| `regex` | 1.13.1 | 2026-07-15 | ✅ 用 |
| `grep-regex` | 0.1.14 | **2025-10-16** | ⚠️ **近一年未更新，建议不用** |

**建议组合：`ignore` + `grep-searcher` + `regex`**，跳过 `grep-regex`。它是三者里 rust 味最重的维护负担，而我们要的只是「按行取出匹配 + 记录 `Range<usize>`」，`grep-searcher` 本身已够。

其余设计沿用初版，均成立：

- **300ms 防抖**
- **可取消**：新搜索取消旧搜索（`tokio::select!` 或 `CancellationToken`）
- **流式结果**：通过 channel 逐条推送而非等全部完成
- **高亮**：匹配词用 `LayoutJob` + 黄色背景

> ⚠️ 初版方案里的 `Task::perform` 是 **iced 的术语**，egui 中没有。实际用 `tokio::spawn` + `ui.ctx().request_repaint()`。

### 4.3 大纲（Outline）—— 这是本 ADR 最大的排期改动

**数据来源核实**：初版说「用 `into_offset_iter()` 拿偏移」—— **正确，API 存在**：

```
Parser::into_offset_iter() -> OffsetIter<'a, F>
OffsetIter::Item = (Event<'a>, Range<usize>)
```

所以**提取侧 genuinely 很容易**。真正的难点在「跳转」，而这里有三个已核实的事实决定了工作量：

| # | 事实 | 出处 |
|---|---|---|
| 1 | `Token` 无源码 span | ADR-003 §5.4 已核实 |
| 2 | `layout_in_ui` 返回 `(Pos2, Arc<Galley>, Response)`，**无「跳到第 N 字节」入口** | `label.rs` |
| 3 | **但已有 `section_to_token: Vec<usize>` 映射 + `section_for_char()` 反查** | `layout.rs`，ADR-003 §5.2 |

第 3 条容易被漏掉，**它能把工作量减半** —— 不需要从零建 byte→galley 位置的索引，现有机制已经把 galley section 映射到了 token。加 source span 后再补一个 token→char offset 累加器就够了。

#### 排期决策：大纲前移到 P0

初版把整个大纲放 P3。我建议**拆开**：

| 阶段 | 范围 | 成本 |
|---|---|---|
| **P0 廉价大纲** | 标题列表 + 点击 → **跳编辑器光标**（`TextEdit` 的 `cursor_range`）。预览完全不滚动。 | **约 3 天** |
| **P3 升级** | 加预览滚动跳转，复用现有 `section_to_token` 映射 | 增量小 |

理由：大纲通过「写下一篇技术文档」测试的程度高于搜索 —— 它是导航手段，不是检索手段。而 3 天的廉价版本不需要任何新子系统。

#### `source_span` 必须早做且只做一次

它同时服务两个消费者：

```
                ┌─→ 大纲跳转（P0 廉价版就需要）
来源 span 补充 ─┤
                └─→ Live Preview（P3）
```

**建议：在 vendor 升级那一次就把 `source_span` 加到 `Token` 上** —— 那本来就是唯一会碰 `Token` 定义的 pass。否则你要打两次补丁，第二次还得重新面对已经遗忘的 24 个升级错误上下文。

---

## 5. 状态管理

### 5.1 结构（沿用初版，补充 `logic`/`ui` 的分工）

```rust
struct State {
    // 原有
    document: DocumentModel,

    // 新增
    sidebar: SidebarState,
    file_tree: FileTreeState,
    search: SearchState,
    outline: Vec<OutlineItem>,
}

struct SidebarState {
    visible: bool,              // 直接喂给 show_collapsible 的 &mut bool
    active_tab: SidebarTab,
}

enum SidebarTab { Files, Search, Outline }

enum Message {
    SidebarTabChanged(SidebarTab),
    FileSelected(PathBuf),
    FileTreeToggled(PathBuf),
    SearchQueryChanged(String),
    SearchCompleted(Vec<SearchHit>),
    SearchResultClicked(PathBuf, usize),
    OutlineItemClicked(Range<usize>),
    GitStatusRefreshed(GitStatusMap),
}
```

> 注意：不再需要 `SidebarToggled` —— `show_collapsible` 直接通过 `&mut bool` 修改。

### 5.2 并发模型：Channel 而非共享锁

初版说「Git 操作全部异步」，方向对，但要明确**通信姿势**：

```
UI 线程                              后台 tokio task
   │                                        │
   ├─ cmd_tx.send(GitCommand::Fetch) ──────→│
   │                                        ├─ 执行 git2 操作
   │←──────── result_rx.try_recv() ────────┤
   │                                        ├─ ctx.request_repaint()
   ├─ 下一帧绘制 ───────────────────────────┘
```

**关键约束**（来自 `App::logic` 文档）：不能在后台线程直接改 UI 读的状态 —— egui 是立即模式，期望状态读取极快。

已有依赖够用，**不需要引入新 crate**：

| 已有 | 用途 |
|---|---|
| `tokio` 1.53.1 | spawn + `mpsc` + `select!` |
| `crossbeam-channel` 0.5.17 | 若偏好同步 channel |
| `egui-async` 0.6.0 | 可选，专为 egui 封装（15,606 下载） |

### 5.3 Git 状态与文件树联动

`git2::Repository::statuses()` 结果缓存到 `GitStatusMap`，文件树渲染时查表。刷新时机：文件保存 / commit / `notify` 触发的外部变更。

---

## 6. 修订后的排期

| 阶段 | 侧边栏内容 | 增量 |
|---|---|---|
| **Vendor 适配** | — | 3-5 工作日（**此处同步补 `Token::source_span`**） |
| **M0 验证** | — | 2 周 |
| **P0 骨架** | 三栏布局 + **廉价大纲（3 天）** + 文件树基础版 | +2 周 |
| **P1 差异化** | 搜索（`ignore` + `grep-searcher` + `regex`） | +1.5 周 |
| **P2 版本层** | 文件树 Git 状态标记 M/A/U/? | +0.5 周 |
| **P3 深水区** | 大纲预览跳转（复用 `section_to_token`） | 增量小 |

总计 P0 从 6-8 周 → **8-10 周**（+2 周：布局调通 0.5 周 + 文件树 1.5 周 + 大纲廉价版 0.5 周，有重叠）。

---

## 7. 设计原则（新增）

11. **文件树懒加载 + `.gitignore`**：`ignore` 是必需项不是可选项。大仓库一次性遍历会卡死 UI。
12. **搜索防抖 + 可取消 + 流式**：300ms 防抖；新搜索取消旧搜索；结果逐条推送。
13. **`source_span` 早做且只做一次**：在 vendor pass 中一并加入，同时服务大纲与 Live Preview。
14. **大纲跳转复用现有 `section_to_token`**：不要在单 Galley 里另建索引。
15. **`source_span` 来源**：`Parser::into_offset_iter()` → `Item = (Event, Range<usize>)`。
16. **用 `Panel` 不用 `SidePanel`**：后者在 egui 0.36.2 已不存在。
17. **折叠用 `show_collapsible`**：自带滑动动画与相邻面板跟随，手写 `bool` 会导致跳位。
18. **`logic` 禁止绘制**：egui 0.36.2 的 `App` trait 明确要求。
19. **不用 `grep-regex`**：近一年未更新（2025-10-16），`grep-searcher` + `regex` 已够。

---

## 8. 行动项

- [ ] vendor pass 时同步加 `Token::source_span`
- [ ] 全局替换 `SidePanel` → `Panel`、`default_width` → `default_size`、`width_range` → `size_range`
- [ ] `App::update` → `logic` / `ui` 二分
- [ ] 搜索依赖跳过 `grep-regex`
- [ ] 大纲 P0 廉价版（跳编辑器光标）先于搜索实现
- [ ] 建立 `tokio::mpsc` + `request_repaint()` 通道骨架
