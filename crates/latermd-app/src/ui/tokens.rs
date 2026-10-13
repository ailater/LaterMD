//! 界面设计 token(docs/ui-polish.md §2)。
//!
//! 只做**语义级**常量:间距、尺寸、圆角与四个语义色。不做每控件样式树
//! (roadmap 专题「明确不做」),也不做序列化 —— 皮肤文件(阶段 4.5 批次 B)
//! 才需要 serde 结构,本轮常量足够。
//!
//! 落在这里而非散在各 UI 模块,是为了让「工具栏按钮高度」这类数字只有
//! 一个真源;改一个 token 即全界面跟随。

use eframe::egui::{self, Color32};

// —— 间距 ——

/// 图标与文字的间隙。
pub const SPACE_XS: f32 = 4.0;
/// 按钮内边距(水平)。
pub const SPACE_SM: f32 = 6.0;
/// 工具栏分组间距。
pub const SPACE_MD: f32 = 10.0;

// —— 尺寸 ——

/// 图标方框边长(按钮内)。
pub const ICON: f32 = 16.0;
/// 图标边长(页签等紧凑位)。
pub const ICON_SM: f32 = 13.0;
/// 工具栏按钮高度。
pub const TOOLBAR_H: f32 = 28.0;
/// Markdown 格式工具条高度(docs/ui-shell-redesign.md §11)。与 `TOOLBAR_H`
/// 同一量级:两条都在编辑区顶部,高度差一眼可见但不大。
pub const FORMAT_BAR_H: f32 = 30.0;
/// 自绘标题栏高度(docs/ui-shell-redesign.md §11,无边框模式才有)。
pub const TITLEBAR_H: f32 = 36.0;
/// 标题栏左上角品牌标识边长(#74)。
///
/// 比 [`ICON`] 略大(16→18):方角彩色标识在 16px 下「MD」两字母糊成一条,
/// 18px 是 smallest size 下仍能读出的档位(素材按 2x 上采样到 256 存,
/// 缩到这里不过采样)。标题栏 36px 高,18px 上下各留 9px,呼吸仍够。
pub const BRAND_LOGO: f32 = 18.0;
// —— 标题栏命令箱(docs/ui-shell-redesign-v2.md §5.6,2026-10-08 新增)——

/// 标题栏右端窗口按钮枚数(与 [`crate::ui::titlebar::TITLE_BUTTONS`] 同
/// 源;`WINDOW_BTN.x` 乘它得整排宽。两者不同步时有断言钉住,见
/// `titlebar::tests::button_rects_tile_from_the_right_edge`)。
pub const TITLE_BUTTON_N: usize = 7;
/// 标题栏右侧窗口按钮命中区(Win 风整块,mac/Linux 同款统一)。
pub const WINDOW_BTN: egui::Vec2 = egui::Vec2::new(32.0, 24.0);
/// 标题栏右端「命令箱」的横向几何(docs/ui-shell-redesign-v2.md §5.6):
/// 搜索胶囊 + 源码/Live 切换,居中最右一组窗口按钮之左。
pub const TITLE_SEARCH_W: f32 = 260.0;
/// 源码/Live 两段切换的总宽(两段各 38)。
pub const TITLE_VIEW_W: f32 = 76.0;
/// 搜索胶囊与切换控件之间的间隙。
pub const TITLE_CMD_GAP: f32 = 6.0;
/// 命令箱总宽:= [`TITLE_SEARCH_W`] + [`TITLE_CMD_GAP`] + [`TITLE_VIEW_W`]
/// (346)。有断言钉住这个等式,改分量必须一起改。
pub const TITLE_CMD_W: f32 = TITLE_SEARCH_W + TITLE_CMD_GAP + TITLE_VIEW_W;
/// 标题栏搜索胶囊的占位提示。旧值「搜索 / 跳转…」名不副实(胶囊只做
/// 全文搜索,跳转另有 Ctrl+G/查找浮层),2026-10-10 随 mac 精修改为
/// 实际用途(mac 工作台同款文案)。
pub const TITLE_SEARCH_HINT: &str = "搜索文档…";
/// 命令箱到最左那枚窗口按钮的留白(不与按钮贴死)。
pub const TITLE_CMD_TO_BTN: f32 = 8.0;
/// 命令箱控件的高度(胶囊 / 切换同高,垂直居中于 36px 标题栏)。
pub const TITLE_CMD_H: f32 = 24.0;
/// 七枚窗口按钮的总宽(= 7 × [`WINDOW_BTN`].x = 224)。
pub const TITLE_BUTTONS_W: f32 = TITLE_BUTTON_N as f32 * WINDOW_BTN.x;
/// 左段窗口标题的最小可见宽;窄于此则标题给命令箱让位(先截断 ellipsis,
/// 再整段不画)。264 的来历见 docs/ui-shell-redesign-v2.md §5.6。
pub const TITLE_TEXT_MIN_W: f32 = 264.0;
/// 窗口不再画命令箱的宽度阈值(docs/ui-shell-redesign-v2.md §5.6):
/// 命令箱 346 + 到七枚按钮 8 + 7×32 + 标题 await 264 = 842。
pub const TITLE_CMD_MAX_W: f32 =
    TITLE_CMD_W + TITLE_CMD_TO_BTN + TITLE_BUTTONS_W + TITLE_TEXT_MIN_W;
/// 左缘图标栏(rail)宽度(2026-10-08,docs/ui-shell-redesign-v2.md
/// §5.5)。画在**左栏内部**而非第四个 `Panel::left`:后者会让左栏总占宽
/// 240 → 288,省下的 31px 纵向 chrome 会以 48px 横向的形式赔回去。
///
/// **40 而非 48 的来历(2026-10-09 修订,坤哥截图反馈贴边距过宽)**:
/// 旧值 48 取自「28px 按钮 + 左右各留 10px,与 `WINDOW_BTN` 呼吸同级」。
/// 但那个 10px 在**工具栏里两侧都是内容、语义是「间隔」**;rail 的一侧
/// 是内容、另一侧是**窗口边界**,语义该是「贴边距」。沿用间隔的量会与
/// egui panel 默认 8px `inner_margin` 双层累加,实测图标距窗缘达 23px
/// (VS Code Activity Bar 约 12px)。故收至 40;按钮改定宽内缩
/// [`SPACE_XS`](2026-10-12 坤哥「再往左一点」,见 `rail_item_rect`)。
///
/// 取偶数便于图标在栏内 2px 级居中。
pub const RAIL_W: f32 = 40.0;
/// rail 内单枚按钮的边长(正方形)。径取 [`TOOLBAR_H`],档位与工具栏
/// 按钮同宽 —— 「和别处的图标按钮一样好点」就是这条取值的全部理由。
pub const RAIL_ITEM: f32 = TOOLBAR_H;
/// rail 内相邻两枚按钮的纵向间隙。比 `item_spacing.y`(3px)松一档,
/// 让上下两组之外还能读出「同一组内」的节奏。
pub const RAIL_GAP: f32 = 4.0;
/// rail 图标边长。比视图 tab 的 `NAV_TAB_ICON`(16) 大一档 —— rail 是
/// 纯图标导航,图标即全部内容(2026-10-12 坤哥反馈「再稍微大一点」)。
pub const RAIL_ICON: f32 = 18.0;
/// 左栏(导航)宽度下限(docs/ui-shell-redesign.md §11,R4):三栏旧下限
/// 160 是二分栏时代的数字,塞进四行视图导航后不够。
pub const SIDEBAR_MIN_W: f32 = 180.0;
/// 右栏(只读预览)初始宽度。
pub const PREVIEW_DEFAULT_W: f32 = 420.0;
/// 右栏宽度下限:再窄代码块与表格就只剩横向滚动了。
pub const PREVIEW_MIN_W: f32 = 260.0;
/// 预览左缘分隔线的**空闲强度**:线色朝面板底色(`content`)插值的比例。
///
/// **为什么是 lerp 而不是 `gamma_multiply`**(2026-10-13 守门测试实测):
/// `gamma_multiply` 是朝**黑**缩放,在浅色皮肤上「乘小一点」把线压得
/// 更暗 —— 于是空闲态(0.32)比 hover 态(0.72)还显眼,方向整个反过来,
/// 用户的直觉完全被误导。实测同一条线在浅色下空闲落差 8.7 灰阶、
/// hover 只有 1.3 灰阶(比空闲还淡)。朝底色插值则在深浅两套皮肤上都是
/// 「越远离底色越明显」,单一方向。
///
/// 0.55 即「往底色退一半」:能看出有一条界(实测落差 ~1.5 灰阶),又不把
/// 面板切成两半。
pub const PREVIEW_DIVIDER_IDLE: f32 = 0.55;
/// 同一条线的 hover / 拖动态:朝底色插值 0.15(几乎就是 `border` 本色)。
///
/// 空闲 0.55 → hover 0.15,实测落差从 ~1.5 拉到 ~7 灰阶,一档能分辨。
pub const PREVIEW_DIVIDER_HOT: f32 = 0.15;
/// 预览左缘的**拖动命中半径**(逻辑像素,向边两侧各取)。
///
/// egui 出厂 3.0(命中带宽 6px)。那 6px 落在面板底色与编辑区之间,
/// 没有任何视觉线索指向它,实测坤哥完全找不到可拖处(2026-10-13)。
/// 取 6.0(带宽 12px)与常显细线配套:线告诉「这里有条界」,命中区
/// 告诉「这里能抓」。
///
/// **只在预览面板那一帧局部抬高**,不改全局 `Style`:全局抬高会把
/// 同一个半径套到 nav 右缘,那里紧邻编辑区正文,凭空多出 6px 误吞
/// 点击的窗口(选中文字会失手)。
pub const PREVIEW_DIVIDER_GRAB: f32 = 6.0;
/// 状态栏右段(MCP/AI/主题)整段保留所需的最小条宽(2026-10-10,mac
/// 单行状态栏全平台化时的收编值)。窄于此则**右段从外往里逐项省略**
/// (MCP 恒留 —— 端口/告警最不可丢),左段文件名与明细随空间截断。
/// 取代三段时代的 `STATUSBAR_MIN_W`(240,随旧实现退役)。
pub const STATUSBAR_DROP_AUX_W: f32 = 640.0;
/// 左栏视图导航的行高(docs/ui-shell-redesign.md §11,M2 三段式)。
///
/// **2026-10-08 起废弃**:视图导航从竖排五行改为**单行图标 tab**
/// (docs/ui-shell-redesign-v2.md §2),高度改由 [`NAV_TAB_H`] 承担。
/// 保留常量不删:历史测试断言按名字引用它,删常量会让旧 commit 无法
/// `git bisect` 复现。**新代码不许再读这个值**。
pub const NAV_ROW_H: f32 = 26.0;
/// 左栏视图导航单行 tab 的高度(五视图横排一行,替代竖排五行)。
///
/// 26 刻意沿用旧 [`NAV_ROW_H`] 的值:16px 图标 + 上下各 5px 呼吸,与
/// 旧行高同高 → 左栏头部**纵向节奏不变**,只有 nav 段从 5×26=130px
/// 缩到 26px,省下的 104px 全给中段的文件树。改这个值等于改整条侧栏
/// 的头部高度。
pub const NAV_TAB_H: f32 = 26.0;
/// 视图 tab 里的图标边长(居中于 [`NAV_TAB_H`] 行内)。
pub const NAV_TAB_ICON: f32 = 16.0;
/// 相邻视图 tab 之间的间隙。
pub const NAV_TAB_GAP: f32 = 2.0;
/// 导航选中行的左侧竖条宽度(整行选中态的另一半;单行 tab 的选中态
/// 改为「下缘 2px 横条」,此值同时用于居中竖排时代的历史断言)。
pub const NAV_BAR_W: f32 = 2.0;
/// 禅定模式的正文限宽(docs/ui-shell-redesign.md §11;源出 ui-design.md
/// §1.2 的「沉浸」参数)。720 约合中文 40 字/行:再宽一行要横向扫读,
/// 再窄代码块与表格就得横向滚动了。
pub const ZEN_TEXT_W: f32 = 720.0;
/// 禅定模式下「退出禅定」浮层到内容区右上角的留白。贴死边缘会与「收起
/// 到边」的视觉直觉打架,也压住滚动条。
pub const ZEN_EXIT_MARGIN: f32 = 10.0;
/// 禅定模式内容区的四周留白(docs/ui-shell-redesign.md §7)。窗口够宽时
/// 正文在剩下的空间里居中;窗口窄于 `ZEN_TEXT_W + 2×gutter` 时由 720 限宽
/// 自己收缩,不至于逼出横向滚动。
///
/// **必须是整数**:`Frame::inner_margin` 最终落成 `Margin`(i8),`f32` 转过去
/// 会被 `round()` 静默吃掉小数。
pub const ZEN_GUTTER: f32 = 24.0;

// —— 控件几何(输入框档,U0 新增)——
//
// 来源:Armas / shadcn 的 h-9 / px-3 / py-2(docs/ui-modernization.md §2.4)。
// 只经 `theme::apply_shell` 投影进 egui 的两套 `Style`,不散落调用点。

/// 输入框(TextEdit)高度。投影为 `spacing.interact_size.y` 的高度语义:
/// egui 里 TextEdit 没有独立高度字段,点击类控件(按钮/输入框/滑条)的最小
/// 高度统一取 `interact_size.y`。
pub const INPUT_H: f32 = 36.0;
/// 输入框内边距(水平)。投影为 `spacing.button_padding.x`,TextEdit 与按钮
/// 共用该字段作为框内文字到边框的留白。
pub const INPUT_PAD_X: f32 = 12.0;
/// 输入框内边距(垂直)。
pub const INPUT_PAD_Y: f32 = 8.0;

// —— 字号(U0 新增)——

/// 小一号正文(提示行、状态栏、次要标签)。shadcn `text-sm = 14px` 的 pt 值。
/// 投影为 `TextStyle::Small` 的字号;Body 13 不动(字号用户设置另行排队,
/// docs/roadmap 专题 #23,与本棒解耦)。
pub const FONT_SM: f32 = 14.0;

// —— 语义档(docs/design-system-plan.md §4,T1 批次只新增、零调用点迁移)——
//
// 第二代尺度真源:命名表达「两个东西的关系」,不表达「它今天解析成
// 多少像素」——取档先问两侧东西彼此是什么,再对号入座。旧 `SPACE_*`
// 等常量与其全部引用一律不动,T2 起按 §4.1 分流规则迁移后才退役。

/// 间距档:量「两个隔开的东西离多远」,不区分间隔与贴边——那两个
/// 语义拆给了下面的 `gap` / `inset` 命名空间,这里只是中性的距离标尺。
// T1 只新增不迁移、暂无生产消费者;T2 接线消费后摘除,clippy 的 unfulfilled 会提醒。
// not(test):守门测试本身就是本档集的首个消费者,cfg(test) 下裸 expect 会 unfulfilled。
#[cfg_attr(not(test), expect(dead_code))]
pub mod space {
    /// 图标基线与文字基线、紧凑分隔线的两侧:两个几乎贴住的东西。
    pub const XXS: f32 = 2.0;
    /// 同一控件内的部件之间:图标与文字、标题与描述。
    pub const XS: f32 = 4.0;
    /// 紧密关联的控件之间:按钮组、对话框动作区。
    pub const SM: f32 = 8.0;
    /// 一个内容组的各部分之间:一行的各列、紧凑表单项。
    pub const MD: f32 = 12.0;
    /// 一节之内分隔的组之间,也量区域整体的内边距。
    pub const LG: f32 = 16.0;
    /// 节与节之间:分节的呼吸。
    pub const XL: f32 = 24.0;
    /// 大区域的边界:空状态的呼吸。
    pub const XXL: f32 = 32.0;
}

/// 字号档:量「这段字以什么身份出场」。取档看文本扮演的角色,不看
/// 它今天解析成多少像素。
// T1 只新增不迁移、暂无生产消费者;T2 接线消费后摘除,clippy 的 unfulfilled 会提醒。
// not(test):守门测试本身就是本档集的首个消费者,cfg(test) 下裸 expect 会 unfulfilled。
#[cfg_attr(not(test), expect(dead_code))]
pub mod text {
    /// 元信息、tooltip:读不读都行的字。
    pub const CAPTION: f32 = 11.0;
    /// 正文与控件标签:界面的默认语气。
    pub const BODY: f32 = 13.0;
    /// 次要标签与提示行:`FONT_SM` 的语义档归宿。
    pub const SMALL: f32 = 14.0;
    /// 窗口/分节/对话框的标题:一块区域的名字。
    pub const TITLE: f32 = 16.0;
    /// 应用名、当前文件名:整个界面只出现一两次的身份字。
    pub const HEADING: f32 = 20.0;
    /// 值得从房间对面读出的数字:统计大头、空状态主文案。
    pub const DISPLAY: f32 = 32.0;
}

/// 间隔:两侧都是内容,用于控件之间。与 `inset` 拆成两个命名空间,
/// 根治「拿间隔的量去填贴边」的度量语境错配(`RAIL_W` 48→40 事故,
/// docs/ui-shell-redesign-v2.md §5.5)。
// T2a 起 `XS` 已有生产消费者(sidebar 的 Git 页节间留白);2026-10-10 设置卡片点样起 `MD`
// 也有了(设置卡片间距)。`SM` 仍暂无生产消费者,挂 expect,接线后
// 摘除(clippy 的 unfulfilled 会提醒)。not(test):守门测试本身就是
// 消费方,cfg(test) 下裸 expect 会 unfulfilled。
pub mod gap {
    /// 同一控件内的部件之间、紧挨着的两个小控件之间。
    pub const XS: f32 = 4.0;
    /// 紧密关联的控件之间:按钮组、对话框动作区。
    // 暂无生产消费者;接线后摘除。
    #[cfg_attr(not(test), expect(dead_code))]
    pub const SM: f32 = 8.0;
    /// 一个内容组的控件/列之间:一行表单的控件与说明。
    pub const MD: f32 = 12.0;
}

/// 贴边距:一侧是内容、另一侧是窗口/容器边界,用于区域内边距。比
/// 同名 `gap` 紧一档——边界不需要对称留白(`inset::MD < gap::MD` 是
/// §4.3 的刻意关系,有守门测试钉住)。
// T2b 起 `MD` 已有生产消费者(layout 的预览面板贴边与查找/跳转浮卡
// 留白);2026-10-10 设置卡片点样起 `LG` 有了(设置卡片内边距)。`SM` 仍暂无生产消费者,
// 挂 expect,接线后摘除(clippy 的 unfulfilled 会提醒)。not(test):
// 守门测试本身就是消费方,cfg(test) 下裸 expect 会 unfulfilled。
pub mod inset {
    /// 控件内容到自身边框的贴边:盒子最里面的一圈。
    // 暂无生产消费者;接线后摘除。
    #[cfg_attr(not(test), expect(dead_code))]
    pub const SM: f32 = 4.0;
    /// 内容区到分组/面板边界的贴边。
    pub const MD: f32 = 8.0;
    /// 大区域到窗口边界的贴边。
    pub const LG: f32 = 12.0;
}

// —— 设置弹窗观感基线(#70 M1)——
//
// 设置窗三段骨架(左分页列 / 中央滚动区 / 底部按钮条)的尺寸与留白
// 单一真源:此前 48 / 112 / 各 margin 散在 settings.rs 行内,改一处漏
// 一处。骨架的**防尺寸反馈环结构**(`exact_size` 定形 + ScrollArea
// `auto_shrink([false,false])`,见 settings.rs `dialog` 注释)不随观感
// 调整改变,这里只统管数字。标题栏本身是 egui Window 的 chrome
// (活动窗填充取 `widgets.open.weak_bg_fill`,随明暗两套 style 走),
// 刻意不再单独配色;本组常量管的是它**下方**三段的间距/分隔/留白。

/// 设置窗左分页列宽(外观/快捷键/AI/MCP/图片 五项竖排)。
pub const SETTINGS_TABS_W: f32 = if cfg!(target_os = "macos") {
    144.0
} else {
    112.0
};
/// 设置窗底部按钮条高:按钮(~24)+ 上下内边距 + 分隔线。#35 从 40
/// 提到 48(坤哥「行高不够,看着不协调」),此后不再回 40。
pub const SETTINGS_FOOTER_H: f32 = if cfg!(target_os = "macos") {
    40.0
} else {
    48.0
};
/// 设置窗默认尺寸(首开锚定屏幕中心,可拖拽缩放)。
pub const SETTINGS_DEFAULT_SIZE: egui::Vec2 = if cfg!(target_os = "macos") {
    egui::vec2(720.0, 500.0)
} else {
    egui::vec2(600.0, 440.0)
};
/// 中央内容区的内边距。与底部按钮条的水平内边距同值(12):分隔线、
/// 正文首行与「关闭」按钮共享同一条纵向基准线,窗内四周呼吸一致。
pub const SETTINGS_BODY_PAD: i8 = if cfg!(target_os = "macos") { 20 } else { 12 };
/// 分页列的内边距:比内容区紧一档 —— 图标+文字行的视觉密度高,
/// 再放到 12 会让 112 的窄列显得空转。
pub const SETTINGS_TABS_PAD: i8 = 8;
/// 底部按钮条的内边距(水平 12 对齐 [`SETTINGS_BODY_PAD`],垂直 6
/// 配 48 高度装下按钮 + 分隔线的呼吸)。
pub const SETTINGS_FOOTER_MARGIN: egui::Margin = egui::Margin::symmetric(12, 6);
/// 设置页两列行的标签列宽(#70 M2):五个分页所有配置行共用 —— 标签列
/// 左对齐、定宽(超长截断),控件列起点 = 内容区左缘 + 此宽 + 列间隙,
/// **跨分页钉在同一 x**。取 148:容得下最长行标签「上下文大小(KB)」
/// (Body 13pt 下约 138px)并留 10px 呼吸。
pub const SETTINGS_LABEL_W: f32 = 148.0;
/// 设置页快捷键行的键位按钮最小宽(#70 M2):37 行的「键位 / 清除 /
/// 重置」排成等宽三段,键位文字("Ctrl+Shift+S" 一档)不把按钮顶宽;
/// 再长的捕获提示截在同宽内。与 [`SETTINGS_LABEL_W`] 同组,但只约束
/// 快捷键页的控件列。
pub const SETTINGS_KEY_W: f32 = 140.0;

// —— 设置分区卡片(2026-10-10 坤哥点样)——
//
// 观感形态对齐 LaterScreen 设置面板的自绘卡片(2026-10-10 坤哥点样):
// 每个分区一张「比窗底浅一档 + 细描边 + 圆角」的卡片,标题行 = 强调色
// 小竖条 + 加粗小字。卡片只管观感,行为仍归 settings.rs 的消息归约。

/// 分区卡片内边距(卡片内容到卡片边框):一侧内容、一侧卡片边界,
/// 语义是贴边距,取 [`inset::LG`]。
pub const SETTINGS_CARD_PAD: i8 = inset::LG as i8;
/// 相邻两张卡片的纵向间隔:两侧都是内容块,语义是间隔,取 [`gap::MD`]。
pub const SETTINGS_CARD_GAP: i8 = gap::MD as i8;
/// 卡片标题左侧的强调竖条(宽 × 高)。竖条是标题的「图钉」,属
/// 物理几何,不进语义档。
pub const SETTINGS_CARD_BAR: egui::Vec2 = egui::Vec2::new(3.0, 14.0);

// —— 圆角 ——

/// 按钮圆角(shadcn rounded-sm 档)。
pub const RADIUS_SM: f32 = 4.0;
/// 控件圆角(shadcn rounded-md 档)。2026-09-27 起为 `theme::apply_shell`
/// 投影的数字真源(此前硬编码 6,ui-polish §2 的划线注记随之作废)。
///
/// 来源:Armas / shadcn 的 `rounded-md = 6px`(docs/ui-modernization.md
/// §2.4「白拿它产出的办法」——抄数值不引库)。
pub const RADIUS_MD: f32 = 6.0;
/// 卡片/大面板圆角(shadcn rounded-lg 档)。2026-10-10 设置卡片点样起设置分区卡片消费;
/// 比控件档大一档,大面积面板的圆角再贴着控件档读会显得拘谨。
///
/// 来源:shadcn 的 `rounded-lg = 8px`(出处同上)。
pub const RADIUS_LG: f32 = 8.0;

// —— 语义色 ——

/// 当前应用皮肤的强调色，手绘图标、行号与控件共用。
pub fn accent(ui: &egui::Ui) -> Color32 {
    crate::theme::shell(ui).accent
}

/// rail 带的底(详见 [`RAIL_W`])。比侧栏退后一档、远比内容区沉,让
/// rail 读成「镶在外壳上的第三条纵带」而非「侧栏里的一列」。
///
/// 明度落在 `sidebar` 与 `content` 之间之外的一侧(比 sidebar 更沉),
/// 约束同 `theme::ShellTokens::sidebar` 的取值口径(S2-3):暗色可以更深
/// 但不能撞上窗口底色(否则读成「挖了个洞」),浅色不能深到与 `border`
/// 撞色(否则侧栏里的分隔线消失)。
pub fn rail_fill(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_rgb(0x16, 0x17, 0x1A)
    } else {
        egui::Color32::from_rgb(0xE4, 0xE7, 0xEB)
    }
}

/// 可行动降级(与回滚 dirty 警示同档黄)。
pub const WARN: Color32 = Color32::from_rgb(0xEB, 0xB4, 0x3C);
/// 不可逆警示(与回滚确认文案同档红)。
pub const DANGER: Color32 = Color32::from_rgb(0xEB, 0x60, 0x60);
/// 成功 / 已配置态。
pub const OK: Color32 = Color32::from_rgb(0x60, 0xC8, 0x78);

#[cfg(test)]
mod tests {
    use super::*;

    /// U0 新 token 的数值口径:抄自 armas/shadcn 的公开设计值
    /// (docs/ui-modernization.md §2.4),改这里必须同步改该表。
    #[test]
    fn u0_tokens_match_sourced_values() {
        assert_eq!(RADIUS_MD, 6.0, "shadcn rounded-md");
        assert_eq!(RADIUS_LG, 8.0, "shadcn rounded-lg");
        assert_eq!(INPUT_H, 36.0, "shadcn h-9");
        assert_eq!(INPUT_PAD_X, 12.0, "shadcn px-3");
        assert_eq!(INPUT_PAD_Y, 8.0, "shadcn py-2");
        assert_eq!(FONT_SM, 14.0, "shadcn text-sm");
    }

    /// 两套 visuals 下强调色不同(明暗各一档),且都非空色。
    #[test]
    fn accent_differs_per_theme() {
        let ctx = egui::Context::default();
        let mut light = None;
        let mut dark = None;
        ctx.run_ui(egui::RawInput::default(), |ui| {
            light = Some(accent(ui));
        })
        .drop_without_applying_deltas();
        let ctx = egui::Context::default();
        ctx.set_theme(egui::Theme::Light);
        ctx.run_ui(egui::RawInput::default(), |ui| {
            dark = Some(accent(ui));
        })
        .drop_without_applying_deltas();
        assert_ne!(light, dark, "明暗两档强调色不同");
    }

    /// 语义档守门(docs/design-system-plan.md §4.5):相邻两档分不出
    /// 大小 = 档位退化成「同一档的两个名字」,严格递增即无重复。
    fn assert_strictly_increasing(scale: &str, steps: &[f32]) {
        for pair in steps.windows(2) {
            assert!(
                pair[0] < pair[1],
                "{scale} 档位须严格递增且无重复:{} !< {}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn space_scale_strictly_increasing() {
        assert_strictly_increasing(
            "space",
            &[
                space::XXS,
                space::XS,
                space::SM,
                space::MD,
                space::LG,
                space::XL,
                space::XXL,
            ],
        );
    }

    #[test]
    fn text_scale_strictly_increasing() {
        assert_strictly_increasing(
            "text",
            &[
                text::CAPTION,
                text::BODY,
                text::SMALL,
                text::TITLE,
                text::HEADING,
                text::DISPLAY,
            ],
        );
    }

    #[test]
    fn gap_scale_strictly_increasing() {
        assert_strictly_increasing("gap", &[gap::XS, gap::SM, gap::MD]);
    }

    #[test]
    fn inset_scale_strictly_increasing() {
        assert_strictly_increasing("inset", &[inset::SM, inset::MD, inset::LG]);
    }

    /// §4.3 刻意关系:贴边距比同名间隔紧一档。这条不等式一旦反过来,
    /// 就回到了 `RAIL_W` 48→40 的度量语境错配,必须钉死。
    /// (经局部变量取值:直接断言两个常量的大小关系会踩
    /// `clippy::assertions_on_constants`。)
    #[test]
    fn inset_md_is_tighter_than_gap_md() {
        let inset_md = inset::MD;
        let gap_md = gap::MD;
        assert!(
            inset_md < gap_md,
            "inset::MD({}) 必须小于 gap::MD({}):贴边距比同名间隔紧一档",
            inset_md,
            gap_md
        );
    }

    /// 钉值防漂移:各档数值与规划 §4.2/§4.3 的表一字不差,改任一侧
    /// 必须同步改另一侧(本测试就是另一侧)。
    #[test]
    fn semantic_tier_values_match_plan() {
        // §4.2 间距 7 档
        assert_eq!(space::XXS, 2.0);
        assert_eq!(space::XS, 4.0);
        assert_eq!(space::SM, 8.0);
        assert_eq!(space::MD, 12.0);
        assert_eq!(space::LG, 16.0);
        assert_eq!(space::XL, 24.0);
        assert_eq!(space::XXL, 32.0);
        // §4.2 字号 6 档
        assert_eq!(text::CAPTION, 11.0);
        assert_eq!(text::BODY, 13.0);
        assert_eq!(text::SMALL, 14.0);
        assert_eq!(text::TITLE, 16.0);
        assert_eq!(text::HEADING, 20.0);
        assert_eq!(text::DISPLAY, 32.0);
        // §4.3 间隔 3 档
        assert_eq!(gap::XS, 4.0);
        assert_eq!(gap::SM, 8.0);
        assert_eq!(gap::MD, 12.0);
        // §4.3 贴边距 3 档
        assert_eq!(inset::SM, 4.0);
        assert_eq!(inset::MD, 8.0);
        assert_eq!(inset::LG, 12.0);
    }
}
