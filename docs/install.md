# Installing mothrix

mothrix is built from source with a current stable Rust. Building needs
`cmake` and `clang` besides cargo (for the TLS library); running needs
nothing but the C library. Optional, and used when present:
`xdg-utils` (opening links and attachments), `libnotify` (desktop
notifications), `wl-clipboard` or `xclip` (pasting pictures), Hunspell
dictionaries (spell checking), `mpv` (playing audio).

## Download

Each release on [GitHub](https://github.com/r1sq/mothrix/releases) has
the Linux binary (x86_64), a `.deb`, and the Windows `.exe`, with their
checksums (`SHA256SUMS`). The Linux binary needs glibc 2.35 (Ubuntu
22.04) or newer; elsewhere build it yourself or use `just static`. The
binary and the `.exe` update themselves; the `.deb` leaves that to your
package manager ([updates.md](updates.md)).

## Build and run

```sh
cargo build --release
./target/release/mothrix            # asks for your homeserver on the first start
```

`mothrix login [SERVER]` signs in without starting the interface;
`SERVER` is a homeserver name or URL, or a full Matrix ID. `mothrix
--help` and `man mothrix` list the rest. Your homeserver needs sliding
sync (Synapse and Continuwuity have it).

## Installing

[just](https://github.com/casey/just) recipes:

```sh
just install                     # binary, man page, menu entry and icon into ~/.local
just deb                         # target/mothrix_<version>_<arch>.deb
sudo apt install ./target/mothrix_*.deb
just static                      # a binary without system libraries (needs musl-tools and cmake)
```

For Arch Linux, `packaging/arch/PKGBUILD` builds from the checkout
(`cd packaging/arch && makepkg -si`).

## macOS and Windows

mothrix is developed and tested on Linux, but nothing in it is tied to
Linux:

- **Windows 11**, in Windows Terminal (1.22 or newer for sharp pictures):
  cross-compiled with `cargo build --release --target
  x86_64-pc-windows-gnu` (needs MinGW). Notifications are toasts; pasting
  pictures is not there.
- **macOS**: should build with `cargo build --release` (never tried). The
  clipboard (`pbcopy`, `pbpaste`; pictures need `pngpaste`),
  notifications and opening attachments are wired up.

Where the files go on each system: [configuration.md](configuration.md).

## The terminal

Sharp pictures need a terminal with the kitty graphics protocol (kitty,
Ghostty) or Sixel (foot, WezTerm, Konsole, Windows Terminal, xterm as
VT340); everywhere else they are drawn with coloured half blocks, which
needs true colour. Inside tmux or screen, set `images = "blocks"`.
