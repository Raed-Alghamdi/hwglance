//! Names for Steam games, whose windows have the class `steam_app_<id>` and no desktop id.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Default)]
pub struct SteamNames {
    /// app id -> name from the appmanifest (`None` when no library has one).
    cache: HashMap<String, Option<String>>,
}

/// `"key"  "value"` -> (key, value). Values use `\\` for a backslash.
fn quoted_pair(line: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = line.trim().split('"').collect();
    if parts.len() < 5 || !parts[0].is_empty() {
        return None;
    }
    Some((parts[1].to_string(), parts[3].replace("\\\\", "\\")))
}

/// Library folders listed in `libraryfolders.vdf`, plus the default Steam folder.
fn libraries() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let steam = home.join(".local/share/Steam");
    let mut libs = vec![steam.clone()];
    let text = fs::read_to_string(steam.join("steamapps/libraryfolders.vdf")).unwrap_or_default();
    for line in text.lines() {
        if let Some((k, v)) = quoted_pair(line)
            && k == "path"
        {
            let p = PathBuf::from(v);
            if !libs.contains(&p) {
                libs.push(p);
            }
        }
    }
    libs
}

fn manifest_name(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| quoted_pair(l).filter(|(k, _)| k == "name").map(|(_, v)| v))
        .filter(|n| !n.is_empty())
}

impl SteamNames {
    /// The game's name from `appmanifest_<id>.acf`, cached per id.
    pub fn name(&mut self, app_id: &str) -> Option<String> {
        self.cache
            .entry(app_id.to_string())
            .or_insert_with(|| {
                libraries().iter().find_map(|lib| {
                    let path = lib.join(format!("steamapps/appmanifest_{app_id}.acf"));
                    fs::read_to_string(path)
                        .ok()
                        .and_then(|t| manifest_name(&t))
                })
            })
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vdf_lines() {
        assert_eq!(
            quoted_pair("\t\t\"path\"\t\t\"/run/media/alex/games/SteamLibrary\""),
            Some(("path".into(), "/run/media/alex/games/SteamLibrary".into()))
        );
        assert_eq!(quoted_pair("\t\"0\""), None);
        assert_eq!(quoted_pair("{"), None);
    }

    #[test]
    fn manifest() {
        let t = "\"AppState\"\n{\n\t\"appid\"\t\t\"1142710\"\n\t\"name\"\t\t\"Total War: WARHAMMER III\"\n}\n";
        assert_eq!(
            manifest_name(t).as_deref(),
            Some("Total War: WARHAMMER III")
        );
    }
}
