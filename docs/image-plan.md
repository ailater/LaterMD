# 图片插入与图床规划（image-plan）

日期：2026-09-27
状态：**待坤哥放行**（规划态，未写 `src/`）
关联：[AGENTS.md](../AGENTS.md) §2 技术栈 / §7 范围边界、[auto-plan.md](auto-plan.md) #21 image-paste、
[ui-shell-redesign.md](ui-shell-redesign.md) §6 工具条、[adr-004](adr-004-technical-stack.md)

> 坤哥 2026-09-27 指令：「加入图片框功能规划，可以添加图片地址，或者上传图片，
> 设置里面添加图床功能，可以添加自定义的图床，上传图片」。
> 本文把它拆成可排期的四段，并写明**哪些不做**。

---

## 1. 现状

| 事实 | 位置 |
|---|---|
| 格式工具条 16 个动作，**没有 Image** | `compose.rs` `FormatAction`、`ui/format_bar.rs` |
| 图片渲染走 vendored 层，`egui::Image::new(url)`，**相对路径不解析** | `vendor/egui_markdown/src/label.rs:844` |
| HTTP 栈只有 `ureq 3.4.2`（`rustls`，**无 multipart**），在 `latermd-ai` | `crates/latermd-ai/Cargo.toml:11` |
| 凭据存系统钥匙串，**已有现成接口** | `latermd-creds` `set_secret` / `get_secret` |
| 设置对话框四页，**没有图床页** | `settings.rs` `SettingsTab{Appearance,Keymap,Ai,Mcp}` |
| auto-plan #21 已排 image-paste（粘贴/拖拽），与本文重叠 | `auto-plan.md` |

**与 #21 的关系**：#21 只做「本地落盘 + 相对路径」；本文在它之上加「URL 直填」与「图床上传」，
并把两者收进同一个「图片框」对话框。**本文落地后 #21 并入 C 段，不再单独排队。**

---

## 2. 目标形态：一个「图片框」对话框，三种来源

点击工具条「图片」按钮弹出，三个页签：

| 页签 | 输入 | 产出 | 落文档的 URL 形态 |
|---|---|---|---|
| **网络地址** | URL 文本框 + alt 文本框 | 直接插入 | 原样 `https://…` |
| **本地文件** | 文件选择（rfd） | 复制到 `<doc名>.assets/` | 相对路径 `./<doc名>.assets/x.png` |
| **图床上传** | 选文件 + 选图床 profile | 上传后回填 URL | 图床返回的绝对 URL |

三条来源在插入环节**汇成同一个动作**：`compose::insert_image(text, sel, url, alt)`。
对话框只负责产出 `(url, alt)`，不碰文本——与 `format_bar`「点击只发消息」同款分工。

---

## 3. 分期

### A 段：插入骨架（纯函数 + 按钮 + 对话框）— 0.5d

1. `compose.rs` 新增 `FormatAction::Image` 与 `pub fn insert_image(text, sel, url, alt)`。
   - 语义对齐 `Link`：新选区**落在 alt 位**（便于直接覆写），不是落在整段上。
   - 理由：图片 alt 是给人读的，比 `[]()` 的标题更需要立刻填。
2. `ui/icons.rs` 新增 `Icon::Image`（自绘：矩形 + 山 + 日，遵守 ui-polish §1.1）。
3. `ui/format_bar.rs` 把 Image 挂进 **Block 组**（引用/代码块/分割线/表格/图片）。
4. `Command::ImageInsert` + 键位 `Cmd/Ctrl+Shift+I`（`Ctrl+I` 已是斜体）。
5. 新 `ui/image_dialog.rs`：alt + URL 双输入、插入/取消。

**A 段验收**
- 单测：`insert_image` 在空文档/有选区/行内/行尾四种情形的文本与选区；
  URL 含空格或中文时**不转义**（Markdown 允许 `<>` 包裹，首期不做自动包裹，写明）。
- 工具条 17 枚渲染不 panic（明暗两套 visuals）。
- 六项门禁全绿。

### B 段：本地文件来源 — 1d

1. 复制目标目录 `<doc目录>/<doc名>.assets/`，文件名 `原名`（冲突时 `-1` `-2` 后缀）。
2. 文件树**过滤** `.assets` 目录本身（与 auto-plan #18 draft 过滤同款手法）。
3. **预览相对路径解析**（本段唯一硬骨头，见 §4.1）。

**B 段验收**：插入本地图片后预览能出图；换一台机器打开同目录仍出图（证明相对路径可移植）。

### C 段：图床 — 2d

新 crate **`latermd-bed`**（前缀合规 AGENTS §8，**不 import egui**，对齐铁律 2）。

```rust
/// 一个图床配置。serde 落 beds.json，与 settings.json 同目录。
pub struct BedProfile {
    pub id: String,          // uuid，也是 creds 的 account 键
    pub name: String,        // 用户可见名
    pub api_url: String,
    pub file_field: String,  // multipart 表单字段名（SM.MS=smfile，Lsky=file）
    pub headers: Vec<(String, String)>,  // 值里可写 ${TOKEN}
    /// 返回 JSON 里取 URL 的**点分路径**，如 "data.url"
    pub url_path: String,
    pub url_prefix: Option<String>,      // 有的图床返回的是路径不是完整 URL
}

pub trait BedUploader { fn upload(&self, p:&BedProfile, bytes:&[u8], name:&str) -> Result<String, BedError>; }
```

决策要点：
- **不引 jsonpath crate**：`url_path` 用手写点分取键（零依赖，够用）。
- **token 一律走 `latermd-creds`**（service=`latermd-bed`，account=profile id），
  **绝不落 beds.json 明文** —— 与 decisions-pending「凭据不含在配置里」同口径。
- **HTTP 复用 `ureq`**（已在依赖树），`latermd-bed` 开 `multipart` feature；
  **不引入 reqwest**，避免第二套 HTTP 栈 + tokio 传染。ADR-004 登记 `ureq` 的 multipart feature。
- 预置 profile：SM.MS / GitHub（Contents API，base64）/ 自定义。其余（七牛/OSS/COS）签名算法各异，
  **首期只给「自定义」表单**，让用户自己填，不做各家 SDK。

上传链路（**不阻塞 UI**，照抄 AI 流式防重入手法）：
```
Message::ImageUploadRequested { profile_id, path }
   → 归约里 spawn 后台线程（ureq 阻塞式）
   → Message::ImageUploadFinished { seq, result: Result<String,String> }
   → 只接受 seq 等于当前最新序号的结果（防旧请求覆盖）
   → 拿 URL 调 compose::insert_image 插入
```

设置页新增第五页 `SettingsTab::Image`（「图片」）：profile 列表 / 新增 / 编辑 / 删除 / **测试上传**。

**C 段验收**
- `latermd-bed` 单测：点分路径抽取、headers 的 `${TOKEN}` 替换、表单字段名拼装（不含真实网络）。
- 真机：配一个自定义图床走通一次上传，URL 正确回填并插入。
- beds.json 里 grep 不到 token 明文。

### D 段：粘贴 / 拖拽 — 0.5d（并入 #21）

- `Ctrl+V` 图片字节 → 落 `.assets/` → 插相对路径。
- 拖入图片文件 → 同上。
- 白名单 PNG/JPEG/WebP/GIF，超 5MB 提示（沿用 #21 既定口径）。

---

## 4. 三个已知坑（先写下来，别到时候现踩）

### 4.1 相对路径在预览里不解析

`vendor/egui_markdown/src/label.rs:844` 直接 `egui::Image::new(url)`，
egui 的 `ImageSource::Uri` **不做相对路径解析**（没有文档目录概念）。
文档里存相对路径是对的（可移植），但喂给预览前必须拼成绝对。

**推荐解法（零 vendor 改动）**：富渲染前把源码里的相对图片路径替换成 `file://` 绝对 URI
（图片数量级十几个，字符串替换成本可忽略）。**2026-10-15 订正**：原写「`ui/preview.rs` 渲染前」；
`ui::preview` 已随 live-unify P4/P5 删除，改写现在由
`live::rich_block_with_base(index, block_text, base_dir)` 的第 5 层完成（`ui::rich_widgets`），
写作档正文与分栏档右栏共用同一份实现，结果纳入 `rich_cache`（缓存键含 `base_dir` 维度）。
**不推荐**：为此改 vendor（要按 AGENTS §6 三类拆分，成本高，且这是"上游可合"类但上游未必收）。
在 `vendor/README.md` 登记为「待上游化：给 Token::Image 加 base_dir」。

### 4.2 插入会打碎 TextEdit 内建 undo

与 `compose.rs` 文档头已记的 §9 R3 同款：走本模块的写入，Ctrl+Z 可能一次回退整次插入。
**已知并接受**，单测只钉「文本与选区正确」，不钉 undo 粒度。

### 4.3 图床上传失败不能吞掉用户内容

上传失败的**唯一**后果是不插入文本，**绝不能**改动用户已有的选区或文档内容。
`ImageUploadFinished(Err(_))` 只弹一条 notice（复用 `DocumentState::notice`）。

---

## 5. 明确不做（首期）

| 不做 | 理由 |
|---|---|
| 各家对象存储 SDK（OSS/COS/七牛） | 签名算法各异，是另一个工作量级；「自定义」表单已能覆盖 |
| 图片编辑（裁剪/压缩/水印） | 超出 Markdown 编辑器范围，AGENTS §7 |
| 图床返回的图片做本地缓存 | 首期直接取远端；缓存是独立优化项 |
| 自动把 `Ctrl+V` 的剪贴板图片**默认上传图床** | 默认行为必须可预测。默认落本地 `.assets/`，上传要用户显式选 |
| 图床 profile 的导入导出（PicGo 配置兼容） | 锦上添花，放二期 |

---

## 6. 排期与依赖

```
A 插入骨架 (0.5d) → B 本地文件 (1d) → C 图床 (2d) → D 粘贴拖拽 (0.5d)
                                        ↑
                              依赖 latermd-creds（已有）、ureq multipart（新增 feature）
```
合计 4d。A 段可以独立先交付（不依赖图床），建议**先放行 A 段**看效果。
