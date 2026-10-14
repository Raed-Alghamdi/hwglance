//! The Task Manager-style process panel: search, sortable and configurable header, virtualised
//! tree, actions. It docks below the sensors or to their right.

use crate::a11y::{self, Ring, Spoken};
use crate::actions::Signal;
use crate::app::{App, KillState, SEARCH_ID};
use crate::fonts;
use crate::model::{self, class_title, RowId, RowKind, SortKey};
use crate::settings::{Dock, PanelView};
use crate::theme::{base, snap4, Palette};
use crate::ui_kit::{self, ellipsized, f_label, f_label_bold, f_mono, f_mono_bold, f_section, f_small, icon};
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, Ui, UiBuilder};
use hwglance_proto::{format_value, ProcClass, ProcGroup, Unit};
use std::collections::{HashMap, HashSet};

/// Width of the bar that marks the selected process row.
pub const SELECTED_BAR_W: f32 = 3.0;

fn row_h() -> f32 {
    crate::theme::hit_h(26.0)
}
fn header_h() -> f32 {
    crate::theme::hit_h(28.0)
}
fn toolbar_h() -> f32 {
    snap4(40.0 * crate::theme::font_scale())
}
fn actions_h() -> f32 {
    snap4(44.0 * crate::theme::font_scale())
}
/// Strength of a column header's colour while it is neither sorted nor hovered (4.5:1 for every
/// device colour on the header band).
pub const HEADER_REST_ALPHA: f32 = 0.95;
/// Rows the bottom dock always keeps on screen, however small the window.
pub const MIN_PANEL_ROWS: f32 = 5.0;
/// Vertical inset of the panel's content on each side (design pixels).
const PANEL_PAD_Y: f32 = 8.0;

/// The least height of the process panel that still lists `rows` rows: the insets, the toolbar,
/// the header, the action row and the rows themselves.
pub fn min_panel_height(rows: f32) -> f32 {
    2.0 * PANEL_PAD_Y + toolbar_h() + header_h() + actions_h() + 2.0 + rows * row_h()
}

const HEAVY_GPU: f32 = 25.0;
/// Memory tint of a machine whose total RAM is not known, and the share of the total it is
/// otherwise (a sixteenth: 2 GiB of 32).
const HEAVY_MEM_FALLBACK: u64 = 2 * 1024 * 1024 * 1024;
const HEAVY_MEM_SHARE: u64 = 16;
/// The least memory that tints, however small the machine.
const HEAVY_MEM_FLOOR: u64 = 512 * 1024 * 1024;
/// A thread counts as running away from this share of one logical CPU, so a pegged thread whose
/// smoothed reading sits a little under 100 % still tints.
const HEAVY_CPU_OF_ONE: f32 = 0.9;
/// Combined storage read and write from which a Disk cell is tinted, bytes per second.
const HEAVY_DISK: f32 = 50e6;

enum Act {
    Select(RowId),
    Toggle(RowId),
    Kill(RowId, Signal),
    CopyPid(RowId),
    CopyCmd(RowId),
    OpenLocation(RowId),
    CopyPath(RowId),
}

/// Widest the Name column gets in the bottom dock (design pixels).
const NAME_CAP: f32 = 560.0;
/// Share of the panel width the Name column may take in the right dock.
pub const NAME_CAP_RIGHT: f32 = 0.4;
/// The Name column never gets narrower than this (design pixels) before the table scrolls.
const NAME_MIN: f32 = 200.0;
/// Name's minimum in the compact views (design pixels): the app icon and about ten characters,
/// so the few columns of a compact view all stay on screen and Name is what gives way.
const NAME_MIN_COMPACT: f32 = 120.0;
/// The Location column's minimum width (design pixels).
const LOCATION_MIN: f32 = 160.0;
/// Where a group's or process's name text starts, from the left edge of its row (after the
/// chevron, the icon and the tree indent).
const NAME_TEXT_X: f32 = 60.0;
/// Room "fit" leaves after the widest name: the cell's 4 px right inset, the 4 px edge of the
/// ellipsis and a little air.
const NAME_FIT_PAD: f32 = 12.0;
/// Padding a data column adds to its widest text (design pixels): the cell's right inset and air.
const CELL_PAD: f32 = 20.0;
/// Room a header keeps for its sort arrow beside the label (design pixels, scaled with the font),
/// so sorting by a column never changes its width.
const ARROW_ROOM: f32 = 16.0;
/// How long a data column's text has to stay narrower before the column gives its room back, so
/// values ticking across a digit do not make the table breathe.
const SHRINK_AFTER_S: f64 = 3.0;
/// Half the width of the Name divider's hit area: 24 px wide, the floor of a target, around a
/// line that stays 1 or 2 px.
const DIVIDER_HIT_HALF: f32 = crate::theme::MIN_HIT / 2.0;

/// The process table's two Tab stops: its column header (Left and Right choose a column, Enter
/// sorts) and its list (Up and Down move the selection). Rows and cells are not stops.
pub const HEADER_ID: &str = "proc-hdr-bg";
pub const LIST_ID: &str = "proc-list";

/// Column geometry of the table: the optional columns in their fixed order after Name, and
/// Location (when shown) taking the width that is left.
pub struct Cols {
    keys: Vec<SortKey>,
    widths: Vec<f32>,
    name_cap: f32,
    /// Minimum widths of Name and Location at the font size the table was built for.
    name_min: f32,
    loc_min: f32,
    /// Name width the user dragged to; `None` keeps the automatic (capped) width.
    name_width: Option<f32>,
    /// The right dock's layout: Name takes everything the data columns leave, so a wider panel
    /// always widens Name, and `data_extra` (not `name_width`) sets where the boundary sits.
    right_dock: bool,
    /// Width the user gave the data columns beyond their autofit total (right dock), shared
    /// equally by them; `None` is none. Clamped when laid out so Name keeps its minimum.
    data_extra: Option<f32>,
}

/// Width in characters of the mono font a column gets before its content has been measured
/// (see `ColFit`); Location is flexible.
fn column_chars(key: SortKey) -> f32 {
    match key {
        SortKey::Pid | SortKey::Gpu | SortKey::Threads => 7.0,
        SortKey::Cpu | SortKey::Encode | SortKey::Decode => 8.0,
        SortKey::Vram | SortKey::Mem => 9.0,
        SortKey::Disk => 11.0,
        SortKey::User => 12.0,
        SortKey::Started => 8.0,
        SortKey::Name | SortKey::Location => 0.0,
    }
}

/// Columns whose text starts at the left edge (the others are numbers, right-aligned).
pub fn left_aligned(key: SortKey) -> bool {
    matches!(key, SortKey::User | SortKey::Started | SortKey::Location | SortKey::Name)
}

impl Cols {
    pub fn new(keys: Vec<SortKey>, char_w: f32, name_cap: f32) -> Cols {
        let loc_min = crate::theme::fs(LOCATION_MIN);
        let widths = keys
            .iter()
            .map(|k| match k {
                SortKey::Location => loc_min,
                k => (char_w * column_chars(*k)).ceil() + 20.0,
            })
            .collect();
        Cols { keys, widths, name_cap, name_min: crate::theme::fs(NAME_MIN), loc_min, name_width: None, right_dock: false, data_extra: None }
    }

    /// Give the data columns the widths `fit` has measured for them; a column it does not know
    /// yet keeps its starting width.
    pub fn with_fit(mut self, fit: &ColFit) -> Cols {
        for (k, w) in self.keys.iter().zip(self.widths.iter_mut()) {
            if let Some(shown) = fit.width(*k) {
                *w = shown;
            }
        }
        self
    }

    /// Lay the table out for the right dock: the data columns are their autofit widths plus
    /// `extra` shared equally, and Name takes all the rest, so a wider or narrower panel changes
    /// Name only. The Name width set by hand does not apply there.
    pub fn right_dock(mut self, extra: Option<f32>) -> Cols {
        self.right_dock = true;
        self.data_extra = extra;
        self
    }

    /// The compact views let Name get narrower than the expanded ones do.
    pub fn for_view(mut self, view: PanelView) -> Cols {
        self.name_min = crate::theme::fs(if view == PanelView::Compact { NAME_MIN_COMPACT } else { NAME_MIN });
        self
    }

    /// Use an explicit Name width (clamped when laid out) instead of the automatic one.
    pub fn with_name_width(mut self, width: Option<f32>) -> Cols {
        self.name_width = width;
        self
    }

    pub fn keys(&self) -> &[SortKey] {
        &self.keys
    }

    fn has_location(&self) -> bool {
        self.keys.contains(&SortKey::Location)
    }

    /// Width of the columns that keep their size (everything but Name and Location).
    fn fixed_width(&self) -> f32 {
        self.keys.iter().zip(&self.widths).filter(|(k, _)| **k != SortKey::Location).map(|(_, w)| *w).sum()
    }

    /// The least width at which the table needs no horizontal scrolling.
    pub fn content_width(&self) -> f32 {
        let loc = if self.has_location() { self.loc_min } else { 0.0 };
        self.name_min + self.fixed_width() + loc
    }

    /// The least and most Name can be in a row of width `row_w`: its view's minimum, and what
    /// keeps every data column (and Location at its minimum) on screen. A row too narrow even for
    /// the minimum gives Name all that is left.
    pub fn name_bounds(&self, row_w: f32) -> (f32, f32) {
        let loc_min = if self.has_location() { self.loc_min } else { 0.0 };
        let most = (row_w - self.fixed_width() - loc_min).max(0.0);
        (self.name_min.min(most), most)
    }

    pub fn is_right_dock(&self) -> bool {
        self.right_dock
    }

    /// Name's width in the right dock before `data_extra` takes its share: all that the data
    /// columns (and Location at its minimum) leave, or with a Location column the automatic
    /// width, up to `name_cap`, so Location keeps taking the remainder.
    pub fn name_auto(&self, row_w: f32) -> f32 {
        let (_, hi) = self.name_bounds(row_w);
        if self.has_location() { hi.min(self.name_cap) } else { hi }
    }

    /// The most the data columns can take beyond their autofit widths in a row of width
    /// `row_w` (right dock): what leaves Name its view's minimum.
    pub fn data_extra_max(&self, row_w: f32) -> f32 {
        (self.name_auto(row_w) - self.name_bounds(row_w).0).max(0.0)
    }

    /// The extra width the data columns have in a row of width `row_w` (right dock): the saved
    /// one held to what Name's minimum allows. The saved value itself is never rewritten by it.
    pub fn data_extra_now(&self, row_w: f32) -> f32 {
        let keys = self.keys.iter().filter(|k| **k != SortKey::Location).count();
        if keys == 0 {
            return 0.0;
        }
        self.data_extra.unwrap_or(0.0).clamp(0.0, self.data_extra_max(row_w))
    }

    /// Name rect plus the rects of the optional columns of `row`. The name is the user's width
    /// (clamped to `name_bounds`) or else takes what the other columns leave, up to `name_cap`;
    /// the columns sit right after it, with Location (last) taking all that remains, so the eye
    /// does not travel across a wide window to reach them. In the right dock (see `right_dock`)
    /// the data columns take their extra width instead, out of Name.
    pub fn rects(&self, row: Rect) -> (Rect, Vec<(SortKey, Rect)>) {
        let (top, bottom) = (row.top(), row.bottom());
        let (lo, hi) = self.name_bounds(row.width());
        let (name_w, extra) = if self.right_dock {
            let extra = self.data_extra_now(row.width());
            let data = self.keys.iter().filter(|k| **k != SortKey::Location).count().max(1);
            (self.name_auto(row.width()) - extra, extra / data as f32)
        } else {
            (
                match self.name_width {
                    Some(w) => w.clamp(lo, hi),
                    None => hi.min(self.name_cap),
                },
                0.0,
            )
        };
        let mut x = row.left() + name_w;
        let mut out = Vec::with_capacity(self.keys.len());
        for (k, w) in self.keys.iter().zip(&self.widths) {
            let w = if *k == SortKey::Location { (row.right() - x).max(*w) } else { *w + extra };
            out.push((*k, Rect::from_min_max(pos2(x, top), pos2(x + w, bottom))));
            x += w;
        }
        (Rect::from_min_max(row.left_top(), pos2(row.left() + name_w - 4.0, bottom)), out)
    }
}

/// The width Name needs to show the widest of the visible group and process names whole, with
/// their process counts, measured by `measure(text, bold)`. Window titles and section captions
/// are not counted.
pub fn widest_name_extent(rows: &[model::Row], groups: &[ProcGroup], measure: impl Fn(&str, bool) -> f32) -> f32 {
    let mut widest = 0.0_f32;
    for row in rows {
        let w = match &row.kind {
            RowKind::Group { gi, .. } => {
                let g = &groups[*gi];
                let count = if g.procs.len() > 1 { 6.0 + measure(&format!("({})", g.procs.len()), false) } else { 0.0 };
                measure(&g.name, true) + count
            }
            RowKind::Proc { gi, pi } => measure(&groups[*gi].procs[*pi].name, false),
            RowKind::Section { .. } => continue,
        };
        widest = widest.max(NAME_TEXT_X + w);
    }
    widest
}

/// Name width that fits a name needing `extent`, kept within the column's bounds.
pub fn fit_name_width(extent: f32, (lo, hi): (f32, f32)) -> f32 {
    (extent + NAME_FIT_PAD).clamp(lo, hi)
}

/// The width a data column needs: its header (label and sort arrow room) or its widest cell text,
/// whichever is wider, plus the cell padding.
pub fn column_want(header_w: f32, cell_w: f32) -> f32 {
    (header_w + crate::theme::fs(ARROW_ROOM)).max(cell_w) + CELL_PAD
}

/// The widest text, per data column in `keys`, of the cells `rows` show, measured by
/// `measure(column, text)`. Numbers share one character width, so only the longest of them is
/// measured; the proportional columns (User, Started) measure each distinct text.
pub fn column_extents(
    keys: &[SortKey],
    rows: &[model::Row],
    groups: &[ProcGroup],
    now_ms: u64,
    measure: impl Fn(SortKey, &str) -> f32,
) -> Vec<(SortKey, f32)> {
    let mut out = Vec::new();
    for key in keys.iter().copied().filter(|k| !matches!(k, SortKey::Name | SortKey::Location)) {
        let mut seen: HashSet<String> = HashSet::new();
        let mut longest = String::new();
        let mut widest = 0.0_f32;
        for row in rows {
            // a class row shows only its totals
            if matches!(row.kind, RowKind::Section { .. }) && !matches!(key, SortKey::Cpu | SortKey::Mem) {
                continue;
            }
            let text = cell_text(key, &row_values(row, groups), now_ms);
            if text.is_empty() {
                continue;
            }
            if left_aligned(key) {
                if !seen.contains(&text) {
                    widest = widest.max(measure(key, &text));
                    seen.insert(text);
                }
            } else if text.chars().count() > longest.chars().count() {
                longest = text;
            }
        }
        if !longest.is_empty() {
            widest = widest.max(measure(key, &longest));
        }
        out.push((key, widest));
    }
    out
}

/// The autofitted widths of the data columns, with the hysteresis that keeps them from jittering
/// as values tick: a column grows the moment its text needs more room and gives room back only
/// after its text has needed less for `SHRINK_AFTER_S` (down to the widest it needed meanwhile).
#[derive(Default)]
pub struct ColFit {
    cols: HashMap<SortKey, FitEntry>,
    /// Font scale the widths were measured at; a change starts them over.
    scale: f32,
}

struct FitEntry {
    shown: f32,
    /// When the text started needing less than `shown`, and the widest need since.
    narrower: Option<(f64, f32)>,
}

impl ColFit {
    /// The width shown for `key`, once it has been measured.
    pub fn width(&self, key: SortKey) -> Option<f32> {
        self.cols.get(&key).map(|e| e.shown)
    }

    /// Take the widths `wanted` at time `now` (seconds). Columns no longer wanted are forgotten.
    /// Returns when the next column is due to shrink, so the caller can repaint then.
    pub fn update(&mut self, wanted: &[(SortKey, f32)], now: f64, scale: f32) -> Option<f64> {
        if self.scale != scale {
            self.cols.clear();
            self.scale = scale;
        }
        self.cols.retain(|k, _| wanted.iter().any(|(w, _)| w == k));
        let mut due: Option<f64> = None;
        for (key, want) in wanted {
            let e = self.cols.entry(*key).or_insert(FitEntry { shown: *want, narrower: None });
            if *want >= e.shown - 0.5 {
                // wider (or the same within a pixel): follow it at once
                e.shown = e.shown.max(*want);
                e.narrower = None;
                continue;
            }
            let (since, peak) = e.narrower.get_or_insert((now, *want));
            *peak = peak.max(*want);
            if now - *since >= SHRINK_AFTER_S {
                e.shown = *peak;
                e.narrower = None;
            } else {
                let at = *since + SHRINK_AFTER_S;
                due = Some(due.map_or(at, |d| d.min(at)));
            }
        }
        due
    }
}

/// Width of one character of the mono font.
fn mono_char_w(ui: &Ui) -> f32 {
    ui.painter().layout_no_wrap("0000000000".into(), f_mono(), Color32::WHITE).size().x / 10.0
}

/// Horizontal inset of the process panel's content on each side.
const PANEL_PAD_X: f32 = 12.0;

/// The narrowest panel at which a compact view shows every one of its columns without a sideways
/// scrollbar: Name at its compact minimum, the columns, the list's vertical bar and the insets.
pub fn compact_min_width(ui: &Ui, fit: &ColFit, columns: &[String]) -> f32 {
    let cols = Cols::new(model::visible_columns(columns), mono_char_w(ui), 0.0).for_view(PanelView::Compact).with_fit(fit);
    panel_width_for(&cols, ui.spacing().scroll.allocated_width())
}

fn panel_width_for(cols: &Cols, bar: f32) -> f32 {
    cols.content_width() + bar + 2.0 * PANEL_PAD_X
}

/// Strengths of the neutral badge tint, picked by name.
const BADGE_TINTS: [f32; 3] = [0.14, 0.22, 0.30];

/// Badge of an app without an icon: ink 3 over the row at one of three strengths, picked by
/// name, so neighbouring rows part without borrowing a device or warning colour (those mean
/// something). The letter is in full ink. Returns the tint colour and its strength.
pub fn badge_tint(name: &str, pal: &Palette) -> (Color32, f32) {
    let h = name.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    (pal.ink3, BADGE_TINTS[h as usize % BADGE_TINTS.len()])
}

/// Strengths the badge tint can take (for the contrast checks).
pub fn badge_tint_strengths() -> [f32; 3] {
    BADGE_TINTS
}

fn fmt_gpu(v: Option<f32>) -> String {
    v.map(|v| format!("{v:.0} %")).unwrap_or_default()
}

fn fmt_bytes(v: Option<u64>) -> String {
    v.map(|v| format_value(Unit::Bytes, v as f64)).unwrap_or_default()
}

/// What one row shows in its columns.
struct Values<'a> {
    pid: String,
    cpu: f32,
    gpu: Option<f32>,
    vram: Option<u64>,
    mem: u64,
    show_pid: bool,
    disk: Option<f32>,
    disk_read: Option<f32>,
    disk_write: Option<f32>,
    enc: Option<f32>,
    dec: Option<f32>,
    threads: Option<u32>,
    user: &'a str,
    started: u64,
    exe: Option<&'a str>,
}

impl<'a> Values<'a> {
    fn empty() -> Values<'a> {
        Values {
            pid: String::new(),
            cpu: 0.0,
            gpu: None,
            vram: None,
            mem: 0,
            show_pid: false,
            disk: None,
            disk_read: None,
            disk_write: None,
            enc: None,
            dec: None,
            threads: None,
            user: "",
            started: 0,
            exe: None,
        }
    }
}

/// Header colour of each column: device colours for the measures, ink-3 for the identity.
pub fn column_color(key: SortKey, pal: &Palette) -> Color32 {
    match key {
        SortKey::Cpu => pal.cpu,
        SortKey::Gpu | SortKey::Vram | SortKey::Encode | SortKey::Decode => pal.gpu,
        SortKey::Mem => pal.mem,
        SortKey::Disk => pal.drive,
        SortKey::Name | SortKey::Pid | SortKey::Threads | SortKey::User | SortKey::Started | SortKey::Location => pal.ink3,
    }
}

/// Where the CPU and memory tints start on this machine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heavy {
    /// CPU share of the whole machine, in percent.
    pub cpu: f32,
    /// Private memory in bytes.
    pub mem: u64,
}

impl Heavy {
    /// From the number of logical CPUs and the total RAM: one thread running flat out tints
    /// (the CPU column is a share of the whole machine, so 16 threads make that 6.25 %), and so
    /// does a sixteenth of the RAM.
    pub fn new(logical_cpus: usize, ram_total: Option<u64>) -> Heavy {
        let n = logical_cpus.max(1) as f32;
        let mem = match ram_total {
            Some(t) if t > 0 => (t / HEAVY_MEM_SHARE).max(HEAVY_MEM_FLOOR),
            _ => HEAVY_MEM_FALLBACK,
        };
        Heavy { cpu: 100.0 / n * HEAVY_CPU_OF_ONE, mem }
    }

    /// From a snapshot: its per-thread readings count the logical CPUs (the per-core ones count
    /// physical cores, so they are the fallback), the summary holds the RAM.
    pub fn of_snapshot(snap: &hwglance_proto::Snapshot) -> Heavy {
        let (mut threads, mut cores) = (0usize, 0usize);
        for r in snap.sensors.iter().filter(|g| g.id == "cpu").flat_map(|g| &g.sections).flat_map(|s| &s.readings) {
            if r.id.starts_with("cpu/thread/") && r.id.ends_with("/usage") {
                threads += 1;
            } else if r.id.starts_with("cpu/usage/core") {
                cores += 1;
            }
        }
        let logical = match (threads, cores) {
            (0, 0) => std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
            (0, c) => c,
            (t, _) => t,
        };
        Heavy::new(logical, snap.summary.ram_total_bytes.map(|b| b as u64))
    }
}

impl Default for Heavy {
    /// Before the first snapshot: the thresholds of this machine's own CPU count.
    fn default() -> Heavy {
        Heavy::new(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1), None)
    }
}

/// The placeholder of the process panel while the first snapshot is awaited: the panel's frame,
/// a search field, the header band and quiet row bars, in the panel's own proportions.
pub fn skeleton_panel(p: &egui::Painter, rect: Rect, pal: &Palette) {
    p.rect(rect, 8.0, pal.panel, Stroke::new(1.0, pal.rule), egui::StrokeKind::Inside);
    let inner = rect.shrink2(vec2(PANEL_PAD_X, PANEL_PAD_Y));
    let toolbar = Rect::from_min_size(inner.min, vec2(inner.width(), toolbar_h()));
    let header = Rect::from_min_size(pos2(inner.left(), toolbar.bottom()), vec2(inner.width(), header_h()));
    let field = Rect::from_center_size(pos2(toolbar.left() + 8.0 + 120.0, toolbar.center().y), vec2(240.0, snap4(24.0 * crate::theme::font_scale())));
    p.rect_stroke(field.intersect(toolbar), 6.0, Stroke::new(1.0, pal.rule), egui::StrokeKind::Inside);
    p.rect_filled(header, 4.0, pal.header);
    let bar = |x: f32, cy: f32, w: f32, color: Color32| {
        if x + w <= inner.right() {
            p.rect_filled(Rect::from_min_size(pos2(x, cy - 4.0), vec2(w, 8.0)), 4.0, color);
        }
    };
    bar(inner.left() + 12.0, header.center().y, 48.0, pal.rule);
    for k in 0..4 {
        bar(inner.right() - 90.0 * (k as f32 + 1.0), header.center().y, 44.0, pal.rule);
    }
    let bottom = inner.bottom() - actions_h();
    let rows = (((bottom - header.bottom()) / row_h()).floor().max(0.0) as usize).min(14);
    for i in 0..rows {
        let r = Rect::from_min_size(pos2(inner.left(), header.bottom() + i as f32 * row_h()), vec2(inner.width(), row_h()));
        if i % 2 == 1 {
            p.rect_filled(r, 0.0, pal.zebra);
        }
        let cy = r.center().y;
        bar(inner.left() + 42.0, cy, 70.0 + ((i * 37) % 90) as f32, pal.rule);
        for k in 0..4 {
            bar(inner.right() - 90.0 * (k as f32 + 1.0), cy, 44.0, pal.rule);
        }
    }
    // End task and Force kill, as outlines
    for k in 0..2 {
        let b = Rect::from_min_size(pos2(inner.left() + k as f32 * 96.0, inner.bottom() - actions_h() * 0.5 - 12.0), vec2(88.0, 24.0));
        p.rect_stroke(b, 6.0, Stroke::new(1.0, pal.rule), egui::StrokeKind::Inside);
    }
}

/// Which cells of a row are heavy enough to be tinted: (cpu, gpu, memory). A class row (Apps,
/// Background, System) adds up many processes, so it never tints.
pub fn heavy_cells(heavy: &Heavy, class_row: bool, cpu: f32, gpu: Option<f32>, mem: u64) -> (bool, bool, bool) {
    if class_row {
        return (false, false, false);
    }
    (cpu >= heavy.cpu, gpu.unwrap_or(0.0) >= HEAVY_GPU, mem >= heavy.mem)
}

/// Is a Disk cell heavy enough to be tinted (read plus write from 50 MB/s)?
pub fn heavy_disk(total: Option<f32>) -> bool {
    total.unwrap_or(0.0) >= HEAVY_DISK
}

/// The search field takes this many characters; a pasted page of text stays out of every redraw.
pub const SEARCH_MAX_CHARS: usize = 200;

/// Message of an empty list. A long query is shortened to a readable length.
pub fn empty_message(filter: &str) -> String {
    let f = filter.trim();
    if f.is_empty() {
        return "No processes reported".to_string();
    }
    const SHOWN: usize = 60;
    if f.chars().count() > SHOWN {
        let cut: String = f.chars().take(SHOWN).collect();
        return format!("No process matches \"{}\u{2026}\"", cut.trim_end());
    }
    format!("No process matches \"{f}\"")
}

/// Header tooltip of a column, including how the smoothed values are made.
pub fn column_tip(key: SortKey, smoothing: f32) -> String {
    let smooth = if smoothing > 0.0 {
        format!(" Shown as a moving average over about {smoothing:.0} s, and the sort order uses the same smoothed value.")
    } else {
        " Not smoothed (see Settings, Processes).".to_string()
    };
    match key {
        SortKey::Name => "Name. Click to sort. Groups combine the processes of one app or service; expand a group to see them. Right-click the header to choose columns.".into(),
        SortKey::Pid => "Process id. Groups with several processes show no PID; expand them to see each one.".into(),
        SortKey::Cpu => format!("CPU time as a share of the whole machine (all logical CPUs), not of one core.{smooth}"),
        SortKey::Gpu => format!("NVIDIA shader (SM) time used by the process. A blank cell means the driver does not list the process (it is not using the GPU); 0 % means it is listed but did no work.{smooth}"),
        SortKey::Vram => "Video memory the process holds on the NVIDIA GPU.".into(),
        SortKey::Mem => "Private memory of the process (anonymous resident pages). Shared memory and file cache are not counted.".into(),
        SortKey::Disk => "Storage read plus write over the last interval. Linux only shows disk activity for your own processes, so other users' processes stay blank. Hover a cell for read and write separately.".into(),
        SortKey::Encode => "NVIDIA video encoder (NVENC) time used by the process.".into(),
        SortKey::Decode => "NVIDIA video decoder (NVDEC) time used by the process.".into(),
        SortKey::Threads => "Number of threads. Groups show the total of their processes.".into(),
        SortKey::User => "The user the process runs as. Groups show the user of their main process.".into(),
        SortKey::Started => "When the process started: the time for today, the date otherwise. Groups show their oldest process. Hover a cell for the full time.".into(),
        SortKey::Location => "The executable's path, with your home folder as ~. Groups show their main executable. Long paths are cut at the start so the file name stays visible.".into(),
    }
}

/// The checklist of optional columns (the header's right-click menu and Settings, Processes).
/// Returns the new saved list when a box was toggled.
pub fn column_checklist(ui: &mut Ui, saved: &[String]) -> Option<Vec<String>> {
    let shown = model::visible_columns(saved);
    let mut out = None;
    ui.label(RichText::new("Columns").strong());
    ui.add_enabled(false, egui::Checkbox::new(&mut true, "Name"));
    for key in SortKey::OPTIONAL {
        let mut on = shown.contains(&key);
        if ui.checkbox(&mut on, key.title()).changed() {
            out = Some(model::toggle_column(saved, key, on));
        }
    }
    out
}

/// The header's right-click menu: the column checklist, which stays open while boxes are ticked.
fn header_menu(resp: &egui::Response, rect: Rect, by_key: bool, saved: &[String], out: &mut Option<Vec<String>>) {
    let refocus = egui::Id::new(HEADER_ID);
    ui_kit::row_menu(resp, rect, by_key, refocus, |ui| {
        if let Some(c) = column_checklist(ui, saved) {
            *out = Some(c);
        }
    });
}

/// Height of the toolbar's icon buttons (the glyph's line plus the button padding), so the search
/// field beside them is no shorter than they are.
fn toolbar_control_h(ui: &Ui) -> f32 {
    let line = ui.ctx().fonts_mut(|f| f.layout_no_wrap("x".into(), ui_kit::f_icon(base() + 3.0), Color32::WHITE).size().y);
    (line + 2.0 * ui.spacing().button_padding.y).max(crate::theme::MIN_HIT).ceil()
}

/// Width the toolbar's buttons take on the right: expand, two dock buttons and Hide.
fn toolbar_buttons_width(ui: &Ui) -> f32 {
    let sp = ui.spacing().item_spacing.x;
    let icon_btn = crate::theme::snap4(base() + 3.0 + 2.0 * ui.spacing().button_padding.x) + 6.0;
    let hide = crate::theme::fs(78.0);
    3.0 * icon_btn + hide + 3.0 * sp
}

/// Does the toolbar lack the room for the title and a usable search field next to its buttons?
pub fn toolbar_is_narrow(toolbar_w: f32, buttons_w: f32, title_w: f32) -> bool {
    toolbar_w - buttons_w - 16.0 < title_w + 8.0 + 220.0
}

/// Short cell text of a column (Location is painted with its own start ellipsis).
fn cell_text(key: SortKey, v: &Values<'_>, now_ms: u64) -> String {
    match key {
        SortKey::Pid => {
            if v.show_pid { v.pid.clone() } else { String::new() }
        }
        SortKey::Cpu => format_value(Unit::Percent, v.cpu as f64),
        SortKey::Gpu => fmt_gpu(v.gpu),
        SortKey::Vram => fmt_bytes(v.vram),
        SortKey::Mem => format_value(Unit::Bytes, v.mem as f64),
        SortKey::Disk => v.disk.map(|d| format_value(Unit::BytesPerSec, d as f64)).unwrap_or_default(),
        SortKey::Encode => fmt_gpu(v.enc),
        SortKey::Decode => fmt_gpu(v.dec),
        SortKey::Threads => v.threads.map(|n| n.to_string()).unwrap_or_default(),
        SortKey::User => v.user.to_string(),
        SortKey::Started => ui_kit::fmt_started(v.started, now_ms),
        SortKey::Location | SortKey::Name => String::new(),
    }
}

/// The values of a row.
fn row_values<'a>(row: &model::Row, groups: &'a [ProcGroup]) -> Values<'a> {
    match &row.kind {
        RowKind::Section { .. } => {
            let class = section_class(&row.id);
            let (cpu, mem) = section_totals(groups, class);
            Values { cpu, mem, ..Values::empty() }
        }
        RowKind::Group { gi, .. } => {
            let g = &groups[*gi];
            let single = g.procs.len() == 1;
            Values {
                pid: if single { g.procs[0].pid.to_string() } else { String::new() },
                cpu: g.cpu_pct,
                gpu: g.gpu_pct,
                vram: g.vram_bytes,
                mem: g.mem_bytes,
                show_pid: single,
                disk: model::disk_total(g.disk_read_bps, g.disk_write_bps),
                disk_read: g.disk_read_bps,
                disk_write: g.disk_write_bps,
                enc: g.gpu_enc_pct,
                dec: g.gpu_dec_pct,
                threads: Some(model::group_threads(g)),
                user: model::group_user(g),
                started: model::group_started(g),
                exe: model::group_exe(g),
            }
        }
        RowKind::Proc { gi, pi } => {
            let pr = &groups[*gi].procs[*pi];
            Values {
                pid: pr.pid.to_string(),
                cpu: pr.cpu_pct,
                gpu: pr.gpu_pct,
                vram: pr.vram_bytes,
                mem: pr.mem_bytes,
                show_pid: true,
                disk: model::disk_total(pr.disk_read_bps, pr.disk_write_bps),
                disk_read: pr.disk_read_bps,
                disk_write: pr.disk_write_bps,
                enc: pr.gpu_enc_pct,
                dec: pr.gpu_dec_pct,
                threads: Some(pr.threads),
                user: &pr.user,
                started: pr.started_unix_ms,
                exe: pr.exe.as_deref(),
            }
        }
    }
}

/// Tooltip for the cell under the pointer, when that column has more to say.
fn cell_tip(key: SortKey, v: &Values<'_>, row: &model::Row, groups: &[ProcGroup]) -> Option<String> {
    match key {
        SortKey::Disk => {
            let f = |x: Option<f32>| x.map(|x| format_value(Unit::BytesPerSec, x as f64)).unwrap_or_else(|| "not shown".to_string());
            v.disk.map(|_| format!("Read {}\nWrite {}", f(v.disk_read), f(v.disk_write)))
        }
        SortKey::Started if v.started > 0 => Some(ui_kit::fmt_timestamp(v.started)),
        SortKey::Location => {
            let path = v.exe?.to_string();
            let cmd = match &row.kind {
                RowKind::Proc { gi, pi } => Some(groups[*gi].procs[*pi].cmdline.clone()),
                _ => None,
            };
            Some(match cmd {
                Some(c) => format!("{path}\n{c}"),
                None => path,
            })
        }
        _ => None,
    }
}

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// The action row's text: the target's name in full ink, bold, then its pid or process count;
/// after an End task or Force kill, how that went instead. `name_of` turns a name into what is
/// painted (right-to-left names in visual order, cut to the room there is).
fn action_text(
    pal: &Palette,
    selection: &Option<RowId>,
    target: &Option<model::Target>,
    watch: Option<(KillState, String, Option<String>)>,
    name_of: &dyn Fn(&str) -> String,
) -> egui::text::LayoutJob {
    let font = f_label();
    let bold = f_label_bold();
    let mut job = egui::text::LayoutJob::default();
    let mut add = |text: &str, strong: bool, color: Color32| {
        let f = if strong { bold.clone() } else { font.clone() };
        job.append(text, 0.0, egui::TextFormat::simple(f, color));
    };
    match (watch, target) {
        (Some((KillState::Failed, _, message)), _) => add(&message.unwrap_or_default(), false, pal.crit),
        (Some((KillState::Waiting, name, _)), _) => {
            add("Waiting for authentication to end ", false, pal.ink2);
            add(&name_of(&name), true, pal.ink);
        }
        (Some((KillState::Checking, name, _)), _) => {
            add("Ending ", false, pal.ink2);
            add(&name_of(&name), true, pal.ink);
            add("...", false, pal.ink2);
        }
        (Some((KillState::Ended, name, _)), _) => {
            add(&name_of(&name), true, pal.ink);
            add(" ended", false, pal.ink2);
        }
        (Some((KillState::StillRunning, name, _)), _) => {
            add(&name_of(&name), true, pal.ink);
            add(" is still running", false, pal.ink2);
        }
        (None, Some(t)) if t.procs.len() == 1 => {
            add(&name_of(&t.name), true, pal.ink);
            add(&format!("  (pid {})", t.procs[0].pid), false, pal.ink3);
        }
        (None, Some(t)) => {
            add(&name_of(&t.name), true, pal.ink);
            add(&format!("  ({} processes)", t.procs.len()), false, pal.ink3);
        }
        (None, None) => match selection {
            Some(RowId::Section(c)) => add(class_title(*c), false, pal.ink3),
            _ => add("Select a process or group", false, pal.ink3),
        },
    }
    job
}

/// The action row's text for `room` pixels: an Arabic or other right-to-left name is put in
/// visual order and, when the row is too narrow, cut on its logical text so the ellipsis closes
/// the end of the name (egui would cut the visual end). Other names are left to the label's own
/// truncation.
fn action_job(
    ui: &Ui,
    pal: &Palette,
    selection: &Option<RowId>,
    target: &Option<model::Target>,
    watch: Option<(KillState, String, Option<String>)>,
    room: f32,
) -> egui::text::LayoutJob {
    use std::hash::{Hash, Hasher};
    let width_of = |job: egui::text::LayoutJob| ui.painter().layout_job(job).size().x;
    // the row without its name says which sentence and font the cut is measured in
    let frame = action_text(pal, selection, target, watch.clone(), &|_| String::new()).text;
    let mut key = std::collections::hash_map::DefaultHasher::new();
    ((room * 4.0) as i32, &frame).hash(&mut key);
    let key = key.finish();
    let name_of = |name: &str| -> String {
        let fits = |t: &str| width_of(action_text(pal, selection, target, watch.clone(), &|_| t.to_string())) <= room;
        let shaped = crate::text::shape_fitted(name, key, &fits);
        // non-right-to-left text comes back as it is
        shaped.into_owned()
    };
    action_text(pal, selection, target, watch.clone(), &name_of)
}

impl App {
    /// Measure what the data columns `keys` need for the rows now in the table and let
    /// `col_fit` follow it. Done once per pass, whichever of the panel and its dock asks first.
    pub(crate) fn refresh_col_fit(&mut self, ui: &Ui, keys: &[SortKey]) {
        let pass = ui.ctx().cumulative_pass_nr();
        if !self.have_procs || self.col_fit_pass == pass {
            return;
        }
        self.col_fit_pass = pass;
        let now_ms = self.snap.as_ref().map(|s| s.unix_ms).unwrap_or(0);
        let p = ui.painter();
        let measure = |key: SortKey, text: &str| {
            let font = if left_aligned(key) { f_small() } else { f_mono_bold() };
            p.layout_no_wrap(crate::text::shape(text).into_owned(), font, Color32::WHITE).size().x
        };
        let extents = column_extents(keys, &self.rows, &self.groups, now_ms, measure);
        let wanted: Vec<(SortKey, f32)> = extents
            .into_iter()
            .map(|(key, cell_w)| {
                let header_w = p.layout_no_wrap(key.title().to_string(), f_label_bold(), Color32::WHITE).size().x;
                (key, column_want(header_w, cell_w))
            })
            .collect();
        if let Some(at) = self.col_fit.update(&wanted, ui.input(|i| i.time), crate::theme::font_scale()) {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((at - ui.input(|i| i.time)).max(0.05)));
        }
    }

    pub(crate) fn procs_panel(&mut self, ui: &mut Ui) {
        let pal = self.pal.clone();
        if !self.have_procs {
            return;
        }
        let groups_arc = self.groups.clone();
        let groups: &[ProcGroup] = &groups_arc;
        let rows = self.rows.clone();
        let panel = ui.max_rect();
        ui.painter().rect(panel, 8.0, pal.panel, Stroke::new(1.0, pal.rule), egui::StrokeKind::Inside);
        let inner = panel.shrink2(vec2(PANEL_PAD_X, PANEL_PAD_Y));
        let dock = self.s.layout.dock();

        let cw = mono_char_w(ui);
        let view = self.s.layout.view();
        let name_cap = match (dock, view) {
            (Dock::Bottom, _) => crate::theme::fs(NAME_CAP),
            // the compact right panel is narrow: no cap, so Name fills what the few columns leave
            // until the divider sets it
            (Dock::Right, PanelView::Compact) => f32::INFINITY,
            (Dock::Right, PanelView::Expanded) => NAME_CAP_RIGHT * inner.width(),
        };
        let view_prefs = self.s.current_view().clone();
        let keys = model::visible_columns(&view_prefs.columns);
        self.refresh_col_fit(ui, &keys);
        let mut cols = Cols::new(keys, cw, name_cap).with_fit(&self.col_fit).for_view(view).with_name_width(view_prefs.name_width);
        if dock == Dock::Right {
            // a wider panel always widens Name; the divider sets the data columns' extra width
            cols = cols.right_dock(view_prefs.data_extra);
        }
        let mut acts: Vec<Act> = Vec::new();
        let mut sort_click: Option<SortKey> = None;
        let mut new_columns: Option<Vec<String>> = None;
        let mut new_dock: Option<Dock> = None;
        let mut toggle_view = false;

        let toolbar = Rect::from_min_size(inner.min, vec2(inner.width(), toolbar_h()));
        let header = Rect::from_min_size(pos2(inner.left(), toolbar.bottom()), vec2(inner.width(), header_h()));
        let actions = Rect::from_min_max(pos2(inner.left(), inner.bottom() - actions_h()), inner.max);
        let list = Rect::from_min_max(pos2(inner.left(), header.bottom()), pos2(inner.right(), actions.top() - 2.0));

        // The pointer over the list holds the row order (see `rebuild_rows`).
        self.procs_list_rect = list;
        self.procs_hovered = ui.ctx().pointer_hover_pos().is_some_and(|p| list.contains(p));
        let held = self.order_held_by_hover();

        // ---- toolbar: title, search, view, dock and hide ----
        // The buttons on the right always stay; what is left of the row goes to the title and the
        // search field, which shrinks to an icon in a narrow panel and opens on a click or "/".
        let buttons_w = toolbar_buttons_width(ui);
        let narrow = toolbar_is_narrow(toolbar.width(), buttons_w, crate::theme::fs(80.0));
        ui.scope_builder(UiBuilder::new().max_rect(toolbar), |ui| {
            ui.horizontal_centered(|ui| {
                let left_w = (toolbar.width() - buttons_w - 16.0).max(40.0);
                if narrow && self.focus_search {
                    self.search_open = true;
                }
                let search_shown = !narrow || self.search_open || !self.filter.is_empty();
                if !(narrow && search_shown) {
                    ui.label(RichText::new("Processes").font(f_section()).color(pal.ink2));
                    ui.add_space(8.0);
                }
                if search_shown {
                    let width = if narrow { left_w - 8.0 } else { crate::theme::fs(if dock == Dock::Right { 240.0 } else { 340.0 }).min(left_w - 80.0) };
                    let te = egui::TextEdit::singleline(&mut self.filter)
                        .id(egui::Id::new(SEARCH_ID))
                        .hint_text(if narrow { "Search  ( / )" } else { "Search name, command or PID  ( / )" })
                        .desired_width(width.max(60.0))
                        .char_limit(SEARCH_MAX_CHARS)
                        .margin(vec2(8.0, 4.0))
                        .min_size(vec2(0.0, toolbar_control_h(ui)));
                    let resp = ui.add(te);
                    a11y::publish(&resp, || Spoken::new(egui::accesskit::Role::SearchInput, "Search processes").described("Search by name, command or PID. Down or Enter moves to the list."));
                    if self.focus_search {
                        resp.request_focus();
                        self.focus_search = false;
                    }
                    if narrow && self.search_open && resp.lost_focus() && self.filter.is_empty() {
                        self.search_open = false;
                    }
                    if resp.changed() {
                        self.rows_dirty = true;
                    }
                    if !self.filter.is_empty() && !narrow && ui.button("Clear").clicked() {
                        self.filter.clear();
                        self.rows_dirty = true;
                    }
                } else {
                    let b = ui.button(RichText::new(icon::SEARCH.to_string()).font(ui_kit::f_icon(base() + 2.0)).color(pal.ink2));
                    a11y::publish(&b, || Spoken::button("Search processes").described("Opens the search field. The slash key does too."));
                    if b.on_hover_text("Search processes ( / )").clicked() {
                        self.search_open = true;
                        self.focus_search = true;
                    }
                }
                ui.add_space(8.0);
                let (n_groups, n_procs) = rows.iter().fold((0usize, 0usize), |(g, p), r| match r.kind {
                    RowKind::Section { groups, procs, .. } => (g + groups, p + procs),
                    _ => (g, p),
                });
                if !self.filter.trim().is_empty() && dock == Dock::Bottom {
                    ui.label(RichText::new(format!("{n_groups} groups, {n_procs} processes match")).color(pal.ink3));
                }
                // left to right, so Tab goes expand, dock below, dock right, Hide
                ui_kit::right_cluster(ui, |ui| {
                    // the other view of this dock: expand when compact, collapse when expanded
                    let (glyph, name, tip) = match view {
                        PanelView::Compact => (icon::ARROW_EXPAND, "Expand the process panel", "Expand the process panel: more columns, more room (Ctrl+E)"),
                        PanelView::Expanded => (icon::ARROW_COLLAPSE, "Collapse the process panel", "Collapse the process panel to its compact view (Ctrl+E)"),
                    };
                    let b = egui::Button::new(RichText::new(glyph.to_string()).font(ui_kit::f_icon(base() + 3.0)).color(pal.ink2));
                    let r = ui.add(b);
                    a11y::publish(&r, || Spoken::button(name).described("Shortcut Ctrl+E"));
                    if r.on_hover_text(tip).clicked() {
                        toggle_view = true;
                    }
                    // two small icon buttons: dock below (the current one is pressed) or at the right
                    for (target, glyph, name, tip) in [
                        (Dock::Bottom, icon::DOCK_BOTTOM, "Dock the process panel below the sensors", "Dock the process panel below the sensors"),
                        (Dock::Right, icon::DOCK_RIGHT, "Dock the process panel at the right", "Dock the process panel at the right"),
                    ] {
                        let b = egui::Button::new(RichText::new(glyph.to_string()).font(ui_kit::f_icon(base() + 3.0)).color(if dock == target { pal.ink } else { pal.ink3 }))
                            .selected(dock == target);
                        let r = ui.add(b);
                        a11y::publish(&r, || Spoken::button(name).selected(dock == target));
                        if r.on_hover_text(tip).clicked() && dock != target {
                            new_dock = Some(target);
                        }
                    }
                    let tip = "Hide the process panel";
                    let r = ui.button(RichText::new(format!("{}  Hide", icon::EYE_OFF)));
                    a11y::publish(&r, || Spoken::button(tip));
                    if r.on_hover_text(tip).clicked() {
                        self.set_procs_visible(false);
                    }
                });
            });
        });

        // ---- table: header and list, scrolling sideways only when the columns need it ----
        let region = Rect::from_min_max(header.min, list.max);
        // the list's own vertical scrollbar takes room from the columns
        let bar = ui.spacing().scroll.allocated_width();
        // Name may give up a few pixels below its minimum before a sideways scrollbar appears,
        // so a table that is only just too wide (the compact right dock) never scrolls.
        const NAME_SQUEEZE: f32 = 24.0;
        if cols.content_width() + bar > region.width() + NAME_SQUEEZE {
            let need = cols.content_width() + bar;
            ui.scope_builder(UiBuilder::new().max_rect(region), |ui| {
                egui::ScrollArea::horizontal().id_salt("procs-hscroll").auto_shrink([false, false]).show(ui, |ui| {
                    // a window squeezed below its minimum can leave less than a header's height
                    let h = (region.height() - ui.spacing().scroll.allocated_width()).max(header_h());
                    let (r, _) = ui.allocate_exact_size(vec2(need, h), Sense::hover());
                    let hrect = Rect::from_min_size(r.min, vec2(need, header_h()));
                    let lrect = Rect::from_min_max(pos2(r.left(), hrect.bottom()), r.max);
                    self.draw_table(ui, hrect, lrect, &cols, &rows, groups, held, &mut acts, &mut sort_click, &mut new_columns);
                });
            });
        } else {
            self.draw_table(ui, header, list, &cols, &rows, groups, held, &mut acts, &mut sort_click, &mut new_columns);
        }

        // ---- action bar ----
        self.actions_rect = Some(actions);
        ui.scope_builder(UiBuilder::new().max_rect(actions), |ui| {
            ui.horizontal_centered(|ui| {
                let target = self.selected_target();
                let enabled = target.is_some();
                // what the last End task or Force kill came to, while this row is still selected
                let watch = self
                    .kill_watch
                    .as_ref()
                    .filter(|w| self.selection.as_ref() == Some(&w.row))
                    .map(|w| (w.state, w.name.clone(), w.message.clone()));
                let still_running = matches!(watch, Some((KillState::StillRunning, ..)));
                let end = ui.add_enabled(enabled, egui::Button::new("End task")).on_hover_text("Ask the process to exit (Delete)");
                if end.clicked() {
                    acts.push(Act::Kill(self.selection.clone().unwrap(), Signal::Term));
                }
                let kill_color = if enabled { pal.crit } else { pal.ink3 };
                let kill_btn = if still_running {
                    // it ignored the polite request: this is the next step, so it is filled
                    let (fill, ink) = pal.crit_button();
                    egui::Button::new(RichText::new("Force kill").color(ink).strong()).fill(fill)
                } else {
                    egui::Button::new(RichText::new("Force kill").color(kill_color)).stroke(Stroke::new(1.0, kill_color)).fill(Color32::TRANSPARENT)
                };
                let kill = ui.add_enabled(enabled, kill_btn).on_hover_text("Stop the process at once, after a confirmation (Shift+Delete)");
                if kill.clicked() {
                    acts.push(Act::Kill(self.selection.clone().unwrap(), Signal::Kill));
                }
                ui.add_space(12.0);
                let job = action_job(ui, &pal, &self.selection, &target, watch, ui.available_width());
                // a narrow panel shortens the text with an ellipsis instead of clipping it
                ui.add(egui::Label::new(job).truncate());
            });
        });

        // ---- apply ----
        if let Some(k) = sort_click {
            self.set_sort(k);
        }
        if let Some(c) = new_columns {
            self.s.current_view_mut().columns = c;
        }
        if let Some(d) = new_dock {
            self.s.layout.set_dock(d);
        }
        if toggle_view {
            self.s.toggle_panel_view();
        }
        for a in acts {
            match a {
                Act::Select(id) => {
                    self.selection = Some(id);
                }
                Act::Toggle(id) => {
                    self.selection = Some(id.clone());
                    self.toggle_row(&id);
                }
                Act::Kill(id, sig) => {
                    self.selection = Some(id.clone());
                    self.request_kill(Some(id), sig);
                }
                Act::CopyPid(id) => {
                    if let Some(t) = model::target_for(groups, &id) {
                        let s = t.procs.iter().map(|p| p.pid.to_string()).collect::<Vec<_>>().join(" ");
                        ui.ctx().copy_text(s);
                        self.set_toast("PID copied".into(), true);
                    }
                }
                Act::CopyCmd(id) => {
                    if let Some(t) = model::target_for(groups, &id) {
                        let s = t.procs.iter().map(|p| p.cmdline.clone()).collect::<Vec<_>>().join("\n");
                        ui.ctx().copy_text(s);
                        self.set_toast("Command line copied".into(), true);
                    }
                }
                Act::OpenLocation(id) => match model::row_exe(groups, &id).and_then(model::parent_dir) {
                    Some(dir) => match crate::actions::open_folder(dir) {
                        Ok(()) => self.set_toast(format!("Opened {dir}"), true),
                        Err(e) => self.set_toast(format!("Could not open {dir}: {e}"), false),
                    },
                    None => self.set_toast("No file location known for this process".into(), false),
                },
                Act::CopyPath(id) => {
                    if let Some(p) = model::row_exe(groups, &id) {
                        ui.ctx().copy_text(p.to_string());
                        self.set_toast("Path copied".into(), true);
                    }
                }
            }
        }
    }

    /// The sortable header and the virtualised list in the given rects.
    #[allow(clippy::too_many_arguments)]
    fn draw_table(
        &mut self,
        ui: &mut Ui,
        header: Rect,
        list: Rect,
        cols: &Cols,
        rows: &std::sync::Arc<Vec<model::Row>>,
        groups: &[ProcGroup],
        held: bool,
        acts: &mut Vec<Act>,
        sort_click: &mut Option<SortKey>,
        new_columns: &mut Option<Vec<String>>,
    ) {
        let pal = self.pal.clone();
        // ---- header ----
        let smoothing = self.s.processes.smoothing_seconds;
        // x of the Name divider while it is dragged: a faint guide runs down the list from it
        let mut guide: Option<f32> = None;
        ui.scope_builder(UiBuilder::new().max_rect(header), |ui| {
            let hw = (list.width() - ui.spacing().scroll.allocated_width()).max(100.0);
            let hrect = Rect::from_min_size(header.min, vec2(hw, header_h()));
            ui.painter().rect_filled(hrect, 6.0, pal.header);
            // The header is one Tab stop: Left and Right choose a column, Enter or Space sorts by it,
            // Shift+F10 opens the column checklist (a right click anywhere on the header does too).
            let bg = ui.interact(hrect, egui::Id::new(HEADER_ID), Sense::click());
            let (name_r, col_r) = cols.rects(hrect);
            let mut cells: Vec<(SortKey, Rect, bool)> = vec![(SortKey::Name, name_r.shrink2(vec2(12.0, 0.0)), false)];
            cells.extend(col_r.iter().map(|(k, r)| (*k, *r, !left_aligned(*k))));
            if !cells.iter().any(|c| c.0 == self.hdr_cursor) {
                self.hdr_cursor = SortKey::Name;
            }
            if bg.has_focus() {
                let step = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) as isize - i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) as isize);
                if step != 0 {
                    let at = cells.iter().position(|c| c.0 == self.hdr_cursor).unwrap_or(0) as isize;
                    self.hdr_cursor = cells[(at + step).clamp(0, cells.len() as isize - 1) as usize].0;
                }
                if bg.clicked() {
                    *sort_click = Some(self.hdr_cursor);
                }
            }
            a11y::keep_keys(&bg, false, true, false);
            a11y::scroll_into_view(ui, &bg);
            let cursor_title = self.hdr_cursor.title();
            a11y::publish(&bg, || Spoken::button(format!("Sort the process table by {cursor_title}")).described("Left and right choose another column, Enter sorts by it").with_menu());
            let by_key = bg.has_focus() && a11y::menu_key(ui.ctx());
            header_menu(&bg, hrect, by_key, &self.s.current_view().columns, new_columns);
            let sort_key = self.sort_key();
            for (key, r, right) in cells.iter().copied() {
                // a cell is a pointer target; the header as a whole is the Tab stop
                let resp = ui.interact(r, egui::Id::new(("proc-hdr", key.as_str())), Sense::CLICK);
                a11y::publish(&resp, || {
                    let s = Spoken::new(egui::accesskit::Role::ColumnHeader, key.title()).described("Click to sort by this column");
                    if sort_key == key { s.sorted(self.s.processes.sort.descending) } else { s }
                });
                let active = sort_key == key;
                let color = column_color(key, &pal);
                let text_color = if active || resp.hovered() { color } else { color.gamma_multiply(HEADER_REST_ALPHA) };
                let font = if active { f_label_bold() } else { f_label() };
                let (anchor, x) = if right {
                    (Align2::RIGHT_CENTER, r.right() - 10.0)
                } else if key == SortKey::Name {
                    (Align2::LEFT_CENTER, r.left())
                } else {
                    (Align2::LEFT_CENTER, r.left() + 4.0)
                };
                let text_rect = ui.painter().with_clip_rect(r).text(pos2(x, r.center().y), anchor, key.title(), font, text_color);
                if active {
                    let cx = if right { text_rect.left() - 10.0 } else { text_rect.right() + 10.0 };
                    arrow(ui.painter(), pos2(cx, r.center().y), self.s.processes.sort.descending, color);
                }
                if key == SortKey::Name && held {
                    // "order held": a quiet chip with a pause glyph
                    let hint_x = text_rect.right() + 36.0;
                    let g = ui.painter().layout_no_wrap("order held".to_string(), f_small(), pal.ink3);
                    let chip = Rect::from_min_size(pos2(hint_x, r.center().y - 10.0), vec2(g.size().x + 30.0, 20.0));
                    if chip.right() < r.right() + 60.0 {
                        ui.painter().rect(chip, 10.0, Color32::TRANSPARENT, Stroke::new(1.0, pal.rule), egui::StrokeKind::Inside);
                        ui.painter().text(pos2(chip.left() + 11.0, chip.center().y), Align2::CENTER_CENTER, icon::PAUSE, ui_kit::f_icon(base() - 1.0), pal.ink3);
                        ui.painter().galley(pos2(chip.left() + 22.0, chip.center().y - g.size().y / 2.0), g, pal.ink3);
                        let hint = ui.interact(chip, egui::Id::new("order-held-hint"), Sense::hover());
                        hint.on_hover_text("The pointer is over the list, so rows stay where they are while their values keep updating. Move the pointer away to re-sort.");
                    }
                }
                header_menu(&resp, r, false, &self.s.current_view().columns, new_columns);
                let resp = resp.on_hover_text(column_tip(key, smoothing));
                if resp.clicked() {
                    *sort_click = Some(key);
                }
            }

            if bg.has_focus()
                && let Some((_, r, _)) = cells.iter().find(|c| c.0 == self.hdr_cursor)
            {
                let r = if self.hdr_cursor == SortKey::Name { name_r } else { *r };
                a11y::paint_ring(ui.painter(), r, 6.0, Ring::Inside, pal.ink);
            }

            // ---- the divider at the right edge of Name; drawn last so it wins its 24 px ----
            let div_x = name_r.right() + 4.0;
            self.name_divider_x = div_x;
            let hit = Rect::from_min_max(pos2(div_x - DIVIDER_HIT_HALF, hrect.top()), pos2(div_x + DIVIDER_HIT_HALF, hrect.bottom()));
            let div = ui.interact(hit, egui::Id::new("proc-name-divider"), Sense::click_and_drag());
            let name_px = name_r.width() + 4.0;
            let home_does = if cols.is_right_dock() { "gives the data columns their fitted widths" } else { "restores the automatic width" };
            a11y::publish(&div, || {
                Spoken::new(egui::accesskit::Role::Splitter, "Resize the Name column")
                    .described(format!("Left and Right change the width by 4 pixels, with Shift by 32. Home {home_does}. Double-click fits the names."))
                    .valued(format!("{} pixels wide", name_px.round()))
            });
            let key = ui_kit::divider_key(&div, true);
            header_menu(&div, hit, false, &self.s.current_view().columns, new_columns);
            if div.hovered() || div.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            let name_now = name_r.width() + 4.0;
            let bounds = cols.name_bounds(hrect.width());
            // The right dock keeps the panel's size and Name takes what the data columns leave, so
            // the divider sets their extra width: Name's width is `auto` less that.
            let right = cols.is_right_dock();
            let auto = cols.name_auto(hrect.width());
            let extra_max = cols.data_extra_max(hrect.width());
            let set_name = |view: &mut crate::settings::ViewPrefs, name: f32| {
                if right {
                    view.data_extra = Some((auto - name).clamp(0.0, extra_max));
                } else {
                    view.name_width = Some(name.clamp(bounds.0, bounds.1));
                }
            };
            let drag_id = egui::Id::new("proc-name-drag");
            if div.drag_started() {
                ui.data_mut(|d| d.insert_temp(drag_id, name_now));
            }
            if div.dragged() {
                guide = Some(div_x);
                let dx = ui_kit::drag_delta(ui, &div).x;
                // the wanted width is kept unclamped while dragging, so a pointer that went
                // past a limit has to come back to it before the column moves again
                let want = ui.data_mut(|d| {
                    let w = d.get_temp_mut_or(drag_id, name_now);
                    *w += dx;
                    *w
                });
                set_name(self.s.current_view_mut(), want);
            }
            // the keyboard's way to the same two things: a step wider or narrower, and back to automatic
            match key {
                Some(ui_kit::DividerKey::Move(d)) => set_name(self.s.current_view_mut(), name_now + d),
                Some(ui_kit::DividerKey::Reset) => {
                    let view = self.s.current_view_mut();
                    view.name_width = None;
                    view.data_extra = None;
                }
                None => {}
            }
            if div.double_clicked() {
                let measure = |t: &str, bold: bool| {
                    let font = if bold { f_label_bold() } else { f_label() };
                    ui.painter().layout_no_wrap(crate::text::shape(t).into_owned(), font, Color32::WHITE).size().x
                };
                let extent = widest_name_extent(rows, groups, measure);
                let most = if right { auto.max(bounds.0) } else { bounds.1 };
                set_name(self.s.current_view_mut(), fit_name_width(extent, (bounds.0, most)));
            }
            let active = div.hovered() || div.dragged() || div.has_focus();
            if div.has_focus() {
                let ring = Rect::from_min_max(pos2(div_x - 4.0, hrect.top() + 2.0), pos2(div_x + 4.0, hrect.bottom() - 2.0));
                a11y::paint_ring(ui.painter(), ring, 3.0, Ring::Inside, pal.ink);
            }
            let (color, w) = if active { (pal.ink2, 2.0) } else { (pal.rule, 1.0) };
            ui.painter().line_segment([pos2(div_x, hrect.top() + 6.0), pos2(div_x, hrect.bottom() - 6.0)], Stroke::new(w, color));
            div.on_hover_text("Drag to resize the Name column. Double-click to fit the names.");
        });

        // ---- list ----
        let selected_idx = self.selection.as_ref().and_then(|id| model::row_index(rows, id));
        let home = home_dir();
        // The list is one Tab stop; the rows are not. Up and Down (handled in `handle_keys`) move
        // the selection while it has the keyboard, and Shift+F10 opens the selected row's menu.
        let list_stop = ui.interact(list, egui::Id::new(LIST_ID), Sense::focusable_noninteractive());
        a11y::keep_keys(&list_stop, true, true, false);
        if list_stop.gained_focus() && self.selection.is_none() && !rows.is_empty() {
            self.move_selection(1);
        }
        let list_focused = list_stop.has_focus();
        let menu_for_selection = list_focused && a11y::menu_key(ui.ctx());
        a11y::publish(&list_stop, || {
            Spoken::new(egui::accesskit::Role::List, "Processes")
                .described("Up and down move the selection, Delete ends the task, Shift+F10 opens its menu")
                .valued(format!("{} rows", rows.len()))
        });
        ui.scope_builder(UiBuilder::new().max_rect(list), |ui| {
            ui.set_clip_rect(list.intersect(ui.clip_rect()));
            if rows.is_empty() {
                ui.add_space(24.0);
                let msg = empty_message(&self.filter);
                ui.vertical_centered(|ui| ui.label(RichText::new(msg).color(pal.ink3)));
                return;
            }
            ui.spacing_mut().item_spacing.y = 0.0;
            let rh = row_h();
            let mut area = egui::ScrollArea::vertical().id_salt("procs-scroll").auto_shrink([false, false]);
            if self.scroll_to_selection {
                self.scroll_to_selection = false;
                if let Some(i) = selected_idx {
                    let (top, bottom) = (i as f32 * rh, (i + 1) as f32 * rh);
                    let (off, vh) = (self.scroll_offset, self.viewport_h);
                    if top < off {
                        area = area.vertical_scroll_offset(top);
                    } else if bottom > off + vh {
                        area = area.vertical_scroll_offset(bottom - vh);
                    }
                }
            }
            let now_ms = self.snap.as_ref().map(|s| s.unix_ms).unwrap_or(0);
            let out = area.show_rows(ui, rh, rows.len(), |ui, range| {
                for idx in range {
                    let row = &rows[idx];
                    // the same width as the header: the list's own scrollbar is kept out of the columns
                    let w = list.width() - ui.spacing().scroll.allocated_width();
                    let (_, rect) = ui.allocate_space(vec2(w, rh));
                    // not a Tab stop (the list is), but a click target that is named
                    let resp = ui.interact(rect, egui::Id::new(("proc-row", &row.id)), Sense::CLICK);
                    let selected = self.selection.as_ref() == Some(&row.id);
                    let v = row_values(row, groups);
                    if ui.is_rect_visible(rect) {
                        self.paint_row(ui, rect, idx, row, &v, cols, selected, resp.hovered(), now_ms, &home);
                        if selected && list_focused {
                            a11y::paint_ring(ui.painter(), rect, 4.0, Ring::Inside, pal.ink);
                        }
                    }
                    a11y::publish(&resp, || row_spoken(row, groups, &v, selected));
                    row_interaction(&resp, rect, row, groups, &v, cols, acts, selected && menu_for_selection);
                }
            });
            self.scroll_offset = out.state.offset.y;
            self.viewport_h = out.inner_rect.height();
        });
        if let Some(x) = guide {
            ui.painter().vline(x, list.y_range(), Stroke::new(1.0, pal.ink3.gamma_multiply(0.45)));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_row(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        idx: usize,
        row: &model::Row,
        v: &Values<'_>,
        cols: &Cols,
        selected: bool,
        hovered: bool,
        now_ms: u64,
        home: &str,
    ) {
        let pal = self.pal.clone();
        let groups = self.groups.clone();
        let (name_r, col_r) = cols.rects(rect);
        let is_section = matches!(row.kind, RowKind::Section { .. });
        if selected {
            // Every ink must stay readable on the fill, which caps how far it can part from the
            // hover and zebra rows; the outline and the bar carry the rest.
            ui.painter().rect(rect, 4.0, pal.selected, Stroke::new(1.0, pal.ink3.gamma_multiply(0.55)), egui::StrokeKind::Inside);
            // a 3 px bar in full ink at the left edge
            let bar = Rect::from_min_size(rect.left_top(), vec2(SELECTED_BAR_W, rect.height()));
            ui.painter().rect_filled(bar, egui::CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 }, pal.ink);
        } else if hovered {
            ui.painter().rect_filled(rect, 4.0, pal.hover);
        } else if is_section {
            ui.painter().rect_filled(rect, 4.0, pal.header);
        } else if idx % 2 == 1 {
            ui.painter().rect_filled(rect, 0.0, pal.zebra);
        }

        let name_font = f_label();
        let mono = f_mono();
        let badge = snap4(20.0 * crate::theme::font_scale());
        let cy = rect.center().y;
        match &row.kind {
            RowKind::Section { collapsed, groups: ng, procs: np } => {
                let class = section_class(&row.id);
                // its caption gives way to the numbers on the right in a narrow table: the
                // counts go first, then the title ends in an ellipsis
                let p = ui.painter().with_clip_rect(name_r);
                let g = if *collapsed { icon::CHEVRON_RIGHT } else { icon::CHEVRON_DOWN };
                p.text(pos2(rect.left() + 16.0, cy), Align2::CENTER_CENTER, g, ui_kit::f_icon(base() + 2.0), pal.ink3);
                let (sx, edge) = (rect.left() + 32.0, name_r.right() - 4.0);
                let t = ellipsized(&p, class_title(class), f_section(), pal.ink2, edge - sx);
                let cx = sx + t.size().x + 8.0;
                p.galley(pos2(sx, cy - t.size().y / 2.0), t, pal.ink2);
                if edge - cx > 40.0 {
                    let gal = ellipsized(&p, &format!("{ng} groups, {np} processes"), f_small(), pal.ink3, edge - cx);
                    p.galley(pos2(cx, cy - gal.size().y / 2.0), gal, pal.ink3);
                }
            }
            RowKind::Group { gi, expanded, expandable } => {
                let g = &groups[*gi];
                let p = ui.painter().with_clip_rect(name_r);
                if *expandable {
                    let c = if *expanded { icon::CHEVRON_DOWN } else { icon::CHEVRON_RIGHT };
                    p.text(pos2(rect.left() + 16.0, cy), Align2::CENTER_CENTER, c, ui_kit::f_icon(base() + 2.0), pal.ink3);
                }
                let icon_c = pos2(rect.left() + 42.0, cy);
                let ctx = ui.ctx().clone();
                let tex = g.icon.as_deref().and_then(|n| self.icons.get(&ctx, n));
                let r = Rect::from_center_size(icon_c, vec2(badge, badge));
                if let Some((tex, tone)) = tex {
                    // a white glyph vanishes on a light row and a black one on a dark row
                    let plate = match tone {
                        crate::icons::Tone::LightGlyph if !pal.is_dark() => Some(pal.ink2),
                        crate::icons::Tone::DarkGlyph if pal.is_dark() => Some(pal.ink2),
                        _ => None,
                    };
                    if let Some(c) = plate {
                        p.rect_filled(r.expand(2.0), 5.0, c);
                    }
                    p.image(tex, r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                } else {
                    let (c, strength) = badge_tint(&g.name, &pal);
                    p.rect_filled(r, 5.0, c.gamma_multiply(strength));
                    let letter = pal.ink;
                    let initial = g.name.chars().find(|c| c.is_alphanumeric()).unwrap_or('?').to_uppercase().to_string();
                    p.text(r.center(), Align2::CENTER_CENTER, initial, FontId::new(base() - 2.5, fonts::bold()), letter);
                }
                // In a narrow table the name ends in an ellipsis and its process count shows
                // only when it fits whole.
                let (nx, edge) = (rect.left() + NAME_TEXT_X, name_r.right() - 4.0);
                let t = ellipsized(&p, &g.name, f_label_bold(), pal.ink, edge - nx);
                let mut x = nx + t.size().x;
                p.galley(pos2(nx, cy - t.size().y / 2.0), t, pal.ink);
                if g.procs.len() > 1 {
                    let c = p.layout_no_wrap(format!("({})", g.procs.len()), name_font.clone(), pal.ink3);
                    let w = c.size().x;
                    if x + 6.0 + w <= edge {
                        p.galley(pos2(x + 6.0, cy - c.size().y / 2.0), c, pal.ink3);
                        x += 6.0 + w;
                    }
                }
                if let Some(title) = &g.title {
                    // Window titles give way to the name column's edge with an ellipsis.
                    let tx = x + 12.0;
                    let room = name_r.right() - 8.0 - tx;
                    if room > 40.0 {
                        let gal = ellipsized(&p, title, f_small(), pal.ink3, room);
                        p.galley(pos2(tx, cy - gal.size().y / 2.0), gal, pal.ink3);
                    }
                }
            }
            RowKind::Proc { gi, pi } => {
                let pr = &groups[*gi].procs[*pi];
                let p = ui.painter().with_clip_rect(name_r);
                let x = rect.left() + 42.0;
                let guide = Stroke::new(1.0, pal.rule);
                p.line_segment([pos2(x, rect.top()), pos2(x, cy)], guide);
                p.line_segment([pos2(x, cy), pos2(x + 12.0, cy)], guide);
                let nx = rect.left() + NAME_TEXT_X;
                let t = ellipsized(&p, &pr.name, name_font, pal.ink2, name_r.right() - 4.0 - nx);
                p.galley(pos2(nx, cy - t.size().y / 2.0), t, pal.ink2);
            }
        }
        let p = ui.painter();
        let strong = matches!(row.kind, RowKind::Group { .. } | RowKind::Section { .. });
        let num_font = if strong { f_mono_bold() } else { mono.clone() };
        let base_color = if strong { pal.ink } else { pal.ink2 };
        let dim = pal.ink3;
        let tint = |p: &egui::Painter, cell: Rect, c: Color32| p.rect_filled(cell.shrink2(vec2(2.0, 2.0)), 4.0, c.gamma_multiply(0.16));
        let (heavy_cpu, heavy_gpu, heavy_mem) = heavy_cells(&self.heavy, is_section, v.cpu, v.gpu, v.mem);
        for (key, cell) in &col_r {
            let right = |r: &Rect| pos2(r.right() - 10.0, r.center().y);
            let clipped = p.with_clip_rect(*cell);
            match key {
                SortKey::Pid => {
                    if v.show_pid {
                        p.text(right(cell), Align2::RIGHT_CENTER, &v.pid, mono.clone(), dim);
                    }
                }
                SortKey::Cpu => {
                    if heavy_cpu {
                        tint(p, *cell, pal.cpu);
                    }
                    let c = if v.cpu < 0.05 { dim } else { base_color };
                    p.text(right(cell), Align2::RIGHT_CENTER, cell_text(*key, v, now_ms), num_font.clone(), c);
                }
                SortKey::Gpu => {
                    if !is_section {
                        if heavy_gpu {
                            tint(p, *cell, pal.gpu);
                        }
                        let c = if v.gpu.unwrap_or(0.0) > 0.5 { base_color } else { dim };
                        p.text(right(cell), Align2::RIGHT_CENTER, cell_text(*key, v, now_ms), num_font.clone(), c);
                    }
                }
                SortKey::Vram => {
                    if !is_section {
                        p.text(right(cell), Align2::RIGHT_CENTER, cell_text(*key, v, now_ms), num_font.clone(), base_color);
                    }
                }
                SortKey::Mem => {
                    if heavy_mem {
                        tint(p, *cell, pal.mem);
                    }
                    p.text(right(cell), Align2::RIGHT_CENTER, cell_text(*key, v, now_ms), num_font.clone(), base_color);
                }
                SortKey::Disk => {
                    if !is_section {
                        if heavy_disk(v.disk) {
                            tint(p, *cell, pal.drive);
                        }
                        let c = if v.disk.unwrap_or(0.0) < 1e3 { dim } else { base_color };
                        p.text(right(cell), Align2::RIGHT_CENTER, cell_text(*key, v, now_ms), num_font.clone(), c);
                    }
                }
                SortKey::Encode | SortKey::Decode => {
                    if !is_section {
                        let val = if *key == SortKey::Encode { v.enc } else { v.dec };
                        let c = if val.unwrap_or(0.0) > 0.5 { base_color } else { dim };
                        p.text(right(cell), Align2::RIGHT_CENTER, cell_text(*key, v, now_ms), num_font.clone(), c);
                    }
                }
                SortKey::Threads => {
                    if !is_section {
                        p.text(right(cell), Align2::RIGHT_CENTER, cell_text(*key, v, now_ms), mono.clone(), dim);
                    }
                }
                SortKey::User | SortKey::Started => {
                    if !is_section {
                        let text = cell_text(*key, v, now_ms);
                        let gal = ellipsized(&clipped, &text, f_small(), base_color, cell.width() - 12.0);
                        clipped.galley(pos2(cell.left() + 4.0, cy - gal.size().y / 2.0), gal, base_color);
                    }
                }
                SortKey::Location => {
                    if let (false, Some(exe)) = (is_section, v.exe) {
                        let shown = model::display_path(exe, home);
                        let room = cell.width() - 12.0;
                        let font = f_small();
                        let text = model::ellipsize_start(&shown, |t| clipped.layout_no_wrap(t.to_string(), font.clone(), dim).size().x <= room);
                        let gal = clipped.layout_no_wrap(text, font, dim);
                        clipped.galley(pos2(cell.left() + 4.0, cy - gal.size().y / 2.0), gal, dim);
                    }
                }
                SortKey::Name => {}
            }
        }
    }
}

/// A process table row as a screen reader says it: its name, count and the numbers that matter,
/// whether it is selected and, for a group or a class, whether it is open.
fn row_spoken(row: &model::Row, groups: &[ProcGroup], v: &Values<'_>, selected: bool) -> Spoken {
    let role = egui::accesskit::Role::ListItem;
    match &row.kind {
        RowKind::Section { collapsed, groups: ng, procs: np } => {
            Spoken::new(role, a11y::class_name(class_title(section_class(&row.id)), *ng, *np)).expanded(!*collapsed).selected(selected)
        }
        RowKind::Group { gi, expanded, expandable } => {
            let g = &groups[*gi];
            let s = Spoken::new(role, a11y::process_name(&g.name, g.procs.len(), v.cpu, v.gpu, v.mem)).selected(selected).with_menu();
            let s = if *expandable { s.expanded(*expanded) } else { s };
            match &g.title {
                Some(t) => s.described(t.clone()),
                None => s,
            }
        }
        RowKind::Proc { gi, pi } => {
            let pr = &groups[*gi].procs[*pi];
            Spoken::new(role, a11y::process_name(&pr.name, 1, v.cpu, v.gpu, v.mem)).selected(selected).with_menu()
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn row_interaction(resp: &egui::Response, rect: Rect, row: &model::Row, groups: &[ProcGroup], v: &Values<'_>, cols: &Cols, acts: &mut Vec<Act>, menu_key: bool) {
    let id = row.id.clone();
    let is_section = matches!(row.kind, RowKind::Section { .. });
    let expandable = match row.kind {
        RowKind::Section { .. } => true,
        RowKind::Group { expandable, .. } => expandable,
        RowKind::Proc { .. } => false,
    };
    if resp.clicked() {
        let on_arrow = resp.interact_pointer_pos().is_some_and(|p| p.x < rect.left() + 30.0);
        if expandable && (on_arrow || is_section) {
            acts.push(Act::Toggle(id.clone()));
        } else {
            acts.push(Act::Select(id.clone()));
        }
    } else if resp.double_clicked() && expandable {
        acts.push(Act::Toggle(id.clone()));
    }
    if resp.secondary_clicked() {
        acts.push(Act::Select(id.clone()));
    }
    if !is_section {
        let has_path = v.exe.and_then(model::parent_dir).is_some();
        ui_kit::row_menu(resp, rect, menu_key, egui::Id::new(LIST_ID), |ui| {
            if ui.button("End task").clicked() {
                acts.push(Act::Kill(id.clone(), Signal::Term));
                ui.close();
            }
            if ui.button("Force kill").clicked() {
                acts.push(Act::Kill(id.clone(), Signal::Kill));
                ui.close();
            }
            ui.separator();
            if ui.add_enabled(has_path, egui::Button::new(format!("{}  Open file location", icon::FOLDER_OPEN))).clicked() {
                acts.push(Act::OpenLocation(id.clone()));
                ui.close();
            }
            if ui.add_enabled(has_path, egui::Button::new("Copy path")).clicked() {
                acts.push(Act::CopyPath(id.clone()));
                ui.close();
            }
            ui.separator();
            if ui.button("Copy PID").clicked() {
                acts.push(Act::CopyPid(id.clone()));
                ui.close();
            }
            if ui.button("Copy command line").clicked() {
                acts.push(Act::CopyCmd(id.clone()));
                ui.close();
            }
        });
    }
    // Tooltip: the cell under the pointer when its column has more to say (disk read and
    // write, start time, the full path), else the command line (processes) or window title and
    // id (groups).
    let cell_tip_text = resp.hover_pos().and_then(|p| cols.rects(rect).1.iter().find(|(_, r)| r.contains(p)).map(|(k, _)| *k)).and_then(|k| cell_tip(k, v, row, groups));
    let tip = cell_tip_text.or_else(|| match row.kind {
        RowKind::Proc { gi, pi } => Some(crate::text::shape(&groups[gi].procs[pi].cmdline).into_owned()),
        RowKind::Group { gi, .. } => {
            let g = &groups[gi];
            let mut s = String::new();
            if let Some(t) = &g.title {
                s.push_str(&crate::text::shape(t));
                s.push('\n');
            }
            s.push_str(&g.id);
            Some(s)
        }
        RowKind::Section { .. } => None,
    });
    if let Some(tip) = tip {
        if !resp.context_menu_opened() {
            resp.clone().on_hover_ui(|ui| {
                ui.set_max_width(520.0);
                ui.label(RichText::new(tip).monospace());
            });
        }
    }
}

fn section_class(id: &RowId) -> ProcClass {
    match id {
        RowId::Section(c) => *c,
        _ => ProcClass::App,
    }
}

fn section_totals(groups: &[ProcGroup], class: ProcClass) -> (f32, u64) {
    groups
        .iter()
        .filter(|g| g.class == class)
        .fold((0.0, 0), |(c, m), g| (c + g.cpu_pct, m + g.mem_bytes))
}

fn arrow(p: &egui::Painter, c: egui::Pos2, down: bool, color: Color32) {
    let g = if down { icon::ARROW_DOWN } else { icon::ARROW_UP };
    p.text(c, Align2::CENTER_CENTER, g, ui_kit::f_icon(base()), color);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_cols(name_cap: f32) -> Cols {
        Cols::new(model::visible_columns(&crate::settings::Settings::default().processes.views.bottom_compact.columns), 8.0, name_cap)
    }

    #[test]
    fn the_name_column_is_capped_and_the_columns_follow_it_with_location_taking_the_rest() {
        let cols = default_cols(560.0);
        assert_eq!(cols.keys().len(), 7);
        assert_eq!(cols.keys().last(), Some(&SortKey::Location), "Location is the last column");
        // a wide panel: the name stops at the cap, the columns start right after it
        let wide = Rect::from_min_size(pos2(10.0, 0.0), vec2(2000.0, 26.0));
        let (name, cells) = cols.rects(wide);
        assert!((name.width() - (560.0 - 4.0)).abs() < 0.01, "{}", name.width());
        assert!((cells[0].1.left() - (wide.left() + 560.0)).abs() < 0.01, "PID starts right after the name");
        for pair in cells.windows(2) {
            assert!((pair[0].1.right() - pair[1].1.left()).abs() < 0.01, "the columns touch");
        }
        let loc = cells.last().unwrap().1;
        assert!((loc.right() - wide.right()).abs() < 0.01, "Location runs to the right edge");
        assert!(loc.width() > 500.0, "and takes the width that is left: {}", loc.width());
        // a narrower panel: the name gives way, Location keeps at least 160 px
        let mid = Rect::from_min_size(pos2(0.0, 0.0), vec2(cols.content_width() + 80.0, 26.0));
        let (name, cells) = cols.rects(mid);
        let loc = cells.last().unwrap().1;
        assert!(loc.width() >= cols.loc_min - 0.01 && name.width() >= cols.name_min - 4.0 - 90.0, "{} {}", name.width(), loc.width());
        assert!((loc.right() - mid.right()).abs() < 0.5);
        // at exactly the content width nothing needs to scroll, and the parts add up
        let exact = Rect::from_min_size(pos2(0.0, 0.0), vec2(cols.content_width(), 26.0));
        let (name, cells) = cols.rects(exact);
        assert!((cells.last().unwrap().1.right() - exact.right()).abs() < 0.5);
        assert!((name.width() + 4.0 - cols.name_min).abs() < 0.5, "{}", name.width());
    }

    #[test]
    fn a_compact_view_fits_all_its_columns_at_its_minimum_width() {
        let keys = vec![SortKey::Cpu, SortKey::Gpu, SortKey::Mem];
        let cols = Cols::new(keys.clone(), 8.4, 1000.0).for_view(PanelView::Compact);
        let bar = 14.0;
        let panel = panel_width_for(&cols, bar);
        // the table region is the panel less its insets, and the list gives the bar its share
        let region = panel - 2.0 * PANEL_PAD_X;
        assert!(cols.content_width() + bar <= region + 0.01, "no sideways scrolling is needed");
        // Name keeps its compact minimum, which is smaller than the expanded one
        let row = Rect::from_min_size(pos2(0.0, 0.0), vec2(region - bar, 26.0));
        let (name, cells) = cols.rects(row);
        assert!(name.width() + 4.0 >= cols.name_min - 0.01, "{}", name.width());
        assert_eq!(cells.len(), 3);
        assert!(cells.iter().all(|(_, r)| r.right() <= row.right() + 0.01));
        let expanded = Cols::new(keys, 8.4, 0.0).for_view(PanelView::Expanded);
        assert!(cols.name_min < expanded.name_min);
        assert_eq!(expanded.name_min, crate::theme::fs(NAME_MIN));
    }

    #[test]
    fn without_location_the_numbers_follow_the_name_and_a_narrow_row_collapses_the_name() {
        let keys = vec![SortKey::Pid, SortKey::Cpu, SortKey::Mem];
        let cols = Cols::new(keys, 8.0, 560.0);
        let narrow = Rect::from_min_size(pos2(0.0, 0.0), vec2(cols.fixed_width() + 300.0, 26.0));
        let (name, cells) = cols.rects(narrow);
        assert!((name.width() - 296.0).abs() < 0.01);
        assert!((cells[2].1.right() - narrow.right()).abs() < 0.01);
        let (name, _) = cols.rects(Rect::from_min_size(pos2(0.0, 0.0), vec2(cols.fixed_width() - 50.0, 26.0)));
        assert!(name.width() <= 0.0);
        // the user's column choice decides the order and the set
        let one = Cols::new(vec![SortKey::Disk], 8.0, 560.0);
        assert_eq!(one.keys(), &[SortKey::Disk]);
    }

    #[test]
    fn the_search_shrinks_to_an_icon_when_the_toolbar_is_narrow() {
        let buttons = 200.0;
        // the compact right panel (about 476 px of toolbar) cannot hold title, field and buttons
        assert!(toolbar_is_narrow(476.0, buttons, 80.0));
        // a wide panel can
        assert!(!toolbar_is_narrow(900.0, buttons, 80.0));
        assert!(!toolbar_is_narrow(1800.0, buttons, 80.0));
        // the threshold is monotone in the width
        let mut seen_wide = false;
        for w in (300..1200).step_by(10) {
            let narrow = toolbar_is_narrow(w as f32, buttons, 80.0);
            assert!(!(seen_wide && narrow), "narrow again at {w}");
            seen_wide |= !narrow;
        }
    }

    #[test]
    fn new_columns_have_their_colours_alignment_and_heat() {
        let p = Palette::dark();
        assert_eq!(column_color(SortKey::Disk, &p), p.drive);
        assert_eq!(column_color(SortKey::Encode, &p), p.gpu);
        assert_eq!(column_color(SortKey::Decode, &p), p.gpu);
        for k in [SortKey::Threads, SortKey::User, SortKey::Started, SortKey::Location] {
            assert_eq!(column_color(k, &p), p.ink3);
        }
        assert!(left_aligned(SortKey::Location) && left_aligned(SortKey::User) && left_aligned(SortKey::Started));
        assert!(!left_aligned(SortKey::Disk) && !left_aligned(SortKey::Pid));
        assert!(!heavy_disk(None) && !heavy_disk(Some(49.9e6)) && heavy_disk(Some(50e6)));
        assert!(column_tip(SortKey::Disk, 3.0).contains("only shows disk activity for your own processes"));
    }

    #[test]
    fn cell_text_and_tooltips_follow_the_values() {
        let v = Values {
            pid: "42".into(),
            disk: Some(13.5e6),
            disk_read: Some(12.0e6),
            disk_write: Some(1.5e6),
            threads: Some(7),
            user: "alex",
            started: 1_760_000_000_000,
            exe: Some("/usr/bin/foo"),
            show_pid: true,
            ..Values::empty()
        };
        assert_eq!(cell_text(SortKey::Pid, &v, 0), "42");
        assert_eq!(cell_text(SortKey::Disk, &v, 0), "13.5 MB/s");
        assert_eq!(cell_text(SortKey::Threads, &v, 0), "7");
        assert_eq!(cell_text(SortKey::User, &v, 0), "alex");
        assert_eq!(cell_text(SortKey::Encode, &v, 0), "");
        assert_eq!(cell_text(SortKey::Started, &Values { started: 0, ..Values::empty() }, 5), "");
        let row = model::Row { id: RowId::Section(ProcClass::App), kind: RowKind::Section { groups: 0, procs: 0, collapsed: false } };
        let tip = cell_tip(SortKey::Disk, &v, &row, &[]).unwrap();
        assert!(tip.contains("Read 12.0 MB/s") && tip.contains("Write 1.5 MB/s"), "{tip}");
        assert!(cell_tip(SortKey::Started, &v, &row, &[]).unwrap().contains("2025"));
        assert_eq!(cell_tip(SortKey::Location, &v, &row, &[]).as_deref(), Some("/usr/bin/foo"));
        assert!(cell_tip(SortKey::Disk, &Values::empty(), &row, &[]).is_none(), "no tooltip for a blank disk cell");
    }

    #[test]
    fn the_alternate_row_is_visibly_off_the_panel() {
        for p in [Palette::dark(), Palette::light()] {
            let step = |a: Color32, b: Color32| (a.r() as i32 - b.r() as i32).abs() + (a.g() as i32 - b.g() as i32).abs() + (a.b() as i32 - b.b() as i32).abs();
            assert!(step(p.zebra, p.panel) >= 2 * step(p.alt, p.panel), "twice the old contrast");
            assert_ne!(p.zebra, p.hover);
        }
    }

    #[test]
    fn column_tips_explain_the_measure_and_smoothing() {
        let cpu = column_tip(SortKey::Cpu, 3.0);
        assert!(cpu.contains("whole machine") && cpu.contains("moving average") && cpu.contains("3 s"));
        assert!(column_tip(SortKey::Cpu, 0.0).contains("Not smoothed"));
        assert!(column_tip(SortKey::Gpu, 3.0).contains("shader"));
        assert!(column_tip(SortKey::Vram, 3.0).contains("Video memory"));
        assert!(column_tip(SortKey::Mem, 3.0).contains("Private memory"));
    }

    #[test]
    fn headers_use_the_device_colours() {
        let p = Palette::dark();
        assert_eq!(column_color(SortKey::Cpu, &p), p.cpu);
        assert_eq!(column_color(SortKey::Gpu, &p), p.gpu);
        assert_eq!(column_color(SortKey::Vram, &p), p.gpu);
        assert_eq!(column_color(SortKey::Mem, &p), p.mem);
        assert_eq!(column_color(SortKey::Name, &p), p.ink3);
        assert_eq!(column_color(SortKey::Pid, &p), p.ink3);
    }

    #[test]
    fn heavy_cells_follow_the_thresholds() {
        let h = Heavy::new(16, Some(32 << 30));
        assert_eq!(heavy_cells(&h, false, 5.5, Some(24.9), h.mem - 1), (false, false, false));
        assert_eq!(heavy_cells(&h, false, 5.7, None, 0), (true, false, false));
        assert_eq!(heavy_cells(&h, false, 0.0, Some(25.0), 0), (false, true, false));
        assert_eq!(heavy_cells(&h, false, 0.0, None, h.mem), (false, false, true));
    }

    #[test]
    fn one_pegged_thread_tints_on_any_cpu_count_and_class_rows_never_do() {
        for n in [4usize, 8, 16, 32] {
            let h = Heavy::new(n, Some(32 << 30));
            let one_thread = 100.0 / n as f32;
            assert!(heavy_cells(&h, false, one_thread, None, 0).0, "{n} threads: one flat out thread tints");
            assert!(!heavy_cells(&h, false, one_thread * 0.5, None, 0).0, "{n} threads: half a thread does not");
            // the same load on a class row (Apps, Background, System) never tints
            assert_eq!(heavy_cells(&h, true, 100.0, Some(100.0), u64::MAX), (false, false, false));
        }
    }

    #[test]
    fn the_memory_tint_scales_with_the_ram_of_the_machine() {
        let gib = 1u64 << 30;
        assert_eq!(Heavy::new(16, Some(32 * gib)).mem, 2 * gib, "a sixteenth: the old fixed value at 32 GiB");
        assert_eq!(Heavy::new(16, Some(64 * gib)).mem, 4 * gib);
        assert_eq!(Heavy::new(16, Some(16 * gib)).mem, gib);
        assert_eq!(Heavy::new(4, Some(2 * gib)).mem, 512 << 20, "never below the floor");
        assert_eq!(Heavy::new(16, None).mem, 2 * gib, "unknown RAM keeps 2 GiB");
        assert_eq!(Heavy::new(0, Some(0)), Heavy::new(1, None), "no information is one CPU and 2 GiB");
    }

    #[test]
    fn the_snapshot_gives_the_logical_cpu_count_and_the_ram() {
        let snap = crate::demo::Demo::new().snapshot(1.0);
        let h = Heavy::of_snapshot(&snap);
        let threads = snap.sensors.iter().filter(|g| g.id == "cpu").flat_map(|g| &g.sections).flat_map(|s| &s.readings).filter(|r| r.id.starts_with("cpu/thread/") && r.id.ends_with("/usage")).count();
        assert!(threads > 0);
        assert!((h.cpu - 90.0 / threads as f32).abs() < 1e-4, "{h:?} for {threads} threads");
        assert_eq!(h.mem, (snap.summary.ram_total_bytes.unwrap() as u64) / 16);
    }

    #[test]
    fn the_fallback_badge_is_neutral_and_still_parts_the_rows() {
        let p = Palette::dark();
        let names = ["Alacritty", "Brave", "Code", "Discord", "Firefox", "Gimp", "Okular", "Steam", "Telegram", "Zed"];
        let mut strengths = std::collections::HashSet::new();
        for pal in [Palette::dark(), Palette::light()] {
            for n in names {
                let (c, a) = badge_tint(n, &pal);
                assert_eq!(c, pal.ink3, "{n}: the badge borrows no device or warning colour");
                assert!(badge_tint_strengths().contains(&a));
                strengths.insert((a * 100.0) as u32);
            }
        }
        assert!(strengths.len() > 1, "different names get different strengths");
        assert_eq!(badge_tint("Steam", &p), badge_tint("Steam", &p), "stable per name");
    }

    #[test]
    fn empty_list_messages() {
        assert_eq!(empty_message(""), "No processes reported");
        assert_eq!(empty_message("  "), "No processes reported");
        assert_eq!(empty_message("xyz"), "No process matches \"xyz\"");
        // a pasted paragraph is cut to a line
        let long = empty_message(&"x".repeat(500));
        assert!(long.chars().count() < 90 && long.ends_with("\u{2026}\""), "{long}");
    }

    fn explicit(width: Option<f32>, view: PanelView) -> Cols {
        default_cols(560.0).for_view(view).with_name_width(width)
    }

    #[test]
    fn an_explicit_name_width_is_used_and_the_data_columns_follow_it() {
        let row = Rect::from_min_size(pos2(10.0, 0.0), vec2(2000.0, 26.0));
        let cols = explicit(Some(300.0), PanelView::Compact);
        let (name, cells) = cols.rects(row);
        assert!((name.width() - 296.0).abs() < 0.01, "{}", name.width());
        assert!((cells[0].1.left() - (row.left() + 300.0)).abs() < 0.01, "the first data column starts right after Name");
        for pair in cells.windows(2) {
            assert!((pair[0].1.right() - pair[1].1.left()).abs() < 0.01);
        }
        assert!((cells.last().unwrap().1.right() - row.right()).abs() < 0.01, "Location takes the rest");
        // the caps only shape the automatic width: an explicit one may exceed them
        let wide = explicit(Some(900.0), PanelView::Compact).rects(row).0;
        assert!((wide.width() - 896.0).abs() < 0.01);
        assert!((explicit(None, PanelView::Compact).rects(row).0.width() - 556.0).abs() < 0.01);
        // without Location the slack sits at the right end
        let keys = vec![SortKey::Pid, SortKey::Cpu, SortKey::Mem];
        let c = Cols::new(keys, 8.0, 560.0).with_name_width(Some(300.0));
        let (_, cells) = c.rects(row);
        assert!((cells[0].1.left() - (row.left() + 300.0)).abs() < 0.01);
        assert!(cells.last().unwrap().1.right() < row.right() - 100.0);
    }

    #[test]
    fn the_name_width_is_clamped_so_the_data_columns_and_location_stay_visible() {
        let cols = explicit(Some(5000.0), PanelView::Expanded);
        // a row only a little wider than the table needs: Name gives way, Location keeps its minimum
        let row = Rect::from_min_size(pos2(0.0, 0.0), vec2(cols.content_width() + 60.0, 26.0));
        let (name, cells) = cols.rects(row);
        let loc = cells.last().unwrap().1;
        assert!(loc.width() >= cols.loc_min - 0.01 && (loc.right() - row.right()).abs() < 0.5, "{}", loc.width());
        assert!((name.width() + 4.0 - (cols.name_min + 60.0)).abs() < 0.5, "Name took what was left: {}", name.width());
        assert!(cells.iter().all(|(_, r)| r.right() <= row.right() + 0.01), "every column is on screen");
        // an explicit width below the minimum renders at the minimum
        let small = explicit(Some(10.0), PanelView::Expanded);
        let (name, _) = small.rects(Rect::from_min_size(pos2(0.0, 0.0), vec2(2000.0, 26.0)));
        assert!((name.width() + 4.0 - small.name_min).abs() < 0.01);
        // the compact views allow a narrower Name than the expanded ones
        let compact = explicit(Some(10.0), PanelView::Compact);
        assert!(compact.name_bounds(2000.0).0 < small.name_bounds(2000.0).0);
        // the bounds follow the window: shrinking it moves the upper limit, the saved value stays
        let (lo, hi) = cols.name_bounds(1500.0);
        assert!(lo == cols.name_min && hi > lo && hi < 1500.0 - cols.fixed_width() - cols.loc_min + 0.01);
        assert_eq!(cols.name_width, Some(5000.0));
    }

    #[test]
    fn fitting_uses_the_widest_visible_name_plus_its_count_and_is_clamped() {
        let demo = crate::demo::Demo::new().snapshot(12.0);
        let mut groups = demo.procs.clone().unwrap();
        groups.truncate(3);
        for (g, name) in groups.iter_mut().zip(["a", "the longest group name", "mid name"]) {
            g.name = name.into();
        }
        // one group with several processes (a count), one single
        groups[1].procs.truncate(2);
        groups[0].procs.truncate(1);
        groups[0].procs[0].name = "a-process-name-that-is-longer-than-any-group-by-far".into();
        let mut rows: Vec<model::Row> = (0..3).map(|gi| model::Row { id: RowId::Group(groups[gi].id.clone()), kind: RowKind::Group { gi, expanded: false, expandable: true } }).collect();
        rows.insert(0, model::Row { id: RowId::Section(ProcClass::App), kind: RowKind::Section { groups: 3, procs: 4, collapsed: false } });
        let measure = |t: &str, _bold: bool| t.chars().count() as f32 * 10.0;
        // the process row is not visible yet: the longest group name (22 chars) plus " (2)"
        let e = widest_name_extent(&rows, &groups, measure);
        assert!((e - (NAME_TEXT_X + 220.0 + 6.0 + 30.0)).abs() < 0.01, "{e}");
        // an expanded process row counts too
        rows.push(model::Row { id: RowId::Proc(groups[0].procs[0].pid, 0), kind: RowKind::Proc { gi: 0, pi: 0 } });
        let e2 = widest_name_extent(&rows, &groups, measure);
        assert!((e2 - (NAME_TEXT_X + 510.0)).abs() < 0.01, "{e2}");
        // the section caption and window titles do not count
        groups[2].title = Some("a very long window title that should not matter at all".into());
        assert_eq!(widest_name_extent(&rows, &groups, measure), e2);
        // fit adds the padding and stays inside the bounds
        assert_eq!(fit_name_width(e, (200.0, 1000.0)), e + NAME_FIT_PAD);
        assert_eq!(fit_name_width(e2, (200.0, 400.0)), 400.0);
        assert_eq!(fit_name_width(10.0, (200.0, 400.0)), 200.0);
    }

    fn demo_rows() -> (Vec<ProcGroup>, Vec<model::Row>) {
        let mut groups = crate::demo::Demo::new().snapshot(12.0).procs.clone().unwrap();
        groups.truncate(4);
        let rows = (0..groups.len()).map(|gi| model::Row { id: RowId::Group(groups[gi].id.clone()), kind: RowKind::Group { gi, expanded: false, expandable: true } }).collect();
        (groups, rows)
    }

    #[test]
    fn a_column_needs_its_header_with_the_arrow_or_its_widest_cell_plus_the_padding() {
        let arrow = crate::theme::fs(ARROW_ROOM);
        // a short number under a long header: the header (and room for the sort arrow) decides
        assert_eq!(column_want(40.0, 20.0), 40.0 + arrow + CELL_PAD);
        // a long number under a short header: the cell decides
        assert_eq!(column_want(40.0, 90.0), 90.0 + CELL_PAD);
        // never below the header, even with no cells at all
        assert!(column_want(40.0, 0.0) > 40.0);
    }

    #[test]
    fn the_extents_are_the_widest_cell_text_of_each_column_in_the_rows() {
        let (mut groups, mut rows) = demo_rows();
        let keys = [SortKey::Cpu, SortKey::Mem, SortKey::User, SortKey::Location, SortKey::Name];
        let measure = |_: SortKey, t: &str| t.chars().count() as f32 * 10.0;
        let ex = column_extents(&keys, &rows, &groups, 0, measure);
        // Name and Location are flexible: no extent
        assert_eq!(ex.iter().map(|e| e.0).collect::<Vec<_>>(), vec![SortKey::Cpu, SortKey::Mem, SortKey::User]);
        let widest = |key: SortKey, ex: &[(SortKey, f32)]| ex.iter().find(|e| e.0 == key).unwrap().1;
        let mem = |g: &ProcGroup| format_value(Unit::Bytes, g.mem_bytes as f64).chars().count() as f32 * 10.0;
        assert_eq!(widest(SortKey::Mem, &ex), groups.iter().map(mem).fold(0.0, f32::max));
        // a longer value in any row widens the column; the other columns do not move
        groups[2].mem_bytes = 123 * 1024 * 1024 * 1024;
        groups[2].procs.iter_mut().for_each(|p| p.user = "a-long-user-name".into());
        let ex2 = column_extents(&keys, &rows, &groups, 0, measure);
        assert!(widest(SortKey::Mem, &ex2) >= widest(SortKey::Mem, &ex));
        assert_eq!(widest(SortKey::User, &ex2), 160.0);
        assert_eq!(widest(SortKey::Cpu, &ex2), widest(SortKey::Cpu, &ex));
        // a class row counts for its totals only, not for columns it leaves empty
        rows.insert(0, model::Row { id: RowId::Section(ProcClass::App), kind: RowKind::Section { groups: 4, procs: 8, collapsed: false } });
        let ex3 = column_extents(&keys, &rows, &groups, 0, measure);
        assert_eq!(widest(SortKey::User, &ex3), 160.0);
    }

    #[test]
    fn a_fitted_column_grows_at_once_and_shrinks_only_after_staying_narrower() {
        let mut fit = ColFit::default();
        assert_eq!(fit.width(SortKey::Cpu), None, "nothing is measured yet");
        assert_eq!(fit.update(&[(SortKey::Cpu, 80.0), (SortKey::Mem, 100.0)], 0.0, 1.0), None);
        assert_eq!((fit.width(SortKey::Cpu), fit.width(SortKey::Mem)), (Some(80.0), Some(100.0)));
        // wider: at once
        fit.update(&[(SortKey::Cpu, 95.0), (SortKey::Mem, 100.0)], 1.0, 1.0);
        assert_eq!(fit.width(SortKey::Cpu), Some(95.0));
        // narrower: held, and the caller is told when to look again
        let due = fit.update(&[(SortKey::Cpu, 80.0), (SortKey::Mem, 100.0)], 2.0, 1.0);
        assert_eq!(fit.width(SortKey::Cpu), Some(95.0));
        assert_eq!(due, Some(2.0 + SHRINK_AFTER_S));
        // it flickers wide again before the time is up: the wait starts over
        fit.update(&[(SortKey::Cpu, 95.0), (SortKey::Mem, 100.0)], 4.0, 1.0);
        fit.update(&[(SortKey::Cpu, 80.0), (SortKey::Mem, 100.0)], 4.5, 1.0);
        fit.update(&[(SortKey::Cpu, 80.0), (SortKey::Mem, 100.0)], 4.5 + SHRINK_AFTER_S - 0.1, 1.0);
        assert_eq!(fit.width(SortKey::Cpu), Some(95.0), "not yet");
        // after the time it shrinks to the widest it needed meanwhile, not to the last value
        fit.update(&[(SortKey::Cpu, 85.0), (SortKey::Mem, 100.0)], 4.5 + SHRINK_AFTER_S + 0.1, 1.0);
        assert_eq!(fit.width(SortKey::Cpu), Some(85.0));
        assert_eq!(fit.width(SortKey::Mem), Some(100.0), "an unchanged column never moved");
        // a column that is no longer shown is forgotten, and a new font scale starts over
        fit.update(&[(SortKey::Mem, 100.0)], 20.0, 1.0);
        assert_eq!(fit.width(SortKey::Cpu), None);
        fit.update(&[(SortKey::Mem, 60.0)], 20.1, 1.25);
        assert_eq!(fit.width(SortKey::Mem), Some(60.0));
    }

    #[test]
    fn the_columns_take_their_fitted_widths_and_the_layout_follows_them() {
        let keys = vec![SortKey::Cpu, SortKey::Gpu, SortKey::Mem];
        let start = Cols::new(keys.clone(), 8.4, 1000.0).for_view(PanelView::Compact);
        let mut fit = ColFit::default();
        fit.update(&[(SortKey::Cpu, 70.0), (SortKey::Gpu, 70.0), (SortKey::Mem, 90.0)], 0.0, 1.0);
        let cols = Cols::new(keys, 8.4, 1000.0).for_view(PanelView::Compact).with_fit(&fit);
        assert_eq!(cols.fixed_width(), 230.0);
        assert!(cols.fixed_width() < start.fixed_width(), "autofit is tighter than the starting widths");
        // the panel's least width, Name's bounds and the rects all use them
        let bar = 14.0;
        assert_eq!(panel_width_for(&cols, bar), cols.name_min + 230.0 + bar + 2.0 * PANEL_PAD_X);
        let row = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 26.0));
        assert_eq!(cols.name_bounds(row.width()).1, 370.0);
        let (name, cells) = cols.rects(row);
        assert_eq!(cells.iter().map(|c| c.1.width()).collect::<Vec<_>>(), vec![70.0, 70.0, 90.0]);
        assert!((name.width() + 4.0 - 370.0).abs() < 0.01 && (cells[2].1.right() - row.right()).abs() < 0.01);
    }

    #[test]
    fn in_the_right_dock_the_panel_width_always_goes_to_name_and_the_extra_to_the_data_columns() {
        let keys = vec![SortKey::Cpu, SortKey::Gpu, SortKey::Mem];
        let mut fit = ColFit::default();
        fit.update(&[(SortKey::Cpu, 70.0), (SortKey::Gpu, 70.0), (SortKey::Mem, 90.0)], 0.0, 1.0);
        let table = |extra: Option<f32>| Cols::new(keys.clone(), 8.4, f32::INFINITY).for_view(PanelView::Compact).with_fit(&fit).with_name_width(Some(123.0)).right_dock(extra);
        let row = |w: f32| Rect::from_min_size(pos2(10.0, 0.0), vec2(w, 26.0));
        let widths = |cells: &[(SortKey, Rect)]| cells.iter().map(|c| c.1.width().round()).collect::<Vec<_>>();
        // no extra: the data columns are their fitted widths and Name has all the rest; the saved
        // Name width plays no part
        let (name, cells) = table(None).rects(row(500.0));
        assert!((name.width() + 4.0 - 270.0).abs() < 0.01);
        assert_eq!(widths(&cells), vec![70.0, 70.0, 90.0]);
        // an extra of 90 is shared equally and comes out of Name
        let (name, cells) = table(Some(90.0)).rects(row(500.0));
        assert!((name.width() + 4.0 - 180.0).abs() < 0.01);
        assert_eq!(widths(&cells), vec![100.0, 100.0, 120.0]);
        assert!((cells[0].1.left() - (10.0 + 180.0)).abs() < 0.01, "the boundary is Name's right edge");
        assert!((cells[2].1.right() - 510.0).abs() < 0.01, "the row stays full");
        // widening the panel by 200 gives Name all 200 and every data column keeps its width
        let (wide, wide_cells) = table(Some(90.0)).rects(row(700.0));
        assert!((wide.width() - name.width() - 200.0).abs() < 0.01);
        assert_eq!(widths(&wide_cells), widths(&cells));
        // a panel narrower than the extra allows holds the extra back to Name's minimum, and the
        // saved value is not rewritten
        let narrow = table(Some(90.0));
        let (name, cells) = narrow.rects(row(400.0));
        assert!((name.width() + 4.0 - narrow.name_min).abs() < 0.01, "{}", name.width());
        assert_eq!(widths(&cells), vec![87.0, 87.0, 107.0]);
        assert_eq!(narrow.data_extra, Some(90.0));
        assert!(narrow.data_extra_now(400.0) < 90.0 && narrow.data_extra_now(500.0) == 90.0);
        // an extra beyond the room is cut to it, and a negative one is none
        let all = table(Some(5000.0));
        assert!((all.data_extra_now(500.0) - (500.0 - 230.0 - all.name_min)).abs() < 0.01);
        assert_eq!(table(Some(-5.0)).data_extra_now(500.0), 0.0);
        // with a Location column that column keeps taking the remainder; the extra still comes out
        // of Name and no data column widens because the panel did
        let mut with_loc = keys.clone();
        with_loc.push(SortKey::Location);
        let loc = |extra: Option<f32>, cap: f32| Cols::new(with_loc.clone(), 8.4, cap).for_view(PanelView::Compact).with_fit(&fit).right_dock(extra);
        let (n0, c0) = loc(Some(60.0), f32::INFINITY).rects(row(700.0));
        let (n1, c1) = loc(Some(60.0), f32::INFINITY).rects(row(900.0));
        assert_eq!(widths(&c0[..3]), vec![90.0, 90.0, 110.0]);
        assert_eq!(widths(&c1[..3]), widths(&c0[..3]), "a wider panel leaves the data columns alone");
        assert!((n1.width() - n0.width() - 200.0).abs() < 0.01, "Name took the width");
        assert!((c0[3].1.right() - 710.0).abs() < 0.01 && c0[3].1.width() > 150.0);
        // under a cap Name stops at it, Location takes the rest, and the extra still moves the boundary
        let (n, c) = loc(None, 250.0).rects(row(900.0));
        let (ne, ce) = loc(Some(60.0), 250.0).rects(row(900.0));
        assert!((n.width() + 4.0 - 250.0).abs() < 0.01);
        assert!((n.width() - ne.width() - 60.0).abs() < 0.01 && (c[3].1.right() - ce[3].1.right()).abs() < 0.01);
        // the bottom dock does not use the extra
        let bottom = Cols::new(keys.clone(), 8.4, f32::INFINITY).for_view(PanelView::Compact).with_fit(&fit).with_name_width(Some(180.0));
        let (name, cells) = bottom.rects(row(500.0));
        assert!((name.width() + 4.0 - 180.0).abs() < 0.01);
        assert_eq!(widths(&cells), vec![70.0, 70.0, 90.0]);
        assert!(cells[2].1.right() < 510.0 - 50.0, "the slack sits at the right");
    }
}
