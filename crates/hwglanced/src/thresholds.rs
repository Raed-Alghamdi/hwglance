//! `~/.config/hwglance/thresholds.json`: the warning and critical limits of every reading, in a
//! file that is easy to edit by hand. This daemon is its only writer; the window asks for changes
//! with `SetThreshold` and `ResetThreshold`.
//!
//! An entry whose warn, crit and below equal the reading's defaults is "default"; anything else is
//! an override that replaces warn, crit and below as a whole, for the colours, the flags, the
//! alerts, the crossings and the bar widget alike. Values in the file are in the unit shown (GiB
//! for memory amounts, MB/s for transfer rates), the rest of the daemon works in the reading's own
//! unit.
//!
//! The file is polled by modification time. A valid file is applied without being rewritten; an
//! invalid one leaves the last good limits in force, is never overwritten, and its error goes to
//! the window. The daemon writes only to create the file, to list readings it does not list yet
//! (every user value stays, and so do entries of readings that are not reported now), and after an
//! edit from the window.

use hwglance_proto::{LimitOverride, SensorGroup, Unit};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const POLL_EVERY: Duration = Duration::from_secs(2);
/// A write that failed is tried again after this long.
const RETRY_WRITE: Duration = Duration::from_secs(30);
/// A file that changed less than this long ago and does not parse may be half written: it is
/// looked at again at the next poll before it is called invalid.
const SETTLE: Duration = Duration::from_millis(1500);
/// Overrides of the old ui.json are looked for among the readings for this long after the start
/// (the slow ones arrive a few seconds late).
const MIGRATE_FOR: Duration = Duration::from_secs(60);
const FILE_VERSION: u32 = 1;
/// Edits from clients kept between two samples.
pub const MAX_EDITS: usize = 256;

/// Warning and critical limits of one reading. `below` flips the direction (fans, pumps).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub warn: Option<f64>,
    pub crit: Option<f64>,
    pub below: bool,
}

/// A threshold override as the window used to store it in ui.json (the reading's own unit).
#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct LegacyLimits {
    #[serde(default)]
    pub warn: Option<f64>,
    #[serde(default)]
    pub crit: Option<f64>,
    #[serde(default)]
    pub below: bool,
}

/// A change asked for by the window, in the reading's own unit.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    Set { id: String, limits: Limits },
    Reset { id: String },
}

/// The informational fields of an entry (`name`, `unit`, `default`), kept for entries of readings
/// that are not reported now so the file stays whole when it is rewritten.
#[derive(Clone, Debug, Default, PartialEq)]
struct Info {
    name: Option<String>,
    unit: Option<String>,
    default: Option<(Option<f64>, Option<f64>)>,
}

/// One entry of the file, in the file's units.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    limits: Limits,
    info: Info,
}

// ---------------------------------------------------------------------------
// Reading the file
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawEntry {
    #[serde(default)]
    name: serde_json::Value,
    #[serde(default)]
    unit: serde_json::Value,
    #[serde(default)]
    default: serde_json::Value,
    #[serde(default)]
    warn: Option<f64>,
    #[serde(default)]
    crit: Option<f64>,
    #[serde(default)]
    below: Option<bool>,
}

#[derive(Deserialize)]
struct RawFile {
    #[serde(default)]
    readings: BTreeMap<String, RawEntry>,
}

fn number_or_null(v: &serde_json::Value, key: &str) -> Option<Option<f64>> {
    match v.get(key)? {
        serde_json::Value::Null => Some(None),
        n => n.as_f64().map(Some),
    }
}

/// Parse the text of the file into entries. An error says where in the text it went wrong.
fn parse(text: &str) -> Result<BTreeMap<String, Entry>, String> {
    let raw: RawFile = serde_json::from_str(text).map_err(|e| describe(&e))?;
    let mut out = BTreeMap::new();
    for (id, e) in raw.readings {
        for (key, v) in [("warn", e.warn), ("crit", e.crit)] {
            if v.is_some_and(|x| !x.is_finite()) {
                return Err(format!("{id}: {key} is not a finite number"));
            }
        }
        let default = e.default.as_object().map(|_| {
            (number_or_null(&e.default, "warn").flatten(), number_or_null(&e.default, "crit").flatten())
        });
        out.insert(
            id,
            Entry {
                limits: Limits { warn: e.warn, crit: e.crit, below: e.below.unwrap_or(false) },
                info: Info { name: e.name.as_str().map(str::to_string), unit: e.unit.as_str().map(str::to_string), default },
            },
        );
    }
    Ok(out)
}

/// serde_json's message with its position first: "line 3, column 14: invalid type ...".
fn describe(e: &serde_json::Error) -> String {
    let text = e.to_string();
    if e.line() == 0 {
        return text;
    }
    let tail = format!(" at line {} column {}", e.line(), e.column());
    let text = text.strip_suffix(&tail).unwrap_or(&text);
    format!("line {}, column {}: {}", e.line(), e.column(), text)
}

// ---------------------------------------------------------------------------
// Writing the file
// ---------------------------------------------------------------------------

/// A number the way a person writes it: 80, not 80.0.
fn num(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v}") }
}

fn opt(v: Option<f64>) -> String {
    v.map_or_else(|| "null".to_string(), num)
}

fn text(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// One row of the document: what is known about a reading when the file is written.
struct Row<'a> {
    id: &'a str,
    entry: &'a Entry,
}

/// The file's text: 2-space indent, the readings in the given order, a trailing newline.
fn render(rows: &[Row]) -> String {
    let mut out = format!("{{\n  \"version\": {FILE_VERSION},\n  \"readings\": {{");
    if rows.is_empty() {
        out.push('}');
    }
    for (i, r) in rows.iter().enumerate() {
        out.push_str(if i == 0 { "\n" } else { ",\n" });
        out.push_str(&format!("    {}: {{\n", text(r.id)));
        if let Some(n) = &r.entry.info.name {
            out.push_str(&format!("      \"name\": {},\n", text(n)));
        }
        if let Some(u) = &r.entry.info.unit {
            out.push_str(&format!("      \"unit\": {},\n", text(u)));
        }
        out.push_str(&format!("      \"warn\": {},\n", opt(r.entry.limits.warn)));
        out.push_str(&format!("      \"crit\": {},\n", opt(r.entry.limits.crit)));
        out.push_str(&format!("      \"below\": {}", r.entry.limits.below));
        if let Some((w, c)) = r.entry.info.default {
            out.push_str(&format!(",\n      \"default\": {{ \"warn\": {}, \"crit\": {} }}", opt(w), opt(c)));
        }
        out.push_str("\n    }");
    }
    if !rows.is_empty() {
        out.push_str("\n  }");
    }
    out.push_str("\n}\n");
    out
}

/// Write atomically: a temp file next to the target, flushed, given the target's permissions, then
/// renamed over it. A symlink (a dotfiles manager's) is followed, so the link stays a link.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(format!(".tmp{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let written = fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(bytes)?;
        if let Ok(old) = fs::metadata(&target) {
            f.set_permissions(old.permissions())?;
        }
        f.sync_all()
    });
    written.and_then(|()| fs::rename(&tmp, &target)).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

// ---------------------------------------------------------------------------
// The limits in force
// ---------------------------------------------------------------------------

type Stamp = (Option<SystemTime>, u64);

/// A limit of a reading's own unit as the file shows it: in the file's unit, to six decimals (a
/// conversion would otherwise leave 37.25290298461914 where 37.252903 says the same).
fn to_file(v: f64, factor: f64) -> f64 {
    ((v / factor) * 1e6).round() / 1e6
}

/// Equal up to the rounding of a conversion between units.
fn same(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => (a - b).abs() <= 1e-6 * b.abs().max(1.0),
        _ => false,
    }
}

pub struct Thresholds {
    path: Option<PathBuf>,
    writable: bool,
    /// Size and modification time of the file as last read or written.
    stamp: Option<Stamp>,
    last_poll: Option<Instant>,
    entries: BTreeMap<String, Entry>,
    /// Readings that are listed, or that the user left out of the file on purpose: only readings
    /// outside this set are added when the file does not list them.
    known: HashSet<String>,
    /// The first valid file (or its absence) has been seen: until then nothing is "left out on
    /// purpose".
    settled: bool,
    /// Make every reading present at the next pass known (the file was just reloaded).
    mark_known: bool,
    /// The file changed a moment ago and does not parse (yet): nothing is judged or written.
    hold: bool,
    error: Option<String>,
    /// The file needs writing.
    pending: bool,
    write_after: Option<Instant>,
    write_error: Option<String>,
    edits: HashMap<String, Edit>,
    /// Old overrides from ui.json still waiting for their reading to be reported.
    migrate: Option<(HashMap<String, LegacyLimits>, Instant)>,
    /// The readings whose limits are not the defaults, as of the last pass.
    overrides: BTreeMap<String, LimitOverride>,
}

impl Thresholds {
    /// `writable` is false for the one-shot dump: it reads the file but never creates or rewrites it.
    pub fn new(path: Option<PathBuf>, writable: bool) -> Self {
        Thresholds {
            path,
            writable,
            stamp: None,
            last_poll: None,
            entries: BTreeMap::new(),
            known: HashSet::new(),
            settled: false,
            mark_known: false,
            hold: false,
            error: None,
            pending: false,
            write_after: None,
            write_error: None,
            edits: HashMap::new(),
            migrate: None,
            overrides: BTreeMap::new(),
        }
    }

    pub fn overrides(&self) -> &BTreeMap<String, LimitOverride> {
        &self.overrides
    }

    /// Why the file cannot be used, with the line and column when known.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The reading's limits are "alert at or below".
    pub fn below(&self, id: &str) -> bool {
        self.overrides.get(id).is_some_and(|o| o.below)
    }

    /// Remember changes asked for by the window; they are applied at the next pass.
    pub fn queue(&mut self, edits: Vec<Edit>) {
        for e in edits {
            let id = match &e {
                Edit::Set { id, .. } | Edit::Reset { id } => id.clone(),
            };
            self.edits.insert(id, e);
        }
    }

    /// Re-read the file when it changed (checked at most every 2 s).
    fn poll(&mut self, now: Instant, legacy: &HashMap<String, LegacyLimits>) {
        if self.last_poll.is_some_and(|t| now.duration_since(t) < POLL_EVERY) {
            return;
        }
        self.last_poll = Some(now);
        let Some(path) = self.path.clone() else { return };
        let meta = match fs::metadata(&path) {
            Ok(m) => m,
            Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => {
                // Never there (the first look) or deleted: every reading is listed again with its
                // defaults. A first look takes over the overrides the window used to keep.
                if self.stamp.is_some() || !self.settled {
                    if !self.settled && !legacy.is_empty() {
                        self.migrate = Some((legacy.clone(), now + MIGRATE_FOR));
                    }
                    self.stamp = None;
                    self.entries.clear();
                    self.known.clear();
                    self.mark_known = false;
                    self.error = None;
                    self.hold = false;
                    self.settled = true;
                    self.pending = true;
                }
                return;
            }
            Err(e) => {
                self.set_error(format!("cannot read the file: {e}"));
                return;
            }
        };
        let stamp: Stamp = (meta.modified().ok(), meta.len());
        if self.stamp == Some(stamp) {
            return;
        }
        let parsed = fs::read_to_string(&path).map_err(|e| format!("cannot read the file: {e}")).and_then(|t| parse(&t));
        match parsed {
            Ok(entries) => {
                self.stamp = Some(stamp);
                self.entries = entries;
                self.error = None;
                self.hold = false;
                // Readings the user took out of the file stay out; the first file seen lists
                // whatever it lacks.
                self.known.clear();
                self.mark_known = self.settled;
                self.settled = true;
                self.migrate = None;
            }
            Err(msg) => {
                // A file written a moment ago may be half written: look again at the next poll.
                let fresh = meta.modified().ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age < SETTLE);
                self.hold = fresh;
                if !fresh {
                    self.stamp = Some(stamp);
                    self.set_error(msg);
                }
            }
        }
    }

    fn set_error(&mut self, msg: String) {
        if self.error.as_deref() != Some(msg.as_str()) {
            eprintln!("hwglanced: thresholds.json is invalid, keeping the last good limits: {msg}");
        }
        self.error = Some(msg);
    }

    /// One pass over a sample's readings: pick up outside edits and the window's, list readings the
    /// file lacks, put the limits in force into the readings and write the file when it is due.
    /// `legacy` is the overrides of the old ui.json, used once when the file is first missing.
    pub fn sync(&mut self, sensors: &mut [SensorGroup], legacy: &HashMap<String, LegacyLimits>) {
        let now = Instant::now();
        self.poll(now, legacy);
        // While the file is invalid (or being written), an edit could not be kept: it is dropped.
        let edits = std::mem::take(&mut self.edits);
        let can_list = self.error.is_none() && !self.hold;
        if self.migrate.as_ref().is_some_and(|(m, until)| m.is_empty() || now >= *until)
            && let Some((left, _)) = self.migrate.take()
            && !left.is_empty()
        {
            let mut ids: Vec<_> = left.into_keys().collect();
            ids.sort();
            eprintln!("hwglanced: ui.json threshold overrides of readings never reported were not carried over: {}", ids.join(", "));
        }
        self.overrides.clear();
        let mark_known = std::mem::take(&mut self.mark_known);
        for g in sensors.iter_mut() {
            for s in g.sections.iter_mut() {
                for r in s.readings.iter_mut().chain(s.summary.iter_mut()) {
                    if r.unit == Unit::Flag {
                        continue;
                    }
                    self.visit(r, &edits, can_list, mark_known);
                }
            }
        }
        if self.pending && can_list && self.writable {
            self.write(sensors, now);
        }
    }

    /// Settle one reading: apply an edit, list it when the file lacks it, put its limits in force.
    fn visit(&mut self, r: &mut hwglance_proto::Reading, edits: &HashMap<String, Edit>, can_list: bool, mark_known: bool) {
        let f = r.unit.limit_factor();
        let default = Limits { warn: r.warn.map(|v| to_file(v, f)), crit: r.crit.map(|v| to_file(v, f)), below: false };
        if can_list {
            if let Some(edit) = edits.get(&r.id) {
                let limits = match edit {
                    Edit::Set { limits, .. } => Limits { warn: limits.warn.map(|v| to_file(v, f)), crit: limits.crit.map(|v| to_file(v, f)), below: limits.below },
                    Edit::Reset { .. } => default,
                };
                self.entries.entry(r.id.clone()).or_insert_with(|| Entry { limits, info: Info::default() }).limits = limits;
                self.known.insert(r.id.clone());
                self.pending = true;
            } else if !self.entries.contains_key(&r.id) {
                if mark_known {
                    self.known.insert(r.id.clone());
                } else if !self.known.contains(&r.id) {
                    let moved = self.migrate.as_mut().and_then(|(m, _)| m.remove(&r.id));
                    let limits = match moved {
                        Some(o) => Limits { warn: o.warn.map(|v| to_file(v, f)), crit: o.crit.map(|v| to_file(v, f)), below: o.below },
                        None => default,
                    };
                    self.entries.insert(r.id.clone(), Entry { limits, info: Info::default() });
                    self.known.insert(r.id.clone());
                    self.pending = true;
                }
            }
        }
        let Some(entry) = self.entries.get(&r.id) else { return };
        let l = entry.limits;
        if !l.below && same(l.warn, default.warn) && same(l.crit, default.crit) {
            return;
        }
        let (warn, crit) = (l.warn.map(|v| v * f), l.crit.map(|v| v * f));
        self.overrides.insert(
            r.id.clone(),
            LimitOverride { warn, crit, below: l.below, default_warn: r.warn, default_crit: r.crit },
        );
        r.warn = warn;
        r.crit = crit;
    }

    /// Write the file: the readings reported now in the order of the sample, with the informational
    /// fields refreshed, then the entries of readings that are not reported now.
    fn write(&mut self, sensors: &[SensorGroup], now: Instant) {
        if self.write_after.is_some_and(|t| now < t) {
            return;
        }
        let Some(path) = self.path.clone() else { return };
        let mut seen = HashSet::new();
        let mut present: Vec<&str> = Vec::new();
        for g in sensors {
            for s in &g.sections {
                let own: HashSet<&str> = s.readings.iter().map(|r| r.id.as_str()).collect();
                let extra = s.summary.iter().filter(|sum| !own.contains(sum.id.as_str()));
                for r in s.readings.iter().chain(extra) {
                    if r.unit == Unit::Flag || !seen.insert(r.id.as_str()) {
                        continue;
                    }
                    present.push(&r.id);
                    let f = r.unit.limit_factor();
                    // the reading carries the limits in force; the defaults are what they replaced
                    let (warn, crit) = match self.overrides.get(&r.id) {
                        Some(o) => (o.default_warn, o.default_crit),
                        None => (r.warn, r.crit),
                    };
                    if let Some(e) = self.entries.get_mut(&r.id) {
                        e.info = Info {
                            name: Some(format!("{} \u{b7} {} \u{b7} {}", g.title, s.title, r.label)),
                            unit: Some(r.unit.limit_unit_text().to_string()),
                            default: Some((warn.map(|v| to_file(v, f)), crit.map(|v| to_file(v, f)))),
                        };
                    }
                }
            }
        }
        // Readings not reported now keep their entries as they were, after the others.
        let rest = self.entries.keys().map(String::as_str).filter(|id| !seen.contains(id));
        let rows: Vec<Row> = present
            .iter()
            .copied()
            .chain(rest)
            .filter_map(|id| self.entries.get_key_value(id).map(|(id, entry)| Row { id, entry }))
            .collect();
        match write_atomic(&path, render(&rows).as_bytes()) {
            Ok(()) => {
                self.stamp = fs::metadata(&path).ok().map(|m| (m.modified().ok(), m.len()));
                self.pending = false;
                self.write_after = None;
                self.write_error = None;
            }
            Err(e) => {
                let msg = format!("{}: {e}", path.display());
                if self.write_error.as_deref() != Some(msg.as_str()) {
                    eprintln!("hwglanced: cannot write thresholds.json: {msg}");
                }
                self.write_error = Some(msg);
                self.write_after = Some(now + RETRY_WRITE);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwglance_proto::{Reading, Section};

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hwglance-thr-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn reading(id: &str, label: &str, unit: Unit, warn: Option<f64>, crit: Option<f64>) -> Reading {
        Reading {
            id: id.into(),
            label: label.into(),
            unit,
            value: Some(1.0),
            stats: None,
            note: None,
            desc: None,
            events: None,
            warn,
            crit,
        }
    }

    fn group(readings: Vec<Reading>) -> Vec<SensorGroup> {
        vec![SensorGroup {
            id: "cpu".into(),
            title: "CPU".into(),
            subtitle: None,
            sections: vec![Section { id: "cpu/temp".into(), title: "Temperatures".into(), collapsed_by_default: false, readings, summary: None }],
        }]
    }

    fn tctl() -> Reading {
        reading("cpu/temp/tctl", "Tctl", Unit::Celsius, Some(80.0), Some(89.0))
    }

    fn none() -> HashMap<String, LegacyLimits> {
        HashMap::new()
    }

    /// Make the next `sync` look at the file again, whatever the clock says.
    fn due(t: &mut Thresholds) {
        t.last_poll = None;
    }

    fn old(path: &Path) {
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(60)).unwrap();
    }

    #[test]
    fn a_missing_file_is_created_with_every_default_and_nothing_is_overridden() {
        let d = dir("create");
        let path = d.join("thresholds.json");
        let mut t = Thresholds::new(Some(path.clone()), true);
        let mut g = group(vec![tctl(), reading("cpu/usage/total", "Total", Unit::Percent, None, None)]);
        t.sync(&mut g, &none());
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.ends_with("}\n"));
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["version"], 1);
        let e = &v["readings"]["cpu/temp/tctl"];
        assert_eq!(e["name"], "CPU \u{b7} Temperatures \u{b7} Tctl");
        assert_eq!((e["unit"].as_str(), e["warn"].as_f64(), e["crit"].as_f64(), e["below"].as_bool()), (Some("°C"), Some(80.0), Some(89.0), Some(false)));
        assert_eq!(e["default"], serde_json::json!({ "warn": 80, "crit": 89 }));
        let n = &v["readings"]["cpu/usage/total"];
        assert!(n["warn"].is_null() && n["crit"].is_null(), "a reading without limits can be given one");
        assert!(text.contains("\n    \"cpu/temp/tctl\": {\n      \"name\""), "2-space indent:\n{text}");
        assert!(t.overrides().is_empty() && t.error().is_none());
        assert_eq!((g[0].sections[0].readings[0].warn, g[0].sections[0].readings[0].crit), (Some(80.0), Some(89.0)));
        // the order is the sample's
        assert!(text.find("tctl").unwrap() < text.find("usage/total").unwrap());
    }

    #[test]
    fn the_dump_reads_the_file_but_never_creates_it() {
        let d = dir("readonly");
        let path = d.join("thresholds.json");
        let mut t = Thresholds::new(Some(path.clone()), false);
        t.sync(&mut group(vec![tctl()]), &none());
        assert!(!path.exists());
    }

    #[test]
    fn default_and_override_semantics() {
        let d = dir("semantics");
        let path = d.join("thresholds.json");
        fs::write(
            &path,
            r#"{"version":1,"readings":{
              "cpu/temp/tctl": {"warn": 80, "crit": 89, "below": false},
              "cpu/temp/ccd1": {"warn": null, "crit": 85},
              "fan/pump": {"warn": 1000, "crit": 400, "below": true},
              "cpu/temp/same_but_below": {"warn": 80, "crit": 89, "below": true}
            }}"#,
        )
        .unwrap();
        let mut t = Thresholds::new(Some(path), true);
        let mut g = group(vec![
            tctl(),
            reading("cpu/temp/ccd1", "CCD1", Unit::Celsius, Some(80.0), Some(89.0)),
            reading("fan/pump", "Pump", Unit::Rpm, None, None),
            reading("cpu/temp/same_but_below", "X", Unit::Celsius, Some(80.0), Some(89.0)),
        ]);
        t.sync(&mut g, &none());
        let r = &g[0].sections[0].readings;
        assert!(!t.overrides().contains_key("cpu/temp/tctl"), "equal to the defaults is default");
        // a partial entry replaces the defaults as a whole: no warning level any more
        assert_eq!((r[1].warn, r[1].crit), (None, Some(85.0)));
        let o = t.overrides()["cpu/temp/ccd1"];
        assert_eq!((o.warn, o.crit, o.below, o.default_warn, o.default_crit), (None, Some(85.0), false, Some(80.0), Some(89.0)));
        // a reading that has no defaults can be given limits
        assert_eq!((r[2].warn, r[2].crit), (Some(1000.0), Some(400.0)));
        assert!(t.below("fan/pump") && !t.below("cpu/temp/ccd1"));
        // below alone makes it an override
        assert!(t.below("cpu/temp/same_but_below"));
    }

    #[test]
    fn values_are_in_the_unit_shown() {
        let d = dir("units");
        let path = d.join("thresholds.json");
        fs::write(&path, r#"{"readings":{"memory/usage/used": {"warn": 12, "crit": 14}, "disk/read": {"crit": 2.5}}}"#).unwrap();
        let mut t = Thresholds::new(Some(path.clone()), true);
        let gib = 1024.0 * 1024.0 * 1024.0;
        let mut g = group(vec![
            reading("memory/usage/used", "Used", Unit::Bytes, Some(24.0 * gib), None),
            reading("disk/read", "Read", Unit::BytesPerSec, None, None),
        ]);
        t.sync(&mut g, &none());
        let r = &g[0].sections[0].readings;
        assert_eq!((r[0].warn, r[0].crit), (Some(12.0 * gib), Some(14.0 * gib)));
        assert_eq!(r[1].crit, Some(2.5e6));
        // and a reading listed by the daemon shows its defaults in GiB
        let mut t2 = Thresholds::new(Some(dir("units2").join("thresholds.json")), true);
        t2.sync(&mut group(vec![reading("memory/usage/used", "Used", Unit::Bytes, Some(24.0 * gib), None)]), &none());
        let text = fs::read_to_string(t2.path.as_ref().unwrap()).unwrap();
        assert!(text.contains("\"unit\": \"GiB\"") && text.contains("\"warn\": 24,"), "{text}");
    }

    #[test]
    fn parse_and_validate() {
        assert!(parse(r#"{"readings":{}}"#).unwrap().is_empty());
        assert!(parse("{}").unwrap().is_empty(), "a file without readings lists none");
        let ok = parse(r#"{"readings":{"a/b/c":{"warn":null,"crit":3.5,"below":null,"name":7,"unit":["x"],"default":"no","extra":1}}}"#).unwrap();
        assert_eq!(ok["a/b/c"].limits, Limits { warn: None, crit: Some(3.5), below: false }, "informational fields are not checked");
        let missing = parse(r#"{"readings":{"a/b/c":{"crit":9}}}"#).unwrap();
        assert_eq!(missing["a/b/c"].limits.warn, None);
        // bad JSON says where
        let e = parse("{\n  \"readings\": {\n    \"a/b/c\": { \"warn\": 80, }\n  }\n}").unwrap_err();
        assert!(e.starts_with("line 3, column "), "{e}");
        // wrong types
        let e = parse("{\n \"readings\": {\"a\": {\"warn\": \"hot\"}}}").unwrap_err();
        assert!(e.starts_with("line 2, column ") && e.contains("invalid type"), "{e}");
        assert!(parse(r#"{"readings":{"a":{"below":"yes"}}}"#).is_err());
        assert!(parse(r#"{"readings":{"a":5}}"#).is_err());
        assert!(parse(r#"{"readings":[]}"#).is_err());
        assert!(parse("[1]").is_err());
        assert!(parse("").is_err());
        // numbers that are not finite
        assert!(parse(r#"{"readings":{"a":{"warn":1e999}}}"#).is_err());
        assert!(parse(r#"{"readings":{"a":{"warn":NaN}}}"#).is_err());
    }

    #[test]
    fn an_outside_edit_is_picked_up_after_the_mtime_changes_and_the_file_is_not_rewritten() {
        let d = dir("reload");
        let path = d.join("thresholds.json");
        let mut t = Thresholds::new(Some(path.clone()), true);
        let mut g = group(vec![tctl()]);
        t.sync(&mut g, &none());
        let before = fs::read_to_string(&path).unwrap();
        // the user lowers the critical limit
        let edited = before.replace("\"crit\": 89,", "\"crit\": 70,");
        assert_ne!(before, edited);
        fs::write(&path, &edited).unwrap();
        old(&path);
        // not looked at again within two seconds
        let mut g = group(vec![tctl()]);
        t.sync(&mut g, &none());
        assert_eq!(g[0].sections[0].readings[0].crit, Some(89.0), "polled every 2 s, not every sample");
        due(&mut t);
        t.sync(&mut g, &none());
        assert_eq!(g[0].sections[0].readings[0].crit, Some(70.0));
        assert_eq!(t.overrides()["cpu/temp/tctl"].crit, Some(70.0));
        assert_eq!(fs::read_to_string(&path).unwrap(), edited, "a valid reload writes nothing");
        // deleting the entry gives the defaults back, and the file stays as the user left it
        let removed = "{\n  \"version\": 1,\n  \"readings\": {}\n}\n";
        fs::write(&path, removed).unwrap();
        old(&path);
        let mut g = group(vec![tctl()]);
        due(&mut t);
        t.sync(&mut g, &none());
        assert_eq!(g[0].sections[0].readings[0].crit, Some(89.0));
        assert!(t.overrides().is_empty());
        assert_eq!(fs::read_to_string(&path).unwrap(), removed, "readings the user took out are not put back");
    }

    #[test]
    fn an_invalid_file_keeps_the_last_good_limits_is_left_alone_and_reports_the_error() {
        let d = dir("invalid");
        let path = d.join("thresholds.json");
        let mut t = Thresholds::new(Some(path.clone()), true);
        let mut g = group(vec![tctl()]);
        t.sync(&mut g, &none());
        let good = fs::read_to_string(&path).unwrap().replace("\"crit\": 89,", "\"crit\": 70,");
        fs::write(&path, &good).unwrap();
        old(&path);
        due(&mut t);
        t.sync(&mut g, &none());
        assert_eq!(t.overrides()["cpu/temp/tctl"].crit, Some(70.0));

        let broken = "{\n  \"readings\": {\n    \"cpu/temp/tctl\": { \"warn\": 80 \"crit\": 70 }\n  }\n}\n";
        fs::write(&path, broken).unwrap();
        old(&path);
        let mut g = group(vec![tctl(), reading("cpu/new", "New", Unit::Celsius, Some(1.0), None)]);
        due(&mut t);
        t.sync(&mut g, &none());
        let err = t.error().expect("the error is reported");
        assert!(err.starts_with("line 3, column "), "{err}");
        assert_eq!(g[0].sections[0].readings[0].crit, Some(70.0), "the last good limits stay in force");
        assert_eq!(fs::read_to_string(&path).unwrap(), broken, "the file is not touched, not even to list a new reading");
        // an edit from the window waits out the error
        t.queue(vec![Edit::Set { id: "cpu/temp/tctl".into(), limits: Limits { warn: Some(1.0), crit: None, below: false } }]);
        due(&mut t);
        t.sync(&mut g, &none());
        assert_eq!(fs::read_to_string(&path).unwrap(), broken);
        assert_eq!(g[0].sections[0].readings[0].warn, Some(80.0), "dropped");

        // fixed: the error goes, the new values apply, and the file is as the user wrote it
        let fixed = good.replace("\"crit\": 70,", "\"crit\": 66,");
        fs::write(&path, &fixed).unwrap();
        old(&path);
        let mut g = group(vec![tctl()]);
        due(&mut t);
        t.sync(&mut g, &none());
        assert!(t.error().is_none());
        assert_eq!(g[0].sections[0].readings[0].crit, Some(66.0));
        assert_eq!(fs::read_to_string(&path).unwrap(), fixed);
    }

    #[test]
    fn a_file_that_is_invalid_at_the_start_is_not_overwritten() {
        let d = dir("invalid-start");
        let path = d.join("thresholds.json");
        fs::write(&path, "{ not json").unwrap();
        old(&path);
        let mut t = Thresholds::new(Some(path.clone()), true);
        let mut g = group(vec![tctl()]);
        t.sync(&mut g, &none());
        assert!(t.error().is_some());
        assert_eq!(g[0].sections[0].readings[0].crit, Some(89.0), "defaults while there is nothing good");
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn a_file_written_a_moment_ago_that_does_not_parse_gets_a_second_look() {
        let d = dir("settle");
        let path = d.join("thresholds.json");
        fs::write(&path, "{ \"readings\": {").unwrap();
        let mut t = Thresholds::new(Some(path.clone()), true);
        let mut g = group(vec![tctl()]);
        t.sync(&mut g, &none());
        assert!(t.error().is_none(), "maybe the editor is still writing");
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ \"readings\": {", "and it is not overwritten meanwhile");
        fs::write(&path, "{ \"readings\": {\"cpu/temp/tctl\": {\"crit\": 60}} }").unwrap();
        due(&mut t);
        t.sync(&mut g, &none());
        assert!(t.error().is_none());
        assert_eq!(g[0].sections[0].readings[0].crit, Some(60.0));
    }

    #[test]
    fn new_readings_are_appended_and_user_values_and_unknown_ids_are_kept() {
        let d = dir("append");
        let path = d.join("thresholds.json");
        fs::write(
            &path,
            r#"{"version":1,"readings":{
              "cpu/temp/tctl": {"warn": 70, "crit": 75, "below": false},
              "disk/unplugged/temp": {"name": "Gone drive", "unit": "°C", "warn": 50, "crit": 60, "below": false, "default": {"warn": 70, "crit": 80}}
            }}"#,
        )
        .unwrap();
        old(&path);
        let mut t = Thresholds::new(Some(path.clone()), true);
        let mut g = group(vec![tctl(), reading("cpu/temp/ccd1", "CCD1", Unit::Celsius, Some(80.0), Some(89.0))]);
        t.sync(&mut g, &none());
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let r = &v["readings"];
        assert_eq!((r["cpu/temp/tctl"]["warn"].as_f64(), r["cpu/temp/tctl"]["crit"].as_f64()), (Some(70.0), Some(75.0)), "the user's values stay");
        assert_eq!(r["cpu/temp/tctl"]["default"], serde_json::json!({"warn": 80, "crit": 89}), "the informational fields are refreshed");
        assert_eq!(r["cpu/temp/ccd1"]["crit"].as_f64(), Some(89.0), "the new reading is listed with its defaults");
        assert_eq!(r["disk/unplugged/temp"]["name"], "Gone drive");
        assert_eq!(r["disk/unplugged/temp"]["crit"].as_f64(), Some(60.0));
        assert_eq!(r["disk/unplugged/temp"]["default"], serde_json::json!({"warn": 70, "crit": 80}));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.find("tctl").unwrap() < text.find("ccd1").unwrap() && text.find("ccd1").unwrap() < text.find("unplugged").unwrap(), "{text}");
        assert_eq!(t.overrides()["cpu/temp/tctl"].crit, Some(75.0));

        // a reading that shows up later is added the same way, with the rest untouched
        let mut g = group(vec![tctl(), reading("cpu/temp/ccd1", "CCD1", Unit::Celsius, Some(80.0), Some(89.0)), reading("cpu/temp/late", "Late", Unit::Celsius, None, Some(90.0))]);
        t.sync(&mut g, &none());
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["readings"]["cpu/temp/late"]["crit"].as_f64(), Some(90.0));
        assert_eq!(v["readings"]["cpu/temp/tctl"]["crit"].as_f64(), Some(75.0));
        assert!(v["readings"]["disk/unplugged/temp"].is_object());
    }

    #[test]
    fn edits_from_the_window_are_applied_written_and_reset() {
        let d = dir("edits");
        let path = d.join("thresholds.json");
        let mut t = Thresholds::new(Some(path.clone()), true);
        let mut g = group(vec![tctl(), reading("fan/pump", "Pump", Unit::Rpm, None, None)]);
        t.sync(&mut g, &none());
        t.queue(vec![
            Edit::Set { id: "cpu/temp/tctl".into(), limits: Limits { warn: Some(60.0), crit: Some(70.0), below: false } },
            Edit::Set { id: "fan/pump".into(), limits: Limits { warn: Some(800.0), crit: Some(300.0), below: true } },
            Edit::Set { id: "not/reported".into(), limits: Limits { warn: Some(1.0), crit: None, below: false } },
        ]);
        t.sync(&mut g, &none());
        let r = &g[0].sections[0].readings;
        assert_eq!((r[0].warn, r[0].crit), (Some(60.0), Some(70.0)));
        assert_eq!((r[1].warn, r[1].crit, t.below("fan/pump")), (Some(800.0), Some(300.0), true));
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["readings"]["cpu/temp/tctl"]["warn"].as_f64(), Some(60.0));
        assert_eq!(v["readings"]["fan/pump"]["below"], true);
        assert!(v["readings"].get("not/reported").is_none(), "an unknown reading is ignored");
        // our own write is not read back as an outside edit
        let stamp = t.stamp;
        due(&mut t);
        t.sync(&mut g, &none());
        assert_eq!(t.stamp, stamp);
        // reset: the defaults again, the row stays in the file
        t.queue(vec![Edit::Reset { id: "cpu/temp/tctl".into() }, Edit::Reset { id: "fan/pump".into() }]);
        let mut g = group(vec![tctl(), reading("fan/pump", "Pump", Unit::Rpm, None, None)]);
        t.sync(&mut g, &none());
        assert!(t.overrides().is_empty());
        assert_eq!(g[0].sections[0].readings[0].crit, Some(89.0));
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["readings"]["cpu/temp/tctl"]["crit"].as_f64(), Some(89.0));
        assert!(v["readings"]["fan/pump"]["warn"].is_null());
        // the last of two edits to one reading wins
        t.queue(vec![
            Edit::Set { id: "cpu/temp/tctl".into(), limits: Limits { warn: Some(1.0), crit: None, below: false } },
            Edit::Set { id: "cpu/temp/tctl".into(), limits: Limits { warn: Some(2.0), crit: None, below: false } },
        ]);
        t.sync(&mut g, &none());
        assert_eq!(g[0].sections[0].readings[0].warn, Some(2.0));
    }

    #[test]
    fn an_edit_of_a_memory_amount_or_a_rate_is_written_in_the_unit_shown() {
        let d = dir("edit-units");
        let path = d.join("thresholds.json");
        let mut t = Thresholds::new(Some(path.clone()), true);
        let gib = 1024.0 * 1024.0 * 1024.0;
        let mut g = group(vec![reading("memory/usage/used", "Used", Unit::Bytes, None, None), reading("disk/read", "Read", Unit::BytesPerSec, None, None)]);
        t.sync(&mut g, &none());
        // what the window's editor sends: the shown value divided by its scale
        t.queue(vec![
            Edit::Set { id: "memory/usage/used".into(), limits: Limits { warn: Some(12.3 / (1.0 / gib)), crit: None, below: false } },
            Edit::Set { id: "disk/read".into(), limits: Limits { warn: Some(5.2 / 1e-6), crit: None, below: false } },
        ]);
        t.sync(&mut g, &none());
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"warn\": 12.3,") && text.contains("\"warn\": 5.2,"), "{text}");
        let r = &g[0].sections[0].readings;
        assert!((r[0].warn.unwrap() / gib - 12.3).abs() < 1e-5 && (r[1].warn.unwrap() / 1e6 - 5.2).abs() < 1e-5);
    }

    #[test]
    fn the_overrides_of_the_old_ui_json_are_folded_in_once() {
        let d = dir("migrate");
        let path = d.join("thresholds.json");
        let legacy: HashMap<String, LegacyLimits> = [
            ("cpu/temp/tctl".to_string(), LegacyLimits { warn: None, crit: Some(85.0), below: false }),
            ("memory/usage/used".to_string(), LegacyLimits { warn: Some(8.0 * 1073741824.0), crit: None, below: false }),
            ("gpu0/temp/hotspot".to_string(), LegacyLimits { warn: Some(90.0), crit: Some(100.0), below: false }),
        ]
        .into();
        let mut t = Thresholds::new(Some(path.clone()), true);
        let mut g = group(vec![tctl(), reading("memory/usage/used", "Used", Unit::Bytes, None, None)]);
        t.sync(&mut g, &legacy);
        let r = &g[0].sections[0].readings;
        // whole replacement, as the window showed it: no warning level any more
        assert_eq!((r[0].warn, r[0].crit), (None, Some(85.0)));
        assert_eq!(r[1].warn, Some(8.0 * 1073741824.0));
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(v["readings"]["cpu/temp/tctl"]["warn"].is_null());
        assert_eq!(v["readings"]["memory/usage/used"]["warn"].as_f64(), Some(8.0), "converted to GiB");
        // a conversion that does not come out even is written the way a person would
        let odd: HashMap<String, LegacyLimits> =
            [("memory/usage/used".to_string(), LegacyLimits { warn: Some(40_000_000_000.0), crit: None, below: false })].into();
        let mut t3 = Thresholds::new(Some(dir("migrate-odd").join("thresholds.json")), true);
        t3.sync(&mut group(vec![reading("memory/usage/used", "Used", Unit::Bytes, None, None)]), &odd);
        let text = fs::read_to_string(t3.path.as_ref().unwrap()).unwrap();
        assert!(text.contains("\"warn\": 37.252903,"), "{text}");
        // a slow reading that arrives a moment later still gets its old override
        let mut g = group(vec![tctl(), reading("gpu0/temp/hotspot", "Hotspot", Unit::Celsius, Some(85.0), Some(95.0))]);
        t.sync(&mut g, &legacy);
        assert_eq!(g[0].sections[0].readings[1].crit, Some(100.0));
        // once the file exists, ui.json is not looked at again
        let mut again = Thresholds::new(Some(path.clone()), true);
        let mut g = group(vec![reading("cpu/temp/tctl", "Tctl", Unit::Celsius, Some(80.0), Some(89.0))]);
        let other: HashMap<String, LegacyLimits> = [("cpu/temp/tctl".to_string(), LegacyLimits { warn: Some(1.0), crit: Some(2.0), below: false })].into();
        again.sync(&mut g, &other);
        assert_eq!(g[0].sections[0].readings[0].crit, Some(85.0), "the file wins");
    }

    #[test]
    fn deleting_the_file_lists_everything_with_defaults_again() {
        let d = dir("deleted");
        let path = d.join("thresholds.json");
        let mut t = Thresholds::new(Some(path.clone()), true);
        let mut g = group(vec![tctl()]);
        t.sync(&mut g, &none());
        t.queue(vec![Edit::Set { id: "cpu/temp/tctl".into(), limits: Limits { warn: None, crit: Some(50.0), below: false } }]);
        t.sync(&mut g, &none());
        assert_eq!(t.overrides()["cpu/temp/tctl"].crit, Some(50.0));
        fs::remove_file(&path).unwrap();
        due(&mut t);
        let mut g = group(vec![tctl()]);
        t.sync(&mut g, &none());
        assert!(t.overrides().is_empty());
        assert!(fs::read_to_string(&path).unwrap().contains("\"crit\": 89"));
    }

    #[test]
    fn a_write_replaces_the_file_atomically_keeps_its_permissions_and_follows_a_link() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("atomic");
        let real = d.join("real.json");
        fs::write(&real, "{}").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
        let link = d.join("thresholds.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, b"{\"readings\":{}}\n").unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link stays a link");
        assert_eq!(fs::read_to_string(&real).unwrap(), "{\"readings\":{}}\n");
        assert_eq!(fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o640);
        let names: Vec<_> = fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().all(|n| !n.contains(".tmp")), "no temp file is left: {names:?}");
        // a failed write leaves the old file
        let ro = d.join("nodir").join("deeper");
        fs::write(d.join("nodir"), "a file where a folder should be").unwrap();
        assert!(write_atomic(&ro.join("thresholds.json"), b"x").is_err());
    }

    #[test]
    fn a_failed_write_is_retried_later_and_does_not_stop_sampling() {
        let d = dir("failwrite");
        fs::write(d.join("blocker"), "x").unwrap();
        let mut t = Thresholds::new(Some(d.join("blocker").join("thresholds.json")), true);
        let mut g = group(vec![tctl()]);
        t.sync(&mut g, &none());
        assert!(t.pending && t.write_after.is_some());
        assert_eq!(g[0].sections[0].readings[0].crit, Some(89.0));
    }

    #[test]
    fn render_is_stable_and_round_trips() {
        let e = |warn, crit, below| Entry {
            limits: Limits { warn, crit, below },
            info: Info { name: Some("A \"quoted\" name".into()), unit: Some("°C".into()), default: Some((Some(80.0), None)) },
        };
        let a = e(Some(80.5), None, false);
        let b = e(None, Some(1e-7), true);
        let rows = [Row { id: "a/b", entry: &a }, Row { id: "c/d", entry: &b }];
        let text = render(&rows);
        assert_eq!(render(&rows), text);
        let back = parse(&text).unwrap();
        assert_eq!(back["a/b"], a);
        assert_eq!(back["c/d"].limits, b.limits);
        assert!(text.contains("\"default\": { \"warn\": 80, \"crit\": null }"), "{text}");
        assert_eq!(render(&[]), "{\n  \"version\": 1,\n  \"readings\": {}\n}\n");
    }
}
