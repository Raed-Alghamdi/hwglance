//! Threshold crossings in the window: what ran hot and since when. The collector records them
//! (`Snapshot::crossings`); this module turns them into the top bar's "2 crossings since 11:30"
//! chip and its list, the hollow ring on the legend chips, and the words in the history popover.
//! The view model is pure and tested; the chip and the list paint it.

use crate::sensors::flag_marker;
use crate::theme::{fs, snap4, Level, Palette};
use crate::ui_kit::{ellipsized, f_flag, f_label, f_label_bold, f_minmax, f_small, fmt_clock, Device};
use egui::{pos2, vec2, Color32, Rect, Sense, Stroke, Ui};
use hwglance_proto::{format_value, Crossing, CrossingLevel, Reading, SensorGroup, Snapshot, Unit};
use std::collections::HashMap;

pub fn level(c: &Crossing) -> Level {
    match c.level {
        CrossingLevel::Warn => Level::Warn,
        CrossingLevel::Crit => Level::Crit,
    }
}

/// A crossing together with the reading it belongs to.
#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    pub c: &'a Crossing,
    pub device: Device,
    pub unit: Unit,
    pub card: &'a str,
    pub label: &'a str,
}

impl Entry<'_> {
    /// "CPU, Tctl"
    pub fn title(&self) -> String {
        format!("{}, {}", self.device.short(), self.label)
    }
}

/// The snapshot's crossings whose reading is still there and whose card is not hidden, newest
/// first.
pub fn entries<'a>(snap: &'a Snapshot, hidden_card: impl Fn(&str) -> bool) -> Vec<Entry<'a>> {
    if snap.crossings.is_empty() {
        return Vec::new();
    }
    let mut by_id: HashMap<&str, (&SensorGroup, &Reading)> = HashMap::new();
    for g in snap.sensors.iter().filter(|g| !hidden_card(&g.id)) {
        for r in g.sections.iter().flat_map(|s| &s.readings) {
            by_id.entry(r.id.as_str()).or_insert((g, r));
        }
    }
    let mut out: Vec<Entry> = snap
        .crossings
        .iter()
        .filter_map(|c| {
            let (g, r) = by_id.get(c.id.as_str())?;
            Some(Entry { c, device: Device::of_card(&g.id), unit: r.unit, card: &g.id, label: &r.label })
        })
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.c.start_ms));
    out
}

/// Still going, or over for less than `window_ms`.
pub fn is_recent(c: &Crossing, now_ms: u64, window_ms: u64) -> bool {
    c.end_ms.is_none_or(|end| now_ms.saturating_sub(end) <= window_ms)
}

fn rank(l: Level) -> u8 {
    match l {
        Level::Normal => 0,
        Level::Warn => 1,
        Level::Crit => 2,
    }
}

/// The worst level among the recent crossings of `device` (all devices when `None`).
pub fn recent_level(entries: &[Entry], device: Option<Device>, now_ms: u64, window_ms: u64) -> Option<Level> {
    entries
        .iter()
        .filter(|e| device.is_none_or(|d| d == e.device) && is_recent(e.c, now_ms, window_ms))
        .map(|e| level(e.c))
        .max_by_key(|l| rank(*l))
}

/// The newest crossing of `device` that is recent, for the legend chip's tooltip.
pub fn latest_recent<'a>(entries: &[Entry<'a>], device: Device, now_ms: u64, window_ms: u64) -> Option<Entry<'a>> {
    entries.iter().filter(|e| e.device == device && is_recent(e.c, now_ms, window_ms)).max_by_key(|e| e.c.start_ms).copied()
}

/// "2 crossings since 11:30", from the oldest start.
pub fn chip_text(entries: &[Entry]) -> String {
    let since = entries.iter().map(|e| e.c.start_ms).min().unwrap_or(0);
    format!("{} {} since {}", entries.len(), if entries.len() == 1 { "crossing" } else { "crossings" }, fmt_clock(since))
}

/// "11:30 to 11:42", or "11:30 to now" while it lasts.
pub fn span_text(c: &Crossing) -> String {
    match c.end_ms {
        Some(end) => format!("{} to {}", fmt_clock(c.start_ms), fmt_clock(end)),
        None => format!("{} to now", fmt_clock(c.start_ms)),
    }
}

/// "12 min", "1 h 5 min", "under a minute".
pub fn duration_text(ms: u64) -> String {
    let min = ms / 60_000;
    match min {
        0 => "under a minute".to_string(),
        1..=59 => format!("{min} min"),
        _ if min.is_multiple_of(60) => format!("{} h", min / 60),
        _ => format!("{} h {} min", min / 60, min % 60),
    }
}

/// How long it lasted (so far, while it lasts).
pub fn lasted(c: &Crossing, now_ms: u64) -> u64 {
    c.end_ms.unwrap_or(now_ms).saturating_sub(c.start_ms)
}

/// "peak 91.5 °C", or "low 350 RPM" for a reading that alerts below.
pub fn peak_text(unit: Unit, c: &Crossing) -> String {
    format!("{} {}", if c.below { "low" } else { "peak" }, format_value(unit, c.peak))
}

/// The marker letters in the flag column's style: "H", "HH", "L", "LL".
pub fn mark(c: &Crossing) -> &'static str {
    flag_marker(level(c), c.below).map(|m| m.0).unwrap_or("")
}

/// A sentence for tooltips.
pub fn sentence(e: &Entry, now_ms: u64) -> String {
    let c = e.c;
    let word = if c.level == CrossingLevel::Crit { "critical" } else { "warning" };
    let dir = if c.below { "below" } else { "above" };
    let value = format_value(e.unit, c.peak);
    let peak = if c.below {
        format!("its lowest was {value} at {}", fmt_clock(c.peak_ms))
    } else {
        format!("it peaked at {value} at {}", fmt_clock(c.peak_ms))
    };
    match c.end_ms {
        None => format!("{} has been {dir} its {word} level since {}; {peak}.", e.title(), fmt_clock(c.start_ms)),
        Some(end) => format!(
            "{} was {dir} its {word} level from {} to {} ({}); {peak}.",
            e.title(),
            fmt_clock(c.start_ms),
            fmt_clock(end),
            duration_text(lasted(c, now_ms))
        ),
    }
}

// ---------------------------------------------------------------------------
// The chip
// ---------------------------------------------------------------------------

/// How much of the chip shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipForm {
    /// "2 crossings since 11:30".
    Text,
    /// Just the count (the words are in the tooltip), for a narrow bar.
    Count,
}

pub fn chip_width(text_w: f32) -> f32 {
    text_w + 14.0 + 20.0
}

/// The chip's text for a form.
pub fn chip_label(entries: &[Entry], form: ChipForm) -> String {
    match form {
        ChipForm::Text => chip_text(entries),
        ChipForm::Count => entries.len().to_string(),
    }
}

/// The top bar's chip. `tone` is the worst recent level: an outlined, tinted chip in amber or
/// red while some crossing is recent, plain grey words when they are all older.
pub fn chip(ui: &mut Ui, pal: &Palette, entries: &[Entry], tone: Option<Level>, form: ChipForm, tip: &str) -> egui::Response {
    let color = tone.and_then(|l| pal.level_color(l)).unwrap_or(pal.ink3);
    let ink = if tone.is_some() { pal.ink } else { pal.ink2 };
    let g = ui.painter().layout_no_wrap(chip_label(entries, form), f_label(), ink);
    let size = vec2(chip_width(g.size().x), snap4(28.0 * crate::theme::font_scale()));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 14.0, pal.hover);
    }
    if tone.is_some() {
        ui.painter().rect(rect, 14.0, color.gamma_multiply(0.14), Stroke::new(1.5, color), egui::StrokeKind::Inside);
    } else {
        ui.painter().rect_stroke(rect, 14.0, Stroke::new(1.0, pal.rule), egui::StrokeKind::Inside);
    }
    ui.painter().circle_filled(pos2(rect.left() + 12.0, rect.center().y), 4.0, color);
    ui.painter().galley(pos2(rect.left() + 22.0, rect.center().y - g.size().y / 2.0), g, ink);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, chip_text(entries)));
    resp.on_hover_text(tip)
}

// ---------------------------------------------------------------------------
// The list
// ---------------------------------------------------------------------------

pub fn list_width() -> f32 {
    fs(620.0)
}

/// The list under the chip: one row per crossing, newest first, with the reading, how bad, the
/// peak and when. Returns the reading id of a row that was clicked.
pub fn list(ui: &mut Ui, pal: &Palette, entries: &[Entry], now_ms: u64, window_ms: u64) -> Option<String> {
    let w = list_width();
    ui.set_width(w);
    ui.label(egui::RichText::new("Threshold crossings").font(f_label_bold()).color(pal.ink));
    ui.label(egui::RichText::new("Click one to see where it is and its history. Reset clears the ones that are over.").font(f_small()).color(pal.ink3));
    ui.add_space(6.0);
    let mut clicked = None;
    let row_h = snap4(28.0 * crate::theme::font_scale());
    egui::ScrollArea::vertical().max_height(fs(360.0)).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for e in entries {
            let (rect, resp) = ui.allocate_exact_size(vec2(w, row_h), Sense::click());
            if ui.is_rect_visible(rect) {
                paint_row(ui.painter(), pal, rect, e, now_ms, is_recent(e.c, now_ms, window_ms), resp.hovered());
            }
            if resp.clicked() {
                clicked = Some(e.c.id.clone());
                ui.close();
            }
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, sentence(e, now_ms)));
            resp.on_hover_text(sentence(e, now_ms));
        }
    });
    clicked
}

/// Column positions of a list row.
pub struct RowCols {
    pub mark: Rect,
    pub name: Rect,
    pub peak: Rect,
    pub span: Rect,
}

pub fn row_cols(rect: Rect) -> RowCols {
    let pad = 10.0;
    let (peak_w, span_w, mark_w) = (fs(120.0), fs(200.0), fs(32.0));
    let cut = |l: f32, r: f32| Rect::from_min_max(pos2(l, rect.top()), pos2(r, rect.bottom()));
    let span = cut(rect.right() - pad - span_w, rect.right() - pad);
    let peak = cut(span.left() - 8.0 - peak_w, span.left() - 8.0);
    let mark = cut(rect.left() + pad, rect.left() + pad + mark_w);
    let name = cut(mark.right() + 8.0, peak.left() - 8.0);
    RowCols { mark, name, peak, span }
}

fn paint_row(p: &egui::Painter, pal: &Palette, rect: Rect, e: &Entry, now_ms: u64, recent: bool, hovered: bool) {
    if hovered {
        p.rect_filled(rect, 4.0, pal.hover);
    }
    let cols = row_cols(rect);
    let cy = rect.center().y;
    // A crossing that is over keeps its letters in quiet ink; amber and red are for what is recent.
    let color = if recent { pal.level_color(level(e.c)).unwrap_or(pal.ink2) } else { pal.ink3 };
    p.text(pos2(cols.mark.left(), cy), egui::Align2::LEFT_CENTER, mark(e.c), f_flag(), color);
    let dev = e.device.color(pal);
    p.circle_filled(pos2(cols.name.left() + 4.0, cy), 4.0, dev);
    let name = ellipsized(p, &e.title(), f_label(), pal.ink2, (cols.name.width() - 14.0).max(20.0));
    p.galley(pos2(cols.name.left() + 14.0, cy - name.size().y / 2.0), name, pal.ink2);
    p.text(pos2(cols.peak.right(), cy), egui::Align2::RIGHT_CENTER, peak_text(e.unit, e.c), f_minmax(), if recent { pal.ink } else { pal.ink2 });
    let span = format!("{}  {}", span_text(e.c), duration_text(lasted(e.c, now_ms)));
    let galley = ellipsized(p, &span, f_minmax(), pal.ink3, cols.span.width());
    p.galley(pos2(cols.span.right() - galley.size().x, cy - galley.size().y / 2.0), galley, pal.ink3);
}

/// The ring of a legend chip: its colour and whether it is the hollow kind (a crossing that is
/// recent but not active now). `now` is the device's current level.
pub fn ring_for(now: Level, recent: Option<Level>, pal: &Palette) -> Option<(Color32, bool)> {
    let worst = if rank(recent.unwrap_or(Level::Normal)) > rank(now) { recent.unwrap_or(Level::Normal) } else { now };
    let color = pal.level_color(worst)?;
    Some((color, now == Level::Normal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwglance_proto::{Section, Stats};

    const MIN: u64 = 60_000;

    fn crossing(id: &str, level: CrossingLevel, start: u64, end: Option<u64>, peak: f64) -> Crossing {
        Crossing { id: id.into(), level, start_ms: start, peak, peak_ms: start + MIN, end_ms: end, below: false }
    }

    fn reading(id: &str, label: &str, unit: Unit) -> Reading {
        Reading {
            id: id.into(),
            label: label.into(),
            unit,
            value: Some(50.0),
            stats: Some(Stats::first(50.0)),
            note: None,
            desc: None,
            events: None,
            warn: Some(80.0),
            crit: Some(89.0),
        }
    }

    fn snap(crossings: Vec<Crossing>) -> Snapshot {
        let group = |id: &str, title: &str, rs: Vec<Reading>| SensorGroup {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            sections: vec![Section { id: format!("{id}/temp"), title: "Temps".into(), collapsed_by_default: false, readings: rs, summary: None }],
        };
        let mut s = crate::demo::Demo::new().snapshot(0.0);
        s.sensors = vec![
            group("cpu", "CPU", vec![reading("cpu/temp/tctl", "Tctl", Unit::Celsius)]),
            group("gpu0", "GPU", vec![reading("gpu0/temp/core", "Core", Unit::Celsius)]),
            group("igpu", "iGPU", vec![reading("igpu/main/temp", "Edge temperature", Unit::Celsius)]),
        ];
        s.crossings = crossings;
        s
    }

    #[test]
    fn the_list_is_newest_first_and_only_has_readings_that_are_there() {
        let s = snap(vec![
            crossing("cpu/temp/tctl", CrossingLevel::Warn, 100 * MIN, Some(110 * MIN), 84.0),
            crossing("gone/temp/x", CrossingLevel::Crit, 105 * MIN, None, 99.0),
            crossing("gpu0/temp/core", CrossingLevel::Crit, 120 * MIN, None, 91.0),
        ]);
        let e = entries(&s, |_| false);
        assert_eq!(e.iter().map(|e| e.c.id.as_str()).collect::<Vec<_>>(), ["gpu0/temp/core", "cpu/temp/tctl"]);
        assert_eq!(e[0].title(), "GPU, Core");
        assert_eq!((e[0].device, e[1].device), (Device::Gpu, Device::Cpu));
        // a hidden card takes its crossings with it
        let hidden = entries(&s, |id| id == "cpu");
        assert_eq!(hidden.len(), 1);
        assert!(entries(&snap(vec![]), |_| false).is_empty());
        assert_eq!(entries(&s, |_| false)[0].unit, Unit::Celsius);
    }

    #[test]
    fn the_chip_counts_and_names_the_oldest_start() {
        let s = snap(vec![
            crossing("cpu/temp/tctl", CrossingLevel::Warn, 100 * MIN, Some(110 * MIN), 84.0),
            crossing("gpu0/temp/core", CrossingLevel::Crit, 120 * MIN, None, 91.0),
        ]);
        let e = entries(&s, |_| false);
        let text = chip_text(&e);
        assert_eq!(text, format!("2 crossings since {}", fmt_clock(100 * MIN)));
        assert_eq!(chip_label(&e, ChipForm::Count), "2");
        let one = entries(&s, |id| id == "cpu");
        assert_eq!(chip_text(&one), format!("1 crossing since {}", fmt_clock(120 * MIN)));
        assert!(chip_width(100.0) > 100.0);
    }

    #[test]
    fn recent_means_going_or_over_for_less_than_the_history_window() {
        let window = 30 * MIN;
        let going = crossing("a", CrossingLevel::Warn, 0, None, 1.0);
        let over = crossing("a", CrossingLevel::Warn, 0, Some(10 * MIN), 1.0);
        assert!(is_recent(&going, 500 * MIN, window), "an ongoing one is always recent");
        assert!(is_recent(&over, 40 * MIN, window), "over for 30 minutes: still in the window");
        assert!(!is_recent(&over, 40 * MIN + 1, window));
    }

    #[test]
    fn a_device_keeps_the_worst_recent_level_for_its_ring() {
        let now = 200 * MIN;
        let s = snap(vec![
            crossing("cpu/temp/tctl", CrossingLevel::Warn, 150 * MIN, Some(160 * MIN), 84.0),
            crossing("cpu/temp/tctl", CrossingLevel::Crit, 100 * MIN, Some(110 * MIN), 92.0), // too old
            crossing("igpu/main/temp", CrossingLevel::Crit, 190 * MIN, Some(195 * MIN), 95.0),
        ]);
        let e = entries(&s, |_| false);
        let w = 60 * MIN;
        assert_eq!(recent_level(&e, Some(Device::Cpu), now, w), Some(Level::Warn));
        assert_eq!(recent_level(&e, Some(Device::Cpu), now, 30 * MIN), None, "a short window forgets the warning");
        assert_eq!(recent_level(&e, Some(Device::Igpu), now, w), Some(Level::Crit), "the iGPU has a ring too");
        assert_eq!(recent_level(&e, Some(Device::Gpu), now, w), None);
        assert_eq!(recent_level(&e, None, now, w), Some(Level::Crit));
        assert_eq!(latest_recent(&e, Device::Cpu, now, w).unwrap().c.start_ms, 150 * MIN);
    }

    #[test]
    fn the_ring_is_hollow_when_only_history_and_solid_while_it_is_going_on() {
        let pal = Palette::dark();
        assert_eq!(ring_for(Level::Normal, None, &pal), None);
        assert_eq!(ring_for(Level::Normal, Some(Level::Warn), &pal), Some((pal.warn, true)));
        assert_eq!(ring_for(Level::Warn, None, &pal), Some((pal.warn, false)));
        // a recent critical crossing outranks the warning that is on now, and the chip is not hollow
        assert_eq!(ring_for(Level::Warn, Some(Level::Crit), &pal), Some((pal.crit, false)));
        assert_eq!(ring_for(Level::Crit, Some(Level::Warn), &pal), Some((pal.crit, false)));
    }

    #[test]
    fn the_words_of_a_crossing() {
        let s = snap(vec![
            crossing("cpu/temp/tctl", CrossingLevel::Warn, 100 * MIN, Some(112 * MIN), 84.0),
            crossing("gpu0/temp/core", CrossingLevel::Crit, 120 * MIN, None, 91.5),
        ]);
        let e = entries(&s, |_| false);
        let (gpu, cpu) = (&e[0], &e[1]);
        assert_eq!(mark(gpu.c), "HH");
        assert_eq!(mark(cpu.c), "H");
        assert_eq!(peak_text(Unit::Celsius, gpu.c), "peak 91.5 °C");
        assert_eq!(span_text(cpu.c), format!("{} to {}", fmt_clock(100 * MIN), fmt_clock(112 * MIN)));
        assert_eq!(span_text(gpu.c), format!("{} to now", fmt_clock(120 * MIN)));
        assert_eq!(duration_text(lasted(cpu.c, 130 * MIN)), "12 min");
        assert_eq!(duration_text(lasted(gpu.c, 120 * MIN + 65 * MIN)), "1 h 5 min");
        assert_eq!(duration_text(30_000), "under a minute");
        assert_eq!(duration_text(2 * 60 * MIN), "2 h");
        let s_gpu = sentence(gpu, 125 * MIN);
        assert!(s_gpu.starts_with("GPU, Core has been above its critical level since "), "{s_gpu}");
        assert!(s_gpu.contains("peaked at 91.5 °C"), "{s_gpu}");
        let s_cpu = sentence(cpu, 125 * MIN);
        assert!(s_cpu.contains("was above its warning level from") && s_cpu.contains("(12 min)"), "{s_cpu}");
        // a reading that alerts below
        let mut low = crossing("gpu0/temp/core", CrossingLevel::Crit, 120 * MIN, None, 300.0);
        low.below = true;
        assert_eq!(mark(&low), "LL");
        assert!(peak_text(Unit::Rpm, &low).starts_with("low 300"));
    }

    #[test]
    fn list_rows_have_room_for_each_column() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(list_width(), 28.0));
        let c = row_cols(rect);
        assert!(c.mark.right() <= c.name.left() && c.name.right() <= c.peak.left() && c.peak.right() <= c.span.left());
        assert!(c.name.width() >= 120.0, "{}", c.name.width());
        assert!(c.span.right() <= rect.right());
    }
}
