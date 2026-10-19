//! Housekeeping of the log folder, off the sampling thread: gzip finished CSV files and delete the
//! ones past the age limit. One worker thread takes the jobs one at a time at a low priority, so a
//! day of "all" readings (tens of MB, seconds of compression) never delays a sample.
//!
//! The worker touches only files whose names are exactly ours (`hwglance_proto::log_file_kind`),
//! never a subfolder or a symlink, and never the file the logger is writing: the logger keeps that
//! path in [`Worker::current`], and the worker looks at it just before it touches each file.

use flate2::Compression;
use flate2::write::GzEncoder;
use hwglance_proto::{LogFile, log_file_kind};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

/// What the worker is asked to do.
pub enum Job {
    /// Gzip one finished file.
    Compress(PathBuf),
    /// Look through a folder: delete what is past `keep_days` (0 keeps all), then gzip the plain
    /// files an earlier run left (when `compress`).
    Maintain { folder: PathBuf, compress: bool, keep_days: u32 },
    /// Answer when every earlier job is done (for the tests).
    #[cfg(test)]
    Barrier(Sender<()>),
}

pub struct Worker {
    tx: Sender<Job>,
    /// The file being written, or none. The logger holds this lock while it creates a file and
    /// sets the path, so the worker never sees a new file before it is marked.
    current: Arc<Mutex<Option<PathBuf>>>,
}

impl Worker {
    pub fn spawn() -> Worker {
        let (tx, rx) = mpsc::channel();
        let current = Arc::new(Mutex::new(None));
        let shared = current.clone();
        // Without a thread the jobs go nowhere: files stay plain and nothing is deleted.
        let _ = std::thread::Builder::new().name("log-files".into()).spawn(move || run(rx, shared));
        Worker { tx, current }
    }

    pub fn send(&self, job: Job) {
        let _ = self.tx.send(job);
    }

    pub fn current(&self) -> MutexGuard<'_, Option<PathBuf>> {
        self.current.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Wait until the worker has done everything sent so far.
    #[cfg(test)]
    pub fn wait(&self) {
        let (tx, rx) = mpsc::channel();
        self.send(Job::Barrier(tx));
        rx.recv_timeout(Duration::from_secs(30)).expect("the worker answers");
    }
}

/// Nice 10 for this thread only (Linux scales priority per thread); best effort.
fn lower_priority() {
    // SAFETY: setpriority only reads its arguments; gettid has none.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 10);
    }
}

fn run(rx: Receiver<Job>, current: Arc<Mutex<Option<PathBuf>>>) {
    lower_priority();
    let is_open = |p: &Path| current.lock().unwrap_or_else(|e| e.into_inner()).as_deref() == Some(p);
    // each distinct failure is told once, however often a sweep meets it again
    let mut told: HashSet<String> = HashSet::new();
    let mut tell = |msg: String| {
        if told.insert(msg.clone()) {
            eprintln!("hwglanced: log: {msg}");
        }
    };
    for job in rx {
        match job {
            Job::Compress(path) => compress_one(&path, &is_open, &mut tell),
            Job::Maintain { folder, compress, keep_days } => {
                let entries = match list(&folder) {
                    Ok(e) => e,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    Err(e) => {
                        tell(format!("cannot read {}: {}", folder.display(), e));
                        continue;
                    }
                };
                if keep_days > 0 {
                    let (removed, failures) = prune(&entries, keep_days, SystemTime::now(), &is_open);
                    for f in failures {
                        tell(f);
                    }
                    if removed > 0 {
                        eprintln!("hwglanced: log: removed {removed} {} older than {keep_days} days", if removed == 1 { "file" } else { "files" });
                    }
                }
                if compress {
                    for e in entries.iter().filter(|e| e.kind == LogFile::Plain && e.path.exists()) {
                        compress_one(&e.path, &is_open, &mut tell);
                    }
                }
            }
            #[cfg(test)]
            Job::Barrier(tx) => {
                let _ = tx.send(());
            }
        }
    }
}

fn compress_one(path: &Path, is_open: &dyn Fn(&Path) -> bool, tell: &mut dyn FnMut(String)) {
    if is_open(path) {
        return;
    }
    match compress_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound && !path.exists() => {} // gone meanwhile
        Err(e) => tell(format!("cannot compress {}: {}", path.display(), e)),
    }
}

/// A file of ours in the log folder.
pub struct Entry {
    pub path: PathBuf,
    pub kind: LogFile,
    pub modified: Option<SystemTime>,
}

/// The regular files of `folder` whose names are exactly ours (no subfolders, no symlinks).
pub fn list(folder: &Path) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in fs::read_dir(folder)?.flatten() {
        let Some(kind) = e.file_name().to_str().and_then(log_file_kind) else { continue };
        if !e.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        out.push(Entry { kind, modified: e.metadata().and_then(|m| m.modified()).ok(), path: e.path() });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Delete the entries last modified more than `keep_days` days before `now`, except the open
/// file. Returns how many went and what could not be deleted, in words.
pub fn prune(entries: &[Entry], keep_days: u32, now: SystemTime, is_open: &dyn Fn(&Path) -> bool) -> (usize, Vec<String>) {
    let Some(cutoff) = now.checked_sub(Duration::from_secs(u64::from(keep_days) * 86_400)) else { return (0, vec![]) };
    let (mut removed, mut failures) = (0, Vec::new());
    for e in entries {
        if e.modified.is_none_or(|m| m >= cutoff) || is_open(&e.path) {
            continue;
        }
        match fs::remove_file(&e.path) {
            Ok(()) => removed += 1,
            Err(err) => failures.push(format!("cannot remove {}: {}", e.path.display(), err)),
        }
    }
    (removed, failures)
}

/// Gzip `path` to `<path>.gz` and remove `path`, but only once the `.gz` is complete: it is
/// written as `<path>.gz.tmp`, finished, flushed to the disk and renamed. On any failure the plain
/// file stays and the temporary one goes. The `.gz` keeps the modification time of the original,
/// so the age limit counts from when the log was written, not from when it was packed.
pub fn compress_file(path: &Path) -> io::Result<()> {
    let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| io::Error::other("no file name"))?;
    let gz = path.with_file_name(format!("{name}.gz"));
    let tmp = path.with_file_name(format!("{name}.gz.tmp"));
    let packed = (|| {
        let mut src = File::open(path)?;
        let modified = src.metadata()?.modified().ok();
        // truncate: a leftover of a crashed run is ours to replace; a directory of that name is an
        // error, and so is a symlink (in a shared folder it could point at someone's file)
        let out = OpenOptions::new().write(true).create(true).truncate(true).custom_flags(libc::O_NOFOLLOW).open(&tmp)?;
        let mut enc = GzEncoder::new(BufWriter::new(out), Compression::default());
        io::copy(&mut src, &mut enc)?;
        let file = enc.finish()?.into_inner().map_err(|e| e.into_error())?;
        if let Some(t) = modified {
            let _ = file.set_modified(t);
        }
        file.sync_all()?;
        fs::rename(&tmp, &gz)
    })();
    if let Err(e) = packed {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::remove_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hwglance-logfiles-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn gunzip(p: &Path) -> Vec<u8> {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(File::open(p).unwrap()).read_to_end(&mut out).unwrap();
        out
    }

    fn body() -> Vec<u8> {
        let mut text = String::from("Time,Unix time (ms),CPU Tctl [°C]\n");
        for i in 0..2000 {
            text.push_str(&format!("2026-10-12 21:04:33,{},{}.{}\n", 1_760_000_000_000u64 + i * 1000, 50 + i % 20, i % 10));
        }
        text.into_bytes()
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        File::create(&p).unwrap().write_all(bytes).unwrap();
        p
    }

    fn age(p: &Path, days: u64) {
        let t = SystemTime::now() - Duration::from_secs(days * 86_400 + 3600);
        OpenOptions::new().write(true).open(p).unwrap().set_modified(t).unwrap();
    }

    fn never(_: &Path) -> bool {
        false
    }

    #[test]
    fn a_compressed_file_unpacks_to_the_original_bytes_and_the_plain_one_goes() {
        let dir = tmpdir("gz");
        let data = body();
        let p = write(&dir, "hwglance-2026-10-12_210433.csv", &data);
        age(&p, 3);
        let before = fs::metadata(&p).unwrap().modified().unwrap();
        compress_file(&p).unwrap();
        let gz = dir.join("hwglance-2026-10-12_210433.csv.gz");
        assert!(!p.exists() && gz.exists());
        assert!(!dir.join("hwglance-2026-10-12_210433.csv.gz.tmp").exists());
        assert_eq!(gunzip(&gz), data);
        assert!(fs::metadata(&gz).unwrap().len() * 3 < data.len() as u64, "it is much smaller");
        assert_eq!(fs::metadata(&gz).unwrap().modified().unwrap(), before, "the age is the log's, not the packing's");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn the_plain_file_stays_when_the_gz_cannot_be_written() {
        let dir = tmpdir("blocked");
        let data = body();
        let p = write(&dir, "hwglance-2026-10-12_210433.csv", &data);
        // a directory with the name of the temporary file
        let tmp = dir.join("hwglance-2026-10-12_210433.csv.gz.tmp");
        fs::create_dir(&tmp).unwrap();
        assert!(compress_file(&p).is_err());
        assert_eq!(fs::read(&p).unwrap(), data, "the log is untouched");
        assert!(tmp.is_dir(), "and what was in the way is not removed");
        assert!(!dir.join("hwglance-2026-10-12_210433.csv.gz").exists());
        // a directory where the .gz goes: the temporary file is cleaned up and the log stays
        fs::remove_dir(&tmp).unwrap();
        fs::create_dir(dir.join("hwglance-2026-10-12_210433.csv.gz")).unwrap();
        assert!(compress_file(&p).is_err());
        assert_eq!(fs::read(&p).unwrap(), data);
        assert!(!tmp.exists(), "no temporary file is left behind");
        // a folder that cannot be written (not for root, which ignores modes)
        let ro = tmpdir("readonly");
        let q = write(&ro, "hwglance-2026-10-12_210433.csv", &data);
        let mut perm = fs::metadata(&ro).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o555);
        fs::set_permissions(&ro, perm).unwrap();
        if File::create(ro.join("probe")).is_err() {
            assert!(compress_file(&q).is_err());
            assert_eq!(fs::read(&q).unwrap(), data);
        }
        let mut perm = fs::metadata(&ro).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
        fs::set_permissions(&ro, perm).unwrap();
        // a stale temporary file of a crashed run is replaced
        let dir = tmpdir("stale");
        let p = write(&dir, "hwglance-2026-10-12_210433.csv", &data);
        write(&dir, "hwglance-2026-10-12_210433.csv.gz.tmp", b"half a file");
        compress_file(&p).unwrap();
        assert_eq!(gunzip(&dir.join("hwglance-2026-10-12_210433.csv.gz")), data);
    }

    #[test]
    fn the_sweep_lists_only_our_plain_and_packed_files() {
        let dir = tmpdir("sweep");
        let names = [
            "hwglance-2026-10-11_080000.csv",
            "hwglance-2026-10-11_080000-2.csv",
            "hwglance-2026-10-10_080000.csv.gz",
            "hwglance-backup.csv",
            "notes.txt",
            "hwglance-2026-10-12_210433.csv.gz.tmp",
            "hwglance-2026-10-12_210433.csv.bak",
        ];
        for n in names {
            write(&dir, n, b"x");
        }
        fs::create_dir(dir.join("hwglance-2026-10-09_080000.csv")).unwrap(); // a folder with our name
        std::os::unix::fs::symlink(dir.join("notes.txt"), dir.join("hwglance-2026-10-08_080000.csv")).unwrap();
        let sub = dir.join("sub");
        fs::create_dir(&sub).unwrap();
        write(&sub, "hwglance-2026-10-07_080000.csv", b"x");
        let found: Vec<(String, LogFile)> = list(&dir).unwrap().into_iter().map(|e| (e.path.file_name().unwrap().to_str().unwrap().to_string(), e.kind)).collect();
        assert_eq!(
            found,
            [
                ("hwglance-2026-10-10_080000.csv.gz".to_string(), LogFile::Gzip),
                ("hwglance-2026-10-11_080000-2.csv".to_string(), LogFile::Plain),
                ("hwglance-2026-10-11_080000.csv".to_string(), LogFile::Plain),
            ]
        );
        assert!(list(&dir.join("missing")).is_err());
    }

    #[test]
    fn maintenance_packs_leftovers_but_never_the_open_file_or_anything_else() {
        let dir = tmpdir("maintain");
        let data = body();
        let old = write(&dir, "hwglance-2026-10-11_080000.csv", &data);
        let open = write(&dir, "hwglance-2026-10-12_210433.csv", &data);
        let other = write(&dir, "hwglance-backup.csv", &data);
        let notes = write(&dir, "notes.txt", &data);
        let worker = Worker::spawn();
        *worker.current() = Some(open.clone());
        worker.send(Job::Maintain { folder: dir.clone(), compress: true, keep_days: 0 });
        worker.wait();
        assert!(!old.exists() && dir.join("hwglance-2026-10-11_080000.csv.gz").exists());
        assert_eq!(gunzip(&dir.join("hwglance-2026-10-11_080000.csv.gz")), data);
        assert!(open.exists() && other.exists() && notes.exists(), "the open file and strangers are left alone");
        // once the file is finished it is packed on request
        *worker.current() = None;
        worker.send(Job::Compress(open.clone()));
        worker.wait();
        assert!(!open.exists() && dir.join("hwglance-2026-10-12_210433.csv.gz").exists());
        // asked for a file that is open: nothing happens
        let live = write(&dir, "hwglance-2026-10-13_000000.csv", &data);
        *worker.current() = Some(live.clone());
        worker.send(Job::Compress(live.clone()));
        worker.wait();
        assert!(live.exists());
        // with compression off nothing is packed
        let dir = tmpdir("maintain-off");
        let p = write(&dir, "hwglance-2026-10-11_080000.csv", &data);
        let worker = Worker::spawn();
        worker.send(Job::Maintain { folder: dir.clone(), compress: false, keep_days: 0 });
        worker.wait();
        assert_eq!(fs::read(&p).unwrap(), data);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        // a folder that is not there is no error
        worker.send(Job::Maintain { folder: dir.join("missing"), compress: true, keep_days: 7 });
        worker.wait();
    }

    #[test]
    fn files_past_the_age_limit_go_and_nothing_else() {
        let dir = tmpdir("prune");
        let old_csv = write(&dir, "hwglance-2026-09-01_080000.csv", b"x");
        let old_gz = write(&dir, "hwglance-2026-09-02_080000.csv.gz", b"x");
        let open = write(&dir, "hwglance-2026-09-03_080000.csv", b"x");
        let recent_csv = write(&dir, "hwglance-2026-10-10_080000.csv", b"x");
        let recent_gz = write(&dir, "hwglance-2026-10-11_080000.csv.gz", b"x");
        let notes = write(&dir, "notes.txt", b"x");
        let backup = write(&dir, "hwglance-backup.csv", b"x");
        let tmp = write(&dir, "hwglance-2026-09-04_080000.csv.gz.tmp", b"x");
        let sub = dir.join("sub");
        fs::create_dir(&sub).unwrap();
        let in_sub = write(&sub, "hwglance-2026-09-05_080000.csv", b"x");
        let dir_named = dir.join("hwglance-2026-09-06_080000.csv");
        fs::create_dir(&dir_named).unwrap();
        for p in [&old_csv, &old_gz, &open, &notes, &backup, &tmp, &in_sub] {
            age(p, 40);
        }
        age(&recent_csv, 6);
        age(&recent_gz, 0);
        let worker = Worker::spawn();
        *worker.current() = Some(open.clone());
        worker.send(Job::Maintain { folder: dir.clone(), compress: false, keep_days: 7 });
        worker.wait();
        for gone in [&old_csv, &old_gz] {
            assert!(!gone.exists(), "{gone:?}");
        }
        for kept in [&open, &recent_csv, &recent_gz, &notes, &backup, &tmp, &in_sub, &dir_named] {
            assert!(kept.exists(), "{kept:?}");
        }
        // the cut is by age: 6 days old stays at 7, goes at 5
        let entries = list(&dir).unwrap();
        let (n, failed) = prune(&entries, 5, SystemTime::now(), &|p: &Path| p == open);
        assert_eq!((n, failed.len()), (1, 0));
        assert!(!recent_csv.exists() && recent_gz.exists());
        // zero days is not a limit here (the caller skips it); one day removes what is a day old
        let (n, _) = prune(&list(&dir).unwrap(), 1, SystemTime::now(), &never);
        assert_eq!(n, 1, "only the open file, now not marked, is left over a day old");
        assert!(!open.exists() && recent_gz.exists());
    }
}
