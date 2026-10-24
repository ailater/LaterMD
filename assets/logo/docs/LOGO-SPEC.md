# LaterMD 图标规范

> **v2.0 · 2026-10-09 · 标识更换为 AIM 方案**（设计：叶可儿）
>
> v1.x 的「A/I/M/D 四重读法」标识已**整体退役**，源文件与交付物均被替换。
> 本文档按新标识重写；v1.x 的可复用部分（交付流水线骨架、icns 容器格式、
> 使用禁忌）沿用并扩充，历史决策见文末「修订记录」。

---

## 一、设计概念

**一句话**：圆角渐变方块内一枚白色 **MD** 字形，右上一颗四角星芒。

| 读法 | 由哪一部分读出 | 含义 |
|---|---|---|
| **M** | 主字形左侧 | Markdown / MD |
| **D** | 主字形右侧碗形 | Document，Markdown 的 D |
| **星芒** | 右上角四角星 | AI 智能层的「闪烁」暗示；同时给纯几何的 MD 一个视觉呼吸点 |

与 v1.x 的根本差别：v1.x 追求「一笔多义」，v2 **放弃合体字**，改用**两字母直读 + 装饰星芒**。代价是失去 A/I 的双重读法，换来的是**任何尺寸下都能一眼读出 MD**——这是本次更替的核心理由（v1.x 在 32px 下谷底并笔，见文末修订记录）。

---

## 二、构图与色彩

| 项目 | 数值 |
|---|---|
| 画布 | 1024 × 1024 |
| 圆角半径 | 225（占 22.0%） |
| 渐变方向 | 45°（左上 → 右下），linear 单段 |
| macOS iconset 内容边长 | 824（= 1024 × `MACOS_INNER_SCALE`，四周 100px 透明边距，见下） |

| 角色 | HEX | 使用位置 |
|---|---|---|
| 渐变起点 | `#199CFD` | 左上角（青蓝） |
| 渐变终点 | `#0C28CA` | 右下角（深蓝） |
| 符号主色 | `#FFFFFF` | MD 字形 + 星芒 |
| 单色版（浅底） | `#0B1220` | `latermd-mark-black.svg` |
| 单色版（深底） | `#FFFFFF` | `latermd-mark-white.svg` |

单段渐变是刻意的：多段渐变在低端屏会产生色带。

> **macOS 网格留边（2026-10-24）**：源 SVG 是全出血构图，但 macOS 的
> Dock / 访达按**画布**等比摆位，全出血直渲的 AppIcon 在 Dock 里比同排
> 其他应用明显大一圈（真机反馈）。故 `build-deliverables.py` 对
> `AppIcon.iconset` 十档按 Apple Big Sur 图标网格缩放居中：内容
> 824×824、画布 1024×1024。其他平台（Windows ICO / Linux hicolor /
> web favicon / 窗口图标）维持全出血 —— 那些系统按内容本身摆位。

---

## 三、素材文件

```
assets/logo/
├── latermd-icon.svg           主稿（渐变方块 + MD + 星芒）
├── latermd-mark.svg           仅字形，无底板（需自绘底时用）
├── latermd-mark-black.svg     单色 #0B1220（浅底）
├── latermd-mark-white.svg     单色 #FFFFFF（深底）
├── build-deliverables.py      交付流水线（唯一产出入口）
└── deliverables/              脚本产出，勿手改
```

**改素材只改上面 4 个 SVG**，然后重跑 `python3 build-deliverables.py`；不要单独手改 `deliverables/` 下的任何文件——下次跑脚本会被覆盖。

### 交付清单

```
deliverables/
├── png/                      icon-16 / 24 / 32 / 36 / 48 / 64 / 96 / 128 / 192 / 256 / 512 / 1024
├── svg/                      4 个源 SVG 的副本
├── macOS/
│   ├── AppIcon.iconset/      10 文件（Apple 规定的命名，勿改）
│   └── AppIcon.icns          预合成（跨平台副本，见下）
├── windows/latermd.ico       6 档复合（16/32/48/64/128/256）
├── web/                      favicon.ico + favicon.svg + apple-touch-icon.png
└── android/res/mipmap-*/    mdpi → xxxhdpi
```

**24 / 36 两档是为 Linux 装的**：hicolor 主题目录按尺寸分档，缺档会让 `linux-deb.yml` 的 `install` 当场失败。删档前先看那边。

---

## 四、icns 为什么有两份

`AppIcon.iconset`（10 个 PNG）与 `AppIcon.icns` 同时入库：

- **macOS 构建走系统 `iconutil`**（`packaging/macos/bundle.sh`，CI 与本地共用），产出最权威；
- **`AppIcon.icns` 是脚本直拼的跨平台副本**，让「产物是否入库」「icns 能否在无 macOS 环境复现」变成可验证的事，而不是只能等发版时才发现漏了。

直拼格式（`build-deliverables.py::build_icns`）：

```
'icns' + u32BE(总长) + 重复的 [ 4 字节 OSType + u32BE(数据长 + 8) + PNG 数据 ]
```

两块易错点：

1. **长度字段含自身 8 字节头**（不是纯数据长）—— 写错则 Cocoa / `iconutil` 解析错位，图标整个不认；
2. **OSType 与尺寸的对应**：16/32/64 → `icp4/5/6`，128 以上 → `ic07/08/09/10`。macOS 10.7+ 全部接受 PNG 负载，不必走更早的 `arnnd`/`ic04` 路线。

改完素材后**两个都要重生成**，别只更新 iconset。

---

## 五、谁在运行时读哪张图

这张表是排错时的第一站——「图标改了没生效」九成是找错了文件。

| 场景 | 消费方 | 读哪个文件 |
|---|---|---|
| 运行时窗口图标（任务栏 / 标题栏 / Alt-Tab） | `main.rs` `viewport_builder` | `png/icon-64.png`（`assets.rs` `include_bytes`） |
| **窗口左上角品牌标识** | `ui/titlebar.rs` | `png/icon-256.png`（`assets.rs` `brand_logo_image`） |
| Windows exe 文件图标（资源管理器 / 任务管理器 / 开始菜单） | `build.rs` 经 `winresource` | `windows/latermd.ico` |
| macOS Dock / 访达 | `Info.plist` 的 `CFBundleIconFile` → `bundle.sh` 的 iconutil | `macOS/AppIcon.iconset` |
| Linux deb 装出来的启动器条目 | `.desktop` 的 `Icon=latermd` → icon-theme | `png/icon-{16,24,32,48,64,128,256,512}.png` |
| GitHub README / 文档站 | 相对路径 | `png/icon-256.png` |

**运行时窗口图标 ≠ exe 文件图标。** 前者走 eframe 的 `ViewportBuilder::with_icon`（WM_SETICON），后者读 exe 自身资源段。两者互不替代，只做一个就会出现「任务栏有图标，资源管理器里是毛坯」。

**macOS 图标文件在包里 ≠ 图标生效。** `CFBundleIconFile` 键必须同时在——只拷 icns 不给键，Cocoa 不会去读它，症状与没拷完全一样。

**Linux 装了二进制 ≠ 应用列表里能看见。** 必须有 `.desktop`；且它必须过 `desktop-file-validate`，不合规的 desktop 文件会被启动器**静默丢弃**，症状与「文件没装进去」一模一样。

---

## 六、标题栏标识为什么用 256 存、18px 画

标题栏里 logo 只有 18px（`tokens::BRAND_LOGO`）。素材按 **2x 上采样**存 256 再缩到 18 绘制，是为了在 1.5x / 2x 屏上不过采样出锯齿。

18px 这个值不是随手取的：16px 下「MD」两字母糊成一条，18px 是仍能读出的最小档。

若将来要改这个尺寸，**先跑红绿验证**：`titlebar::tests` 下有三条守门测试钉住几何与接线，其中「标题起点跟随标识右缘且不压图」会在你把文字起点写死时红。

---

## 七、使用禁忌

- **不要**拉伸非等比；圆角必须保持正圆角。
- **不要**给图标加投影 / 外发光——扁平渐变是刻意的，加投影立刻变廉价。
- **不要**在四周再套一圈描边，会与圆角边界打架。
- **不要**把字形从圆角方块里抠出来单独用作应用图标——渐变底板是背景色的锚，抠出后在浅色背景会失去对比。要单色请用 `latermd-mark-*.svg`，并**自己提供底**。
- **禁止**彩虹渐变、彩虹描边、立体透视。

---

## 修订记录

- **v2.0（2026-10-09）**：标识整体更换为 AIM 方案（`aim-icon.svg` / `aim-mark*.svg`），v1.x 的「A/I/M/D 四重读法」稿退役（源文件与全部交付物被替换，`.bak` 亦不再保留——git 历史即存档）。同时：`build-deliverables.py` 重写为跨平台自足版（新增 icns 直拼、Windows ICO 六档、Linux hicolor 档位补 24/36；修掉旧版把 32px PNG 改名成 `favicon.svg` 的错）；新增 `packaging/linux/latermd.desktop`；`crates/latermd-app/build.rs` 新增 Windows exe 资源嵌入。三平台接线与踩坑见 `docs/adr-004-technical-stack.md` 2026-10-09 修订行。
- **v1.1（2026-09-27）**：小尺寸打磨——删除右上小星、主星加大内收、新增 `-simple` 变体、16/32/48px 改由简化版导出。**该方向后来被证明没走通**：简化版在 32px 下谷底仍并笔，四重读法始终读不出来，最终促成 v2 换成直读的 MD。
- **v1.0（2026-09-26）**：初版，「A/I/M/D 四重读法」。