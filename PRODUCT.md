# Product

<!-- impeccable:product-schema 1 -->

## Platform

linux-desktop

## Users

Linux desktop users who want a fast, readable hardware monitor: people who game, build software or run long jobs on a desktop PC and want to see every sensor and every process in one window. HW Glance is developed and tested on an AMD Ryzen + NVIDIA desktop with Omarchy on Hyprland; users on other hardware and desktops are welcome, but it is untested there. They use it in three situations:

1. **Deliberate check:** open it to see which process is taking resources, and to end it.
2. **After a temperature warning:** open it to see the full picture, what ran hot, how hot and since when.
3. **While gaming or working:** the window sits maximized on a side monitor and the user glances over for a second or two, with the process panel docked on the right in its compact view (name, CPU, GPU, memory).

Users often work in a dim or dark room. Colour is never the only carrier of meaning, so colour-vision differences should not matter.

## Product Purpose

Give Linux the combination of HWiNFO (sensors) and Windows Task Manager (processes) in one window that reads at a glance. Success: the user finds the reading they want in about a second, understands it in the context of its device, and can end a misbehaving app in two clicks.

## Positioning

A monitor that shows every sensor the machine exposes rather than a generic dashboard: GPU hotspot and VRAM temperatures that NVIDIA hides on Linux (read through LACT), DIMM temperatures, and exact CPU package power from a small root helper, next to app-grouped processes with per-process GPU use. An optional Omarchy bar widget gives a quick look from the bar, and an opt-in plain-text export (`text_readouts`) feeds the same numbers to status bars, scripts and macro keypads.

## Operating Context

- The window can open maximized on a chosen monitor (a compositor key binding or window rule), or from the application launcher.
- The collector (`hwglanced`) runs all the time as a user service and keeps min, max, average and history since it started or since the last reset; the window connects to it.
- The Omarchy bar widget opens a quick popup with key values and the top processes.
- Users set their own warning and critical limits, watched and hidden readings and layout, in the window or by editing `~/.config/hwglance/thresholds.json`, which the collector re-reads when it changes.
- English only, Western digits, 24-hour times. No Arabic UI (Arabic window titles and process names are displayed correctly).
- A desktop tool: no personal, patient or health data, no regulated use.

## Capabilities and Constraints

- Sensors: CPU usage and clocks per core, temperatures, package power; GPU load, core, hotspot and VRAM temperatures, clocks, VRAM use, power and fan; memory use, DIMM temperatures and speed; per-drive read/write and temperatures; motherboard temperatures and fans. Current, Min, Max and Average for each.
- Processes grouped like Task Manager (Apps, Background, System), sortable by CPU, GPU, VRAM and memory, with End task and Force kill. Force kill and critical processes (the session, the compositor, the audio server, the collector) ask first.
- Resets (statistics, layout, process views, all customisations) act at once and offer Undo for 30 seconds (Ctrl+Z).
- Read-only toward hardware: it never writes fan, clock or power settings.
- Stack: Rust with egui (immediate mode, glow renderer) for the window; Quickshell QML for the bar widget. The bar widget follows the Omarchy shell design; the window does not need to follow the Omarchy theme. Web-only tooling (the HTML/CSS detector) does not apply.
- Resource budget: collector and window stay well under 2% of one core.
- Every reading keeps a history (30 minutes by default, adjustable) shown on hover; limit alerts as desktop notifications that open the app at the reading; thermal and power throttling Yes/No per device card; a process panel that docks at the bottom or on the right, each with a compact and an expanded view, a column picker and a resizable Name column; one card arrangement shared by every dock and view; a bar button that shows the icon or live CPU and GPU usage; the window's own Maximize and Close buttons.

## Brand Commitments

- Name: HW Glance.

## Evidence on Hand

- Live data from the running collector; a recorded snapshot at `fixtures/snapshot.json` (anonymized); demo data in `crates/hwglance/src/demo.rs`; rendered screenshots in `docs/screenshots/`.
- Tested on real hardware on one AMD + NVIDIA desktop so far; Intel CPUs and discrete AMD GPUs are covered by tests with fake sysfs data only. There are no public users or reviews yet.

## Product Principles

1. Find any value in about a second: which device a value belongs to must be obvious before reading its label.
2. Read values in context: a reading sits in its device's card next to its range and, on demand, its history.
3. Quiet until it matters: attention goes to thresholds crossed and values the user chose to watch.
4. Light on the machine it watches.
5. Act safely: Force kill and ending a critical process confirm, resets act at once with Undo for 30 seconds, and nothing changes hardware settings.

## Accessibility & Inclusion

Readable from a seated distance on a 1440p side monitor at 106.67% scale in a dim room (the design target). Threshold states carry a non-colour cue (flag letters, pills) wherever it costs little.
