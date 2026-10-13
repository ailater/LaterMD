# LaterMD 演进路线图

日期: 2026-09-24  
状态: 已接受  
关联: 全部 ADR

> 本文件是 **动态清单**，不重复各 ADR 的论证，只记录阶段划分、验收标准与当前位置。  
> 论证见 [adr-001](adr-001-gui-and-architecture.md)、[adr-002](adr-002-platform-renderer-wysiwyg.md)、[adr-003](adr-003-renderer-and-ecosystem-audit.md)、[adr-004](adr-004-technical-stack.md)、[adr-005](adr-005-layout-and-sidebar.md)。



---


## 当前位置

```
[x] Vendor 适配   2026-09-24 完成(check.sh 六项全绿,spans 已加入)
[~] M0 技术验证   验证 2 已实测:10 万字真窗口滚动 p50 60.3 fps(llvmpipe 软件渲染下限)、bench 单帧 430 µs;验证 4 实测出流式追加是 O(n)(~77 µs/行),已列为 P1 开工前必解项;2026-10 同口径复测确认仍未解(期间未做修复,水位即原水位:10,000 行档 993.94 ms、较 M0 +25.9%,每行成本随规模上升;修复需 vendored ①类改动,需求挂 decisions-pending #77 待拍板,前后对比见 perf-recheck-2026-10.md §8),挂账维持。验证 1 IME Linux 已首测:输入可用,候选框不跟随光标,排查挂账(m0-report.md 验证 1)。出口仍卡两条真机项(Win/mac IME、Win/mac wgpu),见 m0-report.md
[~] P0 骨架       ← 当前(2026-09-30 按 V1 核对表 R3/R8 纠偏):功能 11/11 已落地(打包分发链路三版跑通);收尾修补(跨平台字体候选/ADR 登记/后端显示)已于 2026-09-25 完成;Release 链路已跑通——v0.0.1 2026-09-26、v0.0.2 09-29、v0.0.3 09-30 三版均自动发版(auto-tag → release → dmg/deb 全链路,runbook 见 [distribution.md](distribution.md),证据 E1–E4/E7);P0 剩余 = 三平台真机验收(安装/IME/wgpu)与连续写作长跑人工段
[~] P1 差异化     全文搜索已落地(2026-09-25),latermd-ai 基础(Mock 流式)已落地(2026-09-25),ai:// 链接与 AI 指令块已落地(2026-09-25),AI commit message 与摘要大纲已落地(2026-09-25);阶段内模块全部落地,P1 剩余仅真实 key 的凭据存取,按 auto-plan 队列归 #7(P2) 实施
[x] P2 版本层     Git 只读集成已落地(2026-09-25);凭据管理已落地(2026-09-25);P2 全部完成(latermd-creds + AI key 闸门/设置区经 PR #20 携带合入)
[x] P2.5 界面打磨   2026-09-26 全部落地:批次 B-light(图标/工具栏/状态栏/设置对话框/快捷键可改绑/AI 配置页)+ 批次 B 本体(三态主题含跟随系统 + themes/*.ron 皮肤文件 + 标准/紧凑密度),设计规格见 docs/ui-polish.md
[x] MCP 附加项   2026-09-26 落地(原登记为「P3 之后」):latermd-search 下沉 + latermd-mcp 五只读工具 + stdio/HTTP 双通道 + 设置页 MCP 页与状态栏;默认关闭,只绑 127.0.0.1。详见 [mcp-plan.md](mcp-plan.md)
[x] P3 深水区    Live Preview v1 + [[wikilink]] 已于 2026-09-26 落地;大纲预览跳转订正(2026-10-03,auto-plan #42):2026-09-26 版从未生效——滚动指令写在 ScrollArea 闭包外,pass_state 滚动目标下一帧 begin_pass 被清空永不消费(即用户 2026-09-30「目前就源码跳转了」反馈的根因,机制定位与修复见 decisions-pending #79),2026-10-03 经 #42 vendored 块表通道+闭包内消费真正落地(源码+预览双栏跳,真机目视留人工);剩余增强 = Live Preview v2(内联半隐藏)、Live Preview 选区扩展、反向链接面板 → 2026-09-26 已入自动流水线(auto-plan #14/#15)
多标签已落地(2026-09-26,用户追加需求)
[x] 外壳重构      2026-09-27 全部落地(规格 2026-09-26 产出:[ui-shell-redesign.md](ui-shell-redesign.md),D1–D5 按默认拍板 decisions-pending #30;M1–M5 = auto-plan 13a–13e):M1 自绘标题栏+三栏重排 `389dafb`、M2 左栏三段式 `8f07cd5`、M3 格式工具条 `b7c918b`、M4 禅定 `7fd24a6`、M5 收口(修复三连 `d797451`/`bff7405` + 明暗像素采样验收 11 断言 [m5-acceptance.md](m5-acceptance.md))**均于 2026-09-27 合入 main**(此前本行曾记「M1 已落地 2026-09-26」,以 git 合入日期为准订正);真机遗留(无边框拖窗/resize 于 Win/mac)见 acceptance-checklist §8 三项
[x] 图片框与图床已落地(2026-09-27)   A–D 四段全部合入 main:A 插入骨架 `ee694b6`(PR #46)、B 本地图片复制进 .assets/ 与相对路径预览出图 `1f96fb6`、C latermd-bed 图床 crate 与设置第五页 `eb9c38e`、D Ctrl+V/拖入图片直接落 .assets/ 插入 `c38084a`(均 2026-09-28 落盘);#21 随 D 段完结;真机验收项(自定义图床上传走通一次、Win/mac 剪贴板与拖入)留人工清单
[x] 表情符号(emoji)面板   规格 [emoji-plan.md](emoji-plan.md)(2026-09-27 坤哥指令),E1–E4 共 ~1.3d:E1–E3 已落地(auto-plan #28,见下行),E4(可选 短码补全)未做;#47 E-C1(2026-10-02)面板与最近使用条改随包 Twemoji 纹理彩色(见下行)。**实测两条硬事实**:egui 0.36.2 默认字体链已含黑白 Noto Emoji(零新增依赖即可渲染);但**彩色 emoji 上游不支持**(epaint 字形管线恒纯白填充),编辑器正文仍黑白、导出/外发的显示效果取决于目标环境的字体(口径核对见 [emoji-color-acceptance.md](emoji-color-acceptance.md) §3)
[x] 右键菜单      2026-10-13 落地(规划 [context-menu-plan.md](context-menu-plan.md),坤哥指令「都列入规划,开始做」):四模块——M1 编辑器(源码+Live 活动块:撤销重做/剪切复制粘贴全选走合成事件与 mac 原生菜单同源通道、右键先落光标[选区内保留/选区外塌缩,上一帧快照判定——egui 对 secondary press 原生塌缩选区]、格式高频五条直出+「更多格式」子菜单、查找/替换/跳转/插入、有选区时 AI 入口)、M2 文件树(打开/重命名[真实改名,打开中标签换绑+named_draft 迁移]/删除[trash 5.2.9 系统废纸篓,dirty 拒删 #144]/复制路径×2/文件管理器显示/新建文件夹)、M3 预览链接与图片(vendor ①类 `link_response`/`image_response`,协议链接不弹)、M4 标签栏补路径三条;四模块 4 commits,decisions-pending #143–#145,trash 依赖登记 distribution.md §6;门禁全绿;真机人工清单见规划 §5
[x] UI 现代化 U0/U1/U3 已落地(2026-09-27)   三 commits 合入(见 auto-plan #27):U0 设计 token 扩充(`RADIUS_MD`/`INPUT_H`/`INPUT_PAD_X`/`INPUT_PAD_Y`/`FONT_SM` 带来源注释与数值断言)+ 九套预设色板字面量编进 `theme_presets.rs` 启动铺盘 `themes/*.ron`;U1 Inter 三字重(Proportional 首位 + CJK 回退链保序 + SemiBold 上标题栏与格式条);U3 自研轻量动效(模式切换 crossfade 0.15s,浮层淡入由 egui 0.36 内建 Area fade-in 承接)。U2 即图片框(已另行落地);**U4 phosphor 迁移后实机回滚(2026-09-29,decisions-pending #35)**——拍板乙并落地(PR #55)当天真机目视 30 枚图标,观感不及定制自绘体系,拍板回滚甲(revert `ca6e164`),`ui/icons.rs` 维持自绘;phosphor 完整实现存档于 PR #55 与 archive tag 备重启。岔路口径见 #43/#44/#45,三 commit 的分支承载与 main 指针处置见 #46;真机观感项(36px 输入框比例、三平台 CJK 混排目视、动效目视)留人工
[x] Emoji 面板已落地(2026-09-27;#47 E-C1 2026-10-02 面板/最近使用条彩色——编辑器正文仍黑白,导出/外发取决于目标环境字体,验收 [emoji-color-acceptance.md](emoji-color-acceptance.md))
```

**本轮已提交模块（2026-09-24 ~ 09-25，11 commits）**：

| 模块                                                                 | commit    |
| ------------------------------------------------------------------ | --------- |
| M0 可自动化验证（[m0-report.md](m0-report.md)：中文字体注入 + bench 数据 + 六项验证状态） | `75871fa` |
| latermd-md 解析与大纲数据层                                                | `18ca4c0` |
| 三栏布局骨架（侧边栏/编辑器/预览 + State/Message 归约）                              | `33fa18f` |
| 源码编辑与实时预览                                                          | `a3f760f` |
| 文件新建/打开/保存/另存为                                                     | `6710669` |
| 大纲面板与光标跳转                                                          | `8c00cdd` |
| HTML 导出（latermd-export）                                            | `35f0bf0` |
| 主题切换与持久化（settings.json）                                            | `bf32495` |
| 统一快捷键与菜单栏                                                          | `b0bbc2c` |
| 文件树基础版（懒加载 + .gitignore + 当前文件高亮）                                  | `37c9e0c` |
| 修复：大纲跳转 span 过期越界钳制 + 保存/导出原子落盘                                    | `efedd6b` |

> P0 范围表对照：Markdown 与代码高亮已由 vendor 提供，上表已覆盖其余功能条目。
>
> **打包分发进展（2026-09-25）**：cargo-dist 0.33 已接入，`dist plan/build` 实测五目标齐备（Linux x64、macOS 双架构、Windows x64 + ARM64），资产名 `latermd-{triple}` 不含版本号；macOS universal2 dmg 走自建 job（`.github/workflows/macos-dmg.yml`：lipo + `.app` + hdiutil），cask 模板在 `packaging/latermd.rb`。**CI 首跑验证已过**(V1 核对表 R4/R13,证据 E2–E4/E7):2026-09-26 v0.0.1 首跑,09-29 / 09-30 两版复跑,三版 dmg job 均成功合成 universal2 dmg 并回传,windows-11-arm 原生 runner 首编通过;本机无 macOS 的部分(dmg 内 .app 真机启动)仍留人工(blocked_external)。
>
> **剩余**：M0 两条真机项（Win/mac IME、Win/mac wgpu；Linux IME 已首测 —— 输入可用、候选框不跟随，缺陷排查挂账见 m0-report.md 验证 1）与三平台真机安装/连续写作长跑人工段;打包首跑验证已完成(V1 核对表 R5,证据 E1–E4)。

> Vendor 适配实测纪要:15 个升级错误全部修复;另有 checklist 未预见的 **TexturesDelta drop 检查**(egui 0.36 新增)导致 10 个测试失败,已在测试中补 `output.textures_delta.clear()`。`source_span` 以**平行数组**形态落地(`Markdown { s, tokens, spans }`),不动 15 个 enum 变体,不变量 `spans.len() == tokens.len()` 有单测。差异全记录在 [vendor/egui\_markdown/README.md](../vendor/egui_markdown/README.md)。100 个测试全绿;`latermd-app` 空窗口在 Linux/X11 实际运行通过(wgpu adapter 正常)。

---

## 阶段 0：Vendor 适配（3-5 工作日）

**目标**：消除全部升级不确定性。

**动作**：

1. `git subtree add` vendor `membrane-io/egui_markdown`（含 `egui_markdown_style` 子 crate、`tests/`、`check.sh`）
2. 修 15 个真实的升级错误（另有 9 个是上游自身 bug，直接删）—— 清单见 [vendor-upgrade-checklist.md](vendor-upgrade-checklist.md)
3. **同步加入 `Token::source_span`**（这是唯一会碰 `Token` 定义的机会，同时服务大纲与 Live Preview）
4. 钉 `rust-toolchain.toml` 到 1.98.0

**验收**（与上游 `check.sh` 六项完全一致，一项不能少）：

```bash
cd vendor/egui_markdown
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --no-default-features -- -D warnings
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo doc --no-deps --all-features
```

全绿。（前提：已删除 `membrane` feature，否则第三轮 clippy 必失败 —— 见 vendor-upgrade-checklist §6。）

---


## 阶段 1：M0 技术验证（2 周）

**目标**：三条不过则不继续。这是**唯一允许失败便宜的阶段**。

| # | 验证项               | 验收标准                                                           | 关联 ADR       |
| - | ----------------- | -------------------------------------------------------------- | ------------ |
| 1 | **IME 中文输入**      | Win11 微软拼音 + macOS 14 简体拼音：候选框跟随 caret、不吞字、不抢焦点                | ADR-002 §6   |
| 2 | **长文档性能**         | 10 万字 md 滚动到中部 ≥ 55fps（验证视口剔除生效）                               | ADR-002 §3.5 |
| 3 | **wgpu 三 target** | Win11(DX12) / macOS14(Metal) / Linux(Vulkan) 均能启动且报告合理 adapter | ADR-002 §6   |

**补充建议（来自 P0 风险前移）**：

| # | 附加验证                | 目的                                                                                               |
| - | ------------------- | ------------------------------------------------------------------------------------------------ |
| 4 | **流式性能边界**          | mock LLM 每 100ms 喂一个 chunk，跑上游现成的 `bench_render_scroll_code_streaming`，记录 500 / 2000 / 10000 行帧率 |
| 5 | **中文渲染**            | 在 Deepin 上确认中文不显示方块；同时定案字体方案（`fontdb` / `font-kit` / cfg 原生）                                     |
| 6 | **tokio ↔ egui 通道** | `mpsc` + `request_repaint()` 往返无锁竞争                                                              |

第 4-6 项很便宜，但第 4 项是唯一有真实信息量的。

**载体约束**：M0 用 vendored crate 自带的 example / bench / 最小 `latermd-app` 窗口骨架做验证，**不写产品代码**。验证代码可以丢，结论不能丢。

**产出物（硬性）**：无论通过与否，M0 结束时必须新增 [docs/m0-report.md](m0-report.md)：三条主验证 + 三条附加验证的实测数据（帧率数字、adapter 信息、IME 行为记录）、结论（继续 / 换 iced 备选 / 停止）。不过则不进入 P0，没有例外。

---

## 阶段 2：P0 骨架（8-10 周）

**目标**：能写下一篇技术文档并交付给别人看。


### 范围

| 模块               | 内容                                                                                                                                            | 周期           |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------- | ------------ |
| 三栏布局             | `Panel::left` × 2 + `CentralPanel`，全部可调宽；侧边栏 `show_collapsible`                                                                               | 0.5 周        |
| 编辑器              | 双栏源码编辑 + 实时预览                                                                                                                                 | 2 周          |
| 文件操作             | 新建 / 打开 / 保存 / 另存为                                                                                                                            | 0.5 周        |
| Markdown         | CommonMark + GFM（表格、任务列表、删除线、脚注）                                                                                                              | 已由 vendor 提供 |
| 代码高亮             | syntect + 复制按钮                                                                                                                                | 已由 vendor 提供 |
| 导出               | HTML                                                                                                                                          | 1 周          |
| **主题系统**（专题批次 A） | token 化 `Theme` + Light/Dark 双内置皮肤，外壳/正文/代码高亮**三层联动** + 切换入口 + 持久化。详见文末「专题：界面美化与皮肤系统」                                                         | 1.5 周        |
| 快捷键              | `Ctrl` / `Cmd` 自动适配                                                                                                                           | 0.5 周        |
| **文件树**（基础版）     | `ignore` + 懒加载 + `.gitignore` + 点击打开 + 当前文件高亮                                                                                                 | 1.5 周        |
| **大纲**（廉价版）      | AST 提取标题 + 点击跳编辑器光标。**不跳预览**                                                                                                                  | 0.5 周        |
| 打包               | 三平台产物发 GitHub Release；macOS universal2 dmg 经 [crazykun/homebrew-ailater](https://github.com/crazykun/homebrew-ailater) cask 分发（复刻 lscreen 模式） | 1 周          |

> **分发依赖（非阻塞）**：无签名证书路线已定（AGENTS.md §5）。macOS cask 复刻 `lscreen` 写法（universal2 单 dmg + postflight 去 quarantine + livecheck）；接入 tap 的 auto-bump 流水线只需在其 `FORMULAS` 表 / `Casks/` 加一项，随时可办，无审批等待。


### crate 增量创建表（防止「开工即 8 个空 crate」）

ADR-001 §3 的 8-crate 结构是**终态**，不是开工指令。空 crate 骨架是过早抽象（违反 AGENTS.md §8）。按下表增量创建：

| 时机           | 新建 crate                                       | 理由                                                                                          |
| ------------ | ---------------------------------------------- | ------------------------------------------------------------------------------------------- |
| 阶段 0（Vendor） | workspace 根 + `latermd-app`（空窗口）               | M0 验证需要载体                                                                                   |
| P0 开工        | `latermd-md`、`latermd-editor`、`latermd-export` | 解析/token 层与 rope/光标层是三条铁律的落点，必须有独立边界；HTML 导出是 P0 验收项                                        |
| P1 开工        | `latermd-ai`                                   | provider trait 与流式                                                                          |
| P2 开工        | `latermd-git`                                  | 只读 Git                                                                                      |
| 出现第二个消费者时    | `latermd-render`、`latermd-core`                | render（绘制指令 IR）在只有 egui 一个后端时没有存在价值；core（状态机/DTO）的职责在 P0 由 `latermd-app` 承担，等 AI 消息总线复杂化后再抽 |

### 验收

- [ ] 三平台可安装
- [ ] 能连续写 1 小时技术文档不崩、不卡
- [ ] 导出的 HTML 可直接交付他人阅读
- [ ] `.md` 文件保持原样（无格式化篡改）

---


## 阶段 3：P1 差异化（+7.5-9.5 周）

**目标**：AI 成为产品的差异点，而非附加功能。

| 模块                    | 内容                                                                                                                                        | 周期    |
| --------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- | ----- |
| AI 流式写作               | `heal()` + `LinkHandler` + provider trait（OpenAI / Anthropic / Ollama）                                                                    | 3 周   |
| `ai://` 链接协议          | `.link_style()` + `.click()` 拦截                                                                                                           | 0.5 周 |
| AI 指令块                | `.is_block_code_widget()` → `.block_code_widget()`（原写法 `.is_block_widget()` 实测仅作用于 `Token::Link` 的 href、够不到围栏代码块，见 decisions-pending #12） | 1 周   |
| AI commit message     | —                                                                                                                                         | 0.5 周 |
| AI 摘要 / 大纲            | —                                                                                                                                         | 1 周   |
| **搜索**（即全文检索，P3 不再重复） | `ignore` + `grep-searcher` + `regex`，300ms 防抖 + 可取消 + 流式结果                                                                                | 1.5 周 |

### 关键约束

- **AI 流式预览必须用稳定 widget id**，不得含 `content.len()`（否则每个 token 清空临时缓存，禁用追加增量高亮）
- 所有 AI 修改在 token/AST 层操作，不操作渲染后的盒子
- `Mut` 操作通过 channel 回传，不在后台线程直接改 UI 状态

---

## 阶段 4：P2 版本层（+4.5-6.5 周）

**目标**：Git 成为可信的时间轴，**只做只读**。

| 模块         | 内容                                               |
| ---------- | ------------------------------------------------ |
| Git 只读集成   | 状态、历史、diff、blame、回滚                              |
| 文件树 Git 标记 | M / A / U / ? 与 `git2::statuses()` 联动            |
| 凭据管理       | 三平台封装（Credential Manager / Keychain / libsecret） |

**明确不做**：rebase、cherry-pick、LFS、submodule、多仓库。

---

## 阶段 4.5：界面打磨窗口（+1.5-2 周）

**目标**：皮肤系统批次 B——机制完整化（三态切换 + 自定义皮肤文件）+ 现代化视觉定稿。

**时机刻意放在 P3 之前**：Live Preview 是 3-4 周的深水区，必须在**视觉终态**上开发，否则皮肤系统后到会导致 Live Preview 的聚焦/半隐藏样式按旧视觉调一遍、换肤后再返工一遍。

范围、验收与「明确不做」见文末[「专题：界面美化与皮肤系统」](#专题界面美化与皮肤系统)。

---

## 阶段 5：P3 深水区（+8-10 周）

| 模块                  | 内容                         | 依赖                         |                                                                                                            |
| ------------------- | -------------------------- | -------------------------- | ---------------------------------------------------------------------------------------------------------- |
| **Live Preview**    | 光标所在 block 显示源码，其余富渲染      | Token source_span（阶段 0 已做） |                                                                                                            |
| 大纲预览跳转              | 复用现有 `section_to_token` 映射 | —                          | **订正(2026-10-03,auto-plan #42)**:原「2026-09-26 落地」失实——该版(`section_anchors()` 路径,fe286e7+4e78b7e)滚动指令写在 ScrollArea 闭包外,pass_state 滚动目标下一帧 begin_pass 被清空,从未生效(即用户 2026-09-30「目前就源码跳转了」反馈的根因,机制定位与修复见 decisions-pending #79);「复用 section_to_token 反查行 y」亦被实测推翻(epaint LayoutJob::append 合并同格式相邻 section 致两序列索引不平行)。真落地 = 2026-10-03 auto-plan #42:vendored ①类新增 BlockSpanRect 块表(block_span_rects/block_rect_at_offset,帧号键控),app 侧偏移穿透 wikilink/相对图片两层改写,在 ScrollArea 闭包内 scroll_to_rect_animation 居中一次到位,大纲点击源码+预览双栏跳;`section_anchors()` 旧 API 保留未删(app 消费已切走);真机目视留人工      |
| 双向链接 `[[wikilink]]` | 通过 `LinkHandler` 实现        | —                          | **2026-09-26 落地**：`latermd_md::expand_wikilinks` 只改渲染（源码不动），LinkHandler 拦 `wiki://`，按文件名在库内找文档并打开；反向链接面板未做 |

> 全文检索已并入 P1 侧边栏搜索，本阶段无检索条目。

### Live Preview 三块补工

1. `Token::source_span` —— **阶段 0 已完成**
2. 块级 caret 路由（↑/↓ 跨 block、Home/End、选区扩展）—— **2026-09-26 落地 v1**：`latermd_md::blocks` 切块（区间连续覆盖全文）+ `crates/latermd-app/src/live.rs` 的活动块编辑代理（`BlockBuffer` 落回同一 rope）；↑ 在块首去上一块末、↓ 在块尾去下一块首，Home/End 仍走 TextEdit 内建，选区扩展未做
3. 内联标记半隐藏 —— **v1 按「聚焦时整条源码裸出来」交付**（块级切换，不做 `**` 半隐藏）；v2 再做块内分段

### 一条不能违反的原则

> **源码模式与 Live Preview 必须共享同一个 rope buffer 和同一批渲染组件**，区别仅在于是否应用那条切换规则。
>
> 否则 Cmd+/ 切换会丢光标、丢 undo 栈。

---

## 专题：界面美化与皮肤系统

日期：2026-09-24（规划新增）  
状态：已接受  
**定位：美化服务于「长时间写作的舒适度」与产品门面，不是皮肤引擎项目。** 夜间/白天模式是长时间写作的护眼刚需（批次 A，P0）；皮肤机制与视觉定稿是品质项（批次 B，P2.5）。

### 能力盘点（已实测，勿重复调研）

vendored 层对明暗双模式是**原生支持**的，皮肤系统全程不动 vendor：

1. `MarkdownStyle` 的颜色字段本身就是成对设计（`color_dark` / `color_light`、`background_dark` / `background_light`），渲染时按 `dark_mode: bool` 自动取值 —— **正文层零成本联动**。
2. 代码高亮有 `.code_theme(&syntect::highlighting::Theme)` 扩展点（label.rs:435-441）；**不传则按 dark/light 自动选择**，批次 A 零成本联动，批次 B 才需要在皮肤文件里指定 syntect 主题名。
3. 布局缓存 hash 已含 `dark_mode` 与 code_theme，切换主题自动失效缓存，无陈旧渲染、无手写缓存清理。
4. UI 外壳是 egui 内建 `Style` / `Visuals`（自带 light/dark 基线），app 侧覆盖 token 即可。

### 三层联动模型

一个 token 化 `Theme` 结构（落 `latermd-app/src/theme.rs`，P0 **不新建 crate**）是皮肤唯一事实源，向三处投影：

```text
              ┌─ egui Style/Visuals         外壳:面板/控件/滚动条/间距/圆角/悬停态
Theme tokens ─┼─ MarkdownStyle + dark_mode   正文:标题/表格/引用/行内代码
              └─ syntect code_theme          代码块高亮(批次 A 用自动跟随)
```

token 只做**语义级**：背景层级（surface / surface_alt）、前景两级、accent、边框、选区、链接色 + 字号阶梯 + 间距阶梯 + 圆角半径。不做每控件粒度的样式树。


### 分批交付

| 批次 | 内容                                                                                                                                                                 | 落点                        | 周期       | 验收                                                        |
| -- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------- | -------- | --------------------------------------------------------- |
| A  | Light/Dark 双内置皮肤全量联动；切换入口（Settings，P0 交付时 Settings 面板顺带落地）；选择持久化用 **eframe 内建 persistence**（`App::save`，不加新依赖）；切换走 `Message::ThemeChanged` 归约（adr-005 logic/ui 二分） | **并入 P0**（主题条目 1 → 1.5 周） | 1.5 周    | 切主题时外壳/正文/代码块**同帧**换肤无闪变；重启保持；两套皮肤下正文与代码对比度均可读（含中文）       |
| B  | 三态：亮 / 暗 / 跟随系统（`dark-light` 检测，Linux 失灵则回退手动两态）；自定义皮肤文件（`Theme` serde 导出 RON 至用户配置目录 `themes/`，内置皮肤可导出为模板）；现代化视觉打磨：间距/圆角统一 token、滚动条、hover/active 态、编辑器行距与 gutter | **阶段 4.5 打磨窗口**           | +1.5-2 周 | 复制皮肤文件改一个颜色重启生效；跟随系统在 Win11/macOS 14 实测联动；Linux 记录检测行为进文档 |
| C  | 皮肤分享/市场、CSS 式主题引擎、每控件自定义、True WYSIWYG 排版自由                                                                                                                         | **明确不做**                  | —        | —                                                         |

### 明确不做

- 皮肤包市场 / 在线分享 —— 另一个产品。
- 运行时热重载样式引擎 —— egui 是立即模式，切皮肤 = 换 token 表重投影，本就同帧生效，「热重载引擎」是伪需求。
- 每控件粒度自定义 —— 维护成本随控件数线性增长，收益趋零。

### 关联风险

见风险登记册 #8（Linux 跟随系统主题无统一规范）。

---

## 非目标（永久）

写进 README，避免范围蔓延：

- True WYSIWYG（Word / Notion 形态）
- 知识图谱、语义搜索、MOC
- 多模态嵌入
- rebase / cherry-pick / LFS / submodule / 多仓库
- 小说助手、闪卡、日记洞察

> 判断标准：如果一项功能不能让「写下一篇技术文档」变得更快，它就不在 P0-P2。

---

## 周期汇总

| 阶段        | 周期         | 累计    |
| --------- | ---------- | ----- |
| Vendor 适配 | 3-5 工作日    | ~1 周  |
| M0 验证     | 2 周        | ~3 周  |
| P0 骨架     | 8-10 周     | ~13 周 |
| P1 差异化    | +7.5-9.5 周 | ~22 周 |
| P2 版本层    | +4.5-6.5 周 | ~28 周 |
| P2.5 界面打磨 | +1.5-2 周   | ~30 周 |
| P3 深水区    | +8-10 周    | ~40 周 |

约 **9.5 个月**（单人，全职）。仅供规划参考 —— 实际会随 M0 结论调整。

---


## 风险登记册

| # | 风险                                                                          | 等级 | 缓解                                                                                           | 触发信号                           | 应对                                      |
| - | --------------------------------------------------------------------------- | -- | -------------------------------------------------------------------------------------------- | ------------------------------ | --------------------------------------- |
| 1 | **IME 缺陷**（吞字 / 候选框不跟随 / 抢焦点）                                               | 高  | M0 第 1 条验证;Linux 已首测(2026-09-25,X11 + fcitx5):输入可用、候选框不跟随 —— 非放行线失败,故障排查中(m0-report.md 验证 1) | M0 实测不过                        | 停止或换 iced 备选（ADR-001 §2.5），不硬修 egui     |
| 2 | **单人 9 个月工期**                                                               | 高  | 每阶段出口评审；P0 完成即有自用价值                                                                          | 连续两阶段超期 50%                    | 砍 P2/P3 范围，P1 收敛为「流式写作」单功能              |
| 3 | **无签名的信任门槛**（Gatekeeper「已损坏」/ SmartScreen 警告）                               | 低  | brew cask `postflight` 去 quarantine；README 写明 Windows「仍要运行」指引                                | 非 brew 用户首次打开受阻                | 已是 lscreen 验证过的成熟路径，不新增投入               |
| 4 | **上游 egui_markdown 停更**                                                     | 中  | 已 vendor（subtree），不受上游发布节奏约束                                                                 | 上游 6 个月无提交                     | fork 并公开维护；升级决策权完全在我们                   |
| 5 | **CharIndex 强类型重构引入 off-by-one**                                            | 中  | vendor 升级后 `cargo test` 必跑（checklist §8 第 4 步）                                               | tests/cache、indent、truncate 回归 | 以测试为准逐个修，不带病合并                          |
| 6 | **egui 上游 churn**（季度级破坏性发版）                                                 | 中  | 钉 0.36.2；升级是显式 ADR 决策，不追新                                                                    | 安全公告 / 必需 bugfix               | 单独开升级 ADR，走 vendor-upgrade-checklist 流程 |
| 7 | **cask 停更**（lscreen 曾因 dmg 资产命名变化导致 cask 停在旧版）                              | 中  | `livecheck :github_latest` + tap 的 auto-bump 流水线；Release 资产命名在 cargo-dist 配置中钉死，不随版本改格式      | 用户 brew 拿不到新版本                 | 命名变化时同步改 cask URL 模板（一次 5 分钟）           |
| 8 | **Linux 跟随系统主题无统一规范**（GNOME gsettings / KDE / Wayland portal 各异，检测库可能失灵或滞后） | 低  | 跟随系统默认关闭，亮/暗/跟随三态中手动优先；Win/mac 是主要实测对象                                                       | 皮肤批次 B 在 Deepin/Linux 实测检测失败   | 该平台回退手动两态，不阻塞交付                         |

---

## 阶段出口条件（维护规则）

每个阶段完成时，**必须**做三件事，否则视为未完成：

1. 更新本文件「当前位置」的 checkbox 与日期。
2. 在 docs/README.md 修订记录表追加一行。
3. 产出该阶段的验收证据（M0 = m0-report.md；Vendor = check.sh 六项全绿截图或日志；P0+ = 三平台安装包 + 验收清单勾选）。

---

## 持续项：跨平台 CI

```yaml
strategy:
  matrix:
    os: [ubuntu-latest, macos-latest, windows-latest]
steps:
  - uses: actions/checkout@v4
  - uses: dtolnay/rust-toolchain@1.98.0    # 不能用 @stable
  - run: cargo test --release
  - run: cargo build --release
  - run: cargo dist build
```

追加 `aarch64-pc-windows-msvc` 发布目标。

> 以上为**结构示意**。落地时以 `cargo dist init` 生成的 workflow 为准（它产出的 `--target` 矩阵、缓存与签名步骤是官方维护的），并追加 `Swatinem/rust-cache`。vendor 的 `check.sh` 六项门禁需接入 CI。
