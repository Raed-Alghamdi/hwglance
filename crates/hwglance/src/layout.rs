//! Pure layout logic for the sensor area: automatic and explicit column arrangement, and the
//! edit operations (move card, resize column, reorder sections and readings, reset).

use crate::settings::{ColumnPref, LayoutPrefs};
use std::collections::BTreeMap;

pub const MIN_COL_W: f32 = 480.0;
pub const MAX_COL_W: f32 = 560.0;
pub const COL_GAP: f32 = 12.0;
pub const NEW_COL_W: f32 = 480.0;
pub const MIN_EXPLICIT_W: f32 = 280.0;
pub const MAX_EXPLICIT_W: f32 = 1100.0;
/// Narrowest a card may be before its rows lose their range bar (at the default font size).
pub const MIN_FIT_W: f32 = 420.0;

/// One column of the arrangement: its width and the cards (ids) in it, top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub width: f32,
    pub cards: Vec<String>,
}

/// How many columns of roughly `min_w` to `max_w` fit into `avail`.
pub fn column_count(avail: f32, gap: f32, groups: usize, min_w: f32, max_w: f32) -> usize {
    let lo = ((avail + gap) / (max_w + gap)).ceil().max(1.0) as usize;
    let hi = ((avail + gap) / (min_w + gap)).floor().max(1.0) as usize;
    let n = if lo <= hi { lo } else { hi };
    n.clamp(1, groups.max(1))
}

/// Split `weights` into at most `n` contiguous chunks minimising the tallest chunk, so the
/// display order stays intact. Returns the item indices of each chunk.
pub fn partition(weights: &[usize], n: usize) -> Vec<Vec<usize>> {
    if weights.is_empty() {
        return vec![];
    }
    let n = n.clamp(1, weights.len());
    let fits = |cap: usize| -> bool {
        let (mut parts, mut cur) = (1, 0);
        for &w in weights {
            if w > cap {
                return false;
            }
            if cur + w > cap {
                parts += 1;
                cur = w;
            } else {
                cur += w;
            }
        }
        parts <= n
    };
    let (mut lo, mut hi) = (*weights.iter().max().unwrap(), weights.iter().sum::<usize>());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if fits(mid) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let cap = lo;
    // Fill greedily up to the cap, but never leave later chunks without items.
    let mut out: Vec<Vec<usize>> = vec![vec![]];
    let mut cur = 0;
    for (i, &w) in weights.iter().enumerate() {
        let remaining = weights.len() - i;
        let chunks_left = n - out.len();
        let must_split = remaining <= chunks_left && !out.last().unwrap().is_empty();
        if (cur + w > cap || must_split) && !out.last().unwrap().is_empty() && out.len() < n {
            out.push(vec![]);
            cur = 0;
        }
        out.last_mut().unwrap().push(i);
        cur += w;
    }
    out
}

/// Automatic arrangement of `cards` (id, weight) into balanced columns for `avail` pixels.
pub fn auto_arrangement(cards: &[(String, usize)], avail: f32, scale: f32) -> Vec<Column> {
    if cards.is_empty() {
        return vec![];
    }
    let n = column_count(avail, COL_GAP, cards.len(), MIN_COL_W * scale, MAX_COL_W * scale);
    let col_w = snap_down((avail - COL_GAP * (n as f32 - 1.0)) / n as f32).max(200.0);
    let weights: Vec<usize> = cards.iter().map(|c| c.1).collect();
    partition(&weights, n)
        .into_iter()
        .map(|idx| Column { width: col_w, cards: idx.into_iter().map(|i| cards[i].0.clone()).collect() })
        .collect()
}

/// The saved explicit columns, restricted to cards that exist (`present`, in display order).
/// Existing cards the layout does not mention (new sensors) join the last column. Unknown ids
/// stay in the preferences but are not shown. Always returns at least one column when there are cards.
pub fn explicit_arrangement(prefs: &LayoutPrefs, present: &[String]) -> Vec<Column> {
    let mut cols: Vec<Column> = prefs
        .columns
        .iter()
        .map(|c| Column {
            width: c.width.clamp(MIN_EXPLICIT_W, MAX_EXPLICIT_W),
            cards: c.cards.iter().filter(|id| present.contains(id)).cloned().collect(),
        })
        .collect();
    if cols.is_empty() {
        cols.push(Column { width: NEW_COL_W, cards: vec![] });
    }
    let placed: Vec<&String> = cols.iter().flat_map(|c| c.cards.iter()).collect();
    let missing: Vec<String> = present.iter().filter(|id| !placed.contains(id)).cloned().collect();
    cols.last_mut().unwrap().cards.extend(missing);
    cols
}

/// Round down to the 4 px layout unit.
pub fn snap_down(v: f32) -> f32 {
    (v / 4.0).floor() * 4.0
}

/// Most a layout made on a narrower screen grows to fill a wider window.
const MAX_GROW: f32 = 1.6;

/// Scale explicit columns proportionally to the window: shrink them when they (and the gaps)
/// are wider than `avail`, so a layout made on a wider screen never needs a horizontal
/// scrollbar, and grow them (up to `MAX_GROW`) when they leave the right side empty.
/// Returns the factor that was applied (1.0 when nothing changed).
pub fn fit_to_width(cols: &mut [Column], avail: f32) -> f32 {
    if cols.is_empty() {
        return 1.0;
    }
    let gaps = COL_GAP * (cols.len() as f32 - 1.0);
    let total: f32 = cols.iter().map(|c| c.width).sum();
    if total <= 0.0 || (total + gaps - avail).abs() < 4.0 {
        return 1.0;
    }
    let factor = ((avail - gaps) / total).clamp(0.0, MAX_GROW);
    for c in cols.iter_mut() {
        c.width = snap_down((c.width * factor).max(MIN_EXPLICIT_W.min(c.width)));
    }
    factor
}

/// The columns to draw. Automatic mode flows the cards into the width. An explicit layout keeps
/// the user's columns when they fit at `MIN_FIT_W` (shrunk to the window); when they do not,
/// neighbouring columns stack, whole and in order, into as many display columns as fit, so the
/// order never changes. While `edit` is on the real columns always show, one to one with
/// `prefs.columns`, and the area scrolls if they overflow.
pub fn arrangement(prefs: &LayoutPrefs, cards: &[(String, usize)], avail: f32, scale: f32, edit: bool) -> Vec<Column> {
    if prefs.auto {
        return auto_arrangement(cards, avail, scale);
    }
    let present: Vec<String> = cards.iter().map(|c| c.0.clone()).collect();
    let mut cols = explicit_arrangement(prefs, &present);
    if !edit {
        let used: Vec<Column> = cols.iter().filter(|c| !c.cards.is_empty()).cloned().collect();
        cols = if used.is_empty() { vec![Column { width: NEW_COL_W, cards: vec![] }] } else { used };
        let fit_w = MIN_FIT_W * scale + COL_GAP;
        if (avail + COL_GAP) < cols.len() as f32 * fit_w {
            let k = (((avail + COL_GAP) / fit_w).floor() as usize).max(1);
            cols = stack_columns(&cols, cards, k);
        }
    }
    fit_to_width(&mut cols, avail);
    cols
}

/// Stack `cols` into at most `k` display columns: contiguous groups of whole columns, balanced
/// by the weight of their cards. A group holds its cards top to bottom in column order and is
/// as wide as its widest column.
fn stack_columns(cols: &[Column], cards: &[(String, usize)], k: usize) -> Vec<Column> {
    let weight = |id: &String| cards.iter().find(|c| &c.0 == id).map_or(1, |c| c.1);
    let weights: Vec<usize> = cols.iter().map(|c| c.cards.iter().map(weight).sum()).collect();
    partition(&weights, k)
        .into_iter()
        .map(|group| Column {
            width: group.iter().map(|&i| cols[i].width).fold(0.0, f32::max),
            cards: group.iter().flat_map(|&i| cols[i].cards.iter().cloned()).collect(),
        })
        .collect()
}

/// Leave automatic mode, keeping the arrangement that is on screen right now. Cards that exist
/// in the old explicit layout but are currently hidden or absent keep their place at the end of
/// their previous column.
pub fn freeze(prefs: &mut LayoutPrefs, current: &[Column]) {
    if !prefs.auto {
        return;
    }
    prefs.auto = false;
    prefs.columns = current
        .iter()
        .map(|c| ColumnPref { width: c.width, cards: c.cards.clone() })
        .collect();
    if prefs.columns.is_empty() {
        prefs.columns.push(ColumnPref { width: NEW_COL_W, cards: vec![] });
    }
}

/// Make the saved columns list the cards as they are shown now, each column's shown cards in
/// order, then the ids it holds that are not shown (hidden or absent ones keep their column).
/// A layout that does not name every card shows the rest at the end of its last column, and a
/// move by position has to count them. `shown` is the arrangement with one column per saved one
/// (edit mode); nothing happens for an automatic layout or another number of columns.
pub fn sync_shown(prefs: &mut LayoutPrefs, shown: &[Column]) {
    if prefs.auto || prefs.columns.len() != shown.len() {
        return;
    }
    for (p, s) in prefs.columns.iter_mut().zip(shown) {
        let hidden: Vec<String> = p.cards.iter().filter(|id| !s.cards.contains(id)).cloned().collect();
        p.cards = s.cards.iter().cloned().chain(hidden).collect();
    }
}

fn remove_card(prefs: &mut LayoutPrefs, card: &str) {
    for c in &mut prefs.columns {
        c.cards.retain(|x| x != card);
    }
}

/// Move `card` to column `col` at position `index` (position after the card has left its old
/// place). Out-of-range values clamp. Needs explicit columns (call `freeze` first).
pub fn move_card(prefs: &mut LayoutPrefs, card: &str, col: usize, index: usize) {
    if prefs.columns.is_empty() {
        return;
    }
    remove_card(prefs, card);
    let col = col.min(prefs.columns.len() - 1);
    let list = &mut prefs.columns[col].cards;
    let index = index.min(list.len());
    list.insert(index, card.to_string());
}

pub fn resize_column(prefs: &mut LayoutPrefs, col: usize, width: f32) {
    if let Some(c) = prefs.columns.get_mut(col) {
        // Widths snap to the 4 px layout unit.
        c.width = ((width.clamp(MIN_EXPLICIT_W, MAX_EXPLICIT_W)) / 4.0).round() * 4.0;
    }
}

pub fn add_column(prefs: &mut LayoutPrefs) {
    prefs.columns.push(ColumnPref { width: NEW_COL_W, cards: vec![] });
}

/// Remove a column; its cards move to the end of the previous one (or the front of the next
/// when it was first). The last remaining column cannot be removed.
pub fn remove_column(prefs: &mut LayoutPrefs, col: usize) {
    if prefs.columns.len() <= 1 || col >= prefs.columns.len() {
        return;
    }
    let removed = prefs.columns.remove(col);
    if col == 0 {
        // The first column's cards stay in front of the next column's.
        let mut cards = removed.cards;
        cards.append(&mut prefs.columns[0].cards);
        prefs.columns[0].cards = cards;
    } else {
        prefs.columns[col - 1].cards.extend(removed.cards);
    }
}

/// Back to the automatic arrangement. Also forgets section and reading order.
pub fn reset(prefs: &mut LayoutPrefs) {
    prefs.auto = true;
    prefs.columns.clear();
    prefs.section_order.clear();
    prefs.reading_order.clear();
}

/// Put back what [`reset`] cleared, from the preferences as they were before it. Everything else
/// (collapsed panels, the dock and its sizes) stays as it is now.
pub fn restore(prefs: &mut LayoutPrefs, saved: LayoutPrefs) {
    prefs.auto = saved.auto;
    prefs.columns = saved.columns;
    prefs.section_order = saved.section_order;
    prefs.reading_order = saved.reading_order;
}

/// Apply a saved order to `default` ids: saved ids that still exist come first in the saved
/// order, then the rest in default order.
pub fn apply_order(default: &[String], saved: Option<&Vec<String>>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(default.len());
    if let Some(saved) = saved {
        for id in saved {
            if default.contains(id) && !out.contains(id) {
                out.push(id.clone());
            }
        }
    }
    for id in default {
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    out
}

/// Move `item` to `index` (position after it has left its old place) within the order stored
/// under `key`, starting from the current visible order `current`.
pub fn move_in_order(map: &mut BTreeMap<String, Vec<String>>, key: &str, current: &[String], item: &str, index: usize) {
    let mut order: Vec<String> = current.to_vec();
    if let Some(from) = order.iter().position(|x| x == item) {
        order.remove(from);
    } else {
        return;
    }
    let index = index.min(order.len());
    order.insert(index, item.to_string());
    map.insert(key.to_string(), order);
}

/// Insertion slot (0..=len) for a pointer at `y` among item mid-points, given as `(top, bottom)`.
pub fn slot_for_y(items: &[(f32, f32)], y: f32) -> usize {
    items.iter().take_while(|(top, bottom)| (top + bottom) / 2.0 < y).count()
}

/// Convert an insertion slot in the original list into the final index after removing the item
/// at `from`.
pub fn final_index(from: Option<usize>, slot: usize) -> usize {
    match from {
        Some(f) if slot > f => slot - 1,
        _ => slot,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cards(spec: &[(&str, usize)]) -> Vec<(String, usize)> {
        spec.iter().map(|(a, b)| (a.to_string(), *b)).collect()
    }

    fn explicit(cols: &[(f32, &[&str])]) -> LayoutPrefs {
        LayoutPrefs {
            auto: false,
            columns: cols
                .iter()
                .map(|(w, c)| ColumnPref { width: *w, cards: c.iter().map(|s| s.to_string()).collect() })
                .collect(),
            ..Default::default()
        }
    }

    fn ids(p: &LayoutPrefs) -> Vec<Vec<&str>> {
        p.columns.iter().map(|c| c.cards.iter().map(|s| s.as_str()).collect()).collect()
    }

    #[test]
    fn column_counts() {
        assert_eq!(column_count(2030.0, COL_GAP, 8, MIN_COL_W, MAX_COL_W), 4);
        assert_eq!(column_count(1180.0, COL_GAP, 8, MIN_COL_W, MAX_COL_W), 2);
        assert_eq!(column_count(500.0, COL_GAP, 8, MIN_COL_W, MAX_COL_W), 1);
        assert_eq!(column_count(2540.0, COL_GAP, 8, MIN_COL_W, MAX_COL_W), 5);
        assert_eq!(column_count(2540.0, COL_GAP, 2, MIN_COL_W, MAX_COL_W), 2);
    }

    #[test]
    fn partition_is_contiguous_and_balanced() {
        let w = [30, 28, 16, 9, 9, 9, 20, 6];
        let parts = partition(&w, 4);
        let flat: Vec<usize> = parts.iter().flatten().copied().collect();
        assert_eq!(flat, (0..w.len()).collect::<Vec<_>>());
        let tallest = parts.iter().map(|p| p.iter().map(|&i| w[i]).sum::<usize>()).max().unwrap();
        assert!(tallest <= 37, "{parts:?}");
        assert!(partition(&[], 3).is_empty());
        assert_eq!(partition(&[5, 5], 1), vec![vec![0, 1]]);
        assert_eq!(partition(&[100, 1, 1, 1], 2), vec![vec![0], vec![1, 2, 3]]);
    }

    #[test]
    fn auto_arrangement_keeps_order() {
        let c = cards(&[("cpu", 30), ("gpu", 28), ("mem", 16), ("d0", 9), ("d1", 9), ("mb", 20)]);
        let cols = auto_arrangement(&c, 2030.0, 1.0);
        assert_eq!(cols.len(), 4);
        let flat: Vec<&str> = cols.iter().flat_map(|c| c.cards.iter().map(|s| s.as_str())).collect();
        assert_eq!(flat, vec!["cpu", "gpu", "mem", "d0", "d1", "mb"]);
        assert!(cols.iter().all(|c| (480.0..=560.0).contains(&c.width) && c.width % 4.0 == 0.0), "{cols:?}");
    }

    #[test]
    fn freeze_then_move_card_between_columns() {
        let mut p = LayoutPrefs::default();
        let current = vec![
            Column { width: 480.0, cards: vec!["cpu".into(), "gpu".into()] },
            Column { width: 500.0, cards: vec!["mem".into()] },
        ];
        freeze(&mut p, &current);
        assert!(!p.auto);
        assert_eq!(ids(&p), vec![vec!["cpu", "gpu"], vec!["mem"]]);
        // freezing again changes nothing
        freeze(&mut p, &[]);
        assert_eq!(ids(&p), vec![vec!["cpu", "gpu"], vec!["mem"]]);

        move_card(&mut p, "cpu", 1, 0);
        assert_eq!(ids(&p), vec![vec!["gpu"], vec!["cpu", "mem"]]);
        move_card(&mut p, "gpu", 1, 99);
        assert_eq!(ids(&p), vec![Vec::<&str>::new(), vec!["cpu", "mem", "gpu"]]);
        // within the same column
        move_card(&mut p, "gpu", 1, 0);
        assert_eq!(ids(&p), vec![Vec::<&str>::new(), vec!["gpu", "cpu", "mem"]]);
    }

    #[test]
    fn resize_clamps_and_add_remove_columns() {
        let mut p = explicit(&[(480.0, &["a", "b"]), (480.0, &["c"]), (480.0, &["d"])]);
        resize_column(&mut p, 1, 640.0);
        assert_eq!(p.columns[1].width, 640.0);
        resize_column(&mut p, 1, 641.7);
        assert_eq!(p.columns[1].width, 642.0 - 2.0, "snaps to the 4 px unit");
        resize_column(&mut p, 1, 10.0);
        assert_eq!(p.columns[1].width, MIN_EXPLICIT_W);
        resize_column(&mut p, 1, 9999.0);
        assert_eq!(p.columns[1].width, MAX_EXPLICIT_W);
        resize_column(&mut p, 7, 500.0); // ignored

        add_column(&mut p);
        assert_eq!(p.columns.len(), 4);
        assert!(p.columns[3].cards.is_empty());
        remove_column(&mut p, 1);
        assert_eq!(ids(&p), vec![vec!["a", "b", "c"], vec!["d"], vec![]]);
        remove_column(&mut p, 0);
        assert_eq!(ids(&p), vec![vec!["a", "b", "c", "d"], vec![]]);
    }

    #[test]
    fn explicit_columns_shrink_to_the_window_instead_of_scrolling() {
        // The user's layout: four columns of 461 and one of 480 on a 2048 px window.
        let mut cols: Vec<Column> = [461.0, 461.0, 461.0, 461.0, 480.0]
            .iter()
            .map(|w| Column { width: *w, cards: vec!["a".into()] })
            .collect();
        let avail = 2030.0;
        let factor = fit_to_width(&mut cols, avail);
        assert!(factor < 1.0 && factor > 0.8, "{factor}");
        let total: f32 = cols.iter().map(|c| c.width).sum::<f32>() + COL_GAP * 4.0;
        assert!(total <= avail, "{total}");
        assert!(cols.iter().all(|c| c.width % 4.0 == 0.0 && c.width >= MIN_EXPLICIT_W));
        // proportions are kept: the wider column stays wider
        assert!(cols[4].width >= cols[0].width);
        // an exact fit: nothing changes
        let mut exact = vec![Column { width: 480.0, cards: vec![] }, Column { width: 500.0, cards: vec![] }];
        assert_eq!(fit_to_width(&mut exact, 480.0 + 500.0 + COL_GAP), 1.0);
        assert_eq!(exact[0].width, 480.0);
        // a wider window: columns grow proportionally to fill it
        let mut roomy = vec![Column { width: 480.0, cards: vec![] }, Column { width: 500.0, cards: vec![] }];
        let grow = fit_to_width(&mut roomy, 1500.0);
        assert!(grow > 1.0 && grow <= MAX_GROW, "{grow}");
        assert!(roomy[1].width > roomy[0].width && roomy[0].width > 480.0);
        assert!(roomy[0].width + roomy[1].width + COL_GAP <= 1500.0);
        // but never past MAX_GROW, however wide the window
        let mut tiny = vec![Column { width: 400.0, cards: vec![] }];
        fit_to_width(&mut tiny, 4000.0);
        assert_eq!(tiny[0].width, snap_down(400.0 * MAX_GROW));
        // the preferences themselves are not touched by fitting
        let p = explicit(&[(461.0, &["a"]), (461.0, &["b"])]);
        let arr = arrangement(&p, &cards(&[("a", 5), ("b", 5)]), 900.0, 1.0, false);
        assert!(arr.iter().map(|c| c.width).sum::<f32>() + COL_GAP <= 900.0);
        assert_eq!(p.columns[0].width, 461.0);
    }

    /// A typical arrangement: four columns of 496.
    fn four_columns() -> (LayoutPrefs, Vec<(String, usize)>) {
        let p = explicit(&[(496.0, &["cpu", "memory"]), (496.0, &["gpu0"]), (496.0, &["board", "igpu"]), (496.0, &["d0", "d1", "d2"])]);
        let c = cards(&[("cpu", 30), ("memory", 16), ("gpu0", 28), ("board", 20), ("igpu", 6), ("d0", 9), ("d1", 9), ("d2", 9)]);
        (p, c)
    }

    fn display_ids(cols: &[Column]) -> Vec<Vec<&str>> {
        cols.iter().map(|c| c.cards.iter().map(|s| s.as_str()).collect()).collect()
    }

    #[test]
    fn columns_that_fit_are_left_alone() {
        let (p, c) = four_columns();
        for edit in [false, true] {
            let cols = arrangement(&p, &c, 2030.0, 1.0, edit);
            assert_eq!(display_ids(&cols), ids(&p), "edit={edit}");
        }
        // exactly at the floor: four columns of MIN_FIT_W
        let floor = 4.0 * (MIN_FIT_W + COL_GAP) - COL_GAP;
        assert_eq!(arrangement(&p, &c, floor, 1.0, false).len(), 4);
        assert_eq!(arrangement(&p, &c, floor - 1.0, 1.0, false).len(), 3);
    }

    #[test]
    fn narrow_space_stacks_neighbouring_columns_in_order() {
        let (p, c) = four_columns();
        let all: Vec<&str> = ids(&p).into_iter().flatten().collect();
        let mut seen = std::collections::BTreeSet::new();
        for avail in [1900.0, 1500.0, 1255.0, 900.0, 600.0, 430.0, 300.0, 100.0] {
            let cols = arrangement(&p, &c, avail, 1.0, false);
            let fit = (((avail + COL_GAP) / (MIN_FIT_W + COL_GAP)).floor() as usize).clamp(1, 4);
            assert_eq!(cols.len(), fit, "{avail}: {:?}", display_ids(&cols));
            // every card once, in the user's order
            let flat: Vec<&str> = display_ids(&cols).into_iter().flatten().collect();
            assert_eq!(flat, all, "{avail}");
            assert!(cols.iter().all(|c| !c.cards.is_empty()));
            seen.insert(cols.len());
        }
        assert_eq!(seen.into_iter().collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        // The right expanded view: two columns, whole user columns kept together
        let two = arrangement(&p, &c, 1255.0, 1.0, false);
        assert_eq!(display_ids(&two), vec![vec!["cpu", "memory", "gpu0"], vec!["board", "igpu", "d0", "d1", "d2"]]);
        assert!(two.iter().map(|c| c.width).sum::<f32>() + COL_GAP <= 1255.0 + 0.5);
        // a very narrow area gives one column
        assert_eq!(arrangement(&p, &c, 100.0, 1.0, false).len(), 1);
        // a larger font needs more room per column
        assert_eq!(arrangement(&p, &c, 1900.0, 1.3, false).len(), 3);
    }

    #[test]
    fn stacked_columns_are_as_wide_as_their_widest_member() {
        let p = explicit(&[(500.0, &["a"]), (700.0, &["b"]), (500.0, &["c"])]);
        let c = cards(&[("a", 10), ("b", 10), ("c", 10)]);
        let cols = arrangement(&p, &c, 900.0, 1.0, false);
        assert_eq!(display_ids(&cols), vec![vec!["a", "b"], vec!["c"]]);
        // 700 and 500 scaled together to fill 900 px
        assert!(cols[0].width > cols[1].width);
        assert!(cols.iter().map(|c| c.width).sum::<f32>() + COL_GAP <= 900.0);
    }

    #[test]
    fn editing_always_shows_the_real_columns() {
        let (p, c) = four_columns();
        for avail in [2030.0, 1255.0, 600.0, 100.0] {
            let cols = arrangement(&p, &c, avail, 1.0, true);
            assert_eq!(display_ids(&cols), ids(&p), "{avail}");
        }
        // even empty ones, so the edit bar maps onto prefs.columns one to one
        let mut with_empty = p.clone();
        with_empty.columns.insert(1, ColumnPref { width: 480.0, cards: vec![] });
        assert_eq!(arrangement(&with_empty, &c, 1255.0, 1.0, true).len(), 5);
    }

    #[test]
    fn empty_user_columns_never_make_empty_display_columns() {
        let mut p = explicit(&[(480.0, &["a"]), (480.0, &[]), (480.0, &["gone"]), (480.0, &["b"])]);
        let c = cards(&[("a", 10), ("b", 10)]);
        for avail in [2000.0, 1000.0, 300.0] {
            let cols = arrangement(&p, &c, avail, 1.0, false);
            assert!(cols.iter().all(|c| !c.cards.is_empty()), "{avail}: {cols:?}");
            let flat: Vec<&str> = display_ids(&cols).into_iter().flatten().collect();
            assert_eq!(flat, vec!["a", "b"]);
        }
        // the two used columns fit side by side in 1000 px
        assert_eq!(arrangement(&p, &c, 1000.0, 1.0, false).len(), 2);
        // no cards at all: one empty column, as before
        assert_eq!(arrangement(&p, &[], 1000.0, 1.0, false).len(), 1);
        // preferences are never changed by arranging
        let before = p.clone();
        arrangement(&p, &c, 300.0, 1.0, false);
        assert_eq!(p, before);
        p.auto = true;
        assert!(!arrangement(&p, &c, 1000.0, 1.0, false).is_empty());
    }

    #[test]
    fn last_column_cannot_be_removed() {
        let mut p = explicit(&[(480.0, &["a"])]);
        remove_column(&mut p, 0);
        assert_eq!(p.columns.len(), 1);
    }

    #[test]
    fn explicit_arrangement_handles_new_and_missing_cards() {
        let p = explicit(&[(480.0, &["cpu", "gone"]), (520.0, &["gpu"])]);
        let present: Vec<String> = ["cpu", "gpu", "newcard"].iter().map(|s| s.to_string()).collect();
        let cols = explicit_arrangement(&p, &present);
        assert_eq!(cols[0].cards, vec!["cpu"]);
        assert_eq!(cols[1].cards, vec!["gpu", "newcard"]);
        assert_eq!(cols[1].width, 520.0);
        // unknown ids stay in the preferences
        assert!(p.columns[0].cards.contains(&"gone".to_string()));
        // no saved columns still yields one column with everything
        let empty = explicit(&[]);
        assert_eq!(explicit_arrangement(&empty, &present)[0].cards.len(), 3);
    }

    #[test]
    fn reset_returns_to_automatic() {
        let mut p = explicit(&[(480.0, &["a"])]);
        p.section_order.insert("a".into(), vec!["s2".into(), "s1".into()]);
        p.reading_order.insert("s1".into(), vec!["r2".into()]);
        p.collapsed_cards.push("a".into());
        reset(&mut p);
        assert!(p.auto && p.columns.is_empty());
        assert!(p.section_order.is_empty() && p.reading_order.is_empty());
        // collapsed cards are not part of the arrangement
        assert_eq!(p.collapsed_cards, vec!["a"]);
        let c = cards(&[("a", 10), ("b", 10)]);
        assert!(!arrangement(&p, &c, 1000.0, 1.0, false).is_empty());
    }

    #[test]
    fn reorder_sections_and_readings() {
        let current: Vec<String> = ["usage", "clock", "temp", "power"].iter().map(|s| s.to_string()).collect();
        let mut m = BTreeMap::new();
        move_in_order(&mut m, "cpu", &current, "temp", 0);
        assert_eq!(m["cpu"], vec!["temp", "usage", "clock", "power"]);
        // the saved order applies on top of the default, new ids append, gone ids are skipped
        let default: Vec<String> = ["usage", "clock", "temp", "power", "fans"].iter().map(|s| s.to_string()).collect();
        assert_eq!(apply_order(&default, m.get("cpu")), vec!["temp", "usage", "clock", "power", "fans"]);
        let shrunk: Vec<String> = ["usage", "temp"].iter().map(|s| s.to_string()).collect();
        assert_eq!(apply_order(&shrunk, m.get("cpu")), vec!["temp", "usage"]);
        // move down, and an unknown item is ignored
        let cur = apply_order(&default, m.get("cpu"));
        move_in_order(&mut m, "cpu", &cur, "temp", 3);
        assert_eq!(m["cpu"], vec!["usage", "clock", "power", "temp", "fans"]);
        let before = m.clone();
        move_in_order(&mut m, "cpu", &cur, "nope", 0);
        assert_eq!(m, before);
    }

    #[test]
    fn drop_slots() {
        let items = [(0.0, 10.0), (10.0, 20.0), (20.0, 30.0)];
        assert_eq!(slot_for_y(&items, -5.0), 0);
        assert_eq!(slot_for_y(&items, 4.0), 0);
        assert_eq!(slot_for_y(&items, 6.0), 1);
        assert_eq!(slot_for_y(&items, 26.0), 3);
        assert_eq!(slot_for_y(&items, 99.0), 3);
        // dropping item 0 into slot 3 ends at index 2; into slot 0 or 1 it stays at 0
        assert_eq!(final_index(Some(0), 3), 2);
        assert_eq!(final_index(Some(0), 1), 0);
        assert_eq!(final_index(Some(2), 0), 0);
        assert_eq!(final_index(None, 2), 2);
    }

    #[test]
    fn a_move_counts_the_cards_the_layout_does_not_name_and_keeps_the_hidden_ones() {
        let cols = |v: &[(&str, &[&str])]| -> Vec<ColumnPref> {
            v.iter().map(|(_, cs)| ColumnPref { width: 480.0, cards: cs.iter().map(|s| s.to_string()).collect() }).collect()
        };
        let mut prefs = LayoutPrefs { auto: false, columns: cols(&[("a", &["cpu", "gone"]), ("b", &["gpu0"])]), ..Default::default() };
        // on screen: new sensors (disk1, disk2) joined the last column; "gone" is not reported
        let shown = vec![
            Column { width: 480.0, cards: vec!["cpu".into()] },
            Column { width: 480.0, cards: vec!["gpu0".into(), "disk1".into(), "disk2".into()] },
        ];
        sync_shown(&mut prefs, &shown);
        assert_eq!(prefs.columns[0].cards, vec!["cpu", "gone"], "the unshown id stays in its column");
        assert_eq!(prefs.columns[1].cards, vec!["gpu0", "disk1", "disk2"]);
        // moving disk2 up one slot is now a move in the saved list too
        move_card(&mut prefs, "disk2", 1, 1);
        assert_eq!(prefs.columns[1].cards, vec!["gpu0", "disk2", "disk1"]);
        // an automatic layout, or a different number of columns, is left alone
        let mut auto = LayoutPrefs::default();
        sync_shown(&mut auto, &shown);
        assert!(auto.columns.is_empty());
        let mut one = LayoutPrefs { auto: false, columns: cols(&[("a", &["cpu"])]), ..Default::default() };
        sync_shown(&mut one, &shown);
        assert_eq!(one.columns[0].cards, vec!["cpu"]);
    }
}
