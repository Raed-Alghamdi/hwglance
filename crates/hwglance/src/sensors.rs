//! The sensor panels: one card per device, rows of label, result, min, range bar, max and flag.
//! Visible-item selection, collapsed summaries, watched values, context menus, tooltips, layout
//! editing (drag and drop) and the hover and click hits that open the history popover.

use crate::a11y::{self, Ring, Spoken};
use crate::layout::{self, Column, COL_GAP};
use crate::limits::{self, Throttle};
use crate::rangebar;
use crate::settings::{LayoutPrefs, Settings};
use crate::theme::{base, fs, snap4, Level, Palette};
use crate::thresholds::{default_thr, effective, level_of, Pending, Thr, ThrEdit};
use crate::ui_kit::{self, ellipsized, f_flag, f_label, f_label_bold, f_minmax, f_result, f_section, f_small, f_unit, icon, ResetButton, UndoOffer, Device, RowUnit};
use egui::{pos2, vec2, Align, Align2, Color32, DragAndDrop, FontId, Rect, RichText, Sense, Stroke, Ui};
use hwglance_proto::{format_value, Reading, Section, SensorGroup, Snapshot, Unit};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Pure selection logic
// ---------------------------------------------------------------------------

pub struct VisSection<'a> {
    pub sec: &'a Section,
    pub readings: Vec<&'a Reading>,
}

/// Sections and readings of a card after hiding and user ordering.
pub fn visible_sections<'a>(g: &'a SensorGroup, s: &Settings) -> Vec<VisSection<'a>> {
    let default_ids: Vec<String> = g.sections.iter().map(|x| x.id.clone()).collect();
    let order = layout::apply_order(&default_ids, s.layout.section_order.get(&g.id));
    let mut out = Vec::new();
    for sid in order {
        let Some(sec) = g.sections.iter().find(|x| x.id == sid) else { continue };
        if s.sensors.hidden_sections.contains(&sec.id) {
            continue;
        }
        let ids: Vec<String> = sec.readings.iter().filter(|r| !s.is_hidden(&r.id)).map(|r| r.id.clone()).collect();
        if ids.is_empty() {
            continue;
        }
        let rorder = layout::apply_order(&ids, s.layout.reading_order.get(&sec.id));
        let readings = rorder.iter().filter_map(|id| sec.readings.iter().find(|r| &r.id == id)).collect();
        out.push(VisSection { sec, readings });
    }
    out
}

/// The reading a collapsed section shows: the user's choice when it still exists in the
/// section, otherwise the daemon's summary.
pub fn section_summary<'a>(sec: &'a Section, overrides: &BTreeMap<String, String>) -> Option<&'a Reading> {
    if let Some(id) = overrides.get(&sec.id) {
        if let Some(r) = sec.readings.iter().find(|r| &r.id == id) {
            return Some(r);
        }
        if let Some(s) = sec.summary.as_ref().filter(|s| &s.id == id) {
            return Some(s);
        }
    }
    sec.summary.as_ref()
}

/// Is the reading a section shows when collapsed (and its chip on a collapsed card) watched?
/// Highlights are keyed by reading id and the summary can be the daemon's own `summary`, which is
/// not one of the section's readings, so this compares ids instead of looking a row up.
pub fn summary_watched(sec: &Section, s: &Settings) -> bool {
    section_summary(sec, &s.sensors.summary_overrides).is_some_and(|r| s.is_highlighted(&r.id))
}

pub fn section_collapsed(s: &Settings, sec: &Section) -> bool {
    s.layout.collapsed_sections.get(&sec.id).copied().unwrap_or(sec.collapsed_by_default)
}

/// Row weight of a card with sections in their default state, so the automatic layout does not
/// jump when the user expands a section.
pub fn card_weight(g: &SensorGroup, s: &Settings) -> usize {
    if s.layout.collapsed_cards.contains(&g.id) {
        return 3;
    }
    2 + visible_sections(g, s)
        .iter()
        .map(|v| 1 + if v.sec.collapsed_by_default { 0 } else { v.readings.len() })
        .sum::<usize>()
}

/// Cards in display order with their weights (hidden cards removed).
pub fn visible_cards(snap: &Snapshot, s: &Settings) -> Vec<(String, usize)> {
    snap.sensors
        .iter()
        .filter(|g| !s.sensors.hidden_cards.contains(&g.id))
        .map(|g| (g.id.clone(), card_weight(g, s)))
        .collect()
}

// ---------------------------------------------------------------------------
// Keyboard: one Tab stop per card, arrows inside it
// ---------------------------------------------------------------------------

/// A stop inside a card besides its header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavKey {
    Section(String),
    Reading(String),
}

/// Where each card's one Tab stop sits: a card without an entry has it on its header. The stop
/// moves with the arrow keys (a roving focus), so Tab visits one place per card instead of every row.
pub type NavCursor = std::collections::HashMap<String, NavKey>;

/// The stops inside a card in reading order: every visible section head and, while the section
/// is open, its rows. Nothing while the card is collapsed.
pub fn nav_items(g: &SensorGroup, s: &Settings) -> Vec<NavKey> {
    if s.layout.collapsed_cards.contains(&g.id) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for vs in visible_sections(g, s) {
        out.push(NavKey::Section(vs.sec.id.clone()));
        if !section_collapsed(s, vs.sec) {
            out.extend(vs.readings.iter().map(|r| NavKey::Reading(r.id.clone())));
        }
    }
    out
}

/// The stop `delta` places on from `cur` (`None` is the header, which comes first), kept within
/// the card; `None` back to the header.
pub fn nav_step(items: &[NavKey], cur: Option<&NavKey>, delta: isize) -> Option<NavKey> {
    let at = cur.and_then(|k| items.iter().position(|i| i == k)).map_or(0, |i| i + 1) as isize;
    let to = (at + delta).clamp(0, items.len() as isize) as usize;
    (to > 0).then(|| items[to - 1].clone())
}

/// The id of a card's header (`None`) or of one of its rows.
pub fn nav_id(card: &str, key: Option<&NavKey>) -> egui::Id {
    match key {
        None => egui::Id::new(("nav-head", card)),
        Some(NavKey::Section(sec)) => egui::Id::new(("nav-sec", card, sec.as_str())),
        Some(NavKey::Reading(r)) => egui::Id::new(("nav-row", card, r.as_str())),
    }
}

/// Is the card's Tab stop on this section head (`sec`) or reading? Compares without building a key.
fn cursor_on(v: &SensorView, card: &str, sec: bool, id: &str) -> bool {
    match v.nav.get(card) {
        Some(NavKey::Section(s)) => sec && s == id,
        Some(NavKey::Reading(r)) => !sec && r == id,
        None => false,
    }
}

/// Focusable while it is the card's stop; a plain click target otherwise, so Tab skips it.
fn stop_sense(here: bool) -> Sense {
    if here { Sense::click() } else { Sense::CLICK }
}

pub fn set_card_collapsed(s: &mut Settings, id: &str, collapsed: bool) {
    crate::settings::toggle_in(&mut s.layout.collapsed_cards, id, collapsed);
}

/// The section a reading is in (for Left on a row).
pub fn section_of(g: &SensorGroup, reading: &str) -> Option<String> {
    g.sections.iter().find(|x| x.readings.iter().any(|r| r.id == reading)).map(|x| x.id.clone())
}

/// Watched (highlighted) readings in display order.
pub fn highlighted<'a>(snap: &'a Snapshot, s: &Settings) -> Vec<&'a Reading> {
    let mut out = Vec::new();
    for g in snap.sensors.iter().filter(|g| !s.sensors.hidden_cards.contains(&g.id)) {
        for v in visible_sections(g, s) {
            for r in v.readings {
                if s.is_highlighted(&r.id) {
                    out.push(r);
                }
            }
        }
    }
    out
}

/// Make sure the reading `id` is on screen: expand its card and section.
pub fn reveal(s: &mut Settings, snap: &Snapshot, id: &str) -> bool {
    for g in &snap.sensors {
        for sec in &g.sections {
            let is_summary = sec.summary.as_ref().is_some_and(|x| x.id == id);
            if is_summary || sec.readings.iter().any(|r| r.id == id) {
                s.layout.collapsed_cards.retain(|c| c != &g.id);
                s.hidden_remove(id, &sec.id, &g.id);
                // A reading is only visible with its section expanded; a summary shows collapsed.
                if !is_summary || sec.readings.iter().any(|r| r.id == id) {
                    s.layout.collapsed_sections.insert(sec.id.clone(), false);
                }
                return true;
            }
        }
    }
    false
}

/// Compact section name for the chips of a collapsed card.
pub fn short_title(title: &str) -> String {
    let first = title.split(" and ").next().unwrap_or(title);
    first.replace("Temperatures", "Temps").replace("Temperature", "Temp")
}

/// Flags column text and whether it is the critical kind: "H", "HH", "L", "LL".
pub fn flag_marker(level: Level, below: bool) -> Option<(&'static str, Level)> {
    let letter = if below { "L" } else { "H" };
    match level {
        Level::Normal => None,
        Level::Warn => Some((letter, Level::Warn)),
        Level::Crit => Some((if below { "LL" } else { "HH" }, Level::Crit)),
    }
}

// ---------------------------------------------------------------------------
// Row geometry (pure)
// ---------------------------------------------------------------------------

pub const MIN_BAR: f32 = 72.0;
pub const MAX_BAR: f32 = 140.0;
const PAD: f32 = 12.0;
const GAP: f32 = 8.0;

/// Labels longer than this many characters are ellipsized before a bar is given up.
pub const LABEL_CHARS: usize = 22;
/// The last resort in a narrow card: labels are cut to this many characters to keep a bar.
pub const LABEL_FLOOR_CHARS: usize = 14;

/// Widths that depend on the content of one section: results, low and high cells and labels
/// are sized per section, so one wide value does not cost the bars of the other sections.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SecMetrics {
    /// Widest result (number plus unit) in the section.
    pub result_w: f32,
    /// Widest min or max text (numbers only).
    pub mm_w: f32,
    /// Narrowest min or max text.
    pub mm_min: f32,
    pub flag_w: f32,
    /// Width the longest label wants.
    pub label_need: f32,
    /// Width the longest label wants once labels over `LABEL_CHARS` characters are ellipsized.
    pub label_min: f32,
    /// Width of the longest label cut to `LABEL_FLOOR_CHARS`: what a narrow card gives a label
    /// before it gives up the bar.
    pub label_floor: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowCols {
    pub label: Rect,
    /// Right edge of the result cell.
    pub result_right: f32,
    pub min: Option<Rect>,
    pub bar: Option<Rect>,
    pub max: Option<Rect>,
    pub flag: Rect,
}

/// Lay a row out: label, result, min, bar, max, flag. Labels keep the width they need first;
/// what is left goes to the min, bar, max cluster. When it cannot hold a bar of `MIN_BAR`,
/// labels over `LABEL_CHARS` characters give up their tail (ellipsis) before the bar is
/// dropped; only when even that is not enough does the bar go, and then min and max too (they
/// stay in the tooltip). The bar returns as soon as there is room for it. The result column's
/// right edge is the same for every row of a section; `row_mm` is the width of the section's
/// widest min or max text.
pub fn row_cols(rect: Rect, m: &SecMetrics, label_left: f32, row_mm: f32) -> RowCols {
    let right = rect.right() - PAD;
    let flag = Rect::from_min_max(pos2(right - m.flag_w, rect.top()), pos2(right, rect.bottom()));
    let label_x = rect.left() + label_left;
    // flag | gap | cluster | 12 | result | gap | label
    let fixed = m.flag_w + GAP + 12.0 + m.result_w + GAP;
    let room = (right - label_x) - fixed;
    let bar_fits = |label_w: f32| room - label_w - 2.0 * row_mm - 16.0 >= MIN_BAR;
    let minmax_fits = |label_w: f32| room - label_w >= 2.0 * m.mm_min + 16.0;
    // Preference: a bar with whole labels, a bar with ellipsized labels, min and max with whole
    // labels, min and max with ellipsized labels, and last whole labels alone.
    let label_w = if bar_fits(m.label_need) {
        m.label_need
    } else if bar_fits(m.label_min) {
        m.label_min
    } else if bar_fits(m.label_floor) {
        m.label_floor
    } else if minmax_fits(m.label_need) {
        m.label_need
    } else if minmax_fits(m.label_min) {
        m.label_min
    } else {
        m.label_need
    };
    let avail = room - label_w;
    let show_minmax = avail >= 2.0 * m.mm_min + 16.0;
    let cluster_w = if show_minmax { avail.min(2.0 * m.mm_w + 16.0 + MAX_BAR) } else { 0.0 };
    let cluster_right = flag.left() - GAP;
    let cluster_left = cluster_right - cluster_w;
    let result_right = if show_minmax { cluster_left - 12.0 } else { flag.left() - GAP };

    let (mut min_r, mut bar_r, mut max_r) = (None, None, None);
    if cluster_w > 0.0 && 2.0 * row_mm + 16.0 <= cluster_w {
        let cell = |right: f32, w: f32| Rect::from_min_max(pos2(right - w, rect.top()), pos2(right, rect.bottom()));
        let max = cell(cluster_right, row_mm);
        let bar_w = (cluster_w - 2.0 * row_mm - 16.0).min(MAX_BAR);
        let mut x = max.left() - GAP;
        if bar_w >= MIN_BAR {
            bar_r = Some(Rect::from_min_max(pos2(x - bar_w, rect.top()), pos2(x, rect.bottom())));
            x -= bar_w + GAP;
        }
        min_r = Some(cell(x, row_mm));
        max_r = Some(max);
    }
    let label = Rect::from_min_max(pos2(label_x, rect.top()), pos2((result_right - m.result_w - GAP).max(label_x + 20.0), rect.bottom()));
    RowCols { label, result_right, min: min_r, bar: bar_r, max: max_r, flag }
}

/// A label as the row prints it when space is short: over `LABEL_CHARS` characters it keeps
/// the first `LABEL_CHARS - 1` and an ellipsis.
pub fn label_capped(label: &str) -> String {
    label_cut(label, LABEL_CHARS)
}

/// `label` cut to `max` characters: the first `max - 1` and an ellipsis when it is longer.
pub fn label_cut(label: &str, max: usize) -> String {
    if label.chars().count() > max {
        let head: String = label.chars().take(max - 1).collect();
        format!("{head}\u{2026}")
    } else {
        label.to_string()
    }
}

/// The column captions ("now", "low", "range", "high") and where they sit for these row
/// columns: the x of their anchor and how the text hangs on it. Captions of columns the row
/// does not have are left out.
pub fn caption_marks(c: &RowCols) -> Vec<(&'static str, f32, Align2)> {
    let mut out = vec![("now", c.result_right, Align2::RIGHT_CENTER)];
    if let Some(r) = c.min {
        out.push(("low", r.right(), Align2::RIGHT_CENTER));
    }
    if let Some(r) = c.bar {
        out.push(("range", r.center().x, Align2::CENTER_CENTER));
    }
    if let Some(r) = c.max {
        out.push(("high", r.right(), Align2::RIGHT_CENTER));
    }
    out
}

/// Where a card's header band (header plus caption row, `head_h` tall) is pinned while the card scrolls under the top of the view: `None`
/// while its real header is on screen or the whole card is gone; otherwise the top of the
/// pinned header, which slides up and out with the card's bottom edge.
pub fn sticky_header_y(viewport_top: f32, head_top: f32, card_bottom: f32, head_h: f32) -> Option<f32> {
    if head_top >= viewport_top - 0.5 || card_bottom <= viewport_top {
        return None;
    }
    Some(viewport_top.min(card_bottom - head_h))
}

// ---------------------------------------------------------------------------
// Device state for the legend chips (pure)
// ---------------------------------------------------------------------------

/// Mark of a device that is throttling right now (a thermometer glyph).
pub const THROTTLE_MARK: &str = "\u{f050f}";

/// The worst current state among the visible readings of a device.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceState {
    pub level: Level,
    /// "H", "HH", "L", "LL" or `THROTTLE_MARK`.
    pub mark: Option<&'static str>,
    /// The first reading in display order with that state, to jump to.
    pub target: Option<String>,
    /// Its label, for the chip's tooltip.
    pub label: String,
}

impl DeviceState {
    pub fn normal() -> DeviceState {
        DeviceState { level: Level::Normal, mark: None, target: None, label: String::new() }
    }

    /// What the mark says, for a tooltip.
    pub fn meaning(&self) -> &'static str {
        match self.mark {
            Some("H") => "is above its warning level",
            Some("HH") => "is above its critical level",
            Some("L") => "is below its warning level",
            Some("LL") => "is below its critical level",
            Some(_) => "is active",
            None => "",
        }
    }
}

/// Worst state of the readings of `dev` that are on the panels: a critical crossing (HH or
/// LL) before throttling now before a warning (H or L); ties go to the first in display order.
pub fn device_state(snap: &Snapshot, s: &Settings, dev: Device) -> DeviceState {
    let mut best: Option<(u8, DeviceState)> = None;
    for g in snap.sensors.iter().filter(|g| Device::of_card(&g.id) == dev && !s.sensors.hidden_cards.contains(&g.id)) {
        for vs in visible_sections(g, s) {
            for r in vs.readings {
                let Some(v) = r.value else { continue };
                let (rank, level, mark) = if r.unit == Unit::Flag {
                    if v < 0.5 {
                        continue;
                    }
                    (3, Level::Crit, THROTTLE_MARK)
                } else {
                    let thr = effective(snap, r);
                    match flag_marker(level_of(v, &thr), thr.below) {
                        Some((m, Level::Crit)) => (4, Level::Crit, m),
                        Some((m, l)) => (2, l, m),
                        None => continue,
                    }
                };
                if best.as_ref().is_none_or(|(b, _)| rank > *b) {
                    best = Some((rank, DeviceState { level, mark: Some(mark), target: Some(r.id.clone()), label: r.label.clone() }));
                }
            }
        }
    }
    best.map(|(_, st)| st).unwrap_or_else(DeviceState::normal)
}

// ---------------------------------------------------------------------------
// Sources that fail
// ---------------------------------------------------------------------------

/// What a failing data source takes from the card of `card_id`, in the words of the card's
/// note: "GPU readings unavailable: NVML ...". Empty for a card no failing source feeds.
pub fn source_notes(snap: &Snapshot, card_id: &str) -> Vec<String> {
    let dev = Device::of_card(card_id);
    let mut out = Vec::new();
    for s in snap.sources.iter().filter(|s| !s.ok) {
        let why = match s.detail.trim() {
            "" => "no answer",
            d => d,
        };
        match (s.name.as_str(), dev) {
            // NVML and LACT feed the NVIDIA card (gpu0); the AMD and Intel cards do not use them.
            ("nvml", Device::Gpu) if card_id == "gpu0" => out.push(format!("GPU readings unavailable: {why}")),
            // LACT's own detail already says what is missing.
            ("lact", Device::Gpu) if card_id == "gpu0" => out.push(why.to_string()),
            ("helper", Device::Cpu) => out.push(format!("Package power unavailable: {why}")),
            ("helper", Device::Memory) => out.push(format!("Memory speed unavailable: {why}")),
            _ => {}
        }
    }
    out
}

/// The snapshot with an empty GPU card added when NVML fails and the collector therefore sent no
/// GPU card at all, so the failure has a card to say it in. `None` when nothing needs adding.
pub fn with_placeholder_cards(snap: &Snapshot) -> Option<Snapshot> {
    let nvml_down = snap.sources.iter().any(|s| s.name == "nvml" && !s.ok);
    if !nvml_down || snap.sensors.iter().any(|g| g.id == "gpu0") {
        return None;
    }
    let mut out = snap.clone();
    let at = out.sensors.iter().position(|g| Device::of_card(&g.id) == Device::Cpu).map_or(0, |i| i + 1);
    out.sensors.insert(at, SensorGroup { id: "gpu0".into(), title: "GPU".into(), subtitle: Some("unavailable".into()), sections: Vec::new() });
    Some(out)
}

/// One note inside a card: an amber dot and the words, wrapped to the card.
fn source_note(ui: &mut Ui, pal: &Palette, text: &str, w: f32) {
    let g = ui.painter().layout(text.to_string(), f_small(), pal.ink2, (w - 36.0).max(80.0));
    let (rect, resp) = ui.allocate_exact_size(vec2(w, g.size().y + 10.0), Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    if ui.is_rect_visible(rect) {
        ui.painter().circle_filled(pos2(rect.left() + 16.0, rect.top() + 5.0 + g.size().y.min(16.0) / 2.0), 4.0, pal.warn);
        ui.painter().galley(pos2(rect.left() + 28.0, rect.top() + 5.0), g, pal.ink2);
    }
}

// ---------------------------------------------------------------------------
// Hits for the history popover
// ---------------------------------------------------------------------------

/// A reading row (or a collapsed section's summary row) the pointer touched.
#[derive(Clone, Debug)]
pub struct Hit {
    pub id: String,
    /// "CPU, Tctl"
    pub title: String,
    pub unit: Unit,
    pub value: Option<f64>,
    pub thr: Thr,
    pub color: Color32,
    pub rect: Rect,
    /// What the row's tooltip says, folded into the popover so the two never stack: the
    /// reading's description, its source and its range since the stats began.
    pub desc: Option<String>,
    pub note: Option<String>,
    pub range: Option<String>,
}

#[derive(Default)]
pub struct ViewOut {
    pub stop_editing: bool,
    /// The row under the pointer.
    pub hover: Option<Hit>,
    /// A row that was clicked (pins the popover).
    pub clicked: Option<Hit>,
    /// "Show history" from a context menu.
    pub show_history: Option<Hit>,
    /// Where the currently pinned reading's row is, when visible.
    pub pinned: Option<Hit>,
    /// Rects of all cards by id, for the legend.
    pub card_rects: Vec<(String, Rect)>,
    /// Cards whose header is pinned at the top of the view right now.
    pub sticky: Vec<String>,
    /// The edit bar's Reset layout (or its Undo) was clicked.
    pub reset_layout: bool,
    /// Each card's one Tab stop this frame: its widget id, the card and where it sits.
    pub stops: Vec<(egui::Id, String, Option<NavKey>)>,
    /// Limit changes made in a reading's menu, for the app to send to the collector.
    pub thr_edits: Vec<ThrEdit>,
}

// ---------------------------------------------------------------------------
// Drag and drop payloads and per-frame records
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Dnd {
    Card(String),
    Section { card: String, id: String },
    Reading { section: String, id: String },
}

#[derive(Default)]
struct FrameRec {
    cols: Vec<Rect>,
    /// (column, card id, rect)
    cards: Vec<(usize, String, Rect)>,
    /// (card id, section id, rect)
    sections: Vec<(String, String, Rect)>,
    /// (section id, reading id, rect)
    rows: Vec<(String, String, Rect)>,
    /// The cards of each column as laid out now (edit mode), for the card menu's move entries.
    columns: Vec<Vec<String>>,
    /// A move picked in a card menu: (card id, column, position in it), applied after the frame.
    pending_move: Option<(String, usize, usize)>,
    /// The visible sections of the card being drawn and the rows of the section being drawn, in
    /// order (edit mode), for the Move up and Move down entries of their menus.
    card_sections: Vec<String>,
    section_rows: Vec<String>,
    /// A move picked in a section or row menu, applied after the frame.
    pending_order: Option<PendingOrder>,
}

/// A section moved within its card, or a row within its section, by a menu entry.
struct PendingOrder {
    /// Sections (true) or rows (false).
    sections: bool,
    /// The card (for sections) or section (for rows) whose order it is.
    key: String,
    current: Vec<String>,
    id: String,
    index: usize,
}

/// "Move up" and "Move down" for an item of an ordered list, in edit mode: the keyboard and
/// pointer path to what the grips do by dragging.
fn reorder_entries(ui: &mut Ui, sections: bool, key: &str, id: &str, current: &[String], pick: &mut Option<PendingOrder>) {
    let Some(at) = current.iter().position(|x| x == id) else { return };
    if current.len() < 2 {
        return;
    }
    ui.separator();
    let mut to = |index: usize| *pick = Some(PendingOrder { sections, key: key.to_string(), current: current.to_vec(), id: id.to_string(), index });
    if at > 0 && ui.button("Move up").clicked() {
        to(at - 1);
        ui.close();
    }
    if at + 1 < current.len() && ui.button("Move down").clicked() {
        to(at + 1);
        ui.close();
    }
}

pub struct SensorView<'a> {
    pub snap: &'a Snapshot,
    pub pal: &'a Palette,
    pub s: &'a mut Settings,
    pub edit: bool,
    pub scroll_to: &'a mut Option<String>,
    pub scroll_to_card: &'a mut Option<String>,
    /// Cards of this device get an outline (the legend chip is hovered).
    pub hover_device: Option<Device>,
    /// Text the app should copy to the clipboard.
    pub copy: &'a mut Option<String>,
    /// Reading id and seconds since the focus flash started.
    pub flash: Option<(&'a str, f32)>,
    /// Reading whose history popover is pinned.
    pub pinned: Option<&'a str>,
    /// Hovering a row opens the history popover, which carries what the tooltip would say: no
    /// tooltip then (edit mode, where the popover is off, keeps it).
    pub popover: bool,
    /// The edit bar's Reset layout reads Undo while this is on offer.
    pub layout_undo: Option<UndoOffer<'a>>,
    /// Where each card's Tab stop is.
    pub nav: &'a mut NavCursor,
    /// Limit changes sent that the snapshots may not show yet (the editor keeps their values).
    pub pending: &'a Pending,
    pub out: &'a mut ViewOut,
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

pub fn row_h() -> f32 {
    crate::theme::hit_h(28.0)
}
fn card_head_h() -> f32 {
    crate::theme::hit_h(36.0)
}
/// The caption row under a card header.
fn caption_h() -> f32 {
    snap4(20.0 * crate::theme::font_scale())
}
/// The fixed gap above a section head: half a row, on the 4 px grid.
pub fn sec_gap() -> f32 {
    (row_h() / 2.0 / 4.0).floor() * 4.0
}

/// Width of a row's own min and max text.
fn row_mm_w(p: &egui::Painter, r: &Reading, fallback: f32) -> f32 {
    match &r.stats {
        Some(st) if r.unit != Unit::Flag => {
            let u = RowUnit::of(r);
            [st.min, st.max].iter().map(|v| text_w(p, &u.number(*v), f_minmax())).fold(0.0, f32::max).ceil()
        }
        _ => fallback,
    }
}

fn text_w(p: &egui::Painter, text: &str, font: FontId) -> f32 {
    p.layout_no_wrap(text.to_string(), font, Color32::WHITE).size().x
}

/// Strength of the wash under a watched row.
pub const WATCH_WASH: f32 = 0.08;
/// Strength of the green flash at its first frame (ink reads on it at 4.5:1).
pub const FLASH_PEAK: f32 = 0.45;

/// Room a watched chip on a collapsed card gives its dot (the dot and the air after it).
const CHIP_DOT_W: f32 = 11.0;
/// Extra room a watched collapsed section row leaves between its title and the summary label,
/// for the watch dot (the label cell already holds this much slack, see `collapsed_label_cell`).
const COLLAPSED_DOT_W: f32 = 6.0;

/// Where the collapsed row's title starts, from the left edge of its label cell.
const COLLAPSED_TITLE_INSET: f32 = 8.0;
/// Gap between the title and the summary label that follows it.
const COLLAPSED_LABEL_GAP: f32 = 10.0;
/// The least room a summary label gets before it is left out (the painter's own threshold).
pub const COLLAPSED_LABEL_MIN: f32 = 56.0;

/// The label cell of a collapsed section row, which holds its title and the summary's label:
/// (whole label, label cut to `LABEL_CHARS`, label cut to `LABEL_FLOOR_CHARS`). `summary` is
/// the label's width whole and cut that way. `row_cols` fills the bar before it gives a tier
/// up, so the bar shrinks to its minimum before the label is cut.
pub fn collapsed_label_cell(title_w: f32, summary: [f32; 3]) -> (f32, f32, f32) {
    let around = COLLAPSED_TITLE_INSET + title_w + COLLAPSED_LABEL_GAP + 6.0;
    let cell = |label_w: f32| around + label_w.max(if label_w > 0.0 { COLLAPSED_LABEL_MIN + 1.0 } else { 0.0 });
    (cell(summary[0]), cell(summary[1]), cell(summary[2]))
}

/// Metrics of one section, from its rows and its collapsed summary. `collapsed` makes room
/// for the section title, which then shares the row with the summary.
fn measure_section(p: &egui::Painter, vs: &VisSection, summary: Option<&Reading>, inner_w: f32, collapsed: bool) -> SecMetrics {
    let (mut num_w, mut unit_w, mut mm_w, mut mm_min, mut label_w, mut label_min) = (0f32, 0f32, 0f32, f32::MAX, 0f32, 0f32);
    let mut label_floor = 0f32;
    let flags_w = text_w(p, "HH", f_flag());
    for r in vs.readings.iter().copied().chain(summary) {
        let u = RowUnit::of(r);
        if let Some(v) = r.value {
            let (n, unit) = u.split(v);
            num_w = num_w.max(text_w(p, &n, f_result()));
            if !unit.is_empty() {
                unit_w = unit_w.max(text_w(p, unit, f_unit()));
            }
        }
        if let Some(st) = &r.stats {
            if r.unit != Unit::Flag {
                let w = [st.min, st.max].iter().map(|v| text_w(p, &u.number(*v), f_minmax())).fold(0.0, f32::max);
                mm_w = mm_w.max(w);
                mm_min = mm_min.min(w);
            }
        }
    }
    for r in &vs.readings {
        let w = text_w(p, &r.label, f_label());
        label_w = label_w.max(w);
        label_min = label_min.max(if r.label.chars().count() > LABEL_CHARS { text_w(p, &label_capped(&r.label), f_label()) } else { w });
        label_floor = label_floor.max(if r.label.chars().count() > LABEL_FLOOR_CHARS { text_w(p, &label_cut(&r.label, LABEL_FLOOR_CHARS), f_label()) } else { w });
    }
    let cap = inner_w * 0.42;
    let result_w = num_w + if unit_w > 0.0 { 4.0 + unit_w } else { 0.0 };
    let mut m = SecMetrics {
        result_w: result_w.ceil().max(40.0),
        mm_w: mm_w.ceil().max(24.0),
        mm_min: if mm_min == f32::MAX { 24.0 } else { mm_min.ceil().max(24.0) },
        flag_w: flags_w.ceil() + 2.0,
        label_need: label_w.ceil().min(cap) + 6.0,
        label_min: label_min.ceil().min(cap) + 6.0,
        label_floor: label_floor.ceil().min(cap) + 6.0,
    };
    m.label_min = m.label_min.min(m.label_need);
    m.label_floor = m.label_floor.min(m.label_min);
    if collapsed {
        // The title is not shortened for a bar, and neither is the summary's label until the
        // bar is down to its minimum: the cell holds both, tier by tier like the rows' labels.
        let title = text_w(p, &vs.sec.title, f_section());
        let (w, w22, w14) = match summary {
            Some(r) => {
                let whole = text_w(p, &r.label, f_label());
                let cut = |n: usize| if r.label.chars().count() > n { text_w(p, &label_cut(&r.label, n), f_label()) } else { whole };
                (whole, cut(LABEL_CHARS), cut(LABEL_FLOOR_CHARS))
            }
            None => (0.0, 0.0, 0.0),
        };
        let max = inner_w * 0.6;
        let (need, min, floor) = collapsed_label_cell(title, [w.ceil(), w22.ceil(), w14.ceil()]);
        m.label_need = need.min(max);
        m.label_min = min.min(m.label_need);
        m.label_floor = floor.min(m.label_min);
    }
    m
}

/// Metrics of a section and the width of its own min and max text, shared by all its rows so
/// their bars line up.
fn section_layout(p: &egui::Painter, vs: &VisSection, s: &Settings, inner_w: f32) -> (SecMetrics, f32) {
    let collapsed = section_collapsed(s, vs.sec);
    let summary = section_summary(vs.sec, &s.sensors.summary_overrides);
    let m = measure_section(p, vs, summary, inner_w, collapsed);
    let mm = if collapsed {
        summary.map(|r| row_mm_w(p, r, m.mm_min)).unwrap_or(m.mm_min)
    } else {
        vs.readings.iter().map(|r| row_mm_w(p, r, 0.0)).fold(m.mm_min, f32::max)
    };
    (m, mm)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Draw all cards. `avail` is the width of the area (scrollbars excluded).
pub fn show(ui: &mut Ui, v: &mut SensorView, avail: f32) {
    let ctx = ui.ctx().clone();
    let cards = visible_cards(v.snap, v.s);
    let scale = crate::theme::font_scale();
    let arr = layout::arrangement(&v.s.layout, &cards, avail, scale, v.edit);
    let mut fr = FrameRec::default();
    if v.edit {
        fr.columns = arr.iter().map(|c| c.cards.clone()).collect();
    }

    if v.edit {
        // How many columns the same layout has when it is not being edited.
        let shown = layout::arrangement(&v.s.layout, &cards, avail, scale, false).len();
        edit_bar(ui, v, &arr, shown);
        ui.add_space(8.0);
    }

    let mut resize: Option<(usize, f32)> = None;
    let mut remove: Option<usize> = None;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = COL_GAP;
        for (ci, col) in arr.iter().enumerate() {
            let inner = ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 12.0;
                ui.set_width(col.width);
                if v.edit {
                    ui.set_min_height(160.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Column {}", ci + 1)).color(v.pal.ink3).font(f_small()));
                        if arr.len() > 1 && ui.button("Remove column").clicked() {
                            remove = Some(ci);
                        }
                    });
                }
                for id in &col.cards {
                    if let Some(g) = v.snap.sensors.iter().find(|g| &g.id == id) {
                        card(ui, v, &mut fr, g, col.width, ci);
                    }
                }
            });
            let rect = inner.response.rect;
            // keep the full column width even when the content is narrower
            let rect = Rect::from_min_max(rect.min, pos2(rect.min.x + col.width, rect.max.y));
            fr.cols.push(rect);
            if v.edit {
                // the line stays thin; the hit area is 24 px wide, centred on the gap
                let handle = Rect::from_min_max(pos2(rect.right() - 2.0, rect.top()), pos2(rect.right() + COL_GAP - 2.0, rect.bottom()));
                let hit = Rect::from_center_size(handle.center(), vec2(handle.width().max(crate::theme::MIN_HIT), handle.height()));
                let resp = ui.interact(hit, egui::Id::new(("col-resize", ci)), Sense::drag());
                a11y::publish(&resp, || {
                    Spoken::new(egui::accesskit::Role::Splitter, format!("Resize column {}", ci + 1))
                        .described("Drag, or press Left and Right to change the width by 4 pixels, with Shift by 32. Home restores the default width.")
                        .valued(format!("{} pixels wide", col.width.round()))
                });
                let key = ui_kit::divider_key(&resp, true);
                let active = resp.hovered() || resp.dragged() || resp.has_focus();
                if active {
                    ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                let color = if active { v.pal.ink2 } else { v.pal.rule };
                let line = Rect::from_center_size(handle.center(), vec2(3.0, rect.height().min(900.0)));
                ui.painter().rect_filled(line, 1.5, color);
                if resp.has_focus() {
                    crate::a11y::paint_ring(ui.painter(), line.expand2(vec2(3.0, 0.0)), 3.0, crate::a11y::Ring::Outside, v.pal.ink);
                }
                if resp.dragged() {
                    resize = Some((ci, col.width + ui_kit::drag_delta(ui, &resp).x));
                }
                match key {
                    Some(ui_kit::DividerKey::Move(d)) => resize = Some((ci, col.width + d)),
                    Some(ui_kit::DividerKey::Reset) => resize = Some((ci, layout::NEW_COL_W)),
                    None => {}
                }
            }
        }
    });
    ui.add_space(12.0);

    if let Some(target) = v.scroll_to_card.take() {
        if let Some((_, r)) = v.out.card_rects.iter().find(|(id, _)| *id == target) {
            ui.scroll_to_rect(*r, Some(Align::Min));
        }
    }

    if v.edit {
        if let Some(ci) = remove {
            layout::freeze(&mut v.s.layout, &arr);
            layout::remove_column(&mut v.s.layout, ci);
        }
        if let Some((ci, w)) = resize {
            layout::freeze(&mut v.s.layout, &arr);
            // A layout that was scaled down to fit the window keeps what is on screen.
            for (i, c) in arr.iter().enumerate() {
                if let Some(p) = v.s.layout.columns.get_mut(i) {
                    p.width = c.width;
                }
            }
            layout::resize_column(&mut v.s.layout, ci, w);
        }
        if let Some((card, col, at)) = fr.pending_move.take() {
            layout::freeze(&mut v.s.layout, &arr);
            layout::sync_shown(&mut v.s.layout, &arr);
            layout::move_card(&mut v.s.layout, &card, col, at);
        }
        if let Some(po) = fr.pending_order.take() {
            layout::freeze(&mut v.s.layout, &arr);
            let map = if po.sections { &mut v.s.layout.section_order } else { &mut v.s.layout.reading_order };
            layout::move_in_order(map, &po.key, &po.current, &po.id, po.index);
        }
        handle_drop(ui, v, &fr, &arr);
    }
}

/// `shown` is how many columns the layout has outside editing, fewer than `arr` when they stack.
fn edit_bar(ui: &mut Ui, v: &mut SensorView, arr: &[Column], shown: usize) {
    let pal = v.pal;
    let used = arr.iter().filter(|c| !c.cards.is_empty()).count();
    egui::Frame::new()
        .fill(pal.header)
        .stroke(Stroke::new(1.0, pal.rule))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            // the bar spans the window like the columns under it, not just its own text
            ui.set_min_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Editing layout.").font(f_label_bold()).color(pal.ink));
                ui.label(RichText::new("Drag the grips to move panels, sections and rows. The menus (right-click, or Shift+F10 on a focused panel, section or row) move them without dragging. Drag a column's right edge to resize it, or focus it and use the arrow keys.").color(pal.ink2));
                if ui.button("Add column").clicked() {
                    layout::freeze(&mut v.s.layout, arr);
                    layout::add_column(&mut v.s.layout);
                }
                let c = ResetButton {
                    enabled: true,
                    label: "Reset layout",
                    tip: "Back to the automatic arrangement: clears your columns and the order of panels, sections and rows.",
                    disabled_tip: "",
                    undo: v.layout_undo.take(),
                };
                if ui_kit::reset_button(ui, pal, &c) {
                    v.out.reset_layout = true;
                }
                if ui.button("Done").clicked() {
                    v.out.stop_editing = true;
                }
            });
            if !v.s.layout.auto && shown < used {
                let place = if v.s.layout.dock() == crate::settings::Dock::Right { "the space beside the process panel is" } else { "the window is" };
                ui.label(
                    RichText::new(format!("Your {used} columns are stacked into {shown} outside editing because {place} narrow. Editing shows all {used}."))
                        .color(pal.ink2),
                );
            }
        });
}

/// While a payload is dragged: find the target under the pointer, draw an insertion line and
/// apply the move when the pointer is released.
fn handle_drop(ui: &mut Ui, v: &mut SensorView, fr: &FrameRec, arr: &[Column]) {
    let ctx = ui.ctx().clone();
    let Some(payload) = DragAndDrop::payload::<Dnd>(&ctx) else { return };
    let Some(pos) = ctx.pointer_latest_pos() else { return };
    let released = ctx.input(|i| i.pointer.any_released());
    let accent = v.pal.ink;
    let line = |ui: &Ui, y: f32, x0: f32, x1: f32| {
        ui.painter().rect_filled(Rect::from_min_max(pos2(x0, y - 1.5), pos2(x1, y + 1.5)), 1.5, accent);
    };

    match &*payload {
        Dnd::Card(card) => {
            let Some((ci, crect)) = fr
                .cols
                .iter()
                .enumerate()
                .min_by(|a, b| dist_x(a.1, pos.x).total_cmp(&dist_x(b.1, pos.x)))
            else {
                return;
            };
            let in_col: Vec<&(usize, String, Rect)> = fr.cards.iter().filter(|c| c.0 == ci).collect();
            let spans: Vec<(f32, f32)> = in_col.iter().map(|c| (c.2.top(), c.2.bottom())).collect();
            let slot = layout::slot_for_y(&spans, pos.y);
            let y = if slot < in_col.len() {
                in_col[slot].2.top() - 6.0
            } else if let Some(last) = in_col.last() {
                last.2.bottom() + 6.0
            } else {
                crect.top() + 28.0
            };
            line(ui, y, crect.left(), crect.right());
            if released {
                let from = in_col.iter().position(|c| &c.1 == card);
                let idx = layout::final_index(from, slot);
                layout::freeze(&mut v.s.layout, arr);
                layout::sync_shown(&mut v.s.layout, arr);
                layout::move_card(&mut v.s.layout, card, ci, idx);
                DragAndDrop::clear_payload(&ctx);
            }
        }
        Dnd::Section { card, id } => {
            let items: Vec<&(String, String, Rect)> = fr.sections.iter().filter(|s| &s.0 == card).collect();
            let Some(first) = items.first() else { return };
            let spans: Vec<(f32, f32)> = items.iter().map(|s| (s.2.top(), s.2.bottom())).collect();
            let slot = layout::slot_for_y(&spans, pos.y);
            let y = if slot < items.len() { items[slot].2.top() } else { items.last().unwrap().2.bottom() };
            line(ui, y, first.2.left(), first.2.right());
            if released {
                let current: Vec<String> = items.iter().map(|s| s.1.clone()).collect();
                let from = current.iter().position(|x| x == id);
                layout::freeze(&mut v.s.layout, arr);
                layout::move_in_order(&mut v.s.layout.section_order, card, &current, id, layout::final_index(from, slot));
                DragAndDrop::clear_payload(&ctx);
            }
        }
        Dnd::Reading { section, id } => {
            let items: Vec<&(String, String, Rect)> = fr.rows.iter().filter(|r| &r.0 == section).collect();
            let Some(first) = items.first() else { return };
            let spans: Vec<(f32, f32)> = items.iter().map(|s| (s.2.top(), s.2.bottom())).collect();
            let slot = layout::slot_for_y(&spans, pos.y);
            let y = if slot < items.len() { items[slot].2.top() } else { items.last().unwrap().2.bottom() };
            line(ui, y, first.2.left(), first.2.right());
            if released {
                let current: Vec<String> = items.iter().map(|s| s.1.clone()).collect();
                let from = current.iter().position(|x| x == id);
                layout::freeze(&mut v.s.layout, arr);
                layout::move_in_order(&mut v.s.layout.reading_order, section, &current, id, layout::final_index(from, slot));
                DragAndDrop::clear_payload(&ctx);
            }
        }
    }
}

fn dist_x(r: &Rect, x: f32) -> f32 {
    if x < r.left() {
        r.left() - x
    } else if x > r.right() {
        x - r.right()
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Painting helpers
// ---------------------------------------------------------------------------

/// Width of a grip's hit area: the 24 px floor of a target.
const GRIP_W: f32 = 24.0;

/// A drag handle: the Material drag glyph. It is a pointer target (not a Tab stop) that is named
/// for what it moves.
fn grip(ui: &mut Ui, rect: Rect, id: egui::Id, payload: Dnd, pal: &Palette, what: impl FnOnce() -> String) {
    let resp = ui.interact(rect, id, Sense::DRAG);
    a11y::publish(&resp, || Spoken::button(what()).described("Drag to move it"));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    if resp.drag_started() {
        DragAndDrop::set_payload(ui.ctx(), payload);
    }
    let color = if resp.hovered() || resp.dragged() { pal.ink } else { pal.ink3 };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon::DRAG, ui_kit::f_icon(base() + 1.0), color);
}

fn chevron(p: &egui::Painter, center: egui::Pos2, open: bool, color: Color32) {
    let g = if open { icon::CHEVRON_DOWN } else { icon::CHEVRON_RIGHT };
    p.text(center, Align2::CENTER_CENTER, g, ui_kit::f_icon(base() + 2.0), color);
}

fn level_color(pal: &Palette, l: Level, normal: Color32) -> Color32 {
    pal.level_color(l).unwrap_or(normal)
}

/// Result: the number in bold ink (or `num_color`), the unit small and quiet at the edge.
fn paint_result(p: &egui::Painter, right: f32, cy: f32, r: &Reading, num_color: Color32, pal: &Palette) {
    let Some(v) = r.value else {
        p.text(pos2(right, cy), Align2::RIGHT_CENTER, "n/a", f_result(), pal.ink3);
        return;
    };
    let (num, unit) = RowUnit::of(r).split(v);
    let mut x = right;
    if !unit.is_empty() {
        let rr = p.text(pos2(x, cy), Align2::RIGHT_CENTER, unit, f_unit(), pal.ink3);
        x = rr.left() - 4.0;
    }
    p.text(pos2(x, cy), Align2::RIGHT_CENTER, num, f_result(), num_color);
}

/// What a row's cells need to know besides the reading: when its statistics began, the
/// snapshot's time (throttling is amber only while recent) and its known capacity (the bar's scale).
struct CellCtx {
    since_ms: u64,
    now_ms: u64,
    cap: Option<f64>,
}

impl CellCtx {
    fn of(snap: &Snapshot, r: &Reading) -> CellCtx {
        CellCtx { since_ms: snap.stats_since_unix_ms, now_ms: snap.unix_ms, cap: rangebar::capacity(snap, r) }
    }
}

/// The data cells of a row: result, min, bar, max, flag. The unit is printed once, after the
/// current value; min and max are bare numbers in that unit.
#[allow(clippy::too_many_arguments)]
fn paint_cells(p: &egui::Painter, cols: &RowCols, r: &Reading, thr: &Thr, device: Color32, pal: &Palette, num_color: Option<Color32>, cx: &CellCtx) {
    let cy = cols.flag.center().y;
    if r.unit == Unit::Flag {
        let on = r.value.is_some_and(|v| v >= 0.5);
        let col = if on { pal.crit } else { num_color.unwrap_or(pal.ink) };
        paint_result(p, cols.result_right, cy, r, col, pal);
        let (text, recent) = limits::flag_detail(r, cx.since_ms, cx.now_ms);
        let left = cols.min.map(|m| m.left()).unwrap_or(cols.result_right + 8.0);
        let right = cols.flag.right();
        let color = if recent { pal.warn } else { pal.ink3 };
        let w = (right - left).max(10.0);
        // A narrow card gets the short form ("never", "3x") before any ellipsis.
        let full = p.layout_no_wrap(text.clone(), f_minmax(), color);
        let text = if full.size().x > w { limits::flag_detail_short(r) } else { text };
        let galley = ellipsized(p, &text, f_minmax(), color, w);
        p.galley(pos2(right - galley.size().x, cy - galley.size().y / 2.0), galley, color);
        return;
    }
    let level = r.value.map(|v| level_of(v, thr)).unwrap_or(Level::Normal);
    let ink = num_color.unwrap_or(pal.ink);
    paint_result(p, cols.result_right, cy, r, level_color(pal, level, ink), pal);
    if let Some(st) = &r.stats {
        let unit = RowUnit::of(r);
        for (cell, v) in [(cols.min, st.min), (cols.max, st.max)] {
            if let Some(rect) = cell {
                let c = level_color(pal, level_of(v, thr), pal.ink3);
                p.text(pos2(rect.right(), cy), Align2::RIGHT_CENTER, unit.number(v), f_minmax(), c);
            }
        }
    } else {
        for rect in [cols.min, cols.max].into_iter().flatten() {
            p.text(pos2(rect.right(), cy), Align2::RIGHT_CENTER, "n/a", f_minmax(), pal.ink3);
        }
    }
    if let (Some(bar), Some(g)) = (cols.bar, rangebar::geometry_in(r, thr, cx.cap)) {
        rangebar::paint(p, bar, &g, device, pal);
    }
    if let Some((text, l)) = r.value.and_then(|v| flag_marker(level_of(v, thr), thr.below)).map(|(t, l)| (t, l)) {
        let c = level_color(pal, l, pal.ink3);
        p.text(pos2(cols.flag.right(), cy), Align2::RIGHT_CENTER, text, f_flag(), c);
    }
}

// ---------------------------------------------------------------------------
// Cards
// ---------------------------------------------------------------------------

/// What a card header shows.
struct HeaderInfo<'a> {
    tab: &'static str,
    color: Color32,
    subtitle: Option<&'a str>,
    throttle: Option<(Throttle, &'a str)>,
    collapsed: bool,
    /// Left inset of the chevron (more in edit mode, for the grip).
    left: f32,
}

/// Paint a card header into `rect`. `square` drops the rounded top corners (the pinned copy).
/// Returns whether the subtitle was cut short.
fn paint_header(p: &egui::Painter, pal: &Palette, rect: Rect, hovered: bool, square: bool, h: &HeaderInfo) -> bool {
    let radius = if h.collapsed {
        egui::CornerRadius::same(7)
    } else if square {
        egui::CornerRadius::ZERO
    } else {
        egui::CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 }
    };
    p.rect_filled(rect, radius, if hovered { pal.hover } else { pal.header });
    chevron(p, pos2(rect.left() + h.left + 6.0, rect.center().y), !h.collapsed, pal.ink3);
    // device tab
    let tab = ui_kit::paint_tab(p, rect.left() + h.left + 20.0, rect.center().y, h.tab, h.color, if hovered { pal.hover } else { pal.header });
    // throttling state, right: pills for the states that matter, quiet text for "none"
    let mut right_edge = rect.right() - 12.0;
    if let Some((state, text)) = h.throttle {
        let pill_font = FontId::new(base() - 2.0, crate::fonts::bold());
        match state {
            // Past throttling is quiet text once it is no longer recent (see `limits::THROTTLE_RECENT_MINUTES`).
            Throttle::Never | Throttle::Past { recent: false, .. } => {
                let gal = p.layout_no_wrap(text.to_string(), f_small(), pal.ink3);
                let pos = pos2(right_edge - gal.size().x, rect.center().y - gal.size().y / 2.0);
                right_edge = pos.x - 12.0;
                p.galley(pos, gal, pal.ink3);
            }
            _ => {
                let (fill, stroke, ink) = match state {
                    Throttle::Now => (pal.crit, Stroke::NONE, pal.on_crit),
                    _ => (Color32::TRANSPARENT, Stroke::new(1.0, pal.warn), pal.warn),
                };
                let gal = p.layout_no_wrap(text.to_string(), pill_font, ink);
                let size = gal.size() + vec2(16.0, 6.0);
                let pill = Rect::from_min_size(pos2(right_edge - size.x, rect.center().y - size.y / 2.0), size);
                p.rect(pill, size.y / 2.0, fill, stroke, egui::StrokeKind::Inside);
                p.galley(pill.center() - gal.size() / 2.0, gal, ink);
                right_edge = pill.left() - 12.0;
            }
        }
    }
    let Some(sub) = h.subtitle else { return false };
    let x = tab.right() + 12.0;
    let galley = ellipsized(p, sub, f_label(), pal.ink3, right_edge - x);
    let cut = galley.elided;
    p.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, pal.ink3);
    cut
}

/// Where the card menu can move a card while the layout is edited: the columns as laid out and
/// the slot for the move that was picked. Dragging is not the only way to rearrange the cards.
struct MoveMenu<'a> {
    columns: &'a [Vec<String>],
    pick: &'a mut Option<(String, usize, usize)>,
}

fn card_menu(ui: &mut Ui, s: &mut Settings, id: &str, collapsed: bool, moves: Option<MoveMenu>) {
    if ui.button(if collapsed { "Expand" } else { "Collapse" }).clicked() {
        toggle_card(s, id);
        ui.close();
    }
    if ui.button("Hide panel").clicked() {
        crate::settings::toggle_in(&mut s.sensors.hidden_cards, id, true);
        ui.close();
    }
    let Some(mv) = moves else { return };
    let Some((col, at)) = mv.columns.iter().enumerate().find_map(|(c, cards)| cards.iter().position(|x| x == id).map(|i| (c, i))) else { return };
    let in_col = mv.columns[col].len();
    ui.separator();
    if at > 0 && ui.button("Move up").clicked() {
        *mv.pick = Some((id.to_string(), col, at - 1));
        ui.close();
    }
    if at + 1 < in_col && ui.button("Move down").clicked() {
        *mv.pick = Some((id.to_string(), col, at + 1));
        ui.close();
    }
    for (c, cards) in mv.columns.iter().enumerate() {
        if c != col && ui.button(format!("Move to column {}", c + 1)).clicked() {
            // to the end of that column, where Add column's empty ones start
            *mv.pick = Some((id.to_string(), c, cards.len()));
            ui.close();
        }
    }
}

/// One small caption row under the header: the column names, aligned with the first section.
fn caption_row(ui: &mut Ui, pal: &Palette, w: f32, label_left: f32, layout: &(SecMetrics, f32)) {
    let (rect, _) = ui.allocate_exact_size(vec2(w, caption_h()), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_caption(ui.painter(), pal, rect, label_left, layout);
    }
}

fn paint_caption(p: &egui::Painter, pal: &Palette, rect: Rect, label_left: f32, layout: &(SecMetrics, f32)) {
    let cols = row_cols(rect, &layout.0, label_left, layout.1);
    for (text, x, align) in caption_marks(&cols) {
        p.text(pos2(x, rect.center().y), align, text, f_small(), pal.ink3);
    }
}

fn card(ui: &mut Ui, v: &mut SensorView, fr: &mut FrameRec, g: &SensorGroup, w: f32, col: usize) {
    let pal = v.pal;
    let device = Device::of_card(&g.id);
    let dcolor = device.color(pal);
    let vis = visible_sections(g, v.s);
    let inner_w = w - 2.0;
    let layouts: Vec<(SecMetrics, f32)> = vis.iter().map(|vs| section_layout(ui.painter(), vs, v.s, inner_w)).collect();
    let collapsed = v.s.layout.collapsed_cards.contains(&g.id);
    let outlined = v.hover_device == Some(device);
    let throttle = limits::find_thermal(g).map(|r| limits::throttle_state(r, v.snap.unix_ms));
    let throttle_text = throttle.map(limits::pill_text);
    let info = HeaderInfo {
        tab: device.short(),
        color: dcolor,
        subtitle: g.subtitle.as_deref(),
        throttle: throttle.zip(throttle_text.as_deref()),
        collapsed,
        left: if v.edit { 30.0 } else { 12.0 },
    };
    let head_h = card_head_h();
    let mut head_rect = Rect::NOTHING;
    // The card's one Tab stop: its header until the arrow keys move it to a row; a row that is
    // no longer shown gives it back to the header.
    if v.nav.get(&g.id).is_some_and(|k| !nav_items(g, v.s).contains(k)) {
        v.nav.remove(&g.id);
    }
    let on_header = !v.nav.contains_key(&g.id);
    v.out.stops.push((nav_id(&g.id, v.nav.get(&g.id)), g.id.clone(), v.nav.get(&g.id).cloned()));
    let card_title = || a11y::card_name(device.short(), g.subtitle.as_deref(), throttle_text.as_deref());

    let frame = egui::Frame::new()
        .fill(pal.panel)
        .stroke(if outlined { Stroke::new(1.5, dcolor) } else { Stroke::new(1.0, pal.rule) })
        .corner_radius(8.0)
        .show(ui, |ui| {
            ui.set_width(inner_w);
            ui.spacing_mut().item_spacing.y = 0.0;

            // ---- header ----
            let (_, rect) = ui.allocate_space(vec2(inner_w, head_h));
            let resp = ui.interact(rect, nav_id(&g.id, None), stop_sense(on_header));
            head_rect = rect;
            let mut cut = false;
            if ui.is_rect_visible(rect) {
                cut = paint_header(ui.painter(), pal, rect, resp.hovered(), false, &info);
                a11y::focus_ring(ui, &resp, 7.0, Ring::Inside, pal);
            }
            a11y::keep_keys(&resp, true, true, true);
            a11y::scroll_into_view(ui, &resp);
            a11y::publish(&resp, || Spoken::button(card_title()).expanded(!collapsed).with_menu());
            let resp = match (cut, info.subtitle) {
                (true, Some(sub)) => resp.on_hover_text(sub),
                _ => resp,
            };
            if v.edit {
                let grip_rect = Rect::from_min_size(rect.left_top(), vec2(28.0, head_h));
                grip(ui, grip_rect, egui::Id::new(("grip-card", &g.id)), Dnd::Card(g.id.clone()), pal, || format!("Move the {} card", card_title()));
            }
            if resp.clicked() {
                toggle_card(v.s, &g.id);
            }
            let by_key = resp.has_focus() && a11y::menu_key(ui.ctx());
            let moves = v.edit.then(|| MoveMenu { columns: &fr.columns, pick: &mut fr.pending_move });
            ui_kit::row_menu(&resp, rect, by_key, resp.id, |ui| card_menu(ui, v.s, &g.id, collapsed, moves));

            if collapsed {
                collapsed_chips(ui, v, &vis, inner_w);
                return;
            }

            let label_left = if v.edit { 26.0 } else { 12.0 };
            if let Some(first) = layouts.first() {
                caption_row(ui, pal, inner_w, label_left + 12.0, first);
            }
            if v.edit {
                fr.card_sections = vis.iter().map(|x| x.sec.id.clone()).collect();
            }
            for (i, vs) in vis.iter().enumerate() {
                if i > 0 {
                    ui.add_space(sec_gap());
                }
                section(ui, v, fr, g, vs, &layouts[i], inner_w, dcolor);
            }
            for note in source_notes(v.snap, &g.id) {
                source_note(ui, pal, &note, inner_w);
            }
            ui.add_space(4.0);
        });
    let card_rect = frame.response.rect;
    v.out.card_rects.push((g.id.clone(), card_rect));
    if v.edit {
        fr.cards.push((col, g.id.clone(), card_rect));
        return;
    }
    // While the card scrolls under the top of the view, its header stays pinned there, so every
    // visible row has its device tab above it.
    if collapsed {
        return;
    }
    // The header and the caption row pin as one band; an opaque backing keeps anything that
    // scrolls under it from showing through.
    let caption = layouts.first();
    let band_h = head_h + if caption.is_some() { caption_h() } else { 0.0 };
    let Some(y) = sticky_header_y(ui.clip_rect().top(), head_rect.top(), card_rect.bottom(), band_h) else { return };
    v.out.sticky.push(g.id.clone());
    let band = Rect::from_min_size(pos2(head_rect.left(), y), vec2(head_rect.width(), band_h));
    let pinned = Rect::from_min_size(band.min, head_rect.size());
    // the pinned copy is a pointer target only: the real header is the Tab stop
    let resp = ui.interact(pinned, egui::Id::new(("sticky-head", &g.id)), Sense::CLICK);
    ui.painter().rect_filled(band, 0.0, pal.panel);
    let cut = paint_header(ui.painter(), pal, pinned, resp.hovered(), true, &info);
    if let Some(layout) = caption {
        let cap = Rect::from_min_size(pos2(band.left(), pinned.bottom()), vec2(band.width(), caption_h()));
        paint_caption(ui.painter(), pal, cap, if v.edit { 38.0 } else { 24.0 }, layout);
    }
    ui.painter().hline(band.x_range(), band.bottom() - 0.5, Stroke::new(1.0, pal.rule));
    let resp = match (cut, info.subtitle) {
        (true, Some(sub)) => resp.on_hover_text(sub),
        _ => resp,
    };
    if resp.clicked() {
        toggle_card(v.s, &g.id);
    }
    ui_kit::row_menu(&resp, pinned, false, resp.id, |ui| card_menu(ui, v.s, &g.id, collapsed, None));
}

fn toggle_card(s: &mut Settings, id: &str) {
    let on = !s.layout.collapsed_cards.iter().any(|c| c == id);
    crate::settings::toggle_in(&mut s.layout.collapsed_cards, id, on);
}

/// One line with the summaries of the first three visible sections, as compact chips.
fn collapsed_chips(ui: &mut Ui, v: &SensorView, vis: &[VisSection], w: f32) {
    let pal = v.pal;
    let (rect, _) = ui.allocate_exact_size(vec2(w, snap4(40.0 * crate::theme::font_scale())), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let p = ui.painter().with_clip_rect(rect);
    let mut x = rect.left() + 12.0;
    let cy = rect.center().y;
    let mut shown = 0;
    for vs in vis {
        if shown == 3 {
            break;
        }
        let Some(r) = section_summary(vs.sec, &v.s.sensors.summary_overrides) else { continue };
        let Some(val) = r.value else { continue };
        let thr = effective(v.snap, r);
        // a watched summary wears the watch treatment of a row: a dot, a wash and a green number
        let watched = v.s.is_highlighted(&r.id);
        let c = level_color(pal, level_of(val, &thr), if watched { pal.watch } else { pal.ink });
        let (num, unit) = ui_kit::split_value(r.unit, val);
        let name = p.layout_no_wrap(short_title(&vs.sec.title), f_small(), pal.ink3);
        let numg = p.layout_no_wrap(num, f_result(), c);
        let unitg = p.layout_no_wrap(unit, f_unit(), pal.ink3);
        let dot_w = if watched { CHIP_DOT_W } else { 0.0 };
        let width = 10.0 + dot_w + name.size().x + 6.0 + numg.size().x + 3.0 + unitg.size().x + 10.0;
        if x + width > rect.right() - 8.0 {
            break; // does not fit: leave it out rather than clip it
        }
        let chip = Rect::from_min_size(pos2(x, cy - 12.0), vec2(width, 24.0));
        p.rect(chip, 12.0, pal.header, Stroke::new(1.0, pal.rule), egui::StrokeKind::Inside);
        let mut tx = chip.left() + 10.0;
        if watched {
            p.rect_filled(chip.shrink(1.0), 11.0, pal.watch.gamma_multiply(WATCH_WASH));
            p.circle_filled(pos2(tx + 3.0, cy), 3.0, pal.watch);
            tx += dot_w;
        }
        p.galley(pos2(tx, cy - name.size().y / 2.0), name.clone(), pal.ink3);
        tx += name.size().x + 6.0;
        p.galley(pos2(tx, cy - numg.size().y / 2.0), numg.clone(), c);
        tx += numg.size().x + 3.0;
        p.galley(pos2(tx, cy - unitg.size().y / 2.0), unitg, pal.ink3);
        x += width + 8.0;
        shown += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn section(
    ui: &mut Ui,
    v: &mut SensorView,
    fr: &mut FrameRec,
    g: &SensorGroup,
    vs: &VisSection,
    layout: &(SecMetrics, f32),
    w: f32,
    dcolor: Color32,
) {
    let pal = v.pal;
    let (m, sec_mm) = (&layout.0, layout.1);
    let sec = vs.sec;
    let collapsed = section_collapsed(v.s, sec);
    let sh = row_h();
    let here = cursor_on(v, &g.id, true, &sec.id);
    let (_, rect) = ui.allocate_space(vec2(w, sh));
    let resp = ui.interact(rect, egui::Id::new(("nav-sec", g.id.as_str(), sec.id.as_str())), stop_sense(here));
    let summary = section_summary(sec, &v.s.sensors.summary_overrides);
    // a watched summary keeps the watch treatment of its own row while the section is collapsed
    let watched = collapsed && summary_watched(sec, v.s);
    if collapsed && summary.is_some_and(|r| v.scroll_to.as_deref() == Some(r.id.as_str())) {
        ui.scroll_to_rect(rect, Some(Align::Center));
        *v.scroll_to = None;
    }
    let left = if v.edit { 26.0 } else { 12.0 };
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if resp.hovered() {
            p.rect_filled(rect, 0.0, pal.hover);
        }
        if watched {
            p.rect_filled(rect, 0.0, pal.watch.gamma_multiply(WATCH_WASH));
        }
        chevron(p, pos2(rect.left() + left + 6.0, rect.center().y), !collapsed, dcolor);
        // A collapsed section shares its row with the summary: the same columns as the rows
        // below, so numbers line up. The title is cut to the label cell before it can run into
        // the value; the summary label fills what is left of the cell after the title.
        let cols = collapsed.then(|| summary.map(|r| (row_cols(rect, m, 24.0, sec_mm), r))).flatten();
        let title_x = rect.left() + left + 20.0;
        let title_max = cols.as_ref().map(|(c, _)| c.label.right()).unwrap_or(rect.right() - 12.0) - title_x;
        let title = ellipsized(p, &sec.title, f_section(), pal.ink2, title_max);
        let title_right = title_x + title.size().x;
        p.galley(pos2(title_x, rect.center().y - title.size().y / 2.0), title.clone(), pal.ink2);
        if collapsed {
            if let Some((cols, r)) = cols {
                let label_x = title_right + COLLAPSED_LABEL_GAP + if watched { COLLAPSED_DOT_W } else { 0.0 };
                let avail = cols.label.right() - label_x;
                // the title stays in its own ink; the dot, the label and the value carry the watch
                if watched && !title.elided {
                    p.circle_filled(pos2(title_right + (COLLAPSED_LABEL_GAP + COLLAPSED_DOT_W) / 2.0, rect.center().y), 3.0, pal.watch);
                }
                if avail > COLLAPSED_LABEL_MIN && !title.elided {
                    let (color, font) = if watched { (pal.watch, f_label_bold()) } else { (pal.ink3, f_label()) };
                    let galley = ellipsized(p, &r.label, font, color, avail);
                    p.galley(pos2(label_x, rect.center().y - galley.size().y / 2.0), galley, color);
                }
                let thr = effective(v.snap, r);
                paint_cells(p, &cols, r, &thr, dcolor, pal, watched.then_some(pal.watch), &CellCtx::of(v.snap, r));
            } else {
                p.text(
                    pos2(title_right + 10.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    format!("{} rows", vs.readings.len()),
                    f_small(),
                    pal.ink3,
                );
            }
        }
    }
    if ui.is_rect_visible(rect) {
        a11y::focus_ring(ui, &resp, 0.0, Ring::Inside, pal);
    }
    a11y::keep_keys(&resp, true, true, true);
    a11y::scroll_into_view(ui, &resp);
    a11y::publish(&resp, || {
        let name = match summary.filter(|_| collapsed) {
            Some(r) if watched => format!("{}, {}, {}, watched", sec.title, r.label, a11y::spoken_value(r.unit, r.value)),
            Some(r) => format!("{}, {}, {}", sec.title, r.label, a11y::spoken_value(r.unit, r.value)),
            None => sec.title.clone(),
        };
        Spoken::button(name).expanded(!collapsed).with_menu()
    });
    if v.edit {
        let grip_rect = Rect::from_min_size(rect.left_top() + vec2(2.0, 0.0), vec2(GRIP_W, rect.height()));
        grip(ui, grip_rect, egui::Id::new(("grip-sec", &sec.id)), Dnd::Section { card: g.id.clone(), id: sec.id.clone() }, pal, || format!("Move the section {}", sec.title));
        fr.sections.push((g.id.clone(), sec.id.clone(), rect));
    }
    if collapsed && !v.edit && !resp.context_menu_opened() {
        if let (Some(r), true) = (summary, resp.hovered()) {
            v.out.hover = Some(make_hit(v, g, r, rect));
        }
    }
    if resp.clicked() {
        v.s.layout.collapsed_sections.insert(sec.id.clone(), !collapsed);
    }
    // A collapsed section with a summary opens the popover on hover instead (see `Hit`).
    let popover_here = collapsed && summary.is_some() && v.popover && !v.edit;
    let resp = if !resp.context_menu_opened() && !popover_here {
        resp.on_hover_ui(|ui| section_tooltip(ui, sec, summary, collapsed, v.s))
    } else {
        resp
    };
    let by_key = resp.has_focus() && a11y::menu_key(ui.ctx());
    let edit = v.edit;
    ui_kit::row_menu(&resp, rect, by_key, resp.id, |ui| {
        if ui.button(if collapsed { "Expand" } else { "Collapse" }).clicked() {
            v.s.layout.collapsed_sections.insert(sec.id.clone(), !collapsed);
            ui.close();
        }
        if ui.button("Hide section").clicked() {
            crate::settings::toggle_in(&mut v.s.sensors.hidden_sections, &sec.id, true);
            ui.close();
        }
        let has_override = v.s.sensors.summary_overrides.contains_key(&sec.id);
        if ui.add_enabled(has_override, egui::Button::new("Reset collapsed value")).clicked() {
            v.s.sensors.summary_overrides.remove(&sec.id);
            ui.close();
        }
        if edit {
            reorder_entries(ui, true, &g.id, &sec.id, &fr.card_sections, &mut fr.pending_order);
        }
    });
    if collapsed {
        return;
    }
    if v.edit {
        fr.section_rows = vs.readings.iter().map(|r| r.id.clone()).collect();
    }
    // Rows of one section share the metrics and the width of their min and max cells, so their
    // results and bars line up.
    for (i, r) in vs.readings.iter().enumerate() {
        reading_row(ui, v, fr, g, sec, r, m, sec_mm, w, i % 2 == 1, dcolor);
    }
}

fn make_hit(v: &SensorView, g: &SensorGroup, r: &Reading, rect: Rect) -> Hit {
    let device = Device::of_card(&g.id);
    Hit {
        id: r.id.clone(),
        title: format!("{}, {}", device.short(), r.label),
        unit: r.unit,
        value: r.value,
        thr: effective(v.snap, r),
        color: device.color(v.pal),
        rect,
        desc: r.desc.clone(),
        note: r.note.clone(),
        range: range_line(r, v.snap.stats_since_unix_ms),
    }
}

/// "Since 08:21: min 41.4 °C, average 52.4 °C, max 84.2 °C" (flags have none).
pub fn range_line(r: &Reading, since_ms: u64) -> Option<String> {
    if r.unit == Unit::Flag {
        return None;
    }
    r.stats.as_ref().map(|s| {
        format!(
            "Since {}: min {}, average {}, max {}",
            ui_kit::fmt_clock(since_ms),
            format_value(r.unit, s.min),
            format_value(r.unit, s.avg),
            format_value(r.unit, s.max)
        )
    })
}

fn section_tooltip(ui: &mut Ui, sec: &Section, summary: Option<&Reading>, collapsed: bool, s: &Settings) {
    ui.set_max_width(fs(380.0));
    ui.label(RichText::new(&sec.title).strong());
    ui.label(if collapsed { "Click to expand." } else { "Click to collapse." });
    match summary {
        Some(r) => {
            let custom = s.sensors.summary_overrides.contains_key(&sec.id);
            let who = if custom { "your choice" } else { "default" };
            ui.label(
                RichText::new(format!(
                    "While collapsed this row shows \"{}\" ({who}). Right-click any row and choose \"Show when collapsed\" to pick another.",
                    r.label
                ))
                .weak(),
            );
            if let Some(d) = &r.desc {
                ui.label(RichText::new(d).weak());
            }
        }
        None => {
            ui.label(RichText::new("No value is shown while this section is collapsed.").weak());
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn reading_row(
    ui: &mut Ui,
    v: &mut SensorView,
    fr: &mut FrameRec,
    g: &SensorGroup,
    sec: &Section,
    r: &Reading,
    m: &SecMetrics,
    sec_mm: f32,
    w: f32,
    odd: bool,
    dcolor: Color32,
) {
    let pal = v.pal;
    let here = cursor_on(v, &g.id, false, &r.id);
    let (_, rect) = ui.allocate_space(vec2(w, row_h()));
    let resp = ui.interact(rect, egui::Id::new(("nav-row", g.id.as_str(), r.id.as_str())), stop_sense(here));
    let watched = v.s.is_highlighted(&r.id);
    let thr = effective(v.snap, r);
    let label_left = if v.edit { 38.0 } else { 24.0 };
    if v.scroll_to.as_deref() == Some(r.id.as_str()) {
        ui.scroll_to_rect(rect, Some(Align::Center));
        *v.scroll_to = None;
    }
    let visible = ui.is_rect_visible(rect);
    if visible {
        let p = ui.painter();
        let cols = row_cols(rect, m, label_left, sec_mm);
        if resp.hovered() {
            p.rect_filled(rect, 0.0, pal.hover);
        } else if odd {
            p.rect_filled(rect, 0.0, pal.alt);
        }
        if watched {
            p.rect_filled(rect, 0.0, pal.watch.gamma_multiply(WATCH_WASH));
        }
        if let Some((id, age)) = v.flash {
            if id == r.id && age < 1.2 {
                // one authored moment: a green glow that eases out over 1.2 s
                let a = FLASH_PEAK * (-4.5 * age / 1.2).exp();
                p.rect_filled(rect, 0.0, pal.watch.gamma_multiply(a));
            }
        }
        let (label_color, font) = if watched { (pal.watch, f_label_bold()) } else { (pal.ink2, f_label()) };
        if watched {
            p.circle_filled(pos2(rect.left() + label_left - 9.0, rect.center().y), 3.0, pal.watch);
        }
        let galley = ellipsized(p, &r.label, font, label_color, cols.label.width());
        p.galley(pos2(cols.label.left(), cols.label.center().y - galley.size().y / 2.0), galley, label_color);
        paint_cells(p, &cols, r, &thr, dcolor, pal, watched.then_some(pal.watch), &CellCtx::of(v.snap, r));
        a11y::focus_ring(ui, &resp, 0.0, Ring::Inside, pal);
    }
    a11y::keep_keys(&resp, true, true, true);
    a11y::scroll_into_view(ui, &resp);
    a11y::publish(&resp, || {
        let mut d = Vec::new();
        d.extend(r.desc.clone());
        d.extend(range_line(r, v.snap.stats_since_unix_ms));
        let s = Spoken::new(egui::accesskit::Role::ListItem, a11y::reading_name(Device::of_card(&g.id).short(), r, &thr, watched)).with_menu();
        if d.is_empty() { s } else { s.described(d.join(". ")) }
    });
    if v.edit {
        let grip_rect = Rect::from_min_size(rect.left_top() + vec2(4.0, 0.0), vec2(GRIP_W, rect.height()));
        grip(ui, grip_rect, egui::Id::new(("grip-row", &r.id)), Dnd::Reading { section: sec.id.clone(), id: r.id.clone() }, pal, || format!("Move {}", r.label));
        fr.rows.push((sec.id.clone(), r.id.clone(), rect));
    } else if visible {
        if resp.hovered() && !resp.context_menu_opened() {
            v.out.hover = Some(make_hit(v, g, r, rect));
        }
        if resp.clicked() {
            v.out.clicked = Some(make_hit(v, g, r, rect));
        }
        if v.pinned == Some(r.id.as_str()) {
            v.out.pinned = Some(make_hit(v, g, r, rect));
        }
    }
    let opened = resp.context_menu_opened();
    // The popover opens after 350 ms and says what the tooltip would (see `Hit`), so the tooltip
    // only serves where the popover does not (edit mode).
    let resp = if opened || v.popover {
        resp
    } else {
        resp.on_hover_ui(|ui| reading_tooltip(ui, r, &thr, v.snap.limit_overrides.contains_key(&r.id), v.snap.stats_since_unix_ms))
    };
    let by_key = resp.has_focus() && a11y::menu_key(ui.ctx());
    let edit = v.edit;
    ui_kit::row_menu(&resp, rect, by_key, resp.id, |ui| {
        reading_menu(ui, v, g, sec, r, rect);
        if edit {
            reorder_entries(ui, false, &sec.id, &r.id, &fr.section_rows, &mut fr.pending_order);
        }
    });
}

pub fn reading_tooltip(ui: &mut Ui, r: &Reading, thr: &Thr, custom: bool, since_ms: u64) {
    ui.set_max_width(fs(400.0));
    ui.label(RichText::new(&r.label).strong());
    if let Some(d) = &r.desc {
        ui.label(d);
    }
    if let Some(n) = &r.note {
        ui.label(RichText::new(format!("Source: {n}")).weak());
    }
    if r.unit != Unit::Flag {
        let line = threshold_text(r.unit, thr);
        ui.label(RichText::new(if custom { format!("{line} (custom)") } else { line }).weak());
        if let Some(line) = range_line(r, since_ms) {
            ui.label(RichText::new(line).weak());
        }
    }
    ui.label(RichText::new(&r.id).monospace().weak().size(base() - 2.0));
}

pub fn threshold_text(unit: Unit, t: &Thr) -> String {
    let f = |v: f64| format_value(unit, v);
    if t.is_none() {
        return "No thresholds set".to_string();
    }
    let at = if t.below { "at or below" } else { "at" };
    let mut parts = Vec::new();
    if let Some(w) = t.warn {
        parts.push(format!("warns {at} {}", f(w)));
    }
    if let Some(c) = t.crit {
        parts.push(format!("critical {at} {}", f(c)));
    }
    capitalize(&parts.join(", "))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// (scale from raw value to the edited value, suffix, drag speed) of a unit in the editor.
pub fn unit_edit(unit: Unit) -> (f64, &'static str, f64) {
    match unit {
        Unit::Percent => (1.0, " %", 0.5),
        Unit::Celsius => (1.0, " °C", 0.5),
        Unit::Mhz => (1.0, " MHz", 10.0),
        Unit::Watt => (1.0, " W", 1.0),
        Unit::Volt => (1.0, " V", 0.01),
        Unit::Rpm => (1.0, " RPM", 10.0),
        Unit::Mts => (1.0, " MT/s", 10.0),
        Unit::Flag => (1.0, "", 1.0),
        Unit::Bytes => (1.0 / (1024.0 * 1024.0 * 1024.0), " GiB", 0.1),
        Unit::BytesPerSec => (1e-6, " MB/s", 5.0),
    }
}

fn reading_menu(ui: &mut Ui, v: &mut SensorView, g: &SensorGroup, sec: &Section, r: &Reading, rect: Rect) {
    let watched = v.s.is_highlighted(&r.id);
    if ui.button(if watched { "Stop watching" } else { "Watch" }).clicked() {
        crate::settings::toggle_in(&mut v.s.sensors.highlighted, &r.id, !watched);
        ui.close();
    }
    if ui.button("Show history").clicked() {
        v.out.show_history = Some(make_hit(v, g, r, rect));
        ui.close();
    }
    if ui.button("Hide").on_hover_text("Bring it back from Settings, Sensors").clicked() {
        crate::settings::toggle_in(&mut v.s.sensors.hidden, &r.id, true);
        ui.close();
    }
    let chosen = section_summary(sec, &v.s.sensors.summary_overrides).is_some_and(|s| s.id == r.id);
    let label = if chosen { "Shown when collapsed (selected)" } else { "Show when collapsed" };
    if ui.add_enabled(!chosen, egui::Button::new(label)).clicked() {
        v.s.sensors.summary_overrides.insert(sec.id.clone(), r.id.clone());
        ui.close();
    }
    if let Some(val) = r.value {
        if ui.button("Copy value").clicked() {
            *v.copy = Some(format_value(r.unit, val));
            ui.close();
        }
    }
    if r.unit != Unit::Flag {
        ui.separator();
        ui.label(RichText::new("Thresholds").strong());
        if threshold_editor(ui, v.pal, v.snap, v.pending, r, &mut v.out.thr_edits) {
            ui.close();
        }
    }
}

/// Warning and Critical fields in the reading's unit, the "below" switch and a reset button.
/// The fields show a frame (menus draw buttons without one) and take typed input on a click;
/// dragging them still works. The two fields share one left edge, and a menu's checkboxes keep
/// their outline (menus draw widgets without one). A change is not stored here: it is pushed to
/// `out` for the app to send to the collector, which keeps the limits in thresholds.json; until
/// the snapshots show it, `pending` supplies the value. While thresholds.json has an error the
/// collector keeps the file as it is, so the fields are off. Returns true when "Reset to default"
/// was clicked.
pub fn threshold_editor(ui: &mut Ui, pal: &Palette, snap: &Snapshot, pending: &Pending, r: &Reading, out: &mut Vec<ThrEdit>) -> bool {
    let default = default_thr(snap, r);
    let cur = pending.thr(snap, r);
    let locked = snap.limits_error.is_some();
    let mut edit = cur;
    let (scale, suffix, speed) = unit_edit(r.unit);
    ui.add_enabled_ui(!locked, |ui| {
        ui.scope(|ui| {
            ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::new(1.0, pal.control());
            egui::Grid::new(ui.id().with("threshold-fields")).num_columns(2).show(ui, |ui| {
                for (label, is_warn) in [("Warning", true), ("Critical", false)] {
                    let slot = if is_warn { &mut edit.warn } else { &mut edit.crit };
                    let mut on = slot.is_some();
                    if ui.checkbox(&mut on, label).changed() {
                        *slot = if on { Some(cur.warn.or(cur.crit).or(r.value).unwrap_or(0.0)) } else { None };
                    }
                    if let Some(x) = slot {
                        let mut shown = *x * scale;
                        ui.scope(|ui| {
                            // menus draw buttons bare: give the field the look of an input (the ground
                            // fill of a text edit and a firm outline)
                            let v = ui.visuals_mut();
                            let (fill, line) = (v.extreme_bg_color, v.widgets.hovered.bg_stroke.color);
                            v.button_frame = true;
                            v.widgets.inactive.weak_bg_fill = fill;
                            v.widgets.inactive.bg_fill = fill;
                            v.widgets.inactive.bg_stroke = Stroke::new(1.0, line);
                            ui.spacing_mut().interact_size.x = fs(96.0);
                            let field = egui::DragValue::new(&mut shown).speed(speed).suffix(suffix).max_decimals(3);
                            let resp = ui.add(field).on_hover_text("Click to type a value, or drag it");
                            if resp.changed() {
                                *x = shown / scale;
                            }
                        });
                    } else {
                        ui.label("");
                    }
                    ui.end_row();
                }
            });
            ui.checkbox(&mut edit.below, "Alert when below")
                .on_hover_text("For fans and pumps: warn when the value drops to or under the thresholds");
        });
    });
    if edit != cur {
        out.push(if edit == default { ThrEdit::Reset { id: r.id.clone() } } else { ThrEdit::Set { id: r.id.clone(), thr: edit } });
    }
    if locked {
        ui.label(RichText::new("Paused: thresholds.json has an error").weak().size(base() - 2.0));
    }
    let def_text = format!("Default: {}", threshold_text(r.unit, &default));
    ui.label(RichText::new(def_text).weak().size(base() - 2.0));
    let reset = ui.add_enabled(pending.custom(snap, r) && !locked, egui::Button::new("Reset to default")).clicked();
    if reset {
        out.push(ThrEdit::Reset { id: r.id.clone() });
    }
    reset
}

// ---------------------------------------------------------------------------
// Skeleton while connecting
// ---------------------------------------------------------------------------

/// A column of the placeholder: its width and the devices of its cards, top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct SkelCol {
    pub width: f32,
    pub cards: Vec<Device>,
}

/// The columns the skeleton is drawn in: the saved layout's when the user made one (arranged as
/// the real panels are, so the placeholder matches), otherwise the automatic count for the width.
pub fn skeleton_columns(prefs: &LayoutPrefs, avail: f32) -> Vec<SkelCol> {
    if !prefs.auto && prefs.columns.iter().any(|c| !c.cards.is_empty()) {
        let cards: Vec<(String, usize)> =
            prefs.columns.iter().flat_map(|c| c.cards.iter()).map(|id| (id.clone(), skeleton_rows(Device::of_card(id)))).collect();
        return layout::arrangement(prefs, &cards, avail, crate::theme::font_scale(), false)
            .into_iter()
            .map(|c| SkelCol { width: c.width, cards: c.cards.iter().map(|id| Device::of_card(id)).collect() })
            .collect();
    }
    let n = layout::column_count(avail, COL_GAP, 4, layout::MIN_COL_W, layout::MAX_COL_W).max(1);
    let col_w = layout::snap_down((avail - COL_GAP * (n as f32 - 1.0)) / n as f32);
    let devices = [Device::Cpu, Device::Gpu, Device::Memory, Device::Drive];
    (0..n).map(|c| SkelCol { width: col_w, cards: vec![devices[c % devices.len()]] }).collect()
}

/// Rows a placeholder card of this device shows.
fn skeleton_rows(d: Device) -> usize {
    match d {
        Device::Cpu | Device::Gpu | Device::Board => 9,
        Device::Memory => 7,
        Device::Drive => 6,
        Device::Igpu => 3,
    }
}

/// Placeholder panels shown before the first snapshot of a running collector: header tabs and
/// quiet row bars, in the columns of the saved layout.
pub fn skeleton(ui: &mut Ui, pal: &Palette, avail: f32, prefs: &LayoutPrefs) {
    let cols = skeleton_columns(prefs, avail);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = COL_GAP;
        for (c, col) in cols.iter().enumerate() {
            ui.vertical(|ui| {
                ui.set_width(col.width);
                ui.spacing_mut().item_spacing.y = 12.0;
                for (k, dev) in col.cards.iter().enumerate() {
                    let rows = skeleton_rows(*dev);
                    let h = card_head_h() + row_h() * rows as f32 + 8.0;
                    let (rect, _) = ui.allocate_exact_size(vec2(col.width, h), Sense::hover());
                    let p = ui.painter();
                    p.rect(rect, 8.0, pal.panel, Stroke::new(1.0, pal.rule), egui::StrokeKind::Inside);
                    let head = Rect::from_min_size(rect.min + vec2(1.0, 1.0), vec2(col.width - 2.0, card_head_h()));
                    p.rect_filled(head, egui::CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 }, pal.header);
                    let tab = Rect::from_min_size(head.min + vec2(32.0, 8.0), vec2(56.0, head.height() - 16.0));
                    p.rect_filled(tab, 6.0, dev.color(pal).gamma_multiply(0.35));
                    for i in 0..rows {
                        let y = head.bottom() + row_h() * (i as f32 + 0.5);
                        let label_w = 80.0 + ((i * 37 + c * 23 + k * 11) % 70) as f32;
                        p.rect_filled(Rect::from_min_size(pos2(rect.left() + 24.0, y - 4.0), vec2(label_w, 8.0)), 4.0, pal.rule);
                        p.rect_filled(Rect::from_min_size(pos2(rect.right() - 70.0, y - 4.0), vec2(46.0, 8.0)), 4.0, pal.rule);
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwglance_proto::Stats;

    fn rd(id: &str, v: f64) -> Reading {
        Reading {
            id: id.into(),
            label: id.into(),
            unit: Unit::Celsius,
            value: Some(v),
            stats: Some(Stats::first(v)),
            note: None,
            desc: None,
            events: None,
            warn: None,
            crit: None,
        }
    }

    fn sec(id: &str, collapsed: bool, readings: Vec<Reading>, summary: Option<Reading>) -> Section {
        Section { id: id.into(), title: id.into(), collapsed_by_default: collapsed, readings, summary }
    }

    fn group() -> SensorGroup {
        SensorGroup {
            id: "cpu".into(),
            title: "CPU".into(),
            subtitle: None,
            sections: vec![
                sec("cpu/usage", false, vec![rd("cpu/usage/total", 20.0), rd("cpu/usage/core0", 30.0)], Some(rd("cpu/usage/total", 20.0))),
                sec("cpu/clock", false, vec![rd("cpu/clock/avg", 4000.0), rd("cpu/clock/max", 5000.0)], Some(rd("cpu/clock/max", 5000.0))),
                sec("cpu/thread", true, vec![rd("cpu/thread/0", 1.0)], Some(rd("cpu/thread/summary", 99.0))),
            ],
        }
    }

    #[test]
    fn summary_default_override_and_stale_override() {
        let g = group();
        let mut ov = BTreeMap::new();
        assert_eq!(section_summary(&g.sections[0], &ov).unwrap().id, "cpu/usage/total");
        ov.insert("cpu/clock".to_string(), "cpu/clock/avg".to_string());
        assert_eq!(section_summary(&g.sections[1], &ov).unwrap().id, "cpu/clock/avg");
        ov.insert("cpu/clock".to_string(), "cpu/clock/gone".to_string());
        assert_eq!(section_summary(&g.sections[1], &ov).unwrap().id, "cpu/clock/max");
        ov.insert("cpu/thread".to_string(), "cpu/thread/summary".to_string());
        assert_eq!(section_summary(&g.sections[2], &ov).unwrap().id, "cpu/thread/summary");
        let bare = sec("x", false, vec![rd("x/a", 1.0)], None);
        assert!(section_summary(&bare, &BTreeMap::new()).is_none());
        ov.insert("x".to_string(), "x/a".to_string());
        assert_eq!(section_summary(&bare, &ov).unwrap().id, "x/a");
    }

    #[test]
    fn a_collapsed_section_is_watched_when_its_summary_reading_is() {
        let g = group();
        let mut s = Settings::default();
        assert!(g.sections.iter().all(|sec| !summary_watched(sec, &s)));
        // the summary is a row of the section
        s.sensors.highlighted.push("cpu/usage/total".into());
        assert!(summary_watched(&g.sections[0], &s));
        assert!(!summary_watched(&g.sections[1], &s), "another section's summary is not affected");
        // watching a row that is not the summary does not mark the section
        s.sensors.highlighted = vec!["cpu/usage/core0".into()];
        assert!(!summary_watched(&g.sections[0], &s));
        // the daemon's summary is not among the readings, but its id is what the highlight keys on
        assert!(g.sections[2].readings.iter().all(|r| r.id != "cpu/thread/summary"));
        s.sensors.highlighted = vec!["cpu/thread/summary".into()];
        assert!(summary_watched(&g.sections[2], &s));
        // a chosen summary follows the choice: watching the daemon's pick no longer counts
        s.sensors.highlighted = vec!["cpu/clock/max".into()];
        assert!(summary_watched(&g.sections[1], &s));
        s.sensors.summary_overrides.insert("cpu/clock".into(), "cpu/clock/avg".into());
        assert!(!summary_watched(&g.sections[1], &s));
        s.sensors.highlighted.push("cpu/clock/avg".into());
        assert!(summary_watched(&g.sections[1], &s));
    }

    #[test]
    fn collapsed_state_defaults_and_overrides() {
        let g = group();
        let mut s = Settings::default();
        assert!(!section_collapsed(&s, &g.sections[0]));
        assert!(section_collapsed(&s, &g.sections[2]));
        s.layout.collapsed_sections.insert("cpu/thread".into(), false);
        s.layout.collapsed_sections.insert("cpu/usage".into(), true);
        assert!(!section_collapsed(&s, &g.sections[2]));
        assert!(section_collapsed(&s, &g.sections[0]));
    }

    #[test]
    fn hiding_and_ordering() {
        let g = group();
        let mut s = Settings::default();
        let ids = |s: &Settings| -> Vec<String> { visible_sections(&g, s).iter().map(|v| v.sec.id.clone()).collect() };
        assert_eq!(ids(&s), vec!["cpu/usage", "cpu/clock", "cpu/thread"]);
        s.sensors.hidden_sections.push("cpu/clock".into());
        assert_eq!(ids(&s), vec!["cpu/usage", "cpu/thread"]);
        s.layout.section_order.insert("cpu".into(), vec!["cpu/thread".into(), "cpu/usage".into()]);
        assert_eq!(ids(&s), vec!["cpu/thread", "cpu/usage"]);
        s.sensors.hidden.push("cpu/usage/core0".into());
        let vs = visible_sections(&g, &s);
        assert_eq!(vs.iter().find(|v| v.sec.id == "cpu/usage").unwrap().readings.len(), 1);
        s.sensors.hidden.push("cpu/usage/total".into());
        assert_eq!(ids(&s), vec!["cpu/thread"]);
        let mut s = Settings::default();
        s.layout.reading_order.insert("cpu/usage".into(), vec!["cpu/usage/core0".into(), "cpu/usage/total".into()]);
        let vs = visible_sections(&g, &s);
        let order: Vec<&str> = vs[0].readings.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(order, vec!["cpu/usage/core0", "cpu/usage/total"]);
    }

    #[test]
    fn card_weight_ignores_collapsed_sections_but_not_hidden_ones() {
        let g = group();
        let mut s = Settings::default();
        let w = card_weight(&g, &s);
        assert_eq!(w, 2 + (1 + 2) + (1 + 2) + 1);
        s.layout.collapsed_sections.insert("cpu/usage".into(), true);
        assert_eq!(card_weight(&g, &s), w);
        s.sensors.hidden_sections.push("cpu/clock".into());
        assert_eq!(card_weight(&g, &s), w - 3);
        s.layout.collapsed_cards.push("cpu".into());
        assert_eq!(card_weight(&g, &s), 3);
    }

    fn snap_with(g: SensorGroup) -> Snapshot {
        Snapshot {
            proto: 1,
            seq: 1,
            unix_ms: 0,
            stats_since_unix_ms: 0,
            interval_ms: 1000,
            history_minutes: 30,
            hostname: "h".into(),
            sensors: vec![g],
            sources: vec![],
            procs: None,
            summary: Default::default(),
            crossings: vec![],
            limit_overrides: Default::default(),
            limits_error: None,
        }
    }

    #[test]
    fn a_failing_source_is_noted_in_the_card_it_feeds() {
        let mut s = snap_with(group());
        s.sources = vec![
            hwglance_proto::SourceStatus { name: "nvml".into(), ok: false, detail: "driver not loaded".into() },
            hwglance_proto::SourceStatus { name: "lact".into(), ok: false, detail: "LACT unavailable: fan RPM missing (refused)".into() },
            hwglance_proto::SourceStatus { name: "helper".into(), ok: false, detail: "not installed".into() },
            hwglance_proto::SourceStatus { name: "hyprland".into(), ok: false, detail: "no socket".into() },
        ];
        assert_eq!(
            source_notes(&s, "gpu0"),
            ["GPU readings unavailable: driver not loaded", "LACT unavailable: fan RPM missing (refused)"]
        );
        // the AMD and Intel cards do not use NVML or LACT
        assert!(source_notes(&s, "gpu1").is_empty() && source_notes(&s, "igpu").is_empty());
        assert_eq!(source_notes(&s, "cpu"), ["Package power unavailable: not installed"]);
        assert_eq!(source_notes(&s, "memory"), ["Memory speed unavailable: not installed"]);
        assert!(source_notes(&s, "board").is_empty() && source_notes(&s, "disk/x").is_empty());
        s.sources.iter_mut().for_each(|x| x.ok = true);
        assert!(source_notes(&s, "gpu0").is_empty());
        // NVML down takes the GPU card with it: an empty one comes back to carry the note
        s.sources[0].ok = false;
        let with = with_placeholder_cards(&s).expect("a placeholder");
        assert!(with.sensors.iter().any(|g| g.id == "gpu0" && g.sections.is_empty()));
        assert!(with_placeholder_cards(&with).is_none(), "not twice");
        s.sources[0].ok = true;
        assert!(with_placeholder_cards(&s).is_none());
    }

    #[test]
    fn watched_and_reveal() {
        let snap = snap_with(group());
        let mut s = Settings::default();
        s.sensors.highlighted = vec!["cpu/clock/max".into(), "gone/x".into(), "cpu/usage/total".into()];
        let ids: Vec<&str> = highlighted(&snap, &s).iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["cpu/usage/total", "cpu/clock/max"], "display order, unknown ids skipped");
        s.sensors.hidden.push("cpu/clock/max".into());
        assert_eq!(highlighted(&snap, &s).len(), 1);

        s.layout.collapsed_cards.push("cpu".into());
        assert!(reveal(&mut s, &snap, "cpu/thread/0"));
        assert!(s.layout.collapsed_cards.is_empty());
        assert_eq!(s.layout.collapsed_sections.get("cpu/thread"), Some(&false));
        assert!(!reveal(&mut s, &snap, "nope"));
    }

    #[test]
    fn reveal_unhides_the_target() {
        let snap = snap_with(group());
        let mut s = Settings::default();
        s.sensors.hidden.push("cpu/usage/core0".into());
        s.sensors.hidden_sections.push("cpu/usage".into());
        s.sensors.hidden_cards.push("cpu".into());
        assert!(reveal(&mut s, &snap, "cpu/usage/core0"));
        assert!(s.sensors.hidden.is_empty() && s.sensors.hidden_sections.is_empty() && s.sensors.hidden_cards.is_empty());
    }

    #[test]
    fn threshold_text_and_units() {
        let t = Thr { warn: Some(80.0), crit: Some(90.0), below: false };
        assert_eq!(threshold_text(Unit::Celsius, &t), "Warns at 80.0 °C, critical at 90.0 °C");
        let t = Thr { warn: Some(1000.0), crit: Some(400.0), below: true };
        assert_eq!(threshold_text(Unit::Rpm, &t), "Warns at or below 1000 RPM, critical at or below 400 RPM");
        assert_eq!(threshold_text(Unit::Rpm, &Thr::default()), "No thresholds set");
        let (scale, suffix, _) = unit_edit(Unit::Bytes);
        assert_eq!(suffix, " GiB");
        assert!((2.0 * 1024.0f64.powi(3) * scale - 2.0).abs() < 1e-9);
        // the editor and the collector's thresholds.json count in the same units
        for u in [Unit::Percent, Unit::Celsius, Unit::Mhz, Unit::Watt, Unit::Volt, Unit::Rpm, Unit::Mts, Unit::Bytes, Unit::BytesPerSec] {
            let (scale, suffix, _) = unit_edit(u);
            assert!((scale * u.limit_factor() - 1.0).abs() < 1e-12, "{u:?}");
            assert_eq!(suffix.trim(), u.limit_unit_text(), "{u:?}");
        }
        assert_eq!(short_title("Temperatures"), "Temps");
        assert_eq!(short_title("Clocks and Power"), "Clocks");
        assert_eq!(short_title("Capacity and health"), "Capacity");
    }

    #[test]
    fn flag_markers() {
        assert_eq!(flag_marker(Level::Normal, false), None);
        assert_eq!(flag_marker(Level::Warn, false), Some(("H", Level::Warn)));
        assert_eq!(flag_marker(Level::Crit, false), Some(("HH", Level::Crit)));
        assert_eq!(flag_marker(Level::Warn, true), Some(("L", Level::Warn)));
        assert_eq!(flag_marker(Level::Crit, true), Some(("LL", Level::Crit)));
    }

    fn metrics() -> SecMetrics {
        SecMetrics { result_w: 62.0, mm_w: 38.0, mm_min: 38.0, flag_w: 22.0, label_need: 160.0, label_min: 160.0, label_floor: 160.0 }
    }

    fn row(w: f32) -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(w, 28.0))
    }

    #[test]
    fn wide_rows_show_the_bar_between_min_and_max() {
        let c = row_cols(row(497.0), &metrics(), 24.0, 38.0);
        let bar = c.bar.expect("room for the bar");
        assert!((MIN_BAR..=MAX_BAR).contains(&bar.width()), "{}", bar.width());
        let (min, max) = (c.min.unwrap(), c.max.unwrap());
        assert!(min.right() < bar.left() && bar.right() < max.left() && max.right() < c.flag.left());
        assert!(c.label.width() >= 160.0 - 0.5, "the label keeps what it needs: {}", c.label.width());
        // a very wide row caps the bar, and the extra width goes to the label
        let wide = row_cols(row(900.0), &metrics(), 24.0, 38.0);
        assert_eq!(wide.bar.unwrap().width(), MAX_BAR);
        assert!(wide.label.width() > 400.0);
    }

    #[test]
    fn the_bar_goes_before_a_label_is_truncated_and_returns_with_room() {
        let m = metrics();
        let narrow = row_cols(row(461.0), &m, 24.0, 38.0);
        assert!(narrow.bar.is_none(), "no room for the bar at 461 px");
        assert!(narrow.min.is_some() && narrow.max.is_some(), "min and max stay while labels fit");
        assert!(narrow.label.width() >= m.label_need - 0.5, "{} vs {}", narrow.label.width(), m.label_need);
        let mut back = None;
        for w in (400..700).step_by(4) {
            if row_cols(row(w as f32), &m, 24.0, 38.0).bar.is_some() {
                back = Some(w);
                break;
            }
        }
        let back = back.expect("the bar returns");
        assert!(back > 461 && back < 500, "{back}");
        for w in (back..760).step_by(4) {
            let c = row_cols(row(w as f32), &m, 24.0, 38.0);
            assert!(c.bar.is_some());
            assert!(c.label.width() >= m.label_need - 0.5);
        }
    }

    #[test]
    fn min_and_max_go_only_when_labels_still_would_not_fit() {
        let m = metrics();
        let tight = row_cols(row(391.0), &m, 24.0, 38.0);
        assert!(tight.bar.is_none());
        assert!(tight.min.is_none() && tight.max.is_none(), "dropped so the label can stay whole");
        assert!(tight.label.width() >= m.label_need - 0.5, "{}", tight.label.width());
        let short = SecMetrics { label_need: 90.0, label_min: 90.0, label_floor: 90.0, ..m };
        assert!(row_cols(row(391.0), &short, 24.0, 38.0).min.is_some());
    }

    #[test]
    fn rows_of_one_section_share_the_result_edge() {
        let m = metrics();
        let a = row_cols(row(497.0), &m, 24.0, 38.0);
        let b = row_cols(row(497.0), &m, 24.0, 38.0);
        assert_eq!(a.result_right, b.result_right, "results line up down a section");
        // a section whose results are wider than another's costs only its own columns
        let wide = SecMetrics { result_w: 100.0, ..m };
        let c = row_cols(row(497.0), &wide, 24.0, 38.0);
        assert!(c.bar.map_or(0.0, |b| b.width()) <= a.bar.unwrap().width(), "the wide section's bar is the one that gives");
        assert!(a.bar.is_some(), "the narrow-results section keeps its bar");
    }

    #[test]
    fn long_labels_are_ellipsized_before_the_bar_is_dropped() {
        // the longest label wants 200 px, but labels cut at 22 characters need only 130 px
        let m = SecMetrics { label_need: 200.0, label_min: 130.0, label_floor: 130.0, ..metrics() };
        let mut found = false;
        for w in (380..560).step_by(2) {
            let c = row_cols(row(w as f32), &m, 24.0, 38.0);
            let full = SecMetrics { label_min: 200.0, label_floor: 200.0, ..m };
            let strict = row_cols(row(w as f32), &full, 24.0, 38.0);
            if strict.bar.is_none() && c.bar.is_some() {
                found = true;
                assert!(c.bar.unwrap().width() >= MIN_BAR - 0.01, "a bar of at least 72 px: {}", c.bar.unwrap().width());
                assert!(c.label.width() >= m.label_min - 0.5, "labels keep what the capped text needs: {}", c.label.width());
                assert!(c.label.width() < m.label_need, "and no more: the longest labels get an ellipsis");
            }
            if c.bar.is_none() {
                // the bar is only dropped when even the capped labels leave no room for it
                assert!(c.label.width() >= m.label_min - 0.5, "{w}: {}", c.label.width());
                if c.min.is_none() {
                    // and with min and max gone too the labels get their full width back
                    assert!(c.label.width() >= m.label_need - 0.5, "{w}: {}", c.label.width());
                }
            }
        }
        assert!(found, "there is a width where only the ellipsis keeps the bar");
        // wide rows are untouched: full labels and a bar
        let wide = row_cols(row(700.0), &m, 24.0, 38.0);
        assert!(wide.bar.is_some() && wide.label.width() >= m.label_need);
    }

    #[test]
    fn a_narrow_card_cuts_labels_to_14_characters_before_it_drops_the_bar() {
        // "CPU package (board)" is under 22 characters, so only the floor stage can keep the bar
        assert_eq!(label_capped("CPU package (board)"), "CPU package (board)");
        assert_eq!(label_cut("CPU package (board)", LABEL_FLOOR_CHARS), "CPU package (\u{2026}");
        let m = SecMetrics { label_need: 134.0, label_min: 134.0, label_floor: 90.0, ..metrics() };
        let mut kept = false;
        for w in (300..560).step_by(2) {
            let c = row_cols(row(w as f32), &m, 24.0, 38.0);
            let whole = row_cols(row(w as f32), &SecMetrics { label_floor: 134.0, ..m }, 24.0, 38.0);
            if whole.bar.is_none() && c.bar.is_some() {
                kept = true;
                assert!(c.bar.unwrap().width() >= MIN_BAR - 0.01);
                assert!(c.label.width() >= m.label_floor - 0.5 && c.label.width() < m.label_need);
            }
        }
        assert!(kept, "a width exists where only the floor keeps the bar");
    }

    #[test]
    fn min_and_max_also_prefer_ellipsized_labels_to_disappearing() {
        let m = SecMetrics { label_need: 200.0, label_min: 120.0, label_floor: 120.0, ..metrics() };
        let mut seen = false;
        for w in (300..520).step_by(2) {
            let c = row_cols(row(w as f32), &m, 24.0, 38.0);
            if c.bar.is_none() && c.min.is_some() && c.label.width() < m.label_need - 0.5 {
                seen = true;
                assert!(c.label.width() >= m.label_min - 0.5);
            }
        }
        assert!(seen, "some width keeps min and max by ellipsizing the longest labels");
    }

    #[test]
    fn labels_over_22_characters_keep_21_and_an_ellipsis() {
        assert_eq!(label_capped("Memory Controller Load"), "Memory Controller Load", "exactly 22 stays whole");
        assert_eq!(label_capped("Top intake (3 fans) extra"), "Top intake (3 fans) e\u{2026}");
        assert_eq!(label_capped("Top intake (3 fans) extra").chars().count(), LABEL_CHARS);
        assert_eq!(label_capped("Tctl"), "Tctl");
    }

    #[test]
    fn captions_sit_over_their_columns() {
        let c = row_cols(row(497.0), &metrics(), 24.0, 38.0);
        let marks = caption_marks(&c);
        let names: Vec<&str> = marks.iter().map(|m| m.0).collect();
        assert_eq!(names, vec!["now", "low", "range", "high"]);
        assert_eq!(marks[0].1, c.result_right);
        assert_eq!(marks[1].1, c.min.unwrap().right());
        assert_eq!(marks[2].1, c.bar.unwrap().center().x);
        assert_eq!(marks[3].1, c.max.unwrap().right());
        // a section that dropped its bar has no "range" caption; one that dropped min and max keeps "now"
        let narrow = caption_marks(&row_cols(row(461.0), &metrics(), 24.0, 38.0));
        assert_eq!(narrow.iter().map(|m| m.0).collect::<Vec<_>>(), vec!["now", "low", "high"]);
        let tight = caption_marks(&row_cols(row(391.0), &metrics(), 24.0, 38.0));
        assert_eq!(tight.iter().map(|m| m.0).collect::<Vec<_>>(), vec!["now"]);
    }

    #[test]
    fn the_header_pins_while_rows_of_its_card_are_in_view() {
        let head_h = 36.0;
        // header still on screen: nothing pinned
        assert_eq!(sticky_header_y(100.0, 100.0, 500.0, head_h), None);
        assert_eq!(sticky_header_y(100.0, 160.0, 500.0, head_h), None);
        // scrolled past the header, rows remain: pinned at the top of the view
        assert_eq!(sticky_header_y(100.0, 40.0, 500.0, head_h), Some(100.0));
        assert_eq!(sticky_header_y(100.0, -300.0, 300.0, head_h), Some(100.0));
        // the card is ending: the pinned header slides out with the card's bottom edge
        assert_eq!(sticky_header_y(100.0, -300.0, 120.0, head_h), Some(84.0));
        // the card is gone
        assert_eq!(sticky_header_y(100.0, -300.0, 100.0, head_h), None);
        assert_eq!(sticky_header_y(100.0, -300.0, 60.0, head_h), None);
        // a visible row always has a device tab above it: whenever the card's bottom is below
        // the view top, either the real header is at or below it or the pinned one is
        for top in [-500.0f32, -200.0, 0.0, 50.0, 99.0, 100.0, 140.0] {
            let bottom = top + 400.0;
            if bottom > 100.0 {
                assert!(top >= 99.5 || sticky_header_y(100.0, top, bottom, head_h).is_some(), "top {top}");
            }
        }
    }

    fn state_group(id: &str, readings: Vec<Reading>) -> SensorGroup {
        SensorGroup {
            id: id.into(),
            title: id.into(),
            subtitle: None,
            sections: vec![sec(&format!("{id}/temp"), false, readings, None)],
        }
    }

    fn flag_reading(id: &str, on: bool) -> Reading {
        let mut r = rd(id, if on { 1.0 } else { 0.0 });
        r.unit = Unit::Flag;
        r
    }

    fn snap_of(groups: Vec<SensorGroup>) -> Snapshot {
        Snapshot { sensors: groups, ..snap_with(group()) }
    }

    #[test]
    fn a_legend_chip_carries_the_worst_current_state_of_its_device() {
        let mut warm = rd("cpu/temp/tctl", 85.0);
        warm.warn = Some(80.0);
        warm.crit = Some(95.0);
        let mut other = rd("cpu/temp/ccd", 40.0);
        other.warn = Some(80.0);
        let snap = snap_of(vec![state_group("cpu", vec![other.clone(), warm.clone()])]);
        let s = Settings::default();
        let st = device_state(&snap, &s, Device::Cpu);
        assert_eq!((st.level, st.mark, st.target.as_deref()), (Level::Warn, Some("H"), Some("cpu/temp/tctl")));
        assert_eq!(st.meaning(), "is above its warning level");
        // critical beats warning, and the first reading with that state is the target
        let mut hot = rd("cpu/temp/ccd2", 99.0);
        hot.crit = Some(95.0);
        let snap = snap_of(vec![state_group("cpu", vec![warm.clone(), hot])]);
        let st = device_state(&snap, &s, Device::Cpu);
        assert_eq!((st.level, st.mark, st.target.as_deref()), (Level::Crit, Some("HH"), Some("cpu/temp/ccd2")));
        // quiet devices carry nothing, and so do devices of other kinds
        let snap = snap_of(vec![state_group("cpu", vec![other.clone()]), state_group("gpu0", vec![warm.clone()])]);
        assert_eq!(device_state(&snap, &s, Device::Cpu), DeviceState::normal());
        assert_eq!(device_state(&snap, &s, Device::Gpu).mark, Some("H"));
        assert_eq!(device_state(&snap, &s, Device::Memory), DeviceState::normal());
        // throttling now counts (red), ranks under HH and over H
        let snap = snap_of(vec![state_group("gpu0", vec![warm.clone(), flag_reading("gpu0/throttle/thermal", true)])]);
        let st = device_state(&snap, &s, Device::Gpu);
        assert_eq!((st.level, st.mark), (Level::Crit, Some(THROTTLE_MARK)));
        let off = snap_of(vec![state_group("gpu0", vec![flag_reading("gpu0/throttle/thermal", false)])]);
        assert_eq!(device_state(&off, &s, Device::Gpu), DeviceState::normal());
        // several cards of one device (drives) are looked at together
        let snap = snap_of(vec![state_group("disk/a", vec![other.clone()]), state_group("disk/b", vec![warm.clone()])]);
        assert_eq!(device_state(&snap, &s, Device::Drive).target.as_deref(), Some("cpu/temp/tctl"));
    }

    #[test]
    fn chip_state_follows_the_users_thresholds_and_hidden_things() {
        let r = rd("cpu/temp/tctl", 60.0);
        let snap = snap_of(vec![state_group("cpu", vec![r.clone()])]);
        let mut s = Settings::default();
        assert_eq!(device_state(&snap, &s, Device::Cpu), DeviceState::normal());
        // the collector sends the user's limits in the reading (and says they are the user's)
        let mut mine = r.clone();
        mine.warn = Some(50.0);
        mine.crit = Some(55.0);
        let snap = snap_of(vec![state_group("cpu", vec![mine])]);
        let st = device_state(&snap, &s, Device::Cpu);
        assert_eq!((st.level, st.mark), (Level::Crit, Some("HH")));
        s.sensors.hidden.push("cpu/temp/tctl".into());
        assert_eq!(device_state(&snap, &s, Device::Cpu), DeviceState::normal(), "a hidden value is not flagged");
        s.sensors.hidden.clear();
        s.sensors.hidden_cards.push("cpu".into());
        assert_eq!(device_state(&snap, &s, Device::Cpu), DeviceState::normal(), "nor is a hidden panel");
        // a fan below its limit reads L
        let mut fan = rd("board/fan/pump", 300.0);
        fan.unit = Unit::Rpm;
        fan.warn = Some(1000.0);
        fan.crit = Some(100.0);
        let mut snap = snap_of(vec![state_group("board", vec![fan])]);
        let s = Settings::default();
        snap.limit_overrides.insert(
            "board/fan/pump".into(),
            hwglance_proto::LimitOverride { warn: Some(1000.0), crit: Some(100.0), below: true, default_warn: None, default_crit: None },
        );
        assert_eq!(device_state(&snap, &s, Device::Board).mark, Some("L"));
    }

    #[test]
    fn the_skeleton_uses_the_saved_layouts_columns() {
        let mut prefs = Settings::default().layout;
        // automatic layout: columns by width, one card each
        let auto = skeleton_columns(&prefs, 2000.0);
        assert!(auto.len() >= 3 && auto.iter().all(|c| c.cards.len() == 1));
        // a saved layout: its widths and its cards' devices
        prefs.auto = false;
        prefs.columns = vec![
            crate::settings::ColumnPref { width: 461.0, cards: vec!["cpu".into(), "memory".into()] },
            crate::settings::ColumnPref { width: 520.0, cards: vec!["gpu0".into()] },
        ];
        // (a window exactly as wide as the saved columns, so the widths stay as saved)
        let cols = skeleton_columns(&prefs, 461.0 + 520.0 + COL_GAP);
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0], SkelCol { width: 461.0, cards: vec![Device::Cpu, Device::Memory] });
        assert_eq!(cols[1], SkelCol { width: 520.0, cards: vec![Device::Gpu] });
        // a wider window grows them to fill it, as the real panels do
        let wide = skeleton_columns(&prefs, 2000.0);
        assert!(wide[0].width > 461.0 && wide[1].width > wide[0].width);
        // a layout wider than the window shrinks to fit, as the real panels do
        let narrow = skeleton_columns(&prefs, 900.0);
        assert_eq!(narrow.len(), 2);
        assert!(narrow.iter().map(|c| c.width).sum::<f32>() + COL_GAP <= 900.0 + 0.5);
        // and one too narrow for its columns stacks them, in order, as the real panels do
        let stacked = skeleton_columns(&prefs, 700.0);
        assert_eq!(stacked.len(), 1);
        assert_eq!(stacked[0].cards, vec![Device::Cpu, Device::Memory, Device::Gpu]);
    }

    #[test]
    fn a_collapsed_row_shrinks_its_bar_to_the_minimum_before_it_cuts_the_summary_label() {
        let title = 60.0;
        let (need, min, floor) = collapsed_label_cell(title, [150.0, 120.0, 90.0]);
        assert!(need > min && min > floor, "{need} {min} {floor}");
        // the whole label and the title both fit the cell
        assert!(need >= COLLAPSED_TITLE_INSET + title + 10.0 + 150.0);
        let m = SecMetrics {
            result_w: 60.0,
            mm_w: 40.0,
            mm_min: 40.0,
            flag_w: 20.0,
            label_need: need,
            label_min: min,
            label_floor: floor,
        };
        let row = |w: f32| row_cols(Rect::from_min_size(pos2(0.0, 0.0), vec2(w, 28.0)), &m, 24.0, 40.0);
        // fixed part: flag, result, gaps, the min and max cells and their gaps, padding
        let fixed = 12.0 + 20.0 + GAP + 12.0 + 60.0 + GAP + 24.0 + 2.0 * 40.0 + 16.0;
        // roomy: the bar is wider than its minimum and the label cell holds the whole label
        let wide = row(fixed + need + MAX_BAR + 20.0);
        assert!(wide.bar.is_some() && wide.label.width() >= need - 0.5);
        // tighter: the bar shrinks, down to MIN_BAR, and the label cell stays whole
        let tight = row(fixed + need + MIN_BAR + 1.0);
        let bar = tight.bar.expect("a bar at its minimum");
        assert!(bar.width() >= MIN_BAR - 0.5 && bar.width() < MIN_BAR + 3.0, "{}", bar.width());
        assert!(tight.label.width() >= need - 0.5, "the label is not cut yet: {} against {need}", tight.label.width());
        // tighter still: only now is the label cut (to the 22 characters), and the bar keeps its minimum
        let cut = row(fixed + need - 8.0 + MIN_BAR);
        assert!(cut.bar.is_some_and(|b| b.width() >= MIN_BAR - 0.5));
        assert!(cut.label.width() < need && cut.label.width() >= min - 0.5, "{}", cut.label.width());
        // a summary is never squeezed to a stub: the cell always leaves it the painter's minimum
        let (_, _, floor_w) = collapsed_label_cell(title, [150.0, 120.0, 10.0]);
        assert!(floor_w >= COLLAPSED_TITLE_INSET + title + 10.0 + COLLAPSED_LABEL_MIN);
    }
}
