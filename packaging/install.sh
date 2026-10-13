#!/usr/bin/env bash
# Install HW Glance for the current user, plus the optional root helper. From a source checkout
# it builds release binaries first; from an unpacked release tarball (it holds bin/) it installs
# the prebuilt ones and needs no Rust.
#   packaging/install.sh [--no-helper] [--prebuilt] [--dry-run]
# --prebuilt installs from ./bin even in a checkout, --dry-run only prints what it would do.
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
prebuilt=0
dry_run=0
[[ -x $ROOT/bin/hwglance ]] && prebuilt=1
for arg in "$@"; do
    case "$arg" in
        --no-helper) with_helper=0 ;;
        --prebuilt) prebuilt=1 ;;
        --dry-run) dry_run=1 ;;
        -h|--help) sed -n '2,8p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

die() { echo "install.sh: $*" >&2; exit 1; }

# Every change goes through act, so --dry-run can print instead of doing.
act() {
    if [[ $dry_run -eq 1 ]]; then echo "[dry-run] $*"; else "$@"; fi
}

# Prerequisites, checked before anything is built or written.
[[ $EUID -ne 0 ]] || die "run this as your normal user, not as root (the helper step asks for root itself)"
if [[ $prebuilt -eq 1 ]]; then
    SRC_BIN="$ROOT/bin"
    for b in hwglance hwglanced $([[ $with_helper -eq 1 ]] && echo hwglance-helper); do
        [[ -x $SRC_BIN/$b ]] || die "$SRC_BIN/$b not found (--prebuilt needs the binaries from a release tarball)"
    done
    "$SRC_BIN/hwglanced" --version >/dev/null 2>&1 \
        || die "the prebuilt binaries do not run on this system (they target x86_64 Linux with glibc). Build from source instead: packaging/install.sh in a git checkout."
else
    SRC_BIN="$ROOT/target/release"
    command -v cargo >/dev/null || die "cargo not found. Install Rust 1.95 or newer (https://rustup.rs or your distribution's rust package)."
fi
command -v systemctl >/dev/null || die "systemctl not found. HW Glance needs systemd."
if [[ $dry_run -eq 0 ]]; then
    systemctl --user show-environment >/dev/null 2>&1 \
        || die "the systemd user manager is not reachable (systemctl --user failed). Run this from a normal login session."
fi
read -ra SUDO_CMD <<<"${SUDO:-sudo}"
if [[ $with_helper -eq 1 && $dry_run -eq 0 ]]; then
    command -v "${SUDO_CMD[0]}" >/dev/null \
        || die "'${SUDO_CMD[0]}' not found for the root helper. Install it, set SUDO=<command>, or pass --no-helper."
fi

# The Omarchy bar widget is only installed on an Omarchy system.
omarchy=0
if command -v omarchy-shell >/dev/null 2>&1 || [[ -d /usr/share/omarchy ]]; then omarchy=1; fi

done_list=()
say() { done_list+=("$1"); }

# The unit files in packaging/ carry the packaged paths (/usr/bin, /usr/lib/hwglance); this
# install puts the binaries elsewhere, so the ExecStart line is rewritten on the way.
rendered=$(mktemp -d)
trap 'rm -rf "$rendered"' EXIT
render_unit() { # <unit file> <ExecStart value>
    sed "s|^ExecStart=.*|ExecStart=$2|" "$ROOT/packaging/$1" >"$rendered/$1"
    grep -qxF "ExecStart=$2" "$rendered/$1" || die "could not set ExecStart in $1"
}

cd "$ROOT"
if [[ $prebuilt -eq 1 ]]; then
    say "using the prebuilt binaries in $SRC_BIN"
else
    echo "Building release binaries (low priority)..."
    low_priority=(nice -n 19)
    command -v ionice >/dev/null && low_priority+=(ionice -c3)
    act "${low_priority[@]}" cargo build --release
    say "built target/release/{hwglance,hwglanced,hwglance-helper}"
fi

act install -Dm755 "$SRC_BIN/hwglance" "$BIN/hwglance"
act install -Dm755 "$SRC_BIN/hwglanced" "$BIN/hwglanced"
say "installed hwglance and hwglanced to $BIN"

if [[ -f packaging/hwglance.desktop ]]; then
    act install -Dm644 packaging/hwglance.desktop "$APPS/hwglance.desktop"
    command -v update-desktop-database >/dev/null && act update-desktop-database "$APPS" || true
    say "installed hwglance.desktop to $APPS"
else
    say "skipped hwglance.desktop (packaging/hwglance.desktop not found)"
fi

if [[ -f packaging/hwglance.svg ]]; then
    act install -Dm644 packaging/hwglance.svg "$ICONS/hwglance.svg"
    act install -Dm644 packaging/hwglance-symbolic.svg "$HOME/.local/share/icons/hicolor/symbolic/apps/hwglance-symbolic.svg"
    command -v gtk-update-icon-cache >/dev/null && act gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
    say "installed the app icon hwglance.svg to $ICONS"
fi
if compgen -G "packaging/icons/hicolor/*/apps/hwglance.png" >/dev/null; then
    for png in packaging/icons/hicolor/*/apps/hwglance.png; do
        size="$(basename "$(dirname "$(dirname "$png")")")"
        act install -Dm644 "$png" "$HOME/.local/share/icons/hicolor/$size/apps/hwglance.png"
    done
    command -v gtk-update-icon-cache >/dev/null && act gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
    say "installed the app icon as PNG (16 to 512 px) under ~/.local/share/icons/hicolor"
fi

# Bar widget (Omarchy only): packaging/hwglance-bar-widget is the one implementation, shared
# with the Arch package. It copies the widget (so `omarchy plugin update` skips it), enables it
# and restarts the shell only when the files changed.
if [[ $omarchy -eq 1 && -d omarchy-plugin/$PLUGIN_ID ]]; then
    act env HWGLANCE_PLUGIN_SRC="$ROOT/omarchy-plugin/$PLUGIN_ID" "$ROOT/packaging/hwglance-bar-widget" install
    say "installed and enabled the Omarchy bar widget (packaging/hwglance-bar-widget install)"
else
    say "skipped the Omarchy bar widget (Omarchy not found)"
fi

render_unit hwglanced.service "%h/.local/bin/hwglanced"
act install -Dm644 "$rendered/hwglanced.service" "$UNITS/hwglanced.service"
act systemctl --user daemon-reload
act systemctl --user enable hwglanced.service
# restart (not just start) so an update takes effect
act systemctl --user restart hwglanced.service
say "installed, enabled and (re)started the user unit hwglanced.service"

if [[ $with_helper -eq 1 ]]; then
    render_unit hwglance-helper.service "$HELPER_BIN"
    act "${SUDO_CMD[@]}" install -Dm755 -o root -g root "$SRC_BIN/hwglance-helper" "$HELPER_BIN"
    act "${SUDO_CMD[@]}" install -Dm644 -o root -g root "$rendered/hwglance-helper.service" "$HELPER_UNIT"
    act "${SUDO_CMD[@]}" systemctl daemon-reload
    act "${SUDO_CMD[@]}" systemctl enable hwglance-helper.service
    act "${SUDO_CMD[@]}" systemctl restart hwglance-helper.service
    say "installed the root helper to $HELPER_BIN and enabled hwglance-helper.service"
else
    say "skipped the root helper (--no-helper): CPU power is estimated, no memory speed"
fi

echo
[[ $dry_run -eq 1 ]] && echo "Dry run: nothing was changed. Summary of the plan:" || true
echo "Done:"
for line in "${done_list[@]}"; do echo "  - $line"; done
echo
case ":$PATH:" in
    *":$BIN:"*) ;;
    *) echo "Note: $BIN is not in your PATH. Add it to start hwglance from a terminal." ;;
esac
echo "Optional: copy packaging/config.example.toml to ~/.config/hwglance/config.toml to rename sensors or turn on the plain-text readouts."
