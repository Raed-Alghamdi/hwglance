//! The CSV log: the readings the user chose (or all of them), one row per interval, appended to a
//! file in the log folder. The collector writes it, so it goes on while no window is open and
//! resumes after login. The choices come from `ui.json` (see `uiprefs.rs`); the answer, including
//! any error, goes back to the window in every snapshot as [`LogStatus`].
//!
//! A file has fixed columns, so a new one is started when the set changes (a value ticked or
//! unticked, a reading that was not there before, another folder), when the local date changes
//! and when the collector restarts. A reading that disappears keeps its column, empty, until
//! then. Rows are written whole and flushed, so `tail -f` follows them and a crash loses at most
//! the row in flight. Nothing here may stop or slow the sampling: every failure becomes a status
//! and a retry, never a panic.
//!
//! A file the logger has finished with (day, columns, folder, logging off, an error) is gzipped by
//! the background worker of `logfiles.rs`, which also sweeps the folder for plain files an earlier
//! run left and deletes files past the age limit. Neither is done on the sampling thread.

use crate::logfiles::{Job, Worker};
use crate::uiprefs::LogPrefs;
use hwglance_proto::{LogStatus, Reading, Snapshot, Unit, log_folder};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// After a start the first file waits this long for the slow sources (LACT, the board and drive
/// chips) so their readings are in it from the first row; they report within a few seconds.
const START_WAIT: Duration = Duration::from_secs(10);
/// A failed folder or write is tried again this often.
const RETRY_EVERY: Duration = Duration::from_secs(10);
/// The folder is looked through at least this often (compression leftovers, the age limit).
const MAINTAIN_EVERY: Duration = Duration::from_secs(3600);

const MIB: f64 = 1024.0 * 1024.0;

/// A local date and time of day, the part of the clock the log needs. Passed in rather than read
/// here, so the tests can move the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub min: u32,
    pub sec: u32,
}

impl Local {
    pub fn of(unix_ms: u64) -> Local {
        let secs = (unix_ms / 1000) as libc::time_t;
        // SAFETY: localtime_r only writes into the tm we pass.
        let tm = unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&secs, &mut tm);
            tm
        };
        Local {
            year: tm.tm_year + 1900,
            month: tm.tm_mon as u32 + 1,
            day: tm.tm_mday as u32,
            hour: tm.tm_hour as u32,
            min: tm.tm_min as u32,
            sec: tm.tm_sec as u32,
        }
    }

    fn date(self) -> (i32, u32, u32) {
        (self.year, self.month, self.day)
    }

    /// The Time cell, which spreadsheets parse as a date and time: "2026-10-12 21:04:33".
    fn text(self) -> String {
        format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", self.year, self.month, self.day, self.hour, self.min, self.sec)
    }

    /// The file name without its number or extension: "hwglance-2026-10-12_210433".
    fn file_stem(self) -> String {
        format!("hwglance-{:04}-{:02}-{:02}_{:02}{:02}{:02}", self.year, self.month, self.day, self.hour, self.min, self.sec)
    }
}

/// What a column of a unit is written as: the text of its header, what to divide a reading by,
/// and how many decimals to keep. The one table, so a header and its numbers agree.
struct ColUnit {
    text: &'static str,
    divisor: f64,
    decimals: usize,
}

/// Memory amounts are written in MiB and transfer rates in MB/s (decimal), a flag as 1 or 0 with
/// no unit text, everything else as it is. The decimals are the ones the window shows
/// (`hwglance_proto::format_value`): a log is read for trends, and digits the window never
/// shows would only make every row longer. Transfer rates keep three because the window goes
/// down to KB/s, which is 0.001 MB/s.
fn csv_unit(unit: Unit) -> ColUnit {
    let (divisor, decimals) = match unit {
        Unit::Percent | Unit::Celsius | Unit::Watt => (1.0, 1),
        Unit::Mhz | Unit::Rpm | Unit::Mts | Unit::Flag => (1.0, 0),
        Unit::Volt => (1.0, 3),
        Unit::Bytes => (MIB, 0),
        Unit::BytesPerSec => (1e6, 3),
    };
    let text = match unit {
        Unit::Bytes => "MiB",
        Unit::BytesPerSec => "MB/s",
        other => other.limit_unit_text(),
    };
    ColUnit { text, divisor, decimals }
}

/// Rounded to `decimals`, trailing zeros trimmed: 72.4, 41, 0.125.
fn number(x: f64, decimals: usize) -> String {
    let mut t = format!("{x:.decimals$}");
    if t.contains('.') {
        let kept = t.trim_end_matches('0').trim_end_matches('.').len();
        t.truncate(kept);
    }
    if t == "-0" { "0".to_string() } else { t }
}

/// A value cell in the header's unit; empty when there is no value (or it is not a number).
fn cell(unit: Unit, value: Option<f64>) -> String {
    let Some(v) = value.filter(|v| v.is_finite()) else { return String::new() };
    if unit == Unit::Flag {
        return (if v >= 0.5 { "1" } else { "0" }).to_string();
    }
    let u = csv_unit(unit);
    number(v / u.divisor, u.decimals)
}

/// A field as RFC 4180 has it: quoted when it holds a comma, a quote or a line break.
fn field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) { format!("\"{}\"", text.replace('"', "\"\"")) } else { text.to_string() }
}

/// A reading found in the snapshot, with the card and section it sits in.
struct Found<'a> {
    group: &'a str,
    section: &'a str,
    reading: &'a Reading,
    /// A section's own summary reading (not a copy of one of its readings).
    summary: bool,
}

/// Every reading in display order, then the derived section summaries, with a lookup by id.
/// Summaries come second so that a copy never stands in for its original.
fn gather(snap: &Snapshot) -> (Vec<Found<'_>>, HashMap<&str, usize>) {
    let mut found = Vec::new();
    let mut index = HashMap::new();
    for summary in [false, true] {
        for g in &snap.sensors {
            for s in &g.sections {
                let readings = if summary { s.summary.as_slice() } else { s.readings.as_slice() };
                for r in readings {
                    if !index.contains_key(r.id.as_str()) {
                        index.insert(r.id.as_str(), found.len());
                        found.push(Found { group: &g.title, section: &s.title, reading: r, summary });
                    }
                }
            }
        }
    }
    (found, index)
}

struct Col {
    id: String,
    unit: Unit,
    group: String,
    section: String,
    label: String,
    name: String,
}

impl Col {
    fn of(f: &Found) -> Col {
        let r = f.reading;
        Col { id: r.id.clone(), unit: r.unit, group: f.group.to_string(), section: f.section.to_string(), label: r.label.clone(), name: String::new() }
    }

    /// "CPU Tctl [°C]"; with the section, "CPU Clocks Core 1 [MHz]".
    fn title(&self, with_section: bool) -> String {
        let mut n = self.group.clone();
        if with_section {
            n = format!("{n} {}", self.section);
        }
        n = format!("{n} {}", self.label);
        match csv_unit(self.unit).text {
            "" => n,
            u => format!("{n} [{u}]"),
        }
    }
}

fn counts(names: &[String]) -> HashMap<&str, usize> {
    let mut c = HashMap::new();
    for n in names {
        *c.entry(n.as_str()).or_default() += 1;
    }
    c
}

/// Give every column its header name: card, label and unit; where two would be the same, the
/// section is put in too, and where they still are, the id in parentheses (ids are unique).
fn name_columns(cols: &mut [Col]) {
    let mut names: Vec<String> = cols.iter().map(|c| c.title(false)).collect();
    let dup: Vec<bool> = names.iter().map(|n| counts(&names)[n.as_str()] > 1).collect();
    for ((n, c), d) in names.iter_mut().zip(cols.iter()).zip(dup) {
        if d {
            *n = c.title(true);
        }
    }
    let dup: Vec<bool> = names.iter().map(|n| counts(&names)[n.as_str()] > 1).collect();
    for ((n, c), d) in names.iter_mut().zip(cols.iter()).zip(dup) {
        if d {
            n.push_str(&format!(" ({})", c.id));
        }
    }
    for (c, n) in cols.iter_mut().zip(names) {
        c.name = n;
    }
}

/// The columns for a snapshot: every reading under `all`, else the ticked ids in tick order that
/// the snapshot has (a derived summary counts; an id nobody reports yet gets no column).
fn columns(prefs: &LogPrefs, found: &[Found], index: &HashMap<&str, usize>) -> Vec<Col> {
    let mut cols: Vec<Col> = if prefs.all {
        found.iter().filter(|f| !f.summary).map(Col::of).collect()
    } else {
        let mut seen = HashSet::new();
        prefs.values.iter().filter(|id| seen.insert(id.as_str())).filter_map(|id| index.get(id.as_str())).map(|&i| Col::of(&found[i])).collect()
    };
    name_columns(&mut cols);
    cols
}

/// An open log file and what it was opened for.
struct Open {
    file: File,
    path: PathBuf,
    folder: PathBuf,
    date: (i32, u32, u32),
    cols: Vec<Col>,
    known: HashSet<String>,
    /// The choices it was opened for.
    all: bool,
    wanted: Vec<String>,
    last_row: Option<Instant>,
}

impl Open {
    /// Whether rows can still go into this file: the choices, the folder and the day are as they
    /// were, no reading has appeared that has no column, and the file is still where it was
    /// (a deleted folder would swallow rows into a file nobody can see).
    fn fits(&self, prefs: &LogPrefs, folder: &Path, local: Local, found: &[Found]) -> bool {
        let same = self.all == prefs.all && (prefs.all || self.wanted == prefs.values) && self.folder == folder && self.date == local.date();
        let complete = if prefs.all {
            found.iter().filter(|f| !f.summary).all(|f| self.known.contains(&f.reading.id))
        } else {
            found.iter().filter(|f| self.wanted.contains(&f.reading.id)).all(|f| self.known.contains(&f.reading.id))
        };
        same && complete && self.path.exists()
    }
}

enum State {
    Closed,
    Open(Open),
    /// Nothing is written until `retry_at`, or until the choices change.
    Failed { error: String, retry_at: Instant, prefs: LogPrefs },
}

pub struct CsvLog {
    /// When the collector started, for the wait of the first file.
    started: Instant,
    state: State,
    /// The last error sent to stderr, so each distinct one is told once.
    told: Option<String>,
    /// Compresses finished files and applies the age limit, off this thread.
    worker: Worker,
    /// What the worker was last asked to look through, and when.
    maintained: Option<Maintained>,
}

/// The state of the folder housekeeping: asked again when any of these change, and at least hourly.
struct Maintained {
    folder: PathBuf,
    compress: bool,
    keep_days: u32,
    date: (i32, u32, u32),
    at: Instant,
}

fn io_text(e: &io::Error) -> String {
    let t = e.to_string();
    match t.rfind(" (os error") {
        Some(i) => t[..i].to_string(),
        None => t,
    }
}

/// Create the folder and a new file in it, named after the local time, with " -2", "-3" added
/// when that name is taken; write the header.
fn open_file(folder: &Path, cols: Vec<Col>, prefs: &LogPrefs, local: Local) -> Result<Open, String> {
    fs::create_dir_all(folder).map_err(|e| format!("cannot create {}: {}", folder.display(), io_text(&e)))?;
    let stem = local.file_stem();
    let mut tried = 1;
    let (file, path) = loop {
        let name = if tried == 1 { format!("{stem}.csv") } else { format!("{stem}-{tried}.csv") };
        let path = folder.join(name);
        match OpenOptions::new().append(true).create_new(true).open(&path) {
            Ok(f) => break (f, path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && tried < 1000 => tried += 1,
            Err(e) => return Err(format!("cannot create {}: {}", path.display(), io_text(&e))),
        }
    };
    let mut open = Open {
        file,
        path,
        folder: folder.to_path_buf(),
        date: local.date(),
        known: cols.iter().map(|c| c.id.clone()).collect(),
        cols,
        all: prefs.all,
        wanted: prefs.values.clone(),
        last_row: None,
    };
    let header: Vec<String> = ["Time".to_string(), "Unix time (ms)".to_string()].into_iter().chain(open.cols.iter().map(|c| field(&c.name))).collect();
    write_line(&mut open, &header.join(","))?;
    Ok(open)
}

/// One line, written whole and flushed (no fsync: a power cut may lose the last seconds).
fn write_line(open: &mut Open, line: &str) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    open.file.write_all(&bytes).and_then(|()| open.file.flush()).map_err(|e| format!("cannot write {}: {}", open.path.display(), io_text(&e)))
}

fn row(open: &Open, snap: &Snapshot, local: Local, found: &[Found], index: &HashMap<&str, usize>) -> String {
    let mut line = format!("{},{}", local.text(), snap.unix_ms);
    for c in &open.cols {
        line.push(',');
        if let Some(&i) = index.get(c.id.as_str()) {
            line.push_str(&cell(c.unit, found[i].reading.value));
        }
    }
    line
}

fn same_choice(a: &LogPrefs, b: &LogPrefs) -> bool {
    a.all == b.all && a.values == b.values && a.folder == b.folder
}

impl CsvLog {
    pub fn new(started: Instant) -> Self {
        CsvLog { started, state: State::Closed, told: None, worker: Worker::spawn(), maintained: None }
    }

    /// The file is done with (the day ended, the columns or the folder changed, logging stopped,
    /// an error): close it and, when asked, hand it to the worker to compress.
    fn finish(&mut self, compress: bool) {
        let State::Open(o) = std::mem::replace(&mut self.state, State::Closed) else { return };
        let path = o.path.clone();
        drop(o);
        *self.worker.current() = None;
        if compress {
            self.worker.send(Job::Compress(path));
        }
    }

    /// Ask the worker to look through the log folder (leftover plain files to compress, files past
    /// the age limit to delete) at a start, when the folder or either setting changes, on a new
    /// day, and at least once an hour.
    fn maintain(&mut self, folder: &Path, prefs: &LogPrefs, now: Instant, local: Local) {
        let due = self.maintained.as_ref().is_none_or(|m| {
            m.folder != folder || m.compress != prefs.compress || m.keep_days != prefs.keep_days || m.date != local.date() || now.saturating_duration_since(m.at) >= MAINTAIN_EVERY
        });
        if !due {
            return;
        }
        self.maintained = Some(Maintained { folder: folder.to_path_buf(), compress: prefs.compress, keep_days: prefs.keep_days, date: local.date(), at: now });
        if prefs.compress || prefs.keep_days > 0 {
            self.worker.send(Job::Maintain { folder: folder.to_path_buf(), compress: prefs.compress, keep_days: prefs.keep_days });
        }
    }

    /// Called once per sample with the current choices and the snapshot just taken. Opens,
    /// changes or closes the file as the choices say, appends a row when one is due, and returns
    /// the status for the snapshot (`None` while logging is off). `slow_ready`: the slow sources
    /// have reported at least once. `now` paces the rows and the retries; `local` is the local
    /// time of the snapshot.
    pub fn update(&mut self, prefs: &LogPrefs, snap: &Snapshot, slow_ready: bool, now: Instant, local: Local) -> Option<LogStatus> {
        if !prefs.active() {
            self.finish(prefs.compress);
            self.state = State::Closed;
            self.told = None;
            return None;
        }
        let status = |file: Option<&Path>, columns: usize, error: Option<String>| {
            Some(LogStatus { file: file.map(|p| p.display().to_string()), columns: columns as u32, interval_s: prefs.interval_s, error })
        };
        if let State::Failed { error, retry_at, prefs: failed_for } = &self.state {
            if now < *retry_at && same_choice(failed_for, prefs) {
                return status(None, 0, Some(error.clone()));
            }
            self.state = State::Closed;
        }
        let folder = match log_folder(&prefs.folder) {
            Ok(f) => f,
            Err(e) => return self.fail(prefs, e, now, status),
        };
        self.maintain(&folder, prefs, now, local);
        let (found, index) = gather(snap);
        if let State::Open(o) = &self.state
            && !o.fits(prefs, &folder, local, &found)
        {
            self.finish(prefs.compress);
        }
        if matches!(self.state, State::Closed) {
            let cols = columns(prefs, &found, &index);
            // Right after a start the slow sources (LACT, the board and drive chips) have not
            // reported: under "all" their readings would split the file a few seconds in, and a
            // ticked one of them would open with an empty first cell. Wait for them, or ten seconds.
            let waiting = !slow_ready && now.saturating_duration_since(self.started) < START_WAIT;
            if cols.is_empty() || waiting {
                return status(None, 0, None);
            }
            // the lock is held while the file is made, so the worker never sees it unmarked
            let mut current = self.worker.current();
            match open_file(&folder, cols, prefs, local) {
                Ok(o) => {
                    *current = Some(o.path.clone());
                    drop(current);
                    self.told = None;
                    self.state = State::Open(o);
                }
                Err(e) => {
                    drop(current);
                    return self.fail(prefs, e, now, status);
                }
            }
        }
        let State::Open(o) = &mut self.state else { return status(None, 0, None) };
        // Half a sample of slack, so a tick that comes a little early does not skip a row.
        let slack = Duration::from_millis(u64::from(snap.interval_ms)) / 2;
        let due = o.last_row.is_none_or(|t| now.saturating_duration_since(t) + slack >= Duration::from_secs(u64::from(prefs.interval_s)));
        if due {
            let line = row(o, snap, local, &found, &index);
            match write_line(o, &line) {
                Ok(()) => o.last_row = Some(now),
                Err(e) => return self.fail(prefs, e, now, status),
            }
        }
        let State::Open(o) = &self.state else { return status(None, 0, None) };
        status(Some(&o.path), o.cols.len(), None)
    }

    /// Close the file, remember why, and tell the journal once per distinct error.
    fn fail(
        &mut self,
        prefs: &LogPrefs,
        error: String,
        now: Instant,
        status: impl FnOnce(Option<&Path>, usize, Option<String>) -> Option<LogStatus>,
    ) -> Option<LogStatus> {
        if self.told.as_deref() != Some(error.as_str()) {
            eprintln!("hwglanced: log: {error}");
            self.told = Some(error.clone());
        }
        self.finish(prefs.compress);
        self.state = State::Failed { error: error.clone(), retry_at: now + RETRY_EVERY, prefs: prefs.clone() };
        status(None, 0, Some(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwglance_proto::{Section, SensorGroup, Summary};

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hwglance-csvlog-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn reading(id: &str, label: &str, unit: Unit, value: Option<f64>) -> Reading {
        Reading { id: id.into(), label: label.into(), unit, value, stats: None, note: None, desc: None, events: None, warn: None, crit: None }
    }

    /// A snapshot from (card title, section title, readings) triples, in that display order.
    fn snap_at(unix_ms: u64, parts: Vec<(&str, &str, Vec<Reading>)>) -> Snapshot {
        let mut sensors: Vec<SensorGroup> = Vec::new();
        for (group, section, readings) in parts {
            let sec = Section { id: format!("{group}/{section}"), title: section.into(), collapsed_by_default: false, readings, summary: None };
            match sensors.iter_mut().find(|g| g.title == group) {
                Some(g) => g.sections.push(sec),
                None => sensors.push(SensorGroup { id: group.to_lowercase(), title: group.into(), subtitle: None, sections: vec![sec] }),
            }
        }
        Snapshot {
            proto: hwglance_proto::PROTO_VERSION,
            seq: 1,
            unix_ms,
            stats_since_unix_ms: 0,
            interval_ms: 1000,
            history_minutes: 30,
            hostname: "h".into(),
            sensors,
            sources: vec![],
            procs: None,
            summary: Summary::default(),
            crossings: vec![],
            limit_overrides: Default::default(),
            limits_error: None,
            logging: None,
        }
    }

    fn basic(unix_ms: u64, tctl: Option<f64>) -> Snapshot {
        snap_at(
            unix_ms,
            vec![
                ("CPU", "Temperatures", vec![reading("cpu/temp/tctl", "Tctl", Unit::Celsius, tctl)]),
                ("CPU", "Load", vec![reading("cpu/usage/total", "Total", Unit::Percent, Some(12.5))]),
                ("GPU", "Load", vec![reading("gpu0/load/core", "Core", Unit::Percent, Some(3.0))]),
            ],
        )
    }

    fn local(day: u32, hour: u32, min: u32, sec: u32) -> Local {
        Local { year: 2026, month: 10, day, hour, min, sec }
    }

    fn prefs(dir: &Path, values: &[&str]) -> LogPrefs {
        LogPrefs { all: false, values: values.iter().map(|s| s.to_string()).collect(), folder: dir.display().to_string(), interval_s: 1, compress: false, keep_days: 0 }
    }

    fn files(dir: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        // a number added to a name sorts after it: "-2.csv" before ".csv" by character, so by length first
        v.sort_by_key(|p| (p.as_os_str().len(), p.clone()));
        v
    }

    fn lines(path: &Path) -> Vec<String> {
        fs::read_to_string(path).unwrap().lines().map(str::to_string).collect()
    }

    /// A log that has been running a long time, so the start wait is over.
    fn running() -> (CsvLog, Instant) {
        let t0 = Instant::now();
        (CsvLog::new(t0), t0 + Duration::from_secs(60))
    }

    fn sec(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn values_are_in_the_headers_unit_rounded_to_what_the_window_shows() {
        let c = |u, v| cell(u, Some(v));
        assert_eq!(c(Unit::Celsius, 72.375), "72.4");
        assert_eq!(c(Unit::Celsius, 41.0), "41");
        assert_eq!(c(Unit::Celsius, 41.04), "41");
        assert_eq!(c(Unit::Percent, 0.25), "0.2");
        assert_eq!(c(Unit::Percent, 12.34567), "12.3");
        assert_eq!(c(Unit::Percent, 99.96), "100");
        assert_eq!(c(Unit::Mhz, 4950.4), "4950");
        assert_eq!(c(Unit::Mhz, 4950.6), "4951");
        assert_eq!(c(Unit::Watt, 61.26), "61.3");
        assert_eq!(c(Unit::Volt, 1.0626), "1.063");
        assert_eq!(c(Unit::Volt, 1.2), "1.2");
        assert_eq!(c(Unit::Rpm, 1200.4), "1200");
        assert_eq!(c(Unit::Mts, 6000.0), "6000");
        assert_eq!(c(Unit::Bytes, 3.0 * MIB), "3");
        assert_eq!(c(Unit::Bytes, 1.5 * MIB * 1024.0), "1536");
        assert_eq!(c(Unit::Bytes, 28206.379 * MIB), "28206");
        assert_eq!(c(Unit::BytesPerSec, 2_500_000.0), "2.5");
        assert_eq!(c(Unit::BytesPerSec, 1234.0), "0.001", "the window's KB/s are 0.001 MB/s");
        assert_eq!(c(Unit::BytesPerSec, 433_700_000.0), "433.7");
        assert_eq!(c(Unit::Flag, 1.0), "1");
        assert_eq!(c(Unit::Flag, 0.0), "0");
        assert_eq!(c(Unit::Celsius, -0.04), "0", "no negative zero");
        assert_eq!(c(Unit::Celsius, -3.55), "-3.5");
        assert_eq!(c(Unit::Mhz, 0.0), "0", "whole numbers lose no zeros");
        assert_eq!(c(Unit::Mhz, 100.0), "100");
        assert_eq!(cell(Unit::Celsius, None), "");
        assert_eq!(cell(Unit::Celsius, Some(f64::NAN)), "");
        assert_eq!(cell(Unit::Celsius, Some(f64::INFINITY)), "");
        // the header and the numbers come from one place
        let u = |unit| {
            let c = csv_unit(unit);
            (c.text, c.divisor, c.decimals)
        };
        assert_eq!(u(Unit::Bytes), ("MiB", MIB, 0));
        assert_eq!(u(Unit::BytesPerSec), ("MB/s", 1e6, 3));
        assert_eq!(u(Unit::Flag).0, "");
        assert_eq!(u(Unit::Celsius), ("°C", 1.0, 1));
    }

    #[test]
    fn fields_are_quoted_the_rfc_4180_way() {
        assert_eq!(field("CPU Tctl [°C]"), "CPU Tctl [°C]");
        assert_eq!(field("a,b"), "\"a,b\"");
        assert_eq!(field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(field("two\nlines"), "\"two\nlines\"");
        assert_eq!(field("cr\rhere"), "\"cr\rhere\"");
    }

    #[test]
    fn header_names_take_the_card_label_and_unit_and_tell_twins_apart() {
        let snap = snap_at(
            1,
            vec![
                ("CPU", "Temperatures", vec![reading("cpu/temp/tctl", "Tctl", Unit::Celsius, Some(50.0))]),
                ("CPU", "Clocks", vec![reading("cpu/clock/c1", "Core 1", Unit::Mhz, Some(4000.0))]),
                ("CPU", "Throttling", vec![reading("cpu/throttle/thermal", "Thermal", Unit::Flag, Some(0.0))]),
                ("GPU", "Load", vec![reading("gpu0/load/core", "Core", Unit::Percent, Some(3.0))]),
                ("GPU", "Temperatures", vec![reading("gpu0/temp/core", "Core", Unit::Celsius, Some(40.0))]),
                // the same card, label and unit in two sections: the section tells them apart
                ("Samsung SSD 990 PRO 1TB", "Usage", vec![reading("disk/a/usage/busy", "Busy", Unit::Percent, Some(1.0))]),
                ("Samsung SSD 990 PRO 1TB", "Queue", vec![reading("disk/a/queue/busy", "Busy", Unit::Percent, Some(2.0))]),
                // and where even that is the same, the id
                ("Fans", "Case", vec![reading("fan/a", "Fan", Unit::Rpm, Some(1.0)), reading("fan/b", "Fan", Unit::Rpm, Some(2.0))]),
            ],
        );
        let (found, index) = gather(&snap);
        let all = LogPrefs { all: true, ..LogPrefs::default() };
        let names: Vec<String> = columns(&all, &found, &index).into_iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            [
                "CPU Tctl [°C]",
                "CPU Core 1 [MHz]",
                "CPU Thermal",
                "GPU Core [%]",
                "GPU Core [°C]",
                "Samsung SSD 990 PRO 1TB Usage Busy [%]",
                "Samsung SSD 990 PRO 1TB Queue Busy [%]",
                "Fans Case Fan [RPM] (fan/a)",
                "Fans Case Fan [RPM] (fan/b)",
            ]
        );
        // same label and unit under one card: the section goes in
        let snap = snap_at(
            1,
            vec![
                ("CPU", "Usage", vec![reading("cpu/usage/c1", "Core 1", Unit::Percent, None)]),
                ("CPU", "Clocks", vec![reading("cpu/clock/c1", "Core 1", Unit::Percent, None)]),
            ],
        );
        let (found, index) = gather(&snap);
        let names: Vec<String> = columns(&all, &found, &index).into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["CPU Usage Core 1 [%]", "CPU Clocks Core 1 [%]"]);
    }

    #[test]
    fn a_ticked_log_writes_its_columns_in_tick_order_with_the_time_columns_first() {
        let dir = tmpdir("ticked");
        let (mut log, now) = running();
        let p = prefs(&dir, &["gpu0/load/core", "cpu/temp/tctl"]);
        let st = log.update(&p, &basic(1_700_000_000_000, Some(72.375)), true, now, local(12, 21, 4, 33)).unwrap();
        let f = files(&dir);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].file_name().unwrap(), "hwglance-2026-10-12_210433.csv");
        assert_eq!((st.columns, st.interval_s, st.error), (2, 1, None));
        assert_eq!(st.file.as_deref(), Some(f[0].to_str().unwrap()));
        log.update(&p, &basic(1_700_000_001_000, None), true, now + sec(1), local(12, 21, 4, 34));
        assert_eq!(
            lines(&f[0]),
            [
                "Time,Unix time (ms),GPU Core [%],CPU Tctl [°C]",
                "2026-10-12 21:04:33,1700000000000,3,72.4",
                "2026-10-12 21:04:34,1700000001000,3,",
            ],
            "a reading without a value is an empty cell"
        );
    }

    #[test]
    fn a_derived_summary_can_be_ticked_and_an_unreported_id_gets_no_column_until_it_appears() {
        let dir = tmpdir("summary");
        let (mut log, now) = running();
        let mut snap = basic(1000, Some(50.0));
        snap.sensors[0].sections[0].summary = Some(reading("cpu/temp/summary", "Hottest", Unit::Celsius, Some(55.0)));
        let p = prefs(&dir, &["cpu/temp/summary", "board/not/there"]);
        let st = log.update(&p, &snap, true, now, local(12, 8, 0, 0)).unwrap();
        assert_eq!(st.columns, 1);
        let f = files(&dir);
        assert_eq!(lines(&f[0])[0], "Time,Unix time (ms),CPU Hottest [°C]");
        assert!(lines(&f[0])[1].ends_with(",55"));
        // it appears: the set changed, so a new file starts, with both
        snap.sensors[1].sections[0].readings.push(reading("board/not/there", "There", Unit::Volt, Some(1.5)));
        log.update(&p, &snap, true, now + sec(1), local(12, 8, 0, 1));
        let f = files(&dir);
        assert_eq!(f.len(), 2);
        assert_eq!(lines(&f[1])[0], "Time,Unix time (ms),CPU Hottest [°C],GPU There [V]");
    }

    #[test]
    fn a_vanished_reading_keeps_its_column_empty() {
        let dir = tmpdir("vanish");
        let (mut log, now) = running();
        let p = prefs(&dir, &["cpu/temp/tctl", "gpu0/load/core"]);
        log.update(&p, &basic(1000, Some(50.0)), true, now, local(12, 8, 0, 0));
        let mut gone = basic(2000, Some(51.0));
        gone.sensors.pop();
        log.update(&p, &gone, true, now + sec(1), local(12, 8, 0, 1));
        let f = files(&dir);
        assert_eq!(f.len(), 1, "no new file for a reading that went");
        assert_eq!(lines(&f[0])[2], "2026-10-12 08:00:01,2000,51,");
    }

    #[test]
    fn a_new_file_starts_when_a_value_is_ticked_or_unticked_or_the_folder_changes() {
        let dir = tmpdir("columns");
        let (mut log, now) = running();
        // one local second throughout, so a new file gets the number that tells it from the last
        let at = |n: u64| (basic(1000 + n * 1000, Some(50.0)), now + sec(n), local(12, 8, 0, 0));
        let run = |log: &mut CsvLog, p: &LogPrefs, n: u64| {
            let (s, t, l) = at(n);
            log.update(p, &s, true, t, l)
        };
        let mut p = prefs(&dir, &["cpu/temp/tctl"]);
        run(&mut log, &p, 0);
        run(&mut log, &p, 1);
        assert_eq!(files(&dir).len(), 1);
        p.values.push("gpu0/load/core".into());
        run(&mut log, &p, 2);
        let f = files(&dir);
        assert_eq!(f.len(), 2, "ticked");
        assert!(f[1].file_name().unwrap().to_str().unwrap().ends_with("-2.csv"), "same second, so the name gets a number: {f:?}");
        assert_eq!(lines(&f[1])[0], "Time,Unix time (ms),CPU Tctl [°C],GPU Core [%]");
        p.values.remove(0);
        run(&mut log, &p, 3);
        let f = files(&dir);
        assert_eq!(f.len(), 3, "unticked");
        assert!(f[2].file_name().unwrap().to_str().unwrap().ends_with("-3.csv"), "{f:?}");
        let other = tmpdir("columns-other");
        p.folder = other.display().to_string();
        let st = run(&mut log, &p, 4).unwrap();
        assert_eq!(files(&other).len(), 1, "the new folder gets the file");
        assert_eq!(files(&dir).len(), 3);
        assert!(st.file.unwrap().starts_with(other.to_str().unwrap()));
        // the same choices again: the same file
        run(&mut log, &p, 5);
        assert_eq!(files(&other).len(), 1);
        assert_eq!(lines(&files(&other)[0]).len(), 3);
    }

    #[test]
    fn a_new_file_starts_when_the_local_date_changes() {
        let dir = tmpdir("date");
        let (mut log, now) = running();
        let p = prefs(&dir, &["cpu/temp/tctl"]);
        log.update(&p, &basic(1000, Some(1.0)), true, now, local(12, 23, 59, 59));
        log.update(&p, &basic(2000, Some(2.0)), true, now + sec(1), local(13, 0, 0, 0));
        log.update(&p, &basic(3000, Some(3.0)), true, now + sec(2), local(13, 0, 0, 1));
        let f = files(&dir);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].file_name().unwrap(), "hwglance-2026-10-12_235959.csv");
        assert_eq!(f[1].file_name().unwrap(), "hwglance-2026-10-13_000000.csv");
        assert_eq!((lines(&f[0]).len(), lines(&f[1]).len()), (2, 3));
    }

    #[test]
    fn a_row_is_written_when_the_interval_has_passed_with_half_a_sample_of_slack() {
        let dir = tmpdir("interval");
        let (mut log, t0) = running();
        let mut p = prefs(&dir, &["cpu/temp/tctl"]);
        p.interval_s = 2;
        let ms = Duration::from_millis;
        // a 1 s sample with jitter of a few tens of ms
        for (i, t) in [0, 1030, 1990, 3010, 3980, 5020, 6000].iter().enumerate() {
            log.update(&p, &basic(1000 * (i as u64 + 1), Some(i as f64)), true, t0 + ms(*t), local(12, 8, 0, i as u32));
        }
        let rows: Vec<String> = lines(&files(&dir)[0]).into_iter().skip(1).collect();
        let values: Vec<&str> = rows.iter().map(|r| r.rsplit(',').next().unwrap()).collect();
        assert_eq!(values, ["0", "2", "4", "6"], "every second tick, the latest sample of each, not an average");
        // a 1 s log writes every sample even when a tick comes early
        let dir = tmpdir("interval1");
        let mut log = CsvLog::new(t0);
        let p = prefs(&dir, &["cpu/temp/tctl"]);
        for (i, t) in [0, 1000, 1960, 3020, 3990].iter().enumerate() {
            log.update(&p, &basic(1000 * (i as u64 + 1), Some(1.0)), true, t0 + ms(*t), local(12, 8, 0, i as u32));
        }
        assert_eq!(lines(&files(&dir)[0]).len(), 6);
        // a sample slower than the log interval writes every sample
        let dir = tmpdir("interval-slow");
        let mut log = CsvLog::new(t0);
        let p = prefs(&dir, &["cpu/temp/tctl"]);
        for i in 0..3u64 {
            let mut s = basic(1000 * (i + 1), Some(1.0));
            s.interval_ms = 5000;
            log.update(&p, &s, true, t0 + sec(5 * i), local(12, 8, 0, i as u32));
        }
        assert_eq!(lines(&files(&dir)[0]).len(), 4);
    }

    #[test]
    fn all_logs_every_reading_and_a_new_one_starts_a_file() {
        let dir = tmpdir("all");
        let (mut log, now) = running();
        let mut p = prefs(&dir, &[]);
        p.all = true;
        let st = log.update(&p, &basic(1000, Some(50.0)), true, now, local(12, 8, 0, 0)).unwrap();
        assert_eq!(st.columns, 3);
        let f = files(&dir);
        assert_eq!(lines(&f[0])[0], "Time,Unix time (ms),CPU Tctl [°C],CPU Total [%],GPU Core [%]");
        // the same readings again, one gone: still the one file
        let mut fewer = basic(2000, Some(51.0));
        fewer.sensors[0].sections.pop();
        log.update(&p, &fewer, true, now + sec(1), local(12, 8, 0, 1));
        assert_eq!(files(&dir).len(), 1);
        assert_eq!(lines(&f[0])[2], "2026-10-12 08:00:01,2000,51,,3");
        // a reading nobody had before
        let mut more = basic(3000, Some(52.0));
        more.sensors[1].sections[0].readings.push(reading("gpu0/temp/core", "Core", Unit::Celsius, Some(40.0)));
        let st = log.update(&p, &more, true, now + sec(2), local(12, 8, 0, 2)).unwrap();
        assert_eq!(st.columns, 4);
        assert_eq!(files(&dir).len(), 2);
        // all wins over the ticked ids, and a derived summary is not a column of it
        let mut with_summary = basic(4000, Some(1.0));
        with_summary.sensors[0].sections[0].summary = Some(reading("cpu/temp/summary", "Hottest", Unit::Celsius, Some(1.0)));
        let dir2 = tmpdir("all-summary");
        let mut p2 = prefs(&dir2, &["gpu0/load/core"]);
        p2.all = true;
        let st = CsvLog::new(Instant::now()).update(&p2, &with_summary, true, Instant::now(), local(12, 8, 0, 3)).unwrap();
        assert_eq!(st.columns, 3);
    }

    #[test]
    fn the_first_file_waits_for_the_slow_sources_after_a_start_but_not_longer_than_ten_seconds() {
        let dir = tmpdir("wait");
        let t0 = Instant::now();
        let mut log = CsvLog::new(t0);
        let mut p = prefs(&dir, &[]);
        p.all = true;
        let s = basic(1000, Some(1.0));
        let st = log.update(&p, &s, false, t0 + sec(1), local(12, 8, 0, 1)).unwrap();
        assert_eq!((st.file, st.columns, st.error), (None, 0, None));
        assert!(files(&dir).is_empty(), "nothing is created while waiting");
        log.update(&p, &s, false, t0 + sec(9), local(12, 8, 0, 9));
        assert!(files(&dir).is_empty());
        // the slow sources reported: go
        let st = log.update(&p, &s, true, t0 + sec(9), local(12, 8, 0, 9)).unwrap();
        assert!(st.file.is_some());
        // or ten seconds are up
        let dir = tmpdir("wait-timeout");
        let mut log = CsvLog::new(t0);
        let p = LogPrefs { all: true, folder: dir.display().to_string(), ..LogPrefs::default() };
        assert!(log.update(&p, &s, false, t0 + Duration::from_millis(9900), local(12, 8, 0, 9)).unwrap().file.is_none());
        assert!(log.update(&p, &s, false, t0 + sec(10), local(12, 8, 0, 10)).unwrap().file.is_some());
        // a ticked log waits too: one of its values may be a slow one
        let dir = tmpdir("wait-ticked");
        let mut log = CsvLog::new(t0);
        let p = prefs(&dir, &["cpu/temp/tctl"]);
        assert!(log.update(&p, &s, false, t0 + sec(2), local(12, 8, 0, 2)).unwrap().file.is_none());
        assert!(log.update(&p, &s, true, t0 + sec(3), local(12, 8, 0, 3)).unwrap().file.is_some());
        // but only right after the start: a log turned on later opens at once
        let dir = tmpdir("wait-later");
        let mut log = CsvLog::new(t0);
        let p = prefs(&dir, &["cpu/temp/tctl"]);
        assert!(log.update(&p, &s, false, t0 + sec(30), local(12, 8, 0, 30)).unwrap().file.is_some());
    }

    #[test]
    fn turning_it_off_closes_the_file_and_turning_it_on_starts_a_new_one() {
        let dir = tmpdir("off");
        let (mut log, now) = running();
        let p = prefs(&dir, &["cpu/temp/tctl"]);
        log.update(&p, &basic(1000, Some(1.0)), true, now, local(12, 8, 0, 0));
        let off = LogPrefs { values: vec![], ..p.clone() };
        assert_eq!(log.update(&off, &basic(2000, Some(1.0)), true, now + sec(1), local(12, 8, 0, 1)), None);
        log.update(&p, &basic(3000, Some(1.0)), true, now + sec(2), local(12, 8, 0, 2));
        assert_eq!(files(&dir).len(), 2);
    }

    #[test]
    fn a_folder_that_cannot_be_made_is_an_error_with_its_path_and_is_retried_every_ten_seconds() {
        let dir = tmpdir("error");
        let blocker = dir.join("not-a-folder");
        fs::write(&blocker, "x").unwrap();
        let bad = blocker.join("logs");
        let (mut log, now) = running();
        let p = prefs(&bad, &["cpu/temp/tctl"]);
        let st = log.update(&p, &basic(1000, Some(1.0)), true, now, local(12, 8, 0, 0)).unwrap();
        let err = st.error.unwrap();
        assert!(err.contains(bad.to_str().unwrap()) && err.starts_with("cannot create"), "{err}");
        assert_eq!((st.file, st.columns), (None, 0));
        // fixed on disk, but not tried again before ten seconds
        fs::remove_file(&blocker).unwrap();
        let st = log.update(&p, &basic(2000, Some(1.0)), true, now + sec(9), local(12, 8, 0, 9)).unwrap();
        assert!(st.error.is_some() && !bad.exists());
        let st = log.update(&p, &basic(3000, Some(1.0)), true, now + sec(10), local(12, 8, 0, 10)).unwrap();
        assert_eq!(st.error, None);
        assert_eq!(files(&bad).len(), 1, "the folder was made");
        // a change of choice is tried at once
        let blocker2 = dir.join("blocker2");
        fs::write(&blocker2, "x").unwrap();
        let p2 = prefs(&blocker2.join("x"), &["cpu/temp/tctl"]);
        assert!(log.update(&p2, &basic(4000, Some(1.0)), true, now + sec(11), local(12, 8, 0, 11)).unwrap().error.is_some());
        let st = log.update(&p, &basic(5000, Some(1.0)), true, now + sec(12), local(12, 8, 0, 12)).unwrap();
        assert_eq!(st.error, None, "back to the good folder without waiting");
    }

    #[test]
    fn a_relative_folder_is_an_error_and_not_resolved_against_anything() {
        let (mut log, now) = running();
        let p = LogPrefs { values: vec!["cpu/temp/tctl".into()], folder: "relative/logs".into(), ..LogPrefs::default() };
        let st = log.update(&p, &basic(1000, Some(1.0)), true, now, local(12, 8, 0, 0)).unwrap();
        assert!(st.error.unwrap().contains("not an absolute path"));
        assert!(!Path::new("relative/logs").exists());
    }

    #[test]
    fn a_failed_write_closes_the_file_and_the_next_attempt_starts_a_new_one() {
        let dir = tmpdir("writefail");
        let (mut log, now) = running();
        let p = prefs(&dir, &["cpu/temp/tctl"]);
        log.update(&p, &basic(1000, Some(1.0)), true, now, local(12, 8, 0, 0));
        // /dev/full takes the open but refuses every write with ENOSPC
        let State::Open(o) = &mut log.state else { panic!("not open") };
        o.file = OpenOptions::new().append(true).open("/dev/full").unwrap();
        let st = log.update(&p, &basic(2000, Some(1.0)), true, now + sec(1), local(12, 8, 0, 1)).unwrap();
        let err = st.error.unwrap();
        assert!(err.starts_with("cannot write") && err.contains("No space left"), "{err}");
        let st = log.update(&p, &basic(3000, Some(1.0)), true, now + sec(11), local(12, 8, 0, 11)).unwrap();
        assert_eq!(st.error, None);
        assert_eq!(files(&dir).len(), 2);
    }

    #[test]
    fn a_deleted_log_folder_is_made_again_instead_of_logging_into_the_void() {
        let dir = tmpdir("deleted");
        let folder = dir.join("logs");
        let (mut log, now) = running();
        let p = prefs(&folder, &["cpu/temp/tctl"]);
        log.update(&p, &basic(1000, Some(1.0)), true, now, local(12, 8, 0, 0));
        fs::remove_dir_all(&folder).unwrap();
        let st = log.update(&p, &basic(2000, Some(1.0)), true, now + sec(1), local(12, 8, 0, 1)).unwrap();
        assert_eq!(st.error, None);
        assert_eq!(files(&folder).len(), 1);
        assert_eq!(lines(&files(&folder)[0]).len(), 2);
    }

    #[test]
    fn a_column_name_with_a_comma_is_quoted_in_the_header() {
        let dir = tmpdir("quote");
        let (mut log, now) = running();
        let snap = snap_at(1, vec![("Disk, main", "Usage", vec![reading("disk/x", "Read \"fast\"", Unit::BytesPerSec, Some(3e6))])]);
        log.update(&prefs(&dir, &["disk/x"]), &snap, true, now, local(12, 8, 0, 0));
        let l = lines(&files(&dir)[0]);
        assert_eq!(l[0], "Time,Unix time (ms),\"Disk, main Read \"\"fast\"\" [MB/s]\"");
        assert!(l[1].ends_with(",3"));
    }
    fn gunzip(p: &Path) -> String {
        use std::io::Read;
        let mut out = String::new();
        flate2::read::GzDecoder::new(File::open(p).unwrap()).read_to_string(&mut out).unwrap();
        out
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_str().unwrap().to_string()).collect();
        v.sort();
        v
    }

    fn compressing(dir: &Path, values: &[&str]) -> LogPrefs {
        LogPrefs { compress: true, ..prefs(dir, values) }
    }

    #[test]
    fn a_finished_file_is_compressed_and_the_one_being_written_stays_plain() {
        let dir = tmpdir("gz-columns");
        let (mut log, now) = running();
        let mut p = compressing(&dir, &["cpu/temp/tctl"]);
        log.update(&p, &basic(1000, Some(50.0)), true, now, local(12, 8, 0, 0));
        log.update(&p, &basic(2000, Some(51.0)), true, now + sec(1), local(12, 8, 0, 1));
        let first = files(&dir).remove(0);
        let written = fs::read_to_string(&first).unwrap();
        p.values.push("gpu0/load/core".into());
        log.update(&p, &basic(3000, Some(52.0)), true, now + sec(2), local(12, 8, 0, 2));
        log.worker.wait();
        assert_eq!(names(&dir), ["hwglance-2026-10-12_080000.csv.gz", "hwglance-2026-10-12_080002.csv"]);
        assert_eq!(gunzip(&dir.join("hwglance-2026-10-12_080000.csv.gz")), written, "the packed file unpacks to what was written");
        assert_eq!(lines(&dir.join("hwglance-2026-10-12_080002.csv"))[0], "Time,Unix time (ms),CPU Tctl [°C],GPU Core [%]");
    }

    #[test]
    fn a_file_is_compressed_when_the_day_changes_the_folder_changes_or_logging_stops() {
        let dir = tmpdir("gz-events");
        let (mut log, now) = running();
        let p = compressing(&dir, &["cpu/temp/tctl"]);
        log.update(&p, &basic(1000, Some(1.0)), true, now, local(12, 23, 59, 59));
        log.update(&p, &basic(2000, Some(2.0)), true, now + sec(1), local(13, 0, 0, 0));
        log.worker.wait();
        assert_eq!(names(&dir), ["hwglance-2026-10-12_235959.csv.gz", "hwglance-2026-10-13_000000.csv"], "the day");
        let other = tmpdir("gz-events-other");
        let moved = compressing(&other, &["cpu/temp/tctl"]);
        log.update(&moved, &basic(3000, Some(3.0)), true, now + sec(2), local(13, 0, 0, 2));
        log.worker.wait();
        assert_eq!(names(&dir), ["hwglance-2026-10-12_235959.csv.gz", "hwglance-2026-10-13_000000.csv.gz"], "the folder");
        assert_eq!(names(&other), ["hwglance-2026-10-13_000002.csv"]);
        let off = LogPrefs { values: vec![], ..moved.clone() };
        assert_eq!(log.update(&off, &basic(4000, Some(4.0)), true, now + sec(3), local(13, 0, 0, 3)), None);
        log.worker.wait();
        assert_eq!(names(&other), ["hwglance-2026-10-13_000002.csv.gz"], "logging stopped");
    }

    #[test]
    fn a_file_closed_by_an_error_is_compressed_too() {
        let dir = tmpdir("gz-error");
        let (mut log, now) = running();
        let p = compressing(&dir, &["cpu/temp/tctl"]);
        log.update(&p, &basic(1000, Some(1.0)), true, now, local(12, 8, 0, 0));
        let State::Open(o) = &mut log.state else { panic!("not open") };
        let kept = o.path.clone();
        o.path = dir.join("hwglance-2026-10-12_080000.csv");
        let real = OpenOptions::new().append(true).open("/dev/full").unwrap();
        o.file = real;
        let st = log.update(&p, &basic(2000, Some(1.0)), true, now + sec(1), local(12, 8, 0, 1)).unwrap();
        assert!(st.error.is_some());
        log.worker.wait();
        assert_eq!(names(&dir), ["hwglance-2026-10-12_080000.csv.gz"]);
        assert!(!kept.exists());
    }

    #[test]
    fn with_compression_off_nothing_is_compressed() {
        let dir = tmpdir("gz-off");
        // a leftover of an earlier run, and a finished file
        fs::write(dir.join("hwglance-2026-10-10_080000.csv"), "old\n").unwrap();
        let (mut log, now) = running();
        let mut p = prefs(&dir, &["cpu/temp/tctl"]);
        log.update(&p, &basic(1000, Some(1.0)), true, now, local(12, 8, 0, 0));
        p.values.push("gpu0/load/core".into());
        log.update(&p, &basic(2000, Some(1.0)), true, now + sec(1), local(12, 8, 0, 1));
        log.update(&LogPrefs { values: vec![], ..p }, &basic(3000, Some(1.0)), true, now + sec(2), local(12, 8, 0, 2));
        log.worker.wait();
        assert_eq!(names(&dir).iter().filter(|n| n.ends_with(".csv")).count(), 3);
        assert!(names(&dir).iter().all(|n| !n.contains(".gz")));
    }

    #[test]
    fn the_start_sweeps_leftover_plain_files_but_not_the_open_one_or_other_names() {
        let dir = tmpdir("gz-sweep");
        fs::write(dir.join("hwglance-2026-10-10_080000.csv"), "leftover\n").unwrap();
        fs::write(dir.join("hwglance-2026-10-10_090000-2.csv"), "leftover two\n").unwrap();
        fs::write(dir.join("hwglance-backup.csv"), "mine\n").unwrap();
        fs::write(dir.join("notes.txt"), "mine\n").unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/hwglance-2026-10-09_080000.csv"), "deeper\n").unwrap();
        let (mut log, now) = running();
        let p = compressing(&dir, &["cpu/temp/tctl"]);
        log.update(&p, &basic(1000, Some(1.0)), true, now, local(12, 8, 0, 0));
        log.worker.wait();
        assert_eq!(
            names(&dir),
            ["hwglance-2026-10-10_080000.csv.gz", "hwglance-2026-10-10_090000-2.csv.gz", "hwglance-2026-10-12_080000.csv", "hwglance-backup.csv", "notes.txt", "sub"]
        );
        assert_eq!(gunzip(&dir.join("hwglance-2026-10-10_090000-2.csv.gz")), "leftover two\n");
        assert_eq!(names(&dir.join("sub")), ["hwglance-2026-10-09_080000.csv"]);
        // turning compression on later sweeps once more
        let dir = tmpdir("gz-sweep-later");
        let (mut log, now) = running();
        let mut p = prefs(&dir, &["cpu/temp/tctl"]);
        log.update(&p, &basic(1000, Some(1.0)), true, now, local(12, 8, 0, 0));
        fs::write(dir.join("hwglance-2026-10-10_080000.csv"), "leftover\n").unwrap();
        log.update(&p, &basic(2000, Some(1.0)), true, now + sec(1), local(12, 8, 0, 1));
        log.worker.wait();
        assert!(dir.join("hwglance-2026-10-10_080000.csv").exists(), "off: left alone");
        p.compress = true;
        log.update(&p, &basic(3000, Some(1.0)), true, now + sec(2), local(12, 8, 0, 2));
        log.worker.wait();
        assert!(dir.join("hwglance-2026-10-10_080000.csv.gz").exists());
        assert!(dir.join("hwglance-2026-10-12_080000.csv").exists(), "the open file stays plain");
    }

    #[test]
    fn old_logs_are_deleted_by_the_age_limit_but_not_the_open_file() {
        let dir = tmpdir("keep");
        let age = |name: &str, days: u64| {
            let t = std::time::SystemTime::now() - Duration::from_secs(days * 86_400 + 3600);
            OpenOptions::new().write(true).open(dir.join(name)).unwrap().set_modified(t).unwrap();
        };
        for n in ["hwglance-2026-09-01_080000.csv", "hwglance-2026-09-02_080000.csv.gz", "hwglance-2026-10-11_080000.csv.gz", "notes.txt", "hwglance-backup.csv"] {
            fs::write(dir.join(n), "x").unwrap();
        }
        age("hwglance-2026-09-01_080000.csv", 30);
        age("hwglance-2026-09-02_080000.csv.gz", 30);
        age("hwglance-2026-10-11_080000.csv.gz", 2);
        age("notes.txt", 30);
        age("hwglance-backup.csv", 30);
        let (mut log, now) = running();
        let mut p = LogPrefs { keep_days: 7, ..prefs(&dir, &["cpu/temp/tctl"]) };
        log.update(&p, &basic(1000, Some(1.0)), true, now, local(12, 8, 0, 0));
        // a limit of 7 days, asked for at the start: the open file is not in the first look
        p.keep_days = 8;
        let open = files(&dir).into_iter().find(|f| f.to_str().unwrap().contains("2026-10-12")).unwrap();
        let t = std::time::SystemTime::now() - Duration::from_secs(40 * 86_400);
        OpenOptions::new().write(true).open(&open).unwrap().set_modified(t).unwrap();
        log.update(&p, &basic(2000, Some(1.0)), true, now + sec(1), local(12, 8, 0, 1));
        log.worker.wait();
        assert_eq!(names(&dir), ["hwglance-2026-10-11_080000.csv.gz", "hwglance-2026-10-12_080000.csv", "hwglance-backup.csv", "notes.txt"]);
        // no limit, nothing is deleted
        let dir2 = tmpdir("keep-off");
        fs::write(dir2.join("hwglance-2026-09-01_080000.csv.gz"), "x").unwrap();
        OpenOptions::new().write(true).open(dir2.join("hwglance-2026-09-01_080000.csv.gz")).unwrap().set_modified(t).unwrap();
        let (mut log, now) = running();
        log.update(&prefs(&dir2, &["cpu/temp/tctl"]), &basic(1000, Some(1.0)), true, now, local(12, 8, 0, 0));
        log.worker.wait();
        assert!(dir2.join("hwglance-2026-09-01_080000.csv.gz").exists());
    }

    #[test]
    fn the_folder_is_looked_through_again_on_a_new_day_and_hourly_but_not_every_sample() {
        let dir = tmpdir("again");
        let (mut log, now) = running();
        let p = compressing(&dir, &["cpu/temp/tctl"]);
        log.update(&p, &basic(1000, Some(1.0)), true, now, local(12, 8, 0, 0));
        let at = log.maintained.as_ref().unwrap().at;
        log.update(&p, &basic(2000, Some(1.0)), true, now + sec(60), local(12, 8, 1, 0));
        assert_eq!(log.maintained.as_ref().unwrap().at, at, "not every sample");
        log.update(&p, &basic(3000, Some(1.0)), true, now + sec(3601), local(12, 9, 0, 1));
        assert!(log.maintained.as_ref().unwrap().at > at, "hourly");
        let at = log.maintained.as_ref().unwrap().at;
        log.update(&p, &basic(4000, Some(1.0)), true, now + sec(3602), local(13, 0, 0, 0));
        assert!(log.maintained.as_ref().unwrap().at > at, "a new day");
    }
}
