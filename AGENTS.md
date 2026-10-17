# LaterMD 项目规范（所有 Agent 必读）

> 本文件是项目的**唯一事实来源**。写任何代码前先读这里。
> 完整论证、实测数据与取舍过程见 [docs/](docs/README.md)，本文件不重复论证，只给结论。

---

## 1. 产品定位

**跨平台、版本化、可对话、可演化的 Markdown 知识工作台** —— Markdown 是内容层，AI 是智能层，Git 是时间层。

**当前阶段：P0 骨架。** 完成的定义是「编辑-预览-导出-打包四件事在三平台跑通」。
任何不能让「写下一篇技术文档」更快的功能，都不在 P0–P2 范围内。

---

## 2. 技术栈（已锁定，不要重新选型）

| 层面 | 方案 | 版本 | 备注 |
|---|---|---|---|
| GUI | **egui + eframe** | **0.36.2** | 五个子 crate 同步发版，无版本错配 |
| 渲染层 | **vendored egui_markdown**（membrane-io） | HEAD + 升级到 0.36.2 | `vendor/` 目录，非 crates.io 依赖；变更登记见 [vendor/README.md](vendor/README.md) |
| Markdown 解析 | **pulldown-cmark** | 0.13.4 | 唯一解析器 |
| 序列化回 MD | `pulldown-cmark-to-cmark` | 22.0.1 | Live Preview 与 AI 回写刚需；**已选型、尚未引入** |
| 语法高亮 | `syntect` | 5.3.0 | vendored 层已集成 |
| 文本缓冲 | `ropey` | 1.6.1 | 编辑器缓冲 |
| Git | `git2` | 0.21.0 | 只开 `vendored-libgit2`，**不启用 openssl**：P2 不做 remote（fetch/push/pull），传输层用不上，锁内无 `openssl*` |
| 文件对话框 | `rfd` | 0.17.2 | — |
| 文件监听 | `notify` | 8.2.0 | **已选型、尚未引入**（ADR-004 已定，依赖树里现在没有它） |
| 异步运行时 | **不引入 `tokio`** | — | LLM 流式走 std 线程 + mpsc；与 tokio 分道的原因见 ADR-004 的 2026-09-25 修订行 |
| 打包编排 | **`axodotdev/cargo-dist`** | 0.33.0 | 不是已归档的 `astral-sh/cargo-dist`；版本出自 `dist-workspace.toml` |
| 工具链 | **rustc 1.98.0** | 钉死 | 见 §4 |

> **上表的版本以 `Cargo.lock` 为准**（2026-10-17 逐项复核过：`egui`/`eframe`/
> `epaint`/`egui-wgpu`/`egui_extras` 均 0.36.2、`syntect` 5.3.0、`ropey` 1.6.1、
> `git2` 0.21.0、`rfd` 0.17.2、`image` 0.25.10、`krilla` 0.8.2）。
> 标「尚未引入」的是**已选型但按增量依赖原则还没落地**的项 —— 别当成现状
> 依赖去 `use`；`cargo-dist` 是 CI 编排工具，不进 `Cargo.lock`。

### 已否决，不要再翻案

| 方案 | 否决理由 |
|---|---|
| **Tauri** | 用户已定纯 Rust；且 Linux 需 WebKitGTK 4.1，Ubuntu 24.04 / Debian 13 已移除 4.0 |
| **GPUI** | crates.io 版本 0.2.2 停更近一年；实际需 git 依赖 Zed 仓库跟随季度级破坏性重构 |
| **tektite** | 146 下载 / 0 star / 单人业余维护。**仅作设计参考，不作依赖** |
| **comrak** | vendored 层已用 pulldown-cmark，引入它 = 两套方言 |
| **iced** | async-first 心智负担重；表格/嵌套滚动控件成熟度不足；9 个月未发版。保留观察 |
| **True WYSIWYG** | 列入 non-goals。理由见 docs/adr-002 §4.7 |

---

## 3. 四条铁律（违反即 PR 驳回）

1. **单一解析器。** 全项目只允许 pulldown-cmark 一个 Markdown 解析器，杜绝预览与导出出现两套方言。
2. **业务逻辑不依赖 UI 框架。** `latermd-render` 只定义绘制指令结构，**不 import `egui`**；由 `latermd-app` 翻译为 egui 调用。目的：将来可做 headless CLI 导出器。
3. **AST 是 AI 增强的基础。** 所有 AI 修改在 token/AST 层操作，绝不操作渲染后的盒子。
4. **单一富渲染路径。** 富渲染只有 `live::ui` 一条（块级 `MarkdownLabel` + `RichLinkHandler`）。三态视图的
   「分栏」档，右栏渲染的是**同一份文档的 Live 富渲染**，只是只读呈现，**不是第二套渲染器**；历史上独立
   的「对照预览」面板及其渲染链路（`ui::preview`）已删除。新增富渲染能力一律进 `live::ui`，
   不要另起渲染器或另起一份面板状态。

---

## 4. 工具链：必须钉 1.98.0

本机默认 toolchain 是 1.94.0，**编不了 egui 0.36.2**（实测报错 `requires rustc 1.95`）。

根目录必须有 `rust-toolchain.toml`：

```toml
[toolchain]
channel = "1.98.0"
```

- 上游 vendored 代码的 `rust-toolchain.toml` 写的是 `channel = "stable"`，vendor 后必须改掉。
- CI 用 `dtolnay/rust-toolchain@1.98.0`，**不能用 `@stable`**。

---

## 5. 平台与渲染后端

- 平台基线：**Windows 11 / macOS 14 / Linux**。**显式不支持 Win10 与 macOS 13**，写进 README。
- 渲染后端：**wgpu**（eframe 默认）。**不要启用 `glow`** —— macOS 上 OpenGL 已被 Apple 废弃。
  仅保留 `LATERMD_RENDERER=glow` 环境变量作为驱动黑名单的逃生口，并在 Settings 面板显示当前后端。
- Linux 开发机（Deepin + rolling kernel）：若 Wayland 下 `egui_wgpu` 起不来，**直接切 X11 会话**，不要在此耗时。
- **分发策略（2026-09-24 定）：无签名证书路线。** 主渠道 GitHub Release；macOS 经 Homebrew tap [`crazykun/homebrew-ailater`](https://github.com/crazykun/homebrew-ailater) 以 **cask** 分发，**复刻 `lscreen` 模式**：universal2 单 dmg（双架构 lipo 合一）+ `postflight` 执行 `xattr -dr com.apple.quarantine` 消除 Gatekeeper 拦截 + `livecheck :github_latest` 自动发现新版本。**不做 codesign 公证，不买证书。**
- Windows 无证书直下会触发 SmartScreen，README 写明「更多信息 → 仍要运行」，不视为缺陷。
- CI 矩阵三平台，追加 `aarch64-pc-windows-msvc` 发布目标。

---

## 6. 关于 vendored 渲染层（改动前必读）

vendored `egui_markdown` 存在于 `vendor/egui_markdown/`，以下结论均已实测，**不要再凭直觉推翻**：

1. **它不是「单 Galley」。** 是分段 Galley 序列：表格 / 代码块 / 引用 / 图片本来就是独立 widget。
2. **视口剔除已内建**（`ui.is_rect_visible()` + `ui.allocate_space()`），每帧工作量是 O(visible area)。**不要自己实现虚拟化。**
3. **Live Preview 的成本是 3-5 天，不是 1-2 周。** 复用现有 `segment_breaks`，在 `render_token_range` 里加「此 block 是否含光标」的分支即可。
   **源码模式与 Live Preview 必须是一个编辑器 + 一个 `render_mode` 标志**，共用同一个 rope buffer 和 undo 栈，不要做成两个编辑器。
4. **`Token` 没有源码 span。** 需从 pulldown-cmark 的 event offset 透传。**这是 Live Preview 的 prerequisite，vendor 时就做掉。**
5. **`heal()` 是字符串预处理函数**，给残缺 Markdown 补闭合标记，让 LLM 流式输出的每一帧语法合法。**P1 AI 功能的必需品，不是可选优化。**
6. **`LinkHandler` 是五级扩展点**（block widget / inline widget / layout_link / link_style / click）。`ai://` 链接、AI 指令块、内联批注都从这里长出来。
7. **AI 流式预览的 widget id 必须稳定**，绝对不能包含 `content.len()`，否则每 token 都清空缓存、增量高亮失效。
8. **`membrane` feature 不启用。** 它带来 9 个额外编译错误（`LeadingSpace`、`TextFormat::bg_stroke` 等），是上游自家产品定制，对我们无价值。vendor 时直接删除该 feature 及其 cfg 代码（约 5 处），删除理由已记录在 [vendor/README.md](vendor/README.md)。
9. **vendor 改动按主题拆 commit，三类不得混。** ①上游可合（版本升级、API 迁移、token span 透传等通用能力，设计时即按上游 CONTRIBUTING 标准）；②私有删改（`membrane` feature 及其测试）；③仓库接驳（删上游 `[workspace]` 段与嵌套 `rust-toolchain.toml`、挂根 workspace）。同一文件混多类时按 hunk 拆（`git add -p`）。①类保持独立可 cherry-pick，用于将来 fork 上游提 PR（上游 main 截至 2026-09 仍在 egui 0.34，升级 PR 有真实价值）。全部改动登记进 [vendor/README.md](vendor/README.md) 的变更表；该文件在 subtree 前缀之外，**LaterMD 的说明一律不写进 `vendor/egui_markdown/` 内部**，否则会污染 subtree pull 与发往 fork 的补丁。

升级 0.34 → 0.36.2 的完整操作清单见 [docs/vendor-upgrade-checklist.md](docs/vendor-upgrade-checklist.md)。
质量门禁照抄上游 `check.sh` 六项到 CI：`cargo fmt --check`、三轮 `clippy -D warnings`、`cargo test --all-features`、`cargo doc --no-deps --all-features`。

---

## 7. 范围边界

**在范围内（按优先级）**

- **P0**：左右双栏实时预览 / 新建打开保存另存为 / CommonMark + GFM / 代码块高亮+复制 / 导出 HTML / 主题切换 / 快捷键 / 三平台打包签名 / **文件树基础版** / **大纲廉价版**（点击跳编辑器光标，不跳预览）
- **P1**：AI 流式写作（`heal()` + `LinkHandler`）/ `ai://` 协议 / AI 指令块 / AI commit message / AI 摘要大纲 / **侧边栏全文搜索**（`ignore` + `grep-searcher` + `regex`）
- **P2**：Git **只读**集成（状态、历史、diff、blame、回滚）
- **P3**：Live Preview / 大纲预览跳转 / 双向链接 `[[wikilink]]`

> 全文检索已在 P1 以侧边栏搜索形态交付，P3 不再有重复条目。各阶段周期与验收明细**只看 [docs/roadmap.md](docs/roadmap.md)**，本节仅列范围。

**明确不做（不是「以后做」，是不属于本产品）**

- 知识图谱、语义搜索、MOC、多模态嵌入 —— **这是另一个产品**
- Git 的 rebase / cherry-pick / LFS / submodule / 多仓库
- 小说助手、闪卡、日记洞察
- True WYSIWYG（形态 C）

---

## 8. 工程约定

- crate 前缀统一 `latermd-`；`latermd-app` 是唯一的 GUI crate（eframe binary）。
- 不引入 `core` / `ui` 二分：egui 是立即模式，UI 与状态机天然耦合，强行拆 `ui` 层会产生大量跨 crate `&mut Ui` 传递。
- 不做 `packages/markdown-editor` 这类过早抽象 —— 第一个可用版本之前不存在第二个消费者。
- Windows 用 `Ctrl`，macOS 用 `Cmd`；换行符 Win = CRLF，其余 LF。三平台都要确保中文字体可用。
- **IME 是头号风险。** Windows 微软拼音 + macOS 简体拼音的候选框跟随、不吞字、不抢焦点，必须在 M0 阶段实测掉，不要想当然。
- **分支与推送（多人协作）。** `main` 是受保护的集成分支，**禁止直接 push 到 main、禁止 force push**。日常在 `feature/<短横线主题>` 上工作（修缺陷用 `fix/<主题>`，基础设施用 `chore/<主题>`）；commit 完成后 `git push -u origin <branch>` 推同名远端分支，不留本地堆积，再 `gh pr create` 开 PR 合入 `main`。
- **PR 合并授权（2026-10-10 用户确认）。** 后续创建 PR 后，在最终 head 通过规定门禁且可合并时，直接合入 `main`，无需逐次询问；合入后同步本地 `main` 并确认该提交的 CI 通过。用户后续明确要求暂不合并时，以当次指令为准。
- **CI 只在 `main` 上跑**（`.github/workflows/rust.yml` 只保留 `push: branches: [main]`）。因此**合入 PR 前必须在本地跑完六项门禁**：`cargo fmt --all --check`、三轮 `cargo clippy --workspace --all-targets`（default / `--no-default-features` / `--all-features`，均 `-D warnings`）、`cargo test --workspace --all-features`、`cargo doc --no-deps --all-features`；vendor 改动加跑 `vendor/egui_markdown/check.sh`。合入后 main 上的 CI 是兜底，不是第一道防线。
- **门禁必须跑在"最终要合入的那个 head"上**：PR 合并**之后**再往该分支推的 commit **不会进 main**（PR 已关闭，分支后续的提交无人搬运），main 于是拿到没过门禁的版本。2026-09-26 实测踩过一次：`feature/p3-live-preview` 合并后补推的 clippy 修复（`cfdb812`）没进 main，main 的 Gate 当场红在 `clippy (default)`，要靠下一个 PR 顺带把同一处改回来才转绿。**补提交一律另开分支 + 新 PR**；合入后也要确认 `origin/main` 最新 run 是绿的再往下走。
- **PR 描述要求**：vendor 改动必须标明类别（①上游可合 / ②私有删改 / ③仓库接驳，见 §6），便于将来 cherry-pick 与解 subtree 冲突；非 vendor 改动注明「非 vendor」即可。
- **与 main 同步用 rebase，不用 merge**：开工前 `git pull --rebase origin main`，PR 落后于 main 时同样 rebase 后 force push **自己的分支**（不是 main）。工作区有他人未提交改动、rebase 无法启动时，允许用 `git merge origin/main` 例外。
- 仓库设置侧应为 `main` 开启分支保护（要求 PR），把「禁止直推 main」固化下来，别只靠自觉。

---

## 9. 跨会话必读

- 决策全文：[docs/README.md](docs/README.md) 是 ADR 索引，先看决策总表。
- 五份 ADR 均有「实测推翻先前判断」的记录。**读 ADR 时优先看修订对照表**，不要在已推翻的旧结论上继续推理。
- 排期与验收的**唯一事实来源是 [docs/roadmap.md](docs/roadmap.md)**。ADR 与本文件里出现的历史周期数字不回写，以 roadmap 为准。
- 风险清单（IME、无签名信任门槛、上游停更、cask 停更、工期）集中在 roadmap.md 的「风险登记册」。
