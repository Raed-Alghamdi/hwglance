# HW Glance: notes for AI coding agents

- Layout: `crates/hwglanced` (collector daemon), `crates/hwglance` (egui window), `crates/hwglance-proto` (wire types), `crates/hwglance-helper` (small root helper), `omarchy-plugin/raed.hwglance` (optional Omarchy bar widget, QML), `packaging/` (install script, systemd units).
- Product and design context: `PRODUCT.md` and `DESIGN.md` (tokens in its frontmatter, implemented in `crates/hwglance/src/theme.rs`).
- Run the window with `cargo run -p hwglance` (live data needs the `hwglanced` user service; `-- --demo` uses demo data). Install everything with `packaging/install.sh`; root steps use `${SUDO:-sudo}`.
- Check with `scripts/check.sh` (tests including renders and snapshots, clippy, bar widget model checks). `HWGLANCE_SHOTS=<dir>` writes the test renders as PNGs.
- Golden images in `crates/hwglance/tests/snapshots/` are GPU-rendered. Refresh them with `UPDATE_SNAPSHOTS=1 cargo test -p hwglance --test snapshots` and review the PNG diff. README screenshots are regenerated with `scripts/readme-shots.sh`.
- Keep sensors read-only: nothing in this project may write to hardware.
- No em dashes in code, UI text or docs.
