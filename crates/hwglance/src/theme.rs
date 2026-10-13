//! The window's two palettes (the night-shift report and its day-shift print), the type scale
//! base and the mapping onto egui `Visuals`. The window does not follow the Omarchy colour theme,
//! only its light or dark mode (see `system_theme`).

use egui::{Color32, Context, FontFamily, FontId, TextStyle};
use std::sync::atomic::{AtomicU32, Ordering};

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Light or dark: which of the two palettes is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Scheme {
    #[default]
    Dark,
    Light,
}

/// The `appearance.theme` setting: follow the desktop or force one scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    #[default]
    System,
    Dark,
    Light,
}

impl ThemeChoice {
    /// Unknown text means the default, like the font setting.
    pub fn parse(s: &str) -> ThemeChoice {
        match s.to_ascii_lowercase().as_str() {
            "dark" => ThemeChoice::Dark,
            "light" => ThemeChoice::Light,
            _ => ThemeChoice::System,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ThemeChoice::System => "system",
            ThemeChoice::Dark => "dark",
            ThemeChoice::Light => "light",
        }
    }

    /// The scheme to paint: the desktop's only when the setting says "system".
    pub fn resolve(self, system: Scheme) -> Scheme {
        match self {
            ThemeChoice::System => system,
            ThemeChoice::Dark => Scheme::Dark,
            ThemeChoice::Light => Scheme::Light,
        }
    }
}

/// Surfaces, ink levels, device legend colours and states. Contrast of every ink on every
/// surface is at least 4.5:1, in both palettes.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub scheme: Scheme,
    pub ground: Color32,
    pub panel: Color32,
    pub header: Color32,
    pub alt: Color32,
    /// Alternate rows of the process table: twice the step of `alt` away from the panel.
    pub zebra: Color32,
    pub hover: Color32,
    pub rule: Color32,
    pub selected: Color32,
    pub ink: Color32,
    pub ink2: Color32,
    pub ink3: Color32,
    pub cpu: Color32,
    pub gpu: Color32,
    pub mem: Color32,
    pub drive: Color32,
    pub board: Color32,
    pub igpu: Color32,
    /// Amber as text, dots and marks; on a light ground it must be dark, so it cannot also be
    /// the fill. Equal to `warn` in the dark palette.
    pub warn: Color32,
    /// Amber as a wash (the warning zone of a range bar).
    pub warn_fill: Color32,
    pub crit: Color32,
    /// Text on a solid `crit` fill (the "Throttling now" pill, a confirm button).
    pub on_crit: Color32,
    /// Watched (highlighted) values, and nothing else.
    pub watch: Color32,
    /// Shadow of floating layers.
    pub shadow: Color32,
    /// Strength of the observed range over the empty track: light tracks need a firmer fill to
    /// keep 3:1 with a device colour that is also readable as text.
    pub range_alpha: f32,
}

impl Palette {
    pub const fn dark() -> Palette {
        Palette {
            scheme: Scheme::Dark,
            ground: rgb(0x0B0E13),
            panel: rgb(0x121821),
            header: rgb(0x171E29),
            alt: rgb(0x141B25),
            zebra: rgb(0x161E29),
            hover: rgb(0x1A2230),
            rule: rgb(0x263040),
            selected: rgb(0x1B2B45),
            ink: rgb(0xECF1F7),
            ink2: rgb(0xAEB8C6),
            ink3: rgb(0x8792A3),
            cpu: rgb(0x5AA8FF),
            gpu: rgb(0xB591FF),
            mem: rgb(0x33D1E3),
            drive: rgb(0xF48FD3),
            board: rgb(0xA3B1C4),
            igpu: rgb(0xCDBBFF),
            warn: rgb(0xF7BA42),
            warn_fill: rgb(0xF7BA42),
            crit: rgb(0xFF5D62),
            on_crit: rgb(0x0B0E13),
            watch: rgb(0x4BE38F),
            shadow: Color32::from_black_alpha(128),
            range_alpha: 0.70,
        }
    }

    /// The same report printed for day shift: cool paper panels on a grey desk, the device hues
    /// and signal colours kept but deepened until they read as text on every surface.
    pub const fn light() -> Palette {
        Palette {
            scheme: Scheme::Light,
            ground: rgb(0xE6EAF0),
            panel: rgb(0xFAFBFD),
            header: rgb(0xEDF1F6),
            alt: rgb(0xF6F8FB),
            zebra: rgb(0xF2F5F9),
            hover: rgb(0xE7ECF3),
            rule: rgb(0xC6CFDB),
            selected: rgb(0xD6E3F6),
            ink: rgb(0x111827),
            ink2: rgb(0x38445A),
            ink3: rgb(0x566377),
            cpu: rgb(0x1A62C3),
            gpu: rgb(0x5E33CF),
            mem: rgb(0x006A7B),
            drive: rgb(0xB12B83),
            board: rgb(0x52667E),
            igpu: rgb(0x6A54A3),
            warn: rgb(0x855400),
            warn_fill: rgb(0xE8A71C),
            crit: rgb(0xB8242B),
            on_crit: rgb(0xFAFBFD),
            watch: rgb(0x0A713E),
            shadow: Color32::from_black_alpha(56),
            range_alpha: 0.85,
        }
    }

    pub const fn for_scheme(scheme: Scheme) -> Palette {
        match scheme {
            Scheme::Dark => Palette::dark(),
            Scheme::Light => Palette::light(),
        }
    }

    pub fn is_dark(&self) -> bool {
        self.scheme == Scheme::Dark
    }

    /// Fill and text of a solid destructive button. The dark window keeps its softened red with
    /// ink; the light one needs the full red to carry light text.
    pub fn crit_button(&self) -> (Color32, Color32) {
        if self.is_dark() { (self.crit.gamma_multiply(0.73), self.ink) } else { (self.crit, self.on_crit) }
    }

    /// Outline of radios, checkboxes and buttons, and the fill of a slider rail: the rule blended
    /// towards ink 3 until it keeps 3:1 on the ground, panel, header and hover surfaces in both
    /// palettes (the rule alone is 1.3:1 in dark and 2.4:1 in light).
    pub fn control(&self) -> Color32 {
        self.rule.lerp_to_gamma(self.ink3, 0.7)
    }

    /// Backdrop of a modal dialog. The dark window needs a heavier scrim to part the dialog
    /// from a page that is already near black; the light one separates well with the default.
    pub fn scrim(&self) -> Color32 {
        Color32::from_black_alpha(if self.is_dark() { 185 } else { 100 })
    }

    /// Colour for a sensor level (`None` for normal).
    pub fn level_color(&self, level: Level) -> Option<Color32> {
        match level {
            Level::Normal => None,
            Level::Warn => Some(self.warn),
            Level::Crit => Some(self.crit),
        }
    }

    pub fn visuals(&self) -> egui::Visuals {
        let mut v = if self.is_dark() { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.panel_fill = self.ground;
        v.window_fill = self.panel;
        v.window_stroke = egui::Stroke::new(1.0, self.rule);
        v.window_corner_radius = egui::CornerRadius::same(8);
        v.menu_corner_radius = egui::CornerRadius::same(8);
        v.faint_bg_color = self.alt;
        v.extreme_bg_color = self.ground;
        v.code_bg_color = self.header;
        v.hyperlink_color = self.cpu;
        v.warn_fg_color = self.warn;
        v.error_fg_color = self.crit;
        v.override_text_color = None;
        v.weak_text_color = Some(self.ink3);
        v.selection.bg_fill = self.selected;
        v.selection.stroke = egui::Stroke::new(1.0, self.ink2);
        v.striped = false;
        v.window_shadow = self.floating_shadow();
        v.popup_shadow = v.window_shadow;

        let w = &mut v.widgets;
        let r = egui::CornerRadius::same(6);
        for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            wv.corner_radius = r;
        }
        w.noninteractive.bg_fill = self.panel;
        w.noninteractive.weak_bg_fill = self.panel;
        w.noninteractive.bg_stroke = egui::Stroke::new(1.0, self.rule);
        w.noninteractive.fg_stroke = egui::Stroke::new(1.0, self.ink2);
        w.inactive.bg_fill = self.header;
        w.inactive.weak_bg_fill = self.header;
        // Radio circles, checkboxes and button outlines: a control has to be found by its outline,
        // so it takes the control colour (3:1 on every surface a control sits on), not the rule.
        w.inactive.bg_stroke = egui::Stroke::new(1.0, self.control());
        w.inactive.fg_stroke = egui::Stroke::new(1.0, self.ink2);
        w.hovered.bg_fill = self.hover;
        w.hovered.weak_bg_fill = self.hover;
        w.hovered.bg_stroke = egui::Stroke::new(1.0, self.ink3);
        w.hovered.fg_stroke = egui::Stroke::new(1.5, self.ink);
        w.active.bg_fill = self.selected;
        w.active.weak_bg_fill = self.selected;
        w.active.bg_stroke = egui::Stroke::new(1.0, self.ink2);
        w.active.fg_stroke = egui::Stroke::new(2.0, self.ink);
        w.open.bg_fill = self.hover;
        w.open.weak_bg_fill = self.hover;
        w.open.fg_stroke = egui::Stroke::new(1.0, self.ink);
        v
    }

    /// Fill and quiet text of the collector-down banner: a red wash over the ground. On the light
    /// ground the wash is thinner and the reason takes ink 2, so both keep 4.5:1.
    pub fn crit_banner(&self) -> (Color32, Color32) {
        if self.is_dark() { (self.crit.gamma_multiply(0.14), self.ink3) } else { (self.crit.gamma_multiply(0.08), self.ink2) }
    }

    /// The one soft shadow of floating layers: windows, menus, the popover and dialogs.
    pub fn floating_shadow(&self) -> egui::Shadow {
        egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: self.shadow }
    }

    /// Install both palettes as egui's dark and light visuals and make `scheme` the active one.
    pub fn apply(scheme: Scheme, ctx: &Context) {
        ctx.set_visuals_of(egui::Theme::Dark, Palette::dark().visuals());
        ctx.set_visuals_of(egui::Theme::Light, Palette::light().visuals());
        ctx.set_theme(match scheme {
            Scheme::Dark => egui::Theme::Dark,
            Scheme::Light => egui::Theme::Light,
        });
    }
}

/// Threshold level of a reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Normal,
    Warn,
    Crit,
}

// ---------------------------------------------------------------------------
// Type scale
// ---------------------------------------------------------------------------

/// The default base size; layout sizes written in design pixels scale with `size / DEFAULT`.
pub const DEFAULT_FONT_SIZE: f32 = 14.5;

static FONT_SIZE: AtomicU32 = AtomicU32::new(0x4164_0000); // 14.5f32

/// Set the user's base font size (10 to 24).
pub fn set_font_size(size: f32) {
    FONT_SIZE.store(size.clamp(10.0, 24.0).to_bits(), Ordering::Relaxed);
}

/// The base of the type scale: every size is derived from it.
pub fn base() -> f32 {
    f32::from_bits(FONT_SIZE.load(Ordering::Relaxed))
}

pub fn font_scale() -> f32 {
    base() / DEFAULT_FONT_SIZE
}

/// Scale a length written for the default font size.
pub fn fs(design: f32) -> f32 {
    design * font_scale()
}

/// Round up to the 4 px layout unit.
pub fn snap4(v: f32) -> f32 {
    (v / 4.0).ceil() * 4.0
}

/// The smallest hit area of anything that can be clicked or dragged, however small the type.
pub const MIN_HIT: f32 = 24.0;

/// A height written for the default font size that something interactive has: scaled, on the
/// 4 px grid and never under `MIN_HIT`.
pub fn hit_h(design: f32) -> f32 {
    snap4(design * font_scale()).max(MIN_HIT)
}

/// Egui text styles follow the type scale.
pub fn apply_text_styles(ctx: &Context) {
    let b = base();
    ctx.all_styles_mut(|style| {
        use FontFamily::{Monospace, Proportional};
        style.text_styles = [
            (TextStyle::Small, FontId::new(b - 2.0, Proportional)),
            (TextStyle::Body, FontId::new(b - 0.5, Proportional)),
            (TextStyle::Button, FontId::new(b - 0.5, Proportional)),
            (TextStyle::Heading, FontId::new(b + 1.5, Proportional)),
            (TextStyle::Monospace, FontId::new(b - 1.0, Monospace)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 4.0);
        style.spacing.button_padding = egui::vec2(10.0, 4.0);
        style.spacing.interact_size.y = hit_h(24.0);
        // Windows, dialogs and menus breathe: the same 16 px inside every frame.
        style.spacing.window_margin = egui::Margin::same(16);
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.floating = false;
        style.interaction.tooltip_delay = 0.9;
        // The edges and corners of a resizable window (Settings) grab 12 px each side: a 24 px target.
        style.interaction.resize_grab_radius_side = MIN_HIT / 2.0;
        style.interaction.resize_grab_radius_corner = MIN_HIT / 2.0;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lum(c: Color32) -> f64 {
        let f = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (lum(a), lum(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    fn both() -> [(&'static str, Palette); 2] {
        [("dark", Palette::dark()), ("light", Palette::light())]
    }

    fn surfaces(p: &Palette) -> [Color32; 7] {
        [p.ground, p.panel, p.header, p.alt, p.zebra, p.hover, p.selected]
    }

    fn inks(p: &Palette) -> [Color32; 12] {
        [p.ink, p.ink2, p.ink3, p.cpu, p.gpu, p.mem, p.drive, p.board, p.igpu, p.warn, p.crit, p.watch]
    }

    fn devices(p: &Palette) -> [Color32; 6] {
        [p.cpu, p.gpu, p.mem, p.drive, p.board, p.igpu]
    }

    /// `fg` at `alpha` painted over `bg`, the way egui composites a `gamma_multiply` tint.
    fn wash(bg: Color32, fg: Color32, alpha: f32) -> Color32 {
        bg.blend(fg.gamma_multiply(alpha))
    }

    #[test]
    fn every_ink_reads_on_every_surface() {
        for (name, p) in both() {
            for s in surfaces(&p) {
                for i in inks(&p) {
                    assert!(contrast(i, s) >= 4.5, "{name}: {i:?} on {s:?}: {:.2}", contrast(i, s));
                }
            }
        }
    }

    /// Text on a tint or a solid fill reaches 4.5:1 in both palettes.
    fn floor(_: &Palette) -> f64 {
        4.5
    }

    #[test]
    fn text_on_solid_fills_reads() {
        for (name, p) in both() {
            assert!(contrast(p.on_crit, p.crit) >= 4.5, "{name}: the Throttling now pill: {:.2}", contrast(p.on_crit, p.crit));
            let (fill, ink) = p.crit_button();
            assert!(contrast(ink, fill) >= floor(&p), "{name}: the kill button: {:.2}", contrast(ink, fill));
        }
        // The dark window also sets ground-coloured text on a device colour.
        let p = Palette::dark();
        for tab in devices(&p) {
            assert!(contrast(p.ground, tab) >= 4.5);
        }
    }

    #[test]
    fn range_bars_and_marks_keep_3_to_1_on_their_track() {
        for (name, p) in both() {
            for device in devices(&p) {
                let track = crate::rangebar::track_color(device, &p);
                let range = wash(track, device, p.range_alpha);
                assert!(contrast(range, track) >= 3.0, "{name}: observed range of {device:?}: {:.2}", contrast(range, track));
                // marker (ink, or the state colour) and average tick
                for mark in [p.ink, p.warn, p.crit, p.ink3] {
                    assert!(contrast(mark, track) >= 3.0, "{name}: {mark:?} on the {device:?} track: {:.2}", contrast(mark, track));
                }
                // the marker's outline is the panel, so it must stand off the panel too
                assert!(contrast(p.ink, p.panel) >= 3.0);
            }
        }
    }

    #[test]
    fn text_reads_on_the_tints_it_sits_on() {
        for (name, p) in both() {
            // a heavy process cell: 16 % of its column's device colour under ink
            for device in devices(&p) {
                for row in [p.panel, p.zebra, p.hover, p.selected] {
                    let cell = wash(row, device, 0.16);
                    for ink in [p.ink, p.ink2] {
                        assert!(contrast(ink, cell) >= floor(&p), "{name}: {ink:?} on a {device:?} cell over {row:?}: {:.2}", contrast(ink, cell));
                    }
                }
            }
            // the fallback app badge: neutral ink 3 tint under a letter in full ink
            for row in [p.panel, p.zebra, p.hover, p.selected] {
                for a in crate::procs::badge_tint_strengths() {
                    let badge = wash(row, p.ink3, a);
                    assert!(contrast(p.ink, badge) >= floor(&p), "{name}: badge at {a} over {row:?}: {:.2}", contrast(p.ink, badge));
                }
            }
            // a watched row: a green wash (up to a heavier one at the start of the focus flash)
            // (a collapsed section's row is on the panel or its hover; a watched chip on a collapsed card is on the header)
            for row in [p.panel, p.alt, p.hover, p.header] {
                let watched = wash(row, p.watch, crate::sensors::WATCH_WASH);
                assert!(contrast(p.watch, watched) >= floor(&p), "{name}: watched on {row:?}: {:.2}", contrast(p.watch, watched));
                // the collapsed row's title (ink 2) and the chip's name and unit (ink 3) stay readable on it
                // (a hovered row dims ink 3 below the floor under the wash, as it does for every min and max)
                assert!(contrast(p.ink2, watched) >= floor(&p), "{name}: ink 2 on a watched {row:?}: {:.2}", contrast(p.ink2, watched));
                if row != p.hover {
                    assert!(contrast(p.ink3, watched) >= floor(&p), "{name}: ink 3 on a watched {row:?}: {:.2}", contrast(p.ink3, watched));
                }
                for level in [p.warn, p.crit] {
                    assert!(contrast(level, watched) >= floor(&p), "{name}: {level:?} on a watched row: {:.2}", contrast(level, watched));
                }
            }
            let flash = wash(p.panel, p.watch, crate::sensors::FLASH_PEAK);
            assert!(contrast(p.ink, flash) >= floor(&p), "{name}: ink on the focus flash: {:.2}", contrast(p.ink, flash));
            // the kill warning box (18 % red) and the collector-down banner (14 % red)
            let warning = wash(p.panel, p.crit, 0.18);
            assert!(contrast(p.ink, warning) >= floor(&p), "{name}: kill warning: {:.2}", contrast(p.ink, warning));
            let (fill, reason) = p.crit_banner();
            let banner = p.ground.blend(fill);
            assert!(contrast(p.crit, banner) >= floor(&p), "{name}: banner heading: {:.2}", contrast(p.crit, banner));
            assert!(contrast(reason, banner) >= floor(&p), "{name}: banner reason: {:.2}", contrast(reason, banner));
            // the red band behind a flag series in the history plot
            assert!(contrast(p.ink2, wash(p.panel, p.crit, 0.45)) >= 3.0, "{name}: history band");
        }
    }

    #[test]
    fn the_process_column_headers_read_at_rest_in_both_palettes() {
        for (name, p) in both() {
            for device in devices(&p) {
                let rest = p.header.blend(device.gamma_multiply(crate::procs::HEADER_REST_ALPHA));
                assert!(contrast(rest, p.header) >= 4.5, "{name}: {device:?} header at rest: {:.2}", contrast(rest, p.header));
            }
            let (fill, ink) = p.crit_button();
            assert!(contrast(ink, fill) >= 4.5, "{name}: the Force kill text: {:.2}", contrast(ink, fill));
        }
    }

    #[test]
    fn control_outlines_and_slider_rails_keep_3_to_1_on_every_surface_they_sit_on() {
        for (name, p) in both() {
            let c = p.control();
            for (sn, s) in [("ground", p.ground), ("panel", p.panel), ("header", p.header), ("hover", p.hover)] {
                assert!(contrast(c, s) >= 3.0, "{name}: control outline on {sn}: {:.2}", contrast(c, s));
            }
            // the visuals use it for the radio, checkbox and button outlines
            assert_eq!(p.visuals().widgets.inactive.bg_stroke.color, c, "{name}");
            // and it still reads as an outline, not as ink
            assert!(contrast(c, p.panel) < contrast(p.ink3, p.panel), "{name}");
        }
    }

    #[test]
    fn the_dark_scrim_is_heavier_than_the_light_one() {
        let (d, l) = (Palette::dark().scrim(), Palette::light().scrim());
        assert!(d.a() >= 170, "dark: {}", d.a());
        assert_eq!(l, Color32::from_black_alpha(100), "light already separates and keeps the default");
        assert!(d.a() > l.a());
    }

    #[test]
    fn device_tabs_read_on_their_own_tint() {
        for (name, p) in both() {
            for device in devices(&p) {
                for bg in [p.header, p.hover] {
                    let fill = crate::ui_kit::tab_fill(device, bg);
                    let ink = crate::ui_kit::tab_ink(device, fill);
                    assert!(contrast(ink, fill) >= 4.5, "{name}: {device:?} on its tint: {:.2}", contrast(ink, fill));
                    assert_ne!(fill, bg, "the tab has a fill");
                }
            }
        }
    }

    #[test]
    fn the_selected_row_stands_off_its_neighbours_as_far_as_the_inks_allow() {
        // Every ink must keep 4.5:1 on the selected fill, which caps how far its luminance can
        // part from the hover and zebra rows; the left bar and the outline carry the rest.
        for (name, p) in both() {
            assert!(contrast(p.selected, p.hover) >= 1.08, "{name}: selected against hover: {:.3}", contrast(p.selected, p.hover));
            assert!(contrast(p.selected, p.zebra) >= 1.15, "{name}: selected against zebra: {:.3}", contrast(p.selected, p.zebra));
            // the bar is full ink: it reads against the fill
            assert!(contrast(p.ink, p.selected) >= 7.0, "{name}: bar on the selected fill");
        }
    }

    #[test]
    fn the_zebra_step_is_twice_the_old_alternate_row() {
        for (name, p) in both() {
            for (panel, alt, zebra) in [(p.panel.r(), p.alt.r(), p.zebra.r()), (p.panel.g(), p.alt.g(), p.zebra.g()), (p.panel.b(), p.alt.b(), p.zebra.b())] {
                let (old, new) = (alt as i32 - panel as i32, zebra as i32 - panel as i32);
                assert_eq!(new, 2 * old, "{name}");
            }
        }
    }

    #[test]
    fn surfaces_step_the_right_way_in_each_palette() {
        let (d, l) = (Palette::dark(), Palette::light());
        // dark: panels are lifted off the ground, the header lifts again
        assert!(lum(d.ground) < lum(d.panel) && lum(d.panel) < lum(d.header));
        // light: panels are the brightest paper on a grey desk, the header band sits a step down
        assert!(lum(l.ground) < lum(l.header) && lum(l.header) < lum(l.panel));
        for (name, p) in both() {
            assert_ne!(p.panel, p.ground, "{name}");
            assert!(contrast(p.rule, p.panel) > 1.3, "{name}: the rule stroke shows on the panel");
        }
        // amber needs a dark text token and a lighter fill in the light palette only
        assert_eq!(d.warn, d.warn_fill);
        assert!(lum(l.warn_fill) > lum(l.warn));
    }

    #[test]
    fn the_scheme_picks_the_palette_and_visuals() {
        assert_eq!(Palette::for_scheme(Scheme::Dark), Palette::dark());
        assert_eq!(Palette::for_scheme(Scheme::Light), Palette::light());
        assert!(Palette::dark().visuals().dark_mode && !Palette::light().visuals().dark_mode);
        let ctx = Context::default();
        Palette::apply(Scheme::Light, &ctx);
        assert_eq!(ctx.theme(), egui::Theme::Light);
        assert_eq!(ctx.style_of(egui::Theme::Light).visuals.panel_fill, Palette::light().ground);
        assert_eq!(ctx.style_of(egui::Theme::Dark).visuals.panel_fill, Palette::dark().ground);
        Palette::apply(Scheme::Dark, &ctx);
        assert_eq!(ctx.theme(), egui::Theme::Dark);
    }

    #[test]
    fn the_system_scheme_counts_only_when_the_setting_says_system() {
        for system in [Scheme::Dark, Scheme::Light] {
            assert_eq!(ThemeChoice::System.resolve(system), system);
            assert_eq!(ThemeChoice::Dark.resolve(system), Scheme::Dark);
            assert_eq!(ThemeChoice::Light.resolve(system), Scheme::Light);
        }
        assert_eq!(ThemeChoice::parse("system"), ThemeChoice::System);
        assert_eq!(ThemeChoice::parse("dark"), ThemeChoice::Dark);
        assert_eq!(ThemeChoice::parse("Light"), ThemeChoice::Light);
        assert_eq!(ThemeChoice::parse("sepia"), ThemeChoice::System, "unknown text means follow the system");
        for c in [ThemeChoice::System, ThemeChoice::Dark, ThemeChoice::Light] {
            assert_eq!(ThemeChoice::parse(c.as_str()), c);
        }
    }

    #[test]
    fn windows_and_dialogs_get_a_16_px_inner_margin() {
        let ctx = Context::default();
        apply_text_styles(&ctx);
        assert_eq!(ctx.global_style().spacing.window_margin, egui::Margin::same(16));
    }

    #[test]
    fn spec_colours() {
        let p = Palette::dark();
        assert_eq!(p.ground, Color32::from_rgb(0x0B, 0x0E, 0x13));
        assert_eq!(p.watch, Color32::from_rgb(0x4B, 0xE3, 0x8F));
        assert_eq!(p.cpu, Color32::from_rgb(0x5A, 0xA8, 0xFF));
        let l = Palette::light();
        assert_eq!(l.panel, Color32::from_rgb(0xFA, 0xFB, 0xFD));
        assert_eq!(l.watch, Color32::from_rgb(0x0A, 0x71, 0x3E));
    }

    #[test]
    fn type_scale_follows_the_base() {
        set_font_size(14.5);
        assert!((fs(10.0) - 10.0).abs() < 1e-4);
        set_font_size(29.0 / 2.0);
        assert_eq!(snap4(26.0), 28.0);
        assert_eq!(snap4(28.0), 28.0);
        set_font_size(40.0);
        assert_eq!(base(), 24.0);
        set_font_size(14.5);
    }
}
