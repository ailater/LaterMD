//! 「插入 Emoji」面板(docs/emoji-plan.md E1 骨架 + E2 搜索/分类/最近使用)。
//!
//! ## 分工(与 `ui::image_dialog` 同一套写法)
//!
//! 本模块只收输入、只发消息:搜索草稿 `query` 与当前分类 `group` 归 UI
//! 原地持有(`TextEdit` 是立即模式控件,草稿必须能就地 `&mut`),插入的
//! 归约在 `state`(走 `compose::insert_emoji`),点选后关面板与 Esc 关闭
//! 也都经消息(`EmojiInserted` / `EmojiPickerToggle(false)`)。
//!
//! ## E2 的口径
//!
//! - **搜索**:`query` 非空时走 `emoji_data::search` 三路匹配(中文名 /
//!   英文名 / 短码,大小写不敏感),结果**跨分类**呈现,段头与 tooltip
//!   都标来源分类;无命中显式给「无匹配」。空查询 = 当前分类全表。
//! - **最近使用**:面板底部一行(空态整行隐藏);点选即再插入 —— 连插
//!   多个靠它二次进入。去重、封顶与落盘都在归约(`state::insert_emoji`
//!   → `settings.json`,与主题同路)。
//!
//! ## 渲染口径
//!
//! 面板与「最近使用」条:随包 Twemoji 72px PNG 画 32 逻辑点白 tint Image
//! —— 彩色(A2,机制与回落见下方「纹理路径」节);**编辑器正文仍是黑白**
//! (epaint 0.36.2 字形管线恒纯白填充,emoji-color-feasibility §1),导出 /
//! 外发的显示效果取决于目标环境的字体,面板底部一行小字即此口径。
//! 无头测试只断言「点击 → 发出正确消息」「渲染不 panic」「实显枚都拿到
//! 纹理(损坏/缺失回落)」,不断言字形(has_glyph 依赖真实字体,会
//! flaky,§7 #7)。
//!
//! ## E3 的口径:不让用户看到豆腐块
//!
//! 面板首帧(字体装载后才会开面板,天然满足「装载后」)按出厂
//! NotoEmoji 的 **cmap format 12 覆盖**核验全表一次,结果缓存于
//! [`EmojiPanelState::glyphs`],此后不再核验(不是每帧)。缺字形条目
//! **数据保留、渲染剔除**(`emoji_data::visible_entries` / `visible_hits`,
//! 谓词注入);分类被剔光时显示「本机字体缺字形」占位而非空网格;
//! 「最近使用」同样过滤。单测给假集合 / 全量集合注入,不碰真实字形。
//!
//! **为什么不是 `Fonts::has_glyphs`**(emoji-plan F4 原方案):epaint
//! 0.36.2 的替换字形 ◻ 恰在 NotoEmoji 上,`has_glyph` 凡该 face 拥有的
//! 字符一律误报 false,实测全表 0/272 —— 原方案作废,按 decisions-pending
//! #52 的 cmap 直验口径运行时化(`emoji_data::from_font_cmap12`),同时
//! 给 #52 的入库清洗加一道常驻回归防线。

use std::collections::HashMap;

use crate::state::Message;
use crate::ui::emoji_data::{self, EmojiEntry};
use crate::ui::tokens;
use eframe::egui;

/// 网格列数(emoji-plan §6.4:8 列)。
const COLUMNS: usize = 8;
/// 单元格边长(emoji-plan §6.4:32×32)。
const CELL: f32 = 32.0;
/// 单元格字号:占格子约六成,再大就顶到 hover 底色边缘。
const CELL_FONT: f32 = CELL * 0.6;
/// 搜索结果滚动区的高度上限(约 5 行)。每类 ≤ 40 枚本就单屏放得下;
/// 跨分类搜索的命中总数不受该约束,用滚动 + 屏外字形不绘制(`cell` 里
/// `is_rect_visible` 跳过)把渲进字体图集的量限在可视区,防图集膨胀
/// (emoji-plan §7 #6)。
const RESULTS_MAX_H: f32 = 200.0;

// ## A2 纹理路径(#47,emoji-color-feasibility §2)
//
// 面板与「最近使用」的彩色来自随包 Twemoji 72px PNG(`assets/emoji/twemoji/`,
// jdecked v17.0.3,CC-BY 4.0,登记 docs/distribution.md §6):`image` 解码 →
// `ctx.load_texture` → 画 32 逻辑点白 tint Image(72px 资产,≥2.25x 显示
// 密度)。纹理句柄存 egui temp memory(per-Context、不序列化、Context 销毁
// 才清,0.36.2 `IdTypeMap` 的 GC 只作用于序列化值),与
// [`EmojiPanelState::glyphs`] 同节奏:面板首帧懒建、会话内不失效。
// 资产缺失 / PNG 解码失败 → 该单元回落出厂 NotoEmoji 黑白字形,与 E3
// 「数据保留、渲染兜底」同哲学,彩色不引入新失败面。

/// 资产表紧凑写法:`(字符, 文件名)` → `(字符, PNG 字节)`,路径相对本文件,
/// include_bytes 编译期钉住:文件挪走 / 改名即编译红。
macro_rules! twemoji_png_assets {
    ($(($char:literal, $file:literal)),* $(,)?) => {
        &[$(($char, include_bytes!(concat!("../../../../assets/emoji/twemoji/", $file)))),*]
    };
}

/// emoji 字符 → 随包 Twemoji PNG,272 枚与 `emoji_data` 全表 1:1(顺序 =
/// 表序;清单与 `assets/emoji/twemoji/fetch.sh` 的钉死下载清单按同一规则
/// 派生,两边不许单边增删)。
static TWEMOJI: &[(&str, &[u8])] = twemoji_png_assets![
    ("😀", "1f600.png"),
    ("😃", "1f603.png"),
    ("😄", "1f604.png"),
    ("😁", "1f601.png"),
    ("😆", "1f606.png"),
    ("😅", "1f605.png"),
    ("😂", "1f602.png"),
    ("😉", "1f609.png"),
    ("😊", "1f60a.png"),
    ("😍", "1f60d.png"),
    ("😘", "1f618.png"),
    ("😋", "1f60b.png"),
    ("😛", "1f61b.png"),
    ("😜", "1f61c.png"),
    ("😏", "1f60f.png"),
    ("😒", "1f612.png"),
    ("😬", "1f62c.png"),
    ("😲", "1f632.png"),
    ("😳", "1f633.png"),
    ("😢", "1f622.png"),
    ("😭", "1f62d.png"),
    ("😱", "1f631.png"),
    ("😴", "1f634.png"),
    ("😷", "1f637.png"),
    ("😪", "1f62a.png"),
    ("😫", "1f62b.png"),
    ("😩", "1f629.png"),
    ("😑", "1f611.png"),
    ("😐", "1f610.png"),
    ("😶", "1f636.png"),
    ("😔", "1f614.png"),
    ("😕", "1f615.png"),
    ("😤", "1f624.png"),
    ("👍", "1f44d.png"),
    ("👎", "1f44e.png"),
    ("👌", "1f44c.png"),
    ("✌️", "270c.png"),
    ("👏", "1f44f.png"),
    ("🙌", "1f64c.png"),
    ("👐", "1f450.png"),
    ("🙏", "1f64f.png"),
    ("💪", "1f4aa.png"),
    ("👋", "1f44b.png"),
    ("👈", "1f448.png"),
    ("👉", "1f449.png"),
    ("👆", "1f446.png"),
    ("👇", "1f447.png"),
    ("☝️", "261d.png"),
    ("👊", "1f44a.png"),
    ("✊", "270a.png"),
    ("✋", "270b.png"),
    ("👶", "1f476.png"),
    ("👦", "1f466.png"),
    ("👧", "1f467.png"),
    ("👨", "1f468.png"),
    ("👩", "1f469.png"),
    ("👴", "1f474.png"),
    ("👵", "1f475.png"),
    ("👮", "1f46e.png"),
    ("🕵️", "1f575.png"),
    ("💁", "1f481.png"),
    ("🙋", "1f64b.png"),
    ("🙆", "1f646.png"),
    ("🙅", "1f645.png"),
    ("🙇", "1f647.png"),
    ("💃", "1f483.png"),
    ("🚶", "1f6b6.png"),
    ("🏃", "1f3c3.png"),
    ("🏊", "1f3ca.png"),
    ("🚴", "1f6b4.png"),
    ("👪", "1f46a.png"),
    ("👫", "1f46b.png"),
    ("🛀", "1f6c0.png"),
    ("🐶", "1f436.png"),
    ("🐱", "1f431.png"),
    ("🐭", "1f42d.png"),
    ("🐻", "1f43b.png"),
    ("🐼", "1f43c.png"),
    ("🐨", "1f428.png"),
    ("🐯", "1f42f.png"),
    ("🐮", "1f42e.png"),
    ("🐷", "1f437.png"),
    ("🐸", "1f438.png"),
    ("🐵", "1f435.png"),
    ("🐔", "1f414.png"),
    ("🐧", "1f427.png"),
    ("🐝", "1f41d.png"),
    ("🐟", "1f41f.png"),
    ("🐙", "1f419.png"),
    ("🐳", "1f433.png"),
    ("🐬", "1f42c.png"),
    ("🍎", "1f34e.png"),
    ("🍌", "1f34c.png"),
    ("🍇", "1f347.png"),
    ("🍉", "1f349.png"),
    ("🍓", "1f353.png"),
    ("🍑", "1f351.png"),
    ("🍞", "1f35e.png"),
    ("🍚", "1f35a.png"),
    ("🍜", "1f35c.png"),
    ("🍕", "1f355.png"),
    ("🍔", "1f354.png"),
    ("🍟", "1f35f.png"),
    ("🍰", "1f370.png"),
    ("☕", "2615.png"),
    ("🍺", "1f37a.png"),
    ("🍷", "1f377.png"),
    ("🐴", "1f434.png"),
    ("🐹", "1f439.png"),
    ("🐰", "1f430.png"),
    ("🌽", "1f33d.png"),
    ("🍅", "1f345.png"),
    ("🍪", "1f36a.png"),
    ("💻", "1f4bb.png"),
    ("🖥️", "1f5a5.png"),
    ("⌨️", "2328.png"),
    ("🖱️", "1f5b1.png"),
    ("📱", "1f4f1.png"),
    ("📞", "1f4de.png"),
    ("📷", "1f4f7.png"),
    ("🔋", "1f50b.png"),
    ("💡", "1f4a1.png"),
    ("🔍", "1f50d.png"),
    ("🔒", "1f512.png"),
    ("🔑", "1f511.png"),
    ("🔧", "1f527.png"),
    ("🔨", "1f528.png"),
    ("📌", "1f4cc.png"),
    ("📎", "1f4ce.png"),
    ("✂️", "2702.png"),
    ("📝", "1f4dd.png"),
    ("📓", "1f4d3.png"),
    ("📚", "1f4da.png"),
    ("📖", "1f4d6.png"),
    ("✏️", "270f.png"),
    ("💰", "1f4b0.png"),
    ("💵", "1f4b5.png"),
    ("💳", "1f4b3.png"),
    ("⏰", "23f0.png"),
    ("⌚", "231a.png"),
    ("🎁", "1f381.png"),
    ("🎈", "1f388.png"),
    ("🎉", "1f389.png"),
    ("🎂", "1f382.png"),
    ("🏆", "1f3c6.png"),
    ("🎸", "1f3b8.png"),
    ("🎮", "1f3ae.png"),
    ("🎲", "1f3b2.png"),
    ("🔔", "1f514.png"),
    ("📢", "1f4e2.png"),
    ("🎧", "1f3a7.png"),
    ("🎤", "1f3a4.png"),
    ("❤️", "2764.png"),
    ("💛", "1f49b.png"),
    ("💚", "1f49a.png"),
    ("💙", "1f499.png"),
    ("💜", "1f49c.png"),
    ("💔", "1f494.png"),
    ("💕", "1f495.png"),
    ("💯", "1f4af.png"),
    ("✅", "2705.png"),
    ("❌", "274c.png"),
    ("⚠️", "26a0.png"),
    ("❗", "2757.png"),
    ("❓", "2753.png"),
    ("⭐", "2b50.png"),
    ("🌟", "1f31f.png"),
    ("✨", "2728.png"),
    ("🔥", "1f525.png"),
    ("💥", "1f4a5.png"),
    ("💫", "1f4ab.png"),
    ("⚡", "26a1.png"),
    ("☀️", "2600.png"),
    ("🌈", "1f308.png"),
    ("🌙", "1f319.png"),
    ("❄️", "2744.png"),
    ("💤", "1f4a4.png"),
    ("💢", "1f4a2.png"),
    ("💬", "1f4ac.png"),
    ("💭", "1f4ad.png"),
    ("♻️", "267b.png"),
    ("➕", "2795.png"),
    ("➖", "2796.png"),
    ("🚫", "1f6ab.png"),
    ("⛔", "26d4.png"),
    ("🆕", "1f195.png"),
    ("🆗", "1f197.png"),
    ("🆒", "1f192.png"),
    ("🔝", "1f51d.png"),
    ("🔴", "1f534.png"),
    ("💐", "1f490.png"),
    ("🌹", "1f339.png"),
    ("🚀", "1f680.png"),
    ("✈️", "2708.png"),
    ("🚉", "1f689.png"),
    ("🚗", "1f697.png"),
    ("🚕", "1f695.png"),
    ("🚌", "1f68c.png"),
    ("🚑", "1f691.png"),
    ("🚒", "1f692.png"),
    ("🚓", "1f693.png"),
    ("🚲", "1f6b2.png"),
    ("🚢", "1f6a2.png"),
    ("⛵", "26f5.png"),
    ("🚂", "1f682.png"),
    ("🗺️", "1f5fa.png"),
    ("🗽", "1f5fd.png"),
    ("🗼", "1f5fc.png"),
    ("🏰", "1f3f0.png"),
    ("🎡", "1f3a1.png"),
    ("🎢", "1f3a2.png"),
    ("⛱️", "26f1.png"),
    ("🌋", "1f30b.png"),
    ("🗻", "1f5fb.png"),
    ("🌊", "1f30a.png"),
    ("🌍", "1f30d.png"),
    ("🌏", "1f30f.png"),
    ("🌎", "1f30e.png"),
    ("🏠", "1f3e0.png"),
    ("🏢", "1f3e2.png"),
    ("🏥", "1f3e5.png"),
    ("🏦", "1f3e6.png"),
    ("🏫", "1f3eb.png"),
    ("⛩️", "26e9.png"),
    ("🏯", "1f3ef.png"),
    ("🌃", "1f303.png"),
    ("🌅", "1f305.png"),
    ("🌄", "1f304.png"),
    ("🚙", "1f699.png"),
    ("🚚", "1f69a.png"),
    ("⛽", "26fd.png"),
    ("⛪", "26ea.png"),
    ("🇨🇳", "1f1e8-1f1f3.png"),
    ("🇺🇸", "1f1fa-1f1f8.png"),
    ("🇬🇧", "1f1ec-1f1e7.png"),
    ("🇯🇵", "1f1ef-1f1f5.png"),
    ("🇰🇷", "1f1f0-1f1f7.png"),
    ("🇫🇷", "1f1eb-1f1f7.png"),
    ("🇩🇪", "1f1e9-1f1ea.png"),
    ("🇮🇹", "1f1ee-1f1f9.png"),
    ("🇪🇸", "1f1ea-1f1f8.png"),
    ("🇵🇹", "1f1f5-1f1f9.png"),
    ("🇷🇺", "1f1f7-1f1fa.png"),
    ("🇮🇳", "1f1ee-1f1f3.png"),
    ("🇧🇷", "1f1e7-1f1f7.png"),
    ("🇨🇦", "1f1e8-1f1e6.png"),
    ("🇦🇺", "1f1e6-1f1fa.png"),
    ("🇳🇿", "1f1f3-1f1ff.png"),
    ("🇸🇬", "1f1f8-1f1ec.png"),
    ("🇲🇾", "1f1f2-1f1fe.png"),
    ("🇹🇭", "1f1f9-1f1ed.png"),
    ("🇻🇳", "1f1fb-1f1f3.png"),
    ("🇵🇭", "1f1f5-1f1ed.png"),
    ("🇮🇩", "1f1ee-1f1e9.png"),
    ("🇳🇱", "1f1f3-1f1f1.png"),
    ("🇨🇭", "1f1e8-1f1ed.png"),
    ("🇸🇪", "1f1f8-1f1ea.png"),
    ("🇳🇴", "1f1f3-1f1f4.png"),
    ("🇫🇮", "1f1eb-1f1ee.png"),
    ("🇩🇰", "1f1e9-1f1f0.png"),
    ("🇵🇱", "1f1f5-1f1f1.png"),
    ("🇧🇪", "1f1e7-1f1ea.png"),
    ("🇬🇷", "1f1ec-1f1f7.png"),
    ("🇹🇷", "1f1f9-1f1f7.png"),
    ("🇪🇬", "1f1ea-1f1ec.png"),
    ("🇿🇦", "1f1ff-1f1e6.png"),
    ("🇦🇷", "1f1e6-1f1f7.png"),
    ("🇨🇱", "1f1e8-1f1f1.png"),
    ("🇲🇽", "1f1f2-1f1fd.png"),
    ("🇸🇦", "1f1f8-1f1e6.png"),
    ("🇦🇪", "1f1e6-1f1ea.png"),
    ("🇺🇦", "1f1fa-1f1e6.png"),
];

/// emoji 字符 → 随包 PNG 字节;表外字符返回 `None`(调用方回落黑白)。
fn twemoji_png(glyph: &str) -> Option<&'static [u8]> {
    TWEMOJI
        .iter()
        .find(|(ch, _)| *ch == glyph)
        .map(|(_, png)| *png)
}

/// 会话级纹理缓存:emoji 字符 → 纹理句柄(节奏见模块顶部 A2 说明)。
type TextureCache = HashMap<String, egui::TextureHandle>;

/// `TextureCache` 在 temp memory 里的槽位。
fn texture_cache_id() -> egui::Id {
    egui::Id::new("latermd_emoji_twemoji_textures")
}

/// PNG 字节 → 纹理句柄(unmultiplied RGBA + 线性采样,白 tint 下即原色,
/// emoji-color-feasibility §2.1 探针口径)。解码失败返回 `None`,调用方
/// 回落黑白,不 panic。
fn decode_texture(ctx: &egui::Context, glyph: &str, png: &[u8]) -> Option<egui::TextureHandle> {
    let rgba = image::load_from_memory(png).ok()?.to_rgba8();
    Some(ctx.load_texture(
        format!("twemoji:{glyph}"),
        egui::ColorImage::from_rgba_unmultiplied(
            [rgba.width() as usize, rgba.height() as usize],
            rgba.as_raw(),
        ),
        egui::TextureOptions::LINEAR,
    ))
}

/// 该字符的纹理:缓存命中直接给;未命中解码一次并落缓存;表外字符 /
/// 解码失败给 `None`(调用方回落黑白)。每枚全会话至多解码一次。
fn texture_for(
    ui: &egui::Ui,
    glyph: &str,
    cache: &mut TextureCache,
) -> Option<egui::TextureHandle> {
    if let Some(handle) = cache.get(glyph) {
        return Some(handle.clone());
    }
    let handle = decode_texture(ui.ctx(), glyph, twemoji_png(glyph)?)?;
    cache.insert(glyph.to_owned(), handle.clone());
    Some(handle)
}

/// 预览内联 emoji(#48 B2)的纹理入口:与面板**同一份会话缓存**—— 面板
/// 先开则预览直接命中,反之亦然,同一 Context 内每枚至多解码一次(A2 的
/// 节奏承诺不变)。表外字符 / 解码失败返回 `None`,调用方保持透明占位
/// 不动(不画黑块不 panic,任务书 B2 的回落口径)。
///
/// 命中走只读 `data`(读锁 + 句柄克隆,emoji 密集文档的每帧常态);未命中
/// 的解码与 `load_texture` **必须在 data 锁外**做 —— `load_texture` 内部经
/// `Context::input` 再入 Context 写锁,包在 `data_mut` 闭包里会自锁死
/// (egui 0.36 实测,#48 B2 开发期踩过),写回用单键 insert,不整表进出。
pub(crate) fn inline_texture(ui: &egui::Ui, glyph: &str) -> Option<egui::TextureHandle> {
    if let Some(handle) = ui
        .ctx()
        .data(|data| data.get_temp::<TextureCache>(texture_cache_id()))
        .and_then(|cache| cache.get(glyph).cloned())
    {
        return Some(handle);
    }
    // 表外字符直接判None:不画图是确定性结论,不为它动缓存
    let png = twemoji_png(glyph)?;
    let handle = decode_texture(ui.ctx(), glyph, png)?;
    ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_insert_with(texture_cache_id(), TextureCache::default)
            .insert(glyph.to_owned(), handle.clone());
    });
    Some(handle)
}

/// Emoji 面板状态(归约置 `open`,UI 改 `query` / `group`)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmojiPanelState {
    /// 面板是否可见。
    pub open: bool,
    /// 搜索草稿。非空时网格换 `emoji_data::search` 的跨分类结果;归约在
    /// 开面板时清空(上次的搜索词对下一次插入没有意义)。
    pub query: String,
    /// 当前分类(`emoji_data::GROUPS` 的下标);清空搜索词即回到它。
    pub group: usize,
    /// 最近使用(去重、新的在前、上限见 `state::EMOJI_RECENT_CAP`)。
    /// 维护与持久化都在归约(E2 起随 settings.json),面板只展示。
    pub recent: Vec<String>,
    /// E3 字形探测缓存:`None` = 未探测(面板首帧探一次);`Some` 后本
    /// 会话不再探测。本应用字体只在启动时装一次(fonts::install),故不
    /// 设失效路径 —— 若将来支持运行中换字体,须同时清此缓存。
    pub glyphs: Option<emoji_data::GlyphSet>,
}

/// 出厂 emoji 字体在 `FontDefinitions::font_data` 里的键名(egui 出厂链,
/// fonts.rs 保序不动)。
const NOTO_EMOJI_FONT: &str = "NotoEmoji-Regular";

/// E3 真实核验(emoji-plan E3,按 decisions-pending #52 的 cmap 口径
/// 运行时化):直读出厂 NotoEmoji-Regular 的 cmap format 12 子表核验
/// 全表覆盖,缺字形的字符不进集合。只在面板首帧调用一次(缓存于
/// `EmojiPanelState::glyphs`),不是每帧 —— 出厂字体编译期嵌入,核验
/// 结果整个会话不变。
///
/// **不用 `Fonts::has_glyphs`**:epaint 0.36.2 的替换字形 ◻ 恰在
/// NotoEmoji 上,凡该 face 拥有的字符一律误报 false(#52 实测,本棒
/// 复测 0/272 全 false);渲染 shaping 走 harfrust 独立解析,不受其害。
///
/// 兜底:键名缺失或解析失败(结果为空)时全量放行 —— 内置字体编译期
/// 嵌入、任何平台必有 emoji 字形,「全空」只可能是核验自身失效;报废
/// 整个面板比冒豆腐风险更糟,豆腐风险的最后一道是真机目视(§8)。
fn probe_glyphs() -> emoji_data::GlyphSet {
    let defs = egui::FontDefinitions::default();
    let Some(bytes) = defs
        .font_data
        .get(NOTO_EMOJI_FONT)
        .map(|fd| fd.font.as_ref())
    else {
        eprintln!("LaterMD: 出厂字体链无 {NOTO_EMOJI_FONT},跳过字形核验(全表放行)");
        return emoji_data::GlyphSet::all();
    };
    let set = emoji_data::GlyphSet::from_font_cmap12(bytes);
    if set.is_empty() {
        eprintln!("LaterMD: {NOTO_EMOJI_FONT} cmap 核验失败(空结果),全表放行");
        return emoji_data::GlyphSet::all();
    }
    set
}

/// 画面板,返回所有可点 emoji 单元的响应(网格 + 搜索结果 + 最近使用,
/// 测试定位用,生产调用方忽略)。
///
/// 点选单元发 [`Message::EmojiInserted`],归约里插入并关面板;Esc 直接
/// 发 `Message::EmojiPickerToggle(false)`(只关面板,不动文档)。
pub fn panel(
    ui: &mut egui::Ui,
    state: &mut EmojiPanelState,
    outbox: &mut Vec<Message>,
) -> Vec<egui::Response> {
    // E3:首帧核验一次并缓存(开面板必然在字体装载之后);生产走真实
    // 核验,无头测试由 frame() 预置注入,不碰真实字形(§7 #7)
    if state.glyphs.is_none() {
        state.glyphs = Some(probe_glyphs());
    }
    // A2:会话级纹理缓存本帧取出来(首帧为空 = 懒建的起点),绘制中未
    // 命中的解码落进来,帧尾整体写回 temp memory —— 一次取还,不在单元
    // 粒度反复克隆
    let mut textures = ui.ctx().data_mut(|data| {
        data.get_temp::<TextureCache>(texture_cache_id())
            .unwrap_or_default()
    });
    let mut cells = Vec::new();
    crate::ui::workbench::centered_dialog(ui, "插入 Emoji")
        .collapsible(false)
        .resizable(false)
        .frame(crate::ui::workbench::dialog_frame(ui))
        .show(ui.ctx(), |ui| {
            // 搜索框:E2 起参与过滤(三路大小写不敏感匹配,emoji_data::search);
            // desired_width 撑满让网格与输入框同宽
            ui.add(
                egui::TextEdit::singleline(&mut state.query)
                    .hint_text("搜索中文名 / 英文名 / 短码")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(tokens::SPACE_SM);
            // 分类横向标签。搜索态下标签保留可点:清空搜索词即回到所选分类
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = tokens::SPACE_XS;
                for (index, group) in emoji_data::GROUPS.iter().enumerate() {
                    if ui
                        .selectable_label(index == state.group, group.name)
                        .clicked()
                    {
                        state.group = index;
                    }
                }
            });
            ui.add_space(tokens::SPACE_SM);
            // E3:字形白名单(首帧已探好),网格 / 搜索 / 最近统一据此过滤
            let glyphs = state
                .glyphs
                .as_ref()
                .expect("面板首帧已探测字形(见函数开头)");
            let has = |glyph: &str| glyphs.allows(glyph);
            // 网格:空查询 = 当前分类全表(剔缺字形);有查询 = 跨分类命中,
            // 段头高亮来源分类(emoji-plan E2)
            let query = state.query.trim();
            if query.is_empty() {
                let index = state.group.min(emoji_data::GROUPS.len() - 1);
                let group = &emoji_data::GROUPS[index];
                // 分类被剔光:占位说明,不是空网格(E3)
                let visible = emoji_data::visible_entries(group.entries, has);
                if visible.is_empty() {
                    ui.weak("本机字体缺字形");
                }
                for row in visible.chunks(COLUMNS) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = tokens::SPACE_XS;
                        for &entry in row {
                            cells.push(cell(ui, entry, None, &mut textures, outbox));
                        }
                    });
                }
            } else {
                let hits = emoji_data::visible_hits(emoji_data::search(query), has);
                if hits.is_empty() {
                    ui.weak("无匹配");
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("emoji-search-results")
                        .max_height(RESULTS_MAX_H)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            // hits 按分类有序:连续同组号即一个来源分类,
                            // 段头就是命中来源(跨分类可发现)
                            for run in hits.chunk_by(|a, b| a.0 == b.0) {
                                let source = emoji_data::GROUPS[run[0].0].name;
                                ui.strong(source);
                                for row in run.chunks(COLUMNS) {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = tokens::SPACE_XS;
                                        for &(_, entry) in row {
                                            cells.push(cell(
                                                ui,
                                                entry,
                                                Some(source),
                                                &mut textures,
                                                outbox,
                                            ));
                                        }
                                    });
                                }
                            }
                        });
                }
            }
            ui.add_space(tokens::SPACE_XS);
            ui.separator();
            // 「最近使用」一行(E2):空态整行隐藏;点选即再插入 —— 连插
            // 多个靠它二次进入,不用重新翻分类。E3:缺字形的历史记录同样
            // 剔除,不让豆腐块从这条缝里漏回来
            let recent: Vec<&String> = state.recent.iter().filter(|g| has(g)).collect();
            if !recent.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = tokens::SPACE_XS;
                    ui.weak("最近");
                    for emoji in recent {
                        cells.push(glyph_cell(
                            ui,
                            emoji,
                            "最近使用 · 点选再次插入",
                            &mut textures,
                            outbox,
                        ));
                    }
                });
                ui.add_space(tokens::SPACE_XS);
                ui.separator();
            }
            ui.weak("面板内彩色;编辑器正文仍黑白;导出/外发的显示效果取决于目标环境的字体");
        });
    // A2:纹理缓存写回(temp memory 随 Context 存活,面板关了再开也不
    // 重复解码)
    ui.ctx()
        .data_mut(|data| data.insert_temp(texture_cache_id(), textures));
    // Esc 关闭:面板开着才走到这里,Esc 就是「收起面板」;不消费 ——
    // 编辑器对 Esc 本就无动作,禅定的 Esc 出口在 draw_zen 里先消费,
    // 输入流顺序天然让「退禅定」优先于「关面板」。
    if ui.ctx().input(|input| input.key_pressed(egui::Key::Escape)) {
        outbox.push(Message::EmojiPickerToggle(false));
    }
    cells
}

/// 网格单元:字符居中 + hover 底色 + tooltip(三名一路,可发现性);
/// `source` 非 None(搜索态)时前置来源分类名。
fn cell(
    ui: &mut egui::Ui,
    entry: &EmojiEntry,
    source: Option<&'static str>,
    textures: &mut TextureCache,
    outbox: &mut Vec<Message>,
) -> egui::Response {
    let tooltip = match source {
        Some(group) => format!(
            "{group} · {} · {} · :{}:",
            entry.name_zh, entry.name_en, entry.shortcode
        ),
        None => format!(
            "{} · {} · :{}:",
            entry.name_zh, entry.name_en, entry.shortcode
        ),
    };
    glyph_cell(ui, entry.char, &tooltip, textures, outbox)
}

/// 字符单元的公共体(网格与「最近使用」共用):32×32 点击区,hover 底色
/// 与 tooltip,点选发插入消息。绘制是 A2 的单点分支:随包 Twemoji 纹理
/// 可用 → 32 逻辑点白 tint Image(72px 资产,≥2.25x 显示密度);资产缺失
/// 或解码失败 → 原样回落出厂 NotoEmoji 黑白字形。点击区、hover、tooltip
/// 与载荷不因彩色变。
fn glyph_cell(
    ui: &mut egui::Ui,
    glyph: &str,
    tooltip: &str,
    textures: &mut TextureCache,
    outbox: &mut Vec<Message>,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(CELL, CELL), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let texture = texture_for(ui, glyph, textures);
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(
                rect,
                tokens::RADIUS_SM,
                ui.visuals().widgets.hovered.bg_fill,
            );
        }
        if let Some(texture) = texture {
            painter.image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                glyph,
                egui::FontId::proportional(CELL_FONT),
                ui.visuals().text_color(),
            );
        }
    }
    let response = response.on_hover_text(tooltip);
    if response.clicked() {
        outbox.push(Message::EmojiInserted(glyph.to_owned()));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, RawInput, Rect};
    use std::collections::HashSet;

    /// 一帧:画面板(可带走本帧的网格单元矩形)。未探测(`None`)时预置
    /// 全量集 —— 既有断言(全表可见)因此不依赖真实字形(E3 口径:过滤
    /// 行为用注入集合测,真实字形交给真机目视,emoji-plan §7 #7)。
    fn frame(
        ctx: &egui::Context,
        state: &mut EmojiPanelState,
        events: Vec<Event>,
        outbox: Option<&mut Vec<Message>>,
    ) -> Vec<egui::Response> {
        let (cells, output) = frame_output(ctx, state, events, outbox);
        output.drop_without_applying_deltas();
        cells
    }

    /// 同 `frame`,但连本帧的 `FullOutput` 一起带走(A2 的纹理命中计数读
    /// `textures_delta`,是引擎层证据);调用方自行 drop 纹理增量。
    fn frame_output(
        ctx: &egui::Context,
        state: &mut EmojiPanelState,
        events: Vec<Event>,
        outbox: Option<&mut Vec<Message>>,
    ) -> (Vec<egui::Response>, egui::FullOutput) {
        if state.glyphs.is_none() {
            state.glyphs = Some(emoji_data::GlyphSet::all());
        }
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 800.0));
        let mut cells = Vec::new();
        let mut sink = Vec::new();
        let outbox = outbox.unwrap_or(&mut sink);
        let output = ctx.run_ui(
            RawInput {
                events,
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| cells = panel(ui, state, outbox),
        );
        (cells, output)
    }

    fn click(pos: egui::Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// 在 `pos` 处完成一次「移动 → 按下 → 抬起」。
    fn click_at(
        ctx: &egui::Context,
        state: &mut EmojiPanelState,
        pos: egui::Pos2,
        outbox: &mut Vec<Message>,
    ) {
        for events in [
            vec![Event::PointerMoved(pos)],
            vec![click(pos, true)],
            vec![click(pos, false)],
        ] {
            frame(ctx, state, events, Some(outbox));
        }
    }

    /// 明暗两套 visuals 各渲染三帧不 panic(首帧字体注册、后续帧
    /// tessellation 各有冷启动路径,单帧绿不等于帧帧绿,与 icons.rs 的
    /// 手法同款);渲染本身不发自发消息。
    #[test]
    fn panel_renders_in_both_visuals_without_messages() {
        for dark in [true, false] {
            let ctx = egui::Context::default();
            if !dark {
                ctx.set_theme(egui::Theme::Light);
            }
            let mut state = EmojiPanelState::default();
            let mut outbox = Vec::new();
            for _ in 0..3 {
                frame(&ctx, &mut state, Vec::new(), Some(&mut outbox));
            }
            assert!(outbox.is_empty(), "仅渲染不产生消息");
        }
    }

    /// 点网格单元发 `EmojiInserted`(载荷 = 该单元的字符),分类标签可
    /// 切换 —— 不断言渲染结果(字形依赖真实字体),只钉「点击 → 消息」
    /// 与「标签 → 网格换内容」两条链路(emoji-plan §7 #7 的口径)。
    #[test]
    fn clicking_a_cell_and_switching_group_emit_messages() {
        let ctx = egui::Context::default();
        let mut state = EmojiPanelState {
            open: true,
            ..EmojiPanelState::default()
        };
        let mut outbox = Vec::new();

        // sizing pass 三遍(浮窗 widget 前几遍不参与命中),第四遍取
        // 首分类(表情)网格首格的矩形
        let mut first_cell = Rect::NOTHING;
        for step in 0..4 {
            let cells = frame(&ctx, &mut state, Vec::new(), None);
            if step == 3 {
                if let Some(first) = cells.first() {
                    first_cell = first.rect;
                }
            }
        }
        let center = first_cell.center();
        assert!(center.x > 0.0, "拿到了网格首格的位置:{center:?}");

        click_at(&ctx, &mut state, center, &mut outbox);
        assert_eq!(
            outbox,
            vec![Message::EmojiInserted("😀".to_owned())],
            "点首格(笑脸)发出插入消息"
        );

        // 切到「旗帜」分类:网格换表,同一位置点下去的载荷应是旗帜首项
        outbox.clear();
        state.group = 7;
        let cells = frame(&ctx, &mut state, Vec::new(), None);
        let center = cells.first().expect("旗帜分类有网格").rect.center();
        click_at(&ctx, &mut state, center, &mut outbox);
        assert_eq!(
            outbox,
            vec![Message::EmojiInserted("🇨🇳".to_owned())],
            "分类切换后网格换成旗帜表"
        );
    }

    /// Esc 关面板:面板开着时按 Esc 发 `EmojiPickerToggle(false)`。
    #[test]
    fn escape_closes_the_panel() {
        let ctx = egui::Context::default();
        let mut state = EmojiPanelState {
            open: true,
            ..EmojiPanelState::default()
        };
        let mut outbox = Vec::new();
        for events in [
            Vec::new(),
            Vec::new(),
            vec![Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        ] {
            frame(&ctx, &mut state, events, Some(&mut outbox));
        }
        assert_eq!(outbox, vec![Message::EmojiPickerToggle(false)]);
    }

    /// 搜索(E2):空查询 = 当前分类全表;非空 = 跨分类三路匹配(当前
    /// 分类是「表情」,「火箭」命中旅行分类 —— 单元数 1 证明网格已换源);
    /// 无命中零单元(「无匹配」提示无点击目标)。
    #[test]
    fn search_filters_the_grid_across_groups() {
        let ctx = egui::Context::default();
        let mut state = EmojiPanelState {
            open: true,
            ..EmojiPanelState::default()
        };

        let cells = frame(&ctx, &mut state, Vec::new(), None);
        assert_eq!(
            cells.len(),
            emoji_data::GROUPS[0].entries.len(),
            "空查询显示当前分类全表"
        );

        state.query = "火箭".to_owned();
        let cells = frame(&ctx, &mut state, Vec::new(), None);
        assert_eq!(cells.len(), 1, "跨分类命中唯一:{cells:?}");

        state.query = "查无此物xyz".to_owned();
        let cells = frame(&ctx, &mut state, Vec::new(), None);
        assert!(cells.is_empty(), "无匹配不给可点单元");
    }

    /// 搜索态点选命中单元:发出该枚字符的插入消息(ScrollArea 需要更多
    /// sizing pass,取第 6 帧的单元矩形定位;与 E1 的 4 帧 + 探针同一手法)。
    #[test]
    fn search_result_click_inserts_the_hit() {
        let ctx = egui::Context::default();
        let mut state = EmojiPanelState {
            open: true,
            query: "火箭".to_owned(),
            ..EmojiPanelState::default()
        };
        let mut first = Rect::NOTHING;
        for step in 0..6 {
            let cells = frame(&ctx, &mut state, Vec::new(), None);
            if step == 5 {
                if let Some(cell) = cells.first() {
                    first = cell.rect;
                }
            }
        }
        let center = first.center();
        assert!(center.x > 0.0, "拿到了命中单元的位置:{center:?}");

        let mut outbox = Vec::new();
        click_at(&ctx, &mut state, center, &mut outbox);
        assert_eq!(outbox, vec![Message::EmojiInserted("🚀".to_owned())]);
    }

    /// 「最近使用」行(E2):空态整行隐藏(单元数 = 网格数);有记录时
    /// 尾追可点单元,点选发同样的插入消息 —— 连插多个的二次入口。
    #[test]
    fn recent_row_hides_when_empty_and_inserts_on_click() {
        let ctx = egui::Context::default();
        let mut state = EmojiPanelState {
            open: true,
            ..EmojiPanelState::default()
        };
        let cells = frame(&ctx, &mut state, Vec::new(), None);
        assert_eq!(
            cells.len(),
            emoji_data::GROUPS[0].entries.len(),
            "空态无最近使用行"
        );

        state.recent = vec!["🚀".to_owned(), "🎉".to_owned()];
        let mut last = Rect::NOTHING;
        for step in 0..6 {
            let cells = frame(&ctx, &mut state, Vec::new(), None);
            assert_eq!(
                cells.len(),
                emoji_data::GROUPS[0].entries.len() + 2,
                "第 {step} 帧:网格之外多出两枚最近单元"
            );
            if step == 5 {
                last = cells.last().expect("最近单元存在").rect;
            }
        }
        let mut outbox = Vec::new();
        click_at(&ctx, &mut state, last.center(), &mut outbox);
        assert_eq!(
            outbox,
            vec![Message::EmojiInserted("🎉".to_owned())],
            "点的是最后一枚(🚀 之后的 🎉)"
        );
    }

    /// E3 过滤(注入集合,不碰真实字形,emoji-plan §7 #7):白名单只放行
    /// 一枚 → 网格只剩它且点选发它;空集 → 全分类占位零单元、搜索态命中
    /// 全被剔也走「无匹配」零单元 —— 豆腐块没有任何可渲染入口。
    #[test]
    fn glyph_filter_prunes_grid_and_shows_placeholder() {
        let ctx = egui::Context::default();
        let mut only_first = emoji_data::GlyphSet::default();
        only_first.insert("😀");
        let mut state = EmojiPanelState {
            open: true,
            glyphs: Some(only_first),
            ..EmojiPanelState::default()
        };

        // 浮窗 sizing 多轮后才取矩形点击(与 clicking_a_cell 同款手法)
        let mut first = Rect::NOTHING;
        for step in 0..4 {
            let cells = frame(&ctx, &mut state, Vec::new(), None);
            assert_eq!(cells.len(), 1, "第 {step} 帧白名单只放行首枚:{cells:?}");
            if step == 3 {
                first = cells[0].rect;
            }
        }
        let mut outbox = Vec::new();
        click_at(&ctx, &mut state, first.center(), &mut outbox);
        assert_eq!(outbox, vec![Message::EmojiInserted("😀".to_owned())]);

        // 空集:分类页与搜索态都零单元(占位文案无点击目标)
        state.glyphs = Some(emoji_data::GlyphSet::default());
        state.query.clear();
        assert!(
            frame(&ctx, &mut state, Vec::new(), None).is_empty(),
            "分类全剔无单元"
        );
        state.group = 4; // 换个分类,占位路径同款
        assert!(
            frame(&ctx, &mut state, Vec::new(), None).is_empty(),
            "任意分类同款占位"
        );
        state.group = 0;
        state.query = "笑脸".to_owned(); // 有文本命中,但全被剔
        assert!(
            frame(&ctx, &mut state, Vec::new(), None).is_empty(),
            "搜索命中全剔无单元"
        );
    }

    /// E3:「最近使用」同受白名单过滤 —— 历史记录里缺字形的不再回到
    /// 面板(豆腐块不从这条缝漏回来),放行的照常可点。
    #[test]
    fn glyph_filter_prunes_the_recent_row() {
        let ctx = egui::Context::default();
        let mut allow_first = emoji_data::GlyphSet::default();
        allow_first.insert("😀");
        let mut state = EmojiPanelState {
            open: true,
            glyphs: Some(allow_first),
            recent: vec!["🚀".to_owned(), "😀".to_owned()],
            ..EmojiPanelState::default()
        };

        let mut last = Rect::NOTHING;
        for step in 0..6 {
            let cells = frame(&ctx, &mut state, Vec::new(), None);
            assert_eq!(
                cells.len(),
                2,
                "第 {step} 帧:同一白名单下网格剩 😀、最近行剩 😀(🚀 两处都被剔)"
            );
            if step == 5 {
                last = cells.last().expect("recent 单元存在").rect;
            }
        }
        let mut outbox = Vec::new();
        click_at(&ctx, &mut state, last.center(), &mut outbox);
        assert_eq!(outbox, vec![Message::EmojiInserted("😀".to_owned())]);
    }

    /// E3 核验契约:出厂 NotoEmoji 的 cmap fmt12 剔除清单**恰好**是这
    /// 7 枚「裸码位 + FE0F」的文本表现条目(独立脚本按同口径复核:4 枚
    /// 增补平面 fmt12/fmt4 全无,3 枚 BMP 仅 fmt4 有而 harfrust 选表只认
    /// fmt12)—— #52 入库清洗的漏网,由本防线兜住。把清单写死:egui
    /// 升级致覆盖变化时此处红 = 期望中的警报,按 #52 口径重核后再更新
    /// 本清单。断言的不是「碰真实渲染字体」,纯静态字节 + 纯函数,零抖动。
    #[test]
    fn factory_font_cmap12_prunes_exactly_the_text_presentation_entries() {
        let defs = egui::FontDefinitions::default();
        let bytes = defs
            .font_data
            .get(NOTO_EMOJI_FONT)
            .expect("出厂字体链恒含 NotoEmoji(egui default_fonts)")
            .font
            .as_ref();
        let set = emoji_data::GlyphSet::from_font_cmap12(bytes);
        assert!(
            !set.is_empty(),
            "cmap 核验空结果:解析器或出厂字体键名失效(生产侧此情走全量放行兜底)"
        );
        // 侦探 / 台式机 / 键盘 / 鼠标 / 地图 / 沙滩伞 / 鸟居
        let pruned: &[&str] = &["🕵️", "🖥️", "⌨️", "🖱️", "🗺️", "⛱️", "⛩️"];
        let missing: Vec<&str> = emoji_data::GROUPS
            .iter()
            .flat_map(|g| g.entries.iter())
            .map(|e| e.char)
            .filter(|glyph| !set.allows(glyph))
            .collect();
        assert_eq!(missing, pruned, "剔除清单漂移:按 #52 口径重核后更新此清单");
    }

    /// E3 兜底口径:出厂链上 `probe_glyphs` 的产出 = 全表减 7 枚 fmt12
    /// 未覆盖条目(见 `factory_font_cmap12_prunes_exactly_the_text_presentation_entries`);
    /// 失败路径(键名缺失/坏字节 → 空集 → 全量放行)由 emoji_data 的
    /// 合成字节测试覆盖。
    #[test]
    fn probe_glyphs_covers_table_on_factory_chain() {
        assert_eq!(
            probe_glyphs().iter().count(),
            emoji_data::GROUPS
                .iter()
                .map(|g| g.entries.len())
                .sum::<usize>()
                - 7,
            "出厂链上核验结果 = 全表 − 7 枚文本表现条目"
        );
    }

    /// 本帧上传的 emoji 纹理数(textures_delta 里 72×72 的整图增量;出厂
    /// 字体图集不是 72×72,天然排除)。纹理命中枚计数的引擎层口径。
    fn emoji_texture_loads(output: &egui::FullOutput) -> usize {
        output
            .textures_delta
            .set
            .values()
            .flatten()
            .filter(|delta| delta.image.size() == [72, 72])
            .count()
    }

    /// 本帧上传了 72×72 emoji 纹理的纹理 id(对照画面上的 image mesh)。
    fn emoji_texture_ids(output: &egui::FullOutput) -> HashSet<egui::TextureId> {
        output
            .textures_delta
            .set
            .iter()
            .filter(|(_, deltas)| deltas.iter().any(|d| d.image.size() == [72, 72]))
            .map(|(id, _)| *id)
            .collect()
    }

    /// A2 资产表契约:272 枚与数据表 1:1(include_bytes 编译期钉住,挪走
    /// / 改名即编译红),且每枚都真能解码成 72×72 —— fetch.sh --verify 的
    /// 魔数校验之上的进程内全量复核。
    #[test]
    fn bundled_assets_cover_the_whole_table_and_decode() {
        assert_eq!(TWEMOJI.len(), 272, "资产表与钉死清单同规模");
        let mut seen = HashSet::new();
        for entry in emoji_data::GROUPS.iter().flat_map(|g| g.entries.iter()) {
            let png = twemoji_png(entry.char)
                .unwrap_or_else(|| panic!("{}(:{}) 无随包资产", entry.name_zh, entry.shortcode));
            assert!(seen.insert(entry.char), "{} 重复入库", entry.shortcode);
            let img = image::load_from_memory(png).unwrap_or_else(|e| {
                panic!("{}(:{}) 资产解码失败:{e}", entry.name_zh, entry.shortcode)
            });
            assert_eq!(
                (img.width(), img.height()),
                (72, 72),
                "{} 非 72×72",
                entry.shortcode
            );
        }
    }

    /// A2 彩色路径(无头):首帧每个实显枚恰好上传一张 72×72 纹理
    /// (命中枚计数 ≥ 实显枚数;全屏可见时取等,缓存保证不重复上传),
    /// 画面上发出绑定这些纹理的 image mesh;第二帧零新增 —— 会话缓存
    /// 命中,不重复解码。明暗两套 visuals 各跑一遍。
    #[test]
    fn panel_loads_one_texture_per_displayed_glyph_and_reuses_it() {
        for dark in [true, false] {
            let ctx = egui::Context::default();
            if !dark {
                ctx.set_theme(egui::Theme::Light);
            }
            let mut state = EmojiPanelState {
                open: true,
                recent: vec!["🚀".to_owned(), "🎉".to_owned()],
                ..EmojiPanelState::default()
            };
            // 实显 distinct 口径:表情分类全表 + 最近两枚(分属旅行/物品,
            // 不与网格重);总数恰为 distinct(无重复上传)
            let displayed: HashSet<&str> = emoji_data::GROUPS[0]
                .entries
                .iter()
                .map(|e| e.char)
                .chain(["🚀", "🎉"])
                .collect();

            // 浮窗首几帧是 sizing pass(不真正绘制,与点击测试的「多帧后
            // 取矩形」同一口径),连跑 3 帧取累计命中 —— 缓存保证每枚
            // distinct 只上传一次,跨帧累计恰等于 distinct
            let mut loaded = 0;
            let mut meshes = 0;
            for _ in 0..3 {
                let (cells, out) = frame_output(&ctx, &mut state, Vec::new(), None);
                assert_eq!(
                    cells.len(),
                    displayed.len(),
                    "实显单元数与 distinct 口径一致"
                );
                loaded += emoji_texture_loads(&out);
                let textured = emoji_texture_ids(&out);
                meshes += out
                    .shapes
                    .iter()
                    .filter(|clipped| match &clipped.shape {
                        egui::Shape::Mesh(mesh) => textured.contains(&mesh.texture_id),
                        _ => false,
                    })
                    .count();
                out.drop_without_applying_deltas();
            }
            assert!(
                loaded >= displayed.len(),
                "纹理命中 {loaded} < 实显 {}:彩色应覆盖全部实显枚",
                displayed.len()
            );
            assert_eq!(loaded, displayed.len(), "每枚 distinct 会话内恰上传一次");
            assert!(
                meshes >= displayed.len(),
                "image mesh {meshes} < 实显 {}:实显单元都应画纹理",
                displayed.len()
            );

            // 第 4 帧:缓存全命中,零重复上传
            let (_, fourth) = frame_output(&ctx, &mut state, Vec::new(), None);
            assert_eq!(emoji_texture_loads(&fourth), 0, "会话缓存命中,不再解码上传");
            fourth.drop_without_applying_deltas();
        }
    }

    /// A2 失败面:坏 PNG 字节(损坏资产口径)解码得 `None` 不 panic;
    /// 表外字符(资产缺失口径)照常渲染且不产生纹理 —— 该单元回落黑白,
    /// 点选载荷仍是原始 Unicode。与 E3「数据保留、渲染兜底」同哲学。
    #[test]
    fn corrupt_or_missing_assets_fall_back_without_panic() {
        let ctx = egui::Context::default();
        assert!(decode_texture(&ctx, "😀", b"not a png").is_none());
        assert!(decode_texture(&ctx, "😀", &[]).is_none());
        let full = twemoji_png("😀").expect("😀 有随包资产");
        assert!(
            decode_texture(&ctx, "😀", &full[..40]).is_none(),
            "截断的真 PNG 同样回落"
        );

        // 资产缺失:🫠(Emoji 14)不在 272 枚清单 → 走回落绘制路径。
        // E3 白名单管「字符可写文档」,与资产有无正交,故注入放行 🫠 的
        // 集合(表内全量 + 🫠)——「白名单放行、资产缺失」正是回落路径的
        // 真实形态。面板照常渲、单元照常可点、不产生纹理(浮窗前几帧
        // sizing 与点击测试同款,多帧累计口径)
        assert!(twemoji_png("🫠").is_none());
        let ctx = egui::Context::default();
        let mut glyphs = emoji_data::GlyphSet::all();
        glyphs.insert("🫠");
        let mut state = EmojiPanelState {
            open: true,
            glyphs: Some(glyphs),
            recent: vec!["🫠".to_owned()],
            ..EmojiPanelState::default()
        };
        let mut last = Rect::NOTHING;
        let mut loaded = 0;
        for step in 0..6 {
            let (cells, out) = frame_output(&ctx, &mut state, Vec::new(), None);
            assert_eq!(
                cells.len(),
                emoji_data::GROUPS[0].entries.len() + 1,
                "第 {step} 帧:回落单元仍在(网格 + 最近行)"
            );
            loaded += emoji_texture_loads(&out);
            if step == 5 {
                last = cells.last().expect("回落单元存在").rect;
            }
            out.drop_without_applying_deltas();
        }
        assert_eq!(
            loaded,
            emoji_data::GROUPS[0].entries.len(),
            "有资产的网格枚各自上传一次,🫠 不产生任何纹理"
        );

        let mut outbox = Vec::new();
        click_at(&ctx, &mut state, last.center(), &mut outbox);
        assert_eq!(
            outbox,
            vec![Message::EmojiInserted("🫠".to_owned())],
            "回落单元载荷仍是原始 Unicode"
        );
    }
}
