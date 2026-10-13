// Checks the pure bar-button helpers in Model.js. Run: node harness/model-test.js
const fs = require("fs")
const path = require("path")
const src = fs.readFileSync(path.join(__dirname, "../raed.hwglance/Model.js"), "utf8").replace(/^\.pragma library$/m, "")
const M = new Function(src + "; return { parseUi, parseSummary, extractReadings, barParts, barText, barSample, barTooltip, compactGroups, topGroups, criticalText, readingMark, levelMark, cardTooltip, armLabel, pidKeys, anyAlive, killPlan, killCommands, contrast, parseColor, mixColors, colorHex, labelColors, parseColorsToml, MIN_CONTRAST }")()
let failed = 0
function eq(name, got, want) {
  const ok = JSON.stringify(got) === JSON.stringify(want)
  if (!ok) failed++
  console.log((ok ? "ok   " : "FAIL ") + name + (ok ? "" : "\n  got  " + JSON.stringify(got) + "\n  want " + JSON.stringify(want)))
}

// ui.json parsing
const display = (text) => M.parseUi(text).barDisplay
eq("default empty", display(""), "icon")
eq("default {}", display("{}"), "icon")
eq("invalid json", display("{nope"), "icon")
eq("icon", display('{"tray":{"bar_display":"icon"}}'), "icon")
eq("cpu", display('{"tray":{"bar_display":"cpu"}}'), "cpu")
eq("gpu", display('{"tray":{"bar_display":"gpu"}}'), "gpu")
eq("cpu_gpu", display('{"tray":{"bar_display":"cpu_gpu"}}'), "cpu_gpu")
eq("unknown", display('{"tray":{"bar_display":"both"}}'), "icon")
eq("wrong type", display('{"tray":{"bar_display":7}}'), "icon")
eq("null", display('{"tray":{"bar_display":null}}'), "icon")

// summary.json parsing
const now = 1_000_000
const sum = (o) => JSON.stringify(o)
const fresh = M.parseSummary(sum({ unix_ms: now - 2000, cpu_usage_pct: 20.68, gpu_usage_pct: 14.0 }), now)
eq("fresh", fresh, { fresh: true, cpu: 20.68, gpu: 14 })
eq("stale", M.parseSummary(sum({ unix_ms: now - 10001, cpu_usage_pct: 1, gpu_usage_pct: 2 }), now).fresh, false)
eq("edge 10 s is fresh", M.parseSummary(sum({ unix_ms: now - 10000, cpu_usage_pct: 1 }), now).fresh, true)
eq("far future is stale", M.parseSummary(sum({ unix_ms: now + 60000, cpu_usage_pct: 1 }), now).fresh, false)
eq("missing text", M.parseSummary("", now), { fresh: false, cpu: null, gpu: null })
eq("garbage", M.parseSummary("{", now).fresh, false)
eq("no unix_ms", M.parseSummary(sum({ cpu_usage_pct: 5 }), now).fresh, false)
eq("null gpu", M.parseSummary(sum({ unix_ms: now, cpu_usage_pct: 5, gpu_usage_pct: null }), now), { fresh: true, cpu: 5, gpu: null })

// labels
const s = { fresh: true, cpu: 12.4, gpu: 40.6 }
eq("text icon", M.barText(M.barParts("icon", s)), "")
eq("text cpu", M.barText(M.barParts("cpu", s)), "CPU 12%")
eq("text gpu", M.barText(M.barParts("gpu", s)), "GPU 41%")
eq("text both", M.barText(M.barParts("cpu_gpu", s)), "CPU 12%  GPU 41%")
eq("text pad", M.barText(M.barParts("cpu_gpu", { fresh: true, cpu: 3, gpu: 100 })), "CPU  3%  GPU 100%")
eq("text clamp", M.barText(M.barParts("cpu", { fresh: true, cpu: 130, gpu: 0 })), "CPU 100%")
eq("text no gpu", M.barText(M.barParts("cpu_gpu", { fresh: true, cpu: 7, gpu: null })), "CPU  7%")
eq("text stale", M.barParts("cpu", { fresh: false, cpu: null, gpu: null }), [])
eq("text no data at all", M.barParts("gpu", { fresh: true, cpu: 7, gpu: null }), [])
eq("sample both", M.barSample("cpu_gpu"), "CPU 100%  GPU 100%")
eq("sample icon", M.barSample("icon"), "")

// tooltips
eq("tip icon", M.barTooltip("icon", s), "HW Glance")
eq("tip icon stale", M.barTooltip("icon", { fresh: false }), "HW Glance")
eq("tip cpu_gpu", M.barTooltip("cpu_gpu", s), "HW Glance\nCPU 12 %, GPU 41 %")
eq("tip cpu", M.barTooltip("cpu", s), "HW Glance\nCPU 12 %")
eq("tip stale", M.barTooltip("gpu", { fresh: false, cpu: null, gpu: null }), "HW Glance\nThe collector is not running")
eq("tip fresh no value", M.barTooltip("gpu", { fresh: true, cpu: 7, gpu: null }), "HW Glance")

// processes as protocol 2 writes them: empty or absent fields are left out, not null
const rawProcs = [
  { id: "app:x", class: "app", name: "X", cpu_pct: 3.5, mem_bytes: 100,
    procs: [{ pid: 7, ppid: 1, uid: 1000, name: "x", cmdline: "/usr/bin/x --a", threads: 2, cpu_pct: 3.5, mem_bytes: 100, start_time_ticks: 5 }] },
  { id: "kthreads", class: "system", name: "Kernel threads", cpu_pct: 0, mem_bytes: 0, gpu_pct: 4,
    procs: [{ pid: 2, ppid: 0, uid: 0, name: "kthreadd", threads: 1, cpu_pct: 0, mem_bytes: 0, start_time_ticks: 1 },
            { pid: 9, ppid: 2, uid: 0, name: "hwglanced", threads: 1, cpu_pct: 0, mem_bytes: 0, start_time_ticks: 1 }] },
]
const groups = M.compactGroups(rawProcs)
// An old collector (protocol 1, or no number) sends no critical field: the fallback name list decides.
eq("groups without optional keys (old collector)", groups.map((g) => [g.id, g.gpu, g.cmd, g.n, g.critical]),
  [["app:x", null, "/usr/bin/x --a", 1, []], ["kthreads", 4, "", 2, ["collector"]]])
eq("pid start times kept", groups[0].pidTicks, [5])
eq("no processes yet", M.compactGroups(undefined), [])
eq("top by gpu keeps groups without a gpu value last", M.topGroups(groups, "gpu", 2).map((g) => g.id), ["kthreads", "app:x"])

// critical processes: protocol 2 says which, the old name list is only a fallback
const v2 = M.compactGroups([
  { id: "app:hypr", class: "app", name: "Hyprland", cpu_pct: 1, mem_bytes: 1,
    procs: [{ pid: 20, uid: 1000, name: "Hyprland", cmdline: "/usr/bin/Hyprland", critical: "session" },
            { pid: 21, uid: 1000, name: "helper", critical: "future-kind" }] },
  { id: "app:quick", class: "app", name: "quickshell", cpu_pct: 1, mem_bytes: 1,
    procs: [{ pid: 30, uid: 1000, name: "quickshell", cmdline: "quickshell -c x" }] },
  { id: "app:one", class: "app", name: "init", cpu_pct: 1, mem_bytes: 1,
    procs: [{ pid: 1, uid: 0, name: "systemd", cmdline: "/sbin/init", critical: "system" }] },
], 2)
eq("v2 uses the collector's kinds", v2.map((g) => g.critical), [["session", "future-kind"], [], ["system"]])
eq("v2 ignores the old name list", v2[1].critical, [])
eq("v1 falls back to the name list", M.compactGroups(rawProcs, 1)[1].critical, ["collector"])
eq("old list also catches the command line and pid 1",
  M.compactGroups([{ id: "a", name: "a", procs: [{ pid: 4, name: "x", cmdline: "/usr/bin/Hyprland" }, { pid: 1, name: "init" }] }])[0].critical.sort(),
  ["session", "system"])
eq("no warning for a plain group", M.criticalText(v2[1]), "")
eq("warning for a known kind", M.criticalText(v2[2]).startsWith("systemd runs"), true)
eq("a kind with no wording still warns", M.criticalText(v2[0]).includes("Ending this can break"), true)
eq("a group with several kinds warns for each",
  M.criticalText({ critical: ["session", "audio"] }).includes("desktop session") && M.criticalText({ critical: ["session", "audio"] }).includes("audio"), true)

// threshold marks, the same letters as the window
eq("mark none", M.levelMark(0, false), "")
eq("mark H", M.levelMark(1, false), "H")
eq("mark HH", M.levelMark(2, false), "HH")
eq("mark L", M.levelMark(1, true), "L")
eq("mark LL", M.levelMark(2, true), "LL")
const hot = { id: "cpu/temp/tctl", label: "CPU Tctl", unit: "celsius", value: 91, warn: 80, crit: 90 }
eq("reading past crit", M.readingMark(hot), "HH")
eq("reading past warn", M.readingMark({ ...hot, value: 85 }), "H")
eq("reading normal", M.readingMark({ ...hot, value: 40 }), "")
// the collector sends the limits in force in warn and crit (the user's from thresholds.json, whole)
eq("a moved limit moves the level", M.readingMark({ ...hot, value: 85, warn: 90, crit: 95 }), "")
eq("no limit at a level means none, whatever the default was", M.readingMark({ ...hot, value: 85, warn: null }), "")
eq("below limits", M.readingMark({ ...hot, id: "gpu0/fan", value: 5, warn: 20, crit: 10, below: true }), "LL")
eq("below limits, warning only", M.readingMark({ ...hot, id: "gpu0/fan", value: 15, warn: 20, crit: 10, below: true }), "L")
// extractReadings takes the direction from the snapshot's limit_overrides
const sensors = [{ title: "GPU", sections: [{ title: "Fans", readings: [
  { id: "gpu0/fan", label: "Fan", unit: "rpm", value: 5, warn: 20, crit: 10 },
  { id: "gpu0/temp", label: "Core", unit: "celsius", value: 50, warn: 80, crit: 90 }] }] }]
const picked = M.extractReadings(sensors, { "gpu0/fan": true, "gpu0/temp": true }, { "gpu0/fan": { warn: 20, crit: 10, below: true } })
eq("direction comes from limit_overrides", [picked["gpu0/fan"].below, picked["gpu0/temp"].below], [true, false])
eq("and shows as a mark", [M.readingMark(picked["gpu0/fan"]), M.readingMark(picked["gpu0/temp"])], ["LL", ""])
eq("an older collector has no limit_overrides", M.extractReadings(sensors, { "gpu0/fan": true })["gpu0/fan"].below, false)
eq("ui.json no longer carries limits", M.parseUi('{"sensors":{"thresholds":{"a":{"warn":1}}}}').thresholds, undefined)
eq("tooltip says it in words", M.cardTooltip({ key: "cpu", ids: ["cpu/temp/tctl"] }, { "cpu/temp/tctl": hot }).split("\n")[0],
  "CPU Tctl: 91.0 °C (above its critical level)")
eq("tooltip without a level", M.cardTooltip({ key: "cpu", ids: ["cpu/temp/tctl"] }, { "cpu/temp/tctl": { ...hot, value: 40 } }).split("\n")[0], "CPU Tctl: 40.0 °C")

// End button label, and following the processes after a kill
eq("arm label", M.armLabel({ name: "Steam", n: 1 }), "End Steam?")
eq("arm label with count", M.armLabel({ name: "Steam", n: 23 }), "End Steam (23)?")
eq("arm label cuts a long name", M.armLabel({ name: "a-very-long-application-name", n: 1 }), "End a-very-long-applica...?")
const keys = M.pidKeys({ pids: [7, 8, 0], pidTicks: [5, undefined, 1] })
eq("pid keys", keys, [{ pid: 7, ticks: 5 }, { pid: 8, ticks: null }])
eq("alive by pid and start time", M.anyAlive(groups, M.pidKeys(groups[0])), true)
eq("a recycled pid is another process", M.anyAlive([{ pids: [7], pidTicks: [99] }], keys), false)
eq("pid without start time still matches", M.anyAlive([{ pids: [8] }], keys), true)
eq("gone", M.anyAlive([{ pids: [100] }], keys), false)
eq("no list, no answer", M.anyAlive(null, keys), null)

// Label contrast on every stock Omarchy theme (colors.toml): ink, the muted
// label colour, warning and critical text and the three device accents must
// reach 4.5:1 on the popup background and on the faint fill of cards and
// hovered rows. The old look (foreground at 60 % opacity, raw palette colours)
// is measured on the background alone, as in the review, for comparison.
const themesDir = "/usr/share/omarchy/themes"
const FILL = 0.08
let themeNames = []
try { themeNames = fs.readdirSync(themesDir).sort().filter((t) => fs.existsSync(path.join(themesDir, t, "colors.toml"))) } catch (e) {}
if (themeNames.length === 0) console.log("skip theme contrast: no themes in " + themesDir)
else {
  const pairs = ["muted", "warn", "crit", "blue", "magenta", "cyan"]
  const oldFails = { label: 0, warn: 0, crit: 0 }
  const bad = []
  const lines = []
  const ratio = (x, bg, fg) => Math.min(M.contrast(x, bg), M.contrast(x, M.colorHex(M.mixColors(M.parseColor(bg), M.parseColor(fg), FILL))))
  for (const t of themeNames) {
    const c = M.parseColorsToml(fs.readFileSync(path.join(themesDir, t, "colors.toml"), "utf8"))
    if (!c.background || !c.foreground) { bad.push(t + ": no background or foreground"); continue }
    const label = M.colorHex(M.mixColors(M.parseColor(c.foreground), M.parseColor(c.background), 0.4))
    if (M.contrast(label, c.background) < 4.5) oldFails.label++
    if (c.yellow && M.contrast(c.yellow, c.background) < 4.5) oldFails.warn++
    if (c.red && M.contrast(c.red, c.background) < 4.5) oldFails.crit++
    const out = M.labelColors(c, c.foreground, c.background, FILL)
    const r = { ink: ratio(c.foreground, c.background, c.foreground) }
    for (const k of pairs) r[k] = ratio(out[k], c.background, c.foreground)
    // The ink is the theme's own and cannot be fixed here; flag it separately.
    if (r.ink < 4.5) bad.push(t + ": the theme's own text is " + r.ink.toFixed(2) + ":1")
    for (const k of pairs) if (r[k] < 4.5 && r.ink >= 4.5) bad.push(t + ": " + k + " " + out[k] + " is " + r[k].toFixed(2) + ":1")
    lines.push(t.padEnd(17) + ["ink", ...pairs].map((k) => k + " " + r[k].toFixed(2)).join("  "))
  }
  console.log(lines.join("\n"))
  console.log("before (60 % opacity labels, palette colours, background only): labels failing " + oldFails.label + " of " + themeNames.length
    + ", warning " + oldFails.warn + ", critical " + oldFails.crit)
  eq("every label pair reaches 4.5:1 on " + themeNames.length + " stock themes", bad, [])
}
eq("fallback on unusable input", M.labelColors({}, "nonsense", undefined, 0.08).muted.length, 7)
eq("a QML colour object is accepted", M.colorHex(M.parseColor({ r: 1, g: 0.5, b: 0 })), "#ff8000")
eq("QML string form with alpha", M.colorHex(M.parseColor("#80ff8000")), "#ff8000")

if (failed) { console.log(failed + " failed"); process.exit(1) }
console.log("all passed")
