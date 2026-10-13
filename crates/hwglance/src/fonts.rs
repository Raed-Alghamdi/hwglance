//! Fonts: Atkinson Hyperlegible for words and its mono Nerd Font for numbers and icons, both
//! embedded. A setting switches everything to Omarchy's current font instead. A system font with
//! Arabic glyphs, when one is installed, is the last fallback of every family (it is read from
//! disk at startup, not embedded).

use egui::{Context, FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

pub const BOLD: &str = "bold";
pub const MONO_BOLD: &str = "mono-bold";

static ATKINSON: &[u8] = include_bytes!("../assets/fonts/AtkinsonHyperlegible-Regular.otf");
static ATKINSON_BOLD: &[u8] = include_bytes!("../assets/fonts/AtkinsonHyperlegible-Bold.otf");
static ATKYNSON_MONO: &[u8] = include_bytes!("../assets/fonts/AtkynsonMonoNerdFontMono-Regular.otf");
static ATKYNSON_MONO_BOLD: &[u8] = include_bytes!("../assets/fonts/AtkynsonMonoNerdFontMono-Bold.otf");

pub fn bold() -> FontFamily {
    FontFamily::Name(BOLD.into())
}

pub fn mono_bold() -> FontFamily {
    FontFamily::Name(MONO_BOLD.into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FontChoice {
    #[default]
    Atkinson,
    Omarchy,
}

impl FontChoice {
    pub fn parse(s: &str) -> FontChoice {
        if s.eq_ignore_ascii_case("omarchy") { FontChoice::Omarchy } else { FontChoice::Atkinson }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FontChoice::Atkinson => "atkinson",
            FontChoice::Omarchy => "omarchy",
        }
    }
}

fn command_output(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// The font family Omarchy uses (`omarchy-font-current`), if it can be asked.
pub fn omarchy_family() -> Option<String> {
    // A launcher or key binding can start us with a minimal PATH, so also try Omarchy's install path.
    command_output("omarchy-font-current", &[])
        .or_else(|| command_output("/usr/share/omarchy/bin/omarchy-font-current", &[]))
}

fn fc_file(pattern: &str) -> Option<Vec<u8>> {
    let path = command_output("fc-match", &["-f", "%{file}", pattern])?;
    std::fs::read(path).ok()
}

/// Regular and bold bytes of the Omarchy font, falling back to fontconfig's monospace.
fn load_omarchy_font() -> Option<(Vec<u8>, Option<Vec<u8>>)> {
    if let Some(family) = omarchy_family() {
        if let Some(regular) = fc_file(&format!("{family}:style=Regular")) {
            return Some((regular, fc_file(&format!("{family}:style=Bold"))));
        }
    }
    fc_file("monospace").map(|r| (r, fc_file("monospace:bold")))
}

/// Regular and bold files of the Arabic fallback font.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArabicFont {
    pub regular: String,
    pub bold: Option<String>,
}

/// Pick the Arabic font out of `fc-list -f '%{family[0]}|%{style[0]}|%{file}\n' :lang=ar`: Noto
/// Sans Arabic, else Noto Naskh Arabic, else any other family that has Arabic. Collections
/// (`.ttc`) are skipped: egui would need the index of the face.
fn pick_arabic(list: &str) -> Option<ArabicFont> {
    let mut faces: Vec<(&str, &str, &str)> = Vec::new();
    for line in list.lines() {
        let mut parts = line.splitn(3, '|');
        let (Some(family), Some(style), Some(file)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let lower = file.to_ascii_lowercase();
        if lower.ends_with(".ttf") || lower.ends_with(".otf") {
            faces.push((family.trim(), style.trim(), file.trim()));
        }
    }
    let rank = |family: &str| match family {
        "Noto Sans Arabic" => 0,
        "Noto Naskh Arabic" => 1,
        f if f.contains("Arabic") && !f.contains("UI") => 2,
        _ => 3,
    };
    let regular = faces.iter().filter(|f| f.1.starts_with("Regular")).min_by_key(|f| (rank(f.0), f.2))?;
    let bold = faces.iter().find(|f| f.0 == regular.0 && f.1.starts_with("Bold")).map(|f| f.2.to_string());
    Some(ArabicFont { regular: regular.2.to_string(), bold })
}

/// The installed Arabic font, looked up once (fontconfig is asked a single time per run).
pub fn arabic_font() -> Option<&'static ArabicFont> {
    static FOUND: std::sync::OnceLock<Option<ArabicFont>> = std::sync::OnceLock::new();
    FOUND.get_or_init(|| pick_arabic(&command_output("fc-list", &["-f", "%{family[0]}|%{style[0]}|%{file}\\n", ":lang=ar"])?)).as_ref()
}

pub fn install(ctx: &Context, choice: FontChoice) {
    let mut fonts = FontDefinitions::default();
    let defaults_prop = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let defaults_mono = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let embed = |fonts: &mut FontDefinitions, key: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(key.into(), Arc::new(FontData::from_static(bytes)));
    };
    embed(&mut fonts, "atkinson", ATKINSON);
    embed(&mut fonts, "atkinson-bold", ATKINSON_BOLD);
    embed(&mut fonts, "atkynson-mono", ATKYNSON_MONO);
    embed(&mut fonts, "atkynson-mono-bold", ATKYNSON_MONO_BOLD);

    // (words regular, words bold, numbers regular, numbers bold)
    let (mut words, mut words_bold, mut nums, mut nums_bold) = (
        vec!["atkinson".to_string()],
        vec!["atkinson-bold".to_string(), "atkinson".to_string()],
        vec!["atkynson-mono".to_string()],
        vec!["atkynson-mono-bold".to_string(), "atkynson-mono".to_string()],
    );
    if choice == FontChoice::Omarchy {
        if let Some((regular, bold_bytes)) = load_omarchy_font() {
            fonts.font_data.insert("omarchy".into(), Arc::new(FontData::from_owned(regular)));
            let mut chain_bold = vec!["omarchy".to_string()];
            if let Some(b) = bold_bytes {
                fonts.font_data.insert("omarchy-bold".into(), Arc::new(FontData::from_owned(b)));
                chain_bold.insert(0, "omarchy-bold".into());
            }
            words.insert(0, "omarchy".into());
            nums.insert(0, "omarchy".into());
            words_bold.splice(0..0, chain_bold.clone());
            nums_bold.splice(0..0, chain_bold);
        }
    }
    // The mono Nerd Font stays in every chain as the source of icon glyphs.
    for chain in [&mut words, &mut words_bold] {
        chain.push("atkynson-mono".into());
        chain.extend(defaults_prop.iter().cloned());
    }
    for chain in [&mut nums, &mut nums_bold] {
        chain.extend(defaults_mono.iter().cloned());
    }
    // Arabic comes last: only text that nothing above can draw reaches it.
    if let Some(arabic) = arabic_font()
        && let Ok(regular) = std::fs::read(&arabic.regular)
    {
        fonts.font_data.insert("arabic".into(), Arc::new(FontData::from_owned(regular)));
        if let Some(b) = arabic.bold.as_ref().and_then(|b| std::fs::read(b).ok()) {
            fonts.font_data.insert("arabic-bold".into(), Arc::new(FontData::from_owned(b)));
            words_bold.push("arabic-bold".into());
            nums_bold.push("arabic-bold".into());
        }
        for chain in [&mut words, &mut words_bold, &mut nums, &mut nums_bold] {
            chain.push("arabic".into());
        }
    }
    fonts.families.insert(FontFamily::Proportional, words);
    fonts.families.insert(FontFamily::Monospace, nums);
    fonts.families.insert(bold(), words_bold);
    fonts.families.insert(mono_bold(), nums_bold);
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_round_trip() {
        assert_eq!(FontChoice::parse("omarchy"), FontChoice::Omarchy);
        assert_eq!(FontChoice::parse("atkinson"), FontChoice::Atkinson);
        assert_eq!(FontChoice::parse("whatever"), FontChoice::Atkinson);
        assert_eq!(FontChoice::parse(FontChoice::Omarchy.as_str()), FontChoice::Omarchy);
    }

    const FC_LIST: &str = "Noto Kufi Arabic|Regular|/usr/share/fonts/noto/NotoKufiArabic-Regular.ttf
Noto Naskh Arabic|Regular|/usr/share/fonts/noto/NotoNaskhArabic-Regular.ttf
Noto Sans Arabic UI|Regular|/usr/share/fonts/noto/NotoSansArabicUI-Regular.ttf
Noto Sans Arabic|Bold|/usr/share/fonts/noto/NotoSansArabic-Bold.ttf
Noto Sans Arabic|Regular|/usr/share/fonts/noto/NotoSansArabic-Regular.ttf
Noto Sans Arabic|Thin|/usr/share/fonts/noto/NotoSansArabic-Thin.ttf
";

    #[test]
    fn noto_sans_arabic_is_preferred_then_naskh_then_anything() {
        let found = pick_arabic(FC_LIST).unwrap();
        assert_eq!(found.regular, "/usr/share/fonts/noto/NotoSansArabic-Regular.ttf");
        assert_eq!(found.bold.as_deref(), Some("/usr/share/fonts/noto/NotoSansArabic-Bold.ttf"));
        let naskh: String = FC_LIST.lines().filter(|l| !l.starts_with("Noto Sans Arabic|")).map(|l| format!("{l}\n")).collect();
        let found = pick_arabic(&naskh).unwrap();
        assert!(found.regular.ends_with("NotoNaskhArabic-Regular.ttf") && found.bold.is_none());
        let other = "DejaVu Sans|Regular|/usr/share/fonts/dejavu/DejaVuSans.ttf\nDejaVu Sans|Bold|/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf\nUnknown|Regular|/x/y.ttc\n";
        let found = pick_arabic(other).unwrap();
        assert!(found.regular.ends_with("DejaVuSans.ttf") && found.bold.is_some());
    }

    #[test]
    fn without_an_arabic_font_nothing_is_picked() {
        assert_eq!(pick_arabic(""), None);
        assert_eq!(pick_arabic("garbage line\nCollection|Regular|/x/y.ttc\n"), None);
    }

    #[test]
    fn the_embedded_fonts_are_present() {
        assert!(ATKINSON.len() > 10_000 && ATKINSON_BOLD.len() > 10_000);
        assert!(ATKYNSON_MONO.len() > 100_000 && ATKYNSON_MONO_BOLD.len() > 100_000);
    }
}
