# Emoji 插入规划（emoji-plan）

日期：2026-09-27
状态：**待坤哥放行**（规划态，只写文档，不动 `src/`）
关联：[AGENTS.md](../AGENTS.md) §2 技术栈 / §5 风险项、[ui-polish.md](ui-polish.md) §1.1 图标自绘、
[ui-shell-redesign.md](ui-shell-redesign.md) §6 工具条、[image-plan.md](image-plan.md)（同为「对话框类动作」）、
[auto-plan.md](auto-plan.md) #28、[decisions-pending.md](decisions-pending.md) #4 依赖登记口径 / #36 数据源选型

> 坤哥 2026-09-27 指令：「编辑的工具栏加入一个 emoji 规划」。
> 本文先给出**三条实测硬事实**（决定了整个方案能不能省掉依赖），再拆成可排期的四段。

---

## 1. 一句话方案

编辑工具条末尾加一枚**自绘笑脸按钮** → 点开 Emoji 面板（搜索 + 分类 + 最近使用）→ 点选即在光标处插入**纯 Unicode 字符** `😀`。

**最大的好消息是字体不用管**：egui 0.36.2 默认字体链里**已经带了黑白 Noto Emoji**，不需要新依赖、不需要开 feature、不需要打包字体文件。

---

## 2. 三条实测硬事实（先看这个，它决定方案走向）

| # | 事实 | 实测出处 | 对方案的影响 |
|---|---|---|---|
| **F1** | egui 0.36.2 `FontDefinitions::default()` **已注册 `NotoEmoji-Regular` 与 `emoji-icon-font`**，且注释写明 emoji "Use as first priority"，排在 Proportional / Monospace 回退链里 | `epaint-0.36.2/src/text/fonts.rs:508-545` | **零新增依赖即可渲染 emoji**。我们的 `fonts.rs::install()` 是在 `FontDefinitions::default()` 之后 push CJK，emoji 已在链上，无需改动 |
| **F2** | **彩色 emoji 不支持**。epaint 把所有字形以白色写入字体图集（`FontColorTransferFunction::Off` 的注释只说"这是彩色 emoji 需要的模式"，但紧邻段落明说未实现、仍是白色图集 + shader 乘色） | `epaint-0.36.2/src/image.rs:350-373` | 应用内 emoji 是**黑白轮廓**。这不是 bug，是上游限制，必须写进用户可见说明。**#47 A2 更新**：面板/最近使用条改走随包 Twemoji PNG 纹理（见下「F2 的连带结论」），**编辑器正文仍黑白**（字形管线结论不变） |
| **F3** | 网上老教程让开 `monochrome_emoji_fonts` feature —— **该 feature 在 0.36.2 已不存在**（已被并入 `default_fonts`），照抄会编不过 | `epaint-0.36.2/Cargo.toml` features 段（只有 `default_fonts`，无 monochrome 项） | 排除一条错误路径；数据虽然还在 `epaint_default_fonts` crate 里，但**不需要**直接依赖它 |
| **F4** | `Fonts::has_glyph(&FontId, char)` / `has_glyphs(&FontId, &str)` 可用 | `epaint-0.36.2/src/text/fonts.rs:779-857` | 可以**主动探测字形缺失**，把渲染不出的 emoji 从面板里剔掉，而不是让用户看到豆腐块 |

### F2 的连带结论（必须让用户知道）

- **本应用内**：编辑器正文黑白线条 emoji（Noto Emoji 风格）。**#47 A2 起**：插入面板与「最近使用」条用随包 Twemoji 72px PNG 纹理显示**彩色**（`assets/emoji/twemoji/`，CC-BY 4.0，登记 distribution.md §6；损坏/缺失回落黑白，实现见 `ui/emoji_panel.rs`「纹理路径」节）。**#48 B1/B2 起**：右栏**预览内联** emoji 同走纹理彩色（`emoji://` 渲染副本改写 + LinkHandler inline_widget 两件套，见 §12；只预览，编辑器正文与 Live 模式仍黑白）。
- **导出的 HTML / 粘到微信、飞书、GitHub**：显示效果**取决于目标环境的字体**，不承诺必然彩色。
- Markdown 文件里存的永远是标准 Unicode 字符，**不存在兼容性问题**。
- 这条要在面板底部用一行小字说明（现行口径：「面板内彩色；编辑器正文仍黑白；导出/外发的显示效果取决于目标环境的字体」），否则用户会以为程序坏了。

---

## 3. 与 ui-polish §1.1 的边界（为什么这不算是破例）

§1.1 写的是「图标一律自绘，**emoji / Unicode 符号（✎ 🗋 ⌘）在三平台缺字风险真实**」。这条**仍然成立、继续遵守**——区别在于层次：

| 层次 | 用什么 | 本方案的做法 |
|---|---|---|
| **UI 图标**（工具条按钮、菜单项） | **自绘** `Painter`，零字体依赖 | ✅ 那枚表情按钮本身是**自绘笑脸**（圆脸 + 两点眼 + 弧嘴），不是 `😀` 字符 |
| **文档内容**（写进 `.md`） | 用户要的 emoji 字符 | ✅ 这是**内容**，不是图标。存的是标准 Unicode，字体只影响本应用内显示 |

一句话：**禁的是拿 emoji 当图标，不是禁往文档里插 emoji。** 工具条按钮自绘，插进去的才是 emoji。

---

## 4. 数据源选型（**待坤哥拍板**，见 decisions-pending #36）

| 方案 | 数据量 | 覆盖 | 搜索体验 | 成本 |
|---|---|---|---|---|
| **甲 · 内置精简表**（推荐） | ~300 枚 / 8 分类，中英双语名 + 短码，约 12KB 常量表 | 覆盖日常高频使用 | 够用（常用词都命中） | **零新增依赖**，不过 ADR-004 清单；手写维护 |
| 乙 · 引 `emojis` crate | 1MB 数据，1800+ 枚，Unicode v17 | 全量 | 最好（含 gemoji 短码） | 新增依赖（须按 decisions-pending #4 登记进 ADR-004）；`(MIT OR Apache-2.0) AND Unicode-3.0` 与 MIT 兼容；库本身健康（0.9.0 / 2026-06 发布 / 37 万下载月 / 144 crates 使用 / 纯数据无 unsafe） |

**推荐甲**，理由有三：

1. 编辑器里实际用到的 emoji 就那几百枚，全量 1800+ 里大半一辈子不会点。
2. **隐藏成本**：epaint 的字形是**渲染过才进图集**——用户一翻面板，整页 emoji 会把字体图集迅速吃满（F2 提到图集是有限纹理），全量数据反而更容易触发图集扩容/重排。
3. 少一个依赖就少一次 ADR 滚动修订，与「能抄数值的不引库」一致。

若坤哥选乙，需同步在 `docs/adr-004-technical-stack.md` 登记一行（版本 + 用途 + 许可证），并把 Unicode-3.0 的署名要求带进 `docs/distribution.md` 的第三方声明。

---

## 5. 分段（合计约 1.3d，甲乙两案工作量相同）

| 段 | 内容 | 工作量 | 是否可独立交付 |
|---|---|---|---|
| **E1** | 按钮 + 面板骨架 + 数据表：`Icon::Emoji` 自绘、工具条末尾挂按钮、新 `ui/emoji_panel.rs`（`egui::Window` 形态，与 `image_dialog.rs` 一致）、8 分类网格、点选插入 | 0.5d | ✅ 最小可用 |
| **E2** | 搜索框（中文名 / 英文名 / 短码三路匹配）+ 分类切换 + 「最近使用」一行（落 `settings.json`，与 `ThemeSettings` 同路） | 0.5d | 依赖 E1 |
| **E3** | 字体能力探测：`has_glyph` 在建表时过滤缺字形条目 + **三平台真机目视**（Linux / Win / mac 各一次） | 0.3d | 依赖 E1 |
| **E4**（可选） | `:gemoji:` 短码自动补全——输入 `:rocket:` 后跟空格自动转 🚀 | 0.3d | 独立 |

---

## 6. 接口形状（只定签名，不实施）

### 6.1 纯函数层 `compose.rs`

```rust
/// 在选区处插入 emoji,替换选中内容,新选区 collapsed 落在 emoji 之后。
/// 选区索引沿用本模块既定的 **char 索引**(`char_to_byte` / `byte_to_char`);
/// emoji 在 Rust 里是单个 `char`(含非 BMP),故天然安全。
pub fn insert_emoji(text: &str, sel: Range<usize>, emoji: &str) -> (String, Range<usize>);
```

**不进 `FormatAction`。** 理由：`FormatAction` 的语义是「纯文本变换」——给定 `text + sel` 就能推导出结果（加粗、加链接前缀、插表格骨架）。emoji 字符**无法从 text/sel 推导**，必须由外部（面板）提供，与 `Image` 同属「对话框类动作」。

> **顺带登记一个边界问题**（暂不改，供以后决策）：此前把 `FormatAction::Image` 放进 `FormatAction::ALL` 也偏勉强——它的骨架 `![](https://)` 虽可推导，但真实用法是走对话框。若 E1 之后工具条出现第三个对话框类动作，应考虑把这批从 `FormatAction` 里分出去（例如 `FormatAction` 只留纯变换，对话框动作在 `format_bar` 末尾单独渲染）。

### 6.2 图标层 `ui/icons.rs`

新增 `Icon::Emoji`：**自绘**——外圆（脸）+ 两个实心点（眼）+ 一段下弯弧（嘴）。遵守 ui-polish §1.1（零字体依赖、随主题取色、归一化坐标）。

### 6.3 命令与消息

| 项 | 取值 | 备注 |
|---|---|---|
| `Command::EmojiPicker` | `Cmd/Ctrl+Shift+E` | **需先查 keymap 冲突**；撞键则按 decisions-pending #9 口径「拒绝并指名占用者」，换一个；用户可在设置「快捷键」页改绑 |
| `Message::EmojiPickerToggle(bool)` | 开/关面板 | |
| `Message::EmojiInserted(String)` | 携带字符 | 归约侧调 `compose::insert_emoji` |
| `State.emoji` | `{ open: bool, query: String, group: usize, recent: Vec<String> }` | `recent` 持久化到 `settings.json` |

### 6.4 面板交互

- 形态：`egui::Window`（`.open()` / `.resizable(false)` / `.collapsible(false)` / `.title(false)`），与 `image_dialog.rs` 同一套写法，便于无头测试复用。
- 布局：顶部搜索框 → 分类横向标签 → 网格（8 列，每格 32×32）→ 底部「最近使用」。
- hover 显示名称 tooltip（可发现性）；点击插入后**关闭面板**（与多数编辑器一致；要连插多个时靠「最近使用」二次进入，成本极低）。
- Esc 关闭（走现有输入处理链路）。

---

## 7. 坑（预先登记，别等踩了再说）

| # | 坑 | 处理 |
|---|---|---|
| 1 | **黑白不是彩色**（F2） | 面板底部一行小字说明；导出/外发的显示效果取决于目标环境的字体。别当 bug 修。#47 A2 起面板本身已走 Twemoji 纹理彩色，但编辑器正文仍是黑白 —— 字形管线上游限制未变 |
| 2 | **Noto Emoji 覆盖落后于 Unicode 17**：2024 后新增的 emoji（如 🫩）在 `NotoEmoji-Regular` 里可能无字形 → 豆腐块 | E3 用 `has_glyph` 在建表时过滤；面板只在探测通过后才显示该枚 |
| 3 | **ZWJ 序列**（👨‍👩‍👧、🏳️‍🌈）是**多个码位**：插入后按一次 Backspace 只删最后一个组件 | 已知接受（主流编辑器同行为）。数据表中序列与单组件**二选一收**，避免重复占位 |
| 4 | **肤色 / 性别变体**会让条目数翻倍 | 只收默认肤色。不做长按展开变体 |
| 5 | **插入打碎 TextEdit 内建 undo**（`compose` §9 R3 同款，与 image-plan 同一条） | 已知接受，不重复论证 |
| 6 | **字体图集膨胀**：翻页即把字形渲进图集（见 §4 推荐甲的理由于 2） | 面板用分页/分类限制单屏条目数（每类 ≤ 40 枚），不全量铺开 |
| 7 | **`has_glyph` 依赖真实字体，单测里断言它会 flaky** | 无头测试**只断言**「点击 → 发出正确 Message」「插入后文本正确」，**不断言渲染结果**；字形目视交给 E3 真机 |

---

## 8. 验收

- 六项门禁全绿（`cargo fmt --check`、三轮 `clippy`、`cargo test`、`cargo doc`）。
- `compose::insert_emoji` 单测覆盖四种情形：空文档 / 有选区 / 行内 / 行尾（CJK 混排），且新选区位置正确。
- 面板在**明暗两套 visuals** 下渲染不 panic（无头 `egui::Context::run_ui` 三帧手法，见 `ui/layout.rs` 既有样例）。
- 真机目视：Linux / Win / mac 各一次，确认黑白 emoji 正常、非豆腐块。

## 9. 不做

- **编辑器正文彩色 emoji 维持黑白**（上游限制，见可行性调查 §4）。面板/预览走纹理路径（见 E-C1/E-C2 与可行性调查；#48 预览内联彩色走 `emoji://` 改写 + inline_widget，不经字形管线），不在本条禁区。
- 自定义表情包 / 图片 emoji（那是 image-plan 的地盘）。
- 肤色选择器、性别变体、长按展开。
- `:shortcode:` 自动补全 → 放进可选的 E4。

## 10. 排期与依赖

- 与 **#26 image-bed** 无依赖关系，可并行；E1 只有 0.5d，**想先看效果可以先放行 E1**。
- 若 UI 现代化 #27 的 U1（Inter + CJK fallback）先做，注意字体链顺序：emoji 在默认链里已是第一优先级，Inter 注入时要把 emoji 保持在链上（别被覆盖掉）。

---

## 11. E-C1 落地（#47，2026-10-02）

> 本节是落地登记，不再是规划。完整取舍与实测依据见可行性调查
> `.zcode/workflow-drafts/emoji-color-feasibility.md`（未入 git 库，路径以工作区为准）。

- **背景**：§2 F2 的「彩色不支持」是 epaint 字形管线上游限制（所有字形恒以纯白填充写入图集）；可行性调查实测换任何彩色字体（CBDT/COLR/sbix）都不可行（CBDT/COLR 字形无矢量轮廓，装上后连黑白都得不到），**随包 PNG 纹理是唯一低侵入路径**（`image` 解码 → `load_texture` → 白 tint Image，零新增依赖，`latermd-app` 已依赖 `image` 的 png feature）。
- **范围**：**面板 + 最近使用条**两处展示层（`ui/emoji_panel.rs` 单文件纹理分支 + `assets/emoji/twemoji/` 资产目录）。资产 = Twemoji（jdecked **v17.0.3**）72×72 PNG × 272 枚，CC-BY 4.0（全文随库，登记 [distribution.md](distribution.md) §6），钉版本下载脚本与 SOURCES.txt 随库可复现。缓存 = 会话级懒解码 `TextureHandle`（首帧懒建，不合并图集）。
- **明确不含**：预览内联彩色（**#48** E-C2，字符串层 `emoji://` 改写 + LinkHandler inline_widget——已于 #48 落地，见 §12 与 [emoji-color-acceptance.md](emoji-color-acceptance.md) E-C2 章）、**编辑器正文**彩色（feasibility §4 否决：需 fork epaint，光标/选择/IME/undo 全锚在 galley，收益配不上代价）。
- **失败面**：PNG 解码失败 / 资产缺失 → 该单元回落出厂 NotoEmoji 黑白（`painter.text` 原路径）；与 E3「数据保留、渲染兜底」同哲学，彩色不引入新失败面。点击区、hover、tooltip、`Message::EmojiInserted`、E3 cmap 过滤、settings.json recent 持久化零改动。
- **对外口径**（用户可见文案的唯一口径）：「**面板内彩色；编辑器正文仍黑白；导出/外发的显示效果取决于目标环境的字体**」——面板底部说明行（`ui/emoji_panel.rs`）、本文 §2、验收文档 §3 三处同源；**不许出现「全面支持彩色」**。
- **验收**：自动证据与人工清单分栏落档于 [emoji-color-acceptance.md](emoji-color-acceptance.md)（渲染不 panic、纹理命中计数、损坏回落、272/272 资产校验、体积/显存数字；Linux X11 / HiDPI / Win/mac 目视与 272 枚抽查待坤哥）。

---

## 12. E-C2 落地（#48，2026-10-04）

> 本节是落地登记，不再是规划。完整取舍与实测依据见可行性调查
> `.zcode/workflow-drafts/emoji-color-feasibility.md` §3/§7；验收证据见
> [emoji-color-acceptance.md](emoji-color-acceptance.md) E-C2 章；实现岔路登记
> decisions-pending #90（B1）/ #91（B2）。commits：B1 `b50a292`、B2 `a6ea1bc`、B3 `bf1bdbc`（验收与口径落档，纯 docs）。

- **机制（两件套，缺一不可，feasibility §3.2 的结论）**：
  - **B1 渲染副本改写器**：`latermd_md::expand_emoji_links` 纯函数（`latermd-md/src/lib.rs:740`，与 wikilinks/inline_marks 同层，不依赖 egui）把裸 emoji 改写成 `[😀](<emoji://😀>)`，产物**只进富渲染副本**——源码、rope、字节偏移、撤销栈、落盘字节分毫不动（与图片改写同一承诺）。**（2026-10-15 订正：原写「只进 `preview.rendered`」；`ui::preview` 与 `PreviewState.rendered` 已随 live-unify P4/P5 删除，改写现在发生在 `live::rich_block*` 的五层链第 3 层（`live.rs:242`），写作档正文与分栏档右栏走同一条路径。）**豁免走**与渲染同一套 pulldown-cmark**（同 vendored options）的事件区间：围栏/缩进代码块、行内代码、既有链接的文本与目标、HTML、脚注。覆盖集 = `emoji_data` 全表 272 枚参数注入（app 侧 `covered_glyphs()` OnceLock，`emoji_data.rs:279`），与面板/纹理共用单一数据源。同次产出第二层 `OffsetMap`（`state.rs:179`），wikilink → emoji 两层**串行穿过可组合**（#14 LP2-4 口径，`preview.rs:129` `map_source_offset` 三层穿透）。
  - **B2 inline widget 接线**：`AiLinkHandler`（`preview.rs:254`）接 vendored `LinkHandler` 五级扩展点，**app 侧实现、vendor 零改动**：`inline_widget_size` = font.size 正方形（行高恒不超正文自然行高，含 emoji 的行与相邻行同高）；`layout_link` = 透明占位用链接文字本体 + 同款字体（推进宽度与「emoji 以普通文本出现」逐像素一致，改写前后文本流零漂移）；`paint_inline_widget` = 查 #47 同源会话缓存 `emoji_panel::inline_texture`（`emoji_panel.rs:415`）→ 白 tint 画正方形（纹理原色），查不到什么都不画（透明占位原样保持）；`link_style` = 正文色 + 无下划线（不吃超链接样式）；`click` = 吞掉（`emoji://😀` 不是合法 URL，交浏览器只会弹错）；手型抑制在 app 侧 label 渲染后按本帧 emoji 区块压回 Default（vendored 对 inline widget 悬停无条件置手型，`link_style` 管不到光标，#91）。
- **范围：只富渲染（写作档正文 / 分栏档右栏，同一条链）**。**编辑器正文（源码档的 TextEdit）不做**——重申 feasibility §4：egui 0.36.2 `LayoutJob` 无图片内嵌通道、字形管线恒白填充，「编辑器彩色」只有 fork epaint（与不 fork 边界冲突）或字符换 widget（破坏 `CCursor`↔字节换算与 IME/undo）两条路，收益配不上代价，§9 禁区维持。**（2026-10-15 订正：本节原写「Live 模式富渲染块同口径维持黑白」——那是 live-unify 之前的状况。P1/P2 已把 emoji 改写、inline widget 与悬停手型抑制并入 Live，`RichLinkHandler` 现在同时服务两处，写作模式下 emoji 同样是彩色 inline widget。）**代码块/行内代码内的 emoji 豁免不改写（保持黑白字形）。
- **三段覆盖断言销账（feasibility §3.2 点名的缺口）**：段落/标题/表格全部吃到 inline widget（`emoji_inline_widget_paints_textured_squares_in_paragraph` / `…_inside_heading_at_link_font_scale` / `…_in_table_cell`，2026-10-04 实跑 3 passed）——无需「该段暂为黑白」的降级登记；唯一缩水口径是**标题内 emoji 不随 H1–H6 字号放大**（vendored 对 `Token::Link` 一律传正文基础字体，与 wiki:///ai:// 在标题里的既有行为同源，#91 已知如实行为，附 vendor ①类改法）。
- **已知边界（如实）**：覆盖集外字符（如 🫠）不改写（只有 272 枚有资产，改写而无纹理 = 从预览消失）；手写表外 `emoji://` 链接只留透明占位不画图；链接引用定义标签与脚注定义正文内的 emoji 暂不改写（黑白，#90）。
- **导出口径（重申 feasibility §5）**：`latermd-export` 零改动，emoji 以 Unicode 原样透传，CSS 只声明系统字体栈——导出/外发的显示效果**取决于目标环境的字体**，不承诺任何平台必然彩色；不做「导出保证彩色」的能力宣称。
- **对外口径（E-C1 口径按交付面扩展，用户可见文案的唯一口径）**：「**面板与预览内联彩色；编辑器正文（含 Live 模式）仍黑白；导出/外发的显示效果取决于目标环境的字体**」——**不许出现「全面支持彩色」**（2026-10-04 grep 核对：docs/crates/README 无一处能力宣称）。遗留：面板底部说明行（`emoji_panel.rs:619`）仍写「面板内彩色…」，句子真但未提预览，待后续 crates/ commit 顺手更新。
- **验收**：自动证据（三段断言、豁免测试、落盘字节不变、副作用/回落/全量回归）与人工清单（真机预览正文/标题/表格观感、HiDPI、明暗主题、代码区黑白对照、缓存同源、Win11/macOS 14、导出抽查共八项）分栏落档于 [emoji-color-acceptance.md](emoji-color-acceptance.md) E-C2 章（B3 `bf1bdbc`）。自动证据 2026-10-04 本机实跑：三段覆盖断言 3 passed（`cargo test -p latermd-app --all-features emoji_inline_widget`）、豁免 9 passed（`cargo test -p latermd-md --all-features expand_emoji_links`）、落盘字节不变 2 passed（`emoji_rewrite`）、emoji 全过滤 50 passed、两 crate 全量 622 passed + 1 ignored（既有 #39 取证）/ 49 passed、`cargo fmt --all --check` 干净、禁语 grep 六处命中全为禁令/核对自身零能力宣称；三轮 clippy 与 cargo doc 本轮未复跑（B3 零代码改动仅 .md，结果与 B2 head 全等，编排最终 head 复验兜底）。真机人工项 blocked_external（本机 Linux 无头、缺 Win11/macOS 14 真机），不以单测冒充目视。
