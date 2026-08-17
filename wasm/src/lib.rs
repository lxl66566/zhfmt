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

/// Add spaces between CJK and Latin characters, except between the given
/// character pairs. `whitelist` is an array of two-character strings, e.g.
/// `["B站"]` keeps `B站` unspaced.
///
/// Returns the input unchanged when no spacing fix is needed.
#[wasm_bindgen]
#[must_use]
pub fn format_with(input: &str, whitelist: &js_sys::Array) -> String {
    let pairs: Vec<(u32, u32)> = whitelist
        .iter()
        .filter_map(|v| v.as_string())
        .filter_map(|s| {
            let mut chars = s.chars();
            match (chars.next(), chars.next(), chars.next()) {
                (Some(left), Some(right), None) => Some((left as u32, right as u32)),
                _ => None,
            }
        })
        .collect();
    let w = zhfmt::whitelist::Whitelist::new(pairs);
    let opts = zhfmt::FormatOptions {
        whitelist: (!w.is_empty()).then_some(&w),
    };
    match zhfmt::format_with(input.as_bytes(), opts) {
        // SAFETY: the engine only inserts ASCII spaces at char boundaries,
        // so the result is valid UTF-8.
        Some(out) => unsafe { String::from_utf8_unchecked(out) },
        None => input.to_string(),
    }
}
