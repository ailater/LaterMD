# 右栏对照预览退役：能力并入 Live 写作渲染

> 状态：**待放行**（2026-10-14 定稿，尚未开工）
> 目标：把右栏「对照预览」的独有能力补齐进 Live 写作渲染，然后**彻底删除右栏预览面板**，全项目只剩一条富渲染路径。
> 行号均为定稿时的快照，执行时以 grep 复核为准。

## 阶段进度

| 阶段 | 内容 | 状态 | 完成日期 | PR |
|---|---|---|---|---|
| P0 | 共享渲染件搬家(零行为变化) | 完成 | 2026-10-14 | [#238](https://github.com/ailater/LaterMd/pull/238) |
| P1 | Live 补齐：图片 / heal / 延迟高亮 / emoji 探针 | 完成 | 2026-10-15 | [#239](https://github.com/ailater/LaterMd/pull/239) |
| P2 | Live 补齐：AI 指令卡与 ai:// 链接 | 完成 | 2026-10-15 | [#240](https://github.com/ailater/LaterMd/pull/240) |
| P3 | 禅定切到 Live 浏览态 | 完成 | 2026-10-15 | [#241](https://github.com/ailater/LaterMd/pull/241) |
| P4 | 删右栏（面板 / 布局状态 / 命令 / 入口 / 常量） | 完成 | 2026-10-15 | [#242](https://github.com/ailater/LaterMd/pull/242) |
| P5 | 测试迁移 + PreviewState 瘦身 | 完成 | 2026-10-15 | [#243](https://github.com/ailater/LaterMd/pull/243) |
| P6 | 文档与 CHANGELOG | 未开始 | | |

### 执行备注(每次推进一步时在此登记,供下一轮接手)

- **P0 已落地的新模块叫 `crate::ui::rich_widgets`**(不是文档原设想的两个),13 个件一次性搬齐,含 `image_rewrites` / `task_checkbox_entries` / `HIGHLIGHT_BG_EXPAND` / `LINE_HEIGHT_FLOOR_SLACK_PX` 四处的可见性提升。
- **`is_openable_href` 是 P0 的遗留口子**:它是 link 协议策略函数(右键菜单「可打开」判定),plan 定稿时还不存在。`live.rs:361` 至今靠 `crate::ui::preview::is_openable_href` 调用,**P4 删 `pub mod preview;` 之前必须先给它安置宿主**(建议随 P2 的 `RichLinkHandler` 一起搬进 `ui/ai_card.rs`,或另起 `ui/link_policy.rs`)。P4 开工时先 grep 确认这一处在零。
- **P0 的行号已作废**:文档里所有 `preview.rs:NNN` 快照行号在 P0 之后全部漂移(搬走 564 行)。后续阶段一律以 grep 复核为准 —— P1 尤其注意,相对图片/`heal`/延迟高亮/emoji 探针四处全要重新定位。
- **P1 已落地,生产入口统一为 `LiveState::rich_block_with_base(index, block_text, base_dir)`**(原 `rich_block` 薄封装已删,测试直接传 `None`)。五层链 wikilink → 高亮 → emoji → 任务 → 图片,`image_map` 进 `rich_cache` 第 7 元、`seen_base`(`Option<PathBuf>`)第 8 元参与命中判定;`click_caret_local` 逆穿顺序相应变为 图片 → 任务 → emoji → 高亮 → wikilink(图片最外层先逆穿)。
- **R1 实测定案(块级 heal 达标,未触发退回)**:latermd_md::block_level_heal_closes_streaming_fragments 钉契约。实测要点:①未闭合 fence **不 heal 也渲染正确** —— pulldown 按 CommonMark 把 fence 闭到 EOF,P1 动因表「fence 渲染残缺」一项实际由解析器兜底;②heal 的可见价值在未闭合行内标记,Live 接线由「块尾无换行的 `**` 帧」红绿锚(live_streaming_frame_heals_unclosed_marks)钉住;③vendored heal 既有边界:尾带换行的未闭合 `**`,闭合符追加到换行后闭不上(CommonMark 行首 flanking),预览同病、块级与整篇 heal 在此等价 —— 将来若要修属 vendor ①类,另起议题;④「整篇 heal 后再切块」备选不可行:healed 偏移与编辑缓冲字节错位,破坏块表=原始文本的安全底线。
- **P1 的传参链**:`layout.rs`(destructure 前取 `live_heal`/`base_dir`)→ `editor::ui`(新增 `base_dir: Option<&Path>`、`heal: bool`,仅 Live 分支消费)→ `live::ui`。defer_offscreen_highlight 固定 `true`(与预览同参,无开关)。emoji 探针 id = `live::emoji_probe_id(editor_id)`,每帧整帧覆盖。
- **P2 开工注意**:live.rs 至今仍引 `crate::ui::preview::is_openable_href`(P0 遗留口子,plan 建议随 RichLinkHandler 安置);`LiveRichHandler` 已带共享 emoji 收集器与 `with_ai` 拆分的自然接缝(clicked 消息通道独立于渲染)。

- **P2 已落地,新宿主叫 `crate::ui::ai_card::RichLinkHandler`**:`preview::AiLinkHandler` 已删,拆层=基础 handler(两栏同一套)+ `with_ai(color, &AiState)` 显式开关;预览恒开(行为零变化)、Live 显式开(D1,#147 in decisions-pending,含回退指南:live.rs 删 `.with_ai(..)` 一行即整体回落)。`LiveRichHandler` 与 `LiveMermaidHandler` 都已退役——Live 每块一个 `RichLinkHandler`(task 坐标域 `Live(块首)`,emoji 收集器全块共享 Rc);mermaid 分派内嵌于 RichLinkHandler,Live 侧块内序号从 0 起(widget id 由块标签 id 命名空间兜住,两块各序 0 不相撞)。`is_openable_href` 安置在 `ui::ai_card`(P0 口子收口,live.rs 不再引 `ui::preview`)。
- **P2 的传参链**:`layout.rs`(编辑器调用点传 `&state.ai`,与 tab 字段借用不相交)→ `editor::ui`(新增 `ai: &crate::ai::AiState`,插在 preview 之后)→ `live::ui` 同位。卡片流状态取全局面态(同时至多一个流),**不受 ai_active_tab 门槛** —— 与预览 handler 同口径:流结束后 last_prompt 仍在,卡片显「已完成」。
- **P3 开工注意**:①`live::ui` 现在 14 参,禅定切 Live 浏览态(D2「只滚不编辑」)要给 `LiveState` 加只滚语义;②mermaid/ai 卡在禅定下会随 `live::ui` 自动可用(无需额外接线),但 `AiState` 传参照抄 layout.rs 编辑器调用点;③preview.rs 的 ScrollProbe/scroll_target 与 `map_source_offset` 五层正向映射仍只在预览侧,P4 删面板前无碍。

- **P3 已落地,D2 定案为 `LiveState::browse_locked` 新字段(#148)**(不是扩展 `browse_only`):`browse_only` 是「还没开始写」的软状态,`browse_locked` 是「该视图不提供写入」的硬锁。禅定帧每帧置 `browse_only=true` + `browse_enabled=false` + `browse_locked=true` **并清 `active`/`pending_caret`/`pending_range`**;锁定下 `jump_to` 只置 pending 做块级滚动,写作浏览态维持既有落点+进编辑。**父 Ui 会被 Live 的 ScrollArea 撑高 ~2px**(auto_shrink false),禅定里必须把正文关进固定矩形的 `new_child` —— 否则其后 `ui::zen_nav` 读到的根 Ui max_rect 越过窗口底缘,两条既有禅定测试几何漂移(2026-10-15 实测,P4 时 `preview::ui` 不再被调用,这条经验随之失效)。退出禅定在 `State::toggle_zen` 还原 `browse_enabled`。
- **P4 开工注意**:①禅定已不调 `preview::ui`,三栏预览 (`layout.rs` 的 `Panel::right("preview")`) 成为最后一个生产消费者;②`LiveScrollProbe` 已在 live.rs(`scroll_probe_id`),P5 把它当预览 `ScrollProbe` 的替代品,预览侧那套可连同胞弟 `map_source_offset` 一起删;③`is_openable_href`(P0 口子)已在 P2 安置到 `ui::ai_card`,此时 `grep -rn "ui::preview::" crates/` 应只剩 `preview::ui` 自身的调用与模块判定;④`browse_locked` 不持久化、不进 `State`,`enter_zen`/`exit_zen` 的序列化不受影响。

- **P4 已落地(净删 945 行)**:面板/命令/布局字段/入口/常量全链摘除;`LayoutSettings` 删 `right`/`right_width`/`pre_live_right`,`pre_zen` 快照退化为单值(`Option<bool>`);`Command::ALL` 44→43,Ctrl/Cmd+Alt+R 消失;`TITLE_BUTTONS` 7→6。**几何连锁效应**(既有断言因此改写,后来者勿误判为回归):①中央编辑区变宽 → minimap `automatic_visible` 的 600px 门槛在 900px 窗口档翻真;②格式工具条恒在宽档 → 「更多」溢出按钮不再绘制,三栏取证信号改用恒绘的「H1」;③`automatic_visible`/`EDITOR_MIN_W` 预算断言的唯一邻居变成中央区。R6 裁定:keymap.json 残留 `toggle_right_preview` 键是 BTreeMap 多余条目,`get()` 按 Command id 查询永远查不到它,**无副作用,不清理**;R7 复定:两个 perf harness 直用 vendored label 自建 id,不受影响。**R6/R7 至此销账**。
- **P5 开工注意**:①`preview.rs` 已挂模块级 `cfg_attr(not(test), allow(dead_code))`,P5 迁移测试后删 `pub mod preview;` 时连同豁免一起摘;②`PreviewState` 五字段(`rendered`/四张映射)生产侧已零读者(仅 state 归约在写),P5 瘦身时 `state.rs` 归约侧的 4 次 `expand_*_with_map` 全文扫描一并删;③大纲跳转悬置测试已删(决策 #80 作废),`PreviewState::scroll_target` 与 `ScrollProbe` 的删除在 P5 清;④`docs/m5-acceptance.md` 已加 P4 后记,其余文档(P6 清单)未动。

- **P5 已落地(净删 3277 行)**:`PreviewState` 只剩 `text`/`synced_rev`/`outline`(`new()` 不再做五层改写),`ui::preview` 整模块删除。迁移存留 5 条测试:相对图片改写 4 条 → `ui::rich_widgets`(`image_rewrites` + `OffsetMap::apply`,即 Live 第 5 层的实际写法),`is_openable_href` 判定矩阵 → `ui::ai_card`。**决策 #80(面板收起期间跳转悬置)随 `scroll_target` 一并作废**,P6 必须清档(decisions-pending 那条 + roadmap 提及)。
- **P6 开工注意**:①代码侧到此全部完成,剩文档与 CHANGELOG;②本模块 doc 死链已在 P5 清(5 处),P6 改文档时若再删符号须同步 `cargo doc`;③`docs/decisions-pending.md` 待处理条目:#80(作废)、#78/#79(重写为 Live 通道)、#115(交付面删右栏)、#121/#122/#129(Alt 系七条变六条),另需把 #147(D1)/#148(D2)两位 newcomer 与现有体系串起来;④R2(id 含块序号的缓存抖动)仍未解决,登记进 decisions-pending 作为后续性能项。

- **【补充 2026-10-15】右栏对照预览已按坤哥指令回归为「源码+对照」三态视图**(两个 PR,均非 vendor):[#250](https://github.com/ailater/LaterMd/pull/250) 让 `live::ui` 可被两列各自调用(`ScrollArea` id 参数化 + `TabState::live_right` 独立块状态与独立 id 命名空间),[#252](https://github.com/ailater/LaterMd/pull/252) 落三态(`RenderMode::Split` + `cycle()`、标题栏三段开关、右栏只读面板、现代版分隔线)。**右栏渲染的是 Live 富渲染本身**,不是另一套渲染器 —— P4 删掉的仍是「第二条渲染链路」这个事实本身,本次回归没有重新引入它。P6 的文档清单因此要改写:不再写「对照预览已退役」,改为「对照预览 = 三态视图的分栏档(只读 Live 渲染)」;decisions-pending 里与右栏相关的条目(#78/#79/#80/#115/#121/#122/#129)按三态重述,并新增三态视图的决策条目(互斥三态 / 右栏只读 / 跟随一次 / 分隔线回归)。


## 阻塞

（无。遇到编译不过 / 测试红 / 决策点时在此记录：问题现象、已排除的可能、建议的两个方案，等人工裁决。）

---

## 1. 为什么做

### 1.1 直接动因

同一份文档在两种模式下观感不一致。差异不是「皮肤没对齐」，是**两条渲染链路各自长出来的**：

| 能力 | 右栏预览 | Live 写作渲染 | 差 |
|---|---|---|---|
| 相对图片 `file://` 改写 | 有（`preview.rs:146` `resolve_relative_images`，绘制帧现算） | **无**（`live.rs` 全文无此调用） | `./x.assets/a.png` 在写作模式下出不来 |
| `heal()` 补闭合（AI 流式） | 有（`preview.rs:1053`，全库唯一生产调用点） | **无** | 未闭合 fence / `**` 在写作模式下渲染残缺 |
| `defer_offscreen_highlight` | 有（`preview.rs:1059`，20k 行冷首切 876→173ms） | **无** | 大文档写作模式首切更慢 |
| AI 指令卡 + `ai://` 链接 | 有（`AiLinkHandler` `preview.rs:307-645`） | **无**（`LiveRichHandler` 只接 mermaid/hl/emoji/task/wiki） | 写作模式里没有 AI 卡入口 |
| emoji inline widget + 探针 + 悬停手型抑制 | 有（`preview.rs:515-558`、`1075-1083`） | 画得出，**无探针、无手型抑制** | 悬停 emoji 光标形态不一致 |
| 块间间距 | vendored 默认 `item_spacing.y=3` + 内部 gap | `live.rs:704` 归零，间距由 `block_gap` 前距独占 | **整篇与分块的纵向节奏不同** |
| 字体 / 高亮底色 / 代码复制头 | — | — | 已同源（`editor_font_size` + `preview_body_family`、`append_highlight_section`、`code_copy_buttons`） |

### 1.2 结构性动因

- 两者**从不同屏**（进写作强制收右栏 `state.rs:2887`），保留两套实现没有用户可见价值，只有双份维护成本。
- 右栏独占了 6 项能力、Live 独占 11 项，交集只有「vendored `MarkdownLabel` 内核」。合成一条路径后，能力不再有「哪一栏才有」的问题。
- 删面板后 `PreviewState` 的 `rendered` + 四层 `offset_map/highlight_map/emoji_map/task_map` **生产侧零消费者**，每次修订号前进的 4 遍全文改写可以整条拿掉。

### 1.3 明确不做

- 不做「渲染粒度统一」——右栏整篇一个 label、Live 每块一个 label，这个分歧**不能也不必统一**：整篇 label 中间插不进 TextEdit，块级 label 是 Live 可编辑的前提。删右栏后粒度只剩块级一种，分歧自然消失。
- 不做 True WYSIWYG（AGENTS.md §7 non-goals）。

---

## 2. 最终形态

| 入口 | 渲染什么 | 可编辑 | 实现 |
|---|---|---|---|
| 写作 | 块级富渲染，活动块显示源码 | 是 | `live::ui`（能力补齐后） |
| 源码 | 纯源码 + gutter + minimap | 是 | `ui::editor::ui` Source 分支 |
| 禅定 | **Live 浏览态**（块级富渲染，只读） | 否 | `live::ui` + `browse_only` |

- 顶部仍是「写作｜源码」两个平级模式（`ui/workbench.rs:428` `mode_switch`），`ToggleLivePreview` 命令保留。
- `ToggleRightPreview` 命令、标题栏 `PanelRight` 按钮、视图菜单条目、`layout.right` / `right_width` / `pre_live_right` 全部删除。
- 大纲点击跳转统一走 Live 的 `cursor.jump_to` 通道（`live.rs:668`），`PreviewState::scroll_target` 与 `ScrollProbe` 退役。

---

## 3. 阶段划分

七个 PR，每个独立可合入、独立可回退。门禁按 AGENTS.md §8：本地跑完六项再开 PR，合入后确认 main 的 CI 绿。分支前缀 `feature/live-unify-<阶段>`。

### P0 — 共享渲染件搬家（零行为变化）

`preview.rs` 里有 13 个函数被 `live.rs` / `mermaid.rs` / `selection_ai.rs` 复用，**不能随面板一起删**。先搬出来，让 `live.rs` 不再 import `ui::preview`。

| 函数 | 现位置 | 搬到 |
|---|---|---|
| `code_copy_buttons` / `copy_button_rects` / `copy_button_probe` | `preview.rs:908 / 883 / 873` | 新建 `ui/rich_widgets.rs` |
| `task_click_hit` / `parse_task_href` / `paint_task_checkbox` / `TaskProbeDomain` / `task_checkbox_rects` / `task_checkbox_probe` | `preview.rs:800 / 658 / 675 / 646 / 776 / 786` | 同上 |
| `stash_focus_on_press` / `restore_stashed_focus` | `preview.rs:823 / 839` | 同上 |
| `append_highlight_section` / `highlight_bg_color` | `preview.rs:76 / 59` | 同上 |
| `wiki_link_color` / `ai_link_color` | `preview.rs:31 / 22` | 同上（或并入 `theme.rs`） |
| `resolve_relative_images` / `image_rewrites` / `is_relative` / `file_uri` | `preview.rs:146-302` | 同上（P1 起 Live 也要用） |

**验收**：`cargo test --workspace --all-features` 全绿；`grep -rn "ui::preview::" crates/latermd-app/src/live.rs` 只剩 P1 要补的空缺。
**回退**：单 PR revert。

### P1 — Live 补齐：图片、heal、延迟高亮、emoji 探针

| 项 | 做法 | 风险点 |
|---|---|---|
| 相对图片 | `live.rich_block()` 之后加第 5 层改写（复用 P0 搬出的 `resolve_relative_images`），**块级改写结果纳入 `rich_cache`**，cache key 增加 `base_dir` 维度（换目录/首次落盘必须失效）。`live::ui` 新增 `base_dir` 参数，由 `ui::editor::ui` → `ui::layout.rs` 传入（与现在传给 `preview::ui` 的同一来源：`tab.document.path.parent()`） | cache key 漏 base_dir → 换目录后图片不刷新 |
| `heal()` | `live.rs` 的 `MarkdownLabel` 加 `.heal(heal)`，`heal` 由 AI 流式条件传入（同 `layout.rs:468` 的 `streaming_here` 口径） | **块级 heal 是否够用需实测**：AI 流式时未闭合 fence 可能把后续内容吞成一个块。先在 `latermd_md` 加一个块级 heal 的单元测试（喂残缺 fence / 未闭合 `**`），不达标则退回「流式期间整篇 heal 后按块切」 |
| `defer_offscreen_highlight` | `live.rs` 的 `MarkdownLabel` 加同名参数 | 低 |
| emoji 探针 + 手型抑制 | 照 `preview.rs:1075-1083` 在 Live 帧末写探针、悬停置 `CursorIcon::Default` | 低 |

**验收**：
- 新测试：写作模式下相对图片出图（照 `preview.rs:2504` `local_file_image_paints_through_loaders` 改写）。
- 新测试：AI 流式帧下 Live 渲染不出现未闭合 fence 的残缺排版。
- 现有 51 个 live 测试 + 45 个 preview 测试全绿。

### P2 — Live 补齐：AI 指令卡与 `ai://` 链接

`AiLinkHandler`（`preview.rs:307-645`）目前**无外部消费者**，是纯右栏能力。搬进 `ui/ai_card.rs`（卡片三态 + 执行按钮 + `ai://` 链接样式）。

**决策 D1（要写进 decisions-pending）**：`decisions-pending.md:362-364`（#51 M3 岔路②）曾裁决「Live 不复用 `AiLinkHandler`，避免借 mermaid 顺手改变 ai 卡/emoji/wikilink 行为」。本次是**显式反向决定**：右栏退役后 AI 卡必须有宿主，故把 handler 拆成两层——`RichLinkHandler`（wiki/hl/emoji/task/mermaid，两栏同一套）+ `with_ai(bool)` 开关（AI 卡渲染与 `ai://` 拦截，Live 侧开启）。原裁决的实质（「不能顺手改变」）由显式开关延续，不是无声漂移。

**验收**：新测试照 `preview.rs:1322` `ai_block_renders_card_without_panic` 改写，跑 `live::ui` 路径；`ai://` 点击发消息的路径在 Live 下可达。

### P3 — 禅定切到 Live 浏览态

`draw_zen`（`ui/layout.rs:887-1032`）的正文目前就是 `preview::ui`（`:969`）。改为调 `live::ui`，并强制只读：

- 禅定帧内临时置 `live.browse_only = true` + `browse_enabled = false`（点击不进编辑、不抢焦点）。**这是 UI 侧临时值，不写回 `State`**（禅定退出不污染持久化的 `render_mode`）。
- 禅定与当前 `render_mode` 解耦：源码模式下进禅定也看富渲染，与现状一致。

**决策 D2（要定）**：大纲点击在禅定/浏览态下应「只滚动不进编辑」。Live 现在的 `cursor.jump_to`（`live.rs:668`）会 `browse_only = false` 强制进编辑。需要给 `LiveState` 加一条「只滚动」语义：浏览态下 `jump_to` 只置 `pending_caret` + `caret_follow`、保持 `browse_only`。这也顺带解决写作浏览态下点大纲不该被拽进编辑的问题。

**验收**：`zen_draw_skips_the_three_column_panels`（`layout.rs:7041`）、`zen_body_ui_is_clamped_to_the_reading_width`（`:7091`）、`zen_roundtrip_restores_the_exact_column_combination`（`:7266`，去掉 right 分量）全绿；手工验禅定下滚动/点击不进编辑。

### P4 — 删右栏

| 位置 | 动作 |
|---|---|
| `ui/layout.rs:437-489` | 整块删除（`preview_max`、`Panel::right("preview")`、`pane_heading("对照预览","只读")`、`preview::ui` 三栏调用、`remember_panel_width("preview")`） |
| `ui/layout.rs:373-380` | `reserve_preview` 删除，`sidebar_max` 改为 `available_width() - EDITOR_MIN_W` |
| `ui/layout.rs:969` | 改调 `live::ui`（P3 已做，此处仅剩清理） |
| `layout.rs:40-42 / 75-76 / 62-64` | 删 `right` / `right_width` / `pre_live_right` 字段与 `Default`（`:84/:92`） |
| `layout.rs:142-145 / 176-204` | `save_to` 的 `pre_live_right` 还原段删除；`enter_zen`/`exit_zen` 的 right 分量退化 |
| `state.rs:2886-2888 / 2933-2935` | `toggle_live_preview` 里存/还右栏的两段删除 |
| `state.rs:2011-2028` | `toggle_right_panel` 删除（含 Live 下先切源码再开右栏的分支） |
| `state.rs:655-657 / 1140` | `Message::RightPanelToggled` 变体与归约删除 |
| `command.rs:181 / 257 / 320 / 375 / 428 / 555 / 646 / 680` | `Command::ToggleRightPreview` 定义/ALL/归组/id/label/默认键/图标/message 全删；`:195/:545/:568/:576/:1408` 的 Alt 层注释改写 |
| `ui/workbench.rs:389-401` | 标题栏 `PanelRight` 按钮删除，右侧按钮序列重排；`:466` tooltip 文案改 |
| `ui/titlebar.rs:33 / 48 / 61 / 566 / 600` | `TitleButton::PanelRight` 变体删除，`TITLE_BUTTONS` 7→6（同步 `docs/m5-acceptance.md:52` 的 x-runs） |
| `ui/menubar.rs:96 / 186 / 232 / 1658` | 视图菜单条目、助记字母、`toggle_checked` arm、测试删除 |
| `ui/workbench.rs:725-749` | `pane_heading` 删除（删后唯一消费者消失） |
| `ui/tokens.rs:98-101` | `PREVIEW_DEFAULT_W` / `PREVIEW_MIN_W` 删除 |
| `main.rs:136-153` | 启动塞回宽度里删 `"preview"` 一项 |
| `ui/mod.rs:17` | `pub mod preview;` 删除（P5 之后） |

**持久化**：`LayoutSettings` 是结构体级 `#[serde(default)]`，旧 layout.json 的 `right`/`right_width` 键被静默忽略，**无需新增迁移**（同 `legacy_json_without_maximized_still_loads` 口径）。`keymap.json` 会残留 `"toggle_right_preview"` 键——照 `keymap.rs:396-427` 的样板加一条「删除不在 `Command::ALL` 中的键」的清理（可选，不清理无副作用）。

**验收**：`cargo test` 全绿；手工验出厂布局、窄窗、禅定往返、重启恢复。

### P5 — 测试迁移与 `PreviewState` 瘦身

1. **测试迁移**：`preview.rs` 里 4 个 helper（`render_emoji_frame` 1683 / `render_copy_frame` 2774 / `render_task_frame` 3024 / `render_highlight_frame` 3522）与 10 个直接调用 `preview::ui` 的测试，整体改指向 `live::ui`（先给 `live.rs` 造等价 helper）。三处 `ScrollProbe` 断言改写到 Live 侧的新滚动探针。
2. **删除**：`ScrollProbe` 结构体、`scroll_probe_id`、`PreviewState::scroll_target`（`state.rs:234-238`、归约 `:1201`）、`state.rs:5480-5548` 与 `ui/layout.rs:5132-5165` 的相关断言。
3. **瘦身**：删 `PreviewState.rendered` / `offset_map` / `highlight_map` / `emoji_map` / `task_map`（生产侧零消费者），`PreviewState::new`（`state.rs:241-273`）里的 4 次 `expand_*_with_map` 全文扫描一并删除，只留 `text` / `outline` / `synced_rev`。连带删 `state.rs:4144-4315` 一组四层映射测试。

> 这一阶段牵动面最大但与 P4 正交，**单独一个 PR**，混在一起最难回滚。

**收益**：每次修订号前进不再付 4 遍全文改写（大纲只需要 `outline`）。

### P6 — 文档与 CHANGELOG

| 文件 | 改什么 |
|---|---|
| `docs/writing-source-modes.md` | **主战场**：标题与关系表删「对照预览」；13-24 节的对照开关规则重写；25 的 `ToggleRightPreview` 兼容性说明改；29 验证范围去掉「对照开关往返」 |
| `docs/adr-005-layout-and-sidebar.md` | §3.2 面板顺序表删 `Panel::right("preview")` |
| `docs/ui-shell-redesign.md` | 115/130-134 三栏改成两栏；247 键位表删 `toggle_right_preview`；297-305 §8 右预览节重写；354 token 行删 |
| `docs/roadmap.md` | 40/124/132 的「三栏/双栏预览」口径改 |
| `docs/decisions-pending.md` | #80（预览收起期间跳转悬置）**整条作废**；#78/#79 重写为 Live 通道；#115 交付面删右栏；#121/#122/#129 的 Alt 层枚举改写；**新增 D1（AI 卡开关）与 D2（浏览态只滚不编辑）两条** |
| `docs/README.md` / `README.md` | 入口描述改 |
| `docs/menu-coverage.md` | 78 行删；87/92「视图三条」改两条；150 的 Alt 系七条变六条 |
| `docs/image-plan.md` / `emoji-plan.md` | 「预览相对路径」「右栏彩色 emoji」改口径为 Live 富渲染块 |
| `docs/ui-polish.md` / `ui-design.md` / `ui-shell-redesign-v2.md` / `auto-plan.md` / `m5-acceptance.md` / `preview-typography-and-keymap-plan.md` | token 表、ASCII 图、行号引用、标题栏 7→6 |
| `CHANGELOG.md` | 加「右栏对照预览退役，能力并入写作模式」 |
| `AGENTS.md:89` | 补一句「不再有第二栏只读预览」 |

---

## 4. 风险登记

| # | 风险 | 处置 |
|---|---|---|
| R1 | **块级 heal 不够用**：AI 流式时未闭合 fence 可能吞掉后续块 | P1 先写块级 heal 单测；不达标就改成「流式期间整篇 heal 后再切块」，代价是流式帧多一次全文扫描（只在流式帧付，可接受） |
| R2 | Live 块级 label 的 id 含块序号（`live.rs:1101` `("live-render", index)`），敲回车分裂块即平移 → vendored 缓存 miss | 不在本次解决（删右栏后粒度只剩块级，无对照）。登记进 decisions-pending，作为后续性能优化项 |
| R3 | 禅定改走 `live::ui` 后可能抢焦点/进编辑 | P3 强制 `browse_only + !browse_enabled`；写一条「禅定帧内不改 `State.render_mode`、不写 `cursor.byte`」的守门测试 |
| R4 | `rich_cache` 漏 base_dir 维度 → 换目录后图片不刷新 | P1 把 base_dir 纳入 cache key 并写测试 |
| R5 | `PreviewState` 瘦身牵动 `state.rs:4144-4315` 一组映射测试 | 单独 PR（P5），便于整体回滚 |
| R6 | 删除后 `keymap.json` 残留废弃键 | 可选清理；不清理无副作用 |
| R7 | 性能基线失真：`ui/perf_finding.rs` / `ui/tab_switch_perf.rs` 复刻的是右栏 label 配置 | P4 后复核这两处的配置是否仍代表生产；`tab_switch_perf.rs` 调 `resolve_relative_images`（`:277/:290`）需改 import |

---

## 5. 全阶段门禁（每个 PR 都跑）

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -D warnings
cargo clippy --workspace --all-targets --no-default-features -D warnings
cargo clippy --workspace --all-targets --all-features -D warnings
cargo test --workspace --all-features
cargo doc --no-deps --all-features
```

加上 P1/P2/P3 的红绿验证：临时改坏实现，确认新守门测试真红。

---

## 6. 排期（粗估）

| 阶段 | 量级 | 说明 |
|---|---|---|
| P0 搬家 | 0.5 天 | 纯搬移，机械改动 |
| P1 图片/heal/高亮/emoji | 1.5–2 天 | R1 是变量 |
| P2 AI 卡 | 1 天 | 含 D1 决策落地 |
| P3 禅定 | 1 天 | 含 D2 新语义 |
| P4 删右栏 | 1.5 天 | 面广但机械 |
| P5 测试迁移 + 瘦身 | 2 天 | 最大但不正交风险低 |
| P6 文档 | 0.5 天 | — |
| **合计** | **8–9 天（约 1.5 周）** | 不含真机人工验收 |

人工验收（三平台真机，无法自动化）：写作模式下图片出图、AI 续写观感、禅定阅读、窄窗布局、标题栏按钮重排后的 macOS 无边框形态。
