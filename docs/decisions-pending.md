# 待用户决策记录（decisions-pending）

> 自动开发循环遇到「本该问用户」的岔路口时，在这里登记：**岔路是什么、自动选了什么、为什么、想改怎么改**。
> 选择由循环自行做出并继续执行，不阻塞；用户事后翻此文件，按「如何改」一节操作即可推翻。
> #30 曾是「等待型」条目（改窗口形态本身，返工成本高），2026-09-26 用户放行后已按默认全部落地（M1–M4 合入 main）。
> 编号 #161 为当前最新条目(2026-10-24 #160 macOS accesskit 止血、#161 命令行入口安装;2026-10-19 写作模式体验批次为 #154–#159;此前 #149–#153 为 live-unify P6 批次)。历史遗留:#22/#30 各存在两条(2026-09-25/26 旧批次撞号,非 #66 引入;重编号牵连 auto-plan/README/commit 信息等跨文件引用,待专项处理)。跨分支预警(2026-10-09,#72 文档收尾记):孤儿 commit `0820817`(#70 落档,PR#146 合入后补推在 feature/titlebar-search-and-view-switch,未进 main)亦定义 #133/#134(#70 设置弹窗两条);本轮 #133/#134(#72 查找/跳转浮卡)已被 `crates/latermd-app/src/ui/layout.rs` 注释与 #135 引用,不动;该线内容将来搬运进 main 时须给其重编号(建议顺延 #136/#137)——已于 #70 收口移植(2026-10-09)执行:该线两条现编为 #136/#137(见下),撞号解除。

## #142 minimap「全屏不可拖」是真缺陷,不是实例/面板问题:`jump_ratio` 的第一道闸门把「minimap 铺不满窄条」误判为「编辑器滚不动」,短文档在全屏下拖不动;#132 结论被取代(2026-10-09,#68 后续复核·自动拍板)

- **岔路**:①坤哥两次真机(2026-10-08、2026-10-09)「小窗可拖、全屏不可拖」到底是 #132 归出的实例/环境因素,还是代码真缺陷;②修法取向——放宽 `jump_ratio` 的 `None` 闸门 / 改掉「行条固定 3px」的量级失配 / 短文档 minimap 按比例拉伸铺满窄条。
- **已做**:先用无头探针取证(临时测试,取证后已删除):editor 层固定 1920×1008 视口,扫 80/120/200/300/500 行五档,读 `ScrollMetrics` 实测量出这张表——

  | 行数 | 编辑器 travel(px) | minimap 行程(总行高−窄条高) | 拖到底后 offset |
  |---|---|---|---|
  | 80 | 196 | −768 | 0 |
  | 120 | 796 | −648 | 0 |
  | 200 | 1996 | −408 | 0 |
  | 300 | 3496 | −108 | 0 |
  | 500 | 6496 | +492 | 2013 |

  300 行以内的文档**编辑器明明有几百到几千 px 行程,minimap 却纹丝不动**。根因就是 `jump_ratio` 的第一道闸门 `if travel <= 0.0 { return None }`:它把「minimap 自己没铺满窄条」(`full = 行数×3px` 小于窄条高)当成了「编辑器也滚不动」。这两件事的阈值差一个量级——编辑区净高 H 时编辑器 `H/18` 行就有行程(H=900 → 约 50 行),minimap 要 `H/3` 行才铺满(约 300 行)。**恰好解释「同一篇文档小窗能拖、最大化后不能拖」**:窗口一变大,H 变大,minimap 的行条总量相对窄条更空,而编辑器反而更有行程。上一轮 #132 的排查方向是对的,但取证样本全都用了「够长」的 500 行(minimap.rs 那条与 layout 层那条都如此),正落在闸门放行的一侧,所以无头两档视口全绿。
  修法选**放宽闸门**,不选取代 `ROW_H` 或做拉伸:

  `crates/latermd-app/src/ui/minimap.rs` 三处(零新增依赖、vendor/ 零改动)——
  ①删掉 `if travel <= 0.0 { return None }`,`None` 闸门只留 `ve >= 1.0`(内容不满一屏是唯一真无行程的情形);
  ②`grab` 臂的滑轨从 `map_h − ve·full` 改为统一式 `span = (1−ve)·full − travel`:长文档时 `travel = full − map_h`,代入即逐位退化回 #68 的原式(四条 `grab_*` 纯函数测试零改动继续绿);短文档 `travel = 0` 时退化成 `(1−ve)·full`,正是 `viewport_highlight` 里 `top` 的实际系数,「抓取点相对阴影不动」的对偶照旧成立;
  ③`None` 语义的文档注释重写:`travel ≤ 0` 时 `window()`/`viewport_highlight()` 顶部铺排、`offset` 恒 0,是「量纲自洽」而非「不可拖」。
- **为什么不取另两条**:①改 `ROW_H`(把行条加粗让短文档也能铺满)先动的是观感,且仍随行数/视口变化,达不到「恒成立」;`ROW_H` 是 #104 已拍的比例尺口径。②给短文档做「拉伸到铺满窄条」——要给 `LineParams` 加新维度(缓存键跟着变),且拉伸后行条与编辑器行的 1:1 对应被打破,点击/拖动的定位精度反而下降。③保留旧闸门、只在 UI 上提示「文档太短」——编辑器明明滚得动,提示是骗人的。
- **验证**:新增端到端回归 `dragging_short_document_works_at_fullscreen_viewport`(200 行 × 1920×1008,先钉「编辑器 travel >100」「minimap 全高 < 窄条高」两个前提,再拖 6 步要求 ≥3 步在滚);改写 `grab_keeps_none_precedence_on_short_or_degenerate_inputs`(删掉「短文档应 None」这条反向断言,补 travel=0 时仍走滑轨公式且与 `viewport_highlight` 对偶)。**红绿双向已实跑**:把闸门变异回 `travel_gate <= 0.0`,这两条当场 FAILED,还原后绿。`--features` 下 `latermd-app` 该模块 41 passed;`cargo fmt` 对改动文件干净、`cargo clippy` 对本文件零告警、`cargo doc --no-deps --all-features` 零 warning。
- **如何改**:①将来若真要「minimap 没铺满时什么都不做」——在 `editor.rs` 的交互注册段加 UI 层判定,不要回到纯函数的 `None` 语义里。②改几何时 `span` 与 `viewport_highlight` 的 `top` 系数必须始终是同一式(改一个就得改一对),否则拖影「抓取点不动」的性质失效。③3px vs ≥18px 这个量级差是本组件的性质,别指望通过调 `ROW_H` 消掉——可行的最小地法令已在本次做完。
- **与 #132 的关系**:#132 原文留档不删,但其「main 代码未找到尺寸相关缺陷」的结论由本条取代——取向对了,取证样本落在放行的一侧。**备注**:同工作区另有他人未提交改动(`fonts.rs` / `latermd-export/src/pdf/cjk.rs` / `docs/README.md`),其引起的 `preview_typography_acceptance` 一条失败与 `latermd-export` 编译错与本改动无关(以 HEAD 版 minimap 复跑同样失败),未处理。

## #141 #71 文档收尾的落点与编号口径:本轮(2026-10-09)在既有工作区未提交改动(#138 M1 条目/#139 M2 条目/#140 评审条目,系实现者与评审轮所写)之上只做**追加**不改既有条目——#139 与 #140 的实现侧技术口径已分别由实现者/评审轮登记完毕,本条只补文档收尾自身的取舍与未竟留痕:①auto-plan #71 行落位在 #70 与 #72 之间(队列表按 # 编号排序,「排队顺序」注记区照既有惯例在区块顶部追加「第十批第二轮」一行,不改写上一轮注记原文——上轮「#71 条目尚未登记」字样是当时事实,由本轮新注记收口而非篡改);②roadmap「当前位置」经核对无涉及(帮助菜单/关于窗/检查更新不在任何阶段条目与 P0–P3 范围表内,是 #70 同类的桌面惯例补齐,不触发 roadmap 更新);③menu-coverage.md 的 #71 注记(「设置」「帮助」非命令不进矩阵+标题助记集加 H)系实现者随模块已写的工作区未提交改动,本轮原样保留不重写;④真机留人工四项(真实 GitHub 请求联通/三平台关于窗观感/按钮禁用态手感/X 按钮真机点击)与 Cargo.lock +1 行待编排收口提交,均已写进 auto-plan #71 行备注,此处不重复展开(2026-10-09,#71 文档收尾·自动拍板)

- **岔路**:文档收尾轮要不要把实现者已写的 #138/#139/#140 三条工作区未提交条目重写并融入,以及 roadmap/menu-coverage 是否需要本轮再动。
- **备选**:①重写融合既有条目(统一口径/单一署名);②只追加不改动,收尾条目单独编号(本条选定);③roadmap/menu-coverage 一并重写。
- **自动选择**:②。三条既有条目(实现者 M1 五口径/M2 六口径/评审 lock 漂移修法)内容完整、五要素齐全(已逐条读过),重写只有口径漂移风险没有信息增益;roadmap「当前位置」通读后确认帮助菜单/关于窗/检查更新不在任何阶段条目内(阶段 4.5 打磨窗口与阶段 5 深水区的条目清单均不含),不强行为其加行;menu-coverage 的 #71 注记是实现者随代码改动同批写的(与代码同真源),文档收尾重写反而制造两个作者版本。
- **理由**:文档收尾的职责是把进度落到档上,不是重写历史;既有条目已被 auto-plan #71 行引用(#138/#139/#140),编号稳定性优先。
- **如何改**:若坤哥希望合并 #138/#139/#140/#141 为单条「#71 全口径」,把四条正文并成「岔路①-⓯」后删此三条,同步改 auto-plan #71 行与 docs/README.md 修订记录的编号引用。

## #140 #71 评审「lock 漂移」的修法:lock 变更留工作区交编排补提交,不在本模块内擅自动 git——3527f09 把 ureq 提升为 latermd-app 直接依赖(Cargo.toml:43)却未一并提交 Cargo.lock 的 `+ "ureq"` 一行,HEAD 的 lock 与 Cargo.toml 不同步;已核实工作区那一行 diff 正是 Cargo 按最新 Cargo.toml 生成的必要同步(非手动伪造),且 ureq 3.4.2 包条目在分支基点 b47120e 的 lock 里就有(latermd-ai/latermd-bed 直依赖),纯提升零新增编译面,vendor/ 零改动,修法就是把这行提交掉(2026-10-09,#71 评审修复·自动拍板)

- **岔路**:独立评审发现 medium——依赖提升的 lock 同步行没随 3527f09 入库,工作区 Cargo.lock 一直处于未提交状态;修法是只修 lock 文件本身,还是连同提交纪律一起处理。
- **备选**:①本模块自行 `git commit` Cargo.lock;②重跑 `cargo update -p latermd-app` 之类命令重新生成再比对;③如实上报交由编排收口提交(本条选定)。
- **自动选择**:③。已核实:工作区 Cargo.lock 相对 HEAD 的全部差异就是 hunk `@@ -2601,6 +2601,7 @@` 的 `+ "ureq"` 一行,落点正是 `latermd-app` 的 dependencies 列表(serde_json 之后,字母序正确);`cargo metadata --locked` 在此工作区通过,证明 lock 与 Cargo.toml 已经同步;公共约束明令本模块不得执行 git commit/push,「补提交」动作天然属于编排收口。
- **理由**:评审要求的实质是「lock 与 Cargo.toml 同步入库」,唯一缺的是 commit 这个动作,而该动作被公共约束显式保留给编排;重新生成 lock 无必要(内容已正确且最小);CI(rust.yml)各步骤未带 `--locked` 属仓库既有口径,不在本任务(#71)修复面内动它。
- **如何改**:若编排收口时发现该行与后续改动冲突,以 `cargo metadata --locked` 通过为准重新生成;若希望 CI 拦住未来 lock 漂移,另开任务给 rust.yml 的 fmt/clippy/test/doc 四步加 `--locked`(属基础设施改动,不混入 #71)。

## #139 #71 M2「检查更新」六口径:结果态四分支含「无法判断」(响应缺 `tag_name` 或 tag 非 semver 不冒充失败也不冒充最新)、版本比较纯函数剥 v 前缀截预发布后缀按三段数字比(不引 semver crate)、失败文案只留一句话不带响应体、按钮行进 #70 两列基线(行标签「检查更新」与按钮同名)、重绘驱动照模型列表(在途 request_repaint,收尾自然停)、ureq 从传递依赖提升为直接依赖(照 arboard 先例,零新增编译面)(2026-10-09,#71 M2 about-update-check·自动拍板)

- **岔路**:①请求成功但 tag 缺失/非 semver 时算「失败」还是「无法判断」;②版本比较引 semver crate 还是手写;③失败文案形态;④「检查更新」行的排版;⑤在途检查的重绘驱动;⑥ ureq 依赖声明方式。
- **备选**:①缺 tag 报错/归「无法判断」;②semver crate/手写三段比较;③带响应体片段(照模型列表 status_failure)/一句话;④独立横排按钮/两列 settings_row;⑤不显式要帧/在途 request_repaint;⑥ureq 走 latermd-ai 转发(给它加公共函数)/latermd-app 直接依赖。
- **自动选择**:①**「无法判断」独立分支**(`UpdatePhase::Unknown { tag: Option<String> }`):请求成功说明网络与 GitHub 都正常,缺 `tag_name` 或 tag 形如 `nightly` 时报「失败」会误导用户以为网络坏了,报「最新」更错;显示 tag 原文让用户自己判断。②**手写** `compare_release_tag`:发布渠道的 tag 就是纯三段,预发布段不参与排序(截断 `-rc`/`+build` 后按 `major.minor.patch` 数值比);两侧对称剥 v/V 前缀(纯函数不假设调用方形态);引 semver crate 为一个 30 行函数加一个新依赖,不划算(任务书明示零新增依赖)。③**一句话**(`检查更新失败:{HTTP 码或网络错误}`):GitHub 错误 JSON 很长对用户无信息量,且「失败可重试」的错误行已由颜色(WARN)与按钮文案(变「重试」)表达。④**两列基线**:与「仓库」「许可协议」两行同构,标签列定宽对齐;按钮在途禁用 + 悬停说明(模型列表按钮同款),失败后文案变「重试」直指可重试语义。⑤**在途 request_repaint**(layout.rs reduce 侧,与 poll_models/is_fetching 同款四连):egui 空闲不来帧,结果到达要靠下一帧收流,不显式要帧会悬到无关重绘。⑥**直接依赖**:任务书明示(照 arboard 先例);Cargo.toml 注释注明「3.4.2 rustls 已在依赖树,提升零新增编译面」,经 latermd-ai 转发反而要给隔壁 crate 扩公共 API 面。
- **理由**:①-③是同一条口径的三个面:检查更新的四分支(最新/有更新/无法判断/失败)各答一个不同的问题,不互相冒充;④复用 #70 基线是 M1 已定方向(#138 的⑤);⑤⑥照既有先例零新设计。**无头测试口径**:真实网络不进无头(任务书明示)——态机测试注入 `UpdateSpawner` 假装配(spawn 真线程验线程机制、零网络,照 settings.rs 模型列表同款),纯函数(比较/解析)直接断言,UI 态机走 shapes/platform_output 取证;真机联 GitHub 留人工。
- **如何改**:①要让缺 tag 也算失败——`run_release_check` 末行把 `Ok(parse_release_tag(&body))` 改回 `parse_release_tag(...).ok_or_else(...)` 并删 `UpdatePhase::Unknown` 的 `tag: None` 分支;②要完整 semver(预发布排序)——引 semver crate 换掉 `compare_release_tag`,单测同步;③要带响应体——照 `settings.rs status_failure` 加 `truncate_chars`;④要横排按钮——把 `settings_row` 调用改 `ui.horizontal`;⑤重绘驱动不用动(收尾清接收端自然停);⑥要收回直接依赖——把 `run_release_check` 搬进 latermd-ai 作公共函数,app 侧删 Cargo.toml 条目,装配点转发。

## #138 #71 M1「帮助」菜单 + 关于对话框五口径:帮助菜单走非命令路线(照「设置」先例)、助记 H/条目 A、关闭三出口(蒙层/X/Esc 全经 `Message::AboutClosed`)、蒙层色值与快捷键蒙层单一事实源、观感复用 #70 设置弹窗基线(2026-10-09,#71 M1 about-dialog·自动拍板)

- **岔路**:①「关于 LaterMD…」是注册 `Command`(进 `Command::ALL`/快速打开/快捷键蒙层/改键页四处注册面)还是照「设置」菜单做非命令直达;②帮助菜单的标题助记字母与条目助记;③关闭语义(既有浮窗有 Esc/蒙层/X 多种组合);④蒙层色值抄一份还是共用;⑤关于窗观感基线与无头取证口径。
- **备选**:①命令注册(好处是进快速打开可搜索)/非命令直达(设置菜单先例);②H(Help 词首)或 A(About)/其他字母;③只窗 X(设置窗现状)/蒙层+X(无 Esc)/蒙层+X+Esc(快捷键蒙层 + 润色浮窗的组合);④复制色值常量/提 pub(crate) 共用;⑤独立观感 token/复用 SETTINGS_* 与 settings_row。
- **自动选择**:①**非命令直达**——帮助菜单单独绘制(`menubar.rs` 设置菜单之后),条目点击发 `Message::AboutOpened`。②标题助记 **H**(Alt 命名空间现有 F/E/O/V/X/A/S 之外 H 空闲;keymap 出厂 Alt 系七条 T/R/F/W/D/C/M 无 H,`title_mnemonics_unique_and_clear_of_keymap_alt_bindings` 审计集已加 H 钉住);条目助记 **A**(About 词首,帮助菜单内唯一,裸字母触发走 `fire_help_item_letter`)。③**蒙层 + X + Esc 三出口全经消息归约**:蒙层点击与窗 X(egui `Window::open` 内建)在 `ui::layout::draw_overlay_dialogs` 汇成一个 bool 返回值翻成 `Message::AboutClosed`,Esc 在 `reduce` 消费裸 Esc(润色确认浮窗同款,排在快捷键蒙层 retain 之前);绘制层不碰状态(铁律)。④蒙层不透明度 `SCRIM_ALPHA_DARK/LIGHT` 提 `pub(crate)`(shortcut_overlay.rs),关于窗引用同一常量——两处蒙层观感永远同步。⑤窗底显式取 `shell_tokens().content` + 两列行走 `settings::settings_row`(标签列定宽 `SETTINGS_LABEL_W`)+ 间距 SPACE_*,窗最小宽自定 `ABOUT_MIN_W=420`(只读速览卡,定位句 + 两列行单行放下);版本 `env!("CARGO_PKG_VERSION")`、定位句取 AGENTS.md §1 首句、仓库链接 `ui.hyperlink_to`(egui 内建 `open_url`,与 vendored 预览外链同路,无头从 `platform_output.commands` 的 `OpenUrl` 取证——live.rs 复制按钮同款)。
- **理由**:①「关于」是打开只读浮窗的会话动作,不是文档命令;进 `Command::ALL` 会连带进快速打开、快捷键蒙层、改键页三处注册面(蒙层/改键页还需无绑定豁免文案),表面积收益为负;设置菜单(直达设置页)是同型先例。②H 是 Help 词首且全局唯一;条目层 A 是菜单内命名空间,与设置菜单的 A(外观)跨菜单允许重复。③三出口是任务书指定语义;全经消息让「开/关」在归约一处收口(M2「检查更新」扩展状态时不会出现两条关闭路径)。④抄常量会出现两处色值漂移。⑤关于窗与设置窗同为模态速览卡,复用基线让同屏出现时观感一致;#70 基线(SETINGS_*/settings_row)就是为此建的。**无头取证口径**:egui 0.36 的 `Window::show` 首帧只注册 Area、次帧才画内容(无头实测 shapes 首帧无文本),关于窗测试一律先跑预热帧再取证;X 按钮是 egui 内建 widget(无公开矩形探针,设置窗先例同样未无头点击),无头覆盖蒙层点击 + Esc + 归约开/关,X 点击留真机人工(与蒙层共用同一返回通道,翻译代码已覆盖)。
- **如何改**:①要把「关于」做成命令——`Command::About` 进 `Command::ALL` + `command_mnemonic` + `MENUS` 增「帮助」表(类型是 Command 数组,可直接进),快速打开/蒙层/改键页自动跟随,届时删除本模块的 `fire_help_item_letter` 与帮助菜单单独绘制。②换助记——改 `HELP_MNEMONIC`/`ABOUT_MNEMONIC` 两常量,审计测试自动跟随(撞了就红)。③要减出口——删 `dialog` 返回通道的对应项或 reduce 的 Esc 分支;要加回车确认——关于窗无可确认动作,不建议。④要独立色值——把 `SCRIM_ALPHA_*` 复制回 about.rs 并在两处注释互指(失去单一事实源)。⑤要改窗宽——`ABOUT_MIN_W` 一个常量;要加内容(M2 检查更新)——`AboutState` 上加在途/结果字段,绘制与归约同文件扩展。

## #135 #72 评审「HEAD 不自包含」的修法:决策条目随分支补交而非删代码引用——M1/M2 两 commit(576b6ae/229bc4f)只含 state.rs+layout.rs,其注释引用的 #133/#134 落在工作区本文件;layout.rs:961-966(可拖翻案)与 :1220-1224(把手自绘)的注释技术结论已自含、编号仅溯源,合入后 main 按仓库惯例「注释引编号+条目同批」可查(先例 layout.rs:507→#122),故收口提交必须把本文件的 #133/#134/#135 与首行指针一并 commit 进 feature/find-bar-theme-drag,否则两处引用在 main 悬空;删引用为备选(会丢「如何改」指南与「把手拖动手感留坤哥真机」留痕,不取)(2026-10-09,#72 评审修复·自动拍板)

- **岔路**:独立评审发现 medium——按当前两个 commit 开 PR 合入,main 上 layout.rs:966(引用 #134)/1223(引用 #133)的决策条目不存在(HEAD 本文件最新条目仍 #132),修法二选一。
- **备选**:①决策条目随分支补交(本文件工作区改动收口时 commit 进分支);②改 crates/ 注释去掉/替换编号引用,让代码不依赖 docs 条目在场。
- **自动选择**:①。已核实工作区 #133(三口径)/#134(六口径+真机手感留坤哥)完整、与两处注释口径逐条对应(见上两条),无缺口需补;crates/ 零改动。
- **理由**:①注释的技术结论本就自含(翻案缘由、自绘理由写在注释正文),编号是溯源指针,合入后条目可查即闭环,与 #31/#103/#113/#122 等数十处既有引用同构;②删引用治标——条目正文里「如何改」五分支指南、egui 0.36 window.rs 实读证据、真机手感留痕仍是未提交状态,评审点「如实记录会丢」的实质未解,且注释失去与决策链的挂钩。
- **如何改**:若收口约束导致 docs 无法随分支提交(与本条前提冲突),退路是②——两处注释去掉「decisions-pending #134」「#133 同款」字样,口径缩写进注释正文,同时本条与 #133/#134 由编排另寻通道落档,不静默丢弃。

## #134 查找/跳转浮卡可拖六口径:把手形态=**顶部全宽窄条(10px)中央自绘抓手胶囊**而非把手 icon(icons.rs 无 Grip/Handle 变体,#133 同款不新增;胶囊纯 shape 不吃字体 glyph 覆盖)、拖动通道=`Sense::click_and_drag` 帧内累计进 `FindBarState/GotoBarState.drag_offset` + `movable(false)` 关掉无标题栏 Window 的 drag-anywhere 静默兜底(egui 0.36 window.rs effective_drag:title_bar(false) 自动回落「拖任意处」且帧内生效,与 fixed_pos 每帧重设相拼会按住边缘闪跳,双拖动通道必关一个)、位置=锚点+偏移(`constrain_to(source_rect.shrink)` 保留钳回,offset=0 时 fixed_pos 输入与改前逐字节相同)、钳制回写=每帧按**帧初**偏移对账(把手 delta 帧内累计、下一帧才进 fixed_pos,按帧末值对账会把刚累计的增量当漂移抹掉——无头探针实证;差 >0.5px 判真实钳制才回写,同时丢弃越界段超出量,贴边拖过头后回拖立即跟手无空程;≤0.5px 取整噪声不动,偏移字段不渗残渣)、持久化=**不做**(会话内保持,关了再开仍在拖后处,重开 app 回锚点;浮卡位置是瞬态工作区状态,进 layout.json 的收益配不上序列化面)、改前「避免它变成可拖对话框」注释口径按坤哥 2026-10-08 新诉求推翻(2026-10-09,#72 M2 find-bar-theme-drag·自动拍板)

- **岔路**:①把手形态(顶部窄条/把手 icon/整卡 drag-anywhere);②拖动消息通道(egui Window 内建 movable/自算把手);③钳制与偏移的对账时机;④偏移持久化与否;⑤「不可拖」旧注释与 M1 口径的翻案记录;⑥把手与输入框/按钮的命中边界。
- **备选**:①顶部窄条/行首把手 icon/整卡可拖;②`Window::movable`(标题栏或 drag-anywhere)/`Sense::click_and_drag` 自累计;③不回写(保留空程)/每帧无条件回写(渗入取整残渣)/按帧初偏移阈值回写;④随 layout.json 持久化/会话内保持;⑤维持禁拖注释/翻案并同步改注释;⑥把手占独立行(结构不相交)/与输入框同行。
- **自动选择**:①**顶部窄条+自绘胶囊**(`overlay_drag_handle`:全宽 10px 条、中央 28×3 圆角胶囊、hover/拖动取强色,Grab 光标)——卡宽 ~416px 高 ~58px,全宽条的抓取面远大于行首 icon,且不依赖 icons.rs 新变体与字体 glyph;②**自累计 + movable(false)**;③**帧初对账 + 0.5px 阈值**(`overlay_drag_sync`);④**不持久化**;⑤**翻案**:draw_find_overlay/draw_goto_overlay 注释改为「位置=锚点+偏移,可拖」,与 #133 的浮卡口径链一致;⑥**把手独立成行**在两行内容之上,与 TextEdit/按钮命中区零相交,非可聚焦控件不进 Tab 焦点链(查找框↔替换框↔编辑器焦点链零改动)。
- **理由**:①egui 0.36 实读(window.rs:570-581):无标题栏的 movable Window 静默回落 `WindowDrag::Anywhere`,整卡 body 都是 DRAG 命中区,与本任务「把手区与 TextEdit/按钮命中不冲突」直接矛盾,整卡方案出局;icon 方案要么扩 icons 枚举(与 #133「不为两枚箭头扩枚举面」同款纪律)要么赌字体 glyph 三平台覆盖。②把手的 drag_delta 帧内累计、fixed_pos 次帧生效是立即模式的标准一帧滞后(egui 自家标题栏拖动同款),60fps 下不可感知;保留两个拖动通道则按住卡边缘会闪跳。③钳制发生在 Window 内部、取整(round_to_pixels+round_ui)每帧都在,不回写则贴边越界段成了回拖空程,无条件回写则 (a) 无拖动时把 ≤0.5px 取整差渗进偏移字段破坏「offset=0 逐字节一致」、(b) 帧末对账抹掉当帧增量(探针实证:纯上拖 −40px 后 offset 只剩 (−0.4,0.4) 取整残渣、卡片纹丝不动)。④两卡都是打字间隙的瞬态工具,「记得上次拖到哪」跨会话无明确收益,而 layout.json 已有结构化 schema,塞两个 Vec2 要动读写两侧。⑤禁拖是 2026-09 查找条初版的设计决定,坤哥新诉求(能拖动位置)直接命中该条,推翻并在代码注释与本条留痕,不静默改。
- **如何改**:①要行首把手——把 `overlay_drag_handle` 从 `find_bar_contents`/`goto_bar_contents` 顶部挪到首行行首,改窄条的 `allocate_exact_size` 为固定小尺寸,测试 `overlay_drag_pill` 的尺寸签名断言同步;②要整卡可拖——去掉两处 `.movable(false)`,并放弃 `fixed_pos` 每帧重设、改为从 Area 记忆的 pivot_pos 回读用户拖动(fixed_pos 会每帧吃掉内建拖动,两者只能留一个,工程量另评);③要空程换简单——删 `overlay_drag_sync` 调用,`overlay_drag_clamps_inside_source_area_and_returns_without_dead_zone` 的「贴缘偏移归零/回拖立即跟手」两断言会红,按新口径改断言;④要持久化——`drag_offset` 并进 `LayoutSettings`(layout.json)读写两侧,重开恢复,`find_and_goto_cards_drag_by_handle_and_hold_offset_across_frames` 的「重开 app 回锚点」边界同步改;⑤把手高度/胶囊尺寸——`OVERLAY_DRAG_STRIP_H` 与 `overlay_drag_handle` 内两个常数,胶囊 28×3 尺寸签名被测试 `overlay_drag_pill` 钉住,同改。已知边界:非整数像素锚点下(如 ppp 缩放)offset 字段保留浮点累计值,渲染位置仍走 egui 整像素取整;把手拖动手感(阈值/跟手性)留坤哥真机。

## #133 查找/跳转浮卡主题自适应三口径:frame 取 `Frame::popup(&ctx.global_style())` 随当前生效主题(两卡共用 `overlay_popup_frame` 单一真源)、标签列对齐用「两词最大自然宽」的固定列宽补位(改前 delta=0 恰是两词等宽的巧合非设计保证)、↑↓ 按任务书口径保留文本(icons.rs 无 ChevronUp/Down,不新增变体)(2026-10-09,#72 M1 find-bar-theme-drag·自动拍板)

- **岔路**:①浮卡 frame 怎么随主题——沿用写死 `style_of(Theme::Dark)` 还是随当前生效主题;随主题的话用哪个 API(egui 0.36 无 `ctx.style()`);②「替换行标签与查找行标签列对齐」的实现法;③命中计数着色分档;④↑↓/关闭按钮 icon 化的边界。
- **备选**:①`ctx.style_of(ctx.theme())` / `ctx.global_style()` / 维持写死;②靠两词等宽的现状不管(已对齐)/固定列宽补位/Grid;③统一弱色(改前现状)/有结果弱色+无结果警示色;④新增 ChevronUp/Down 自绘变体/保留文本按钮。
- **自动选择**:①**`Frame::popup(&ctx.global_style())`**,两卡(draw_find_overlay/draw_goto_overlay)共用新函数 `overlay_popup_frame(ctx)`——0.36.2 里 `global_style()` 就是 `Window` 渲染自取的那套活动 style(window.rs:628 同源),由 `ThemeSettings::apply` 按解析后明暗切换;`style_of(ctx.theme())` 等价但多一次 theme 读取,弃。②**固定列宽**:`bar_label_column_width` 取「查找/替换」两词在同一 Body 字体下的最大自然宽,行内标签自然宽渲染后补位到该宽——2026-10-09 无头实测改前 delta 恰为 0.0,那是两词都是二汉字恰好等宽的巧合,字体偏好/回退链一变就散,结构化后不依赖巧合。③**分色**:有结果 `weak_text_color`(与改前同观感),无结果 `tokens::WARN`(与设置页告警同色);文字语义(「无结果」/「N/M」)零变化,只动颜色——空 query 时也会显示警示色「无结果」沿既有文字语义,观感是否需要空 query 不显示留真机。④**↑↓ 保留文本**:icons.rs 现有变体无 ChevronUp/ChevronDown(任务书明示「无则保留文本记 notes」),不为两枚箭头扩枚举面;关闭按钮两卡早已用 `Icon::Close` 矢量叉,不动。
- **理由**:①`global_style()` 是 0.36 语义上「后续浮窗实际使用的那套 style」的官方入口,与 Window 内部取法逐字同源,无第二解释;单一真源函数让「两卡观感分叉」在结构上不可能。②补位法复用 `ui.label` 的 egui 原生绘制路径(不手绘 galley),每帧两次 `layout_no_wrap` 且 fonts 层有缓存,代价可忽略。③警示色的信息量在「查了但没有」这一刻,有结果时计数退到弱色不打扰,与设置页 WARN 同色保持「警示=同一种黄」的全局一致性。④自绘体系的设计成本(坐标+测试+枚举文档)对两枚箭头不划算,Unicode ↑↓(U+2191/2193)在本仓三平台目标字体覆盖内风险可控,记 notes 留待真机观感复核。
- **如何改**:①想换 API 或观感——只动 `overlay_popup_frame` 一个函数;想回写死主题即重引入浅色黑窗,像素测试 `find_and_goto_cards_paint_effective_theme_popup_fill` 当场红(咬合力已实证:临时还原写死 Dark,浅色探针取到 #292A2D≠#FFFFFF)。②想改列宽口径——动 `bar_label_column_width`/`bar_label` 两函数,`find_and_replace_input_columns_align` 钉住输入框左缘 <0.5px。③想调整色档——动 `hit_count_label`,`hit_count_warns_without_hits_and_weakens_with_hits` 钉住无命中 WARN 在场/有命中 WARN 缺席。④要真 chevron——按 icons.rs「加枚举变体+draw 分支+every_icon_renders 补项」的既有流程加,顺手把本条④作废。

## #137 #70 M2 五分页配置行两列对齐(原 #134,跨分支撞号收口重编号):**弃 Grid 改定宽标签列 helper**(148px 进 ui::tokens 跨页钉死控件列起点)、快捷键页键位按钮等宽三段、列表/多行/动作行不进两列的边界口径、Grid id 退役与 ComboBox id 显式化(2026-10-09,#70 M2 settings-align-polish·自动拍板)

- **岔路**:统一两列行用 Grid(推广 keymap/bed 两处先例)还是等价方案;行内多段控件(键位/清除/重置)要不要表格化;哪些内容算「配置行」进两列;定宽怎么在 egui 里真正落地。
- **已做**:①行方案 = `settings_row(ui, 标签, 控件闭包)` helper(settings.rs,`pub(crate)` 供 ai_key::key_editor 复用):标签列定宽 [`SETTINGS_LABEL_W`]=148(容最长标签「上下文大小(KB)」≈138px)+ 零高占位补齐,控件列起点 = 内容区左缘+148+列间隙,**跨五分页同一 x**。弃 Grid 的理由:Grid 列宽随内容自适应,五页五张表只能页内对齐、跨页漂移;多行控件(图床请求头 3 行)会把同行标签顶到格顶。keymap/bed 两处 Grid 语义等价迁移进 helper 后退役,`settings-keymap-grid`/`settings-bed-grid` 两个 id 随之消失(grep 确认无外部引用);所有 `ComboBox::from_label`(按标签文本派生 id)改 `from_id_salt` 显式 id(settings-skin-select / settings-ai-provider / settings-ai-models-known)。②egui 实测坑(实读 ui.rs `scope_dyn`):`allocate_ui_*` 家族推进父 cursor 用的是内容 **min_rect 而非 desired 宽**,「分配 148 宽子 Ui」会退化成标签实际宽 → helper 改为量出标签宽后 `allocate_exact_size` 补零高占位,列宽钉死。③快捷键页:键位按钮 `min_size` = [`SETTINGS_KEY_W`]=140,37 行的键位/清除/重置排成等宽三段(原 Grid 的列对齐观感,否则按钮随 "Ctrl+Shift+S" 一类长文字伸缩、右侧两列呈锯齿);捕获态文案缩短为「按下新键位…」(「Esc 取消」已在页首说明行,信息不丢)。④边界口径(不进两列的内容):MCP 工具权限 5 个 checkbox、MCP 调用计数、图床 profile 列表行(名称用户自定义宽度不定)、动作行(保存/放弃修改/获取模型列表/全部恢复默认/图床编辑器保存取消)——列表与动作保持整行;图床 headers 多行编辑区**进**两列(标签「请求头」垂直居中于多行框旁,horizontal 交叉居中自然落位)。⑤整句区块标签拆分:标签列短语 + 行下 weak 说明(主题「外壳与正文、代码块同帧联动」/密度)或控件 hover(皮肤「正文与代码高亮样式」);checkbox 开关行名称进标签列(Minimap/打字机模式/专注模式)、描述留 checkbox 文本;字号/行距滑杆去掉 `.text()` 尾标(标签列已表意),hover 补「作用于编辑器与预览」语境;只读信息行(渲染后端/状态/检索范围)也进两列。
- **如何改**:嫌标签列宽/键位按钮宽不合适只改 `SETTINGS_LABEL_W`/`SETTINGS_KEY_W` 两常量;要恢复滑杆全宽,在控件闭包里给 Slider 显式 `desired_width`;要把工具权限也排两列,给 `ToolKind::name()` 起中文显示名后按 checkbox 开关行同款进 helper。已知取舍:Slider 在 horizontal 内宽 = `style.slider_width`(约 100px,非旧版全宽),观感留真机;任务书提及「标题宽」经查不在设置页(`tab_title_width` 由标签条右键菜单 #37 设置),M2 按「对齐既有配置行」范围不新增设置项。
- **验证**:`every_tab_rows_align_two_columns` 四层断言全绿:每页全部行标签文本 x 全等(0.25px 容差)且跨五页同一起点;同类控件(三态首选项/复选框文字/输入框 hint)文本 x 成列一致;同行「标签 ↔ 输入框内文本」y 差 ≤2px(同基线,实测 0.87px);快捷键页 37 枚等宽键位按钮**矩形左缘**一致(键位文本在按钮内水平居中是 egui 按钮排版,列对齐按 rect 口径断言)。控件齐全性:五页逐项断言在场 + 既有 every_tab_renders / AI kept-fields / 滑杆 / 禅定 / minimap / ai_key 按钮消息全链路测试全部通过,消息归约(草稿/保存/捕获/notice)零改动。

## #136 #70 M1 设置弹窗观感基线五口径(原 #133,跨分支撞号收口重编号):常量落 ui::tokens 单一真源且**只改观感不动骨架**、中央区 pad 8→12 与按钮条水平基准线对齐、标题栏维持 egui chrome 不单独配色、窗底显式取当帧 shell.content(非写死一档)、顺手把 find/goto 浮层写死的 `style_of(Theme::Dark)` 改随当前主题(该「顺手修」在合入形态中由 #72 的 overlay_popup_frame 单一真源覆盖同一处,殊途同归)(2026-10-09,#70 M1 settings-align-polish·自动拍板)

- **岔路**:观感常量放哪、改多少;「整窗随主题」是依赖既有投影还是显式取色;顺手盘点中发现的两处写死 Dark 修不修。
- **已做**:①骨架值(footer 48 / tabs 112 / 默认 600×440)原样进 `ui::tokens::SETTINGS_*`,分毫不动——exact_size 三段定形 + `auto_shrink([false,false])` 是 2026-09-29 实测的防尺寸反馈环防线,观感调整无权触碰;唯一数值变化 = 中央区 pad 8→12,动机是与底部按钮条水平内边距(12)对齐同一条纵向基准线(分隔线/正文首行/「关闭」按钮同起点),分页列保持 8(窄列+图标行密度高,放到 12 显空转)。②标题栏不单独配色:活动窗标题填充硬绑 `widgets.open.weak_bg_fill`(egui title_ui 源码),改它牵连全部浮窗,超 M1 面;token 注释里写明它是 egui chrome。③窗底 `.frame(Frame::window(style).fill(shell.content))`:生产路径 `theme.apply` 每帧已把 `window_fill` 投成同值,显式化是让「不依赖投影也在场」成为本窗自身性质,明暗两档各走各的 shell_tokens。④find/goto 浮层(layout.rs)`Frame::popup(&ctx.style_of(Theme::Dark))` 是 #17 时代的遗留捷径,浅色主题下查找/跳转浮条仍是暗底;改 `style_of(ctx.theme())` 两行,同通道低风险顺手对齐。⑤设置窗补挂显式 id `settings-dialog`(find/goto 浮层同款):egui 0.36 按标题文本派生 Area id(且 `Atoms::text()` 返回 `Option<Cow>`,id 是对 Option 的 hash,`Id::new("设置")` 取不到),标题一改拖动位置记忆就丢;显式 id 解耦,无头像素测试也按名取 Area 矩形。
- **如何改**:嫌中央区 12 太宽/太窄只改 `SETTINGS_BODY_PAD` 一处;想要标题栏也吃 sidebar 色组成「chrome 带」,需把活动窗标题填充从 `widgets.open.weak_bg_fill` 摘出来(app 侧自定义 title_frame 会被 active 分支覆盖,egui 0.36 无公开开关),属后续模块;四浮层(quick_open/emoji_panel/image_dialog 走默认 Window 框架、shortcut_overlay 显式随 visuals)盘点结论均随主题,无需改动。
- **验证**:`settings_shell_follows_light_and_dark_themes` 明暗两轮像素采样(分页列露 sidebar 色/按钮条与中央区多数点露 content 色/两主题采样色互异),变异双向——写死暗色后浅色轮当场红(#292A2D vs #FFFFFF),分页列底色改 content 后红(#FFFFFF vs #EDEFF2),恢复后绿;观感细节(呼吸感/分隔线粗细)留真机人工。

## #132 #68「小窗可拖/全屏不可拖」排查记档:**无头两层(editor 层+完整 UI 层)两档视口(900×600/1920×1008)拖动全部复现失败(全绿)**,main 代码未找到尺寸相关缺陷;真机报告最大嫌疑=实例来源——LaterMD-tb 不含 #68(PR#147 在其分叉之后)且其工作区带未提交的 sidebar/tokens 布局半成品,行为不可信;排查中 :0 上的 X11 注入实验因窗口未激活全部注入到了用户桌面(指针冻结于 (990,588) 实锤),实验数据作废,后续真机实验须 Xvfb 或用户亲自验证(2026-10-08,#68 后续·自动拍板)

- **岔路**:真机「全屏不可拖」是 main 代码缺陷还是环境/实例因素。
- **已做**:editor 层 frame_sized 参数化+1920×1008 拖动(绿);layout 层完整 UI(侧栏+编辑器+预览)两档视口拖影断言(绿,fix/minimap-fullscreen-regression 分支入库);debug 诊断实例干净 1920 启动下 minimap_rect/hit_rect 值正常([1249,1357])。
- **如何改/下一步**:用户在主仓 main 全新编译验证(小窗一次+最大化一次);若 main 实例仍全屏不可拖,则聚焦真机特有因素(WM 全屏路径 F11 vs 最大化、多屏、DPI),届时装 Xvfb 隔离复现,禁止再在用户活动的 :0 上注入。

## #131 #69 Alt 误触修复方案:**`Event::WindowFocused(false)` 帧缴械 armed**(首选,最小面)而非「release 触发要求本窗口见过 Alt press 且期间未失焦」的会话完整性判定——egui 0.36.2 `Event::WindowFocused(bool)` 可达(egui-winit lib.rs:444 从 winit `WindowEvent::Focused` 推送,失焦时上游还顺带清 modifiers,同方向防御),失焦帧必然早于下一次聚焦,单 match 分支三行即切断「armed 跨会话遗留 + 孤立 Alt release 误判单击」链;会话完整性判定要多存一份「press 是否本窗口见过」的状态,防的路径完全被失焦清零覆盖(2026-10-08,#69 alt-focus-guard·自动拍板)

- **岔路**:修 `single_alt_click` 的 armed 遗留,用失焦清零还是会话完整性判定。
- **备选**:①失焦清零(本次所选)——`Event::WindowFocused(false) => armed = false` 一个 match 分支;②会话完整性——armed 拆成「本窗口 press 见过」+「期间未失焦」两个条件,多一份状态且与①防的路径完全重叠;③干脆去掉单击 Alt 聚焦功能——损失 Win/Linux 惯例,否决。
- **如何改**:若真机(Win/mac)出现 egui-winit 不推 WindowFocused 的平台差异导致修复失效,退路是②,实现点仍在 `single_alt_click`。
- **验证**:无头红绿双向——变异(去缴械分支)后 `alt_release_after_refocus_does_not_fire_stale_armed` 当场 FAILED(panicked 于首断言),恢复后绿;正常单击 Alt 聚焦(`single_alt_click_opens_first_menu`)与 Alt+字母/组合键污染路径零回归;Deepin Ctrl+Alt+方向真机实测留坤哥(无头测不了真 WM 时序)。

## #130 #68 拖阴影的四处口径:grab 形态=**给 `JumpInput` 加 `grab: Option<f32>` 字段而非新函数**(与居中路径共单入口、`travel`/`ve` 的 `None` 判定只写一处,「`grab=None` 逐字节等价旧行为」的锚点有直接对应物;代价是 M1 阶段 editor.rs 消费点一行编译伴生 `grab: None`)、press 帧信号=`is_pointer_button_down_on` **叠加**当帧 `pointer.primary_pressed()`(egui 0.36 `click_and_drag` 的 `dragged()` 要指针移过点击阈值才为真,按下帧无现成单次信号)、grab 通道=**独立 temp `minimap-grab`**(`Option<f32>`,press 帧判定写入、拖动序列随读不删、`drag_stopped`|`clicked` 清零)**不并入 jump temp**(两者生命周期不同:意图消费即清 vs 手势期常驻)、落地=**land 绝对化**(`scroll_to_rect` 的 TOP 是相对换算,rect 顶须写成 `clip.top + (target×travel − metrics.offset)`;旧的 `clip.top + target×travel` 在非零 offset 起手时每次调用再累加一遍行程)(2026-10-08,#68 minimap-drag-viewport M1+M2·自动拍板)

- **岔路**:①grab 语义怎么进纯函数层(加字段 / 新函数);②何时、如何识别「按下这一帧」;③抓取偏移存在哪(temp 通道形态);④多次/非零 offset 起手时如何落到绝对目标位置。
- **备选**:①**新函数**(如 `grab_ratio`)——调用方各自选函数,`travel<=0`/`ve>=1` 的 `None` 判定要在两处重复,「`grab=None` 逐字节等价旧行为」失去同一代码的对照物 / 加字段(本次所选) / 用 `Option<JumpInput>` 之类的 wrapper 类型。
②只用 `dragged()`(按下帧为假,抓取判定要晚一帧且依赖指针移动) / 只用 `clicked()`(纯点击才真,拖动永远抓不到) / 把判定推迟到首次 `dragged` 帧用按下点坐标(需自己缓存 press 坐标,多一份状态) / 叠加当帧 press 事件(本次所选)。
③**并入 jump temp**(把 y 与偏移压成一个 struct)——但两者生命周期不同:jump 意图「消费即清」、grab 须在整段手势内常驻,合一要么让手势期每帧重写、要么让 release 帧的清理语义不清 / 独立 temp(本次所选) / 存进 `State`(跨越了纯 UI 手势的边界,还要处理标签切换导致的 per-editor 隔离)。
④保持 `clip.top + target×travel` 旧式(接受它在非零 offset 起手时每次调用多走一份行程) / 在调用方先减掉当帧 offset / 改用 `Align::Center` 之类不同 API (改的不是「差多少」而是「anchor 是谁」,与 minimap 的「rect 顶表位置」契约不符) / 在 rect 顶扣当帧 offset(本次所选)。
- **自动选了**:见首行四处口径。落地(均在 `crates/latermd-app/src/ui/`,vendor/ 零改动、零新增依赖):`JumpInput.grab` + `jump_ratio` 的 grab 臂 `target = (pointer_y - grab) / (窄条高 - ve·full)`(minimap.rs,滑轨即 `viewport_highlight` 的 top 系数,故「目标阴影 top 对准视口顶」与「抓取点相对阴影不动」互为对偶、可由同函数回代互证);press 帧判定与 grab 通道读写、`land` 绝对化(editor.rs),实现注释里已把 egui 0.36 两条硬事实写明:`scroll_to_rect` TOP 的相对性、`click_and_drag` 的 `dragged()` 需移过 ~6px 阈值。测试面:M1 四测(`grab_drag_keeps_grab_point_fixed_on_highlight`、`grab_none_equals_legacy_centered_target_byte_for_byte`、`grab_edges_reach_document_endpoints_and_clamp`、`grab_keeps_none_precedence_on_short_or_degenerate_inputs`)+ M2 三测(`dragging_highlight_body_pins_pointer_to_highlight`、`dragging_outside_highlight_keeps_centered_target`、`release_then_press_re_judges_grab_state`)+ land 回归锁 `repeated_click_from_nonzero_offset_does_not_accumulate`。**红绿验证**:grab 公式污染 → `grab_drag` 红;`None` 公式污染(0.5→0.6)→ 逐字节等价测试红;land 改回旧式 → 回归锁在第二次断言处红(期望 ≈1983.9、实测 3184.9);恢复后均绿(该三项为模块自验结论,由本文档收尾者转述,未在本轮重跑污染版)。**本轮实跑**:`cargo fmt --all --check` 过、`cargo clippy --workspace --all-targets -- -D warnings` 零告警、`cargo clippy -p latermd-app --all-targets --all-features -- -D warnings` 零告警、`cargo clippy --workspace --all-targets --no-default-features -- -D warnings` 零告警、`cargo test --workspace --all-features` 全绿、`cargo doc --no-deps --all-features` 零 warning(实现阶段自述未跑的 doc 与 `--no-default-features` 两轮,已于本轮补齐并确认通过)。
- **为什么**:①加字段让「现状路径」与「grab 路径」在同一个 `match` 里相邻,`travel<=0`/`ve>=1` 的 `None` 判定只有一处,「`grab=None` 逐字节等价旧行为」才有可断言的同一份代码做对照物;新函数会把「两份 `None` 口径是否还一致」变成靠人记。②按下帧是唯一能读到「用户按下时看到的那帧阴影」的时机,而 egui 0.36 对 `click_and_drag` 没有单次 press response —— `is_pointer_button_down_on` 从 press 帧起为真,再叠当帧 `primary_pressed()` 即精确抓一次,比自己缓存 press 坐标便宜。③两个通道生命周期不同(jump 意图「消费即清」、grab 须在整段手势内常驻),硬合一会把「每帧重写」与「release 帧残留误读」两个风险同时背上;独立 temp 的代价只是多一个 `with("minimap-grab")` id。④`land` 的旧式不是观感问题而是**可累加的漂移**:同一 target 连落两帧实测 offset 1000→2000,必须先修再谈拖影。
- **如何改**:①要退回「只有抓住框才能拖、其余一动不动」——删 press 帧的框内判定即可(grab 恒 `None` 即现状);要换成「按下后先拖过阈值才开始跟随」的语义——把 grab 的写入时机从 press 帧挪到首个 `dragged()` 帧并自行缓存 press 坐标。②grab 通道想合进 jump temp——改 `minimap::grab_id` / `jump_id` 两处的读写即可,但须同步改 `drag_stopped`/`clicked` 的清理时机。③press 帧信号想换实现——egui 后续若提供 press response,可整体替换掉叠加写法,改点只在 editor.rs 一处。④`land` 若改用别的 anchor(`Align::Center`/`BOTTOM`)——`repeated_click_from_nonzero_offset_does_not_accumulate` 的期望算法(与生产同源的 `jump_ratio` 独立算)要同步替换,否则容差失败会误导成「加加减减」而不是 anchor 改了。

## #129 #67 M2 minimap 视图开关的四处口径:消息=**新增无参翻转 `Message::ToggleMinimap`** 而非复用带值 `ShowMinimapToggled(bool)`(与 `ToggleTypewriter` 同构——菜单/快捷键是翻转语义,现值在 State、`Command::message()` 无 state 访问拿不到,带值消息留给设置外观页复选框专用;归约同一字段 `theme.show_minimap` 同一 `persist_theme` 通路,**未造第二份状态**)、菜单位置=「视图」第一段 FocusModeToggle 之后、ToggleTheme 之前(编辑区呈现方式开关聚组:Live/打字机/专注/minimap 连排,主题互换收段尾;禅定仍独立第二段)、勾选态口径=**布偶开关才加 ✓**:**2026-10-15 后为六条**(侧边栏/切换视图/打字机/专注/minimap/禅定 —— 原「预览栏」一条随 `ToggleRightPreview` 删除,「Live」改由「切换视图」一条表达且勾选判据是 `render_mode == Live`,分栏档下不勾选)从 `State` 单一事实源取真值(`toggle_checked`,菜单/设置页/标题栏钮同源),开=`✓ ` 前缀、关=两半角空格占位(开关条目间文字主体对齐;✓ 与空格字宽随字体略出入,观感留真机),「切换主题」是明暗互换非开/关(哪个算「开」无自然答案)不带勾选列,非开关命令一律无前缀;快捷键=Ctrl/Cmd+Alt+M(M=Minimap 词首,Ctrl/Cmd+Alt 层既有 C/W/D 之外 M 空闲,菜单标题助记集 F/E/O/V/X/A/S 不含 M,#121/#122/#127 同款避让;`conflict` 断言+真按键触发+标题审计三项实测绿)(2026-10-08,#67 menu-effect-audit M2·自动拍板)

- **岔路**:①翻转消息形态(新无参消息 vs 复用带值消息 vs 菜单层闭包读现值发带值消息);②菜单位置;③勾选态覆盖面(全部 Toggle 名命令 vs 仅布偶开关 vs 不加);④快捷键字母。
- **备选**:①复用 `ShowMinimapToggled(bool)`(菜单绘制时从 state 读现值取反传入——菜单层就得读 State 且消息语义从「翻转」漂移成「设值」);②挂「编辑」或设置菜单(不对——group 归 View,`menu_placement_matches_command_group` 会红);③全部 Toggle* 命令加勾选(「切换主题」无 bool 语义,硬造勾选态误导);④Ctrl/Cmd+Alt+M 之外候选(纯 Alt+M 与标题助记层同命名空间,被审计否决;无 Alt 的 Ctrl+M 独占 M 键且偏离本组 Alt 层惯例)。
- **自动选了**:见首行四处口径。落地:`command.rs` 全套注册 + `minimap_shortcut_is_ctrl_alt_m_and_conflict_free`;`state.rs` `ToggleMinimap` 归约 + `minimap_menu_toggle_flips_field_and_persists`;`menubar.rs` VIEW_MENU 条目 + `toggle_checked`/`check_prefix` + `view_menu_toggle_items_show_check_state`/`minimap_menu_item_click_flips_theme_and_persists`;守门红绿三验(删菜单入口→覆盖断言红、stub ✓→勾选态断言红、改默认键→键位断言红,恢复后均绿)。
- **为什么**:①无参翻转与 #64 打字机/专注完全同构,带值/翻转双消息分立是既有先例,菜单层闭包读 state 会让消息语义依赖调用方;②菜单归属与 `Command::group()` 的一致性是守门测试钉住的现状,View 组只能挂「视图」;③「显示当前状态」只对布尔语义成立,主题互换加 ✓ 是制造假信息;占位空格是廉价的对齐手段,精确勾选列(固定列宽绘制)留给观感不足时的后续。
- **如何改**:①想让菜单也发设值消息——改 `Command::ToggleMinimap.message()` 即可,归约双消息并存;②勾选列观感不满意——把 `check_prefix` 的字符串前缀换成固定列宽自绘,`view_menu_toggle_items_show_check_state` 断言面跟着换;③键位想换——`default_shortcut` + `minimap_shortcut…` 测试 + menu-coverage §5 清点三处同步,老档迁移照 `keymap::load_from` 既有模式。

## #128 #67 M1 菜单链路核查的三处口径:核查结论=main 菜单点击链路无断链(实测 menubar 16 项+command 映射+task/InsertToc/focus 归约测试全绿,menubar outbox 由 `LaterMdApp::reduce` layout.rs:94 全量 `state.apply`,零生产代码改动,只补两处测试面缺口——`ToggleTypewriter` 无参翻转归约断言(带值消息既有测试只测 `TypewriterToggled(bool)`,菜单/键位路径用的无参翻转原本无 apply 断言,与 `ToggleFocusMode` 的既有断面对称补齐)、「设置」菜单直达条目真实点击路径(设置菜单不在 MENUS 表、`clicking_every_menu_item…` 不遍历它,齿轮路径的归约断言在 layout.rs 但 Keymap 直达页无任何断言;新增 Alt+S 开菜单→展开帧定位条目→指针点击→`SettingsOpened(tab)`→`State::apply` 落页签的完整断言,两处均红绿验证)、egui 0.36 MenuButton 展开时序事实(事件帧 `open_id` 写记忆→次帧菜单按钮收到开态→**再次帧**闭包才画出条目,无头测试须两次空帧后取 shapes 定位,实测 frame2 无条目 frame3 才有)、版本滞后口径=**实证最新发版为 v0.0.4(2026-10-04,tag 9f20d83/PR#102),非主会话背景记录的 v0.0.3**;`merge-base --is-ancestor` 逐条核验 #62 菜单栏整体优化(PR#122,2026-10-07)与 #63-#66(PR#124/#125/#126/#127,2026-10-07~08)**全部不在 v0.0.4 内**,其后 main 积压 26 个 PR/95 个 commit——「菜单没生效」按版本滞后解释成立且更精确(v0.0.3→v0.0.4 只是把滞后窗口收窄 4 天);发版建议=bump **v0.0.5**(任务书原文写「v0.0.4 发版建议」,以现状 v0.0.4 已于 2026-10-04 发布为准修正;workspace version 现为 0.0.4,下版必是 0.0.5),由坤哥拍板后走 bump PR+CHANGELOG 小节,本模块不擅自发版)(2026-10-08,#67 menu-effect-audit M1·自动拍板)

- **岔路**:①发现缺口时怎么处置(补测试 vs 视为断链去修代码);②设置菜单条目无 MENUS 覆盖,断言走哪条路(真实指针点击 vs 只测消息构造);③版本滞后的事实口径(沿用主会话「最新 v0.0.3」 vs 以仓库 tag/CHANGELOG 实证为准)。
- **备选**:①判 断链→改生产代码(但归约分支、消息映射、item() 点击三段代码逐行核验均在,红绿验证时 stub 掉归约测试立即红——链路是通的,不该动);②只写 `SettingsOpened(tab)` 构造消息的归约测试(更小,但放弃「真实点击路径抽查」的要求,popup 展开帧→闭包绘制→按钮 clicked 这段 UI 链就无人守);③沿用 v0.0.3 口径写 notes(与仓库现状矛盾,v0.0.4 tag 与 CHANGELOG 小节俱在)。
- **自动选了**:见首行三处口径。落地:`state.rs` 既有打字机测试内补 `ToggleTypewriter` 无参翻转段;`menubar.rs` 新增 `settings_menu_direct_tabs_click_through_and_reduce`(外观/快捷键两页);均 stub 注入断链验证过红、恢复后绿。
- **为什么**:①「菜单没生效」的可信核查必须先证明链路通,再解释为何用户看不到——直接改代码等于预设了断链结论,违背任务书「取证为主」;②点击路径抽查的价值恰在 popup 这段 MENUS 覆盖矩阵到不了的 UI 面(设置菜单独立绘制),条目文本 shapes 定位是 layout.rs `topmost_text` 同款既有手法;③发版建议写错版本号会直接误导 bump PR。
- **如何改**:①若坤哥复核发现某真机菜单仍无响应,优先查安装版本号(关于/设置),不是代码;②要收紧 MenuButton 展开时序的依赖——egui 升级后新测试的两次空帧若 flaky,改为循环空帧直到目标文本出现(带上限);③发版拍板后:v0.0.5 bump PR 照 CHANGELOG 头部流程(version+小节同 PR,合入后 auto-tag 发布),小节需覆盖 v0.0.4 以来的 26 个 PR。


## #125 #65 M2 修复:HTML 导出高亮的扫描单元=相邻 Text 事件合并串(独立评审发现 `==a&amp;b==` 预览高亮而导出漏标:pulldown 把 `&amp;` 解码成独立 Text("&") 事件,一对 `==` 被拆进三条 Text,逐条扫描漏标且 `==` 原样回流正文;修法=wrap_highlights 加相邻 Text 缓冲,任何非 Text 事件即 flush,push_marked_text 改收 `Vec<CowStr>` 合并整串再扫 highlight_spans;单段无配对仍走零拷贝快路径;软换行/行内代码/脚注引用等非 Text 事件打断合并,不跨构造拼接——跨行 guard 与豁免深度门控不受影响)(2026-10-08,#65 highlight-syntax 评审修复·自动拍板)

- **岔路**:导出端高亮扫描单元——单条 Event::Text(原实现) / 相邻 Text 合并串 / 全文 span 经 offset 映射进事件流。
- **备选**:①逐条扫描(实体输入必漏,实测失败形态 `<p>==a&amp;b== 与 <mark>真高亮</mark></p>`) / ②合并相邻 Text(实体只产生连续 Text 事件序列,合并即覆盖;配对口径回到与预览整串扫描同一把尺) / ③offset 映射(#124 备选④同因否决:转义/实体解码后事件内容与源码字节错位,映射脆弱)。
- **自动选了**:②。落地:`wrap_highlights` 的 `pending` 缓冲 + `drain_pending` + `push_marked_text(Vec<CowStr>)`(latermd-export/src/lib.rs),新测试 `highlight_survives_entity_split_text_events` 先红后绿。
- **为什么**:合并串是配对口径的最小载体——highlight_spans 的 flanking/最左配对语义定义在整串上,拆到事件粒度就没有「配对」可言;合并到非 Text 即止保住既有否决线(跨行不算、豁免构造不扫、行内代码原子),43 项既有测试零回归实测。
- **如何改**:放宽合并面(允许跨 InlineHtml/InlineMath)——pending 接收分支增事件种类即可,但 InlineHtml 拼进合并串会把活标签转义成死文本,须连带改输出形态;数字实体残余:`&#61;&#61;x&#61;&#61;` 解码出 == 字面,导出合并串照常配对,而预览改写层扫源码副本无 == 字面不标——底色观感分歧的已知残余(不回流、不丢字,无害),要消除需预览侧改写层也吃解码后文本,属 #123 通道形态的独立工程,本条不预支。

## #124 #65 M2 高亮渲染与导出的六处口径:通道=`layout_link` 非 widget 分支接管(实测对 `hl://` 普通链接以超链接色调 `layout_link`(layout.rs:173),返回 true 即整段接管,`TextFormat::background` 走 egui 标准字段,epaint `add_row_backgrounds` 按 glyph logical_rect 落底色矩形——零 vendor,①类纪律全程未触发)、底色=一条推导式出明暗两档(30% 荧光黄 `(255,230,0)` × 70% 主题 `extreme_bg_color`,逐通道 u8 四舍五入;生产输入=出厂皮肤投影——`apply_shell_to` 把 extreme_bg 覆盖为 code_bg token,暗 `#232427`→`#655E1B`、明 `#F5F6F7`→`#F8F1AD`;对出厂正文色 `#E8EAED`/`#1F2329` 的 WCAG 对比度 5.50/13.66,均 ≥ AA 正文线 4.5:1;不做用户可调,#81/#122 同款规格;锚黄与比例是仅有常量,无双份硬编码十六进制)、排版段=vendored `line_height_for` 同式复制(私有函数 app 侧抄公式,含 0.75px 行高吸附补偿;`valign=Bottom` 与正文段同侧;由「高亮段行高 == 相邻正文段行高」探针钉住,公式漂移当场红)、HTML 导出=原生 `<mark>`+内嵌 CSS 同推导值(暗档 mark 文字 `#e8eaed`——暗媒体查询里正文仍是黑字,不另设文字色会黑压暗橄榄 3.2:1 不达标;`<title>` 取包裹后事件流,`# ==重点==` 的标题是「重点」,标记不回流正文;扫描**复用 `latermd_md::highlight_spans` 唯一扫描器**,豁免构造=与 `highlight_exempt_ranges` 同一份清单、由 pulldown 事件嵌套深度门控表达——代码块/链接/图片/强调类/脚注定义/HTML 块内的 Text 不扫,行内代码是原子 `Event::Code` 天然豁免)、Live 接入=`task_block` 扩名为 `rich_block`(高亮层先跑、任务层后跑,与预览四层链生产顺序一致;同一每块缓存,#63 纪律;`LiveRichHandler` 三分支与预览共享 `append_highlight_section`/`highlight_bg_color` 单一真源)、PDF 不随输出=如实核验后不硬做(`latermd-render::Style` IR 无 highlight 概念,`==` 按普通文本进 PDF;pdf 模块的 bg 矩形仅行内代码且 ink 硬编码 `Gray(240)`;将来路径=IR `Style` 加 `highlight` 位 + pdf `Prim::Rect`,本模块不做)(2026-10-07,#65 highlight-syntax M2·自动拍板)

- **岔路**:①底色推导源——extreme_bg / panel_bg / 双份固定 hex;②行高——复制 vendored 公式 / 不设行高只靠 `valign`;③导出标签——原生 `<mark>` / `<span class=mark>` / 裸 `<mark>` 不配 CSS;④导出扫描器——复用 `highlight_spans` / export 自写第二套 / 全文 span 经 offset 映射进事件流;⑤Live 缓存——扩 `task_cache` 为两层同槽 / 另开一槽;⑥悬停与手型——vendored 默认(下划线+手型) / 探针抑制。
- **备选**:①`extreme_bg`(文本背景最远端,对比度最优) / `panel_bg` / `if dark {两份 hex}`(wiki_link_color 先例);②抄公式(测试钉住) / `line_height: None`(egui 行内基线对齐,但与 1.30 行距档的正文段行盒不等高,底色矩形偏矮);③`<mark>`(语义标签,无 CSS 也有浏览器默认黄兜底) / span+class(要靠 CSS 才有观感) / 裸 mark(观感全交给 UA);④复用唯一扫描器(铁律 1 同族) / 第二套方言(明确禁止) / offset 映射(转义/实体解码后事件内容与源码字节错位,映射脆弱);⑤同槽(改写本就是「富渲染块改写文本」一件事) / 双槽(两份缓存同键冗余);⑥默认 / 抑制(要仿 emoji 探针再拉一套区块登记)。
- **自动选了**:见首行六处口径。落地:`highlight_bg_color` + `append_highlight_section`(preview.rs,`pub(crate)` 单一真源);`AiLinkHandler`/`LiveRichHandler` 的 `link_style`(正文色+无下划线意图)、`click`(吞掉不开浏览器不发消息)、`layout_link`(接管排版段)三分支;导出侧 `wrap_highlights`/`push_marked_text`(latermd-export,新依赖 latermd-md);`live.task_block`→`rich_block`(`task_cache`→`rich_cache` 同步改名)。
- **为什么**:①extreme_bg 是文本背景最远端,明暗推导只动明度端、色相恒定,恰好是「荧光笔」语义;egui 裸默认暗主题正文(`widgets.noninteractive.fg_stroke`)只有 2.59:1,所以对比度下限必须对**出厂皮肤投影后的生产输入**断言(实测教训,首版用裸默认当场红);②抄公式+钉住测试是vendored 私有函数下的最小侵入方案,不设行高的行盒偏矮会把底色矩形和行距档一起做歪;③`<mark>` 是 HTML 语义同义标签的标准答案,配 CSS 管观感、无 CSS 有 UA 默认兜底;④导出管线吃源码(#123 探明),== 识别必须另起,但「另一套方言」是铁律红线——复用 `highlight_spans` 让配对口径天然与预览逐字节一致(豁免构造在事件流里成对出现,深度门控即平衡);⑤两层改写同属「富渲染块改写文本」,一个缓存键一份产物,双槽是冗余;⑥悬停下划线 vendored 无条件画(#11 已知限),能定的是颜色走 `link_style` 取正文色低调呈现,探针抑制要为新构造拉一整套区块登记,收益配不上成本。
- **如何改**:①换底色源——改 `HIGHLIGHT_TINT`/`HIGHLIGHT_TINT_RATIO` 或 `highlight_bg_color` 的 base 取值,导出 CSS 两处十六进制同步;②不抄公式——vendored 若把 `line_height_for` 公开,删 app 侧副本改直调,钉住测试即转绿;③换导出标签——改 `MARK_OPEN`/`MARK_CLOSE` 与 CSS 选择器;④放宽导出豁免(允许强调内高亮)——先在 `highlight_candidates` 放宽,导出深度门控同步增删 Tag 分支(清单与预览永远同一份);⑤Live 缓存拆槽——`rich_cache` 拆回两层并各挂各的修订号;⑥抑制悬停——照 emoji 探针手法为 `hl://` 段登记区块+帧末压光标/下划线。PDF 接入走 IR 扩展(见首行第六处),属独立模块级工程。

## #123 #65 M1 高亮的六处口径:通道=渲染副本改写 `[文本](<hl://>)`(载荷为空,链接文字即内容;照 #48/#63 先例,M2 零 vendor——实测 vendored `append_link_to_job` 对非 inline-widget 链接同样以正文色调用 `layout_link`(layout.rs:172-176),app 侧返回 true + `TextFormat::background` 即得底色,无需 ①类 hook;span 旁路反而要按区间拆 Text 段的 ①类 hook)、层序=wikilink → 高亮 → emoji → 任务(高亮在 emoji 之前:`==😀==` 先成 hl 链接,emoji 层既有的「链接文本/目标整体豁免」顺势保护它——高亮保住、emoji 以字形直显;反过来 emoji 先跑会让 `==😀==` 整对作废;在 wikilink 之后:展开出的链接文本/目标里的 `==` 靠豁免清单不误伤)、配对=贴字定界 flanking(开 `==` 后随字符非空白 char 级、闭 `==` 前驱字符非空白 ASCII 级,全角空格等多字节空白视作内容)+ 最左最近配对、**首个合法闭标即定终**(该对作废则开标按字面、闭标不回收,`==a ==b== c==` 只高亮 `b`)+ 不嵌套 + 不跨行 + 未闭合/空/纯空白全字面(`==x`、`====`、`== ==`、`==x ==`)、内部富构造=整对作废(禁 `[`/`]`——改写形态是链接文字,未配平方括号会把链接拆成字面;禁 `==`;豁免清单=emoji 层清单**再加强调类**——实测 vendored parser 对链接文字只取**最后一个** Text/Code 事件(parser.rs:487-496,覆盖而非拼接),`==**b** c==` 这类多段内容改写后会**丢字**而非仅丢样式,故纯文本才高亮、宁缺勿错)、scheme=`hl://`(`HIGHLIGHT_SCHEME` 常量,载荷为空故无 emoji 层「载荷直书受 `<…>` 目标禁 `>`/换行」的约束)、M2 待定挂账=底色值(明暗主题 token 推导+对比度下限,#122 同族规格)、HTML 导出标签形态(`<mark>` 或同义;导出管线吃**源码**(state.rs:3382 `export_html(editor.text())`),高亮改写根本不进导出,导出侧要自行识别 `==` 语法)、Live 富渲染块接入(照 live.rs `task_block` 先例加每块缓存,#65 M2 范围)(2026-10-07,#65 highlight-syntax M1·自动拍板)

- **岔路**:①通道形态——任务书二选一:渲染副本改写(照 #48 实况)vs span 旁路;②四层改写链中高亮插在哪;③配对口径(未闭合/空/纯空白/跨行/嵌套的取舍);④高亮内容里混入方括号/链接/行内代码/强调怎么办;⑤scheme 命名与载荷形态;⑥底色值/导出标签形态/Live 接入的任务书留白项。
- **备选**:①`[文本](<hl://>)` 链接改写 / `highlight_spans` 区间表直送渲染层;②wikilink 之后 emoji 之前 / 链尾(task 之后);③markdown-it 系 flanking + 最左配对 / 无 flanking 的朴素 `find("==")` 配对 / 消费双端的作废口径;④方括号与富构造整对作废 / 方括号转义进链接文字 / 仅豁免 emoji 清单不豁免强调;⑤`hl://` 空载荷 / 载荷直书内容 / 百分号编码;⑥M2 自定。
- **自动选了**:见首行六处口径。落地:`highlight_spans` 纯函数 + `expand_highlight_links` 四层链新第二层(latermd-md);`PreviewState::highlight_map` + `map_source_offset` 五层串穿 + 任务载荷逆穿改三层(app);落盘零变化钉在 `highlight_rewrite_never_leaks_into_saved_bytes`(#32 快照护栏同法)。
- **为什么**:①M1 红线「vendor 零改动」——链接渠道 M2 只动 app 侧 `AiLinkHandler::layout_link`(实测对普通链接以非透明色调用,返回 true 即接管 section 样式,`TextFormat::background` 是 egui 标准字段);span 旁路要把字节区间映射进 vendored 的 galley section,必须 ①类;②高频形态 `==纯文本==`/`==😀==` 保住高亮,链接内 `==` 的误伤由豁免清单兜住(与 emoji 层对既有链接同一纪律);③无 flanking 的朴素配对会被 `== ==` 吞掉后续真配对的开标(`== == 与 ==真高亮==` 会高亮出「 与 」),flanking 是 markdown-it-mark 同族直觉,「闭标不回收」保证作废对不产生连锁误配;④实测 vendored `link_text` 取最后事件(覆盖语义),转义方括号救得了字面救不了多段丢字,作废最诚实;⑤链接文字本身就是高亮内容,载荷重复且引入编码约束,空载荷零解码;⑥M2 范围,M1 只把渠道与约束面探明并挂账。
- **如何改**:①改 span 旁路——latermd-md 产区间表 + vendored ①类 hook(按区间拆 Text section 设 background),按 §6 独立 vendor: commit + 变更表;②改层序——`PreviewState::new` 与 `map_source_offset`/任务载荷逆穿的串行顺序同步换(消费点全部显式传表,编译器兜底);③改配对口径——`latermd_md::highlight_candidates` 的 flanking/定终两段;④放宽嵌套(允许强调类进高亮)——先给 vendored 链接文字扁平化补 ①类(聚合多个 Text/Code 事件再产出),或整体换 span 旁路;⑤改 scheme——`HIGHLIGHT_SCHEME` 常量与 app 侧前缀拦截(M2 落地处)同改;⑥底色值/`<mark>`/Live 接入在 #65 M2 内拍板,本条不预支。

## #122 #64 M2 专注模式的八处口径:淡化范围=活动块+光标邻块(前后各一,abs_diff ≤1)保持全对比度、距离 ≥2 的非活动块淡化、无活动块(active=None,光标未落)时全部正常(专注的对象是「正在编辑的块」,无焦点无淡化,全淡化=整篇涂灰只剩干扰)、淡化档位=遮罩色 text_edit_bg_color × α110(残留 ≈57%,#81 内联标记遮罩同源;出厂明暗主题按 sRGB 逐通道混合×WCAG 相对亮度折算混后对比度 5.10/4.74:1,均 ≥ AA 正文线 4.5:1;不做用户可调,#81 同款规格口径)、无过渡动画(常量遮罩,#81 同款;crossfade 要给每块挂一个动画状态面,且关闭态零路径的否决线不允许任何动画簿记)、禅定模式下不生效(禅定正文是只读预览面板,draw_zen 走 ui::preview 无活动块概念;将来禅定若允许编辑再接入)、快捷键=Ctrl/Cmd+Alt+D(D=Dim,淡化非活动块;出厂表 Ctrl/Cmd+Alt 层**2026-10-15 后**只有 W,D 空闲 —— 原 R = ToggleRightPreview 已随该命令删除;含 Alt 的绑定按保守口径避开菜单标题助记集 F/E/O/V/X/A/S,#62 口径 —— 首选 Ctrl/Cmd+Alt+F 因 Alt 裸 F=「文件」标题被 menubar 标题审计测试当场否决,审计工作正常;与 Ctrl/Cmd+D 复制选中只差一个 Alt,靠「修饰键个数降序」共存,同打字机之于关标签)、菜单归属=「视图」第一段(Live/打字机同段,同为编辑区呈现方式开关;条目层助记 F,该段 S/P/V/W/T/Z 未占)、源码模式不可达=如实记录(源码是单个 TextEdit,分段淡化没有纯绘制层落点,块级遮罩会盖住光标/选区,不硬做;开关在源码模式下无效果不报错;将来路径=自定义 Layouter 按 block 弱化非活动行,属 vendored/egui 层 ①类工程)(2026-10-07,#64 typewriter-focus M2·自动拍板)

- **岔路**:①淡化范围(只活动块 vs 活动块+邻块);②无活动块时的行为;③淡化档位(α 值、可读性下限、要不要用户可调);④要不要过渡动画;⑤禅定模式下的行为;⑥快捷键与 Alt 层避让;⑦菜单归属与条目助记;⑧源码模式的边界(任务书明文「不可达如实记录」)。

- **自动选了**:见首行八处口径。落地要点:纯函数面在 `crates/latermd-app/src/ui/focus.rs`(`dimmed` 判定 + `DIM_ALPHA` 常量,零 egui 依赖,#64 M1 typewriter 同款分层);绘制在 `live.rs` 富渲染分支块矩形上叠一层遮罩(#81 纯绘制先例:不注册交互、不参与命中测试),遮罩矩形=块的点击命中矩形(`clicked_for_edit` 同一 rect,全宽)——视觉与命中一致;淡化块仍可点击进入编辑,进入即变活动块、下帧起不再淡化。遮罩后画覆盖块内 checkbox/复制头的绘制层,但其命中探针独立于绘制,点击行为不受影响(测试 `focus_click_on_faded_block_enters_edit_and_recovers` 钉住)。

- **为什么**:①任务书明文「活动块与光标邻块正常」,邻块保留是渐进淡化的最小上下文(iA Writer/Ulysses 同族形态);②没有编辑焦点就没有专注对象,全部淡化只剩干扰;③α110 是「明显弱化但不残废」与两主题 4.5:1 下限的交点,α 再高(如 #81 的 166)整块正文掉到 3.4:1 以下,不可读;④块切换频闪成本低,动画簿记与关闭否决线冲突;⑤禅定没有 Live 编辑器,接线无落点;⑥F 在语义上最直白但被标题审计当场否决(这正是审计该干的活),D=Dim 语义贴合功能本质且在 Alt 组合层完全空闲,与 Ctrl+D 的共存靠修饰键个数降序(#9 口径);⑦与 Live/打字机相邻,用户在同一个地方找呈现方式开关;⑧单 TextEdit 无分段绘制落点是结构事实,自定义 Layouter 是另一个量级的 vendored 工程,不混入本模块。

- **如何改**:①范围阈值改 `ui::focus::dimmed` 的 `abs_diff` 常量;②α 改 `ui::focus::DIM_ALPHA`(改前重折算两主题对比度下限,亮底黑字是紧的一侧);③快捷键改 `command.rs` `default_shortcut`(出厂表迁移需照 #45 K1/#114/#121 写 `keymap::load_from` 感知迁移);④加动画:绘制处改用 `ui::fade::crossfade`,id 含块序号(注意编辑帧块序漂移会让 alpha 闪变,需按块内容键或接受闪烁);⑤禅定接入:`draw_zen` 若改为可编辑布局,把 `show_focus_mode` 传进该路径并复用同一 `dimmed`。

## #121 #64 M1 打字机模式的八处口径:目标带位置=视口 1/3 线(ANCHOR_RATIO,光标行顶对齐,打字推进下方留 2/3 视口,主流打字机滚动惯例)、死区=目标线 ±1.5 行(DEAD_ZONE_ROWS,行内打字不滚、越带一次滚一次不逐键微跳)、接管恢复口径=任务书选项①「直到下次编辑/光标移动再恢复」(编辑帧含修订号前进:打字/IME/undo/redo/程序写入;键盘导航帧同属恢复;UserOverride 态本身不抑制滚动 —— 恢复即编辑帧,状态机是接管判定的显式记账与未来 N 秒口径的挂点)、用户滚动判定=offset diff(本帧偏移变化且非打字机自身落地且非编辑帧 → Override;落地后 4 帧宽限豁免 egui「end 记目标→次帧 begin 应用 offset→再次帧布局反映」的滚动落地时序;编辑帧优先,打字+滚轮同帧打字赢;已知边界=滚轮撞文档边界时 egui 保留未消费滚动量会抵消之后一次 land,以及 Live 活动块切换引起的 offset 钳位会被误记一次 Override —— 均为一次无害暂停,下次编辑即恢复)、默认值=关(改变滚动行为的功能出厂不替用户决定;关闭时滚动路径与从前逐字节相同,否决线)、快捷键=Ctrl/Cmd+Alt+W(出厂表整条 Shortcut 无占用者;与 Ctrl/Cmd+W 关标签只差一个 Alt,靠「修饰键个数降序」消费顺序共存,EmojiPicker/ExportHtml 同款;Alt 层纯字母 F/E/O/V/X/A/S/T 已被菜单标题助记与主题键占用(#119/#45 K1),W 空闲)、菜单归属=「视图」第一段(Live 同段,同为编辑区呈现方式开关;条目层助记 W,该菜单内 S/P/V/T 已占)、触发面扩展=修订号前进帧进打字/光标移动跟随(TextEdit 已把消费掉的按键移出事件流,修订号是不依赖事件枚举的编辑信号;undo/redo/程序写入一网打尽)+开启当帧视为一次编辑(开启即对齐一次)(2026-10-07,#64 typewriter-focus M1·自动拍板)

- **岔路**:①目标带位置(任务书建议中 1/3,值自选);②死区带宽(避免每键微跳);③接管恢复口径(任务书二选一:下次编辑恢复 vs N 秒静默恢复);④用户滚动的判定方式;⑤出厂默认值;⑥快捷键与 Alt 类字母避让;⑦菜单归属;⑧打字/undo 帧的跟随触发面。

- **自动选了**:见首行八处口径。落地要点:纯函数面在 `crates/latermd-app/src/ui/typewriter.rs`(`scroll_delta` 决策 + `step` 状态机,无 egui 依赖),落地在 `ui::editor` / `live` 的 ScrollArea 闭包内(单像素矩形 + `Align::TOP` + `ScrollAnimation::none` 一次到位,文首/文末钳位由 egui `ScrollArea::end` 的 offset 钳制兜底);实测确立的 egui 0.36.2 事实已写进代码注释:程序化滚动 = 本帧 end 记目标 → 次帧 begin() 应用 offset → 再次帧布局反映(无头测试按三帧收敛驱动);滚轮撞文档边界时未消费的滚动量会残留在 `smooth_scroll_delta` 并抵消之后一次 land(scroll_area.rs:1236-1249,只有离界消费才清零)。

- **为什么**:①1/3 线是打字机模式的事实标准(下方留 2/3 预读区);②死区按行计不按 px(行高随字号偏好变化,按行语义稳定);③「下次编辑恢复」是各编辑器打字机模式的共同行为,N 秒口径在其之上无行为增量(空闲帧本来不滚),留作挂点不实现;④offset diff 是唯一能同时覆盖滚轮/滚动条拖动/滚动条轨道点击/内容拖拽的信号,显式事件枚举必然漏;⑤默认关与 #55 minimap 默认开相对 —— minimap 是纯增量视觉,打字机改变滚动行为;⑥W 在 Alt 层空闲且 Ctrl+Alt+R(切预览栏)同族;⑦视图段与 Live 相邻,用户在同一个地方找呈现方式开关;⑧修订号前进是最小可靠信号(事件枚举在 TextEdit 消费后不可靠)。

- **如何改**:全部常量集中在 `ui/typewriter.rs` 顶部(`ANCHOR_RATIO`/`DEAD_ZONE_ROWS`/`LAND_GRACE_FRAMES`/`MOVED_EPSILON`),改值即改观感;恢复口径改 N 秒需在 `step` 加时间参量并在 `Memory` 存时间戳;默认值改 `theme.rs` 的 `ThemeSettings::default` 与两处测试;快捷键在 `command.rs` `default_shortcut` 改(出厂表迁移需照 #45 K1/#114 写 `keymap::load_from` 感知迁移)。

## #120 #63 任务列表点击勾选的六处口径:实现路径=①渲染副本改写器(照 #48)为骨架叠加 #38 探针命中(两者不互斥:vendored 对列表项 inline widget 的 click 路径实测不可达,探针是唯一可达点击通道)、命中区=透明占位区整体(13a 同层,自绘方框 0.72 边长只是视觉)、悬停=PointingHand+边框加粗+背景微亮(自读指针不注册 widget)、导出零新代码(管线吃源码,pulldown HTML writer 本产 `<input type=checkbox>` 勾选态随源码天然一致)、勾选写法=已勾(`x`/`X`)取消写空格、勾选统一写小写 `x`、Live 侧点击消费=帧末探针命中+坐标域分账(Preview=emoji 层坐标逆穿两层 / Live=块内坐标加记录时块首),handler.click 只吞不转发(防 vendored 将来修好错位后双发)(2026-10-07,#63 task-checkbox M1·自动拍板)

- **岔路**:①实现路径——任务书二选一:①渲染副本改写器(照 #48 emoji 先例)vs ②数据探针+命中(照 #38 copy_button_rects 先例);②命中区大小——自绘方框本身 vs 改写占位区整体;③悬停态形态——自选;④导出一致性口径——导出现状核验后自选记录;⑤勾选写法——翻转 ` `↔`x` 之外大写 `X` 与写回大小写怎么定;⑥Live 侧点击通道——vendored `LinkHandler::click` vs 探针帧末判定;⑦点击勾选的焦点纪律——归还给谁、何时取快照。
- **备选**:①a 纯改写器(点击走 vendored handler);①b 纯探针(不改写,渲染仍是字面 `☑`/`☐`,点击按几何找行);①c 改写器骨架 + 探针命中(改写让 checkbox 变成 inline widget 有占位几何,探针记录占位区做点击判定)。②a 自绘方框 rect;②b 占位区(符号+尾空格推进宽度)整体。③a 仅光标手型;③b 手型+视觉反馈。④a 导出加 checkbox 后处理;④b 核验现状零改动。⑤a `X` 取消后写回大写 `X`;⑤b 统一写小写 `x`(GFM 主流);⑤c 保原大小写。⑥a 依赖 vendored handler.click;⑥b 帧末探针判定 + handler 只吞不发;⑥c 双通道并发。⑦a 释放帧帧首读 `focused()` 归还;⑦b 按下帧快照暂存、释放帧命中后归还。
- **自动选择**:①c + ②b + ③b + ④b + ⑤b + ⑥b + ⑦b。**①**:现状核验——vendored `layout.rs:574` 把 TaskListMarker 画成字面 `☑ `/`☐ ` 文本(base_format,不可点无专门渲染),纯探针(①b)要自己从字面文本反推几何,反而更多表面积;改写器让标记变成链接占位,几何免费。但 vendored 点击路径对列表项 inline widget **实测不可达**(handle_click 依赖上一帧 handle_hover 的 section 映射,列表项布局的 section_to_token 与 egui 0.36 galley 空文本 section 错位;预览与 Live 两处各有断零消息测试钉住)——探针帧末判定(#38/#61 纪律,读原始指针不挂 widget)因此不是「二选一的另一条路」而是**唯一可达的点击通道**,①c 是现状下的唯一完整解。**②**:占位区整体是改写层免费给出的几何,且比 0.72 边长的方框大 ~39%,点击容错好;方框只是绘制,命中区即占位区(与 #48 emoji「占位区即几何」同口径)。**③**:checkbox 是可点件,手型 + 边框加粗 + 背景微亮是标准可点反馈;emoji 的「手型抑制」方向相反但机制同源(都按探针几何自设光标,vendored 对列表项 widget 的 hover 光标同样到不了)。**④**:导出管线(latermd-export)吃**源码**而非预览渲染串,pulldown HTML writer 对 TaskListMarker 本产 `<input disabled type="checkbox" checked>`(既有测试 `task_list_renders_checkboxes` 钉住);源码翻转后导出自动跟随,零新代码零漂移面。**⑤**:已勾含大写 `X`(pulldown 事件 checked=true 的两种源码形态),取消一律写空格,勾选统一写小写 `x`——GitHub/VS Code 主流行为,`X`→`X` 的保形反而制造方言。**⑥**:见①的不可达实测;handler 侧仍返回 true 吞掉(不交系统浏览器),但**不转发消息**——将来 vendored 修好错位时双通道会双发(两次切换=一步空转+两份 undo),单点消费在帧末判定。**⑦**:egui 0.36 的让焦(`SurrenderFocusOn::Presses`)发生在**按下帧**的 widget 绘制段(context.rs `read_response` 逐 widget 结算),释放帧帧首 `focused()` 已是 None——⑦a 实测把焦点还给了 label 自己(焦点测试红过一轮);绘制段内的 request_focus 又会被同帧后续 widget 的让焦吃掉,唯一稳定位是绘制段之后(layout.rs `keep_find_focus` 同款)。按下帧帧首快照存 egui temp(帧号核对,预览面板先画先收,Live 后画不覆盖),释放帧命中后归还;不点 checkbox 的拖选自然过期,不误还。
- **理由**:①改写层完全复用 #48 的三层映射基建(OffsetMap 串行穿过/逆穿),判定即豁免(pulldown TaskListMarker 事件 range 之外的 `[ ]` 一律不碰,代码块/行内代码/普通文本天然豁免,#10 同款否决线);②命中区与占位区同 rect 免去「视觉件与命中件错位」这一整类缺陷;④导出侧的一致性由「单一解析器」铁律免费保证;⑤「宁可不写也不写错位置」的归约闸门(`is_task_marker_start` pulldown 重扫)兜住全部过期偏移/形态漂移;⑥/⑦的不可达实测与让焦时序都以无头渲染测试钉住(live 断零消息测试 + 焦点归还测试),不是推断。
- **如何改**:①要 vendored 原生可点 checkbox——属 vendor ①类(upstream 可合的 inline widget click 修复),按 §6 纪律另开分支,修好后删帧末探针判定改走 handler.click(载荷协议不变);②要命中区缩到方框——`paint_task_checkbox` 记录 `box_rect` 而非占位 rect,一行改动;③要纯手型无视觉反馈——删 hover 分支两处绘制调用;④导出要非 disabled 形态——改 latermd-export 的 HTML 后处理(pulldown 无开关),与勾选回写无关;⑤要保大小写——`toggle_task_checkbox` 的 match 加 `[X]` 分支写 `X`(现有测试 `task_toggle_flips_marker_state` 的对应断言同步改);⑥vendored 错位修复后 handler 转发与帧末判定二选一保留,另一条删除,防双发的断言(单次点击恰一条消息)已在两处测试里;⑦焦点不要归还——删 `restore_stashed_focus` 两处调用即可,暂存写入无副作用。

## #119 #62 M2 排版统一与 Alt 助记键的七处口径:egui 0.36.2 无 `&X` 助记语法→菜单层自建「标题层 Alt+字母 / 条目层展开态裸字母」两层降级、助记字母分配 文件F/编辑E/格式O/视图V/导出X/AI A/设置S(条目层全量分配菜单内唯一)、显示形态取「文件(F)」括号后缀(epaint 无法只给单字母画下划线)、mac 不显示不启用助记、单击 Alt 聚焦菜单栏首菜单(egui-winit 的 Alt 键事件可达,状态机实现)、菜单溢出策略=保持单行裁切不换行不滚动、菜单层两侧(菜单条/下拉)一致不占图标位(2026-10-07,#62 menu-overhaul M2·自动拍板)

- **岔路**:①egui 0.36.2 的 mnemonic 支持度(任务书要求 registry 源码实读)——支持 `&X` 则直接用,不支持则降级到哪层;②助记字母怎么分配(标题层全局唯一 + 条目层范围),与 keymap 全部 Alt 类绑定的冲突怎么审计;③显示形态——Windows 风单字母下划线 vs 括号后缀「文件(F)」;④mac 口径——不显示助记 vs 保留字符;⑤「单击 Alt 聚焦菜单栏」——egui/winit 层可行性未验,可行则实现方式;⑥菜单多到溢出的策略——换行 vs 横向滚动 vs 现状单行;⑦排版统一的「图标位」——预留固定图标槽 vs 两侧一致不占位。
- **备选**:①a egui 内建 `&X`(若支持);①b 仅标题层 Alt 助记;①c 自建两层(标题层 + 展开态条目层)。②标题层字母取菜单名首字母直译(F/F 冲突)/ 惯例避让(F E O V X A S);③a 下划线;③b 括号后缀。④a mac 不显示不启用;④b 显示但不启用。⑤a Alt 键事件可达 → 状态机检测「按下→抬起无夹杂」;⑤b 不可达则不做。⑥a 保持单行;⑥b horizontal_wrapped 换行;⑥c 横向 ScrollArea。⑦a 预留 ICON 宽度空槽;⑦b 两侧一致不占位,记录口径。
- **自动选择**:①c + ②惯例避让 + ③b + ④a + ⑤a + ⑥a + ⑦b。**①**:实读结论——egui-0.36.2 与 epaint-0.36.2 全 crate `mnemonic` grep 零命中;`containers/menu.rs` 的 `MenuButton`/`SubMenuButton` 只是 `Button` 包装,`widgets/button.rs` 无 `&` 解析;无 accessibility 助记 registry。`&X` 不存在,降级自建:标题层用 `Popup::open_id`(egui 公开 API,popup.rs:679)驱动菜单展开,条目层在展开闭包内扫裸字母。两层都比「仅标题层」(①b)多覆盖一层键盘可达性,且条目层与 keymap 零交集(keymap 的 `bindable()` 拒绝裸字母绑定,出厂表无裸字母/数字键)。**②**:标题层全局唯一 F E O V X A S —— 格式的自然字母 F 已给文件,取 O(LibreOffice「格式」惯例);导出取 X(eXport);全部避开 Alt+T(主题,#45 K1)与 R(Ctrl+Alt+R 预览栏占用,虽然检测用位相等不会误触,仍保守避开)。条目层 40 条全量分配、菜单内唯一(跨菜单允许重复:裸字母按当前展开菜单局部解析)。冲突审计不是一次性的:`title_mnemonics_unique_and_clear_of_keymap_alt_bindings` 遍历 keymap 出厂表逐条断言(非 mac 上任何含 Alt 的绑定主键都不得在标题字母集),`item_mnemonics_unique_per_menu_and_keymap_has_no_bare_letters` 钉住条目层与裸键零交集 —— 将来新命令绑 Alt+F 或字母撞车,CI 必红。**③**:epaint 的下划线(`TextFormat::underline`)作用于整段 job,无法只给单个字形画线;自绘两段 galley 拼接成本高且字体度量脆弱。括号后缀是中文 Windows 软件的标准形态(记事本/资源管理器同款),零渲染风险。**④**:macOS 菜单助记是系统级菜单栏的事(且常默认隐藏下划线),自绘菜单显示助记字符反而违背平台惯例;`label_with_mnemonic` 与全部检测函数在 mac 编译目标下早退,零行为。**⑤**:实读 egui-winit 0.36.2 `lib.rs:1648-1649` 把 `KeyCode::AltLeft/Right` 映射为 `Key::AltLeft/AltRight`、`lib.rs:1038` 一律推 `Event::Key` —— Alt 修饰键本身的事件可达,无需 vendor/winit 改动。实现为跨帧武装状态机(press 武装、任何其他按键/文本/指针按下污染、release 且未污染则触发),武装标志挂 egui 临时数据(纯 UI 会话态,不进 State 存档)。触发动作=打开首个菜单「文件」(Windows 惯例是聚焦首个菜单标题,打开即聚焦的近似)。**⑥**:实测 egui 0.36 `MenuBar` 是固定 `ui.horizontal`(menu.rs:262),不换行不滚动;7 个菜单标题(带助记后缀约 420px)远小于窗口常规宽度,窄窗(400px)单行裁切不 panic(`menubar_and_item_share_font_size_and_narrow_render_is_safe` 守门)。换行会拉高菜单条挤编辑区,横向滚动违背菜单条的即时可视性,都为不存在的场景引入复杂度。**⑦**:菜单条与下拉当前都无图标,预留空槽会让每条左移 20px 的空白,观感劣化;图标位口径=两侧一致不占位,将来若加图标再统一补槽(工具条 `ui::icons` 自绘体系现成)。
- **理由**:①两层自建的表面积收敛在 `ui/menubar.rs` 一个文件(MENUS 三元组 + `command_mnemonic` 表 + 三个检测函数),零 vendor、零 command/keymap 改动,守「纯菜单层红线」;条目层触发在展开闭包内做,天然只在菜单可见时消费输入,闭合语义免费。②标题层字母的冲突面只有「纯 Alt+键」(检测用 `modifiers == Modifiers::ALT` 位相等,`matches_logically` 的宽松语义——忽略多余 Alt——会放 Ctrl+Alt+R 进来,故弃用);条目层只在菜单展开时生效,与快捷键(必须有修饰键)是两个不相交的输入面。③egui 的 `RichText` 没有部分字形装饰,下划线方案要手写两段 layouter,为观感细节引脆弱排版代码不值。④助记层的三个入口(显示后缀/Alt 字母/裸字母/Alt 单击)共用 `cfg!(target_os = "macos")` 早退,一处口径。⑤Alt 单击的污染规则覆盖 Alt+字母组合(字母按下帧即污染,组合走 Alt+字母助记)、Alt+Tab(切窗)、Alt+拖拽(指针按下污染),不会误聚焦。⑥溢出是假设性场景,现状核验后定=单行;7 菜单是编译期常量,真到溢出时再换策略,现在不做。⑦「图标位统一」的正解是两侧同口径,而不是两侧都空出一个看不见的槽。
- **如何改**:①要 egui 原生助记——等上游(0.36 无计划,issue 区无相关提案),届时删自建层换 `&X`,字母表可原样平移;②调整字母——改 `MENUS` 三元组的第二项或 `command_mnemonic` 的 match,冲突审计测试自动重跑(撞 keymap 必红);③要下划线形态——给 `label_with_mnemonic` 换两段 galley 拼接实现(epaint `TextFormat::underline` 只能整段,需按字母偏移拆 job),同时保留括号后缀为 mac 口径;④mac 想显示助记——删 `label_with_mnemonic` 的 cfg 早退(键盘触发层的三个早退是否同步放开另议,⌥ 键在 mac 是特殊字符输入修饰);⑤单击 Alt 想只聚焦不展开——把 `open_first` 的动作从 `Popup::open_id(首菜单)` 改为给菜单条标题 `request_focus`(需要 Button 可聚焦,egui 0.36 默认不参加 tab 导航,得逐个 `.focusable(true)`);⑥要换行/滚动——`MenuBar` 不支持,需弃 `egui::MenuBar` 自组 `horizontal_wrapped` 或 ScrollArea 包裹,菜单展开锚定语义需回归验证;⑦要图标——`MENUS` 加图标字段 + 条目 `item()` 前补 `icons::icon_button` 同款绘制,两侧同帧上,不单侧先行。

## #118 #62 M1 菜单功能完善的三处口径:菜单栏顺序重排为 文件→编辑→格式→视图→导出→AI→设置(导出从第二位移到视图后)、「格式」菜单的块级段不含图片(图片与 Emoji 同归「插入」段,与工具条 FormatGroup::Block 含图片的口径有意分叉)、覆盖豁免清单定为空(注册表 40 条全量进菜单)(2026-10-07,#62 menu-overhaul M1·自动拍板)

- **岔路**:①菜单栏横向顺序 —— 维持既有(文件/导出/编辑/视图/AI/设置)最小扰动 vs 按「常用在前」重排;②新增「格式」菜单的分组 —— 完全照抄工具条 `FormatGroup` 四组(块级组含图片)vs 按用户心智把两个对话框类动作(图片/Emoji)独立成「插入」段;③「某命令确实不适合进菜单的在矩阵标注理由」的豁免清单 —— 给浮标/上下文类动作留豁免位 vs 断言全量覆盖(豁免为空)。
- **备选**:①a 重排为 文件→编辑→格式→视图→导出→AI→设置;①b 保持 文件→导出→编辑→视图→AI→设置,新菜单插尾部。②a 格式菜单五段:行内/标题/块(不含图片)/列表/插入(图片+Emoji);②b 照 FormatGroup 四段,图片进「块」,Emoji 附尾。③a 豁免为空 + 文档说明「注册表不含上下文类动作」;③b 给 selection_ai 浮标动作等留豁免声明。
- **自动选择**:①a+②a+③a。**①**:任务书明写「分组与排序规范:同类聚组、常用在前」,编辑/格式是写作高频入口,导出是低频派生物,放在编辑前违背该规范;命令行为与快捷键零变化(纯菜单层红线),重排不碰归约。**②**:图片要 alt+url 两个输入、Emoji 要点选,都是「点了不改文档」的对话框类动作(`Command` 注释与 format_bar 模块文档的既有定性),与十五条直改文档的格式动作隔一段更符合「同类聚组」;工具条按空间排(图片挤在块组),菜单按心智排,两处分组口径有意分叉并在 menu-coverage.md §2 落档。**③**:豁免位是给「确实不适合进菜单的命令」留的,当前注册表没有这类命令(选区 AI 浮标动作不经 Command 层);留空豁免 + 全量断言让「新命令忘进菜单」在 CI 必红,比预防性留豁免位(可能被当成漏加菜单的借口)守护更强。
- **理由**:①菜单顺序是纯展示层,MENUS 常量一处改动即整体生效,守门测试(归属断言按 group 映射)不受顺序影响;②图片/Emoji 的对话框定性在代码注释里已有两处先例(command.rs `ImageInsert` 文档、format_bar 模块文档),菜单跟进同一口径;③40/40 全覆盖让 `every_command_has_a_menu_entry` 无豁免分支,断言最简也最强(非恒真已实测:篡改常量当场红)。
- **如何改**:①要回旧顺序 —— 调整 `MENUS` 数组元素顺序即可,测试无感知;②要图片回「块」段 —— 把 `ImageInsert` 挪进 FORMAT_MENU 第三段、`EmojiPicker` 并入或保留插入段,同步 menu-coverage.md §1/§2;③要给某命令豁免 —— 在 `every_command_has_a_menu_entry` 加显式豁免集并在 menu-coverage.md §3 登记理由,不得静默跳过。

## #117 #61 M3 润色确认浮窗的六处口径:chunk 分流=复用既有流通道+State 会话字段(照 #116 的 ai_insert_at 手法)不建第二条流、素材超上下文上限=按 #58 预算拒绝显文案(润色素材不可截断)、浮窗期不锁编辑器(确认帧修订号见证失配即拒绝不盲写)、空选区=拒绝显文案(替换没有「文档末」可回退)、流式逐块显示+复制按钮带上、待确认会话的生命周期收口(新流顶掉/发起标签关闭作废)(2026-10-06,#61 selection-ai-bar M3·自动拍板)

- **岔路**:①润色 chunk 的载体(任务书「结果进只读浮窗」)——复用既有 AiChunk 流通道按会话分流 vs 给 AiState 建第二组 rx/streaming 的独立流;②「选区超上下文上限(#58 上下文配置若在,复用其截断口径)→显文案不 panic」的具体口径——超限拒绝 vs 照 summary 先例截断进 prompt;③「浮窗期不锁编辑器(或锁定口径自选记录)」——不锁+确认帧见证 vs 浮窗期锁编辑器;④空选区/塌缩选区的防御分支——拒绝显文案 vs 照 M2 续写回退某默认行为;⑤「流式逐块显示或完成后一次显示自选记录」+「复制按钮可选自选记录」;⑥陈旧见证的比对手续——修订号(#17 口径,照 M2)vs 选区内容比对(`buffer[selection] == 发起时原文`)。
- **备选**:①a 复用 `ai.start()` + State 挂 `ai_polish: Option<AiPolishSession>`(tab_id/selection/rev/draft),`append_ai_delta` 按会话有无分流(会话在途 chunk 进 draft 不写文档);①b AiState 加第二条独立流通道。②a `selected.len() > latermd_ai::polish_budget(context_budget())` 即拒绝落文案(预算换算收口在 latermd-ai `polish_budget`,None 落 32KB 现状默认);②b 截断素材进 prompt(summary 同款)。③a 不锁编辑器,`SelectionAiPolishConfirmed` 归约先比 `revision() != session.rev` 再替换,失配落「选区位置已失效」提示并作废会话;③b 浮窗期禁写编辑器。④a 无选区/塌缩/全空白一律 `document.notice` 显文案零副作用;④b 照续写回退。⑤a 浮窗每帧读最新 draft(流式逐块可见),确认按钮流式中禁用,「复制」按钮带上(`ctx.copy_text`);⑤b 完成后一次显示、不带复制。⑥a 修订号见证;⑥b 内容比对。
- **自动选择**:①a+②a+③a+④a+⑤a+⑥a。**①**:润色与续写共用防重入/AiDone/AiFailed 生命周期与 `ai_active_tab` 绑定,第二条流要复制整套簿记且「同时两条 AI 流」的产品语义(两个结果往哪放)没人定义;①a 与 #116 的 `ai_insert_at` 同构(会话字段分流写入目标),`ai_insert_at` 与 `ai_polish` 互斥由发起清场保证,存量四入口经 `start_ai_stream_with_prompt` 发起时清会话(纯 UI 状态,零文档改动)防 chunk 误分进旧草稿。**②**:润色素材**就是替换结果的来源**,截断=确认后把不完整的文本写进文档,语义上不可接受——summary 截断先例不可平移;预算读侧复用 #58 的 `context_budget()`(配置在则 KB×1024,不在落 32KB,与摘要同档),超限显「约 N KB」文案,「截断口径」的复用落在预算换算而非截断行为。**③**:锁编辑器违背 M1「浮标不抢焦点、出现期间打字正常」的既定体验,且流式等 3-5 秒里锁死输入是惩罚等待;不锁的代价(编辑后选区漂移)由确认帧 rev 见证兜住——修订号是单一事实源,失配即拒绝+提示+作废会话,绝不按过期坐标盲写;已知保守性与 #116 相同(编辑后撤销同样触发拒绝,宁误停不误写)。**④**:替换动作没有「文档末」可回退(M2 续写的回退语义不存在),没有素材只能拒绝;落显文案比 no-op(无声失败)诚实。**⑤**:逐块显示让「润色中」有打字机的可感知进度(浮窗零额外状态,每帧读 draft);确认按钮流式中禁用防半截草稿被确认;复制按钮成本一行(`copy_text`,commit 浮窗同款)而用户放弃场景里「把结果拷走另用」是高频诉求。**⑥**:修订号是缓冲自带的单调计数,O(1) 且与 M2 的插入点见证同一口径(认知零增量);内容比对要保存发起时原文(draft 之外多一份全量拷贝)且区间越界(编辑缩短文档)还要额外防御,复杂度更高却只比 rev 见证多放过「编辑过但选区内容恰好复原」这一种场景——不值得。
- **理由**:①关键回归(AiChunk 写发起标签/防重入/key 闸门/AiFailed 收尾)全数保留,新增断言只加不改;②错误路径矩阵(塌缩/空白/超限/无 key/请求失败/空结果)每条都有显文案零副作用单测;③确认替换走 `replace_range` 一次写入,TextEdit 内建 undoer 的单条 undo 契约(一次 Ctrl+Z 整体回原状)以真实 undoer 无头测试钉死(editor.rs,与 #17 全部替换同款);④放弃(按钮/Esc)零改动有专门测试,流式中放弃同时作废在途流;⑤Esc 消费在 reduce、先于蒙层 retain(最顶层浮层优先),浮窗不在场时零拦截;⑥待确认会话指向被关标签时作废(remove_tab 收口),浮窗不可能悬空卡死。
- **如何改**:①要独立双流——AiState 加第二组 rx/streaming 并给两流定义并发语义(不建议);②要截断口径——删 `start_selection_ai_polish` 的超限拒绝分支、改调 truncate(不建议:确认后丢内容);③要锁编辑器——reduce/draw 按 `ai_polish.is_some()` 拦编辑器输入(Esc/按钮之外的键全吞);④要空选区回退——`start_selection_ai_polish` 的塌缩分支改走某默认选区(无合理默认,不建议);⑤要完成后一次显示——浮窗在 `ai.is_streaming()` 时隐藏即可;⑥要内容比对——`AiPolishSession` 加 `original: String`,确认分支改比 `buffer[selection] == original`。

## #116 #61 M2 选区尾续写接线的五处口径:插入点=流会话级 State 字段(偏移+修订号见证)不进 Chunk 类型、捕获素材随消息携带(Live 块内坐标捕获帧换算文档坐标,不读源码模式专属的 TabState::selection)、陈旧防御=修订号失配即中止+提示不钳制、补段空行=插入点前后两侧锚定(文末选区与存量文档末路径逐字符等价)、无选区回退文档末续写(2026-10-06,#61 selection-ai-bar M2·自动拍板)

- **岔路**:①「目标插入偏移」随流携带的载体(任务书「最小扩展为随流携带,存量调用不传=文档末」)——State 流会话级字段 vs 扩展 `latermd-ai::Chunk` 类型带偏移 vs 每条 `Message::AiChunk` 加载荷;②捕获素材来源(任务书「动作时记录选区尾偏移+文档修订号见证」)——归约里读 `TabState::selection` vs 让选区随 `SelectionAiActionRequested` 消息携带;③陈旧防御口径(任务书「钳制/中止并提示,口径自选记录」);④插入点的段落化:只在选区尾前补空行 vs 前后两侧都补 vs 不补(任务书未提;文档末路径有 `ensure_trailing_blank_line` 先例);⑤防御分支「点续写时已无有效选区」的行为(落提示/静默忽略/回退文档末)。
- **备选**:①a State 挂 `ai_insert_at: Option<AiInsertPoint>`(偏移+见证修订号,流会话级,`AiDone`/`AiFailed`/作废一并清);①b `Chunk { delta, done }` 加 `at: Option<usize>`;①c `AiChunk { delta, at }` 每条带。②a 消息载荷带点选帧选区(`selection: Option<(usize,usize)>` 文档坐标,Live 捕获帧把块内偏移加块基换算);②b 归约读 `TabState::selection`。③a 失配即 `abort_ai_stream` + 提示落发起标签;③b 钳到文档末;③c 按编辑量平移偏移。④a 前侧矩阵(尾前连续换行 0 补 2/1 补 1/≥2 不补)+后侧对称矩阵,插入点夹在两组之间;④b 只补前侧;④c 不补。⑤a 回退 `start_ai_stream`(文档末续写);⑤b 落提示不执行;⑤c no-op。
- **自动选择**:①a+②a+③a+④a+⑤a。**①**:偏移是 app 层的文档坐标概念,provider 后台线程不识文档(它只按 prompt 吐块),`Chunk` 是错的载体;①c 每条消息重发同一个数是冗余;会话级字段与 `ai_active_tab` 同生命周期同手法,存量入口不挂它即 `None` = 文档末,零行为变化可单测钉死。**②**:「动作时」的真值在**点选帧**的 UI 层(源码 TextEdit 持久选区 / Live 活动块 `output.state.cursor`),而 `TabState::selection` 只有源码模式每帧回填,Live 下归约时读到的是**源码模式时代的陈旧值**(M1 里 Live 浮标的选区素材就是块内局部变量,从不写回标签字段);点选帧距归约仅一帧且中间无 UI 执行,载荷携带即零漂移。**③**:钳到文档末=把 AI 文本静默写进用户没点过的位置(比陈旧本身更糟);平移要 diff 整篇推算偏移,不可靠也不可测;中止+提示诚实且与「关发起标签作废流」同一收口(`abort_ai_stream`)。见证的刷新口径:AI 自身的写入随写随刷(`offset += delta.chars().count()`,`rev` 取写后值),只有**用户编辑**才会推高修订号触发失配;已知保守性——编辑后撤销(内容复原、修订号 +2)同样触发中止,rev 见证不认「内容相同」,宁误停不误写。**④**:不补则 AI 首行与选区尾行粘连、末行与尾后文本粘连(源码与预览都难读);只补前侧则末行粘连尾后文;两侧各按 `ensure_trailing_blank_line` 同一矩阵(0 补 2/1 补 1/≥2 不补),chunk 恰好长在两组补行之间。补行是发起归约的一部分,见证取补行后 → 补行不算用户编辑;选区到文档末时后侧为 0,与 `AiStart` 路径逐字符等价(有等价性单测钉死)。**⑤**:浮标只在有选区时在场(`badge_visible` 门),该分支纯防御;回退文档末保持「续写」动作语义可用,比弹提示(吓人)与 no-op(无声失败)都好。
- **理由**:①存量四入口(AiStart/ai:// 链接/摘要/commit)的回归面由「不挂字段 = 文档末」一个分支守住,关键回归测试全数保留并加否决线断言;②Live 块内→文档坐标的换算与光标回填同款(`byte_to_char(块基) + 块内字符偏移`,live.rs 光标回填先例),活动块是唯一可编辑块、块基不受本帧编辑影响,换算恒成立,且换算结果在无头测试里逐值断言(Live 选区 (2,6) → (9,13)/(13,9));③中止文案落**发起标签**(与 `AiFailed` 同定位),流式标志/指令卡状态键/绑定/插入点四处一并清,不会卡防重入;④补行使中点选区的最终形态是「前文 ⏎⏎AI⏎⏎后文」,heal/预览链路照既有通道显示(`streaming_here` 判定只看 `ai_active_tab`,不区分落点);⑤五条入口的 key 闸门与防重入语义不变(被拦连补行都不发生,与其它 AI 入口同一纪律)。
- **如何改**:①要偏移进协议层——`latermd-ai::Chunk` 加 `at` 并让 app 归约消费,注意四家 provider 都要填默认;②要归约自读选区——先给 Live 模式补 `TabState::selection` 的文档坐标回填(需定义活动块选区与标签字段的同步时机),再让 `SelectionAiActionRequested` 退回单载荷;③要钳制口径——`append_ai_delta` 失配分支改为 `offset.min(len_chars())`(不建议:静默写错位置);④要不补行/只补一行——删 `start_selection_ai_continue` 的对应 `pad` 计算即可,矩阵是纯函数无隐藏耦合;⑤要无选区落提示——回退分支改置 `document.notice`。

## #115 #61 M1 选中态 AI 浮标的五处口径:形态=chip+点开两行菜单(单自选)、拖拽判定以 ctx.dragged_id 为准(释放帧起算)、浮标/菜单行双 sense 压拖选榜、菜单外按下即合拢+失焦即隐靠 egui 失焦塌缩选区的结构性语义、交付面=源码+Live(**2026-10-15 订正**:分栏档右栏是**只读**的 Live 实例,无选区;vendored 标签层 app 读不到选区,故浮标在右栏不出现 —— 这是只读的应有之义,不是缺口)(2026-10-06,#61 selection-ai-bar M1·自动拍板)

- **岔路**:①浮标形态(任务书「浮出 AI 小标识;点开两动作(续写/润色,形态自选记录)」——单 chip 点开菜单 vs chip 直接并排两钮);②「拖拽进行中不弹(以指针释放帧起算,判定口径自选记录)」的机制口径(指针事件序列状态机 vs egui 拖拽状态);③命中区放同层末尾的 sense 选择(纯 click vs click_and_drag)——egui 0.36 hit_test 按 click/drag **分榜**裁决,单 click 的浮标赢了 click 榜仍把 drag 榜留给下面的 TextEdit;④菜单关闭时机(点动作即关/仅手动关)与「失焦即隐」的实现依赖(自己盯焦点 vs egui 失焦塌缩选区的既有语义);⑤交付面:右栏纯预览富渲染的文本选择(egui `LabelSelectionState`)能不能同享浮标(任务书点名「核验其无文本选择能力(现状),如实记录不伪造」)。
- **备选**:①a 28×17 单 chip(「AI」)+点开两行菜单;①b chip 右侧直接并排「续写/润色」两钮(无二级)。②a `ctx.dragged_id()`(排除浮标自身)为准,释放帧 egui 已清空 dragged_id、浮标当帧即出;②b 自记「按下→移动→抬起」三态指针状态机。③a 双 sense(click_and_drag);③b 纯 click(实测按浮标塌缩选区,不可行);③c Foreground Area(13a 纪律禁止,不列入选型)。④a 菜单外按下即合拢+动作即关+浮标隐藏连带合拢;④b 仅显式关闭。⑤a 只做源码+Live,右栏如实登记不可达;⑤b 给 vendored label 的选区加偏移外泄(vendor ①类改造,越本模块授权)。
- **自动选择**:①a+②a+③a+④a+⑤a。**①**:选区尾端空间金贵(chip 右侧常贴正文/行尾,并排两钮 ~90px 宽会把「浮标」变「浮条」盖住下文),单 chip 只有 28px、两动作藏进二级,菜单行还天然容纳将来动作;点开的发现性由 chip 本身保证。**②**:egui 拖选期间 `dragged_id` 恒为 TextEdit(被拖的 widget),**释放帧** egui 已清空 —— 判定 `dragged_id`(排除浮标自身,见③)即「拖拽中不弹、释放帧起算」,零自维护簿记;②b 要自己处理阈值/相位,重造 egui 已算好的事实,且拖拽被打断(滚轮/切窗)会留幽灵状态。**③**:egui 0.36 hit_test.rs 对「click 榜命中 + drag 榜命中」**双双报告**(「Report both hits, e.g. the top Button and the ScrollArea behind it」,hit_test.rs:364-390)——纯 click 的浮标赢 click 榜、drag 榜仍归 TextEdit,按在浮标上的一下经拖选路径把选区塌缩到按点,浮标当帧自隐、菜单永远开不出来(无头三帧序列实测复现后才定位到);双 sense 让浮标/菜单行在后注册的前提下**两榜都居上**,TextEdit 分毫不沾,代价仅是「从浮标起手拖不出选区」(浮标 28×17,与一切工具栏按钮同款边界)。**④**:菜单外按下即合拢(点在选区高亮内不塌缩的位置由「浮标/菜单矩形之外按下」兜住);动作点选即关+发消息;「失焦即隐」**不自己盯焦点**——egui 0.36 对失焦 TextEdit 每帧把持久选区塌缩回光标(builder.rs `!owns_ime_events` 分支,`owns_ime_events == has_focus`),选区清空浮标自隐,「失焦/切标签即隐」是结构性的,重聚焦不凭空恢复(需重新选中),测试按此语义钉死。**⑤**:右栏纯预览的选区活在 vendored `MarkdownLabel` 内部的 `LabelSelectionState`(egui 文本选择插件,per-viewport 全局态),app 侧既读不到选区偏移也拿不到尾端 galley 矩形——选区数据不过 app 层,M1 浮标无从锚定;如实登记「交付面=源码+Live」。
- **理由**:①「小标识」的语义就是最小占用,两动作的发现性由点开后的菜单行保证(VS Code/GitHub 的选中浮条也是「点开才见动作」);②机制正确性来自 egui 的事实而非自己的簿记;③单 click 版本在无头三帧序列里实测必挂(按下帧探针即空),不是观感取舍是功能正确性;13a 纪律(同层末尾、不建 Foreground Area)全程守住——命中区在 ScrollArea 之后的同一层 `Ui::interact` 注册,矩形外的打字/拖选/点击逐字节不变;④「隐」由单一事实(选区没了)驱动一组连带(浮标隐→菜单合拢),不维护第二份开关;⑤任务书写明「不可达如实记录不伪造」,⑤a 是唯一诚实选项;将来要做须按 vendor ①类纪律(给 `LabelSelectionState` 外泄选区范围+尾端矩形)单独立项走变更表,不混入本模块。
- **如何改**:①要并排两钮——`ui/selection_ai.rs::show` 的菜单分支改为常开两钮横排(删 menu_open temp 与 `menu_open_id`),probe.menu 语义同步;②要指针状态机口径——`badge_visible` 第三参改由调用点喂自记判定;③要纯 click——`show` 两处 `Sense::click_and_drag()` 改回 `Sense::click()` 并删 `is_floater_id`(选区塌缩回归、按浮标即隐,不建议);④要仅手动关——删 `show` 的「浮标/菜单之外按下即合拢」分支;⑤要右栏同享——vendor ①类:给 `LabelSelectionState` 加选区外泄(读侧偏移+尾端 rect),右栏调用点复用 `selection_ai::show`,按 vendor/README.md 变更表登记。

## #114 Ctrl+H 替换键的 mac 改排:mac 出厂 ⌘H→⌥⌘F(⌘H 被 winit 默认菜单 Hide 项持有,是死键+隐藏窗口副作用)、Win/Linux 保持 Ctrl+H、老档 Cmd+H 值感知迁移(#45 K1 同构无需平台 cfg 门)、⌘G 复核维持 #113、替换条全模式核验五场景结论(2026-10-06,#60 goto-line M2·自动拍板)

- **岔路**:①M2 键位核验点名的 mac ⌘H 冲突——macOS 系统级「隐藏窗口」占 ⌘H,出厂表 `ReplaceInDoc = COMMAND+H` 在 mac 是留是改;②改的话 mac 侧新键选型(⌥⌘F vs ⌘R vs 裸 Ctrl+H vs 全平台改键);③老档迁移范围(已在 keymap.json 里躺着 Cmd+H 的 mac 老用户迁不迁);④Win/Linux 是否跟着动;⑤Ctrl+G 的 mac 复核(⌘G 是否同样被系统持有)。
- **备选**:①a 保留 ⌘H(mac 上菜单点击仍可达替换条,键位成哑弹);①b mac 改键。②a ⌥⌘F(VS Code / Sublime 的 mac 替换键);②b ⌘R(IntelliJ mac 惯例,但 ⌘R 更广的联想是「刷新/运行」);②c 裸 Ctrl+H(mac 上 Ctrl 与 Cmd 是不同修饰键,系统不拦);②d 全平台改键保统一。③a 值感知迁移(只迁「值==旧默认」条目,#45 K1 先例);③b 不迁(老用户手点设置页重置)。④a Win/Linux 保持 Ctrl+H 不动;④b 全平台统一新键。⑤a 维持 ⌘G;⑤b 让位。
- **自动选择**:①b+②a+③a+④a+⑤a。**①**:实测取证 winit 0.30.13(本仓锁定的传递依赖)在 macOS 无条件构建默认应用菜单,`platform_impl/macos/menu.rs:34-42` 的 `Hide <app>` 项 keyEquivalent = `h`+⌘(`sel!(hide:)`),`app_state.rs:139-142` 在事件循环启动时调 `menu::initialize`,`event_loop.rs:212` 的 `default_menu` 默认 `true` 且 eframe 0.36.2 / egui-winit 0.36.2 全程未调 `with_default_menu(false)`——Cocoa `performKeyEquivalent` 主菜单匹配先于应用事件,⌘H 按键**到不了 egui**,`ReplaceInDoc` 在 mac 是死键且副作用是整个窗口消失(用户以为替换条没开,应用没了),这不是「惯例冲突」而是功能性死键,保留不可辩护。**②**:⌥⌘F——与 ⌘F 查找同族(替换是查找的扩展,只多一个 Alt),VS Code/Sublime 双先例,出厂表无占用者(`Keymap::builtin().conflict` 断言),且 winit 默认菜单的 `Hide Others` 是 ⌥⌘H 不是 ⌥⌘F,不撞系统;⌘R 让位给更广的「刷新」联想,裸 Ctrl+H 在 mac 菜单里显示形态别扭(Ctrl 与 ⌘ 混排),全平台改键是替 mac 一家付费。**③**:值感知迁移,#45 K1(主题键 ⌘⇧T→⌥T)同构:只迁「解析值==旧默认 Cmd+H」的条目到新默认且写回落盘,用户真自定义(别的键)与主动清除(空串/缺项)分毫不动。**④**:Win/Linux 新默认==旧默认(Ctrl+H),迁移条件 `new != retired` 天然不命中,出厂值落盘的老档分毫不动——所以迁移代码**不需要平台 cfg 门**,两条 CI 都能实测迁移本体(比 #45 K1 多出的好处:那次只能靠逻辑推「其余平台不受影响」)。**⑤**:⌘G 维持 #113——winit 默认菜单只有 About/Services/Hide/Hide Others/Show All/Quit,无 G 键项,⌘G 直达 egui,#113 的「空闲键」判断经 winit 取证复核成立。**核验矩阵五场景**(每场景一条非恒真断言或既有测试复验,全部通过):源码基线=#17 既有测试全绿(82 处 find/replace 相关零削弱);Live 块编辑=替换走**整篇 rope**(`replace_all`/`replace_range`,归约 `replace_all_in_doc`/`replace_current` 同一对写路径),rev 推进后下一帧 `sync` 重切块+活动块重定位,无第二份缓冲,不产 `pending_caret`(`live::tests::replace_writes_whole_rope_and_next_sync_rechunks` 新钉,undo 单步回滚由既有 `undo_after_replace_all_reverts_whole_operation_in_one_step` 复验);禅定=替换条状态翻到位但浮层只画三栏源码路径,Esc 退出后同帧落回,是「延后」不是「被吞」,13a 不适用(`ctrl_h_in_zen_flips_state_and_overlay_waits_for_three_pane` 新钉);多标签=切标签/关标签即按新文档重扫(`scanned_for=(tab.id,rev)` 见证),既有 `switching_tabs_rescans_find_hits_for_new_document`/`closing_tab_rescans_find_hits_for_new_document` 复验;minimap=出厂默认即开态,查找卡浮层与 minimap 两态共存、正文顶边不变(`find_overlay_coexists_with_minimap` 新钉)。
- **理由**:①见上——死键+隐藏窗口的双重负收益,菜单点击入口救不了键盘用户,且菜单里显示 ⌘H 等于教用户按一个会隐藏应用的键。②同族键优先:⌘F 查找/⌥⌘F 替换的记忆成本最低,VS Code+Sublime 两家编辑器的 mac 惯例背书。③「当年落盘的出厂值」不是用户手笔(#45 K1 定过的口径),不迁则 mac 老用户永远按哑键;迁移条件按解析值识别,别名/大小写变体同样命中。④见「自动选择 ④」——条件设计让平台正确性来自逻辑而非编译开关,可测性更好。⑤winit 默认菜单源码是本次核验的第一手证据,#113 当时凭「出厂表 G 无占用者」推断,现在补上了系统层证据。
- **如何改**:①/②要 mac 回 ⌘H 或换别的键——`command.rs::default_shortcut` 的 `ReplaceInDoc` mac 分支改/删,同步 `replace_shortcut_fires_replace_command` 与 `replace_in_doc_default_and_incremental_migration` 的平台断言,并删 `keymap.rs::load_from` 的 replace 迁移段与 `replace_default_migration_on_load`;④要全平台统一新键——同上且把迁移条件的 `new != retired` 门保持(非 mac 也会开始迁移);③要不迁移——删 `load_from` 的 replace 迁移段(mac 老用户需手点「全部恢复默认」)。


## #113 Ctrl+G 跳转行的七处口径:出厂键位全平台 Ctrl/Cmd+G(mac ⌘G 与「查找下一个」惯例的取舍)、浮条照 #17 查找卡同款、非数字回车取忽略不取提示、与查找条互斥、跳成即关+焦点钉、Live 经 jump_to→pending_caret 可达、浮条只画三栏路径(2026-10-06,#60 goto-line M1·自动拍板)

- **岔路**:①任务书点名核验的 mac 键位取舍——⌘G 在 mac 惯例是「查找下一个」,出厂绑定 `Modifiers::COMMAND+G` 在 mac 显示 ⌘G,是否要让位;②浮条形态(照 #17 替换条/查找卡 vs 模态对话框 vs 状态栏内联输入);③非数字输入容错「忽略 vs 提示」任务书留自选;④替换条与跳转浮条「互斥 vs 共存」任务书留自选;⑤跳成后浮条关不关、焦点怎么给;⑥Live 模式可达性实测;⑦禅定模式下 Ctrl+G 的浮条画不画。
- **备选**:①a 全平台 Ctrl/Cmd+G;①b mac 让位 ⌘G 给「查找下一个」、跳转改裸 Ctrl+G(VS Code mac 版做法)。②a 照 #17 查找卡同款 Window;②b 模态对话框;②c 状态栏内联。③a 忽略(不发消息浮条保留);③b 落 `document.notice` 提示。④a 互斥(一侧开收另一侧);④b 共存(浮条挪锚点错开)。⑤a 跳成即关+输入清空+焦点钉;⑤b 跳成留浮条可连续跳。
- **自动选择**:①a+②a+③a+④a+⑤a+「Live 可达(既有通道,新增钉住测试)」+「浮条只画三栏路径」。**①**:出厂表 G 键无占用者(`command.rs::default_shortcut` 全表核查,`Keymap::builtin().conflict` 测试断言);本仓查找条的「下一个」是查找框内 Enter(#17),没有独立 find-next 键位,⌘G 在 mac 上是空闲键,不构成现实冲突;VS Code mac 的 Ctrl+G 是给 ⌘G 让位的无奈,本仓没这个债,统一平台键位(仓库 `Modifiers::COMMAND` 抽象的既有口径)优先。**②**:照 #17 查找卡同款 `egui::Window`(无标题栏/不可拖/`fixed_pos` 右上锚点/`Frame::popup`),源码与 Live 两模式都画。**③**:空/非数字/溢出 usize 的输入,回车与「跳转」钮都不发消息,浮条保留可改。**④**:互斥——Ctrl+G 开浮条收查找条(含替换行,同 Esc 关整条的清理:open/replace_open/hit 三清),Ctrl+F/Ctrl+H 开查找条收浮条(`toggle_find`/`toggle_goto` 双向写)。**⑤**:跳成即关、草稿清空;浮条开着就持焦(quick_open 的焦点钉同款),Ctrl+G 后直接打数字回车。**⑥**:可达——归约只写 `cursor.jump_to`,`live::ui` 既有入口当帧切块 + `pending_caret` 路由(通道本身由 `live_jump_to_routes_caret_and_scrolls_into_view` 证毕,新增 `goto_jump_in_live_mode_routes_block_caret` 钉住 goto 全链路);空文档 Live 侧无可落点,`jump_to` 已 take 即算消费(live.rs 既有文档化行为,源码侧空文档钳第 1 行行首)。**⑦**:浮条只画三栏路径,#17 查找卡同款先例(禅定帧 Ctrl+G 翻状态、退出禅定后浮条可见);13a 教训不涉——本浮条是 Window 浮层(egui 管理的 Area,与查找卡/commit 浮窗同类),不是边缘感应区,无「Foreground Area 跨层屏蔽吞点击」风险。
- **理由**:①mac「查找下一个」惯例的前提是有 find-next 命令占着 ⌘G,本仓没有,让位是替不存在的冲突付费;真发生冲突时 keymap 设置页可改绑(撞键拒绝会报出占用者),自救通道现成。②替换条形态已被 #17 验收过(锚定/浮层语义/无头测试基建全套现成),对话框与状态栏内联都是第二套观感。③输入框就在眼前,回车没反应天然可自我纠正;提示行要走「知道了」反而打断输入节奏;溢出数字解析失败与非法字符同一分支,无需单独文案。④两者同锚右上角,同帧在场必重叠;VS Code 同位置 widget 也是替换式;互斥比错位共存少一份布局状态。⑤连续跳行的需求被「重新 Ctrl+G」覆盖(键位就在手上),留浮条反而多一个 Esc 步骤。⑦禅定是预览态(本就不画编辑器),浮条在禅定里没有可跳的行号槽参照,等价于查找条在禅定的现状。
- **如何改**:①要 mac 让位——`command.rs::default_shortcut` 的 `GotoLine` 改 `Modifiers::NONE+Key::G` 之外的组合(如 `Modifiers::CTRL`(裸 Ctrl)或改绑别的键),同步 `goto_line_shortcut_fires_goto_command` 断言;③要提示——`goto_bar_contents` 解析失败分支改推 `Message::NoticeDismissed` 之外的新提示消息;④要共存——删 `toggle_find`/`toggle_goto` 里的互斥三行、浮条锚点挪 `source_rect` 左下角,共存断言反向改;⑤要连续跳——`State::goto_line` 删掉关闭两行;⑦要禅定同达——把 `draw_goto_overlay` 挂进 `draw_zen`(需在禅定路径先算出预览区 rect,照 `draw_find_overlay` 的 source_rect 机制补)。

## #112 M2 的 vendor: commit(4a8f79c)粒度违规的修复路线:两个独立 ①类主题合收一枚且混入 app 三件——收官推送前 rebase 拆分 vs 变更表登记+按主题提取清单(2026-10-06,#59 perf-round 评审修复·自动拍板)

- **岔路**:独立评审(medium)指出 4a8f79c 违反两条口径:①「不与 app 混装」——`vendor:` commit 内含 crates/latermd-app 三件(ui/perf_finding.rs +12、ui/preview.rs +6、ui/tab_switch_perf.rs +8;内容本身正当,均为视口外高亮延迟的接线:预览 label 开启 + 两 harness 对齐生产配置);②AGENTS §6.9「按主题拆 commit」——「块表帧内一次收集化」(#99 销账)与「视口外高亮延迟」(#60 可安全子集)是两个互不依赖的 ①类主题,合收一枚后整枚无法 cherry-pick 到上游 fork(需手工剥 crates/ 路径),两修绑死回滚与提 PR 粒度。perf-recheck §11 头注本就写明「独立 `vendor:` commit 由编排收口」(两笔),偏差发生在编排收口环节而非模块交付;且 #42 行(61d7903)同类混装刚于 2026-10-03 以重做路径拆净(85cdf93+1b46f88),本轮属复发。修复路线两岔。
- **备选**:A 编排层在收官推送/开 PR 前对 feature/perf-round 交互式 rebase,把 4a8f79c 拆成三枚:`vendor:`(块表一次收集化,按变更表提取清单)+ `vendor:`(高亮延迟,余下 vendored 全部)+ `perf(app):`(app 三件接线),完成后变更表两行 commit 列回填新 hash;B commit 原样保留,vendor/README.md 变更表两行回填 4a8f79c、登记违规、按主题登记上游回馈提取清单。
- **自动选择**:B(文档收口)。选 A 的窗口仍向编排层开放(分支未推送,「收官推送与 PR 合入由编排统一执行」之前随时可 rebase),见「如何改」。附带把变更表两处同类收尾欠账一并闭环(#42 行 61d7903→85cdf93、#43 M2 行 f8ce3b2→031fc6f——两笔拆分当日均已按重做路径完成且进 origin/main,仅登记行漏回填 hash 与过期「待拆出」注记未清)。
- **理由**:①本修复模块受公共约束「不执行 git commit/push/checkout」,rebase 拆分超出其权限,硬做即违约束;②上游回馈的既定工作流(vendor/README §向上游回馈)是 `git diff <①类基点> -- vendor/egui_markdown` 导出补丁 + `git apply -p2`,整枚 cherry-pick 从来不是必要通道——混装的真实代价收敛为「提取时按变更表清单剥 hunk」,而非「无法回馈」;③两主题内容、测试与复测数字均已定稿(perf-recheck §11/§12),收官挤压中做 history 手术一旦出错(丢 hunk/冲突)风险大于收益;④#42 先例证明拆分必须在进 main 前完成——PR 一经开出拆分窗口即关闭,届时不论意愿都以 B 为终态。
- **如何改**:改选 A——编排层在推送/开 PR 前对 4a8f79c 执行 rebase edit:先按 vendor/README.md 变更表「块表帧内一次收集化」行的提取清单提交 `vendor:`(该主题),再按「视口外代码块高亮延迟」行的提取清单提交 `vendor:`(余下 vendored 全部),最后 app 三件提交 `perf(app):`(对齐 5508a8e 风格);完成后把变更表两行 commit 列改填新 hash 并在本条追加「已按 A 执行」注记(或回删)。维持 B——本条保留作为违规记录与提取清单宿主,不回删。

## #111 冷首切残余 143 ms 的收口路线:估计高度型视口外延迟布局(已拍板 B 并落地,本条销账转注记)(2026-10-06 登记;2026-10-16 坤哥拍板备选 B,lazy-offscreen-layout 落地)

- **岔路**:#60 挂账的「20000 行冷首切 847–927 ms」在本轮已按其路线的**可安全子集**收口大头——视口外代码块高亮延迟(`defer_offscreen_highlight`,vendor ①类,几何/文本零变化,无高度估计):20k 冷首帧 855.02→143.42 ms(6.0×)、2k 85.22→18.57 ms(达标 <20 ms),见 perf-recheck §11.2。但 §10.6 写明的验收「20k ≥8× 即 <110 ms」**未达**:剩余 135.7 ms = miss 帧对全文档(~645 KB)的全量 shaping(`Fonts::layout_job`)——shaping 本身就是精确测高,要跳过它只能用**估计高度**替代(文本行数/字宽折算、代码 fence 行数×行高、表格行数×行高),这正是 #60 备选 B 的完整形态,当年(2026-09-30)与 §10.4 都点名「估计高度失准 → 布局塌陷/滚动条跳动,需首刷占位+后帧校正」。现在的问题是:**这个 ~140 ms 的一次性卡顿要不要用估计高度的复杂度+风险去换**。
- **备选**:A 接受现状(143 ms 一次性,已 6×,2k 已达标;估计高度方案登记为「不做」并回删本条);B 做完整延迟布局:miss 帧只 shape 视口内段,屏外段按 token 类型估高占位(文本段 `字符数×平均字宽/可用宽` 折行估算、fence 行数×代码行高、表格行数×行高),后帧滚入时精确化并校正——需要 ①vendor 估高器(按 token 类型)②size 缓存带「估计/实测」双态 ③滚动锚定(app 侧按内容偏移补偿总高变化,涉 #78/#79/#80 的滚动语义)④块表/锚点对估计段几何的语义修订(现契约=当帧真实几何);C 折中:仅对**代码 fence 单 token 段**(估高最准:行数×行高,monospace 无折行误差)做延迟 shaping,文本段保持全量——预计把 136 ms 压到 ~40–60 ms(20k 样本 fence 段占 shaping 主体),风险面收窄到 admitted fence 一类。
- **自动选择**:不选,挂账待拍板(本条目即登记)。理由:①三案的 UX 取舍(一次性 143 ms vs 滚动中内容跳动 vs 折中精度)是产品手感问题,无头测试测不出来,不该由实现棒代答;②B/C 都动「块表当帧真实几何」契约(#42/#78 消费方是大纲跳预览),需连同消费者一起评估(#99 原条目同款理由);③两轮修复预算已用于高亮延迟(6×)+块表销账,再扩面属 §10.6 明文的「独立拍板」范围。
- **理由(现状评估依据)**:143 ms 是**单文档首次打开/切换**的一次性成本(缓存热后稳态 3.3 ms),非每帧;22k token 下的场景频率取决于用户开大文档的习惯;M1 取证(§10.5)排序里它本就是第一热点,但「第一热点」≠「必须压到 0」——6× 后已低于 §10.5 表中其余所有热点的绝对量级。
- **如何改**:选 A——回删本条并在 #60 加「143 ms 现状为最终水位」注记;选 B/C——授权 vendor ①类(估高器+双态 size 缓存)+ app 侧(滚动锚定),按 #60「如何改」同款流程独立 commit + 变更表登记;C 先行的话 B 的文本段部分留待其后。验收口径沿用 §10.6:20k 冷首切 <110 ms(估高方案理论可到 ~20–30 ms),且滚动手感(无塌陷/无跳动)须真机目视验收(无头测不出)。

> **销账注记(2026-10-16,选 B 执行)**:坤哥拍板备选 B,当轮落地(vendored ①类 `estimate.rs` 估高器 + app 侧 live 块级视口外剔除占位 + `estimate_drift` 滚动锚定 + 屏外跳转落点随实测自校;同轮收口 2026-10-16 上午取证的「拖栏宽卡顿」)。前后数字与机制全文见 perf-recheck §13:**拖宽帧 2000 行 14.32→0.68 ms、20000 行 159.23→4.72 ms(34×,稳态同带)**;live 整帧冷首切 20000 行 216→89 ms(**2.4×,§10.6 的 <110 ms 首次达标**)。无头哨兵:剔除(命中表存在性双向)+ 锚定(稳态 offset 漂移 <0.5px)双绿,1081 全绿零回归。**真机手感项(滚动条长度一次性软变化、无内容跳动)无头测不出,留真机目视验收** —— 这是备选 B 当年登记风险的收尾,未验收前不视为完全闭合。方案落地形态与本条原文的偏差:①估高器不区分「估计/实测双态 size 缓存」——占位高直接以估高给出,实测高另行记录仅用于锚定差值,双态合并为「上一帧实测优先、无则估高」的单链;②块表契约未修订 —— 命中表只由真实渲染臂建立(新增 `block_span_rects_present` 探针),屏外块无表,消费方(#42/#78 大纲跳预览)语义不变:跳转目标经 `scroll_to_rect` 滚入视口后当帧转真实渲染,几何当帧精确。

## #110 #58 M3 上下文大小配置的五处口径:口径取纯字节 KB 上限非 token 近似、0=未配置落回 32KB/16KB 现状默认(否决线:未配置截断行为不变)、显式值一个预算统一摘要与 diff 两用途、截断说明进 prompt 且 KB 数随预算如实、范围 0..=1024KB 钳制+滑杆恒可编辑(2026-10-06,#58 ai-config-slim M3·自动拍板)

- **岔路**:①口径自选:「按字符/字节近似 token(1 token≈N 字符,CJK 加权)」还是「纯字节上限」,换算依据要写清;②`AiConfig` 新增的单一配置项如何驱动两处**不同**现状默认(摘要文档 32KB / commit diff 16KB):0=跟随现状、单值默认直接覆盖(如 32KB,diff 跟着变 32KB)、还是拆两个字段;③「已截断」提示的通道:进 prompt(现状口径)还是补 UI 提示;④取值范围、单位粒度与 UI 形态(滑杆上下限、是否随 provider 灰显)。
- **备选**:①a 纯字节(KB)上限;①b token 近似(字符计数 + CJK 加权系数换算成字节预算)。②a `context_kb: u32` 默认 0=未配置,`context_budget()` 给 None、latermd-ai `unwrap_or` 落回各自常量;②b 默认 32KB 直接覆盖两用途(diff 默认从 16KB 变 32KB);②c 拆 `summary_context_kb`/`diff_context_kb` 两字段。③a 提示进 prompt,数字随预算变;③b 截断时落 `document.notice`;③c 两者都做。④a 0..=1024KB、KB 粒度、egui 滑杆、恒可编辑(Mock 也生效);④b 上限 2048/4096;④c 随 `uses_settings()` 灰显。
- **自动选择**:①a + ②a + ③a + ④a。`AiConfig` 新增 `context_kb: u32`(0=未配置)+ `CONTEXT_KB_MAX=1024` 常量,`normalize()` 与 `context_budget()` 双侧钳上限;`context_budget()` 把 KB 换算成字节(`kb×1024`,`None`=未配置),`state.rs` 摘要/commit 两调用点透传给 latermd-ai 新增的 `summary_prompt_with_budget`/`commit_message_prompt_with_budget`(`Option<usize>`,`None`=`unwrap_or` 落回 32KB/16KB 现状常量,旧函数变薄包装且行为逐字节不变);截断说明从写死的「约 32KB」改为按真实预算格式化(默认下输出与旧文案逐字节相同);设置页 AI 页加「上下文大小(KB)」滑杆(0..=1024,0=跟随默认,悬停/说明行给换算依据「约 1KB ≈ 250-350 token」),不随 provider 灰显。换算依据:模型上下文窗口以 token 计,英文约 4 字节/token(ASCII 1B/字符、~4 字符/token),UTF-8 常用汉字 3B/字、约 1-1.5 字/token 即 ~2-3B/token,故 1KB ≈ 250-350 token;字节是 UTF-8 的精确物理量,取字节口径时该换算只用于给用户的心智参照,不参与截断计算。
- **理由**:①token 近似没有 tokenizer 支撑,CJK 加权系数本身是猜测,把估算值当「精确 token 数」配置给用户反而误导;字节口径下现状 32KB/16KB 本就是字节,默认零迁移、`truncate_bytes` 的 char 边界安全原样继承(emoji 4 字节同款回退);②「未配置时截断行为不变」是否决线,②a 把它机制化:旧档缺字段 → serde default 0 → `None` → 现状常量,链路上没有任何一处能让默认行为漂移,单测双向钉住(`prompt_with_budget_none_or_default_matches_legacy` ×2 + 旧档缺字段回落);显式值一个预算统一两用途符合用户心智(「我的模型能吃下多少」不分用途),②b 会让 diff 默认从 16KB 翻倍(顺手改现状行为,越授权),②c 两字段把一个滑杆能说清的事变两处;③截断发生在 prompt 组装层(app 发起前),注记进 prompt 是现状口径且模型可见(模型知道输入不完整,如实),数字随预算变才不撒谎;UI notice 是新增表面积,截断无害(结果照常产出),不因「可以提示」就提示;④1MB ≈ 至多 ~50 万 token,已数倍于主流 128K 窗口,更大只会把请求打爆;KB 粒度与现状常量同单位零换算歧义;Mock 的摘要/commit 走同一 prompt 组装,灰显会造成「Mock 下配了不生效」的误导;滑杆+范围钳制照 #23 先例(出厂值=不改变现状)。
- **如何改**:①要 token 口径,在 latermd-ai 加 `estimate_tokens(text)` 纯函数(ASCII/CJK/emoji 加权表),`context_budget` 改为「token 预算 ÷ 加权字节比」出字节,或直接把 `*_with_budget` 参数改 token 数(两处现状常量同步换算);②要两字段,`AiConfig` 拆两键、`context_budget` 拆两个读取口、state.rs 两调用点分别取值、设置页两根滑杆;③要 UI 提示,让 `*_with_budget` 返回 `(String, bool)` 外泄截断标志,`request_summary`/`request_commit_message` 据此落 `document.notice`(commit 侧注意与建议浮窗同帧落);④要调范围/默认,改 `CONTEXT_KB_MAX` 与滑杆范围(读入 `normalize`、读取 `context_budget`、滑杆三处同源,漏一处则手改 JSON 可越界)。


## #109 #58 M2 模型列表自动获取的六处口径:OpenAI 兼容列表端点拼 {base}/models(base_url 语义已含版本段,不重复拼 /v1)、解析容错取「顶层坏=失败/条目坏=跳过/保序去重不排序」、Mock 按钮禁用+悬停说明、手输名不在候选保留不改写回显「(手输)」、拉取中重入取忽略(非替换)、失败保留旧候选(2026-10-06,#58 ai-config-slim M2·自动拍板)

- **岔路**:①任务书给 OpenAI 兼容写的是 `GET {base}/v1/models`,但仓库 `OpenAiSettings` 的 base_url 语义**已含版本段**(出厂 `https://api.openai.com/v1`,请求走 `{base}/chat/completions`;用户实测配置如 DeepSeek `.../v1`、智谱 `.../paas/v4` 同构)——列表端点照抄任务字面拼 `{base}/v1/models` 还是沿仓库口径拼 `{base}/models`;②响应解析的容错粒度:响应体畸形(坏 JSON/`data` 缺失/非数组)与条目级坏数据(id 非字符串/空白)怎么分,列表排序与否;③Mock 不获取,按钮「禁用+悬停说明」还是「隐藏」;④拉取成功但手输的模型名不在列表里:保留手输名、强制回填、还是清空;⑤拉取中再次发起:忽略(防重入)还是新请求替换旧请求(bed 测试上传的 seq 口径);⑥拉取失败时已获取的旧候选列表清不清。
- **备选**:①a 沿仓库口径 `{base}/models`(Anthropic/Ollama 的 base 无版本段,分别拼 `/v1/models`、`/api/tags` 不受影响);①b 照任务字面拼 `{base}/v1/models`(会拼出 `/v1/v1/models`,与聊天请求的 url 组装互相矛盾)。②a 顶层结构坏=失败文案、条目坏=跳过、保序去重不排序;②b 条目坏即整体失败;②c 按字母排序。③a 禁用+悬停说明(「Mock 不联网,无需获取模型列表」);③b 隐藏。④a 保留手输名,下拉回显带「(手输)」标记,点选才覆盖;④b 成功后回填候选第一个;④c 清空等用户重选。⑤a 在途忽略(UI 禁用按钮 + 归约防御,同 `AiState` 防重入);⑤b 新请求替换旧请求(bed 的 seq 手法)。⑥a 失败保留旧候选只落错误行;⑥b 失败清空候选。
- **自动选择**:①a + ②a + ③a + ④a + ⑤a + ⑥a。①端点拼装收在 `latermd-ai::models` 的 `ModelsSource::endpoint()`,三家各自沿 adapter `url()` 的 base 语义(OpenAI 兼容 `{base}/models`、Anthropic `{base}/v1/models` + `x-api-key`/`anthropic-version`、Ollama `{base}/api/tags` 无鉴权),请求方法 GET;②解析三纯函数(`parse_openai_models`/`parse_anthropic_models`/`parse_ollama_models`)返回 `Option<Vec<String>>`:`None`=顶层结构坏(报「响应不是预期的模型列表结构」),`Some(vec![])`=合法空列表,条目坏跳过,保序去重不排序(服务端顺序常是推荐序);③按钮恒渲染,`add_enabled` 分「拉取中」与「Mock」两种禁用原因给不同悬停文案;④候选下拉与手输框并存,`known` 时回显名字本身、否则「{名}(手输)」、空草稿回显「已获取 N 个模型,点选填入」;⑤`ModelListState::start` 在途忽略,接收端不中途替换(与 bed 的差异:bed 每 profile 一行按钮无法全局禁用才走 seq 替换,此处单按钮两道防线同语义);⑥`finish(Err)` 只落 `error`,旧 `options` 原样。
- **理由**:①任务字面的 `{base}/v1/models` 以裸域名为 base,与仓库 base_url 已含版本段的既定语义相抵 —— 同一配置(DeepSeek `.../v1`)聊天请求拼 `/chat/completions` 正确、列表请求拼 `/v1/v1/models` 必错,以现状为准(任务书自述「与仓库现状冲突以现状为准」);②下拉是「尽力而为的候选」,一个坏条目废掉整份列表比跳过它伤害大;顶层结构坏必须与空列表区分,否则空 key 的端点返回 `{}` 会被误报成「没有模型」;保序是零成本的忠实,排序反而打乱 provider 的推荐序;③隐藏让用户不知道有这功能,禁用+说明与 bed 测试上传同款,可发现性优先;④手输名是用户的显式输入,拉取是候选补充不是覆写授权 —— 强制回填会静默丢用户输入(与 `adopt_provider_defaults`「手改过的字段绝不静默覆盖」同一价值观);⑤单按钮场景 UI 禁用已是完整防线,归约同语义忽略即可,seq 替换是为「多个按钮各自可点」的 bed 场景设计的,照搬是无机制收益的复杂度;⑥失败时旧列表仍然正确(它来自上次成功拉取),清空只会让用户在排查错误的同时丢失可用候选。
- **如何改**:①要按任务字面拼版本段,改 `latermd-ai/src/models.rs` `ModelsSource::endpoint()` 的 OpenAI 兼容分支为 `{base}/v1/models`(注意先同步放宽既有 `endpoints_follow_adapter_base_url_semantics` 断言);②要条目坏即失败,改 `collect_model_ids` 把 `continue` 改 `return None`;要排序,在三个 parse 函数出口加 `ids.sort()`;③要隐藏 Mock 按钮而非禁用,把 `ai_page` 里 `add_enabled(editable && !fetching, …)` 一段改为按 `editable` 分支;④要拉取成功后回填/清空手输名,在 `Message::AiModelsFetchFinished` 的 `Ok` 分支(`ModelListState::finish`)加对 `settings.ai_draft.model` 的写回(注意 finish 在 `settings.models` 上,需要归约层改);⑤要在途新请求替换旧请求,删 `ModelListState::start` 的 `rx.is_some()` 早退并仿 `BedState` 加 seq 校验(消息需补序号载荷);⑥要失败清空候选,把 `finish` 的 `Err` 分支加 `self.options.clear()`。

## #108 #58 配置页参数精简的四处口径:被删参数内部默认钉住改前装配值(0.7/1.0/2048,Anthropic/Ollama 也是发而不是停发)、兼容策略取删字段+忽略未知键、#35 滑杆几何探针随被测控件移除而删除、「当前生效」行去掉流式形态(2026-10-06,#58 ai-config-slim M1·自动拍板)

- **岔路**:①Temperature/Top-P/Max tokens 从 UI 与 `AiConfig` 删除后,`set_provider` 装配请求要给被删参数什么值——逐字复刻**删除前** `AiConfig::default()` 的装配(Anthropic/Ollama 也发 Some(0.7)/Some(1.0)/2048),还是落到各 adapter 自家 `Default`(Anthropic/Ollama 是 None 不发,decisions-pending #92/#93);②旧 `ai.json` 含被删键(temperature/top_p/max_tokens/system_prompt/timeout_secs/stream)的兼容策略——结构体删字段+serde 忽略未知键、保留字段仅删 UI、还是 `#[serde(alias)]` 折中;③#35 的「AI 页四根滑杆两档密度几何对齐」无头探针,被测的四根滑杆本身已删,测试怎么办;④设置页「当前生效」行原本带流式/整段形态,字段删了文案怎么办。
- **备选**:①a 复刻改前装配(0.7/1.0/2048,三家一致);①b 落 adapter Default(OpenAI 恰好同为 0.7/1.0/2048,Anthropic/Ollama 变 None);②a 删字段+忽略未知键;②b 字段保留只删 UI;②c alias 保读取。③a 随控件删除测试;③b 改写为「滑杆绝迹」断言保留测试名;③c 删控件留测试(必红,不可行)。④a 删掉该段只报 provider·模型;④b 保留固定「流式」文案。
- **自动选择**:①a + ②a + ③a + ④a。①在 `ai.rs` 顶层钉 `INTERNAL_TEMPERATURE=0.7`/`INTERNAL_TOP_P=1.0`/`INTERNAL_MAX_TOKENS=2048` 三常量,三家装配显式给值(注释点名 Anthropic/Ollama 不能整体 `..Default::default()`),超时/流式/system prompt 走 adapter Default(与删前出厂同源);新测试 `set_provider_dispatches_all_kinds_with_pre_removal_defaults` 逐字段钉住四家装配。②从 `AiConfig` 删除六个字段(结构体本就 `#[serde(default)]`,未知键 serde 默认不拒绝),旧档读入只取 provider/base_url/model 三键;测试 `legacy_ai_json_with_all_removed_fields_loads_losslessly`(含全部旧字段+`api_style` 的三家旧档无损读入)、`save_drops_removed_fields_from_disk`(落盘七键绝迹+保留三键在场)双向钉住。③删除该测试(被测物已不存在,保留即必红;非「降断言过关」——控件消失的验收由新渲染探针 `ai_page_renders_kept_fields_without_removed_parameter_widgets` 承接,黑名单含被删控件全部标签文本);④「当前生效:{provider} · {model}」。
- **理由**:①任务书钉死「请求体仍按各 provider 现行为组装,不因删 UI 改变默认请求」——改前 app 装配对 Anthropic/Ollama 一直塞 0.7/1.0(adapter 的 None 默认从未在 app 里生效过),复刻它才是零行为变化;①b 会让 Anthropic/Ollama 默认请求体各少三个字段,是「顺手改默认行为」,超出本模块「删 UI」授权(想改回 None 走「如何改」①);②删字段是三条路里唯一让「被删参数」真正不可见用户不可误改的:②b 字段仍在 `AiConfig`,改 JSON 或旧档仍能带值但 UI 不显示,等于藏了个幽灵配置;②c alias 与「删」语义矛盾且省不掉任何错误处理;serde 默认忽略未知键本就是既有事实(#94 删 `api_style` 同款),零新机制;③被测控件删除后测试前提坍塌,改写为「绝迹」断言与文本探针重复,保留必红版本则更不可取;④流式形态不再是配置概念,留着就是撒谎。
- **如何改**:①要 Anthropic/Ollama 默认不发采样参数(落 #92/#93 的 adapter 语义),把 `ai.rs` `set_provider` 的 Anthropic/Ollama 分支改为 `..AnthropicSettings::default()`/`..OllamaSettings::default()` 并删三常量,同步改 `set_provider_dispatches_all_kinds_with_pre_removal_defaults` 的断言(temperature/top_p/num_predict 为 None);②要保留字段仅删 UI,把六字段加回 `AiConfig`(带 `#[serde(default)]`),`save_drops_removed_fields_from_disk` 改为断言旧档读回的旧键值原样保留;③要恢复滑杆连同其几何探针,恢复 `ai_page` 控件后从 git 历史取回该测试;④要恢复流式形态显示,在「当前生效」行补读运行时(注意 `AiRuntime` 不持有 stream 配置,需另行透传)。

## #107 #57 M2 禅定导航三态配置的四处置口径:默认悬停(升级不改既有观感)、设置页 selectable_label 三选一、三态分叉点在显隐判定一步(其后动画/绘制全共用)、切换三态即复位会话级悬停态(2026-10-06,#57 zen-hover-nav M2·自动拍板)

- **岔路**:任务书交给实现者的自由度:①默认态(悬停/常显/关闭三选一,用户原话「可常显示或鼠标划过去」两种形态都提了,未说哪个是默认);②设置页 UI 形态(下拉/单选自选);③常显与悬停「共用同一渲染件」的分叉点放哪层(判定函数内分叉 vs 两个入口);④禅定内切三态时会话级悬停态(去抖计数/可见位/动画值)的去留。
- **备选**:①悬停 vs 常显(用户原话里「常显示」在前);②ComboBox 下拉 vs selectable_label 单选行;③`ui_with_probe` 内按 mode match(Off 早退/Always 恒真/Hover 走 step)vs 常显单独入口(复用 `row` 等内部件);④保留残值交给下一帧判定自然收敛 vs 归约里立即复位。
- **自动选择**:①默认**悬停**;②selectable_label 单选(与主题三态/密度二态同款);③`ui_with_probe` 开头一个 match,Off 首行早退,Always 恒真 `(true, 0)`,Hover 走 M1 的 `step`;④`Message::ZenNavModeChanged` 归约里 `zen_nav = ZenNavState::default()` 一并复位。
- **理由**:①沿用 `TitleWidthMode::Full`/`show_minimap=true` 的「升级不改变默认行为」口径——M1 已合入的既有观感就是悬停,升级默认变常显等于替用户改掉禅定「屏幕上没有别的东西」的核心属性;常显/关闭是显式选择,设置页一眼可达;②主题/密度的既有形态就是 selectable_label,三选项平铺一眼可见,ComboBox 要点一下才见全选项,且该页已有两处同款,零学习成本;③任务书钉死「共用同一渲染件,只是显隐条件不同」——分叉收在显隐判定一步,其后动画、行绘制、滚动、点击归约三态逐字节同路径,关停档的「零路径」也由早退保证(不是画一个透明列);④不复位时 Always→Hover 会带着至多 12 帧(~200ms)残显、Hover 的 `nav_hit` 会把已隐列的驻位矩形算回「占屏」,复位后淡出立即起步、新模式从干净状态起算,行为可预期且只有一行(state.rs 本就依赖 `ZenNavState`)。
- **如何改**:①改 `theme.rs` `ZenNavMode` 的 `#[default]`(手动 `Default` impl 与旧档 serde 回落同源,一处改动两头生效);②换 `settings.rs` appearance 的三态控件形态(改 `for mode in ZenNavMode::ALL` 那一段);③改 `ui/zen_nav.rs` `ui_with_probe` 开头的 match;④删 `Message::ZenNavModeChanged` 归约里的复位行(接受残值自然收敛的话)。

## #106 #57 M1 禅定悬停标签导航的七处口径:感应区 16px、导航列 220px/行高 NAV_ROW_H 直角贴边抽屉、隐藏去抖 12 帧(进入无去抖)、显隐非对称(全隐的列不参与判定)、动画=0.15s 淡入+24px 自左滑入(egui 动画管理器自驱)、全隐帧零形状零命中、frameless 左缘 6px 让给窗口缩放区(2026-10-06,#57 zen-hover-nav M1·自动拍板)

- **岔路**:任务书交给实现者的自由度:①左缘感应区宽度(建议 ~12-24px);②导航列形态(宽度/圆角/贴边方式)与「标签多时可滚动」的落点;③唤出/隐藏动画形态(淡入/位移自选);④隐藏去抖帧数(任务书钉死「带去抖帧数自选记录」,进入侧要不要去抖未说);⑤显隐判定的「导航列内」边界——全隐的导航列算不算占屏;⑥去抖计数靠什么帧推进(任务书钉死「指针移动本身驱动重绘无需额外 repaint」);⑦无边框窗口左缘同时是缩放命令区,导航列与它谁吃命中。
- **备选**:①12px/16px/24px;②220px 直角贴左缘全高抽屉 vs 圆角浮卡;③纯淡入/纯位移/淡入+位移;④去抖 6/12/24 帧,进入侧带或不带去抖;⑤全隐列参与判定(列驻位矩形恒算「内」) vs 不参与;⑥显隐全靠事件帧 vs 隐藏待决窗口 request_repaint 兜底;⑦导航列后分配吃掉 6px 缩放区 vs 保持缩放区最后分配居上、导航列让出该条。
- **自动选择**:①16px;②220px、行高沿用 `NAV_ROW_H`(26)、直角贴左缘全高、`ScrollArea::vertical` 承接长列表;③0.15s 淡入(`ui::fade::crossfade`,`style.animation_time` 可访问性总闸)+24px 自左滑入(同一 alpha 插值);④隐藏去抖 12 帧(~200ms @60Hz),**进入无去抖**(移近即现);⑤**不参与**——全隐帧的列传 `Rect::NOTHING`,只有「可见或动画值>0」的列算占屏;⑥全靠事件帧(动画进行中 egui 自动 request_repaint,顺带推进计数);⑦缩放区保持最后分配居上,导航列让出左缘 6px。
- **理由**:①16px 在建议区间中位:12px 在高 DPI 缩放下细至难划,24px 会让「读正文时停在左侧」频繁误唤出;②220px 装得下常规文件名+脏星(长名走与标签条同一套 `elide_text` 省略号截断,#37 显示口径不二开);直角贴边是「从边缘抽出的抽屉」语义,圆角浮卡在禅定的极简正文上是更强的视觉噪音;行高沿用侧栏视图导航的 token,不新造数字;③纯淡入在深色正文上「凭空浮现」缺方向感,24px 位移给出「从左缘抽出」,两者同一 alpha 插值实现一份;时长复用 `fade::FADE_S`(0.15s 上限,用户把动画时长调 0 即时落端点,egui 内建直落且不产帧);④进入去抖等于迟疑的感应区——唤出手感必须即时;隐藏去抖吸收贴边抖动与沿缘划过的瞬时进出,12 帧 ≈ 200ms 与常见 hover-dismiss(100-300ms)同量级;⑤若全隐列也算「内」,左缘 220px 全变成感应区,阅读时停在左半屏会不停唤出——任务书的「移近**左缘**」语义就是窄条触发,唤出后列本体(与感应区连续,中间无断链)接管保持;⑥任务书钉死不做额外 repaint;已知并接受的边界:**指针移出后若完全静止且淡出动画已收敛,去抖计数停在半途,导航列保持显示直到下一次指针移动**——真实指针移动每次都产帧,只有「急甩出后立刻停住再不动」才触达该边界;⑦导航列后分配会劫走窗口缩放手势(无框窗口的唯一 resize 入口),缩放区让出则只在导航列可见时损失列左缘 6px 的点击(行宽 220px,点击目标不受实质影响)——resize 是全局命令、导航行是局部便利,前者优先。
- **如何改**:①改 `ui/zen_nav.rs` 的 `EDGE_W`;②改 `NAV_W` 与 `row` 的绘制(圆角/内边距);③改 `SLIDE_PX` 与 `FADE_S`(时长在 `ui/fade.rs`,全局共用,单独调时长须另起常量);④改 `HIDE_DEBOUNCE_FRAMES`;要进入也去抖,给 `step` 加独立的进入计数;⑤把 `ui_with_probe` 里 `nav_hit` 的条件改回恒 `rest`(一行);⑥要确定性隐藏,在「可见但已连续在外」窗口加 `ctx.request_repaint()`;⑦把 `draw_zen` 里导航列的调用挪到 `edge_resize_zones` 之后即反转裁决(不建议,见理由⑦)。

## #105 #55 M2 minimap 渲染与跳转的六处口径:窄条总宽 108(2+96 行条区+10 滚动条避让)、默认开、点击处对准视口中心、跳转经 temp 一帧滞后落地(窄条命中须在 ScrollArea 之后注册才不被背景拖拽抢层级)、颜色当帧 visuals 推导不设手工色表、折行文档按均匀逻辑行+内容比例(不做 galley visual row 模型)(2026-10-05,#55 source-minimap M2·自动拍板)

- **岔路**:任务书交给实现者的 M2 自由度:①窄条宽度(建议 ~80-100px)与内部构成;②`show_minimap` 默认值(开/关自选);③点击/拖动的对齐语义;④跳转意图的落地通道——同帧 `scroll_to_rect` 需窄条命中先注册,而 egui 同层后注册者居上,顺序约束与消费时机互相顶牛;⑤行条与高亮框的配色来源(两主题);⑥折行文档的行条铺排与高亮框口径(M1 行模型是逻辑行均匀等差,折行行的真实渲染高度不是)。
- **备选**:①96px 无避让 / 108px(2px 左内边距+96px 行条区+10px 滚动条避让)/ 120px;②默认开(VS Code 出厂一致)/ 默认关(严格维持现状观感);③a 点击处对准编辑器视口中心 ③b 视口顶对齐 ③c 滚动条拖块语义(点击处维持在高亮框内的相对位置);④a 窄条 interact 在 ScrollArea 前注册+闭内同帧消费 ④b 闭外(ScrollArea 之后)注册居上+意图经 temp 传接、下一帧闭包开头消费 ④c 直写 ScrollArea 持久 state 的 offset;⑤a 从当帧 visuals 推导(每主题一函数) ⑤b 每主题手工色表;⑥a 均匀逻辑行模型+高亮框按内容比例 ⑥b 换 galley visual row 模型。
- **自动选择**:①108px 构成如上;②默认**开**;③a 中心对齐;④b 一帧滞后 temp 传接;⑤a visuals 推导;⑥a 均匀逻辑行+内容比例。
- **理由**:①96px 行条区落在任务书建议区间,1px/单元恰与 M1 出厂 `MAX_UNITS=96` 对齐(条宽 = 96×1px/单元,`LineParams::factory()` 零重算直接可用);egui 滚动条贴视口右缘画在后、居上,行条最长 98px,最后 10px 留给它,不靠巧合避让;②VS Code/Sublime 等出厂即开,minimap 是编辑器标配导航件;「关闭=现状零变化」由否决线探针钉住(关=零形状零缓存路径),默认开不破坏该承诺,不想用就关;③中心对齐落点可预期(点哪里、哪里进视口中央),顶对齐在点窄条下部时把目标行推到视口顶、视线还要再扫半屏,拖块语义在点边缘时不动、实现又最繁;④实测推翻了「同帧更优」:interact 先注册会被 ScrollArea 的背景拖拽抢走命中(同层后注册者上,点击根本到不了 minimap),闭外注册+temp 传接是既有滚动通道(#29 口径)内的最小改动,60fps 下一帧滞后无感、拖动逐帧覆写意图连续跟随;直写持久 state 绕过既有通道且 ScrollArea 持久 id 派生链脆弱(editor.rs 注释明言不便推导);另实测 egui 0.36 `with_lines` 的 `data_mut` 借用内不能再进 painter(同一把 Context 写锁重入即死锁),故 shapes 构造与上画布拆成两段;⑤两主题同一函数(text fg 压 α 做行条、faint 提白/压黑做高亮框),将来调 visuals 或加主题自动跟随,单测断言两主题行条色不同且方向正确(暗偏白/亮偏黑);⑥行条本就是 M1 的抽象行(缩进×长度的宏观示意),高亮框按内容比例换算在非折行文档下与行条严格对齐,折行文档的视觉误差随折行深度增大——minimap 是宏观导航不是像素镜面,VS Code 同样不保证折行文档行条与正文严格等比;换 visual row 模型要把 M1 行模型(每行宽度也随折行变)推倒重做,收益配不上表面积。
- **如何改**:①改 `ui/minimap.rs` 的 `MINIMAP_W`(行条区 = 条宽 − PAD_X − 避让;要加宽行条区需同步 `LineParams.max_units`,参数进缓存键自动重建);②默认值改 `theme.rs` 手动 `Default` 的 `show_minimap` 与 `show_minimap_round_trips_and_old_settings_default_on` 断言(旧档回落值随 serde default 同源);③对齐语义改 `jump_ratio` 的中心化项(顶对齐把 `- ve*0.5` 改 `- ve`,维持比例则删该项);④要同帧落地——把 ScrollArea 的背景拖拽关掉(改变现状交互,须先评估拖空白区滚动的回归)再让窄条在其前注册;⑤配色改 `bar_color`/`viewport_fill` 两函数;⑥折行精确对齐需行模型换 galley visual row 源,等于 M1 重做(同时 `window()`/`viewport_highlight` 的换算基都要跟着换)。

> **销账注记(2026-10-05,#105 追加项「悬停滚轮转发」已执行,自选最优=转发,VS Code 行为;#56 遗留清理棒 ③)**:悬停窄条滚轮 → 文档按 minimap/文档高度比放大滚动,已落地。**先修正一处既往事实认定**:auto-plan #55 行「已知边界:悬停 minimap 时滚轮不转发给编辑器(该区域指针不在 ScrollArea 视口内)」的机制描述不成立——egui 0.36 `ScrollArea::end` 的内建滚轮消费只按「指针在 outer_rect 内」几何判定(scroll_area.rs:1222),窄条让 TextEdit 压窄、却仍在 ScrollArea 的 outer_rect 里,无头实测滚轮原本就以 **1:1 原速**滚动文档(offset 0→120,非死角);本条做的是把 1:1 原速升级为放大转发。实现(editor.rs ScrollArea 闭包内 + minimap.rs `wheel_editor_delta` 纯函数):①截获点必须在闭包内、`end()` 之前——读走 `smooth_scroll_delta.y` 并清零(egui 嵌套滚动同款抑制手法,该字段文档明言 ScrollArea 读后即清),不清零则内建先按 1:1 滚一次、与转发叠成双滚;②放大系数 = delta ×(内容高 / 总行数×ROW_H),与 `jump_ratio` 同一几何(full = 总行数×比例尺),VS Code 实按视口高比(更快)但偏离点击换算源,不取——滚轮步长与行条自身像素位移 1:1;③落地走 `Ui::scroll_with_delta_animation` **相对增量**而非 `scroll_to_rect` 绝对目标:egui 平滑滚轮的事件帧只交 90%、余量随后几帧衰减交齐,绝对目标拿上一帧基数互相覆写(无头实测振荡 540→54→1085),相对增量在 `end()` 里加到当帧已应用的 offset 上,天然链接、越端由 `end()` 的 offset 边界统一钳。边界全部保持:命中区与点击/拖动同一矩形(最右 10px 仍让滚动条);`dragged_id` 判据镜像内建口径(拖窄条/滚动条/选区进行中不截获);跳转 temp 通道零改动;高亮框逻辑零改动(随 offset 自然更新);正文区滚轮仍 1:1(无头断言钉死);Live 模式与关闭态不进该路径。单测:纯函数放大恒等式与 None 边界 + 端到端(放大步长、连续两档 = 2×步长的否决线、向上退回、正文区 1:1、高亮框随动、不写跳转意图)。真机目视:放大系数手感(≈行高/ROW_H ≈ 6.7×)与触控板动量尾帧跟手度留人工。本注记不改动上方原条目。

## #104 #55 M1 minimap 行模型的六处口径:比例尺 3px、短文档顶部对齐不拉伸、tab 折 4 空格(egui 现状口径)、显示宽用简化 EAW 大区段表(ZWJ/变体选择符零宽)、缩进+长度联合封顶、缓存挂 editor_id 派生的 egui temp memory 而非 TabState 字段(2026-10-05,#55 source-minimap M1·自动拍板)

- **岔路**:任务书把六处自由度交给实现者:①比例尺(minimap 每行条高,建议 2-4px 自选);②文档行数少于 minimap 可容纳行数时的铺排(不满高自选);③tab 宽口径(「读现状,没有则以现状为准记录」——仓库无 tab 设置项,需定基准);④CJK 全宽之外的显示宽判定(emoji/ZWJ 组合串/变体选择符怎么计,零新增 crates.io 依赖红线内不能引 unicode-width);⑤超长行截断封顶的口径(只封内容、还是缩进+内容联合封顶);⑥缓存的宿主形态——照 #39 先例有两个可抄面:preview.rs `tab_preview_id`(egui temp memory 挂 id)或 `PreviewState`(TabState 字段、`synced_rev` 修订号键控)。
- **备选**:①2px/3px/4px;②a 顶部对齐余量留底 ②b 行高拉伸填满(VS Code 短文档形态)②c 垂直居中;③a tab=4(egui `FontTweak::tab_size` 默认,epaint 对 `\t` 的 advance 即 tab_size×空格宽) ③b tab=2 或 tab=8;④a 完整 East Asian Width 表逐码点 ④b 简化 W/F 大区段表+emoji 密集区并入+零宽连接符特判 ④c 引 unicode-width(违零依赖红线);⑤a 只封内容宽度(缩进可把条推出右缘) ⑤b 缩进+内容联合封顶(条永不溢出);⑥a egui temp memory 挂 `editor_id.with("minimap-lines")` ⑥b TabState 加 `MinimapState` 字段。
- **自动选择**:①3px(`ROW_H = 3.0`);②a 顶部对齐、余量留底;③a tab=4(`TAB_UNITS = 4`);④b 简化表;⑤b 联合封顶(内容预算 = `max_units - indent`);⑥a egui temp memory。
- **理由**:①2px 在 HiDPI 缩放下细至难辨、4px 让 10000 行占 40000px(行程过长、滑起来发飘),3px 下 10000 行 = 30000px、600px 视口约 200 行,与 VS Code 默认观感同量级;②短文档时编辑器本身滚不动(travel=0),minimap 拉伸行高会引入「文档变长时行高突变」的跳变,顶部对齐 + 固定比例尺在长短文档间形态一致,余量留底即 VS Code 大文档关掉的状态;③编辑器现状无任何 tab 定制(无 FontTweak、无设置项),egui 0.36 epaint text_layout 对 `\t` 的 advance = `tab_size × 空格宽`、`tab_size` 默认 4.0,取 4 即「以现状为准」;④完整 EAW 表上百个零散 W 单点,对 minimap 抽象条的相对长度观感无意义,大区段合并表的误差方向只是把极少数窄字符记宽 ≤1 单元;ZWJ 家庭序列(👨‍👩‍👧)与变体选择符(❤️)按字形显示语义应整体记一次全宽/零宽,逐段累计会把长度放大 N 倍;⑤条溢出右缘再靠渲染层 clip 是两道工序一道结果,直接在模型里联合封顶,M2 拿到的数据天然合法,单测也能钉「缩进+宽度 ≤ max_units」的全表不变量;⑥temp memory 路线与 `ime-caret`(editor.rs)同款、回调借出存储切片命中帧零 clone,不动 tabs.rs(M1 纯数据层最小表面积);TabState 字段路线要动构造/换入路径,留给 M2 若需要持久化 minimap 自身 UI 状态时再议。
- **如何改**:①要调比例尺——改 `ui/minimap.rs` 的 `ROW_H` 常量(窗口计算是参数化的,M2 也可不传默认值自传);②要拉伸填满——`window()` 的短文档分支按 `minimap_height / total` 重算有效 row_h(注意与 ① 的下限 0.5px 取 max);③要改 tab 口径——改 `TAB_UNITS` 或 M2 侧传 `LineParams { tab_units, .. }`(参数进了缓存键,自动重建);④要完整表——把 `is_fullwidth` 的 `matches!` 区段表换成逐码点数据(引 unicode-width 需先在 ADR 登记依赖,违本任务零新增红线);⑤要只封内容——`line_model` 里 `budget` 从 `max_units - indent` 改为 `max_units`,并同步删 `ten_thousand_line_document_builds_correctly` 的联合封顶断言;⑥要搬进 TabState——`MinimapCache` 挂 `TabState` 新字段(`TabState::new`/`load` 各补一处),`with_lines` 改从 `&mut TabState` 借,egui temp memory 槽位删除。

## #103 #54 M2 蒙层渲染与内容源的六处口径:暗底 160/亮底 216(alpha 从 visuals.dark_mode 推导)、无绑定命令照列、分组=Command::group() 注册表新维度(7 组+兜底,无 Git 组)、Esc 归蒙层(reduce 层 retain 移除)、关闭即重起表不设抑制、分组两列 Grid(horizontal_wrapped 无头不收敛的教训)(2026-10-05,#54 shortcut-overlay M2·自动拍板)

- **岔路**:任务书把自由度交给实现者:①全窗半透明底的不透明度与取色(只钉「两主题各取 visuals 推导不硬编码」);②无绑定命令(AI 三条/导出 PDF/分割线/表格)显示与否;③「分组沿用注册表既有分类」——现状是注册表(`command.rs`)**没有**分类字段,既有分类散在 menubar 子菜单(文件/导出/编辑/视图/AI)与 format_bar 工具条(格式组),任务书例举的「Git」组在命令表里无对应物;④Esc 关闭路径与其它 Esc 语义(禅定退出/emoji 关闭/改键捕获取消)的层级;⑤点击/Esc 关闭后修饰键仍按住时要不要再触发;⑥分组卡片的排版(可滚动是钉死的,列形态自选)。
- **备选**:①a 单一黑底固定 alpha 两主题共用 ①b 两主题各取黑/白且 alpha 分档;②a 照列(标「未绑定」) ②b 隐藏;③a 蒙层自抄一份分组清单 ③b 给 `Command` 加 `group()` 注册表维度(单一事实源);④a Esc 谁都不消费全靠各层自己判断 ④b 蒙层可见帧在归约层把 Esc 从事件流移除;⑤a 关闭后抑制到修饰键松开 ⑤b 关闭即回 Idle 重新起表;⑥a `horizontal_wrapped` 流式换列 ⑥b 两列 Grid。
- **自动选择**:①b+②a+③b+④b+⑤b+⑥b。蒙层底暗色 `from_black_alpha(160)`(63% 黑)、亮色 `from_white_alpha(216)`(85% 白),按 `visuals().dark_mode` 分档;无绑定命令照列(组内弱化「未绑定」),蒙层命令集==`Command::ALL` 全集;`CommandGroup` 七组(文件/编辑/格式/视图/标签/导出/AI)+`Other` 兜底,归属沿用 menubar 与 format_bar 的既有分类,全部 39 条命令显式归类、`every_command_has_a_group` 钉住兜底组为空;蒙层可见帧在 `reduce` 里 retain 移除 Esc 按下事件(蒙层可见期间修饰键按着,`consume_key(NONE,…)` 的逻辑匹配对带修饰键的 Esc 不成立,故用 poll_capture 同款 retain);`close()` 与 step 的按键/松开关闭同语义——回 Idle,再长按 3s 再触发;分组用 `egui::Grid` 两列摆放、组内命令/键位两列、外层 `ScrollArea::max_height` 限高。
- **理由**:①亮色下正文是深字,弱化同样内容需要更厚的白,单一黑底在亮色主题下会把底层内容压成泥;两档 alpha 都从 `visuals().dark_mode` 推导,换主题皮肤自动跟随;②蒙层与设置页「快捷键」、快速打开的命令组口径一致(两处都显示无绑定命令),且「命令集==注册表全集」的断言由此字面成立;AI 三条是暂无键位而非无意义,蒙层里可见=可发现「这些还没键位」;③蒙层不抄第二份清单是任务书红线;注册表加维度是「单一事实源」的正解——menubar 将来可迁移消费(本次不动它,避免回归面扩大);Git 在命令表里没有命令(P2 的 Git 是面板不是命令),任务书例举的「Git」组按现状落空,如实不造空组;④浮层层级语义:最顶层浮层先吃 Esc,禅定/emoji/捕获当帧不可达——蒙层关掉后用户再按一次 Esc 即可到达下层语义,一次一键不叠加;⑤M1 测试 `visible_closes_on_release_key_or_unfocus` 已钉死「关闭后仍按住→重新起表」,点击/Esc 关闭跟随同语义不另设抑制期;重新触发需再按满 3s,门槛本身即防抖;⑥实测教训:`horizontal_wrapped` 配 `set_width` 在无头帧里不收敛,分组卡被排成一行直到屏外(x 到 1505),换 `Grid` 两列(设置页 keymap 表同款容器)后布局确定;Grid 列宽由内容收敛,无头/真机一致。
- **如何改**:①要调观感——改 `shortcut_overlay.rs` 的 `SCRIM_ALPHA_DARK`/`SCRIM_ALPHA_LIGHT` 两个常量;②要隐藏无绑定命令——`grouped_rows` 的行过滤 `shortcut.is_some()`(同步删 `overlay_rows_cover_every_command` 的全集断言改为「绑定命令全集」);③要改分组/加 Git 组——改 `command.rs` 的 `CommandGroup::ALL` 与 `Command::group()`(新命令忘了归类会落 `Other`,测试会红);④要 Esc 穿透蒙层——删 `ui::layout::reduce` 里 `overlay_visible` 的 retain 段(蒙层关闭本身由 step 的「其它按键」规则完成,不动也不影响关闭,只影响下层可见性);⑤要抑制期——`ShortcutOverlayState` 加 `Suppressed` 态,`close()` 落它、`step` 在修饰键松开/失焦时才回 Idle;⑥要换列形态——改 `paint` 里 `shortcut-overlay-groups` Grid 的 `num_columns` 与 `chunks(n)` 步长。

## #102 #54 M1 长按检测状态机的三处口径:IME 组合态进行中不触发(视为打字)、仅「平台主修饰键单独按住」才算长按(叠加 Shift/Alt 是和弦)、设置页改键捕获期间不特殊处理(留给 M2)(2026-10-05,#54 shortcut-overlay M1·自动拍板)

- **岔路**:任务书把三处自由度交给实现者:①IME 组合态进行中(候选框打字中)长按到点是否触发——任务书明示「自选并记录(倾向不触发,防打字误触)」;②「按住平台主修饰键」的判定口径——只要 Ctrl/⌘ 按住就算(叠加 Shift/Alt 也算),还是必须**单独**按住;③设置页「快捷键」正在捕获新键位(用户可能长时间只按着修饰键等灵感)时,检测是否要为该模态让路。
- **备选**:①a 组合态期间照常触发 ①b 组合态期间不触发(视同打字);②a 宽口径(修饰键按下即可,叠加其它修饰键不取消) ②b 严口径(`Modifiers::command_only`:Win/Linux 只按 Ctrl、mac 只按 ⌘,叠加即取消);③a 捕获期间暂停检测并清零 ③b 不特殊处理(按通用取消规则自然覆盖)。
- **自动选择**:①b + ②b + ③b。`is_other_key` 把任何 `Event::Ime`(Preedit 组合中 / Commit 上屏 / DeleteSurrounding)都算「其它按键」——组合态的每一帧都在取消长按,3s 自然凑不满;`modifier_held` 取 egui 既有 `Modifiers::command_only()` 平台抽象;改键捕获期间检测照常运行,不另设分支。
- **理由**:①任务书倾向即是结论:候选框期间用户在打字,长按 ⌘ 的常见来源是输入法切换/选词的残留指法,此刻弹蒙层是典型误触;且组合态每帧都有 Ime 事件流入,「视为打字」实现为零特判(事件归类进 other_key 一条 match 臂);②叠加 Shift/Alt 时用户在和弦(如准备 Ctrl+Shift+K),macOS Cheatsheet 系蒙层也只在 ⌘ 单独按住时出现;`command_only` 是 egui 既有平台抽象,零自研平台判定;③改键捕获对「按住修饰键等 3s」的响应路径已被通用规则覆盖——捕获完成的按键本身即 other_key 取消;真出现「捕获中只按修饰键 3s」的场景,M1 无渲染无感知,M2 画蒙层时若观感冲突再在此条追加「如何改」③分支,不为未发生的场景预写分支。
- **如何改**:①要组合态期间可触发——`shortcut_overlay.rs::is_other_key` 删 `Event::Ime(_)` 那条臂,并删 `chords_text_clipboard_and_ime_count_as_other_key` 里对应的 Ime 断言;②要宽口径——`frame_input` 的 `modifier_held` 从 `command_only()` 改为 `modifiers.command`,并同步 `chords_text_clipboard_and_ime_count_as_other_key` 的「叠加 Shift 是和弦」断言(该断言同时钉住取消语义:叠加修饰键当帧取消);③要捕获期间暂停——`ui::layout::reduce` 的检测接线外包一层 `state.settings.capture.is_none()` 并在 else 分支调 `state.shortcut_overlay` 的取消入口(需新增,现为纯 `step` 驱动)。

## #101 #53 M3 行内词级高亮的四处置口径:仅「单删+单增」紧邻块启用(一对多降级回行级)、第二级底色取同公式 35% 占比(不下划线)、整行重写区段铺满两侧、区段计算 char 级(ZWJ 序列不保证 grapheme 边界)(2026-10-05,#53 git 双栏对比 M3·自动拍板)

- **岔路**:①任务书授权的降级口径——「相邻配对在连续删/增块中歧义过大(如一对多)时,允许只对『单删+单增』紧邻对启用」:歧义判定的具体形态是什么(按块内删/增行数?按配对结果逐对判断?);②行内区段的视觉形态——「更深的第二级底色 or 下划线」二选一,深多少、怎么与主题推导;③整行重写(无公共前后缀)时区段=整行,是否跳过高亮避免「第二级盖满整行」;④区段边界粒度——任务书钉死 char 级不 byte 切,但 emoji 的 ZWJ 序列(👨‍👩‍👧)由多 char 组成,边界落在序列中段怎么办。
- **备选**:①a 按块判定(块内恰 1 删 1 增才算无歧义,整块启用/整块降级) ①b 逐对判定(只要某对形态是「一删对一增」就算,2删2增块的两对也算) ①c 词级 LCS/Myers 对齐(接近 GitHub unified 的词 diff);②a 第二级底色(占比高于整行档) ②b 下划线;③a 整行重写也画(区段铺满) ③b 区段=整行时跳过(视为无高亮);④a char 级即可(ZWJ 可能被切开) ④b grapheme cluster 级(ZWJ 序列不切开,需 unicode-segmentation 或手写 ZWJ 判定)。
- **自动选择**:①a 按块判定——块内恰「一删一增」(olds.len()==1 && news.len()==1)才计算行内区段,配对层(`split_rows`)打 `inline` 标,其余形态(纯删/纯增/一对多/多对多/上下文对)一律 `None` 保持 M2 行级;②a 第二级底色:与整行底色同一公式(语义色向 panel_fill 伽马插值),占比 0.35(整行 0.16),`TextFormat::background` 画在字形之下的 mesh 里,分段 LayoutJob(前缀/区段/后缀三段);③a 整行重写也画——区段铺满两侧,叠加后呈「整行更深的语义色」,与 GitHub split 的观感一致(那里整行重写也是满行 word-diff 底色);④a char 级——零新增依赖,文本切片天然是完整 char 序列不截半字节;ZWJ 序列被划进区段时仍整段合法显示,不 panic。
- **理由**:①块是 diff 的天然歧义单元——xdiff 在多删多增块内的删↔增对应本来就是下标启发式(配对层注释如实写着),行内区段会把「猜测的对应」再放大成「看似精确的词对比」,误导性强;一删一增是唯一语义无歧义的替换形态(单行修改),这里高亮置信度 100%;词级 LCS 是完整 word-diff 引擎,超任务书口径(GitHub split 的行内高亮也是前缀/后缀裁剪而非词对齐)且复杂度失控风险高——按任务书「复杂度失控同理,如实降级不硬做」精神不引入;②第二级底色与整行底色必须同主题推导(暗色主题下下划线的可见性与语义色都难调,且下划线在等宽代码字体里与拼写检查/链接惯例冲突);0.35/0.16 的两档在明暗主题下都保持「第二级可辨、不压正文」(渲染探针钉住「第二级距面板底色比第一级远 ≥8」),数值精确值留真机目视(见 notes);③跳过整行重写会让「完全改掉的一行」反而没有行内高亮,与「只改一个词的行」视觉不一致;铺满两侧在 0.35 档下不会盖过正文对比度;④grapheme 级要引入 unicode-segmentation(app 目前零此依赖)或手写 ZWJ/ZVJ 判定,收益仅是极端输入(把 ZWJ 序列部分划入区段)的显示边缘案例,char 级即任务书钉死的口径,如实保留边界特性。
- **如何改**:①要对一对多也启用——`git_split_diff.rs` 的 `split_rows` 块内判定从 `olds.len() == 1 && news.len() == 1` 放宽(如全部两两配对形态都算),配对层测试同步放宽;②要下划线——`sidebar.rs` 的 `paint_split_side` 把区段 section 的 `TextFormat::background` 换成 `underline: Stroke::new(1.0, tint)`;③要跳过整行重写——`split_rows` 打标处加 `!change.old.is_empty() && change.old.len() < old.chars().count()` 类判定(注意镜像判定新侧);④要 grapheme 级——引入 unicode-segmentation 或在 `inline_change` 的前后缀推进循环里加 `is_zwj延续` 判定(边界外推到序列边界),登记依赖后动。

## #100 #53 M2 双栏 diff 的六处口径:默认双栏、长行单行截断+悬停全文、行对上限 2000、切换不持久化、结构化失败回落统一文本、reload 双取保留 diff: String(2026-10-05,#53 git 双栏对比 M2·自动拍板)

- **岔路**:①默认视图(任务书默认双栏,选统一才要记理由,此处落档);②长行策略——截断+悬停全文 or 横向滚动(任务书二选一自选并记录);③行对数上限值(任务书建议 2000,「值自选」);④「双栏/统一」切换状态是否持久化(任务书「不必持久化(自选记录)」);⑤双栏数据通道的结构化 API 失败时降级口径(任务书「错误文案仍进 diff 区可见,现状口径不破」的具体形态);⑥既有 `diff: String` 字段去留(任务书「去留自选」:结构化 API 单取+丢弃 String,还是双取并存)。
- **备选**:①默认双栏/默认统一;②a 单行截断(…)+悬停 tooltip 全文 ②b 横向滚动(统一视图现状 `ScrollArea::both`);③2000/1000/5000/不加(只靠 M1 的 ~64KB);④a 会话内不持久化(重启回默认) ④b 落 settings.json;⑤a 双栏回落渲染统一文本(错误文案在 `diff` 字符串里,原样可见) ⑤b 双栏区显示独立错误行;⑥a 双取(diff_file + diff_file_lines 各调一次) ⑥b 单取结构化、`diff` 字段删除/由结构化重建。
- **自动选择**:①默认双栏;②**单行截断+悬停全文**(epaint `TextWrapping::truncate_at_width`,elision 在字形层完成,char 边界安全;悬停挂行响应显示被截断一侧全文);③**2000**(取任务书建议档);④a 不持久化(会话内状态,`GitPanelState::diff_view`,重启回双栏);⑤a 回落统一文本渲染;⑥a 双取并存,`diff: String` 保留(统一视图行为零变化)。
- **理由**:①任务书钉死默认双栏(GitHub 对比视图即产品对标),无翻案理由;②双栏各占侧栏半宽(~150-200px),横滚在「两侧各有行号列+正文」的布局里一次只能滚半边、另一半被带歪,可用性差;截断保行盒恒定(行高不随内容变,滚动稳定),悬停补全文——与 #40 文件树单行截断+悬停全名的先例同款口径;行对上限配合视口裁剪后,每帧绘制 O(可见行),布局 O(2000) 恒定;③2000 行对≈4000 单元格,与文件树/搜索 500 行、status 500 条的截断先例同量级偏宽(diff 是临时查阅场景,宽一点减少日常触顶),M1 的 ~64KB 行级预算(≤~32k 行)仍是第一道闸,本值只防「预算内但超长」的卡帧;④diff 视图是「正在看的这个文件」的临时阅读偏好,跨会话记住它的价值低于「每次进来都是产品默认对标形态(GitHub 双栏)」的一致性,且省一条 settings 字段与读写通路;⑤结构化失败实践上与 diff_file 同源同错(同一份 libgit2 diff),回落统一文本使「错误文案进 diff 区」的现状口径在两视图下字面成立,不需要第二套错误展示;⑥双取是毫秒级本地读×2,换统一视图(含既有测试断言的 `git.diff` 文本口径)零改动——单取+重建 unified 文本会丢文件头(`--- a/x`/`+++ b/x`)且要自写重建器,纯增表面积。
- **如何改**:①要默认统一——`git_panel.rs` 的 `DiffView` 枚举把 `#[default]` 从 `Split` 挪到 `Unified`;②要横滚——`paint_split_side` 的 `LayoutJob` 从 `truncate_at_width` 改 `no_max_width`,外层 `split_diff_view` 的 `ScrollArea::vertical` 改 `both`(注意双栏共享横滚,两侧行号会一起滚出视口,需另行评估);③要改上限——改 `git_split_diff.rs` 的 `MAX_SPLIT_PAIRS` 一个常量(提示行文案与测试断言同步);④要持久化——`State` 侧加 settings 字段并在 `Message::GitDiffViewChanged` 归约里落盘(照 `ThemeDensityChanged` 通路);⑤要独立错误行——`diff_area` 的 `None` 分支改为渲染 `git.diff` 中的错误文案独立弱化行;⑥要去掉 `diff: String`——`reload_diff` 单取 `diff_file_lines`,统一视图改由结构化 hunks 重建文本(文件头会丢,既有 `select_reads_diff...`/`git_messages_drive...` 等断言 `git.diff` 的测试要连片改)。

## #98 #51 M3 app 侧 mermaid widget 四处口径:超宽图整图等比缩进面板而非横向留白、Live 列用只拦 mermaid 的独立 handler 而非复用 AiLinkHandler、块序号取本帧文档序计数、回落用「嵌套无 handler 的 MarkdownLabel」而非手写代码块(2026-10-04,#51 mermaid-render M3·自动拍板)

- **岔路**:①任务书「widget 高度=布局高度,宽度超面板时按比例缩小或水平留白,取舍写 notes」二选一留给实现;②Live 富渲染块接 mermaid 的方式:复用右栏预览的 `AiLinkHandler`(一并带上 ai 卡/emoji/wikilink 行为)还是新造只拦 mermaid 的 handler;③任务书「用块序号+内容哈希等稳定键」——vendored `block_code_widget` 回调不带 token 序号,块序号从哪来;④回落态源码代码块的画法:vendor 的 `render_code_block` 是私有函数,app 侧手写近似代码块还是嵌套 `MarkdownLabel`。
- **备选**:①a 等比缩放(字号随之缩小) ①b 原尺寸+水平溢出留白/裁切;②a 复用 AiLinkHandler ②b 独立 LiveMermaidHandler;③a vendor ①类补丁透传块序号 ③b app 侧 handler 帧内计数(照 ```ai 卡 `card_count` 模式);④a 手写代码块(背景+高亮+按钮) ④b 嵌套 MarkdownLabel 渲染原围栏文本。
- **自动选择**:①等比缩放,`scale = min(1, 面板宽/图宽)`,缩放同步作用于全部坐标与文字字号,窄于面板时水平居中留白;②独立 `LiveMermaidHandler`(只实现 `is_block_code_widget`/`block_code_widget` 对 mermaid 命中,其余全走 vendored 默认);③handler 帧内计数器(preview 侧进 `AiLinkHandler::mermaid_count`,与 `card_count` 分开计数;widget/缓存键 = 外层 label id + 块序号,内容只进缓存条目的 content_hash);④嵌套 `MarkdownLabel::new(稳定id, "```mermaid\n{源}\n```")`,**不传 link_handler**。
- **理由**:①流程图超宽时留白等于右侧内容永远不可见,缩放保完整性;布局缓存不含面板宽,拖窗宽不重排只重缩放,流式期间也不因面板抖动清缓存;字号同比缩小使「文字在盒内」的几何关系在任意 scale 下近似保持;②Live 列此前不接任何 handler,复用 AiLinkHandler 会顺带改变 ai 卡/emoji/wikilink 在 Live 的渲染行为,越出 #51 的改动边界(否决线:非 mermaid 块路径不变);③vendor 补丁透传序号要动 ①类补丁+登记,而帧内计数在「文档结构不变时每帧序号稳定」上与 token 序号等价,流式追加只动最后一块,上游插入 mermaid 块才平移下游序号(且只清 mermaid 自己的缓存);④手写代码块要么放弃 syntect 高亮要么给 app 新增 syntect 直接依赖,嵌套 label 零 vendor 改动复用 vendored 高亮+复制按钮全链路,不传 handler 即 `is_block_code_widget` 恒 false,天然不可能递归回 mermaid widget。
- **如何改**:①要改为留白:`ui/mermaid.rs` `paint_diagram` 的 `scale` 固定 1.0、水平居中改左对齐,超宽部分交给面板横向滚动(需另开横向 ScrollArea);②要让 Live 也渲染 ai 卡/emoji:Live 列改为构造 `AiLinkHandler`(需传 `AiState`,live.rs 现无此参数,要改 `live::ui` 签名并从 App 层传入);③要 token 序号:vendor label.rs `block_code_widget` 调用点(约 951 行)加 `push_id(i)` 包裹或改 trait 签名透传(①类补丁,vendor/README 登记);④要手写:在 `paint_fallback` 里用 `ui.visuals().code_bg_color` 画底+`highlight_code` 同款逻辑(app 需直接依赖 syntect,先在 ADR-004 登记依赖再动)。

## #97 #51 M1 mermaid 解析与布局的六处口径:渲染路线取纯 Rust、方向仅收 TD|LR 不收 TB 别名、`---` 按书写方向参与分层、孤立单节点 Ok 不 Err、跨层长边拆虚拟节点走正交肘形、链式边/引号标签收编而更长横线与 `&` 并列显式 Err(2026-10-04,#51 mermaid-render M1·自动拍板)

- **岔路**:①渲染路线(auto-plan #51 已拍板「纯 Rust 内嵌 vs 外部 mmdc(node)/远程 kroki——取纯 Rust」,五要素需在本表在册);②方向别名:任务书钉死「flowchart/graph TD|LR」,但 mermaid 另有 TB(TD 的别名)/BT/RL 与小写写法;③`---` 无向边在分层布局里的方向语义;④任务书「空图/孤立单节点/环(a-->b-->a)不 panic 且 Err 有可读信息」一句中,孤立单节点到底是 Ok 渲染还是 Err;⑤连线形态:任务书允许「折线或正交连线」二选一,跨层长边(A→C 中间隔着 B 层)是否上 Sugiyama 虚拟节点;⑥标签边界:双引号标签、链式 `A --> B --> C`、更长横线 `---->`、`-- 文字 -->`、`A & B` 并列是否算 v1 子集。
- **备选**:①外部 mmdc / 远程 kroki;②a 顺手收 TB 等别名 ②b 严格只收 TD|LR;③a 无向边双向等价 ③b 按书写方向定向参与分层;④a 孤立单节点 Err ④b Ok 渲染一个盒;⑤a 直线折线 ⑤b 正交肘形+跨层长边拆虚拟节点 ⑤c 正交肘形不拆虚拟节点(垂直段可能穿中间层盒子,靠「宽松断言」兜);⑥a 严格最小集(链式/引号也 Err)⑥b 收编链式与引号、其余 Err。
- **自动选择**:①纯 Rust 内嵌(latermd-render `mermaid` 模块,零新增 crates.io 依赖);②严格 TD|LR,TB/BT/RL/小写一律 Err(错误信息写明「仅支持 TD|LR」);③`---` 按书写方向参与分层与消交叉,绘制时不带箭头;④孤立单节点 Ok 渲染一个盒,空图(0 节点)与环 Err 且消息可读(环给出 `a → b → a` 全路径);⑤**b**:正交肘形折线 + 跨层长边拆零尺寸虚拟节点(参与 barycenter 排序与列槽占位),因此全部边只走相邻层,折线不穿任何盒——任务书的「宽松断言」被升级成严格断言(样本图含跨层长边也零穿越);⑥b:链式边拆成多条边、双引号标签收编(容纳 `)`/`<` 等特殊字符),`---->`/`-- 文字 -->`/`A & B`/`A((x))` 复合形状等显式 Err。
- **理由**:①离线+隐私+铁律 2(auto-plan 已拍板,此处落档五要素);②别名集没有任务书背书,「不猜测不吞错」宁可 Err 回落为源码代码块也不静默猜方向,TB 是高频写法但收编它是扩子集的决定,应显式做;③无向边若按双向等价处理,任何 `A --- B` 都成环、分层不变式被破坏,按书写方向定向是 dagre 系通行做法,视觉上无箭头仍忠实「无向」原义;④mermaid 本身渲染孤立节点,Err 会把合法输入错打成回落代码块;任务书该句的可辨认意图是鲁棒性(不 panic),空图与环才真正无法产出图;⑤首版实现先走了 c,样本图(判断分支 B→D 跨层)立刻暴露垂直段穿中间层盒——分支汇合是 flowchart 最常见形态,靠挑样本图绕过等于把缺陷留给用户文档,虚拟节点是 Sugiyama 标准第二步、约 40 行,换来「任意图零穿盒」的强保证;⑥链式与引号是 flowchart 核心语法、真实文档高频,收编成本十数行;更长横线/并列/复合形状超出任务书枚举,显式 Err 保持子集可审计(v1 子集边界由测试逐条钉住)。
- **如何改**:①要切外部引擎(mmdc/kroki)——推翻 auto-plan #51 的拍板本身:删 latermd-render 的 mermaid 模块与 app 侧 `ui/mermaid.rs`,`block_code_widget` 分支改为起外部进程/发远程请求取图;须先接受 mmdc 依赖 node 运行时、kroki 需联网(离线+隐私两条理由失守),且铁律 2 的 headless 渲染场随之外包给外部工具链;②要收 TB:改 `crates/latermd-render/src/mermaid/parser.rs` 方向 match,加 `"TB" => Direction::TopDown` 分支(小写别名同理);③要无向语义:`layout` 的环检测与分层前把 `---` 边从邻接表剔除(分层不再受它约束,绘制仍画线);④要 Err:在 `parse` 收尾处对 `nodes.len()==1 && edges.is_empty()` 加错误分支;⑤要退回直线折线:删 `layout.rs` 的虚拟节点拆链段(`chains`/`ext_layers`),`elbow` 换成两点直连,并把 mod.rs 的 `edge_polylines_avoid_boxes` 断言降回「相邻层边」子集;⑥要放行 `---->`:在 `parse_statement` 的箭头 token match 加更长横线模式;要收 `A & B`:在 `node_ref` 前加 `&` 分隔的多节点展开。M2/M3 若发现回落率过高,优先按本条扩子集而不是放宽「不吞错」纪律。

## #96 #25 PDF 导出技术路线:A 纯 Rust 直绘(krilla 后端)而非 B HTML→PDF,printpdf 被实测否决(2026-10-04,#25 export-pdf M1·自动拍板)

- **岔路**:PDF 导出只有两族技术路线——A「纯 Rust 直绘」(Markdown token → 自建布局引擎 → PDF 编码后端,后端候选 printpdf / genpdf / krilla)与 B「HTML→PDF」(复用 latermd-export 的 HTML 产物,引擎候选纯 Rust 的 fullbleed、Rust CDP 的 headless_chrome、外部的 wkhtmltopdf/weasyprint)。这也是 roadmap「crate 增量创建表」里 `latermd-render`(「出现第二个消费者时」创建)是否随 PDF 落地的岔路:走 A 则 latermd-render 本次创建并成为铁律 2 验证场,走 B 则 HTML 直达引擎、latermd-render 依旧不创建。选型证据全文见 [adr-006-export-pdf-route.md](adr-006-export-pdf-route.md)(草案,待追认)。
- **备选**:A1 printpdf 0.12.8;A2 krilla 0.8.2;A3 genpdf(printpdf 系布局层);B1 fullbleed 2.5.6(纯 Rust HTML/CSS→PDF);B2 headless_chrome(用户机浏览器);B3 wkhtmltopdf/weasyprint(外部引擎)。
- **自动选择**:**A2 = krilla 0.8.2**。新建 `latermd-render`(token→布局指令 IR,不 import egui)承担 CJK 断行/段落流/表格/代码块/分页/页码的全部布局,`latermd-export` 依赖 krilla 0.8.2 做编码后端;后续模块(编排驱动)按 [adr-006-export-pdf-route.md](adr-006-export-pdf-route.md) §6 锁定实现。
- **理由**:①离线约束先砍人——无签名分发(ADR-004 §3)的对价是装完即用,headless_chrome 要求用户装 Chrome(README 明示驱动本机 Chrome/Chromium)、wkhtmltopdf 仓库已归档(gh api 实测 archived:true、2022-11-22 停推),B 只剩 fullbleed;②fullbleed 不够格押注——crate 2026-02-11 才创建(8 个月)、总下载 2,519、47 stars、docs.rs 文档率 9.18%,且**随包字体零 CJK(实测:不注册字体时中文全部渲染为 `?`,pdftotext 输出 `??,LaterMD ??`)**,必须运行时注册系统字体才行;③A 路线后端里 printpdf 有实测致命伤——同一张 19.5MB 系统 TTC、同一段中英混排、`subset_fonts: true`,printpdf 输出 13,190,466 B(子集化失效,debug/release 同值)而 krilla 6,293 B,差 2099 倍,「能邮件发出去的 PDF」场景直接不可用;genpdf(printpdf 系现成布局层)最后发版停在 2021-06-17 且钉 printpdf ^0.3.4 旧线,不可用;④krilla 其余维度全面占优或持平——70 个外部 crates(探针实测;printpdf default-features=false 也有 105)/release 全新编译 15.8s/维护极活跃(0.8.2 于 2026-06-04,150 万下载/90d,作者 typst 生态 LaurenzV,90+ 快照测试+6 款阅读器视觉回归)/MIT OR Apache-2.0/MSRV 1.92≤钉死的 1.98.0/TTC face index 与 CJK 落字探针实跑通过(pdftotext 回读逐字一致)/cargo tree 实测树内无 egui;⑤铁律 2 的验证场是本功能的使命——roadmap 写明 latermd-render「出现第二个消费者时」创建,PDF 就是第二消费者,走 B 则铁律 2 永远停留在纸面。**A 路线的代价如实承认:布局全部自建(B 路线的 fullbleed 引擎内建 @page/页眉页脚,布局工作量近乎为零),这是本选择最大的成本项,ADR §5 已设「CJK 断行+段落流+表格任一里程碑两工作日不可演示即回看」的检查点。**
- **如何改**:要切到 B 路线(fullbleed)——用户在本文档或 adr-006 上批注追认 B,按 adr-006 §5 复活条件核对(fullbleed 成熟度/CJK 一等公民/CSS 覆盖对齐),把 adr-006 改版为 B 决议(后端 fullbleed 2.5.6、运行时注册 fonts.rs 候选表字体、CSS 写 print 变体),latermd-render 创建推迟;要留 A 但换 printpdf 后端——需先解决其 TTC 子集化 13MB 实测问题(上游 issue 跟踪或改嵌独立 TTF),否则维持 krilla;要改 krilla 版本/特性——改 adr-006 §6 表(实现模块落地时按 #4 口径同步登记 ADR-004 §2 与 docs/README.md 索引,本草案未动这两个文件)。

## #95 M3 设置页三选一的四处落地口径:切 provider 出厂值跟随(手改不动)、空端点按当前 provider 回落、connects_network 与 requires_key 解耦、旧拼法 openai_compatible 加 alias(2026-10-04,#20 ai-adapters M3·自动拍板)

- **岔路**:①设置页把 provider 从 OpenAI 兼容切到 Anthropic/Ollama 后,表单里的端点/模型/超时怎么办——原样保留(切完保存即坏配置:provider=Ollama 端点还是 api.openai.com)、无条件重置为新家出厂值(用户为前一家手改的网关地址被静默冲掉)、还是仅当当前值仍是「任一 provider 出厂值」(即用户从未手改)时才跟随;②`normalize()` 的空端点/模型回落——旧版回落全局默认(OpenAI 出厂值),对 provider=Ollama 的手改 JSON 回落 api.openai.com 是错的;③`connects_network()` 旧实现 = `requires_key()`,Ollama 无 key 也连本机 11434,两维是否解耦;④旧 `ai.json` 的 provider 字段:serde snake_case 把 `OpenAiCompatible` 落成 `open_ai_compatible`,而任务书写的是 `openai_compatible`(无下划线),按哪种拼法保兼容。
- **备选**:①a 原样保留;①b 无条件重置;①c 出厂值跟随(手改不动)。②a 全局默认;②b 当前 provider 出厂值。③a 维持 =requires_key;③b 解耦为 =uses_settings()(唯 Mock 不联网)。④a 只认旧落盘名 `open_ai_compatible`;④b 再加 `#[serde(alias = "openai_compatible")]` 两种拼法都收。
- **自动选择**:①c + ②b + ③b + ④b。`AiConfig::adopt_provider_defaults(new)`:端点/模型/超时若等于任一 provider 的出厂值则换成 new 的出厂值(含 Ollama 超时 120 本地推理档),手改过的字段原样保留,绝不静默覆盖;`normalize()` 空端点/模型回落**当前 provider** 的出厂值;`connects_network()` 改为 `provider.uses_settings()`(Mock 唯一 false,设置页端点行对 Ollama 也显示);`OpenAiCompatible` 变体加 alias,`open_ai_compatible`(旧版真实落盘名)与 `openai_compatible`(任务书拼法)都能读,落盘仍写 `open_ai_compatible`。出厂值单一事实源:`ProviderKind::factory()` 直接取 latermd-ai 各 adapter 的 `Default` 实现,app 侧不另抄常数。
- **理由**:①c 是「不丢用户输入」与「不产出坏配置」的交集:出厂值跟随覆盖了全新安装与从未手改的主流路径,手改值可见可改,静默覆盖是配置页最伤信任的行为;②provider 换了,「空值该回到哪」的语义自然跟着换,回落 OpenAI 地址对新 provider 是二次伤害;③Ollama 的端点是真实配置项(默认 http://127.0.0.1:11434),端点行不显示会让用户以为没生效;requires_key 驱动命令闸门、connects_network 驱动端点显示,本就是两个问题;④alias 只影响反序列化,零成本兜住两种拼法,向后兼容对实际旧文件(`open_ai_compatible`)与任务书口径(`openai_compatible`)同时成立。
- **如何改**:①要无条件重置,把 `adopt_provider_defaults` 里三段 `factories.iter().any(...)` 判断删掉直接赋值(接受手改值被冲掉);②把 `normalize()` 里两处 `factory.base_url`/`factory.model` 换回 `Self::default().base_url`/`model`;③把 `connects_network()` 改回 `self.provider.requires_key()`(Ollama 端点行随之不显示);④删变体上的 `#[serde(alias = "openai_compatible")]` 并同步删 `legacy_ai_json_with_api_style_loads` 里无下划线拼法的断言分支。

## #94 M3 provider 与 ApiStyle 的关系收敛:删除 ApiStyle 字段,ProviderKind 成为唯一开关(2026-10-04,#20 ai-adapters M3·自动拍板)

- **岔路**:任务书要求「优先让 ProviderKind 成为唯一开关、api_style 随 provider 派生」,并授权按小表面积原则取舍「保留或删除 ApiStyle 字段」。三条路:①保留 `AiConfig.api_style` 字段 + `normalize()` 派生覆盖(每次读写都把派生值写回去);②保留字段但只作展示、保存时强制对齐;③整字段删除,协议形态信息移进 provider 的 label/description,UI 的「接口方式」下拉随之退役。
- **备选**:①字段 + 派生回写;②字段仅展示;③删除字段。
- **自动选择**:③。`ApiStyle` 枚举、`AiConfig.api_style` 字段、`normalize()` 的「未实现回落 ChatCompletions」段、设置页「接口方式」下拉与「尚未实现」警示行全部删除;provider 下拉显示名改为带协议形态(「OpenAI 兼容(/chat/completions)」「Anthropic(/v1/messages)」「Ollama 本地(/api/chat)」,Ollama 从任务书点名的错误标签 `/api/generate(未实现)` 更正为实际实现的 `/api/chat`),新增 `ProviderKind::description()` 说明行承担 key 需求与端点性质提示。旧 `ai.json` 里遗留的 `api_style` 键读取时被 serde 忽略(结构体未开 `deny_unknown_fields`),向后兼容不受影响。
- **理由**:四种 provider 各自钉死一种协议后,`api_style` 是 100% 可从 `provider` 派生的冗余状态——保留它,手改 JSON 就能造出「provider=anthropic + api_style=chat_completions」的矛盾组合,归一化与 UI 都要为「对齐派生值」写防御代码,表面积不减反增;它还是「未实现」误导文案(Anthropic /v1/messages 未实现、Ollama /api/generate 未实现)的载体,M1/M2 落地后这些状态已不存在,连字段一起删才删得干净。删除后配置文件少一个字段、UI 少一个下拉、normalize 少一条回落规则,符合任务的小表面积授权。
- **如何改**:要恢复「同一 provider 多协议」的将来形态(如 OpenAI 兼容端点同时支持 Responses API),把 `ApiStyle` 枚举与字段加回 `ai_config.rs`(serde snake_case),`set_provider` 分派处按 (provider, api_style) 二元组 match,设置页恢复第二下拉;git 历史里本条目的删除 commit 即完整反参照。

## #93 M2 Ollama 适配器的四处口径:超时默认 120(非范本的 60)、num_predict 取 i32(负值合法)、error 行文案带「Ollama 流失败」前缀、complete_sync 恒非流式照 anthropic 口径(2026-10-04,#20 ai-adapters M2·自动拍板)

- **岔路**:①`timeout_secs` 默认值任务未规定,openai/anthropic 范本共用 60,但 Ollama 是本地推理——冷启动先要把几 GB 模型载入内存(CPU 机器数十秒);②`num_predict`(= 各家 max_tokens)类型未规定:openai 的 max_tokens 是 `Option<u32>`,而 Ollama 的 num_predict 负值是协议合法取值(-1 不限、-2 填满上下文窗口);③NDJSON 顶层 error 行的失败块 delta 文案:裸服务端字符串还是带前缀(M1 #92 ③b 同款问题);④`complete_sync` 的请求体:照 openai.rs 原样塞设置里的 stream(流式配置下会拿到 NDJSON 却按单 JSON 解析),还是照 anthropic.rs 恒按 stream=false 请求。
- **备选**:①a 60(与范本一致);①b 120(本地推理加倍)。②a `Option<u32>`(与 openai max_tokens 同型);②b `Option<i32>`。③a 裸字符串;③b 前缀+字符串。④a 照 openai(设置原样透传);④b 照 anthropic(恒非流式)。
- **自动选择**:①b + ②b + ③b + ④b。timeout 默认 120;num_predict 为 `Option<i32>`(负值原样透传,单测钉了 -1);error 块 delta 为 `Ollama 流失败:{error}`(error 为 null 的行不当错误、非字符串退「未知错误」);`request_body(prompt, stream)` 带 stream 参数,complete_sync 恒传 false。
- **理由**:①60s 对本地冷启动是误伤,用户首次点「续写」大概率撞超时报「AI 请求失败」,不是用户可修的问题;本地慢是常态而非异常,加倍是最小偏离。②u32 表达不了 Ollama 老手常用的 -1;serde 对 i32 透传无成本,配置页将来加下界校验即可。③Chunk 失败约定要求 delta 面向用户,裸 "model 'x' not found, try pulling it first" 在 UI 上无来源上下文;前缀还与 #92 ③b、openai 的「AI 请求失败:{err}」同构,用户能区分错误来自哪家。④openai.rs 那套是被 anthropic.rs 注释点名过的既有瑕疵(流式配置下 complete_sync 必炸),M1 已改对,Ollama 跟随。
- **如何改**:①改 `crates/latermd-ai/src/ollama.rs` `OllamaSettings::default()` 的 timeout_secs(单测 `default_settings_match_documented_defaults` 同步);②类型改 `Option<u32>` 并删「负值透传」单测断言;③改 `error_chunk` 的 format 前缀(单测 `ndjson_error_line_yields_failed_chunk_and_stops` 同步);④把 `request_body` 的 stream 参数删掉改读 `self.settings.stream`(不建议,见 openai.rs 同问题的既有行为)。

## #92 M1 Anthropic 适配器的三处口径:默认型号取 Sonnet 系别名、采样参数默认 None 不发、错误事件文案带「Anthropic 流失败」前缀(2026-10-04,#20 ai-adapters M1·自动拍板)

- **岔路**:①任务要求「model 默认取 Anthropic 当前主力型号,注释说明来源」,但官方 docs.anthropic.com 对本地区域封锁(实测返回 supported-countries 拦截页),取不到官方 model 表;第三方信息混乱(2026-02 有 Claude Opus 4.6 发布报道但均非官方来源,另有未证实的「Claude Mythos」泄露帖),且「主力」有两解——能力旗舰(Opus 级)还是多数任务的默认推荐(Sonnet 级);②temperature/top_p 的默认值未规定:镜像 openai.rs 的 Some(0.7)/Some(1.0),还是 None 不发;③SSE error 事件的失败块 delta 文案格式未规定:裸服务端 message 还是带前缀。
- **备选**:①a `claude-sonnet-4-5`(Sonnet 系 alias);①b `claude-opus-4-6`(能力旗舰,存在性仅非官方报道);②a 默认 None/None;②b 镜像 openai 默认 0.7/1.0;③a 裸 message;③b 前缀+message。
- **自动选择**:①a + ②a + ③b。model 默认 `claude-sonnet-4-5`(alias 自动跟随官方最新快照,不钉日期版本,来源与查证受限事实已写进 anthropic.rs 的字段注释);temperature/top_p 默认 None(不发字段=用官方默认 temperature=1.0);error 块 delta 为 `Anthropic 流失败:{message}`(message 缺失退 error.type,再退「未知错误」)。
- **理由**:①Anthropic 文档口径是 Sonnet 级为多数任务的默认推荐、Opus 留给最难任务;openai.rs 范本默认同为入门级(gpt-4o-mini),「主力」按默认推荐取;Opus 单价比 Sonnet 高数倍,LaterMD 的续写/commit/摘要场景不是 Opus 级任务;alias 形式不受快照迭代失效影响。②官方端点默认值明确,「不发」语义最干净;openai.rs 硬塞 0.7 的理由是「各家兼容端点默认值不一致」,对官方 Anthropic 端点不成立。③Chunk 失败约定要求 delta 是面向用户的错误描述,裸「Overloaded」在 UI 上无上下文,前缀还让用户能区分错误来源(与 openai.rs 的「AI 请求失败:{err}」同构)。
- **如何改**:①改 `crates/latermd-ai/src/anthropic.rs` 里 `AnthropicSettings::default()` 的 model 字段(单测 `default_settings_match_documented_defaults`、`request_body_is_anthropic_messages_stream` 的断言同步改);②同文件 default 的 temperature/top_p 改 `Some(0.7)`/`Some(1.0)` 并同步上述单测;③改 `error_chunk` 的 format 前缀(单测 `sse_error_event_yields_failed_chunk_and_stops` 同步)。

## #91 #48 B2 emoji inline widget 的两处口径:标题里的 emoji 不随标题字号缩放(沿用 vendored 链接基础字体)、手型抑制在 app 侧渲染后压回(不改 vendor)(2026-10-04,#48 emoji-color B2·自动拍板)

- **岔路**:①可行性调查 §3.2 点名要验的「heading 段是否吃到 inline widget」实测落地:吃到了(图片落在标题行内),但 vendored 层对 `Token::Link` 一律传**正文基础字体**(layout.rs `Token::Link` 分支不走 `Token::Text` 的 heading 字号放大),`inline_widget_size`/`layout_link` 拿到的 font 恒为正文档 —— 标题里的 emoji 彩图因此不随 H1-H6 缩放;②vendored 对悬停中的 inline widget **无条件**置 PointingHand(label.rs `handle_hover` 的 inline-widget 分支、layout.rs `render_link_in_ui`),`link_style` 管不到光标,任务书却要求「不吃手型」。
- **备选**:①a 接受(emoji 在标题里按正文字号画,与 wiki:///ai:// 链接文本在标题里的既有行为同源);①b vendor ①类补丁:`Token::Link` 分支继承当前 heading 的放大字体(或引入 current font 上下文),登记 vendor/README 变更表;②a app 侧在 `MarkdownLabel::show` 之后按本帧 emoji widget 区块把悬停光标压回 `CursorIcon::Default`(同面板内后写者胜);②b vendor 补丁给 `LinkStyle` 加 cursor 字段或 inline widget 关手型开关。
- **自动选择**:①a + ②a。标题覆盖断言(preview.rs `emoji_inline_widget_paints_inside_heading_at_link_font_scale`)把两个事实钉成已知如实行为:图片纵向落在标题行带内(生效)、边长与段落档一致(不缩放)。
- **理由**:B2 任务红线「app 侧实现,不改 vendor」直接排除 ①b/②b;①b 还会牵动**所有**链接(含 http/ai://wiki://)在标题里的字形 metrics,影响面远超 emoji;现状并非 LaterMD 缺陷而是上游简化(标题里的任何链接文本都按正文渲染,emoji 只是首次把这个差异**画**出来);②a 的副作用面已收敛:压回只发生在指针落在本帧 emoji 区块内、且写点在 label 渲染之后、同帧后续面板(编辑器/侧栏)之前 —— 后续面板按各自悬停自设光标,不受影响(无头双向断言:emoji 上 Default、普通链接上 PointingHand 均钉住)。
- **如何改**:①要 emoji 随标题缩放 —— vendor/egui_markdown/src/layout.rs `Token::Link` 分支把 `font_id` 换成继承当前 heading 放大后的 format(照 `Token::Text` 的 heading 分支同款推导),按 §6 ①类登记 vendor/README 变更表并跑 check.sh,app 侧零改动(`inline_widget_size`/占位宽度都按传入 font 派生,自动跟随);②要更根治的手型口径 —— vendored `LinkStyle` 加 `cursor: Option<CursorIcon>` 字段(①类),app 侧删掉 preview::ui 里 label 渲染后的压回段与 `emoji_rects` 探针的抑制用途(探针保留供无头断言)。

> **销账注记(2026-10-05,#91 岔路①按「如何改」①收口,标题 emoji 改为随档缩放)**:#48 emoji-color C2 模块以 vendor ①类补丁落地本条「如何改」①——但**影响面按 C2 任务红线收窄**:只缩放 inline widget 的字体入参,不再如原「如何改」行文那样把整个 `font_id` 换成 heading 放大字体。具体:`Token::Link` 新增 `heading: Option<u8>`(parser 透传所在标题块层级;选 parser 透传而非 layout 同块扫描,因纯链接标题 `# [emoji](emoji://…)` 的 token 流没有任何带 heading 的 Text 可扫、flush/分段路径的 token 切片又会切断标题块,启发式两处都漏),`append_link_to_job` 增 `inline_widget_font: Option<&FontId>` 入参,`build_layout` 按 `heading.scales` 同款下标推导传给 `inline_widget_size`/占位 `layout_link`;**标题里的普通链接文本(wiki:///ai:///http)仍按正文字号渲染,逐像素不变**(vendored tests/inline_widget_heading_scale.rs 否决线钉死,正文段落/表格 cell 同零变化)。app 生产代码零改动,如原「如何改」预判;preview.rs 原「边长与段落档一致(不缩放)」断言随行为翻案为「标题档 ≈ 段落档 × H1 2.0」(测试更名 `emoji_inline_widget_scales_with_heading_level`)。本条岔路②(手型 app 侧压回)不在 C2 范围、维持原口径不动。vendor 登记见 vendor/README.md 变更表「标题内 inline widget 随 H1–H6 档位缩放」行,CHANGELOG [Unreleased] 已记。本注记不改动上方原条目,岔路②翻案仍按其「如何改」②。**提交构成注记(2026-10-05 看护复核)**:本条 latermd-md 侧适配(lib.rs `Token::Link` 模式匹配与构造)与 preview.rs 断言翻案实际落在下一 commit `5c90f3e`(#105① minimap 滚轮)内,vendor commit `6ae22b4` 单独 checkout 时 workspace 不可编译(E0027/E0063 缺 heading 字段),`git bisect` 落在 6ae22b4 会得假阳性;最终 head `2732e13` 起六项门禁全绿,非运行时缺陷。

## #90 #48 B1 emoji→emoji:// 链接改写器的三处口径:覆盖集取 emoji_data 全表 272 枚(参数注入 md 层)、豁免扫描走 pulldown 事件区间(而非手写围栏开关机)、载荷原文直书尖括号目标(而非百分号编码)(2026-10-04,#48 emoji-color B1·自动拍板)

- **岔路**:任务书把三处自由度交给实现者:①**覆盖口径**——只改写 `emoji_data` 覆盖的枚,还是全量 emoji 码位段(任务书明示二选一记本表);②**豁免扫描机制**——任务书说「照 inline_image_dests/wikilinks 先例」(两者都是手写 ```/~~~ 围栏开关机),但同段又要求行内代码与链接文本/目标内豁免,先例机制并不覆盖后两样;③**载荷编码**——emoji 原文 URL 编码或等价可逆编码,口径自定。
- **备选**:①全量 Unicode emoji 码位段(RangeTable 式判定,面板外枚也改写);②app 侧手写扫描器(围栏开关机 + 自造反引号配对 + 括号配对);③百分号编码 `emoji://%F0%9F%98%80`。
- **自动选择**:①**latermd-md 新纯函数 `expand_emoji_links(text, covered)`,覆盖集 = `emoji_data::covered_glyphs()`(面板数据表全量 272 枚,含旗帜/带 FE0F 的两字符形态,前缀长优先匹配),app 侧 OnceLock 注入**;②**豁免区间走与渲染同一套 pulldown-cmark(同 vendored options)的事件区间**——CodeBlock(围栏+缩进+info string)/Code(行内)/Link/Image(构造区间含文本与目标)/Html 块与行内标签/脚注引用与定义,首字符快路径无命中不解析;③**载荷原文直书 `[😀](<emoji://😀>)`**,可逆 = 剥 `emoji://` 前缀。
- **理由**:①覆盖集外的字符改写后无纹理可画——B2 的回落是「透明占位不动」,等于该 emoji 从预览**消失**;只有 272 枚有 #47 的 Twemoji 资产(272/272 在位),覆盖集与面板/纹理共用单一数据源,改表自动跟随;参数化注入让 md 层不反向依赖 app 的数据表,归属与 wikilinks/inline_marks 同层。②手写开关机对「行内代码/链接内」豁免要自造 CommonMark 括号/反引号配对,且与渲染器判定漂移——缩进代码块、跨行链接文本、引用式链接都会判错(实测 pulldown 对这些全部给对区间);pulldown 是铁律 1 的唯一解析器,LP2-1 `inline_marks` 已是同款先例;代价是含 emoji 文档每次快照重建多一次全文解析(无 emoji 文档走快路径零成本)。③尖括号目标允许除换行与 `>` 外的一切字符,覆盖集单测钉住不含这些字符(`covered_glyphs_round_trip_through_emoji_links`);与 `wiki://中文目标` 同口径,渲染串人可读;百分号编码要多一对 encode/decode 面且没有要解的冲突。**已知边界(如实)**:链接引用定义 `[😀]: url` 的标签 pulldown 不产事件、不豁免(极罕见,出现时该行变可见文本);流式未闭合 `[` 后紧跟 emoji 的帧会照改、预览多显示一个 `[`(下一帧闭合即自愈);脚注定义整个区间(含正文)保守豁免,定义正文里的 emoji 暂为黑白。
- **如何改**:①要全量码位段——`latermd_md::expand_emoji_links` 的 `covered` 参数改传按码位段展开的集合(或把参数改成谓词),`state.rs` 注入点同步换;②要手写扫描——替换 `latermd-md` `emoji_rewrites` 实现,并补缩进代码块/跨行链接/引用式链接的豁免测试;③要百分号编码——改 `emoji_rewrites` 内 `replacement` 的 `format!` 一处,并与 B2 的 `emoji://` 解码侧同步。

## #89 #24 C3 快速打开浮层的两处落地口径:选中 QuickOpen 自身只发一条翻转(不补第二条)、浮层高度自适应内容但封顶 430(宽度才定死)(2026-10-03,#24 quick-open C3·自动拍板)

- **岔路**:任务书规定「选中命令 → outbox.push(cmd.message()) 并关面板」与尺寸铁律「窗口宽度定死(如 560px,resizable(false) 或 exact-size 手法)」,落地各有一处自由度:①**命令集里含 `Command::QuickOpen` 自己**——照字面「push 消息 + 补一条关面板」会发两条 `ToggleQuickOpen`(它的 message() 就是这条翻转),同帧先后归约 = 关了又开,浮层关不上;②**「宽度定死」之外高度取什么**——严格定高(浮层恒 430px,短列表空撑一大截)还是自适应内容高度(需自证有界,不违反「禁无界组合」铁律的字面——铁律只钉宽度与列表限高)。
- **备选**:①QuickOpen 自身改为「关面板一条在前 + cmd.message() 一条在后」(顺序避开双翻)、或引入专用 `QuickOpenClose` 消息、或浮层候选里干脆排除 QuickOpen 命令;②严格定高 430(内容尾部 allocate 剩余高度)、或高度完全交给内容(不设上限,靠组上限 8 自证有界)。
- **自动选择**:①**选中 QuickOpen 自身只发 `cmd.message()` 一条,不补关面板那条**(其余命令照旧补一条 `ToggleQuickOpen`);②**`fixed_size([560, 430])`:宽度由此定死(输入框 `desired_width(f32::INFINITY)` 撑满,实测恒 560);高度端 egui 对不可调尺寸窗口本就按内容自适应(`Resize::end` 的「Probably a window」分支),`WINDOW_H=430` 作为内容布局区上限封顶,短列表自然收缩**。
- **理由**:①两条同帧先后归约无顺序能避开「关→开」(翻转幂等路径只有「只发一条」);专用消息要动 C2 已合入的 Message 表与归约(扩表面积);排除 QuickOpen 让「浮层能执行命令全集」的任务字面失真——「快速打开」本身也是可执行命令,且它恰好是「再开/已开就关」的自然语义(与 VS Code 同款 toggle)。回归锁:`command_rows_execute_via_message_and_close` 断言 Enter 后 outbox 恰为一条 `ToggleQuickOpen` 且浮层净效果为关。②严格定高在两三条结果的常见场景里留一大块空白,观感差;而「有界」由三层保证——每组条数上限 8(可见行 ≤ 18)、列表 `ScrollArea::max_height(320)`、布局区封顶 430——高度不随文件总数增长,`window_stays_fixed_and_inside_viewport_with_long_list` 钉死「文件 40 → 80 浮层 rect 逐项不变、宽恒 560、高 ≤ 430、rect 含于视口」。
- **如何改**:①要「关面板」语义更显式——给 Message 加 `QuickOpenCloseRequested` 变体(归约只关不开),浮层对全部命令统一 push 它;②要严格定高——浮窗内容尾部 `ui.allocate_space(ui.available_size())` 把 min_size 顶满,或把测试断言改为恒等高度后调整 `WINDOW_H`;要改宽度/限高数值,只动 `ui/quick_open.rs` 的 `WINDOW_W`/`WINDOW_H`/`LIST_MAX_H`/`MAX_PER_GROUP` 四个常量。

## #88 BK2 反向链接面板(#15)的四层口径:触发=帧末快照比对、点击=WikilinkClicked 同链路(载荷=来源剥后缀)、摘要在扫描层产出、内容变更不重扫(2026-10-03,#15 backlinks BK2·自动拍板)

- **岔路**:任务书定了「当前文档变更/仓库根变更时防抖触发扫描」「点击走 Message::WikilinkClicked → open_wikilink 同一条打开文档路径」「每条目显示来源文件名 + 命中行摘要」,落地仍有四处自由度:①**触发怎么埋**——在打开/切换/保存/另存/换根逐入口插桩,还是统一快照比对;②**点击载荷怎么造**——`Backlink` 有来源确切的相对路径,发携带绝对路径的消息直开(如 `SearchResultClicked`)还是按任务字面拼一个 `[[..]]` 风格目标串走 `find_by_name`;③**命中行摘要从哪来**——`latermd_search::Backlink`(BK1)只有路径/行号/目标原文,行文本要么 app 层重读文件、要么扫描层顺手产出;④**重扫的粒度**——文档「内容」变化(敲字)要不要也触发。
- **备选**:①逐入口插桩(`open_path`/`switch_active`/`save_to`/`change_file_tree_root` 各加一行);②面板行点击发 `Message::SearchResultClicked(root.join(path), line_no)`(`open_search_hit` 已实现打开/切标签/读失败弱提示/跳行全套);③app 层渲染时按 path+line_no 重读文件取行(UI 线程 IO);④触发键加 editor revision,内容一变就重扫。
- **自动选择**:①**帧末快照比对**——`State::end_of_logic` 把「(文件树根, 当前文档路径)」与 `BacklinkState::requested_for` 比对,变了才 `invalidate` 顺延 300ms 防抖(与 `layout_written` 比对写盘同手法);根或文档缺失时 `reset`(不再排程,面板给引导提示)。②**遵循任务字面走 WikilinkClicked**——载荷 = 来源相对路径剥一个 `.md`/`.markdown` 后缀(`jump_target`:`a.md`→`a`、`sub/b.md`→`sub/b`),与 `[[..]]` 目标写法对称,`find_by_name` 的「带 `/` 按相对路径直取 / 否则 stem 忽略大小写」两把钥匙都吃得下;来源已打开时 `open_path` 按路径去重切标签,来源被删时 `find_by_name` 落空 → 提示行弱提示,不崩溃。③**扫描层产出**——`Backlink` 增 `line_text` 字段(BK1 纯加法,扫描时全文在手,按行号折算零成本),app 复用侧栏既有 `SNIPPET_MAX_CHARS`(120 字符)截断,UI 线程零 IO。④**内容变更不重扫**——反向链接匹配的是路径不是正文,「谁链接了我」不因我敲字而变;为每次键入全仓重扫不值得,真需要时换文档/换根的自然重扫已覆盖绝大多数场合。
- **理由**:①快照比对小表面积且**不可能漏入口**——后来新增的任何文档变更路径(将来的拖拽打开、MCP 写回)自动被覆盖,逐入口插桩漏一处就是「面板显示旧文档的反向链接」的静默错误;比对在帧末跑,与防抖顺延天然合并。②复用 `open_wikilink` 即复用「无根提示」「找不着提示」「孤儿 draft 检测」(`spawn_tab` 内)全部既有语义,面板零新路由;`jump_target` 的剥后缀口径与 BK1 `strip_md_extension`(#87)一致,反向收进来的写法正向都打得开。③重读文件在后台线程也行,但扫描层已经拿着整份 lossy 串,行文本是现成的;面板与搜索面板(`SearchResult.line_text`)同构。④见岔路④。**已知并接受的边界**:点击**不跳到来源的命中行**(open_wikilink 只开文档,面板行的行号仅作展示);无 `/` 载荷按 stem 全库找,同名文档多篇时可能切到另一篇(`find_by_name` 列表首个命中,#26 既有模糊性);`find_by_name` 受 `MAX_LIST_ENTRIES`(500) 截断,库尾的同名来源可能找不到;**面板列表不随库内容实时刷新**——别的标签里给第三篇文档加链接,当前文档的反向链接要等下次目标变更才更新;扫描为一次性直扫,大仓首扫期间的防抖窗口内多次变更会合并为最后一次(见下条测试)。
- **如何改**:①要逐入口精确触发——把 `sync_backlink_target`(crates/latermd-app/src/state.rs)的比对键换成各入口显式调用;②要点击直开确切文件并跳行——面板行改发 `Message::SearchResultClicked(root.join(link.path), link.line_no)`(`open_search_hit` 已有全套语义,还能跳到命中行),代价是脱离任务指定的 WikilinkClicked 链路;③要摘要在 app 层现算——revert `Backlink.line_text`,渲染时后台读文件(代价:UI 或归约线程 IO);④要内容级实时——触发键加 `(tab_id, editor.revision)`,每帧比对(代价:每次键入全仓重扫,防抖 300ms 只是缓冲,不建议);要面板手动刷新——加一个「重新扫描」按钮发 `Message::BacklinksRequested` 即可(载荷本就取当下状态)。

## #68 标签「缩短标题/完整标题」(#37)的三个口径:作用范围=整条标签条、默认=完整(既有观感)、收窄分配=保底+max-min 公平(2026-10-01,#37 tab-management 标题宽度显示模式·自动拍板;原编号 #67,因重命名条目重编号顺延为 #68,撞号事由见 #67 条头注)

- **岔路**:任务书规定作用范围「以 docs/decisions-pending.md 本轮规划条目为准(单标签或整条标签条),找不到条目则按『整条标签条』最小方案执行并补记决策」——本轮开工前全表 grep「标题宽度/显示模式/缩短/宽度模式/完整模式」无相关条目(最新条目为重命名的别名语义条,时记 #66,后递改 #67),兜底条款触发,作用范围按整条标签条执行;但落地仍有三处自由度:①**默认模式**(缩短还是完整);②**收窄的分配算法**(Chrome 式全部等宽 vs 按各标签完整宽封顶的 max-min 公平分配);③**最小宽数值**(任务书只说「留合理最小宽」)。
- **备选**:①默认缩短(浏览器原生观感);②等宽平分(所有 chip 一刀切同宽,Chrome tab-strip 行为);③最小宽取仅容「…+关闭钮」的 ~36px。
- **自动选择**:①**默认完整(Full)**——`TitleWidthMode::Full` 为 `#[default]`,旧 settings.json 缺 `tab_title_width` 字段经 `#[serde(default)]` 也回落 Full;②**保底 + max-min 公平**——预算内每个 chip 先保最小宽 56px(不超过自身完整宽),余量按「完整宽低者先封顶退出、剩余标签平分」追加(`ui::tabs::share_widths`);③**CHIP_MIN_W = 56px** = chrome 28px(左右内边距 6+4、关闭钮 12、右内边距 6)+ 约 28px 文本(一到两个汉字 + 省略号)。
- **理由**:①默认完整是不改变 #11 已交付观感的保守取值——缩短模式是用户显式开启的新行为,升级不该顺手改掉所有人的标签条;持久化字段进 `ThemeSettings`(settings.json 既有载荷,主题/emoji_recent 同路),不另开文件;②纯等宽会让「a.md」与长标题一样窄,短标签白白损失辨识度,max-min 让短标签先拿满自然宽、挤压由长标题承担,更贴「按可用空间收窄」的任务书原文;③56 装得下省略号与关闭钮并留一两个字符的辨识余量,预算再紧也不跌破(跌破即关闭钮点不中),溢出交给既有单行水平滚动兜底。切换消息 `TabTitleWidthChanged` 只动 `theme.tab_title_width` + `persist_theme()`,归约测试 `tab_title_width_switch_persists_without_touching_tabs` 钉死「路径/缓冲/dirty/别名/标签 id 逐项不变 + 目录里只有 settings.json」。
- **如何改**:①要默认缩短——`theme.rs` 里 `TitleWidthMode` 的 `#[default]` 从 `Full` 挪到 `Short` 即可(连同旧 settings 回落语义:那是「缺字段=出厂默认」,默认改了回落跟着改);②要等宽平分——`share_widths` 删掉按 cap 封顶的排序循环,直接每项取 `min(full, budget/n)`;③要调最小宽——改 `ui/tabs.rs` 的 `CHIP_MIN_W` 常量(注意保持 > CHIP_CHROME + 省略号宽,否则窄窗下关闭钮被挤);④想给单标签独立设宽——需把 `TitleWidthMode` 从 `ThemeSettings` 挪进 `TabState`(每标签一份)并在右键菜单按被右键标签读写,与「重命名」别名同层,但会失去「一条标签条一种节奏」的整体观感,不建议。

## #67 标签「重命名」(#37)的语义:显示别名而非真实文件改名——非法名只拒空白、别名可重复不查冲突、不持久化、作用范围=标签条 chip 与关闭确认模态(2026-10-01,#37 tab-management 标签重命名·自动拍板;**编号说明**:本棒自记 #66,与本文件已合入的「查找条 Esc 关闭」条目(#17 M1,随 PR #83 进 main)撞号——彼时该条追在文件末尾且文件头「最新条目」行未同步(仍写 #65),自记时未见撞号;2026-10-01 文档收尾递改本条为 #67、标题宽度条顺延 #68,Esc 条目保持 #66 不动)

- **岔路**:auto-plan #37 行只写「实施时明确『重命名』是实际文件改名还是标签显示别名」,本轮规划未在 decisions-pending 拍板(全表 grep「重命名/改名/别名/rename/alias」无相关条目,最新条目 #65 是孤儿 draft 恢复条)。任务书据此规定「找不到该条目则按『显示别名』最小方案执行,并把岔路/选择/理由补记新编号」——语义已由任务书定死为别名,仍留下四处落地自由度:①别名的「非法名」边界(任务书为文件改名列的「空/含路径分隔符/前后空白」是否照搬);②文件改名的「同名冲突拒绝」在别名语义下的对应物;③别名的作用范围(哪些 UI 位跟随别名);④别名是否持久化。
- **备选**:①照搬文件改名的全部非法名(含拒绝 `/` 与路径分隔符);②别名也做同名查重拒绝;③作用范围扩大到窗口标题/状态栏/文件树;④别名随标签持久化(会话重启存活)。以及上位的另一大岔路:真实文件改名(协调路径去重/文件树/保存目标/draft 迁移,任务书 1) 的完整清单)。
- **自动选择**:①**只拒空白**——草稿 trim 后为空即拒绝(浮窗保留、提示行说明,UI「确定」按钮在空草稿时已禁用,归约兜直发消息);`/` 等路径分隔符**不拒**,因为别名是纯显示文本、永不落盘,没有「拼进路径」的攻击面,照搬文件名的非法表只会让用户困惑「为什么浏览器标签能叫 a/b 这里不行」;前后空白 trim 掉(显示文本留首尾空白毫无意义)。②**别名可重复**——两个标签允许同名别名(单测 `tab_rename_duplicate_alias_is_allowed` 反向钉死),「同名已存在即拒绝」防的是文件系统覆盖,纯显示文本没有可覆盖的东西;别名与路径解耦后,同一路径在不同会话/不同标签可挂不同别名,互不牵连。③**作用范围 = 标签条 chip 文本 + 脏标签关闭确认模态文案**(模态问的是「关闭这个标签」,用户在标签条上认的是什么名字模态就问什么,`tab.display_name()`);**窗口标题、状态栏、文件树、保存/另存为对话框一律不跟随**(仍显示文件名)—— 窗口标题与状态栏表达落盘身份,且是别名盖住 chip 后用户找回真实文件名的锚点;浮窗内常驻一行「文件:<名>(重命名不改动它)」明示作用范围,兼作文件名的可视锚。④**不持久化**——标签本身(开过哪些文档)就没有会话持久化,单独持久化别名会造出「重启后别名还在、标签没了」的孤儿数据;别名随标签生灭(会话内状态)。
- **理由**:别名是最小方案,零文件系统操作即零失败面(任务书 1) 的「文件系统失败 → 路径/缓冲/dirty 全部保持不变」由「根本不做文件操作」平凡满足);真实文件改名要协调四件事(路径去重口径、文件树刷新、保存目标、#18 draft 迁移),任何一件漏掉都是数据事故面,而用户诉求(「这个标签我想叫它初稿」)用显示层就能满足,要改真名用户有文件树/系统文件管理器。落点:`TabState::alias`(#37 纯显示层)+ `TabsState::rename` 浮窗状态(稳定 id,`remove` 时与 `confirm_close` 同款撤下)+ 归约 `request_tab_rename`/`confirm_tab_rename`(crates/latermd-app/src/state.rs)+ 浮窗 `ui::tabs::rename_dialog` + chip 显示名改走 `tab.display_name()`。别名置入前后 path/缓冲/dirty 逐一不动由 `tab_rename_sets_alias_display_only`/`tab_rename_rejects_blank_name` 钉死;保存目标不跟随由 `tab_rename_alias_keeps_save_target_and_disk_untouched` 钉死(Ctrl+S 仍写原路径、目录里不出现别名文件);`#[37 别名不改窗口标题]` 由装配测试 `rename_dialog_wired_into_overlay_draw` 钉死。
- **如何改**:①若想要真实文件改名——新开一棒实现 `Message::TabFileRename`(走任务书 1) 全清单:无覆盖目标、同目录名字校验、失败不变式、成功后 `find_by_path` 口径不变但路径换新值、文件树刷新、保存目标切换、`named_draft_path` 草稿迁移),与本别名并存(右键菜单里「重命名」管别名、「在文件管理器中改名」管真名,或把本条目回删);②想禁止 `/` 入别名——`confirm_tab_rename` 加 `name.contains('/')` 分支拒绝即可(连同 UI 侧 `rename_dialog` 的 ready 判定与两条单测);③想让窗口标题跟随别名——titlebar.rs 与 layout.rs 的 `window_title()` 调用换成 `tab.display_name()` 前缀拼接,并同步删装配测试的「不跟随」断言;④想持久化——需先给标签会话(路径列表+active)建持久化文件,别名作为其字段随行,单独做别名持久化无意义。

## #65 孤儿 draft 恢复条(#18)的五处口径:直接编辑即撤条且 draft 保留、读取失败撤条留现场、丢弃无确认、行内条而非浮层、相对时间文案(2026-10-01,#18 孤儿 draft 恢复条·自动拍板)

- **岔路**:①任务书只说「用户无视恢复条直接编辑时 draft 保留」,没说条本身去留;②「恢复」读 draft 失败(坏内容/权限/外部删走)后条的去留;③「丢弃」是否要确认模态(删盘上文件不可逆);④恢复条形态——编辑区顶部行内条(下推编辑器)还是浮层;⑤「保存时间」文案——本仓无 chrono/time 依赖,绝对时刻格式化无依赖可用。
- **备选**:①条一直留到显式点「恢复/丢弃」;②失败后条保留可重试;③丢弃前弹确认;④浮层(Area)或对话框;⑤引 chrono 格式化本地时刻 / 显示 unix 时间戳。
- **自动选择**:①缓冲一旦变脏(帧末 `autosave_pass` 判定)即撤条、**draft 文件不删**(停顿/切出路径照常另写);②失败撤条、draft 文件保留现场、提示行带路径与原因;③无确认直接删,提示行留痕(`已丢弃未保存草稿(<路径>)`);④行内条,画在标签条/提示行之下的 CentralPanel 流里(与提示行同族);⑤相对时刻纯函数(`draft_saved_label`:刚刚保存 / N 分钟前 / N 小时前 / N 天前,mtime 缺失或时钟倒流一律「保存时间未知」)。
- **理由**:①条的存在意义是「盘上版本 vs 遗留稿」的二选一,用户动手编辑就是投了前者,继续挂着只会诱导一次「拿旧稿盖掉新稿」的误点;draft 保留是任务书明文,防丢镜像交停顿路径接管(单测钉了 30s 后新缓冲覆盖旧稿);②坏内容是确定性失败,每次点都一样,留一个永远兑现不了的按钮不如一次说清;文件保留现场,用户尚可手工捞;③丢弃的是「从未保存过的内容」,拒绝它等价于它从未发生,盘上版本分毫未动——无可撤销是因为无可损失,加确认模态反而把最常见路径(打开文档→不想要旧稿)变重;④恢复条是需要持续在场的裁决入口,行内条不可被滚动顶走、不与查找浮层/对话框抢 z 序,位置语义与提示行一致;⑤一个展示字段不值得引入时间依赖,相对时刻对「这稿子多新」的判断也比绝对时刻更直接。
- **如何改**:①想改为条长挂——删 `autosave_pass` 循环里 `if tab.recover.is_some() && tab.editor.is_dirty()` 那三行(state.rs);②想改为失败可重试——把 `recover_draft` 的 Err 分支里的 `tab.recover = None` 删掉即可;③想加确认——仿 `confirm_close` 的稳定 id 模态,确认后才调 `discard_draft`;④想改浮层——把 `recovery_bar` 的调用从 CentralPanel 流里挪进 `draw_find_overlay` 同款的 Area(锚 `source_rect`);⑤想显示绝对时刻——给 latermd-app 加 `chrono`(需登记 ADR-004 依赖清单),`draft_saved_label` 一处替换。



## #64 自动保存「切标签(切出)」是否覆盖「开新标签」(#18 autosave-core·自动拍板)

- **岔路**：任务书原文「当前标签 dirty 且【距上次改动 ≥30s 常量 或 本帧发生切标签(切出)】→ 写 draft」。多标签 #11 的既有语义是「打开文件**永远开新标签**、新建也开新标签」（`TabsState::open_tab` 直接推进 active，不经 `switch_active`）——「开新标签」是否算「切出」直接决定打开另一文件瞬间原标签的脏缓冲有没有 draft 保障。
- **备选**：①只认显式切换入口（标签条点击 `TabActivate` / `Ctrl+Tab` 的 `TabNext`），开新标签不算；②两类都算：`switch_active` 与 `spawn_tab` 都记切出标签 id，帧末统一判定。
- **自动选择**：②。`State::autosave_switch_out` 单槽记录切出方的稳定 id，两个入口都写它。
- **理由**：切出的防丢语义是「标签不可见后缓冲失去看护」，打开另一文件时原标签同样不可见，与点了标签条没有本质差别；只认①会在「打开文件」这个最高频的离开路径上漏掉保障。单槽的局限（同帧连切两次只记最后一位切出者）由停顿路径兜底，不为罕见序列引入 Vec。
- **如何改**：若认为开新标签不该触发（保持①）——删 `spawn_tab`（crates/latermd-app/src/state.rs）里的 `self.autosave_switch_out = Some(...)` 一行即可；若要覆盖同帧连切——把单槽换成 `Vec<u64>` 并在 `autosave_pass` 里去重消费。

## #63 未命名文档 draft 的文件名形态:标签 id 而非时间戳/内容哈希(#18 autosave-core·自动拍板)

- **岔路**：规格只定了「未命名文档落 `config_dir()/drafts/`」，没定文件名。同一会话可开多个未命名标签，文件名形态决定它们是否互不覆盖、以及用户翻状态目录时看副不看得懂。
- **备选**：①`untitled-<标签id>.latermd-draft`（`TabsState::next_id` 发放，稳定不复用）；②时间戳；③内容哈希。
- **自动选择**：①（`State::untitled_draft_path`，crates/latermd-app/src/state.rs）。
- **理由**：标签 id 稳定且不复用（#11 的既有不变量，undo/光标隔离同款前提），同一标签的 draft 永远写同一位置、多标签互不覆盖；时间戳每次落盘换文件名，会积累一串孤儿；内容哈希同一标签连续编辑也换名，还把「防丢镜像」变成了「版本历史」（那是 Git 的职责，不是本功能）。代价：id 数字对用户无语义，崩溃后孤儿文件只能靠恢复条（下一棒）按「存在即候选」处理，不能按文件名猜时间。
- **如何改**：想让用户翻目录时能看懂——文件名加首写时刻（如 `untitled-3-20261001.latermd-draft`，id 前缀保持唯一性），改 `untitled_draft_path` 一处即可；想按时间找孤儿——在写 draft 时顺带维护 `drafts/index.json`（id → 路径与时刻），恢复条消费它。

## #61 代码块复制按钮(#38)的六处口径:挂载点上游已有零 vendor、常驻显示、语言标签取 info 首词、空块两形态、反馈按内容指纹、无语言块也留按钮(2026-09-30,#38 codeblock-copy·自动拍板)

- **岔路**:①甲案预设「在 vendored 代码块 widget 头部加可选 copy 按钮挂载点(独立 vendor: commit + 登记 vendor/README.md)」,动手探测发现 subtree 引入的上游代码本就带 `MarkdownLabel::code_block_buttons(Fn(&mut Ui, &str, &str))` 挂载点(subtree 基点 8e58d97 即有,label.rs 两条代码块渲染路径都调用,回调直接给 `(块源文本, 语言)`);②头部 hover 显示 vs 常驻;③语言标签显示完整 info string 还是首词;④空代码块形态(pulldown 对无空行的空围栏不产 Text 事件,vendored parser 无 token、整块不渲染;带空行的空块 text 为空串、块存在);⑤「已复制」✓ 反馈态的键控(widget id 还是内容);⑥无语言块显示按钮与否。
- **备选**:①按任务书字面在 vendor 再造一个 hook(与上游既有挂载点重复、多一个 vendor: commit 与登记行);② hover 才显示;③显示完整 info;④给空块强行造按钮;⑤按 egui widget id 键控;⑥无语言藏按钮。
- **自动选择**:①零 vendor 改动,直接消费上游挂载点(甲案意图——通用挂载点、按钮视觉 app 侧注入、点击经回调出 app 侧——完全被既有 API 满足);②常驻;③首词;④带空行空块留按钮、点击复制空串,无空行空块随上游语义不渲染(只验不 panic);⑤全局单份「最近复制块指纹+到期时刻」存 egui data,按内容认领;⑥无语言块按钮照旧(只是没有标签行)。
- **理由**:①重复造 hook 违反「以现状为准」;上游回调直接给源文本,任务书「经 code_block_spans 切 rendered」的绕路不再需要(少一处偏移换算出错面);②常驻可发现性好(GitHub/Gitea 同款),hover-only 在无指针环境不可达;③完整 info 可带 `title=` 等元数据,不是语言名;④⑤vendored 挂载点的子 Ui id 按帧内序号自动分配,文档编辑后块序平移会让 id 键控的反馈错位到别的块;内容指纹最多让同文本多块同显 ✓(与 ```ai 卡片按指令文本认领状态同款简化,decisions-pending #13 先例),且跨视口(右栏预览与 Live 中央富渲染)同显合理——同一块内容;⑥交互一致性优先于按内容藏控件。
- **如何改**:①若坚持 vendor 侧出按钮皮肤——把 `code_copy_buttons`(crates/latermd-app/src/ui/preview.rs)的绘制逻辑下沉进 vendored ①类改动并按 AGENTS §6 登记;②改 hover-only——在 `code_copy_buttons` 里按 `ui.ctx().input(|i| i.pointer.latest_pos())` 与 rect 的包含关系门控绘制;③改完整 info——`lang.split_whitespace().next()` 换 `lang`;④想让无空行空块也渲染——需 vendored parser 对空 CodeBlock 补 token(①类,另登记);⑤改 per-块反馈——把指纹换成 (tab, 块序号) 二元组键;⑥藏无语言块按钮——`lang.split_whitespace().next().is_none()` 时提前 return。

## #60 切换卡顿(#39 M2)四候选修法的取舍:per-tab 缓存槽位+heal 条件化落地,首屏分段与后台预热不做(2026-09-30,#39 tab-switch-perf M2·自动拍板)

- **岔路**:M1 根因锁定「切换帧 vendored 预览缓存全量 miss」后,任务列了四个候选修法:rendered/heal 按 (tab, rev) 缓存、heal 仅流式启用、大文档首屏分段(视口外延迟)、TextEdit 切换预热;另有「切 tab 后台预热下一文档解析」。实施时发现四者并非并列:①归约/快照侧(rendered/outline)本来就按 rev 缓存(PreviewState 只在 revision 前进时 rebuild),无重复成本可省;②真正的命中障碍是 vendored 缓存全部挂同一个常量 widget id("preview-md"),切 tab 时槽位被另一文档的 hash 覆盖——这可以在 app 侧解(id 加 tab 维度)也可以在 vendored 侧解(缓存搬出 temp memory 做跨文档 LRU);③「大文档首屏分段」必须改 vendored flush_text_range 的 miss 路径(miss 帧没有段高缓存,跳过布局会塌、用估计高度会跳),是①类中等改动且破坏布局连续性;④「TextEdit 预热」经核实不需要——editor.rs 的 TextEdit 早已用 `tab_editor_id`(per-tab),往返天然命中。最大的实测岔路:M1 基线「切换后首帧 9.45ms」与本次改动后 92ms 的对比表面上是回归,实为 harness 同构样本的假象(sample_doc(1)/sample_doc(2) 模板相同,大量段落 token 内容一致,旧路径同 id 换文本时段级 ctx_hash 命中 A 残留缓存,6.95ms;换成同规模非同构文档对照,旧路径首切实测 92.4ms,与 per-tab id 的 91.5ms 持平)。
- **备选**:A 只做 app 侧:per-tab widget id + heal 条件化,首切成本如实报数字,首屏分段/预热登记后续路线;B 连 vendored 一起改:缓存迁出 temp memory 做跨文档 LRU + miss 帧视口外延迟布局(①类,~几百行,含布局正确性风险);C 加后台预热:打开文档时空闲帧预渲染(无免费午餐——成本只是移到别的帧,galley 非 Send 做不到线程池)。
- **自动选择**:A。附带一个顺手的正确性修复:预览 ScrollArea 的 id_salt 同步 per-tab(原常量 "preview-scroll" 下所有标签共享滚动偏移,切 tab 滚动位置互相踩;per-tab 后各标签记住各自的滚动位置——egui temp memory 无按帧回收,已实测跨往返存活)。
- **理由**:①egui 0.36 `IdTypeMap` 实证无按帧 GC(temp 仅在 clear/remove 时清,egui src/util/id_type_map.rs 与 memory/mod.rs begin_pass/end_pass 均不触碰 data),per-tab id 即可让 vendored 缓存跨切换存活,零 vendor 达成任务预期;②首切 miss 是「该文档第一次被渲染」的固有成本,预热只是把卡移位不消除,分帧预热又有布局塌陷/滚动条跳动问题,两者都违反「不做不达标优化」的务实原则;③2000 行口径往返切回 9.38→2.82ms、20000 行往返 41.2ms,稳态帧 594.7→458.5µs(heal 条件化),改善达标(以上为 M2 当轮数字;**2026-09-30 评审补测修正**:原括注「旧路径真实场景同场景为 900ms+ 量级」系机制推断且量级有误——harness [E] 新增旧路径对照行(常量 id 单槽,doc7 覆盖后切回 big 文档)release 实测 20000 行往返切回 72.1ms(单件),900ms 量级实为「该文档从未渲染过」的冷首切(847–927ms),新旧路径同付,本改法改善的是往返与稳态而非冷首切;同轮 release 复跑 2000 行往返 6.29ms(旧机制单件)→2.33ms(整帧)、20000 行 72.1→31.1ms,结论 A 不变);④20000 行首切 926.8ms>100ms 属「不达标」项,按任务要求如实进 notes 并给出后续路线(vendored ①类:miss 帧视口外段延迟布局,或打开时空闲帧逐段预热),不隐瞒不糊弄。
- **如何改**:若要首帧也达标——授权 vendored ①类改动(label.rs `flush_text_range` miss 路径加视口外延迟 + `CachedMarkdownLayout`/`CachedFlushRange`/`StreamingCodeCache` 迁到跨文档 LRU,按 AGENTS §6 三分类独立 vendor: commit 并登记 vendor/README.md);若接受首切一次卡顿的现状——本条可回删;若要恢复滚动位置跨标签共享的旧行为——把 preview.rs `tab_preview_id` 相关的 ScrollArea `id_salt` 改回常量即可(MarkdownLabel 的 per-tab id 不受影响)。

> **销账注记(2026-10-06,#59 perf-round M2+M3)**:本条「如何改」授权的 vendor ①类路线已按其**可安全子集**落地大头——视口外代码块高亮延迟(`defer_offscreen_highlight`,几何/文本逐字节相同仅颜色缺席,无高度估计、无校正无跳屏,perf-recheck §11.2):20000 行冷首切 855.02→143.42 ms(**6.0×**,tab_switch 整帧口径 916.61→194.87 ms)、2000 行 85.22→18.57 ms(**§10.6 验收「84→<20 ms」达标**);M3 收口复跑逐字吻合(perf-recheck §12:20k 冷首帧 146.23 ms、2k 19.14 ms、tab_switch 20k 首切 195.85 ms,bench 三项 criterion No change)。**20k 对「≥8× 即 <110 ms」未达(6.0×),如实保留**:剩余 ~136 ms = miss 帧对全文档(~645 KB)的全量 shaping——shaping 本身就是精确测高,继续压缩必须做本条备选 B 的完整形态(估计高度型视口外延迟布局,含占位/后帧校正/滚动锚定与块表几何契约修订),按 perf-recheck §10.6「大改,独立拍板」挂 **#111**,后续路线以 #111 为准,本条不再重复维护。原条目其余三项口径不变:跨文档 LRU 未做(per-tab id 语义保留,#42 时代现状);后台预热维持「不做」(§理由②);滚动位置 per-tab(顺带修复)维持。「20k 首切不达标」自本注记起由 #111 承接追踪。

## #59 评审修复棒被禁 commit,却被要求把 README.md 补段送进 commit——以「点名暂存+借道编排收口提交」闭合(2026-09-30,#22 cask-bump 独立评审修复·自动拍板)

- **岔路**:独立评审 finding(medium)指出 M2 docs commit 6932052 声称「失效下载手动路径补 README」,但 README.md「brew 拿不到新版本时」bullet 仍在工作区未提交;按现状推送则补段不随分支走,docs/distribution.md §3.4 的「README 安装节有同款面向用户的说明」在 main 上失引用,finding 开出的修复是「推送前 `git add README.md && git commit`」。但本修复棒被明令「不要自己执行 git commit(提交由编排按模块声明路径收口)」,且编排收口路径集=模块 paths 并集(`.github/workflows/macos-dmg.yml`+`docs/`),不含 README.md——字面守约束则该 finding 修不了;更糟的是编排收口后有全局 `git status --porcelain` 残留检查(本 run 脚本 `.zcode/workflow-runs/dwfrun-a8bf3e81-*.mjs` 的 st2 步),README.md 留在工作树会让整个 run 在收口时 stopRun 中止,push/PR 全部报废。
- **备选**:A 不动 README,summary 如实列剩——run 大概率在残留检查中止,交付报废;B 只 `git add README.md` 点名暂存(内容一字不动),登记本条;修复棒自身在 docs/ 有本条登记改动,修复棒收口提交必然触发,而编排 `commitPathsIfDirty` 的 `git commit` 无 pathspec、提交整个 index,暂存的 README.md 随之入库;C escalate 等人工裁决。
- **自动选择**:B。
- **理由**:①README.md 改动是本 run 自己 M2 模块按任务书第 2 点(明文「与主仓 README 安装节各补一段」)写就,已在 #58 登记为有意改动,评审确认「内容本身已登记、问题仅是未进 commit」——不是他人 WIP,不触「绝不混入」;②「不要自己执行 git commit」的意图是把提交收口权留给编排,`git add` 不在禁用清单(仅禁 `git add -A`),单文件点名暂存恰是 finding 开出的修复动作,最终落 commit 的仍是编排的收口提交;③编排 `commitPathsIfDirty` 先 `git add -- <声明路径>` 再 `git commit`(无 pathspec)是脚本既有语义,点名暂存是让已登记改动随收口入库的受支持通道;④不修则残留检查必中止整个 run,损失远大于「暂存一个已登记文件」的口径争议。
- **如何改**:若认为本棒无权暂存 README.md——`git reset HEAD README.md` 后按 #58 的回退方案处理(回删 README「brew 拿不到新版本时」bullet,失效下载说明以 distribution.md §3.4 为唯一落点);若认可入库但要求独立 commit——README.md 已随本棒收口提交入库,可事后 rebase 拆分;本条在确认 README.md 已入库后可回删。

## #58 M2 文档棒被点名改 README.md,而「改动面只有 macos-dmg.yml 与 docs/」——同文两口径冲突的解读(2026-09-30,#22 cask-bump M2 文档对齐·自动拍板)

- **岔路**:#22 收官的 M2 任务书 2) 明确要求「在 distribution.md 该小节与主仓 README 安装节各补一段」失效下载说明,同一任务书公共约束又写「改动面只有 .github/workflows/macos-dmg.yml 与 docs/」——README.md 两个口径都不包含,直接矛盾(与 #54 同型:模块 paths 与任务文本点名 README 的冲突)。
- **备选**:A 按「改动面」句跳过 README,失效下载说明只落 distribution.md;B 改 README 补段并登记本条;C escalate 等人工裁决。
- **自动选择**:B。
- **理由**:任务 2 是本模块三大目标之一(模块标题即「失效下载说明」),README 是用户侧说明的第一入口,不改则任务 2 只完成一半;「改动面」一句的可辨认意图是排除其余 workflow(同句逐一列出 release.yml/auto-tag.yml/linux-deb.yml/rust.yml)与代码,README 属文档面不在排除语义内;#54 前例里 README 改动只是次要纠偏且彼时 paths 确为 ["docs"],本次不同——README 补段是主交付物。补充内容全部为既有实测口径的复述(universal2 dmg 资产、xattr 命令、tap cron 兜底),无新断言。
- **如何改**:若评审认为该棒无权动 README,回删 README.md「安装」节新增的「brew 拿不到新版本时」bullet 即可,失效下载说明以 distribution.md §3.4 为唯一落点,其余改动不受影响。

## #57 #22 cask 回填落地:与 tap 侧 auto-bump cron 双写同一 cask 文件的去留(2026-09-30,#22 cask-bump·自动拍板)

- **岔路**:docs/distribution.md §3.4 与 auto-plan.md:94 已核实 tap 仓 auto-bump 含 CASKS 表、每小时自动跟版 cask version/sha256,主仓 #22 与其功能重复、「撤与留留后续评审」;但 auto-plan 队列(权威清单 priority-queue-2026-09-30.json)仍排 #22 且本棒任务书明确要求在 macos-dmg.yml 实现 cask 回填步——照做(与文档「重复」结论冲突)还是停手等评审,是本棒第一岔路。次级岔路:任务书要求 `?ref=main` 读 cask,实测 tap 仓只有 master 一个分支、`ref=main` 404。
- **备选**:A 拒做引用 #53 等评审;B 按任务书落地,步内注释与 docs 如实标注双写关系与竞态分析,分支按现状取默认分支;C escalate。
- **自动选择**:B;分支读写不钉名字(GET 省 ref、PUT 省 branch 字段,均落默认分支,现为 master)。
- **理由**:#53 只登记冲突未裁决,队列把 #22 排在 #19 之后且本 run 按队列派工,「后续评审」的执行主体是用户不是本棒;该步失败语义为纯告警,落地后与 tap cron 并存无实际风险——两边写同一文件的 version/sha256 两行且值同源同值(sha256 都取自同一 dmg 文件:本地 shasum == GitHub asset digest),contents API 带 blob sha 原子覆盖,撞车最坏是 PUT 409 → 告警跳过、cron 兜底,双方都写不坏文件;ref=main 实测 404(gh api 「No commit found for the ref main」,2026-09-30),按「任务描述与仓库现状冲突,以现状为准」改默认分支,还顺带免疫未来分支改名。
- **如何改**:若评审结论是撤主仓侧——删 macos-dmg.yml 尾部 cask-bump 步与 secrets.HOMEBREW_TAP_TOKEN,回删 distribution.md §3.4/§4 与 auto-plan.md:94 的双路口径即可,tap 侧无感;若撤 tap 侧 CASKS 表项——本步成为唯一回填通道,须把 HOMEBREW_TAP_TOKEN 从可选升为必配并在 §4 发版清单加巡检项;若要钉死分支,把 GET 换回 `?ref=<分支>` 并给 PUT payload 加 `"branch"` 字段。

## #56 IME 补报红线的「空闲帧」口径:按自动路径谓词镜像重定义(2026-09-30,#19 ime-follow 独立评审修复·自动拍板)

- **岔路**:原任务红线写「只在编辑器持焦点且光标位置实际变化的帧上报;失焦帧/空闲帧不发任何 IME 命令」;而独立评审 finding 1(high)指出「持焦点+光标未动+有输入事件」的帧(keyup/鼠标 motion/preedit 未变更新帧)里 egui-winit 自动路径必然重写 spot 到 TextEdit 左上角(其 lib.rs:1173 第二个触发项即「事件非空」),不补报就留下整帧错位(坤哥症状帧类)。字面守「光标变化才报」= finding 1 不可修,两条指令冲突。
- **备选**:A 字面守红线、不修 finding 1(残留症状);B 把「空闲帧」解读为「无输入事件且无任何位移的帧」,触发判定镜像自动路径谓词(事件非空 ∨ 内容矩形变化即补报),红线意图(失焦不抢位、空闲/动画帧不轰炸)保住;C escalate 等人裁决。
- **自动选择**:B。
- **理由**:本棒指令「逐条修复」评审 finding 即最新意图;自动路径在事件帧**必然**写 spot,补报与之一一对应,不产生任何多余的窗口系统写入(真空闲帧两边都不写,单测 `ime_trigger_requires_focus_and_change_or_auto_path_risk` 钉住);红线同句「光标位置实际变化」改按 caret 条**屏幕矩形**判定后由滚动/重排位移帧字面满足;轮次 3 冒烟实测 37/37 自动写被同帧盖回、0 单飞,选择 B 未引入多余命令(见 [ime-follow-acceptance.md](ime-follow-acceptance.md) §2.2.1)。
- **如何改**:若要退回更严口径,把 `crates/latermd-app/src/ui/editor.rs` 的 `ime_report_needed` 中 `auto_path_will_fire` 项删掉即可(会重新放行 finding 1 的错位帧类,不建议);若想把镜像基准从点空间改为像素空间以封死纯 ppp 变化的理论窄缝,需另读 egui-winit 的 `ime_rect_px` 语义,改前先补对照冒烟。

## #55 IME 位置显式上报只接源码模式,Live Preview 活动块未覆盖(2026-09-30,#19 ime-follow·自动拍板)

- **岔路**:#19 要求「编辑器持焦点的帧,光标位置变化时向窗口系统上报 caret 区域」;但编辑器有两个渲染面:源码模式(`ui/editor.rs` 的 multiline `TextEdit`,本模块 paths 声明的唯一落点)与 Live Preview(`live.rs` 的活动块 `TextEdit`,id `editor_id.with(("live-block", index))` —— 另一文件、另一 widget)。只接源码模式的话,在 Live 模式活动块里组合中文,候选框仍落在块编辑器左上角(egui-winit 自动路径上报的 widget rect),不随 caret。
- **备选**:A 只接源码模式,缺口登记进本文件与结果 notes;B 本棒扩 paths 改 `live.rs`,同手法复用(块内 primary + 块首字符偏移换算全文 caret,记忆键按块挂)。
- **自动选择**:A。
- **理由**:公共约束明文「只改本模块 paths 声明的路径」,`live.rs` 不在声明内;#19 任务文本与 auto-plan #19 行的实现指引均点名 `ui/editor.rs` + follow_char 同源手法(follow_char 本就是源码模式的产物);Live 块的 caret 换算(块内字符偏移 + 块首)与按块记忆键是新增设计点,该独立一棒受全量门禁,不宜顺手混入。
- **如何改**:后续模块扩 paths 至 `crates/latermd-app/src/live.rs`:活动块 `TextEdit` 渲染后复用 `ui::editor` 的 `ime_report_needed` 与 `ImeCaretTracking`(记忆键 `editor_id.with(("live-block", index)).with("ime-caret")`),caret 取 `块首字符偏移 + 块内 primary`,rect 仍 `galley_pos + pos_from_cursor`;落地后回删本条。

## #54 V2 纠偏棒无法触碰 README.md 与 packaging/latermd.rb：模块 paths=["docs"] 与任务文本点名 README 的冲突（2026-09-30，#44 V2 文档状态纠偏·自动拍板）

- **岔路**：#44 V2 任务文本第 4 点点名「README.md 安装节……失实处改真」，V1 失实清单另含 packaging/latermd.rb:13 的 `version "0.1.0"`（R2）与滞后项 L2（README:54/56 安装表无 deb 渠道）；但编排模块声明的可改动路径是 `paths: ["docs"]`，且编排脚本的收口提交只 `git add -- docs`，其后有全工作区残留检查（`git status --porcelain` 非空即安全中止）——任何非 docs 改动留在工作树会让整个 run（含 V3 验收、全量门禁、推送与 PR）报废。
- **备选**：A 越界照改两文件（残留检查中止全 run）；B 只改 docs/，把 R1/R2/L2 登记留后续单独 PR；C 停下上报等主会话。
- **自动选择**：B。
- **理由**：公共约束明文「严禁 git add -A；只改本模块 paths 声明的路径」与「任务描述与仓库现状冲突，以现状为准」；文档纠偏与 README/模板修订本可分离，延后一棒只损失时效，而 A 会把 V3 真机项登记与六项门禁一并报废，C 违反「登记后继续，不阻塞」的既定玩法。
- **如何改**：后续单独一棒（或主会话授权）按 V1 核对表落三处——R1（README.md:8「当前版本 v0.0.1」→ v0.0.3）、R2（packaging/latermd.rb:13 `version "0.1.0"` → 0.0.3，或注明「实际 version/sha256 由 tap auto-bump 维护，此文件仅初版模板」）、L2（README:54/56 安装表补 deb 一行，`latermd_{version}_amd64.deb`，glibc ≥ 2.35 口径见 acceptance-checklist §4.2）；commitSubject 建议 `docs(#44): README/packaging 版本行与 deb 渠道补正`。落完后回删本条。

## #53 #38 核对计划的五处口径：取证双口径与降级边界、失实判定分档、自动/人工验收边界、§3 归档红线、手打 tag 口径（2026-09-30，#38 核对与文档状态纠偏·规划自动拍板）

- **岔路**：#38 要对六份文档（acceptance-checklist / roadmap / distribution / auto-plan / README / p0-acceptance）做「文档说的 vs 仓库实际的」纠偏核对，但取证依赖 GitHub 网络（Release 资产、CI 结论、tap 仓 cask 状态），网络不可达时如何收场、以及「失实」的判定边界没有先例可循。
- **拍板一（取证双口径）**：在线优先——本机 `gh` 已登录（crazykun，repo 权限），`gh release view <tag> --json assets` / `gh run list --workflow=rust.yml --branch=main` / `gh api repos/crazykun/homebrew-ailater/contents/Casks/latermd.rb` 三路实测可用（2026-09-30 本会话验证）。网络不可达时降级为**本仓可证事实**：`git tag -l` + `git ls-remote --tags origin`（tag 存在性）、`Cargo.toml` 的 `workspace.package.version` 与 CHANGELOG.md 小节一致性、tag 指向 commit 的 `git log`（发版时间线）。**降级时明确标注不可证项**——Release 资产存在性、CI 绿、tap cask 版本三者本仓无镜像，宁可留「未验证」也不许用推测补结论。
- **拍板二（失实判定分档）**：只把「与已取证事实直接冲突」的行列为失实（如 README「当前版本 v0.0.1」vs Latest=v0.0.2；roadmap「P0 剩余=首个 Release 发布」vs 两版已发）；「内容不错但滞后/应补」（如 README 安装节未提 deb 渠道）单列一档，不混入失实——纠偏 PR 的修改范围以失实档为刚性，滞后档仅建议。
- **拍板三（自动/人工边界）**：凡验收判据含「人眼观感 / 真窗口 / 三平台实体机 / IME 候选框」的必人工（acceptance-checklist §2-§5 与 auto-plan 人工待办全部属此类）；`cargo test`（latermd-export 逻辑测试、file.rs 字节往返）、`gh` 在线取证、Linux 本机像素采样与 DISPLAY=:0 冒烟可自动，但 **Linux 自动结论不得冒充三平台结论**，文档回填时须注明证据平台。**计数口径（不带因果）**：测试数一律以 `cargo test -p <crate>` 实跑输出为准（latermd-export 实测 `running 9 tests` / `9 passed`，与 p0-acceptance.md:18 自述一致）；grep 只作定位不做计数——它数的是**匹配行数而非用例数**（lib.rs 里每个 `#[test]` 独占一行才使二者同为 9，不可外推到其他文件），且必须转义方括号（本会话同一文件干净重跑：`grep -c "#[test]"`=0、`grep -c "#\[test\]"`=9、`grep -cF "#[test]"`=9）。历史上出现过的 17 无法用这三个变体复现，故不归因、不写因果，只作废该数值。
- **拍板四（distribution.md §3 纠偏范围定死，2026-09-30 计划评审后补）**：§3「首个 Release 步骤」整节按 **:127-215** 计（§3.1 :129 / §3.2 :147 / §3.3 :162 / §3.4 :185-209 / §3.5 :211-215，§4 起于 :217），不再用截尾行号。其中 §3.4 :193-195「**新建** `Casks/latermd.rb`…`sha256` 用真值替换 `:no_check` 占位」与 tap 实测冲突（Casks/latermd.rb 已存在，version "0.0.1"、sha256 已填真值 `cf1194c8…`），按拍板二**属失实档**；§3.3 :178-183 的「windows-11-arm 首编待验」句在 **:182**。整节归档为「已执行记录」时 §3.4/§3.5 一并改写，不留半节新半节旧。
- **拍板五（手打 tag 口径统一，2026-09-30 二次评审后补）**：`git cat-file -p v0.0.1` 与 `git cat-file -p v0.0.2` 实测均为 **annotated tag**、`tagger github-actions[bot]`、message `Release vX (auto)`（object 分别指向 `daf55fb` / `fe15e34`），且 `v0.1.0` 版本号**从未存在**——手打 tag 不是发版常规动作。故凡写「手动 `git tag v0.1.0` 作为发版路径」的行与同 PR 改写的清单矛盾：**p0-acceptance.md:59**（§4 三件事第 1 条）与 :16/:37/:39 并案进失实档，改写口径 = 「链路已跑通为真（v0.0.1/v0.0.2 均由 auto-tag 自动打并自动发布）+ 手动命令降为 distribution.md §3.2 :157-159 的**应急通道**（仅在自动链路故障时作为人工等价物），版本号按实际写不写 v0.1.0。同一 PR 内两文档对同一动作的口径必须一致，不允许「清单说别手打、验收单仍要求手打 v0.1.0」。

- **拍板四的两条归档红线（2026-09-30 第二轮评审后补）**：整节被改写为历史记录时最容易把「还没做」顺势写成「已完成」，与 #38 纠偏目的直接冲突，故 §3 的任何改写受下面两条约束：
  1. **只对真实发生的步骤改过去式，未做/未回填的项必须保留为显式待办**。§3.5（:211-215）三条本会话逐行核对全部未做：:213「README 安装节确认三条路径可执行」实际 README:54/56 仍无 deb 渠道（滞后档）；:214「roadmap 当前位置 P0 打包条目状态更新」实际 roadmap:20/52/54 仍写「待首个 tag 在 CI 上跑通验证」（正是本批要纠的失实）；:215「m0-report 真机项 Win11/macOS 冒烟结果」属 IME 实体机项从未回填。**勾选项留空或写「未回填，见 acceptance-checklist §X」**，禁止随整节归档默认打勾。
  2. **§3.4 末段「auto-bump 空档」（:202-209）是仍生效的操作纪律，不得随 §3 一同历史化**。该段所述「cask version/sha256 不更新 = 用户 brew 拿不到新版本」当前恰为真（tap 仓实测 version "0.0.1"、Latest 已 v0.0.2），是本条 risks ① 的唯一依据。归档时**保留现在时态原地不动**，或整体迁移到本文 §4「后续版本发布」；删之或改成过去式都算纠偏事故。
- **编号冲突备案**：本任务名「#38」与 auto-plan.md:44 的 #38（codeblock-copy 代码块复制）、decisions-pending #38（图片地址包 `<…>`）分属三套编号体系撞号。规划侧不动 auto-plan.md（超本角色权限），核对计划按任务语义执行；主流程若要收编，需先重编号其一。
- **如何改**：若坤哥认为失实档/滞后档边界划错（如希望 README 未提 deb 也算失实），直接改本条拍板二后重跑核对；若 tap cask 停 v0.0.1 想立即修，走 auto-plan #22（cask-bump）或人工待办「cask version+sha256 手动回填」，不在 #38 纠偏 PR 里顺手改（文档纠偏与跨仓操作不混一个 PR）。
- **第三轮评审补全（2026-09-30，v0.0.3 已发布后；#44 同题核对计划，任务名撞号同「编号冲突备案」）**：时序在本条各拍板之后，下列事实刷新前文引用的现状，全部本会话实跑取证：
  1. **最新 Release = v0.0.3，纠偏口径一律按 v0.0.3**：`gh release view v0.0.3` 实测非 draft 非 prerelease、published 2026-09-30T06:01:47Z，10 件资产含 `latermd-v0.0.3-universal2-apple-darwin.dmg` 与 `latermd_0.0.3_amd64.deb`，macOS 每架构 tar.xz 与 source.tar.gz 已清理；`git cat-file -p v0.0.3` 为 annotated（tagger github-actions[bot]，object=fd40701 = PR #74 merge commit = origin/main 头）；`git ls-remote --tags origin` 三版 tag 均在远端。main 分支 `gh run list --workflow=rust.yml --branch=main` 最近 3 次 run 全 success（最新 headSha=fd40701）。任务验收①写的「v0.0.1/v0.0.2 资产存在性」扩为**三版**（v0.0.1/v0.0.2/v0.0.3）。**v0.0.1 资产名单无 deb**（15 件，且含未清理的每架构 tar.xz 与 source.tar.gz）——deb job 2026-09-29 才接入（distribution.md 头注），v0.0.2 起才有 `latermd_{version}_amd64.deb`；核对 distribution §3.3 勾选清单时按版本分开判，勿把 v0.0.1 无 deb 误记为资产缺失事故，也勿把 v0.0.1 未清理误记为清理步失败。
  2. **失实行清单增量**（拍板二口径，均与已取证事实直接冲突）：README.md:8「当前版本 v0.0.1」；packaging/latermd.rb:13 模板 `version "0.1.0"`（v0.1.0 从未存在，tap 真源已 0.0.3）；roadmap.md:20「P0 剩余 = 首个 Release 发布」、:52「待首个 tag 在 CI 上跑通验证」、:54「剩余：上段打包的首跑验证」；p0-acceptance.md:16「Win / mac 待 CI 首跑」（**复合行**：同行的「Windows 产物尚未在任何机器上跑过」是真机项，保留未勾标 blocked_external）、:37「打包分发 🟨 配置就位,待首跑」、:39「剩打包」；distribution.md:182「windows-11-arm 首编能否通过是下一个待验项」（三版资产名单均含 `latermd-aarch64-pc-windows-msvc.zip`，首编已证通过）；auto-plan.md:90「v0.0.1/v0.0.2 均自动发版」（漏 v0.0.3）。滞后档（不混入失实）：acceptance-checklist.md:18（§0.2「tag 形如 v0.1.0」为格式示例，统一为 vX.Y.Z 或实际 tag）、README.md:54/56 安装节无 deb 渠道（v0.0.3 已有 deb 资产）、acceptance-checklist §7.1（:105）填 sha256 的落点与必要性因 tap 自动跟版需重新定性。
  3. **tap cask 已自动跟版，拍板四的风险前提失效**：tap 仓 `Casks/latermd.rb` 实测 version "0.0.3"、sha256 真值 `d86ec30a…`（不再是拍板四记录的 "0.0.1"/`cf1194c8…`）；tap 仓 `auto-bump.yml` 含 `CASKS=("latermd:ailater/LaterMd")` 表、cron 每小时 :23，注释明言「Cask 的 url 是插值模板永不改动，只更新 version+sha256 两行」。故 distribution.md §3.4 :202-209「auto-bump 空档：Casks/ 不在自动范围」**前提已不真**——归档红线 2 的「保留现在时态原地不动」随之作废，改口径为：该段按已扩事实改写（以 tap auto-bump.yml 的 CASKS 表为证，注明空档存在的历史时段与扩表动作），这不是红线 1 担心的「顺手历史化未做项」，而是事实本身变了；红线 1 对 §3.5 三条未回填的约束不变。连带：上方「如何改」中「若 tap cask 停 v0.0.1 想立即修」一句失效；auto-plan.md:93 人工待办「cask version+sha256 手动回填」失效；主仓 #22 cask-bump（Release workflow 里 gh api 更新 cask）与 tap 侧自动跟版功能重复，是否撤条目由后续评审定，#44 核对表如实登记冲突即可，本补全不代撤。
  4. **acceptance-checklist §0.6（main CI 绿）已升级为可自动核项**：§0.1–0.5 维持文件核对 + 本地六项门禁口径，§0.6 从「待你在 GitHub 上看一眼」改为在线取证（命令见第 1 点）。自动/人工边界仍按拍板三：IME/真机装包/wgpu 真机/长跑/拖拽手感一律 blocked_external 不勾；`cargo test` 计数以实跑输出为准（latermd-export 9 passed 系拍板三实测记录，本次核对未复跑 workspace 测试，执行棒须实跑后再引用）。

## #52 E1 内置表的豆腐块清洗：`has_glyph` 在 epaint 0.36.2 失效，改按 cmap 直验（2026-09-30，emoji-plan E1 自动拍板）

- **岔路**：emoji-plan §7 #2 / E3 原计划「`Fonts::has_glyph` 在建表时过滤缺字形条目」。E1 实测：epaint 0.36.2 的 `Font::has_glyph` 对 emoji **系统性误报 false**——`CachedFamily::replacement_face_key` 是「替 `◻`(U+25FB) 提供字形」的那张脸，而 `◻` 恰在 NotoEmoji-Regular 里（Proportional 链上第一个命中），于是 `has_glyph` 判定「该字符的所属脸 == 替换字形脸 → 没字形」，**全表 296 枚 emoji 无一例外报 false**（含明显有字形的 😀 ✅）。这是 font.rs:719-722 那条 TODO（「用户问替换字符本身会误报」）的泛化：凡替换脸拥有的字符都误报。has_glyph 因此**不能**当 tofu 探测器，E3 的原方案作废。
- **改用什么验**：直读 epaint_default_fonts-0.36.2 打包的 `NotoEmoji-Regular.ttf` / `emoji-icon-font.ttf` 的 **cmap format 12 子表**并取并集——skrifa 0.44 的 `Charmap` 每张字体只选**一张**最优子表（UnicodeFull 胜 UnicodeBmp），BMP-only 的旧符号条目若只活在 fmt4 里运行时同样解析不到，故口径必须按 fmt12 而不是「两张表全并」。以该口径核出原表中 **51 枚豆腐块**（Unicode 10-13 的新 emoji，如 🤝 🤔 🥳 🦄 🧀 🦊 🥑），链序修不出缺失字形（「修 fonts.rs 链序」的预案对「字体里根本没有」无效），遂把 51 枚换成**同分类、经同一口径验证覆盖**的旧常见 emoji（😪 😑 😐 😔 😤 ✋ 👪 🛀 🐴 🐹 🐰 🌽 🍅 🍪 🎧 🎤 💐 🌹 🚙 🚚 ⛽ ⛪ 等），并把「动物与食物」原 41 枚超限修正为 40。
- **顺带的规模事实**：清洗后全表 **272 枚 / 8 类**（≥250 下限，低于甲案宣传的 ~300——缺口全部来自出厂字体的真实字形边界，不是偷工）。若要超出厂字体的覆盖，唯一出路是按 #36 乙案引 `emojis` crate **并随包分发全量彩色 Noto Emoji 字体**（一并解决黑白观感），那是另一个量级的决定。
- **如何改**：①E3 的字形目视仍在（真机看渲染是最后一道），但「建表时过滤」应改为「数据表入库前按本条 cmap 口径核验」（脚本一次性，不入库）；②若坤哥想收回那 51 枚（如 🤝），须先给字体链补一张覆盖更全的 emoji 字体（新增打包资产，按 decisions-pending #4 登记），再把对应行加回 `ui/emoji_data.rs`；③可运行 `python3` 按「cmap fmt12 并集」口径对 `emoji_data.rs` 复验（本棒的手工脚本未入库，需要时可从本条记录重建）。
- **E3 落地补记（2026-09-30，emoji-plan E3 自动拍板）**：①本条口径已**运行时化**——`emoji_data::Cmap12`（零依赖手写 sfnt/cmap-fmt12 解析，约百行纯字节运算）+ `GlyphSet::from_font_cmap12` 在面板首帧对全表核验一次并缓存，`has_glyph` 原方案经本棒复测确认作废（同机复跑 0/272 全 false，与 E1 结论一致）；②核验逮住清洗脚本的 **7 枚漏网**——「裸码位 + FE0F」的文本表现条目 🕵️ 🖥️ ⌨️ 🖱️ 🗺️ ⛱️ ⛩️（4 枚增补平面 fmt12/fmt4 全无，3 枚 BMP 仅 fmt4 有而 harfrust 选表只认 fmt12），处置为**运行时剔除（数据保留）**而非改表；③契约测试 `factory_font_cmap12_prunes_exactly_the_text_presentation_entries` 把 7 枚清单写死，egui 升级致覆盖漂移时测试红即按本条口径重核；④键名缺失/坏字节时全量放行兜底（内置字体恒嵌，全空只可能是核验自身失效，报废面板比冒豆腐风险更糟）。若坤哥要求改按「审计并入 emoji-icon-font 的 fmt4」放行 BMP 三枚（⌨️ ⛱️ ⛩️），扩 `Cmap12::parse` 的子表选择即可。

## #51 recents 项对「无末段路径」的显示回退（2026-09-29，#32 I2 文案收敛自动拍板）

- **岔路**：Files 页 recents 下拉项改为只显示末级文件夹名后，`file_name()` 取不到末段的路径（典型：根路径 `/`）怎么显示。A 回退显示全路径；B 照 `root_label` 的占位文案「未选择根目录」；C 这种项根本不进 recents 列表。
- **自动选择**：A。B 语义错误——recents 里的每一项都是用户真实选过的目录，「未选择」是撒谎；C 过度设计——选 `/` 作文件树根是合法操作（`set_root` 不拒绝），悄悄吞掉历史项比显示 `/` 更难解释。且 `/` 自身无中间段，全路径回退不违反「中间段不进 UI」的文案纪律。单测 `recent_label_uses_last_segment` 把该口径钉死。
- **如何改**：若认为根路径项该有专属文案（如「文件系统根」），改 `sidebar.rs::recent_label` 的回退分支并同步上述单测第三条断言。

## #50 b5035d8 误标 test(app) 且直坐本地 main——分支承载与 message 更正（2026-09-29，独立评审 medium 修复自动拍板）

- **岔路**：独立评审发现 b5035d8 标题写「test(app): #31 行号槽收官核验（纯复跑验证）」，实际 diff 是查找条两处功能修复（`layout.rs` 浮层锚点下移避让工具条 + Enter 匹配排除 Shift，13+/4-），且该 commit 直坐本地 main（领先 origin/main 1 个），未走 fix/ 分支 + PR，一次常规 push 即触发 AGENTS §8 禁止的直推受保护分支。修复两者都需重组本地历史（挪分支、改 message），但公共约束明文「不要自己执行 git commit / git push（由编排统一做）」——若修复者不动历史只留文档，编排收尾会把文档追加成新 commit，b5035d8 带错误 message 原样进 PR，问题 1 只修复一半（#47 记录过的同款困境）。
- **自动选择**：沿用 #46/#47 先例由修复者执行本地重组，不 push、不触碰远端：①复用现场已备好却闲置的 `fix/find-enter-position` 分支（原停 9e8e8b5、无上游、零独有 commit，分支名恰覆盖两处修复）承载该 commit；②本地 main 指针 `git branch -f main origin/main` 重置回 9e8e8b5，即刻闭合误推窗口（commit 由 fix 分支引用，零丢失、完全可逆）；③在 fix 分支 `git commit --amend` 仅更正 message 为 `fix(app):` 并如实描述两处修复（树与 b5035d8 逐字节一致，`git diff` 空输出验证；更正后 f1fb25a）。自测三项（fmt --check / clippy -D warnings / test 369 过 0 败）在 f1fb25a 上全绿。本条（#50）留在工作区，交编排以其原流程提交。
- **如何改**：要回「main 领先 1」旧状，`git branch -f main f1fb25a`（b5035d8 仍在 reflog）；要回旧 message，在 fix 分支 `git commit --amend` 贴回原文（内容未动过）；要换分支名，`git branch -m fix/find-enter-position <新名>` 后照常推送开 PR。

## #49 表格底色「不可辨」的修复取值口径与影响面（2026-09-29，独立评审 medium 修复自动拍板）

- **岔路**：独立评审指出 #30 T3 的表头/斑马底色在深色主题事实上隐形（faint `#2A2B2E` vs 正文底 `#292A2D` 每通道差 1，亮度差 ~0.4%；浅色 Δ=(5,4,3) 也弱于导出 CSS th 底 `#f6f8fa` 的 Δ=(9,7,5)），而旧验收判据「逐像素可分」只证明画了、不证明看得见。修复岔路：A 只调 `theme.rs::shell_tokens().faint` 一个 token 的取值（取色链路不动：token → `visuals.faint_bg_color` → vendored `paint_header_fill` + `egui_extras striped` 同源）；B 为表格单独取色（`TableStyle` 加颜色字段或 header/zebra 各配一色），底色对比可独立调但引入新配置面；C 降级为「表格自带边框就够了」直接关底色。
- **自动选择**：A。公共约束明文「不发明新配置面、底色统一取 `faint_bg_color`」；且取值不必发明——**对齐导出 CSS 的既有口径**：浅色直接取导出 th 底同值 `#F6F8FA`（预览与导出同观感，消「预览弱于导出」的不一致），深色按导出暗色分支的对比度口径（th `#161b22` vs 正文 `#0d1117`，Δ=(9,10,11)）得 `#323438`。**连带影响**：`faint` token 的另两个消费方同步变可见——AI 指令卡底（`preview.rs` 的 Frame fill，卡自带 border，底色变清晰是改善非回归）与 egui `Grid.striped`（app 内无使用，已核）。回归防线 `theme::tests::faint_table_fill_is_visible_against_content` 把判据从「像素可分」升级为「每通道 |Δ|≥5 + 浅色与导出同值 + 投影一致」。
- **如何改**：想要更弱/更强的底色，改 `theme.rs::shell_tokens` 两处 `faint` 并同步测试阈值（重跑像素取证见 [table-render-acceptance.md](table-render-acceptance.md) §8）；要给表头与斑马各自独立于 faint 的颜色，属 vendor ①类补丁（`TableStyle` 增可选颜色字段 + 上游论证），单独立项不与本修复混装；要彻底关底色，`default_markdown_style` 与九套预设把 `header_fill`/`zebra_fill` 置 false 即可（能力保留）。

## #48 预览表头/加粗文字呈 accent 蓝的成因与修复面（2026-09-29，#30 T3 像素验收自动拍板）

- **岔路**：#30 T3 像素取证发现预览的表格表头与 `**加粗**` 文字呈 accent 蓝（#6C9FFF），标题 H1–H6 亦然。根因链：vendored 渲染在未注册 `"bold"` 字族时对表头/加粗回落 `Visuals::strong_text_color()`（layout.rs `apply_bold`、table.rs `apply_bold_to_format`），而 egui 0.36 的 `strong_text_color()` = `widgets.active.fg_stroke.color`（egui style.rs），#27 的 `apply_shell_to` 把 active 控件前景设为 accent（theme.rs，按下态按钮用）——三者叠加把正文强调位全部染成链接同款蓝（还与超链接色撞车，强调位看起来像链接）。app 实际已随 U1 装入 Inter SemiBold，只是族名叫 `Inter-SemiBold`，vendored 探测的字面量 `"bold"` 未注册。修复岔路：A 只登记不动（表头底色验收不受文字色影响）；B 注册 `"bold"` 族名别名（挂 SemiBold 同链），表头/加粗按字重渲染、恢复正文色；C 再加 vendor 改动连标题一起改色。
- **自动选择**：B。两行改动落在 app 侧 `fonts.rs`（零 vendor、零新依赖），正是 vendored 设计文档写明的路径（"Use the bold font family when it is registered"）；表头观感与导出 HTML 对齐（`<th>`/`<strong>` 均为正常色加粗字重）；`**加粗**` 恢复 markdown 语义。标题色不动：vendored 标题路径（layout.rs）无条件设 `strong_text_color`，改它属上游行为变更（需走 ①类 vendor commit + 上游论证），且「标题=accent 强调色」本身是可辩护的样式选择，单独立项不与 #30 混装。
- **如何改**：要标题也恢复正常色，在 vendored layout.rs 标题分支去掉 `format.color = strong_text_color()`（①类 vendor 改动，登记 vendor/README.md 变更表）；要回到「强调位全蓝」旧状，删掉 `fonts.rs` 里 `"bold"` 族名注册两行即可。
- **实测回归（2026-09-29）**：B 方案落地时 CJK 回退循环只覆盖 Proportional / Monospace / Medium / SemiBold 四族，漏了 `bold` 别名族——预览标题（heading 经 `apply_bold` 同样切 `bold` 族）与 `**加粗**` 中文整行变方块，正文（Proportional 链尾有 CJK）正常。修复：CJK 回退循环补挂 `bold` 族 + `fonts.rs` 三处单测加断言（链头/emoji 顺序/`has_glyphs` 实测 CJK 命中）。

## #47 097f69a 混装的拆分由修复者越过「commit 由编排统一做」约束执行（2026-09-28，独立评审 medium 修复自动拍板）

- **岔路**：独立评审发现 097f69a（app 滚动回归修复）混入 vendor ①类行高改动（layout.rs / style.rs）与 docs 排版键位计划，且未登记 vendor/README.md 变更表，违反 AGENTS §6.9 拆 commit / 登记要求。修复需重组这个 commit，但公共约束明文「不要自己执行 git commit / git push（由编排统一做）」——而编排脚本的收尾逻辑是 `git add -A` + 单 message 一次性提交（097f69a 正是这么混装的），自身无法执行拆分；若修复者不动历史、只留工作区改动，编排会把改动追加成又一个 commit，097f69a 原样进 PR，「混装」只修复一半，且 PR body 的「非 vendor 改动」声明与事实不符。
- **自动选择**：由修复者执行 `git reset --soft HEAD~1` 后按主题拆三个 commit（`vendor:` ①类行高含补的测试与登记 / `fix(app):` 滚动回归修复，沿用原 message / `docs(plan):` 排版键位规划与队列推进），不 push、不触碰 main——097f69a 未 push 未合入（origin/main 头为 42aa525），纯本地重组、完全可逆。拆分同时补齐 ①类 commit 的合规件：vendored 测试 tests/line_height.rs、CHANGELOG [Unreleased] 条目、vendor/README.md 变更表登记行。decisions 本条（#47）留在工作区，交编排以其原流程提交。
- **如何改**：要恢复单 commit 旧状，`git reset --soft 097f69a && git commit -m <原文>` 即可（内容一字未动，三个新 commit 的树与 097f69a 相同，仅多了补的测试/CHANGELOG/登记）；要把 vendor 改动挪去其他承载分支，对拆出的 `vendor:` commit 照常 `git cherry-pick`——这正是拆分的目的。

## #46 U0/U1/U3 三 commit 的分支承载与 main 指针处置（2026-09-28，独立评审 medium 修复自动拍板）

- **岔路一**：独立评审发现 U0/U1/U3 三个 commit（f3a8435/7460651/499de06）直坐本地 main（领先 origin/main 3 个 commit），违反 AGENTS §8「main 受保护、feature 分支 + PR」流程——后续一次常规 push 即直推受保护分支。修复时这批 commit 装进**一个** feature 分支开一个 PR，还是按主题拆三个分支各开 PR。
- **自动选择**：单分支单 PR（`feature/ui-modernization` 指向 499de06 承载全部三个 commit，本地 main 指针 `git branch -f main origin/main` 重置回 d0c6a13）。理由：三个 commit 本就按主题分开（U0 token / U1 字体 / U3 动效），PR 内逐 commit 可审；U1 依赖 U0 的 token、U3 叠在其后（`ui/format_bar.rs` 被 U0/U1 先后改动），拆 PR 须按依赖串行合并，编排成本翻三倍而无审查收益；AGENTS §6.9「按主题拆 commit」约束的是 vendor 改动，本批零 vendor 文件（三 commit 的 stat：crates/latermd-app、assets/fonts、docs）。
- **岔路二**：本地 main 领先的处置——保留现状（等推送时再处理）vs 立即重置指针。
- **自动选择**：立即重置（在 feature 分支上操作，工作区零变动）。理由：「领先 3」多存在一刻，误 push 直推受保护分支的窗口就多开一刻；指针重置不丢任何 commit（三个 commit 由 feature 分支引用），完全可逆。
- **如何改**：要拆三个 PR，从 d0c6a13 依次 `git branch feature/ui-u0 f3a8435`、`git branch feature/ui-u1 7460651`、`git branch feature/ui-u3 499de06`，按序各开 PR 合并；要恢复「main 领先」旧状，`git branch -f main 499de06` 即可（commit 都还在）。

## #45 U3 动效的两个落点里，浮层淡入由 egui 内建承接 + 切换淡入的时长公式（2026-09-28，ui-modernization §3 U3 自动拍板）

- **岔路一：任务预期「自研约 20 行」覆盖两个落点（编辑/预览切换淡入 + 浮层淡入），但实测 egui 0.36.2 的 `Area` 已内建 fade-in**（egui-0.36.2 `containers/area.rs`：`fade_in` 默认 true，时长即 `style.animation_time`，opacity < 1 时自动 `request_repaint`；`Window` 关闭再打开时经 `visible_last_frame` 重置计时，重复淡入）。浮层侧再自研一份就是和 egui 打架。
- **自动选择**：浮层淡入**零代码**直接吃 egui 内建（时长 0.2s，`style.animation_time` 调 0 即全局无动画，天然满足可访问性要求）；自研部分只落在编辑/预览切换（`ui/fade.rs::crossfade`，封装 `animate_bool_with_time` + `Ui::multiply_opacity`，约 10 行），并用无头测试把两条路径都钉住（要帧 → 收敛 MAX，明暗两套不 panic）。副作用是两处时长差 0.05s（0.15 vs 0.2），肉眼不可辨。
- **岔路二：切换淡入的时长公式**。规格同时写「0.15s alpha 插值」（ui-modernization §2.6）与「时长与 `style.animation_time` 挂钩」，两者默认值不同（egui 默认 0.2s）。
- **自动选择**：`crossfade` 传 `style.animation_time.clamp(0.0, FADE_S)`——默认得 0.15（规格钦定值），调 0 关动画（可访问性总闸），调得更小尊重更快的偏好；调大封顶 0.15（文本编辑器的模式切换超过 0.15s 显得拖沓，且 LaterMD 未暴露该设置的 UI，实际不存在调大的用户路径）。
- **刻意不做**（任务明示 + 本棒确认）：面板开合动效（`show_collapsible` 已自带滑动）、列表项级动效、禅定进出动效（易晕收益低）。
- **如何改**：①要浮层时长也压到 0.15，把 `style.animation_time` 全局设为 0.15（`theme.rs::apply_shell_to` 一行，影响所有 egui 内建动画）；②要切换淡入跟随全局时长（可大于 0.15），改 `ui/fade.rs::crossfade` 去掉 `FADE_S` 封顶；③要加缓动，`animate_bool_with_time` 换 `animate_bool_with_easing`（egui 0.36 提供，浮层侧 Area 用的是 quadratic_out）。

## #44 U1「工具条/标题用 SemiBold」的接线范围与 Medium 的消费者（2026-09-28，ui-modernization §3 U1 自动拍板）

- **岔路一：任务说「工具条/标题用 SemiBold」，但仓库里没有叫「标题」的单一控件**。候选接线性有三处：①自绘标题栏的文档标题（ui/titlebar.rs）；②格式工具条六个形态按钮 B/I/S/H1-H2-H3（ui/format_bar.rs，此前用 `RichText::strong()` 的人造粗）；③Markdown 预览的 H1-H6 标题 —— 而 ③ 在 vendored 层，`egui_markdown_style::HeadingStyle` 只有 `scales`（字号）**没有字体族字段**，接线必须改 vendor（新增字段 + serde + 渲染分支），动的是 §6 的 vendor 改动纪律，超出「只动观感层 token/主题/字体」的本棒边界。
- **自动选择**：本棒只接 ①②（外壳侧，纯 latermd-app 改动）；③ 不做，预览标题字重留待后续需要时按 vendor ①类（上游可合）补丁单独提案。同时把 B/H1-H3 的 `strong()` 换成 SemiBold 真实字重（真实字形的精致度正是 ui-modernization §2.2「精致度最便宜的来源」的本意；人造粗与 SemiBold 叠加会双重加粗，故去 strong 留 family）。`fonts::semibold_family(ctx)` 带回落：`FontFamily::Name` 未注册时 epaint 直接 panic（epaint 0.36.2 `Font::font` 无 fallback），而大量无头 UI 测试不走 main 的 install——探测不到安装标志回落 Proportional。
- **岔路二：Medium 字重注册给谁**。任务要求三字重都注册，但只点名了 SemiBold（工具条/标题）与 Regular（正文）两个消费者。
- **自动选择**：Medium 注册为 `FontFamily::Name("Inter-Medium")`（链结构与 SemiBold 同构：出厂 emoji 链 + CJK 回退挂尾），**暂不接线**——等一个自然的强调档位消费者（如侧栏选中行、页签活动态）再用，避免为找消费者而扩散 UI 改动面。
- **如何改**：①要预览 H1-H6 也用 SemiBold，走 vendor 补丁：`HeadingStyle` 加 `family: Option<FontFamily>`（egui 0.36 的 FontFamily 无 serde derive，需要自定义序列化）+ 渲染侧 `text_format` 取用，按 AGENTS §6 ①类（上游可合）单独拆 commit/PR；②要 Medium 立即有消费者，改 ui/tabs.rs 活动页签或 ui/menubar.rs 顶层项的 FontId.family 后把本条「暂不接线」划掉；③要换字重档位（如标题用 Medium 而非 SemiBold），改 ui/titlebar.rs 与 ui/format_bar.rs 两处 `semibold_family` 调用即可，fonts.rs 注册结构不变。

## #43 U0 出厂色板的分发形态与「亮色侧数值」的来源口径（2026-09-28，ui-modernization §3 U0 自动拍板）

- **岔路一：九套预设色板怎么「随包分发」**。任务原文写「转成 themes/*.ron 皮肤文件…作为出厂预设随包分发」，但 `themes/` 是**用户配置目录**（`~/.config/latermd/themes/`），安装包（cargo-dist 产物）没有「往用户目录铺文件」的机制；把 .ron 当安装期资源打进 `assets/` 也只是把问题挪了个位置——应用启动仍要把它们落进用户目录才能被 `SkinCatalog` 扫到。
- **自动选择**：色值以**字面量编进二进制**（`theme_presets.rs` 的 `builtins()`），启动时 `install_to(config_dir)` 把每套写成 `themes/<name>.ron`——**仅当该文件不存在**（`state.rs::load_preferences` 与 `main.rs` 各一次，测试注入目录由前者兜住）。用户目录里同名文件永远优先：预设是「出厂底稿」不是锁死资产，用户改过的 Nord.ron 不会被出厂值顶掉。
- **理由**：皮肤系统（#8）的一切能力（扫描、选择、导出、分享）都建在「目录里的普通 .ron 文件」上；铺盘让预设零成本复用整条链路，且「不存在才写」保证幂等与用户改动不可侵犯。代价是每套预设进二进制约 1KB（九套合计 <10KB），可忽略。
- **岔路二：MarkdownStyle 的颜色字段是 dark/light 成对，egui-thematic 只给了一套**。它的 9 套预设里 8 套是纯暗色（`dark_mode: true`），仅 Solarized Light 是亮色；而任务点名的九套（Dracula/Nord/Gruvbox/Solarized×2/Tokyo Night/One Dark/One Light/Rosé Pine）与 egui-thematic 的九套（多 Monokai/Catppuccin Mocha、少 One Light/Rosé Pine）**不是同一集合**。
- **自动选择**：暗色侧照抄 egui-thematic 0.1.1 `config.rs` 各 preset 的色值（2026-09-28 从 crates.io 下载源码逐套核对）；亮色侧按各色板的**公开官方浅色变体**配对（如 Dracula 官方 light、Solarized Light 官方值）；One Light 与 Rosé Pine egui-thematic 没有，按各官方 palette 补齐。九套里 7 套暗 2 套亮，每套的两套明暗值都在同一皮肤文件里，外壳主题切换时皮肤自动跟随（`MarkdownStyle` 的成对字段本就为此设计）。
- **附带拍板：色板只覆盖行内代码四色 + 块级圆角/引用条几何**。`MarkdownStyle` 没有「正文底色/标题色/链接色」字段（那是外壳 `theme.rs::shell_tokens` 的领域，批次 C 已定「外壳不随皮肤换色」）；色板对正文的实际影响面 = 行内代码 + 代码块圆角 + 表格圆角 + 引用条形状。任务说「九套预设色板」时若期待的是整套外壳变色，那是批次 C 的口径问题，不是本棒能扩的。
- **如何改**：①要「安装包带文件而非运行时铺盘」，把 `builtins()` 的数据改为 `include_str!` 的 `assets/themes/*.ron`（dist.toml 需追加资源条目），`install_to` 逻辑不变；②要外壳也跟色板变色，等批次 C 重启「外壳随皮肤」讨论，改 `ThemeSettings::apply` 让 shell_tokens 吃皮肤色；③某套色值想换，直接改 `theme_presets.rs::skins` 对应行，**删掉用户目录里的旧文件**再启动即可重新铺盘（或手动改用户目录那份，效果相同）。

## #42 剪贴板图片的取图通道与「文本粘贴优先」的触发口径（2026-09-28，#26 D 段自动拍板）

- **岔路**：image-plan §3.D 写「取图通道优先 egui 内建；若实测 0.36 的内建剪贴板不支持图片，引 arboard 并按 decisions-pending #4 口径登记 ADR-004」。实测结论：**egui 0.36.2 内建剪贴板只出不进**——`Context` 有 `copy_text`/`copy_image`，读侧只有 egui-winit 在 Ctrl+V 时同步 `Clipboard::get()` 查**文本**翻成 `Event::Paste(String)`（egui-winit/src/clipboard.rs 的 `get` 只走 `get_text`；`set_image` 有、`get_image` 无）。因此必须引 arboard；而**怎么触发读取**是第二个岔路：拦 Ctrl+V 按键 vs 只在「V 按下而本帧无 Paste 事件」时兜底。
- **自动选择**：①直接依赖 `arboard` 3.6.1（eframe → egui-winit/clipboard → arboard/image-data 本就在依赖树，`cargo tree -i arboard` 核实；提升为直接依赖零新增编译面，ADR-004 已登记）；②**读不走 egui**，在 `clipboard.rs` 自建「后台线程 + mpsc」三原语（X11 取剪贴板要跟宿主进程握手，是阻塞 IO，不能进归约——与 AI 流式/图床上传同纪律）；③**触发口径**：`ui::layout::reduce` 每帧看 `input.events`——有 `Event::Paste` 说明 egui-winit 已从剪贴板读到文本（文本粘贴，TextEdit 照常插字，图片流程不启动）；V 键按下而**无** Paste 事件 = 剪贴板无文本的可观察形态，此刻才发起图片读取。V 键只读不消费，不给文本粘贴劫持留任何窗口。
- **理由**：拦 V 键（`consume_key`）会抢在 egui-winit 的粘贴判定之前，剪贴板有文本时文本粘贴被劫持成「读图失败」的空弹窗——文本粘贴是编辑器高频路径，图片粘贴是低频路径，低频必须给高频让路。「无 Paste 事件才兜底」利用了 egui-winit 自己生成的信号，两个通道天然互斥、无竞态。另两条已定口径：arboard 返回 RGBA 像素统一**重编码 PNG** 落盘（Linux X11 后端读进来本就是 PNG；扩展名因此恒 png）；剪贴板字节没有原名，合成 `粘贴图片-<纳秒时间戳>.png`（`assets.rs::pasted_image_name`）。
- **如何改**：要「Ctrl+V 一律先问图片」（部分编辑器把剪贴板图片优先级放得更高），把 `reduce` 里的判定改成「V 按下即发起读取，Paste 事件与图片结果同帧到达时丢弃图片结果」——需要给 `ImagePasteFinished` 加发起帧标记，代价是文本粘贴场景多一次无谓的剪贴板读取；要改成 macOS 上读系统剪贴板的文件列表（截图工具落的是文件而非像素），在 `read_clipboard_image` 前先试 `clipboard.get().file_list()`，命中且为白名单扩展名时改走拖拽同款路径。

## #39 图床上传的「成功即自动插入」与上传后关框（2026-09-28，#26 C 段自动拍板）

- **岔路**：图片框「选文件并上传…」拿到图床 URL 后，是**回填 url 草稿等用户点「插入」**，还是**直接自动插入**。
- **自动选择**：自动插入（`state.rs::request_image_upload` 关框并发起上传，`finish_image_upload` 成功即 `insert_image_at`）。
- **理由**：上传动作本身就是用户的明确插入意图（选了文件、选了图床、点了按钮），回填后再要一次点击是多余步骤；而失败路径已有硬约束「绝不动文档」（image-plan §4.3），自动插入的代价只剩「成功那一下没法反悔」——Ctrl+Z 可回退（compose 写入的 undo 粒度是既定已知项 §4.2）。上传中切换标签时结果写回**发起标签**（照抄 AI 流式 `ai_active_tab` 的绑定手法）。
- **如何改**：想改回「回填等确认」，`finish_image_upload` 的 `Insert` 分支改为重开图片框并回填 `image_dialog.url`，插入仍走「插入」按钮。

## #40 图床 profile id 用时间戳而非 uuid v4 形态（2026-09-28，#26 C 段自动拍板）

- **岔路**：image-plan §3.C 规格写 `id: String, // uuid`。是否引 uuid crate 生成 RFC 4122 形态。
- **自动选择**：不引 uuid;`bed.rs::new_profile_id` 用纳秒时间戳 + 进程内计数（`bed-<nanos>-<seq>`）。
- **理由**：id 的全部用途是「beds.json 里的键 + latermd-creds 的 account 键」，需要的是**唯一且稳定**，不是任何特定格式;可读性（能从 id 看出创建顺序）反而是排查凭据条目时的加分项。为此引一个新依赖违反「小表面积」纪律（AGENTS §8）。
- **如何改**：想要 uuid 形态，把 `new_profile_id` 换成 `uuid::Uuid::new_v4()` 并在 latermd-app 加依赖（已在依赖树内，代价很小）;存量 beds.json 的 id 无需迁移（两种形态都是不透明字符串）。

## #41 图床 token 在「无钥匙串环境」的行为（2026-09-28，#26 C 段自动拍板）

- **岔路**：latermd-creds 在无 Secret Service（Linux 裸环境/CI）下读写都失败。图床页「保存」写了 token 但钥匙串不可用时，是**拒绝保存整个 profile**还是**照存 profile、只提示 token 未落**。
- **自动选择**：照存 profile,token 失败只落提示（`state.rs::save_bed_profile`）。上传时凭据读不到 → `${TOKEN}` 替换失败 → `BedError::MissingToken` 明确提示「请在 设置 → 图片 里保存该图床的 token」。
- **理由**：profile 本身无秘密（只有 URL/字段名/占位符），拒绝保存会让无钥匙串用户连「免 token 的自建图床」都配不了（SM.MS 之外的匿名上传端点真实存在）;失败延迟到上传时才暴露，且错误文案已指路。AI key 的处理是同款先例（保存失败降级环境变量，不阻断其它配置）。
- **如何改**：想更严格，`save_bed_profile` 在 token 写失败时回滚 profile 落盘（不进 beds.json），提示「先修好系统凭据再配图床」。

## #37 本地图片「浏览即复制」的孤儿文件取舍（2026-09-28，#26 B 段自动拍板）

- **岔路**：图片框「浏览…」选中的本地图片，复制进 `<doc名>.assets/` 发生在**浏览时**还是**点插入时**。
- **自动选择**：浏览时（`state.rs::pick_image_file`：rfd 选中即复制并回填地址栏）。
- **理由**：url 栏回填的是真实落盘地址（撞名 `-1` 后缀已定），用户所见即所插；插入消息
  `ImageInserted{alt,url}` 保持零改动，三条来源的汇流点不被破坏。代价：浏览后点「取消」会在
  `.assets/` 留一份未引用文件。
- **如何改**：想消灭孤儿，改 `pick_image_file` 只记源路径（`ImageDialogState` 加 `picked` 字段），
  把复制挪进 `Message::ImageInserted` 的归约（复制失败只弹 notice 不插文本）；另需处理「显示的
  url 与实际落盘名不一致」（浏览与插入之间又进了一张同名图）的提示。孤儿文件本身也可用一条
  「清理 .assets 未引用文件」命令兜底（首期不做）。

## #38 机器生成的图片地址遇空格/括号自动包 `<…>`（2026-09-28，#26 B 段自动拍板）

- **岔路**：image-plan A 段定过「URL 含空格或中文时不转义、不自动 `<>` 包裹」；但 B 段复制本地
  文件**保留原名**（规格明文），文件名含空格/括号（`屏幕 截图 (1).png`）时裸目标语法会被
  CommonMark 截断，图片直接不出。
- **自动选择**：A 段口径**只约束用户手填的网络地址**；B 段机器生成的相对地址在含空格/括号时
  自动包 `<…>`（`assets.rs::relative_url`，中文仍裸放），预览侧改写函数同款处理
  （`ui/preview.rs::resolve_relative_images`）。与 `expand_wikilinks` 包 `<>` 是同一先例。
- **理由**：A 段的「不包裹」是为了不替用户改写输入；B 段的地址是我们自己生成的，语法合法是
  生成方的责任。
- **如何改**：若坤哥希望连本地文件也保持裸写，删 `relative_url` 与 `resolve_relative_images`
  两处的包裹分支，并把复制时的改名策略换成「空格/括号替换为 `-`」（改名比留语法残骸干净）。
- **附记（现状冲突，非取舍）**：image-plan §1 写「图片渲染走 vendored 层 label.rs:844」，
  实况是该分支整体在 vendored 的 `images` feature 之后且上游默认关——B 段已在 latermd-app 侧
  启用该 feature 并装 `egui_extras` 图片 loader（file/image，解码格式开到 D 段白名单
  PNG/JPEG/GIF/WebP）；vendor 文件一行未动。

## #35 图标体系是否整体迁移到 egui-phosphor（2026-09-27 坤哥转来外部建议；2026-09-29 拍板乙执行、**同日实机回滚甲**）

- **岔路**：外部推荐 P1「集成 egui-phosphor，替换所有文字按钮为图标+文字组合，工作量低」。
  查证（2026-09-27）：库本身**健康** —— 0.14.0、14 天前 bump 到 egui **0.36**（与我们对齐）、
  ~2900 下载/月、**#65 in GUI**、MIT/Apache-2.0、字体 bundled（1.03 MiB，可 subset 裁剪）。
- **首轮不做的理由（不是否决库，是否决时机）**：① 我们已有 **30+ 枚自绘图标**（`ui/icons.rs`：
  文件组 / 格式组 17 枚 / 标题栏六钮 / 侧边栏四页 / 状态），且 LOGO 也自绘同源（`assets/logo/`），
  已过明暗双主题与像素验收；② **两套图标并存 = 视觉分裂**，是所有选项里最糟的，只能「全换」或「不换」；
  ③ 全量迁移要动工具条 + 文件栏 + 侧边栏 + 标题栏 ≈ **2–3 天**，不是外部估计的「低」。
- **首轮选择**：本轮**不换**。精致度走 `ui-modernization.md` 的 U0/U1（抄 armas 数值进 tokens +
  Inter + CJK fallback），零依赖、约 1 天。
- **拍板与执行（2026-09-29，坤哥指令「合并 feature/ui-modern-all，完成 egui-phosphor 整体图标迁移」）**：
  选**乙**。`feature/ui-modern-all` 上的 ⑥（0327480，2026-09-27）cherry-pick 到 main 头上，落定要点：
  ① 30 个变体人工映射 phosphor 码位（Save→FLOPPY_DISK、Theme→PALETTE 等名字对不上的逐个挑）；
  ② 字形是 bundled TTF 的私有区码位（U+E0xx），三平台一致，「自绘不用字体」的旧论据只对系统字体成立，
  对 bundled 字体库不成立（#34 已修订）；③ 绘制退化成排版一段文本，字体族走 **Proportional** 而非单开
  `FontFamily::Name` —— 后者遇到没跑 `fonts::install` 的 Context 直接 panic，前者只是豆腐块；
  ④ `build_definitions` 里 `add_to_fonts` 落位 **Inter 之后、NotoEmoji 之前**（实测 insert(1)，探针+单测
  `inter_leads_and_emoji_order_survives` 双重锁定），与 CJK/emoji 互不抢；⑤ 补 `Icon::Image`（#26 图片框
  在分支分叉后新增的变体）映射 `ph::IMAGE`；⑥ `egui-phosphor 0.14` 按惯例登记 ADR-004。
  **分流记录**：该分支的 ①⑤（tokens/theme 自研版）、③（Inter 内联正文）、④（按钮 hover 淡入）分别被
  main 已落地的 #27 U0/U1/U3 替代，不随本迁移合入；分支打 archive tag 后删除。
- **如何改（两个选项，存档备查）**：
  - **甲**：不换。需新增图标时继续自绘。
  - **乙（已执行后回滚，见下）**：单开一棒「图标全量迁移 phosphor」，`ui/icons.rs` 自绘退役，与图片/图床主线**串行**做。
  - 回退：revert `da8061a` 即回到自绘版（icons.rs 消费者 API 不变）。
    注意选乙后，`Icon` 枚举的所有消费者（`titlebar`/`sidebar`/`format_bar`/`menubar`）要同步改。
- **实机目视回滚（2026-09-29，坤哥看真机后「这图标还没原来的好看」）**：拍板乙的同一天真机验收 30 枚
  phosphor 图标，观感整体不及原自绘体系 —— 自绘 30+ 枚是按 LaterMD 的线宽/密度/明暗 token 逐枚画的
  （过 #27 明暗像素验收），phosphor 是通用图标集，笔画的粗细节奏与外壳不咬合；映射里还有 Save→软盘这类
  语义让步。**拍板回滚甲**：revert `da8061a`（`ca6e164`），`ui/icons.rs` 回自绘版（含 #26 的
  `Icon::Image`），`fonts.rs`/`Cargo.toml`/ADR-004 依赖同步还原；`egui-phosphor 0.14` 从依赖表移除。
  phosphor 线的完整实现保留在 git 历史（PR #55、`archive/feature-ui-modern-all` tag），将来若要重启
  （如 phosphor 换 weight 变体、subset 裁剪后再评）从那里捡。**教训**：图标这类纯视觉决策，像素验收
  只能证「画了且一致」，「好不好看」必须在真机上人眼拍板 —— 乙的执行成本（迁移+回滚）本身印证了
  首轮「全换要 2–3 天」的估算是准的。
- **2026-09-27 更新**：坤哥指令「根据文档的最近规划，图片，emoji，ui等功能，添加新的流水线workflow」——
  UI 现代化已按**甲**（不迁移）排入流水线（auto-plan #27 已放行，范围 U0/U1/U3，预写脚本待看护自动开棒）；
  U4/phosphor 本条**仍待拍板**，选乙时在 #27 完成后单开一棒。

## #36 Emoji 面板的数据源选型（2026-09-27，坤哥指令「工具栏加入 emoji」，**待拍板**）

- **岔路**：面板里那批 emoji 的数据（字符 + 名称 + 短码 + 分类）从哪来。
- **两个选项**：
  - **甲（推荐 / 当前默认）内置精简表** —— ~300 枚 / 8 分类，中英双语名 + 短码，约 12KB 常量表，**零新增依赖**。
  - **乙 引 `emojis` crate** —— 1MB 数据、1800+ 枚、Unicode v17、含 gemoji 短码。库本身健康
    （0.9.0 / 2026-06 发布 / 37 万下载月 / 144 crates 使用 / 纯数据无 unsafe），许可证
    `(MIT OR Apache-2.0) AND Unicode-3.0` 与 MIT 兼容；但要按 **#4** 登记进 ADR-004，
    且 Unicode-3.0 的署名要求要带进 `docs/distribution.md` 的第三方声明。
- **自动选择（甲）的三个理由**：① 编辑器里实际会点的 emoji 就那几百枚，全量大半用不上；
  ② **隐藏成本** —— epaint 的字形是**渲染过才进字体图集**，全量数据下用户翻一页就把整页
  emoji 渲进图集，反而更容易触发图集扩容/重排（这是选乙容易被忽略的代价）；
  ③ 少一个依赖就少一次 ADR 滚动修订，与「能抄数值的不引库」一致。
- **如何改**：选乙 → `latermd-app/Cargo.toml` 加 `emojis = "0.9"` + 同步 ADR-004 登记一行；
  选甲 → 直接在 `ui/emoji_panel.rs` 写常量表，无外部动作。
- **附一条实测（防以后走弯路）**：网上教程让开 `monochrome_emoji_fonts` 以获得 emoji 字体 —— 该
  feature 在 **0.36.2 已不存在**（已并入 `default_fonts`），照抄编不过。egui 0.36.2 的默认字体链
  **本来就含** `NotoEmoji-Regular`，不需要任何额外动作，也不需要直接依赖 `epaint_default_fonts`。
- **2026-09-27 更新**：坤哥指令把 emoji 排入流水线（auto-plan #28 已放行，预写脚本待看护自动开棒），
  未另行拍板 → 按本条既定默认**甲·内置精简表**执行；事后想改乙，按上方「如何改」操作
  （`emojis = "0.9"` + ADR-004 登记），改后 amend #28 脚本即可。

## #34 ui-polish §1.1「图标自绘」的论据需要细分（2026-09-27，查证后修订）

- **岔路**：ui-polish §1.1 写「图标一律用 Painter 画线段/圆/矩形，零字体依赖」，
  论据是「emoji / Unicode 符号（✎ 🗋 ⌘）在三平台字体下缺字风险真实」。
  用这同一条论据去否 `egui-phosphor`，**是错的**。
- **修订**：缺字风险要按「字体从哪来」分两类 ——
  | 类型 | 缺字风险 | 结论 |
  |---|---|---|
  | 系统 emoji / Unicode 符号 | **有**（三平台字体不同） | 原结论成立，**继续禁用** |
  | 内嵌字体的图标库（phosphor，bundled font bytes） | **无**（字体随 crate 分发，不查系统字体） | 原论据**不适用** |
- **选择**：保留「自绘」作为当前实现，但**禁用/采纳的判断依据改为「返工成本 + 视觉一致性」**，
  不再引用「字体依赖风险」（见 #35）。`ui-polish.md` §1.1 原文的字体缺字论据仍然有效，不改动原文。
- **如何改**：将来若再评估图标库，直接看 #35，不要用「有字体依赖」一句话打发。

## #33 状态栏右端是否放内容（2026-09-27，M5 像素验收发现）

- **岔路**：M5 像素验收断言「状态栏左右端 50px 内均有非纯背景采样」右端不满足——`status_bar`（`ui/layout.rs:526`）是 `horizontal_wrapped` 左起排列，当前内容（文档名 · 行列 · 字数 · 主题 · 模式 · 后端 · AI · MCP）总长约 320px，未触及右端，右端 50px 为纯底色。**底色横跨全窗已像素级验证成立**（900 列连续，仅 4 列因文字笔画边界判非连续），不满足的只是「右端有内容」这半句。
- **选择**：维持左起排列不动。右端放什么（MCP 端口右对齐？git 分支？时钟？）是产品内容取舍，验收棒不加功能（AGENTS §7 范围边界）。验收文档如实记录「横跨 PASS / 右端无内容采样（设计现状）」。
- **如何改**：若要右端有常驻指示，在 `status_bar` 里加 `with_layout(Layout::right_to_left)` 段或用 `ui.allocate_ui_with_layout` 撑开左段后右对齐放元素；改完状态栏跨窗断言的右端子句自动满足。

## #32 编辑器区顶部文件工具栏退役，文件动作收口左栏图标版（2026-09-27，用户指令）

- **岔路**：坤哥 2026-09-27 第二批反馈「文件动作在左栏和编辑器顶各有一排，重复」。左栏顶段（M2 起）与编辑器区顶部文件工具栏（ui-polish 批次的 `ui/toolbar.rs`：新建/打开/保存/另存为/导出文字按钮 + 视图组 + AI 下拉 + 齿轮）并存近一天。
- **选择**：**保留左栏图标版**并核对为最全集（`Command::FILE` + `ExportHtml`，共五枚，tooltip 带出厂键位）；`ui/toolbar.rs` **整体删除**（含 `mod` 声明与 `icons::icon_text_button`/`allocate_button`/`tooltip` 三个仅存工具栏消费者的辅助函数）。AI 三命令不新增入口（菜单栏「AI」菜单本就有，原工具栏下拉为重复）；`ToggleSidebar`/`ToggleTheme` 不重复加（标题栏已有 `┃左`，主题在菜单栏「视图」与设置浮窗外观页三态）。
- **顺带迁移**（工具栏的唯一无主能力）：`document.notice` 提示行（撞键拒绝、保存失败等「知道了」式提示）迁到编辑器面板顶、标签条之下，存在才显示（`ui/layout.rs` 的 `notice_bar`）；文档名与齿轮在标题栏/状态栏已有，无能力丢失。
- **如何改**：想要回编辑器顶部的文字按钮排，从 git 历史恢复 `ui/toolbar.rs` + `ui/mod.rs` 的 `pub mod toolbar;` + `ui/layout.rs` CentralPanel 里的 `toolbar::ui` 调用即可，三处一一对应。

## #31 设置入口从左栏底段挪到标题栏右端齿轮（2026-09-27，用户指令）

- **岔路**：坤哥 2026-09-27 运行实测外壳后指令「设置挪右上」。原 M2 规格（ui-shell-redesign.md §5）把设置放在左栏**底段**（齿轮 + 「设置」一行，左键默认页 / 右键四项直达）。
- **选择**：左栏收回为三段（顶动作 / 视图导航 / 中段视图内容），设置入口改为**标题栏右端齿轮**（最小化按钮左侧，七个窗口按钮中的第 4 个）；左键发 `SettingsOpened(Appearance)`（默认落地页不变），**右键四页直达随迁到齿轮右键**（不丢能力）。`NAV_BOTTOM_H` token 与 `SidebarBands::bottom` 一并清理。
- **顺带修复**（同一批用户反馈，根因同为 panel 顺序）：①四栏黑条——编辑器曾用 `Panel::left`，其后中央残余区无人认领，改回 `CentralPanel`；②状态栏横跨——`bottom("statusbar")` 挪到 nav/preview **之前**画，先画者占满全窗横向。
- **禅定不受影响**：draw_zen 整体分叉，标题栏（含齿轮）按 D4 决策保留，statusbar 照旧退场。
- **如何改**：想把设置挪回左栏，恢复 `ui/sidebar.rs` 的底段（git 历史里有 `settings_row`）并从 `TITLE_BUTTONS` 摘掉 `Settings`；齿轮与窗口按钮同格（`WINDOW_BTN` 命中区），若嫌它混进窗口控制区，可改为独立小图标按钮并加一格间距。

## #30 外壳重构的五个岔路口（2026-09-26，**已拍板落地：M1–M4 合入 main**）

- **岔路**：坤哥指定「左导航 / 中源码 / 右只读」三分栏 + 右上开关 + 禅定。规格全文在 [ui-shell-redesign.md](ui-shell-redesign.md)。其中五处是真正需要人拍的取舍，不像以往那样「自选一个继续」—— 因为 D1 改的是**窗口形态本身**，代价不可逆，不符合本文件「先干再改」的既定玩法，故登记等待而非自选。
- **为什么这次不自选**：#1–#29 的岔路失败后按「如何改」一节回退的成本都在几百行代码内；D1 一旦落成无边框，三平台的 resize/阴影/圆角行为要真机逐项验收（R1），返工要动 `main.rs` 的 `ViewportBuilder` + 自绘层的全部拖拽逻辑。
- **D1 标题栏**：**自绘**（`with_decorations(false)` + 自绘 36px 条，六按钮）vs 保留原生装饰、开关退到菜单栏右端。默认自绘；逃生口 `LATERMD_NATIVE_DECORATIONS=1`。
- **D2 菜单栏**：保留独立行（可发现性优先）vs 并入 ☰（省 28px）。默认保留。
- **D3 左栏形态**：单栏三段式（记号：改动集中在 `ui/sidebar.rs`）vs VS Code activity bar（两个 panel，省横向地方）。默认三段式 —— 只有 4 个视图，不值两个 panel。
- **D4 禅定**：保留标题栏（鼠标移到顶部 2s 淡出，v2 做）vs 全无 chrome（只剩 Esc 退出，可发现性差且与 macOS 全屏手势打架）。默认保留。
- **D5 可视化编辑**：预留接口不实现 vs 现在就动手。默认预留 —— 它是 Live Preview v2，属 P3 剩余项。
- **如何改**：拍板后按 ui-shell-redesign.md §12 的 M1–M5 顺序开工；D1 若选保留原生装饰，M1 退化为「菜单栏右端加三个开关按钮」，省约 1 天且 R1 整体消失。

## #29 SaveAs 撞上已在另一标签打开的路径:写盘前拒绝认领(2026-09-26)

- **岔路**:独立评审指出 `State::save_to` 认领路径前不查重,另存为到已在另一标签打开的路径会破坏「同一路径至多一个标签」不变量(此后两边各保存一次就互相静默覆盖)。修法两派:①写盘前拒绝认领 + 提示行指路(先关另一标签或另选路径);②允许保存并自动接管 —— 写盘成功后关掉另一标签(干净时)或仅在另一标签脏时才拒绝。
- **自动选择**:①**写盘前拒绝**,提示「{路径} 已在另一标签打开;请先关闭该标签或另选保存路径」(`State::save_to` 入口按 `find_by_path` 查重,命中且非本标签即返回,盘上内容与另一标签缓冲均不被触碰)。理由:与仓库既有哲学同构(撞键拒绝、脏标签关闭要确认 —— 静默覆盖与标签凭空消失的代价都大于多一次操作);②的「自动关掉干净标签」虽无丢稿风险,但用户眼里的标签消失同样是意外行为,且要连带处理关谁的 active 指向与在途 AI 流作废,复杂度不成比例。保存到本标签已持有的路径(常规 Ctrl+S / 同路径另存为)不受影响 —— 命中即本标签,放行。
- **已知并接受的边界**:查重走 `find_by_path` 的逐字节路径比较(与打开侧三入口同一口径),不做 canonicalize —— 符号链接、相对/绝对混写、大小写不敏感文件系统上的别名路径查不出(打开侧同样查不出,如要收紧须两边一起)。
- **如何改**:要②的「另存为即接管」,把 `save_to` 的拒绝分支改成写盘成功后对命中标签走 `remove_tab`(另一标签脏时仍拒绝,文案不变);要支持别名路径,给 `find_by_path` 加 canonicalize 并同步覆盖打开侧,防两边口径不一。

## #28 WorkBuddy 风外壳：强调色从紫罗兰改蓝、面板靠底色分区（2026-09-26）

- **岔路**：坤哥看过 WorkBuddy 截图后要求「UI 要这种风格」。冲突点：①既有强调色是紫罗兰（与 `ai://` 链接同源，decisions-pending #11/#24），WorkBuddy 是飞书系蓝；②egui 出厂的 light/dark visuals 是"灰底 + 硬边框"的桌面风，WorkBuddy 是「侧栏灰 #F2F2F2 / 内容白 / **无硬边框**，靠底色分区」；③皮肤系统（#24）只管正文，外壳配色按批次 C 约定"随皮肤换色"是不做的。
- **自动选择**：①强调色改**飞书系蓝**（浅 #3370FF / 暗 #6C9FFF），AI 专属元素（ai:// 链接、指令卡）**保留紫罗兰** —— 强调色中立化后，AI 反而是全界面唯一的紫，更醒目；②新增 `theme::shell_tokens(dark)` + `apply_shell()`：把侧栏 #F2F3F5、内容 #FFFFFF、文字 #1F2329、悬停、浅蓝选区、控件圆角 6 等投影进 egui 的**两套 style**（只投影一次，`style_mut_of` 会推进 style 版本作废布局缓存）；面板分区用底色不用线 —— `noninteractive.bg_stroke` 压到 border 色一档；③外壳是**内置观感**不是皮肤（皮肤仍只管正文），批次 C 的约定不变。
- **实现里踩的 egui 0.36 坑**：①`Visuals` 已无 `window_rounding`/`menu_rounding` 字段，`Spacing` 也没有，浮窗圆角只能走出厂值；②TextEdit 底 = `extreme_bg_color`，而 vendored 代码块底 = `code_bg_color` —— **两者必须分开**（都给灰的话编辑器整片是灰的，"灰侧栏 + 白内容"就没了）：extreme 给内容白、code_bg 给 #F5F6F7；③`Panel`/`CentralPanel` 无 `.fill()`，预览区用 `CentralPanel::frame(Frame::default().inner_margin(8).fill(content))`。
- **实测验证**（llvmpipe + import 截图 + 像素采样）：侧栏 #F0F3F5、菜单栏灰、编辑器 TextEdit 区白、全图 918 个像素命中 #3370FF（页签蓝条/侧栏选中/链接）；暗色一套同构投影。
- **如何改**：嫌蓝不对就改 `theme.rs::shell_tokens` 与 `ui/tokens.rs::accent` 两处（前后者管页签/选中，前者管面板底色）；AI 元素的紫罗兰在 `ui/preview.rs::ai_link_color`，要跟着改蓝就在那。

## #27 大纲预览跳转：为什么动了 vendor、以及滚动目标的归属（2026-09-26）

- **岔路**：roadmap 写「大纲预览跳转（复用 `section_to_token` 映射）」，但 vendored `MarkdownLabel` 把内容画进单个 galley，`section_to_token` 与布局 y 坐标都不对外暴露 —— app 侧无论怎么算都拿不到「这一节在第几像素」。可选：①动 vendor 暴露锚点；②app 侧按字节比例估算 y；③不做跳转。
- **自动选择**：**①动 vendor（①类，纯新增能力）** —— `SectionAnchor { byte_start, y }` + `section_anchors(ui, id)`，在 `render_galley` 两条分支记录各 section 顶部 y 到 `ui.data`。②不可接受：代码块、表格让「字节比例 → 像素」的误差大到能差好几屏，跳转就失去意义。
- **滚动目标算谁的状态**：**UI 关注点，不是文档状态** —— 存 `PreviewState::scroll_target`，由预览绘制消费一次（与侧边栏把手、键位捕获同一口径）。若走归约，则每帧都会重新滚动，用户再也滚不动预览。
- **已知并接受的边界**：`byte_start` 是**渲染文本**（经 wikilink 展开 / heal）内的偏移，而大纲 span 基于**源码**；文档里有 `[[wikilink]]` 时两者会错位（误差等于展开新增的字符数）。heal 对完整文档是恒等变换，故绝大多数文档不受影响。跳转粒度是**节**（section），不是精确的标题行 —— 落在标题所在节的顶部。
- **如何改**：要精确对齐，在 `expand_wikilinks` 同时产出「源码偏移 → 渲染偏移」的映射表，跳转前换算；要做到标题行级，让 vendored 记录每个 token 的 y 而非 section 的 y。

## #26 `[[wikilink]]` 的三处口径：展开位置、代码块豁免、匹配规则（2026-09-26）

- **岔路**：roadmap 只写了「`[[wikilink]]` 双向链接（LinkHandler）」，落地三处自由度。①`[[目标]]` 在哪一层变成可点击的链接（改源码 / 改解析 / 只改渲染）；②代码块里的 `[[…]]` 算不算链接（`arr[[0]]` 这种 Rust 代码会被误伤）；③「目标」怎么匹配到文件（精确文件名 / 去扩展名 / 大小写 / 子目录）。
- **自动选择**：①**只改渲染** —— `latermd_md::expand_wikilinks` 把 `[[目标]]` 展开成 `[显示名](<wiki://目标>)`，结果存进 `PreviewState::rendered`（随修订号重建，空闲帧不付代价），**源码一字不改**（roadmap P0 验收「`.md` 保持原样」）；点击由既有 LinkHandler 拦 `wiki://`（与 `ai://` 同一条通道），颜色取青绿与 AI 紫罗兰区分。②**围栏代码块内的 `[[…]]` 不算链接**（以 ``` / ~~~ 切换代码态，与 CommonMark 一致）。③匹配走 `filetree::find_by_name`：文件名去扩展名后**忽略大小写全等**、扩展名须 md/markdown；目标带 `/` 时按相对路径直取；遍历复用 `latermd_search`，因此同样尊重 `.gitignore`。
- **已知并接受的边界**：目标文档按**文件名**解析，不做「标题即文档」的别名解析（同一标题多篇文档会歧义）；超过 `MAX_LIST_ENTRIES`(500) 的库尾部分可能找不到（与搜索结果截断同语义，不谎称全库精确）；`[[目标|显示名]]` 支持显示名，但反向链接面板（谁引用了我）没做，属后续（2026-10-03 补记：已由 #15 BK1/BK2 落地，扫描与面板口径见 #87/#88）。
- **如何改**：要「标题即文档」，在 `find_by_name` 未命中时退回遍历各文件的首个 H1 做匹配（代价：全库读头，需缓存）；要反向链接面板，用 `latermd_md::wikilinks` 对每个文件扫一遍建索引（代价：库大时要后台线程 + 增量）。

## #25 Live Preview v1 的五处口径：块粒度、落点、路由、重算、持久化（2026-09-26）

- **岔路**：roadmap 阶段 5 只写了「光标所在 block 显示源码」+「v1 可简化为聚焦时整条源码裸出来」，落地时五处自由度。①块按什么粒度切（段落 / 行 / token span）；②点非活动块时光标落在哪（精确 hit-test 还是块末）；③↑↓ 跨块怎么走、Home/End 要不要接管；④编辑导致块分裂/合并后，活动块按序号记还是按光标记；⑤`render_mode` 要不要持久化。
- **自动选择**：①**块 = `latermd_md::blocks` 的字节区间**（段落按空行切、块级 token 独占、列表每项一块），且**连续覆盖全文** —— 这是安全底线：光标块是被编辑的区间，落在块外的字节会在敲键时静默丢失（单测 `assert_covers_text` 钉住）；②点击落点取**块末**（精确 hit-test 要反查文本布局，v1 不追求像素级精确）；③↑ 在块首去上一块**末尾**、↓ 在块尾去下一块**开头**，Home/End 仍交给 TextEdit 内建；④活动块**按光标字节重定位**（`LiveState::sync`）—— 按序号记必然错位：在第 2 块开头敲回车后原第 3 块变成第 4 块；⑤**不持久化**（会话内偏好，持久化需新增配置文件，收益小于成本）。
- **如何守住铁律**：活动块的编辑经 `BlockBuffer`（egui `TextBuffer` 适配）按「块内偏移 + 块首偏移」落回**同一个 `EditorBuffer`**，文本没有第二份真源；两种模式只是 `render_mode` 一个标志的分派，切换零搬运 —— 单测 `toggling_live_preview_only_flips_the_flag` 钉住（文本/修订号/dirty 都不变）。
- **已知并接受的边界**：块是**整块**切换，不做内联标记半隐藏（`**` 只隐藏一半那种），属 v2；每块的 undo 快照是本块的（跨块撤销按块分段，不是全文一步撤销）；MarkdownLabel 不返回响应，非活动块的点击区域用渲染前后的 cursor 差值框出来（近似区域）。
- **如何改**：要做像素级点击落点，用 `MarkdownLabel` 的布局信息反查字符偏移（需 vendor 侧暴露，属 ①类改动）；要全文统一 undo，把 undoer 从 TextEdit 内建换成自维护的（代价：IME 组合与选区行为要自己兜）；要 v2 内联半隐藏，在块内再按 token span 分段渲染（roadmap 已列为 v2）。

## #24 皮肤批次 B 的四处口径：System 解析、皮肤存储、密度基准、文件名（2026-09-26）

- **岔路**：roadmap「专题：界面美化与皮肤系统」批次 B 只写了「三态切换 + 自定义皮肤文件 + 视觉打磨」，落地时有四处自由度。①`跟随系统` 是个**非确定值**：检测结果缓存在哪、多久刷一次、检测不到怎么办；②皮肤文件用什么格式、存哪、内容要不要再存一份进 `settings.json`；③密度 token 怎么算（以出厂值为基准还是基于当前值缩放、动不动字号）；④皮肤名来自用户输入却要拼进路径。
- **自动选择**：①`ThemeMode::System` 只在 `resolve(detected, fallback)` 处落到确定值 —— 结果缓存在 `State::system_theme`，**仅跟随系统模式才轮询**（1 秒节流；非跟随模式返回 `None`，让 egui 收敛到深度空闲），检测失败与 `Unspecified` 一律回落上一次的手动值并在设置页明示「本机读不到系统主题设置」，不猜；②皮肤文件是**唯一事实源**：`themes/*.ron` 存 `MarkdownStyle`，`settings.json` 只存皮肤名，启动扫描目录把内容载入 `ThemeSettings::skin_style`（`#[serde(skip)]`）—— 避免同一份样式两处存放、改一处另一处不跟着变；用目录扫描而非配置清单，用户把别人给的 ron 丢进目录即生效；③密度以 `egui::Style::default()` 的出厂值为**基准**缩放（间距/控件尺寸 0.7、圆角 0.8、滚动条同比例收窄），不基于「当前值」再乘（否则标准↔紧凑来回切会逐次累积）；**不动字号** —— 中文在小字号下的可读性损失远大于多出来的几行；④皮肤名经 `skin_file_name` 清洗（`/ \ : * ? " < > | .` 全换 `_`，空名给默认名），挡住 `..` 拼进路径。
- **已知并接受的边界**：跟随系统的首次值来自启动那次探测，系统切主题后最迟 1 秒跟上；Linux 上 `dark-light` 走 freedesktop portal，Deepin/KDE 等环境可能恒返回 `Unspecified`（本机实测结果待人工补记）；皮肤只覆盖**正文与代码高亮**（`MarkdownStyle`），外壳配色仍由 egui 自带的 light/dark visuals 决定 —— 要让外壳一起换色需另加 shell token 表，属批次 C（明确不做）。
- **如何改**：要更快的系统主题响应，调小 `SYSTEM_THEME_POLL`（代价：更频繁查 dbus/注册表）；要让外壳也随皮肤换色，在 `ThemeSettings` 加 shell token 表并在 `apply_density` 同处投影；要让皮肤内容也进 `settings.json`（自包含），去掉 `skin_style` 的 `#[serde(skip)]` 并在 `select_skin` 里同步写回 `overrides`（代价：两份真源，改皮肤文件后界面不变）。

## #22 MCP server 落地的六处口径（2026-09-26）

- **岔路**：mcp-plan.md 给了形态与工具集，落地时仍有六处自由度。①HTTP 与 stdio 谁是主通道（GUI 进程内的 stdin 不是管道，stdio 在常驻进程里没有客户端）；②`tools/call` 缺 `name` 该怎么报错；③关掉的工具是「调了才拒」还是「对客户端不存在」；④文件树换根后服务要不要重启；⑤`--mcp-stdio` 子进程模式要不要受 `mcp.json` 的 `enabled` 约束；⑥`list_files` 的 glob 用什么实现（引 `glob` / `globset` 还是复用 `ignore`）。
- **自动选择**：①**HTTP 是 GUI 进程内的主通道**（应用开着就能被调 —— 坤哥的诉求原话），stdio 作为 `--mcp-stdio` 子进程模式给 `claude mcp add` 这类客户端，两者共用同一个 `Server`，只是传输不同；②缺 `name` 走**协议层 `InvalidParams`**（-32602），工具执行失败才走 `isError` 内容块 —— 前者是请求格式问题、后者是工具结果，混在一起客户端不好分支；③关掉的工具**不出现在 `tools/list`**（最小权限要真的生效，而不是「列出来让你调、调了才拒」），真被点名时仍回「工具已在设置里关闭」；④换根走 **`SharedRoot` 共享句柄**（`Arc<Mutex<Option<PathBuf>>>`），服务不重启；⑤headless 模式**不看 `enabled`** —— 用户显式用参数启动就是一次授权，而 `enabled` 管的是「GUI 进程内是否自动监听端口」这件不同的事；根取环境变量 `LATERMD_MCP_ROOT`；⑥glob 走 **`ignore` 的 override 匹配查询**（`Override::matched(path, is_dir)`）而非 `builder.overrides()` —— 实测后者只筛文件、目录条目照旧产出（`*.txt` 会带出 `notes` 目录），而列目录的语义是「条目本身要不要出现」，目录必须过同一把筛子；零新增依赖。
- **已知并接受的边界**：HTTP 侧**单连接串行**（工具是毫秒级检索，排队即可，也避开「两个 AI 并发改同一个编辑器缓冲」）；不实现 MCP 的 `resources` / `prompts` / `sampling` 与 SSE 长连接流（客户端要 SSE 时按单帧 `data:` 回，语义与 JSON 一致）；`outline` 的行号按标题 span 换算，而 span 会吸收上一块尾部的换行（latermd-md 的已知行为），换算时跳过前导换行。
- **如何改**：要 stdio 当主通道，把 GUI 启动的 `http::serve` 换成 `stdio::serve`（代价：常驻 GUI 的 stdin 无处接客户端，等于放弃「应用开着就能被调」）；要让关掉的工具仍出现在列表里，去掉 `tool_list` 的 `filter`、保留 `tool_call` 的拒绝分支；要并发 HTTP，把 `serve_with` 的 accept 循环改成每连接一个线程（需同步处理工具对同一库的并发读）。

## #23 多标签归约迁移的三处消息口径:TabOpen 不引入、确认关闭不带载荷、孤立 chunk 丢弃(2026-09-26)

- **岔路**:multi-tabs 棒的任务规格写「Message 新增 `TabOpen { path }`、`TabCloseConfirmed { index }`」,而 main 上已落地的多标签骨架(`crates/latermd-app/src/tabs.rs` + state.rs 归约)用了不同的等价结构;另有一个规格没覆盖的防御分支(在途流的发起标签已不存在时,迟到的 `AiChunk` 写到哪)需要定口径。
- **自动选择**:①**不引入 `TabOpen`**——「打开」的三个入口(菜单「打开」对话框 / 文件树点击 / 搜索跳转)都在归约内部完成「路径去重 → 命中激活 / 未命中开新标签」(`State::open_path`),UI 层没有任何场景需要直接产出 `TabOpen`,引入无人产出的消息只增表面积;②**`TabCloseConfirmed` 不带 `{ index }` 载荷**——确认目标存在 `TabsState::confirm_close`(请求时刻的快照),确认的必是弹窗所问的那个标签,比消息载荷更防错(载荷在模态期间标签增删后会指错对象);③**孤立 chunk 丢弃并作废流**(`State::append_ai_delta` 的 `ai_stream_tab_index() == None` 分支)——发起标签被关闭时 `remove_tab` 已先行作废流,真实链路走不到该分支;万一未来重构弄丢绑定,fail-safe 是「丢块可见(续写中断)」而非「静默写进 active(写错文档)」。
- **如何改**:要让 UI 能直接开标签(比如将来的拖拽打开),加 `Message::TabOpen { path: Option<PathBuf> }` 并在归约里转 `open_path`/`spawn_tab` 即可;要确认关闭改带载荷,给 `TabCloseConfirmed` 加 `usize` 并在 `layout.rs` 的 `tab_close_dialog` 处带上 `confirm_close` 的值;要孤立 chunk 落到当前标签,把 `append_ai_delta` 的 `else` 分支改成 `self.tabs.current_mut()`(须接受写错标签的风险,不建议)。

## #22 界面打磨批次的四个口径:图标自绘、撞键拒绝、未实现项禁用、MCP 只出规划(2026-09-25)

- **岔路**:用户指令「图标、快捷键设置、AI 配置页、MCP 规划」留了四处自由度。①egui 无图标集,用 emoji/Unicode 字符(✎ 🗋)还是自绘?②改键撞到别的命令的键位时,抢占还是拒绝?③AI 配置页的「接口方式」里 Anthropic/Ollama adapter 还没写,下拉里给不给选?④MCP 做到什么深度?
- **自动选择**:①**全部自绘**(`ui/icons.rs`,`Painter` 线段/圆/矩形,归一化坐标)—— emoji 在三平台缺字风险真实(AGENTS §5 已把字体列风险项),自绘零字体/纹理依赖且随主题取色;②**拒绝并指名占用者**("Ctrl+K 已被「打开」占用,未修改")—— 静默抢占会让用户莫名丢另一个命令的键位;裸字母/数字一律拒绑(会被编辑器当输入吞掉);③**显式禁用并写明"未实现"**—— 伪装可选会让用户配完发现没生效,与 decisions-pending #12 同口径;④**只出规划文档**([mcp-plan.md](mcp-plan.md))与设置页禁用态开关,不写半截 server —— MCP 是新增范围(AGENTS §7 深水区之外),该有单独立项,设置页伪造"运行中"不可接受。
- **如何改**:要换字体图标方案,`ui/icons.rs` 的 `Icon::draw` 是唯一绘制点;要改抢占语义,`State::assign_shortcut` 的 conflict 分支改 `set` 即可;要实现 Anthropic/Ollama,在 `latermd-ai` 加 adapter 并放开 `settings.rs` 里 `implemented()` 的两个禁用点;MCP 开工按 mcp-plan.md 的阶段表走。

## #21 设置面板 AI key 接线的三岔路：浮窗形态、状态分组、key 闸门位置（2026-09-25）

- **岔路**：任务写「Settings 面板新增 AI Provider 区」，但仓库没有独立 Settings 面板实体——设置只有工具栏的「设置」`menu_button`（`ui/toolbar.rs`），且仓库自己的注释证明「egui 菜单内点击任意控件自动收起」，把密码框 TextEdit 直接嵌进菜单有「点进输入框菜单即收起」的交互风险；任务又写「State 增加 `ai_key_configured: bool`」，字面平铺与仓库的状态分组风格（`SidebarState`/`SearchState`/`AiState`）相悖；key 闸门（provider 启动链路）若放流式共用入口 `start_ai_stream_with_prompt`，`AiStart` 的外层归约会先补空行、`AiSummaryRequested` 会先移除旧摘要节——被拦的命令留下副作用。
- **自动选择**：①「设置」菜单加「AI Provider…」入口（原地翻转 `dialog_open`，同 `SidebarState::visible` 的 UI 关注点口径），密码框/保存/清除/状态行放独立 Window 浮窗（与 commit 建议浮窗、回滚确认浮窗同模式，`crates/latermd-app/src/ai_key.rs::ai_key_dialog`）；②状态分组成 `State.ai_key: AiKeyState`（`configured`/`backend_ok`/`draft` 在内），任务字段的语义落点 = `state.ai_key.configured`；③key 闸门放在**每个 AI 命令归约的最前面**（`AiStart`/`AiLinkClicked`/`AiCommitRequested`/`AiSummaryRequested` 四入口，模式 `if self.ai.is_streaming() || !self.ai_key_gate() { return; }`），被拦命令零副作用；provider 是否需 key 由 `AiState::provider_requires_key` 表达，当前 Mock 恒 `false` 直通（无 key 也能跑），主模型启用时随 provider 置 `true` 即生效——测试已覆盖置 `true` 后的拦截/放行两分支。
- **如何改**：要密码框直接长在菜单里，把控件从浮窗搬进 `menu_button` 闭包并实测菜单收起行为；要平铺字段就把 `AiKeyState` 拆散上提到 `State`；要 Mock 也强制配 key，把 `provider_requires_key` 默认值改 `true`（测试 `ai_stream_blocked_without_key_when_provider_requires_it`/`mock_provider_runs_without_key` 同步改）。

## #20 latermd-creds 的四岔路：keyring 维护线、get_secret 签名、env 回退测试注入、测试值口径（2026-09-25）

- **岔路**：P2 凭据 crate 落地时任务留了四处自由度。①keyring crate 有两条版本线：3.6.3（hwchen 原维护线终版，无默认 features，需手工配平台组合，已随项目移交停更）与 4.2.0（open-source-cooperative 接管后的重构线，2026-08 仍更新，默认 feature `v1` 即三平台 store）；②任务签名写作 `get_secret(...) -> Option<String>`，但同批约束要求「所有后端调用优雅降级（Err 返回）」且单测要「断言错误文案不含 secret」——Option 装不下错误文案；③环境变量回退顺序的测试：临时改进程环境变量（并行测试竞态）还是注入；④红线「凭据值不进测试断言明文、测试只断言存在性/删除成功」与「内存后端全 CRUD」的关系——CRUD 的 R 不验证读回值就测不出后端正确性。
- **自动选择**：①**keyring 4.2.0**：4.x 是唯一仍在维护的线；默认 feature 按 target 自动落 Windows Credential Manager / macOS Keychain / Linux Secret Service（zbus 纯 Rust 实现，不链 libsecret C 库；Cargo.lock 既有 zbus 条目复用）；keyring-core `Error` 的 `Display` 实测不携带凭据字节（`BadEncoding` 打固定文案）；②`get_secret` 返回 `Result<Option<String>, CredentialError>`（错误可见、文案可断言、不吞「后端坏了」），`has_secret -> bool` 与 `ai_api_key -> Option<String>` 保持任务签名——Err 折叠为 false/None 的降级语义自洽（便捷查询定位，用户重新保存时会看到 set 的真实错误）；③注入式：`Credentials::ai_api_key_from(env_value: Option<&str>)` 显式传环境变量取值（测试注入点），顶层 `ai_api_key()` 内部读真环境变量；④测试值全部是 `placeholder-*` 占位假值，断言只做相等性/存在性比较——验证的是后端读写一致性，不是把真实凭据写进断言。
- **已知并接受的边界**：后端读失败时 `has_secret` 返回 false（「不可用」与「未配置」在便捷查询层不可区分）；keyring 4.x 的 v1 模块在 Linux 无 dbus 时首次 `Entry::new` 即快速失败并**缓存**初始化结果——优雅降级成立，但运行中途 Secret Service 才挂掉的场景不会重试（LaterMD 桌面应用的 keyring 在进程启动后基本常驻，可接受）。
- **如何改**：要回 3.x 线，把 crate Cargo.toml 改 `keyring = "3.6.3"` + `features = ["apple-native", "windows-native", "sync-secret-service"]`（`NoEntry` 匹配同款，改动很小）；要 `get_secret` 恢复纯 Option 签名，删 `Result` 包装并把「错误文案不含 secret」断言收缩到 set/delete；要改用进程级环境变量测试，删 `ai_api_key_from` 注入点、测试里 `std::env::set_var`（须接受竞态或串行化）；要把「存在性-only」测试口径执行得更严，删 CRUD 测试里的相等性断言（代价：后端写坏值不再被测出，不建议）。

## #19 git status 的条数上限与超限文件的行为（2026-09-25）

- **岔路**：独立评审指出 `latermd_git::status` 无条数上限（`recurse_untracked_dirs` 全量展开），叠加同步跑在 UI 线程的 3s 轮询与侧边栏每帧全量渲染，超大仓库会卡帧——log（50 条）与 diff（64KB）都有上限，唯独 status 没有。加多少、超限文件的行为（角标/选中/回滚）怎么定未指定。
- **自动选择**：上限 **500**（`latermd_git::DEFAULT_STATUS_LIMIT`），与文件树 `MAX_CHILDREN`、搜索 `MAX_HITS` 两个既有先例同量级；`status(root, limit)` 返回 `StatusSnapshot { entries, truncated }`，**先按路径排序再截断**（保留字典序最小的前 500 条，保证确定性）；Git 页改动列表尾部渲染「…还有 N 项未显示」（与文件树同款提示行）。超限文件的降级：无文件树角标、不可选中/回滚（select/request_checkout 的「列表外路径忽略」防御天然覆盖）——与文件树截断、搜索 MAX_HITS 同语义。注意：libgit2 的 statuses 遍历本身无法提前截断（StatusOptions 无 limit），本上限消除的是「Vec 无界 + 每帧全量渲染」两项；遍历成本仍属 #17 已登记的「掉帧再挪线程」取舍。
- **如何改**：嫌 500 太小改 `DEFAULT_STATUS_LIMIT` 一个常量（调用方 `git_panel.rs` 自动跟随）；要「显示全部」，给 Git 页加展开交互并让 `status` 支持分页或提高上限；要消除遍历成本，把 `recurse_untracked_dirs` 关掉（未跟踪目录只报目录一条，快得多，但文件树逐文件打标失效）或按 #17 的「如何改」挪后台线程。

## #18 回滚目标恰是编辑器当前文档时，dirty 缓冲的处置（2026-09-25）

- **岔路**：独立评审指出回滚（checkout 单文件）若目标正是编辑器当前打开的文档，归约后编辑器不重载、无提示——dirty 场景一次 Ctrl+S 就把被丢弃的改动静默写回（反转回滚）；非 dirty 场景编辑器显示与磁盘不一致。修法有两派：①回滚后无条件重载编辑器（强一致，但 dirty 时静默丢掉未保存稿）；②分 dirty 分流。
- **自动选择**：**②分 dirty 分流**（`State::after_git_checkout`）：目标非当前文档不触碰编辑器；是当前文档且非 dirty → 重读磁盘换入缓冲（预览同帧联动，编辑器与磁盘重新一致）；是当前文档且 dirty → **保留未保存稿**（静默丢稿的代价大于不一致，与 `unsaved_guard` 的既有哲学一致），提示行明示「已回滚 X：编辑器里未保存的修改仍保留，保存(Ctrl+S)会把它们写回」。确认模态同步加针对性警示（`checkout_dialog` 的 `checkout_extra_warning`）：目标在编辑器中打开时按 dirty 显式告知上述行为，不再只有通用不可逆警示。
- **如何改**：要①的强一致语义，把 `after_git_checkout` 的 dirty 分支改成同样调 `open_from(file)`（并在 `checkout_extra_warning` 的 dirty 文案里说明将丢弃编辑器修改）；要更保守的「dirty 时拒绝回滚」（像 unsaved_guard 那样拦下），在 `request_checkout` 前置检查并落提示行。

## #17 Git UI 接驳的刷新机制、仓库根定位与确认模态形态（2026-09-25）

- **岔路**：把 latermd-git 接进侧边栏时任务留了三处自由度。①「每 N 秒或触发时刷新」的 N 未定，且同步归约执行还是后台线程未定（`git status` 大仓库冷缓存可能上百毫秒，卡帧风险真实存在）；②#16 ③ 已定 latermd-git API 层用 `Repository::open` 严格仓库根，但 UI 的文件树根常是**仓库子目录**（比如选了 `docs/` 当根），以哪个目录调 status/diff/log 没定；③「确认模态」在 egui 0.36 没有内建阻塞模态层，用什么形态承载。
- **自动选择**：①**N=3 秒，同步在归约里执行**（`crates/latermd-app/src/git_panel.rs::REFRESH_INTERVAL`）：与 `git_diff.rs` 的既有口径一致（本地 git 读是毫秒级，不上后台线程），到点由 `ui::layout::reduce` 触发并 `request_repaint_after` 要帧；触发式刷新 = 换根 / 切到 Git 页 / 回滚完成；**降级（非 git 目录）即停轮询**，重探由换根/切页签触发——保证 egui 空闲收敛（search 去抖测试守护的不变量），零成本挂着的失败探测没有价值。②新增 `latermd_git::discover`（`Repository::discover` 向上探测，裸仓库显式 Err）：crate 其余 API 的「严格仓库根」口径不动，UI 接驳层先 discover 把文件树根换算成仓库根；状态条目仍记「相对仓库根」路径，角标拼成绝对路径与文件树条目匹配。③确认模态用**非阻塞 `egui::Window` 浮窗 + 红色警示文案「未提交的改动将被丢弃，此操作不可撤销」**（与 AI commit 建议浮窗同模式）：checkout 只在「回滚」按钮点击后的消息归约里执行，浮窗本身零 git 调用。
- **如何改**：嫌 3s 太钝/太勤，改 `REFRESH_INTERVAL` 一个常量；大仓库实测掉帧，把 `GitPanelState::refresh` 的两次 git 读挪后台线程（对 UI 的接口不变，参照 search 的代际号取消模式）；要收紧回「文件树根必须是仓库根」，删 `latermd_git::discover` 并让 `GitPanelState::refresh` 直接以文件树根调 status（非根目录会走降级提示）；要真阻塞式模态，等 egui 内建 modal 层（0.36 无）或自绘全屏遮罩 Area。

## #16 latermd-git 的三个落地口径：U 的语义、git2 features、仓库根定位（2026-09-25）

- **岔路**：P2 首个 Git crate 落地时任务留了三处歧义。①状态码集合写作 `M|A|U|D|?`，U 是 unmerged（git CLI short format 语义）还是 untracked（VS Code 装饰字母语义）——若 U=untracked 则 `?` 无含义。②ADR-004 登记 git2 0.21.0 的组合是 `vendored-libgit2 + vendored-openssl`，但 vendored-openssl 会拉 openssl-src 全量编译（三平台 CI 各多数分钟），而它的唯一用途是 https 传输。③API 以仓库路径为参数：`Repository::open`（严格根）还是 `Repository::discover`（向上层搜 `.git`）。
- **自动选择**：①**U=unmerged（合并冲突），?=untracked**，按 git CLI `--short` 语义（`crates/latermd-git/src/lib.rs::StatusKind`），与 roadmap「文件树 Git 标记 M/A/U/?」并排五码自洽；②git2 取 **`default-features = false, features = ["vendored-libgit2"]`**（版本 0.21 与 ADR-004 一致）：P2 明确只读、无 fetch/push/pull，ssh/https 传输层整层用不上，关掉后零 openssl 面（不依赖系统包、不编译 openssl-src）；若将来做 remote 再加 `vendored-openssl` 即可；③**`Repository::open` 严格仓库根**，不向上搜——与 #14 app 侧「不向上搜 `.git`」的既有口径一致，非 git 目录一律 `Err` 交给 UI 降级成提示。
- **已知并接受的边界**：status 里非 UTF-8 文件名不出现（libgit2 的 `entry.path()` 返回 `Option<&str>`，非 UTF-8 时为 None，极罕见）；blame 基于 HEAD 提交内容，工作区未提交的行不参与行级归属（libgit2 限制，doc 已注明）；`checkout_file` 的 path 走 git pathspec 语义（与 `git checkout -- <path>` 一致，含 glob 元字符的文件名理论上可被通配匹配）。
- **如何改**：要改 U=untracked，改 `lib.rs::status_kind` 的优先级映射一处（untracked 同时映射 U 与 `?` 的需求不存在，五码本来就单字母）；要恢复 ADR 原样的 openssl 组合，把 crate Cargo.toml 的 features 改回 `["vendored-libgit2", "vendored-openssl"]`（须同时去掉 `default-features = false`，否则 https feature 仍关着）；要支持从子目录自动定位仓库根，把 `open_repo` 的 `Repository::open` 换成 `Repository::discover`，但 status/diff 的相对路径语义需随之在 UI 侧重排。

## #15 AI 摘要的插入形态、引用块前缀来源与 Mock 请求识别（2026-09-25）

- **岔路**：任务把展示形态留成二选一——「以引用块形式插入文档末尾」或「展示给用户可选插入」。另外 prompt 输出要求固定为「每条一行中文，以 '- ' 开头」，而最终插入形态是引用块（`> - …` 行），`> ` 前缀由谁加上、Mock provider 在共用 `stream_complete` 通道里如何区分摘要请求与续写请求，都需要定口径。
- **自动选择**：**直接插入文档末尾**（`crates/latermd-app/src/state.rs::request_summary`）。理由：摘要流式落文档与续写同语义，天然复用 `AiChunk` 追加通道与防重入；浮窗形态走不了流式追加（commit 选浮窗是因为 subject 是「建议」，摘要是要写进文档的内容）。`> ` 前缀由 **Mock 替身直接产出最终文档形态**（`crates/latermd-ai/src/mock.rs::mock_summary` 输出 `> - …` 行序列），prompt 指令保持任务原口径不改；真实 key 接入时在适配层把模型输出的 `- ` 行包成 `> - `（与 commit 的真实 provider TODO 同批，见 `request_commit_message` 的 TODO 注释）。Mock 请求识别用**摘要指令头前缀嗅探**（`stream_complete` 里 `starts_with(SUMMARY_INSTRUCTIONS)`，与现有「按 prompt 关键词选脚本」同构；真实 provider 无此问题，模型自己读指令）。
- **已知并接受的边界**：①摘要节定位按「二级标题 + 文本精确等于 `AI 摘要`」（`latermd_md::heading_section_span`），用户改层级/改名后的旧节不清理（保守匹配，防误删手写内容）；②流失败时旧节已删、新标题已插、要点可能半截——与续写流「失败留半截正文」同语义，演示期 Mock 不产生失败块；③摘要节被用户挪到文档中间且其后还有内容时，删节后正文与下一节间保留一个换行（合法 Markdown，源码视觉紧凑）。
- **如何改**：要改浮窗形态，在 `State` 加 `ai_summary_suggestion: Option<String>` 并把 `request_summary` 改成收流进缓冲区外的暂存（AiChunk 归约需按流类型分流）；要匹配用户变体的旧节，放宽 `heading_section_span` 的层级参数或做模糊文本匹配；要让真实 provider 输出自动加 `> `，在接入 `OpenAiProvider` 时于 app 侧对摘要流的 chunk 做行级包装（需在 `AiState` 加当前流类型标志）。

## #14 AI commit message 的仓库定位与浮窗/复制口径（2026-09-25）

- **岔路**：菜单「AI: 生成 commit message」要读「当前仓库」的未提交改动，但 LaterMD 没有「当前仓库」的概念——文档可以不在任何 git 仓库里，文件树根也可以是任意目录，还可以向上搜 `.git` 找仓库根。另外 ask 把展示（状态栏 vs 对话框）与复制（按钮 vs 自动写剪贴板）留成二选一。
- **自动选择**：仓库定位取**当前文档所在目录，退文件树根目录**，两者皆无则提示「先保存文档或设置文件树根目录」；不在仓库/无 git 时 `git diff` 的 stderr 直接落提示行（`crates/latermd-app/src/state.rs::request_commit_message`）。不向上搜 `.git` 找根：`git diff` 在仓库子目录里跑也返回全仓改动，先找根纯属多余。展示用**浮窗对话框**（subject 要整行可读，状态栏 notice 行是错误专用、红色语义不符）；复制用**显式「复制」按钮**（`Context::copy_text`），不自动写剪贴板——未经用户动作覆盖系统剪贴板会冲掉用户正在搬运的内容。生成路径演示期为 `MockProvider::mock_commit_subject` 同步合成（流式脚本对 commit 场景不适用），真实 key 接入后改走 `OpenAiProvider` 低温度补全取首行（同函数内的 TODO）。
- **如何改**：要支持显式指定仓库（如设置面板里选仓库根），改 `request_commit_message` 的目录解析一处即可；要改自动复制，在 `Message::AiCommitSuggestion` 归约里补 `Context::copy_text(subject)`（消息已带 subject，归约侧拿得到 ctx）；要让建议随换文档消失，在 `State::load_document` 里顺手清 `ai_commit_suggestion`（本轮刻意不清：建议是仓库级派生物，不是文档的）。

## #13 ```ai 指令卡状态行的键控口径（2026-09-25）

- **岔路**：指令卡状态行要求「未执行 / 进行中 / 已完成」三态，需要回答「哪张卡算进行中/已完成」。可选：①按卡片指令文本与最近一次发起的 prompt 匹配（`AiState::last_prompt`）；②按块在文档中的序号维护每卡状态表。
- **自动选择**：①（`crates/latermd-app/src/ui/preview.rs::AiLinkHandler::card_status`）。理由：防重入保证同时至多一个流，「哪张卡发起」由 prompt 文本即足以判定；序号表在用户增删块时会整体错位，还要处理失效清理；文本匹配零新增结构。已知并接受的简化：**两卡片指令文本完全相同则状态同亮**；指令文本被编辑后状态回「未执行」（文本变了=另一条指令，语义自洽）。菜单入口（`AiStart`）的 prompt 是文档尾部拼装文本，不会与任何指令文本相等，菜单流不点亮卡片。
- **失效时机**：`last_prompt` 只在两处清空——流失败（`Message::AiFailed` 归约，失败不算完成）与换文档（`State::load_document`，卡片是文档的派生物）；`AiDone` 后保留，让「已完成」可见。
- **如何改**：要按序号键控（同文卡片状态独立），在 `PreviewState` 加 `card_status: HashMap<usize, AiCardStatus>` 并把 `block_code_widget` 的 `index` 传进消息，即可替换匹配逻辑；消息归约与 vendored 扩展点无需动。

## #12 ```ai 指令块走最小 vendor 改动（代码块级 block widget 扩展点）（2026-09-25）

- **岔路**：任务优先「只用 app 侧扩展点，不动 vendor」。实测 vendored `LinkHandler::is_block_widget`/`block_widget`（`vendor/egui_markdown/src/link.rs`）只作用于 **`Token::Link` 的 href**（判定点 `layout.rs` `append_link_to_job`/`needs_segmentation`/`build_layout`），**够不到围栏代码块**——roadmap 阶段 3 写的「`.is_block_widget()` → `.block_widget()`」对 ```ai 围栏不成立。app 侧唯一代码块扩展点是 `code_block_buttons` 头部 overlay 回调（回调签名 `(ui, text, lang)`，无块序号/span），画不出「卡片 + 状态行」，也拿不到稳定块身份（AGENTS §6.7 的 id 稳定性无从谈起）。
- **自动选择**：给 vendored `LinkHandler` 加**代码块级 block widget** 两方法（`is_block_code_widget(language)` / `block_code_widget(ui, text, language)`，按 info string 判定），与链接 block widget 同构：命中即 segment break，在 `render_token_range` 独立渲染；`needs_segmentation` / `build_layout` / `render_token_range` 三处按上游既有「必须同步」约定同步改。类别 **①上游可合**（通用能力、带 tests/block_code_widget.rs，可 cherry-pick 提上游 PR），登记见 `vendor/README.md` 提交级登记表与 `vendor/egui_markdown/README.md` 差异表 #7。
- **如何改**：若不认可动 vendor，revert 该 ① 类 commit 并把 app 侧退到 `code_block_buttons` overlay 形态（功能降级：状态行并入代码块头、卡片视觉消失、多卡身份按内容 hash 近似）——代价已实测如上，不建议。

## #11 `ai://` 链接协议语义与 prompt 编解码口径（2026-09-25）

- **岔路**：roadmap 阶段 3 对「ai:// 链接协议」只写了「`.link_style()` + `.click()` 拦截」的实现方式，协议本体没有定稿——已实现动作是哪个、未实现动作点了怎么办、prompt 怎么编码、`+` 算不算空格，都得有个说法才能写测试。另实测发现 vendored 的 `LinkStyle.underline` 字段（`vendor/egui_markdown/src/link.rs:86`）当前**没有任何读取点**。
- **定稿**：`ai://write?prompt=<urlencoded 提示词>` 触发 Mock 流式续写，prompt **原样透传** provider（不拼文档尾部——链接作者写的就是完整指令；续写仍落在当前文档末尾，防重入与菜单「AI 续写」同一入口 `start_ai_stream_with_prompt`）。其余 `ai://` 动作（`ai://summarize` 等）**识别但不拦截成执行**：点击提示「未实现的 AI 动作：<action>」。`ai://write` 缺 prompt 参数、prompt 为空、坏 `%` 序列、非 UTF-8 字节，均提示且不执行。解码只用严格 `%XX`（`percent-encoding` 2.3.2，坏序列自行校验补严——该 crate 默认原样放行），**`+` 不当空格**：markdown 链接里作者本就该用 `%20`。非 `ai://` 前缀完全不拦截，走 vendored 默认 `open_url`（系统浏览器）。
- **证据**：vendored `layout.rs:144`（`link_style().color` 决定链接**文字色**）、`layout.rs:197`（hover 下划线对**全部**链接无条件绘制，颜色取 `link_style().color`）、`label.rs:1165`（`click` 返回 true 则跳过 `open_url`）。app 侧 `LinkStyle { color, underline: true }` 里 `underline` 是**声明意图**——vendored 层没人读它，样式区分实际由颜色承担；ai:// 链接取紫罗兰色（明暗主题两档），与默认 `hyperlink_color` 区分。
- **如何改**：新增动作或让 `+` 当空格，改 `crates/latermd-app/src/ai_link.rs::parse` 一处（消息载荷 `Message::AiLinkClicked { prompt: Result<String, String> }` 不变，`Err` 文案在 parse 里拼）；要让 `underline` 字段真正生效需改 vendored 层（①上游可合类），本轮按「优先只用 app 侧扩展点」未动 vendor。

## #10 heal() 的作用时机：逐块(后台线程) vs 整篇(渲染帧)（2026-09-25）

- **岔路**：AI 流式接线 ask 要求「后台线程每块先过 vendored heal()，经 mpsc 回 UI，delta 追加进编辑器 rope」。但 `heal(s)` 的语义是给**残缺文本前缀**补闭合标记（`vendor/egui_markdown/src/parser.rs:45`：`heal("```rust\nlet x = 1;")` → 追加闭合 fence、`heal("**bold text")` → 追加 `**`）。MockProvider 按固定 20 字符切块，块边界会切在代码 fence/加粗中间：若把逐块 healed 文本追加进 rope，闭合标记会被**永久写进文档**，且下一块拼在闭合 fence 之后产生非法残文（例：块尾 `fn invalidate(cache: &mut C` 被补成 `…C\n```` ，下一块 `ache, doc_id…` 紧跟其后再开一个 fence）。
- **自动选择**：heal 移到**渲染帧**、作用于**整篇快照**——预览 `MarkdownLabel` 开 `.heal(true)`（`crates/latermd-app/src/ui/preview.rs`），vendored 层在 parse 前对全文调 `parser::heal`（`label.rs` render 的既定钩子，docstring 即「Useful for streaming LLM output」）；编辑器 rope 只收原始 delta，文档内容不被污染。这与 AGENTS.md §6.5「让 LLM 流式输出的每一帧语法合法」一致——每一帧 = 每次渲染的全文快照。预览对完整文档 heal 是恒等变换（`Cow::Borrowed` 原样返回），非流式场景行为不变。
- **如何改**：若确实要逐块 heal（例如想把「 healed 帧」单独喂给某个纯预览通道、不进编辑器），在 `crates/latermd-app/src/ai.rs` 的 `poll()` 里对 delta 调 `egui_markdown::heal` 并另开一条不落盘的预览通道即可；只要别把 healed 文本写进 `EditorBuffer`。

## #9 流式失败信号与 OpenAI adapter 默认值（2026-09-25）

- **岔路**：`latermd-ai` 的 `Chunk` 按 ask 固定为 `{ delta, done }` 两字段，但流式请求失败（HTTP 非 2xx / 连接中断 / 读超时）没有天然的信号位；另外 OpenAI adapter 的默认端点、模型名与是否引入额外环境变量，ask 未规定。
- **自动选择**：约定「`done == true` 且 `delta` 非空 = 流失败，`delta` 是面向用户的错误描述（不写入文档）；`done == true` 且 `delta` 为空 = 成功结束」，见 `crates/latermd-ai/src/lib.rs` 的 `Chunk` 文档。默认端点 `https://api.openai.com/v1`、默认模型 `gpt-4o-mini`，可分别用 `LATERMD_AI_BASE_URL`、`LATERMD_AI_MODEL` 覆盖（key 仍只有 `LATERMD_AI_API_KEY`，decisions-pending #3 不变）。
- **如何改**：若希望失败信号更显式（如 `Chunk` 加 `error` 字段或改 enum），改动点集中在 `latermd-ai` 的 `Chunk` 定义与 `openai.rs::run`，消费方尚只有 mock 联调链路，无迁移负担；默认模型/端点改 `OpenAiProvider` 两个 `DEFAULT_*` 常量即可。

## #8 搜索去抖到点发起从 `ui::sidebar` 挪进归约侧（2026-09-25）

- **岔路**：修复「清空搜索输入 / 输入后切走页签后 `debounce_due` 残留过期时刻，`layout.rs` 每帧 `request_repaint_after(ZERO)` 满帧空转」时，评审给了两个薄修：①去掉 `ui::sidebar` 到点判断里的非空输入条件；②`layout.rs` 对已过期的 due 不再要帧。①只修「清空输入」主路径，「输入后切到 Files/Outline 页签」路径 `search_panel` 不渲染、无人清计时，依旧空转；②会打断接力最后一环——到点帧 reduce 先于 ui 执行，reduce 见 remaining==0 不要帧后，同帧 `ui::sidebar` 发出的 `SearchRequested` 滞留 outbox，无下一帧 apply，表现为「输入完不动鼠标搜索永不发起」。
- **自动选择**：把到点判断整体挪进 `layout.rs` 的 `reduce`（每帧必跑、不看页签可见性），到点当帧 `apply(SearchRequested)` → `SearchState::start` 入口清计时，过期 due 活不过一帧；重绘驱动与到点判断同处一处，两类残留（空输入 / 切页签）一并消除。副产品：输入后 300ms 内切走页签也照常发起，切回来即见结果（比「切回 Search 页才自愈」更符合直觉）。
- **如何改**：若更在意「只在 Search 页可见时才发起」，把 reduce 里的到点块移回 `ui::sidebar::search_panel` 并同时采纳②之外的方案（例如 reduce 里对过期 due 保留一次要帧兜底）；三个测试锚点在 `layout.rs`（`search_debounce_fires_in_reduce_even_when_tab_switched_away`、`search_debounce_due_cleared_for_empty_query_without_repaint_loop`）。

## #7 搜索核心不用 `grep-regex` 桥接（2026-09-25）

- **岔路**：roadmap 阶段 3 搜索条目的组件清单写作「`grep_searcher::Searcher` + `regex`」，但 `grep_searcher::Searcher::search_*` 全系 API 要求 `grep_matcher::Matcher` 实参，`regex::Regex` 并未实现该 trait，二者**无法直连**。要么补引 `grep-regex`（ripgrep 官方桥，连带 `grep-matcher`），要么改用同 crate 的 `grep_searcher::LineIter` 做行迭代、`regex` 直接匹配。
- **自动选择**：`LineIter`（`regex::bytes` 变体）+ 直接匹配，P1 搜索核心已按此落地（`crates/latermd-app/src/search.rs`）。不引 `grep-regex` 的理由：它带来的只是 trait 适配与流式读取，而 md 单文件整读进内存完全可行（实现里加了 16MB 单文件上限防病态大文件），大小写开关由 `RegexBuilder::case_insensitive` 一行承接；少一个清单外依赖比「与 ripgrep 同构」更有价值（#4 口径）。
- **如何改**：若 P1 搜索面板需要 multiline 正则或超大文件流式匹配，改引 `grep-regex` + `grep-matcher` 并在 ADR-004 补登，替换 `src/search.rs` 的行循环即可；对外三原语（发起 / 接收 / 取消）与事件模型不变。

## #6 双会话并发冲突（已裁决，2026-09-25）

- **事实**：2026-09-25 上午，本自动循环与另一活跃会话（UI 设计文档线）共享同一工作目录，互相踩踏致第一棒四次卡在 `git checkout main`，被主动停止（stop_reason=model，可恢复，无半截污染）。
- **裁决**：用户选择**选项 1（继续循环）**——已人工解决冲突（`d5de607` 取 stash 侧）、合并 origin/main 进 feature/kun、提交循环文档（`6b2ddc0`，已 cherry-pick 到 main 为 `8a500f0`）、清空 stash。
- **遗留提醒**：若其他会话今后仍需在此工作区工作，建议改用选项 2（`git worktree add ../LaterMD-auto main` + 脚本改造），循环脚本开头的两步 stash 防御只是兜底不是根治。

## #1 打包工作的合并（已解决）

- **原岔路**：打包工作在 `feature/p0-packaging` 分支，自动循环是否代为合并。
- **结果**：另一会话已走正规 PR 流程合并——PR #7（`feature/p0-packaging`：cargo-dist、universal2 dmg、release workflow、cask 模板）与 PR #8（`feature/m0-perf-bench`：长文档 bench）均已合入 main（`f822b61`）。剩余真机验收（打 tag 看 Release、brew 装机）仍属人工。

## #2 直推 main 豁免（2026-09-25 中午起已被现实推翻，改为 PR 自合并通道）

- **原自动选择**：循环直推 origin/main（用户晨间指令）。
- **新事实**：远端 main 已开启分支保护（require PR，GH013 拒绝直推）。
- **现行流程（PR #13 验证可行）**：每棒完成后 `git push origin HEAD:refs/heads/auto/<功能名>` → `gh pr create --head auto/<功能名> --base main` → `gh pr merge <N> --merge --delete-branch` → 本地 `git checkout main && git pull`。满足保护规则且无需人工；若仓库后续加 required review 导致自合并失败，则退化为「推分支 + 提示人工合并」。
- **注意**：rebase 远端新提交时文档冲突（roadmap/README 修订表）按「两边行都保留」合并。

## #3 AI provider 的 API key

- **岔路**：P1 AI 功能（队列 #3-#5）需要真实模型端点与 key 才能端到端。
- **自动选择**：以 MockProvider（100ms/chunk 流式）交付全部链路与 UI；provider trait 预留 OpenAI/Anthropic/Ollama adapter，key 从 `LATERMD_AI_API_KEY` 或凭据管理读取，**代码不硬编码任何 key**。
- **如何改**：设置界面填 key（或 `export LATERMD_AI_API_KEY=...`），选 provider 后重启即用真实模型。

## #4 依赖新增的登记口径

- **岔路**：循环会给仓库引入 ADR-004 清单外依赖（grep-searcher、dark-light、keyring 等）。
- **自动选择**：每引入一个，同步在 `docs/adr-004-technical-stack.md` 表内登记（版本+用途+notes），视作 ADR 滚动修订。
- **如何改**：若某个依赖不认可，revert 对应 commit，登记行随代码一起回退。

## #5 打包分支 WIP 的贮藏（现状已简化）

- **岔路**：切换 main 时工作区残留未提交改动，阻塞 checkout。
- **自动选择**：stash 无损贮藏，不丢、不代提交。
- **现状**：打包工作已随 PR #7 合入 main；工作区现存 README.md / docs/README.md / docs/roadmap.md 修改与 docs/distribution.md 属 UI 设计线会话恢复的 WIP，归它处置，本循环不再 stash（见 #6）。stash 栈若仍有 `auto-cycle:` 条目，恢复前先 `git stash show -p` 与 main 对比，无增量直接 drop。

## #30 外壳重构 D1–D5 拍板（2026-09-26，依据用户指令自动拍板）

- **岔路**：ui-shell-redesign.md 的五个决策点（D1 自绘无边框标题栏 / D2 菜单栏独立行 / D3 单栏三段 / D4 禅定保留标题栏退出入口 / D5 可视化编辑只预留接口）原计划等坤哥逐条拍板。
- **拍板依据**：用户 2026-09-26 指令「看下现在还有什么未完成的任务，加入流水线」——即放行 #13 进入自动循环；按循环授权（自动选最优解不等人），D1–D5 全部取规格中已论证的**默认选择**。
- **风险兜底**：D1 无边框的三平台拖拽/缩放风险保留 `LATERMD_NATIVE_DECORATIONS=1` 逃生口回落原生装饰（与 `LATERMD_RENDERER=glow` 同构）；M5 收口棒带明暗像素采样验收，Win/mac 真机复测留在人工清单。
- **如何改**：任一决策想推翻，改 ui-shell-redesign.md 对应 §（D1 见 §3、D2 见 §2、D3 见 §5、D4 见 §7、D5 见 §8），在对应里程碑棒完成前修订代价最低；已完成后再改 D1 需同时保留逃生口路径。

## #62 代码块复制按钮(#38)乙案接管通道不可行的探测取证:块体复用三件套均 pub(crate)(2026-09-30 自动选择)

- **岔路**：auto-plan #38 的乙案要求「`is_block_code_widget` 接管全部代码块,块体复用 vendored 高亮/横向滚动」——接管意味着 app 侧自绘块体,复用是否成立决定乙/甲走向。
- **取证(探测棒实读)**：块体复用所需的 `scrolling_code_galley` / `StreamingCodeCache` / `theme_identity_ptr` 在 vendor/egui_markdown/src/layout.rs 尾部(`pub(crate) use syntect_code::{scrolling_code_galley, theme_identity_ptr, StreamingCodeCache}`)均为 **`pub(crate)`,app 侧不可见**;公开的 `highlight_code` 只有裸 LayoutJob,无横向滚动包装与流式缓存。乙案要走通必须先做 vendor ①类改 pub——比直接消费既有挂载点还重。
- **自动选择**：**零 vendor(与 #61① 同结论,独立取证)**——消费上游既有 `MarkdownLabel::code_block_buttons` 挂载点;源文本由 vendored 回调直接透传 `Token::CodeBlock.text`(与 ```ai 指令卡同法)。另记录任务口径出入:auto-plan #38 写「经 code_block_spans 偏移切 rendered」,但 ```ai 先例的实际机制是 vendored 回调透传文本(label.rs `block_code_widget(ui, text, …)`),不经偏移换算;按「以现状为准」取后者。
- **如何改**：若将来确需乙案接管(如块体上方独立行),在 vendor ①类里把上述三件套提为 `pub`(上游可合的通用能力)并登记 vendor/README.md 变更表;落地口径见 #61。

## #66 查找条 Esc 关闭在 egui 0.36 下是哑弹的修复口径(#17 M1,2026-10-01 自动选择)

- **岔路**:#17 M1 规格「Esc 整条关闭沿用现状」,但无头实证发现已合入的查找浮层里 Esc 关闭**本来就是坏的**——egui 0.36 的 `Focus::begin_pass`(egui/src/memory/mod.rs:595-598)把裸 Esc 当「交出焦点」在帧首清焦,而 `find_bar_contents` 的 Esc 检测以 `response.has_focus()` 为前提,永不触发(探针:聚焦查找框发 Esc,outbox 空、`find_open` 保持 true、焦点被 egui 拿走;只有 ✕ 按钮真能关)。M1 红线「不得重做已合入查找浮层、发现缺陷如实交人工不顺手大改」与「Esc 关整条」规格在此冲突:沿用现状=交付一个哑弹键位。
- **备选**:①只修替换行、查找框留缺陷——两框 Esc 行为不一致(替换框行、查找框不行),比统一坏更怪;②两框都修(各加一行 builder 参数);③都不修,Esc 全部交人工。
- **自动选择**:②——给查找/替换两个输入框 TextEdit 都加 `egui::EventFilter{ escape: true, .. }`(TextEdit 出厂默认 arrows 锁框内、escape 不锁),egui 官方为「Esc 应作用于控件而非交焦」提供的出口(builder.rs:351 注释原文举的例子就是 completion popup)。改动是两行 builder 参数,查找行的结构/逻辑零变化,不构成「重做」;修复后无头回归 `find_row_escape_closes_the_bar` / `replace_row_escape_closes_the_bar` 钉住两框行为。
- **理由**:tooltip 与验收口径(acceptance-checklist §9 语境)都承诺 Esc 关闭;规格字面「沿用现状」的意图是保持行为,不是保持缺陷;两框必须同口径,否则用户按 Esc 结果取决于焦点在哪个框。
- **如何改**:若认为这越过了「不顺手大改」边界,revert `ui/layout.rs` 中 `FIND_BAR_EVENT_FILTER` 常量及两处 `.event_filter(...)` 调用即可(查找条回到哑弹 Esc、替换行 Esc 检测保留但同样不触发),两个 Esc 回归测试需同步删。

## #69 主题默认键改排 Alt+T(#45 K1)旧 keymap.json 的迁移口径:值感知迁移「旧默认值→新默认值」,用户自定义/主动清除不动;附带 mac 显示文本 ⌥ 口径(2026-10-01,#45 tab-restore-keymap K1·自动拍板)

- **岔路一(迁移口径)**:任务书要求「旧 keymap.json 已有用户自定义 Theme 绑定的,加载时不覆盖(增量迁移语义,与 #17 同口径)」,但 #17 的字面机制(只给**缺失** command id 补默认、既有条目一律不动)与本任务目标「把 Cmd/Ctrl+Shift+T 空出来给 TabRestore」对存量用户冲突——历史上每次新增命令,`load_from` 的增量写回都会把**当时的全表默认值**落盘,老用户的 keymap.json 里几乎都躺着 `"toggle_theme": "Ctrl+Shift+T"`(当年出厂值,非用户手笔)。若严格不动它:老用户主题继续占 Cmd/Ctrl+Shift+T,K2 的 TabRestore 增量补进去后同键,而消费顺序(`poll_shortcuts` 修饰键位数降序 + `ALL` 序稳定排序)在前的 ToggleTheme 每次抢先吞键——TabRestore 对全部存量用户永久哑键,「空出来」只对全新安装生效。
- **备选**:①严格 #17 字面口径,既有条目一律不动(代价:存量用户 TabRestore 哑键);②**值感知迁移**——仅当 toggle_theme 条目**解析值**== 旧默认(Cmd/Ctrl+Shift+T)时改写为新默认 Alt+T,其他值(用户自定义、主动清除的空串、解析不了的坏行)分毫不动;③启动时整表 reset_all 到新默认(覆盖一切用户自定义,最粗暴)。
- **自动选择**:②——`load_from` 在增量补默认前先做一次「退役默认值」比对迁移(用 `parse_shortcut` 比**解析值**而非原文,`cmd+shift+t` 等别名/大小写手改档同样识别),迁移发生即走既有 `changed` 通路写回 keymap.json(下次启动不重复迁移)。任务书的保护条款「用户自定义 Theme 绑定不覆盖」在 ② 下字面与意图都成立。
- **理由**:默认键变更的产品语义是「没自定义过的用户跟新默认走」,只有显式偏离应被保留;① 会让 K2 的核心验收(Cmd/Ctrl+Shift+T 恢复标签)对存量用户静默失效,比迁移更伤用户;③ 踩「不覆盖用户绑定」红线。已知局限:曾在旧版**显式**把主题改回旧默认的用户与「从未动过」不可区分,会被一并迁到 Alt+T——与「恢复出厂即新默认」的既有语义一致,接受。
- **如何改**:想回 ①,删 `keymap.rs` `load_from` 中 `retired_theme_default` 迁移块及 `theme_default_migration_on_load` 测试(需接受存量用户 TabRestore 哑键或手动改绑);想区分「显式选过旧默认」的用户,需引入「用户改过键」持久标记,超出本轮范围。
- **岔路二(附带口径,mac 显示文本)**:任务书要求「Win=Alt+T / mac=⌥T 三平台口径」。菜单栏走 egui `format_shortcut`,mac 上本就显示 ⌥T(符号集,`can_show_modifier_symbols` 自带字体回退);但设置「快捷键」页/工具条 tooltip 走自家的 `Shortcut::platform_text`,原先 mac 上也写死 "Alt+"。**自动选择**:`platform_text` 在 mac 输出 `⌥+`(保持与既有 `Cmd+` 同款「词+加号」形态,菜单 ⌥T 与设置页 ⌥+T 的差异同今天 ⌘S/Cmd+S 的差异,口径一致),`parse_shortcut` 增加 `⌥` 别名(与 alt/option 同义)保证存档往返与 mac 手改档可解析。副作用:mac 上含 Alt 组合的存档文本随之改变(ToggleRightPreview 由 `Cmd+Alt+R` 变 `Cmd+⌥+R`),旧档 `alt` 别名保留仍可解析,不受影响。**如何改**:不想要该口径,revert `platform_text` 的 mac 分支与 `parse_shortcut` 的 `⌥` 别名即可(mac 显示回 "Alt+")。

## #70 TabRestore(#45 K2)重开失败条目的去留:失败即丢弃并继续下一条,不保留在栈顶(2026-10-01,#45 tab-restore-keymap K2·自动拍板)

- **岔路**:任务书明确把「重开失败(文件被删/移动)时该栈条目是否出栈」列为二选一(①失败即丢弃并继续下一条 / ②失败保留在栈里),要求取舍登记 decisions-pending 并用单测钉住。失败时提示行已按既有口径写「打开失败 <路径>」(`file::read` 的 FileError 文案),分歧只在栈条目去留。
- **备选**:①丢弃并继续——失败条目当场出栈,同一次触发接着尝试下一条,直到某条成功或栈空;②保留——条目留在栈顶,提示行说明后本次触发结束,下一条要再按一次才摸得到。
- **自动选择**:①。`State::restore_tab` 的循环按「pop → open_path → 成功即停 / 失败继续 pop」实现,死条目绝不留在栈里。
- **理由**:本产品心智是「重开=重新读盘」(preview-typography §3.3),文件被删/移走后该条目在**本会话内**永远无法恢复成功;若保留(②),它永远堵在栈顶,此后每次 Cmd/Ctrl+Shift+T 都先撞一次失败才能摸到下一条,任务书验收「连续按 Cmd/Ctrl+Shift+T 能逐条回走完整条栈」被死条目打断;用户已从提示行得知哪个文件打不开,信息不丢。②的辩护场景是「网络盘/外接盘暂时离线、之后又可用」,但该场景下用户重按一次快捷键即可,代价远小于①堵栈的代价;且不做跨会话持久化(已定案),「暂时不可用跨会话找回」本就不成立。
- **如何改**:想改成②(失败保留),把 `state.rs` `restore_tab` 的 `while let Some(path) = self.tabs.recently_closed.pop()` 改为先 `last().cloned()` 试开、失败即 `return`(不出栈),并同步改 `tab_restore_failed_path_noticed_dropped_and_continues` 测试的断言(死条目留栈、alive 不在同一次触发里恢复)。

## #71 文件树带 Git 角标行改「整行宽+整行可点」(#40 filetree-truncate,2026-10-02·自动拍板)

- **岔路**:角标要「贴行尾」且不被省略号挤掉,egui 0.36 的标准做法是 `Button::right_text`(内部 `Atom::grow`)。grow 原子在空间富余时把块撑满可用宽,于是带角标的行从旧 `selectable_label` 的「内容宽」变成「整行宽」,选中/hover 底色与命中区域随行铺满——这是用户可见的行为变化,不是纯内部改写。
- **备选**:①`Button::selectable(selected, name).truncate().right_text(角标)`——角标真·贴行尾(宽栏窄栏都成立),带角标行整行宽/整行可点,无角标行维持内容宽;②手绘整行(仿 `nav_row` 的 `allocate_exact_size` + `Sense::click`)再自绘名字 galley 与角标——行宽口径全部自控,但要复刻 `Button::selectable` 的选中/hover 样式与无障碍信息,表面积大。
- **自动选择**:①。
- **理由**:①复用 `selectable_label` 的同一条渲染路径(后者在 egui 0.36 就是 `Button::selectable(..).ui(..)` 的别名),选中/悬停样式零漂移;「角标贴行尾」由布局原子保证,名字的截断宽度先扣除角标固有宽,省略号永远挤不掉角标;整行可点/整行高亮是树形列表的通行预期(VSCode 同款),把旧行为里「点名字才算点行」的偏差顺带修掉。git 页改动行(`git_status_row` 同走 `badged_row_label`)一并受益。②为了行宽口径自控引入约 60 行手绘样式代码,违背小表面积。
- **如何改**:想回「内容宽」,把 `badged_row_label` 里 `right_text` 的 grow 语义换掉——不用 `Button::right_text`,改在 `ui.horizontal` 里先 `add(角标 Label)` 再倒排名字,或走②手绘;同时改 `tree_row_truncates_long_names_in_narrow_panel` 里「角标贴行尾(距行右沿 < 12px)」的断言。角标与名字的间距现在是 2×`icon_spacing`(出厂 8px,grow 原子两侧各一个 gap),嫌宽可给 `Button::gap(…)` 传更小值。

## #72 #43 M2 混排基线修复只作用于预览族,编辑器/全局 UI 的同根因基线偏差不在本轮修(2026-10-02,#43 preview-font-metrics M2·自动拍板)

- **岔路**:M1 取证确认混排基线偏差的根因是「链头 Inter 与 CJK 回退 face 的行 metrics 表值差」(残差 ≈0.072em,与任何 line_height 设置无关),修法是把链头 override 成 CJK 同款。但 override 挂到哪个族有两条路:①只挂预览专用族(`Inter-Preview` 正文族 + `bold` 别名族链头,本轮实现)——Proportional/SemiBold/Medium 原生族不动,UI 外壳与编辑器的行高维持现状;②把 Proportional 链头直接换成 override 副本——编辑器(TextEdit)与全部 UI 控件的中英混排基线一并对齐,但**所有 UI 文本行高从 Inter 原生 1.21em 涨到 CJK 同款 ≈1.448em(+19.6%)**,菜单/按钮/标签全套界面密度变松。
- **备选**:①预览族隔离(本轮);②Proportional 全局 override(界面密度全局变化);③编辑器单独再开一个 TextEdit 专用族(编辑器基线对齐且 UI 不动,但编辑器与预览行距不同源,且 TextEdit 的字体配置面要单独拉一遍)。
- **自动选择**:①。#43 条目的症状与验收全部落在预览排版;UI 密度是全局产品决策(+19.6% 行高影响每一屏),单方面改掉超出本任务边界,也不是「预览字体修复」的题中之义。
- **理由**:预览与 UI 的排版需求本就不同源(预览是长文阅读排版,CJK 行距下限 1.448em+ 合理;UI 是控件密度,Inter 原生行高是刻意保留);①把修复面精确对准症状面,降级路径(无 CJK 候选/表值解析失败)全部回落现状行为。编辑器混排基线偏差(同根因,TextEdit 走 Proportional)如实保留,现状与修复前一致——不劣化,只是没顺手修。
- **如何改**:若拍板要全局对齐,把 `fonts.rs` `build_definitions` 里 Proportional 链头从 `NAME_REGULAR` 换成 `PREVIEW_REGULAR`(SemiBold/Medium 同理),`preview_body_family` 即可退役回落 Proportional;UI 行高变化的观感需真机人工复核后定。编辑器单独修则按 ① 的同款手法注册 TextEdit 专用族,由 #23(字号行距设置)一并考虑。附带说明:代码块(Monospace 族)的中文注释行高/基线同样未修(代码块 format 的 line_height=None 走链头行高,不经过 vendored floor;链头是 epui 内置等宽字体,与 CJK Mono 的表值差同根因),一并留给 #23 或后续。
- **2026-10-02 坤哥反馈拍板**:编辑器混排基线修理由 #50 M1 落地(`97a701e`)——坤哥 2026-10-02 反馈截图「源码页面字体大小不一,高低不一致」即本条登记的同根因症状在源码页的呈现;采用本条「如何改」预留的「编辑器单独注册 TextEdit 专用族」路径(`fonts.rs` 新增 `editor-mono` 族,经 `TextStyle::Monospace` 档投影统一源码 TextEdit/行号槽/Live 活动块),实测混排基线偏差 +1~+3px→+0.000px(12-24pt 全档×明暗两主题)。实现与任务书有一处方向偏差:反向 override(CJK 等宽 face 副本对齐链头 Hack 表值、链头不动,保「纯 ASCII 行盒不变」否决线),全文见 #75;上文附带说明的代码块面因 vendored 硬编码 `FontId::monospace` 未能一并换族,保留现状,全文见 #76;预览侧与 vendored 均零改动。

## #73 #23 F3 行距滑杆只作用于预览正文,源码编辑器的行距(行盒倍率)不在本轮投影(2026-10-02,#23 font-prefs F3·自动拍板)

- **岔路**:任务书 F3 目标句写「字号/行距真正作用于编辑器与预览正文」,但子任务清单的实现路径是三条互斥分工——①字号→编辑器(`text_styles` Monospace 档投影);②字号→预览(显式 `FontId`);③行距→`markdown_style()` 的 `line_height_ratio` 覆盖(vendored,即预览)。行距在清单里只出现一次且落点在预览:源码模式 TextEdit/行号槽/Live 活动块的行距没有任何投影项,编辑器行盒保持「等宽族链头自然行高 + `spacing.extra_text_line_spacing`(出厂 0)」。auto-plan #23 规格语言同样宽泛(「作用于编辑器与预览正文」),两处文本与子任务清单的实现边界不一致。
- **备选**:①行距只进预览(F3 实施)——编辑器行盒维持现状,源码模式拖「行距」滑杆无感;②编辑器同步投影行距——egui TextEdit 无行距倍率字段,只能把 `style.spacing.extra_text_line_spacing` 设为绝对像素加值:`clamp(0, size × ratio − ctx.fonts().row_height(Monospace 档@size))`,每帧按运行时字体 metrics 换算,且调低(如 1.2)时要 clamp 到 0 兜底自然行高(CJK 行高物理下限,与预览侧 `min_line_height_em` 同款语义)。
- **自动选择**:①。
- **理由**:子任务清单是实现权威(行距若要双投影,会像字号一样写「→编辑器」「→预览」两条);②把行距语义(字号倍率)翻译成绝对像素加值需要依赖运行时 `Fonts::row_height`(字体链与 CJK face 相关,与 `apply` 的 staleness 投影模型耦合更深——size 与 ratio 任一变化都要重算),是一块独立改动,不该以「顺手可做」混进本棒;且 #72 已定源码编辑器的混排基线/行高本就未修(TextEdit 行 metrics 取等宽族链头出厂值),单独拉行距投影会制造「行距对齐了、基线没对齐」的半吊子状态,不如与 #72 的「如何改」里预留的 TextEdit 专用族排版面一起做。写作面(编辑器)行距的体验感知也弱于阅读面(预览)。
- **如何改**:若拍板编辑器也要行距,在 `theme.rs::ThemeSettings::apply` 里追加 `extra_text_line_spacing` 投影(值 = `(size × ratio − row_height).max(0)`,row_height 用 `ctx.fonts(|f| f.row_height(&FontId::monospace(size)))` 现算,与 `apply_font_size` 共用 staleness 槽),并确认 `ui/editor.rs:167` 的行高估算公式(`row_height + extra_text_line_spacing`,既有已含 extra,无需改)、行号槽 `gutter.rs` 与 `live.rs` 活动块的 desired_rows 同步;补「行距调低不裁切 CJK」的边界测试(clamp ≥ 自然行高)。建议与 #72 的编辑器行 metrics 对齐(TextEdit 专用族/override 副本)同棒做,排版面一次对齐。
- **2026-10-02 坤哥反馈拍板**:岔路转正——坤哥 2026-10-02 反馈「源码页面…每行间距也太小感觉」,原自动拍板①(行距只进预览,理由之一是「写作面行距的体验感知弱于阅读面」)被真机体验推翻,按本条「如何改」预留的方案②落地(#50 M2,`0a7f4be`):`theme.rs::apply_font_size` 扩为字号+行距联合投影,`extra_text_line_spacing = max(0, 字号×行距−自然行高)`(自然行高经 `ctx.fonts` 现算投影后 Monospace 档的 FontId,与 F3 共用按值键控 staleness 槽,size/ratio 任一变化当帧重投影),行盒精确=字号×行距(15pt 实测 1.2 档 18.000px/2.0 档 30.000px),clamp≥0 兜底自然行高(「行距调低不裁切 CJK」边界测试已钉);「与 #72 同棒做、排版面一次对齐」的建议同棒兑现——M1 的 editor-mono 族(混排基线 +0.000px)与 M2 行距投影同在 feature/editor-typography 先后落地,不出现「行距对齐了、基线没对齐」的半吊子状态。

## #74 #23 F4 标题呼吸间距的实现落点:任务书指定 `render_token_range` 层 add_space,实际落在 `build_layout` 层透明 spacer 行(2026-10-02,#23 font-prefs F4·自动拍板)

- **岔路**:任务书(#23 F4)与 preview-typography §2.2 均指定「改 `render_token_range`:token 为标题时 `add_space(block_spacing + heading_space_above)`」。但仓库现状里该函数只服务**分段渲染路径**(文档含表格/图片/引用/block widget/scroll 代码块时 `needs_segmentation` 才为 true,`label.rs` 走 `render_segmented`);纯正文+普通代码块文档——LaterMD 预览最常见的形态——走整篇 galley 路径(`render_galley`),**根本不经过 `render_token_range`**,标题间距完全不生效。且即便在分段路径上生效,`add_space(12px)` 是替换空行(标题前换行 token 被 `flush_text_range` trim 掉)而非叠加,12px 反而**小于**正文段落间距(空行行盒 ≈16.9px@13pt×1.3,app 侧 ratio=1.5 时 ≈19.5px),违背计划文档 §2.3 验收③「标题上下呼吸明显大于行间距」。
- **备选**:①`build_layout` 层在 heading 块首 token(前一 token 是 Newline,天然排除 heading 行内 bold/链接切片)前 append 透明 spacer 行(行高 `block_spacing + heading_space_above`),叠在既有空行之上的额外呼吸——两条渲染路径都调 `build_layout`,自动一致,`render_token_range` 的 `before_block`/`after_block` 零改动;②任务书原口径 `render_token_range` 层 add_space,但要覆盖纯文本文档就必须同时把 heading 拉成 segment break——缓存键、块高缓存、视口剔除语义全变,渲染成本与上游可合性双输;③①②并行——分段路径下 spacer 行与 render 层 add_space 双计,直接排除。
- **自动选择**:①。
- **理由**:产品目标(标题上下有明显大于段落间距的呼吸)与两条硬约束(正文观感不变、两条渲染路径行为一致)都指向同一落点。①的数值语义是「叠加」:标题上方总呼吸 ≈ 空行(16.9px)+ spacer(8+4=12px)≈ 28.9px,明显大于段落间距且可通过一个字段全局调;机制先例是同文件 `HorizontalRule` 的透明 spacer 行做法,新增面极小。触发条件保守:文档/flush 段首标题与紧邻块元素的标题(after_block 已 trim 掉空行,heading 是段内首 token)不插 spacer,顶部不撑空、块间距一视同仁;字段归零时 spacer 行高恰等于 `block_spacing`,语义完全回落到现状。无头实证(egui 0.36.2):spacer 行高三档(8/12/48px)精确等于 `block_spacing + heading_space_above`,总高度随标题数×Δ 精确线性,纯正文文档 rows 数与高度对该字段**完全不变**;「纯正文零影响」已钉成测试(否决线)。
- **如何改**:若想要「12px 纯间距」的替代式观感,把 `layout.rs` heading 分支里 spacer 行高从 `block_spacing + heading_space_above` 改为只 `heading_space_above`(触发条件不变);若想要块元素(表格等)后紧跟的标题也吃呼吸,在 `render_token_range` 的 `after_block` 跳完换行后、下一 token 是 heading 时补一段 `add_space(heading_space_above)`(flush 段首无 spacer,不会双计),该情形当前保持普通 `block_spacing`,已有测试钉死该保守语义。出厂 `line_height_ratio=1.30` 本轮未动;真机目视后微调只需改 `heading_space_above` 一个出厂值(app 侧接线见 F5,皮肤 ron 兼容:已存在的预设文件不含新字段走 serde default 4.0 兜底,显式 0.0 是合法偏好会原样保留)。

## #75 #50 M1 编辑器等宽族的 override 方向:任务书指定「链头 override 对齐 CJK 表值」,实际反向「CJK 副本 override 对齐链头」(2026-10-02,#50 editor-typography M1·自动拍板)

- **岔路**:任务书 M1 写「链头=当前编辑器等宽拉丁 face 的副本(行 metrics override/FontTweak 对齐本机 CJK face 表值,同 #43 M2 手法)」——把链头(Hack,行高 1.164em)的表值改写为 CJK 等宽 face 同款(1.448em);但同一任务书的否决线写「纯 ASCII 文档编辑器 rows 数与行盒高度对新族完全不变」。TextEdit 行盒高(desired_rows 的输入)直接由链头行 metrics 决定,链头被 override 则纯 ASCII 行盒必然从 1.164em 涨到 1.448em(15pt 实测 17px→21.7px,+24%),两条指令不可同时满足。
- **备选**:①照任务书字面,链头副本 override 到 CJK 表值——基线归零但纯 ASCII 行盒 +24%,否决线破;②链头不动,在 CJK face 上用 `FontTweak::y_offset_factor` 平移墨迹——layout 基线仍差(y_offset 只移墨迹不移布局基线),且偏差随字号在 +1~+3px 间变化(本机实测 12-24pt),单个 em 比例因子无法在整数字号档全域精确归零;③**反向 override**:CJK 等宽 face **副本**的表值改写为链头 Hack 同款 em 值(#43 M2 同一套 `override_vertical_metrics` 机制、方向相反),链头分毫不动。
- **自动选择**:③。副本只挂 `editor-mono` 专用族(`fonts::FAMILY_EDITOR_MONO`)并替换链尾原生 CJK 等宽条目(链内混入原生 face 会先命中、override 白做,与 #43 M2 预览族同一纪律);字号仍走 #23 F3 的 `TextStyle::Monospace` 档投影,族由 `apply_font_size` 一并投影(`fonts::editor_mono_family`,未注册回落 `Monospace`)。
- **理由**:否决线的本意是把「行距太小」的改动留给 #50 M2(行距滑杆投影 `extra_text_line_spacing`),M1 只对齐基线;③使链头与 fallback 行 metrics 全等,基线公式(`ascent + valign·(行盒−line_height) + 0.5×(链头行高−face行高)`)残差与 #43 M2 同理精确归零,且 advance/outline/win 表不动 → 纯 ASCII 的行盒/折行/行数逐像素不变。无头实证(egui 0.36.2,本机 Noto CJK face 7):混排基线偏差旧族 +1~+3px → 新族在 12-24pt 全档 × 明暗两主题下 +0.000px,纯 ASCII 文档 rows/逐行行盒/总高与旧族全等(`editor_typography_acceptance` 三测试);`#43 M2 手法`的机制一致性与否决线在此不冲突——冲突的只是改写方向。CJK 与拉丁的墨迹高比(1.25@15pt,face 设计值)未动:任务书第 3 条的「视觉尺寸均衡」经基线对齐后已达「同行无高低差」,进一步缩放 CJK 观感(如 `FontTweak::scale`)需连动 override 目标值(否则破坏基线恒等),留真机目视后再定。
- **如何改**:若拍板要 #43 M2 原方向(链头对齐 CJK、行高随 CJK 涨),把 `fonts.rs` `build_definitions` 编辑器族分支的目标/源头对调(patch "Hack" 字节到 CJK mono face 表值作链头副本),并重锚 `install_registers_editor_mono_family_with_aligned_baseline` 与 `editor_typography_acceptance` 的行盒不变断言(纯 ASCII 行盒将 +24%,需与 #50 M2 的 `extra_text_line_spacing` 语义对表,避免行距双重放大)。

## #76 #50 M1 的「源码高亮代码块」等宽面未能指向新族:vendored 硬编码 `FontId::monospace` 且预览代码块共用同一族,app 侧单独换族不可达(2026-10-02,#50 editor-typography M1·自动拍板)

- **岔路**:任务书第 2 条要求编辑器全部等宽渲染面(源码 TextEdit、行号槽、Live 活动块、**源码高亮代码块**)统一指向新族。前三者都经 `TextStyle::Monospace` 档,族投影一处生效;第四个(带 syntect 高亮的代码块 widget)由 vendored 层渲染,`vendor/egui_markdown/src/layout.rs`(626/643/732/969 等处)硬编码 `FontId::monospace(code_font_size)` = `FontFamily::Monospace` 族,**且预览面板的代码块走同一硬编码路径**——app 侧没有任何可单独给「编辑器里的代码块」换族的扩展点。任务书同时规定本棒「全程不触碰 vendor/egui_markdown/」与否决线「预览侧零改动」。
- **备选**:①给 vendored `MarkdownStyle` 加 code 族字段或在 layout 读 context 标记(①类 vendored 补丁)——触碰 vendor 红线;②直接把 `FontFamily::Monospace` 的链头换成 override 副本——预览代码块纯 ASCII 行盒同步 +24%、含 CJK 注释的代码渲染变化,否决线二破;③不换,现状保留。
- **自动选择**:③。
- **理由**:①违反本棒「不触碰 vendor」的显式约束;②把编辑器修复外溢到预览(预览行距/行盒是 #23/#50 M2 的领地,代码块拉丁行从 17px 涨 21.7px 是可见回归);③的代价可控——代码块拉丁字形与新族链头同为内置 Hack face,纯拉丁代码渲染逐像素一致,唯一保留的现状缺陷是「代码块内 CJK 注释与拉丁的基线差」(同根因,#72 已登记),不影响坤哥反馈的源码 TextEdit 主症状。
- **如何改**:若拍板代码块也要基线对齐,走①类 vendored 最小补丁(`MarkdownStyle` 增 `code_font_family: Option<String>` 之类字段,layout 的 `FontId::monospace` 处改读字段、None 回落现行为,独立 `vendor:` commit + vendor/README 登记),app 侧把编辑器语境的 `MarkdownStyle` 填 `editor-mono`、预览填 `Inter-Preview` 或 None;或拍板全局换 `FontFamily::Monospace` 链头(一行改动,预览代码块与全部 UI 等宽文本随之变化,需真机目视行盒 +24% 的观感)。

## #77 流式 O(n) 的主导修复(布局缓存失效粒度块级化)确需 vendor ①类改动,#46 R2 红线内不可修,移交人工拍板(2026-10-02,#46 streaming-perf R2·待人工拍板)

- **岔路**:R1 复测判定流式追加仍 O(n)(10000 行档 1001.7 ms,较 M0 +26.9%,[perf-recheck-2026-10.md](perf-recheck-2026-10.md) §2/§5),R2 修复路径激活;但 R1 交接的修复对象(疑点 A:布局缓存失效粒度从「整篇 text_hash」降到「块/段」)机制位置**全部在 vendored 层**——顶层 `CachedMarkdownLayout` 以整篇文本哈希为门控(`vendor/egui_markdown/src/label.rs:57-65` `hash_text`、`:636` 命中判定),miss 即 `parser::parse` 全文解析 + `tokens_to_owned` 全量深拷贝(`label.rs:663-665`),纯代码块等 `needs_segmentation=false` 文档更是整篇 `build_layout` 单 galley(`label.rs:669-691`、`layout.rs:262-275`),块级剔除缓存的 key 同样是整篇哈希(`label.rs:137-151` `try_cull_block`、`:772/:808/:837` 调用点)——改任何一处都动 `vendor/egui_markdown/src/label.rs`。#46 R2 模块约束「不触碰 vendor/,修复确需 vendor 时把需求记 decisions-pending 交人工拍板,本模块按 app 侧可达近似或如实报未修」。
- **备选**:①**vendor ①类补丁(两步)**:第一步放宽 `needs_segmentation` 准入(按 token 数/字节数或代码块行数阈值,阈值挂 `MarkdownStyle` 可配、默认保守)让长文档进分段路径;第二步把块级缓存 key(`try_cull_block`/`cache_block_height` 的 `text_hash`)换成「块内容 hash」(token 在手逐块可算,追加尾行只失效尾块);flush 段缓存已是 per-range(`label.rs:975-976` `hash_flush_context` 按 token 切片)可直接受益;整篇 galley 路径的全文 parse+深拷贝占比 ~0.13%(R1 实测外推)可接受不动。②**app 侧近似**:逐一核对后**不存在**——heal 已条件化(#39,`ui/layout.rs:301-310` 三栏与 `:671-674` Live 两处仅 AI 流式写入本标签帧开)、`LinkHandler::id()` 用默认 0 稳定(`app ui/preview.rs` 未 override,`vendor link.rs:92-94`)、widget id 只含 tab id(`ui/preview.rs:539-541`)、修订号纪律在位(`ui/editor.rs:295-299` 仅 rev 前进重建快照);开 `scroll_code_blocks(true)` 强制分段**不构成修复**——块级 cull key 仍是整篇 text_hash(追加仍全块失效),且代码块渲染形态改滚动窗格是观感回归。③**产品侧降级(app 可达,属行为变化)**:AI 流式写入时预览降级纯文本排版,或对流式文档设规模上限,`m0-report` §4.2 曾列的备选方向。
- **自动选择**:本模块不动任何代码(红线);修复需求按①+③登记移交人工拍板;#46 R3 复测按「未修」口径落档(水位即 R1 水位),M0 验证 4 挂账不销。
- **理由**:主导成本(10000 行档 ~99%,全文解析仅 ~0.13%)在 vendored 整篇 galley `build_layout` 与整篇哈希门控;app 侧四候选方向(流式帧免全文重解析/heal 路径/布局缓存命中/无关帧重建)逐项实读核对均「已在位」或「不可达」,任何 app 侧修补都不改变 O(n) 判定曲线;擅动 `scroll_code_blocks` 有观感回归且被 cull key 机制证明无效,属投机优化不做。
- **如何改**:拍板①时——按「先分段准入、再块级 key」两步走 vendor ①类最小补丁(独立 `vendor:` commit + vendor/README.md 变更表 ①类登记 + `vendor/egui_markdown/check.sh` 六项;①类按上游 CONTRIBUTING 标准保持可 cherry-pick),注意两处连带语义:`debug_assert!(layout.segment_breaks.is_empty())`(`label.rs:692`)的两侧一致性约束、以及块序号作 id 成分时编辑中部不得挪移后续块序号(AGENTS §6.7 widget id 纪律);完成后按 perf-recheck §2 同一条命令复测,验收口径 = 每行成本随规模趋稳(亚线性),M0 挂账凭新数字销账。拍板③时——改动局限 crates/(流式路径按文档规模切渲染配置或关流式预览),一个 PR 可完成,但流式写作的产品体验降级需坤哥先认。

> **已拍板执行(2026-10-04 授权,扩队 #52)——已按①落地并销账**:#77 修复路径①两步(M1 分段准入放宽 + M2 块级缓存 key 换块内容 hash,`be05b52`/`ccc2945`)+ M3 复测追加修复两处(anchors 记录二次方 → 单遍扫描;flush 命中帧 O(n) job 拷贝+哈希 → shaped galley 缓存)全部落地;验收按 perf-recheck §2 同命令复测:每行成本 0.0718 → 0.0536 → 0.0551 µs(下降后趋稳),10000 行档 550.75 µs 对 R1 1001.7 ms −99.945%,M0 验证 4 已销(m0-report §4.4)。数字与逐字输出见 [perf-recheck-2026-10.md](perf-recheck-2026-10.md) §9。本注记不改动上方原条目,翻案仍按其「如何改」。
>
> **落库时序勘误(2026-10-05 独立评审补记)**:本注记与 m0-report §4.4 落档的 commit `3f19c2e` 虽带 `vendor:` 前缀,但**只含 docs 五件、不含任何 M3 代码**;销账时点 M3 的 vendor 改动(label.rs / layout.rs / tests/cache.rs / tests/section_anchors.rs / CHANGELOG / vendor README,共 +401/−50)系**未提交工作区**——纯 HEAD(3f19c2e)检出实跑 `cargo test -p egui_markdown --all-features` = 147 passed,本注记引用的 152 passed 与 550.75µs 终测数字对应的是叠加工作区 M3 后的代码。复现销账数字须以 M3 收口后的独立 `vendor:` commit 为检出基点(收口后在本注记与 vendor/README.md 变更表 M3 行回填 hash)。M1(`be05b52`)/M2(`ccc2945`)已在 git 历史,不受影响。

## #78 #42 M1 偏移→预览块位置通道选 vendor ①类而非 app-only(2026-10-03,#42 outline-preview-jump M1·自动拍板)

- **岔路**:任务书给「vendor ①类(渲染时记录块 span→rect 表 + pub 查询)」与「app-only 替代方案」两条路,要求自选并记录理由。**2026-10-15 订正**:本条原为「大纲跳预览」而做,预览面板已并入 Live 富渲染(live-unify P4/P5);块表现在由 `crate::live::ui` 消费(`live.rs:310` 与测试 `:5938`),三态视图下写作档正文与分栏档右栏共用同一条 vendor 通道。结论与理由不变。探测结论:vendored `render_token_range` 的块 widget 分支(表格/代码块/图片/引用/AI 指令卡)全部在 vendor 内部渲染,app 侧无任何挂载点能拿到它们的 rect(`code_block_buttons` 回调只有按钮行的 Ui,表格完全无回调);`section_anchors`(2026-09-26 已有)只在单 galley 路径有效,分段文档每 flush 覆盖写同一 data key 且 byte_start 是段内偏移——含表格/代码块的文档查表必错。app-only 不可达,通道必须开在 vendor。
- **备选**:①vendor ①类块表(`BlockSpanRect` + `block_span_rects`/`block_rect_at_offset`,记录点覆盖 render_galley 细块/块 widget 粗块/flush culling 粗块);②app-only 探针(照 `copy_button_rects` 手法在可挂载的回调里记 rect)——只能覆盖代码块按钮行,表格/图片/引用全盲,否决;③修旧 `section_anchors`(分段聚合 + 块 widget 补锚)——行级锚点模型(单 y、无 span)承载不了「块内偏移归属/间隙归后续块」的查询语义,改造量等于重写。
- **自动选择**:①。
- **理由**:块 widget 几何只有渲染它的那一层知道,①是唯一完整覆盖;帧号键控的表(首写重置 + 跨帧读 None)天然满足「查询结果随缓存失效」——屏幕坐标过期即不可用,文档变更随重渲染同帧重建;①类按上游 CONTRIBUTING 标准(纯新增 API + 独立测试 + 不动既有渲染路径),`vendor/egui_markdown/check.sh` 六项全绿,可 cherry-pick。实现中推翻了一个直觉方案:`section_to_token` 不能反查行 y——epaint `LayoutJob::append` 会合并同格式相邻 section(text_layout_types.rs:205-213),两序列索引不平行;改用块首文本在 job.text 单调正向查找 + `Galley::pos_from_cursor(prefer_next_row)` 定位(O(doc) 单遍,测试钉死单调性)。
- **如何改**:想换回行级锚点模型(③),删 label.rs 的 `BlockSpanRect`/`block_span_rects`/`block_rect_at_offset`/`is_block_start`/`block_needle`/`record_text_blocks` 与三处记录点,把 `section_anchors` 改成跨 flush 聚合(offset 换算 + 块 widget 手工补锚),app 侧查询退回「锚点 y 二分」;测试 tests/block_span_rects.rs 同步删。

## #79 #42 M1 大纲跳预览的滚动语义取「一次到位 + 目标居中」(Align::Center + ScrollAnimation::none)(2026-10-03,#42 outline-preview-jump M1·自动拍板)

- **岔路**:任务书跳转语义原文「预览 scroll_to_rect 一次到位居中」——「居中」可读作目标块滚到视口中央(Align::Center),也可读作「跳转到位」的笼统说法、实际按编辑器侧惯例对齐视口顶(Align::TOP,2026-09-26 旧实现即 TOP);「一次到位」可读作禁用滚动动画(ScrollAnimation::none)或仅「不要分步多次滚动」(默认动画也算一步请求)。
- **备选**:①`scroll_to_rect_animation(rect, Align::Center, ScrollAnimation::none())`——点击即达、目标块居中;②`Align::TOP` + 默认动画——沿用旧行为与 egui 默认平滑滚动;③Center + 默认动画——居中但走动画。
- **自动选择**:①。
- **理由**:按任务书字面直译;「一次到位」排除动画(平滑滚动在长文档里目标是屏外千行级距离,动画观感是「飞过半个文档」,编辑器内跳转同类场景也是瞬达);跳转目标是大纲点击的标题,居中让标题上下文(前后文)同时可见,长文档跳到底部标题时 TOP 会让目标贴顶失去「到了哪」的参照。端到端测试 `outline_jump_actually_scrolls_preview` 断言滚动真实发生(这是用户反馈「目前就源码跳转了」的回归锁:旧实现的 scroll_to_rect 写在 ScrollArea 闭包外,pass_state 滚动目标在下一帧 begin_pass 被清空,永不消费——egui 0.36 `PassState::begin_pass` 实读 + 双向对照实验实锤)。
- **如何改**:想改回顶部对齐或平滑滚动,只动 `crates/latermd-app/src/ui/preview.rs` 消费段的 `Align::Center`/`ScrollAnimation::none()` 两个实参;真机目视若觉居中跳动大,改 `Align::TOP` 一处即可。

> **2026-10-15 订正(载体迁移,live-unify P5)**:上述「如何改」里的 `ui/preview.rs` 已随
> `ui::preview` 整模块删除而不存在。该语义的现载体是 **Live 富渲染**(`crate::live::ui`:
> 写作档正文本身,以及分栏档右栏那一份只读实例),消费段在 `live.rs`(`ScrollProbe` /
> `scroll_probe_id`,预览侧那套 `ScrollProbe`/`map_source_offset` 已删)。跳转语义本身
> (Center + 无动画)不变,但**浏览态与禅定下只滚不进编辑**由 #148 的 `browse_locked` 决定。

## #80 #42 M2 预览面板收起期间的大纲跳转请求取「悬置,重开补跳」而非「即弃」(2026-10-03,#42 outline-preview-jump M2·自动拍板)

- **岔路**:右栏收起时点大纲(左栏大纲仍可见,这是真实路径),预览侧请求(`PreviewState::scroll_target`)怎么处置:①悬置——重开面板的第一帧消费并补跳到目标;②即弃——面板不可见就当请求作废,重开停在原地。任务书只写了「预览面板不可见时消费请求无 panic」,没有给可见性语义,两条都不与之冲突。
- **备选**:①悬置补跳(零代码 delta:消费只发生在 `preview::ui` 渲染帧,收起时本就不跑);②归约侧判 `layout.right`,不可见直接把 `scroll_target` 清掉(要给归约加 UI 可见性判断,与「滚动位置不是文档状态、不进归约」的既有口径相悖);③收起时挂起、重开时若期间有编辑才丢弃(现行为已是这个并集:悬置 + rebuild 即弃,见 `outline_click_residual_request_dropped_after_rebuild` 与 layout 测试第二段)。
- **自动选择**:①(即现状 ③的并集)。
- **理由**:用户点大纲的意图是「带我去那」,面板当时看不见不等于意图消失,重开补跳最贴近意图;文档变更后旧偏移对新文本无意义,rebuild 丢弃兜住「补跳跳错文本」的风险;①不新增任何生产代码,也不把可见性判断漏进归约。落档测试 `outline_click_with_preview_collapsed_pending_then_dropped_after_edit`(完整 draw 路径:悬置→重开补跳→收起→变更→丢弃→重开不复燃)。
- **如何改**:想改成「即弃」,在 `Message::RightPanelToggled(false)` 的归约里加一行清 `scroll_target`(归约本来就知道面板开关),并把 layout 测试第一段「重开消费」的断言翻转为「重开仍在原地」;悬置语义的文档口径在 `state.rs` 的 `scroll_target` 字段注释,同步改。

> **2026-10-15 整条作废(live-unify P5)**:`ui::preview` 与 `PreviewState::scroll_target`、
> `ScrollProbe` 已全部删除,「面板收起期间悬置」这个问题连同它的载体一起消失 —— 三态视图下
> 右栏不是可独立收起的面板,而是分栏档本身;大纲跳转统一走 Live 的 `cursor.jump_to` 通道
> (#148 定了浏览态/禅定下只滚不进编辑)。本条留档作历史记录,不再指导任何代码。
> 同批作废的还有本条落档那两条测试(`outline_click_with_preview_collapsed_*`)。

## #81 LP2-1 半隐藏的显形口径与弱化档位:caret 紧邻即显形 + text_edit 背景色 α0.65 常量,块级标记不弱化(2026-10-03,#14 lp-v2 LP2-1·自动拍板)

- **岔路**:①规格只说「光标(焦点)落在某段标记上时,该段标记显形」,「落在」的精确边界未定义——严格内部(`cs<c<ce`,两字符标记如 `**` 仅剩一个可触发位置)还是紧邻即显形(`cs<=c<=ce`);②弱化的实现档位——遮罩色与不透明度取什么、要不要做成用户设置;③「标记」的外延——只内联(`**`/`*`/`_`/`` ` ``/`~~`/链接括号/图片/autolink)还是连块级(`#`、列表符、表格竖线、任务框 `[ ]`)一并弱化。
- **备选**:①显形:严格内部 / 紧邻即显形 / 整对联动(光标在 `**bold**` 内容里也显形两端,Typora 风格);②档位:字形重排着色(自定义 layouter,与本帧光标有一帧错位)/ 背景色遮罩 α 可调 / 背景色遮罩 α 常量;③外延:仅内联 / 内联+块级。
- **自动选择**:①紧邻即显形;②`text_edit_bg_color` × α=166(≈0.65)编译期常量(`MARK_FADE_ALPHA`,live.rs),不做用户可调配置面(规格明文);③仅内联。
- **理由**:①egui 点击定位把光标放在被点字形的边界上,严格内部会让「点一下标记显形」成为哑路径(测试 `live_click_on_faded_mark_focuses_editor_and_reveals` 钉住点击显形);整对联动与「该段标记显形」的字面不符且常亮削弱「接近渲染效果」的目标;②遮罩与编辑框底色同源才能读作「文字变淡」而非「贴了高亮」;α0.65 按混合公式字形残留约 35% 亮度(数值推断,真机目视留人工,见 notes),用户可调会新增设置面,规格明文不做;③任务书名即「内联标记半隐藏」,块级标记弱化会碰 `#`/列表符的编辑可读性,且 `inline_marks` 的 gap 机制扩展到 Heading 是一行 match 的事,留作后续增量。
- **如何改**:①想改严格内部,把 `paint_mark_fades` 里 `start <= caret && caret <= chars` 收紧为 `start < caret && caret < chars`(live.rs,测试 `live_reveals_mark_under_caret_or_selection` 第二段同步把光标 1 断言改到仅内部位置);②想调 α,改 `MARK_FADE_ALPHA` 一处常量;想做整对联动,需要在 `inline_marks` 输出上补「配对标记同组」元数据(区间成对标记),显形判定按组;③想把块级标记纳入,在 `latermd_md::inline_marks` 的容器 match 里加 `Tag::Heading`(Start/End 区间同为整构造,gap 机制现成),并补 heading 样式的 md 单测。

## #82 LP2-2 选区扩展的交互边界:双击吞标记、拖选锚点口径、嵌套取最内层、键盘/三击不扩展(2026-10-03,#14 lp-v2 LP2-2·自动拍板)

- **岔路**:①双击选词是否吞掉标记(词扩到 `**词**` 还是保持裸词);②拖选半程松手的落点——何时把选区吸附到完整标记对;③嵌套构造(`[**b**](u)`、`***x***`)扩到哪一层;④拖选「从标记内侧发起」的锚点边界口径(单字符标记 `[`/`` ` `` 没有内部位置);⑤键盘选区(Shift+方向键)与三击选行是否同样扩展。
- **备选**:①吞标记 / 不吞(裸词,传统编辑器行为);②吸附时机 = 拖选全程持续吸附 / 仅松手帧吸附 / 从不吸附 / 「选区两端只要都在对内」就吸附;③最内层 / 最外层;④锚点严格在标记字符之间(单字符标记不可及)/ 压着标记含两侧边界;⑤键盘与三击同样扩展 / 不扩展。
- **自动选择**:①吞——双击词扩到**包含该词的最内层**标记对(内容+两侧标记);②仅松手帧、且要「锚点压着某一侧标记发起 + 本次选区整个含在该构造内」两个条件同时成立才一次性吸附(锚点=按下帧的塌缩光标,press 帧记、release 帧消费);③最内层(构造区间最短的一对);④闭区间 `opening.start <= anchor <= opening.end`(闭侧同理)——压上即算;⑤不扩展,只有指针双击/拖选两种手势触发。
- **理由**:①任务书明文目标是「让编辑标记本身有可及入口」,双击是最高频入口,吞掉标记后选区盖住整对、两段标记随选区显形,打字即连标记一起替换;②持续吸附会在拖选过程中反复改写选区(与用户手感对抗),「选区两端都在对内就吸附」会把长粗体段里的普通拖选吹胀成整段(最小惊讶的对立面);锚点条件保证只有「从标记上发起」的拖选才扩,从内容中部发起的普通拖选原样保留(测试 `live_drag_release_expands_only_from_mark_anchor` 双向钉住);③最内层=词直接所在的那一对,贴直觉;外层可通过先选中内层后再选外层文本间接达成;④egui 光标只能落在字符边界上,`[`/`` ` `` 这类单字符标记只有两个边界位置,严格内部口径会让它们永远不可及;⑤键盘选区是精确操作,被静默改写最令人惊讶;三击选行与跨对选区同理,且选行/跨对时「选区整个含在构造内」多数自然不成立。扩展是一次性的(仅事件帧),静止帧不吸附。
- **如何改**:①改双击不吞,把 live.rs 手势判定里 `double_clicked` 分支删掉(只留拖选),测试 `live_double_click_inside_pair_expands_selection_once` 翻转为断言选区停在词上;②放宽/收紧吸附条件在 `latermd_md::mark_interaction` 的 `anchored` 谓词与 `within(construct, selection)` 过滤(想全程吸附则把 `MarkGesture` 从事件帧改为每帧传入并跳过 `expanded != 当前选区` 的守卫);③改最外层,把 best-pair 的比较从「构造最短」翻成「构造最长」(latermd-md `mark_interaction`);④改严格内部,把两处闭区间 `<=` 收成 `<`(注意单字符标记将不可及);⑤想把键盘选区也纳入,需要在 live.rs 键盘事件帧合成 `MarkGesture` 传入(现状无此路径,键盘选区不经手势判定)。

## #83 LP2-3 点击进编辑的光标写回不触发视口跟随(仅键盘导航/跨块路由/大纲跳转触发)(2026-10-03,#14 lp-v2 LP2-3·自动拍板)

- **岔路**:pending_caret 落地帧的视口跟随是否区分来源 —— 点击富渲染块进编辑(v1:光标落到块末)也是一次程序化光标写回,跟不跟随。
- **备选**:①所有 pending_caret 落地帧一律跟随(实现最省,来源无关);②仅跨块路由/大纲跳转跟随,点击进编辑不跟随;③点击进编辑改为光标落在点击行并跟随(要新增点击点→块内偏移换算,v1 落块末的语义一并改)。
- **自动选择**:②。`LiveState::caret_follow` 与 pending_caret 同生共死:路由/jump_to 置真,activate 置假。
- **理由**:点击是指针交互,用户刚用指针把视口定位到点击处,同帧再把视口拽到光标落点(块末)等于抢滚动(#29「滚轮/空闲帧不抢滚动」的同族红线);对高度超视口的长块尤其明显 —— 点块首、视图跳块尾。路由/跳转是键盘/结构性移动,光标去了别的块,视图不同帧跟上就会出现「光标写了但视图没跟」。③超出 LP2-3 范围(v1 点击落块末是既有语义)。
- **如何改**:想改①,把 live.rs `activate` 分支的 `live.caret_follow = false` 删掉(该字段默认即真);想改③,需要在富渲染分支记录点击命中的块内行,`pending_caret` 给行首偏移,并同步改 `live_click_outside_copy_button_still_enters_edit` 的断言。

## #84 LP2-3 Live 大纲跳转的目标块按「字节归属」而非「标题内容块」(2026-10-03,#14 lp-v2 LP2-3·自动拍板)

- **岔路**:jump_to 的字符偏移落到 v1 块表时,目标块怎么取 —— 块表现状里标题的 `# ` 标记可能归上一块尾(实测 `# 顶\n\n…\n\n# 底\n` 的底标题 `# ` 字节属段落块),按字节归属会激活段落块而非标题内容块。
- **备选**:①`block_containing(跳转字节)` —— 字节属哪块就激活哪块(与 sync/路由的既有不变量一致);②跳过标题标记字节(`#`/空格)落到标题内容所在块;③改 latermd_md::blocks 让标题标记归标题块(块表语义变更,波及 v1 全部行为)。
- **自动选择**:①。
- **理由**:光标字节与源码模式 jump_to 的落点完全相同(标题行首),「同一入口同一时序口径」在光标位置上严格成立;视口滚到光标行,标题行必在视口内,跳转观感达成。②引入 app 侧对标题语法的二次猜测,与「结构化操作在 token/AST 层」铁律相悖;③是块表 v2 的课题,不在 LP2-3 内夹带。
- **如何改**:想改②,在 live.rs jump_to 消费处对字节做「跳过 #/空格」换算(注意 Setext/列表等形态会猜错,不建议);想改③,动 `latermd_md::blocks` 的 heading 段起点(连 `blocks_cover_the_whole_document` 等既有断言一起改)。

## #85 LP2-3 接受 egui 0.36 焦点锁单帧窗:授予帧的下一帧按裸方向键会被记忆层焦点导航抢焦,app 侧不绕(2026-10-03,#14 lp-v2 LP2-3·自动拍板)

- **岔路**:egui 0.36 的 `set_focus_lock_filter`(TextEdit 捕获方向键、阻止记忆层焦点导航)要等「持焦的下一帧 show()」才生效;`request_focus` 授予帧的紧邻下一帧若出现裸 ↑/↓,`Focus::begin_pass` 会按 cardinal 导航把焦点交给相邻可聚焦 widget(Live 面板里富渲染块 `Sense::click_and_drag` 含 FOCUSABLE 位,候选存在)。窗口宽一帧(≈16ms)。app 侧是否人为闭合它。
- **备选**:①接受窗口(人类点击→按键间隔 ≥100ms,永远跨过它;测试按真实节奏在授予帧后空转一帧);②授予前一帧提前 `request_focus`(窗口宽度不变,只是挪位置);③每次落地帧后主动监测「焦点被抢」再抢回来(启发式,与框架对抗)。
- **自动选择**:①。
- **理由**:②实测无收益(窗口仍是一帧,且提前给尚未渲染的块授焦引入新边界);③是在模拟框架行为,必然跟 egui 内部实现耦合,升级即碎。源码栏不受影响的唯一原因是其面板内恰无其它可聚焦 widget(碰巧而非设计),Live 面板富渲染块天然可聚焦,窗口才显形 —— 这是 egui 0.36 框架级时序,不是本仓库代码缺陷。
- **如何改**:若上游修复或升级 egui 后窗口消失,删掉 live.rs 测试 `live_keyboard_navigation_scrolls_caret_into_view` 里授予帧后的空转帧注释段即可收紧测试;真要 app 侧闭合,唯一干净路径是在 vendored 层给活动块 TextEdit 换 `.event_filter(EventFilter{ vertical_arrows: true, horizontal_arrows: true, tab: false, escape: false })` 之外再想办法提前授焦 —— 属 vendor 变更,须按 §6 登记。

## #86 LP2-4 统一撤销评估:维持「按块分段的 TextEdit 内建 undoer」,不换自维护 undoer(2026-10-03,#14 lp-v2 LP2-4·评估+自动拍板)

- **岔路**:#25 已知边界「Live Preview 每块 undo 快照是本块的,跨块撤销按块分段」,LP2-4 要求评估「换成自维护 undoer 换全文统一撤销」的代价后定去留。机制现状(本会话实读):undo 走 egui 0.36.2 TextEdit 内建 undoer,快照 = `(CCursorRange, String)`(整篇文本 + 光标区间,egui `util/undoer.rs:12`、`text_edit/builder.rs:1085-1087` 帧前后各 feed 一次),undoer 挂在按 **widget id** 键控的 `TextEditState`(每块独立一栈,`max_undos: 100`);源码模式整篇一个 id(`tab_editor_id`),Live 模式每块一个 id(live.rs `editor_id.with(("live-block", index))`)—— 块 id 不同 → 栈不同 → Ctrl+Z 只撤当前块内的修改。
- **备选**:①换自维护 undoer(latermd-editor 建全文快照栈,live.rs 活动块改只读渲染 + 自管编辑路径,统一 Ctrl+Z 跨块一步回);②维持分段(现状);③中间路线「跨块撤销桥」:保留内建 undoer,当前块栈撤空时把焦点与下一次 Ctrl+Z 路由到上一个编辑过的块(经 pending_caret 路由,不动 undoer 本体)。
- **自动选择(评估结论)**:②维持分段;③登记为将来有真实用户反馈时的首选升级路径。产品行为同步写进用户可见文档(README.md「功能特性 · 编辑与预览」:Live 模式下撤销按块分段)。
- **理由**:①的代价逐项过 —— ⑴**IME 组合要自兜**:egui 的快照分组是 flux/stable_time(1s)/auto_save(30s)时序语义(`undoer.rs:178-225`),组合(preedit)期间文本不进 buffer、commit 时一次性落一个点;自维护若把 preedit 变更当编辑推栈,撤销会在组合中间撕开文本。AGENTS §8「IME 是头号风险」,#19 刚把候选框跟随做稳,为撤销语义重开 IME 边界不值。⑵**选区/光标行为要自兜**:内建快照带 CCursorRange,撤销恢复文本的同帧恢复光标;自维护要每步快照自带光标,跨块撤销还得走 live.rs 的 pending_caret 跨块路由 + caret_follow 时序(LP2-3 刚收口的口径)。⑶**内建交互竞争要自兜**:Ctrl+Z/Ctrl+Shift+Z/Ctrl+Y 在 TextEdit 事件循环内部消费(`builder.rs:1195-1230`),换自维护要么 event_filter 拦键要么关内建,双栈并存必错;拦键后还要处理「焦点不在编辑器时 Ctrl+Z 归谁」。⑷**快照成本**:内建按整篇 String 存(每槽一份,上限 100 份);全文统一撤销沿用快照式则每推一点拷整篇(长文档下百份快照的内存/拷贝成本),delta 化又是一块独立工程。⑸**收益面**:块内撤销是绝对主频;分段撤销是「语义分段」的体验缺口而非功能缺失(每次 Ctrl+Z 撤当前块内一步,仍可用),当前无用户反馈驱动。
- **如何改**:要统一撤销走① —— latermd-editor 加自维护快照栈(文本+光标,flux 分组),live.rs 活动块 TextEdit 换非交互渲染或锁内建 undoer,预估一个独立 feature 棒(IME/选区/拦键三面都要无头+真机验收);要较轻的跨块连续撤销走③ —— live.rs 维护「最近编辑块序号栈」,块 undoer 撤空(该次 Ctrl+Z 无变更)时转移焦点到上一个编辑块,不动 undoer 本体、IME 零风险;推翻本条(换回①)时同步回改 README 的行为说明与 #25 的「已知边界」表述(该边界随本条从「接受」升格为「有意为之的产品行为」)。

## #87 BK1 反向链接扫描(#15)的四处口径:路径式目标按相对路径匹配、同步直扫无后台服务、自链计入、悬空引用计入(2026-10-03,#15 backlinks BK1·自动拍板)

- **岔路**:任务书把四处自由度交给实现者:①路径式目标 `[[dir/doc]]` 按文件名末段还是相对路径匹配;②API 形态取 SearchService 同款后台服务还是同步函数(任务书明示「反向链接量级小,允许同步直扫」);③自链(`note.md` 里写 `[[note]]`)计不计入结果;④悬空引用(目标文件不存在)计不计入结果。
- **备选**:①末段匹配(任何目录的同名文档都收,Obsidian 部分插件行为);②后台服务(代际号 + channel 流式回传);③自链排除(「谁链接了我」直觉上不含自己);④悬空不计数(扫描时校验目标在盘上存在)。
- **自动选择**:①**相对路径匹配**——目标含 `/`(或 `\`)时,目标串与文档相对路径两侧都归一化分隔符、末段剥一个 `.md`/`.markdown` 后缀、忽略大小写后全等;目标不含 `/` 时按文件名 stem 忽略大小写全等(`[[同名]]` 与 `[[同名.md]]` 互容,扩展名本身不限 md/markdown)。②**同步直扫**——`latermd_search::backlinks(root, doc, stop, max_hits)`,`stop` 短路保留(大仓可中断,中断返回已收到的部分且不置 truncated);收全 → 排序(来源路径, 行号, 目标)→ 截断,与 `list_files` 同款纪律保证截断确定性。③自链**计入**。④悬空引用**计入**(纯文本匹配,不查盘上存在性)。
- **理由**:①与预览侧 `find_by_name`(decisions-pending #26)的正向解析对称——带 `/` 的目标正向只按相对路径直取,反向若按末段匹配会出现「面板说有反向链接、点过去却打开另一目录的同名文档」的错位;不同目录的同名文档(`dir/goal.md` 与 `other/goal.md`)不被 `[[other/goal]]` 误伤;大小写不敏感与 stem 口径(#26)在扫描内部保持一致,不因目标是否带 `/` 而忽明忽暗。②反向链接量级小(单文档被引常态几十以内),面板要的是完整列表而非流式增量;`SearchService` 的 channel + 代际号是为「边出结果边取消」的全库正则搜索设计的,为一次直扫复制一套线程机制只增表面积;要后台化的消费方拿 `stop` 闭包自行包线程即可。③自链是真实引用(文档内目录、回头引用常见),扫描层忠实呈现,要不要在 UI 层折叠展示留给面板模块决定。④不查存在性使扫描无「先列文件再匹配」的顺序依赖,且天然覆盖「未落盘/未保存文档已被引用」的场合;被查询文档存在时,按上述口径命中它的链接按定义不悬空,两条口径不打架。**已知边界**:无 `/` 目标按文件名全局匹配,同名文档多篇时(如 `a/goal.md` 与 `b/goal.md` 各被 `[[goal]]` 引用)两篇各自查询都会收到该引用——与 `find_by_name` 正向「取列表首个命中」的模糊性同源(#26 既有),不在此放大;目标原文中的 `#` 锚点(`[[doc#章节]]`)不剥,正向同样不解析,维持对称;反向的 `.md` 后缀容错比正向宽(正向 `find_by_name` 对无 `/` 目标不剥后缀),`[[note.md]]` 会计入反向链接但点击未必正向打开——面板展示目标原文可让用户识别写法。
- **如何改**:①要末段匹配——把 `target_matches_doc`(crates/latermd-search/src/lib.rs)的含 `/` 分支改成取末段走 stem 比较;要路径匹配大小写敏感(与 Linux 正向直取完全对称)——`path_key` 去掉两处 `to_lowercase`;②要后台服务——照 `SearchService` 的代际号模式包一层线程,`backlinks` 本体不动;③要排除自链——面板层按「来源相对路径 == 目标文档相对路径」过滤,或给 `backlinks` 加 `include_self: bool` 参数;④要悬空不计数——匹配前先 `find_by_name` 验证目标可解析(代价:每条候选多一次全库列举,需缓存);要让 `[[note.md]]` 正向也能打开——给 `find_by_name` 的无 `/` 分支补同款 `strip_md_extension` 容错(app 侧 filetree.rs,与本条正向对称收口)。

> **销账注记(2026-10-05,#87 已知边界「后缀容错错位」收口)**:本条「如何改」末句点名的正向对称收口已落地——`find_by_name`(app 侧 filetree.rs)无 `/` 分支补同款后缀容错(`strip_md_suffix`,剥一个 `.md`/`.markdown`,与扫描侧 `strip_md_extension` 同口径):先精确名(目标 == 文件 stem,忽略大小写)匹配,未命中且剥后缀非空时按 stem 再试一轮;含 `/` 的路径式目标口径不动(直取不中不回落模糊匹配),两轮各取列表首个命中、精确命中恒先于剥后缀命中(既有优先级不变)。自此 `[[note.md]]` 计入反向、点击同样打开,错位消除;`.markdown` 后缀一并覆盖(任务书行文只写「.md 后缀」,按本条「同款 strip_md_extension 容错」与对齐目标取两后缀,悬空目标仍落提示行)。本注记不改动上方原条目,翻案仍按其「如何改」。

## #99 #42 块表记录(block_span_rects)在表格密集长文档上每帧 ~136µs 的记录成本,待其归属模块优化或加开关(#52 M3 复测发现·挂账)

- **岔路**:#52 M3 按否决线复跑滚动 bench(long_doc_100k)发现稳态帧对 R1 +47%/+54%(358.83→528.05 µs / 371.36→571.32 µs,冷首帧持平)。A/B 逐项关断定位(临时 env 开关,测完还原):**#42 的 vendored 块表记录(main `85cdf93`,R1 基线 2026-10-02 之后才进 main)贡献 +136µs/帧**——混合文档 ~166 表格 + 数百 flush 块每帧全部 record_block_rect/record_widget_block/record_text_blocks 进帧级 Vec;M1 准入扫描 +29µs(已被 #52 M3 的字节快速否决修掉)、M2 块 key 内容哈希 +36µs(内容寻址 cull 语义必需,已接受)、#52 M3 shaped galley 复用 −14µs。非本模块修复对象(#42 是已合入 main 的另一模块交付物),按约束挂账。
- **备选**:①record 路径增量化/惰性化(块表本来按帧重建,代价在「每帧全量重建」——可改为跨帧增量或仅在查询发生时标记脏);②块表记录加 opt-in(如 `record_block_rects(bool)` builder,LaterMD 在跳预览/大纲功能活跃时开、纯阅读关);③接受现状(距 55fps 预算仍有 34× 余量,非用户可感知)。
- **自动选择**:③暂接受 + 本条挂账(记录在案,不动 #42 已验收行为)。
- **理由**:#52 M3 的验收对象是流式追加曲线(已达标销账),滚动回归的**主因在本分支之外**(main 既有);#42 的块表是大纲跳预览(#78-#80)的功能基座,单方面改记录语义可能破坏其当帧几何查询契约,应连同其消费者一起评估;且绝对值(528µs vs 18.18ms 预算)不构成用户可见劣化。
- **如何改**:若要动,优先②(builder 开关,默认保持现状,#42 的 273 行 vendored 测试钉住开态行为);要①需先核 #78「帧号键控、首写重置、跨帧读 None」契约——增量表与「屏幕坐标过期即不可用」语义的兼容性是设计点;两条都是 vendor ①类,按 §6 独立 commit + 变更表登记。

> **销账注记(2026-10-06,#59 perf-round M2)**:①路线落地——`record_block_rect`(vendor label.rs)改 `get_temp_mut_or_insert_with` 原地追加(记录均摊 O(1),保留每记录的帧号校验 clear,与 render() 帧首重置双保险;#78 契约逐字保持),连带帧首重置原地 clear 使块表 allocation 跨帧复用;语义零变化(读侧 `block_span_rects` 不动,#42 既有行为测试全绿)。验收数字(perf-recheck §11.1):100k bench 稳态 542.28→426.04 µs(top,−21.7% p=0.00)/558.62→426.06 µs(middle,−22.8% p=0.00),落在「#42 前水位 R1 + M2 固有 +36µs」预期带;tab_switch 20k 整帧稳态 12.52→3.66 ms、单件 21.40→4.60 ms(与 §10.2 探针 B「关块表」4.49 ms 吻合)。本注记不改动上方原条目,翻案仍按其「如何改」。

## #126 TOC 生成锚点五口径:slug 规则自选(GitHub 风格+CJK 保留)、导出 HTML 顺手补 id、深度上限 h1–h3、无标题回空串、缩进取相对最小层级(2026-10-08,#66 M1 toc-generate·自动拍板)

- **岔路**:①导出 HTML 现状核实为**无标题锚点**(`export_html` 直接 `html::push_html`,事件流无 id 注入,latermd-export/src/lib.rs 旧版)——TOC 链接的 `#锚点` 无处可跳,slug 规则必须连同导出侧一起定;②slug 规则本身;③TOC 深度上限;④空文档/无标题文档的行为;⑤缩进口径(按绝对层级还是相对层级)。
- **备选**:①导出不补 id(TOC 链接在导出物里全断)/补 id;②GitHub 风格(小写化+标点删+空格→`-`+重复 `-1` 后缀)/全保留中文原文/严格 GitHub(剥 CJK)/Pandoc 风格;③h1–h3 / h1–h4 / 全 h1–h6;④空串/占位提示行;⑤按绝对层级(2×(level−1) 空格)/按相对最小层级。
- **自动选择**:①**补 id**——`export_html` 在高亮包裹后的事件流上注入(`attach_heading_ids`,无 id 标题全部注入;`parser_options` 未开 `ENABLE_HEADING_ATTRIBUTES`,`{#custom-id}` 不被解析,无覆盖冲突;PDF 链路不补,同 #65「如实不硬做」先例)。②**GitHub 风格+CJK 保留**:Unicode 空白→`-`(逐字符,连续空白产连续 `-`)、`alphanumeric`(含 CJK/假名)与 `-`/`_` 保留、字母 `to_lowercase`、其余(标点/emoji)删除、**不 trim**(`🚀 Launch`→`-launch`,GitHub anchor 同款);重复标题序贯去重 `x`→`x-1`→`x-2`(撞上真实标题 `x-1` 就继续加到 `x-1-1`)。规则唯一持有点 `latermd_md::heading_slug`+`Slugger`,TOC 与导出共用。③**h1–h3**(`TOC_MAX_DEPTH=3`),但 depth 外标题**照常消耗去重计数**——TOC 与导出对同一标题全集序贯分配,两侧锚点才不漂移。④**空串**(不产提示,提示属消费方 UI)。⑤**相对最小层级**:2 空格 × (level − 入选条目最小层级),首行永远零缩进——按绝对层级会让 h3 开头的文档首行吃 4 空格,CommonMark 里落成缩进代码块;跳级照实缩进。
- **理由**:①现状无锚点,只做 TOC 不补 id 等于交付全断链的目录;补 id 是事件流上加一个属性,不碰渲染管线,pulldown 0.13.4 html writer 对 `Tag::Heading{id:Some}` 原生输出 `id="…"`(实测)。②GitHub 风格是 Markdown 生态事实标准,读者对 `-launch` 形态有预期;CJK 保留是中文产品的基本盘(严格 GitHub 剥 CJK 会把全部中文标题锚点变空);「不 trim+逐字符映射」让规则纯函数化,逐字符断言可钉死,导出与 TOC 天然一致。③h1–h3 与主流工具默认一致,h4+ 进目录只会淹没结构;计数不分层级是「两侧对同一全集序贯分配」的一致性要求,否则 h4 的同名标题只在导出侧吃掉一个后缀,TOC 与导出锚点漂移(测试 `toc_anchors_align_with_exported_heading_ids` 钉住该案例)。④空串让消费方零成本判空,不替 UI 做文案决定。⑤首行零缩进是硬正确(文件顶 4 空格=代码块),相对层级保真优于绝对层级的美观。
- **如何改**:①想不补 id——删 `attach_heading_ids` 调用与函数(latermd-export),既有测试 `headings_carry_anchor_ids` 等四条同步删;要支持 `{#custom-id}` 显式锚点——`parser_options` 加 `ENABLE_HEADING_ATTRIBUTES` 并让注入跳过 `id:Some`(需评估正文里 `{#…}` 误解析面)。②改规则只动 `latermd_md::heading_slug` 一个函数,两侧自动跟随;要 trim 或合并连续空白,同步改 `heading_slug_punctuation_and_emoji_char_by_char` 的逐字符断言。③调 `TOC_MAX_DEPTH` 一个常量;要 h4 进目录需同时接受 8 空格缩进行的观感审查。④要占位提示——`generate_toc` 空分支改返回提示串,M2 消费侧同步。⑤要绝对层级——把基线 `base` 钉成 1(注意 h2+/h3+ 开头文档首行落代码块的回归,`generate_toc_indent_baseline_is_min_selected_level` 会红)。已知边界:含内联格式的标题行在大纲通道拆条(vendored 既有廉价口径),TOC 条目随之拆条(`generate_toc_feeds_from_real_outline_channel` 如实钉住);纯 emoji 标题 slug 为空,TOC 产出 `(#)`,导出产 `id=""`,点击无效但文本合法。

## #127 InsertToc 命令接入五口径:菜单归位「编辑」、插入点文档头、区域识别标记 HTML 注释包围、无标题双档提示、替换后光标折叠到块首(2026-10-08,#66 M2 toc-generate·自动拍板)

- **岔路**:①菜单归属(「编辑」还是「格式」插入段);②插入点(光标处还是文档头);③既有 TOC 块的区域识别口径(标记 HTML 注释包围还是标题列表启发式);④无标题文档执行命令的行为(静默/提示文案);⑤替换后的光标行为(块首/块尾/原地不动)+出厂快捷键的取舍。
- **备选**:①「编辑」(与复制行/查找/替换/跳转同列)/「格式」插入段(与图片、Emoji 同列);②文档头(确定性)/光标处(随用随插);③标记包围(`<!-- TOC -->`/`<!-- /TOC -->` 整行 trim 全等,doctoc/mkdocs-material 同族)/标题列表启发式(逐行匹配 `- [..](#..)` 且随大纲收敛,误识别面大:正文里手写的同形态列表会被当成块)/不做替换(重复执行堆叠,#66 任务书明令排除);④全无标题与「只有四级及更深标题」分两档提示/合一档/静默;⑤块首(`jump_to` 通道,源码/Live 同一入口)/块尾/不动光标;快捷键 Ctrl/Cmd+Alt+C/Ctrl+Shift+O(与 Open 家族语义相撞)/不绑键(照 ExportPdf 先例)。
- **自动选择**:①**「编辑」菜单、Edit 组**——格式菜单十五条全是选区/光标处的格式化,格式「插入」段现存两条都是对话框类动作(点了先弹框);InsertToc 点了直接改文档、不作用选区,与复制行/查找/替换/跳转同属「不作用选区的文档级辅助动作」,归「编辑」更诚实(条目助记也顺:编辑菜单内 T 空闲,恰好是 TOC 词首;格式菜单里 T 已被 FormatTable 占用)。②**文档头**——TOC 惯例位置在文首;光标处插入会让「再执行=替换」的落点随光标漂移,文档头让插入/替换两个路径共用一个确定性锚点(byte 0),undo 的快照边界也随之稳定。③**标记 HTML 注释包围**——`latermd_md::TOC_BEGIN_MARKER`/`TOC_END_MARKER`(`<!-- TOC -->`/`<!-- /TOC -->`,整行 trim 全等容忍行尾空白与 CRLF 的 `\r`),写入(`toc_block`)与识别(`toc_region_span`)同一事实源,不存在「生成的块认不出」的漂移;取第一对配对标记,有头无尾/结束先于开始都按无块处理(走首插);**手工改过标记之间的内容再执行=整块覆盖,不保留**(任务书指定口径,如实登记:生成物由命令负责,手工目录该用启发式工具的读者自带)。④**双档提示**:大纲为空落「文档没有标题,无法生成目录」;大纲非空但 depth 内无可收录条目落「目录只收录一至三级标题,当前文档没有可收录的标题」——两种成因的修复动作不同(前者先写标题,后者改层级或调 `TOC_MAX_DEPTH`),文档/选区/修订号一概不动。⑤**光标折叠到块首**(旧块原地替换则块首不变,`cursor.jump_to` 通道,写回帧视口随光标滚到块首,TOC 立即可见;源码/Live 两模式同一入口,与大纲跳转同通道);`cursor.byte` 照 `apply_format` 同款按字符边界折算(防状态栏同帧切片越界,2026-09-27 实测崩溃的既有防线)。快捷键**出厂 Ctrl/Cmd+Alt+C**(C = Contents):出厂表 Ctrl/Cmd+Alt 层只有 R/W/D,C 空闲;`matches_logically` 只把 Shift 当可忽略的额外键、Alt 必须显式按下(egui 0.36.2 modifiers.rs:211 实读),与 Ctrl/Cmd+Shift+C(围栏代码块)双向不误触(`insert_toc_shortcut_is_ctrl_alt_c_and_conflict_free` 两条对照实测);含 Alt 的绑定避开菜单标题助记集 F/E/O/V/X/A/S(#122 同款审计),C 不在其中。
- **理由**:①归属跟着动作家族走,不跟着「名字里有插入」走——菜单是用户找功能的索引,「编辑=对整篇文档做点什么」比「格式=只管选中文字的样子的例外条款」更可预测。②确定性的插入点让三个可测试性质同时成立:首插文本可逐字节断言、替换区间与光标无关、反复执行文本不动(幂等,`replace_round_trip_is_stable` 钉住)。③启发式识别要回答「几行才算目录、缩进不合规范算不算、正文里的示例列表算不算」,每个答案都是误伤面;标记包围的识别规则一行可述,且与 doctoc/mkdocs-material 等主流工具的用户习惯一致,已在用这类工具的文档天然兼容。④静默会让「为什么没反应」变成排查题;合一档提示让只有四级标题的作者误以为要凭空造一级标题。⑤块首落点是「看刚生成的目录」的自然视点;不动光标在长文档替换后会留在原字节偏移,文本变短时指向漂移,还得另做钳制,不如直接给一个有语义的落点。Alt+C 出厂:不绑键照 ExportPdf 先例虽可行,但任务书要求「快捷键触发」档测试,且三修饰层还有余量;Ctrl+Shift+O 与「打开」家族(O/Open、Shift+O 无占用但语义相撞)和浏览器书签管理器惯例相撞,弃。
- **如何改**:①要归「格式」——`Command::group()` 的 Edit 臂移到 Format,EDIT_MENU 条目移进 FORMAT_MENU 插入段,`menu_placement_matches_command_group` 自动跟随;条目助记得换(T 被 FormatTable 占用)。②要光标处插入——`insert_toc` 归约里 `byte_start` 取当前选区起点,但「再执行=替换」需同时保留标记识别;光标恰落在旧块内时插入点与替换区间的先后关系要单独定义。③要启发式识别——`toc_region_span` 换逐行匹配 `- [..](#..)` 与当前大纲条目的收敛比较,`toc_region_span_requires_paired_markers` 五条断言全部重写;建议保留标记包围为兼容读取。④要合一档提示——`insert_toc` 的 `notice` 两分支合并,`insert_toc_without_usable_headings_only_notices` 的第二条断言同步。⑤要块尾落点——`jump_to` 取 `end` 对应字符偏移;要不动光标——删 `jump_to` 赋值行,但 `cursor.byte` 折算必须保留(越界防线)。要改键位——`default_shortcut` 一处,`insert_toc_shortcut_is_ctrl_alt_c_and_conflict_free` 的出厂断言同步;老 `keymap.json` 走既有增量补默认(keymap.rs `load_from`),缺 id 自动补新默认。已知边界:标记行写在围栏代码块内同样被识别(不做围栏感知,doctoc 同宽);残块(有头无尾)不替换、新块插文档头,旧残块原地留存;替换文本恰为块本体,块外字节零增删,首插在块尾补一个空行隔开正文而替换不补(块后既有空行原样保留)——反复执行不涨空行;CRLF 文档里插入的块是 LF 行尾(文件层 UTF-8 原样进出不做换行转换,块内 LF 与文档混排,渲染与导出无碍);导出 HTML 里两个标记注释原样输出为 HTML 注释(不可见、无害)。

## #143 编辑器右键菜单四口径:Live 非活动块不弹、弹层期间消息下一帧归约、粘贴文本同步读、已知边界浮标吞右键(2026-10-13,#context-menus M1·自动拍板)

- **岔路**:①Live 模式非活动块(富渲染)右键弹不弹菜单;②菜单条目点击产出的 `Message` 走 outbox(下一帧 logic 归约)还是当帧直改状态;③「粘贴」读系统剪贴板文本同步(arboard/NSPasteboard)还是后台线程;④选区 AI 浮标矩形上的右键归属。
- **备选**:①不弹(先激活再右键)/弹只读菜单(复制/全选)/弹完整菜单;②outbox 消息(既有纪律)/当帧直改;③同步读(mac_menu 先例,X11 阻塞风险面与 egui-winit Ctrl+V 相同)/后台线程 + mpsc(clipboard.rs 图片同款);④浮标吞(可见 UI 优先)/穿透到编辑器菜单。
- **自动选择**:①**不弹**——Live 富渲染块不是 TextEdit,没有选区/光标语义,「复制当前行/加粗」作用的对象不存在;单击进入编辑(设活动块)后右键即得完整菜单,一步激活是既有的进入语义,不为右键另造激活通道。②**outbox 消息,下一帧归约**——与全部既有 UI 同纪律(state 归约只在 logic,ADR-005 §2.3);代价是菜单点击到动作生效隔一帧(60fps 下 ~16ms,无感)。③**同步读**——mac_menu 先例在前,风险面与 egui-winit Ctrl+V 的文本读取相同(同一条 `get_text` 路径);真正要后台化时图片粘贴的线程+mpsc 手法现成,不为菜单单独做。④**浮标吞**——浮标是挂在选区尾的可见 UI(「AI 续写/润色」两枚按钮),点它就是用浮标;右键落在浮标上时浮标响应居上层,TextEdit 收不到 secondary click,菜单不弹,这是层级语义的自然结果。
- **理由**:①右键菜单的作用对象是「编辑态」——VS Code/Typora 同款:非编辑区右键要么不弹要么只有视图级条目;LaterMD 的 Live 浏览态没有任何可在弹层里执行的编辑动作,弹了必是灰条。②当帧直改会绕开归约纪律,undo/选区镜像全部要另立通道,成本远大于一帧延迟。③同步读的卡顿窗口 = 一次剪贴板宿主握手(毫秒级),菜单粘贴是显式用户动作,用户对它的延迟容忍远高于打字流;真在 X11 观测到卡顿再后台化,不预支复杂度。④浮标吞是 egui 层级系统的既有行为,不是我们实现的选择;「穿透」需要给浮标 Sense::hover() 降级,反而让「点浮标」这个主路径变钝。
- **如何改**:①要非活动块也弹——`live.rs` 富渲染分支给块 rect 挂 `context_menu`,条目限「复制块文本/全选(激活后)/查找」,与 TextEdit 菜单分表;②要当帧直改——`editor_menu::build` 里点击直接调 `state.apply`(需把 `&mut State` 传进 UI 层,破 ADR-005 纪律,不建议);③要后台读——`edit_events::dispatch` 的 paste 分支改发 `Message::ClipboardReadRequested`,`ClipboardState::start` 加文本模式,收流侧把 `Paste(Option<String>)` 翻回合成事件;④要穿透——selection_ai 的浮标 rect 注册换 `Sense::hover()`,右键穿透到编辑器(浮标上的左键点击也一并失效,需要另行恢复)。

## #144 文件树删除守卫三口径:dirty 拒删、干净标签先关再删、移入废纸篓而非 unlink(2026-10-13,#context-menus M2·自动拍板)

- **岔路**:①删除目标有**未保存修改的打开标签**时删不删;②目标(及子树)的**干净打开标签**怎么处理;③删除语义 unlink 还是移入系统废纸篓。
- **备选**:①照删(标签悬空)/拒绝(提示先保存或关闭)/自动保存后删;②标签留着(路径失效)/先关闭再删;③unlink(直删,rm 语义)/废纸篓(trash crate,可恢复)。
- **自动选择**:①**拒绝**——删掉磁盘文件而留下脏缓冲,之后一次 Ctrl+S 会把已删文件原样写回,「删除」被保存动作静默撤销,比拒绝更危险;自动保存后删把「保存」和「删除」两个动作搅在一起,用户无法预期。②**先关闭再删**——标签留着会指向不存在的路径,状态栏/标题都在说谎;关闭走 `remove_tab` 既有通路(AI 在途流作废保护一并继承)。③**废纸篓**(trash 5.2.9)——无签名产品的用户容错底线:撤销不了的就少做;确认对话框(复用 #186 destructive_confirm_dialog)已是一道闸,废纸篓是第二道,unlink 一道闸都不留。
- **理由**:①dirty 拒绝守的是「保存复活已删文件」这条最隐蔽的数据丢失路径;②remove_tab 的关闭栈还把路径记进「最近关闭」,Ctrl+Shift+T 能立刻找回(若还没真删);③trash 的三平台实现(freedesktop Trash 规范/macOS NSScriptingTrash/Windows SHFileOperation)各自走系统标准位置,用户从废纸篓找回不需要懂 LaterMD。
- **如何改**:①要照删——`confirm_delete_with` 删掉 dirty_open 分支,注意标签悬空后保存会重建文件,建议同时把该标签标记为「外部删除恢复」态;②要留标签——删掉关闭循环,标签路径失效后靠既有孤儿 draft 检测兜底;③要 unlink——`trash_delete` 换 `std::fs::remove_dir_all/remove_file`,确认浮窗文案同步改「不可恢复」。

## #145 合成编辑事件与命令快捷键共存的已知边界:用户把命令绑到 Cmd/Ctrl+Z 或 A 时右键动作连带触发(2026-10-13,#context-menus M1·自动拍板)

- **岔路**:撤销/重做/全选以合成**按键事件**(Cmd/Ctrl+Z、Cmd/Ctrl+A)进入输入流,`poll_shortcuts` 同帧也会看到它们——出厂键位表没有命令绑这两键,默认零副作用;但键位表用户可改,绑了就会连带触发。
- **备选**:①接受(登记已知边界);②合成事件打标记跳过 poll_shortcuts(egui 无公开 API 给单条事件打私有标记,须自建旁路通道);③撤销/重做/全选改走非事件直改(TextEdit undoer 的私有 API 不可从外部驱动,变复杂度最高)。
- **自动选择**:①**接受并登记**——触发条件苛刻(用户主动把命令绑到 Cmd/Ctrl+Z/A 这两个编辑器保留键),触发后果是多执行一条用户自己绑的命令,不丢数据;规避(②)的成本是一条贯穿 input-事件-快捷键的旁路机制,收益与成本完全不成比例。
- **理由**:Cmd/Ctrl+Z 与 Cmd/Ctrl+A 在任何编辑器里都是保留键,「绑定冲突」在 `Keymap` 侧本就有拒绝逻辑的讨论(裸字母拒绑,但 Cmd+Z 不在拒绝清单);真要防,正确层位是 keymap 的冲突审计把「编辑器保留键」列入拒绝清单,而不是在事件流里打标记。
- **如何改**:要根治——`keymap::assign_shortcut` 增加保留键清单(Cmd/Ctrl+Z、A、C、V、X)拒绝绑定为命令快捷键(注意先审出厂表确认无命令占用这五键,再动拒绝逻辑);要只解本边界——无,①即现状。

## #146 本地文档链接四口径:解析基底文档目录优先、锚点与 TOC 同 slug 规则、命中路径 canonicalize、Live 链接点击不连带进编辑(2026-10-14,#doclinks·自动拍板)

- **岔路**:①普通 Markdown 链接指向本地文档时按什么基底解析(当前文档目录/文档库根);②`#锚点` 与标题怎么对上(slug/纯文本/都要);③解析命中的路径要不要 canonicalize;④Live 富渲染块里点链接,除了跳转要不要连带把块切进编辑。
- **备选**:①只认文档目录(标准语义,库根写法失效)/只认库根(wikilink 式,`./` 语义失效)/文档目录→库根→全库按名三级兜底;②只 slug(手抄原文对不上)/只纯文本(TOC 链接对不上)/slug 优先+纯文本忽略大小写兜底;③原样返回(`./`、`../` 拼接形态进 find_by_path,已开标签去重失效,点链接开出第二个标签)/canonicalize(一次 syscall,符号链接库的路径形态漂移);④只挡外链(靠帧输出 OpenUrl 探测,应用内消息看不见)/按 handler 拦截标志挡(消息与 OpenUrl 一视同仁)。
- **自动选择**:①**文档目录→库根→全库按名三级兜底**——相对路径的 Markdown 标准语义是文档目录(和图片 `resolve_relative_images` 同一参照系),库根相对与裸文件名是知识库习惯写法,三级各管一段互不越权;扩展名补齐(`.md`/`.markdown`)与 `filetree::find_by_name` 同口径,百分号解码只作原文未命中的补位(浏览器习惯 `%20`)。②**slug 优先+纯文本兜底**——应用自己生成的 TOC(`generate_toc` → `#锚点`)必须点得回,这是本功能的第一用户,slug 规则与 TOC/导出 HTML 同一条(`heading_slug`+`Slugger` 序贯去重,`x`→`x-1` 对得上);手抄标题原文的写法用忽略大小写的纯文本匹配接住,两规则按文档序首个命中。③**canonicalize**(Windows `\\?\` verbatim 前缀剥除)——去重的收益是硬的:点击 `../top.md` 时已开的 top.md 标签要被认出来激活而不是开第二个;符号链接文档库下 canonical 形态与文件树展示路径可能不同形导致去重漏,属罕见部署,不为它把 canonical 键位引进共享的 `find_by_path`。④**按 handler 拦截标志挡**——`clicked_for_edit` 的既有护栏是「帧输出含 OpenUrl 就不进编辑」,应用内跳转只发 `Message::DocLinkClicked` 无 OpenUrl,护栏天然看不见;drain 前取「本块 handler 拦了链接点击」的布尔传进去,外链(OpenUrl)与应用内消息两条路都挡住,顺带修掉 Live 里 wikilink 点击误切编辑的既有隐患(此前只靠 OpenUrl 探测,wikilink 同样会连带进编辑)。
- **理由**:①三级兜底的每一段都对应一种用户已有心智:相对路径来自「文档和它的附件住一起」(图片段已确立),库根路径来自 wikilink 的 `子目录/笔记` 写法,裸文件名来自 `[[..]]` 的按名习惯;解析是点击时的直取+两次兜底扫描,无常驻索引,与 wikilink 同一成本口径。②锚点匹配的判据不是「我们喜欢哪种」而是「应用自己已经产出了哪种」——TOC 生成的链接点不回对应标题是硬缺陷;slug 分配与消费共用 `Slugger`/`heading_slug` 两个纯函数,不存在两套口径漂移的空间。③不 canonicalize 的后果(重复标签)每次点击都会撞上,canonicalize 的代价(罕见部署下的形态漂移)只在符号链接库出现;收益代价不对称。④护栏和被护栏的对象必须看同一信号——OpenUrl 探测当初就是为「点链接别进编辑」设的,应用内跳转引入了第二种链接消费信号,护栏跟着扩一份,而不是让两种信号打架。
- **如何改**:①要只认文档目录——`doclink::resolve` 删掉 root 基底与 find_by_name 兜底两段,对应测试(root 相对、按名兜底)同步删除;②要只 slug——`find_anchor` 删纯文本分支,`#My Head` 这类手抄锚点将失效;③要不 canonicalize——`canonical_found` 换 `to_path_buf`,注意 `./正文.md` 链接在标签去重上将开出第二标签(`resolve_prefers_document_relative_then_root` 的 `./` 断言同步);④要恢复连带进编辑——`clicked_for_edit` 删 `link_intercepted` 参数与 live.rs drain 前的标志捕获,外链仍受 OpenUrl 探测保护,wikilink/本地链接恢复「跳转+进编辑」双动作。

## #147 D1 AI 卡开关:右栏退役后 AI 卡以 `RichLinkHandler`+`with_ai` 显式开关落进 Live,推翻 #51 M3 岔路②的「不复用」但延续其实质(live-unify-plan P2·2026-10-15,计划内拍板)

- **岔路**:右栏对照预览退役(live-unify-plan P4)后,AI 指令卡与 `ai://` 拦截(AiLinkHandler,原纯右栏能力)的宿主去哪:①Live 富渲染块直接复用 `AiLinkHandler`;②保持不复用,AI 卡随右栏一起退役(AI 卡在写作模式下从此不可用);③拆两层 —— 基础 `RichLinkHandler`(wiki/hl/emoji/task/mermaid + doclink + 菜单,两栏同一套)加 `with_ai` 显式开关(AI 卡渲染与 `ai://` 拦截),Live 侧显式开启。
- **备选**:①照搬(最少代码,但 #51 M3 岔路②的否决线是「不能借 mermaid 顺手改变 ai 卡/emoji/wikilink 行为」,照搬=无声漂移);②退役(功能回退,写作模式下 AI 指令块退化成普通代码块,`ai://` 链接点击开浏览器——两处都是用户可见的倒退);③显式开关(代码量居中,行为变化全部显式)。
- **选择**:③拆两层。预览恒开 `with_ai`(行为零变化);Live 侧 P2 起构造 `RichLinkHandler` 并显式 `with_ai`——```ai 围栏出卡、`ai://` 链接紫罗兰样式且点击发 `AiLinkClicked`,emoji/wikilink/hl/task/mermaid 各臂与预览逐字节同一实现。同时 `is_openable_href` 随迁 `ui::ai_card`(P0 遗留口子收口),`LiveMermaidHandler` 退役(生产侧死代码,mermaid 测试改用关闭 AI 的 `RichLinkHandler`,mermaid 分派等价)。
- **理由**:②违背本重构的第一性目标(能力并入 Live,不是删能力);①与③的渲染结果完全一致,差别只在「变化是否被显式表达」——#51 M3 的否决线针对的是「借 mermaid 顺手夹带」,现在 AI 卡进 Live 本身就是本阶段的显式目标,经计划文档、PR 与本条三处留痕,原裁决的实质(不能无声改变)由开关语义延续;实现上两层拆分让「Live 与预览同一套」成为类型系统事实(同一结构体),而不是两份实现靠测试对齐。
- **如何改**:要回退 Live 的 AI 卡——`live.rs` 构造处删掉 `.with_ai(..)` 一行,```ai 围栏回落普通代码块、`ai://` 走 vendored 默认(守门测试 `live_ai_block_renders_card`/`live_ai_link_click_sends_message` 当场红);要连预览一起退——`preview.rs` 同样删 `with_ai`,AI 卡功能整体退役(等于推翻 live-unify-plan 前提,须回到计划层讨论)。

## #148 D2 浏览态跳转语义:「锁定浏览」硬锁的新字段而非扩展 browse_only(live-unify-plan P3·2026-10-15 记账)

- **岔路**:禅定改走 Live 浏览态后,大纲/Ctrl+G 的跳转(`cursor.jump_to`)在浏览态下该「只滚动」还是仍按写作语义「落点并进编辑」。触发点:计划文档 D2 原提议「给 LiveState 加一条只滚语义」;而写作浏览态(打开文档未双击)的既有契约是「跳转即进编辑」(#60 goto / #42 大纲的实测用例钉住)。
- **备选**:①以 `browse_only` 为准 —— 浏览态一律只滚;②新加 `browse_locked` 字段,只有禅定(强制阅读场景)置位;③禅定帧同样允许进编辑(当前行为即此)。
- **选择**:②新字段 `browse_locked`。禅定帧每帧置 `browse_only=true` + `browse_enabled=false` + `browse_locked=true`(不写回 State 持久化);`jump_to` 在锁定时只置 `pending_caret` 做块级滚动,在写作浏览态维持既有「落点+进编辑」。③被否:禅定是**阅读**场景,点提纲被拽进一排源码 TextEdit 是明确倒退。①被否:会让既有 goto/大纲测试与「打开文档 → 跳转即可写」的实际用法退步。
- **理由**:`browse_only` 的语义是「还没开始写」的软状态(任何写入意图都能翻转它),`browse_locked` 是「这个视图不提供写入」的硬约束 —— 两者不是同一维度的取值,塞进一个枚举会让 `sync`/`reset`/`activate` 每处都要重判;分开后顺手解决了禅定的兄弟问题(帧垃圾回收:cursor.byte / active / pending 一律清空,不与 State 交互)。
- **关联修正(同一轮实测)**:禅定 CentralPanel 的 Live `ScrollArea`(`auto_shrink([false,false])`)会把父 Ui 的矩形向下撑约 2px,导致其后 `ui::zen_nav` 读到的根 Ui `max_rect()` 越过窗口底缘 —— 两条既有禅定测试(`zen_hover_nav_*`)因此几何漂移而红。处置:正文改走固定矩形的 `new_child`(高度增长留在子 Ui 内,不向上传播),横向限宽仍靠 `vertical_centered` + `set_max_width`。
- **如何改**:要让禅定也能进编辑 —— 删 `draw_zen` 的 `tab.live.browse_locked = true;`(连同 R3 守门 `zen_frame_never_mutates_*` 里「不产活动块」那条断言);要让 goto 在禅定里也落点 —— 在 `live::ui` 的 `jump_to` 分支把 `browse_locked` 与 `browse_only` 合并判定。

## #149 三态互斥:右栏的开合就是 `render_mode` 本身,不再有独立开合 bool 与「离开写作时右栏该不该回来」的快照补工(live-unify P4 + 三态回归 #250/#252,2026-10-15)

- **岔路**:右栏对照列回归后,它的开合状态挂在哪 —— ①回到 live-unify P4 删掉的那套(`layout.right: bool` + `right_width` + `pre_live_right` 快照);②并入 `render_mode` 成为第三档(`RenderMode::Split`),三档互斥;③保留一个「上次的右栏开关」只用于写作档回来时还原。
- **备选**:①旧三件套(Live 与右栏可同时存在,于是必须有「进写作强制收右栏」和「切回源码恢复上次开关」两条补工 —— 这正是 P4 净删 945 行的主体);②三态互斥;③中间态。
- **选择**:②。`RenderMode::{Source, Live, Split}` + `cycle()` 一条循环;持久化只有 `layout.render_mode: Option<RenderMode>` 一个字段(宽度另存 `live_right_width`,新名天然零迁移)。删掉的有:`layout.right` / `right_width` / `pre_live_right`、`Command::ToggleRightPreview`(`Command::ALL` 44→43)、`Message::RightPanelToggled`、`State::toggle_right_panel`、`TitleButton::PanelRight`(标题栏按钮 7→6)、`ui::preview` 整模块。
- **理由**:「写作与右栏同时存在」这件事本身没有用户价值 —— 两者从不同屏,写的时候不看对照、看对照的时候不写。①的全部复杂度都来自为这个不存在的组合做快照补工。②把「右栏开不开」降维成「当前是第几档」,补工整条消失;代价是「从分栏回写作再回分栏」不记忆中间态,而这个序列本来就是连点循环开关的自然结果。
- **如何改**:要恢复「写作档同时开着右栏」——`RenderMode` 加回一个独立 `compare: bool` 字段并重建快照补工(不建议,等于推翻 live-unify 的第一性动因);只想调循环顺序 —— 改 `RenderMode::cycle()` 的三行 match 与标题栏三段数组的顺序(两处必须同步,顺序即循环序)。

## #150 分栏右栏只读:每帧置位的 UI 侧硬锁(`browse_locked`),不写回 State(#250/#252,2026-10-15,与 #148 同族)

- **岔路**:右栏用哪份状态渲染、怎么保证它只读 —— ①复用写作档那份 `TabState::live`(省一份状态,但两块实例会抢 `browse_only`/`active`/`pending_caret` 同一组槽);②独立 `TabState::live_right`,只读靠每帧置位(与禅定浏览态同款);③独立实例 + 只读做成 `live::ui` 的新参数。
- **备选**:①共用;②独立实例 + 每帧置位(本条选定);③新参数。
- **选择**:②。右栏帧内每帧置 `browse_only=true` + `browse_enabled=false` + `browse_locked=true` + `active=None`;只读列不回写共享的 `cursor` / `selection`(用局部占位接住,否则会和源码侧抢这两个每帧镜像的槽)。`ScrollArea` 的 id 与块 widget id 命名空间参数化(PR1),两块各序 0 不相撞。
- **理由**:①会真的坏 —— 右栏每帧置浏览态会把写作档正在编辑的活动块清掉(两块共用一组字段),这是「两块渲染同一份文档」的直接后果。③要给 `live::ui` 加形参而它已经 14 参;每帧置位是禅定那条路径已经验证过的手法(「硬锁不写回 State,退出即还原」),不新增机制。
- **如何改**:要让右栏可编辑 —— 删 `tab.live_right.browse_locked = true;` 一行并让它的 `cursor`/`selection` 接回真槽(注意此时与源码侧抢槽的问题必须一并解决,不是删一行就完事);要让右栏不被 git diff 审阅挤掉 —— 改 `if split && !git_diff_workspace` 的合取条件。

## #151 右栏「跟随一次」:源码侧光标移动才下滚动目标,消费后即清(#250/#252,2026-10-15)

- **岔路**:分栏档下右栏要不要持续跟随源码光标 —— ①持续跟随(每次光标动右栏就滚);②跟随一次(光标移动下一颗滚动目标,落到目标帧消费后即清,此后右栏保留自己的滚动位置);③不跟随(右栏停在顶部,纯靠用户自己翻)。
- **备选**:①持续跟随;②跟随一次(本条选定);③不跟随。
- **选择**:②。`followed_byte` 记「上次已跟随过的光标字节」,只有它与当前 `cursor.byte` 不等时才 `sync` + 置 `pending_caret` + `caret_follow=true`;目标被消费后不再重下,直到光标再次移动。
- **理由**:①会让右栏在编辑时不停跳动,而「对照」的用途恰恰是要能**停在别处**跟源码比较 —— 持续跟随把右栏变成源码的镜像,分栏就失去意义。③则丢失「点大纲/光标移动 → 右栏跟到同一处」这个最自然的预期。②的语义是「把两边对齐一次,然后放手」,正好是「跟随」这个词在编辑器里的通常含义(VS Code 的 markdown 预览同步同款)。
- **如何改**:要改成持续跟随 —— 删 `followed_byte` 判等那层,每帧按光标 sync;要改成不跟随 —— 把整段 `if tab.live_right.followed_byte != cursor_byte {..}` 删掉(右栏仍随文档编辑重渲染,只是不滚)。

## #152 对照列左缘分隔线回归:走 `panel_divider` 统一入口,画在全部 panel 之后(#252,2026-10-15)

- **岔路**:右栏回归后左缘那条线怎么画 —— ①用 `Panel::show_separator_line(true)`(egui 内建);②自绘,复用 `workbench::panel_divider`(侧栏/其他面板同一个入口);③`Frame::stroke`。
- **备选**:①内建;②自绘 + 统一入口(本条选定);③Frame 描边。
- **选择**:②。`show_separator_line(false)`(内建线在 hover/drag 时才出现,不是常显),改由 `panel_divider(ui, COMPARE_PANEL_ID, split)` 在**全部 panel 之后**用 `layer_painter` 单画一根线;面板 `Frame::stroke` 置 `Stroke::NONE`(它是四边描边,会把顶/右/底也描上)。Git diff 审阅帧没有对照列,跳过画线 —— 否则 `PanelState` 里残留的上一次矩形会把线画进 diff 区。
- **理由**:这条线的边界取法三段递降(拖动中取 `interact_pointer_pos()` / 空闲取 `PanelState` / 都不成不画)是 2026-10-13 坤哥「线不跟着我拉的框走」反馈后实测逼出来的,`panel_divider` 是它的唯一实现;新面板复用它等于免费继承那套结论。它画在 `layer_painter` 上,与 egui 自带 hover/drag 高亮线(画在 `parent_ui.painter()`,本层之下)**并存且不冲突** —— 后者只在交互时出现,视觉上是「常显线变粗」。
- **如何改**:要改回内建线 —— `show_separator_line(true)` 并删 `panel_divider` 调用(代价:线只在 hover/drag 时出现,不常显);要改线的粗细/颜色 —— 动 `workbench::panel_divider` 内部(注意它服务多个面板,改的是全部)。

## #153 R2 挂账:Live 块级 label 的 widget id 含块序号,敲回车分裂块即平移 → vendored 缓存 miss(后续性能项,本次不解决,2026-10-15 登记)

- **岔路**:块级渲染的 widget id 用 `("live-render", index)` 还是按块内容/块首偏移派生。前者在插入块(敲回车)时**后面所有块的序号都 +1**,id 全变,vendored 的布局/高亮缓存整片失效。
- **现状**:保持含序号的写法,不在 live-unify 内解决。删掉右栏后渲染粒度只剩块级一种,**没有第二条路径可以对照**,因此这条不影响「两栏观感不一致」这个重构目标;它只是一项独立的性能优化。
- **备选**:①按块首字节偏移派生 id(插入块不影响已有块的偏移 → 缓存保留;代价是块内编辑导致块首偏移变化仍会 miss,且块表重建时机要重新界定);②按块内容哈希(最稳,但要每块算一次哈希,且内容改动本就该 miss);③保持现状。
- **如何做**:真要动 —— 在 `live.rs` 构造 label id 处把 `index` 换成块首偏移(或内容哈希),并给「连续敲回车 N 次后首切耗时不增长」补一条 perf 性质测试(`ui/tab_switch_perf.rs` 同款 harness)。**先量化再动**:目前的 miss 代价没有被测量过,别凭直觉认定它是瓶颈。

## #154 Live 浏览态单击即进编辑:推翻「双击才进写作」的门槛(2026-10-19,坤哥拍板「改成鼠标单击,就可以在当前位置编辑」)

- **岔路**:打开文档后的浏览态(`browse_only`)进编辑的入口——①保持双击(早期口径:避免「随便点一下就进插入态」);②单击即进,光标落点击字形(#170 已有落点换算,入口只剩门槛);③浏览态保持只读,进入写作走显式按钮。
- **拍板**:②。单击与写作态的块间切换是同一条 `clicked_for_edit` 路径,双击门槛反而制造了「两种点击语义」的心智负担;点哪编哪是 MarkText 手感的底线,与用户同日提出的「写作模式跳来跳去」(#155)是同一组体验问题。
- **实现**:富块分支 `browse_only` 臂的 `double_click` 参数删除(`clicked_for_edit` 收敛为单击判定);空文档占位提示改「单击此处开始写作」(`double_clicked()` → `clicked()`)。`browse_enabled` 闸不动——禅定锁定时单击仍不进编辑(#148 硬锁语义不变)。
- **如何改**:想回退,把 live.rs 富块分支 `browse_only` 臂的入口换回 `button_double_clicked` 判定并恢复双击提示文案即可;`browse_single_click_enters_edit_at_clicked_glyph` 测试同步翻红。

## #155 Live 块切换视口锚定:点击帧预测补偿 + 落地帧残差校正,被点击行钉在原地(2026-10-19,坤哥报「每次点击更新总是跳来跳去」·自动拍板)

- **岔路**:点击富渲染块进编辑时,块从富渲染切到源码 TextEdit 高度必变,视口内内容整体平移。①接受跳变(状态 quo);②落地帧实测后补偿(实现省,但重排帧已画错一帧,肉眼可见闪跳);③点击帧同帧预测(源码 TextEdit 同参 shaping + 上一活动块富渲染估高),帧末把补偿写进 ScrollArea state,下一帧重排一次到位,落地帧再按实测光标行做残差校正(亚像素)。
- **自动选择**:③。跳变发生在用户视线聚焦的点击瞬间,闪一帧都不行;预测的误差源(估高器、滚动条出现/消失)由残差校正兜底。
- **实现**:点击行锚 = vendored 新 API `row_rect_at_pos`(#7546657,`char_index_at_pos` 的姊妹查询)取被点击渲染行屏幕顶;`LiveState::toggle_anchor` 与 pending_caret 同生共死(jump_to/路由覆盖即清,落地帧消费即清);补偿落账在 ScrollArea 收尾之后 `State::load/store`,打字机 `last_offset` 在其写回处同步加同等量(感知不到的滚动不记 Override 暂停,#121 边界类的显式豁免)。活动块源码形态实测高记账于 `("live-src-h", index)` temp,塌缩位移 = 富渲染估高 − 源码实测。
- **顺带修复(实measured bug)**:live.rs 的 ScrollArea 状态键原写法 `panel.make_persistent_id(裸 Id)` 与 ScrollArea 实际派生键(`make_persistent_id(IdSalt::new(...))`,egui 0.36 起经 `IdSalt` 哈希)**不同源**,`State::load` 恒 MISS——#111 B 的 estimate_drift 落账因此一直静默无效(占位/实测交替的漂移从未被真正补偿过,offscreen 测试只钉「稳态不漂」,恰是空转断言,漏检)。现统一 `egui::IdSalt::new(editor_id.with("live-scroll"))` 派生。
- **如何改**:嫌补偿激进可把 `predict_source_layout` 的预测臂砍掉只留残差校正(改回②,实现最省,代价是点击帧闪跳);锚定粒度想从「行」退到「块顶」,把 `row_rect_at_pos` 换 `block_rect_at_offset` 即可。

## #156 Live 跨块路由补全:四方向 + 幻影行收敛 + Shift 守卫(2026-10-19,「写作模式比较隔离」的键盘部分·自动拍板)

- **岔路**:活动块内方向键到边界的行为——①维持只有 ↑/↓ 且目标为整块字符长(块尾的空行幻影行是必经站,↓ 要按 2-3 次才出块);②↑/↓/←/→ 四方向同语义路由,块尾判定改「可见内容末尾」(`trim_end_matches('\n')` 后的字符长),按住 Shift 一律不路由。
- **自动选择**:②。幻影行粘滞是键盘「发粘」的主因;←→ 在边界不动是纯损失;Shift 守卫是因为选区活在单个 TextEdit 的持久 state 里,跨块选区不可达——宁可原地不动也不打断/清空用户正在拉的选择。
- **实现**:`visible_char_len` 帮手;路由目标块为上一块时落其可见末尾(不再是整块长);`live_arrow_keys_route_across_blocks_with_shift_guard` 钉四个方向 + Shift 守卫;`live_cross_block_route_lands_caret_and_scroll_together` 的交接点断言随语义更新(599 = 可见末尾,不再是 601 = 整块长)。
- **已知边界(如实登记)**:跨块**选区**(拖选/Shift 扩选跨块)仍不可达,要做需要文档级选区模型(块间共享锚点+逐块渲染选区),体量是一个独立战役,不在本次范围;Ctrl/Cmd+方向键(词跳/行首尾)仍在块内截断。
- **如何改**:想要 Ctrl+← 在块首路由上一块,在路由判定加 `modifiers.ctrl` 分支即可;跨块选区立项时从 `sel_ai_selection` 的文档坐标镜像起步。

## #157 活动块形态切换零展开:编辑视图裁掉幻影空行 + 行距同源(2026-10-19,坤哥复测「点击后还有一个展开的感觉」·自动拍板)

- **岔路**:#155 锚定后点击行不跳了,但块本身仍有「展开感」。剩余膨胀两个来源:①块尾幻影空行(块间空行归前一块,富渲染忽略、TextEdit 全显示 → 凭空多 1-2 行);②等宽行盒高 ≠ 预览正文行盒高。修法:①编辑视图裁剪(`BlockBuffer::visible_bytes`,裁剪是前缀截断,块内字符偏移前后同义,编辑换算零改动);②TextEdit 拨 `extra_text_line_spacing = body_line_height − mono_row_height` 与预览逐行等高。备选「块表改分块规则让空行不归前一块」否决 —— 破坏「覆盖全文」不变量(块外字节编辑静默丢失)。
- **两条延伸规则保住编辑语义**:①光标所在行可见(光标进幻影区时视图跟到光标行);②块尾 Enter 当帧预先多给一个换行 —— 插入的换行必须当场可见,否则 egui 把新光标钳回内容末,新行落进不可见区,接着敲的字把新段落吞回块尾(实测踩过:`live_enter_at_block_end_reveals_new_line` 钉住)。
- **效果**:无行内标记的普通段落,富渲染 ↔ 源码切形态块高残差只剩 TextEdit 上下内距 4px(`live_plain_paragraph_toggle_keeps_block_height` 容差 5px 钉住);行内标记段落与标题仍有固有差(标记字符/字号缩放),由 #155 锚定兜底。
- **已知边界(如实)**:块尾幻影行在裁剪后不可达,Live 模式下**无法用方向键+退格删除块间空行**(合并段落)——请切源码模式操作;后续若要补,给块尾 Delete/Backspace 接跨块删除是独立小战役。
- **如何改**:嫌裁剪激进,把 `visible_bytes` 恒取 `range.len()`(全块)即回退;行距同源不要,把 `extra_text_line_spacing` 拨动删掉即可,锚定路径不受影响。

## #158 渲染形态原地编辑:活动块自定义排版(标题放大加粗、正文=预览字体)+ 类型徽标(2026-10-19,坤哥发 Notion 截图拍板「不改变文字大小,原地编辑,有特殊功能地方比如 H1 双击时候显示」)

- **岔路**:活动块编辑视图的排版——①维持等宽源码形态(状态 quo,点击进标题「缩成小字」);②Notion 式渲染形态编辑:文本与缓冲不变,自定义 `TextEdit::layouter` 让活动块按渲染排版逐行排版 —— 标题行按 vendored 同式放大加粗(字号 × `heading.scales`、`bold` 族、strong 色文案、行高按放大后字号重推导),ATX 前缀 `# ` 淡化为弱色正文小字,正文行 = 预览正文字体(`preview_body_family`);③真 WYSIWYG(已否决,non-goals 不翻案)。
- **自动选择**:②。egui `TextEdit` 原生支持自定义 layouter,文本/光标/选区/IME 通路零改动 —— 拿到②的观感而不碰③的工程深渊;Widget 块(代码/表格/引用/分隔线/图片)保持等宽回退(对齐代码列与表格列仍是刚需)。
- **类型徽标**:H1–H6/代码/表格/引用/分隔线/图片,编辑时悬在块顶的块间空隙(首块钳回视口顶),纯绘制不参与命中;普通段落不标。前缀判定与 `block_kind` 同一套规则。
- **实现要点**:①排版作业构造(`active_block_job`)与视口锚定预测(`predict_source_layout`)**共用同一个函数** —— 预测即真相,锚定精度不随排版改变回退(标题块塌缩位移因此≈0);②**作业构造必须在 `fonts_mut` 闭包外求值**:内部取 context 写锁(`ctx.fonts` = `self.write()`),嵌套进 `fonts_mut` 即自死锁(49ef922 记过的同类陷阱,本轮测试当场抓到);③`.layouter(&mut 闭包字面量)` 的临时值撑不过语句,闭包须先绑定。
- **已知边界(如实)**:行内 `**` 等标记仍是字符(遮罩层淡化,LP2-1 不变),不做隐藏;引用/表格等 Widget 块仍是等宽(富化另立批次);徽标在上一块是元件时可能与其底部重叠约 7px(半透明底色,观感可接受);TextEdit 光标条高度用统一行高,标题行上光标条略短于行盒。
- **如何改**:不要渲染形态、回等宽源码 —— 删掉 `.layouter(&mut layouter)` 接线即可,徽标与锚定不受影响;徽标嫌吵,删 `block_badge_label` 挂点即可。

## #159 ATX 前缀改幽灵语法:标题同字号 + 文本色 43% 透明度(2026-10-19,坤哥复测「H1 没显示 #号,双击也不能让#可编辑,修改成H2这种」·自动拍板)

- **岔路**:#158 的前缀弱色正文小字实际不可见(weak 色太暗太小),用户以为 # 没显示、不可编辑。①维持弱色小字;②幽灵语法:前缀与标题**同字号**,文本色降透明度(~43%,alpha 110)—— Obsidian/MarkText 观感,既是明显可编辑的语法,点击目标也够大;③隐藏前缀靠徽标交互改级别(徽标点击弹级别菜单,另立批次)。
- **自动选择**:②。改级别 = 光标放块首敲/删 `#`,源码真实改写、徽标即时跟随(`live_heading_marker_ghost_visible_and_level_editable` 端到端钉住:H1 块首敲 `#` → 源码 `##`、徽标 H2);行高仍用标题行盒,块高零变化。
- **如何改**:嫌幽灵色太显/太淡,调 `active_block_job` 里 `ghost` 的 alpha(现 110);要徽标点击改级别,给徽标挂弹层走 `clicked_for_edit` 的探针排除路径即可(登记为候选后续)。

## #160 macOS 间歇崩溃止血:只在 macOS 关掉 eframe 的 accesskit(上游 AccessKit/accesskit#823;2026-10-23 真机崩溃反馈·自动拍板)

- **岔路**:真机崩溃(2026-10-23 22:49,macOS 15.3.1 / MacBookPro15,4 / LaterMD 0.0.8,启动约 3 分钟后主线程 `EXC_BAD_INSTRUCTION (SIGILL)`),栈是
  `-[_NSTouchBarFinderObservation invalidate]` → `removeObserver:forKeyPath:` → `NSDisplayCycleFlush` → `-[NSApplication run]`。
  根因在上游 **AccessKit/accesskit#823**:`accesskit_macos` 的 `SubclassingAdapter` 用 `object_setClass` 换掉 winit 的 view 类、drop 时再换回来,而 AppKit 期间已对 `nextResponder`/`touchBar` 注册 KVO(类被换成 `NSKVONotifying_*`),换回之后观察者仍在册,下一次 `removeObserver` 抛 NSRangeException ⇒ AppKit 捕获不到直接终止进程。我们锁的 `accesskit_macos 0.26.3` 正含此缺陷。
  四条路:①**全平台关 accesskit**;②**只在 macOS 关**;③vendor/fork 一份 `accesskit_macos` 把 #835 移植到 0.26.x;④升 eframe 0.37 等上游发版。
- **自动选择**:②。`crates/latermd-app/Cargo.toml` 里 `eframe` 拆成两个 `[target.*.dependencies]` 段 —— macOS 段 `default-features = false` + 逐项列出 default 里除 accesskit 之外的六项,非 macOS 段保持 `eframe = "0.36.2"`(默认 features 全开)。`Cargo.lock` 零变化。
- **为什么不取另三条**:①accesskit 在 Windows/Linux 走 UIA / AT-SPI,**没有** macOS 这个缺陷,跟着一起关等于白丢两个平台的屏幕阅读器支持。③#835 的修复(2026-10-23 merge)截至 2026-10-24 **尚未发版**(crates.io 上 `accesskit_macos` 最新仍是 0.27.1 / 2026-09-25),而 eframe 0.36.2 钉的是 `accesskit_winit 0.32 → accesskit_macos ^0.26`,即使发 0.27.2 也升不上去;fork 一份 0.26.4 要常驻维护。④eframe 0.36→0.37 是 egui 大版本迁移,为一个崩溃点不值得,且同样等上游。
- **验证**:`cargo tree -p latermd-app -f "{p} {f}"` 三目标对比 —— `aarch64-apple-darwin` / `x86_64-apple-darwin` 树里 eframe features 为 `default_fonts,links,wayland,web_screen_reader,wgpu,x11`(无 accesskit、无 accesskit_winit/accesskit_macos),`x86_64-unknown-linux-gnu` / `x86_64-pc-windows-msvc` 仍带 `accesskit`(+ `accesskit_unix` / `accesskit_windows`)。本机 `cargo check -p latermd-app --all-targets` 与 `--features glow` 两条产线均 rc=0。**macOS 真机待复验**(本机 Linux 编不到:交叉 `cargo check --target x86_64-apple-darwin` 卡在 libz-sys 需要 macOS C 工具链)。
  新增守门 `crates/latermd-app/src/macos_feature_gate.rs`(`#[cfg(test)]`,跑在所有平台因为它守的正是本机编不到的那段)三条:macOS 段不许出现 accesskit 且六项 feature 齐全、非 macOS 段不许 `default-features = false`、两段 eframe 版本一致。**红绿已实跑**:把 accesskit 塞回 macOS 段、删掉 macOS 段的 `default-features = false`、给非 macOS 段套上同款清单,三条各自当场 FAILED,还原后全绿(3 passed)。
- **遗留**:`winit/default` 无法照抄进依赖声明(Cargo 报 "not allowed to contain slashes"),缺的是 `wayland-dlopen` / `wayland-csd-adwaita` 两项 —— 均为 Linux 后端,macOS 走 AppKit 不受影响;`rwh_06` 由 eframe 自己声明 winit 时直接开启,也不受影响。
- **如何改**:上游发布含 #835 的 `accesskit_macos` 且 eframe 跟版之后,把 macOS 段还原成 `eframe = "0.36.2"`(删掉整段分档、把 eframe 放回 `[dependencies]`),并删除 `macos_feature_gate.rs` 与 main.rs 里对应 `mod` 一行。代价是 macOS 上 VoiceOver 读不到界面内容,属已知取舍,不是 bug。

## #161 命令行入口三平台:mac 应用内菜单装软链、Windows 安装器 PATH 任务、Linux 已由 deb 覆盖(2026-10-23 mac 反馈 #3·自动拍板)

- **岔路**:反馈「同时注册命令行 latermd 命令入口,可以直接 latermd xxx.md」。argv 解析与单实例移交早已在库(`cli.rs`,「双击 .md」三段链路),缺的只是各平台命令进 PATH。**Linux**:deb 装二进制到 `/usr/bin/latermd`(默认 PATH),已开箱即用,零改动。**macOS**:①由谁装 —— dmg 拖装没有安装脚本,要么应用内做(VS Code「Install 'code' command in PATH」式),要么让用户手敲;②装到哪 —— `/usr/local/bin`(全 mac 默认 PATH 含它,写它要管理员)/ `/opt/homebrew/bin`(brew 机免 sudo,但仅 brew 用户有)/ `~/.local/bin`(多数 mac 不在 PATH);③brew 渠道要不要单独给(cask `binary` stanza 由 brew 建链,免授权、卸载自动清)。**Windows**:④安装器装在 `{localappdata}\Programs\LaterMD`,PATH 注册由 `[Registry]` 整值覆盖还是 `[Code]` 读-改-写;⑤做成必做还是可选任务。
- **自动选择**:macOS ①应用内(mac 原生应用菜单「安装命令行工具…」,后台线程 `osascript … with administrator privileges` 软链到 /usr/local/bin,授权弹窗不卡 UI,与剪贴板图片读取同款 channel/repaint,结果落提示行)+ ②`/usr/local/bin`(`mkdir -p` 兜底;`ln -sf` 覆盖式幂等)+ ③cask `binary` 一并给(tap 仓另改)。Windows ④`[Code]` 读-改-写(`[Registry]` 整值覆盖会把用户已有 PATH 焊死),写回用 `RegWriteExPandedValue` 保 `REG_EXPAND_SZ` 类型,条目按分号切分精确比较去重;⑤默认勾选的 `addpath` 任务 + `ChangesEnvironment=yes` 广播 + 卸载时移除本应用条目(PATH 清空则删值不删键);全走 HKCU 不碰 HKLM,与按用户安装(mdassoc 同口径)一致。
- **理由**:Linux 无需动作;macOS dmg 是主渠道,点一次菜单比手敲 `ln -s` 门槛低一个量级,授权只提一次,升级重装后 .app 路径不变软链不用重建;`/usr/local/bin` 是唯一「所有 mac 都在 PATH」的通用落点,brew 用户由 ③覆盖得更干净(两条通道都指向同一二进制,`ln -sf` 谁后执行谁生效,不冲突);Windows 按「整条目精确比较」去重而非子串判断(`D:\Tools` 匹配不了 `D:\Tools2`),REG_EXPAND_SZ 不保会把用户 Path 里的 `%USERPROFILE%` 焊死;可选任务让用户能拒绝(拒绝后重装勾回即可,不必手改注册表)。
- **口径**:应用内通道(macos)开发裸跑(非 `.app`)只落提示不安装 —— 装出去的软链指向 `target/debug` 临时二进制,重编译即失效(`is_bundled_macos_app` 判定 `*.app/Contents/MacOS` 三段连续);用户取消授权(osascript 报 `User canceled`)翻译成「已取消授权,未安装」,其余 stderr 原样透传;路径过两层引号(shell 单引号 + AppleScript 双引号)各为独立纯函数逐字钉死。Windows 侧 zip/裸 exe 分发形态没有安装器,PATH 由用户自理(README 口径,不做应用内写注册表 —— 安装器已有勾选项,双通道重复且写 HKCU 该走系统惯例)。装之前就开着的终端 Windows 与 mac 都不热更新环境,重开终端即认(广播已尽人事)。
- **如何改**:mac 要换落点改 `cli_install.rs` 的 `LINK_DIR`(同步改成功提示文案与本条);要去应用内通道删 `mac_menu.rs` 的 `InstallCli` 项与 `state.rs` 的两条消息;要撤 brew 通道删 tap 仓库 `Casks/latermd.rb` 的 `binary` 行。Windows 要撤 PATH 注册删 `.iss` 的 `addpath` 任务与 `[Code]` 段(守门 `windows_inno_setup_adds_to_path_via_code_section` 会红,连带更新);将来若做正式安装器(pkg/msi),各平台通道都可退役。
- **验证**:纯函数(两层转义/命令拼装/bundle 判定/`run_command` 三分支/取消翻译)与状态机生命周期(恰一条结果/断连兜底/收尾)全平台无头测试,归约侧(裸二进制拒绝/收尾双向落提示)无头测试;Windows 侧 `.iss` 文本层守门(`packaging_assoc.rs`,本机无 iscc,与文件关联守门同理由):ChangesEnvironment/addpath 默认勾选/读-改-写/REG_EXPAND_SZ/卸载清理/不碰 HKLM 六判据;Linux 侧 `/usr/bin` 落点由 linux-deb.yml 产线钉住。**真机留人工**(blocked_external):mac 授权弹窗与软链生效、Windows 安装器勾选后新终端 `latermd 文档.md` 直开与卸载清理。
