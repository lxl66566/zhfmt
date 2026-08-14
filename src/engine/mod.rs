//! The core spacing engine.
//!
//! Design notes (performance):
//!
//! - Insertions can only ever happen at a boundary where one side is a CJK (non-ASCII) character,
//!   or at a structural delimiter (`` ` ``, `[`, `]`) whose inner content decides the boundary.
//!   Everything else is copied verbatim. The scanner therefore skips over "boring" bytes with a
//!   SWAR loop (8 bytes per step, no per-byte branching) and only wakes on bytes `>= 0x80` and the
//!   three structural delimiters.
//! - Output is copy-on-write: the input is scanned once, and only when the first insertion point is
//!   found do we allocate an output buffer and start copying. Files that are already correctly
//!   spaced cost one linear scan and zero allocations, and are never rewritten.
//! - The transform is purely *insertive* (only ever inserts single spaces, never deletes), which
//!   keeps it idempotent and safe.
//!
//! Boundary rules (see project docs): a single space is inserted between a
//! [`Class::Latin`] and a [`Class::Cjk`] character. Fullwidth punctuation
//! ([`Class::Neutral`]), other symbols ([`Class::Other`]) and whitespace never
//! create boundaries. Quotes, parens and stray angle brackets are opaque
//! ([`Class::Other`]): they block the boundary instead of being looked
//! through, so markup is never split from the text it wraps.
//!
//! Structural constructs recognized by the scanner:
//!
//! - Code spans (`` `...` ``) and link URLs (`[text](url)`) are skipped over; the boundary of a
//!   code span / link is decided by the first/last *content* character inside it.
//! - HTML tags / comments (`<...>`, `<!-- ... -->`) and footnote references (`[^id]`) are opaque
//!   atoms: their interiors (including attribute values such as `title="中文"`) are never
//!   formatted, and they block boundaries on both sides.
//! - Emphasis runs (`*...*`, `**...**`, `~~...~~`) act as wrappers when a matching closing run
//!   exists: the outer boundary is decided by the interior's content and spaces are only ever
//!   placed *outside* the markers, so `CG**鉴赏**` becomes `CG **鉴赏**`, never `CG** 鉴赏**`.
//!   Unpaired runs stay transparent ([`Class::Soft`]).
// Hot-path helpers are intentionally force-inlined.
#![allow(clippy::inline_always)]

use memchr::memchr;

use crate::classify::{Class, ascii_class, char_start_before, classify_at, is_wake_byte};

const SWAR_LO: u64 = 0x0101_0101_0101_0101;
const SWAR_HI: u64 = 0x8080_8080_8080_8080;

/// Whether the SWAR word contains a zero byte.
#[inline(always)]
const fn has_zero(x: u64) -> bool {
    x.wrapping_sub(SWAR_LO) & !x & SWAR_HI != 0
}

/// Whether the SWAR word contains byte `b` (`b < 0x80`).
#[inline(always)]
const fn has_byte(x: u64, b: u8) -> bool {
    has_zero(x ^ (SWAR_LO * b as u64))
}

/// Whether the SWAR word contains a wake byte (see [`is_wake_byte`]).
#[inline(always)]
fn wake_in_word(w: u64) -> bool {
    (w & SWAR_HI != 0)
        || has_byte(w, b'`')
        || has_byte(w, b'[')
        || has_byte(w, b']')
        || has_byte(w, b'<')
        || has_byte(w, b'*')
        || has_byte(w, b'~')
}

/// Find the next wake byte at or after `from`; returns `input.len()` if none.
#[inline]
fn find_wake(input: &[u8], from: usize) -> usize {
    let len = input.len();
    let mut i = from;
    while i + 8 <= len {
        // SAFETY: bounds checked by the loop condition.
        let w = u64::from_le_bytes(unsafe { input.get_unchecked(i..i + 8) }.try_into().unwrap());
        if wake_in_word(w) {
            let mut k = i;
            while !is_wake_byte(unsafe { *input.get_unchecked(k) }) {
                k += 1;
            }
            return k;
        }
        i += 8;
    }
    while i < len {
        if is_wake_byte(input[i]) {
            return i;
        }
        i += 1;
    }
    len
}

/// Whether a space must be inserted between these two classes.
#[inline(always)]
const fn crosses(left: Option<Class>, right: Option<Class>) -> bool {
    matches!(
        (left, right),
        (Some(Class::Latin), Some(Class::Cjk)) | (Some(Class::Cjk), Some(Class::Latin))
    )
}

/// Class of the first *content* char at or after `from`, looking through
/// [`Class::Soft`] delimiters. Returns `None` on whitespace, a hard delimiter
/// or end of input (the boundary is then owned by someone else / nobody).
fn peek_forward_class(input: &[u8], from: usize) -> Option<Class> {
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

/// Class of the last content char before `pos`, used after copying a pure
/// ASCII run. `Soft` chars are skipped; a `Hard` char or the region start
/// means the boundary was already decided by a previous event, so the current
/// `prev` is kept.
fn lookback_class(
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
fn last_content_class(text: &[u8]) -> Option<Class> {
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

/// Format `input`, returning `None` when no change is needed.
#[must_use]
pub fn format(input: &[u8]) -> Option<Vec<u8>> {
    Formatter::new(input).run()
}

struct Formatter<'a> {
    input: &'a [u8],
    /// Scan position.
    pos: usize,
    /// Output buffer, allocated lazily at the first insertion.
    out: Option<Vec<u8>>,
    /// Bytes of `input` before `last` have been flushed to `out`.
    last: usize,
    /// Class of the effective previous content char.
    prev: Option<Class>,
    /// Position of the most recent `[` (candidate link-text start).
    last_bracket_open: Option<usize>,
}

impl<'a> Formatter<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            pos: 0,
            out: None,
            last: 0,
            prev: None,
            last_bracket_open: None,
        }
    }

    fn run(mut self) -> Option<Vec<u8>> {
        let len = self.input.len();
        while self.pos < len {
            let wake = find_wake(self.input, self.pos);
            if wake > self.pos {
                self.prev = lookback_class(self.input, self.pos, wake, self.prev);
                self.pos = wake;
            }
            if wake >= len {
                break;
            }
            match self.input[wake] {
                b'`' => self.on_backtick(),
                b'[' => self.on_bracket_open(),
                b']' => self.on_bracket_close(),
                b'<' => self.on_tag(),
                b'*' | b'~' => self.on_emphasis(),
                _ => self.on_multibyte(),
            }
        }
        self.out.map(|mut out| {
            out.extend_from_slice(&self.input[self.last..]);
            out
        })
    }

    /// Insert a space at `at` (before the char at that position), flushing the
    /// pending input up to `at` to the output buffer.
    fn insert_space(&mut self, at: usize) {
        debug_assert!(at >= self.last);
        let out = self.out.get_or_insert_with(|| fresh_out(self.input.len()));
        out.extend_from_slice(&self.input[self.last..at]);
        out.push(b' ');
        self.last = at;
    }

    /// A multibyte character (CJK, fullwidth punctuation, emoji, ...).
    fn on_multibyte(&mut self) {
        let (class, len) = classify_at(self.input, self.pos);
        if class == Class::Cjk {
            if crosses(self.prev, Some(Class::Cjk)) {
                self.insert_space(self.pos);
            }
            if peek_forward_class(self.input, self.pos + len) == Some(Class::Latin) {
                self.insert_space(self.pos + len);
            }
        }
        // Soft chars intentionally keep `prev` so the boundary looks through them.
        if !matches!(class, Class::Soft) {
            self.prev = match class {
                Class::Space => None,
                c => Some(c),
            };
        }
        self.pos += len;
    }

    /// A backtick run: try to match a code span. Unclosed runs are treated as
    /// transparent delimiters.
    fn on_backtick(&mut self) {
        let len = self.input.len();
        let mut n = 0;
        while self.pos + n < len && self.input[self.pos + n] == b'`' {
            n += 1;
        }
        // Find the closing run of exactly the same length.
        let mut scan = self.pos + n;
        let close = loop {
            match memchr(b'`', &self.input[scan..]) {
                Some(off) => {
                    let c = scan + off;
                    let mut m = 0;
                    while c + m < len && self.input[c + m] == b'`' {
                        m += 1;
                    }
                    if m == n {
                        break Some(c);
                    }
                    scan = c + m;
                },
                None => break None,
            }
        };
        match close {
            Some(c) => {
                let interior = &self.input[self.pos + n..c];
                let first = peek_forward_class(interior, 0);
                if crosses(self.prev, first) {
                    self.insert_space(self.pos);
                }
                self.prev = last_content_class(interior);
                self.pos = c + n;
            },
            None => {
                // Unclosed: treat as transparent, keep `prev`.
                self.pos += n;
            },
        }
    }

    /// `<`: an HTML tag / comment / processing instruction is an opaque atom
    /// — its interior (including attribute values such as `title="中文"` or
    /// Vue slot names like `<template #廃村少女2>`) is never formatted, and
    /// it blocks boundaries on both sides. A `<` that does not start a valid
    /// tag is a literal char and blocks the boundary as well.
    fn on_tag(&mut self) {
        self.prev = Some(Class::Other);
        self.pos = scan_html_tag(self.input, self.pos).unwrap_or(self.pos + 1);
    }

    /// An emphasis run (`*` or `~`): try to pair it with a closing run of the
    /// same delimiter and length. A paired run is a wrapper — the outer
    /// boundary is decided by the interior's first/last content char, spaces
    /// are only placed *outside* the markers, and the interior is formatted
    /// recursively. Unpaired runs stay transparent ([`Class::Soft`]).
    fn on_emphasis(&mut self) {
        let len = self.input.len();
        let c = self.input[self.pos];
        let mut n = 0;
        while self.pos + n < len && self.input[self.pos + n] == c {
            n += 1;
        }
        // Strikethrough is `~~`; a single `~` stays transparent.
        let pairable = c == b'*' || n == 2;
        let after = self.pos + n;
        // An opening run must be immediately followed by a non-whitespace
        // char (crude CommonMark left-flanking; rejects list bullets `* `).
        if pairable && after < len && !is_space_byte(self.input[after]) {
            if let Some(close) = find_closing_run(self.input, after, c, n) {
                let interior = &self.input[after..close];
                let first = peek_forward_class(interior, 0);
                // `self.pos > self.last`: a CJK char's forward peek may have
                // already inserted a space at this exact position.
                if self.pos > self.last && crosses(self.prev, first) {
                    self.insert_space(self.pos);
                }
                if let Some(inner) = format(interior) {
                    let out = self.out.get_or_insert_with(|| fresh_out(self.input.len()));
                    out.extend_from_slice(&self.input[self.last..after]);
                    out.extend_from_slice(&inner);
                    self.last = close;
                }
                self.prev = last_content_class(interior);
                self.pos = close + n;
                return;
            }
        }
        // Unpaired: transparent delimiter, keep `prev`.
        self.pos += n;
    }

    /// `[`: the following content decides the left boundary (link text or
    /// plain bracket content). A footnote reference `[^id]` is an opaque atom
    /// instead: it never creates a boundary on either side.
    fn on_bracket_open(&mut self) {
        let len = self.input.len();
        if self.pos + 1 < len && self.input[self.pos + 1] == b'^' {
            if let Some(off) = memchr(b']', &self.input[self.pos + 2..]) {
                self.prev = Some(Class::Other);
                self.pos += 2 + off + 1;
                return;
            }
        }
        let first = peek_forward_class(self.input, self.pos + 1);
        if crosses(self.prev, first) {
            self.insert_space(self.pos);
        }
        self.last_bracket_open = Some(self.pos);
        self.pos += 1;
    }

    /// `]`: if followed by `(`, this is an inline link — skip the URL so it
    /// neither gets formatted nor influences the boundary; the link text's
    /// last content char becomes the effective previous char.
    fn on_bracket_close(&mut self) {
        let len = self.input.len();
        if self.pos + 1 < len && self.input[self.pos + 1] == b'(' {
            if let Some(end) = find_url_end(self.input, self.pos + 2) {
                // The link text is what decides the boundary, not the URL.
                if let Some(open) = self.last_bracket_open.take() {
                    if open < self.pos {
                        self.prev = last_content_class(&self.input[open + 1..self.pos]);
                    }
                }
                self.pos = end + 1;
                return;
            }
        }
        // Not a link (or malformed): treat as transparent.
        self.pos += 1;
    }
}

/// A fresh output buffer, pre-sized for the whole input plus insertions.
fn fresh_out(input_len: usize) -> Vec<u8> {
    Vec::with_capacity(input_len + input_len / 8 + 16)
}

/// Whether this byte is ASCII whitespace. Multibyte chars (>= 0x80) are not.
#[inline(always)]
const fn is_space_byte(b: u8) -> bool {
    b < 0x80 && matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Find a closing emphasis run of delimiter `c` with exactly length `n`,
/// searching from `from`. A closing run must be immediately preceded by a
/// non-whitespace byte (crude CommonMark right-flanking).
fn find_closing_run(input: &[u8], from: usize, c: u8, n: usize) -> Option<usize> {
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

/// Scan an HTML tag / comment / processing instruction starting at `<`
/// (position `pos`). Returns the position just past the closing `>`, or
/// `None` if `<` does not open a valid tag. Quoted attribute values are
/// skipped, so `>` inside `title="..."` never ends the tag early and
/// attribute interiors are never inspected.
fn scan_html_tag(input: &[u8], pos: usize) -> Option<usize> {
    let mut i = pos + 1;
    match *input.get(i)? {
        b'!' => {
            // Comment `<!-- ... -->`; anything else (`<!DOCTYPE ...>`,
            // `<![CDATA[...]]>`) runs to the next `>`.
            if input[i..].starts_with(b"<!--") {
                i += 4;
                loop {
                    let gt = memchr(b'>', &input[i..]).map(|o| i + o)?;
                    if &input[gt - 2..gt] == b"--" {
                        return Some(gt + 1);
                    }
                    i = gt + 1;
                }
            }
            memchr(b'>', &input[i..]).map(|o| i + o + 1)
        },
        // Processing instruction `<? ... >`.
        b'?' => memchr(b'>', &input[i..]).map(|o| i + o + 1),
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
fn find_url_end(input: &[u8], from: usize) -> Option<usize> {
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

#[cfg(test)]
mod tests;
