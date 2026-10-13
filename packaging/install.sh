#!/usr/bin/env bash
# Build HW Glance and install it for the current user, plus the optional root helper.
#   packaging/install.sh [--no-helper]
# Root steps (only the helper) run through ${SUDO:-sudo}, for example SUDO=doas packaging/install.sh.
# Reverse with packaging/uninstall.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$HOME/.local/bin"
APPS="$HOME/.local/share/applications"
ICONS="$HOME/.local/share/icons/hicolor/scalable/apps"
UNITS="$HOME/.config/systemd/user"
HELPER_BIN=/usr/local/libexec/hwglance-helper
HELPER_UNIT=/etc/systemd/system/hwglance-helper.service
PLUGIN_ID=raed.hwglance
PLUGIN_DST="$HOME/.config/omarchy/plugins/$PLUGIN_ID"

with_helper=1
for arg in "$@"; do
    case "$arg" in
        --no-helper) with_helper=0 ;;
        -h|--help) sed -n '2,5p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

die() { echo "install.sh: $*" >&2; exit 1; }

# Prerequisites, checked before anything is built or written.
[[ $EUID -ne 0 ]] || die "run this as your normal user, not as root (the helper step asks for root itself)"
command -v cargo >/dev/null || die "cargo not found. Install Rust 1.85 or newer (https://rustup.rs or your distribution's rust package)."
command -v systemctl >/dev/null || die "systemctl not found. HW Glance needs systemd."
systemctl --user show-environment >/dev/null 2>&1 \
    || die "the systemd user manager is not reachable (systemctl --user failed). Run this from a normal login session."
read -ra SUDO_CMD <<<"${SUDO:-sudo}"
if [[ $with_helper -eq 1 ]]; then
    command -v "${SUDO_CMD[0]}" >/dev/null \
        || die "'${SUDO_CMD[0]}' not found for the root helper. Install it, set SUDO=<command>, or pass --no-helper."
fi

# The Omarchy bar widget is only installed on an Omarchy system.
omarchy=0
if command -v omarchy-shell >/dev/null 2>&1 || [[ -d /usr/share/omarchy ]]; then omarchy=1; fi

done_list=()
say() { done_list+=("$1"); }

cd "$ROOT"
echo "Building release binaries (low priority)..."
low_priority=(nice -n 19)
command -v ionice >/dev/null && low_priority+=(ionice -c3)
"${low_priority[@]}" cargo build --release
say "built target/release/{hwglance,hwglanced,hwglance-helper}"

install -Dm755 target/release/hwglance "$BIN/hwglance"
install -Dm755 target/release/hwglanced "$BIN/hwglanced"
say "installed hwglance and hwglanced to $BIN"

if [[ -f packaging/hwglance.desktop ]]; then
    install -Dm644 packaging/hwglance.desktop "$APPS/hwglance.desktop"
    command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" || true
    say "installed hwglance.desktop to $APPS"
else
    say "skipped hwglance.desktop (packaging/hwglance.desktop not found)"
fi

if [[ -f packaging/hwglance.svg ]]; then
    install -Dm644 packaging/hwglance.svg "$ICONS/hwglance.svg"
    install -Dm644 packaging/hwglance-symbolic.svg "$HOME/.local/share/icons/hicolor/symbolic/apps/hwglance-symbolic.svg"
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
    say "installed the app icon hwglance.svg to $ICONS"
fi
if compgen -G "packaging/icons/hicolor/*/apps/hwglance.png" >/dev/null; then
    for png in packaging/icons/hicolor/*/apps/hwglance.png; do
        size="$(basename "$(dirname "$(dirname "$png")")")"
        install -Dm644 "$png" "$HOME/.local/share/icons/hicolor/$size/apps/hwglance.png"
    done
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
    say "installed the app icon as PNG (16 to 512 px) under ~/.local/share/icons/hicolor"
fi

# Bar widget (Omarchy only): copied, not symlinked, so `omarchy plugin update` (git checkouts only) skips it.
if [[ $omarchy -eq 1 && -d omarchy-plugin/$PLUGIN_ID ]]; then
    changed=1
    if [[ -d $PLUGIN_DST ]] && diff -rq "omarchy-plugin/$PLUGIN_ID" "$PLUGIN_DST" >/dev/null 2>&1; then changed=0; fi
    if [[ $changed -eq 1 ]]; then
        rm -rf "$PLUGIN_DST"
        mkdir -p "$(dirname "$PLUGIN_DST")"
        cp -a "omarchy-plugin/$PLUGIN_ID" "$PLUGIN_DST"
    fi
    command -v omarchy-shell >/dev/null && omarchy-shell shell rescanPlugins >/dev/null 2>&1 || true
    if [[ $changed -eq 1 ]] && command -v omarchy-shell >/dev/null && omarchy-shell shell ping >/dev/null 2>&1; then
        # The shell's hot reload keeps serving its cached copy of changed widget QML,
        # so a running shell needs a restart to show the new version (notification
        # history is saved to disk and survives it).
        omarchy restart shell >/dev/null 2>&1 || true
        say "installed the bar widget to $PLUGIN_DST and restarted the Omarchy shell to load it"
    else
        say "bar widget at $PLUGIN_DST is current (first time: omarchy plugin enable $PLUGIN_ID)"
    fi
else
    say "skipped the Omarchy bar widget (Omarchy not found)"
fi

install -Dm644 packaging/hwglanced.service "$UNITS/hwglanced.service"
systemctl --user daemon-reload
systemctl --user enable hwglanced.service
# restart (not just start) so an update takes effect
systemctl --user restart hwglanced.service
say "installed, enabled and (re)started the user unit hwglanced.service"

if [[ $with_helper -eq 1 ]]; then
    "${SUDO_CMD[@]}" install -Dm755 -o root -g root target/release/hwglance-helper "$HELPER_BIN"
    "${SUDO_CMD[@]}" install -Dm644 -o root -g root packaging/hwglance-helper.service "$HELPER_UNIT"
    "${SUDO_CMD[@]}" systemctl daemon-reload
    "${SUDO_CMD[@]}" systemctl enable hwglance-helper.service
    "${SUDO_CMD[@]}" systemctl restart hwglance-helper.service
    say "installed the root helper to $HELPER_BIN and enabled hwglance-helper.service"
else
    say "skipped the root helper (--no-helper): CPU power is estimated, no memory speed"
fi

echo
echo "Done:"
for line in "${done_list[@]}"; do echo "  - $line"; done
echo
case ":$PATH:" in
    *":$BIN:"*) ;;
    *) echo "Note: $BIN is not in your PATH. Add it to start hwglance from a terminal." ;;
esac
echo "Optional: copy packaging/config.example.toml to ~/.config/hwglance/config.toml to rename sensors or turn on the plain-text readouts."
