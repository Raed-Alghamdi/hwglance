//! /proc/meminfo.

use std::fs;

#[derive(Clone, Debug, Default)]
pub struct MemVal {
    pub total: u64,
    pub available: u64,
    /// Buffers + Cached.
    pub cache: u64,
    pub swap_used: u64,
    pub swap_total: u64,
}

pub fn read() -> Option<MemVal> {
    let text = fs::read_to_string("/proc/meminfo").ok()?;
    let get = |key: &str| -> u64 {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
            .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
            .map(|kb| kb * 1024)
            .unwrap_or(0)
    };
    let total = get("MemTotal");
    if total == 0 {
        return None;
    }
    Some(MemVal {
        total,
        available: get("MemAvailable"),
        cache: get("Cached") + get("Buffers"),
        swap_used: get("SwapTotal").saturating_sub(get("SwapFree")),
        swap_total: get("SwapTotal"),
    })
}
