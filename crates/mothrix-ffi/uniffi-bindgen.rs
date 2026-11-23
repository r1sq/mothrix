// SPDX-License-Identifier: MPL-2.0

//! Writes the Swift side of the interface (`ios/build-rust.sh` runs it).
fn main() {
    uniffi::uniffi_bindgen_main()
}
