# HW Glance bar widget (raed.hwglance)

A quick-look Omarchy shell bar widget for HW Glance. It replaces a general
activity monitor with something smaller: a bar button (an icon, or live CPU and
GPU usage), a popup with key readings and the top processes, and a button that
opens the full app.

## What it does

- Bar: the button shows a static chip icon, or live usage, as chosen in the HW
  Glance app's Settings (`tray.bar_display` in `ui.json`: `icon`, `cpu`, `gpu` or
  `cpu_gpu`; a missing or unknown value means `icon`). Usage reads "CPU 12%",
  "GPU 41%" or "CPU 12%  GPU 41%" in the bar's own label style (the clock's font
  size, margins and foreground colour), in a slot sized for the widest text so the
  bar does not shift as digits change. The tooltip is "HW Glance" plus the values
  ("CPU 12 %, GPU 41 %"). Left click toggles the popup, right click opens the full
  app, middle click does nothing.
- The values come from `$XDG_RUNTIME_DIR/hwglance/summary.json`, which the
  collector rewrites on every sample (once a second by default). It is reloaded every 2 s, and only
  while a value mode is set, so icon mode reads nothing. If the file is missing or
  its `unix_ms` is more than 10 s old, the button falls back to the icon and the
  tooltip says the collector is not running. Vertical bars always show the icon.
- The collector socket connection exists only while the popup is open: it subscribes
  (with processes), asks for the last 120 s of history of each card's big value
  so the trend lines are filled at once, then appends live values. It
  disconnects on close.
- Popup: header with Open and settings buttons, then three device cards side by
  side (CPU blue, GPU magenta, Memory cyan, taken from the theme's
  `colors.toml`). `tray.popup_values` is grouped by first path segment in list
  order; the first id of a group is the big value, up to four more become rows.
  Values take warn (yellow) and critical (red) colours from the limits the
  collector sends with each reading (its defaults, or the ones in the user's
  `thresholds.json`, which replace the defaults as a whole), and also carry the window's letters beside the
  value (H and HH for high, L and LL for a low-limit reading), so the state does
  not rest on colour alone; the hover text says it in words ("above its critical
  level"). A red "Throttling" tag shows while
  `cpu/throttle/thermal` or `gpu0/throttle/thermal` is 1. Hovering a card lists
  its values with full labels and short descriptions. Summary readings (for
  example `memory/temp/summary`) are looked up too. `tray.bar_values` is ignored.
- Top processes (`tray.popup_process_count`, CPU / GPU / Memory sort buttons for
  this session only). Row order is held while the pointer is over the list, a
  row has the keyboard cursor, a button is armed or a question is open, and is
  re-sorted at most every 3 s otherwise. Up and Down (or j and k) move a row
  cursor, the same highlight the pointer uses; Enter, Space or x does what
  clicking that row's End task button does. Esc disarms before it closes.
- End task never goes in one click. The first click on a row's button turns it
  into "End <name>?" (with the process count for a group) for 3 s; a second click
  sends. The button is at least 24 px tall. Right click a row for End task (which
  arms the row the same way) or Force kill. Force kill always asks first, naming
  the target and its process count ("It stops at once and cannot save its work.").
  A group with a critical process (the collector marks them: protocol 2 sends a
  `critical` kind such as session, system, xwayland, shell, audio or collector)
  skips the arming and asks in the question box, with a plain warning of what
  breaks. Only against an older collector (protocol 1, no such field) does the
  widget fall back to its own name list. A question is answered with the mouse
  (or Esc), never from the keyboard, so a stray key cannot Force kill.
- The outcome follows the processes: "Sent End task to 3 processes", then
  "<name> ended" once the collector's list no longer holds them, or after 4 s
  "<name> is still running" with a Force kill button beside it. The row also
  reads "<name> ended". Own pids go through `kill -s <SIGNAL> -- <pids>`, other
  users' pids through `pkexec /usr/bin/kill -s <SIGNAL> -- <pids>` (polkit prompts).
- Colours: labels, captions and values use the popup's ink and a muted ink, never
  opacity. The muted ink is the theme's `dark_foreground` or `muted` when that
  reads at 4.5:1 on the popup, else the text colour pulled toward the background as
  far as 4.5:1 allows. Warn, critical and the three device accents take the theme's
  yellow, red, blue, magenta and cyan, else their bright variant, else a blend
  toward the text colour, so each reaches 4.5:1 on the popup background and on the
  faint fill of cards and hovered rows (`Model.labelColors`, computed at runtime from
  `colors.toml` and the shell's popup colours; a failure falls back to the plain
  palette). A theme's own text colour is taken as given.

## Files

| File | Role |
|---|---|
| `manifest.json` | Plugin manifest (bar widget, no settings schema) |
| `BarWidget.qml` | Entry point: bar segment, popup window, wiring |
| `PopupContent.qml` | Popup UI: cards, process list, menu, confirmation |
| `DeviceCard.qml`, `Sparkline.qml` | One device card and its trend line |
| `HwClient.qml` | Socket client for `hwglanced` (open only while the popup is) with history, reconnect and stale detection |
| `UiSettings.qml` | Read-only watcher for `~/.config/hwglance/ui.json` |
| `SummaryFeed.qml` | Polls the collector's `summary.json` for the bar values (value modes only) |
| `ThemeColors.qml` | Muted, warn, critical and device text colours, picked from the theme's `colors.toml` for contrast |
| `KillController.qml` | Armed button, confirmation state, kill commands, result line that follows the processes |
| `Model.js` | Pure helpers: formatting, thresholds and marks, grouping, kill plan, contrast |

## Notes

- Settings are never written. Defaults apply when `ui.json` is missing or a key
  is absent.
- Structure follows the `stappmus.activity-monitor` plugin (MIT): a `Panel`
  with a `KeyboardPanel` popup and a guarded process-action controller. No code
  was copied verbatim.
- Test overrides (not needed in normal use): `HWGLANCE_SOCKET`, `HWGLANCE_UI` and
  `HWGLANCE_SUMMARY` point the widget at another socket, settings file or summary file.
- `../harness/` holds the developer tooling, none of it part of the plugin (do not install it):
  `node ../harness/model-test.js` checks the parsing, label, critical-process and threshold-mark
  helpers, and the popup's text colours on every stock theme in
  `/usr/share/omarchy/themes/*/colors.toml` (it fails if any label pair is under 4.5:1);
  `../harness/fake-daemon.py <socket>` serves the fixture snapshot with history and throttle flags
  (use with `HWGLANCE_SOCKET`); `shell.qml` is a Quickshell test shell with a mock bar and dummy
  `sleep` processes (run `quickshell -p omarchy-plugin/harness` on an Omarchy system; set
  `HWG_SCREEN` to a monitor name).
