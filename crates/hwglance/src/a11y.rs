//! What assistive technology and the keyboard get from the painted controls: one place that
//! composes a role, a name and a state (`Spoken`), the words a reading or a process is announced
//! with, the focus ring and the menu key. Everything the window paints itself (card headers, rows,
//! chips, dividers, grips) goes through `publish`, so no painted control is a bare, unnamed node.
//!
//! The tree is only built once an assistive technology asks for it (egui's `enable_accesskit`,
//! called by eframe on the bridge's first request), so `publish` costs nothing otherwise: its
//! closure, which formats the name, never runs.

use crate::theme::{Level, Palette};
use crate::thresholds::{level_of, Thr};
use crate::ui_kit::split_value;
use egui::accesskit::{self, Action, Role, SortDirection};
use egui::{Color32, Context, EventFilter, Key, Painter, Rect, Response, Stroke, StrokeKind, Ui};
use hwglance_proto::{format_value, Reading, Unit};

// ---------------------------------------------------------------------------
// Role, name and state
// ---------------------------------------------------------------------------

/// A control as a screen reader says it: its role, its name by purpose (never its glyph), a
/// description for what the name leaves out, and the state it is in.
#[derive(Clone, Debug)]
pub struct Spoken {
    pub role: Role,
    pub name: String,
    pub description: Option<String>,
    pub value: Option<String>,
    /// Open or closed, for a header, a section or a group that folds.
    pub expanded: Option<bool>,
    pub selected: Option<bool>,
    pub sort: Option<SortDirection>,
    /// Opens a menu on the context-menu key.
    pub menu: bool,
}

impl Spoken {
    pub fn new(role: Role, name: impl Into<String>) -> Spoken {
        Spoken { role, name: name.into(), description: None, value: None, expanded: None, selected: None, sort: None, menu: false }
    }
    pub fn button(name: impl Into<String>) -> Spoken {
        Spoken::new(Role::Button, name)
    }
    pub fn described(mut self, d: impl Into<String>) -> Spoken {
        self.description = Some(d.into());
        self
    }
    pub fn valued(mut self, v: impl Into<String>) -> Spoken {
        self.value = Some(v.into());
        self
    }
    pub fn expanded(mut self, open: bool) -> Spoken {
        self.expanded = Some(open);
        self
    }
    pub fn selected(mut self, on: bool) -> Spoken {
        self.selected = Some(on);
        self
    }
    pub fn sorted(mut self, descending: bool) -> Spoken {
        self.sort = Some(if descending { SortDirection::Descending } else { SortDirection::Ascending });
        self
    }
    pub fn with_menu(mut self) -> Spoken {
        self.menu = true;
        self
    }
}

/// Give `resp`'s node its role, name and state. `make` runs only while a tree is being built.
/// Works on controls egui made no node for (a click-only painted row) as well as on the ones it
/// did (a button whose glyph name is replaced by its purpose).
pub fn publish(resp: &Response, make: impl FnOnce() -> Spoken) {
    resp.ctx.accesskit_node_builder(resp.id, |n| {
        let s = make();
        n.set_bounds(accesskit::Rect {
            x0: resp.rect.min.x.into(),
            y0: resp.rect.min.y.into(),
            x1: resp.rect.max.x.into(),
            y1: resp.rect.max.y.into(),
        });
        if !resp.enabled() {
            n.set_disabled();
        }
        if resp.sense.is_focusable() {
            n.add_action(Action::Focus);
        }
        if resp.sense.senses_click() {
            n.add_action(Action::Click);
        }
        n.set_role(s.role);
        n.set_label(s.name);
        if let Some(d) = s.description {
            n.set_description(d);
        }
        if let Some(v) = s.value {
            n.set_value(v);
        }
        if let Some(open) = s.expanded {
            n.set_expanded(open);
        }
        if let Some(on) = s.selected {
            n.set_selected(on);
        }
        if let Some(d) = s.sort {
            n.set_sort_direction(d);
        }
        if s.menu {
            n.set_has_popup(accesskit::HasPopup::Menu);
        }
    });
}

/// Name the edge and corner grabs of an egui window (they are drag-only handles egui leaves bare):
/// "Resize the Settings window, right edge". `layer` is the window's layer.
pub fn name_window_edges(ctx: &Context, layer: egui::LayerId, window: &str) {
    let base = egui::Id::new(layer).with("edge_drag");
    for (suffix, side) in [
        ("right", "right edge"),
        ("left", "left edge"),
        ("bottom", "bottom edge"),
        ("top", "top edge"),
        ("right_bottom", "bottom right corner"),
        ("right_top", "top right corner"),
        ("left_bottom", "bottom left corner"),
        ("left_top", "top left corner"),
    ] {
        ctx.accesskit_node_builder(base.with(suffix), |n| {
            n.set_role(Role::Splitter);
            n.set_label(format!("Resize the {window} window, {side}"));
        });
    }
}

/// Hide a painted duplicate (the pinned copy of a card header) from assistive technology.
pub fn hide(resp: &Response) {
    resp.ctx.accesskit_node_builder(resp.id, |n| n.set_hidden());
}

// ---------------------------------------------------------------------------
// Words
// ---------------------------------------------------------------------------

/// A number the way it is said: "80", "53.6", never "80.0".
pub fn plain_number(s: &str) -> String {
    match s.strip_suffix(".0") {
        Some(n) => n.to_string(),
        None => s.to_string(),
    }
}

/// The unit as a screen reader should say it.
pub fn unit_words(suffix: &str) -> &str {
    match suffix {
        "°C" => "degrees",
        "%" => "percent",
        "MHz" => "megahertz",
        "W" => "watts",
        "V" => "volts",
        "RPM" => "RPM",
        "MT/s" => "megatransfers per second",
        "GiB" => "gibibytes",
        "MiB" => "mebibytes",
        "KiB" => "kibibytes",
        "KB/s" => "kilobytes per second",
        "MB/s" => "megabytes per second",
        "GB/s" => "gigabytes per second",
        other => other,
    }
}

/// "53.6 degrees", "Yes", "no reading".
pub fn spoken_value(unit: Unit, v: Option<f64>) -> String {
    let Some(v) = v else { return "no reading".into() };
    let (num, suffix) = split_value(unit, v);
    if suffix.is_empty() { num } else { format!("{} {}", plain_number(&num), unit_words(&suffix)) }
}

/// The limits a reading carries: "warning above 80, critical above 95".
pub fn spoken_limits(unit: Unit, t: &Thr) -> Option<String> {
    let dir = if t.below { "below" } else { "above" };
    let n = |v: f64| plain_number(&split_value(unit, v).0);
    let mut parts = Vec::new();
    if let Some(w) = t.warn {
        parts.push(format!("warning {dir} {}", n(w)));
    }
    if let Some(c) = t.crit {
        parts.push(format!("critical {dir} {}", n(c)));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// A sensor row: "CPU, Tctl, 53.6 degrees, warning above 80". A reading past a limit says so
/// before the limits ("83.7 degrees, warning level, warning above 80"), and a watched one ends
/// with "watched".
pub fn reading_name(device: &str, r: &Reading, thr: &Thr, watched: bool) -> String {
    let mut parts = vec![device.to_string(), r.label.clone(), spoken_value(r.unit, r.value)];
    if let Some(v) = r.value {
        match level_of(v, thr) {
            Level::Normal => {}
            Level::Warn => parts.push("warning level".into()),
            Level::Crit => parts.push("critical level".into()),
        }
    }
    if r.unit != Unit::Flag {
        parts.extend(spoken_limits(r.unit, thr));
    }
    if watched {
        parts.push("watched".into());
    }
    parts.join(", ")
}

/// A card header: "CPU, AMD Ryzen 7 7800X3D, Throttled 3x, last 11:56".
pub fn card_name(device: &str, subtitle: Option<&str>, throttle: Option<&str>) -> String {
    let mut parts = vec![device.to_string()];
    parts.extend(subtitle.map(str::to_string));
    parts.extend(throttle.map(str::to_string));
    parts.join(", ")
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A process or an app group: "Brave, 10 processes, CPU 3 percent, memory 2.5 GiB".
pub fn process_name(name: &str, procs: usize, cpu_pct: f32, gpu_pct: Option<f32>, mem_bytes: u64) -> String {
    let mut parts = vec![name.to_string()];
    if procs > 1 {
        parts.push(count(procs, "process", "processes"));
    }
    parts.push(format!("CPU {} percent", plain_number(&format!("{cpu_pct:.1}"))));
    if let Some(g) = gpu_pct.filter(|g| *g >= 0.5) {
        parts.push(format!("GPU {} percent", plain_number(&format!("{g:.1}"))));
    }
    parts.push(format!("memory {}", format_value(Unit::Bytes, mem_bytes as f64)));
    parts.join(", ")
}

/// A class row of the process table: "Apps, 7 groups, 169 processes".
pub fn class_name(title: &str, groups: usize, procs: usize) -> String {
    format!("{title}, {}, {}", count(groups, "group", "groups"), count(procs, "process", "processes"))
}

// ---------------------------------------------------------------------------
// Focus
// ---------------------------------------------------------------------------

/// Width of the focus ring and the gap between it and the control.
pub const RING_W: f32 = 2.0;
pub const RING_GAP: f32 = 1.0;

/// Where the ring goes. Controls with room around them (chips, buttons) wear it outside; rows and
/// headers that touch their neighbours or the edge of a scroll area wear it inside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ring {
    Outside,
    Inside,
}

/// The rectangle whose edge the ring's stroke is drawn on, and how the stroke sits on it.
pub fn ring_rect(rect: Rect, place: Ring) -> (Rect, StrokeKind) {
    match place {
        Ring::Outside => (rect.expand(RING_GAP), StrokeKind::Outside),
        Ring::Inside => (rect.shrink(RING_GAP), StrokeKind::Inside),
    }
}

/// A 2 px ring in ink, one pixel off the control, in both palettes.
pub fn paint_ring(p: &Painter, rect: Rect, radius: f32, place: Ring, ink: Color32) {
    let (r, kind) = ring_rect(rect, place);
    let radius = match place {
        Ring::Outside => radius + RING_GAP,
        Ring::Inside => (radius - RING_GAP).max(0.0),
    };
    p.rect_stroke(r, radius, Stroke::new(RING_W, ink), kind);
}

/// The ring around `resp` while it has the keyboard. A mouse press anywhere gives the focus up
/// again, so a pointer user never sees it.
pub fn focus_ring(ui: &Ui, resp: &Response, radius: f32, place: Ring, pal: &Palette) {
    if resp.has_focus() {
        paint_ring(ui.painter(), resp.rect, radius, place, pal.ink);
    }
}

/// A control that handles keys itself (a card, the process table) says which, so egui does not
/// also walk the focus to the nearest widget on an arrow, or drop it on Esc.
pub fn keep_keys(resp: &Response, vertical: bool, horizontal: bool, escape: bool) {
    if resp.has_focus() {
        resp.ctx.memory_mut(|m| {
            m.set_focus_lock_filter(resp.id, EventFilter { vertical_arrows: vertical, horizontal_arrows: horizontal, escape, ..Default::default() })
        });
    }
}

/// Bring a control into view when the keyboard lands on it.
pub fn scroll_into_view(ui: &Ui, resp: &Response) {
    if resp.gained_focus() {
        ui.scroll_to_rect(resp.rect, None);
    }
}

/// Shift+F10: open the context menu of the focused control. (The Menu key reaches egui as no
/// event at all, because egui-winit has no mapping for it, so Shift+F10 is the one keyboard path.)
pub fn menu_key(ctx: &Context) -> bool {
    ctx.input(|i| i.key_pressed(Key::F10) && i.modifiers.shift_only())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(unit: Unit, value: Option<f64>) -> Reading {
        Reading { id: "cpu/temp/tctl".into(), label: "Tctl".into(), unit, value, stats: None, note: None, desc: None, events: None, warn: None, crit: None }
    }

    #[test]
    fn a_sensor_row_says_value_and_limits() {
        let t = Thr { warn: Some(80.0), crit: None, below: false };
        let r = reading(Unit::Celsius, Some(53.6));
        assert_eq!(reading_name("CPU", &r, &t, false), "CPU, Tctl, 53.6 degrees, warning above 80");
        // past a limit it says so, then the limits; a watched row says that last
        let hot = reading(Unit::Celsius, Some(83.7));
        let both = Thr { warn: Some(80.0), crit: Some(95.0), below: false };
        assert_eq!(reading_name("CPU", &hot, &both, true), "CPU, Tctl, 83.7 degrees, warning level, warning above 80, critical above 95, watched");
        // no limits, no value
        assert_eq!(reading_name("Board", &reading(Unit::Celsius, None), &Thr::default(), false), "Board, Tctl, no reading");
        // a below-threshold reads below
        let pump = Thr { warn: Some(1000.0), crit: Some(400.0), below: true };
        let fan = reading(Unit::Rpm, Some(700.0));
        assert_eq!(reading_name("Board", &fan, &pump, false), "Board, Tctl, 700 RPM, warning level, warning below 1000, critical below 400");
    }

    #[test]
    fn units_are_said_as_words() {
        assert_eq!(spoken_value(Unit::Percent, Some(31.0)), "31 percent");
        assert_eq!(spoken_value(Unit::Watt, Some(63.5)), "63.5 watts");
        assert_eq!(spoken_value(Unit::Mhz, Some(5049.0)), "5049 megahertz");
        assert_eq!(spoken_value(Unit::Flag, Some(1.0)), "Yes");
        assert_eq!(spoken_value(Unit::Flag, Some(0.0)), "No");
        assert_eq!(spoken_value(Unit::Bytes, Some(25.3 * 1024.0 * 1024.0 * 1024.0)), "25.3 gibibytes");
        assert_eq!(spoken_value(Unit::BytesPerSec, Some(870.4e6)), "870.4 megabytes per second");
    }

    #[test]
    fn a_process_row_says_who_and_how_much() {
        assert_eq!(process_name("Brave", 10, 3.0, None, (2.5 * 1024.0 * 1024.0 * 1024.0) as u64), "Brave, 10 processes, CPU 3 percent, memory 2.5 GiB");
        assert_eq!(process_name("bash", 1, 0.1, Some(12.0), 3 * 1024 * 1024), "bash, CPU 0.1 percent, GPU 12 percent, memory 3 MiB");
        assert_eq!(class_name("Apps", 7, 169), "Apps, 7 groups, 169 processes");
        assert_eq!(class_name("System", 1, 1), "System, 1 group, 1 process");
    }

    #[test]
    fn the_ring_is_two_pixels_one_pixel_off() {
        let r = Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(40.0, 20.0));
        let (out, k) = ring_rect(r, Ring::Outside);
        assert_eq!((out, k), (r.expand(1.0), StrokeKind::Outside));
        let (inn, k) = ring_rect(r, Ring::Inside);
        assert_eq!((inn, k), (r.shrink(1.0), StrokeKind::Inside));
        assert_eq!(RING_W, 2.0);
    }
}
