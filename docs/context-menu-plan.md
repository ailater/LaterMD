# 右键菜单规划(context-menu-plan)

日期:2026-10-13
状态:**已批准实施**(坤哥 2026-10-13 指令「都列入规划,开始做吧」)
关联:[AGENTS.md](../AGENTS.md) §6 vendored 渲染层 / §7 范围边界、[ui-shell-redesign.md](ui-shell-redesign.md)、
[decisions-pending.md](decisions-pending.md) #143–#145、[distribution.md](distribution.md) §6(trash 依赖登记)

> 坤哥 2026-10-13 指令:添加右键菜单功能,先列举常用项 → 列举获准「都列入规划,开始做」。
> 四个界面区域一次规划、分模块实施。标签栏右键菜单(#37)已在产,本规划补齐其余三区。

---

## 1. 一句话方案

三个还没有右键菜单的区域(编辑器、文件树、预览)各挂一层 `Response::context_menu`,
菜单条目**全部复用既有命令与消息通道**(Command / Message / compose),零新增动作系统;
唯一的能力增量是文件树的「重命名 / 删除(废纸篓)/ 在文件管理器中显示」三件真实文件操作,
与 vendored 层一处 ①类小改(把链接/图片 widget 的 Response 暴露给 LinkHandler)。

## 2. 实测硬事实(决定方案走向,已逐一验证)

| # | 事实 | 出处 | 对方案的影响 |
|---|---|---|---|
| F1 | `Response::context_menu` 可挂任意响应,弹出/收起 egui 内建 | `ui/tabs.rs:477` 在产(#37) | 菜单 UI 零新依赖,三区同一手法 |
| F2 | egui TextEdit 内建消费 `Event::Copy` / `Event::Cut` / `Event::Paste`(含选区删除、剪贴板写出、TextEdit undoer 快照) | egui 0.36.2 `widgets/text_edit/builder.rs:1114-1145` | **剪切/复制/粘贴走合成事件 = 撤销链完整**,不需要 compose 层重造 |
| F3 | 合成事件通道在产:macOS 原生菜单把 `Event::Copy/Cut/Paste` 与 `Key::Z`(撤销/重做)、`Cmd+A`(全选)推入 `ctx.input_mut` | `mac_menu.rs:285-318` | 编辑器菜单的剪贴板/撤销项**平台中立复用同一条通道**;关键时序:事件必须在 `logic()` 阶段推入(归约跑在 `ui()` 之前),ui() 阶段推的当帧末被清空 |
| F4 | 右键落光标:`Galley::cursor_from_pos(pos) -> CCursor` | epaint 0.36.2 `text/text_layout_types.rs:1181` | 源码模式右键先落光标(VS Code 口径:落点在选区内则保留选区)有现成 API |
| F5 | 程序化写回选区+焦点:`write_selection`(state.store + request_focus) | `ui/editor.rs:901` | 右键落光标与菜单动作后的焦点归还都有现成出口 |
| F6 | `ctx.copy_text` 写剪贴板在产 | `settings.rs:1191` | 复制路径/复制链接地址不需要 arboard |
| F7 | arboard 已是直接依赖(图片粘贴通道提升而来) | `clipboard.rs`(ADR-004) | Linux/Windows 的粘贴文本读取零新增依赖;mac 走 NSPasteboard(mac_menu 先例) |
| F8 | vendored `link()` 的 Response 被弃置(`allocate_exact_size` 后只消费 clicked),图片块 `ui.add(image)` 同样弃置 Response | vendored `layout.rs:218-247`、`label.rs:1268-1297` | 预览链接/图片右键需要 ①类 vendor 小改:经 trait 方法把 Response 暴露给 handler(与 `block_widget` 返回 Response 的既有设计同向) |
| F9 | 破坏性确认对话框 `destructive_confirm_dialog` 在产 | PR #186 | 文件删除确认复用 |
| F10 | 文件树已有 `NewFileDraft` 浮窗 / `create_markdown` / `valid_file_name`(补后缀、撞名让开) | `filetree.rs` | 重命名浮窗镜像同形态;删除能力全新(见 §5) |

## 3. 模块拆分(四模块,依赖关系 M3 ← vendor ①类)

### M1 编辑器右键菜单(源码 + Live 活动块)——核心

挂点:源码 `editor.rs` 的 TextEdit response;Live `live.rs` 的活动块 TextEdit 与空文档块
(非活动块富渲染 v1 不挂,右键不弹——先单击/双击激活再右键,口径记 decisions-pending #143)。

**右键先落光标**(VS Code 口径):secondary click 先经 `cursor_from_pos` 换算字符位;
落点在既有选区内 → 保留选区,否则塌缩为该处光标(`write_selection`)。菜单动作作用的位置
与用户看到的一致。

**菜单条目**(全部映射既有通道):

| 分组 | 条目 | 通道 |
|---|---|---|
| 编辑 | 撤销 / 重做 / 剪切 / 复制 / 粘贴 / 全选 | 新 `EditAction` 消息 → 归约推合成事件(抽出 mac_menu 的 dispatch 为平台中立 `edit_events` 模块,mac 菜单同源);粘贴文本读取 mac=NSPasteboard、Linux/Win=arboard 同步读(F7,egui-winit Ctrl+V 同风险面) |
| 格式 | 加粗/斜体/删除线/行内代码/链接 + 「格式」子菜单全量 15 条 | `Command::FormatXxx` → 既有 `apply_format` / `apply_format_live` |
| 文档 | 复制当前行 / 查找 / 替换 / 跳转到行 / 插入图片 / 插入 Emoji | 既有 `Command::DuplicateLine` / `FindInDoc` / `ReplaceInDoc` / `GotoLine` / `ImageInsert` / `EmojiPicker` |
| AI | 问 AI 此选区(有选区时) | 复用 selection_ai 既有的请求消息 |

状态口径:无选区时剪切/复制禁用;undo/redo 可用性 v1 不探测(TextEdit undoer 无公开
peek API),恒可用,点击无撤销步时无害。

### M2 文件树右键菜单——文件操作补全

挂点:`ui/sidebar.rs` 文件树行 response。**文件**与**目录**两套条目:

- 文件:打开 / 重命名 / 删除 / 复制路径 / 复制相对路径 / 在文件管理器中显示
- 目录:新建文件 / 新建文件夹 / 重命名 / 删除 / 复制路径 / 复制相对路径 / 在文件管理器中显示 / 刷新

**重命名 = 真实文件改名**(非标签别名,那条已在 #37):浮窗镜像 `NewFileDraft` 形态;
`valid_file_name` 校验 + 撞名拒绝;成功后——文件树缓存失效、打开中的标签换绑新路径
(保存目标跟移,缓冲/dirty 保留)、named_draft 草稿迁移、窗口标题/状态栏自然跟随。
改名失败不变式:磁盘不动,浮窗保留。

**删除 = 移入系统废纸篓**(不是 unlink):新依赖 `trash`(MIT/Apache-2.0,登记
distribution.md §6);`destructive_confirm_dialog` 确认;打开中文件的口径记
decisions-pending #144(clean → 关标签后删;dirty → 拒绝并提示先保存或关闭)。
测试经注入的 remover 闭包(trash 不进无头测试,避免污染用户废纸篓/CI 环境)。

**在文件管理器中显示**:Linux `xdg-open` 父目录(无跨桌面选中标准)/ macOS `open -R` /
Windows `explorer /select,`。spawn 后即忘。

### M3 预览与 Live 富块:链接 / 图片右键

vendor ①类(独立 commit,可 cherry-pick 提上游):`LinkHandler` 增两个默认无操作方法
`link_response` / `image_response`,`layout.rs::link` 与 `label.rs` images 臂把 Response
透传。app 侧 `AiLinkHandler` 实现之:clone + `context_menu`——
打开链接 / 复制链接地址;图片:在浏览器打开 / 复制图片地址。
`emoji://` `task://` `hl://` 等**协议链接不弹菜单**(不是链接是字形/checkbox,与 click
吞掉口径一致)。

### M4 标签栏菜单补充

既有 #37 菜单尾部追加:复制路径 / 复制相对路径 / 在文件管理器中显示(复用 M2 的消息与实现)。

## 4. 消息与状态增量(全部落 `state.rs` 归约)

- `EditAction`(Undo/Redo/Cut/Copy/Paste/SelectAll)→ `Message::EditAction`,归约推合成
  事件 + 把焦点还给当前编辑目标(源码=editor_id;Live=活动块 id;归约在 logic() 阶段,
  同帧 ui() 的 TextEdit 消费——F3 时序)
- 文件树:`FileTreeRenameRequested/Confirmed/Cancelled`(带 rename 草稿)、
  `FileTreeDeleteRequested/Confirmed/Cancelled`(带确认模态状态)、`FileTreeNewFolderRequested/Confirmed/Cancelled`、
  `FileTreeCopyPath{path, relative}`、`FileTreeRevealInFileManager(path)`、`FileTreeOpenRequested(path)`
- 标签栏:M4 复用 `FileTreeCopyPath` / `FileTreeRevealInFileManager`(消息名以文件树命名,
  语义就是「对某路径做 X」)
- 预览:链接/图片菜单发 `Message::CopyText(String)`(泛化 copy_text)与
  `Message::OpenExternal(String)`(复用既有外开通道,若无需新增)

## 5. 验收(无头为主,真机目视留清单)

1. **M1 菜单构建**:无头帧中挂 context_menu,probe Ui 断言条目集合/禁用态随「有/无选区」「源码/Live」正确(tabs.rs 测试同手法)
2. **右键落光标**:推 secondary click 事件于指定坐标,断言 `tab.selection` 塌缩到对应字符;落点在选区内时选区保留
3. **剪贴板动作**:注入式粘贴文本源(不读真实剪贴板);reduce(EditAction::Copy) 后 `platform_output.copied_text` 含选区文本;Cut 后缓冲缩短且 copied_text 一致;Paste(注入文本)后缓冲插入且光标在插入尾;Undo 后回到操作前文本(`builtin_undo_redo` 测试同手法)
4. **M2 重命名**:tempdir 建文件→改名→磁盘断言;撞名/非法名拒绝且磁盘不动;打开中标签换绑新路径、缓冲保留;named_draft 迁移
5. **M2 删除**:spy remover 断言目标路径;dirty 拒绝文案;clean 关标签;确认模态取消零磁盘动作
6. **M3 vendor**:vendored 侧单测断言 link_response/image_response 被以正确 (text, href, response) 调用;app 侧断言协议链接不弹菜单
7. **M4**:扩 context_menu_items 测试断言新条目与响应
8. 真机人工清单:右键菜单观感(位置/样式/深浅主题)、Linux/Win/mac 三平台粘贴与「在文件管理器中显示」实测、Live 非活动块右键不弹的手感

## 6. 明确不做

- 编辑器右键塞全量 44 条命令(菜单栏/QuickOpen 已承担全量检索)
- 大纲面板、minimap、gutter 的右键菜单(无用户预期)
- Git 面板右键(P2 已交付只读集成,菜单随其后续需求另议)
- 预览只读区「复制选中文本」(galley 不可选,无选区可言;Live 活动块的复制走 M1)
- Live 非活动块右键直达菜单(先激活再右键,v1 口径)
