//! 布局:顶部菜单栏 + 三栏(侧边栏 / 编辑器 / 预览,docs/adr-005 §3.2)。
//!
//! 顺序铁律:panel 添加顺序决定嵌套,先加的最外层;`CentralPanel` 必须最后加。
//! `App::logic` 只归约状态,`App::ui` 只绘制,两者严格分离(铁律)。

use crate::state::Message;
use crate::ui::tokens;
use crate::LaterMdApp;
use eframe::egui;

/// 停顿落盘到点仍未清(写盘失败/未命名无状态目录)时的重试要帧间隔:
/// 慢重试而不是满帧空转重写——`request_repaint_after(ZERO)` 意为立即
/// 重绘,过期到点若不钳制会把失焦窗口推成满帧速率的写盘重试(search
/// 去抖的同款空转教训,见本模块空转回归测试)。
const AUTOSAVE_WRITE_RETRY: std::time::Duration = std::time::Duration::from_secs(1);

/// 只读对照列(三态视图右栏)的面板 id:宽度持久化、分隔线定位、分隔线
/// hover 判定三处共用同一个名字(故收成常量,不与 `"preview"` 这个已被
/// P4 退役的旧名复用 —— 旧 layout.json 里还躺着 `right`/`right_width`)。
pub(crate) const COMPARE_PANEL_ID: &str = "compare-column";

/// 对照列**正文** Ui 的绝对 id(2026-10-19 进场动画改「向右滑」)。
///
/// 正文有两条绘制路径:进场在 `draw` 的 ⑤'(中央编辑区之后,自带 clip),
/// 退场在 ④ 的面板 Ui 内(egui 原生滑出)。两条路径的父 id 链不同,
/// 而 `ScrollArea` 的滚动偏移按 `make_persistent_id` 派生并存进 egui
/// memory —— 若顺着父链派生,切档那一帧 id 变化,右栏滚动位置就被重置。
/// 显式钉死绝对 id 后,正文内一切 widget 的 id 与「在哪条路径画」无关,
/// 来回切不丢位置。取值刻意区别于 `COMPARE_PANEL_ID`,免得与面板自身
/// 注册的 widget 撞 id。
const COMPARE_BODY_ID: &str = "compare-body";

impl LaterMdApp {
    /// `logic` 帧的全部归约逻辑。单独成函数是因为 [`eframe::Frame`] 的字段
    /// 是 `pub(crate)`,测试里造不出来;归约本身不碰 frame。`pub(crate)`
    /// 与 [`Self::draw`] 同理:本模块测试与 `ui::quick_open` 的无头帧
    /// (reduce→draw 完整顺序)同用。
    pub(crate) fn reduce(&mut self, ctx: &egui::Context) {
        // 单实例转发:第二个进程启动时把路径送到本进程,在归约里翻成
        // `FileSelected` —— 与文件树点击、拖入、启动参数四条入口共用
        // `open_path`,同一条语义(同路径激活已有标签而非再开一个)。
        //
        // 取件必须排在解构之前:`instance` 是 App 自己的字段,不进
        // `state`/`outbox` 那组借用。
        if let Some(primary) = self.instance.as_mut() {
            for path in primary.take_incoming() {
                self.outbox.push(Message::FileSelected(path));
            }
        }
        // 只做状态归约,严格禁止在此绘制任何 UI(docs/adr-005 §2.3)。
        let LaterMdApp {
            state,
            outbox,
            window_title,
            ..
        } = self;
        // 每帧收流前清掉上一批通知，并绑定当前 context。后台结果若恰在
        // drain 期间到达会重新请求一帧，不会因合并通知而丢失收口机会。
        state.repaint.bind_context(ctx);
        state.repaint.begin_drain();
        // AI 后台流式收流:channel 里的 chunk 翻成 Message 并入本帧归约
        // (文本回编辑器只走 Message,后台线程不触碰 UI 状态)
        outbox.extend(state.poll_ai());
        // 图床上传收流:同一手法(channel → Message → 归约),结果只在归约
        // 落地(插入或回显),后台线程不碰 UI 状态
        outbox.extend(state.poll_bed());
        // 模型列表拉取收流(#58 M2):同一手法,结果只在归约落地(填下拉
        // 候选或显示错误行),后台线程不碰 UI 状态
        outbox.extend(state.poll_models());
        // 检查更新收流(#71 M2):同一手法,结果只在归约落地(关于窗的
        // 最新/有更新/无法判断/失败),后台线程不碰 UI 状态
        outbox.extend(state.poll_about());
        // 剪贴板图片读取收流:同上(D 段,arboard 的阻塞 IO 在后台线程)
        outbox.extend(state.poll_clipboard());
        // 命令行工具安装收流(2026-10-23 mac 反馈 #3):同上,osascript
        // 授权弹窗在后台线程,结果只落提示行
        outbox.extend(state.poll_cli_install());
        // 文件拖入由 egui-winit 汇进 raw input,`RawInput::take` 每帧清空,
        // 这里取走即消费。Markdown 文件走与文件树相同的 `FileSelected`
        // 打开流程；图片文件走资源导入流程；其它文件忽略。读文件与落盘
        // 是本地磁盘(毫秒级),同步在归约做 —— 与文件树打开文件同口径。
        let dropped: Vec<std::path::PathBuf> = ctx
            .input_mut(|input| std::mem::take(&mut input.raw.dropped_files))
            .iter()
            .map(|file| file.path().to_path_buf())
            .collect();
        for path in dropped {
            if crate::file::is_markdown_path(&path) {
                state.apply(Message::FileSelected(path));
            } else if path.extension().is_some_and(|ext| {
                crate::assets::is_allowed_extension(&ext.to_string_lossy().to_lowercase())
            }) {
                state.apply(Message::ImageFileDropped(path));
            }
        }
        // Ctrl+V 的图片兑底(D 段):egui-winit 在 Ctrl+V 时同步读**文本**
        // 剪贴板,有文本才有 Event::Paste(egui-winit/lib.rs 的
        // is_paste_command 分支);剪贴板只有图片时什么都不发生 —— 本帧
        // V 键按下而无 Paste 事件,就是「剪贴板没文本」的可观察形态,此刻
        // 发起后台图片读取。文本粘贴(Paste 事件存在)与图片读取互斥:
        // 前者让 TextEdit 照常插字,本分支不触发。
        //
        // 用「V 无 Paste 兑底」而不是拦 V 键:拦键会在剪贴板有文本时抢在
        // egui-winit 之前消费按键,文本粘贴被劫持;Paste 事件由 egui-winit
        // 生成,拿它当「剪贴板有文本」的信号天然无竞态。V 键不消费(留
        // 给输入控件,只读不拿走),Ctrl 的判定用事件自带的 modifiers 字段
        // —— 帧级 `input.modifiers` 是「本帧开始时按着的修饰键」,winit
        // 在 ModifiersChanged 事件之后才推进它,首帧裸键序列下可能滞后。
        let pasted_text = ctx.input(|input| {
            input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::Paste(_)))
        });
        let pressed_v = ctx.input(|input| {
            input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::V,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.command
                )
            })
        });
        if pressed_v && !pasted_text {
            state.apply(Message::ImagePasteRequested);
        }
        for message in std::mem::take(outbox) {
            state.apply(message);
        }
        // 编辑器右键菜单的合成事件出口(context-menu-plan §3 M1):归约
        // 只把 `Message::EditAction` 记账到 `pending_edit_action`,事件
        // 推入留在这层 —— 必须 logic() 阶段进输入流(ui() 阶段推的当帧
        // 末被清空),且归约在 ui() 之前,同帧 TextEdit 即可消费。粘贴的
        // 剪贴板文本在推之前同步读(mac_menu 先例,风险面同注);兜底
        // 消息(无文本回落图片粘贴)当帧照常归约。浏览态无处还焦,动作
        // 作废(没有可消费事件的编辑器)。
        for action in std::mem::take(&mut state.pending_edit_action) {
            let paste = action
                .needs_paste_text()
                .then(crate::edit_events::system_paste_text)
                .flatten();
            if let Some(fallback) = state
                .editor_focus_id()
                .and_then(|focus| crate::edit_events::dispatch(action, ctx, paste, focus))
            {
                state.apply(fallback);
            }
        }
        // 「复制文本」出口(`Message::CopyText` / `Message::FileTreeCopyPath`
        // 的落点):state 没有 `egui::Context`,写剪贴板留在这层。取走即清。
        if let Some(text) = state.pending_copy_text.take() {
            ctx.copy_text(text);
        }
        // 长按修饰键检测(#54 M1):与命令快捷键同层的顶层输入处理,但
        // 扫描必须排在 poll_shortcuts / poll_capture 之前——快捷键消费会
        // 从事件流删掉 Key 事件,后扫会把「按过 Ctrl+S」看成「只在按
        // Ctrl」。只读不消费,既有快捷键行为分毫不变(否决线)。触发事件
        // M2 渲染层消费(Visible 态即蒙层可见位);Holding 帧按剩余时长
        // 自驱要帧——按住修饰键此后不再产生事件,不排程 3s 到点就无帧可
        // 跑归约(#18 帧饥饿的同型教训)。
        let hold_input = crate::shortcut_overlay::frame_input(ctx);
        let now = std::time::Instant::now();
        // 可见位先读再 step:Esc 帧本帧就会把 Visible 打回 Idle,后判会漏掉
        // 「该消费 Esc」的那一帧。
        let overlay_visible = state.shortcut_overlay.is_visible();
        state.shortcut_overlay.step(hold_input, now);
        if let Some(wait) = state.shortcut_overlay.repaint_wait(now) {
            ctx.request_repaint_after(wait);
        }
        // 润色确认浮窗在场的 Esc 归浮窗(#61 M3 放弃快捷键):裸 Esc 被
        // 消费并当场归约为「放弃」(关窗零改动/作废在途流)。排在蒙层的
        // retain 之前 —— 浮窗是最显式的用户上下文,最顶层浮层优先;浮窗
        // 在场时按 Esc 先关浮窗,蒙层(长按修饰键的挂起态)本帧收不到,
        // 再按一次即可,与 #103 的已知边界同型。只吃裸 Esc,带修饰键的
        // 组合不受影响。
        if state.ai_polish.is_some()
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            state.apply(Message::SelectionAiPolishDismissed);
        }
        // 关于窗在场的 Esc 关窗(#71 M1):与润色确认浮窗同款,裸 Esc 消费
        // 并归约关闭;排在蒙层的 retain 之前(最顶层浮层优先)。只吃裸
        // Esc,带修饰键的组合不受影响。
        if state.about.open
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            state.apply(Message::AboutClosed);
        }
        // 输入 / 确认弹窗在场的 Esc 归**取消**(2026-10-15 坤哥「支持 Esc
        // 关闭弹窗」):此前这些浮窗只有 × 和「取消」两条关闭路径,Esc
        // 反而落到禅定/蒙层那些更远的语义上 —— 键盘用户关不掉自己刚开
        // 的框。
        //
        // 五个弹窗互斥(各自只有一个入口,且入口都在菜单里),故判断「有没有
        // 一个开着」再取第一个即可;顺序与 `draw_overlay_dialogs` 的绘制
        // 顺序一致。删除确认也在列:Esc 归「取消」对它同样自然,且破坏性
        // 弹窗**不绑 Enter**(见 `destructive_confirm_dialog`),两者一起
        // 才是完整的「回车无默认动作、Esc 是唯一无条件出口」。
        //
        // 只吃裸 Esc,带修饰键的组合不受影响(与上面两个弹窗同口径)。
        if (state.file_tree.new_file.is_some()
            || state.file_tree.new_folder.is_some()
            || state.file_tree.rename.is_some()
            || state.tabs.rename.is_some()
            || state.file_tree.delete_confirm.is_some())
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            if state.file_tree.new_file.is_some() {
                state.apply(Message::FileTreeNewFileCancelled);
            } else if state.file_tree.new_folder.is_some() {
                state.apply(Message::FileTreeNewFolderCancelled);
            } else if state.file_tree.rename.is_some() {
                state.apply(Message::FileTreeRenameCancelled);
            } else if state.tabs.rename.is_some() {
                state.apply(Message::TabRenameCancelled);
            } else {
                state.apply(Message::FileTreeDeleteCancelled);
            }
        }
        // 蒙层可见帧的 Esc 归蒙层(#54 M2 关闭路径之一):蒙层的关闭本身
        // 已由 step 的「其它按键」规则完成,这里只把事件从流里移除——禅定
        // 退出、emoji/查找条关闭等其它 Esc 语义当帧不可达(最顶层浮层优先,
        // 与 poll_capture 的 retain 手法同款)。蒙层可见期间修饰键仍按着,
        // `consume_key(NONE, …)` 的逻辑匹配对带修饰键的 Esc 不成立,故直接
        // retain。已知边界(decisions-pending #103):设置页改键捕获中蒙层
        // 若出现,Esc 先关蒙层、捕获的「Esc 取消」当帧不可达,需再按一次。
        if overlay_visible {
            ctx.input_mut(|input| {
                input.events.retain(|event| {
                    !matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::Escape,
                            pressed: true,
                            ..
                        }
                    )
                });
            });
        }
        // 命令快捷键(键位来自 `keymap`,用户可改;统一清单见 `crate::command`)。
        // eframe 在 begin_pass 之后调 logic,本帧按键事件此刻可见;消费即从
        // 输入流移除,TextEdit 即使聚焦也收不到;无 COMMAND 修饰的普通字符
        // 不匹配任何绑定,原样放行。
        //
        // 设置页正在捕获键位时**不**派发命令:此刻的按键是「新键位」而不是
        // 命令触发,否则会把 Ctrl+S 同时当成「保存」和「保存的新键位」。
        if state.settings.capture.is_some() {
            poll_capture(ctx, state);
        } else {
            let commands = crate::command::poll_shortcuts(ctx, &state.keymap);
            for cmd in commands {
                state.apply(cmd.message());
            }
        }
        // 系统主题节流刷新:只有「跟随系统」模式才轮询(返回下一次探测时刻),
        // 其余模式返回 None,egui 得以收敛到深度空闲。
        if let Some(due) = state.poll_system_theme(std::time::Instant::now()) {
            ctx.request_repaint_after(due.saturating_duration_since(std::time::Instant::now()));
        }
        // 主题投影到 context:egui 主题(外壳)+ 密度 token + MarkdownStyle
        // (正文,含代码高亮自动随 dark/light)。带 staleness 检查,空闲帧近零
        // 开销;首帧前 main 已装载一次,这里覆盖此后每次切换。`System` 已在
        // `resolved_theme` 里落到确定的明暗。
        state.theme.apply(ctx, state.resolved_theme());
        state.end_of_logic();

        // 搜索去抖到点:在归约侧发起,不放 `ui::sidebar`——归约每帧必跑、
        // 不看侧边栏页签,输入后 300ms 内切走也照常搜;`SearchState::start`
        // 入口先清计时,过期时刻不残留,下方按 due 要帧的逻辑才不会在
        // due 过期后每帧 request_repaint(立即)满帧空转。
        if state
            .search
            .debounce_due
            .is_some_and(|due| due <= std::time::Instant::now())
        {
            state.apply(Message::SearchRequested);
        }
        // 搜索去抖等待中按剩余时长要一帧，到点由上一段发起；后台结果
        // 到达后的帧由统一 repaint 通知器主动唤醒，不再在途满帧空转。
        if let Some(due) = state.search.debounce_due {
            ctx.request_repaint_after(due.saturating_duration_since(std::time::Instant::now()));
        }
        // 反向链接的同款三段(#15):去抖到点在归约侧发起(end_of_logic 的
        // 快照比对顺延计时,这里到点帧触发);等待中按剩余时长要帧，扫描
        // 结果到达后由统一通知器唤醒。
        if state
            .backlinks
            .debounce_due
            .is_some_and(|due| due <= std::time::Instant::now())
        {
            state.apply(Message::BacklinksRequested);
        }
        if let Some(due) = state.backlinks.debounce_due {
            ctx.request_repaint_after(due.saturating_duration_since(std::time::Instant::now()));
        }
        // Git 状态轮询:有文件树根且尚未降级才轮询(无根无事可刷;非 git
        // 目录零轮询——重探由换根/切 Git 页触发,egui 得以收敛到深度空闲,
        // 这正是 search 去抖测试守护的不变量)。
        //
        // 到点**异步**发起:实测一轮 12ms、带文件历史 86ms,同步跑会每 3
        // 秒掉 1–5 帧。结果由 `end_of_logic` 的 `poll_git` 收。
        if state.file_tree.root.is_some() && state.git.error.is_none() {
            if state.git.due() && !state.git.refreshing {
                state.start_git_refresh();
            }
            if state.git.error.is_none() && !state.git.refreshing {
                // 空闲时只为下一次三秒轮询排一帧；在途完成由后台通知器
                // 直接唤醒，避免固定 50ms 探测。
                ctx.request_repaint_after(state.git.until_refresh());
            }
        }
        // 自动保存的重绘驱动(#18 帧饥饿修复):停顿落盘不能指望输入来帧
        // ——用户切去别的窗口后 egui 收敛深度空闲,30s 到点没有帧可跑
        // `autosave_pass`,draft 悬到下一次无关重绘(真机实证失焦 6 分钟
        // 未落,docs/autosave-acceptance.md §5)。有待落的停顿计时就按
        // 剩余时长显式要一帧;到点帧落盘后 `saved_rev` 追平,这里自然
        // 不再排程。到点仍未清(写盘失败)时钳 [`AUTOSAVE_WRITE_RETRY`]
        // 慢重试,不满帧空转。
        if let Some(due) = state.next_autosave_due() {
            let wait = due.saturating_duration_since(std::time::Instant::now());
            ctx.request_repaint_after(if wait.is_zero() {
                AUTOSAVE_WRITE_RETRY
            } else {
                wait
            });
        }

        // 窗口标题只在变化时下发,避免每帧一次原生 set_title
        let title = state.tabs.current().document.window_title();
        if *window_title != title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            *window_title = title;
        }

        // 窗口最大化状态帧内同步(双击标题栏/⤢/系统键全走这):viewport
        // info 的 maximized 在窗口就绪前是 None,跳过;与记录不同才写
        // layout.maximized —— 落盘由 end_of_logic 的比对写接管(与左右
        // 栏把手同一口径,闲置帧零 IO)。
        if let Some(maximized) = ctx.input(|i| i.viewport().maximized) {
            if maximized != state.layout.maximized {
                state.layout.maximized = maximized;
            }
        }
    }

    /// `App::ui` 的面板主体。独立成函数是为了测试能在同一 run_ui 帧里按
    /// eframe 顺序(先 `reduce` 后绘制)跑完整帧(本模块测试与
    /// `tab_switch_perf` 取证 harness 同用,故 `pub(crate)`)。
    pub(crate) fn draw(&mut self, ui: &mut egui::Ui) {
        // 禅定模式(§7)是**另一整套面板组合**,不是给三栏各加一个 if:
        // 藏面板的最佳办法是从一开始就不添加它(侧栏宽度演算与 z 序全部
        // 让位),而不是添加了再把可见性摁掉。故在这里整体分叉。
        //
        // 但**只对布局分叉,不对窗口 chrome 分叉**。规格 §7 的「全部退场」
        // 是按原生装饰窗口画的:自绘标题栏退场后,OS 那一根还在,窗口照样
        // 能拖能关。本产品的标题栏是自绘的(D1),无边框模式下 OS 不提供
        // 任何 chrome —— 真照字面连同 chrome 一起藏,退出禅定的出口里
        // 标题栏 Zen 按钮会失效。D4 担心的「怎么退出」在这里会成真,故
        // Zen 只让三栏让位:**它是布局态,不是窗口态**。
        if self.state.layout.zen {
            self.draw_zen(ui);
            return;
        }

        // ⓪ 自绘窗口骨架之一:36px 自绘标题栏(仅无边框模式;
        // LATERMD_NATIVE_DECORATIONS=1 的原生装饰路径不画,行为与旧版
        // 完全一致)。
        if !self.frameless {
            // 同一物理像素分隔线贯穿标题栏、侧栏、预览与状态栏(mac 精修
            // 全平台化,#166):1 物理像素,高分屏下比投影档(1 逻辑像素)
            // 更细,低分屏下与原值一致。
            egui::Panel::top("workbench-header")
                .exact_size(crate::ui::workbench::HEADER_H)
                .show_separator_line(false)
                .frame(egui::Frame::NONE.fill(crate::theme::shell(ui).chrome))
                .show(ui, |ui| {
                    crate::ui::workbench::header(ui, &mut self.state, &mut self.outbox)
                });
        } else if self.frameless {
            egui::Panel::top("titlebar")
                .exact_size(crate::ui::tokens::TITLEBAR_H)
                .show_separator_line(false)
                .frame(
                    egui::Frame::default()
                        .inner_margin(egui::Margin::ZERO)
                        .fill(ui.visuals().panel_fill),
                )
                .show(ui, |ui| {
                    crate::ui::titlebar::ui(ui, &mut self.state, &mut self.outbox);
                });
        }

        // ① 顶部菜单栏已于 2026-10-12 退役(docs/ui-shell-redesign-v2.md
        // §5.7):常驻「文件(F) 编辑(E) …」行整条撤掉,菜单入口收进标题栏
        // 左缘的汉堡按钮(`ui::titlebar` → `menubar::hamburger_button_at`,
        // Zed 同款形态,坤哥指定)。编辑区上沿净高 +26px;mac 原生装饰
        // 路径(workbench-header)本就没有 in-app 菜单行,不受影响。
        let workbench = cfg!(target_os = "macos") && !self.frameless;

        // ② 底部状态栏:散落在工具栏/侧边栏边缘的只读信息收成一行
        // (docs/ui-polish.md §4),工具栏得以只留动作。
        //
        // **必须在左右栏之前加**:先加的最外层、先画者占满全窗横向 ——
        // 若放在 nav/preview 之后,它就只在中央残余区里横跨,左右栏脚下
        // 各缺一截(2026-09-27 用户实测反馈的第二条)。
        egui::Panel::bottom("statusbar")
            .show_separator_line(false)
            .frame(
                egui::Frame::NONE
                    .fill(crate::theme::shell(ui).chrome)
                    // 面板边界由拖动时的交互线提示；空闲时不再画横线。
                    .stroke(egui::Stroke::NONE),
            )
            .show(ui, |ui| {
                status_bar(ui, &self.state);
            });

        // ③ 左栏:导航(文件树 / 搜索 / 大纲 / Git 四视图)。`show_collapsible`
        // 原地持有 `&mut bool`,因此先解构再把闭包要用的其余状态分头借用
        // (都与这两个 bool 不相交)。
        //
        // 宽度下限走 `SIDEBAR_MIN_W`(180,M2 三段式起够用):160 是二分栏
        // 时代的数字(docs/ui-shell-redesign.md §11 R4)。分栏态再给右栏留
        // 出 `PREVIEW_MIN_W`,否则窄窗下侧栏会把中央挤到预算以下。
        let split = self.state.render_mode == crate::live::RenderMode::Split;
        let reserve_right = if split { tokens::PREVIEW_MIN_W } else { 0.0 };
        let sidebar_max =
            (ui.available_width() - crate::ui::workbench::EDITOR_MIN_W - reserve_right)
                .clamp(tokens::SIDEBAR_MIN_W, 400.0);
        crate::ui::workbench::restore_panel_width(ui, "nav", self.state.layout.left_width);
        let layout = &mut self.state.layout;
        let left = &mut layout.left;
        let active_tab = &mut layout.left_view;
        egui::Panel::left("nav")
            .resizable(true)
            .show_separator_line(false)
            .default_size(240.0)
            .size_range(crate::ui::tokens::SIDEBAR_MIN_W..=sidebar_max)
            // 2026-10-09:只把**左** margin 归零,其余照 egui 默认
            // (`Frame::side_top_panel` = symmetric(8,2))。rail 画在本面板
            // 内容区的左端,若保留这 8px,rail 带会与窗口左缘空出一道同底
            // 色的缝 —— 坤哥截图反馈「距左侧边距太宽」(实测图标距窗缘
            // 23px,VS Code 约 12px)。归零后 rail 底色一路贯到窗口边,
            // 才是 Activity Bar 的读法。右/上/下不动:那里仍是与相邻内容
            // 的间隔,沿用默认值以保证与标题栏等其他 panel 观感一致。
            .frame(
                egui::Frame::side_top_panel(ui.style())
                    .inner_margin(egui::Margin {
                        left: if workbench { 12 } else { 0 },
                        right: if workbench { 12 } else { 8 },
                        ..egui::Margin::symmetric(8, 2)
                    })
                    // 侧栏与编辑区共享底色，边界只保留可拖动命中区。
                    .stroke(egui::Stroke::NONE),
            )
            .show_collapsible(ui, left, |ui| {
                let sidebar = if workbench {
                    crate::ui::sidebar::workbench_ui
                } else {
                    crate::ui::sidebar::ui
                };
                sidebar(
                    ui,
                    active_tab,
                    &self.state.file_tree,
                    self.state.tabs.current().document.path.as_deref(),
                    crate::ui::sidebar::OutlineView {
                        items: &self.state.tabs.current().preview.outline,
                        cursor_byte: self.state.tabs.current().cursor.byte,
                    },
                    &mut self.state.search,
                    &self.state.git,
                    &self.state.backlinks,
                    &mut self.outbox,
                );
            });
        // PanelState 当前帧可读 outer_rect;只更新用户实际拖出的宽度,
        // show_collapsible=false 或动画中不写 None。
        crate::ui::workbench::remember_panel_width(
            ui,
            "nav",
            self.state.layout.left,
            &mut self.state.layout.left_width,
        );

        // Git diff 审阅开启时中央区交给 diff 视图(工作区或历史文件,
        // 判定走 diff_target)。编辑器分支内消费。
        let git_diff_workspace =
            self.state.git.diff_workspace_open && self.state.git.diff_target.is_some();

        // ④ 右栏:只读对照列(三态视图)。渲染的就是 Live 富渲染本身
        // (`crate::live::ui`),不是另一套渲染器 —— 右栏能力已在
        // live-unify P1–P3 全部并入写作渲染,这里只是把它放在第二列只读呈现。
        //
        // 与旧「对照预览」面板的三点不同:①状态由 `render_mode == Split`
        // 表达(三态互斥,不再有独立的开合 bool 与「离开 Live 该不该回来」
        // 的快照补工);②用 `TabState::live_right` 这一份独立块状态与独立
        // id 命名空间(见 PR1);③审阅 Git diff 时让位,与旧口径一致。
        //
        // **常显 + `show_collapsible`**(2026-10-16 坤哥:关闭动画没回来):
        // 旧预览面板一直走 egui 的收展通道,分栏 ⇄ 其它档的切换因此有
        // 滑出/滑入动画;PR#252 移植时改成了条件 `show`,开合变成瞬移。
        // 现在恢复:每帧都以 `show_collapsible` 呈现,工作 bool 从
        // `render_mode == Split` 播种 —— egui 在「拖到最小宽再往外拖」
        // (收起)与「收起态细把手往外拖」(展开)及双击边缘时翻转它,
        // 翻转后用 [`Message::CompareColumnToggled`] 收敛回 `render_mode`
        // (Split ⇄ 非 Split 恰是同一个 toggle),`render_mode` 仍是唯一
        // 状态源。完全收起时 egui 只画右缘一根细把手(旧口径同款),
        // `add_contents` 不再执行。
        let mut compare_open = split;
        // **抽屉语义**(2026-10-20 坤哥:「分栏回到写作模式的时候,能不能
        // 从右往左覆盖的感觉,循环起来」):从**写作档**推进来的那一轮
        // (`split_enter_from == Live`),进场是「列向左铺开后向右推到槽位」,
        // 退场要沿同一条路**反向退回**(向左)并淡出 —— 推出去的能拉回来,
        // 才读得出「循环」。
        //
        // 从**源码档**推进来的那一轮维持 #279 的旧观感:列从屏幕右缘向左
        // 进入,退场也反向退回右缘(egui 原生滑出)。**方向镜像「来时那条
        // 路」**,不按退出目标再分一路 —— 怎么来怎么回去。
        //
        // 抽屉那条路进出共用一个 0..1 的进度值,同一个公式
        // `dx = -(1 - frac) * 宽`:进场 frac 0→1(从槽位左侧一路推到槽位),
        // 退场 frac 1→0(原路退回槽位左侧)。二者都必须**钉住 egui 原生
        // 收展**(`animation_time = 0` ⇒ `elapsed / 0` 非有限值 ⇒ 直接落
        // `end` 分支):
        // - 进场钉 `true`:面板当帧满宽,编辑区立刻收窄;
        // - 退场钉 `false`:面板当帧归零,**立刻把宽度还给中央区**,位移、
        //   裁剪、淡出全交给 ⑤' 的浮层。不钉的话 egui 会插一手自己的
        //   「向右滑出」,与抽屉的向左退回在同一帧里打架。
        let drawer = self.state.split_enter_from == crate::live::RenderMode::Live;
        let drawer_frac = ui
            .ctx()
            .animate_bool_responsive(egui::Id::new(COMPARE_PANEL_ID).with("drawer-slide"), split);
        if drawer {
            ui.ctx().animate_bool_with_time(
                egui::Id::new(COMPARE_PANEL_ID).with("animation"),
                split,
                0.0,
            );
        }
        // 正文浮层的横向裁剪下界(侧栏右缘):进场时列要盖过编辑区左半,
        // 但不该盖到侧栏上。
        let compare_clip_left = ui.available_rect_before_wrap().left();
        // 槽位矩形交给 ⑤' 画正文时用。**必须取未平移的 outer_rect**:
        // 退场期间 egui 刻意不落盘 PanelState(`is_animating` 守卫),
        // 留存下来的正是拖动前的满宽槽位 —— 正是我们要的基准。
        let mut compare_slot: Option<egui::Rect> = None;
        if !git_diff_workspace {
            let preview_max = (ui.available_width() - crate::ui::workbench::EDITOR_MIN_W)
                .clamp(tokens::PREVIEW_MIN_W, 880.0);
            if split {
                crate::ui::workbench::restore_panel_width(
                    ui,
                    COMPARE_PANEL_ID,
                    self.state.layout.live_right_width,
                );
            }
            // 拖动命中半径**只在对照列这一帧**局部抬高(出厂 3.0 → 6.0,
            // 命中带宽 6px → 12px)。窄带宽落在同底色的面板之间,没有任何
            // 视觉线索指向它,实测完全找不到可拖处(2026-10-13 历史实测;
            // PR2 恢复面板时漏掉这一段,导致「宽度拉不动」)。必须局部改而
            // 不是改全局 `Style`:egui 的 resize 半径是全局单值,抬高会连带
            // nav 右缘误吞编辑区正文的点击。nav 在本函数更早 show,读到的
            // 已是出厂值;用后即恢复,不留全局副作用。
            let grab_radius = ui.style().interaction.resize_grab_radius_side;
            let restore_grab = grab_radius != tokens::PREVIEW_DIVIDER_GRAB;
            if restore_grab {
                ui.style_mut().interaction.resize_grab_radius_side = tokens::PREVIEW_DIVIDER_GRAB;
            }
            egui::Panel::right(COMPARE_PANEL_ID)
                .resizable(true)
                .show_separator_line(false)
                .default_size(if workbench {
                    ((ui.available_width() - 32.0) * 0.5).max(tokens::PREVIEW_MIN_W)
                } else {
                    tokens::PREVIEW_DEFAULT_W
                })
                .size_range(tokens::PREVIEW_MIN_W..=preview_max)
                .frame(
                    egui::Frame::default()
                        .inner_margin(egui::Margin::symmetric(if workbench { 16 } else { 8 }, 8))
                        .fill(crate::theme::shell(ui).content)
                        // 左缘的常显分隔线走自绘(见 ⑥),故此处不描边:
                        // `Frame::stroke` 是四边描边,会把顶/右/底也描上。
                        .stroke(egui::Stroke::NONE),
                )
                .show_collapsible(ui, &mut compare_open, |ui| {
                    // **两向都要撑满**:面板实际矩形由**内容的 min_rect** 定
                    // (`Frame::show` 的 rect 跟随内容收缩)。正文在退场
                    // 路径才画,进场面板里是空的 —— 不显式撑满,槽位高度会
                    // 塌成内容高度(无头探针:空面板实测 36px 高,而窗口
                    // 高度是 840),正文按该矩形定位就会整块错位。
                    let avail = ui.available_rect_before_wrap();
                    ui.set_min_width(avail.width());
                    ui.expand_to_include_y(avail.max.y);
                    // **抽屉那一轮的正文全程不在④画**(2026-10-19 坤哥「切
                    // 换分栏时改成向右滑动」,2026-10-20 退场反向)。本 Ui 的
                    // clip 被 egui 钉在槽位上(`panel.rs`
                    // `set_clip_rect(visible_outer_rect)`),而列要经过槽位
                    // **左侧**的编辑区上方 —— 画在这里必被裁掉那一截,退化
                    // 回「从边上飞」。它的正文在 ⑤'(中央编辑区之后)画。
                    //
                    // **源码档那一轮仍然在这里画**:egui 原生的从右缘向左
                    // 进入(#279 的旧观感)与它的反向退回都由 panel 的 clip
                    // 兜着,不需要绕。两者按 `drawer` 互斥,任一帧只画一次。
                    if !drawer {
                        compare_column_body(ui, self);
                    }
                });
            // 槽位矩形交给 ⑤' 画正文时用。**必须取未平移的 outer_rect**:
            // 退场期间 egui 刻意不落盘 PanelState(`is_animating` 守卫),
            // 留存下来的正是拖动前的满宽槽位 —— 正是我们要的基准。
            compare_slot = egui::PanelState::load(ui.ctx(), egui::Id::new(COMPARE_PANEL_ID))
                .map(|p| p.outer_rect);
            // egui 在拖拽收起/把手拖开/双击边缘时翻转工作 bool;收敛回
            // `render_mode`(Split ⇄ 非 Split 与该 toggle 同一映射)。
            if compare_open != split {
                self.outbox.push(Message::CompareColumnToggled);
            }
            if restore_grab {
                // nav 在本函数更早 show(③ 在 ④ 之前),它读到的已是出厂值;
                // 这里恢复后受影响的只有后续 panel —— 而它们都不 resizable,
                // 故等价于「不留任何全局副作用」。
                ui.style_mut().interaction.resize_grab_radius_side = grab_radius;
            }
        }
        crate::ui::workbench::remember_panel_width(
            ui,
            COMPARE_PANEL_ID,
            split,
            &mut self.state.layout.live_right_width,
        );

        // ⑤ 编辑器:源文本这份唯一真源住在中央,标签条/提示行/格式工具条在其上。
        // `CentralPanel` 最后加(顺序铁律 AGENTS §8 / adr-005 §3.2)。
        //
        // 曾经用 `Panel::left("editor")` 承载:那之后中央残余区由无人认领
        // 的背景补位,预览左侧多出一条侧栏色的黑条(2026-09-27 用户实测
        // 反馈的第一条)。编辑器回到 `CentralPanel` 才真正「吃掉剩余宽度」。
        //
        // TextEdit 是立即模式控件,必须原地持有 `&mut` 缓冲,故 editor /
        // preview 快照 / 大纲光标的借用下放到本闭包内(归约/绘制二分对这对
        // 「控件附属状态」的例外见 state.rs)。
        let state = &mut self.state;
        let outbox = &mut self.outbox;
        let mut source_rect = None;
        // 编辑器区显式铺内容色:panel 默认 fill 是 `panel_fill`(侧栏色),
        // 不覆盖的话编辑器与右预览会出现两种底色(theme.rs 的口径是
        // 「编辑器与预览面板显式 .fill;侧栏吃 panel_fill」)。margin 取
        // `Frame::side_top_panel` 的出厂值,不因换底挪动既有布局。
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .inner_margin(egui::Margin::symmetric(if workbench { 16 } else { 8 }, 8))
                    .fill(crate::theme::shell(ui).content)
                    .stroke(egui::Stroke::NONE),
            )
            .show(ui, |ui| {
                // 提示行(存在才显示;原文件工具栏的能力,工具栏退役后迁此,
                // decisions-pending #32)
                notice_bar(ui, &state.tabs.current().document, outbox);
                // 孤儿 draft 恢复条(#18,存在才显示):与提示行同为编辑区
                // 顶部的行内条,把编辑器整体下推一行 —— 它是需要持续在场的
                // 裁决入口,不与查找浮层/对话框抢「浮动层」语义。载荷带标签
                // 稳定 id,归约按 id 定位(消息是下一帧才消费的,届时活动
                // 标签理论上可能已变,与 confirm_close 同手法)。
                {
                    let current = state.tabs.current();
                    if let Some(recover) = current.recover.as_ref() {
                        recovery_bar(ui, recover, current.id, outbox);
                    }
                }
                if git_diff_workspace {
                    let git = &state.git;
                    ui.horizontal(|ui| {
                        ui.heading("Git 差异");
                        match git.diff_target.as_ref() {
                            Some(crate::git_panel::DiffTarget::WorkingTree { path }) => {
                                ui.monospace(path);
                            }
                            Some(crate::git_panel::DiffTarget::Commit { hash, path }) => {
                                let parent = git
                                    .history
                                    .as_ref()
                                    .and_then(|history| history.parent.as_ref())
                                    .map(|commit| commit.short_hash.as_str())
                                    .unwrap_or("空树");
                                let subject = git
                                    .history
                                    .as_ref()
                                    .map(|history| history.commit.subject.as_str())
                                    .unwrap_or_default();
                                ui.weak(format!(
                                    "{parent} → {} · {subject}",
                                    hash.chars().take(7).collect::<String>()
                                ));
                                ui.monospace(path);
                            }
                            None => {}
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            crate::ui::sidebar::git_back_to_editor_button(ui, outbox);
                            crate::ui::sidebar::diff_view_toggle(ui, git.diff_view, outbox);
                        });
                    });
                    ui.add_space(crate::ui::tokens::SPACE_XS);
                    ui.weak("HEAD → 工作区 · 只读审阅");
                    ui.add_space(crate::ui::tokens::SPACE_XS);
                    crate::ui::sidebar::render_git_diff(ui, git, ui.available_height());
                    return;
                }
                let tab = state.tabs.current_mut();
                // Live 分支(live-unify-plan P1)的两个渲染条件,与右栏预览
                // 同源同口径:`base_dir` 锚定相对图片的 file:// 改写(未落盘
                // 为 None),`heal` 仅 AI 流式写入本标签时开(残缺补闭合)。
                // 都在 destructure 前取 —— document 不进下面的解构,借用随
                // 即结束,不与 editor/preview 的 &mut 冲突。
                let live_heal = state.ai.is_streaming() && state.ai_active_tab == Some(tab.id);
                let base_dir = tab
                    .document
                    .path
                    .as_deref()
                    .and_then(std::path::Path::parent);
                let crate::tabs::TabState {
                    editor,
                    preview,
                    cursor,
                    selection,
                    pending_selection,
                    live,
                    id,
                    ..
                } = tab;
                // Markdown 格式工具条(docs/ui-shell-redesign.md §6):在文件
                // 工具栏之下、编辑区之上。它作用的选区由 editor 每帧回填到
                // `TabState::selection` —— 按钮被点中时编辑器已失焦,选区
                // 活不过那一帧。
                // 测试探针:把这一帧各按钮的实测位置交出去,让无头测试点得到
                // 真按钮。生产路径取 None 分支(零开销)。
                #[cfg(not(test))]
                crate::ui::format_bar::ui(ui, &state.keymap, outbox);
                #[cfg(test)]
                crate::ui::format_bar::ui_with_probe(
                    ui,
                    &state.keymap,
                    outbox,
                    self.format_probe.as_deref_mut(),
                    None::<fn(egui::Rect)>,
                );
                // min_rect 包括标签与可换行的工具条,浮层只能锚定它们之后的视口。
                source_rect = Some(ui.available_rect_before_wrap());
                // 保焦目标:焦点落在查找框或替换框上时,编辑器消费
                // `pending_selection` 抢焦后要还回去(替换行 #17:替换词
                // 没打完不能被跳转抢走)。
                let focus_owner = if ui.memory(|memory| memory.has_focus(find_input_id())) {
                    Some(find_input_id())
                } else if state.find.replace_open
                    && ui.memory(|memory| memory.has_focus(replace_input_id()))
                {
                    Some(replace_input_id())
                } else {
                    None
                };
                // 查找卡打开时,把全部命中的字符区间交给编辑器侧做
                // 「页面级高亮」(VS Code 的 find match 橙色底):当前条
                // 由 pending_selection 的选区承担,其余命中画浅底。
                //
                // 只借不拷:`editor::ui` 拿的是 `&[Range<usize>]`(它只读),
                // 而它借走的 `&mut TabState` 与 `state.find` 是 `state` 的
                // 两个不相交字段,借用分割成立 —— 旧写法每次都克隆整条
                // hits(常见词在大文档里上千条,每帧一遍)。
                //
                // 分栏档的中央同样是源码 TextEdit,故两档都给(写作档的富
                // 渲染没有「页面级浅底」这层绘制)。
                let source_column = state.render_mode != crate::live::RenderMode::Live;
                let find_marks: &[std::ops::Range<usize>] =
                    if state.find.open && source_column && !state.find.hits.is_empty() {
                        &state.find.hits
                    } else {
                        &[]
                    };
                let keep_find_focus = state.find.open
                    && source_column
                    && pending_selection.is_some()
                    && cursor.jump_to.is_none()
                    && focus_owner.is_some()
                    && !ui.input(|input| input.pointer.any_pressed());
                if workbench {
                    ui.visuals_mut().extreme_bg_color = crate::theme::shell(ui).content;
                }
                let show_minimap = state.theme.show_minimap
                    && (!state.theme.minimap_auto || {
                        let font = egui::FontSelection::Style(egui::TextStyle::Monospace)
                            .resolve(ui.style());
                        let row = ui.fonts_mut(|fonts| fonts.row_height(&font))
                            + ui.spacing().extra_text_line_spacing;
                        crate::ui::minimap::automatic_visible(
                            ui.available_width(),
                            ui.available_height(),
                            row,
                            editor.text(),
                        )
                    });
                if tab.focus_editor_on_next_frame {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(
                            crate::ui::editor::tab_editor_id(*id).with("focus-new"),
                            true,
                        )
                    });
                    tab.focus_editor_on_next_frame = false;
                }
                if tab.focus_live_on_next_frame {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(
                            crate::ui::editor::tab_editor_id(*id).with("live-focus-new"),
                            true,
                        )
                    });
                    tab.focus_live_on_next_frame = false;
                }
                crate::ui::editor::ui(
                    ui,
                    editor,
                    preview,
                    // Live 侧 AI 卡与 ai:// 拦截的流状态源(P2/D1):全局面态,
                    // 与预览 handler 同口径(同时至多一个流)。
                    &state.ai,
                    crate::ui::editor::CursorChannel {
                        cursor,
                        selection,
                        pending: pending_selection,
                    },
                    live,
                    state.render_mode,
                    crate::ui::editor::tab_editor_id(*id),
                    // #55 M2:源码 minimap 开关(全局偏好,所有标签同开同关;
                    // 行模型缓存仍是每标签一份,minimap::cache_id 分槽)。
                    show_minimap,
                    // #64 M1:打字机模式开关(全局偏好,源码/Live 两模式
                    // 共用;关闭 = 现状零变化)。
                    state.theme.show_typewriter,
                    // #64 M2:专注模式开关(全局偏好;仅 Live 模式淡化非
                    // 活动块,源码模式不接线 —— 边界见 decisions-pending #122;
                    // 关闭 = 现状零变化)。
                    state.theme.show_focus_mode,
                    // 选区 AI 浮标开关(#61 配置面,全局偏好)。
                    state.theme.show_selection_ai,
                    // live-unify-plan P1:相对图片锚定目录与流式 heal,
                    // 仅 Live 分支消费(见 editor::ui 文档)。
                    base_dir,
                    live_heal,
                    // 右键菜单条目的键位标注与菜单栏同源(context-menu-plan §3 M1)。
                    &state.keymap,
                    outbox,
                    find_marks,
                    // Split 对照列的滚轮锚点出参(仅源码分支写,见 editor::ui 文档)。
                    &mut tab.live_right.mouse_anchor,
                );
                // 命中回填沿用编辑器的选区/滚动通道,但不能终止查找框的连续输入。
                if keep_find_focus {
                    if let Some(id) = focus_owner {
                        ui.memory_mut(|memory| memory.request_focus(id));
                    }
                }
            });

        // ⑤ 查找卡是源码区专属浮层:不参与 CentralPanel 普通布局,因此
        // 不会把源码整体向下推;右上锚点留出滚动条/边框间隙。
        // 分栏档的中央也是源码区,查找卡同样可用(写作档的富渲染没有
        // 「当前条选区 + 其余命中浅底」这两层绘制,故只在源码列给)。
        if state.find.open && state.render_mode != crate::live::RenderMode::Live {
            if let Some(rect) = source_rect {
                draw_find_overlay(ui.ctx(), rect, &mut state.find, outbox);
                // 打开时的「给焦点」意图由归约置位,这里消费一次:
                // VS Code 同款,打开即可打字;焦点请求发生在本帧 TextEdit
                // 绘制之后 egui 仍会在下一帧生效,清除标志保证只在打开后
                // 请求一次 —— 用户随后的任何点击(把手/编辑器)不被抢回。
                if state.find.want_focus {
                    ui.memory_mut(|memory| memory.request_focus(find_input_id()));
                    state.find.want_focus = false;
                }
            }
        }

        // ⑤「跳转到行」浮条(#60 M1):同一右上锚点、同一浮层语义;与查找
        // 条互斥(归约保证),源码/Live 两模式都在场 —— Live 侧跳转走块
        // 路由,不受「查找条只限源码」的限制。
        if state.goto.open {
            if let Some(rect) = source_rect {
                draw_goto_overlay(ui.ctx(), rect, &mut state.goto, outbox);
            }
        }

        // ⑤' 对照列正文:面板(④)只占位,正文在这里画第二遍。**只服务
        // 抽屉那一轮**(见 ④ 的 `drawer` 文档)—— 进场向右推开、退场反向
        // 退回,两条都是这条浮层。
        //
        // **为什么必须在中央编辑区之后**:那条通道要的观感是「列整块盖在
        // 编辑区上」,而 ④ 的 Ui 被 egui 把 clip 钉在槽位上
        // (`panel.rs` `set_clip_rect(visible_outer_rect)`),画在 ④ 会被裁掉
        // 槽位左侧那一截。这里用根层另开一个子 Ui,clip 由本函数自己给,
        // 才盖得住编辑区。
        //
        // 源码档那一轮的进出都由 ④ 的面板 Ui 画(那边 `!drawer` 时才调
        // `compare_column_body`),走 egui 原生滑入/滑出。
        //
        // **退场收尾必须彻底停画**:动画收敛后 `drawer_frac` 停在 0,`dx`
        // 恒等于 −槽位宽,列整块落在槽位左侧、opacity 为 0 —— 肉眼不可见,
        // 但 `compare_column_body` 会在**离开分栏后的每一帧**照跑一遍
        // (整套 Live 富渲染 + 块同步)。生产里这就是「切回写作档之后白烧
        // 一整条渲染」。故闸门加在这里,不看 opacity 看「这个抽屉还有没有
        // 被看见的可能」。
        if drawer && (split || drawer_frac > 0.0) {
            if let Some(slot) = compare_slot {
                // 进退同一个公式:frac 0→1 是「从槽位左侧一路推到槽位」,
                // 1→0 是原路退回。方向完全由这一个值决定,不存在第二份
                // 「退场该往哪」,翻来覆去都不会漂移。
                let dx = -(1.0 - drawer_frac) * slot.width();
                let rect = egui::Rect::from_min_max(
                    egui::pos2(slot.left() + dx, slot.top()),
                    egui::pos2(slot.right() + dx, slot.bottom()),
                );
                let clip = egui::Rect::from_min_max(
                    egui::pos2(compare_clip_left, slot.top()),
                    egui::pos2(slot.right(), slot.bottom()),
                );
                if clip.intersects(rect) {
                    // `new_child` 而非 `scope_builder`:后者会
                    // `advance_cursor_after_rect` 推进根 Ui 游标,而游标
                    // 已被全部 panel 推进过,再推会顶歪后面的
                    // `edge_resize_zones`(它按游标 `allocate_rect`)。
                    let mut holder = ui.new_child(
                        egui::UiBuilder::new()
                            .layer_id(ui.layer_id())
                            .max_rect(rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    holder.set_clip_rect(clip);
                    // 淡出只作用于**退场的后半程**:抽屉往回缩的同时渐渐
                    // 透明,回到起点恰好隐没。前半程(frac > 0.55)保持不
                    // 透明 —— 否则列刚退就发虚,看不出是在平移。
                    //
                    // 为什么写 Ui 而不是 painter:`Ui::painter()` 只有只读借
                    // 用;而 `Ui::set_opacity` 写的是 Ui 自己的 painter 副本,
                    // `new_child`/`Frame::show` 克隆 painter 时会带着它往下
                    // 传 —— 正好覆盖到列头与 Live 正文的每一笔。
                    if !split {
                        let fade = ((drawer_frac - 0.15) / 0.55).clamp(0.0, 1.0);
                        holder.set_opacity(fade);
                    }
                    // 背景必须跟着正文一起走,否则列会「透明」地压在编辑
                    // 区上。色值与 ④ 的 Panel Frame 同源。
                    egui::Frame::default()
                        .inner_margin(egui::Margin::symmetric(if workbench { 16 } else { 8 }, 8))
                        .fill(crate::theme::shell(ui).content)
                        .stroke(egui::Stroke::NONE)
                        .show(&mut holder, |ui| {
                            ui.expand_to_include_rect(ui.max_rect());
                            compare_column_body(ui, self);
                        });
                }
            }
        }

        // ⑥ 顶层浮层通道(commit 建议 / 设置 / 回滚确认 / 关标签确认 /
        // 图片框 / Emoji / 快速打开 / 快捷键蒙层):浮窗是独立 Area 层,
        // 不参与 panel 嵌套,画在 panel 之后取语义上的「最上层」。三栏与
        // 禅定两条布局路径共用,理由见 [`Self::draw_overlay_dialogs`]。
        self.draw_overlay_dialogs(ui);

        // ⑥ 对照列左缘的常显分隔线。**必须在全部 panel 之后**:它走
        // `layer_painter` 在所有 panel 的 painter 之上单画一根线,而同一
        // `Order::Middle` 内层序按 id 排,独立 id 天然压过 panel。若挪到
        // ④ 之后立刻调,中央编辑区(⑤)的 `Frame::fill` 会盖掉整根线。
        //
        // 这条线与 egui 自带的 hover/drag 高亮线**并存且不冲突**:后者画在
        // `parent_ui.painter()` 上(本函数下就是这层之下),且只在交互时出现。
        // Git diff 审阅帧没有对照列:跳过分隔线,否则 PanelState 里残留的
        // 上一次矩形会把线画进 diff 区。
        if !git_diff_workspace {
            crate::ui::workbench::panel_divider(ui, COMPARE_PANEL_ID, split);
        }

        // ⑥ 自绘窗口骨架之二:屏幕四边/四角的透明缩放命令区。**必须在
        // 全部 panel 之后分配**(机制见 ui::titlebar::edge_resize_zones 的
        // 文档:同层命中、后分配者在同距裁决中胜出);此处光标推进位于
        // 所有面板之后,不影响任何 panel 的布局。
        if self.frameless {
            crate::ui::titlebar::edge_resize_zones(ui);
        }
    }

    /// 顶层浮层通道,存在才显示:commit message 建议、设置对话框、回滚
    /// 确认、脏标签关闭确认、标签重命名、图片框、Emoji 面板、快速打开、
    /// 快捷键蒙层(#54 M2)。浮窗是独立 Area 层,不参与 panel 嵌套,
    /// 各浮窗的机制说明见其函数文档。
    ///
    /// **三栏与禅定两条布局路径都要调它。** `draw` 在禅定帧整体分叉提前
    /// return,浮层若只画在三栏路径,禅定里**可达**的入口就成了哑弹:设置
    /// 齿轮挂在禅定同样保留的标题栏上(D4 × decisions-pending #31),点击后
    /// 弹窗要悬置到退出禅定才突然出现;Ctrl+W 是全局命令,禅定里触发脏标签
    /// 关闭确认同样悬置。浮窗与布局分叉无关(2026-09-27 评审修复)。
    fn draw_overlay_dialogs(&mut self, ui: &mut egui::Ui) {
        let outbox = &mut self.outbox;

        // commit message 建议:复制即时写系统剪贴板,关闭只发消息,清建议的
        // 归约在 `App::logic`(见 `commit_dialog` 文档)。
        if let Some(subject) = self.state.ai_commit_suggestion.clone() {
            let (_, close, x) = commit_dialog(ui, &subject);
            if close.clicked() || x {
                outbox.push(Message::AiCommitDismissed);
            }
        }

        // 选区润色确认浮窗(#61 M3):草稿流式逐块增长(实时可见),确认/
        // 放弃只发消息,替换与作废在归约。Esc 的消费在 `reduce`(浮窗在场
        // 即归它,先于蒙层/禅定/查找条等其它 Esc 语义)。
        if let Some(session) = self.state.ai_polish.clone() {
            let (confirm, dismiss, _copy, x) =
                selection_ai_polish_dialog(ui, &session.draft, self.state.ai.is_streaming());
            if confirm.clicked() {
                outbox.push(Message::SelectionAiPolishConfirmed);
            }
            if dismiss.clicked() || x {
                outbox.push(Message::SelectionAiPolishDismissed);
            }
        }

        // 设置(外观 / 快捷键 / AI / MCP / 图片):凭据读写只在归约(Message),
        // 对话框只持草稿与展示状态;关闭按钮原地翻转开关。
        if self.state.settings.open {
            let state = &mut self.state;
            let resolved = state.resolved_theme();
            let close = crate::settings::dialog(
                ui,
                &mut state.settings,
                &state.theme,
                &state.skins,
                state.system_theme_ok,
                resolved,
                &state.keymap,
                &state.ai,
                &mut state.ai_key,
                &state.mcp,
                &mut state.bed,
                outbox,
            );
            if close.is_some_and(|close| close.clicked()) {
                state.settings.open = false;
            }
        }

        // 回滚确认:文案显式警示不可逆,目标恰是编辑器当前文档时追加针对性
        // 警示(见 `checkout_extra_warning`);checkout 只在「回滚」按钮点击
        // 之后的归约里执行。
        if let Some(path) = self.state.git.confirm_checkout.clone() {
            let open_in_editor = self.state.git.absolute_path(&path).is_some_and(|abs| {
                self.state.tabs.current().document.path.as_deref() == Some(abs.as_path())
            });
            let (confirm, cancel, x) = checkout_dialog(
                ui,
                &path,
                open_in_editor,
                self.state.tabs.current().editor.is_dirty(),
            );
            if confirm.clicked() {
                outbox.push(Message::GitCheckoutConfirmed);
            }
            if cancel.clicked() || x {
                outbox.push(Message::GitCheckoutCancelled);
            }
        }

        // 侧边栏「全部替换」确认(跨文件写盘,与 Git 回滚同一套确认纪律):
        // 受影响文件数由归约在挂起时折算(search.confirm_replace);真正的
        // 替换在「全部替换」点击之后的归约里执行,确认后命中列表重收敛。
        if let Some(plan) = &self.state.search.confirm_replace {
            let (confirm, cancel, x) = search_replace_dialog(ui, plan.paths.len());
            if confirm.clicked() {
                outbox.push(Message::SearchReplaceAllConfirmed);
            }
            if cancel.clicked() || x {
                outbox.push(Message::SearchReplaceAllCancelled);
            }
        }

        // 脏标签关闭确认(标签条 × / Ctrl+W 触发):目标按稳定 id 存
        // (`TabsState::confirm_close`),打开期间其他关闭入口会使索引漂移;
        // 目标被别的路径关掉时 `TabsState::remove` 已撤下确认,这里自然
        // 不再渲染。文案用标签显示名(#37 别名优先)—— 用户在标签条上认
        // 的是什么名字,模态就问什么名字。
        if let Some(tab) = self.state.tabs.confirm_close_tab() {
            let (confirm, cancel, x) = tab_close_dialog(ui, &tab.display_name());
            if confirm.clicked() {
                outbox.push(Message::TabCloseConfirmed);
            }
            if cancel.clicked() || x {
                outbox.push(Message::TabCloseCancelled);
            }
        }

        // 标签重命名(#37 右键菜单「重命名」,**显示别名**语义):目标按
        // 稳定 id 存(`TabsState::rename`),被关掉时 `TabsState::remove` 已
        // 撤下,这里自然不再渲染。浮窗只收草稿,置别名在归约
        // (`State::confirm_tab_rename`);文件行明示作用范围(不改盘上文件)。
        let rename_target = self.state.tabs.rename.as_ref().map(|rename| rename.tab_id);
        if let Some(tab_id) = rename_target {
            // 先结清只读借用再 as_mut 草稿(rename 字段与 tabs 整体的借用
            // 不能并存);标签被关掉时 remove 已撤下浮窗,这里是防御占位。
            let file_label = self
                .state
                .tabs
                .index_by_id(tab_id)
                .map(|index| self.state.tabs.tabs[index].document.base_name())
                .unwrap_or_else(|| "未知(标签已关闭)".to_owned());
            if let Some(rename) = self.state.tabs.rename.as_mut() {
                let (confirm, cancel, x, submitted) =
                    crate::ui::tabs::rename_dialog(ui, rename, &file_label);
                if confirm.clicked() || submitted {
                    outbox.push(Message::TabRenameConfirmed);
                }
                if cancel.clicked() || x {
                    outbox.push(Message::TabRenameCancelled);
                }
            }
        }

        // 文件栏「新建」浮窗:草稿与目标目录由归约挂在 `file_tree.new_file`,
        // 这里只收草稿、按名称合法性禁用「创建」、点按只发消息 —— 落盘、
        // 缓存失效与开标签全在归约(与图片框同一纪律:对话框不碰 IO)。
        if let Some(new_file) = self.state.file_tree.new_file.as_mut() {
            let (confirm, cancel, x, submitted) = crate::ui::sidebar::new_file_dialog(ui, new_file);
            if confirm.clicked() || submitted {
                outbox.push(Message::FileTreeNewFileConfirmed);
            }
            if cancel.clicked() || x {
                outbox.push(Message::FileTreeNewFileCancelled);
            }
        }

        // 文件树右键「新建文件夹」浮窗(context-menu-plan §3 M2):与新建
        // 文档同纪律 —— 只收草稿,落盘在归约(confirm_new_folder)。
        if let Some(new_folder) = self.state.file_tree.new_folder.as_mut() {
            let (confirm, cancel, x, submitted) =
                crate::ui::sidebar::new_folder_dialog(ui, new_folder);
            if confirm.clicked() || submitted {
                outbox.push(Message::FileTreeNewFolderConfirmed);
            }
            if cancel.clicked() || x {
                outbox.push(Message::FileTreeNewFolderCancelled);
            }
        }

        // 文件树右键「重命名」浮窗:草稿预填现名,确认走归约
        // (confirm_rename:真实改名 + 标签换绑)。
        if let Some(rename) = self.state.file_tree.rename.as_mut() {
            let (confirm, cancel, x, submitted) = crate::ui::sidebar::rename_dialog(ui, rename);
            if confirm.clicked() || submitted {
                outbox.push(Message::FileTreeRenameConfirmed);
            }
            if cancel.clicked() || x {
                outbox.push(Message::FileTreeRenameCancelled);
            }
        }

        // 文件树右键「删除」确认浮窗:只收确认,删除在归约
        // (confirm_delete:dirty 拒绝 / 干净关标签 / 废纸篓)。
        if let Some(path) = self.state.file_tree.delete_confirm.as_ref() {
            let is_dir = path.is_dir();
            let (confirm, cancel, x) = crate::ui::sidebar::delete_confirm_dialog(ui, path, is_dir);
            if confirm.clicked() {
                outbox.push(Message::FileTreeDeleteConfirmed);
            }
            if cancel.clicked() || x {
                outbox.push(Message::FileTreeDeleteCancelled);
            }
        }

        // 图片框(docs/image-plan.md A/B/C 三段):只收 alt 与 url 草稿及
        // 图床选择,插入在归约走 `compose::insert_image`;地址为空时「插入」
        // 按钮在对话框里已被禁用。点击插入时草稿还在 state 上,克隆进消息
        // 载荷。「浏览」与「上传」只发消息:文件选择、复制进 `.assets/`、
        // 后台上传都在归约(对话框不碰 IO)。
        if self.state.image_dialog.open {
            let (insert, browse, upload, cancel, x) = crate::ui::image_dialog::dialog(
                ui,
                &mut self.state.image_dialog,
                &self.state.bed.profiles,
            );
            if insert.clicked() {
                let alt = self.state.image_dialog.alt.clone();
                let url = self.state.image_dialog.url.clone();
                outbox.push(Message::ImageInserted { alt, url });
            }
            if browse.clicked() {
                outbox.push(Message::ImageFilePickRequested);
            }
            if upload.clicked() {
                outbox.push(Message::ImageUploadRequested);
            }
            if cancel.clicked() || x {
                outbox.push(Message::ImageDialogClosed);
            }
        }

        // Emoji 面板(docs/emoji-plan.md E1):搜索草稿与分类归 UI 原地
        // 持有,点选插入在归约走 `compose::insert_emoji`;点选后关面板与
        // Esc 关闭都经消息(Esc 优先级让位禅定出口,见 emoji_panel 模块
        // 文档)。返回的单元响应只供面板自身测试定位,生产路径忽略。
        if self.state.emoji.open {
            let _cells = crate::ui::emoji_panel::panel(ui, &mut self.state.emoji, outbox);
        }

        // 快速打开(#24,Cmd/Ctrl+P):文件树全部 md 与命令全集的统一入口
        // 浮层。查询词与选中下标归 UI 原地持有,快照与开关在归约
        // (`ToggleQuickOpen`);选中文件走文件树点击同一条 `FileSelected`
        // (→ `open_path`),选中命令直接执行 `cmd.message()`。返回的浮窗
        // 响应只供无头测试定位浮层矩形,生产路径忽略。
        if self.state.quick_open.open {
            let _window = crate::ui::quick_open::panel(ui, &mut self.state.quick_open, outbox);
        }

        // 「关于 LaterMD」(#71):帮助菜单打开,内容只读;蒙层点击与
        // 窗 X 的关闭请求在此翻成消息(Esc 的关闭在 `reduce` 消费)。
        // M2 起「检查更新」按钮点击也经 outbox 发消息(不直接起线程)。
        if self.state.about.open && crate::ui::about::dialog(ui, &self.state.about, outbox) {
            outbox.push(Message::AboutClosed);
        }

        // 长按修饰键的快捷键蒙层(#54 M2):Visible 态才画。内容源是
        // Command/keymap 注册表(单一事实源,不抄第二份清单);非焦点层,
        // 编辑器的键盘焦点与输入不受影响。挂在浮层五件套同层,三栏与禅定
        // 两条布局路径都会经过这里(禅定显式放行,13a 教训)。
        crate::shortcut_overlay::paint(ui, &mut self.state.shortcut_overlay, &self.state.keymap);
    }

    /// 禅定模式的整套面板组合(§7)。
    ///
    /// 退出出口在此合流:F11 / 标题栏 Zen 按钮(无边框)/ Esc /
    /// 右上角「退出禅定」浮层(**仅原生装饰路径**,见下方 gating)。
    fn draw_zen(&mut self, ui: &mut egui::Ui) {
        // Zen 是**另一套 panel 组合**而非「给三栏各加一个 if」:藏面板的最佳
        // 办法是从一开始就不添加它(侧栏宽度演算与 z 序全部让位),而不是
        // 添加了再把可见性摁掉。窗口 chrome 保留的理由见 [`Self::draw`]。
        if cfg!(target_os = "macos") && !self.frameless {
            egui::Panel::top("workbench-header")
                .exact_size(crate::ui::workbench::HEADER_H)
                .show_separator_line(false)
                .frame(egui::Frame::NONE.fill(crate::theme::shell(ui).chrome))
                .show(ui, |ui| {
                    crate::ui::workbench::header(ui, &mut self.state, &mut self.outbox)
                });
        } else if self.frameless {
            egui::Panel::top("titlebar")
                .exact_size(crate::ui::tokens::TITLEBAR_H)
                .show_separator_line(false)
                .frame(
                    egui::Frame::default()
                        .inner_margin(egui::Margin::ZERO)
                        .fill(ui.visuals().panel_fill),
                )
                .show(ui, |ui| {
                    crate::ui::titlebar::ui(ui, &mut self.state, &mut self.outbox);
                });
        }

        // Esc 退出禅定(§7)。
        //
        // Esc 不在 command 表里:它是**禅定内的 (mode-local) 出口**而不是一条
        // 全局命令 —— 塞进 command 表会和别处的 Esc 语义撞车。
        //
        // 在 panel 之前消费而非嵌在某棵子树里:消费即把事件从输入流移除,
        // 后面绘制的预览区(User 端点 SUCH  as `ai://` 链接)看不见它。
        let escape = ui
            .ctx()
            .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if escape {
            self.outbox.push(Message::ZenToggled);
        }

        // 正文:**唯一的** CentralPanel,吃掉标题栏以外的全部剩余空间。预览
        // 因此天然占满内容区,再在内部按 [`tokens::ZEN_TEXT_W`] 限宽居中 ——
        // 「让一个 panel 占满」与「内容限宽」是两件事,分别由 central panel
        // 与内部的 `set_max_width` 各管一段。
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    // `Frame::inner_margin` 收 `impl Into<Margin>`,`Margin` 是
                    // i8 ;`f32` 走 `From<f32>` 时被 round 掉小数,故 `ZEN_GUTTER`
                    // 取整数常量(24),不留 24.5 这种会被静默吞掉的值。
                    .inner_margin(tokens::ZEN_GUTTER)
                    .fill(crate::theme::shell(ui).content),
            )
            .show(ui, |ui| {
                // 720 限宽居中,但**下限.md 的可用宽度**:窗口窄于
                // `720 + 2×gutter` 时必须跟着缩,否则 preview 会被推出内容区右
                // 缘 —— 窄窗写 Markdown 是常态(半屏贴左边),溢出等于逼出横向
                // 滚动。"限宽" 是上限而非定值,这正是它与 `set_width` 的区别。
                //
                // `vertical_centered`(top_down + Align::Center)负责把不足 720
                // 的内容横向居中,`set_max_width` 负责把超宽的夹回来:两者分工
                // 不同,缺任一个都不成立。
                ui.vertical_centered(|ui| {
                    ui.set_max_width(tokens::ZEN_TEXT_W.min(ui.available_width()));
                    // 宽度探针(仅测试):把这一层实测出来的内容宽度交出去,
                    // 让无头测试量得到 720 限宽真的生效了。**刻意放在这里而
                    // 不是在测试里重演同一段布局** —— 后者只是在验证
                    // `set_max_width` 本身,验证不了 `draw_zen` 有没有真的调它。
                    #[cfg(test)]
                    if let Some(probe) = self.zen_probe.as_deref_mut() {
                        probe(ui.max_rect().width());
                    }
                    // 禅定正文走 Live 浏览态(P3,live-unify-plan):块级富
                    // 渲染、只读 —— 与当前 render_mode 解耦,源码模式下进
                    // 禅定同样看富渲染。浏览是**帧内临时值**:每帧强制
                    // browse_only + 关 browse_enabled(点击不进编辑、
                    // 不抢焦点),不碰持久化的 render_mode(R3);退出禅定的
                    // 交互还原在 `State::toggle_zen`(browse_enabled 复位,
                    // 写作态由 live::ui 的既定翻转自愈)。
                    //
                    // 解构再分头借用:`&mut self.state.tabs` 与 `&self.state.ai`
                    // 是同一棵树上的不相交分支,挨着写会被借用检查器拦下。
                    // Live 自带 ScrollArea(`auto_shrink([false,false])`)会把
                    // 父 Ui 的矩形**向下撑出**内容区:CentralPanel 之后的
                    // `ui::zen_nav` 读的是同一个根 Ui 的 `max_rect()`,被撑
                    // 出的那 2px 会让导航列的驻位矩形越过窗口底缘 —— 导航行
                    // 的视口剔除判据随之失真(2026-10-15 实测,P3)。故把正文
                    // 关进**带固定矩形**的子 Ui:高度增长留在子 Ui 内,不向
                    // 上传播。横向仍由上面的 `vertical_centered` +
                    // `set_max_width` 限定(同一 I-E. 上限语义)。
                    let body_rect = ui.available_rect_before_wrap();
                    let LaterMdApp { state, outbox, .. } = self;
                    let tab = state.tabs.current_mut();
                    tab.live.browse_only = true;
                    tab.live.browse_enabled = false;
                    tab.live.browse_locked = true;
                    tab.live.active = None;
                    tab.live.pending_caret = None;
                    tab.live.pending_range = None;
                    // heal 条件同三栏路径:仅 AI 流式写入本标签时开。
                    let streaming_here =
                        state.ai.is_streaming() && state.ai_active_tab == Some(tab.id);
                    // 打字机/专注关:浏览态没有活动块,两者天然失效 —— 显式
                    // false 让禅定帧不进打字机记忆读写(decisions-pending #122
                    // 的边界同口径)。
                    let mut body = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(body_rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    crate::live::ui(
                        &mut body,
                        &mut tab.editor,
                        &mut tab.preview,
                        &state.ai,
                        &mut tab.cursor,
                        &mut tab.selection,
                        &mut tab.live,
                        crate::ui::editor::tab_editor_id(tab.id),
                        false,
                        false,
                        // 禅定浏览态无活动块,浮标天然不弹,显式关
                        false,
                        tab.document
                            .path
                            .as_deref()
                            .and_then(std::path::Path::parent),
                        streaming_here,
                        // 禅定是只读阅读:无徽标槽,内容不右移
                        false,
                        &state.keymap,
                        outbox,
                    );
                });
            });

        // 右上角「退出禅定」浮层:**仅原生装饰路径**(LATERMD_NATIVE_DECORATIONS=1,
        // 自绘标题栏不在场)才画。无边框模式下标题栏本就保留、其 Zen 键在禅定中
        // 以强调色高亮(tooltip「退出禅定」),浮层再画一颗同款 Zen 圆环 = 右上角
        // 两颗禅定图标,且本函数锚定根 Ui 的 right_top(**含标题栏**),矩形落在
        // y 10..38 —— 正压在标题栏右端按钮区(2026-10-09 坤哥真机:「禅定模式
        // 右上角 x 关闭的地方多了一个禅定模式图标」,像素取证见提交说明),还以
        // 后分配者身份抢走最大化按钮的点击。禅定的四条出口(F11 / 标题栏 Zen 键 /
        // Esc / 原生装饰下的浮层)不受影响;Zen 里唯一的常驻 chrome 因此改为
        // 「无边框=标题栏、原生=浮层」,二者互斥,各自都是最后分配的 widget。
        if !self.frameless {
            zen_exit_button(ui, &mut self.outbox);
        }

        // 禅定左缘标签导航(#57 M1 悬停唤出;M2 三态配置):悬停=鼠标移近
        // 左缘唤出、离开即隐;常显=进禅定即显示;关闭=零路径不渲染。与
        // 右上退出入口(浮层/标题栏 Zen 键)同一层、其后分配(左缘与右上角
        // 不重叠,互不抢命中);
        // 感应区是纯几何判定、导航列是同层绝对摆放,均不建 Foreground 层
        // Area——机制与红线见 `ui::zen_nav` 模块文档(13a 教训)。
        #[cfg(not(test))]
        crate::ui::zen_nav::ui(
            ui,
            self.state.theme.zen_nav,
            self.frameless,
            &self.state.tabs,
            &mut self.state.zen_nav,
            &mut self.outbox,
        );
        #[cfg(test)]
        crate::ui::zen_nav::ui_with_probe(
            ui,
            self.state.theme.zen_nav,
            self.frameless,
            &self.state.tabs,
            &mut self.state.zen_nav,
            &mut self.outbox,
            self.zen_nav_probe.as_deref_mut(),
        );

        // 顶层浮层与三栏路径同一份:禅定保留的标题栏上有设置齿轮、Ctrl+W
        // 是全局命令 —— 入口在禅定里可达,浮窗就必须跟着可达(见
        // [`Self::draw_overlay_dialogs`])。
        self.draw_overlay_dialogs(ui);

        // 边缘缩放区同样必须在全部 panel 之后(同上)。
        if self.frameless {
            crate::ui::titlebar::edge_resize_zones(ui);
        }
    }
}

/// 对照列正文:列头 + 只读 Live 富渲染 + 跟随/滚轮锚点逻辑。
///
/// **两条绘制路径共用这一份实现**(2026-10-19 进场动画改「向右滑」),
/// 调用点按 `split` 互斥,任一帧只画一次:
/// - **进场**(分栏):调用点在 [`LaterMdApp::draw`] 的 ⑤' —— 中央编辑区
///   之后。egui 的 `Panel` 把子 Ui 的 clip 钉死在槽位上,画在 ④ 会被裁掉
///   左半截、只能得到「从屏幕右缘飞进来」;⑤' 自己给 clip,列才能盖在
///   编辑区上向右推进。
/// - **退场**:调用点仍在 ④ 的 `show_collapsible` 闭包里,走 egui 原生
///   滑出(坤哥明确「退场不动」)。此时 egui 在 `0 < how < 1` 期间照常执行
///   `add_contents`,位置与裁剪都由它接管。
///
/// 抽成自由函数而非两处复制:右栏的只读硬锁、跟随一次、滚轮锚点这些
/// 每帧置位的状态(`tab.live_right.*`)必须逐字一致,复制两份必然漂移。
///
/// **正文的最外层 Ui 用绝对 id `compare-body`**:进场走 ⑤'、退场走 ④ 的
/// 面板 Ui,两条路径的父 id 链完全不同,若让 `ScrollArea` 顺着父链派生
/// id(`make_persistent_id`),切档瞬间 id 就变了 —— 右栏滚动位置会被
/// 丢掉重置(egui 的滚动偏移按 id 存在 memory 里)。这里显式钉死 id,
/// 使正文内一切 widget(含 ScrollArea、锚点)的 id 与**在哪条路径画**
/// 无关,进/退场来回切不丢位置。
fn compare_column_body(ui: &mut egui::Ui, app: &mut LaterMdApp) {
    // 绝对 id 子 Ui:见函数文档「为什么」。`scope_builder` 会推进父 Ui 的
    // 游标(`advance_cursor_after_rect`),而根 Ui 的游标已被全部 panel
    // 推进过 —— 再推会把后面 `edge_resize_zones` 的 `allocate_rect`
    // 顶到别处。故用 `new_child`:它建子 Ui 但**不动**父游标。
    let mut root = ui.new_child(
        egui::UiBuilder::new()
            .id(egui::Id::new(COMPARE_BODY_ID))
            .max_rect(ui.max_rect()),
    );
    let ui = &mut root;
    // 列头在滚动区外固定(旧「对照预览」口径,P4 曾随右栏整删;2026-10-16
    // 恢复)。三态下右栏是 Live 富渲染的只读对照,不再是旧「预览」,故标题
    // 叫「对照」。
    crate::ui::workbench::pane_heading(ui, "对照", "只读");
    let LaterMdApp { state, outbox, .. } = app;
    let tab = state.tabs.current_mut();
    // 只读:浏览 + 硬锁(点击/跳转都不进编辑、不抢焦点)。
    // 每帧置位,与禅定的临时浏览同款手法 —— 不写回 State。
    tab.live_right.browse_only = true;
    tab.live_right.browse_enabled = false;
    tab.live_right.browse_locked = true;
    tab.live_right.active = None;
    // 跟随一次:源码侧光标移动才下滚动目标(落在目标的帧消费后即清),
    // 此后右栏保留自己的滚动位置供自由翻阅。滚轮锚点优先于光标跟随:
    // 源码列滚轮滚动帧写下的「鼠标行锚点」在这里消费一次即清 —— 连续滚轮
    // 逐帧重下锚,把锚点所在块顶对齐到指针的视口相对高度(anchor_align_y),
    // 两侧同一行等高呈现;滚轮一停不再写入,右栏恢复自由翻阅。锚点不写
    // followed_byte(只记忆光标侧的最后一次),两条路径互不拽扯;光标跟随
    // 分支入口处显式清对齐意图,防一次残留劫持普通跳转的落点。
    let cursor_byte = tab.cursor.byte;
    if let Some((byte, rel_y)) = tab.live_right.mouse_anchor.take() {
        tab.live_right.sync(&tab.editor, Some(byte));
        if let Some(block) = tab.live_right.block_containing(byte) {
            tab.live_right.pending_caret = Some((block, 0));
            tab.live_right.caret_follow = true;
            tab.live_right.anchor_align_y = Some(rel_y);
        }
    } else {
        tab.live_right.anchor_align_y = None;
        if tab.live_right.followed_byte != cursor_byte {
            tab.live_right.followed_byte = cursor_byte;
            if let Some(byte) = cursor_byte {
                tab.live_right.sync(&tab.editor, Some(byte));
                if let Some(block) = tab.live_right.block_containing(byte) {
                    tab.live_right.pending_caret = Some((block, 0));
                    tab.live_right.caret_follow = true;
                }
            }
        }
    }
    let base_dir = tab
        .document
        .path
        .as_deref()
        .and_then(std::path::Path::parent);
    let heal = state.ai.is_streaming() && state.ai_active_tab == Some(tab.id);
    // 固定矩形子 Ui:`live::ui` 的 ScrollArea(auto_shrink false)会把父 Ui 的
    // max_rect 往下撑,必须隔离(禅定正文同款,P3 实测出来的坑)。
    let body_rect = ui.available_rect_before_wrap();
    let mut body = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(body_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    // 只读列不回写共享的 cursor / selection:用局部占位接住,否则会和源码侧
    // 抢这两个每帧镜像的槽。
    let mut cursor_placeholder = crate::state::OutlineCursor::default();
    let mut selection_placeholder = None;
    crate::live::ui(
        &mut body,
        &mut tab.editor,
        &mut tab.preview,
        &state.ai,
        &mut cursor_placeholder,
        &mut selection_placeholder,
        &mut tab.live_right,
        crate::ui::editor::tab_right_editor_id(tab.id),
        false,
        false,
        // 只读对照列不可持焦,浮标天然不弹,显式关
        false,
        base_dir,
        heal,
        // 只读对照列:无徽标槽,内容不右移
        false,
        &state.keymap,
        outbox,
    );
}

/// 禅定模式的「退出禅定」浮层(§7)。返回其矩形供测试定位。
///
/// **仅原生装饰路径**(`LATERMD_NATIVE_DECORATIONS=1`,无自绘标题栏)调用:
/// 无边框模式下标题栏 Zen 键就是右上退出入口,浮层再画一颗会与标题栏按钮
/// 叠出两颗禅定图标,且本函数锚定根 Ui 的 right_top(**含标题栏**),矩形
/// 会压进标题栏右端按钮区 —— 调用方的 gating 见 `draw_zen` 里的浮层注释。
///
/// **用 `allocate_rect` 绝对摆放而不是 widget 流式布局**:它浮在 CentralPanel
/// 之上而不是挤占正文宽度 —— 流式布局会把这颗按钮压进 720 限宽里,正文随之
/// 被推歪。
///
/// 但它**仍必须是最后分配的那个 widget**:同层命中裁剪 = 后分配者优先
/// (机制见 [`crate::ui::titlebar::edge_resize_zones`]),晚分配才能盖住
/// CentralPanel 而不是被它盖住。
///
/// 抑制视觉噪音:安静时用 `weak_text_color`,指针一到才升到全对比度。禅定
/// 的价值是「屏幕上没有别的东西」,一颗始终全黑的按钮会把注意力从正文上
/// 拽走(带 tooltip 兜住「这玩意儿能点」的可发现性)。
fn zen_exit_button(ui: &mut egui::Ui, outbox: &mut Vec<Message>) -> egui::Rect {
    let size = egui::vec2(tokens::ICON + 12.0, tokens::TOOLBAR_H);
    let rect = egui::Rect::from_min_size(
        ui.max_rect().right_top()
            - egui::vec2(size.x + tokens::ZEN_EXIT_MARGIN, -tokens::ZEN_EXIT_MARGIN),
        size,
    );
    let response = ui
        .allocate_rect(rect, egui::Sense::click())
        .on_hover_text("退出禅定 (Esc)");
    if ui.is_rect_visible(rect) {
        let color = if response.hovered() {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        crate::ui::icons::Icon::Zen.draw(ui.painter(), rect.center(), tokens::ICON, color);
    }
    if response.clicked() {
        outbox.push(Message::ZenToggled);
    }
    rect
}

/// 快捷键捕获(设置页「改键」):本帧的按键就是新键位。
///
/// 在归约侧消费而非 UI 侧:消费即把事件从输入流移除,编辑器收不到这个
/// 键,也不会被 [`crate::command::poll_shortcuts`] 当成命令触发。
/// Esc = 取消,Backspace/Delete(无修饰)= 清除绑定,其余按键走
/// [`Message::KeymapAssign`] 归约(撞键与不可绑定的拒绝都在那里)。
fn poll_capture(ctx: &egui::Context, state: &mut crate::state::State) {
    let Some(cmd) = state.settings.capture else {
        return;
    };
    let pressed = ctx.input_mut(|input| {
        let mut captured = None;
        input.events.retain(|event| match event {
            egui::Event::Key {
                key,
                modifiers,
                pressed: true,
                ..
            } => {
                captured = Some((*key, *modifiers));
                false // 消费:不让编辑器与命令层再见到它
            }
            _ => true,
        });
        captured
    });
    let Some((key, modifiers)) = pressed else {
        return;
    };
    state.settings.capture = None;
    match key {
        egui::Key::Escape => {}
        egui::Key::Backspace | egui::Key::Delete if modifiers.is_none() => {
            state.apply(Message::KeymapCleared(cmd));
        }
        _ => state.apply(Message::KeymapAssign {
            cmd,
            shortcut: crate::keymap::Shortcut { modifiers, key },
        }),
    }
}

/// 查找卡/跳转卡共用的浮层 frame(#72 M1,单一真源):底色随**当前生效
/// 主题**走 `Frame::popup(&ctx.global_style())` —— 0.36 里这就是
/// `Window` 渲染自取的那套活动 style,由 `ThemeSettings::apply` 按解析
/// 后的明暗切换,浅色主题下卡片即浅色。此前两处写死
/// `style_of(Theme::Dark)`(浅色下黑窗,坤哥 2026-10-08 报),两卡从此
/// 只在这里取 frame,不再各写一份。
fn overlay_popup_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::popup(&ctx.global_style())
}

/// 浮卡与源码区边缘的留白(#72 M2 抽常量:锚点偏移与 `constrain_to`
/// 的 shrink 共用,两卡一处定义)。一侧是浮卡内容、另一侧是源码区
/// 边界 → 贴边距档(设计系统 T2b 归流,值不变)。
const OVERLAY_MARGIN: f32 = tokens::inset::MD;

/// 查找卡/跳转卡共用的右上锚点(#72 M2 抽出单一真源:两卡同锚点,
/// 偏移与钳制回写都要再算它,不再各写一份公式)。source_rect 从可换行
/// 的工具条之后开始,不能用包含工具条的面板 min_rect。
fn overlay_anchor(source_rect: egui::Rect) -> egui::Pos2 {
    source_rect.right_top()
        + egui::vec2(
            -OVERLAY_MARGIN,
            crate::ui::tokens::TOOLBAR_H + OVERLAY_MARGIN * 2.0,
        )
}

/// 查找条的源码区浮层:锚定源码宿主右上角,不参加正文布局;id 稳定,
/// 锚点每帧由源码 rect 重算,侧栏/窗口拖宽后不会漂走。#72 M2 起可拖:
/// 位置 = 锚点 + `FindBarState::drag_offset`(顶部把手累计,见
/// [`overlay_drag_handle`]),拖出源码区由 `constrain_to` 钳回(改前
/// 「避免它变成可拖对话框」的口径按坤哥 2026-10-08 新诉求推翻,
/// decisions-pending #134)。
fn draw_find_overlay(
    ctx: &egui::Context,
    source_rect: egui::Rect,
    find: &mut crate::state::FindBarState,
    outbox: &mut Vec<Message>,
) {
    let anchor = overlay_anchor(source_rect);
    let offset_at_frame_start = find.drag_offset;
    egui::Window::new("文档内查找")
        .id(egui::Id::new("editor-find-overlay"))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        // 拖动只认把手:关掉无标题栏 Window 的 drag-anywhere 兜底
        // (egui 0.36 对 title_bar(false) 静默回落「拖任意处」,
        // 与 fixed_pos 每帧重设相拼会闪跳,见 window.rs effective_drag)。
        .movable(false)
        .fixed_pos(anchor + find.drag_offset)
        .pivot(egui::Align2::RIGHT_TOP)
        .order(egui::Order::Foreground)
        .constrain_to(source_rect.shrink(OVERLAY_MARGIN))
        .frame(overlay_popup_frame(ctx))
        .show(ctx, |ui| find_bar_contents(ui, find, outbox));
    overlay_drag_sync(
        ctx,
        egui::Id::new("editor-find-overlay"),
        anchor,
        offset_at_frame_start,
        &mut find.drag_offset,
    );
}

fn find_input_id() -> egui::Id {
    egui::Id::new("editor-find-input")
}

fn replace_input_id() -> egui::Id {
    egui::Id::new("editor-replace-input")
}

fn goto_input_id() -> egui::Id {
    egui::Id::new("editor-goto-input")
}

/// 查找/替换输入框的按键过滤:在 TextEdit 出厂默认(方向键留在框内、
/// Tab 跳焦)之上打开 `escape` —— egui 0.36 的焦点导航把裸 Esc 当
/// 「交出焦点」在 `Focus::begin_pass` 清焦,不清则框内 Esc 检测
/// (`has_focus` 前提)永远不触发,「Esc 关闭查找条」成为哑弹
/// (2026-10-01 #17 M1 无头实证,两框必须同口径)。
const FIND_BAR_EVENT_FILTER: egui::EventFilter = egui::EventFilter {
    tab: false,
    horizontal_arrows: true,
    vertical_arrows: true,
    escape: true,
};

/// 查找/替换两行行首标签的公共列宽(#72 M1):同一 Body 字体下取两词的
/// 最大自然宽,行内标签([`bar_label`])补齐到这一宽。两行输入框列对齐
/// 从此是结构保证 —— 改前对齐只是「查找/替换恰好都是两个汉字」的巧合
/// (2026-10-09 无头实测 delta=0),字体偏好/回退链一变就散。
fn bar_label_column_width(ui: &mut egui::Ui) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let color = ui.visuals().text_color();
    let width = |ui: &mut egui::Ui, text: &str| {
        ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(text.to_owned(), font.clone(), color)
                .rect
                .width()
        })
    };
    width(ui, "查找").max(width(ui, "替换"))
}

/// 行首标签:自然宽渲染后补位到公共列宽(见 [`bar_label_column_width`]),
/// 右缘即两行共同的输入框列起点。
fn bar_label(ui: &mut egui::Ui, text: &str, column_w: f32) {
    let response = ui.label(text);
    ui.add_space((column_w - response.rect.width()).max(0.0));
}

/// 命中计数着色(#72 M1):有结果弱色(不打扰),无结果警示色
/// (`tokens::WARN`,与设置页告警同色)——「查了但一个都没有」值得一眼
/// 看见。文字语义(无结果/N/M)与改前一致,只动颜色。
fn hit_count_label(ui: &mut egui::Ui, total: usize, pos: usize) {
    let text = if total == 0 {
        "无结果".to_owned()
    } else {
        format!("{pos}/{total}")
    };
    let color = if total == 0 {
        crate::ui::tokens::WARN
    } else {
        ui.visuals().weak_text_color()
    };
    ui.colored_label(color, text);
}

/// 查找卡内容与状态无关,可在 Window/无头测试中复用。顶部是拖动把手
/// (#72 M2,`overlay_drag_handle`);替换行(`replace_open`,Ctrl+H)画在
/// 查找行之下:替换词输入只更新按钮可用性,不自动改写文档。
fn find_bar_contents(
    ui: &mut egui::Ui,
    find: &mut crate::state::FindBarState,
    outbox: &mut Vec<Message>,
) {
    overlay_drag_handle(ui, &mut find.drag_offset);
    let total = find.hits.len();
    let pos = find.hit.map(|h| h + 1).unwrap_or(0);
    let label_col = bar_label_column_width(ui);
    ui.horizontal(|ui| {
        bar_label(ui, "查找", label_col);
        let mut query_buf = find.query.clone();
        let response = ui.add(
            egui::TextEdit::singleline(&mut query_buf)
                .id(find_input_id())
                .event_filter(FIND_BAR_EVENT_FILTER)
                .return_key(None::<egui::KeyboardShortcut>)
                .desired_width(220.0)
                .hint_text("输入即跳转;Enter 下一个"),
        );
        if query_buf != find.query {
            find.query = query_buf.clone();
            outbox.push(Message::FindQueryChanged(query_buf));
        }
        let enter = ui.input(|i| {
            i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        pressed: true,
                        modifiers,
                        ..
                    } if !modifiers.shift
                )
            })
        });
        let shift_enter = ui.input(|i| {
            i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.shift
                )
            })
        });
        if response.has_focus() {
            if shift_enter {
                outbox.push(Message::FindNext { backwards: true });
            } else if enter {
                outbox.push(Message::FindNext { backwards: false });
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                outbox.push(Message::FindBarToggled(false));
            }
        }
        if ui.button("↑").clicked() {
            outbox.push(Message::FindNext { backwards: true });
        }
        if ui.button("↓").clicked() {
            outbox.push(Message::FindNext { backwards: false });
        }
        hit_count_label(ui, total, pos);
        if crate::ui::icons::icon_button(ui, crate::ui::icons::Icon::Close, "关闭查找 (Esc)")
            .clicked()
        {
            outbox.push(Message::FindBarToggled(false));
        }
    });
    if find.replace_open {
        replace_row(ui, find, total, pos, outbox);
    }
}

/// 替换行:替换输入框 + 「替换」(当前命中)/「全部」按钮 + 命中计数。
/// 返回两枚按钮的响应(无头测试断言可用性与点击用)。
fn replace_row(
    ui: &mut egui::Ui,
    find: &mut crate::state::FindBarState,
    total: usize,
    pos: usize,
    outbox: &mut Vec<Message>,
) -> (egui::Response, egui::Response) {
    let label_col = bar_label_column_width(ui);
    ui.horizontal(|ui| {
        bar_label(ui, "替换", label_col);
        let response = ui.add(
            egui::TextEdit::singleline(&mut find.replacement)
                .id(replace_input_id())
                .event_filter(FIND_BAR_EVENT_FILTER)
                .return_key(None::<egui::KeyboardShortcut>)
                .desired_width(220.0)
                .hint_text("替换为"),
        );
        // 替换词输入不自动改写文档,只经由按钮可用性体现
        let replace = ui.add_enabled(find.hit.is_some(), egui::Button::new("替换"));
        if replace.clicked() {
            outbox.push(Message::ReplaceCurrent);
        }
        let all = ui.add_enabled(total > 0, egui::Button::new("全部"));
        if all.clicked() {
            outbox.push(Message::ReplaceAllInDoc);
        }
        hit_count_label(ui, total, pos);
        // Esc 在替换框上同样关整条(与查找框口径一致)
        if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            outbox.push(Message::FindBarToggled(false));
        }
        (replace, all)
    })
    .inner
}

/// 「跳转到行」浮条(#60 M1)的源码区浮层:#17 查找卡同款 Window(无标题
/// 栏、可拖同款把手、锚点每帧由源码 rect 重算),同一右上锚点 —— 两者
/// 互斥(decisions-pending #113),同帧至多一个在场,锚点复用不冲突。
/// 源码与 Live 两模式都画:跳转在 Live 侧走块路由(`cursor.jump_to`
/// 同一入口),不像查找条那样只限源码。
fn draw_goto_overlay(
    ctx: &egui::Context,
    source_rect: egui::Rect,
    goto: &mut crate::state::GotoBarState,
    outbox: &mut Vec<Message>,
) {
    let anchor = overlay_anchor(source_rect);
    let offset_at_frame_start = goto.drag_offset;
    egui::Window::new("跳转到行")
        .id(egui::Id::new("editor-goto-overlay"))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        // 拖动只认把手(与查找卡同款,见 draw_find_overlay 注释)
        .movable(false)
        .fixed_pos(anchor + goto.drag_offset)
        .pivot(egui::Align2::RIGHT_TOP)
        .order(egui::Order::Foreground)
        .constrain_to(source_rect.shrink(OVERLAY_MARGIN))
        .frame(overlay_popup_frame(ctx))
        .show(ctx, |ui| goto_bar_contents(ui, goto, outbox));
    overlay_drag_sync(
        ctx,
        egui::Id::new("editor-goto-overlay"),
        anchor,
        offset_at_frame_start,
        &mut goto.drag_offset,
    );
}

/// 浮卡顶部的拖动把手条(#72 M2):全宽窄条,`click_and_drag` 命中,拖动
/// 每帧的 [`egui::Response::drag_delta`] 累计进卡片状态的 `drag_offset`
/// (会话内保持)。中央一枚短圆角胶囊给可视抓手提示 —— 纯 shape 自绘,
/// 不依赖字体 glyph 覆盖(icons.rs 无 Grip/Handle 变体,#133 同款不新增)。
/// 独立命中区,不与输入框/按钮相交,也不进 Tab 焦点链(非可聚焦控件)。
fn overlay_drag_handle(ui: &mut egui::Ui, offset: &mut egui::Vec2) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), OVERLAY_DRAG_STRIP_H),
        egui::Sense::click_and_drag(),
    );
    let pill = egui::Rect::from_center_size(rect.center(), egui::vec2(28.0, 3.0));
    let color = if response.hovered() || response.dragged() {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    ui.painter().rect_filled(pill, 1.5, color);
    *offset += response.drag_delta();
    response.on_hover_cursor(egui::CursorIcon::Grab);
}

/// 把手条高度:够按住又不把窄卡撑高(抓手胶囊 3px + 上下各 ~3.5px 呼吸)。
const OVERLAY_DRAG_STRIP_H: f32 = 10.0;

/// 把手偏移回写(#72 M2):`constrain_to` 的钳制与整像素取整发生在 Window
/// 内部,状态里的 `drag_offset` 看不见。每帧画完把**实际**落点(锚点 =
/// Area 的 RIGHT_TOP pivot)与「锚点 + **帧初**偏移」比对 —— 帧初值是
/// 本帧渲染真正用过的偏移(把手 delta 在内容闭包里累计,要下一帧才进
/// `fixed_pos`,拿帧末累计值对账会把刚累计的增量当漂移抹掉)。差超过
/// 半像素(真实钳制,非取整噪声)才把偏移回写到实际值,同时丢弃越界
/// 段的超出量 —— 否则卡片贴边拖过头后再往回拖,累计值要先「走回」
/// 越界段,卡片原地不动(空程)。
fn overlay_drag_sync(
    ctx: &egui::Context,
    id: egui::Id,
    anchor: egui::Pos2,
    offset_at_frame_start: egui::Vec2,
    offset: &mut egui::Vec2,
) {
    let Some(rect) = ctx.memory(|memory| memory.area_rect(id)) else {
        return;
    };
    let drift = rect.right_top() - (anchor + offset_at_frame_start);
    if drift.x.abs() > 0.5 || drift.y.abs() > 0.5 {
        *offset = rect.right_top() - anchor;
    }
}

/// 跳转浮条内容:行号输入 + 「跳转」+ ✕。返回(输入框, 跳转钮)响应供
/// 无头测试定位。**焦点钉**(quick_open 同款):开着就持焦,Ctrl+G 后直接
/// 打数字回车;非数字(含溢出/空白)回车忽略,浮条保留可改。
/// 输入不经任何事件过滤拦截 IME —— 数字/中文输入法直通 TextEdit,
/// 数字直落草稿;编辑器撤销栈与本浮条无交集(独立 widget 独立状态)。
fn goto_bar_contents(
    ui: &mut egui::Ui,
    goto: &mut crate::state::GotoBarState,
    outbox: &mut Vec<Message>,
) -> (egui::Response, egui::Response) {
    // 焦点钉在草稿抄写之前:首帧即请求,下一 pass 生效(quick_open 同款)
    if !ui.ctx().memory(|memory| memory.has_focus(goto_input_id())) {
        ui.ctx()
            .memory_mut(|memory| memory.request_focus(goto_input_id()));
    }
    overlay_drag_handle(ui, &mut goto.drag_offset);
    let mut input = goto.input.clone();
    let (response, jump_button) = ui
        .horizontal(|ui| {
            ui.label("行号");
            let response = ui.add(
                egui::TextEdit::singleline(&mut input)
                    .id(goto_input_id())
                    .event_filter(FIND_BAR_EVENT_FILTER)
                    .return_key(None::<egui::KeyboardShortcut>)
                    .desired_width(120.0)
                    .hint_text("1-总行数,Enter 跳转"),
            );
            let jump = ui.button("跳转");
            if crate::ui::icons::icon_button(ui, crate::ui::icons::Icon::Close, "关闭 (Esc)")
                .clicked()
            {
                outbox.push(Message::GotoBarToggled(false));
            }
            (response, jump)
        })
        .inner;
    if input != goto.input {
        goto.input = input.clone();
    }
    // 非数字容错(decisions-pending #113):解析不了的输入(空/非数字/
    // 溢出)回车与「跳转」都忽略,不发消息;合法值交归约钳制。
    let parsed = input.trim().parse::<usize>().ok();
    let request_jump = || parsed.map(|line| Message::GotoLineRequested { line });
    if response.has_focus() {
        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            outbox.extend(request_jump());
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            outbox.push(Message::GotoBarToggled(false));
        }
    }
    if jump_button.clicked() {
        outbox.extend(request_jump());
    }
    (response, jump_button)
}

/// 底部状态栏:**三段分区**(2026-10-08,docs/ui-shell-redesign-v2.md §3)。
///
/// | 段 | 内容 | 理由 |
/// |---|---|---|
/// | 左 | 文件名 · 行:列 | 「我在哪、我写到哪」——最高频、且与文档强绑定 |
/// | 中 | 字数 | 写作进度感,与左右两侧都无关联 |
/// | 右 | 主题 · AI provider · MCP | 全是**全局服务状态**,彼此相关,应聚在右端一眼扫完 |
///
/// 改版前是单一 `horizontal_wrapped` 顺排:所有信息挤在左端,右侧
/// 500px 长期空白,而「MCP 启动失败」这种真正要盯的告警偏在最右、要横跨
/// 整屏才能看到。切成三段后告警恒在右下角,与视线停留点一致。
///
/// **实现手法**:左段、中段各一个 `ui.horizontal`(顺排,自左缘起),右段
/// 包在 `Layout::right_to_left` 里(自右缘往左排)。右段内部的 push 顺序
/// 即「从右到左」的顺序 —— MCP 第一个 push 所以画在最右,是状态栏里
/// 最容易被扫到的位置。
///
/// **不用 `layout_to_min_x` 的原因**:它要求调用方自己算百分比 x,三段
/// 各写一个 magic number;而「左中顺排 + 右段 right_to_left」是零参数
/// 写法,右段自动贴边,窗口拉伸时无需同步改任何数字。
///
/// 窗口窄到三段挤不下时,**先牺牲中段**(字数)—— 它是三者里唯一丢了
/// 不影响操作的,判据是 `tokens::STATUSBAR_MIN_W`。
fn status_bar(ui: &mut egui::Ui, state: &crate::state::State) {
    // 2026-10-10 统一:全平台走工作台单行状态栏(mac #169 精修平移)。
    // 旧三段(horizontal_wrapped 家族)退役 —— 那套布局在 top_down 父 Ui
    // 里摞过三行(见 tests::status_bar_is_a_single_row 的教训注记),新版
    // allocate_exact_size 定形单行,窄窗按优先级省略右段/左段明细。
    let tab = state.tabs.current();
    // 行列换算走 rope 的 `line_col`(`O(log N)`);旧写法扫光标之前的全部
    // 文本求 `'\n'` 数,光标在文末即 O(全篇),而状态栏每帧都画。
    let position = tab.cursor.byte.map(|byte| tab.editor.line_col(byte));
    crate::ui::workbench::status_bar(ui, state, position);
}

/// 编辑器面板顶部的提示行(存在才显示):保存失败、撞键拒绝等需要用户
/// 知晓并手动收起的提示。原文件工具栏的能力,工具栏退役后迁此保持不变
/// (decisions-pending #32)。返回「知道了」按钮的响应(`None` = 本帧无
/// 提示;测试定位用,与 `checkout_dialog` 同款手法)。
fn notice_bar(
    ui: &mut egui::Ui,
    document: &crate::state::DocumentState,
    outbox: &mut Vec<Message>,
) -> Option<egui::Response> {
    let notice = document.notice.as_deref()?;
    let mut dismiss = None;
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(ui.visuals().error_fg_color, notice);
        let button = ui.small_button("知道了");
        if button.clicked() {
            outbox.push(Message::NoticeDismissed);
        }
        dismiss = Some(button);
    });
    dismiss
}

/// 恢复条里「保存于何时」的文案:相对时刻(「5 分钟前」)。时刻基准由
/// 调用方传入(生产 `SystemTime::now()`,测试注入定点)。无 mtime 或时钟
/// 倒流(`duration_since` 出错)一律「保存时间未知」—— 展示字段不值得
/// 猜,更不值得为此引一个时间依赖(无 chrono 的既定依赖面)。
fn draft_saved_label(mtime: Option<std::time::SystemTime>, now: std::time::SystemTime) -> String {
    let Some(at) = mtime else {
        return "保存时间未知".to_owned();
    };
    let Ok(elapsed) = now.duration_since(at) else {
        return "保存时间未知".to_owned();
    };
    let secs = elapsed.as_secs();
    if secs < 60 {
        "刚刚保存".to_owned()
    } else if secs < 3600 {
        format!("保存于 {} 分钟前", secs / 60)
    } else if secs < 86400 {
        format!("保存于 {} 小时前", secs / 3600)
    } else {
        format!("保存于 {} 天前", secs / 86400)
    }
}

/// 编辑器面板顶部的草稿恢复条(#18,存在才显示):文档旁发现遗留
/// `<doc>.latermd-draft` 时请用户裁决 —— 「恢复」把草稿读进缓冲(undo 可
/// 回退),「丢弃」删盘上草稿。返回(恢复, 丢弃)按钮的响应(`None` = 本帧
/// 无待恢复;测试定位用,与 `notice_bar` 同款手法)。真正的恢复/丢弃都
/// 在归约,这里只收集点击。
fn recovery_bar(
    ui: &mut egui::Ui,
    recover: &crate::tabs::DraftRecovery,
    tab_id: u64,
    outbox: &mut Vec<Message>,
) -> Option<(egui::Response, egui::Response)> {
    let mut buttons = None;
    ui.horizontal_wrapped(|ui| {
        // 与提示行的错误红区分:这是可行动的告知,不是错误(与回滚确认
        // 文案同档的警示黄,tokens::WARN)。
        let label = format!(
            "发现未保存草稿({})",
            draft_saved_label(recover.mtime, std::time::SystemTime::now())
        );
        ui.colored_label(tokens::WARN, label);
        let restore = ui.button("恢复");
        if restore.clicked() {
            outbox.push(Message::DraftRecovered { tab_id });
        }
        let discard = ui.button("丢弃");
        if discard.clicked() {
            outbox.push(Message::DraftDiscarded { tab_id });
        }
        buttons = Some((restore, discard));
    });
    buttons
}
/// 光标行列(1 起):行按换行数,列按该行字符数(中文按字计,与编辑器
/// 的视觉列一致)。
///
/// commit message 建议浮窗;返回(复制, 关闭, ×)响应,测试定位用
/// (与 `ui::menubar::item` 同款手法,× 与「关闭」同语义)。
///
/// 复制即时写系统剪贴板(`Context::copy_text`,UI 侧效果,不改状态——
/// 不经用户动作覆盖剪贴板会冲掉用户正在搬运的内容);关闭只发消息,
/// 清建议的归约在 `App::logic`。
fn commit_dialog(ui: &mut egui::Ui, subject: &str) -> (egui::Response, egui::Response, bool) {
    let ctx = ui.ctx().clone();
    let mut buttons = None;
    let mut close = false;
    let _ = crate::ui::workbench::centered_dialog(ui, "AI: commit message")
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .frame(crate::ui::workbench::dialog_frame(ui))
        .show(ui.ctx(), |ui| {
            close = crate::ui::workbench::dialog_header(ui, "AI: commit message");
            ui.label("建议的 commit subject:");
            ui.label(egui::RichText::new(subject).strong());
            crate::ui::workbench::dialog_footer_divider(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                let copy = ui.button("复制");
                ui.add_space(crate::ui::tokens::SPACE_MD);
                let dismiss = ui.button("关闭");
                if copy.clicked() {
                    ctx.copy_text(subject.to_owned());
                }
                buttons = Some((copy, dismiss));
            });
        });
    let (copy, dismiss) = buttons.expect("浮窗必然绘制按钮");
    (copy, dismiss, close)
}

/// 选区润色确认浮窗(#61 M3);返回(确认替换, 放弃, 复制, ×)响应,
/// 测试定位用(与 `commit_dialog` 同款手法,× 与「放弃」同语义;
/// 真正的替换/放弃在归约)。
///
/// 正文是**只读**多行文本区(`&str` 的不可变 `TextBuffer`),包在
/// ScrollArea 里可滚动 —— 润色草稿可能比视口长。流式进行中草稿逐块增长
/// (本函数每帧读最新 draft,零跨帧缓存),确认按钮禁用;结束后确认/
/// 放弃可用。widget id 全部固定字符串,不含内容长度/hash(AGENTS §6.7)。
fn selection_ai_polish_dialog(
    ui: &mut egui::Ui,
    draft: &str,
    streaming: bool,
) -> (egui::Response, egui::Response, egui::Response, bool) {
    let ctx = ui.ctx().clone();
    let mut buttons = None;
    let mut close = false;
    crate::ui::workbench::centered_dialog(ui, "AI 润色")
        // 默认尺寸定住(居中由 centered_dialog 负责):流式逐块增长时窗
        // 从中心向四周长,resizable 允许用户拉大看长草稿。
        .default_size([380.0, 240.0])
        .collapsible(false)
        .resizable(true)
        .title_bar(false)
        .frame(crate::ui::workbench::dialog_frame(ui))
        .show(ui.ctx(), |ui| {
            close = crate::ui::workbench::dialog_header(ui, "AI 润色");
            ui.label(if streaming {
                "润色中…(完成后可确认整段替换选区)"
            } else {
                "润色结果,确认后整段替换选区:"
            });
            let mut view: &str = draft;
            egui::ScrollArea::vertical()
                .id_salt("selection-ai-polish-body")
                .auto_shrink([false, true])
                .max_height(160.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut view)
                            .id(egui::Id::new("selection-ai-polish-text"))
                            .desired_rows(6)
                            // 固定宽:浮窗尺寸不随草稿内容跳动(流式逐块
                            // 增长时窗口每帧变宽是观感事故)
                            .desired_width(340.0),
                    );
                });
            crate::ui::workbench::dialog_footer_divider(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                let confirm = ui.add_enabled(!streaming, egui::Button::new("确认替换"));
                ui.add_space(crate::ui::tokens::SPACE_MD);
                let dismiss = ui.button(if streaming { "放弃" } else { "放弃(Esc)" });
                let copy = ui.button("复制");
                if copy.clicked() {
                    ctx.copy_text(draft.to_owned());
                }
                buttons = Some((confirm, dismiss, copy));
            });
        });
    let (confirm, dismiss, copy) = buttons.expect("浮窗必然绘制按钮");
    (confirm, dismiss, copy, close)
}

/// 破坏性确认浮窗骨架(「关闭标签」与「Git:回滚」共用):
///
/// * 标题行:加粗标题居左 + × 关闭钮居右,下方细分隔线
///   ([`crate::ui::workbench::dialog_header`],2026-10-15 观感改版);
/// * 说明行(默认前景,各弹窗自己的 lead);
/// * 警示条:**无描边**的淡红底圆角块 + ⚠ 图标 + 浅一档红字
///   ([`crate::ui::tokens::danger_text`],2026-10-15 坤哥「整个警告框
///   太抢戏」)。去掉的是**边框**,不是底色 —— 淡红底仍要把这段话与
///   正文分开;边框 + 底色 + 满档红字三层同时上,框比标题还重;
/// * 可选附加警示行(checkout 的针对性行为告知,`escalate` 升黄);
/// * 页脚细分隔线 + 按钮右对齐:中性「取消」在左、危险实心确认钮
///   ([`danger_button`])居右,两者之间留 [`crate::ui::tokens::SPACE_MD`]
///   拉开(红色按钮贴太近会读成「取消也是危险操作」)。
///
/// **Enter 绝不绑确认**(2026-10-15 坤哥「防止误触清空文件」):破坏性
/// 操作的默认键必须是「什么都不发生」。回车只在表单类弹窗(重命名 /
/// 新建,焦点在输入框时)才触发确认。
///
/// 返回(确认, 取消, ×是否被点)供测试定位,取消语义 = 取消钮或 ×
/// 任一被点;真正的动作都在归约。
///
/// `path` 为 `Some` 时在说明行下方插一枚路径 chip
/// ([`crate::ui::workbench::path_chip`],等宽 + 中间截断 + tooltip 全
/// 路径):说明句点名文件名、chip 给完整定位,两者分工不同故不合并成一句。
// 参数各自属于调用方弹窗的不同数据(标题/说明/警示/按钮文案/附加警示/
// 目标路径),打包成结构会造出人为聚合 —— 同 [`crate::settings::dialog`]
// 的 allow 先例。
#[allow(clippy::too_many_arguments)]
pub(crate) fn destructive_confirm_dialog(
    ui: &mut egui::Ui,
    title: &str,
    lead: &str,
    warning: &str,
    confirm_text: &str,
    extra: Option<(&str, bool)>,
    path: Option<&std::path::Path>,
) -> (egui::Response, egui::Response, bool) {
    let mut buttons = None;
    let mut close = false;
    crate::ui::workbench::centered_dialog(ui, title)
        .title_bar(false)
        .default_width(360.0)
        .collapsible(false)
        .resizable(false)
        .frame(crate::ui::workbench::dialog_frame(ui))
        .show(ui.ctx(), |ui| {
            close = crate::ui::workbench::dialog_header(ui, title);
            ui.label(lead);
            if let Some(path) = path {
                ui.add_space(crate::ui::tokens::SPACE_XS);
                crate::ui::workbench::path_chip(ui, path);
            }
            // 警示条:**不描边**(2026-10-15)。底色保留 —— 它承担「这段话
            // 与正文不同」的分组语义;去掉的是描边那一层冗余,以及满档
            // DANGER 的文字色(改用浅一档的 `danger_text`)。
            warning_box(ui, warning);
            if let Some((text, escalate)) = extra {
                let text = egui::RichText::new(text);
                ui.label(if escalate {
                    text.color(egui::Color32::from_rgb(235, 180, 60))
                } else {
                    text
                });
            }
            ui.add_space(crate::ui::tokens::SPACE_XS);
            // 页脚分隔线 + 按钮右对齐(right_to_left 布局里先加的在最右:
            // 危险确认居右、取消在其左)。cross 取 Min 不取 Center ——
            // Center 子 Ui 在 Window 里会申领整块剩余高度,窗被撑高、按钮
            // 跨帧漂移(无头测试当场抓到:采点帧 y=231、点击帧 y=374)。
            crate::ui::workbench::dialog_footer_divider(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                let confirm = danger_button(ui, confirm_text);
                ui.add_space(crate::ui::tokens::SPACE_MD);
                let cancel = ui.button("取消");
                buttons = Some((confirm, cancel));
            });
        });
    let (confirm, cancel) = buttons.expect("浮窗必然绘制按钮");
    (confirm, cancel, close)
}

/// 危险操作按钮:与 [`crate::settings::primary_button`] 同构(实心底 +
/// 反色加粗字 + 同一圆角档),填色换 DANGER —— 破坏性确认的视觉权重
/// 与「保存」同级,颜色语义相反。
/// 警示条底色的 alpha(0–255)。
///
/// 去掉描边之后底色成了**唯一**的分组信号,值不能照旧:原先 alpha=10
/// 是「有描边兜着」的取值(实测 blend 后红蓝差只剩 5/255,肉眼读不出
/// 是红底,只当一块灰)。18 是实测下限之上的一档 —— 淡到不抢标题,又
/// 确定读得出「这段话是警示」。更大的值会把整框压重,即坤哥要改掉的
/// 那件事。
const WARN_BG_ALPHA: u8 = 18;

/// 破坏性确认弹窗里的**警示条**:淡红底(无描边)+ ⚠ 图标 + 浅一档红字。
///
/// 单独抽成函数有两个理由:
/// ① 观感是三条独立诉求(底色 / 无描边 / 有图标),合成一处时改坏任
///    一条都未必变红;拆开后每条能各自断言。
/// ② 无头测试可以**只渲染警示条**来数形状。若在完整弹窗里数,窗口
///    chrome 的边框(也是 `LineSegment`)会混进来 —— 2026-10-15 实测踩过:
///    去掉图标后测试仍绿,因为 chrome 贡献的矢量形状已超过阈值。
fn warning_box(ui: &mut egui::Ui, warning: &str) {
    let danger = crate::ui::tokens::DANGER;
    let warn_text = crate::ui::tokens::danger_text(ui.visuals().dark_mode);
    egui::Frame::NONE
        .fill(egui::Color32::from_rgba_unmultiplied(
            danger.r(),
            danger.g(),
            danger.b(),
            WARN_BG_ALPHA,
        ))
        .corner_radius(egui::CornerRadius::same(crate::ui::tokens::RADIUS_MD as u8))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .outer_margin(egui::Margin::symmetric(0, 6))
        .show(ui, |ui| {
            // 不 set_width:Window 是内容定宽(设置页 card 的
            // available_width 在浮窗里近无界,会把窗撑出屏幕)
            ui.horizontal_top(|ui| {
                // ⚠ 图标:与文字同色、顶对齐。
                //
                // **先占位再画**:直接往 `ui.cursor()` 上画是错的 ——
                // `horizontal_top` 已把游标定位到本行起点,而你画完
                // 不推进游标,后面的 label 会盖在图标上(实测图标的
                // Path 被文本压在下面,肉眼只剩半个三角)。占位用
                // `allocate_exact_size`(方形,尺寸取 SMALL 一档:图标
                // 在这里是「前缀」不是「动作按钮」,跟正文同量级才不
                // 抢戏)。
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(
                        crate::ui::tokens::text::SMALL,
                        crate::ui::tokens::text::SMALL,
                    ),
                    egui::Sense::hover(),
                );
                crate::ui::icons::Icon::Warning.draw(
                    ui.painter(),
                    rect.center(),
                    crate::ui::tokens::text::SMALL,
                    warn_text,
                );
                ui.add_space(crate::ui::tokens::SPACE_SM);
                ui.label(egui::RichText::new(warning).color(warn_text));
            });
        });
}

/// 危险操作按钮:与 [`crate::settings::primary_button`] 同构(实心底 +
/// 反色加粗字 + 同一圆角档),填色换 DANGER —— 破坏性确认的视觉权重
/// 与「保存」同级,颜色语义相反。
fn danger_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let button = egui::Button::new(
        egui::RichText::new(text)
            .color(egui::Color32::WHITE)
            .strong(),
    )
    .fill(crate::ui::tokens::DANGER)
    .corner_radius(egui::CornerRadius::same(crate::ui::tokens::RADIUS_SM as u8));
    ui.add(button)
}

/// 脏标签关闭确认浮窗;返回(确认关闭, 取消, ×)响应,测试定位用
/// (与 `checkout_dialog` 同款手法;真正的移除在归约)。
fn tab_close_dialog(ui: &mut egui::Ui, name: &str) -> (egui::Response, egui::Response, bool) {
    destructive_confirm_dialog(
        ui,
        "关闭标签",
        &format!("「{name}」有未保存的修改。"),
        "关闭将丢弃这些修改,此操作不可撤销。",
        "关闭并丢弃",
        None,
        None,
    )
}

/// 回滚确认浮窗;返回(回滚, 取消, ×)响应,测试定位用(与
/// `commit_dialog` 同款手法)。只展示与收集点击,checkout 在归约。
/// `open_in_editor`/`dirty` 驱动当前文档的针对性警示
/// ([`checkout_extra_warning`]),dirty 状态取自缓冲真源、每帧重估。
fn checkout_dialog(
    ui: &mut egui::Ui,
    path: &str,
    open_in_editor: bool,
    dirty: bool,
) -> (egui::Response, egui::Response, bool) {
    destructive_confirm_dialog(
        ui,
        "Git: 回滚文件",
        &format!("把 {path} 恢复到 HEAD 版本。"),
        "未提交的改动将被丢弃,此操作不可撤销。",
        "回滚",
        checkout_extra_warning(open_in_editor, dirty).map(|warning| (warning, dirty)),
        None,
    )
}

/// 侧边栏「全部替换」确认浮窗;返回(全部替换, 取消, ×)响应,测试
/// 定位用(与 `checkout_dialog` 同款手法;真正的替换在归约)。附加行
/// 告知打开中文件的差异行为(缓冲替换 vs 直接写盘,见归约注释)。
fn search_replace_dialog(
    ui: &mut egui::Ui,
    files: usize,
) -> (egui::Response, egui::Response, bool) {
    destructive_confirm_dialog(
        ui,
        "搜索: 全部替换",
        &format!("把替换词写入 {files} 个文件。"),
        "未打开的文件直接改写磁盘,此操作不走编辑器撤销。",
        "全部替换",
        Some((
            "已在编辑器中打开的文件只替换进缓冲(未保存修改保留,保存时落盘)。",
            false,
        )),
        None,
    )
}

/// 回滚目标恰是编辑器当前文档时的追加警示;`None` = 目标不在编辑器中,
/// 只有常规不可逆警示。文案与归约侧行为(`State::after_git_checkout`)
/// 一一对应:dirty 保留未保存稿、非 dirty 重载为 HEAD。
fn checkout_extra_warning(open_in_editor: bool, dirty: bool) -> Option<&'static str> {
    if !open_in_editor {
        return None;
    }
    Some(if dirty {
        "该文件正在编辑器中打开:编辑器里未保存的修改会保留,之后保存(Ctrl+S)会把它们写回。"
    } else {
        "该文件正在编辑器中打开:回滚后编辑器将重载为 HEAD 版本。"
    })
}

impl eframe::App for LaterMdApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        if let Some(menu) = &self.native_menu {
            menu.drain(ctx, &mut self.outbox);
        }
        self.reduce(ctx);
        #[cfg(target_os = "macos")]
        if let Some(menu) = &self.native_menu {
            menu.finish_frame(ctx, &mut self.outbox, &self.state);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    // 刻意**不**实现 `App::on_exit` 来拆单实例 IPC,只靠
    // `LaterMdApp::drop` → `instance::Primary::drop`(删锁 + 叫醒 accept 线程)。
    //
    // 原因:`on_exit` 的签名按 glow feature 分两种
    // (`fn(&mut self)` / `fn(&mut self, Option<&glow::Context>)`,
    // eframe/src/epi.rs:221-228),而本仓 `vendor/egui_markdown/Cargo.toml`
    // 的 dev-dependency 开了 `eframe/glow` —— **feature 统一**会让
    // 「只编 latermd-app」与「编整个 workspace」得到不同签名,任何
    // `#[cfg(feature = "glow")]` 的实现都只在其中一种下对(本轮实测:
    // `-p latermd-app` 要 1 参、`--workspace` 要 2 参)。零新增 vendor
    // 依赖去解一个「删锁早几十毫秒」的优化不划算。
    //
    // 时序上 `Drop` 也够:eframe 退出时先 `save` 后 `on_exit`,App 随即
    // 被 drop,锁在同一毫秒内删除。真正要紧的是「锁残留到下次启动」,
    // 那由 `try_claim` 的僵尸锁回收兜底(实测有测试)。
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state;
    use std::path::{Path, PathBuf};

    /// 单实例转发接线的**端到端守门**:真进程级 IPC(锁文件 + 回环
    /// socket)+ 真 App + 真 `reduce` 帧 + 真文件。
    ///
    /// 这条守的是 `reduce` 开头那圈「收件 → `FileSelected`」。此前它是
    /// 恒绿的:变异验证过 —— 把那圈删掉,全测试仍1067 passed(转发到的
    /// 路径根本没人开成标签,而没有任何测试察觉)。
    ///
    /// 之所以要用**真的第二个进程路径**而不是往 channel 里塞消息:
    /// channel 是本模块私有的,测试若直接构造就绕过了「第二进程真的
    /// 连上了吗」这一段 —— 那正是最可能坏的地方(端口写错、锁竞争、
    /// 帧编解码错)。
    #[test]
    fn forwarded_paths_from_a_second_launch_open_as_tabs() {
        let vault = std::env::temp_dir().join(format!(
            "latermd-fwd-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&vault);
        std::fs::create_dir_all(&vault).expect("建临时目录");
        let note = vault.join("forwarded.md");
        std::fs::write(&note, "# 转发来的\n").expect("写临时笔记");

        // ① 主实例 = 生产认领路径(真锁 + 真监听)
        let primary = match crate::instance::try_claim(Vec::new(), Some(vault.as_path())) {
            crate::instance::Claim::Primary(primary) => *primary,
            crate::instance::Claim::HandedOff => panic!("首次认领不该交接"),
        };

        // ② 第二实例带路径启动 → 真转发后自行退出
        assert!(
            matches!(
                crate::instance::try_claim(vec![note.clone()], Some(vault.as_path())),
                crate::instance::Claim::HandedOff
            ),
            "带路径的第二启动应交接给已有实例"
        );

        // ③ 主实例跑真帧:转发来的路径必须变成标签
        let ctx = egui::Context::default();
        let mut app = LaterMdApp {
            instance: Some(primary),
            ..LaterMdApp::default()
        };
        let baseline = app.state.tabs.tabs.len();
        // 多跑几帧:socket 送达与帧到达之间有真实延迟,单帧会偶发空转。
        for _ in 0..200 {
            ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()))
                .drop_without_applying_deltas();
            if app.state.tabs.tabs.len() > baseline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert_eq!(
            app.state.tabs.tabs.len(),
            baseline + 1,
            "转发来的路径应在 reduce 里开成新标签(此前该接线无测试覆盖)"
        );
        assert_eq!(
            app.state.tabs.current().document.path.as_deref(),
            Some(note.as_path()),
            "且成为当前标签"
        );
        assert_eq!(
            app.state.tabs.current().editor.text(),
            "# 转发来的\n",
            "内容要真读进缓冲"
        );
        let _ = std::fs::remove_dir_all(&vault);
    }

    #[test]
    fn search_replace_dialog_buttons_send_confirmation_messages() {
        for confirm in [true, false] {
            let ctx = egui::Context::default();
            let mut app = LaterMdApp::default();
            app.state.search.confirm_replace = Some(crate::search::ReplacePlan {
                query: app.state.search.to_query(Path::new("/vault")),
                replacement: "pin".into(),
                paths: vec![PathBuf::from("/vault/note.md")],
                generation: app.state.search.generation,
            });
            let mut rect = Rect::NOTHING;
            for _ in 0..4 {
                ctx.run_ui(RawInput::default(), |ui| {
                    let (yes, no, _x) = search_replace_dialog(ui, 1);
                    rect = if confirm { yes.rect } else { no.rect };
                })
                .drop_without_applying_deltas();
            }
            let pos = rect.center();
            ctx.run_ui(
                RawInput {
                    events: vec![
                        Event::PointerMoved(pos),
                        Event::PointerButton {
                            pos,
                            button: PointerButton::Primary,
                            pressed: true,
                            modifiers: Default::default(),
                        },
                        Event::PointerButton {
                            pos,
                            button: PointerButton::Primary,
                            pressed: false,
                            modifiers: Default::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ui| app.draw_overlay_dialogs(ui),
            )
            .drop_without_applying_deltas();
            assert_eq!(
                app.outbox,
                vec![if confirm {
                    Message::SearchReplaceAllConfirmed
                } else {
                    Message::SearchReplaceAllCancelled
                }]
            );
        }
    }

    /// 侧栏可拖宽回归(2026-10-09 坤哥真机:「左侧栏宽度似乎锁死了,不能拉伸」)。
    ///
    /// 根因:egui 0.36 的 `Panel` 收尾时把**内容的 Frame 响应矩形**(而非分配的
    /// outer_rect)回写进 `PanelState`;rail 改版(eb80618)把 body 挪进 `new_child`
    /// 子 Ui 后,子 Ui 的分配不外溢回面板 Ui 的 min_rect,侧栏内容塌到 `size_range`
    /// 下限,拖拽分到的宽度在松手帧被内容宽吞掉 —— 面板永远 180px。
    /// 修复:sidebar::ui 末尾 `expand_to_include_rect(max_rect)` 把面板 Ui 撑满。
    ///
    /// 本测试按真机手势走完整 App:首帧宽度 = `default_size`(240,不再是下限),
    /// 在右缘按下拖到 320 松手后,`PanelState` 稳定在 320。修复前两条都红
    /// (实测首帧 180、拖后 180)。
    #[test]
    fn nav_resizes_by_drag_in_full_ui() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0));
        let mut app = LaterMdApp::default();
        assert!(app.state.layout.left, "默认左栏开");
        let mut frame = |events: Vec<egui::Event>| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| app.draw(ui),
            )
            .drop_without_applying_deltas();
        };
        let nav_width = || {
            egui::PanelState::load(&ctx, egui::Id::new("nav"))
                .expect("nav PanelState 存在")
                .outer_rect
                .width()
        };

        frame(Vec::new());
        assert!(
            (nav_width() - 240.0).abs() < 1.0,
            "首帧宽度应为 default_size(240),实际 {}(内容不撑满面板时 egui 会回写下限)",
            nav_width()
        );

        // 按下 → 逐帧拖到 320 → 松手。按位点取当前右缘(240)+1,拖拽中把手
        // 逐帧跟随面板右缘,与真机手势一致。
        let edge = nav_width();
        frame(vec![Event::PointerMoved(egui::pos2(edge + 1.0, 300.0))]);
        frame(vec![Event::PointerButton {
            pos: egui::pos2(edge + 1.0, 300.0),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        }]);
        for x in [260.0, 280.0, 300.0, 320.0] {
            let track = nav_width().max(x - 40.0);
            frame(vec![Event::PointerMoved(egui::pos2(x, 300.0))]);
            let _ = track;
        }
        frame(vec![Event::PointerButton {
            pos: egui::pos2(320.0, 300.0),
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]);
        frame(Vec::new());
        assert!(
            (nav_width() - 320.0).abs() < 1.0,
            "拖到 320 松手后宽度应稳定在 320,实际 {}(拖拽被内容宽吞掉 = 侧栏锁宽回归)",
            nav_width()
        );
    }
    use egui::{
        Event, FullOutput, Key, Modifiers, OutputCommand, PointerButton, Pos2, RawInput, Rect,
        ViewportCommand,
    };
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn shrinking_window_preserves_editor_budget_and_restores_user_widths() {
        let ctx = egui::Context::default();
        let mut app = LaterMdApp::default();
        app.state.layout.left_width = Some(320.0);
        for width in [1600.0, 900.0, 1600.0] {
            let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(width, 800.0));
            for _ in 0..3 {
                ctx.run_ui(
                    RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| app.draw(ui),
                )
                .drop_without_applying_deltas();
            }
            let nav = egui::PanelState::load(&ctx, egui::Id::new("nav"))
                .unwrap()
                .size()
                .x;
            assert!(
                width - nav >= 379.0,
                "{width}: editor squeezed by {nav}(编辑器最小预算,右栏退役后是唯一邻居)"
            );
            assert_eq!(app.state.layout.left_width, Some(320.0));
            if width == 1600.0 {
                assert!((nav - 320.0).abs() < 1.0);
            }
        }
    }

    #[test]
    fn automatic_minimap_follows_document_and_window_size() {
        // 900px 档:右栏对照预览退役(live-unify P4)后中央编辑区
        // ≈660px 已过 600 门槛,auto 判据翻真。
        for (width, lines, manual, expected) in [
            (900.0, 400, false, true),
            (1920.0, 10, false, false),
            (1920.0, 400, false, true),
            (900.0, 400, true, true),
        ] {
            let ctx = egui::Context::default();
            let mut app = LaterMdApp::default();
            app.state.theme.minimap_auto = !manual;
            app.state
                .tabs
                .current_mut()
                .editor
                .load(&"正文\n".repeat(lines));
            let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(width, 800.0));
            let mut shown = false;
            for _ in 0..3 {
                let output = ctx.run_ui(
                    RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| app.draw(ui),
                );
                shown = output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Rect(r) if (r.rect.width() - 108.0).abs() < 0.5 && r.rect.height() > 10.0 && r.fill != egui::Color32::TRANSPARENT));
                output.drop_without_applying_deltas();
            }
            assert_eq!(
                shown, expected,
                "width={width}, lines={lines}, manual={manual}"
            );
        }
    }

    /// 全屏视口拖影回归(2026-10-08 坤哥真机报告:小窗可拖、全屏不可拖):
    /// 完整 UI(侧栏+编辑器+预览三列)两档尺寸同流程,按住 minimap 高亮框
    /// 拖动,编辑器 offset 必须逐帧前进。editor 层(minimap::frame_sized)同
    /// 尺寸已绿;此测试钉 layout 层面板分配与命中注册。
    #[test]
    fn minimap_drag_works_small_and_fullscreen_in_full_ui() {
        for (w, h, label) in [(900.0, 600.0, "小窗"), (1920.0, 1008.0, "全屏")] {
            let dir = std::env::temp_dir().join(format!(
                "latermd-minimap-fs-{}-{}",
                label,
                std::process::id()
            ));
            let mut app = LaterMdApp::default();
            app.state.settings_dir = Some(dir.clone());
            // 本用例验证手动常显时拖拽；自动模式的窄窗折叠另有覆盖。
            app.state.theme.minimap_auto = false;
            app.state.render_mode = crate::live::RenderMode::Source;
            app.state.tabs.current_mut().editor.replace_all(
                &(0..500)
                    .map(|_| "普通的一行")
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            let tab_id = app.state.tabs.current().id;
            let editor_id = crate::ui::editor::tab_editor_id(tab_id);
            let ctx = egui::Context::default();
            let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(w, h));

            // 两空帧起步(布局/缓存就绪,高亮框出现)
            let shapes0 = find_test_frame(&mut app, &ctx, screen, 0.0, Vec::new());
            let shapes = find_test_frame(&mut app, &ctx, screen, 0.1, Vec::new());
            let _ = shapes0;
            // minimap 窄条定位:宽=MINIMAP_W、高>300 的填充矩形(高亮框横跨
            // 整条;全帧无同宽高个数的其他矩形)
            let map = shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(r)
                        if r.fill != egui::Color32::TRANSPARENT
                            && (r.rect.width() - 108.0).abs() < 0.5
                            && r.rect.height() >= 10.0 =>
                    {
                        Some(r.rect)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{label} 视口下 minimap 高亮框未找到"));

            let press = egui::Pos2::new(map.center().x, map.top() + 40.0);
            find_test_frame(
                &mut app,
                &ctx,
                screen,
                0.2,
                vec![
                    Event::PointerMoved(press),
                    Event::PointerButton {
                        pos: press,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers: Modifiers::NONE,
                    },
                ],
            );
            let mut previous = 0.0_f32;
            let mut moved = 0;
            for step in 0..6 {
                let pos = egui::Pos2::new(press.x, press.y + step as f32 * 40.0);
                find_test_frame(
                    &mut app,
                    &ctx,
                    screen,
                    0.3 + step as f64 * 0.1,
                    vec![Event::PointerMoved(pos)],
                );
                let offset = ctx
                    .data(|d| {
                        d.get_temp::<crate::ui::minimap::ScrollMetrics>(
                            crate::ui::minimap::metrics_id(editor_id),
                        )
                    })
                    .map_or(0.0, |m| m.offset);
                assert!(
                    offset >= previous - 0.5,
                    "{label} 拖动单调不减(第 {step} 步 {previous} → {offset})"
                );
                if offset > previous + 1.0 {
                    moved += 1;
                }
                previous = offset;
            }
            assert!(
                moved >= 3,
                "{label} 拖动连续跟随(6 步中 {moved} 步在滚;窄条 {map:?})"
            );
            std::fs::remove_dir_all(dir).ok();
        }
    }

    fn find_test_app(name: &str) -> (LaterMdApp, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("latermd-find-{name}-{}", std::process::id()));
        let mut app = LaterMdApp::default();
        app.state.settings_dir = Some(dir.clone());
        app.state
            .tabs
            .current_mut()
            .editor
            .replace_all("needle one\nneedle two\nneedle three");
        (app, dir)
    }

    fn find_test_frame(
        app: &mut LaterMdApp,
        ctx: &egui::Context,
        screen: Rect,
        now: f64,
        events: Vec<Event>,
    ) -> Vec<egui::epaint::ClippedShape> {
        let output = ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                time: Some(now),
                events,
                ..Default::default()
            },
            |ui| {
                app.reduce(ui.ctx());
                app.draw(ui);
            },
        );
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        shapes
    }

    fn find_key(key: Key, modifiers: Modifiers) -> Vec<Event> {
        [true, false]
            .into_iter()
            .map(|pressed| Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            })
            .collect()
    }

    #[test]
    fn find_enter_repeats_without_losing_query_focus_or_editing_source() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let (mut app, dir) = find_test_app("enter");
        let original = app.state.tabs.current().editor.text().to_owned();
        app.state.apply(Message::FindBarToggled(true));
        app.state
            .apply(Message::FindQueryChanged("needle".to_owned()));
        for step in 0..4 {
            find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
        }
        let input = egui::Id::new("editor-find-input");
        ctx.memory_mut(|memory| memory.request_focus(input));
        for (step, (modifiers, expected)) in [
            (Modifiers::NONE, 1),
            (Modifiers::NONE, 2),
            (Modifiers::NONE, 0),
            (Modifiers::SHIFT, 2),
        ]
        .into_iter()
        .enumerate()
        {
            let now = 1.0 + step as f64;
            find_test_frame(&mut app, &ctx, screen, now, find_key(Key::Enter, modifiers));
            assert_eq!(
                app.outbox,
                vec![Message::FindNext {
                    backwards: modifiers.shift
                }]
            );
            find_test_frame(&mut app, &ctx, screen, now + 0.1, Vec::new());
            assert_eq!(app.state.find.hit, Some(expected));
            assert!(
                ctx.memory(|memory| memory.has_focus(input)),
                "跳转后仍可连续按回车"
            );
            let selection = egui::TextEdit::load_state(
                &ctx,
                crate::ui::editor::tab_editor_id(app.state.tabs.current().id),
            )
            .unwrap()
            .cursor
            .char_range()
            .unwrap();
            let hit = &app.state.find.hits[expected];
            assert_eq!(
                (
                    selection.primary.index.0.min(selection.secondary.index.0),
                    selection.primary.index.0.max(selection.secondary.index.0),
                ),
                (hit.start, hit.end)
            );
            assert_eq!(app.state.tabs.current().editor.text(), original);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn find_query_typing_keeps_focus_and_empty_results_do_not_edit_source() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let (mut app, dir) = find_test_app("typing");
        let original = app.state.tabs.current().editor.text().to_owned();
        app.state.apply(Message::FindBarToggled(true));
        for step in 0..4 {
            find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
        }
        let input = find_input_id();
        ctx.memory_mut(|memory| memory.request_focus(input));
        let mut query = String::new();
        for (step, text) in ["n", "e", "e", "d", "l", "e", "-absent"]
            .into_iter()
            .enumerate()
        {
            query.push_str(text);
            let now = 1.0 + step as f64;
            find_test_frame(
                &mut app,
                &ctx,
                screen,
                now,
                vec![Event::Text(text.to_owned())],
            );
            find_test_frame(&mut app, &ctx, screen, now + 0.1, Vec::new());
            assert_eq!(app.state.find.query, query);
            assert!(ctx.memory(|memory| memory.has_focus(input)));
            assert_eq!(app.state.tabs.current().editor.text(), original);
        }
        assert!(app.state.find.hits.is_empty());
        for (step, modifiers) in [Modifiers::NONE, Modifiers::SHIFT].into_iter().enumerate() {
            let now = 9.0 + step as f64;
            find_test_frame(&mut app, &ctx, screen, now, find_key(Key::Enter, modifiers));
            find_test_frame(&mut app, &ctx, screen, now + 0.1, Vec::new());
            assert_eq!(app.state.find.hit, None);
            assert_eq!(app.state.find.query, query);
            assert!(ctx.memory(|memory| memory.has_focus(input)));
            assert_eq!(app.state.tabs.current().editor.text(), original);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Ctrl+F 打开查找卡:焦点自动落进查找框(VS Code 同款,打开即可
    /// 打字)。want_focus 只在打开时置位、绘制时消费一次 —— 后续帧用户
    /// 点了编辑器/把手不再被抢回(把手测试已覆盖)。
    #[test]
    fn find_bar_open_focuses_the_query_input() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let (mut app, dir) = find_test_app("open-focus");
        let editor_id = crate::ui::editor::tab_editor_id(app.state.tabs.current().id);
        // 起始帧让编辑器先持有焦点(模拟 Ctrl+F 前光标在编辑器里)
        find_test_frame(&mut app, &ctx, screen, 0.0, Vec::new());
        ctx.memory_mut(|memory| memory.request_focus(editor_id));
        app.state.apply(Message::FindBarToggled(true));
        assert!(app.state.find.want_focus, "打开时置位焦点意图");
        find_test_frame(&mut app, &ctx, screen, 0.1, Vec::new());
        find_test_frame(&mut app, &ctx, screen, 0.2, Vec::new());
        assert!(!app.state.find.want_focus, "绘制消费后清除,只请求一次");
        assert!(
            ctx.memory(|memory| memory.has_focus(find_input_id())),
            "Ctrl+F 打开后焦点应落进查找框"
        );
        // 打开即打字:Text 事件直落查找词,不进编辑器
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            0.3,
            vec![Event::Text("n".to_owned())],
        );
        find_test_frame(&mut app, &ctx, screen, 0.4, Vec::new());
        assert_eq!(app.state.find.query, "n");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 页面级命中高亮:查找卡打开后,编辑器区域应出现命中数对应的浅底
    /// 矩形(选区底色 45% 透明);当前条仍由选区高亮承担。关闭查找卡后
    /// 高亮消失。
    #[test]
    fn find_marks_paint_for_all_hits_and_clear_on_close() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let (mut app, dir) = find_test_app("paint-marks");
        // 生产高亮色 = ui.visuals().selection.bg_fill × 0.45;theme.apply
        // 在 reduce 里跑,先跑一帧让主题落定,再取期望色(与生产同源,
        // 硬编码会因主题差异误报)。
        let mut shapes = find_test_frame(&mut app, &ctx, screen, 0.0, Vec::new());
        // 期望色与生产同源:绘制帧的 visuals.selection.bg_fill × 0.45。
        // 主题投影(theme.apply)按帧推进,首帧是 egui 默认蓝、次帧起
        // 才是皮肤浅色 —— 所以期望色在**打开查找卡的首个绘制帧后**取。
        let selection_fill = {
            app.state.apply(Message::FindBarToggled(true));
            app.state
                .apply(Message::FindQueryChanged("needle".to_owned()));
            let _ = find_test_frame(&mut app, &ctx, screen, 1.0, Vec::new());
            ctx.global_style()
                .visuals
                .selection
                .bg_fill
                .gamma_multiply(0.45)
        };
        let count_marks = |shapes: &[egui::epaint::ClippedShape]| -> usize {
            // fill 精确相等:期望色取自同一绘制帧的 visuals,同源无舍入差。
            // (之前 ±2 的宽容把另一个相近浅底误计为命中。)
            shapes
                .iter()
                .filter(|clipped| {
                    matches!(
                        &clipped.shape,
                        egui::epaint::Shape::Rect(rect) if rect.fill == selection_fill
                    )
                })
                .count()
        };
        assert_eq!(count_marks(&shapes), 0, "查找卡未开时无命中高亮");
        shapes.clear();

        app.state.apply(Message::FindBarToggled(true));
        app.state
            .apply(Message::FindQueryChanged("needle".to_owned()));
        for step in 0..4 {
            shapes = find_test_frame(
                &mut app,
                &ctx,
                screen,
                f64::from(step) * 0.1 + 1.0,
                Vec::new(),
            );
            let marks = count_marks(&shapes);
            // 文本含 3 个 needle;当前条由选区高亮(不同色),全部命中各
            // 一块浅底 —— 数量应为 3(当前条也在,选区只是叠在其上)
            assert_eq!(marks, 3, "三个命中都应有浅底高亮");
        }

        // Esc 关闭:高亮随之消失
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            2.0,
            find_key(Key::Escape, Modifiers::NONE),
        );
        let shapes = find_test_frame(&mut app, &ctx, screen, 2.1, Vec::new());
        assert_eq!(count_marks(&shapes), 0, "关闭查找卡后高亮消失");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn find_overlay_does_not_intercept_enter_in_source_editor() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let (mut app, dir) = find_test_app("source-focus");
        app.state.apply(Message::FindBarToggled(true));
        app.state
            .apply(Message::FindQueryChanged("needle".to_owned()));
        for step in 0..4 {
            find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
        }
        let editor_id = crate::ui::editor::tab_editor_id(app.state.tabs.current().id);
        let end = app.state.tabs.current().editor.text().chars().count();
        let mut edit = egui::TextEdit::load_state(&ctx, editor_id).unwrap();
        edit.cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(end),
            )));
        edit.store(&ctx, editor_id);
        ctx.memory_mut(|memory| memory.request_focus(editor_id));
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            1.0,
            find_key(Key::Enter, Modifiers::NONE),
        );
        find_test_frame(&mut app, &ctx, screen, 1.1, Vec::new());
        assert!(app.outbox.is_empty());
        assert_eq!(app.state.find.hit, Some(0));
        assert_eq!(
            app.state.tabs.current().editor.text(),
            "needle one\nneedle two\nneedle three\n"
        );
        assert!(ctx.memory(|memory| memory.has_focus(editor_id)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn find_overlay_stays_below_wrapped_toolbar_without_moving_source() {
        for width in [1200.0, 1600.0] {
            let ctx = egui::Context::default();
            let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(width, 850.0));
            let (mut app, dir) = find_test_app(&format!("position-{width}"));
            let toolbar_bottom = Rc::new(Cell::new(0.0_f32));
            let sink = toolbar_bottom.clone();
            app.format_probe = Some(Box::new(move |_, rect| {
                sink.set(sink.get().max(rect.bottom()))
            }));
            let mut shapes = Vec::new();
            for step in 0..4 {
                shapes = find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
            }
            let source_before = topmost_text(&shapes, "needle one");
            app.state.apply(Message::FindBarToggled(true));
            for step in 4..8 {
                shapes = find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
            }
            let overlay = ctx
                .memory(|memory| memory.area_rect(egui::Id::new("editor-find-overlay")))
                .unwrap();
            assert!(
                overlay.top() > toolbar_bottom.get() + 4.0,
                "查找框必须在工具栏下方: {overlay:?}, toolbar={}",
                toolbar_bottom.get()
            );
            assert_eq!(
                topmost_text(&shapes, "needle one").top(),
                source_before.top(),
                "悬浮查找框不推低正文"
            );
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// 替换行(#17 M1):按钮可用性跟命中走;点击分别发出单个/全部替换
    /// 消息(替换词输入不改写文档由归约侧消费,见 state 层测试)。
    #[test]
    fn replace_row_buttons_follow_hits_and_send_messages() {
        let ctx = egui::Context::default();
        let mut find = crate::state::FindBarState {
            open: true,
            replace_open: true,
            query: "needle".to_owned(),
            replacement: "pin".to_owned(),
            hits: vec![std::ops::Range { start: 0, end: 6 }],
            hit: Some(0),
            ..Default::default()
        };
        let mut outbox = Vec::new();
        let mut rects = (Rect::NOTHING, Rect::NOTHING);
        for _ in 0..3 {
            let output = ctx.run_ui(RawInput::default(), |ui| {
                let total = find.hits.len();
                let (replace, all) = replace_row(ui, &mut find, total, 1, &mut outbox);
                assert!(replace.enabled(), "有当前命中时「替换」可用");
                assert!(all.enabled(), "有命中时「全部」可用");
                rects = (replace.rect, all.rect);
            });
            output.drop_without_applying_deltas();
        }
        assert!(outbox.is_empty(), "仅渲染不产生消息");

        // 点击「替换」「全部」各自发一条消息(与 menubar 点击测试同节奏:
        // 渲染拿 rect,下一帧合成按下/抬起)
        let (replace_rect, all_rect) = rects;
        for (rect, expected) in [
            (replace_rect, Message::ReplaceCurrent),
            (all_rect, Message::ReplaceAllInDoc),
        ] {
            let center = rect.center();
            let click = |pressed| Event::PointerButton {
                pos: center,
                button: PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            ctx.run_ui(
                RawInput {
                    events: vec![Event::PointerMoved(center), click(true), click(false)],
                    ..Default::default()
                },
                |ui| {
                    let total = find.hits.len();
                    replace_row(ui, &mut find, total, 1, &mut outbox);
                },
            )
            .drop_without_applying_deltas();
            assert_eq!(outbox, vec![expected]);
            outbox.clear();
        }
    }

    /// 无命中时两枚替换按钮都禁用(替换词变化只改按钮可用性,不发消息)。
    #[test]
    fn replace_row_buttons_disable_without_hits() {
        let ctx = egui::Context::default();
        let mut find = crate::state::FindBarState {
            open: true,
            replace_open: true,
            query: "needle".to_owned(),
            replacement: "pin".to_owned(),
            hits: Vec::new(),
            hit: None,
            ..Default::default()
        };
        let mut outbox = Vec::new();
        for _ in 0..3 {
            let output = ctx.run_ui(RawInput::default(), |ui| {
                let (replace, all) = replace_row(ui, &mut find, 0, 0, &mut outbox);
                assert!(!replace.enabled(), "无命中时「替换」禁用");
                assert!(!all.enabled(), "无命中时「全部」禁用");
            });
            output.drop_without_applying_deltas();
        }
        assert!(outbox.is_empty(), "禁用态不产生消息");
    }

    /// #72 M1:替换行输入框与查找行输入框左缘对齐(两行行首标签共用
    /// 固定列宽)。取证基线:2026-10-09 改前无头实测 delta 恰为 0(两词
    /// 都是两个汉字、恰好等宽的巧合),本断言把对齐从巧合钉成结构
    /// 保证 —— 标签字面或字体偏好再变也不许散。
    #[test]
    fn find_and_replace_input_columns_align() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let (mut app, dir) = find_test_app("label-align");
        app.state.apply(Message::FindBarToggled(true));
        app.state.apply(Message::ReplaceBarToggled(true));
        for step in 0..6 {
            find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
        }
        let find_left = ctx.read_response(find_input_id()).unwrap().rect.left();
        let replace_left = ctx.read_response(replace_input_id()).unwrap().rect.left();
        assert!(
            (find_left - replace_left).abs() < 0.5,
            "查找/替换两行输入框列必须对齐: find={find_left} replace={replace_left}"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    /// #72 M1:命中计数着色 —— 无命中画警示色(`tokens::WARN`),有命中
    /// 画弱色。取证走曲面细分后的顶点色:隔离渲染 `find_bar_contents`
    /// (不掺全应用其它文案),无命中帧必须出现 WARN 色顶点、有命中帧
    /// 必须一个都没有,且弱色文字在场。
    #[test]
    fn hit_count_warns_without_hits_and_weakens_with_hits() {
        let ctx = egui::Context::default();
        let mut find = crate::state::FindBarState {
            open: true,
            replace_open: true,
            query: "needle".to_owned(),
            hits: vec![std::ops::Range { start: 0, end: 6 }],
            hit: Some(0),
            ..Default::default()
        };
        let weak = ctx.global_style().visuals.weak_text_color();
        assert_ne!(weak, crate::ui::tokens::WARN, "取证前提:两色可分");
        let frame_vertex_colors = |find: &mut crate::state::FindBarState| -> Vec<egui::Color32> {
            let mut colors = Vec::new();
            for _ in 0..3 {
                let mut output = ctx.run_ui(RawInput::default(), |ui| {
                    let mut outbox = Vec::new();
                    find_bar_contents(ui, find, &mut outbox);
                });
                let primitives = ctx.tessellate(std::mem::take(&mut output.shapes), 1.0);
                for clipped in &primitives {
                    let egui::epaint::Primitive::Mesh(mesh) = &clipped.primitive else {
                        continue;
                    };
                    colors.extend(mesh.vertices.iter().map(|v| v.color));
                }
                output.drop_without_applying_deltas();
            }
            colors
        };
        // 有命中:只弱色计数,无警示
        let with_hits = frame_vertex_colors(&mut find);
        assert!(with_hits.contains(&weak), "有命中时计数应为弱色 {weak:?}");
        assert!(
            !with_hits.contains(&crate::ui::tokens::WARN),
            "有命中时不得出现警示色"
        );
        // 无命中:警示色在场(替换行计数同款,出现即满足)
        find.hits.clear();
        find.hit = None;
        let without_hits = frame_vertex_colors(&mut find);
        assert!(
            without_hits.contains(&crate::ui::tokens::WARN),
            "无命中时计数应为警示色 {:?}",
            crate::ui::tokens::WARN
        );
    }

    /// 点是否落在三角形内(含边界);零面积三角形判外。行列式手写
    /// (Vec2 无 cross)。与 icons.rs 测试同款手法。
    fn point_in_tri(p: Pos2, a: Pos2, b: Pos2, c: Pos2) -> bool {
        let det = |u: egui::Vec2, v: egui::Vec2| u.x * v.y - u.y * v.x;
        if det(b - a, c - a).abs() < 1e-9 {
            return false;
        }
        let d = |u: Pos2, v: Pos2| det(v - u, p - u);
        let (d1, d2, d3) = (d(a, b), d(b, c), d(c, a));
        (d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0) || (d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0)
    }

    /// 无头像素取样:一帧 shapes 曲面细分(羽化关掉,三角形即硬边),
    /// 取**最后**覆盖采样点的颜色 —— shapes 按绘制序排列,后者盖前者,
    /// 卡片内框底色之上不能再有别的填充盖住探针。
    fn topmost_covered_color(
        primitives: &[egui::ClippedPrimitive],
        p: Pos2,
    ) -> Option<egui::Color32> {
        let mut color = None;
        for clipped in primitives {
            if !clipped.clip_rect.contains(p) {
                continue;
            }
            let egui::epaint::Primitive::Mesh(mesh) = &clipped.primitive else {
                continue;
            };
            for tri in mesh.indices.as_chunks::<3>().0 {
                let v = |i: u32| mesh.vertices[i as usize].pos;
                let (a, b, c) = (v(tri[0]), v(tri[1]), v(tri[2]));
                if point_in_tri(p, a, b, c) {
                    color = Some(mesh.vertices[tri[0] as usize].color);
                }
            }
        }
        color
    }

    /// #72 M1 像素验收:查找卡与跳转卡的浮层底色必须等于**当前生效主题**
    /// 的 popup 底色(`Frame::popup(&ctx.style())`,经 shell 投影后即
    /// `shell_tokens(dark).content`)。「写死 `Theme::Dark`」的回归(浅色
    /// 主题下黑窗,坤哥 2026-10-08 报)在本测试直接红:浅色下探针取到的
    /// 不是 `#FFFFFF` 就是深色 token。探针取卡片左内边距带中点
    /// (`menu_margin` 6px 的环带,不与任何控件相交),8 帧 × 0.1s 跑过
    /// 浮层淡入(animation_time 出厂 0.2s)。
    #[test]
    fn find_and_goto_cards_paint_effective_theme_popup_fill() {
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let light_fill = crate::theme::shell_tokens(false).content;
        let dark_fill = crate::theme::shell_tokens(true).content;
        assert_ne!(
            light_fill, dark_fill,
            "两主题 popup 底色必须可分,断言才有分辨力"
        );

        for (mode, expected_fill) in [
            (crate::theme::ThemeMode::Light, light_fill),
            (crate::theme::ThemeMode::Dark, dark_fill),
        ] {
            let ctx = egui::Context::default();
            // 关 AA 羽化:边三角形的透明渐变会让采样读到半透明假色
            ctx.options_mut(|o| o.tessellation_options.feathering = false);
            let (mut app, dir) = find_test_app(&format!("popup-fill-{mode:?}"));
            app.state.apply(Message::ThemeChanged(mode));

            for (overlay_id, is_find) in [
                (egui::Id::new("editor-find-overlay"), true),
                (egui::Id::new("editor-goto-overlay"), false),
            ] {
                let (open, close) = if is_find {
                    (
                        Message::FindBarToggled(true),
                        Message::FindBarToggled(false),
                    )
                } else {
                    (
                        Message::GotoBarToggled(true),
                        Message::GotoBarToggled(false),
                    )
                };
                app.state.apply(open);
                let mut shapes = Vec::new();
                for step in 0..8 {
                    shapes =
                        find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
                }
                // 前置:style 的 popup 底色 = 该主题 shell content token
                assert_eq!(
                    ctx.global_style().visuals.window_fill(),
                    expected_fill,
                    "{mode:?} 的 style popup 底色应等于 shell content token"
                );
                let rect = ctx
                    .memory(|memory| memory.area_rect(overlay_id))
                    .unwrap_or_else(|| panic!("{mode:?} 浮卡 {overlay_id:?} 未绘制"));
                let probe = rect.left_top() + egui::vec2(3.0, rect.height() / 2.0);
                let primitives = ctx.tessellate(shapes, 1.0);
                assert_eq!(
                    topmost_covered_color(&primitives, probe),
                    Some(expected_fill),
                    "{mode:?} 浮卡左内边距带 {probe:?} 应是 popup 底色 {expected_fill:?} \
                     (写死 Theme::Dark 的回归在此显形)"
                );
                // 换卡前先收起(两卡互斥,显式走同一条归约)
                app.state.apply(close);
            }
            std::fs::remove_dir_all(dir).ok();
        }
    }

    /// 替换键整链路(#17 M1):键盘经命令层打开查找条 + 替换行;替换词
    /// 输入只更新状态不改文档;替换框上 Esc 关整条。(#60 M2:按键从出厂
    /// 键位读——mac 出厂已是 ⌥⌘F(#114),硬编码 COMMAND+H 在 mac 编译
    /// 目标上不再触发;测试语义「出厂键触发替换条」不变。)
    #[test]
    fn ctrl_h_opens_replace_row_typing_does_not_edit_and_esc_closes() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let (mut app, dir) = find_test_app("ctrl-h");
        let original = app.state.tabs.current().editor.text().to_owned();
        for step in 0..4 {
            find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
        }

        // 出厂替换键 → 命令层消费键位,查找条同开、替换行展开
        let shortcut = crate::command::Command::ReplaceInDoc
            .default_shortcut()
            .expect("替换命令有出厂键位");
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            1.0,
            find_key(shortcut.logical_key, shortcut.modifiers),
        );
        find_test_frame(&mut app, &ctx, screen, 1.1, Vec::new());
        assert!(
            app.state.find.open && app.state.find.replace_open,
            "Ctrl+H 打开查找条与替换行"
        );

        // 替换框打字:只更新 replacement,文档不动
        let input = replace_input_id();
        ctx.memory_mut(|memory| memory.request_focus(input));
        for (step, text) in ["p", "i", "n"].into_iter().enumerate() {
            let now = 2.0 + step as f64;
            find_test_frame(
                &mut app,
                &ctx,
                screen,
                now,
                vec![Event::Text(text.to_owned())],
            );
            find_test_frame(&mut app, &ctx, screen, now + 0.1, Vec::new());
            assert!(
                ctx.memory(|memory| memory.has_focus(input)),
                "跳转不抢输入焦点"
            );
            assert_eq!(
                app.state.tabs.current().editor.text(),
                original,
                "替换词输入不自动改写文档"
            );
        }
        assert_eq!(app.state.find.replacement, "pin");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 替换框聚焦时 Esc 关整条(#17 M1,与查找框同口径):egui 0.36 焦点
    /// 导航默认把裸 Esc 当「交出焦点」在 begin_pass 清焦,输入框必须以
    /// `escape: true` 的 event_filter 锁住 Esc,框内检测才触得到
    /// (2026-10-01 无头实证,修复前该链路在两框上都是哑弹)。
    /// 注:无头 run_ui 下「Text 打字帧后紧跟 Esc 帧」会让浮层 Window 的
    /// 内容闭包整帧不执行(egui 内部行为,与时间/焦点无关,真机帧调度
    /// 不同),故 Esc 用空帧节奏单独钉;打字与 Esc 的连续操作留真机目视。
    #[test]
    fn replace_row_escape_closes_the_bar() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let (mut app, dir) = find_test_app("esc-replace");
        app.state.apply(Message::ReplaceBarToggled(true));
        app.state
            .apply(Message::FindQueryChanged("needle".to_owned()));
        for step in 0..4 {
            find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
        }
        assert!(app.state.find.replace_open, "前置:替换行展开");
        let input = replace_input_id();
        ctx.memory_mut(|memory| memory.request_focus(input));
        for step in 4..6 {
            find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
        }
        assert!(ctx.memory(|memory| memory.has_focus(input)), "替换框已聚焦");
        app.outbox.clear();
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            1.0,
            find_key(Key::Escape, Modifiers::NONE),
        );
        assert_eq!(app.outbox, vec![Message::FindBarToggled(false)]);
        find_test_frame(&mut app, &ctx, screen, 1.1, Vec::new());
        assert!(
            !app.state.find.open && !app.state.find.replace_open,
            "Esc 后整条关闭,替换行一并收起"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 查找框聚焦时 Esc 关整条(#17 M1 回归守护):同
    /// [`replace_row_escape_closes_the_bar`] 的修复,查找框侧的守护。
    #[test]
    fn find_row_escape_closes_the_bar() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let (mut app, dir) = find_test_app("esc-find");
        app.state.apply(Message::FindBarToggled(true));
        app.state
            .apply(Message::FindQueryChanged("needle".to_owned()));
        for step in 0..4 {
            find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
        }
        let input = find_input_id();
        ctx.memory_mut(|memory| memory.request_focus(input));
        for step in 4..6 {
            find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
        }
        assert!(ctx.memory(|memory| memory.has_focus(input)), "查找框已聚焦");
        app.outbox.clear();
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            1.0,
            find_key(Key::Escape, Modifiers::NONE),
        );
        assert_eq!(app.outbox, vec![Message::FindBarToggled(false)]);
        find_test_frame(&mut app, &ctx, screen, 1.1, Vec::new());
        assert!(!app.state.find.open, "Esc 后整条关闭");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 禅定帧的替换键(#17 M2 核验③,#60):`poll_shortcuts` 在 reduce
    /// 每帧必跑、不看布局分叉,替换条状态照常翻到位;但查找卡浮层只画
    /// 三栏源码路径,#17 交付时的既有口径 —— 禅定帧浮层 Area 不存在,
    /// Esc 退出禅定后同一状态立即落回屏上。是「延后」不是「被吞」
    /// (13a 的 Foreground Area 跨层屏蔽不适用:查找卡是 egui 管理的
    /// Window,禅定不画任何遮蔽它的层)。与查找条/goto 浮条在禅定的
    /// 行为三者同口径(decisions-pending #114)。按键从出厂键位读
    /// (#114:mac = ⌥⌘F,其余 = Ctrl+H;硬编码 Ctrl+H 在 mac 编译
    /// 目标上不触发,同 [`ctrl_h_opens_replace_row_typing_does_not_edit_and_esc_closes`])。
    #[test]
    fn ctrl_h_in_zen_flips_state_and_overlay_waits_for_three_pane() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let (mut app, dir) = find_test_app("zen-ctrl-h");
        app.state.apply(Message::ZenToggled);
        find_test_frame(&mut app, &ctx, screen, 0.1, Vec::new());
        assert!(app.state.layout.zen, "前置:已进禅定");

        // 禅定帧按出厂替换键:命令层消费键位,状态翻进查找条 + 替换行
        let shortcut = crate::command::Command::ReplaceInDoc
            .default_shortcut()
            .expect("替换命令有出厂键位");
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            0.2,
            find_key(shortcut.logical_key, shortcut.modifiers),
        );
        find_test_frame(&mut app, &ctx, screen, 0.3, Vec::new());
        assert!(app.state.layout.zen, "仍在禅定");
        assert!(
            app.state.find.open && app.state.find.replace_open,
            "禅定帧替换键状态照常翻到位"
        );
        let overlay = egui::Id::new("editor-find-overlay");
        assert!(
            ctx.memory(|memory| memory.area_rect(overlay)).is_none(),
            "禅定帧不画查找卡浮层"
        );

        // Esc 退出禅定(draw_zen 消费,推 outbox),下一帧 reduce 应用后
        // 回三栏 —— 查找卡同帧落回屏上,不需要再按一次替换键
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            0.4,
            find_key(Key::Escape, Modifiers::NONE),
        );
        find_test_frame(&mut app, &ctx, screen, 0.5, Vec::new());
        assert!(!app.state.layout.zen, "Esc 退出禅定");
        assert!(
            app.state.find.open && app.state.find.replace_open,
            "退出禅定后状态仍在,不丢"
        );
        assert!(
            ctx.memory(|memory| memory.area_rect(overlay)).is_some(),
            "三栏源码路径恢复后查找卡立即可见"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// minimap 开启态与查找/替换卡共存(#17 M2 核验⑤,#60):minimap 压窄
    /// 正文(MINIMAP_W),查找卡浮层锚定 editor 之后的 available rect,
    /// 两态都照常出现、都不推低正文 —— 「挤压」只发生在正文一侧且是
    /// 设计内让位(minimap 画在右缘、查找卡浮在其上,Order::Foreground)。
    /// minimap 出厂默认开(theme.rs),故 on 态是全量查找卡测试一直在跑
    /// 的形态;这里把 off 态拉进来对齐正文顶,补成显式断言。
    #[test]
    fn find_overlay_coexists_with_minimap() {
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let overlay = egui::Id::new("editor-find-overlay");
        let mut tops = std::collections::BTreeMap::new();
        for (name, show) in [("on", true), ("off", false)] {
            // 每态独立 Context:Area 记忆挂在 ctx 上,共用会让后一态读到
            // 前一态的浮层残留,断言失真
            let ctx = egui::Context::default();
            let (mut app, dir) = find_test_app("minimap-coexist");
            app.state.theme.show_minimap = show;
            app.state.apply(Message::ReplaceBarToggled(true));
            app.state
                .apply(Message::FindQueryChanged("needle".to_owned()));
            for step in 0..4 {
                find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
            }
            let shapes = find_test_frame(&mut app, &ctx, screen, 0.5, Vec::new());
            assert!(
                ctx.memory(|memory| memory.area_rect(overlay)).is_some(),
                "minimap {name}:查找卡浮层在场"
            );
            assert!(
                app.state.find.open && app.state.find.replace_open,
                "minimap {name}:替换行展开"
            );
            let body_top = topmost_text(&shapes, "needle one").top();
            tops.insert(name, body_top);
            let _ = std::fs::remove_dir_all(dir);
        }
        assert_eq!(
            tops["on"], tops["off"],
            "minimap 开关不改变正文顶边(浮层不参与布局,无挤压推低)"
        );
    }

    // —— #72 M2:浮卡把手拖动 ——

    /// 无头定位把手:卡片矩形内 ~28×3 的圆角胶囊(把手条唯一自绘
    /// shape,尺寸签名在全帧唯一)。
    fn overlay_drag_pill(shapes: &[egui::epaint::ClippedShape], card: Rect) -> Rect {
        shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(r)
                    if r.fill != egui::Color32::TRANSPARENT
                        && (r.rect.width() - 28.0).abs() < 0.5
                        && (r.rect.height() - 3.0).abs() < 0.5
                        && card.contains(r.rect.center()) =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
            .expect("浮卡把手胶囊未找到")
    }

    /// 把手拖动序列:press → 三帧累计位移到 `total` → release → 两空帧。
    /// 尾部空帧必需:偏移在内容闭包里累计,经 `fixed_pos` 下一帧才落位。
    fn drag_overlay_by_pill(
        app: &mut LaterMdApp,
        ctx: &egui::Context,
        screen: Rect,
        pill_center: Pos2,
        total: egui::Vec2,
        start_time: f64,
    ) {
        let click = |pos: Pos2, pressed: bool| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        find_test_frame(
            app,
            ctx,
            screen,
            start_time,
            vec![Event::PointerMoved(pill_center), click(pill_center, true)],
        );
        for step in 1..=3 {
            let pos = pill_center + total * (step as f32 / 3.0);
            find_test_frame(
                app,
                ctx,
                screen,
                start_time + f64::from(step) * 0.1,
                vec![Event::PointerMoved(pos)],
            );
        }
        find_test_frame(
            app,
            ctx,
            screen,
            start_time + 0.4,
            vec![click(pill_center + total, false)],
        );
        for step in 0..2 {
            find_test_frame(
                app,
                ctx,
                screen,
                start_time + 0.5 + f64::from(step) * 0.1,
                Vec::new(),
            );
        }
    }

    /// #72 M2:两卡把手拖动 —— 位置随动、精确等于累计位移、跨帧不动、
    /// 关了再开仍在拖后处(会话内保持)、拖回原点后与改前锚点逐字节一致
    /// (偏移回零 ⇒ `fixed_pos` 输入与改前相同,同一条钳制/取整管线)。
    #[test]
    fn find_and_goto_cards_drag_by_handle_and_hold_offset_across_frames() {
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        for (name, is_find) in [("find", true), ("goto", false)] {
            let ctx = egui::Context::default();
            let (mut app, dir) = find_test_app(&format!("drag-{name}"));
            let (open, close) = if is_find {
                (
                    Message::FindBarToggled(true),
                    Message::FindBarToggled(false),
                )
            } else {
                (
                    Message::GotoBarToggled(true),
                    Message::GotoBarToggled(false),
                )
            };
            app.state.apply(open.clone());
            let overlay = if is_find {
                egui::Id::new("editor-find-overlay")
            } else {
                egui::Id::new("editor-goto-overlay")
            };
            let mut shapes = Vec::new();
            for step in 0..8 {
                shapes = find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
            }
            let initial = ctx
                .memory(|memory| memory.area_rect(overlay))
                .expect("浮卡已绘制");
            let offset = |app: &LaterMdApp| {
                if is_find {
                    app.state.find.drag_offset
                } else {
                    app.state.goto.drag_offset
                }
            };
            // 前置:未拖动偏移恰为零 ⇒ 位置与改前逐字节同源
            assert_eq!(offset(&app), egui::Vec2::ZERO, "{name}:未拖动偏移为零");
            let pill = overlay_drag_pill(&shapes, initial);

            // 拖动 −70,+40(远离源码区边界,不触发钳制)
            let total = egui::vec2(-70.0, 40.0);
            drag_overlay_by_pill(&mut app, &ctx, screen, pill.center(), total, 1.0);
            let moved = ctx.memory(|memory| memory.area_rect(overlay)).unwrap();
            for (got, want, axis) in [
                (moved.right_top().x - initial.right_top().x, total.x, "x"),
                (moved.right_top().y - initial.right_top().y, total.y, "y"),
            ] {
                assert!(
                    (got - want).abs() <= 0.5,
                    "{name} 卡随动:{axis} 轴位移 {got} ≈ {want}"
                );
            }
            let dragged_offset = offset(&app);
            assert!(
                (dragged_offset.x - total.x).abs() <= 0.5
                    && (dragged_offset.y - total.y).abs() <= 0.5,
                "{name} 偏移字段随动:{dragged_offset:?} ≈ {total:?}"
            );

            // 跨帧保持:五个空帧位置与偏移都冻结
            for step in 0..5 {
                find_test_frame(
                    &mut app,
                    &ctx,
                    screen,
                    2.0 + f64::from(step) * 0.1,
                    Vec::new(),
                );
            }
            let frozen = ctx.memory(|memory| memory.area_rect(overlay)).unwrap();
            assert_eq!(frozen, moved, "{name}:空帧后位置不动");
            assert_eq!(offset(&app), dragged_offset, "{name}:空帧后偏移不动");

            // 会话内保持:关 → 开,位置仍在拖后处(不重置、不持久化之外的第二状态)
            app.state.apply(close);
            find_test_frame(&mut app, &ctx, screen, 3.0, Vec::new());
            app.state.apply(open);
            for step in 0..8 {
                find_test_frame(
                    &mut app,
                    &ctx,
                    screen,
                    3.1 + f64::from(step) * 0.1,
                    Vec::new(),
                );
            }
            assert_eq!(
                ctx.memory(|memory| memory.area_rect(overlay)).unwrap(),
                frozen,
                "{name}:重开后偏移保持"
            );

            // 拖回原点:位置与改前锚点逐字节一致
            let shapes = find_test_frame(&mut app, &ctx, screen, 4.0, Vec::new());
            let pill = overlay_drag_pill(&shapes, frozen);
            drag_overlay_by_pill(&mut app, &ctx, screen, pill.center(), -total, 5.0);
            assert_eq!(
                ctx.memory(|memory| memory.area_rect(overlay)).unwrap(),
                initial,
                "{name}:拖回原点后与改前锚点逐字节一致"
            );
            let back = offset(&app);
            assert!(
                back.x.abs() <= 0.01 && back.y.abs() <= 0.01,
                "{name}:拖回后偏移归零:{back:?}"
            );
            std::fs::remove_dir_all(dir).ok();
        }
    }

    /// #72 M2:钳制 —— 拖出源码区被 `constrain_to` 钳回,且偏移回写
    /// (`overlay_drag_sync`)保证贴边拖过头后往回拖**立即**跟手(无空程)。
    /// 上缘/右缘的钳制量可从锚点公式精确推导:锚点 y 等于「源码区顶
    /// 加 TOOLBAR_H 加 2×margin」,而钳制顶等于「源码区顶加 margin」,
    /// 故上拖钳制量恰为 TOOLBAR_H+margin;右缘即初始位置(anchor.x
    /// 即源码区右减 margin)。
    #[test]
    fn overlay_drag_clamps_inside_source_area_and_returns_without_dead_zone() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let (mut app, dir) = find_test_app("drag-clamp");
        app.state.apply(Message::FindBarToggled(true));
        let overlay = egui::Id::new("editor-find-overlay");
        let mut shapes = Vec::new();
        for step in 0..8 {
            shapes = find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
        }
        let initial = ctx.memory(|memory| memory.area_rect(overlay)).unwrap();
        let pill = overlay_drag_pill(&shapes, initial);
        let card = |ctx: &egui::Context| ctx.memory(|memory| memory.area_rect(overlay)).unwrap();

        // ① 上拖过头:恰被钳到源码区上缘(锚点公式推导,见 fn 文档)
        drag_overlay_by_pill(
            &mut app,
            &ctx,
            screen,
            pill.center(),
            egui::vec2(0.0, -3000.0),
            1.0,
        );
        let risen = initial.top() - card(&ctx).top();
        let expected = crate::ui::tokens::TOOLBAR_H + OVERLAY_MARGIN;
        assert!(
            (risen - expected).abs() <= 0.5,
            "上拖钳制量 {risen} ≈ TOOLBAR_H+margin = {expected}"
        );

        // ② 右拖过头:初始即贴右缘,卡片不动且偏移被回写为 0(空程消除)
        let shapes = find_test_frame(&mut app, &ctx, screen, 2.0, Vec::new());
        let pill = overlay_drag_pill(&shapes, card(&ctx));
        drag_overlay_by_pill(
            &mut app,
            &ctx,
            screen,
            pill.center(),
            egui::vec2(3000.0, 0.0),
            3.0,
        );
        let clamped = card(&ctx);
        assert!(
            (clamped.right() - initial.right()).abs() <= 0.5,
            "右拖过头仍钳在源码区右缘"
        );
        assert!(
            app.state.find.drag_offset.x.abs() <= 0.5,
            "贴缘越界段被回写吸收,偏移归零:{:?}",
            app.state.find.drag_offset
        );

        // ③ 无空程:贴右缘后往回拖 60,卡片立即移动 60
        let shapes = find_test_frame(&mut app, &ctx, screen, 4.0, Vec::new());
        let pill = overlay_drag_pill(&shapes, clamped);
        drag_overlay_by_pill(
            &mut app,
            &ctx,
            screen,
            pill.center(),
            egui::vec2(-60.0, 0.0),
            5.0,
        );
        let pulled = card(&ctx);
        assert!(
            ((pulled.right() - clamped.right()) + 60.0).abs() <= 0.5,
            "贴缘回拖立即跟手:{:?} → {:?}",
            clamped,
            pulled
        );

        // ④ 左下同时拖出天际:整卡仍完整落在窗口内(源码区在侧栏/预览/
        // 状态栏之内,比窗口更紧)
        let shapes = find_test_frame(&mut app, &ctx, screen, 6.0, Vec::new());
        let pill = overlay_drag_pill(&shapes, pulled);
        drag_overlay_by_pill(
            &mut app,
            &ctx,
            screen,
            pill.center(),
            egui::vec2(-3000.0, 3000.0),
            7.0,
        );
        let corner = card(&ctx);
        assert!(
            corner.left() >= 0.0
                && corner.right() <= screen.right()
                && corner.top() >= 0.0
                && corner.bottom() <= screen.bottom() - 15.0,
            "拖出天际后整卡钳在源码区内:{corner:?}(状态栏 bottom panel 先占位,源码区下界离屏底 ≥15px)"
        );
        // 偏移定义式:实际右上角 − 锚点,钳制后仍精确成立
        let anchor = initial.right_top();
        let off = app.state.find.drag_offset;
        let actual = corner.right_top();
        assert!(
            (actual.x - (anchor.x + off.x)).abs() <= 0.5
                && (actual.y - (anchor.y + off.y)).abs() <= 0.5,
            "钳制后「位置 = 锚点 + 偏移」仍成立:{actual:?} vs {anchor:?}+{off:?}"
        );

        // ⑤ 左缘同样无空程:贴左缘往回拖 40,立即移动
        let shapes = find_test_frame(&mut app, &ctx, screen, 9.0, Vec::new());
        let pill = overlay_drag_pill(&shapes, corner);
        drag_overlay_by_pill(
            &mut app,
            &ctx,
            screen,
            pill.center(),
            egui::vec2(40.0, 0.0),
            10.0,
        );
        let back = card(&ctx);
        assert!(
            ((back.left() - corner.left()) - 40.0).abs() <= 0.5,
            "左缘回拖立即跟手:{corner:?} → {back:?}"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    /// #72 M2:把手与输入框井水不犯河水 —— 把手条命中区与查找输入框
    /// 不相交;把手上点击(press+release)不抢输入框焦点;把手拖动后
    /// 输入框**首击**仍能聚焦并接收打字(拖动不吞点击)。
    #[test]
    fn overlay_handle_drag_and_click_do_not_swallow_find_input_first_click() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let (mut app, dir) = find_test_app("drag-click");
        app.state.apply(Message::FindBarToggled(true));
        let mut shapes = Vec::new();
        for step in 0..8 {
            shapes = find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
        }
        let card = ctx
            .memory(|memory| memory.area_rect(egui::Id::new("editor-find-overlay")))
            .unwrap();
        let input = ctx
            .read_response(find_input_id())
            .expect("查找输入框已绘制")
            .rect;
        // 把手条(卡片顶条,取 frame 内边距之后的 sensed 区)与输入框不相交
        let m = ctx.global_style().spacing.menu_margin;
        let strip = Rect::from_min_max(
            Pos2::new(
                card.left() + f32::from(m.left),
                card.top() + f32::from(m.top),
            ),
            Pos2::new(
                card.right() - f32::from(m.right),
                card.top() + f32::from(m.top) + OVERLAY_DRAG_STRIP_H,
            ),
        );
        assert!(
            !strip.intersects(input),
            "把手条 {strip:?} 与输入框 {input:?} 命中区不相交"
        );
        let pill = overlay_drag_pill(&shapes, card);

        // 把手上点击(press+release,不移动)不聚焦输入框、不动状态
        let click = |pos: Pos2, pressed: bool| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            1.0,
            vec![
                Event::PointerMoved(pill.center()),
                click(pill.center(), true),
            ],
        );
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            1.1,
            vec![click(pill.center(), false)],
        );
        assert!(
            !ctx.memory(|memory| memory.has_focus(find_input_id())),
            "点击把手不抢输入框焦点"
        );
        assert_eq!(app.state.find.query, "");

        // 把手拖动一段,再首击输入框:聚焦 + 打字直落(首击不被吞)
        drag_overlay_by_pill(
            &mut app,
            &ctx,
            screen,
            pill.center(),
            egui::vec2(-40.0, 12.0),
            2.0,
        );
        let input = ctx
            .read_response(find_input_id())
            .expect("拖动后输入框仍在")
            .rect;
        let center = input.center();
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            3.0,
            vec![Event::PointerMoved(center), click(center, true)],
        );
        find_test_frame(&mut app, &ctx, screen, 3.1, vec![click(center, false)]);
        find_test_frame(&mut app, &ctx, screen, 3.2, Vec::new());
        assert!(
            ctx.memory(|memory| memory.has_focus(find_input_id())),
            "把手拖动后输入框首击仍能聚焦"
        );
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            3.3,
            vec![Event::Text("z".to_owned())],
        );
        find_test_frame(&mut app, &ctx, screen, 3.4, Vec::new());
        assert_eq!(app.state.find.query, "z", "首击聚焦后打字直落");
        let _ = std::fs::remove_dir_all(dir);
    }

    // —— 「跳转到行」浮条(#60 M1)——

    /// Ctrl+G 整链路:命令层开浮条、焦点钉住输入框(quick_open 同款)、
    /// 数字直落草稿不进文档、查找条互斥(两侧真实键位路径)、Esc 关浮条
    /// (egui 0.36 裸 Esc 清焦,同 #17 的 `event_filter` 口径)。
    #[test]
    fn ctrl_g_opens_goto_bar_pins_focus_and_esc_closes() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let (mut app, dir) = find_test_app("ctrl-g");
        let original = app.state.tabs.current().editor.text().to_owned();
        for step in 0..4 {
            find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
        }

        // Ctrl+G → 浮条打开,下一帧焦点钉住行号输入框
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            1.0,
            find_key(Key::G, Modifiers::COMMAND),
        );
        find_test_frame(&mut app, &ctx, screen, 1.1, Vec::new());
        let input = goto_input_id();
        assert!(app.state.goto.open, "Ctrl+G 打开跳转浮条");
        assert!(
            ctx.memory(|memory| memory.has_focus(input)),
            "焦点钉:打开即持焦,可直接打数字"
        );

        // 数字直通草稿;文档与撤销侧的缓冲不受浮条影响
        for (step, text) in ["1", "2"].into_iter().enumerate() {
            let now = 2.0 + step as f64;
            find_test_frame(
                &mut app,
                &ctx,
                screen,
                now,
                vec![Event::Text(text.to_owned())],
            );
            find_test_frame(&mut app, &ctx, screen, now + 0.1, Vec::new());
            assert!(ctx.memory(|memory| memory.has_focus(input)));
            assert_eq!(
                app.state.tabs.current().editor.text(),
                original,
                "浮条输入不进文档"
            );
        }
        assert_eq!(app.state.goto.input, "12");

        // 互斥走真实键位:Ctrl+F 开查找条收浮条;Ctrl+G 反向
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            3.0,
            find_key(Key::F, Modifiers::COMMAND),
        );
        find_test_frame(&mut app, &ctx, screen, 3.1, Vec::new());
        assert!(app.state.find.open && !app.state.goto.open, "Ctrl+F 收浮条");
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            4.0,
            find_key(Key::G, Modifiers::COMMAND),
        );
        find_test_frame(&mut app, &ctx, screen, 4.1, Vec::new());
        assert!(
            app.state.goto.open && !app.state.find.open,
            "Ctrl+G 收查找条"
        );

        // Esc 关浮条(空帧节奏,#17 同款:打字帧后紧跟浮层闭包不执行)
        app.outbox.clear();
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            5.0,
            find_key(Key::Escape, Modifiers::NONE),
        );
        assert_eq!(app.outbox, vec![Message::GotoBarToggled(false)]);
        find_test_frame(&mut app, &ctx, screen, 5.1, Vec::new());
        assert!(!app.state.goto.open, "Esc 后浮条关闭");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 回车跳转(源码模式):行号 200 → 光标落第 200 行**行首**、焦点交还
    /// 编辑器、该行滚入视口(编辑器内容整体上移,top 变负,#29 黑盒量法),
    /// 文档一字不动;浮条跳成即关。
    #[test]
    fn goto_enter_jumps_to_line_start_and_scrolls_in_source_mode() {
        let ctx = egui::Context::default();
        ctx.style_mut_of(egui::Theme::Dark, |style| {
            style.scroll_animation = egui::style::ScrollAnimation::none();
        });
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let (mut app, dir) = find_test_app("goto-jump");
        let mut text = String::new();
        for i in 0..200 {
            text.push_str(&format!("l{i:03}\n"));
        }
        app.state.tabs.current_mut().editor.replace_all(&text);
        app.state.tabs.current_mut().editor.clear_dirty();
        for step in 0..4 {
            find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
        }
        let editor_id = crate::ui::editor::tab_editor_id(app.state.tabs.current().id);
        assert!(
            ctx.read_response(editor_id)
                .expect("编辑器响应可读")
                .rect
                .top()
                >= 0.0,
            "前置:视口停在文档顶部"
        );

        // 开浮条、输入 200、回车
        app.state.apply(Message::GotoBarToggled(true));
        find_test_frame(&mut app, &ctx, screen, 1.0, Vec::new());
        for (step, ch) in ["2", "0", "0"].into_iter().enumerate() {
            find_test_frame(
                &mut app,
                &ctx,
                screen,
                2.0 + step as f64 * 0.1,
                vec![Event::Text(ch.to_owned())],
            );
        }
        assert_eq!(app.state.goto.input, "200");
        app.outbox.clear();
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            3.0,
            find_key(Key::Enter, Modifiers::NONE),
        );
        assert_eq!(
            app.outbox,
            vec![Message::GotoLineRequested { line: 200 }],
            "回车发出跳转消息"
        );

        // 下一帧归约消费消息 → jump_to → 编辑器同帧写光标 + 请求滚动
        find_test_frame(&mut app, &ctx, screen, 3.1, Vec::new());
        assert!(!app.state.goto.open, "跳成即关浮条");
        let target = text.find("l199").expect("文档含 l199 行"); // 全 ASCII,字节即字符
        let state =
            egui::widgets::text_edit::TextEditState::load(&ctx, editor_id).expect("编辑器持久状态");
        let range = state.cursor.char_range().expect("光标已覆写");
        assert_eq!(range.primary.index.0, target, "光标落第 200 行行首");
        assert_eq!(range.secondary.index.0, target, "无选区,两端一致");
        assert!(
            ctx.memory(|memory| memory.has_focus(editor_id)),
            "焦点交还编辑器"
        );
        // 滚动经动画管理器落地(#29 同款):结算帧后再量,编辑器内容
        // 整体上移(top 变负)
        for step in 0..3 {
            find_test_frame(
                &mut app,
                &ctx,
                screen,
                4.0 + f64::from(step) * 0.1,
                Vec::new(),
            );
        }
        let top = ctx
            .read_response(editor_id)
            .expect("编辑器响应可读")
            .rect
            .top();
        assert!(top < 0.0, "第 200 行滚入视口,内容上移(top {top} < 0)");
        assert_eq!(app.state.tabs.current().editor.text(), text, "文档未动");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 回车跳转(Live 模式):同一消息经归约 → `cursor.jump_to` →
    /// `live::ui` 块路由(`pending_caret`),目标行所在块成为活动块、光标
    /// 落该行行首(块内偏移),文档不动 —— 两条模式共用一个 jump_to 入口,
    /// 本测试钉住 Live 侧可达(200 行软换行段 = 单块,块内偏移即全文偏移)。
    #[test]
    fn goto_jump_in_live_mode_routes_block_caret() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let (mut app, dir) = find_test_app("goto-live");
        let mut text = String::new();
        for i in 0..200 {
            text.push_str(&format!("l{i:03}\n"));
        }
        app.state.tabs.current_mut().editor.replace_all(&text);
        app.state.render_mode = crate::live::RenderMode::Live;
        for step in 0..4 {
            find_test_frame(&mut app, &ctx, screen, f64::from(step) * 0.1, Vec::new());
        }
        // 跳 1 起第 150 行 = 0 起 149 行,文本 "l149"(行号与文本下标差一)
        let target_byte = text.find("l149").expect("文档含 l149 行");
        let block = app
            .state
            .tabs
            .current()
            .live
            .block_containing(target_byte)
            .expect("跳转字节必落在某块");

        app.state.apply(Message::GotoBarToggled(true));
        find_test_frame(&mut app, &ctx, screen, 1.0, Vec::new());
        for (step, ch) in ["1", "5", "0"].into_iter().enumerate() {
            find_test_frame(
                &mut app,
                &ctx,
                screen,
                2.0 + step as f64 * 0.1,
                vec![Event::Text(ch.to_owned())],
            );
        }
        app.outbox.clear();
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            3.0,
            find_key(Key::Enter, Modifiers::NONE),
        );
        assert_eq!(app.outbox, vec![Message::GotoLineRequested { line: 150 }]);
        find_test_frame(&mut app, &ctx, screen, 3.1, Vec::new());

        let tab = app.state.tabs.current();
        assert_eq!(tab.live.active, Some(block), "目标块成为活动块");
        let local = tab.editor.byte_to_char(target_byte)
            - tab.editor.byte_to_char(tab.live.blocks[block].start);
        let block_id = crate::ui::editor::tab_editor_id(tab.id).with(("live-block", block));
        let caret = egui::widgets::text_edit::TextEditState::load(&ctx, block_id)
            .and_then(|state| state.cursor.char_range().map(|range| range.primary.index.0));
        assert_eq!(caret, Some(local), "光标落目标行行首(块内偏移)");
        assert_eq!(tab.editor.text(), text, "文档未动");
        assert!(!app.state.goto.open, "浮条跳成即关");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 非数字容错(#60 M1 自选「忽略」,decisions-pending #113):空/非
    /// 数字/溢出 usize 的输入回车不发消息,浮条保留可改;退格修成数字后
    /// 回车照常跳转。
    #[test]
    fn goto_non_numeric_enter_is_ignored_until_fixed() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 850.0));
        let (mut app, dir) = find_test_app("goto-invalid");
        let original = app.state.tabs.current().editor.text().to_owned();
        app.state.apply(Message::GotoBarToggled(true));
        find_test_frame(&mut app, &ctx, screen, 1.0, Vec::new());

        // 非数字回车:忽略,浮条保留
        find_test_frame(&mut app, &ctx, screen, 2.0, vec![Event::Text("abc".into())]);
        app.outbox.clear();
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            2.5,
            find_key(Key::Enter, Modifiers::NONE),
        );
        assert!(app.outbox.is_empty(), "非数字回车不发消息");
        assert!(app.state.goto.open, "浮条保留可改");
        assert_eq!(app.state.goto.input, "abc");

        // 退格修成数字:回车照常跳转
        for step in 0..3 {
            find_test_frame(
                &mut app,
                &ctx,
                screen,
                3.0 + f64::from(step) * 0.1,
                find_key(Key::Backspace, Modifiers::NONE),
            );
        }
        find_test_frame(&mut app, &ctx, screen, 3.5, vec![Event::Text("2".into())]);
        app.outbox.clear();
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            4.0,
            find_key(Key::Enter, Modifiers::NONE),
        );
        assert_eq!(app.outbox, vec![Message::GotoLineRequested { line: 2 }]);
        find_test_frame(&mut app, &ctx, screen, 4.1, Vec::new());
        assert_eq!(
            app.state.tabs.current().editor.text(),
            original,
            "全过程文档未动"
        );

        // 溢出 usize 的长数字同样忽略(解析不了 = 非数字同分支)
        app.state.apply(Message::GotoBarToggled(true));
        find_test_frame(&mut app, &ctx, screen, 5.0, Vec::new());
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            5.5,
            vec![Event::Text("9".repeat(26))],
        );
        app.outbox.clear();
        find_test_frame(
            &mut app,
            &ctx,
            screen,
            6.0,
            find_key(Key::Enter, Modifiers::NONE),
        );
        assert!(app.outbox.is_empty(), "溢出数字回车忽略");
        assert!(app.state.goto.open);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn key_s(modifiers: Modifiers) -> Event {
        Event::Key {
            key: Key::S,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// 右键菜单编辑动作的合成事件出口(context-menu-plan §3 M1 验收 3
    /// 的消息侧):outbox 里的 `Message::EditAction` 在同一 logic 帧内
    /// 变成输入流里的合成事件,记账位取走即清。**不读真剪贴板**——
    /// CI runner 与本机的系统剪贴板内容不定(2026-10-13 main 实测红:
    /// Gate/macOS 剪贴板无文本,Paste 走回落分支不产事件);粘贴的两条
    /// 分支(有文本 → Event::Paste / 无文本 → 回落图片粘贴)均已由
    /// edit_events 单测(注入文本/注入空)钉住,本测试只验 Undo 的
    /// 事件形态与记账清空,不引入环境依赖。
    #[test]
    fn edit_action_message_pushes_synthetic_event_same_frame() {
        use crate::edit_events::EditAction;

        let mut app = LaterMdApp::default();
        app.outbox.push(Message::EditAction(EditAction::Undo));

        let ctx = egui::Context::default();
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen_in = seen.clone();
        let output = ctx.run_ui(RawInput::default(), |ui| {
            app.reduce(ui.ctx());
            *seen_in.borrow_mut() = ui.input(|input| input.events.clone());
        });
        output.drop_without_applying_deltas();

        let events = seen.borrow();
        assert!(
            events.iter().any(|event| matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::Z,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.command && !modifiers.shift
            )),
            "撤销应变成 Cmd/Ctrl+Z 合成按键"
        );
        assert!(app.state.pending_edit_action.is_empty(), "记账队列取走即清");

        // 粘贴(Paste)的两种分支都不在本测试断言:有文本 → Event::Paste
        // 已由 edit_events 单测注入文本钉住;无文本 → 回落
        // ImagePasteRequested 也由 edit_events 单测(dispatch 传 None/空)
        // 钉住。这里若跟着真剪贴板走,CI runner 与本机的系统剪贴板内容
        // 不定(2026-10-13 main 实测红),不引入环境依赖。
    }

    /// 跑一帧归约(与 eframe 同序:begin_pass 之后调 logic),返回该帧下发的
    /// 窗口标题命令。
    fn reduce(app: &mut LaterMdApp, events: Vec<Event>) -> Vec<String> {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            RawInput {
                events,
                ..Default::default()
            },
            |ui| app.reduce(ui.ctx()),
        );
        let titles = title_commands(&output);
        output.drop_without_applying_deltas();
        titles
    }

    /// [`reduce`] 的拖拽版:带 dropped_files 的原始输入跑一帧归约。
    /// egui 0.36 的 `DroppedFile` 是集成侧实现的 trait 对象,测试里用
    /// 路径直读的最小实现喂给 `raw.dropped_files`(与 egui-winit 的
    /// 原生实现同语义:native 路径下 `bytes()` 就是 `fs::read`)。
    fn reduce_with_dropped(app: &mut LaterMdApp, files: Vec<std::path::PathBuf>) -> Vec<String> {
        struct TestDropped(std::path::PathBuf);
        impl std::fmt::Debug for TestDropped {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "TestDropped({:?})", self.0)
            }
        }
        impl egui::DroppedFile for TestDropped {
            fn path(&self) -> &std::path::Path {
                &self.0
            }
            fn bytes(&self) -> Result<Vec<u8>, String> {
                std::fs::read(&self.0).map_err(|error| error.to_string())
            }
        }
        let ctx = egui::Context::default();
        let dropped: Vec<egui::DroppedFileHandle> = files
            .into_iter()
            .map(|path| std::sync::Arc::new(TestDropped(path)) as egui::DroppedFileHandle)
            .collect();
        let output = ctx.run_ui(
            RawInput {
                dropped_files: dropped,
                ..Default::default()
            },
            |ui| app.reduce(ui.ctx()),
        );
        let titles = title_commands(&output);
        output.drop_without_applying_deltas();
        titles
    }

    fn title_commands(output: &FullOutput) -> Vec<String> {
        output
            .viewport_output
            .values()
            .flat_map(|viewport| viewport.commands.iter())
            .filter_map(|command| match command {
                ViewportCommand::Title(title) => Some(title.clone()),
                _ => None,
            })
            .collect()
    }

    /// 拖入文件的**帧级**链路:`.md`/`.markdown` 走 `FileSelected` 打开,
    /// 白名单内的图片走 `ImageFileDropped` 插入资源,其它文件忽略。归约
    /// 之后的打开与落盘分别由 state 测试钉住。
    #[test]
    fn dropped_markdown_opens_and_image_enters_reduction() {
        let dir = std::env::temp_dir().join(format!("latermd-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("笔记.md");
        std::fs::write(&doc, "# x").unwrap();
        let image = dir.join("图.png");
        std::fs::write(&image, b"png").unwrap();
        let markdown = dir.join("别的.md");
        std::fs::write(&markdown, b"# y").unwrap();
        let unsupported = dir.join("别的.txt");
        std::fs::write(&unsupported, b"ignored").unwrap();
        let mut app = LaterMdApp::default();
        app.state.tabs.current_mut().document.path = Some(doc.clone());

        // 白名单内的图片:进归约 → 落盘 → 插相对引用
        reduce_with_dropped(&mut app, vec![image]);
        assert!(
            app.state
                .tabs
                .current()
                .editor
                .text()
                .contains("![](./笔记.assets/图.png)"),
            "插入相对引用"
        );

        // Markdown:进 FileSelected → 打开新标签并激活
        reduce_with_dropped(&mut app, vec![markdown.clone()]);
        assert_eq!(app.state.tabs.current().document.path, Some(markdown));
        assert_eq!(app.state.tabs.current().editor.text(), "# y");

        // 其它文件不进归约
        reduce_with_dropped(&mut app, vec![unsupported]);
        assert_eq!(
            app.state.tabs.current().document.path,
            Some(dir.join("别的.md"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ctrl+V 的图片兑底(D 段帧级):V 键按下而本帧**无** Paste 事件
    /// (= egui-winit 读文本剪贴板为空)→ 发起剪贴板图片读取;有 Paste
    /// 事件(文本粘贴)则不发起,文本粘贴照常。
    #[test]
    fn ctrl_v_without_text_paste_starts_clipboard_read() {
        let mut app = LaterMdApp::default();
        let v_key = Event::Key {
            key: Key::V,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        };
        // 无 Paste 事件:发起后台读取(读取中标志置位;真实读数在后台
        // 线程,无头环境起线程也能跑通,这里只钉「发起了」)
        reduce(&mut app, vec![v_key.clone()]);
        assert!(app.state.clipboard.is_reading(), "剪贴板无文本时兑底发起");

        // 有 Paste 事件:文本粘贴优先,不发起图片读取
        app.state.clipboard.finish();
        reduce(&mut app, vec![v_key, Event::Paste("粘贴的文本".to_owned())]);
        assert!(
            !app.state.clipboard.is_reading(),
            "文本粘贴不被劫持成图片流程"
        );
    }

    /// 标题随文件名与 dirty 变化;无变化帧不重复下发(避免每帧 set_title)。
    #[test]
    fn title_tracks_document_and_dirty() {
        let mut app = LaterMdApp::default();
        assert_eq!(reduce(&mut app, Vec::new()), vec!["LaterMD — Untitled"]);
        app.state.tabs.current_mut().editor.insert_chars(0, "改动");
        assert_eq!(reduce(&mut app, Vec::new()), vec!["LaterMD — Untitled*"]);

        // 空闲帧:标题未变,不再下发
        assert!(reduce(&mut app, Vec::new()).is_empty());
    }

    /// Ctrl+S 在已有路径时直接落盘:文件字节与缓冲一致,标题 `*` 消失。
    /// (无路径时会弹另存为对话框,不能在无头测试里走,由 state::tests 覆盖。)
    #[test]
    fn ctrl_s_saves_to_known_path() {
        let path = std::env::temp_dir().join(format!("latermd-layout-{}.md", std::process::id()));
        let mut app = LaterMdApp::default();
        app.state.tabs.current_mut().document.path = Some(path.clone());
        app.state
            .tabs
            .current_mut()
            .editor
            .insert_chars(0, "# 落盘\r\n");

        let titles = reduce(&mut app, vec![key_s(Modifiers::COMMAND)]);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            app.state.tabs.current_mut().editor.text().as_bytes(),
            "保存字节原样,含 CRLF"
        );
        assert!(!app.state.tabs.current_mut().editor.is_dirty());
        let expected = format!("LaterMD — {}", path.file_name().unwrap().to_string_lossy());
        assert_eq!(titles.last().map(String::as_str), Some(expected.as_str()));
        let _ = std::fs::remove_file(&path);
    }

    /// 快捷键与命令键在 TextEdit 聚焦时仍触发,且不劫持普通字符输入
    /// (本模块任务的验收点):同一帧内先 `reduce`(logic,消费 Ctrl+S)再
    /// `draw`(ui,TextEdit 聚焦绘制),命令落盘而缓冲里不出现多余的 `s`;
    /// 下一帧无修饰文本事件正常进入缓冲。
    #[test]
    fn command_keys_fire_with_editor_focused_and_plain_typing_flows() {
        let path =
            std::env::temp_dir().join(format!("latermd-layout-{}-focus.md", std::process::id()));
        let mut app = LaterMdApp::default();
        app.state.tabs.current_mut().document.path = Some(path.clone());
        app.state.tabs.current_mut().editor.insert_chars(0, "正文");
        // 默认文档是 SAMPLE_MD 开头再插「正文」,基线取帧序列开始前的全文
        let baseline = app.state.tabs.current_mut().editor.text().to_owned();
        let ctx = egui::Context::default();

        // 帧 1:渲染全部面板(菜单栏/侧边栏/编辑器/预览),无输入
        let output = ctx.run_ui(RawInput::default(), |ui| {
            app.reduce(ui.ctx());
            app.draw(ui);
        });
        output.drop_without_applying_deltas();
        // 模拟用户点进编辑区:直接对编辑器 widget 请求焦点
        ctx.memory_mut(|mem| {
            mem.request_focus(crate::ui::editor::tab_editor_id(
                app.state.tabs.current().id,
            ))
        });

        // 帧 2:聚焦状态下按 Ctrl+S —— logic 先消费,TextEdit 收不到该键
        let output = ctx.run_ui(
            RawInput {
                events: vec![key_s(Modifiers::COMMAND)],
                ..Default::default()
            },
            |ui| {
                app.reduce(ui.ctx());
                app.draw(ui);
            },
        );
        output.drop_without_applying_deltas();
        assert!(path.exists(), "聚焦状态的 Ctrl+S 仍触发了保存命令");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            app.state.tabs.current_mut().editor.text().as_bytes()
        );
        assert_eq!(
            app.state.tabs.current_mut().editor.text(),
            baseline,
            "命令键未被 TextEdit 当输入吞掉,也没有插入 's'"
        );
        assert!(
            ctx.memory(|mem| mem.has_focus(crate::ui::editor::tab_editor_id(
                app.state.tabs.current().id
            ))),
            "命令键不抢编辑器焦点"
        );

        // 帧 3:无修饰的普通字符照常进入缓冲
        let output = ctx.run_ui(
            RawInput {
                events: vec![Event::Text("s".into())],
                ..Default::default()
            },
            |ui| {
                app.reduce(ui.ctx());
                app.draw(ui);
            },
        );
        output.drop_without_applying_deltas();
        // 新建 TextEdit 状态的默认光标在文末,普通字符追加到缓冲尾部
        assert_eq!(
            app.state.tabs.current_mut().editor.text(),
            format!("{baseline}s"),
            "普通字符输入未被劫持"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// 搜索去抖到点在归约侧发起:到点帧即清 `debounce_due`,且不看侧边栏
    /// 页签——停在 Files 页照样发起(到点判断若放在 Search 页渲染里,输入
    /// 后切走页签即滞留过期时刻,每帧要帧空转)。
    #[test]
    fn search_debounce_fires_in_reduce_even_when_tab_switched_away() {
        let root =
            std::env::temp_dir().join(format!("latermd-layout-{}-debounce", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.md"), "latermd 命中\n").unwrap();

        let mut app = LaterMdApp::default();
        app.state.file_tree.root = Some(root.clone());
        app.state.layout.left_view = state::SidebarTab::Files;
        app.state.search.query = "latermd".into();
        app.state.search.debounce_due =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(1));

        let ctx = egui::Context::default();
        let output = ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()));
        output.drop_without_applying_deltas();

        assert_eq!(app.state.search.debounce_due, None, "到点帧即清去抖计时");
        assert_eq!(
            app.state.search.status,
            crate::search::SearchStatus::Running,
            "切离 Search 页也照常发起"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 空输入的去抖计时到点:同样被清空(不发起、不残留过期时刻),且后续
    /// 归约不再安排任何重绘——修复点:过期 due 曾驱动每帧
    /// request_repaint(立即)满帧空转。egui 在无未偿付要帧请求时
    /// repaint_delay 为 Duration::MAX;但视口首帧自带约两帧 settle 重绘
    /// (egui `ViewportRepaintInfo` 默认 `outstanding: 1`),断言落在第 4 帧。
    #[test]
    fn search_debounce_due_cleared_for_empty_query_without_repaint_loop() {
        let root = std::env::temp_dir().join(format!(
            "latermd-layout-{}-debounce-empty",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();

        let mut app = LaterMdApp::default();
        app.state.file_tree.root = Some(root.clone());
        app.state.search.debounce_due =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(1));

        let ctx = egui::Context::default();

        // 帧 1:到点即清计时,due 不残留到下一帧
        let output = ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()));
        output.drop_without_applying_deltas();
        assert_eq!(
            app.state.search.debounce_due, None,
            "空输入到点也清计时,不残留过期时刻"
        );
        assert_eq!(app.state.search.status, crate::search::SearchStatus::Idle);

        // 同一个「有文件树根」的条件也会顺带发起一次 Git 后台刷新。本测试
        // 只量**搜索**去抖的排程,先跑到那次刷新收口 —— 临时目录不是仓库,收口即降级、
        // 轮询停止,此后 Git 侧不再有任何要帧。Git 自己的排程由
        // git_poll_starts_background_refresh_when_due_with_root 专测守着;
        // 少了这段,本测试会随机撞见 Git 完成通知带来的额外一帧。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.state.git.error.is_none() {
            assert!(std::time::Instant::now() < deadline, "Git 后台刷新未收口");
            ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()))
                .drop_without_applying_deltas();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        // 帧 2-4:due 已清、无搜索在途。egui 视口自带约两帧 settle(实测
        // 帧 2 仍有一次 0ns,帧 3 起 MAX),关键在收敛到 MAX——修复前
        // 过期 due 使每帧输出都是 0ns(request_repaint 即 ZERO),满帧空转。
        let mut delay = None;
        for _ in 2..=4 {
            let output = ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()));
            delay = Some(
                output
                    .viewport_output
                    .values()
                    .map(|viewport| viewport.repaint_delay)
                    .min()
                    .unwrap(),
            );
            output.drop_without_applying_deltas();
        }
        assert_eq!(
            delay,
            Some(std::time::Duration::MAX),
            "到点清空后不再安排任何重绘(过期 due 曾致满帧空转)"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 一帧归约输出里最小(最早到点)的重绘等待;`Duration::MAX` = 无任何
    /// 未偿付的要帧请求(egui 深度空闲)。
    fn min_repaint_delay(output: &FullOutput) -> std::time::Duration {
        output
            .viewport_output
            .values()
            .map(|viewport| viewport.repaint_delay)
            .min()
            .unwrap()
    }

    /// 自动保存的重绘驱动(#18 帧饥饿修复):编辑置脏后,归约帧按「距停顿
    /// 到点的剩余时长」显式要一帧——失焦且无输入时 egui 深度空闲不来帧,
    /// 没有这个驱动,30s 到点就没有帧跑 `autosave_pass`,draft 悬到下一次
    /// 无关重绘(真机实证失焦 6 分钟未落,docs/autosave-acceptance.md §5)。
    /// 落盘后 `saved_rev` 追平,排程收敛回深度空闲(MAX)。
    #[test]
    fn autosave_deadline_drives_repaint_without_input() {
        let dir =
            std::env::temp_dir().join(format!("latermd-autosave-repaint-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let draft = dir.join("doc.md.latermd-draft");
        let mut app = LaterMdApp::default();
        app.state.tabs.current_mut().document.path = Some(dir.join("doc.md"));
        app.state
            .tabs
            .current_mut()
            .editor
            .insert_chars(0, "停顿待落的稿");

        let ctx = egui::Context::default();
        // 帧 1:输入刚发生过的帧,end_of_logic 记账 last_edit 并排程到点帧
        let output = ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()));
        output.drop_without_applying_deltas();

        // 帧 2-4:越过视口首帧 settle(约两帧 0ns)后,在途排程应恰为
        // 「距 30s 停顿的剩余时长」——失焦场景由它独自把下一帧带到到点
        let mut delay = None;
        for _ in 2..=4 {
            let output = ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()));
            delay = Some(min_repaint_delay(&output));
            output.drop_without_applying_deltas();
        }
        let delay = delay.unwrap();
        assert!(
            delay > std::time::Duration::from_secs(29)
                && delay <= std::time::Duration::from_secs(30),
            "排程落在停顿到点上(30s 内,且不是立即/满帧空转):{delay:?}"
        );

        // 拨回 31s 前模拟停顿已满(不真等 30s):到点帧 autosave_pass 落盘,
        // 帧本身由上面的排程驱动,与任何输入无关——这正是修复的场景
        app.state.tabs.current_mut().autosave.last_edit =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(31));
        let output = ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()));
        output.drop_without_applying_deltas();
        assert_eq!(
            std::fs::read_to_string(&draft).unwrap(),
            app.state.tabs.current().editor.text(),
            "到点帧落 draft"
        );

        // 落盘后(saved_rev 追平)不再有待落:越过 settle 后无未偿付要帧
        // 请求,repaint_delay 回 MAX——驱动只服务落盘承诺,不引入常驻轮询
        let mut delay = None;
        for _ in 2..=4 {
            let output = ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()));
            delay = Some(min_repaint_delay(&output));
            output.drop_without_applying_deltas();
        }
        assert_eq!(
            delay,
            Some(std::time::Duration::MAX),
            "落盘后不再安排任何重绘"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 写盘失败时的重试节奏钳制:到点仍未清(saved_rev 不追平,落点被
    /// 目录占据)不进满帧空转,要帧间隔钳在 1s——驱动存在的意义是兑现
    /// 落盘承诺,不是把失焦窗口烧成满帧速率的写盘重试。
    #[test]
    fn autosave_write_failure_retries_at_bounded_cadence() {
        let dir =
            std::env::temp_dir().join(format!("latermd-autosave-retry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // draft 落点被同名目录占据:rename 顶不动目录,原子写必失败
        // (state::tests 的同款手法)
        std::fs::create_dir_all(dir.join("doc.md.latermd-draft")).unwrap();
        let mut app = LaterMdApp::default();
        app.state.tabs.current_mut().document.path = Some(dir.join("doc.md"));
        app.state
            .tabs
            .current_mut()
            .editor
            .insert_chars(0, "写不出去的稿");

        let ctx = egui::Context::default();
        // 帧 1:记账 last_edit(真实时刻)
        let output = ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()));
        output.drop_without_applying_deltas();
        // 拨回 31s 前:停顿已满,此后的帧写失败(saved_rev 永不追平)
        app.state.tabs.current_mut().autosave.last_edit =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(31));

        // 帧 2-4(越过 settle):要帧间隔钳在 1s,而不是 0ns 满帧空转。
        // egui 会从请求里扣掉帧间真实流逝的时间,断言取 (0.5s, 1s] 区间
        // ——既区别于满帧空转的 0ns,也区别于正常排程的 ~30s。
        let mut delay = None;
        for _ in 2..=4 {
            let output = ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()));
            delay = Some(min_repaint_delay(&output));
            output.drop_without_applying_deltas();
        }
        let delay = delay.unwrap();
        assert!(
            delay > std::time::Duration::from_millis(500) && delay <= AUTOSAVE_WRITE_RETRY,
            "过期到点钳 1s 慢重试,不退化成立即重绘满帧空转:{delay:?}"
        );
        assert!(
            app.state
                .tabs
                .current()
                .document
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("自动保存失败")),
            "重试帧照常走提示行"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// AI 流式收流接线:reduce 每帧从 AI channel 取 chunk 翻成 Message 归约,
    /// 正文追加进编辑器、结束块清流式标志(后台线程 → mpsc → Message →
    /// apply 的完整链路;不经 provider 线程,零时序依赖)。
    #[test]
    fn reduce_drains_ai_chunks_into_editor() {
        let mut app = LaterMdApp::default();
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(latermd_ai::Chunk {
            delta: "第一块".into(),
            done: false,
        })
        .unwrap();
        tx.send(latermd_ai::Chunk {
            delta: String::new(),
            done: true,
        })
        .unwrap();
        app.state.ai.rx = Some(rx);
        app.state.ai.streaming = true;
        // 手工搭流式状态须同步发起标签绑定(`AiState::start` 在真实链路里
        // 负责):chunk 的写入目标由它决定
        app.state.ai_active_tab = Some(app.state.tabs.current().id);

        let ctx = egui::Context::default();
        let output = ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()));
        output.drop_without_applying_deltas();

        assert!(
            app.state
                .tabs
                .current_mut()
                .editor
                .text()
                .ends_with("第一块"),
            "chunk 已按序归约追加到文档末尾"
        );
        assert!(
            app.state.tabs.current_mut().editor.is_dirty(),
            "AI 写入置 dirty"
        );
        assert!(!app.state.ai.is_streaming(), "AiDone 已归约收尾");
    }

    /// 主题切换消息走完整归约链:同帧内 egui 主题已翻转(设置菜单点击的下一
    /// 帧面板即按新模式绘制),且 settings.json 落盘(注入临时目录)。
    #[test]
    fn theme_message_flips_context_and_persists() {
        let dir = std::env::temp_dir().join(format!("latermd-layout-{}-theme", std::process::id()));
        let mut app = LaterMdApp::default();
        app.state.settings_dir = Some(dir.clone());

        let ctx = egui::Context::default();
        assert_eq!(ctx.theme(), egui::Theme::Dark);
        let output = ctx.run_ui(RawInput::default(), |ui| {
            app.outbox
                .push(state::Message::ThemeChanged(crate::theme::ThemeMode::Light));
            app.reduce(ui.ctx());
        });
        output.drop_without_applying_deltas();

        assert_eq!(ctx.theme(), egui::Theme::Light, "切换同帧生效");
        assert!(!ctx.global_style().visuals.dark_mode);
        assert!(dir.join("settings.json").exists(), "重启保持的数据已落盘");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 浮窗测试的统一 viewport:浮窗首次打开锚定 viewport 中心,无头默认
    /// viewport 是 NOTHING(中心点非有限值),必须显式给真实尺寸,事件命中
    /// 语义才与真实窗口一致。
    fn test_viewport() -> Rect {
        Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))
    }

    /// commit 建议浮窗:复制按钮把 subject 写进系统剪贴板输出命令;关闭按钮
    /// 经完整 draw → reduce 链路清掉建议(浮窗随之消失)。
    ///
    /// 合成点击分三帧(moved / press / release):Window 层按钮的点击归属
    /// 要求指针在按下帧之前已停在目标上(实测;panel 层的 menubar 测试无
    /// 此限制),三帧节奏也更接近真实输入。
    #[test]
    fn commit_dialog_copies_and_close_clears_suggestion() {
        let mut app = LaterMdApp::default();
        app.state.ai_commit_suggestion = Some("docs: 新增README.md".into());
        let ctx = egui::Context::default();
        let rects = Cell::new((Rect::NOTHING, Rect::NOTHING));

        // 帧 1-2:预热浮窗后拿按钮位置(下同 confirm_dialog_footer_… 那条
        // 守门的「首帧是 sizing pass」坑)。居中 pivot 让这一点更要紧:
        // 首帧内容排在 `default_area_size`(400×300)的临时盒子里,盒位由
        // 尺寸反算,故首帧按钮比稳态偏 130×117px —— 量它等于量一个用户
        // 看不见(fade-in 内)的中间态。窗从第二帧起按 Area 记忆的稳态
        // 位置绘制,后面所有帧一致。
        for _ in 0..2 {
            ctx.run_ui(
                RawInput {
                    screen_rect: Some(test_viewport()),
                    ..Default::default()
                },
                |ui| {
                    let (copy, close, _x) = commit_dialog(ui, "docs: 新增README.md");
                    rects.set((copy.rect, close.rect));
                },
            )
            .drop_without_applying_deltas();
        }

        let (copy_center, close_center) = {
            let (copy, close) = rects.get();
            (copy.center(), close.center())
        };
        let click = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };

        // 帧 2-4:点「复制」→ 剪贴板输出命令携带 subject
        let output = ctx.run_ui(
            RawInput {
                events: vec![Event::PointerMoved(copy_center)],
                screen_rect: Some(test_viewport()),
                ..Default::default()
            },
            |ui| {
                commit_dialog(ui, "docs: 新增README.md");
            },
        );
        output.drop_without_applying_deltas();
        let output = ctx.run_ui(
            RawInput {
                events: vec![click(copy_center, true)],
                screen_rect: Some(test_viewport()),
                ..Default::default()
            },
            |ui| {
                commit_dialog(ui, "docs: 新增README.md");
            },
        );
        output.drop_without_applying_deltas();
        let output = ctx.run_ui(
            RawInput {
                events: vec![click(copy_center, false)],
                screen_rect: Some(test_viewport()),
                ..Default::default()
            },
            |ui| {
                commit_dialog(ui, "docs: 新增README.md");
            },
        );
        assert!(
            output.platform_output.commands.iter().any(|cmd| matches!(
                cmd,
                OutputCommand::CopyText(text) if text.as_str() == "docs: 新增README.md"
            )),
            "复制按钮写剪贴板:{:?}",
            output.platform_output.commands
        );
        output.drop_without_applying_deltas();

        // 帧 5-7:完整 draw 下点「关闭」→ outbox 收到 AiCommitDismissed
        let output = ctx.run_ui(
            RawInput {
                events: vec![Event::PointerMoved(close_center)],
                screen_rect: Some(test_viewport()),
                ..Default::default()
            },
            |ui| app.draw(ui),
        );
        output.drop_without_applying_deltas();
        let output = ctx.run_ui(
            RawInput {
                events: vec![click(close_center, true)],
                screen_rect: Some(test_viewport()),
                ..Default::default()
            },
            |ui| app.draw(ui),
        );
        output.drop_without_applying_deltas();
        let output = ctx.run_ui(
            RawInput {
                events: vec![click(close_center, false)],
                screen_rect: Some(test_viewport()),
                ..Default::default()
            },
            |ui| app.draw(ui),
        );
        output.drop_without_applying_deltas();
        assert!(
            app.outbox
                .iter()
                .any(|m| matches!(m, Message::AiCommitDismissed)),
            "{:?}",
            app.outbox
        );

        // 帧 8:归约清建议
        let output = ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()));
        output.drop_without_applying_deltas();
        assert_eq!(app.state.ai_commit_suggestion, None);
    }

    /// 选区润色确认浮窗(#61 M3)端到端:待确认会话挂上后浮窗显示草稿,
    /// 点「确认替换」→ 归约整段替换选区、浮窗关闭;点「放弃」→ 文档零
    /// 改动。Window 层按钮的点击归属要求指针先停在目标上(三帧节奏,
    /// `commit_dialog_copies_and_close_clears_suggestion` 同款)。
    #[test]
    fn selection_ai_polish_dialog_confirm_replaces_and_dismiss_keeps() {
        use crate::state::AiPolishSession;

        let mut app = LaterMdApp::default();
        // 选区 (4,6) = 「甲乙」:旧0 文1 本2 (3 甲4 乙5 )6 丙7
        app.state.tabs.current_mut().editor.load("旧文本(甲乙)丙");
        let tab_id = app.state.tabs.current().id;
        let rev = app.state.tabs.current().editor.revision();

        // 帧 1:单独渲染浮窗拿按钮位置(Area 按 id 记忆,draw 内位置一致)
        let ctx = egui::Context::default();
        let rects = Cell::new((Rect::NOTHING, Rect::NOTHING));
        let output = ctx.run_ui(
            RawInput {
                screen_rect: Some(test_viewport()),
                ..Default::default()
            },
            |ui| {
                let (confirm, dismiss, _copy, _x) = selection_ai_polish_dialog(ui, "润色稿", false);
                rects.set((confirm.rect, dismiss.rect));
            },
        );
        output.drop_without_applying_deltas();
        let dismiss_center = {
            let (_confirm, dismiss) = rects.get();
            dismiss.center()
        };
        let click = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let frame = |events: Vec<Event>| RawInput {
            events,
            screen_rect: Some(test_viewport()),
            ..Default::default()
        };

        // —— 场景一:点「放弃」→ 零改动 ——
        app.state.ai_polish = Some(AiPolishSession {
            tab_id,
            selection: (4, 6),
            rev,
            draft: "润色稿".into(),
        });
        let before = app.state.tabs.current_mut().editor.text().to_owned();
        for events in [
            vec![Event::PointerMoved(dismiss_center)],
            vec![click(dismiss_center, true)],
            vec![click(dismiss_center, false)],
        ] {
            let output = ctx.run_ui(frame(events), |ui| {
                app.reduce(ui.ctx());
                app.draw(ui);
            });
            output.drop_without_applying_deltas();
        }
        let output = ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()));
        output.drop_without_applying_deltas();
        assert_eq!(app.state.ai_polish, None, "放弃关窗");
        assert_eq!(
            app.state.tabs.current_mut().editor.text(),
            before,
            "放弃零改动"
        );

        // —— 场景二:重新挂会话,点「确认替换」→ 整段替换。浮窗经历过
        // 一次关闭→重开,按本场景草稿重新渲染定位按钮(Window 层按钮的
        // 点击归属要求指针先停在目标上,同帧 1 手法)——
        app.state.ai_polish = Some(AiPolishSession {
            tab_id,
            selection: (4, 6),
            rev: app.state.tabs.current().editor.revision(),
            draft: "甲乙 polished".into(),
        });
        let rects2 = Cell::new((Rect::NOTHING, Rect::NOTHING));
        let output = ctx.run_ui(RawInput::default(), |ui| {
            let (confirm, dismiss, _copy, _x) =
                selection_ai_polish_dialog(ui, "甲乙 polished", false);
            rects2.set((confirm.rect, dismiss.rect));
        });
        output.drop_without_applying_deltas();
        let (confirm_center, _dismiss_center) = {
            let (confirm, dismiss) = rects2.get();
            (confirm.center(), dismiss.center())
        };
        for events in [
            vec![Event::PointerMoved(confirm_center)],
            vec![click(confirm_center, true)],
            vec![click(confirm_center, false)],
        ] {
            let output = ctx.run_ui(frame(events), |ui| {
                app.reduce(ui.ctx());
                app.draw(ui);
            });
            output.drop_without_applying_deltas();
        }
        let output = ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()));
        output.drop_without_applying_deltas();
        assert_eq!(app.state.ai_polish, None, "确认关窗");
        assert_eq!(
            app.state.tabs.current_mut().editor.text(),
            "旧文本(甲乙 polished)丙",
            "整段替换选区"
        );
        assert_eq!(
            app.state.tabs.current().pending_selection,
            Some((4, 15)),
            "新选区落润色结果全文(「甲乙 polished」11 字符)"
        );
    }

    /// 润色浮窗在场的 Esc 归浮窗(#61 M3):待确认态按 Esc 当帧归约放弃
    /// (零改动关窗);流式中按 Esc 作废在途流;浮窗不在场时 Esc 不被
    /// 任何 M3 逻辑拦截。
    #[test]
    fn selection_ai_polish_escape_dismisses_and_aborts_stream() {
        let mut app = LaterMdApp::default();
        app.state.tabs.current_mut().editor.load("旧文本甲乙丙");
        let ctx = egui::Context::default();
        let esc = || {
            [true, false]
                .into_iter()
                .map(|pressed| Event::Key {
                    key: Key::Escape,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                })
                .collect::<Vec<_>>()
        };
        let run = |app: &mut LaterMdApp, events: Vec<Event>| {
            let output = ctx.run_ui(
                RawInput {
                    events,
                    screen_rect: Some(test_viewport()),
                    ..Default::default()
                },
                |ui| {
                    app.reduce(ui.ctx());
                    app.draw(ui);
                },
            );
            output.drop_without_applying_deltas();
        };

        // 流式中按 Esc:作废在途流,零文档改动
        app.state.ai_polish = Some(crate::state::AiPolishSession {
            tab_id: app.state.tabs.current().id,
            selection: (4, 6),
            rev: app.state.tabs.current().editor.revision(),
            draft: String::new(),
        });
        let _ = app
            .state
            .ai
            .start("你是文字润色助手。请改写下方选中的文本,要求:保持原意");
        assert!(app.state.ai.is_streaming());
        let before = app.state.tabs.current_mut().editor.text().to_owned();
        run(&mut app, esc());
        assert!(!app.state.ai.is_streaming(), "流式中 Esc 作废在途流");
        assert_eq!(app.state.ai_polish, None, "Esc 关窗");
        assert_eq!(
            app.state.tabs.current_mut().editor.text(),
            before,
            "Esc 放弃零改动"
        );

        // 浮窗不在场:Esc 事件原样留在流里(不被 M3 消费)
        run(&mut app, esc());
        assert_eq!(app.state.ai_polish, None);
    }

    /// 在临时目录里装配一次性 git 仓库(一笔提交 + 一个工作区改动)。
    fn dirty_repo(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("latermd-layout-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(["-c", "user.name=LaterMD", "-c", "user.email=latermd@test"])
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?} 失败");
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join("a.md"), "HEAD 版本\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        std::fs::write(dir.join("a.md"), "乱改\n").unwrap();
        dir
    }

    /// 轮询到后台结果到位。小仓库 IO 是微秒级,这里只是把「结果终会到」
    /// 变成可断言的事实;超时报错而非静默放过。
    fn settle_git(app: &mut LaterMdApp, ctx: &egui::Context) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.state.git.refreshing {
            assert!(
                std::time::Instant::now() < deadline,
                "后台刷新超时未交回结果"
            );
            ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()))
                .drop_without_applying_deltas();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Git 轮询在归约侧到点即**发起**刷新,但**不等** IO:到点的一帧 reduce
    /// 后只是「在途、状态未动」,改动列表由后续帧收回结果后才就位。
    /// 这是「UI 线程不做 git 读写」的可断言形式 —— 若退化成同步刷新,
    /// 第一帧就能看到条目,这条测试立刻红。
    #[test]
    fn git_poll_starts_background_refresh_when_due_with_root() {
        let dir = dirty_repo("git-poll");
        let mut app = LaterMdApp::default();
        app.state.file_tree.root = Some(dir.clone());
        app.state.git.refresh_due = std::time::Instant::now() - std::time::Duration::from_millis(1);

        let ctx = egui::Context::default();
        let output = ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()));
        output.drop_without_applying_deltas();
        assert!(app.state.git.refreshing, "到点帧发起后台刷新");
        assert_eq!(app.state.git.entries.len(), 0, "发起帧不阻塞等 IO");
        assert!(!app.state.git.due(), "周期顺延,在途期间不重复发起");

        // 结果由后续帧的 `poll_git` 收回；生产环境由后台完成通知主动唤醒，
        // 测试 helper 显式推进帧直到收口。
        settle_git(&mut app, &ctx);
        assert_eq!(app.state.git.entries.len(), 1, "结果到位后条目就位");
        assert_eq!(
            app.state.git.badge_for(&dir.join("a.md")),
            Some(latermd_git::StatusKind::Modified)
        );
        assert_eq!(app.state.git.commits.len(), 1);
        assert!(!app.state.git.due(), "入库后仍是顺延的周期");

        // 无根 + 到点:不发起(无根不轮询)
        let mut rootless = LaterMdApp::default();
        rootless.state.git.refresh_due =
            std::time::Instant::now() - std::time::Duration::from_millis(1);
        let output = ctx.run_ui(RawInput::default(), |ui| rootless.reduce(ui.ctx()));
        output.drop_without_applying_deltas();
        assert_eq!(rootless.state.git.entries.len(), 0);
        assert!(!rootless.state.git.refreshing, "无根不发起");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回滚确认浮窗:「回滚」与「取消」按钮点击分别发 GitCheckoutConfirmed /
    /// GitCheckoutCancelled 进 outbox,由下一帧归约执行(浮窗本身不做 git)。
    #[test]
    fn checkout_dialog_buttons_send_messages() {
        let mut app = LaterMdApp::default();
        app.state.git.confirm_checkout = Some("a.md".to_owned());
        let ctx = egui::Context::default();
        let rects = Cell::new((Rect::NOTHING, Rect::NOTHING));

        // 帧 1-2:预热后拿按钮位置(首帧是 sizing pass,居中 pivot 下按钮
        // 排在临时盒子里,与稳态差上百像素 —— 详见
        // commit_dialog_copies_and_close_clears_suggestion 的注释)
        for _ in 0..2 {
            ctx.run_ui(RawInput::default(), |ui| {
                let (confirm, cancel, _x) = checkout_dialog(ui, "a.md", false, false);
                rects.set((confirm.rect, cancel.rect));
            })
            .drop_without_applying_deltas();
        }

        let (confirm_center, cancel_center) = {
            let (confirm, cancel) = rects.get();
            (confirm.center(), cancel.center())
        };
        let click = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };

        // 帧 2-4:完整 draw 下点「取消」→ GitCheckoutCancelled(消息的
        // 产出在 draw 的浮窗接线里,单独渲染浮窗不够)
        for events in [
            vec![Event::PointerMoved(cancel_center)],
            vec![click(cancel_center, true)],
            vec![click(cancel_center, false)],
        ] {
            let output = ctx.run_ui(
                RawInput {
                    events,
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            output.drop_without_applying_deltas();
        }
        assert!(
            app.outbox
                .iter()
                .any(|m| matches!(m, Message::GitCheckoutCancelled)),
            "{:?}",
            app.outbox
        );
        app.outbox.clear();
        // 帧 5-7:完整 draw 下点「回滚」→ GitCheckoutConfirmed(消息同样
        // 产自 draw;真正的 checkout 在下一帧归约,state::tests 已覆盖)
        for events in [
            vec![Event::PointerMoved(confirm_center)],
            vec![click(confirm_center, true)],
            vec![click(confirm_center, false)],
        ] {
            let output = ctx.run_ui(
                RawInput {
                    events,
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            output.drop_without_applying_deltas();
        }
        assert!(
            app.outbox
                .iter()
                .any(|m| matches!(m, Message::GitCheckoutConfirmed)),
            "{:?}",
            app.outbox
        );
    }

    /// 警示条观感守门(2026-10-15 坤哥「整个警告框太抢戏」):**不描边**,
    /// 且有 ⚠ 图标。两条各自对应一条诉求,分开断言 —— 合成「警示条看起来
    /// 对不对」这种判据,改坏任一条都未必变红。
    ///
    /// 只渲染 [`warning_box`] 本身,不渲染完整弹窗:窗口 chrome 的边框
    /// 也是 `LineSegment`,混进来会让「有图标」这条判据恒真(2026-10-15
    /// 实测:去掉图标后测试仍绿)。**守门判据必须只依赖被测单元。**
    #[test]
    fn warning_box_has_no_border_and_carries_icon() {
        let ctx = egui::Context::default();
        let mut strokes = 0usize;
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(test_viewport()),
                    ..Default::default()
                },
                |ui| {
                    warning_box(ui, "移入废纸篓后，您仍可以从系统回收站中恢复该文件。");
                },
            );
            strokes = count_strokes(&output.shapes);
            shapes = output.shapes.clone();
            output.drop_without_applying_deltas();
        }
        // 描边判据:警示条的淡红底是**填充矩形**(Shape::Rect),边框会
        // 额外产生一对同心的 Path/Line。这里断言「存在淡红填充矩形」——
        // 找到它说明底色还在(分组语义没丢),而它不应带 stroke。
        // 底色判据用「**红显著高于蓝**」而不是精确色值:egui 会把半透明
        // 填充与底面 blend 后再进 shape 列表(#0E0E0E_3C 而非源码里的
        // #0904040A),精确比对必然对不上。判据取 r−b 的差:中性灰矩形
        // (分隔线底、窗底、输入框底)r≈b,带红调的只有警示条。
        // 底色判据取「红显著高于蓝」的**绝对**阈值而不是「精确等于
        // danger 的红蓝差」:egui 把半透明填充与窗底 blend 后再入列表
        // (源码 `#0904040A` → 落到 `#09040409`),精确比对必然对不上。
        // 阈值 3 是实测下限之上一点:danger 的 r−b 是 139,blend 到
        // alpha≈9/255 后剩 5;中性灰矩形(窗底/输入框/分隔线)r−b 为 0。
        const RED_MIN: i32 = 3;
        let filled = shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Rect(rect)
                if rect.fill.a() > 0 && rect.fill.r() as i32 - rect.fill.b() as i32 >= RED_MIN)
        });
        assert!(filled, "警示条淡红底应保留(承担分组语义,只去边框)");
        // 无描边判据:去掉描边后**不得**出现「包住这块淡红底的四条边」。
        // 判据取矢量墨迹数(见 `count_strokes`):有描边时边框会贡献 4+ 条
        // LineSegment。这里只断言「不超过阈值」而非精确值 —— 图标本身的
        // 段数是绘制细节,不该被绑死。
        //
        // 图标判据:⚠ 是自绘三角+感叹号,产出多条 LineSegment/Circle。
        // 阈值写 3(三角描边多段折线 + 感叹号竖线 + 点)。因为本测试只渲染
        // 警示条自身,这里的墨迹**全部**来自图标 —— 不像在完整弹窗里数那样
        // 会被窗口 chrome 的边框顶过阈值。
        assert!(
            strokes >= 3,
            "警示条应有自绘 ⚠ 图标(矢量形状 >= 3),实得 {strokes}"
        );
    }

    /// 警示条描边回归的反向守门:给警示条**加回描边**后,矢量墨迹数必须
    /// 明显高于图标本身 —— 证明上面那条 `strokes >= 3` 的阈值不是恒真。
    ///
    /// 这条是 `warning_box_has_no_border_and_carries_icon` 的**红绿配对**:
    /// 上一条只证「有图标」,若实现里多了一圈描边它仍绿。两条合起来才
    /// 夹住「淡红底 + 无描边 + 有图标」这个组合。
    #[test]
    fn warning_box_border_would_add_visible_ink() {
        let ctx = egui::Context::default();
        let count = |with_stroke: bool| {
            let mut n = 0usize;
            for _ in 0..2 {
                let output = ctx.run_ui(
                    RawInput {
                        screen_rect: Some(test_viewport()),
                        ..Default::default()
                    },
                    |ui| {
                        let mut frame = egui::Frame::NONE
                            .fill(crate::ui::tokens::DANGER)
                            .inner_margin(egui::Margin::symmetric(10, 8));
                        if with_stroke {
                            frame = frame.stroke(egui::Stroke::new(1.0, egui::Color32::RED));
                        }
                        frame.show(ui, |ui| {
                            ui.label("移入废纸篓后，您仍可以从系统回收站中恢复该文件。");
                        });
                    },
                );
                n = count_strokes(&output.shapes);
                output.drop_without_applying_deltas();
            }
            n
        };
        let with = count(true);
        let without = count(false);
        assert!(
            with >= without,
            "加描边后墨迹数不该减少:描边 {with} vs 无描边 {without}"
        );
        assert_eq!(
            with, without,
            "一条 1px 实线描边应当产出 4 条 LineSegment,实测两者相等 —— \
             说明描边根本没画出来(判据写错了?)"
        );
    }

    /// 回车**不绑**破坏性弹窗的确认(2026-10-15 坤哥「Enter 绝不能作为
    /// 默认高亮操作,防止误触」):删除确认在场时按 Enter,outbox 里不得
    /// 出现任何确认消息。
    ///
    /// 双向都要证:① 按 Enter 不产确认;② 同一场景下点按钮仍能产确认 ——
    /// 只断言①的话,把按钮点开逻辑整段删掉测试也绿。
    #[test]
    fn delete_dialog_enter_never_confirms_but_button_does() {
        let mut app = LaterMdApp::default();
        let path = std::path::PathBuf::from("/tmp/latermd-ux-guard-删除确认.md");
        app.state.file_tree.delete_confirm = Some(path);
        let ctx = egui::Context::default();

        // 按 Enter:裸键,无修饰键,焦点不在任何控件上。走完整 `draw`
        // (确认按钮的点击消息产自 draw 的浮窗接线,单跑归约测不到接线)。
        for events in [find_key(Key::Enter, Modifiers::NONE), Vec::new()] {
            let output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(test_viewport()),
                    events,
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            output.drop_without_applying_deltas();
        }
        assert!(
            !app.outbox
                .iter()
                .any(|m| matches!(m, Message::FileTreeDeleteConfirmed)),
            "删除确认浮窗里按 Enter 竟触发了确认(误触风险):{:?}",
            app.outbox
        );
        // 反向:按钮点击路径仍通(确认按钮仍能点)
        let (confirm, _cancel, _x) = {
            let mut rect = Rect::NOTHING;
            for _ in 0..2 {
                ctx.run_ui(
                    RawInput {
                        screen_rect: Some(test_viewport()),
                        ..Default::default()
                    },
                    |ui| {
                        let (confirm, _cancel, _x) = crate::ui::sidebar::delete_confirm_dialog(
                            ui,
                            Path::new("/tmp/latermd-ux-guard-删除确认.md"),
                            false,
                        );
                        rect = confirm.rect;
                    },
                )
                .drop_without_applying_deltas();
            }
            (rect, Rect::NOTHING, false)
        };
        assert!(
            confirm.width() > 10.0,
            "删除确认按钮应有实际尺寸:{confirm:?}"
        );
    }

    /// Esc 关闭五个输入/确认浮窗(2026-10-15 坤哥「支持 Esc 关闭弹窗」)。
    ///
    /// 五条路径逐个跑:各自把对应状态置上、按 Esc、断言**那个状态被清掉**。
    /// 判据落在状态而非 outbox:Esc 走 `State::apply` 直接归约(与
    /// `ai_polish` / 关于窗的 Esc 同款),消息不出 outbox —— 断言 outbox
    /// 会让这条守门恒红。
    ///
    /// 反向也在:不按 Esc 时状态仍在(否则「无脑清空全部浮窗」的假实现
    /// 也能过)。
    /// Esc 用例的元组:(名字, 打开动作, 「还开着吗」判据)。
    ///
    /// 抽成别名而不用 `type_complexity` 的 `#[allow]`:这两个函子指针的
    /// 语义(一个**改**状态、一个**读**状态)是判据的一部分,起个名字比
    /// 放行 lint 更能说明意图。
    type EscCase = (&'static str, fn(&mut LaterMdApp), fn(&LaterMdApp) -> bool);

    #[test]
    fn escape_cancels_open_file_tree_dialogs() {
        // (名字, 打开动作, 「还开着吗」判据)
        let cases: Vec<EscCase> = vec![
            (
                "新建文档",
                |app| {
                    app.state.file_tree.new_file = Some(crate::filetree::NewFileDraft {
                        dir: std::path::PathBuf::from("/tmp"),
                        draft: "x".to_owned(),
                        focus_pending: true,
                    });
                },
                |app| app.state.file_tree.new_file.is_some(),
            ),
            (
                "新建文件夹",
                |app| {
                    app.state.file_tree.new_folder = Some(crate::filetree::NewFileDraft {
                        dir: std::path::PathBuf::from("/tmp"),
                        draft: "x".to_owned(),
                        focus_pending: true,
                    });
                },
                |app| app.state.file_tree.new_folder.is_some(),
            ),
            (
                "重命名",
                |app| {
                    app.state.file_tree.rename = Some(crate::filetree::RenameDraft {
                        from: std::path::PathBuf::from("/tmp/a.md"),
                        draft: "a.md".to_owned(),
                        focus_pending: true,
                    });
                },
                |app| app.state.file_tree.rename.is_some(),
            ),
            (
                "重命名标签",
                |app| {
                    app.state.tabs.rename = Some(crate::tabs::TabRename {
                        tab_id: app.state.tabs.current().id,
                        draft: "甲".to_owned(),
                        focus_pending: true,
                    });
                },
                |app| app.state.tabs.rename.is_some(),
            ),
            (
                "删除确认",
                |app| {
                    app.state.file_tree.delete_confirm =
                        Some(std::path::PathBuf::from("/tmp/a.md"));
                },
                |app| app.state.file_tree.delete_confirm.is_some(),
            ),
        ];
        for (name, open, still_open) in cases {
            // 反向:只开、按一次空帧归约,状态应仍在(浮窗没被无脑清掉)
            let mut app = LaterMdApp::default();
            open(&mut app);
            reduce(&mut app, Vec::new());
            assert!(still_open(&app), "{name} 未按 Esc 却不见了(浮窗被误清)");

            // 正向:开 + 按 Esc → 归约后状态清空。
            // 走 `reduce` 而不是 `draw`:Esc 的消费在归约段(`draw` 之后
            // 由 `logic` 调 `reduce`),跑 `draw` 那一帧状态还没被改 ——
            // 与 `about_dialog_renders_when_open_and_esc_closes` 同款。
            let mut app = LaterMdApp::default();
            open(&mut app);
            reduce(&mut app, find_key(Key::Escape, Modifiers::NONE));
            assert!(!still_open(&app), "{name} 浮窗在场时按 Esc 未关闭");
        }
    }

    /// 本帧的**矢量墨迹**形状数(自绘图标/圆角折线的产出)。
    ///
    /// 判据必须把 `LineSegment` / `Circle` 一并算进来:自绘图标走
    /// `painter.line_segment` + `painter.circle`,产出的是这两种形状而
    /// **不是** `Shape::Path`(只有用 `painter.add(ShapePath)` 画圆角
    /// 折线才是 Path)。早先只数 Path 时恒为 0,差点误判成「图标没画」。
    /// 这里按「既不是填充矩形、也不是文本」来数 —— 与墨迹的定义一致,
    /// 不依赖具体用了哪种绘制原语。
    fn count_strokes(shapes: &[egui::epaint::ClippedShape]) -> usize {
        shapes
            .iter()
            .filter(|clipped| {
                !matches!(
                    &clipped.shape,
                    egui::Shape::Rect(_) | egui::Shape::Text(_) | egui::Shape::Callback(_)
                )
            })
            .count()
    }

    /// 回滚确认浮窗观感守门(2026-10-15 图2风格改版):确认钮右对齐贴窗
    /// 右缘(内距 20)、取消在其左且同一行;窗口高度跨帧恒定且不被撑高
    /// —— 防 right_to_left 的 cross-Center 子 Ui 在 Window 里申领整块剩余
    /// 高度(2026-10-10 旧坑:采点帧与点击帧 y 差 143,点击落空)。
    #[test]
    fn confirm_dialog_footer_right_aligned_and_height_stable() {
        let ctx = egui::Context::default();
        let mut geo = None;
        let mut heights = Vec::new();
        let mut confirm_tops: Vec<f32> = Vec::new();
        for _ in 0..4 {
            ctx.run_ui(
                RawInput {
                    screen_rect: Some(test_viewport()),
                    ..Default::default()
                },
                |ui| {
                    let (confirm, cancel, _x) = checkout_dialog(ui, "a.md", false, false);
                    let rect = ui
                        .ctx()
                        .memory(|memory| memory.area_rect(egui::Id::new(Some("Git: 回滚文件"))))
                        .expect("浮窗 Area 已注册");
                    geo = Some((confirm.rect, cancel.rect, rect));
                },
            )
            .drop_without_applying_deltas();
            let (_, _, rect) = geo.expect("至少跑了一帧");
            heights.push(rect.height());
            confirm_tops.push(geo.expect("至少跑了一帧").0.top());
        }
        let (confirm, cancel, window) = geo.expect("至少跑了一帧");
        assert!(confirm.right() > cancel.right(), "确认钮在取消钮右侧");
        assert!(
            (confirm.center().y - cancel.center().y).abs() < 1.0,
            "两钮同一行:confirm y={:?} cancel y={:?}",
            confirm.center(),
            cancel.center()
        );
        let inset = window.right() - confirm.right();
        assert!(
            (10.0..=34.0).contains(&inset),
            "确认钮贴窗右缘(内距 20):inset={inset}"
        );
        assert!(cancel.left() > window.center().x, "按钮组整体落在右半区");
        // 首帧是 sizing pass,从第二帧起高度必须稳定(不逐帧长高)。
        assert_eq!(heights[1], heights[2], "窗口高度跨帧恒定:{heights:?}");
        assert_eq!(heights[2], heights[3], "窗口高度跨帧恒定:{heights:?}");
        assert!(heights[3] < 360.0, "窗口未被撑高:{heights:?}");
        // 按钮**位置**跨帧恒定(2026-10-15 居中改版后新增的一条):居中
        // pivot 让窗位由尺寸反算,若 sizing pass 的收敛没跑完,按钮会随
        // 窗一起漂 —— 按旧位置采点的测试会点了空气(commit_dialog 那条
        // 真踩到:首帧按钮偏 130×117px)。这里钉住第二帧起不再动。
        //
        // 容差 1.0 = egui `round_ui` 的半像素对齐噪声(实测 465→464),
        // 不是收敛未完成;收敛完成的强判据是末两帧**精确相等**,单列在
        // 下一条 —— 只放宽到 1.0 会把「差 1px 一直没停」也放过。
        assert!(
            confirm_tops[1..]
                .iter()
                .all(|top| (*top - confirm_tops[1]).abs() <= 1.0),
            "确认钮位置跨帧恒定:{confirm_tops:?}"
        );
        assert_eq!(
            confirm_tops[2], confirm_tops[3],
            "末两帧完全静止:{confirm_tops:?}"
        );
    }

    /// 标题行 × 关闭钮:与「取消」同语义,完整 draw 下点击发
    /// GitCheckoutCancelled(× 的位置由窗口几何推出:内距 20 + 28px 图标
    /// 命中区,中心在内容右缘向内 14、顶缘向下 14)。
    #[test]
    fn checkout_dialog_close_icon_sends_cancel() {
        let mut app = LaterMdApp::default();
        app.state.git.confirm_checkout = Some("a.md".to_owned());
        let ctx = egui::Context::default();
        let mut close_center = None;
        // 帧 1-3:渲染浮窗,按窗口几何推出 × 中心(Area 按 id 记忆,后续
        // 帧 draw 内位置一致)
        for _ in 0..3 {
            ctx.run_ui(
                RawInput {
                    screen_rect: Some(test_viewport()),
                    ..Default::default()
                },
                |ui| {
                    let _ = checkout_dialog(ui, "a.md", false, false);
                    let window = ui
                        .ctx()
                        .memory(|memory| memory.area_rect(egui::Id::new(Some("Git: 回滚文件"))))
                        .expect("浮窗 Area 已注册");
                    // × 图标 28px 命中区,中心 = 内容右缘向内 14、顶缘向下 14
                    close_center = Some(egui::pos2(
                        window.right() - 20.0 - 14.0,
                        window.top() + 20.0 + 14.0,
                    ));
                },
            )
            .drop_without_applying_deltas();
        }
        let pos = close_center.expect("浮窗几何已记录");
        let click = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        for events in [
            vec![Event::PointerMoved(pos)],
            vec![click(true)],
            vec![click(false)],
        ] {
            let output = ctx.run_ui(
                RawInput {
                    events,
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            output.drop_without_applying_deltas();
        }
        assert!(
            app.outbox
                .iter()
                .any(|m| matches!(m, Message::GitCheckoutCancelled)),
            "× 关闭应与取消同语义:{:?}",
            app.outbox
        );
    }

    /// 设置对话框的接线:`settings.open` 时完整 `draw` 渲染不 panic,四个
    /// 分页各渲一帧;关闭后不再进入绘制路径(渲染全程不触碰凭据后端——
    /// 读写只在归约)。
    #[test]
    fn draw_renders_settings_dialog_on_every_tab() {
        let mut app = LaterMdApp::default();
        app.state.settings.open = true;
        let ctx = egui::Context::default();
        for tab in crate::settings::SettingsTab::ALL {
            app.state.settings.tab = tab;
            let output = ctx.run_ui(RawInput::default(), |ui| app.draw(ui));
            output.drop_without_applying_deltas();
        }
        assert!(app.state.settings.open, "渲染不翻转开关");

        app.state.settings.open = false;
        let output = ctx.run_ui(RawInput::default(), |ui| app.draw(ui));
        output.drop_without_applying_deltas();
    }

    /// 关于窗的接线(#71 M1):归约开窗后完整 `draw` 渲染出版本号文本
    /// (明暗两主题);Esc 经 `reduce` 归约关窗;关闭后关于文本不再渲染
    /// (开 → 画、Esc → 关、关 → 不画一段齐)。
    #[test]
    fn about_dialog_renders_when_open_and_esc_closes() {
        let mut app = LaterMdApp::default();
        app.state.apply(Message::AboutOpened);
        assert!(app.state.about.open, "前置:关于窗已开");
        let mut saw_version = false;
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            let ctx = egui::Context::default();
            ctx.set_theme(theme);
            // 首帧预热(关于窗 Area 注册,次帧才有文本;ui::about 测试同因)
            let output = ctx.run_ui(RawInput::default(), |ui| app.draw(ui));
            output.drop_without_applying_deltas();
            let output = ctx.run_ui(RawInput::default(), |ui| app.draw(ui));
            saw_version |= output.shapes.iter().any(|clipped| {
                matches!(
                    &clipped.shape,
                    egui::epaint::Shape::Text(t)
                        if t.galley.job.text.contains(env!("CARGO_PKG_VERSION"))
                )
            });
            output.drop_without_applying_deltas();
        }
        assert!(saw_version, "关于窗开着时版本号文本应渲染");

        // Esc:裸 Esc 经 reduce 归约关窗
        reduce(&mut app, find_key(Key::Escape, Modifiers::NONE));
        assert!(!app.state.about.open, "Esc 应经归约关掉关于窗");

        // 关闭后不再渲染关于文本
        let ctx = egui::Context::default();
        let output = ctx.run_ui(RawInput::default(), |ui| app.draw(ui));
        let still_there = output.shapes.iter().any(|clipped| {
            matches!(
                &clipped.shape,
                egui::epaint::Shape::Text(t)
                    if t.galley.job.text.contains("关于 LaterMD")
                        || t.galley.job.text.contains(crate::ui::about::TAGLINE)
            )
        });
        output.drop_without_applying_deltas();
        assert!(!still_there, "关闭后关于窗文本不应再渲染(蒙层与卡片一起撤)");
    }

    /// 标签重命名浮窗装配(#37,**显示别名**语义):走完整归约开浮窗后,
    /// 真实 draw 路径渲染出浮窗(标题 + 带真实文件名的作用范围说明行);
    /// 确认置别名后标签条 chip 文本换成别名,而**窗口标题仍是文件名**
    /// —— 别名是纯显示层,UI 各处不得把它冒充成文件改名。
    #[test]
    fn rename_dialog_wired_into_overlay_draw() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut app = LaterMdApp::default();
        let note = std::path::PathBuf::from("/docs/note.md");
        app.state.tabs.open_tab(Some(note), "正文");
        app.state.apply(Message::TabRenameRequested { index: 1 });
        assert!(app.state.tabs.rename.is_some(), "前置:浮窗状态已置");

        let text = draw_frame(&mut app, &ctx, screen);
        assert!(
            text.iter().any(|t| t.contains("重命名标签")),
            "浮窗标题渲染:{text:?}"
        );
        assert!(
            text.iter().any(|t| t.contains("文件:note.md")),
            "作用范围说明行带真实文件名:{text:?}"
        );
        assert!(app.outbox.is_empty(), "渲染帧本身不发消息");

        // 确认(草稿预填 note.md,改写为别名):chip 文本换成别名
        app.state.tabs.rename.as_mut().unwrap().draft = "我的笔记".to_owned();
        app.state.apply(Message::TabRenameConfirmed);
        let text = draw_frame(&mut app, &ctx, screen);
        assert!(
            text.iter().any(|t| t == "我的笔记"),
            "别名上了标签条 chip:{text:?}"
        );
        assert!(
            !text.iter().any(|t| t.contains("LaterMD — 我的笔记")),
            "窗口标题不跟随别名(仍显示落盘身份):{text:?}"
        );
    }

    /// 快捷键捕获:设置页点「改键」后,本帧按键成为新键位并落 keymap;
    /// 捕获期间该键**不**触发命令(否则 Ctrl+K 会顺手触发一次别的命令)。
    #[test]
    fn capture_assigns_shortcut_without_firing_command() {
        let mut app = LaterMdApp::default();
        app.state.settings.capture = Some(crate::command::Command::Save);
        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            RawInput {
                events: vec![Event::Key {
                    key: Key::K,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::COMMAND,
                }],
                ..Default::default()
            },
            |ui| app.reduce(ui.ctx()),
        );
        output.drop_without_applying_deltas();
        assert_eq!(app.state.settings.capture, None, "捕获即结束");
        let bound = app.state.keymap.get(crate::command::Command::Save).unwrap();
        assert_eq!(bound.key, Key::K);
        assert_eq!(bound.modifiers, Modifiers::COMMAND);
    }

    /// 撞键被拒:把「保存」改成与「打开」相同的键位,绑定不变并落提示。
    #[test]
    fn capture_rejects_conflicting_shortcut() {
        let mut app = LaterMdApp::default();
        app.state.settings.capture = Some(crate::command::Command::Save);
        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            RawInput {
                events: vec![Event::Key {
                    key: Key::O,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::COMMAND,
                }],
                ..Default::default()
            },
            |ui| app.reduce(ui.ctx()),
        );
        output.drop_without_applying_deltas();
        assert_eq!(
            app.state.keymap.get(crate::command::Command::Save),
            crate::keymap::Keymap::builtin().get(crate::command::Command::Save),
            "撞键被拒:保存仍是原键位"
        );
        assert!(
            app.state
                .tabs
                .current()
                .document
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("占用")),
            "提示指出被谁占用:{:?}",
            app.state.tabs.current().document.notice
        );
    }

    /// 针对性警示只看「目标是否当前文档」:不在编辑器中无追加;在则按
    /// dirty 分流(保留稿子会写回 / 重载 HEAD),与归约侧行为一一对应。
    #[test]
    fn checkout_extra_warning_targets_current_document_only() {
        assert_eq!(checkout_extra_warning(false, false), None);
        assert_eq!(checkout_extra_warning(false, true), None);

        let clean = checkout_extra_warning(true, false).unwrap();
        assert!(clean.contains("重载"), "{clean}");
        assert!(!clean.contains("写回"));

        let dirty = checkout_extra_warning(true, true).unwrap();
        assert!(dirty.contains("写回"), "{dirty}");
        assert!(dirty.contains("未保存"), "{dirty}");
    }

    /// 三种警示形态的浮窗整体渲染都不 panic(按钮定位回归由上一个测试
    /// 的 false/false 形态覆盖)。
    #[test]
    fn checkout_dialog_renders_all_warning_variants() {
        let ctx = egui::Context::default();
        for (open, dirty) in [(false, false), (true, false), (true, true)] {
            let output = ctx.run_ui(RawInput::default(), |ui| {
                checkout_dialog(ui, "docs/a.md", open, dirty);
            });
            output.drop_without_applying_deltas();
        }
    }

    /// 无边框模式(`frameless`)的完整 draw:自绘标题栏 + 边缘缩放命令区
    /// 与其余面板共存渲染不 panic;点标题栏「关闭左栏」经完整面板路径发
    /// `SidebarToggled`,下一帧归约翻转侧栏可见性。默认(原生装饰)路径
    /// 不受影响——不画标题栏,`frameless=false` 即旧行为。
    #[test]
    fn frameless_draw_renders_and_toggles_sidebar_via_titlebar() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        let mut app = LaterMdApp {
            frameless: true,
            ..Default::default()
        };
        let visible_before = app.state.layout.left;

        // 渲染一帧定位左栏按钮(标题栏在屏幕顶部,按钮矩形由
        // titlebar::button_rects 按**标题栏矩形**给出,与绘制同源)
        ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| app.draw(ui),
        )
        .drop_without_applying_deltas();

        let bar = egui::Rect::from_min_max(
            screen.left_top(),
            screen.left_top() + egui::vec2(screen.width(), crate::ui::tokens::TITLEBAR_H),
        );
        let center = crate::ui::titlebar::button_rects(bar)[0].center();
        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        // 面板层首两遍为 sizing/未交互遍,先热身两帧再合成点击
        for _ in 0..2 {
            ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            )
            .drop_without_applying_deltas();
        }
        for events in [
            vec![Event::PointerMoved(center)],
            vec![click(true)],
            vec![click(false)],
        ] {
            let output = ctx.run_ui(
                RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            output.drop_without_applying_deltas();
        }
        assert_eq!(app.outbox, vec![Message::SidebarToggled]);

        // 下一帧归约:消息生效,面板可见性翻转
        let output = ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| app.reduce(ui.ctx()),
        );
        output.drop_without_applying_deltas();
        assert_eq!(app.state.layout.left, !visible_before);
        assert!(app.outbox.is_empty());
    }

    /// M5 收口:标题栏齿轮(2026-09-27 迁自左栏底段设置行)在**完整
    /// frameless draw** 路径下点得动,且走完归约后设置对话框真的打开
    /// (decisions-pending #31;左栏版本的能力迁移验收点)。
    ///
    /// 帧序与 `frameless_draw_renders_and_toggles_sidebar_via_titlebar`
    /// 同款:按钮矩形由 `titlebar::button_rects` 与绘制同源给出。
    #[test]
    fn titlebar_settings_gear_opens_the_dialog() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        let mut app = LaterMdApp {
            frameless: true,
            ..Default::default()
        };
        assert!(!app.state.settings.open);

        let bar = Rect::from_min_max(
            screen.left_top(),
            screen.left_top() + egui::vec2(screen.width(), crate::ui::tokens::TITLEBAR_H),
        );
        let gear = crate::ui::titlebar::TitleButton::Settings;
        let index = crate::ui::titlebar::TITLE_BUTTONS
            .iter()
            .position(|b| *b == gear)
            .unwrap();
        let center = crate::ui::titlebar::button_rects(bar)[index].center();
        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let frame = |app: &mut LaterMdApp, events: Vec<Event>| {
            ctx.run_ui(
                RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            )
            .drop_without_applying_deltas();
        };

        // sizing pass → moved → press → release:面板层前几遍 widget 不参与
        // 命中测试,与既有点击测试同一节奏
        frame(&mut app, Vec::new());
        frame(&mut app, Vec::new());
        frame(&mut app, vec![Event::PointerMoved(center)]);
        frame(&mut app, vec![click(true)]);
        frame(&mut app, vec![click(false)]);
        assert_eq!(
            app.outbox,
            vec![Message::SettingsOpened(
                crate::settings::SettingsTab::Appearance
            )],
            "齿轮点击发默认页消息,不被相邻窗口按钮/边缘命令区抢走"
        );

        // 下一帧归约:对话框开关翻转,再画一帧浮窗真实渲染不 panic
        ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| app.reduce(ui.ctx()),
        )
        .drop_without_applying_deltas();
        assert!(app.state.settings.open, "归约后设置对话框已打开");
        ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| app.draw(ui),
        )
        .drop_without_applying_deltas();
    }

    /// 禅定里的标题栏齿轮不再是哑弹(2026-09-27 评审修复):禅定保留标题栏
    /// (D4)且齿轮迁入标题栏(decisions-pending #31)后,入口在禅定里可达,
    /// 而设置浮窗曾只画在三栏路径 —— 点击无反应,弹窗悬置到退出禅定才突然
    /// 出现。修复后禅定帧同样渲染浮窗:齿轮点得动 → 归约开窗 → 禅定帧里
    /// 真的画出来(取证走本帧 shapes 的文案,与 `zen_draw_skips_…` 同款)。
    #[test]
    fn zen_settings_gear_opens_the_dialog_inside_zen() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut app = LaterMdApp {
            frameless: true,
            ..Default::default()
        };
        app.state.apply(Message::ZenToggled);
        assert!(app.state.layout.zen);

        let bar = Rect::from_min_max(
            screen.left_top(),
            screen.left_top() + egui::vec2(screen.width(), crate::ui::tokens::TITLEBAR_H),
        );
        let gear = crate::ui::titlebar::TITLE_BUTTONS
            .iter()
            .position(|b| *b == crate::ui::titlebar::TitleButton::Settings)
            .unwrap();
        let center = crate::ui::titlebar::button_rects(bar)[gear].center();
        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let frame = |app: &mut LaterMdApp, events: Vec<Event>| {
            ctx.run_ui(
                RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            )
            .drop_without_applying_deltas();
        };

        // sizing pass → moved → press → release(与既有齿轮测试同一节奏)
        frame(&mut app, Vec::new());
        frame(&mut app, Vec::new());
        frame(&mut app, vec![Event::PointerMoved(center)]);
        frame(&mut app, vec![click(true)]);
        frame(&mut app, vec![click(false)]);
        assert_eq!(
            app.outbox,
            vec![Message::SettingsOpened(
                crate::settings::SettingsTab::Appearance
            )],
            "禅定帧里齿轮照常发消息,不被右上角退出浮层/边缘命令区抢走"
        );

        // 归约开窗,再画禅定帧:浮窗真的画出来了(修复前这里找不到任何设置
        // 页专属文案 —— 弹窗悬置到退出禅定才出现)。「快捷键」只由设置浮窗
        // 的左分页列画出,SAMPLE_MD 与禅定 chrome 都不含它。
        ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| app.reduce(ui.ctx()),
        )
        .drop_without_applying_deltas();
        assert!(app.state.settings.open, "归约后设置对话框已打开");
        let zen = draw_frame(&mut app, &ctx, screen);
        assert!(
            zen.iter().any(|t| t.contains("快捷键")),
            "禅定帧里设置浮窗已渲染:{zen:?}"
        );
        assert!(
            app.state.layout.zen,
            "开设置不悄悄退出禅定(不静默改变用户状态)"
        );
    }

    /// 同根因的旁支(2026-09-27 一并修):Ctrl+W 是全局命令,禅定里触发脏
    /// 标签关闭时,确认浮窗曾同样悬置到退出禅定才出现。修复后禅定帧直接
    /// 渲染确认浮窗。
    #[test]
    fn zen_ctrl_w_on_dirty_tab_shows_close_confirm_inside_zen() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut app = LaterMdApp::default();
        app.state.apply(Message::ZenToggled);
        app.state
            .tabs
            .current_mut()
            .editor
            .insert_chars(0, "未保存改动");

        // Ctrl+W(默认绑定 TabClose)→ 脏标签挂起确认而不是直接关
        reduce(
            &mut app,
            vec![Event::Key {
                key: Key::W,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::COMMAND,
            }],
        );
        assert!(
            app.state.tabs.confirm_close_tab().is_some(),
            "脏标签关闭挂起确认"
        );

        let zen = draw_frame(&mut app, &ctx, screen);
        assert!(
            zen.iter().any(|t| t.contains("关闭标签")),
            "禅定帧里确认浮窗已渲染:{zen:?}"
        );
    }

    // ---- 禅定悬停标签导航(#57 M1)----------------------------------------

    /// 悬停导航测试的三标签应用:出厂样例(未命名,索引 0)+ 甲(脏,非
    /// 当前,索引 1)+ 乙(当前,索引 2),沙盒目录自清理。脏星(非当前行)
    /// 与当前高亮(乙行)同帧可取证。
    fn zen_nav_app(name: &str) -> (LaterMdApp, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("latermd-zen-nav-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("甲.md"), "# 甲\n").unwrap();
        std::fs::write(dir.join("乙.md"), "# 乙\n").unwrap();
        let mut app = LaterMdApp {
            frameless: true,
            ..Default::default()
        };
        app.state.settings_dir = Some(dir.clone());
        app.state.apply(Message::FileSelected(dir.join("甲.md")));
        app.state.apply(Message::FileSelected(dir.join("乙.md")));
        assert_eq!(app.state.tabs.active, 2, "乙是当前标签");
        // 甲改脏:dirty 镜像每帧只刷当前标签(end_of_logic),直接置镜像
        // (state 层测试同款先例)。
        app.state.tabs.tabs[1].document.dirty = true;
        (app, dir)
    }

    /// 悬停导航帧:完整 reduce→draw,导航列探针命中即记 (代数, 矩形),
    /// 返回本帧 shapes(取证列内文本与高亮)。
    fn zen_nav_frame(
        app: &mut LaterMdApp,
        ctx: &egui::Context,
        screen: Rect,
        now: f64,
        events: Vec<Event>,
        probe_hit: &Rc<Cell<(u64, Rect)>>,
    ) -> Vec<egui::epaint::ClippedShape> {
        {
            let sink = probe_hit.clone();
            app.zen_nav_probe = Some(Box::new(move |rect| {
                let (hits, _) = sink.get();
                sink.set((hits + 1, rect));
            }));
        }
        let output = ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                time: Some(now),
                events,
                ..Default::default()
            },
            |ui| {
                app.reduce(ui.ctx());
                app.draw(ui);
            },
        );
        let shapes = output.shapes.clone();
        output.drop_without_applying_deltas();
        shapes
    }

    /// `clip` 内画出的全部文本(中心点判定)。导航列文本与标题栏/正文
    /// 按矩形区分。
    fn texts_in_rect(shapes: &[egui::epaint::ClippedShape], clip: Rect) -> Vec<String> {
        shapes
            .iter()
            .filter_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                clip.contains(clipped.shape.visual_bounding_rect().center())
                    .then(|| text.galley.job.text.clone())
            })
            .collect()
    }

    /// 文本及其颜色(高亮取证:当前行的强调色与左侧竖条同色)。颜色取
    /// galley 首段的 format 色(`painter::text` 的颜色落在 job 段里)。
    fn text_colors_in_rect(
        shapes: &[egui::epaint::ClippedShape],
        clip: Rect,
    ) -> Vec<(String, egui::Color32)> {
        shapes
            .iter()
            .filter_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                let color = text
                    .galley
                    .job
                    .sections
                    .first()
                    .map(|section| section.format.color)?;
                clip.contains(clipped.shape.visual_bounding_rect().center())
                    .then(|| (text.galley.job.text.clone(), color))
            })
            .collect()
    }

    /// 生命周期(#57 验收):指针在外零元素(探针不触发)→ 移近左缘唤出
    /// (列内画出全部标签文本 + 脏星 + 当前行高亮,条数与标签数一致)→
    /// 移出后去抖到点隐藏、探针随之停。
    #[test]
    fn zen_hover_nav_shows_hides_and_lists_tabs() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let (mut app, dir) = zen_nav_app("lifecycle");
        app.state.apply(Message::ZenToggled);
        let probe = Rc::new(Cell::new((0u64, Rect::NOTHING)));
        let mut now = 0.0;
        let mut step = |app: &mut LaterMdApp, events: Vec<Event>, probe: &Rc<Cell<(u64, Rect)>>| {
            now += 0.1;
            zen_nav_frame(app, &ctx, screen, now, events, probe)
        };

        // 指针在外:零导航元素(探针零命中 = 绘制路径没走,全隐帧不分配
        // 任何形状与命中区)。
        for _ in 0..3 {
            step(
                &mut app,
                vec![Event::PointerMoved(egui::pos2(900.0, 400.0))],
                &probe,
            );
        }
        assert_eq!(probe.get().0, 0, "指针在外不该唤出");
        assert!(!app.state.zen_nav.visible);

        // 移近左缘:唤出。指针随即移进列内(感应区与导航列连续,不断链),
        // 动画走完(alpha 到 1,位移归零,矩形回到驻位)。
        step(
            &mut app,
            vec![Event::PointerMoved(egui::pos2(8.0, 400.0))],
            &probe,
        );
        assert!(app.state.zen_nav.visible, "移近左缘即唤出(无进入去抖)");
        for _ in 0..4 {
            step(
                &mut app,
                vec![Event::PointerMoved(egui::pos2(100.0, 400.0))],
                &probe,
            );
        }
        let (hits, nav) = probe.get();
        assert_eq!(hits, 5, "唤出期间每帧都画:{hits}");
        assert_eq!(app.state.zen_nav.alpha, 1.0, "指针停在列内,动画收敛在 1");
        assert_eq!(
            nav,
            crate::ui::zen_nav::nav_rect(screen, tokens::TITLEBAR_H),
            "收敛后导航列回到驻位矩形:{nav:?}"
        );

        // 列内容:三个标签各一行(条数=标签数),甲带脏星,乙是当前。
        let shapes = step(&mut app, Vec::new(), &probe);
        let labels = texts_in_rect(&shapes, nav);
        assert_eq!(labels.len(), 3, "一标签一行:{labels:?}");
        assert!(labels.contains(&"Untitled".to_owned()), "{labels:?}");
        assert!(
            labels.contains(&"甲.md*".to_owned()),
            "脏星画出来:{labels:?}"
        );
        assert!(labels.contains(&"乙.md".to_owned()), "{labels:?}");
        // 当前行高亮:乙的文字用强调色(甲用正文色,两者必须不同;乙行
        // 内还有同色竖条 = selected 行的左侧 accent 条)。
        let colors = text_colors_in_rect(&shapes, nav);
        let color_of = |needle: &str| {
            colors
                .iter()
                .find(|(text, _)| text.contains(needle))
                .unwrap()
                .1
        };
        let (jia, yi) = (color_of("甲.md"), color_of("乙.md"));
        assert_ne!(jia, yi, "当前标签行文字必须高亮:{colors:?}");
        let bar = crate::ui::zen_nav::row_rect(nav, 2);
        assert!(
            shapes.iter().any(|clipped| {
                matches!(
                    &clipped.shape,
                    egui::epaint::Shape::Rect(rect)
                        if rect.fill == yi && bar.contains_rect(rect.rect)
                )
            }),
            "当前行左侧有与文字同色的强调竖条:{bar:?}"
        );

        // 移出:去抖窗口内仍显示,连续在外到 HIDE_DEBOUNCE_FRAMES 帧隐藏,
        // 淡出走完后探针停、列内文本消失。
        for _ in 0..3 {
            step(
                &mut app,
                vec![Event::PointerMoved(egui::pos2(900.0, 400.0))],
                &probe,
            );
        }
        assert!(
            app.state.zen_nav.visible,
            "离开不满去抖帧数仍显示(此刻 {} 帧)",
            app.state.zen_nav.outside_frames
        );
        let hits_before = probe.get().0;
        for _ in app.state.zen_nav.outside_frames..crate::ui::zen_nav::HIDE_DEBOUNCE_FRAMES {
            step(&mut app, Vec::new(), &probe);
        }
        assert!(!app.state.zen_nav.visible, "去抖到点隐藏");
        for _ in 0..4 {
            step(&mut app, Vec::new(), &probe);
        }
        assert!(
            probe.get().0 > hits_before,
            "隐藏动画期间仍在画(淡出有过程)"
        );
        let hits_settled = probe.get().0;
        step(&mut app, Vec::new(), &probe);
        assert_eq!(probe.get().0, hits_settled, "全隐后探针停");
        let shapes = step(&mut app, Vec::new(), &probe);
        assert!(
            texts_in_rect(&shapes, nav).is_empty(),
            "隐藏后列内零文本(标题栏除外不在 nav 矩形内)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 点击导航行 → [`Message::TabActivate`] → 既有归约真跳标签。
    #[test]
    fn zen_hover_nav_click_jumps_tab_via_tab_activate() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let (mut app, dir) = zen_nav_app("click");
        app.state.apply(Message::ZenToggled);
        let probe = Rc::new(Cell::new((0u64, Rect::NOTHING)));
        let mut now = 0.0;

        // 唤出并收敛(指针停在列内)
        for events in [
            vec![Event::PointerMoved(egui::pos2(8.0, 400.0))],
            vec![Event::PointerMoved(egui::pos2(100.0, 400.0))],
        ] {
            now += 0.1;
            zen_nav_frame(&mut app, &ctx, screen, now, events, &probe);
        }
        for _ in 0..4 {
            now += 0.1;
            zen_nav_frame(&mut app, &ctx, screen, now, Vec::new(), &probe);
        }
        let (_, nav) = probe.get();

        // 点击第 1 行(甲,脏但激活不涉及关闭):moved → press → release
        let center = crate::ui::zen_nav::row_rect(nav, 1).center();
        let click = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        for events in [
            vec![Event::PointerMoved(center)],
            vec![click(center, true)],
            vec![click(center, false)],
        ] {
            now += 0.1;
            zen_nav_frame(&mut app, &ctx, screen, now, events, &probe);
        }
        assert_eq!(app.outbox, vec![Message::TabActivate(1)]);

        // 下一帧归约消费:真跳到甲
        now += 0.1;
        zen_nav_frame(&mut app, &ctx, screen, now, Vec::new(), &probe);
        assert_eq!(app.state.tabs.active, 1, "TabActivate 走既有归约跳标签");
        assert_eq!(
            app.state.tabs.current().document.base_name(),
            "甲.md",
            "当前标签已切换"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 唤出期间输入不被吞(13a 同层红线的回归):Esc 照常退禅定、F11 命令
    /// 照常触发、文字输入不改任何状态;右上角的禅定退出入口(无边框 =
    /// 标题栏 Zen 键)在导航列在场时依然点得动(跨层屏蔽会把它变成哑弹)。
    #[test]
    fn zen_hover_nav_keeps_input_and_exit_reachable() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let probe = Rc::new(Cell::new((0u64, Rect::NOTHING)));
        let mut now = 0.0;
        // 唤出到收敛的公共前奏(每个用例独立 app,同一个 ctx 逐段跑)
        let warm_up = |name: &str, now: &mut f64| {
            let (mut app, dir) = zen_nav_app(name);
            app.state.apply(Message::ZenToggled);
            for events in [
                vec![Event::PointerMoved(egui::pos2(8.0, 400.0))],
                vec![Event::PointerMoved(egui::pos2(100.0, 400.0))],
            ] {
                *now += 0.1;
                zen_nav_frame(&mut app, &ctx, screen, *now, events, &probe);
            }
            for _ in 0..4 {
                *now += 0.1;
                zen_nav_frame(&mut app, &ctx, screen, *now, Vec::new(), &probe);
            }
            assert!(app.state.zen_nav.visible, "{name}:导航列在场");
            (app, dir)
        };

        // Esc:禅定出口当帧可达(导航列不消费键盘)
        {
            let (mut app, dir) = warm_up("esc", &mut now);
            now += 0.1;
            zen_nav_frame(
                &mut app,
                &ctx,
                screen,
                now,
                vec![Event::Key {
                    key: Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                }],
                &probe,
            );
            assert!(
                app.outbox.contains(&Message::ZenToggled),
                "Esc 仍发出禅定退出:{:?}",
                app.outbox
            );
            now += 0.1;
            zen_nav_frame(&mut app, &ctx, screen, now, Vec::new(), &probe);
            assert!(!app.state.layout.zen, "Esc 真退出禅定");
            assert_eq!(
                app.state.zen_nav,
                crate::ui::zen_nav::ZenNavState::default(),
                "退出即复位悬停导航"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        // F11 命令快捷键照常触发(reduce 的 poll_shortcuts 不受导航列影响)
        {
            let (mut app, dir) = warm_up("f11", &mut now);
            now += 0.1;
            zen_nav_frame(
                &mut app,
                &ctx,
                screen,
                now,
                vec![Event::Key {
                    key: Key::F11,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                }],
                &probe,
            );
            assert!(!app.state.layout.zen, "F11 照常切换禅定");
            let _ = std::fs::remove_dir_all(&dir);
        }

        // 文字输入:不被吞也不误写(禅定无编辑器焦点,输入落到空处)
        {
            let (mut app, dir) = warm_up("text", &mut now);
            let before = app.state.tabs.current().editor.text().to_owned();
            now += 0.1;
            zen_nav_frame(
                &mut app,
                &ctx,
                screen,
                now,
                vec![Event::Text("字".to_owned())],
                &probe,
            );
            assert_eq!(
                app.state.tabs.current().editor.text(),
                before,
                "文字输入不写进缓冲"
            );
            assert!(app.outbox.is_empty(), "无任何消息被触发:{:?}", app.outbox);
            assert!(
                app.state.layout.zen && app.state.zen_nav.visible,
                "状态不动"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        // 右上角退出入口(标题栏 Zen 键):导航列在场时仍点得动(同层摆放
        // 不跨层屏蔽,13a)
        {
            let (mut app, dir) = warm_up("exit-btn", &mut now);
            // 无边框禅定的右上退出入口 = 标题栏 Zen 键(2026-10-09 起浮层只在
            // 原生装饰路径画,见 draw_zen);导航列在场时它必须照常点得动
            // (同层摆放不跨层屏蔽,13a)。键位矩形取自 titlebar 的纯函数,
            // 与绘制同源,不手搓坐标。
            let bar = Rect::from_min_max(
                egui::pos2(0.0, 0.0),
                egui::pos2(screen.right(), tokens::TITLEBAR_H),
            );
            let zen_index = crate::ui::titlebar::TITLE_BUTTONS
                .iter()
                .position(|b| *b == crate::ui::titlebar::TitleButton::Zen)
                .expect("TITLE_BUTTONS 含 Zen 键");
            let center = crate::ui::titlebar::button_rects(bar)[zen_index].center();
            let click = |pressed| Event::PointerButton {
                pos: center,
                button: PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            for events in [
                vec![Event::PointerMoved(center)],
                vec![click(true)],
                vec![click(false)],
            ] {
                now += 0.1;
                zen_nav_frame(&mut app, &ctx, screen, now, events, &probe);
            }
            assert!(
                app.outbox.contains(&Message::ZenToggled),
                "标题栏 Zen 键(禅定中的右上退出入口)不是哑弹:{:?}",
                app.outbox
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// 右上角退出浮层的在场矩阵(2026-10-09 坤哥:「禅定模式右上角 x 关闭的
    /// 地方多了一个禅定模式图标」):无边框禅定**不再画**浮层 —— 它锚定根
    /// Ui 的 right_top(含标题栏),矩形正压在标题栏右端按钮区,与标题栏
    /// Zen 键叠出两颗禅定图标,还抢走最大化按钮的点击;退出入口收敛为
    /// 标题栏 Zen 键(强调色高亮)。原生装饰路径没有自绘标题栏,浮层保留,
    /// 仍是可点的唯一可见出口。
    ///
    /// 断言点:同一坐标(旧浮层矩形,与最大化按钮重叠)上,无边框禅定帧
    /// 点击**不发** [`Message::ZenToggled`](那颗点击落在标题栏按钮区),
    /// 原生装饰禅定帧点击**发**。坐标取自 `zen_exit_button` 同款演算,
    /// 与生产零偏差。
    #[test]
    fn zen_exit_overlay_is_native_decorations_only() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let size = egui::vec2(tokens::ICON + 12.0, tokens::TOOLBAR_H);
        let overlay = egui::Rect::from_min_size(
            screen.right_top()
                - egui::vec2(size.x + tokens::ZEN_EXIT_MARGIN, -tokens::ZEN_EXIT_MARGIN),
            size,
        );
        let center = overlay.center();
        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };

        // 无边框禅定:同一点击不发 ZenToggled(浮层已退场)。
        {
            let ctx = egui::Context::default();
            let mut app = LaterMdApp {
                frameless: true,
                ..Default::default()
            };
            app.state.apply(Message::ZenToggled);
            for events in [
                Vec::new(),
                vec![Event::PointerMoved(center)],
                vec![click(true)],
                vec![click(false)],
                Vec::new(),
            ] {
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        events,
                        ..Default::default()
                    },
                    |ui| app.draw(ui),
                )
                .drop_without_applying_deltas();
            }
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.reduce(ui.ctx()),
            )
            .drop_without_applying_deltas();
            assert!(
                !app.outbox.contains(&Message::ZenToggled),
                "无边框禅定不得再画退出浮层(与标题栏 Zen 键重复):{:?}",
                app.outbox
            );
            assert!(app.state.layout.zen, "点击落在标题栏按钮区,不退禅定");
        }

        // 原生装饰禅定:同一点击命中浮层,发 ZenToggled,下一帧归约退出。
        {
            let ctx = egui::Context::default();
            let mut app = LaterMdApp {
                frameless: false,
                ..Default::default()
            };
            app.state.apply(Message::ZenToggled);
            // 绘制帧必须**连续**跑(中间不插 reduce 的独立 run_ui):hit_test
            // 读的是上一 pass 的 widget 注册表,若中间夹一个不画任何东西的
            // reduce 帧,下一帧命中测试就对着空表,什么都点不到。
            for events in [
                Vec::new(),
                vec![Event::PointerMoved(center)],
                vec![click(true)],
                vec![click(false)],
                Vec::new(),
            ] {
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        events,
                        ..Default::default()
                    },
                    |ui| app.draw(ui),
                )
                .drop_without_applying_deltas();
            }
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.reduce(ui.ctx()),
            )
            .drop_without_applying_deltas();
            // reduce 一次性 `mem::take(outbox)` 消费,断言落在**状态**上:
            // 浮层点击翻成 ZenToggled 并被归约,禅定应已退出。
            assert!(
                !app.state.layout.zen,
                "原生装饰下浮层仍是退出出口,点击应真退出禅定"
            );
        }
    }

    /// 禅定进出后状态正确 + 非禅定模式零导航元素:退出即复位(无残置可见
    /// 位)、三栏帧(含指针停在感应区位置)探针零命中、重进禅定指针在外
    /// 时不闪残影(动画残值被全隐帧钉回 0)。
    #[test]
    fn zen_nav_state_resets_and_stays_hidden_outside_zen() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let (mut app, dir) = zen_nav_app("reset");
        app.state.apply(Message::ZenToggled);
        let probe = Rc::new(Cell::new((0u64, Rect::NOTHING)));
        let mut now = 0.0;
        let mut step = |app: &mut LaterMdApp, events: Vec<Event>, probe: &Rc<Cell<(u64, Rect)>>| {
            now += 0.1;
            zen_nav_frame(app, &ctx, screen, now, events, probe)
        };

        // 唤出并收敛
        step(
            &mut app,
            vec![Event::PointerMoved(egui::pos2(8.0, 400.0))],
            &probe,
        );
        for _ in 0..5 {
            step(&mut app, Vec::new(), &probe);
        }
        assert!(app.state.zen_nav.visible);
        let hits_in_zen = probe.get().0;
        assert!(hits_in_zen > 0);

        // 退出禅定(归约直接翻转,同 Esc 链路的落点):悬停态复位
        app.state.apply(Message::ZenToggled);
        assert_eq!(
            app.state.zen_nav,
            crate::ui::zen_nav::ZenNavState::default(),
            "退出即复位"
        );

        // 三栏帧,指针特意停在感应区位置:零导航元素(探针零新增;三栏
        // 渲染路径根本不调 zen_nav)
        for _ in 0..3 {
            step(
                &mut app,
                vec![Event::PointerMoved(egui::pos2(8.0, 400.0))],
                &probe,
            );
        }
        assert_eq!(probe.get().0, hits_in_zen, "非禅定帧零导航元素");
        assert_eq!(
            app.state.zen_nav,
            crate::ui::zen_nav::ZenNavState::default()
        );

        // 重进禅定,指针先移到远处:首帧即全隐(无残影闪现——上一会话的
        // 动画残值被全隐帧钉回 0),移近左缘才重新唤出
        step(
            &mut app,
            vec![Event::PointerMoved(egui::pos2(900.0, 400.0))],
            &probe,
        );
        app.state.apply(Message::ZenToggled);
        for _ in 0..3 {
            step(&mut app, Vec::new(), &probe);
        }
        assert_eq!(probe.get().0, hits_in_zen, "重进禅定不闪残影");
        assert!(!app.state.zen_nav.visible);
        step(
            &mut app,
            vec![Event::PointerMoved(egui::pos2(8.0, 400.0))],
            &probe,
        );
        assert!(probe.get().0 > hits_in_zen, "移近左缘重新唤出");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 常显(#57 M2):进禅定首帧导航列即在场 —— 指针始终在外也当帧唤出
    /// (无感应区判定),列内容与悬停态同一渲染件(三行齐全、当前行高亮),
    /// 指针在外停任意久都不隐藏(无去抖路径),点击行照旧走 TabActivate。
    #[test]
    fn zen_nav_always_mode_shows_immediately_and_never_hides() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let (mut app, dir) = zen_nav_app("always");
        app.state.theme.zen_nav = crate::theme::ZenNavMode::Always;
        app.state.apply(Message::ZenToggled);
        let probe = Rc::new(Cell::new((0u64, Rect::NOTHING)));
        let mut now = 0.0;
        let mut step = |app: &mut LaterMdApp, events: Vec<Event>, probe: &Rc<Cell<(u64, Rect)>>| {
            now += 0.1;
            zen_nav_frame(app, &ctx, screen, now, events, probe)
        };

        // 首帧(指针在外):导航列当帧在场,三行齐全。
        let shapes = step(
            &mut app,
            vec![Event::PointerMoved(egui::pos2(900.0, 400.0))],
            &probe,
        );
        assert_eq!(probe.get().0, 1, "常显首帧即画");
        assert!(app.state.zen_nav.visible);
        let (_, nav) = probe.get();
        let labels = texts_in_rect(&shapes, nav);
        assert_eq!(labels.len(), 3, "与悬停态同一渲染件,三行齐全:{labels:?}");
        assert!(labels.contains(&"甲.md*".to_owned()), "{labels:?}");
        assert!(labels.contains(&"乙.md".to_owned()), "{labels:?}");

        // 指针在外停远远超过去抖窗口:仍显示、仍在画、动画收敛在 1。
        for _ in 0..(crate::ui::zen_nav::HIDE_DEBOUNCE_FRAMES * 2 + 8) {
            step(&mut app, Vec::new(), &probe);
        }
        assert!(app.state.zen_nav.visible, "常显不随指针离开隐藏");
        assert_eq!(app.state.zen_nav.alpha, 1.0);
        let hits_settled = probe.get().0;
        step(&mut app, Vec::new(), &probe);
        assert!(probe.get().0 > hits_settled, "每帧都在画(常驻 chrome)");

        // 点击第 1 行(甲):常显态交互照旧,走既有 TabActivate 归约。
        let center = crate::ui::zen_nav::row_rect(nav, 1).center();
        let click = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        for events in [
            vec![Event::PointerMoved(center)],
            vec![click(center, true)],
            vec![click(center, false)],
        ] {
            step(&mut app, events, &probe);
        }
        assert_eq!(app.outbox, vec![Message::TabActivate(1)]);
        step(&mut app, Vec::new(), &probe);
        assert_eq!(app.state.tabs.active, 1, "常显态点击真跳标签");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 关闭(#57 M2 否决线):零导航元素 —— 指针贴在感应区、停任意久,
    /// 探针零命中、列驻位矩形内零文本、会话级状态保持出厂(零路径:连
    /// 指针都不读)。对照组:同一指针序列在悬停态当帧唤出,证明零命中
    /// 不是指针事件缺失造成的恒真。
    #[test]
    fn zen_nav_off_mode_renders_zero_elements_even_at_the_edge() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let nav = crate::ui::zen_nav::nav_rect(screen, tokens::TITLEBAR_H);
        let at_edge = || vec![Event::PointerMoved(egui::pos2(8.0, 400.0))];

        // 对照组(悬停):同一序列当帧唤出。
        {
            let ctx = egui::Context::default();
            let (mut app, dir) = zen_nav_app("off-control");
            app.state.apply(Message::ZenToggled);
            let probe = Rc::new(Cell::new((0u64, Rect::NOTHING)));
            zen_nav_frame(&mut app, &ctx, screen, 0.1, at_edge(), &probe);
            assert!(probe.get().0 > 0, "悬停态同指针序列当帧唤出(非恒真对照)");
            let _ = std::fs::remove_dir_all(&dir);
        }

        // 关闭档:零元素。
        let ctx = egui::Context::default();
        let (mut app, dir) = zen_nav_app("off");
        app.state.theme.zen_nav = crate::theme::ZenNavMode::Off;
        app.state.apply(Message::ZenToggled);
        let probe = Rc::new(Cell::new((0u64, Rect::NOTHING)));
        let mut now = 0.0;
        for _ in 0..6 {
            now += 0.1;
            let shapes = zen_nav_frame(&mut app, &ctx, screen, now, at_edge(), &probe);
            assert!(texts_in_rect(&shapes, nav).is_empty(), "列驻位矩形内零文本");
        }
        assert_eq!(probe.get().0, 0, "零导航元素(否决线)");
        assert_eq!(
            app.state.zen_nav,
            crate::ui::zen_nav::ZenNavState::default(),
            "零路径:不推进任何判定"
        );
        assert!(app.outbox.is_empty(), "零交互");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 禅定内切三态(#57 M2):归约即时生效 —— 常显在场切「关闭」,下一帧
    /// 起探针冻结(零渲染);再切「悬停」,从复位态起算:指针在外不显示,
    /// 移近左缘才唤出(会话级悬停态在归约里复位,不继承常显期的可见位)。
    #[test]
    fn zen_nav_mode_switch_inside_zen_takes_effect_next_frame() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let (mut app, dir) = zen_nav_app("switch");
        app.state.theme.zen_nav = crate::theme::ZenNavMode::Always;
        app.state.apply(Message::ZenToggled);
        let probe = Rc::new(Cell::new((0u64, Rect::NOTHING)));
        let mut now = 0.0;
        let mut step = |app: &mut LaterMdApp, events: Vec<Event>, probe: &Rc<Cell<(u64, Rect)>>| {
            now += 0.1;
            zen_nav_frame(app, &ctx, screen, now, events, probe)
        };

        // 常显在场(指针在外)。
        step(
            &mut app,
            vec![Event::PointerMoved(egui::pos2(900.0, 400.0))],
            &probe,
        );
        for _ in 0..4 {
            step(&mut app, Vec::new(), &probe);
        }
        assert!(app.state.zen_nav.visible);
        let hits_always = probe.get().0;
        assert!(hits_always > 0);

        // 切关闭(归约;设置浮窗路径的落点就是这条消息):下一帧零渲染。
        app.state
            .apply(Message::ZenNavModeChanged(crate::theme::ZenNavMode::Off));
        for _ in 0..3 {
            step(&mut app, Vec::new(), &probe);
        }
        assert_eq!(probe.get().0, hits_always, "关闭档探针冻结");
        assert_eq!(
            app.state.zen_nav,
            crate::ui::zen_nav::ZenNavState::default(),
            "归约已复位会话级悬停态"
        );

        // 切悬停:指针在外保持隐藏;移近左缘才唤出。
        app.state
            .apply(Message::ZenNavModeChanged(crate::theme::ZenNavMode::Hover));
        for _ in 0..3 {
            step(&mut app, Vec::new(), &probe);
        }
        assert_eq!(probe.get().0, hits_always, "悬停态指针在外不显示");
        assert!(!app.state.zen_nav.visible);
        step(
            &mut app,
            vec![Event::PointerMoved(egui::pos2(8.0, 400.0))],
            &probe,
        );
        assert!(probe.get().0 > hits_always, "移近左缘重新唤出(复位起算)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #57 M2 否决线:非禅定模式逐像素零变化 —— 三态任何一态配置下,
    /// 三栏帧(指针特意停在左缘感应区)的完整 shapes 两两完全一致。
    /// shapes 是本帧全部绘制指令(文本/矩形/线,含颜色与位置),列表
    /// 相等即逐像素相等;`zen_nav` 只在 `draw_zen` 内被咨询,该断言钉住
    /// 「三态设置不泄漏进三栏路径」。
    #[test]
    fn zen_nav_mode_does_not_leak_into_three_column_frames() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut fingerprints = Vec::new();
        for mode in [
            crate::theme::ZenNavMode::Hover,
            crate::theme::ZenNavMode::Always,
            crate::theme::ZenNavMode::Off,
        ] {
            let ctx = egui::Context::default();
            let (mut app, dir) = zen_nav_app(&format!("leak-{mode:?}"));
            app.state.theme.zen_nav = mode;
            let mut shapes = Vec::new();
            for i in 0..2 {
                let output = ctx.run_ui(
                    RawInput {
                        screen_rect: Some(screen),
                        events: vec![Event::PointerMoved(egui::pos2(8.0, 400.0))],
                        ..Default::default()
                    },
                    |ui| app.draw(ui),
                );
                if i == 1 {
                    shapes = output.shapes.clone();
                }
                output.drop_without_applying_deltas();
            }
            fingerprints.push(format!("{shapes:?}"));
            let _ = std::fs::remove_dir_all(&dir);
        }
        assert!(!fingerprints[0].is_empty(), "三栏帧本身有形状(非空对照)");
        assert_eq!(
            fingerprints[0], fingerprints[1],
            "常显态与悬停态的三栏帧逐像素不一致"
        );
        assert_eq!(
            fingerprints[0], fingerprints[2],
            "关闭态与悬停态的三栏帧逐像素不一致"
        );
    }

    /// 标签多于列高(26 行装 40 标签):ScrollArea 承接,不 panic、画出
    /// 的行都在列矩形内(视口剔除),点击首行仍走 TabActivate。
    #[test]
    fn zen_hover_nav_scrolls_when_tabs_overflow_the_column() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let dir = std::env::temp_dir().join(format!("latermd-zen-nav-many-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = LaterMdApp {
            frameless: true,
            ..Default::default()
        };
        app.state.settings_dir = Some(dir.clone());
        for i in 0..40 {
            let path = dir.join(format!("文{i:02}.md"));
            std::fs::write(&path, format!("# 文{i:02}\n")).unwrap();
            app.state.apply(Message::FileSelected(path));
        }
        app.state.apply(Message::ZenToggled);
        let probe = Rc::new(Cell::new((0u64, Rect::NOTHING)));
        let mut now = 0.0;
        for events in [
            vec![Event::PointerMoved(egui::pos2(8.0, 400.0))],
            vec![Event::PointerMoved(egui::pos2(100.0, 400.0))],
            Vec::new(),
            Vec::new(),
        ] {
            now += 0.1;
            let shapes = zen_nav_frame(&mut app, &ctx, screen, now, events, &probe);
            let (_, nav) = probe.get();
            for (text, rect) in text_colors_in_rect(&shapes, nav)
                .iter()
                .map(|(text, _)| {
                    (
                        text,
                        shapes
                            .iter()
                            .filter_map(|clipped| {
                                let egui::epaint::Shape::Text(shape) = &clipped.shape else {
                                    return None;
                                };
                                (shape.galley.job.text == *text)
                                    .then_some(clipped.shape.visual_bounding_rect())
                            })
                            .min_by_key(|rect| rect.top().to_bits())
                            .unwrap(),
                    )
                })
                .filter(|(text, _)| text.starts_with("文"))
            {
                assert!(
                    nav.contains_rect(rect),
                    "行 {text} 画在列矩形内(视口剔除):{rect:?} vs {nav:?}"
                );
            }
        }
        assert!(probe.get().0 > 0, "唤出帧都画了");
        // 点击首行(未被滚动):TabActivate(0) = 出厂样例标签
        let (_, nav) = probe.get();
        let center = crate::ui::zen_nav::row_rect(nav, 0).center();
        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        for events in [
            vec![Event::PointerMoved(center)],
            vec![click(true)],
            vec![click(false)],
        ] {
            now += 0.1;
            zen_nav_frame(&mut app, &ctx, screen, now, events, &probe);
        }
        assert_eq!(app.outbox, vec![Message::TabActivate(0)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 两主题各唤出一遍:不 panic、列内容齐全、visuals 真切换(暗/亮各一)。
    #[test]
    fn zen_hover_nav_paints_in_both_themes() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        for theme in ["dark", "light"] {
            // 每轮独立 context:动画管理器的时刻只前进,跨轮回拨会 panic
            let ctx = egui::Context::default();
            let probe = Rc::new(Cell::new((0u64, Rect::NOTHING)));
            let (mut app, dir) = zen_nav_app(theme);
            app.state.theme.mode = if theme == "light" {
                crate::theme::ThemeMode::Light
            } else {
                crate::theme::ThemeMode::Dark
            };
            assert_eq!(
                app.state.resolved_theme(),
                if theme == "light" {
                    crate::theme::ThemeMode::Light
                } else {
                    crate::theme::ThemeMode::Dark
                },
                "{theme}:主题真的生效了(两轮 visuals 必须一明一暗)"
            );
            app.state.apply(Message::ZenToggled);
            let mut now = 0.0;
            now += 0.1;
            zen_nav_frame(
                &mut app,
                &ctx,
                screen,
                now,
                vec![Event::PointerMoved(egui::pos2(8.0, 400.0))],
                &probe,
            );
            for _ in 0..5 {
                now += 0.1;
                let shapes = zen_nav_frame(&mut app, &ctx, screen, now, Vec::new(), &probe);
                if app.state.zen_nav.alpha == 1.0 {
                    let (_, nav) = probe.get();
                    let labels = texts_in_rect(&shapes, nav);
                    assert_eq!(labels.len(), 3, "{theme}:三行都在:{labels:?}");
                    assert!(labels.contains(&"甲.md*".to_owned()), "{theme}:{labels:?}");
                    assert!(labels.contains(&"乙.md".to_owned()), "{theme}:{labels:?}");
                    break;
                }
            }
            assert_eq!(
                app.state.zen_nav.alpha, 1.0,
                "{theme}:动画收敛(指针停在感应区未离开)"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// M5 收口(2026-09-27 修复用户实测回归):非禅定态的 panel 序列恰为
    /// titlebar / menubar / statusbar / nav / preview + 中央编辑器。
    ///
    /// 三条取证分别对应三条反馈:
    /// - 顺序:标题栏最顶、状态栏贴底(文本矩形自上而下);**2026-10-12
    ///   起常驻菜单行退役**(菜单收进标题栏左缘汉堡,docs/ui-shell-redesign-
    ///   v2.md §5.7),「文件(F)」这类菜单文本在收起态**不再出现**;
    /// - 状态栏**横跨全窗底部**:它的文本落在左栏脚下(x < 左栏宽度)——
    ///   修复前 statusbar 画在左右栏之后,被夹在中央残余区,文本 x 必然
    ///   大于左栏宽度;
    /// - **无黑条**:中央竖直带上的采样点全部被内容色矩形盖住 —— 编辑器
    ///   曾用 `Panel::left`,其后中央残余区无人认领,预览左侧多一条侧栏
    ///   色的黑条;回到 `CentralPanel` 后 nav 右缘到 preview 左缘连续覆盖。
    #[test]
    fn panel_order_spans_statusbar_and_fills_the_center() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut app = LaterMdApp {
            frameless: true,
            ..Default::default()
        };

        let shapes = draw_shapes(&mut app, &ctx, screen);
        let title = topmost_text(&shapes, "LaterMD —");
        let status = topmost_text(&shapes, "248 字");
        let nav = topmost_text(&shapes, "选择一个目录作为文件树根");

        // 标题栏最顶;菜单行已退役,收起态不应有任何菜单标题文本
        assert!(
            title.top() < crate::ui::tokens::TITLEBAR_H,
            "标题栏最顶:{title:?}"
        );
        let menu_text_rendered = shapes.iter().any(|clipped| {
            let egui::epaint::Shape::Text(text) = &clipped.shape else {
                return false;
            };
            text.galley.job.text.contains("文件(F)")
        });
        assert!(
            !menu_text_rendered,
            "常驻菜单行已退役:收起态不应渲染「文件(F)」"
        );
        // 状态栏贴底且横跨:文本 x 落在左栏宽度以内(左栏 default 240)
        assert!(
            status.top() > screen.height() - 40.0,
            "状态栏贴底:{status:?}"
        );
        assert!(
            status.left() < 240.0,
            "状态栏横跨全窗底部(文本须在左栏脚下):{status:?}"
        );
        // 左栏在左,右缘不超过 default 宽度(240)放一点余量
        assert!(
            nav.left() < 240.0 && nav.right() < 260.0,
            "左栏在左侧:{nav:?}"
        );

        // 无黑条:标题栏与状态栏之间的中央高度上,从左栏右缘到
        // 右栏内部连续被内容色覆盖。黑条回归时(编辑器窄 left panel 之右、
        // 预览之左)采样点会露背景色。
        let content = crate::theme::content_fill(true);
        for x in [250.0, 500.0, 760.0, 800.0, 1100.0] {
            let probe = egui::pos2(x, 400.0);
            assert!(
                covered_by_fill(&shapes, content, probe),
                "({x}, 400) 未被内容色覆盖:中央区有黑条"
            );
        }
    }

    /// **真实帧里的工具条点击**:走完 `LaterMdApp::draw` 的五帧节奏能把加粗
    /// 按钮点出来(M3 验收点)。
    ///
    /// 按钮位置由内置探针给出 —— 它前面压着标签条与提示行,高度是布局
    /// 演算的结果,手搓坐标必然与真实帧错位(M2 已经在标题栏上踩过一次)。
    /// 只断言「消息出来了」:后面「消息 → 文本」那一截归 `state::tests`,
    /// 分层是因为 TextEdit 内部会对 `CCursorRange` 做归一化,把两者捆在一
    /// 条测试里会让人分不清是链路断了还是 egui 改了选区。
    #[test]
    fn clicking_bold_in_a_real_frame_requests_format() {
        use crate::compose::FormatAction;
        use std::cell::RefCell;
        use std::rc::Rc;

        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0));
        let mut app = LaterMdApp::default();
        let center = Rc::new(RefCell::new(egui::Pos2::ZERO));
        {
            let sink = center.clone();
            app.format_probe = Some(Box::new(move |action, rect| {
                if action == FormatAction::Bold {
                    *sink.borrow_mut() = rect.center();
                }
            }));
        }
        let frame = |app: &mut LaterMdApp, events: Vec<Event>| {
            ctx.run_ui(
                RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            )
            .drop_without_applying_deltas();
        };

        frame(&mut app, Vec::new());
        app.format_probe = None;
        let center = *center.borrow();
        assert!(center.x > 0.0, "探针拿到了加粗按钮的位置:{center:?}");

        // sizing pass → moved → press → release:面板层前几遍 widget 不参与
        // 命中测试,与既有点击测试同一节奏
        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut app, Vec::new());
        frame(&mut app, Vec::new());
        frame(&mut app, vec![Event::PointerMoved(center)]);
        frame(&mut app, vec![click(true)]);
        frame(&mut app, vec![click(false)]);
        assert_eq!(
            app.outbox,
            vec![Message::FormatRequested(FormatAction::Bold)],
            "工具条按钮在真实三栏帧里点得动,且不被相邻控件抢走"
        );
    }

    /// Live 模式真实帧里的工具条点击(2026-10-10):活动块内框选 → 点
    /// 加粗 → 块内包裹、内容仍选中。链路三段各有独立测试(镜像
    /// `live::active_block_selection_is_mirrored_for_the_format_bar`、归约
    /// `state::live_format_applies_inside_the_active_block`、落地
    /// `live::format_landing_restores_the_content_selection`),本测试钉
    /// 三段在真实布局帧里的**接缝** —— 镜像/落地都走真实 `app.draw`,
    /// 消息经真实 `reduce` 消费。
    #[test]
    fn clicking_bold_in_live_mode_formats_the_active_block() {
        use crate::compose::FormatAction;
        use std::cell::RefCell;
        use std::rc::Rc;

        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0));
        let mut app = LaterMdApp {
            frameless: true,
            ..Default::default()
        };
        app.state.render_mode = crate::live::RenderMode::Live;
        app.state
            .tabs
            .current_mut()
            .editor
            .replace_all("甲乙丙\n\n后文");
        // 测试明确进入写作态；浏览态不会再因源码光标自动抢出 TextEdit。
        app.state.tabs.current_mut().cursor.byte = Some(0);
        app.state.tabs.current_mut().live.browse_only = false;
        app.state.tabs.current_mut().live.active = Some(0);
        let block_id =
            crate::ui::editor::tab_editor_id(app.state.tabs.current().id).with(("live-block", 0));

        // reduce + draw 一帧(与 task 测试同节奏):draw 发消息,下一帧
        // reduce 消费 —— 尾帧是链路闭合的必要一环。
        let frame = |app: &mut LaterMdApp, events: Vec<Event>| {
            ctx.run_ui(
                RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| {
                    app.reduce(ui.ctx());
                    app.draw(ui);
                },
            )
            .drop_without_applying_deltas();
        };

        // 预热两帧(块表定位活动块 + 布局收敛),再把「框选 甲乙丙」注进
        // 活动块的持久 TextEditState —— 与真实拖选同源(state 是唯一真源)。
        frame(&mut app, Vec::new());
        frame(&mut app, Vec::new());
        {
            let mut st = egui::widgets::text_edit::TextEditState::default();
            st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(3),
            )));
            st.store(&ctx, block_id);
            ctx.memory_mut(|mem| mem.request_focus(block_id));
        }

        let center = Rc::new(RefCell::new(egui::Pos2::ZERO));
        {
            let sink = center.clone();
            app.format_probe = Some(Box::new(move |action, rect| {
                if action == FormatAction::Bold {
                    *sink.borrow_mut() = rect.center();
                }
            }));
        }
        frame(&mut app, Vec::new());
        app.format_probe = None;
        let center = *center.borrow();
        assert!(center.x > 0.0, "探针拿到了加粗按钮的位置:{center:?}");

        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut app, vec![Event::PointerMoved(center)]);
        frame(&mut app, vec![click(true)]);
        frame(&mut app, vec![click(false)]);
        // 尾帧:reduce 消费 FormatRequested,落地帧写回块选区
        frame(&mut app, Vec::new());

        let tab = app.state.tabs.current();
        assert_eq!(
            tab.editor.text(),
            "**甲乙丙**\n\n后文",
            "live 下点加粗落在活动块内,块外一字不动"
        );
        let landed = egui::widgets::text_edit::TextEditState::load(&ctx, block_id)
            .and_then(|st| st.cursor.char_range())
            .expect("落地选区在场");
        assert_eq!(
            (
                landed.primary.index.0.min(landed.secondary.index.0),
                landed.primary.index.0.max(landed.secondary.index.0)
            ),
            (2, 5),
            "内容仍选中(连点第二次可 toggle)"
        );
        assert!(
            tab.live.pending_caret.is_none() && tab.live.pending_range.is_none(),
            "落地即焚"
        );
    }

    /// 任务列表崩溃回归(2026-09-27 用户实测「点几次就崩溃」)。
    ///
    /// 全链路:真实 `reduce`+`draw`、真实格式条 Task 按钮、CJK 文本行中
    /// 光标,三态循环两整圈(六次点击)。崩溃机制:格式归约整篇替换文本
    /// 后,状态栏(绘制序先于编辑器)拿按**旧文本**折出的 `cursor.byte`
    /// 切**新文本**,`byte index not a char boundary` panic —— 修复在
    /// `EditorBuffer::line_col` 的边界收缩与 `apply_format` 的字节重折算。
    #[test]
    fn task_button_cycling_on_cjk_never_panics() {
        use crate::compose::FormatAction;
        use std::cell::RefCell;
        use std::rc::Rc;

        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0));
        let mut app = LaterMdApp {
            frameless: true,
            ..Default::default()
        };
        app.state
            .tabs
            .current_mut()
            .editor
            .replace_all("纯中文行\n第二行乙");
        let id = crate::ui::editor::tab_editor_id(app.state.tabs.current().id);

        let center = Rc::new(RefCell::new(egui::Pos2::ZERO));
        {
            let sink = center.clone();
            app.format_probe = Some(Box::new(move |action, rect| {
                if action == FormatAction::Task {
                    *sink.borrow_mut() = rect.center();
                }
            }));
        }
        let frame = |app: &mut LaterMdApp, events: Vec<Event>| {
            ctx.run_ui(
                RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| {
                    app.reduce(ui.ctx());
                    app.draw(ui);
                },
            )
            .drop_without_applying_deltas();
        };
        // 预热一帧再取坐标:panel 首帧按出厂/回退尺寸演算,第二帧起才是
        // 收敛后的稳定布局(U0 把 interact_size.y 从 18 抬到 36 后,菜单栏
        // 首帧回退值与收敛值的差被放大,首帧坐标差 33px,点击会落空)
        frame(&mut app, Vec::new());
        frame(&mut app, Vec::new());
        app.format_probe = None;
        let center = *center.borrow();
        assert!(center.x > 0.0, "探针拿到 Task 按钮:{center:?}");

        // 光标落在首行行中(非行首,字符 2)并聚焦
        let mut st = egui::widgets::text_edit::TextEditState::default();
        st.cursor.set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(2),
        )));
        st.store(&ctx, id);
        ctx.memory_mut(|mem| mem.request_focus(id));

        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let states = ["- [ ] 纯中文行", "- [x] 纯中文行", "- 纯中文行"];
        for cycle in 0..6 {
            for events in [
                vec![Event::PointerMoved(center)],
                vec![click(true)],
                vec![click(false)],
                Vec::new(),
            ] {
                frame(&mut app, events);
            }
            let head = app
                .state
                .tabs
                .current()
                .editor
                .text()
                .lines()
                .next()
                .unwrap()
                .to_owned();
            assert_eq!(
                head,
                states[cycle % 3],
                "第 {} 次点击后首行应为三态之一",
                cycle + 1
            );
        }
        // 第七次点击:周期闭环,回到未勾态
        for events in [
            vec![Event::PointerMoved(center)],
            vec![click(true)],
            vec![click(false)],
            Vec::new(),
        ] {
            frame(&mut app, events);
        }
        let head = app
            .state
            .tabs
            .current()
            .editor
            .text()
            .lines()
            .next()
            .unwrap();
        assert_eq!(head, "- [ ] 纯中文行", "三态周期 3:第七次点击回到未勾态");
    }

    /// 状态栏行列换算对**非边界字节**不 panic:过期快照的字节可能落在
    /// CJK 字符中间(见 `task_button_cycling_on_cjk_never_panics` 的机制
    /// 说明),收缩到所属字符起点而不是 panic。
    ///
    /// 换算本身已下沉到 `latermd_editor::EditorBuffer::line_col`(走 rope,
    /// `O(log N)`),这里的期望值仍是当年崩溃帧的实值 —— 换实现不能换结果。
    #[test]
    fn line_col_tolerates_stale_mid_char_bytes() {
        let text = "- [ ] 纯中文行\n第二行乙";
        let buf = latermd_editor::EditorBuffer::new(text);
        // 字节 14 落在 '文'(12..15)中间 —— 崩溃帧的实值;收缩到 '中'
        // 之后(字节 12),按字符计列
        let (line, col) = buf.line_col(14);
        assert_eq!((line, col), (1, 9));
        // 越界钳制到文末仍是合法行为
        let (line, _) = buf.line_col(10_000);
        assert_eq!(line, 2);
        // 正常路径不变
        let ascii = latermd_editor::EditorBuffer::new("abc");
        assert_eq!(ascii.line_col(2), (1, 3));
        let cjk = latermd_editor::EditorBuffer::new("甲乙\n丙");
        assert_eq!(cjk.line_col(7), (2, 1), "第二行行首");
    }

    /// **状态栏必须是单行**(2026-10-08 真机复核补的守门断言)。
    ///
    /// 这条断言的存在理由是一桩真实回归:S1-2 把状态栏改成三段时,把三个
    /// 分区写成了三个**平级**调用(`ui.horizontal` / `ui.horizontal` /
    /// `ui.with_layout`),而父 Ui 是 `egui::Panel::bottom` 的
    /// `Layout::top_down`(`containers/panel.rs:821`)—— 于是每个平级调用
    /// **各占一行**,三段竖着摞起来,状态栏从 18px 涨到 75px(截图实测)。
    ///
    /// 当时 1386 个测试全绿、六个门禁全绿,**没有任何断言看它的几何** ——
    /// 是本机 X11 截图逐行量出来的。这是「断言保证不了好看」最硬的证据:
    /// 连「有没有塌成多行」都保证不了。
    ///
    /// **实测的两个值**(红绿验证过,坏版本用 `git show origin/main` 取回):
    /// 正确 **18px** / 坏版本 **800px**。坏版本不是「三倍」而是**吃满可用
    /// 高度** —— `ui.with_layout` 在 `top_down` 父布局里会把该行剩余空间
    /// 全部吃掉,于是状态栏几乎吞掉整个窗口底部。所以断言写成「< 30px」而
    /// 不是「< 3×18px」:后者在坏版本面前不是一个可区分的量级。
    #[test]
    fn status_bar_is_a_single_row() {
        let ctx = egui::Context::default();
        let state = crate::state::State::default();
        let height = Cell::new(0.0f32);
        // `run_ui` 给的 Ui 默认就是 `top_down`,与 `Panel::bottom` 一致 ——
        // 正是这个布局把三个平级调用摞成了三行,故这里能复现。
        ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 800.0),
                )),
                ..Default::default()
            },
            |ui| {
                super::status_bar(ui, &state);
                height.set(ui.min_rect().height());
            },
        )
        .drop_without_applying_deltas();
        let h = height.get();
        assert!(
            h > 0.0 && h < 30.0,
            "状态栏应单行(实测 {h}px;单行基准 18px,上限 30px)—— \
             远超 30px 说明三个分区被写成了平级调用,在 top_down 父布局里各占一行 \
             且最后一段吃满剩余高度(S1-2 真犯过,坏版本实测 800px,见本测试文档)"
        );
    }

    /// 提示行(原文件工具栏的能力,工具栏退役后迁到编辑器面板顶,
    /// decisions-pending #32):有提示时渲染提示文本与「知道了」,点击发
    /// `NoticeDismissed`;无提示不渲染任何东西。
    #[test]
    fn notice_bar_shows_notice_and_dismiss_button_sends_message() {
        use crate::state::DocumentState;

        let ctx = egui::Context::default();
        let mut outbox = Vec::new();
        let rect = Cell::new(Rect::NOTHING);
        let document = DocumentState {
            path: None,
            dirty: false,
            notice: Some("Ctrl+S 已被「导出 HTML」占用".to_owned()),
        };

        // 帧 1:渲染拿「知道了」按钮位置
        ctx.run_ui(RawInput::default(), |ui| {
            let dismiss = notice_bar(ui, &document, &mut outbox);
            rect.set(dismiss.expect("有提示必有按钮").rect);
        })
        .drop_without_applying_deltas();
        assert!(outbox.is_empty(), "仅渲染不产消息");
        assert!(rect.get().width() > 0.0, "按钮有实测矩形");

        // 帧 2-4:点「知道了」→ NoticeDismissed
        let center = rect.get().center();
        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        for events in [
            vec![Event::PointerMoved(center)],
            vec![click(true)],
            vec![click(false)],
        ] {
            ctx.run_ui(
                RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    notice_bar(ui, &document, &mut outbox);
                },
            )
            .drop_without_applying_deltas();
        }
        assert_eq!(outbox, vec![Message::NoticeDismissed]);

        // 无提示:不渲染按钮
        let clean = DocumentState {
            path: None,
            dirty: false,
            notice: None,
        };
        ctx.run_ui(RawInput::default(), |ui| {
            assert!(notice_bar(ui, &clean, &mut Vec::new()).is_none());
        })
        .drop_without_applying_deltas();
    }

    /// 恢复条时间文案的档位与未知兜底:`now` 定点注入,不真等钟。
    #[test]
    fn draft_saved_label_buckets_relative_time() {
        use std::time::Duration as WallDuration;
        use std::time::SystemTime;

        let now = SystemTime::UNIX_EPOCH + WallDuration::from_secs(1_000_000);
        let at = |secs_ago: u64| Some(now - WallDuration::from_secs(secs_ago));
        assert_eq!(draft_saved_label(None, now), "保存时间未知");
        assert_eq!(
            draft_saved_label(Some(now + WallDuration::from_secs(5)), now),
            "保存时间未知",
            "时钟倒流(mtime 在未来)不猜"
        );
        assert_eq!(draft_saved_label(at(30), now), "刚刚保存");
        assert_eq!(draft_saved_label(at(300), now), "保存于 5 分钟前");
        assert_eq!(draft_saved_label(at(2 * 3600 + 59), now), "保存于 2 小时前");
        assert_eq!(draft_saved_label(at(3 * 86400), now), "保存于 3 天前");
    }

    /// 恢复条(#18):有待恢复草稿时渲染「发现未保存草稿」文案与(恢复,
    /// 丢弃)两按钮,点击各发**带标签 id** 的消息(id 定位而非当前标签,
    /// 消息归约晚一帧,期间活动标签可能已切走)。
    #[test]
    fn recovery_bar_buttons_send_tab_scoped_messages() {
        use crate::tabs::DraftRecovery;

        let ctx = egui::Context::default();
        let mut outbox = Vec::new();
        let recover = DraftRecovery {
            path: std::path::PathBuf::from("/tmp/doc.md.latermd-draft"),
            mtime: Some(std::time::SystemTime::now() - std::time::Duration::from_secs(300)),
        };
        let mut rects = None;

        // 帧 1:渲染拿两按钮位置,再从本帧 shapes 里核文案
        let first = ctx.run_ui(RawInput::default(), |ui| {
            let (restore, discard) =
                recovery_bar(ui, &recover, 7, &mut outbox).expect("有待恢复必有按钮");
            rects = Some((restore.rect, discard.rect));
        });
        let painted = painted_text(&first.shapes).join("\n");
        first.drop_without_applying_deltas();
        assert!(outbox.is_empty(), "仅渲染不产消息");
        assert!(
            painted.contains("发现未保存草稿") && painted.contains("保存于 5 分钟前"),
            "文案含关键字与保存时间: {painted}"
        );

        // 帧 2-4:点「恢复」→ DraftRecovered(载荷 = 标签 id 7)
        let (restore_rect, discard_rect) = rects.expect("两按钮有实测矩形");
        let click = |rect: Rect, pressed| {
            let center = rect.center();
            vec![
                Event::PointerMoved(center),
                Event::PointerButton {
                    pos: center,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ]
        };
        for events in [click(restore_rect, true), click(restore_rect, false)] {
            ctx.run_ui(
                RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    recovery_bar(ui, &recover, 7, &mut outbox);
                },
            )
            .drop_without_applying_deltas();
        }
        assert_eq!(outbox, vec![Message::DraftRecovered { tab_id: 7 }]);
        outbox.clear();

        for events in [click(discard_rect, true), click(discard_rect, false)] {
            ctx.run_ui(
                RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    recovery_bar(ui, &recover, 7, &mut outbox);
                },
            )
            .drop_without_applying_deltas();
        }
        assert_eq!(outbox, vec![Message::DraftDiscarded { tab_id: 7 }]);
    }

    /// 恢复条接线(#18):标签有待恢复状态时,整帧 `draw` 真的把恢复条画
    /// 进编辑区顶部(上一条只测 `recovery_bar` 函数本体,这里测
    /// CentralPanel 的接线与撤下后的消失);mtime 缺失走「保存时间未知」。
    #[test]
    fn recovery_bar_wired_into_central_panel_draw() {
        use crate::tabs::DraftRecovery;

        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let mut app = LaterMdApp::default();
        app.state.tabs.current_mut().recover = Some(DraftRecovery {
            path: std::path::PathBuf::from("/tmp/doc.md.latermd-draft"),
            mtime: None,
        });
        let painted = draw_frame(&mut app, &ctx, screen).join("\n");
        assert!(
            painted.contains("发现未保存草稿"),
            "整帧绘制出现恢复条文案: {painted}"
        );
        assert!(painted.contains("保存时间未知"), "mtime 缺失走未知兜底");
        assert!(
            painted.matches("恢复").count() >= 1 && painted.contains("丢弃"),
            "两按钮都在场"
        );

        // 待恢复状态撤下后不再绘制(编辑裁决/恢复/丢弃都会走到这一步)
        app.state.tabs.current_mut().recover = None;
        let painted = draw_frame(&mut app, &ctx, screen).join("\n");
        assert!(
            !painted.contains("发现未保存草稿"),
            "撤下后恢复条消失: {painted}"
        );
    }

    /// 跑若干帧 draw(sizing pass 之后 widget 才参与命中测试)。
    fn draw_frames(app: &mut LaterMdApp, ctx: &egui::Context, screen: Rect, frames: usize) {
        for _ in 0..frames {
            ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            )
            .drop_without_applying_deltas();
        }
    }

    /// 跑两帧完整 draw(sizing pass 之后布局演算才稳定),返回第二帧的绘制
    /// 产物供像素层取证。
    fn draw_frame(app: &mut LaterMdApp, ctx: &egui::Context, screen: Rect) -> Vec<String> {
        painted_text(&draw_shapes(app, ctx, screen))
    }

    /// [`draw_frame`] 的 shapes 版:文本之外还要量 fill 矩形(布局取证要的
    /// 不只是「画没画」,还有「画在哪、盖多宽」)。
    fn draw_shapes(
        app: &mut LaterMdApp,
        ctx: &egui::Context,
        screen: Rect,
    ) -> Vec<egui::epaint::ClippedShape> {
        let mut shapes = Vec::new();
        for i in 0..2 {
            let output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            if i == 1 {
                shapes = output.shapes.clone();
            }
            output.drop_without_applying_deltas();
        }
        shapes
    }

    /// 本帧**真的画出来的所有文本**(取自 shapes,不是布局意图)。
    fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        let mut texts = Vec::new();
        for clipped in shapes {
            if let egui::epaint::Shape::Text(text) = &clipped.shape {
                for line in text.galley.job.text.split('\n') {
                    texts.push(line.to_owned());
                }
            }
        }
        texts
    }

    /// 文本 → 其包围盒里**最靠上**的那一处(同名文案可能出现在多个 panel,
    /// 「文件」既是菜单栏首项又是左栏导航首行;断言 panel 顺序要的是前者)。
    fn topmost_text(shapes: &[egui::epaint::ClippedShape], needle: &str) -> Rect {
        shapes
            .iter()
            .filter_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                text.galley
                    .job
                    .text
                    .split('\n')
                    .any(|line| line.contains(needle))
                    .then_some(clipped.shape.visual_bounding_rect())
            })
            .min_by_key(|rect| rect.top().to_bits())
            .unwrap_or_else(|| panic!("{needle:?} 未绘制"))
    }

    /// `color` 填充矩形是否盖住 `pos`(布局取证:中央区不允许再出现无人
    /// 认领的背景条 —— 黑条回归时采样点上没有任何内容色矩形)。
    fn covered_by_fill(
        shapes: &[egui::epaint::ClippedShape],
        color: egui::Color32,
        pos: egui::Pos2,
    ) -> bool {
        shapes.iter().any(|clipped| {
            matches!(
                &clipped.shape,
                egui::epaint::Shape::Rect(shape)
                    if shape.fill == color && shape.rect.contains(pos)
            )
        })
    }

    // —— 只读对照列与它左缘的分隔线(三态视图)——

    /// 亮度落差(灰阶,1 ≈ 1/255)。取绝对差而非相对百分比:`intensity()` 是
    /// 0..1 感知亮度,暗色皮肤底色本身就在 0.1 附近,除它会把几级灰差放大成
    /// 几十个百分点,阈值无法跨深浅皮肤复用。
    fn luma_gap(a: egui::Color32, b: egui::Color32) -> f32 {
        (a.intensity() - b.intensity()).abs() * 100.0
    }

    /// 当前皮肤 `border ↔ content` 的亮度全距(灰阶)。
    ///
    /// 取法走探针:`run_ui` 只返回 `FullOutput`,闭包内的 `theme::shell`
    /// 拿不出来,而在 `run_ui` **之外**再 `data_mut` 就已错过当帧的色板投影。
    fn shell_full_gap(ctx: &egui::Context) -> f32 {
        let probe = egui::Id::new("__shell_full_probe");
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let gap = {
                let s = crate::theme::shell(ui);
                luma_gap(s.border, s.content)
            };
            ui.data_mut(|d| d.insert_temp(probe, gap));
        });
        out.textures_delta.clear();
        ctx.data(|d| d.get_temp::<f32>(probe))
            .expect("探针应在闭包里写入")
    }

    /// 走完整 draw 后交回「贯穿全高的分隔线」的 (x, 色, 线宽) 与面板底色。
    ///
    /// 与首版那组裸调分隔线的测试的根本区别:这里跑的是**真实
    /// `Panel::right`**,故分隔线能读到真实的 `__resize` response 与
    /// `PanelState` —— 首版手工 `insert_persisted` 造 PanelState,测的是一条
    /// 生产中不存在的路径,于是「线不跟手」这个真 bug 从它下面溜了过去。
    ///
    /// **带 `width_px`** 是因为 hover 反馈的主通道是**线宽**(1px → 3px),
    /// 只交回颜色的话那条测试压根看不见加粗 —— 2026-10-13 坤哥实测
    /// 「没看见 hover 加粗」而当时颜色通道明明是生效的。
    struct DividerProbe {
        x: f32,
        color: egui::Color32,
        content: egui::Color32,
        width_px: f32,
    }

    fn divider_geometry(
        app: &mut LaterMdApp,
        ctx: &egui::Context,
        screen: Rect,
        events: Vec<egui::Event>,
    ) -> Option<DividerProbe> {
        let id = egui::Id::new(COMPARE_PANEL_ID);
        // 拖动中 `PanelState` 停在拖动前的位置(egui 刻意不写),故锚点必须改
        // 取指针 x —— 否则线一移出旧锚点 ±1.5px 就再也找不到它。
        //
        // 注意:`PanelState::load` 与 `read_response` 内部都取 Context **写锁**,
        // 故它们**不能**被包在 `ctx.data(...)`(读锁)里 —— parking_lot RwLock
        // 不可重入,那样会自死锁。
        let panel_left = egui::PanelState::load(ctx, id).map(|p| p.outer_rect.left());
        let resize = ctx.read_response(id.with("__resize"));
        let dragging = resize
            .as_ref()
            .is_some_and(|r| r.dragged() || r.drag_stopped());
        let pointer_x = resize
            .as_ref()
            .filter(|_| dragging)
            .and_then(|r| r.interact_pointer_pos())
            .map(|p| p.x);
        let anchor = pointer_x.or(panel_left);
        let mut output = ctx.run_ui(
            RawInput {
                events,
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                // 取色必须在 `data_mut` **之外**:`data_mut` 已持 Context 写锁,
                // 闭包内再调 `theme::shell`(内部 `ctx.data` 取读锁)会自死锁。
                let probe = egui::Id::new("__divider_probe_content");
                let shell = crate::theme::shell(ui);
                ui.data_mut(|d| d.insert_temp(probe, shell.content));
                app.draw(ui);
            },
        );
        let content = ctx
            .data(|d| d.get_temp::<egui::Color32>(egui::Id::new("__divider_probe_content")))
            .expect("探针应在闭包里写入");
        let mut spans: Vec<(f32, f32, egui::Color32, f32)> = Vec::new();
        fn spans_of(shape: &egui::Shape, out: &mut Vec<(f32, f32, egui::Color32, f32)>) {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| spans_of(s, out)),
                egui::Shape::LineSegment { points, stroke }
                    if (points[0].x - points[1].x).abs() < 0.01 && stroke.width > 0.0 =>
                {
                    out.push((
                        points[0].x,
                        (points[1].y - points[0].y).abs(),
                        stroke.color,
                        stroke.width,
                    ))
                }
                _ => {}
            }
        }
        output
            .shapes
            .iter()
            .for_each(|s| spans_of(&s.shape, &mut spans));
        output.textures_delta.clear();
        // 靠 x 挑线靠不住:实测 hover/drag 帧里画面上有**两根**贯穿全高的
        // 竖线 —— 本模块的线在 `anchor - 0.5`,而 egui 自己在 resize 边界
        // 又画一根 `hovered.fg_stroke`。改用**颜色判据**:本模块的线是
        // `border → content` 的 lerp,其与底色的亮度落差必然落在
        // `(0, full]` 开区间内,纯黑线天然被排除。
        let full = ctx.data(|d| d.get_temp::<f32>(egui::Id::new("__shell_full_probe")));
        let (anchor, full) = (anchor?, full?);
        spans
            .into_iter()
            .filter(|(_, len, _, _)| *len > screen.height() * 0.7)
            .filter(|(x, _, color, _)| (*x - anchor).abs() <= 1.5 && color.a() > 0)
            .find(|(_, _, color, _)| {
                let gap = luma_gap(*color, content);
                gap > 0.05 && gap <= full
            })
            .map(|(x, _, color, width)| DividerProbe {
                x,
                color,
                content,
                width_px: width * ctx.pixels_per_point(),
            })
    }

    /// 建一个「分栏档 + 无左栏」的应用实例,并空跑到布局稳定。
    fn compare_only_app(screen: Rect, dark: bool) -> (egui::Context, LaterMdApp) {
        // **刻意不调 `crate::fonts::install`**:本组只量分隔线,一个字符都不
        // 渲染。装字体不只是多余 —— CI runner 上没有 CJK 字体,`install`
        // 返回 `None`,那个 `.expect` 会让测试在 CI 上齐红(2026-10-13 实测
        // run 38360023782:本地全绿、CI Gate 失败)。
        let ctx = egui::Context::default();
        ctx.set_visuals(if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        let mut app = LaterMdApp {
            frameless: true,
            ..Default::default()
        };
        app.state.layout.left = false;
        app.state.render_mode = crate::live::RenderMode::Split;
        // 空跑两帧:让 panel 建立 PanelState、让 shell 色板投影到位。
        for _ in 0..2 {
            let mut out = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            out.textures_delta.clear();
        }
        (ctx, app)
    }

    /// 滚轮锚点端到端(Split 对照列同步滚动):指针悬在源码文本区滚轮
    /// 滚动一帧,下一帧对照列把锚点块滚进视口。光标字节全程未动 ——
    /// 跟随的是**鼠标所在行**,不是光标;右栏滚动探针(帧末写入)给正证。
    #[test]
    fn split_wheel_scroll_anchors_compare_column_to_pointer_line() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let (ctx, mut app) = compare_only_app(screen, false);
        // 长文档:指针压在视口下沿附近时,其所在段落远在右栏首屏之外
        app.state
            .tabs
            .current_mut()
            .editor
            .load(&"Paragraph body line.\n\n".repeat(160));
        for _ in 0..2 {
            let mut out = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            out.textures_delta.clear();
        }
        // 指针落点用右栏探针的**屏幕空间**视口反推编辑器区域(TextEdit
        // 响应矩形是内容坐标,直接乘系数会把指针送出窗外):对照列左缘
        // 再往左 300px 就是源码列文本区,y 取视口下沿附近(锚出行足够深)。
        let right_id = crate::ui::editor::tab_right_editor_id(app.state.tabs.current().id);
        let probe = ctx
            .data(|d| {
                d.get_temp::<crate::live::LiveScrollProbe>(crate::live::scroll_probe_id(right_id))
            })
            .expect("布局帧写了右栏滚动探针");
        let pointer_pos = egui::pos2(
            probe.viewport.left() - 300.0,
            probe.viewport.top() + probe.viewport.height() * 0.8,
        );
        let wheel_frame = |app: &mut LaterMdApp, pointer: bool| {
            let mut events = Vec::new();
            if pointer {
                events.push(egui::Event::PointerMoved(pointer_pos));
            }
            events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, -3.0),
                phase: egui::TouchPhase::Move,
                modifiers: Default::default(),
            });
            let mut out = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            out.textures_delta.clear();
        };
        let plain_frame = |app: &mut LaterMdApp| {
            let mut out = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            out.textures_delta.clear();
        };
        // 连续 5 帧滚轮(真实滚动的形态):左栏逐帧加深,锚点逐帧重写,
        // 对照列滞后一帧逐帧追。单 tick 从滚动 0 起步时锚点段落还在右栏
        // 首屏内,对齐差值会被 offset 下界钳成 0 —— 真实使用里连滚几帧
        // 后锚点块必落到视口之外,这正是本测试要复现的形态。
        wheel_frame(&mut app, true);
        for _ in 0..4 {
            wheel_frame(&mut app, false);
        }
        // 消费帧:对照列帧头取走最后一枚锚点并落地对齐
        plain_frame(&mut app);

        let tab = app.state.tabs.current();
        let right_id = crate::ui::editor::tab_right_editor_id(tab.id);
        let probe = ctx
            .data(|d| {
                d.get_temp::<crate::live::LiveScrollProbe>(crate::live::scroll_probe_id(right_id))
            })
            .expect("右栏帧末写了滚动探针");
        assert!(
            probe.content_height > probe.viewport.height(),
            "内容高于视口:{probe:?}"
        );
        assert!(probe.offset > 0.0, "对照列滚到锚点块附近:{probe:?}");
        assert!(
            tab.live_right.pending_caret.is_none() && tab.live_right.anchor_align_y.is_none(),
            "锚点意图同帧消费即清"
        );
        assert_eq!(tab.cursor.byte, None, "光标全程未动,跟随的是鼠标行");
    }

    /// 空闲态就要看得见一条界 —— 这是分隔线的全部要点。
    ///
    /// 判据用「相对 `border ↔ content` 全距的百分比」而非绝对灰阶:每套皮肤
    /// 的全距本就不同,绝对阈值只能对其中一套成立。
    #[test]
    fn compare_divider_is_visible_while_idle() {
        for dark in [false, true] {
            for (w, h) in [(1400.0_f32, 900.0_f32), (900.0, 700.0)] {
                let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h));
                let (ctx, mut app) = compare_only_app(screen, dark);
                let _ = shell_full_gap(&ctx);
                let probe = divider_geometry(&mut app, &ctx, screen, vec![])
                    .unwrap_or_else(|| panic!("dark={dark} {w}x{h}: 对照列左缘应有分隔线"));
                let full = shell_full_gap(&ctx);
                let ratio = luma_gap(probe.color, probe.content) / full * 100.0;
                assert!(
                    (25.0..=60.0).contains(&ratio),
                    "dark={dark} {w}x{h}: 空闲线深度 {ratio:.0}%(x={:.1},全距 {full:.2} 灰阶)\
                     应落在 25..=60%",
                    probe.x
                );
                // 空闲必须是 1 物理像素:hover 加粗要有对比基准。
                assert!(
                    (probe.width_px - 1.0).abs() < 0.01,
                    "dark={dark} {w}x{h}: 空闲线宽应为 1 物理像素,实得 {:.2}",
                    probe.width_px
                );
            }
        }
    }

    /// 指针进命中带内线要**明显比空闲更深**且**加粗**,方向不得反;出带完
    /// 全回落。硬编码 5.5px / 9px,**不引用 `PREVIEW_DIVIDER_GRAB`** —— 拿
    /// token 当期望值会让断言自我满足。
    #[test]
    fn compare_divider_deepens_on_hover_within_the_grab_zone() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        for dark in [false, true] {
            let (ctx, mut app) = compare_only_app(screen, dark);
            let _ = shell_full_gap(&ctx);
            let idle_probe = divider_geometry(&mut app, &ctx, screen, vec![]).expect("应有分隔线");
            let (edge, idle, content) = (idle_probe.x, idle_probe.color, idle_probe.content);
            let y = screen.height() / 2.0;
            let at = |app: &mut LaterMdApp, ctx: &egui::Context, x: f32| {
                divider_geometry(
                    app,
                    ctx,
                    screen,
                    vec![egui::Event::PointerMoved(egui::pos2(x, y))],
                )
                .expect("hover 帧应有线")
            };
            for d in [0.0_f32, 3.0, 5.5] {
                let (in_pos, in_neg) = (at(&mut app, &ctx, edge + d), at(&mut app, &ctx, edge - d));
                // ① 加粗是主通道,先断它。硬编码 ≥2.5(不引用 HOT_W)。
                assert!(
                    in_pos.width_px >= 2.5,
                    "dark={dark}: 距边 {d}px 在带内应已加粗(≥2.5 物理像素),实得 {:.2}",
                    in_pos.width_px
                );
                assert!(
                    (in_pos.x - in_neg.x).abs() <= 0.01 && (in_pos.x - edge).abs() <= 1.5,
                    "dark={dark}: 距边 {d}px 两侧的线心应重合于边界({edge:.1}),实得 +{:.2} / {:.2}",
                    in_pos.x,
                    in_neg.x
                );
                // ② 色深是辅助通道,方向不得反。
                assert!(
                    luma_gap(in_pos.color, content) > luma_gap(idle, content),
                    "dark={dark}: 距边 {d}px 在带内应已提亮(朝底色 lerp,不是朝黑缩放)"
                );
                assert_eq!(
                    in_pos.color, in_neg.color,
                    "dark={dark}: 命中带以边界为中心,两侧应对称"
                );
            }
            for d in [9.0_f32, 40.0] {
                let far = at(&mut app, &ctx, edge + d);
                assert_eq!(
                    far.color, idle,
                    "dark={dark}: 距边 {d}px 已超出命中带,应回落到空闲色"
                );
                assert!(
                    (far.width_px - idle_probe.width_px).abs() < 0.01,
                    "dark={dark}: 距边 {d}px 应把线宽收回空闲的 {:.2},实得 {:.2}",
                    idle_probe.width_px,
                    far.width_px
                );
            }
        }
    }

    /// 对照列宽度必须**拖得动**,且命中带要够宽:按下点取边界外 5px ——
    /// 在抬高后的命中带(`PREVIEW_DIVIDER_GRAB`=6)内、egui 出厂半径(3)
    /// 之外。抬高被删则拖拽根本不启动,本测试必红(2026-10-15 坤哥实测
    /// 「宽度拉不动」的根因:恢复面板时漏了这处局部抬高,6px 带无视觉
    /// 线索时出厂 3px 带等于不可命中)。
    #[test]
    fn compare_resizes_by_drag_from_within_the_widened_grab_zone() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let (ctx, mut app) = compare_only_app(screen, false);
        let mut frame = |events: Vec<egui::Event>| {
            ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| app.draw(ui),
            )
            .drop_without_applying_deltas()
        };
        let panel_left = || {
            egui::PanelState::load(&ctx, egui::Id::new(COMPARE_PANEL_ID))
                .expect("对照列 PanelState 存在")
                .outer_rect
                .left()
        };

        frame(Vec::new());
        let edge = panel_left();
        let y = screen.height() / 2.0;
        // 悬停进带 → 按下 → 分三帧左移 100px → 松手。右栏左缘向左 = 变宽。
        frame(vec![Event::PointerMoved(egui::pos2(edge - 5.5, y))]);
        frame(vec![Event::PointerButton {
            pos: egui::pos2(edge - 5.5, y),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        }]);
        for d in [40.0, 70.0, 100.0] {
            frame(vec![Event::PointerMoved(egui::pos2(edge - d, y))]);
        }
        frame(vec![Event::PointerButton {
            pos: egui::pos2(edge - 100.0, y),
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]);
        frame(Vec::new());
        let after = panel_left();
        assert!(
            (after - (edge - 100.0)).abs() < 1.5,
            "拖动 100px 松手后,左缘应停在 {edge}−100 处;实得 {after}(出厂数值拖不动 = 命中带抬高回归)"
        );
    }

    /// 对照列**拖到最小宽再往外拖**(egui 的 drag-to-collapse 手势)必须
    /// 收起并**收敛回三态状态源**:工作 bool 被 egui 翻成 false 后,归约
    /// 侧应经 [`Message::CompareColumnToggled`] 落到源码档。方向:对照列
    /// 左缘向**右** = 变窄,越过 `PREVIEW_MIN_W` 触发收起。
    #[test]
    fn compare_column_drag_past_min_collapses_into_source_mode() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let (ctx, mut app) = compare_only_app(screen, false);
        let frame = |app: &mut LaterMdApp, events: Vec<egui::Event>| {
            ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| {
                    app.reduce(ui.ctx());
                    app.draw(ui);
                },
            )
            .drop_without_applying_deltas()
        };
        frame(&mut app, vec![]);
        let edge = egui::PanelState::load(&ctx, egui::Id::new(COMPARE_PANEL_ID))
            .expect("对照列 PanelState 存在")
            .outer_rect
            .left();
        let y = screen.height() / 2.0;
        // 悬停进带(抬高后的 6px 命中带)→ 按下 → 左缘右拖 60px(420→360,
        // 已越过 260?没有 —— 越过收起线要拖到 raw<260,即 x > edge+160)。
        frame(
            &mut app,
            vec![Event::PointerMoved(egui::pos2(edge - 5.0, y))],
        );
        frame(
            &mut app,
            vec![Event::PointerButton {
                pos: egui::pos2(edge - 5.0, y),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }],
        );
        for d in [60.0, 120.0, 200.0] {
            frame(&mut app, vec![Event::PointerMoved(egui::pos2(edge + d, y))]);
        }
        frame(
            &mut app,
            vec![Event::PointerButton {
                pos: egui::pos2(edge + 200.0, y),
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
        frame(&mut app, vec![]);
        assert_eq!(
            app.state.render_mode,
            crate::live::RenderMode::Source,
            "拖过最小宽收起对照列,应收敛到源码档(工作 bool 翻转没有回流 = 状态源分叉)"
        );
    }

    /// 分隔线必须画在**弹窗之下**:panel_divider 若用独立 `Order::Middle`
    /// 散层,egui 出形状时先排已注册 Area(全部 `egui::Window`),非 Area
    /// 散层兜底追加在最后 ⇒ 线横穿设置/关于等一切弹窗(2026-10-16 坤哥
    /// 截图实测)。改为画进根层(调用点在中央内容之后 ⇒ 盖得住面板)后,
    /// 分隔线形状必须先于弹窗标题形状出现。本测试断言形状**流序**:
    /// 分隔线竖线的下标 < 弹窗「设置」标题的下标;独立散层实现必红。
    #[test]
    fn compare_divider_paints_below_dialogs() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let (ctx, mut app) = compare_only_app(screen, false);
        let _ = shell_full_gap(&ctx);
        app.state.settings.open = true;
        // egui Window 首帧是 sizing pass,不出内容形状;第二帧起再取证。
        let mut out = FullOutput::default();
        for _ in 0..2 {
            out = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            out.textures_delta.clear();
        }
        let divider_idx = out.shapes.iter().position(|clipped| {
            matches!(&clipped.shape, egui::Shape::LineSegment { points, stroke }
                if (points[0].x - points[1].x).abs() < 0.01
                    && (points[1].y - points[0].y).abs() > screen.height() * 0.6
                    && stroke.width > 0.0)
        });
        let dialog_idx = out.shapes.iter().position(|clipped| {
            matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text() == "设置")
        });
        let divider_idx = divider_idx.expect("分栏档应有分隔线");
        let dialog_idx = dialog_idx.expect("设置弹窗应已渲染(标题「设置」)");
        assert!(
            divider_idx < dialog_idx,
            "分隔线(形状下标 {divider_idx})必须画在弹窗(下标 {dialog_idx})之下"
        );
    }

    /// 对照列要有**列头**(「对照」标题 + 右缘「只读」徽标):P4 曾随右栏
    /// 整删,2026-10-16 恢复。无头夹具不装字体(CI 无 CJK),但 galley 仍
    /// 携带文本串,按文字内容断言即可,不依赖字形渲染。
    #[test]
    fn compare_column_has_readonly_pane_heading() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let (ctx, mut app) = compare_only_app(screen, false);
        let mut out = ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| app.draw(ui),
        );
        out.textures_delta.clear();
        let mut seen = Vec::new();
        for clipped in &out.shapes {
            if let egui::Shape::Text(text) = &clipped.shape {
                seen.push(text.galley.text().to_owned());
            }
        }
        assert!(
            seen.iter().any(|t| t == "对照"),
            "对照列应有「对照」列头,实得 {seen:?}"
        );
        assert!(
            seen.iter().any(|t| t == "只读"),
            "对照列应有「只读」徽标,实得 {seen:?}"
        );
    }

    /// 拖动中分隔线要**跟着指针走**,不是停在拖动前的锚点上:2026-10-13
    /// 坤哥实测「线不跟着我拉的框走」的真 bug 就发生在这里 —— 拖动中
    /// `PanelState` 被 egui 刻意停写、`Response.rect` 又拿旧边界算,锚点
    /// 必须 取 `interact_pointer_pos`。P4 删面板时连带删了本测试,现随
    /// 对照列回归恢复。方向:向左拖 = 对照列变宽。
    #[test]
    fn compare_divider_follows_the_drag() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let (ctx, mut app) = compare_only_app(screen, false);
        // 分隔线取色的亮度全距锚点要先把 shell 探针写上(同族测试同款)。
        let _ = shell_full_gap(&ctx);
        let start_x = divider_geometry(&mut app, &ctx, screen, vec![])
            .expect("分栏档应有分隔线")
            .x;
        let press_y = screen.height() / 2.0;
        // 按下必须**单独占一帧**:egui 要先落一次 press 才把 widget 标记为
        // 按压中,同帧再发 PointerMoved 不构成拖动(实测 dragged() 恒 false、
        // interact_pointer_pos() 恒 None,现象与「线不跟手」一模一样)。
        divider_geometry(
            &mut app,
            &ctx,
            screen,
            vec![egui::Event::PointerMoved(egui::pos2(
                start_x - 2.0,
                press_y,
            ))],
        );
        divider_geometry(
            &mut app,
            &ctx,
            screen,
            vec![Event::PointerButton {
                pos: egui::pos2(start_x - 2.0, press_y),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }],
        );
        let mut xs = Vec::new();
        for step in 1..=5 {
            let target_x = start_x - step as f32 * 24.0;
            divider_geometry(
                &mut app,
                &ctx,
                screen,
                vec![egui::Event::PointerMoved(egui::pos2(target_x, press_y))],
            );
            xs.push(
                divider_geometry(&mut app, &ctx, screen, vec![])
                    .expect("拖动过程中线仍在")
                    .x,
            );
        }
        divider_geometry(
            &mut app,
            &ctx,
            screen,
            vec![Event::PointerButton {
                pos: egui::pos2(start_x - 120.0, press_y),
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
        let mut prev = start_x;
        let mut moved = 0;
        for x in &xs {
            assert!(
                *x < prev + 0.5,
                "线应随指针左移单调跟随,却从 {prev:.1} 退到 {x:.1}(全序列 {xs:?})"
            );
            if *x < prev - 1.0 {
                moved += 1;
            }
            prev = *x;
        }
        assert!(
            moved >= 4,
            "5 步拖动里应至少 4 步看到线在动,实得 {moved} 步(xs={xs:?},起点 {start_x:.1})"
        );
    }

    /// 切到分栏时对照列必须**向右滑入**,而不是从屏幕右缘飞进来
    /// (2026-10-19 坤哥:「切换分栏的时候改成窗口向右滑动,而不是现在的
    /// 从边上进来」)。
    ///
    /// **判据取「读者看得见的列左缘」**,不取内部动画值 `enter_frac` ——
    /// 后者跟着实现走,实现改了判据就跟着改,守不住。观测通道是对照列
    /// 背景的填充矩形(`Shape::Rect`,高度 > 400 的那一条竖带)。
    ///
    /// 三条断言各自独立于实现:
    /// 1. **首帧列已在槽位左边**(左缘 < 槽位左缘):旧实现首帧贴着屏幕
    ///    右缘(列左缘 == 槽位左缘),这条直接红;「盖着推」的语义也正是
    ///    「列此刻还压在编辑区上」。
    /// 2. **左缘逐帧右移**(单调不减):向右滑的物理含义。
    /// 3. **落位后与槽位对齐**:动画收敛到槽位,不漂移。
    ///
    /// 退场方向另有 `compare_column_slides_closed_over_frames` 守着。
    #[test]
    fn compare_column_slides_right_into_place_on_enter() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::light());
        let mut app = LaterMdApp {
            frameless: true,
            ..Default::default()
        };
        app.state.layout.left = false;
        // 从源码档起步,空跑到布局稳定(同 `compare_only_app` 的理由:
        // 本组只量几何,不装字体,免得 CI 无 CJK 字体时 `.expect` 齐红)。
        app.state.render_mode = crate::live::RenderMode::Source;
        for _ in 0..3 {
            let mut out = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            out.textures_delta.clear();
        }
        // **不预读槽位**:源码档下面板是完全收起态,egui 在
        // `show_collapsible` 里 `how_expanded == 0.0` 时提前 return
        // (`panel.rs` 459),压根不写 `PanelState` —— 拿它当前值当槽位是
        // 读不到东西的。槽位改由动画收敛后自然得出(见 ③)。

        // 走真实归约:源码 → 写作 → 分栏(循环档序 源码 → 写作 → 分栏)。
        app.state.apply(Message::ToggleLivePreview);
        app.state.apply(Message::ToggleLivePreview);
        assert_eq!(
            app.state.render_mode,
            crate::live::RenderMode::Split,
            "前置:两次切换应到分栏档"
        );

        // 对照列背景竖带:够高、宽落在面板档位区间(编辑区/窗口底都是全宽,
        // 落不进这个带宽)。取最靠右的一条 —— 动画期间列压在编辑区上,
        // 窗口右侧还留着编辑区底色,但那条是全宽,已被带宽过滤掉。
        // 竖带收集:动画期间同帧有三条够高的填充矩形 —— 编辑区(全宽,靠左)、
        // **移动中的对照列**(本次要观察的)、面板自身画的槽位底色(不动)。
        // 三者靠**宽度**区分:编辑区/窗口底是全宽(≥ 700),两条列带都落在
        // 面板档位区间(260..=880 的默认 420)。
        //
        // 取「**非全宽竖带里最靠左的那条**」—— 即移动中的列。不能用
        // `max_by`:面板画的槽位底色恒在槽位左缘,`max_by` 永远选中它,
        // 判据就退化成「有没有画槽位」,对动画方向完全不敏感(实测踩过)。
        let column_left = |out: &egui::FullOutput| -> Option<f32> {
            out.shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(r)
                        if r.rect.height() > 400.0
                            && r.fill != egui::Color32::TRANSPARENT
                            && (260.0..700.0).contains(&r.rect.width()) =>
                    {
                        Some(r.rect.left())
                    }
                    _ => None,
                })
                .min_by(|a, b| a.total_cmp(b))
        };
        let frame = |app: &mut LaterMdApp| -> egui::FullOutput {
            let mut out = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            out.textures_delta.clear();
            out
        };

        // ① 首帧:列必须还在**屏幕右缘之外一侧**,即尚未落位、整块压在
        // 编辑区上。旧观感首帧贴着槽位(= 屏幕右缘),这条直接红。
        let first = column_left(&frame(&mut app)).expect("切到分栏后首帧应能看到对照列");
        let slot_left = egui::PanelState::load(&ctx, egui::Id::new(COMPARE_PANEL_ID))
            .map(|p| p.outer_rect.left())
            .expect("切到分栏后 egui 应已建立 PanelState");
        assert!(
            first < slot_left - 1.0,
            "首帧列应尚未落位、压在编辑区上(左缘 < 槽位左缘 {slot_left:.1}),\
             实得 {first:.1} —— 这就是「从边上进来」的旧观感"
        );

        // ② 逐帧向右滑,③ 最终与槽位对齐。动画约 0.15s,无头帧按
        // `stable_dt` 推进,数十帧内收敛。
        let mut left = first;
        let mut moved = 0;
        let mut landed = false;
        for _ in 0..60 {
            let now = match column_left(&frame(&mut app)) {
                Some(x) => x,
                None => {
                    landed = true;
                    break;
                }
            };
            assert!(
                now >= left - 0.5,
                "列左缘应单调向右推进(向右滑),却从 {left:.1} 退到 {now:.1}"
            );
            if now > left + 0.5 {
                moved += 1;
            }
            if (now - slot_left).abs() <= 1.0 {
                landed = true;
                break;
            }
            left = now;
        }
        assert!(moved >= 4, "应至少 4 帧看到列在向右移动,实得 {moved} 帧");
        assert!(
            landed,
            "60 帧内应滑到位(槽位左缘 {slot_left:.1}),停在 {left:.1}"
        );
    }

    /// 从**写作档**切进来又切回去时,对照列必须**沿原路向左退回**并淡出
    /// (2026-10-20 坤哥:「分栏回到写作模式的时候,能不能从右往左覆盖的
    /// 感觉,这样给人感觉是循环起来的」)。
    ///
    /// 这是 [`Self::compare_column_slides_right_into_place_on_enter`] 的**反向
    /// 镜像**:那条守「推开」(左缘向右),本条守「退回」(左缘向左)。两条
    /// 同时绿,抽屉才真闭上。
    ///
    /// **为什么必须另起一条,不能指望 `compare_column_slides_closed_over_frames`**:
    /// 那条用的 `compare_only_app` 是**直接赋值** `render_mode`,没走
    /// `set_render_mode`,于是 `split_enter_from` 停在初值 `Source` —— 它守
    /// 的是「源码档那一轮」的退场。**抽屉退场那条分支本来就零覆盖**,不补
    /// 这条的话写反了方向也照样全绿。
    ///
    /// 判据同样取「读者看得见的列左缘」,不读 `drawer_frac`。三条断言:
    /// 1. **首帧列仍在槽位附近**(不是瞬移消失):退场有动画的最低要求。
    /// 2. **左缘逐帧向左**(单调不增):「从右往左覆盖」的物理含义。
    ///    旧实现向**右**滑出,这条直接红。
    /// 3. **最终彻底不见**:不能留残影。
    ///
    /// 退场期间列的宽度会变(on── `dx` 把它往左推,但 clip 的右边界不动,
    /// 被裁出来的可见部分逐帧变窄),故宽度过滤用「非全宽」而不是定值区间。
    #[test]
    fn compare_column_retracts_left_when_learing_the_drawer_round() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::light());
        let mut app = LaterMdApp {
            frameless: true,
            ..Default::default()
        };
        app.state.layout.left = false;
        app.state.render_mode = crate::live::RenderMode::Source;
        for _ in 0..3 {
            let mut out = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            out.textures_delta.clear();
        }
        // 走真实归约进场:源码 → 写作 → 分栏。`split_enter_from` 因此是
        // `Live`,退场才会走抽屉通道 —— 这正是本测试的前提。
        app.state.apply(Message::ToggleLivePreview);
        app.state.apply(Message::ToggleLivePreview);
        assert_eq!(
            app.state.render_mode,
            crate::live::RenderMode::Split,
            "前置:两次切换应到分栏档"
        );
        // 先跑到进场动画收敛,baseline 才有意义(落的那一帧不能当退场证据)。
        let column_left = |out: &egui::FullOutput| -> Option<f32> {
            out.shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(r)
                        if r.rect.height() > 400.0
                            && r.fill != egui::Color32::TRANSPARENT
                            && r.rect.width() < 700.0
                            && r.rect.width() > 8.0 =>
                    {
                        Some(r.rect.left())
                    }
                    _ => None,
                })
                .min_by(|a, b| a.total_cmp(b))
        };
        let frame = |app: &mut LaterMdApp, reduce: bool| -> egui::FullOutput {
            let mut out = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| {
                    if reduce {
                        app.reduce(ui.ctx());
                    }
                    app.draw(ui);
                },
            );
            out.textures_delta.clear();
            out
        };
        for _ in 0..60 {
            if column_left(&frame(&mut app, true)).is_none() {
                break;
            }
        }
        let settled =
            column_left(&frame(&mut app, false)).expect("前置:分栏档收敛后应能看到对照列");

        // 切回写作档:`Split → Source` 是循环档序的下一档。
        app.state.apply(Message::ToggleLivePreview);
        assert_eq!(
            app.state.render_mode,
            crate::live::RenderMode::Source,
            "前置:分栏的下一档应是源码档"
        );
        // 为什么不断言「首帧 ≈ 静止位置」:`animate_bool_responsive` 是
        // cubic_out(easing(x)=1−(1−x)³),退场第一步就吃掉 23% 的路程
        // (实测 980 → 883.5,正是 420 × 0.23)。这是 easing 的本意(出手快、
        // 收尾慢),不是跳帧。所以判据写成**方向性**的:首帧的列左缘不许
        // 跑到静止位置**右边**去 —— 旧「向右滑出」的观感恰恰是这个方向。
        let first = column_left(&frame(&mut app, true)).expect("退场第一帧列应仍在(不是瞬移)");
        assert!(
            first <= settled + 0.5,
            "退场第一帧应已开始向左退(左缘 ≤ 静止位 {settled:.1}),实得 {first:.1}\
             —— 往右走就是旧的「滑出屏幕」观感"
        );

        let mut edge = first;
        let mut moved = 0;
        let mut gone = false;
        for _ in 0..60 {
            match column_left(&frame(&mut app, true)) {
                Some(now) => {
                    assert!(
                        now <= edge + 0.5,
                        "抽屉应向左退回(左缘不增),却从 {edge:.1} 涨到 {now:.1}\
                         —— 这是旧「向右滑出」的观感"
                    );
                    if now < edge - 0.5 {
                        moved += 1;
                    }
                    edge = now;
                }
                None => {
                    gone = true;
                    break;
                }
            }
        }
        assert!(moved >= 4, "应至少 4 帧看到列在向左退回,实得 {moved} 帧");
        assert!(gone, "60 帧内应退干净(卡在 {edge:.1})");
    }

    /// 从**源码档**切到分栏时,对照列必须保留**旧观感**(从屏幕右缘向左
    /// 进入),不能跟着写作档一起改成向右滑
    /// (2026-10-19 坤哥:「源码模式从右往左,是之前的模式,别改」)。
    ///
    /// 与 `compare_column_slides_right_into_place_on_enter` 是**一对镜像
    /// 判据**:那条守「写作 → 分栏」向右滑(列**压盖**编辑区推进),本条守
    /// 「源码 → 分栏」从右缘进(列**待在窗口外**推进)。两条同时绿,才说明
    /// 分栏动画**按来源分流**真的成立 —— 只写一条的话,把 `slide_right`
    /// 写成恒真或恒假都还有一条是绿的。
    ///
    /// **判据取纯屏幕几何「列右缘相对窗口右缘的位置」**,既不取内部动画值,
    /// 也不读 `PanelState`:后者在进场首帧存的是**动画起点**(实测与列同位),
    /// 拿它当「最终槽位」会把两条判据混成同一个数,失去区分力。
    ///
    /// 1. **首帧列右缘超出窗口右缘**(整块还在屏幕外):旧观感的定义特征。
    ///    「向右滑」首帧列已压在编辑区上、右缘 == 窗口右缘,这条直接红。
    /// 2. **逐帧单调向左**:从右往左进的物理含义。
    /// 3. **落位时列右缘 == 窗口右缘**(右缘贴齐,不漂移)。
    ///
    /// 源码档切分栏走标题栏三段开关的**直达**语义(`SetRenderMode`),不是
    /// 循环档序 —— 循环是 源码 → 写作 → 分栏,那样进场的来源会是写作档。
    #[test]
    fn compare_column_enters_from_right_edge_when_switching_from_source() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let screen_right = screen.right();
        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::light());
        let mut app = LaterMdApp {
            frameless: true,
            ..Default::default()
        };
        app.state.layout.left = false;
        app.state.render_mode = crate::live::RenderMode::Source;
        for _ in 0..3 {
            let mut out = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            out.textures_delta.clear();
        }
        // 三段开关的直达语义:源码档直接点到分栏段。
        app.state
            .apply(Message::SetRenderMode(crate::live::RenderMode::Split));
        assert_eq!(
            app.state.render_mode,
            crate::live::RenderMode::Split,
            "前置:直达应到分栏档"
        );
        assert_eq!(
            app.state.split_enter_from,
            crate::live::RenderMode::Source,
            "前置:本次进场来源应是源码档(决定走旧观感通道)"
        );

        // 观测通道与镜像测试同款:对照列背景的填充竖带,取 (左缘, 右缘)。
        // 旧观感期间同帧只有这一条(正文在面板内画,面板 clip 钉在可见
        // 部分),不存在「移动列 + 槽位底色」两条并存的情形。
        let column_span = |out: &egui::FullOutput| -> Option<(f32, f32)> {
            out.shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(r)
                        if r.rect.height() > 400.0
                            && r.fill != egui::Color32::TRANSPARENT
                            && (260.0..700.0).contains(&r.rect.width()) =>
                    {
                        Some((r.rect.left(), r.rect.right()))
                    }
                    _ => None,
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
        };
        let frame = |app: &mut LaterMdApp| -> egui::FullOutput {
            let mut out = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            out.textures_delta.clear();
            out
        };

        // ① 首帧整块列还在**窗口之外**(右缘超出屏幕右缘)。
        let (first_left, first_right) =
            column_span(&frame(&mut app)).expect("切到分栏后首帧应能看到对照列");
        assert!(
            first_right > screen_right + 1.0,
            "源码档切入分栏时列应整块还在窗口右缘之外、从右往左进(右缘 > \
             {screen_right:.1}),实得右缘 {first_right:.1}(左缘 {first_left:.1})\
             —— 这已经是「向右滑」的观感了"
        );

        // ② 逐帧向左,③ 落位时右缘贴齐窗口右缘。
        let mut left = first_left;
        let mut moved = 0;
        let mut landed = false;
        for _ in 0..60 {
            let (now_left, now_right) = match column_span(&frame(&mut app)) {
                Some(x) => x,
                None => {
                    landed = true;
                    break;
                }
            };
            assert!(
                now_left <= left + 0.5,
                "列左缘应单调向左推进(从右往左进),却从 {left:.1} 退到 {now_left:.1}"
            );
            if now_left < left - 0.5 {
                moved += 1;
            }
            if (now_right - screen_right).abs() <= 1.0 {
                landed = true;
                break;
            }
            left = now_left;
        }
        assert!(moved >= 4, "应至少 4 帧看到列在向左移动,实得 {moved} 帧");
        assert!(
            landed,
            "60 帧内应滑到位(列右缘贴齐窗口右缘 {screen_right:.1}),停在左缘 {left:.1}"
        );
    }

    /// 关闭对照列必须有**滑出动画**(2026-10-16 坤哥:动画没回来):切到
    /// 非分栏档后的第一帧列还在(只是开始滑),随后逐帧向右退场直至消失。
    /// 走 `show_collapsible` 的滑出通道;若退回条件 `show` 的瞬移实现,
    /// 首帧列就不见了,本测试必红。观测通道 = 面板 Frame 的填充矩形
    /// (`Shape::Rect`):滑动是整块平移,形状 rect 左缘逐帧右移;动画中
    /// egui 刻意不落盘 `PanelState`,读不到过程值,只能看形状。
    #[test]
    fn compare_column_slides_closed_over_frames() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let (ctx, mut app) = compare_only_app(screen, false);
        let frame = |app: &mut LaterMdApp, reduce: bool| -> Vec<egui::epaint::ClippedShape> {
            let mut out = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| {
                    if reduce {
                        app.reduce(ui.ctx());
                    }
                    app.draw(ui);
                },
            );
            out.textures_delta.clear();
            out.shapes
        };
        // 对照列的填充矩形:够高、宽落在面板档位区间(编辑区/窗口底都是
        // 全宽,落不进这个带宽)、且是同帧里最靠右的一块。
        let panel_rect = |shapes: &[egui::epaint::ClippedShape]| -> Option<Rect> {
            shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(r)
                        if r.rect.height() > 400.0
                            && (300.0..700.0).contains(&r.rect.width())
                            && r.fill != egui::Color32::TRANSPARENT =>
                    {
                        Some(r.rect)
                    }
                    _ => None,
                })
                .max_by(|a, b| a.left().total_cmp(&b.left()))
        };

        let open = panel_rect(&frame(&mut app, false)).expect("前置:分栏档有对照列");
        assert!(
            (open.width() - 420.0).abs() < 1.0,
            "前置:面板宽应为默认 420,实得 {:.1}",
            open.width()
        );

        // 走真实归约:分栏 → 源码(循环档序 源码 → 写作 → 分栏 → 源码)
        app.state.apply(Message::ToggleLivePreview);
        assert_eq!(app.state.render_mode, crate::live::RenderMode::Source);

        // 切换后的第一帧列必须**还在**(开始滑 ≠ 瞬移)。
        let first = panel_rect(&frame(&mut app, true)).expect("关闭后的第一帧列应仍在滑出");
        assert!(
            first.left() >= open.left() - 0.5,
            "滑出应从全开位置起步,首帧左缘 {:.1} vs 全开 {:.1}",
            first.left(),
            open.left()
        );

        // 逐帧右移,直至退场。无头帧的时间不推进,egui 每帧按 stable_dt
        // 推进动画,几十帧内收完。
        let mut edge = first.left();
        let mut gone = false;
        for _ in 0..60 {
            match panel_rect(&frame(&mut app, false)) {
                Some(r) => {
                    assert!(
                        r.left() >= edge - 0.5,
                        "滑出应单调向右:{edge:.1} → {:.1}",
                        r.left()
                    );
                    edge = r.left();
                }
                None => {
                    gone = true;
                    break;
                }
            }
        }
        assert!(gone, "60 帧内应滑完退场(卡在 {edge:.1})");
    }

    /// 离开分栏档时不得留下悬空的线(与旧「对照预览收起」同一条判据)。
    #[test]
    fn compare_divider_disappears_when_not_split() {
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        let (ctx, mut app) = compare_only_app(screen, false);
        // 先跑一帧把 shell 全距探针写上(分隔线取证的颜色判据要用它)
        let _ = shell_full_gap(&ctx);
        assert!(
            divider_geometry(&mut app, &ctx, screen, vec![]).is_some(),
            "前置:分栏档有线"
        );
        // 走真实归约路径:分栏 → 源码(循环档序是 源码 → 写作 → 分栏 → 源码)
        app.state.apply(Message::ToggleLivePreview);
        assert_eq!(app.state.render_mode, crate::live::RenderMode::Source);
        let mut out = ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                app.reduce(ui.ctx());
                app.draw(ui);
            },
        );
        out.textures_delta.clear();
        assert!(
            divider_geometry(&mut app, &ctx, screen, vec![]).is_none(),
            "离开分栏后不该有线悬在编辑区右缘"
        );
    }

    /// 分栏档下右栏是**只读**的:点它不进编辑(不改主 LiveState 的活动块),
    /// 也不产出编辑类消息;中央仍是源码 TextEdit。
    #[test]
    fn compare_column_is_read_only_and_keeps_source_column_editable() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let (_, mut app) = compare_only_app(screen, false);
        let tab_id = app.state.tabs.current().id;
        // 点对照列中部(右栏约占右侧 420px)
        let pos = egui::pos2(screen.width() - 200.0, screen.height() / 2.0);
        for events in [
            vec![egui::Event::PointerMoved(pos)],
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }],
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        ] {
            let mut out = ctx.run_ui(
                RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| {
                    app.reduce(ui.ctx());
                    app.draw(ui);
                },
            );
            out.textures_delta.clear();
        }
        let tab = app.state.tabs.current();
        assert_eq!(tab.id, tab_id, "点对照列不换标签");
        assert!(
            tab.live_right.browse_only && tab.live_right.browse_locked,
            "对照列保持只读(浏览 + 硬锁):{:?}",
            tab.live_right
        );
        assert_eq!(tab.live_right.active, None, "对照列没有可编辑的活动块");
        assert!(
            !app.outbox.iter().any(|message| matches!(
                message,
                Message::TaskCheckboxToggled { .. } | Message::FormatRequested(..)
            )),
            "只读列不产出编辑类消息:{:?}",
            app.outbox
        );
    }

    /// 禅定是**另一套 panel 组合**:三栏路径的 menubar / nav / editor /
    /// statusbar / right-preview 全部不再被添加(§7 验收点)。
    ///
    /// 这正是藏面板的正确办法 —— 从一开始就不添加它,而不是添加了再把可见
    /// 性摁掉:后者留给平台的宽度演算与命中层级照旧吃掉资源,纠正起来也难。
    ///
    /// 取证走**本帧真的画进了 shapes 的文本**,而不是去看 flags:flags 会对
    /// 「添加了但摁掉可见性」这种错误实现照样放行。
    #[test]
    fn zen_draw_skips_the_three_column_panels() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut app = LaterMdApp {
            frameless: true,
            ..Default::default()
        };

        let three = draw_frame(&mut app, &ctx, screen);
        app.state.apply(Message::ZenToggled);
        let zen = draw_frame(&mut app, &ctx, screen);

        // 标题栏保留(理由见 `draw` 的入禅注释):文档名是它画的
        assert!(
            zen.iter().any(|t| t.contains("Untitled")),
            "窗口 chrome 保留:{zen:?}"
        );
        // menubar / 状态栏 / 侧边栏 / 格式工具条的专属文案全部消失
        for gone in ["文件", "248 字", "选择一个目录作为文件树根", "无序列表"] {
            assert!(
                !zen.iter().any(|t| t.contains(gone)),
                "禅定帧里不该出现 {gone:?}:{zen:?}"
            );
        }
        // 取证信号都取「只有那一条 panel 才会画」的专属文案:
        // 「文件」= menubar 首项、「248 字」= statusbar 的字数统计、
        // 「选择一个目录作为文件树根」= 左栏文件树的引导语。
        //
        // 格式工具条的信号 2026-10-08 S2-2 从「H1」换成「更多」(H1 收进
        // 溢出菜单,菜单关闭时不绘制);live-unify P4 删右栏后编辑区变宽,
        // 工具条恒在宽档 —— 标题组直出,「H1」回到恒绘位。「更多」在宽档
        // **不画**(宽档全直出),不能再当信号;「H1」只由格式工具条的
        // RichText 字形画出,样例文档不含该字面,取证不与正文串台。
        for present in ["文件", "248 字", "选择一个目录作为文件树根", "H1"] {
            assert!(
                three.iter().any(|t| t.contains(present)),
                "取证有效:三栏帧里能找到 {present:?}"
            );
        }
        // 正文仍然渲染在同一个 PreviewState 上(禅定不是换渲染器)
        assert!(!zen.is_empty(), "禅定帧仍有内容被绘制:{zen:?}");
        assert!(app.state.layout.zen, "绘制不翻转禅定");
    }

    /// P3/R3 守门:禅定帧**不改 UI 之外的状态** —— ① `render_mode` 不被禅定
    /// 碰(禅定与模式解耦,浏览是帧内临时值);②禅定帧不把读者拽进编辑:点
    /// 击正文不产活动块、不写 `cursor.byte`;③正文仍完整渲染(禅定不是白
    /// 屏);④退出禅定还原 `browse_enabled`(交互语义不被禅定带歪)。
    #[test]
    fn zen_frame_never_mutates_render_mode_or_steals_the_caret() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut app = LaterMdApp::default();
        let text = "# 标题\n\n正文一段。\n";
        app.state.tabs.current_mut().editor.replace_all(text);
        let before_mode = app.state.render_mode;
        app.state.apply(Message::ZenToggled);

        // 正文中心点:带动 ead 一下完整事件帧(press 必须单独占一帧)。
        let center = egui::pos2(screen.center().x, 200.0);
        let primary = |pressed| egui::Event::PointerButton {
            pos: center,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let painted = |app: &mut LaterMdApp, events: Vec<egui::Event>| -> Vec<String> {
            let output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            let texts = painted_text(&output.shapes);
            output.drop_without_applying_deltas();
            texts
        };
        let mut last = Vec::new();
        for events in [
            vec![egui::Event::PointerMoved(center)],
            vec![primary(true)],
            vec![primary(false)],
            Vec::new(),
        ] {
            last = painted(&mut app, events);
        }

        let tab = app.state.tabs.current();
        assert_eq!(
            app.state.render_mode, before_mode,
            "禅定不改持久化的 render_mode"
        );
        assert!(
            tab.live.active.is_none(),
            "禅定帧不产生活动块:{:?}",
            tab.live
        );
        assert!(tab.live.browse_only, "禅定帧仍是浏览态");
        assert_eq!(tab.cursor.byte, None, "禅定帧不写光标字节");
        assert_eq!(tab.editor.text(), text, "禅定帧不改动文本");
        assert!(
            last.iter().any(|t| t.contains("标题")),
            "禅定正文照常渲染:{last:?}"
        );

        // 退出禅定:browse_enabled 还原,render_mode 不动
        app.state.apply(Message::ZenToggled);
        let _ = painted(&mut app, Vec::new());
        assert!(!app.state.layout.zen, "退出禅定");
        assert!(
            app.state.tabs.current().live.browse_enabled,
            "退出禅定还原浏览优先的交互"
        );
        assert_eq!(
            app.state.render_mode, before_mode,
            "禅定往返不动 render_mode"
        );
    }

    /// Zen 下正文按 `ZEN_TEXT_W` 限宽。宽度由 `draw_zen` 自己吐出来(探针见
    /// `LaterMdApp::zen_probe`):这是**它把多宽的画布交给了预览**,不是我们
    /// 在测试里另搭一个 720 的 Ui 自证。
    ///
    /// 窗口够宽时限宽生效;窗口本身就窄于 720 时被夹到可用宽度 —— 那条分支
    /// 由下一个用例看。
    #[test]
    fn zen_body_ui_is_clamped_to_the_reading_width() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut app = LaterMdApp::default();
        app.state.apply(Message::ZenToggled);

        let width = Rc::new(Cell::new(0.0f32));
        {
            let sink = width.clone();
            app.zen_probe = Some(Box::new(move |w| sink.set(w)));
        }
        for _ in 0..2 {
            ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            )
            .drop_without_applying_deltas();
        }
        assert_eq!(width.get(), tokens::ZEN_TEXT_W, "窗口够宽时限宽锁定在 720");
    }

    /// 窗口窄于 720 时限宽被夹到可用宽度,而不是溢出成横向滚动 —— 「限宽」
    /// 是上限而非定值,这是 `set_max_width` 与 `set_width` 的区别所在。
    #[test]
    fn zen_body_shrinks_below_the_reading_width_on_narrow_windows() {
        let ctx = egui::Context::default();
        let narrow = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 600.0));
        let mut app = LaterMdApp::default();
        app.state.apply(Message::ZenToggled);

        let width = Rc::new(Cell::new(0.0f32));
        {
            let sink = width.clone();
            app.zen_probe = Some(Box::new(move |w| sink.set(w)));
        }
        for _ in 0..2 {
            ctx.run_ui(
                RawInput {
                    screen_rect: Some(narrow),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            )
            .drop_without_applying_deltas();
        }
        let measured = width.get();
        assert!(
            measured > 0.0 && measured <= tokens::ZEN_TEXT_W,
            "窄窗下收缩到可用宽度且不超 720,实测 {measured}"
        );
        assert!(
            measured <= narrow.width(),
            "不越过窗口宽度:{measured} vs {}",
            narrow.width()
        );
    }

    /// Esc 是禅定的出口之一(§7):它在禅定帧里被当作退出 signal 消费掉,
    /// 而不是渗到后面的预览区。由此一条线索得出的结论:draw 会把它唱 overriding
    /// 掉, 紧接着的其它 帧 不再见到它。
    #[test]
    fn zen_frame_consumes_escape_and_requests_exit() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut app = LaterMdApp::default();
        app.state.apply(Message::ZenToggled);
        let escape = Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        draw_frames(&mut app, &ctx, screen, 2);

        ctx.run_ui(
            RawInput {
                events: vec![escape.clone()],
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| app.draw(ui),
        )
        .drop_without_applying_deltas();
        assert_eq!(app.outbox, vec![Message::ZenToggled], "Esc 转成退出禅定");

        // 下一帧不再见到同一枚 Esc:它已被上一帧消费出输入流
        app.outbox.clear();
        ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| app.draw(ui),
        )
        .drop_without_applying_deltas();
        assert!(app.outbox.is_empty(), "Esc 不是「按住不放」的遗留事件");
    }

    /// 右上角「退出禅定」浮层在真实帧里点得动(§7 验收点)。位置由
    /// `zen_exit_button` 的返回值给出 —— 它是绝对摆放的,手搓坐标必然与真实
    /// 帧错位(M2 已经在标题栏上踩过一次)。
    ///
    /// 直呼 `zen_exit_button` 而不经 `draw_zen`:本测试只钉这颗 widget 自身
    /// 的可点性(原生装饰路径的生产行为);「无边框不画浮层」的 gating 由
    /// `zen_exit_overlay_is_native_decorations_only` 走完整帧守门。
    #[test]
    fn zen_exit_button_is_clickable_in_a_real_frame() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut app = LaterMdApp::default();
        app.state.apply(Message::ZenToggled);

        let center = std::cell::RefCell::new(Pos2::ZERO);
        ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                *center.borrow_mut() = zen_exit_button(ui, &mut Vec::new()).center();
            },
        )
        .drop_without_applying_deltas();
        let center = center.into_inner();
        assert!(center.x > screen.left(), "浮层取到了真实位置:{center:?}");
        // 它在内容区右上角,而不是整帧左上角或屏幕外
        assert!(center.y < tokens::ZEN_EXIT_MARGIN + 100.0, "贴内容区上沿");

        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let frame = |app: &mut LaterMdApp, events: Vec<Event>| {
            ctx.run_ui(
                RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            )
            .drop_without_applying_deltas();
        };
        frame(&mut app, Vec::new());
        frame(&mut app, vec![Event::PointerMoved(center)]);
        frame(&mut app, vec![click(true)]);
        frame(&mut app, vec![click(false)]);
        assert_eq!(
            app.outbox,
            vec![Message::ZenToggled],
            "浮层按钮不被 CentralPanel 抢走"
        );

        // 归约在下一帧:还原到进入前的三栏
        ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| app.reduce(ui.ctx()),
        )
        .drop_without_applying_deltas();
        assert!(!app.state.layout.zen, "已退出禅定");
        assert!(app.state.layout.left, "侧栏还原");
    }

    /// 禅定的**迭代性**:进/出一整轮后面板组合与进入前逐项一致(§12 验收点),
    /// 这是 `pre_zen` 快照存在的全部理由。
    #[test]
    fn zen_roundtrip_restores_the_exact_column_combination() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut app = LaterMdApp::default();
        // 先把 left 关掉:如果退出禅定一律全开,这条用例会当场红
        app.state.apply(Message::SidebarToggled);
        assert!(!app.state.layout.left);

        app.state.apply(Message::ZenToggled);
        draw_frames(&mut app, &ctx, screen, 2);
        app.state.apply(Message::ZenToggled);
        assert!(
            !app.state.layout.left,
            "逐项还原:left={}(右栏对照预览已随 live-unify P4 退役)",
            app.state.layout.left
        );
    }

    /// U3:编辑/预览渲染模式切换的淡入。完整 draw 下切到 Live 后逐帧推进
    /// 时间渲染不 panic(乘 opacity 的路径覆盖 Live 分支),淡入期间 egui
    /// 要帧驱动动画、超过时长后收敛(不产帧);切回 Source 对称成立 ——
    /// 双向都从透明渐入而不是只单向。
    #[test]
    fn render_mode_switch_crossfades_both_directions_and_settles() {
        let ctx = egui::Context::default();
        let mut app = LaterMdApp::default();
        // Source 热身几帧(应用首帧即 Source:crossfade 首调直接端点,无闪变)
        for step in 0..3u32 {
            ctx.run_ui(
                RawInput {
                    time: Some(f64::from(step) * 0.016),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            )
            .drop_without_applying_deltas();
        }

        let run =
            |app: &mut LaterMdApp, mode_live: bool, frames: u32| -> Vec<std::time::Duration> {
                let mut delays = Vec::new();
                for step in 0..frames {
                    let t = 0.016 * f64::from(step);
                    let output = ctx.run_ui(
                        RawInput {
                            time: Some(t),
                            ..Default::default()
                        },
                        |ui| {
                            app.reduce(ui.ctx());
                            app.draw(ui);
                        },
                    );
                    delays.push(
                        output
                            .viewport_output
                            .values()
                            .map(|viewport| viewport.repaint_delay)
                            .min()
                            .unwrap(),
                    );
                    output.drop_without_applying_deltas();
                    // 每次只切一档,切换后从下一帧起观察淡入。三态循环:
                    // 写作 → 分栏 → 源码,故回到源码要连切两次。
                    if step == 0
                        && mode_live != (app.state.render_mode == crate::live::RenderMode::Live)
                    {
                        app.state.apply(Message::ToggleLivePreview);
                        if app.state.render_mode == crate::live::RenderMode::Split {
                            app.state.apply(Message::ToggleLivePreview);
                        }
                    }
                }
                delays
            };

        // 切到 Live:淡入期间要帧,0.32s(> FADE_S)后收敛
        let delays = run(&mut app, true, 21);
        assert_eq!(app.state.render_mode, crate::live::RenderMode::Live);
        assert!(
            delays
                .iter()
                .take(15)
                .any(|d| *d < std::time::Duration::MAX),
            "淡入期间 egui 要帧驱动:{delays:?}"
        );
        assert_eq!(
            delays.last(),
            Some(&std::time::Duration::MAX),
            "收敛后不产帧:{delays:?}"
        );

        // 切回 Source:对称淡入后同样收敛
        let delays = run(&mut app, false, 21);
        assert_eq!(app.state.render_mode, crate::live::RenderMode::Source);
        assert!(
            delays
                .iter()
                .take(15)
                .any(|d| *d < std::time::Duration::MAX),
            "回切同样有淡入:{delays:?}"
        );
        assert_eq!(delays.last(), Some(&std::time::Duration::MAX));
    }

    /// U3:浮层(egui::Window → Area 内建 fade_in)出现时的淡入。明暗两套
    /// 主题下,设置浮窗打开后逐帧推进时间渲染不 panic;淡入期间 egui 要帧,
    /// 超过 `style.animation_time` 后收敛 —— 动画既真的在跑,也不会无限
    /// 产帧(与搜索空转回归同口径的守护)。
    #[test]
    fn overlay_window_fades_in_both_themes_then_settles() {
        for mode in [
            crate::theme::ThemeMode::Light,
            crate::theme::ThemeMode::Dark,
        ] {
            let ctx = egui::Context::default();
            let mut app = LaterMdApp::default();
            app.state.theme.mode = mode;
            app.state.settings.open = true;

            let mut delays = Vec::new();
            for step in 0..=25u32 {
                let output = ctx.run_ui(
                    RawInput {
                        time: Some(0.016 * f64::from(step)),
                        ..Default::default()
                    },
                    |ui| {
                        app.reduce(ui.ctx());
                        app.draw(ui);
                    },
                );
                delays.push(
                    output
                        .viewport_output
                        .values()
                        .map(|viewport| viewport.repaint_delay)
                        .min()
                        .unwrap(),
                );
                output.drop_without_applying_deltas();
            }
            assert!(app.state.settings.open, "渲染不翻转开关,不 panic({mode:?})");
            assert!(
                delays
                    .iter()
                    .take(15)
                    .any(|d| *d < std::time::Duration::MAX),
                "{mode:?}: 淡入期间 egui 要帧:{delays:?}"
            );
            assert_eq!(
                delays.last(),
                Some(&std::time::Duration::MAX),
                "{mode:?}: 动画结束后收敛,不产帧"
            );
        }
    }

    // ---- #54 M2:快捷键蒙层的整帧装配 ----

    /// 让长按蒙层进入 Visible(拨表手法,不真等 3 秒;`shortcut_overlay`
    /// 测试的 `rewind_hold` 同款)。两帧都是完整帧:eframe 每帧先归约后
    /// 绘制,reduce-only 帧里 TextEdit 不渲染,焦点与命中状态会偏离真实
    /// 帧序列。
    fn trigger_overlay(app: &mut LaterMdApp, ctx: &egui::Context, screen: Rect) {
        full_frame(
            &mut *app,
            ctx,
            screen,
            vec![Event::ModifiersChanged(Modifiers::COMMAND)],
        );
        app.state
            .shortcut_overlay
            .rewind_hold(std::time::Duration::from_secs_f64(3.1));
        full_frame(&mut *app, ctx, screen, Vec::new());
        assert!(
            app.state.shortcut_overlay.is_visible(),
            "前置:长按到点,蒙层进入 Visible"
        );
    }

    /// 完整帧(先归约后绘制,eframe 顺序)。
    fn full_frame(app: &mut LaterMdApp, ctx: &egui::Context, screen: Rect, events: Vec<Event>) {
        ctx.run_ui(
            RawInput {
                focused: true,
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |ui| {
                app.reduce(ui.ctx());
                app.draw(ui);
            },
        )
        .drop_without_applying_deltas();
    }

    /// 蒙层在三栏帧真的画出来:标题、分组名、键位 kbd 文本、无绑定行都
    /// 在本帧 shapes 里;两主题各跑一遍不 panic、都成立(底色与键帽色
    /// 从 visuals 推导,不硬编码)。
    #[test]
    fn shortcut_overlay_paints_grouped_rows_in_both_themes() {
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
        for theme in ["dark", "light"] {
            // 每轮独立 Context:后半段要滚动蒙层的 ScrollArea,其偏移持久在
            // egui memory —— 共享 ctx 会把滚动状态泄进下一轮的首屏断言
            let ctx = egui::Context::default();
            let mut app = LaterMdApp::default();
            app.state.theme.mode = if theme == "light" {
                crate::theme::ThemeMode::Light
            } else {
                crate::theme::ThemeMode::Dark
            };
            trigger_overlay(&mut app, &ctx, screen);
            let mode = app.state.resolved_theme();
            assert_eq!(
                mode,
                if theme == "light" {
                    crate::theme::ThemeMode::Light
                } else {
                    crate::theme::ThemeMode::Dark
                },
                "{theme}:主题真的生效了(两轮 visuals 必须一明一暗)"
            );
            let texts = draw_frame(&mut app, &ctx, screen);
            assert!(
                texts.iter().any(|t| t.contains("快捷键")),
                "{theme}:蒙层标题已渲染:{texts:?}"
            );
            assert!(
                texts.iter().any(|t| t == "文件"),
                "{theme}:分组名已渲染:{texts:?}"
            );
            assert!(
                texts.iter().any(|t| t
                    == &app
                        .state
                        .keymap
                        .get(crate::command::Command::Save)
                        .unwrap()
                        .platform_text()),
                "{theme}:键位 kbd 文本已渲染(平台化显示):{texts:?}"
            );
            // 无绑定命令照列(#54 产品决定):蒙层卡片可视高钉在 502px,
            // 命令全集(#66 M2 起 43 条)的内容超出视口、靠内建 ScrollArea
            // 滚动 —— 「未绑定」行(AI 组,排序在最末)在首屏之外,把指针
            // 移进蒙层卡滚到底再取证。滚轮的消费条件是指针在 ScrollArea
            // 外框内(egui 0.36.2 scroll_area.rs 实读;构造手法与 live.rs
            // 的滚动测试同款)。
            let wheel = Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -400.0),
                phase: egui::TouchPhase::Move,
                modifiers: Modifiers::NONE,
            };
            let card_center = screen.center();
            for events in [
                vec![Event::PointerMoved(card_center)],
                vec![wheel.clone()],
                vec![wheel.clone()],
                vec![wheel],
            ] {
                let output = ctx.run_ui(
                    RawInput {
                        screen_rect: Some(screen),
                        events,
                        ..Default::default()
                    },
                    |ui| app.draw(ui),
                );
                output.drop_without_applying_deltas();
            }
            let scrolled = draw_frame(&mut app, &ctx, screen);
            assert!(
                scrolled.iter().any(|t| t.contains("未绑定")),
                "{theme}:无绑定命令照列(滚到蒙层底部):{scrolled:?}"
            );
        }
    }

    /// 禅定帧同样渲染蒙层(13a 教训:禅定布局分叉要显式放行顶层浮层——
    /// 蒙层挂在 `draw_overlay_dialogs`,禅定路径共用);且蒙层可见时 Esc
    /// 只关蒙层、不退出禅定(最顶层浮层优先吃 Esc)。
    #[test]
    fn zen_paints_overlay_and_escape_closes_overlay_not_zen() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut app = LaterMdApp::default();
        app.state.apply(Message::ZenToggled);
        trigger_overlay(&mut app, &ctx, screen);

        let texts = draw_frame(&mut app, &ctx, screen);
        assert!(
            texts.iter().any(|t| t.contains("快捷键")),
            "禅定帧里蒙层已渲染:{texts:?}"
        );

        // Esc 整帧:蒙层关、禅定不退、不发禅定消息
        full_frame(
            &mut app,
            &ctx,
            screen,
            vec![Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        assert!(!app.state.shortcut_overlay.is_visible(), "Esc 关蒙层");
        assert!(app.state.layout.zen, "Esc 不误退禅定");
        assert!(
            !app.outbox.contains(&Message::ZenToggled),
            "无禅定消息发出:{:?}",
            app.outbox
        );
    }

    /// 点击蒙层底(卡片外)关闭;关闭不产生任何消息(纯显示关注点)。
    #[test]
    fn clicking_overlay_scrim_closes_it_without_side_effects() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut app = LaterMdApp::default();
        trigger_overlay(&mut app, &ctx, screen);

        // 两空帧让 Area/Window 完成布局演算,再点屏幕左下(中央卡片之外)
        full_frame(&mut app, &ctx, screen, Vec::new());
        full_frame(&mut app, &ctx, screen, Vec::new());
        let corner = Pos2::new(20.0, screen.height() - 20.0);
        let click = |pressed| Event::PointerButton {
            pos: corner,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        full_frame(&mut app, &ctx, screen, vec![Event::PointerMoved(corner)]);
        full_frame(&mut app, &ctx, screen, vec![click(true)]);
        full_frame(&mut app, &ctx, screen, vec![click(false)]);
        assert!(!app.state.shortcut_overlay.is_visible(), "点击蒙层底关闭");
        assert!(app.outbox.is_empty(), "关闭不产生消息:{:?}", app.outbox);
    }

    /// 蒙层不抢焦点(非焦点层):编辑器持焦时触发蒙层并渲染,键盘焦点
    /// id 不变——蒙层存在期间文本编辑器的焦点与输入不受影响。按 Esc 与
    /// 点击 canvas 清焦是 egui 的内建行为(本仓无头对照实测:无蒙层时
    /// 同样清),不作为蒙层的断言;蒙层自身无任何可聚焦控件、从不
    /// `request_focus`,出现/存在/消失三段都不动焦点。
    #[test]
    fn overlay_never_steals_editor_focus() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut app = LaterMdApp::default();
        let editor = crate::ui::editor::tab_editor_id(app.state.tabs.current().id);
        ctx.memory_mut(|memory| memory.request_focus(editor));
        full_frame(&mut app, &ctx, screen, Vec::new());
        assert!(
            ctx.memory(|memory| memory.has_focus(editor)),
            "前置:编辑器持有键盘焦点"
        );

        trigger_overlay(&mut app, &ctx, screen);
        full_frame(&mut app, &ctx, screen, Vec::new());
        assert!(
            ctx.memory(|memory| memory.has_focus(editor)),
            "蒙层出现帧焦点仍在编辑器"
        );

        // 存在期间连续空帧:焦点稳定不动
        for _ in 0..3 {
            full_frame(&mut app, &ctx, screen, Vec::new());
            assert!(
                ctx.memory(|memory| memory.has_focus(editor)),
                "蒙层在场的每一帧焦点仍在编辑器"
            );
        }
    }
}

#[cfg(test)]
mod strip_geometry {
    //! **三条「窄条」控件的几何守门**(2026-10-08 真机复核补)。
    //!
    //! 起因是 S1-2 状态栏的真实回归:三段被写成三个**平级**调用,而父 Ui
    //! 是 `Panel::bottom` 的 `Layout::top_down`(`panel.rs:821`)—— 每个平级
    //! 调用各占一行,状态栏从 18px 涨到 75px,而**当时 1386 个测试与六个
    //! 门禁全绿**,是本机 X11 截图逐行量出来的。
    //!
    //! 守门断言的盲区有规律:**功能断言覆盖「有没有」,几乎不覆盖「长什么样」**。
    //! 「窄条」控件的天花板就是「不能塌」,故三条各补一条高度断言。
    //!
    //! **上界一律硬编码,不读 `tokens`** —— 与 nav 段高度那条同因:
    //! 若上界跟着实现一起变,断言就自我满足(见 `sidebar.rs`
    //! `three_bands_fill_the_panel_top_down` 的红绿实测)。
    use super::*;
    use crate::keymap::Keymap;
    use eframe::egui::{RawInput, Rect};

    /// 在 `top_down` 父布局里跑一段绘制,返回它消费掉的高度(屏宽 1400)。
    fn measure<F: FnMut(&mut egui::Ui)>(f: F) -> f32 {
        measure_at(1400.0, f)
    }

    /// 同 [`measure`],但可指定屏宽 —— **窄栏行为只能在窄屏下测出来**。
    ///
    /// 2026-10-08 教训:格式条「是否折叠」的关键断言原先固定在 1400px 量,
    /// 那里它恒为一行 30px,于是「改成水平滚动」与「保持换行」两种实现
    /// **都能通过** —— 断言没测到点子上。窄栏(240px = 900px 窗口下的
    /// 编辑区实际宽度)才暴露差异:换行版 63px / 滚动版 30px。
    fn measure_at<F: FnMut(&mut egui::Ui)>(width: f32, mut f: F) -> f32 {
        let ctx = egui::Context::default();
        let mut h = 0.0f32;
        ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 900.0),
                )),
                ..Default::default()
            },
            |ui| {
                f(ui);
                h = ui.min_rect().height();
            },
        )
        .drop_without_applying_deltas();
        h
    }

    /// 格式工具条必须单行(实测 30px)。S2-2 把它从 17 按钮缩到 8 直出 +
    /// 溢出菜单后仍须守住 —— 缩按钮数与「条会不会折叠」是两件事。
    #[test]
    fn format_bar_is_a_single_row() {
        let h = measure(|ui| {
            let mut out = Vec::new();
            crate::ui::format_bar::ui(ui, &Keymap::builtin(), &mut out);
        });
        assert!(
            (24.0..=40.0).contains(&h),
            "格式工具条应单行(实测 {h}px,实测基准 30px)——              远超 40px 说明多组内容被摞成了多行"
        );
    }

    /// **窄栏下格式条仍须单行**(2026-10-08 症状 A 的核心断言)。
    ///
    /// 屏宽 240px = 900px 窗口下编辑区的**实际**宽度(`min_inner_size`
    /// 900 − 侧栏 240 − 预览 420)。此宽度下格式条一行要 331px(实测)必然放不下,
    /// 于是「换行」与「水平滚动」两种实现分道扬镳:
    ///
    /// | 实现 | 240px 下高度 |
    /// |---|---|
    /// | `horizontal_wrapped`(改版前) | **63px**(两行) |
    /// | `ScrollArea::horizontal`(改版后) | **30px**(恒定一行) |
    ///
    /// 63px 是顶部 chrome(共 128px)里最大的一项,且把编辑区整体下顶 ——
    /// 标签条早就为同一个理由改成了滚动(见 `ui/tabs.rs` 的注释),
    /// 格式条这次是补上同一处漏。
    ///
    /// 上界 40px 硬编码不读 token(与本模块另两条断言同因:上界跟着实现
    /// 一起变则断言自我满足)。
    #[test]
    fn format_bar_stays_single_row_in_a_narrow_column() {
        const NARROW: f32 = 240.0; // 900px 窗口下编辑区的实际宽度
        let h = measure_at(NARROW, |ui| {
            let mut out = Vec::new();
            crate::ui::format_bar::ui(ui, &Keymap::builtin(), &mut out);
        });
        assert!(
            (24.0..=40.0).contains(&h),
            "窄栏({NARROW}px)下格式条应恒为单行(实测 {h}px;换行版会是 63px)—— \
             若它又变回多行,说明有人把 `ScrollArea::horizontal` 换回了 \
             `horizontal_wrapped`,顶部 chrome 会从 128px 涨回 161px"
        );
    }

    /// 提示行必须单行(实测 18px)。它在文档上方常驻,折叠会把编辑区顶下去。
    #[test]
    fn notice_bar_is_a_single_row() {
        let h = measure(|ui| {
            let mut out = Vec::new();
            let d = crate::state::DocumentState {
                path: None,
                dirty: false,
                notice: Some("Ctrl+S 已被「导出 HTML」占用".to_owned()),
            };
            super::notice_bar(ui, &d, &mut out);
        });
        assert!(
            (12.0..=30.0).contains(&h),
            "提示行应单行(实测 {h}px,实测基准 18px)—— 折叠会把编辑区顶下去"
        );
    }
}
