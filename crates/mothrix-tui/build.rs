// SPDX-License-Identifier: MPL-2.0

// The version shown to the user, with the commit for builds that are not
// a release (`../build-version.rs`).
include!("../build-version.rs");

fn main() {
    emit_version();
}
