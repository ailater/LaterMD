//! 系统 CJK 字体发现(PDF 嵌入用)。
//!
//! 候选表是**单一来源**:egui 界面侧(latermd-app `fonts.rs`)与 PDF 导出
//! 共用 [`CJK_SYSTEM_CANDIDATES`],预览与导出命中同一个 face,中文观感
//! 一致。发现是注入读函数的纯遍历:按序取第一个「可读且 krilla 可解析」
//! 的候选(`.ttc` 的 face 序号逐候选携带,如 Noto Sans CJK 集合内比例与
//! 等宽两套 SC 字型);候选全失配返回 [`CjkFontError`] 交导出失败路径
//! 展示,绝不 panic 或产出乱码 PDF。

use std::fmt;
use std::io;

use super::shaping;
use super::{PdfFont, PdfFonts};

/// 一个系统 CJK 字体候选:文件路径 + `.ttc` 集合内的 face 序号。
///
/// face 序号与 egui 界面侧同口径:Linux 各发行版的 Noto 路径由 `fc-query`
/// 枚举得出,Windows/macOS 条目为资料建议值,待真机核验(见 docs/m0-report.md)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CjkFontCandidate {
    /// 绝对路径(三平台前缀互斥,存在性探测天然分流,无需 #[cfg] 分表)。
    pub path: &'static str,
    /// 比例字形 face 序号(PDF 正文)。
    pub proportional_index: u32,
    /// 等宽字形 face 序号(PDF 代码块;与比例同 face 的候选填相同值,
    /// 发现时不重复嵌入,见 [`discover_cjk_fonts`])。
    pub monospace_index: u32,
    /// 同族**粗体**变体的绝对路径;`None` = 该平台/字体无粗体文件可配。
    /// 界面侧(`fonts.rs`)用它把 CJK 粗体 face 插进 `bold` 族链 —— 否则
    /// 中文加粗落回 Regular face、视觉零变化(2026-10-09 坤哥报告
    /// 「Live 模式加粗 ** 不渲染」)。PDF 侧暂不消费(PDF 的粗体嵌入
    /// 是独立工作,缺它不影响现有导出)。
    ///
    /// face 序号**复用 `proportional_index`**:两文件是同族平行集合
    /// (NotoSansCJK-Bold.ttc 实测 face 2 = SC,与 Regular 同构;msyhbd
    /// 与 msyh 同构)。非平行结构会造成粗体错 face,宁可 `None`。
    pub bold_path: Option<&'static str>,
}

/// 系统 CJK 候选表,按序探测:Noto Sans CJK(Debian/Ubuntu、Arch 系、
/// Fedora/RHEL 的 noto 包路径互不相同)→ 文泉驿微米黑兜底 → 微软雅黑/
/// 黑体 → PingFang/Hiragino(Windows/macOS 路径在对应平台前缀外不可达)。
/// Linux 各发行版的 .ttc 同为上游 noto-cjk 发布集合,face 结构一致
/// (比例 SC=2 / 等宽 SC=7,Bold 平行);wqy 为单字型集合。
pub const CJK_SYSTEM_CANDIDATES: &[CjkFontCandidate] = &[
    // Debian/Ubuntu:同一 .ttc 内含比例与等宽两套 SC 字型;
    // Bold.ttc 与 Regular.ttc face 结构平行(fc-query 实测 SC 同为 face 2)
    CjkFontCandidate {
        path: "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        proportional_index: 2,
        monospace_index: 7,
        bold_path: Some("/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc"),
    },
    // Arch 系(Arch/CachyOS/Manjaro,noto-fonts-cjk 包):同一上游集合,
    // fc-query 实测 face 表与 Debian 版全同(2=SC / 7=Mono SC)
    CjkFontCandidate {
        path: "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        proportional_index: 2,
        monospace_index: 7,
        bold_path: Some("/usr/share/fonts/noto-cjk/NotoSansCJK-Bold.ttc"),
    },
    // Fedora/RHEL 系(google-noto-sans-cjk-ttc-fonts 包):同一上游集合
    CjkFontCandidate {
        path: "/usr/share/fonts/google-noto-sans-cjk-ttc-fonts/NotoSansCJK-Regular.ttc",
        proportional_index: 2,
        monospace_index: 7,
        bold_path: Some("/usr/share/fonts/google-noto-sans-cjk-ttc-fonts/NotoSansCJK-Bold.ttc"),
    },
    // Linux 兜底:文泉驿微米黑,单字型集合,无粗体变体(Debian/Ubuntu 路径)
    CjkFontCandidate {
        path: "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        proportional_index: 0,
        monospace_index: 0,
        bold_path: None,
    },
    // 文泉驿兜底第二路径(Arch/Fedora 的 wqy-microhei 包)
    CjkFontCandidate {
        path: "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
        proportional_index: 0,
        monospace_index: 0,
        bold_path: None,
    },
    // Windows 11:msyh.ttc 的 face 0 = 微软雅黑(face 1 为 UI 变体);simhei 单字型;
    // msyhbd 为粗体集合(msyh 同构),仅作最末兜底
    CjkFontCandidate {
        path: "C:\\Windows\\Fonts\\msyh.ttc",
        proportional_index: 0,
        monospace_index: 0,
        bold_path: Some("C:\\Windows\\Fonts\\msyhbd.ttc"),
    },
    CjkFontCandidate {
        path: "C:\\Windows\\Fonts\\simhei.ttf",
        proportional_index: 0,
        monospace_index: 0,
        bold_path: None,
    },
    CjkFontCandidate {
        path: "C:\\Windows\\Fonts\\msyhbd.ttc",
        proportional_index: 0,
        monospace_index: 0,
        // 自身已是粗体集合;再指自己是浪费一次 face 查询,保持 None
        bold_path: None,
    },
    // macOS 14:PingFang 的 index 0 为占位(任一 face 均含 CJK 可消除方块,
    // SC Regular 确切 index 待真机枚举后修正);Hiragino Sans GB 为简体兜底。
    // 两者的多字重藏在同文件多 face 里,与 Regular 文件不是平行结构,
    // 粗体映射待真机枚举后再补(先 None,行为 = 修复前)
    CjkFontCandidate {
        path: "/System/Library/Fonts/PingFang.ttc",
        proportional_index: 0,
        monospace_index: 0,
        bold_path: None,
    },
    CjkFontCandidate {
        path: "/System/Library/Fonts/Hiragino Sans GB.ttc",
        proportional_index: 0,
        monospace_index: 0,
        bold_path: None,
    },
];

/// CJK 字体发现错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CjkFontError {
    /// 候选表全部失配:文件不存在、不可读或 krilla 解析不出 face。
    NoReadableCandidate,
}

impl fmt::Display for CjkFontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CjkFontError::NoReadableCandidate => write!(
                f,
                "未找到可嵌入的中文字体(Noto Sans CJK / 微软雅黑 / PingFang \
                 等候选均不可用),请先安装任一候选字体"
            ),
        }
    }
}

impl std::error::Error for CjkFontError {}

/// 按序探测候选,返回第一个可读且可解析的字体集。
///
/// `read` 注入文件读取(生产传 [`std::fs::read`],测试注入假读),「可读」
/// = 读出字节且 krilla 能解析该 face —— 文件残缺、face 序号越界都跳到
/// 下一候选,不 panic。等宽 face 与比例同序号时不单独嵌入(绘制回落正文
/// 字体,避免同一 face 嵌两份);粗体恒 `None`(候选表无粗体主条目,标题
/// 靠字号区分,即 [`PdfFonts`] 的既有回落语义)。
pub fn discover_cjk_fonts(
    candidates: &[CjkFontCandidate],
    read: &dyn Fn(&str) -> io::Result<Vec<u8>>,
) -> Result<PdfFonts, CjkFontError> {
    for candidate in candidates {
        let Ok(data) = read(candidate.path) else {
            continue;
        };
        if shaping::Face::new(&PdfFont {
            data: data.clone(),
            index: candidate.proportional_index,
        })
        .is_none()
        {
            continue;
        }
        let mono = if candidate.monospace_index != candidate.proportional_index {
            // 等宽 face 解析失败只降级(代码块回落正文字体),不弃整个候选
            shaping::Face::new(&PdfFont {
                data: data.clone(),
                index: candidate.monospace_index,
            })
            .map(|_| PdfFont {
                data: data.clone(),
                index: candidate.monospace_index,
            })
        } else {
            None
        };
        let regular = PdfFont {
            data,
            index: candidate.proportional_index,
        };
        return Ok(PdfFonts {
            regular,
            bold: None,
            mono,
        });
    }
    Err(CjkFontError::NoReadableCandidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 合法字体字节来源:DejaVu(分布最广);无字体环境跳过注入类断言
    /// (与 pdf/mod.rs 的 `system_font` 同口径)。
    fn valid_font_bytes() -> Option<Vec<u8>> {
        std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").ok()
    }

    /// 注入候选表(路径全假,由注入 read 决定命中),验证发现遍历本身。
    fn fake_candidates() -> [CjkFontCandidate; 2] {
        [
            CjkFontCandidate {
                path: "/fake/first.ttc",
                proportional_index: 0,
                monospace_index: 0,
                bold_path: None,
            },
            CjkFontCandidate {
                path: "/fake/second.ttf",
                proportional_index: 0,
                monospace_index: 0,
                bold_path: None,
            },
        ]
    }

    #[test]
    fn first_readable_candidate_wins() {
        let Some(bytes) = valid_font_bytes() else {
            eprintln!("跳过:本机无可用系统字体");
            return;
        };
        let candidates = fake_candidates();
        let found = discover_cjk_fonts(&candidates, &|path| {
            (path == "/fake/first.ttc")
                .then(|| bytes.clone())
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
        })
        .expect("第一条可读即命中");
        assert_eq!(found.regular.data, bytes);
        assert_eq!(found.regular.index, 0);
        assert!(found.bold.is_none(), "候选表无粗体主条目");
        assert!(
            found.mono.is_none(),
            "等宽与比例同序号时不重复嵌入,回落正文字体"
        );
    }

    /// 读失败(不存在/权限)与不可解析(坏字节)都跳到下一候选,不 panic。
    #[test]
    fn unreadable_and_broken_candidates_fall_through() {
        let Some(bytes) = valid_font_bytes() else {
            eprintln!("跳过:本机无可用系统字体");
            return;
        };
        let candidates = fake_candidates();
        // 首条读失败 → 命中第二条
        let found = discover_cjk_fonts(&candidates, &|path| {
            (path == "/fake/second.ttf")
                .then(|| bytes.clone())
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
        })
        .expect("首候选读失败应降级到第二条");
        assert_eq!(found.regular.data, bytes);

        // 首条可读但字节非法(krilla 解析不出 face)→ 同样降级
        let found = discover_cjk_fonts(&candidates, &|path| {
            (path == "/fake/first.ttc")
                .then(|| vec![0u8; 32])
                .or_else(|| Some(bytes.clone()))
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
        })
        .expect("首候选坏字节应降级到第二条");
        assert_eq!(found.regular.data, bytes);
    }

    /// 全部候选失配:明确错误而非 panic;文案点到候选族名,可直接进导出
    /// 失败提示行。
    #[test]
    fn no_candidate_yields_explicit_error() {
        let candidates = fake_candidates();
        let error = discover_cjk_fonts(&candidates, &|_| {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        })
        .unwrap_err();
        assert_eq!(error, CjkFontError::NoReadableCandidate);
        let message = error.to_string();
        assert!(message.contains("中文字体"), "文案应说明缺什么:{message}");
        assert!(!message.is_empty());
    }

    /// 生产候选表三平台形态(与 app 侧既有测试同款口径,防迁移时抄错):
    /// 路径按目标平台语义绝对且唯一,Windows 条目 face 0。
    #[test]
    fn system_candidates_shape() {
        fn is_absolute_on_target_platform(path: &str) -> bool {
            path.starts_with('/') || path.as_bytes().get(1) == Some(&b':')
        }
        assert_eq!(CJK_SYSTEM_CANDIDATES.len(), 10);
        let mut paths: Vec<&str> = CJK_SYSTEM_CANDIDATES.iter().map(|c| c.path).collect();
        paths.sort_unstable();
        paths.dedup();
        assert_eq!(paths.len(), CJK_SYSTEM_CANDIDATES.len(), "候选路径重复");
        for candidate in CJK_SYSTEM_CANDIDATES {
            assert!(
                is_absolute_on_target_platform(candidate.path),
                "非绝对路径: {}",
                candidate.path
            );
            if candidate.path.starts_with("C:") {
                assert_eq!(
                    (candidate.proportional_index, candidate.monospace_index),
                    (0, 0),
                    "{} 偏离 face 0 约定",
                    candidate.path
                );
            }
        }
    }

    /// 生产表端到端(本机有 CJK 候选才跑):命中候选的正文 face 可被 krilla
    /// 解析;Noto 的等宽 face(序号 7 ≠ 2)随之嵌入,单 face 候选(wqy 等)
    /// 的等宽槽回落正文字体。
    #[test]
    fn production_table_discovers_readable_cjk() {
        let found = discover_cjk_fonts(CJK_SYSTEM_CANDIDATES, &|path| std::fs::read(path));
        let found = match found {
            Ok(found) => found,
            Err(error) => {
                eprintln!("跳过:本机无 CJK 候选字体({error})");
                return;
            }
        };
        assert!(found.regular.index == 2 || found.regular.index == 0);
        match found.regular.index {
            // Noto:比例 2 / 等宽 7,两个 face 都该嵌入
            2 => assert_eq!(
                found.mono.map(|font| font.index),
                Some(7),
                "Noto 候选应嵌入等宽 face 7"
            ),
            // 其余候选等宽与比例同序号,回落正文字体
            _ => assert!(found.mono.is_none(), "单 face 候选不应重复嵌入等宽槽"),
        }
    }
}
