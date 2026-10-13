//! Types shared by the collector daemon (`hwglanced`), the window (`hwglance`)
//! and the root helper (`hwglance-helper`).
//!
//! Wire format: newline-delimited JSON over the Unix socket at [`socket_path`].
//! The client sends [`ClientMsg`] lines; the daemon answers with [`DaemonMsg`] lines,
//! one snapshot per sample interval.
//!
//! Version 2 leaves out empty or absent fields instead of writing `null`, `""` or `0`, so every
//! optional field reads as missing (`serde(default)` here, `undefined` in QML). A client that sends
//! `slim: true` with its subscription also gets the static process fields (command line,
//! executable, owner, start time) only for processes it cannot know yet; see [`ProcInfo`].
//!
//! Version 3 adds, all optional so version 2 peers keep working: [`Snapshot::crossings`] (the
//! threshold crossings the collector recorded), [`Summary::swap_total_bytes`], and the
//! `unfocused` flag of `Subscribe` and `SetProcs` (a window that has no focus tells the collector
//! it may sample processes less often). A peer that does not know a field ignores it; a missing
//! one reads as its default (no crossings, focused).
//!
//! Version 4 moves the warning and critical limits into `thresholds.json`, which only the
//! collector writes. `Reading::warn` and `Reading::crit` are now the limits in force (the user's
//! when they differ from the defaults), [`Snapshot::limit_overrides`] says which readings are
//! customised and what their defaults are, [`Snapshot::limits_error`] reports a file the collector
//! could not read, and the window changes limits with [`ClientMsg::SetThreshold`] and
//! [`ClientMsg::ResetThreshold`] instead of writing a file itself.

pub mod crossings;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

pub const PROTO_VERSION: u32 = 4;

// ---------------------------------------------------------------------------
// Sensors
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Percent,
    Celsius,
    Mhz,
    Watt,
    Volt,
    Rpm,
    /// Memory amounts. Formatted as MiB or GiB.
    Bytes,
    /// Transfer rates. Formatted as KB/s, MB/s or GB/s (decimal, like disk tools).
    BytesPerSec,
    /// Memory transfer rate in MT/s.
    Mts,
    /// A yes/no state such as thermal throttling: 0 is no, 1 is yes. `stats.max` says whether
    /// it ever happened since the stats started, `stats.avg` the share of samples it was on.
    Flag,
}

impl Unit {
    /// How many of the reading's own units make one unit of the limits file and the editor:
    /// memory amounts are edited in GiB and transfer rates in MB/s, everything else as is.
    pub fn limit_factor(self) -> f64 {
        match self {
            Unit::Bytes => 1024.0 * 1024.0 * 1024.0,
            Unit::BytesPerSec => 1e6,
            _ => 1.0,
        }
    }

    /// The unit text written beside the limits in `thresholds.json` (informational).
    pub fn limit_unit_text(self) -> &'static str {
        match self {
            Unit::Percent => "%",
            Unit::Celsius => "°C",
            Unit::Mhz => "MHz",
            Unit::Watt => "W",
            Unit::Volt => "V",
            Unit::Rpm => "RPM",
            Unit::Mts => "MT/s",
            Unit::Bytes => "GiB",
            Unit::BytesPerSec => "MB/s",
            Unit::Flag => "",
        }
    }
}

/// Onsets of a `Unit::Flag` reading since the stats started.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlagEvents {
    /// Number of times it switched from no to yes.
    pub count: u32,
    pub last_unix_ms: u64,
    /// When it last switched from yes to no. Missing while it is on, before it ever was, and
    /// from collectors older than version 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_unix_ms: Option<u64>,
}

/// Recent values of one reading, oldest first, for the hover graph.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HistorySeries {
    pub id: String,
    pub unit: Unit,
    /// `[unix_ms, value]` pairs. Gaps (no value that sample) are simply missing.
    pub points: Vec<(u64, f32)>,
}

/// Running statistics since the daemon started or since the last reset.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    pub min: f64,
    pub max: f64,
    pub avg: f64,
    pub samples: u64,
}

impl Stats {
    pub fn first(v: f64) -> Self {
        Stats { min: v, max: v, avg: v, samples: 1 }
    }

    pub fn push(&mut self, v: f64) {
        self.samples += 1;
        self.min = self.min.min(v);
        self.max = self.max.max(v);
        self.avg += (v - self.avg) / self.samples as f64;
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Reading {
    /// Stable key, unique within a snapshot, e.g. `cpu/temp/tctl`, `gpu0/temp/hotspot`.
    pub id: String,
    pub label: String,
    pub unit: Unit,
    /// Latest value; `None` when the source did not answer this sample.
    pub value: Option<f64>,
    /// `None` until the first valid sample.
    pub stats: Option<Stats>,
    /// Hover text, e.g. where the value comes from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// One or two short sentences for the hover tooltip: what the value means, and how it
    /// differs from similar values when they could be confused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    /// For `Unit::Flag` readings: how often and when it last switched on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<FlagEvents>,
    /// The limits in force, in the reading's own unit: values at or above `warn` render as
    /// warm, at or above `crit` as hot (at or below with [`LimitOverride::below`]). They are the
    /// collector's defaults unless the reading is in [`Snapshot::limit_overrides`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crit: Option<f64>,
}

/// A reading whose limits differ from the collector's defaults, from `thresholds.json` or an edit
/// in the window. It replaces the defaults as a whole: a `None` here means no limit at that level,
/// whatever the default says. Values are in the reading's own unit.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct LimitOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crit: Option<f64>,
    /// Alert when the value is at or below the limits (fans, pumps).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub below: bool,
    /// The defaults this replaces (always "above"), for "Reset to default".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_warn: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_crit: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Section {
    /// Stable key, used by the window to remember collapsed state.
    pub id: String,
    pub title: String,
    pub collapsed_by_default: bool,
    pub readings: Vec<Reading>,
    /// Value to show on the section's header row while it is collapsed (for example the
    /// highest clock, or the busiest thread). Either a copy of one of `readings` or a derived
    /// value with its own id and stats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<Reading>,
}

/// One device block in the sensor table: CPU, GPU, Memory, a drive, Motherboard.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SensorGroup {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    pub sections: Vec<Section>,
}

/// How bad a threshold crossing got.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum CrossingLevel {
    Warn,
    Crit,
}

/// One stretch during which a reading was past a threshold, recorded by the collector so a
/// window opened (or reconnected) after the fact can say what ran hot and since when. It lasts
/// from the first sample past the warning (or critical) threshold until the value is back inside
/// it; a value that comes back within half a minute continues the same crossing.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Crossing {
    /// The reading's id, e.g. `cpu/temp/tctl`.
    pub id: String,
    /// The worst level reached.
    pub level: CrossingLevel,
    pub start_ms: u64,
    /// The most extreme value (the lowest one when `below`).
    pub peak: f64,
    pub peak_ms: u64,
    /// When it was last past the threshold. Missing while it still is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u64>,
    /// The thresholds were "alert at or below".
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub below: bool,
}

// ---------------------------------------------------------------------------
// Processes
// ---------------------------------------------------------------------------

/// Task Manager style buckets.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ProcClass {
    /// Owns at least one window (plus its helper processes).
    App,
    /// The user's own processes without a window.
    Background,
    /// Other users' processes (root services, kernel threads).
    System,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

fn default_state() -> char {
    'S'
}

fn is_sleeping(c: &char) -> bool {
    *c == 'S'
}

/// One process. `cmdline`, `exe`, `user` and `started_unix_ms` never change while the process
/// lives. A client that subscribed with `slim: true` receives them only with the first few
/// snapshots that contain the process, with every snapshot that follows a subscription or a
/// `SetProcs` and about every 30 s; otherwise they are empty (see [`ProcInfo::has_static`]) and
/// the client fills them from what it saw before, keyed by `pid` and `start_time_ticks`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProcInfo {
    pub pid: i32,
    pub ppid: i32,
    pub uid: u32,
    pub name: String,
    /// Full command line, truncated to a sane length. Empty for kernel threads.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cmdline: String,
    /// Written only when it is not `S` (sleeping), the common case.
    #[serde(default = "default_state", skip_serializing_if = "is_sleeping")]
    pub state: char,
    pub threads: u32,
    /// Share of the whole machine (all logical CPUs) over the last interval, 0..=100.
    pub cpu_pct: f32,
    /// Private resident memory (RssAnon), bytes.
    pub mem_bytes: u64,
    /// NVIDIA SM utilisation attributed to this process, 0..=100. `None` when not on the GPU.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_pct: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_enc_pct: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_dec_pct: Option<f32>,
    /// VRAM used by this process, bytes. `None` when not on the GPU or not reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_bytes: Option<u64>,
    /// Field 22 of /proc/<pid>/stat; with `pid` it identifies a process instance.
    pub start_time_ticks: u64,
    /// Executable path when the kernel shows it (the user's own processes); otherwise the
    /// command line's first word when that is an absolute path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
    /// Storage reads and writes over the last interval, bytes per second. `None` when the
    /// kernel hides them (other users' processes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_read_bps: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_write_bps: Option<f32>,
    /// When the process started. 0 when unknown.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub started_unix_ms: u64,
    /// The owner's user name. Never empty in a full record, so empty means "static fields left out".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user: String,
    /// Set by the collector for processes whose termination breaks the desktop session or this
    /// monitor: the kind of damage (`session`, `system`, `xwayland`, `shell`, `audio`,
    /// `collector`). The collector owns the list; clients only show the matching warning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critical: Option<String>,
}

impl ProcInfo {
    /// False for a record whose static fields were left out (`slim` subscribers).
    pub fn has_static(&self) -> bool {
        !self.user.is_empty()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProcGroup {
    /// Stable while the app runs, e.g. `app:brave-browser`, `unit:sshd.service`, `kthreads`.
    pub id: String,
    pub class: ProcClass,
    pub name: String,
    /// Freedesktop icon name or absolute icon path, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Main window title for apps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub procs: Vec<ProcInfo>,
    /// Sums over `procs`.
    pub cpu_pct: f32,
    pub mem_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_pct: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_bytes: Option<u64>,
    /// Sums over the members that report them; `None` when none do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_read_bps: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_write_bps: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_enc_pct: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_dec_pct: Option<f32>,
}

// ---------------------------------------------------------------------------
// Snapshot and summary
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SourceStatus {
    /// Short name: `nvml`, `lact`, `helper`, `hyprland`, ...
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub proto: u32,
    pub seq: u64,
    pub unix_ms: u64,
    pub stats_since_unix_ms: u64,
    pub interval_ms: u32,
    /// How far back the collector keeps history for every reading.
    #[serde(default)]
    pub history_minutes: u32,
    pub hostname: String,
    /// Display order: CPU, GPU, Memory, drives, Motherboard and fans, others.
    pub sensors: Vec<SensorGroup>,
    pub sources: Vec<SourceStatus>,
    /// Present only while at least one subscriber wants processes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub procs: Option<Vec<ProcGroup>>,
    pub summary: Summary,
    /// Threshold crossings since the collector started (closed ones for a few hours), oldest
    /// first. Left out when there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crossings: Vec<Crossing>,
    /// The readings whose limits are not the defaults (left out when there are none), by id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub limit_overrides: BTreeMap<String, LimitOverride>,
    /// Why `thresholds.json` could not be used (it is kept as it is and the last good limits
    /// stay in force), with the line and column when known. Missing while the file is fine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits_error: Option<String>,
}

/// Headline values. Also written every sample to [`summary_path`] for any
/// bar widget or script.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Summary {
    pub unix_ms: u64,
    pub cpu_usage_pct: Option<f64>,
    pub cpu_temp_c: Option<f64>,
    pub cpu_power_w: Option<f64>,
    pub cpu_clock_avg_mhz: Option<f64>,
    pub gpu_usage_pct: Option<f64>,
    pub gpu_temp_c: Option<f64>,
    pub gpu_hotspot_c: Option<f64>,
    pub gpu_vram_temp_c: Option<f64>,
    pub gpu_power_w: Option<f64>,
    pub gpu_vram_used_bytes: Option<f64>,
    pub gpu_vram_total_bytes: Option<f64>,
    pub ram_used_pct: Option<f64>,
    pub ram_used_bytes: Option<f64>,
    pub ram_total_bytes: Option<f64>,
    pub swap_total_bytes: Option<f64>,
    pub ram_temp_max_c: Option<f64>,
    pub ssd_temp_max_c: Option<f64>,
    pub disk_read_bps: Option<f64>,
    pub disk_write_bps: Option<f64>,
}

// ---------------------------------------------------------------------------
// IPC
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Start receiving snapshots. The daemon sends the latest one right away. With `slim` the
    /// process records leave out their static fields once the client has had them (see
    /// [`ProcInfo`]); omitted, the client gets every field in every snapshot.
    Subscribe {
        procs: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        slim: bool,
        /// The client's window has no focus. Absent means focused, as for a client that never says.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        unfocused: bool,
    },
    /// Turn process collection on or off for this client, and say whether its window has focus
    /// (the collector samples processes every second while any subscribed client that wants
    /// them is focused, every two seconds otherwise; sensors always keep their interval).
    SetProcs {
        enabled: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        unfocused: bool,
    },
    /// Reset min/max/average for all readings.
    ResetStats,
    /// Ask for the recent history of some readings; answered with `DaemonMsg::History`.
    History { ids: Vec<String>, seconds: u32 },
    /// Set the limits of a reading, replacing its defaults as a whole (`None` is no limit at that
    /// level). Values are in the reading's own unit. The collector writes `thresholds.json`.
    SetThreshold {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        warn: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crit: Option<f64>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        below: bool,
    },
    /// Give a reading its default limits back.
    ResetThreshold { id: String },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonMsg {
    Snapshot(Box<Snapshot>),
    History { series: Vec<HistorySeries> },
    /// A warning notification was clicked: bring this reading into view.
    Focus { id: String },
    Error { message: String },
}

/// Write one message as a JSON line and flush.
pub fn write_msg<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    serde_json::to_writer(&mut *w, msg).map_err(io::Error::other)?;
    w.write_all(b"\n")?;
    w.flush()
}

/// Read one JSON line. `Ok(None)` on clean EOF. `buf` is reused between calls.
pub fn read_msg<R: BufRead, T: DeserializeOwned>(r: &mut R, buf: &mut String) -> io::Result<Option<T>> {
    buf.clear();
    if r.read_line(buf)? == 0 {
        return Ok(None);
    }
    serde_json::from_str(buf.trim_end())
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// `$XDG_RUNTIME_DIR/hwglance`, falling back to `/run/user/<uid>/hwglance`.
pub fn runtime_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            use std::os::unix::fs::MetadataExt;
            let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0);
            PathBuf::from(format!("/run/user/{uid}"))
        });
    base.join("hwglance")
}

pub fn socket_path() -> PathBuf {
    runtime_dir().join("daemon.sock")
}

pub fn summary_path() -> PathBuf {
    runtime_dir().join("summary.json")
}

/// `$XDG_CONFIG_HOME/hwglance`, falling back to `~/.config/hwglance`. `None` without a home.
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("hwglance"))
}

/// The warning and critical limits of every reading, edited by hand or from the window and written
/// by the collector only.
pub fn thresholds_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("thresholds.json"))
}

/// Written by the root helper (systemd `RuntimeDirectory=hwglance-helper`).
pub const HELPER_DIR: &str = "/run/hwglance-helper";

pub fn helper_power_path() -> PathBuf {
    PathBuf::from(HELPER_DIR).join("cpu_power.json")
}

pub fn helper_memory_path() -> PathBuf {
    PathBuf::from(HELPER_DIR).join("memory.json")
}

// ---------------------------------------------------------------------------
// Root helper file formats
// ---------------------------------------------------------------------------

/// `cpu_power.json`, rewritten about once a second from RAPL energy counters.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HelperPower {
    pub unix_ms: u64,
    pub package_w: Option<f64>,
    pub core_w: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HelperDimm {
    pub locator: String,
    pub bank: String,
    pub size_mb: u64,
    pub mem_type: String,
    pub rated_mts: Option<u32>,
    pub configured_mts: Option<u32>,
    pub configured_mv: Option<u32>,
    pub manufacturer: String,
    pub part: String,
}

/// `memory.json`, written once at helper start from the SMBIOS type 17 entries.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HelperMemory {
    pub unix_ms: u64,
    pub dimms: Vec<HelperDimm>,
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

/// Human formatting shared by the window and command-line dumps.
pub fn format_value(unit: Unit, v: f64) -> String {
    match unit {
        Unit::Percent => format!("{v:.1} %"),
        Unit::Celsius => format!("{v:.1} °C"),
        Unit::Mhz => format!("{v:.0} MHz"),
        Unit::Watt => format!("{v:.1} W"),
        Unit::Volt => format!("{v:.3} V"),
        Unit::Rpm => format!("{v:.0} RPM"),
        Unit::Mts => format!("{v:.0} MT/s"),
        Unit::Flag => (if v >= 0.5 { "Yes" } else { "No" }).to_string(),
        Unit::Bytes => {
            const MIB: f64 = 1024.0 * 1024.0;
            const GIB: f64 = MIB * 1024.0;
            if v >= GIB {
                format!("{:.1} GiB", v / GIB)
            } else {
                format!("{:.0} MiB", v / MIB)
            }
        }
        Unit::BytesPerSec => {
            if v >= 1e9 {
                format!("{:.2} GB/s", v / 1e9)
            } else if v >= 1e6 {
                format!("{:.1} MB/s", v / 1e6)
            } else {
                format!("{:.0} KB/s", v / 1e3)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        Snapshot {
            proto: PROTO_VERSION,
            seq: 7,
            unix_ms: 1,
            stats_since_unix_ms: 0,
            interval_ms: 1000,
            history_minutes: 30,
            hostname: "testhost".into(),
            sensors: vec![SensorGroup {
                id: "cpu".into(),
                title: "CPU".into(),
                subtitle: Some("Test CPU 8-Core".into()),
                sections: vec![Section {
                    id: "cpu/temp".into(),
                    title: "Temperatures".into(),
                    collapsed_by_default: false,
                    readings: vec![Reading {
                        id: "cpu/temp/tctl".into(),
                        label: "Tctl".into(),
                        unit: Unit::Celsius,
                        value: Some(54.6),
                        stats: Some(Stats::first(54.6)),
                        note: None,
                        desc: Some("AMD's control temperature.".into()),
                        events: None,
                        warn: Some(80.0),
                        crit: Some(89.0),
                    }],
                    summary: None,
                }],
            }],
            sources: vec![],
            procs: Some(vec![]),
            summary: Summary::default(),
            crossings: vec![],
            limit_overrides: BTreeMap::new(),
            limits_error: None,
        }
    }

    #[test]
    fn snapshot_round_trip() {
        let msg = DaemonMsg::Snapshot(Box::new(sample()));
        let mut out = Vec::new();
        write_msg(&mut out, &msg).unwrap();
        assert!(out.ends_with(b"\n"));
        let mut buf = String::new();
        let back: DaemonMsg = read_msg(&mut out.as_slice(), &mut buf).unwrap().unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn client_msg_shape() {
        let s = serde_json::to_string(&ClientMsg::Subscribe { procs: true, slim: false, unfocused: false }).unwrap();
        assert_eq!(s, r#"{"cmd":"subscribe","procs":true}"#);
        let r: ClientMsg = serde_json::from_str(r#"{"cmd":"reset_stats"}"#).unwrap();
        assert_eq!(r, ClientMsg::ResetStats);
    }

    #[test]
    fn subscribe_slim_is_optional() {
        let old: ClientMsg = serde_json::from_str(r#"{"cmd":"subscribe","procs":true}"#).unwrap();
        assert_eq!(old, ClientMsg::Subscribe { procs: true, slim: false, unfocused: false });
        let new = serde_json::to_string(&ClientMsg::Subscribe { procs: false, slim: true, unfocused: false }).unwrap();
        assert_eq!(new, r#"{"cmd":"subscribe","procs":false,"slim":true}"#);
    }

    #[test]
    fn version_3_additions_are_optional_both_ways() {
        // the focus flag is left out when focused and read as focused when missing
        let s = serde_json::to_string(&ClientMsg::SetProcs { enabled: true, unfocused: false }).unwrap();
        assert_eq!(s, r#"{"cmd":"set_procs","enabled":true}"#);
        let old: ClientMsg = serde_json::from_str(&s).unwrap();
        assert_eq!(old, ClientMsg::SetProcs { enabled: true, unfocused: false });
        let s = serde_json::to_string(&ClientMsg::SetProcs { enabled: true, unfocused: true }).unwrap();
        assert_eq!(s, r#"{"cmd":"set_procs","enabled":true,"unfocused":true}"#);
        let sub: ClientMsg = serde_json::from_str(r#"{"cmd":"subscribe","procs":true,"slim":true,"unfocused":true}"#).unwrap();
        assert_eq!(sub, ClientMsg::Subscribe { procs: true, slim: true, unfocused: true });
        // a version 2 collector reads a new client's message by ignoring the field it does not know:
        // modelled here by an enum without it
        #[derive(Deserialize, Debug, PartialEq)]
        #[serde(tag = "cmd", rename_all = "snake_case")]
        enum V2 {
            SetProcs { enabled: bool },
        }
        assert_eq!(serde_json::from_str::<V2>(&s).unwrap(), V2::SetProcs { enabled: true });

        // crossings are left out when empty, and a version 2 snapshot reads as having none
        let mut snap = sample();
        let plain = serde_json::to_string(&snap).unwrap();
        assert!(!plain.contains("crossings"), "{plain}");
        assert_eq!(serde_json::from_str::<Snapshot>(&plain).unwrap().crossings, vec![]);
        snap.crossings.push(Crossing {
            id: "cpu/temp/tctl".into(),
            level: CrossingLevel::Crit,
            start_ms: 10,
            peak: 91.5,
            peak_ms: 20,
            end_ms: None,
            below: false,
        });
        let with = serde_json::to_string(&snap).unwrap();
        assert!(with.contains(r#""crossings":[{"id":"cpu/temp/tctl","level":"crit","start_ms":10,"peak":91.5,"peak_ms":20}]"#), "{with}");
        assert_eq!(serde_json::from_str::<Snapshot>(&with).unwrap(), snap);
    }

    fn proc_sample() -> ProcInfo {
        ProcInfo {
            pid: 5,
            ppid: 1,
            uid: 1000,
            name: "brave".into(),
            cmdline: "/opt/brave/brave --type=gpu".into(),
            state: 'S',
            threads: 4,
            cpu_pct: 1.5,
            mem_bytes: 1024,
            gpu_pct: None,
            gpu_enc_pct: None,
            gpu_dec_pct: None,
            vram_bytes: None,
            start_time_ticks: 77,
            exe: Some("/opt/brave/brave".into()),
            disk_read_bps: None,
            disk_write_bps: None,
            started_unix_ms: 123,
            user: "alex".into(),
            critical: None,
        }
    }

    #[test]
    fn empty_process_fields_are_left_out_and_read_back() {
        let mut p = proc_sample();
        p.cmdline.clear();
        p.exe = None;
        p.user.clear();
        p.started_unix_ms = 0;
        let s = serde_json::to_string(&p).unwrap();
        for key in ["cmdline", "exe", "user", "started_unix_ms", "gpu_pct", "vram_bytes", "state", "critical"] {
            assert!(!s.contains(&format!("\"{key}\"")), "{key} in {s}");
        }
        let back: ProcInfo = serde_json::from_str(&s).unwrap();
        assert_eq!(back, p);
        assert!(!back.has_static() && proc_sample().has_static());
        // a record from the version 1 collector (nulls and all) still parses
        let v1 = r#"{"pid":1,"ppid":0,"uid":0,"name":"systemd","cmdline":"","state":"S","threads":1,"cpu_pct":0.0,"mem_bytes":1,"gpu_pct":null,"gpu_enc_pct":null,"gpu_dec_pct":null,"vram_bytes":null,"start_time_ticks":1}"#;
        let old: ProcInfo = serde_json::from_str(v1).unwrap();
        assert_eq!((old.state, old.gpu_pct, old.user.as_str()), ('S', None, ""));
        let mut busy = proc_sample();
        busy.state = 'R';
        busy.critical = Some("session".into());
        let back: ProcInfo = serde_json::from_str(&serde_json::to_string(&busy).unwrap()).unwrap();
        assert_eq!(back, busy);
    }

    #[test]
    fn snapshot_without_procs_key_reads_as_none() {
        let mut snap = sample();
        snap.procs = None;
        let s = serde_json::to_string(&snap).unwrap();
        assert!(!s.contains("\"procs\""), "{s}");
        let back: Snapshot = serde_json::from_str(&s).unwrap();
        assert_eq!(back.procs, None);
    }

    #[test]
    fn stats_average() {
        let mut s = Stats::first(10.0);
        s.push(20.0);
        s.push(30.0);
        assert_eq!((s.min, s.max, s.samples), (10.0, 30.0, 3));
        assert!((s.avg - 20.0).abs() < 1e-9);
    }

    #[test]
    fn history_round_trip() {
        let msg = DaemonMsg::History {
            series: vec![HistorySeries { id: "cpu/temp/tctl".into(), unit: Unit::Celsius, points: vec![(1, 54.5), (2, 55.0)] }],
        };
        let s = serde_json::to_string(&msg).unwrap();
        assert!(s.contains(r#""points":[[1,54.5],[2,55.0]]"#), "{s}");
        let back: DaemonMsg = serde_json::from_str(&s).unwrap();
        assert_eq!(back, msg);
        let req: ClientMsg = serde_json::from_str(r#"{"cmd":"history","ids":["a"],"seconds":300}"#).unwrap();
        assert_eq!(req, ClientMsg::History { ids: vec!["a".into()], seconds: 300 });
    }

    #[test]
    fn version_4_limits_are_optional_both_ways() {
        // no overrides and no error: nothing on the wire, and a version 3 snapshot reads as having none
        let mut snap = sample();
        let plain = serde_json::to_string(&snap).unwrap();
        assert!(!plain.contains("limit_overrides") && !plain.contains("limits_error"), "{plain}");
        let old = serde_json::from_str::<Snapshot>(&plain).unwrap();
        assert!(old.limit_overrides.is_empty() && old.limits_error.is_none());
        snap.limit_overrides.insert(
            "cpu/temp/tctl".into(),
            LimitOverride { warn: None, crit: Some(85.0), below: false, default_warn: Some(80.0), default_crit: Some(89.0) },
        );
        snap.limits_error = Some("expected value at line 3 column 5".into());
        let with = serde_json::to_string(&snap).unwrap();
        assert!(with.contains(r#""limit_overrides":{"cpu/temp/tctl":{"crit":85.0,"default_warn":80.0,"default_crit":89.0}}"#), "{with}");
        assert_eq!(serde_json::from_str::<Snapshot>(&with).unwrap(), snap);
    }

    #[test]
    fn threshold_requests_round_trip() {
        let set = ClientMsg::SetThreshold { id: "fan/pump".into(), warn: Some(1000.0), crit: None, below: true };
        let s = serde_json::to_string(&set).unwrap();
        assert_eq!(s, r#"{"cmd":"set_threshold","id":"fan/pump","warn":1000.0,"below":true}"#);
        assert_eq!(serde_json::from_str::<ClientMsg>(&s).unwrap(), set);
        // a request without the optional fields reads as no limits, "above"
        let bare: ClientMsg = serde_json::from_str(r#"{"cmd":"set_threshold","id":"x/y"}"#).unwrap();
        assert_eq!(bare, ClientMsg::SetThreshold { id: "x/y".into(), warn: None, crit: None, below: false });
        let reset = ClientMsg::ResetThreshold { id: "cpu/temp/tctl".into() };
        let s = serde_json::to_string(&reset).unwrap();
        assert_eq!(s, r#"{"cmd":"reset_threshold","id":"cpu/temp/tctl"}"#);
        assert_eq!(serde_json::from_str::<ClientMsg>(&s).unwrap(), reset);
    }

    #[test]
    fn limit_units_are_what_the_editor_shows() {
        assert_eq!(Unit::Bytes.limit_factor(), 1073741824.0);
        assert_eq!(Unit::Bytes.limit_unit_text(), "GiB");
        assert_eq!(Unit::BytesPerSec.limit_factor(), 1e6);
        assert_eq!(Unit::Celsius.limit_factor(), 1.0);
        assert_eq!(Unit::Celsius.limit_unit_text(), "°C");
    }

    #[test]
    fn formatting() {
        assert_eq!(format_value(Unit::Flag, 1.0), "Yes");
        assert_eq!(format_value(Unit::Flag, 0.0), "No");
        assert_eq!(format_value(Unit::Bytes, 13630.0 * 1024.0 * 1024.0), "13.3 GiB");
        assert_eq!(format_value(Unit::BytesPerSec, 2_300_000.0), "2.3 MB/s");
        assert_eq!(format_value(Unit::Celsius, 48.0), "48.0 °C");
    }
}
