//! Character classification for the spacing engine.
//!
//! Characters are grouped into classes that decide whether a space should be
//! inserted at a boundary. The engine works on raw bytes; multibyte characters
//! are decoded on demand (only at "wake" points), and malformed UTF-8 is
//! conservatively treated as [`Class::Other`] single bytes so binary-ish input
//! never breaks the scanner.
// Hot-path classification helpers are intentionally force-inlined.
#![allow(clippy::inline_always)]

/// Boundary class of a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// ASCII letters and digits: `a-z A-Z 0-9`.
    Latin,
    /// CJK ideographs, kana, hangul, bopomofo, fullwidth alphanumerics.
    Cjk,
    /// Fullwidth / CJK punctuation (`，` `。` `「」` etc.): never creates a boundary.
    Neutral,
    /// Transparent delimiter: skipped when looking for the effective boundary
    /// character. Only unpaired emphasis markers (`*`, `~`) remain soft;
    /// quotes and brackets are opaque so markup is never split from text.
    Soft,
    /// Structural delimiter that wakes the scanner (`` ` ``, `[`, `]`).
    /// (`<`, `*` and `~` also wake the scanner, but keep their own classes.)
    Hard,
    /// Whitespace: breaks any boundary.
    Space,
    /// Everything else (ASCII symbols, emoji, malformed bytes): blocks boundary.
    Other,
}

/// ASCII class lookup table, indexed by byte value (`< 0x80` is guaranteed by callers).
#[rustfmt::skip]
#[allow(clippy::cast_possible_truncation)]
const ASCII_CLASS: [Class; 128] = {
    use Class::{Hard, Latin, Other, Soft, Space};
    let mut t = [Other; 128];
    let mut i = 0;
    while i < 128 {
        t[i] = match i as u8 {
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' => Latin,
            b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C => Space,
            b'`' | b'[' | b']' => Hard,
            b'*' | b'~' => Soft,
            _ => Other,
        };
        i += 1;
    }
    t
};

/// Whether this byte can wake the SWAR scanner: non-ASCII, a hard delimiter,
/// or the start of a structural construct (`<` for HTML tags, `*`/`~` for
/// emphasis runs).
#[inline(always)]
#[must_use]
pub const fn is_wake_byte(b: u8) -> bool {
    b >= 0x80 || matches!(b, b'`' | b'[' | b']' | b'<' | b'*' | b'~')
}

#[inline(always)]
#[must_use]
pub fn ascii_class(b: u8) -> Class {
    debug_assert!(b < 0x80);
    // SAFETY: callers only pass ASCII bytes (< 0x80); the table has 128 entries.
    unsafe { *ASCII_CLASS.get_unchecked(b as usize) }
}

/// Decode the UTF-8 char starting at `pos`.
///
/// Returns `(codepoint, len)`. Malformed sequences yield `(replacement, 1)` so
/// the scanner can always make progress; callers map the replacement char to
/// [`Class::Other`].
#[inline]
#[must_use]
pub fn decode_char(input: &[u8], pos: usize) -> (u32, usize) {
    debug_assert!(pos < input.len());
    let b0 = input[pos];
    if b0 < 0x80 {
        return (u32::from(b0), 1);
    }
    let len = match b0 {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => return (char::REPLACEMENT_CHARACTER as u32, 1),
    };
    if pos + len > input.len() {
        return (char::REPLACEMENT_CHARACTER as u32, 1);
    }
    let mut cp = u32::from(b0) & (0x7f >> len);
    for &b in &input[pos + 1..pos + len] {
        if b & 0xc0 != 0x80 {
            return (char::REPLACEMENT_CHARACTER as u32, 1);
        }
        cp = (cp << 6) | (u32::from(b) & 0x3f);
    }
    // Reject overlong encodings and surrogates/out-of-range codepoints.
    let min = match len {
        2 => 0x80,
        3 => 0x800,
        _ => 0x10000,
    };
    if cp < min || cp > 0x0010_ffff || (0xd800..=0xdfff).contains(&cp) {
        return (char::REPLACEMENT_CHARACTER as u32, 1);
    }
    (cp, len)
}

/// Find the start of the UTF-8 char that ends at `end` (exclusive), i.e. walk
/// back over continuation bytes. Returns `end - 1` for malformed input.
#[inline]
#[must_use]
pub fn char_start_before(input: &[u8], end: usize) -> usize {
    debug_assert!(end > 0 && end <= input.len());
    let mut s = end - 1;
    while s > 0 && input[s] & 0xc0 == 0x80 {
        s -= 1;
    }
    s
}

/// Classify a decoded codepoint.
#[inline]
#[must_use]
pub const fn classify_codepoint(cp: u32) -> Class {
    match cp {
        // CJK ideographs (BMP + extensions), compat ideographs.
        0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xF900..=0xFAFF
        | 0x20000..=0x2A6DF
        | 0x2A700..=0x2B73F
        | 0x2B740..=0x2B81F
        | 0x2B820..=0x2CEAF
        | 0x2CEB0..=0x2EBEF
        | 0x30000..=0x3134F
        // Hiragana, katakana, kana extension.
        | 0x3040..=0x309F
        | 0x30A0..=0x30FF
        | 0x1B000..=0x1B16F
        // Bopomofo.
        | 0x3100..=0x312F
        | 0x31A0..=0x31BF
        // Hangul jamo + syllables.
        | 0x1100..=0x11FF
        | 0x3130..=0x318F
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7AF
        | 0xD7B0..=0xD7FF
        // CJK iteration marks.
        | 0x3005..=0x3007
        // Fullwidth digits and latin letters.
        | 0xFF10..=0xFF19
        | 0xFF21..=0xFF3A
        | 0xFF41..=0xFF5A
        // Halfwidth katakana.
        | 0xFF61..=0xFF9F => Class::Cjk,
        // CJK symbols & punctuation, fullwidth punctuation/symbols.
        0x3000..=0x303F | 0xFF01..=0xFF60 | 0xFFE0..=0xFFEF => Class::Neutral,
        // Curly quotes are opaque like their ASCII counterparts: they block
        // the boundary instead of letting it look through.
        _ => Class::Other,
    }
}

/// Classify the char starting at `pos`; also returns its byte length.
#[inline(always)]
#[must_use]
pub fn classify_at(input: &[u8], pos: usize) -> (Class, usize) {
    let b = input[pos];
    if b < 0x80 {
        (ascii_class(b), 1)
    } else {
        let (cp, len) = decode_char(input, pos);
        (classify_codepoint(cp), len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cls(s: &str) -> Class {
        classify_at(s.as_bytes(), 0).0
    }

    #[test]
    fn ascii_classes() {
        assert_eq!(cls("a"), Class::Latin);
        assert_eq!(cls("Z"), Class::Latin);
        assert_eq!(cls("9"), Class::Latin);
        assert_eq!(cls(" "), Class::Space);
        assert_eq!(cls("\n"), Class::Space);
        assert_eq!(cls("`"), Class::Hard);
        assert_eq!(cls("["), Class::Hard);
        assert_eq!(cls("]"), Class::Hard);
        assert_eq!(cls("*"), Class::Soft);
        assert_eq!(cls("("), Class::Other);
        assert_eq!(cls(","), Class::Other);
        assert_eq!(cls("_"), Class::Other);
        assert_eq!(cls("%"), Class::Other);
    }

    #[test]
    fn cjk_classes() {
        assert_eq!(cls("中"), Class::Cjk);
        assert_eq!(cls("あ"), Class::Cjk);
        assert_eq!(cls("ア"), Class::Cjk);
        assert_eq!(cls("한"), Class::Cjk);
        assert_eq!(cls("々"), Class::Cjk);
        assert_eq!(cls("Ａ"), Class::Cjk); // fullwidth A
        assert_eq!(cls("３"), Class::Cjk); // fullwidth 3
        assert_eq!(cls("ｱ"), Class::Cjk); // halfwidth katakana
        assert_eq!(cls("𠀀"), Class::Cjk); // ext B (4-byte)
    }

    #[test]
    fn neutral_classes() {
        assert_eq!(cls("，"), Class::Neutral);
        assert_eq!(cls("。"), Class::Neutral);
        assert_eq!(cls("「"), Class::Neutral);
        assert_eq!(cls("》"), Class::Neutral);
        assert_eq!(cls("￥"), Class::Neutral);
    }

    #[test]
    fn soft_and_other() {
        assert_eq!(cls("*"), Class::Soft);
        assert_eq!(cls("~"), Class::Soft);
        // Quotes, parens and angle brackets are opaque: they block boundaries.
        assert_eq!(cls("\""), Class::Other);
        assert_eq!(cls("'"), Class::Other);
        assert_eq!(cls("("), Class::Other);
        assert_eq!(cls(")"), Class::Other);
        assert_eq!(cls("<"), Class::Other);
        assert_eq!(cls(">"), Class::Other);
        assert_eq!(cls("“"), Class::Other);
        assert_eq!(cls("”"), Class::Other);
        assert_eq!(cls("‘"), Class::Other);
        assert_eq!(cls("«"), Class::Other);
        assert_eq!(cls("🎉"), Class::Other);
    }

    #[test]
    fn malformed_utf8() {
        assert_eq!(classify_at(&[0x80, 0x80], 0), (Class::Other, 1));
        assert_eq!(classify_at(&[0xe4, 0xb8], 0), (Class::Other, 1)); // truncated 中
        assert_eq!(classify_at(&[0xff, b'a'], 0), (Class::Other, 1));
        // overlong encoding of '/'
        assert_eq!(classify_at(&[0xc0, 0xaf], 0), (Class::Other, 1));
    }

    #[test]
    fn backward_decode() {
        let s = "a中b";
        let b = s.as_bytes();
        assert_eq!(char_start_before(b, 2), 1); // inside 中
        assert_eq!(char_start_before(b, 4), 1); // just after 中
        assert_eq!(char_start_before(b, 1), 0);
    }
}
