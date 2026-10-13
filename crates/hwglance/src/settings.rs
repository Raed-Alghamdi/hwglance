//! `~/.config/hwglance/ui.json`: every user customisation, shared with the Omarchy bar widget
//! (which reads the `tray` part). This app is the only writer: atomic write (temp file plus
//! rename), debounced by the app, re-read when the mtime changes. The warning and critical limits
//! are not here: they live in `thresholds.json`, which only the collector writes.
//!
//! Every object keeps unknown keys (`extra`), so new fields added by other tools survive a
//! round trip, and ids of sensors that no longer exist are never dropped.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

pub const VERSION: u32 = 1;
pub const DEFAULT_FONT_SIZE: f32 = 14.5;

fn t() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Appearance {
    pub font_size: f32,
    /// `atkinson` (the default) or `omarchy`.
    pub font: String,
    /// `system` (the default: follow the desktop's light or dark setting), `dark` or `light`.
    pub theme: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance { font_size: DEFAULT_FONT_SIZE, font: "atkinson".into(), theme: "system".into(), extra: Map::new() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct SensorPrefs {
    pub highlighted: Vec<String>,
    pub hidden: Vec<String>,
    pub hidden_sections: Vec<String>,
    pub hidden_cards: Vec<String>,
    /// Section id to the id of the reading shown on its header while collapsed.
    pub summary_overrides: BTreeMap<String, String>,
    /// How many minutes of history the collector keeps for every reading.
    pub history_minutes: u32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for SensorPrefs {
    fn default() -> Self {
        SensorPrefs {
            highlighted: Vec::new(),
            hidden: Vec::new(),
            hidden_sections: Vec::new(),
            hidden_cards: Vec::new(),
            summary_overrides: BTreeMap::new(),
            history_minutes: 30,
            extra: Map::new(),
        }
    }
}

pub const HISTORY_CHOICES: [u32; 7] = [5, 10, 15, 30, 60, 120, 240];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ColumnPref {
    pub width: f32,
    #[serde(default)]
    pub cards: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct LayoutPrefs {
    pub auto: bool,
    pub columns: Vec<ColumnPref>,
    pub collapsed_cards: Vec<String>,
    /// Section id to collapsed (true) or expanded (false); absent means the daemon default.
    pub collapsed_sections: BTreeMap<String, bool>,
    pub section_order: BTreeMap<String, Vec<String>>,
    pub reading_order: BTreeMap<String, Vec<String>>,
    pub process_panel_fraction: f32,
    pub process_panel_visible: bool,
    /// Where the process panel sits: `bottom` (default) or `right`.
    pub process_dock: String,
    /// The remembered view of each dock: `compact` or `expanded`.
    pub process_view: DockViews,
    /// Width share of the process panel when docked right.
    pub process_panel_width_fraction: f32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The remembered view (`compact` or `expanded`) of each dock position.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct DockViews {
    pub bottom: String,
    pub right: String,
}

impl Default for DockViews {
    fn default() -> Self {
        DockViews { bottom: "compact".into(), right: "compact".into() }
    }
}

/// Compact (few columns, small) or expanded (the full table, large) view of the process panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelView {
    Compact,
    Expanded,
}

impl PanelView {
    pub fn as_str(self) -> &'static str {
        if self == PanelView::Expanded { "expanded" } else { "compact" }
    }

    pub fn parse(s: &str) -> PanelView {
        if s == "expanded" { PanelView::Expanded } else { PanelView::Compact }
    }

    pub fn toggled(self) -> PanelView {
        if self == PanelView::Expanded { PanelView::Compact } else { PanelView::Expanded }
    }
}

/// Where the process panel is docked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dock {
    Bottom,
    Right,
}

impl LayoutPrefs {
    pub fn dock(&self) -> Dock {
        if self.process_dock == "right" { Dock::Right } else { Dock::Bottom }
    }

    pub fn set_dock(&mut self, d: Dock) {
        self.process_dock = if d == Dock::Right { "right" } else { "bottom" }.to_string();
    }

    /// The remembered view of the current dock.
    pub fn view(&self) -> PanelView {
        PanelView::parse(match self.dock() {
            Dock::Bottom => &self.process_view.bottom,
            Dock::Right => &self.process_view.right,
        })
    }

    /// Switch the current dock's view (the other dock keeps its own).
    pub fn set_view(&mut self, v: PanelView) {
        let slot = match self.dock() {
            Dock::Bottom => &mut self.process_view.bottom,
            Dock::Right => &mut self.process_view.right,
        };
        *slot = v.as_str().to_string();
    }
}

impl Default for LayoutPrefs {
    fn default() -> Self {
        LayoutPrefs {
            auto: true,
            columns: Vec::new(),
            collapsed_cards: Vec::new(),
            collapsed_sections: BTreeMap::new(),
            section_order: BTreeMap::new(),
            reading_order: BTreeMap::new(),
            process_panel_fraction: 0.45,
            process_panel_visible: true,
            process_dock: "bottom".into(),
            process_view: DockViews::default(),
            process_panel_width_fraction: 0.42,
            extra: Map::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SortPref {
    pub column: String,
    pub descending: bool,
}

/// One view of the process panel: its size and its columns.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ViewPrefs {
    /// Share of the content height (bottom dock) or width (right dock) the panel takes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    /// Width in logical pixels, for the views sized by pixels (right, compact).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width_px: Option<f32>,
    /// Name column width in logical pixels, set by dragging the header divider. Absent means the
    /// automatic width (caps apply only to that). Used in the bottom dock only: the right dock
    /// keeps `data_extra` instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_width: Option<f32>,
    /// Right dock: the width, in logical pixels, the data columns take beyond their autofit
    /// widths, set by dragging the Name divider. Name takes the rest, so the panel's width always
    /// goes to Name. Absent means none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_extra: Option<f32>,
    /// Optional columns shown, in table order (ids as in `model::SortKey::OPTIONAL`).
    pub columns: Vec<String>,
}

impl Default for ViewPrefs {
    fn default() -> Self {
        ViewPrefs { size: None, width_px: None, name_width: None, data_extra: None, columns: crate::model::DEFAULT_COLUMNS.iter().map(|s| s.to_string()).collect() }
    }
}

/// Default width of the right dock's compact view: Name at about 220 px plus CPU, GPU and Memory.
pub const COMPACT_WIDTH_PX: f32 = 500.0;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ProcViews {
    pub bottom_compact: ViewPrefs,
    pub bottom_expanded: ViewPrefs,
    pub right_compact: ViewPrefs,
    pub right_expanded: ViewPrefs,
}

impl Default for ProcViews {
    fn default() -> Self {
        let full = ViewPrefs::default();
        ProcViews {
            bottom_compact: ViewPrefs { size: Some(0.30), ..full.clone() },
            bottom_expanded: ViewPrefs { size: Some(0.65), ..full.clone() },
            right_compact: ViewPrefs {
                width_px: Some(COMPACT_WIDTH_PX),
                columns: ["cpu", "gpu", "memory"].iter().map(|s| s.to_string()).collect(),
                ..ViewPrefs::default()
            },
            right_expanded: ViewPrefs { size: Some(0.55), ..full },
        }
    }
}

impl ProcViews {
    pub fn get(&self, dock: Dock, view: PanelView) -> &ViewPrefs {
        match (dock, view) {
            (Dock::Bottom, PanelView::Compact) => &self.bottom_compact,
            (Dock::Bottom, PanelView::Expanded) => &self.bottom_expanded,
            (Dock::Right, PanelView::Compact) => &self.right_compact,
            (Dock::Right, PanelView::Expanded) => &self.right_expanded,
        }
    }

    pub fn get_mut(&mut self, dock: Dock, view: PanelView) -> &mut ViewPrefs {
        match (dock, view) {
            (Dock::Bottom, PanelView::Compact) => &mut self.bottom_compact,
            (Dock::Bottom, PanelView::Expanded) => &mut self.bottom_expanded,
            (Dock::Right, PanelView::Compact) => &mut self.right_compact,
            (Dock::Right, PanelView::Expanded) => &mut self.right_expanded,
        }
    }

    pub const ALL: [(Dock, PanelView); 4] = [
        (Dock::Bottom, PanelView::Compact),
        (Dock::Bottom, PanelView::Expanded),
        (Dock::Right, PanelView::Compact),
        (Dock::Right, PanelView::Expanded),
    ];
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ProcPrefs {
    pub hold_order_on_hover: bool,
    pub resort_seconds: f32,
    pub smoothing_seconds: f32,
    pub sort: SortPref,
    /// Legacy: the one column list of versions before views. Read to seed the views of an old
    /// file; `views` holds the columns now.
    pub columns: Vec<String>,
    /// Size and columns of the four panel views.
    pub views: ProcViews,
    /// Process section (`app`, `background`, `system`) to collapsed.
    pub collapsed_sections: BTreeMap<String, bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for ProcPrefs {
    fn default() -> Self {
        ProcPrefs {
            hold_order_on_hover: t(),
            resort_seconds: 3.0,
            smoothing_seconds: 3.0,
            sort: SortPref { column: "cpu".into(), descending: true },
            columns: crate::model::DEFAULT_COLUMNS.iter().map(|s| s.to_string()).collect(),
            views: ProcViews::default(),
            collapsed_sections: BTreeMap::new(),
            extra: Map::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct TrayPrefs {
    /// Flat list; the bar widget shows it as three cards (CPU, GPU, Memory) by id prefix.
    pub popup_values: Vec<String>,
    pub popup_process_count: u32,
    /// `cpu`, `gpu` or `memory`.
    pub popup_sort: String,
    /// What the bar button shows: `icon`, `cpu`, `gpu` or `cpu_gpu` (anything else acts as `icon`).
    pub bar_display: String,
    /// Keeps keys this version no longer edits (`bar_values`) and anything newer.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for TrayPrefs {
    fn default() -> Self {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect();
        TrayPrefs {
            popup_values: v(&[
                "cpu/usage/total",
                "cpu/temp/tctl",
                "cpu/power/package",
                "cpu/clock/max",
                "gpu0/load/core",
                "gpu0/temp/core",
                "gpu0/temp/hotspot",
                "gpu0/mem/used",
                "gpu0/power/draw",
                "memory/usage/pct",
                "memory/usage/used",
                "memory/temp/summary",
            ]),
            popup_process_count: 8,
            popup_sort: "cpu".into(),
            bar_display: "icon".into(),
            extra: Map::new(),
        }
    }
}

/// The values `TrayPrefs::bar_display` can take.
pub const BAR_DISPLAYS: [&str; 4] = ["icon", "cpu", "gpu", "cpu_gpu"];

impl TrayPrefs {
    /// The bar button's display mode, with an unknown saved value read as `icon`.
    pub fn bar_mode(&self) -> &str {
        BAR_DISPLAYS.iter().find(|m| **m == self.bar_display).copied().unwrap_or("icon")
    }
}

/// Notification settings the collector reads.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct AlertPrefs {
    /// `off`, `critical` or `warning` (warning and critical).
    pub level: String,
    /// `temperatures` or `all` (all values with thresholds).
    pub scope: String,
    pub cooldown_minutes: u32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for AlertPrefs {
    fn default() -> Self {
        AlertPrefs { level: "critical".into(), scope: "temperatures".into(), cooldown_minutes: 10, extra: Map::new() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub appearance: Appearance,
    pub sensors: SensorPrefs,
    pub layout: LayoutPrefs,
    pub processes: ProcPrefs,
    pub tray: TrayPrefs,
    pub alerts: AlertPrefs,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            version: VERSION,
            appearance: Appearance::default(),
            sensors: SensorPrefs::default(),
            layout: LayoutPrefs::default(),
            processes: ProcPrefs::default(),
            tray: TrayPrefs::default(),
            alerts: AlertPrefs::default(),
            extra: Map::new(),
        }
    }
}

impl Settings {
    /// The view the panel is in now (current dock, its remembered view).
    pub fn current_view(&self) -> &ViewPrefs {
        self.processes.views.get(self.layout.dock(), self.layout.view())
    }

    pub fn current_view_mut(&mut self) -> &mut ViewPrefs {
        let (d, v) = (self.layout.dock(), self.layout.view());
        self.processes.views.get_mut(d, v)
    }

    /// The process panel's size in the current view back to what that view starts with (Home on
    /// the panel divider). Its columns and Name width stay.
    pub fn reset_panel_size(&mut self) {
        let (d, v) = (self.layout.dock(), self.layout.view());
        let default = ProcViews::default().get(d, v).clone();
        let cur = self.processes.views.get_mut(d, v);
        cur.size = default.size;
        cur.width_px = default.width_px;
    }

    /// Switch between the compact and the expanded view of the current dock.
    pub fn toggle_panel_view(&mut self) {
        let v = self.layout.view().toggled();
        self.layout.set_view(v);
    }

    /// Seed the views of a file written before views existed from its old keys, so the
    /// panel keeps the size and the columns it had: the bottom dock's compact view takes the
    /// old height share and the column list, the right dock's expanded view the old width
    /// share and the column list. A file docked right starts in that expanded view.
    pub fn seed_views_from_legacy(&mut self) {
        let cols = self.processes.columns.clone();
        let v = &mut self.processes.views;
        v.bottom_compact.size = Some(self.layout.process_panel_fraction);
        v.bottom_compact.columns = cols.clone();
        v.bottom_expanded.columns = cols.clone();
        v.right_expanded.size = Some(self.layout.process_panel_width_fraction);
        v.right_expanded.columns = cols;
        if self.layout.dock() == Dock::Right {
            self.layout.process_view.right = "expanded".into();
        }
    }


    /// Fold the retired per-dock arrangement into the single one: a file whose bottom
    /// arrangement is automatic but whose right-dock arrangement was customised keeps that
    /// arrangement, so it is not lost. The old keys are dropped either way.
    pub fn adopt_right_arrangement(&mut self) {
        let auto_right = self.layout.extra.remove("auto_right");
        let columns_right = self.layout.extra.remove("columns_right");
        if !self.layout.auto || auto_right != Some(Value::Bool(false)) {
            return;
        }
        let cols: Vec<ColumnPref> = columns_right.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        if !cols.is_empty() {
            self.layout.auto = false;
            self.layout.columns = cols;
        }
    }

    /// "Reset all customisations": everything except the bar widget and alert choices.
    pub fn reset_customisations(&mut self) {
        let tray = std::mem::take(&mut self.tray);
        let alerts = std::mem::take(&mut self.alerts);
        *self = Settings { tray, alerts, ..Settings::default() };
    }

    /// Take back [`Settings::reset_customisations`]: `saved` is the state before it. The bar widget
    /// and alert choices are the current ones, as the reset kept them.
    pub fn restore_customisations(&mut self, saved: Settings) {
        let tray = std::mem::take(&mut self.tray);
        let alerts = std::mem::take(&mut self.alerts);
        *self = Settings { tray, alerts, ..saved };
    }

    pub fn is_highlighted(&self, id: &str) -> bool {
        self.sensors.highlighted.iter().any(|h| h == id)
    }

    /// Un-hide a reading together with its section and card.
    pub fn hidden_remove(&mut self, reading: &str, section: &str, card: &str) {
        self.sensors.hidden.retain(|x| x != reading);
        self.sensors.hidden_sections.retain(|x| x != section);
        self.sensors.hidden_cards.retain(|x| x != card);
    }

    pub fn is_hidden(&self, id: &str) -> bool {
        self.sensors.hidden.iter().any(|h| h == id)
    }
}

fn finite_or(v: f32, default: f32) -> f32 {
    if v.is_finite() { v } else { default }
}

/// A size that must be a finite number, within `lo..=hi`; anything else is dropped (automatic).
fn sane_opt(v: &mut Option<f32>, lo: f32, hi: f32) {
    *v = v.filter(|x| x.is_finite()).map(|x| x.clamp(lo, hi));
}

impl Settings {
    /// Bring every number a hand-edited or damaged file can get wrong back into the range the
    /// window works in, so a value like 1e30 seconds or a width of -5 never reaches a layout, a
    /// timer or a duration. Run on every load. Values already in range are left alone.
    pub fn sanitize(&mut self) {
        let a = &mut self.appearance;
        a.font_size = finite_or(a.font_size, DEFAULT_FONT_SIZE).clamp(10.0, 24.0);
        let l = &mut self.layout;
        l.process_panel_fraction = finite_or(l.process_panel_fraction, 0.45).clamp(0.1, 0.85);
        l.process_panel_width_fraction = finite_or(l.process_panel_width_fraction, 0.42).clamp(0.2, 0.85);
        for c in &mut l.columns {
            c.width = finite_or(c.width, crate::layout::NEW_COL_W).clamp(crate::layout::MIN_EXPLICIT_W, crate::layout::MAX_EXPLICIT_W);
        }
        let p = &mut self.processes;
        p.resort_seconds = finite_or(p.resort_seconds, 3.0).clamp(0.0, 300.0);
        p.smoothing_seconds = finite_or(p.smoothing_seconds, 3.0).clamp(0.0, 60.0);
        for (dock, view) in ProcViews::ALL {
            let v = p.views.get_mut(dock, view);
            sane_opt(&mut v.size, 0.1, 0.85);
            sane_opt(&mut v.width_px, 100.0, 8000.0);
            sane_opt(&mut v.name_width, 20.0, 4000.0);
            sane_opt(&mut v.data_extra, 0.0, 4000.0);
        }
        // the collector keeps 1 to 240 minutes; the window asks for what it can answer
        self.sensors.history_minutes = self.sensors.history_minutes.clamp(1, 240);
    }
}

pub fn toggle_in(list: &mut Vec<String>, id: &str, on: bool) {
    let present = list.iter().any(|x| x == id);
    if on && !present {
        list.push(id.to_string());
    } else if !on && present {
        list.retain(|x| x != id);
    }
}

pub fn default_path() -> PathBuf {
    config_dir().join("ui.json")
}

pub fn config_dir() -> PathBuf {
    hwglance_proto::config_dir().unwrap_or_else(|| PathBuf::from(".").join("hwglance"))
}

/// The limits file the collector keeps next to ui.json.
pub fn thresholds_path() -> PathBuf {
    config_dir().join("thresholds.json")
}

pub struct Loaded {
    pub settings: Settings,
    /// The file exists but could not be parsed (it is backed up before the next write).
    pub corrupt: bool,
    pub existed: bool,
}

pub fn load(path: &Path) -> Loaded {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Settings>(&text) {
            Ok(mut settings) => {
                // A file from before views existed: keep its sizes and columns as its views.
                let has_views = serde_json::from_str::<Value>(&text).ok().is_some_and(|v| v.pointer("/processes/views").is_some());
                if !has_views {
                    settings.seed_views_from_legacy();
                }
                settings.adopt_right_arrangement();
                settings.sanitize();
                // Retired keys disappear on the next write.
                settings.appearance.extra.remove("highlights_strip");
                // The limits moved to thresholds.json: the old ones stay in this file (as an
                // unknown key, untouched) until the collector has folded them into the new file.
                if path.with_file_name("thresholds.json").exists() {
                    settings.sensors.extra.remove("thresholds");
                }
                Loaded { settings, corrupt: false, existed: true }
            }
            Err(_) => Loaded { settings: Settings::default(), corrupt: true, existed: true },
        },
        Err(_) => Loaded { settings: Settings::default(), corrupt: false, existed: false },
    }
}

pub fn to_json(s: &Settings) -> String {
    let mut text = serde_json::to_string_pretty(s).unwrap_or_else(|_| "{}".into());
    text.push('\n');
    text
}

/// Write `bytes` to `path` atomically: a temp file in the same directory, flushed to the disk,
/// then renamed over the target. A failed write (a full disk, a read-only folder) removes the temp
/// file and leaves the previous file as it was. A symlink (a dotfiles manager's) is followed, so
/// the link stays a link and its target is what changes.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(bytes)?;
        f.sync_all()
    });
    written.and_then(|()| std::fs::rename(&tmp, &path)).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Save the settings (see `write_atomic`). With `backup_corrupt` the unreadable file that is
/// about to be replaced is copied to `ui.json.bad` first.
pub fn save_atomic(path: &Path, s: &Settings, backup_corrupt: bool) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if backup_corrupt && path.exists() {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bad");
        let _ = std::fs::copy(path, PathBuf::from(bak));
    }
    write_atomic(path, to_json(s).as_bytes())
}

/// Why a save failed, in words: what is wrong with the settings folder or the disk, and where.
pub fn save_error_text(e: &std::io::Error, path: &Path) -> String {
    let dir = path.parent().map_or_else(|| path.display().to_string(), |d| d.display().to_string());
    // the errno values, which std maps to kinds that are not all stable
    match e.raw_os_error() {
        Some(28) => format!("the disk is full ({dir})"),
        Some(122) => format!("the disk quota is used up ({dir})"),
        Some(30) => format!("the disk is read-only ({dir})"),
        Some(13) | Some(1) => format!("{dir} is not writable for this user"),
        Some(2) => format!("{dir} cannot be created"),
        _ => format!("{e} ({dir})"),
    }
}

pub fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Poll the file's mtime every 2 s on its own thread; sets `flag` and wakes the UI on a change
/// (the app compares against its own last write to ignore echoes).
pub fn spawn_watcher(ctx: egui::Context, path: PathBuf, flag: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("settings-watch".into())
        .spawn(move || {
            let mut last = mtime(&path);
            loop {
                std::thread::sleep(Duration::from_secs(2));
                let now = mtime(&path);
                if now != last {
                    last = now;
                    flag.store(true, Ordering::Relaxed);
                    ctx.request_repaint();
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hwglance-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn defaults_match_the_schema() {
        let v: Value = serde_json::from_str(&to_json(&Settings::default())).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["appearance"]["font_size"], 14.5);
        assert_eq!(v["appearance"]["font"], "atkinson");
        assert_eq!(v["appearance"]["theme"], "system");
        assert!(v["appearance"].get("highlights_strip").is_none(), "the strip is gone");
        assert_eq!(v["sensors"]["history_minutes"], 30);
        assert_eq!(v["alerts"]["level"], "critical");
        assert_eq!(v["alerts"]["scope"], "temperatures");
        assert_eq!(v["alerts"]["cooldown_minutes"], 10);
        assert_eq!(v["layout"]["auto"], true);
        assert_eq!(v["layout"]["process_panel_fraction"], 0.45);
        assert_eq!(v["processes"]["sort"]["column"], "cpu");
        assert_eq!(v["processes"]["sort"]["descending"], true);
        assert_eq!(v["tray"]["popup_process_count"], 8);
        assert!(v["tray"].get("bar_values").is_none(), "the bar shows a single icon now");
        let popup = v["tray"]["popup_values"].as_array().unwrap();
        assert_eq!(popup.len(), 12);
        assert_eq!(popup[3], "cpu/clock/max");
        assert_eq!(popup[11], "memory/temp/summary");
    }

    #[test]
    fn round_trip_keeps_unknown_keys_and_ids() {
        let text = r#"{
          "version": 1,
          "future_top": {"a": 1},
          "appearance": {"font_size": 16.0, "highlights_strip": false, "font": "omarchy", "theme_hint": "x"},
          "sensors": {
            "highlighted": ["cpu/temp/tctl", "gone/sensor"],
            "hidden": ["board/superio/auxtin2"],
            "thresholds": {"cpu/usage/total": {"warn": 85.0, "crit": 95.0},
                           "fan/pump": {"warn": 800.0, "crit": 300.0, "below": true}},
            "summary_overrides": {"cpu/clock": "cpu/clock/avg"}
          },
          "layout": {"auto": false, "columns": [{"width": 480.0, "cards": ["cpu", "gpu0"]}],
                     "collapsed_sections": {"cpu/thread": true}},
          "tray": {"popup_sort": "gpu", "bar_values": ["cpu/temp/tctl"], "popup_values": ["cpu/usage/total"]},
          "alerts": {"level": "warning", "scope": "all", "cooldown_minutes": 3}
        }"#;
        let s: Settings = serde_json::from_str(text).unwrap();
        assert_eq!(s.appearance.font_size, 16.0);
        assert_eq!(s.appearance.font, "omarchy");
        assert_eq!(s.alerts.level, "warning");
        assert_eq!(s.alerts.cooldown_minutes, 3);
        // keys this version no longer edits survive, and the old popup list is not migrated
        assert_eq!(s.tray.extra["bar_values"][0], "cpu/temp/tctl");
        assert_eq!(s.tray.popup_values, vec!["cpu/usage/total"]);
        assert_eq!(s.sensors.history_minutes, 30, "missing history_minutes takes the default");
        // the old limits are an unknown key now: they stay in the file, untouched, for the collector
        assert_eq!(s.sensors.extra["thresholds"]["fan/pump"]["below"], true);
        assert_eq!(s.sensors.extra["thresholds"]["cpu/usage/total"]["warn"], 85.0);
        assert_eq!(s.tray.popup_sort, "gpu");
        // fields missing from the file take their defaults
        assert_eq!(s.processes.resort_seconds, 3.0);

        let back: Settings = serde_json::from_str(&to_json(&s)).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.sensors.highlighted, vec!["cpu/temp/tctl", "gone/sensor"]);
        assert_eq!(back.extra["future_top"]["a"], 1);
        assert_eq!(back.appearance.extra["theme_hint"], "x");
        // the retired strip key is dropped when the file is loaded
        let path = tmpdir("retired").join("ui.json");
        std::fs::write(&path, text).unwrap();
        let loaded = load(&path);
        assert!(!loaded.settings.appearance.extra.contains_key("highlights_strip"));
        let rewritten = to_json(&loaded.settings);
        assert!(!rewritten.contains("highlights_strip"));
        assert!(rewritten.contains("theme_hint"));
        // they are written back as they were until the collector's file exists
        let v: Value = serde_json::from_str(&to_json(&s)).unwrap();
        assert!(v["sensors"]["thresholds"]["cpu/usage/total"].get("below").is_none());
        assert_eq!(v["sensors"]["thresholds"]["fan/pump"]["below"], true);
        assert!(rewritten.contains("\"thresholds\""));
        // once thresholds.json exists they have been folded in, and the window drops them
        let dir = tmpdir("moved");
        std::fs::write(dir.join("ui.json"), text).unwrap();
        std::fs::write(dir.join("thresholds.json"), "{}").unwrap();
        let moved = load(&dir.join("ui.json"));
        assert!(!moved.settings.sensors.extra.contains_key("thresholds"));
        assert!(!to_json(&moved.settings).contains("\"thresholds\""));
        assert_eq!(moved.settings.tray.popup_sort, "gpu", "nothing else is touched");
    }

    #[test]
    fn the_users_real_file_loads_intact() {
        let text = r#"{"version":1,"appearance":{"font_size":14.5,"highlights_strip":true},
          "sensors":{"highlighted":["cpu/temp/tctl","gpu0/load/core","gpu0/temp/hotspot"],"hidden":[],"hidden_sections":[],"hidden_cards":[],"thresholds":{},"summary_overrides":{}},
          "layout":{"auto":false,"columns":[{"width":461.0,"cards":["cpu","memory"]},{"width":461.0,"cards":["gpu0"]},{"width":480.0,"cards":["disk/2514074033BD10771265"]}],
            "collapsed_cards":[],"collapsed_sections":{"cpu/clock":true,"cpu/temp":false},"section_order":{},"reading_order":{},"process_panel_fraction":0.43575704,"process_panel_visible":true},
          "processes":{"hold_order_on_hover":true,"resort_seconds":2.0,"smoothing_seconds":3.0,"sort":{"column":"cpu","descending":true},"collapsed_sections":{}},
          "tray":{"bar_values":["cpu/temp/tctl","gpu0/temp/core"],"popup_values":["cpu/usage/total","cpu/temp/tctl"],"popup_process_count":8,"popup_sort":"cpu"}}"#;
        let s: Settings = serde_json::from_str(text).unwrap();
        assert_eq!(s.sensors.highlighted.len(), 3);
        assert!(!s.layout.auto);
        assert_eq!(s.layout.columns.len(), 3);
        assert_eq!(s.layout.collapsed_sections["cpu/clock"], true);
        assert_eq!(s.processes.resort_seconds, 2.0);
        assert!((s.layout.process_panel_fraction - 0.43575704).abs() < 1e-6);
        assert_eq!(s.appearance.font, "atkinson", "a missing font key means the default");
        assert_eq!(s.appearance.theme, "system", "a missing theme key means follow the system");
        assert_eq!(s.alerts, AlertPrefs::default());
        assert_eq!(s.tray.popup_values.len(), 2);
        assert!(s.tray.extra.contains_key("bar_values"));
    }

    #[test]
    fn a_damaged_number_never_reaches_the_window() {
        let d = tmpdir("sanitize");
        let path = d.join("ui.json");
        std::fs::write(
            &path,
            r#"{"appearance":{"font_size":400},
                "layout":{"auto":false,"columns":[{"width":-5,"cards":["cpu"]},{"width":99999,"cards":[]}],"process_panel_fraction":7,"process_panel_width_fraction":-1},
                "processes":{"resort_seconds":1e30,"smoothing_seconds":-3,
                  "views":{"bottom_compact":{"size":0.0,"width_px":-40,"name_width":1e9,"data_extra":-7,"columns":["cpu"]}}},
                "sensors":{"history_minutes":4000000000}}"#,
        )
        .unwrap();
        let l = load(&path);
        assert!(!l.corrupt, "the file parses, so only its numbers are wrong");
        let s = l.settings;
        assert_eq!(s.processes.resort_seconds, 300.0, "1e30 seconds is cut to the longest sane wait");
        assert!((10.0..=24.0).contains(&s.appearance.font_size));
        assert!(s.layout.columns.iter().all(|c| (crate::layout::MIN_EXPLICIT_W..=crate::layout::MAX_EXPLICIT_W).contains(&c.width)));
        assert!((0.1..=0.85).contains(&s.layout.process_panel_fraction));
        assert!((0.2..=0.85).contains(&s.layout.process_panel_width_fraction));
        assert!((0.0..=300.0).contains(&s.processes.resort_seconds));
        assert!((0.0..=60.0).contains(&s.processes.smoothing_seconds));
        // a sane Duration comes out of every one of them
        let _ = std::time::Duration::from_secs_f32(s.processes.resort_seconds);
        let v = &s.processes.views.bottom_compact;
        assert!(v.size.is_none_or(|x| (0.1..=0.85).contains(&x)));
        assert!(v.width_px.is_none_or(|x| (100.0..=8000.0).contains(&x)));
        assert!(v.name_width.is_none_or(|x| (20.0..=4000.0).contains(&x)));
        assert_eq!(v.data_extra, Some(0.0), "a negative extra width is none");
        assert!((1..=240).contains(&s.sensors.history_minutes));
    }

    #[test]
    fn sanitize_leaves_a_sane_file_alone_and_fixes_non_finite_numbers() {
        let mut s = Settings::default();
        s.layout.columns = vec![ColumnPref { width: 461.0, cards: vec!["cpu".into()] }];
        s.processes.views.right_compact.width_px = Some(512.0);
        let before = s.clone();
        s.sanitize();
        assert_eq!(s, before);
        s.appearance.font_size = f32::NAN;
        s.processes.resort_seconds = f32::INFINITY;
        s.layout.process_panel_fraction = f32::NEG_INFINITY;
        s.processes.views.bottom_expanded.size = Some(f32::NAN);
        s.layout.columns[0].width = f32::NAN;
        s.sanitize();
        assert_eq!(s.appearance.font_size, DEFAULT_FONT_SIZE);
        assert_eq!(s.processes.resort_seconds, 3.0);
        assert!(s.processes.views.bottom_expanded.size.is_none());
        assert_eq!(s.layout.columns[0].width, crate::layout::NEW_COL_W);
        assert!((0.1..=0.85).contains(&s.layout.process_panel_fraction));
    }

    #[test]
    fn a_failed_save_leaves_the_old_file_and_no_temp_file() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmpdir("readonly");
        let path = d.join("ui.json");
        let mut s = Settings::default();
        s.appearance.font_size = 18.0;
        save_atomic(&path, &s, false).unwrap();
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o500)).unwrap();
        // a user that may write anyway (root) cannot show a refusal
        let may_write = std::fs::File::create(d.join("probe")).is_ok();
        s.appearance.font_size = 20.0;
        let r = save_atomic(&path, &s, false);
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
        if may_write {
            return;
        }
        let e = r.expect_err("a read-only folder refuses the save");
        assert!(save_error_text(&e, &path).contains("not writable"), "{}", save_error_text(&e, &path));
        assert_eq!(load(&path).settings.appearance.font_size, 18.0, "the previous file is intact");
        let leftovers: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".tmp")).collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn saving_through_a_symlink_keeps_the_link_and_changes_its_target() {
        let d = tmpdir("symlink");
        let real = d.join("dotfiles").join("ui.json");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        let link = d.join("ui.json");
        let mut s = Settings::default();
        s.appearance.font_size = 16.0;
        save_atomic(&real, &s, false).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        s.appearance.font_size = 19.0;
        save_atomic(&link, &s, false).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "still a link");
        assert_eq!(load(&real).settings.appearance.font_size, 19.0, "the target changed");
        assert_eq!(load(&link).settings.appearance.font_size, 19.0);
    }

    #[test]
    fn save_errors_are_told_in_words() {
        let p = Path::new("/home/x/.config/hwglance/ui.json");
        let t = |code| save_error_text(&std::io::Error::from_raw_os_error(code), p);
        assert!(t(28).starts_with("the disk is full"), "{}", t(28));
        assert!(t(30).starts_with("the disk is read-only"));
        assert!(t(13).contains("/home/x/.config/hwglance"));
        assert!(t(122).contains("quota"));
    }

    #[test]
    fn atomic_save_and_load() {
        let dir = tmpdir("save");
        let path = dir.join("sub").join("ui.json");
        let missing = load(&path);
        assert!(!missing.existed && !missing.corrupt);
        let mut s = Settings::default();
        s.sensors.hidden.push("a/b".into());
        save_atomic(&path, &s, false).unwrap();
        let l = load(&path);
        assert!(l.existed && !l.corrupt);
        assert_eq!(l.settings, s);
        // no temp files left behind
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn corrupt_file_is_flagged_and_backed_up() {
        let dir = tmpdir("corrupt");
        let path = dir.join("ui.json");
        std::fs::write(&path, "{ not json").unwrap();
        let l = load(&path);
        assert!(l.corrupt);
        save_atomic(&path, &l.settings, true).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("ui.json.bad")).unwrap(), "{ not json");
        assert!(!load(&path).corrupt);
    }

    #[test]
    fn the_theme_setting_round_trips() {
        assert_eq!(Settings::default().appearance.theme, "system");
        let path = tmpdir("theme").join("ui.json");
        for theme in ["dark", "light", "system"] {
            let mut s = Settings::default();
            s.appearance.theme = theme.into();
            save_atomic(&path, &s, false).unwrap();
            let back = load(&path).settings;
            assert_eq!(back.appearance.theme, theme);
            assert_eq!(back, s);
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"theme\": \"system\""), "{text}");
        // an old file without the key, and the reset, both mean "system"
        let mut s: Settings = serde_json::from_str(r#"{"version":1,"appearance":{"font_size":16.0}}"#).unwrap();
        assert_eq!(s.appearance.theme, "system");
        s.appearance.theme = "light".into();
        s.reset_customisations();
        assert_eq!(s.appearance.theme, "system");
    }

    #[test]
    fn reset_keeps_tray_and_toggle_helper() {
        let mut s = Settings::default();
        s.tray.popup_sort = "memory".into();
        s.sensors.hidden.push("x".into());
        s.appearance.font_size = 20.0;
        s.alerts.level = "off".into();
        s.reset_customisations();
        assert_eq!(s.tray.popup_sort, "memory");
        assert_eq!(s.alerts.level, "off");
        assert!(s.sensors.hidden.is_empty());
        assert_eq!(s.appearance.font_size, DEFAULT_FONT_SIZE);
        let mut l = vec![];
        toggle_in(&mut l, "a", true);
        toggle_in(&mut l, "a", true);
        assert_eq!(l, vec!["a"]);
        toggle_in(&mut l, "a", false);
        assert!(l.is_empty());
    }

    #[test]
    fn columns_and_dock_are_persisted_with_their_defaults() {
        let v: Value = serde_json::from_str(&to_json(&Settings::default())).unwrap();
        let cols: Vec<&str> = v["processes"]["views"]["bottom_compact"]["columns"].as_array().unwrap().iter().map(|c| c.as_str().unwrap()).collect();
        assert_eq!(cols, ["pid", "cpu", "gpu", "vram", "memory", "disk", "location"]);
        assert_eq!(v["layout"]["process_dock"], "bottom");
        assert_eq!(v["layout"]["process_panel_width_fraction"], 0.42);
        assert!(v["layout"].get("auto_right").is_none() && v["layout"].get("columns_right").is_none(), "one arrangement for every dock");

        // a round trip keeps a changed column list (in the order saved) and the dock
        let dir = tmpdir("columns");
        let path = dir.join("ui.json");
        let mut s = Settings::default();
        let cols = crate::model::toggle_column(&s.current_view().columns, crate::model::SortKey::User, true);
        s.current_view_mut().columns = cols;
        s.layout.set_dock(Dock::Right);
        s.layout.process_panel_width_fraction = 0.5;
        save_atomic(&path, &s, false).unwrap();
        let back = load(&path).settings;
        assert_eq!(back.processes.views.bottom_compact.columns, ["pid", "cpu", "gpu", "vram", "memory", "disk", "user", "location"]);
        assert_eq!(back.processes.views.right_compact.columns, ["cpu", "gpu", "memory"]);
        assert_eq!(back.layout.dock(), Dock::Right);
        assert_eq!(back.layout.process_panel_width_fraction, 0.5, "the old key is still written and read");
        // an older file without any of these keys gets the defaults
        std::fs::write(&path, r#"{"version":1,"layout":{"auto":true},"processes":{"sort":{"column":"cpu","descending":true}}}"#).unwrap();
        let old = load(&path).settings;
        assert_eq!(old.layout.dock(), Dock::Bottom);
        assert_eq!(old.processes.views.bottom_compact.columns, Settings::default().processes.columns);
        // an unknown dock name means the bottom
        let mut l = LayoutPrefs::default();
        l.process_dock = "sideways".into();
        assert_eq!(l.dock(), Dock::Bottom);
    }

    #[test]
    fn every_dock_and_view_shares_one_arrangement() {
        use crate::layout;
        let cards: Vec<(String, usize)> = ["cpu", "gpu0", "memory", "board"].iter().map(|c| (c.to_string(), 8)).collect();
        let mut l = LayoutPrefs::default();
        let arr = layout::arrangement(&l, &cards, 1500.0, 1.0, false);
        layout::freeze(&mut l, &arr);
        assert!(!l.auto && !l.columns.is_empty());
        let bottom_columns = l.columns.clone();

        // docking right shows the same columns
        l.set_dock(Dock::Right);
        assert_eq!(l.columns, bottom_columns);
        l.set_view(PanelView::Expanded);
        assert_eq!(l.columns, bottom_columns);
        // an edit made there is the edit everywhere
        layout::move_card(&mut l, "board", 0, 0);
        let edited = l.columns.clone();
        assert_ne!(edited, bottom_columns);
        assert!(edited[0].cards.contains(&"board".to_string()));
        l.set_dock(Dock::Bottom);
        assert_eq!(l.columns, edited);
        // and so is a reset
        layout::reset(&mut l);
        l.set_dock(Dock::Right);
        assert!(l.auto && l.columns.is_empty());
    }

    fn write_layout(name: &str, layout: &str) -> Loaded {
        let dir = tmpdir(name);
        let path = dir.join("ui.json");
        std::fs::write(&path, format!(r#"{{"version":1,"layout":{layout}}}"#)).unwrap();
        load(&path)
    }

    #[test]
    fn an_old_right_dock_arrangement_is_adopted_when_the_bottom_one_was_automatic() {
        let loaded = write_layout(
            "adopt-right",
            r#"{"auto":true,"columns":[],"auto_right":false,"columns_right":[{"width":500.0,"cards":["cpu","gpu0"]},{"width":480.0,"cards":["memory"]}]}"#,
        );
        let l = &loaded.settings.layout;
        assert!(!l.auto);
        assert_eq!(l.columns.len(), 2);
        assert_eq!(l.columns[0].cards, ["cpu", "gpu0"]);
        assert_eq!(l.columns[1].width, 480.0);
        // the old keys are gone and are not written back
        assert!(!l.extra.contains_key("auto_right") && !l.extra.contains_key("columns_right"));
        let v: Value = serde_json::from_str(&to_json(&loaded.settings)).unwrap();
        assert!(v["layout"].get("auto_right").is_none() && v["layout"].get("columns_right").is_none());
    }

    #[test]
    fn an_old_right_dock_arrangement_never_replaces_a_bottom_one() {
        // the bottom arrangement was customised: it wins
        let loaded = write_layout(
            "keep-bottom",
            r#"{"auto":false,"columns":[{"width":496.0,"cards":["cpu","memory"]}],"auto_right":false,"columns_right":[{"width":500.0,"cards":["gpu0"]}]}"#,
        );
        let l = &loaded.settings.layout;
        assert!(!l.auto);
        assert_eq!(l.columns.len(), 1);
        assert_eq!(l.columns[0].cards, ["cpu", "memory"]);
        // an automatic right dock with no columns changes nothing
        let loaded = write_layout("auto-right", r#"{"auto":true,"auto_right":true,"columns_right":[]}"#);
        assert!(loaded.settings.layout.auto && loaded.settings.layout.columns.is_empty());
        // a customised right dock whose columns are empty has nothing to adopt
        let loaded = write_layout("empty-right", r#"{"auto":true,"auto_right":false,"columns_right":[]}"#);
        assert!(loaded.settings.layout.auto && loaded.settings.layout.columns.is_empty());
        assert!(!loaded.settings.layout.extra.contains_key("auto_right") && !loaded.settings.layout.extra.contains_key("columns_right"));
        let v: Value = serde_json::from_str(&to_json(&loaded.settings)).unwrap();
        assert!(v["layout"].get("auto_right").is_none() && v["layout"].get("columns_right").is_none());
    }

    #[test]
    fn the_four_views_have_their_defaults() {
        let v: Value = serde_json::from_str(&to_json(&Settings::default())).unwrap();
        assert_eq!(v["layout"]["process_view"]["bottom"], "compact");
        assert_eq!(v["layout"]["process_view"]["right"], "compact");
        let views = &v["processes"]["views"];
        assert_eq!(views["bottom_compact"]["size"], 0.3);
        assert_eq!(views["bottom_expanded"]["size"], 0.65);
        assert_eq!(views["right_expanded"]["size"], 0.55);
        assert_eq!(views["right_compact"]["width_px"], COMPACT_WIDTH_PX);
        assert!(views["right_compact"].get("size").is_none() && views["bottom_compact"].get("width_px").is_none());
        let cols = |k: &str| -> Vec<String> { views[k]["columns"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect() };
        assert_eq!(cols("right_compact"), ["cpu", "gpu", "memory"], "Name is always shown, so it is not listed");
        let full = ["pid", "cpu", "gpu", "vram", "memory", "disk", "location"];
        for k in ["bottom_compact", "bottom_expanded", "right_expanded"] {
            assert_eq!(cols(k), full, "{k}");
        }
        // the compact right view is wide enough for Name at about 220 px plus its columns
        assert!((460.0..=520.0).contains(&COMPACT_WIDTH_PX));
    }

    #[test]
    fn switching_views_is_per_dock_and_each_view_keeps_its_own_size_and_columns() {
        let mut s = Settings::default();
        assert_eq!((s.layout.dock(), s.layout.view()), (Dock::Bottom, PanelView::Compact));
        assert_eq!(s.current_view().size, Some(0.30));
        s.toggle_panel_view();
        assert_eq!(s.layout.view(), PanelView::Expanded);
        assert_eq!(s.current_view().size, Some(0.65));
        // resizing and column edits touch only the current view
        s.current_view_mut().size = Some(0.7);
        s.current_view_mut().columns = vec!["cpu".into()];
        assert_eq!(s.processes.views.bottom_compact.size, Some(0.30));
        assert_eq!(s.processes.views.bottom_compact.columns.len(), 7);
        assert_eq!(s.processes.views.bottom_expanded.columns, ["cpu"]);
        // the right dock has its own remembered view, compact by default
        s.layout.set_dock(Dock::Right);
        assert_eq!(s.layout.view(), PanelView::Compact);
        assert_eq!(s.current_view().columns, ["cpu", "gpu", "memory"]);
        s.toggle_panel_view();
        assert_eq!(s.current_view().size, Some(0.55));
        s.layout.set_dock(Dock::Bottom);
        assert_eq!(s.layout.view(), PanelView::Expanded, "the bottom dock remembers its view");
        // and the choice survives a restart
        let dir = tmpdir("views");
        let path = dir.join("ui.json");
        save_atomic(&path, &s, false).unwrap();
        let back = load(&path).settings;
        assert_eq!(back.layout.process_view, DockViews { bottom: "expanded".into(), right: "expanded".into() });
        assert_eq!(back.processes.views, s.processes.views);
    }

    #[test]
    fn a_file_from_before_views_seeds_them_from_its_old_keys() {
        let dir = tmpdir("seed");
        let path = dir.join("ui.json");
        // docked at the bottom with its own height share and columns
        std::fs::write(
            &path,
            r#"{"version":1,"layout":{"process_panel_fraction":0.5,"process_panel_width_fraction":0.38},"processes":{"columns":["pid","cpu","memory","user"]}}"#,
        )
        .unwrap();
        let s = load(&path).settings;
        assert_eq!(s.layout.view(), PanelView::Compact);
        assert_eq!(s.current_view().size, Some(0.5), "the present height is kept");
        assert_eq!(s.current_view().columns, ["pid", "cpu", "memory", "user"], "and the present columns");
        assert_eq!(s.processes.views.bottom_expanded.columns, ["pid", "cpu", "memory", "user"]);
        assert_eq!(s.processes.views.bottom_expanded.size, Some(0.65), "the other views keep their defaults");
        assert_eq!(s.processes.views.right_compact, ProcViews::default().right_compact);
        // docked at the right: the present wide panel is the expanded view
        std::fs::write(
            &path,
            r#"{"version":1,"layout":{"process_dock":"right","process_panel_width_fraction":0.38},"processes":{"columns":["cpu","gpu","location"]}}"#,
        )
        .unwrap();
        let s = load(&path).settings;
        assert_eq!((s.layout.dock(), s.layout.view()), (Dock::Right, PanelView::Expanded));
        assert_eq!(s.current_view().size, Some(0.38));
        assert_eq!(s.current_view().columns, ["cpu", "gpu", "location"]);
        assert_eq!(s.processes.views.right_compact.columns, ["cpu", "gpu", "memory"]);
        // a file that already has views is left alone
        std::fs::write(&path, r#"{"version":1,"layout":{"process_panel_fraction":0.5},"processes":{"views":{"bottom_compact":{"size":0.2}}}}"#).unwrap();
        assert_eq!(load(&path).settings.processes.views.bottom_compact.size, Some(0.2));
    }

    #[test]
    fn the_name_width_is_saved_per_view_and_absent_means_automatic() {
        let v: Value = serde_json::from_str(&to_json(&Settings::default())).unwrap();
        for k in ["bottom_compact", "bottom_expanded", "right_compact", "right_expanded"] {
            assert!(v["processes"]["views"][k].get("name_width").is_none(), "{k} starts automatic");
        }
        let mut s = Settings::default();
        s.processes.views.bottom_compact.name_width = Some(260.0);
        s.processes.views.right_expanded.name_width = Some(333.5);
        let text = to_json(&s);
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["processes"]["views"]["bottom_compact"]["name_width"], 260.0);
        assert!(v["processes"]["views"]["bottom_expanded"].get("name_width").is_none());
        let back: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.processes.views.bottom_compact.name_width, Some(260.0));
        assert_eq!(back.processes.views.right_expanded.name_width, Some(333.5));
        assert_eq!(back.processes.views.bottom_expanded.name_width, None);
        assert_eq!(back.processes.views.right_compact.name_width, None);
        // the defaults (what "Reset views" restores) hold no Name width
        assert!(ProcViews::ALL.iter().all(|(d, w)| ProcViews::default().get(*d, *w).name_width.is_none()));
    }

    #[test]
    fn the_data_columns_extra_width_is_saved_per_view_and_absent_means_none() {
        let v: Value = serde_json::from_str(&to_json(&Settings::default())).unwrap();
        for k in ["bottom_compact", "bottom_expanded", "right_compact", "right_expanded"] {
            assert!(v["processes"]["views"][k].get("data_extra").is_none(), "{k} starts with none");
        }
        let mut s = Settings::default();
        s.processes.views.right_compact.data_extra = Some(48.5);
        let text = to_json(&s);
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["processes"]["views"]["right_compact"]["data_extra"], 48.5);
        assert!(v["processes"]["views"]["right_expanded"].get("data_extra").is_none());
        let back: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.processes.views.right_compact.data_extra, Some(48.5));
        assert_eq!(back.processes.views.right_expanded.data_extra, None);
        // an older file without the key reads as none
        let old: Settings = serde_json::from_str(r#"{"processes":{"views":{"right_compact":{"name_width":250}}}}"#).unwrap();
        assert_eq!(old.processes.views.right_compact.data_extra, None);
        assert!(ProcViews::ALL.iter().all(|(d, w)| ProcViews::default().get(*d, *w).data_extra.is_none()));
    }

    #[test]
    fn the_bar_display_defaults_to_the_icon_and_unknown_values_act_as_the_icon() {
        let s = Settings::default();
        assert_eq!(s.tray.bar_display, "icon");
        let v: Value = serde_json::from_str(&to_json(&s)).unwrap();
        assert_eq!(v["tray"]["bar_display"], "icon");
        // an older file without the key reads as the icon
        let old: Settings = serde_json::from_str(r#"{"version":1,"tray":{"popup_sort":"gpu","bar_values":["cpu/temp/tctl"]}}"#).unwrap();
        assert_eq!(old.tray.bar_mode(), "icon");
        assert!(old.tray.extra.contains_key("bar_values"), "the legacy key is still preserved");
        for m in BAR_DISPLAYS {
            let mut s = Settings::default();
            s.tray.bar_display = m.to_string();
            let back: Settings = serde_json::from_str(&to_json(&s)).unwrap();
            assert_eq!(back.tray.bar_display, m);
            assert_eq!(back.tray.bar_mode(), m);
        }
        let weird: Settings = serde_json::from_str(r#"{"version":1,"tray":{"bar_display":"temperature"}}"#).unwrap();
        assert_eq!(weird.tray.bar_mode(), "icon");
        assert!(!weird.tray.extra.contains_key("bar_display"), "the key is a field, not an extra");
    }
}
