# Test icons

App icons for the golden renders (`tests/snapshots/`) and the README screenshots
(`docs/screenshots/`). The render tests point `HWGLANCE_ICON_DIRS` at this directory, which
replaces the icon search roots and turns off the `/usr/share/pixmaps` fallback, so the renders do
not depend on the icon themes installed on the machine. Not shipped in the app.

- Source: [Papirus icon theme](https://github.com/PapirusDevelopmentTeam/papirus-icon-theme),
  Arch package `papirus-icon-theme` 20260801-1, files copied unchanged from `64x64/apps/`
  (symlinks resolved).
- License: GPL-3.0, the same license as this repository (see the root `LICENSE`).
- Trademarks: the logos depicted belong to their respective owners and are used here only to
  illustrate the demo data.

Files in `hicolor/scalable/apps/`:

| File | Used for |
|---|---|
| `brave-browser.svg` | Brave (demo), Brave Origin (`fixtures/snapshot.json`) |
| `steam.svg` | Steam |
| `applications-games.svg` | Elden Ring (demo) |
| `foot.svg` | foot |
| `code.svg` | Code (demo) |
| `vscode.svg` | Code (`fixtures/snapshot.json`; identical to `code.svg`) |
| `zed.svg` | Zed |
| `discord.svg` | Discord |
| `org.gnome.Nautilus.svg` | Files |
| `mpv.svg` | mpv |
| `org.telegram.desktop.svg` | Telegram |
| `spotify-client.svg` | Spotify (`fixtures/snapshot.json`) |
