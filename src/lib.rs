//! `zhfmt`: add spaces between CJK and Latin characters, following Chinese
//! copywriting guidelines (写作原则).
//!
//! The transform is purely insertive (only single spaces are added, nothing is
//! ever removed), idempotent, and conservative around Markdown structures:
//! code span interiors and link URLs are left untouched, HTML tags and
//! footnote references are opaque atoms, paired emphasis is only spaced on
//! the outside of its markers, and the boundary of a code span / link is
//! decided by its inner text.

pub mod classify;
#[cfg(feature = "bin")]
pub mod config;
mod engine;
#[cfg(feature = "bin")]
pub mod process;
pub mod whitelist;

pub use engine::{format, format_with};

/// Engine options controlling the spacing transform.
#[derive(Debug, Default, Clone, Copy)]
pub struct FormatOptions<'a> {
    /// Character pairs between which no space is ever inserted; see
    /// [`whitelist::Whitelist`].
    pub whitelist: Option<&'a whitelist::Whitelist>,
}

/// Format a string, borrowing when no change is needed.
#[must_use]
pub fn format_str(input: &str) -> std::borrow::Cow<'_, str> {
    match format(input.as_bytes()) {
        // SAFETY: the engine only inserts ASCII spaces at char boundaries, so
        // the result is valid UTF-8.
        Some(out) => std::borrow::Cow::Owned(unsafe { String::from_utf8_unchecked(out) }),
        None => std::borrow::Cow::Borrowed(input),
    }
}
