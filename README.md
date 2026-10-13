<p align="center">
  <img src="docs/logo.svg" alt="HW Glance logo: three horizontal result bars in blue, violet and cyan on a dark rounded square" width="96" height="96">
</p>

# HW Glance

HW Glance is a hardware monitor for the Linux desktop. It puts every sensor your machine exposes (CPU, GPU, memory, drives, motherboard) and a Task Manager style process list in one window, laid out so a value can be found in about a second. A small background collector keeps min, max, average and a short history of every reading, and the window and an optional bar widget both read from it.

![HW Glance main window, dark theme](docs/screenshots/main-dark.png)

The screenshots in this README are rendered from built-in demo data, not from a real machine.

## Features

- Per-device cards for CPU, GPU, memory, each drive, motherboard and integrated GPU. Every reading shows current, min, max and average, with a range bar against its own scale.
- Warning and critical limits with flag letters (H, HH, L, LL) and colours, kept for every reading in a plain file (`thresholds.json`) you can edit by hand or from the window, and desktop notifications that open the window at the value.
- Thermal and power throttling shown as Yes/No on the CPU and GPU cards, with a count and the time of the last event.
- A history for every value (30 minutes by default), shown in a popover on hover.
- A process list grouped like Task Manager (Apps, Background, System), sortable by CPU, GPU, VRAM and memory, with per-process GPU use. End task, and a confirmed Force kill.
- A process panel that docks at the bottom or on the right, each with a compact and an expanded view, a column picker and a resizable name column.
- Watched values, hidden values, renamed sensors, a drag-and-drop card layout, dark and light themes that follow the desktop.
- Light on the machine: the collector and the window each stay well under 2% of one core.

| Light theme | Process panel docked on the right |
|---|---|
| ![Light theme](docs/screenshots/main-light.png) | ![Compact process panel on the right](docs/screenshots/dock-right-compact.png) |

## Requirements

Required:

- Linux with systemd (the collector runs as a systemd user service).
- A desktop session with OpenGL on Wayland or X11.
- Rust 1.95 or newer to build (`rustup` or your distribution's `rust` package).

Optional, each one adds readings or features:

| Component | What it adds |
|---|---|
| NVIDIA driver (NVML) | GPU load, temperature, clocks, VRAM use, power and fan on NVIDIA cards |
| [LACT](https://github.com/ilya-zlobintsev/LACT) daemon | GPU hotspot and VRAM temperatures, fan RPM and core voltage, which the NVIDIA driver does not expose on Linux |
| The root helper (`hwglance-helper`, installed by default) | CPU package power from RAPL and memory speed and module details from SMBIOS, both of which only root can read |
| Kernel hwmon drivers for your board (for example `nct6775`, `asus-ec-sensors`) | Motherboard temperatures and fans |
| Hyprland | Window titles in the process list, and the maximized-window state |
| Omarchy | The bar widget, and the window following the Omarchy theme and font |
| polkit and `pkexec` | Ending processes that belong to other users |
| An Arabic font | Arabic window titles and process names (the text is shaped and ordered correctly either way) |

## Build and install

```sh
git clone https://github.com/Raed-Alghamdi/hwglance.git
cd hwglance
packaging/install.sh
```

The script checks its prerequisites, builds the release binaries at low priority, and installs:

- `hwglance` (the window) and `hwglanced` (the collector) to `~/.local/bin`, with a desktop entry and an icon;
- the `hwglanced` systemd user service, enabled and started;
- the root helper to `/usr/local/libexec` with its systemd unit, which needs root;
- the Omarchy bar widget, only if Omarchy is detected.

Options and variables:

- `packaging/install.sh --no-helper` skips the root helper. CPU power is then estimated and memory speed is not shown.
- `SUDO=doas packaging/install.sh` runs the root steps through another command (default `sudo`). Only the helper needs root.
- Make sure `~/.local/bin` is in your `PATH`.

Start the window from your launcher, or with `hwglance`. `hwglance --demo` runs it on demo data without the collector.

To remove everything the installer put in place (your config stays):

```sh
packaging/uninstall.sh            # add --no-helper to keep the root helper
```

## Configuration

Settings changed in the window (watched values, layout, theme) are saved to `~/.config/hwglance/ui.json`. The warning and critical limits are not: they live in `~/.config/hwglance/thresholds.json` (see Thresholds below).

Collector settings are optional and live in `~/.config/hwglance/config.toml`. Copy [`packaging/config.example.toml`](packaging/config.example.toml) as a starting point. It covers the sample intervals, renaming sensors with a `[labels]` table (keyed `chip/sensor`), hiding readings with `[hide]`, and the CPU limit below. `hwglanced --once --json` prints every reading id and sensor name your machine has.

### CPU thermal limit

The top-level key `cpu_thermal_limit_c` (40 to 120, in °C) sets the temperature at which the CPU lowers its clocks. The CPU "Thermal throttling" reading and the default CPU temperature limits use it. Without the key the limit is, in order:

1. the limit the driver reports (`temp*_crit`, else `temp*_max`);
2. for AMD CPUs, a built-in table of AMD's published Tjmax per Ryzen model (`amd_tjmax` in [`crates/hwglanced/src/cpu.rs`](crates/hwglanced/src/cpu.rs));
3. 95 °C.

The warning limit is the critical one minus 9 °C. Intel CPUs report real throttle events (the `thermal_throttle` counters). AMD CPUs do not report throttling to Linux, so it is inferred: "Yes" while the temperature is within half a degree of the limit.

### Thresholds

The warning and critical limits of every reading are kept in `~/.config/hwglance/thresholds.json` (under `$XDG_CONFIG_HOME` when that is set). Only the collector writes this file. It creates it on first start with every reading that can have a limit, so you can open it and change a number:

```json
{
  "version": 1,
  "readings": {
    "cpu/temp/tctl": {
      "name": "CPU · Temperatures · Tctl",
      "unit": "°C",
      "warn": 80,
      "crit": 89,
      "below": false,
      "default": { "warn": 80, "crit": 89 }
    },
    "board/fan/nct6798/fan1": {
      "name": "Motherboard · Fans · Pump",
      "unit": "RPM",
      "warn": 1500,
      "crit": 800,
      "below": true,
      "default": { "warn": null, "crit": null }
    }
  }
}
```

- `warn` and `crit` are numbers in the unit shown (GiB for memory amounts, MB/s for transfer rates), or `null` for no limit at that level. A value at or above `warn` is a warning, at or above `crit` critical. With `"below": true` the direction flips: at or below the limit, for fans and pumps. To add a limit to a reading that has none, replace `null` with a number.
- `name`, `unit` and `default` are for your eyes: the collector refreshes them and ignores them when it reads the file.
- An entry whose `warn`, `crit` and `below` equal the defaults is "default". Anything else replaces the defaults as a whole, everywhere: the colours and flag letters in the window and the bar widget, the notifications, and the crossings list. A level you leave `null` has no limit, even where the default has one. Delete an entry to give that reading its defaults back.
- The collector checks the file's modification time every 2 seconds and applies a valid edit without rewriting the file.
- A file that is invalid (broken JSON, a limit that is not a number, ...) is not applied and not overwritten. The last good limits stay in force, the window shows a banner (and Settings, Alerts shows the file's path and the error with its line and column), and the collector logs it. Fix the file and the next check clears the banner.
- The collector writes the file only to create it, to list readings that appear later (your values and the entries of readings that are not present, such as an unplugged drive, are kept), and when you change a limit in the window. The right-click menu of a value and Settings, Sensors send that change to the collector, which writes the same file. Settings, Alerts has an Open file button.
- Limits you set in `ui.json` with an older version are folded into this file once, the first time it is created.

## Optional extras

### Omarchy bar widget

[`omarchy-plugin/raed.hwglance`](omarchy-plugin/raed.hwglance) is a bar widget for the [Omarchy](https://omarchy.org) shell: an icon or live CPU and GPU usage in the bar, and a popup with key readings and the top processes. `packaging/install.sh` copies it into place on an Omarchy system; enable it once with `omarchy plugin enable raed.hwglance`. The plugin id is `author.name`, which is why it carries the author's handle. See its [README](omarchy-plugin/raed.hwglance/README.md).

## Privacy and safety

- Sensors are read only. HW Glance never writes fan, clock, power or any other hardware setting.
- Nothing leaves your machine. There is no network code in the window or the collector, and no telemetry. The collector serves the window and the bar widget over a Unix socket in `$XDG_RUNTIME_DIR`.
- The collector runs as your user. The only root part is `hwglance-helper`: it reads the SMBIOS tables once and the RAPL energy counters every second, and publishes one-second averages (finer readings are what power side channels need). It has no network, no child processes and no other writes, and its systemd unit is sandboxed (`ProtectSystem=strict`, `ProtectHome`, `PrivateNetwork`, `NoNewPrivileges`, an empty capability set and a system call filter, among others). Skip it with `--no-helper`.
- A process is only ended when you ask: End task asks first, Force kill asks again and names the target, and ending a process that belongs to another user goes through polkit. Processes the app knows to be critical to the session (the compositor, the audio server, the collector itself) carry an extra warning.

## Known limitations

- It has been tested on real hardware on one desktop only (AMD Ryzen with an NVIDIA GPU). Intel CPUs and discrete AMD GPUs are implemented but covered so far only by tests with fake sysfs data. Other hardware, laptops especially, may show missing or oddly named readings. Bug reports with the output of `hwglanced --once --json` are welcome.
- The window is English only.
- The bar widget and the Hyprland features are Omarchy and Hyprland specific. Other compositors work, but without window titles.

### Hardware notes

- Names come from the system: the CPU from `/proc/cpuinfo`, GPUs from the driver or the PCI ID database, the board and memory from SMBIOS/DMI, drives from sysfs.
- AMD integrated and discrete GPUs are told apart by a heuristic. A laptop discrete GPU that has only an edge sensor, no fan and under 3 GiB of VRAM can be taken for an integrated one, and there is no override yet. Only one integrated GPU gets a card.
- NVIDIA support covers the first NVML device only (no multi-NVIDIA setups). The nouveau and the old radeon drivers get no card.
- Intel GPUs show their name, hwmon temperatures, fan and GT clock, nothing more.
- AMD GPUs have no throttle state in sysfs, so they have no GPU Limits section. Throttling inferred on AMD CPUs can miss brief events.
- Super I/O chips other than `nct6*` show fans only, and chips with plain `tempN` labels (it87 and similar) show no board temperatures.

## Contributing

```sh
cargo run -p hwglance -- --demo     # the window on demo data
cargo run -p hwglanced -- --once --json   # one sample from the collector
scripts/check.sh                    # tests, clippy, bar widget model checks
```

- `scripts/check.sh` is the check to pass before a pull request. The golden image tests in `crates/hwglance/tests/snapshots/` are rendered on a GPU; if you change the UI on purpose, refresh them with `UPDATE_SNAPSHOTS=1 cargo test -p hwglance --test snapshots` and look at the PNG diff. `HWGLANCE_SKIP_GOLDEN=1` skips the comparison on a machine without a usable GPU.
- The README screenshots are regenerated from the demo data with `scripts/readme-shots.sh` (output in `docs/screenshots/`).
- `PRODUCT.md` and `DESIGN.md` describe who the app is for and the visual system (tokens, rules, layout). `CLAUDE.md` holds short notes for AI coding agents.
- Please do not use em dashes in code, UI text or docs.

## License

HW Glance is free software under the [GNU General Public License v3.0 or later](LICENSE). Copyright 2026 Raed Alghamdi.

The Omarchy bar widget in `omarchy-plugin/raed.hwglance` is under the MIT license (see the `LICENSE` file in that directory).

Bundled fonts (see [`crates/hwglance/assets/fonts`](crates/hwglance/assets/fonts), whose [README](crates/hwglance/assets/fonts/README.md) lists the third-party notices), all under the SIL Open Font License 1.1:

- [Atkinson Hyperlegible](https://www.brailleinstitute.org/freefont/), Copyright 2020 Braille Institute of America, Inc.
- AtkynsonMono Nerd Font Mono, a Nerd Fonts patched build of Atkinson Hyperlegible Mono (Braille Institute of America, Inc.), which also carries the icon glyphs of the [Nerd Fonts](https://www.nerdfonts.com/) project.

The Rust dependencies are under MIT, Apache-2.0, BSD, Zlib, ISC, MPL-2.0 and similar licenses that are compatible with the GPL; `cargo metadata` lists them.
