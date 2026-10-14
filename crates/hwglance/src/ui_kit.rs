//! Shared pieces of the visual system: device identity, icons, the type scale as fonts, and the
//! split of a formatted value into its number and unit.

use crate::fonts;
use crate::visibility;
use crate::theme::{base, Palette};
use egui::{Color32, FontFamily, FontId};
use hwglance_proto::{format_value, Reading, Unit};

// ---------------------------------------------------------------------------
// Icons: Material Design glyphs of the mono Nerd Font.
// ---------------------------------------------------------------------------

pub mod icon {
    pub const COG: char = '\u{f0493}';
    pub const EYE: char = '\u{f0208}';
    pub const EYE_OFF: char = '\u{f0209}';
    pub const PAUSE: char = '\u{f03e4}';
    pub const CHEVRON_DOWN: char = '\u{f0140}';
    pub const CHEVRON_RIGHT: char = '\u{f0142}';
    pub const THERMOMETER: char = '\u{f050f}';
    pub const CLOSE: char = '\u{f0156}';
    pub const HISTORY: char = '\u{f02da}';
    pub const DRAG: char = '\u{f01db}';
    pub const ARROW_EXPAND: char = '\u{f0616}';
    pub const ARROW_COLLAPSE: char = '\u{f0615}';
    pub const SEARCH: char = '\u{f0349}';
    pub const DOCK_BOTTOM: char = '\u{f10a9}';
    pub const DOCK_RIGHT: char = '\u{f10ab}';
    pub const FOLDER_OPEN: char = '\u{f0770}';
    pub const PIN: char = '\u{f0403}';
    pub const ARROW_UP: char = '\u{f005d}';
    pub const ARROW_DOWN: char = '\u{f0045}';
    pub const PLUS: char = '\u{f0415}';
    pub const BELL: char = '\u{f009a}';
    pub const PENCIL: char = '\u{f03eb}';
    pub const WINDOW_MAXIMIZE: char = '\u{f05af}';
    pub const WINDOW_RESTORE: char = '\u{f05b2}';
    pub const WINDOW_CLOSE: char = '\u{f05ad}';
}

// ---------------------------------------------------------------------------
// Type scale
// ---------------------------------------------------------------------------

pub fn f_label() -> FontId {
    FontId::new(base() - 0.5, FontFamily::Proportional)
}
pub fn f_label_bold() -> FontId {
    FontId::new(base() - 0.5, fonts::bold())
}
pub fn f_result() -> FontId {
    FontId::new(base() + 0.5, fonts::mono_bold())
}
pub fn f_unit() -> FontId {
    FontId::new(base() - 2.0, FontFamily::Monospace)
}
pub fn f_minmax() -> FontId {
    FontId::new(base() - 2.0, FontFamily::Monospace)
}
pub fn f_flag() -> FontId {
    FontId::new(base() - 2.0, fonts::mono_bold())
}
pub fn f_section() -> FontId {
    FontId::new(base() - 1.5, fonts::bold())
}
pub fn f_tab() -> FontId {
    FontId::new(base() - 1.5, fonts::bold())
}
pub fn f_title() -> FontId {
    FontId::new(base() + 1.5, fonts::bold())
}
pub fn f_small() -> FontId {
    FontId::new(base() - 2.0, FontFamily::Proportional)
}
pub fn f_mono() -> FontId {
    FontId::new(base() - 1.0, FontFamily::Monospace)
}
pub fn f_mono_bold() -> FontId {
    FontId::new(base() - 1.0, fonts::mono_bold())
}
pub fn f_icon(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

// ---------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    Cpu,
    Gpu,
    Memory,
    Drive,
    Board,
    Igpu,
}

impl Device {
    /// The device a sensor card (by id) belongs to.
    pub fn of_card(card_id: &str) -> Device {
        match card_id {
            "cpu" => Device::Cpu,
            "memory" => Device::Memory,
            "board" => Device::Board,
            "igpu" => Device::Igpu,
            id if id.starts_with("gpu") => Device::Gpu,
            id if id.starts_with("disk") || id.starts_with("nvme") || id.starts_with("drive") => Device::Drive,
            _ => Device::Board,
        }
    }

    /// Device of a reading id (`cpu/temp/tctl`, `gpu0/load/core`, ...).
    pub fn of_reading(id: &str) -> Device {
        Device::of_card(id.split('/').next().unwrap_or(""))
    }

    /// Short name on the header tab.
    pub fn short(self) -> &'static str {
        match self {
            Device::Cpu => "CPU",
            Device::Gpu => "GPU",
            Device::Memory => "Memory",
            Device::Drive => "Drive",
            Device::Board => "Board",
            Device::Igpu => "iGPU",
        }
    }

    /// Name in the legend of the top bar.
    pub fn legend(self) -> &'static str {
        match self {
            Device::Drive => "Drives",
            d => d.short(),
        }
    }

    pub fn color(self, p: &Palette) -> Color32 {
        match self {
            Device::Cpu => p.cpu,
            Device::Gpu => p.gpu,
            Device::Memory => p.mem,
            Device::Drive => p.drive,
            Device::Board => p.board,
            Device::Igpu => p.igpu,
        }
    }
}

// ---------------------------------------------------------------------------
// Device tab
// ---------------------------------------------------------------------------

/// Share of the device colour in a tab's fill, over the surface it sits on.
pub const TAB_TINT: f32 = 0.22;
/// Vertical correction of the tab's text, as a share of its font size: Atkinson Hyperlegible's
/// line box leaves more room under capitals than above them, so centring the line box alone
/// puts the glyphs off-centre. Measured offscreen at 12, 14.5 and 18 (see tests/tab.rs).
pub const TAB_TEXT_DY: f32 = 0.04;

/// Tab fill: the device colour at `TAB_TINT` over `bg`.
pub fn tab_fill(device: Color32, bg: Color32) -> Color32 {
    bg.lerp_to_gamma(device, TAB_TINT)
}

fn luminance(c: Color32) -> f64 {
    let f = |v: u8| {
        let v = v as f64 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}

/// WCAG contrast ratio of two colours.
pub fn contrast(a: Color32, b: Color32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Tab text: the device colour, moved a little away from the fill (brighter on a dark tab,
/// darker on a light one) when it would not reach 4.5:1 on its own tint.
pub fn tab_ink(device: Color32, fill: Color32) -> Color32 {
    let away = if luminance(fill) > 0.4 { Color32::BLACK } else { Color32::WHITE };
    let mut c = device;
    for _ in 0..8 {
        if contrast(c, fill) >= 4.5 {
            break;
        }
        c = c.lerp_to_gamma(away, 0.12);
    }
    c
}

/// Paint a device tab (a tinted badge with the short name in the device colour) with its left
/// edge at `left`, vertically centred on `cy`, over `bg`. Returns its rect.
pub fn paint_tab(p: &egui::Painter, left: f32, cy: f32, name: &str, device: Color32, bg: Color32) -> egui::Rect {
    let font = f_tab();
    let px = font.size;
    let fill = tab_fill(device, bg);
    let ink = tab_ink(device, fill);
    let g = p.layout_no_wrap(name.to_string(), font, ink);
    let h = (22.0 * crate::theme::font_scale()).round();
    let top = (cy - h / 2.0).round();
    let rect = egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(g.size().x + 16.0, h));
    p.rect_filled(rect, 6.0, fill);
    let pos = egui::pos2(rect.center().x - g.size().x / 2.0, rect.center().y - g.size().y / 2.0 + TAB_TEXT_DY * px);
    p.galley(pos, g, ink);
    rect
}

fn local_tm(unix_ms: u64) -> libc::tm {
    let t = (unix_ms / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    tm
}

/// When a process started, short: "21:04" for today, otherwise "Oct 12". Empty without a time.
pub fn fmt_started(started_ms: u64, now_ms: u64) -> String {
    if started_ms == 0 {
        return String::new();
    }
    let (a, n) = (local_tm(started_ms), local_tm(now_ms));
    if a.tm_year == n.tm_year && a.tm_yday == n.tm_yday {
        return format!("{:02}:{:02}", a.tm_hour, a.tm_min);
    }
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {}", MONTHS[(a.tm_mon as usize).min(11)], a.tm_mday)
}

/// Full local timestamp for a tooltip: "2026-10-12 21:04:33".
pub fn fmt_timestamp(unix_ms: u64) -> String {
    let a = local_tm(unix_ms);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", a.tm_year + 1900, a.tm_mon + 1, a.tm_mday, a.tm_hour, a.tm_min, a.tm_sec)
}

/// Local HH:MM for a unix timestamp in milliseconds.
pub fn fmt_clock(unix_ms: u64) -> String {
    let t = (unix_ms / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// A formatted value split into its number and its unit ("67.2", "°C"). Flags have no unit.
pub fn split_value(unit: Unit, v: f64) -> (String, String) {
    let s = format_value(unit, v);
    match s.split_once(' ') {
        Some((n, u)) => (n.to_string(), u.to_string()),
        None => (s, String::new()),
    }
}

const MIB: f64 = 1024.0 * 1024.0;
const GIB: f64 = MIB * 1024.0;

/// The unit one row prints its numbers in. Bytes and rates pick the scale from the row's
/// largest value, so the current value, low and high all read in the same unit and the unit
/// is printed once, after the current value. Other units never change scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowUnit {
    unit: Unit,
    div: f64,
    decimals: usize,
    suffix: &'static str,
}

impl RowUnit {
    /// Scale for a row whose largest value is `largest` (in the unit's base).
    pub fn new(unit: Unit, largest: f64) -> RowUnit {
        let (div, decimals, suffix) = match unit {
            Unit::Bytes if largest >= GIB => (GIB, 1, "GiB"),
            Unit::Bytes => (MIB, 0, "MiB"),
            Unit::BytesPerSec if largest >= 1e9 => (1e9, 2, "GB/s"),
            Unit::BytesPerSec if largest >= 1e6 => (1e6, 1, "MB/s"),
            Unit::BytesPerSec => (1e3, 0, "KB/s"),
            Unit::Percent => (1.0, 1, "%"),
            Unit::Celsius => (1.0, 1, "°C"),
            Unit::Mhz => (1.0, 0, "MHz"),
            Unit::Watt => (1.0, 1, "W"),
            Unit::Volt => (1.0, 3, "V"),
            Unit::Rpm => (1.0, 0, "RPM"),
            Unit::Mts => (1.0, 0, "MT/s"),
            Unit::Flag => (1.0, 0, ""),
        };
        RowUnit { unit, div, decimals, suffix }
    }

    /// The scale of a reading's row: its largest value over current, low, high and average.
    pub fn of(r: &Reading) -> RowUnit {
        let mut largest = r.value.unwrap_or(0.0).abs();
        if let Some(s) = &r.stats {
            largest = largest.max(s.min.abs()).max(s.max.abs()).max(s.avg.abs());
        }
        RowUnit::new(r.unit, largest)
    }

    /// The number alone ("23.6").
    pub fn number(&self, v: f64) -> String {
        match self.unit {
            Unit::Flag => format_value(Unit::Flag, v),
            _ => format!("{:.*}", self.decimals, v / self.div),
        }
    }

    /// The unit text printed after the current value ("GiB"); empty for flags.
    pub fn suffix(&self) -> &'static str {
        self.suffix
    }

    /// Number and unit of the current value.
    pub fn split(&self, v: f64) -> (String, &'static str) {
        (self.number(v), self.suffix)
    }
}

/// Fit `text` into `width` with an ellipsis. Arabic and other right-to-left text is put in
/// visual order first (`text::shape`), and cut before that so its ellipsis closes the logical end.
pub fn ellipsized(p: &egui::Painter, text: &str, font: FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    use std::hash::{Hash, Hasher};
    let width = width.max(4.0);
    let mut key = std::collections::hash_map::DefaultHasher::new();
    ((width * 4.0) as i32, &font).hash(&mut key);
    let fits = |t: &str| p.layout_no_wrap(t.to_string(), font.clone(), color).size().x <= width;
    let text = crate::text::shape_fitted(text, key.finish(), &fits);
    let mut job = egui::text::LayoutJob::single_section(text.into_owned(), egui::TextFormat::simple(font, color));
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    p.layout_job(job)
}

/// A slider whose rail is drawn in the control colour: egui paints the rail with the inactive
/// fill (the header band), which is 1.05:1 on the panel.
pub fn slider(ui: &mut egui::Ui, pal: &Palette, name: &str, slider: egui::Slider<'_>) -> egui::Response {
    let r = ui
        .scope(|ui| {
            ui.visuals_mut().widgets.inactive.bg_fill = pal.control();
            ui.add(slider)
        })
        .inner;
    crate::a11y::publish(&r, || crate::a11y::Spoken::new(egui::accesskit::Role::Slider, name));
    r
}

// ---------------------------------------------------------------------------
// Reset with Undo
// ---------------------------------------------------------------------------

/// What an offered Undo looks like.
pub struct UndoOffer<'a> {
    /// Time left to take the reset back.
    pub left: std::time::Duration,
    /// The whole window, for the proportion of the bar.
    pub total: std::time::Duration,
    /// What comes back, as a phrase ("low, high and average since 11:30").
    pub what: &'a str,
}

/// Texts and state of a reset button. One click resets; while an Undo is on offer the same
/// button reads "Undo" instead, at the same place and size.
pub struct ResetButton<'a> {
    pub enabled: bool,
    /// The idle button: "Reset", "Reset layout".
    pub label: &'a str,
    pub tip: &'a str,
    pub disabled_tip: &'a str,
    pub undo: Option<UndoOffer<'a>>,
}

/// A button that resets on one click and then offers "Undo" for a while: a thin bar along its
/// bottom edge shows the time left, in whole seconds (a stepped readout, not an animation).
/// Returns whether it was clicked; the caller knows from its own state whether that meant Reset
/// or Undo. It is one widget in one place in both states, so keyboard focus stays on it.
pub fn reset_button(ui: &mut egui::Ui, pal: &Palette, c: &ResetButton) -> bool {
    use crate::a11y::{self, Spoken};
    let pad = ui.spacing().button_padding.x;
    let width_of = |text: &str| {
        egui::WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button).size().x + 2.0 * pad
    };
    // "Undo" takes the room of the button it replaces, so nothing around it moves
    let size = egui::vec2(width_of(c.label).max(width_of("Undo")), 0.0);
    let Some(undo) = &c.undo else {
        let mut b = ui.add_enabled(c.enabled, egui::Button::new(c.label).min_size(size)).on_hover_text(c.tip);
        if !c.disabled_tip.is_empty() {
            b = b.on_disabled_hover_text(c.disabled_tip);
        }
        return b.clicked();
    };
    let secs = undo.left.as_secs_f32().ceil().max(1.0);
    let tip = format!("Bring back {}. {secs:.0} s left; Ctrl+Z undoes the latest reset.", undo.what);
    // added the same way as the idle button (add_enabled makes a child ui), so its id and the focus stay
    let b = ui.add_enabled(true, egui::Button::new("Undo").min_size(size)).on_hover_text(tip);
    let total = undo.total.as_secs();
    a11y::publish(&b, || {
        Spoken::button("Undo reset").described(format!("Brings back {}. Available for {total} seconds.", undo.what))
    });
    // the bar: a track in the rule colour, the time left in ink 3, inset from the rounded corners
    let frac = (secs / undo.total.as_secs_f32().max(1.0)).clamp(0.0, 1.0);
    let r = b.rect;
    let (x0, x1, y) = (r.left() + 7.0, r.right() - 7.0, r.bottom() - 4.0);
    if x1 > x0 {
        let stroke = |color| egui::Stroke::new(2.0, color);
        ui.painter().line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], stroke(pal.rule));
        ui.painter().line_segment([egui::pos2(x0, y), egui::pos2(x0 + (x1 - x0) * frac, y)], stroke(pal.ink3));
    }
    b.clicked()
}

/// A row of controls against the right edge, laid out left to right. A right-to-left layout
/// creates its rightmost control first, so Tab would visit them right to left; here they are
/// created in reading order and the row is placed with the width it had a frame ago. The first
/// time (or after a change of content) the width is not known: the first time `add` also runs once
/// in an invisible, disabled ui to measure it, so even the first frame puts the row where it
/// belongs; a change settles on the next frame.
pub fn right_cluster<R>(ui: &mut egui::Ui, mut add: impl FnMut(&mut egui::Ui) -> R) -> R {
    let key = ui.id().with("right-cluster-width");
    let avail = ui.available_rect_before_wrap();
    let ltr = egui::Layout::left_to_right(egui::Align::Center);
    let known = ui.ctx().data(|d| d.get_temp::<f32>(key));
    let prev = match known {
        Some(w) => w,
        None => {
            let mut probe = ui.new_child(egui::UiBuilder::new().max_rect(avail).layout(ltr).invisible());
            add(&mut probe);
            probe.min_rect().width()
        }
    };
    let rect = egui::Rect::from_min_max(egui::pos2((avail.right() - prev).max(avail.left()), avail.top()), avail.right_bottom());
    let out = ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(ltr), |ui| {
        let r = add(ui);
        (r, ui.min_rect().width())
    });
    let (r, w) = out.inner;
    if known.is_none() || (w - prev).abs() > 0.5 {
        ui.ctx().data_mut(|d| d.insert_temp(key, w));
        if known.is_some() {
            visibility::repaint(ui.ctx());
        }
    }
    r
}

/// The drag since the last frame, counting the first frame from where the pointer was pressed:
/// egui starts a drag only after the pointer has moved a few pixels (the slop), and a handle
/// that ignored those would sit that far behind the pointer. Now that the divider's hit area is
/// wider, the pointer stays inside it for those pixels.
pub fn drag_delta(ui: &egui::Ui, resp: &egui::Response) -> egui::Vec2 {
    if resp.drag_started()
        && let Some((o, p)) = ui.input(|i| i.pointer.press_origin().zip(i.pointer.latest_pos()))
    {
        return p - o;
    }
    resp.drag_delta()
}

/// A divider moved from the keyboard goes this far per arrow press (WCAG 2.5.7: whatever dragging
/// does has a path without dragging), or `DIVIDER_STEP_BIG` with Shift held.
pub const DIVIDER_STEP: f32 = 4.0;
pub const DIVIDER_STEP_BIG: f32 = 32.0;

/// What a key asks of the divider that has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DividerKey {
    /// Move the divider this many pixels: positive is to the right (a divider that stands upright)
    /// or down (one that lies flat).
    Move(f32),
    /// Home: back to the default position or size.
    Reset,
}

/// The arrow or Home key a focused divider takes this frame, consumed so nothing else acts on it.
/// `upright` is a vertical line that moves left and right; otherwise it lies flat and moves up
/// and down. Shift makes the step `DIVIDER_STEP_BIG`.
pub fn divider_key(resp: &egui::Response, upright: bool) -> Option<DividerKey> {
    use egui::{Key, Modifiers};
    if !resp.has_focus() {
        return None;
    }
    crate::a11y::keep_keys(resp, !upright, upright, false);
    let (less, more) = if upright { (Key::ArrowLeft, Key::ArrowRight) } else { (Key::ArrowUp, Key::ArrowDown) };
    resp.ctx.input_mut(|i| {
        // Shift first: a plain key also matches while Shift is held
        for (mods, step) in [(Modifiers::SHIFT, DIVIDER_STEP_BIG), (Modifiers::NONE, DIVIDER_STEP)] {
            if i.consume_key(mods, less) {
                return Some(DividerKey::Move(-step));
            }
            if i.consume_key(mods, more) {
                return Some(DividerKey::Move(step));
            }
        }
        i.consume_key(Modifiers::NONE, Key::Home).then_some(DividerKey::Reset)
    })
}

/// How far in from a row's left edge a menu opened from the keyboard sits (design pixels).
const MENU_KEY_INSET: f32 = 24.0;

/// A context menu that stays open for clicks inside it (one-shot actions call `ui.close()`).
/// Right click opens it under `row` at the pointer's x, or above when there is no room; with
/// `by_key` (Shift+F10 on the control) it opens under the row near its left edge. A menu opened
/// from the keyboard hands the focus back to `refocus` when it closes, so the keyboard user is
/// where they were. Never over the row's own value.
pub fn row_menu(resp: &egui::Response, row: egui::Rect, by_key: bool, refocus: egui::Id, add: impl FnOnce(&mut egui::Ui)) {
    use egui::Popup;
    let ctx = &resp.ctx;
    let pid = Popup::default_response_id(resp);
    let at_id = pid.with("opened-at-x");
    let key_id = pid.with("opened-by-key");
    if by_key {
        ctx.data_mut(|d| {
            d.insert_temp(at_id, row.left() + crate::theme::fs(MENU_KEY_INSET));
            d.insert_temp(key_id, true);
        });
        Popup::open_id(ctx, pid);
    } else if let (true, Some(p)) = (resp.secondary_clicked(), resp.interact_pointer_pos()) {
        ctx.data_mut(|d| {
            d.insert_temp(at_id, p.x);
            d.remove_temp::<bool>(key_id);
        });
    }
    let x = ctx.data(|d| d.get_temp::<f32>(at_id)).unwrap_or(row.left());
    let anchor = egui::Rect::from_min_max(egui::pos2(x, row.top()), egui::pos2(x, row.bottom()));
    Popup::context_menu(resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .anchor(anchor)
        .align(egui::RectAlign::BOTTOM_START)
        .align_alternatives(&[egui::RectAlign::TOP_START])
        .gap(2.0)
        .show(add);
    if !Popup::is_id_open(ctx, pid) && ctx.data(|d| d.get_temp::<bool>(key_id)).unwrap_or(false) {
        ctx.data_mut(|d| d.remove_temp::<bool>(key_id));
        ctx.memory_mut(|m| m.request_focus(refocus));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_and_unit_are_split() {
        assert_eq!(split_value(Unit::Celsius, 67.24), ("67.2".into(), "°C".into()));
        assert_eq!(split_value(Unit::Percent, 31.4), ("31.4".into(), "%".into()));
        assert_eq!(split_value(Unit::Mhz, 4424.0), ("4424".into(), "MHz".into()));
        assert_eq!(split_value(Unit::Bytes, 23.3 * 1024.0 * 1024.0 * 1024.0), ("23.3".into(), "GiB".into()));
        assert_eq!(split_value(Unit::BytesPerSec, 870.4e6), ("870.4".into(), "MB/s".into()));
        assert_eq!(split_value(Unit::Mts, 6000.0), ("6000".into(), "MT/s".into()));
        assert_eq!(split_value(Unit::Flag, 1.0), ("Yes".into(), String::new()));
        assert_eq!(split_value(Unit::Flag, 0.0), ("No".into(), String::new()));
    }

    #[test]
    fn a_row_prints_one_unit_chosen_from_its_largest_value() {
        let g = 1024.0 * 1024.0 * 1024.0;
        // memory row: current 23.6, low 20.1, high 38.4 GiB: bare numbers, unit once
        let u = RowUnit::new(Unit::Bytes, 38.4 * g);
        assert_eq!(u.number(23.6 * g), "23.6");
        assert_eq!(u.number(38.4 * g), "38.4");
        assert_eq!(u.split(23.6 * g), ("23.6".to_string(), "GiB"));
        // a small low value stays in the row's unit instead of switching to MiB
        assert_eq!(u.number(0.26 * g), "0.3");
        // a row that never reaches 1 GiB reads in MiB
        let m = RowUnit::new(Unit::Bytes, 900.0 * 1024.0 * 1024.0);
        assert_eq!(m.number(512.0 * 1024.0 * 1024.0), "512");
        assert_eq!(m.suffix(), "MiB");
        // rates: the largest value picks MB/s even when the low is a few KB/s
        let r = RowUnit::new(Unit::BytesPerSec, 870.4e6);
        assert_eq!(r.number(870.4e6), "870.4");
        assert_eq!(r.number(2.0e3), "0.0");
        assert_eq!(r.suffix(), "MB/s");
        assert_eq!(RowUnit::new(Unit::BytesPerSec, 3.2e9).split(3.2e9), ("3.20".to_string(), "GB/s"));
        assert_eq!(RowUnit::new(Unit::BytesPerSec, 4.0e3).suffix(), "KB/s");
        // other units never rescale
        assert_eq!(RowUnit::new(Unit::Celsius, 99.0).number(45.1), "45.1");
        assert_eq!(RowUnit::new(Unit::Watt, 140.0).number(134.1), "134.1");
        assert_eq!(RowUnit::new(Unit::Flag, 1.0).number(1.0), "Yes");
    }

    #[test]
    fn row_units_agree_with_the_shared_formatter() {
        // number + suffix is what `format_value` prints, for every unit at a typical value
        let cases = [
            (Unit::Percent, 31.4),
            (Unit::Celsius, 67.24),
            (Unit::Mhz, 4424.0),
            (Unit::Watt, 134.1),
            (Unit::Volt, 1.352),
            (Unit::Rpm, 2650.0),
            (Unit::Mts, 6000.0),
            (Unit::Bytes, 23.3 * GIB),
            (Unit::Bytes, 640.0 * MIB),
            (Unit::BytesPerSec, 870.4e6),
            (Unit::BytesPerSec, 3.2e9),
            (Unit::BytesPerSec, 12.0e3),
        ];
        for (unit, v) in cases {
            let (n, s) = RowUnit::new(unit, v).split(v);
            assert_eq!(format!("{n} {s}"), format_value(unit, v), "{unit:?}");
        }
    }

    #[test]
    fn the_row_unit_looks_at_every_value_of_the_row() {
        let r = Reading {
            id: "x".into(),
            label: "X".into(),
            unit: Unit::Bytes,
            value: Some(300.0 * MIB),
            stats: Some(hwglance_proto::Stats { min: 200.0 * MIB, max: 3.0 * GIB, avg: 1.0 * GIB, samples: 5 }),
            note: None,
            desc: None,
            events: None,
            warn: None,
            crit: None,
        };
        let u = RowUnit::of(&r);
        assert_eq!(u.suffix(), "GiB", "the high decides, not the current value");
        assert_eq!(u.number(300.0 * MIB), "0.3");
    }

    #[test]
    fn devices_from_card_ids() {
        assert_eq!(Device::of_card("cpu"), Device::Cpu);
        assert_eq!(Device::of_card("gpu0"), Device::Gpu);
        assert_eq!(Device::of_card("memory"), Device::Memory);
        assert_eq!(Device::of_card("disk/S6XXNJ0R412087B"), Device::Drive);
        assert_eq!(Device::of_card("board"), Device::Board);
        assert_eq!(Device::of_card("igpu"), Device::Igpu);
        assert_eq!(Device::of_card("something-new"), Device::Board);
        assert_eq!(Device::of_reading("gpu0/temp/core"), Device::Gpu);
        assert_eq!(Device::Drive.legend(), "Drives");
        assert_eq!(Device::Drive.short(), "Drive");
    }

    #[test]
    fn icons_are_material_glyphs_not_plain_symbols() {
        for c in [icon::COG, icon::EYE, icon::PAUSE, icon::CHEVRON_DOWN, icon::CHEVRON_RIGHT, icon::THERMOMETER] {
            assert!((c as u32) >= 0xF0001, "{c:?}");
        }
    }
}
