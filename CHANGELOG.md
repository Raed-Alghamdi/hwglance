# Changelog

All notable changes to HW Glance are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `hwglance --version` (the collector already had it).
- Dependency checks with `cargo-deny` (`deny.toml`): advisories, licenses, duplicate versions and sources. They run in CI on every push and weekly in `.github/workflows/audit.yml`.
- Dependabot version updates for Cargo and GitHub Actions (monthly).
- `SECURITY.md`, `MAINTAINING.md`, issue forms and a pull request template.

### Changed

- GitHub Actions are pinned to commit SHAs, on Node 24 versions. Workflows run with read-only permissions, and CI cancels superseded runs.
- The workspace crates are marked `publish = false`.

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

[Unreleased]: https://github.com/Raed-Alghamdi/hwglance/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Raed-Alghamdi/hwglance/releases/tag/v0.1.0
