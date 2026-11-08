//! Markdown 格式工具条(docs/ui-shell-redesign.md §6)。
//!
//! 十七个动作分四组:行内 / 标题 / 块 / 列表。点击只发
//! [`Message::FormatRequested`],语义全在 [`crate::compose`] —— 本模块
//! 一行 Markdown 逻辑都没有。
//!
//! ## 直出与溢出按宽度分档(2026-10-10)
//!
//! S2-2 曾把标题/块两组无条件收进「更多」;现按可用宽度三档升降
//! ([`tier_plan`]):宽屏四组全直出、不画「更多」;中档收块组;窄档
//! 收标题+块组(S2-2 原状)。收进来的组与直出按钮共用同一个
//! [`button`],点击语义零分叉。
//!
//! 唯一的例外是**图片**:它要 alt 与 url 两个输入,点了是开「图片框」对话
//! 框(发 [`Message::ImageDialogOpened`]),不是改文档(docs/image-plan.md
//! A 段)。
//!
//! ## 形态选择
//!
//! 加粗/斜体/删除线/H1-H3 用 **RichText**(`B` / `I` / `S` / `H1` 等)
//! 而不是自绘图标:这三个是「形态」抽象概念,线段自绘只能画出没有辨识度
//! 的矩形;而 `RichText::italics()/strikethrough()` 与 Inter SemiBold
//! 字重(U1,`fonts::semibold_family`)是 egui 内建富文本能力,零字形
//! 依赖风险(§6.1)。
//! 其余用自绘线段图标,遵守 ui-polish §1.1「图标是矢量自绘,不是字体字符」。
//!
//! ## 为什么要单独一条
//!
//! 文件动作收在左栏顶段(`ui::sidebar` 的 top_actions),本条是「对**选
//! 区**做文本级动作」,与文件级动作分居两处,心智模型不同;单独一条也让
//! 十七个按钮有完整的横向空间,不与谁挤。

use crate::command::Command;
use crate::compose::{FormatAction, FormatGroup};
use crate::keymap::Keymap;
use crate::state::Message;
use crate::ui::icons::Icon;
use crate::ui::tokens;
use eframe::egui;

/// 行内元素的横向间距(与 `ui_with_probe` 里 `item_spacing.x` 同源)。
const GAP: f32 = 2.0;
/// 分隔线的占位宽度(`separator` 的 `allocate_exact_size`)。
const SEP_W: f32 = 9.0;
/// 图标按钮的边长(`workbench::small_icon` 的 `allocate_exact_size`)。
const ICON_BTN: f32 = 28.0;
/// 「更多」菜单按钮与形态按钮共用的横向内距(`button_padding.x`)。
const BUTTON_PAD_X: f32 = 8.0;
/// 档位演算的安全余量:浮点累加零头 + 菜单按钮实测宽度与 galley 估算
/// 之间的毛刺。宁可早一档收拢,不让最后一枚按钮溢出可视区。
const SAFETY: f32 = 8.0;

/// 中间档的直出组:块组(引用/代码块/分割线/表格/图片,五项里最低频)
/// 收进「更多」,其余三组亮在行上。
const MID_DIRECT: [FormatGroup; 3] = [FormatGroup::Inline, FormatGroup::Heading, FormatGroup::List];
/// 中间档的溢出组:只有块组。
const MID_OVERFLOW: [FormatGroup; 1] = [FormatGroup::Block];

/// 绘制整条格式工具条。键位文案取自 `keymap`(用户可改)。
///
/// 高度恒定单行(30px):面板拖窄时**先逐档把低频组收进「更多」**
/// (`tier_plan`),实在放不下再水平滚动 —— 不换行。
pub fn ui(panel: &mut egui::Ui, keymap: &Keymap, outbox: &mut Vec<Message>) {
    ui_with_probe(
        panel,
        keymap,
        outbox,
        None::<fn(FormatAction, egui::Rect)>,
        None::<fn(egui::Rect)>,
    )
}

/// 同 [`ui`],额外把每个按钮的 `(动作, 矩形)` 交给 `probe`(`None` 即不探针)。
///
/// 无头测试量按钮位置用:十七个按钮的具体坐标由 `horizontal_wrapped` 的换
/// 行演算 + 它前面的标签条/提示行共同决定,手搓必然与真实帧错位。
/// `emoji_probe` 同理,量的是末尾那枚 Emoji 入口(不是 FormatAction,进
/// 不了 `probe` 的载荷)。
/// 点了某个格式动作按钮该发什么消息。
///
/// **图片是唯一「点了不改文档」的动作**:它要 alt 与 url 两个输入,点了
/// 是开「图片框」对话框(发 `ImageDialogOpened`),真正的写入走
/// `compose::insert_image`(docs/image-plan.md A 段)。
///
/// 直出按钮与溢出菜单里的条目**共用这一个函数** —— 两处若各写一份,
/// 迟早会漂移出「直出的图片发格式请求、菜单里的图片发对话框」这种
/// 方向相反的 bug(2026-10-08 S2-2 引入溢出菜单时的显式约束)。
fn request(action: FormatAction, outbox: &mut Vec<Message>) {
    outbox.push(if action == FormatAction::Image {
        Message::ImageDialogOpened
    } else {
        Message::FormatRequested(action)
    });
}

/// 按可用宽度选档:返回(**直出组**, **溢出组**)。
///
/// 2026-10-10 用户反馈「宽度够时把「更多」里的按钮都亮出来」:S2-2 把
/// 标题/块两组无条件收进菜单,宽屏(编辑区常见 700px+)下行上只剩 8 枚
/// 按钮,九个低频动作每次都要多点开一次菜单。改为三档升降:
///
/// | 档 | 直出 | 溢出 | 触发宽度(非 mac,实测演算) |
/// |---|---|---|---|
/// | 宽 | 全四组 17 项 | 无(不画「更多」) | ≥ ~535px |
/// | 中 | 行内+标题+列表 12 项 | 块组 | ≥ ~436px |
/// | 窄 | 行内+列表 8 项(S2-2 原状) | 标题+块组 | 更窄(滚动兜底) |
///
/// 降档顺序 Block → Heading:块组(引用/表格/分割线)是按段落语境才用
/// 的最低频动作,标题组次之 —— 与 S2-2「工具条只留高频」同一判据,只是
/// 把「无条件」放宽成「宽度不够时」。
///
/// 宽度取自面板、与工具条自身内容无关(高度恒定 30px,不构成反馈环),
/// 故不存在档位抖动;用户拖窗时档位随宽度单调变化。
fn tier_plan(panel: &egui::Ui) -> (&'static [FormatGroup], &'static [FormatGroup]) {
    let avail = panel.available_width() - SAFETY;
    let more_w = more_button_width(panel);
    if row_width(more_w, &FormatGroup::ALL, &[]) <= avail {
        (&FormatGroup::ALL, &[])
    } else if row_width(more_w, &MID_DIRECT, &MID_OVERFLOW) <= avail {
        (&MID_DIRECT, &MID_OVERFLOW)
    } else {
        (&FormatGroup::DIRECT, &FormatGroup::OVERFLOW)
    }
}

/// 一行(直出组 + 可选的「更多」+ 尾部 emoji 入口)的排版总宽。
///
/// 逐项复刻 `ui_with_probe` 里的 `allocate_exact_size` 尺寸与 `GAP` 间距
/// —— 两处必须同步改,否则档位会切早/切晚。最后一个元素的尾随间距不占宽。
fn row_width(more_w: f32, direct: &[FormatGroup], overflow: &[FormatGroup]) -> f32 {
    let mut width = 0.0;
    for group in direct {
        for action in group.actions() {
            width += action_width(*action) + GAP;
        }
    }
    if !overflow.is_empty() {
        width += SEP_W + GAP; // 菜单前的分隔线
        width += more_w + GAP; // 「更多」本体
    }
    width += SEP_W + GAP; // emoji 前的分隔线
    width += ICON_BTN + GAP; // emoji 本体
    width - GAP
}

/// 单个动作按钮的宽度(与 `button` 的 allocate 尺寸同源)。
fn action_width(action: FormatAction) -> f32 {
    match action {
        // 形态类走 `rich`:mac 28×28,其余 ICON+8
        FormatAction::Bold
        | FormatAction::Italic
        | FormatAction::Strike
        | FormatAction::H1
        | FormatAction::H2
        | FormatAction::H3 => {
            if cfg!(target_os = "macos") {
                28.0
            } else {
                tokens::ICON + 8.0
            }
        }
        // 其余走 `compact_icon` → `workbench::small_icon` 的 28×28
        _ => ICON_BTN,
    }
}

/// 「更多」的按钮文案。菜单绘制与档位演算**共用同一个构造**,规格漂移
/// 时两处一起错 —— 这是有意的同源约束。
fn more_label() -> egui::RichText {
    egui::RichText::new("更多").size(12.0)
}

/// 「更多」按钮的排版宽度(galley + 两侧 `button_padding`)。
///
/// `menu_button` 的真实宽度 egui 不外露,用同一 `more_label()` 规格自量
/// 一份;内距取 `BUTTON_PAD_X` 常量(与 `ui_with_probe` 里设置的一致)。
fn more_button_width(panel: &egui::Ui) -> f32 {
    let galley = egui::WidgetText::from(more_label()).into_galley(
        panel,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Body,
    );
    galley.size().x + 2.0 * BUTTON_PAD_X
}

pub fn ui_with_probe(
    panel: &mut egui::Ui,
    keymap: &Keymap,
    outbox: &mut Vec<Message>,
    probe: Option<impl FnMut(FormatAction, egui::Rect)>,
    emoji_probe: Option<impl FnMut(egui::Rect)>,
) {
    let mut probe = probe;
    let mut emoji_probe = emoji_probe;
    // 每一档的「直出 + 溢出」都必须**恰好**覆盖全部四组,不多不少、无
    // 重复。这条断言拦住的是「将来给 `FormatGroup` 加了第五组,却忘了
    // 放进某一档」—— 那会让第五组在**该档宽度下静默地从工具条消失**:
    // 不报编译错、菜单栏照常有它、只有常驻按钮不见了。2026-10-10 响应
    // 式改版后有三档,断言相应推广(原窄档单档版)。
    // `debug_assert!` 而非 `assert!`:一帧一次的 UI 绘制路径不该付
    // 运行时检查的钱,而 debug 构建(测试与开发)已经覆盖。
    for (direct, overflow) in [
        (&FormatGroup::ALL as &[_], &[] as &[_]),
        (&MID_DIRECT as &[_], &MID_OVERFLOW as &[_]),
        (&FormatGroup::DIRECT as &[_], &FormatGroup::OVERFLOW as &[_]),
    ] {
        let mut seen: Vec<FormatGroup> = Vec::new();
        debug_assert!(
            direct.len() + overflow.len() == FormatGroup::ALL.len()
                && direct
                    .iter()
                    .chain(overflow.iter())
                    .all(|g| FormatGroup::ALL.contains(g) && !seen.contains(g) && {
                        seen.push(*g);
                        true
                    }),
            "每档的 direct + overflow 必须恰好覆盖 FormatGroup::ALL(防新增组被遗忘/重复)"
        );
    }
    // 档位在 ScrollArea 之外演算:可用宽度取自面板(见 `tier_plan` 的
    // 无反馈环论证),子 Ui 里改 spacing 不会反过来影响它。
    let (direct, overflow) = tier_plan(panel);
    // 2026-10-08 症状 A(§6.3):原为 `horizontal_wrapped` **逐组换行**,
    // 900×600 下编辑区列仅 240px 而本条一行要 331px → 必然换行成两行
    // 63px,是顶部 chrome(共 128px)里最大的一项,且把编辑区往下顶。
    // 改为**水平滚动**,照 `ui/tabs.rs` 的同款做法(其注释已写明:
    // 「换行会让高度随按钮数成倍增长,把编辑区顶得上下跳」)——
    // 高度恒定 30px,与按钮数、面板宽窄无关。
    //
    // 代价如实记:240px 极端窄列落在**最窄档**,一行 331px 仍放不下,
    // 「更多」会滚出视野,九个低频动作里八个可经菜单栏「格式」触达,
    // `Plain` 只能水平滚回工具条的「更多」。判为可接受 —— 菜单栏「格式」
    // 收了 17 条命令(15 条 `Format*` + `ImageInsert` + `EmojiPicker`),
    // 而 `every_command_has_a_menu_entry` 的 44/44 守的是 `Command::ALL`
    // 全集,不是这 17 项;而 33px 的稳定高度收益是每天都吃得到的。
    // 2026-10-10 起中宽档先按 `tier_plan` 收拢,滚动只剩极端窄列一种情形。
    //
    // > 【订正 2026-11-08】旧句写「九个低频动作只能经菜单栏「格式」触达」+
    // > 「菜单栏本就收了全部 17 项(menu-coverage 44/44 守门),可发现性
    // > 不丢」。两处集合混淆:①44/44 遍历的是 `Command::ALL`,守「每条
    // > 命令恰好进一个菜单一次」,不作为「17 个格式动作全覆盖」的判据;
    // > ②17 是 `FORMAT_MENU` 的**命令条目数**,其中 `EmojiPicker` 不是
    // > `FormatAction`,而 `FormatAction::ALL` 的 17 个里 `Plain` 没有
    // > Command(`command_of` 返回 `None`),菜单栏「格式」里没有它 ——
    // > 故「九个都能经菜单栏触达」「可发现性不丢」对它不成立。此为该档位
    // > 的既有边界,本轮只订正措辞,不改行为(补 `Plain` 的命令/菜单入口
    // > 属新功能,不在本次范围)。
    egui::ScrollArea::horizontal()
        .id_salt("format-bar")
        .auto_shrink([false, true])
        .show(panel, |ui| {
            // 28pt 点击区 + 8px 按钮内距(2026-10-10 mac 精修全平台化,
            // #166/#169):与工作台头部/标签条同一点击档。横向内距与
            // `BUTTON_PAD_X` 同源,档位演算按它估「更多」的宽度。
            ui.spacing_mut().interact_size.y = 28.0;
            ui.spacing_mut().button_padding = egui::vec2(BUTTON_PAD_X, 4.0);
            ui.spacing_mut().extra_text_line_spacing = 0.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;

                // ① 直出组:档位决定亮几组(宽档全亮,见 `tier_plan`)。
                for group in direct {
                    for action in group.actions() {
                        let response = button(ui, *action, keymap);
                        if let Some(probe) = probe.as_mut() {
                            probe(*action, response.rect);
                        }
                        if response.clicked() {
                            request(*action, outbox);
                        }
                    }
                }

                // ② 溢出菜单:档位收进来的组(窄档=标题+块,中档=块;
                // 宽档为空,**不画「更多」**)。
                //
                // 菜单项**走同一个 `button()`**,故 tooltip 口径、图标画法、
                // 点击语义与直出完全一致;分段之间画分隔线(与菜单栏同款)。
                //
                // 「探针在菜单内也生效」是刻意的:无头测试据此在菜单打开后拿到条目
                // 矩形,否则菜单里的动作**没有任何测试能定位** —— 等于格式动作
                // 的点击路径在溢出后就失去覆盖。
                if !overflow.is_empty() {
                    separator(ui);
                    let menu = more_label().color(crate::theme::shell(ui).secondary);
                    ui.menu_button(menu, |ui| {
                        for (index, group) in overflow.iter().enumerate() {
                            if index > 0 {
                                ui.separator();
                            }
                            for action in group.actions() {
                                let response = button(ui, *action, keymap);
                                if let Some(probe) = probe.as_mut() {
                                    probe(*action, response.rect);
                                }
                                if response.clicked() {
                                    request(*action, outbox);
                                }
                            }
                        }
                    });
                }

                // ③ Emoji 面板入口(docs/emoji-plan.md E1):第二个「对话框类动作」,
                // 与 Image 同款 —— 点了只开面板,不进 `FormatAction` 四组(emoji
                // 字符无法从 text+sel 推导,§6.1 的边界)。它留在直出位:面板是
                // 「插入一个字符」的高频动作,与低频的段落级格式不同层。
                separator(ui);
                let response = compact_icon(ui, Icon::Emoji, &emoji_tooltip(keymap));
                if let Some(probe) = emoji_probe.as_mut() {
                    probe(response.rect);
                }
                if response.clicked() {
                    outbox.push(Message::EmojiPickerToggle(true));
                }
            });
        });
}

fn separator(ui: &mut egui::Ui) {
    // 细线分隔(28pt 档内居中 12px 短线),替代通高 egui separator
    let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, 28.0), egui::Sense::hover());
    ui.painter().line_segment(
        [
            rect.center() - egui::vec2(0.0, 6.0),
            rect.center() + egui::vec2(0.0, 6.0),
        ],
        crate::ui::workbench::separator(ui),
    );
}

fn compact_icon(ui: &mut egui::Ui, icon: Icon, tip: &str) -> egui::Response {
    // 28pt 方形小图标钮(带悬停底与辅助语义),与工具条/工作台同款
    crate::ui::workbench::small_icon(ui, icon, tip)
}

/// Emoji 按钮 tooltip(带当前键位,与动作按钮的 tooltip 同款口径)。
fn emoji_tooltip(keymap: &Keymap) -> String {
    match keymap.get(Command::EmojiPicker) {
        Some(shortcut) => format!("插入 Emoji({})", shortcut.platform_text()),
        None => "插入 Emoji".to_owned(),
    }
}

/// 单个按钮。返回响应以便测试定位(与 `ui::menubar::item` 同款手法)。
fn button(ui: &mut egui::Ui, action: FormatAction, keymap: &Keymap) -> egui::Response {
    match action {
        // 形态类走富文本:B / H1-H3 是抽象概念的自解释字形,字重取 Inter
        // SemiBold(U1,真实字重替代 strong() 的人造粗);I / S 叠加斜体 /
        // 删除线。其余一律自绘线段图标。
        FormatAction::Bold => rich(ui, action, "B", Glyph::Weight),
        FormatAction::Italic => rich(ui, action, "I", Glyph::Italic),
        FormatAction::Strike => rich(ui, action, "S", Glyph::Strike),
        FormatAction::H1 => rich(ui, action, "H1", Glyph::Weight),
        FormatAction::H2 => rich(ui, action, "H2", Glyph::Weight),
        FormatAction::H3 => rich(ui, action, "H3", Glyph::Weight),
        _ => icon_button(ui, action, keymap),
    }
}

/// 形态按钮的附加富文本形态(SemiBold 字重是 `rich` 内统一施加的底座)。
enum Glyph {
    /// 无附加形态:视觉重量全靠 SemiBold 字形。
    Weight,
    Italic,
    Strike,
}

/// 富文本按钮:`B` / `I` / `S` / `H1` 等。tooltip 带当前键位 —— 形态字形
/// 本身不解释自己,靠 tooltip 兜可读性(ui-polish §1.1 的同款要求)。
fn rich(ui: &mut egui::Ui, action: FormatAction, glyph: &str, shape: Glyph) -> egui::Response {
    // 原先取 `interact_size.y`(egui 出厂 18)当宽度;U0 投影后它涨到
    // INPUT_H=36,六个形态按钮共宽 108px,把 Task 挤过 horizontal_wrapped
    // 的换行点(实测按钮挪到第二行,task_button_cycling 测试点击落空)。
    // 形态按钮的宽度是**字形的排版需求**(ICON+8),不是「可交互最小高度」,
    // 后者只该作用于按钮高度 —— 高度这排恒取 FORMAT_BAR_H,本就与
    // interact_size 无关。改成只按排版宽度取值,U0 的高度投影不再外溢成
    // 宽度副作用。
    let width = if cfg!(target_os = "macos") {
        28.0
    } else {
        tokens::ICON + 8.0
    };
    let size = egui::vec2(
        width,
        if cfg!(target_os = "macos") {
            28.0
        } else {
            tokens::FORMAT_BAR_H
        },
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let enabled = ui.is_enabled();
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.hovered() && enabled {
            painter.rect_filled(
                rect,
                tokens::RADIUS_SM,
                ui.visuals().widgets.hovered.bg_fill,
            );
        }
        // 走 `WidgetText::into_galley` 而不是 `painter.text`:后者签名的
        // `impl ToString` 会把 RichText 降级成纯字符串,字重 / 斜体 /
        // 删除线在这一个转换里全丢 —— 加粗按钮于是长得跟普通按钮没两
        // 样,删除线干脆看不见。
        let mut text = egui::RichText::new(glyph)
            .size(if cfg!(target_os = "macos") {
                14.0
            } else {
                tokens::ICON_SM
            })
            .family(crate::fonts::semibold_family(ui.ctx()));
        if cfg!(target_os = "macos") {
            text = text.color(crate::theme::shell(ui).secondary);
        }
        match shape {
            Glyph::Weight => {}
            Glyph::Italic => text = text.italics(),
            Glyph::Strike => text = text.strikethrough(),
        }
        if !enabled {
            text = text.color(ui.visuals().weak_text_color());
        }
        let galley = egui::WidgetText::from(text).into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        );
        let pos = if cfg!(target_os = "macos") {
            egui::pos2(
                rect.center().x - galley.size().x / 2.0,
                rect.center().y - galley.mesh_bounds.center().y,
            )
        } else {
            egui::Align2::CENTER_CENTER
                .anchor_size(rect.center(), galley.size())
                .min
        };
        painter.galley(pos, galley, ui.visuals().text_color());
    }
    response.on_hover_text(command_tooltip(action))
}

/// 自绘图标按钮(形态抽象概念以外的动作)。tooltip 带当前键位 —— 这十六
/// 个动作没有别的常驻入口可以提示键位。
fn icon_button(ui: &mut egui::Ui, action: FormatAction, keymap: &Keymap) -> egui::Response {
    let label = action.label();
    let tooltip = match command_of(action).and_then(|cmd| keymap.get(cmd)) {
        Some(shortcut) => format!("{label}({})", shortcut.platform_text()),
        None => label.to_owned(),
    };
    compact_icon(ui, icon_of(action), &tooltip)
}

/// 动作 → 自绘图标。放 `ui` 层而不是 `compose`:后者发誓不碰 egui。
fn icon_of(action: FormatAction) -> Icon {
    use Icon::*;
    match action {
        FormatAction::InlineCode => CodeInline,
        FormatAction::Link => Link,
        FormatAction::Plain => Paragraph,
        FormatAction::Quote => Quote,
        FormatAction::CodeBlock => CodeBlock,
        FormatAction::Divider => Divider,
        FormatAction::Table => Table,
        FormatAction::Image => Image,
        // duplicate 不上工具条(键盘动作,menu/快捷键层即可),这里只为
        // match 穷尽;若将来要上,补自绘或映射既有图标
        FormatAction::DuplicateSelection | FormatAction::DuplicateLine => SaveAs,
        FormatAction::Bullet => BulletList,
        FormatAction::Ordered => OrderedList,
        FormatAction::Task => TaskList,
        // 形态类(B/I/S/H1-H3)走 RichText,不经过这里;写在这里只为让
        // match 穷尽:将来误把它们派到 `icon_button` 时,一眼能看出不对
        FormatAction::Bold
        | FormatAction::Italic
        | FormatAction::Strike
        | FormatAction::H1
        | FormatAction::H2
        | FormatAction::H3 => Close,
    }
}

/// 动作 → 命令(键位与图标都挂在命令层,工具条不另抄一份)。
///
/// 返回 `None` 表示只有工具条按钮、没有对应命令 —— 目前只有 `Plain`。
fn command_of(action: FormatAction) -> Option<Command> {
    use Command::*;
    Some(match action {
        FormatAction::Bold => FormatBold,
        FormatAction::Italic => FormatItalic,
        FormatAction::Strike => FormatStrike,
        FormatAction::InlineCode => FormatInlineCode,
        FormatAction::Link => FormatLink,
        FormatAction::H1 => FormatH1,
        FormatAction::H2 => FormatH2,
        FormatAction::H3 => FormatH3,
        // 「正文」(去前缀)只在工具条上有按钮:菜单不收它(§6.3 的工具条
        // 已经是它的家),也就没有 Command 与键位。
        FormatAction::Plain => return None,
        FormatAction::Quote => FormatQuote,
        FormatAction::CodeBlock => FormatCodeBlock,
        FormatAction::Divider => FormatDivider,
        FormatAction::Table => FormatTable,
        FormatAction::Image => ImageInsert,
        FormatAction::DuplicateSelection => DuplicateSelection,
        FormatAction::DuplicateLine => DuplicateLine,
        FormatAction::Bullet => FormatBullet,
        FormatAction::Ordered => FormatOrdered,
        FormatAction::Task => FormatTask,
    })
}

/// 悬浮提示:动作名(+ 键位,若该动作有命令且用户绑了键)。
fn command_tooltip(action: FormatAction) -> String {
    match command_of(action).and_then(|cmd| Keymap::builtin().get(cmd)) {
        Some(shortcut) => format!("{}({})", action.label(), shortcut.platform_text()),
        None => action.label().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, RawInput, Rect};
    use std::cell::Cell;

    /// `FormatAction::ALL` 里除 `Plain` 外的 16 个动作各有一个 command:
    /// 这是「键位与图标都挂在命令层」的守卫,也是 `Plain` 目前没有
    /// Command 这一事实的唯一登记处。
    ///
    /// > 【订正 2026-11-08】旧首句写「十七个动作各有一个 command」,与
    /// > 紧接的次句「`Plain` 没有 Command」及本测试自身
    /// > (`every_action_has_a_command_except_plain`)都自相矛盾。现按测试
    /// > 实际断言写:遍历 `FormatAction::ALL` 的 17 个,16 个有、`Plain`
    /// > 无。
    #[test]
    fn every_action_has_a_command_except_plain() {
        for action in FormatAction::ALL {
            if action == FormatAction::Plain {
                assert_eq!(command_of(action), None, "正文只在工具条上有按钮");
                continue;
            }
            assert!(command_of(action).is_some(), "{action:?} 缺 Command");
        }
    }

    /// 整条工具条渲染不 panic,且不自发消息(明暗两套 visuals 都过一遍)。
    #[test]
    fn bar_renders_without_messages() {
        for dark in [true, false] {
            let ctx = egui::Context::default();
            if !dark {
                ctx.set_theme(egui::Theme::Light);
            }
            let mut outbox = Vec::new();
            let output = ctx.run_ui(RawInput::default(), |ui| {
                super::ui(ui, &Keymap::builtin(), &mut outbox);
            });
            output.drop_without_applying_deltas();
            assert!(outbox.is_empty(), "仅渲染不产生消息");
        }
    }

    /// 点加粗按钮发 `FormatRequested(Bold)`。
    ///
    /// 走完整 `ui()` 而不是孤立 `button()`:十六个按钮挤在同一行、共享同一
    /// 命中层,只测孤立按钮会漏掉「相邻按钮抢走了这次点击」 —— M1 已经在
    /// 标题栏上踩过一次。
    #[test]
    fn clicking_first_button_requests_bold() {
        let ctx = egui::Context::default();
        let mut outbox = Vec::new();
        let rect = Cell::new(Rect::NOTHING);

        ctx.run_ui(RawInput::default(), |ui| {
            let response = button(ui, FormatAction::Bold, &Keymap::builtin());
            rect.set(response.rect);
        })
        .drop_without_applying_deltas();
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
                |ui| super::ui(ui, &Keymap::builtin(), &mut outbox),
            )
            .drop_without_applying_deltas();
        }
        assert_eq!(outbox, vec![Message::FormatRequested(FormatAction::Bold)]);
    }

    /// 点图片按钮是**开对话框**,不是发格式请求 —— 十七个动作里只有它走
    /// 这条路径(需要 alt 与 url 两个输入)。
    ///
    /// **2026-10-10 响应式档位后 Image 有两种住址**:宽屏下 Block 组直出
    /// (按钮就在行上,探针直接给矩形);窄屏下只存在于「更多」菜单
    /// (先点开菜单再探)。两条路径各点一遍 —— `request` 对两种住址发
    /// 同一种消息,但「直出命中」与「popup 命中」是两套点击链路,只测
    /// 一条会漏掉另一条的回归。
    #[test]
    fn clicking_image_requests_the_dialog_not_a_format() {
        for (screen_w, via_menu) in [(1400.0, false), (400.0, true)] {
            let ctx = egui::Context::default();
            let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(screen_w, 800.0));
            let mut outbox = Vec::new();
            let keymap = Keymap::builtin();
            let click_at = |pos: egui::Pos2, pressed| Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };

            let center = if via_menu {
                // —— 第 1 段:定位「更多」按钮并点开菜单(400px 下块组在
                // 菜单里;走 shapes 文本定位而非探针:菜单按钮本身不是
                // FormatAction,探针不覆盖它。菜单栏的同类测试已实证同一
                // 手法。)——
                let mut more_rect = None;
                for _ in 0..3 {
                    let output = ctx.run_ui(
                        RawInput {
                            screen_rect: Some(screen),
                            ..Default::default()
                        },
                        |ui| super::ui(ui, &keymap, &mut Vec::new()),
                    );
                    let shapes = output.shapes.clone();
                    output.drop_without_applying_deltas();
                    more_rect = shapes.iter().find_map(|clipped| {
                        let egui::epaint::Shape::Text(text) = &clipped.shape else {
                            return None;
                        };
                        (text.galley.job.text == "更多")
                            .then(|| clipped.shape.visual_bounding_rect())
                    });
                    if more_rect.is_some() {
                        break;
                    }
                }
                let more_center = more_rect
                    .unwrap_or_else(|| panic!("工具条上找不到「更多」按钮"))
                    .center();
                for events in [
                    Vec::new(),
                    vec![Event::PointerMoved(more_center)],
                    vec![click_at(more_center, true)],
                    vec![click_at(more_center, false)],
                ] {
                    ctx.run_ui(
                        RawInput {
                            events,
                            screen_rect: Some(screen),
                            ..Default::default()
                        },
                        |ui| super::ui(ui, &keymap, &mut Vec::new()),
                    )
                    .drop_without_applying_deltas();
                }

                // —— 第 2 段:菜单展开后取 Image 矩形 ——
                // egui 0.36 的 MenuButton 从「popup 记忆开态」到「闭包真正绘制
                // 条目」隔一帧(事件帧写记忆 → 次帧按钮收到开态 → 再次帧闭包执行),
                // 故最多试三帧,拿不到就明说而不是静默跳过(与 decisions-pending
                // #128 记录的 menubar 同款时序)。
                let rect = Cell::new(Rect::NOTHING);
                for _ in 0..3 {
                    let output = ctx.run_ui(
                        RawInput {
                            screen_rect: Some(screen),
                            ..Default::default()
                        },
                        |ui| {
                            super::ui_with_probe(
                                ui,
                                &keymap,
                                &mut Vec::new(),
                                Some(|action: FormatAction, button_rect: Rect| {
                                    if action == FormatAction::Image {
                                        rect.set(button_rect);
                                    }
                                }),
                                None::<fn(Rect)>,
                            );
                        },
                    );
                    output.drop_without_applying_deltas();
                    if rect.get() != Rect::NOTHING {
                        break;
                    }
                }
                assert!(
                    rect.get() != Rect::NOTHING,
                    "菜单展开后探针应拿到 Image 条目的位置"
                );
                rect.get().center()
            } else {
                // —— 宽屏直出:一帧探针直接给矩形,无需点开任何菜单 ——
                let rect = Cell::new(Rect::NOTHING);
                ctx.run_ui(
                    RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| {
                        super::ui_with_probe(
                            ui,
                            &keymap,
                            &mut Vec::new(),
                            Some(|action: FormatAction, button_rect: Rect| {
                                if action == FormatAction::Image {
                                    rect.set(button_rect);
                                }
                            }),
                            None::<fn(Rect)>,
                        );
                    },
                )
                .drop_without_applying_deltas();
                assert!(
                    rect.get() != Rect::NOTHING,
                    "宽屏下 Image 应直出(探针应拿到矩形)"
                );
                rect.get().center()
            };

            // —— 第 3 段:点它 ——
            for events in [
                vec![Event::PointerMoved(center)],
                vec![click_at(center, true)],
                vec![click_at(center, false)],
            ] {
                ctx.run_ui(
                    RawInput {
                        events,
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| super::ui(ui, &keymap, &mut outbox),
                )
                .drop_without_applying_deltas();
            }
            assert_eq!(
                outbox,
                vec![Message::ImageDialogOpened],
                "屏宽 {screen_w}px 下点图片仍应开对话框"
            );
        }
    }

    /// **三档各自恰好覆盖全部四组**(2026-10-08 S2-2 守门;2026-10-10
    /// 响应式改版推广到宽/中/窄三档)。
    ///
    /// 拦的是「将来给 `FormatGroup` 加了第五组,却忘了放进某一档」——
    /// 那会让新组在该档宽度下**静默从工具条消失**:不报编译错、菜单栏
    /// 照常有它、只有常驻按钮不见了。生产路径有同款 `debug_assert!`,
    /// 本测试是它的可读版本(且在 release 下也跑)。
    #[test]
    fn every_tier_covers_every_group_exactly_once() {
        let tiers: [(&[FormatGroup], &[FormatGroup]); 3] = [
            (&FormatGroup::ALL, &[]),
            (&MID_DIRECT, &MID_OVERFLOW),
            (&FormatGroup::DIRECT, &FormatGroup::OVERFLOW),
        ];
        for (direct, overflow) in tiers {
            assert_eq!(
                direct.len() + overflow.len(),
                FormatGroup::ALL.len(),
                "每档两组之和须等于全部组数(direct={direct:?} overflow={overflow:?})"
            );
            let mut seen: Vec<FormatGroup> = Vec::new();
            for group in direct.iter().chain(overflow.iter()) {
                assert!(FormatGroup::ALL.contains(group), "{group:?} 必须出自 ALL");
                assert!(
                    !seen.contains(group),
                    "{group:?} 在同一档里出现两次:{direct:?} + {overflow:?}"
                );
                seen.push(*group);
            }
            assert_eq!(seen.len(), FormatGroup::ALL.len(), "四组一个不能少");
        }
    }

    /// 最窄档的直出位是行内 + 列表两组(2026-10-08 S2-2 的**收益断言**,
    /// 档位化后收窄为「最窄档构成」守门)。
    ///
    /// 锁的是「工具条只留高频」这条 ui-polish §1.2 原则。列表组在列,
    /// 不是因为它高频,而是因为 `Task` 是唯一的**多步交互**(三态循环)——
    /// 进菜单会让「一次状态切换」从 3 次点击涨到 6 次,详见
    /// `FormatGroup::DIRECT` 的文档。
    ///
    /// 哪天有人把标题 / 块组挪回最窄档的直出位(看起来「更方便」),本
    /// 测试变红;中/宽档的构成由 `direct_set_grows_with_available_width`
    /// 行为级守住。
    #[test]
    fn narrowest_tier_direct_row_is_inline_plus_list_only() {
        // **比切片而不是定长数组**:数组长度一变(如把 Heading 挪进
        // DIRECT),`assert_eq!([T; 3], [T; 2])` 是**编译错误**而非断言
        // 失败 —— 守门测试若以「编不过」的方式拦回归,就不是守门测试,而是
        // 一根会误伤正确改动的拦路桩(首版就踩了这个)。
        assert_eq!(
            FormatGroup::DIRECT.as_slice(),
            &[FormatGroup::Inline, FormatGroup::List],
            "最窄档直出组应是行内 + 列表(列表因 Task 三态循环而必须直出)"
        );
        assert_eq!(
            FormatGroup::OVERFLOW.as_slice(),
            &[FormatGroup::Heading, FormatGroup::Block],
            "最窄档溢出组应是标题 + 块"
        );
        // 收益量化:17 项里 8 项直出、9 项进菜单,加「更多」与 emoji 共
        // 10 个可见槽位(改版前 18 个)。
        let direct: usize = FormatGroup::DIRECT.iter().map(|g| g.actions().len()).sum();
        let overflow: usize = FormatGroup::OVERFLOW
            .iter()
            .map(|g| g.actions().len())
            .sum();
        assert_eq!(
            direct + overflow,
            FormatAction::ALL.len(),
            "17 项一个不能少"
        );
        assert_eq!(direct, 8, "直出 8 项(行内 5 + 列表 3)");
        assert_eq!(overflow, 9, "溢出 9 项(标题 4 + 块 5)");
    }

    /// **档位随可用宽度升降**(2026-10-10「宽度够就把「更多」里的按钮
    /// 都亮出来」的行为级守门)。
    ///
    /// | 根 Ui 宽 | 直出 | 「更多」 |
    /// |---|---|---|
    /// | 1400 | 17(全四组) | 无 |
    /// | 500 | 12(行内+标题+列表,块组进菜单) | 有 |
    /// | 300 | 8(行内+列表,S2-2 原状) | 有 |
    ///
    /// 探针只对**本帧画出的按钮**触发(菜单闭着时菜单条目不画),故一帧
    /// 的探针集合就是直出集合;「更多」在场与否用 shapes 文本取证(与
    /// 图片测试第 1 段同一手法)。中档使用 500px，给 macOS 形态按钮
    /// (28 vs 24)、根 Ui 边距及「更多」文字宽度留足空间，仍小于全展开阈值。
    #[test]
    fn direct_set_grows_with_available_width() {
        use std::collections::HashSet;
        for (width, direct_expected, more_expected) in
            [(1400.0, 17, false), (500.0, 12, true), (300.0, 8, true)]
        {
            let ctx = egui::Context::default();
            let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 400.0));
            let mut seen: Vec<FormatAction> = Vec::new();
            let mut has_more = false;
            for _ in 0..2 {
                let output = ctx.run_ui(
                    RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| {
                        super::ui_with_probe(
                            ui,
                            &Keymap::builtin(),
                            &mut Vec::new(),
                            Some(|action: FormatAction, _: Rect| seen.push(action)),
                            None::<fn(Rect)>,
                        );
                    },
                );
                has_more = output.shapes.iter().any(|clipped| {
                    matches!(
                        &clipped.shape,
                        egui::epaint::Shape::Text(t) if t.galley.job.text == "更多"
                    )
                });
                output.drop_without_applying_deltas();
            }
            let distinct: HashSet<_> = seen.iter().collect();
            assert_eq!(
                distinct.len(),
                direct_expected,
                "{width}px 下直出按钮数不符:{seen:?}"
            );
            assert_eq!(
                has_more, more_expected,
                "{width}px 下「更多」按钮的在场性不符"
            );
        }
    }

    /// 点笑脸按钮是**开 Emoji 面板**,不是发格式请求:第二个「对话框类
    /// 动作」(docs/emoji-plan.md E1),不进 FormatAction 四组。位置经
    /// emoji 探针在同一条真实工具条布局里取(与图片测试同一手法)。
    #[test]
    fn clicking_emoji_opens_the_picker_not_a_format() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0));
        let mut outbox = Vec::new();
        let rect = Cell::new(Rect::NOTHING);

        ctx.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                super::ui_with_probe(
                    ui,
                    &Keymap::builtin(),
                    &mut Vec::new(),
                    None::<fn(FormatAction, Rect)>,
                    Some(|button_rect: Rect| rect.set(button_rect)),
                );
            },
        )
        .drop_without_applying_deltas();
        let center = rect.get().center();
        assert!(center.x > 0.0, "探针拿到了 Emoji 按钮的位置:{center:?}");
        let click = |pressed| Event::PointerButton {
            pos: center,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };

        for events in [
            Vec::new(),
            Vec::new(),
            vec![Event::PointerMoved(center)],
            vec![click(true)],
            vec![click(false)],
        ] {
            ctx.run_ui(
                RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| super::ui(ui, &Keymap::builtin(), &mut outbox),
            )
            .drop_without_applying_deltas();
        }
        assert_eq!(outbox, vec![Message::EmojiPickerToggle(true)]);
    }

    /// 自绘图标基本不重合。十七个动作里六个形态类走 RichText(不经
    /// `icon_of`),余下十一个各有各的画面。
    #[test]
    fn icons_are_distinct_per_action() {
        use std::collections::HashSet;
        let seen: HashSet<String> = FormatAction::ALL
            .iter()
            .map(|action| format!("{:?}", icon_of(*action)))
            .collect();
        // 6 个形态类共用一个占位值 + 11 个自绘各一枚 = 12 枚;任何两枚被
        // 借来借去都会掉到 11 以下(`Plain` 曾借 `Table` 的旧账)。
        assert_eq!(
            seen.len(),
            12,
            "图标疑似复用(除形态类占位外不应有重码):{:?}",
            seen.iter().collect::<Vec<_>>()
        );
    }
}
