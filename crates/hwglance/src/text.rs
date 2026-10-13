//! Display order for text that comes from the system (window titles, process and group names).
//!
//! egui shapes text with HarfBuzz one run at a time (a run is a stretch of characters drawn by
//! the same font), so Arabic letters already join, and a run in Arabic script comes out in its
//! own right-to-left order. What egui does not do is bidirectional layout: the runs are placed
//! left to right in the order of the string, so the words of an Arabic title come out in reverse
//! and a Latin word inside Arabic text ends up on the wrong side.
//!
//! `shape` fixes that by reordering the runs into visual order (the Unicode bidi algorithm, via
//! `unicode-bidi`) while keeping the characters inside an Arabic run in reading order, since
//! egui reverses those itself. Neutral text drawn by the Latin font (spaces, dashes, brackets) in
//! a right-to-left stretch is reversed and its brackets are mirrored by hand. Text without any
//! right-to-left character is returned untouched, without a copy.
//!
//! The reshaping crates (`ar_reshaper` and friends) are deliberately not used: they turn letters
//! into presentation forms, which egui's shaper would then reverse a second time.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use unicode_bidi::BidiInfo;

/// Entries a cache keeps before it starts over (a process list is a few hundred strings).
const CACHE_CAP: usize = 2048;
const ELLIPSIS: char = '\u{2026}';
const LRM: char = '\u{200E}';
const RLM: char = '\u{200F}';

thread_local! {
    static SHAPED: RefCell<HashMap<String, Arc<str>>> = RefCell::new(HashMap::new());
    static FITTED: RefCell<HashMap<(String, u64), Arc<str>>> = RefCell::new(HashMap::new());
    static RUNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Does `c` belong to a right-to-left script (Arabic, Hebrew and their presentation forms)?
/// These are the characters only the Arabic fallback font draws, so each stretch of them is one
/// egui run.
fn is_rtl(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// Does `text` need reordering at all?
pub fn has_rtl(text: &str) -> bool {
    text.chars().any(is_rtl)
}

/// The mirrored form of a bracket, for characters drawn in a right-to-left stretch.
fn mirror(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '\u{AB}' => '\u{BB}',
        '\u{BB}' => '\u{AB}',
        '\u{2039}' => '\u{203A}',
        '\u{203A}' => '\u{2039}',
        c => c,
    }
}

/// Direction marks and joiners only steer the ordering; they are not painted.
fn is_control(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// A stretch of text that egui draws as one run and that has one bidi level.
struct Atom<'a> {
    text: &'a str,
    level: u8,
    /// Drawn by the Arabic font: egui puts its letters in right-to-left order itself.
    own_order: bool,
}

fn shape_uncached(text: &str) -> String {
    RUNS.with(|r| r.set(r.get() + 1));
    let info = BidiInfo::new(text, None);
    let mut out = String::with_capacity(text.len());
    for para in &info.paragraphs {
        // the line break that ends a paragraph stays at its end
        let body = text[para.range.clone()].trim_end_matches(['\n', '\r']);
        let body_end = para.range.start + body.len();
        let mut atoms: Vec<Atom> = Vec::new();
        let mut start = para.range.start;
        let mut prev: Option<(u8, bool)> = None;
        for (i, c) in body.char_indices().map(|(i, c)| (i + para.range.start, c)) {
            let key = (info.levels[i].number(), is_rtl(c));
            if prev.is_some_and(|p| p != key) {
                let (level, own_order) = prev.unwrap();
                atoms.push(Atom { text: &text[start..i], level, own_order });
                start = i;
            }
            prev = Some(key);
        }
        if let Some((level, own_order)) = prev {
            atoms.push(Atom { text: &text[start..body_end], level, own_order });
        }
        // rule L2: from the highest level down to the lowest odd one, reverse every stretch of
        // atoms at that level or above
        let (hi, lo_odd) = atoms.iter().fold((0, u8::MAX), |(h, l), a| (h.max(a.level), if a.level % 2 == 1 { l.min(a.level) } else { l }));
        for level in (lo_odd..=hi).rev() {
            let mut i = 0;
            while i < atoms.len() {
                if atoms[i].level < level {
                    i += 1;
                    continue;
                }
                let end = atoms[i..].iter().position(|a| a.level < level).map_or(atoms.len(), |n| i + n);
                atoms[i..end].reverse();
                i = end;
            }
        }
        for a in atoms {
            if a.level % 2 == 1 && !a.own_order {
                out.extend(a.text.chars().rev().map(mirror));
            } else {
                out.push_str(a.text);
            }
        }
        out.push_str(&text[body_end..para.range.end]);
    }
    out.retain(|c| !is_control(c));
    out
}

/// `text` as it is to be painted: in egui's visual order when it holds right-to-left script,
/// else the very same string. Results are cached per string.
pub fn shape(text: &str) -> Cow<'_, str> {
    if !has_rtl(text) {
        return Cow::Borrowed(text);
    }
    let shaped = SHAPED.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(s) = c.get(text) {
            return s.clone();
        }
        if c.len() >= CACHE_CAP {
            c.clear();
        }
        let s: Arc<str> = shape_uncached(text).into();
        c.insert(text.to_string(), s.clone());
        s
    });
    Cow::Owned(shaped.to_string())
}

/// `text` cut to what `fits` (a width test on the reordered string) accepts, with an ellipsis
/// where it was cut. The cut is made on the logical text, before reordering: the end of an Arabic
/// title is its visually leftmost part, so cutting the reordered string would drop the beginning.
fn fit_uncached(text: &str, fits: &dyn Fn(&str) -> bool) -> String {
    let whole = shape_uncached(text);
    if fits(&whole) {
        return whole;
    }
    let ends: Vec<usize> = text.char_indices().map(|(i, _)| i).skip(1).chain(std::iter::once(text.len())).collect();
    // Marks on both sides give the ellipsis the direction of what precedes it, so it sits next to
    // the cut.
    let candidate = |n: usize| -> String {
        let head = if n == 0 { "" } else { text[..ends[n - 1]].trim_end_matches(|c: char| c.is_whitespace() || "-\u{2013}\u{2014}\u{B7}|:,;/".contains(c)) };
        let last = head.chars().rev().find(|c| c.is_alphabetic());
        let mut s = head.to_string();
        match last {
            Some(c) if is_rtl(c) => s.extend([RLM, ELLIPSIS, RLM]),
            Some(_) => s.extend([LRM, ELLIPSIS, LRM]),
            None => s.push(ELLIPSIS),
        }
        shape_uncached(&s)
    };
    // the most characters whose shaped form with the ellipsis still fits
    let (mut lo, mut hi) = (0, ends.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(&candidate(mid)) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    candidate(lo)
}

/// Like `shape` but cut to what `fits` accepts, with an ellipsis. `key` names the width and font
/// the test measures with, so a result is reused until either changes. Text without
/// right-to-left script is returned as is: the painter's own truncation handles it.
pub fn shape_fitted<'a>(text: &'a str, key: u64, fits: &dyn Fn(&str) -> bool) -> Cow<'a, str> {
    if !has_rtl(text) {
        return Cow::Borrowed(text);
    }
    let fitted = FITTED.with(|c| {
        let mut c = c.borrow_mut();
        let k = (text.to_string(), key);
        if let Some(s) = c.get(&k) {
            return s.clone();
        }
        if c.len() >= CACHE_CAP {
            c.clear();
        }
        let s: Arc<str> = fit_uncached(text, fits).into();
        c.insert(k, s.clone());
        s
    });
    Cow::Owned(fitted.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "WhatsApp - chat with Ahmad"
    const TITLE: &str = "واتساب - محادثة مع أحمد";
    /// "WhatsApp"
    const WORD: &str = "واتساب";

    fn runs() -> usize {
        RUNS.with(|r| r.get())
    }

    #[test]
    fn a_known_title_comes_out_in_visual_order_with_each_word_in_reading_order() {
        // One right-to-left paragraph: the words are reversed. Inside a word egui reverses the
        // letters itself, so they stay in reading order here.
        assert_eq!(shape(TITLE), "أحمد مع محادثة - واتساب");
        assert_eq!(shape("سلام عليكم"), "عليكم سلام");
        assert_eq!(shape(WORD), WORD);
    }

    #[test]
    fn latin_text_and_numbers_are_returned_untouched() {
        for s in ["Inbox - Brave", "hwglance - Visual Studio Code", "btop 1.4.2 (3)", "", "ELDEN RING", "Ünïcode ✓ 日本語"] {
            assert!(matches!(shape(s), Cow::Borrowed(_)), "{s}");
            assert_eq!(shape(s), s);
        }
    }

    #[test]
    fn mixed_arabic_latin_and_digits_keep_each_run_readable() {
        // Latin first: a left-to-right paragraph. The digits after Arabic belong to its run, so
        // they sit to its left.
        assert_eq!(shape("Chrome - واتساب 2024"), format!("Chrome - 2024 {WORD}"));
        // Arabic first: a right-to-left paragraph. The Latin words and numbers keep their reading
        // order but come out to the left of the Arabic word.
        assert_eq!(shape("واتساب Chrome 2024"), format!("Chrome 2024 {WORD}"));
        assert_eq!(shape("سعر 5 دولار"), "دولار 5 سعر");
        // Latin before the Arabic is not touched
        assert_eq!(shape("Tab 12 of 30 - واتساب"), format!("Tab 12 of 30 - {WORD}"));
        // Arabic-Indic digits are numbers too: they keep their order inside the Arabic text
        assert_eq!(shape("سعر ٢٠٢٤ دولار"), "دولار ٢٠٢٤ سعر");
    }

    #[test]
    fn brackets_are_mirrored_in_right_to_left_stretches_only() {
        assert_eq!(shape(&format!("({WORD})")), format!("({WORD})"));
        assert_eq!(shape(&format!("{WORD} (Chrome)")), format!("(Chrome) {WORD}"));
        assert_eq!(shape(&format!("Chrome ({WORD})")), format!("Chrome ({WORD})"));
        assert_eq!(shape(&format!("[{WORD}] مع")), format!("مع [{WORD}]"));
        assert_eq!(mirror('('), ')');
        assert_eq!(mirror('a'), 'a');
    }

    #[test]
    fn several_lines_are_each_reordered_alone() {
        assert_eq!(shape("سلام عليكم\nChrome"), "عليكم سلام\nChrome");
    }

    #[test]
    fn shaping_is_cached_per_string() {
        let before = runs();
        let a = shape("سلام عليكم").into_owned();
        assert_eq!(runs(), before + 1);
        for _ in 0..50 {
            assert_eq!(shape("سلام عليكم"), a);
        }
        assert_eq!(runs(), before + 1, "repeats are answered from the cache");
        shape("مرحبا");
        assert_eq!(runs(), before + 2);
    }

    #[test]
    fn direction_marks_never_reach_the_painter() {
        let s = shape("\u{200F}واتساب\u{200E} Chrome\u{202B}");
        assert!(!s.chars().any(is_control), "{s:?}");
        assert!(s.contains("Chrome") && s.contains(WORD), "{s:?}");
    }

    /// Width test counting one unit per character.
    fn within(n: usize) -> impl Fn(&str) -> bool {
        move |s: &str| s.chars().count() <= n
    }

    #[test]
    fn a_fitting_string_is_not_cut_and_a_long_one_ends_in_an_ellipsis() {
        assert_eq!(fit_uncached(TITLE, &within(100)), shape(TITLE));
        assert_eq!(fit_uncached(TITLE, &within(23)), shape(TITLE));

        // The end of the title goes, so the ellipsis is the leftmost mark and the beginning of
        // the title survives at the right.
        let cut = fit_uncached(TITLE, &within(12));
        assert_eq!(cut, format!("{ELLIPSIS}مح - {WORD}"));
        assert_eq!(cut.chars().count(), 12);

        // a cut after the dash does not leave the dash or a space dangling before the ellipsis
        assert_eq!(fit_uncached(TITLE, &within(9)), format!("{ELLIPSIS}{WORD}"));
        // too little room for anything but the ellipsis
        assert_eq!(fit_uncached(TITLE, &within(1)), ELLIPSIS.to_string());
    }

    #[test]
    fn the_ellipsis_sits_next_to_the_cut_whichever_way_the_text_runs() {
        // an Arabic paragraph ending in Latin: the cut falls inside the Latin word
        assert_eq!(fit_uncached("واتساب Chrome browser", &within(12)), format!("Chro{ELLIPSIS} {WORD}"));
        // a left-to-right paragraph ending in Arabic: the cut falls inside the Arabic run, whose
        // end is on its left
        assert_eq!(fit_uncached("Chrome - واتساب", &within(13)), format!("Chrome - {ELLIPSIS}وات"));
    }

    #[test]
    fn fitted_strings_are_cached_per_width_key() {
        let before = runs();
        let calls = std::cell::Cell::new(0);
        let fits = |s: &str| {
            calls.set(calls.get() + 1);
            s.chars().count() <= 10
        };
        let a = shape_fitted(TITLE, 7, &fits).into_owned();
        let after_first = runs();
        assert!(after_first > before);
        let used = calls.get();
        assert_eq!(shape_fitted(TITLE, 7, &fits), a);
        assert_eq!(runs(), after_first, "same key: no new shaping");
        assert_eq!(calls.get(), used, "same key: no new measuring");
        shape_fitted(TITLE, 8, &fits);
        assert!(calls.get() > used, "a different key measures again");
        assert!(matches!(shape_fitted("Inbox - Brave", 7, &fits), Cow::Borrowed(_)), "Latin is left to the painter");
    }
}
