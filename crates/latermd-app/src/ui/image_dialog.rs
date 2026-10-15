//! 「图片框」对话框(docs/image-plan.md A 段 + B 段本地文件来源 + C 段图床)。
//!
//! ## 分工
//!
//! 本模块**只收输入、只发消息**:alt 与 url 两个草稿字符串归 UI 原地持有
//! (和 `SettingsState` 持草稿同款 —— `TextEdit` 是立即模式控件,草稿必须
//! 能就地 `&mut`),插入文本的归约在 `state`(走 `compose::insert_image`),
//! 本地文件的复制与地址回填同样在归约(`Message::ImageFilePickRequested`)。
//!
//! ## 为什么它是唯一「点了不改文档」的工具条动作
//!
//! `![alt](url)` 要两个输入,没法像加粗那样从选区推断 —— 富文本工具条里
//! 其余十六个动作都能从「当前选区 + 一个标记」推出结果,只有图片不行。
//! 因此它的按钮与快捷键都只开对话框。
//!
//! ## 三条来源共用这一个出口
//!
//! 网络地址手填、B 段「浏览…」复制进 `<doc名>.assets/` 后回填相对地址、
//! C 段「选文件并上传…」交给图床(归约里关框并发起后台上传,成功即自动
//! 插入 —— 没有留给用户的第二次点击)。插入动作始终是同一个
//! `Message::ImageInserted{ alt, url }` —— 汇流点只有一个,单测才钉得住。

use eframe::egui;

use latermd_bed::BedProfile;

/// 图片框的草稿状态(归约置 `open`,UI 改 `alt` / `url` / `bed`)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageDialogState {
    /// 对话框是否可见。
    pub open: bool,
    /// 替代文字草稿;开对话框时按选区预填,浏览本地文件时空则补文件名。
    pub alt: String,
    /// 图片地址草稿;手填网络地址,或由「浏览…」回填复制后的相对地址。
    pub url: String,
    /// 图床上传选中的 profile id;开框时沿用上次选择或默认第一个(归约
    /// 预置),关框保留(见 `State::close_image_dialog`)。
    pub bed: Option<String>,
}

/// 画对话框,返回**(插入, 浏览, 上传, 取消, ×)**五个响应,测试定位
/// 用(与 `ui::layout::commit_dialog` 同款手法,× 与「取消」同语义)。
///
/// 地址为空时禁用「插入」:`![](url)` 里 url 是唯一的实质内容,空着插进去
/// 只会在预览里留一个破图占位。alt 允许为空 —— 装饰性图片的 alt 本来就
/// 该空着(无障碍规范)。「浏览」复制的是**文件本体**,url 栏回填后用户
/// 仍可改(比如手改 alt 再插)。「上传」没有可选 profile 时禁用并指路设置。
pub fn dialog(
    ui: &mut egui::Ui,
    draft: &mut ImageDialogState,
    beds: &[BedProfile],
) -> (
    egui::Response,
    egui::Response,
    egui::Response,
    egui::Response,
    bool,
) {
    let mut browse = None;
    let mut upload = None;
    let mut buttons = None;
    let mut close = false;
    egui::Window::new("插入图片")
        // 首次打开锚定屏幕中心;拖动后由 Area 按窗口 id 记忆保持。显式
        // 锚点让无头测试的帧间位置稳定(viewport_rect 在测试里同样确定)。
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ui.ctx().viewport_rect().center())
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .frame(crate::ui::workbench::dialog_frame(ui))
        .show(ui.ctx(), |ui| {
            close = crate::ui::workbench::dialog_header(ui, "插入图片");
            ui.label("替代文字(alt)");
            ui.text_edit_singleline(&mut draft.alt)
                .on_hover_text("图片加载失败时显示的文字;装饰性图片可留空");
            ui.label("图片地址");
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut draft.url).on_hover_text(
                    "网络地址直接填;或点「浏览…」选本地图片,复制到文档旁的 .assets/ 目录后自动回填相对地址",
                );
                browse = Some(
                    ui.button("浏览…").on_hover_text(
                        "选本地图片,复制到 <doc名>.assets/ 目录(需要文档已保存)",
                    ),
                );
            });
            ui.add_space(crate::ui::tokens::SPACE_SM);
            ui.separator();
            ui.label("图床上传(上传成功后自动插入)");
            ui.horizontal(|ui| {
                let selected = draft
                    .bed
                    .as_deref()
                    .and_then(|id| beds.iter().find(|profile| profile.id == id));
                egui::ComboBox::from_label("图床")
                    .selected_text(
                        selected
                            .map(|profile| profile.display_name().to_owned())
                            .unwrap_or_else(|| "选择图床…".to_owned()),
                    )
                    .show_ui(ui, |ui| {
                        for profile in beds {
                            let is_selected = draft.bed.as_deref() == Some(profile.id.as_str());
                            if ui
                                .selectable_label(is_selected, profile.display_name())
                                .clicked()
                            {
                                draft.bed = Some(profile.id.clone());
                            }
                        }
                    });
                let ready = selected.is_some();
                upload = Some(
                    ui.add_enabled(ready, egui::Button::new("选文件并上传…")).on_hover_text(
                        if ready {
                            "选图片上传到该图床,成功后把返回的 URL 插进文档(token 存系统凭据,不落配置文件)"
                        } else {
                            "尚未配置图床:设置 → 图片 里新增一个"
                        },
                    ),
                );
            });
            crate::ui::workbench::dialog_footer_divider(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                let ready = !draft.url.trim().is_empty();
                let insert = ui.add_enabled(ready, egui::Button::new("插入"));
                ui.add_space(crate::ui::tokens::SPACE_MD);
                let cancel = ui.button("取消");
                buttons = Some((insert, cancel));
            });
        });
    let (insert, cancel) = buttons.expect("浮窗必然绘制插入/取消按钮");
    (
        insert,
        browse.expect("浮窗必然绘制浏览按钮"),
        upload.expect("浮窗必然绘制上传按钮"),
        cancel,
        close,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::RawInput;

    /// 渲染不 panic(空草稿 / 已填草稿 × 有无图床),按钮可用性与约定一致:
    /// 地址空禁用插入,取消与浏览恒可点;选中**存在的**图床才可上传
    /// (草稿里的 id 可能指向已删除的 profile,选不到即视为未选)。
    #[test]
    fn dialog_renders_in_both_draft_states() {
        // 浮窗首帧只完成注册、按钮响应还未参与命中(与 ui::layout 点击测试
        // 的「sizing pass」同一节奏),断言落在稳定的第三帧。
        let mut smms = BedProfile::preset_smms();
        smms.id = "p-1".to_owned();
        let beds = [smms];
        for mut draft in [
            ImageDialogState::default(),
            ImageDialogState {
                open: true,
                alt: "示意图".to_owned(),
                url: "https://x/y.png".to_owned(),
                bed: Some("p-1".to_owned()),
            },
        ] {
            for beds in [&beds[..], &[][..]] {
                let ctx = egui::Context::default();
                let mut seen = None;
                for _ in 0..3 {
                    let output = ctx.run_ui(RawInput::default(), |ui| {
                        let (insert, browse, upload, cancel, _x) = dialog(ui, &mut draft, beds);
                        seen = Some((
                            insert.enabled(),
                            browse.enabled(),
                            upload.enabled(),
                            cancel.enabled(),
                        ));
                    });
                    output.drop_without_applying_deltas();
                }
                let (insert_enabled, browse_enabled, upload_enabled, cancel_enabled) =
                    seen.expect("至少跑了一帧");
                // 地址空 → 插入按钮被禁用(不产出可点的响应)
                assert_eq!(
                    insert_enabled,
                    !draft.url.trim().is_empty(),
                    "地址为空时禁用插入"
                );
                assert!(browse_enabled, "浏览恒可点(禁用与否由归约侧提示)");
                let selected = draft
                    .bed
                    .as_deref()
                    .is_some_and(|id| beds.iter().any(|profile| profile.id == id));
                assert_eq!(upload_enabled, selected, "选中可用图床才可上传");
                assert!(cancel_enabled, "取消永远可点");
            }
        }
    }

    /// 草稿里的图床 id 指向不存在的 profile(已删除):下拉回落占位文案,
    /// 上传按钮禁用,渲染不 panic。
    #[test]
    fn stale_bed_selection_disables_upload() {
        let mut draft = ImageDialogState {
            open: true,
            bed: Some("gone".to_owned()),
            ..ImageDialogState::default()
        };
        let ctx = egui::Context::default();
        let mut upload_enabled = true;
        for _ in 0..3 {
            let output = ctx.run_ui(RawInput::default(), |ui| {
                let (_, _, upload, _, _x) = dialog(ui, &mut draft, &[]);
                upload_enabled = upload.enabled();
            });
            output.drop_without_applying_deltas();
        }
        assert!(!upload_enabled, "选不到图床时禁用上传");
    }
}
