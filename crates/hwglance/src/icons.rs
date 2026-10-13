//! Application icons from the freedesktop icon theme, cached by name.
//! Optional polish: everything falls back to a coloured first-letter badge.
//!
//! Lookup order: the icon name in the current theme (Yaru-magenta, then its parent Yaru),
//! then hicolor, then /usr/share/pixmaps. PNGs of 32 to 48 px are preferred; SVGs load through
//! egui_extras' svg loader. Names and misses are cached so the file system is hit once per name.

use egui::{Context, TextureId};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const THEMES: [&str; 3] = ["Yaru-magenta", "Yaru", "hicolor"];
const PNG_SIZES: [&str; 6] = ["32x32", "48x48", "24x24", "64x64", "22x22", "128x128"];

pub struct IconCache {
    /// Icon name to resolved `file://` URI (`None` = no icon found).
    resolved: HashMap<String, Option<String>>,
    /// Tone of each loaded icon, measured once from its pixels.
    tones: HashMap<String, Tone>,
    roots: Vec<PathBuf>,
}

/// How an icon sits against a row. Most app icons carry their own background, but a light or
/// dark glyph on transparency vanishes on a row of the same tone and needs a plate behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Its own background, or a mix of tones that reads on any row.
    Opaque,
    LightGlyph,
    DarkGlyph,
}

/// Classify an icon from its pixels: a glyph covers well under the image, a backed icon most of
/// it (a circle covers 79%).
pub fn tone_of(img: &egui::ColorImage) -> Tone {
    let (mut covered, mut lum) = (0usize, 0f32);
    for px in &img.pixels {
        if px.a() > 128 {
            let [r, g, b, _] = px.to_srgba_unmultiplied();
            lum += (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0;
            covered += 1;
        }
    }
    if covered == 0 || covered as f32 > 0.6 * img.pixels.len() as f32 {
        return Tone::Opaque;
    }
    match lum / covered as f32 {
        m if m > 0.8 => Tone::LightGlyph,
        m if m < 0.2 => Tone::DarkGlyph,
        _ => Tone::Opaque,
    }
}

fn user_icon_roots() -> Vec<PathBuf> {
    // `HWGLANCE_ICON_DIRS` (colon separated) replaces the search roots, so renders do not
    // depend on the installed icon themes (golden-image tests).
    if let Some(dirs) = std::env::var_os("HWGLANCE_ICON_DIRS") {
        return std::env::split_paths(&dirs).collect();
    }
    let mut v = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        v.push(Path::new(&home).join(".local/share/icons"));
    }
    v.push(PathBuf::from("/usr/share/icons"));
    v
}

/// Find an icon file for `name` (a theme icon name or an absolute path).
pub fn find_icon(roots: &[PathBuf], name: &str) -> Option<PathBuf> {
    if name.starts_with('/') {
        let p = PathBuf::from(name);
        let ok = p.extension().is_some_and(|e| e == "png" || e == "svg");
        return (ok && p.is_file()).then_some(p);
    }
    if name.is_empty() || name.contains('/') {
        return None;
    }
    for theme in THEMES {
        for root in roots {
            let base = root.join(theme);
            if !base.is_dir() {
                continue;
            }
            for size in PNG_SIZES {
                for cat in ["apps", "applications"] {
                    let p = base.join(size).join(cat).join(format!("{name}.png"));
                    if p.is_file() {
                        return Some(p);
                    }
                    let p = base.join(cat).join(size).join(format!("{name}.png"));
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
            for cat in ["apps", "applications"] {
                let p = base.join("scalable").join(cat).join(format!("{name}.svg"));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    for ext in ["png", "svg"] {
        let p = Path::new("/usr/share/pixmaps").join(format!("{name}.{ext}"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

impl IconCache {
    pub fn new() -> Self {
        IconCache { resolved: HashMap::new(), tones: HashMap::new(), roots: user_icon_roots() }
    }

    /// Texture and tone for `name` when the icon exists and has finished loading.
    pub fn get(&mut self, ctx: &Context, name: &str) -> Option<(TextureId, Tone)> {
        let roots = &self.roots;
        let uri = self
            .resolved
            .entry(name.to_string())
            .or_insert_with(|| find_icon(roots, name).map(|p| format!("file://{}", p.display())))
            .clone()?;
        let hint = egui::load::SizeHint::Size { width: 48, height: 48, maintain_aspect_ratio: true };
        match ctx.try_load_texture(&uri, egui::TextureOptions::LINEAR, hint) {
            Ok(egui::load::TexturePoll::Ready { texture }) => {
                let tone = match self.tones.get(name) {
                    Some(t) => *t,
                    None => match ctx.try_load_image(&uri, hint) {
                        Ok(egui::load::ImagePoll::Ready { image }) => {
                            let t = tone_of(&image);
                            self.tones.insert(name.to_string(), t);
                            t
                        }
                        _ => Tone::Opaque,
                    },
                };
                Some((texture.id, tone))
            }
            Ok(egui::load::TexturePoll::Pending { .. }) => None,
            Err(_) => {
                self.resolved.insert(name.to_string(), None);
                None
            }
        }
    }
}

impl Default for IconCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_and_bad_names() {
        let roots = user_icon_roots();
        assert!(find_icon(&roots, "").is_none());
        assert!(find_icon(&roots, "../etc/passwd").is_none());
        assert!(find_icon(&roots, "/etc/passwd").is_none());
        assert!(find_icon(&roots, "definitely-not-an-icon-name-xyz").is_none());
    }

    #[test]
    fn icon_tones() {
        use egui::{Color32, ColorImage};
        let img = |f: &dyn Fn(usize, usize) -> Color32| {
            let mut i = ColorImage::new([10, 10], vec![Color32::TRANSPARENT; 100]);
            for y in 0..10 {
                for x in 0..10 {
                    i.pixels[y * 10 + x] = f(x, y);
                }
            }
            i
        };
        let stroke = |x: usize, y: usize| (3..6).contains(&x) || y == 2;
        let white_glyph = img(&|x, y| if stroke(x, y) { Color32::WHITE } else { Color32::TRANSPARENT });
        let black_glyph = img(&|x, y| if stroke(x, y) { Color32::BLACK } else { Color32::TRANSPARENT });
        let backed = img(&|x, _| if x > 0 { Color32::from_rgb(220, 90, 40) } else { Color32::TRANSPARENT });
        assert_eq!(tone_of(&white_glyph), Tone::LightGlyph);
        assert_eq!(tone_of(&black_glyph), Tone::DarkGlyph);
        assert_eq!(tone_of(&backed), Tone::Opaque);
        assert_eq!(tone_of(&img(&|_, _| Color32::TRANSPARENT)), Tone::Opaque);
    }

    #[test]
    fn finds_a_theme_icon_when_installed() {
        if Path::new("/usr/share/icons/Yaru-magenta").is_dir() {
            assert!(find_icon(&user_icon_roots(), "utilities-system-monitor").is_some());
        }
    }
}
