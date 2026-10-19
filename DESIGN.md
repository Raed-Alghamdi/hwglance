---
name: HW Glance
description: A night-shift results report for one PC's sensors and processes, read at a glance, with a day-shift print for light desktops.
colors:
  ground: "#0B0E13"
  panel: "#121821"
  header: "#171E29"
  alt-row: "#141B25"
  zebra-row: "#161E29"
  hover: "#1A2230"
  selected: "#1B2B45"
  rule: "#263040"
  ink: "#ECF1F7"
  ink-2: "#AEB8C6"
  ink-3: "#8792A3"
  device-cpu: "#5AA8FF"
  device-gpu: "#B591FF"
  device-memory: "#33D1E3"
  device-drive: "#F48FD3"
  device-board: "#A3B1C4"
  device-igpu: "#CDBBFF"
  warn: "#F7BA42"
  warn-fill: "#F7BA42"
  crit: "#FF5D62"
  on-crit: "#0B0E13"
  watch: "#4BE38F"
  shadow: "rgba(0, 0, 0, 0.5)"
  light-ground: "#E6EAF0"
  light-panel: "#FAFBFD"
  light-header: "#EDF1F6"
  light-alt-row: "#F6F8FB"
  light-zebra-row: "#F2F5F9"
  light-hover: "#E7ECF3"
  light-selected: "#D6E3F6"
  light-rule: "#C6CFDB"
  light-ink: "#111827"
  light-ink-2: "#38445A"
  light-ink-3: "#566377"
  light-device-cpu: "#1A62C3"
  light-device-gpu: "#5E33CF"
  light-device-memory: "#006A7B"
  light-device-drive: "#B12B83"
  light-device-board: "#52667E"
  light-device-igpu: "#6A54A3"
  light-warn: "#855400"
  light-warn-fill: "#E8A71C"
  light-crit: "#B8242B"
  light-on-crit: "#FAFBFD"
  light-watch: "#0A713E"
  light-shadow: "rgba(0, 0, 0, 0.22)"
typography:
  title:
    fontFamily: "Atkinson Hyperlegible Bold"
    fontSize: "16px"
    fontWeight: 700
  label:
    fontFamily: "Atkinson Hyperlegible"
    fontSize: "14px"
    fontWeight: 400
  section:
    fontFamily: "Atkinson Hyperlegible Bold"
    fontSize: "13px"
    fontWeight: 700
  small:
    fontFamily: "Atkinson Hyperlegible"
    fontSize: "12.5px"
    fontWeight: 400
  result:
    fontFamily: "AtkynsonMono Nerd Font Mono"
    fontSize: "15px"
    fontWeight: 700
  mono:
    fontFamily: "AtkynsonMono Nerd Font Mono"
    fontSize: "13.5px"
    fontWeight: 400
  unit:
    fontFamily: "AtkynsonMono Nerd Font Mono"
    fontSize: "12.5px"
    fontWeight: 400
  flag:
    fontFamily: "AtkynsonMono Nerd Font Mono"
    fontSize: "12.5px"
    fontWeight: 700
rounded:
  track: "3px"
  row: "4px"
  badge: "5px"
  widget: "6px"
  header-top: "7px"
  card: "8px"
  pill: "9999px"
spacing:
  unit: "4px"
  cell-gap: "8px"
  pad: "12px"
  frame: "16px"
  row: "28px"
  caption: "20px"
  card-header: "36px"
  hit: "24px"
  focus-ring: "2px"
  focus-gap: "1px"
components:
  card:
    backgroundColor: "{colors.panel}"
    rounded: "{rounded.card}"
  card-header:
    backgroundColor: "{colors.header}"
    height: "{spacing.card-header}"
    rounded: "{rounded.header-top}"
  device-tab:
    textColor: "{colors.device-cpu}"
    typography: "{typography.section}"
    rounded: "{rounded.widget}"
    height: "22px"
    padding: "0 8px"
  sensor-row:
    textColor: "{colors.ink-2}"
    height: "{spacing.row}"
    padding: "0 12px"
  sensor-row-odd:
    backgroundColor: "{colors.alt-row}"
  sensor-row-hover:
    backgroundColor: "{colors.hover}"
  sensor-row-watched:
    textColor: "{colors.watch}"
  throttle-pill-now:
    backgroundColor: "{colors.crit}"
    textColor: "{colors.on-crit}"
    rounded: "{rounded.pill}"
    padding: "3px 8px"
  throttle-pill-recent:
    textColor: "{colors.warn}"
    rounded: "{rounded.pill}"
    padding: "3px 8px"
  legend-chip:
    textColor: "{colors.ink-2}"
    typography: "{typography.label}"
    rounded: "{rounded.widget}"
    height: "28px"
  legend-chip-hover:
    backgroundColor: "{colors.hover}"
  summary-chip:
    backgroundColor: "{colors.header}"
    rounded: "{rounded.pill}"
    height: "24px"
    padding: "0 10px"
  process-header:
    backgroundColor: "{colors.header}"
    rounded: "{rounded.widget}"
    height: "28px"
  process-row-odd:
    backgroundColor: "{colors.zebra-row}"
  process-row-selected:
    backgroundColor: "{colors.selected}"
    rounded: "{rounded.row}"
  icon-plate:
    backgroundColor: "{colors.ink-2}"
    rounded: "{rounded.badge}"
    size: "24px"
  edit-bar:
    backgroundColor: "{colors.header}"
    textColor: "{colors.ink-2}"
    rounded: "{rounded.card}"
    padding: "8px 12px"
  button:
    backgroundColor: "{colors.header}"
    textColor: "{colors.ink-2}"
    rounded: "{rounded.widget}"
    padding: "4px 10px"
  button-hover:
    backgroundColor: "{colors.hover}"
    textColor: "{colors.ink}"
  history-popover:
    backgroundColor: "{colors.panel}"
    rounded: "{rounded.card}"
    padding: "12px"
    width: "380px"
    height: "170px"
---

# Design System: HW Glance

Scope: the native egui window (`crates/hwglance/src/`). The Omarchy bar widget (button and popup) is a scoped exception, described at the end of Components.

## Overview

**Creative North Star: "The Night-Shift Results Report"**

Every value is a result read against its own range, grouped by device the way a pathology report groups its panels. A blue-black ground carries lifted panels; each panel has a tinted device tab, a row of column captions and dense rows of label, current result, low, range bar, high and flag. Results are off-white in a bold mono face with tabular figures; labels are quieter in a humanist sans. Attention is rationed: amber and red appear only when a threshold is crossed, green only on values the user chose to watch, and the device legend colours only identify devices. History is not on the page; it appears on hover as a hairline plot.

The same report is printed for day shift: cool paper panels on a grey desk, with the device hues and signal colours kept but deepened until they read as text on every surface. The two palettes share every role, rule and geometry; only values change. The window follows the desktop's light or dark setting, not the Omarchy colour theme.

The window is dense but calm, built for a seated glance at a side monitor in a dim room. It is flat and tonal, with one soft shadow reserved for floating layers. It rejects gauges, glow and the terminal wall of bars; the one graphic every row carries is the reference-range bar.

**Key Characteristics:**
- Two fixed palettes (dark night shift, light day shift), chosen by the desktop's colour scheme or a setting, independent of the Omarchy colour theme; every ink passes 4.5:1 on every surface in both (unit tested).
- Device identity by colour first (tab, range bars, legend chip dot, process column headers), label second.
- Numbers right-align in fixed cells; a state recolours a row, never moves it.
- One layout unit (4 px); every height derives from the font-size setting and snaps to it.
- One authored motion: the focus flash.
- Every reset acts on one click and stays undoable for 30 s; no reset asks first.

## Colors

A cool tonal stack (blue-black for night, grey desk and paper for day) with six device legend hues and three reserved signal colours. Frontmatter keys without a prefix are the dark palette; `light-` keys are the same roles in the light palette.

### Primary (device legend)
- **Signal Blue** (device-cpu, light-device-cpu): CPU tab, CPU range bars, CPU legend dot, CPU process column header and heavy-CPU cell tint.
- **Soft Violet** (device-gpu, light-device-gpu): the same roles for every discrete GPU card (gpu0, gpu1 and on), plus the GPU, VRAM, Encode and Decode process columns.
- **Clear Cyan** (device-memory, light-device-memory): the same roles for Memory and the Memory process column.
- **Orchid** (device-drive, light-device-drive): the same roles for every drive panel and the Disk process column.
- **Slate** (device-board, light-device-board): the same roles for the motherboard and for unknown cards.
- **Pale Lilac** (device-igpu, light-device-igpu): the integrated GPU.

A legend colour is owned by its device. It may tint a device tab (22 % over the header, with the name in the device colour), wash a range track (18 % over the panel), fill an observed range (the palette's range alpha: 70 % dark, 85 % light), tint a heavy process cell (16 %), colour a legend dot or popover dot, and outline a device's cards (1.5 px) while its legend chip is hovered. It may not mean state, decoration or anything other than its device. In the light palette the hues are deepened so that each reads as text on every surface.

### Secondary (signals)
- **Amber** (warn, light-warn): warning thresholds as text and marks. Flag letters H and L, warning-level results, low and high cells past a threshold, the outlined "Throttled Nx" pill and the "Nx, last 03:00" row note for 10 minutes after the last event (then ink-3), degraded source dot, the dot of a failing source's note in a card, the "stale" pill, the warning threshold line in history.
- **Amber Wash** (warn-fill, light-warn-fill): the warning zone of a range bar (28 %). In the dark palette it equals Amber; on a light ground readable amber must be dark, so the light palette splits a brighter wash from the darker text colour.
- **Alarm Red** (crit, light-crit): critical thresholds and destructive actions. HH and LL flags, the critical zone (38 %), a filled "Throttling now" pill, a throttle flag reading Yes, collector-down states, Force kill and the kill confirmation.
- **On Red** (on-crit, light-on-crit): text on a solid red fill (the "Throttling now" pill, the light palette's confirm button). Ground in the dark palette, paper in the light one.
- **Watch Green** (watch, light-watch): watched values and nothing else. Watched row label (bold), result and min/max ink, a 3 px dot left of the label, an 8 % row wash (also on a collapsed section row and a collapsed-card chip whose summary reading is watched), the focus flash, and watched names in Settings.

### Neutral
- **Ground** (ground, light-ground): window background, top bar, status bar.
- **Panel** (panel, light-panel): cards, the process panel, popovers, dialogs, the waiting card.
- **Header** (header, light-header): card header band, process column header, inactive buttons, summary chips, the edit bar.
- **Alternate Row** (alt-row) and **Zebra Row** (zebra-row): odd sensor rows and odd process rows; in both palettes the zebra step from the panel is exactly twice the alternate-row step.
- **Hover** (hover) and **Selected** (selected): pointer and selection surfaces.
- **Rule** (rule): 1 px card and dialog strokes, the pinned-band hairline, skeleton bars, idle split handle, the track of the Undo countdown bar. Control outlines (radio circles, checkboxes, button strokes) and the slider rail use the control colour in both palettes: the rule blended 70 % toward ink-3, which keeps 3:1 on the ground, panel, header and hover surfaces.
- **Ink** (ink): results and primary text; the history plot line and current marker.
- **Ink 2** (ink-2): row labels, section titles, chip text, icon plates.
- **Ink 3** (ink-3): units, low and high numbers, captions, subtitles, chevrons, the average tick, quiet states, the time left on the Undo countdown bar.

### The two palettes
- **Surface order:** dark panels lift off the ground and the header lifts again (ground, panel, header from dark to light). Light panels are the brightest paper on a grey desk and the header band sits a step below the panel (ground, header, panel from dark to light).
- **Selection:** the app reads `appearance.theme`: "system" (default) follows the XDG portal's `color-scheme` (2 is light, anything else or no portal is dark) and switches live when the desktop does, so an Omarchy theme switch switches the window; "dark" and "light" force a palette.
- **Per-palette adjustments:** the solid destructive button is red at 73 % with ink text in dark, full red with on-crit text in light. The collector-down banner washes the ground with 14 % red and an ink-3 reason in dark, 8 % red and an ink-2 reason in light. Floating shadows are black at 50 % in dark, 22 % in light.

### Contrast floors (unit tested in `theme.rs`)
- Every ink (ink, ink-2, ink-3, the six device colours, warn, crit, watch) reaches 4.5:1 on every surface (ground, panel, header, alt-row, zebra-row, hover, selected), in both palettes.
- Device tab text reaches 4.5:1 on its own tint over the header and the hover fill, in both palettes; the device colour is nudged toward white (dark tab) or black (light tab) until it does.
- On-crit on crit reaches 4.5:1 in both palettes.
- Text on tints (heavy process cells, the fallback app badge, watched rows, the focus flash at its 45 % start, the kill warning box, the collector-down banner, the destructive button) reaches 4.5:1 in both palettes (the palette tests and `no_painted_pair_is_under_its_target` hold every painted pair to it, and outlines to 3:1).
- The observed range, the marker (ink, warn or crit) and the average tick reach 3:1 against every device's track, in both palettes; this is why the light range alpha is 85 %.
- Ink 2 reaches 3:1 on the 45 % red band of a flag series in the history plot.

### Named Rules
**The Reserved Green Rule.** Watch Green marks values the user chose to watch, and nothing else. Healthy states stay ink-3; no success colour exists.

**The Threshold-Only Rule.** Amber and red appear only for a crossed threshold, an active or recent throttle, a degraded or failed collector, or a destructive action.

**The Device Owns Its Hue Rule.** A legend colour identifies one device everywhere it appears and is never borrowed for another meaning.

**The Same Report Rule.** The light palette is the dark one reprinted: every role, rule, size and alpha that is not listed as a per-palette value stays the same, and a new colour lands in both palettes and both contrast tests at once.

## Typography

**Label Font:** Atkinson Hyperlegible (regular and bold, embedded)
**Result/Mono Font:** AtkynsonMono Nerd Font Mono (regular and bold, embedded; also the icon source)

**Character:** a hyperlegible humanist sans for words and its mono sibling for numbers, so labels read easily and results line up in tabular columns. A setting swaps both for Omarchy's current font; the mono Nerd Font stays in every chain for icons.

### Hierarchy
All sizes derive from the base font size B (setting, 10 to 24, default 14.5). Frontmatter sizes are at the default.
- **Title** (bold, B + 1.5): the app name, the "Collector not running" heading, Settings titles. Dialog titles use 18 px scaled.
- **Result** (mono bold, B + 0.5): current values in rows, summary chips and the popover title value.
- **Label** (regular, B - 0.5): row labels, legend chips, subtitles, buttons, process column headers; bold for watched rows, process group names, the sort column, the edit bar's lead and the popover title.
- **Section / Tab** (bold, B - 1.5): section titles, device tabs, the Processes heading, process class rows.
- **Mono** (mono, B - 1): process table numbers (bold on group and class rows), status bar.
- **Unit / Min-max** (mono, B - 2): units after the result, low and high numbers, popover axis labels, flag detail text.
- **Flag** (mono bold, B - 2): H, HH, L, LL.
- **Small** (regular, B - 2): column captions (now, low, range, high), chip names, status pills, counts, window titles in the process table.

### Named Rules
**The Unit Once Rule.** A row prints its unit once, small and ink-3, after the current value; low and high are bare numbers in the same unit, chosen from the row's largest value.

**The Numbers Are Mono Rule.** Every measured number uses the mono face and right-aligns to a fixed cell edge.

**The Ellipsis Not Clip Rule.** Text that runs out of room ends in an ellipsis (labels, names, window titles, hints) or, for paths, loses its start so the file name stays; it is never clipped mid-glyph.

## Layout

A 4 px unit governs everything: heights are written for the default size, scaled by B / 14.5 and rounded up to 4 px (`snap4`); column widths snap down to 4 px.

- **Window:** top bar (16 px horizontal, 8 px vertical inset) with the name, legend chips, then right-aligned Since, Reset (Undo for 30 s after a reset), Sources chip, Edit layout and Settings, and after a 12 px gap the window controls: Maximize (Restore while maximized) and Close, ghost buttons the gear's size with the Material window glyphs of the icon font, a hover wash like the other buttons (Close washes in the critical colour with its own on-critical text, 4.5:1 or better in both palettes) and the usual focus ring. Double-clicking an empty part of the bar maximizes or restores. Under Hyprland, which reports every tiled window as maximized, the state (and so the glyph and the name) is read from its `j/clients` query on the Hyprland socket. There is no minimize: Hyprland ignores it. The Settings window has none. A narrow bar gives up parts in order before anything can overlap: the Since text, the words of the Sources chip (its dot stays), the legend names (shortened to Mem and Drive), then the names altogether (dots with their state marks; the names stay in the tooltips). Edit layout is a framed button like the others, pressed while editing. The legend ends in the crossings chip when there are any, then the logging chip while the collector logs. Sensor columns fill the area beside the process panel; the panel docks at the bottom or on the right behind a draggable 12 px split handle (64 x 3 px grip, 24 px of hit area). Status bar at the bottom: process count and groups, CPU, GPU and memory on the left, the age of the data (and a toast) on the right. A narrow bar gives up parts of the left end in order before anything can overlap: the group count, the RAM total ("25.3 GiB (41 %)"), then the process count and the RAM size ("Memory 41 %"); a toast that cannot fit beside even that waits for room.
- **Process dock and view:** four arrangements, bottom or right, each compact or expanded, and each keeps its own size, Name width and columns. Defaults: bottom compact 30 % of the height, bottom expanded 65 %, right compact 500 px wide with CPU, GPU and Memory, right expanded 55 % of the width. The bottom split runs 10 % to 85 % of the height. On the right the sensors always keep 480 px and the expanded panel at least 520 px; the compact panel is never narrower than the width at which all its columns show (Name's compact minimum is 120 px, otherwise 200 px), and when the window is too narrow for both the sensors keep theirs.
- **Card layout:** one arrangement of sensor cards serves every dock and view. Automatic columns are 480 to 560 px wide, 12 px apart; explicit widths 280 to 1100 px, shrunk to the window (and grown at most 1.6 x). When the user's columns no longer fit at 420 px each, neighbouring columns stack, whole and in order, into as many display columns as fit, each as wide as its widest member, so the order never changes. Edit mode always shows the real columns one to one and scrolls if they overflow. Cards stack in columns with 12 px between them.
- **Card metrics (default size):** header 36 px, caption row 20 px, row 28 px, section gap half a row floored to 4 px (12 px), 4 px after the last row. Collapsed card summary row 40 px.
- **Row columns, right to left:** 12 px pad, flag cell, 8 px gap, the low / bar / high cluster (cells 8 px apart), 12 px, the result cell (shared right edge per section), 8 px, the label. Labels start 24 px in (38 px in edit mode).
- **Space negotiation:** labels keep their full width first; then labels over 22 characters ellipsize; then cut to 14 characters; only then the bar drops (minimum 72 px, maximum 140 px), then low and high (they stay in the tooltip). The bar returns when room returns.
- **Pinned header band:** while a card scrolls under the top of the view, its header and caption row pin as one opaque band (square corners, 1 px rule underneath) and slide out with the card's bottom edge.
- **Process panel:** toolbar 40 px, column header 28 px, rows 28 px at default (26 snapped), actions row 44 px. The bottom dock never gets shorter than its chrome plus five rows, whatever size was saved or dragged, unless that would leave the sensors under 120 px. In the bottom dock Name is the user's width or the automatic one, capped at 560 px. In the right dock the data columns keep their fitted widths plus the user's extra, and Name takes everything else, so a change of panel width changes Name only; with a Location column in the right expanded view Name stops at 40 % of the panel. Location takes what is left (at least 160 px). In a narrow toolbar the search field shrinks to an icon and opens on a click or "/".
- **Frames:** windows, dialogs and menus use a 16 px inner margin; item spacing 8 x 4 px; button padding 10 x 4 px; interactive height 24 px snapped.

## Elevation & Depth

Flat and tonal. Depth comes from the surface stack (ground, panel, header in dark; ground, header, panel in light) and 1 px rule strokes. Cards carry no shadow.

### Shadow Vocabulary
- **Floating layer** (offset 0 6 px, blur 18 px, the palette's shadow colour: black at 50 % dark, 22 % light): windows, menus, the history popover and dialogs only.

### Named Rules
**The Flat Panel Rule.** Panels sit on the ground with a 1 px rule stroke; only layers that float above the page get the shadow.

## Shapes

Gently rounded and consistent: cards, popovers and the edit bar 8 px, header tops 7 px (inside the 1 px stroke), widgets, tabs, legend chips and the process header 6 px, app icon plates and letter badges 5 px, process row highlights and heavy-cell tints 4 px, range track and zones 3 px. Pills (throttle states, stale, summary chips, Sources chip, "order held") are fully rounded. Dots: 4 px radius for legend and source dots, 3 px for the watched dot. Strokes are 1 px; 1.5 px for state rings and hover outlines.

## Components

### Card and Panel Header
- **Card:** panel fill, 1 px rule stroke, 8 px corners; 1.5 px device-colour stroke while that device's legend chip is hovered. Cards come from the running system, and the device decides the colour: one CPU card (AMD, or Intel through coretemp with a Package row per package), one Soft Violet card with the "GPU" tab per discrete GPU (gpu0 for NVIDIA, gpu1 and on for discrete AMD), one Pale Lilac iGPU card, Memory, the drives and the board; a card the window does not recognise takes Slate.
- **Header:** header fill (hover fill on hover), chevron in ink-3, then the device tab, then the subtitle (model name) in ink-3 Label, ellipsized with the full text on hover. Click collapses the card; right-click offers Collapse and Hide panel.
- **Device tab:** the short name ("CPU", "Memory", "Drive") in Section type, in the device colour, on a fill of the device colour at 22 % over the surface beneath; 22 px tall (scaled), text width plus 16 px, 6 px corners. The text colour steps away from the fill until it reaches 4.5:1.
- **Throttling state (CPU and GPU, right side):** "No throttling" as quiet ink-3 Small text; recent throttling (the last event, counted from its end, within `limits::THROTTLE_RECENT_MINUTES`, 10) as an amber-outlined pill ("Throttled 2x, last 03:00"), and the same words as quiet ink-3 Small text after that; active throttling as a filled red pill with on-crit text ("Throttling now"). Pills use bold B - 2, 8 x 3 px padding.

### Caption Row
One 20 px row under the header: "now", "low", "range", "high" in ink-3 Small, aligned to the first section's result edge, low and high right edges and bar centre. Captions for columns a card has dropped are left out.

### Sensor Rows and the Flag Column
- 28 px rows; odd rows alt-row; hover fill on hover.
- Label ink-2, result ink bold mono with a quiet unit, low and high ink-3 mono (each recoloured amber or red when that value itself crossed a threshold), range bar, flag.
- **Flag column:** H or L (amber) for a warning crossing, HH or LL (red) for critical; below-thresholds use L. A normal row leaves the cell empty. The current result takes the level colour too, so state reads by letter and colour.
- **Boolean rows** (throttling, power limit): result "Yes" in red or "No" in ink, with a right-aligned history note in mono ("never since 02:26" in ink-3, "4x, last 03:10" in amber), shortened before it ellipsizes.
- Missing values print "n/a" in ink-3.

### Watched Rows
Bold green label, green result and green low and high, a 3 px green dot left of the label, and an 8 % green wash over the row. Threshold colours still win on the result when crossed. The treatment survives folding: a collapsed section or card whose summary reading is watched keeps it (see below).

### Collapsed Section and Card Summaries
- **Collapsed section:** the section row keeps the row columns: chevron in the device colour, title in ink-2, the summary reading's label in ink-3, then its result, low, bar, high and flag aligned with the rows below. Without a summary it says "N rows". When the summary reading is watched the row takes the 8 % green wash, a 3 px green dot after the title, the summary label in bold green and green cells; the title keeps ink-2.
- **Collapsed card:** one 40 px line of up to three summary chips (header fill, rule stroke, 24 px tall, fully rounded): section short name in ink-3 Small, result in Result type (level-coloured), unit in ink-3. A watched summary chip adds an 8 % green wash, a 3 px green dot before the name and a green result (a crossed threshold still wins). A chip that does not fit is left out, never clipped.

### Range Bar (signature)
One track per row; flags get none.
- **Track:** 6 px tall, 3 px corners, the panel blended 18 % toward the device colour, so every row carries its device.
- **Zones:** warning zone in Amber Wash at 28 %, critical zone red at 38 %, both running from the threshold to the end of the domain (to the left end for below-thresholds); the critical zone nests inside the warning zone.
- **Observed range:** min to max in the device colour at the palette's range alpha (70 % dark, 85 % light), at least 3 px wide.
- **Average tick:** 1.5 x 10 px, ink-3.
- **Current marker:** 3 x 14 px, 1.5 px corners, ink (amber or red at a warning or critical level), with a 0.5 px panel-coloured outline.
- **Domain:** percent is fixed 0 to 100; temperatures start at the lower of the observed min and 25, floored to a multiple of 5, and end at 1.05 x the larger of the observed max and the critical (else warning) threshold; other units start at 0 and end at 1.05 x the larger of the observed max and the critical threshold. Values outside clamp to the ends.
- **Known capacities:** where the snapshot states one, the track runs from 0 to it (to the observed max if that is larger): total RAM for Used, Available and Cache, the swap size, total VRAM for VRAM used, the GPU's power limit for Power draw. Nothing states a fan's top speed or a drive's top rate, so those keep the observed scale.
- **Constants and unmoved values:** configured memory speed, memory clock and the GPU power limit have no bar. On a scale that follows the observed maximum (no capacity, no thresholds, not percent or temperature) the track stays empty, without range, tick or marker, until the value has moved by 5 % of its largest value.
- **Quiet fill:** an observed range covering 60 % or more of the track is drawn at half the range alpha, so drive activity (0 to the burst peak) stops being the loudest mark; marker, average tick and zones keep full strength.
- Bar geometry is fixed in pixels; only its width follows the layout.

### Legend Chips
Device dot (4 px radius, device colour) and the legend name ("Drives") in ink-2 Label, 28 px tall, 6 px corners; hover fill on hover, and hovering outlines that device's cards. One chip per device present, the iGPU included. A device with a flagged reading gets a 1.5 px ring in the state colour over a 7 % wash of it, and its worst mark (HH or LL, then a thermometer icon for active throttling, then H or L). A device that crossed a threshold within the history window (30 min by default) but is normal now keeps the 1.5 px ring alone, hollow, in the worst level it reached; the wash is the current state, the hollow ring is the past. The tooltip adds the crossing in words. Click jumps to the panel, or to the flagged row with a focus flash.

### Crossings Chip
After the legend, while the collector holds any crossing of a reading on screen: "3 crossings since 05:20" (count and the oldest start) in a fully rounded 28 px chip, with a 4 px dot. Outlined and washed in amber or red (the worst level) while some crossing is recent, plain rule outline and ink-3 dot when they are all older; in a narrow top bar it shortens to the count. A click opens a list under it: one 28 px row per crossing, newest first, with H, HH, L or LL (state colour while recent, ink-3 after), the device dot and "Device, Label", "peak 84.2 °C" (or "low" for below-thresholds), "06:06 to now" or "06:00 to 06:04" with its duration. A row scrolls to its reading, flashes it and pins its history popover. The collector records them from the first sample past a threshold until the value is back inside, merging returns within 30 s, for six hours or 100 entries; Reset drops the ones that are over.

### Logging Chip
After the crossings chip, only while the collector reports that it logs (never with demo data): a fully rounded 28 px chip with a 4 px dot and "Logging" in ink-2, outlined in the rule colour, like an idle crossings chip. When the log cannot be written it is outlined and washed in amber (14 %) and reads "Log error". In a narrow top bar, with the legend names gone, it is the dot alone. The hover says what the collector is doing (the values, the interval and the file, or the error), and a click opens Settings on the Logging tab. It wears the outside focus ring and is a button named "Logging" or "Log error".

### Sources Chip and Status Pills
The Sources chip is quiet while every source answers: a 3 px ink-3 dot and the word "Sources" in ink-3, no outline. When a source fails it becomes an outlined pill in amber (some down) or red (all down) with a 14 % wash, a 4 px dot and "Sources 3 of 4" in ink; per-source detail on hover. A click on it opens Settings on the Collector tab. Counters everywhere read "n of m". "stale" is an amber outlined pill.

### History Popover
Panel fill, 1 px rule, 8 px corners, floating shadow, 12 px inset, 380 px wide and 170 px tall plus, when the reading has them, up to three small lines under the plot (its description in ink-2, its source and its range since the stats began in ink-3: what a tooltip would say, so a row never shows both). Title row: device dot, "Device, Label" in bold Label, current value right. An ink 1.5 px hairline plot owns the field; min and max points marked with 2.5 px dots and mono labels on a panel backing that give the value and the time ("max 85.4 °C 11:35"); warning and critical thresholds as dashed amber and red lines with value labels at the right. No two labels ever overlap and none leaves the plot: they are placed in priority order, critical, warning, highest, lowest. A threshold label sits at the right end, the topmost line's above its line (below it when the plot's top is too near) and the others' below, nudged past a label already there; the highest sits above its point and the lowest below, else at either end of the plot, else beside the point; one that finds no free spot is left out (the note under the plot and the range line still give the peak); the onset of the last crossing as a vertical line; flag series as red bands. Axis: "-30 min" and "now" in ink-3 mono over a rule baseline. A note line under the plot always names the onset: while a crossing lasts, in amber or red, "Above warning since 11:30, peak 84.2 °C at 11:35"; after it, in ink-2, "Last crossed critical: 11:30 to 11:42, peak 106.0 °C at 11:32" (from the collector's record, or from the series' own last crossing without one). Empty state "No history yet". The plot has a text equivalent for a screen reader ("History of CPU, Tctl, last 30 minutes: lowest 58.0 °C at 05:42, highest 84.2 °C at 06:08"). It opens on hover and pins on click (Enter on a focused row pins it, Esc lets go). It sits beside the row (right, then left) or under or over it inside its own card, whichever covers the least of the other cards and the process panel; it never covers the row and stays on screen.

### Process Panel
- **Frame:** panel fill, 1 px rule, 8 px corners. Toolbar: the "Processes" Section heading and search on the left; on the right, icon buttons for expand or collapse (Ctrl+E), dock right, dock bottom (the current dock shows active), then Hide.
- **Column header:** header fill, 6 px corners. Name, PID, Threads, User, Started and Location in ink-3; CPU in blue; GPU, VRAM, Encode and Decode in violet; Memory in cyan; Disk in orchid; idle headers at 95 % strength (4.5:1 for every device colour on the header band), full on hover. The sort column is bold with an arrow, and an "order held" chip (rule outline, pause icon, ink-3 Small) shows while hovering the list holds the order.
- **Column picker:** right-click anywhere on the header opens a "Columns" checklist (Name always on and disabled, then PID, CPU, GPU, VRAM, Memory, Disk, Encode, Decode, Threads, User, Started, Location) that stays open while boxes are ticked. Columns always appear in that fixed order; the same checklist sits in Settings, Processes.
- **Name divider:** a 24 px hit area at Name's right edge (the line stays 1 or 2 px) with a horizontal-resize cursor; while dragging, a 1 px ink-3 guide at 45 % runs down the list. Double-click fits Name to its widest visible name plus 12 px. The divider never resizes the panel. In the bottom dock it sets Name's width; in the right dock it sets how much wider than their fitted widths the data columns are (shared equally), and Name always takes everything else, so widening or narrowing the panel changes Name only and never the data columns. The data columns fit their widest text (or their header) and, to keep values from jittering, grow at once and shrink only after about 3 s of needing less.
- **Rows:** class rows (Apps, Background, System) bold with counts; group rows with a 20 px app icon, bold ink name, process count and window title in ink-3; child rows in ink-2 with rule guides. Names and titles end in an ellipsis at Name's edge, and a count shows only when it fits whole; Location loses its start. Numbers in mono, right-aligned 10 px from the cell edge, ink-3 when idle. A heavy cell gets a 16 % tint of its column's colour: CPU from 90 % of one logical CPU's share of the machine (100 / threads, so 5.6 % on 16 threads), GPU from 25 %, memory from a sixteenth of the total RAM (at least 512 MiB), Disk from 50 MB/s. Class rows never tint. Odd rows zebra-row, hover fill. The selected row takes the selected fill, an ink-3 outline at 55 % and a 3 px full-ink bar at its left edge (the fill alone cannot part far from hover and zebra, because every ink must keep 4.5:1 on it). Actions row: End task, Force kill outlined in red, then the target's name (right-to-left names in visual order, cut at their logical end) in bold ink with its pid or process count. After End task or Force kill the row says how it went ("Ending X...", "X ended", "X is still running") while that row stays selected, for about 10 s; the processes are checked for about 3 s, and when one is still running Force kill turns solid red.

### App Icons and Plates
App icons come from the freedesktop icon theme at 20 px (scaled). Each icon's tone is measured once: one covering more than 60 % of its square, or of mixed brightness, carries its own background; a glyph of mean luminance above 0.8 is light, below 0.2 dark. A light glyph in the light palette, or a dark glyph in the dark palette, sits on an ink-2 plate 2 px larger on each side with 5 px corners, so it never vanishes into a row of its own tone. Without an icon the row shows a first-letter badge (5 px corners): a neutral ink-3 tint at one of three strengths (14, 22 or 30 %) picked by name so neighbouring rows part, the letter in full ink. It borrows no device or warning colour.

### Edit Bar
While editing the layout a bar above the cards, as wide as the window: header fill, 1 px rule, 8 px corners, 12 x 8 px inset. "Editing layout." in bold ink, the instructions in ink-2, then Add column, Reset layout (Undo for 30 s after it acts) and Done. When stacking is active outside editing it adds one ink-2 line: "Your N columns are stacked into M outside editing because the window is narrow" (or "the space beside the process panel is narrow"), "Editing shows all N."

### Buttons and Inputs
egui widgets mapped to the palette: inactive on header fill with a control-colour stroke and ink-2 text, hover on hover fill with an ink-3 stroke and ink text, active on selected. 6 px corners throughout.

### Resets and Settings Window
- **Resets with Undo:** every reset acts on the first click: the top bar's Reset (and Ctrl+R, never while a text field has focus) for low, high, average, throttle history and finished crossings; the edit bar's Reset layout and Settings, Layout's Reset to automatic (one reset, two buttons); Settings, Processes' Reset views; Settings, General's Reset all customisations; Settings, Logging's Stop logging (it brings back the ticked values and Log all hardware readings). The same button then reads "Undo" for 30 s, at the same place and width (sized for the wider of the two words, so nothing beside it moves, and keyboard focus stays on it). A 2 px bar runs along its bottom edge, 7 px in from each side: a rule-coloured track with the time left in ink-3, stepping once a second (a readout, not an animation). The tooltip names what comes back and the seconds left; the screen reader hears a button named "Undo reset". Ctrl+Z undoes the latest reset of any kind (not while a text field has focus). A newer reset of the same kind replaces the older offer and restarts the time. A reset that changes nothing offers no Undo; the top bar's Reset is disabled until the collector has sent data.
- **Reading menu:** the right-click menu opens under the row at the pointer's x (above when there is no room), stays open for clicks inside it, and closes on one-shot actions or a click outside. Its threshold fields are framed inputs (ground fill, ink-3 outline) that take typed values on a click, share one left edge, and its checkboxes keep the control outline. Settings > Sensors adds a threshold to any reading: search, pick, then the warning and critical fields. The window never stores a limit: a change goes to the collector, the only writer of `thresholds.json` in the config directory (`~/.config/hwglance/` by default), and the field holds the typed value until a snapshot shows it. While that file is invalid the threshold editor is paused: the fields are off and a small quiet line says "Paused: thresholds.json has an error". Settings > Alerts ends in a Limits file block: the path in ink-3 mono, an Open file button, an ink-2 line saying edits apply within a few seconds, and, when the file has an error, "The file has an error" in bold critical red, the error itself in red, and an ink-2 line that the last good limits stay in force.
- **Settings window:** it opens with its right edge on the gear's and its top just under it, keeps one width on every tab, closes on Esc (unless a field is being edited or a menu is open) and has a 24 px close button in place of the stock title bar.

### Settings: Theme
Settings, General, Theme: three radios, "Follow system", "Dark" and "Light", saved as `appearance.theme`, with an ink-3 note that Follow system tracks the desktop's light or dark setting.

### Keyboard, Focus and Names
- **Focus ring:** every custom-painted control that takes the keyboard wears a 2 px ring in ink (`ink` of the active palette), 1 px off the control, in both palettes. Controls with room around them (legend and Sources chips) wear it outside; headers, rows and table cells that touch their neighbours or a scroll area's edge wear it inside (the same 2 px, 1 px in). Egui's own buttons, fields and checkboxes keep egui's focus look. A mouse press anywhere gives the focus up, so the ring is only ever the keyboard's.
- **Tab order, default layout, at most 30 stops (tested; 148 before):** the legend chips (and the crossings and logging chips), Reset, Sources, Edit layout, Settings, Maximize, Close (left to right: those six are laid out in reading order, so Tab follows what the eye sees), then one stop per card header, then the process panel: search, expand, dock below, dock right, Hide, the column header, the Name divider, the list and the panel divider (made after the list so it wins the pixels at its edge). A reading row, a section head, a header cell and a process row are never Tab stops of their own.
- **Dividers have a keyboard path (WCAG 2.5.7):** the panel divider, the Name divider and, while editing the layout, each column edge are Tab stops named "Resize ..." that say their size. The arrow keys along their axis move them 4 px (Shift: 32 px), Home restores the default (the view's own size, the automatic Name width, a new column's width). In Edit layout the menu of a panel (right-click or Shift+F10 on its header) offers Move to column N, Move up and Move down; the menus of a section and a row offer Move up and Move down.
- **Cards:** the card's one stop is its header until the arrow keys move it (a roving stop: Tab comes back to the row it was left on). Down and Up walk the header, each section head and, while the section is open, its rows; Home and End go to the ends; Left and Right fold the card or section (Left on a row goes up to its section). Enter or Space does what a click does: fold a card or section, pin a row's history (Esc lets go and keeps the focus). Shift+F10 opens the focused thing's menu: card, section or value menu, under the row near its left edge; Esc closes it and the focus is back where it was.
- **Process table:** the column header is one stop (Left and Right choose a column, a ring on it; Enter or Space sorts; Shift+F10 opens the column checklist) and the list is one stop (Up, Down, PageUp, PageDown, Home, End move the selection; Left and Right fold a group; Enter folds or unfolds it; Delete and Shift+Delete end the task; Shift+F10 opens the selected process's menu; arriving with nothing selected selects the first row). The ring is on the selected row. The process keys act only when nothing, or the list, has the keyboard, and never while a menu is open: Delete on a focused button does not end a task.
- **Menu key:** egui-winit 0.36 drops the Menu key before egui sees it (no mapping), so Shift+F10 is the one keyboard path to a context menu.
- **Names:** every interactive painted control is published through one helper (`a11y::publish`: a role, a name, a description and a state) and its words come from one place (`a11y.rs`). A card header is a button named "CPU, <model name>, No throttling" that is expanded or collapsed; a section head the same with its title (and, collapsed, its summary reading); a reading row a list item "CPU, Tctl, 53.6 degrees, warning above 80" (a reading past a limit adds "warning level" or "critical level" before the limits, a watched one ends with "watched", its description is what the tooltip says); a process row "Brave, 10 processes, CPU 3 percent, memory 2.5 GiB", selected, and expanded or collapsed when it folds; a header cell a column header with its sort direction; a legend chip, the Sources chip, the crossings chip and the logging chip buttons named by what they open; a divider a splitter ("Resize the process panel", "Resize the Name column", "Resize column 2"); a grip a button ("Move the section Temperatures"); a reset button offering its Undo a button "Undo reset" described by what comes back. Icon-only buttons are named by purpose ("Settings", "Dock the process panel at the right"), never by their glyph. The tree is built only while an assistive technology is connected, so the names cost nothing otherwise.

### Hit Areas
Anything that can be clicked or dragged has a hit area of at least 24 px in its small dimension (`MIN_HIT`), while the visual stays as thin as it was: interactive height and every list row, header and legend chip never fall under 24 px whatever the type size (`hit_h`); the bar widget list buttons, Remove column and the layout grips are 24 px squares or wider; the split handles are 12 px of layout with 24 px of hit area (the extra over the panel's own padding, never over a scroll bar), the Name divider and the column resize handle 24 px wide around a 1 to 3 px line; the Settings window's edges and corners grab 12 px each side; text fields are 24 px tall. A drag counts from where the pointer was pressed, not from where egui's few pixels of slop ended. Exception: egui's own scroll bars stay 8 px (egui gives a bar one width for look and hit alike); the wheel and the keyboard scroll.

### Dialogs
The kill confirmation is a modal on the popup frame (16 px margin, 440 px wide): bold 18 px title ("End task?" / "Force kill?"), the target and process count, the consequence, an optional red-tinted warning box (18 % red fill, 1 px red stroke, 6 px corners), then Cancel and a solid red confirm button (per-palette fill and text, see Colors). It opens just above the process panel's action row, at its left end (centred when the panel is hidden), over a scrim of 100 black alpha in light and 185 in dark. The bar widget's popup uses the same wording: buttons "End task" and "Force kill", "End task asks the process to exit, so it can save data and clean up." and "Force kill stops the process at once. It cannot save data or clean up." Every kill dialog, force or critical, puts focus on Cancel, so Enter cancels; confirming takes a click, or Tab then Enter.

### Collector States
- **Connecting / waiting for the first snapshot:** a skeleton of the saved layout (cards with header bands, device tabs at 35 % of the device colour, rule-coloured placeholder bars) beside a skeleton of the process panel in its dock (search outline, header band, zebra rows of bars, two outlined buttons), under a centred 360 x 56 px panel card with a bold ink-2 message.
- **Unreadable settings:** a one-time banner under the top bar (amber wash at 14 %, ink text, a bold "Settings reset" lead, a Dismiss button) says the settings file could not be read and names the copy kept as ui.json.bad, in the future tense until the first save makes it. Dismissed, it is gone for the session.
- **Not running:** a centred 460 x 124 px card (taller by the lines of an error) with a red Title heading, the reason in ink-2, a failed Start collector's own message ("Could not start the collector: ...", the first line systemctl printed) in red, and a Start collector button. With data on screen, a red-tinted banner (per-palette wash) says the same. A failed restart from Settings raises the same message as a toast.
- **Version mismatch:** a live collector whose protocol version differs from the window's gets an amber banner (14 % wash, ink text) under the top bar: "Collector is older" with the reason, `systemctl --user restart hwglanced` and a Restart collector button, or "Collector is newer" with "Reinstall HW Glance and restart this window".
- **Invalid limits file:** an amber banner (14 % wash, ink text) under the top bar: a bold "Limits file has an error" lead, the file and the position of the error ("thresholds.json, line 3, column 14: ..."), a line saying the last good limits stay in force and the file is left as it is, and an Open file button. It stays until the file is valid; a toast says it once when it appears and once when it is valid again.
- **Stale data:** nothing is dimmed (red keeps its contrast). The amber "stale" pill says it, and the status bar reads "as of 12:31, 47 s ago" in amber instead of "updated 0.8 s ago".
- **A source that fails:** its card gets a note under the last section, an amber 4 px dot and Small ink-2 text: "GPU readings unavailable: <reason>", LACT's own sentence, "Package power unavailable: ..." on the CPU, "Memory speed unavailable: ..." on Memory. NVML and LACT notes go only to the NVIDIA card (gpu0); discrete AMD cards (gpu1 and on) and the iGPU carry none. When NVML's failure leaves no NVIDIA card at all, an empty one (tab, "unavailable", the note) stands in its place.

### Icons
Material Design icons from the embedded mono Nerd Font (cog, eye, chevrons, thermometer, pause, pin, bell, pencil, dock bottom, dock right, expand, collapse and others), coloured with ink or state colours like text.

### Motion
**The One Flash Rule.** The only authored animation is the focus flash: when a row is focused (from a notification or a flagged legend chip) the window scrolls it to centre and washes it green, starting at 45 % and easing out exponentially over 1.2 s. Nothing else pulses, glows or animates; the Undo countdown bar steps once a second as a readout and does not glide.

### Scoped Exception: Omarchy Bar Widget
The Quickshell widget (`omarchy-plugin/raed.hwglance/`) follows the Omarchy shell design, not this world: it inherits the shell's Style tokens (corner radius, fills, font sizes, font family) and the active Omarchy theme's `colors.toml`, and it ignores the window's light and dark palettes.
- **Bar button:** by default the static icon. With `tray.bar_display` set to CPU, GPU or both, it shows live text ("CPU 12%  GPU 41%") in the clock's label style (body font, bar foreground, 8.75 px margins), at a fixed width sized for the widest text so neighbouring widgets never move, updated about every 2 s. A vertical bar, or a collector that is not running, shows the icon.
- **Popup:** device cards take their accent from the Omarchy palette: CPU blue, GPU magenta, Memory cyan; warning is the theme's yellow and critical its red (urgent).
Rules in this file other than the reserved meanings of warning and critical do not apply to it.

## Do's and Don'ts

### Do:
- **Do** derive every height from the base size and snap it to 4 px; snap column widths to 4 px.
- **Do** give every non-flag sensor row a range bar with track, zones, observed range, average tick and marker, and keep its domain rules.
- **Do** keep numbers in the mono face, right-aligned to a shared section edge, with the unit printed once.
- **Do** give every threshold state a letter (H, HH, L, LL) or a worded pill as well as its colour.
- **Do** keep the device colour on the tab, track, range, legend dot and process column for that device.
- **Do** keep labels whole before giving up the bar, and drop columns before overlapping or clipping text.
- **Do** add every new colour to both palettes and keep every ink at 4.5:1 or better on every surface in both, extending the contrast tests in the same change.
- **Do** paint from the active palette's tokens (warn for text, warn-fill for washes, on-crit on solid red) rather than literal colours, so the light palette follows.
- **Do** stack card columns whole and in order when space runs short, and say so in the edit bar.
- **Do** give every painted control a name, a role and a state through `a11y::publish`, a focus ring when it can take the keyboard, and a 24 px hit area.
- **Do** keep one Tab stop per region (a card, the table header, the list) and move inside it with the arrow keys.
- **Do** make a reset act on one click and turn its own button into Undo for 30 s with the 2 px countdown bar; offer no Undo when nothing changed.

### Don't:
- **Don't** use Watch Green for anything but watched values; healthy and success states stay ink-3.
- **Don't** use amber or red for anything but thresholds, throttling, collector health and destructive actions.
- **Don't** borrow a device legend colour for another meaning.
- **Don't** add gauges, glow, gradients or extra animation; the focus flash is the only motion.
- **Don't** put a confirmation step in front of a reset; Undo is the safety net (confirmation stays for ending processes).
- **Don't** shadow cards; the soft shadow belongs to floating layers only.
- **Don't** let a state move or resize a row; states recolour in place.
- **Don't** show history on the page; it lives in the hover popover.
- **Don't** make a row, a cell or a divider a Tab stop, or name a control by its glyph.
- **Don't** apply this window's palettes to the Omarchy bar widget, or Omarchy's colour theme to the window; the window takes only the desktop's light or dark setting.
