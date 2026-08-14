//! Boundary decision layer: pure queries about the characters and structural
//! constructs around a position.
//!
//! A space is inserted at a seam iff the effective content classes on both
//! sides are `Latin` and `Cjk` in some order ([`crosses`]). The effective
//! class is found by looking through transparent ([`Class::Soft`]) delimiters:
//!
//! - forward: [`peek_forward_class`] / [`next_is_latin`]
//! - backward: [`lookback_class`] / [`last_content_class`]
//!
//! The structural scanners ([`scan_html_tag`], [`find_url_end`],
//! [`find_closing_run`]) measure how far an opaque or skipped construct
//! extends, which the [`Formatter`](super::Formatter) event handlers need to
//! advance over it without inspecting its interior.
// Hot-path helpers are intentionally force-inlined.
#![allow(clippy::inline_always)]

use memchr::memchr;

use crate::classify::{Class, ascii_class, char_start_before, classify_at};

/// Whether a space must be inserted between these two classes.
#[inline(always)]
pub(super) const fn crosses(left: Option<Class>, right: Option<Class>) -> bool {
    matches!(
        (left, right),
        (Some(Class::Latin), Some(Class::Cjk)) | (Some(Class::Cjk), Some(Class::Latin))
    )
}

/// Class of the first *content* char at or after `from`, looking through
/// [`Class::Soft`] delimiters. Returns `None` on whitespace, a hard delimiter
/// or end of input (the boundary is then owned by someone else / nobody).
pub(super) fn peek_forward_class(input: &[u8], from: usize) -> Option<Class> {
    let mut i = from;
    while i < input.len() {
        let (class, len) = classify_at(input, i);
        match class {
            Class::Soft => i += len,
            Class::Space | Class::Hard => return None,
            c => return Some(c),
        }
    }
    None
}

/// Whether the next *content* char at or after `from` is ASCII Latin,
/// looking through [`Class::Soft`] delimiters. Equivalent to
/// `peek_forward_class(input, from) == Some(Class::Latin)` but without
/// decoding multibyte chars: `Latin` is always ASCII, so any byte `>= 0x80`
/// immediately rules it out. `Soft` is likewise ASCII-only (`*`, `~`).
#[inline]
pub(super) fn next_is_latin(input: &[u8], from: usize) -> bool {
    let mut i = from;
    while i < input.len() {
        match input[i] {
            b'*' | b'~' => i += 1,
            b => return b.is_ascii_alphanumeric(),
        }
    }
    false
}

/// Class of the last content char before `pos`, used after copying a pure
/// ASCII run. `Soft` chars are skipped; a `Hard` char or the region start
/// means the boundary was already decided by a previous event, so the current
/// `prev` is kept.
pub(super) fn lookback_class(
    input: &[u8],
    region_start: usize,
    pos: usize,
    prev: Option<Class>,
) -> Option<Class> {
    let mut j = pos;
    while j > region_start {
        let b = input[j - 1];
        if b < 0x80 {
            match ascii_class(b) {
                Class::Soft => j -= 1,
                Class::Space => return None,
                Class::Hard => return prev,
                c => return Some(c),
            }
        } else {
            let s = char_start_before(input, j);
            match classify_at(input, s).0 {
                Class::Soft => j = s,
                c => return Some(c),
            }
        }
    }
    prev
}

/// Class of the last content char in `text` (skipping `Soft` and whitespace),
/// searching backwards from the end. Used for code spans and link texts.
pub(super) fn last_content_class(text: &[u8]) -> Option<Class> {
    let mut j = text.len();
    let prev = None;
    while j > 0 {
        let b = text[j - 1];
        if b < 0x80 {
            match ascii_class(b) {
                Class::Soft | Class::Space => j -= 1,
                Class::Hard => return prev,
                c => return Some(c),
            }
        } else {
            let s = char_start_before(text, j);
            match classify_at(text, s).0 {
                Class::Soft => j = s,
                c => return Some(c),
            }
        }
    }
    prev
}

/// Whether this byte is ASCII whitespace. Multibyte chars (>= 0x80) are not.
#[inline(always)]
pub(super) const fn is_space_byte(b: u8) -> bool {
    b < 0x80 && matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Find a closing emphasis run of delimiter `c` with exactly length `n`,
/// searching from `from`. A closing run must be immediately preceded by a
/// non-whitespace byte (crude CommonMark right-flanking).
pub(super) fn find_closing_run(input: &[u8], from: usize, c: u8, n: usize) -> Option<usize> {
    let mut scan = from;
    while let Some(off) = memchr(c, &input[scan..]) {
        let start = scan + off;
        let mut m = 0;
        while start + m < input.len() && input[start + m] == c {
            m += 1;
        }
        if m == n && start > 0 && !is_space_byte(input[start - 1]) {
            return Some(start);
        }
        scan = start + m;
    }
    None
}

/// Find the end of an HTML comment starting at `pos` (which must point at
/// `<!--`). Returns the position just past the closing `-->`, or `None` if
/// the comment is unterminated. A `>` inside the comment body (e.g.
/// `<!-- a>b -->`) does not close it.
pub(super) fn find_comment_end(input: &[u8], pos: usize) -> Option<usize> {
    debug_assert!(input[pos..].starts_with(b"<!--"));
    let mut i = pos + 4;
    loop {
        // SAFETY-free: `gt >= pos + 4`, so `gt - 2` never underflows.
        let gt = memchr(b'>', &input[i..]).map(|o| i + o)?;
        if &input[gt - 2..gt] == b"--" {
            return Some(gt + 1);
        }
        i = gt + 1;
    }
}

/// Scan an HTML tag / processing instruction starting at `<` (position
/// `pos`). Returns the position just past the closing `>`, or `None` if `<`
/// does not open a valid tag. Quoted attribute values are skipped, so `>`
/// inside `title="..."` never ends the tag early and attribute interiors
/// are never inspected. Comments are handled separately (see
/// [`find_comment_end`]); `<!DOCTYPE ...>` and friends run to the next `>`.
pub(super) fn scan_html_tag(input: &[u8], pos: usize) -> Option<usize> {
    let mut i = pos + 1;
    match *input.get(i)? {
        // Declarations (<!DOCTYPE ...>) and processing instructions (<? ...>).
        b'!' | b'?' => memchr(b'>', &input[i..]).map(|o| i + o + 1),
        _ => {
            if input[i] == b'/' {
                i += 1;
            }
            // A tag name must start with an ASCII letter.
            if input.get(i)?.is_ascii_alphabetic() {
                scan_tag_body(input, i)
            } else {
                None
            }
        },
    }
}

/// Scan from inside a tag (at its name) to just past the closing `>`,
/// skipping over quoted attribute values.
fn scan_tag_body(input: &[u8], mut i: usize) -> Option<usize> {
    loop {
        match *input.get(i)? {
            b'>' => return Some(i + 1),
            q @ (b'"' | b'\'') => {
                let close = memchr(q, &input[i + 1..])?;
                i += close + 2;
            },
            _ => i += 1,
        }
    }
}

/// Find the `)` closing a link URL starting at `from` (just after `(`).
/// Tracks one level of nested parens (Wikipedia-style URLs). Bails out on
/// whitespace or end of input (malformed / not really a URL).
pub(super) fn find_url_end(input: &[u8], from: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut i = from;
    while i < input.len() {
        match input[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            },
            b' ' | b'\t' | b'\n' | b'\r' => return None,
            _ => {},
        }
        i += 1;
    }
    None
}
