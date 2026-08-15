//! The core spacing engine: a single-pass scanner over raw bytes.
//!
//! Design notes (performance):
//!
//! - Insertions can only ever happen at a boundary where one side is a CJK (non-ASCII) character,
//!   or at a structural delimiter (`` ` ``, `[`, `]`) whose inner content decides the boundary.
//!   Everything else is copied verbatim. The scanner therefore skips over "boring" bytes with a
//!   hybrid scan (inline SWAR first word + SIMD continuation: AVX2 / SSE2 on x86_64, selected at
//!   runtime; SWAR elsewhere) and only wakes on bytes `>= 0x80`, the structural delimiters and `\n`
//!   (line starts matter for indented code blocks). See [`scan`] for the scanning layer.
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
//! - Fenced code blocks are skipped as one big code span via backtick-run pairing; indented code
//!   blocks (a line indented by 4+ spaces or a tab, following a blank line) and YAML front matter
//!   are skipped verbatim — their content is hardcode, never prose.
//! - HTML tags (`<...>`) and footnote references (`[^id]`) are opaque atoms: their interiors
//!   (including attribute values such as `title="中文"`) are never formatted, and they block
//!   boundaries on both sides. HTML comments (`<!-- ... -->`) keep the opaque boundary behavior but
//!   their body is prose and gets formatted recursively.
//! - Emphasis runs (`*...*`, `**...**`, `~~...~~`) act as wrappers when a matching closing run
//!   exists within a bounded window (see [`MAX_EMPHASIS_SPAN`]): the outer boundary is decided by
//!   the interior's content and spaces are only ever placed *outside* the markers, so `CG**鉴赏**`
//!   becomes `CG **鉴赏**`, never `CG** 鉴赏**`. The closer search skips code spans and HTML
//!   tags/comments atomically (a `*` inside them is code or markup, never a delimiter), so a stray
//!   prose `*` can never pair into a span and desync the backtick pairing. An opener whose closer
//!   lies beyond the window is paired with it optimistically (pending opener), keeping spaces
//!   outside the markers as well; any other unpaired run is literal text and its boundary hugs the
//!   CJK side.
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
    MAX_EMPHASIS_SPAN, crosses, find_closing_run, find_code_span, find_comment_end, find_url_end,
    front_matter_end, is_indented_code_line, is_space_byte, last_content_class, latin_after_punct,
    lookback_class, peek_forward_class, prev_line_blank, prose_first_class, prose_last_class,
    run_len, scan_html_tag, scan_indented_code,
};
use scan::{Scan, find_ascii, find_wake};

/// Format `input`, returning `None` when no change is needed.
#[must_use]
pub fn format(input: &[u8]) -> Option<Vec<u8>> {
    Formatter::new(input).run()
}

/// Which side of an emphasis run a boundary space belongs to; see
/// [`Formatter::emphasis_edge`].
#[derive(Clone, Copy)]
enum RunRole {
    /// Opening markers: the space goes before the run.
    Open,
    /// Closing markers: the space goes after the run.
    Close,
    /// Literal (unpaired) markers: the space hugs the CJK side.
    Literal,
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
    /// Openers whose closer lies beyond the emphasis pairing window, awaiting
    /// that far closer: `pending_star[len - 1]` tracks `*` runs of length
    /// 1..=3, `pending_tilde` tracks `~~`.
    pending_star: [bool; 3],
    pending_tilde: bool,
    /// Memoized "delimiter occurs beyond the window" tail searches, one slot
    /// per delimiter byte (`*` at 0, `~` at 1); see [`Self::delim_beyond`].
    tail_memo: [Option<(usize, Option<usize>)>; 2],
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
            pending_star: [false; 3],
            pending_tilde: false,
            tail_memo: [None; 2],
            scan: Scan::select(),
        }
    }

    fn run(mut self) -> Option<Vec<u8>> {
        let len = self.input.len();
        // YAML front matter is file-level metadata: skip it verbatim. (A
        // recursive slice from splice_formatted starting with `---` would be
        // skipped too — a harmless conservative miss.) `last` stays at 0 so
        // the skipped prefix is still part of the first COW flush.
        if let Some(end) = front_matter_end(self.input) {
            self.pos = end;
        }
        // An indented code block may also open the document (or directly
        // follow the front matter).
        if is_indented_code_line(self.input, self.pos) {
            self.pos = scan_indented_code(self.input, self.pos);
        }
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
                b'\n' => self.on_newline(),
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
        if last_class == Class::Cjk {
            if self.input.get(end).is_some_and(u8::is_ascii_alphanumeric) {
                self.insert_space(end);
            } else if let Some(at) = latin_after_punct(self.input, end) {
                // `中文,english`: the comma attaches to the preceding word;
                // the boundary space goes after the punctuation run.
                self.insert_space(at);
            }
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
        let n = run_len(self.input, self.pos, b'`');
        // Find the closing run of exactly the same length.
        match find_code_span(self.input, self.pos, n) {
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
            // Unclosed: treat as transparent, keep `prev`.
            None => self.pos += n,
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

    /// An emphasis run (`*` or `~`). Three outcomes:
    ///
    /// - A closing run of the same length exists within the bounded window ([`MAX_EMPHASIS_SPAN`]):
    ///   the run pair is a wrapper — the outer boundary is decided by the interior's content,
    ///   spaces are only placed *outside* the markers, and the interior is formatted recursively.
    /// - No closer within the window, but the delimiter occurs again farther ahead
    ///   ([`Self::delim_beyond`]): the run is likely an opener whose closer lies beyond the window.
    ///   It is recorded as pending and the matching later run becomes its closer; both edges behave
    ///   like the wrapper case, so a space never lands between a marker and its content (which
    ///   would destroy the delimiter's flanking and disable the emphasis in the rendered output).
    /// - Otherwise the run is literal text: the boundary hugs the CJK side, as if the markers
    ///   weren't there.
    fn on_emphasis(&mut self) {
        let len = self.input.len();
        let c = self.input[self.pos];
        let n = run_len(self.input, self.pos, c);
        // Strikethrough is `~~`; a single `~` stays literal.
        let pairable = c == b'*' || n == 2;
        let after = self.pos + n;
        // An opening run must be immediately followed by a non-whitespace
        // char, a closing run immediately preceded by one (crude CommonMark
        // flanking; the former rejects list bullets `* `).
        let left_flanking = after < len && !is_space_byte(self.input[after]);
        let right_flanking = self.pos > 0 && !is_space_byte(self.input[self.pos - 1]);
        if pairable {
            // A closer matches the oldest pending opener first, mirroring
            // CommonMark's delimiter stack.
            if right_flanking && self.take_pending(c, n) {
                self.emphasis_edge(after, RunRole::Close);
                self.pos = after;
                return;
            }
            if left_flanking {
                // The closer search is bounded (see MAX_EMPHASIS_SPAN):
                // without the window, inputs like `*a *a *a ...` would
                // rescan the whole input per opening run.
                let until = (after + MAX_EMPHASIS_SPAN).min(len);
                if let Some(close) = find_closing_run(self.input, after, until, c, n) {
                    let interior = &self.input[after..close];
                    // Prose interior: paired code spans inside decide the
                    // wrapper's boundary by their content.
                    let first = prose_first_class(interior);
                    // `self.pos > self.last`: defensive against double
                    // insertion if a previous handler already inserted here.
                    if self.pos > self.last && crosses(self.prev, first) {
                        self.insert_space(self.pos);
                    }
                    self.splice_formatted(after..close);
                    self.prev = prose_last_class(interior);
                    self.pos = close + n;
                    self.insert_right_boundary();
                    return;
                }
                if self.delim_beyond(c, until) && self.set_pending(c, n) {
                    self.emphasis_edge(after, RunRole::Open);
                    self.pos = after;
                    return;
                }
            }
        }
        self.emphasis_edge(after, RunRole::Literal);
        self.pos = after;
    }

    /// Boundary handling for the seams around an emphasis run whose role is
    /// `role`; `after` is the position just past the run. A run that already
    /// hugs whitespace on either side belongs to the other side's content —
    /// its seams are already expressed, so nothing is inserted (this keeps
    /// the transform idempotent).
    fn emphasis_edge(&mut self, after: usize, role: RunRole) {
        let Some(first) = peek_forward_class(self.input, after) else {
            // Whitespace, a hard delimiter or EOI follows: that seam is owned
            // by the following construct's handler (or nobody).
            if self.input.get(after).is_some_and(|&b| is_space_byte(b)) {
                self.prev = None;
            }
            return;
        };
        let hugged = (self.pos > 0 && is_space_byte(self.input[self.pos - 1]))
            || is_space_byte(self.input[after]);
        if !hugged && crosses(self.prev, Some(first)) {
            match role {
                // Spaces outside the markers keep the delimiter's flanking.
                RunRole::Open => self.insert_space(self.pos),
                RunRole::Close => self.insert_space(after),
                // Literal markers: the space hugs the CJK side, as if the
                // run weren't there.
                RunRole::Literal => match self.prev {
                    Some(Class::Cjk) => self.insert_space(self.pos),
                    _ => self.insert_space(after),
                },
            }
        }
        self.prev = Some(first);
    }

    /// The pending-opener slot for delimiter `c` and run length `n`. `*`
    /// runs longer than 3 are too rare to track (they degrade to literal);
    /// only `~~` is pairable among `~` runs (see [`Self::on_emphasis`]).
    fn pending_slot(&mut self, c: u8, n: usize) -> Option<&mut bool> {
        match c {
            b'~' => Some(&mut self.pending_tilde),
            b'*' => self.pending_star.get_mut(n - 1),
            _ => None,
        }
    }

    /// Record an opener whose closer lies beyond the pairing window.
    /// Returns false when the run length is not trackable.
    fn set_pending(&mut self, c: u8, n: usize) -> bool {
        if let Some(slot) = self.pending_slot(c, n) {
            *slot = true;
            return true;
        }
        false
    }

    /// Consume a pending opener matching this run, if any.
    fn take_pending(&mut self, c: u8, n: usize) -> bool {
        self.pending_slot(c, n).is_some_and(std::mem::take)
    }

    /// Whether delimiter `c` occurs again at or after `from` (i.e. beyond
    /// the pairing window). The tail `memchr` is O(remaining input) per
    /// call, but a memoized hit stays valid for every later query starting
    /// at or before it, so the total stays linear on pathological inputs
    /// like `*a *a *a ...`.
    fn delim_beyond(&mut self, c: u8, from: usize) -> bool {
        let slot = &mut self.tail_memo[usize::from(c == b'~')];
        if let Some((query, hit)) = *slot
            && query <= from
            && hit.is_none_or(|h| h >= from)
        {
            return hit.is_some();
        }
        let hit = memchr(c, &self.input[from..]).map(|o| from + o);
        *slot = Some((from, hit));
        hit.is_some()
    }

    /// `\n`: line boundaries matter for exactly one construct — indented
    /// code blocks. A line indented by 4+ spaces or a tab that follows a
    /// blank line (or opens the document) is code: CommonMark says indented
    /// code cannot interrupt a paragraph. Its content is hardcode, never
    /// prose, so the whole block is skipped verbatim.
    fn on_newline(&mut self) {
        self.prev = None;
        let line = self.pos + 1;
        if prev_line_blank(self.input, self.pos) && is_indented_code_line(self.input, line) {
            self.pos = scan_indented_code(self.input, line);
        } else {
            self.pos += 1;
        }
    }

    /// `[`: inline-link text (`[text](url)`) is prose and decides the left
    /// boundary. Everything else bracketed is an opaque atom instead — see
    /// below — and never creates a boundary on either side.
    fn on_bracket_open(&mut self) {
        let len = self.input.len();
        if self.pos + 1 < len
            && self.input[self.pos + 1] == b'^'
            && let Some(off) = memchr(b']', &self.input[self.pos + 2..])
            && self.input.get(self.pos + 2 + off + 1) != Some(&b'(')
        {
            self.prev = Some(Class::Other);
            self.pos += 2 + off + 1;
            return;
        }
        // A bracketed group that is NOT an inline link (no `(` after `]`)
        // is an opaque atom — reference-style/shortcut links and literal
        // bracket annotations alike: like parens and quotes, the brackets
        // hug their content and never create boundaries.
        if let Some(off) = memchr(b']', &self.input[self.pos + 1..]) {
            let close = self.pos + 1 + off;
            if self.input.get(close + 1) != Some(&b'(') {
                self.prev = Some(Class::Other);
                self.pos = close + 1;
                return;
            }
        }
        // Link text is prose: a code span inside it decides the boundary by
        // its interior content.
        let first = prose_first_class(&self.input[self.pos + 1..]);
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
                        self.prev = prose_last_class(&self.input[open + 1..self.pos]);
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
