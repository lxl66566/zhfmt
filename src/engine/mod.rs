//! The core spacing engine: a single-pass scanner over raw bytes.
//!
//! Design notes (performance):
//!
//! - Insertions can only ever happen at a boundary where one side is a CJK (non-ASCII) character,
//!   or at a structural delimiter (`` ` ``, `[`, `]`) whose inner content decides the boundary.
//!   Everything else is copied verbatim. The scanner therefore skips over "boring" bytes with a
//!   hybrid scan (inline SWAR first word + SIMD continuation: AVX2 / SSE2 on x86_64, selected at
//!   runtime; SWAR elsewhere) and only wakes on bytes `>= 0x80` and the three structural
//!   delimiters. See [`scan`] for the scanning layer.
//! - Consecutive multibyte characters are processed as one *run*: `Latin` is always ASCII, so no
//!   boundary can exist between two multibyte chars. Only the first and last char of a run are
//!   classified; the interior is skipped with a "first ASCII byte" scan. Malformed runs fall back
//!   to per-char processing to keep the conservative `Class::Other` semantics for bad bytes.
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
//! through, so markup is never split from the text it wraps. The effective
//! classes around a seam are determined by the pure queries in [`boundary`].
//!
//! Structural constructs recognized by the scanner:
//!
//! - Code spans (`` `...` ``) and link URLs (`[text](url)`) are skipped over; the boundary of a
//!   code span / link is decided by the first/last *content* character inside it.
//! - HTML tags (`<...>`) and footnote references (`[^id]`) are opaque atoms: their interiors
//!   (including attribute values such as `title="中文"`) are never formatted, and they block
//!   boundaries on both sides. HTML comments (`<!-- ... -->`) keep the opaque boundary behavior but
//!   their body is prose and gets formatted recursively.
//! - Emphasis runs (`*...*`, `**...**`, `~~...~~`) act as wrappers when a matching closing run
//!   exists: the outer boundary is decided by the interior's content and spaces are only ever
//!   placed *outside* the markers, so `CG**鉴赏**` becomes `CG **鉴赏**`, never `CG** 鉴赏**`.
//!   Unpaired runs stay transparent ([`Class::Soft`]).
//!
//! Module layout: [`scan`] locates the next interesting byte, [`boundary`]
//! decides effective classes and construct extents, and this module holds the
//! [`Formatter`] state machine that ties them together.
// Hot-path helpers are intentionally force-inlined.
#![allow(clippy::inline_always)]

mod boundary;

use memchr::memchr;

use crate::classify::{Class, classify_at};

mod scan;

use boundary::{
    crosses, find_closing_run, find_comment_end, find_url_end, is_space_byte, last_content_class,
    lookback_class, next_is_latin, peek_forward_class, scan_html_tag,
};
use scan::{Scan, find_ascii, find_wake};

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
    /// SIMD/SWAR scanners selected for this CPU.
    scan: Scan,
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
            scan: Scan::select(),
        }
    }

    fn run(mut self) -> Option<Vec<u8>> {
        let len = self.input.len();
        while self.pos < len {
            let wake = find_wake(self.input, self.pos, self.scan.wake());
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

    /// Right boundary of a content-wrapped construct (link text, code span,
    /// emphasis) against a Latin neighbor. The construct handlers set `prev`
    /// from the interior and jump past the closer, but a following ASCII
    /// Latin char never wakes the scanner — without this check the crossing
    /// would go unnoticed (`中[中文]b` would stay unspaced).
    fn insert_right_boundary(&mut self) {
        if matches!(self.prev, Some(Class::Cjk))
            && self.pos > self.last
            && self
                .input
                .get(self.pos)
                .is_some_and(u8::is_ascii_alphanumeric)
        {
            self.insert_space(self.pos);
        }
    }

    /// A multibyte character (CJK, fullwidth punctuation, emoji, ...).
    ///
    /// Consecutive multibyte characters are handled as one run: `Latin` is
    /// always ASCII, so no boundary can exist *inside* a run — only the first
    /// char (left boundary, `prev`) and the last char (right boundary,
    /// look-ahead) can participate in a crossing. The run interior is skipped
    /// with a bulk "first ASCII byte" scan instead of per-char decoding.
    fn on_multibyte(&mut self) {
        let start = self.pos;
        let end = find_ascii(self.input, start, self.scan.ascii());
        // Start of the last char in the run, bounded to the run itself.
        let mut last_start = end - 1;
        while last_start > start && self.input[last_start] & 0xc0 == 0x80 {
            last_start -= 1;
        }
        let (last_class, last_len) = classify_at(self.input, last_start);
        if last_start + last_len != end {
            // Malformed layout (e.g. a stray continuation byte at the run
            // tail): fall back to per-char processing so bad bytes keep their
            // conservative `Class::Other` semantics.
            while self.pos < end {
                self.on_multibyte_char();
            }
            return;
        }
        // The left boundary can only cross when the previous content char is
        // Latin, so the first char needs no decoding otherwise.
        if matches!(self.prev, Some(Class::Latin)) {
            let (first_class, _) = classify_at(self.input, start);
            if first_class == Class::Cjk {
                self.insert_space(start);
            }
        }
        if last_class == Class::Cjk && next_is_latin(self.input, end) {
            self.insert_space(end);
        }
        self.prev = Some(last_class);
        self.pos = end;
    }

    /// The original per-char handler, used as the fallback for malformed runs.
    fn on_multibyte_char(&mut self) {
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
                self.insert_right_boundary();
            },
            None => {
                // Unclosed: treat as transparent, keep `prev`.
                self.pos += n;
            },
        }
    }

    /// Splice the recursively formatted `input[range]` into the output,
    /// flushing pending bytes. Interior text of wrapper constructs
    /// (emphasis, HTML comments) is itself prose and gets formatted.
    fn splice_formatted(&mut self, range: std::ops::Range<usize>) {
        if let Some(inner) = format(&self.input[range.clone()]) {
            let out = self.out.get_or_insert_with(|| fresh_out(self.input.len()));
            out.extend_from_slice(&self.input[self.last..range.start]);
            out.extend_from_slice(&inner);
            self.last = range.end;
        }
    }

    /// `<`: an HTML tag / comment / processing instruction is an opaque atom
    /// — its interior (including attribute values such as `title="中文"` or
    /// Vue slot names like `<template #廃村少女2>`) is never formatted, and
    /// it blocks boundaries on both sides. The exception is HTML comments:
    /// their body is prose and is formatted recursively, while the comment
    /// itself still blocks boundaries on both sides. A `<` that does not
    /// start a valid tag is a literal char and blocks the boundary as well.
    fn on_tag(&mut self) {
        if self.input[self.pos..].starts_with(b"<!--")
            && let Some(end) = find_comment_end(self.input, self.pos)
        {
            self.splice_formatted(self.pos + 4..end - 3);
            self.prev = Some(Class::Other);
            self.pos = end;
            return;
        }
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
                self.splice_formatted(after..close);
                self.prev = last_content_class(interior);
                self.pos = close + n;
                self.insert_right_boundary();
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
        self.prev = first;
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
                self.insert_right_boundary();
                return;
            }
        }
        // Not a link (or malformed): treat as transparent.
        self.pos += 1;
        self.insert_right_boundary();
    }
}

/// A fresh output buffer, pre-sized for the whole input plus insertions.
fn fresh_out(input_len: usize) -> Vec<u8> {
    Vec::with_capacity(input_len + input_len / 8 + 16)
}

#[cfg(test)]
mod tests;
