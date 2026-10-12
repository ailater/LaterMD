//! Load Apple fonts from this Mac; no system font files are bundled or exported.
use super::*;
use std::path::PathBuf;

fn pingfang_path() -> Option<PathBuf> {
    let legacy = PathBuf::from("/System/Library/Fonts/PingFang.ttc");
    if legacy.is_file() {
        return Some(legacy);
    }
    // Recent macOS releases deliver PingFang as a font asset. Its hash and
    // asset generation are OS-owned, so neither is hard-coded.
    let assets = std::fs::read_dir("/System/Library/AssetsV2").ok()?;
    for folder in assets.flatten() {
        if !folder
            .file_name()
            .to_string_lossy()
            .starts_with("com_apple_MobileAsset_Font")
        {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(folder.path()) {
            for entry in entries.flatten() {
                let path = entry.path().join("AssetData/PingFang.ttc");
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// Locate a TTC face by PostScript name instead of assuming OS-specific indices.
fn face_index(bytes: &[u8], name: &str) -> Option<u32> {
    if bytes.get(..4)? != b"ttcf" {
        return None;
    }
    for index in 0..be_u32(bytes, 8)? {
        let offset = be_u32(bytes, 12 + index as usize * 4)? as usize;
        for table in 0..be_u16(bytes, offset + 4)? as usize {
            let record = offset + 12 + table * 16;
            if bytes.get(record..record + 4)? != b"name" {
                continue;
            }
            let start = be_u32(bytes, record + 8)? as usize;
            let strings = start + be_u16(bytes, start + 4)? as usize;
            for n in 0..be_u16(bytes, start + 2)? as usize {
                let row = start + 6 + n * 12;
                if be_u16(bytes, row + 6)? != 6 {
                    continue;
                }
                let len = be_u16(bytes, row + 8)? as usize;
                let pos = strings + be_u16(bytes, row + 10)? as usize;
                let raw = bytes.get(pos..pos + len)?;
                let value = if matches!(be_u16(bytes, row)?, 0 | 3) {
                    String::from_utf16(
                        &raw.as_chunks::<2>()
                            .0
                            .iter()
                            .map(|p| u16::from_be_bytes([p[0], p[1]]))
                            .collect::<Vec<_>>(),
                    )
                    .ok()?
                } else {
                    String::from_utf8_lossy(raw).into_owned()
                };
                if value == name {
                    return Some(index);
                }
            }
        }
    }
    None
}

pub(super) fn cjk_source() -> Option<(PathBuf, u32, u32)> {
    let path = pingfang_path().or_else(|| {
        CANDIDATES
            .iter()
            .find(|c| Path::new(c.path).is_file())
            .map(|c| PathBuf::from(c.path))
    })?;
    let bytes = std::fs::read(&path).ok()?;
    let regular = face_index(&bytes, "PingFangSC-Regular").unwrap_or(0);
    let bold = face_index(&bytes, "PingFangSC-Semibold").unwrap_or(regular);
    Some((path, regular, bold))
}

fn system_face(bytes: Vec<u8>, weight: f32) -> FontData {
    FontData::from_owned(bytes).tweak(egui::FontTweak {
        coords: egui::epaint::text::VariationCoords::new([(b"wght", weight), (b"opsz", 13.0)]),
        ..Default::default()
    })
}

pub(super) fn install(ctx: &egui::Context) -> Option<String> {
    let sf = std::fs::read("/System/Library/Fonts/SFNS.ttf").ok()?;
    let (path, regular, bold) = cjk_source()?;
    let cjk = std::fs::read(&path).ok()?;
    let metrics = parse_vertical_tables(&cjk, regular)?.vertical_metrics_em();
    let mut defs = build_definitions(Some((&cjk, regular, regular, metrics)), Some((&cjk, bold)));
    // Keep the existing family identifiers: renderers and user themes refer to
    // these aliases. Only their registered font data changes on macOS.
    for (name, weight) in [
        (NAME_REGULAR, 400.0),
        (FAMILY_MEDIUM, 500.0),
        (FAMILY_SEMIBOLD, 600.0),
    ] {
        defs.font_data
            .insert(name.into(), Arc::new(system_face(sf.clone(), weight)));
    }
    for (name, weight) in [(PREVIEW_REGULAR, 400.0), (PREVIEW_SEMIBOLD, 600.0)] {
        let bytes = override_vertical_metrics(&sf, 0, metrics)?;
        defs.font_data
            .insert(name.into(), Arc::new(system_face(bytes, weight)));
    }
    // UI 推广副本的 mac 修正(#43 M2 UI 推广,2026-10-12):上方
    // build_definitions 把 UI 链尾 CJK 副本对齐到 Inter,但本函数随后把
    // UI 链头换成了 SF NS —— 副本目标必须跟着链头走。用 SF NS 表值重写
    // 同名键(与 PREVIEW 副本「SF 就 CJK」方向相反,同 editor-mono 的
    // 「CJK 就链头」);SF NS 表值解析失败则保留 Inter 对齐版(基线残差
    // 一个 1/32 网格步,由取证断言的网格等价容差吸收)。
    if let Some(sf_ui_target) = parse_vertical_tables(&sf, 0).map(|t| t.vertical_metrics_em()) {
        if let Some(patched) = override_vertical_metrics(&cjk, regular, sf_ui_target) {
            defs.font_data.insert(
                CJK_PROPORTIONAL_UI.into(),
                Arc::new(FontData {
                    index: regular,
                    ..FontData::from_owned(patched)
                }),
            );
        }
    }
    if let Ok(mono) = std::fs::read("/System/Library/Fonts/SFNSMono.ttf") {
        if let Some(target) = parse_vertical_tables(&mono, 0).map(|t| t.vertical_metrics_em()) {
            if let Some(patched) = override_vertical_metrics(&cjk, regular, target) {
                defs.font_data
                    .insert("SF-Mono".into(), Arc::new(system_face(mono, 400.0)));
                defs.font_data.insert(
                    CJK_MONOSPACE_EDITOR.into(),
                    Arc::new(FontData {
                        index: regular,
                        ..FontData::from_owned(patched)
                    }),
                );
                for family in [
                    FontFamily::Monospace,
                    FontFamily::Name(Arc::from(FAMILY_EDITOR_MONO)),
                ] {
                    defs.families
                        .entry(family)
                        .or_default()
                        .insert(0, "SF-Mono".into());
                }
            }
        }
    }
    ctx.set_fonts(defs);
    mark_installed(ctx, Some(metrics), true);
    Some(format!("San Francisco / SF Mono · {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_font_faces_are_loaded_with_real_weights_and_monospaced_latin() {
        let ctx = egui::Context::default();
        install(&ctx).expect("macOS system fonts must be available");
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        let family = editor_mono_family(&ctx);
        ctx.fonts(|fonts| {
            let defs = fonts.definitions();
            assert_eq!(
                defs.font_data[NAME_REGULAR].font.as_ref(),
                std::fs::read("/System/Library/Fonts/SFNS.ttf").unwrap()
            );
            assert_ne!(
                defs.font_data[NAME_REGULAR].tweak.coords,
                defs.font_data[FAMILY_SEMIBOLD].tweak.coords
            );
            assert_eq!(defs.families[&family][0], "SF-Mono");
            assert_eq!(
                defs.font_data[CJK_PROPORTIONAL].index,
                cjk_source().unwrap().1
            );
        });
        ctx.fonts_mut(|fonts| {
            let font = egui::FontId::new(15.0, family);
            assert!(fonts.has_glyphs(&font, "苹果中文"));
            let narrow = fonts
                .layout_no_wrap("iiii".to_owned(), font.clone(), egui::Color32::WHITE)
                .size()
                .x;
            let wide = fonts
                .layout_no_wrap("WWWW".to_owned(), font, egui::Color32::WHITE)
                .size()
                .x;
            assert!((narrow - wide).abs() < 0.1);
        });
    }
}
