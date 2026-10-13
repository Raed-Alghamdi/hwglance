//! Small helpers shared by the collectors.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// File contents with surrounding whitespace removed; `None` on any error.
pub fn read_trim<P: AsRef<Path>>(p: P) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

pub fn read_num<P: AsRef<Path>>(p: P) -> Option<f64> {
    read_trim(p)?.parse().ok()
}

/// Lowercase ASCII identifier: runs of non-alphanumerics become one `_`.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => s[..i].to_string(),
        None => s.to_string(),
    }
}

/// Decode systemd's `\xNN` escapes (cgroup and unit names use them for `-`, `/`, ...).
pub fn unescape_systemd(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1] == b'x'
            && let Some(h) = s.get(i + 2..i + 4)
            && let Ok(v) = u8::from_str_radix(h, 16)
        {
            out.push(v);
            i += 4;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn basename(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

pub fn hostname() -> String {
    read_trim("/proc/sys/kernel/hostname").unwrap_or_else(|| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("PECI/TSI Agent 0"), "peci_tsi_agent_0");
        assert_eq!(slug("Composite"), "composite");
        assert_eq!(slug("CPU_Opt"), "cpu_opt");
    }

    #[test]
    fn unescape() {
        assert_eq!(
            unescape_systemd(r"app-dbus\x2d:1.1\x2dorg"),
            "app-dbus-:1.1-org"
        );
        assert_eq!(unescape_systemd("plain"), "plain");
        assert_eq!(unescape_systemd(r"tail\x2"), r"tail\x2");
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate_chars("héllo", 2), "hé");
        assert_eq!(truncate_chars("hi", 5), "hi");
    }
}
