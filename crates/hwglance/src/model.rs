//! Process list model: sorting, filtering and flattening of process groups into rows.
//! Pure functions, no egui, so everything here is unit tested.

use crate::search::Query;
use hwglance_proto::{ProcClass, ProcGroup, ProcInfo};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SortKey {
    Name,
    Pid,
    Cpu,
    Gpu,
    Vram,
    Mem,
    /// Storage read plus write.
    Disk,
    /// NVIDIA video encoder share.
    Encode,
    /// NVIDIA video decoder share.
    Decode,
    Threads,
    User,
    Started,
    Location,
}

impl SortKey {
    /// The optional table columns in their fixed order (Name is always shown).
    pub const OPTIONAL: [SortKey; 12] = [
        SortKey::Pid,
        SortKey::Cpu,
        SortKey::Gpu,
        SortKey::Vram,
        SortKey::Mem,
        SortKey::Disk,
        SortKey::Encode,
        SortKey::Decode,
        SortKey::Threads,
        SortKey::User,
        SortKey::Started,
        SortKey::Location,
    ];

    /// Names sort ascending on the first click, numbers and times descending.
    pub fn default_desc(self) -> bool {
        !matches!(self, SortKey::Name | SortKey::User | SortKey::Location)
    }

    /// Name used in ui.json.
    pub fn as_str(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Pid => "pid",
            SortKey::Cpu => "cpu",
            SortKey::Gpu => "gpu",
            SortKey::Vram => "vram",
            SortKey::Mem => "memory",
            SortKey::Disk => "disk",
            SortKey::Encode => "encode",
            SortKey::Decode => "decode",
            SortKey::Threads => "threads",
            SortKey::User => "user",
            SortKey::Started => "started",
            SortKey::Location => "location",
        }
    }

    pub fn parse(s: &str) -> Option<SortKey> {
        Some(match s {
            "name" => SortKey::Name,
            "pid" => SortKey::Pid,
            "cpu" => SortKey::Cpu,
            "gpu" => SortKey::Gpu,
            "vram" => SortKey::Vram,
            "memory" | "mem" => SortKey::Mem,
            "disk" => SortKey::Disk,
            "encode" => SortKey::Encode,
            "decode" => SortKey::Decode,
            "threads" => SortKey::Threads,
            "user" => SortKey::User,
            "started" => SortKey::Started,
            "location" => SortKey::Location,
            _ => return None,
        })
    }

    /// Title in the column header and the column checklists.
    pub fn title(self) -> &'static str {
        match self {
            SortKey::Name => "Name",
            SortKey::Pid => "PID",
            SortKey::Cpu => "CPU",
            SortKey::Gpu => "GPU",
            SortKey::Vram => "VRAM",
            SortKey::Mem => "Memory",
            SortKey::Disk => "Disk",
            SortKey::Encode => "Encode",
            SortKey::Decode => "Decode",
            SortKey::Threads => "Threads",
            SortKey::User => "User",
            SortKey::Started => "Started",
            SortKey::Location => "Location",
        }
    }
}

/// The columns shown by default, as written in ui.json.
pub const DEFAULT_COLUMNS: [&str; 7] = ["pid", "cpu", "gpu", "vram", "memory", "disk", "location"];

/// The optional columns to show for the saved ids: known ids only, once each, in the fixed
/// order of `SortKey::OPTIONAL` whatever order they were saved in.
pub fn visible_columns(saved: &[String]) -> Vec<SortKey> {
    SortKey::OPTIONAL.iter().copied().filter(|k| saved.iter().any(|s| s == k.as_str())).collect()
}

/// Saved ids after switching `key` on or off, in the fixed order.
pub fn toggle_column(saved: &[String], key: SortKey, on: bool) -> Vec<String> {
    let mut keys = visible_columns(saved);
    keys.retain(|k| *k != key);
    if on && key != SortKey::Name {
        keys.push(key);
    }
    SortKey::OPTIONAL.iter().filter(|k| keys.contains(k)).map(|k| k.as_str().to_string()).collect()
}

pub const CLASSES: [ProcClass; 3] = [ProcClass::App, ProcClass::Background, ProcClass::System];

pub fn class_key(c: ProcClass) -> &'static str {
    match c {
        ProcClass::App => "app",
        ProcClass::Background => "background",
        ProcClass::System => "system",
    }
}

pub fn class_title(c: ProcClass) -> &'static str {
    match c {
        ProcClass::App => "Apps",
        ProcClass::Background => "Background processes",
        ProcClass::System => "System",
    }
}

/// Identity of a row that survives snapshot refreshes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RowId {
    Section(ProcClass),
    Group(String),
    /// `pid` plus `start_time_ticks`, so a recycled pid is a different process.
    Proc(i32, u64),
}

#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    Section { groups: usize, procs: usize, collapsed: bool },
    /// `gi` indexes the snapshot's group list. `expandable` is false for single-process groups.
    Group { gi: usize, expanded: bool, expandable: bool },
    Proc { gi: usize, pi: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: RowId,
    pub kind: RowKind,
}

/// Everything that decides what the flattened list looks like.
pub struct ViewParams<'a> {
    pub sort: SortKey,
    pub desc: bool,
    /// Raw search text; matching ignores case and Arabic spelling variants (see `search`).
    pub filter: &'a str,
    /// Expanded group ids.
    pub expanded: &'a HashSet<String>,
    /// Explicit section collapsed state by `class_key`; `None` uses the default (System collapsed).
    pub section_collapsed: &'a BTreeMap<String, bool>,
}

pub fn section_is_collapsed(map: &BTreeMap<String, bool>, c: ProcClass) -> bool {
    map.get(class_key(c)).copied().unwrap_or(c == ProcClass::System)
}

fn cmp_f32(a: f32, b: f32) -> Ordering {
    a.total_cmp(&b)
}

fn cmp_opt_f32(a: Option<f32>, b: Option<f32>) -> Ordering {
    cmp_f32(a.unwrap_or(0.0), b.unwrap_or(0.0))
}

fn cmp_opt_u64(a: Option<u64>, b: Option<u64>) -> Ordering {
    a.unwrap_or(0).cmp(&b.unwrap_or(0))
}

fn name_cmp(a: &str, b: &str) -> Ordering {
    a.to_lowercase().cmp(&b.to_lowercase())
}

pub fn group_min_pid(g: &ProcGroup) -> i32 {
    g.procs.iter().map(|p| p.pid).min().unwrap_or(i32::MAX)
}

/// Read plus write, `None` when the kernel shows neither.
pub fn disk_total(read: Option<f32>, write: Option<f32>) -> Option<f32> {
    match (read, write) {
        (None, None) => None,
        (r, w) => Some(r.unwrap_or(0.0) + w.unwrap_or(0.0)),
    }
}

/// Thread count of a group: the sum over its processes.
pub fn group_threads(g: &ProcGroup) -> u32 {
    g.procs.iter().map(|p| p.threads).sum()
}

/// The owner of a group: its root process's user, else the first member's.
pub fn group_user(g: &ProcGroup) -> &str {
    group_root(g).map(|p| p.user.as_str()).or_else(|| g.procs.first().map(|p| p.user.as_str())).unwrap_or("")
}

/// When a group started: its oldest process.
pub fn group_started(g: &ProcGroup) -> u64 {
    g.procs.iter().map(|p| p.started_unix_ms).filter(|t| *t > 0).min().unwrap_or(0)
}

/// The process of a group whose parent is not in the group (the lowest pid when several).
fn group_root(g: &ProcGroup) -> Option<&ProcInfo> {
    let pids: HashSet<i32> = g.procs.iter().map(|p| p.pid).collect();
    g.procs.iter().filter(|p| !pids.contains(&p.ppid)).min_by_key(|p| p.pid)
}

/// The executable shown for a group: its root process's, else the one most members share
/// (ties go to the path that sorts first).
pub fn group_exe(g: &ProcGroup) -> Option<&str> {
    if let Some(e) = group_root(g).and_then(|p| p.exe.as_deref()) {
        return Some(e);
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for e in g.procs.iter().filter_map(|p| p.exe.as_deref()) {
        *counts.entry(e).or_default() += 1;
    }
    counts.into_iter().max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0))).map(|(e, _)| e)
}

fn cmp_group(a: &ProcGroup, b: &ProcGroup, key: SortKey, desc: bool) -> Ordering {
    let primary = match key {
        SortKey::Name => name_cmp(&a.name, &b.name),
        SortKey::Pid => group_min_pid(a).cmp(&group_min_pid(b)),
        SortKey::Cpu => cmp_f32(a.cpu_pct, b.cpu_pct),
        SortKey::Gpu => cmp_opt_f32(a.gpu_pct, b.gpu_pct),
        SortKey::Vram => cmp_opt_u64(a.vram_bytes, b.vram_bytes),
        SortKey::Mem => a.mem_bytes.cmp(&b.mem_bytes),
        SortKey::Disk => cmp_opt_f32(disk_total(a.disk_read_bps, a.disk_write_bps), disk_total(b.disk_read_bps, b.disk_write_bps)),
        SortKey::Encode => cmp_opt_f32(a.gpu_enc_pct, b.gpu_enc_pct),
        SortKey::Decode => cmp_opt_f32(a.gpu_dec_pct, b.gpu_dec_pct),
        SortKey::Threads => group_threads(a).cmp(&group_threads(b)),
        SortKey::User => name_cmp(group_user(a), group_user(b)),
        SortKey::Started => group_started(a).cmp(&group_started(b)),
        SortKey::Location => name_cmp(group_exe(a).unwrap_or(""), group_exe(b).unwrap_or("")),
    };
    let primary = if desc { primary.reverse() } else { primary };
    // Ties always fall back to name ascending, then id, so the order is stable.
    primary.then_with(|| name_cmp(&a.name, &b.name)).then_with(|| a.id.cmp(&b.id))
}

fn cmp_proc(a: &ProcInfo, b: &ProcInfo, key: SortKey, desc: bool) -> Ordering {
    let primary = match key {
        SortKey::Name => name_cmp(&a.name, &b.name),
        SortKey::Pid => a.pid.cmp(&b.pid),
        SortKey::Cpu => cmp_f32(a.cpu_pct, b.cpu_pct),
        SortKey::Gpu => cmp_opt_f32(a.gpu_pct, b.gpu_pct),
        SortKey::Vram => cmp_opt_u64(a.vram_bytes, b.vram_bytes),
        SortKey::Mem => a.mem_bytes.cmp(&b.mem_bytes),
        SortKey::Disk => cmp_opt_f32(disk_total(a.disk_read_bps, a.disk_write_bps), disk_total(b.disk_read_bps, b.disk_write_bps)),
        SortKey::Encode => cmp_opt_f32(a.gpu_enc_pct, b.gpu_enc_pct),
        SortKey::Decode => cmp_opt_f32(a.gpu_dec_pct, b.gpu_dec_pct),
        SortKey::Threads => a.threads.cmp(&b.threads),
        SortKey::User => name_cmp(&a.user, &b.user),
        SortKey::Started => a.started_unix_ms.cmp(&b.started_unix_ms),
        SortKey::Location => name_cmp(a.exe.as_deref().unwrap_or(""), b.exe.as_deref().unwrap_or("")),
    };
    let primary = if desc { primary.reverse() } else { primary };
    primary.then_with(|| name_cmp(&a.name, &b.name)).then_with(|| a.pid.cmp(&b.pid))
}

/// Does this process match the query (name, command line or pid; Arabic spelling variants and the
/// article ال do not matter, see `search`)?
pub fn proc_matches(p: &ProcInfo, q: &Query) -> bool {
    q.matches(&p.name) || q.matches(&p.cmdline) || p.pid.to_string().contains(q.plain())
}

/// Identity used to remember where a group or process sat the last time the list was sorted.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum OrderKey {
    Group(String),
    Proc(i32, u64),
}

/// Positions from the last real sort. While the order is held, known items keep these
/// positions and new items go to the end of their container.
#[derive(Default, Debug)]
pub struct OrderMemory {
    ranks: HashMap<OrderKey, u64>,
    next: u64,
}

impl OrderMemory {
    fn rank(&self, k: &OrderKey) -> Option<u64> {
        self.ranks.get(k).copied()
    }

    fn assign(&mut self, k: OrderKey) {
        self.ranks.entry(k).or_insert_with(|| {
            self.next += 1;
            self.next
        });
    }

    pub fn clear(&mut self) {
        self.ranks.clear();
        self.next = 0;
    }

    pub fn len(&self) -> usize {
        self.ranks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ranks.is_empty()
    }
}

/// Sort `items` with `cmp`; when holding, items already in `mem` keep their remembered order and
/// unknown items follow, ordered by `cmp`.
fn sort_held<T>(items: &mut [T], hold: bool, mem: &OrderMemory, key: impl Fn(&T) -> OrderKey, cmp: impl Fn(&T, &T) -> Ordering) {
    if !hold {
        items.sort_by(cmp);
        return;
    }
    items.sort_by(|a, b| match (mem.rank(&key(a)), mem.rank(&key(b))) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => cmp(a, b),
    });
}

/// Exponential moving average of CPU and GPU shares, keyed by process and by group, so the
/// list is readable and the sort order does not flicker. Time constant 0 disables smoothing.
#[derive(Default)]
pub struct Smoother {
    procs: HashMap<(i32, u64), (f32, f32)>,
    groups: HashMap<String, (f32, f32)>,
}

impl Smoother {
    /// Replace `cpu_pct` and `gpu_pct` of every group and process by their smoothed values.
    /// `dt` is the time since the previous snapshot in seconds, `tau` the time constant.
    pub fn apply(&mut self, groups: &mut [ProcGroup], dt: f32, tau: f32) {
        let alpha = if tau <= 0.0 { 1.0 } else { 1.0 - (-dt.max(0.0) / tau).exp() };
        let step = |prev: Option<&(f32, f32)>, cpu: f32, gpu: f32| -> (f32, f32) {
            match prev {
                Some(&(pc, pg)) => (pc + alpha * (cpu - pc), pg + alpha * (gpu - pg)),
                None => (cpu, gpu),
            }
        };
        let mut procs = HashMap::with_capacity(self.procs.len());
        let mut grps = HashMap::with_capacity(self.groups.len());
        for g in groups.iter_mut() {
            for p in g.procs.iter_mut() {
                let key = (p.pid, p.start_time_ticks);
                let (c, gp) = step(self.procs.get(&key), p.cpu_pct, p.gpu_pct.unwrap_or(0.0));
                procs.insert(key, (c, gp));
                p.cpu_pct = c;
                if p.gpu_pct.is_some() {
                    p.gpu_pct = Some(gp);
                }
            }
            let (c, gp) = step(self.groups.get(&g.id), g.cpu_pct, g.gpu_pct.unwrap_or(0.0));
            grps.insert(g.id.clone(), (c, gp));
            g.cpu_pct = c;
            if g.gpu_pct.is_some() {
                g.gpu_pct = Some(gp);
            }
        }
        // Entries of processes that exited are dropped here.
        self.procs = procs;
        self.groups = grps;
    }
}

/// Flatten groups into the visible row list: sections in the order Apps, Background, System,
/// groups sorted by their totals, children by the same key. With a filter, matching groups are
/// auto-expanded and sections ignore their collapsed state.
pub fn build_rows(groups: &[ProcGroup], p: &ViewParams, mem: &mut OrderMemory, hold: bool) -> Vec<Row> {
    let q = Query::new(p.filter);
    let filtering = !q.is_empty();
    let mut rows = Vec::with_capacity(groups.len() * 2);
    // Group keys with their child keys in display order, to refresh the order memory afterwards.
    let mut remember: Vec<(OrderKey, Vec<OrderKey>)> = Vec::new();

    for class in CLASSES {
        // (group index, indices of procs to show as children)
        let mut items: Vec<(usize, Vec<usize>)> = Vec::new();
        for (gi, g) in groups.iter().enumerate() {
            if g.class != class {
                continue;
            }
            let mut pis: Vec<usize> = if filtering {
                let name_hit = q.matches(&g.name) || q.matches(&g.id) || g.title.as_deref().is_some_and(|t| q.matches(t));
                if name_hit {
                    (0..g.procs.len()).collect()
                } else {
                    let hits: Vec<usize> =
                        (0..g.procs.len()).filter(|&i| proc_matches(&g.procs[i], &q)).collect();
                    if hits.is_empty() {
                        continue;
                    }
                    hits
                }
            } else {
                (0..g.procs.len()).collect()
            };
            sort_held(
                &mut pis,
                hold,
                mem,
                |&i| OrderKey::Proc(g.procs[i].pid, g.procs[i].start_time_ticks),
                |&a, &b| cmp_proc(&g.procs[a], &g.procs[b], p.sort, p.desc),
            );
            items.push((gi, pis));
        }
        if filtering && items.is_empty() {
            continue;
        }
        sort_held(
            &mut items,
            hold,
            mem,
            |it| OrderKey::Group(groups[it.0].id.clone()),
            |a, b| cmp_group(&groups[a.0], &groups[b.0], p.sort, p.desc),
        );

        let collapsed = !filtering && section_is_collapsed(p.section_collapsed, class);
        remember.extend(items.iter().map(|(gi, pis)| {
            (OrderKey::Group(groups[*gi].id.clone()), pis.iter().map(|&i| OrderKey::Proc(groups[*gi].procs[i].pid, groups[*gi].procs[i].start_time_ticks)).collect::<Vec<_>>())
        }));
        let proc_count: usize = items.iter().map(|(gi, _)| groups[*gi].procs.len()).sum();
        rows.push(Row {
            id: RowId::Section(class),
            kind: RowKind::Section { groups: items.len(), procs: proc_count, collapsed },
        });
        if collapsed {
            continue;
        }
        for (gi, pis) in items {
            let g = &groups[gi];
            let expandable = g.procs.len() > 1;
            let expanded = expandable && (filtering || p.expanded.contains(&g.id));
            rows.push(Row {
                id: RowId::Group(g.id.clone()),
                kind: RowKind::Group { gi, expanded, expandable },
            });
            if expanded {
                for pi in pis {
                    let pr = &g.procs[pi];
                    rows.push(Row {
                        id: RowId::Proc(pr.pid, pr.start_time_ticks),
                        kind: RowKind::Proc { gi, pi },
                    });
                }
            }
        }
    }
    if !hold {
        // A real sort: forget old positions that are not on screen anymore and store the new ones.
        // Keys of filtered-out or collapsed items are kept while their process still exists.
        let alive: HashSet<OrderKey> = groups
            .iter()
            .flat_map(|g| {
                std::iter::once(OrderKey::Group(g.id.clone()))
                    .chain(g.procs.iter().map(|pr| OrderKey::Proc(pr.pid, pr.start_time_ticks)))
            })
            .collect();
        mem.ranks.retain(|k, _| alive.contains(k));
        for (g, kids) in &remember {
            mem.ranks.remove(g);
            for k in kids {
                mem.ranks.remove(k);
            }
        }
    }
    for (g, kids) in remember {
        mem.assign(g);
        for k in kids {
            mem.assign(k);
        }
    }
    rows
}

pub fn row_index(rows: &[Row], id: &RowId) -> Option<usize> {
    rows.iter().position(|r| &r.id == id)
}

/// Processes targeted by an action.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// Name shown to the user.
    pub name: String,
    pub procs: Vec<TargetProc>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TargetProc {
    pub pid: i32,
    pub uid: u32,
    pub start_time_ticks: u64,
    pub name: String,
    pub cmdline: String,
    /// The collector's critical kind for this process, when it has one.
    pub critical: Option<String>,
}

impl From<&ProcInfo> for TargetProc {
    fn from(p: &ProcInfo) -> Self {
        TargetProc {
            pid: p.pid,
            uid: p.uid,
            start_time_ticks: p.start_time_ticks,
            name: p.name.clone(),
            cmdline: p.cmdline.clone(),
            critical: p.critical.clone(),
        }
    }
}

/// Resolve a selection to its target: all pids of a group, or a single process.
pub fn target_for(groups: &[ProcGroup], id: &RowId) -> Option<Target> {
    match id {
        RowId::Section(_) => None,
        RowId::Group(gid) => {
            let g = groups.iter().find(|g| &g.id == gid)?;
            Some(Target { name: g.name.clone(), procs: g.procs.iter().map(Into::into).collect() })
        }
        RowId::Proc(pid, ticks) => groups.iter().find_map(|g| {
            g.procs
                .iter()
                .find(|p| p.pid == *pid && p.start_time_ticks == *ticks)
                .map(|p| Target { name: p.name.clone(), procs: vec![p.into()] })
        }),
    }
}

/// The executable of a row: a process's own, a group's main one, none for sections.
pub fn row_exe<'a>(groups: &'a [ProcGroup], id: &RowId) -> Option<&'a str> {
    match id {
        RowId::Section(_) => None,
        RowId::Group(gid) => groups.iter().find(|g| &g.id == gid).and_then(group_exe),
        RowId::Proc(pid, ticks) => groups
            .iter()
            .flat_map(|g| g.procs.iter())
            .find(|p| p.pid == *pid && p.start_time_ticks == *ticks)
            .and_then(|p| p.exe.as_deref()),
    }
}

/// A path as shown in the table: the home directory is "~".
pub fn display_path(path: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    if !home.is_empty() {
        if path == home {
            return "~".to_string();
        }
        if let Some(rest) = path.strip_prefix(home).filter(|r| r.starts_with('/')) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

/// The folder an executable lives in.
pub fn parent_dir(path: &str) -> Option<&str> {
    let (dir, _) = path.rsplit_once('/')?;
    Some(if dir.is_empty() { "/" } else { dir })
}

/// `text` cut at the start so that its end stays visible: the whole text when `fits` accepts
/// it, otherwise "…" plus the longest tail that `fits` accepts (at least the last character).
pub fn ellipsize_start(text: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(text) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let tail = |n: usize| -> String { std::iter::once('\u{2026}').chain(chars[chars.len() - n..].iter().copied()).collect() };
    // the longest tail that fits (fits is monotone: a longer tail is never narrower)
    let (mut lo, mut hi) = (1usize, chars.len().saturating_sub(1).max(1));
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(&tail(mid)) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    tail(lo)
}

/// Why ending this target would hurt, if it would. Which processes are critical is the
/// collector's call (`ProcInfo::critical`, the kind of damage); this only words the warning.
pub fn critical_warning(t: &Target) -> Option<String> {
    if t.procs.iter().any(|p| p.pid == 1) {
        return Some("This is process 1 (init). Ending it takes down the whole system.".into());
    }
    let has = |kind: &str| t.procs.iter().any(|p| p.critical.as_deref() == Some(kind));
    let mut msg = String::new();
    if has("session") {
        msg.push_str("This ends your desktop session: every window and unsaved work in them is lost. ");
    }
    if has("system") {
        msg.push_str("systemd runs your user services and the system itself; ending it can log you out or destabilise the machine. ");
    }
    if has("xwayland") {
        msg.push_str("Xwayland hosts every X11 app, so they will all close. ");
    }
    if has("shell") {
        msg.push_str("This removes your bar and shell until it is restarted. ");
    }
    if has("audio") {
        msg.push_str("This cuts all audio and screen sharing until it restarts. ");
    }
    if has("collector") {
        msg.push_str("This stops the HW Glance collector, so this window loses its data. ");
    }
    // A kind a newer collector added that this window has no wording for.
    if msg.is_empty() && t.procs.iter().any(|p| p.critical.is_some()) {
        msg.push_str("Ending this can break your desktop session or the system. ");
    }
    (!msg.is_empty()).then(|| msg.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: i32, name: &str, cpu: f32, mem: u64) -> ProcInfo {
        ProcInfo {
            pid,
            ppid: 1,
            uid: 1000,
            name: name.into(),
            cmdline: format!("/usr/bin/{name} --flag"),
            state: 'S',
            threads: 1,
            cpu_pct: cpu,
            mem_bytes: mem,
            gpu_pct: None,
            gpu_enc_pct: None,
            gpu_dec_pct: None,
            vram_bytes: None,
            start_time_ticks: pid as u64 * 10,
            exe: None,
            disk_read_bps: None,
            disk_write_bps: None,
            started_unix_ms: 0,
            user: String::new(),
            critical: None,
        }
    }

    fn group(id: &str, class: ProcClass, name: &str, procs: Vec<ProcInfo>) -> ProcGroup {
        let cpu_pct = procs.iter().map(|p| p.cpu_pct).sum();
        let mem_bytes = procs.iter().map(|p| p.mem_bytes).sum();
        ProcGroup {
            id: id.into(),
            class,
            disk_read_bps: None,
            disk_write_bps: None,
            gpu_enc_pct: None,
            gpu_dec_pct: None,
            name: name.into(),
            icon: None,
            title: None,
            procs,
            cpu_pct,
            mem_bytes,
            gpu_pct: None,
            vram_bytes: None,
        }
    }

    fn sample() -> Vec<ProcGroup> {
        vec![
            group("app:brave", ProcClass::App, "Brave", vec![proc(10, "brave", 5.0, 900), proc(11, "brave-gpu", 20.0, 100), proc(12, "brave-renderer", 1.0, 5000)]),
            group("app:steam", ProcClass::App, "Steam", vec![proc(20, "steam", 8.0, 700)]),
            group("unit:dictate", ProcClass::Background, "dictate", vec![proc(30, "dictate", 0.1, 50)]),
            group("kthreads", ProcClass::System, "Kernel threads", vec![proc(2, "kthreadd", 0.0, 0), proc(3, "rcu_gp", 0.0, 0)]),
        ]
    }

    fn params<'a>(
        sort: SortKey,
        desc: bool,
        filter: &'a str,
        expanded: &'a HashSet<String>,
        sc: &'a BTreeMap<String, bool>,
    ) -> ViewParams<'a> {
        ViewParams { sort, desc, filter, expanded, section_collapsed: sc }
    }

    fn build(g: &[ProcGroup], p: &ViewParams) -> Vec<Row> {
        build_rows(g, p, &mut OrderMemory::default(), false)
    }

    fn ids(rows: &[Row]) -> Vec<RowId> {
        rows.iter().map(|r| r.id.clone()).collect()
    }

    #[test]
    fn sections_in_order_system_collapsed() {
        let g = sample();
        let (e, sc) = (HashSet::new(), BTreeMap::new());
        let rows = build(&g, &params(SortKey::Cpu, true, "", &e, &sc));
        let sections: Vec<_> = rows
            .iter()
            .filter_map(|r| match r.id {
                RowId::Section(c) => Some(c),
                _ => None,
            })
            .collect();
        assert_eq!(sections, vec![ProcClass::App, ProcClass::Background, ProcClass::System]);
        // System collapsed by default: no kernel group row, but the section row counts procs.
        assert!(!ids(&rows).contains(&RowId::Group("kthreads".into())));
        let sys = rows.last().unwrap();
        assert_eq!(sys.kind, RowKind::Section { groups: 1, procs: 2, collapsed: true });
    }

    #[test]
    fn sort_groups_by_totals_desc_and_asc() {
        let g = sample();
        let (e, sc) = (HashSet::new(), BTreeMap::new());
        // brave total cpu 26, steam 8
        let rows = build(&g, &params(SortKey::Cpu, true, "", &e, &sc));
        assert_eq!(rows[1].id, RowId::Group("app:brave".into()));
        assert_eq!(rows[2].id, RowId::Group("app:steam".into()));
        let rows = build(&g, &params(SortKey::Cpu, false, "", &e, &sc));
        assert_eq!(rows[1].id, RowId::Group("app:steam".into()));
    }

    #[test]
    fn sort_by_name_and_memory() {
        let g = sample();
        let (e, sc) = (HashSet::new(), BTreeMap::new());
        let rows = build(&g, &params(SortKey::Name, false, "", &e, &sc));
        assert_eq!(rows[1].id, RowId::Group("app:brave".into()));
        // brave mem 6000, steam 700
        let rows = build(&g, &params(SortKey::Mem, true, "", &e, &sc));
        assert_eq!(rows[1].id, RowId::Group("app:brave".into()));
        let rows = build(&g, &params(SortKey::Mem, false, "", &e, &sc));
        assert_eq!(rows[1].id, RowId::Group("app:steam".into()));
    }

    #[test]
    fn children_sorted_by_same_key() {
        let g = sample();
        let mut e = HashSet::new();
        e.insert("app:brave".to_string());
        let sc = BTreeMap::new();
        let rows = build(&g, &params(SortKey::Cpu, true, "", &e, &sc));
        let kids: Vec<i32> = rows
            .iter()
            .filter_map(|r| match r.id {
                RowId::Proc(pid, _) => Some(pid),
                _ => None,
            })
            .collect();
        assert_eq!(kids, vec![11, 10, 12]);
        let rows = build(&g, &params(SortKey::Mem, true, "", &e, &sc));
        let kids: Vec<i32> = rows
            .iter()
            .filter_map(|r| match r.id {
                RowId::Proc(pid, _) => Some(pid),
                _ => None,
            })
            .collect();
        assert_eq!(kids, vec![12, 10, 11]);
    }

    #[test]
    fn single_process_groups_are_not_expandable() {
        let g = sample();
        let mut e = HashSet::new();
        e.insert("app:steam".to_string());
        let sc = BTreeMap::new();
        let rows = build(&g, &params(SortKey::Name, false, "", &e, &sc));
        let steam = rows.iter().find(|r| r.id == RowId::Group("app:steam".into())).unwrap();
        assert_eq!(steam.kind, RowKind::Group { gi: 1, expanded: false, expandable: false });
        assert!(!rows.iter().any(|r| matches!(r.id, RowId::Proc(20, _))));
    }

    #[test]
    fn filter_by_name_cmdline_and_pid_expands() {
        let g = sample();
        let (e, sc) = (HashSet::new(), BTreeMap::new());
        // child name match: only matching child shown, group auto-expanded
        let rows = build(&g, &params(SortKey::Cpu, true, "renderer", &e, &sc));
        assert_eq!(
            ids(&rows),
            vec![
                RowId::Section(ProcClass::App),
                RowId::Group("app:brave".into()),
                RowId::Proc(12, 120)
            ]
        );
        // pid match
        let rows = build(&g, &params(SortKey::Cpu, true, "11", &e, &sc));
        assert!(rows.iter().any(|r| r.id == RowId::Proc(11, 110)));
        // cmdline match
        let rows = build(&g, &params(SortKey::Cpu, true, "--flag", &e, &sc));
        // every sample command line has --flag, and filtering ignores the collapsed System section
        assert!(rows.iter().any(|r| r.id == RowId::Group("kthreads".into())));
        assert!(rows.iter().any(|r| r.id == RowId::Group("app:steam".into())));
        // group-name match shows all children; ignores collapsed System section
        let rows = build(&g, &params(SortKey::Cpu, true, "kernel", &e, &sc));
        assert!(rows.iter().any(|r| r.id == RowId::Proc(2, 20)));
        assert!(rows.iter().any(|r| r.id == RowId::Proc(3, 30)));
        // no match: nothing at all
        assert!(build(&g, &params(SortKey::Cpu, true, "zzzz", &e, &sc)).is_empty());
    }

    /// Groups with Arabic names, window titles and command lines.
    fn arabic_sample() -> Vec<ProcGroup> {
        let mut notes = group("app:notes", ProcClass::App, "المفكرة", vec![proc(40, "notes", 1.0, 10)]);
        notes.title = Some("مُلاحَظاتي - إنشاء ملف".into());
        let mut viewer = group("app:viewer", ProcClass::App, "Viewer", vec![proc(41, "viewer", 1.0, 10)]);
        viewer.procs[0].cmdline = "/usr/bin/viewer --open /home/alex/مستشفى/الأخبار.txt".into();
        let plain = group("app:other", ProcClass::App, "Other", vec![proc(42, "other", 1.0, 10)]);
        vec![notes, viewer, plain]
    }

    fn groups_for(filter: &str) -> Vec<String> {
        let g = arabic_sample();
        let (e, sc) = (HashSet::new(), BTreeMap::new());
        build(&g, &params(SortKey::Name, false, filter, &e, &sc))
            .iter()
            .filter_map(|r| match &r.id {
                RowId::Group(id) => Some(id.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_search_ignores_arabic_spelling_variants_in_names_titles_and_command_lines() {
        // a group name, with and without the article and with a different spelling
        assert_eq!(groups_for("مفكرة"), vec!["app:notes"]);
        assert_eq!(groups_for("المفكره"), vec!["app:notes"]);
        // a window title: diacritics on one side, an alef with hamza on the other
        assert_eq!(groups_for("ملاحظاتي"), vec!["app:notes"]);
        assert_eq!(groups_for("انشاء"), vec!["app:notes"]);
        // a command line: teh marbuta against heh and alef maksura against yeh, the article off
        assert_eq!(groups_for("مستشفي"), vec!["app:viewer"]);
        assert_eq!(groups_for("اخبار"), vec!["app:viewer"]);
        assert_eq!(groups_for("الاخبار.txt"), vec!["app:viewer"]);
        // plain English is unchanged, and no match is no match
        assert_eq!(groups_for("VIEWER"), vec!["app:viewer"]);
        assert!(groups_for("متصفح").is_empty());
    }

    #[test]
    fn an_arabic_hit_in_a_child_expands_its_group_like_an_english_one() {
        let mut g = arabic_sample();
        g[2].procs.push(proc(43, "المساعد", 1.0, 10));
        let (e, sc) = (HashSet::new(), BTreeMap::new());
        let rows = build(&g, &params(SortKey::Name, false, "مساعد", &e, &sc));
        assert!(rows.iter().any(|r| r.id == RowId::Proc(43, 430)));
        assert!(!rows.iter().any(|r| r.id == RowId::Proc(42, 420)));
    }

    #[test]
    fn expanded_survives_resort() {
        let g = sample();
        let mut e = HashSet::new();
        e.insert("app:brave".to_string());
        let sc = BTreeMap::new();
        for key in [SortKey::Cpu, SortKey::Name, SortKey::Pid] {
            let rows = build(&g, &params(key, key.default_desc(), "", &e, &sc));
            let brave = rows.iter().find(|r| r.id == RowId::Group("app:brave".into())).unwrap();
            assert!(matches!(brave.kind, RowKind::Group { expanded: true, .. }));
            assert_eq!(rows.iter().filter(|r| matches!(r.kind, RowKind::Proc { .. })).count(), 3);
        }
    }

    #[test]
    fn section_override_opens_system() {
        let g = sample();
        let e = HashSet::new();
        let mut sc = BTreeMap::new();
        sc.insert("system".to_string(), false);
        sc.insert("app".to_string(), true);
        let rows = build(&g, &params(SortKey::Name, false, "", &e, &sc));
        assert!(rows.iter().any(|r| r.id == RowId::Group("kthreads".into())));
        assert!(!rows.iter().any(|r| r.id == RowId::Group("app:brave".into())));
    }

    #[test]
    fn targets_and_critical() {
        let g = sample();
        let t = target_for(&g, &RowId::Group("app:brave".into())).unwrap();
        assert_eq!(t.procs.len(), 3);
        assert_eq!(critical_warning(&t), None);
        let t = target_for(&g, &RowId::Proc(11, 110)).unwrap();
        assert_eq!(t.procs.len(), 1);
        assert_eq!(t.name, "brave-gpu");
        assert!(target_for(&g, &RowId::Proc(11, 999)).is_none());

        let crit = Target {
            name: "Hyprland".into(),
            procs: vec![TargetProc { pid: 900, uid: 1000, start_time_ticks: 1, name: "Hyprland".into(), cmdline: String::new(), critical: Some("session".into()) }],
        };
        assert!(critical_warning(&crit).unwrap().contains("desktop session"));
        let init = Target {
            name: "init".into(),
            procs: vec![TargetProc { pid: 1, uid: 0, start_time_ticks: 1, name: "weird".into(), cmdline: String::new(), critical: None }],
        };
        assert!(critical_warning(&init).is_some());
        let mixed = Target {
            name: "x".into(),
            procs: vec![
                TargetProc { pid: 5, uid: 1000, start_time_ticks: 1, name: "pipewire".into(), cmdline: String::new(), critical: Some("audio".into()) },
                TargetProc { pid: 6, uid: 1000, start_time_ticks: 1, name: "hwglanced".into(), cmdline: String::new(), critical: Some("collector".into()) },
            ],
        };
        let msg = critical_warning(&mixed).unwrap();
        assert!(msg.contains("audio") && msg.contains("collector"));
        // a name on the old window list means nothing without the collector's flag
        let unflagged = Target {
            name: "Hyprland".into(),
            procs: vec![TargetProc { pid: 900, uid: 1000, start_time_ticks: 1, name: "Hyprland".into(), cmdline: String::new(), critical: None }],
        };
        assert_eq!(critical_warning(&unflagged), None);
        let newer = Target {
            name: "x".into(),
            procs: vec![TargetProc { pid: 7, uid: 1000, start_time_ticks: 1, name: "x".into(), cmdline: String::new(), critical: Some("future".into()) }],
        };
        assert!(critical_warning(&newer).is_some());
    }

    fn group_ids(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .filter_map(|r| match &r.id {
                RowId::Group(g) => Some(g.clone()),
                _ => None,
            })
            .collect()
    }

    fn proc_pids(rows: &[Row]) -> Vec<i32> {
        rows.iter()
            .filter_map(|r| match r.id {
                RowId::Proc(p, _) => Some(p),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn held_order_ignores_value_changes_new_go_last_exited_vanish() {
        let mut g = sample();
        let mut e = HashSet::new();
        e.insert("app:brave".to_string());
        let sc = BTreeMap::new();
        let mut mem = OrderMemory::default();
        let p = params(SortKey::Cpu, true, "", &e, &sc);
        let rows = build_rows(&g, &p, &mut mem, false);
        assert_eq!(group_ids(&rows)[..2], ["app:brave", "app:steam"]);
        assert_eq!(proc_pids(&rows), vec![11, 10, 12]);

        // Steam jumps ahead and brave's renderer becomes the busiest: a re-sort would reorder both.
        g[1].procs[0].cpu_pct = 90.0;
        g[1].cpu_pct = 90.0;
        g[0].procs[2].cpu_pct = 70.0;
        g[0].cpu_pct = 5.0 + 20.0 + 70.0;
        let fresh = build_rows(&g, &p, &mut OrderMemory::default(), false);
        assert_eq!(proc_pids(&fresh), vec![12, 11, 10]);
        let held = build_rows(&g, &p, &mut mem, true);
        assert_eq!(group_ids(&held)[..2], ["app:brave", "app:steam"]);
        assert_eq!(proc_pids(&held), vec![11, 10, 12], "values changed but rows did not move");

        // A new child lands at the end of its group, a new group at the end of its section; an
        // exited process disappears without moving the others.
        g[0].procs.push(proc(13, "brave-new", 99.0, 1));
        g.push(group("app:new", ProcClass::App, "NewApp", vec![proc(40, "newapp", 99.0, 1)]));
        g[0].procs.remove(1); // pid 11 exits
        let held = build_rows(&g, &p, &mut mem, true);
        assert_eq!(proc_pids(&held), vec![10, 12, 13]);
        assert_eq!(group_ids(&held)[..3], ["app:brave", "app:steam", "app:new"]);
        // holding again keeps the new items where they landed
        let held2 = build_rows(&g, &p, &mut mem, true);
        assert_eq!(ids(&held), ids(&held2));

        // A real re-sort puts everything in value order again.
        let resorted = build_rows(&g, &p, &mut mem, false);
        assert_eq!(group_ids(&resorted)[0], "app:new");
        assert_eq!(proc_pids(&resorted)[0], 13);
    }

    #[test]
    fn hold_with_empty_memory_sorts_normally() {
        let g = sample();
        let (e, sc) = (HashSet::new(), BTreeMap::new());
        let p = params(SortKey::Cpu, true, "", &e, &sc);
        let rows = build_rows(&g, &p, &mut OrderMemory::default(), true);
        assert_eq!(group_ids(&rows)[..2], ["app:brave", "app:steam"]);
    }

    #[test]
    fn memory_forgets_exited_processes_on_a_real_sort() {
        let mut g = sample();
        let (e, sc) = (HashSet::new(), BTreeMap::new());
        let p = params(SortKey::Cpu, true, "", &e, &sc);
        let mut mem = OrderMemory::default();
        build_rows(&g, &p, &mut mem, false);
        let before = mem.len();
        g.remove(0);
        build_rows(&g, &p, &mut mem, false);
        assert!(mem.len() < before);
    }

    #[test]
    fn smoother_converges_and_first_sample_is_raw() {
        let mut sm = Smoother::default();
        let mut g = sample();
        sm.apply(&mut g, 1.0, 3.0);
        assert_eq!(g[0].procs[1].cpu_pct, 20.0, "first sight is not smoothed");
        // a step from 20 to 80 moves 1 - e^(-1/3) = 28.3 % of the way each second
        let mut g2 = sample();
        g2[0].procs[1].cpu_pct = 80.0;
        sm.apply(&mut g2, 1.0, 3.0);
        let expect = 20.0 + (80.0 - 20.0) * (1.0 - (-1.0f32 / 3.0).exp());
        assert!((g2[0].procs[1].cpu_pct - expect).abs() < 1e-3, "{}", g2[0].procs[1].cpu_pct);
        // repeated samples approach the target
        for _ in 0..30 {
            let mut g3 = sample();
            g3[0].procs[1].cpu_pct = 80.0;
            sm.apply(&mut g3, 1.0, 3.0);
            g2 = g3;
        }
        assert!((g2[0].procs[1].cpu_pct - 80.0).abs() < 0.1);
    }

    #[test]
    fn smoother_zero_tau_is_passthrough_and_gpu_stays_none() {
        let mut sm = Smoother::default();
        let mut g = sample();
        sm.apply(&mut g, 1.0, 0.0);
        let mut g2 = sample();
        g2[0].procs[0].cpu_pct = 55.0;
        sm.apply(&mut g2, 1.0, 0.0);
        assert_eq!(g2[0].procs[0].cpu_pct, 55.0);
        assert_eq!(g2[0].procs[0].gpu_pct, None);
        // GPU values smooth too, groups by their own totals
        let mut a = sample();
        a[0].procs[0].gpu_pct = Some(40.0);
        a[0].gpu_pct = Some(40.0);
        let mut sm = Smoother::default();
        sm.apply(&mut a, 1.0, 3.0);
        let mut b = sample();
        b[0].procs[0].gpu_pct = Some(0.0);
        b[0].gpu_pct = Some(0.0);
        sm.apply(&mut b, 1.0, 3.0);
        assert!(b[0].procs[0].gpu_pct.unwrap() > 20.0);
        assert!(b[0].gpu_pct.unwrap() > 20.0);
    }

    #[test]
    fn sort_key_names_round_trip() {
        for k in std::iter::once(SortKey::Name).chain(SortKey::OPTIONAL) {
            assert_eq!(SortKey::parse(k.as_str()), Some(k));
        }
        assert_eq!(SortKey::parse("bogus"), None);
        // the ids in ui.json are fixed
        let ids: Vec<&str> = SortKey::OPTIONAL.iter().map(|k| k.as_str()).collect();
        assert_eq!(ids, ["pid", "cpu", "gpu", "vram", "memory", "disk", "encode", "decode", "threads", "user", "started", "location"]);
    }

    #[test]
    fn default_columns_and_their_order() {
        let saved: Vec<String> = DEFAULT_COLUMNS.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            visible_columns(&saved),
            vec![SortKey::Pid, SortKey::Cpu, SortKey::Gpu, SortKey::Vram, SortKey::Mem, SortKey::Disk, SortKey::Location]
        );
        // the order is the fixed one whatever was saved; unknown and repeated ids are ignored
        let messy: Vec<String> = ["location", "bogus", "cpu", "cpu", "name", "pid"].iter().map(|s| s.to_string()).collect();
        assert_eq!(visible_columns(&messy), vec![SortKey::Pid, SortKey::Cpu, SortKey::Location]);
        assert!(visible_columns(&[]).is_empty(), "Name is not optional, an empty list shows only it");
    }

    #[test]
    fn toggling_a_column_keeps_the_fixed_order() {
        let saved: Vec<String> = DEFAULT_COLUMNS.iter().map(|s| s.to_string()).collect();
        let with_user = toggle_column(&saved, SortKey::User, true);
        assert_eq!(with_user, ["pid", "cpu", "gpu", "vram", "memory", "disk", "user", "location"]);
        let without_gpu = toggle_column(&with_user, SortKey::Gpu, false);
        assert_eq!(without_gpu, ["pid", "cpu", "vram", "memory", "disk", "user", "location"]);
        assert_eq!(toggle_column(&saved, SortKey::Name, true), saved, "Name cannot be added: it is always shown");
        assert_eq!(toggle_column(&saved, SortKey::Pid, true), saved, "switching on a shown column changes nothing");
    }

    fn rich_sample() -> Vec<ProcGroup> {
        let mut g = sample();
        // brave: three processes, root 10 with its exe; steam: one; dictate: other user
        for (p, exe, user, started, disk) in [
            (10, "/usr/lib/brave-bin/brave", "alex", 3_000u64, Some((1.0e6f32, 0.5e6f32))),
            (11, "/usr/lib/brave-bin/brave", "alex", 4_000, Some((2.0e6, 0.0))),
            (12, "/usr/lib/brave-bin/helper", "alex", 5_000, None),
        ] {
            let pr = g[0].procs.iter_mut().find(|x| x.pid == p).unwrap();
            pr.exe = Some(exe.into());
            pr.user = user.into();
            pr.started_unix_ms = started;
            pr.disk_read_bps = disk.map(|d| d.0);
            pr.disk_write_bps = disk.map(|d| d.1);
            pr.threads = p as u32;
        }
        g[0].procs[1].ppid = 10;
        g[0].procs[2].ppid = 10;
        g[0].disk_read_bps = Some(3.0e6);
        g[0].disk_write_bps = Some(0.5e6);
        g[0].gpu_enc_pct = Some(2.0);
        g[0].gpu_dec_pct = None;
        let s = &mut g[1].procs[0];
        s.exe = Some("/usr/lib/steam/steam".into());
        s.user = "alex".into();
        s.started_unix_ms = 1_000;
        s.threads = 90;
        g[1].disk_write_bps = Some(80.0e6);
        g[1].gpu_dec_pct = Some(11.0);
        let v = &mut g[2].procs[0];
        v.exe = Some("/home/alex/.local/bin/dictate".into());
        v.user = "root".into();
        v.started_unix_ms = 9_000;
        g
    }

    /// The App groups in display order (sections sort within themselves).
    fn order(g: &[ProcGroup], key: SortKey, desc: bool) -> Vec<String> {
        let (e, sc) = (HashSet::new(), BTreeMap::new());
        let rows = build(g, &params(key, desc, "", &e, &sc));
        rows.iter()
            .filter_map(|r| match &r.id {
                RowId::Group(id) if id.starts_with("app:") => Some(id.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_new_column_sorts() {
        let g = rich_sample();
        // disk: steam 80 MB/s (write), brave 3.5 MB/s
        assert_eq!(order(&g, SortKey::Disk, true), ["app:steam", "app:brave"]);
        assert_eq!(order(&g, SortKey::Disk, false), ["app:brave", "app:steam"]);
        // encode and decode: only brave encodes, only steam decodes
        assert_eq!(order(&g, SortKey::Encode, true)[0], "app:brave");
        assert_eq!(order(&g, SortKey::Decode, true)[0], "app:steam");
        // threads: brave 10+11+12 = 33 (the sample's pids as threads), steam 90
        assert_eq!(group_threads(&g[0]), 33);
        assert_eq!(order(&g, SortKey::Threads, true), ["app:steam", "app:brave"]);
        assert_eq!(order(&g, SortKey::Threads, false), ["app:brave", "app:steam"]);
        // user alphabetically; equal users fall back to the name
        let mut u = g.clone();
        u[0].procs.iter_mut().for_each(|p| p.user = "zed".into());
        assert_eq!(order(&u, SortKey::User, false), ["app:steam", "app:brave"], "alex before zed");
        assert_eq!(order(&u, SortKey::User, true), ["app:brave", "app:steam"]);
        assert_eq!(order(&g, SortKey::User, false), ["app:brave", "app:steam"], "a tie: by name");
        // started by time: steam 1000, brave's oldest is 3000
        assert_eq!(order(&g, SortKey::Started, false), ["app:steam", "app:brave"]);
        assert_eq!(order(&g, SortKey::Started, true), ["app:brave", "app:steam"]);
        // location alphabetically by the group's executable
        assert_eq!(order(&g, SortKey::Location, false), ["app:brave", "app:steam"]);
        assert_eq!(order(&g, SortKey::Location, true), ["app:steam", "app:brave"]);
        // defaults: names ascending, numbers and times descending
        assert!(!SortKey::Location.default_desc() && !SortKey::User.default_desc() && !SortKey::Name.default_desc());
        assert!(SortKey::Disk.default_desc() && SortKey::Started.default_desc() && SortKey::Threads.default_desc());
    }

    #[test]
    fn the_children_of_a_group_sort_by_the_new_columns_too() {
        let g = rich_sample();
        let mut e = HashSet::new();
        e.insert("app:brave".to_string());
        let sc = BTreeMap::new();
        let rows = build(&g, &params(SortKey::Disk, true, "", &e, &sc));
        let kids: Vec<i32> = rows.iter().filter_map(|r| if let RowId::Proc(pid, _) = r.id { Some(pid) } else { None }).collect();
        assert_eq!(kids, vec![11, 10, 12], "2.0, 1.5 MB/s, then no value");
        let rows = build(&g, &params(SortKey::Location, false, "", &e, &sc));
        let kids: Vec<i32> = rows.iter().filter_map(|r| if let RowId::Proc(pid, _) = r.id { Some(pid) } else { None }).collect();
        assert_eq!(kids.last(), Some(&12), "helper sorts after brave");
    }

    #[test]
    fn a_groups_executable_is_its_roots_else_the_most_common() {
        let g = rich_sample();
        assert_eq!(group_exe(&g[0]), Some("/usr/lib/brave-bin/brave"), "root process 10");
        // without a root exe the most common one wins
        let mut b = g[0].clone();
        b.procs[0].exe = None;
        assert_eq!(group_exe(&b), Some("/usr/lib/brave-bin/brave"), "11 and 12 differ: 11 is the only brave, helper once, ties go to the first path");
        b.procs[1].exe = Some("/usr/lib/brave-bin/helper".into());
        assert_eq!(group_exe(&b), Some("/usr/lib/brave-bin/helper"), "two of three");
        // nobody has one: none
        for p in b.procs.iter_mut() {
            p.exe = None;
        }
        assert_eq!(group_exe(&b), None);
        assert_eq!(group_user(&g[0]), "alex");
        assert_eq!(group_started(&g[0]), 3_000, "the oldest process");
        assert_eq!(disk_total(None, None), None);
        assert_eq!(disk_total(Some(1.0), None), Some(1.0));
        assert_eq!(row_exe(&g, &RowId::Group("app:steam".into())), Some("/usr/lib/steam/steam"));
        assert_eq!(row_exe(&g, &RowId::Section(ProcClass::App)), None);
        let p10 = &g[0].procs[0];
        assert_eq!(row_exe(&g, &RowId::Proc(p10.pid, p10.start_time_ticks)), Some("/usr/lib/brave-bin/brave"));
    }

    #[test]
    fn paths_show_the_home_folder_as_a_tilde_and_cut_at_the_start() {
        assert_eq!(display_path("/home/alex/.local/bin/dictate", "/home/alex"), "~/.local/bin/dictate");
        assert_eq!(display_path("/home/alex", "/home/alex/"), "~");
        assert_eq!(display_path("/home/alexx/bin", "/home/alex"), "/home/alexx/bin", "only whole folders");
        assert_eq!(display_path("/usr/bin/foo", ""), "/usr/bin/foo");
        assert_eq!(parent_dir("/usr/lib/steam/steam"), Some("/usr/lib/steam"));
        assert_eq!(parent_dir("/steam"), Some("/"));
        assert_eq!(parent_dir("steam"), None);
        // one char per unit of width: the tail stays, the head becomes an ellipsis
        let path = "/usr/lib/steam/steamwebhelper";
        assert_eq!(ellipsize_start(path, |t| t.chars().count() <= 100), path, "fits whole");
        let cut = ellipsize_start(path, |t| t.chars().count() <= 26);
        assert_eq!(cut, "\u{2026}/lib/steam/steamwebhelper");
        assert!(cut.chars().count() <= 26 && cut.ends_with("steamwebhelper"));
        // the file name survives as far as it can
        let tiny = ellipsize_start(path, |t| t.chars().count() <= 8);
        assert_eq!(tiny, "\u{2026}bhelper");
        assert!(tiny.chars().count() <= 8 && tiny.starts_with('\u{2026}'));
        // nothing fits: still the last character with the ellipsis
        assert_eq!(ellipsize_start(path, |_| false), "\u{2026}r");
    }
}
