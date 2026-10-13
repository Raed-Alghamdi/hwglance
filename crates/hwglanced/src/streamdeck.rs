//! Ready-made Stream Deck key texts, rewritten every sample.
//!
//! `streamdeck/sd-stat KEY` prints two lines (a label and the values) for one key. Starting
//! a script for that costs far more than the collector spends on the whole sample, so the
//! collector writes the two lines to `$XDG_RUNTIME_DIR/hwglance/streamdeck/KEY.txt` and the script
//! only reads that file while it is fresh. The texts match what the script's own logic prints.

use hwglance_proto::{Summary, runtime_dir};
use std::fs;
use std::io;
use std::path::PathBuf;

pub fn dir() -> PathBuf {
    runtime_dir().join("streamdeck")
}

/// `round()` as Python does it (halves to even), for values that must print the same.
fn r(v: f64) -> Option<i64> {
    v.is_finite().then(|| v.round_ties_even() as i64)
}

/// The key texts: file name, label and values. `None` for a key whose values are missing; its
/// file is removed so that the script falls back to reading the hardware itself.
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
    [("cpu", cpu), ("gpu", gpu), ("ram", ram), ("ssd", ssd)]
}

/// Write each key's file atomically (temp file and rename), so a reader never sees half of one.
pub fn write(s: &Summary) -> io::Result<()> {
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
    fn texts_match_the_script() {
        let s = summary();
        // Python rounds halves to even: round(12.5) is 12, round(61.5) is 62, round(0.5) is 0
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
    fn files_are_written_and_removed() {
        let base = std::env::temp_dir().join(format!("hwglance-sd-{}", std::process::id()));
        // SAFETY: this test is the only one that reads XDG_RUNTIME_DIR in this binary.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &base) };
        let mut s = summary();
        write(&s).unwrap();
        let cpu = fs::read_to_string(dir().join("cpu.txt")).unwrap();
        assert_eq!(cpu, "CPU\n55\u{b0} 12%\n");
        assert!(!dir().join("cpu.txt.tmp").exists());
        s.gpu_usage_pct = None;
        write(&s).unwrap();
        assert!(!dir().join("gpu.txt").exists());
        assert!(dir().join("ram.txt").exists());
        fs::remove_dir_all(&base).ok();
    }
}
