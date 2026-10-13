//! Desktop entries: display names and icons for windows and services. Cached, rescanned every 60 s.

use crate::util::basename;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct Entry {
    /// File name without `.desktop`.
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub wm_class: Option<String>,
    pub exec_base: Option<String>,
    pub no_display: bool,
    /// Steam app id when the entry launches `steam://rungameid/<id>`.
    pub steam_id: Option<String>,
}

#[derive(Default)]
pub struct Desktop {
    entries: Vec<Entry>,
    by_id: HashMap<String, usize>,
    by_wm_class: HashMap<String, usize>,
    by_last_segment: HashMap<String, usize>,
    by_exec: HashMap<String, usize>,
    by_steam: HashMap<String, usize>,
    loaded: Option<Instant>,
}

fn dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut v = vec![
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
    ];
    if let Some(h) = &home {
        v.push(h.join(".local/share/applications"));
    }
    v.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    if let Some(h) = &home {
        v.push(h.join(".local/share/flatpak/exports/share/applications"));
    }
    v
}

/// First word of an `Exec=` line that is the program (skipping `env VAR=x`), as a basename.
fn exec_basename(exec: &str) -> Option<String> {
    for w in exec.split_whitespace() {
        let w = w.trim_matches('"');
        if w == "env" || w.contains('=') || w.starts_with('-') {
            continue;
        }
        return Some(basename(w).to_string());
    }
    None
}

pub fn parse_entry(id: &str, text: &str) -> Option<Entry> {
    let mut in_main = false;
    let (mut name, mut icon, mut wm, mut exec, mut hidden) = (None, None, None, None, false);
    let mut steam_id = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "Name" => name = Some(v.trim().to_string()),
                "Icon" => icon = Some(v.trim().to_string()).filter(|s| !s.is_empty()),
                "StartupWMClass" => wm = Some(v.trim().to_string()).filter(|s| !s.is_empty()),
                "Exec" => {
                    exec = exec_basename(v.trim());
                    steam_id = v.split_once("steam://rungameid/").map(|(_, rest)| {
                        rest.chars()
                            .take_while(char::is_ascii_digit)
                            .collect::<String>()
                    });
                    steam_id = steam_id.filter(|s| !s.is_empty());
                }
                "NoDisplay" => hidden = v.trim() == "true",
                _ => {}
            }
        }
    }
    Some(Entry {
        id: id.to_string(),
        name: name?,
        icon,
        wm_class: wm,
        // Launchers that only wrap other programs would match too much.
        exec_base: exec.filter(|e| {
            !matches!(
                e.as_str(),
                "flatpak" | "sh" | "bash" | "env" | "python3" | "python"
            )
        }),
        no_display: hidden,
        steam_id,
    })
}

impl Desktop {
    pub fn refresh_if_stale(&mut self) {
        if self.loaded.is_some_and(|t| t.elapsed() < REFRESH) {
            return;
        }
        let mut by_path: HashMap<String, Entry> = HashMap::new();
        for dir in dirs() {
            for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
                let fname = e.file_name().to_string_lossy().into_owned();
                let Some(id) = fname.strip_suffix(".desktop") else {
                    continue;
                };
                if let Some(entry) = fs::read_to_string(e.path())
                    .ok()
                    .and_then(|t| parse_entry(id, &t))
                {
                    by_path.insert(id.to_string(), entry); // later directories override earlier ones
                }
            }
        }
        let mut entries: Vec<Entry> = by_path.into_values().collect();
        // Shown entries first, so they win lookups where several share a key.
        entries.sort_by(|a, b| {
            a.no_display
                .cmp(&b.no_display)
                .then_with(|| a.id.cmp(&b.id))
        });
        let mut d = Desktop {
            entries,
            loaded: Some(Instant::now()),
            ..Default::default()
        };
        for (i, e) in d.entries.iter().enumerate() {
            let id = e.id.to_lowercase();
            d.by_last_segment
                .entry(id.rsplit('.').next().unwrap_or(&id).to_string())
                .or_insert(i);
            d.by_id.entry(id).or_insert(i);
            if let Some(w) = &e.wm_class {
                d.by_wm_class.entry(w.to_lowercase()).or_insert(i);
            }
            if let Some(s) = &e.steam_id {
                d.by_steam.entry(s.clone()).or_insert(i);
            }
            if let Some(x) = &e.exec_base {
                d.by_exec.entry(x.to_lowercase()).or_insert(i);
            }
        }
        *self = d;
    }

    /// The entry Steam wrote for a game (`Exec=steam steam://rungameid/<id>`).
    pub fn for_steam(&self, app_id: &str) -> Option<&Entry> {
        self.by_steam.get(app_id).map(|&i| &self.entries[i])
    }

    /// Window class lookup: desktop id, then StartupWMClass, then the id's last dot segment,
    /// then the program name of the app's root process (`exe_names` are lowercase).
    pub fn for_class(&self, class_lc: &str, exe_names: &[String]) -> Option<&Entry> {
        self.by_id
            .get(class_lc)
            .or_else(|| self.by_wm_class.get(class_lc))
            .or_else(|| self.by_last_segment.get(class_lc))
            .or_else(|| exe_names.iter().find_map(|n| self.by_exec.get(n)))
            .map(|&i| &self.entries[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        let t = "[Desktop Entry]\nName=Brave\nName[de]=Mutig\nIcon=brave-browser\nStartupWMClass=Brave-browser\nExec=/usr/bin/brave %U\n[Desktop Action x]\nName=New window\n";
        let e = parse_entry("brave-browser", t).unwrap();
        assert_eq!(e.name, "Brave");
        assert_eq!(e.icon.as_deref(), Some("brave-browser"));
        assert_eq!(e.exec_base.as_deref(), Some("brave"));
        assert_eq!(e.wm_class.as_deref(), Some("Brave-browser"));
        assert_eq!(e.steam_id, None);
        let g = "[Desktop Entry]\nName=Some Game\nIcon=steam_icon_42\nExec=steam steam://rungameid/42\n";
        let e = parse_entry("Some Game", g).unwrap();
        assert_eq!(e.steam_id.as_deref(), Some("42"));
        assert_eq!(e.icon.as_deref(), Some("steam_icon_42"));
    }

    #[test]
    fn exec_forms() {
        assert_eq!(
            exec_basename("env FOO=1 /opt/x/app --flag %U").as_deref(),
            Some("app")
        );
        assert_eq!(
            exec_basename("\"/usr/bin/steam\" %U").as_deref(),
            Some("steam")
        );
    }
}
