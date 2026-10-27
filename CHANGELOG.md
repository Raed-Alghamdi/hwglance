# Changelog

All notable changes to HW Glance are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- A Fan duty section on the Motherboard card shows the PWM duty (percent) the board sends each fan header, read from the driver's `pwm` files and labelled like the fan's RPM row. Some fans never report a speed (0 RPM at any duty, for example behind a splitter that does not pass the speed signal on), so the duty is the only reading those fans give. Chips without `pwm` files, such as the ASUS embedded controller, get no duty row. It is read only, like everything else here.

### Fixed

- "Power limit reached" is no longer shown as a warning. Hitting the GPU's power limit is normal operation, but the top bar turned the GPU state red whenever it was Yes. Now only a flag with a limit of its own counts (thermal throttling): the GPU chip, the row's value and the event text in the window stay quiet for the power limit, and its history shows in neutral grey. The row still shows Yes or No with its event history.

## [0.2.0] - 2026-10-19

### Added

- CSV logging of readings. Right-click a reading and tick Log to file, or tick Log all hardware readings in the new Settings, Logging tab, which also sets the interval, the folder (with a folder chooser when `zenity` or `kdialog` is installed), shows the size to expect and has Stop logging with Undo. The collector writes the log, so it goes on while the window is closed and after you log in. Values are rounded the way the window shows them, and finished files are gzipped to `.csv.gz` by default (Compress finished files; about a quarter of the size). An optional age limit (Delete logs older than: 7, 30, 90 or 365 days) deletes old log files for good. Settings, Logging also shows what the folder holds. A new file starts when the logged values change and each day. The top bar shows a Logging chip while it runs, or Log error when the file cannot be written. Details in the README. The protocol version is now 6 (`Snapshot::logging`), so a window tells an older collector to restart.

### Changed

- Reset all customisations keeps the logging choices, as it keeps the bar widget and alert choices.

## [0.1.2] - 2026-10-15

### Added

- The window now records every way it can end, so a window that vanishes can be traced. A startup or event-loop error (which only reached a stderr the launcher discards) is written to `crash.log` and the journal. SIGTERM, SIGINT, SIGHUP and SIGQUIT are recorded there too, with the sender's pid, uid and process name. A normal exit logs one journal line saying whether the window's own Close button or a close request from the compositor or window manager closed it, and each start logs one line with the version, pid and parent process.

### Changed

- New default temperature limits, set from published ratings. CPU warning is now 4 °C below the CPU's limit (was 9). The integrated GPU gets limits too: the driver's, else the CPU's. NVIDIA core is 85 and 90 °C (was 80 and 88), hotspot 100 and 105 (was 95 and 105), VRAM and memory chips 90 and 95 (was 90 and 100), DIMMs 80 and 85 (was 60 and 75). Listed NVMe drives (Samsung 990 PRO, T-Force TM8FP5) take their datasheet rating of 70 °C for the Composite reading, with the warning 5 °C below. The ASUS EC's VRM reading gets 90 and 100 when the driver reports no limits. Limits you set yourself are not changed.

### Fixed

- On Wayland (seen on Hyprland with an NVIDIA GPU) the window could freeze and be offered for termination as "not responding" when it was out of view: on a workspace no monitor shows, or under a fullscreen or maximized window. A window that started on a hidden workspace froze on its second frame. The compositor stops sending frame callbacks to a window it is not showing, and the window waited for one inside a vsync buffer swap. The window now takes its pacing from the compositor's frame callbacks on Wayland (so the vsync setting of the toolkit has no effect there), keeps running its logic while no callbacks arrive, draws nothing, and paints again as soon as it is shown. The frame rate while visible is unchanged. This is [egui pull request 8631](https://github.com/emilk/egui/pull/8631), carried in `vendor/eframe` until egui 0.37 is released. X11 is unchanged.

## [0.1.1] - 2026-10-14

### Added

- `hwglance --version` (the collector already had it).
- Dependency checks with `cargo-deny` (`deny.toml`): advisories, licenses, duplicate versions and sources. They run in CI on every push and weekly in `.github/workflows/audit.yml`.
- Dependabot version updates for Cargo and GitHub Actions (monthly).
- `SECURITY.md`, `MAINTAINING.md`, issue forms and a pull request template.
- A panic now leaves a trace: the window and the collector write a report to `~/.local/state/hwglance/crash.log` (and the window to the journal), where before the message went to stderr only.

### Changed

- GitHub Actions are pinned to commit SHAs, on Node 24 versions. Workflows run with read-only permissions, and CI cancels superseded runs.
- The workspace crates are marked `publish = false`.
- Updated the `toml` crate to 1.1.

### Security

- A first `cargo audit` and `cargo deny check` found no known vulnerabilities, unmaintained or yanked dependencies, so there are no dependency fixes yet.

## [0.1.0] - 2026-10-14

First release.

- Device cards for the CPU, GPU, memory, each drive, the motherboard and an integrated GPU, with current, min, max and average values and a 30 minute history.
- Warning and critical limits with colour marks and desktop notifications.
- A process list grouped as Apps, Background and System, with per-process GPU use and safe End task and Force kill actions.
- Opt-in plain-text readouts for status bars and scripts.
- Optional Omarchy bar widget.
- Sensors are read only, and nothing leaves your machine.
- Packages: an Arch package and a generic x86_64 Linux tarball.

[Unreleased]: https://github.com/Raed-Alghamdi/hwglance/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/Raed-Alghamdi/hwglance/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/Raed-Alghamdi/hwglance/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/Raed-Alghamdi/hwglance/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Raed-Alghamdi/hwglance/releases/tag/v0.1.0
