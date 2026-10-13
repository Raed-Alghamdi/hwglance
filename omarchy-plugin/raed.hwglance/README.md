# HW Glance bar widget (raed.hwglance)

A quick-look bar widget for the Omarchy shell and the HW Glance collector: a bar button (an icon, or live CPU and GPU usage), a popup with key readings and the top processes, and a button that opens the full HW Glance window.

## Install

`packaging/install.sh` (or `hwglance-bar-widget install`, which the Arch package ships) copies this directory to `~/.config/omarchy/plugins/raed.hwglance` on an Omarchy system and enables it; `hwglance-bar-widget update` refreshes the copy after an upgrade. The collector (`hwglanced`) must be running for the popup and for live usage in the bar. The widget never writes settings.

## What it shows

### Bar

- The button shows a chip icon, or live usage ("CPU 12%", "GPU 41%" or "CPU 12%  GPU 41%"), as set in the HW Glance window's Settings (`tray.bar_display` in `ui.json`: `icon`, `cpu`, `gpu` or `cpu_gpu`; a missing or unknown value means `icon`). Usage uses the bar's own label style, in a slot sized for the widest text so the bar does not shift as digits change. Vertical bars always show the icon.
- The tooltip is "HW Glance" plus the values ("CPU 12 %, GPU 41 %").
- Left click toggles the popup. Right click opens the full window. Middle click does nothing.
- Usage comes from `$XDG_RUNTIME_DIR/hwglance/summary.json`, which the collector rewrites on every sample (every second by default). The widget reloads it every 2 s, and only in a usage mode. If the file is missing or its `unix_ms` is more than 10 s old, the button falls back to the icon and the tooltip says the collector is not running.

### Popup

- A header with Open and settings buttons, then three device cards side by side: CPU (blue), GPU (magenta) and Memory (cyan), with colours from the theme's `colors.toml`. Hovering a card lists its values with full labels and short descriptions.
- `tray.popup_values` in `ui.json` lists the readings, grouped by first path segment in list order. The first id of a group is the big value; up to four more become rows. Summary readings (for example `memory/temp/summary`) work too. `tray.bar_values` is ignored.
- A reading is yellow at its warning limit and red at its critical limit, with the window's flag letters beside it (H and HH for high, L and LL for a low-limit reading), so colour is never the only cue. Hover text says it in words ("above its critical level"). The limits come with each reading from the collector: its defaults, or the entries of the user's `thresholds.json`, which replace the defaults as a whole.
- A red "Throttling" tag shows while `cpu/throttle/thermal` or `gpu0/throttle/thermal` is 1.
- The popup connects to the collector's socket only while it is open. It subscribes (with processes), asks for the last 120 s of history of each card's big value so the trend lines are filled at once, then appends live values.

### Top processes and End task

- `tray.popup_process_count` sets how many rows show. CPU, GPU and Memory buttons sort them for this session only. The list holds its order while the pointer is over it, a row has the keyboard cursor, a button is armed or a question is open, and otherwise re-sorts at most every 3 s.
- Up and Down (or j and k) move the row cursor. Enter, Space or x does what clicking that row's End task button does. Esc disarms before it closes.
- End task takes two clicks. The first turns the button into "End <name>?" (with the process count for a group) for 3 s; the second sends. Right click a row for End task (which arms the row the same way) or Force kill.
- Force kill always asks first, naming the target and its process count ("It stops at once and cannot save its work."). A group with a critical process (the collector marks them with a `critical` kind: session, system, xwayland, shell, audio or collector; protocol 2) skips the arming and asks in the question box, with a plain warning of what breaks. Against an older collector (protocol 1) the widget falls back to its own name list. A question is answered with the mouse (or Esc), never the keyboard, so a stray key cannot Force kill.
- The outcome follows the processes: "Sent End task to 3 processes", then "<name> ended" once the collector's list no longer holds them, or after 4 s "<name> is still running" with a Force kill button beside it.
- Own processes are ended with `kill -s <SIGNAL> -- <pids>`, other users' with `pkexec /usr/bin/kill -s <SIGNAL> -- <pids>` (polkit prompts). Kernel threads cannot be ended.

### Colours

Labels, captions and values reach 4.5:1 contrast on the popup, except where a theme's own text colour is taken as given. The muted ink is the theme's `dark_foreground` or `muted` when that passes, else the text colour pulled toward the background. Warning, critical and the three device accents take the theme's yellow, red, blue, magenta and cyan, else their bright variant, else a blend toward the text colour, so they also pass on the faint fill of cards and hovered rows. `Model.labelColors` computes this at runtime from `colors.toml` and the shell's popup colours, and falls back to the plain palette on failure.

## Files

| File | Role |
|---|---|
| `manifest.json` | Plugin manifest (bar widget, no settings schema) |
| `BarWidget.qml` | Entry point: bar segment, popup window, wiring |
| `PopupContent.qml` | Popup UI: cards, process list, menu, confirmation |
| `DeviceCard.qml`, `Sparkline.qml` | One device card and its trend line |
| `HwClient.qml` | Socket client for `hwglanced`, with history, reconnect and stale detection |
| `UiSettings.qml` | Read-only watcher for `~/.config/hwglance/ui.json` |
| `SummaryFeed.qml` | Polls the collector's `summary.json` for the bar values |
| `ThemeColors.qml` | Muted, warning, critical and device text colours, picked from the theme's `colors.toml` for contrast |
| `KillController.qml` | Armed button, question state, kill commands, the result line that follows the processes |
| `Model.js` | Pure helpers: formatting, limits and marks, grouping, kill plan, contrast |

## Development

- The structure follows the `stappmus.activity-monitor` plugin (MIT): a `Panel` with a `KeyboardPanel` popup and a guarded process-action controller. No code was copied verbatim.
- `HWGLANCE_SOCKET`, `HWGLANCE_UI` and `HWGLANCE_SUMMARY` point the widget at another socket, settings file or summary file (for tests; not needed in normal use). Defaults apply when `ui.json` is missing or a key is absent.
- `../harness/` holds developer tooling that is not part of the plugin (do not install it):
  - `node ../harness/model-test.js` checks the parsing, label, critical-process and limit-mark helpers, and the popup's text colours on every stock theme in `/usr/share/omarchy/themes/*/colors.toml` (it fails if any label pair is under 4.5:1).
  - `../harness/fake-daemon.py <socket>` serves the fixture snapshot with history and throttle flags (use it with `HWGLANCE_SOCKET`).
  - `shell.qml` is a Quickshell test shell with a mock bar and dummy `sleep` processes: run `quickshell -p omarchy-plugin/harness` on an Omarchy system, with `HWG_SCREEN` set to a monitor name.
