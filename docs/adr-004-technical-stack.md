# ADR-004: 技术栈与依赖版本清单

日期: 2026-09-24
状态: 已接受（2026-09-24 重构，见文末修订记录）
关联: [[adr-001-gui-and-architecture]]、[[adr-002-platform-renderer-wysiwyg]]、[[adr-003-renderer-and-ecosystem-audit]]

> 本 ADR 只记录**技术栈选型、依赖版本与打包方案**。
> 功能范围与排期不在本文 —— 唯一事实来源是 [roadmap.md](roadmap.md)。

---

## 1. 项目定位

**跨平台、版本化、可对话、可演化的 Markdown 知识工作台。**

- **Markdown** = 内容层（pulldown-cmark，单一解析器）
- **AI** = 智能层（token/AST 层操作，见 ADR-003 §3/§6）
- **Git** = 时间层（只读集成，P2）
- **纯 Rust GUI** = 跨平台与性能保障（egui + eframe）

三条贯穿全局的约束（单一解析器 / 业务逻辑不依赖 UI 框架 / AST 是 AI 基础）见 [README.md](README.md)，此处不重复。

---

## 2. 技术栈总览（已按实测修正）

| 层面 | 方案 | 版本 | 说明 |
| :-- | :-- | :-- | :-- |
| **GUI** | **egui + eframe** | 0.36.2 (2026-09-08) | 五个子 crate 同步发版，无版本错配 |
| **渲染层** | **vendored egui_markdown (membrane-io)** | HEAD + 升级到 0.36.2 | 4001 行，`forbid(unsafe_code)`，MIT OR Apache-2.0，带集成测试与 criterion bench |
| **Markdown 解析** | **pulldown-cmark** | 0.13.4 | 从 comrak 改过来，理由见 ADR-003 §4 |
| **序列化回 MD** | `pulldown-cmark-to-cmark` | 22.0.1 | Live Preview 与 AI 回写的刚需 |
| **语法高亮** | `syntect` | 5.3.0 | vendored egui_markdown 已集成 |
| **文本缓冲** | `ropey` | 1.6.1 | 编辑器缓冲 |
| **Git** | `git2` | 0.21.0 | vendored-libgit2 + vendored-openssl 简化构建 |
| **文件对话框** | `rfd` | 0.17.2 | Win32 / Cocoa / GTK+XDG Portal |
| **文件监听** | `notify` | 8.2.0 | inotify / FSEvents / ReadDirectoryChangesW |
| **异步运行时** | `tokio` | 1.53.1 | — |
| **序列化** | `serde` | 1（lock 1.0.229） | Theme 持久化（settings.json）等状态落盘的序列化 derive；P0 主题模块引入 |
| **JSON 序列化** | `serde_json` | 1（lock 1.0.151） | Theme 持久化落盘 settings.json（文件树 file_tree.json 同用）；P0 主题模块引入 |
| **CLI** | `clap` | 4.6.7 | — |
| **目录遍历** | `ignore` | 0.4.33 | 文件树与搜索共用（ADR-005 §4） |
| **搜索·行迭代** | `grep-searcher` | 0.1.17 | P1 侧边栏搜索：只用其 `LineIter`（ripgrep 同源行语义）；`Searcher` 系 API 需 `Matcher` 实参而 `regex` 未实现，亦不为此引入 `grep-regex`（decisions-pending #7） |
| **搜索·正则** | `regex` | 1.13.1（bytes 变体） | P1 侧边栏搜索的匹配引擎；大小写开关经 `RegexBuilder::case_insensitive` |
| **URL 编解码** | `percent-encoding` | 2.3.2 | P1 `ai://` 链接协议：prompt 查询参数的严格 %XX 解码（坏序列自行校验，`+` 不当空格）；decisions-pending #11 |
| **LLM·HTTP 客户端** | `ureq` | 3.4.2（default-features 关闭，仅 rustls） | P1 `latermd-ai`：阻塞 HTTP 读 SSE 流；零 async 依赖。选型取舍见修订记录 2026-09-25 |
| **LLM 接入** | OpenAI / Anthropic / Ollama | — | HTTP + SSE 流式；经 `latermd-ai` 以 std 线程 + mpsc 阻塞流实现，不经 tokio |
| **凭据存取** | `keyring` | 4.2.0 | 三平台系统凭据统一封装（Windows Credential Manager / macOS Keychain / Linux Secret Service 的 zbus 实现，不链 libsecret C 库）；P2 `latermd-creds` 引入；4.x 维护线选择见 decisions-pending #20 |
| **系统主题检测** | `dark-light` | 3.0.0 | P2.5 皮肤批次 B「跟随系统」：同步 `detect()`（Win 注册表 / macOS NSUserDefaults / Linux freedesktop portal）。Linux 无统一规范时返回 `Unspecified` 或 Err，一律折叠为「不知道」并回落手动值（roadmap 风险 #8）；只在跟随系统模式按 1s 节流轮询；口径见 decisions-pending #24 |
| **图片解码/编码** | `image` | 0.25（lock 0.25.10，default-features=false，features=png/jpeg/gif/webp） | 图片框 B/D 段：B 段喂 `egui_extras` 图片 loader（预览出图），D 段把剪贴板 RGBA 像素重编码为 PNG 落盘。features 即 D 段白名单本身（PNG/JPEG/WebP/GIF）；eframe/arboard 依赖树里本就有它，B 段起提升为直接依赖 |
| **SVG 解码** | `resvg` | 0.45.1（经 `egui_extras` 的 `svg`+`svg_text` feature 传入，不直接依赖） | 图片框 B 段：badges/README 生态的图片大量是 SVG。2026-10-22 随链接徽章用例开启 `svg`，同日补开 `svg_text`（=resvg `text`+`system-fonts`）：shields/GitHub 徽章的文字是 `<text>` 元素，resvg 默认不栅格化文字（真机实证：徽章底出来了、Rust/passing 没有）——开 text 后 egui_extras SvgLoader 的 cfg(svg_text) 分支自动 `load_system_fonts`。代价：fontdb 扫系统字体（启动一次性）+ fontconfig-parser 等 5 个传递依赖新增进编译面 |
| **剪贴板** | `arboard` | 3.6.1（lock，default-features 关闭，宿主 feature `image-data`） | 图片框 D 段：Ctrl+V 读取剪贴板图片字节。egui 0.36.2 的剪贴板**只出不进**（`copy_text`/`copy_image` 有，`get_image` 无——egui-winit 的 `Clipboard::get` 只查文本）；arboard 已在依赖树（eframe 默认开 `egui-winit/clipboard` → `arboard/image-data`），此处把传递依赖提升为直接依赖，**零新增编译面**。返回的 RGBA 像素重编码为 PNG 落盘（`image` 同条版本线）；通道选型实测与口径见 decisions-pending #42 |
| **皮肤文件序列化** | `ron` | 0.12.2 | P2.5 皮肤批次 B：`themes/*.ron` 存 `MarkdownStyle`。选 RON 而非 JSON 是因为皮肤给人手写与分享（支持注释、键名无引号）；样式本体仍靠 `egui_markdown_style` 的 serde derive |
| **PDF 导出后端** | `krilla` | 0.8.2（ADR-006 §6 锁定；0.x 的 minor 即破坏性版本，升级须过 PDF 快照回归） | #25 导出 PDF（ADR-006 路线 A 纯 Rust 直绘）：`latermd-export` 的 pdf 模块把 `latermd-render` 的绘制指令 IR 布局到 A4 并编码字节。默认 features（simple-text/raster-images）；实测 CJK 子集化生效（19.5MB NotoSansCJK face → 7.1KB 样例 PDF）。选型对比与 printpdf 否决依据见 ADR-006（**草案待用户追认**，decisions-pending #96） |
| **PDF 文本整形** | `rustybuzz` | 0.20.1（与 krilla 0.8.2 内部同版；树内既有传递依赖提升为直接依赖，零新增编译面） | #25 PDF：布局测宽与绘制共用同一份整形结果（rustybuzz shape → 归一 advance → krilla draw_glyphs），断行宽度即绘制宽度 |
| **PDF 字体度量** | `skrifa` | 0.42.0（lock 0.42.1，与 krilla 同版线；树内既有 0.44 为 egui 侧，两版本共存非本次引入） | #25 PDF：读 ascent/descent 定行盒与基线（krilla 未公开垂直度量） |
| **界面字体** | **Inter**（rsms/inter，**资源非依赖**，`include_bytes` 嵌入） | 4.1（Regular/Medium/SemiBold 三字重 TTF，共约 1.2MB） | U1（docs/ui-modernization.md §3）：Regular 挂 `FontDefinitions` Proportional **首位**，Medium/SemiBold 注册为 `FontFamily::Name` 按字重取用（egui 无字重轴，权重即族名）；三平台 CJK 候选表续后回退，emoji 链（NotoEmoji → emoji-icon-font）保序。SIL OFL 1.1，许可文本随仓库 assets/fonts/LICENSE-Inter.txt，第三方声明见 docs/distribution.md §6 |
| **打包编排** | **`axodotdev/cargo-dist`** | **v0.33.0** | ❌ 修正：不是已归档的 `astral-sh/cargo-dist` |
| **打包（备选）** | `crabnebula-dev/cargo-packager` | 0.11.8 | ❌ 修正：不是 `tauri-apps/`（404） |
| **macOS 打包** | `cargo-bundle` | 0.12.0 | 2026-09-20 仍活跃 |
| **Linux AppImage** | `cargo-appimage` | 2.4.0 | — |
| **Linux deb** | `cargo-deb` | 3.8.0 | — |
| **Linux rpm** | `cargo-generate-rpm` | 0.21.0 | — |

### 已否决

| 方案 | 否决理由 |
|---|---|
| **Tauri** | 用户已选纯 Rust。补充：Linux 需 WebKitGTK 4.1，Ubuntu 24.04 / Debian 13 已移除 4.0 包 |
| **GPUI** | crates.io 版本 0.2.2 停更近一年；实际需 git 依赖 Zed 仓库跟随季度级破坏性重构 |
| **tektite** | 146 下载 / 0 star / 单人业余维护。**仅保留作为设计参考** |
| **True WYSIWYG** | 列入 non-goals，理由见 ADR-002 §4.7 |

---

## 3. 跨平台专项

（扩充自 ADR-002 §5，增加打包维度）

| 类别 | Windows 11 | macOS 14 | Linux |
|---|---|---|---|
| 窗口后端 | Win32 | Cocoa | X11 / Wayland |
| 渲染后端 | DX12 / Vulkan | Metal | Vulkan |
| 快捷键 | `Ctrl` | `Cmd` | `Ctrl` |
| 换行符 | CRLF | LF | LF |
| 字体 | 需加载中文字体 | 系统字体较好 | 需确保中文字体可用 |
| 输入法 | IME | IME | IME |
| 文件监听 | ReadDirectoryChangesW | FSEvents | inotify |
| Git 凭据 | Credential Manager | Keychain | libsecret / gnome-keyring |
| 打包 | `.msi` / `.exe` | `.app`（universal2 dmg） | `.AppImage` / `.deb` / `.rpm` |

### 分发策略（2026-09-24 修订：无签名证书路线）

**不购买 Apple Developer 账号与代码签名证书。** 主渠道 GitHub Release，macOS 经自有 Homebrew tap [`crazykun/homebrew-ailater`](https://github.com/crazykun/homebrew-ailater) 分发：

- **cask 复刻 tap 内 `lscreen` 的成熟模式**：universal2 单 dmg（双架构 lipo 合一）、`postflight` 执行 `xattr -dr com.apple.quarantine` 消除 Gatekeeper「已损坏」拦截、`livecheck :github_latest` 自动发现新版本。
- tap 已有每小时 auto-bump 流水线（`.github/workflows/auto-bump.yml`），接入 LaterMD 只需新增一项。
- cargo-dist 只负责构建三平台 Release 产物与 CI；tap 更新走自有流水线，不走 cargo-dist 的 homebrew installer（它生成 formula 装 `bin/`，不适合 GUI app 的 cask 形态）。
- Linux 的 AppImage / deb / rpm 照常产出，供非 brew 用户直下。
- Windows 直下 `.exe` 触发 SmartScreen，README 写明「更多信息 → 仍要运行」，不视为缺陷。

> 先前的「macOS 必须 codesign + notarytool、建议购买 Windows 代码签名证书」结论**作废**——那是签名路线的前提，成本（$99/年 + 证书费 + 公证调试）对本项目无必要，brew 路线已由 lscreen 验证。

CI 矩阵结构见 [roadmap.md](roadmap.md) 持续项一节（落地以 `cargo dist init` 生成物为准）。

---

## 4. 工具链：必须钉 1.98.0

**实测失败**：

```
error: rustc 1.94.0 is not supported by the following packages:
  egui@0.36.2 requires rustc 1.95
  ecolor@0.36.2 requires rustc 1.95
  emath@0.36.2 requires rustc 1.95
  epaint@0.36.2 requires rustc 1.95
```

本机默认 toolchain 为 1.94.0。**必须加 `rust-toolchain.toml`**：

```toml
[toolchain]
channel = "1.98.0"
```

本机已装该版本，实测可编译通过。

> 注意：上游 egui_markdown 的 `rust-toolchain.toml` 写的是 `channel = "stable"`，vendor 后需改为钉死版本。

---

## 5. 项目结构（终态）

```
LaterMD/
├── Cargo.toml
├── rust-toolchain.toml              # channel = "1.98.0"
├── vendor/
│   └── egui_markdown/               # vendored + 升到 egui 0.36.2
│       ├── src/
│       ├── egui_markdown_style/     # 子 crate：MarkdownStyle + serde
│       ├── tests/                   # cache / indent / truncate / width
│       ├── check.sh                 # 照抄到 CI
│       └── LICENSE-MIT, LICENSE-APACHE
├── crates/
│   ├── latermd-core/                # 应用状态机、DTO（出现第二消费者时创建）
│   ├── latermd-md/                  # pulldown-cmark 封装 + token span
│   ├── latermd-render/              # token → 绘制指令（不 import egui）
│   ├── latermd-editor/              # ropey + caret/选区 + IME
│   ├── latermd-git/                 # git2 封装（P2）
│   ├── latermd-ai/                  # provider trait、流式（P1）
│   ├── latermd-export/              # HTML（P0）/ PDF / DOCX
│   └── latermd-app/                 # eframe binary，唯一 GUI crate
├── docs/
└── .github/workflows/
```

**此结构是终态，不是开工指令。** 各 crate 的增量创建时机见 [roadmap.md](roadmap.md)「crate 增量创建表」—— 空骨架是过早抽象。

**说明**：此结构相对于初版方案的 `apps/desktop` + `core` + `ui` + `packages/` 做了简化。理由：

- egui 是立即模式，UI 代码天然与状态机耦合，强行拆 `ui` 层会产生大量跨 crate 的 `&mut Ui` 传递
- `packages/markdown-editor` 是过早抽象 —— 第一个可用版本之前不存在第二个消费者

保留的核心原则不变：**业务逻辑不依赖 UI 框架**。

---

## 6. 历史决策摘录（保留结论，细节见出处）

- **PoC 阶段已被 Vendor 适配取代。** 原 PoC 的三个验证项中，「升级改动量」已由实测回答（24 个错误、3-5 工作日，无阻塞风险），故可直接进 M0。
- **打包工具勘误**：cargo-dist 官方仓库是 `axodotdev/cargo-dist`；`astral-sh/cargo-dist` 已归档；`tauri-apps/cargo-packager` 404，实为 `crabnebula-dev/cargo-packager`。

---

## 修订记录

| 日期 | 变更 |
|---|---|
| 2026-09-24 | 初版：技术栈汇总（当时还承担「最终汇总」职责，含功能范围与排期） |
| 2026-09-24 | **重构**：原 §六功能范围、§七演进路线是过期快照（P0 6-8 周、无文件树/大纲/搜索），与 ADR-005 修订后的 roadmap 冲突。按「单一事实来源」原则，范围与排期全部移交 roadmap.md，本文收敛为纯技术栈 ADR。同步删除与 ADR-001/002/003 重复的铁律、heal/LinkHandler、架构约束三节 |
| 2026-09-24 | **分发策略修订**：确认无 Apple Developer 账号与签名证书，分发改为 GitHub Release + Homebrew tap `crazykun/homebrew-ailater`（cask 复刻 lscreen 模式）。原「存在性阻塞」小节的公证/证书结论作废，排期中证书申请动作移除 |
| 2026-09-25 | **依赖表补登**（P0 评审整改）：latermd-app 为主题持久化新增的 serde / serde_json 此前漏登，补两行（P0 批次）。同步订正 latermd-app/Cargo.toml 中「本就在依赖图中（criterion 的传递依赖）」的错误注释——criterion 是 dev-dependency，不进运行时依赖图，真实理由是主题持久化需要 |
| 2026-09-25 | **依赖表补登**：P1 搜索核心（`crates/latermd-app/src/search.rs`）新增 grep-searcher 0.1.17 与 regex 1.13.1，遵循 decisions-pending #4 口径；不引入 `grep-regex` 的取舍见 decisions-pending #7 |
| 2026-09-25 | **依赖表补登**：P1 新建 `crates/latermd-ai`（crate 增量创建表「P1 开工」行授权），新增 `ureq` 3.4.2。选型取舍：reqwest 的 blocking 客户端内部仍自建 tokio runtime，与本轮「流式 = std 线程 + mpsc、不引入 tokio」的既定口径冲突，故选零 async 依赖的 ureq；default-features 关闭去掉 gzip（SSE 不需要），只留 rustls。同轮订正「LLM 接入」行的「异步」表述——实际为阻塞流。serde_json 复用清单既有条目，不另列 |
| 2026-09-25 | **依赖表补登**：P1 `ai://` 链接协议（`crates/latermd-app/src/ai_link.rs`）新增 `percent-encoding` 2.3.2，遵循 decisions-pending #4 口径；零传递依赖、Cargo.lock 原有条目提升为直接依赖。协议语义定稿见 decisions-pending #11 |
| 2026-09-25 | **依赖表补登**：P2 凭据管理新建 `crates/latermd-creds`（roadmap 阶段 4「凭据管理」条目），新增 `keyring` 4.2.0（默认 feature `v1`）。选型：3.6.3 随原维护者移交已停更且无默认 features；4.2.0（open-source-cooperative 接管，2026-08 仍更新）默认 feature 即按平台选三平台 native store，Linux 走 zbus 纯 Rust 实现免 libsecret C 库，Cargo.lock 既有 zbus 条目复用；keyring-core `Error` 的 `Display` 不携带凭据字节，契合「凭据值不进错误信息」红线。岔路与签名口径登记见 decisions-pending #20 |
| 2026-09-26 | **依赖表补登**：P2.5 皮肤批次 B（roadmap「专题：界面美化与皮肤系统」批次 B）新增 `dark-light` 3.0.0（跟随系统主题检测）与 `ron` 0.12.2（`themes/*.ron` 皮肤文件），遵循 decisions-pending #4 口径；二者均为 app 侧依赖，不进 `latermd-mcp` / `latermd-search`（守铁律二）。取舍与回退口径见 decisions-pending #24 |
| 2026-09-28 | **依赖表补登**：图片框 D 段（粘贴/拖拽插入，docs/image-plan.md §3.D）把 `arboard` 3.6.1 从传递依赖（eframe → egui-winit/clipboard）提升为 latermd-app 直接依赖。此前 B 段已把 `image` 0.25（default-features=false，png/jpeg/gif/webp 白名单 features）登记为直接依赖，本行补 arboard；剪贴板图片通道的实测依据（egui 0.36 无 `get_image`）与「文本粘贴优先、图片兑底」的触发口径见 decisions-pending #42 |
| 2026-09-28 | **资源登记**（U1，docs/ui-modernization.md §3）：assets/fonts/ 新增 Inter 4.1 三字重 TTF（Regular/Medium/SemiBold，`include_bytes` 嵌入随包分发）。字体是资源不是库，不进 Cargo 依赖、不占依赖表行——在技术栈表单列「界面字体」行登记；SIL OFL 1.1 许可与发布产物合规口径见 docs/distribution.md §6。链顺序与混排行高的无头测试在 crates/latermd-app/src/fonts.rs |
| 2026-09-29 | **依赖表补登**：U4 图标全量迁移（decisions-pending #35 拍板乙，cherry-pick 自 feature/ui-modern-all ⑥）新增 `egui-phosphor` 0.14——bundled TTF 私有区码位（U+E0xx）三平台一致，`ui/icons.rs` 自绘线段退役改 30+ 变体人工映射；链位 Inter 之后、NotoEmoji 之前（`fonts.rs` 单测锁定），字体族走 Proportional 不单开 `FontFamily::Name`（未 install 的 Context 上后者 panic）。选型核对：0.14 与 egui 0.36 对齐、MIT/Apache-2.0、字体可 subset |
| 2026-09-29 | **依赖回退**：U4 迁移（当日上一条补登）落地当天经真机目视，坤哥拍板「图标还没原来的好看」→ revert（decisions-pending #35 拍板回滚甲），`egui-phosphor` 0.14 从依赖表移除，`ui/icons.rs` 回自绘体系。phosphor 完整实现存档 PR #55 / `archive/feature-ui-modern-all`，字体链位探针与单测方法论（insert(1) 实测）沉淀于 #35 记录。**图标类纯视觉决策以真机人眼为准，像素验收只证一致性** |
| 2026-10-02 | **资源接线登记**（#16 图标接入）：窗口图标接线落地——main.rs 视口构建 `ViewportBuilder::with_icon` + `include_bytes` 嵌入 `assets/logo/deliverables/png/icon-64.png`（LOGO-SPEC v1.1 素材，编译期路径校验）+ `image` crate 解码为 `egui::IconData`。**依赖零新增**：复用依赖表既有 image 0.25（B 段图片框已登记，png feature 在位）；图标是资源不是库，不占依赖表行。失败回落口径：解码任一步失败返回 `None` 并终端告警，回落 egui 默认图标，绝不 unwrap/panic 拦启动；Some/None 两分支有无头单测（main.rs）。窗口图标（任务栏/标题栏/Alt-Tab）的真机显示效果留坤哥目视确认 |
| 2026-10-04 | **依赖表补登**（#25 导出 PDF M2，ADR-006 路线 A 落地）：新建 `crates/latermd-render`（path 依赖,绘制指令 IR,铁律 2 验证场——`cargo tree -p latermd-render --invert egui` 实测 not found,正向树仅 pulldown-cmark）;`latermd-export` 新增 `krilla` 0.8.2（PDF 编码后端）、`rustybuzz` 0.20.1 与 `skrifa` 0.42.0（与 krilla 内部同版的既有传递依赖提升为直接依赖,零新增外部 crate）。断行自写（空格后可断 + CJK 字符间可断）,未引入 unicode-linebreak（ADR-006 §6 预授权分支）。ADR-006 状态仍为**草案待用户追认**（decisions-pending #96）,本行登记滞后于 M2 实现 commit（`c0d1f7b` 已先行落库,三行依赖表与本修订行随 #25 后续 commit 补齐落库）,追认与否不影响回滚口径（revert #25 时三行依赖表登记与本修订行一并回退） |
| 2026-10-09 | **依赖表补登 + 资源接线登记**（#74 品牌标识三平台接入）：素材换为 AIM 标识（`assets/logo/latermd-icon.svg` 源 + `latermd-mark{,-black,-white}.svg` 单色变体），`build-deliverables.py` 重写并新增**三平台原生格式**产出：Windows `latermd.ico`（16…256 六档）、macOS `AppIcon.iconset`（10 档）+ **预合成 `AppIcon.icns`**（Linux 无 iconutil，按 icns 容器格式直拼：`'icns'` + u32BE 总长 + `[OSType + u32BE(数据长+8) + PNG]`，长度字段含自身 8 字节头）、Linux hicolor 八档、web favicon。**新增依赖 `winresource` 0.1.31**（`[target.'cfg(windows)'.build-dependencies]`）：把 ico 嵌进 exe 资源段 —— eframe 的 `ViewportBuilder::with_icon` 只管运行时**窗口**图标（WM_SETICON），资源管理器列表 / 任务管理器 / 开始菜单读的是 exe 自身资源段，**两者互不替代**。连带 `toml` 1.1.8 + `serde_spanned`/`toml_writer` 为其最小闭包（toml 在本仓为独家引入，无既有依赖牵连）。接线四处：①`assets.rs` 新增 `brand_logo_image()`（`include_bytes` icon-256.png + image 解码为 ColorImage），titlebar 左上角经 `ctx.data` 缓存纹理、**首帧 load 一次**，素材按 2x 上采样存 256 缩到 18px 绘制不过采样，解码失败回落矢量 `Icon::Files` 不拦启动；②`build.rs` 嵌 Windows exe 图标（`#[cfg(windows)]` 圈死，素材缺失 assert 令构建红）；③`packaging/macos/Info.plist` 补 `CFBundleIconFile=AppIcon`（`bundle.sh` 早已 iconutil 出 icns，**缺此键则图标文件在包里也没人读**，Dock/访达仍是毛坯）；④`linux-deb.yml` 补 checkout（此前无，拿不到仓库内图标与 `.desktop`）+ `packaging/linux/latermd.desktop` + hicolor 八档安装 + `desktop-file-validate` 过闸（不合规的 desktop 文件被启动器**静默丢弃**，症状与没装一样）。守门测试三条且**已做红绿验证**：几何（贴左缘/边长取 token/不越中线/标题起点跟随标识右缘且不压图）、纹理确实被 load（回落路径会红，防「换了张图左上角还是旧图标」）、素材非纯色 256×256。**红绿验证中淘汰一条假绿测试**：初版「logo 不吞拖拽」注入 `ui.interact` 破坏后仍通过 —— 前提不成立（背景 `allocate_rect` 铺满整条 bar 且先注册），已删除换成「标识与标题不得重叠」 |
| 2026-10-22 | **依赖表补登**（链接徽章用例，坤哥「能支持下渲染吗」）：图片框 B 段开启 `egui_extras/svg`（resvg 0.45.1 传入），白名单口径同步 docs/image-plan.md；http 解码通道由 vendored `images` feature 联动的 `egui_extras/http` 早已在位（ehttp→ureq 复用）。同轮 vendor ①类（PR #303 同仓继续）：`Token::Image` 新增 `link` 字段——`[![alt](img)](target)` 的图片 token 携带外层链接，点击新标签打开（egui::Image 默认 hover-only，链接态升 Sense::click），外层不再补零宽空 Link token |
