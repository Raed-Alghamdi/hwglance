#!/usr/bin/env bash
# Build the generic x86_64 Linux release tarball:
#   scripts/dist.sh
# Writes dist/hwglance-<version>-x86_64-linux.tar.gz and a .sha256 file beside it. The tarball
# holds prebuilt binaries in bin/, and install.sh / uninstall.sh that install them for the
# current user (no Rust needed). SKIP_BUILD=1 reuses target/release (used by the release workflow
# after its test run built it).
set -euo pipefail
cd "$(dirname "$0")/.."

[[ $(uname -m) == x86_64 ]] || { echo "dist.sh: builds the x86_64 tarball, this machine is $(uname -m)" >&2; exit 1; }

version="$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version = "\(.*\)"/\1/p}' Cargo.toml)"
[[ -n $version ]] || { echo "dist.sh: no workspace version in Cargo.toml" >&2; exit 1; }
name="hwglance-$version-x86_64-linux"
out="dist/$name"

if [[ ${SKIP_BUILD:-0} != 1 ]]; then
    echo "== cargo build --release --locked"
    low_priority=(nice -n 19)
    command -v ionice >/dev/null && low_priority+=(ionice -c3)
    "${low_priority[@]}" cargo build --release --locked
fi
for b in hwglance hwglanced hwglance-helper; do
    [[ -x target/release/$b ]] || { echo "dist.sh: target/release/$b is missing" >&2; exit 1; }
done

rm -rf "$out" "$out.tar.gz" "$out.tar.gz.sha256"
mkdir -p "$out/bin" "$out/packaging" "$out/licenses/fonts" "$out/omarchy-plugin"

install -m755 target/release/{hwglance,hwglanced,hwglance-helper} "$out/bin/"

# The packaging directory keeps its repository layout, so packaging/install.sh works in place.
cp -a packaging/. "$out/packaging/"
rm -rf "$out/packaging/arch"
# Top-level entry points; install.sh sees bin/ and installs the prebuilt binaries.
for script in install uninstall; do
    printf '#!/bin/sh\nexec "$(dirname "$0")/packaging/%s.sh" "$@"\n' "$script" >"$out/$script.sh"
    chmod 755 "$out/$script.sh"
done

cp -a omarchy-plugin/raed.hwglance "$out/omarchy-plugin/"
install -m644 README.md LICENSE "$out/"
install -m644 packaging/config.example.toml "$out/config.example.toml"
install -m644 omarchy-plugin/raed.hwglance/LICENSE "$out/licenses/LICENSE-omarchy-widget-MIT"
install -m644 crates/hwglance/assets/fonts/{README.md,OFL-*.txt,LICENSE-*.txt} "$out/licenses/fonts/"

# Reproducible archive: sorted, fixed owner, commit time as mtime.
mtime="$(git log -1 --format=%ct 2>/dev/null || date +%s)"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$mtime" \
    -C dist -cf - "$name" | gzip -9n >"$out.tar.gz"
(cd dist && sha256sum "$name.tar.gz" >"$name.tar.gz.sha256")
rm -rf "$out"

echo "== dist/$name.tar.gz"
ls -l "dist/$name.tar.gz" "dist/$name.tar.gz.sha256"
