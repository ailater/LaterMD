# 右栏对照预览退役：能力并入 Live 写作渲染

> 状态：**P0–P6 全部完成**（2026-10-15；目标句按下文「三态回归」补充订正）
> 目标：把右栏「对照预览」的独有能力补齐进 Live 写作渲染，然后**彻底删除右栏的第二条渲染链路**，全项目只剩一条富渲染路径。
> **订正（2026-10-15）**：右栏面板以「三态视图的分栏档」形式回归（#250/#252），渲染的仍是 **Live 富渲染本身**（只读），
> 故 P4 删掉的「第二条渲染链路」这个事实没有被推翻；本轮 P6 的文档一律按此口径书写。
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
| P6 | 文档与 CHANGELOG | 完成 | 2026-10-15 | [#254](https://github.com/ailater/LaterMd/pull/254) |

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

- **P6 已落地(纯文档,零代码改动)**:口径按上一条「三态重述」执行 —— 不再写「对照预览已退役」,一律写「对照预览 = 三态视图的分栏档(只读 Live 渲染)」。①`docs/writing-source-modes.md` 整篇重写(三档关系表 + 互斥/循环/持久化/只读/跟随一次/禅定六组规则 + 验证范围);②`AGENTS.md` 铁律三→**四**,新增「单一富渲染路径」(把 P4 删第二条渲染链路的成果固化成约束力);③`README.md` / `docs/README.md` 入口描述改三态;④`CHANGELOG.md` 新增 `## Unreleased` 小节(三态 + 能力并入 + 命令精简 + 瘦身);⑤`docs/menu-coverage.md` 删 `ToggleRightPreview` 行 + 「视图三条」改两条 + Alt 系七条改六条;⑥`docs/ui-shell-redesign.md`(Panel 排布代码块改 `COMPARE_PANEL_ID` 条件式 + 图标表删 `PanelRight` + 键位表 `toggle_right_preview`→`toggle_live_preview` + §8 右预览节加后记 + token 用途改写);⑦`docs/adr-005-layout-and-sidebar.md`(面板顺序表第 1/5 行改六钮与仅分栏档);⑧`docs/roadmap.md` 两处三栏/双栏口径加订正行;⑨`docs/m5-acceptance.md` P4 后记补「三态回归 ≠ PanelRight 回归」;⑩`docs/image-plan.md` / `docs/emoji-plan.md` 把「`ui/preview.rs` / `preview.rendered`」的口径换成 Live 富渲染块(`rich_block_with_base` 第 5 层 / 第 3 层),并订正「Live 维持黑白 emoji」这句已被 P1/P2 推翻的旧结论;⑪`docs/ui-polish.md`(token 用途 + ASCII 图 + `ai_link_color` 宿主改 `ui::ai_card`);⑫`docs/preview-typography-and-keymap-plan.md` 键位表删行 + 加订正;⑬`docs/auto-plan.md` 加后记说明历史行里的 `ToggleRightPreview` 与「Alt 系七条」是当时事实。**decisions-pending.md**:#78/#79 加「载体迁到 Live」订正、#80 整条作废、`#115` 交付面改「右栏只读无选区」、`#121/#122/#129` 的 Alt 层枚举去掉 R、勾选态七条改六条(预览栏→切换视图),并新增 **#149(三态互斥)/ #150(右栏只读硬锁)/ #151(跟随一次)/ #152(分隔线走 `panel_divider` 统一入口)/ #153(R2 缓存抖动挂账)** 五条。
- **P6 未做(如实登记)**:`docs/ui-design.md` 与 `docs/ui-shell-redesign-v2.md` 里的「预览」字样是**当时截图取证与设计规范**的历史记录(三态下右栏仍在、分区配色口径不变),改写只会伪造历史,**未动**;两份文档若将来重写需按三态口径重述。
- **R1–R7 至此全部销账**(R1 P1 实测达标 / R2 转 #153 挂账 / R3 P3 守门测试 / R4 P1 入缓存键 / R5 P5 单独 PR / R6–R7 P4 裁定无副作用)。

- **【补充 2026-10-15】右栏对照预览已按坤哥指令回归为「源码+对照」三态视图**(两个 PR,均非 vendor):[#250](https://github.com/ailater/LaterMd/pull/250) 让 `live::ui` 可被两列各自调用(`ScrollArea` id 参数化 + `TabState::live_right` 独立块状态与独立 id 命名空间),[#252](https://github.com/ailater/LaterMd/pull/252) 落三态(`RenderMode::Split` + `cycle()`、标题栏三段开关、右栏只读面板、现代版分隔线)。**右栏渲染的是 Live 富渲染本身**,不是另一套渲染器 —— P4 删掉的仍是「第二条渲染链路」这个事实本身,本次回归没有重新引入它。P6 的文档清单因此要改写:不再写「对照预览已退役」,改为「对照预览 = 三态视图的分栏档(只读 Live 渲染)」;decisions-pending 里与右栏相关的条目(#78/#79/#80/#115/#121/#122/#129)按三态重述,并新增三态视图的决策条目(互斥三态 / 右栏只读 / 跟随一次 / 分隔线回归)。

- **【修复 2026-10-16】三态视图真机三回归收编([#255](https://github.com/ailater/LaterMd/pull/255),非 vendor)**:①**对照列宽度拉不动**——双根因:(a)egui 0.36 侧栏面板按**内容 min_rect** 定实际宽,而右栏为隔离「ScrollArea 撑高父 Ui」套的固定矩形子 Ui(`new_child`)min_rect **不回传帧内容层**,面板缩到 `size_range` 下限并被 `remember_panel_width` 持久化、松手帧又按内容宽存回 ⇒ 拖动无效;修法 = 面板内容层 `set_min_width` 撑满一行(无头探针:撑满 420 / 不撑 260)。这是个**新通则:egui 0.36 里「固定矩形子 Ui」与「Panel 撑满」不可兼得,Panel 内容必须显式撑宽**。(b)恢复拖动命中半径局部抬高 3.0→6.0(出厂数值实测找不到可拖处);新守门测试按下点取边界外 5.5px(6px 带内、出厂带外),tokens=3 红 / =6 绿双向验证。②**标题栏 PanelRight 键缺失**——恢复 7 键布局,`TITLE_BUTTON_N` 加**编译期断言**钉死与 `TITLE_BUTTONS` 同步(P4 删按钮时 tokens 漏改、原注释声称的断言实际不存在);`TITLE_TEXT_MIN_W` 264→252 保 900px 命令箱。③**三段开关点击语义**——点段改 `Message::SetRenderMode` 直达目标档(点谁到谁),循环仍走 Ctrl/Cmd+/;另补 `Command::ToggleCompare`(Cmd/Ctrl+Alt+R + 视图菜单)给标题栏按钮做对照列开合入口。

- **【修复 2026-10-16】关闭对照列的滑出动画回归(PR #258,非 vendor)**:PR#252 把右栏从旧的条件 `show_collapsible` 改成条件 `show`,开合变瞬移。现恢复常显 + `show_collapsible`:工作 bool 每帧从 `render_mode == Split` 播种,egui 在拖拽收起/双击边缘/收起态把手拖开时翻转它,翻转经 `Message::CompareColumnToggled` 收敛回 `render_mode`(`render_mode` 仍是唯一状态源)。守门测试两条:`compare_column_slides_closed_over_frames`(切换后首帧列仍在、逐帧右移退场,条件 `show` 必红)+ `compare_column_drag_past_min_collapses_into_source_mode`(drag-to-collapse 收敛回源码档,摘掉收敛推送必红)。

- **【补缺 2026-10-16】P4 删除盘点后补回最后两件(非 vendor)**:①列头 `pane_heading` 回归(「对照」+「只读」徽标,28px 固定在滚动区外;文案按三态身份改叫「对照」,不再叫旧「对照预览」);②「分隔线拖动中跟指针走」守门测试恢复(旧 `preview_divider_follows_the_drag` 的等价物,`panel_divider` 拖动中取 `interact_pointer_pos` 的三段锚点由此钉住 —— 2026-10-13「线不跟手」真 bug 的回归防线)。两条均红绿闭环(`compare_column_has_readonly_pane_heading` / `compare_divider_follows_the_drag`)。至此 P4 删除清单逐项盘点完毕:代码与测试面全部回归或被三态设计有意识取代,无其余缺口。

- **【修复 2026-10-16】分隔线压弹窗(层序,非 P4 缺失而是历史旧病)**:`panel_divider` 原先画在独立 `Order::Middle` 散层,而 egui 出同一 Order 形状时先排**已注册 Area**(全部 `egui::Window` 弹窗)、非 Area 散层兜底追加在最后 ⇒ 线横穿设置/关于等一切弹窗(坤哥截图实测;P4 前的旧 `preview_divider` 同构,属历史旧病)。修法 = 改画**调用方所在层**(根层,提交序:调用点 ⑥ 在中央面板内容之后 ⇒ 盖得住面板;弹窗是注册 Area ⇒ 永远在根层之上)。守门 `compare_divider_paints_below_dialogs` 断言形状流序(分隔线下标 < 弹窗标题下标;egui Window 首帧是 sizing pass 不出形状,须跑两帧取证),独立散层实现必红。

- **【核验 2026-10-20】对 main `c2c2fe7`(#281,距上次核验 #279 又 3 个提交)重验七阶段不变量,全部成立 ⇒ 无阶段可推进**。`ui::preview::` / `pub mod preview` 零命中;`ToggleRightPreview` 只剩 `command.rs` 两条注释 + 三份历史文档(`docs/README.md`、`docs/auto-plan.md`、`docs/decisions-pending.md` —— 按「历史记录只加后记不改原文」保留);`RenderMode` 三态齐(`live.rs:91`),`TabState::live_right`(`tabs.rs:126`)+ `LiveState::browse_locked`(`live.rs:135`)+ `layout.rs` 的 `live_right_width` 在位;`MarkdownLabel` 生产调用点只在 `live::ui`(其余命中是 benches / examples / 验收 harness,非生产);`AGENTS.md` 铁律四含「单一富渲染路径」;main CI #281 success。**R1–R7 全部销账、阻塞为空、文档里没有 P7 ⇒ 本轮不造阶段、不改判据。**

- **【清理 2026-10-21】`ui/preview.rs` 死链注释清理完毕(纯注释,零行为变化)**:上一条遗留的二选一按**方案①就地改口径**执行(无人值守轮次取默认:保留溯源语义、指向现存真实宿主,不丢信息)。实际清 **11 处**(原登记 10 处 + 复核新发现 3 处中已确认的 2 处 —— 见下):`live.rs` 4 处(1302 去「与预览 scroll_probe_id 同款」/ 2531 `preview::ScrollProbe`→`LiveScrollProbe` / 3608 →「本模块测试共用的取证 helper」/ 3621 →`ui::editor_menu::click_frames` 同手法)、`preview_typography_acceptance.rs` 1 处(→生产入口 `live::ui` + `editor_id.with(("live-render", index))`)、`font_metrics_repro.rs` 1 处(**复核新发现**、`preview::ui`→`live::ui`)、`ui/minimap.rs` 2 处、`ui/editor_menu.rs` 1 处、`ui/tab_switch_perf.rs` 2 处、`ui/editor.rs` 1 处、`ui/mermaid.rs` 1 处(→`ui::ai_card::is_instruction_info`)、`doclink.rs` 1 处(**复核新发现**,「live/preview 的 handler」→「live 的 handler」)。**两处保留不动**:①`ui/ai_card.rs:4`「原 `preview::AiLinkHandler`(右栏 handler,已随之退役)」—— 它自述退役、是继任模块的来历说明,属历史记录,按「历史只加后记不改原文」保留;②`latermd-export` / `latermd-md` / `fonts.rs` / `theme.rs` 里 ~20 处**泛指的「与预览一致 / 预览侧」**—— 它们是「渲染侧同口径」的语义简写,不指向任何已删文件,改写面大且会牵动导出与字体的既有论证,**登记为后续议题,不在本轮动**。
- **【核验 2026-10-23】对 main `97f7942`(#323,距上次核验 #306 又 34 个提交)重验七阶段不变量,全部成立 ⇒ 无阶段可推进**。重验的必要性:这 34 个提交里 #317/#318/#319/#321/#322/#323 连续六条动过 `live.rs` 与 vendored `label.rs`(表格原位编辑 / 表格尺寸 / 工具条三连),上一轮的「单一富渲染路径」结论不能直接沿用。六项:`ui::preview::` 与 `pub mod preview` 均零命中;`ToggleRightPreview` 只剩 `command.rs:122/560` 两条注释;`RenderMode` 三态齐(`live.rs:112`);右栏每帧三置位在 `layout.rs:1471-1474`(`browse_only` / `!browse_enabled` / `browse_locked`);`MarkdownLabel` 生产调用点只在 `live::ui`(`ui/mermaid.rs:347` 是卡内嵌套回落,仍属同一路径);`AGENTS.md:58` 铁律四含「单一富渲染路径」。main CI #323 success。**R1–R7 全销账、阻塞为空、文档里没有 P7 ⇒ 本轮不造阶段、不改判据。**
- **【裁定 2026-10-23】2026-10-21 登记的唯一在册遗留正式关闭,不排期**:`latermd-export` / `theme.rs` / `fonts.rs` / `latermd-md` 里 ~20 处**泛指的「与预览一致 / 预览侧」措辞不是缺陷**。三态回归后「预览」指的就是分栏档右栏,而右栏渲染的正是 `live::ui`,这些措辞语义仍然成立、不指向任何已删文件;改写只产生噪音,且会牵动导出与字体两条既有论证链。**不改动。**若将来三态命名再变,随那一次改名一并处理。**至此本计划的全部在册工作归零** —— 七阶段完成、七个风险销账、零遗留、零阻塞,后续如需继续推进须由人工新增阶段(建议指向 `docs/decisions-pending.md` 的 #153 R2 缓存抖动,或另起新计划)。
- **【核验 2026-10-24】对 main `9c18b81`(#337,距上次核验 #323 又 30 个提交)重验七阶段不变量,全部成立 ⇒ 无阶段可推进**。重验的必要性:这 30 个提交里有两处正踩在本计划的成果面上 —— ①#336 `67af613` 动过**标题栏**(三态开关与 `TITLE_BUTTONS` 的宿主,P4/P6 的成果区);②#335 `d345f55` 大纲指纹闸门动过 **P5 瘦身后保留的 `PreviewState::outline` 同步位**。六项(对 `origin/main` tree 只读核验):`ui::preview::` 与 `pub mod preview` 均零命中;`ToggleRightPreview` 只剩 `command.rs:122/560` 两条注释;`RenderMode` 三态齐(`live.rs:112-119`);右栏每帧三置位在 `layout.rs:1482-1484`;`AGENTS.md:58` 铁律四含「单一富渲染路径」。**本轮多验的三处**:①`titlebar.rs:46` 仍 7 键且 `:58` 的编译期断言(与 `TITLE_BUTTON_N` 同步)未被 #336 破坏;②`MarkdownLabel` 生产调用点 = `live.rs` 四处 + `ui/mermaid.rs:347`(卡内嵌套回落,不传 link_handler,走 vendored 普通代码路径,仍属同一路径)—— `mermaid.rs` 的 530/770/814/868/944 均在 `:439 mod tests` 内,非生产;③`PreviewState` 仍是 P5 瘦身形态(生产侧 `outline` / `outline_rev` / `synced_rev`,`text` 仅 `#[cfg(test)]`),#335 新增的 `outline_fingerprint` / `outline_shown` / `outline_rescans` 是闸门与取证位,均非死代码(`outline_rescans` 被 `state.rs:5138` 起的守门测试依赖)。**R1–R7 全销账、阻塞为空、文档里没有 P7 ⇒ 本轮不造阶段、不改判据。**
- **【方法 2026-10-24】核验必须对准 `origin/main` tree,不能对准主工作区**:开工时本地 `origin/main` ref 停在 `aa315bf`(#335)而远端已到 `9c18b81`(#337),差 3 个提交 —— 判据是 `gh run list --branch main` 的 `headSha` 与 `git rev-parse origin/main` 对不上(本轮 40105043491 的 headSha 是 `9c18b81`,本地 ref 是 `aa315bf`)。先 `git fetch --prune` 再核验;只读核验一律走 `git grep <pattern> origin/main -- crates/` 与 `git show origin/main:<path>`(不切分支、不动工作区)。**主工作区有他人 WIP 时连 `git fetch` 都要谨慎**(会让 index 与 HEAD 不符),无 WIP 时 fetch 是安全的。
- **【遗留 2026-10-20,非阻塞,已清理】`crates/` 里曾有 10 处注释把已删的 `ui/preview.rs` 当「同款参照」**:`live.rs` 2 处(2864 / 2877)、`preview_typography_acceptance.rs` 1 处(126)、`ui/editor.rs` 1 处(806)、`ui/editor_menu.rs` 1 处(268)、`ui/mermaid.rs` 1 处(40)、`ui/minimap.rs` 2 处(264 / 593)、`ui/tab_switch_perf.rs` 2 处(5 / 122)。**根因**:P5 删模块、P6 清的 5 处死链是 **rustdoc 链接**(`[`...`]` 型),扫的是文档面,没扫「照 `ui/preview.rs` tests 同款」这类**参照系注释** —— 它们是过程信息(「当年抄自哪里」),不是事实陈述,删模块后就成了指向不存在文件的路标,会误导后来者。**建议两个方案**(本轮未动代码):①**就地改口径** —— 把参照从句改指向现存真实宿主(`live.rs` 自身的测试手法 / `ui::rich_widgets` / `ui::ai_card`),语义信息零丢失,8 个非 `live.rs` 文件可立即清;②**只删不替** —— 认定「抄自哪里」不是读者需要的信息,直接删掉从句,改动最小但丢一条溯源。**本轮不动 `crates/` 的理由**:开工时主工作区有他人未提交 WIP(`live.rs` +463/−2、`latermd-md/src/lib.rs` +11,含 `lp25_probe` 调试模块),而 `live.rs` 正是死链文件之一,改它必与在途改动抢 hunk —— 按共享工作区纪律让路,先登记等裁决。
- **【核验 2026-10-25】对 main `1138e91`(#343,距上次核验 #337 又 13 个提交)重验七阶段不变量,全部成立 ⇒ 无阶段可推进**。重验的必要性:这 13 个提交里 **#339 `7f8117d`「mermaid 块『图表/源码』切换与预览只读(#164)」直接改了本计划的成果面** —— 它给 `ui/mermaid.rs` 新增了**「源码视图」这条二级渲染分支**,上册「`MarkdownLabel` 生产调用点 = `live.rs` 四处 + `mermaid.rs:347` 一处」的行号与构成都已变,必须重扫而不是沿用。六项(一律对 `origin/main` tree 只读核验):`ui::preview::` 与 `pub mod preview` 均**零命中**;`ToggleRightPreview` 只剩 `command.rs:122/560` 两条注释;`RenderMode` 三态齐(`live.rs:112-120`,`cycle()` 在 `:125`);`TabState::live_right`(`tabs.rs:126`)+ 右栏每帧三置位在 `layout.rs:1482-1484`;`AGENTS.md:58` 铁律四含「单一富渲染路径」。**本轮多验的四处**:
  1. **`MarkdownLabel` 生产调用点重扫后仍是同一条路径**:生产构建里只剩 `live.rs`(`:1946` / `:3017` 估高占位、`:2801` 主渲染;`:4312` 起是 `mod tests`)与 **`ui/mermaid.rs:442`** —— 后者是 #339 新增的 `paint_source`(定义在 `:429`),比上册的 `:347` 后移且**语义换了**:不再只是解析失败回落,而是「用户切到源码视图」的常驻分支。**为什么仍不构成第二条渲染链路**:调用链是 `live::ui` → `RichLinkHandler`(`live.rs:1935/2771/3002` 构造)→ `mermaid::block_widget`(`ai_card.rs:452` 是唯一生产调用点)→ `paint_source`;`paint_source` 刻意**不传 link_handler** ⇒ `is_block_code_widget` 恒 false ⇒ 走 vendored 普通代码块路径、天然不可能递归回 mermaid widget。它是挂在单一路径内部的**叶子 widget**,不是并列通路;`paint_diagram`(自绘矢量)同样只经 `block_widget` 进入。
  2. **`live::ui` 生产调用点仍是三处**,与三态一一对应:`ui/editor.rs:245`(Live 档)、`ui/layout.rs:1365`(禅定)、`ui/layout.rs:1532`(分栏档右栏)。无第四处消费者。
  3. **新增/留存的 harness 全部是 `#[cfg(test)]`**,不构成生产面:`ui/perf_finding.rs`(`ui/mod.rs:39`,`#59` 取证)、`ui/screenshot_gen.rs`(README 截图)、`ui/tab_switch_perf.rs`;顶层 `font_metrics_repro` / `preview_pixel_acceptance` / `preview_typography_acceptance` 亦均为 `main.rs` 里的 `#[cfg(test)] mod`;`benches/longdoc.rs` 与 `examples/scrollbench.rs` 不入 binary。**铁律二顺带确认**:`egui_markdown::MarkdownLabel` 的全部命中仍只在 `latermd-app` 一棵 crate 内。
  4. **标题栏 7 键与编译期断言未被 #341 发版 / #342/#343 dmg 改动破坏**:`ui/titlebar.rs:46` 仍 `[TitleButton; 7]`、`:58` 的 `const _: () = assert!(TITLE_BUTTONS.len() == tokens::TITLE_BUTTON_N)` 在位;`PreviewState` 仍是 P5 瘦身形态(#335 新增的 `outline_fingerprint` / `outline_shown` / `outline_rescans` 是闸门与取证位,非死代码)。

  main CI 对 headSha `1138e91` 的**四条 workflow run 全 success**:`Rust`(六项门禁)/ `Mirror to CNB` / `Auto Tag` / `macOS dmg`。附带事实:上一个 head `458dad5`(#342)的 `macOS dmg` 曾 failure,正由 #343 `c4de7af`「组包改显式分步,绕开 `hdiutil -srcfolder` 估容缺陷」修掉 —— 本计划不涉及 dmg 组包,仅登记该红已在同一天闭合。**R1–R7 全销账、阻塞为空、文档里没有 P7 ⇒ 本轮不造阶段、不改判据。**
- **【观察 2026-10-25,非缺陷、不入遗留】#339 做出了第一个「块内二级视图」先例**:此前 block widget 的形态是「要么自绘图表、要么回落源码」,两者不由用户来回切;#339 后 `source_view` 成为 mermaid 块的持久二态(视图态由 `source_view_key(ui, index)` 派生 id 存在 ctx 里)。对本计划的含义:**它不在「单一富渲染路径」的判据内**(路径仍只有 `live::ui` 一条),但给将来任何「块形态切换」的改动留了统一手法 —— 复用 `mermaid::source_view_key` 这种「同一 `ui` + 同一 index 派生稳定 id」的做法,不要新造第二套块级状态存放机制。本轮仅登记,不排期。

- **【核验 2026-10-26】对 main `2cdbde1`(#351,距上次核验 #343 又 8 个提交)重验七阶段不变量,全部成立 ⇒ 无阶段可推进**。重验的必要性:这 8 个提交里 **#348 `7fdaa70`「mermaid 块换代码块同款外框」直接重写了本计划的成果面** —— 它把 `paint_source` 从「嵌套 `MarkdownLabel` + `code_block_buttons`」改成「`highlight_code` + galley 直铺」,上一轮登记的「`MarkdownLabel` 生产调用点含 `mermaid.rs:442`」**已不再成立**,必须重扫而不是沿用。六项(一律对 `origin/main` tree 只读核验):`ui::preview::` 与 `pub mod preview` 均**零命中**;`ToggleRightPreview` 只剩 `command.rs:122/560` 两条注释;`RenderMode` 三态齐(`live.rs:112-120`,`cycle()` 在 `:125`);`TabState::live_right` + 右栏每帧三置位在 `layout.rs:1482-1485`;`titlebar.rs:46` 仍 `[TitleButton; 7]` 且 `:58` 编译期断言在位;`AGENTS.md:58` 铁律四含「单一富渲染路径」;`PreviewState` 仍是 P5 瘦身形态。main CI 对 headSha `2cdbde1`(`Rust` run 40266896259)success。
- **【订正 2026-10-26】单一富渲染路径进一步收紧 —— `MarkdownLabel` 现在**只在 `live.rs` 一处**:#348 后 `ui/mermaid.rs` 的 `MarkdownLabel` 命中**全部落进 `:685 mod tests`**(`:690` import 及其后 8 处),生产侧 **零命中**;`paint_source` 改走 `egui_markdown::layout::highlight_code`(即 vendored 代码块正文的高亮管线本身)+ `painter.galley` 直铺,不再嵌套第二层 label。**这对本计划是加强而非削弱**:结论从「生产调用点 = `live.rs` 三处 + `mermaid.rs` 一处叶子 widget」变为「生产调用点 = `live.rs` 三处(估高占位 ×2 + 主渲染 ×1),另有零处叶子 widget」。2026-10-25 登记的那条判据(「看能否绕过 `live::ui` 抵达」)因此**不再需要动用** —— 这次连嵌套都没有了。
- **【清理 2026-10-26】#348 遗留的 4 处过时「嵌套 `MarkdownLabel`」描述已订正(纯注释 + 一处容差说明,零行为变化)**:①`ui/mermaid.rs:487` 段头注释「共用嵌套 label 渲染」;②同文件 `paint_source` 的 rustdoc 首段(它一边说「嵌套 `MarkdownLabel` 渲染」、一边在末段说「不再嵌套 MarkdownLabel」,**自相矛盾** —— 现统一为「galley 直铺 + 复制钮挂载点在外框头带」);③`ui/mermaid.rs:1018` 测试注释「复制按钮经嵌套 label 的 `code_block_buttons` 挂载」;④`live.rs:7233` 同口径注释。**为什么值得改**:#348 把复制按钮的挂载点从「嵌套 label 的 `code_block_buttons`」搬到了外框 `block_widget` 头带的 `code_header_buttons`,但注释仍写旧路径 —— 后来者按注释去 `paint_source` 找复制钮会找不到。两条断言(`ev.copy_buttons` 非空)之所以仍绿,正因为探针取的是**头带那一枚**,注释指的路与断言实际走的路不一致,属典型「注释先于实现腐坏」。附带订正同处「头带 24px」→「[`HEADER_HEIGHT`] 28px」(#351 已改值,断言的 26px 是容差不是硬值)。
- **【核验 2026-10-28】对 main `2524eb7`(#354 v0.0.13,距上次核验 #351 又 3 个提交)重验七阶段不变量,全部成立 ⇒ 无阶段可推进**。重验的必要性:这 3 个提交里 #353 `c97ded6` 动 `latermd-export/src/pdf/cjk.rs`(CJK 字体候选表)、#354 `2524eb7` 是发版(版本 + CHANGELOG + README),**都不碰本计划的成果面**,但跨版本核验是本自动化每轮的固定动作。六项(一律对 `origin/main` tree 只读核验):`ui::preview::` 与 `pub mod preview` 均**零命中**;`ToggleRightPreview` 只剩 `command.rs:122/560` 两条注释;`RenderMode` 三态齐(`live.rs:110-119`,`cycle()` 在 `:123`);`TabState::live_right` + 右栏每帧三置位在 `layout.rs:1350`(禅定)/`:1484`(分栏右栏);`MarkdownLabel` 生产调用点仍是 `live.rs` 三处(`:1946` / `:2810` / `:3026`),`ui/mermaid.rs` 的命中全在 `:688 mod tests` 之后 ⇒ 生产侧**零命中**(与 2026-10-26 订正一致);`live::ui` 生产调用点仍是 `editor.rs:245` / `layout.rs:1365` / `layout.rs:1532` 三处;`titlebar.rs:46` 仍 `[TitleButton; 7]` 且 `:58` 编译期断言在位;`AGENTS.md:58` 铁律四含「单一富渲染路径」;`PreviewState` 仍是 P5 瘦身形态。**R1–R7 全销账、阻塞为空、文档里没有 P7 ⇒ 本轮不造阶段、不改判据。**
- **【订正 2026-10-28】扫出两类「注释仍在描述三态前的旧形态」,均非泛指措辞、属事实性过时(纯注释订正,零行为变化)**:
  1. **把「整篇单标签的右栏」当现役实体** —— `live.rs` 的 `block_gap` rustdoc 首句原写「镜像 vendored 单标签流的块间节奏(**右栏预览即此口径**)」、次句「**预览把整篇塞进一个** `MarkdownLabel`」。这两句是 2026-10-10 的真事实,但三态视图后右栏渲染的**就是 `live::ui` 的块级标签**(与左栏同一个 `block_gap`),项目里已没有「整篇单标签那一栏」可对照 —— 后来者按此注释会误判「右栏另有一条整篇渲染路径」,**直接冲撞铁律四的判据面**。订正:参照物改指向 vendored `MarkdownLabel` 的**整篇用法**(即 `block_gap_mirrors_the_single_label_rhythm` 里自建的基准标签),并加一段订正注说明旧句为何作废。连带订正同测试的 `out2` 段(注释「两栏的标签」→「整篇基准 vs Live 块级标签」、`.expect("预览单标签里有该段行")` 与断言消息)、`emoji_probe_id` 的「与预览 `emoji_probe_id` 同款」(那份已随 P5 删)。
  2. **「P4 已退役」口径违反 P6 定稿** —— `ui/rich_widgets.rs` 的 `TaskProbeDomain::Preview` rustdoc 写「右栏预览(**P4 已退役**,仅测试路径保留)」,而 P6 定稿口径明确要求「一律写『对照预览 = 三态视图的分栏档』,**不写『已退役』**」。订正:前半句改为「整篇单标签形态下的全局坐标」+ 订正注(右栏未退役、本变体确实只被 `ui/mermaid.rs` 测试使用 —— 后半句仍成立);同模块首句「右栏预览与 Live 写作渲染**两条路径**都用的部分」同样作废(三态后只有一条路径),改口径并加订正注;另 `state.rs` 大纲点击处的「右栏预览退役」改为「整篇改写链与悬置滚动目标随 P5 删除」并补口径注。**保留不动**:`state.rs:249/259` 讲「整篇改写链只为右栏预览存在」是 P5 当时的删除论证,所指的「整篇产物」确实已不存在,准确。
- **【方法 2026-10-28,可复用】「泛指措辞不是缺陷」这条裁定有边界 —— 只豁免「与 X 同源/同款」型语义简写,不豁免「X 是 Y 形态」型事实陈述**。2026-10-23 裁定 `live.rs` 里「与右栏预览同源/同款」那批泛指不动,前提是右栏仍在且渲染的就是 `live::ui`,简写仍成立。但本轮扫出的两类**不是简写而是断言**:①「右栏 = 整篇单标签」(形态断言,三态后假);②「右栏已退役」(存废断言,P6 明令不许写)。判据:**注释里对某个实体说了「是什么形态 / 还在不在」的,随实现变;只说「与它同源」的,只要实体还在就成立**。扫描手法:grep 该实体名 + 形态词(`整篇` / `单标签` / `已退役` / `两条路径`),比只 grep 实体名命中率高得多。
- **【方法 2026-10-26,可复用】改动「某 widget 的渲染内部实现」时,要连带扫它自己的 rustdoc 首句是否会与新实现自相矛盾**:`paint_source` 的旧首句讲「嵌套 label」、末句补「不再嵌套」,是**同一次改实现只补了后半句没改前半句**的典型形态。`cargo doc` / clippy / 测试**全都不会报**(注释不是事实陈述的机器可检面),只能靠 grep 关键词(本例是「嵌套」)比对实现。判据:改完实现后 grep 该函数新旧两条路径的关键词,同现即腐坏。
- **【环境 2026-10-26】`gh auth status` 报 token 失效,但 `~/.git-credentials` 里的 token 仍可用**:走 `gh` 前先 `export GH_TOKEN=$(grep -oP '(?<=://[^:]*:)[^@]+' ~/.git-credentials | head -1)`(注意 fish 里 `${#VAR}` 语法不同,这条命令要在 bash 里跑)。`git push` / `git ls-remote` 不受影响(走 credential helper)。

- **【核验 2026-10-29】对 main `d68a26c`(#355)重验七阶段不变量,全部成立 ⇒ 无阶段可推进**。**特殊之处:本轮开工时 `origin/main` 与上一轮终点是同一棵树(0 个新提交)**,故重验的实质价值低于往轮,仍按固定动作做完。六项(一律对 `origin/main` tree 只读核验):`ui::preview::` 与 `pub mod preview` 均**零命中**;`ToggleRightPreview` 只剩 `command.rs:122/560` 两条注释;`RenderMode` 三态齐(`live.rs:112-120`,`cycle()` 在 `:123`);`TabState::live_right` + 右栏每帧三置位在 `layout.rs:1348-1350`(禅定)/`:1482-1484`(分栏右栏);`live::ui` 生产调用点仍是 `editor.rs:245` / `layout.rs:1365` / `layout.rs:1532` 三处;`MarkdownLabel` 生产调用点仍是 `live.rs` 三处(`:1956` / `:2820` / `:3036`),`ui/mermaid.rs` 生产侧零命中;`titlebar.rs:46` 仍 `[TitleButton; 7]` 且 `:58` 编译期断言在位;`AGENTS.md:58` 铁律四含「单一富渲染路径」;`PreviewState` 仍是 P5 瘦身形态。main CI 对 headSha `d68a26c`(`Rust` run 40621283671)success。**R1–R7 全销账、阻塞为空、文档里没有 P7 ⇒ 本轮不造阶段、不改判据。**
- **【订正 2026-10-29】按上一轮立的「存废断言」判据做全量扫描,又扫出 12 处事实性过时注释(纯注释,零行为变化)**。上一轮只订正了 `rich_widgets.rs` 一处就收尾;这次先 `grep -rn "退役" crates/` 拉全量再逐条分类,分两类:
  1. **P6 定稿口径违规 —— 把「已退役」挂在对照预览这个实体上,共 9 处**:`theme.rs:330`(「原右栏预览已退役」)、`layout.rs:26`(「右栏对照预览退役后只剩左栏一元」+ 中英混杂的「禅定的定义 itself」)、`state.rs:201` / `:259` / `:4954`、`ui/layout.rs:2736`(断言消息「右栏退役后是唯一邻居」)/ `:2747` / `:9715`(断言消息)、`ui/rich_widgets.rs:1134`。一律改写为「整篇单标签产物随 P5 删除 / 右栏开合归 `render_mode` 三态 / 非分栏档下没有对照列」,并显式带上「按 P6 定稿口径不写『已退役』」的口径注。
  2. **Alt 层键位占用注释把 R 说成「已随对照预览退役」,共 3 处**:`command.rs:547` / `:573` / `:583`。**事实是 R 根本没空出来** —— P4 摘除 `ToggleRightPreview` 后,同一键位立即由 `ToggleCompare`(对照列开合)占用(`:562`),该层现有 R/W/D/M/C 五个字母键。三处按当前真值订正(R = 对照列开合,不是空键)。
  **保留不动(退役主语=已删构件,准确;或属历史记录)**:`command.rs:122/560`(`ToggleRightPreview` 命令确已退役)、`layout.rs:72`(旧字段名 `right_width` 确已退役)、`ui/layout.rs:19`(旧面板 id `"preview"` 确已退役)、`rich_widgets.rs:4`(下方订正块已把整段框定为三态前的历史)、`ai_card.rs:4/13`(自述退役 + D1 决策记录)、`docs/decisions-pending.md:1269`(P2 岔路历史)。另 `state.rs:6970` 与 `layout.rs:243` 的 `right`/`right_width` **键**确已退役,但原句把退役主语写成了「右栏对照预览」,本轮把主语改回「旧右栏面板开合」并补「对照预览现由三态 `render_mode` 表达」。
- **【方法 2026-10-29,可复用】「存废断言」类注释要按实体逐条枚举,不能只订正扫到的第一处**。2026-10-28 立了判据却只修 `rich_widgets.rs` 一处便收尾,同一判据下还有 11 处同类。手法:**先 `grep -rn "退役" crates/`(或任意形态词)拉全量,再逐条问「这句的退役主语是哪个实体」** —— 主语是**已删构件**(命令 / 字段名 / 面板 id / 整篇产物)的准确、不动;主语是**对照预览这个实体**的违规、必改。**上一轮把 `state.rs:249/259` 判为「保留不动」是同一疏漏,本轮一并订正**:保留的是「整篇产物已随 P5 删除」这个事实,订正的是「预览已退役」这个主语(按同一判据它属存废断言,不豁免)。

## 阻塞

（无。遇到编译不过 / 测试红 / 决策点时在此记录：问题现象、已排除的可能、建议的两个方案，等人工裁决。）

**非阻塞遗留**：~~上一节末条登记的「`crates/` 里 10 处 `ui/preview.rs` 死链注释」（2026-10-20）~~ —— **已清，见下条执行备注（2026-10-21）**。原待裁决项「改口径 / 只删不替」按**方案①就地改口径**执行（无人值守轮次自行取默认并声明）。

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
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo doc --no-deps --all-features
```

加上 P1/P2/P3 的红绿验证：临时改坏实现，确认新守门测试真红。

> **踩过的坑（2026-10-23 实测）**：三轮 clippy 的 `-D warnings` **必须经 `--` 传给 clippy**，
> 写成 `cargo clippy --workspace --all-targets -D warnings`（无 `--`）时 cargo 会自己解析 `-D`
> 并报 `error: unexpected argument '-D' found`（Usage 提示的是 `cargo check`），**rc=1 但没有任何
> 真实 lint 告警** —— 容易被误读成「门禁红了」。CI 的 `.github/workflows/rust.yml` 一直用的
> 是带 `--` 的正确形式，照抄它。

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
