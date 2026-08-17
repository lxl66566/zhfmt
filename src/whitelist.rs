//! Configurable pair whitelist: character pairs between which no space is
//! ever inserted.
//!
//! The engine inserts a space at any `Latin` ↔ `Cjk` seam; the whitelist
//! vetoes such insertions for specific adjacent character pairs (e.g.
//! `"B站"` stays as written). Pairs are matched against the *actual*
//! adjacent characters of the seam, so a transparent delimiter between them
//! (`*`/`~` emphasis markers) means the seam does not match.
//!
//! Performance: lookup is a binary search over a sorted, deduplicated packed
//! `u64` array — no hashing, deterministic, allocation-free after
//! construction, and shareable by reference across parallel workers.
//! Boundary events are rare (only at `Latin` ↔ `Cjk` seams), so the
//! per-event cost is negligible; with no whitelist configured the engine
//! skips the check entirely (a single `Option` branch per insertion point).

use crate::classify::{char_start_before, decode_char};

/// A compiled set of `(left, right)` character-pair exclusions.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Whitelist {
    /// Packed `(left << 32) | right`, sorted and deduplicated.
    pairs: Box<[u64]>,
}

impl Whitelist {
    /// Build a whitelist from `(left, right)` codepoint pairs, sorting and
    /// deduplicating them.
    #[must_use]
    pub fn new(pairs: Vec<(u32, u32)>) -> Self {
        let mut packed: Vec<u64> = pairs
            .into_iter()
            .map(|(l, r)| (u64::from(l) << 32) | u64::from(r))
            .collect();
        packed.sort_unstable();
        packed.dedup();
        Self {
            pairs: packed.into_boxed_slice(),
        }
    }

    /// Whether the whitelist is empty (the engine skips the check then).
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// Whether the pair `(left, right)` is whitelisted.
    #[inline]
    #[must_use]
    pub fn contains(&self, left: u32, right: u32) -> bool {
        let key = (u64::from(left) << 32) | u64::from(right);
        self.pairs.binary_search(&key).is_ok()
    }

    /// Whether a space would be vetoed at the seam in `input` between the
    /// char ending at `at` (exclusive) and the char starting at `at`.
    ///
    /// Positions outside `(0, len)` cannot be a real seam (a space is never
    /// inserted at the very start or end of the input), so they never veto.
    #[inline]
    #[must_use]
    pub(crate) fn vetoes_at(&self, input: &[u8], at: usize) -> bool {
        if at == 0 || at >= input.len() {
            return false;
        }
        let left = if input[at - 1] < 0x80 {
            u32::from(input[at - 1])
        } else {
            decode_char(input, char_start_before(input, at)).0
        };
        let right = if input[at] < 0x80 {
            u32::from(input[at])
        } else {
            decode_char(input, at).0
        };
        self.contains(left, right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wl(pairs: &[(&str, &str)]) -> Whitelist {
        Whitelist::new(
            pairs
                .iter()
                .map(|(l, r)| {
                    (
                        l.chars().next().unwrap() as u32,
                        r.chars().next().unwrap() as u32,
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn sorts_and_deduplicates() {
        let w = Whitelist::new(vec![(2, 1), (1, 2), (1, 2), (3, 3)]);
        assert_eq!(w.pairs.as_ref(), &[
            0x0000_0001_0000_0002,
            0x0000_0002_0000_0001,
            0x0000_0003_0000_0003
        ]);
        assert!(w.contains(1, 2));
        assert!(w.contains(2, 1));
        assert!(!w.contains(1, 1));
    }

    #[test]
    fn empty_never_vetoes() {
        let w = Whitelist::default();
        assert!(w.is_empty());
        assert!(!w.vetoes_at("中a".as_bytes(), 3));
    }

    #[test]
    fn vetoes_at_matches_actual_adjacent_chars() {
        let w = wl(&[("中", "a")]);
        let s = "中a";
        assert!(w.vetoes_at(s.as_bytes(), 3));
        // Reverse order is a different seam.
        assert!(!w.vetoes_at("a中".as_bytes(), 1));
        // ASCII-ASCII pair.
        let w2 = wl(&[("a", "b")]);
        assert!(w2.vetoes_at("ab".as_bytes(), 1));
        // Multibyte right side.
        let w3 = wl(&[("a", "中")]);
        assert!(w3.vetoes_at("a中".as_bytes(), 1));
    }

    #[test]
    fn vetoes_at_guards_input_edges() {
        let w = wl(&[("a", "b")]);
        assert!(!w.vetoes_at("ab".as_bytes(), 0));
        assert!(!w.vetoes_at("ab".as_bytes(), 2));
    }
}
