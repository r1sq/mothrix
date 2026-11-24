#!/bin/sh
# SPDX-License-Identifier: MPL-2.0
# Build a Debian package from the release binary: target/mothrix_<version>_<arch>.deb
set -eu
# Folders everyone can enter, whatever the builder's own umask is.
umask 022
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
homepage=$(sed -n 's/^repository = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(dpkg --print-architecture)
binary=${1:-target/release/mothrix}
[ -x "$binary" ] || { echo "build first: just deb (MOTHRIX_PACKAGED=1 cargo build --release)" >&2; exit 1; }

# The oldest glibc the binary runs with: the newest version among the
# symbols it takes from it. Read from the binary itself, so it holds
# wherever that was built (the release's on Ubuntu 22.04).
glibc=$(objdump -T "$binary" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1)
[ -n "$glibc" ] || { echo "$binary: could not read which glibc it needs (objdump, from binutils)" >&2; exit 1; }

root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
# mktemp makes it for the owner alone, whatever the umask; it becomes the
# package's "./", which is unpacked over the system's "/".
chmod 755 "$root"
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
Depends: libc6 (>= $glibc), libgcc-s1
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
