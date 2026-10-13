# 分发与发布 runbook(cargo-dist + GitHub Release + Homebrew cask)

> 本文是发布操作的**唯一事实来源**。配置侧的事实源分别是:
> [dist-workspace.toml](../dist-workspace.toml)(dist 目标矩阵)、
> [.github/workflows/release.yml](../.github/workflows/release.yml)(dist 生成 + 三处 LOCAL PATCH,见 §1)、
> [.github/workflows/auto-tag.yml](../.github/workflows/auto-tag.yml)(自动打 tag,2026-09-26 接入)、
> [.github/workflows/macos-dmg.yml](../.github/workflows/macos-dmg.yml)(自建 dmg job)、
> [.github/workflows/linux-deb.yml](../.github/workflows/linux-deb.yml)(自建 deb job,2026-09-29 接入)、
> [packaging/latermd.rb](../packaging/latermd.rb)(cask 模板)。
> 与本文冲突时,以配置文件为准并回改本文。

## 1. 发布链路总览(2026-09-26 起全自动)

发版动作 = PR 里 bump `Cargo.toml` 的 `workspace.package.version`(建议同 PR 带
CHANGELOG.md 小节)→ 合入 main。此后无人值守:

```
PR 合入 main(版本号已 bump)
        │
        ▼  rust.yml 门禁跑绿
        │  auto-tag.yml(workflow_run 监听 Rust conclusion=success)
        │  读 Cargo.toml 版本 → 无对应 tag 则打 v{version} → dispatch
        ▼  gh workflow run release.yml --ref <tag>   ← workflow_dispatch
┌─────────────────────────────────────────────┐
│ release.yml(cargo-dist 0.33.0 生成)          │
│ plan → build-local(4 job,5 目标)             │
│   macos-14:双 darwin target 单 job(merge-tasks)│
│   ubuntu-22.04:linux x64                     │
│   windows-11-arm 原生:win ARM64(ring 汇编,   │
│   xwin 容器编不过,见 dist-workspace.toml 注) │
│   windows-2022:win x64                       │
│ → host:gh release create + 上传全部资产        │
│   (正文 = CHANGELOG.md 对应版本小节)          │
└─────────────────────────────────────────────┘
        │  host job 尾部 dispatch(非 on: release 事件)
        ▼  gh workflow run macos-dmg.yml / linux-deb.yml -f tag=<tag>
┌─────────────────────────────────────────────┐
│ macos-dmg.yml(自建 job,macos-14)             │
│ checkout(取 packaging/macos/Info.plist)      │
│ → 下载双架构 tar.xz → lipo 合一 → 组装 .app    │
│ → hdiutil 合 dmg → sha256 写 step summary     │
│ → gh release upload 回传同一 Release           │
│ → 尾部清理冗余资产(mac 每架构 tar.xz 等,见 §2) │
│ → 回填 tap cask(cask-bump 步,失败仅告警)      │
└─────────────────────────────────────────────┘
┌─────────────────────────────────────────────┐
│ linux-deb.yml(自建 job,ubuntu-22.04)         │
│ 下载 linux tar.xz → 取二进制 → dpkg-deb 组包   │
│ → gh release upload 回传同一 Release           │
└─────────────────────────────────────────────┘
```

**为什么 deb 也走自建 job**:cargo-dist 0.33 的 installer 白名单是
shell/powershell/npm/homebrew/msi/pkg,**没有 deb**,`installers = ["deb"]`
直接 TOML 解析报错。deb 与 dmg 同构:不重新编译,取 dist 已产出的
tar.xz 里的二进制组包(deb 用 dpkg-deb,dmg 用 lipo + hdiutil)。
Linux 分发渠道 = tar.xz(热链)+ deb(装包),两者并存,deb 不是替代品。

**为什么 macos-dmg 尾部要清理资产**:macOS 分发只走 universal2 dmg
(AGENTS.md §5),每架构 tar.xz 只是 dmg 的 lipo 原料;dist 的
source.tar.gz 与 GitHub 自动生成的 source 归档重复。dist 没有产物
白名单/排除配置,只能发布后清理(`gh release delete-asset`)。已知代价:
`sha256.sum` 是 dist 生成的全量清单,仍含已删条目,接受。

**为什么链路必须显式 dispatch(GitHub 防递归规则)**:workflow 用 GITHUB_TOKEN
推的 tag、建的 Release,产生的事件**不会触发新的 workflow run**(防无限递归),
唯一例外是 `workflow_dispatch` / `repository_dispatch`。因此 auto-tag 推完 tag
必须 `gh workflow run release.yml`;release.yml host job 建完 Release 必须
`gh workflow run macos-dmg.yml`(macos-dmg 的 `on: release` 保留给人工/PAT 场景)。
`release: published` 事件在自动链路中**永远不会到达** —— 不要把链路改回事件驱动。

**release.yml 的三处 LOCAL PATCH**(dist 模板之外的增量,`dist generate`
重新生成时会被覆盖,必须重新打上):
1. `on:` 增加 `workflow_dispatch`(auto-tag 的 dispatch 入口);
2. `permissions` 增加 `"actions": "write"`(host job 要 dispatch macos-dmg);
3. host job 尾部「Dispatch macOS dmg build」步骤(一个步骤内先后
   dispatch macos-dmg.yml 与 linux-deb.yml)。

**LOCAL PATCH 的前置条件**:`dist-workspace.toml` 里 `allow-dirty = ["ci"]`。
dist 0.33 运行时会校验 release.yml 与生成模板逐字节一致,任何补丁都会触发
「has out of date contents and needs to be regenerated」硬错误退出
(2026-09-26 v0.0.1 首发实测踩中);`["ci"]` 精确放行这一类校验。
**若移除补丁回归纯模板,应同时移除 allow-dirty**,恢复漂移检测。

**版本策略**:tag 号永远取自 Cargo.toml(workspace.package.version),
auto-tag 不自行递增 —— dist 强制 tag 与包版本一致,自动改号必失配;
bot 直推 main 改版本号会被分支保护拦截。版本号不变地合入 main 不发版
(tag 已存在,auto-tag 秒跳过)。

- **tag 格式**:`v0.1.0` / `latermd/0.1.0` / `releases/v1.0.0` 均可触发(模式
  `**[0-9]+.[0-9]+.[0-9]+*`);带 `-beta.1` 等预发布后缀时 GitHub Release 自动标记
  prerelease。tag 必须是三段 SemVer。
- **PR 也会触发 release.yml**,但默认 `pr-run-mode = "plan"` 只跑 dist plan 校验,
  不构建不发布,与「CI 只在 main 跑」(rust.yml)不冲突。
- **CI 工具链**:release.yml 不安装任何指定版本工具链,rustup 由仓库根
  `rust-toolchain.toml`(channel = "1.98.0")接管——AGENTS.md §4 的钉法就是
  这个文件,**不要**给 dist 配置加 `rust-toolchain-version`(已弃用)。
- **产物不含 glow**:所有构建只走默认 feature(wgpu)。`LATERMD_RENDERER=glow`
  仅是运行时逃生口(AGENTS.md §5)。

## 2. 资产命名规范(钉死,不随版本变格式)

| 资产 | 产生方 | 命名 | 版本号 |
|---|---|---|---|
| Linux | dist | `latermd-x86_64-unknown-linux-gnu.tar.xz` | **无**(官方刻意,`releases/latest/download/` 热链可用) |
| Linux deb | linux-deb.yml | `latermd_{version}_amd64.deb` | **有**(Debian 惯例,文件名内嵌版本) |
| macOS x64 | dist | `latermd-x86_64-apple-darwin.tar.xz` | 无;**发布后清理**(仅 dmg 原料) |
| macOS arm64 | dist | `latermd-aarch64-apple-darwin.tar.xz` | 无;**发布后清理**(仅 dmg 原料) |
| Windows x64 | dist | `latermd-x86_64-pc-windows-msvc.zip` | 无 |
| Windows x64 裸 exe | release.yml host 尾部 | `latermd.exe` | 无(从 x64 zip 提取补传) |
| Windows 安装器 | release.yml windows-installer job | `latermd-{version}-setup.exe` | **有**(Inno Setup,同 GLMeter 形态) |
| macOS universal2 | macos-dmg.yml | `latermd-v{version}-universal2-apple-darwin.dmg` | **有**(lscreen 同构,cask url 模板依赖) |
| 校验和 | dist | 每资产附 `.sha256`,另有 `sha256.sum` | — |

- **Windows ARM64 不再出包**(2026-10-12,坤哥:资产面收敛):dist targets
  移除 `aarch64-pc-windows-msvc`,ARM64 机器经 Windows 的 x64 模拟运行 x64
  包;源码级测试矩阵(rust.yml)仍保留该 target。历史版本的 ARM64 zip 不追删。

- **发布后清理**(macos-dmg.yml 尾部步骤):上述两行 macOS 每架构 tar.xz
  (含 `.sha256`)与 `source.tar.gz`(含 `.sha256`)在 dmg 回传后从 Release
  删除;清理是容忍失败式(资产缺失只 echo)。重打历史 dmg 时 tar.xz 已不在,
  需从对应 tag 重跑 dist 构建取料。
- 二进制名统一 `latermd`(crate 名仍为 `latermd-app`,`[[bin]]` 改名 +
  `crates/latermd-app/dist.toml` shadow 资产前缀)。
- **风险登记册 #7**:cask 停更的教训是 dmg 资产命名变化后 cask URL 模板没跟上
  (lscreen 曾因此停在 0.6.0)。**任何资产命名调整必须同步改 cask url / livecheck**,
  并在本文表 2 回改。
- 注意 dist 的 tar.xz/zip 与自建 dmg/deb 的版本号策略**刻意不同**:前者无版本号
  (dist 官方设计,支持 latest 热链),后者带版本号(Homebrew cask 惯例,url 与
  version 绑定以便 sha256 校验;Debian 惯例文件名内嵌版本)。这是两套命名
  共存的原因,不是疏漏。

## 3. 首个 Release 历史记录(v0.0.1,2026-09-26 已过)

> **本节已成历史**(V1 核对表 R3,证据 E1–E4):首个 Release(v0.0.1)已于 2026-09-26
> 自动发版,v0.0.2(09-29)/ v0.0.3(09-30)复跑同链路。日常发版流程见 §4;§3.2 的
> 应急通道与 §3.1 的前置检查清单仍然有效(§4 第 4 条仍引用 §3.1)。以下小节按
> 历史记录订正(V1 R12–R15),未回填项如实保留,不随归档默认打勾。

### 3.1 前置检查

1. 发版 PR 已合入 `main`:PR 内含 `workspace.package.version` 的版本 bump
   与 CHANGELOG.md 对应小节(分支保护要求 PR;发布从 tag 指向的 commit 构建)。
2. 本地六项门禁全绿(AGENTS.md §8;vendor 改动另跑 check.sh)。
3. 配置核对(改过 dist-workspace.toml 才需要):

   ```bash
   dist manifest --output-format=json \
     --target=aarch64-apple-darwin --target=x86_64-apple-darwin \
     --target=x86_64-unknown-linux-gnu --target=x86_64-pc-windows-msvc
   ```

   确认矩阵含 4 个 build job(双 darwin 合并在 macos-14)、资产名与表 2 一致。
4. `Cargo.toml` 的 `workspace.package.version` 已是待发版本(dist 要求 tag 版本
   与 workspace 版本一致,否则报版本不匹配)。

### 3.2 发布(全自动)

合入后无需任何手动操作:main 上 rust.yml 门禁跑绿 → auto-tag.yml 自动打
`v{version}` 并 dispatch release.yml → 五目标构建 + 建 Release(正文取自
CHANGELOG.md)→ host job dispatch macos-dmg.yml 合成 dmg 回传。
全程约 20–40 分钟(Windows 目标最慢),在 Actions 页盯
`Auto Tag` → `Release` → `macOS dmg` 三个 workflow 依次变绿即可。

应急通道(自动链路故障时,人工等价物):

```bash
git checkout main && git pull --rebase origin main
git tag v0.0.1 && git push origin v0.0.1   # 人工 tag push 直接触发 release.yml
```

### 3.3 资产核对清单(历史回填:2026-09-30 按三版 Release 资产名单逐项核对)

> 证据 = V1 核对表 §1(核对项 A1–A6)与证据摘录 E1–E5/E18;**v0.0.1 无 deb 与
> 未清理系时序事实**(deb job 与清理步均 2026-09-29 由 `0519c01`/PR #56 接入,
> E18/E21),不是资产缺失或清理失败事故,按版本分开判。

- [x] `latermd-x86_64-unknown-linux-gnu.tar.xz`(+`.sha256`)—— 三版均在(A2)
- [x] `latermd_{version}_amd64.deb` —— v0.0.2/v0.0.3 在;v0.0.1 无(deb job
      2026-09-29 才接入,设计时序)(A3)
- [x] `latermd-x86_64-pc-windows-msvc.zip`(+`.sha256`)—— 三版均在(A2)
- [x] `latermd-aarch64-pc-windows-msvc.zip`(+`.sha256`)—— 三版均在(A2);
      windows-11-arm 原生 runner 首编已证通过(见下方 R13)
- [x] `latermd-v{version}-universal2-apple-darwin.dmg`(macos-dmg job 完成)
      —— 三版均在,实际资产名按各自版本 v0.0.1/v0.0.2/v0.0.3(V1 核对表 R12,
      证据 A4/E2–E4;原文写的 `v0.1.0` 从未存在,E9);**回传资产不在 dist 生成的
      下载表里**,Release 正文补一行见 §4 第 5 步
- [x] `sha256.sum`、`dist-manifest.json`(dist 附带)—— 在;sha256.sum 含已删
      条目为 §1 既知代价(A6/E5)
- [~] **清理已生效** —— v0.0.2/v0.0.3 已无 `*-apple-darwin.tar.xz` 与
      `source.tar.gz`;**v0.0.1 未清理**系彼时清理步尚未引入(A5/E18),不是
      清理步失败
- [x] 「macOS dmg」job 的 **step summary** 里有 dmg 的 sha256 —— 已产出;但
      cask 填值已不经此路径(tap auto-bump 直读 Release asset digest,
      V1 R14/L3,证据 E12/E13)
- [x] Release 未被误标 prerelease —— 三版均非 draft 非 prerelease(A1/E1)

首个 Release 的额外验证(只此一次,之后信任链路):
- ~~Windows xwin 交叉编能否通过~~ **已实测编不过**:ring 0.17 的 ARM64 汇编
  在 cargo-xwin 容器内失败(容器 clang 不认 cc-rs 的 `/imsvc` 参数,
  2026-09-26 v0.0.1 首发踩中),已改 `github-custom-runners` 把
  `aarch64-pc-windows-msvc` 指到 `windows-11-arm` 原生 runner;
  ~~windows-11-arm 首编能否通过是下一个待验项~~ **已实测通过**(V1 核对表 R13,
  证据 E2–E4:三版资产名单均含 win ARM64 zip);
- dmg 内 .app 在真机可启动 —— **仍未做**,留人工(blocked_external,本机无
  macOS;acceptance-checklist §3)。

### 3.4 cask 落地(动 tap 仓库 crazykun/homebrew-ailater)—— 已落地

模板在本仓 [packaging/latermd.rb](../packaging/latermd.rb)(按 lscreen 模式:
universal2 单 dmg url、`livecheck :github_latest`、postflight 去 quarantine、
`depends_on macos: :sonoma`、zap)。**复刻对象是 `Casks/lscreen.rb`,不要抄
同 tap 的 `glmeter.rb`**(后者仍是按架构拼 URL 的旧式双包写法,lscreen v0.8.0
起已废弃该模式)。原「要动三处」的现状(2026-09-30 核实,V1 R14,证据 E12/E13):

1. **`Casks/latermd.rb` 已建**(不再是「新建」):version "0.0.3"、sha256 真值
   `d86ec30a…`,由 tap 仓 auto-bump workflow 自动维护(源:Release asset
   digest);主仓 packaging/latermd.rb 的 `version "0.1.0"` / `sha256
   :no_check` 仅为初版模板占位(V1 R2,主仓侧纠偏见 decisions-pending #54)。
2. **tap README**:是否已加 `latermd` 行未核(V1 取证未覆盖 tap README,以
   tap 仓库现状为准)。
3. **验证**:`brew install --cask crazykun/ailater/latermd` 真机跑通**仍未做**
   (blocked_external,缺 macOS 真机;acceptance-checklist §7.2);主仓 README
   「安装」节三条路径与三版 Release 资产核对一致(E2–E4/E12)。

**auto-bump 空档(历史,已闭合)**:本节曾记「tap 的 auto-bump(cron 每小时
:23)只遍历 `Formula/`,`Casks/` 不在自动范围,latermd cask 需每版手动回填
version/sha256」——该空档在 tap 侧已扩表闭合(V1 R15,证据 E12/E13):tap 的
auto-bump.yml 现含 `CASKS=("latermd:ailater/LaterMd" …)` 表(cron 每小时
:23),注释明言「Cask 的 url 是 v#{version} 插值模板永不改动,只更新
version + sha256 两行;sha256 从 GitHub Release asset 的 digest 字段直接读取
(免下载)」,cask 已自动跟版至 0.0.3。发版后不再需要手动回填(§4 第 3 步已
相应改写)。主仓 #22(cask-bump)已于 2026-09-30 落地:macos-dmg.yml 尾部
cask-bump 步在 dmg 回传后**即时**回填同一 cask 文件(gh api PUT,
HOMEBREW_TAP_TOKEN,无 token/4xx/行格式不匹配均只告警不阻塞发布)。与
tap 侧 cron 双写同一文件但值同源同值(sha256 都取自同一 dmg 文件:本地
shasum == GitHub asset digest),竞态最坏是 PUT 409,回填步告警跳过、由
cron 兜底,无害;实测 tap 仓默认分支是 master(任务书原写 ref=main 会 404),
步骤按默认分支读写,不钉分支名。两路是否撤其一仍留后续评审
(decisions-pending #53 第三轮补全第 3 点、#57)。

**cask 自动回填(#22 cask-bump 步)三要素**:

- **触发时序**:release.yml host job dispatch macos-dmg.yml(`-f tag=<tag>`)
  → dmg job 下载双架构 tar.xz → lipo 合一 → 组装 .app → hdiutil 合 dmg →
  `gh release upload` 回传同一 Release → 清理冗余资产 → 尾部 cask-bump 步:
  `gh api GET repos/crazykun/homebrew-ailater/contents/Casks/latermd.rb` 取
  blob sha 与内容 → python 正则替换 version/sha256 两行 → `gh api PUT` 回写
  tap **默认分支**(实测 master,不钉分支名)。
- **所需凭据**:`HOMEBREW_TAP_TOKEN` = 对 tap 仓库有 `contents:write` 权限的
  PAT,**需人工配置**到本仓(LaterMD)Settings → Secrets and variables →
  Actions;`GITHUB_TOKEN` 跨仓库无写权限,不能替代。未配置时步走「告警+跳过」
  分支(tap cron 兜底),本仓侧交付照常;**GitHub 侧实际生效(真发一版看 PUT
  落地)留人工验证**,blocked_external。
- **失败语义**:三类失败(无 token / gh api 读或写失败含 4xx 撞车 / version
  或 sha256 行正则命中数 ≠ 1)各自 `::warning::` + 写 step summary,末尾
  `trap EXIT` 强制 exit 0,step 恒绿,**仅告警不阻塞发布链**(dmg 回传与
  资产清理已在本步之前完成)。

**cask 真实行格式与替换规则**(2026-09-30 `gh api` 只读实测现网 Casks/latermd.rb):

- 现网格式:version 行为 `  version "0.0.3"`、sha256 行为
  `  sha256 "d86ec30a…"`(两空格缩进);url 是 `v#{version}` 插值模板永不改动,
  livecheck `:github_latest` 自动发现新版本,回填只动 version/sha256 两行
  (与上方空档段 tap 侧注释「只更新两行」的口径互证)。
- 替换规则:与 cask-bump 步内嵌正则一致,`^( +version +")([^"]*)("$)` 与
  `^( +sha256 +")([^"]*)("$)`(多行模式)各**恰好命中一行**才回写;命中数 ≠ 1
  (cask 行格式被改/出现同名行)则告警跳过不盲写,避免把 tap 文件改坏。

**brew 拿不到新版本时(手动路径)**:cask 跟版窗口 = 发版链正常时 cask-bump
步在 dmg 回传后即时回填;`HOMEBREW_TAP_TOKEN` 未配置时由 tap auto-bump cron
(每小时 :23)兜底,最长约 1 小时;两路同时失败(cron 故障、tap 改名/权限
回收等)cask 才会停在旧版。此时:

- **用户侧**:直接从 GitHub Release 页下载 universal2 dmg 安装(资产名模板
  `latermd-v{version}-universal2-apple-darwin.dmg`,命名规范见 §2,具体
  **以 Release 页为准**),拖入 `/Applications/` 后执行一次
  `sudo xattr -dr com.apple.quarantine /Applications/LaterMD.app`(口径同 §5)。
  README 安装节有同款面向用户的说明。
- **维护者侧**:按上一段真实行格式改 tap 仓 `Casks/latermd.rb` 的
  version/sha256 两行,push 默认分支即可(macos-dmg.yml 无 token 告警文案
  「人工回填步骤见 docs/distribution.md §3.4」指的就是本段;`brew
  bump-cask-pr` 对自定义 tap 的行为未实测,不写进口径)。

### 3.5 首发后回填(2026-09-30 纠偏回填;未做项如实保留,不随归档默认打勾)

- 主仓 README 安装节:三条路径(macOS dmg/brew、Windows zip、Linux tar.xz)
  与三版 Release 资产核对一致(E2–E4/E12);**未完**:deb 渠道一行未列
  (V1 L2)、README:8 版本行仍写 v0.0.1(V1 R1)——README/packaging 因本棒
  路径约束未改,登记 decisions-pending #54 留后续单独 PR。
- docs/roadmap.md「当前位置」:P0 行已按三版发版事实订正(本 PR,R3)。
- m0-report.md 真机项:Win11 / macOS 冒烟结果(IME、字体 face index 核对)
  **未回填**,留人工(blocked_external)。

## 4. 后续版本发布(v0.0.2+)

1. PR 里把 `workspace.package.version` 提到新版本号(workspace 内 dist-able
   crate 版本必须一致,lockstep),CHANGELOG.md 顶部加对应小节,合入 main。
   **CHANGELOG.md 小节与版本号 bump 同等重要**:dist 拿它当 Release 正文,
   漏了 Release 页面就只剩一张下载表(v0.0.2 踩过一次,事后 `gh release edit`
   才补上)。
2. 链路全自动(§3.2),无需打 tag。
3. **发版后核对 tap(已自动化,原「必做手动回填」口径作废)**:cask 的
   version/sha256 走双路自动跟版——主仓 macos-dmg.yml 尾部 cask-bump 步在
   dmg 回传后即时回填(#22,HOMEBREW_TAP_TOKEN,失败仅告警),tap 仓
   auto-bump cron(每小时 :23)兜底(源:Release asset digest,E12/E13);
   发版后只需巡检 cask version 与新 tag 一致。两路重复,是否撤其一留后续
   评审(decisions-pending #53 第三轮补全第 3 点、#57)。
4. dist 配置(dist-workspace.toml)改动后本地必须重跑 §3.1 第 3 步的
   manifest 校验,再 `dist generate` 重新生成 release.yml —— 重新生成会
   **覆盖三处 LOCAL PATCH**(§1),必须按清单重新打上;allow-dirty 见 §1
   的前置条件说明。
5. **发版后核对 Release 正文**:①标题下面有本版本的 Release Notes(来自
   CHANGELOG.md);②下载表之外另有 macOS universal2 dmg 一行 —— dmg 是
   macos-dmg.yml 事后 `gh release upload` 回传的,dist 建 Release 时它还不存在,
   因此**永远不在 dist 自动生成的下载表里**。缺任一项用 `gh release edit <tag>`
   补:`gh release edit v0.0.2 --notes-file <(cat <<'EOF' ... EOF)`,正文注意
   保留 dist 生成的原下载表。

## 5. 无签名路线的用户侧影响(README「安装」节的依据)

- **Windows**:SmartScreen 警告 → 「更多信息 → 仍要运行」。视为已知门槛,
  不修(无证书路线,AGENTS.md §5)。
- **macOS + brew cask**:postflight 自动 `xattr -dr com.apple.quarantine`,
  用户无感。
- **macOS 直下 dmg**:Gatekeeper「已损坏,无法打开」/「无法验证开发者」→
  `sudo xattr -dr com.apple.quarantine /Applications/LaterMD.app`。
- ad-hoc 签名(codesign -s -,macos-dmg.yml)**不解决** Gatekeeper,只为避免
  无签名 bundle 的启动异常;信任门槛完全靠 cask postflight / 手动 xattr。

## 6. 第三方组件许可声明

发布产物携带的第三方组件与资源在此登记(依赖版本清单见
[adr-004](adr-004-technical-stack.md);仅记**随二进制分发**的东西,
build 期工具不在此列):

| 组件 | 形态 | 许可 | 源内位置 |
|---|---|---|---|
| Inter 4.1(Regular/Medium/SemiBold,rsms/inter) | 嵌入字体资源(`include_bytes`,U1) | SIL Open Font License 1.1 | crates/latermd-app/src/fonts.rs;字文件与许可文本 [assets/fonts/](../assets/fonts/) |
| Twemoji(jdecked/twemoji v17.0.3) | 嵌入 PNG 图形资源(#47 彩色 emoji 面板,`include_bytes` 懒解码纹理) | CC-BY 4.0(义务 = 署名 + 附许可文本;本表即署名,全文随源码库 [assets/emoji/twemoji/LICENSE-GRAPHICS](../assets/emoji/twemoji/LICENSE-GRAPHICS)) | 资产与下载脚本 [assets/emoji/twemoji/](../assets/emoji/twemoji/)(272 枚 72x72,来源/版本/复现见同目录 SOURCES.txt) |

**OFL 1.1 的分发合规口径(2026-09-28 定)**:OFL 要求字体再分发时随附许可
文本。LaterMD 的 Release 产物是单一二进制(TTF 以 `include_bytes` 打进
去,不是独立文件),安装包内**没有**独立的 license 文件;合规依赖两条:
许可文本随源码仓库分发(assets/fonts/LICENSE-Inter.txt,仓库公开可取),
且本表即是产物级的许可声明。若后续要更严格的随包合规,可在 Release 资产
附 `licenses.zip` 或在应用内加「关于/开源许可」页 —— 现阶段按上述口径执行。
