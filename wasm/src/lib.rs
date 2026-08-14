//! WASM bindings for `zhfmt`, consumed by `prettier-plugin-zhfmt`.
//!
//! The binary-only features (CLI, config discovery, parallel walker) are
//! disabled; only the pure formatting engine is exposed.

use wasm_bindgen::prelude::*;

/// Add spaces between CJK and Latin characters.
///
/// Returns the input unchanged when no spacing fix is needed.
#[wasm_bindgen]
#[must_use]
pub fn format(input: &str) -> String {
    zhfmt::format_str(input).into_owned()
}
