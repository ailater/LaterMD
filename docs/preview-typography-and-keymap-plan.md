# 预览排版与键位调整规划（2026-09-28）

> 坤哥 2026-09-28 提的三件事：① 预览行高重叠；② UI 更协调好看；③ 主题快捷键让出 `Ctrl+Shift+T`。
> **本文是规划，不是完工记录。** 已完成的部分在文末「已完成」一节，其余按 §5 的执行顺序待办。

---

## 1. 已完成：预览行高（根因 + 修复）

### 1.1 根因

vendored 层把**所有行**的行高写死成一个像素常量：

```rust
// vendor/egui_markdown/src/layout.rs（修改前）
pub(crate) const MARKDOWN_LINE_HEIGHT_POINTS: Option<f32> = Some(17.0);
```

`text_format()` 把它塞进每个 `TextFormat`，而 `Token::Text` 处理标题时**只放大字号、不重算行高**：

```rust
format.font_id.size *= style_ref.heading.scales[idx];   // 20.8pt
// 行高仍是上面那个 17.0 —— 没有跟着改
```

实测亏空（`rowprobe` 探针读数）：

| 级别 | 字号 | 自然行高 | 写死行高 | 亏空 |
|---|---|---|---|---|
| 正文 | 13.0 | 15.0 | 17.0 | +2.0（余） |
| H1 | 20.8 | 23.9 | 17.0 | **−6.9** |
| H2 | 17.6 | 20.2 | 17.0 | **−3.2** |
| H3 | 15.6 | 17.9 | 17.0 | **−0.9** |

标题行拿到比字形所需的更矮的行框，换行后就互相压——这就是截图里的重叠。
正文反倒是松的（17.0 对 15.0），所以观感是「正文散、标题挤」。

### 1.2 改法

行高改成**字号 × 倍率**，并把倍率提成 `MarkdownStyle` 字段让用户可调：

| 位置 | 改动 |
|---|---|
| `egui_markdown_style/src/style.rs` | 新增 `line_height_ratio: f32`（默认 `1.30`，serde 兜底）、进 `Hash`、进样式编辑器 UI |
| `vendor/egui_markdown/src/layout.rs` | 删 `MARKDOWN_LINE_HEIGHT_POINTS`；新增 `line_height_for(size, style) -> Option<f32>`；`text_format()` 加 `style` 实参；标题分支在放大字号后**重算** `format.line_height` |

效果（同一探针）：

| 级别 | 字号 | 旧行高 | 新行高 | 差额 |
|---|---|---|---|---|
| 正文 | 13.0 | 17.0 | 16.9 | −0.1（几乎不动，观感无缝） |
| H1 | 20.8 | 17.0 | 27.0 | +10.0 |
| H2 | 17.6 | 17.0 | 22.8 | +5.8 |
| H3 | 15.6 | 17.0 | 20.3 | +3.3 |
| H4 | 14.3 | 17.0 | 18.6 | +1.6 |

`1.30` 这个值不是拍的：`13 × 1.30 = 16.9 ≈ 旧值 17.0`，**正文观感不变**、只有标题被解开。验收时这是第一条要看的：正文别跑版。

### 1.3 分类与门禁

按 AGENTS §6 三类拆分，属 **①上游可合**（对任何使用者通用，纯能力改进）：

- 一个 setter 都不删给别人用；把「写死像素」换成「字号 × 倍率」并暴露为样式字段，是典型的通用改进。
- 回 Lewin 提 PR 时的独立抓手：`git diff <基点> -- vendor/egui_markdown > /tmp/lh.patch` → fork 里 `git apply -p2`。

已跑过的门禁：

- vendor `check.sh` 六项：**fmt ✅ / clippy×3 ✅ / test 106 passed ✅ / doc ✅**
- 主仓全量：`cargo test --workspace --all-features` **597 passed, 0 failed**

---

## 2. 待办 A：行距节奏 + 标题区分（UI 协调性）—— ✅ 已落地（2026-10-02，auto-plan #23 F4/F5）

§1 解开的是**重叠**（bug）。让预览「好看」是另一件事，层次要靠两条一起调：**字号分级** + **块间距**。

> **落地状态（2026-10-02）**：本节三项已全部随 auto-plan #23 落地——（1）（2）为 vendored **①类（上游可合）** commit `02b33ad`（F4，独立 `vendor:` commit），（3）的用户设置页滑杆为 #23 F2/F3（`e5a7203`/`3e55b39`）；`theme_presets.rs` 同步与无头像素验收为 `f138140`（F5），验收记录见 [preview-typography-acceptance.md](preview-typography-acceptance.md)。逐项对照见 §2.2 各小节旁注；§2.3 的自动侧验收（数值/几何断言）已过，**真机目视 M1–M6 留人工**（验收文档 §5）。与计划原文的两处偏差以现状为准：① spacer 落点在 `build_layout` 层而非下文写的 `render_token_range`（纯正文/代码块文档走整篇 galley 路径不经过该函数，岔路全文见 decisions-pending #74）；② 出厂 `line_height_ratio=1.30` 未动，但 app 侧生效值被用户行距滑杆覆盖（默认 1.5，优先级：滑杆 > 皮肤 > overrides > 出厂）。

### 2.1 现状问题

`HeadingStyle::scales` 默认是 `[1.6, 1.35, 1.2, 1.1, 1.05, 1.0]`。以 13pt 正文计：

| | H1 | H2 | H3 | H4 | H5 | H6 |
|---|---|---|---|---|---|---|
| 字号 | 20.8 | 17.6 | 15.6 | 14.3 | 13.7 | 13.0 |
| 与正文比 | 1.60 | 1.35 | 1.20 | 1.10 | 1.05 | **1.00** |

问题在后三档：**H4–H6 挤在一起**（14.3 / 13.7 / 13.0），肉眼几乎分不出，H6 与正文完全同号。
加上 `block_spacing = 8.0` 对所有块一视同仁，标题上下没得到比行间距更多的呼吸——视觉上「都是一串字」。

### 2.2 计划改法

**（1）拉开低阶标题的字号分级 —— ✅ 已落地（2026-10-02，vendored ①类 `02b33ad`）**

```
scales: [2.0, 1.55, 1.30, 1.15, 1.08, 1.0]   // 建议值，需真机目视后微调
```

落地值与本节建议值**逐项一致**；注释里的「真机目视后微调」仍成立——微调只需改 `HeadingStyle::scales` 出厂值一行（真机目视项 M1/M4，验收文档 §5）。

| | H1 | H2 | H3 | H4 | H5 | H6 |
|---|---|---|---|---|---|---|
| 现 | 20.8 | 17.6 | 15.6 | 14.3 | 13.7 | 13.0 |
| 改后 | 26.0 | 20.2 | 16.9 | 15.0 | 14.0 | 13.0 |

H1 从 1.6 拉到 2.0 是中文排版常用档（英文正文常用 1.8–2.0，中文因字形密度建议偏上限）。

**（2）块间距分级：标题前后比正文多留 —— ✅ 已落地（2026-10-02，vendored ①类 `02b33ad` + app `f138140`）**

现在 `before_block` / `after_block` 一律 `ui.add_space(style.block_spacing)`（`label.rs:745-761`）。标题应该比这个更宽。

`MarkdownStyle` 加一个字段，例如：

```rust
/// Extra space above a heading, added on top of `block_spacing`. Default: `4.0`.
pub heading_space_above: f32,
```

> **落地偏差（以现状为准，decisions-pending #74）**：字段与默认值照计划落地，但实现不在 `render_token_range` 里 `add_space`，而是在 `build_layout` 层给标题块首 token 前插**透明 spacer 行**（行高 `block_spacing + heading_space_above`，数值语义同为「叠在空行之上」）——纯正文/普通代码块文档走整篇 galley 路径根本不经过 `render_token_range`，且 `add_space(12px)` 是替换空行而非叠加（12px < 段落间距，反而比段落更紧，违背 §2.3 验收③）；两条渲染路径需一处产出才能一致。`render_token_range` 零改动，正文观感否决线由「纯正文文档 rows 数与高度对新字段完全不变」测试钉死。

`skinsPresets` 同步已做（`f138140`，F5）：`theme_presets.rs` 的 `base()` 显式钉 `heading_space_above=4.0`，与 vendored 默认/出厂默认做**三方交叉断言**（vendored 默认再变时测试红，强制显式同步）；不按皮肤微调——九套预设只差颜色，标题节奏是全局一致属性。

**（3）顺手校正的行距 —— ✅ 已落地（2026-10-02，#23 F2 `e5a7203` / F3 `3e55b39`）：** `line_height_ratio = 1.30` 已在 §1 落地，中文可读区间通常 1.5–1.8。
**但改正文倍率会让正文跑版**（17.0 → 19.5+，一屏少看两三行），属于偏好不是 bug —— 建议留成用户在设置页自己调，出厂值维持 1.30，等坤哥目视后再定要不要出厂就调松。

> **落地状态**：本节「留成用户在设置页自己调」的建议已原样兑现——#23 F2 在外观页交付行距滑杆（1.2–2.0、步进 0.1、默认 1.5，即时生效即时落盘 settings.json），F3 把滑杆值覆盖到 markdown style 生效值（vendored 出厂 1.30 未动，见本节开头状态段）。注意默认出厂观感因此变为 15pt×1.5（#23 规格默认值），vendored 基线仍 1.30。源码编辑器侧的行距投影不在 #23 范围（decisions-pending #73，随 #50 转正）。

### 2.3 验收 —— 自动侧已过（2026-10-02），真机目视留人工

- 一篇含 H1–H6 + 列表 + 引用 + 表格 + 代码块的样例文档，明暗两套主题各截一张。
- 逐条看：① 换行标题不再压字；② H4–H6 能分辨层级；③ 标题上下呼吸明显大于行间距；④ 正文密度不变（这是否决线）。
- 对比基准＝当前 build 的同文档截图，`compare -metric AE` 只用于确认「确实变了」，真正判好看得靠眼睛。

> **落地状态（2026-10-02，`f138140`）**：①④ 与 ③ 的数值/几何断言已由**无头像素验收**覆盖（[preview-typography-acceptance.md](preview-typography-acceptance.md)：生产生效链路 15pt/用户行距 1.5/CJK 行高下限下，标题上方空隙 35.00px − 段落间 23.00px = **恰 12.00px = spacer**；section 字号 30.00/23.25/19.50/17.25/16.20/15.00pt 精断；**纯正文文档 rows/高度对 `heading_space_above` 全不变**（否决线④）；② 有像素带高度分离新旧分级的下限断言）。全元素样例文档明暗两主题的结构带（22 文本带/6 strong 带/引用竖条/表格横边框 6 条/代码底色 694×45px）全部像素层可检出。「真正判好看得靠眼睛」的部分即验收文档 **§5 M1–M6 真机目视清单**，留坤哥人工——眼睛说了算之前不宣称「好看」只宣称「数值如设计」。

---

## 3. 待办 B：快捷键重排

### 3.1 现状

`Command::default_shortcut`（`crates/latermd-app/src/command.rs:249`）现有这张表：

| 命令 | 现键位 | 备注 |
|---|---|---|
| `ToggleTheme` | **Cmd/Ctrl+Shift+T** | ← 要让位 |
| `TabNext` | Cmd/Ctrl+Tab | 与浏览器一致 ✅ |
| `TabClose` | Cmd/Ctrl+W | 与浏览器一致 ✅ |
| `ToggleSidebar` | Cmd/Ctrl+\\ | 与 VS Code 一致 ✅ |
| `ToggleLivePreview` | Cmd/Ctrl+/ | VS Code「切换注释」同键 ✅。**2026-10-15**：三态循环入口（源码 → 写作 → 分栏 → 源码） |
| ~~`ToggleRightPreview`~~ | ~~Cmd/Ctrl+Alt+R~~ | **2026-10-15 已删除**（命令随右栏独立开关退役；右栏改由 `ToggleLivePreview` 循环到分栏档打开）。旧 `keymap.json` 残留该键无副作用 |
| `ToggleZen` | F11 | 与浏览器全屏一致 ✅ |
| `FormatLink` | Cmd/Ctrl+Shift+K | 注：VS Code 是裸 Ctrl+K |

### 3.2 定案（坤哥已选；①②已于 2026-10-01 落地，见 §3.3 对照）

**① 主题 → `Alt+T`（Kun 选的选项 1）—— ✅ 已落地（2026-10-01，commit 780a79c）**

选它的理由：避开 `Cmd/Ctrl+Shift+T`（要让位给恢复标签）与 `Cmd+K` 系列；VS Code 的「颜色主题」也是 Alt 系，习惯相通。
**`Alt+T` 是无 Cmd/Ctrl 的组合**，`Shortcut::bindable()` 要求「至少一个修饰键或是功能键」—— Alt 算修饰键，**满足条件**，不用开后门。

> ⚠️ 实现时要确认 `Modifiers::ALT` + `Key::T` 在三个平台都不被系统吞（Windows 上 Alt+字母常触发菜单栏助记键）。egui 走 winit 的 `received_character` + 物理键，一般拿得到，但**要在 Win/mac 真机上按一次确认**。这是本条唯一的实机验证点。
> **落地状态**：默认键已改排（`command.rs` `default_shortcut` 的 `ToggleTheme` 分支）；旧 `keymap.json` 里值等于旧默认的条目做**值感知迁移**到 Alt+T，用户自定义/主动清除不动（decisions-pending #69）；mac 显示 ⌥（菜单栏 egui 原生 ⌥T，设置页/tooltip `platform_text` 输出 `⌥+`，存档 `⌥` 别名可解析，#69 岔路二）。键位断言 `theme_is_alt_t_and_restore_slot_is_free`（`command.rs` 测试）钉死「Alt+T 不撞任何出厂键位」。**三平台真机按下不被系统吞尚未验证**，见 §3.4 真机项。

**② `Cmd/Ctrl+Shift+T` → 留给 `TabRestore`（恢复刚关闭的标签）—— ✅ 已落地（2026-10-01，commit 881edfd）**

浏览器（Chrome/Edge/Firefox）恢复关闭标签就是这个键，VS Code 同理。**现在是空位**：`default_shortcut` 里没人占 `Cmd+Shift+T`。

**③ 其余键位不动** —— 已经与浏览器/VS Code 对齐了，没必要为改而改。（落地时未动任何其他出厂键位，K1 只改了 `ToggleTheme` 一行。）

### 3.3 TabRestore 的实现要点

命令层一条龙，`Command` 是唯一清单：

| 层 | 位置 | 做什么 |
|---|---|---|
| 枚举 | `Command::TabRestore` | 加进 `ALL`（放 `TabNext`/`TabClose` 之后），同步 `id()` = `"tab_restore"`、`label()` = `"恢复关闭的标签"` |
| 快捷键 | `default_shortcut()` | `Modifiers::COMMAND \| Modifiers::SHIFT, Key::T` |
| 消息 | `Message::TabRestore` | 新增 |
| 归约 | `State::apply` | 从关闭栈弹出最近一条 → 按路径 `open_path()` 重开 |
| 关闭栈 | `TabsState` | 新增 `recently_closed: Vec<PathBuf>` |

几个要想清楚的点：

- **存什么**：只存**已落盘**的路径（`TabState.document.path`）。未保存的新标签没有路径，重开拿不回内容——强行把正文文本也塞进栈会让这个结构变成「半个文档仓库」，超出范围。**未落盘的关了就找不回**，与「重开=重新读盘」的心智一致。
- **栈多长**：浏览器无上限，建议 **封顶 20 条**（`Vec` 超了从头 `remove(0)`），防止长会话无限增长。
- **已在别处打开的重开**：走现有 `open_path()` 语义——同名已开着就**激活它**，不开第二个。这是 `find_by_path` 的既有行为，不冲突。
- **重开失败**（文件被外部删了/移了）：按既有口径写 `document.notice` 提示行，不静默吞，也不 panic。
- **要不要清栈**：重开成功的那条要出栈。**连续按 Cmd+Shift+T 应能逐条往回要走完整条栈**（这才是浏览器的行为）。
- **要不要落盘**（关了应用还能恢复）：**不做**。标签会话不持久化是现状，为了这一个命令破例不值。

参考现状实现：`remove_tab()`（`state.rs:1711`）是唯一的摘除点，在那里面入栈就行，不用到处打补丁。
九套皮肤是 `Vec`，没有 [dead_code]警告吗？看看对了写了个 skin。算了不重要

#### §3.3 落地对照（2026-10-01 核验，逐条对照仓库现状）

表格五行与要点六条的全部落点：

| 要点 | 落点（核验时行号） | 结果 |
|---|---|---|
| 枚举 `Command::TabRestore` 进 `ALL`（`TabNext`/`TabClose` 之后） | `command.rs:52`（枚举）、`:134-136`（`ALL` 序，紧跟 `TabClose`） | ✅ |
| `id()` = `"tab_restore"`、`label()` = `"恢复关闭的标签"` | `command.rs:203`、`:246` | ✅ |
| 快捷键 `Modifiers::COMMAND \| Modifiers::SHIFT, Key::T` | `command.rs:308-310` | ✅ |
| 消息 `Message::TabRestore` | `state.rs:654`；`command.rs:441` 的 `message()` 映射 | ✅ |
| 归约：关闭栈弹出最近一条 → `open_path()` 重开 | `state.rs:868` → `restore_tab`（`state.rs:2368-2374`） | ✅ |
| 关闭栈 `recently_closed: Vec<PathBuf>` | `tabs.rs:217-222` | ✅ |

六个「要想清楚的点」：

- **只存已落盘路径** ✅ —— 唯一入栈点 `TabsState::remove`（`tabs.rs:342`，即上文「`remove_tab()` 是唯一的摘除点」的现状落点；#37 标签管理后摘除函数是 `TabsState::remove`，不再叫 `state.rs:1711` 的 `remove_tab`）里 `document.path.clone()` 命中才 push（`tabs.rs:347-352`），未落盘新标签不入栈。
- **封顶 20 条** ✅ —— `RECENTLY_CLOSED_CAP = 20`（`tabs.rs:182`），超出 `remove(0)` 淘汰最旧（`tabs.rs:349-351`）。
- **已在别处打开 → 激活不开第二个** ✅ —— 走 `open_path()` 的 `find_by_path` 既有语义（`state.rs:2353-2360`），单测 `tab_restore_activates_already_open_tab_instead_of_duplicating` 钉死。
- **重开失败不静默不 panic** ✅ —— 失败落 `document.notice` 提示行（`open_in_tab` 的 Err 分支，`state.rs:2343-2346`）；条目**丢弃并继续下一条**（decisions-pending #70），单测 `tab_restore_failed_path_noticed_dropped_and_continues`。
- **成功出栈、连续触发回走整条栈** ✅ —— `restore_tab` 的 `while let … pop()` 循环（`state.rs:2369-2373`），单测 `tab_restore_reopens_closed_tabs_lifo`（混合已落盘/未落盘）+ 空栈 no-op `tab_restore_empty_stack_is_noop`。
- **不跨会话落盘** ✅ —— 无持久化字段（`tabs.rs:220-221` 注释明示，标签会话本就不落盘）。

联动件：菜单入口在「文件」菜单尾部（`ui/menubar.rs:27`，与 Ctrl+Tab/Ctrl+W 同组）；K1 键位断言 `theme_is_alt_t_and_restore_slot_is_free`、K2 消费断言 `restore_shortcut_fires_only_tab_restore`（`command.rs` 测试，后者用 `poll_shortcuts` 实跑断言 Cmd/Ctrl+Shift+T 只触发 TabRestore 一条）。

### 3.4 验收

- 单测：关 N 个标签后连续 restore N 次，顺序是**后进先出**且最终回到原状。✅ `tab_restore_reopens_closed_tabs_lifo`（混合已落盘/未落盘，`state.rs` 测试）。
- 单测：restore 未落盘标签（无路径）→ 栈里跳过它去取下一条，不 panic。✅ 未落盘标签**不入栈**（`tabs.rs` 只记 `document.path`），混合用例即上面那条；失败路径另有 `tab_restore_failed_path_noticed_dropped_and_continues`、空栈 no-op `tab_restore_empty_stack_is_noop`。
- 真机：`Ctrl+Shift+T` 真按下能被 `poll_shortcuts` 接到（`keymap.rs` 的捕获测试会当它是「任意空闲键」，注意别撞那条回归）。🟨 无头侧已有等价断言 `restore_shortcut_fires_only_tab_restore`（`poll_shortcuts` 收到该键事件只触发 TabRestore 一条）；真窗口三平台按下属真机项，见下。

**真机项清单（留坤哥人工，blocked_external——本模块无法在本机代验）**：

1. **Win11**：Alt+T 是否被菜单栏助记键吞（§3.2 ① 唯一的实机验证点）。若被吞，换键候选与迁移口径按 decisions-pending #69 的「如何改」走（改 `default_shortcut` + 迁移值同步换）。
2. **macOS**：⌥T 行为——切主题是否生效、菜单栏显示 ⌥T、设置「快捷键」页显示 ⌥+T（#69 岔路二的口径）。
3. **三平台**：Cmd/Ctrl+Shift+T 真按下被 `poll_shortcuts` 接住并恢复标签（§3.4 原文提醒的捕获回归：快捷键设置的捕获模式别把它当「任意空闲键」抢走）。

---

## 4. 待办 C：暂时的踝腕アイテム

坤哥没点名，但在改 UI 时会顺带碰到的两个：

- **` ThemeSettings::overrides` 出厂值**：现在是 `None`（全吃 vendored 默认）。要不要出厂就给一份调好的 `overrides`（含新的 `line_height_ratio` / `heading_space_above`），决定权环境问题——出厂给 == 所有人一致 + 用户可调（**, skin + overrides ）；出厂不给 == 全依赖皮肤文件。倾向**给一份**。
- **`theme_presets.rs` 的九套预设**：新增了样式字段后，预设 ron 是**已存在的才不动**（`install_to` 语义），老安装的用户拿不到新字段（serde 走 `default` 兜底，值为 1.30/默认，不炸）。这是可接受行为，但要在 CHANGELOG 里写一句。

---

## 5. 建议执行顺序

| # | 任务 | 交付物 | 门禁 |
|---|---|---|---|
| 1 | ✅ **预览行高修复**（已完成） | vendor 两文件 | vendor check.sh + 主仓 597 |
| 2 | ✅ **键位改排**（已完成 2026-10-01，主题 → Alt+T，Cmd+Shift+T 空出给 TabRestore，commit 780a79c） | `command.rs` / `keymap.rs` | 主仓六项 |
| 3 | ✅ **TabRestore 完整实现**（已完成 2026-10-01，commit 881edfd） | `command.rs` / `state.rs` / `tabs.rs` / `ui/menubar.rs` | 主仓六项 + 新增单测 |
| 4 | ✅ **heading scales 拉开 + 标题块间距**（已完成 2026-10-02，#23 F4/F5：vendored ①类 `02b33ad` + app `f138140`；无头像素验收落档 [preview-typography-acceptance.md](preview-typography-acceptance.md)，真机目视 M1–M6 留人工） | vendor + `theme_presets.rs` | vendor check.sh + 主仓六项（各自实跑全绿，F4 另含 vendored 10+1 新测试） |
| 5 | 明暗两套真机截图对比（自动侧明暗两轮像素验收已随第 4 行做过，**真机截图本体留人工**，对应验收文档 M5） | 截图 | 目视验收 |

**分支纪律**（AGENTS §8）：上述分两条 commit 走——vendor 改动用 `vendor:` 前缀标 ①类；app 侧改动注明「非 vendor」。`main` 禁止直推，走 `feature/<主题>` + PR。

---

## 6. 已完成清单

- [x] 预览行高根因定位（定量：H1 亏 6.9px / H2 亏 3.2 / H3 亏 0.9）
- [x] `MarkdownStyle::line_height_ratio` 字段（serde + Hash + UI）
- [x] `line_height_for()` 替换写死常量，标题重算行高
- [x] vendor 六项门禁 + 主仓 597 测试全绿
- [x] 键位改排（2026-10-01，780a79c）：主题 → Alt+T，旧 `keymap.json` 值感知迁移「旧默认→新默认」不覆盖用户绑定，mac 显示 ⌥（取舍见 decisions-pending #69）
- [x] TabRestore 完整实现（2026-10-01，881edfd）：关闭栈 LIFO / 只记已落盘 / 封顶 20 / 失败丢弃继续（取舍见 decisions-pending #70），「文件」菜单入口 + state 侧四条单测 + command 侧两条键位断言（K1/K2 联动）；对照明细见 §3.3 落地对照
- [x] 真机项登记（2026-10-01）：Win11 Alt+T 助记键 / macOS ⌥T / 三平台 Cmd/Ctrl+Shift+T 捕获回归，三项留坤哥人工，见 §3.4 真机项清单
- [x] 标题字号分级 / 块间距（2026-10-02，#23 F4/F5：vendored ①类 `02b33ad` + app `f138140`）——scales 新默认 [2.0,1.55,1.30,1.15,1.08,1.0]、`MarkdownStyle::heading_space_above=4.0`（落点 `build_layout` 透明 spacer 行而非 `render_token_range`，decisions-pending #74；`render_token_range` 零改动、纯正文否决线测试钉死）、`theme_presets base()` 显式钉 4.0 与 vendored/出厂三方交叉断言、行距滑杆与皮肤标题间距正交单测；无头像素验收落档 [preview-typography-acceptance.md](preview-typography-acceptance.md)（spacer 行精确 12.00px/section 字号精确/呼吸差恰 12.00px/纯正文 rows 与高度全不变），**真机目视 M1–M6 留人工**（§5 表第 5 行同此）
