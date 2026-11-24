# SPDX-License-Identifier: MPL-2.0
# Run `just` for the list.

default:
    @just --list

# Build the release binary (target/release/mothrix)
build:
    cargo build --release

# Run the tests of all crates
test:
    cargo test --workspace

# Formatting and lints, as checked before a commit
lint:
    cargo fmt --check
    cargo clippy --workspace --all-targets -- -D warnings

# Check the crates in Cargo.lock against known advisories (asks osv.dev)
audit:
    python3 -I dev/audit.py

# Install for the current user: binary, man page, menu entry
install: build
    install -Dm755 target/release/mothrix ~/.local/bin/mothrix
    install -Dm644 docs/mothrix.1 ~/.local/share/man/man1/mothrix.1
    install -Dm644 packaging/mothrix.desktop ~/.local/share/applications/mothrix.desktop
    install -Dm644 assets/mothrix-512.png ~/.local/share/icons/hicolor/512x512/apps/mothrix.png

# Build a Debian package (target/mothrix_<version>_<arch>.deb); its mothrix
# leaves updating to the package manager
deb:
    MOTHRIX_PACKAGED=1 cargo build --release
    packaging/build-deb.sh

# Build a binary that runs on any Linux of the same architecture (needs musl-tools and cmake)
static:
    rustup target add x86_64-unknown-linux-musl
    CC_x86_64_unknown_linux_musl=musl-gcc CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc cargo build --release --target x86_64-unknown-linux-musl
