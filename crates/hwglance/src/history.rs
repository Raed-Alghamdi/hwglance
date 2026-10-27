//! The history popover: the pure view model of a series (domain, extremes, onset of the last
//! warning or critical crossing, flag bands) and its painting.

use crate::rangebar;
use crate::theme::{Level, Palette};
use crate::thresholds::{level_of, Thr};
use crate::ui_kit::{f_label_bold, f_minmax, f_result, f_unit, fmt_clock, split_value, RowUnit};
use egui::{pos2, vec2, Align2, Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use hwglance_proto::{format_value, Crossing, CrossingLevel, HistorySeries, Unit};

pub const POP_W: f32 = 380.0;
pub const POP_H: f32 = 170.0;

#[derive(Clone, Debug, PartialEq)]
pub struct HistView {
    pub t0: u64,
    pub t1: u64,
    pub y0: f64,
    pub y1: f64,
    pub min: Option<(u64, f64)>,
    pub max: Option<(u64, f64)>,
    /// Where the value last crossed into a higher level (warning, then critical), with the level.
    pub onset: Option<(u64, Level)>,
    /// Intervals during which a flag was on.
    pub bands: Vec<(u64, u64)>,
}

fn rank(l: Level) -> u8 {
    match l {
        Level::Normal => 0,
        Level::Warn => 1,
        Level::Crit => 2,
    }
}

/// The points of `series` between `t0` and `t1` (a few seconds of slack after it), in time order.
/// The collector sends them in order; a reply that is not (an older collector across a clock
/// step) must not draw a zigzag back over the plot, and a point stamped after now is not shown.
pub fn points_in(series: &HistorySeries, t0: u64, t1: u64) -> Vec<(u64, f64)> {
    let mut pts: Vec<(u64, f64)> = series.points.iter().filter(|p| p.0 >= t0 && p.0 <= t1.saturating_add(5000)).map(|&(t, v)| (t, v as f64)).collect();
    pts.sort_by_key(|p| p.0);
    pts
}

/// Build the view of `series` over the window ending at `now_ms`.
pub fn build_view(series: &HistorySeries, now_ms: u64, window_ms: u64, thr: &Thr) -> Option<HistView> {
    let t1 = now_ms;
    let t0 = now_ms.saturating_sub(window_ms);
    let pts = points_in(series, t0, t1);
    if pts.is_empty() {
        return None;
    }
    let mut min = pts[0];
    let mut max = pts[0];
    for &p in &pts {
        if p.1 < min.1 {
            min = p;
        }
        if p.1 > max.1 {
            max = p;
        }
    }
    let (y0, y1) = if series.unit == Unit::Flag { (0.0, 1.0) } else { history_domain(series.unit, min.1, max.1, thr) };

    let mut onset = None;
    let mut prev = Level::Normal;
    for &(t, v) in &pts {
        let l = level_of(v, thr);
        if rank(l) > rank(prev) {
            onset = Some((t, l));
        }
        prev = l;
    }

    let mut bands = Vec::new();
    if series.unit == Unit::Flag {
        let mut start: Option<u64> = None;
        for (i, &(t, v)) in pts.iter().enumerate() {
            if v >= 0.5 && start.is_none() {
                start = Some(t);
            }
            if v < 0.5 {
                if let Some(s) = start.take() {
                    bands.push((s, t));
                }
            }
            if i == pts.len() - 1 {
                if let Some(s) = start.take() {
                    bands.push((s, t.max(s + 1000)));
                }
            }
        }
    }
    Some(HistView {
        t0,
        t1,
        y0,
        y1,
        min: Some((min.0, min.1)),
        max: Some((max.0, max.1)),
        onset,
        bands,
    })
}

/// Where a threshold line's value label sits: at the right end of the plot, the upper line's
/// above its line and the lower line's below, so two close thresholds do not overprint.
pub fn threshold_label_anchor(plot: Rect, y: f32, upper: bool) -> (egui::Pos2, Align2) {
    if upper {
        (pos2(plot.right() - 3.0, y - 1.0), Align2::RIGHT_BOTTOM)
    } else {
        (pos2(plot.right() - 3.0, y + 1.0), Align2::RIGHT_TOP)
    }
}

/// The popover body split into its parts: the plot is inset between a left gutter (y labels)
/// and a bottom gutter (x labels), so labels and plot never share pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopLayout {
    pub plot: Rect,
    /// The column left of the plot, as tall as the plot.
    pub left: Rect,
    /// The row under the plot, as wide as the plot.
    pub bottom: Rect,
}

pub const BOTTOM_GUTTER: f32 = 18.0;

/// Split `body` (everything under the title row) into plot and gutters.
pub fn layout(body: Rect, left_gutter: f32) -> PopLayout {
    let plot = Rect::from_min_max(
        pos2(body.left() + left_gutter, body.top()),
        pos2(body.right(), (body.bottom() - BOTTOM_GUTTER).max(body.top() + 20.0)),
    );
    PopLayout {
        plot,
        left: Rect::from_min_max(body.left_top(), pos2(plot.left(), plot.bottom())),
        bottom: Rect::from_min_max(pos2(plot.left(), plot.bottom()), pos2(plot.right(), body.bottom())),
    }
}

/// The smallest span a plotted domain may have, so a near-flat series is not glued to an axis:
/// percent 10 points, temperatures 5 degrees, anything else a tenth of its value.
pub fn min_span(unit: Unit, largest: f64) -> f64 {
    match unit {
        Unit::Percent => 10.0,
        Unit::Celsius => 5.0,
        _ => (largest.abs() * 0.1).max(1e-6),
    }
}

/// The smallest "round" number (1, 1.5, 2, 2.5, 3, 4, 5, 6, 8 times a power of ten) at or above `v`.
pub fn nice_up(v: f64) -> f64 {
    if v <= 0.0 {
        return 1.0;
    }
    let exp = 10f64.powf(v.log10().floor());
    let m = v / exp;
    let step = [1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0].into_iter().find(|s| *s >= m - 1e-9).unwrap_or(10.0);
    step * exp
}

/// The plotted value range of a series: the observed range and the thresholds, at least
/// `min_span` wide, with a round top. Percent starts at 0 and never passes 100 unless the data does.
pub fn history_domain(unit: Unit, obs_min: f64, obs_max: f64, thr: &Thr) -> (f64, f64) {
    let (lo, mut hi) = match unit {
        Unit::Percent => {
            let top = [Some(obs_max), thr.warn, thr.crit].into_iter().flatten().fold(obs_max, f64::max);
            (0.0, top * 1.02)
        }
        _ => rangebar::domain(unit, obs_min, obs_max, thr),
    };
    hi = hi.max(lo + min_span(unit, obs_max));
    hi = match unit {
        Unit::Celsius => (hi / 5.0).ceil() * 5.0,
        Unit::Percent => nice_up(hi).min(100.0f64.max(obs_max)).max(lo + min_span(unit, obs_max)),
        _ => nice_up(hi),
    };
    (lo, hi)
}

/// An axis value, compact: in the unit the top of the axis reads in, without the unit and
/// without trailing zeros ("0", "20", "1.35").
pub fn axis_text(unit: Unit, v: f64, top: f64) -> String {
    let s = RowUnit::new(unit, top).number(v);
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

/// "-30 min", "-2 h"
pub fn ago_label(minutes: u32) -> String {
    if minutes >= 60 && minutes % 60 == 0 { format!("-{} h", minutes / 60) } else { format!("-{minutes} min") }
}

/// Keep roughly `max` points: every k-th plus the last.
pub fn decimate(points: &[(f32, f32)], max: usize) -> Vec<(f32, f32)> {
    if points.len() <= max || max < 2 {
        return points.to_vec();
    }
    let k = points.len().div_ceil(max);
    let mut out: Vec<(f32, f32)> = points.iter().step_by(k).copied().collect();
    if let (Some(last), Some(o)) = (points.last(), out.last()) {
        if last != o {
            out.push(*last);
        }
    }
    out
}

/// The label of an extreme: "max 85.4 °C 11:35".
pub fn extreme_label(kind: &str, unit: Unit, v: f64, t: u64) -> String {
    format!("{kind} {} {}", format_value(unit, v), fmt_clock(t))
}

/// Where a label of `size` goes next to the point `at`: centred above it (or below when `above`
/// is false), else on the other side, else to its left or right, taking the first spot that lies
/// inside `plot` and clear of everything in `avoid`. When none is free the first one, pulled
/// inside the plot, is used.
pub fn place_label(size: Vec2, at: Pos2, above: bool, plot: Rect, avoid: &[Rect]) -> Rect {
    let gap = 5.0;
    let cx = (at.x - size.x / 2.0).clamp(plot.left(), (plot.right() - size.x).max(plot.left()));
    let over = Rect::from_min_size(pos2(cx, at.y - gap - size.y), size);
    let under = Rect::from_min_size(pos2(cx, at.y + gap), size);
    let ty = at.y - size.y / 2.0;
    let left = Rect::from_min_size(pos2(at.x - gap - size.x, ty), size);
    let right = Rect::from_min_size(pos2(at.x + gap, ty), size);
    let order = if above { [over, under, left, right] } else { [under, over, left, right] };
    let free = |r: &Rect| plot.expand(0.5).contains_rect(*r) && !avoid.iter().any(|a| a.intersects(r.expand2(vec2(2.0, 1.0))));
    let pick = order.iter().find(|r| free(r)).copied().unwrap_or(order[0]);
    let x = pick.left().clamp(plot.left(), (plot.right() - size.x).max(plot.left()));
    let y = pick.top().clamp(plot.top(), (plot.bottom() - size.y).max(plot.top()));
    Rect::from_min_size(pos2(x, y), size)
}

/// Padding of a label's backing: the plot never lets two of these touch.
const LABEL_PAD: Vec2 = Vec2::new(3.0, 1.0);

/// Is `r` inside `plot` and clear of every rectangle in `avoid` (their backings included)?
fn label_free(r: &Rect, plot: Rect, avoid: &[Rect]) -> bool {
    plot.expand(0.5).contains_rect(*r) && !avoid.iter().any(|a| a.expand2(LABEL_PAD).intersects(r.expand2(LABEL_PAD)))
}

/// Where the labels of a popover go. Nothing overlaps, and nothing sits outside the plot.
#[derive(Clone, Debug, PartialEq)]
pub struct LabelPlan {
    /// One per threshold line, in the order given; the first always has a spot, a later one that
    /// finds none (a plot too small for them) is `None`.
    pub thresholds: Vec<Option<Rect>>,
    /// One per extreme, in the order given; `None` for one that found no free spot and is left out.
    pub extremes: Vec<Option<Rect>>,
}

/// Lay out the labels of a popover plot, most important first: the threshold labels (`lines`:
/// the line's y and its label's size, critical before warning), then the extremes (`extremes`:
/// the point, the label's size and whether it prefers to sit above, the maximum before the
/// minimum). A threshold label sits at the right end, the topmost line's above its line and the
/// others' below, on the other side when that is outside the plot, and nudged past the labels
/// already placed when both are taken. An extreme takes the free spot nearest its point (above or
/// below it, then at the ends of the plot, then beside it); with none free it is dropped, because
/// the lower-priority label gives way.
pub fn plan_labels(plot: Rect, lines: &[(f32, Vec2)], extremes: &[(Pos2, Vec2, bool)]) -> LabelPlan {
    let top_y = lines.iter().map(|l| l.0).fold(f32::INFINITY, f32::min);
    let mut placed: Vec<Rect> = Vec::new();
    let mut thresholds = Vec::new();
    for (y, size) in lines {
        let (above_at, above_align) = threshold_label_anchor(plot, *y, true);
        let (below_at, below_align) = threshold_label_anchor(plot, *y, false);
        let above = above_align.anchor_size(above_at, *size);
        let below = below_align.anchor_size(below_at, *size);
        let right = plot.right() - 3.0 - size.x;
        let under_taken = placed.iter().map(|r| r.bottom() + 2.0 * LABEL_PAD.y + 1.0).fold(plot.top(), f32::max);
        let over_taken = placed.iter().map(|r| r.top() - 2.0 * LABEL_PAD.y - 1.0 - size.y).fold(plot.bottom() - size.y, f32::min);
        let nudged_down = Rect::from_min_size(pos2(right, under_taken.max(*y + 1.0)), *size);
        let nudged_up = Rect::from_min_size(pos2(right, over_taken.min(*y - 1.0 - size.y)), *size);
        let order = if *y <= top_y { [above, below, nudged_down, nudged_up] } else { [below, above, nudged_down, nudged_up] };
        let pick = order.iter().find(|r| label_free(r, plot, &placed)).copied().or_else(|| {
            // nowhere clear: the first label takes its preferred side pulled inside the plot; a
            // later one gives way
            placed.is_empty().then(|| {
                let r = order[0];
                Rect::from_min_size(pos2(r.left(), r.top().clamp(plot.top(), (plot.bottom() - size.y).max(plot.top()))), *size)
            })
        });
        placed.extend(pick);
        thresholds.push(pick);
    }
    let mut out = Vec::new();
    for (at, size, above) in extremes {
        // nearest to its point first; a label has a backing, so it may sit over a dashed line
        let spot = try_place_label(*size, *at, *above, plot, &placed);
        if let Some(r) = spot {
            placed.push(r);
        }
        out.push(spot);
    }
    LabelPlan { thresholds, extremes: out }
}

/// The first spot around `at` for a label of `size` that lies inside `plot` and clear of `avoid`:
/// centred above or below the point (the preferred side first), then pushed against either end
/// of the plot (clear of the line and the dot), then to its left or right. `None` when every one
/// is taken.
pub fn try_place_label(size: Vec2, at: Pos2, above: bool, plot: Rect, avoid: &[Rect]) -> Option<Rect> {
    let gap = 5.0;
    let cx = (at.x - size.x / 2.0).clamp(plot.left(), (plot.right() - size.x).max(plot.left()));
    let (over_y, under_y) = (at.y - gap - size.y, at.y + gap);
    let ty = at.y - size.y / 2.0;
    let sides = [(cx, over_y), (cx, under_y)];
    let ends = [(plot.right() - size.x, over_y), (plot.right() - size.x, under_y), (plot.left(), over_y), (plot.left(), under_y)];
    // beside the point the label's backing must not touch the dot's neighbours: a wider gap
    let aside = gap + 2.0 * LABEL_PAD.x;
    let beside = [(at.x - aside - size.x, ty), (at.x + aside, ty)];
    let ordered = if above { sides } else { [sides[1], sides[0]] };
    ordered.into_iter().chain(ends).chain(beside).map(|(x, y)| Rect::from_min_size(pos2(x, y), size)).find(|r| label_free(r, plot, avoid))
}

/// The line under the plot that always names the onset: since when the reading is past a
/// threshold, or when its most recent crossing began and ended, with its peak. The level is
/// `Some` only while it lasts (amber or red); a crossing that is over reads in quiet ink.
pub fn onset_note(series: Option<&HistorySeries>, crossing: Option<&Crossing>, unit: Unit, thr: &Thr, now_ms: u64, window_ms: u64) -> Option<(String, Option<Level>)> {
    if let Some(c) = crossing {
        let word = if c.level == CrossingLevel::Crit { "critical" } else { "warning" };
        let dir = if c.below { "Below" } else { "Above" };
        let peak = format!("{} {} at {}", if c.below { "lowest" } else { "peak" }, format_value(unit, c.peak), fmt_clock(c.peak_ms));
        return Some(match c.end_ms {
            None => (format!("{dir} {word} since {}, {peak}", fmt_clock(c.start_ms)), Some(if c.level == CrossingLevel::Crit { Level::Crit } else { Level::Warn })),
            Some(end) => (format!("Last crossed {word}: {} to {}, {peak}", fmt_clock(c.start_ms), fmt_clock(end)), None),
        });
    }
    // A collector without crossings (version 2): the series' own last crossing into a level.
    let view = build_view(series?, now_ms, window_ms, thr)?;
    let (t, level) = view.onset?;
    let word = if level == Level::Crit { "critical" } else { "warning" };
    let now_past = series?.points.last().is_some_and(|p| level_of(p.1 as f64, thr) != Level::Normal);
    Some((format!("Past its {word} level since {}", fmt_clock(t)), now_past.then_some(level)))
}

pub struct PopInput<'a> {
    pub color: Color32,
    pub title: String,
    pub unit: Unit,
    pub value: Option<f64>,
    pub thr: Thr,
    pub series: Option<&'a HistorySeries>,
    pub now_ms: u64,
    pub history_minutes: u32,
    /// The collector's record of this reading's current or most recent crossing, if any.
    pub crossing: Option<Crossing>,
}

/// The plot in words, for a screen reader: "History of CPU, Tctl, last 30 minutes: lowest 58.0 °C
/// at 05:42, highest 84.2 °C at 06:08". (The note under the plot says since when it is past a limit.)
pub fn spoken_summary(inp: &PopInput) -> String {
    let minutes = inp.history_minutes.max(1);
    let head = format!("History of {}, {}", inp.title, if minutes == 1 { "last minute".to_string() } else { format!("last {minutes} minutes") });
    let view = inp.series.and_then(|s| build_view(s, inp.now_ms, u64::from(minutes) * 60_000, &inp.thr));
    let Some(view) = view else { return format!("{head}: no history yet") };
    if inp.unit == Unit::Flag {
        return format!("{head}: {}", if view.bands.is_empty() { "not active in this window" } else { "active at times in this window" });
    }
    match (view.min, view.max) {
        (Some(mn), Some(mx)) => format!(
            "{head}: lowest {} at {}, highest {} at {}",
            format_value(inp.unit, mn.1),
            fmt_clock(mn.0),
            format_value(inp.unit, mx.1),
            fmt_clock(mx.0)
        ),
        _ => format!("{head}: no history yet"),
    }
}

/// Paint the popover contents into `rect` (the area inside the frame's margins).
pub fn paint(p: &Painter, rect: Rect, pal: &Palette, inp: &PopInput) {
    // Title row: device dot, name, current value on the right.
    let title_h = 22.0;
    p.circle_filled(pos2(rect.left() + 4.0, rect.top() + title_h / 2.0), 4.0, inp.color);
    p.text(pos2(rect.left() + 14.0, rect.top() + title_h / 2.0), Align2::LEFT_CENTER, &inp.title, f_label_bold(), pal.ink);
    if let Some(v) = inp.value {
        let (num, unit) = split_value(inp.unit, v);
        let mut x = rect.right();
        if !unit.is_empty() {
            let r = p.text(pos2(x, rect.top() + title_h / 2.0), Align2::RIGHT_CENTER, &unit, f_unit(), pal.ink3);
            x = r.left() - 3.0;
        }
        p.text(pos2(x, rect.top() + title_h / 2.0), Align2::RIGHT_CENTER, num, f_result(), pal.ink);
    }

    let window_ms = inp.history_minutes.max(1) as u64 * 60_000;
    let view = inp.series.and_then(|s| build_view(s, inp.now_ms, window_ms, &inp.thr));
    let is_flag = inp.unit == Unit::Flag;

    // Gutters first: the y labels (the domain's bottom and top) get a column of their own and the
    // x labels a row of their own, the plot sits between them, so no label can land on the plot.
    let body = Rect::from_min_max(pos2(rect.left(), rect.top() + title_h + 8.0), rect.right_bottom());
    let y_labels = view.as_ref().filter(|_| !is_flag).map(|v| (axis_text(inp.unit, v.y1, v.y1), axis_text(inp.unit, v.y0, v.y1)));
    let gutter_w = y_labels
        .as_ref()
        .map(|(top, bottom)| {
            let w = |t: &String| p.layout_no_wrap(t.clone(), f_minmax(), pal.ink3).size().x;
            w(top).max(w(bottom)).ceil() + 8.0
        })
        .unwrap_or(0.0);
    let lay = layout(body, gutter_w);
    let plot = lay.plot;

    if let Some((top, bottom)) = &y_labels {
        p.text(pos2(lay.left.right() - 6.0, plot.top()), Align2::RIGHT_TOP, top, f_minmax(), pal.ink3);
        p.text(pos2(lay.left.right() - 6.0, plot.bottom()), Align2::RIGHT_BOTTOM, bottom, f_minmax(), pal.ink3);
    }
    let ago = p.text(pos2(plot.left(), lay.bottom.center().y), Align2::LEFT_CENTER, ago_label(inp.history_minutes.max(1)), f_minmax(), pal.ink3);
    let now = p.text(pos2(plot.right(), lay.bottom.center().y), Align2::RIGHT_CENTER, "now", f_minmax(), pal.ink3);
    p.hline(plot.x_range(), plot.bottom(), Stroke::new(1.0, pal.rule));

    let (Some(series), Some(view)) = (inp.series, view) else {
        p.text(plot.center(), Align2::CENTER_CENTER, "No history yet", f_label_bold(), pal.ink3);
        return;
    };

    let tx = |t: u64| plot.left() + ((t.saturating_sub(view.t0)) as f32 / (view.t1 - view.t0).max(1) as f32).clamp(0.0, 1.0) * plot.width();
    let ty = |v: f64| plot.bottom() - (((v - view.y0) / (view.y1 - view.y0)) as f32).clamp(0.0, 1.0) * plot.height();

    if is_flag {
        for &(a, b) in &view.bands {
            let r = Rect::from_min_max(pos2(tx(a), plot.top()), pos2(tx(b).max(tx(a) + 2.0), plot.bottom()));
            // red only for a flag that means trouble (thermal throttling), quiet for the rest
            let band = if inp.thr.crit.is_some() || inp.thr.warn.is_some() { pal.crit } else { pal.ink3 };
            p.rect_filled(r, 0.0, band.gamma_multiply(0.45));
        }
        if view.bands.is_empty() {
            p.text(plot.center(), Align2::CENTER_CENTER, "Not active in this window", f_minmax(), pal.ink3);
        }
        return;
    }

    // Threshold lines. The upper line's label sits above it and the lower line's below, so two
    // close thresholds do not overprint each other.
    let mut lines: Vec<(f32, Color32, String)> = Vec::new();
    for (limit, color) in [(inp.thr.warn, pal.warn), (inp.thr.crit, pal.crit)] {
        if let Some(l) = limit {
            if l >= view.y0 && l <= view.y1 {
                lines.push((ty(l), color, format_value(inp.unit, l)));
            }
        }
    }
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (y, color, _) in &lines {
        let pts = [pos2(plot.left(), *y), pos2(plot.right(), *y)];
        p.add(Shape::dashed_line(&pts, Stroke::new(1.0, *color), 4.0, 3.0));
    }

    // Onset tick, its time in the bottom gutter (left out when it would touch "-30 min" or "now";
    // the note under the plot names it either way). The collector's record wins over what the
    // series alone shows, because it also knows an onset before the window began.
    let onset = match &inp.crossing {
        Some(c) if c.start_ms >= view.t0 && c.start_ms <= view.t1 => Some((c.start_ms, if c.level == CrossingLevel::Crit { Level::Crit } else { Level::Warn })),
        Some(_) => None,
        None => view.onset,
    };
    if let Some((t, level)) = onset {
        let x = tx(t);
        let c = pal.level_color(level).unwrap_or(pal.warn);
        p.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], Stroke::new(1.0, c.gamma_multiply(0.7)));
        let g = p.layout_no_wrap(fmt_clock(t), f_minmax(), c);
        let half = g.size().x / 2.0;
        let r = Rect::from_center_size(pos2(x.clamp(plot.left() + half, plot.right() - half), lay.bottom.center().y), g.size());
        if !r.expand(4.0).intersects(ago) && !r.expand(4.0).intersects(now) {
            p.galley(r.min, g, c);
        }
    }

    // The hairline.
    let pts: Vec<(f32, f32)> = points_in(series, view.t0, view.t1).into_iter().map(|(t, v)| (tx(t), ty(v))).collect();
    let pts = decimate(&pts, (plot.width() * 2.0) as usize);
    let line: Vec<egui::Pos2> = pts.iter().map(|&(x, y)| pos2(x, y)).collect();
    if line.len() >= 2 {
        p.add(Shape::line(line, Stroke::new(1.5, pal.ink)));
    } else if let Some(&(x, y)) = pts.first() {
        p.circle_filled(pos2(x, y), 2.0, pal.ink);
    }

    // Labels, on a panel-coloured backing so the hairline and the dashes never touch the digits.
    // Thresholds first (critical, then warning), then the extremes (the highest, then the lowest);
    // one plan places them all so no two overlap and a label with no room is left out.
    let by_priority: Vec<&(f32, Color32, String)> = {
        let mut v: Vec<&(f32, Color32, String)> = lines.iter().collect();
        // critical before warning, whichever line is higher
        v.sort_by_key(|l| l.1 != pal.crit);
        v
    };
    let line_galleys: Vec<_> = by_priority.iter().map(|(_, color, label)| p.layout_no_wrap(label.clone(), f_minmax(), *color)).collect();
    let spans: Vec<(f32, Vec2)> = by_priority.iter().zip(&line_galleys).map(|(l, g)| (l.0, g.size())).collect();
    let mut extreme_marks: Vec<(Pos2, &str, f64, u64, bool)> = Vec::new();
    if let (Some(mn), Some(mx)) = (view.min, view.max) {
        if (mx.1 - mn.1).abs() > f64::EPSILON {
            for ((t, v), kind, above) in [(mx, "max", true), (mn, "min", false)] {
                extreme_marks.push((pos2(tx(t), ty(v)), kind, v, t, above));
            }
        }
    }
    let ex_galleys: Vec<_> = extreme_marks.iter().map(|(_, kind, v, t, _)| p.layout_no_wrap(extreme_label(kind, inp.unit, *v, *t), f_minmax(), pal.ink2)).collect();
    let asks: Vec<(Pos2, Vec2, bool)> = extreme_marks.iter().zip(&ex_galleys).map(|(m, g)| (m.0, g.size(), m.4)).collect();
    let plan = plan_labels(plot, &spans, &asks);
    for ((rect, g), (_, color, _)) in plan.thresholds.iter().zip(line_galleys).zip(by_priority) {
        if let Some(rect) = rect {
            p.rect_filled(rect.expand2(LABEL_PAD), 2.0, pal.panel);
            p.galley(rect.min, g, *color);
        }
    }
    for ((rect, g), mark) in plan.extremes.iter().zip(ex_galleys).zip(&extreme_marks) {
        p.circle_filled(mark.0, 2.5, pal.ink);
        if let Some(r) = rect {
            p.rect_filled(r.expand2(LABEL_PAD), 2.0, pal.panel);
            p.galley(r.min, g, pal.ink2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(unit: Unit, pts: &[(u64, f32)]) -> HistorySeries {
        HistorySeries { id: "x".into(), unit, points: pts.to_vec() }
    }

    const MIN: u64 = 60_000;

    #[test]
    fn threshold_labels_sit_at_the_right_end_clear_of_the_left_edge() {
        let plot = Rect::from_min_max(pos2(10.0, 40.0), pos2(350.0, 140.0));
        let (up, up_align) = threshold_label_anchor(plot, 60.0, true);
        let (low, low_align) = threshold_label_anchor(plot, 80.0, false);
        assert!(up.x > plot.center().x && low.x > plot.center().x, "right end, not the left edge");
        assert!(up.x <= plot.right() && low.x <= plot.right());
        assert_eq!((up_align, low_align), (Align2::RIGHT_BOTTOM, Align2::RIGHT_TOP));
        assert!(up.y < 60.0 && low.y > 80.0, "the upper label sits above its line, the lower below");
    }

    #[test]
    fn the_plot_is_inset_between_a_left_gutter_and_a_bottom_gutter() {
        let body = Rect::from_min_max(pos2(10.0, 40.0), pos2(366.0, 164.0));
        let l = layout(body, 28.0);
        // the three parts never share a pixel and all lie inside the body
        let overlap = |a: Rect, b: Rect| a.intersect(b).is_positive();
        assert!(!overlap(l.plot, l.left) && !overlap(l.plot, l.bottom) && !overlap(l.left, l.bottom));
        for r in [l.plot, l.left, l.bottom] {
            assert!(body.contains_rect(r), "{r:?}");
        }
        assert_eq!(l.left.right(), l.plot.left());
        assert_eq!(l.bottom.top(), l.plot.bottom());
        assert_eq!(l.left.height(), l.plot.height());
        assert_eq!(l.bottom.width(), l.plot.width());
        assert!(l.left.width() == 28.0 && l.bottom.height() == BOTTOM_GUTTER);
        // without y labels (flags) the plot takes the full width
        assert_eq!(layout(body, 0.0).plot.left(), body.left());
        // a very short body still leaves a plot
        assert!(layout(Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 30.0)), 20.0).plot.height() >= 20.0 - 0.01);
    }

    #[test]
    fn near_flat_series_get_a_minimum_span() {
        let none = Thr::default();
        // percent: 0 to 0.4 still plots 0 to 10; 0 to 20 gets a round top
        assert_eq!(history_domain(Unit::Percent, 0.0, 0.4, &none), (0.0, 10.0));
        let (lo, hi) = history_domain(Unit::Percent, 0.0, 19.4, &none);
        assert_eq!((lo, hi), (0.0, 20.0));
        assert_eq!(history_domain(Unit::Percent, 40.0, 100.0, &none).1, 100.0, "never past 100");
        // temperatures: at least 5 degrees, bottom on a multiple of 5
        let (lo, hi) = history_domain(Unit::Celsius, 27.0, 27.2, &none);
        assert_eq!(lo, 25.0);
        assert!(hi - lo >= 5.0 && hi % 5.0 == 0.0, "{lo} {hi}");
        // other units: at least a tenth of the value, from zero
        let (lo, hi) = history_domain(Unit::Rpm, 2650.0, 2650.0, &none);
        assert_eq!(lo, 0.0);
        assert!(hi - lo >= 265.0);
        assert!(min_span(Unit::Watt, 150.0) >= 15.0 - 1e-9 && (min_span(Unit::Percent, 3.0) - 10.0).abs() < 1e-9);
        // thresholds stretch the top, as in the rows
        let thr = Thr { warn: Some(80.0), crit: Some(90.0), below: false };
        assert!(history_domain(Unit::Celsius, 40.0, 50.0, &thr).1 >= 90.0);
        assert!(history_domain(Unit::Percent, 5.0, 12.0, &Thr { warn: Some(85.0), crit: None, below: false }).1 >= 85.0);
        // round tops
        assert_eq!(nice_up(19.6), 20.0);
        assert_eq!(nice_up(21.0), 25.0);
        assert_eq!(nice_up(4120.0), 5000.0);
        assert_eq!(nice_up(0.0), 1.0);
    }

    #[test]
    fn axis_labels_are_compact() {
        assert_eq!(axis_text(Unit::Percent, 20.0, 20.0), "20");
        assert_eq!(axis_text(Unit::Percent, 0.0, 20.0), "0");
        assert_eq!(axis_text(Unit::Celsius, 25.0, 75.0), "25");
        assert_eq!(axis_text(Unit::Volt, 1.35, 1.4), "1.35");
        let g = 1024.0 * 1024.0 * 1024.0;
        assert_eq!(axis_text(Unit::Bytes, 24.0 * g, 24.0 * g), "24");
        assert_eq!(axis_text(Unit::Bytes, 0.0, 24.0 * g), "0");
    }

    #[test]
    fn empty_history_has_no_view() {
        assert!(build_view(&series(Unit::Celsius, &[]), 100 * MIN, 30 * MIN, &Thr::default()).is_none());
        // points outside the window do not count either
        assert!(build_view(&series(Unit::Celsius, &[(MIN, 40.0)]), 100 * MIN, 30 * MIN, &Thr::default()).is_none());
    }

    #[test]
    fn extremes_domain_and_window() {
        let now = 100 * MIN;
        let s = series(Unit::Celsius, &[(now - 20 * MIN, 41.0), (now - 10 * MIN, 67.0), (now - MIN, 50.0)]);
        let v = build_view(&s, now, 30 * MIN, &Thr::default()).unwrap();
        assert_eq!(v.min, Some((now - 20 * MIN, 41.0)));
        assert_eq!(v.max, Some((now - 10 * MIN, 67.0)));
        assert_eq!(v.t0, now - 30 * MIN);
        assert_eq!(v.t1, now);
        assert_eq!(v.y0, 25.0);
        assert_eq!(v.y1, 75.0, "67 * 1.05 rounds up to a multiple of 5");
        assert!(v.onset.is_none());
    }

    #[test]
    fn onset_is_the_last_crossing_into_a_higher_level() {
        let now = 100 * MIN;
        let thr = Thr { warn: Some(80.0), crit: Some(90.0), below: false };
        let s = series(
            Unit::Celsius,
            &[
                (now - 25 * MIN, 60.0),
                (now - 20 * MIN, 82.0), // into warning
                (now - 15 * MIN, 60.0),
                (now - 10 * MIN, 85.0), // into warning again
                (now - 5 * MIN, 92.0),  // into critical
                (now - MIN, 70.0),
            ],
        );
        let v = build_view(&s, now, 30 * MIN, &thr).unwrap();
        assert_eq!(v.onset, Some((now - 5 * MIN, Level::Crit)));
        // when it never crossed there is no onset
        let calm = series(Unit::Celsius, &[(now - 5 * MIN, 50.0), (now - MIN, 52.0)]);
        assert!(build_view(&calm, now, 30 * MIN, &thr).unwrap().onset.is_none());
        // a flat run inside the warning zone does not move the onset
        let steady = series(Unit::Celsius, &[(now - 9 * MIN, 50.0), (now - 8 * MIN, 85.0), (now - 2 * MIN, 86.0)]);
        assert_eq!(build_view(&steady, now, 30 * MIN, &thr).unwrap().onset, Some((now - 8 * MIN, Level::Warn)));
    }

    #[test]
    fn extremes_are_labelled_with_their_value_and_time() {
        let t = 1_700_000_000_000;
        assert_eq!(extreme_label("max", Unit::Celsius, 85.4, t), format!("max 85.4 °C {}", fmt_clock(t)));
        assert_eq!(extreme_label("min", Unit::Watt, 12.0, t), format!("min 12.0 W {}", fmt_clock(t)));
    }

    #[test]
    fn a_label_goes_where_nothing_else_is() {
        let plot = Rect::from_min_max(pos2(0.0, 0.0), pos2(300.0, 120.0));
        let size = vec2(100.0, 14.0);
        // above its point when there is room, centred on it
        let r = place_label(size, pos2(150.0, 60.0), true, plot, &[]);
        assert!(r.bottom() < 60.0 && (r.center().x - 150.0).abs() < 0.01, "{r:?}");
        // below for the minimum
        let r = place_label(size, pos2(150.0, 60.0), false, plot, &[]);
        assert!(r.top() > 60.0, "{r:?}");
        // at the top edge it drops under the point, at the sides it stays inside the plot
        let r = place_label(size, pos2(150.0, 4.0), true, plot, &[]);
        assert!(r.top() > 4.0 && plot.contains_rect(r), "{r:?}");
        let r = place_label(size, pos2(296.0, 60.0), true, plot, &[]);
        assert!(plot.contains_rect(r), "{r:?}");
        let r = place_label(size, pos2(2.0, 60.0), true, plot, &[]);
        assert!(plot.contains_rect(r), "{r:?}");
        // it keeps clear of a threshold label sitting where it would go
        let taken = Rect::from_min_max(pos2(100.0, 40.0), pos2(200.0, 56.0));
        let r = place_label(size, pos2(150.0, 60.0), true, plot, &[taken]);
        assert!(!r.intersects(taken), "{r:?}");
        // and a label never overlaps the one placed before it
        let first = place_label(size, pos2(150.0, 60.0), true, plot, &[]);
        let second = place_label(size, pos2(160.0, 60.0), true, plot, &[first]);
        assert!(!first.intersects(second));
    }

    /// No two rectangles share a pixel (their 3 x 1 backings included) and all lie inside `plot`.
    fn assert_clear(plot: Rect, rects: &[Rect], what: &str) {
        for (i, a) in rects.iter().enumerate() {
            assert!(plot.expand(0.5).contains_rect(*a), "{what}: label {i} {a:?} leaves the plot {plot:?}");
            for (j, b) in rects.iter().enumerate().skip(i + 1) {
                assert!(!a.expand2(LABEL_PAD).intersects(b.expand2(LABEL_PAD)), "{what}: labels {i} and {j} overlap: {a:?} {b:?}");
            }
        }
    }

    #[test]
    fn threshold_and_extreme_labels_never_overlap_and_the_lower_priority_one_gives_way() {
        // the popover of batch B's render: domain 25 to 95, warning 80 and critical 90 both near
        // the top of a 120 px plot, the maximum (81.6) just above the warning line
        let plot = Rect::from_min_max(pos2(40.0, 40.0), pos2(380.0, 160.0));
        let y = |v: f32| plot.bottom() - (v - 25.0) / 70.0 * plot.height();
        let t = vec2(52.0, 13.0);
        let lines = [(y(90.0), t), (y(80.0), t)];
        let ex = [(pos2(plot.left() + 0.78 * plot.width(), y(81.6)), vec2(104.0, 13.0), true), (pos2(plot.left() + 0.05 * plot.width(), y(58.0)), vec2(104.0, 13.0), false)];
        let plan = plan_labels(plot, &lines, &ex);
        let mut all: Vec<Rect> = plan.thresholds.iter().flatten().copied().collect();
        all.extend(plan.extremes.iter().flatten().copied());
        assert_eq!(all.len() - plan.extremes.iter().flatten().count(), 2, "both threshold labels are placed in a plot of this size");
        assert_clear(plot, &all, "close thresholds near the top");
        // the critical label is inside the plot, not over the title row, and the warning label stays near its line
        let (crit, warn) = (plan.thresholds[0].unwrap(), plan.thresholds[1].unwrap());
        assert!(crit.top() >= plot.top());
        assert!((warn.center().y - lines[1].0).abs() < 20.0, "the warning label stays near its line");
        // the minimum, far from everything, is always placed
        assert!(plan.extremes[1].is_some());
        // a label with no room is dropped, never drawn over another
        let tiny = Rect::from_min_max(pos2(0.0, 0.0), pos2(120.0, 40.0));
        let crowded = plan_labels(tiny, &[(8.0, t), (20.0, t)], &[(pos2(60.0, 14.0), vec2(104.0, 13.0), true)]);
        let mut all: Vec<Rect> = crowded.thresholds.iter().flatten().copied().collect();
        all.extend(crowded.extremes.iter().flatten().copied());
        assert!(crowded.thresholds[0].is_some(), "the critical label is always there");
        assert_clear(tiny, &all, "crowded");
        assert_eq!(crowded.extremes, vec![None], "the maximum gives way to the thresholds");
    }

    #[test]
    fn well_spaced_labels_sit_where_they_always_did() {
        let plot = Rect::from_min_max(pos2(40.0, 40.0), pos2(380.0, 160.0));
        let t = vec2(52.0, 13.0);
        // warning at 60 % height, critical at 25 %: the upper label above its line, the lower below
        let lines = [(70.0, t), (120.0, t)];
        let plan = plan_labels(plot, &lines, &[]);
        let (up, low) = (plan.thresholds[0].unwrap(), plan.thresholds[1].unwrap());
        assert!(up.bottom() < 70.0 && low.top() > 120.0);
        assert!([up, low].iter().all(|r| r.right() <= plot.right() - 3.0 + 0.01), "at the right end");
        // an extreme with room is centred above or below its point as before
        let ex = [(pos2(200.0, 100.0), vec2(104.0, 13.0), true), (pos2(120.0, 140.0), vec2(104.0, 13.0), false)];
        let plan = plan_labels(plot, &[], &ex);
        let (max, min) = (plan.extremes[0].unwrap(), plan.extremes[1].unwrap());
        assert!(max.bottom() < 100.0 && (max.center().x - 200.0).abs() < 0.01);
        assert!(min.top() > 140.0);
    }

    #[test]
    fn a_series_out_of_time_order_is_drawn_in_order() {
        let now = 100 * MIN;
        let pts: Vec<(u64, f32)> = vec![(now - 5 * MIN, 60.0), (now - 2 * MIN, 70.0), (now - 4 * MIN, 65.0), (now - MIN, 61.0)];
        let s = series(Unit::Celsius, &pts);
        let pts = points_in(&s, now - 30 * MIN, now);
        assert!(pts.windows(2).all(|w| w[0].0 <= w[1].0), "{pts:?}");
        assert_eq!(pts.len(), 4);
        // a point stamped after now (the clock stepped back since) is not drawn
        let s = series(Unit::Celsius, &[(now - MIN, 60.0), (now + 20 * MIN, 99.0)]);
        assert_eq!(points_in(&s, now - 30 * MIN, now).len(), 1);
    }

    #[test]
    fn the_plot_has_a_text_equivalent() {
        let now = 100 * MIN;
        let pts: Vec<(u64, f32)> = (0..=30).map(|i| (now - (30 - i) * MIN, if i == 10 { 58.0 } else if i == 20 { 81.6 } else { 62.0 })).collect();
        let s = series(Unit::Celsius, &pts);
        let mut inp = PopInput { color: Color32::WHITE, title: "CPU, Tctl".into(), unit: Unit::Celsius, value: Some(62.0), thr: Thr::default(), series: Some(&s), now_ms: now, history_minutes: 30, crossing: None };
        let t = spoken_summary(&inp);
        assert!(t.starts_with("History of CPU, Tctl, last 30 minutes: lowest 58.0 °C at "), "{t}");
        assert!(t.contains("highest 81.6 °C at "), "{t}");
        inp.series = None;
        assert_eq!(spoken_summary(&inp), "History of CPU, Tctl, last 30 minutes: no history yet");
    }

    #[test]
    fn the_onset_is_always_named() {
        let now = 100 * MIN;
        let thr = Thr { warn: Some(80.0), crit: Some(90.0), below: false };
        let w = 30 * MIN;
        let going = Crossing { id: "x".into(), level: CrossingLevel::Warn, start_ms: now - 4 * MIN, peak: 85.4, peak_ms: now - 2 * MIN, end_ms: None, below: false };
        let (text, level) = onset_note(None, Some(&going), Unit::Celsius, &thr, now, w).unwrap();
        assert_eq!(level, Some(Level::Warn));
        assert!(text.starts_with(&format!("Above warning since {}", fmt_clock(now - 4 * MIN))), "{text}");
        assert!(text.contains(&format!("peak 85.4 °C at {}", fmt_clock(now - 2 * MIN))), "{text}");
        // a crossing that is over reads in quiet ink with both times
        let over = Crossing { end_ms: Some(now - MIN), level: CrossingLevel::Crit, ..going.clone() };
        let (text, level) = onset_note(None, Some(&over), Unit::Celsius, &thr, now, w).unwrap();
        assert_eq!(level, None);
        assert!(text.starts_with("Last crossed critical: ") && text.contains(" to "), "{text}");
        // a collector without crossings: the series' own last crossing, amber only while it lasts
        let s = series(Unit::Celsius, &[(now - 20 * MIN, 60.0), (now - 10 * MIN, 85.0), (now - MIN, 86.0)]);
        let (text, level) = onset_note(Some(&s), None, Unit::Celsius, &thr, now, w).unwrap();
        assert_eq!(level, Some(Level::Warn));
        assert_eq!(text, format!("Past its warning level since {}", fmt_clock(now - 10 * MIN)));
        let cooled = series(Unit::Celsius, &[(now - 20 * MIN, 60.0), (now - 10 * MIN, 85.0), (now - MIN, 60.0)]);
        assert_eq!(onset_note(Some(&cooled), None, Unit::Celsius, &thr, now, w).unwrap().1, None);
        // never crossed: nothing to say
        let calm = series(Unit::Celsius, &[(now - 5 * MIN, 50.0)]);
        assert!(onset_note(Some(&calm), None, Unit::Celsius, &thr, now, w).is_none());
        assert!(onset_note(None, None, Unit::Celsius, &thr, now, w).is_none());
        // a reading that alerts below says lowest
        let low = Crossing { below: true, peak: 300.0, ..going };
        assert!(onset_note(None, Some(&low), Unit::Rpm, &thr, now, w).unwrap().0.starts_with("Below warning since"));
    }

    #[test]
    fn flag_series_become_bands() {
        let now = 100 * MIN;
        let s = series(
            Unit::Flag,
            &[(now - 20 * MIN, 0.0), (now - 15 * MIN, 1.0), (now - 12 * MIN, 1.0), (now - 10 * MIN, 0.0), (now - 3 * MIN, 1.0), (now - MIN, 1.0)],
        );
        let v = build_view(&s, now, 30 * MIN, &Thr::default()).unwrap();
        assert_eq!(v.bands, vec![(now - 15 * MIN, now - 10 * MIN), (now - 3 * MIN, now - MIN)]);
        assert_eq!((v.y0, v.y1), (0.0, 1.0));
    }

    #[test]
    fn below_thresholds_cross_downwards() {
        let now = 100 * MIN;
        let thr = Thr { warn: Some(1000.0), crit: Some(400.0), below: true };
        let s = series(Unit::Rpm, &[(now - 9 * MIN, 2600.0), (now - 6 * MIN, 900.0), (now - 3 * MIN, 2600.0)]);
        assert_eq!(build_view(&s, now, 30 * MIN, &thr).unwrap().onset, Some((now - 6 * MIN, Level::Warn)));
    }

    #[test]
    fn axis_labels_and_decimation() {
        assert_eq!(ago_label(30), "-30 min");
        assert_eq!(ago_label(5), "-5 min");
        assert_eq!(ago_label(60), "-1 h");
        assert_eq!(ago_label(240), "-4 h");
        let pts: Vec<(f32, f32)> = (0..1000).map(|i| (i as f32, 0.0)).collect();
        let d = decimate(&pts, 100);
        assert!(d.len() <= 102 && d.len() >= 50);
        assert_eq!(d.last().unwrap().0, 999.0, "the newest point always stays");
        assert_eq!(decimate(&pts[..10], 100).len(), 10);
    }
}
