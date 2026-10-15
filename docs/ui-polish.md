# 界面打磨设计（批次 B-light：已完成功能的视觉与交互定稿）

日期：2026-09-25
状态：已接受
关联：[roadmap.md](roadmap.md)「专题：界面美化与皮肤系统」、[adr-004](adr-004-technical-stack.md)、[adr-005](adr-005-layout-and-sidebar.md)、[adr-002](adr-002-platform-renderer-wysiwyg.md)

> 本文件是**本轮打磨的设计规格**，不重复 ADR 的论证。
> 范围刻意收在「已完成功能的打磨 + AI 配置页 + MCP 规划」三块；皮肤文件的三态切换
> （批次 B 的自定义皮肤 RON）不在本轮，仍留在阶段 4.5。

---

## 1. 设计原则（本轮四条）

1. **图标是矢量自绘，不是字体字符。** egui 无内置图标集；emoji / Unicode 符号（✎ 🗋 ⌘）
   在三平台字体下缺字风险真实存在（AGENTS.md §5 已把中文字体列为头号风险项之一）。
   因此图标一律用 `Painter` 画线段/圆/矩形，零字体依赖、零纹理依赖、随主题取色。
2. **工具栏只留高频动作，其余收进设置。** 工具栏不是「全部命令的投影」——可发现性由
   菜单栏负责（P0 既定分工）。本轮工具栏从 5 个纯文字按钮 + 一个塞满杂项的「设置」
   菜单，改为「文件组 / 视图组 / AI 组」三段 + 右侧文档状态 + 齿轮入口。
3. **设置从「菜单里的浮窗」升级为「独立对话框 + 左侧分页」。** 凭据浮窗只有一项时可
   以塞进菜单，AI 配置有 8 个字段、快捷键有 N 行，菜单装不下。
4. **状态栏是新增的信息层，不是装饰。** 把散落在工具栏/侧边栏边缘的只读信息
   （路径、行列、字数、主题、渲染后端、AI provider、MCP）收到底部一行，工具栏即可瘦身。

---

## 2. 设计 token（本轮引入，落在 `ui/tokens.rs`）

> **2026-09-27 回写（外壳重构 M5 收口）**：本表按 `tokens.rs` 现值逐项核对订正。
> `TOOLBAR_H` 已于 2026-09-26 订正为 28，本轮复核一致。三处设计与实现不一致：
> `SPACE_LG` / `STATUSBAR_H` / `RADIUS_MD` 从未落地进 `tokens.rs`（git `-S` 全历史
> 只命中本文件），删划线标注；`ACCENT` 已随 WorkBuddy 风外壳改为飞书蓝
> （decisions-pending #30，2026-09-26）。外壳重构新增 token 见 §2.1。
> **2026-09-28 更新（UI 现代化 U0）**：`RADIUS_MD` 已落地进 `tokens.rs`（=6，
> 本表下方原划线状态随之解除）；U0 另扩五个新 token，见 §2.2。

| 类别 | token | 值 | 用途 |
|---|---|---|---|
| 间距 | `SPACE_XS` | 4 | 图标与文字间隙 |
| | `SPACE_SM` | 6 | 按钮内边距（水平） |
| | `SPACE_MD` | 10 | 工具栏分组间距 |
| | ~~`SPACE_LG`~~ | ~~14~~ | 设计列出、未落地（无消费者，`tokens.rs` 无此常量） |
| 尺寸 | `ICON` | 16 | 图标方框边长（按钮内） |
| | `ICON_SM` | 13 | 页签/菜单内图标 |
| | `TOOLBAR_H` | 28 | 工具栏按钮高度（原文误记 32，2026-09-26 订正，与 tokens.rs 对齐） |
| | ~~`STATUSBAR_H`~~ | ~~24~~ | 未落地：状态栏高度由行高自然决定（实测 22px，[m5-acceptance.md](m5-acceptance.md) §0） |
| 圆角 | `RADIUS_SM` | 4 | 按钮 |
| | `RADIUS_MD` | 6 | 控件圆角（2026-09-28 U0 落进 `tokens.rs`，此前仅由 `theme::shell_tokens` 投影，#30；见 §2.2） |
| 语义色 | `ACCENT`（`accent(ui)` 函数） | 飞书蓝 `#3370FF`（明）/ `#6C9FFF`（暗） | 页签选中、选中态下划线、主按钮（2026-09-26 WorkBuddy 定，#30） |
| | `WARN` | `#EBB43C` | 可行动降级（沿用回滚 dirty 警示黄） |
| | `DANGER` | `#EB6060` | 不可逆警示（沿用回滚文案红） |
| | `OK` | `#60C878` | 已配置 / 成功态 |

> ACCENT 原定紫罗兰（与 `ai://` 链接色同源，decisions-pending #11）；2026-09-26 换
> WorkBuddy 飞书蓝后，**AI 专属元素（`ai://` 链接、指令卡）保留紫罗兰**—— 强调色中立化后
> AI 是全界面唯一的紫，反而更醒目（#30）。该色现由 `ui::ai_card::ai_link_color` 提供
> **（2026-10-15：`ui::preview` 已随右栏独立渲染链路删除，此处宿主改为 `ui::ai_card`）**。

### 2.1 外壳重构 token 增补（M1–M4，2026-09-27 回写；出处 [ui-shell-redesign.md](ui-shell-redesign.md) §11）

| token | 值 | 里程碑 | 用途 |
|---|---|---|---|
| `TITLEBAR_H` | 36 | M1 | 自绘标题栏高（仅无边框模式画） |
| `WINDOW_BTN` | 32×24 | M1 | 标题栏右侧窗口按钮命中区（Win 风整块，三平台统一） |
| `SIDEBAR_MIN_W` | 180 | M1/M2 | 左栏宽度下限（160→180，R4：三段式后 160 不够） |
| `PREVIEW_DEFAULT_W` / `PREVIEW_MIN_W` | 420 / 260 | M1 | 分栏档右栏（只读对照列）初始宽 / 下限（**2026-10-15**：右栏从独立面板改为三态视图的一档，两个 token 保留并继续服务于它） |
| `NAV_ROW_H` | 26 | M2 | 左栏视图导航行高（选中行实测 26px 见 m5-acceptance §1②） |
| `NAV_BAR_W` | 2 | M2 | 选中行左侧 accent 竖条宽（整行选中态的另一半） |
| ~~`NAV_BOTTOM_H`~~ | ~~28~~ | M2 引入、M5 删除 | 随左栏底段设置行撤销而删除（decisions-pending #31，`d797451`），`tokens.rs` 现无此常量 |
| `FORMAT_BAR_H` | 30 | M3 | Markdown 格式工具条高 |
| `ZEN_TEXT_W` | 720 | M4 | 禅定正文限宽（ui-design.md §1.2「沉浸」参数） |
| `ZEN_EXIT_MARGIN` | 10 | M4 | 「退出禅定」浮层到内容区右上角留白 |
| `ZEN_GUTTER` | 24 | M4 | 禅定内容区四周留白（**必须整数**：`Margin` 是 i8，f32 被 round 吞小数） |

### 2.2 UI 现代化 U0 token 增补（2026-09-28 回写；出处 [ui-modernization.md](ui-modernization.md) §3 U0，数值来源 shadcn/armas，`tokens.rs` 带来源注释与 `u0_tokens_match_sourced_values` 数值断言）

| token | 值 | 用途 |
|---|---|---|
| `RADIUS_MD` | 6 | 控件圆角（从 §2 划线状态解除，落进 `tokens.rs`；经 `theme.rs::apply_shell_to` 投影五组控件圆角） |
| `INPUT_H` | 36 | 输入类控件高度（投影 `interact_size`；紧凑档按设计值 ×0.7 缩放，不再从 egui 出厂值出发） |
| `INPUT_PAD_X` / `INPUT_PAD_Y` | 12 / 8 | 输入类控件内边距（投影 `button_padding`） |
| `FONT_SM` | 14 | Small 字号（投影进明暗两套 Style） |

> U0 另将九套预设色板（Dracula / Nord / Gruvbox Dark / Solarized Dark·Light / Tokyo Night /
> One Dark·One Light / Rosé Pine）以纯数据字面量编进 `theme_presets.rs`，启动铺成
> `themes/*.ron`（仅文件不存在才写，用户改动不被顶掉）。色板影响面 = 行内代码四色 +
> 块级圆角 + 引用条几何；外壳不随皮肤换色仍是批次 C 既定口径（decisions-pending #43）。

**不引入 CSS 式主题引擎、不做每控件自定义**（roadmap 专题「明确不做」）。token 是
Rust 常量而非 serde 结构 —— 皮肤文件（批次 B）才需要序列化，本轮不需要。

---

## 3. 图标体系（`ui/icons.rs`）

`Icon` 枚举 + `draw(painter, center, size, color)` 纯绘制函数，坐标以 `size` 为基准的
`[-0.5, 0.5]` 归一化空间，线宽 `size / 12`（16px 图标 = 1.33px 线，与 1px 物理像素在高
DPI 下不产生虚线）。

| Icon | 形态 | 落点 |
|---|---|---|
| `New` | 文档矩形 + 右上加号 | 工具栏/菜单 |
| `Open` | 文件夹（底矩形 + 顶部凸舌） | 工具栏/菜单 |
| `Save` | 软盘（外框 + 内窗 + 底条） | 工具栏/菜单 |
| `SaveAs` | 软盘 + 右下折线箭头 | 工具栏/菜单 |
| `Export` | 向下箭头 + 托盘底线 | 工具栏/菜单 |
| `Sidebar` | 外框 + 左侧实心竖条 | 工具栏/菜单 |
| `Theme` | 太阳（圆 + 6 条射线） | 工具栏/菜单 |
| `Ai` | 四角星（sparkle） | 工具栏/AI 菜单/AI 配置页 |
| `Files` / `Search` / `Outline` / `Git` | 叠放文档 / 放大镜 / 三级缩进线 / 分支图 | 侧边栏页签 |
| `Settings` | 齿轮（圆 + 4 辐条） | 工具栏右侧 |
| `Reset` | 回环折线箭头 | 快捷键页「重置」 |
| `FolderClosed` / `FolderOpen` / `File` | 闭合文件夹（与 `Open` 共用同一 match 分支几何，DRY 有意为之）/ 开口文件夹（一笔连画）/ 折角纸页 | 文件树行首（2026-09-29 #32 落地：目录行=三角+随开合切换的文件夹、文件行=纸页；前导空格按字体度量折算防重叠，快照层护栏锁死「图标字符不得回流文本」） |

按钮形态 `icon_text_button(ui, icon, label, shortcut, enabled)`：手工
`allocate_response` + 自绘（hover 底、按下态、圆角、图标、文字、右侧灰阶快捷键文本），
不复用 `egui::Button`（它不接受自定义图标 painter 且 `shortcut_text` 的排版在窄面板下溢出）。

---

## 4. 布局（三栏 → 四区）

> **2026-09-27 修订**：本节图中的「编辑器顶部文件工具栏」一行已退役
> （decisions-pending #32，用户指令「按钮去重」）：文件动作收口到左栏顶段
> 图标版（`ui/sidebar.rs` 的 top_actions，`Command::FILE` + `ExportHtml` 全集），
> AI 走菜单栏「AI」、视图开关在标题栏；`document.notice` 提示行迁编辑器面板顶。
> 下图与其后的工具栏描述保留作历史记录，现行布局以
> [ui-shell-redesign.md](ui-shell-redesign.md) §5 为准。

```
┌──────────────────────────────────────────────────────┐
│ 菜单栏  文件 ▾  导出 ▾  视图 ▾  AI ▾  帮助 ▾          │  ← 图标 + 文字,含全部命令
├──────────────────────────────────────────────────────┤
│ [新建][打开][保存][另存为] │ [栏][主题] │ [AI▸]   ⚙  │  ← 工具栏:三段分组 + 齿轮
├───────────┬───────────────────┬──────────────────────┤
│ 侧边栏    │ 编辑器(源码)        │ 对照(Live 只读)       │ ← 2026-10-15:分栏档,原「预览」
│ ⌸文件     │                    │                      │
│ ⌕搜索     │                    │                      │
│ ☰大纲     │                    │                      │
│ ⑂Git      │                    │                      │
├───────────┴───────────────────┴──────────────────────┤
│ docs/a.md · 行 12:4 · 1,204 字 · 深色 · wgpu · AI:Mock· MCP:关 │
└──────────────────────────────────────────────────────┘
```

- **工具栏**：文件组（4）+ 分隔线 + 视图组（侧边栏、主题）+ 分隔线 + AI 下拉
  （续写 / 摘要 / commit message，合并为一个「AI」菜单按钮，避免三个长文字按钮挤爆窄面板）
  + 右侧弹性空白 + 文档名（dirty 加 `*`）+ 齿轮。
- **AI 组为什么收成下拉**：三个 AI 命令的 label 都很长（「AI: 生成 commit message」），
  平铺在 500px 默认宽度的编辑面板上必然换行；下拉与菜单栏同形态，且视觉上把 AI 归为
  一类能力。
- **状态栏**（新增，`Panel::bottom`）：左→右为 路径 · 行列 · 字数 · 主题 · 渲染后端 ·
  AI provider · MCP 状态。`·` 分隔，超出宽度自动裁剪尾部（保留路径优先）。
- **侧边栏页签**：图标 + 文字，选中态用 ACCENT 下划线而非填充块（左侧面板已有填充式
  selectable_label 用于文件树，页签需与它区分层级）。
- 浮动层（commit 建议、回滚确认、设置对话框）保持 `egui::Window` 形态不变。

---

## 5. 设置对话框（新，`settings.rs`）

左侧竖排分页（外观 / 快捷键 / AI / MCP）+ 右侧内容区，单个 Window，`default_size`
`[560, 420]`。**取代**原工具栏「设置」菜单里的主题列表与 AI Provider 入口：

| 页 | 内容 |
|---|---|
| 外观 | 明/暗定向选择（未来放跟随系统）+ 渲染后端只读显示 + 界面密度切换（紧凑/标准，本轮仅标准可用，紧凑标注待实现） |
| 快捷键 | 命令表：命令名 / 当前键位 / [改键] [清除] [重置本行] / 全局「全部重置」；冲突检测提示 |
| AI | 见 §6 |
| MCP | 见 §7 |

原「设置」菜单保留两项快捷入口（外观、快捷键）供直达，齿轮打开默认页。

---

## 6. AI 配置页（新，`ai_config.rs`）

| 字段 | 控件 | 默认值 | 落盘 |
|---|---|---|---|
| Provider | 下拉：Mock / OpenAI 兼容 / Anthropic（未实现，禁用）/ Ollama（未实现，禁用） | Mock | `ai.json` |
| 接口方式 | 下拉：OpenAI `/chat/completions` SSE / Anthropic Messages SSE（未实现）/ Ollama generate（未实现） | OpenAI SSE | 同 |
| Base URL | 单行文本 | `https://api.openai.com/v1` | 同 |
| 模型 | 单行文本 + 「自填」 | `gpt-4o-mini` | 同 |
| Temperature | Slider 0–2 | 0.7 | 同 |
| Top-P | Slider 0–1 | 1.0 | 同 |
| Max tokens | 拖条 256–8192 | 2048 | 同 |
| System prompt | 多行文本 | 空（不发送 system 消息） | 同 |
| 超时（秒） | 拖条 10–300 | 60 | 同 |
| 流式 | 复选 | true | 同 |
| API key | 密码框 + 保存/清除 | — | **系统凭据**（latermd-creds，不进 ai.json） |

- **未实现的接口方式显式禁用并标注**，不伪装可用（诚实原则，同 decisions-pending #12 的口径）。
- key 与参数分离存储：参数进 `ai.json`（可备份、可分享），key 只进系统凭据。
- 保存后即时重建 provider 运行时（`AiState` 改为 enum：Mock / OpenAI），无需重启。
- Provider = Mock 时参数区整块灰显（参数对 Mock 无意义），避免「配了半天发现没生效」。
- 需要 key 的 provider（`provider_requires_key`）在切换时自动置位，沿用既有 key 闸门
  （decisions-pending #21 的四入口归约不变）。

---

## 7. MCP：规划已落地（2026-09-26）

**目标**：LaterMD 启动后开一个本地 MCP server，让外部 AI（Claude Code / Cursor / 其他
Agent）能调用它检索本机文档库、读取文档内容，而不是让每个 AI 各自去 grep。

**形态**：进程内后台线程 + JSON-RPC 2.0 over **stdio**（主通道，零端口冲突、零防火墙）
+ 可选 **HTTP on 127.0.0.1:<port>**（给不支持 stdio 的客户端）。默认关闭，设置页开关。

**工具集（全部只读，与 P2「Git 只读」同哲学）**：

| 工具 | 入参 | 返回 |
|---|---|---|
| `search_docs` | query, case_insensitive, max_hits | 路径 + 行号 + 摘要（复用 `latermd-app::search`） |
| `read_document` | path, offset?, limit? | 原文分片 |
| `outline` | path | 标题层级列表 |
| `list_files` | dir?, glob? | 文件树条目（复用 `filetree`，尊重 `.gitignore`） |
| `git_status` | — | 改动列表（复用 `latermd-git`，只读） |

**安全边界**：只服务 127.0.0.1；路径必须落在已选文件树根之内（`..` 一律拒绝）；
无写工具（写入一律经 UI 的用户显式动作）；开关默认关；每工具记录调用计数，设置页可见。

**落地分阶段**：① 协议与工具 trait（`latermd-mcp` crate，不依赖 egui —— 守铁律二）→
② stdio server + 进程内装配 → ③ 设置页开关/端口/工具开关/调用计数 → ④ HTTP 通道。
详细排期与验收见 [mcp-plan.md](mcp-plan.md)。

**2026-09-26 已全部落地**（本轮 ui-polish 的占位态随之退役）：设置页 MCP 页为
「启用开关 + 端口 + 五个工具权限复选框 + 状态行 + 调用计数 + 保存/放弃」，与 AI 页同款
「草稿 + 保存」分工 —— 端口与工具权限都是「改了要重建监听」的动作，让用户显式确认
比静默生效可预期；状态栏右侧显示 `MCP: 关 / 启动中 / 127.0.0.1:<port> / 启动失败`。
服务默认关闭，开了只监听回环地址。

---

## 8. 本轮明确不做

- 外壳配色随皮肤换色（需另加 shell token 表，属批次 C）；皮肤市场 / CSS 式主题引擎 / 每控件自定义。
- 标签拖拽排序、紧凑密度实现、皮肤市场。
- MCP 的**写工具**（本轮只有五个只读工具，写入一律经 UI 的用户显式动作）。
