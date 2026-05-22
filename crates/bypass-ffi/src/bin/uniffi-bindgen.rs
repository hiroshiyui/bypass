// SPDX-License-Identifier: GPL-3.0-or-later

//! Trivial binary that runs UniFFI's bindgen CLI against the
//! current crate. Invoked by the Android Gradle build at compile
//! time (8.2) to emit Kotlin sources; runnable by hand for local
//! exploration:
//!
//! ```sh
//! cargo run -p bypass-ffi --bin uniffi-bindgen -- \
//!     generate \
//!     --library target/debug/libbypass.so \
//!     --language kotlin \
//!     --out-dir /tmp/bypass-kotlin
//! ```

fn main() {
    uniffi::uniffi_bindgen_main()
}
