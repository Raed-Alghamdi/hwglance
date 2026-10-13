//! Arabic window titles and process names: the font fallback, the shaping and the renders.
//! Writes PNGs into the directory named by `HWGLANCE_SHOTS` (a `light` variant when
//! `HWGLANCE_THEME=light`); without it the tests only check that rendering works.

mod common;

use common::*;
use hwglance::settings::Settings;

const SIZE: (f32, f32) = (2546.0, 1400.0);

/// An egui context with the app's fonts applied.
fn font_ctx() -> egui::Context {
    let ctx = egui::Context::default();
    hwglance::fonts::install(&ctx, hwglance::fonts::FontChoice::Atkinson);
    // fonts apply from the next pass
    ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
    ctx
}

/// The characters of `text` as egui lays them out, left to right (one per cluster).
fn glyphs_left_to_right(ctx: &egui::Context, family: egui::FontFamily, text: &str) -> String {
    let galley = ctx.fonts_mut(|f| f.layout_no_wrap(text.to_string(), egui::FontId::new(15.0, family), egui::Color32::WHITE));
    let mut glyphs: Vec<(f32, char)> = galley.rows.iter().flat_map(|r| r.glyphs.iter()).filter(|g| g.advance_width > 0.0).map(|g| (g.pos.x, g.chr)).collect();
    glyphs.sort_by(|a, b| a.0.total_cmp(&b.0));
    glyphs.into_iter().map(|g| g.1).collect()
}

#[test]
fn an_installed_arabic_font_covers_every_family() {
    // The fallback is looked up on this machine; without an Arabic font nothing changes.
    let Some(font) = hwglance::fonts::arabic_font() else { return };
    assert!(std::path::Path::new(&font.regular).is_file(), "{font:?}");
    let ctx = font_ctx();
    ctx.fonts_mut(|f| {
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace, hwglance::fonts::bold(), hwglance::fonts::mono_bold()] {
            let id = egui::FontId::new(14.0, family.clone());
            assert!(f.has_glyphs(&id, ARABIC_TITLE), "{family:?} lacks Arabic");
            assert!(f.has_glyphs(&id, "Chrome - المفكرة (2024)"), "{family:?}");
        }
    });
}

#[test]
fn egui_lays_the_reordered_title_out_so_it_reads_right_to_left() {
    if hwglance::fonts::arabic_font().is_none() {
        return;
    }
    let ctx = font_ctx();
    let reversed = |s: &str| s.chars().rev().collect::<String>();
    // Pure Arabic: left to right on screen is the title read backwards, letter by letter.
    for family in [egui::FontFamily::Proportional, hwglance::fonts::bold(), egui::FontFamily::Monospace] {
        let shaped = hwglance::text::shape(ARABIC_TITLE);
        assert_eq!(glyphs_left_to_right(&ctx, family.clone(), &shaped), reversed(ARABIC_TITLE), "{family:?}");
    }
    // Latin and numbers inside it keep their reading order.
    let shaped = hwglance::text::shape("Chrome - واتساب 2024");
    assert_eq!(glyphs_left_to_right(&ctx, egui::FontFamily::Proportional, &shaped), format!("Chrome - 2024 {}", reversed("واتساب")));
    let shaped = hwglance::text::shape("واتساب Chrome 2024");
    assert_eq!(glyphs_left_to_right(&ctx, egui::FontFamily::Proportional, &shaped), format!("Chrome 2024 {}", reversed("واتساب")));
}

#[test]
fn the_process_panel_with_arabic_rows_renders() {
    let mut h = arabic_procs_harness("arabic-procs", SIZE, Settings::default());
    let theme = std::env::var("HWGLANCE_THEME").unwrap_or_else(|_| "dark".into());
    save(&mut h, &format!("arabic-compact-{theme}.png"));
}

#[test]
fn the_expanded_process_panel_with_arabic_rows_renders() {
    let mut h = arabic_procs_harness("arabic-procs-expanded", SIZE, expanded_view(Settings::default()));
    let theme = std::env::var("HWGLANCE_THEME").unwrap_or_else(|_| "dark".into());
    save(&mut h, &format!("arabic-expanded-{theme}.png"));
}

#[test]
fn a_narrow_docked_panel_cuts_arabic_titles_with_an_ellipsis() {
    let mut s = Settings::default();
    s.layout.set_dock(hwglance::settings::Dock::Right);
    let mut h = arabic_procs_harness("arabic-procs-right", SIZE, s);
    let theme = std::env::var("HWGLANCE_THEME").unwrap_or_else(|_| "dark".into());
    save(&mut h, &format!("arabic-right-dock-{theme}.png"));
}
