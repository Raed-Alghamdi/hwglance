# HW Glance: notes for AI coding agents

## Layout

- `crates/hwglanced`: the collector (daemon). `crates/hwglance`: the window (egui). `crates/hwglance-proto`: wire types shared by both. `crates/hwglance-helper`: the small root helper.
- `omarchy-plugin/raed.hwglance`: the optional Omarchy bar widget (QML). `omarchy-plugin/harness`: its developer tooling, not installed.
- `packaging/`: install and uninstall scripts, `hwglance-bar-widget` (copies the widget into `~/.config/omarchy/plugins`), systemd units (they carry the packaged `/usr` paths; `install.sh` rewrites `ExecStart` for the per-user install), `config.example.toml`, icons. `packaging/arch/`: the Arch `PKGBUILD` and `hwglance.install`. `scripts/dist.sh`: the generic release tarball.
- Maintainer runbook: `MAINTAINING.md`. Security policy: `SECURITY.md`. Dependency policy: `deny.toml` (`cargo deny check`).
- Context: `PRODUCT.md` (who and why), `DESIGN.md` (tokens in its frontmatter, implemented in `crates/hwglance/src/theme.rs`), `README.md` (user docs).

## Commands

- Run the window with `cargo run -p hwglance` (live data needs the `hwglanced` user service; add `-- --demo` for demo data). One collector sample: `cargo run -p hwglanced -- --once --json`.
- Check with `scripts/check.sh`: tests (renders and golden snapshots included), clippy, and the bar widget model checks (Node.js). Run it before reporting a change as done.
- Install with `packaging/install.sh`; its root steps use `${SUDO:-sudo}`. `--dry-run` prints the plan only; `--prebuilt` (automatic inside a release tarball) skips cargo.
- Builds are heavy. Run cargo at low priority (`nice -n 19 ionice -c3 cargo ...`, as `install.sh` does) and one cargo command at a time.

## Releasing

1. Set the workspace version in `Cargo.toml` (and the widget's `manifest.json`), update `Cargo.lock`, move `CHANGELOG.md`'s Unreleased entries into a new version section, write `.github/release-notes/vX.Y.Z.md`, and commit. Do this before tagging.
2. Tag and push: `git tag vX.Y.Z && git push origin vX.Y.Z`. `.github/workflows/release.yml` tests, builds the tarball (`scripts/dist.sh`) and the Arch package, and creates the GitHub release with both, `SHA256SUMS`, and the finalized `PKGBUILD` and `hwglance.install`. It fails when the tag and the workspace version differ.
3. Copy `pkgver` and the `sha256sums` line from the released `PKGBUILD` into `packaging/arch/PKGBUILD` (the committed file keeps `sha256sums=('SKIP')` until then) and commit. Publish that file to the AUR once an AUR package exists.
4. To test the package locally, run `makepkg` in a scratch directory with a PKGBUILD whose `source` points at a `git archive --prefix=hwglance-X.Y.Z/ HEAD` tarball.

## Goldens and screenshots

- The images in `crates/hwglance/tests/snapshots/` are GPU-rendered. After an intended UI change, refresh them with `UPDATE_SNAPSHOTS=1 cargo test -p hwglance --test snapshots` and review the PNG diff. `HWGLANCE_SKIP_GOLDEN=1` skips the comparison without a usable GPU (CI sets it).
- `HWGLANCE_SHOTS=<dir>` makes the render tests also write PNGs. `scripts/readme-shots.sh` regenerates `docs/screenshots/`.

## Rules

- Sensors stay read-only: nothing in this project may write to hardware.
- No em dashes in code, UI text or docs.
- When behaviour changes, update `README.md`, `PRODUCT.md` and `packaging/config.example.toml` in the same change, and say each fact in one place.
