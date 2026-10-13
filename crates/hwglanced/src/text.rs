//! Human-readable dump for `hwglanced --once`.

use hwglance_proto::{ProcClass, Snapshot, format_value};

pub fn print(s: &Snapshot) {
    println!(
        "hwglanced snapshot: host {}, seq {}, interval {} ms",
        s.hostname, s.seq, s.interval_ms
    );
    for g in &s.sensors {
        println!();
        match &g.subtitle {
            Some(sub) => println!("== {} ({sub})", g.title),
            None => println!("== {}", g.title),
        }
        for sec in &g.sections {
            let tag = if sec.collapsed_by_default {
                " (collapsed)"
            } else {
                ""
            };
            println!("  [{}]{tag}", sec.title);
            for r in &sec.readings {
                let value = r
                    .value
                    .map(|v| format_value(r.unit, v))
                    .unwrap_or_else(|| "n/a".into());
                let note = r
                    .note
                    .as_deref()
                    .map(|n| format!("   # {n}"))
                    .unwrap_or_default();
                println!("    {:<30} {value}{note}", r.label);
            }
        }
    }
    println!();
    println!("== Sources");
    for src in &s.sources {
        println!(
            "  {:<9} {:<4} {}",
            src.name,
            if src.ok { "ok" } else { "DOWN" },
            src.detail
        );
    }
    if let Some(groups) = &s.procs {
        println!();
        let count = |c: ProcClass| groups.iter().filter(|g| g.class == c).count();
        println!(
            "== Processes: {} groups ({} apps, {} background, {} system), {} processes",
            groups.len(),
            count(ProcClass::App),
            count(ProcClass::Background),
            count(ProcClass::System),
            groups.iter().map(|g| g.procs.len()).sum::<usize>()
        );
        for g in groups {
            let class = match g.class {
                ProcClass::App => "app",
                ProcClass::Background => "bg",
                ProcClass::System => "sys",
            };
            let gpu = g
                .gpu_pct
                .map(|p| format!("  gpu {p:.0}%"))
                .unwrap_or_default();
            let vram = g
                .vram_bytes
                .map(|b| {
                    format!(
                        "  vram {}",
                        format_value(hwglance_proto::Unit::Bytes, b as f64)
                    )
                })
                .unwrap_or_default();
            println!(
                "  {class:<4} {:<34} {:>3} procs  cpu {:>5.1}%  mem {:>9}{gpu}{vram}   {}",
                g.name,
                g.procs.len(),
                g.cpu_pct,
                format_value(hwglance_proto::Unit::Bytes, g.mem_bytes as f64),
                g.id
            );
        }
    }
}
