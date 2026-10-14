# Changelog

All notable changes to HW Glance are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- The window now records every way it can end, so a window that vanishes can be traced. A startup or event-loop error (which only reached a stderr the launcher discards) is written to `crash.log` and the journal. SIGTERM, SIGINT, SIGHUP and SIGQUIT are recorded there too, with the sender's pid, uid and process name. A normal exit logs one journal line saying whether the window's own Close button or a close request from the compositor or window manager closed it, and each start logs one line with the version, pid and parent process.

### Changed

- New default temperature limits, set from published ratings. CPU warning is now 4 °C below the CPU's limit (was 9). The integrated GPU gets limits too: the driver's, else the CPU's. NVIDIA core is 85 and 90 °C (was 80 and 88), hotspot 100 and 105 (was 95 and 105), VRAM and memory chips 90 and 95 (was 90 and 100), DIMMs 80 and 85 (was 60 and 75). Listed NVMe drives (Samsung 990 PRO, T-Force TM8FP5) take their datasheet rating of 70 °C for the Composite reading, with the warning 5 °C below. The ASUS EC's VRM reading gets 90 and 100 when the driver reports no limits. Limits you set yourself are not changed.

### Fixed

- On Hyprland the window could freeze and be ended from "not responding" while it was out of view (its workspace not shown on any monitor, or a fullscreen or maximized window in front of it). It kept drawing about once a second, and with vsync a draw to a window Hyprland is not showing waits for a frame callback that never comes, so the window stopped answering the compositor. The window now follows Hyprland's event socket, asks `j/clients` and `j/monitors` on every event that can move or cover it, and asks for no frames while it is hidden; it draws again at once when it is shown. Vsync and the frame rate are unchanged, and the collector keeps recording. Outside Hyprland nothing changes. If the compositor cannot be asked, the window counts as shown.

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

[Unreleased]: https://github.com/Raed-Alghamdi/hwglance/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/Raed-Alghamdi/hwglance/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Raed-Alghamdi/hwglance/releases/tag/v0.1.0
