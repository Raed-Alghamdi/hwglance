//! The device tab's text sits optically centred: measured on offscreen renders at several font
//! sizes. Its own binary because it changes the global font size.

use egui::{vec2, Color32};
use hwglance::fonts::{self, FontChoice};
use hwglance::theme::{self, Palette};
use hwglance::ui_kit;

/// Render a tab and return (top padding, bottom padding) in pixels: the empty rows above and
/// below the glyphs inside the badge.
fn render(name: &str, device: Color32, base: f32) -> (image::RgbaImage, egui::Rect) {
    theme::set_font_size(base);
    let pal = Palette::dark();
    let name = name.to_string();
    let rect_out = std::rc::Rc::new(std::cell::Cell::new(egui::Rect::NOTHING));
    let rect_in = rect_out.clone();
    let mut first = true;
    let mut h = egui_kittest::Harness::builder().with_size(vec2(220.0, 90.0)).wgpu().build_ui(move |ui| {
        if first {
            first = false;
            // fonts apply from the next pass
            fonts::install(ui.ctx(), FontChoice::Atkinson);
            return;
        }
        ui.painter().rect_filled(ui.max_rect(), 0.0, pal.header);
        let r = ui_kit::paint_tab(ui.painter(), 40.0, 45.0, &name, device, pal.header);
        rect_in.set(r);
    });
    h.run_steps(4);
    (h.render().expect("render"), rect_out.get())
}

fn paddings(name: &str, device: Color32, base: f32) -> (i32, i32) {
    let (img, r) = render(name, device, base);
    let fill = img.get_pixel(r.left() as u32 + 2, r.top() as u32 + 2).0;
    let diff = |x: u32, y: u32| -> i32 {
        let p = img.get_pixel(x, y).0;
        (0..3).map(|i| (p[i] as i32 - fill[i] as i32).abs()).max().unwrap()
    };
    // glyph pixels: well away from the fill (ignore the rounded corners and antialiasing fringes)
    let (x0, x1) = (r.left() as u32 + 5, r.right() as u32 - 5);
    let (y0, y1) = (r.top() as u32, r.bottom() as u32);
    let max = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| diff(x, y)).max().unwrap();
    let ink_row = |y: u32| (x0..x1).any(|x| diff(x, y) * 2 > max);
    let first = (y0..y1).find(|&y| ink_row(y)).expect("ink");
    let last = (y0..y1).rev().find(|&y| ink_row(y)).unwrap();
    (first as i32 - y0 as i32, (y1 - 1) as i32 - last as i32)
}

#[test]
fn tab_text_is_optically_centred_and_the_badge_scales_at_every_size() {
    let pal = Palette::dark();
    for base in [12.0, 14.5, 18.0] {
        // all-caps names: top and bottom padding match within 1 px
        for (name, color) in [("CPU", pal.cpu), ("GPU", pal.gpu), ("Board", pal.board)] {
            let (top, bottom) = paddings(name, color, base);
            assert!((top - bottom).abs() <= 1, "size {base} {name}: top {top} vs bottom {bottom}");
        }
        // the badge is about 22 px tall at the default size and scales with the font
        let (_, r) = render("CPU", pal.cpu, base);
        let want = (22.0 * base / 14.5f32).round();
        assert!((r.height() - want).abs() <= 1.0, "base {base}: height {}", r.height());
        // a name with a dotted i reaches above the capitals: allow 2 px
        let (top, bottom) = paddings("Drive", pal.drive, base);
        assert!((top - bottom).abs() <= 2, "size {base} Drive: top {top} vs bottom {bottom}");
    }
    theme::set_font_size(14.5);
}

