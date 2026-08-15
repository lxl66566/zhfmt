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
//! For *prose* interiors (emphasis, link text) the span-aware variants
//! [`prose_first_class`] / [`prose_last_class`] additionally look through
//! code spans (paired spans decide by their interior, unpaired backticks
//! are transparent).
//!
//! The structural scanners ([`scan_html_tag`], [`find_url_end`],
//! [`find_closing_run`]) measure how far an opaque or skipped construct
//! extends, which the [`Formatter`](super::Formatter) event handlers need to
//! advance over it without inspecting its interior.
// Hot-path helpers are intentionally force-inlined.
#![allow(clippy::inline_always)]

use memchr::{memchr, memchr3};

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

/// Halfwidth punctuation that attaches to the preceding word: looking
/// backward across it is transparent, so `中文,english` spaces after the
/// comma and `C++语言` after the pluses. `!` is excluded (it would split
/// `![alt](url)` images from preceding text); quotes and parens stay opaque
/// by design.
#[inline(always)]
pub(super) const fn is_trailing_punct(b: u8) -> bool {
    matches!(b, b',' | b'.' | b';' | b':' | b'?' | b'%' | b'+')
}

/// If a run of trailing punctuation starting at `from` is immediately
/// followed by an ASCII Latin char, return that char's position — the
/// insertion point after the punctuation run (`中文,english` → before `e`).
#[inline]
pub(super) fn latin_after_punct(input: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while input.get(i).is_some_and(|&b| is_trailing_punct(b)) {
        i += 1;
    }
    if i > from && input.get(i).is_some_and(u8::is_ascii_alphanumeric) {
        return Some(i);
    }
    None
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
                // Trailing punctuation attaches to the preceding word.
                _ if is_trailing_punct(b) => j -= 1,
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

/// Class of the first content char in *prose* `text` (emphasis or
/// link-text interior): like [`peek_forward_class`], but code-span aware —
/// a paired span decides the boundary by its interior content (the same
/// rule a bare span follows in the main scan), and an unpaired backtick
/// run is transparent.
pub(super) fn prose_first_class(text: &[u8]) -> Option<Class> {
    let mut i = 0;
    while i < text.len() {
        if text[i] == b'`' {
            let k = run_len(text, i, b'`');
            if let Some(close) = find_code_span(text, i, k) {
                // The interior is code: its raw first content char decides,
                // without looking through it any further.
                return peek_forward_class(&text[i + k..close], 0);
            }
            i += k;
        } else {
            match classify_at(text, i) {
                (Class::Soft, _) => i += 1,
                (Class::Space | Class::Hard, _) => return None,
                (c, _) => return Some(c),
            }
        }
    }
    None
}

/// Class of the last content char in *prose* `text` (emphasis or
/// link-text interior): like [`last_content_class`], but code-span aware.
/// Replays the main scanner's span pairing left to right so paired spans
/// decide by their interior and unpaired backtick runs stay transparent.
pub(super) fn prose_last_class(text: &[u8]) -> Option<Class> {
    let mut last = None;
    let mut i = 0;
    while i < text.len() {
        if text[i] == b'`' {
            let k = run_len(text, i, b'`');
            match find_code_span(text, i, k) {
                Some(close) => {
                    last = last_content_class(&text[i + k..close]);
                    i = close + k;
                },
                None => i += k,
            }
        } else {
            let (class, len) = classify_at(text, i);
            match class {
                Class::Soft | Class::Space => {},
                // Brackets block the boundary, consistent with the raw
                // lookup; later content overrides them.
                Class::Hard => last = None,
                c => last = Some(c),
            }
            i += len;
        }
    }
    last
}

/// Whether this byte is ASCII whitespace. Multibyte chars (>= 0x80) are not.
#[inline(always)]
pub(super) const fn is_space_byte(b: u8) -> bool {
    b < 0x80 && matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Length of the run of byte `c` starting at `pos`.
#[inline(always)]
pub(super) fn run_len(input: &[u8], pos: usize, c: u8) -> usize {
    let mut n = 0;
    while input.get(pos + n) == Some(&c) {
        n += 1;
    }
    n
}

/// Find the closing backtick run for the opening run of length `n` at
/// `pos`: the next run of *exactly* the same length (CommonMark pairing).
/// Returns the closer's start position, or `None` if the run is unclosed
/// (a literal, transparent backtick).
pub(super) fn find_code_span(input: &[u8], pos: usize, n: usize) -> Option<usize> {
    let mut scan = pos + n;
    while let Some(off) = memchr(b'`', &input[scan..]) {
        let c = scan + off;
        let m = run_len(input, c, b'`');
        if m == n {
            return Some(c);
        }
        scan = c + m;
    }
    None
}

/// Maximum distance between emphasis delimiters for them to pair: the
/// closing-run search stops at this window, keeping the scan linear on
/// pathological inputs (`*a *a *a ...` would otherwise rescan the whole
/// input per opening run, O(n²)). Real emphasis spans are far shorter;
/// pairs farther apart are treated as unpaired (transparent markers).
pub(super) const MAX_EMPHASIS_SPAN: usize = 4096;

/// Find a closing emphasis run of delimiter `c` with exactly length `n`,
/// searching from `from` up to `until` (exclusive bound for the closer's
/// start). A closing run must be immediately preceded by a non-whitespace
/// byte (crude CommonMark right-flanking).
///
/// Code spans and HTML tags/comments are skipped atomically: a delimiter
/// inside them is code or markup, never an emphasis marker. Skipping
/// spans mirrors the main scanner's pairing, so a paired wrapper can
/// never jump *into* a span — which would desync all later backtick
/// pairing and stuff spaces inside code spans.
pub(super) fn find_closing_run(
    input: &[u8],
    from: usize,
    until: usize,
    c: u8,
    n: usize,
) -> Option<usize> {
    let mut scan = from;
    while let Some(off) = memchr3(b'`', b'<', c, input.get(scan..until).unwrap_or(&[])) {
        let start = scan + off;
        match input[start] {
            b'`' => {
                let k = run_len(input, start, b'`');
                // Skip the whole span when the run pairs (the closer may
                // lie beyond `until`: the window then holds no valid
                // closer). An unpaired run is a literal and its interior
                // stays prose.
                scan = find_code_span(input, start, k)
                    .map_or(start + k, |close| (close + k).min(until));
            },
            b'<' => {
                scan = if input[start..].starts_with(b"<!--")
                    && let Some(end) = find_comment_end(input, start)
                {
                    end
                } else {
                    // A `<` that opens no valid tag is a literal char.
                    scan_html_tag(input, start).unwrap_or(start + 1)
                }
                .min(until);
            },
            // The delimiter byte `c` (`*` or `~`).
            _ => {
                let m = run_len(input, start, c);
                if m == n && start > 0 && !is_space_byte(input[start - 1]) {
                    return Some(start);
                }
                // Runs of another length are atomic; skip the whole run.
                scan = start + m;
            },
        }
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
