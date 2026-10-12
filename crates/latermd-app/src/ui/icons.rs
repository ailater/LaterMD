//! 自绘线性图标体系(docs/ui-polish.md §3)。
//!
//! **为什么自绘而不是字体字符**:egui 无内置图标集,emoji / Unicode 符号
//! (✎ 🗋 ⌘)在三平台缺字风险真实(AGENTS.md §5 已把字体列为风险项)。全部
//! 图标用 `Painter` 画线段/圆/矩形:零字体依赖、零纹理依赖、随主题取色、
//! 在高 DPI 下不会因位图缩放糊掉。
//!
//! 坐标约定:以 `size` 为基准的 `[-0.5, 0.5]` 归一化空间,线宽 `size / 12`
//! (16px 图标 = 1.33px 线)。改 `size` 即整体等比缩放,不存在第二套坐标。

use crate::ui::tokens::{ICON, RADIUS_SM, SPACE_SM, SPACE_XS, TOOLBAR_H};
use eframe::egui::{self, Align2, Color32, Painter, Pos2, Rect, Stroke, Vec2};

/// 图标标识。新增图标 = 加枚举变体 + [`Icon::draw`] 一个分支,调用方无需
/// 感知绘制细节。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// 新建:文档 + 右上加号。
    New,
    /// 打开:文件夹。
    Open,
    /// 保存:软盘。
    Save,
    /// 另存为:软盘 + 右下箭头。
    SaveAs,
    /// 导出:向下箭头入托盘。
    Export,
    /// 打印 / 生成 PDF:机身 + 出纸口 + 托出一张纸。
    ///
    /// **2026-10-08 新增**（症状 B）：原先 `ExportHtml` 与 `ExportPdf`
    /// **共用 `Icon::Export`**，而左栏顶段是 6 枚**纯图标无文字**并排 ——
    /// 相邻两枚长得一模一样，tooltip 是唯一区分手段，实际不可辨识。
    /// 现按动作语义拆开：HTML = 「下载一个文件」（沿用 `Export` 箭头入托盘），
    /// PDF = 「打印成 PDF」（本图标）。两者本就是不同动作，拆开不损语义。
    Print,
    /// 侧边栏:外框 + 左侧实心条。
    Sidebar,
    /// 主题:太阳。
    Theme,
    /// AI:四角星(与 `ai://` 链接色同源的语义)。
    Ai,
    /// 文件页签:叠放文档。
    Files,
    /// 搜索页签:放大镜。
    Search,
    /// 大纲页签:缩进列表。
    Outline,
    /// Git 页签:分支图。
    Git,
    /// 设置:齿轮。
    Settings,
    /// 重置:回环箭头。
    Reset,
    /// 关右栏:外框 + 右侧实心条(左栏用既有 `Sidebar` 镜像)。
    PanelRight,
    /// 禅定:同心圆。
    Zen,
    /// 最小化:一条横线。
    Minimize,
    /// 最大化:方框。
    Maximize,
    /// 还原:前后错位的两个方框(后框只画上、右两边)。
    Restore,
    /// 关闭:叉。
    Close,
    // —— Markdown 格式工具条(docs/ui-shell-redesign.md §6.1)——
    /// 行内代码:左右两枚尖角 `‹ ›`。
    CodeInline,
    /// 代码块:`‹ ›` 加外框起伏。
    CodeBlock,
    /// 链接:两枚斜置的链环。
    Link,
    /// 引用:左侧竖条 + 两条文字线。
    Quote,
    /// 分割线:中间一条贯穿横线。
    Divider,
    /// 表格:2×2 网格外框 + 表头横线。
    Table,
    /// 正文(去前缀):四条长短不一的文字线,**没有**左竖条 —— 与 `Quote`
    /// 的唯一区别就是那根条,一眼能看出「退出引用」。
    ///
    /// 规格 §6.1 原本写的是 Unicode `¶`;这里改自绘:文首「图标是矢量自绘,
    /// 不是字体字符」(ui-polish §1.1)对 Gecko/缺字环境的顾虑同样适用。
    Paragraph,
    /// 图片(docs/image-plan.md A 段):外框 + 山 + 日(右上小圆)。
    ///
    /// 与 `Table`(网格)的区分靠「框内是折线不是横竖分割线」;与 `Open`
    /// (文件夹)的区分靠右上角那个实心日 —— 都是「外框 + 内部图形」,少了
    /// 太阳就退化成文件夹。
    Image,
    /// 无序列表:三点 + 三线。
    BulletList,
    /// 有序列表:三条竖短线(序号笔画抽象)+ 三线。
    OrderedList,
    /// 任务列表:勾选框两个 + 两条线,首框带勾。
    TaskList,
    // —— 文件树(docs/auto-plan.md #32 I1)——
    /// 文件树目录(收起):经典闭合文件夹。
    FolderClosed,
    /// 文件树目录(展开):开口文件夹 —— 前盖下移外张,与闭合形一眼可分。
    FolderOpen,
    /// 文件树文件:折角纸页。
    File,
    // —— Emoji 面板入口(docs/emoji-plan.md E1)——
    /// 表情:外圆脸 + 两实心点眼 + 下弯弧嘴(自绘笑脸)。
    ///
    /// 这是**UI 图标**,不是文档内容 —— 插进文档的 emoji 是内容不受
    /// ui-polish §1.1 约束,但工具条按钮本身必须是自绘,不是 `😀` 字符
    /// (emoji-plan §3 的边界)。
    Emoji,
    // —— 代码块复制头(docs/auto-plan.md #38)——
    /// 复制:两枚错位矩形(后框被前框遮住的右、下两边不画,仿 `Restore`
    /// 的遮挡画法)。
    Copy,
    /// 已复制(✓ 反馈态):短下挑 + 长上挑两条线段。
    Check,
    // —— 侧栏搜索替换行 ——
    /// 全部替换:右上空心框(被搜的原文)+ 左下实心块(替换写入),对角
    /// 错位。与 `Copy`/`Restore` 的区分靠那枚实心块 —— 纯线条错位双框
    /// 已经被复制/还原占用,一空一实才是「写入替换」的语义。
    ReplaceAll,
}

impl Icon {
    /// 在 `center` 处以 `size` 边长画图标,颜色由调用方给(通常取 visuals
    /// 前景色,禁用态取 weak)。
    pub fn draw(self, painter: &Painter, center: Pos2, size: f32, color: Color32) {
        let stroke = Stroke::new((size / 12.0).max(1.0), color);
        // 归一化坐标 → 屏幕坐标
        let at = |x: f32, y: f32| center + Vec2::new(x * size, y * size);
        let seg = |a: (f32, f32), b: (f32, f32)| {
            painter.line_segment([at(a.0, a.1), at(b.0, b.1)], stroke)
        };
        let path = |pts: &[(f32, f32)]| {
            for pair in pts.windows(2) {
                seg(pair[0], pair[1]);
            }
        };
        let frame = |a: (f32, f32), b: (f32, f32)| {
            painter.rect_stroke(
                Rect::from_two_pos(at(a.0, a.1), at(b.0, b.1)),
                size * 0.08,
                stroke,
                egui::StrokeKind::Outside,
            );
        };
        let fill = |a: (f32, f32), b: (f32, f32)| {
            painter.rect_filled(
                Rect::from_two_pos(at(a.0, a.1), at(b.0, b.1)),
                size * 0.06,
                color,
            );
        };
        let dot = |c: (f32, f32), r: f32| painter.circle_filled(at(c.0, c.1), r * size, color);
        let ring = |c: (f32, f32), r: f32| painter.circle_stroke(at(c.0, c.1), r * size, stroke);
        // 四角星:`k` 为缩放。
        let star = |c: (f32, f32), k: f32| {
            let (x, y) = c;
            path(&[
                (x, y - 0.42 * k),
                (x + 0.13 * k, y - 0.13 * k),
                (x + 0.42 * k, y),
                (x + 0.13 * k, y + 0.13 * k),
                (x, y + 0.42 * k),
                (x - 0.13 * k, y + 0.13 * k),
                (x - 0.42 * k, y),
                (x - 0.13 * k, y - 0.13 * k),
                (x, y - 0.42 * k),
            ]);
        };
        // 折线近似圆弧(egui 无 arc API);返回终点坐标,箭头用它接续。
        let arc = |c: (f32, f32), r: f32, a0: f32, a1: f32| -> (f32, f32) {
            let steps = 18usize;
            let point = |i: usize| {
                let t = a0 + (a1 - a0) * (i as f32 / steps as f32);
                (c.0 + r * t.cos(), c.1 + r * t.sin())
            };
            for i in 0..steps {
                seg(point(i), point(i + 1));
            }
            point(steps)
        };
        // 极坐标取点(齿轮齿圈用;中心在原点,调用方自行平移)
        let polar = |r: f32, a: f32| (r * a.cos(), r * a.sin());

        match self {
            Self::New => {
                frame((-0.42, -0.40), (0.02, 0.42));
                seg((-0.30, -0.14), (-0.08, -0.14));
                seg((-0.30, 0.06), (-0.08, 0.06));
                seg((0.20, 0.02), (0.44, 0.02));
                seg((0.32, -0.10), (0.32, 0.14));
            }
            // 工具栏「打开」与文件树收起目录共用闭合文件夹几何:闭合文件夹
            // 就是这把形状,分两套坐标只会画出两个略有出入的文件夹。
            Self::Open | Self::FolderClosed => {
                frame((-0.42, -0.22), (0.42, 0.38));
                path(&[
                    (-0.42, -0.22),
                    (-0.42, -0.40),
                    (-0.12, -0.40),
                    (-0.04, -0.22),
                ]);
            }
            Self::Save => {
                frame((-0.42, -0.40), (0.42, 0.40));
                frame((-0.18, -0.40), (0.18, -0.04));
                seg((-0.24, 0.20), (0.24, 0.20));
                seg((-0.24, 0.32), (0.24, 0.32));
            }
            Self::SaveAs => {
                // 软盘缩小让位给右下箭头,两者不重叠
                frame((-0.46, -0.40), (-0.02, 0.34));
                frame((-0.28, -0.40), (-0.06, -0.10));
                seg((-0.32, 0.14), (-0.08, 0.14));
                seg((0.08, 0.02), (0.44, 0.40));
                seg((0.44, 0.40), (0.20, 0.40));
                seg((0.44, 0.40), (0.44, 0.16));
            }
            Self::Export => {
                seg((0.0, -0.42), (0.0, 0.10));
                path(&[(-0.20, -0.08), (0.0, 0.12), (0.20, -0.08)]);
                seg((-0.36, 0.30), (0.36, 0.30));
            }
            // 打印 / PDF:机身 + 托出的纸 + 状态点。**刻意不与 `Export`
            // 共享任何笔画** —— 那正是本图标存在的原因(见枚举处注释):
            // 两枚相邻的纯图标按钮长得一样就等于没有图标。
            Self::Print => {
                // 机身(横宽矩形)
                frame((-0.38, -0.22), (0.38, 0.14));
                // 托出的纸(机身下方的竖窄矩形,略出头)
                frame((-0.20, 0.14), (0.20, 0.46));
                // 机身右上的状态点:给纯线条加一处实心,16px 下才不至于
                // 与 `FolderClosed` 那种「两矩形」形状混在一起
                fill((0.22, -0.14), (0.32, -0.04));
            }
            Self::Sidebar => {
                frame((-0.44, -0.36), (0.44, 0.36));
                fill((-0.44, -0.36), (-0.24, 0.36));
            }
            Self::Theme => {
                ring((0.0, 0.0), 0.15);
                // 六条射线,每 60°
                for step in 0..6 {
                    let angle = std::f32::consts::PI / 3.0 * step as f32;
                    let (cos, sin) = (angle.cos(), angle.sin());
                    seg((cos * 0.26, sin * 0.26), (cos * 0.40, sin * 0.40));
                }
            }
            Self::Ai => {
                star((0.02, 0.04), 1.0);
                star((0.32, -0.28), 0.42);
            }
            Self::Files => {
                frame((-0.06, -0.36), (0.42, 0.34));
                frame((-0.42, -0.28), (0.06, 0.42));
            }
            Self::Search => {
                ring((-0.08, -0.08), 0.20);
                seg((0.06, 0.06), (0.36, 0.36));
            }
            Self::Outline => {
                for row in [-0.28_f32, 0.0, 0.28] {
                    dot((-0.30, row), 0.05);
                    seg((-0.14, row), (0.38, row));
                }
            }
            Self::Git => {
                dot((-0.28, -0.28), 0.11);
                dot((-0.28, 0.30), 0.11);
                dot((0.28, 0.02), 0.11);
                seg((-0.28, -0.28), (-0.28, 0.30));
                seg((-0.28, 0.12), (0.28, 0.02));
            }
            Self::Settings => {
                // 标准齿轮(2026-09-30 用户反馈:原「环形 + 四正向辐条」呈
                // 十字星芒,不像齿轮):8 齿波浪外圈 + 中心孔。外圈半径随
                // 角度按余弦在齿顶(0.44)/齿根(0.28)两档间起伏,4 齿正向、
                // 4 齿斜向;平滑轮廓没有尖角,描边不会在拐点长出斜接凸块,
                // 齿形在 ICON_SM 档也不糊成圆。中心孔 ring 与外圈间的空档
                // 守住「齿轮」而非「圆盘/太阳」的辨识(Theme 太阳是细长
                // 射线,与此区分)。
                let (r_tip, r_root, r_hole) = (0.44, 0.28, 0.12);
                let amp = (r_tip - r_root) / 2.0;
                let steps = 12usize; // 每齿 12 段折线,段间转角小
                let mut pts = Vec::with_capacity(8 * steps + 1);
                for i in 0..8 * steps {
                    let a = i as f32 * std::f32::consts::TAU / (8.0 * steps as f32);
                    pts.push(polar(r_root + amp * (1.0 + (8.0 * a).cos()), a));
                }
                pts.push(pts[0]);
                path(&pts);
                ring((0.0, 0.0), r_hole);
            }
            Self::Reset => {
                // 3/4 圆弧 + 末端箭头:260° → -40°(顺时针收口)
                //
                // **半径 0.455 是实测标定出来的,不是随手取的**:圆环的
                // 固有直径只有同排满幅方形图标的 ~0.75 —— 原始 0.32 实测
                // 视觉宽 11.5px,而 `Open` 是 16.1px,并排时右边那枚看着
                // 缩了一号。按实测放大到 0.455 后视觉宽 15.8px,与满幅
                // 方形同档(差 0.3px)。标定值钉在
                // `icons::tests::icons_in_a_row_share_optical_size`,
                // 改半径而不同步那条断言会当场红。
                let end = arc(
                    (0.0, 0.0),
                    0.455,
                    std::f32::consts::PI * 1.45,
                    std::f32::consts::PI * 3.05,
                );
                // 箭头按同比例放大,否则圆变大了、箭头还是原来的大小,
                // 收口处会显得头重脚轻。
                seg(end, (end.0 + 0.03, end.1 + 0.285));
                seg(end, (end.0 + 0.285, end.1 - 0.06));
            }
            Self::PanelRight => {
                frame((-0.44, -0.36), (0.44, 0.36));
                fill((0.24, -0.36), (0.44, 0.36));
            }
            Self::Zen => {
                ring((0.0, 0.0), 0.30);
                dot((0.0, 0.0), 0.09);
            }
            Self::Minimize => {
                seg((-0.30, 0.0), (0.30, 0.0));
            }
            Self::Maximize => {
                frame((-0.28, -0.28), (0.28, 0.28));
            }
            Self::Restore => {
                // 后框被前框遮住的左、下两边不画,保持「还原」辨识度
                path(&[(-0.06, -0.34), (0.34, -0.34), (0.34, 0.06)]);
                frame((-0.34, -0.06), (0.06, 0.34));
            }
            Self::Close => {
                seg((-0.28, -0.28), (0.28, 0.28));
                seg((-0.28, 0.28), (0.28, -0.28));
            }
            // —— 格式工具条:全部线段/圆点自绘,零字形依赖(ui-polish §1.1)——
            Self::CodeInline => {
                path(&[(-0.10, -0.26), (-0.30, 0.0), (-0.10, 0.26)]);
                path(&[(0.10, -0.26), (0.30, 0.0), (0.10, 0.26)]);
            }
            Self::CodeBlock => {
                frame((-0.42, -0.34), (0.42, 0.34));
                path(&[(-0.10, -0.16), (-0.24, 0.0), (-0.10, 0.16)]);
                path(&[(0.10, -0.16), (0.24, 0.0), (0.10, 0.16)]);
            }
            Self::Link => {
                // 两枚斜置 oval(用矩形缺角近似),中间一横连接
                frame((-0.40, -0.18), (-0.06, 0.18));
                frame((0.06, -0.18), (0.40, 0.18));
                seg((-0.06, 0.0), (0.06, 0.0));
            }
            Self::Quote => {
                fill((-0.40, -0.26), (-0.30, 0.26));
                seg((-0.16, -0.14), (0.40, -0.14));
                seg((-0.16, 0.14), (0.28, 0.14));
            }
            Self::Divider => {
                seg((-0.44, 0.0), (0.44, 0.0));
                seg((-0.36, -0.22), (0.36, -0.22));
                seg((-0.36, 0.22), (0.36, 0.22));
            }
            Self::Table => {
                frame((-0.42, -0.32), (0.42, 0.32));
                seg((-0.42, -0.08), (0.42, -0.08));
                seg((0.0, -0.32), (0.0, 0.32));
            }
            Self::Image => {
                frame((-0.44, -0.34), (0.44, 0.34));
                // 山:一折到底的两段线,落在框的下半部
                path(&[(-0.32, 0.18), (-0.08, -0.16), (0.14, 0.18)]);
                // 日:右上实心小圆(半径取线宽量级,大了会糊成一坨)
                dot((0.28, -0.16), 0.06);
            }
            Self::Paragraph => {
                for (row, stop) in [(-0.30_f32, 0.42), (-0.10, 0.42), (0.10, 0.42), (0.30, 0.20)] {
                    seg((-0.42, row), (stop, row));
                }
            }
            Self::BulletList => {
                for row in [-0.26_f32, 0.0, 0.26] {
                    dot((-0.34, row), 0.06);
                    seg((-0.18, row), (0.40, row));
                }
            }
            Self::OrderedList => {
                // 序号用抽象笔画:三条不等长短竖,不与具体字形绑定
                for (row, length) in [(-0.26_f32, 0.16), (0.0, 0.20), (0.26, 0.12)] {
                    path(&[
                        (-0.40, row - length / 2.0),
                        (-0.34, row - length / 2.0),
                        (-0.34, row + length / 2.0),
                    ]);
                    seg((-0.18, row), (0.40, row));
                }
            }
            Self::TaskList => {
                frame((-0.42, -0.24), (-0.14, 0.04));
                // 首框打勾
                path(&[(-0.38, -0.04), (-0.28, 0.04), (-0.16, -0.16)]);
                frame((-0.42, 0.16), (-0.14, 0.44));
                seg((-0.02, -0.10), (0.42, -0.10));
                seg((-0.02, 0.30), (0.42, 0.30));
            }
            // —— 文件树(#32 I1):开口文件夹与折角纸页(闭合 FolderClosed
            // 已并入上方 Open 分支)——
            Self::FolderOpen => {
                // 一笔连画:前盖(口在左端内收、右端外张)→ 底边 → 背板左沿
                // → 舌片 → 背板上沿 → 背板右沿收在前盖上沿上方,留出「口」
                path(&[
                    (-0.24, 0.10),
                    (-0.10, -0.08),
                    (0.34, -0.08),
                    (0.46, 0.14),
                    (0.32, 0.38),
                    (-0.34, 0.38),
                    (-0.46, 0.24),
                    (-0.46, -0.30),
                    (-0.36, -0.40),
                    (-0.10, -0.40),
                    (0.02, -0.30),
                    (0.30, -0.30),
                    (0.42, -0.14),
                ]);
            }
            Self::File => {
                // 折角纸页:右上角切角,补一小三角表示折进去的页角
                path(&[
                    (-0.26, -0.44),
                    (0.14, -0.44),
                    (0.26, -0.32),
                    (0.26, 0.44),
                    (-0.26, 0.44),
                    (-0.26, -0.44),
                ]);
                path(&[(0.14, -0.44), (0.14, -0.32), (0.26, -0.32)]);
            }
            // —— Emoji 面板入口:自绘笑脸(圆脸 + 两点眼 + 弧嘴,§3 边界)——
            Self::Emoji => {
                ring((0.0, 0.0), 0.42);
                dot((-0.15, -0.13), 0.055);
                dot((0.15, -0.13), 0.055);
                // 嘴是下弯弧:弧心在脸上方,画圆的下半段 —— 两端高、中间
                // 低,即经典笑口(egui 无 arc API,折线近似与 Reset 同款)
                arc(
                    (0.0, -0.10),
                    0.22,
                    std::f32::consts::PI * 0.25,
                    std::f32::consts::PI * 0.75,
                );
            }
            // —— 代码块复制头(#38):错位双矩形与对勾 ——
            Self::Copy => {
                // 前框(右下)完整;后框(左上)只画上边与左边,右、下
                // 两边落在前框内不画 —— 交叠处的线被「遮挡」是这个图标
                // 的全部立体感来源。
                frame((-0.10, -0.10), (0.42, 0.42));
                path(&[(0.10, -0.42), (-0.42, -0.42), (-0.42, 0.10)]);
            }
            Self::Check => {
                seg((-0.30, 0.02), (-0.08, 0.24));
                seg((-0.08, 0.24), (0.34, -0.22));
            }
            Self::ReplaceAll => {
                frame((-0.04, -0.44), (0.44, 0.04));
                fill((-0.44, -0.04), (0.04, 0.44));
            }
        }
    }
}

/// 纯图标按钮(左栏顶段动作、标题栏齿轮等):宽度只够一个图标,语义靠
/// tooltip 补。
pub fn icon_button(ui: &mut egui::Ui, icon: Icon, tooltip_text: &str) -> egui::Response {
    button_shell(ui, tooltip_text, |_, painter, rect, color| {
        icon.draw(painter, rect.center(), ICON, color);
    })
}

/// 窄一档的图标按钮(默认档 `ICON + 2 * SPACE_SM`,见 [`button_shell`] 里
/// 生成的矩形):给**宽度紧张的容器**用 —— 如文件栏根目录行,那一行还要
/// 挤一个目录名,三枚默认档按钮在 192px 侧栏里要占 50%,把目录名压到
/// 放不下常用文案(实测「未选择根目录」被截成「未…」)。
///
/// 高度不变(仍是 `TOOLBAR_H`),故只收横向留白;命中矩形随之变窄,
/// 调用方**必须同步更新自己的宽度预算**,否则会预留错。
pub fn narrow_icon_button(ui: &mut egui::Ui, icon: Icon, tooltip_text: &str) -> egui::Response {
    button_shell_narrow(ui, tooltip_text, |_, painter, rect, color| {
        icon.draw(painter, rect.center(), ICON, color);
    })
}

/// 「忙碌中」的图标按钮:图形换成 egui 内置 [`egui::Spinner`],转满一段
/// 窗口再落回原图标(见 [`crate::filetree::FileTreeState::begin_refresh`]
/// 里为什么需要那个窗口)。
///
/// **为什么用 egui 自带的 spinner 而不是自绘**:它带
/// `ui.request_repaint()` —— 动画自己在要帧,空闲时也能持续转;自绘的
/// 旋转弧得由调用方额外驱动重绘,漏一处就转半圈卡住。
///
/// **窄档**:给宽度紧张的容器(文件栏根目录行)用,见 [`narrow_icon_button`]
/// —— 那里的三枚按钮必须同档,故刷新按钮与选文件夹/新建走同一入口。
///
/// `busy` 只换图形,**不禁用**:忙碌中再点是合理的(用户想再刷一次),
/// 禁用会让按钮灰掉像坏了。语义标签同步改成「正在…」,辅助技术读到的
/// 也是当前状态而不是永远「刷新」。
pub fn narrow_busy_icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    busy: bool,
    busy_text: &str,
    idle_text: &str,
) -> egui::Response {
    let tooltip_text = if busy { busy_text } else { idle_text };
    button_shell_narrow(ui, tooltip_text, |ui, painter, rect, color| {
        if busy {
            egui::Spinner::new()
                .size(ICON)
                .color(color)
                .paint_at(ui, rect);
        } else {
            icon.draw(painter, rect.center(), ICON, color);
        }
    })
}

/// 图标按钮的共同外壳:占位、hover 底、前景/禁用色、辅助技术标签与
/// tooltip。图形部分交给 `paint`。
///
/// `icon_button` 与 [`narrow_busy_icon_button`] 拆出这一层,是为了让两者的
/// 命中矩形与配色**保证同一份** —— 分两处写的话,改 hover 底时容易只改
/// 一边,表现为「忙碌态的按钮手感不一样」。
fn button_shell(
    ui: &mut egui::Ui,
    tooltip_text: &str,
    paint: impl FnOnce(&egui::Ui, &egui::Painter, egui::Rect, Color32),
) -> egui::Response {
    button_shell_with(ui, ICON + 2.0 * SPACE_SM, tooltip_text, paint)
}

/// [`narrow_icon_button`] 的外壳:横向留白收到 `SPACE_XS`,与默认档
/// **共用同一段实现** —— 两档的差异只由宽度参数承担,hover 底、配色、
/// 辅助技术标签都走同一处,不会出现「窄档手感与默认档不一致」。
fn button_shell_narrow(
    ui: &mut egui::Ui,
    tooltip_text: &str,
    paint: impl FnOnce(&egui::Ui, &egui::Painter, egui::Rect, Color32),
) -> egui::Response {
    button_shell_with(
        ui,
        ICON + 2.0 * crate::ui::tokens::SPACE_XS,
        tooltip_text,
        paint,
    )
}

/// 真正干活的外壳(见 [`button_shell`]):`width` 由调用方给定。
///
/// **窄档与默认档拆成两个入口而不是加布尔参数**:布尔开关会让调用点
/// 变成 `icon_button(ui, icon, tip, true)` 这种读不出含义的写法。
fn button_shell_with(
    ui: &mut egui::Ui,
    width: f32,
    tooltip_text: &str,
    paint: impl FnOnce(&egui::Ui, &egui::Painter, egui::Rect, Color32),
) -> egui::Response {
    let size = egui::vec2(width, TOOLBAR_H);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let enabled = ui.is_enabled();
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.hovered() && enabled {
            painter.rect_filled(rect, RADIUS_SM, ui.visuals().widgets.hovered.bg_fill);
        }
        let color = if enabled {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        paint(ui, painter, rect, color);
    }
    // 自绘控件补辅助技术语义(mac 精修带回的口径,#166)
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tooltip_text));
    response.on_hover_text(tooltip_text)
}

/// 侧边栏页签:图标 + 文字,选中态用强调色下划线(与文件树的填充式
/// `selectable_label` 区分层级)。宽度按内容,窄面板由外层
/// `horizontal_wrapped` 换行而不是压缩。
pub fn icon_tab(ui: &mut egui::Ui, icon: Icon, label: &str, selected: bool) -> egui::Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let text_color = if selected {
        crate::ui::tokens::accent(ui)
    } else {
        ui.visuals().text_color()
    };
    let label_w = ui
        .fonts_mut(|fonts| fonts.layout_no_wrap(label.to_owned(), font.clone(), text_color))
        .rect
        .width();
    let size = egui::vec2(
        SPACE_SM + crate::ui::tokens::ICON_SM + SPACE_XS + label_w + SPACE_SM,
        TOOLBAR_H,
    );
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        // WorkBuddy 风:选中项浅蓝圆角底,悬停浅灰
        if selected {
            let selected_bg = crate::theme::shell(ui).selected_bg;
            painter.rect_filled(rect, RADIUS_SM, selected_bg);
        } else if response.hovered() {
            painter.rect_filled(rect, RADIUS_SM, ui.visuals().widgets.hovered.bg_fill);
        }
        icon.draw(
            painter,
            egui::pos2(
                rect.left() + SPACE_SM + crate::ui::tokens::ICON_SM / 2.0,
                rect.center().y,
            ),
            crate::ui::tokens::ICON_SM,
            text_color,
        );
        painter.text(
            egui::pos2(
                rect.left() + SPACE_SM + crate::ui::tokens::ICON_SM + SPACE_XS,
                rect.center().y,
            ),
            Align2::LEFT_CENTER,
            label,
            font,
            text_color,
        );
        if selected {
            // 下划线:2px 条贴在页签底部
            painter.rect_filled(
                Rect::from_min_size(
                    egui::pos2(rect.left() + SPACE_SM, rect.bottom() - 2.0),
                    egui::vec2(rect.width() - 2.0 * SPACE_SM, 2.0),
                ),
                1.0,
                text_color,
            );
        }
    }
    if response.clicked() {
        response.mark_changed();
    }
    // 自绘控件补辅助技术语义(mac 精修带回的口径,#166)
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label)
    });
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全部图标都能画出来且不 panic(无头渲染一帧,覆盖每条绘制分支)。
    #[test]
    fn every_icon_renders() {
        let ctx = egui::Context::default();
        let icons = [
            Icon::New,
            Icon::Open,
            Icon::Save,
            Icon::SaveAs,
            Icon::Export,
            Icon::Sidebar,
            Icon::Theme,
            Icon::Ai,
            Icon::Files,
            Icon::Search,
            Icon::Outline,
            Icon::Git,
            Icon::Settings,
            Icon::Reset,
            Icon::PanelRight,
            Icon::Zen,
            Icon::Minimize,
            Icon::Maximize,
            Icon::Restore,
            Icon::Close,
            // 格式工具条(新增图标务必加进来:本测试是「每条绘制分支都不
            // panic」的唯一守卫)
            Icon::CodeInline,
            Icon::CodeBlock,
            Icon::Link,
            Icon::Quote,
            Icon::Divider,
            Icon::Table,
            Icon::Image,
            Icon::Paragraph,
            Icon::BulletList,
            Icon::OrderedList,
            Icon::TaskList,
            // 文件树(#32 I1)
            Icon::FolderClosed,
            Icon::FolderOpen,
            Icon::File,
            // Emoji 面板入口(#28 E1)
            Icon::Emoji,
            // 代码块复制头(#38)
            Icon::Copy,
            Icon::Check,
            // 导出 PDF(#141 症状 B:原先与 ExportHtml 共用 Icon::Export)
            Icon::Print,
        ];
        for icon in icons {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                icon.draw(ui.painter(), egui::pos2(20.0, 20.0), ICON, Color32::WHITE);
            });
            output.drop_without_applying_deltas();
        }
    }

    /// Close 是 Painter 画的两条交叉线,不走 Unicode 字体字形(查找卡
    /// 曾用 `ui.button("✕")`,缺字时呈方框)。用 shapes 取证至少应有
    /// 两条 LineSegment,且一条正斜率/一条负斜率。
    #[test]
    fn close_icon_is_vector_cross_not_text() {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            Icon::Close.draw(ui.painter(), egui::pos2(20.0, 20.0), ICON, Color32::WHITE);
        });
        let mut lines = Vec::new();
        for clipped in &output.shapes {
            if let egui::Shape::LineSegment { points, .. } = &clipped.shape {
                lines.push(*points);
            }
        }
        assert!(lines.len() >= 2, "Close 必须至少画两条交叉线: {lines:?}");
        assert!(
            lines.iter().any(|[a, b]| (b.x - a.x) * (b.y - a.y) > 0.0)
                && lines.iter().any(|[a, b]| (b.x - a.x) * (b.y - a.y) < 0.0),
            "Close 必须同时包含两种对角方向: {lines:?}"
        );
        output.drop_without_applying_deltas();
    }

    #[test]
    fn icon_tab_renders_selected_and_clickable() {
        let ctx = egui::Context::default();
        for selected in [false, true] {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                icon_tab(ui, Icon::Files, "文件", selected);
            });
            output.drop_without_applying_deltas();
        }
    }

    /// 纯图标按钮渲染不 panic 且带 tooltip 文案。
    #[test]
    fn icon_button_renders() {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            icon_button(ui, Icon::Settings, "设置");
        });
        output.drop_without_applying_deltas();
    }

    /// 一排里并排的图标,视觉尺寸须落在同一档 —— 坤哥反馈「选择文件夹和
    /// 新建文件按钮弄好看点,最好能看起来等宽」的那一条守门。
    ///
    /// **量的是墨迹包围盒,不是画布**:画布永远都是 `size` 见方,量它
    /// 等于量「我有没有按 size 画」,恒真。圆环类图标的固有直径天然小于
    /// 满幅方形(`Reset` 原为 11.5px vs `Open` 16.1px,肉眼明显缩一号),
    /// 故靠放大半径做视觉补偿。
    ///
    /// 上界 **1.2px** 是实测标定出来的:`Open` 16.11 / `New` 15.76 /
    /// `Reset` 15.83,三档最大差 0.35px。放宽到 2px 也能过,但那正是
    /// 「看得出大小不一」的水位 —— 定 1.2 是给标定值留余量又不放弃
    /// 守门。**这个上界独立于被测实现**:它不引用任何参与造坐标的常数,
    /// 只把 `Icon::draw` 的输出量出来比,所以改半径会真红。
    #[test]
    fn icons_in_a_row_share_optical_size() {
        /// 图标墨迹的包围盒宽(px)。
        fn ink_width(icon: Icon) -> f32 {
            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                icon.draw(ui.painter(), egui::pos2(50.0, 50.0), ICON, Color32::WHITE);
            });
            output.textures_delta.clear();
            let (mut min, mut max) = (f32::MAX, f32::MIN);
            for clipped in &output.shapes {
                let rect = clipped.shape.visual_bounding_rect();
                min = min.min(rect.left());
                max = max.max(rect.right());
            }
            max - min
        }

        // 根目录行三枚 + 设置页「恢复默认」(同一枚 Reset),外加 rail 的
        // Files 页签作满幅方形的基准参照。
        let row = [
            ("Open", Icon::Open),
            ("New", Icon::New),
            ("Reset", Icon::Reset),
        ];
        let widths: Vec<f32> = row.iter().map(|(_, icon)| ink_width(*icon)).collect();
        let (min, max) = (
            widths.iter().cloned().fold(f32::MAX, f32::min),
            widths.iter().cloned().fold(f32::MIN, f32::max),
        );
        const TOLERANCE: f32 = 1.2;
        assert!(
            max - min <= TOLERANCE,
            "并排图标视觉宽度须同档(实测 {row:?} = {widths:?},差 {:.2} > {TOLERANCE})",
            max - min
        );
        // 基准参照本身也别缩得太小:满幅方形应当真的画满了画布的大半,
        // 否则「三枚都偏小」也会一起过关(补偿过头是另一种难看)。
        let reference = ink_width(Icon::Files);
        assert!(
            reference >= ICON * 0.9,
            "满幅方形基准 {reference:.2} 应接近画布 {ICON},否则同档断言失去意义"
        );
    }

    /// 忙碌态:图形换成 spinner,且 spinner 是**矢量折线**不是文本字符。
    ///
    /// 命中矩形两态全等这条由 `sidebar::tests` 的几何测试负责(那里拿得到
    /// [`egui::Response::rect`]);本测试盯住图形本身,因为它对应坤哥反馈的
    /// 原话「点了也没个动态」:
    ///
    /// ①两态 shape 数不同 —— 图形确实换了(按了有反应);
    /// ②忙碌态是折线、零文本 —— spinner 一旦回退成 `…` 之类的字体字符,
    /// 缺字环境下渲染成方框(同 `Copy`/`Check` 的考法)。
    #[test]
    fn narrow_busy_icon_button_swaps_graphic_with_a_vector_spinner() {
        let render = |busy: bool| {
            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                narrow_busy_icon_button(ui, Icon::Reset, busy, "正在刷新目录树…", "刷新目录树");
            });
            // 本测试要读 shape,故不能一丢了之;`clear` 是
            // `drop_without_applying_deltas` 的同一件事,只是把「丢掉」
            // 换成「清空后照常返回」(直接 drop 未应用的 textures delta
            // 会 panic)。
            output.textures_delta.clear();
            output
        };
        let (idle, busy) = (render(false), render(true));

        let lines = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .filter(|shape| matches!(shape.shape, egui::Shape::LineSegment { .. }))
                .count()
        };
        let texts = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .filter(|shape| matches!(shape.shape, egui::Shape::Text(_)))
                .count()
        };
        assert_ne!(
            lines(&busy),
            lines(&idle),
            "两态折线数必须不同(spinner 是多段弧,回环箭头是固定几段)"
        );
        assert_eq!(texts(&busy), 0, "忙碌态不得混入文本层:缺字环境会渲染成方框");
    }

    /// Copy 与 Check 是 Painter 画的矢量,不走 Unicode 字形(`⧉`/`✓` 在三平台
    /// 缺字风险真实)。Copy 在左、Check 在右(x=40 分界):Copy 应有一枚完整
    /// 矩形描边(前框)+ 至少两条边线(后框上、左边);Check 两条线段一负
    /// 斜率(下挑)一正斜率(上挑),连成对勾。
    #[test]
    fn copy_and_check_icons_are_vector_shapes() {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            Icon::Copy.draw(ui.painter(), egui::pos2(20.0, 20.0), ICON, Color32::WHITE);
            Icon::Check.draw(ui.painter(), egui::pos2(60.0, 20.0), ICON, Color32::WHITE);
        });
        let mut copy_rects = 0;
        let mut copy_lines = 0;
        let mut check_lines = Vec::new();
        for clipped in &output.shapes {
            match &clipped.shape {
                egui::Shape::Rect(rect) if rect.brush.is_none() => {
                    if rect.visual_bounding_rect().center().x < 40.0 {
                        copy_rects += 1;
                    }
                }
                egui::Shape::LineSegment { points, .. } => {
                    if points[0].x < 40.0 {
                        copy_lines += 1;
                    } else {
                        check_lines.push(*points);
                    }
                }
                _ => {}
            }
        }
        assert!(
            copy_rects >= 1,
            "Copy 前框应有一枚矩形描边:rects={copy_rects}"
        );
        assert!(
            copy_lines >= 2,
            "Copy 后框应至少画上、左两条边:{copy_lines}"
        );
        assert!(
            check_lines
                .iter()
                .any(|[a, b]| (b.x - a.x) * (b.y - a.y) < 0.0)
                && check_lines
                    .iter()
                    .any(|[a, b]| (b.x - a.x) * (b.y - a.y) > 0.0),
            "Check 应同时包含下挑与上挑两种斜率:{check_lines:?}"
        );
        output.drop_without_applying_deltas();
    }

    /// 文件树三枚新图标:明暗两套 visuals 各渲染三帧不 panic,且每帧都有
    /// Shape 落进输出(三帧手法与 gutter.rs 同款——首帧字体注册、后续帧
    /// tessellation 各有冷启动路径,单帧绿不等于帧帧绿)。颜色随主题取
    /// visuals 前景,验证的是取色路径本身,不是某个写死色。
    #[test]
    fn tree_icons_render_three_frames_in_both_visuals() {
        for dark in [true, false] {
            let ctx = egui::Context::default();
            ctx.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            for step in 0..3 {
                for icon in [Icon::FolderClosed, Icon::FolderOpen, Icon::File] {
                    let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                        icon.draw(
                            ui.painter(),
                            egui::pos2(20.0, 20.0),
                            crate::ui::tokens::ICON_SM,
                            ui.visuals().text_color(),
                        );
                    });
                    assert!(
                        !output.shapes.is_empty(),
                        "{} 第 {step} 帧 {icon:?} 应有 Shape 输出",
                        if dark { "暗色" } else { "亮色" }
                    );
                    output.drop_without_applying_deltas();
                }
            }
        }
    }

    /// 点是否落在三角形内(含边界)。零面积三角形直接判外,避免退化
    /// 三角形把整条直线上的点都算成「覆盖」。行列式手写(Vec2 无 cross)。
    fn point_in_tri(p: Pos2, a: Pos2, b: Pos2, c: Pos2) -> bool {
        let det = |u: Vec2, v: Vec2| u.x * v.y - u.y * v.x;
        if det(b - a, c - a).abs() < 1e-9 {
            return false;
        }
        let d = |u: Pos2, v: Pos2| det(v - u, p - u);
        let (d1, d2, d3) = (d(a, b), d(b, c), d(c, a));
        (d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0) || (d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0)
    }

    /// 无头像素取样(#41):把一帧 shapes 曲面细分(feathering 关掉,三角
    /// 形即硬边、不依赖 GPU),对采样点做三角形覆盖测试 —— 等价于把图标
    /// 画在纯色画布上读该点像素。返回覆盖该点的颜色,None = 露背景。
    fn covered_color(primitives: &[egui::ClippedPrimitive], p: Pos2) -> Option<Color32> {
        for cp in primitives {
            let egui::epaint::Primitive::Mesh(mesh) = &cp.primitive else {
                continue;
            };
            for tri in mesh.indices.as_chunks::<3>().0 {
                let v = |i: u32| mesh.vertices[i as usize].pos;
                let (a, b, c) = (v(tri[0]), v(tri[1]), v(tri[2]));
                if point_in_tri(p, a, b, c) {
                    return Some(mesh.vertices[tri[0] as usize].color);
                }
            }
        }
        None
    }

    /// 两色的通道距离和(0 = 同色)。
    fn color_dist(a: Color32, b: Color32) -> i32 {
        let ch = |x: u8, y: u8| (i32::from(x) - i32::from(y)).abs();
        ch(a.r(), b.r()) + ch(a.g(), b.g()) + ch(a.b(), b.b())
    }

    /// #41 齿轮像素验收:明暗两套 visuals × ICON / ICON_SM 两档尺寸,各
    /// 渲染一帧齿轮并按归一化坐标采样 ——
    /// - 中心孔存在:圆心露背景,孔环着前景,两者颜色可分;
    /// - 齿数特征:齿顶带(r=0.42)圆周上齿角(每 45°)着前景、齿间(22.5°
    ///   偏移)露背景,沿圆周数覆盖弧段恰 8 段(小尺寸不糊成圆的硬证据);
    /// - 护栏(照 #32 快照手法):画布上不得出现任何文本字形。
    #[test]
    fn settings_gear_pixels_in_both_visuals() {
        // 探针半径:齿顶带(0.398..0.482)内侧、避开折线顶点接缝(浮点
        // 误差可能留亚像素发丝缝);齿间处该半径已越过齿根带外缘,糊成
        // 整圆在这里就露不出背景
        let probe_r = 0.42;
        let (r_root, r_hole) = (0.28, 0.12);
        for dark in [true, false] {
            for size in [ICON, crate::ui::tokens::ICON_SM] {
                let ctx = egui::Context::default();
                ctx.set_visuals(if dark {
                    egui::Visuals::dark()
                } else {
                    egui::Visuals::light()
                });
                // 关抗锯齿羽化:边三角形带透明渐变,采样会读到半透明假前景
                ctx.options_mut(|o| o.tessellation_options.feathering = false);
                // 前景/背景色从 visuals 取真值(Cell 递出闭包;0.36 无 ctx.style)
                let colors = std::cell::Cell::new((Color32::BLACK, Color32::BLACK));
                let center = egui::pos2(8.0 + size / 2.0, 8.0 + size / 2.0);
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    let v = ui.visuals();
                    colors.set((v.text_color(), v.panel_fill));
                    Icon::Settings.draw(ui.painter(), center, size, v.text_color());
                });
                let (fg, bg) = colors.get();
                assert!(
                    color_dist(fg, bg) > 100,
                    "{} 主题前景背景应可分:fg={fg:?} bg={bg:?}",
                    if dark { "暗色" } else { "亮色" }
                );
                for clipped in &output.shapes {
                    assert!(
                        !matches!(clipped.shape, egui::epaint::Shape::Text(_)),
                        "{} size={size} 图标画布不得出现文本字形",
                        if dark { "暗色" } else { "亮色" }
                    );
                }
                let primitives = ctx.tessellate(std::mem::take(&mut output.shapes), 1.0);
                let probe = |r: f32, a: f32| {
                    covered_color(
                        &primitives,
                        center + Vec2::new(r * size * a.cos(), r * size * a.sin()),
                    )
                };

                // 中心孔:圆心露背景、孔环着前景 —— 「中心采样与齿圈采样
                // 颜色可分」
                assert_eq!(
                    probe(0.0, 0.0),
                    None,
                    "{} size={size} 中心孔内应露背景",
                    if dark { "暗色" } else { "亮色" }
                );
                assert_eq!(
                    probe(r_hole, 0.0),
                    Some(fg),
                    "{} size={size} 孔环应着前景",
                    if dark { "暗色" } else { "亮色" }
                );

                // 齿圈本体(齿根带内、离中心线半步避开接缝)着前景;齿角
                // 着前景、齿间露背景
                for k in 0..8_u32 {
                    let tooth = k as f32 * std::f32::consts::FRAC_PI_4;
                    let gap = tooth + std::f32::consts::FRAC_PI_8;
                    assert_eq!(
                        probe(r_root + 0.01, gap),
                        Some(fg),
                        "{} size={size} 第 {k} 段齿根弧应着前景",
                        if dark { "暗色" } else { "亮色" }
                    );
                    assert_eq!(
                        probe(probe_r, tooth),
                        Some(fg),
                        "{} size={size} 第 {k} 齿应探到前景",
                        if dark { "暗色" } else { "亮色" }
                    );
                    assert_eq!(
                        probe(probe_r, gap),
                        None,
                        "{} size={size} 第 {k} 齿间应露背景(糊成圆即红)",
                        if dark { "暗色" } else { "亮色" }
                    );
                }

                // 齿数特征:沿 r=probe_r 圆周每 0.5° 采样数覆盖弧段。从齿
                // 间中心(22.5°)起绕一圈,避免同一齿跨首尾被数成两段;弧
                // 段宽度 ≥3°(6 步)才计入:真齿 ≥20°,接缝发丝缝 ≤1°,
                // 过滤的是细分伪影不是齿形本身
                let mut arcs = 0;
                let mut run = 0_i32;
                for step in 0..720 {
                    let a =
                        std::f32::consts::FRAC_PI_8 + step as f32 * (std::f32::consts::TAU / 720.0);
                    if probe(probe_r, a).is_some() {
                        run += 1;
                    } else if run > 0 {
                        if run >= 6 {
                            arcs += 1;
                        }
                        run = 0;
                    }
                }
                if run >= 6 {
                    arcs += 1;
                }
                assert_eq!(
                    arcs,
                    8,
                    "{} size={size} 齿轮外缘应有 8 个独立齿",
                    if dark { "暗色" } else { "亮色" }
                );
                output.drop_without_applying_deltas();
            }
        }
    }
}
