//! Processes and Task Manager style grouping.
//!
//! Every sample reads /proc/<pid>/stat and statm for all processes; names, command lines,
//! owners and cgroups are read once per process and cached.

use crate::desktop::Desktop;
use crate::hypr::Hypr;
use crate::nvml::GpuProc;
use crate::steam::SteamNames;
use crate::util::{basename, truncate_chars, unescape_systemd};
use hwglance_proto::{ProcClass, ProcGroup, ProcInfo};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Processes that end an upward walk: init, the session, launchers and terminal emulators.
const BOUNDARY: &[&str] = &[
    "kthreadd",
    "systemd",
    "hyprland",
    "start-hyprland",
    "uwsm",
    "uwsm-app",
    "app2unit",
    "systemd-run",
    "foot",
    "alacritty",
    "kitty",
    "ghostty",
    "wezterm-gui",
    "konsole",
    "gnome-terminal-server",
    "xterm",
];
/// Shells and interpreters: launcher scripts, not apps. Members of a tree when grouping background work.
const SHELLS: &[&str] = &[
    "bash", "sh", "dash", "zsh", "fish", "python", "python3", "perl", "ruby", "lua",
];
/// Session launchers that appear as the first part of `app-<launcher>-<id>-<random>.scope`.
const LAUNCHERS: &[&str] = &[
    "hyprland", "flatpak", "uwsm", "snap", "sway", "niri", "gnome", "kde", "plasma", "river",
    "labwc", "wayfire", "dbus", "systemd", "xdg",
];

/// Processes whose termination breaks the desktop session or this monitor, matched on the process
/// name or the base name of its first command line word (lowercase), with the kind of damage.
/// The one list: windows and bar widgets only show the warning for the kind the collector sends.
const CRITICAL: &[(&str, &str)] = &[
    ("hyprland", "session"),
    ("start-hyprland", "session"),
    ("systemd", "system"),
    ("xwayland", "xwayland"),
    ("quickshell", "shell"),
    ("omarchy-shell", "shell"),
    ("pipewire", "audio"),
    ("pipewire-pulse", "audio"),
    ("wireplumber", "audio"),
    ("hwglanced", "collector"),
];

fn critical_kind(pid: i32, name: &str, cmdline: &str) -> Option<&'static str> {
    if pid == 1 {
        return Some("system");
    }
    let name = name.to_lowercase();
    let first = basename(cmdline.split(' ').next().unwrap_or("")).to_lowercase();
    CRITICAL
        .iter()
        .find(|(n, _)| *n == name || *n == first)
        .map(|(_, kind)| *kind)
}

/// The static fields of a record are sent again for this many samples after a process appears,
/// so that a snapshot a client missed does not leave it without them.
const FRESH_SAMPLES: u64 = 3;
/// Files kept open for reading again; beyond this they are opened by path each time.
const MAX_KEPT_FILES: usize = 6000;

/// Allow enough open files to keep three per process.
fn raise_fd_limit() {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: lim is a valid rlimit for getrlimit/setrlimit to read and write.
    unsafe {
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) == 0 {
            let want = lim.rlim_max.min(16384);
            if lim.rlim_cur < want {
                lim.rlim_cur = want;
                libc::setrlimit(libc::RLIMIT_NOFILE, &lim);
            }
        }
    }
}

/// Read a small procfs file through a handle that is kept open. Opening a procfs file costs more
/// than reading it, so a process' files stay open from sample to sample and are read with
/// `pread`. A missing handle, or one whose process died (the read fails or returns nothing), is
/// replaced by opening `path`. Returns the number of bytes in `buf`, `None` when the file cannot be
/// read or does not fit.
fn read_kept(slot: &mut Option<File>, path: impl FnOnce() -> String, buf: &mut [u8]) -> Option<usize> {
    if let Some(f) = slot.as_ref() {
        match f.read_at(buf, 0) {
            Ok(n) if n > 0 && n < buf.len() => return Some(n),
            Ok(n) if n == buf.len() => return None,
            _ => *slot = None,
        }
    }
    let f = File::open(path()).ok()?;
    let n = f.read_at(buf, 0).ok()?;
    if n == 0 || n >= buf.len() {
        return None;
    }
    *slot = Some(f);
    Some(n)
}

/// Open /proc files of one process (see [`read_kept`]).
#[derive(Default)]
struct Files {
    stat: Option<File>,
    statm: Option<File>,
    io: Option<File>,
}

impl Files {
    fn count(&self) -> usize {
        [&self.stat, &self.statm, &self.io]
            .iter()
            .filter(|f| f.is_some())
            .count()
    }
}

/// What is remembered about one process between samples; replaced when the pid is reused.
struct Entry {
    start: u64,
    st: Rc<Static>,
    /// Value of `ProcCollector::collects` when it was first seen.
    born: u64,
    files: Files,
}

struct Static {
    /// `/proc/<pid>/comm`, or argv[0]'s basename when comm hit the 15 character limit.
    name: String,
    cmdline: String,
    uid: u32,
    exe: String,
    /// Full path of the executable, when known.
    exe_path: Option<String>,
    cgroup: String,
    kthread: bool,
    /// Kind of damage ending it would do, when it is on the [`CRITICAL`] list.
    critical: Option<&'static str>,
}

struct P {
    pid: i32,
    ppid: i32,
    state: char,
    threads: u32,
    ticks: u64,
    start: u64,
    rss_anon: u64,
    /// Cumulative (read_bytes, write_bytes) of the process, for the user's own processes.
    io: Option<(u64, u64)>,
    st: Rc<Static>,
    born: u64,
}

/// Longest window title sent to the window (an ellipsis shortens it there anyway).
const MAX_TITLE_CHARS: usize = 256;

struct Grp {
    class: ProcClass,
    id: String,
    name: Option<String>,
    icon: Option<String>,
    title: Option<String>,
    members: Vec<i32>,
}

pub struct ProcCollector {
    cache: HashMap<i32, Entry>,
    /// Number of passes over /proc so far.
    collects: u64,
    /// Processes (pid, start time) first seen within [`FRESH_SAMPLES`] passes of the last collection.
    fresh: HashSet<(i32, u64)>,
    prev_ticks: HashMap<i32, (u64, u64)>,
    /// (start time, read_bytes, write_bytes) at the previous collection.
    prev_io: HashMap<i32, (u64, u64, u64)>,
    /// Boot time (unix seconds) and clock ticks per second, for process start times.
    boot_secs: u64,
    ticks_per_sec: u64,
    users: Users,
    last_collect: Option<Instant>,
    page_size: u64,
    my_uid: u32,
    desktop: Desktop,
    steam: SteamNames,
    pub hypr: Hypr,
    alive: HashSet<i32>,
}

/// Seconds since the epoch at boot (`btime` in /proc/stat).
fn read_boot_secs() -> u64 {
    fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("btime "))
                .and_then(|v| v.trim().parse().ok())
        })
        .unwrap_or(0)
}

/// uid -> user name from /etc/passwd, reloaded when the file changes.
#[derive(Default)]
struct Users {
    names: HashMap<u32, String>,
    mtime: Option<std::time::SystemTime>,
}

impl Users {
    fn refresh(&mut self) {
        let mtime = fs::metadata("/etc/passwd").and_then(|m| m.modified()).ok();
        if mtime.is_some() && mtime == self.mtime {
            return;
        }
        self.mtime = mtime;
        self.names = parse_passwd(&fs::read_to_string("/etc/passwd").unwrap_or_default());
    }

    fn name(&self, uid: u32) -> String {
        self.names
            .get(&uid)
            .cloned()
            .unwrap_or_else(|| uid.to_string())
    }
}

fn parse_passwd(text: &str) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let mut f = line.split(':');
        if let (Some(name), Some(_), Some(uid)) = (f.next(), f.next(), f.next())
            && let Ok(uid) = uid.parse::<u32>()
        {
            map.entry(uid).or_insert_with(|| name.to_string());
        }
    }
    map
}

/// `read_bytes` and `write_bytes` (actual storage I/O) from /proc/<pid>/io.
fn parse_io(text: &str) -> Option<(u64, u64)> {
    let field = |key: &str| -> Option<u64> {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.trim().parse().ok())
    };
    Some((field("read_bytes:")?, field("write_bytes:")?))
}

fn io_baseline(procs: &HashMap<i32, P>) -> HashMap<i32, (u64, u64, u64)> {
    procs
        .values()
        .filter_map(|p| p.io.map(|(r, w)| (p.pid, (p.start, r, w))))
        .collect()
}

/// `v` rounded to a multiple of `1 / scale`, as the f32 that goes on the wire (short in JSON).
fn round_to(v: f64, scale: f64) -> f32 {
    ((v * scale).round() / scale) as f32
}

fn is_boundary(p: &P) -> bool {
    p.pid <= 2 || BOUNDARY.contains(&p.st.name.to_lowercase().as_str())
}

fn is_shell_name(name: &str) -> bool {
    let n = name.to_lowercase();
    SHELLS.contains(&n.as_str()) || n.starts_with("python3.")
}

fn is_shell(p: &P) -> bool {
    is_shell_name(&p.st.name)
}

/// Name for a group that has no desktop entry or unit name: the name of its root process.
/// When that root is a shell or interpreter (a launcher script), the script's name is more useful:
/// its first argument that is not an option, else the biggest member that is not a shell.
fn group_name_from_members(infos: &[ProcInfo]) -> String {
    let set: HashSet<i32> = infos.iter().map(|i| i.pid).collect();
    let Some(root) = infos
        .iter()
        .filter(|i| !set.contains(&i.ppid))
        .min_by_key(|i| i.pid)
        .or(infos.first())
    else {
        return String::new();
    };
    if !is_shell_name(&root.name) {
        return root.name.clone();
    }
    if let Some(script) = script_name(&root.cmdline) {
        return script;
    }
    infos
        .iter()
        .filter(|i| !is_shell_name(&i.name))
        .max_by_key(|i| i.mem_bytes)
        .map(|i| i.name.clone())
        .unwrap_or_else(|| root.name.clone())
}

/// The script an interpreter was started with: `python3 /x/y.sh arg` -> `y.sh`.
/// `bash -c "..."` has no script.
fn script_name(cmdline: &str) -> Option<String> {
    for arg in cmdline.split_whitespace().skip(1) {
        if arg == "-c" {
            return None;
        }
        if !arg.starts_with('-') {
            return Some(basename(arg).to_string()).filter(|s| !s.is_empty());
        }
    }
    None
}

/// Parse `/proc/<pid>/stat` after the closing parenthesis of the command name.
fn parse_stat(text: &str) -> Option<(&str, char, i32, u32, u64, u64)> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let comm = text.get(open + 1..close)?;
    let f: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    // f[0] is field 3 (state); field n is f[n - 3].
    let state = f.first()?.chars().next()?;
    let ppid = f.get(1)?.parse().ok()?;
    let utime: u64 = f.get(11)?.parse().ok()?;
    let stime: u64 = f.get(12)?.parse().ok()?;
    let threads = f.get(17)?.parse().ok()?;
    let start = f.get(19)?.parse().ok()?;
    Some((comm, state, ppid, threads, start, utime + stime))
}

fn load_static(pid: i32, comm: String, ppid: i32) -> Static {
    let kthread = pid == 2 || ppid == 2;
    let uid = fs::metadata(format!("/proc/{pid}"))
        .map(|m| m.uid())
        .unwrap_or(0);
    if kthread {
        let critical = critical_kind(pid, &comm, "");
        return Static {
            name: comm,
            cmdline: String::new(),
            uid,
            exe: String::new(),
            exe_path: None,
            cgroup: String::new(),
            kthread,
            critical,
        };
    }
    let raw = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let argv0 = raw
        .split(|b| *b == 0)
        .next()
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .unwrap_or_default();
    let cmdline = String::from_utf8_lossy(&raw)
        .trim_end_matches('\0')
        .split('\0')
        .collect::<Vec<_>>()
        .join(" ");
    let name = if comm.len() >= 15 && !argv0.is_empty() {
        basename(&argv0).to_string()
    } else {
        comm
    };
    let link = fs::read_link(format!("/proc/{pid}/exe")).ok();
    let exe = link
        .as_ref()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    // The link is hidden for other users' processes: fall back to an absolute argv[0].
    let exe_path = link
        .map(|p| p.to_string_lossy().into_owned())
        .map(|p| {
            p.strip_suffix(" (deleted)")
                .map(str::to_string)
                .unwrap_or(p)
        })
        .or_else(|| Some(argv0.clone()).filter(|a| a.starts_with('/')));
    let cgroup = fs::read_to_string(format!("/proc/{pid}/cgroup"))
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("0::").map(str::to_string))
        })
        .unwrap_or_default();
    let critical = critical_kind(pid, &name, &cmdline);
    Static {
        name,
        cmdline: truncate_chars(&cmdline, 512),
        uid,
        exe,
        exe_path,
        cgroup,
        kthread,
        critical,
    }
}

/// `app-Hyprland-gtk\x2dlaunch-aeb31189.scope` -> `gtk-launch`;
/// `app-flatpak-com.core447.StreamController-65.scope` -> `com.core447.StreamController`.
fn app_id_from_unit(leaf: &str) -> String {
    let base = leaf
        .strip_suffix(".scope")
        .or_else(|| leaf.strip_suffix(".service"))
        .unwrap_or(leaf);
    let Some(base) = base.strip_prefix("app-") else {
        // A plain service: the unit name is the name.
        return unescape_systemd(base.split('@').next().unwrap_or(base));
    };
    let base = base.split('@').next().unwrap_or(base);
    let mut segs: Vec<&str> = base.split('-').collect();
    let is_random = |s: &str| s.len() >= 4 && s.bytes().all(|b| b.is_ascii_hexdigit());
    if segs.len() > 1 && segs.last().is_some_and(|s| is_random(s)) {
        segs.pop();
    }
    if segs.len() > 1 && LAUNCHERS.contains(&segs[0].to_lowercase().as_str()) {
        segs.remove(0);
    }
    unescape_systemd(&segs.join("-"))
}

/// "org.chromium.Chromium" -> "Chromium", "steam_app_1" -> "Steam app 1".
fn pretty_class(class: &str) -> String {
    let last = class.rsplit('.').next().unwrap_or(class);
    let words: Vec<String> = last
        .split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .enumerate()
        .map(|(i, w)| {
            let mut c = w.chars();
            match c.next() {
                Some(f) if i == 0 => f.to_uppercase().collect::<String>() + c.as_str(),
                _ => w.to_string(),
            }
        })
        .collect();
    if words.is_empty() {
        class.to_string()
    } else {
        words.join(" ")
    }
}

impl ProcCollector {
    pub fn new() -> Self {
        // SAFETY: sysconf and getuid have no preconditions.
        let (page, uid) = unsafe { (libc::sysconf(libc::_SC_PAGESIZE), libc::getuid()) };
        raise_fd_limit();
        ProcCollector {
            cache: HashMap::new(),
            collects: 0,
            fresh: HashSet::new(),
            prev_ticks: HashMap::new(),
            prev_io: HashMap::new(),
            boot_secs: read_boot_secs(),
            // SAFETY: sysconf has no preconditions.
            ticks_per_sec: u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) })
                .ok()
                .filter(|t| *t > 0)
                .unwrap_or(100),
            users: Users::default(),
            last_collect: None,
            page_size: if page > 0 { page as u64 } else { 4096 },
            my_uid: uid,
            desktop: Desktop::default(),
            steam: SteamNames::default(),
            hypr: Hypr::new(),
            alive: HashSet::new(),
        }
    }

    /// True when the last collection is recent enough for CPU deltas to be meaningful.
    pub fn has_baseline(&self, within: Duration) -> bool {
        self.last_collect.is_some_and(|t| t.elapsed() <= within)
    }

    pub fn is_alive(&self, pid: u32) -> bool {
        self.alive.contains(&(pid as i32))
    }

    fn read_all(&mut self) -> HashMap<i32, P> {
        self.collects += 1;
        let mut out = HashMap::new();
        let mut buf = [0u8; 2048];
        let mut buf2 = [0u8; 2048];
        let mut old = std::mem::take(&mut self.cache);
        let mut next = HashMap::with_capacity(old.len());
        let mut kept = 0usize;
        for e in fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else {
                continue;
            };
            let mut prev = old.remove(&pid);
            let mut stat_slot = prev.as_mut().and_then(|p| p.files.stat.take());
            let Some((comm, state, ppid, threads, start, ticks)) =
                read_kept(&mut stat_slot, || format!("/proc/{pid}/stat"), &mut buf)
                    .and_then(|n| std::str::from_utf8(&buf[..n]).ok())
                    .and_then(parse_stat)
            else {
                continue; // vanished
            };
            let mut ent = match prev {
                Some(p) if p.start == start => p,
                _ => Entry {
                    start,
                    st: Rc::new(load_static(pid, comm.to_string(), ppid)),
                    born: self.collects,
                    files: Files::default(),
                },
            };
            ent.files.stat = stat_slot;
            let st = ent.st.clone();
            let rss_anon = if st.kthread {
                0
            } else {
                read_kept(&mut ent.files.statm, || format!("/proc/{pid}/statm"), &mut buf2)
                    .and_then(|n| std::str::from_utf8(&buf2[..n]).ok())
                    .and_then(|t| {
                        let mut it = t.split_whitespace().skip(1);
                        let resident: u64 = it.next()?.parse().ok()?;
                        let shared: u64 = it.next()?.parse().ok()?;
                        Some(resident.saturating_sub(shared) * self.page_size)
                    })
                    .unwrap_or(0)
            };
            // Other users' io files are unreadable: do not spend the syscall.
            let io = (!st.kthread && st.uid == self.my_uid)
                .then(|| {
                    read_kept(&mut ent.files.io, || format!("/proc/{pid}/io"), &mut buf2)
                        .and_then(|n| std::str::from_utf8(&buf2[..n]).ok())
                        .and_then(parse_io)
                })
                .flatten();
            out.insert(
                pid,
                P {
                    pid,
                    ppid,
                    state,
                    threads,
                    ticks,
                    start,
                    rss_anon,
                    io,
                    st,
                    born: ent.born,
                },
            );
            let n = ent.files.count();
            if kept + n > MAX_KEPT_FILES {
                ent.files = Files::default();
            } else {
                kept += n;
            }
            next.insert(pid, ent);
        }
        self.cache = next;
        self.alive = out.keys().copied().collect();
        out
    }

    /// Forget what is kept between collections, closing the open files. Called while nobody wants
    /// processes; the next collection starts from scratch (and sends every static field again).
    pub fn release(&mut self) {
        if !self.cache.is_empty() {
            self.cache = HashMap::new();
            self.fresh.clear();
        }
    }

    /// Processes first seen within the last few collections (their static fields are worth sending).
    pub fn fresh(&self) -> &HashSet<(i32, u64)> {
        &self.fresh
    }

    /// Read everything once so the next collection has CPU baselines.
    pub fn prime(&mut self) {
        let procs = self.read_all();
        self.prev_ticks = procs
            .values()
            .map(|p| (p.pid, (p.start, p.ticks)))
            .collect();
        self.prev_io = io_baseline(&procs);
        self.last_collect = Some(Instant::now());
    }

    /// `jiffies` is the all-CPU jiffy delta since the previous sample (the share divisor).
    pub fn collect(&mut self, jiffies: u64, gpu: &HashMap<u32, GpuProc>) -> Vec<ProcGroup> {
        self.hypr.begin(); // answered while /proc is read
        self.desktop.refresh_if_stale();
        let procs = self.read_all();
        let dt = self
            .last_collect
            .map(|t| t.elapsed().as_secs_f64())
            .filter(|d| *d > 0.0);
        self.users.refresh();
        let clients = self.hypr.clients().unwrap_or_default();

        let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
        for p in procs.values() {
            children.entry(p.ppid).or_default().push(p.pid);
        }
        for v in children.values_mut() {
            v.sort_unstable();
        }

        // Window owners: pid -> lowercase class (first window wins), plus display data per class.
        let mut windows: BTreeMap<i32, String> = BTreeMap::new();
        let mut class_title: HashMap<String, (String, String)> = HashMap::new(); // lc -> (class, title)
        for c in &clients {
            let class = if c.class.is_empty() {
                &c.initial_class
            } else {
                &c.class
            };
            if class.is_empty() || !procs.contains_key(&c.pid) {
                continue;
            }
            let lc = class.to_lowercase();
            windows.entry(c.pid).or_insert_with(|| lc.clone());
            class_title
                .entry(lc)
                .or_insert_with(|| (class.clone(), c.title.clone()));
        }

        let app_roots = self.find_app_roots(&procs, &windows);

        // App members: the root and its descendants, cut at other apps.
        let mut member_of: HashMap<i32, i32> = HashMap::new();
        for (&root, class) in &app_roots {
            let mut stack = vec![root];
            while let Some(p) = stack.pop() {
                if member_of.contains_key(&p) {
                    continue;
                }
                member_of.insert(p, root);
                for &c in children.get(&p).into_iter().flatten() {
                    if app_roots.contains_key(&c) || windows.get(&c).is_some_and(|w| w != class) {
                        continue;
                    }
                    stack.push(c);
                }
            }
        }

        let mut groups: HashMap<(ProcClass, String), Grp> = HashMap::new();
        for (&root, class) in &app_roots {
            let g = groups
                .entry((ProcClass::App, format!("app:{class}")))
                .or_insert_with(|| {
                    let (orig, title) = class_title
                        .get(class)
                        .cloned()
                        .unwrap_or_else(|| (class.clone(), String::new()));
                    let rp = &procs[&root];
                    let mut exes: Vec<String> = [
                        rp.st.name.as_str(),
                        rp.st.exe.as_str(),
                        basename(rp.st.cmdline.split(' ').next().unwrap_or("")),
                    ]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_lowercase())
                    .collect();
                    exes.dedup();
                    let steam_id = class
                        .strip_prefix("steam_app_")
                        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()));
                    let (name, icon) = if let Some(app_id) = steam_id {
                        // Steam game: its own desktop entry, else the appmanifest, else the window title.
                        if let Some(e) = self.desktop.for_steam(app_id) {
                            (e.name.clone(), e.icon.clone())
                        } else if let Some(n) = self.steam.name(app_id) {
                            (n, None)
                        } else if !title.is_empty() {
                            (title.clone(), None)
                        } else {
                            (pretty_class(&orig), None)
                        }
                    } else {
                        let entry = self.desktop.for_class(class, &exes);
                        (
                            entry
                                .map(|e| e.name.clone())
                                .unwrap_or_else(|| pretty_class(&orig)),
                            entry.and_then(|e| e.icon.clone()),
                        )
                    };
                    Grp {
                        class: ProcClass::App,
                        id: format!("app:{class}"),
                        name: Some(name),
                        icon,
                        // a page title can run to kilobytes; the window shows a line of it
                        title: Some(truncate_chars(&title, MAX_TITLE_CHARS)).filter(|t| !t.is_empty()),
                        members: Vec::new(),
                    }
                });
            for (&p, &r) in &member_of {
                if r == root {
                    g.members.push(p);
                }
            }
        }

        // Everything else: background services, scopes and process trees; other users; kernel threads.
        let mut rest: Vec<&P> = procs
            .values()
            .filter(|p| !member_of.contains_key(&p.pid))
            .collect();
        rest.sort_by_key(|p| p.pid);
        for p in rest {
            let (class, id, name, icon) = self.classify_other(p, &procs);
            groups
                .entry((class, id.clone()))
                .or_insert_with(|| Grp {
                    class,
                    id,
                    name,
                    icon,
                    title: None,
                    members: Vec::new(),
                })
                .members
                .push(p.pid);
        }

        // Per-process records and group totals.
        let prev = std::mem::take(&mut self.prev_ticks);
        let prev_io = std::mem::take(&mut self.prev_io);
        let mut result = Vec::new();
        let used_ids: HashSet<String> = groups
            .iter()
            .filter(|((c, _), _)| *c != ProcClass::System)
            .map(|((_, id), _)| id.clone())
            .collect();
        for ((_, _), g) in groups {
            let mut infos: Vec<ProcInfo> = g
                .members
                .iter()
                .map(|pid| {
                    let p = &procs[pid];
                    let cpu = match prev.get(pid) {
                        Some((s, t)) if *s == p.start && jiffies > 0 => {
                            p.ticks.saturating_sub(*t) as f64 * 100.0 / jiffies as f64
                        }
                        _ => 0.0,
                    };
                    let gp = gpu.get(&(*pid as u32));
                    // Storage rate over the interval; none for a new process or an unreadable one.
                    let (disk_r, disk_w) = match (p.io, prev_io.get(pid), dt) {
                        (Some((r, w)), Some(&(s, pr, pw)), Some(dt)) if s == p.start => (
                            Some(round_to(r.saturating_sub(pr) as f64 / dt, 1.0)),
                            Some(round_to(w.saturating_sub(pw) as f64 / dt, 1.0)),
                        ),
                        _ => (None, None),
                    };
                    ProcInfo {
                        pid: p.pid,
                        ppid: p.ppid,
                        uid: p.st.uid,
                        name: p.st.name.clone(),
                        cmdline: p.st.cmdline.clone(),
                        state: p.state,
                        threads: p.threads,
                        cpu_pct: round_to(cpu, 100.0),
                        mem_bytes: p.rss_anon,
                        gpu_pct: gp.map(|g| g.sm),
                        gpu_enc_pct: gp.map(|g| g.enc),
                        gpu_dec_pct: gp.map(|g| g.dec),
                        vram_bytes: gp.and_then(|g| g.vram),
                        start_time_ticks: p.start,
                        exe: p.st.exe_path.clone(),
                        disk_read_bps: disk_r,
                        disk_write_bps: disk_w,
                        started_unix_ms: self.boot_secs * 1000
                            + p.start * 1000 / self.ticks_per_sec,
                        user: self.users.name(p.st.uid),
                        critical: p.st.critical.map(str::to_string),
                    }
                })
                .collect();
            infos.sort_by(|a, b| {
                b.cpu_pct
                    .total_cmp(&a.cpu_pct)
                    .then(b.mem_bytes.cmp(&a.mem_bytes))
                    .then(a.pid.cmp(&b.pid))
            });
            let name = g.name.unwrap_or_else(|| group_name_from_members(&infos));
            let sum_opt = |f: fn(&ProcInfo) -> Option<f32>| -> Option<f32> {
                infos
                    .iter()
                    .filter_map(f)
                    .fold(None, |a, v| Some(a.unwrap_or(0.0) + v))
            };
            let gpu_pct = sum_opt(|i| i.gpu_pct);
            let disk_read = sum_opt(|i| i.disk_read_bps);
            let disk_write = sum_opt(|i| i.disk_write_bps);
            let enc = sum_opt(|i| i.gpu_enc_pct);
            let dec = sum_opt(|i| i.gpu_dec_pct);
            let vram = infos
                .iter()
                .filter_map(|i| i.vram_bytes)
                .fold(None, |a: Option<u64>, v| Some(a.unwrap_or(0) + v));
            // A system group that shares an id with a user group gets its own namespace.
            let id = if g.class == ProcClass::System && used_ids.contains(&g.id) {
                format!("sys:{}", g.id)
            } else {
                g.id
            };
            result.push(ProcGroup {
                id,
                class: g.class,
                name,
                icon: g.icon,
                title: g.title,
                cpu_pct: round_to(infos.iter().map(|i| i.cpu_pct as f64).sum(), 100.0),
                mem_bytes: infos.iter().map(|i| i.mem_bytes).sum(),
                gpu_pct,
                vram_bytes: vram,
                disk_read_bps: disk_read,
                disk_write_bps: disk_write,
                gpu_enc_pct: enc,
                gpu_dec_pct: dec,
                procs: infos,
            });
        }
        self.prev_ticks = procs
            .values()
            .map(|p| (p.pid, (p.start, p.ticks)))
            .collect();
        self.prev_io = io_baseline(&procs);
        self.last_collect = Some(Instant::now());
        self.fresh = procs
            .values()
            .filter(|p| self.collects - p.born < FRESH_SAMPLES)
            .map(|p| (p.pid, p.start))
            .collect();
        result.sort_by(|a, b| {
            a.class
                .cmp(&b.class)
                .then(b.cpu_pct.total_cmp(&a.cpu_pct))
                .then(b.mem_bytes.cmp(&a.mem_bytes))
                .then_with(|| a.id.cmp(&b.id))
        });
        result
    }

    /// App roots: pid -> lowercase class. Each window owner climbs through wrapper processes.
    fn find_app_roots(
        &self,
        procs: &HashMap<i32, P>,
        windows: &BTreeMap<i32, String>,
    ) -> BTreeMap<i32, String> {
        let climb = |start: i32| -> i32 {
            let uid = procs[&start].st.uid;
            let mut cur = start;
            loop {
                let pp = procs[&cur].ppid;
                let Some(par) = procs.get(&pp) else { break };
                if par.st.uid != uid
                    || is_boundary(par)
                    || is_shell(par)
                    || windows.contains_key(&pp)
                {
                    break;
                }
                cur = pp;
            }
            cur
        };
        // root -> [(class, window owner)]
        let mut claims: BTreeMap<i32, Vec<(String, i32)>> = BTreeMap::new();
        for (&pid, class) in windows {
            claims
                .entry(climb(pid))
                .or_default()
                .push((class.clone(), pid));
        }
        let depth = |owner: i32, root: i32| -> usize {
            let (mut d, mut cur) = (0, owner);
            while cur != root && d < 64 {
                cur = procs[&cur].ppid;
                d += 1;
            }
            d
        };
        let mut roots = BTreeMap::new();
        for (root, list) in claims {
            let classes: HashSet<&String> = list.iter().map(|(c, _)| c).collect();
            if classes.len() == 1 {
                roots.insert(root, list[0].0.clone());
                continue;
            }
            // Several apps share one wrapper (a game under Steam). The class named like the root
            // program owns it, else the class whose window is nearest; the others start below it.
            let rp = &procs[&root];
            let names = [rp.st.name.to_lowercase(), rp.st.exe.to_lowercase()];
            let primary = list
                .iter()
                .map(|(c, o)| (!names.contains(c), depth(*o, root), c.clone()))
                .min()
                .map(|(_, _, c)| c)
                .unwrap_or_default();
            roots.insert(root, primary.clone());
            for (class, owner) in &list {
                if *class == primary {
                    continue;
                }
                let mut child = *owner;
                while procs[&child].ppid != root && child != root {
                    child = procs[&child].ppid;
                }
                roots.entry(child).or_insert_with(|| class.clone());
            }
        }
        roots
    }

    /// Class, group id, preferred name and icon for a process that is not part of an app.
    fn classify_other(
        &self,
        p: &P,
        procs: &HashMap<i32, P>,
    ) -> (ProcClass, String, Option<String>, Option<String>) {
        if p.st.kthread {
            return (
                ProcClass::System,
                "kthreads".into(),
                Some("Kernel threads".into()),
                None,
            );
        }
        let class = if p.st.uid == self.my_uid {
            ProcClass::Background
        } else {
            ProcClass::System
        };
        let path = &p.st.cgroup;
        let leaf = path.rsplit('/').next().unwrap_or("");
        let in_session_slice = path.split('/').any(|s| s == "session.slice");
        if leaf.ends_with(".service") && !leaf.starts_with("wayland-wm@") && !in_session_slice {
            let id = app_id_from_unit(leaf);
            let e = self.desktop.for_class(&id.to_lowercase(), &[]);
            let name = e.map(|e| e.name.clone()).unwrap_or(id);
            return (
                class,
                format!("unit:{leaf}"),
                Some(name),
                e.and_then(|e| e.icon.clone()),
            );
        }
        if leaf.starts_with("app-") && leaf.ends_with(".scope") {
            let id = app_id_from_unit(leaf);
            let e = self.desktop.for_class(&id.to_lowercase(), &[]);
            return (
                class,
                format!("scope:{leaf}"),
                e.map(|e| e.name.clone()),
                e.and_then(|e| e.icon.clone()),
            );
        }
        // Compositor service, login session scopes, init: group by process tree.
        let mut cur = p;
        while let Some(par) = procs.get(&cur.ppid) {
            if par.st.uid != p.st.uid || is_boundary(par) {
                break;
            }
            cur = par;
        }
        (class, format!("tree:{}:{}", cur.pid, cur.start), None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_parsing() {
        let line = "1234 (my (odd) app) S 99 1234 1234 0 -1 4194560 100 0 0 0 7 3 0 0 20 0 5 0 4242 1000 50 18446744073709551615";
        let (comm, state, ppid, threads, start, ticks) = parse_stat(line).unwrap();
        assert_eq!(comm, "my (odd) app");
        assert_eq!((state, ppid, threads, start, ticks), ('S', 99, 5, 4242, 10));
    }

    #[test]
    fn unit_ids() {
        assert_eq!(
            app_id_from_unit(r"app-Hyprland-gtk\x2dlaunch-aeb31189.scope"),
            "gtk-launch"
        );
        assert_eq!(
            app_id_from_unit("app-org.chromium.Chromium-1700.scope"),
            "org.chromium.Chromium"
        );
        assert_eq!(
            app_id_from_unit("app-flatpak-com.core447.StreamController-658825150.scope"),
            "com.core447.StreamController"
        );
        assert_eq!(
            app_id_from_unit("app-Dropbox@autostart.service"),
            "Dropbox"
        );
        assert_eq!(app_id_from_unit("dictate.service"), "dictate");
        assert_eq!(app_id_from_unit("dbus-broker.service"), "dbus-broker");
        assert_eq!(
            app_id_from_unit("app-Hyprland-editor-06da36fe.scope"),
            "editor"
        );
    }

    #[test]
    fn script_names() {
        assert_eq!(
            script_name("/usr/bin/python3 /home/alex/.local/bin/launch-game.sh").as_deref(),
            Some("launch-game.sh")
        );
        assert_eq!(
            script_name("/bin/bash /usr/bin/steam").as_deref(),
            Some("steam")
        );
        assert_eq!(
            script_name("bash -e /x/run.sh --flag").as_deref(),
            Some("run.sh")
        );
        assert_eq!(script_name("bash -c echo hi"), None);
        assert_eq!(
            script_name("python3 /x/a.py -c conf").as_deref(),
            Some("a.py")
        );
        assert_eq!(script_name("-bash"), None);
    }

    fn info(pid: i32, ppid: i32, name: &str, cmdline: &str, mem: u64) -> ProcInfo {
        ProcInfo {
            pid,
            ppid,
            uid: 1000,
            name: name.into(),
            cmdline: cmdline.into(),
            state: 'S',
            threads: 1,
            cpu_pct: 0.0,
            mem_bytes: mem,
            gpu_pct: None,
            gpu_enc_pct: None,
            gpu_dec_pct: None,
            vram_bytes: None,
            start_time_ticks: 1,
            exe: None,
            disk_read_bps: None,
            disk_write_bps: None,
            started_unix_ms: 0,
            user: String::new(),
            critical: None,
        }
    }

    #[test]
    fn a_window_title_is_cut_on_a_character_boundary() {
        let long = "مرحبا".repeat(300);
        let cut = truncate_chars(&long, MAX_TITLE_CHARS);
        assert_eq!(cut.chars().count(), MAX_TITLE_CHARS);
        assert!(long.starts_with(&cut));
        assert_eq!(truncate_chars("short", MAX_TITLE_CHARS), "short");
    }

    #[test]
    fn critical_list() {
        assert_eq!(critical_kind(5, "Hyprland", ""), Some("session"));
        assert_eq!(critical_kind(5, "pipewire-pulse", ""), Some("audio"));
        assert_eq!(critical_kind(5, "python3", "/usr/bin/hwglanced --x"), Some("collector"));
        assert_eq!(critical_kind(1, "weird", ""), Some("system"));
        assert_eq!(critical_kind(5, "brave", "/opt/brave/brave --type=gpu"), None);
        assert_eq!(critical_kind(5, "bash", "-bash"), None);
    }

    #[test]
    fn kept_handles_read_like_paths_and_survive_a_dead_process() {
        let mut slot = None;
        let mut buf = [0u8; 2048];
        let me = std::process::id();
        let path = format!("/proc/{me}/stat");
        let n = read_kept(&mut slot, || path.clone(), &mut buf).expect("first read opens the file");
        assert!(slot.is_some());
        let first = parse_stat(std::str::from_utf8(&buf[..n]).unwrap()).unwrap().4;
        let n = read_kept(&mut slot, || unreachable!("the kept handle is used"), &mut buf).unwrap();
        assert_eq!(parse_stat(std::str::from_utf8(&buf[..n]).unwrap()).unwrap().4, first);
        // a child that has exited and been reaped: the kept handle fails and the path is gone
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let cpath = format!("/proc/{}/stat", child.id());
        let mut cslot = None;
        read_kept(&mut cslot, || cpath.clone(), &mut buf).expect("alive or zombie");
        child.wait().unwrap();
        assert_eq!(read_kept(&mut cslot, || cpath.clone(), &mut buf), None);
        assert!(cslot.is_none());
    }

    #[test]
    fn rounding_for_the_wire() {
        assert_eq!(round_to(1.5662651, 100.0), 1.57);
        assert_eq!(round_to(1234.56, 1.0), 1235.0);
        assert_eq!(serde_json::to_string(&round_to(0.123456, 100.0)).unwrap(), "0.12");
    }

    #[test]
    fn shell_rooted_group_names() {
        let with_script = [
            info(
                10,
                1,
                "python3",
                "/usr/bin/python3 /h/launch-game.sh",
                5,
            ),
            info(11, 10, "game", "game", 900),
        ];
        assert_eq!(group_name_from_members(&with_script), "launch-game.sh");
        let no_script = [
            info(10, 1, "bash", "bash", 5),
            info(11, 10, "ssh", "ssh", 50),
            info(12, 10, "node", "node", 90),
        ];
        assert_eq!(group_name_from_members(&no_script), "node");
        let only_shells = [
            info(10, 1, "bash", "-bash", 5),
            info(11, 10, "sh", "sh", 50),
        ];
        assert_eq!(group_name_from_members(&only_shells), "bash");
        let plain = [info(10, 1, "pipewire", "pipewire", 5)];
        assert_eq!(group_name_from_members(&plain), "pipewire");
    }

    #[test]
    fn io_and_passwd_parsing() {
        let io = "rchar: 10\nwchar: 5\nsyscr: 1\nsyscw: 1\nread_bytes: 4096\nwrite_bytes: 8192\ncancelled_write_bytes: 0\n";
        assert_eq!(parse_io(io), Some((4096, 8192)));
        assert_eq!(parse_io("rchar: 1\n"), None);
        let users = parse_passwd(
            "root:x:0:0::/root:/bin/bash\nalex:x:1000:1000:Alex:/home/alex:/bin/bash\nbad line\n",
        );
        assert_eq!(users[&0], "root");
        assert_eq!(users[&1000], "alex");
    }

    #[test]
    fn pretty() {
        assert_eq!(pretty_class("org.chromium.Chromium"), "Chromium");
        assert_eq!(pretty_class("steam_app_1142710"), "Steam app 1142710");
    }
}
