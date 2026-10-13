//! `hwglanced`: sensor and process collector daemon for hwglance.

mod alerts;
mod config;
mod core;
mod cpu;
mod describe;
mod desktop;
mod disks;
mod gpu;
mod helper;
mod history;
mod hwmon;
mod hypr;
mod ipc;
mod lact;
mod layout;
mod memory;
mod nvml;
mod pci;
mod procs;
mod slow;
mod steam;
mod textexport;
mod text;
mod thresholds;
mod uiprefs;
mod util;

use config::Config;
use core::{Core, ProcCadence};
use hwglance_proto::{ProcGroup, Snapshot, runtime_dir, socket_path, summary_path, write_msg};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Slim clients get the static fields of every process this often, so one that missed a snapshot
/// catches up.
const FULL_STATIC_EVERY: Duration = Duration::from_secs(30);

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

fn install_signals() {
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

const USAGE: &str = "usage: hwglanced [--once [--procs] [--json]] [--version]\n\
  (no arguments)   run the daemon, serving snapshots on the socket in $XDG_RUNTIME_DIR/hwglance\n\
  --once           take two samples 1 s apart, print the second and exit\n\
  --procs          with --once: include the process list\n\
  --json           with --once: print the snapshot as JSON\n\
  --version        print the version\n\
  --test-notify [ID]   show one test notification (normal urgency, expires after 8 s) and report the answer\n\
  --bench-history MINUTES   measure the memory and CPU cost of recording every reading for MINUTES";

fn main() {
    let mut once = false;
    let mut procs = false;
    let mut json = false;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--test-notify") => return test_notify(args.get(1).map(String::as_str)),
        Some("--bench-history") => {
            let minutes = args.get(1).and_then(|m| m.parse().ok()).unwrap_or(30);
            return bench_history(Config::load(), minutes);
        }
        _ => {}
    }
    for a in args {
        match a.as_str() {
            "--once" => once = true,
            "--procs" => procs = true,
            "--json" => json = true,
            "--version" | "-V" => {
                println!("hwglanced {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return;
            }
            other => {
                eprintln!("hwglanced: unknown argument {other}\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    if (procs || json) && !once {
        eprintln!("hwglanced: --procs and --json only apply with --once\n{USAGE}");
        std::process::exit(2);
    }
    let cfg = Config::load();
    let result = if once {
        run_once(cfg, procs, json)
    } else {
        run_daemon(cfg)
    };
    if let Err(e) = result {
        eprintln!("hwglanced: {e}");
        std::process::exit(1);
    }
}

fn new_history() -> Arc<Mutex<history::History>> {
    Arc::new(Mutex::new(history::History::new(util::now_ms(), 30)))
}

/// One real notification with a made-up reading id, to check that notify-send and its action work.
fn test_notify(id: Option<&str>) {
    let id = id.unwrap_or("test/fake/reading");
    let alert = alerts::Alert {
        id: id.to_string(),
        severity: alerts::Severity::Warning,
        title: "HW Glance test notification".into(),
        body: format!(
            "Fake reading {id}; nothing is wrong. Click Open to see what the button does."
        ),
    };
    match alerts::show(&alert, &["-t", "8000"]) {
        Ok(action) if action == "open" => {
            println!(
                "answer: open (a real alert would now send Focus {{ id: {id} }} and run omarchy-launch-or-focus)"
            )
        }
        Ok(action) => println!(
            "answer: {:?} (dismissed or expired; no action chosen)",
            action
        ),
        Err(e) => println!("notify-send failed: {e}"),
    }
}

fn rss_kib() -> u64 {
    util::read_trim("/proc/self/status")
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("VmRSS:"))
                .and_then(|v| v.split_whitespace().next()?.parse().ok())
        })
        .unwrap_or(0)
}

/// Fill a history with every reading of a real snapshot for `minutes` of virtual time and report
/// what that costs. Slow readings are recorded every 2nd second, like the daemon does.
fn bench_history(cfg: Config, minutes: u32) {
    let mut core = Core::new(cfg, new_history());
    core.poll_slow_now();
    core.sample(false);
    let snap = core.sample(false);
    let mut items: Vec<(String, hwglance_proto::Unit, bool)> = Vec::new();
    for s in snap.sensors.iter().flat_map(|g| &g.sections) {
        let derived = s.summary.iter().filter(|r| r.id.ends_with("/summary"));
        for r in s.readings.iter().chain(derived) {
            items.push((r.id.clone(), r.unit, core::is_slow_id(&r.id)));
        }
    }
    drop(core);
    let before = rss_kib();
    let mut h = history::History::new(0, minutes);
    let start = Instant::now();
    let rounds = u64::from(minutes) * 60;
    for sec in 0..rounds {
        let now = 1_000_000 + sec * 1000;
        for (n, (id, unit, slow)) in items.iter().enumerate() {
            if *slow && sec % 2 == 1 {
                continue;
            }
            h.record(now, id, *unit, 40.0 + ((sec + n as u64) % 50) as f64);
        }
    }
    let fill = start.elapsed();
    let after = rss_kib();
    let ids: Vec<String> = items.iter().map(|i| i.0.clone()).collect();
    let now = 1_000_000 + rounds * 1000;
    let q = Instant::now();
    let series = h.query(now, &ids[..ids.len().min(20)], minutes * 60);
    let qt = q.elapsed();
    println!(
        "history {minutes} min: {} readings recorded, {} series, {} points, memory +{:.1} MiB (RSS {} KiB -> {} KiB), recording {:.1} us per sample round, 20-series query {:.2} ms ({} points each at most)",
        items.len(),
        h.series_count(),
        h.total_points(),
        after.saturating_sub(before) as f64 / 1024.0,
        before,
        after,
        fill.as_secs_f64() * 1e6 / rounds as f64,
        qt.as_secs_f64() * 1e3,
        series.iter().map(|s| s.points.len()).max().unwrap_or(0),
    );
}

fn run_once(cfg: Config, procs: bool, json: bool) -> io::Result<()> {
    let mut core = Core::new(cfg, new_history());
    core.poll_slow_now();
    core.sample(procs); // baseline for rates and CPU shares
    if procs {
        core.prime_procs();
    }
    std::thread::sleep(Duration::from_secs(1));
    let snap = core.sample(procs);
    if json {
        let mut out = io::stdout().lock();
        write_msg(&mut out, &snap)
    } else {
        text::print(&snap);
        Ok(())
    }
}

/// `DaemonMsg::Snapshot` for a snapshot that is only borrowed (same JSON as the owned one).
#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SnapshotMsg<'a> {
    Snapshot(&'a Snapshot),
}

fn encode_one(snap: &Snapshot) -> io::Result<Arc<Vec<u8>>> {
    let mut out = Vec::with_capacity(if snap.procs.is_some() { 256 * 1024 } else { 64 * 1024 });
    write_msg(&mut out, &SnapshotMsg::Snapshot(snap))?;
    Ok(Arc::new(out))
}

/// Command lines in the full (non-slim) encoding are cut at this many characters: the widget
/// reads the program and shows a line of it, and it gets them in every snapshot.
const FULL_CMDLINE_CHARS: usize = 200;

/// Cut command lines short, marking the cut with an ellipsis.
fn cap_cmdlines(groups: &mut [ProcGroup], max: usize) {
    for p in groups.iter_mut().flat_map(|g| &mut g.procs) {
        if let Some((i, _)) = p.cmdline.char_indices().nth(max) {
            p.cmdline.truncate(i);
            p.cmdline.push('\u{2026}');
        }
    }
}

/// Leave out the static fields of every process that is not in `fresh`.
fn strip_static(groups: &mut [ProcGroup], fresh: &HashSet<(i32, u64)>) {
    for p in groups.iter_mut().flat_map(|g| &mut g.procs) {
        if !fresh.contains(&(p.pid, p.start_time_ticks)) {
            p.cmdline = String::new();
            p.exe = None;
            p.user = String::new();
            p.started_unix_ms = 0;
        }
    }
}

/// Serialize a snapshot for the wire: without the process list, and with it in the forms the
/// subscribed clients need (`demand`). `fresh` names the processes whose static fields a slim
/// client may not have yet; `all_static` sends them for every process.
fn encode(
    mut snap: Snapshot,
    demand: ipc::Demand,
    fresh: &HashSet<(i32, u64)>,
    all_static: bool,
) -> io::Result<ipc::Latest> {
    let procs = snap.procs.take();
    let lite = encode_one(&snap)?;
    let (mut full, mut slim) = (None, None);
    if let Some(mut groups) = procs {
        if demand.full {
            // Cut a copy when the slim form is wanted too, else the groups themselves.
            let mut cut = if demand.slim {
                groups.clone()
            } else {
                std::mem::take(&mut groups)
            };
            cap_cmdlines(&mut cut, FULL_CMDLINE_CHARS);
            snap.procs = Some(cut);
            full = Some(encode_one(&snap)?);
        }
        if demand.slim {
            if !all_static {
                strip_static(&mut groups, fresh);
            }
            snap.procs = Some(groups);
            slim = Some(encode_one(&snap)?);
        }
    }
    Ok(ipc::Latest {
        lite,
        full,
        slim,
        slim_complete: all_static,
    })
}

/// Write the summary atomically so readers never see a partial file.
fn write_summary(s: &hwglance_proto::Summary) -> io::Result<()> {
    let path = summary_path();
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec(s).map_err(io::Error::other)?)?;
    fs::rename(&tmp, &path)
}

fn run_daemon(cfg: Config) -> io::Result<()> {
    let text_readouts = cfg.text_readouts;
    let dir = runtime_dir();
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;

    let (kick_tx, kick_rx) = mpsc::channel();
    let history = new_history();
    let shared = ipc::Shared::new(kick_tx, history.clone());
    ipc::serve(&socket_path(), shared.clone())?;
    install_signals();

    let mut core = Core::new(cfg, history);
    core.own_thresholds_file();
    {
        let shared = shared.clone();
        core.enable_alerts(Arc::new(move |id: &str| {
            shared.broadcast_focus(id);
            alerts::launch_or_focus(id);
        }));
    }
    core.spawn_slow();
    let interval = core.interval();
    let mut next = Instant::now();
    let mut last_full = Instant::now();
    let mut cadence = ProcCadence::default();
    while !STOP.load(Ordering::Relaxed) {
        // Sleep until the next sample, waking early only to take process baselines.
        loop {
            let now = Instant::now();
            if now >= next || STOP.load(Ordering::Relaxed) {
                break;
            }
            match kick_rx.recv_timeout((next - now).min(Duration::from_millis(500))) {
                Ok(()) => {
                    if shared.want_procs() {
                        core.prime_procs();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
        if STOP.load(Ordering::Relaxed) {
            break;
        }
        if shared.take_reset() {
            core.reset_stats();
        }
        core.queue_threshold_edits(shared.take_edits());
        let mode = cadence.mode(Instant::now(), shared.want_procs(), shared.procs_focused(), shared.need_full_pending(), interval);
        let snap = core.sample(mode);
        if let Err(e) = write_summary(&snap.summary) {
            eprintln!("hwglanced: cannot write summary: {e}");
        }
        if let Err(e) = textexport::update(text_readouts, &snap.summary) {
            eprintln!("hwglanced: cannot write text readouts: {e}");
        }
        // A slim client that joined is owed a complete snapshot, and the flag stays until one has processes.
        let all_static = snap.procs.is_some()
            && (shared.take_need_full() || last_full.elapsed() >= FULL_STATIC_EVERY);
        if all_static {
            last_full = Instant::now();
        }
        match encode(snap, shared.demand(), core.fresh_statics(), all_static) {
            Ok(latest) => shared.publish(latest),
            Err(e) => eprintln!("hwglanced: cannot encode snapshot: {e}"),
        }
        next += interval;
        let now = Instant::now();
        if next < now {
            next = now + interval; // fell behind: do not burst to catch up
        }
    }
    let _ = fs::remove_file(socket_path());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwglance_proto::{DaemonMsg, ProcClass, ProcInfo, Summary};

    fn proc(pid: i32, cmdline: &str) -> ProcInfo {
        ProcInfo {
            pid,
            ppid: 1,
            uid: 1000,
            name: "p".into(),
            cmdline: cmdline.into(),
            state: 'S',
            threads: 1,
            cpu_pct: 0.5,
            mem_bytes: 10,
            gpu_pct: None,
            gpu_enc_pct: None,
            gpu_dec_pct: None,
            vram_bytes: None,
            start_time_ticks: pid as u64 * 10,
            exe: Some("/usr/bin/p".into()),
            disk_read_bps: None,
            disk_write_bps: None,
            started_unix_ms: 99,
            user: "alex".into(),
            critical: None,
        }
    }

    fn snapshot() -> Snapshot {
        let long = "x".repeat(400);
        Snapshot {
            proto: hwglance_proto::PROTO_VERSION,
            seq: 1,
            unix_ms: 1,
            stats_since_unix_ms: 0,
            interval_ms: 1000,
            history_minutes: 30,
            hostname: "h".into(),
            sensors: vec![],
            sources: vec![],
            procs: Some(vec![ProcGroup {
                id: "app:p".into(),
                class: ProcClass::App,
                name: "P".into(),
                icon: None,
                title: None,
                procs: vec![proc(1, "old one"), proc(2, &long)],
                cpu_pct: 1.0,
                mem_bytes: 20,
                gpu_pct: None,
                vram_bytes: None,
                disk_read_bps: None,
                disk_write_bps: None,
                gpu_enc_pct: None,
                gpu_dec_pct: None,
            }]),
            summary: Summary::default(),
            crossings: vec![],
            limit_overrides: Default::default(),
            limits_error: None,
        }
    }

    fn procs_of(bytes: &[u8]) -> Option<Vec<ProcInfo>> {
        let DaemonMsg::Snapshot(s) = serde_json::from_slice(bytes).unwrap() else {
            panic!("not a snapshot")
        };
        s.procs.map(|g| g.into_iter().flat_map(|g| g.procs).collect())
    }

    #[test]
    fn slim_leaves_static_fields_out_of_known_processes_only() {
        let fresh: HashSet<(i32, u64)> = [(2, 20)].into();
        let demand = ipc::Demand { full: true, slim: true };
        let latest = encode(snapshot(), demand, &fresh, false).unwrap();
        assert_eq!(procs_of(&latest.lite), None);
        assert!(!latest.slim_complete);

        let slim = procs_of(latest.slim.as_ref().unwrap()).unwrap();
        assert!(!slim[0].has_static() && slim[0].cmdline.is_empty() && slim[0].exe.is_none());
        assert!(slim[1].has_static() && slim[1].cmdline.len() == 400, "a new process keeps all of it");

        let full = procs_of(latest.full.as_ref().unwrap()).unwrap();
        assert!(full.iter().all(ProcInfo::has_static), "the full form never leaves them out");
        assert_eq!(full[0].cmdline, "old one");
        assert_eq!(full[1].cmdline.chars().count(), FULL_CMDLINE_CHARS + 1);
        assert!(full[1].cmdline.ends_with('\u{2026}'));

        let complete = encode(snapshot(), demand, &fresh, true).unwrap();
        assert!(complete.slim_complete);
        assert!(procs_of(complete.slim.as_ref().unwrap()).unwrap().iter().all(ProcInfo::has_static));
    }

    #[test]
    fn only_the_wanted_forms_are_encoded() {
        let none = HashSet::new();
        let l = encode(snapshot(), ipc::Demand::default(), &none, false).unwrap();
        assert!(l.full.is_none() && l.slim.is_none());
        let l = encode(snapshot(), ipc::Demand { full: false, slim: true }, &none, false).unwrap();
        assert!(l.full.is_none() && l.slim.is_some());
        let slim = procs_of(l.slim.as_ref().unwrap()).unwrap();
        assert!(slim.iter().all(|p| !p.has_static()));
        let mut no_procs = snapshot();
        no_procs.procs = None;
        let l = encode(no_procs, ipc::Demand { full: true, slim: true }, &none, true).unwrap();
        assert!(l.full.is_none() && l.slim.is_none());
    }
}
