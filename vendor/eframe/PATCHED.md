# Patched copy of eframe

This is eframe 0.36.2 from crates.io, unchanged except for the eframe files of egui pull request
[#8631](https://github.com/emilk/egui/pull/8631), "eframe: keep running on Wayland when frame
callbacks stop" (merge commit `85de2e4a1`, merged 2026-10-06, not yet in a release). The rest of
the egui crates are the unpatched 0.36.2 from crates.io.

Why: on Hyprland with the NVIDIA driver, a window on a workspace no monitor shows, or under a
fullscreen or maximized window, stopped getting Wayland frame callbacks. The glow backend then
blocked forever in a vsync `eglSwapBuffers`, the window stopped answering the compositor's pings
and Hyprland offered to terminate it. A window that started on a hidden workspace blocked on its
second frame.

What changed (`src/native/`, applied from the PR's diff; the line numbers only differ by offsets
between 0.36.2 and the PR's merge base, so no hunk needed editing):

- `glow_integration.rs`: calls `Window::pre_present_notify` before swapping, so winit holds back
  `RedrawRequested` until the compositor's frame callback arrives; swaps with interval 0 on
  Wayland (the frame callbacks pace it, so `glow_options.vsync` has no effect there); takes a
  `PassMode`.
- `run.rs`: if a requested redraw does not arrive within 250 ms (`MISSED_REDRAW_TIMEOUT`), runs
  `App::logic` without an egui pass or painting, until the compositor sends a frame callback again.
- `wgpu_integration.rs`, `winit_integration.rs`: the new `PassMode` argument. HW Glance does not
  use the wgpu backend.

Not applied: the PR's three doc lines in `egui_glow/src/lib.rs` (that crate is not vendored).

Added here: `LICENSE-MIT` and `LICENSE-APACHE` (the crates.io package does not ship them), copied
from the egui repository, and this file.

When egui 0.37 is released: set `eframe = "0.37"` (and the other egui crates) in
`crates/hwglance/Cargo.toml`, delete the `[patch.crates-io]` section and `exclude = ["vendor"]`
in the root `Cargo.toml`, delete `vendor/`, run `cargo update` and `scripts/check.sh`.
