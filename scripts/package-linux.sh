#!/usr/bin/env bash
set -euo pipefail

# Build on Ubuntu 24.04 amd64 so the package keeps that ABI floor.
# Packaging is also used for CI previews; it does not publish a release.
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
cd -- "$root"
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || {
    echo 'Build the Ubuntu package on Linux x86_64.' >&2
    exit 1
}
for tool in cargo python3 dpkg dpkg-deb desktop-file-validate; do
    command -v "$tool" >/dev/null || { echo "Missing build tool: $tool" >&2; exit 1; }
done

version=$(python3 -c 'import tomllib; print(tomllib.load(open("apps/linux/Cargo.toml", "rb"))["package"]["version"])')
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([~+.-][A-Za-z0-9.+~-]+)?$ ]] || {
    echo 'Invalid package version.' >&2
    exit 1
}
# Debian sorts '~' before a final release; a SemVer '-' would instead become
# a revision and make the beta newer than the corresponding stable package.
deb_version=${version/-/"~"}
if [[ "$version" == *-* ]]; then
    dpkg --compare-versions "$deb_version" lt "${version%%-*}" || {
        echo 'Prerelease package must sort before its stable version.' >&2
        exit 1
    }
fi
cargo build --release --locked --manifest-path apps/linux/Cargo.toml
cargo build --release --locked --manifest-path crates/tidemark-agent/Cargo.toml --bin island-hook

stage=$(mktemp -d "${TMPDIR:-/tmp}/tidemark-deb.XXXXXXXX")
cleanup() { rm -rf -- "$stage"; }
trap cleanup EXIT
mkdir -p -- "$stage/DEBIAN" "$root/dist"
install -Dm755 apps/linux/target/release/tidemark "$stage/usr/bin/tidemark"
install -Dm755 crates/tidemark-agent/target/release/island-hook "$stage/usr/bin/island-hook"
install -Dm644 installer/linux/io.github.treyzar.Tidemark.desktop \
    "$stage/usr/share/applications/io.github.treyzar.Tidemark.desktop"
desktop-file-validate "$stage/usr/share/applications/io.github.treyzar.Tidemark.desktop"
install -Dm644 assets/brand/tidemark.svg \
    "$stage/usr/share/icons/hicolor/scalable/apps/io.github.treyzar.Tidemark.svg"
install -Dm644 docs/ubuntu/architecture.md "$stage/usr/share/doc/tidemark/architecture.md"

# GNOME integration is part of the package, rather than a silently omitted
# optional file. Installing it never enables it or changes desktop settings.
extension_source=gnome-extension/tidemark@treyzar.github.io
[[ -f "$extension_source/metadata.json" && -f "$extension_source/extension.js" ]] || {
    echo 'GNOME adapter is incomplete; refusing to package it.' >&2
    exit 1
}
uuid=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["uuid"])' "$extension_source/metadata.json")
[[ "$uuid" == tidemark@treyzar.github.io ]] || { echo 'Unexpected GNOME extension UUID.' >&2; exit 1; }
extension_dir="$stage/usr/share/gnome-shell/extensions/$uuid"
mkdir -p -- "$extension_dir"
cp -R -- "$extension_source/." "$extension_dir/"
if [[ -d "$extension_dir/schemas" ]]; then
    glib-compile-schemas --strict "$extension_dir/schemas"
fi

sed "s/@VERSION@/$deb_version/g" installer/linux/control.in > "$stage/DEBIAN/control"
chmod 0755 "$stage/DEBIAN"
chmod 0644 "$stage/DEBIAN/control"
output="$root/dist/tidemark-${version}-ubuntu-amd64.deb"
dpkg-deb --root-owner-group --build "$stage" "$output"
dpkg-deb --info "$output"
echo "Ubuntu package: $output"
