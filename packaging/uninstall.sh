#!/usr/bin/env bash
# Remove everything packaging/install.sh installed.
#   packaging/uninstall.sh [--no-helper]
# Leaves ~/.config/hwglance (your config) alone. Root steps (only the helper) run through
# ${SUDO:-sudo}, for example SUDO=doas packaging/uninstall.sh.
set -euo pipefail

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

die() { echo "uninstall.sh: $*" >&2; exit 1; }

[[ $EUID -ne 0 ]] || die "run this as your normal user, not as root (the helper step asks for root itself)"
command -v systemctl >/dev/null || die "systemctl not found. HW Glance needs systemd."
read -ra SUDO_CMD <<<"${SUDO:-sudo}"
if [[ $with_helper -eq 1 && ( -e $HELPER_UNIT || -e $HELPER_BIN ) ]]; then
    command -v "${SUDO_CMD[0]}" >/dev/null \
        || die "'${SUDO_CMD[0]}' not found for the root helper. Install it, set SUDO=<command>, or pass --no-helper."
fi

done_list=()
say() { done_list+=("$1"); }

systemctl --user disable --now hwglanced.service 2>/dev/null || true
rm -f "$UNITS/hwglanced.service"
systemctl --user daemon-reload 2>/dev/null || true
say "stopped, disabled and removed the user unit hwglanced.service"

rm -f "$BIN/hwglance" "$BIN/hwglanced"
say "removed hwglance and hwglanced from $BIN"

if [[ -f "$APPS/hwglance.desktop" ]]; then
    rm -f "$APPS/hwglance.desktop"
    command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" || true
    say "removed hwglance.desktop"
fi

if compgen -G "$HOME/.local/share/icons/hicolor/*x*/apps/hwglance.png" >/dev/null; then
    rm -f "$HOME"/.local/share/icons/hicolor/*x*/apps/hwglance.png
    say "removed the PNG app icons"
fi
if [[ -f "$ICONS/hwglance.svg" ]]; then
    rm -f "$ICONS/hwglance.svg" "$HOME/.local/share/icons/hicolor/symbolic/apps/hwglance-symbolic.svg"
    say "removed the app icon hwglance.svg"
fi

if [[ -d $PLUGIN_DST ]]; then
    command -v omarchy >/dev/null && omarchy plugin disable "$PLUGIN_ID" >/dev/null 2>&1 || true
    rm -rf "$PLUGIN_DST"
    say "disabled and removed the Omarchy bar widget $PLUGIN_ID"
fi

if [[ $with_helper -eq 1 ]]; then
    if [[ -e $HELPER_UNIT || -e $HELPER_BIN ]]; then
        "${SUDO_CMD[@]}" systemctl disable --now hwglance-helper.service || true
        "${SUDO_CMD[@]}" rm -f "$HELPER_UNIT" "$HELPER_BIN"
        "${SUDO_CMD[@]}" systemctl daemon-reload
        say "stopped and removed the root helper and hwglance-helper.service"
    else
        say "root helper was not installed"
    fi
else
    say "kept the root helper (--no-helper)"
fi

echo "Done:"
for line in "${done_list[@]}"; do echo "  - $line"; done
