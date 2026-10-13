//! Owns every sampler and turns one sampling pass into a [`Snapshot`].

use crate::alerts::{Engine, Notifier, Observation, OnOpen};
use crate::config::Config;
use crate::cpu::CpuSampler;
use crate::disks::DiskSampler;
use crate::gpu::{self, GpuSampler};
use crate::helper::HelperState;
use crate::history::History;
use crate::hwmon::{self, Chip, ChipVal};
use crate::layout::{self, Inputs};
use crate::memory;
use crate::nvml::NvmlSource;
use crate::procs::ProcCollector;
use crate::slow::{SlowData, SlowPoller};
use crate::statsundo;
use crate::thresholds::{Edit, Thresholds};
use crate::uiprefs::PrefsWatcher;
use crate::util::{hostname, now_ms, read_trim};
use hwglance_proto::{
    crossings, FlagEvents, PROTO_VERSION, Reading, SensorGroup, Snapshot, SourceStatus, Stats, Unit,
};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What one sampling pass does about processes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcsMode {
    /// Nobody wants them: collect none and let go of the files kept open.
    Off,
    /// Somebody wants them but not this time (every second pass while no window is focused):
    /// collect none and keep what is kept, so the next collection is as cheap as ever.
    Skip,
    Collect,
}

impl From<bool> for ProcsMode {
    fn from(wanted: bool) -> Self {
        if wanted { ProcsMode::Collect } else { ProcsMode::Off }
    }
}

/// While no window that wants processes has focus, they are sampled this often (sensors keep
/// the configured interval).
pub const UNFOCUSED_PROCS_EVERY: Duration = Duration::from_secs(2);

/// Decides, tick by tick, whether processes are collected: every tick while a window that wants
/// them is focused, a joining client is waiting or nothing has been collected yet, and every
/// [`UNFOCUSED_PROCS_EVERY`] otherwise.
#[derive(Default)]
pub struct ProcCadence {
    last: Option<Instant>,
}

impl ProcCadence {
    /// `urgent`: a client is owed a complete first list now.
    pub fn mode(&mut self, now: Instant, wanted: bool, focused: bool, urgent: bool, interval: Duration) -> ProcsMode {
        if !wanted {
            self.last = None;
            return ProcsMode::Off;
        }
        // Half an interval of slack, so a tick that comes a little early or late does not skip twice.
        let due = self.last.is_none_or(|t| now.saturating_duration_since(t) + interval / 2 >= UNFOCUSED_PROCS_EVERY);
        if focused || urgent || due {
            self.last = Some(now);
            ProcsMode::Collect
        } else {
            ProcsMode::Skip
        }
    }
}

pub struct Core {
    cfg: Config,
    chips: Arc<Vec<Chip>>,
    cpu: CpuSampler,
    disks: DiskSampler,
    nvml: NvmlSource,
    gpus: GpuSampler,
    /// An NVIDIA GPU is on the bus: only then are NVML and LACT asked for anything.
    nvidia: bool,
    helper: HelperState,
    procs: ProcCollector,
    slow: Arc<Mutex<SlowData>>,
    slow_poller: Option<SlowPoller>,
    stats: HashMap<String, Stats>,
    stats_since: u64,
    seq: u64,
    last_sample: Option<Instant>,
    hostname: String,
    board_name: String,
    /// Last few raw iGPU PPT readings.
    ppt_history: VecDeque<f64>,
    prefs: PrefsWatcher,
    /// The limits of every reading, from `thresholds.json`.
    limits: Thresholds,
    history: Arc<Mutex<History>>,
    /// Slow poll counter last seen, to tell fresh slow values from repeats.
    slow_stamp: u64,
    /// Flag readings: previous state and onset events.
    flags: HashMap<String, (bool, FlagEvents)>,
    engine: Engine,
    notifier: Option<Notifier>,
    sweeps: u64,
    crossings: crossings::Tracker,
    /// What the latest stats reset cleared, for Undo.
    undo: statsundo::Undo,
    /// All-CPU jiffies since the last process collection (the divisor of the processes' shares).
    proc_jiffies: u64,
}

/// Readings fed by the slow poller (or the Super I/O chip behind it): only new when it ran.
pub fn is_slow_id(id: &str) -> bool {
    id.starts_with("board/")
        || id.starts_with("memory/temp/")
        || id.starts_with("gpu0/vramchip")
        || id.starts_with("cpu/temp/board_")
        || matches!(
            id,
            "gpu0/temp/hotspot" | "gpu0/temp/vram" | "gpu0/power/voltage" | "gpu0/fan/rpm"
        )
        || (id.starts_with("disk/") && id.contains("/temp/"))
}

/// Track on/off switches of a flag reading and attach its event record.
fn update_flag(flags: &mut HashMap<String, (bool, FlagEvents)>, now_ms: u64, r: &mut Reading) {
    if r.unit != Unit::Flag {
        return;
    }
    let entry = flags.entry(r.id.clone()).or_insert((
        false,
        FlagEvents {
            count: 0,
            last_unix_ms: 0,
            ended_unix_ms: None,
        },
    ));
    if let Some(v) = r.value {
        let on = v >= 0.5;
        if on && !entry.0 {
            entry.1.count += 1;
            entry.1.last_unix_ms = now_ms;
            entry.1.ended_unix_ms = None;
        } else if !on && entry.0 {
            entry.1.ended_unix_ms = Some(now_ms);
        }
        entry.0 = on;
    }
    r.events = Some(entry.1);
}

/// Fold a reading's value into its running statistics and attach them.
fn update_stats(stats: &mut HashMap<String, Stats>, r: &mut Reading) {
    if let Some(v) = r.value {
        match stats.get_mut(&r.id) {
            Some(s) => s.push(v),
            None => {
                stats.insert(r.id.clone(), Stats::first(v));
            }
        }
    }
    r.stats = stats.get(&r.id).copied();
}

/// The PPT sensor is instantaneous and drops to a few watts now and then.
const PPT_WINDOW: usize = 5;
const PPT_MIN_W: f64 = 5.0;

/// Median of the readings at or above [`PPT_MIN_W`].
fn ppt_median(history: &VecDeque<f64>) -> Option<f64> {
    let mut v: Vec<f64> = history
        .iter()
        .copied()
        .filter(|w| *w >= PPT_MIN_W)
        .collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let mid = v.len() / 2;
    Some(if v.len() % 2 == 1 {
        v[mid]
    } else {
        (v[mid - 1] + v[mid]) / 2.0
    })
}

fn board_name() -> String {
    let vendor = read_trim("/sys/class/dmi/id/board_vendor").unwrap_or_default();
    let name = read_trim("/sys/class/dmi/id/board_name").unwrap_or_default();
    let vendor = match vendor.as_str() {
        "ASUSTeK COMPUTER INC." => "ASUS",
        "Micro-Star International Co., Ltd." | "Micro-Star International" => "MSI",
        "Gigabyte Technology Co., Ltd." => "Gigabyte",
        v => v,
    };
    format!("{vendor} {name}").trim().to_string()
}

impl Core {
    pub fn new(cfg: Config, history: Arc<Mutex<History>>) -> Self {
        let chips = Arc::new(hwmon::discover());
        let found = gpu::discover();
        Core {
            slow_poller: Some(SlowPoller::new(chips.clone(), found.nvidia)),
            chips,
            cpu: CpuSampler::new(),
            disks: DiskSampler::new(),
            nvml: NvmlSource::new(found.nvidia),
            nvidia: found.nvidia,
            gpus: GpuSampler::new(found.gpus),
            helper: HelperState::new(),
            procs: ProcCollector::new(),
            slow: Arc::new(Mutex::new(SlowData::default())),
            stats: HashMap::new(),
            stats_since: now_ms(),
            seq: 0,
            last_sample: None,
            hostname: hostname(),
            board_name: board_name(),
            ppt_history: VecDeque::new(),
            prefs: PrefsWatcher::new(),
            limits: Thresholds::new(hwglance_proto::thresholds_path(), false),
            history,
            slow_stamp: 0,
            flags: HashMap::new(),
            engine: Engine::new(),
            notifier: None,
            sweeps: 0,
            crossings: crossings::Tracker::new(),
            undo: statsundo::Undo::default(),
            proc_jiffies: 0,
            cfg,
        }
    }

    /// Processes (pid, start time) first seen within the last few samples.
    pub fn fresh_statics(&self) -> &std::collections::HashSet<(i32, u64)> {
        self.procs.fresh()
    }

    pub fn interval(&self) -> Duration {
        Duration::from_millis(self.cfg.interval_ms)
    }

    /// Start the slow poller on its own thread.
    pub fn spawn_slow(&mut self) {
        if let Some(p) = self.slow_poller.take() {
            p.spawn(
                self.slow.clone(),
                Duration::from_millis(self.cfg.slow_interval_ms),
            );
        }
    }

    /// One synchronous slow poll (for `--once`).
    pub fn poll_slow_now(&mut self) {
        if let Some(p) = self.slow_poller.as_mut() {
            let data = p.poll();
            if let Ok(mut g) = self.slow.lock() {
                *g = data;
            }
        }
    }

    /// Make this process the writer of `thresholds.json`: it is created, completed and updated.
    /// Without it (the one-shot dump) the file is only read.
    pub fn own_thresholds_file(&mut self) {
        self.limits = Thresholds::new(hwglance_proto::thresholds_path(), true);
    }

    /// Changes of limits asked for by windows; applied by the next sample.
    pub fn queue_threshold_edits(&mut self, edits: Vec<Edit>) {
        self.limits.queue(edits);
    }

    /// Show desktop notifications for alerts; `on_open` runs when one is clicked.
    pub fn enable_alerts(&mut self, on_open: OnOpen) {
        self.notifier = Some(Notifier::new(on_open));
    }

    /// Clear min/max/average, the throttle history and the finished crossings, keeping what went
    /// for [`statsundo::WINDOW`] after `at` (when the collector received the request).
    pub fn reset_stats(&mut self, at: Instant) {
        self.undo.keep(at, &self.stats, &self.flags, &self.crossings, self.stats_since);
        self.stats.clear();
        for (_, ev) in self.flags.values_mut() {
            *ev = FlagEvents {
                count: 0,
                last_unix_ms: 0,
                ended_unix_ms: None,
            };
        }
        self.crossings.clear_closed();
        self.stats_since = now_ms();
    }

    /// Take back the latest reset if `at` is within the window of it. Returns whether it did.
    pub fn undo_reset_stats(&mut self, at: Instant) -> bool {
        self.undo.restore(at, &mut self.stats, &mut self.flags, &mut self.crossings, &mut self.stats_since)
    }

    /// Take process baselines when collection is about to start, so the first
    /// published process list already has CPU shares.
    pub fn prime_procs(&mut self) {
        if !self.procs.has_baseline(self.interval() * 2) {
            self.procs.prime();
        }
    }

    /// Add this sample's values to the history. Slow readings are added only when fresh.
    fn record_history(
        &mut self,
        sensors: &[SensorGroup],
        now_ms: u64,
        slow_fresh: bool,
        minutes: u32,
    ) {
        let Ok(mut h) = self.history.lock() else {
            return;
        };
        h.set_minutes(minutes);
        for s in sensors.iter().flat_map(|g| &g.sections) {
            // Copies share their original's id and are already in `readings`.
            let derived = s.summary.iter().filter(|r| r.id.ends_with("/summary"));
            for r in s.readings.iter().chain(derived) {
                if let Some(v) = r.value
                    && (slow_fresh || !is_slow_id(&r.id))
                {
                    h.record(now_ms, &r.id, r.unit, v);
                }
            }
        }
        self.sweeps += 1;
        if self.sweeps.is_multiple_of(120) {
            h.sweep(now_ms);
        }
    }

    fn check_alerts(&mut self, sensors: &[SensorGroup], now_ms: u64, slow_fresh: bool) {
        let prefs = &self.prefs.prefs;
        for g in sensors {
            for r in g.sections.iter().flat_map(|s| &s.readings) {
                let Some(value) = r.value else { continue };
                let slow = is_slow_id(&r.id);
                if slow && !slow_fresh {
                    continue;
                }
                let obs = Observation {
                    id: &r.id,
                    group: &g.title,
                    label: &r.label,
                    unit: r.unit,
                    value,
                    warn: r.warn,
                    crit: r.crit,
                    below: self.limits.below(&r.id),
                    slow,
                };
                if let Some(alert) = self.engine.observe(now_ms, &prefs.alerts, &obs)
                    && let Some(n) = &self.notifier
                {
                    n.notify(alert);
                }
            }
        }
    }

    /// Record the readings past their thresholds (the same effective thresholds the alerts use).
    fn track_crossings(&mut self, sensors: &[SensorGroup], now_ms: u64, slow_fresh: bool) {
        for r in sensors.iter().flat_map(|g| &g.sections).flat_map(|s| &s.readings) {
            let Some(value) = r.value else { continue };
            if is_slow_id(&r.id) && !slow_fresh {
                continue;
            }
            self.crossings.observe(now_ms, &r.id, value, r.warn, r.crit, self.limits.below(&r.id));
        }
        self.crossings.prune(now_ms);
    }

    pub fn sample(&mut self, procs: impl Into<ProcsMode>) -> Snapshot {
        let mode = procs.into();
        let now = Instant::now();
        let dt = self.last_sample.map(|t| now.duration_since(t));
        self.last_sample = Some(now);
        self.seq += 1;

        self.prefs.poll();
        let cpu = self.cpu.sample();
        let mut chips: Vec<ChipVal> = self
            .chips
            .iter()
            .filter(|c| !c.slow)
            .map(hwmon::read_chip)
            .collect();
        let slow = self.slow.lock().map(|g| g.clone()).unwrap_or_default();
        chips.extend(slow.chips.iter().cloned());
        let mem = memory::read();
        let disks = self.disks.sample(dt);
        let gpu = self.nvml.sample();
        self.helper.poll();

        self.proc_jiffies += cpu.jiffies;
        let procs = (mode == ProcsMode::Collect).then(|| {
            let procs = &self.procs;
            self.nvml.prune(|p| procs.is_alive(p));
            let gpu_procs = self.nvml.processes();
            let jiffies = std::mem::take(&mut self.proc_jiffies);
            self.procs.collect(jiffies, &gpu_procs)
        });
        if mode == ProcsMode::Off {
            self.procs.release(); // nobody wants processes: do not keep their files open
            self.proc_jiffies = 0;
        }

        let gpus = self.gpus.sample();
        if let Some(w) = gpus
            .iter()
            .find(|g| g.card == "igpu")
            .and_then(|g| g.power_w)
        {
            if self.ppt_history.len() >= PPT_WINDOW {
                self.ppt_history.pop_front();
            }
            self.ppt_history.push_back(w);
        }
        let ppt_estimate = ppt_median(&self.ppt_history);

        let inputs = Inputs {
            cfg: &self.cfg,
            cpu: &cpu,
            chips: &chips,
            mem: mem.as_ref(),
            disks: &disks,
            gpu: gpu.as_ref(),
            gpus: &gpus,
            lact: slow.lact.as_ref(),
            helper_power: self.helper.power(),
            helper_mem: self.helper.memory(),
            board_name: &self.board_name,
            ppt_estimate,
        };
        let mut sensors = layout::build(&inputs);
        let unix_ms = now_ms();
        let mut summary = layout::summary(&sensors, &inputs, unix_ms);
        summary.unix_ms = unix_ms;

        for g in &mut sensors {
            for s in &mut g.sections {
                s.readings.retain(|r| !self.cfg.hide.contains(&r.id));
                // A summary that copies a hidden reading goes with it.
                let readings = &s.readings;
                s.summary.take_if(|sum| {
                    self.cfg.hide.contains(&sum.id)
                        || (!sum.id.ends_with("/summary")
                            && !readings.iter().any(|r| r.id == sum.id))
                });
            }
            g.sections.retain(|s| !s.readings.is_empty());
        }
        sensors.retain(|g| !g.sections.is_empty());
        // From here on every reading carries the limits in force (the user's, else its defaults).
        self.limits.sync(&mut sensors, &self.prefs.prefs.legacy_thresholds);
        let slow_fresh = slow.stamp != self.slow_stamp;
        self.slow_stamp = slow.stamp;
        for s in sensors.iter_mut().flat_map(|g| &mut g.sections) {
            for r in &mut s.readings {
                update_flag(&mut self.flags, unix_ms, r);
                update_stats(&mut self.stats, r);
            }
            if let Some(sum) = &mut s.summary {
                // A copy shares its original's statistics (counting them once); a derived value has its own.
                match s.readings.iter().find(|r| r.id == sum.id) {
                    Some(orig) => {
                        sum.stats = orig.stats;
                        sum.events = orig.events;
                    }
                    None => update_stats(&mut self.stats, sum),
                }
            }
        }

        let minutes = self.prefs.prefs.history_minutes;
        self.record_history(&sensors, unix_ms, slow_fresh, minutes);
        self.track_crossings(&sensors, unix_ms, slow_fresh);
        if self.notifier.is_some() {
            self.check_alerts(&sensors, unix_ms, slow_fresh);
        }

        let (nvml_ok, nvml_detail) = self.nvml.status();
        let (helper_ok, helper_detail) = self.helper.status();
        let (hypr_ok, hypr_detail) = self.procs.hypr.status();
        let mut sources = Vec::new();
        // NVML and LACT feed the NVIDIA card: without NVIDIA hardware they are not failing, just unused.
        if self.nvidia {
            sources.push(SourceStatus {
                name: "nvml".into(),
                ok: nvml_ok,
                detail: nvml_detail,
            });
            sources.push(SourceStatus {
                name: "lact".into(),
                ok: slow.lact_ok,
                detail: slow.lact_detail,
            });
        }
        sources.push(SourceStatus {
            name: "helper".into(),
            ok: helper_ok,
            detail: helper_detail,
        });
        sources.push(SourceStatus {
            name: "hyprland".into(),
            ok: hypr_ok,
            detail: hypr_detail,
        });

        Snapshot {
            proto: PROTO_VERSION,
            seq: self.seq,
            unix_ms,
            stats_since_unix_ms: self.stats_since,
            interval_ms: self.cfg.interval_ms as u32,
            history_minutes: minutes,
            hostname: self.hostname.clone(),
            sensors,
            sources,
            procs,
            summary,
            crossings: self.crossings.list().to_vec(),
            limit_overrides: self.limits.overrides().clone(),
            limits_error: self.limits.error().map(str::to_string),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_remembers_when_it_began_and_when_it_ended() {
        let mut flags = HashMap::new();
        let mut step = |t: u64, v: f64| {
            let mut r = Reading {
                id: "cpu/throttle/thermal".into(),
                label: "Thermal throttling".into(),
                unit: Unit::Flag,
                value: Some(v),
                stats: None,
                note: None,
                desc: None,
                events: None,
                warn: None,
                crit: None,
            };
            update_flag(&mut flags, t, &mut r);
            r.events.unwrap()
        };
        assert_eq!(step(1000, 0.0).ended_unix_ms, None, "never on");
        let on = step(2000, 1.0);
        assert_eq!((on.count, on.last_unix_ms, on.ended_unix_ms), (1, 2000, None));
        assert_eq!(step(3000, 1.0).ended_unix_ms, None, "still on");
        let off = step(4000, 0.0);
        assert_eq!((off.count, off.last_unix_ms, off.ended_unix_ms), (1, 2000, Some(4000)));
        assert_eq!(step(5000, 0.0).ended_unix_ms, Some(4000), "the end stays where it was");
        let again = step(6000, 1.0);
        assert_eq!((again.count, again.last_unix_ms, again.ended_unix_ms), (2, 6000, None));
    }

    #[test]
    fn processes_are_sampled_every_tick_when_focused_and_every_second_tick_when_not() {
        let t0 = Instant::now();
        let sec = Duration::from_secs(1);
        let run = |focused: bool| -> Vec<ProcsMode> {
            let mut c = ProcCadence::default();
            (0..6).map(|n| c.mode(t0 + sec * n, true, focused, false, sec)).collect()
        };
        assert!(run(true).iter().all(|m| *m == ProcsMode::Collect));
        use ProcsMode::{Collect as C, Skip as S};
        assert_eq!(run(false), [C, S, C, S, C, S], "the first pass always collects, then every 2 s");
        // jitter of a few tens of ms does not make it skip twice or collect every tick
        let mut c = ProcCadence::default();
        let ms = Duration::from_millis;
        let ticks = [0, 1030, 1990, 3010, 3980, 5020];
        let modes: Vec<_> = ticks.iter().map(|t| c.mode(t0 + ms(*t), true, false, false, sec)).collect();
        assert_eq!(modes, [C, S, C, S, C, S]);
        // a joining client (urgent) and a window that gains focus get their list at once
        let mut c = ProcCadence::default();
        assert_eq!(c.mode(t0, true, false, false, sec), C);
        assert_eq!(c.mode(t0 + sec, true, false, true, sec), C);
        assert_eq!(c.mode(t0 + sec * 2, true, false, false, sec), S);
        assert_eq!(c.mode(t0 + sec * 2, true, true, false, sec), C);
        // nobody wants them: off, and the next wish starts with a collection
        assert_eq!(c.mode(t0 + sec * 3, false, true, false, sec), ProcsMode::Off);
        assert_eq!(c.mode(t0 + sec * 4, true, false, false, sec), C);
        // a configured 2 s interval collects every tick
        let mut c = ProcCadence::default();
        let two = Duration::from_secs(2);
        assert!((0..4).all(|n| c.mode(t0 + two * n, true, false, false, two) == C));
    }

    #[test]
    fn ppt_median_ignores_dropouts() {
        let h: VecDeque<f64> = [53.6, 57.5, 2.8, 63.2, 49.4].into();
        assert_eq!(ppt_median(&h), Some((53.6 + 57.5) / 2.0));
        let h: VecDeque<f64> = [1.1, 2.0].into();
        assert_eq!(ppt_median(&h), None);
        let h: VecDeque<f64> = [60.0, 1.0, 50.0, 70.0].into();
        assert_eq!(ppt_median(&h), Some(60.0));
    }
}
