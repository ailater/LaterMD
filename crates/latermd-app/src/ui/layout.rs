//! 布局:顶部菜单栏 + 三栏(侧边栏 / 编辑器 / 预览,docs/adr-005 §3.2)。
//!
//! 顺序铁律:panel 添加顺序决定嵌套,先加的最外层;`CentralPanel` 必须最后加。
//! `App::logic` 只归约状态,`App::ui` 只绘制,两者严格分离(铁律)。

use crate::state::Message;
use crate::ui::tokens;
use crate::LaterMdApp;
use eframe::egui;

impl LaterMdApp {
    /// `logic` 帧的全部归约逻辑。单独成函数是因为 [`eframe::Frame`] 的字段
    /// 是 `pub(crate)`,测试里造不出来;归约本身不碰 frame。
    fn reduce(&mut self, ctx: &egui::Context) {
        // 只做状态归约,严格禁止在此绘制任何 UI(docs/adr-005 §2.3)。
        let LaterMdApp {
            state,
            outbox,
            window_title,
            ..
        } = self;
        // 后台生产者使用事件唤醒代替满帧轮询。先清合并标志再收 channel，
        // drain 期间到达的新消息会重新请求下一帧，不会因竞态悬置。
        state.bind_repaint_context(ctx);
        state.begin_background_drain();
        // AI 后台流式收流:channel 里的 chunk 翻成 Message 并入本帧归约
        // (文本回编辑器只走 Message,后台线程不触碰 UI 状态)
        outbox.extend(state.poll_ai());
        // 图床上传收流:同一手法(channel → Message → 归约),结果只在归约
        // 落地(插入或回显),后台线程不碰 UI 状态
        outbox.extend(state.poll_bed());
        // 剪贴板图片读取收流:同上(D 段,arboard 的阻塞 IO 在后台线程)
        outbox.extend(state.poll_clipboard());
        // 图片拖入落盘(D 段):dropped_files 由 egui-winit 汇进 raw input,
        // `RawInput::take` 每帧清空,这里取走即消费。白名单外的不进归约
        // (egui 全窗口收文件,非图片文件的拖入不该弹图片提示);大小上限
        // 在归约里读文件后才判。读文件与落盘是本地磁盘(毫秒级),同步在
        // 归约做 —— 与文件树打开文件同口径。
        let dropped: Vec<std::path::PathBuf> = ctx
            .input_mut(|input| std::mem::take(&mut input.raw.dropped_files))
            .iter()
            .filter(|file| {
                file.path().extension().is_some_and(|ext| {
                    crate::assets::is_allowed_extension(&ext.to_string_lossy().to_lowercase())
                })
            })
            .map(|file| file.path().to_path_buf())
            .collect();
        for path in dropped {
            state.apply(Message::ImageFileDropped(path));
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
        // 搜索去抖仍按到期时刻要一帧；真正开始后由后台线程在命中/完成
        // 事件入队时唤醒，不再在 Running 期间满帧轮询。
        if let Some(due) = state.search.debounce_due {
            ctx.request_repaint_after(due.saturating_duration_since(std::time::Instant::now()));
        }
        // Git 状态轮询:有文件树根且尚未降级才轮询(无根无事可刷;非 git
        // 目录零轮询——重探由换根/切 Git 页触发,egui 得以收敛到深度空闲,
        // 这正是 search 去抖测试守护的不变量)。到点只发起后台单飞；未到点
        // 预约到期帧，结果入队再由通知器唤醒，不满帧轮询。
        if state.file_tree.root.is_some() && state.git.error.is_none() {
            if state.git.due() {
                state.refresh_git();
            }
            if state.git.error.is_none() {
                ctx.request_repaint_after(state.git.until_refresh());
            }
        }
        // AI、commit、Git、图床、剪贴板和 MCP 均在生产者成功发送结果后
        // 通过 RepaintNotifier 唤醒；此处不再按“任务在途”持续请求帧。

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
    /// eframe 顺序(先 `reduce` 后绘制)跑完整帧。
    fn draw(&mut self, ui: &mut egui::Ui) {
        // 禅定模式(§7)是**另一整套面板组合**,不是给三栏各加一个 if:
        // 藏面板的最佳办法是从一开始就不添加它(侧栏宽度演算与 z 序全部
        // 让位),而不是添加了再把可见性摁掉。故在这里整体分叉。
        //
        // 但**只对布局分叉,不对窗口 chrome 分叉**。规格 §7 的「全部退场」
        // 是按原生装饰窗口画的:自绘标题栏退场后,OS 那一根还在,窗口照样
        // 能拖能关。本产品的标题栏是自绘的(D1),无边框模式下 OS 不提供
        // 任何 chrome —— 真照字面连同 chrome 一起藏,退出禅定的四条出口里
        // 两条(标题栏 Zen 按钮、右上浮层入口所在的画布)会同时失效。D4
        // 担心的「怎么退出」在这里会成真,故 Zen 只让三栏让位:**它是布局
        // 态,不是窗口态**。
        if self.state.layout.zen {
            self.draw_zen(ui);
            return;
        }

        // ⓪ 自绘窗口骨架之一:36px 自绘标题栏(仅无边框模式;
        // LATERMD_NATIVE_DECORATIONS=1 的原生装饰路径不画,行为与旧版
        // 完全一致)。
        if self.frameless {
            egui::Panel::top("titlebar")
                .exact_size(crate::ui::tokens::TITLEBAR_H)
                .frame(
                    egui::Frame::default()
                        .inner_margin(egui::Margin::ZERO)
                        .fill(ui.visuals().panel_fill),
                )
                .show(ui, |ui| {
                    crate::ui::titlebar::ui(ui, &self.state, &mut self.outbox);
                });
        }

        // ① 次外层:顶部菜单栏(全部命令的可发现性入口)
        egui::Panel::top("menubar").show(ui, |ui| {
            crate::ui::menubar::ui(ui, &self.state.keymap, &mut self.outbox);
        });

        // ② 底部状态栏:散落在工具栏/侧边栏边缘的只读信息收成一行
        // (docs/ui-polish.md §4),工具栏得以只留动作。
        //
        // **必须在左右栏之前加**:先加的最外层、先画者占满全窗横向 ——
        // 若放在 nav/preview 之后,它就只在中央残余区里横跨,左右栏脚下
        // 各缺一截(2026-09-27 用户实测反馈的第二条)。
        egui::Panel::bottom("statusbar").show(ui, |ui| {
            status_bar(ui, &self.state);
        });

        // ③ 左栏:导航(文件树 / 搜索 / 大纲 / Git 四视图)。`show_collapsible`
        // 原地持有 `&mut bool`,因此先解构再把闭包要用的其余状态分头借用
        // (都与这两个 bool 不相交)。
        //
        // 宽度下限走 `SIDEBAR_MIN_W`(180,M2 三段式起够用):160 是二分栏
        // 时代的数字(docs/ui-shell-redesign.md §11 R4)。
        let layout = &mut self.state.layout;
        let left = &mut layout.left;
        let active_tab = &mut layout.left_view;
        egui::Panel::left("nav")
            .resizable(true)
            .default_size(240.0)
            .size_range(crate::ui::tokens::SIDEBAR_MIN_W..=400.0)
            .show_collapsible(ui, left, |ui| {
                crate::ui::sidebar::ui(
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
                    &mut self.outbox,
                );
            });
        // PanelState 当前帧可读 outer_rect;只更新用户实际拖出的宽度,
        // show_collapsible=false 或动画中不写 None。
        if let Some(panel) = egui::PanelState::load(ui.ctx(), egui::Id::new("nav")) {
            self.state.layout.left_width = Some(panel.size().x);
        }

        // ④ 右栏:只读预览。 `Panel::right` 必须在 `CentralPanel` 之前加
        // (先加的最外层),编辑器因此是吃剩余宽度的那个 —— 左右任意开合
        // 都只是让中间伸缩,不会挤掉谁。
        let right = &mut self.state.layout.right;
        egui::Panel::right("preview")
            .resizable(true)
            .default_size(crate::ui::tokens::PREVIEW_DEFAULT_W)
            .size_range(crate::ui::tokens::PREVIEW_MIN_W..=880.0)
            .frame(
                egui::Frame::default()
                    .inner_margin(egui::Margin::same(8))
                    .fill(crate::theme::content_fill(ui.visuals().dark_mode)),
            )
            .show_collapsible(ui, right, |ui| {
                let tab = self.state.tabs.current_mut();
                crate::ui::preview::ui(
                    ui,
                    &mut tab.preview,
                    &self.state.ai,
                    // 相对图片以文档所在目录为锚拼 file://(未落盘为 None)
                    tab.document
                        .path
                        .as_deref()
                        .and_then(std::path::Path::parent),
                    &mut self.outbox,
                );
            });
        if let Some(panel) = egui::PanelState::load(ui.ctx(), egui::Id::new("preview")) {
            self.state.layout.right_width = Some(panel.size().x);
        }

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
                    .inner_margin(egui::Margin::symmetric(8, 2))
                    .fill(crate::theme::content_fill(ui.visuals().dark_mode)),
            )
            .show(ui, |ui| {
                // 标签条(多标签 #11)在格式工具条之上:先选文档,再对文档操作
                crate::ui::tabs::ui(ui, &state.tabs, outbox);
                // 提示行(存在才显示;原文件工具栏的能力,工具栏退役后迁此,
                // decisions-pending #32)
                notice_bar(ui, &state.tabs.current().document, outbox);
                let tab = state.tabs.current_mut();
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
                );
                crate::ui::editor::ui(
                    ui,
                    editor,
                    preview,
                    crate::ui::editor::CursorChannel {
                        cursor,
                        selection,
                        pending: pending_selection,
                    },
                    live,
                    state.render_mode,
                    crate::ui::editor::tab_editor_id(*id),
                );
                source_rect = Some(ui.min_rect());
            });

        // ⑤ 查找卡是源码区专属浮层:不参与 CentralPanel 普通布局,因此
        // 不会把源码整体向下推;右上锚点留出滚动条/边框间隙。
        if state.find.open && state.render_mode == crate::live::RenderMode::Source {
            if let Some(rect) = source_rect {
                draw_find_overlay(ui.ctx(), rect, &mut state.find, outbox);
            }
        }

        // ⑥ 顶层浮层五件套(commit 建议 / 设置 / 回滚确认 / 关标签确认 /
        // 图片框):浮窗是独立 Area 层,不参与 panel 嵌套,画在 panel 之后
        // 取语义上的「最上层」。三栏与禅定两条布局路径共用,理由见
        // [`Self::draw_overlay_dialogs`]。
        self.draw_overlay_dialogs(ui);

        // ⑥ 自绘窗口骨架之二:屏幕四边/四角的透明缩放命令区。**必须在
        // 全部 panel 之后分配**(机制见 ui::titlebar::edge_resize_zones 的
        // 文档:同层命中、后分配者在同距裁决中胜出);此处光标推进位于
        // 所有面板之后,不影响任何 panel 的布局。
        if self.frameless {
            crate::ui::titlebar::edge_resize_zones(ui);
        }
    }

    /// 顶层浮层五件套,存在才显示:commit message 建议、设置对话框、回滚
    /// 确认、脏标签关闭确认、图片框。浮窗是独立 Area 层,不参与 panel 嵌套,
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
            let (_, close) = commit_dialog(ui, &subject);
            if close.clicked() {
                outbox.push(Message::AiCommitDismissed);
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
            let (confirm, cancel) = checkout_dialog(
                ui,
                &path,
                open_in_editor,
                self.state.tabs.current().editor.is_dirty(),
            );
            if confirm.clicked() {
                outbox.push(Message::GitCheckoutConfirmed);
            }
            if cancel.clicked() {
                outbox.push(Message::GitCheckoutCancelled);
            }
        }

        // 脏标签关闭确认(标签条 × / Ctrl+W 触发):目标按稳定 id 存
        // (`TabsState::confirm_close`),打开期间其他关闭入口会使索引漂移;
        // 目标被别的路径关掉时 `TabsState::remove` 已撤下确认,这里自然
        // 不再渲染。
        if let Some(tab) = self.state.tabs.confirm_close_tab() {
            let (confirm, cancel) = tab_close_dialog(ui, &tab.document.display_name());
            if confirm.clicked() {
                outbox.push(Message::TabCloseConfirmed);
            }
            if cancel.clicked() {
                outbox.push(Message::TabCloseCancelled);
            }
        }

        // 图片框(docs/image-plan.md A/B/C 三段):只收 alt 与 url 草稿及
        // 图床选择,插入在归约走 `compose::insert_image`;地址为空时「插入」
        // 按钮在对话框里已被禁用。点击插入时草稿还在 state 上,克隆进消息
        // 载荷。「浏览」与「上传」只发消息:文件选择、复制进 `.assets/`、
        // 后台上传都在归约(对话框不碰 IO)。
        if self.state.image_dialog.open {
            let (insert, browse, upload, cancel) = crate::ui::image_dialog::dialog(
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
            if cancel.clicked() {
                outbox.push(Message::ImageDialogClosed);
            }
        }
    }

    /// 禅定模式的整套面板组合(§7)。
    ///
    /// 四条出口在此合流:F11 / 标题栏 Zen 按钮 / Esc / 右上角「退出禅定」。
    fn draw_zen(&mut self, ui: &mut egui::Ui) {
        // Zen 是**另一套 panel 组合**而非「给三栏各加一个 if」:藏面板的最佳
        // 办法是从一开始就不添加它(侧栏宽度演算与 z 序全部让位),而不是
        // 添加了再把可见性摁掉。窗口 chrome 保留的理由见 [`Self::draw`]。
        if self.frameless {
            egui::Panel::top("titlebar")
                .exact_size(crate::ui::tokens::TITLEBAR_H)
                .frame(
                    egui::Frame::default()
                        .inner_margin(egui::Margin::ZERO)
                        .fill(ui.visuals().panel_fill),
                )
                .show(ui, |ui| {
                    crate::ui::titlebar::ui(ui, &self.state, &mut self.outbox);
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
                    .fill(crate::theme::content_fill(ui.visuals().dark_mode)),
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
                    // 与三栏路径同一个 widget:同一个 `PreviewState`、同一条
                    // 渲染链路,不另起一份。
                    //
                    // 解构再分头借用:`&mut self.state.tabs` 与 `&self.state.ai`
                    // 是同一棵树上的不相交分支,挨着写会被借用检查器拦下。
                    let LaterMdApp { state, outbox, .. } = self;
                    let tab = state.tabs.current_mut();
                    crate::ui::preview::ui(
                        ui,
                        &mut tab.preview,
                        &state.ai,
                        tab.document
                            .path
                            .as_deref()
                            .and_then(std::path::Path::parent),
                        outbox,
                    );
                });
            });

        // 右上角「退出禅定」:Zen 里唯一的常驻 chrome,因此必须是最后分配
        // 的那个 widget —— 同层命中的后来者优先(机制见
        // `ui::titlebar::edge_resize_zones` 的文档)。
        zen_exit_button(ui, &mut self.outbox);

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

/// 禅定模式的「退出禅定」浮层(§7)。返回其矩形供测试定位。
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

/// 查找条的源码区浮层:固定在源码宿主右上角,不参加正文布局。用 Window
/// 而不是普通 horizontal,并关闭 title bar/resize/collapse,避免它变成可拖
/// 对话框;id 稳定,位置每帧由源码 rect 重算,侧栏/窗口拖宽后不会漂走。
fn draw_find_overlay(
    ctx: &egui::Context,
    source_rect: egui::Rect,
    find: &mut crate::state::FindBarState,
    outbox: &mut Vec<Message>,
) {
    let margin = 8.0;
    // 源码区 rect 包含格式工具条下方的编辑器宿主;再向下留一段
    // 工具条高度+间距,避免浮层贴住工具条底边。Window 的 pivot 是
    // RIGHT_TOP,所以 y 正值就是向下。
    let anchor =
        source_rect.right_top() + egui::vec2(-margin, crate::ui::tokens::TOOLBAR_H + margin * 2.0);
    egui::Window::new("文档内查找")
        .id(egui::Id::new("editor-find-overlay"))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .fixed_pos(anchor)
        .pivot(egui::Align2::RIGHT_TOP)
        .order(egui::Order::Foreground)
        .constrain_to(source_rect.shrink(margin))
        .frame(egui::Frame::popup(&ctx.style_of(egui::Theme::Dark)))
        .show(ctx, |ui| find_bar_contents(ui, find, outbox));
}

/// 查找卡内容与状态无关,可在 Window/无头测试中复用。
fn find_bar_contents(
    ui: &mut egui::Ui,
    find: &mut crate::state::FindBarState,
    outbox: &mut Vec<Message>,
) {
    let total = find.hits.len();
    let pos = find.hit.map(|h| h + 1).unwrap_or(0);
    ui.horizontal(|ui| {
        ui.label("查找");
        let mut query_buf = find.query.clone();
        let response = ui.add(
            egui::TextEdit::singleline(&mut query_buf)
                .id(egui::Id::new("editor-find-input"))
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
        ui.weak(if total == 0 {
            "无结果".to_owned()
        } else {
            format!("{pos}/{total}")
        });
        if crate::ui::icons::icon_button(ui, crate::ui::icons::Icon::Close, "关闭查找 (Esc)")
            .clicked()
        {
            outbox.push(Message::FindBarToggled(false));
        }
    });
}

/// 底部状态栏:路径 · 行列 · 字数 · 主题 · AI · MCP。
fn status_bar(ui: &mut egui::Ui, state: &crate::state::State) {
    ui.horizontal_wrapped(|ui| {
        ui.weak(state.tabs.current().document.display_name());
        let text = state.tabs.current().editor.text();
        if let Some(byte) = state.tabs.current().cursor.byte {
            let (line, col) = cursor_position(text, byte);
            ui.weak(format!("行 {line}:{col}"));
        }
        ui.weak(format!("{} 字", text.chars().count()));
        separator(ui);
        ui.weak(state.theme.mode.label());
        separator(ui);
        let ai = if state.ai.is_streaming() {
            format!("{} · 生成中", state.ai.provider_label())
        } else {
            state.ai.provider_label().to_owned()
        };
        ui.weak(ai);
        separator(ui);
        // MCP:关闭时只写「关」,开启才展开端点(状态栏是窄条,不堆信息)
        match &state.mcp.status {
            crate::mcp::McpStatus::Listening(port) => {
                ui.weak(format!("MCP: 127.0.0.1:{port}"));
            }
            crate::mcp::McpStatus::Failed(_) => {
                ui.colored_label(crate::ui::tokens::WARN, "MCP: 启动失败");
            }
            crate::mcp::McpStatus::Starting => {
                ui.weak("MCP: 启动中");
            }
            crate::mcp::McpStatus::Disabled => {
                ui.weak("MCP: 关");
            }
        }
    });
}

fn separator(ui: &mut egui::Ui) {
    ui.weak("·");
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
/// 光标行列(1 起):行按换行数,列按该行字符数(中文按字计,与编辑器
/// 的视觉列一致)。
///
/// `byte` 可能是**过期快照**:格式动作在归约侧整篇替换文本,而本函数在
/// 同一帧的绘制序里先于编辑器跑,拿到的还是按旧文本折出的字节(状态栏
/// 2026-09-27 实测崩溃:「byte index not a char boundary」)。收缩到字符
/// 边界而非钳长,越界与非边界一并兜住。
fn cursor_position(text: &str, byte: usize) -> (usize, usize) {
    let byte = text.floor_char_boundary(byte.min(text.len()));
    let before = &text[..byte];
    let line = before.matches('\n').count() + 1;
    let col = before.chars().rev().take_while(|ch| *ch != '\n').count() + 1;
    (line, col)
}

/// commit message 建议浮窗;返回(复制, 关闭)按钮的响应,测试定位用
/// (与 `ui::menubar::item` 同款手法)。
///
/// 复制即时写系统剪贴板(`Context::copy_text`,UI 侧效果,不改状态——
/// 不经用户动作覆盖剪贴板会冲掉用户正在搬运的内容);关闭只发消息,
/// 清建议的归约在 `App::logic`。
fn commit_dialog(ui: &mut egui::Ui, subject: &str) -> (egui::Response, egui::Response) {
    let ctx = ui.ctx().clone();
    let mut buttons = None;
    egui::Window::new("AI: commit message")
        // 固定初始位置:浮窗出现位置可预期(不与菜单栏重叠),拖动后由
        // Area 记忆保持;显式初始位也让无头测试的帧间位置稳定。
        .default_pos([80.0, 120.0])
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            ui.label("建议的 commit subject:");
            ui.label(egui::RichText::new(subject).strong());
            ui.horizontal(|ui| {
                let copy = ui.button("复制");
                if copy.clicked() {
                    ctx.copy_text(subject.to_owned());
                }
                buttons = Some((copy, ui.button("关闭")));
            });
        });
    buttons.expect("浮窗必然绘制按钮")
}

/// 脏标签关闭确认浮窗;返回(确认关闭, 取消)按钮的响应,测试定位用
/// (与 `checkout_dialog` 同款手法;真正的移除在归约)。
fn tab_close_dialog(ui: &mut egui::Ui, name: &str) -> (egui::Response, egui::Response) {
    let mut buttons = None;
    egui::Window::new("关闭标签")
        .default_pos([80.0, 120.0])
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            ui.label(format!("「{name}」有未保存的修改。"));
            ui.label(
                egui::RichText::new("关闭将丢弃这些修改,此操作不可撤销。")
                    .strong()
                    .color(crate::ui::tokens::DANGER),
            );
            ui.horizontal(|ui| {
                let confirm = ui.button("关闭并丢弃");
                let cancel = ui.button("取消");
                buttons = Some((confirm, cancel));
            });
        });
    buttons.expect("浮窗必然绘制按钮")
}

/// 回滚确认浮窗;返回(回滚, 取消)按钮的响应,测试定位用(与
/// `commit_dialog` 同款手法)。只展示与收集点击,checkout 在归约。
/// `open_in_editor`/`dirty` 驱动当前文档的针对性警示
/// ([`checkout_extra_warning`]),dirty 状态取自缓冲真源、每帧重估。
fn checkout_dialog(
    ui: &mut egui::Ui,
    path: &str,
    open_in_editor: bool,
    dirty: bool,
) -> (egui::Response, egui::Response) {
    let mut buttons = None;
    egui::Window::new("Git: 回滚文件")
        .default_pos([80.0, 120.0])
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            ui.label(format!("把 {path} 恢复到 HEAD 版本。"));
            ui.label(
                egui::RichText::new("未提交的改动将被丢弃,此操作不可撤销。")
                    .strong()
                    .color(crate::ui::tokens::DANGER),
            );
            if let Some(warning) = checkout_extra_warning(open_in_editor, dirty) {
                let text = egui::RichText::new(warning);
                // dirty 分支有真实损失(保存会反转回滚),黄色升级警示;
                // 非 dirty 只是行为告知,走默认前景
                let text = if dirty {
                    text.color(egui::Color32::from_rgb(235, 180, 60))
                } else {
                    text
                };
                ui.label(text);
            }
            ui.horizontal(|ui| {
                let confirm = ui.button("回滚");
                let cancel = ui.button("取消");
                buttons = Some((confirm, cancel));
            });
        });
    buttons.expect("浮窗必然绘制按钮")
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
        self.reduce(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state;
    use egui::{
        Event, FullOutput, Key, Modifiers, OutputCommand, PointerButton, Pos2, RawInput, Rect,
        ViewportCommand,
    };
    use std::cell::Cell;
    use std::rc::Rc;

    fn key_s(modifiers: Modifiers) -> Event {
        Event::Key {
            key: Key::S,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
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

    /// 拖入图片文件的**帧级**链路(D 段):`raw.dropped_files` 里白名单内的
    /// 图片进归约(`ImageFileDropped`),白名单外的(.md)不进 —— egui 是
    /// 全窗口收文件,拖 .md 的语义是「打开文件」而不是插图,过滤发生在
    /// layout 侧。归约之后的落盘与插入由 state::tests 钉住。
    #[test]
    fn dropped_whitelisted_image_enters_reduction_others_do_not() {
        let dir = std::env::temp_dir().join(format!("latermd-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("笔记.md");
        std::fs::write(&doc, "# x").unwrap();
        let image = dir.join("图.png");
        std::fs::write(&image, b"png").unwrap();
        let markdown = dir.join("别的.md");
        std::fs::write(&markdown, b"# y").unwrap();
        let mut app = LaterMdApp::default();
        app.state.tabs.current_mut().document.path = Some(doc.clone());

        // 白名单外的 .md:被 layout 过滤,文档不动(拖 .md 开文件属
        // 将来的拖开标签,不是本段语义)
        reduce_with_dropped(&mut app, vec![markdown]);
        assert_eq!(
            app.state.tabs.current().editor.text(),
            state::State::default().tabs.current().editor.text(),
            "非图片不进归约"
        );
        assert_eq!(app.state.tabs.current().document.notice, None);

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
        assert_eq!(reduce(&mut app, Vec::new()), vec!["LaterMD — 未命名"]);
        app.state.tabs.current_mut().editor.insert_chars(0, "改动");
        assert_eq!(reduce(&mut app, Vec::new()), vec!["LaterMD — 未命名*"]);

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
        // 本测试只量搜索去抖的 repaint；标记非 Git 根，避免合法的 3 秒
        // Git 定时刷新混入 viewport delay。
        app.state.git.error = Some("测试中停用 Git 轮询".to_owned());
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

        // 帧 1:单独渲染浮窗拿按钮位置(Area 按 id 记忆,draw 内位置一致)
        ctx.run_ui(
            RawInput {
                screen_rect: Some(test_viewport()),
                ..Default::default()
            },
            |ui| {
                let (copy, close) = commit_dialog(ui, "docs: 新增README.md");
                rects.set((copy.rect, close.rect));
            },
        )
        .drop_without_applying_deltas();

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

    /// Git 轮询在归约侧到点即发起后台单飞；结果线程唤醒后由后续 reduce
    /// 换入。无根时到点也不启动任务(不空转)。
    #[test]
    fn git_poll_refreshes_when_due_with_root() {
        let dir = dirty_repo("git-poll");
        let mut app = LaterMdApp::default();
        app.state.file_tree.root = Some(dir.clone());
        app.state.git.refresh_due = std::time::Instant::now() - std::time::Duration::from_millis(1);

        let ctx = egui::Context::default();
        let output = ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()));
        output.drop_without_applying_deltas();
        assert!(app.state.git.is_refreshing(), "到点帧已发起后台刷新");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.state.git.is_refreshing() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(2));
            let output = ctx.run_ui(RawInput::default(), |ui| app.reduce(ui.ctx()));
            output.drop_without_applying_deltas();
        }
        assert_eq!(app.state.git.entries.len(), 1, "后台结果已换入");
        assert!(!app.state.git.due(), "刷新顺延了下一周期");

        // 无根 + 到点:不刷(无根不轮询)
        let mut rootless = LaterMdApp::default();
        rootless.state.git.refresh_due =
            std::time::Instant::now() - std::time::Duration::from_millis(1);
        let output = ctx.run_ui(RawInput::default(), |ui| rootless.reduce(ui.ctx()));
        output.drop_without_applying_deltas();
        assert_eq!(rootless.state.git.entries.len(), 0);

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

        // 帧 1:渲染浮窗拿按钮位置(Area 按 id 记忆,draw 内位置一致)
        ctx.run_ui(RawInput::default(), |ui| {
            let (confirm, cancel) = checkout_dialog(ui, "a.md", false, false);
            rects.set((confirm.rect, cancel.rect));
        })
        .drop_without_applying_deltas();

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

    /// 自绘标题栏的「关闭右侧」按钮在**完整三栏 draw** 路径下真的能点到,
    /// 且翻转的是右栏而非左栏(M1 验收点)。
    ///
    /// 走 `LaterMdApp::draw`(而非单独渲 `titlebar::ui`)是有意的:三栏重排后
    /// 新加的 `Panel::right("preview")` 与边缘缩放命令区挤在同一命中层里,
    /// 只测孤立标题栏会漏掉「谁抢走了这次点击」这类回归。
    #[test]
    fn titlebar_right_button_toggles_only_the_right_panel() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        let mut app = LaterMdApp {
            frameless: true,
            ..Default::default()
        };
        assert!(
            app.state.layout.left && app.state.layout.right,
            "出厂三栏全开"
        );

        let bar = egui::Rect::from_min_max(
            screen.left_top(),
            screen.left_top() + egui::vec2(screen.width(), crate::ui::tokens::TITLEBAR_H),
        );
        // 六个按钮从左至右:`TITLE_BUTTONS` 顺序,右栏是第 2 个(下标 1)
        let center = crate::ui::titlebar::button_rects(bar)[1].center();
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

        // 面板层前几遍为 sizing pass,widget 尚不参与命中测试
        frame(&mut app, Vec::new());
        frame(&mut app, Vec::new());
        frame(&mut app, vec![Event::PointerMoved(center)]);
        frame(&mut app, vec![click(true)]);
        frame(&mut app, vec![click(false)]);
        assert_eq!(app.outbox, vec![Message::RightPanelToggled]);

        // 归约在下一帧:只翻右栏,左栏纹丝不动
        ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| app.reduce(ui.ctx()),
        )
        .drop_without_applying_deltas();
        assert!(
            app.state.layout.left && !app.state.layout.right,
            "只收右栏,左栏保持:left={} right={}",
            app.state.layout.left,
            app.state.layout.right
        );
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

    /// M5 收口(2026-09-27 修复用户实测回归):非禅定态的 panel 序列恰为
    /// titlebar / menubar / statusbar / nav / preview + 中央编辑器。
    ///
    /// 三条取证分别对应三条反馈:
    /// - 顺序:标题栏最顶、菜单栏次之、状态栏贴底(文本矩形自上而下);
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
        let menubar = topmost_text(&shapes, "文件");
        let status = topmost_text(&shapes, "248 字");
        let nav = topmost_text(&shapes, "未选择根目录");

        // 自上而下:标题栏 → 菜单栏(都在自己那条带里)
        assert!(
            title.top() < crate::ui::tokens::TITLEBAR_H,
            "标题栏最顶:{title:?}"
        );
        assert!(
            menubar.top() > crate::ui::tokens::TITLEBAR_H && menubar.top() < 100.0,
            "菜单栏紧随标题栏:{menubar:?}"
        );
        // 状态栏贴底且横跨:文本 x 落在左栏宽度以内(左栏 default 240)
        assert!(
            status.top() > screen.height() - 40.0,
            "状态栏贴底:{status:?}"
        );
        assert!(
            status.left() < 100.0,
            "状态栏横跨全窗底部(文本须在左栏脚下):{status:?}"
        );
        // 左栏在左,右缘不超过 default 宽度(240)放一点余量
        assert!(
            nav.left() < 240.0 && nav.right() < 260.0,
            "左栏在左侧:{nav:?}"
        );

        // 无黑条:menubar 与 statusbar 之间的中央高度上,从左栏右缘到
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

    /// 任务列表崩溃回归(2026-09-27 用户实测「点几次就崩溃」)。
    ///
    /// 全链路:真实 `reduce`+`draw`、真实格式条 Task 按钮、CJK 文本行中
    /// 光标,三态循环两整圈(六次点击)。崩溃机制:格式归约整篇替换文本
    /// 后,状态栏(绘制序先于编辑器)拿按**旧文本**折出的 `cursor.byte`
    /// 切**新文本**,`byte index not a char boundary` panic —— 修复在
    /// `cursor_position` 的边界收缩与 `apply_format` 的字节重折算。
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
    #[test]
    fn cursor_position_tolerates_stale_mid_char_bytes() {
        let text = "- [ ] 纯中文行\n第二行乙";
        // 字节 14 落在 '文'(12..15)中间 —— 崩溃帧的实值;收缩到 '中'
        // 之后(字节 12),按字符计列
        let (line, col) = cursor_position(text, 14);
        assert_eq!((line, col), (1, 9));
        // 越界钳制到文末仍是合法行为
        let (line, _) = cursor_position(text, 10_000);
        assert_eq!(line, 2);
        // 正常路径不变
        assert_eq!(cursor_position("abc", 2), (1, 3));
        assert_eq!(cursor_position("甲乙\n丙", 7), (2, 1), "第二行行首");
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
            zen.iter().any(|t| t.contains("未命名")),
            "窗口 chrome 保留:{zen:?}"
        );
        // menubar / 状态栏 / 侧边栏 / 格式工具条的专属文案全部消失
        for gone in ["文件", "248 字", "未选择根目录", "无序列表"] {
            assert!(
                !zen.iter().any(|t| t.contains(gone)),
                "禅定帧里不该出现 {gone:?}:{zen:?}"
            );
        }
        // 取证信号都取「只有那一条 panel 才会画」的专属文案:
        // 「文件」= menubar 首项、「248 字」= statusbar 的字数统计、
        // 「未选择根目录」= 左栏文件树、「无序列表」= 编辑器上方格式工具条
        // 的按钮 tooltip(只在 hover 时才画;改用按钮本体自绘的「H1」字形
        // 文案 —— 它只由格式工具条的 rich 按钮画出)
        for present in ["文件", "248 字", "未选择根目录", "H1"] {
            assert!(
                three.iter().any(|t| t.contains(present)),
                "取证有效:三栏帧里能找到 {present:?}"
            );
        }
        // 正文仍然渲染在同一个 PreviewState 上(禅定不是换渲染器)
        assert!(!zen.is_empty(), "禅定帧仍有内容被绘制:{zen:?}");
        assert!(app.state.layout.zen, "绘制不翻转禅定");
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
        assert!(app.state.layout.left && app.state.layout.right, "三栏还原");
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
        assert!(!app.state.layout.left && app.state.layout.right);

        app.state.apply(Message::ZenToggled);
        draw_frames(&mut app, &ctx, screen, 2);
        app.state.apply(Message::ZenToggled);
        assert!(
            !app.state.layout.left && app.state.layout.right,
            "逐项还原:left={} right={}",
            app.state.layout.left,
            app.state.layout.right
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
                    // 每次只切一次模式,切换后从下一帧起观察淡入
                    if step == 0
                        && mode_live != (app.state.render_mode == crate::live::RenderMode::Live)
                    {
                        app.state.apply(Message::ToggleLivePreview);
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
}
