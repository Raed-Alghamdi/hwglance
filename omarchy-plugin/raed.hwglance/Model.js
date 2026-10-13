.pragma library

// Pure helpers for the HW Glance bar widget: settings parsing, threshold
// levels, value formatting, process grouping and the kill plan. No QML types
// are used here so the logic can be exercised outside the shell.

var DEFAULT_POPUP_VALUES = [
  "cpu/usage/total", "cpu/temp/tctl", "cpu/power/package", "cpu/clock/max",
  "gpu0/load/core", "gpu0/temp/core", "gpu0/temp/hotspot", "gpu0/mem/used", "gpu0/power/draw",
  "memory/usage/pct", "memory/usage/used", "memory/temp/summary"
]

// What the bar button shows: the static icon, or live usage of the CPU, the
// GPU or both (tray.bar_display in ui.json).
var BAR_DISPLAYS = ["icon", "cpu", "gpu", "cpu_gpu"]

// A summary older than this is not live: the collector has stopped.
var SUMMARY_MAX_AGE_MS = 10000

// ---------------------------------------------------------------------------
// ui.json
// ---------------------------------------------------------------------------

function stringList(value, fallback) {
  if (!Array.isArray(value)) return fallback.slice()
  var out = []
  for (var i = 0; i < value.length; i++)
    if (typeof value[i] === "string" && value[i] !== "") out.push(value[i])
  return out
}

// Turns the text of ui.json into a normalised config. Anything missing or of
// the wrong type falls back to its default, and invalid JSON gives all defaults.
function parseUi(text) {
  var json = {}
  try {
    var parsed = JSON.parse(String(text || "{}"))
    if (parsed && typeof parsed === "object") json = parsed
  } catch (e) {
    json = {}
  }
  var tray = (json.tray && typeof json.tray === "object") ? json.tray : {}

  var count = Math.round(Number(tray.popup_process_count))
  if (!isFinite(count)) count = 8
  count = Math.max(1, Math.min(30, count))

  var sort = String(tray.popup_sort || "cpu").toLowerCase()
  if (sort === "memory") sort = "mem"
  if (sort !== "cpu" && sort !== "gpu" && sort !== "mem") sort = "cpu"

  var display = String(tray.bar_display || "icon").toLowerCase()
  if (BAR_DISPLAYS.indexOf(display) < 0) display = "icon"

  return {
    popupValues: stringList(tray.popup_values, DEFAULT_POPUP_VALUES),
    processCount: count,
    sort: sort,
    barDisplay: display
  }
}

// ---------------------------------------------------------------------------
// Bar button (summary.json)
// ---------------------------------------------------------------------------

function finiteOrNull(value) {
  if (value === null || value === undefined || value === "") return null
  var n = Number(value)
  return isFinite(n) ? n : null
}

// Reads the collector's summary.json. `fresh` is false when the file is missing,
// unreadable or older than SUMMARY_MAX_AGE_MS (the clock may be a little off in
// either direction), and then no values are returned.
function parseSummary(text, nowMs) {
  var stale = { fresh: false, cpu: null, gpu: null }
  var json
  try {
    json = JSON.parse(String(text || ""))
  } catch (e) {
    return stale
  }
  if (!json || typeof json !== "object") return stale
  var stamp = finiteOrNull(json.unix_ms)
  if (stamp === null || Math.abs(Number(nowMs) - stamp) > SUMMARY_MAX_AGE_MS) return stale
  return { fresh: true, cpu: finiteOrNull(json.cpu_usage_pct), gpu: finiteOrNull(json.gpu_usage_pct) }
}

function wholePercent(value) {
  return Math.max(0, Math.min(100, Math.round(value)))
}

// The values the mode asks for, as { label, pct } in bar order. A value the
// summary has none of (no GPU, say) is left out. Icon mode and stale data give none.
function barParts(mode, summary) {
  var parts = []
  if (!summary || !summary.fresh) return parts
  if ((mode === "cpu" || mode === "cpu_gpu") && summary.cpu !== null)
    parts.push({ label: "CPU", pct: wholePercent(summary.cpu) })
  if ((mode === "gpu" || mode === "cpu_gpu") && summary.gpu !== null)
    parts.push({ label: "GPU", pct: wholePercent(summary.gpu) })
  return parts
}

// "CPU 12%  GPU 41%". The number is padded to two places so a single digit does
// not move the rest; the bar font is monospace, so every digit has the same width.
function barText(parts) {
  var out = []
  for (var i = 0; i < parts.length; i++) {
    var num = String(parts[i].pct)
    if (num.length < 2) num = " " + num
    out.push(parts[i].label + " " + num + "%")
  }
  return out.join("  ")
}

// The widest text the mode can show, used to size the button once.
function barSample(mode) {
  var parts = []
  if (mode === "cpu" || mode === "cpu_gpu") parts.push({ label: "CPU", pct: 100 })
  if (mode === "gpu" || mode === "cpu_gpu") parts.push({ label: "GPU", pct: 100 })
  return barText(parts)
}

// "HW Glance", then the values ("CPU 12 %, GPU 41 %"). In a value mode with no
// live summary it says the collector is not running.
function barTooltip(mode, summary) {
  if (mode === "icon") return "HW Glance"
  if (!summary || !summary.fresh) return "HW Glance\nThe collector is not running"
  var parts = barParts(mode, summary)
  var items = []
  for (var i = 0; i < parts.length; i++) items.push(parts[i].label + " " + parts[i].pct + " %")
  return items.length > 0 ? "HW Glance\n" + items.join(", ") : "HW Glance"
}

// Ids the widget needs from each snapshot: the card values plus the thermal
// throttle flag of every card.
function wantedSet(config) {
  var set = {}
  var cards = buildCards(config.popupValues)
  for (var c = 0; c < cards.length; c++) {
    set[cards[c].key + "/throttle/thermal"] = true
    for (var i = 0; i < cards[c].ids.length; i++) set[cards[c].ids[i]] = true
  }
  return set
}

// ---------------------------------------------------------------------------
// Readings
// ---------------------------------------------------------------------------

// Short tag for a section title, used when two wanted readings would share a label
// (GPU "Core" exists under Usage, Temperatures and Clocks).
function sectionTag(title) {
  var t = String(title || "").toLowerCase()
  if (t.indexOf("temp") === 0) return "temp"
  if (t.indexOf("clock") === 0) return "clock"
  if (t.indexOf("power") === 0) return "power"
  return t
}

function pickReading(group, section, reading, wanted, out, seen, limits) {
  if (!wanted[reading.id] || out[reading.id]) return
  var base = String(group.title || "") + " " + String(reading.label || "")
  seen[base] = (seen[base] || 0) + 1
  out[reading.id] = {
    id: reading.id,
    group: String(group.title || ""),
    baseLabel: base,
    tag: sectionTag(section.title),
    label: base,
    plain: String(reading.label || ""),
    unit: reading.unit,
    value: (reading.value === null || reading.value === undefined) ? null : Number(reading.value),
    desc: reading.desc || "",
    note: reading.note || "",
    warn: (reading.warn === null || reading.warn === undefined) ? null : Number(reading.warn),
    crit: (reading.crit === null || reading.crit === undefined) ? null : Number(reading.crit),
    // "at or below" limits (fans, pumps); only the collector's limit_overrides say so
    below: !!(limits && limits[reading.id] && limits[reading.id].below === true),
    events: reading.events || null
  }
}

// Picks the wanted readings out of the snapshot's sensor groups, looking at
// each section's readings and at its collapsed-view summary. `limits` is the
// snapshot's limit_overrides (the readings' warn and crit already are the
// limits in force). The label is
// "<group title> <reading label>", for example "CPU Tctl"; labels that would
// repeat get their section as a suffix, for example "GPU Core temp" and
// "GPU Core load".
function extractReadings(sensors, wanted, limits) {
  var out = {}
  if (!Array.isArray(sensors)) return out
  var seen = {}
  for (var g = 0; g < sensors.length; g++) {
    var group = sensors[g]
    var sections = group.sections || []
    for (var s = 0; s < sections.length; s++) {
      var readings = sections[s].readings || []
      for (var r = 0; r < readings.length; r++) pickReading(group, sections[s], readings[r], wanted, out, seen, limits)
      if (sections[s].summary) pickReading(group, sections[s], sections[s].summary, wanted, out, seen, limits)
    }
  }
  for (var id in out) {
    if (seen[out[id].baseLabel] > 1 && out[id].tag) out[id].label = out[id].baseLabel + " " + out[id].tag
  }
  return out
}

// ---------------------------------------------------------------------------
// Cards
// ---------------------------------------------------------------------------

var SHORT_LABELS = {
  "cpu/usage/total": "Usage",
  "cpu/clock/max": "Highest clock",
  "cpu/clock/avg": "Average clock",
  "cpu/power/package": "Package",
  "gpu0/load/core": "Usage",
  "gpu0/temp/core": "Core",
  "gpu0/mem/used": "VRAM",
  "gpu0/power/draw": "Power",
  "gpu0/clock/core": "Clock",
  "memory/usage/pct": "Usage",
  "memory/usage/used": "Used",
  "memory/temp/summary": "Hottest DIMM"
}

function shortLabel(id, reading) {
  if (SHORT_LABELS.hasOwnProperty(id)) return SHORT_LABELS[id]
  if (reading && reading.plain) return reading.plain
  return fallbackLabel(id)
}

function deviceTitle(key, reading) {
  if (key === "cpu") return "CPU"
  if (key === "memory") return "Memory"
  if (/^gpu[0-9]+$/.test(key)) return key === "gpu0" ? "GPU" : "GPU " + key.substring(3)
  if (reading && reading.group) return reading.group
  return key
}

// Palette key for the device accent.
function deviceColorKey(key) {
  if (key === "cpu") return "blue"
  if (/^gpu[0-9]+$/.test(key)) return "magenta"
  if (key === "memory") return "cyan"
  return ""
}

// Groups the flat popup_values list by first path segment, in list order.
// The first id of each group is its big value. At most four more follow.
function buildCards(values) {
  var cards = []
  var byKey = {}
  for (var i = 0; i < values.length; i++) {
    var key = String(values[i]).split("/")[0]
    if (!byKey.hasOwnProperty(key)) {
      byKey[key] = { key: key, ids: [] }
      cards.push(byKey[key])
    }
    var ids = byKey[key].ids
    if (ids.length < 5 && ids.indexOf(values[i]) < 0) ids.push(values[i])
  }
  return cards
}

function bigIds(cards) {
  var out = []
  for (var i = 0; i < cards.length; i++) out.push(cards[i].ids[0])
  return out
}

function firstSentence(text) {
  var t = String(text || "")
  var dot = t.indexOf(". ")
  if (dot > 0) t = t.substring(0, dot + 1)
  return t.length > 90 ? t.substring(0, 87) + "..." : t
}

// Hover text for a card: every value with its full label, plus a short
// description of each reading. It also says in words when a value is past
// its warning or critical level.
function cardTooltip(card, readings) {
  var lines = []
  for (var i = 0; i < card.ids.length; i++) {
    var r = readings[card.ids[i]]
    var label = r ? r.label : fallbackLabel(card.ids[i])
    var line = label + ": " + (r ? formatFull(r.value, r.unit) : "--")
    var words = r ? levelWords(r) : ""
    lines.push(words ? line + " (" + words + ")" : line)
    if (r && r.events && r.events.count > 0)
      lines.push("  switched on " + r.events.count + (r.events.count === 1 ? " time" : " times"))
    var d = r ? firstSentence(r.desc || r.note) : ""
    if (d) lines.push("  " + d)
  }
  return lines.join("\n")
}

function fallbackLabel(id) {
  var parts = String(id).split("/")
  return parts.slice(-2).join(" ")
}

function isNumber(v) {
  return v !== null && v !== undefined && isFinite(v)
}

// Thresholds in force: the reading's own warn and crit, which the collector
// fills from the defaults or from the user's thresholds.json (as a whole, never
// merged), and whether they are "at or below" limits.
function thresholdsFor(reading) {
  var warn = reading ? reading.warn : null
  var crit = reading ? reading.crit : null
  return { warn: isNumber(warn) ? Number(warn) : null, crit: isNumber(crit) ? Number(crit) : null, below: !!(reading && reading.below === true) }
}

// 0 normal, 1 warm, 2 hot.
function levelFor(value, t) {
  if (!isNumber(value)) return 0
  if (t.below) {
    if (t.crit !== null && value <= t.crit) return 2
    if (t.warn !== null && value <= t.warn) return 1
    return 0
  }
  if (t.crit !== null && value >= t.crit) return 2
  if (t.warn !== null && value >= t.warn) return 1
  return 0
}

function readingLevel(reading) {
  if (!reading) return 0
  return levelFor(reading.value, thresholdsFor(reading))
}

// The same marks the window shows next to a value past a threshold, so the
// state does not rest on colour alone: "H" and "HH" for a high value, "L" and
// "LL" for a low one ("below" thresholds).
function levelMark(level, below) {
  if (level >= 2) return below ? "LL" : "HH"
  if (level === 1) return below ? "L" : "H"
  return ""
}

function readingMark(reading) {
  if (!reading) return ""
  var t = thresholdsFor(reading)
  return levelMark(levelFor(reading.value, t), t.below)
}

// "above its critical level" and so on, or "" when the value is normal.
function levelWords(reading) {
  if (!reading) return ""
  var t = thresholdsFor(reading)
  var level = levelFor(reading.value, t)
  if (level === 0) return ""
  return (t.below ? "below its " : "above its ") + (level >= 2 ? "critical" : "warning") + " level"
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

function trimDecimals(v) {
  return v >= 10 ? String(Math.round(v)) : v.toFixed(1)
}

// "16G", "5.4G", "512M", "300K". Binary units, one letter.
function compactBytes(v) {
  v = Number(v)
  if (!isFinite(v) || v < 0) return "--"
  var g = v / 1073741824
  if (g >= 1) return trimDecimals(g) + "G"
  var m = v / 1048576
  if (m >= 1) return trimDecimals(m) + "M"
  return Math.round(v / 1024) + "K"
}

// Decimal units like disk tools: "1.2M/s".
function compactRate(v) {
  v = Number(v)
  if (!isFinite(v) || v < 0) return "--"
  if (v >= 1e9) return trimDecimals(v / 1e9) + "G/s"
  if (v >= 1e6) return trimDecimals(v / 1e6) + "M/s"
  if (v >= 1e3) return Math.round(v / 1e3) + "K/s"
  return Math.round(v) + "B/s"
}

// Short form for the bar: "62°", "26%", "60W", "16G".
function formatCompact(value, unit) {
  if (!isNumber(value)) return "--"
  var v = Number(value)
  switch (unit) {
    case "celsius": return Math.round(v) + "°"
    case "percent": return Math.round(v) + "%"
    case "watt": return Math.round(v) + "W"
    case "volt": return v.toFixed(2) + "V"
    case "mhz": return v >= 1000 ? (v / 1000).toFixed(1) + "GHz" : Math.round(v) + "MHz"
    case "rpm": return Math.round(v) + "rpm"
    case "bytes": return compactBytes(v)
    case "bytes_per_sec": return compactRate(v)
    case "mts": return Math.round(v) + "MT/s"
    case "flag": return v >= 0.5 ? "Yes" : "No"
  }
  return String(Math.round(v * 10) / 10)
}

// Full form for tooltips and the popup: "62.0 °C", "17.3 GiB".
function formatFull(value, unit) {
  if (!isNumber(value)) return "--"
  var v = Number(value)
  switch (unit) {
    case "celsius": return v.toFixed(1) + " °C"
    case "percent": return v.toFixed(1) + " %"
    case "watt": return v.toFixed(1) + " W"
    case "volt": return v.toFixed(2) + " V"
    case "mhz": return Math.round(v) + " MHz"
    case "rpm": return Math.round(v) + " RPM"
    case "bytes":
      if (v >= 1073741824) return (v / 1073741824).toFixed(1) + " GiB"
      return Math.round(v / 1048576) + " MiB"
    case "bytes_per_sec":
      if (v >= 1e9) return (v / 1e9).toFixed(1) + " GB/s"
      if (v >= 1e6) return (v / 1e6).toFixed(1) + " MB/s"
      if (v >= 1e3) return Math.round(v / 1e3) + " KB/s"
      return Math.round(v) + " B/s"
    case "mts": return Math.round(v) + " MT/s"
    case "flag": return v >= 0.5 ? "Yes" : "No"
  }
  return String(Math.round(v * 10) / 10)
}

function percentText(v) {
  if (!isNumber(v)) return "-"
  v = Number(v)
  return (v < 10 ? v.toFixed(1) : String(Math.round(v))) + "%"
}

function pluralProcesses(n) {
  return n + (n === 1 ? " process" : " processes")
}

// ---------------------------------------------------------------------------
// Label colours
// ---------------------------------------------------------------------------

// The popup draws its text in real colours, not in the foreground at reduced
// opacity (which loses contrast on many themes). These helpers pick, from the
// theme's own palette, the variant that reaches MIN_CONTRAST (WCAG AA for text)
// on the popup surface. Colours are passed as "#rrggbb" strings (or a QML
// colour) and come back as "#rrggbb"; internally they are [r, g, b] in 0..255.

var MIN_CONTRAST = 4.5

// "#rgb", "#rrggbb" or "#aarrggbb" (QML's string form), or an object with r, g
// and b in 0..1 (a QML colour). Null for anything else. Alpha is ignored.
function parseColor(value) {
  if (value && typeof value === "object") {
    if (!isNumber(value.r) || !isNumber(value.g) || !isNumber(value.b)) return null
    return [Math.round(value.r * 255), Math.round(value.g * 255), Math.round(value.b * 255)]
  }
  var t = String(value === undefined || value === null ? "" : value).replace(/^\s+|\s+$/g, "")
  var m = t.match(/^#([0-9A-Fa-f]{3})$/)
  if (m) return [0, 1, 2].map(function(i) { return parseInt(m[1].charAt(i) + m[1].charAt(i), 16) })
  m = t.match(/^#(?:[0-9A-Fa-f]{2})?([0-9A-Fa-f]{6})$/)
  if (!m) return null
  return [0, 2, 4].map(function(i) { return parseInt(m[1].substr(i, 2), 16) })
}

function colorHex(rgb) {
  return "#" + rgb.map(function(c) {
    var h = Math.max(0, Math.min(255, Math.round(c))).toString(16)
    return h.length < 2 ? "0" + h : h
  }).join("")
}

function luminance(rgb) {
  var lin = rgb.map(function(c) {
    var v = c / 255
    return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4)
  })
  return 0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2]
}

function contrastOf(a, b) {
  var la = luminance(a)
  var lb = luminance(b)
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05)
}

// WCAG contrast ratio of two colours given as strings.
function contrast(a, b) {
  var ca = parseColor(a)
  var cb = parseColor(b)
  return (ca && cb) ? contrastOf(ca, cb) : 0
}

// `t` of the way from a to b.
function mixColors(a, b, t) {
  return [0, 1, 2].map(function(i) { return a[i] + (b[i] - a[i]) * t })
}

function worstContrast(rgb, surfaces) {
  var worst = Infinity
  for (var i = 0; i < surfaces.length; i++) worst = Math.min(worst, contrastOf(rgb, surfaces[i]))
  return worst
}

// Surfaces text sits on: the popup background and the faint foreground-tinted
// fill of cards and hovered rows (`fillAlpha`, 0.08 at the shell's defaults, the
// strongest fill a label meets).
function labelSurfaces(ink, bg, fillAlpha) {
  var a = Math.max(0, Math.min(1, Number(fillAlpha) || 0))
  return [bg, mixColors(bg, ink, a)]
}

// The first candidate that reaches `min` on every surface (and, for a colour
// drawn on its own tint such as a tag, on that tint over the background too).
// When none does, the first candidate blended toward `ink` in 5 % steps until
// one does; `ink` itself when even that falls short.
function readableColor(candidates, ink, bg, surfaces, min, tintAlpha) {
  function ok(c) {
    var all = surfaces
    if (tintAlpha > 0) all = surfaces.concat([mixColors(bg, c, tintAlpha)])
    return worstContrast(c, all) >= min
  }
  var list = candidates.map(parseColor).filter(function(c) { return c !== null })
  for (var i = 0; i < list.length; i++) if (ok(list[i])) return list[i]
  if (list.length === 0) return ink
  for (var step = 1; step <= 20; step++) {
    var blended = mixColors(list[0], ink, step * 0.05)
    if (ok(blended)) return blended
  }
  return ink
}

// The muted label colour: a muted token of the theme when it is readable and
// no stronger than the ink, otherwise the ink pulled toward the background by
// as much as still keeps `min` (at most 35 %).
function mutedColor(tokens, ink, bg, surfaces, min) {
  var inkRatio = worstContrast(ink, surfaces)
  for (var i = 0; i < tokens.length; i++) {
    var c = parseColor(tokens[i])
    if (c === null) continue
    var ratio = worstContrast(c, surfaces)
    if (ratio >= min && ratio <= inkRatio) return c
  }
  for (var step = 7; step >= 1; step--) {
    var dimmed = mixColors(ink, bg, step * 0.05)
    if (worstContrast(dimmed, surfaces) >= min) return dimmed
  }
  return ink
}

// Text colours for the popup, from a theme's colour table (key -> "#rrggbb",
// see parseColorsToml), the popup's ink and background and its strongest fill
// alpha. Every value is a "#rrggbb" string:
//   ink              the popup text colour, as given
//   muted            secondary labels and captions
//   warn, crit       warning and critical text (yellow, red or a variant)
//   blue, magenta, cyan   device accents used as text
// A key missing from the table falls back to the next candidate; unusable
// input gives the plain colours rather than an error.
function labelColors(colors, ink, bg, fillAlpha) {
  var c = colors || {}
  var inkRgb = parseColor(ink) || [202, 204, 204]
  var bgRgb = parseColor(bg) || [16, 19, 21]
  var surfaces = labelSurfaces(inkRgb, bgRgb, fillAlpha)
  function pick(base, bright, extra, tint) {
    return colorHex(readableColor([c[base], c[bright]].concat(extra || []), inkRgb, bgRgb, surfaces, MIN_CONTRAST, tint || 0))
  }
  return {
    ink: colorHex(inkRgb),
    muted: colorHex(mutedColor([c.dark_foreground, c.muted], inkRgb, bgRgb, surfaces, MIN_CONTRAST)),
    // The Throttling tag draws critical text on a 16 % tint of itself.
    crit: pick("red", "bright_red", [], 0.16),
    warn: pick("yellow", "bright_yellow", [c.orange]),
    blue: pick("blue", "bright_blue"),
    magenta: pick("magenta", "bright_magenta"),
    cyan: pick("cyan", "bright_cyan")
  }
}

// Reads the "key = "#rrggbb"" lines of a theme's colors.toml.
function parseColorsToml(text) {
  var out = {}
  var lines = String(text || "").split("\n")
  for (var i = 0; i < lines.length; i++) {
    var m = lines[i].match(/^\s*([A-Za-z0-9_-]+)\s*=\s*["']?(#[0-9A-Fa-f]{6})/)
    if (m) out[m[1]] = m[2]
  }
  return out
}

// ---------------------------------------------------------------------------
// Processes
// ---------------------------------------------------------------------------

// Which processes are critical is the collector's call (protocol 2 puts the
// kind of damage in each process's `critical` field). This list is only the
// fallback for an older collector (protocol 1) that sends no such field.
// Matching is on the process name (comm) or the basename of the first command
// line word, case-insensitive.
var LEGACY_CRITICAL = {
  "hyprland": "session",
  "start-hyprland": "session",
  "systemd": "system",
  "xwayland": "xwayland",
  "quickshell": "shell",
  "omarchy-shell": "shell",
  "pipewire": "audio",
  "pipewire-pulse": "audio",
  "wireplumber": "audio",
  "hwglanced": "collector"
}

// Plain warnings, worded like the window's.
var CRITICAL_TEXT = {
  session: "This ends your desktop session: every window and unsaved work in them is lost.",
  system: "systemd runs your user services and the system itself; ending it can log you out or destabilise the machine.",
  xwayland: "Xwayland hosts every X11 app, so they will all close.",
  shell: "This removes your bar and shell until it is restarted.",
  audio: "This cuts all audio and screen sharing until it restarts.",
  collector: "This stops the HW Glance collector, so values show -- until it restarts."
}

// For a kind a newer collector added that has no wording here.
var CRITICAL_TEXT_OTHER = "Ending this can break your desktop session or the system."

// The collector speaks protocol 2 or later: `critical` is reliable, and its
// absence means the process is not critical.
function collectorMarksCritical(proto) {
  return Number(proto) >= 2
}

function legacyCriticalKey(p) {
  if (Number(p.pid) === 1) return "system"
  var name = String(p.name || "").toLowerCase()
  if (LEGACY_CRITICAL.hasOwnProperty(name)) return LEGACY_CRITICAL[name]
  var first = String(p.cmdline || "").split(" ")[0]
  var base = first.substring(first.lastIndexOf("/") + 1).toLowerCase()
  if (LEGACY_CRITICAL.hasOwnProperty(base)) return LEGACY_CRITICAL[base]
  return ""
}

function criticalKey(p, proto) {
  if (collectorMarksCritical(proto))
    return (typeof p.critical === "string") ? p.critical : ""
  return legacyCriticalKey(p)
}

// Keeps only what the popup shows from the (large) process payload. `proto` is
// the snapshot's protocol number; without it the collector is taken to be an
// old one and the fallback name list decides what is critical.
function compactGroups(raw, proto) {
  var out = []
  if (!Array.isArray(raw)) return out
  for (var i = 0; i < raw.length; i++) {
    var g = raw[i]
    var procs = g.procs || []
    var pids = []
    var pidUids = []
    var pidTicks = []
    var crit = {}
    var cmd = ""
    for (var j = 0; j < procs.length; j++) {
      var p = procs[j]
      pids.push(p.pid)
      pidUids.push(p.uid)
      pidTicks.push(isNumber(p.start_time_ticks) ? Number(p.start_time_ticks) : null)
      var key = criticalKey(p, proto)
      if (key) crit[key] = true
      if (!cmd && p.cmdline) cmd = String(p.cmdline)
    }
    out.push({
      id: String(g.id),
      cls: g.class || "",
      name: String(g.name || g.id),
      title: g.title ? String(g.title) : "",
      n: procs.length,
      cpu: Number(g.cpu_pct) || 0,
      gpu: isNumber(g.gpu_pct) ? Number(g.gpu_pct) : null,
      mem: Number(g.mem_bytes) || 0,
      pids: pids,
      pidUids: pidUids,
      pidTicks: pidTicks,
      critical: Object.keys(crit),
      cmd: cmd,
      ended: false
    })
  }
  return out
}

function sortValue(g, key) {
  if (key === "gpu") return g.gpu === null ? -1 : g.gpu
  if (key === "mem") return g.mem
  return g.cpu
}

// Top `count` groups across all classes, highest first; name breaks ties.
function topGroups(groups, key, count) {
  var copy = groups.slice()
  copy.sort(function(a, b) {
    var d = sortValue(b, key) - sortValue(a, key)
    if (d !== 0) return d
    return a.name < b.name ? -1 : (a.name > b.name ? 1 : 0)
  })
  return copy.slice(0, count)
}

// A held row whose group vanished: kept in place, flagged and zeroed.
function endedCopy(g) {
  var c = {}
  for (var k in g) c[k] = g[k]
  c.cpu = 0
  c.gpu = null
  c.mem = 0
  c.ended = true
  return c
}

function displayName(g) {
  return g.n > 1 ? g.name + " (" + g.n + ")" : g.name
}

function pidText(pids, max) {
  if (!pids || pids.length === 0) return ""
  var shown = pids.slice(0, max)
  var text = shown.join(", ")
  if (pids.length > max) text += " +" + (pids.length - max) + " more"
  return text
}

function groupTooltip(g) {
  var lines = []
  lines.push(g.title ? g.title : g.name)
  if (g.title && g.title !== g.name) lines.push(g.name)
  lines.push("pid " + pidText(g.pids, 8))
  if (g.cmd) lines.push(g.cmd.length > 160 ? g.cmd.substring(0, 160) + "..." : g.cmd)
  return lines.join("\n")
}

// The warning for a group that holds critical processes, or "" when it holds
// none. A kind without wording of its own still gets a plain warning.
function criticalText(g) {
  var kinds = g.critical || []
  var seen = {}
  var parts = []
  for (var i = 0; i < kinds.length; i++) {
    var text = CRITICAL_TEXT.hasOwnProperty(kinds[i]) ? CRITICAL_TEXT[kinds[i]] : CRITICAL_TEXT_OTHER
    if (!seen[text]) {
      seen[text] = true
      parts.push(text)
    }
  }
  return parts.join(" ")
}

// What identifies the processes of a group across snapshots: the pid and,
// when the collector sent it, the start time (a recycled pid has another one).
function pidKeys(g) {
  var keys = []
  var pids = g.pids || []
  for (var i = 0; i < pids.length; i++) {
    var pid = Number(pids[i])
    if (!(pid >= 1)) continue
    var ticks = (g.pidTicks && isNumber(g.pidTicks[i])) ? Number(g.pidTicks[i]) : null
    keys.push({ pid: pid, ticks: ticks })
  }
  return keys
}

// True when any of the keyed processes is still in `groups`. Null when there
// is no process list to look at, so nothing can be said.
function anyAlive(groups, keys) {
  if (!Array.isArray(groups)) return null
  for (var g = 0; g < groups.length; g++) {
    var pids = groups[g].pids || []
    for (var i = 0; i < pids.length; i++) {
      var ticks = (groups[g].pidTicks && isNumber(groups[g].pidTicks[i])) ? Number(groups[g].pidTicks[i]) : null
      for (var k = 0; k < keys.length; k++) {
        if (keys[k].pid !== Number(pids[i])) continue
        if (keys[k].ticks === null || ticks === null || keys[k].ticks === ticks) return true
      }
    }
  }
  return false
}

// Text of a row's End button once it has been clicked: names the target, with
// a long name cut short.
function armLabel(g) {
  var name = String(g.name || "")
  if (name.length > 20) name = name.substring(0, 19) + "..."
  return "End " + (g.n > 1 ? name + " (" + g.n + ")" : name) + "?"
}

// Splits the group's pids into those the user owns (plain kill) and the rest
// (kill through pkexec). Kernel threads cannot be ended at all.
function killPlan(g, myUid) {
  if (g.id === "kthreads") return { blocked: "Kernel threads can't be ended", own: [], other: [] }
  var own = []
  var other = []
  for (var i = 0; i < g.pids.length; i++) {
    var pid = Number(g.pids[i])
    if (pid < 1) continue
    if (Number(g.pidUids[i]) === Number(myUid)) own.push(pid)
    else other.push(pid)
  }
  if (own.length === 0 && other.length === 0) return { blocked: "Nothing to end", own: own, other: other }
  return { blocked: "", own: own, other: other }
}

// argv vectors for the plan. Never goes through a shell.
function killCommands(plan, signal) {
  var cmds = []
  var tail = ["-s", signal, "--"]
  if (plan.own.length > 0)
    cmds.push(["kill"].concat(tail, plan.own.map(String)))
  if (plan.other.length > 0)
    cmds.push(["pkexec", "/usr/bin/kill"].concat(tail, plan.other.map(String)))
  return cmds
}
