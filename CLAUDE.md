# HW Glance: notes for AI coding agents

## Layout

- `crates/hwglanced`: the collector (daemon). `crates/hwglance`: the window (egui). `crates/hwglance-proto`: wire types shared by both. `crates/hwglance-helper`: the small root helper.
- `omarchy-plugin/raed.hwglance`: the optional Omarchy bar widget (QML). `omarchy-plugin/harness`: its developer tooling, not installed.
- `packaging/`: install and uninstall scripts, systemd units, `config.example.toml`, icons.
- Context: `PRODUCT.md` (who and why), `DESIGN.md` (tokens in its frontmatter, implemented in `crates/hwglance/src/theme.rs`), `README.md` (user docs).

## Commands

- Run the window with `cargo run -p hwglance` (live data needs the `hwglanced` user service; add `-- --demo` for demo data). One collector sample: `cargo run -p hwglanced -- --once --json`.
- Check with `scripts/check.sh`: tests (renders and golden snapshots included), clippy, and the bar widget model checks (Node.js). Run it before reporting a change as done.
- Install with `packaging/install.sh`; its root steps use `${SUDO:-sudo}`.
- Builds are heavy. Run cargo at low priority (`nice -n 19 ionice -c3 cargo ...`, as `install.sh` does) and one cargo command at a time.

## Goldens and screenshots

- The images in `crates/hwglance/tests/snapshots/` are GPU-rendered. After an intended UI change, refresh them with `UPDATE_SNAPSHOTS=1 cargo test -p hwglance --test snapshots` and review the PNG diff. `HWGLANCE_SKIP_GOLDEN=1` skips the comparison without a usable GPU (CI sets it).
- `HWGLANCE_SHOTS=<dir>` makes the render tests also write PNGs. `scripts/readme-shots.sh` regenerates `docs/screenshots/`.

## Rules

- Sensors stay read-only: nothing in this project may write to hardware.
- No em dashes in code, UI text or docs.
- When behaviour changes, update `README.md`, `PRODUCT.md` and `packaging/config.example.toml` in the same change, and say each fact in one place.
