#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# The Rust side of the iOS app: mothrix-ffi for the iPhone and the
# simulator as ios/build/MothrixFFI.xcframework, and its Swift bindings in
# ios/Generated. Runs on a Mac with Xcode and rustup; CARGO_PROFILE=dev
# for a quicker, larger debug build, ONLY_SIMULATOR=1 for the simulator
# alone (trying things out). Honours CARGO_TARGET_DIR (the CI keeps one
# between runs).
set -euo pipefail
cd "$(dirname "$0")/.."

profile=${CARGO_PROFILE:-release}
dir=$([ "$profile" = dev ] && echo debug || echo "$profile")
out=${CARGO_TARGET_DIR:-target}
targets=(aarch64-apple-ios aarch64-apple-ios-sim)
[ -n "${ONLY_SIMULATOR:-}" ] && targets=(aarch64-apple-ios-sim)

if command -v rustup >/dev/null; then
    rustup target add "${targets[@]}" >/dev/null
fi
# Rust and the C inside (sqlite, aws-lc) for the same iOS as the app
# (project.yml); else the linker misses symbols like ___chkstk_darwin.
export IPHONEOS_DEPLOYMENT_TARGET=26.0
# Only the static library: iOS links nothing else of it.
for target in "${targets[@]}"; do
    cargo rustc -p mothrix-ffi --lib --crate-type staticlib --profile "$profile" --target "$target"
done

# The bindings come from the library itself (UniFFI's library mode).
cargo build -p mothrix-ffi --lib
rm -rf ios/Generated ios/build/headers ios/build/MothrixFFI.xcframework
mkdir -p ios/Generated ios/build/headers
# The app's version is the workspace's (Cargo.toml); project.yml takes it
# from here.
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
echo "MARKETING_VERSION = $version" > ios/build/Version.xcconfig
cargo run -q -p mothrix-ffi --bin uniffi-bindgen -- generate \
    --library "$out/debug/libmothrix_ffi.dylib" --language swift --out-dir ios/build/bindings
mv ios/build/bindings/mothrix_ffi.swift ios/Generated/
mv ios/build/bindings/mothrix_ffiFFI.h ios/build/headers/
mv ios/build/bindings/mothrix_ffiFFI.modulemap ios/build/headers/module.modulemap
rm -rf ios/build/bindings

args=()
for target in "${targets[@]}"; do
    args+=(-library "$out/$target/$dir/libmothrix_ffi.a" -headers ios/build/headers)
done
xcodebuild -create-xcframework "${args[@]}" -output ios/build/MothrixFFI.xcframework
echo "ios/build/MothrixFFI.xcframework and ios/Generated/mothrix_ffi.swift are ready."
