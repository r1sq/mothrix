#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
# Build a Debian package from the release binary: target/mothrix_<version>_<arch>.deb
set -eu
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
homepage=$(sed -n 's/^repository = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(dpkg --print-architecture)
binary=${1:-target/release/mothrix}
[ -x "$binary" ] || { echo "build first: just deb (MOTHRIX_PACKAGED=1 cargo build --release)" >&2; exit 1; }

root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
install -Dm755 "$binary" "$root/usr/bin/mothrix"
install -Dm644 docs/mothrix.1 "$root/usr/share/man/man1/mothrix.1"
gzip -9n "$root/usr/share/man/man1/mothrix.1"
install -Dm644 packaging/mothrix.desktop "$root/usr/share/applications/mothrix.desktop"
install -Dm644 assets/mothrix-512.png "$root/usr/share/icons/hicolor/512x512/apps/mothrix.png"
install -Dm644 README.md "$root/usr/share/doc/mothrix/README.md"
for doc in features usage configuration install updates; do
  install -Dm644 "docs/$doc.md" "$root/usr/share/doc/mothrix/$doc.md"
done
install -Dm644 LICENSE "$root/usr/share/doc/mothrix/copyright"
mkdir -p "$root/DEBIAN"
cat > "$root/DEBIAN/control" <<CONTROL
Package: mothrix
Version: $version
Architecture: $arch
Maintainer: mothrix packagers <root@localhost>
Section: net
Priority: optional
Homepage: $homepage
Depends: libc6, libgcc-s1
Recommends: xdg-utils, libnotify-bin, wl-clipboard | xclip, hunspell-en-us
Suggests: mpv
Description: terminal Matrix client with a room sidebar and mouse support
 Rooms and people are listed on the left, the conversation is on the
 right, and everything that can be done with a key can also be clicked.
 End-to-end encryption, polls, threads, spaces and several accounts.
CONTROL
out="target/mothrix_${version}_${arch}.deb"
dpkg-deb --root-owner-group --build "$root" "$out" >/dev/null
echo "$out"
