# 流式性能复测与 M0 基线对照(2026-10,#46 R1,只测不改)

日期: 2026-10-02
分支: `feature/streaming-perf-recheck`(head = `618b3ab`,与 origin/main 同步)
关联: [m0-report.md](m0-report.md) 主验证 2 / 附加验证 4、[auto-plan.md](auto-plan.md) #46、roadmap「当前位置」
执行者: R1 基线复测模块(红线:不改任何产品代码;bench 命令、参数、输出逐字记录,数字不做美化)

> **执行环境**: Deepin 25(内核 6.18.48-amd64-desktop-rolling)/ X11 会话(DISPLAY=:0)/ rustc 1.98.0 / bench 二进制 `target/release/deps/longdoc-9a3abfe4edeb0714`。
> M0 报告环境(2026-09-24/25)与本轮的**环境差异见 §1**——其中「会话级 Vulkan FIFO present 阻塞」使真窗口滚动帧率本轮**无法按 M0 口径复测**,已按 blocked_external 如实标注,可自动做的部分(headless bench + 无节流探针旁证)照常完成。

---

## 0. 判定口径(先写口径,再下结论)

**流式追加(验证 4 复测)**

- 判据 = 「追加 1 行 + 整帧渲染」的中值成本随文档规模(500 / 2000 / 10000 行)的**增长曲线**:
  - 成本增长倍数 ≈ 规模增长倍数(×4 规模 ≈ ×4 成本,×5 ≈ ×5)→ **线性 = O(n) 未解**;
  - 成本增长显著低于规模增长、且每行成本随规模**趋稳或下降** → **亚线性 = 已解**。
- 对照 M0 基线(m0-report §4.2:38.7 / 154.7 / 789.3 ms,折合 ~77–79 µs/行)分档记「变好 / 变差 / 持平」,分档带宽 ±5%(带宽内记持平)。
- µs/行 = criterion 中值 ÷ 行数(换算列,非四舍五入美化;中值取 criterion 区间三点之中间值)。
- 基准语义与 M0 完全同源(`benches/longdoc.rs` 自 M0 未改;每迭代一次 `body.clone()` + 追加一行 + 整帧渲染——clone 成本两侧同样存在,可比)。

**长文档滚动(验证 2 复测)**

- 主判据 = 真窗口 `scrollbench` 10 万字滚动 p50 / p99 对照 M0 基线(p50 16.58 ms = 60.3 fps,vsync 锁定,llvmpipe)。
- 辅判据 = headless bench 三项(冷首帧 / 稳态顶部 / 稳态中部)对照 M0(136.3 ms / 415.3 µs / 430.2 µs),同口径同二进制源,完全可比。
- 环境前提必须先声明(§1);主判据若因环境不可执行,记 blocked_external 并以辅判据 + 同载体无节流探针旁证下结论,**不得用旁证数字冒充 vsync-locked 帧率**。

---

## 1. 环境与 M0 基线可比性(先读这个再看数字)

| 项 | M0(2026-09-24/25) | 本轮(2026-10-02) | 对可比性的影响 |
|---|---|---|---|
| rustc / egui / wgpu | 1.98.0 / 0.36.2 / 30.0.1 | 同左(钉死) | 无 |
| bench 载体源码 | `benches/longdoc.rs` + `examples/scrollbench.rs` | **逐字节相同**(git 确认 M0 后未改) | headless 两项完全可比 |
| Mesa / LLVM | LLVM 18.1.8(m0-report §验证3) | **25.0.7 / LLVM 19.1.4**(glxinfo 实读,系统升级) | 涉及 present 行为(§3b) |
| 开机 / 会话 | M0 会话 | 2026-09-28 重启;kwin_x11 自 09-28 09:21 运行,合成 active(dbus-send `org.kde.kwin.Compositing.active` → `boolean true`) | 涉及 present 行为 |
| 显示器 | 未记录 | 双屏 DP-1(主,1920×1080)+ DP-3(1920×1080),当前 3840×1080 | 涉及 present 行为 |
| Vulkan 枚举 | **仅 llvmpipe 一项**(m0-report §验证3) | **两项**:`[0] llvmpipe (LLVM 19.1.4, 256 bits)` + `[1] Intel(R) Graphics (RPL-S)`,loader 排序后 **wgpu 建在 Intel 硬件 GPU 上**(证据:`VK_LOADER_DEBUG=all` 逐字摘录——) | 环境口径与 M0 不同,滚动项需注明 |
| app 在屏 adapter 显示 | M0 冒烟载体有(§验证3 截图核对) | **已随冒烟载体移除**(现产品 UI 无 AdapterInfo 显示) | adapter 证据改用 loader debug,见上 |

`VK_LOADER_DEBUG=all target/release/examples/scrollbench` 关键逐字输出(stderr 摘录):

```
[Vulkan Loader] INFO | DRIVER:  linux_read_sorted_physical_devices:
[Vulkan Loader] INFO | DRIVER:       Original order:
[Vulkan Loader] INFO | DRIVER:             [0] llvmpipe (LLVM 19.1.4, 256 bits)
[Vulkan Loader] INFO | DRIVER:             [1] Intel(R) Graphics (RPL-S)
[Vulkan Loader] INFO | DRIVER:       Sorted order:
[Vulkan Loader] INFO | DRIVER:             [0] Intel(R) Graphics (RPL-S)
[Vulkan Loader] INFO | DRIVER:             [1] llvmpipe (LLVM 19.1.4, 256 bits)
...
[Vulkan Loader] DRIVER:            <Device>
[Vulkan Loader] DRIVER:                Using "Intel(R) Graphics (RPL-S)" with driver: "libvulkan_intel.so"
```

**可比性结论**:§2(流式)与 §3a(headless 滚动)不涉及 GPU / 窗口 / present(纯 `egui::Context::run_ui`,CPU 路径),与 M0 数字**同口径可比**;§3b(真窗口滚动)受 §1 环境变化影响,主判据本轮不可得,见 §3b 的诊断链与 blocked_external 标注。

---

## 2. 流式追加复测(验证 4)—— **O(n) 仍未解,10000 行档变差**

命令(M0 同款,`benches/longdoc.rs` 的 `streaming_append_by_lines` 组,`sample_size(10)`):

```bash
cargo bench -p latermd-app --bench longdoc -- streaming_append
```

逐字输出(2026-10-02,全量,含 criterion 告警):

```
    Finished `bench` profile [optimized] target(s) in 0.15s
     Running benches/longdoc.rs (target/release/deps/longdoc-9a3abfe4edeb0714)
Gnuplot not found, using plotters backend
long doc: 100251 chars, 3704 lines
Benchmarking streaming_append_by_lines/append_1_line_at_500
Benchmarking streaming_append_by_lines/append_1_line_at_500: Warming up for 3.0000 s
Benchmarking streaming_append_by_lines/append_1_line_at_500: Collecting 10 samples in estimated 5.6012 s (165 iterations)
Benchmarking streaming_append_by_lines/append_1_line_at_500: Analyzing
streaming_append_by_lines/append_1_line_at_500
                        time:   [33.548 ms 33.800 ms 34.120 ms]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking streaming_append_by_lines/append_1_line_at_2000
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 8.0s or enable flat sampling.
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Collecting 10 samples in estimated 7.9906 s (55 iterations)
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Analyzing
streaming_append_by_lines/append_1_line_at_2000
                        time:   [144.96 ms 146.46 ms 147.75 ms]
Benchmarking streaming_append_by_lines/append_1_line_at_10000
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 10.1s.
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Collecting 10 samples in estimated 10.120 s (10 iterations)
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Analyzing
streaming_append_by_lines/append_1_line_at_10000
                        time:   [990.48 ms 1.0017 s 1.0177 s]
Found 3 outliers among 10 measurements (30.00%)
  2 (20.00%) low mild
  1 (10.00%) high severe
```

换算表(中值折算,µs/行 = 中值 ÷ 行数):

| 档位 | 本次中值 | 本次 µs/行 | M0 中值 | M0 µs/行 | 分档(±5% 带宽) |
|---|---|---|---|---|---|
| 500 行 | 33.800 ms | **67.60 µs** | 38.7 ms | 77.4 µs | **变好(−12.7%)** |
| 2,000 行 | 146.46 ms | **73.23 µs** | 154.7 ms | 77.35 µs | **≈持平(−5.3%,带宽边界)** |
| 10,000 行 | 1,001.7 ms | **100.17 µs** | 789.3 ms | 78.93 µs | **变差(+26.9%)** |

增长曲线判定(口径见 §0):

- 规模 ×4(500→2000):成本 ×**4.33**(M0 为 ×4.00);
- 规模 ×5(2000→10000):成本 ×**6.84**(M0 为 ×5.10);
- 每行成本**随规模上升**(67.6 → 73.2 → 100.2 µs),M0 是平的(77.4 → 77.35 → 78.9 µs)。

**判定:流式追加仍是 O(n),未解;且 10000 行档绝对值比 M0 变差 +26.9%(789.3 ms → 1001.7 ms),曲线还略呈超线性。** 超 ~1000 行即跟不上 100 ms/chunk 的 LLM 吐字节奏(2000 行档 146 ms 已超),与 M0「~1300 行」的结论同向且更紧。

---

## 3. 长文档滚动复测(验证 2)

### 3a headless 渲染路径(与 M0 完全可比)—— **三项全部变好,视口剔除保持**

命令(M0 同款):

```bash
cargo bench -p latermd-app --bench longdoc -- long_doc_100k
```

逐字输出(2026-10-02,全量):

```
    Finished `bench` profile [optimized] target(s) in 0.17s
     Running benches/longdoc.rs (target/release/deps/longdoc-9a3abfe4edeb0714)
Gnuplot not found, using plotters backend
long doc: 100251 chars, 3704 lines
Benchmarking long_doc_100k/cold_first_frame
Benchmarking long_doc_100k/cold_first_frame: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 6.7s or enable flat sampling.
Benchmarking long_doc_100k/cold_first_frame: Collecting 10 samples in estimated 6.7376 s (55 iterations)
Benchmarking long_doc_100k/cold_first_frame: Analyzing
long_doc_100k/cold_first_frame
                        time:   [120.95 ms 123.49 ms 126.19 ms]
Benchmarking long_doc_100k/steady_state_top
Benchmarking long_doc_100k/steady_state_top: Warming up for 3.0000 s
Benchmarking long_doc_100k/steady_state_top: Collecting 10 samples in estimated 5.0183 s (14k iterations)
Benchmarking long_doc_100k/steady_state_top: Analyzing
long_doc_100k/steady_state_top
                        time:   [357.08 µs 358.83 µs 360.95 µs]
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking long_doc_100k/steady_state_scroll_middle
Benchmarking long_doc_100k/steady_state_scroll_middle: Warming up for 3.0000 s
Benchmarking long_doc_100k/steady_state_scroll_middle: Collecting 10 samples in estimated 5.0162 s (13k iterations)
Benchmarking long_doc_100k/steady_state_scroll_middle: Analyzing
long_doc_100k/steady_state_scroll_middle
                        time:   [365.44 µs 371.36 µs 376.43 µs]
```

对照 M0(m0-report §2.1):

| 场景 | 本次中值 | M0 中值 | 分档(±5% 带宽) |
|---|---|---|---|
| `cold_first_frame` | 123.49 ms | 136.3 ms | **变好(−9.4%)** |
| `steady_state_top` | 358.83 µs | 415.3 µs | **变好(−13.6%)** |
| `steady_state_scroll_middle` | 371.36 µs | 430.2 µs | **变好(−13.7%)** |

顶部与中部仅差 12.5 µs(≈3.4%),视口剔除在 10 万字规模下依旧生效,单帧成本与滚动位置无关;距 18.18 ms(55 fps)预算余量 **48.9×**(M0 为 42×)。

### 3b 真窗口滚动帧率 —— **本轮无法按 M0 口径复测(blocked_external:会话级 Vulkan FIFO present 阻塞)**

命令(M0 同款):

```bash
target/release/examples/scrollbench     # 20 秒自动退出并打印统计
```

逐字输出(2026-10-02,共 6 次运行——默认 adapter、`VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json` 钉 llvmpipe、聚焦/置顶三种条件,全部同一结果):

```
scrollbench: 文档 100030 字符 / 4384 行
scrollbench: 没有采到帧
```

「没有采到帧」的语义(scrollbench.rs:79-84):样本仅在 `frame_index > 120` 后采集,而统计在 20 秒后打印——即 **20 秒内 ui 帧数不足 121 帧(≤6 fps)**,与 M0 同一二进制的 1,066 帧(p50 60.3 fps)完全不同。

**诊断链(逐字证据,定位阻塞层,全部为只读观测或临时探针,见 §4-C):**

1. **窗口正常、进程几乎不耗 CPU**:窗口 `Map State: IsViewable`(800×600);进程 CPU ticks 时间线(`awk '{print $14+$15}' /proc/PID/stat`,每秒采样)——前 2 秒 23 ticks(启动 + 解析),其后 23 秒仅再涨 7 ticks(≈70 ms CPU ≈ 每秒 ~10 ms,按稳态单帧 ~371 µs 折算 ≈ **6–7 fps**,其余时间全部睡眠)。
2. **阻塞点在 Vulkan WSI present 队列线程**:线程列表(`for tid in /proc/PID/task; cat wchan`)逐字摘录——
   ```
   tid 2544741: wchan=futex_do_wait WSI swapchain q
   ```
   主线程与其余工作线程同样停在 futex,仅有 X11 事件线程在 `poll_schedule_timeout`。
3. **GL 通路完全健康(同会话对照)**:`timeout 7 glxgears` 逐字输出——
   ```
   Running synchronized to the vertical refresh.  The framerate should be
   approximately the same as the monitor refresh rate.
   2164 frames in 5.7 seconds = 377.875 FPS
   ```
   GLX 的 vblank 节流失效是「失败开放」(不等待,照跑 377 fps);Vulkan Fifo 是「失败关闭」(等一个不来的 vblank,每帧卡 ~150 ms)。
4. **无节流探针:应用侧渲染+提交+present 通路健康**(临时探针 `scrollbench_novsync.rs`,与 scrollbench.rs 唯一差异是 `WgpuConfiguration.surface.present_mode` 覆盖,测完已删,见 §4-C)。默认 adapter(Intel RPL-S 硬件)两次运行逐字输出:

   `PROBE_PRESENT=immediate`(PresentMode::Immediate):
   ```
   scrollbench-novsync: present_mode=Immediate
   scrollbench: 文档 100030 字符 / 4384 行
   --- scrollbench-novsync 结果 ---
   帧数: 23551 (含启动与冲到中部的阶段)
   平均: 0.82 ms  (1226.5 fps)
   p50: 0.73 ms  (1366.6 fps)
   p95: 1.08 ms  (923.3 fps)
   p99: 1.21 ms  (829.7 fps)
   max: 5.37 ms  (186.3 fps)
   ```

   `PROBE_PRESENT=mailbox`(PresentMode::Mailbox):
   ```
   scrollbench-novsync: present_mode=Mailbox
   scrollbench: 文档 100030 字符 / 4384 行
   --- scrollbench-novsync 结果 ---
   帧数: 9113 (含启动与冲到中部的阶段)
   平均: 1.96 ms  (509.0 fps)
   p50: 0.67 ms  (1486.5 fps)
   p95: 0.81 ms  (1240.4 fps)
   p99: 1.14 ms  (877.4 fps)
   max: 661.20 ms  (1.5 fps)
   ```
5. **llvmpipe 无法绕过 Fifo**:`VK_DRIVER_FILES=.../lvp_icd.json PROBE_PRESENT=immediate` 逐字输出(exit=101)——
   ```
   wgpu error: Validation Error

   Caused by:
     In Surface::configure
       Requested present mode Immediate is not in the list of supported present modes: [Fifo]
   ```
   即 M0 的 adapter(llvmpipe)上 Fifo 是**唯一** present 模式,本会话的 Fifo 阻塞在其上无法规避。

**判定**:

- **渲染路径无回归、反而变好**(§3a 三项 + §3b-4 旁证:同样载体无节流 present 下 p50 0.73 ms/帧,距 18.18 ms 预算 24.9×);
- **「vsync 锁定 p50/p99」这一 M0 口径的数字,本轮在本会话测不出来**——阻塞层定位在会话/驱动(Mesa 自 M0 后升级、双屏、kwin 会话)的 Vulkan WSI Fifo/vblank 等待,**不是 LaterMD 或 vendored 层代码路径**(同一二进制 M0 得 60.3 fps;两个 adapter、聚焦/置顶均复现;GL 正常);
- 该项记 **blocked_external**:缺一个 Vulkan Fifo present 正常的会话(或真机)。恢复后的复测命令即上文 `target/release/examples/scrollbench`,与 m0-report §2.2 同表对照。

---

## 4. 疑点清单

**疑点 A:流式 O(n) 的机制层 = 布局缓存整篇失效(文本哈希门控粒度过粗)**
- 证据(vendored 层,本次实读):`vendor/egui_markdown/src/label.rs:57-64` `hash_text` 对**整篇文本**哈希;`label.rs:636` 缓存命中要求 `cached.text_hash == text_hash`;`label.rs:663-664` miss 即 `parser::parse(text)` 全文解析 + 全量 layout;块级高度缓存 `try_cull_block`(`label.rs:137-151`)同样以 `text_hash` 为 key,`label.rs:141-143` `cached_hash != text_hash` 即全块失效——**追加一行 = 整篇每块重排**,这就是 ~100 µs/行 × 行数的来源。
- 分层排除:「逐帧全量重解析」不是主导(解析按 M0 上游 bench 94.3 µs/100 节外推 ~1.3 ms/10 万字,占 10000 行档 1001.7 ms 的 ~0.13%);「heal 全文」不在本 bench 路径(`MarkdownLabel` 默认 `heal: false`,`label.rs:352`);「图集重建」不适用于 headless bench(无 GPU)。**注意**:产品 preview 路径开启了 `heal(true)`(`crates/latermd-app/src/ui/preview.rs:597`),流式场景每帧另有一次 O(n) 补闭合扫描——R2 修布局时该层要一并纳入考虑。

**疑点 B:10000 行档 +26.9% 的怀疑方向(相关,未插桩证明因果)**
- M0 之后 vendored 层落地了四个排版类改动:`2a36f64`(行高按字号×比例)、`031fc6f`(CJK 行高下限 `min_line_height_em`)、`02b33ad`(标题字号分级 + `heading_space_above` 透明 spacer 行)、`fe286e7`(section 锚点暴露)、`6191beb`(表格底色)。这些都会增加每行/每节的排版工作量,方向上与 10000 行档变差一致;500/2000 行档反而变好,说明存在相互抵消的因素(如缓存/分配行为随规模变化)。**未做插桩归因**,如实存疑,留 R2 修复时顺带核。

**疑点 C:真窗口 fps 不可测的根因层 = 会话/驱动的 Vulkan WSI Fifo/vblank 等待(非应用代码)**
- 三条独立证据:①同一 scrollbench 二进制在 M0(2026-09-25)得 1,066 帧/p50 60.3 fps,本轮 ≤121 帧;②阻塞与 adapter 无关(Intel 硬件 ANV 与 llvmpipe 均复现)、与窗口焦点无关(focus/raise 后复现),且唯一被阻塞的系统线程是 `WSI swapchain q`;③同一载体把 present 换成 Immediate/Mailbox 后 23551 帧/p50 0.73 ms,渲染+提交+present 全链路健康。根因(节点 vblank 事件失效:Mesa 25.0.7 升级 / 双屏 CRTC / kwin 会话)在本模块权限之外,不在 R2 代码修复范围。
- 探针载体说明:`crates/latermd-app/examples/scrollbench_novsync.rs` 为一次性探针(scrollbench.rs 原样拷贝 + `NativeOptions.wgpu_options.surface.present_mode` 覆盖,`eframe::wgpu::PresentMode::Immediate|Mailbox`,env `PROBE_PRESENT` 选择),**测完已删**,工作区净(仅新增本文档);探针数字只作旁证,不得当 vsync-locked 基线引用。

---

## 5. 判定结论汇总

| 项 | M0 基线 | 本轮 | 判定 |
|---|---|---|---|
| 流式 500 行 | 38.7 ms(77.4 µs/行) | 33.800 ms(67.60 µs/行) | 变好 |
| 流式 2000 行 | 154.7 ms(77.35 µs/行) | 146.46 ms(73.23 µs/行) | ≈持平(带宽边界) |
| 流式 10000 行 | 789.3 ms(78.93 µs/行) | 1001.7 ms(100.17 µs/行) | **变差(+26.9%)** |
| 流式增长曲线 | 线性 O(n) | **线性偏超线性(×4.33 / ×6.84)** | **O(n) 未解** |
| 滚动·冷首帧 | 136.3 ms | 123.49 ms | 变好 |
| 滚动·稳态顶部 | 415.3 µs | 358.83 µs | 变好 |
| 滚动·稳态中部 | 430.2 µs | 371.36 µs | 变好 |
| 滚动·真窗口 p50 | 60.3 fps(16.58 ms) | **不可测(blocked_external)** | 渲染路径旁证健康;p50/p99 留待会话恢复复测 |

**一句话结论**:M0 验证 4 的流式 O(n) 问题**仍未解决**(成本随行数线性增长,10000 行档还比 M0 变差 ~27%),R2 应按最小修复开工;M0 验证 2 的滚动渲染路径**无回归且优于基线**,但「真窗口 vsync 帧率」这一口径在本机会话因 Vulkan WSI Fifo present 阻塞测不出数字,属环境问题,不构成应用层达标/不达标的证据。

## 6. 对 R2 的交接

- 修复对象 = 疑点 A:把失效粒度从「整篇 text_hash」降到「块/段」(追加只失效尾块),复用 `segment_breaks` 与分段 galley(m0-report §4.2 既定方向);产品 preview 的 `heal(true)` O(n) 扫描一并评估。
- 修复后用 §2 同一条命令对照本表,验收口径 = 每行成本随规模趋稳/下降(亚线性),而不是绝对值。
- 疑点 B 在修复时顺带核对四个 vendor 排版 commit 的单档影响;疑点 C 走环境复测(真机或会话修复后跑 §3b 命令),不占 R2。

---

## 7. R2 修复评估(2026-10-02,#46 R2 模块)—— **主导修复确需 vendor ①类,app 侧无可达近似,如实报未修**

> R2 输入 = §5 判定(流式 O(n) 未解 → 修复路径激活)。R2 模块红线:`paths=["crates"]`、零新增依赖、**不触碰 vendor/**;约束原文「修复确需 vendor 时停下,把需求记 decisions-pending 交人工拍板,本模块按 app 侧可达近似或如实报未修」。本节 = 机制复核 + app 侧可达性逐项核对 + 结论。**crates/ 零改动,本模块未修任何产品代码。**

### 7.1 疑点 A 机制复核(实读 vendored 层,两个 R1 未展开的 nuance)

R1 疑点 A 的机制描述(整篇 `hash_text` 门控,miss 即全文解析)复核**属实**,行号与现状一致;补两个对 vendor 修复设计有直接影响的细节:

1. **bench 场景走整篇 galley 路径,分段路径的 per-range 缓存帮不了它。** §2 bench 的 `code_fence` 文档是纯代码块,`MarkdownLabel` 未开 `scroll_code_blocks` 且无 `link_handler`(`benches/longdoc.rs:80-82`),`needs_segmentation` 返回 false(`vendor/egui_markdown/src/layout.rs:262-275`:纯 `CodeBlock` token 且两条件皆 false)→ 走 `label.rs:679-691` 的**整篇 `build_layout` 单 galley**,10000 行档的 ~1 秒主导在这里。分段路径(`render_segmented`)里 `flush_text_range` 的段缓存本来就是 per-range 的(`label.rs:975-976` `hash_flush_context` 按 **token 切片**哈希,`:991-1008` 命中即复用 galley)——「失效粒度过粗」对**文本段**不成立,成立的是顶层门控(`label.rs:636`)与块级剔除 key。
2. **即使强制分段,块级剔除缓存的 key 仍是整篇 text_hash。** `try_cull_block`/`cache_block_height`(`label.rs:137-156`)以 `text_hash`(整篇,`:629` 传入)为 key,`:772`(Table)/`:808`(滚动代码块)/`:837`(Image)三个块型都是——追加一行让整篇 hash 变,**视口外所有块剔除失效、全部重测**。这解释了 M0 上游 bench(`scroll_code_blocks(true)`,m0-report §验证 4 基准语义)同样呈线性的现象。**含义:app 侧开 `scroll_code_blocks` 不是修复,块级 key 必须换。**

### 7.2 app 侧可达性核对表(任务候选方向 + 自查,逐项行号证据)

| 候选方向(R1 §6 / 任务书) | 核对结果 | 证据(本轮实读) |
|---|---|---|
| 流式帧避免全文重解析(修订号增量) | 快照侧**已在位**;全文重解析主导在 vendor `render()` miss 分支,app 不可达 | `ui/editor.rs:295-299`「快照只在修订号前进时重建…第一层,vendored 层 text hash 缓存是第二层」、`live.rs:287-290` 同款;vendor `label.rs:663-665` |
| heal 只对流式预览帧生效(路径核对) | **已在位(#39 落地)**,两处调用点均按「AI 流式写入本标签」条件传参,稳态帧 `heal=false` | `ui/layout.rs:301-310`(三栏)、`ui/layout.rs:671-674`(Live,注释「heal 条件同三栏路径」)、`ui/preview.rs:594-597` 消费;流式帧 heal 是语义必需(AGENTS §6.5),成本 ~0.1 µs/行量级(§4-A 实测 2 万行 ~2ms),占 10000 行档 <0.1%,不动 |
| 布局缓存命中 | app 侧贡献项全稳定,无泄漏:handler `id()` 用 trait 默认 0、widget id 只含 tab id、font/style 稳定 | `ui/preview.rs:231-258`(AiLinkHandler 无 `fn id` override)、`vendor link.rs:92-94`(默认 0)、`ui/preview.rs:539-541`(`tab_preview_id` 只含 tab id,§6.7) |
| 避免无关帧重建 | **已在位**:修订号纪律 + egui request_repaint 纪律,空闲帧不渲染 | `ui/editor.rs:295-299`、`live.rs:288` |
| (R2 自查)`scroll_code_blocks(true)` 强制分段 | **不构成修复**:§7.1-2(cull key 整篇哈希,追加仍全块失效)+ 代码块渲染形态改滚动窗格(#38 复制头/既有观感回归) | vendor `label.rs:137-151/772/808`;#39 已记「预览既有配置不开 scroll_code_blocks」 |

### 7.3 结论与去向

- **未修(如实)**:O(n) 主导修复(失效粒度块级化)确需 vendor ①类改动,越出本模块红线;app 侧四候选方向 + 自查开关逐项核对,无一个可达且能改变 O(n) 判定曲线的修复点。按约束「如实报未修」执行,本模块 `crates/` **零改动**。
- 修复需求(两步 vendor ①类方案 + 产品侧降级备选)已按五要素登记 **decisions-pending #77**,交人工拍板。
- R3 复测口径:水位即 §2 当前水位(未修),挂账不销;拍板后修复的验收命令与口径见 §6 第二条。
- 单测:无修复行为,无新增断言(不造假凑数);已在位机制的既有护栏 = heal 开关切换不改渲染(`ui/preview.rs` `preview_reflects_edits_immediately_despite_per_tab_cache`)、widget id 稳定(#39 `tab_switch_perf` 回归)、rev 纪律(state.rs 既有断言),本轮未动。
- 门禁:本模块零代码改动,实跑 fmt/clippy 三轮/test 确认工作区健康(结果见 §7.4);六项全量门禁由编排在本棒最终 head 复验。

### 7.4 门禁实跑记录(2026-10-02,本模块收尾时,全部实跑)

```
$ cargo fmt --all --check                                            # 通过(exit 0,无输出)
$ cargo check --workspace --all-features --quiet                     # 通过(exit 0)
$ cargo clippy --workspace --all-targets -- -D warnings              # 通过(exit 0)
$ cargo clippy --workspace --all-targets --no-default-features -- -D warnings   # 通过(exit 0)
$ cargo clippy --workspace --all-targets --all-features -- -D warnings          # 通过(exit 0)
$ cargo test --workspace --all-features                              # 通过(exit 0)
```

test 汇总:**830 passed / 0 failed / 1 ignored**(唯一 ignored = #39 既有 `tab_switch` 取证测试,与 #47 A2 轮基线一致);本模块零代码改动,数字为工作区健康确认,非修复后对比。`cargo doc` 未在本模块单跑(零代码改动无文档面变化),由编排在本棒最终 head 六项全量门禁复验。

---

## 8. R3 同口径复测与前后对比(2026-10-02,#46 R3 模块)—— **未做修复,水位即当前水位;六项 headless 数字与 R1 无统计差异**

> 前置事实:#46 R2 未触发代码修复(§7:主导修复确需 vendor ①类,app 侧无可达近似,`crates/` 零改动,修复需求挂 [decisions-pending.md](decisions-pending.md) #77 待人工拍板)→ 按任务口径,**本轮复测结论 = R1 结论,「未做修复,水位即当前水位」**,挂账不销。本节 = 同命令同参数复跑 + R1→R3 前后对比落档。

### 8.1 同口径前提核对(先证明确实在同一条基线上比)

- **命令与参数**:与 §2 / §3a / §3b 逐字相同的三条命令,一个参数未动(见各小节引用)。
- **载体源码**:`benches/longdoc.rs` 与 `examples/scrollbench.rs` 自 R1 零改动——`git diff --name-only 618b3ab..HEAD` 仅 `docs/perf-recheck-2026-10.md`(R1 文档提交 `31828b8`);R2 模块 `crates/` 零改动(§7)。R1→R3 之间**没有任何产品代码变更**,这是「水位可比」的前提。
- **运行环境**:同机同会话(X11,DISPLAY=:0);Mesa 25.0.7-2 / LLVM 19.1.4 与 R1 相同;`target/release/deps/longdoc-9a3abfe4edeb0714` 与 `target/release/examples/scrollbench` 为 R1 同一二进制(增量构建 `Finished in 0.15s` 未重编)。
- **criterion change 基线**:criterion 的 `change:` 行是与**上一轮存档估算**的自动对比;R1(2026-10-02)之后再无人跑过这两条 bench(target/criterion 存档未刷新),故本轮 `change:` 即 **R3 vs R1** 的统计学检验。

### 8.2 流式追加复跑(命令 = §2 同款)

```bash
cargo bench -p latermd-app --bench longdoc -- streaming_append
```

逐字输出(2026-10-02 R3,全量,含 criterion change 行):

```
    Finished `bench` profile [optimized] target(s) in 0.15s
     Running benches/longdoc.rs (target/release/deps/longdoc-9a3abfe4edeb0714)
Gnuplot not found, using plotters backend
long doc: 100251 chars, 3704 lines
Benchmarking streaming_append_by_lines/append_1_line_at_500
Benchmarking streaming_append_by_lines/append_1_line_at_500: Warming up for 3.0000 s
Benchmarking streaming_append_by_lines/append_1_line_at_500: Collecting 10 samples in estimated 5.6132 s (165 iterations)
Benchmarking streaming_append_by_lines/append_1_line_at_500: Analyzing
streaming_append_by_lines/append_1_line_at_500
                        time:   [33.575 ms 33.853 ms 34.163 ms]
                        change: [-1.0122% +0.1254% +1.2037%] (p = 0.84 > 0.05)
                        No change in performance detected.
Benchmarking streaming_append_by_lines/append_1_line_at_2000
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 8.0s or enable flat sampling.
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Collecting 10 samples in estimated 8.0238 s (55 iterations)
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Analyzing
streaming_append_by_lines/append_1_line_at_2000
                        time:   [144.61 ms 146.00 ms 147.94 ms]
                        change: [-0.6276% +0.8818% +2.4399%] (p = 0.29 > 0.05)
                        No change in performance detected.
Benchmarking streaming_append_by_lines/append_1_line_at_10000
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 9.9s.
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Collecting 10 samples in estimated 9.8525 s (10 iterations)
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Analyzing
streaming_append_by_lines/append_1_line_at_10000
                        time:   [980.55 ms 993.94 ms 1.0079 s]
                        change: [-2.7368% -0.7791% +1.1116%] (p = 0.48 > 0.05)
                        No change in performance detected.
```

前后对比表(R1 = §2,换算口径同 §0:µs/行 = 中值 ÷ 行数):

| 档位 | R1 中值 | R3 中值 | R3 µs/行 | R3 vs R1 | criterion 检验 |
|---|---|---|---|---|---|
| 500 行 | 33.800 ms | 33.853 ms | 67.71 µs | +0.16% | No change(p=0.84) |
| 2,000 行 | 146.46 ms | 146.00 ms | 73.00 µs | −0.31% | No change(p=0.29) |
| 10,000 行 | 1,001.7 ms | 993.94 ms | 99.39 µs | −0.78% | No change(p=0.48) |

R3 增长曲线(口径 = §0):规模 ×4 成本 ×**4.31**(33.853→146.00 ms),规模 ×5 成本 ×**6.81**(146.00→993.94 ms),每行成本 67.71 → 73.00 → 99.39 µs 仍随规模上升。**判定与 R1 相同:线性偏超线性,O(n) 未解**——R2 未修,曲线自然原样。

### 8.3 headless 滚动复跑(命令 = §3a 同款)

```bash
cargo bench -p latermd-app --bench longdoc -- long_doc_100k
```

逐字输出(2026-10-02 R3,全量):

```
    Finished `bench` profile [optimized] target(s) in 0.27s
     Running benches/longdoc.rs (target/release/deps/longdoc-9a3abfe4edeb0714)
Gnuplot not found, using plotters backend
long doc: 100251 chars, 3704 lines
Benchmarking long_doc_100k/cold_first_frame
Benchmarking long_doc_100k/cold_first_frame: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 6.8s or enable flat sampling.
Benchmarking long_doc_100k/cold_first_frame: Collecting 10 samples in estimated 6.8117 s (55 iterations)
Benchmarking long_doc_100k/cold_first_frame: Analyzing
long_doc_100k/cold_first_frame
                        time:   [120.25 ms 121.84 ms 124.86 ms]
                        change: [-1.8532% +0.6817% +3.1808%] (p = 0.62 > 0.05)
                        No change in performance detected.
Benchmarking long_doc_100k/steady_state_top
Benchmarking long_doc_100k/steady_state_top: Warming up for 3.0000 s
Benchmarking long_doc_100k/steady_state_top: Collecting 10 samples in estimated 5.0167 s (14k iterations)
Benchmarking long_doc_100k/steady_state_top: Analyzing
long_doc_100k/steady_state_top
                        time:   [357.76 µs 360.55 µs 364.09 µs]
                        change: [-1.1553% -0.0752% +1.0505%] (p = 0.90 > 0.05)
                        No change in performance detected.
Benchmarking long_doc_100k/steady_state_scroll_middle
Benchmarking long_doc_100k/steady_state_scroll_middle: Warming up for 3.0000 s
Benchmarking long_doc_100k/steady_state_scroll_middle: Collecting 10 samples in estimated 5.0159 s (13k iterations)
Benchmarking long_doc_100k/steady_state_scroll_middle: Analyzing
long_doc_100k/steady_state_scroll_middle
                        time:   [368.16 µs 373.51 µs 380.19 µs]
                        change: [-1.3971% +0.7185% +3.0133%] (p = 0.56 > 0.05)
                        No change in performance detected.
Found 3 outliers among 10 measurements (30.00%)
  2 (20.00%) low mild
  1 (10.00%) high mild
```

前后对比表(M0 与 R1 数字取自 §5):

| 场景 | M0 中值 | R1 中值 | R3 中值 | R3 vs M0 | R3 vs R1 | criterion 检验 |
|---|---|---|---|---|---|---|
| `cold_first_frame` | 136.3 ms | 123.49 ms | 121.84 ms | **变好(−10.6%)** | −1.3% | No change(p=0.62) |
| `steady_state_top` | 415.3 µs | 358.83 µs | 360.55 µs | **变好(−13.2%)** | +0.5% | No change(p=0.90) |
| `steady_state_scroll_middle` | 430.2 µs | 371.36 µs | 373.51 µs | **变好(−13.2%)** | +0.6% | No change(p=0.56) |

顶部与中部差 12.96 µs(≈3.5%),视口剔除维持;距 18.18 ms(55 fps)预算余量 **48.7×**。**三项维持「优于 M0」判定,水位与 R1 持平。**

### 8.4 真窗口 scrollbench 复跑(命令 = §3b 同款)—— **blocked_external 状态未变**

```bash
target/release/examples/scrollbench     # 20 秒自动退出并打印统计
```

逐字输出(2026-10-02 R3,单次复跑):

```
scrollbench: 文档 100030 字符 / 4384 行
scrollbench: 没有采到帧
```

与 §3b 六次运行结果逐字相同(20 秒内 ui 帧数仍 <121)。环境旁证复查:kwin 合成仍 active(`dbus-send --dest=org.kde.KWin /Compositor org.freedesktop.DBus.Properties.Get string:org.kde.kwin.Compositing string:active` → `boolean true`;注:R1 用的方法调用形式本轮报 `No such method`,KWin 现仅暴露同值 property,读取结论一致)。§3b 的诊断链与结论(会话级 Vulkan WSI Fifo present 阻塞,非应用代码)原样沿用,**该项维持 blocked_external,不因本轮复跑解除**。

### 8.5 R3 结论汇总

| 项 | M0 | R1 | R3(本轮) | 判定 |
|---|---|---|---|---|
| 流式 500 行 | 38.7 ms(77.4 µs/行) | 33.800 ms | 33.853 ms(67.71 µs/行) | 与 R1 持平(No change) |
| 流式 2000 行 | 154.7 ms(77.35 µs/行) | 146.46 ms | 146.00 ms(73.00 µs/行) | 与 R1 持平(No change) |
| 流式 10000 行 | 789.3 ms(78.93 µs/行) | 1001.7 ms | 993.94 ms(99.39 µs/行) | 与 R1 持平(No change);对 M0 仍 +25.9% 变差 |
| 流式增长曲线 | 线性 O(n) | 线性偏超线性 | 线性偏超线性(×4.31 / ×6.81) | **O(n) 未解(与 R1 同判)** |
| 滚动·冷首帧 | 136.3 ms | 123.49 ms | 121.84 ms | 优于 M0,与 R1 持平 |
| 滚动·稳态顶部 | 415.3 µs | 358.83 µs | 360.55 µs | 优于 M0,与 R1 持平 |
| 滚动·稳态中部 | 430.2 µs | 371.36 µs | 373.51 µs | 优于 M0,与 R1 持平 |
| 滚动·真窗口 p50 | 60.3 fps(16.58 ms) | 不可测(blocked_external) | 不可测(**复跑同果,仍 blocked_external**) | 环境未恢复 |

10000 行档 R3 对 M0 换算:993.94 / 789.3 = +25.9%,与 R1 的 +26.9% 同量级(R1 用中值 1001.7;两轮各自中值对同一 M0 基准,差异在轮间噪声内)。

**一句话结论:R2 未做修复,复测即原水位——流式 O(n) 未解(曲线与绝对值均与 R1 无统计差异),滚动渲染路径维持优于 M0,真窗口 vsync 口径维持 blocked_external。** 修复验收命令与口径见 §6 第二条(拍板后复跑 §2 命令,判据 = 每行成本随规模趋稳);环境恢复后跑 §3b 命令与 m0-report §2.2 同表对照。

---

## 9. M3 复测与两轮修复(2026-10-04,#52 streaming-cache-blockkey M3·拍板①执行)—— **每行成本趋稳达成,10000 行档对 R1 −99.94%,M0 验证 4 凭本节数字销账**

> 前置:M1(`be05b52` 分段准入放宽)与 M2(`ccc2945` 块级缓存 key 换块内容 hash)已落在本分支。本节 = 同命令复测 → 发现新超线性 → 两轮修复(预算 2 轮,§公共约束)→ 终测判定与滚动路径回归归因。修复均属 vendor ①类(独立 `vendor:` commit 由编排收口,vendor/README 变更表 + vendored CHANGELOG 已登记)。
> **落库时序(2026-10-05 独立评审补记)**:销账落档 commit `3f19c2e` 只含 docs 五件——M3 代码在该时点系未提交工作区(6 文件 +401/−50),git 历史中 M3 不存在;纯 HEAD(3f19c2e)检出实跑 `cargo test -p egui_markdown --all-features` = **147** passed,§9.7 的 **152** passed 与本节全部终测数字对应叠加工作区 M3 后的代码。复现本节数字须以 M3 收口后的独立 `vendor:` commit 为检出基点(hash 收口后在 decisions-pending #77 销账注记与 vendor/README.md 变更表 M3 行回填);`3f19c2e` 的 `vendor:` 前缀系销账落档误用,勿据其认定 M3 已落库。M1/M2 hash 不受影响。
> **执行环境**:与 §1 相同(Deepin 25 / X11 / rustc 1.98.0);bench 二进制因 vendor 改动重编为 `target/release/deps/longdoc-b3c9b69313f2cdbc`(R1/R3 为 `longdoc-9a3abfe4edeb0714`,载体源码 `benches/longdoc.rs` 全程零改动,`git diff` 确认)。

### 9.1 首轮复测(仅 M1+M2,未修)—— 发现曲线**二次方**

命令 = §2 同款:

```bash
cargo bench -p latermd-app --bench longdoc -- streaming_append
```

逐字输出(2026-10-04,全量):

```
    Finished `bench` profile [optimized] target(s) in 0.15s
     Running benches/longdoc.rs (target/release/deps/longdoc-b3c9b69313f2cdbc)
Gnuplot not found, using plotters backend
long doc: 100251 chars, 3704 lines
Benchmarking streaming_append_by_lines/append_1_line_at_500
Benchmarking streaming_append_by_lines/append_1_line_at_500: Warming up for 3.0000 s
Benchmarking streaming_append_by_lines/append_1_line_at_500: Collecting 10 samples in estimated 5.0325 s (5775 iterations)
Benchmarking streaming_append_by_lines/append_1_line_at_500: Analyzing
streaming_append_by_lines/append_1_line_at_500
                        time:   [882.66 µs 894.34 µs 902.31 µs]
                        change: [-97.392% -97.363% -97.336%] (p = 0.00 < 0.05)
                        Performance has improved.
Benchmarking streaming_append_by_lines/append_1_line_at_2000
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Warming up for 3.0000 s
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Collecting 10 samples in estimated 5.0503 s (385 iterations)
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Analyzing
streaming_append_by_lines/append_1_line_at_2000
                        time:   [12.670 ms 12.782 ms 12.923 ms]
                        change: [-91.350% -91.167% -90.961%] (p = 0.00 < 0.05)
                        Performance has improved.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking streaming_append_by_lines/append_1_line_at_10000
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Warming up for 3.0000 s
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Collecting 10 samples in estimated 6.3504 s (20 iterations)
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Analyzing
streaming_append_by_lines/append_1_line_at_10000
                        time:   [317.08 ms 320.46 ms 323.62 ms]
                        change: [-68.326% -67.758% -67.216%] (p = 0.00 < 0.05)
                        Performance has improved.
```

(`change:` 对照的是 criterion 上一轮存档 = R3 §8,故百分比是「M1+M2 对未修」的改善。)三档绝对值全部大幅变好(10000 行档 1001.7ms → 320.46ms,−68%),**但增长曲线恶化**:

| 档位 | 中值 | µs/行 | 规模增长 | 成本增长 |
|---|---|---|---|---|
| 500 行 | 894.34 µs | 1.789 µs | — | — |
| 2,000 行 | 12.782 ms | 6.391 µs | ×4 | **×14.28** |
| 10,000 行 | 320.46 ms | 32.05 µs | ×5 | **×25.07** |

成本 ≈ 规模的 1.93 次方——**二次方**。按 §0 口径未达「每行成本趋稳」,激活修复轮。

### 9.2 根因定位(一次性探针,照 R1 §4-C 手法,测完已删)

临时探针 `crates/latermd-app/examples/streaming_profile.rs`(分阶段计时 + vendor 内临时 eprintln 行级计时,**测完已删,工作区净**),关键逐字证据:

1. **追加帧不重排版、不重高亮**:bench 文档追加行落在闭合围栏**之后**,fence token 不变 → flush 缓存命中(sections=32013, rows=8003, 8000 行档 `layout_job` 命中仅 ~1.0ms/帧,`highlight_code` 零调用)。M1/M2 机制按设计工作。
2. **热点 = `record_section_anchors`**:`compute_section_anchors`(label.rs)对**每个 section**从头重扫全部 rows + `job.text[..byte_start].chars().count()` O(n) 前缀重算。syntect 每个 code line 出一个 section → 8000 行 fence = 32013 sections × 8003 rows,单帧逐字实测:

```
[probe] render_galley sections=32013 rows=8003 glyphs=262970
[probe] record_section_anchors 197.223478ms
[probe] record_text_blocks 3.989µs
[probe] run_ui total 198.182835ms | ui closure 198.156ms
```

   195-205ms/帧,占整帧(198-207ms)的 **98%**——这就是二次方项。它同时是 R1 §2「线性偏超线性」里被线性项掩盖的隐藏二次项(外推 R1 10000 行档约 +300ms,与 R1 的 ×6.84>×5 相符):M1/M2 把线性主项杀掉后它显形。
3. 修复轮 1 后剩余主导 = flush 命中帧的 `cached.layout.job.clone()`(整 job 深拷贝)+ `Fonts::layout_job` 整 job 哈希找 galley(8k 行 ~1.0ms/帧)。

### 9.3 修复轮 1:anchors 单遍扫描(label.rs `compute_section_anchors`)

sections 与 rows 都按文档序,byte→char 游标与 row 游标各自只前进,一遍扫完;空行 `max(1)` 语义、末段沉底、byte_start 越界钳制等边界逐项保持。**值完全不变**(等价改写)。复测逐字输出:

```
Benchmarking streaming_append_by_lines/append_1_line_at_500
Benchmarking streaming_append_by_lines/append_1_line_at_500: Collecting 10 samples in estimated 5.0059 s (44k iterations)
streaming_append_by_lines/append_1_line_at_500
                        time:   [115.03 µs 116.18 µs 117.29 µs]
                        change: [-87.024% -86.859% -86.700%] (p = 0.00 < 0.05)
                        Performance has improved.
Benchmarking streaming_append_by_lines/append_1_line_at_2000
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Collecting 10 samples in estimated 5.0195 s (11k iterations)
streaming_append_by_lines/append_1_line_at_2000
                        time:   [439.07 µs 443.34 µs 447.79 µs]
                        change: [-96.656% -96.588% -96.520%] (p = 0.00 < 0.05)
                        Performance has improved.
Benchmarking streaming_append_by_lines/append_1_line_at_10000
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Collecting 10 samples in estimated 5.0919 s (2200 iterations)
streaming_append_by_lines/append_1_line_at_10000
                        time:   [2.2843 ms 2.3220 ms 2.3791 ms]
                        change: [-99.282% -99.270% -99.257%] (p = 0.00 < 0.05)
                        Performance has improved.
```

二次方项消除:500→2000 ×3.82(×4 规模)、2000→10000 ×5.24(×5 规模)——回到「线性小系数」(0.232→0.222→0.232 µs/行,趋稳但增长不显著低于规模)。未达验收,进入修复轮 2(预算最后一轮)。

### 9.4 修复轮 2:flush 缓存记住 shaped galley(+ 三项 perf 收尾)

- `CachedFlushRange` 增 `CachedShapedGalley { max_width, break_anywhere, pixels_per_point, galley, anchors }`:命中帧直接用已 shape 的 galley(与相对 anchors),跳过整 job 深拷贝与整 job 哈希;key 失配(换宽/换缩放)回落常规 shaping 路径;`map_job` 存在时的尾段不参与复用(app 未用 map_job,上游语义保留)。
- 连带三项:块高缓存 key 的 style+handler 份额从每块一哈希提为每 range 一哈希;`code_block_admits_segmentation` 字节长度快速否决(行数 ≤ 字节数,短 fence 免逐行扫描);anchors 随 shaped galley 缓存免重算。
- **踩坑(已写注释)**:egui 0.36 `Context::pixels_per_point` 取 context **写锁**,在 `ui.data_mut` 闭包内调用会自死锁——本仓测试当场抓到(10s RwLock 超时),读 ppp 必须提到 data 锁外。

终测(命令同 §2,逐字):

```
Benchmarking streaming_append_by_lines/append_1_line_at_500
Benchmarking streaming_append_by_lines/append_1_line_at_500: Collecting 10 samples in estimated 5.0017 s (133k iterations)
streaming_append_by_lines/append_1_line_at_500
                        time:   [35.206 µs 35.886 µs 36.482 µs]
                        change: [-5.5497% -3.8234% -1.9321%] (p = 0.00 < 0.05)
                        Performance has improved.
Benchmarking streaming_append_by_lines/append_1_line_at_2000
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Collecting 10 samples in estimated 5.0042 s (46k iterations)
streaming_append_by_lines/append_1_line_at_2000
                        time:   [104.88 µs 107.29 µs 109.56 µs]
                        change: [-6.3825% -4.5591% -2.5805%] (p = 0.00 < 0.05)
                        Performance has improved.
Benchmarking streaming_append_by_lines/append_1_line_at_10000
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Collecting 10 samples in estimated 5.0246 s (9075 iterations)
streaming_append_by_lines/append_1_line_at_10000
                        time:   [546.50 µs 550.75 µs 555.65 µs]
                        change: [-5.5887% -4.1767% -2.6438%] (p = 0.00 < 0.05)
                        Performance has improved.
```

### 9.5 判定(口径 = §0 / 任务书「每行成本随规模趋稳(亚线性)」)

| 档位 | M3 终测中值 | µs/行 | R1 中值 | 对 R1 | M0 中值 | 对 M0 |
|---|---|---|---|---|---|---|
| 500 行 | 35.886 µs | **0.0718 µs** | 33.800 ms | **−99.89%** | 38.7 ms | −99.91% |
| 2,000 行 | 107.29 µs | **0.0536 µs** | 146.46 ms | **−99.93%** | 154.7 ms | −99.93% |
| 10,000 行 | 550.75 µs | **0.0551 µs** | 1001.7 ms | **−99.945%** | 789.3 ms | −99.93% |

- **每行成本随规模**:0.0718 → 0.0536 → 0.0551 µs——**下降后趋稳**(末两档差 +2.8%,在 §0 的 ±5% 持平带宽内;R1 是 67.6→73.2→100.2 随规模上升)。✅
- **成本增长 vs 规模增长**:500→2000 成本 ×2.99(规模 ×4,低 25%);2000→10000 成本 ×5.13(规模 ×5,+2.6% 与线性持平);总体 ×20 规模成本 ×15.34(低 23%)。如实记录:2000→10000 段未达「显著低于」,残余线性项 = 整篇 parse/哈希/anchors 记录,合计 ~0.05 µs/行——两轮修复预算已用于两个主导项(二次方 anchors 与 O(n) job 拷贝+哈希),不再扩面(增量解析属另一工程量级)。
- **M0 验证 4 的原始痛点**(m0-report §4.2「超 ~1,300 行单 chunk 成本超 100ms 吐字节奏」):现 10000 行单 chunk = 550.75 µs = 100ms 预算的 **0.55%**(余量 181×);三档全部深藏预算内。**验证 4 凭本节销账**(m0-report §4.4 已按新数字落档)。

### 9.6 滚动路径回归核对(long_doc_100k,与本步否决线)—— 回归属实但主因不在本步

修复后同命令复跑 `cargo bench -p latermd-app --bench longdoc -- long_doc_100k`,逐字:

```
long_doc_100k/cold_first_frame
                        time:   [122.53 ms 123.90 ms 125.44 ms]
                        change: [-1.7899% -0.7989% +0.1813%] (p = 0.15 > 0.05)
long_doc_100k/steady_state_top
                        time:   [522.86 µs 528.05 µs 532.83 µs]
                        change: [+28.190% +29.619% +31.083%] (p = 0.00 < 0.05)
                        Performance has regressed.
long_doc_100k/steady_state_scroll_middle
                        time:   [559.06 µs 571.32 µs 578.67 µs]
                        change: [+28.359% +32.214% +36.216%] (p = 0.00 < 0.05)
                        Performance has regressed.
```

冷首帧持平;稳态两档对 R1(358.83/371.36 µs)+47%/+54%。**A/B 定位**(临时 env 开关逐项关断,测完还原,工作区净)定出构成:

| 分量 | 稳态 top 影响 | 归属 |
|---|---|---|
| #42 块表记录(block_span_rects,main `85cdf93`) | **+136 µs/帧** | **main 既有,先于本分支**(R1 基线 2026-10-02 未含 #42;`git log 618b3ab..origin/main -- vendor/` 仅此一个 vendor commit) |
| M1 准入门逐行扫描 | +29 µs/帧 | 本分支 M1;**本轮已修**(字节长度快速否决,终测数字已含) |
| M2 块 key 内容哈希 | +36 µs/帧 | 本分支 M2 的固有代价(内容寻址 cull 的语义必需,换 streaming 正确性,接受) |
| 本步 shaped galley 复用 | **−14 µs/帧** | 本轮 M3(净改善) |
| 残余未归因 | ~30-40 µs | 轮间噪声/环境漂移量级(cold 帧持平佐证环境可比) |

即:**去掉 main 既有的 #42 分量后,观测差 +33µs(+9%),其中可解释的本分支净贡献 = M2(+36)− 本步复用(−14)≈ +22µs(+6%)(M1 的 +29 已在本轮修掉),其余落在轮间噪声/环境漂移带内**;且距 18.18ms(55fps)预算余量仍有 34×。像素零变化否决线维持(152 项 vendored 测试全绿,含 M2 的热缓存像素指纹测试与本轮新增等价/复用测试)。#42 块表的每帧记录成本已按五要素登记 decisions-pending 新条目,交其归属模块处理,不在本步范围。

### 9.7 测试与验证(全部实跑)

- vendored:`cargo test -p egui_markdown --all-features` **152 passed / 0 failed**(含新增 5 测:section_anchors.rs +2——等价 oracle(4 文档矩阵,测试内嵌旧算法对照)+ 520 行 admitted fence 每行有锚;cache.rs +3——重复帧 painted text+anchors 逐项相等 / 换宽后 anchors == 全新 ctx 同宽 / admitted fence 流式追加新行可见)。
- 变异验证 2 次:row 游标「匹配即前进」→ 等价断言 8≠15 失败;丢弃 shaped galley 宽度 key → 换宽测试失败;还原后全绿(断言非恒真)。
- 探针载体 `streaming_profile.rs` 测完已删;vendor 内临时插桩(env 门控 eprintln/计时器/关断开关)全部还原,`git diff vendor/` 仅剩本轮正式改动。
- 六项门禁(fmt/三轮 clippy/test/doc)与 `vendor/egui_markdown/check.sh` 在本模块收尾时实跑,结果见 auto-plan/README 修订记录;真机目视项见 notes(blocked_external)。

---

## 10. M1 全面取证:基准汇总+补剖面+热点排序(2026-10-06,#59 perf-round M1·只测不改)

> **红线**:本模块只取证不改生产代码;新增 harness `crates/latermd-app/src/ui/perf_finding.rs` 为 `cfg(test)` 纯测试模块(照 `tab_switch_perf` 先例,生产构建不编译,分段计时全部公开 API);`vendor/` 的 A/B 归因用 **env 门控临时探针**(OnceLock + 环境变量,见 §10.2),**测后已逐字还原,`git diff vendor/` 为空**(本轮收尾时核实)。后续 M2 的修复沿 #99/#60 既有登记路线,本节只供数字。
> **执行环境**:与 §1 同机同会话(Deepin 25 / 内核 6.18.48-amd64-desktop-rolling / X11,DISPLAY=:0 / rustc 1.98.0);分支 `feature/perf-round`(head = `122bbc7`,与 origin/main 同步)。本节全部数字为**无头 CPU 路径**(`egui::Context::run_ui`,不涉 GPU/present),llvmpipe 软渲染为既知条件但与本节数字无耦合;真窗口 vsync 口径见 §10.1.4(仍 blocked_external)。
> **bench 二进制**:M3 vendor 改动落库后重编为 `target/release/deps/longdoc-e0de551c31baa257`(§9 为 `longdoc-b3c9b69313f2cdbc`);载体源码 `benches/longdoc.rs` 零改动(git 核实)。criterion `change:` 行对照的存档基线 = §9 M3 终测(2026-10-04)。
> **20k 规模的轮间漂移带**:同代码三组独立复跑(§10.3 [SCROLL] 稳态三次:13.8–15.3 / 12.0–13.1 / 11.6–12.5 ms)实证 ±10–15% 机器漂移,**跨轮绝对值对比一律以 criterion 统计检验或同构建 A/B 差值为准**,不拿跨构建绝对值下结论。

### 10.1 既有基准复跑(命令与输出逐字)

#### 10.1.1 流式追加(#46 §2 同命令)

```bash
cargo bench -p latermd-app --bench longdoc -- streaming_append
```

逐字输出(2026-10-06,全量):

```
    Finished `bench` profile [optimized] target(s) in 7.09s
     Running benches/longdoc.rs (target/release/deps/longdoc-e0de551c31baa257)
Gnuplot not found, using plotters backend
long doc: 100251 chars, 3704 lines
Benchmarking streaming_append_by_lines/append_1_line_at_500
Benchmarking streaming_append_by_lines/append_1_line_at_500: Warming up for 3.0000 s
Benchmarking streaming_append_by_lines/append_1_line_at_500: Collecting 10 samples in estimated 5.0015 s (132k iterations)
Benchmarking streaming_append_by_lines/append_1_line_at_500: Analyzing
streaming_append_by_lines/append_1_line_at_500
                        time:   [37.725 µs 38.340 µs 38.855 µs]
                        change: [+3.8241% +5.8621% +7.8848%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking streaming_append_by_lines/append_1_line_at_2000
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Warming up for 3.0000 s
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Collecting 10 samples in estimated 5.0036 s (42k iterations)
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Analyzing
streaming_append_by_lines/append_1_line_at_2000
                        time:   [111.92 µs 113.36 µs 115.37 µs]
                        change: [+4.1403% +5.8748% +7.6579%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
Benchmarking streaming_append_by_lines/append_1_line_at_10000
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Warming up for 3.0000 s
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Collecting 10 samples in estimated 5.0023 s (8635 iterations)
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Analyzing
streaming_append_by_lines/append_1_line_at_10000
                        time:   [563.66 µs 571.72 µs 577.22 µs]
                        change: [+2.1444% +3.7366% +5.1917%] (p = 0.00 < 0.05)
                        Performance has regressed.
```

对照(`change:` 基线 = §9.4 M3 终测存档;µs/行口径同 §0):

| 档位 | M3 终测 | 本轮中值 | 本轮 µs/行 | 对 M3 | 对 M0(R1 未修水位) |
|---|---|---|---|---|---|
| 500 行 | 35.886 µs | 38.340 µs | 0.0767 µs | +6.9%(p=0.00) | −99.90% |
| 2,000 行 | 107.29 µs | 113.36 µs | 0.0567 µs | +5.7%(p=0.00) | −99.93% |
| 10,000 行 | 550.75 µs | 571.72 µs | 0.0572 µs | +3.8%(p=0.00) | −99.93% |

- **增长曲线判定(口径 = §0)**:500→2000 成本 ×**2.96**(规模 ×4);2000→10000 成本 ×**5.04**(规模 ×5);每行成本 0.0767 → 0.0567 → 0.0572 µs,**下降后趋稳**(末两档差 +0.9%,在 ±5% 持平带宽内)。**§9.5 的「趋稳」判定维持,验收曲线无回归**。
- 绝对值三档 +3.8~6.9%:criterion 判 regressed(p=0.00),但幅度在 §10 头注的轮间漂移带内(同日两组复跑亦有 ±10%),且三档绝对值仍深藏 100ms 吐字预算(10000 行档 571.72 µs = 预算 0.57%)。**判定:达标水位维持,不构成回归证据**;M2 若动 vendor,复跑本命令时以同日基线对照。

#### 10.1.2 滚动稳态(#52 §9.6 同命令,#99 水位复核)

```bash
cargo bench -p latermd-app --bench longdoc -- long_doc_100k
```

逐字输出(2026-10-06,全量):

```
     Running benches/longdoc.rs (target/release/deps/longdoc-e0de551c31baa257)
Gnuplot not found, using plotters backend
long doc: 100251 chars, 3704 lines
Benchmarking long_doc_100k/cold_first_frame
Benchmarking long_doc_100k/cold_first_frame: Warming up for 3.0000 s

Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 6.9s or enable flat sampling.
Benchmarking long_doc_100k/cold_first_frame: Collecting 10 samples in estimated 6.9180 s (55 iterations)
Benchmarking long_doc_100k/cold_first_frame: Analyzing
long_doc_100k/cold_first_frame
                        time:   [122.83 ms 124.60 ms 128.09 ms]
                        change: [+0.8673% +3.0866% +5.5810%] (p = 0.03 < 0.05)
                        Change within noise threshold.
Benchmarking long_doc_100k/steady_state_top
Benchmarking long_doc_100k/steady_state_top: Warming up for 3.0000 s
Benchmarking long_doc_100k/steady_state_top: Collecting 10 samples in estimated 5.0033 s (8965 iterations)
Benchmarking long_doc_100k/steady_state_top: Analyzing
long_doc_100k/steady_state_top
                        time:   [544.41 µs 548.87 µs 551.70 µs]
                        change: [+2.6561% +4.0006% +5.3035%] (p = 0.00 < 0.05)
                        Performance has regressed.
Benchmarking long_doc_100k/steady_state_scroll_middle
Benchmarking long_doc_100k/steady_state_scroll_middle: Warming up for 3.0000 s
Benchmarking long_doc_100k/steady_state_scroll_middle: Collecting 10 samples in estimated 5.0246 s (9020 iterations)
Benchmarking long_doc_100k/steady_state_scroll_middle: Analyzing
long_doc_100k/steady_state_scroll_middle
                        time:   [557.19 µs 563.22 µs 569.37 µs]
                        change: [-0.4363% +2.5036% +5.7625%] (p = 0.15 > 0.05)
                        No change in performance detected.
```

| 场景 | R1(未修基线) | §9.6 | 本轮 | 本轮对 R1 |
|---|---|---|---|---|
| `cold_first_frame` | 123.49 ms | 123.90 ms | 124.60 ms | +0.9%(持平) |
| `steady_state_top` | 358.83 µs | 528.05 µs | 548.87 µs | **+52.9%** |
| `steady_state_scroll_middle` | 371.36 µs | 571.32 µs | 563.22 µs | **+51.6%** |

**#99 的滚动稳态 +47%/+54% 水位本轮维持**(+52.9%/+51.6%,同量级);冷首帧持平佐证环境可比。归因见 §10.2。

#### 10.1.3 tab_switch_perf(#39 口径,`--release --ignored`)

```bash
cargo test -p latermd-app --release tab_switch -- --test-threads=1 --ignored --nocapture
```

逐字输出(2026-10-06,全量):

```
running 1 test
test ui::tab_switch_perf::tab_switch_finding_report ... ==== M1 切换卡顿取证:样本 2000 行(doc_a 64236 字节 / doc_b 64236 字节,同一 ctx)====
[A] 归约/快照侧(打开与编辑时付,不在切换帧)
  expand_wikilinks(全文展开)                                     156.2 µs
  outline(全文标题扫描)                                            472.2 µs
[B] 预览渲染侧(每帧);heal 完整文档应为 Cow::Borrowed
  heal(完整文档) => Cow::Borrowed(恒等零拷贝)
  heal(完整文档逐行扫描)                                             186.9 µs
  parse(heal 后全文)                                            388.7 µs
  resolve_relative_images(有相对图,Some(base))                   96.8 µs
  resolve_relative_images(无相对图,借回)                           96.7 µs
[B2] MarkdownLabel 单件(pre-M2 常量 id 口径,id=preview-md,同一 ctx 连续帧)
  冷首帧 A(parse+layout+高亮全量)                                   110.91 ms
  稳态帧 A(缓存命中,7 帧中位)                                          573.5 µs
  切到 B 首帧(同 id 换文本 = miss)                                   7.03 ms
  稳态帧 B(缓存命中)                                                583.3 µs
  切回 A 首帧(往返:flush 段缓存残留与否)                                  6.60 ms
  对照:B 在全新 ctx 的冷首帧                                          88.93 ms
[C] TextEdit 单件(生产配置,同一 ctx)
  冷首帧 A(整篇 layout)                                           7.56 ms
  稳态帧 A(galley 缓存命中)                                         16.3 µs
  换到 B 首帧(文本变化 = 整篇 layout)                                  2.15 ms
  换回 A 首帧(往返)                                                1.66 ms
[D] 整帧(生产路径 LaterMdApp::draw,含全部面板)
  open_tab A(读文本建预览快照,同步)                                    1.06 ms
  open_tab B(同上)                                             888.7 µs
  稳态帧 A(5 帧中位)                                               584.8 µs
  归约 TabActivate(1)(switch_active 本体)                        0.6 µs
  切换后首帧                                                      89.21 ms
  切换后次帧                                                      997.4 µs
  稳态帧 B(5 帧中位)                                               657.3 µs
  往返切回 A 首帧                                                  2.45 ms
[E] 规模放大(切换首帧是否随文档规模线性;用户的「明显卡顿」按此口径外推)
  20000 行整帧稳态                                                12.52 ms
  20000 行切换后首帧                                               916.61 ms
  20000 行切换后次帧                                               18.62 ms
  20000 行往返切回 A 首帧                                           35.84 ms
  20000 行 MarkdownLabel 稳态(单件)                               21.40 ms
  20000 行 MarkdownLabel 换文本首帧(单件)                            89.44 ms
  20000 行旧路径(常量 id)往返切回 A 首帧(单件)                             92.16 ms
==== 结论速读(数字解释见报告)====
  切换首帧 89.21 ms/稳态帧 = 152.5×(A→B);往返首帧 2.45 ms/稳态帧 A = 4.2×
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 750 filtered out; finished in 3.73s
```

对照既有挂账口径:

| 项 | #39 M2 时代(2026-09-30) | 本轮 | 判定 |
|---|---|---|---|
| 2000 行切换后首帧 | ~91.5 ms(同规模非同构对照) | 89.21 ms | 持平 |
| 2000 行往返切回 A 首帧 | 2.33 ms(整帧) | 2.45 ms | 持平 |
| **20000 行冷首切** | **847–927 ms(#60 挂账)** | **916.61 ms** | **挂账维持,超 100ms 目标 9.2×** |
| 20000 行往返切回 A 首帧 | 31.1 ms | 35.84 ms | 同量级 |
| 20000 行整帧稳态 | (当年未单列) | **12.52 ms** | **新发现热点,见 §10.2** |
| 20000 行 MarkdownLabel 稳态(单件,heal 开) | (当年未单列) | **21.40 ms** | **同上;归因后 ≈78% 是块表记录** |

#### 10.1.4 真窗口 scrollbench(§3b 口径)—— 仍 blocked_external

```bash
timeout 30 target/release/examples/scrollbench
```

逐字输出(2026-10-06):

```
scrollbench: 文档 100030 字符 / 4384 行
scrollbench: 没有采到帧
```

与 §3b/§8.4 逐字相同(20 秒内 ui 帧数 <121),会话级 Vulkan WSI Fifo present 阻塞未恢复,**该项维持 blocked_external**,诊断链沿用 §3b 不重复。

### 10.2 #99 归因复测(块表记录,同构建 env 门控 A/B,测后已还原)

**方法**:在 `vendor/egui_markdown/src/label.rs` 临时加两处 OnceLock+环境变量探针(`PROBE_NO_BLOCK_TABLE` → `record_block_rect` 首行提前返回;`PROBE_NO_ANCHORS` → `record_section_anchors` 首行提前返回),一次构建跑四配置(criterion 落独立 baseline `probe_ab`,不污染默认存档),**测完逐字还原,`git diff vendor/` 为空**。手法沿 §9.2/§9.6 的「临时探针测完删」先例。

四配置矩阵(`cargo bench -p latermd-app --bench longdoc -- long_doc_100k --save-baseline probe_ab`,中值,µs):

| 配置 | steady_top | steady_middle | cold_first |
|---|---|---|---|
| A 基线(探针关) | 531.74 | 548.98 | 124.55 ms |
| B 关块表记录 | 406.48 | 431.49 | 123.74 ms |
| C 关锚点记录 | 537.90 | 536.55 | 123.22 ms |
| D 双关 | 407.26 | 416.55 | 125.91 ms |

(criterion change 行:A 之后各轮对上一配置自动比较,B 对 A = −23.8%/−21.2% p=0.00,D 对 C = −23.6%/−24.0% p=0.00,cold 全部 No change。)

同一探针构建下 tab_switch harness 20000 行口径(env off / `PROBE_NO_BLOCK_TABLE=1` / 双开,`cargo test -p latermd-app --release tab_switch -- --test-threads=1 --ignored --nocapture`):

| 行 | 探针关 | 关块表 | 双关 |
|---|---|---|---|
| 20000 行整帧稳态 | 12.13 ms | 3.86 ms | 3.63 ms |
| 20000 行 MarkdownLabel 稳态(单件) | 20.56 ms | 4.49 ms | 4.50 ms |
| 20000 行切换后首帧(冷) | 933.60 ms | 904.91 ms | 917.03 ms |
| 20000 行往返切回 A 首帧 | 33.13 ms | 21.73 ms | 23.80 ms |

**归因结论**:

1. **#99 复测确认,主因即块表记录**:100k bench 口径贡献 **+125.3 µs/帧(top)/ +117.5 µs(middle)**(A−B),与 §9.6 的 +136 µs 同量级——#99 归因成立,数字无漂移。B/D 回不到 R1 的 358.83 µs(差 ~+48 µs)= M2 内容哈希 +36 µs(已接受的固有代价)+ 噪声,与 §9.6 的构成表吻合。
2. **该成本超线性增长,20k 行规模成为一等热点**:单件稳态 20.56→4.49 ms,**块表 = 16.07 ms/帧(78%)**;整帧口径 12.13→3.86(+8.27 ms)。同一条路径在 3.7k 行的 longdoc bench(块表约千条)上只有 ~125 µs,20k 样本实测 4673 条记录要 16.07 ms——**块数约 3–5×、成本约 129×**:机理是「每记录固定开销(读改写 temp memory,线性)」+「每记录克隆整表 Vec(O(N²) memcpy,20k 规模下 4673²/2 × 56B ≈ 0.6 GB/帧)」叠加,大 N 进入二次项主导区。
3. **机制(本轮实读)**:`record_block_rect`(label.rs:441)每记录一条都 `get_temp`(egui temp memory 读取**克隆整张 Vec**)+ push + `insert_temp`,N 条记录 = O(N²) 拷贝;且 cull 路径(`render_token_range` 四处 + flush cull)对**视口外块也照记**——`block_span_rects` 公开 API 实测 20000 行稳态帧记录 **4673 条**(视口仅 ~40 行;4673 ≈ 667 节 × 7 块)。4673²/2 × 56 字节 ≈ 0.6 GB/帧 memcpy,与 16 ms 实测吻合。
4. **锚点记录不值得修**:C vs A 差在噪声内(±1%),20k 行帧锚点只记 **61 条**(只对可见 flush 段记录,`record_section_anchors` 在 `render_galley` 内、cull 段不进)——与块表不同源,标「不值得修」。
5. **块表与冷首切无关**:冷首帧 A/B/D 全部持平(122–126 ms)——miss 帧主导是 layout(§10.4),两热点独立。

### 10.3 补缺失剖面(新增 harness `perf_finding`,cfg(test))

跑法(与 `tab_switch_perf` 同口径,release;样本生成器同构,30 行一节,1280×800 无头帧):

```bash
cargo test -p latermd-app --release perf_finding -- --test-threads=1 --ignored --nocapture
```

逐字输出(2026-10-06 最终版;harness 演进说明:首版「换文本首帧」行同 ctx 同文本取中位,被 egui Fonts 层 galley 缓存命中掩盖成假 miss(29.6 µs),已改为逐帧异文真 miss 后定稿,演进口径见 harness 注释):

```
test ui::perf_finding::perf_finding_report ... ==== #59 M1 全面取证:样本 2000 行(64236 字节)/ 20000 行(645348 字节),1280×800 无头帧 ====
[EDIT] 大文档编辑帧(生产配置 TextEdit 单件,7 帧中位)
  -- 5000 行(160848 字节)--
  稳态帧(galley 缓存命中)                                               26.4 µs
  键入 1 字符帧(整篇重排)                                                 915.9 µs
  换文本首帧(逐帧异文 = 真 miss)                                           793.5 µs
  -- 10000 行(322671 字节)--
  稳态帧(galley 缓存命中)                                               49.6 µs
  键入 1 字符帧(整篇重排)                                                 1.57 ms
  换文本首帧(逐帧异文 = 真 miss)                                           1.71 ms
  -- 20000 行(645348 字节)--
  稳态帧(galley 缓存命中)                                               112.2 µs
  键入 1 字符帧(整篇重排)                                                 3.05 ms
  换文本首帧(逐帧异文 = 真 miss)                                           3.19 ms
[SCROLL] 滚动稳态帧(20000 行预览单件,id=perf-scroll-md,7 帧中位)
  冷首帧(对照,1 帧)                                                    863.85 ms
  稳态帧 offset=0                                                   12.30 ms
  稳态帧 offset=100000                                              12.47 ms
  稳态帧 offset=250000                                              12.28 ms
  稳态帧 offset=400000                                              11.96 ms
  稳态帧 offset=4000000                                             11.62 ms
  连续滚动帧(800px/帧,24 帧中位)                                          11.14 ms
  稳态帧记录面:块表 4673 条 / 锚点 61 条(视口内可见行 ~40)
[COLDSWITCH] 冷首切构成分解(2000 行)
  ① expand_wikilinks(打开时,快照侧)                                    95.4 µs
  ② heal 全文扫描(流式帧才开)                                             115.2 µs
  ③ parse 全文(缓存 miss 时)                                          296.6 µs
  ④ 冷首帧整帧(heal+parse+layout+高亮+记录)                               84.00 ms
  ⑤ 稳态帧(命中,对照)                                                   451.5 µs
  ≈差值归因:④−③−② ≈ layout+高亮+缓存构建 ≈ 83.59 ms(近似口径:①在快照侧不进帧,记录/哈希含在差值里)
  视口外延迟布局的可挽回上界(#60 路线)= ④−⑤ = 83.55 ms
[COLDSWITCH] 冷首切构成分解(20000 行)
  ① expand_wikilinks(打开时,快照侧)                                    1.00 ms
  ② heal 全文扫描(流式帧才开)                                             1.19 ms
  ③ parse 全文(缓存 miss 时)                                          3.11 ms
  ④ 冷首帧整帧(heal+parse+layout+高亮+记录)                               835.99 ms
  ⑤ 稳态帧(命中,对照)                                                   14.23 ms
  ≈差值归因:④−③−② ≈ layout+高亮+缓存构建 ≈ 831.68 ms(近似口径:①在快照侧不进帧,记录/哈希含在差值里)
  视口外延迟布局的可挽回上界(#60 路线)= ④−⑤ = 821.76 ms
[STARTUP] 应用启动(app 侧;eframe/wgpu/窗口创建与 fonts::install 属原生路径,无头不可测)
  LaterMdApp::default()(状态构造)                                    126.4 µs
  首帧(冷:字体图集+全部面板)                                                7.01 ms
  稳态帧(5 帧中位,示例文档)                                                138.2 µs
  open_tab(20000 行)+TabActivate 归约:8.393641ms(快照同步在建)
  大文档首帧(缓存全 miss)                                                883.99 ms
  大文档次帧                                                          12.87 ms
==== 完 ====
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 751 filtered out; finished in 6.63s
```

剖面读数:

- **[EDIT] 大文档编辑帧**:键入一字 915.9 µs(5k)→ 1.57 ms(10k)→ 3.05 ms(20k),**线性 O(n)**(每次键入 TextEdit 整篇重排);20k 行的 3.05 ms = 60fps 帧预算的 18%,**现规模在预算内**;稳态帧 26–112 µs 线性小系数,无碍。外推:>10 万行单文件键入才会破 16 ms 预算。
- **[SCROLL] 滚动稳态帧(20k 行,现行生产稳态口径 heal 关)**:各滚动偏移稳态 **11.6–12.5 ms,与滚动位置无关**(视口剔除位置无关性在 20k 规模维持);连续滚动 11.14 ms/帧。**但 12 ms 已贴 60fps 预算线(16.6 ms)**——构成见 §10.2:块表 ~16 ms(单件宽口径)/+8.3 ms(整帧窄口径)主导,纯滚动剔除本身无问题。注意此口径与 §10.1.3 [B2] 行(heal 开,21.40 ms)同源不同配置,heal 实测仅 +1.2 ms,余差 ~7 ms 在轮间漂移带 + 配置差内,未做插桩归因,如实存疑;归因锚点以 §10.2 同构建 A/B 为准。
- **[COLDSWITCH]**:构成 = layout+高亮+缓存构建 ~832 ms(20k)/ ~83.6 ms(2k),**占冷首帧 99.5%+**;parse(3.11 ms)/heal(1.19 ms)/expand_wikilinks(1.00 ms,且在快照侧不在帧内)皆是零头。
- **[STARTUP]**(app 侧可测部分):状态构造 126 µs、首帧 7.0 ms(一次性字体图集+面板)、示例文档稳态 138 µs、open_tab 20k 行 8.4 ms(快照同步建,一次性)——**启动路径无热点**;eframe/wgpu/窗口创建与 `fonts::install` 属原生路径,无头不可测(blocked_external,与 §3b 同因不同层:后者是 present 阻塞,前者是无窗口环境)。

### 10.4 #60 复测与路线评估(20000 行冷首切 847 ms)

三处独立测量同口径互证:tab_switch [E] **916.61 ms**(§10.1.3)、perf_finding [SCROLL] 冷首帧 **863.85 ms**、perf_finding [STARTUP] 大文档首帧 **883.99 ms**(探针构建下 904.91–933.60 ms,§10.2)——**#60 挂账(847–927 ms)维持,超 100ms 目标 8.4–9.3×**;2000 行同源(84.00 ms,超 100ms 线下但贴近)。

**vendor ①类路线评估(按 #60 已写明的「miss 帧视口外延迟布局」)**:

- 可挽回上界 = ④−⑤ = **821.76 ms(20k)/ 83.55 ms(2k)**:miss 帧只布局视口内 flush 段、屏外段沿用块级尺寸缓存/估计高度占位,则冷首切理论上收敛到「稳态帧 + 视口内 layout」≈ 几 ms 级。
- 可行性旁证:M1/M2/M3 后**热帧已经是分段命中**(⑤ = 14.23 ms,含块表 16ms 宽口径的重叠不可简单相减,量级证据而已),即「只算可见段」的每帧路径存在且被缓存命中路径每天在走;miss 帧缺的只是「屏外段先给占位高度、命中后补真值」。
- 已知风险(#60 备选 B 原文):估计高度失准 → 布局塌陷/滚动条跳动,需要「首刷占位 + 后帧校正」策略;块序号 widget id 纪律与 debug_assert 一致性(#77 点名连带)必须保持。
- 结论:**路线可达、收益 ~822 ms(98%),是 M2 两大主修对象之一**;工程量与风险显著高于块表修复(后者语义零变化),M2 若预算只够一项,优先级见 §10.5。

### 10.5 热点排序表(按可挽回成本,20k 行最坏场景口径)

| # | 热点 | 位置 | 实测成本 | 可挽回上限 | 路线 | 判定 |
|---|---|---|---|---|---|---|
| 1 | **冷首切全量 layout+高亮**(miss 帧视口外段照排) | vendor flush miss 路径(#60 已写明) | 836–934 ms/次(2k 行 84 ms) | ~822 ms(98%) | vendor ①类:视口外延迟布局 | **修**(超 100ms 预算 8.4×) |
| 2 | **块表记录 O(N²)**(每记录克隆整表 Vec,视口外块照记) | vendor `record_block_rect`(#42 落地,#99 挂账) | +16.1 ms/帧 @20k 单件稳态(78%);+125 µs @3.7k bench;+8.3 ms 整帧口径 | ≈全部(A/B 实证) | vendor ①类:帧内局部收集一次 insert(语义零变化) | **修**(20k 行稳态帧 12–20 ms 已贴 60fps 预算线;3.7k 行以上随规模二次方恶化) |
| 3 | 稳态帧 O(doc) 重哈希(flush ctx 哈希 + 块 key 内容哈希 + 整篇 text 哈希) | vendor `hash_flush_context`/`hash_block_content`/`hash_text` | A/B 关断后残差 ~3.5–4.5 ms/帧 @20k(B/D 配置 3.6–4.5 ms) | 部分(需段/块级摘要哈希,动缓存键语义) | vendor ①类 | **备选**(收益中、风险中;1/2 落地后预算富余再评估) |
| 4 | 编辑键入整篇重排 | egui TextEdit(egui 内建,非本仓非 vendor) | 3.05 ms/键 @20k,线性 | 预算内(18%) | — | **不值得修**(如实标注;>10 万行单文件再议) |
| 5 | section anchors 记录 | vendor `record_section_anchors` | ≈0(只记可见段,61 条/帧) | — | — | **不值得修**(A/B 实证噪声级) |
| 6 | 流式追加残余线性项 | 整篇 parse/哈希(§9.5 已记 ~0.05 µs/行) | 571.72 µs @10000 行档(预算 0.57%) | 微 | — | **不值得修**(#52 已销账,曲线趋稳维持) |
| 7 | 应用启动(app 侧) | 状态构造/首帧/open_tab | 126 µs / 7.0 ms / 8.4 ms,一次性 | 已达标 | — | 无需修 |

### 10.6 对 M2 的交接

1. **主修一(最小改动最大确定性):块表记录一次收集化**。`record_block_rect` 改为帧内局部 Vec 收集、帧末一次 `insert_temp`(或等价 get_mut 原地 push),「帧号键控、首写重置、跨帧读 None」契约(#78)不变;纯性能语义零变化,风险最低。验收 = §10.1.2 同命令稳态回到 #42 前水位(R1 358.83/371.36 µs 的 ±5% 带宽,即去掉 M2 固有 +36 µs 后 ~395–410 µs 一线)、tab_switch 20k 稳态行降到 ~4 ms 级;**#99 凭此销账**。
2. **主修二(大改,独立拍板):冷首切视口外段延迟布局**(#60 路线)。验收 = 20k 冷首切向 100 ms 逼近(≥8× 改善即 836→<110 ms 量级)、2k 口径 84→<20 ms;像素零变化否决线沿用 #52 M3(渲染结果不变,vendored 152 项测试全绿)。占位高度失准的塌陷/跳屏对策须先设计后动手(§10.4)。
3. 不承诺项:热点 3(稳态哈希摘要化)只在 1/2 落地后按剩余预算评估,不达标即如实挂账不硬凑。
4. 修复全部属 vendor ①类:按 AGENTS §6.9 独立 `vendor:` commit + vendor/README 变更表 + vendored CHANGELOG + 可 cherry-pick;M2 复跑验收命令 = §10.1.1/§10.1.2 两条 bench + §10.1.3/§10.3 两条 harness,验收口径 = 每项「前后数字对比」。

---

## 11. M2 对症修复:块表一次收集化 + 视口外高亮延迟(2026-10-06,#59 perf-round M2)—— **#99 销账;冷首切 6.0×(#60 的 ≥8× 未达,剩余按 #111 挂账)**

> 本节 = §10.6 交接的两项修复落地与前后数字逐字对比。主修一 = #99 归因项(块表记录 O(N²));主修二按 #60 路线取其**可安全子集**(视口外高亮延迟,无高度估计),估计高度型的全量延迟布局未做,按五要素挂账 decisions-pending #111。两修均属 vendor ①类(独立 `vendor:` commit 由编排收口;vendor/README 变更表 + vendored CHANGELOG 已登记)。
> **执行环境**:与 §10 同机同会话(Deepin 25 / X11 / rustc 1.98.0);分支 `feature/perf-round`。「前」基线 = 本节首次复跑(2026-10-06,criterion `change:` 对照 §10 存档),无头 CPU 路径,llvmpipe 软渲染为既知条件但与本节数字无耦合。
> **探针纪律**:冷帧构成剖面用 vendor 内 env 门控临时插桩(`PROBE_COLD` 累计器,照 §10.2 手法)+ 一次性 example `cold_profile.rs`,**测后逐字还原并删除**,`git diff` 仅剩正式改动(过程修正两处,均当场复原并以全量门禁复验:①探针移除时 layout.rs 的 `pub fn build_layout` 签名区一度被误删;②修复二的 tests/ 目录恢复操作一度连带抹掉修复一已入库的 block_span_rects 新测试,发现后补回并重跑其变异验证)。

### 11.1 修复一:块表 `record_block_rect` 一次收集化(vendor ①类)

`label.rs` 的 `record_block_rect` 旧实现每记录一条都 `get_temp`(克隆整张 Vec)+ push + `insert_temp` 写回——一帧 N 条记录 = O(N²) memcpy(§10.2 归因:20k 行稳态帧 4673 条 ≈ 16 ms/帧)。改为 `IdTypeMap::get_temp_mut_or_insert_with` 原地追加(push 均摊 O(1));「帧号键控、首写重置、跨帧读 None」契约逐字保持(每记录仍校验 `seen != frame` 即 clear,与 `render()` 帧首重置互为双保险);连带把 `render()` 帧首重置同样原地 `clear`,块表 allocation 跨帧复用。语义零变化,读侧 `block_span_rects` 不动。

**复测(命令 = §10.1.2 同款)**:

```bash
cargo bench -p latermd-app --bench longdoc -- long_doc_100k
```

| 场景 | 前(本轮基线,§10 水位) | 后 | criterion 检验 |
|---|---|---|---|
| `steady_state_top` | 542.28 µs | **426.04 µs** | −21.7%(p=0.00,improved) |
| `steady_state_scroll_middle` | 558.62 µs | **426.06 µs** | −22.8%(p=0.00,improved) |
| `cold_first_frame` | 127.58 ms | 122.51 ms | No change(p=0.06)——块表与冷帧无关,与 §10.2 归因一致 |

426 µs 落在 §10.6 预期带(R1 358.83 + M2 内容哈希固有 +36 µs ≈ 395–410 µs,+4% 在 §10 头注 ±10–15% 轮间漂移带内);对 R1 残差 +54 µs ≈ M2 固有 +36 µs + 噪声,构成与 §9.6 表吻合。

**20k 规模(perf_finding / tab_switch,命令 = §10.3/§10.1.3 同款)**:

| 行 | 前(§10) | 后 |
|---|---|---|
| 20000 行整帧稳态 | 12.52 ms | **3.66 ms**(−71%) |
| 20000 行 MarkdownLabel 稳态(单件) | 21.40 ms | **4.60 ms**(−78%;§10.2 探针 B「关块表」= 4.49 ms,吻合) |
| 20000 行切换后次帧 | 18.62 ms | 7.89 ms |
| 20000 行往返切回 A 首帧 | 35.84 ms | 23.61 ms |
| [SCROLL] 稳态帧(各 offset) | 12.38–12.45 ms | **3.15–3.38 ms**(§10.6 验收「~4 ms 级」达成) |

**#99 凭本节销账**(decisions-pending 已附注记)。

### 11.2 修复二:视口外代码块高亮延迟(#60 路线的可安全子集,vendor ①类)

**冷帧构成剖面(修复前探针,20k 样本,总 880.17 ms)**:syntect 高亮 **653.96 ms(74%)**/667 次 + shaping(`Fonts::layout_job`)201.41 ms(23%)+ build_layout 其余 ~7 ms。#60 假定的「视口外延迟布局」若按估计高度做,需要占位高度 + 后帧校正 + 滚动锚定(#60 备选 B 原文的塌陷/跳屏风险);本轮取其**可安全子集**——只延迟高亮(冷帧成本的 74%),不动几何:

- `MarkdownLabel::defer_offscreen_highlight(bool)`(默认 false,上游零变化):起始于视口下缘(clip rect 底)之外的 flush 段按**无高亮**构建——同一 padded 文本、同一 monospace 字号与默认 metrics(`append_plain_padded_line` 同款),**文本与几何逐字节相同、仅颜色缺席**;`CachedFlushRange` 增 `deferred_highlight` 标记,任一后续帧该段不再 below-fold 即整段带高亮重建(一次性,发生在滚入视口的当帧)。
- **无任何高度估计**:defer 判定读布局游标的精确 y(游标 = 已排段的真实高度之和),无占位误差、无校正、无跳屏。这是几何零变化否决线得以维持的关键。
- `build_layout` 增 `highlight_code_blocks: bool`(破坏性签名,CHANGELOG 已记);整篇 galley 路径与 `syntax_highlighting` 关闭构建不受影响。测试 tests/deferred_highlight.rs 3 例 + 变异验证 2 次(defer 恒关/补高亮恒关,各自对应断言如预期失败)。
- **复测变异发现并修复一处测试缺陷**:rehighlight 断言首版在 headless 下因 `scroll_to_rect_animation` 依赖输入时间推进(无头 `RawInput` 恒 time=0,动画永不前进)而恒假绿,加 `ScrollArea::animated(false)`(同 pass 立即应用)后断言真实生效;随后用「补高亮恒关」变异验证其判别力(失败符合预期)。

**复测数字(命令 = §10.1.3/§10.3 同款;app 侧 preview.rs 与两 harness 同步开启)**:

| 行 | 前(§10) | 后 | 改善 |
|---|---|---|---|
| [COLDSWITCH] 20000 行冷首帧整帧 | 855.02 ms | **143.42 ms** | **6.0×** |
| [COLDSWITCH] 2000 行冷首帧整帧 | 85.22 ms | **18.57 ms** | 4.6×(**§10.6 验收「84→<20 ms」达成**) |
| tab_switch 20000 行切换后首帧 | 916.61 ms | **194.87 ms** | 4.7× |
| perf_finding [STARTUP] 大文档首帧 | 883.99 ms | 189.51 ms | 4.7× |
| tab_switch 切换后首帧/稳态(2k) | 89.21 ms / 584.8 µs = 152× | 20.47 ms / 482.7 µs = **43×** | — |

探针口径(修复后同构建):总冷帧 875.72 → **172.81 ms**,高亮 651.18 → 22.01 ms,shaping 201.41 → **135.71 ms**(占位段每 fence 合并为单 section 的顺带收益),build_layout 661.22 → 25.41 ms。

**§10.6 验收口径对照**:2k 达标(<20 ms);20k 143 ms 对「≥8× 即 <110 ms」**未达(6.0×)**。剩余成本 = miss 帧对全文档 ~645 KB 文本的全量 shaping(135.71 ms)——继续压缩必须做**估计高度型的视口外延迟布局**(shaping 本身就是测高,跳过它只能用估计高度替代),即 #60 备选 B 的完整形态,引入占位误差/滚动跳动与锚定设计,按 §10.6「大改,独立拍板」挂账 **decisions-pending #111**,不硬凑。

### 11.3 修复后回归核对(bench 载体零改动,`benches/longdoc.rs` 未动)

```bash
cargo bench -p latermd-app --bench longdoc -- long_doc_100k    # 426.43 / 430.10 µs,cold 121.51 ms:对修复一复测 No change
cargo bench -p latermd-app --bench longdoc -- streaming_append # 37.55 / 116.96 / 574.00 µs:对 §10.1.1 在 ±5% 漂移带内,曲线判定(每行成本下降后趋稳)维持
```

两条 bench 载体(整篇 galley 路径 + admitted fence 流式路径)不经过新 flag 路径,数字证实零回归。

### 11.4 门禁(全部实跑)

```
$ cargo fmt --all --check                 # 通过
$ cargo clippy --workspace --all-targets -- -D warnings              # 通过
$ cargo clippy --workspace --all-targets --no-default-features -- -D warnings  # 通过
$ cargo clippy --workspace --all-targets --all-features -- -D warnings         # 通过
$ cargo test --workspace --all-features   # 1209 passed / 0 failed / 1 ignored(唯一 ignored = #39 取证测试,口径与 §7.4 一致;**勘误见 §12.3**:ignored 实为 3,#59 M1 perf_finding 与 #25 pdftotext 两项漏数,passed/failed 数不受影响)
$ cargo doc --no-deps --all-features      # 通过
$ bash vendor/egui_markdown/check.sh      # All checks passed
```

vendored 测试 159 → **163**(修复一 +1:block_span_rects 槽位复用测试;修复二 +3:deferred_highlight)。真机目视项:视口外高亮延迟的「滚入即补齐」在真窗口的手感(滚动节奏下颜色补齐不可察觉)属真机目视,无头不可测,列 blocked_external。

---

## 12. M3 收口复验与定稿(2026-10-06,#59 perf-round M3·文档收口)—— §11 全部四条验收命令独立复跑吻合,本轮次(§10/§11)数字定稿

> 本模块只落档不改代码:`crates/`、`vendor/`、`benches/` 零改动;auto-plan #59 状态行由编排收口,本节不写完成态。执行环境:与 §10/§11 同机同会话(Deepin 25 / X11 / rustc 1.98.0);两条 bench 增量构建 `Finished in 0.15s` 未重编,二进制与 §10 头注同一枚(`longdoc-e0de551c31baa257`);两 harness 复跑时工作区相对 §11 收尾仅 docs 改动。criterion `change:` 对照的存档 = §11 的 M2 复跑(M3 复跑前最后一次),故本节 change 行即「M3 vs M2」检验。llvmpipe 软渲染为既知条件,本节全部为无头 CPU 路径;真窗口 vsync 口径维持 §10.1.4 的 blocked_external。

### 12.1 复跑命令与逐字输出(§11 验收命令全量,2026-10-06)

（1）滚动 bench(命令 = §10.1.2 / §11.1 同款):

```bash
cargo bench -p latermd-app --bench longdoc -- long_doc_100k
```

逐字输出(头部四行构建信息与下条流式 bench 同构建逐字相同,复跑时经管道截取未捕获,自采样告警行起全量照录):

```
Warning: Unable to complete 10 samples in 5.0s. You may wish to increase target time to 6.7s or enable flat sampling.
Benchmarking long_doc_100k/cold_first_frame: Collecting 10 samples in estimated 6.7403 s (55 iterations)
Benchmarking long_doc_100k/cold_first_frame: Analyzing
long_doc_100k/cold_first_frame
                        time:   [120.55 ms 121.76 ms 122.67 ms]
                        change: [-1.4412% -0.2904% +0.9148%] (p = 0.66 > 0.05)
                        No change in performance detected.
Found 2 outliers among 10 measurements (20.00%)
  1 (10.00%) low mild
  1 (10.00%) high mild
Benchmarking long_doc_100k/steady_state_top
Benchmarking long_doc_100k/steady_state_top: Warming up for 3.0000 s
Benchmarking long_doc_100k/steady_state_top: Collecting 10 samples in estimated 5.0172 s (12k iterations)
Benchmarking long_doc_100k/steady_state_top: Analyzing
long_doc_100k/steady_state_top
                        time:   [423.37 µs 425.04 µs 426.66 µs]
                        change: [-1.1135% -0.1873% +0.7545%] (p = 0.72 > 0.05)
                        No change in performance detected.
Benchmarking long_doc_100k/steady_state_scroll_middle
Benchmarking long_doc_100k/steady_state_scroll_middle: Warming up for 3.0000 s
Benchmarking long_doc_100k/steady_state_scroll_middle: Collecting 10 samples in estimated 5.0090 s (11k iterations)
Benchmarking long_doc_100k/steady_state_scroll_middle: Analyzing
long_doc_100k/steady_state_scroll_middle
                        time:   [429.11 µs 440.12 µs 454.21 µs]
                        change: [-1.3624% +0.5419% +2.6790%] (p = 0.63 > 0.05)
                        No change in performance detected.
Found 1 outliers among 10 measurements (10.00%)
  1 (10.00%) high mild
```

（2）流式 bench(命令 = §10.1.1 / §11.3 同款,全量):

```bash
cargo bench -p latermd-app --bench longdoc -- streaming_append
```

```
    Finished `bench` profile [optimized] target(s) in 0.15s
     Running benches/longdoc.rs (target/release/deps/longdoc-e0de551c31baa257)
Gnuplot not found, using plotters backend
long doc: 100251 chars, 3704 lines
Benchmarking streaming_append_by_lines/append_1_line_at_500
Benchmarking streaming_append_by_lines/append_1_line_at_500: Warming up for 3.0000 s
Benchmarking streaming_append_by_lines/append_1_line_at_500: Collecting 10 samples in estimated 5.0004 s (135k iterations)
Benchmarking streaming_append_by_lines/append_1_line_at_500: Analyzing
streaming_append_by_lines/append_1_line_at_500
                        time:   [36.846 µs 37.322 µs 37.737 µs]
                        change: [-1.6464% +0.2409% +1.9818%] (p = 0.81 > 0.05)
                        No change in performance detected.
Benchmarking streaming_append_by_lines/append_1_line_at_2000
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Warming up for 3.0000 s
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Collecting 10 samples in estimated 5.0043 s (44k iterations)
Benchmarking streaming_append_by_lines/append_1_line_at_2000: Analyzing
streaming_append_by_lines/append_1_line_at_2000
                        time:   [110.21 µs 111.65 µs 112.91 µs]
                        change: [-6.1242% -4.3181% -2.5276%] (p = 0.00 < 0.05)
                        Performance has improved.
Benchmarking streaming_append_by_lines/append_1_line_at_10000
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Warming up for 3.0000 s
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Collecting 10 samples in estimated 5.0247 s (8855 iterations)
Benchmarking streaming_append_by_lines/append_1_line_at_10000: Analyzing
streaming_append_by_lines/append_1_line_at_10000
                        time:   [568.21 µs 575.72 µs 579.65 µs]
                        change: [-1.6680% -0.0907% +1.7308%] (p = 0.93 > 0.05)
                        No change in performance detected.
```

（3）tab_switch harness(命令 = §10.1.3 / §11.2 同款,全量):

```bash
cargo test -p latermd-app --release tab_switch -- --test-threads=1 --ignored --nocapture
```

```
    Finished `release` profile [optimized] target(s) in 0.16s
     Running unittests src/main.rs (target/release/deps/latermd-cbd4be27278583de)

running 1 test
test ui::tab_switch_perf::tab_switch_finding_report ... ==== M1 切换卡顿取证:样本 2000 行(doc_a 64236 字节 / doc_b 64236 字节,同一 ctx)====
[A] 归约/快照侧(打开与编辑时付,不在切换帧)
  expand_wikilinks(全文展开)                                     164.7 µs
  outline(全文标题扫描)                                            631.9 µs
[B] 预览渲染侧(每帧);heal 完整文档应为 Cow::Borrowed
  heal(完整文档) => Cow::Borrowed(恒等零拷贝)
  heal(完整文档逐行扫描)                                             173.9 µs
  parse(heal 后全文)                                            482.1 µs
  resolve_relative_images(有相对图,Some(base))                   93.9 µs
  resolve_relative_images(无相对图,借回)                           95.1 µs
[B2] MarkdownLabel 单件(pre-M2 常量 id 口径,id=preview-md,同一 ctx 连续帧)
  冷首帧 A(parse+layout+高亮全量)                                   21.62 ms
  稳态帧 A(缓存命中,7 帧中位)                                          439.2 µs
  切到 B 首帧(同 id 换文本 = miss)                                   6.60 ms
  稳态帧 B(缓存命中)                                                448.6 µs
  切回 A 首帧(往返:flush 段缓存残留与否)                                  6.47 ms
  对照:B 在全新 ctx 的冷首帧                                          21.22 ms
[C] TextEdit 单件(生产配置,同一 ctx)
  冷首帧 A(整篇 layout)                                           7.98 ms
  稳态帧 A(galley 缓存命中)                                         14.1 µs
  换到 B 首帧(文本变化 = 整篇 layout)                                  2.26 ms
  换回 A 首帧(往返)                                                1.71 ms
[D] 整帧(生产路径 LaterMdApp::draw,含全部面板)
  open_tab A(读文本建预览快照,同步)                                    1.06 ms
  open_tab B(同上)                                             926.8 µs
  稳态帧 A(5 帧中位)                                               465.0 µs
  归约 TabActivate(1)(switch_active 本体)                        0.5 µs
  切换后首帧                                                      19.95 ms
  切换后次帧                                                      686.8 µs
  稳态帧 B(5 帧中位)                                               487.0 µs
  往返切回 A 首帧                                                  2.19 ms
[E] 规模放大(切换首帧是否随文档规模线性;用户的「明显卡顿」按此口径外推)
  20000 行整帧稳态                                                3.73 ms
  20000 行切换后首帧                                               195.85 ms
  20000 行切换后次帧                                               7.64 ms
  20000 行往返切回 A 首帧                                           22.99 ms
  20000 行 MarkdownLabel 稳态(单件)                               5.26 ms
  20000 行 MarkdownLabel 换文本首帧(单件)                            72.44 ms
  20000 行旧路径(常量 id)往返切回 A 首帧(单件)                             76.52 ms
==== 结论速读(数字解释见报告)====
  切换首帧 19.95 ms/稳态帧 = 42.9×(A→B);往返首帧 2.19 ms/稳态帧 A = 4.7×
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 751 filtered out; finished in 1.08s
```

（4）perf_finding harness(命令 = §10.3 / §11.2 同款,全量):

```bash
cargo test -p latermd-app --release perf_finding -- --test-threads=1 --ignored --nocapture
```

```
    Finished `release` profile [optimized] target(s) in 0.16s
     Running unittests src/main.rs (target/release/deps/latermd-cbd4be27278583de)

running 1 test
test ui::perf_finding::perf_finding_report ... ==== #59 M1 全面取证:样本 2000 行(64236 字节)/ 20000 行(645348 字节),1280×800 无头帧 ====
[EDIT] 大文档编辑帧(生产配置 TextEdit 单件,7 帧中位)
  -- 5000 行(160848 字节)--
  稳态帧(galley 缓存命中)                                               35.3 µs
  键入 1 字符帧(整篇重排)                                                 768.8 µs
  换文本首帧(逐帧异文 = 真 miss)                                           794.7 µs
  -- 10000 行(322671 字节)--
  稳态帧(galley 缓存命中)                                               50.5 µs
  键入 1 字符帧(整篇重排)                                                 1.54 ms
  换文本首帧(逐帧异文 = 真 miss)                                           1.71 ms
  -- 20000 行(645348 字节)--
  稳态帧(galley 缓存命中)                                               115.2 µs
  键入 1 字符帧(整篇重排)                                                 3.14 ms
  换文本首帧(逐帧异文 = 真 miss)                                           3.20 ms
[SCROLL] 滚动稳态帧(20000 行预览单件,id=perf-scroll-md,7 帧中位)
  冷首帧(对照,1 帧)                                                    152.36 ms
  稳态帧 offset=0                                                   3.35 ms
  稳态帧 offset=100000                                              3.61 ms
  稳态帧 offset=250000                                              3.57 ms
  稳态帧 offset=400000                                              3.35 ms
  稳态帧 offset=4000000                                             3.34 ms
  连续滚动帧(800px/帧,24 帧中位)                                          6.15 ms
  稳态帧记录面:块表 4673 条 / 锚点 61 条(视口内可见行 ~40)
[COLDSWITCH] 冷首切构成分解(2000 行)
  ① expand_wikilinks(打开时,快照侧)                                    102.2 µs
  ② heal 全文扫描(流式帧才开)                                             122.7 µs
  ③ parse 全文(缓存 miss 时)                                          486.0 µs
  ④ 冷首帧整帧(heal+parse+layout+高亮+记录)                               19.14 ms
  ⑤ 稳态帧(命中,对照)                                                   363.0 µs
  ≈差值归因:④−③−② ≈ layout+高亮+缓存构建 ≈ 18.53 ms(近似口径:①在快照侧不进帧,记录/哈希含在差值里)
  视口外延迟布局的可挽回上界(#60 路线)= ④−⑤ = 18.78 ms
[COLDSWITCH] 冷首切构成分解(20000 行)
  ① expand_wikilinks(打开时,快照侧)                                    1.06 ms
  ② heal 全文扫描(流式帧才开)                                             1.22 ms
  ③ parse 全文(缓存 miss 时)                                          3.42 ms
  ④ 冷首帧整帧(heal+parse+layout+高亮+记录)                               146.23 ms
  ⑤ 稳态帧(命中,对照)                                                   3.29 ms
  ≈差值归因:④−③−② ≈ layout+高亮+缓存构建 ≈ 141.58 ms(近似口径:①在快照侧不进帧,记录/哈希含在差值里)
  视口外延迟布局的可挽回上界(#60 路线)= ④−⑤ = 142.94 ms
[STARTUP] 应用启动(app 侧;eframe/wgpu/窗口创建与 fonts::install 属原生路径,无头不可测)
  LaterMdApp::default()(状态构造)                                    129.1 µs
  首帧(冷:字体图集+全部面板)                                                6.84 ms
  稳态帧(5 帧中位,示例文档)                                                131.6 µs
  open_tab(20000 行)+TabActivate 归约:8.041503ms(快照同步在建)
  大文档首帧(缓存全 miss)                                                193.30 ms
  大文档次帧                                                          5.73 ms
==== 完 ====
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 751 filtered out; finished in 1.66s
```

### 12.2 对照与判定

| 项 | §11 记录(M2 轮) | M3 复跑 | criterion 检验 / 判定 |
|---|---|---|---|
| `steady_state_top` | 426.04 µs(§11.1) | 425.04 µs | **No change(p=0.72)** |
| `steady_state_scroll_middle` | 426.06 µs(§11.1) | 440.12 µs | **No change(p=0.63)**(+3.3%,轮间漂移带内) |
| `cold_first_frame` | 122.51 ms(§11.1) | 121.76 ms | **No change(p=0.66)** |
| 流式 500 行 | 37.55 µs(§11.3) | 37.32 µs | No change(p=0.81) |
| 流式 2000 行 | 116.96 µs(§11.3) | 111.65 µs | improved −4.3%(p=0.00,漂移带内向好) |
| 流式 10000 行 | 574.00 µs(§11.3) | 575.72 µs | No change(p=0.93) |
| tab_switch 20000 行切换后首帧 | 194.87 ms(§11.2) | 195.85 ms | 吻合 |
| tab_switch 20000 行整帧稳态 | 3.66 ms(§11.1) | 3.73 ms | 吻合 |
| tab_switch 20000 行单件稳态 | 4.60 ms(§11.1) | 5.26 ms | 同带(§10.2 探针 B 预测 4.49 一线) |
| perf_finding 20000 行冷首帧整帧 | 143.42 ms(§11.2) | 146.23 ms | 吻合([SCROLL] 口径 152.36 ms 同带) |
| perf_finding 2000 行冷首帧整帧 | 18.57 ms(§11.2,**<20 达标**) | 19.14 ms | **达标维持** |
| perf_finding [SCROLL] 各 offset 稳态 | 3.15–3.38 ms(§11.1) | 3.34–3.61 ms | 吻合(「~4 ms 级」维持) |
| perf_finding [STARTUP] 大文档首帧 | 189.51 ms(§11.2) | 193.30 ms | 吻合 |
| 20k 视口外延迟布局可挽回上界(#111 缺口) | ~136 ms(miss 帧全量 shaping) | 142.94 ms | 同带 |

- **判定一(#99 销账数字可复现)**:块表修复后的 426 µs 稳态水位 criterion 三项全部 No change,20k 稳态 3.7 ms 级、单件 4.6–5.3 ms 独立复跑吻合——decisions-pending #99 销账注记的数字定稿。
- **判定二(流式曲线达标维持)**:每行成本 0.0746 → 0.0558 → 0.0576 µs,下降后趋稳(末两档差 +3.3%,±5% 持平带宽内);成本增长 ×2.99(规模 ×4)/ ×5.16(规模 ×5),§9.5 / §10.1.1 的「趋稳」判定原样。
- **判定三(未达标项如实保留)**:20k 冷首切 **6.0×(143 ms 级)对 §10.6「≥8× 即 <110 ms」维持未达**——M3 复跑三次独立测量(139.07 / 146.23 / 195.85 ms,分别 perf_finding [COLDSWITCH]/[SCROLL] 与 tab_switch 口径)稳定在此带,缺口 = miss 帧对全文档全量 shaping(复跑可挽回上界 142.94 ms);已按五要素挂 decisions-pending **#111** 待人工拍板,#60 销账注记同轮落档(后续路线转 #111)。复验不改变未达标结论,不放松口径。
- 如实存疑(不在任何验收口径内):[SCROLL] 连续滚动帧 6.15 ms 高于单点稳态(3.3–3.6 ms),与 §10.3 的行位方向(连续 11.14 < 单点 11.6–12.5)相反;24 帧中位样本小,不解读,留后续取证棒顺带核。

### 12.3 §11.4 勘误与 M3 门禁(全部实跑)

- **勘误**:§11.4 原记「1209 passed / 0 failed / **1** ignored(唯一 ignored = #39 取证测试)」漏数两项。M3 在与 §11.4 完全相同的代码状态(HEAD `4a8f79c` 之后 `crates/` 零改动)实跑 `cargo test --workspace --all-features` = **1209 passed / 0 failed / 3 ignored**:① `ui::tab_switch_perf::tab_switch_finding_report`(#39 既有);② `ui::perf_finding::perf_finding_report`(#59 M1 新增,`5508a8e` 引入 `#[ignore]`,§10.3 跑法即 `--ignored`);③ `latermd-export` `pdf::tests::pdftotext_roundtrip_evidence`(#25 M2 `c0d1f7b` 起,依赖系统 pdftotext 的环境性 ignore)。三项均为有意不进常规门禁的取证/环境测试,与 §10.1.3/§10.3 的 `--ignored` 跑法互证;passed/failed 数与 §11.4 一致,不影响任何判定,仅汇总行漏数,随本轮定稿勘误。
- M3 轮门禁实跑(本模块收尾,输出摘要):

```
$ cargo fmt --all --check                                            # 通过(exit 0,无输出)
$ cargo clippy --workspace --all-targets -- -D warnings              # 通过
$ cargo clippy --workspace --all-targets --no-default-features -- -D warnings   # 通过
$ cargo clippy --workspace --all-targets --all-features -- -D warnings          # 通过
$ cargo test --workspace --all-features                              # 1209 passed / 0 failed / 3 ignored(构成见上)
```

- `cargo doc --no-deps --all-features` 未在本模块单跑(纯文档改动无文档面变化,照 §7.4 先例),由编排在本棒最终 head 六项全量复验。

## 13. #111 备选 B 落地:视口外估高延迟布局(2026-10-16,lazy-offscreen-layout)—— **拖宽帧 159→4.7 ms(20k,34×);live 整帧冷首切 217→89 ms(2.4×);滚动锚定 + 稳态零漂移有哨兵**

### 13.1 拍板与方案

2026-10-16 坤哥拍板执行 decisions-pending **#111 备选 B 的完整形态**:估高器(vendored ①类)+ app 侧块级视口外剔除(占位)+ 滚动锚定 + 落点二次校正。同轮把 2026-10-16 上午取证的「拖宽卡顿」(docs 无档,数字见 §13.3)一并收口 —— 拖宽与冷首切同根(全量 shape),同一机制(视口外只估不 shape)同时治两症。

机制(与 §10.6 备选 B 写的路线一致,细化后落地):

- **vendored 估高器**(`vendor/egui_markdown/src/estimate.rs`,①类可合):`MarkdownLabel::estimated_height(ui)` —— 结构化 token 遍历(镜像 `build_layout` 的行语义与 `render_token_range` 的分段 flush 语义:空行一整行盒、标题 spacer 行、列表槽宽、fence 逐行单段、`after_block`/`before_block` 间距与尾随空行裁剪),以逐字 `glyph_width` 求和折算行数(width 单调,缓存于 id+文本 hash,重复调用纯算术)。精度:2k 混排样本 10% 容差内过,vanilla fence 5% 内过,单调性钉住。
- **app 块级剔除**(`live.rs` 块循环头):非活动块起于裁剪区外(上/下两向,上向以「cursor_y + 已知高(上一帧实测或本帧估高)」判定 —— egui 0.36 无限高 content Ui 的 `cursor().bottom()` 恒 inf,不可用)→ 不进 `MarkdownLabel::show`,以 `estimated_height + 4px`(TextEdit 内距同款)占位。富渲染配置逐项镜像真实臂(handler/字体/滚代码框头/heal),估高与真实渲染同源。命中表存在性(`block_span_rects_present`,①类新公开探针)即「本帧是否真实渲染」的判据。
- **滚动锚定**(`estimate_drift`):视口上方占位块「占位 − 上一帧实测」的差值累计,ScrollArea 收尾前同向修正 `state.offset`(键 = `panel.make_persistent_id(id_salt)`,与 ScrollArea 内部派生同键 —— 内容闭包内派生键是错的,子 Ui id 链不同)。下方块的差只影响滚动条长度,不补偿。真实渲染块帧末记实测高(`live-render-h` per-block temp),占位替换帧据此比对。
- **落点二次校正**:锁定浏览跳转的目标块在屏外时,占位矩形同样走 `scroll_to_rect`(与真实臂同一落地),滚入视口后转真实渲染,几何随实测自校 —— 不需要额外的校正通路。
- **铁律守住**:单一解析器(估高器走同一 `parser::parse`,只是不 shape);widget id 稳定(占位块不新建 label id);单一富渲染路径(视口内照旧 `live::ui`,占位只是不渲染,不是第二渲染器)。

### 13.2 观感风险与守门(备选 B 当年登记的「估计高度失准 → 布局塌陷/滚动条跳动」)

- 视口内几何**当帧精确**(被裁到的块当帧即真实渲染,无占位混入视口);滚动中无跳变。
- 占位与实测的差全部落在视口外的滚动条长度上;上方块差值按 `estimate_drift` 抵消,锚定哨兵钉「同视口稳态帧 offset 漂移 < 0.5px」。
- 首帧滚动条长度按估高给(与实测差在 10% 容差带),后续帧随块转真实渲染逐块收敛 —— 滚动条长度有一次性的软变化,无内容跳动(真机手感项,留真机验收)。
- 剔除哨兵钉行为面:滚到文末/文首后,屏外块无本帧命中表(= 不 shape),视口内有。

### 13.3 前后数字(release 无头,同一机器,llvmpipe 条件与 §10/§11 相同)

拖宽取证(2026-10-16 上午,main 之上,临时 harness 已删):MarkdownLabel 单件(旧对照列口径)拖动帧中位 **2000 行 14.32 ms / 20000 行 159.23 ms**,最大 184 ms,比稳态贵 22–28×;600↔601 抖动帧 15.2/166 ms。60fps 预算 16.6 ms,**2000 行单列拖动即吃满,20k 是 6fps**。

落地后(同 harness 口径复测,`drag_width_perf` 临时模块,已删):

| 场景 | 前(单件 MarkdownLabel) | 后(live::ui 整帧) | 倍率 |
|---|---|---|---|
| 2000 行拖动帧中位 | 14.32 ms | **0.68 ms** | 21× |
| 20000 行拖动帧中位 | 159.23 ms | **4.72 ms** | **34×** |
| 20000 行拖动帧最大 | 184.09 ms | 5.22 ms | 35× |
| 2000 行冷首帧 | 25.08 ms | **41.3 ms→(见注)** | — |
| 20000 行冷首帧 | 216.32 ms | **88.74 ms** | 2.4× |

注:live 整帧冷首(41/89 ms)与单件冷首(25/216 ms)口径不同 —— live 是逐块标签 + 块循环 + 每块独立估高/剔除,20k 的 89 ms 里含全部 667 块的估高器首次建模(parse + 逐字宽度和)与视口内块的 shape+paint;对照旧口径的整帧冷首切(tab_switch 整帧 195.85 ms / perf_finding 146.23 ms,§12.2)是 **2.2–2.4×** 改善。2000 行档 live 冷首 41 ms 高于单件 25 ms,是块级逐标签的固有开销(667 块每块一个 label 的 parse 与 job 构建),冷帧一次性、缓存热后稳态 542 µs 与单件同带。

对 §10.6 验收线「20k 冷首切 <110 ms」:live 整帧口径 88.74 ms **首次达标**(此前 143–196 ms 带,6.0×);换算 tab_switch 整帧口径(历史 195.85)按同比例预期 ~85–95 ms,以合入后 CI/复跑为准。

### 13.4 落点与守门测试

- vendored:estimate.rs 6 测(容差/单调/稳定/fence 近精确/边界),全套 87+ 绿。
- app:`live/live_offscreen_tests.rs` 2 哨兵(剔除:跳文末/文首的命中表存在性双向;锚定:跳中部收敛后两帧稳态 offset 漂移 <0.5px + 离开文档顶)。全套 1081 passed / 0 failed(基线 1079 + 2 新)。
- 既有回归零变化:1079 全绿含 tab_switch 两条缓存哨兵(命中/miss 断言)与 locked_browse 跳转族 —— per-tab 缓存语义未动。
