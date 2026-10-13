//! Opt-in plain-text readouts, rewritten every sample while `text_readouts = true` is set in
//! config.toml.
//!
//! Status bars, scripts and macro keypads can show a value without starting a program of their
//! own: the collector writes two lines (a label and the values) per readout to
//! `$XDG_RUNTIME_DIR/hwglance/text/KEY.txt` for cpu, gpu, ram and ssd, and a reader only
//! needs to read the file.

use hwglance_proto::{Summary, runtime_dir};
use std::fs;
use std::io;
use std::path::PathBuf;

const KEYS: [&str; 4] = ["cpu", "gpu", "ram", "ssd"];

pub fn dir() -> PathBuf {
    runtime_dir().join("text")
}

/// Rounds halves to even, so a reader that rounds the raw values with Python's `round()` agrees.
fn r(v: f64) -> Option<i64> {
    v.is_finite().then(|| v.round_ties_even() as i64)
}

/// The readout texts: file name, label and values. `None` for a readout whose values are
/// missing; its file is removed so that a reader never shows a stale value.
pub fn texts(s: &Summary) -> [(&'static str, Option<(&'static str, String)>); 4] {
    let cpu = s.cpu_usage_pct.and_then(r).map(|u| {
        let usage = format!("{u}%");
        match s.cpu_temp_c.and_then(r) {
            Some(t) => ("CPU", format!("{t}\u{b0} {usage}")),
            None => ("CPU", usage),
        }
    });
    let gpu = s
        .gpu_temp_c
        .and_then(r)
        .zip(s.gpu_usage_pct.and_then(r))
        .map(|(t, u)| ("GPU", format!("{t}\u{b0} {u}%")));
    let ram = s.ram_used_pct.and_then(r).map(|p| ("RAM", format!("{p}%")));
    let ssd = s
        .ssd_temp_max_c
        .and_then(r)
        .map(|t| ("SSD", format!("{t}\u{b0}C")));
    [(KEYS[0], cpu), (KEYS[1], gpu), (KEYS[2], ram), (KEYS[3], ssd)]
}

/// Write each readout's file atomically (temp file and rename), so a reader never sees half of one.
/// With `enabled` false nothing is written; a `text/` directory left by an earlier run with the
/// export on is removed, once.
pub fn update(enabled: bool, s: &Summary) -> io::Result<()> {
    if enabled {
        write(s)
    } else {
        remove_stale()
    }
}

/// Remove the four readout files an earlier run left behind, and the directory if that empties
/// it. Anything else in the directory is not ours and stays.
fn remove_stale() -> io::Result<()> {
    let dir = dir();
    if !dir.is_dir() {
        return Ok(());
    }
    for key in KEYS {
        for name in [format!("{key}.txt"), format!("{key}.txt.tmp")] {
            match fs::remove_file(dir.join(name)) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
    }
    // fails (and is ignored) when something else lives there
    fs::remove_dir(&dir).ok();
    Ok(())
}

fn write(s: &Summary) -> io::Result<()> {
    let dir = dir();
    fs::create_dir_all(&dir)?;
    let mut result = Ok(());
    for (key, text) in texts(s) {
        let path = dir.join(format!("{key}.txt"));
        let step = match text {
            Some((label, values)) => {
                let tmp = dir.join(format!("{key}.txt.tmp"));
                fs::write(&tmp, format!("{label}\n{values}\n")).and_then(|_| fs::rename(&tmp, &path))
            }
            None => match fs::remove_file(&path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
        };
        if step.is_err() {
            result = step;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> Summary {
        Summary {
            cpu_usage_pct: Some(12.5),
            cpu_temp_c: Some(54.6),
            gpu_usage_pct: Some(99.4),
            gpu_temp_c: Some(61.5),
            ram_used_pct: Some(0.5),
            ssd_temp_max_c: Some(40.49),
            ..Summary::default()
        }
    }

    fn get(s: &Summary, key: &str) -> Option<(&'static str, String)> {
        texts(s).into_iter().find(|t| t.0 == key).unwrap().1
    }

    #[test]
    fn texts_round_halves_to_even() {
        let s = summary();
        // halves round to even: 12.5 is 12, 61.5 is 62, 0.5 is 0
        assert_eq!(get(&s, "cpu"), Some(("CPU", "55\u{b0} 12%".to_string())));
        assert_eq!(get(&s, "gpu"), Some(("GPU", "62\u{b0} 99%".to_string())));
        assert_eq!(get(&s, "ram"), Some(("RAM", "0%".to_string())));
        assert_eq!(get(&s, "ssd"), Some(("SSD", "40\u{b0}C".to_string())));
    }

    #[test]
    fn missing_values() {
        let mut s = summary();
        s.cpu_temp_c = None;
        assert_eq!(get(&s, "cpu"), Some(("CPU", "12%".to_string())));
        s.cpu_usage_pct = None;
        assert_eq!(get(&s, "cpu"), None);
        s.gpu_temp_c = None;
        assert_eq!(get(&s, "gpu"), None);
        s.ssd_temp_max_c = Some(f64::NAN);
        assert_eq!(get(&s, "ssd"), None);
    }

    #[test]
    fn disabled_writes_nothing_enabled_writes_four_files() {
        let base = std::env::temp_dir().join(format!("hwglance-text-{}", std::process::id()));
        fs::remove_dir_all(&base).ok();
        // SAFETY: this test is the only one that reads XDG_RUNTIME_DIR in this binary.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &base) };
        assert!(dir().ends_with("hwglance/text"));
        let mut s = summary();

        update(false, &s).unwrap();
        assert!(!dir().exists(), "disabled writes nothing");

        update(true, &s).unwrap();
        for key in KEYS {
            assert!(dir().join(format!("{key}.txt")).is_file(), "{key}.txt");
        }
        assert_eq!(fs::read_to_string(dir().join("cpu.txt")).unwrap(), "CPU\n55\u{b0} 12%\n");
        assert_eq!(fs::read_to_string(dir().join("ssd.txt")).unwrap(), "SSD\n40\u{b0}C\n");
        assert!(!dir().join("cpu.txt.tmp").exists());

        s.gpu_usage_pct = None;
        update(true, &s).unwrap();
        assert!(!dir().join("gpu.txt").exists());
        assert!(dir().join("ram.txt").exists());

        // switched off again: the leftovers go, a foreign file keeps the directory
        fs::write(dir().join("mine.txt"), "x").unwrap();
        update(false, &s).unwrap();
        assert!(!dir().join("cpu.txt").exists());
        assert!(dir().join("mine.txt").exists());
        fs::remove_file(dir().join("mine.txt")).unwrap();
        update(false, &s).unwrap();
        assert!(!dir().exists());
        fs::remove_dir_all(&base).ok();
    }
}
