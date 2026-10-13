//! Search text matching for the process list, aware of Arabic spelling.
//!
//! Window titles and app names come in Arabic as often as in English, and the same word is typed
//! in several ways: the alef with or without a hamza or madda, a final teh marbuta or heh, a final
//! yeh or alef maksura, tatweel stretching a word, harakat that the title has and the query does
//! not, and the article ال that a title may carry and a query may leave off. `Query` folds both
//! sides the same way, so none of those differences hides a process.
//!
//! The folding (applied to the query and to every searched text alike):
//! - the alef forms أ إ آ ٱ become ا, ة becomes ه and ى becomes ي;
//! - tatweel (ـ) and the diacritics (harakat, shadda, sukun, the superscript alef and the Quranic
//!   marks) are dropped;
//! - Arabic-Indic and Persian digits become 0 to 9;
//! - everything else is lower-cased.
//!
//! A word that starts with ال (and keeps at least two letters after it) also matches without it,
//! on both sides: "المفكرة" finds "مفكرة" and "مفكرة" finds "المفكرة". Text without any non-ASCII
//! character takes a plain lower-case path, as before.

use std::borrow::Cow;

/// Is `c` a mark that the folding drops (tatweel, harakat and the other Arabic combining marks)?
fn is_dropped(c: char) -> bool {
    matches!(c as u32,
        0x0640 // tatweel
        | 0x064B..=0x065F // fathatan to the extra marks: harakat, shadda, sukun, maddah, hamza marks
        | 0x0670 // superscript alef
        | 0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x06E7 | 0x06E8 | 0x06EA..=0x06ED // Quranic marks
    )
}

/// One character of folded text (a lower-cased letter can be more than one character).
fn fold_char(c: char, out: &mut String) {
    match c {
        '\u{0623}' | '\u{0625}' | '\u{0622}' | '\u{0671}' => out.push('\u{0627}'),
        '\u{0629}' => out.push('\u{0647}'),
        '\u{0649}' => out.push('\u{064A}'),
        '\u{0660}'..='\u{0669}' => out.push((b'0' + (c as u32 - 0x0660) as u8) as char),
        '\u{06F0}'..='\u{06F9}' => out.push((b'0' + (c as u32 - 0x06F0) as u8) as char),
        c if is_dropped(c) => {}
        c if c.is_ascii() => out.push(c.to_ascii_lowercase()),
        c => out.extend(c.to_lowercase()),
    }
}

/// `text` folded for matching (see the module docs). The article is not touched here.
pub fn fold(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        fold_char(c, &mut out);
    }
    out
}

const ARTICLE: &str = "\u{0627}\u{0644}";

/// Folded text with the article taken off every word that has one and keeps two letters after
/// it. Borrowed when there is nothing to take off.
pub fn without_article(folded: &str) -> Cow<'_, str> {
    let strippable = |w: &str| w.strip_prefix(ARTICLE).is_some_and(|rest| rest.chars().count() >= 2);
    if !folded.split_whitespace().any(strippable) {
        return Cow::Borrowed(folded);
    }
    let mut out = String::with_capacity(folded.len());
    let mut first = true;
    for w in folded.split_whitespace() {
        if !first {
            out.push(' ');
        }
        first = false;
        out.push_str(if strippable(w) { &w[ARTICLE.len()..] } else { w });
    }
    Cow::Owned(out)
}

/// A search query, folded once and matched against many texts.
#[derive(Clone, Debug, Default)]
pub struct Query {
    /// The folded query.
    plain: String,
    /// The same without the article on its words.
    bare: String,
}

impl Query {
    pub fn new(raw: &str) -> Query {
        let plain = fold(raw.trim());
        let bare = without_article(&plain).into_owned();
        Query { plain, bare }
    }

    /// Nothing to search for: everything matches.
    pub fn is_empty(&self) -> bool {
        self.plain.is_empty()
    }

    /// The folded query text (digits included), for matching numbers such as a pid.
    pub fn plain(&self) -> &str {
        &self.plain
    }

    /// Does `text` contain the query, spelled either way?
    pub fn matches(&self, text: &str) -> bool {
        if self.plain.is_empty() {
            return true;
        }
        let folded = fold(text);
        if folded.contains(&self.plain) {
            return true;
        }
        // the article only exists in non-ASCII text, so plain English never pays for the second look
        if folded.is_ascii() && self.plain.is_ascii() {
            return false;
        }
        let bare_text = without_article(&folded);
        bare_text.contains(&self.bare)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(q: &str, text: &str) -> bool {
        Query::new(q).matches(text)
    }

    #[test]
    fn english_stays_a_case_insensitive_substring() {
        assert!(hit("CHROME", "google-chrome-stable"));
        assert!(hit("  chr ", "Chrome"));
        assert!(!hit("firefox", "Chrome"));
        assert!(hit("", "anything"));
        assert!(hit("   ", "anything"));
    }

    #[test]
    fn every_alef_form_matches_every_other() {
        for q in ["أحمد", "إحمد", "آحمد", "احمد", "ٱحمد"] {
            for t in ["أحمد", "إحمد", "آحمد", "احمد", "ٱحمد"] {
                assert!(hit(q, t), "{q} should find {t}");
            }
        }
        // inside a word as well
        assert!(hit("مسأله", "مسألة"));
        assert!(hit("قرآن", "قران"));
    }

    #[test]
    fn teh_marbuta_is_heh_and_alef_maksura_is_yeh() {
        assert!(hit("مفكرة", "مفكره"));
        assert!(hit("مفكره", "مفكرة"));
        assert!(hit("مستشفى", "مستشفي"));
        assert!(hit("مستشفي", "مستشفى"));
        assert!(hit("على", "علي"));
    }

    #[test]
    fn tatweel_and_diacritics_are_ignored_on_both_sides() {
        assert!(hit("محمد", "مُحَمَّد"));
        assert!(hit("مُحَمَّد", "محمد"));
        assert!(hit("مـحـمـد", "محمد"));
        assert!(hit("محمد", "مـــحـمّـد"));
        assert!(hit("كتاب", "كِتَابٌ"));
        // the superscript alef of الله / الرحمن written with it
        assert!(hit("الرحمن", "الرحمٰن"));
    }

    #[test]
    fn the_article_is_optional_on_either_side() {
        assert!(hit("المفكرة", "مفكرة"));
        assert!(hit("مفكرة", "المفكرة"));
        assert!(hit("المفكرة", "المفكرة"));
        assert!(hit("مفكرة", "تطبيق المفكرة - Chrome"));
        assert!(hit("المفكرة", "Chrome - مفكرة"));
        // across words
        assert!(hit("برنامج مفكرة", "برنامج المفكرة"));
        assert!(hit("برنامج المفكرة", "برنامج مفكرة"));
        assert!(hit("البرنامج المفكرة", "برنامج مفكرة"));
    }

    #[test]
    fn the_article_works_with_the_other_folding_together() {
        assert!(hit("الأخبار", "اخبار"));
        assert!(hit("اخبار", "الأخبار"));
        assert!(hit("الإنترنت", "انترنت"));
        assert!(hit("المَكتَبة", "مكتبه"));
        assert!(hit("الـمـكتبة", "مكتبة"));
    }

    #[test]
    fn a_bare_article_or_a_short_word_is_not_stripped_to_nothing() {
        // "ال" alone and "الم" keep what they have: they do not turn into a search for everything
        assert!(!hit("ال", "كتب"));
        assert!(!hit("الم", "كتب"));
        assert!(hit("ال", "الكتاب"));
        assert_eq!(without_article("ال"), "ال");
        assert_eq!(without_article("الم"), "الم");
        assert_eq!(without_article("الكتاب"), "كتاب");
    }

    #[test]
    fn different_words_still_do_not_match() {
        assert!(!hit("مفكرة", "متصفح"));
        assert!(!hit("كتاب", "كتب"));
        assert!(!hit("المفكرة", "متصفح"));
    }

    #[test]
    fn arabic_and_persian_digits_find_ascii_digits() {
        assert!(hit("١٢٣", "pid 123"));
        assert!(hit("123", "المهمة ١٢٣"));
        assert!(hit("۱۲۳", "123"));
    }

    #[test]
    fn mixed_scripts_search_each_part() {
        assert!(hit("chrome مفكرة", "Chrome المفكرة"));
        assert!(hit("CHROME - المفكره", "chrome - مفكرة"));
    }

    #[test]
    fn folding_is_idempotent() {
        for s in ["المُفَكِّرَة", "أإآٱ", "ـــ", "Chrome", "مستشفى", ""] {
            let once = fold(s);
            assert_eq!(fold(&once), once, "{s}");
        }
    }

    #[test]
    fn ascii_text_borrows_and_folds_without_the_unicode_path() {
        assert_eq!(fold("Hello"), "hello");
        assert!(matches!(without_article("hello world"), Cow::Borrowed(_)));
    }
}
