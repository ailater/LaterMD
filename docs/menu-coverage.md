# 菜单覆盖矩阵(M1 菜单功能完善)

> 生成于 2026-10-07(#62 M1)。**单一事实源**:`Command::ALL`
> (`crates/latermd-app/src/command.rs`)× 菜单常量 `MENUS`
> (`crates/latermd-app/src/ui/menubar.rs`)。本表是盘点落档,不是第二份
> 清单 —— 代码侧的守门测试见文末;两处不一致时以代码为准,并回来修本表。
>
> 命令列按 `Command::ALL` 的枚举顺序枚举产出(非手抄);「出厂快捷键」取
> `Command::default_shortcut`(实际显示随用户 `keymap.json` 改绑如实变化)。
>
> 「设置」「帮助」(#71 M1)不是命令(直达设置页 / 关于窗,照「设置」
> 先例单独绘制),不进本矩阵;「帮助」菜单含「关于 LaterMD…」一条
> (标题助记 H、条目助记 A,取舍见 decisions-pending #138)。

## macOS 原生菜单（2026-10-10 修复）

`mac_menu.rs` 直接复用 `ui/menubar.rs::MENUS` 的 44 个命令、分段和
`toggle_checked` 状态；不再维护一份缩水的原生占位清单。「视图」在 macOS
显示为「显示」，其余分组保持「文件 / 编辑 / 格式 / 导出 / AI」。
原生 target/action 回调只入队并唤醒 egui，命令仍通过 `Command::message`
归约。快捷键从用户 keymap 同步，改绑与取消绑定立即反映在菜单中。

- 设置恢复外观、快捷键入口，并提供 AI、MCP、图片五页直达；应用菜单的
  「设置…」使用 Cmd+,。「关于 LaterMD…」打开原有关于窗及检查更新功能。
- 编辑补上撤销、重做、剪切、复制、粘贴、全选，交给当前聚焦的 TextEdit；
  文本粘贴读取系统剪贴板，无文本时复用后台图片粘贴路径。
- AppKit target 由应用持有，避免弱引用提前释放；文件菜单不再依赖没有
  NSDocument 响应者的 `openDocument:` / `saveDocument:` 占位 selector。
- 测试覆盖中文选区格式化、设置/关于消息、默认快捷键与改绑、TextEdit
  全选/复制/剪切/撤销/重做；原有 44 命令覆盖测试继续约束共同清单。

## 1. 三列矩阵(44 / 44 全覆盖)

| 命令(label) | 入口 | 出厂快捷键 |
|---|---|---|
| 新建 New | 菜单「文件」· 侧边栏按钮 · 快速打开 | Ctrl/Cmd+N |
| 打开 Open | 菜单「文件」· 侧边栏按钮 · 快速打开 | Ctrl/Cmd+O |
| 快速打开… QuickOpen | 菜单「文件」· 快速打开(自身) | Ctrl/Cmd+P |
| 保存 Save | 菜单「文件」· 侧边栏按钮 · 快速打开 | Ctrl/Cmd+S |
| 另存为 SaveAs | 菜单「文件」· 侧边栏按钮 · 快速打开 | Ctrl/Cmd+Shift+S |
| 导出 HTML ExportHtml | 菜单「导出」· 侧边栏按钮 · 快速打开 | Ctrl/Cmd+E |
| 导出 PDF ExportPdf | 菜单「导出」· 侧边栏按钮 · 快速打开 | —(不绑,#见 command.rs:强凑键位收益盖不过撞键风险) |
| 切换主题 ToggleTheme | 菜单「视图」· 快速打开 | Alt+T(#45 K1,mac ⌥T) |
| 切换侧边栏 ToggleSidebar | 菜单「视图」· 标题栏左面板钮 · 快速打开 | Ctrl/Cmd+\\ |
| AI: Mock 流式续写 AiMockStream | 菜单「AI」· 快速打开 | —(联调入口不抢键位) |
| AI: 生成 commit message AiCommitMessage | 菜单「AI」· 快速打开 | —(同上) |
| AI: 生成摘要 AiSummary | 菜单「AI」· 快速打开 | —(同上) |
| 下一个标签 TabNext | 菜单「文件」尾段 · 禅定左缘导航 · 快速打开 | Ctrl/Cmd+Tab |
| 关闭标签 TabClose | 菜单「文件」尾段 · 标签条 × · 快速打开 | Ctrl/Cmd+W |
| 恢复关闭的标签 TabRestore | 菜单「文件」尾段 · 快速打开 | Ctrl/Cmd+Shift+T(#45 K2) |
| 切换 Live Preview ToggleLivePreview | **菜单「视图」(M1 补)** · 快速打开 | Ctrl/Cmd+/ |
| 打字机模式 TypewriterToggle | **菜单「视图」(#64 M1 补)** · 外观设置页复选框 · 快速打开 | Ctrl/Cmd+Alt+W |
| 专注模式 FocusModeToggle | **菜单「视图」(#64 M2 补)** · 外观设置页复选框 · 快速打开 | Ctrl/Cmd+Alt+D |
| Minimap 缩略图 ToggleMinimap | **菜单「视图」(#67 M2 补,带 ✓ 勾选态)** · 外观设置页复选框(同字段) · 快速打开 | Ctrl/Cmd+Alt+M(#129) |
| 加粗 FormatBold | **菜单「格式」(M1 补)** · 格式工具条 · 快速打开 | Ctrl/Cmd+B |
| 斜体 FormatItalic | 同上 | Ctrl/Cmd+I |
| 删除线 FormatStrike | 同上 | Ctrl/Cmd+Shift+X |
| 行内代码 FormatInlineCode | 同上 | Ctrl/Cmd+\` |
| 链接 FormatLink | 同上 | Ctrl/Cmd+Shift+K |
| 一级标题 FormatH1 | 同上 | Ctrl/Cmd+1 |
| 二级标题 FormatH2 | 同上 | Ctrl/Cmd+2 |
| 三级标题 FormatH3 | 同上 | Ctrl/Cmd+3 |
| 引用 FormatQuote | 同上 | Ctrl/Cmd+Shift+. |
| 围栏代码块 FormatCodeBlock | 同上 | Ctrl/Cmd+Shift+C |
| 分割线 FormatDivider | 同上 | —(插画布性质,只工具条/菜单) |
| 2×2 表格骨架 FormatTable | 同上 | —(同上) |
| 无序列表 FormatBullet | 同上 | Ctrl/Cmd+Shift+8 |
| 有序列表 FormatOrdered | 同上 | Ctrl/Cmd+Shift+7 |
| 任务列表 FormatTask | 同上 | Ctrl/Cmd+Shift+9 |
| 插入图片 ImageInsert | **菜单「格式」插入段(M1 补)** · 格式工具条 · 快速打开 | Ctrl/Cmd+Shift+I |
| 插入 Emoji EmojiPicker | **菜单「格式」插入段(M1 补)** · 格式工具条 Emoji 钮 · 快速打开 | Ctrl/Cmd+Shift+E |
| 复制选中 DuplicateSelection | 菜单「编辑」· 快速打开 | Ctrl/Cmd+D |
| 复制当前行 DuplicateLine | 菜单「编辑」· 快速打开 | Ctrl/Cmd+Shift+D |
| 查找 FindInDoc | 菜单「编辑」· 快速打开 | Ctrl/Cmd+F |
| 替换 ReplaceInDoc | 菜单「编辑」· 快速打开 | Win/Linux Ctrl+H;mac ⌥⌘F(#114,⌘H 被 winit 默认菜单持有) |
| 跳转到行 GotoLine | 菜单「编辑」(#60 M1 既有)· 快速打开 | Ctrl/Cmd+G |
| 插入目录 InsertToc | **菜单「编辑」(#66 M2 补)** · 快速打开 | Ctrl/Cmd+Alt+C(编辑组归位与插入/替换口径见 decisions-pending #127) |
| 禅定模式 ToggleZen | **菜单「视图」(M1 补)** · 标题栏禅定钮 · 右上退出浮层 · 快速打开 | F11 |

「快速打开」= Ctrl/Cmd+P 浮层按 label 模糊匹配执行 `Command::ALL` 全集,
是全部命令的兜底入口(不单列)。

## 2. 缺项补齐记录(M1 之前 → 之后)

M1 之前缺菜单入口的 20 条:格式十五条 + ImageInsert + EmojiPicker +
ToggleLivePreview + ToggleRightPreview + ToggleZen。补齐去向:

> **2026-10-15 三态视图后记**:`ToggleRightPreview` 已随右栏独立开关一并
> 删除(live-unify P4),三态回归后视图切换只留 `ToggleLivePreview` 一条
> (id `toggle_live_preview`,label「切换视图」,Ctrl/Cmd+/,按
> `RenderMode::cycle()` 在三档间推进)。本表该行已删;「视图」菜单条目
> 相应由三条变两条。

- **格式十七条 → 新增「格式」菜单**:行内 / 标题 / 块 / 列表四段与格式
  工具条 `FormatGroup` 同一口径,末尾「插入」段收图片与 Emoji 两个
  对话框类动作(点了不改文档,与十五条直接改文档的格式动作隔开)。
- **视图两条(ToggleLivePreview / ToggleZen)→ 「视图」菜单**:布局外观
  开关一段,禅定(整套面板组合,非普通开关)独立一段。

近期新增命令重点核对结果:GotoLine / 替换 / 查找 / 标签三条 / AI 三条 /
导出两条在 M1 之前已各有菜单入口,本表如实登记,无缺项;插入目录
InsertToc(#66 M2)进「编辑」菜单,落地即覆盖;Minimap 缩略图
ToggleMinimap(#67 M2)进「视图」第一段(专注之后、主题之前),同一
消息翻转 `theme.show_minimap`(#55 M2 既有配置字段),菜单/快捷键/
外观页复选框三方一致。

## 3. 豁免清单

**为空。** `Command::ALL` 的 44 条全部进了菜单。原因:命令注册表当前
不含「上下文类浮标动作」—— 选区 AI 续写/润色是 `selection_ai` 浮标的
局部动作(不经 Command 注册表,`SelectionAiActionRequested` 消息直达),
不属于全局命令;若将来有命令确实不适合进菜单,须在本节登记理由并同步
`every_command_has_a_menu_entry` 的豁免集。

## 4. 分组与排序规范(本菜单栏的实现约定)

- **同类聚组 + 分隔线**:菜单内按「段」分组,段间画分隔线(文件:文件
  动作 | 标签;格式:行内 | 标题 | 块 | 列表 | 插入;视图:开关 | 禅定)。
- **开关类条目勾选态**(#67 M2):视图菜单的布偶开关(侧边栏/预览栏/
  Live/打字机/专注/minimap/禅定)条目前置勾选列 —— 开 = `✓` 前缀,关 =
  同宽空格占位(条目文字主体对齐);真值从 `State` 单一事实源取
  (`toggle_checked`,与设置页/标题栏按钮同源)。「切换主题」是明暗互换
  而非开/关,不带勾选列;非开关命令同样无前缀。
- **常用在前**:菜单栏顺序 文件 → 编辑 → 格式 → 视图 → 导出 → AI →
  设置 → 帮助(高频编辑动作在前,派生动作在后,帮助居末位是桌面
  惯例);段内按使用频率排
  (新建 → 打开 → 快速打开 → 保存 → 另存为)。
- **破坏性/不可逆项**:命令注册表当前**不含**回滚/丢弃类命令 —— Git
  回滚在 Git 面板上下文菜单(已有确认模态 + 不可逆警示)、草稿丢弃在
  恢复条(已有确认语义)、关标签丢弃在确认浮窗,均为上下文动作不经
  Command 层。菜单侧无破坏性条目需要隔离;将来新增破坏性命令时必须
  独立成段与常规项隔开并配确认模态。
- **菜单项右侧显示出厂快捷键**:取自 `keymap.get(cmd)` 经
  `Context::format_shortcut` 平台化显示(与工具栏 tooltip 的
  decisions-pending #32 同源先例一致)—— 显示的是**当前生效绑定**
  (出厂值或用户改绑值),用户自定义绑定如实反映;无绑定的命令只显示
  名字。

## 5. 助记字母与 Alt 键位审计

- **助记字母**:egui 0.36.2 无 `&` 助记字母机制(registry 源码 grep
  `mnemonic`/`"&"` 零命中),本菜单全部标题与条目为纯中文文本,不含
  `&` 前缀 —— **当前零冲突**。若将来 egui 引入助记字母,启用前须重审:
  Windows 惯例的 Alt+<字母> 助记会与下述 Alt 系出厂键位相争。
- **标题助记集**(Alt 命名空间,全局唯一):F(文件)/ E(编辑)/
  O(格式)/ V(视图)/ X(导出)/ A(AI)/ S(设置)/ **H(帮助,
  #71 M1 —— H = Help 词首,七字母之外空闲,keymap Alt 系七条也无
  Alt+H;审计测试 `title_mnemonics_unique_and_clear_of_keymap_alt_
  bindings` 的标题集已含 H)**。「帮助」菜单条目助记 A(About 词首,
  菜单内唯一;条目层是菜单内命名空间,与设置菜单的 A(外观)跨菜单
  重复允许)。
- **Alt 系出厂键位逐一清点**(全部含 Alt 的绑定,共六条 —— 原七条的
  `Ctrl/Cmd+Alt+R` = ToggleRightPreview 已随该命令删除):
  1. `Alt+T` = ToggleTheme(#45 K1;mac 显示 ⌥T);
  2. `⌥⌘F` = ReplaceInDoc(仅 mac 出厂,#114);
  3. `Ctrl/Cmd+Alt+C` = InsertToc(#66 M2,C = Contents;菜单标题助记集
     F/E/O/V/X/A/S 不含 C,#127);
  4. `Ctrl/Cmd+Alt+W` = TypewriterToggle(#64 M1;与 Ctrl/Cmd+W 关标签
     只差一个 Alt,靠修饰键个数降序共存,#121);
  5. `Ctrl/Cmd+Alt+D` = FocusModeToggle(#64 M2;与 Ctrl/Cmd+D 复制选中
     同款共存,避开助记集 F/E/O/V/X/A/S,#122);
  6. `Ctrl/Cmd+Alt+M` = ToggleMinimap(#67 M2,M = Minimap 词首;Ctrl/
     Cmd+Alt 层 C/W/D 之外 M 空闲,避开助记集 F/E/O/V/X/A/S,#129)。
  六条互不冲突,且 `matches_logically` 对「显式无 Ctrl/Cmd」的 Alt+T
  要求事件确实没按 Ctrl/Cmd(`alt_t_fires_only_toggle_theme` 钉住),
  与 Ctrl/Cmd+Alt+R / Ctrl/Cmd+Alt+C 不相抢。

## 6. 守门测试(矩阵的代码面)

全部在 `crates/latermd-app/src/ui/menubar.rs` 的 `tests` 模块:

| 测试 | 钉住的不变量 |
|---|---|
| `every_command_has_a_menu_entry` | 遍历 `Command::ALL`,每条恰好出现在一个菜单一次(豁免集当前为空) |
| `menu_placement_matches_command_group` | 命令挂的菜单 == `Command::group()` 对应菜单(Tab 组挂「文件」是 `CommandGroup` 文档的既定豁免) |
| `clicking_every_menu_item_sends_its_command_message` | 全部 44 个菜单条目逐个点击,发出的消息即该命令的归约入口(与快捷键触发殊途同归) |
| `all_menu_sections_render_without_panic` | 全部菜单的分组段展开渲染(不经 `menu_button` 折叠)不 panic、纯渲染零消息 |
| `menu_item_shortcuts_follow_keymap` | 菜单项键位文本与 keymap 同源:出厂如实显示、改绑后跟随新键位、旧键位消失 |
| `edit_menu_lists_every_edit_command`(既有) | 编辑组命令与「编辑」菜单互为镜像 |

菜单结构常量(`FILE_MENU`/`EDIT_MENU`/`FORMAT_MENU`/`VIEW_MENU`/
`EXPORT_MENU`/`AI_MENU`/`MENUS`)是绘制与测试的同一事实源:删条目先
撞常量长度编译错,换错条目撞覆盖断言 —— 不存在「渲染里消失而测试仍绿」
的路径(已实测:ExportPdf 换成重复 ExportHtml,覆盖断言当场红)。

## 7. 真机目视项(单测覆盖不到)

- 菜单排版观感:分隔线位置、段间距、长标签(「AI: 生成 commit
  message」)与右侧键位文本在 1x/2x 缩放下的挤占情况。
- 助记键/Alt 系真机捕获:Win/Linux Alt+T 与窗口管理器全局快捷键的
  相争情况因发行版而异,需真机核对(mac ⌥T 同理,CI 无头环境测不了)。
